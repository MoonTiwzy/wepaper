//! An output's picture cut from a larger one, on the CPU: for a renderer plugin that
//! draws a page as a monitor would see it and gives a device the part its fill and
//! look frame, and for the daemon's snapshot of such a device. The same placement
//! as the engine's (`Frame`): the picture is fitted by the fill, enlarged by the zoom
//! about the output's middle and moved by the shifts.

use crate::Framing;

/// How a picture of another shape fills the output (the daemon's `fill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fit {
    /// Covers the output, what does not fit is cropped.
    #[default]
    Cover,
    /// All of it, with bars.
    Contain,
    /// Covers the output, distorted.
    Stretch,
}

impl Fit {
    /// `cover`, `contain` or `stretch`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "cover" => Some(Self::Cover),
            "contain" => Some(Self::Contain),
            "stretch" => Some(Self::Stretch),
            _ => None,
        }
    }

    /// The name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cover => "cover",
            Self::Contain => "contain",
            Self::Stretch => "stretch",
        }
    }
}

/// Where the pixels of an output come from in a picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    picture: (u32, u32),
    output: (u32, u32),
    /// The picture's size in output widths and heights, after the fill.
    span: (f32, f32),
    framing: Framing,
}

impl Cut {
    /// The cut of a `picture` large picture for an `output` large output.
    pub fn new(picture: (u32, u32), output: (u32, u32), fit: Fit, framing: Framing) -> Self {
        let (pw, ph) = (picture.0.max(1) as f32, picture.1.max(1) as f32);
        let (ow, oh) = (output.0.max(1) as f32, output.1.max(1) as f32);
        let scale = match fit {
            Fit::Cover => (ow / pw).max(oh / ph),
            Fit::Contain => (ow / pw).min(oh / ph),
            Fit::Stretch => 0.0,
        };
        let span = if fit == Fit::Stretch { (1.0, 1.0) } else { (pw * scale / ow, ph * scale / oh) };
        let zoom = if framing.zoom.is_finite() && framing.zoom > 0.0 { framing.zoom } else { 1.0 };
        Self { picture, output, span, framing: Framing { zoom, ..framing } }
    }

    /// Where the middle of the output's pixel `x`, `y` (from the top left) is in the
    /// picture, in its pixels from the top left; `None` where the picture is not.
    pub fn source(&self, x: u32, y: u32) -> Option<(f32, f32)> {
        // The output's point, in its widths and heights from the middle, up.
        let ux = (x as f32 + 0.5) / self.output.0.max(1) as f32 - 0.5;
        let uy = 0.5 - (y as f32 + 0.5) / self.output.1.max(1) as f32;
        // Before the zoom and the shifts, then in the picture's widths and heights.
        let vx = (ux - self.framing.shift_x) / self.framing.zoom / self.span.0;
        let vy = (uy - self.framing.shift_y) / self.framing.zoom / self.span.1;
        if vx.abs() > 0.5 || vy.abs() > 0.5 {
            return None;
        }
        Some(((vx + 0.5) * self.picture.0 as f32, (0.5 - vy) * self.picture.1 as f32))
    }

    /// The output's picture: four bytes a pixel in the order of `picture`'s, black
    /// where the picture is not, sampled between the four nearest pixels.
    pub fn apply(&self, picture: &[u8]) -> Vec<u8> {
        let (ow, oh) = self.output;
        let mut out = Vec::with_capacity(ow as usize * oh as usize * 4);
        for y in 0..oh {
            for x in 0..ow {
                let pixel = self.source(x, y).map_or([0, 0, 0, 255], |at| sample(picture, self.picture, at));
                out.extend_from_slice(&pixel);
            }
        }
        out
    }
}

/// The colour at `at` (pixels from the top left, pixel middles at .5) of a picture
/// of four bytes a pixel, between its four nearest pixels.
pub fn sample(picture: &[u8], size: (u32, u32), at: (f32, f32)) -> [u8; 4] {
    let (w, h) = (size.0.max(1) as usize, size.1.max(1) as usize);
    if picture.len() < w * h * 4 {
        return [0, 0, 0, 255];
    }
    let fx = (at.0 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (at.1 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let px = |x: usize, y: usize, c: usize| f32::from(picture[(y * w + x) * 4 + c]);
    let mut pixel = [0u8; 4];
    for (c, value) in pixel.iter_mut().enumerate() {
        let top = px(x0, y0, c) * (1.0 - tx) + px(x1, y0, c) * tx;
        let bottom = px(x0, y1, c) * (1.0 - tx) + px(x1, y1, c) * tx;
        *value = (top * (1.0 - ty) + bottom * ty).round() as u8;
    }
    pixel
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Framing {
        Framing { zoom: 1.0, shift_x: 0.0, shift_y: 0.0 }
    }

    #[track_caller]
    fn near(a: Option<(f32, f32)>, b: (f32, f32)) {
        let a = a.expect("inside the picture");
        assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3, "{a:?} != {b:?}");
    }

    #[test]
    fn stretched_the_pixels_meet() {
        let cut = Cut::new((4, 2), (4, 2), Fit::Stretch, plain());
        near(cut.source(0, 0), (0.5, 0.5));
        near(cut.source(3, 1), (3.5, 1.5));
    }

    #[test]
    fn a_wide_picture_covers_a_square_by_its_middle() {
        // 16:9 onto 9x9: the middle nine columns.
        let cut = Cut::new((16, 9), (9, 9), Fit::Cover, plain());
        near(cut.source(0, 0), (4.0, 0.5));
        near(cut.source(8, 8), (12.0, 8.5));
        near(cut.source(4, 4), (8.0, 4.5));
    }

    #[test]
    fn contained_it_leaves_bars() {
        let cut = Cut::new((16, 9), (16, 16), Fit::Contain, plain());
        assert_eq!(cut.source(8, 0), None, "a bar above");
        // Row 8.5 of 16 is 5 rows into the 9 the picture spans in the middle.
        near(cut.source(0, 8), (0.5, 5.0));
    }

    #[test]
    fn the_zoom_enlarges_about_the_middle_and_the_shifts_move_it() {
        let zoomed = Cut::new((100, 100), (100, 100), Fit::Stretch, Framing { zoom: 2.0, ..plain() });
        near(zoomed.source(0, 0), (25.25, 25.25));
        // Moved a quarter of the output right: the left quarter of the picture is in the middle.
        let moved = Cut::new((100, 100), (100, 100), Fit::Stretch, Framing { shift_x: 0.25, ..plain() });
        near(moved.source(50, 50), (25.5, 50.5));
        // Moved up: what was lower comes into the middle.
        let up = Cut::new((100, 100), (100, 100), Fit::Stretch, Framing { shift_y: 0.25, ..plain() });
        near(up.source(50, 50), (50.5, 75.5));
        // Shrunk: the picture is small in the middle, black around.
        let small = Cut::new((100, 100), (100, 100), Fit::Stretch, Framing { zoom: 0.5, ..plain() });
        assert_eq!(small.source(0, 0), None);
    }

    #[test]
    fn the_cut_samples_between_pixels() {
        // Black and white side by side, in the middle half grey.
        let picture = [0, 0, 0, 255, 255, 255, 255, 255];
        assert_eq!(sample(&picture, (2, 1), (1.0, 0.5)), [128, 128, 128, 255]);
        assert_eq!(sample(&picture, (2, 1), (0.5, 0.5)), [0, 0, 0, 255]);
        let out = Cut::new((2, 1), (4, 1), Fit::Stretch, plain()).apply(&picture);
        assert_eq!(out.len(), 16);
        assert_eq!(&out[..4], &[0, 0, 0, 255]);
        assert_eq!(&out[12..], &[255, 255, 255, 255]);
        // Outside the picture: black.
        let shrunk =
            Cut::new((2, 1), (4, 1), Fit::Stretch, Framing { zoom: 0.25, ..plain() }).apply(&picture);
        assert_eq!(&shrunk[..4], &[0, 0, 0, 255]);
    }
}
