//! Puppet layers: an image drawn on a mesh that bones bend.
//!
//! The puppet and its mesh are shared ([`crate::share::SharedPuppet`]); what a layer
//! owns is where each of its animation layers is in its animation, and the bones'
//! matrices of this frame, which live in the scene's value table like every other
//! value a shader is fed.

use std::rc::Rc;

use crate::Engine;
use crate::doc::MaterialPass;
use crate::math::Mat4;
use crate::read::Files;
use crate::read::puppet::{LayerSpec, PlayingLayer};
use crate::share::SharedPuppet;

use super::bindings::{Bindings, PuppetField, Slot};
use super::shade::{PassInput, Shade, TextureChoice};

/// One of the puppet's animation layers as it plays.
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
}

/// What an image layer has when it is a puppet.
///
/// Effects work on the flat picture, as they were painted for it; the mesh bends
/// what comes out of them. So a puppet has two passes of its own, both the layer's
/// material with the bones switched on: one that draws the picture straight from
/// its texture (a layer without effects), one that draws the layer's finished buffer.
pub(crate) struct PuppetState {
    pub(crate) shared: Rc<SharedPuppet>,
    /// The picture from its texture, on the mesh.
    pub(crate) direct: Shade,
    /// The layer's finished buffer, on the mesh.
    pub(crate) warp: Shade,
    pub(crate) layers: Vec<AnimationLayer>,
    /// The slot of the value table that holds `g_Bones`.
    bones: usize,
    /// The slot that holds `g_BonesAlpha`, when the puppet fades bones.
    alpha: Option<usize>,
    /// Where the mesh's centre is from the layer's origin.
    pub(crate) offset: [f32; 2],
    playing: Vec<PlayingLayer>,
}

impl PuppetState {
    /// The state of a layer warped with `shared`, playing `specs`, drawn with the
    /// material pass `pass`. `index` is the layer's place in the scene, for what
    /// drives its animation layers.
    pub(crate) fn new(
        engine: &mut Engine,
        files: &Files,
        bindings: &mut Bindings,
        shared: Rc<SharedPuppet>,
        specs: Vec<LayerSpec>,
        pass: &MaterialPass,
        index: usize,
    ) -> Result<Self, String> {
        let puppet = &shared.puppet;
        let mut rest = Vec::new();
        puppet.pose(&[], &mut rest);
        let bones = bindings.value(rest);
        let alpha = puppet.fades_bones().then(|| bindings.value(vec![1.0; puppet.bones.len()]));

        let mut combos = pass.combos.clone();
        let mut slots = Vec::new();
        if puppet.is_skinned() {
            combos.insert("SKINNING".to_owned(), 1);
            combos.insert("BONECOUNT".to_owned(), puppet.bones.len() as i32);
            slots.push(("g_Bones", bones));
            // The shader asks whether this one is defined at all: only when it is needed.
            if let Some(alpha) = alpha {
                combos.insert("SKINNING_ALPHA".to_owned(), 1);
                slots.push(("g_BonesAlpha", alpha));
            }
        }
        let from_texture = super::build::choices(&pass.textures);
        let mut from_buffer = from_texture.clone();
        match from_buffer.first_mut() {
            Some(first) => *first = TextureChoice::Previous,
            None => from_buffer.push(TextureChoice::Previous),
        }
        let constants = super::build::constants(pass);
        let mut compile = |textures: &[TextureChoice]| {
            Shade::compile(
                &mut engine.share,
                files,
                &PassInput {
                    shader: &pass.shader,
                    textures,
                    combos: &combos,
                    constants: &constants,
                    buffers: &[],
                    slots: &slots,
                },
                bindings,
            )
        };
        let direct = compile(&from_texture)?;
        let warp = compile(&from_buffer)?;

        let layers = specs
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
                    spec,
                }
            })
            .collect();
        Ok(Self { shared, direct, warp, layers, bones, alpha, offset: [0.0, 0.0], playing: Vec::new() })
    }

    /// Whether the mesh follows bones; one that does not only replaces the quad.
    pub(crate) fn skinned(&self) -> bool {
        self.shared.puppet.is_skinned()
    }

    /// Plays the animation layers on by `seconds` and writes this frame's bones.
    pub(crate) fn update(&mut self, seconds: f32, values: &mut [Vec<f32>]) {
        if !self.skinned() {
            return;
        }
        self.playing.clear();
        for layer in &mut self.layers {
            layer.clock += seconds * layer.rate;
            if layer.visible {
                self.playing.push(PlayingLayer {
                    animation: layer.spec.animation,
                    time: layer.clock,
                    blend: layer.blend,
                    additive: layer.spec.additive,
                });
            }
        }
        let puppet = &self.shared.puppet;
        if let Some(bones) = values.get_mut(self.bones) {
            puppet.pose(&self.playing, bones);
        }
        if let Some(alpha) = self.alpha.and_then(|slot| values.get_mut(slot)) {
            puppet.bone_alpha(&self.playing, alpha);
        }
    }

    /// The matrix that moves the mesh from its own centre to where the layer's origin wants it.
    pub(crate) fn placement(&self) -> Mat4 {
        Mat4::translation([self.offset[0], self.offset[1], 0.0])
    }
}
