//! `.tex` texture containers.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use super::lz4;

/// How the pixels of a texture are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Four bytes a pixel.
    Rgba8,
    /// BC1: 8 bytes per 4x4 block.
    Dxt1,
    /// BC2: 16 bytes per 4x4 block.
    Dxt3,
    /// BC3: 16 bytes per 4x4 block.
    Dxt5,
    /// Two bytes a pixel.
    Rg8,
    /// One byte a pixel.
    R8,
}

impl PixelFormat {
    /// How many bytes a `width` x `height` level takes in this format.
    pub fn level_size(self, width: u32, height: u32) -> usize {
        let (w, h) = (width as usize, height as usize);
        let blocks = w.div_ceil(4).max(1) * h.div_ceil(4).max(1);
        match self {
            PixelFormat::Rgba8 => w * h * 4,
            PixelFormat::Rg8 => w * h * 2,
            PixelFormat::R8 => w * h,
            PixelFormat::Dxt1 => blocks * 8,
            PixelFormat::Dxt3 | PixelFormat::Dxt5 => blocks * 16,
        }
    }
}

/// Why a texture container could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureError {
    /// The file ends before what its header promises.
    Truncated,
    /// A header the reader does not know.
    Unknown(String),
    /// A level's compressed data does not unpack.
    Corrupt,
    /// There is no such image or level.
    NoSuchLevel,
}

impl fmt::Display for TextureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TextureError::Truncated => f.write_str("texture file is cut short"),
            TextureError::Unknown(what) => write!(f, "unknown texture container: {what}"),
            TextureError::Corrupt => f.write_str("texture data is corrupt"),
            TextureError::NoSuchLevel => f.write_str("no such texture level"),
        }
    }
}

impl std::error::Error for TextureError {}

/// One level of detail. Its bytes stay in the file until [`TextureFile::pixels`] asks.
#[derive(Debug, Clone)]
pub struct Mip {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    range: Range<usize>,
    /// The size its LZ4 block unpacks to, when it is one.
    unpacked: Option<usize>,
}

/// One image of the container with its levels, largest first.
#[derive(Debug, Clone, Default)]
pub struct Image {
    /// The levels of detail.
    pub mips: Vec<Mip>,
}

/// One frame of an animated texture: a rectangle of one of the images.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpriteFrame {
    /// Which image of the container the frame is cut from.
    pub image: u32,
    /// How long the frame shows, in seconds.
    pub duration: f32,
    /// Left edge in pixels.
    pub x: f32,
    /// Top edge in pixels.
    pub y: f32,
    /// Width in pixels (negative when the frame is stored rotated).
    pub width: f32,
    /// Height in pixels.
    pub height: f32,
    /// The rotated frame's second axis: how far the width runs down, the height across.
    pub skew: (f32, f32),
}

/// The frames of an animated texture.
#[derive(Debug, Clone, Default)]
pub struct SpriteSheet {
    /// The frames in playing order.
    pub frames: Vec<SpriteFrame>,
}

impl SpriteSheet {
    /// How long one loop takes, in seconds.
    pub fn duration(&self) -> f32 {
        self.frames.iter().map(|f| f.duration).sum()
    }

    /// Which frame shows `time` seconds in; the animation loops.
    pub fn frame_at(&self, time: f32) -> usize {
        let total = self.duration();
        if total <= 0.0 || self.frames.is_empty() {
            return 0;
        }
        let mut left = time.rem_euclid(total);
        for (i, frame) in self.frames.iter().enumerate() {
            if left < frame.duration {
                return i;
            }
            left -= frame.duration;
        }
        self.frames.len() - 1
    }
}

/// What one level holds.
#[derive(Debug)]
pub enum Pixels<'a> {
    /// Pixels ready for the GPU in `format`.
    Raw {
        /// How `data` is laid out.
        format: PixelFormat,
        /// The pixels; borrowed from the file unless they had to be unpacked.
        data: Cow<'a, [u8]>,
    },
    /// A PNG or JPEG file: see [`super::picture::decode`].
    Encoded(&'a [u8]),
    /// An MP4 file: the texture is a video.
    Video(&'a [u8]),
}

/// A parsed texture container: what it holds and where, without its pixels.
#[derive(Debug, Clone)]
pub struct TextureFile {
    /// The format of raw levels.
    pub format: PixelFormat,
    /// Width of the stored texture (padded to what the format needs).
    pub width: u32,
    /// Height of the stored texture.
    pub height: u32,
    /// Width of the picture inside the texture.
    pub image_width: u32,
    /// Height of the picture inside the texture.
    pub image_height: u32,
    /// Sample without repeating.
    pub clamp: bool,
    /// Sample without filtering.
    pub nearest: bool,
    /// The images; more than one only for animated textures.
    pub images: Vec<Image>,
    /// The frames, when the texture is animated.
    pub sprites: Option<SpriteSheet>,
    /// Levels hold an MP4 file.
    pub video: bool,
    /// How many slices a volume texture has; 1 for a flat one.
    pub depth: u32,
    /// Levels hold an encoded picture, not pixels.
    encoded: bool,
}

const FLAG_NEAREST: u32 = 1;
const FLAG_CLAMP: u32 = 2;
const FLAG_ANIMATED: u32 = 4;
const FLAG_VIDEO: u32 = 0x20;
const FLAG_VOLUME: u32 = 0x40;
/// FreeImage's code for "no file format": the levels are raw pixels.
const KIND_RAW: i32 = -1;
/// The code Wallpaper Engine uses for MP4.
const KIND_MP4: i32 = 35;

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], TextureError> {
        let end = self.at.checked_add(count).ok_or(TextureError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(TextureError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, TextureError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn f32(&mut self) -> Result<f32, TextureError> {
        Ok(f32::from_bits(self.u32()?))
    }

    /// A tag such as `TEXB0003`, written with a zero byte after it.
    fn tag(&mut self) -> Result<&'a str, TextureError> {
        let tag = self.take(9)?;
        std::str::from_utf8(&tag[..8]).map_err(|_| TextureError::Unknown("unreadable tag".to_owned()))
    }
}

impl TextureFile {
    /// Reads the container's headers. No pixel data is touched.
    pub fn parse(bytes: &[u8]) -> Result<Self, TextureError> {
        let mut cursor = Cursor { bytes, at: 0 };
        let (magic, info) = (cursor.tag()?, cursor.tag()?);
        if !magic.starts_with("TEXV") || !info.starts_with("TEXI") {
            return Err(TextureError::Unknown(format!("{magic}/{info}")));
        }
        let format = match cursor.u32()? {
            0 => PixelFormat::Rgba8,
            4 => PixelFormat::Dxt5,
            6 => PixelFormat::Dxt3,
            7 => PixelFormat::Dxt1,
            8 => PixelFormat::Rg8,
            9 => PixelFormat::R8,
            other => return Err(TextureError::Unknown(format!("pixel format {other}"))),
        };
        let flags = cursor.u32()?;
        let (width, height) = (cursor.u32()?, cursor.u32()?);
        let (image_width, image_height) = (cursor.u32()?, cursor.u32()?);
        // A volume (a colour lookup table) is stored as its slices side by side.
        let depth = if flags & FLAG_VOLUME != 0 { cursor.u32()? } else { 1 };
        // The picture's average colour.
        cursor.u32()?;

        let container = cursor.tag()?;
        let version = match container {
            "TEXB0001" => 1,
            "TEXB0002" => 2,
            "TEXB0003" => 3,
            "TEXB0004" => 4,
            other => return Err(TextureError::Unknown(other.to_owned())),
        };
        let image_count = cursor.u32()?;
        let kind = if version >= 3 { cursor.u32()? as i32 } else { KIND_RAW };
        if version >= 4 {
            cursor.u32()?;
        }
        let video = flags & FLAG_VIDEO != 0 || kind == KIND_MP4;
        let mut images = Vec::new();
        for _ in 0..image_count {
            let mip_count = cursor.u32()?;
            let mut mips = Vec::new();
            for _ in 0..mip_count {
                let (mip_width, mip_height) = (cursor.u32()?, cursor.u32()?);
                if flags & FLAG_VOLUME != 0 {
                    cursor.u32()?;
                }
                let unpacked = if version >= 2 {
                    let (packed, size) = (cursor.u32()?, cursor.u32()?);
                    (packed != 0).then_some(size as usize)
                } else {
                    None
                };
                let length = cursor.u32()? as usize;
                let start = cursor.at;
                cursor.take(length)?;
                mips.push(Mip { width: mip_width, height: mip_height, range: start..start + length, unpacked });
            }
            images.push(Image { mips });
        }

        let sprites = if flags & FLAG_ANIMATED != 0 && cursor.at < bytes.len() { Some(parse_sprites(&mut cursor)?) } else { None };
        Ok(Self {
            format,
            width,
            height,
            image_width,
            image_height,
            clamp: flags & FLAG_CLAMP != 0,
            nearest: flags & FLAG_NEAREST != 0,
            images,
            sprites,
            video,
            depth,
            encoded: kind != KIND_RAW && !video,
        })
    }

    /// The contents of level `level` of image `image`, unpacked when it was packed.
    ///
    /// `bytes` is the file [`parse`](Self::parse) read.
    pub fn pixels<'a>(&self, bytes: &'a [u8], image: usize, level: usize) -> Result<Pixels<'a>, TextureError> {
        let mip = self.images.get(image).and_then(|i| i.mips.get(level)).ok_or(TextureError::NoSuchLevel)?;
        let stored = bytes.get(mip.range.clone()).ok_or(TextureError::Truncated)?;
        let data = match mip.unpacked {
            Some(size) => Cow::Owned(lz4::decompress(stored, size).map_err(|_| TextureError::Corrupt)?),
            None => Cow::Borrowed(stored),
        };
        if self.video || self.encoded {
            // These are never packed in practice; if one is, it still has to outlive this call.
            let Cow::Borrowed(data) = data else { return Err(TextureError::Unknown("packed picture".to_owned())) };
            return Ok(if self.video { Pixels::Video(data) } else { Pixels::Encoded(data) });
        }
        Ok(Pixels::Raw { format: self.format, data })
    }
}

fn parse_sprites(cursor: &mut Cursor<'_>) -> Result<SpriteSheet, TextureError> {
    let tag = cursor.tag()?;
    let version = match tag {
        "TEXS0001" => 1,
        "TEXS0002" => 2,
        "TEXS0003" => 3,
        other => return Err(TextureError::Unknown(other.to_owned())),
    };
    let count = cursor.u32()?;
    if version >= 3 {
        cursor.u32()?;
        cursor.u32()?;
    }
    let mut frames = Vec::new();
    for _ in 0..count {
        let image = cursor.u32()?;
        let duration = cursor.f32()?;
        // The first version stores the rectangle in whole pixels.
        let number = |cursor: &mut Cursor<'_>| -> Result<f32, TextureError> {
            if version == 1 { cursor.u32().map(|v| v as i32 as f32) } else { cursor.f32() }
        };
        let (x, y, width, down, across, height) =
            (number(cursor)?, number(cursor)?, number(cursor)?, number(cursor)?, number(cursor)?, number(cursor)?);
        frames.push(SpriteFrame { image, duration, x, y, width, height, skew: (down, across) });
    }
    Ok(SpriteSheet { frames })
}
