//! Shader text cut into tokens, each with its place in the text.

/// What a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Class {
    /// A name: a variable, a function, a kind, a keyword.
    Name,
    /// A number as written.
    Number,
    /// An operator or a bracket.
    Sign,
    /// A preprocessor line, whole.
    Directive,
}

/// One token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Token<'a> {
    pub class: Class,
    pub text: &'a str,
    /// Where it starts and ends in the text it was cut from.
    pub start: usize,
    pub end: usize,
}

/// The signs longer than one character, the longest first.
const SIGNS: [&str; 21] = [
    "<<=", ">>=", "++", "--", "<=", ">=", "==", "!=", "&&", "||", "^^", "+=", "-=", "*=", "/=", "%=", "&=",
    "|=", "^=", "<<", ">>",
];

/// Cuts `text` into tokens. Comments are left out.
pub(super) fn tokens(text: &str) -> Vec<Token<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    // Whether only blanks stand between the last line break and `at`.
    let mut line_start = true;
    while at < bytes.len() {
        let byte = bytes[at];
        let start = at;
        let class = match byte {
            b'\n' => {
                at += 1;
                line_start = true;
                continue;
            }
            b' ' | b'\t' | b'\r' => {
                at += 1;
                continue;
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                at = text[at..].find('\n').map_or(bytes.len(), |n| at + n);
                continue;
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = text[at + 2..].find("*/").map_or(bytes.len(), |n| at + 2 + n + 2);
                continue;
            }
            b'#' if line_start => {
                at = directive_end(text, at);
                Class::Directive
            }
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                    at += 1;
                }
                Class::Name
            }
            b'0'..=b'9' => {
                at = number_end(bytes, at);
                Class::Number
            }
            b'.' if bytes.get(at + 1).is_some_and(u8::is_ascii_digit) => {
                at = number_end(bytes, at);
                Class::Number
            }
            _ => {
                // Anything else is a sign of its own, which no expression takes.
                let long = SIGNS.iter().find(|sign| text[at..].starts_with(*sign));
                at += long
                    .map_or_else(|| text[at..].chars().next().map_or(1, char::len_utf8), |sign| sign.len());
                Class::Sign
            }
        };
        line_start = false;
        out.push(Token { class, text: &text[start..at], start, end: at });
    }
    out
}

/// Where the preprocessor line that starts at `at` ends, with its continuations.
fn directive_end(text: &str, at: usize) -> usize {
    let mut end = at;
    loop {
        let line_end = text[end..].find('\n').map_or(text.len(), |n| end + n);
        if !text[end..line_end].trim_end().ends_with('\\') || line_end == text.len() {
            return line_end;
        }
        end = line_end + 1;
    }
}

/// Where the number that starts at `at` ends: digits, a point, an exponent with its
/// sign, a suffix.
fn number_end(bytes: &[u8], mut at: usize) -> usize {
    let hex = bytes[at] == b'0' && matches!(bytes.get(at + 1), Some(b'x' | b'X'));
    while at < bytes.len() {
        let byte = bytes[at];
        let signed_exponent =
            !hex && matches!(byte, b'+' | b'-') && matches!(bytes.get(at.wrapping_sub(1)), Some(b'e' | b'E'));
        if byte.is_ascii_alphanumeric() || byte == b'.' || signed_exponent {
            at += 1;
        } else {
            break;
        }
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_cut_into_names_numbers_and_signs_with_their_places() {
        let text = "a.xy += 1.5e-3*b[2u] // done\n#define X \\\n  2\n/* c */ c";
        let cut: Vec<(Class, &str)> = tokens(text).iter().map(|t| (t.class, t.text)).collect();
        assert_eq!(
            cut,
            [
                (Class::Name, "a"),
                (Class::Sign, "."),
                (Class::Name, "xy"),
                (Class::Sign, "+="),
                (Class::Number, "1.5e-3"),
                (Class::Sign, "*"),
                (Class::Name, "b"),
                (Class::Sign, "["),
                (Class::Number, "2u"),
                (Class::Sign, "]"),
                (Class::Directive, "#define X \\\n  2"),
                (Class::Name, "c"),
            ]
        );
        let last = tokens(text).last().copied().expect("a token");
        assert_eq!(&text[last.start..last.end], "c");
    }
}
