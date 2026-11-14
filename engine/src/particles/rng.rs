//! The random numbers a particle system draws: a seeded stream for spawning and a
//! stateless hash for the per-particle constants operators need every frame.

/// A seeded stream of random numbers (xorshift64*). The same seed gives the same stream.
#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    /// A stream for `seed`.
    pub(crate) fn new(seed: u64) -> Self {
        // Scrambled so that neighbouring seeds do not give neighbouring streams; never zero,
        // which xorshift cannot leave.
        let mut state = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        state = (state ^ (state >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        state = (state ^ (state >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Self((state ^ (state >> 31)) | 1)
    }

    /// The next 32 random bits.
    pub(crate) fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }

    /// A number in `[0, 1)`.
    pub(crate) fn unit(&mut self) -> f32 {
        unit_from(self.next_u32())
    }

    /// A number in `[-1, 1)`.
    pub(crate) fn signed(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// A number between `min` and `max`, biased by `exponent` the way the editor's
    /// "exponent" fields are: above 1 leans to `min`, towards 0 leans to `max`.
    pub(crate) fn between(&mut self, min: f32, max: f32, exponent: f32) -> f32 {
        let t = self.unit();
        let t = if exponent == 1.0 { t } else { t.powf(exponent.max(0.0)) };
        min + (max - min) * t
    }
}

/// The top 24 bits as a number in `[0, 1)`: every such value is exact in an `f32`.
fn unit_from(bits: u32) -> f32 {
    (bits >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// A number in `[0, 1)` that is fixed for a particle (`seed`) and a purpose (`salt`).
///
/// Operators use it for what a particle "chose at random" for its whole life (an
/// oscillation frequency, a turbulence phase) without storing it per particle.
pub(crate) fn fixed_unit(seed: u32, salt: u32) -> f32 {
    let mut x = seed ^ salt.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    unit_from(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_repeats_its_stream_and_stays_in_range() {
        let (mut a, mut b, mut c) = (Rng::new(7), Rng::new(7), Rng::new(8));
        let mut differs = false;
        for _ in 0..1000 {
            let value = a.unit();
            assert_eq!(value, b.unit());
            assert!((0.0..1.0).contains(&value));
            differs |= value != c.unit();
        }
        assert!(differs);
    }

    #[test]
    fn exponent_leans_towards_an_end() {
        let mut rng = Rng::new(1);
        let mean = |rng: &mut Rng, exponent: f32| {
            (0..4000).map(|_| rng.between(0.0, 1.0, exponent)).sum::<f32>() / 4000.0
        };
        assert!((mean(&mut rng, 1.0) - 0.5).abs() < 0.05);
        assert!(mean(&mut rng, 4.0) < 0.3);
        assert_eq!(rng.between(2.0, 5.0, 0.0), 5.0);
    }

    #[test]
    fn fixed_numbers_depend_on_particle_and_purpose() {
        assert_eq!(fixed_unit(42, 3), fixed_unit(42, 3));
        assert_ne!(fixed_unit(42, 3), fixed_unit(42, 4));
        assert_ne!(fixed_unit(42, 3), fixed_unit(43, 3));
        assert!((0..2000).all(|seed| (0.0..1.0).contains(&fixed_unit(seed, 9))));
    }
}
