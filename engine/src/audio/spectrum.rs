//! The audio spectrum shaders and scripts read.

use super::capture::{Capture, WINDOW};
use super::fft;

/// The loudness of the sound by frequency, for each ear, at three resolutions.
#[derive(Debug, Clone, PartialEq)]
pub struct Spectrum {
    left: [f32; 64],
    right: [f32; 64],
}

impl Default for Spectrum {
    fn default() -> Self {
        Self { left: [0.0; 64], right: [0.0; 64] }
    }
}

impl Spectrum {
    /// The bands of one ear at a resolution of 16, 32 or 64, lowest frequency first.
    /// Coarser resolutions are averages of the finest. Another count gives nothing.
    pub fn bands(&self, count: usize, right: bool, out: &mut Vec<f32>) {
        out.clear();
        let source = if right { &self.right } else { &self.left };
        if matches!(count, 16 | 32 | 64) {
            let group = 64 / count;
            out.extend(source.chunks_exact(group).map(|chunk| chunk.iter().sum::<f32>() / group as f32));
        }
    }

    /// Whether there is any sound in it.
    pub fn is_silent(&self) -> bool {
        self.left.iter().chain(&self.right).all(|band| *band < 1e-4)
    }

    /// Computes the bands of interleaved stereo `samples` (the newest window) and
    /// eases the current bands towards them: up fast, down slowly, as a meter moves.
    pub(super) fn feed(&mut self, samples: &[f32], dt: f32) {
        let frames = samples.len() / 2;
        if frames < WINDOW {
            self.decay(dt);
            return;
        }
        let newest = &samples[(frames - WINDOW) * 2..];
        for (channel, bands) in [&mut self.left, &mut self.right].into_iter().enumerate() {
            let mut re = [0.0f32; WINDOW];
            let mut im = [0.0f32; WINDOW];
            for (i, value) in re.iter_mut().enumerate() {
                // A Hann window, so the edges of the block do not ring.
                let window = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (WINDOW - 1) as f32).cos();
                *value = newest[i * 2 + channel] * window;
            }
            fft::transform(&mut re, &mut im);
            for (band, level) in bands.iter_mut().enumerate() {
                // Bands are spread evenly in pitch, from about 40 Hz to about 16 kHz.
                let edge = |b: usize| (1.0 * (372.0f32 / 1.0).powf(b as f32 / 64.0)).round() as usize;
                let (from, to) = (edge(band).max(1), edge(band + 1).max(edge(band) + 1).min(WINDOW / 2));
                let power: f32 = (from..to).map(|bin| re[bin] * re[bin] + im[bin] * im[bin]).sum::<f32>() / (to - from).max(1) as f32;
                // Scaled so that music at a normal level fills most of 0..1.
                let target = (power.sqrt() / (WINDOW as f32 / 4.0) * 6.0).min(1.5);
                let rate = if target > *level { 30.0 } else { 6.0 };
                *level += (target - *level) * (rate * dt).min(1.0);
            }
        }
    }

    fn decay(&mut self, dt: f32) {
        for level in self.left.iter_mut().chain(&mut self.right) {
            *level -= *level * (6.0 * dt).min(1.0);
        }
    }
}

/// Listens to the system's sound while anyone wants the spectrum.
#[derive(Default)]
pub struct Listener {
    capture: Option<Capture>,
    /// Starting failed once: do not try again every frame.
    unavailable: bool,
    spectrum: Spectrum,
}

impl Listener {
    /// A listener that is not listening yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The spectrum as of the last [`update`](Self::update).
    pub fn spectrum(&self) -> &Spectrum {
        &self.spectrum
    }

    /// Whether the recording is running.
    pub fn listening(&self) -> bool {
        self.capture.is_some()
    }

    /// Brings the spectrum up to date. `wanted` says whether anything shown reads
    /// it: the recording starts when something does and stops when nothing does.
    pub fn update(&mut self, wanted: bool, dt: f32) {
        if !wanted {
            if self.capture.take().is_some() {
                self.spectrum = Spectrum::default();
            }
            return;
        }
        if self.capture.is_none() && !self.unavailable {
            self.capture = Capture::start();
            self.unavailable = self.capture.is_none();
        }
        match &self.capture {
            Some(capture) => {
                // The lock is held only to copy two thousand numbers.
                let samples = capture.samples.lock().map(|samples| samples.clone()).unwrap_or_default();
                self.spectrum.feed(&samples, dt);
            }
            None => self.spectrum.decay(dt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f32, amplitude: f32) -> Vec<f32> {
        (0..WINDOW).flat_map(|i| [(2.0 * std::f32::consts::PI * frequency * i as f32 / 44100.0).sin() * amplitude; 2]).collect()
    }

    fn settled(samples: &[f32]) -> Spectrum {
        let mut spectrum = Spectrum::default();
        for _ in 0..200 {
            spectrum.feed(samples, 1.0 / 60.0);
        }
        spectrum
    }

    #[test]
    fn bands_follow_pitch() {
        let mut out = Vec::new();
        settled(&tone(100.0, 0.5)).bands(64, false, &mut out);
        let low = (0..64).max_by(|a, b| out[*a].total_cmp(&out[*b])).unwrap();
        settled(&tone(5000.0, 0.5)).bands(64, false, &mut out);
        let high = (0..64).max_by(|a, b| out[*a].total_cmp(&out[*b])).unwrap();
        assert!(low < 20 && high > 40, "a low tone lights a low band ({low}), a high tone a high one ({high})");
    }

    #[test]
    fn bands_are_normalised() {
        let mut out = Vec::new();
        let loud = settled(&tone(1000.0, 1.0));
        loud.bands(64, true, &mut out);
        assert!(out.iter().all(|b| (0.0..=1.5).contains(b)), "no band leaves 0..1.5");
        assert!(out.iter().copied().fold(0.0, f32::max) > 0.3, "a full-scale tone is clearly there");
        loud.bands(16, true, &mut out);
        assert_eq!(out.len(), 16);
        loud.bands(32, false, &mut out);
        assert_eq!(out.len(), 32);
        loud.bands(7, false, &mut out);
        assert!(out.is_empty(), "only 16, 32 and 64 exist");
        assert!(settled(&vec![0.0; WINDOW * 2]).is_silent());
    }

    #[test]
    fn spectrum_rises_fast_and_falls_slowly() {
        let mut spectrum = Spectrum::default();
        let tone = tone(1000.0, 0.8);
        spectrum.feed(&tone, 1.0 / 60.0);
        let mut out = Vec::new();
        spectrum.bands(64, false, &mut out);
        let after_one = out.iter().copied().fold(0.0, f32::max);
        let full = {
            settled(&tone).bands(64, false, &mut out);
            out.iter().copied().fold(0.0, f32::max)
        };
        assert!(after_one > full * 0.3, "one frame in, the band is well on its way up");
        let mut falling = settled(&tone);
        falling.feed(&vec![0.0; WINDOW * 2], 1.0 / 60.0);
        falling.bands(64, false, &mut out);
        assert!(out.iter().copied().fold(0.0, f32::max) > full * 0.8, "one frame of silence barely lowers it");
        falling.feed(&[], 1.0 / 60.0);
        assert!(!falling.is_silent(), "too few samples only lets the bands fall");
    }

    #[test]
    fn listener_without_wish_does_nothing() {
        let mut listener = Listener::new();
        listener.update(false, 1.0 / 60.0);
        assert!(!listener.listening() && listener.spectrum().is_silent());
    }
}
