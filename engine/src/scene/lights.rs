//! The lights of a scene, as lit shaders read them.
//!
//! A light is an object of the scene like any other: it has a place, parents, and
//! values that properties, scripts and timelines move. What the shaders get each frame
//! is two `vec4`s a light (see `glsl/lighting.rs`): its colour and reach, and where it
//! is or which way its light comes from. A scene without lights has none of this.
//!
//! Spot and tube lights are lit as points: no scene of the library has one.

use crate::doc::{LightKind, LightObject};
use crate::math::{Mat4, normalize};

use super::Layer;
use super::bindings::{Bindings, Slot};

/// One light, by the layer it is.
struct Light {
    layer: usize,
    kind: LightKind,
    /// Slots of the value table: what drives them is the light's own.
    color: usize,
    intensity: usize,
    radius: f32,
    exponent: f32,
}

/// The scene's lights and what the shaders are given of them this frame.
#[derive(Default)]
pub(crate) struct Lights {
    lights: Vec<Light>,
    /// Colour times intensity, then the radius; a radius of 0 for light from far away.
    pub(crate) color_radius: Vec<[f32; 4]>,
    /// Where the light is, then its falloff; for light from far away, the way to it.
    pub(crate) position_exponent: Vec<[f32; 4]>,
}

impl Lights {
    /// Adds the light that layer `index` is. Its colour and intensity get slots that
    /// whatever drives them in the document writes to.
    pub(crate) fn add(&mut self, bindings: &mut Bindings, index: usize, light: &LightObject) {
        let color = bindings.value(light.color.value.to_vec());
        bindings.link_named(&light.color, Slot::Value(color), Some("color"));
        let intensity = bindings.value(vec![light.intensity.value]);
        bindings.link_named(&light.intensity, Slot::Value(intensity), Some("intensity"));
        self.lights.push(Light {
            layer: index,
            kind: light.kind,
            color,
            intensity,
            // A point light that reaches nowhere would read as light from far away.
            radius: light.radius.max(1e-3),
            exponent: light.exponent.max(0.0),
        });
    }

    /// How many lights the shaders are compiled for.
    pub(crate) fn count(&self) -> usize {
        self.lights.len()
    }

    /// Works out this frame's uniforms from the layers' places and the value table.
    /// A light that is hidden gives no light.
    pub(crate) fn update(&mut self, layers: &[Layer], values: &[Vec<f32>]) {
        self.color_radius.clear();
        self.position_exponent.clear();
        for light in &self.lights {
            let layer = &layers[light.layer];
            let floats = |slot: usize| values.get(slot).map_or(&[][..], Vec::as_slice);
            let color = floats(light.color);
            let intensity = floats(light.intensity).first().copied().unwrap_or(1.0);
            let strength = if layer.shown { intensity } else { 0.0 };
            let channel = |at: usize| color.get(at).copied().unwrap_or(1.0) * strength;
            let (radius, place) = placed(light.kind, &layer.world, light.radius, light.exponent);
            self.color_radius.push([channel(0), channel(1), channel(2), radius]);
            self.position_exponent.push(place);
        }
    }
}

/// What a light's place gives the shader: its radius (0 for light from far away), and
/// its position and falloff, or the way to it.
///
/// A directional light shines along its own x axis, the way the editor draws its arrow.
fn placed(kind: LightKind, world: &Mat4, radius: f32, exponent: f32) -> (f32, [f32; 4]) {
    match kind {
        LightKind::Directional => {
            let along = normalize(world.transform_vector([1.0, 0.0, 0.0]));
            (0.0, [-along[0], -along[1], -along[2], 0.0])
        }
        LightKind::Point | LightKind::Spot | LightKind::Tube => {
            (radius, [world.0[12], world.0[13], world.0[14], exponent])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directional_light_comes_from_against_its_arrow() {
        // Turned a quarter round z, its x axis points up: its light comes from below.
        let world =
            Mat4::translation([5.0, 5.0, 5.0]).mul(&Mat4::rotation([0.0, 0.0, std::f32::consts::FRAC_PI_2]));
        let (radius, place) = placed(LightKind::Directional, &world, 10.0, 2.0);
        assert_eq!(radius, 0.0);
        assert!(place[0].abs() < 1e-6 && (place[1] + 1.0).abs() < 1e-6 && place[2].abs() < 1e-6, "{place:?}");
    }

    #[test]
    fn a_point_light_is_where_its_layer_is() {
        let world = Mat4::translation([1.0, 2.0, 3.0]).mul(&Mat4::scaling([4.0; 3]));
        assert_eq!(placed(LightKind::Point, &world, 10.0, 2.0), (10.0, [1.0, 2.0, 3.0, 2.0]));
    }
}
