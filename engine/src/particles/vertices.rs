//! The vertices that draw a system: sprites for the stock `genericparticle` shader,
//! ribbons for `genericropeparticle`. The module documentation has the contract.

use super::def::{AnimationMode, Renderer};
use super::particle::Particle;
use super::rng::fixed_unit;
use super::sim::ParticleSystem;
use super::value::{Vec3, add, cross, direction, length, lerp, sub};
use super::{Batch, View};

/// `a_Position`, `a_TexCoordVec4`, `a_Color`, `a_TexCoordVec4C1`, `a_TexCoordC2`.
const SPRITE_LAYOUT: &[(u32, i32)] = &[(0, 3), (3, 4), (2, 4), (4, 4), (6, 2)];
/// `a_PositionVec4`, `a_TexCoordVec4`, `a_TexCoordVec4C1`, `a_TexCoordVec4C2`,
/// `a_TexCoordVec4C3`, `a_TexCoordC4`, `a_Color`.
const ROPE_LAYOUT: &[(u32, i32)] =
    &[(12, 4), (3, 4), (4, 4), (5, 4), (super::A_TEXCOORDVEC4C3, 4), (super::A_TEXCOORDC4, 2), (2, 4)];

const SPRITE_STRIDE: usize = 17;
const ROPE_STRIDE: usize = 26;

const SPRITE_COMBOS: &[(&str, i32)] = &[("GS_ENABLED", 0), ("THICKFORMAT", 1)];
const TRAIL_COMBOS: &[(&str, i32)] = &[("GS_ENABLED", 0), ("THICKFORMAT", 1), ("TRAILRENDERER", 1)];

/// The corners of the two triangles of a quad, as (across, along).
const CORNERS: [(f32, f32); 6] = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

/// The salt of a particle's random sprite sheet frame.
const FRAME_SALT: u32 = 0xf4a3_e001;

/// A point a ribbon passes through.
#[derive(Clone, Copy)]
struct Knot {
    at: Vec3,
    /// Half the ribbon's width here.
    half: f32,
    color: [f32; 4],
}

impl ParticleSystem {
    /// Appends this frame's vertices (interleaved `f32`) to `out` and says how to
    /// draw them. Children are separate systems with their own batches.
    pub fn write_vertices(&self, view: &View, out: &mut Vec<f32>) -> Batch {
        let renderer = self.def.renderer;
        let before = out.len();
        let (layout, stride, shader, combos, render_var0) = match renderer.kind {
            Renderer::Sprite => {
                self.sprites(false, out);
                (SPRITE_LAYOUT, SPRITE_STRIDE, "genericparticle", SPRITE_COMBOS, [0.0; 4])
            }
            Renderer::SpriteTrail => {
                self.sprites(true, out);
                (
                    SPRITE_LAYOUT,
                    SPRITE_STRIDE,
                    "genericparticle",
                    TRAIL_COMBOS,
                    [renderer.length, renderer.max_length, renderer.min_length, 0.0],
                )
            }
            Renderer::Rope => {
                self.rope(view, out);
                (ROPE_LAYOUT, ROPE_STRIDE, "genericropeparticle", SPRITE_COMBOS, [0.0; 4])
            }
            Renderer::RopeTrail => {
                let elapsed = self.rope_trails(view, out);
                let most = self.trail_points as f32;
                (ROPE_LAYOUT, ROPE_STRIDE, "genericropeparticle", TRAIL_COMBOS, [most, 0.0, elapsed, most])
            }
        };
        Batch {
            vertices: (out.len() - before) / stride,
            layout,
            renderer: renderer.kind,
            combos,
            shader,
            render_var0,
        }
    }

    /// The colour a particle is drawn with, the instance's tint and opacity applied.
    fn color_of(&self, particle: &Particle) -> [f32; 4] {
        let instance = &self.def.instance;
        [
            (particle.now_color[0] * instance.color[0]).max(0.0),
            (particle.now_color[1] * instance.color[1]).max(0.0),
            (particle.now_color[2] * instance.color[2]).max(0.0),
            (particle.now_alpha * instance.alpha).max(0.0),
        ]
    }

    /// Where in the sprite sheet a particle is, as the fraction of the sheet the
    /// shader turns into a frame.
    fn sheet_position(&self, particle: &Particle) -> f32 {
        match self.def.animation() {
            AnimationMode::Sequence => {
                let played = (particle.life() * self.def.sequence_multiplier()).rem_euclid(1.0);
                if played.is_finite() { played.min(0.9999) } else { 0.0 }
            }
            AnimationMode::RandomFrame => fixed_unit(particle.seed, FRAME_SALT),
            AnimationMode::Once => (particle.life() * self.def.sequence_multiplier()).clamp(0.0, 0.9999),
        }
    }

    /// Six vertices per particle. With `along_velocity` the shader builds the quad
    /// from the velocity, which then must have a direction.
    fn sprites(&self, along_velocity: bool, out: &mut Vec<f32>) {
        out.reserve(self.particles.len() * 6 * SPRITE_STRIDE);
        for particle in &self.particles {
            let at = add(self.anchor(particle), particle.position);
            let color = self.color_of(particle);
            // The shader spreads a quad one "size" across. Measured against Wallpaper
            // Engine's own pictures of wallpapers, a sprite is half its size across,
            // as a ribbon is half its size to each side.
            let size = particle.now_size.max(0.0) * 0.5;
            let velocity = if along_velocity && length(particle.velocity) < 1e-4 {
                [0.0, 1e-4, 0.0]
            } else {
                particle.velocity
            };
            let sheet = self.sheet_position(particle);
            for (across, along) in CORNERS {
                out.extend_from_slice(&[
                    at[0],
                    at[1],
                    at[2],
                    across,
                    along,
                    particle.rotation[2],
                    size,
                    color[0],
                    color[1],
                    color[2],
                    color[3],
                    velocity[0],
                    velocity[1],
                    velocity[2],
                    sheet,
                    particle.rotation[0],
                    particle.rotation[1],
                ]);
            }
        }
    }

    /// The particle at place `index` of rope order.
    fn roped(&self, index: usize) -> &Particle {
        match self.order.get(index) {
            Some(&at) => &self.particles[(at as usize).min(self.particles.len() - 1)],
            None => &self.particles[index],
        }
    }

    fn knot_of(&self, particle: &Particle) -> Knot {
        Knot {
            at: add(self.anchor(particle), particle.position),
            half: particle.now_size.max(0.0) * 0.5,
            color: self.color_of(particle),
        }
    }

    /// One ribbon per source through its particles in birth order, rounded with a
    /// Catmull-Rom spline cut into the renderer's subdivisions.
    fn rope(&self, view: &View, out: &mut Vec<f32>) {
        let total = self.particles.len();
        let pieces = self.def.renderer.subdivision.max(1) as usize;
        out.reserve(total * pieces * 6 * ROPE_STRIDE);
        let mut first = 0;
        while first < total {
            let source = self.roped(first).source;
            let mut end = first + 1;
            while end < total && self.roped(end).source == source {
                end += 1;
            }
            let particles = end - first;
            if particles >= 2 {
                let knots = (particles - 1) * pieces + 1;
                let knot = |index: usize| {
                    let (segment, step) = (index / pieces, index % pieces);
                    let at = |offset: isize| {
                        self.knot_of(self.roped(
                            first + (segment as isize + offset).clamp(0, particles as isize - 1) as usize,
                        ))
                    };
                    let (from, to) = (at(0), at(1));
                    if step == 0 {
                        return from;
                    }
                    let t = step as f32 / pieces as f32;
                    Knot {
                        at: catmull_rom(at(-1).at, from.at, to.at, at(2).at, t),
                        half: from.half + (to.half - from.half) * t,
                        color: [0, 1, 2, 3].map(|c| from.color[c] + (to.color[c] - from.color[c]) * t),
                    }
                };
                ribbon(out, view.forward, knots, knot, knots as f32);
            }
            first = end;
        }
    }

    /// One ribbon per particle from where it is back along the points it
    /// remembers. Returns how far the trails are towards recording their next
    /// point, 0 to 1.
    fn rope_trails(&self, view: &View, out: &mut Vec<f32>) -> f32 {
        let stride = self.trail_points;
        if stride == 0 {
            return 0.0;
        }
        let interval = self.def.renderer.length / stride as f32;
        let elapsed = (self.trail_clock / interval).clamp(0.0, 1.0);
        out.reserve(self.particles.len() * stride * 6 * ROPE_STRIDE);
        for (particle, points) in self.particles.iter().zip(self.trail.chunks_exact(stride)) {
            let recorded = (particle.trail as usize).min(stride);
            if recorded == 0 {
                continue;
            }
            let anchor = self.anchor(particle);
            let head = self.knot_of(particle);
            let knot = |index: usize| {
                if index == 0 {
                    return head;
                }
                let mut at = points[index - 1];
                if index == stride {
                    // A full trail gives up its oldest stretch gradually, as the head
                    // grows the newest, so the tail does not jump when a point drops.
                    let before = if index >= 2 { points[index - 2] } else { particle.position };
                    at = lerp(before, at, 1.0 - elapsed);
                }
                Knot { at: add(anchor, at), ..head }
            };
            ribbon(out, view.forward, recorded + 1, knot, recorded as f32);
        }
        elapsed
    }
}

/// A point on the Catmull-Rom spline through `b` and `c`, `t` of the way between them.
fn catmull_rom(a: Vec3, b: Vec3, c: Vec3, d: Vec3, t: f32) -> Vec3 {
    let (t2, t3) = (t * t, t * t * t);
    [0, 1, 2].map(|i| {
        0.5 * (2.0 * b[i]
            + (c[i] - a[i]) * t
            + (2.0 * a[i] - 5.0 * b[i] + 4.0 * c[i] - d[i]) * t2
            + (3.0 * b[i] - a[i] - 3.0 * c[i] + d[i]) * t3)
    })
}

/// Writes a ribbon through `knots` points; `length` is what the shader divides a
/// piece's number by to get its texture coordinate.
fn ribbon(out: &mut Vec<f32>, forward: Vec3, knots: usize, knot: impl Fn(usize) -> Knot, length: f32) {
    if knots < 2 {
        return;
    }
    let mut from = knot(0);
    let mut to = knot(1);
    let mut before = from.at;
    for piece in 0..knots - 1 {
        let after = if piece + 2 < knots { Some(knot(piece + 2)) } else { None };
        // The ribbon's side at a knot is square to the line through its neighbours,
        // so consecutive pieces meet without a gap.
        let tangents = (sub(to.at, before), sub(after.map_or(to.at, |knot| knot.at), from.at));
        write_piece(out, forward, (&from, &to), tangents, length, piece as f32);
        before = from.at;
        from = to;
        if let Some(after) = after {
            to = after;
        }
    }
}

/// Writes the six vertices of one piece of a ribbon, or nothing when the piece has
/// no length or lies along the view, where the shader would divide by zero.
fn write_piece(
    out: &mut Vec<f32>,
    forward: Vec3,
    (from, to): (&Knot, &Knot),
    (tangent_from, tangent_to): (Vec3, Vec3),
    length: f32,
    number: f32,
) {
    if direction(sub(to.at, from.at)).is_none() {
        return;
    }
    let (Some(side_from), Some(side_to)) =
        (direction(cross(forward, tangent_from)), direction(cross(forward, tangent_to)))
    else {
        return;
    };
    // The shader derives its two side vectors from `delta + (start - c1)` and
    // `delta - (end - c2)`; these make them the tangents above.
    let control_from = sub(to.at, tangent_from);
    let control_to = add(from.at, tangent_to);
    for (across, along) in CORNERS {
        let (side, half) = if along == 0.0 { (side_from, from.half) } else { (side_to, to.half) };
        // The stock shader offsets by `side * across * 2.0 - 1.0`: one-sided, and a
        // unit off on every axis. Shifting all four points of the vertex by the
        // opposite puts the ribbon centred on its line.
        let shift = [1.0 - side[0] * half, 1.0 - side[1] * half, 1.0 - side[2] * half];
        let (start, end) = (add(from.at, shift), add(to.at, shift));
        let (c1, c2) = (add(control_from, shift), add(control_to, shift));
        out.extend_from_slice(&[
            start[0],
            start[1],
            start[2],
            from.half,
            end[0],
            end[1],
            end[2],
            length,
            c1[0],
            c1[1],
            c1[2],
            number,
            c2[0],
            c2[1],
            c2[2],
            to.half,
            to.color[0],
            to.color[1],
            to.color[2],
            to.color[3],
            across,
            along,
            from.color[0],
            from.color[1],
            from.color[2],
            from.color[3],
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::value::scale;
    use super::*;

    /// What the stock rope vertex shader computes for one vertex.
    fn shaded(vertex: &[f32], forward: Vec3) -> Vec3 {
        let start = [vertex[0], vertex[1], vertex[2]];
        let end = [vertex[4], vertex[5], vertex[6]];
        let control_start = sub(start, [vertex[8], vertex[9], vertex[10]]);
        let control_end = sub(end, [vertex[12], vertex[13], vertex[14]]);
        let (size_start, size_end) = (vertex[3], vertex[15]);
        let (across, along) = (vertex[20], vertex[21]);
        let delta = sub(end, start);
        let right_start = scale(direction(cross(forward, add(delta, control_start))).unwrap(), size_start);
        let right_end = scale(direction(cross(forward, sub(delta, control_end))).unwrap(), size_end);
        let position = lerp(start, end, along);
        let right = lerp(right_start, right_end, along);
        [0, 1, 2].map(|i| position[i] + right[i] * across * 2.0 - 1.0)
    }

    #[test]
    fn the_stock_shader_puts_a_ribbon_centred_on_its_line() {
        let white = [1.0; 4];
        let knots = [
            Knot { at: [0.0, 0.0, 0.0], half: 2.0, color: white },
            Knot { at: [10.0, 0.0, 0.0], half: 3.0, color: white },
            Knot { at: [10.0, 10.0, 0.0], half: 1.0, color: white },
        ];
        let forward = [0.0, 0.0, -1.0];
        let mut out = Vec::new();
        ribbon(&mut out, forward, 3, |index| knots[index], 3.0);
        assert_eq!(out.len(), 2 * 6 * ROPE_STRIDE);
        let vertices: Vec<Vec3> =
            out.as_chunks::<ROPE_STRIDE>().0.iter().map(|vertex| shaded(vertex, forward)).collect();
        let close = |a: Vec3, b: Vec3| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-3);
        // The first piece starts square to its own direction, two units to each side.
        assert!(
            close(vertices[0], [0.0, 2.0, 0.0]) && close(vertices[1], [0.0, -2.0, 0.0]),
            "{:?}",
            &vertices[..2]
        );
        // At the corner both pieces share one edge, square to the line through the neighbours.
        let diagonal = 3.0 / 2f32.sqrt();
        assert!(
            close(vertices[2], [10.0 - diagonal, diagonal, 0.0])
                && close(vertices[4], [10.0 + diagonal, -diagonal, 0.0]),
            "{:?}",
            &vertices[2..5]
        );
        assert!(close(vertices[6], vertices[2]) && close(vertices[7], vertices[4]));
        // The last piece ends square to its own direction.
        assert!(
            close(vertices[8], [9.0, 10.0, 0.0]) && close(vertices[10], [11.0, 10.0, 0.0]),
            "{:?}",
            &vertices[8..11]
        );
    }

    #[test]
    fn pieces_the_shader_cannot_draw_are_left_out() {
        let knot = |at: Vec3| Knot { at, half: 1.0, color: [1.0; 4] };
        let mut out = Vec::new();
        let stacked = [knot([1.0; 3]), knot([1.0; 3]), knot([1.0, 5.0, 1.0])];
        ribbon(&mut out, [0.0, 0.0, 1.0], 3, |index| stacked[index], 3.0);
        assert_eq!(out.len(), 6 * ROPE_STRIDE, "only the piece with a length is drawn");
        out.clear();
        let along_view = [knot([0.0; 3]), knot([0.0, 0.0, 8.0])];
        ribbon(&mut out, [0.0, 0.0, 1.0], 2, |index| along_view[index], 2.0);
        assert!(out.is_empty());
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_spline_passes_through_its_points() {
        let (a, b, c, d) = ([0.0, 0.0, 0.0], [1.0, 2.0, 0.0], [3.0, 2.0, 1.0], [4.0, 0.0, 1.0]);
        assert_eq!(catmull_rom(a, b, c, d, 0.0), b);
        assert_eq!(catmull_rom(a, b, c, d, 1.0), c);
        let middle = catmull_rom(a, b, c, d, 0.5);
        assert!(middle[0] > 1.0 && middle[0] < 3.0 && middle[1] > 2.0);
    }
}
