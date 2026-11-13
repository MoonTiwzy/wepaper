//! The wepaper engine: a wallpaper folder in, frames out.
//!
//! The crate is laid out along the path a wallpaper takes:
//!
//! - [`read`]: bytes on disk become data (archives, layered files, textures).
//!
//! Nothing here knows about Wayland or the control protocol; the `wepaper` binary
//! brings those.

pub mod read;
