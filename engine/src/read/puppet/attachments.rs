//! The attachment points of a model: named places on its bones that other objects
//! of the scene hang from.

use crate::math::Mat4;

use super::PuppetError;
use super::bytes::Reader;

/// A place on a bone that another object can be attached to.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// What scenes call it (`"attachment": "Arm"`).
    pub name: String,
    /// The bone it is on.
    pub bone: usize,
    /// Where it is on the bone, relative to the bone.
    pub offset: Mat4,
}

/// Reads the body of an `MDAT` section: a u16 count, then per attachment a u16 bone,
/// the name, and a 4x4 matrix, column-major.
pub(super) fn read(section: &mut Reader) -> Result<Vec<Attachment>, PuppetError> {
    let count = section.u16()?;
    let mut attachments = Vec::with_capacity(usize::from(count).min(256));
    for _ in 0..count {
        let bone = usize::from(section.u16()?);
        let name = section.string()?;
        let offset = Mat4(section.floats()?);
        attachments.push(Attachment { name, bone, offset });
    }
    Ok(attachments)
}
