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
mod text;
mod tree;

use wepaper_json::Value;

use crate::doc::Project;
use crate::gpu::{Blend, Mesh};
use crate::math::Mat4;
use crate::{Fill, Viewport};

pub(crate) use text::system_font;

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
    /// The layer's text, when its picture is text.
    pub(crate) text: Option<text::TextState>,
    /// The pass that puts the layer's picture into the frame with a blend mode
    /// (multiply, screen, ...) instead of plain blending, when it has one.
    pub(crate) blend_pass: Option<Shade>,
}

/// What a layer is.
pub(crate) enum LayerKind {
    Image(Box<ImageLayer>),
    /// A sound, playing while the engine's sound is on.
    Sound(SoundLayer),
    /// Nothing drawn (yet): keeps the hierarchy and its transforms.
    Group,
}

/// A sound layer: the file it plays and, while sound is on, the player.
pub(crate) struct SoundLayer {
    pub(crate) file: Option<std::path::PathBuf>,
    pub(crate) looping: bool,
    /// The layer's own volume, 0 to 1.
    pub(crate) volume: f32,
    pub(crate) player: Option<crate::audio::Sound>,
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

/// The passes of the scene's bloom, in the order they run.
pub(crate) struct Bloom {
    /// Keeps what is brighter than the threshold, at a quarter of the size.
    pub(crate) bright: Shade,
    /// Blurs across, down to an eighth of the size.
    pub(crate) across: Shade,
    /// Blurs down.
    pub(crate) down: Shade,
    /// Adds the result to the frame.
    pub(crate) combine: Shade,
    pub(crate) enabled: bool,
}

/// The scene's camera parallax settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Parallax {
    pub(crate) amount: f32,
    /// Seconds the camera takes to catch up with the pointer.
    pub(crate) delay: f32,
    pub(crate) mouse_influence: f32,
}

/// The scene's camera shake settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shake {
    pub(crate) amplitude: f32,
    pub(crate) roughness: f32,
    pub(crate) speed: f32,
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
    /// How far the camera is zoomed in; 1 shows the whole scene.
    zoom: f32,
    viewport: Viewport,
    fill: Fill,
    view_projection: Mat4,
    /// Layers that could not be built, with why: for `inspect` and logs.
    skipped: Vec<String>,
    /// The scene's bloom: bright parts of the finished frame, blurred and added back.
    bloom: Option<Bloom>,
    /// The ids of layers whose finished picture another layer reads.
    provides: std::collections::HashSet<i64>,
    /// The videos the scene's layers show, held for as long as it plays.
    videos: Vec<crate::video::VideoHold>,
    /// The camera follows the pointer, shifting layers by their depth.
    parallax: Option<Parallax>,
    /// The pointer as the parallax sees it: eased towards where it really is.
    eased_pointer: [f32; 2],
    /// The camera shakes.
    shake: Option<Shake>,
    /// The time of the last frame, for what eases over time.
    last_time: f64,
    /// A shader of the scene reads the audio spectrum.
    wants_audio: bool,
    paused: bool,
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
                    Slot::Zoom => self.zoom = three[0].max(0.01),
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
            Change::Flag(Slot::Bloom, on) => {
                if let Some(bloom) = &mut self.bloom {
                    bloom.enabled = on;
                }
            }
            Change::Flag(Slot::EffectVisible(layer, effect), on) => {
                if let LayerKind::Image(image) = &mut self.layers[layer].kind
                    && let Some(effect) = image.effects.get_mut(effect)
                {
                    effect.visible = on;
                }
            }
            Change::Text(Slot::LayerText(layer), new) => {
                if let LayerKind::Image(image) = &mut self.layers[layer].kind
                    && let Some(text) = &mut image.text
                    && text.text != new
                {
                    text.text = new;
                    text.dirty = true;
                }
            }
            Change::Flag(..) | Change::Text(..) => {}
        }
    }

    /// Pauses or resumes the scene's videos and sounds. A video shared with another
    /// output stops only when every output showing it is paused.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        for video in &self.videos {
            video.set_paused(paused);
        }
        for layer in &self.layers {
            if let LayerKind::Sound(SoundLayer { player: Some(player), .. }) = &layer.kind {
                player.set_paused(paused);
            }
        }
    }

    /// Whether the scene reads the audio spectrum, so that somebody has to listen.
    pub fn wants_audio(&self) -> bool {
        self.wants_audio
    }

    /// Switches the scene's sound layers on or off and sets their volume (0 to 100).
    /// Off, nothing is decoded: there are no players at all.
    pub fn set_sound(&mut self, on: bool, volume: f32) {
        for layer in &mut self.layers {
            let visible = layer.visible;
            let LayerKind::Sound(sound) = &mut layer.kind else { continue };
            let level = volume * sound.volume;
            match (&sound.player, on && visible) {
                (Some(player), true) => player.set_volume(level),
                (Some(_), false) => sound.player = None,
                (None, true) => {
                    sound.player = sound.file.as_ref().and_then(|file| crate::audio::Sound::play(file, sound.looping, level));
                    if let Some(player) = &sound.player {
                        player.set_paused(self.paused);
                    }
                }
                (None, false) => {}
            }
        }
    }

    /// Changes how the scene fills an output of another shape.
    pub fn set_fill(&mut self, fill: Fill) {
        self.fill = fill;
    }
}
