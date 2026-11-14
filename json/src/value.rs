//! The value tree.

/// A JSON value.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    /// `null`, and what every failed lookup yields.
    #[default]
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// Any number. Integers up to 2^53 are exact.
    Number(f64),
    /// A string.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object, in the order its keys were written.
    Object(Map),
}

static NULL: Value = Value::Null;

impl Value {
    /// The member `key` of an object; `Null` when this is not an object or has no such key.
    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Object(map) => map.get(key).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// The element `index` of an array; `Null` when out of range or not an array.
    pub fn at(&self, index: usize) -> &Value {
        match self {
            Value::Array(items) => items.get(index).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// The number, if this is one.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The number as a 32-bit float, if this is one.
    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|n| n as f32)
    }

    /// The number as an integer, if this is a number without a fraction.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n) if n.fract() == 0.0 && n.abs() < 9.3e18 => Some(*n as i64),
            _ => None,
        }
    }

    /// The boolean, if this is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The members, if this is an object.
    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The elements of an array, or nothing for anything else.
    pub fn items(&self) -> std::slice::Iter<'_, Value> {
        self.as_array().unwrap_or(&[]).iter()
    }

    /// The members of an object, or nothing for anything else.
    pub fn members(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.as_object().into_iter().flat_map(Map::iter)
    }
}

/// An object's members, kept in insertion order.
///
/// Objects in wallpapers are small, so lookups scan; nothing is hashed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// The value of `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The value of `key`, to change it.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Sets `key`, replacing the value it had and keeping its position.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Removes `key`, returning what it held.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(index).1)
    }

    /// The members in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// How many members there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no members.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<K: Into<String>> FromIterator<(K, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (K, Value)>>(iter: I) -> Self {
        let mut map = Map::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_owned())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::String(s.clone())
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Self {
        Value::Array(items)
    }
}

impl From<Map> for Value {
    fn from(map: Map) -> Self {
        Value::Object(map)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Value::Null, Into::into)
    }
}

macro_rules! number_from {
    ($($ty:ty),*) => {$(
        impl From<$ty> for Value {
            fn from(n: $ty) -> Self {
                Value::Number(n as f64)
            }
        }
    )*};
}
number_from!(f64, i64, i32, u64, u32, u16, u8, usize);

impl From<f32> for Value {
    /// The number as it is written in single precision: `0.27`, not the nearest
    /// double to its bits (`0.27000001072883606`).
    fn from(number: f32) -> Self {
        Value::Number(number.to_string().parse().unwrap_or(f64::from(number)))
    }
}
