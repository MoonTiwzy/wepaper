//! Video: libmpv decodes, and draws each frame into a texture of ours.
//!
//! libmpv is loaded at run time, so the engine starts without it (and then has no
//! video). It renders with our own GL context: a frame goes from the decoder's
//! planes to our texture on the GPU, with no copy through main memory and no second
//! context to pay for.
//!
//! One file is decoded once per process however many outputs and scenes show it:
//! [`VideoHub`] hands every asker the same [`VideoStream`].

mod hub;
pub(crate) mod mpv;
mod stream;
mod tape;
mod transcode;
mod wallpaper;

pub use hub::{SharedVideo, VideoHub};
pub use stream::{VideoError, VideoOptions, VideoStream, Waker};
pub use transcode::Limits;
pub(crate) use wallpaper::VideoHold;
pub use wallpaper::VideoInstance;

/// Whether libmpv is there to play video with.
pub fn available() -> bool {
    mpv::api().is_some()
}
