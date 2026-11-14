//! Repairs driven by what the driver says.
//!
//! Some mistakes cannot be seen without compiling: an `#endif` too many, a `%`
//! between floats. The driver names the line; the repair for that kind of mistake is
//! applied there and the shader is compiled again.

/// The first line a driver's messages point at, counted from the `#line 1` the
/// assembled source carries, with everything the driver says about that line.
/// Understands NVIDIA (`0(12) : error`) and Mesa (`0:12(4): error`).
fn error_line(log: &str) -> Option<(usize, Vec<&str>)> {
    let mut found: Option<(usize, Vec<&str>)> = None;
    for message in log.lines().filter(|line| line.contains("error")) {
        let digits = |text: &str| {
            text.chars().take_while(char::is_ascii_digit).collect::<String>().parse::<usize>().ok()
        };
        let line = if let Some(rest) = message.strip_prefix("0(") {
            digits(rest)
        } else if let Some(rest) = message.strip_prefix("0:") {
            digits(rest)
        } else {
            None
        };
        match (&mut found, line) {
            (None, Some(line)) => found = Some((line, vec![message])),
            (Some((first, messages)), Some(line)) if line == *first => messages.push(message),
            _ => {}
        }
    }
    found
}

/// Tries to repair the mistake the driver's `log` reports in `source`. Returns
/// whether anything was changed: when not, compiling again would fail the same way.
pub fn repair(source: &mut String, log: &str) -> bool {
    let Some((number, messages)) = error_line(log) else { return false };
    let says = |what: &[&str]| messages.iter().any(|message| what.iter().any(|what| message.contains(what)));
    let marker = [super::source::BODY, super::source::BODY_LENIENT]
        .into_iter()
        .find_map(|m| source.find(m).map(|at| at + m.len()));
    let Some(body_start) = marker else { return false };
    let body = &source[body_start..];
    let Some(line_start) = line_offset(body, number) else { return false };
    let line_end = body[line_start..].find('\n').map_or(body.len(), |n| line_start + n);
    let line = &body[line_start..line_end];

    // A line may hold several mistakes, and a driver may name them all at once: the one
    // whose repair takes others with it comes first.
    let replacement = if says(&["unmatched #endif", "#endif without #if"]) {
        line.trim_start().starts_with("#endif").then(|| "// #endif without an #if".to_owned())
    } else if says(&["cannot index a non-array", "cannot dereference non-array"]) {
        scalar_index(line, body)
    } else if says(&["\"%\"", "operator '%'", "operands to %", "of operator %"]) {
        remainder_calls(line)
    } else if says(&["array index must be integer"]) {
        integer_index(line)
    } else if says(&["Syntax error in #if", "#if with no expression"]) {
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

/// The index groups on a line: for each `[...]`, where it starts, how long it is and
/// where the name it follows starts (`None` after anything but a name).
fn index_groups(line: &str) -> Vec<(usize, usize, Option<usize>)> {
    let mut groups = Vec::new();
    let bytes = line.as_bytes();
    let mut at = 0;
    while let Some(found) = line[at..].find('[') {
        let open = at + found;
        let mut depth = 0;
        let close = line[open..].char_indices().find_map(|(index, c)| {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(open + index)
        });
        let Some(close) = close else { break };
        let name_start = (0..open)
            .rev()
            .take_while(|&index| bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            .last();
        groups.push((open, close + 1 - open, name_start));
        // Groups inside this one are indices of their own.
        at = open + 1;
    }
    groups
}

/// Whether `text` declares `name` as an array of floats.
fn float_array(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].trim_end();
        let after = text[at + name.len()..].trim_start();
        before.ends_with("float")
            && after.starts_with('[')
            && !text[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Drops the second index of `floats[a][b]`: one float of an array has no parts to
/// index, and where the original lets that through it reads the float itself.
fn scalar_index(line: &str, text: &str) -> Option<String> {
    let groups = index_groups(line);
    let second = groups.iter().find_map(|&(open, length, name)| {
        let name = &line[name?..open];
        let next = groups.iter().find(|&&(after, _, _)| after == open + length)?;
        float_array(text, name).then_some((next.0, next.1))
    })?;
    let mut out = line.to_owned();
    out.replace_range(second.0..second.0 + second.1, "");
    Some(out)
}

/// Makes every index on a line an integer: `values[i / 4.0]` is `values[int(i / 4.0)]`.
fn integer_index(line: &str) -> Option<String> {
    let mut out = line.to_owned();
    let mut changed = false;
    // From the last group to the first, so that the places of the earlier ones hold.
    for (open, length, _) in index_groups(line).into_iter().rev() {
        let inside = out[open + 1..open + length - 1].trim().to_owned();
        if inside.is_empty() || inside.chars().all(|c| c.is_ascii_digit()) || inside.starts_with("int(") {
            continue;
        }
        out.replace_range(open + 1..open + length - 1, &format!("int({inside})"));
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
        let mut text = source(
            "float a;\nuint b = frequency % RESOLUTION;\nfloat c = g[(i + 1) % 4] + (x % (y * 2.0));\ni %= 3;\n",
        );
        assert!(repair(&mut text, "0(2) : error C1021: operands to \"%\" must be integral\n"));
        assert!(text.contains("uint b = wepaper_rem(frequency, RESOLUTION);"));
        assert!(
            repair(&mut text, "0:3(12): error: operator '%' is reserved in GLSL 1.20\n"),
            "Mesa's message format is read too"
        );
        assert!(
            text.contains("float c = g[wepaper_rem((i + 1), 4)] + (wepaper_rem(x, (y * 2.0)));"),
            "{text}"
        );
        assert!(
            !repair(&mut text, "0(4) : error C1021: operands to \"%\" must be integral\n"),
            "%= is not rewritten"
        );
    }

    #[test]
    fn a_second_index_on_one_float_of_an_array_is_dropped() {
        // The original takes `floats[a][b]`: one float there is a vector of one, and
        // its only component is the float.
        let mut text = source(
            "uniform float g_Audio[64];\nfloat f(float id) {\n    return clamp(0., 1., g_Audio[id / 4][id % 4]) + v[1][2];\n}\n",
        );
        assert!(repair(&mut text, "0(3) : error C1011: cannot index a non-array value\n"));
        assert!(text.contains("return clamp(0., 1., g_Audio[id / 4]) + v[1][2];"), "{text}");
        assert!(!repair(&mut text, "0(3) : error C1011: cannot index a non-array value\n"));
    }

    #[test]
    fn a_line_with_several_mistakes_is_repaired_from_the_outermost_in() {
        // Mesa lists every mistake of the line; the index that goes takes its `%` along.
        let mut text = source(
            "uniform float g_Audio[64];\nfloat f(float id) {\n    return g_Audio[id / 4][id % 4];\n}\n",
        );
        let log = "0:3(20): error: array index must be integer type\n\
                   0:3(30): error: LHS of operator % must be an integer\n\
                   0:3(30): error: cannot dereference non-array / non-matrix / non-vector\n";
        assert!(repair(&mut text, log));
        assert!(text.contains("return g_Audio[id / 4];"), "{text}");
        assert!(repair(&mut text, "0:3(20): error: array index must be integer type\n"));
        assert!(text.contains("return g_Audio[int(id / 4)];"), "{text}");
        assert!(!repair(&mut text, "0:3(20): error: array index must be integer type\n"));
    }

    #[test]
    fn a_remainder_of_floats_is_repaired_whatever_the_driver_calls_it() {
        let mut text = source("float a = b % 4.0;\n");
        assert!(repair(&mut text, "0:1(11): error: LHS of operator % must be an integer\n"));
        assert!(text.contains("float a = wepaper_rem(b, 4.0);"));
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
