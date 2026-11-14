//! The `MDLA` section: the animations.

use super::PuppetError;
use super::bytes::Reader;

/// The size of one keyframe in the file: nine floats.
const KEYFRAME_BYTES: usize = 36;

/// Where a bone is at one frame, relative to its parent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    /// The move, in the layer's pixels.
    pub position: [f32; 3],
    /// The rotation around x, y and z in radians, applied in that order.
    pub angles: [f32; 3],
    /// The scale along each axis.
    pub scale: [f32; 3],
}

impl Keyframe {
    pub(super) fn from_floats(floats: [f32; 9]) -> Self {
        let [px, py, pz, ax, ay, az, sx, sy, sz] = floats;
        Self { position: [px, py, pz], angles: [ax, ay, az], scale: [sx, sy, sz] }
    }

    pub(super) fn floats(&self) -> [f32; 9] {
        let ([px, py, pz], [ax, ay, az], [sx, sy, sz]) = (self.position, self.angles, self.scale);
        [px, py, pz, ax, ay, az, sx, sy, sz]
    }
}

/// What one bone does over an animation: a keyframe for every frame, the first and the
/// last included.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    /// The keyframes, one frame apart.
    pub frames: Vec<Keyframe>,
}

impl Track {
    /// The bone's transform `frame` frames in, between the two keyframes around it.
    pub fn at(&self, frame: f32) -> Option<Keyframe> {
        let last = self.frames.len().checked_sub(1)?;
        let (before, after, mix) = around(frame, last);
        let (a, b) = (self.frames[before].floats(), self.frames[after].floats());
        Some(Keyframe::from_floats(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * mix)))
    }
}

/// The two entries around `frame` in a list whose last index is `last`, and how far
/// between them it is.
pub(super) fn around(frame: f32, last: usize) -> (usize, usize, f32) {
    let frame = if frame.is_finite() { frame.clamp(0.0, last as f32) } else { 0.0 };
    let before = (frame.floor() as usize).min(last);
    (before, (before + 1).min(last), frame - before as f32)
}

/// What an animation does when its time runs past its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Looping {
    /// Starts over (`loop`).
    Loop,
    /// Plays backwards to the start, then forwards again (`mirror`).
    Mirror,
    /// Stays on its last frame (`single`).
    Single,
}

/// One animation of a puppet.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    /// The number a scene's animation layer names it by.
    pub id: i64,
    /// The name the author gave it.
    pub name: String,
    /// How many frames play in a second.
    pub fps: f32,
    /// How many frames long it is; the tracks hold one keyframe more.
    pub frames: u32,
    /// What happens past its end.
    pub looping: Looping,
    /// What every bone does, in the order of the bones.
    pub tracks: Vec<Track>,
    /// How opaque every bone is at every frame (`g_BonesAlpha`); empty when the
    /// animation does not fade bones.
    pub alpha: Vec<Vec<f32>>,
}

impl Animation {
    /// How long one pass takes, in seconds.
    pub fn length(&self) -> f32 {
        if self.fps > 0.0 { self.frames as f32 / self.fps } else { 0.0 }
    }

    /// The frame that shows `time` seconds in, as the animation loops. Time may be
    /// negative: an animation played backwards.
    pub fn frame_at(&self, time: f32) -> f32 {
        let length = self.frames as f32;
        let frame = time * self.fps;
        if !(length > 0.0 && frame.is_finite()) {
            return 0.0;
        }
        match self.looping {
            Looping::Loop => frame.rem_euclid(length),
            Looping::Mirror => {
                let there_and_back = frame.rem_euclid(2.0 * length);
                if there_and_back > length { 2.0 * length - there_and_back } else { there_and_back }
            }
            Looping::Single => frame.clamp(0.0, length),
        }
    }
}

/// Reads the section; `reader` ends where the section does. An animation that does not
/// read ends the list: the ones before it still play.
pub(super) fn read(reader: &mut Reader, version: u32) -> Vec<Animation> {
    let mut animations = Vec::new();
    let Ok(count) = reader.u32() else { return animations };
    for index in 0..count {
        let animation = match animation(reader) {
            Ok(animation) => animation,
            Err(why) => {
                log::warn!("puppet animation {index} of {count} is skipped with the ones after it: {why}");
                break;
            }
        };
        animations.push(animation);
        let Some(animation) = animations.last_mut() else { break };
        // After the last animation only the bone alpha is wanted; the rest may be cut short.
        if let Err(why) = trailer(reader, version, animation)
            && index + 1 < count
        {
            log::warn!("puppet animations after {index} of {count} are skipped: {why}");
            break;
        }
    }
    animations
}

fn animation(reader: &mut Reader) -> Result<Animation, PuppetError> {
    let id = i64::from(reader.u32()?);
    reader.take(4)?;
    let name = reader.string()?;
    let looping = match reader.string()?.as_str() {
        "mirror" => Looping::Mirror,
        "single" => Looping::Single,
        _ => Looping::Loop,
    };
    let fps = reader.f32()?;
    let frames = reader.u32()?;
    reader.take(4)?;
    let count = reader.u32()? as usize;
    if !fps.is_finite() || fps < 0.0 || count > reader.remaining() / 8 {
        return Err(PuppetError::Corrupt("an animation's header makes no sense"));
    }

    let mut tracks = Vec::with_capacity(count);
    let mut track_bytes = 0;
    for _ in 0..count {
        reader.take(4)?;
        track_bytes = reader.u32()? as usize;
        if !track_bytes.is_multiple_of(KEYFRAME_BYTES) {
            return Err(PuppetError::Corrupt("a track is not a whole number of keyframes"));
        }
        let mut data = reader
            .until(reader.position().checked_add(track_bytes).ok_or(PuppetError::Truncated)?)
            .map_err(|_| PuppetError::Truncated)?;
        reader.take(track_bytes)?;
        let mut track = Track { frames: Vec::with_capacity(track_bytes / KEYFRAME_BYTES) };
        while data.remaining() > 0 {
            track.frames.push(Keyframe::from_floats(data.floats()?));
        }
        tracks.push(track);
    }

    // More tracks of the same shape may follow the bones' (they move the targets of
    // inverse kinematics). Nothing counts them; what follows the tracks starts with
    // eight bytes that cannot be taken for a track's header.
    while let Some(after) = extra_track(reader, track_bytes) {
        *reader = after;
    }
    Ok(Animation { id, name, fps, frames, looping, tracks, alpha: Vec::new() })
}

/// The reader past one more track of `track_bytes` bytes, when one is next.
fn extra_track<'a>(reader: &Reader<'a>, track_bytes: usize) -> Option<Reader<'a>> {
    let mut probe = reader.clone();
    let is_track = track_bytes > 0 && probe.u32().ok()? == 0 && probe.u32().ok()? as usize == track_bytes;
    (is_track && probe.take(track_bytes).is_ok()).then_some(probe)
}

/// What follows an animation's tracks. It grew with the section's version, and only
/// the bone alpha in it is understood.
fn trailer(reader: &mut Reader, version: u32, animation: &mut Animation) -> Result<(), PuppetError> {
    reader.take(4)?;
    if version < 4 {
        return Ok(());
    }
    if reader.u8()? != 0 {
        let mut alpha = Vec::with_capacity(animation.tracks.len());
        for _ in 0..animation.tracks.len() {
            reader.take(4)?;
            let size = reader.u32()? as usize;
            alpha.push(
                reader
                    .take(size)?
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|float| f32::from_le_bytes(*float))
                    .collect(),
            );
        }
        animation.alpha = alpha;
    }
    let unknown = match version {
        4 => 5,
        5 => 29,
        _ => 30,
    };
    let rest = reader.take(unknown)?;
    // In version 6, bytes 1 to 24 are six floats: the box the animation moves the mesh
    // in, zero in a puppet and not in a 3D model.
    let bounds = if version >= 6 { 1..25 } else { 0..0 };
    if rest.iter().enumerate().any(|(at, byte)| *byte != 0 && !bounds.contains(&at)) {
        return Err(PuppetError::Corrupt("an animation ends with data this reader does not know"));
    }
    Ok(())
}
