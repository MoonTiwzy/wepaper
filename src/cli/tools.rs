//! Offline tools: looking into wallpapers without showing them.

use std::path::{Path, PathBuf};

use wepaper_engine::gpu::{Context, Target, TextureFormat};
use wepaper_engine::read::Archive;
use wepaper_engine::scene::FrameInput;
use wepaper_engine::{Engine, Options, Viewport, Wallpaper, locate};
use wepaper_json::{Value, obj};

use super::{Failure, value};

pub const USAGE: &str = "\
Offline tools:
  render <wallpaper> -o FILE [options]         one frame of a wallpaper to a PNG, without a display
  inspect <wallpaper> [--json]                 what a wallpaper is made of: layers, effects, properties
  check <wallpaper or folder of them>...       load each scene and say what does not work in it
  unpack <scene.pkg or wallpaper> <folder>     the files of a package, written out
";

/// Positional arguments and the flags the tools share.
struct Arguments {
    words: Vec<String>,
    json: bool,
    assets: Option<PathBuf>,
    device: Option<String>,
}

fn arguments(mut args: lexopt::Parser, usage: &str) -> Result<Option<Arguments>, Failure> {
    use lexopt::prelude::*;
    let mut parsed = Arguments { words: Vec::new(), json: false, assets: None, device: None };
    while let Some(arg) = args.next().map_err(|e| e.to_string())? {
        match arg {
            Long("json") => parsed.json = true,
            Long("assets") => parsed.assets = Some(value(&mut args)?.into()),
            Long("device") => parsed.device = Some(value(&mut args)?),
            Short('h') | Long("help") => {
                print!("{usage}");
                return Ok(None);
            }
            Value(word) => parsed.words.push(word.to_string_lossy().into_owned()),
            other => return Err(other.unexpected().to_string()),
        }
    }
    Ok(Some(parsed))
}

/// An engine that draws to nothing.
fn headless(arguments: &Arguments) -> Result<(Context, Engine), Failure> {
    let context = Context::headless(arguments.device.as_deref()).map_err(|e| e.to_string())?;
    let options = Options {
        assets: arguments.assets.clone(),
        cache_dir: Some(super::cache_dir()),
        ..Options::default()
    };
    let engine = Engine::new(context.gl(), options).map_err(|e| e.to_string())?;
    Ok((context, engine))
}

/// `wepaper inspect`.
pub fn inspect(args: lexopt::Parser) -> Result<(), Failure> {
    let Some(arguments) =
        arguments(args, "wepaper inspect <wallpaper> [--json] [--assets DIR] [--device NODE]\n")?
    else {
        return Ok(());
    };
    let name = arguments.words.first().ok_or("which wallpaper? (a folder or a workshop id)")?;
    let (context, mut engine) = headless(&arguments)?;
    let wallpaper = engine
        .load(Path::new(name), Viewport { width: 1920, height: 1080 }, &[])
        .map_err(|e| e.to_string())?;
    let project = wallpaper.project();
    let memory = context.gl().memory();
    let mut tree = obj! {
        "title" => &project.title,
        "kind" => project.kind.name(),
        "file" => &project.file,
        "workshop_id" => project.workshop_id.clone(),
        "video_memory" => obj! { "textures" => memory.textures, "buffers" => memory.targets, "video" => memory.video },
    };
    if let (Value::Object(map), Value::Object(scene)) = (&mut tree, wallpaper.tree()) {
        for (key, value) in scene.iter() {
            map.insert(key, value.clone());
        }
    }
    if arguments.json {
        println!("{}", tree.to_json_pretty());
        return Ok(());
    }
    println!(
        "{} ({})",
        if project.title.is_empty() { "untitled" } else { &project.title },
        project.kind.name()
    );
    println!("video memory when loaded: textures {:.1} MB", memory.textures as f64 / 1048576.0);
    let layers = tree.get("layers");
    println!("layers: {}", layers.members().count());
    for (id, layer) in layers.members() {
        let effects: Vec<&str> =
            layer.get("effects").members().filter_map(|(_, effect)| effect.get("name").as_str()).collect();
        let hidden = if layer.get("visible").as_bool() == Some(false) { " (hidden)" } else { "" };
        let with =
            if effects.is_empty() { String::new() } else { format!(", effects: {}", effects.join(", ")) };
        println!(
            "  {id:>6}  {:<8} {}{hidden}{with}",
            layer.get("kind").as_str().unwrap_or("?"),
            layer.get("name").as_str().unwrap_or("")
        );
    }
    let properties = tree.get("schema");
    if properties.items().len() > 0 {
        println!("properties:");
        for property in properties.items().filter(|p| p.get("type").as_str() != Some("group")) {
            let name = property.get("name").as_str().unwrap_or("");
            println!(
                "  {name:<28} {:<8} = {}",
                property.get("type").as_str().unwrap_or("?"),
                tree.get("properties").get(name).to_json()
            );
        }
    }
    for line in tree.get("skipped").items().filter_map(Value::as_str) {
        println!("not shown: {line}");
    }
    Ok(())
}

/// The wallpaper folders a `check` argument stands for: itself, or the ones inside it.
fn wallpapers_in(name: &str) -> Vec<PathBuf> {
    if let Some(folder) = locate::wallpaper(name).filter(|folder| folder.join("project.json").is_file()) {
        return vec![folder];
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(name)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("project.json").is_file())
        .collect();
    found.sort();
    found
}

/// `wepaper check`.
pub fn check(args: lexopt::Parser) -> Result<(), Failure> {
    let usage = "wepaper check <wallpaper or folder of them>... [--json] [--assets DIR] [--device NODE]\n\n\
                 Loads every scene, draws a frame of it and reports what could not be loaded or shown.\n\
                 The exit status is 1 when a wallpaper did not load at all.\n";
    let Some(arguments) = arguments(args, usage)? else { return Ok(()) };
    if arguments.words.is_empty() {
        return Err("check what? (a wallpaper, or a folder of wallpapers)".to_owned());
    }
    let (context, mut engine) = headless(&arguments)?;
    let frame = Target::new(context.gl(), 1280, 720, TextureFormat::Rgba8).map_err(|e| e.to_string())?;
    let (mut clean, mut partial, mut failed, mut other) = (0, 0, 0, 0);
    let mut report = Vec::new();
    for folder in arguments.words.iter().flat_map(|word| wallpapers_in(word)) {
        let id = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        match engine.load(&folder, Viewport { width: 1280, height: 720 }, &[]) {
            Ok(mut wallpaper) => {
                wallpaper.draw(
                    &mut engine,
                    &FrameInput { time: 1.0, pointer: [0.5, 0.5], pointer_down: false, daytime: 0.5 },
                    Some(frame.framebuffer()),
                    (1280, 720),
                    true,
                );
                let skipped: Vec<String> = match &wallpaper {
                    Wallpaper::Scene(scene) => scene.skipped().to_vec(),
                    Wallpaper::Video(_) => Vec::new(),
                };
                if skipped.is_empty() {
                    clean += 1;
                } else {
                    partial += 1;
                    if !arguments.json {
                        println!("{id}: {} not shown", skipped.len());
                        for line in &skipped {
                            println!("    {line}");
                        }
                    }
                }
                report.push(obj! { "wallpaper" => id, "loaded" => true, "skipped" => Value::Array(skipped.into_iter().map(Value::from).collect()) });
            }
            Err(error) => {
                let message = error.to_string();
                // Kinds the engine leaves to plugins are not failures of the engine.
                if message.contains("are not drawn by the engine") {
                    other += 1;
                } else {
                    failed += 1;
                    if !arguments.json {
                        println!("{id}: DID NOT LOAD: {message}");
                    }
                }
                report.push(obj! { "wallpaper" => id, "loaded" => false, "error" => message });
            }
        }
        engine.trim();
    }
    if arguments.json {
        println!("{}", Value::Array(report).to_json_pretty());
    } else {
        println!(
            "{clean} complete, {partial} with parts not shown, {failed} did not load, {other} of a kind left to plugins"
        );
    }
    if failed > 0 { Err(format!("{failed} wallpapers did not load")) } else { Ok(()) }
}

/// `wepaper unpack`.
pub fn unpack(args: lexopt::Parser) -> Result<(), Failure> {
    let Some(arguments) = arguments(args, "wepaper unpack <scene.pkg or wallpaper> <folder>\n")? else {
        return Ok(());
    };
    let (Some(source), Some(target)) = (arguments.words.first(), arguments.words.get(1)) else {
        return Err("unpack what, and where to?".to_owned());
    };
    let package = if Path::new(source).is_file() {
        PathBuf::from(source)
    } else {
        locate::wallpaper(source).ok_or(format!("no such wallpaper: {source}"))?.join("scene.pkg")
    };
    let archive = Archive::open(&package).map_err(|e| format!("{}: {e}", package.display()))?;
    let target = Path::new(target);
    let names: Vec<String> = archive.names().map(str::to_owned).collect();
    for name in &names {
        // A name that climbs out of the folder is not written anywhere.
        let relative: PathBuf = name
            .split(['/', '\\'])
            .filter(|part| !part.is_empty() && *part != "." && *part != "..")
            .collect();
        let path = target.join(relative);
        let bytes = archive
            .read(name)
            .ok_or("the package changed while it was read")?
            .map_err(|e| format!("{name}: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!("{} files to {}", names.len(), target.display());
    Ok(())
}
