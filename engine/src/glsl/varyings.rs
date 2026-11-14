//! Varyings the two stages of a shader declare with different types.
//!
//! HLSL ties the stages together by position, so a workshop shader may write a `vec4`
//! in the vertex stage and read a `vec2` of the same name in the fragment stage. GLSL
//! ties them by name and type and refuses to link such a pair. The fragment stage is
//! made to declare what the vertex stage writes, and a macro gives its code the type
//! it expected: the first components of a wider varying, a narrower one padded with
//! zeros (what HLSL reads there is undefined anyway).

use std::collections::HashMap;

use super::Source;

/// Every varying a stage declares, with its type, when every declaration of a name agrees.
fn declared(text: &str) -> HashMap<&str, Option<&str>> {
    let mut found: HashMap<&str, Option<&str>> = HashMap::new();
    for (kind, name) in text.lines().filter_map(declaration) {
        found
            .entry(name)
            .and_modify(|known| {
                // A name declared again with another type (under another `#if`) is left alone.
                if *known != Some(kind) {
                    *known = None;
                }
            })
            .or_insert(Some(kind));
    }
    found
}

/// The type and name of a line that declares one plain varying.
fn declaration(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim().strip_prefix("varying ")?;
    let (kind, rest) = rest.trim_start().split_once(char::is_whitespace)?;
    let name = rest.trim().strip_suffix(';')?.trim_end();
    let identifier = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (width(kind).is_some() && identifier).then_some((kind, name))
}

/// How many components a type has, for the types a varying is given.
fn width(kind: &str) -> Option<usize> {
    match kind {
        "float" => Some(1),
        "vec2" => Some(2),
        "vec3" => Some(3),
        "vec4" => Some(4),
        _ => None,
    }
}

/// How a value of `from` is read as `to`.
fn conversion(name: &str, from: usize, to: usize) -> String {
    if to < from {
        format!("{name}.{}", &"xyzw"[..to])
    } else {
        let zeros = vec!["0.0"; to - from].join(", ");
        format!("vec{to}({name}, {zeros})")
    }
}

impl Source {
    /// This fragment stage, declaring its varyings as `vertex` writes them.
    pub fn matching(&self, vertex: &Source) -> Source {
        let written = declared(&vertex.text);
        let read = declared(&self.text);
        let mut text = String::with_capacity(self.text.len() + 256);
        for line in self.text.split_inclusive('\n') {
            let changed = declaration(line).and_then(|(kind, name)| {
                let theirs = (*written.get(name)?)?;
                let ours = (*read.get(name)?)?;
                (theirs != ours && ours == kind).then_some((theirs, ours, name))
            });
            match changed {
                Some((theirs, ours, name)) => {
                    let (from, to) = (width(theirs).unwrap_or(4), width(ours).unwrap_or(4));
                    text.push_str(&format!(
                        "varying {theirs} {name};\n#define {name} {}\n",
                        conversion(name, from, to)
                    ));
                }
                None => text.push_str(line),
            }
        }
        Source { stage: self.stage, text, declared: self.declared.clone() }
    }
}
