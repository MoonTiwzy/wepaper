//! Scene wallpapers: a scene document becomes layers, layers become passes.
//!
//! A [`SceneInstance`] is one playing copy of a scene, for one output. It owns only
//! what is its own: transforms, the values properties drive, its scene target.
//! Textures and programs come from the engine's cache and are shared with every
//! other instance; effect buffers are borrowed from the engine's pool for the time
//! a layer is being drawn.

mod bindings;
mod build;
mod draw;
mod shade;

use wepaper_json::Value;

use crate::doc::Project;
use crate::gpu::{Blend, Mesh, Target};
use crate::math::Mat4;
use crate::{Fill, Viewport};

use bindings::{Bindings, Change, Slot};
use shade::Shade;

/// How an image layer gets its picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageMode {
    /// From its own texture.
    Normal,
    /// From the scene behind it, within its own rectangle.
    Compose,
    /// From the whole scene behind it.
    Fullscreen,
}

/// One step of an effect.
pub(crate) enum Step {
    /// Draw a pass into the chain's next buffer, or into a named one.
    Draw { shade: Shade, target: Option<usize> },
    /// Copy one named buffer into another.
    Copy { source: usize, target: usize },
}

/// An effect on a layer.
pub(crate) struct Effect {
    pub(crate) name: String,
    pub(crate) visible: bool,
    pub(crate) steps: Vec<Step>,
}

/// A buffer an effect declares, by name.
pub(crate) struct BufferSpec {
    pub(crate) name: String,
    /// How many times smaller than the layer it is.
    pub(crate) scale: f32,
}

/// An image layer.
pub(crate) struct ImageLayer {
    pub(crate) mode: ImageMode,
    /// The layer's size in scene units.
    pub(crate) size: [f32; 2],
    pub(crate) alpha: f32,
    pub(crate) color: [f32; 3],
    pub(crate) brightness: f32,
    pub(crate) blend: Blend,
    pub(crate) base: Shade,
    pub(crate) effects: Vec<Effect>,
    pub(crate) buffers: Vec<BufferSpec>,
    /// Layer space, texture coordinates over the picture.
    pub(crate) quad: Mesh,
    /// Layer space, texture coordinates over a whole buffer.
    pub(crate) quad_unit: Mesh,
    /// Clip space, texture coordinates over the picture.
    pub(crate) base_quad: Mesh,
}

/// What a layer is.
pub(crate) enum LayerKind {
    Image(Box<ImageLayer>),
    /// Nothing drawn (yet): keeps the hierarchy and its transforms.
    Group,
}

/// One object of the scene, as it plays.
pub(crate) struct Layer {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) parent: Option<usize>,
    pub(crate) visible: bool,
    pub(crate) origin: [f32; 3],
    pub(crate) scale: [f32; 3],
    pub(crate) angles: [f32; 3],
    pub(crate) parallax_depth: [f32; 2],
    /// The layer's place in the scene, parents included.
    pub(crate) world: Mat4,
    /// Whether it and every parent are visible.
    pub(crate) shown: bool,
    pub(crate) kind: LayerKind,
}

/// What a frame is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameInput {
    /// Seconds since the wallpaper started.
    pub time: f64,
    /// The pointer over the output, 0 to 1 from the top left.
    pub pointer: [f32; 2],
    /// The time of day, 0 (midnight) to 1.
    pub daytime: f32,
}

/// A user property and its current value.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyValue {
    /// The property's name.
    pub name: String,
    /// Its current value.
    pub value: Value,
}

/// One playing copy of a scene.
pub struct SceneInstance {
    project: Project,
    layers: Vec<Layer>,
    bindings: Bindings,
    /// The scene's own size.
    scene_size: [f32; 2],
    clear: [f32; 3],
    viewport: Viewport,
    fill: Fill,
    target: Target,
    view_projection: Mat4,
    /// Layers that could not be built, with why: for `inspect` and logs.
    skipped: Vec<String>,
    /// The videos the scene's layers show, held for as long as it plays.
    videos: Vec<crate::video::VideoHold>,
}

impl SceneInstance {
    /// The wallpaper's `project.json`.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The scene's own size, in its units.
    pub fn scene_size(&self) -> [f32; 2] {
        self.scene_size
    }

    /// What could not be loaded, one line each.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// How many layers there are.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// The current values of the user properties.
    pub fn properties(&self) -> Vec<PropertyValue> {
        self.project
            .properties
            .iter()
            .map(|p| PropertyValue { name: p.name.clone(), value: self.bindings.properties.get(&p.name).cloned().unwrap_or_else(|| p.default.clone()) })
            .collect()
    }

    /// Sets a user property. Everything linked to it changes in place; nothing reloads.
    pub fn set_property(&mut self, name: &str, value: &Value) {
        self.bindings.properties.insert(name.to_owned(), value.clone());
        for change in self.bindings.changes(name, value) {
            self.apply(change);
        }
    }

    fn apply(&mut self, change: Change) {
        match change {
            Change::Floats(Slot::Value(index), floats) => {
                if let Some(value) = self.bindings.values.get_mut(index) {
                    *value = floats;
                }
            }
            Change::Floats(slot, floats) => {
                let three = [floats.first().copied().unwrap_or(0.0), floats.get(1).copied().unwrap_or(0.0), floats.get(2).copied().unwrap_or(0.0)];
                match slot {
                    Slot::LayerOrigin(layer) => self.layers[layer].origin = three,
                    Slot::LayerScale(layer) => self.layers[layer].scale = three,
                    Slot::LayerAngles(layer) => self.layers[layer].angles = three,
                    Slot::LayerAlpha(layer) => {
                        if let LayerKind::Image(image) = &mut self.layers[layer].kind {
                            image.alpha = three[0];
                        }
                    }
                    Slot::LayerColor(layer) => {
                        if let LayerKind::Image(image) = &mut self.layers[layer].kind {
                            image.color = three;
                        }
                    }
                    _ => {}
                }
            }
            Change::Flag(Slot::LayerVisible(layer), on) => self.layers[layer].visible = on,
            Change::Flag(Slot::EffectVisible(layer, effect), on) => {
                if let LayerKind::Image(image) = &mut self.layers[layer].kind
                    && let Some(effect) = image.effects.get_mut(effect)
                {
                    effect.visible = on;
                }
            }
            Change::Flag(..) | Change::Text(..) => {}
        }
    }

    /// Changes the size the scene is drawn at.
    pub fn resize(&mut self, engine: &crate::Engine, viewport: Viewport) -> Result<(), crate::LoadError> {
        if viewport != self.viewport {
            self.target = Target::new(&engine.gl, viewport.width.max(1), viewport.height.max(1), crate::gpu::TextureFormat::Rgba8)
                .map_err(|e| crate::LoadError(e.to_string()))?;
            self.viewport = viewport;
        }
        Ok(())
    }

    /// Pauses or resumes the scene's videos. A video shared with another output
    /// stops only when every output showing it is paused.
    pub fn set_paused(&mut self, paused: bool) {
        for video in &self.videos {
            video.set_paused(paused);
        }
    }

    /// Changes how the scene fills an output of another shape.
    pub fn set_fill(&mut self, fill: Fill) {
        self.fill = fill;
    }
}
