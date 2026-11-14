//! Puppet layers: an image drawn on a mesh that bones bend.
//!
//! The puppet and its mesh are shared ([`crate::share::SharedPuppet`]); what a layer
//! owns is its [`Skeleton`]: where each of its animation layers is in its animation,
//! and the bones' matrices of this frame.

use std::rc::Rc;

use crate::Engine;
use crate::doc::MaterialPass;
use crate::math::Mat4;
use crate::read::Files;
use crate::read::puppet::LayerSpec;
use crate::share::SharedPuppet;

use super::bindings::Bindings;
use super::shade::{PassInput, Shade, TextureChoice};
use super::skeleton::Skeleton;

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
    pub(crate) skeleton: Skeleton,
    /// Where the mesh's centre is from the layer's origin.
    pub(crate) offset: [f32; 2],
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
        let mut skeleton = Skeleton::new(bindings, puppet);
        let mut combos = pass.combos.clone();
        let slots = skeleton.inputs(puppet, &mut combos);
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
        skeleton.play(bindings, specs, index);
        Ok(Self { shared, direct, warp, skeleton, offset: [0.0, 0.0] })
    }

    /// Plays the animation layers on by `seconds` and writes this frame's bones.
    pub(crate) fn update(&mut self, seconds: f32, values: &mut [Vec<f32>]) {
        self.skeleton.update(&self.shared.puppet, seconds, values);
    }

    /// The matrix that moves the mesh from its own centre to where the layer's origin wants it.
    pub(crate) fn placement(&self) -> Mat4 {
        Mat4::translation([self.offset[0], self.offset[1], 0.0])
    }
}
