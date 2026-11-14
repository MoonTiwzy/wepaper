//! Operators: what happens to particles while they live.

use std::f32::consts::TAU;

use wepaper_json::Value;

use super::noise;
use super::particle::{ControlPoint, Particle};
use super::rng::fixed_unit;
use super::value::{Vec3, add, cross, direction, dot, index, length, number, scale, sub, vec3};

/// What an operator may look at besides the particles.
pub(crate) struct Frame<'a> {
    /// Seconds this step covers.
    pub(crate) dt: f32,
    /// Seconds the system has run.
    pub(crate) clock: f32,
    /// The control points this frame.
    pub(crate) control_points: &'a [ControlPoint; 8],
    /// The instance's `speed` factor, which scales every force.
    pub(crate) force: f32,
}

/// A multiplier that goes from one value to another over a part of a particle's life.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ramp<T> {
    start_time: f32,
    end_time: f32,
    start_value: T,
    end_value: T,
}

impl<T> Ramp<T> {
    /// How far the ramp has gone at `life`, 0 to 1.
    fn progress(&self, life: f32) -> f32 {
        let span = self.end_time - self.start_time;
        if span <= 1e-6 {
            if life >= self.end_time { 1.0 } else { 0.0 }
        } else {
            ((life - self.start_time) / span).clamp(0.0, 1.0)
        }
    }
}

/// A sine each particle runs with its own frequency and phase, between two values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Wave {
    frequency: (f32, f32),
    scale: (f32, f32),
    phase: (f32, f32),
}

impl Wave {
    fn parse(doc: &Value, frequency_max: f32) -> Self {
        Self {
            frequency: (number(doc, "frequencymin", 0.0), number(doc, "frequencymax", frequency_max)),
            scale: (number(doc, "scalemin", 0.0), number(doc, "scalemax", 1.0)),
            phase: (number(doc, "phasemin", 0.0), number(doc, "phasemax", TAU)),
        }
    }

    /// The frequency a particle chose; `salt` tells the operator and axis apart.
    fn frequency(&self, seed: u32, salt: u32) -> f32 {
        mix(self.frequency, fixed_unit(seed, salt))
    }

    /// The angle of a particle's sine at `age`, given the frequency it chose.
    fn angle(&self, seed: u32, salt: u32, frequency: f32, age: f32) -> f32 {
        frequency * age + mix(self.phase, fixed_unit(seed, salt ^ 0x5bd1_e995))
    }

    /// The multiplier at `age`: it swings between the two scale values.
    fn factor(&self, seed: u32, salt: u32, age: f32) -> f32 {
        mix(self.scale, 0.5 + 0.5 * noise::sin(self.angle(seed, salt, self.frequency(seed, salt), age)))
    }
}

/// The point `t` of the way through a pair.
fn mix(pair: (f32, f32), t: f32) -> f32 {
    pair.0 + (pair.1 - pair.0) * t
}

/// The vortex flag "infinite axis": distance is measured from the axis, not the centre.
const VORTEX_INFINITE_AXIS: u32 = 1;

/// One operator of a system.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Operator {
    /// `movement`: velocity moves the particle; gravity and drag change the velocity.
    Movement { gravity: Vec3, drag: f32 },
    /// `angularmovement`: the same for rotation.
    AngularMovement { force: Vec3, drag: f32 },
    /// `alphafade`: fades in until `fade_in` of the life, out from `fade_out` of it.
    AlphaFade { fade_in: f32, fade_out: f32 },
    /// `sizechange`.
    SizeChange(Ramp<f32>),
    /// `alphachange`.
    AlphaChange(Ramp<f32>),
    /// `colorchange`.
    ColorChange(Ramp<Vec3>),
    /// `oscillatealpha`.
    OscillateAlpha(Wave),
    /// `oscillatesize`.
    OscillateSize(Wave),
    /// `oscillateposition`: a sway on each axis the mask lets through.
    OscillatePosition { wave: Wave, mask: Vec3 },
    /// `turbulence`: a force that follows a smooth random field.
    Turbulence { speed: (f32, f32), phase: (f32, f32), scale: f32, time_scale: f32, mask: Vec3 },
    /// `vortex`: a force around an axis through a control point.
    Vortex {
        control_point: usize,
        offset: Vec3,
        axis: Vec3,
        inner: f32,
        outer: f32,
        speed_inner: f32,
        speed_outer: f32,
        infinite_axis: bool,
    },
    /// `controlpointattract`: a pull towards a control point (a push when negative)
    /// on particles nearer than `threshold`.
    Attract { control_point: usize, origin: Vec3, strength: f32, threshold: f32 },
}

impl Operator {
    /// Reads an operator; `None` for a kind this engine does not have.
    pub(crate) fn parse(doc: &Value) -> Option<Self> {
        let ramp = |start: f32, end: f32| Ramp {
            start_time: number(doc, "starttime", 0.0),
            end_time: number(doc, "endtime", 1.0),
            start_value: number(doc, "startvalue", start),
            end_value: number(doc, "endvalue", end),
        };
        Some(match doc.get("name").as_str()? {
            "movement" => {
                Self::Movement { gravity: vec3(doc, "gravity", [0.0; 3]), drag: number(doc, "drag", 0.0) }
            }
            "angularmovement" => {
                Self::AngularMovement { force: vec3(doc, "force", [0.0; 3]), drag: number(doc, "drag", 0.0) }
            }
            "alphafade" => Self::AlphaFade {
                fade_in: number(doc, "fadeintime", 0.1),
                fade_out: number(doc, "fadeouttime", 0.9),
            },
            "sizechange" => Self::SizeChange(ramp(1.0, 0.0)),
            "alphachange" => Self::AlphaChange(ramp(1.0, 0.0)),
            "colorchange" => Self::ColorChange(Ramp {
                start_time: number(doc, "starttime", 0.0),
                end_time: number(doc, "endtime", 1.0),
                start_value: vec3(doc, "startvalue", [1.0; 3]),
                end_value: vec3(doc, "endvalue", [1.0; 3]),
            }),
            "oscillatealpha" => Self::OscillateAlpha(Wave::parse(doc, 10.0)),
            "oscillatesize" => Self::OscillateSize(Wave::parse(doc, 10.0)),
            "oscillateposition" => Self::OscillatePosition {
                wave: Wave::parse(doc, 5.0),
                mask: vec3(doc, "mask", [1.0, 1.0, 0.0]),
            },
            "turbulence" => Self::Turbulence {
                speed: (number(doc, "speedmin", 500.0), number(doc, "speedmax", 1000.0)),
                phase: (number(doc, "phasemin", 0.0), number(doc, "phasemax", 0.0)),
                scale: number(doc, "scale", 0.01),
                time_scale: number(doc, "timescale", 20.0),
                mask: vec3(doc, "mask", [1.0, 1.0, 0.0]),
            },
            "vortex" => Self::Vortex {
                control_point: index(doc, "controlpoint", 0, 7),
                offset: vec3(doc, "offset", [0.0; 3]),
                axis: direction(vec3(doc, "axis", [0.0, 0.0, 1.0])).unwrap_or([0.0, 0.0, 1.0]),
                inner: number(doc, "distanceinner", 500.0),
                outer: number(doc, "distanceouter", 650.0),
                speed_inner: number(doc, "speedinner", 2500.0),
                speed_outer: number(doc, "speedouter", 0.0),
                infinite_axis: number(doc, "flags", 0.0) as u32 & VORTEX_INFINITE_AXIS != 0,
            },
            "controlpointattract" => Self::Attract {
                control_point: index(doc, "controlpoint", 0, 7),
                origin: vec3(doc, "origin", [0.0; 3]),
                strength: number(doc, "scale", 1.0),
                threshold: number(doc, "threshold", 64.0),
            },
            _ => return None,
        })
    }

    /// Applies the operator to every particle for one step. `salt` is the operator's
    /// place in the system, so two operators of a kind make different random choices.
    pub(crate) fn apply(&self, salt: u32, particles: &mut [Particle], frame: &Frame<'_>) {
        let dt = frame.dt;
        match self {
            Self::Movement { gravity, drag } => {
                let push = scale(*gravity, frame.force * dt);
                let keep = (-drag * dt).exp();
                for p in particles {
                    p.velocity = scale(add(p.velocity, push), keep);
                    p.position = add(p.position, scale(p.velocity, dt));
                }
            }
            Self::AngularMovement { force, drag } => {
                let push = scale(*force, dt);
                let keep = (-drag * dt).exp();
                for p in particles {
                    p.angular_velocity = scale(add(p.angular_velocity, push), keep);
                    p.rotation = add(p.rotation, scale(p.angular_velocity, dt));
                }
            }
            Self::AlphaFade { fade_in, fade_out } => {
                for p in particles {
                    let life = p.life();
                    if life < *fade_in {
                        p.now_alpha *= life / fade_in;
                    } else if life > *fade_out {
                        p.now_alpha *= ((1.0 - life) / (1.0 - fade_out).max(1e-6)).clamp(0.0, 1.0);
                    }
                }
            }
            Self::SizeChange(ramp) => {
                for p in particles {
                    p.now_size *= mix((ramp.start_value, ramp.end_value), ramp.progress(p.life()));
                }
            }
            Self::AlphaChange(ramp) => {
                for p in particles {
                    p.now_alpha *= mix((ramp.start_value, ramp.end_value), ramp.progress(p.life()));
                }
            }
            Self::ColorChange(ramp) => {
                for p in particles {
                    let t = ramp.progress(p.life());
                    for channel in 0..3 {
                        p.now_color[channel] *= mix((ramp.start_value[channel], ramp.end_value[channel]), t);
                    }
                }
            }
            Self::OscillateAlpha(wave) => {
                for p in particles {
                    p.now_alpha *= wave.factor(p.seed, salt, p.age);
                }
            }
            Self::OscillateSize(wave) => {
                for p in particles {
                    p.now_size *= wave.factor(p.seed, salt, p.age);
                }
            }
            Self::OscillatePosition { wave, mask } => {
                for axis in (0..3u32).filter(|&axis| mask[axis as usize] != 0.0) {
                    let axis_salt = salt.wrapping_add((axis + 1) << 8);
                    let reach = mask[axis as usize];
                    for p in particles.iter_mut() {
                        // The sway is added as the distance the sine covers in this step,
                        // so it composes with every other motion instead of overriding it.
                        let frequency = wave.frequency(p.seed, axis_salt);
                        let amplitude = mix(wave.scale, fixed_unit(p.seed, axis_salt ^ 0x1b87_3593));
                        let angle = wave.angle(p.seed, axis_salt, frequency, p.age - dt * 0.5);
                        p.position[axis as usize] += reach * amplitude * frequency * noise::cos(angle) * dt;
                    }
                }
            }
            Self::Turbulence { speed, phase, scale: field_scale, time_scale, mask } => {
                let drift = frame.clock * time_scale * field_scale;
                for p in particles {
                    let offset = mix(*phase, fixed_unit(p.seed, salt));
                    let at = [
                        p.position[0] * field_scale + offset,
                        p.position[1] * field_scale + offset,
                        p.position[2] * field_scale + drift,
                    ];
                    let push = mix(*speed, fixed_unit(p.seed, salt ^ 0x68e3_1da4)) * frame.force * dt;
                    let field = noise::vector(at);
                    for axis in 0..3 {
                        p.velocity[axis] += field[axis] * mask[axis] * push;
                    }
                }
            }
            Self::Vortex {
                control_point,
                offset,
                axis,
                inner,
                outer,
                speed_inner,
                speed_outer,
                infinite_axis,
            } => {
                let point = frame.control_points[*control_point];
                if !point.active {
                    return;
                }
                let center = add(point.position, *offset);
                for p in particles {
                    let mut arm = sub(p.position, center);
                    if *infinite_axis {
                        arm = sub(arm, scale(*axis, dot(arm, *axis)));
                    }
                    let distance = length(arm);
                    let speed = if distance <= *inner {
                        *speed_inner
                    } else if distance >= *outer {
                        *speed_outer
                    } else {
                        mix((*speed_inner, *speed_outer), (distance - inner) / (outer - inner))
                    };
                    if let Some(around) = direction(cross(*axis, arm)) {
                        p.velocity = add(p.velocity, scale(around, speed * frame.force * dt));
                    }
                }
            }
            Self::Attract { control_point, origin, strength, threshold } => {
                let point = frame.control_points[*control_point];
                if !point.active {
                    return;
                }
                let center = add(point.position, *origin);
                let push = strength * frame.force * dt;
                for p in particles {
                    let towards = sub(center, p.position);
                    let distance = length(towards);
                    if distance < *threshold && distance > 1e-4 {
                        p.velocity = add(p.velocity, scale(towards, push / distance));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(json: &str, mut particle: Particle, steps: usize, points: &[ControlPoint; 8]) -> Particle {
        let operator = Operator::parse(&wepaper_json::parse(json).unwrap()).unwrap();
        for step in 0..steps {
            particle.now_alpha = particle.alpha;
            particle.now_size = particle.size;
            particle.now_color = particle.color;
            let frame = Frame { dt: 0.01, clock: step as f32 * 0.01, control_points: points, force: 1.0 };
            operator.apply(3, std::slice::from_mut(&mut particle), &frame);
            particle.age += 0.01;
        }
        particle
    }

    fn at_life(life: f32) -> Particle {
        let mut particle = Particle::newborn(77, 0, 0);
        particle.lifetime = 2.0;
        particle.age = life * 2.0;
        particle
    }

    const REST: [ControlPoint; 8] = [ControlPoint { position: [0.0; 3], active: true }; 8];

    #[test]
    fn ramps_hold_their_ends_and_cross_between() {
        let ramp = r#"{"name":"sizechange","starttime":0.2,"endtime":0.6,"startvalue":2,"endvalue":0.5}"#;
        assert_eq!(run(ramp, at_life(0.1), 1, &REST).now_size, 40.0);
        assert!((run(ramp, at_life(0.4), 1, &REST).now_size - 25.0).abs() < 1e-3);
        assert_eq!(run(ramp, at_life(0.9), 1, &REST).now_size, 10.0);
        assert_eq!(run(r#"{"name":"sizechange"}"#, at_life(0.25), 1, &REST).now_size, 15.0);
        assert_eq!(
            run(r#"{"name":"alphachange","starttime":0.5,"endtime":0.5}"#, at_life(0.4), 1, &REST).now_alpha,
            1.0
        );
        assert_eq!(
            run(r#"{"name":"alphachange","starttime":0.5,"endtime":0.5}"#, at_life(0.6), 1, &REST).now_alpha,
            0.0
        );
        let color = run(
            r#"{"name":"colorchange","startvalue":"1 0.5 0","endvalue":"0 0.5 1"}"#,
            at_life(0.5),
            1,
            &REST,
        )
        .now_color;
        assert!(
            (color[0] - 0.5).abs() < 1e-4 && (color[1] - 0.5).abs() < 1e-4 && (color[2] - 0.5).abs() < 1e-4
        );
    }

    #[test]
    fn waves_stay_between_their_scales_and_differ_per_particle() {
        let wave =
            r#"{"name":"oscillatealpha","frequencymin":5,"frequencymax":9,"scalemin":0.2,"scalemax":0.8}"#;
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        for step in 0..400 {
            let alpha = run(wave, at_life(step as f32 / 400.0), 1, &REST).now_alpha;
            assert!((0.199..=0.801).contains(&alpha));
            low = low.min(alpha);
            high = high.max(alpha);
        }
        assert!(low < 0.25 && high > 0.75);
        let mut other = at_life(0.3);
        other.seed = 78;
        assert_ne!(run(wave, at_life(0.3), 1, &REST).now_alpha, run(wave, other, 1, &REST).now_alpha);
    }

    #[test]
    fn sway_returns_and_respects_the_mask() {
        let sway = r#"{"name":"oscillateposition","frequencymin":3,"frequencymax":3,"scalemin":10,"scalemax":10,"phasemin":0,"phasemax":0,"mask":"1 0 0"}"#;
        // A quarter period of sin(3t) from zero phase reaches the full amplitude (the
        // helper ages after the step, half a step off the simulation, hence the slack).
        let quarter = run(sway, Particle::newborn(5, 0, 0), 52, &REST);
        assert!(
            (quarter.position[0] - 10.0).abs() < 0.4 && quarter.position[1] == 0.0,
            "{:?}",
            quarter.position
        );
        let full = run(sway, Particle::newborn(5, 0, 0), 209, &REST);
        assert!(full.position[0].abs() < 0.4, "{:?}", full.position);
    }

    #[test]
    fn vortex_turns_around_its_axis_and_picks_speed_by_distance() {
        let vortex = r#"{"name":"vortex","distanceinner":10,"distanceouter":20,"speedinner":100,"speedouter":-50,"offset":"0 5 0"}"#;
        let mut particle = Particle::newborn(1, 0, 0);
        particle.position = [5.0, 5.0, 0.0];
        assert!(
            (run(vortex, particle, 1, &REST).velocity[1] - 1.0).abs() < 1e-4,
            "inside: counter-clockwise at the inner speed"
        );
        particle.position = [30.0, 5.0, 0.0];
        assert!((run(vortex, particle, 1, &REST).velocity[1] + 0.5).abs() < 1e-4);
        particle.position = [15.0, 5.0, 0.0];
        assert!((run(vortex, particle, 1, &REST).velocity[1] - 0.25).abs() < 1e-4);
        particle.position = [0.0, 5.0, 0.0];
        assert_eq!(run(vortex, particle, 1, &REST).velocity, [0.0; 3]);
    }

    #[test]
    fn forces_tied_to_an_absent_pointer_rest() {
        let mut points = REST;
        points[1] = ControlPoint { position: [0.0; 3], active: false };
        let mut particle = Particle::newborn(1, 0, 0);
        particle.position = [5.0, 0.0, 0.0];
        assert_eq!(
            run(r#"{"name":"controlpointattract","controlpoint":1,"scale":-500}"#, particle, 3, &points)
                .velocity,
            [0.0; 3]
        );
        assert_eq!(
            run(
                r#"{"name":"vortex","controlpoint":1,"distanceinner":0,"distanceouter":0,"speedouter":9}"#,
                particle,
                3,
                &points
            )
            .velocity,
            [0.0; 3]
        );
    }

    #[test]
    fn unknown_kinds_are_not_operators() {
        assert!(Operator::parse(&wepaper_json::parse(r#"{"name":"boids"}"#).unwrap()).is_none());
    }
}
