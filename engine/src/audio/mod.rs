//! Sound: what scenes play, and what they hear.
//!
//! Two unrelated things share this module because both are audio. [`Sound`] plays a
//! scene's sound layers through libmpv. [`Listener`] captures what the system is
//! playing and turns it into the spectrum that audio-reactive shaders and scripts
//! read; it runs only while something asks for it.

mod capture;
mod fft;
mod sound;
mod spectrum;

pub use sound::Sound;
pub use spectrum::{Listener, Spectrum};
