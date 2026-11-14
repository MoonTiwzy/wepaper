//! Puppets: hand-built files of every layout the reader knows, and the poses they give.
//! The last test reads a real library when `WEPAPER_TEST_WORKSHOP` is set (see `corpus.rs`).

use std::f32::consts::FRAC_PI_2;
use std::path::PathBuf;

use wepaper_engine::read::Archive;
use wepaper_engine::read::puppet::{Looping, Part, PlayingLayer, Puppet, PuppetError, layers_from_scene};

/// A bone or a keyframe: the parent, the position, the angle around z.
#[derive(Clone, Copy)]
struct Joint {
    parent: i32,
    position: [f32; 2],
    angle: f32,
}

const fn joint(parent: i32, x: f32, y: f32, angle: f32) -> Joint {
    Joint { parent, position: [x, y], angle }
}

struct Clip {
    id: u32,
    mode: &'static str,
    fps: f32,
    /// A row of keyframes (one a bone) for every frame and the end.
    keyframes: Vec<Vec<Joint>>,
    /// The same shape, when the clip fades bones.
    alpha: Option<Vec<Vec<f32>>>,
}

/// What goes into a hand-built file.
struct Spec {
    version: u32,
    /// Position, texture coordinate, and the bone that moves the vertex.
    vertices: Vec<([f32; 2], [f32; 2], u32)>,
    indices: Vec<u16>,
    skeleton: Option<(u32, Vec<Joint>)>,
    second_pose: Option<Vec<Joint>>,
    clips: Option<(u32, Vec<Clip>)>,
}

#[derive(Default)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn raw(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.extend_from_slice(bytes);
        self
    }
    fn u32(&mut self, value: u32) -> &mut Self {
        self.raw(&value.to_le_bytes())
    }
    fn floats(&mut self, values: &[f32]) -> &mut Self {
        values.iter().for_each(|value| self.0.extend_from_slice(&value.to_le_bytes()));
        self
    }
    fn string(&mut self, text: &str) -> &mut Self {
        self.raw(text.as_bytes()).raw(&[0])
    }
    fn tag(&mut self, letter: char, version: u32) -> &mut Self {
        self.string(&format!("MDL{letter}{version:04}"))
    }
    /// A block whose size in bytes precedes it.
    fn sized(&mut self, block: &Bytes) -> &mut Self {
        self.u32(block.0.len() as u32).raw(&block.0)
    }
    /// A section: its header, the position where it ends, its body.
    fn section(&mut self, letter: char, version: u32, body: &Bytes) -> &mut Self {
        self.tag(letter, version);
        let end = self.0.len() + 4 + body.0.len();
        self.u32(end as u32).raw(&body.0)
    }
}

fn matrix(joint: &Joint) -> [f32; 16] {
    let (sin, cos) = joint.angle.sin_cos();
    [
        cos,
        sin,
        0.0,
        0.0,
        -sin,
        cos,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        joint.position[0],
        joint.position[1],
        0.0,
        1.0,
    ]
}

impl Spec {
    fn skinned(&self) -> bool {
        self.skeleton.is_some()
    }

    /// The file, and where its mesh ends.
    fn build(&self) -> (Vec<u8>, usize) {
        let format = match (self.version, self.skinned()) {
            (..=16, false) => 0x9,
            (..=16, true) => 0x0180_0009,
            (_, false) => 0xf,
            (_, true) => 0x0180_000f,
        };
        let mut file = Bytes::default();
        file.tag('V', self.version)
            .u32(if self.version >= 17 { format & !0x6 } else { format })
            .u32(1)
            .u32(1);
        file.string("materials/test.json").u32(0);
        if self.version >= 17 {
            file.floats(&[-1.0, -1.0, 0.0, 1.0, 1.0, 0.0]);
        }
        if self.version >= 14 {
            file.u32(format);
        }
        let mut vertices = Bytes::default();
        for (position, uv, bone) in &self.vertices {
            vertices.floats(&[position[0], position[1], 0.0]);
            if format & 0x6 != 0 {
                vertices.floats(&[0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0]);
            }
            if self.skinned() {
                vertices.u32(*bone).u32(0).u32(0).u32(0).floats(&[1.0, 0.0, 0.0, 0.0]);
            }
            vertices.floats(uv);
        }
        file.sized(&vertices);
        let mut indices = Bytes::default();
        for index in &self.indices {
            indices.raw(&index.to_le_bytes());
        }
        file.sized(&indices);
        if self.version >= 21 {
            // Version 21's second set of positions, then the parts.
            let mut positions = Bytes::default();
            for _ in &self.vertices {
                positions.floats(&[7.0, 7.0, 7.0]);
            }
            if self.version == 21 {
                file.raw(&[1]).u32(1).sized(&positions);
            } else {
                file.raw(&[0]);
            }
            let mut parts = Bytes::default();
            parts.u32(0).u32(0).u32(0).u32(self.indices.len() as u32);
            file.raw(&[1]).sized(&parts);
            if self.version >= 23 {
                file.u32(0);
            }
        }
        let mesh_end = file.0.len();

        if let Some((version, bones)) = &self.skeleton {
            let mut body = Bytes::default();
            body.raw(&(bones.len() as u16).to_le_bytes()).raw(&[0, 0]);
            for bone in bones {
                body.string("")
                    .u32(1)
                    .raw(&bone.parent.to_le_bytes())
                    .u32(64)
                    .floats(&matrix(bone))
                    .string("");
            }
            if *version >= 2 {
                body.raw(&[0, 0]);
                match &self.second_pose {
                    Some(pose) => pose.iter().fold(body.raw(&[1]), |body, joint| body.floats(&matrix(joint))),
                    None => body.raw(&[0]),
                };
                // What real files go on with: read by nobody.
                body.u32(0).u32(0).raw(&[1]).floats(&vec![0.5; 19 * bones.len()]);
            }
            file.section('S', *version, &body);
        }
        if let Some((version, clips)) = &self.clips {
            let mut body = Bytes::default();
            body.u32(clips.len() as u32);
            for clip in clips {
                let bones = clip.keyframes[0].len();
                body.u32(clip.id).u32(0).string("clip").string(clip.mode).floats(&[clip.fps]);
                body.u32(clip.keyframes.len() as u32 - 1).u32(0).u32(bones as u32);
                for bone in 0..bones {
                    let mut track = Bytes::default();
                    for row in &clip.keyframes {
                        track.floats(&[
                            row[bone].position[0],
                            row[bone].position[1],
                            0.0,
                            0.0,
                            0.0,
                            row[bone].angle,
                            1.0,
                            1.0,
                            1.0,
                        ]);
                    }
                    body.u32(0).sized(&track);
                }
                body.u32(0);
                if *version >= 4 {
                    match &clip.alpha {
                        Some(alpha) => {
                            body.raw(&[1]);
                            for bone in 0..bones {
                                let mut track = Bytes::default();
                                for row in alpha {
                                    track.floats(&[row[bone]]);
                                }
                                body.u32(0).sized(&track);
                            }
                        }
                        None => {
                            body.raw(&[0]);
                        }
                    }
                    body.raw(
                        &[0; 30][..match version {
                            4 => 5,
                            5 => 29,
                            _ => 30,
                        }],
                    );
                }
            }
            file.section('A', *version, &body);
        }
        file.raw(&[0]);
        (file.0, mesh_end)
    }
}

/// A square of two triangles around the origin whose right half follows the second bone.
fn square(version: u32) -> Spec {
    Spec {
        version,
        vertices: vec![
            ([-10.0, -10.0], [0.0, 1.0], 0),
            ([10.0, -10.0], [1.0, 1.0], 1),
            ([10.0, 10.0], [1.0, 0.0], 1),
            ([-10.0, 10.0], [0.0, 0.0], 0),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
        skeleton: None,
        second_pose: None,
        clips: None,
    }
}

const BONES: [Joint; 2] = [joint(-1, -10.0, 0.0, 0.0), joint(0, 15.0, 0.0, 0.0)];

/// Two frames at ten a second: the second bone turns a quarter, then is back unturned
/// and 10 higher.
fn swing(id: u32, mode: &'static str) -> Clip {
    let keyframes = vec![
        BONES.to_vec(),
        vec![BONES[0], joint(0, 15.0, 0.0, FRAC_PI_2)],
        vec![BONES[0], joint(0, 15.0, 10.0, 0.0)],
    ];
    Clip { id, mode, fps: 10.0, keyframes, alpha: None }
}

fn rigged(version: u32, skeleton: u32, animations: u32, clips: Vec<Clip>) -> Spec {
    Spec { skeleton: Some((skeleton, BONES.to_vec())), clips: Some((animations, clips)), ..square(version) }
}

fn parse(spec: &Spec) -> Puppet {
    Puppet::parse(&spec.build().0).unwrap_or_else(|why| panic!("version {}: {why}", spec.version))
}

fn layer(animation: i64, time: f32) -> PlayingLayer {
    PlayingLayer { animation, time, blend: 1.0, additive: false }
}

/// Where the shader puts every vertex with `matrices` in `g_Bones`.
fn skin(puppet: &Puppet, matrices: &[f32]) -> Vec<[f32; 3]> {
    assert_eq!(matrices.len(), puppet.bones.len() * 12);
    if !puppet.is_skinned() {
        return puppet.vertices.iter().map(|vertex| vertex.position).collect();
    }
    puppet
        .vertices
        .iter()
        .map(|vertex| {
            let mut out = [0.0; 3];
            for (bone, weight) in vertex.bones.iter().zip(vertex.weights) {
                let m = &matrices[*bone as usize * 12..][..12];
                let [x, y, z] = vertex.position;
                for row in 0..3 {
                    out[row] += weight * (m[row] * x + m[3 + row] * y + m[6 + row] * z + m[9 + row]);
                }
            }
            out
        })
        .collect()
}

fn posed(puppet: &Puppet, layers: &[PlayingLayer]) -> Vec<[f32; 3]> {
    let mut matrices = vec![99.0];
    puppet.pose(layers, &mut matrices);
    skin(puppet, &matrices)
}

#[track_caller]
fn assert_near(actual: [f32; 3], expected: [f32; 2]) {
    let off = (actual[0] - expected[0]).abs().max((actual[1] - expected[1]).abs()).max(actual[2].abs());
    assert!(off < 1e-3, "{actual:?} is not {expected:?}");
}

#[test]
fn a_version_16_file_without_bones_parses_to_its_mesh() {
    let puppet = parse(&square(16));
    assert_eq!((puppet.version, puppet.material.as_str()), (16, "materials/test.json"));
    assert_eq!(puppet.vertices.len(), 4);
    assert_eq!(puppet.vertices[1].position, [10.0, -10.0, 0.0]);
    assert_eq!(puppet.vertices[1].uv, [1.0, 1.0]);
    assert_eq!(puppet.vertices[1].normal, [0.0, 0.0, 1.0]);
    assert_eq!(puppet.indices, [0, 1, 2, 0, 2, 3]);
    assert!(!puppet.is_skinned() && puppet.bones.is_empty() && puppet.animations.is_empty());
    let mut matrices = vec![1.0];
    puppet.pose(&[], &mut matrices);
    assert!(matrices.is_empty(), "a puppet without bones has no matrices");
}

#[test]
fn a_version_17_mesh_without_bones_has_its_bounds_and_normals_stepped_over() {
    let puppet = parse(&square(17));
    assert_eq!(
        puppet.vertices.iter().map(|v| v.uv).collect::<Vec<_>>(),
        [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]
    );
    assert_eq!(puppet.vertices[3].tangent, [1.0, 0.0, 0.0, 1.0]);
    assert!(!puppet.is_skinned());
}

#[test]
fn every_skinned_layout_parses_to_the_same_puppet() {
    for (version, skeleton, animations) in [(13, 1, 1), (17, 2, 4), (19, 2, 5), (21, 3, 6), (23, 4, 6)] {
        let puppet =
            parse(&rigged(version, skeleton, animations, vec![swing(7, "loop"), swing(8, "single")]));
        let label = format!("version {version}");
        assert_eq!(puppet.version, version);
        assert!(puppet.is_skinned(), "{label}");
        assert_eq!(
            puppet.vertices.iter().map(|v| v.position).collect::<Vec<_>>(),
            [[-10.0, -10.0, 0.0], [10.0, -10.0, 0.0], [10.0, 10.0, 0.0], [-10.0, 10.0, 0.0]],
            "{label}"
        );
        assert_eq!(puppet.vertices[2].uv, [1.0, 0.0], "{label}");
        assert_eq!(puppet.vertices.iter().map(|v| v.bones[0]).collect::<Vec<_>>(), [0, 1, 1, 0], "{label}");
        assert_eq!(puppet.vertices[2].weights, [1.0, 0.0, 0.0, 0.0], "{label}");
        assert_eq!(puppet.indices, [0, 1, 2, 0, 2, 3], "{label}");
        assert_eq!(puppet.bones.iter().map(|b| b.parent).collect::<Vec<_>>(), [None, Some(0)], "{label}");
        assert_eq!(&puppet.bones[1].bind.0[12..14], [15.0, 0.0], "{label}");
        let expected_parts: &[Part] =
            if version >= 21 { &[Part { bone: 0, first_index: 0, index_count: 6 }] } else { &[] };
        assert_eq!(puppet.parts, expected_parts, "{label}");

        assert_eq!(
            puppet.animations.iter().map(|a| (a.id, a.looping)).collect::<Vec<_>>(),
            [(7, Looping::Loop), (8, Looping::Single)],
            "{label}"
        );
        let animation = puppet.animation(8).expect("the second animation");
        assert_eq!((animation.name.as_str(), animation.fps, animation.frames), ("clip", 10.0, 2), "{label}");
        assert!((animation.length() - 0.2).abs() < 1e-6, "{label}");
        assert_eq!(animation.tracks.iter().map(|t| t.frames.len()).collect::<Vec<_>>(), [3, 3], "{label}");
        assert_eq!(animation.tracks[1].frames[1].angles, [0.0, 0.0, FRAC_PI_2], "{label}");
        assert_eq!(animation.tracks[1].frames[2].position, [15.0, 10.0, 0.0], "{label}");
        assert!(!puppet.fades_bones(), "{label}");
    }
}

#[test]
fn a_version_nobody_has_seen_is_read_by_the_layout_that_adds_up() {
    // 15 is taken for "format, no bounds"; this file has the bounds as well.
    let mut spec = rigged(17, 2, 4, vec![swing(7, "loop")]);
    let (mut bytes, _) = spec.build();
    bytes[4..8].copy_from_slice(b"0015");
    let puppet = Puppet::parse(&bytes).expect("the other layout");
    assert_eq!(
        (puppet.version, puppet.vertices.len(), puppet.bones.len(), puppet.animations.len()),
        (15, 4, 2, 1)
    );

    // And a block between the triangles and the skeleton that the reader does not know.
    spec.version = 23;
    let (bytes, mesh_end) = spec.build();
    let mut padded = bytes[..mesh_end].to_vec();
    padded.extend_from_slice(&[0xee; 13]);
    let shift = padded.len() - mesh_end;
    padded.extend_from_slice(&bytes[mesh_end..]);
    // The sections say where they end, counted from the start of the file.
    for letter in *b"SA" {
        let at = padded.windows(4).position(|w| w == [b'M', b'D', b'L', letter]).expect("the section") + 9;
        let end = u32::from_le_bytes(padded[at..at + 4].try_into().unwrap()) + shift as u32;
        padded[at..at + 4].copy_from_slice(&end.to_le_bytes());
    }
    let puppet = Puppet::parse(&padded).expect("the skeleton is found past the unknown block");
    assert_eq!((puppet.bones.len(), puppet.animations.len()), (2, 1));
}

#[test]
fn a_file_cut_short_is_an_error_and_never_a_panic() {
    for spec in [
        square(16),
        rigged(13, 1, 1, vec![swing(7, "loop")]),
        rigged(23, 4, 6, vec![swing(7, "loop"), swing(8, "mirror")]),
    ] {
        let (bytes, mesh_end) = spec.build();
        let whole = Puppet::parse(&bytes).expect("the whole file");
        // A mesh without bones is whole once its triangles are. One with bones is a puppet
        // without animations when it ends where their section would start, and otherwise
        // needs that section to its last byte (only the zero byte that ends the file may go).
        let animations = bytes.windows(4).position(|window| window == b"MDLA");
        for length in 0..bytes.len() {
            if let Ok(puppet) = Puppet::parse(&bytes[..length]) {
                let label = format!("version {}: {length} of {} bytes parsed", spec.version, bytes.len());
                assert_eq!(puppet.vertices, whole.vertices, "{label}");
                match animations {
                    None => assert!(length >= mesh_end, "{label}"),
                    Some(start) if length == start => assert!(puppet.animations.is_empty(), "{label}"),
                    Some(_) => {
                        assert!(length == bytes.len() - 1 && puppet.animations == whole.animations, "{label}")
                    }
                }
            }
        }
        assert_eq!(
            Puppet::parse(&bytes[..60]).err(),
            Some(PuppetError::Truncated),
            "version {}",
            spec.version
        );
    }
}

#[test]
fn garbage_is_an_error_and_never_a_panic() {
    assert!(matches!(Puppet::parse(b""), Err(PuppetError::Unknown(_))));
    assert!(matches!(
        Puppet::parse(b"MDLS0004\0 and so on, long enough to be a header"),
        Err(PuppetError::Unknown(_))
    ));
    assert!(matches!(
        Puppet::parse(b"TEXV0005\0TEXI0001\0\0\0\0\0\0\0\0\0\0\0\0\0"),
        Err(PuppetError::Unknown(_))
    ));
    assert!(Puppet::parse(&[b"MDLV0017\0".as_slice(), &[0xff; 4096]].concat()).is_err());

    // Every byte of a good file broken in turn, and then noise after a good header.
    let (bytes, _) = Spec {
        second_pose: Some(BONES.to_vec()),
        ..rigged(21, 3, 6, vec![swing(7, "loop"), swing(8, "single")])
    }
    .build();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut noise = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 24) as u8
    };
    let try_out = |bytes: &[u8]| {
        if let Ok(puppet) = Puppet::parse(bytes) {
            assert!(puppet.indices.iter().all(|index| (*index as usize) < puppet.vertices.len()));
            assert!(
                puppet
                    .vertices
                    .iter()
                    .all(|v| v.bones.iter().all(|bone| (*bone as usize) < puppet.bones.len().max(1)))
            );
            let mut out = Vec::new();
            puppet.pose(&[layer(7, 0.05), PlayingLayer { additive: true, ..layer(8, 1e9) }], &mut out);
            assert_eq!(out.len(), puppet.bones.len() * 12);
            puppet.bone_alpha(&[layer(7, f32::NAN)], &mut out);
            assert_eq!(out.len(), puppet.bones.len());
        }
    };
    for at in 0..bytes.len() {
        for value in [0x00, 0xff, noise()] {
            let mut broken = bytes.clone();
            broken[at] = value;
            try_out(&broken);
        }
    }
    for round in 0..2000 {
        let mut broken = bytes.clone();
        let from = 9 + round % (bytes.len() - 9);
        broken[from..].iter_mut().for_each(|byte| *byte = noise());
        try_out(&broken);
    }
}

#[test]
fn the_rest_pose_leaves_the_mesh_where_it_is() {
    let mut spec = rigged(17, 2, 4, vec![swing(7, "loop")]);
    // Bind transforms that are not plain moves, so the inverse has work to do.
    spec.skeleton = Some((2, vec![joint(-1, -10.0, 3.0, 0.4), joint(0, 15.0, -2.0, -1.3)]));
    let puppet = parse(&spec);
    let mut matrices = Vec::new();
    puppet.pose(&[], &mut matrices);
    assert_eq!(matrices.len(), 24);
    for (vertex, moved) in puppet.vertices.iter().zip(skin(&puppet, &matrices)) {
        assert_near(moved, [vertex.position[0], vertex.position[1]]);
    }
    // A layer that names no animation of the puppet, or shows nothing of one, is the rest pose too.
    let mut again = Vec::new();
    puppet.pose(&[layer(999, 1.0), PlayingLayer { blend: 0.0, ..layer(7, 0.1) }], &mut again);
    assert_eq!(matrices, again);
}

#[test]
fn an_animation_poses_the_bones_it_moves() {
    let puppet = parse(&rigged(17, 2, 4, vec![swing(7, "loop"), swing(8, "single")]));
    // The second bone sits at (5, 0) in the layer; the right half of the square follows it.
    let at = |id: i64, time: f32| posed(&puppet, &[layer(id, time)]);

    let start = at(8, 0.0);
    assert_near(start[1], [10.0, -10.0]);
    assert_near(start[2], [10.0, 10.0]);

    // One frame in, the bone has turned a quarter around itself.
    let turned = at(8, 0.1);
    assert_near(turned[0], [-10.0, -10.0]);
    assert_near(turned[1], [15.0, 5.0]);
    assert_near(turned[2], [-5.0, 5.0]);
    assert_near(turned[3], [-10.0, 10.0]);

    // Halfway between two keyframes is halfway between their numbers: an eighth of a turn.
    let between = at(8, 0.05);
    let (sin, cos) = (FRAC_PI_2 / 2.0).sin_cos();
    assert_near(between[1], [5.0 + 5.0 * cos + 10.0 * sin, 5.0 * sin - 10.0 * cos]);

    // At its end, and past it, an animation that plays once holds its last keyframe.
    for time in [0.2, 5.0] {
        let end = at(8, time);
        assert_near(end[0], [-10.0, -10.0]);
        assert_near(end[1], [10.0, 0.0]);
        assert_near(end[2], [10.0, 20.0]);
    }
    assert_eq!(at(8, -3.0), start, "before its start it holds the first");
}

#[test]
fn animations_loop_mirror_or_stop() {
    let puppet = parse(&rigged(19, 2, 5, vec![swing(7, "loop"), swing(8, "single"), swing(9, "mirror")]));
    let at = |id: i64, time: f32| posed(&puppet, &[layer(id, time)])[1];
    let near = |a: [f32; 3], b: [f32; 3]| assert_near(a, [b[0], b[1]]);

    // A loop is back at its start when its length has passed, and a negative time plays it backwards.
    near(at(7, 0.2), at(7, 0.0));
    near(at(7, 0.25), at(7, 0.05));
    near(at(7, 20.0 + 0.15), at(7, 0.15));
    near(at(7, -0.05), at(7, 0.15));
    let (sin, cos) = (FRAC_PI_2 / 2.0).sin_cos();
    assert_near(at(7, 0.15), [5.0 + 5.0 * cos + 10.0 * sin, 5.0 + 5.0 * sin - 10.0 * cos]);

    // A mirror runs back from its end.
    near(at(9, 0.25), at(9, 0.15));
    near(at(9, 0.35), at(9, 0.05));
    near(at(9, 0.45), at(9, 0.05));
    assert_near(at(9, 0.2), [10.0, 0.0]);

    let animation = puppet.animation(9).expect("the mirror");
    assert!((animation.frame_at(0.3) - 1.0).abs() < 1e-4);
    assert_eq!(puppet.animation(8).expect("the single").frame_at(7.0), 2.0);
}

#[test]
fn layers_blend_in_order() {
    let puppet = parse(&rigged(17, 2, 4, vec![swing(7, "single"), swing(8, "single")]));
    let end = |blend: f32, additive: bool| PlayingLayer { blend, additive, ..layer(7, 0.2) };

    // Half of an animation that lifts the bone by 10 lifts it by 5; twice lifts by 20; minus one lowers it.
    assert_near(posed(&puppet, &[end(0.5, false)])[1], [10.0, -5.0]);
    assert_near(posed(&puppet, &[end(2.0, false)])[1], [10.0, 10.0]);
    assert_near(posed(&puppet, &[end(-1.0, false)])[1], [10.0, -20.0]);

    // A replacing layer on top of another hides it; an additive one adds its movement.
    let turned = layer(8, 0.1);
    assert_near(posed(&puppet, &[turned, end(1.0, false)])[1], [10.0, 0.0]);
    assert_near(posed(&puppet, &[turned, end(1.0, true)])[1], [15.0, 15.0]);
    assert_near(posed(&puppet, &[end(1.0, true), end(1.0, true)])[1], [10.0, 10.0]);
}

#[test]
fn additive_layers_are_measured_from_the_second_pose() {
    // The animation's keyframes are around a pose far from the bind pose, as in the real
    // files that carry a second pose; only the difference from it may show.
    let far = [joint(-1, 300.0, -200.0, 0.0), joint(0, 40.0, 70.0, 0.0)];
    let keyframes = vec![far.to_vec(), vec![far[0], joint(0, 40.0, 73.0, 0.0)]];
    let clip = Clip { id: 7, mode: "single", fps: 1.0, keyframes, alpha: None };
    let puppet = parse(&Spec { second_pose: Some(far.to_vec()), ..rigged(17, 2, 4, vec![clip]) });

    // The puppet rests in its second pose, not where its vertices lie in the picture.
    assert!(puppet.has_second_pose());
    let rest = posed(&puppet, &[]);
    assert_near(rest[1], [345.0, -140.0]);

    let additive = |time: f32| posed(&puppet, &[PlayingLayer { additive: true, ..layer(7, time) }]);
    assert_near(additive(0.0)[1], [rest[1][0], rest[1][1]]);
    assert_near(additive(1.0)[1], [rest[1][0], rest[1][1] + 3.0]);
    assert_near(additive(1.0)[0], [rest[0][0], rest[0][1]]);

    // A replacing layer ends where the animation says, whatever the rest pose.
    let replacing = posed(&puppet, &[layer(7, 1.0)]);
    assert_near(replacing[1], [rest[1][0], rest[1][1] + 3.0]);
}

#[test]
fn bone_alpha_follows_its_tracks() {
    let mut clip = swing(7, "single");
    clip.alpha = Some(vec![vec![1.0, 1.0], vec![1.0, 0.5], vec![1.0, 0.0]]);
    let puppet = parse(&rigged(17, 2, 4, vec![clip, swing(8, "loop")]));
    assert!(puppet.fades_bones());
    let alpha = |layers: &[PlayingLayer]| {
        let mut out = vec![5.0; 7];
        puppet.bone_alpha(layers, &mut out);
        out
    };
    assert_eq!(alpha(&[]), [1.0, 1.0]);
    assert_eq!(alpha(&[layer(7, 0.1)]), [1.0, 0.5]);
    assert_eq!(alpha(&[layer(7, 0.15)]), [1.0, 0.25]);
    assert_eq!(alpha(&[layer(7, 0.2), layer(8, 0.0)]), [1.0, 0.0], "an animation without alpha leaves it");
    assert_eq!(alpha(&[PlayingLayer { blend: 0.5, ..layer(7, 0.2) }]), [1.0, 0.5]);
    assert_eq!(alpha(&[PlayingLayer { blend: 3.0, ..layer(7, 0.2) }]), [1.0, 0.0], "never outside 0 to 1");
}

#[test]
fn scene_layers_read_with_their_links() {
    let doc = wepaper_json::parse(
        r#"[
            {"additive": true, "animation": 47, "blend": 0.63, "blendin": true, "blendout": false, "blendtime": 0.25,
             "id": 57, "name": "kopf", "rate": 0.4, "visible": true},
            {"additive": false, "animation": 697, "blend": {"user": "strength", "value": 2.0}, "id": 805, "name": "arm",
             "rate": {"script": "export function update(value) { return 1; }", "value": -1.0},
             "visible": {"user": "float", "value": false}},
            {"animation": 3},
            "not a layer"
        ]"#,
    )
    .expect("the document");
    let layers = layers_from_scene(&doc);
    assert_eq!(layers.len(), 3);

    let head = &layers[0];
    assert_eq!((head.id, head.animation, head.name.as_str()), (57, 47, "kopf"));
    assert_eq!(
        (head.rate.value, head.blend.value, head.visible.value, head.additive),
        (0.4, 0.63, true, true)
    );
    assert_eq!((head.blend_in, head.blend_out, head.blend_time), (true, false, 0.25));
    assert_eq!(
        head.playing(10.0),
        Some(PlayingLayer { animation: 47, time: 4.0, blend: 0.63, additive: true })
    );

    let arm = &layers[1];
    assert_eq!((arm.blend.value, arm.rate.value, arm.visible.value), (2.0, -1.0, false));
    assert_eq!(arm.blend.user.as_ref().map(|link| link.name.as_str()), Some("strength"));
    assert!(arm.rate.script.is_some() && arm.visible.user.is_some());
    assert_eq!(arm.playing(1.0), None, "a hidden layer does not play");

    let bare = &layers[2];
    assert_eq!(
        (bare.animation, bare.rate.value, bare.blend.value, bare.visible.value, bare.additive),
        (3, 1.0, 1.0, true, false)
    );
    assert!(layers_from_scene(&wepaper_json::Value::Null).is_empty());
}

#[test]
fn every_puppet_of_the_library_reads() {
    let Some(workshop) =
        std::env::var_os("WEPAPER_TEST_WORKSHOP").map(PathBuf::from).filter(|dir| dir.is_dir())
    else {
        eprintln!("skipped: WEPAPER_TEST_WORKSHOP is not set");
        return;
    };
    let mut packages: Vec<PathBuf> = std::fs::read_dir(&workshop)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("scene.pkg"))
        .filter(|path| path.is_file())
        .collect();
    packages.sort();

    let mut failures = Vec::new();
    let mut read = 0;
    eprintln!(
        "{:<11} {:<26} {:>4} {:>6} {:>6} {:>5} {:>5}  animations: id name frames@fps = seconds (+alpha)",
        "wallpaper", "file", "ver", "verts", "tris", "bones", "parts"
    );
    for package in &packages {
        let Ok(archive) = Archive::open(package) else { continue };
        let wallpaper = package
            .parent()
            .and_then(|dir| dir.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let names: Vec<String> =
            archive.names().filter(|name| name.to_lowercase().ends_with(".mdl")).map(str::to_owned).collect();
        for name in names {
            let label = format!("{wallpaper}:{name}");
            let Some(Ok(bytes)) = archive.read(&name) else {
                failures.push(format!("{label}: unreadable"));
                continue;
            };
            let puppet = match Puppet::parse(&bytes) {
                Ok(puppet) => puppet,
                Err(why) => {
                    failures.push(format!("{label}: {why}"));
                    continue;
                }
            };
            read += 1;
            let animations: Vec<String> = puppet
                .animations
                .iter()
                .map(|a| {
                    format!(
                        "{} {:?} {}@{} = {:.2}s{}",
                        a.id,
                        a.name,
                        a.frames,
                        a.fps,
                        a.length(),
                        if a.alpha.is_empty() { "" } else { " +alpha" }
                    )
                })
                .collect();
            let file = name.rsplit('/').next().unwrap_or(&name);
            eprintln!(
                "{wallpaper:<11} {file:<26} {:>4} {:>6} {:>6} {:>5} {:>5}  {}",
                puppet.version,
                puppet.vertices.len(),
                puppet.indices.len() / 3,
                puppet.bones.len(),
                puppet.parts.len(),
                if animations.is_empty() { "-".to_owned() } else { animations.join("; ") }
            );
            failures.extend(check(&puppet).into_iter().map(|why| format!("{label}: {why}")));
        }
    }
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(read > 0, "no puppets in {}", workshop.display());
    eprintln!("{read} puppets read");
}

/// What must hold for any real puppet.
fn check(puppet: &Puppet) -> Vec<String> {
    let mut wrong = Vec::new();
    let mut expect = |holds: bool, what: &str| {
        if !holds {
            wrong.push(what.to_owned());
        }
    };
    expect(!puppet.vertices.is_empty() && !puppet.indices.is_empty(), "an empty mesh");
    expect(puppet.indices.len().is_multiple_of(3), "indices that are not triangles");
    expect(
        puppet.indices.iter().all(|index| (*index as usize) < puppet.vertices.len()),
        "an index out of range",
    );
    expect(puppet.material.ends_with(".json"), "a material that is not a document");
    expect(
        puppet.vertices.iter().all(|v| v.position.iter().chain(&v.uv).all(|n| n.is_finite())),
        "a vertex that is not a number",
    );
    expect(
        puppet.parts.iter().map(|part| part.index_count as usize).sum::<usize>()
            == if puppet.parts.is_empty() { 0 } else { puppet.indices.len() },
        "parts that do not cover the triangles",
    );
    expect(puppet.is_skinned() == !puppet.bones.is_empty(), "bones without weights or weights without bones");
    if !puppet.is_skinned() {
        expect(puppet.animations.is_empty(), "animations without a skeleton");
        return wrong;
    }
    expect(
        puppet.vertices.iter().all(|v| v.bones.iter().all(|bone| (*bone as usize) < puppet.bones.len())),
        "a bone out of range",
    );
    expect(
        puppet.vertices.iter().all(|v| {
            (v.weights.iter().sum::<f32>() - 1.0).abs() < 1e-3
                && v.weights.iter().all(|w| (0.0..=1.0).contains(w))
        }),
        "weights that do not sum to 1",
    );
    expect(
        puppet.bones.iter().enumerate().all(|(index, bone)| bone.parent.is_none_or(|parent| parent < index)),
        "a parent after its child",
    );

    let mut matrices = Vec::new();
    puppet.pose(&[], &mut matrices);
    let rest = skin(puppet, &matrices);
    let extent =
        puppet.vertices.iter().flat_map(|v| v.position).fold(1.0f32, |extent, n| extent.max(n.abs()));
    let drift = rest
        .iter()
        .zip(&puppet.vertices)
        .flat_map(|(moved, vertex)| (0..3).map(move |i| (moved[i] - vertex.position[i]).abs()))
        .fold(0.0f32, f32::max);
    // A thousandth of a pixel on a mesh a few hundred pixels wide; on one thousands wide, what
    // single precision loses through a chain of bones: hundredths of a pixel.
    // A puppet with a second pose rests in it, its parts moved from where they lie in the picture.
    if puppet.has_second_pose() {
        expect(drift.is_finite(), "a rest pose that is not a number");
    } else {
        expect(drift <= 1e-3f32.max(extent * 1e-5), &format!("the rest pose moves the mesh by {drift}"));
    }

    for animation in &puppet.animations {
        let id = animation.id;
        expect(animation.fps > 0.0 && animation.frames > 0, &format!("animation {id} has no length"));
        expect(
            animation.tracks.len() == puppet.bones.len(),
            &format!("animation {id} has {} tracks for {} bones", animation.tracks.len(), puppet.bones.len()),
        );
        expect(
            animation.tracks.iter().all(|track| track.frames.len() == animation.frames as usize + 1),
            &format!("animation {id} has tracks of another length"),
        );
        expect(
            animation.alpha.is_empty() || animation.alpha.len() == puppet.bones.len(),
            &format!("animation {id} fades some bones only"),
        );
        expect(
            animation.alpha.iter().all(|track| {
                track.len() == animation.frames as usize + 1 && track.iter().all(|a| (0.0..=1.0).contains(a))
            }),
            &format!("animation {id} has alpha outside 0 to 1"),
        );

        let mut largest = 0.0f32;
        let mut finite = true;
        let mut alpha = Vec::new();
        for step in -8..=40 {
            let time = animation.length() * step as f32 / 16.0;
            for additive in [false, true] {
                let layers = [PlayingLayer { animation: id, time, blend: 0.63, additive }];
                puppet.pose(&layers, &mut matrices);
                finite &= matrices.len() == puppet.bones.len() * 12 && matrices.iter().all(|n| n.is_finite());
                puppet.bone_alpha(&layers, &mut alpha);
                finite &= alpha.len() == puppet.bones.len() && alpha.iter().all(|a| (0.0..=1.0).contains(a));
                if additive {
                    for (moved, vertex) in skin(puppet, &matrices).iter().zip(&puppet.vertices) {
                        largest =
                            largest.max((moved[0] - vertex.position[0]).hypot(moved[1] - vertex.position[1]));
                    }
                }
            }
        }
        expect(finite, &format!("animation {id} gives matrices that are not numbers"));
        // Played additively no animation of the library throws the mesh further than its own size.
        expect(
            largest < 2.0 * extent,
            &format!("animation {id} moves a vertex by {largest} on a mesh of {extent}"),
        );
        eprintln!(
            "{:>11} animation {id}: additive at 0.63 moves vertices by up to {largest:.1} px (mesh reaches {extent:.0} px from its centre)",
            ""
        );
    }
    wrong
}
