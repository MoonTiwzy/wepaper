//! JSON documents become typed values.
//!
//! Wallpaper Engine's documents are dynamically typed: a field may be a literal, a
//! string of numbers, or an object that links it to a user property, a script or an
//! animation. [`Dyn`] is that shape, and every field that can move is one.

mod animation;
mod effect;
mod material;
mod model;
mod project;
mod scene;
mod value;

pub use animation::{Animation, Key, Repeat};
pub use effect::{EffectBind, EffectDoc, EffectFbo, EffectPass};
pub use material::{Blending, MaterialDoc, MaterialPass};
pub use model::ModelDoc;
pub use project::{Kind, Project, Property, PropertyKind};
pub use scene::{
    Alignment, EffectUse, General, ImageObject, Object, ObjectKind, ParticleObject, PassOverride, Scene,
    SoundObject, TextObject,
};
pub use value::{AsFloats, Dyn, FromDoc, Script, UserLink, floats, typed_flag, typed_floats};
