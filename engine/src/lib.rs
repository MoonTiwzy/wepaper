//! The wepaper engine: a wallpaper folder in, frames out.
//!
//! The crate is laid out along the path a wallpaper takes:
//!
//! - [`read`]: bytes on disk become data (archives, layered files, textures).
//! - [`doc`]: JSON documents become typed values (project, scene, material, effect).
//! - [`glsl`]: the shader dialect becomes GLSL 330.
//!
//! Nothing here knows about Wayland or the control protocol; the `wepaper` binary
//! brings those.

pub mod doc;
pub mod glsl;
pub mod math;
pub mod read;
