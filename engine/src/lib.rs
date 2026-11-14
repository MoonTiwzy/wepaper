//! The wepaper engine: a wallpaper folder in, frames out.
//!
//! The crate is laid out along the path a wallpaper takes:
//!
//! - [`read`]: bytes on disk become data (archives, layered files, textures).
//! - [`doc`]: JSON documents become typed values (project, scene, material, effect).
//! - [`glsl`]: the shader dialect becomes GLSL 330.
//! - [`gpu`]: OpenGL: the context, textures, programs, pooled render targets.
//! - [`share`]: the cache every playing wallpaper draws its resources from.
//! - [`scene`]: a scene becomes layers and passes, and a frame is drawn.
//! - [`video`]: libmpv decodes video straight into our textures.
//! - [`audio`]: sound layers, and the spectrum of what the system plays.
//! - [`particles`]: particle systems, simulated on the CPU.
//! - [`post`]: output shaders run over the finished frame.
//! - `script`: SceneScript, behind the `script` feature.
//!
//! Nothing here knows about Wayland or the control protocol; the `wepaper` binary
//! brings those.
//!
//! ```no_run
//! use wepaper_engine::gpu::{Context, Target, TextureFormat};
//! use wepaper_engine::{Engine, Options, Viewport, scene::FrameInput};
//!
//! let context = Context::headless(None)?;
//! let mut engine = Engine::new(context.gl(), Options::default())?;
//! let mut wallpaper = engine.load("3566329932".as_ref(), Viewport { width: 1920, height: 1080 }, &[])?;
//! let frame = Target::new(context.gl(), 1920, 1080, TextureFormat::Rgba8)?;
//! wallpaper.draw(&mut engine, &FrameInput { time: 2.0, ..Default::default() }, Some(frame.framebuffer()), (1920, 1080), true);
//! let rgba = frame.read_rgba();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod audio;
pub mod doc;
pub mod glsl;
pub mod gpu;
pub mod locate;
pub mod math;
pub mod particles;
pub mod post;
pub mod read;
pub mod scene;
#[cfg(feature = "script")]
pub mod script;
pub mod share;
pub mod video;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use wepaper_json::Value;

use doc::{Kind, Project};
use gpu::{Gl, GpuError, Mesh, Program, TargetPool, Texture};
use read::{Archive, Files};
use scene::{FrameInput, PropertyValue, SceneInstance};
use share::Share;
use video::VideoInstance;

/// Why a wallpaper did not load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(pub String);

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

impl From<GpuError> for LoadError {
    fn from(error: GpuError) -> Self {
        Self(error.to_string())
    }
}

/// The size, in pixels, a wallpaper is drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// How a wallpaper fills an output of another shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fill {
    /// Fill the output, cropping what does not fit.
    #[default]
    Cover,
    /// Show all of it, with bars.
    Contain,
    /// Fill the output, distorting.
    Stretch,
}

impl Fill {
    /// The fill a name stands for: `cover`, `contain`, `stretch`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "cover" | "crop" | "fill" => Some(Fill::Cover),
            "contain" | "fit" => Some(Fill::Contain),
            "stretch" => Some(Fill::Stretch),
            _ => None,
        }
    }

    /// The name used in the config and the protocol.
    pub fn name(self) -> &'static str {
        match self {
            Fill::Cover => "cover",
            Fill::Contain => "contain",
            Fill::Stretch => "stretch",
        }
    }
}

/// A playing wallpaper of any kind the engine draws itself.
pub enum Wallpaper {
    /// A scene.
    Scene(Box<SceneInstance>),
    /// A video.
    Video(Box<VideoInstance>),
}

impl Wallpaper {
    /// The wallpaper's `project.json`.
    pub fn project(&self) -> &Project {
        match self {
            Wallpaper::Scene(scene) => scene.project(),
            Wallpaper::Video(video) => video.project(),
        }
    }

    /// Draws the frame for `input` into `to` (`None`: the current surface) of `size`.
    ///
    /// Returns whether anything was drawn: a video draws only when it has a new
    /// frame, unless `force` says the destination's contents are gone.
    pub fn draw(
        &mut self,
        engine: &mut Engine,
        input: &FrameInput,
        to: Option<glow::Framebuffer>,
        size: (u32, u32),
        force: bool,
    ) -> bool {
        match self {
            Wallpaper::Scene(scene) => {
                scene.render(engine, input, to, size);
                true
            }
            Wallpaper::Video(video) => video.draw(engine, to, size, force),
        }
    }

    /// Whether the wallpaper needs a frame every display refresh (a scene), or only
    /// when its source has a new one (a video).
    pub fn animates(&self) -> bool {
        matches!(self, Wallpaper::Scene(_))
    }

    /// Changes how the wallpaper fills an output of another shape.
    pub fn set_fill(&mut self, fill: Fill) {
        match self {
            Wallpaper::Scene(scene) => scene.set_fill(fill),
            Wallpaper::Video(video) => video.set_fill(fill),
        }
    }

    /// Sets how fast what plays on its own clock plays: a video. A scene plays at the
    /// speed of the time it is drawn for.
    pub fn set_rate(&mut self, rate: f32) {
        if let Wallpaper::Video(video) = self {
            video.set_rate(rate);
        }
    }

    /// Pauses or resumes what plays on its own clock (videos, sound).
    pub fn set_paused(&mut self, paused: bool) {
        match self {
            Wallpaper::Scene(scene) => scene.set_paused(paused),
            Wallpaper::Video(video) => video.set_paused(paused),
        }
    }

    /// Whether the wallpaper reads the audio spectrum.
    pub fn wants_audio(&self) -> bool {
        matches!(self, Wallpaper::Scene(scene) if scene.wants_audio())
    }

    /// Why the wallpaper stopped showing anything, when it did: a video that cannot be
    /// played. `None` while it plays.
    pub fn failure(&self) -> Option<&'static str> {
        match self {
            Wallpaper::Video(video) if video.failed() => Some("the video cannot be played"),
            _ => None,
        }
    }

    /// Whether the wallpaper has scripts that show what the system's media player
    /// is on, so that somebody has to follow it.
    pub fn wants_media(&self) -> bool {
        matches!(self, Wallpaper::Scene(scene) if scene.wants_media())
    }

    /// Switches the wallpaper's own sound on or off and sets its volume (0 to 100).
    /// Whether a video's track is decoded at all follows the engine's video settings;
    /// how loud this video is, is set here.
    pub fn set_sound(&mut self, on: bool, volume: f32) {
        match self {
            Wallpaper::Scene(scene) => scene.set_sound(on, volume),
            Wallpaper::Video(video) => video.set_volume(if on { volume } else { 0.0 }),
        }
    }

    /// The current values of the user properties.
    pub fn properties(&self) -> Vec<PropertyValue> {
        match self {
            Wallpaper::Scene(scene) => scene.properties(),
            Wallpaper::Video(_) => Vec::new(),
        }
    }

    /// Sets a user property.
    pub fn set_property(&mut self, name: &str, value: &Value) {
        if let Wallpaper::Scene(scene) = self {
            scene.set_property(name, value);
        }
    }

    /// A snapshot of everything about the playing wallpaper that can be read by path.
    pub fn tree(&self) -> Value {
        match self {
            Wallpaper::Scene(scene) => scene.tree(),
            Wallpaper::Video(_) => wepaper_json::obj! {},
        }
    }

    /// Changes the value at `path`, relative to the wallpaper.
    pub fn edit(&mut self, path: &[&str], value: &Value) -> Result<(), String> {
        match self {
            Wallpaper::Scene(scene) => scene.edit(path, value),
            Wallpaper::Video(_) => Err("a video wallpaper has nothing to set".to_owned()),
        }
    }
}

/// What an engine is set up with.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Wallpaper Engine's `assets` folder; found in Steam when not given.
    pub assets: Option<PathBuf>,
    /// Where compiled programs and prepared videos are kept between runs.
    pub cache_dir: Option<PathBuf>,
    /// Textures larger than this on a side are loaded from a smaller level; 0 for no limit.
    pub max_texture_side: u32,
}

/// The engine: the GPU handle with everything shared between playing wallpapers.
pub struct Engine {
    pub(crate) gl: Gl,
    pub(crate) share: Share,
    pub(crate) pool: TargetPool,
    pub(crate) fullscreen: Mesh,
    pub(crate) white: Rc<share::SharedTexture>,
    pub(crate) blit: Program,
    pub(crate) listener: audio::Listener,
    pub(crate) media: audio::Media,
    assets: Option<PathBuf>,
}

const BLIT_VERTEX: &str = "#version 330 core\nin vec3 a_Position;\nin vec2 a_TexCoord;\nout vec2 v_TexCoord;\nuniform mat4 u_Transform;\n\
                           void main() { v_TexCoord = a_TexCoord; gl_Position = u_Transform * vec4(a_Position, 1.0); }\n";
const BLIT_FRAGMENT: &str = "#version 330 core\nin vec2 v_TexCoord;\nout vec4 color;\nuniform sampler2D u_Texture;\nuniform float u_Alpha;\n\
                             void main() { color = texture(u_Texture, v_TexCoord); color.a *= u_Alpha; }\n";

impl Engine {
    /// An engine on the context `gl` belongs to.
    pub fn new(gl: &Gl, options: Options) -> Result<Self, LoadError> {
        gl.set_cache_dir(options.cache_dir.as_ref().map(|dir| dir.join("programs")));
        let mut share = Share::new(gl);
        if options.max_texture_side > 0 {
            share.max_texture_side = options.max_texture_side;
        }
        if let Some(dir) = &options.cache_dir {
            share.video_dir = dir.join("video");
            share.videos.cache = Some(dir.join("video"));
        }
        let white = share.white();
        Ok(Self {
            gl: gl.clone(),
            pool: TargetPool::new(gl),
            fullscreen: Mesh::fullscreen(gl)?,
            blit: Program::link(gl, BLIT_VERTEX, BLIT_FRAGMENT).map_err(|e| LoadError(e.to_string()))?,
            white,
            share,
            listener: audio::Listener::new(),
            media: audio::Media::default(),
            assets: options.assets.or_else(locate::assets),
        })
    }

    /// The GPU handle.
    pub fn gl(&self) -> &Gl {
        &self.gl
    }

    /// The white 1x1 texture as a plain texture.
    pub(crate) fn white(&self) -> &Texture {
        &self.white.texture
    }

    /// Opens a wallpaper folder: its project and its files stacked on the stock assets.
    pub fn open(&self, wallpaper: &Path) -> Result<(Files, Project), LoadError> {
        let dir = locate::wallpaper(&wallpaper.to_string_lossy())
            .ok_or_else(|| LoadError(format!("no such wallpaper: {}", wallpaper.display())))?;
        let text = std::fs::read(dir.join("project.json"))
            .map_err(|e| LoadError(format!("{}: {e}", dir.join("project.json").display())))?;
        let document = wepaper_json::parse(&String::from_utf8_lossy(&text))
            .map_err(|e| LoadError(format!("project.json: {e}")))?;
        let project = Project::parse(&document);
        let mut files = Files::new();
        match &self.assets {
            Some(assets) => files.mount_dir(assets.clone()),
            None if project.kind == Kind::Scene => {
                return Err(LoadError("Wallpaper Engine's assets were not found: install it in Steam, or name the folder with --assets".to_owned()));
            }
            None => {}
        }
        files.mount_dir(dir.clone());
        let package = dir.join("scene.pkg");
        if package.is_file() {
            files.mount_archive(
                Archive::open(&package).map_err(|e| LoadError(format!("{}: {e}", package.display())))?,
            );
        }
        Ok((files, project))
    }

    /// Loads a scene wallpaper for an output of `viewport`, with the user's property `values`.
    pub fn load_scene(
        &mut self,
        wallpaper: &Path,
        viewport: Viewport,
        values: &[(String, Value)],
    ) -> Result<SceneInstance, LoadError> {
        let (files, project) = self.open(wallpaper)?;
        if project.kind != Kind::Scene {
            return Err(LoadError(format!(
                "{} is a {} wallpaper, not a scene",
                wallpaper.display(),
                project.kind.name()
            )));
        }
        let scene = SceneInstance::build(self, &files, project, viewport, values)?;
        self.share.sweep();
        Ok(scene)
    }

    /// Loads a wallpaper of any kind the engine draws: a scene or a video. A preset
    /// (property values for another wallpaper) plays that wallpaper with its values
    /// under the user's.
    pub fn load(
        &mut self,
        wallpaper: &Path,
        viewport: Viewport,
        values: &[(String, Value)],
    ) -> Result<Wallpaper, LoadError> {
        let (files, project) = self.open(wallpaper)?;
        match project.kind {
            Kind::Preset => {
                let dependency = project.dependency.as_deref().ok_or_else(|| {
                    LoadError("the preset does not say which wallpaper it is for".to_owned())
                })?;
                let target = locate::wallpaper(dependency).ok_or_else(|| {
                    LoadError(format!("the preset is for wallpaper {dependency}, which is not installed"))
                })?;
                if self.open(&target)?.1.kind == Kind::Preset {
                    return Err(LoadError("a preset of a preset".to_owned()));
                }
                // A file a preset names is the preset's own, beside it.
                let own = |value: &Value| {
                    value
                        .as_str()
                        .map(|file| wallpaper.join(file))
                        .filter(|file| file.is_file())
                        .map(|file| Value::from(file.to_string_lossy().into_owned()))
                };
                let mut merged: Vec<(String, Value)> = project
                    .preset
                    .members()
                    .map(|(name, value)| (name.to_owned(), own(value).unwrap_or_else(|| value.clone())))
                    .collect();
                merged.extend(values.iter().cloned());
                self.load(&target, viewport, &merged)
            }
            Kind::Scene => {
                let scene = SceneInstance::build(self, &files, project, viewport, values)?;
                self.share.sweep();
                Ok(Wallpaper::Scene(Box::new(scene)))
            }
            Kind::Video => {
                let dir = locate::wallpaper(&wallpaper.to_string_lossy())
                    .ok_or_else(|| LoadError(format!("no such wallpaper: {}", wallpaper.display())))?;
                let stream = self
                    .share
                    .videos
                    .open(&dir.join(&project.file), true)
                    .map_err(|e| LoadError(e.to_string()))?;
                Ok(Wallpaper::Video(Box::new(VideoInstance::new(project, stream))))
            }
            other => Err(LoadError(format!("{} wallpapers are not drawn by the engine", other.name()))),
        }
    }

    /// Brings the audio spectrum up to date, once per round of drawing. `wanted` is
    /// whether anything shown reads it; while nothing does, nothing listens.
    pub fn listen(&mut self, wanted: bool, dt: f32) {
        self.listener.update(wanted, dt);
    }

    /// Brings what the system's media players are on up to date, once per round of
    /// drawing. `wanted` is whether anything shown has scripts that hear of it; while
    /// nothing does, the players are not followed.
    pub fn follow_media(&mut self, wanted: bool) {
        self.media.update(wanted);
    }

    /// Whether the system's sound is being recorded for the spectrum right now.
    pub fn listening(&self) -> bool {
        self.listener.listening()
    }

    /// The videos being played, and how new ones are.
    pub fn videos(&mut self) -> &mut video::VideoHub {
        &mut self.share.videos
    }

    /// Frees what is held only for reuse: idle buffers and dead cache entries.
    pub fn trim(&mut self) {
        self.pool.trim();
        self.share.sweep();
    }

    /// The most effect buffers that were in use at once.
    pub fn peak_buffers(&self) -> usize {
        self.pool.peak_lent
    }
}
