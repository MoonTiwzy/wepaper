//! Emitters: when particles are born and where.

use wepaper_json::Value;

use super::rng::Rng;
use super::value::{Vec3, direction, extent, index, mul, number, scale, vec3};

/// The volume an emitter fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    /// A box around the origin (`boxrandom`).
    Box,
    /// A sphere, or a ring when the directions flatten it (`sphererandom`).
    Sphere,
}

/// The emitter flag "limit to one per frame".
const ONE_PER_FRAME: u32 = 2;

/// One emitter of a system.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Emitter {
    shape: Shape,
    /// Where the volume sits, relative to the control point.
    origin: Vec3,
    /// How far the volume reaches along each axis, in multiples of the distance.
    directions: Vec3,
    distance_min: Vec3,
    distance_max: Vec3,
    /// Per axis: 1 keeps positions positive, -1 negative, 0 leaves them.
    sign: Vec3,
    speed_min: f32,
    speed_max: f32,
    /// Particles per second.
    pub(crate) rate: f32,
    /// Particles born at once when the emitter starts.
    pub(crate) instantaneous: u32,
    /// Seconds the emitter runs for; 0 for ever.
    pub(crate) duration: f32,
    /// Seconds before the emitter starts.
    pub(crate) delay: f32,
    /// The control point the volume is attached to.
    pub(crate) control_point: usize,
    /// Whether at most one particle is born per frame (ropes need the spacing).
    pub(crate) one_per_frame: bool,
}

impl Emitter {
    /// Reads an emitter; `None` for a kind this engine does not have.
    pub(crate) fn parse(doc: &Value) -> Option<Self> {
        let shape = match doc.get("name").as_str()? {
            "boxrandom" => Shape::Box,
            "sphererandom" => Shape::Sphere,
            _ => return None,
        };
        let (distance_min, distance_max) = match shape {
            Shape::Box => {
                (extent(doc, "distancemin", [0.0; 3]), extent(doc, "distancemax", [256.0, 256.0, 0.0]))
            }
            Shape::Sphere => (extent(doc, "distancemin", [0.0; 3]), extent(doc, "distancemax", [256.0; 3])),
        };
        Some(Self {
            shape,
            origin: vec3(doc, "origin", [0.0; 3]),
            directions: vec3(doc, "directions", [1.0, 1.0, 0.0]),
            distance_min,
            distance_max,
            sign: vec3(doc, "sign", [0.0; 3]),
            speed_min: number(doc, "speedmin", 0.0),
            speed_max: number(doc, "speedmax", 0.0),
            rate: number(doc, "rate", 5.0).max(0.0),
            instantaneous: number(doc, "instantaneous", 0.0).clamp(0.0, 1_000_000.0) as u32,
            duration: number(doc, "duration", 0.0).max(0.0),
            delay: number(doc, "delay", 0.0).max(0.0),
            control_point: index(doc, "controlpoint", 0, 7),
            one_per_frame: number(doc, "flags", 0.0) as u32 & ONE_PER_FRAME != 0,
        })
    }

    /// Whether the emitter only ever makes its opening burst.
    pub(crate) fn is_burst_only(&self) -> bool {
        self.rate <= 0.0
    }

    /// Picks where a particle is born, relative to the emitter's control point, and
    /// the velocity the emitter gives it (outwards, when it has a speed).
    pub(crate) fn place(&self, rng: &mut Rng) -> (Vec3, Vec3) {
        let mut offset = match self.shape {
            Shape::Box => {
                let mut at = [0.0f32; 3];
                for (axis, value) in at.iter_mut().enumerate() {
                    let reach = rng.between(self.distance_min[axis], self.distance_max[axis], 1.0);
                    *value = if rng.unit() < 0.5 { -reach } else { reach };
                }
                at
            }
            Shape::Sphere => {
                // The direction is drawn among the axes the emitter reaches along, so a
                // flattened sphere with equal distances is a ring, not a filled disc.
                let mut heading = [0.0f32; 3];
                for _ in 0..8 {
                    for (axis, value) in heading.iter_mut().enumerate() {
                        *value = if self.directions[axis] == 0.0 { 0.0 } else { rng.signed() };
                    }
                    let squared = heading.iter().map(|v| v * v).sum::<f32>();
                    if (1e-4..=1.0).contains(&squared) {
                        break;
                    }
                }
                let heading = direction(heading).unwrap_or([0.0; 3]);
                scale(heading, rng.between(self.distance_min[0], self.distance_max[0], 1.0))
            }
        };
        offset = mul(offset, self.directions);
        for (value, sign) in offset.iter_mut().zip(self.sign) {
            if sign > 0.0 {
                *value = value.abs();
            } else if sign < 0.0 {
                *value = -value.abs();
            }
        }
        let velocity = if self.speed_min == 0.0 && self.speed_max == 0.0 {
            [0.0; 3]
        } else {
            // A particle born exactly at the centre still has to fly somewhere.
            let outwards = direction(offset)
                .or_else(|| direction([rng.signed(), rng.signed(), 0.0]))
                .unwrap_or([0.0, 1.0, 0.0]);
            scale(outwards, rng.between(self.speed_min, self.speed_max, 1.0))
        };
        (super::value::add(offset, self.origin), velocity)
    }
}

#[cfg(test)]
mod tests {
    use super::super::value::length;
    use super::*;

    fn emitter(json: &str) -> Emitter {
        Emitter::parse(&wepaper_json::parse(json).unwrap()).unwrap()
    }

    #[test]
    fn box_fills_between_its_distances_on_every_reached_axis() {
        let emitter = emitter(
            r#"{"name":"boxrandom","distancemin":"10 0 0","distancemax":"20 5 9","directions":"1 2 0","origin":"100 0 0"}"#,
        );
        let mut rng = Rng::new(3);
        let (mut left, mut right) = (false, false);
        for _ in 0..500 {
            let (at, velocity) = emitter.place(&mut rng);
            let x = at[0] - 100.0;
            assert!((10.0..=20.0).contains(&x.abs()) && at[1].abs() <= 10.0 && at[2] == 0.0);
            assert_eq!(velocity, [0.0; 3]);
            left |= x < 0.0;
            right |= x > 0.0;
        }
        assert!(left && right);
    }

    #[test]
    fn flattened_sphere_with_one_distance_is_a_ring_and_sign_keeps_a_side() {
        let emitter = emitter(
            r#"{"name":"sphererandom","distancemin":64,"distancemax":64,"directions":"1 1 0","sign":"0 1 0","speedmin":5,"speedmax":5}"#,
        );
        let mut rng = Rng::new(4);
        for _ in 0..300 {
            let (at, velocity) = emitter.place(&mut rng);
            assert!((length(at) - 64.0).abs() < 1e-2 && at[2] == 0.0 && at[1] >= 0.0);
            assert!((length(velocity) - 5.0).abs() < 1e-3);
            assert!(super::super::value::dot(at, velocity) > 0.0, "the emitter's speed points outwards");
        }
    }

    #[test]
    fn unknown_kinds_are_not_emitters_and_odd_numbers_are_tamed() {
        assert!(Emitter::parse(&wepaper_json::parse(r#"{"name":"layerimage"}"#).unwrap()).is_none());
        assert!(Emitter::parse(&wepaper_json::parse("[]").unwrap()).is_none());
        let emitter =
            emitter(r#"{"name":"sphererandom","rate":-3,"instantaneous":-1,"controlpoint":99,"flags":2}"#);
        assert_eq!(
            (emitter.rate, emitter.instantaneous, emitter.control_point, emitter.one_per_frame),
            (0.0, 0, 7, true)
        );
    }
}
