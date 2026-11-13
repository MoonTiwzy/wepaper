//! Repairs for shaders that compile as HLSL but are not valid GLSL.
//!
//! Workshop authors only ever see Wallpaper Engine's own compiler accept their
//! shader, and it is more forgiving than a GLSL driver. Each repair here is for a
//! pattern found in real wallpapers.

/// Words workshop shaders use as names that GLSL reserves.
const RESERVED: &[&str] = &["sample", "active", "input", "output", "filter", "common", "partition", "smooth", "flat", "packed"];

/// Renames identifiers that GLSL reserves, in whatever role the shader uses them.
///
/// A member access (`.sample`) is left alone, and so are the words `smooth` and
/// `flat` when they qualify a declaration.
pub(super) fn rename_reserved(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            let word = &text[start..at];
            let member = start > 0 && bytes[start - 1] == b'.';
            let qualifier = matches!(word, "smooth" | "flat") && {
                let next = text[at..].trim_start();
                ["in ", "out ", "varying "].iter().any(|k| next.starts_with(k))
            };
            out.push_str(word);
            if RESERVED.contains(&word) && !member && !qualifier {
                out.push_str("_w");
            }
        } else if byte == b'/' && bytes.get(at + 1) == Some(&b'/') {
            // Comments pass through untouched.
            let end = text[at..].find('\n').map_or(bytes.len(), |n| at + n);
            out.push_str(&text[at..end]);
            at = end;
        } else {
            // Not ASCII-alphabetic: copy the whole character, whatever its width.
            let ch = text[at..].chars().next().expect("in bounds");
            out.push(ch);
            at += ch.len_utf8();
        }
    }
    out
}

/// A shader that brings its own `mod(float, float)` (HLSL has `fmod`, authors write a
/// `mod` for it) clashes with the built-in: theirs is renamed and used instead.
pub(super) fn rename_own_mod(text: &str) -> String {
    let defines_mod = text.lines().any(|line| {
        let line = line.trim_start();
        ["float mod(", "float mod (", "vec2 mod(", "vec3 mod(", "vec4 mod("].iter().any(|start| line.starts_with(start))
    });
    if !defines_mod {
        return text.to_owned();
    }
    replace_word(text, "mod", "wepaper_mod")
}

/// Replaces `word` where it stands alone as an identifier.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let bytes = text.as_bytes();
    let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out = String::with_capacity(text.len());
    let mut rest = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find(word) {
        let start = search + found;
        let end = start + word.len();
        let alone = (start == 0 || !ident(bytes[start - 1]) && bytes[start - 1] != b'.') && (end >= bytes.len() || !ident(bytes[end]));
        if alone {
            out.push_str(&text[rest..start]);
            out.push_str(with);
            rest = end;
        }
        search = end;
    }
    out.push_str(&text[rest..]);
    out
}
