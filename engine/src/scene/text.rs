//! Text layers: text rasterised into a texture the layer's shader samples.
//!
//! A text layer is an image layer whose picture is drawn by a font rasteriser
//! instead of read from a file. The texture is one byte a pixel (coverage), made
//! again only when the text changes.

use std::cell::RefCell;
use std::rc::Rc;

use ab_glyph::{Font, FontVec, Glyph, PxScale, ScaleFont, point};

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
    /// Space kept free around the text, in scene units: room for the layer's effects
    /// to reach into. A layer without effects has none.
    pub(crate) padding: f32,
    /// Wrap lines at this width, in scene units; 0 for the width of the box.
    pub(crate) max_width: f32,
}

/// The text of a layer, what it is drawn with, and the texture it is drawn into.
pub(crate) struct TextState {
    pub(crate) text: String,
    pub(crate) font: Rc<FontVec>,
    pub(crate) style: TextStyleSpec,
    /// The box the text is laid out in, in scene units: what the text takes, and the
    /// padding around it.
    pub(crate) size: [f32; 2],
    pub(crate) surface: Rc<TextSurface>,
    /// The text changed since the texture was made.
    pub(crate) dirty: bool,
}

impl TextState {
    /// The box `text` needs in this layer's style.
    pub(crate) fn fit(font: &FontVec, text: &str, style: &TextStyleSpec) -> [f32; 2] {
        let [width, height] = measure(font, text, style);
        [width + 2.0 * style.padding, height + 2.0 * style.padding]
    }

    /// Fits the box to the text as it is now; whether the box changed.
    pub(crate) fn refit(&mut self) -> bool {
        let size = Self::fit(&self.font, &self.text, &self.style);
        let changed = size != self.size;
        self.size = size;
        changed
    }

    /// Where the box's centre is from the layer's origin. The origin is where the
    /// text is aligned to: the left edge of left-aligned text, the middle of centred
    /// text, the top of text aligned to the top. So text that changes grows away
    /// from that edge, and padding (which is around the text) does not move it.
    pub(crate) fn offset(&self) -> [f32; 2] {
        let text = [self.size[0] - 2.0 * self.style.padding, self.size[1] - 2.0 * self.style.padding];
        [-f32::from(self.style.horizontal) * text[0] / 2.0, -f32::from(self.style.vertical) * text[1] / 2.0]
    }
}

/// One line of text: its glyphs with their offsets from the line's start, and its width.
struct Line {
    glyphs: Vec<(ab_glyph::GlyphId, f32)>,
    width: f32,
}

/// Lays `text` out in lines: at its own line breaks, and at `wrap_at` pixels when given.
fn lines<F: Font>(font: &impl ScaleFont<F>, text: &str, wrap_at: Option<f32>) -> Vec<Line> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = Line { glyphs: Vec::new(), width: 0.0 };
        // Where the line can be broken: the glyph count and width just before the last space.
        let mut break_at: Option<(usize, f32)> = None;
        let mut previous = None;
        for character in paragraph.chars().filter(|c| *c != '\r') {
            let id = font.glyph_id(character);
            let mut kern = previous.map_or(0.0, |before| font.kern(before, id));
            let advance = font.h_advance(id);
            if character == ' ' {
                break_at = Some((line.glyphs.len(), line.width));
            }
            if let Some(limit) = wrap_at
                && line.width + kern + advance > limit
                && let Some((count, width)) = break_at.take()
            {
                // The words after the last space move to a new line.
                let rest: Vec<(ab_glyph::GlyphId, f32)> = line.glyphs.split_off(count);
                let first =
                    rest.iter().position(|(glyph, _)| *glyph != font.glyph_id(' ')).unwrap_or(rest.len());
                let shift = rest.get(first).map_or(0.0, |(_, x)| *x);
                out.push(Line { glyphs: std::mem::take(&mut line.glyphs), width });
                line.glyphs = rest[first..].iter().map(|(glyph, x)| (*glyph, x - shift)).collect();
                line.width = if line.glyphs.is_empty() { 0.0 } else { line.width - shift };
                (previous, kern) = (None, 0.0);
                // A line does not start with the space it was broken at.
                if character == ' ' && line.glyphs.is_empty() {
                    continue;
                }
            }
            line.glyphs.push((id, line.width + kern));
            line.width += kern + advance;
            previous = Some(id);
        }
        out.push(line);
    }
    out
}

/// The scale a font is drawn at for a style, at `density` pixels a scene unit.
fn scale_of(font: &FontVec, style: &TextStyleSpec, density: f32) -> PxScale {
    // The size is that of the em square; the rasteriser wants the height from ascent to descent.
    let em = (style.point_size * POINT * density).clamp(1.0, MAX_SIDE as f32);
    PxScale::from(em * font.height_unscaled() / font.units_per_em().unwrap_or(1000.0))
}

/// How much room `text` takes, in scene units: the width of its widest line and
/// the height of its lines.
pub(crate) fn measure(font: &FontVec, text: &str, style: &TextStyleSpec) -> [f32; 2] {
    let scaled = font.as_scaled(scale_of(font, style, 1.0));
    let lines = lines(&scaled, text, (style.max_width > 0.0).then_some(style.max_width));
    let width = lines.iter().map(|line| line.width).fold(0.0, f32::max);
    let height = (scaled.height() + scaled.line_gap()) * lines.len().max(1) as f32;
    [width.ceil().max(1.0), height.ceil().max(1.0)]
}

/// Coverage of `text` laid out in a box of `size` scene units, at `density` pixels a
/// unit: (width, height, one byte a pixel, rows top to bottom).
pub(crate) fn rasterize(
    font: &FontVec,
    text: &str,
    style: &TextStyleSpec,
    size: [f32; 2],
    density: f32,
) -> (u32, u32, Vec<u8>) {
    let density = density.clamp(0.05, 4.0);
    let width = ((size[0] * density).ceil() as u32).clamp(1, MAX_SIDE);
    let height = ((size[1] * density).ceil() as u32).clamp(1, MAX_SIDE);
    let mut pixels = vec![0u8; (width * height) as usize];
    let padding = style.padding * density;
    let inner = ((width as f32 - 2.0 * padding).max(1.0), (height as f32 - 2.0 * padding).max(1.0));
    let scale = scale_of(font, style, density);
    let scaled = font.as_scaled(scale);
    // Lines break where the text says, and at a width only when the layer sets one.
    let lines = lines(&scaled, text, (style.max_width > 0.0).then_some(style.max_width * density));
    let line_height = scaled.height() + scaled.line_gap();
    let block = line_height * lines.len() as f32;
    let top = match style.vertical {
        1 => padding,
        -1 => padding + inner.1 - block,
        _ => padding + (inner.1 - block) / 2.0,
    };
    for (index, line) in lines.iter().enumerate() {
        // Each line is placed by its own width: one wider than the box overhangs
        // on both sides instead of being broken.
        let left = match style.horizontal {
            -1 => padding,
            1 => padding + inner.0 - line.width,
            _ => padding + (inner.0 - line.width) / 2.0,
        };
        let baseline = top + index as f32 * line_height + scaled.ascent();
        for (id, x) in &line.glyphs {
            let glyph: Glyph = id.with_scale_and_position(scale, point(left + x, baseline));
            let Some(outline) = font.outline_glyph(glyph) else { continue };
            let bounds = outline.px_bounds();
            let (x0, y0) = (bounds.min.x as i64, bounds.min.y as i64);
            outline.draw(|gx, gy, coverage| {
                let (x, y) = (x0 + i64::from(gx), y0 + i64::from(gy));
                if x >= 0 && y >= 0 && x < i64::from(width) && y < i64::from(height) {
                    let at = (y as u32 * width + x as u32) as usize;
                    // Glyphs that touch keep the stronger coverage.
                    pixels[at] = pixels[at].max((coverage.clamp(0.0, 1.0) * 255.0) as u8);
                }
            });
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
        match Texture::from_levels(
            gl,
            TextureFormat::R8,
            &[(width, height, &pixels)],
            Sampling { clamp: true, nearest: false },
        ) {
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
    let output = std::process::Command::new("fc-match")
        .args(["--format", "%{file}", family])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    let path = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Option<FontVec> {
        let path = system_font("sans")?;
        FontVec::try_from_vec(std::fs::read(path).ok()?).ok()
    }

    fn bounds(width: u32, pixels: &[u8]) -> Option<(u32, u32, u32, u32)> {
        let lit: Vec<(u32, u32)> = pixels
            .iter()
            .enumerate()
            .filter(|(_, c)| **c > 64)
            .map(|(i, _)| (i as u32 % width, i as u32 / width))
            .collect();
        Some((
            lit.iter().map(|p| p.0).min()?,
            lit.iter().map(|p| p.1).min()?,
            lit.iter().map(|p| p.0).max()?,
            lit.iter().map(|p| p.1).max()?,
        ))
    }

    #[test]
    fn layout_aligns_lines() {
        let Some(font) = font() else {
            eprintln!("skipped: no system font");
            return;
        };
        let style = |horizontal, vertical| TextStyleSpec {
            point_size: 8.0,
            horizontal,
            vertical,
            padding: 0.0,
            max_width: 0.0,
        };
        let (width, height, left_top) = rasterize(&font, "Hi", &style(-1, 1), [400.0, 200.0], 1.0);
        assert_eq!((width, height, left_top.len()), (400, 200, 400 * 200));
        let (x0, y0, x1, y1) = bounds(width, &left_top).expect("the text has pixels");
        assert!(
            x0 < 20 && y0 < 40 && x1 < 200 && y1 < 100,
            "left/top text sits in the top left corner: {x0} {y0} {x1} {y1}"
        );
        let (.., right_bottom) = rasterize(&font, "Hi", &style(1, -1), [400.0, 200.0], 1.0);
        let (x0, y0, x1, y1) = bounds(width, &right_bottom).unwrap();
        assert!(
            x0 > 200 && x1 > 380 && y0 > 100 && y1 > 160,
            "right/bottom text sits in the bottom right corner: {x0} {y0} {x1} {y1}"
        );
        let (.., centred) = rasterize(&font, "Hi", &style(0, 0), [400.0, 200.0], 1.0);
        let (x0, y0, x1, y1) = bounds(width, &centred).unwrap();
        assert!(
            ((x0 + x1) as i32 / 2 - 200).abs() < 12 && ((y0 + y1) as i32 / 2 - 100).abs() < 16,
            "centred text is centred: {x0} {y0} {x1} {y1}"
        );
    }

    #[test]
    fn density_scales_the_texture_and_empty_text_is_blank() {
        let Some(font) = font() else { return };
        let style =
            TextStyleSpec { point_size: 8.0, horizontal: 0, vertical: 0, padding: 8.0, max_width: 0.0 };
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

#[cfg(test)]
mod wrapping {
    use super::*;

    #[test]
    fn lines_break_at_the_text_and_at_a_width_only_when_asked() {
        let Some(path) = system_font("sans") else { return };
        let Ok(font) = FontVec::try_from_vec(std::fs::read(path).unwrap_or_default()) else { return };
        let scaled = font.as_scaled(PxScale::from(20.0));
        let unwrapped = lines(&scaled, "one two three four five six\nseven", None);
        assert_eq!(unwrapped.len(), 2, "without a width only the text's own breaks count");
        let wrapped = lines(&scaled, "one two three four five six", Some(unwrapped[0].width / 2.5));
        assert!(wrapped.len() >= 3, "a width breaks the line between words");
        assert!(
            wrapped.iter().all(|line| line.width <= unwrapped[0].width / 2.5 + 1.0),
            "and no line is wider than it"
        );
        let glyphs: usize = wrapped.iter().map(|line| line.glyphs.len()).sum();
        assert!(glyphs >= "onetwothreefourfivesix".len(), "no letter is lost at a break");
        let long_word = lines(&scaled, "incomprehensibilities", Some(10.0));
        assert_eq!(long_word.len(), 1, "a word with nowhere to break stays whole");
    }
}
