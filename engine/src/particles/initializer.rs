//! Initializers: what a particle is born with.

use std::f32::consts::{PI, TAU};

use wepaper_json::Value;

use super::noise;
use super::particle::{ControlPoint, Particle};
use super::rng::Rng;
use super::value::{Vec3, add, cross, direction, dot, index, lerp, number, numbers, scale, sub, vec3};

/// What an initializer may look at besides the particle.
pub(crate) struct Birth<'a> {
    /// The system's random stream.
    pub(crate) rng: &'a mut Rng,
    /// Seconds the system has run.
    pub(crate) clock: f32,
    /// How many particles the source has made before this one.
    pub(crate) sequence: u32,
    /// The control points this frame.
    pub(crate) control_points: &'a [ControlPoint; 8],
}

/// A range a scalar is drawn from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Range {
    min: f32,
    max: f32,
    exponent: f32,
}

impl Range {
    fn parse(doc: &Value, min: f32, max: f32) -> Self {
        Self {
            min: number(doc, "min", min),
            max: number(doc, "max", max),
            exponent: number(doc, "exponent", 1.0),
        }
    }

    fn draw(&self, rng: &mut Rng) -> f32 {
        rng.between(self.min, self.max, self.exponent)
    }

    /// The largest value the range can give.
    pub(crate) fn upper(&self) -> f32 {
        self.min.max(self.max)
    }
}

/// A box a vector is drawn from, each axis on its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Span {
    min: Vec3,
    max: Vec3,
    exponent: f32,
}

impl Span {
    fn parse(doc: &Value, min: Vec3, max: Vec3) -> Self {
        Self {
            min: vec3(doc, "min", min),
            max: vec3(doc, "max", max),
            exponent: number(doc, "exponent", 1.0),
        }
    }

    fn draw(&self, rng: &mut Rng) -> Vec3 {
        [0, 1, 2].map(|axis| rng.between(self.min[axis], self.max[axis], self.exponent))
    }
}

/// One initializer of a system.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Initializer {
    /// `lifetimerandom`: seconds to live.
    Lifetime(Range),
    /// `sizerandom`.
    Size(Range),
    /// `alpharandom`.
    Alpha(Range),
    /// `velocityrandom`: added to the velocity.
    Velocity(Span),
    /// `rotationrandom`, radians.
    Rotation(Span),
    /// `angularvelocityrandom`, radians per second.
    AngularVelocity(Span),
    /// `colorrandom`: one point on the line between two colours (0 to 255 in the document).
    Color { min: Vec3, max: Vec3, exponent: f32 },
    /// `colorlist`: one of a few colours (0 to 1 in the document), each as likely.
    /// Its `huenoise` is not applied.
    ColorList(Vec<Vec3>),
    /// `hsvcolorrandom`: hue in turns, saturation and value 0 to 1.
    HsvColor { hue: (f32, f32), saturation: (f32, f32), value: (f32, f32) },
    /// `turbulentvelocityrandom`: a speed along a heading that wanders with time, so
    /// particles born one after another leave in a winding stream.
    TurbulentVelocity {
        speed: (f32, f32),
        phase: (f32, f32),
        spread: f32,
        time_scale: f32,
        offset: f32,
        forward: Vec3,
        normal: Vec3,
    },
    /// `mapsequencearoundcontrolpoint`: successive particles go around a control
    /// point, `count` to a turn.
    AroundControlPoint { control_point: usize, count: f32, axis: Vec3, speed_min: Vec3, speed_max: Vec3 },
    /// `mapsequencebetweencontrolpoints`: successive particles step from one control
    /// point to another, `count` to the stretch.
    BetweenControlPoints { start: usize, end: usize, count: f32 },
}

impl Initializer {
    /// Reads an initializer; `None` for a kind this engine does not have.
    pub(crate) fn parse(doc: &Value) -> Option<Self> {
        Some(match doc.get("name").as_str()? {
            "lifetimerandom" => Self::Lifetime(Range::parse(doc, 0.0, 1.0)),
            "sizerandom" => Self::Size(Range::parse(doc, 0.0, 20.0)),
            "alpharandom" => Self::Alpha(Range::parse(doc, 0.05, 1.0)),
            "velocityrandom" => Self::Velocity(Span::parse(doc, [-32.0, -32.0, 0.0], [32.0, 32.0, 0.0])),
            "rotationrandom" => Self::Rotation(Span::parse(doc, [0.0; 3], [0.0, 0.0, TAU])),
            "angularvelocityrandom" => {
                Self::AngularVelocity(Span::parse(doc, [0.0, 0.0, -5.0], [0.0, 0.0, 5.0]))
            }
            "colorrandom" => Self::Color {
                min: scale(vec3(doc, "min", [0.0; 3]), 1.0 / 255.0),
                max: scale(vec3(doc, "max", [255.0; 3]), 1.0 / 255.0),
                exponent: number(doc, "exponent", 1.0),
            },
            "colorlist" => {
                let colors: Vec<Vec3> = doc
                    .get("colors")
                    .items()
                    .filter_map(|color| match numbers(color) {
                        (_, 0) => None,
                        (values, _) => Some(values),
                    })
                    .collect();
                if colors.is_empty() {
                    return None;
                }
                Self::ColorList(colors)
            }
            "hsvcolorrandom" => {
                let (hue_min, hue_max) = (number(doc, "huemin", 0.0), number(doc, "huemax", 1.0));
                // Older documents count hue in degrees.
                let turns = if hue_min.abs().max(hue_max.abs()) > 1.0 { 1.0 / 360.0 } else { 1.0 };
                Self::HsvColor {
                    hue: (hue_min * turns, hue_max * turns),
                    saturation: (number(doc, "saturationmin", 0.0), number(doc, "saturationmax", 1.0)),
                    value: (number(doc, "valuemin", 0.0), number(doc, "valuemax", 1.0)),
                }
            }
            "turbulentvelocityrandom" => Self::TurbulentVelocity {
                speed: (number(doc, "speedmin", 100.0), number(doc, "speedmax", 250.0)),
                phase: (number(doc, "phasemin", 0.0), number(doc, "phasemax", 0.1)),
                spread: number(doc, "scale", 1.0),
                time_scale: number(doc, "timescale", 1.0),
                offset: number(doc, "offset", 0.0),
                forward: direction(vec3(doc, "forward", [0.0, 1.0, 0.0])).unwrap_or([0.0, 1.0, 0.0]),
                normal: direction(vec3(doc, "right", [0.0, 0.0, 1.0])).unwrap_or([0.0, 0.0, 1.0]),
            },
            "mapsequencearoundcontrolpoint" => Self::AroundControlPoint {
                control_point: index(doc, "controlpoint", 0, 7),
                count: number(doc, "count", 1.0),
                axis: direction(vec3(doc, "axis", [0.0, 0.0, 1.0])).unwrap_or([0.0, 0.0, 1.0]),
                speed_min: vec3(doc, "speedmin", [0.0; 3]),
                speed_max: vec3(doc, "speedmax", [0.0; 3]),
            },
            "mapsequencebetweencontrolpoints" => Self::BetweenControlPoints {
                start: index(doc, "controlpointstart", 0, 7),
                end: index(doc, "controlpointend", 1, 7),
                count: number(doc, "count", 1.0),
            },
            _ => return None,
        })
    }

    /// The longest life this initializer can give, if it gives one.
    pub(crate) fn longest_life(&self) -> Option<f32> {
        match self {
            Self::Lifetime(range) => Some(range.upper()),
            _ => None,
        }
    }

    /// Sets what this initializer is about on a newborn particle.
    pub(crate) fn apply(&self, particle: &mut Particle, birth: &mut Birth<'_>) {
        match self {
            Self::Lifetime(range) => particle.lifetime = range.draw(birth.rng),
            Self::Size(range) => particle.size = range.draw(birth.rng),
            Self::Alpha(range) => particle.alpha = range.draw(birth.rng),
            Self::Velocity(span) => particle.velocity = add(particle.velocity, span.draw(birth.rng)),
            Self::Rotation(span) => particle.rotation = span.draw(birth.rng),
            Self::AngularVelocity(span) => particle.angular_velocity = span.draw(birth.rng),
            Self::Color { min, max, exponent } => {
                particle.color = lerp(*min, *max, birth.rng.between(0.0, 1.0, *exponent))
            }
            Self::ColorList(colors) => {
                let pick = birth.rng.between(0.0, colors.len() as f32, 1.0) as usize;
                particle.color = colors[pick.min(colors.len() - 1)];
            }
            Self::HsvColor { hue, saturation, value } => {
                let rng = &mut *birth.rng;
                particle.color = hsv(
                    rng.between(hue.0, hue.1, 1.0),
                    rng.between(saturation.0, saturation.1, 1.0),
                    rng.between(value.0, value.1, 1.0),
                );
            }
            Self::TurbulentVelocity { speed, phase, spread, time_scale, offset, forward, normal } => {
                let at = birth.clock * time_scale + birth.rng.between(phase.0, phase.1, 1.0);
                let angle = offset + noise::vector([at, 0.37, 0.71])[0] * spread * PI;
                let heading = rotate(*forward, *normal, angle);
                particle.velocity =
                    add(particle.velocity, scale(heading, birth.rng.between(speed.0, speed.1, 1.0)));
            }
            Self::AroundControlPoint { control_point, count, axis, speed_min, speed_max } => {
                let center = birth.control_points[*control_point].position;
                let offset = sub(particle.position, center);
                let along = dot(offset, *axis);
                let radius = super::value::length(sub(offset, scale(*axis, along)));
                let side = direction(cross(*axis, [1.0, 0.0, 0.0]))
                    .or_else(|| direction(cross(*axis, [0.0, 1.0, 0.0])))
                    .unwrap_or([0.0, 1.0, 0.0]);
                let other = cross(*axis, side);
                let angle = if count.abs() > 1e-6 { TAU * birth.sequence as f32 / count } else { 0.0 };
                let outwards = add(scale(side, angle.cos()), scale(other, angle.sin()));
                particle.position = add(center, add(scale(*axis, along), scale(outwards, radius)));
                let speed = lerp(*speed_min, *speed_max, birth.rng.unit());
                let around = cross(*axis, outwards);
                particle.velocity = add(
                    particle.velocity,
                    add(add(scale(outwards, speed[0]), scale(around, speed[1])), scale(*axis, speed[2])),
                );
            }
            Self::BetweenControlPoints { start, end, count } => {
                let steps = count.max(1.0);
                let along = (birth.sequence as f32 % steps) / steps;
                let stretch = sub(birth.control_points[*end].position, birth.control_points[*start].position);
                particle.position = add(particle.position, scale(stretch, along));
            }
        }
    }
}

/// `v` turned by `angle` radians around the unit vector `axis`.
fn rotate(v: Vec3, axis: Vec3, angle: f32) -> Vec3 {
    let (sin, cos) = angle.sin_cos();
    add(add(scale(v, cos), scale(cross(axis, v), sin)), scale(axis, dot(axis, v) * (1.0 - cos)))
}

/// The colour of a hue in turns with a saturation and a value in 0 to 1.
fn hsv(hue: f32, saturation: f32, value: f32) -> Vec3 {
    let sector = hue.rem_euclid(1.0) * 6.0;
    let channel = |shift: f32| {
        let k = (shift + sector).rem_euclid(6.0);
        value - value * saturation * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [channel(5.0), channel(3.0), channel(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn born(json: &str, sequence: u32, points: &[ControlPoint; 8]) -> Particle {
        let initializer = Initializer::parse(&wepaper_json::parse(json).unwrap()).unwrap();
        let mut rng = Rng::new(11 + sequence as u64);
        let mut particle = Particle::newborn(1, u64::from(sequence), 0);
        particle.position = [10.0, 0.0, 0.0];
        initializer
            .apply(&mut particle, &mut Birth { rng: &mut rng, clock: 1.5, sequence, control_points: points });
        particle
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3)
    }

    #[test]
    fn a_colour_list_gives_each_particle_one_of_its_colours() {
        let points = [ControlPoint::default(); 8];
        let json = r#"{"name":"colorlist","colors":["0.5 0.9 1","1 0.6 0.8","1 1 1"],"huenoise":0}"#;
        let mut seen = [false; 3];
        for sequence in 0..60 {
            let color = born(json, sequence, &points).color;
            let which = [[0.5, 0.9, 1.0], [1.0, 0.6, 0.8], [1.0; 3]]
                .iter()
                .position(|listed| close(color, *listed))
                .unwrap_or_else(|| panic!("{color:?} is not one of the list"));
            seen[which] = true;
        }
        assert_eq!(seen, [true; 3], "every colour comes up");
        assert!(
            Initializer::parse(&wepaper_json::parse(r#"{"name":"colorlist","colors":[]}"#).unwrap())
                .is_none()
        );
    }

    #[test]
    fn ranges_set_their_field_within_bounds() {
        let points = [ControlPoint::default(); 8];
        for sequence in 0..50 {
            assert!(
                (3.0..=5.0).contains(
                    &born(r#"{"name":"lifetimerandom","min":3,"max":5}"#, sequence, &points).lifetime
                )
            );
            assert!((50.0..=200.0).contains(
                &born(r#"{"name":"sizerandom","min":50,"max":200,"exponent":2}"#, sequence, &points).size
            ));
            assert!(
                (0.2..=0.4).contains(
                    &born(r#"{"name":"alpharandom","min":0.2,"max":0.4}"#, sequence, &points).alpha
                )
            );
            let moving =
                born(r#"{"name":"velocityrandom","min":"-50 10 0","max":"50 20 0"}"#, sequence, &points);
            assert!(
                moving.velocity[0].abs() <= 50.0
                    && (10.0..=20.0).contains(&moving.velocity[1])
                    && moving.velocity[2] == 0.0
            );
            let turned = born(r#"{"name":"rotationrandom"}"#, sequence, &points);
            assert!(turned.rotation[0] == 0.0 && (0.0..=TAU).contains(&turned.rotation[2]));
            assert_eq!(
                born(r#"{"name":"rotationrandom","min":-0.4,"max":-0.4}"#, sequence, &points).rotation,
                [-0.4, 0.0, 0.0]
            );
            assert!(
                born(r#"{"name":"angularvelocityrandom","min":"0 0 -2","max":"0 0 2"}"#, sequence, &points)
                    .angular_velocity[2]
                    .abs()
                    <= 2.0
            );
        }
    }

    #[test]
    fn colour_is_a_point_between_the_two_colours() {
        let points = [ControlPoint::default(); 8];
        for sequence in 0..50 {
            let color =
                born(r#"{"name":"colorrandom","min":"255 0 0","max":"0 0 255"}"#, sequence, &points).color;
            assert!(color[1] == 0.0 && (color[0] + color[2] - 1.0).abs() < 1e-4, "{color:?}");
        }
        assert_eq!(born(r#"{"name":"colorrandom","min":"255 255 255"}"#, 0, &points).color, [1.0; 3]);
        assert!(
            close(hsv(0.0, 1.0, 1.0), [1.0, 0.0, 0.0]) && close(hsv(1.0 / 3.0, 1.0, 1.0), [0.0, 1.0, 0.0])
        );
        assert!(close(hsv(2.0 / 3.0, 1.0, 0.5), [0.0, 0.0, 0.5]) && close(hsv(0.2, 0.0, 0.7), [0.7; 3]));
        let pure = born(
            r#"{"name":"hsvcolorrandom","huemin":0.5,"huemax":0.5,"saturationmin":1,"valuemin":1}"#,
            0,
            &points,
        )
        .color;
        assert!(close(pure, [0.0, 1.0, 1.0]));
    }

    #[test]
    fn turbulent_velocity_has_the_asked_speed_and_keeps_forward_without_spread() {
        let points = [ControlPoint::default(); 8];
        let straight =
            born(r#"{"name":"turbulentvelocityrandom","scale":0,"speedmin":80,"speedmax":80}"#, 0, &points);
        assert!(close(straight.velocity, [0.0, 80.0, 0.0]));
        for sequence in 0..30 {
            let winding = born(
                r#"{"name":"turbulentvelocityrandom","scale":2,"speedmin":40,"speedmax":60,"phasemax":6}"#,
                sequence,
                &points,
            );
            let speed = super::super::value::length(winding.velocity);
            assert!((40.0..=60.01).contains(&speed) && winding.velocity[2].abs() < 1e-3);
        }
    }

    #[test]
    fn sequences_go_around_and_between_control_points() {
        let mut points = [ControlPoint::default(); 8];
        points[1].position = [0.0, 500.0, 0.0];
        let around =
            r#"{"name":"mapsequencearoundcontrolpoint","count":4,"speedmin":"0 100 0","speedmax":"0 100 0"}"#;
        let (first, second) = (born(around, 0, &points), born(around, 1, &points));
        assert!((super::super::value::length(first.position) - 10.0).abs() < 1e-3);
        assert!(dot(first.position, second.position).abs() < 1e-2, "a quarter turn apart");
        assert!(
            dot(first.position, first.velocity).abs() < 1e-2
                && (super::super::value::length(first.velocity) - 100.0).abs() < 1e-2
        );
        let between = r#"{"name":"mapsequencebetweencontrolpoints","count":4}"#;
        assert!(close(born(between, 0, &points).position, [10.0, 0.0, 0.0]));
        assert!(close(born(between, 3, &points).position, [10.0, 375.0, 0.0]));
        assert!(close(born(between, 4, &points).position, [10.0, 0.0, 0.0]));
    }

    #[test]
    fn unknown_kinds_are_not_initializers() {
        assert!(Initializer::parse(&wepaper_json::parse(r#"{"name":"colorlist"}"#).unwrap()).is_none());
        assert!(Initializer::parse(&wepaper_json::parse(r#"{"min":1}"#).unwrap()).is_none());
    }
}
