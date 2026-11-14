//! Values on their way between Rust and JavaScript.
//!
//! Vectors cross as arrays of their components and become `Vec2`/`Vec3`/`Vec4` objects
//! in the prelude: an array costs one allocation, where building the object from here
//! would cost a constructor call through the engine's C interface.

use rquickjs::{Array, Ctx, Object, Value as Js};
use wepaper_json::Value;

use super::host::{Datum, LayerId};

/// A string as a JavaScript value.
pub(super) fn text<'js>(ctx: &Ctx<'js>, text: &str) -> rquickjs::Result<Js<'js>> {
    Ok(rquickjs::String::from_str(ctx.clone(), text)?.into_value())
}

/// Numbers as a JavaScript array.
pub(super) fn numbers<'js>(ctx: &Ctx<'js>, numbers: &[f64]) -> rquickjs::Result<Js<'js>> {
    let array = Array::new(ctx.clone())?;
    for (i, number) in numbers.iter().enumerate() {
        array.set(i, *number)?;
    }
    Ok(array.into_value())
}

/// A host value as the prelude reads it.
pub(super) fn from_datum<'js>(ctx: &Ctx<'js>, datum: &Datum) -> rquickjs::Result<Js<'js>> {
    match datum {
        Datum::Null => Ok(Js::new_null(ctx.clone())),
        Datum::Bool(flag) => Ok(Js::new_bool(ctx.clone(), *flag)),
        Datum::Number(number) => Ok(Js::new_float(ctx.clone(), *number)),
        Datum::Text(string) => text(ctx, string),
        Datum::Vec2(v) => numbers(ctx, v),
        Datum::Vec3(v) => numbers(ctx, v),
        Datum::Vec4(v) => numbers(ctx, v),
        Datum::Layer(layer) => Ok(Js::new_float(ctx.clone(), f64::from(layer.0))),
    }
}

/// What the prelude sends for a value: a primitive, or an array of two to four
/// components for a vector. `None` for `undefined` and for what is not a value.
pub(super) fn to_datum(value: &Js<'_>) -> Option<Datum> {
    if let Some(flag) = value.as_bool() {
        return Some(Datum::Bool(flag));
    }
    if let Some(number) = value.as_number() {
        return Some(Datum::Number(number));
    }
    if let Some(string) = value.as_string() {
        return string.to_string().ok().map(Datum::Text);
    }
    if value.is_null() {
        return Some(Datum::Null);
    }
    let array = value.as_array()?;
    let mut parts = [0.0; 4];
    let count = array.len();
    for (i, part) in parts.iter_mut().enumerate().take(count) {
        *part = array.get::<f64>(i).ok()?;
    }
    match count {
        2 => Some(Datum::Vec2([parts[0], parts[1]])),
        3 => Some(Datum::Vec3([parts[0], parts[1], parts[2]])),
        4 => Some(Datum::Vec4(parts)),
        _ => None,
    }
}

/// Whether every number in a value is one a scene can use. Scripts divide by zero and
/// normalise empty vectors; what comes of it must not reach the picture.
pub(super) fn is_finite(datum: &Datum) -> bool {
    datum.floats().is_none_or(|floats| floats.iter().all(|f| f.is_finite()))
}

/// The layer a number from the prelude names; `None` for the detached layers scripts
/// get when they create one.
pub(super) fn layer(id: i32) -> Option<LayerId> {
    u32::try_from(id).ok().map(LayerId)
}

/// A JSON document as JavaScript objects.
pub(super) fn from_json<'js>(ctx: &Ctx<'js>, value: &Value) -> rquickjs::Result<Js<'js>> {
    match value {
        Value::Null => Ok(Js::new_null(ctx.clone())),
        Value::Bool(flag) => Ok(Js::new_bool(ctx.clone(), *flag)),
        Value::Number(number) => Ok(Js::new_float(ctx.clone(), *number)),
        Value::String(string) => text(ctx, string),
        Value::Array(items) => {
            let array = Array::new(ctx.clone())?;
            for (i, item) in items.iter().enumerate() {
                array.set(i, from_json(ctx, item)?)?;
            }
            Ok(array.into_value())
        }
        Value::Object(_) => {
            let object = Object::new(ctx.clone())?;
            for (key, member) in value.members() {
                object.set(key, from_json(ctx, member)?)?;
            }
            Ok(object.into_value())
        }
    }
}
