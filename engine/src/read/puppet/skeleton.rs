//! The `MDLS` section: the bones.

use crate::math::Mat4;

use super::PuppetError;
use super::bytes::Reader;

/// One bone of the skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    /// The name the author gave it; usually empty.
    pub name: String,
    /// The bone it hangs from; `None` for a root. Always an earlier bone.
    pub parent: Option<usize>,
    /// The bind transform: where the bone was when the mesh was bound to it, relative
    /// to its parent (to the layer for a root).
    pub bind: Mat4,
    /// The bone's physics and constraint settings, a JSON document as written in the
    /// file; empty when it has none.
    pub simulation: String,
}

/// What the section holds.
pub(super) struct Skeleton {
    pub(super) bones: Vec<Bone>,
    /// A second pose of the same bones, relative to their parents; files whose
    /// animations do not start from the bind pose carry it.
    pub(super) reference: Option<Vec<Mat4>>,
}

/// Reads the section; `reader` ends where the section does.
pub(super) fn read(reader: &mut Reader, version: u32) -> Result<Skeleton, PuppetError> {
    let count = usize::from(reader.u16()?);
    reader.take(2)?;
    let mut bones = Vec::with_capacity(count.min(reader.remaining() / 78));
    for index in 0..count {
        let name = reader.string()?;
        reader.take(4)?;
        let parent = reader.i32()?;
        let size = reader.u32()? as usize;
        if size < 64 {
            return Err(PuppetError::Corrupt("a bone's transform is smaller than a matrix"));
        }
        let mut transform = reader
            .until(reader.position().checked_add(size).ok_or(PuppetError::Truncated)?)
            .map_err(|_| PuppetError::Truncated)?;
        reader.take(size)?;
        let bind = Mat4(transform.floats()?);
        if !bind.0.iter().all(|value| value.is_finite()) {
            return Err(PuppetError::Corrupt("a bone's transform is not a number"));
        }
        let simulation = reader.string()?;
        let parent = match usize::try_from(parent) {
            Ok(parent) if parent < index => Some(parent),
            Ok(parent) => {
                log::warn!(
                    "puppet bone {index} hangs from bone {parent}, which does not come before it: taken as a root"
                );
                None
            }
            Err(_) => None,
        };
        bones.push(Bone { name, parent, bind, simulation });
    }
    let reference = if version >= 2 { reference(reader, bones.len()).ok().flatten() } else { None };
    Ok(Skeleton { bones, reference })
}

/// The second pose, when the file has one. What else follows the bones (targets for
/// inverse kinematics, a shape per bone) is not read.
fn reference(reader: &mut Reader, bones: usize) -> Result<Option<Vec<Mat4>>, PuppetError> {
    for _ in 0..reader.u16()? {
        reader.string()?;
        reader.take(8 + 64)?;
    }
    if reader.u8()? != 1 || reader.remaining() < bones * 64 {
        return Ok(None);
    }
    let mut pose = Vec::with_capacity(bones);
    for _ in 0..bones {
        let transform = Mat4(reader.floats()?);
        if !transform.0.iter().all(|value| value.is_finite()) {
            return Ok(None);
        }
        pose.push(transform);
    }
    Ok(Some(pose))
}
