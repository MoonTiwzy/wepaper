//! 3D model objects: meshes with their materials, drawn through the scene's camera.
//!
//! A model is one `.mdl` file: its meshes, each with a material, and maybe a skeleton
//! with animations (read like a puppet's, [`crate::read::puppet`]). The file and its
//! mesh on the GPU are shared ([`crate::share::SharedModel`]); a model object owns a
//! pass per mesh and its [`Skeleton`]. Its materials are Wallpaper Engine's own lit
//! shaders (`generic4` and the like), fed the scene's camera and lights.

use std::rc::Rc;

use crate::Engine;
use crate::doc::{MaterialDoc, ModelObject};
use crate::gpu::{Blend, Depth};
use crate::math::Mat4;
use crate::read::Files;
use crate::read::puppet::layers_from_scene;
use crate::share::SharedModel;

use super::bindings::Bindings;
use super::draw::{Canvas, Frame, LayerState, Pictures, apply};
use super::shade::{PassInput, Shade};
use super::skeleton::Skeleton;

/// One mesh of a model, with the pass that draws it.
struct Surface {
    shade: Shade,
    blend: Blend,
    depth: Depth,
    cull: bool,
    first_index: u32,
    index_count: u32,
}

/// A model object of the scene.
pub(crate) struct ModelLayer {
    pub(crate) shared: Rc<SharedModel>,
    surfaces: Vec<Surface>,
    pub(crate) skeleton: Skeleton,
    /// Where its attachment points are this frame, in the space of its mesh.
    pub(crate) places: Vec<Mat4>,
}

impl ModelLayer {
    /// Builds the model object `object`, layer `index` of the scene. A mesh whose
    /// material cannot be built is left out and noted in `skipped`; a model none of
    /// whose meshes can be is an error.
    pub(crate) fn build(
        engine: &mut Engine,
        files: &Files,
        bindings: &mut Bindings,
        object: &ModelObject,
        index: usize,
        skipped: &mut Vec<String>,
    ) -> Result<Self, String> {
        let shared = engine.share.model(files, &object.file)?;
        let model = &shared.model;
        let mut skeleton = Skeleton::new(bindings, model);
        let mut surfaces = Vec::with_capacity(model.surfaces.len());
        for surface in &model.surfaces {
            let built = super::build::read_doc(files, &surface.material).and_then(|doc| {
                let material = MaterialDoc::parse_model(&doc);
                let pass = material.passes.first().ok_or("the material has no passes")?;
                let mut combos = pass.combos.clone();
                let slots = skeleton.inputs(model, &mut combos);
                let shade = Shade::compile(
                    &mut engine.share,
                    files,
                    &PassInput {
                        shader: &pass.shader,
                        textures: &super::build::choices(&pass.textures),
                        combos: &combos,
                        constants: &super::build::constants(pass),
                        buffers: &[],
                        slots: &slots,
                    },
                    bindings,
                )?;
                Ok(Surface {
                    shade,
                    blend: super::build::blend(pass.blending),
                    depth: Depth { test: pass.depth_test, write: pass.depth_write },
                    cull: pass.cull,
                    first_index: surface.first_index,
                    index_count: surface.index_count,
                })
            });
            match built {
                Ok(built) => surfaces.push(built),
                Err(why) => {
                    log::warn!("{}: mesh {} is skipped: {why}", object.file, surface.material);
                    skipped.push(format!("{}: mesh {}: {why}", object.file, surface.material));
                }
            }
        }
        if surfaces.is_empty() {
            return Err("none of its meshes can be drawn".to_owned());
        }
        skeleton.play(bindings, layers_from_scene(&object.animation_layers), index);
        let mut places = Vec::new();
        skeleton.attachment_places(model, &mut places);
        Ok(Self { shared, surfaces, skeleton, places })
    }

    /// Plays the animation layers on by `seconds` and writes this frame's bones.
    pub(crate) fn update(&mut self, seconds: f32, values: &mut [Vec<f32>]) {
        self.skeleton.update(&self.shared.model, seconds, values);
        if !self.places.is_empty() {
            self.skeleton.attachment_places(&self.shared.model, &mut self.places);
        }
    }

    /// The attachment point called `name`, by its place in the model.
    pub(crate) fn attachment(&self, name: &str) -> Option<usize> {
        self.shared.model.attachments.iter().position(|attachment| attachment.name == name)
    }

    /// The shaders' passes, for what the scene asks of every pass it has.
    pub(crate) fn passes(&self) -> impl Iterator<Item = &Shade> {
        self.surfaces.iter().map(|surface| &surface.shade)
    }

    /// Draws every mesh into `canvas`, placed by `world`.
    pub(super) fn draw(
        &self,
        engine: &Engine,
        frame: &Frame<'_>,
        world: &Mat4,
        canvas: Canvas,
        background: Option<&crate::gpu::Texture>,
        pictures: &Pictures,
    ) {
        let state = LayerState {
            mvp: frame.view_projection.mul(world),
            model: *world,
            color: [1.0; 3],
            alpha: 1.0,
            brightness: 1.0,
            authored: frame.screen,
            vars: [[0.0; 4]; 2],
        };
        canvas.bind(&engine.gl);
        for surface in &self.surfaces {
            engine.gl.set_blend(surface.blend);
            engine.gl.set_depth(surface.depth);
            engine.gl.set_cull(surface.cull);
            apply(&engine.gl, &surface.shade, frame, &state, None, &[], background, pictures, engine.white());
            self.shared.mesh.draw_range(surface.first_index, surface.index_count);
        }
    }
}
