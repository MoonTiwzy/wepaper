//! Render targets and the pool effect chains borrow them from.

use glow::HasContext;

use super::{Gl, GpuError, MemoryClass, Texture, TextureFormat};

/// A texture that can be drawn into.
pub struct Target {
    gl: Gl,
    framebuffer: glow::Framebuffer,
    /// What was drawn.
    pub texture: Texture,
    /// How near what was drawn is, for a target a 3D scene is drawn in.
    depth: Option<glow::Renderbuffer>,
    /// The frame it was last given back in; the pool trims by it.
    idle_since: u64,
}

impl Target {
    /// A target of the given size.
    pub fn new(gl: &Gl, width: u32, height: u32, format: TextureFormat) -> Result<Self, GpuError> {
        Self::make(gl, width, height, format, Kind::Plain)
    }

    /// A target of the given size with a depth buffer: what a 3D scene is drawn in.
    pub fn with_depth(gl: &Gl, width: u32, height: u32, format: TextureFormat) -> Result<Self, GpuError> {
        Self::make(gl, width, height, format, Kind::Depth)
    }

    fn make(gl: &Gl, width: u32, height: u32, format: TextureFormat, kind: Kind) -> Result<Self, GpuError> {
        let levels = if kind == Kind::Mipmapped { u32::MAX } else { 1 };
        let texture = Texture::empty_levels(gl, format, width, height, MemoryClass::Targets, levels)?;
        let depth = kind == Kind::Depth;
        // SAFETY: the context is current; the texture and the renderbuffer attached are
        // alive for as long as this target.
        let (framebuffer, depth) = unsafe {
            let framebuffer = gl.raw().create_framebuffer().map_err(GpuError)?;
            gl.bind_framebuffer(Some(framebuffer), (width, height));
            gl.raw().framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture.raw()),
                0,
            );
            let depth = if depth {
                let buffer = gl.raw().create_renderbuffer().map_err(GpuError)?;
                gl.raw().bind_renderbuffer(glow::RENDERBUFFER, Some(buffer));
                gl.raw().renderbuffer_storage(
                    glow::RENDERBUFFER,
                    glow::DEPTH_COMPONENT24,
                    width as i32,
                    height as i32,
                );
                gl.raw().framebuffer_renderbuffer(
                    glow::FRAMEBUFFER,
                    glow::DEPTH_ATTACHMENT,
                    glow::RENDERBUFFER,
                    Some(buffer),
                );
                Some(buffer)
            } else {
                None
            };
            if gl.raw().check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.raw().delete_framebuffer(framebuffer);
                if let Some(buffer) = depth {
                    gl.raw().delete_renderbuffer(buffer);
                }
                gl.surface_changed();
                return Err(GpuError(format!("a {width}x{height} render target is not usable on this GPU")));
            }
            (framebuffer, depth)
        };
        if depth.is_some() {
            gl.account(MemoryClass::Targets, Self::depth_bytes(width, height) as i64);
        }
        Ok(Self { gl: gl.clone(), framebuffer, texture, depth, idle_since: 0 })
    }

    /// What a depth buffer of this size takes: 24 bits of depth, stored in four bytes.
    fn depth_bytes(width: u32, height: u32) -> u64 {
        u64::from(width) * u64::from(height) * 4
    }

    /// Whether the target has a depth buffer.
    pub fn has_depth(&self) -> bool {
        self.depth.is_some()
    }

    /// How many levels its picture has: more than one for a mipmapped target.
    pub fn levels(&self) -> u32 {
        self.texture.levels
    }

    /// Makes the smaller levels of a mipmapped target from what was drawn into the largest.
    pub fn generate_mipmaps(&self) {
        self.gl.bind_texture(15, self.texture.raw());
        // SAFETY: the texture is alive and bound; its levels were allocated with it.
        unsafe { self.gl.raw().generate_mipmap(glow::TEXTURE_2D) };
    }

    fn kind(&self) -> Kind {
        match (self.depth.is_some(), self.levels() > 1) {
            (true, _) => Kind::Depth,
            (false, true) => Kind::Mipmapped,
            (false, false) => Kind::Plain,
        }
    }

    /// Directs drawing at this target.
    pub fn bind(&self) {
        self.gl.bind_framebuffer(Some(self.framebuffer), (self.texture.width, self.texture.height));
    }

    /// Fills the target with `color`.
    pub fn clear(&self, color: [f32; 4]) {
        self.bind();
        // SAFETY: plain state and a clear on the framebuffer bound above.
        unsafe {
            self.gl.raw().clear_color(color[0], color[1], color[2], color[3]);
            self.gl.raw().clear(glow::COLOR_BUFFER_BIT);
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.texture.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.texture.height
    }

    /// The GL framebuffer.
    pub fn framebuffer(&self) -> glow::Framebuffer {
        self.framebuffer
    }

    /// Copies this target's picture onto `to`, scaled to fit it.
    pub fn copy_to(&self, to: &Target) {
        self.blit(Some(to.framebuffer), (to.width(), to.height()));
    }

    /// Copies this target's picture onto `framebuffer` (`None`: the current surface) of `size`.
    pub fn blit(&self, framebuffer: Option<glow::Framebuffer>, size: (u32, u32)) {
        self.gl.copy_framebuffer(Some(self.framebuffer), (self.width(), self.height()), framebuffer, size);
    }

    /// Reads the picture back: RGBA, rows top to bottom as drawn with `y` up.
    pub fn read_rgba(&self) -> Vec<u8> {
        let (width, height) = (self.width() as usize, self.height() as usize);
        let mut pixels = vec![0u8; width * height * 4];
        self.bind();
        self.gl.tight_rows();
        // SAFETY: the buffer holds width * height RGBA pixels and rows were just declared tight.
        unsafe {
            self.gl.raw().read_pixels(
                0,
                0,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
        }
        // GL's first row is the bottom one.
        let row = width * 4;
        for y in 0..height / 2 {
            let (top, bottom) = pixels.split_at_mut((height - 1 - y) * row);
            top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
        }
        pixels
    }
}

impl Drop for Target {
    fn drop(&mut self) {
        self.gl.surface_changed();
        // SAFETY: the framebuffer and renderbuffer belong to the process's one context, current on this thread.
        unsafe {
            self.gl.raw().delete_framebuffer(self.framebuffer);
            if let Some(depth) = self.depth {
                self.gl.raw().delete_renderbuffer(depth);
                self.gl.account(
                    MemoryClass::Targets,
                    -(Self::depth_bytes(self.texture.width, self.texture.height) as i64),
                );
            }
        }
    }
}

/// What a target has besides its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    /// A depth buffer.
    Depth,
    /// Every smaller level of its picture.
    Mipmapped,
}

/// Targets handed out for the length of a layer's effect chain and taken back.
///
/// A layer needs its buffers only while its chain runs, so the layers of a frame can
/// take turns on the same few. What the pool holds is bounded by the deepest chain,
/// not by how many layers there are.
pub struct TargetPool {
    gl: Gl,
    free: Vec<Target>,
    frame: u64,
    lent: usize,
    /// The most targets that were out at once since the pool was made.
    pub peak_lent: usize,
}

/// A returned target nobody asked for in this many frames is freed.
const KEEP_FRAMES: u64 = 120;

impl TargetPool {
    /// An empty pool.
    pub fn new(gl: &Gl) -> Self {
        Self { gl: gl.clone(), free: Vec::new(), frame: 0, lent: 0, peak_lent: 0 }
    }

    /// A target of exactly this size and format, reused if one is free.
    pub fn take(&mut self, width: u32, height: u32, format: TextureFormat) -> Result<Target, GpuError> {
        self.lend(width, height, format, Kind::Plain)
    }

    /// The same with every smaller level of its picture.
    pub fn take_mipmapped(
        &mut self,
        width: u32,
        height: u32,
        format: TextureFormat,
    ) -> Result<Target, GpuError> {
        self.lend(width, height, format, Kind::Mipmapped)
    }

    /// The same with a depth buffer.
    pub fn take_with_depth(
        &mut self,
        width: u32,
        height: u32,
        format: TextureFormat,
    ) -> Result<Target, GpuError> {
        self.lend(width, height, format, Kind::Depth)
    }

    fn lend(
        &mut self,
        width: u32,
        height: u32,
        format: TextureFormat,
        kind: Kind,
    ) -> Result<Target, GpuError> {
        let fits = |t: &Target| {
            t.width() == width && t.height() == height && t.texture.format == format && t.kind() == kind
        };
        let target = match self.free.iter().position(fits) {
            Some(index) => self.free.swap_remove(index),
            None => Target::make(&self.gl, width, height, format, kind)?,
        };
        self.lent += 1;
        self.peak_lent = self.peak_lent.max(self.lent);
        Ok(target)
    }

    /// Takes a target back for others to use.
    pub fn give(&mut self, mut target: Target) {
        target.idle_since = self.frame;
        self.lent = self.lent.saturating_sub(1);
        self.free.push(target);
    }

    /// Marks the end of a frame and frees targets that sat unused for a while.
    pub fn end_frame(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.free.retain(|t| frame - t.idle_since < KEEP_FRAMES);
    }

    /// Frees every target that is not lent out.
    pub fn trim(&mut self) {
        self.free.clear();
    }

    /// How many targets sit in the pool unused.
    pub fn idle(&self) -> usize {
        self.free.len()
    }
}
