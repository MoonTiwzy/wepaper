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

/// Whether draws test against depth and write it. Only scenes with a perspective
/// camera ever turn either on; everything else draws with GL's default, both off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Depth {
    /// Draw only where nothing nearer was drawn.
    pub test: bool,
    /// Keep how near what is drawn is.
    pub write: bool,
}

#[derive(Default)]
struct State {
    program: Option<glow::Program>,
    textures: [Option<glow::Texture>; 16],
    unit: u32,
    blend: Option<Blend>,
    depth: Option<Depth>,
    cull: Option<bool>,
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
    /// Tells GL that rows of pixels follow each other without padding. This is set
    /// before every upload and readback and not once: libmpv, drawing in this
    /// context, puts the alignment back to 4, which shears every texture whose rows
    /// are not a multiple of four bytes (text, greyscale pictures).
    pub(super) fn tight_rows(&self) {
        // SAFETY: plain state on the current context.
        unsafe {
            self.raw().pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.raw().pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        }
    }

    pub(super) fn new(raw: glow::Context, compatibility: bool, loader: Loader) -> Self {
        // SAFETY: the context `raw` was loaded from is current (the caller made it so).
        let caps = unsafe {
            let extensions = raw.supported_extensions();
            let vendor = raw.get_parameter_string(glow::VENDOR);
            Caps {
                // NVIDIA's compiler, given no #version in a compatibility context, takes
                // HLSL's implicit conversions with a warning instead of an error.
                lenient_shaders: compatibility && vendor.contains("NVIDIA"),
                renderer: raw.get_parameter_string(glow::RENDERER),
                version: raw.get_parameter_string(glow::VERSION),
                s3tc: extensions.contains("GL_EXT_texture_compression_s3tc"),
                program_binary: extensions.contains("GL_ARB_get_program_binary")
                    && raw.get_parameter_i32(glow::NUM_PROGRAM_BINARY_FORMATS) > 0,
                max_texture_size: raw.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(0) as u32,
            }
        };
        Self(Rc::new(Shared {
            raw,
            loader,
            caps,
            memory: Cell::default(),
            state: RefCell::default(),
            cache_dir: RefCell::default(),
        }))
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

    /// Copies the picture of one framebuffer onto another (`None`: the current
    /// surface), scaled when their sizes differ.
    pub fn copy_framebuffer(
        &self,
        from: Option<glow::Framebuffer>,
        from_size: (u32, u32),
        to: Option<glow::Framebuffer>,
        to_size: (u32, u32),
    ) {
        let filter = if from_size == to_size { glow::NEAREST } else { glow::LINEAR };
        // SAFETY: both framebuffers are alive (or the surface's), and the rectangles are their sizes.
        unsafe {
            let gl = &self.0.raw;
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, from);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, to);
            gl.blit_framebuffer(
                0,
                0,
                from_size.0 as i32,
                from_size.1 as i32,
                0,
                0,
                to_size.0 as i32,
                to_size.1 as i32,
                glow::COLOR_BUFFER_BIT,
                filter,
            );
        }
        self.surface_changed();
    }

    /// Sends everything drawn so far to the GPU and sleeps until it is done.
    ///
    /// Presenting a frame the GPU is still working on makes some drivers wait for
    /// it by spinning a CPU core. Waiting here, asleep, costs nothing; what comes
    /// after finds the work done.
    ///
    /// Returns how long the GPU took, so that a caller whose frames are light can
    /// stop asking: for those the naps cost more than the spin they avoid.
    pub fn wait_for_gpu(&self, patience: std::time::Duration) -> std::time::Duration {
        let gl = &self.0.raw;
        let started = std::time::Instant::now();
        // SAFETY: a sync object is made, polled without blocking and deleted, all on the current context.
        unsafe {
            let Ok(fence) = gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) else {
                return std::time::Duration::ZERO;
            };
            gl.flush();
            let mut nap = std::time::Duration::from_micros(250);
            while !matches!(
                gl.client_wait_sync(fence, 0, 0),
                glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED
            ) && started.elapsed() < patience
            {
                std::thread::sleep(nap);
                nap = (nap * 2).min(std::time::Duration::from_millis(1));
            }
            gl.delete_sync(fence);
        }
        started.elapsed()
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
                    gl.blend_func_separate(
                        glow::SRC_ALPHA,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                    );
                }
                Blend::Additive => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(glow::SRC_ALPHA, glow::ONE, glow::ONE, glow::ONE);
                }
                Blend::Premultiplied => {
                    gl.enable(glow::BLEND);
                    gl.blend_func_separate(
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ONE,
                        glow::ONE_MINUS_SRC_ALPHA,
                    );
                }
            }
        }
    }

    /// Sets how draws use depth.
    pub fn set_depth(&self, depth: Depth) {
        let mut state = self.0.state.borrow_mut();
        // GL starts with both off: a context nobody asked for depth stays untouched.
        if state.depth.unwrap_or_default() == depth {
            return;
        }
        state.depth = Some(depth);
        let gl = &self.0.raw;
        // SAFETY: plain state changes on the current context.
        unsafe {
            if depth.test || depth.write {
                // Writing needs the test on; always passing it is "write only".
                gl.enable(glow::DEPTH_TEST);
                gl.depth_func(if depth.test { glow::LEQUAL } else { glow::ALWAYS });
            } else {
                gl.disable(glow::DEPTH_TEST);
            }
            gl.depth_mask(depth.write);
        }
    }

    /// Sets whether triangles facing away are skipped.
    pub fn set_cull(&self, cull: bool) {
        let mut state = self.0.state.borrow_mut();
        if state.cull.unwrap_or(false) == cull {
            return;
        }
        state.cull = Some(cull);
        // SAFETY: plain state changes on the current context.
        unsafe {
            if cull {
                self.0.raw.enable(glow::CULL_FACE);
                self.0.raw.cull_face(glow::BACK);
            } else {
                self.0.raw.disable(glow::CULL_FACE);
            }
        }
    }

    /// Fills whatever is bound with `color` and resets its depth to the farthest.
    pub fn clear_with_depth(&self, color: [f32; 4]) {
        // Depth is only cleared where it may be written.
        let depth = self.0.state.borrow().depth.unwrap_or_default();
        self.set_depth(Depth { test: depth.test, write: true });
        // SAFETY: plain state and a clear on the bound framebuffer.
        unsafe {
            self.0.raw.clear_color(color[0], color[1], color[2], color[3]);
            self.0.raw.clear_depth_f32(1.0);
            self.0.raw.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
        }
        self.set_depth(depth);
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
