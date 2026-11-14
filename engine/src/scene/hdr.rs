//! The high dynamic range pipeline of a scene with `general.hdr`.
//!
//! Such a scene is drawn in half floats from start to end: its passes are compiled
//! with `HDR` (Wallpaper Engine's lit shaders then let light and brightness go past
//! one), its layers' effect buffers keep what is over one from pass to pass, and the
//! finished frame is brought into the output's range only at the end. Its glow is the
//! stock one for such scenes (`hdr_downsample`, `combine_hdr`): what is brighter than
//! the threshold, cut in softly by the feather, is taken down to smaller and smaller
//! sizes, added back up the chain with the scatter, and added to the frame, which is
//! then clamped to what a screen shows.
//!
//! The scatter weighs each smaller size that much more than the one above it: how far
//! the glow spreads. How the original keeps that from adding up to more light than
//! there was is not known; here the weights are made to sum to one (the strength is
//! divided by their sum), so an evenly bright picture glows by its strength and a
//! small bright one spreads wide.
//!
//! A scene without `hdr` has none of this: no float buffer, no pass, no combo.

use crate::Engine;
use crate::doc::{BloomHdr, Dyn};
use crate::glsl::Combos;
use crate::gpu::{Blend, Target, TextureFormat};
use crate::math::Mat4;
use crate::read::Files;

use super::bindings::Bindings;
use super::draw::{Frame, LayerState, apply};
use super::shade::{PassInput, Shade, TextureChoice};

/// What the frame of an HDR scene and its buffers are kept in.
pub(crate) const FORMAT: TextureFormat = TextureFormat::Rgba16F;

/// The passes of the glow of an HDR scene.
pub(crate) struct HdrBloom {
    /// Keeps what is bright enough, at half the size.
    bright: Shade,
    /// Halves.
    down: Shade,
    /// Adds a smaller size onto the next larger one.
    up: Shade,
    /// Adds the glow to the frame and clamps it.
    combine: Shade,
    /// How many sizes the glow goes down through.
    iterations: u32,
    pub(crate) enabled: bool,
}

/// The stock shader's soft threshold: `x` the threshold, `y` where the soft part
/// starts, `z` how wide it is, `w` what makes it a curve.
fn blend_params(threshold: f32, feather: f32) -> [f32; 4] {
    let knee = (threshold * feather).max(1e-5);
    [threshold, threshold - knee, 2.0 * knee, 0.25 / knee]
}

/// What the glow's strength is divided by: the sum of the weights the scatter gives
/// its sizes, the largest weighing one.
fn spread(scatter: f32, iterations: u32) -> f32 {
    (0..iterations.max(1)).map(|level| scatter.max(0.0).powi(level as i32)).sum::<f32>().max(1e-6)
}

/// The sizes the glow is taken down through: half the frame's, then each half the
/// one before, `iterations` of them at most and none smaller than a pixel.
fn levels(size: (u32, u32), iterations: u32) -> Vec<(u32, u32)> {
    let mut levels = Vec::new();
    let (mut width, mut height) = size;
    for _ in 0..iterations {
        if width <= 1 && height <= 1 {
            break;
        }
        (width, height) = ((width / 2).max(1), (height / 2).max(1));
        levels.push((width, height));
    }
    levels
}

impl HdrBloom {
    /// The glow's passes, from the stock shaders, set as the scene says.
    pub(crate) fn build(
        engine: &mut Engine,
        files: &Files,
        bindings: &mut Bindings,
        settings: &BloomHdr,
    ) -> Result<Self, String> {
        let constant = |value: Vec<f32>| {
            let text = value.iter().map(f32::to_string).collect::<Vec<_>>().join(" ");
            Dyn::constant(wepaper_json::Value::from(text.as_str()))
        };
        let strength = settings.strength.value / spread(settings.scatter, settings.iterations);
        let bright = [
            ("bloomstrength".to_owned(), constant(vec![strength])),
            ("blend".to_owned(), constant(blend_params(settings.threshold.value, settings.feather).to_vec())),
            ("bloomtint".to_owned(), constant(settings.tint.to_vec())),
        ];
        let scatter = [("scatter".to_owned(), constant(vec![settings.scatter]))];
        let mut pass = |shader: &str, combo: Option<&str>, textures: &[TextureChoice], constants| {
            let combos: Combos = combo.map(|name| (name.to_owned(), 1)).into_iter().collect();
            Shade::compile(
                &mut engine.share,
                files,
                &PassInput { shader, textures, combos: &combos, constants, buffers: &[], slots: &[] },
                bindings,
            )
        };
        let previous = [TextureChoice::Previous];
        Ok(Self {
            bright: pass("hdr_downsample", Some("BLOOM"), &previous, &bright)?,
            down: pass("hdr_downsample", None, &previous, &[])?,
            up: pass("hdr_downsample", Some("UPSAMPLE"), &previous, &scatter)?,
            // The output is not an sRGB surface: the glow is added and the sum clamped.
            combine: pass(
                "combine_hdr",
                Some("LINEAR"),
                &[TextureChoice::Previous, TextureChoice::Buffer(0)],
                &[],
            )?,
            iterations: settings.iterations,
            enabled: true,
        })
    }

    /// Adds the glow to the finished `frame` and draws the result into `to` (`None`:
    /// the current surface) of `size`. Without the memory for its buffers, the frame
    /// goes out without it.
    pub(super) fn finish(
        &self,
        engine: &mut Engine,
        frame: &Frame<'_>,
        staged: &Target,
        to: Option<glow::Framebuffer>,
        size: (u32, u32),
    ) {
        engine.gl.set_blend(Blend::None);
        let sizes = levels(size, self.iterations);
        let mut chain: Vec<Target> = Vec::with_capacity(sizes.len());
        for &(width, height) in &sizes {
            match engine.pool.take(width, height, FORMAT) {
                Ok(target) => chain.push(target),
                Err(error) => {
                    log::warn!("hdr bloom: {error}");
                    break;
                }
            }
        }
        if chain.is_empty() {
            staged.blit(to, size);
            return;
        }
        // Each pass reads four taps a texel of its source apart, diagonally.
        let run = |engine: &Engine, shade: &Shade, source: &Target, extra: Option<&crate::gpu::Texture>| {
            let (x, y) = (1.0 / source.width() as f32, 1.0 / source.height() as f32);
            let state = LayerState {
                mvp: Mat4::IDENTITY,
                model: Mat4::IDENTITY,
                color: [1.0; 3],
                alpha: 1.0,
                brightness: 1.0,
                authored: frame.screen,
                vars: [[-x, -y, x, y], [0.0; 4]],
            };
            let pictures = Vec::new();
            apply(
                &engine.gl,
                shade,
                frame,
                &state,
                Some(&source.texture),
                &[extra],
                None,
                &pictures,
                engine.white(),
            );
            engine.fullscreen.draw();
        };
        chain[0].bind();
        run(engine, &self.bright, staged, None);
        for level in 1..chain.len() {
            chain[level].bind();
            run(engine, &self.down, &chain[level - 1], None);
        }
        // Back up: each size adds itself onto the next larger one.
        engine.gl.set_blend(Blend::Additive);
        for level in (1..chain.len()).rev() {
            chain[level - 1].bind();
            run(engine, &self.up, &chain[level], None);
        }
        engine.gl.set_blend(Blend::None);
        engine.gl.bind_framebuffer(to, size);
        run(engine, &self.combine, staged, Some(&chain[0].texture));
        for target in chain {
            engine.pool.give(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glow_goes_down_by_halves_as_far_as_it_is_told() {
        assert_eq!(levels((1920, 1080), 3), [(960, 540), (480, 270), (240, 135)]);
        assert_eq!(levels((4, 2), 8), [(2, 1), (1, 1)], "never smaller than a pixel");
    }

    #[test]
    fn the_sizes_of_the_glow_weigh_one_together() {
        assert!((spread(1.0, 8) - 8.0).abs() < 1e-6);
        assert!((spread(3.0, 3) - 13.0).abs() < 1e-5, "1 + 3 + 9");
        assert!(spread(0.0, 4) >= 1.0, "a scatter of nothing keeps the largest size alone");
    }

    #[test]
    fn the_threshold_cuts_in_over_the_feather() {
        let [threshold, start, width, curve] = blend_params(0.62, 0.4);
        assert_eq!(threshold, 0.62);
        assert!(
            (start - 0.372).abs() < 1e-6
                && (width - 0.496).abs() < 1e-6
                && (curve - 1.008_064_5).abs() < 1e-5
        );
        // A feather of nothing is a hard cut, without dividing by zero.
        assert!(blend_params(1.0, 0.0)[3].is_finite());
    }
}
