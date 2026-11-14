//! Render targets and the pool effect chains borrow them from.

use glow::HasContext;

use super::{Gl, GpuError, MemoryClass, Texture, TextureFormat};

/// A texture that can be drawn into.
pub struct Target {
    gl: Gl,
    framebuffer: glow::Framebuffer,
    /// What was drawn.
    pub texture: Texture,
    /// The frame it was last given back in; the pool trims by it.
    idle_since: u64,
}

impl Target {
    /// A target of the given size.
    pub fn new(gl: &Gl, width: u32, height: u32, format: TextureFormat) -> Result<Self, GpuError> {
        let texture = Texture::empty(gl, format, width, height, MemoryClass::Targets)?;
        // SAFETY: the context is current; the texture attached is alive for as long as this target.
        let framebuffer = unsafe {
            let framebuffer = gl.raw().create_framebuffer().map_err(GpuError)?;
            gl.bind_framebuffer(Some(framebuffer), (width, height));
            gl.raw().framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture.raw()),
                0,
            );
            if gl.raw().check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                gl.raw().delete_framebuffer(framebuffer);
                gl.surface_changed();
                return Err(GpuError(format!("a {width}x{height} render target is not usable on this GPU")));
            }
            framebuffer
        };
        Ok(Self { gl: gl.clone(), framebuffer, texture, idle_since: 0 })
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
        // SAFETY: the framebuffer belongs to the process's one context, current on this thread.
        unsafe { self.gl.raw().delete_framebuffer(self.framebuffer) };
    }
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
        let fits = |t: &Target| t.width() == width && t.height() == height && t.texture.format == format;
        let target = match self.free.iter().position(fits) {
            Some(index) => self.free.swap_remove(index),
            None => Target::new(&self.gl, width, height, format)?,
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
