//! From the animations that play to the matrices the shader takes.

use crate::math::Mat4;

use super::animation::{Animation, Keyframe, around};
use super::skeleton::Bone;

/// An animation playing on a puppet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayingLayer {
    /// The animation's id, as the scene's animation layer names it.
    pub animation: i64,
    /// How far into the animation it is, in seconds; the animation loops by itself.
    pub time: f32,
    /// How much of the animation shows: 0 nothing, 1 all of it. More than 1
    /// exaggerates it and less than 0 mirrors it, as Wallpaper Engine lets authors do.
    pub blend: f32,
    /// Whether the animation's movement is added on top of the layers before it
    /// instead of replacing them.
    pub additive: bool,
}

/// What posing needs of a skeleton, worked out once.
#[derive(Debug, Clone, Default)]
pub(super) struct Rig {
    parents: Vec<Option<usize>>,
    /// The bind transforms, relative to the parent.
    bind: Vec<Mat4>,
    /// The same, taken apart into what keyframes hold.
    bind_parts: Vec<[f32; 9]>,
    /// The pose the puppet rests in, and what an additive animation's movement is
    /// measured from: the skeleton's second pose when the file has one, the bind
    /// pose otherwise.
    reference: Vec<[f32; 9]>,
    /// From the layer to each bone at bind time.
    inverse_bind: Vec<Mat4>,
}

impl Rig {
    pub(super) fn new(bones: &[Bone], reference: Option<&[Mat4]>) -> Self {
        let parents: Vec<Option<usize>> = bones.iter().map(|bone| bone.parent).collect();
        let bind: Vec<Mat4> = bones.iter().map(|bone| bone.bind).collect();
        let bind_parts: Vec<[f32; 9]> = bind.iter().map(take_apart).collect();
        let reference = match reference {
            Some(pose) if pose.len() == bones.len() => pose.iter().map(take_apart).collect(),
            _ => bind_parts.clone(),
        };
        let inverse_bind = in_layer(&parents, &bind).iter().map(Mat4::inverse).collect();
        Self { parents, bind, bind_parts, reference, inverse_bind }
    }

    /// Writes a 4x3 matrix a bone: from where a vertex is in the mesh to where the
    /// pose puts it.
    pub(super) fn pose(&self, animations: &[Animation], layers: &[PlayingLayer], out: &mut Vec<f32>) {
        out.clear();
        for (posed, inverse_bind) in self.posed(animations, layers).iter().zip(&self.inverse_bind) {
            let skin = posed.mul(inverse_bind).0;
            for column in 0..4 {
                out.extend_from_slice(&skin[column * 4..column * 4 + 3]);
            }
        }
    }

    /// Every bone where the layers put it, in the space of the mesh.
    pub(super) fn posed(&self, animations: &[Animation], layers: &[PlayingLayer]) -> Vec<Mat4> {
        // A puppet cut from a sheet of parts is bound where the parts lie in the
        // picture and rests where they are put together: its second pose.
        let mut parts = self.reference.clone();
        for layer in layers {
            let Some((animation, frame)) = playing(animations, layer) else { continue };
            for (bone, track) in animation.tracks.iter().enumerate().take(parts.len()) {
                let Some(keyframe) = track.at(frame) else { continue };
                let from = if layer.additive { self.reference[bone] } else { parts[bone] };
                for ((part, to), from) in parts[bone].iter_mut().zip(keyframe.floats()).zip(from) {
                    *part += layer.blend * (to - from);
                }
            }
        }
        // A bone nothing moved keeps its bind matrix bit for bit, so the mesh at rest
        // is exactly the mesh in the file.
        let local: Vec<Mat4> = parts
            .iter()
            .enumerate()
            .map(
                |(bone, parts)| {
                    if *parts == self.bind_parts[bone] { self.bind[bone] } else { put_together(parts) }
                },
            )
            .collect();
        in_layer(&self.parents, &local)
    }

    /// Writes how opaque every bone is.
    pub(super) fn alpha(&self, animations: &[Animation], layers: &[PlayingLayer], out: &mut Vec<f32>) {
        out.clear();
        out.resize(self.bind.len(), 1.0);
        for layer in layers {
            let Some((animation, frame)) = playing(animations, layer) else { continue };
            for (alpha, track) in out.iter_mut().zip(&animation.alpha) {
                let Some(last) = track.len().checked_sub(1) else { continue };
                let (before, after, mix) = around(frame, last);
                let to = track[before] + (track[after] - track[before]) * mix;
                *alpha += layer.blend * (to - if layer.additive { 1.0 } else { *alpha });
            }
        }
        for alpha in out {
            *alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
        }
    }
}

/// The animation a layer plays and the frame it is at; `None` when the puppet has no
/// such animation or nothing of it shows.
fn playing<'a>(animations: &'a [Animation], layer: &PlayingLayer) -> Option<(&'a Animation, f32)> {
    if layer.blend == 0.0 || !layer.blend.is_finite() {
        return None;
    }
    let animation = animations.iter().find(|animation| animation.id == layer.animation)?;
    Some((animation, animation.frame_at(layer.time)))
}

/// Every bone's transform relative to the layer, from the ones relative to the parent.
fn in_layer(parents: &[Option<usize>], local: &[Mat4]) -> Vec<Mat4> {
    let mut world: Vec<Mat4> = Vec::with_capacity(local.len());
    for (bone, local) in local.iter().enumerate() {
        // Parents come before their children: the reader makes anything else a root.
        let transform = match parents[bone].and_then(|parent| world.get(parent)) {
            Some(parent) => parent.mul(local),
            None => *local,
        };
        world.push(transform);
    }
    world
}

/// The matrix of a keyframe's nine numbers: scale, then rotation, then the move.
fn put_together(parts: &[f32; 9]) -> Mat4 {
    let keyframe = Keyframe::from_floats(*parts);
    Mat4::translation(keyframe.position)
        .mul(&Mat4::rotation(keyframe.angles))
        .mul(&Mat4::scaling(keyframe.scale))
}

/// A transform as a keyframe's nine numbers. Exact for what puppets hold (a move, a
/// rotation, a scale); a sheared matrix loses its shear.
fn take_apart(transform: &Mat4) -> [f32; 9] {
    let m = &transform.0;
    let length = |column: usize| {
        (m[column] * m[column] + m[column + 1] * m[column + 1] + m[column + 2] * m[column + 2]).sqrt()
    };
    let mut scale = [length(0), length(4), length(8)];
    let determinant = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[4] * (m[1] * m[10] - m[2] * m[9])
        + m[8] * (m[1] * m[6] - m[2] * m[5]);
    if determinant < 0.0 {
        scale[0] = -scale[0];
    }
    // The rotation's columns; an axis scaled to nothing keeps the one it would have unscaled.
    let axis = |column: usize, row: usize, unscaled: f32| {
        if scale[column / 4].abs() > 1e-12 { m[column + row] / scale[column / 4] } else { unscaled }
    };
    let (r00, r10, r20) = (axis(0, 0, 1.0), axis(0, 1, 0.0), axis(0, 2, 0.0));
    let (r11, r21) = (axis(4, 1, 1.0), axis(4, 2, 0.0));
    let (r01, r22) = (axis(4, 0, 0.0), axis(8, 2, 1.0));
    let y = (-r20).clamp(-1.0, 1.0).asin();
    let (x, z) =
        if r20.abs() < 0.999_999 { (r21.atan2(r22), r10.atan2(r00)) } else { (0.0, (-r01).atan2(r11)) };
    [m[12], m[13], m[14], x, y, z, scale[0], scale[1], scale[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transform_taken_apart_goes_back_together() {
        for parts in [
            [3.0, -2.0, 0.0, 0.0, 0.0, 0.7, 1.0, 1.0, 1.0],
            [10.0, 5.0, 1.0, 0.3, -0.4, 2.5, 2.0, 0.5, 1.5],
            [0.0, 0.0, 0.0, 0.0, 0.0, -1.588, -1.0, 1.0, 1.0],
        ] {
            let matrix = put_together(&parts);
            let again = put_together(&take_apart(&matrix));
            assert!(
                matrix.0.iter().zip(again.0).all(|(a, b)| (a - b).abs() < 1e-4),
                "{parts:?}: {matrix:?} != {again:?}"
            );
        }
        let plain = take_apart(&put_together(&[3.0, -2.0, 0.0, 0.0, 0.0, 0.7, 2.0, 3.0, 1.0]));
        assert!(
            plain
                .iter()
                .zip([3.0, -2.0, 0.0, 0.0, 0.0, 0.7, 2.0, 3.0, 1.0])
                .all(|(a, b)| (a - b).abs() < 1e-5),
            "{plain:?}"
        );
    }
}
