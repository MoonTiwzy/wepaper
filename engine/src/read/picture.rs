//! Pictures embedded in texture containers.

use std::fmt;

/// A decoded picture: 8-bit RGBA, rows top to bottom.
pub struct Picture {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Why a picture did not decode.
#[derive(Debug)]
pub struct PictureError(String);

impl fmt::Display for PictureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PictureError {}

/// Decodes a PNG or JPEG, told apart by its first bytes.
pub fn decode(data: &[u8]) -> Result<Picture, PictureError> {
    if data.starts_with(b"\x89PNG") {
        decode_png(data)
    } else if data.starts_with(&[0xff, 0xd8]) {
        decode_jpeg(data)
    } else {
        Err(PictureError("unsupported picture format (not PNG or JPEG)".to_owned()))
    }
}

fn decode_png(data: &[u8]) -> Result<Picture, PictureError> {
    let fail = |e: png::DecodingError| PictureError(format!("PNG: {e}"));
    let mut decoder = png::Decoder::new(data);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(fail)?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).map_err(fail)?;
    buffer.truncate(info.buffer_size());
    let pixels = (info.width * info.height) as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::Rgb => buffer.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::Grayscale => buffer.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => buffer.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Indexed => return Err(PictureError("PNG: palette was not expanded".to_owned())),
    };
    if rgba.len() != pixels * 4 {
        return Err(PictureError("PNG: unexpected amount of pixel data".to_owned()));
    }
    Ok(Picture { width: info.width, height: info.height, rgba })
}

fn decode_jpeg(data: &[u8]) -> Result<Picture, PictureError> {
    use zune_jpeg::zune_core::{colorspace::ColorSpace, options::DecoderOptions};
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(data, options);
    let rgba = decoder.decode().map_err(|e| PictureError(format!("JPEG: {e}")))?;
    let (width, height) = decoder.dimensions().ok_or_else(|| PictureError("JPEG: no dimensions".to_owned()))?;
    Ok(Picture { width: width as u32, height: height as u32, rgba })
}
