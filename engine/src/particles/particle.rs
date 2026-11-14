//! One particle, and the control points it reacts to.

use super::value::Vec3;

/// One live particle.
///
/// Initializers set the first group of fields at birth; operators move the particle
/// and, every frame, derive the `now_*` values that are drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Particle {
    /// Where it is, in the system's local space (relative to what it follows, for a
    /// child system that follows a particle).
    pub position: Vec3,
    /// Units per second.
    pub velocity: Vec3,
    /// Rotation around x, y and z in radians.
    pub rotation: Vec3,
    /// Radians per second around x, y and z.
    pub angular_velocity: Vec3,
    /// The colour it was born with, 0 to 1.
    pub color: Vec3,
    /// The opacity it was born with.
    pub alpha: f32,
    /// The size it was born with: the width of its sprite.
    pub size: f32,
    /// Seconds since birth.
    pub age: f32,
    /// Seconds it lives for.
    pub lifetime: f32,
    /// The colour drawn this frame.
    pub now_color: Vec3,
    /// The opacity drawn this frame.
    pub now_alpha: f32,
    /// The size drawn this frame.
    pub now_size: f32,
    /// What its per-particle random choices derive from.
    pub(crate) seed: u32,
    /// Its number in birth order; particles are kept sorted by it.
    pub(crate) id: u64,
    /// The source (emitter instance) it came from.
    pub(crate) source: u32,
    /// How many points of its trail are recorded.
    pub(crate) trail: u32,
}

impl Particle {
    /// A particle as it is before any initializer ran.
    pub(crate) fn newborn(seed: u32, id: u64, source: u32) -> Self {
        Self {
            position: [0.0; 3],
            velocity: [0.0; 3],
            rotation: [0.0; 3],
            angular_velocity: [0.0; 3],
            color: [1.0; 3],
            alpha: 1.0,
            size: 20.0,
            age: 0.0,
            lifetime: 1.0,
            now_color: [1.0; 3],
            now_alpha: 1.0,
            now_size: 20.0,
            seed,
            id,
            source,
            trail: 0,
        }
    }

    /// How far through its life it is, 0 to 1.
    pub fn life(&self) -> f32 {
        (self.age / self.lifetime).clamp(0.0, 1.0)
    }
}

/// A control point as the simulation sees it this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControlPoint {
    /// Where it is, in the system's local space.
    pub(crate) position: Vec3,
    /// False for a point locked to a pointer that is not over the wallpaper: forces
    /// tied to it rest instead of acting from a made-up place.
    pub(crate) active: bool,
}

impl Default for ControlPoint {
    fn default() -> Self {
        Self { position: [0.0; 3], active: true }
    }
}
