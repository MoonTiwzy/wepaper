//! The handle everything draws through: capabilities, accounting, cached state.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use glow::HasContext;

/// Something the GPU or its driver refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuError(pub String);

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GpuError {}

/// What the context can do.
#[derive(Debug, Clone, Default)]
pub struct Caps {
    /// The GPU's name as the driver gives it.
    pub renderer: String,
    /// The OpenGL version string.
    pub version: String,
    /// DXT textures can be used compressed.
    pub s3tc: bool,
    /// Linked programs can be saved and restored.
    pub program_binary: bool,
    /// The largest texture side.
    pub max_texture_size: u32,
    /// Shaders without a `#version` compile with HLSL's implicit conversions allowed.
    pub lenient_shaders: bool,
}

/// What video memory is counted under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryClass {
    /// Pictures loaded from wallpapers.
    Textures,
    /// Buffers the engine draws into.
    Targets,
    /// Video frames.
    Video,
}

/// Bytes of video memory the engine has allocated, by what for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Memory {
    /// Pictures loaded from wallpapers.
    pub textures: u64,
    /// Buffers the engine draws into.
    pub targets: u64,
    /// Video frames.
    pub video: u64,
}

/// How a draw combines with what is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Blend {
    /// Replace.
    #[default]
    None,
    /// Straight alpha.
    Alpha,
    /// Add, scaled by alpha.
    Additive,
    /// Alpha of premultiplied colour.
    Premultiplied,
}

#[derive(Default)]
struct State {
    program: Option<glow::Program>,
    textures: [Option<glow::Texture>; 16],
    unit: u32,
    blend: Option<Blend>,
    framebuffer: Option<Option<glow::Framebuffer>>,
    viewport: (i32, i32),
}

/// Looks a GL function up by name; what foreign renderers (libmpv) are handed.
pub(crate) type Loader = Box<dyn Fn(&std::ffi::CStr) -> *const std::ffi::c_void>;

struct Shared {
    raw: glow::Context,
    loader: Loader,
    caps: Caps,
    memory: Cell<Memory>,
    state: RefCell<State>,
    cache_dir: RefCell<Option<PathBuf>>,
}

/// A cheap, cloneable handle to the context's functions.
///
/// It caches the bindings it made, so asking for a state that is already set costs
/// nothing; everything in the engine goes through it for that reason.
#[derive(Clone)]
pub struct Gl(Rc<Shared>);

impl Gl {
    pub(super) fn new(raw: glow::Context, compatibility: bool, loader: Loader) -> Self {
        // SAFETY: the context `raw` was loaded from is current (the caller made it so).
        let caps = unsafe {
            raw.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            raw.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            let extensions = raw.supported_extensions();
            let vendor = raw.get_parameter_string(glow::VENDOR);
            Caps {
                // NVIDIA's compiler, given no #version in a compatibility context, takes
                // HLSL's implicit conversions with a warning instead of an error.
                lenient_shaders: compatibility && vendor.contains("NVIDIA"),
                renderer: raw.get_parameter_string(glow::RENDERER),
                version: raw.get_parameter_string(glow::VERSION),
                s3tc: extensions.contains("GL_EXT_texture_compression_s3tc"),
                program_binary: extensions.contains("GL_ARB_get_program_binary") && raw.get_parameter_i32(glow::NUM_PROGRAM_BINARY_FORMATS) > 0,
                max_texture_size: raw.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(0) as u32,
            }
        };
        Self(Rc::new(Shared { raw, loader, caps, memory: Cell::default(), state: RefCell::default(), cache_dir: RefCell::default() }))
    }

    /// The raw functions, for what the cache does not cover.
    pub fn raw(&self) -> &glow::Context {
        &self.0.raw
    }

    /// The address of the GL function called `name`, null when there is none.
    pub(crate) fn proc_address(&self, name: &std::ffi::CStr) -> *const std::ffi::c_void {
        (self.0.loader)(name)
    }

    /// What the context can do.
    pub fn caps(&self) -> &Caps {
        &self.0.caps
    }

    /// Video memory allocated through this handle.
    pub fn memory(&self) -> Memory {
        self.0.memory.get()
    }

    /// Where linked programs are kept between runs; `None` keeps none.
    pub fn set_cache_dir(&self, dir: Option<PathBuf>) {
        *self.0.cache_dir.borrow_mut() = dir;
    }

    pub(super) fn cache_dir(&self) -> Option<PathBuf> {
        self.0.cache_dir.borrow().clone()
    }

    pub(crate) fn account(&self, class: MemoryClass, bytes: i64) {
        let mut memory = self.0.memory.get();
        let slot = match class {
            MemoryClass::Textures => &mut memory.textures,
            MemoryClass::Targets => &mut memory.targets,
            MemoryClass::Video => &mut memory.video,
        };
        *slot = slot.saturating_add_signed(bytes);
        self.0.memory.set(memory);
    }

    /// Moves `bytes` from one class to another: the same memory, counted for what it is used for.
    pub(crate) fn reclassify(&self, from: MemoryClass, to: MemoryClass, bytes: u64) {
        self.account(from, -(bytes as i64));
        self.account(to, bytes as i64);
    }

    /// Forgets the cached framebuffer and viewport: another surface is current now,
    /// or foreign code (libmpv) drew with the context.
    pub fn surface_changed(&self) {
        let mut state = self.0.state.borrow_mut();
        state.framebuffer = None;
        state.viewport = (0, 0);
    }

    /// Points drawing to the default framebuffer at the surface's back buffer.
    pub(super) fn draw_to_back_buffer(&self) {
        // SAFETY: plain state changes on the current context, with a surface bound.
        unsafe {
            self.0.raw.bind_framebuffer(glow::FRAMEBUFFER, None);
            self.0.raw.draw_buffer(glow::BACK);
            self.0.raw.read_buffer(glow::BACK);
        }
    }

    /// Forgets everything cached: foreign code used the context.
    pub fn invalidate(&self) {
        *self.0.state.borrow_mut() = State::default();
    }

    /// Makes `program` the one that draws.
    pub fn use_program(&self, program: glow::Program) {
        let mut state = self.0.state.borrow_mut();
        if state.program != Some(program) {
            state.program = Some(program);
            // SAFETY: `program` is a live program of this context.
            unsafe { self.0.raw.use_program(Some(program)) };
        }
    }

    /// Binds `texture` to texture unit `unit`.
    pub fn bind_texture(&self, unit: u32, texture: glow::Texture) {
        let mut state = self.0.state.borrow_mut();
        let slot = unit as usize;
        if slot < state.textures.len() && state.textures[slot] == Some(texture) {
            return;
        }
        // SAFETY: `texture` is a live texture of this context; `unit` is within the 16 units GL 3.3 guarantees.
        unsafe {
            if state.unit != unit {
                state.unit = unit;
                self.0.raw.active_texture(glow::TEXTURE0 + unit);
            }
            self.0.raw.bind_texture(glow::TEXTURE_2D, Some(texture));
        }
        if slot < state.textures.len() {
            state.textures[slot] = Some(texture);
        }
    }

    /// Forgets that `texture` is bound anywhere (it is about to be deleted or respecified).
    pub(super) fn forget_texture(&self, texture: glow::Texture) {
        for slot in &mut self.0.state.borrow_mut().textures {
            if *slot == Some(texture) {
                *slot = None;
            }
        }
    }

    /// Sets how draws blend.
    pub fn set_blend(&self, blend: Blend) {
        let mut state = self.0.state.borrow_mut();
        if state.blend == Some(blend) {
            return;
        }
        state.blend = Some(blend);
        let gl = &self.0.raw;
        // SAFETY: plain state changes on the current context.
        unsafe {
            match blend {
                Blend::None => gl.disable(glow::BLEND),
                Blend::Alpha => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                }
                Blend::Additive => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE, glow::ONE, glow::ONE);
                }
                Blend::Premultiplied => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                }
            }
        }
    }

    /// Directs drawing at `framebuffer` (`None`: the current surface) with a viewport of `size`.
    pub fn bind_framebuffer(&self, framebuffer: Option<glow::Framebuffer>, size: (u32, u32)) {
        let mut state = self.0.state.borrow_mut();
        let size = (size.0 as i32, size.1 as i32);
        // SAFETY: `framebuffer` is a live framebuffer of this context or the default one.
        unsafe {
            if state.framebuffer != Some(framebuffer) {
                state.framebuffer = Some(framebuffer);
                self.0.raw.bind_framebuffer(glow::FRAMEBUFFER, framebuffer);
            }
            if state.viewport != size {
                state.viewport = size;
                self.0.raw.viewport(0, 0, size.0, size.1);
            }
        }
    }
}
