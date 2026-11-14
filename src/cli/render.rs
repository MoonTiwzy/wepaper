//! `wepaper render`: one frame of a wallpaper to a PNG, without a display.

use std::path::{Path, PathBuf};

use wepaper_engine::gpu::{Context, Target, TextureFormat};
use wepaper_engine::scene::FrameInput;
use wepaper_engine::{Engine, Fill, Options, Viewport};

use super::{Failure, cache_dir, parse_property, parse_size};

pub const USAGE: &str = "\
wepaper render <wallpaper> -o <file.png> [options]

Renders one frame of a scene or video wallpaper to a PNG. Needs no compositor.

  <wallpaper>          a wallpaper folder, or a workshop id
  -o, --output FILE    where to write the PNG
  -s, --size WxH       the frame's size (default 1920x1080)
  -t, --time SECONDS   the moment to show (default 0)
      --frames N       draw N frames up to that moment, so that what
                       builds up over time is there (default 1)
      --fill MODE      cover (default), contain or stretch
  -p, --property K=V   set a user property (repeatable)
      --assets DIR     Wallpaper Engine's assets folder (default: found in Steam)
      --device NODE    the GPU to render on, e.g. /dev/dri/renderD128
      --no-cache       do not read or write the program cache
      --stats          print load and frame times and memory to stderr
";

pub fn run(mut args: lexopt::Parser) -> Result<(), Failure> {
    use lexopt::prelude::*;
    let (mut wallpaper, mut output, mut assets, mut device) =
        (None::<String>, None::<PathBuf>, None::<PathBuf>, None::<String>);
    let (mut size, mut time, mut frames, mut fill, mut cache, mut stats) =
        ((1920, 1080), 0.0f64, 1u32, Fill::Cover, true, false);
    let mut properties = Vec::new();
    let text = |value: std::ffi::OsString| value.to_string_lossy().into_owned();
    while let Some(arg) = args.next().map_err(|e| e.to_string())? {
        match arg {
            Short('o') | Long("output") => output = Some(args.value().map_err(|e| e.to_string())?.into()),
            Short('s') | Long("size") => size = parse_size(&text(args.value().map_err(|e| e.to_string())?))?,
            Short('t') | Long("time") => {
                time = text(args.value().map_err(|e| e.to_string())?)
                    .parse()
                    .map_err(|_| "--time takes seconds")?
            }
            Long("frames") => {
                frames = text(args.value().map_err(|e| e.to_string())?)
                    .parse()
                    .map_err(|_| "--frames takes a number")?
            }
            Long("fill") => {
                let name = text(args.value().map_err(|e| e.to_string())?);
                fill = Fill::from_name(&name)
                    .ok_or_else(|| format!("unknown fill '{name}': cover, contain or stretch"))?;
            }
            Short('p') | Long("property") => {
                properties.push(parse_property(&text(args.value().map_err(|e| e.to_string())?))?)
            }
            Long("assets") => assets = Some(args.value().map_err(|e| e.to_string())?.into()),
            Long("device") => device = Some(text(args.value().map_err(|e| e.to_string())?)),
            Long("no-cache") => cache = false,
            Long("stats") => stats = true,
            Short('h') | Long("help") => {
                print!("{USAGE}");
                return Ok(());
            }
            Value(value) if wallpaper.is_none() => wallpaper = Some(text(value)),
            other => return Err(other.unexpected().to_string()),
        }
    }
    let wallpaper = wallpaper.ok_or("which wallpaper? (a folder or a workshop id)")?;
    let output = output.ok_or("where to? (-o file.png)")?;

    let started = std::time::Instant::now();
    let context = Context::headless(device.as_deref()).map_err(|e| e.to_string())?;
    let options = Options { assets, cache_dir: cache.then(cache_dir), ..Options::default() };
    let mut engine = Engine::new(context.gl(), options).map_err(|e| e.to_string())?;
    let viewport = Viewport { width: size.0, height: size.1 };
    let mut wallpaper =
        engine.load(Path::new(&wallpaper), viewport, &properties).map_err(|e| e.to_string())?;
    wallpaper.set_fill(fill);
    let frame = Target::new(context.gl(), size.0, size.1, TextureFormat::Rgba8).map_err(|e| e.to_string())?;
    frame.clear([0.0, 0.0, 0.0, 1.0]);
    // A video has nothing to show until its decoder delivers a frame.
    let patience = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !engine.videos().poll_all() && std::time::Instant::now() < patience {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let loaded = started.elapsed();

    // Frames before the last one are spaced a sixtieth of a second apart, ending at `time`.
    let frames = frames.max(1);
    let drawing = std::time::Instant::now();
    for index in 0..frames {
        let moment = (time - f64::from(frames - 1 - index) / 60.0).max(0.0);
        let input = FrameInput { time: moment, pointer: [0.5, 0.5], daytime: 0.5 };
        wallpaper.draw(&mut engine, &input, Some(frame.framebuffer()), size, true);
    }
    let pixels = frame.read_rgba();
    let drawn = drawing.elapsed();
    write_png(&output, size.0, size.1, &pixels)?;

    if stats {
        let memory = context.gl().memory();
        let mb = |bytes: u64| bytes as f64 / 1048576.0;
        eprintln!("gpu: {}", context.description);
        eprintln!(
            "load: {:.0} ms, {} frames: {:.1} ms",
            loaded.as_secs_f64() * 1e3,
            frames,
            drawn.as_secs_f64() * 1e3
        );
        eprintln!(
            "video memory: textures {:.1} MB, buffers {:.1} MB, video {:.1} MB; most effect buffers at once: {}",
            mb(memory.textures),
            mb(memory.targets),
            mb(memory.video),
            engine.peak_buffers()
        );
    }
    if let wepaper_engine::Wallpaper::Scene(scene) = &wallpaper {
        for line in scene.skipped() {
            log::info!("{line}");
        }
    }
    Ok(())
}

/// Writes RGBA pixels as a PNG, through a temporary file so a reader never sees half of one.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), Failure> {
    let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    let part = path.with_extension(format!("{}.part", std::process::id()));
    let file = std::fs::File::create(&part).map_err(|e| fail(&e))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let result =
        encoder.write_header().and_then(|mut writer| writer.write_image_data(rgba)).map_err(|e| fail(&e));
    match result {
        Ok(()) => std::fs::rename(&part, path).map_err(|e| fail(&e)),
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}
