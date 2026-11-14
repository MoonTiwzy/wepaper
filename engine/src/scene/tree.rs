//! The scene as a tree of values that can be read and changed by path.
//!
//! This is the wallpaper's part of the daemon's control tree: every layer, every
//! effect, every user property is a node. Reading builds a snapshot; writing changes
//! the playing scene in place.

use wepaper_json::{Map, Value, obj};

use crate::doc::{FromDoc, PropertyKind, typed_flag, typed_floats};

use super::bindings::{Change, SceneField, Slot};
use super::shade::Shade;
use super::{LayerKind, SceneInstance, Step};

fn vector(values: &[f32]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(*v)).collect())
}

/// One number as a number, several as a list.
fn number_or_vector(values: &[f32]) -> Value {
    match values {
        [one] => Value::from(*one),
        many => vector(many),
    }
}

/// The constants of some passes, by name. A name several passes share is one node.
fn constants<'a>(shades: impl Iterator<Item = &'a Shade>, values: &[Vec<f32>]) -> Value {
    let mut node = Map::new();
    for shade in shades {
        for (name, feed) in &shade.constants {
            if node.get(name).is_none() {
                node.insert(name.as_str(), number_or_vector(shade.constant(*feed, values)));
            }
        }
    }
    Value::Object(node)
}

impl SceneInstance {
    /// A snapshot of everything readable: the scene's size, its user properties with
    /// their descriptions, and its layers with their effects.
    pub fn tree(&self) -> Value {
        let mut layers = Map::new();
        for layer in &self.layers {
            let mut node = Map::new();
            node.insert("name", Value::from(&layer.name));
            node.insert("visible", Value::from(layer.visible));
            node.insert("origin", vector(&layer.origin));
            node.insert("scale", vector(&layer.scale));
            node.insert("angles", vector(&layer.angles));
            node.insert("parallax_depth", vector(&layer.parallax_depth));
            match &layer.kind {
                LayerKind::Image(image) => {
                    node.insert("kind", Value::from("image"));
                    node.insert("alpha", Value::from(image.alpha));
                    node.insert("color", vector(&image.color));
                    node.insert("brightness", Value::from(image.brightness));
                    node.insert("size", vector(&image.size));
                    if let Some(text) = &image.text {
                        node.insert("kind", Value::from("text"));
                        node.insert("text", Value::from(&text.text));
                    }
                    node.insert("constants", constants(std::iter::once(&image.base), &self.bindings.values));
                    let effects: Map = image
                        .effects
                        .iter()
                        .enumerate()
                        .map(|(index, effect)| {
                            let node = obj! { "name" => &effect.name, "visible" => effect.visible, "constants" => constants(effect.passes(), &self.bindings.values) };
                            (index.to_string(), node)
                        })
                        .collect();
                    node.insert("effects", Value::Object(effects));
                }
                LayerKind::Model(model) => {
                    node.insert("kind", Value::from("model"));
                    node.insert("constants", constants(model.passes(), &self.bindings.values));
                }
                LayerKind::Sound(sound) => {
                    node.insert("kind", Value::from("sound"));
                    node.insert("volume", Value::from(sound.volume));
                    node.insert("playing", Value::from(sound.is_playing()));
                    node.insert("heard", Value::from(sound.sounding()));
                }
                LayerKind::Particles(_) => node.insert("kind", Value::from("particles")),
                LayerKind::Group => node.insert("kind", Value::from("group")),
            }
            layers.insert(layer.id.to_string(), Value::Object(node));
        }
        let properties: Map = self.properties().into_iter().map(|p| (p.name, p.value)).collect();
        let schema: Vec<Value> = self
            .project
            .properties
            .iter()
            .map(|property| {
                let mut node = Map::new();
                node.insert("name", Value::from(&property.name));
                node.insert("label", Value::from(&property.label));
                node.insert("default", property.default.clone());
                node.insert("condition", Value::from(property.condition.clone()));
                let kind = match &property.kind {
                    PropertyKind::Slider { min, max, step } => {
                        node.insert("min", Value::from(*min));
                        node.insert("max", Value::from(*max));
                        node.insert("step", Value::from(*step));
                        "slider"
                    }
                    PropertyKind::Bool => "bool",
                    PropertyKind::Color => "color",
                    PropertyKind::Combo(options) => {
                        let options = options
                            .iter()
                            .map(|(label, value)| obj! { "label" => label, "value" => value.clone() })
                            .collect::<Vec<_>>();
                        node.insert("options", Value::Array(options));
                        "combo"
                    }
                    PropertyKind::Text => "text",
                    PropertyKind::File => "file",
                    PropertyKind::SceneTexture => "texture",
                    PropertyKind::Group => "group",
                    PropertyKind::Other(name) => name.as_str(),
                };
                node.insert("type", Value::from(kind));
                Value::Object(node)
            })
            .collect();
        let mut scene = Map::new();
        for (name, field) in SceneField::ALL {
            scene.insert(name, self.scene_value(field));
        }
        obj! {
            "size" => vector(&self.scene_size),
            "scene" => Value::Object(scene),
            "properties" => Value::Object(properties),
            "schema" => Value::Array(schema),
            "layers" => Value::Object(layers),
            "skipped" => Value::Array(self.skipped.iter().map(Value::from).collect()),
        }
    }

    /// The current value of one of the scene's own fields.
    pub(super) fn scene_value(&self, field: SceneField) -> Value {
        match field {
            SceneField::Zoom => Value::from(self.zoom),
            SceneField::Clear => vector(&self.clear),
            SceneField::Bloom => Value::from(
                self.bloom.as_ref().is_some_and(|bloom| bloom.enabled)
                    || self.hdr_bloom.as_ref().is_some_and(|bloom| bloom.enabled),
            ),
            SceneField::Parallax => Value::from(self.parallax.enabled),
            SceneField::ParallaxAmount => Value::from(self.parallax.amount),
            SceneField::ParallaxDelay => Value::from(self.parallax.delay),
            SceneField::ParallaxMouse => Value::from(self.parallax.mouse_influence),
            SceneField::Shake => Value::from(self.shake.enabled),
            SceneField::ShakeAmplitude => Value::from(self.shake.amplitude),
            SceneField::ShakeRoughness => Value::from(self.shake.roughness),
            SceneField::ShakeSpeed => Value::from(self.shake.speed),
            SceneField::Ambient => vector(&self.ambient),
            SceneField::Skylight => vector(&self.skylight),
        }
    }

    /// Changes the value at `path` (relative to the wallpaper): a user property
    /// (`properties/<name>`), one of the scene's own values (`scene/zoom`), a layer's
    /// field (`layers/<id>/alpha`), a shader constant of a layer or of one of its
    /// effects (`layers/<id>/effects/<n>/constants/<name>`), or whether an effect
    /// runs (`layers/<id>/effects/<n>/visible`).
    pub fn edit(&mut self, path: &[&str], value: &Value) -> Result<(), String> {
        match path {
            ["scene", name] => {
                let (_, field) = SceneField::ALL
                    .into_iter()
                    .find(|(known, _)| known == name)
                    .ok_or_else(|| format!("the scene has no value \"{name}\""))?;
                if field.is_flag() {
                    let on = typed_flag(value).ok_or("expected true or false")?;
                    if field == SceneField::Bloom && on && self.bloom.is_none() {
                        return Err("the scene was made without bloom".to_owned());
                    }
                    self.apply(Change::Flag(Slot::Scene(field), on));
                } else {
                    let mut numbers = typed_floats(value).ok_or("expected a number")?;
                    // One number for a colour means that number in every component.
                    if let [one] = numbers[..] {
                        numbers = vec![one; 3];
                    }
                    self.apply(Change::Floats(Slot::Scene(field), numbers));
                }
                Ok(())
            }
            ["properties", name] => {
                if !self.project.properties.iter().any(|p| p.name == *name) {
                    return Err(format!("the wallpaper has no property \"{name}\""));
                }
                self.set_property(name, value);
                Ok(())
            }
            ["layers", id, rest @ ..] => {
                let index = id
                    .parse::<i64>()
                    .ok()
                    .and_then(|id| self.layers.iter().position(|l| l.id == id))
                    .ok_or_else(|| format!("no layer with id {id}"))?;
                let layer = &mut self.layers[index];
                let flag = || typed_flag(value).ok_or_else(|| "expected true or false".to_owned());
                let numbers = typed_floats(value);
                let three = || -> Result<[f32; 3], String> {
                    match numbers.as_deref() {
                        Some([one]) => Ok([*one; 3]),
                        Some([x, y]) => Ok([*x, *y, 0.0]),
                        Some([x, y, z, ..]) => Ok([*x, *y, *z]),
                        _ => Err("expected three numbers".to_owned()),
                    }
                };
                let number = || {
                    numbers
                        .as_deref()
                        .and_then(<[f32]>::first)
                        .copied()
                        .ok_or_else(|| "expected a number".to_owned())
                };
                match rest {
                    ["visible"] => layer.visible = flag()?,
                    ["origin"] => layer.origin = three()?,
                    ["scale"] => layer.scale = three()?,
                    ["angles"] => layer.angles = three()?,
                    ["parallax_depth"] => {
                        let depth = three()?;
                        layer.parallax_depth = [depth[0], depth[1]];
                    }
                    ["volume"] => {
                        let LayerKind::Sound(sound) = &mut layer.kind else {
                            return Err(format!("layer {id} is not a sound"));
                        };
                        sound.volume = number()?.clamp(0.0, 1.0);
                    }
                    ["constants", name] => {
                        let LayerKind::Image(image) = &mut layer.kind else {
                            return Err(format!("layer {id} has no constants"));
                        };
                        if numbers.is_none() {
                            return Err("expected a number or several".to_owned());
                        }
                        if !image.base.set_constant(name, value, &mut self.bindings.values) {
                            return Err(format!("layer {id} has no constant \"{name}\""));
                        }
                    }
                    ["effects", effect, "constants", name] => {
                        let LayerKind::Image(image) = &mut layer.kind else {
                            return Err(format!("layer {id} has no effects"));
                        };
                        let found = effect
                            .parse::<usize>()
                            .ok()
                            .and_then(|n| image.effects.get_mut(n))
                            .ok_or_else(|| format!("layer {id} has no effect {effect}"))?;
                        if numbers.is_none() {
                            return Err("expected a number or several".to_owned());
                        }
                        let mut any = false;
                        for step in &mut found.steps {
                            if let Step::Draw { shade, .. } = step {
                                any |= shade.set_constant(name, value, &mut self.bindings.values);
                            }
                        }
                        if !any {
                            return Err(format!("effect {effect} of layer {id} has no constant \"{name}\""));
                        }
                    }
                    [field @ ("alpha" | "color" | "brightness")] => {
                        let LayerKind::Image(image) = &mut layer.kind else {
                            return Err(format!("layer {id} has no {field}"));
                        };
                        match *field {
                            "alpha" => image.alpha = number()?,
                            "brightness" => image.brightness = number()?,
                            _ => image.color = three()?,
                        }
                    }
                    ["text"] => {
                        let LayerKind::Image(image) = &mut layer.kind else {
                            return Err(format!("layer {id} has no text"));
                        };
                        let text = image.text.as_mut().ok_or_else(|| format!("layer {id} has no text"))?;
                        text.text = String::from_doc(value).ok_or("expected text")?;
                        text.dirty = true;
                    }
                    ["effects", effect, "visible"] => {
                        let LayerKind::Image(image) = &mut layer.kind else {
                            return Err(format!("layer {id} has no effects"));
                        };
                        let effect = effect
                            .parse::<usize>()
                            .ok()
                            .and_then(|n| image.effects.get_mut(n))
                            .ok_or_else(|| format!("layer {id} has no effect {effect}"))?;
                        effect.visible = flag()?;
                    }
                    _ => return Err(format!("layers/{id}/{} cannot be set", rest.join("/"))),
                }
                if rest == ["volume"] {
                    // The player hears of its layer's new volume.
                    let (on, volume) = self.sound;
                    self.set_sound(on, volume);
                }
                Ok(())
            }
            _ => Err(format!("{} cannot be set", path.join("/"))),
        }
    }
}
