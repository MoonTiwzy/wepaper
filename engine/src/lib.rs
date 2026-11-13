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
//! - [`particles`]: particle systems, simulated on the CPU.
//! - `script`: SceneScript, behind the `script` feature.
//!
//! Nothing here knows about Wayland or the control protocol; the `wepaper` binary
//! brings those.
//!
//! ```no_run
//! use wepaper_engine::{Engine, Options, Viewport, gpu::Context, scene::FrameInput};
//!
//! let context = Context::headless(None)?;
//! let mut engine = Engine::new(context.gl(), Options::default())?;
//! let mut scene = engine.load_scene("3566329932".as_ref(), Viewport { width: 1920, height: 1080 }, &[])?;
//! let frame = scene.render(&mut engine, &FrameInput { time: 2.0, ..Default::default() });
//! let rgba = frame.read_rgba();
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod doc;
pub mod glsl;
pub mod gpu;
pub mod locate;
pub mod math;
pub mod particles;
pub mod read;
pub mod scene;
#[cfg(feature = "script")]
pub mod script;
pub mod share;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use wepaper_json::Value;

use doc::{Kind, Project};
use gpu::{Gl, GpuError, Mesh, Program, TargetPool, Texture};
use read::{Archive, Files};
use scene::SceneInstance;
use share::Share;

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
        let white = share.white();
        Ok(Self {
            gl: gl.clone(),
            pool: TargetPool::new(gl),
            fullscreen: Mesh::fullscreen(gl)?,
            blit: Program::link(gl, BLIT_VERTEX, BLIT_FRAGMENT).map_err(|e| LoadError(e.to_string()))?,
            white,
            share,
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
        let dir = locate::wallpaper(&wallpaper.to_string_lossy()).ok_or_else(|| LoadError(format!("no such wallpaper: {}", wallpaper.display())))?;
        let text = std::fs::read(dir.join("project.json")).map_err(|e| LoadError(format!("{}: {e}", dir.join("project.json").display())))?;
        let document = wepaper_json::parse(&String::from_utf8_lossy(&text)).map_err(|e| LoadError(format!("project.json: {e}")))?;
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
            files.mount_archive(Archive::open(&package).map_err(|e| LoadError(format!("{}: {e}", package.display())))?);
        }
        Ok((files, project))
    }

    /// Loads a scene wallpaper for an output of `viewport`, with the user's property `values`.
    pub fn load_scene(&mut self, wallpaper: &Path, viewport: Viewport, values: &[(String, Value)]) -> Result<SceneInstance, LoadError> {
        let (files, project) = self.open(wallpaper)?;
        if project.kind != Kind::Scene {
            return Err(LoadError(format!("{} is a {} wallpaper, not a scene", wallpaper.display(), project.kind.name())));
        }
        let scene = SceneInstance::build(self, &files, project, viewport, values)?;
        self.share.sweep();
        Ok(scene)
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
