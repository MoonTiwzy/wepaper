//! Repairs driven by what the driver says.
//!
//! Some mistakes cannot be seen without compiling: an `#endif` too many, a `%`
//! between floats. The driver names the line; the repair for that kind of mistake is
//! applied there and the shader is compiled again.

/// The line a driver's message points at, counted from the `#line 1` the assembled
/// source carries. Understands NVIDIA (`0(12) : error`) and Mesa (`0:12(4): error`).
fn error_line(log: &str) -> Option<(usize, &str)> {
    for message in log.lines().filter(|line| line.contains("error")) {
        let digits = |text: &str| text.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<usize>().ok();
        let line = if let Some(rest) = message.strip_prefix("0(") {
            digits(rest)
        } else if let Some(rest) = message.strip_prefix("0:") {
            digits(rest)
        } else {
            None
        };
        if let Some(line) = line {
            return Some((line, message));
        }
    }
    None
}

/// Tries to repair the mistake the driver's `log` reports in `source`. Returns
/// whether anything was changed: when not, compiling again would fail the same way.
pub fn repair(source: &mut String, log: &str) -> bool {
    let Some((number, message)) = error_line(log) else { return false };
    let marker = [super::source::BODY, super::source::BODY_LENIENT].into_iter().find_map(|m| source.find(m).map(|at| at + m.len()));
    let Some(body_start) = marker else { return false };
    let body = &source[body_start..];
    let Some(line_start) = line_offset(body, number) else { return false };
    let line_end = body[line_start..].find('\n').map_or(body.len(), |n| line_start + n);
    let line = &body[line_start..line_end];

    let replacement = if message.contains("unmatched #endif") || message.contains("#endif without #if") {
        line.trim_start().starts_with("#endif").then(|| "// #endif without an #if".to_owned())
    } else if message.contains("\"%\"") || message.contains("operator '%'") || message.contains("operands to %") {
        remainder_calls(line)
    } else if message.contains("Syntax error in #if") || message.contains("#if with no expression") {
        // A condition the preprocessor cannot evaluate (it names a variable) is false.
        line.trim_start().starts_with("#if").then(|| "#if 0".to_owned())
    } else {
        None
    };
    let Some(replacement) = replacement.filter(|r| r != line) else { return false };
    let (start, end) = (body_start + line_start, body_start + line_end);
    source.replace_range(start..end, &replacement);
    true
}

/// Where line `number` (from 1) starts in `text`.
fn line_offset(text: &str, number: usize) -> Option<usize> {
    if number == 0 {
        return None;
    }
    let mut offset = 0;
    for _ in 1..number {
        offset += text[offset..].find('\n')? + 1;
    }
    Some(offset)
}

/// Rewrites every `a % b` on a line as `wepaper_rem(a, b)`, which takes floats too.
fn remainder_calls(line: &str) -> Option<String> {
    let mut out = line.to_owned();
    let mut changed = false;
    let mut search = 0;
    while let Some(found) = out[search..].find('%') {
        let at = search + found;
        // `%=` is left alone.
        if out[at + 1..].starts_with('=') {
            search = at + 1;
            continue;
        }
        let left = operand_before(&out[..at]);
        let right = operand_after(&out[at + 1..]);
        let (Some(left), Some(right)) = (left, right) else {
            search = at + 1;
            continue;
        };
        let call = format!("wepaper_rem({}, {})", out[left..at].trim(), out[at + 1..at + 1 + right].trim());
        out.replace_range(left..at + 1 + right, &call);
        search = left + call.len();
        changed = true;
    }
    changed.then_some(out)
}

fn is_operand_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.')
}

/// Where the operand that ends at the end of `text` starts.
fn operand_before(text: &str) -> Option<usize> {
    let trimmed = text.trim_end();
    let mut depth = 0;
    let mut start = trimmed.len();
    for (index, c) in trimmed.char_indices().rev() {
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            c if depth > 0 || is_operand_char(c) => {}
            _ => break,
        }
        start = index;
        if depth < 0 {
            return None;
        }
    }
    (start < trimmed.len() && depth == 0).then_some(start)
}

/// How long the operand at the start of `text` is.
fn operand_after(text: &str) -> Option<usize> {
    let mut depth = 0;
    let mut end = 0;
    let mut seen = false;
    for (index, c) in text.char_indices() {
        match c {
            ' ' | '\t' if !seen => {}
            '(' | '[' => {
                depth += 1;
                seen = true;
            }
            ')' | ']' if depth > 0 => depth -= 1,
            c if depth > 0 || is_operand_char(c) => seen = true,
            _ => break,
        }
        end = index + c.len_utf8();
    }
    (seen && depth == 0).then_some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(body: &str) -> String {
        format!("#version 330 core\n#define X 1\n#line 1\n{body}")
    }

    #[test]
    fn stray_endif_is_dropped() {
        let mut text = source("void main() {\n#endif\n}\n");
        assert!(repair(&mut text, "0(2) : error C7102: unmatched #endif\n"));
        assert!(text.ends_with("void main() {\n// #endif without an #if\n}\n"));
        assert!(!repair(&mut text, "0(2) : error C7102: unmatched #endif\n"), "nothing left to repair there");
    }

    #[test]
    fn float_remainder_becomes_a_call() {
        let mut text = source("float a;\nuint b = frequency % RESOLUTION;\nfloat c = g[(i + 1) % 4] + (x % (y * 2.0));\ni %= 3;\n");
        assert!(repair(&mut text, "0(2) : error C1021: operands to \"%\" must be integral\n"));
        assert!(text.contains("uint b = wepaper_rem(frequency, RESOLUTION);"));
        assert!(repair(&mut text, "0:3(12): error: operator '%' is reserved in GLSL 1.20\n"), "Mesa's message format is read too");
        assert!(text.contains("float c = g[wepaper_rem((i + 1), 4)] + (wepaper_rem(x, (y * 2.0)));"), "{text}");
        assert!(!repair(&mut text, "0(4) : error C1021: operands to \"%\" must be integral\n"), "%= is not rewritten");
    }

    #[test]
    fn unknown_errors_and_bad_lines_change_nothing() {
        let mut text = source("void main() {}\n");
        let before = text.clone();
        assert!(!repair(&mut text, "0(1) : error C0000: something else entirely\n"));
        assert!(!repair(&mut text, "0(99) : error C7102: unmatched #endif\n"));
        assert!(!repair(&mut text, "no line number here: error\n"));
        assert_eq!(text, before);
        let mut condition = source("#if g_Texture0Resolution.y <= 640\n#endif\n");
        assert!(repair(&mut condition, "0(1) : error C0105: Syntax error in #if\n"));
        assert!(condition.contains("#line 1\n#if 0\n#endif"));
    }
}
