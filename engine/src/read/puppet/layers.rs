//! The scene's side of a puppet: which animations an object plays.

use wepaper_json::Value;

use crate::doc::Dyn;

use super::pose::PlayingLayer;

/// One entry of an object's `animationlayers`: an animation of the puppet and how it plays.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerSpec {
    /// The layer's own id (scripts find it by this or by its name).
    pub id: i64,
    /// The id of the animation in the puppet.
    pub animation: i64,
    /// The layer's name.
    pub name: String,
    /// How fast it plays: 1 is the animation's own speed, a negative rate plays backwards.
    pub rate: Dyn<f32>,
    /// How much of the animation shows; see [`PlayingLayer::blend`].
    pub blend: Dyn<f32>,
    /// Whether it plays at all.
    pub visible: Dyn<bool>,
    /// Whether it adds to the layers before it instead of replacing them.
    pub additive: bool,
    /// Whether it fades in over [`blend_time`](Self::blend_time) when it starts.
    pub blend_in: bool,
    /// Whether it fades out over [`blend_time`](Self::blend_time) before it ends.
    pub blend_out: bool,
    /// How long those fades take, in seconds.
    pub blend_time: f32,
}

impl LayerSpec {
    /// The layer as it plays `seconds` after it started, with the values written in
    /// the scene; `None` while it is hidden. A caller that drives `rate`, `blend` or
    /// `visible` (user properties, scripts) builds the [`PlayingLayer`] itself and
    /// adds up the time as the rate changes.
    pub fn playing(&self, seconds: f32) -> Option<PlayingLayer> {
        self.visible.value.then_some(PlayingLayer {
            animation: self.animation,
            time: seconds * self.rate.value,
            blend: self.blend.value,
            additive: self.additive,
        })
    }
}

/// Reads an object's `animationlayers` value into what plays, in the order the layers
/// apply. Anything that is not a list of layers gives none.
pub fn layers_from_scene(doc: &Value) -> Vec<LayerSpec> {
    doc.items()
        .filter(|layer| layer.as_object().is_some())
        .map(|layer| LayerSpec {
            id: layer.get("id").as_i64().unwrap_or(-1),
            animation: layer.get("animation").as_i64().unwrap_or(-1),
            name: layer.get("name").as_str().unwrap_or_default().to_owned(),
            rate: Dyn::read(layer.get("rate"), 1.0),
            blend: Dyn::read(layer.get("blend"), 1.0),
            visible: Dyn::read(layer.get("visible"), true),
            additive: layer.get("additive").as_bool().unwrap_or(false),
            blend_in: layer.get("blendin").as_bool().unwrap_or(false),
            blend_out: layer.get("blendout").as_bool().unwrap_or(false),
            blend_time: layer.get("blendtime").as_f32().unwrap_or(0.5),
        })
        .collect()
}
