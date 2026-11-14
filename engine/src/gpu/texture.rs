//! Textures.

use glow::HasContext;

use super::{Gl, GpuError, MemoryClass};

/// How a texture's pixels are stored on the GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureFormat {
    /// Four bytes a pixel.
    Rgba8,
    /// Two bytes a pixel.
    Rg8,
    /// One byte a pixel.
    R8,
    /// BC1, compressed 8:1.
    Dxt1,
    /// BC2, compressed 4:1.
    Dxt3,
    /// BC3, compressed 4:1.
    Dxt5,
    /// Four half floats a pixel: for buffers that hold more than 0..1.
    Rgba16F,
}

impl TextureFormat {
    /// How many bytes a level of this size takes.
    pub fn level_size(self, width: u32, height: u32) -> u64 {
        let (w, h) = (u64::from(width), u64::from(height));
        let blocks = w.div_ceil(4).max(1) * h.div_ceil(4).max(1);
        match self {
            TextureFormat::Rgba8 => w * h * 4,
            TextureFormat::Rg8 => w * h * 2,
            TextureFormat::R8 => w * h,
            TextureFormat::Dxt1 => blocks * 8,
            TextureFormat::Dxt3 | TextureFormat::Dxt5 => blocks * 16,
            TextureFormat::Rgba16F => w * h * 8,
        }
    }

    /// (internal format, upload format, upload type); compressed formats have only the first.
    fn gl(self) -> (u32, u32, u32) {
        match self {
            TextureFormat::Rgba8 => (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE),
            TextureFormat::Rg8 => (glow::RG8, glow::RG, glow::UNSIGNED_BYTE),
            TextureFormat::R8 => (glow::R8, glow::RED, glow::UNSIGNED_BYTE),
            TextureFormat::Dxt1 => (0x83F1, 0, 0),
            TextureFormat::Dxt3 => (0x83F2, 0, 0),
            TextureFormat::Dxt5 => (0x83F3, 0, 0),
            TextureFormat::Rgba16F => (glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT),
        }
    }

    fn compressed(self) -> bool {
        matches!(self, TextureFormat::Dxt1 | TextureFormat::Dxt3 | TextureFormat::Dxt5)
    }
}

/// How a texture is sampled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sampling {
    /// Do not repeat past the edges.
    pub clamp: bool,
    /// Do not filter between pixels.
    pub nearest: bool,
}

/// A texture on the GPU. Deleted when dropped.
pub struct Texture {
    gl: Gl,
    raw: glow::Texture,
    /// Width of the largest level.
    pub width: u32,
    /// Height of the largest level.
    pub height: u32,
    /// Its storage format.
    pub format: TextureFormat,
    bytes: u64,
    class: MemoryClass,
    /// How many levels it has.
    pub levels: u32,
}

impl Texture {
    /// Uploads `levels` (width, height, pixels), largest first, as one texture.
    pub fn from_levels(
        gl: &Gl,
        format: TextureFormat,
        levels: &[(u32, u32, &[u8])],
        sampling: Sampling,
    ) -> Result<Self, GpuError> {
        let Some(&(width, height, _)) = levels.first() else {
            return Err(GpuError("a texture needs at least one level".to_owned()));
        };
        if format.compressed() && !gl.caps().s3tc {
            return Err(GpuError("this GPU cannot use DXT-compressed textures".to_owned()));
        }
        let limit = gl.caps().max_texture_size;
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(GpuError(format!(
                "a {width}x{height} texture is outside what this GPU takes (up to {limit})"
            )));
        }
        let (internal, upload, kind) = format.gl();
        let mut bytes = 0;
        gl.tight_rows();
        // SAFETY: the context is current; every slice handed to GL is checked to hold
        // what the call reads, and rows were just declared tight.
        let raw = unsafe {
            let raw = gl.raw().create_texture().map_err(GpuError)?;
            gl.bind_texture(15, raw);
            for (level, &(w, h, pixels)) in levels.iter().enumerate() {
                let needed = format.level_size(w, h);
                if (pixels.len() as u64) < needed {
                    gl.forget_texture(raw);
                    gl.raw().delete_texture(raw);
                    return Err(GpuError(format!(
                        "texture level {level} holds {} bytes, {needed} needed",
                        pixels.len()
                    )));
                }
                let pixels = &pixels[..needed as usize];
                if format.compressed() {
                    gl.raw().compressed_tex_image_2d(
                        glow::TEXTURE_2D,
                        level as i32,
                        internal as i32,
                        w as i32,
                        h as i32,
                        0,
                        pixels.len() as i32,
                        pixels,
                    );
                } else {
                    gl.raw().tex_image_2d(
                        glow::TEXTURE_2D,
                        level as i32,
                        internal as i32,
                        w as i32,
                        h as i32,
                        0,
                        upload,
                        kind,
                        glow::PixelUnpackData::Slice(Some(pixels)),
                    );
                }
                bytes += needed;
            }
            gl.raw().tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, levels.len() as i32 - 1);
            raw
        };
        let texture = Self {
            gl: gl.clone(),
            raw,
            width,
            height,
            format,
            bytes,
            class: MemoryClass::Textures,
            levels: levels.len() as u32,
        };
        texture.set_sampling(sampling, levels.len() > 1);
        gl.account(MemoryClass::Textures, bytes as i64);
        Ok(texture)
    }

    /// A texture with storage and no contents, counted under `class`.
    pub fn empty(
        gl: &Gl,
        format: TextureFormat,
        width: u32,
        height: u32,
        class: MemoryClass,
    ) -> Result<Self, GpuError> {
        Self::empty_levels(gl, format, width, height, class, 1)
    }

    /// The same with `levels` levels, each half the size of the one before (and at
    /// least a pixel): what a picture sampled blurred by level is drawn into.
    pub fn empty_levels(
        gl: &Gl,
        format: TextureFormat,
        width: u32,
        height: u32,
        class: MemoryClass,
        levels: u32,
    ) -> Result<Self, GpuError> {
        let (internal, upload, kind) = format.gl();
        let levels = levels.clamp(1, 32 - width.max(height).max(1).leading_zeros());
        if format.compressed() || width == 0 || height == 0 || width.max(height) > gl.caps().max_texture_size
        {
            return Err(GpuError(format!("cannot make an empty {width}x{height} {format:?} texture")));
        }
        let mut bytes = 0;
        // SAFETY: the context is current; a null pixel pointer allocates without reading.
        let raw = unsafe {
            let raw = gl.raw().create_texture().map_err(GpuError)?;
            gl.bind_texture(15, raw);
            for level in 0..levels {
                let (w, h) = ((width >> level).max(1), (height >> level).max(1));
                gl.raw().tex_image_2d(
                    glow::TEXTURE_2D,
                    level as i32,
                    internal as i32,
                    w as i32,
                    h as i32,
                    0,
                    upload,
                    kind,
                    glow::PixelUnpackData::Slice(None),
                );
                bytes += format.level_size(w, h);
            }
            gl.raw().tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, levels as i32 - 1);
            raw
        };
        let texture = Self { gl: gl.clone(), raw, width, height, format, bytes, class, levels };
        texture.set_sampling(Sampling { clamp: true, nearest: false }, levels > 1);
        gl.account(class, bytes as i64);
        Ok(texture)
    }

    /// Replaces the whole of level 0 with `pixels`.
    pub fn write(&self, pixels: &[u8]) -> Result<(), GpuError> {
        let (_, upload, kind) = self.format.gl();
        if self.format.compressed() || (pixels.len() as u64) < self.format.level_size(self.width, self.height)
        {
            return Err(GpuError("pixel data does not match the texture".to_owned()));
        }
        self.gl.bind_texture(15, self.raw);
        self.gl.tight_rows();
        // SAFETY: the slice holds a full level, checked above.
        unsafe {
            self.gl.raw().tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                self.width as i32,
                self.height as i32,
                upload,
                kind,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
        }
        Ok(())
    }

    /// Changes how the texture is sampled.
    pub fn set_sampling(&self, sampling: Sampling, mipmaps: bool) {
        let wrap = if sampling.clamp { glow::CLAMP_TO_EDGE } else { glow::REPEAT } as i32;
        let (min, mag) = match (sampling.nearest, mipmaps) {
            (true, _) => (glow::NEAREST, glow::NEAREST),
            (false, true) => (glow::LINEAR_MIPMAP_LINEAR, glow::LINEAR),
            (false, false) => (glow::LINEAR, glow::LINEAR),
        };
        self.gl.bind_texture(15, self.raw);
        // SAFETY: plain parameter changes on the texture bound above.
        unsafe {
            let gl = self.gl.raw();
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, mag as i32);
        }
    }

    /// The GL object.
    pub fn raw(&self) -> glow::Texture {
        self.raw
    }

    /// The video memory it takes.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        self.gl.forget_texture(self.raw);
        self.gl.account(self.class, -(self.bytes as i64));
        // SAFETY: the texture belongs to the process's one context, which is current on this thread.
        unsafe { self.gl.raw().delete_texture(self.raw) };
    }
}
