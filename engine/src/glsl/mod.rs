//! Wallpaper Engine's shader dialect becomes GLSL 330.
//!
//! The dialect is GLSL written to also compile as HLSL: `attribute` and `varying`,
//! `gl_FragColor`, and HLSL's names for things (`mul`, `lerp`, `frac`, `saturate`,
//! `texSample2D`). Its settings live in comments: `// [COMBO] {...}` declares a
//! compile-time switch, and the JSON after a `uniform` line says which material
//! value feeds it and what it defaults to.
//!
//! Translation is textual and cheap: includes are inlined ([`Source::load`]), the
//! comments are read ([`Declared`]), and [`Source::assemble`] puts a prelude of
//! `#define`s in front that maps the dialect onto GLSL. The driver does the rest,
//! and when it refuses, [`repair`] reads its message and fixes what it can.

mod declared;
mod fixups;
mod prelude;
mod repair;
mod source;
mod typed;

pub use declared::{ComboDecl, Declared, SamplerDecl, UniformDecl};

/// How many texture slots a pass can have: `g_Texture0` to `g_Texture15`. A document
/// or a shader that names a slot beyond these names nothing.
pub const TEXTURE_SLOTS: usize = 16;
pub use repair::{repair, repair_link};
pub use source::{Combos, Source, Stage};
