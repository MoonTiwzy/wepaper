//! Sound: what scenes play, and what they hear.
//!
//! Unrelated things share this module because all are audio. [`Sound`] plays a
//! scene's sound layers through libmpv. [`Listener`] captures what the system is
//! playing and turns it into the spectrum that audio-reactive shaders and scripts
//! read. [`Media`] follows the system's media players for the track they are on.
//! The last two run only while something asks for them.

mod capture;
mod fft;
mod media;
mod sound;
mod spectrum;

pub use media::{Media, NowPlaying, Playback};
pub use sound::Sound;
pub use spectrum::{Listener, Spectrum};
