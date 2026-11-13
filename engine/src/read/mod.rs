//! Bytes on disk become data.
//!
//! A wallpaper is a folder, usually with a `scene.pkg` archive in it, on top of the
//! stock assets Wallpaper Engine ships. [`Files`] stacks those and answers lookups
//! the way Wallpaper Engine does: case-insensitively, the wallpaper's files first.

mod archive;
mod files;
pub mod lz4;
pub mod picture;
mod texture;

pub use archive::Archive;
pub use files::{Files, Located};
pub use texture::{Image, Mip, PixelFormat, Pixels, SpriteFrame, SpriteSheet, TextureError, TextureFile};

/// FNV-1a: the hash behind cache identities. Stable across runs and machines,
/// which the default hasher is not.
pub(crate) fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3))
}

/// Where an [`fnv`] chain starts.
pub(crate) const FNV_SEED: u64 = 0xcbf2_9ce4_8422_2325;

/// A path as lookups compare it: `/` separators, no `.` or empty parts, `..` resolved,
/// lower case. Returns the cleaned path with its case kept, and the comparison key.
pub(crate) fn normalize(path: &str) -> (String, String) {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    let clean = parts.join("/");
    let key = clean.to_lowercase();
    (clean, key)
}
