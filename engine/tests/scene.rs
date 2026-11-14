//! Whole scenes, drawn on a headless context from Wallpaper Engine's own stock pieces
//! (its solid layer, its effects, its shaders). They run when `WEPAPER_TEST_ASSETS` names
//! the `assets` folder and the machine gives a context, and are skipped otherwise.

use std::path::{Path, PathBuf};

use wepaper_engine::gpu::{Context, Target, TextureFormat};
use wepaper_engine::scene::FrameInput;
use wepaper_engine::{Engine, Options, Viewport};

const SIDE: u32 = 64;

/// Writes a scene of `objects` (the inside of the `objects` array) on a black canvas
/// of [`SIDE`] units a side into a folder of its own and returns the folder.
fn wallpaper(name: &str, objects: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wepaper-scene-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("project.json"), r#"{ "type": "scene", "file": "scene.json", "title": "test" }"#)
        .unwrap();
    let scene = format!(
        r#"{{
            "camera": {{ "center": "0 0 -1", "eye": "0 0 0", "up": "0 1 0" }},
            "general": {{
                "clearcolor": "0 0 0",
                "orthogonalprojection": {{ "width": {SIDE}, "height": {SIDE} }}
            }},
            "objects": [{objects}]
        }}"#
    );
    std::fs::write(dir.join("scene.json"), scene).unwrap();
    dir
}

/// Copies everything under `from` into `to`.
fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).into_iter().flatten().flatten() {
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_tree(&source, &target);
        } else {
            std::fs::create_dir_all(to).unwrap();
            std::fs::copy(&source, &target).unwrap();
        }
    }
}

/// An engine on a headless context, to draw wallpapers with.
struct Bench {
    // The engine holds what the context made: it goes first.
    engine: Engine,
    context: Context,
    assets: PathBuf,
}

impl Bench {
    /// `None`: nothing to draw with here.
    fn new() -> Option<Self> {
        let assets = PathBuf::from(std::env::var_os("WEPAPER_TEST_ASSETS")?);
        let context = match Context::headless(None) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("skipped: {error}");
                return None;
            }
        };
        let options = Options { assets: Some(assets.clone()), cache_dir: None, ..Options::default() };
        let engine = Engine::new(context.gl(), options).expect("the engine starts");
        Some(Self { engine, context, assets })
    }

    fn load(&mut self, folder: &Path) -> wepaper_engine::Wallpaper {
        // A stock effect is a folder of its own, with its material and its shaders: the
        // editor puts those into every wallpaper that uses the effect.
        for part in ["materials", "shaders"] {
            copy_tree(&self.assets.join("effects/blend").join(part), &folder.join(part));
        }
        let viewport = Viewport { width: SIDE, height: SIDE };
        self.engine.load(folder, viewport, &[]).expect("the scene loads")
    }

    /// Draws a few frames of `wallpaper` and returns the last one's pixels, RGBA.
    fn draw(&mut self, wallpaper: &mut wepaper_engine::Wallpaper) -> Vec<u8> {
        let target = Target::new(self.context.gl(), SIDE, SIDE, TextureFormat::Rgba8).unwrap();
        for index in 0..3 {
            let input = FrameInput {
                time: f64::from(index) / 60.0,
                pointer: [0.5, 0.5],
                pointer_down: false,
                daytime: 0.5,
            };
            wallpaper.draw(&mut self.engine, &input, Some(target.framebuffer()), (SIDE, SIDE), true);
        }
        target.read_rgba()
    }
}

/// Draws a few frames of the wallpaper in `folder` and returns the last one's pixels.
/// `None`: nothing to draw with here.
fn frame(folder: &Path) -> Option<Vec<u8>> {
    let mut bench = Bench::new()?;
    let mut wallpaper = bench.load(folder);
    let pixels = bench.draw(&mut wallpaper);
    drop(wallpaper);
    let _ = std::fs::remove_dir_all(folder);
    Some(pixels)
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 3] {
    let at = ((y * SIDE + x) * 4) as usize;
    [pixels[at], pixels[at + 1], pixels[at + 2]]
}

/// A solid layer covering the canvas.
fn solid(id: u32, color: &str, rest: &str) -> String {
    format!(
        r#"{{ "id": {id}, "name": "layer {id}", "image": "models/util/solidlayer.json", "color": "{color}",
              "origin": "32 32 0", "scale": "1 1 1", "angles": "0 0 0", "size": "{SIDE} {SIDE}" {rest} }}"#
    )
}

/// The stock blend effect, laying layer `from`'s picture over the layer it is on.
fn blend(from: u32) -> String {
    format!(
        r#", "effects": [{{ "file": "effects/blend/effect.json", "id": 9, "name": "blend", "visible": true,
              "passes": [{{ "id": 10, "combos": {{ "BLENDMODE": 0 }},
                            "textures": [null, "_rt_imageLayerComposite_{from}_a"] }}] }}]"#
    )
}

#[test]
fn a_layer_reads_the_picture_of_another() {
    let objects = [solid(1, "1 0 0", ""), solid(2, "0 0 1", &blend(1))].join(",");
    let Some(pixels) = frame(&wallpaper("read", &objects)) else { return };
    assert_eq!(pixel(&pixels, 32, 32), [255, 0, 0]);
}

#[test]
fn a_hidden_layer_another_layer_reads_still_has_its_picture() {
    // Scenes build a picture out of layers nobody is to see as they are: a globe out of
    // a map, its clouds and a mask, each hidden, read by the one layer that is shown.
    let objects = [solid(1, "1 0 0", r#", "visible": false"#), solid(2, "0 0 1", &blend(1))].join(",");
    let Some(pixels) = frame(&wallpaper("hidden-read", &objects)) else { return };
    assert_eq!(pixel(&pixels, 32, 32), [255, 0, 0], "the shown layer shows the hidden one's picture");
}

#[test]
fn a_hidden_layer_nobody_reads_is_not_drawn() {
    let objects = [solid(1, "1 0 0", r#", "visible": false"#), solid(2, "0 0 1", "")].join(",");
    let Some(pixels) = frame(&wallpaper("hidden-unread", &objects)) else { return };
    assert_eq!(pixel(&pixels, 32, 32), [0, 0, 255]);
}

#[test]
fn a_hidden_layer_is_not_drawn_for_a_reader_that_is_hidden_too() {
    // The frame is the canvas: neither the reader nor what it reads reaches it.
    let hidden = r#", "visible": false"#;
    let objects = [solid(1, "1 0 0", hidden), solid(2, "0 0 1", &format!("{hidden}{}", blend(1)))].join(",");
    let Some(pixels) = frame(&wallpaper("hidden-both", &objects)) else { return };
    assert_eq!(pixel(&pixels, 32, 32), [0, 0, 0]);
}

/// An effect of the wallpaper's own, `effects/paint`, that fills its layer with `color`.
fn paint(folder: &Path, color: &str) {
    let write = |path: &str, text: &str| {
        let file = folder.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    };
    write(
        "effects/paint/effect.json",
        r#"{ "name": "paint", "passes": [{ "material": "materials/effects/paint.json" }] }"#,
    );
    write(
        "materials/effects/paint.json",
        r#"{ "passes": [{ "shader": "effects/paint", "blending": "normal", "depthtest": "disabled",
                          "depthwrite": "disabled", "cullmode": "nocull" }] }"#,
    );
    write(
        "shaders/effects/paint.vert",
        "uniform mat4 g_ModelViewProjectionMatrix;\nattribute vec3 a_Position;\nattribute vec2 a_TexCoord;\n\
         varying vec2 v_TexCoord;\nvoid main() {\n\tgl_Position = mul(vec4(a_Position, 1.0), g_ModelViewProjectionMatrix);\n\
         \tv_TexCoord = a_TexCoord;\n}\n",
    );
    write(
        "shaders/effects/paint.frag",
        &format!(
            "uniform sampler2D g_Texture0; // {{\"hidden\":true}}\nvarying vec2 v_TexCoord;\n\
             void main() {{\n\tgl_FragColor = vec4({color}, 1.0);\n}}\n"
        ),
    );
}

#[test]
fn two_wallpapers_with_a_shader_of_the_same_name_each_get_their_own() {
    // Every wallpaper carries its own copy of the effects it uses, under the same
    // names, and an author may have changed his.
    let Some(mut bench) = Bench::new() else { return };
    let painted = r#", "effects": [{ "file": "effects/paint/effect.json", "id": 9, "name": "paint", "visible": true,
                                    "passes": [{ "id": 10 }] }]"#;
    let (red, green) = (
        wallpaper("paint-red", &solid(1, "0 0 1", painted)),
        wallpaper("paint-green", &solid(1, "0 0 1", painted)),
    );
    paint(&red, "1.0, 0.0, 0.0");
    paint(&green, "0.0, 1.0, 0.0");
    let mut first = bench.load(&red);
    let mut second = bench.load(&green);
    assert_eq!(pixel(&bench.draw(&mut first), 32, 32), [255, 0, 0]);
    assert_eq!(pixel(&bench.draw(&mut second), 32, 32), [0, 255, 0]);
    drop((first, second));
    for folder in [red, green] {
        let _ = std::fs::remove_dir_all(folder);
    }
}
