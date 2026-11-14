//! Reading the loosely typed fields of particle documents, and the vector arithmetic
//! the simulation needs.

use wepaper_json::Value;

/// A vector of three, as everywhere in the engine.
pub(crate) type Vec3 = [f32; 3];

/// The literal behind a field: fields linked to a user property or a script are
/// objects whose `value` holds what applies until something drives them.
pub(crate) fn plain(doc: &Value) -> &Value {
    match doc {
        Value::Object(map) => map.get("value").unwrap_or(doc),
        _ => doc,
    }
}

/// The numbers a field holds: a number, or a string such as `"1 0.5 0"`.
/// Anything that is not a finite number reads as absent.
fn numbers(doc: &Value) -> ([f32; 3], usize) {
    let mut out = [0.0f32; 3];
    let mut count = 0;
    match plain(doc) {
        Value::Number(n) => {
            out[0] = *n as f32;
            count = 1;
        }
        Value::Bool(b) => {
            out[0] = f32::from(u8::from(*b));
            count = 1;
        }
        Value::String(text) => {
            for part in text.split([' ', ',', '\t']).filter(|part| !part.is_empty()).take(3) {
                out[count] = part.parse().unwrap_or(0.0);
                count += 1;
            }
        }
        _ => {}
    }
    if out.iter().any(|n| !n.is_finite()) {
        return ([0.0; 3], 0);
    }
    (out, count)
}

/// The number in `doc[key]`, or `default`.
pub(crate) fn number(doc: &Value, key: &str, default: f32) -> f32 {
    match numbers(doc.get(key)) {
        (_, 0) => default,
        (values, _) => values[0],
    }
}

/// The vector in `doc[key]`, or `default`. Components the document leaves out are
/// zero: a lone `-0.4` is `-0.4 0 0`, as the editor writes it back.
pub(crate) fn vec3(doc: &Value, key: &str, default: Vec3) -> Vec3 {
    match numbers(doc.get(key)) {
        (_, 0) => default,
        (values, _) => values,
    }
}

/// The vector in `doc[key]` where a lone number means the same extent on every
/// axis (emitter distances), or `default`.
pub(crate) fn extent(doc: &Value, key: &str, default: Vec3) -> Vec3 {
    match numbers(doc.get(key)) {
        (_, 0) => default,
        (values, 1) => [values[0]; 3],
        (values, _) => values,
    }
}

/// The whole number in `doc[key]` within `0..=max`, or `default`.
pub(crate) fn index(doc: &Value, key: &str, default: usize, max: usize) -> usize {
    match numbers(doc.get(key)) {
        (_, 0) => default,
        (values, _) => (values[0].max(0.0) as usize).min(max),
    }
}

/// `a + b`.
pub(crate) fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a - b`.
pub(crate) fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `a` scaled by `by`.
pub(crate) fn scale(a: Vec3, by: f32) -> Vec3 {
    [a[0] * by, a[1] * by, a[2] * by]
}

/// `a` and `b` multiplied component by component.
pub(crate) fn mul(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// The dot product.
pub(crate) fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The cross product.
pub(crate) fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The length.
pub(crate) fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` at unit length, or `None` when it is too short to have a direction.
pub(crate) fn direction(a: Vec3) -> Option<Vec3> {
    let len = length(a);
    (len > 1e-6 && len.is_finite()).then(|| scale(a, 1.0 / len))
}

/// The point `t` of the way from `a` to `b`.
pub(crate) fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_read_as_numbers_strings_and_linked_values() {
        let doc = wepaper_json::parse(
            r#"{ "a": 2.5, "b": "1 0.5", "c": {"user": "speed", "value": 4}, "d": "0 0 6.5",
                 "e": {"script": "x", "value": "1 2 3"}, "f": "x y", "g": [1] }"#,
        )
        .unwrap();
        assert_eq!(number(&doc, "a", 9.0), 2.5);
        assert_eq!(number(&doc, "c", 9.0), 4.0);
        assert_eq!(number(&doc, "missing", 9.0), 9.0);
        assert_eq!(number(&doc, "g", 9.0), 9.0);
        assert_eq!(vec3(&doc, "a", [9.0; 3]), [2.5, 0.0, 0.0]);
        assert_eq!(vec3(&doc, "b", [9.0; 3]), [1.0, 0.5, 0.0]);
        assert_eq!(vec3(&doc, "e", [9.0; 3]), [1.0, 2.0, 3.0]);
        assert_eq!(vec3(&doc, "f", [9.0; 3]), [0.0, 0.0, 0.0]);
        assert_eq!(extent(&doc, "a", [9.0; 3]), [2.5; 3]);
        assert_eq!(extent(&doc, "d", [9.0; 3]), [0.0, 0.0, 6.5]);
        assert_eq!(index(&doc, "a", 0, 7), 2);
        assert_eq!(index(&doc, "d", 5, 7), 0);
        assert_eq!(index(&doc, "missing", 5, 7), 5);
    }

    #[test]
    fn numbers_that_are_not_finite_read_as_absent() {
        let doc = wepaper_json::obj! { "a" => f64::NAN, "b" => "inf 1 2" };
        assert_eq!(number(&doc, "a", 3.0), 3.0);
        assert_eq!(vec3(&doc, "b", [7.0; 3]), [7.0; 3]);
    }

    #[test]
    fn direction_refuses_what_has_none() {
        assert_eq!(direction([0.0, 3.0, 4.0]), Some([0.0, 0.6, 0.8]));
        assert_eq!(direction([0.0; 3]), None);
        assert_eq!(direction([f32::NAN, 0.0, 0.0]), None);
        assert_eq!(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]);
    }
}
