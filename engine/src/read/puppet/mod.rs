//! `.mdl` puppets: the mesh a layer is warped with, its skeleton and its animations.
//!
//! There is no documentation of the format. What follows was worked out from the 18
//! files of a workshop library (`MDLV` 13, 16, 17, 19, 21 and 23) and holds for all of
//! them; what is a guess says so.
//!
//! # The file
//!
//! Little-endian throughout; strings end with a zero byte. The file is a mesh followed
//! by sections, each starting with a nine-byte header (`MDL`, a letter, four digits of
//! version, a zero byte).
//!
//! ```text
//! "MDLVnnnn\0"
//! u32      flags: the vertex format of version 13 (0x01800009 skinned, 9 or 0xf not)
//! u32 u32  1, 1 (unknown)
//! string   the material's path
//! u32      0 (4 in one file; unknown)
//! f32 x 6  the bounding box                              version 17 and later
//! u32      the vertex format                             version 14 and later (*)
//! u32      the size of the vertices in bytes
//!          the vertices
//! u32      the size of the indices in bytes
//!          the indices: u16, three a triangle
//! u8       1 when a block follows: u32 count, then count x (u32 size, bytes).
//!          One file has it: a second position for every vertex. Not read.
//!                                                        version 21 and later
//! u8       1 when the parts follow: u32 size, then size / 16 x
//!          (u32 bone, u32 0, u32 first index, u32 index count): the triangles
//!          grouped by the bone that moves them most         version 21 and later
//! u32      0 (unknown)                                   version 23
//!          sections; a file without any ends with one zero byte
//! ```
//!
//! (*) Versions 14, 15, 18, 20 and 22 were not seen. The reader tries the layout the
//! version suggests and then the other two, and takes the first whose sizes add up; and
//! when a section does not start where the layout says, it looks for the skeleton's
//! header instead.
//!
//! The vertex format is a set of bits; a vertex holds, in this order: the position
//! (`0x1`, 3 floats), the normal (`0x2`, 3 floats), the tangent (`0x4`, 4 floats), the
//! bones (`0x0080_0000`, 4 u32), the weights (`0x0100_0000`, 4 floats), the texture
//! coordinate (`0x8`, 2 floats). That is 20 bytes for a puppet without a skeleton, 52
//! for version 13 with one and 80 for the later ones.
//!
//! Every section's header is followed by a u32: the position in the file where the
//! section ends. Sections with a letter this reader does not know (`MDLE` in one file)
//! are stepped over by it.
//!
//! ```text
//! "MDLSnnnn\0"  (versions 1 to 4 seen)
//! u32      where the section ends
//! u16 u16  the number of bones, 0
//!          per bone:
//!   string   the name (empty in every file seen)
//!   i32      unknown (0, 1 or 3)
//!   i32      the parent, -1 for a root; always an earlier bone
//!   u32      64: the size of the transform
//!   f32 x 16 the bind transform, column-major, relative to the parent
//!   string   physics and constraints as JSON, or empty
//!          version 2 and later:
//! u16      the number of inverse kinematics targets; per target a string,
//!          two u32 and a 4x4 matrix
//! u8       1 when a second pose follows: a 4x4 matrix a bone, relative to the
//!          parent (see "The pose")
//!          the rest (a shape per bone, more with targets) is not read
//! ```
//!
//! ```text
//! "MDLAnnnn\0"  (versions 1, 4, 5 and 6 seen)
//! u32      where the section ends
//! u32      the number of animations
//!          per animation:
//!   u32 u32  the id, 0
//!   string   the name
//!   string   "loop" ("mirror" and "single" are the editor's other two; not seen)
//!   f32      frames a second
//!   u32      the length in frames
//!   u32      0
//!   u32      the number of bone tracks
//!            per track: u32 0, u32 size, then size / 36 keyframes of 9 floats:
//!            position, angles (radians), scale. There is a keyframe for every
//!            frame and one for the end: length + 1 of them.
//!            More tracks of the same size follow when the skeleton has inverse
//!            kinematics targets. Nothing counts them; they are stepped over.
//!   u32      0
//!   u8       version 4 and later: 1 when the bones' alpha follows, per bone
//!            u32 0, u32 size, size / 4 floats (one a keyframe)
//!            then zero bytes: 5 in version 4, 29 in version 5, 30 in version 6
//! ```
//!
//! Every file with more than one animation has section version 4, so the lengths of
//! those zero runs for 5 and 6 come from the distance to the section's end in files with
//! one animation. When they are wrong, or not zero, the animations after the first are
//! dropped with a warning.
//!
//! # Coordinates
//!
//! Positions are in the pixels of the layer's image, measured from the image's centre,
//! x to the right and y **up**; z is 0. Texture coordinates have `0 0` at the top left
//! of the image, so `v` grows downwards. This is the space the stock image quad is in,
//! and the layer's own transform (`M_MDL`) applies after skinning.
//!
//! A bone's bind transform is relative to its parent; a root's is relative to the
//! layer. Multiplying down the chain from the root gives the bone in the layer's space.
//!
//! # The pose
//!
//! A keyframe is a bone's whole transform relative to its parent (not a difference from
//! the bind): `translate(position) * rotate(angles) * scale(scale)`, the rotation being
//! `Rz * Ry * Rx` as [`Mat4::rotation`] builds it. In the files whose animations start
//! at the bind pose, the first keyframes equal the bind transforms to float precision
//! under exactly this reading (rotated bind transforms included).
//!
//! A puppet may be cut from a sheet of parts: its vertices then lie where the parts
//! are in the picture (the bind pose), and the skeleton carries a second pose, the one
//! that puts the parts together. That second pose is what such a puppet rests in. A
//! puppet without one rests in its bind pose.
//!
//! [`Puppet::pose`] starts from the rest pose with every bone taken apart into those
//! nine numbers, and applies the layers in order. A layer that replaces moves each
//! number towards the animation's: `pose += blend * (keyframe - pose)`. An additive
//! layer adds the animation's movement from the rest pose: `pose += blend * (keyframe -
//! rest)`. Then, per bone: `local` from the nine numbers, `in layer = parent's in layer *
//! local`, and the matrix for the shader is `in layer * inverse(bind in layer)`. With no
//! layers and no second pose that is the identity for every bone.
//!
//! How Wallpaper Engine blends is not documented; this is the reading that fits the
//! files and the pictures: the two files with a second pose draw as scattered parts
//! from the bind pose and as the character their wallpapers show from the second.
//! Scenes use blends of -1 and 2 on replacing layers, so the blend is not clamped.
//!
//! Between two keyframes the nine numbers are interpolated linearly. `blendin`,
//! `blendout` and `blendtime` of a scene's layer are read ([`LayerSpec`]) but nothing
//! here fades: the caller scales [`PlayingLayer::blend`].
//!
//! # Drawing
//!
//! With the stock image shader (`genericimage2.vert`, `genericimage4.vert`), when
//! [`Puppet::is_skinned`]:
//!
//! - combos: `SKINNING 1`, `BONECOUNT` = `bones.len()`; `SKINNING_ALPHA 1` as well when
//!   [`Puppet::fades_bones`] (the shader tests it with `#ifdef` in one place: define it
//!   only then). They are not in the material; Wallpaper Engine sets them for a puppet.
//! - attributes: `a_Position` vec3 = `position`, `a_TexCoord` vec2 = `uv`,
//!   `a_BlendIndices` **uvec4** = `bones` (an integer attribute:
//!   `glVertexAttribIPointer` with `GL_UNSIGNED_INT`), `a_BlendWeights` vec4 =
//!   `weights`, `a_Normal` vec3 = `normal`, `a_Tangent4` vec4 = `tangent` (the last two
//!   only matter with lighting).
//! - `g_Bones`: the output of [`Puppet::pose`], 12 floats a bone, column-major (three
//!   rotation columns of three floats, then the move): `glUniformMatrix4x3fv(location,
//!   bones.len(), GL_FALSE, ..)`. The shader computes `mul(vec4(position, 1.0), g_Bones[i])`,
//!   which in the dialect is `g_Bones[i] * vec4(position, 1.0)`.
//! - `g_BonesAlpha`: the output of [`Puppet::bone_alpha`], a float a bone.
//! - triangles: [`Puppet::indices`], all of them, in the file's order, in place of the
//!   image quad. The puppet materials seen say `cullmode: nocull`; the winding of the
//!   triangles was not checked.
//! - the model document of some puppets has a `cropoffset`; what it moves was not
//!   worked out here.
//!
//! A puppet that is not skinned is drawn the same way without the combos: only its mesh
//! replaces the quad.

mod animation;
mod bytes;
mod layers;
mod mesh;
mod pose;
mod skeleton;

use std::fmt;

pub use animation::{Animation, Keyframe, Looping, Track};
pub use layers::{LayerSpec, layers_from_scene};
pub use mesh::{Part, PuppetVertex};
pub use pose::PlayingLayer;
pub use skeleton::Bone;

#[cfg(doc)]
use crate::math::Mat4;

use bytes::Reader;
use pose::Rig;

/// Why a puppet could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PuppetError {
    /// The file ends before what it promises.
    Truncated,
    /// Not a puppet, or one with a layout the reader does not know.
    Unknown(String),
    /// The file contradicts itself.
    Corrupt(&'static str),
}

impl fmt::Display for PuppetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PuppetError::Truncated => f.write_str("puppet file is cut short"),
            PuppetError::Unknown(what) => write!(f, "unknown puppet file: {what}"),
            PuppetError::Corrupt(what) => write!(f, "puppet file is corrupt: {what}"),
        }
    }
}

impl std::error::Error for PuppetError {}

/// A puppet: a mesh in place of a layer's quad, and the bones and animations that warp it.
#[derive(Debug, Clone)]
pub struct Puppet {
    /// The version in the file's header (`MDLV0017` is 17).
    pub version: u32,
    /// The path of the material the file names (the layer's model names one too).
    pub material: String,
    /// The corners of the mesh.
    pub vertices: Vec<PuppetVertex>,
    /// The triangles: three indices into [`vertices`](Self::vertices) each.
    pub indices: Vec<u32>,
    /// The triangles grouped by bone, when the file says; empty before version 21.
    pub parts: Vec<Part>,
    /// The skeleton; empty for a puppet that only replaces the quad with a mesh.
    pub bones: Vec<Bone>,
    /// The animations the skeleton can play.
    pub animations: Vec<Animation>,
    skinned: bool,
    second_pose: bool,
    rig: Rig,
}

impl Puppet {
    /// Reads a `.mdl` file. Never panics, whatever the bytes are.
    pub fn parse(bytes: &[u8]) -> Result<Puppet, PuppetError> {
        let mut reader = Reader::new(bytes);
        let version = match reader.tag() {
            Some((b'V', version)) => version,
            _ => return Err(PuppetError::Unknown("no MDLV header".to_owned())),
        };
        let mesh::Mesh { material, mut vertices, indices, mut parts, skinned } =
            mesh::read(&mut reader, version)?;

        let mut bones = Vec::new();
        let mut reference = None;
        let mut animations = Vec::new();
        while let Some((letter, section_version)) = reader.tag() {
            let end = reader.u32()? as usize;
            let mut section = reader.until(end)?;
            match letter {
                b'S' => {
                    let skeleton = skeleton::read(&mut section, section_version)?;
                    bones = skeleton.bones;
                    reference = skeleton.reference;
                }
                b'A' => animations = animation::read(&mut section, section_version),
                _ => {}
            }
            reader.seek(end)?;
        }
        if reader.cut_tag() {
            return Err(PuppetError::Truncated);
        }

        if skinned && bones.is_empty() {
            // A file cut before its skeleton, or one whose skeleton was not found.
            return Err(PuppetError::Corrupt("the vertices follow bones but the file has no skeleton"));
        }
        for vertex in &mut vertices {
            if !skinned {
                (vertex.bones, vertex.weights) = ([0; 4], [0.0; 4]);
                continue;
            }
            for (bone, weight) in vertex.bones.iter_mut().zip(vertex.weights) {
                if (*bone as usize) < bones.len() {
                    continue;
                }
                if weight != 0.0 {
                    return Err(PuppetError::Corrupt("a vertex follows a bone the skeleton does not have"));
                }
                *bone = 0;
            }
        }
        parts.retain(|part| {
            (part.bone as usize) < bones.len()
                && u64::from(part.first_index) + u64::from(part.index_count) <= indices.len() as u64
        });

        let rig = Rig::new(&bones, reference.as_deref());
        let second_pose = reference.as_ref().is_some_and(|pose| pose.len() == bones.len());
        Ok(Puppet {
            version,
            material,
            vertices,
            indices,
            parts,
            bones,
            animations,
            skinned,
            second_pose,
            rig,
        })
    }

    /// Whether the mesh follows bones: the vertices carry weights and there is a
    /// skeleton. When not, the mesh is drawn as it is and there is nothing to pose.
    pub fn is_skinned(&self) -> bool {
        self.skinned
    }

    /// Whether the skeleton carries a second pose: the puppet then rests in that one,
    /// its parts put together, and not where its vertices lie in the picture.
    pub fn has_second_pose(&self) -> bool {
        self.second_pose
    }

    /// Whether any animation changes how opaque bones are (`SKINNING_ALPHA`).
    pub fn fades_bones(&self) -> bool {
        self.animations.iter().any(|animation| !animation.alpha.is_empty())
    }

    /// The animation a scene names by `id`.
    pub fn animation(&self, id: i64) -> Option<&Animation> {
        self.animations.iter().find(|animation| animation.id == id)
    }

    /// The matrices for `g_Bones` (one 4x3 matrix a bone, column-major, 12 floats) for the
    /// animation layers playing at their times, blended as the module's documentation
    /// says. No layers: the rest pose, which for a puppet without a second pose leaves
    /// the mesh as it is in the file. A layer naming an
    /// animation the puppet does not have is ignored. `out` is cleared first.
    pub fn pose(&self, layers: &[PlayingLayer], out: &mut Vec<f32>) {
        self.rig.pose(&self.animations, layers, out);
    }

    /// The values for `g_BonesAlpha` (one float a bone, 0 to 1) for the same layers; all
    /// 1 unless [`fades_bones`](Self::fades_bones). `out` is cleared first.
    pub fn bone_alpha(&self, layers: &[PlayingLayer], out: &mut Vec<f32>) {
        self.rig.alpha(&self.animations, layers, out);
    }
}
