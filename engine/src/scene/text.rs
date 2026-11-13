//! Text layers: text rasterised into a texture the layer's shader samples.
//!
//! A text layer is an image layer whose picture is drawn by a font rasteriser
//! instead of read from a file. The texture is one byte a pixel (coverage), made
//! again only when the text changes.

use std::cell::RefCell;
use std::rc::Rc;

use fontdue::layout::{CoordinateSystem, HorizontalAlign, Layout, LayoutSettings, TextStyle, VerticalAlign, WrapStyle};

use crate::gpu::{Gl, Sampling, Texture, TextureFormat};

/// The largest side of a text texture.
const MAX_SIDE: u32 = 4096;
/// Text layers give their size in points; Wallpaper Engine lays text out at 300 dots
/// an inch, which makes a point this many scene units.
const POINT: f32 = 300.0 / 72.0;

/// The texture a text layer's shader samples. Empty until the text is first drawn.
#[derive(Default)]
pub(crate) struct TextSurface {
    pub(crate) texture: RefCell<Option<Texture>>,
}

/// How a block of text is laid out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextStyleSpec {
    /// The size in points.
    pub(crate) point_size: f32,
    /// -1 left, 0 centre, 1 right.
    pub(crate) horizontal: i8,
    /// -1 bottom, 0 centre, 1 top.
    pub(crate) vertical: i8,
    /// Space kept free around the text, in scene units.
    pub(crate) padding: f32,
    /// Wrap lines at this width, in scene units; 0 for the width of the box.
    pub(crate) max_width: f32,
}

/// The text of a layer, what it is drawn with, and the texture it is drawn into.
pub(crate) struct TextState {
    pub(crate) text: String,
    pub(crate) font: Rc<fontdue::Font>,
    pub(crate) style: TextStyleSpec,
    /// The box the text is laid out in, in scene units.
    pub(crate) size: [f32; 2],
    pub(crate) surface: Rc<TextSurface>,
    /// The text changed since the texture was made.
    pub(crate) dirty: bool,
}

/// Coverage of `text` laid out in a box of `size` scene units, at `density` pixels a
/// unit: (width, height, one byte a pixel, rows top to bottom).
pub(crate) fn rasterize(font: &fontdue::Font, text: &str, style: &TextStyleSpec, size: [f32; 2], density: f32) -> (u32, u32, Vec<u8>) {
    let density = density.clamp(0.05, 4.0);
    let width = ((size[0] * density).ceil() as u32).clamp(1, MAX_SIDE);
    let height = ((size[1] * density).ceil() as u32).clamp(1, MAX_SIDE);
    let mut pixels = vec![0u8; (width * height) as usize];
    let padding = style.padding * density;
    let inner = ((width as f32 - 2.0 * padding).max(1.0), (height as f32 - 2.0 * padding).max(1.0));
    // Lines break where the text says, and at a width only when the layer sets one.
    let wrap_at = (style.max_width > 0.0).then(|| style.max_width * density);
    let mut layout: Layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        x: 0.0,
        y: padding,
        max_width: wrap_at,
        max_height: Some(inner.1),
        horizontal_align: HorizontalAlign::Left,
        vertical_align: match style.vertical {
            1 => VerticalAlign::Top,
            -1 => VerticalAlign::Bottom,
            _ => VerticalAlign::Middle,
        },
        wrap_style: WrapStyle::Word,
        wrap_hard_breaks: true,
        ..LayoutSettings::default()
    });
    layout.append(&[font], &TextStyle::new(text, (style.point_size * POINT * density).max(1.0), 0));
    // Each line is placed in the box by its own width: a line wider than the box
    // overhangs on both sides instead of being broken.
    let glyphs = layout.glyphs();
    let lines: Vec<(usize, usize)> = layout.lines().map(|lines| lines.iter().map(|line| (line.glyph_start, line.glyph_end)).collect()).unwrap_or_default();
    for (start, end) in lines {
        let Some(line) = glyphs.get(start..=end.min(glyphs.len().saturating_sub(1))) else { continue };
        let left = line.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
        let right = line.iter().map(|g| g.x + g.width as f32).fold(f32::NEG_INFINITY, f32::max);
        if !left.is_finite() || !right.is_finite() {
            continue;
        }
        let shift = match style.horizontal {
            -1 => padding - left,
            1 => padding + inner.0 - right,
            _ => padding + (inner.0 - (right - left)) / 2.0 - left,
        };
        for glyph in line.iter().filter(|g| g.width > 0 && g.height > 0) {
            let (_, coverage) = font.rasterize_config(glyph.key);
            let (x0, y0) = ((glyph.x + shift).round() as i64, glyph.y.round() as i64);
            for row in 0..glyph.height {
                let y = y0 + row as i64;
                if y < 0 || y >= i64::from(height) {
                    continue;
                }
                for column in 0..glyph.width {
                    let x = x0 + column as i64;
                    if x < 0 || x >= i64::from(width) {
                        continue;
                    }
                    let at = (y as u32 * width + x as u32) as usize;
                    // Glyphs that touch keep the stronger coverage.
                    pixels[at] = pixels[at].max(coverage[row * glyph.width + column]);
                }
            }
        }
    }
    (width, height, pixels)
}

impl TextState {
    /// Draws the text into its texture again if it changed.
    pub(crate) fn refresh(&mut self, gl: &Gl, density: f32) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let (width, height, pixels) = rasterize(&self.font, &self.text, &self.style, self.size, density);
        let mut slot = self.surface.texture.borrow_mut();
        // A texture of the same size is written into; another size needs a new one.
        if let Some(texture) = slot.as_ref().filter(|t| t.width == width && t.height == height) {
            let _ = texture.write(&pixels);
            return;
        }
        match Texture::from_levels(gl, TextureFormat::R8, &[(width, height, &pixels)], Sampling { clamp: true, nearest: false }) {
            Ok(texture) => *slot = Some(texture),
            Err(error) => log::warn!("text layer: {error}"),
        }
    }
}

/// The file a system font name stands for, asked of fontconfig.
pub(crate) fn system_font(name: &str) -> Option<std::path::PathBuf> {
    // `systemfont_arial`, `systemfont_comicsans` ...
    let family = match name.strip_prefix("systemfont_").unwrap_or(name) {
        "comicsans" => "Comic Sans MS",
        "timesnewroman" => "Times New Roman",
        "couriernew" => "Courier New",
        other => other,
    };
    let output = std::process::Command::new("fc-match").args(["--format", "%{file}", family]).stdin(std::process::Stdio::null()).output().ok()?;
    let path = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Option<fontdue::Font> {
        let path = system_font("sans")?;
        fontdue::Font::from_bytes(std::fs::read(path).ok()?, fontdue::FontSettings::default()).ok()
    }

    fn bounds(width: u32, pixels: &[u8]) -> Option<(u32, u32, u32, u32)> {
        let lit: Vec<(u32, u32)> = pixels.iter().enumerate().filter(|(_, c)| **c > 64).map(|(i, _)| (i as u32 % width, i as u32 / width)).collect();
        Some((lit.iter().map(|p| p.0).min()?, lit.iter().map(|p| p.1).min()?, lit.iter().map(|p| p.0).max()?, lit.iter().map(|p| p.1).max()?))
    }

    #[test]
    fn layout_aligns_lines() {
        let Some(font) = font() else {
            eprintln!("skipped: no system font");
            return;
        };
        let style = |horizontal, vertical| TextStyleSpec { point_size: 8.0, horizontal, vertical, padding: 0.0, max_width: 0.0 };
        let (width, height, left_top) = rasterize(&font, "Hi", &style(-1, 1), [400.0, 200.0], 1.0);
        assert_eq!((width, height, left_top.len()), (400, 200, 400 * 200));
        let (x0, y0, x1, y1) = bounds(width, &left_top).expect("the text has pixels");
        assert!(x0 < 20 && y0 < 40 && x1 < 200 && y1 < 100, "left/top text sits in the top left corner: {x0} {y0} {x1} {y1}");
        let (.., right_bottom) = rasterize(&font, "Hi", &style(1, -1), [400.0, 200.0], 1.0);
        let (x0, y0, x1, y1) = bounds(width, &right_bottom).unwrap();
        assert!(x0 > 200 && x1 > 380 && y0 > 100 && y1 > 160, "right/bottom text sits in the bottom right corner: {x0} {y0} {x1} {y1}");
        let (.., centred) = rasterize(&font, "Hi", &style(0, 0), [400.0, 200.0], 1.0);
        let (x0, y0, x1, y1) = bounds(width, &centred).unwrap();
        assert!(((x0 + x1) as i32 / 2 - 200).abs() < 12 && ((y0 + y1) as i32 / 2 - 100).abs() < 16, "centred text is centred: {x0} {y0} {x1} {y1}");
    }

    #[test]
    fn density_scales_the_texture_and_empty_text_is_blank() {
        let Some(font) = font() else { return };
        let style = TextStyleSpec { point_size: 8.0, horizontal: 0, vertical: 0, padding: 8.0, max_width: 0.0 };
        let (width, height, pixels) = rasterize(&font, "Hello world", &style, [300.0, 100.0], 0.5);
        assert_eq!((width, height), (150, 50), "half the density is half the pixels");
        assert!(pixels.iter().any(|c| *c > 64));
        let (.., blank) = rasterize(&font, "", &style, [300.0, 100.0], 1.0);
        assert!(blank.iter().all(|c| *c == 0));
        // Text far larger than its box is clipped, never out of bounds.
        let huge = TextStyleSpec { point_size: 900.0, ..style };
        let (w, h, clipped) = rasterize(&font, "WWWW\nWWWW", &huge, [64.0, 64.0], 1.0);
        assert_eq!(clipped.len(), (w * h) as usize);
    }
}
