//! A skeleton as it plays: the animation layers of a puppet or a model, and the
//! bones' matrices of this frame.
//!
//! The bones live in the scene's value table like every other value a shader is fed;
//! the animation layers' rate, blend and switch are slots that properties, timelines
//! and scripts write to.

use crate::glsl::Combos;
use crate::read::puppet::{LayerSpec, PlayingLayer, Puppet};

use super::bindings::{Bindings, PuppetField, Slot};

/// One animation layer as it plays.
pub(crate) struct AnimationLayer {
    pub(crate) spec: LayerSpec,
    /// How fast it plays: 1 is the animation's own speed.
    pub(crate) rate: f32,
    /// How much of it shows.
    pub(crate) blend: f32,
    pub(crate) visible: bool,
    /// Where it is in its animation, in seconds: the time played, each stretch at
    /// the rate it had.
    pub(crate) clock: f32,
    /// Whether its clock runs: a script may pause it, or stop it at the start.
    pub(crate) running: bool,
}

impl AnimationLayer {
    /// Plays on by `seconds`, at its rate, unless it is paused.
    pub(crate) fn advance(&mut self, seconds: f32) {
        if self.running {
            self.clock += seconds * self.rate;
        }
    }

    /// Puts it at `frame` of an animation of `fps` frames a second.
    pub(crate) fn seek(&mut self, frame: f32, fps: f32) {
        if fps > 0.0 && frame.is_finite() {
            self.clock = frame / fps;
        }
    }
}

/// The animation layers of one puppet or model and where its bones are.
pub(crate) struct Skeleton {
    pub(crate) layers: Vec<AnimationLayer>,
    /// The slot of the value table that holds `g_Bones`.
    bones: usize,
    /// The slot that holds `g_BonesAlpha`, when the puppet fades bones.
    alpha: Option<usize>,
    playing: Vec<PlayingLayer>,
}

impl Skeleton {
    /// The skeleton of `puppet` at rest, its bones in the value table.
    pub(crate) fn new(bindings: &mut Bindings, puppet: &Puppet) -> Self {
        let mut rest = Vec::new();
        puppet.pose(&[], &mut rest);
        let bones = bindings.value(rest);
        let alpha = puppet.fades_bones().then(|| bindings.value(vec![1.0; puppet.bones.len()]));
        Self { layers: Vec::new(), bones, alpha, playing: Vec::new() }
    }

    /// Gives the skeleton the animation layers `specs`. `index` is the layer's place in
    /// the scene, for what drives them.
    pub(crate) fn play(&mut self, bindings: &mut Bindings, specs: Vec<LayerSpec>, index: usize) {
        self.layers = specs
            .into_iter()
            .enumerate()
            .map(|(layer, spec)| {
                bindings.link(&spec.rate, Slot::Puppet(index, layer, PuppetField::Rate));
                bindings.link(&spec.blend, Slot::Puppet(index, layer, PuppetField::Blend));
                bindings.link(&spec.visible, Slot::Puppet(index, layer, PuppetField::Visible));
                AnimationLayer {
                    rate: spec.rate.value,
                    blend: spec.blend.value,
                    visible: spec.visible.value,
                    clock: 0.0,
                    running: true,
                    spec,
                }
            })
            .collect();
    }

    /// What a pass needs to be drawn with these bones: the combos that switch
    /// skinning on, and the uniforms fed from the value table. Nothing for a puppet
    /// that is not skinned.
    pub(crate) fn inputs(&self, puppet: &Puppet, combos: &mut Combos) -> Vec<(&'static str, usize)> {
        let mut slots = Vec::new();
        if puppet.is_skinned() {
            combos.insert("SKINNING".to_owned(), 1);
            combos.insert("BONECOUNT".to_owned(), puppet.bones.len() as i32);
            slots.push(("g_Bones", self.bones));
            // The shader asks whether this one is defined at all: only when it is needed.
            if let Some(alpha) = self.alpha {
                combos.insert("SKINNING_ALPHA".to_owned(), 1);
                slots.push(("g_BonesAlpha", alpha));
            }
        }
        slots
    }

    /// The animation layer a script means: by its name, or by its place.
    pub(crate) fn find_layer(&self, key: &str) -> Option<usize> {
        self.layers
            .iter()
            .position(|layer| layer.spec.name == key)
            .or_else(|| key.parse::<usize>().ok().filter(|index| *index < self.layers.len()))
    }

    /// Where the puppet's attachment points are this frame, in the space of its mesh.
    pub(crate) fn attachment_places(&self, puppet: &Puppet, out: &mut Vec<crate::math::Mat4>) {
        puppet.attachment_places(&self.playing, out);
    }

    /// Plays the animation layers on by `seconds` and writes this frame's bones.
    pub(crate) fn update(&mut self, puppet: &Puppet, seconds: f32, values: &mut [Vec<f32>]) {
        if !puppet.is_skinned() {
            return;
        }
        self.playing.clear();
        for layer in &mut self.layers {
            layer.advance(seconds);
            if layer.visible {
                self.playing.push(PlayingLayer {
                    animation: layer.spec.animation,
                    time: layer.clock,
                    blend: layer.blend,
                    additive: layer.spec.additive,
                });
            }
        }
        if let Some(bones) = values.get_mut(self.bones) {
            puppet.pose(&self.playing, bones);
        }
        if let Some(alpha) = self.alpha.and_then(|slot| values.get_mut(slot)) {
            puppet.bone_alpha(&self.playing, alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Dyn;

    fn layer(name: &str) -> AnimationLayer {
        let spec = LayerSpec {
            id: 1,
            animation: 7,
            name: name.to_owned(),
            rate: Dyn::constant(2.0),
            blend: Dyn::constant(1.0),
            visible: Dyn::constant(true),
            additive: false,
            blend_in: false,
            blend_out: false,
            blend_time: 0.5,
        };
        AnimationLayer { rate: 2.0, blend: 1.0, visible: true, clock: 0.0, running: true, spec }
    }

    #[test]
    fn a_paused_layer_keeps_its_place_and_a_frame_is_a_place() {
        let mut jump = layer("Jump");
        jump.advance(0.5);
        assert_eq!(jump.clock, 1.0, "at twice its speed");
        jump.running = false;
        jump.advance(0.5);
        assert_eq!(jump.clock, 1.0, "paused, it stays");
        jump.seek(12.0, 24.0);
        assert_eq!(jump.clock, 0.5, "frame 12 at 24 a second is half a second in");
        let skeleton =
            Skeleton { layers: vec![layer("Idle"), jump], bones: 0, alpha: None, playing: Vec::new() };
        assert_eq!(skeleton.find_layer("Jump"), Some(1));
        assert_eq!(skeleton.find_layer("1"), Some(1), "or by its place");
        assert_eq!(skeleton.find_layer("Run"), None);
    }
}
