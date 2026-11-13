//! Lenient JSON for wepaper: values, a forgiving parser and a writer.
//!
//! Wallpaper Engine's documents are almost JSON. They carry byte order marks,
//! trailing commas and the occasional `-nan(ind)` where a number should be, and
//! a strict parser throws the whole wallpaper away over one of them. This crate
//! reads all of that, keeps the order of keys, and never fails on a lookup: a
//! missing key is [`Value::Null`].
//!
//! ```
//! let scene = wepaper_json::parse(r#"{ "general": { "zoom": 1.5, }, // loose
//!                                      "objects": [] }"#).unwrap();
//! assert_eq!(scene.get("general").get("zoom").as_f64(), Some(1.5));
//! assert!(scene.get("camera").get("eye").is_null());
//! ```

mod parse;
mod value;
mod write;

pub use parse::{Error, parse};
pub use value::{Map, Value};

/// Builds a [`Value::Object`] from `key => value` pairs, keeping their order.
///
/// ```
/// let v = wepaper_json::obj! { "ok" => true, "fps" => 60 };
/// assert_eq!(v.to_json(), r#"{"ok":true,"fps":60}"#);
/// ```
#[macro_export]
macro_rules! obj {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut map = $crate::Map::new();
        $(map.insert($key, $crate::Value::from($value));)*
        $crate::Value::Object(map)
    }};
}
