//! Particle systems: definitions, simulation and the vertices that draw them.
//!
//! A particle document ([`ParticleDef`]) names emitters (where and how often
//! particles are born), initializers (what they are born with), operators (what
//! happens to them while they live) and a renderer. [`ParticleSystem`] plays one on
//! the CPU and writes the vertices Wallpaper Engine's stock particle shaders draw.
//! Nothing here touches the GPU or the scene.
//!
//! ```no_run
//! use std::rc::Rc;
//! use wepaper_engine::particles::{Environment, ParticleDef, ParticleSystem, View};
//!
//! # let (document, child_document, instance_override) = (wepaper_json::Value::Null, wepaper_json::Value::Null, wepaper_json::Value::Null);
//! let mut def = ParticleDef::parse(&document)?;
//! def.apply_override(&instance_override);
//! let def = Rc::new(def);
//! let mut system = ParticleSystem::new(def.clone(), 1);
//! for (index, _child) in def.children().iter().enumerate() {
//!     // The caller reads `_child.path` and parses it.
//!     system.attach_child(index, Rc::new(ParticleDef::parse(&child_document)?));
//! }
//! let mut env = Environment::of(&def);
//! let mut vertices = Vec::new();
//! loop {
//!     env.time += 1.0 / 60.0;
//!     system.advance(1.0 / 60.0, &env);
//!     vertices.clear();
//!     let batch = system.write_vertices(&View::default(), &mut vertices);
//!     // draw `batch` with `def.material()`, then the same for `system.children()`
//! #   break;
//! }
//! # Ok::<(), String>(())
//! ```
//!
//! # Render contract
//!
//! Everything a caller needs to draw a [`Batch`] with the stock shaders.
//!
//! **Space.** Vertex positions are in the local space of the scene object that
//! owns the system: its origin is the system's origin, one unit is one scene unit
//! before the object's scale and rotation. `g_ModelViewProjectionMatrix` is
//! therefore projection x view x the object's model matrix (translation, rotation,
//! scale of the object; parallax belongs in it too). [`Environment::pointer`] and
//! [`Environment::control_points`] are in the same local space. Children write
//! into the space of the top-level system: draw them with the parent's matrices.
//! When [`ParticleDef::perspective`] is set the document asks for a perspective
//! projection even in a flat scene.
//!
//! **Material.** [`ParticleDef::material`] is a material document; all real ones
//! name the shader `genericparticle`, whatever the renderer. Draw with
//! [`Batch::shader`] instead of the material's shader name: `genericparticle` for
//! [`Renderer::Sprite`] and [`Renderer::SpriteTrail`], `genericropeparticle` for
//! [`Renderer::Rope`] and [`Renderer::RopeTrail`] (same textures, constants,
//! combos and blending as the material says). Blending, depth and culling come
//! from the material pass as for any material: `additive` (source alpha, one) in
//! most, `translucent` in the rest; depth test and write are off, culling is off
//! (ribbons and rotated sprites show both faces, so keep it off even if a material
//! says otherwise).
//!
//! **Combos.** Set the material's combos, then [`Batch::combos`] on top. They
//! always carry `GS_ENABLED 0` (the engine has no geometry shaders; the vertex
//! shaders then take whole quads) and `THICKFORMAT 1` (the vertex layouts below
//! are the thick ones), plus `TRAILRENDERER 1` for the two trail renderers. The
//! caller adds, from texture 0 of the material:
//!
//! - `SPRITESHEET 1` when the texture has sprite frames (sprite renderers only),
//! - `SPRITESHEETBLEND 1` when it does and [`ParticleDef::blends_frames`] is true,
//! - `SPRITESHEETBLENDNPOT 1` when it does and the texture is padded to a power of
//!   two (its `g_Texture0Resolution.z` differs from `.x`).
//!
//! Do not set `TRAILSCROLLALPHA`, `TRAILFADEALPHA` or `TRAILFADESIZE`: the vertices
//! are not laid out for them, and `TRAILFADESIZE` does not compile without a
//! geometry shader.
//!
//! **Vertices.** [`Batch::layout`] lists (attribute location, float count) in
//! interleave order; the stride is the sum of the counts times four bytes. Draw
//! `Batch::vertices` vertices as `GL_TRIANGLES`, no index buffer. A batch may have
//! zero vertices; skip it.
//!
//! Sprites (17 floats, six vertices per particle):
//!
//! | attribute | location | holds |
//! |---|---|---|
//! | `a_Position` | 0 | the particle's centre |
//! | `a_TexCoordVec4` | 3 | corner (0 or 1, 0 or 1), rotation z, the sprite's width (half the particle's size) |
//! | `a_Color` | 2 | red, green, blue, alpha, 0 to 1, not premultiplied |
//! | `a_TexCoordVec4C1` | 4 | velocity, then the place in the sprite sheet as a fraction 0 to 1 |
//! | `a_TexCoordC2` | 6 | rotation x and y |
//!
//! Ribbons (26 floats, six vertices per piece; a piece runs from a start to an end):
//!
//! | attribute | location | holds |
//! |---|---|---|
//! | `a_PositionVec4` | 12 | start, half width at the start |
//! | `a_TexCoordVec4` | 3 | end, the number of pieces the shader maps the texture over |
//! | `a_TexCoordVec4C1` | 4 | the start's spline control point, the piece's number |
//! | `a_TexCoordVec4C2` | 5 | the end's spline control point, half width at the end |
//! | `a_TexCoordVec4C3` | 13 ([`A_TEXCOORDVEC4C3`]) | colour at the end |
//! | `a_TexCoordC4` | 14 ([`A_TEXCOORDC4`]) | corner (0 or 1 across, 0 or 1 along) |
//! | `a_Color` | 2 | colour at the start |
//!
//! `a_TexCoordVec4C3` and `a_TexCoordC4` are not in `gpu::program::ATTRIBUTES`
//! yet: they must be bound at 13 and 14 for ribbons to draw.
//!
//! The stock rope vertex shader ends with `position += right * uvs.x * 2.0 - 1.0`,
//! which draws the ribbon to one side of its line and a unit off on every axis.
//! The ribbon vertices are pre-shifted so that this exact expression lands them
//! centred; that depends on [`View::forward`] being the vector the shader uses
//! (below). If the shader text is ever repaired instead, the shift in
//! `vertices.rs` (`write_piece`) has to go.
//!
//! **Uniforms.** Besides `g_ModelViewProjectionMatrix` and the material's own
//! (`g_Texture0`, `g_Overbright`, ...):
//!
//! - `g_OrientationRight`, `g_OrientationUp`, `g_OrientationForward`: the axes
//!   sprites lie in, in the object's local space. For screen-facing sprites in a
//!   flat scene with an unrotated object: (1,0,0), (0,1,0), (0,0,1). In a 3D scene:
//!   the camera's right, up and forward taken through the inverse of the object's
//!   rotation.
//! - [`View::forward`]: must equal `g_OrientationForward` x mat3(`g_ModelMatrixInverse`),
//!   the vector the rope shader calls `eyeDirection`. With an unrotated object that
//!   is `g_OrientationForward` itself. `View::right`, `up` and `eye` are the same
//!   camera in local space; only `forward` is read today.
//! - `g_ModelMatrix`, `g_ModelMatrixInverse`: the object's model matrix and its
//!   inverse. The rope shader and the sprite trail path read the inverse.
//! - `g_EyePosition`: the camera's position in scene space (sprite trails turn
//!   their quads around the line to it; fog and lighting use it too). In a flat
//!   scene: the centre of the view, a long way out on +z.
//! - `g_ViewRight`, `g_ViewUp`: the screen's axes in scene space, (1,0,0) and
//!   (0,1,0) in a flat scene; read only with `REFRACT`.
//! - `g_RenderVar0`: [`Batch::render_var0`], as is. (Sprite trail: length per unit
//!   of speed, longest and shortest stretch. Rope trail: the most pieces per trail,
//!   0, how far the trails are towards recording their next point, the most pieces
//!   again. Zero otherwise.)
//! - `g_RenderVar1`: only with `SPRITESHEET`: (frame width, frame height, frame
//!   count, frame height in pixels / frame width in pixels), the first two as
//!   fractions of the texture. The shader assumes the frames fill a grid row by
//!   row, as the sheets particles use do: frame width = 1 / columns, frame height
//!   = 1 / rows.
//! - `g_Texture0Resolution`: (width, height, unpadded width, unpadded height) of
//!   texture 0, as for every material. Without `SPRITESHEET` the shader takes the
//!   sprite's aspect from `.y / .x`.
//!
//! **Control points.** [`Environment::control_points`] are local to the system.
//! A point whose [`ControlPointDef::world_space`] is set has its offset in scene
//! space instead: the caller takes it through the inverse of the object's model
//! matrix before putting it in the environment. Points locked to the pointer are
//! placed here from [`Environment::pointer`].
//!
//! **Values that change.** `instanceoverride` fields linked to a user property or
//! a script are applied with [`ParticleSystem::apply_override`] whenever their
//! value changes; it takes the same object the document has, with the new values.
//!
//! **What is where.** [`ParticleDef::world_space`] systems leave their particles
//! behind when their origin moves. For child systems that is done here. For a
//! top-level system whose scene object moves it is not (the module never sees the
//! object's transform): such particles move with the object.

mod def;
mod emitter;
mod initializer;
mod noise;
mod operator;
mod particle;
mod rng;
pub(crate) use rng::Rng;
mod sim;
mod value;
mod vertices;

pub use def::{AnimationMode, ChildDef, ChildKind, ControlPointDef, ParticleDef, Renderer};
pub use particle::Particle;
pub use sim::ParticleSystem;

/// The location ribbons expect `a_TexCoordVec4C3` (the colour at a piece's end) at.
pub const A_TEXCOORDVEC4C3: u32 = 13;
/// The location ribbons expect `a_TexCoordC4` (a ribbon vertex's corner) at.
pub const A_TEXCOORDC4: u32 = 14;

/// What a system reacts to from outside, for one step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Environment {
    /// Scene time in seconds. The simulation keeps its own clock (the sum of the
    /// steps it was advanced by), so that a system is repeatable for a seed and
    /// pausing it does not make it leap; this is here for callers and for what will
    /// need the scene's clock.
    pub time: f32,
    /// Where the pointer is, in the system's local space; `None` while it is not
    /// over the wallpaper. Control points locked to the pointer sit on it, and the
    /// forces tied to them rest while there is none.
    pub pointer: Option<[f32; 3]>,
    /// Where the control points of the top-level system are, in its local space.
    /// Start from [`Environment::of`], which puts them where the document (and the
    /// instance override) does, and change the ones a script or an animation moves.
    pub control_points: [[f32; 3]; 8],
}

impl Environment {
    /// The surroundings `def` describes for itself: its control points at rest, no pointer.
    pub fn of(def: &ParticleDef) -> Self {
        Self { time: 0.0, pointer: None, control_points: def.control_points().map(|point| point.offset) }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self { time: 0.0, pointer: None, control_points: [[0.0; 3]; 8] }
    }
}

/// The camera, in the local space of the scene object that owns the system.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// The screen's right.
    pub right: [f32; 3],
    /// The screen's up.
    pub up: [f32; 3],
    /// The direction ribbons are flat against: what the rope shader computes as
    /// `g_OrientationForward` through the inverse model matrix.
    pub forward: [f32; 3],
    /// Where the camera is.
    pub eye: [f32; 3],
}

impl Default for View {
    /// A flat scene seen head on.
    fn default() -> Self {
        Self {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
            eye: [0.0, 0.0, 1000.0],
        }
    }
}

/// How to draw the vertices one [`ParticleSystem::write_vertices`] call appended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Batch {
    /// How many vertices were appended; draw them as triangles.
    pub vertices: usize,
    /// (attribute location, float count) in interleave order.
    pub layout: &'static [(u32, i32)],
    /// The renderer the vertices are for.
    pub renderer: Renderer,
    /// The shader combos the batch needs on top of the material's.
    pub combos: &'static [(&'static str, i32)],
    /// The shader to draw with, in place of the one the material names.
    pub shader: &'static str,
    /// The value of `g_RenderVar0`.
    pub render_var0: [f32; 4],
}

impl Batch {
    /// How many floats one vertex takes.
    pub fn stride(&self) -> usize {
        self.layout.iter().map(|&(_, count)| count as usize).sum()
    }
}
