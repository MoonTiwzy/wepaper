//! The scene as a tree of values that can be read and changed by path.
//!
//! This is the wallpaper's part of the daemon's control tree: every layer, every
//! effect, every user property is a node. Reading builds a snapshot; writing changes
//! the playing scene in place.

use wepaper_json::{Map, Value, obj};

use crate::doc::{FromDoc, PropertyKind};

use super::shade::value_floats;
use super::{LayerKind, SceneInstance};

fn vector(values: &[f32]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(*v)).collect())
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
            match &layer.kind {
                LayerKind::Image(image) => {
                    node.insert("kind", Value::from("image"));
                    node.insert("alpha", Value::from(image.alpha));
                    node.insert("color", vector(&image.color));
                    node.insert("brightness", Value::from(image.brightness));
                    node.insert("size", vector(&image.size));
                    let effects: Map =
                        image.effects.iter().enumerate().map(|(index, effect)| (index.to_string(), obj! { "name" => &effect.name, "visible" => effect.visible })).collect();
                    node.insert("effects", Value::Object(effects));
                }
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
                        let options = options.iter().map(|(label, value)| obj! { "label" => label, "value" => value.clone() }).collect::<Vec<_>>();
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
        obj! {
            "size" => vector(&self.scene_size),
            "properties" => Value::Object(properties),
            "schema" => Value::Array(schema),
            "layers" => Value::Object(layers),
            "skipped" => Value::Array(self.skipped.iter().map(Value::from).collect()),
        }
    }

    /// Changes the value at `path` (relative to the wallpaper): a user property
    /// (`properties/<name>`), a layer's field (`layers/<id>/alpha`), or whether an
    /// effect runs (`layers/<id>/effects/<n>/visible`).
    pub fn edit(&mut self, path: &[&str], value: &Value) -> Result<(), String> {
        match path {
            ["properties", name] => {
                if !self.project.properties.iter().any(|p| p.name == *name) {
                    return Err(format!("the wallpaper has no property \"{name}\""));
                }
                self.set_property(name, value);
                Ok(())
            }
            ["layers", id, rest @ ..] => {
                let index = id.parse::<i64>().ok().and_then(|id| self.layers.iter().position(|l| l.id == id)).ok_or_else(|| format!("no layer with id {id}"))?;
                let layer = &mut self.layers[index];
                let flag = || bool::from_doc(value).ok_or_else(|| "expected true or false".to_owned());
                let three = || -> Result<[f32; 3], String> {
                    let floats = value_floats(value, 3);
                    if value.is_null() { Err("expected three numbers".to_owned()) } else { Ok([floats[0], floats[1], floats[2]]) }
                };
                let number = || f32::from_doc(value).ok_or_else(|| "expected a number".to_owned());
                match rest {
                    ["visible"] => layer.visible = flag()?,
                    ["origin"] => layer.origin = three()?,
                    ["scale"] => layer.scale = three()?,
                    ["angles"] => layer.angles = three()?,
                    [field @ ("alpha" | "color" | "brightness")] => {
                        let LayerKind::Image(image) = &mut layer.kind else { return Err(format!("layer {id} has no {field}")) };
                        match *field {
                            "alpha" => image.alpha = number()?,
                            "brightness" => image.brightness = number()?,
                            _ => image.color = three()?,
                        }
                    }
                    ["effects", effect, "visible"] => {
                        let LayerKind::Image(image) = &mut layer.kind else { return Err(format!("layer {id} has no effects")) };
                        let effect = effect.parse::<usize>().ok().and_then(|n| image.effects.get_mut(n)).ok_or_else(|| format!("layer {id} has no effect {effect}"))?;
                        effect.visible = flag()?;
                    }
                    _ => return Err(format!("layers/{id}/{} cannot be set", rest.join("/"))),
                }
                Ok(())
            }
            _ => Err(format!("{} cannot be set", path.join("/"))),
        }
    }
}
