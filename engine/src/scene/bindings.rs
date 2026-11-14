//! What drives the values of a scene: user properties now, scripts and animations
//! through the same table.

use std::collections::HashMap;

use wepaper_json::Value;

use crate::doc::{Animation, AsFloats, Dyn, FromDoc, UserLink};

use super::shade::value_floats;

/// Something a binding writes to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Slot {
    /// A slot of the value table (a shader constant).
    Value(usize),
    /// A layer's visibility.
    LayerVisible(usize),
    /// A layer's opacity.
    LayerAlpha(usize),
    /// A layer's tint.
    LayerColor(usize),
    /// A layer's position.
    LayerOrigin(usize),
    /// A layer's scale.
    LayerScale(usize),
    /// A layer's rotation.
    LayerAngles(usize),
    /// Whether one of a layer's effects runs.
    EffectVisible(usize, usize),
    /// A text layer's text.
    LayerText(usize),
    /// How far the scene's camera is zoomed in.
    Zoom,
    /// Whether the scene blooms.
    Bloom,
}

/// A link from a user property to a slot.
#[derive(Debug, Clone)]
pub(crate) struct Binding {
    pub(crate) link: UserLink,
    pub(crate) slot: Slot,
}

/// A keyframe animation playing on a slot.
#[derive(Debug, Clone)]
pub(crate) struct Playing {
    pub(crate) animation: Animation,
    pub(crate) slot: Slot,
    /// The value written in the document: what the curves offset when they are
    /// relative, and what the components without a curve keep.
    pub(crate) base: Vec<f32>,
    /// How many frames it has played.
    pub(crate) played: f32,
    pub(crate) running: bool,
    /// Its value has to reach the slot even though it is not running.
    pub(crate) stale: bool,
}

/// The table of driven values and what drives them.
#[derive(Default)]
pub(crate) struct Bindings {
    /// The keyframe animations.
    pub(crate) animations: Vec<Playing>,
    /// The driven shader constants.
    pub(crate) values: Vec<Vec<f32>>,
    /// The links from user properties.
    pub(crate) links: Vec<Binding>,
    /// The current value of every user property.
    pub(crate) properties: HashMap<String, Value>,
}

/// A change a property made, for the scene to apply.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Change {
    Floats(Slot, Vec<f32>),
    Flag(Slot, bool),
    Text(Slot, String),
}

impl Bindings {
    /// A new slot in the value table, holding `initial`.
    pub(crate) fn value(&mut self, initial: Vec<f32>) -> usize {
        self.values.push(initial);
        self.values.len() - 1
    }

    /// Registers whatever drives `value` as a writer of `slot`.
    pub(crate) fn link<T: AsFloats>(&mut self, value: &Dyn<T>, slot: Slot) {
        if let Some(link) = &value.user {
            self.links.push(Binding { link: link.clone(), slot });
        }
        if let Some(animation) = &value.animation {
            let base = match slot {
                Slot::Value(index) => self.values.get(index).cloned().unwrap_or_default(),
                _ => value.value.as_floats(),
            };
            let running = !animation.start_paused;
            self.animations.push(Playing { animation: animation.clone(), slot, base, played: 0.0, running, stale: true });
        }
    }

    /// Plays the animations on by `seconds` and returns what they changed.
    pub(crate) fn animate(&mut self, seconds: f32, changes: &mut Vec<Change>) {
        let mut sample = Vec::new();
        for playing in &mut self.animations {
            if !playing.running && !playing.stale {
                continue;
            }
            if playing.running && !playing.stale {
                playing.played += seconds * playing.animation.fps;
            }
            playing.stale = false;
            playing.animation.sample(playing.animation.frame_at(playing.played), &mut sample);
            let mut value = playing.base.clone();
            value.resize(value.len().max(sample.len()), 0.0);
            for (component, curve) in value.iter_mut().zip(&sample) {
                *component = if playing.animation.relative { *component + curve } else { *curve };
            }
            if playing.animation.finished(playing.played) {
                playing.running = false;
            }
            changes.push(match playing.slot {
                Slot::LayerVisible(_) | Slot::EffectVisible(..) | Slot::Bloom => Change::Flag(playing.slot, value.first().is_some_and(|v| *v > 0.5)),
                slot => Change::Floats(slot, value),
            });
        }
    }

    /// The changes that setting `property` to `value` makes.
    pub(crate) fn changes(&self, property: &str, value: &Value) -> Vec<Change> {
        self.links
            .iter()
            .filter(|binding| binding.link.name == property)
            .filter_map(|binding| {
                Some(match binding.slot {
                    Slot::LayerVisible(_) | Slot::EffectVisible(..) | Slot::Bloom => Change::Flag(binding.slot, holds(value, binding.link.condition.as_deref())),
                    Slot::LayerText(_) => Change::Text(binding.slot, String::from_doc(value)?),
                    Slot::Value(index) => Change::Floats(binding.slot, value_floats(value, self.values.get(index)?.len())),
                    Slot::LayerAlpha(_) | Slot::Zoom => Change::Floats(binding.slot, value_floats(value, 1)),
                    Slot::LayerColor(_) | Slot::LayerOrigin(_) | Slot::LayerScale(_) | Slot::LayerAngles(_) => {
                        Change::Floats(binding.slot, value_floats(value, 3))
                    }
                })
            })
            .collect()
    }
}

/// Whether a property's value makes a linked flag true: equal to the link's
/// condition when it has one, truthy otherwise.
pub(crate) fn holds(value: &Value, condition: Option<&str>) -> bool {
    match condition {
        Some(condition) => {
            let text = match value {
                Value::String(s) => s.clone(),
                Value::Bool(b) => u8::from(*b).to_string(),
                other => other.to_json(),
            };
            text == condition || (matches!(value, Value::Bool(true)) && condition == "true") || (matches!(value, Value::Bool(false)) && condition == "false")
        }
        None => bool::from_doc(value).unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(name: &str, condition: Option<&str>) -> Dyn<bool> {
        Dyn { value: true, user: Some(UserLink { name: name.to_owned(), condition: condition.map(str::to_owned) }), script: None, animation: None }
    }

    #[test]
    fn property_reaches_every_slot_linked_to_it() {
        let mut bindings = Bindings::default();
        let slot = bindings.value(vec![0.0; 3]);
        bindings.link(&link("tint", None), Slot::Value(slot));
        bindings.link(&link("tint", None), Slot::LayerColor(4));
        bindings.link(&link("other", None), Slot::LayerAlpha(1));
        let changes = bindings.changes("tint", &Value::from("1 0.5 0"));
        assert_eq!(changes, [Change::Floats(Slot::Value(slot), vec![1.0, 0.5, 0.0]), Change::Floats(Slot::LayerColor(4), vec![1.0, 0.5, 0.0])]);
        assert!(bindings.changes("unknown", &Value::from(1)).is_empty());
    }

    #[test]
    fn animation_drives_its_slot_and_stops_at_the_end() {
        let doc = wepaper_json::parse(
            r#"{"value":"10 20 0","animation":{"relative":true,"options":{"fps":10,"length":10,"mode":"single"},
                "c0":[{"frame":0,"value":0},{"frame":10,"value":5}],"c1":[{"frame":0,"value":1},{"frame":10,"value":1}]}}"#,
        )
        .unwrap();
        let origin: Dyn<[f32; 3]> = Dyn::read(&doc, [0.0; 3]);
        let mut bindings = Bindings::default();
        bindings.link(&origin, Slot::LayerOrigin(2));
        let mut changes = Vec::new();
        // The first frame shows where the animation starts, whatever time has passed.
        bindings.animate(0.3, &mut changes);
        assert_eq!(changes, [Change::Floats(Slot::LayerOrigin(2), vec![10.0, 21.0, 0.0])]);
        changes.clear();
        bindings.animate(0.5, &mut changes);
        assert_eq!(changes, [Change::Floats(Slot::LayerOrigin(2), vec![12.5, 21.0, 0.0])]);
        changes.clear();
        bindings.animate(5.0, &mut changes);
        assert_eq!(changes, [Change::Floats(Slot::LayerOrigin(2), vec![15.0, 21.0, 0.0])]);
        changes.clear();
        bindings.animate(1.0, &mut changes);
        assert!(changes.is_empty(), "a finished animation changes nothing more");
    }

    #[test]
    fn paused_animation_holds_its_first_frame() {
        let doc = wepaper_json::parse(
            r#"{"value":1,"animation":{"options":{"fps":60,"length":60,"mode":"single","startpaused":true,"name":"show"},
                "c0":[{"frame":0,"value":0},{"frame":60,"value":1}]}}"#,
        )
        .unwrap();
        let alpha: Dyn<f32> = Dyn::read(&doc, 1.0);
        let mut bindings = Bindings::default();
        bindings.link(&alpha, Slot::LayerAlpha(0));
        let mut changes = Vec::new();
        bindings.animate(0.5, &mut changes);
        bindings.animate(0.5, &mut changes);
        assert_eq!(changes, [Change::Floats(Slot::LayerAlpha(0), vec![0.0])]);
    }

    #[test]
    fn condition_toggles_visibility() {
        let mut bindings = Bindings::default();
        bindings.link(&link("mode", Some("2")), Slot::LayerVisible(0));
        bindings.link(&link("mode", None), Slot::EffectVisible(1, 0));
        assert_eq!(bindings.changes("mode", &Value::from(2)), [Change::Flag(Slot::LayerVisible(0), true), Change::Flag(Slot::EffectVisible(1, 0), true)]);
        assert_eq!(bindings.changes("mode", &Value::from(1))[0], Change::Flag(Slot::LayerVisible(0), false));
        assert_eq!(bindings.changes("mode", &Value::from(0))[1], Change::Flag(Slot::EffectVisible(1, 0), false));
        assert!(holds(&Value::from("b"), Some("b")) && !holds(&Value::from("a"), Some("b")));
        assert!(holds(&Value::from(true), Some("1")) && holds(&Value::from(false), Some("0")));
    }
}
