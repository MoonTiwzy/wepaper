//! The smooth random field turbulence reads, and the cheap sine oscillators use.

/// A smooth random vector at `point`, each component in `[-1, 1]`.
///
/// Value noise on the unit lattice: one hash per corner gives all three components,
/// so a sample costs eight hashes instead of twenty-four.
pub(crate) fn vector(point: [f32; 3]) -> [f32; 3] {
    let cell = point.map(floor);
    // Far from the origin the cell saturates and the fraction is no fraction any more;
    // clamped, the field is merely flat there.
    let blend = [0, 1, 2].map(|axis| fade((point[axis] - cell[axis] as f32).clamp(0.0, 1.0)));
    let [x, y, z] = cell;
    let mut out = [0.0f32; 3];
    for corner in 0..8u32 {
        let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
        let weight = pick(blend[0], dx) * pick(blend[1], dy) * pick(blend[2], dz);
        let bits = lattice(x.wrapping_add(dx as i32), y.wrapping_add(dy as i32), z.wrapping_add(dz as i32));
        out[0] += weight * component(bits);
        out[1] += weight * component(bits >> 10);
        out[2] += weight * component(bits >> 20);
    }
    out
}

/// The whole number at or below `x`. `f32::floor` is a library call on the baseline
/// x86-64 target, and this runs several times per particle per frame.
fn floor(x: f32) -> i32 {
    let whole = x as i32;
    whole.wrapping_sub(i32::from(whole as f32 > x))
}

/// The weight of the near (`side` 0) or far (`side` 1) corner along one axis.
fn pick(blend: f32, side: u32) -> f32 {
    if side == 0 { 1.0 - blend } else { blend }
}

/// Smoothstep, so the field has no creases at cell borders.
fn fade(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Ten bits as a number in `[-1, 1]`.
fn component(bits: u32) -> f32 {
    (bits & 0x3ff) as f32 * (2.0 / 1023.0) - 1.0
}

/// The random bits of a lattice point.
fn lattice(x: i32, y: i32, z: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

/// The sine of `angle` to about three decimals, several times cheaper than the exact
/// one; oscillators run it for every particle every frame.
pub(crate) fn sin(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    if !angle.is_finite() {
        return 0.0;
    }
    // Clamped for angles too large to wrap exactly: any value in range will do there.
    let x = (angle - TAU * floor(angle * (1.0 / TAU) + 0.5) as f32).clamp(-PI, PI);
    let y = (4.0 / PI) * x - (4.0 / (PI * PI)) * x * x.abs();
    0.225 * (y * y.abs() - y) + y
}

/// The cosine of `angle`, as cheap as [`sin`].
pub(crate) fn cos(angle: f32) -> f32 {
    sin(angle + std::f32::consts::FRAC_PI_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_is_bounded_continuous_and_not_constant() {
        let mut low = f32::MAX;
        let mut high = f32::MIN;
        let mut previous = vector([0.0, 0.3, 0.7]);
        for step in 1..4000 {
            let at = vector([step as f32 * 0.01, 0.3, 0.7]);
            for axis in 0..3 {
                assert!((-1.0..=1.0).contains(&at[axis]));
                assert!((at[axis] - previous[axis]).abs() < 0.08, "a jump at step {step}");
                low = low.min(at[axis]);
                high = high.max(at[axis]);
            }
            previous = at;
        }
        assert!(low < -0.5 && high > 0.5);
        assert!(vector([f32::MAX, -f32::MAX, 1e30]).iter().all(|v| v.is_finite()));
    }

    #[test]
    fn cheap_sine_stays_close_to_the_exact_one() {
        for step in -4000..4000 {
            let angle = step as f32 * 0.01;
            assert!((sin(angle) - angle.sin()).abs() < 2e-3, "sin({angle})");
            assert!((cos(angle) - angle.cos()).abs() < 2e-3, "cos({angle})");
        }
        assert_eq!(sin(f32::NAN), 0.0);
        assert!([1e12, -1e12, f32::MAX, f32::MIN].iter().all(|&huge| sin(huge).abs() <= 1.001));
        assert_eq!([floor(-0.5), floor(-1.0), floor(0.0), floor(2.9), floor(-2.1)], [-1, -1, 0, 2, -3]);
    }
}
