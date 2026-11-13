//! OpenGL: contexts, cached state, textures, programs and pooled render targets.
//!
//! One context serves the whole process. Every surface the daemon draws on is made
//! current against it in turn, so a texture, a compiled program or a video frame is
//! one object however many outputs show it. Everything here assumes that context is
//! current on the calling thread, which the single render thread guarantees.

mod context;
mod egl;
mod mesh;
mod program;
mod target;
mod texture;

pub use context::{Blend, Caps, Gl, GpuError, Memory, MemoryClass};
pub use egl::{Context, Surface};
pub use mesh::Mesh;
pub use program::{Program, ProgramError, Uniform};
pub use target::{Target, TargetPool};
pub use texture::{Sampling, Texture, TextureFormat};
