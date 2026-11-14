//! Values that may be linked to something that changes them.

use wepaper_json::Value;

use super::Animation;

/// A link from a value to a user property.
#[derive(Debug, Clone, PartialEq)]
pub struct UserLink {
    /// The property's name.
    pub name: String,
    /// For visibility: the expression over the property that must hold, e.g. `"2"`.
    pub condition: Option<String>,
}

/// A SceneScript attached to a value.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// The script's source.
    pub source: String,
    /// The values of the properties the script declares.
    pub properties: Value,
}

/// A value and what may drive it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dyn<T> {
    /// The value written in the document: what holds while nothing drives it.
    pub value: T,
    /// The user property it follows.
    pub user: Option<UserLink>,
    /// The script that computes it.
    pub script: Option<Script>,
    /// The keyframe animation that plays on it.
    pub animation: Option<Animation>,
}

impl<T> Dyn<T> {
    /// A value nothing drives.
    pub fn constant(value: T) -> Self {
        Self { value, user: None, script: None, animation: None }
    }

    /// Whether anything can change the value after loading.
    pub fn is_driven(&self) -> bool {
        self.user.is_some() || self.script.is_some() || self.animation.is_some()
    }
}

impl<T: FromDoc> Dyn<T> {
    /// Reads a field that is either a literal or an object with `value` and links.
    pub fn read(doc: &Value, default: T) -> Self {
        let linked = doc.as_object().is_some_and(|o| ["value", "user", "script", "animation"].iter().any(|k| o.get(k).is_some()));
        if !linked {
            return Self::constant(T::from_doc(doc).unwrap_or(default));
        }
        let user = match doc.get("user") {
            Value::String(name) => Some(UserLink { name: name.clone(), condition: None }),
            link @ Value::Object(_) => link.get("name").as_str().map(|name| UserLink {
                name: name.to_owned(),
                condition: link.get("condition").as_str().map(str::to_owned),
            }),
            _ => None,
        };
        let script = doc
            .get("script")
            .as_str()
            .map(|source| Script { source: source.to_owned(), properties: doc.get("scriptproperties").clone() });
        let animation = Animation::read(doc.get("animation"));
        Self { value: T::from_doc(doc.get("value")).unwrap_or(default), user, script, animation }
    }
}

/// The numbers in a string such as `"0.5 1 -2"`.
pub fn floats(text: &str) -> Vec<f32> {
    text.split([' ', ',', '\t']).filter(|part| !part.is_empty()).map(|part| part.parse().unwrap_or(0.0)).collect()
}

/// A value as the numbers an animation or a shader works with.
pub trait AsFloats {
    /// The value's components; empty when it is not a number.
    fn as_floats(&self) -> Vec<f32>;
}

impl AsFloats for f32 {
    fn as_floats(&self) -> Vec<f32> {
        vec![*self]
    }
}

impl AsFloats for bool {
    fn as_floats(&self) -> Vec<f32> {
        vec![f32::from(u8::from(*self))]
    }
}

impl AsFloats for String {
    fn as_floats(&self) -> Vec<f32> {
        Vec::new()
    }
}

impl<const N: usize> AsFloats for [f32; N] {
    fn as_floats(&self) -> Vec<f32> {
        self.to_vec()
    }
}

impl AsFloats for Value {
    fn as_floats(&self) -> Vec<f32> {
        match self {
            Value::String(s) => floats(s),
            Value::Number(n) => vec![*n as f32],
            Value::Bool(b) => b.as_floats(),
            Value::Array(items) => items.iter().filter_map(f32::from_doc).collect(),
            _ => Vec::new(),
        }
    }
}

/// A type a document value converts to, as loosely as Wallpaper Engine reads it.
pub trait FromDoc: Sized {
    /// The value, or `None` when the document has nothing usable.
    fn from_doc(doc: &Value) -> Option<Self>;
}

impl FromDoc for f32 {
    fn from_doc(doc: &Value) -> Option<Self> {
        match doc {
            Value::Number(n) => Some(*n as f32),
            Value::Bool(b) => Some(f32::from(u8::from(*b))),
            Value::String(s) => floats(s).first().copied(),
            _ => None,
        }
    }
}

impl FromDoc for i32 {
    fn from_doc(doc: &Value) -> Option<Self> {
        f32::from_doc(doc).map(|n| n as i32)
    }
}

impl FromDoc for bool {
    fn from_doc(doc: &Value) -> Option<Self> {
        match doc {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => Some(*n != 0.0),
            Value::String(s) => Some(!matches!(s.as_str(), "" | "0" | "false")),
            _ => None,
        }
    }
}

impl FromDoc for String {
    fn from_doc(doc: &Value) -> Option<Self> {
        match doc {
            Value::String(s) => Some(s.clone()),
            Value::Number(_) | Value::Bool(_) => Some(doc.to_json()),
            _ => None,
        }
    }
}

impl<const N: usize> FromDoc for [f32; N] {
    /// A string of numbers, an array, or one number for every component.
    fn from_doc(doc: &Value) -> Option<Self> {
        let numbers: Vec<f32> = match doc {
            Value::String(s) => floats(s),
            Value::Number(n) => vec![*n as f32],
            Value::Array(items) => items.iter().filter_map(f32::from_doc).collect(),
            _ => return None,
        };
        match numbers.as_slice() {
            [] => None,
            [one] => Some([*one; N]),
            many => {
                let mut out = [0.0; N];
                for (slot, number) in out.iter_mut().zip(many) {
                    *slot = *number;
                }
                Some(out)
            }
        }
    }
}

impl FromDoc for Value {
    fn from_doc(doc: &Value) -> Option<Self> {
        Some(doc.clone())
    }
}
