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
    let structural = if says(&["unmatched #endif", "#endif without #if"]) {
        Some(line.trim_start().starts_with("#endif").then(|| "// #endif without an #if".to_owned()))
    } else if says(&["cannot index a non-array", "cannot dereference non-array"]) {
        Some(scalar_index(line, body))
    } else if says(&["\"%\"", "operator '%'", "operands to %", "of operator %"]) {
        Some(remainder_calls(line))
    } else {
        None
    };
    // Otherwise the kinds are worked out, and the conversions the original makes are
    // written in; what that does not cover is read from the message.
    if structural.is_none() && !line.trim_start().starts_with('#') {
        let (start, end) = statement(body, line_start, line_end);
        if let Some(converted) = super::typed::convert(source, body_start + start, body_start + end) {
            source.replace_range(body_start + start..body_start + end, &converted);
            return true;
        }
    }
    let replacement = if let Some(structural) = structural {
        structural
    } else if says(&["array index must be integer"]) {
        integer_index(line)
    } else if let Some((to, initializer)) = messages.iter().find_map(|message| assigned_kind(message)) {
        assigned_as(line, to, initializer)
    } else if says(&["no matching function for call to"]) {
        called_as(line, &messages)
    } else if says(&["operands to arithmetic operators must be numeric"]) {
        counted_comparisons(line)
    } else if says(&["Syntax error in #if", "#if with no expression", "preprocessor error"]) {
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

/// The lines of the statement that line `start..end` of `body` is part of: the lines
/// it continues and the lines that complete it, so that a statement written over
/// several lines is read whole.
fn statement(body: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    let code = |line: &str| line.split("//").next().unwrap_or("").trim().to_owned();
    let open = |line: &str| {
        let code = code(line);
        !code.is_empty() && !code.starts_with('#') && !code.ends_with([';', '{', '}'])
    };
    for _ in 0..3 {
        let Some(previous_end) = start.checked_sub(1) else { break };
        let previous_start = body[..previous_end].rfind('\n').map_or(0, |n| n + 1);
        if !open(&body[previous_start..previous_end]) {
            break;
        }
        start = previous_start;
    }
    for _ in 0..8 {
        if !open(&body[body[..end].rfind('\n').map_or(0, |n| n + 1)..end]) || end >= body.len() {
            break;
        }
        let next_end = body[end + 1..].find('\n').map_or(body.len(), |n| end + 1 + n);
        if code(&body[end + 1..next_end]).starts_with('#') {
            break;
        }
        end = next_end;
    }
    (start, end)
}

/// Tries to repair what the driver's `log` says of the two stages together. Returns
/// whether anything was changed.
///
/// Two mistakes are known, both of a name the stages declare with different kinds,
/// which the original takes since it builds each stage on its own:
/// - a value the vertex stage writes as a vector of another length than the fragment
///   stage reads. The fragment stage takes the value as it is written and, wherever it
///   names it, reads its first components, or the whole value with what is missing as
///   GL fills an attribute nobody sets: nought, and one for the fourth.
/// - a uniform. The fragment stage's kind is the one the material's value is set as;
///   the vertex stage takes the uniform as that and converts it where it names it.
pub fn repair_link(vertex: &mut String, fragment: &mut String, log: &str) -> bool {
    let quoted = |text: &str| -> Option<String> {
        let (_, rest) = text.split_once('`')?;
        rest.split_once('\'').map(|(inside, _)| inside.to_owned())
    };
    for message in log.lines() {
        if let Some((written, read)) = message.split_once("but fragment shader input declared as type") {
            let Some((name, written)) = written.split_once("declared as type") else { continue };
            let (Some(name), Some(written), Some(read)) = (quoted(name), quoted(written), quoted(read))
            else {
                continue;
            };
            return passed_on(fragment, &name, &written, &read);
        }
        // NVIDIA: `unknown semantics "ATTR0.xy" specified for "v_TexCoord"`, no kinds.
        if message.contains("unknown semantics")
            && let Some((_, rest)) = message.split_once("specified for \"")
            && let Some((name, _)) = rest.split_once('"')
        {
            let written = declared_kind(vertex, &["varying", "out"], name);
            let read = declared_kind(fragment, &["varying", "in"], name);
            let (Some(written), Some(read)) = (written, read) else { continue };
            return passed_on(fragment, name, &written, &read);
        }
        if let Some(rest) = message.split_once("uniform `").map(|(_, rest)| rest)
            && rest.contains("declared as type")
        {
            let Some(name) = rest.split_once('\'').map(|(name, _)| name.to_owned()) else { continue };
            let set = declared_kind(fragment, &["uniform"], &name);
            let (Some(set), Some(declared)) = (set, declared_kind(vertex, &["uniform"], &name)) else {
                continue;
            };
            let Some(reads) = read_as(&name, &set, &declared, true) else { continue };
            return redeclare(
                vertex,
                &["uniform"],
                &name,
                &declared,
                &format!("uniform {set} {name};"),
                &reads,
            );
        }
    }
    false
}

/// The code of a line, without its comment, cut into words.
fn words(line: &str) -> Vec<&str> {
    let code = line.split("//").next().unwrap_or("");
    code.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect()
}

/// The kind the body of `source` declares `name` with, after one of `qualifiers`.
fn declared_kind(source: &str, qualifiers: &[&str], name: &str) -> Option<String> {
    let body =
        source.split_once(super::source::BODY).or_else(|| source.split_once(super::source::BODY_LENIENT))?.1;
    body.lines().find_map(|line| match words(line)[..] {
        [qualifier, kind, declared] if qualifiers.contains(&qualifier) && declared == name => {
            Some(kind.to_owned())
        }
        _ => None,
    })
}

/// Makes the fragment stage read the value `name`, which the vertex stage passes on as
/// `written`, where it declares it as `read`.
fn passed_on(fragment: &mut String, name: &str, written: &str, read: &str) -> bool {
    let Some(reads) = read_as(name, written, read, false) else { return false };
    redeclare(fragment, &["varying", "in"], name, read, &format!("in {written} {name};"), &reads)
}

/// How a value of the kind `given` called `name` is read as the kind `taken`: its
/// first components, or all of it with what is missing filled in. `spread`: a number
/// is spread over a vector rather than filled in after.
fn read_as(name: &str, given: &str, taken: &str, spread: bool) -> Option<String> {
    let (Some(given_width), Some(taken_width)) = (width(given), width(taken)) else { return None };
    let floats = |kind: &str| kind.starts_with("vec") || kind == "float";
    if given_width == taken_width || !floats(given) || !floats(taken) {
        return None;
    }
    Some(if taken_width < given_width {
        format!("{name}.{}", &"xyzw"[..taken_width])
    } else if spread && given_width == 1 {
        format!("{taken}({name})")
    } else {
        let missing: Vec<&str> =
            (given_width..taken_width).map(|at| if at == 3 { "1.0" } else { "0.0" }).collect();
        format!("{taken}({name}, {})", missing.join(", "))
    })
}

/// Replaces the declaration of `name` as `kind` (after one of `qualifiers`) in the
/// body of `source` by a macro that `reads` it, and puts `declaration` in front of
/// the body. The macro stands on the declaration's line, so that every line after it
/// keeps its number.
fn redeclare(
    source: &mut String,
    qualifiers: &[&str],
    name: &str,
    kind: &str,
    declaration: &str,
    reads: &str,
) -> bool {
    let marker = [super::source::BODY, super::source::BODY_LENIENT]
        .into_iter()
        .find_map(|marker| source.find(marker).map(|at| (at, marker.len())));
    let Some((marker_at, marker_length)) = marker else { return false };
    let body_start = marker_at + marker_length;
    let mut body = String::with_capacity(source.len() - body_start);
    let mut changed = false;
    for line in source[body_start..].split_inclusive('\n') {
        if matches!(words(line)[..], [qualifier, declared_kind, declared] if qualifiers.contains(&qualifier) && declared_kind == kind && declared == name)
        {
            body.push_str(&format!("#define {name} {reads}\n"));
            changed = true;
        } else {
            body.push_str(line);
        }
    }
    if changed {
        source.truncate(body_start);
        source.push_str(&body);
        source.insert_str(marker_at, &format!("{declaration}\n"));
    }
    changed
}

/// How many numbers a value of the kind called `kind` holds, for scalars and vectors.
fn width(kind: &str) -> Option<usize> {
    match kind {
        "float" | "int" | "uint" | "bool" => Some(1),
        _ => {
            let digits = kind
                .strip_prefix("vec")
                .or_else(|| kind.strip_prefix(['i', 'u', 'b']).and_then(|rest| rest.strip_prefix("vec")))?;
            digits.parse().ok().filter(|width| (2..=4).contains(width))
        }
    }
}

/// What converting a value of the kind `given` to the kind `taken` costs, the way
/// the original converts by itself: a vector is cut shorter, a number is spread over
/// a vector or turned into another kind of number. `None`: it does not convert.
fn conversion(given: &str, taken: &str) -> Option<f32> {
    if given == taken {
        return Some(0.0);
    }
    let (from, to) = (width(given)?, width(taken)?);
    // Whole numbers and fractions tell apart by their first letter.
    let other_numbers = if given.as_bytes()[0] == taken.as_bytes()[0] || (from > 1) == (to > 1) && from == to
    {
        0.25
    } else {
        0.0
    };
    match (from, to) {
        (from, to) if from == to => Some(0.25),
        (from, to) if from > to => Some((from - to) as f32 + other_numbers),
        (1, _) => Some(0.5 + other_numbers),
        _ => None,
    }
}

/// The names the dialect has for the functions a driver names.
const ALIASES: [(&str, &[&str]); 6] = [
    ("mix", &["lerp"]),
    ("fract", &["frac"]),
    ("atan", &["atan2"]),
    ("texture", &["texSample2D", "texture2D", "texSample3D", "textureCube"]),
    ("textureLod", &["texSample2DLod", "texture2DLod"]),
    ("dFdx", &["ddx"]),
];

/// The arguments of the call whose opening bracket is at `open`: where each starts
/// and ends, and where the call's closing bracket is.
fn arguments(line: &str, open: usize) -> Option<(Vec<(usize, usize)>, usize)> {
    let mut depth = 0;
    let mut parts = Vec::new();
    let mut start = open + 1;
    for (index, c) in line[open..].char_indices() {
        let at = open + index;
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    if !line[start..at].trim().is_empty() || !parts.is_empty() {
                        parts.push((start, at));
                    }
                    return Some((parts, at));
                }
            }
            ',' if depth == 1 => {
                parts.push((start, at));
                start = at + 1;
            }
            _ => {}
        }
    }
    None
}

/// Converts the arguments of a call the driver found no function for to what the
/// nearest of the functions it lists takes: `mix(four, three, t)` is
/// `mix(vec3(four), three, t)`. The first such call on the line that is not converted yet.
fn called_as(line: &str, messages: &[&str]) -> Option<String> {
    let call = messages.iter().find_map(|message| {
        let (_, rest) = message.split_once("no matching function for call to `")?;
        rest.split_once('\'').map(|(call, _)| call)
    })?;
    let (name, given) = call.split_once('(')?;
    let kinds = |list: &str| -> Vec<String> {
        list.trim_end_matches(')')
            .split(',')
            .map(|kind| kind.trim().to_owned())
            .filter(|k| !k.is_empty())
            .collect()
    };
    let given = kinds(given);
    // "error:    vec3 mix(vec3, vec3, float)": what it could have been called with.
    let taken = messages
        .iter()
        .filter(|message| !message.contains("no matching function"))
        .filter_map(|message| {
            let (_, rest) = message.rsplit_once("error:")?;
            let (head, list) = rest.trim().split_once('(')?;
            (head.split_whitespace().last() == Some(name)).then(|| kinds(list))
        })
        .filter(|taken| taken.len() == given.len())
        .filter_map(|taken| {
            let cost = given
                .iter()
                .zip(&taken)
                .map(|(given, taken)| conversion(given, taken))
                .sum::<Option<f32>>()?;
            Some((cost, taken))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, taken)| taken)?;

    let plain = name.strip_prefix("wepaper_").unwrap_or(name);
    let aliases = ALIASES.iter().find(|(known, _)| *known == plain).map_or(&[][..], |(_, aliases)| aliases);
    let bytes = line.as_bytes();
    for spelled in std::iter::once(plain).chain(aliases.iter().copied()) {
        for (at, _) in line.match_indices(spelled) {
            let whole =
                !at.checked_sub(1).is_some_and(|i| bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_');
            let open = at + spelled.len() + line[at + spelled.len()..].len()
                - line[at + spelled.len()..].trim_start().len();
            if !whole || bytes.get(open) != Some(&b'(') {
                continue;
            }
            let Some((parts, _)) = arguments(line, open) else { continue };
            if parts.len() != given.len() {
                continue;
            }
            let wanted: Vec<(usize, usize, &str)> = parts
                .iter()
                .zip(given.iter().zip(&taken))
                .filter(|(_, (given, taken))| given != taken)
                .map(|(&(start, end), (_, taken))| (start, end, taken.as_str()))
                .collect();
            if wanted.iter().all(|&(start, end, taken)| is_call_of(line[start..end].trim(), taken)) {
                continue;
            }
            let mut out = line.to_owned();
            for &(start, end, taken) in wanted.iter().rev() {
                let argument = line[start..end].trim();
                if !is_call_of(argument, taken) {
                    let lead =
                        &line[start..end][..line[start..end].len() - line[start..end].trim_start().len()];
                    out.replace_range(start..end, &format!("{lead}{taken}({argument})"));
                }
            }
            return Some(out);
        }
    }
    None
}

/// Makes a number of every comparison in brackets that the line counts with:
/// `(depth < limit) * 6.0` is `float(depth < limit) * 6.0`.
fn counted_comparisons(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut out = line.to_owned();
    let mut changed = false;
    let opens: Vec<usize> = line.match_indices('(').map(|(at, _)| at).collect();
    for open in opens.into_iter().rev() {
        // What stands before the bracket: a name makes it a call, unless the name is `return`.
        let before = line[..open].trim_end();
        let word_start = (0..before.len())
            .rev()
            .take_while(|&i| bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            .last()
            .unwrap_or(before.len());
        let word = &before[word_start..];
        if !word.is_empty() && word != "return" {
            continue;
        }
        let Some((_, close)) = arguments(line, open) else { continue };
        let inside = &line[open + 1..close];
        let mut depth = 0;
        let compares = inside.char_indices().any(|(index, c)| {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                _ => {}
            }
            depth == 0
                && (matches!(c, '<' | '>')
                    || ["==", "!=", "&&", "||"].iter().any(|sign| inside[index..].starts_with(sign)))
        });
        if compares {
            out.insert_str(open, "float");
            changed = true;
        }
    }
    changed.then_some(out)
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

/// What a message about an assignment of the wrong kind says the variable is, and
/// whether it is being declared: "initializer of type float cannot be assigned to
/// variable of type uint" gives `("uint", true)`.
fn assigned_kind(message: &str) -> Option<(&str, bool)> {
    let (what, to) = message.split_once(" cannot be assigned to variable of type ")?;
    let to = to.split(|c: char| !c.is_ascii_alphanumeric()).next().filter(|to| !to.is_empty())?;
    Some((to, what.contains("initializer of type")))
}

/// Where the expression that starts at `from` ends: at the `;` or `,` that is not
/// inside brackets, or at the end of the line.
fn expression_end(line: &str, from: usize) -> usize {
    let mut depth = 0;
    for (index, c) in line[from..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            ')' | ';' | ',' if depth == 0 => return from + index,
            _ => {}
        }
    }
    line.len()
}

/// Whether `text` is one call of `name` and nothing else: `vec3(...)`.
fn is_call_of(text: &str, name: &str) -> bool {
    let Some(rest) = text.strip_prefix(name).map(str::trim_start).filter(|rest| rest.starts_with('(')) else {
        return false;
    };
    let mut depth = 0;
    rest.char_indices().find_map(|(index, c)| {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        (depth == 0).then_some(index)
    }) == Some(rest.len() - 1)
}

/// Converts what a line assigns to the kind `to` of what it is assigned to, as the
/// original does by itself: `vec3 a = b;` becomes `vec3 a = vec3(b);`. With
/// `initializer`, in a declaration of that kind; without, in a plain assignment. The
/// first place on the line that is not converted yet is.
fn assigned_as(line: &str, to: &str, initializer: bool) -> Option<String> {
    let bytes = line.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut places = Vec::new();
    for (at, _) in line.match_indices('=') {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        let after = bytes.get(at + 1).copied();
        if after == Some(b'=') || before.is_some_and(|c| b"=!<>+-*/%&|^".contains(&c)) {
            continue;
        }
        if initializer {
            // `<kind> <name> =`: the name, then the kind, right before the sign.
            let name_end = line[..at].trim_end().len();
            let name_start = (0..name_end).rev().take_while(|&i| word(bytes[i])).last().unwrap_or(name_end);
            let kind_end = line[..name_start].trim_end().len();
            let kind_start = (0..kind_end).rev().take_while(|&i| word(bytes[i])).last().unwrap_or(kind_end);
            if name_start == name_end || &line[kind_start..kind_end] != to {
                continue;
            }
        }
        places.push(at + 1);
    }
    for from in places {
        let end = expression_end(line, from);
        let expression = line[from..end].trim();
        if expression.is_empty() || is_call_of(expression, to) {
            continue;
        }
        let mut out = line.to_owned();
        out.replace_range(from..end, &format!(" {to}({expression})"));
        return Some(out);
    }
    None
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
    fn a_stage_that_takes_less_of_a_value_than_the_other_gives_takes_all_and_reads_less() {
        // The original links a vertex stage that writes four numbers with a fragment
        // stage that reads two of them.
        let mut text = source(
            "varying vec2 v_TexCoord;\nvoid main() {\n\tgl_FragColor = texture(g_Texture0, v_TexCoord);\n}\n",
        );
        let log = "error: vertex shader output `v_TexCoord' declared as type `vec4', but fragment shader input declared as type `vec2'\n";
        assert!(repair_link(&mut String::new(), &mut text, log));
        // The lines of the body keep their numbers: what the driver says next still points at them.
        assert!(
            text.ends_with("in vec4 v_TexCoord;\n#line 1\n#define v_TexCoord v_TexCoord.xy\nvoid main() {\n\tgl_FragColor = texture(g_Texture0, v_TexCoord);\n}\n"),
            "{text}"
        );
        assert!(!repair_link(&mut String::new(), &mut text, log), "nothing left to repair");
        assert!(!repair_link(&mut String::new(), &mut text, "error: something else\n"));
    }

    #[test]
    fn a_stage_that_reads_more_of_a_value_than_the_other_gives_gets_the_rest_as_nought_and_one() {
        // The missing components are what GL gives an attribute nobody sets: (0, 0, 0, 1).
        let mut text = source("varying vec4 v_TexCoord;\nvoid main() {\n\tgl_FragColor = v_TexCoord;\n}\n");
        let log = "error: vertex shader output `v_TexCoord' declared as type `vec2', but fragment shader input declared as type `vec4'\n";
        assert!(repair_link(&mut String::new(), &mut text, log));
        assert!(
            text.ends_with("in vec2 v_TexCoord;\n#line 1\n#define v_TexCoord vec4(v_TexCoord, 0.0, 1.0)\nvoid main() {\n\tgl_FragColor = v_TexCoord;\n}\n"),
            "{text}"
        );
        assert!(!repair_link(&mut String::new(), &mut text, log), "nothing left to repair");
    }

    #[test]
    fn a_value_the_stages_disagree_on_is_found_by_its_declarations_when_the_driver_names_no_kinds() {
        // NVIDIA names only the value; what the stages make of it is in their declarations.
        let mut vertex =
            source("varying vec4 v_TexCoord;\nvoid main() {\n\tv_TexCoord.xy = a_TexCoord;\n}\n");
        let mut fragment = source("varying vec2 v_TexCoord;\nvoid main() {}\n");
        let log = "Vertex info\n-----------\n0(6) : error C5108: unknown semantics \"ATTR0.xy\" specified for \"v_TexCoord\"\n";
        assert!(repair_link(&mut vertex, &mut fragment, log));
        assert!(
            fragment.contains("in vec4 v_TexCoord;\n#line 1\n#define v_TexCoord v_TexCoord.xy\n"),
            "{fragment}"
        );
        assert!(!repair_link(&mut vertex, &mut fragment, log), "nothing left to repair");
    }

    #[test]
    fn a_uniform_of_two_kinds_is_set_as_the_fragment_stage_has_it_and_converted_in_the_other() {
        let mut vertex = source(
            "uniform vec2 u_refResolution; // {\"default\":\"512 512\"}\nvoid main() {\n\tgl_Position = vec4(u_refResolution, 0.0, 1.0);\n}\n",
        );
        let mut fragment = source("uniform float u_refResolution; // {\"default\":512}\nvoid main() {}\n");
        let unchanged = fragment.clone();
        let log = "error: uniform `u_refResolution' declared as type `float' and type `vec2'\n";
        assert!(repair_link(&mut vertex, &mut fragment, log));
        assert_eq!(fragment, unchanged);
        assert!(
            vertex.contains("uniform float u_refResolution;\n#line 1\n#define u_refResolution vec2(u_refResolution)\nvoid main() {\n"),
            "{vertex}"
        );
        assert!(!repair_link(&mut vertex, &mut fragment, log), "nothing left to repair");
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
    fn what_is_assigned_is_made_the_kind_it_is_assigned_to() {
        // The original converts by itself: a vector to a shorter one or to its first
        // component, a number to a vector of it, one kind of number to another.
        let mut text = source(
            "vec3 albedo = texSample2D(g_Texture0, v_TexCoord.xy);\n\
             uint bar = wepaper_rem(frequency, RESOLUTION), other = 2u;\n\
             \tfor (int i = -iterations; i <= iterations; i++){\n\
             v_NoiseCoord = v_TexCoord;\n\
             albedo.rgb *= x; albedo.a = mask;\n",
        );
        let told = |line: usize, what: &str| format!("0:{line}(9): error: {what}\n");
        assert!(repair(
            &mut text,
            &told(1, "initializer of type vec4 cannot be assigned to variable of type vec3")
        ));
        assert!(text.contains("vec3 albedo = vec3(texSample2D(g_Texture0, v_TexCoord.xy));"), "{text}");
        assert!(repair(
            &mut text,
            &told(2, "initializer of type float cannot be assigned to variable of type uint")
        ));
        assert!(text.contains("uint bar = uint(wepaper_rem(frequency, RESOLUTION)), other = 2u;"), "{text}");
        assert!(repair(
            &mut text,
            &told(3, "initializer of type float cannot be assigned to variable of type int")
        ));
        assert!(text.contains("for (int i = int(-iterations); i <= iterations; i++){"), "{text}");
        assert!(repair(
            &mut text,
            &told(4, "value of type vec4 cannot be assigned to variable of type vec2")
        ));
        assert!(text.contains("v_NoiseCoord = vec2(v_TexCoord);"), "{text}");
        // Said again of a line that was repaired: nothing more to do there.
        assert!(!repair(
            &mut text,
            &told(4, "value of type vec4 cannot be assigned to variable of type vec2")
        ));
        // An assignment that is not the line's first, and not a compound one.
        assert!(repair(
            &mut text,
            &told(5, "value of type vec4 cannot be assigned to variable of type float")
        ));
        assert!(text.contains("albedo.rgb *= x; albedo.a = float(mask);"), "{text}");
    }

    #[test]
    fn what_a_function_is_given_is_made_what_it_takes() {
        let mut text = source(
            "albedo.rgb = mix(albedo, newAlbedo, mask) + mix(a, b, c);\n\
             vec2 at = rotateVec2(v_TexCoord, -u_direction + 1.57) * u_scale;\n",
        );
        // The driver lists what the function could have been called with.
        let log = "0:1(14): error: no matching function for call to `mix(vec4, vec3, float)'; candidates are:\n\
                   0:1(14): error:    float mix(float, float, float)\n\
                   0:1(14): error:    vec2 mix(vec2, vec2, float)\n\
                   0:1(14): error:    vec3 mix(vec3, vec3, float)\n\
                   0:1(14): error:    vec4 mix(vec4, vec4, float)\n\
                   0:1(14): error:    vec2 mix(vec2, vec2, vec2)\n";
        assert!(repair(&mut text, log));
        // The longer vector is cut to the shorter, as the original cuts it; nothing is made up.
        assert!(text.contains("albedo.rgb = mix(vec3(albedo), newAlbedo, mask) + mix(a, b, c);"), "{text}");
        // Said of the same line again, it is the next call's turn.
        assert!(repair(&mut text, log));
        assert!(text.contains("mix(vec3(albedo), newAlbedo, mask) + mix(vec3(a), b, c);"), "{text}");
        let log = "0:2(11): error: no matching function for call to `rotateVec2(vec4, float)'; candidates are:\n\
                   0:2(11): error:    vec2 rotateVec2(vec2, float)\n";
        assert!(repair(&mut text, log));
        assert!(
            text.contains("vec2 at = rotateVec2(vec2(v_TexCoord), -u_direction + 1.57) * u_scale;"),
            "{text}"
        );
        // A call nothing of what it could take fits is left alone.
        let log = "0:2(11): error: no matching function for call to `rotateVec2(mat4, float)'; candidates are:\n\
                   0:2(11): error:    vec2 rotateVec2(vec2, float)\n";
        assert!(!repair(&mut text, log));
    }

    #[test]
    fn a_comparison_counted_with_is_a_number() {
        // True is one and false is nought where the original multiplies by them.
        let mut text = source("depth *= (depth < limit) * 6.0 + step(a, (b));\n");
        assert!(repair(&mut text, "0:1(11): error: operands to arithmetic operators must be numeric\n"));
        assert!(text.contains("depth *= float(depth < limit) * 6.0 + step(a, (b));"), "{text}");
        assert!(!repair(&mut text, "0:1(11): error: operands to arithmetic operators must be numeric\n"));
    }

    /// A fragment stage as it is assembled: with the prelude, and a combo.
    fn fragment(body: &str) -> String {
        format!(
            "#version 330 core\n{}{}#define INVERT 1\n#line 1\n{body}",
            super::super::prelude::common(),
            super::super::prelude::FRAGMENT
        )
    }

    /// The same for the vertex stage.
    fn vertex(body: &str) -> String {
        format!(
            "#version 330 core\n{}{}#line 1\n{body}",
            super::super::prelude::common(),
            super::super::prelude::VERTEX
        )
    }

    #[test]
    fn two_vectors_of_different_lengths_meet_at_the_shorter() {
        // Mesa does not say which kinds met; the declarations do.
        let mut text = fragment(
            "varying vec4 v_TexCoord;\nuniform float u_vignetteIntensity;\nvoid main() {\n\
             \tfloat cd = pow(length(abs(v_TexCoord - CAST2(0.5)) * 1.25), 3.0) * u_vignetteIntensity;\n}\n",
        );
        let log = "0:4(36): error: vector size mismatch for arithmetic operator\n\
                   0:4(32): error: no matching function for call to `abs(error)'; candidates are:\n\
                   0:4(32): error:    float abs(float)\n";
        assert!(repair(&mut text, log));
        assert!(
            text.contains(
                "\tfloat cd = pow(length(abs(vec2(v_TexCoord) - CAST2(0.5)) * 1.25), 3.0) * u_vignetteIntensity;\n"
            ),
            "{text}"
        );
        assert!(!repair(&mut text, log), "nothing left to repair");
    }

    #[test]
    fn a_vertex_stage_output_is_read_at_the_length_it_is_used_at() {
        let mut text = vertex(
            "uniform vec2 g_Scale;\nuniform vec2 g_Scale_FollowCursor_Multiplier;\nvarying vec4 v_PointerUV;\n\
             void main() {\n\
             vec2 da = v_PointerUV * (g_Scale * g_Scale_FollowCursor_Multiplier) * 0.001; // scaled\n}\n",
        );
        let log = "0:5(12): error: vector size mismatch for arithmetic operator\n\
                   0:5(12): error: operands to arithmetic operators must be numeric\n";
        assert!(repair(&mut text, log));
        assert!(
            text.contains(
                "vec2 da = vec2(v_PointerUV) * (g_Scale * g_Scale_FollowCursor_Multiplier) * 0.001; // scaled\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_compound_assignment_takes_what_its_target_holds() {
        let mut text = fragment(
            "uniform vec4 g_Texture0Resolution;\nuniform float g_Strength;\nvoid main() {\n\
             \tvec2 strength = vec2(g_Strength * g_Strength * 60);\n\tstrength /= g_Texture0Resolution;\n}\n",
        );
        let log = "0:5(2): error: vector size mismatch for arithmetic operator\n\
                   0:5(2): error: could not implicitly convert error to vec2\n";
        assert!(repair(&mut text, log));
        assert!(text.contains("\tstrength /= vec2(g_Texture0Resolution);\n"), "{text}");
    }

    #[test]
    fn whole_numbers_with_and_without_a_sign_meet_without_one() {
        // GLSL 3.30 has no conversion between int and uint; the original makes both uint.
        let mut text = fragment(
            "#define RESOLUTION 64\nuniform float frequency;\nvoid main() {\n\
             \tuint barFreq1 = uint(wepaper_rem(frequency, RESOLUTION));\n\
             \tuint barFreq2 = wepaper_rem((barFreq1 + 1), RESOLUTION);\n}\n",
        );
        let log = "0:5(31): error: could not implicitly convert operands to arithmetic operator\n\
                   0:5(18): error: no matching function for call to `wepaper_rem(error, int)'; candidates are:\n\
                   0:5(18): error:    float wepaper_rem(float, float)\n";
        assert!(repair(&mut text, log));
        assert!(text.contains("\tuint barFreq2 = wepaper_rem((barFreq1 + 1u), RESOLUTION);\n"), "{text}");
    }

    #[test]
    fn a_number_or_a_vector_chooses_where_a_condition_is_wanted() {
        let mut text = fragment(
            "uniform vec4 a;\nuniform vec4 b;\nuniform vec4 c;\nvoid main() {\n\
             \tfloat mask = 0.5;\n\tmask = INVERT ? 1 - mask : mask;\n\
             \tfloat outside = 1.0;\n\tvec4 final = outside ? a : b;\n\
             \tvec4 each = c ? a : 0.0;\n}\n",
        );
        let told = |line: usize| format!("0:{line}(9): error: condition of `?:' must be scalar boolean\n");
        assert!(repair(&mut text, &told(6)));
        assert!(text.contains("\tmask = bool(INVERT) ? 1 - mask : mask;\n"), "{text}");
        assert!(repair(&mut text, &told(8)));
        assert!(text.contains("\tvec4 final = bool(outside) ? a : b;\n"), "{text}");
        // A vector condition chooses each component on its own.
        assert!(repair(&mut text, &told(9)));
        assert!(text.contains("\tvec4 each = mix(vec4(0.0), a, bvec4(c));\n"), "{text}");
    }

    #[test]
    fn a_function_returns_the_kind_it_says() {
        // Two functions with a `col` each, of different kinds: each line reads its own.
        let mut text = fragment(
            "vec3 drawSence(vec2 uv)\n{\n    vec4 col = vec4(0.,0.,0.,0.);\n    return col;\n}\n\
             void main() {\n\tvec3 col = drawSence(vec2(0.0));\n\treturn;\n}\n",
        );
        let log = "0:4(2): error: `return' with wrong type vec4, in function `drawSence' returning vec3\n";
        assert!(repair(&mut text, log));
        assert!(text.contains("    return vec3(col);\n"), "{text}");
    }

    #[test]
    fn what_a_function_of_the_shader_is_given_is_made_what_it_takes() {
        let mut text = fragment(
            "varying vec4 v_TexCoord;\nvec2 rotateVec2(vec2 v, float a) { return v; }\n\
             void shift(inout vec2 v) { v += 1.0; }\nvoid main() {\n\
             \tvec2 at = rotateVec2(v_TexCoord, 1);\n\tshift(v_TexCoord);\n}\n",
        );
        assert!(repair(
            &mut text,
            "0:5(12): error: no matching function for call to `rotateVec2(vec4, int)'\n"
        ));
        assert!(text.contains("\tvec2 at = rotateVec2(vec2(v_TexCoord), 1);\n"), "{text}");
        // What a function writes into cannot be a conversion.
        let before = text.clone();
        assert!(!repair(&mut text, "0:6(2): error: no matching function for call to `shift(vec4)'\n"));
        assert_eq!(text, before);
    }

    #[test]
    fn a_name_of_two_kinds_is_not_guessed_at() {
        // Declared twice with different kinds (in two branches of an #if, say), `v` is
        // not known: the line is left to the repairs that read the message.
        let mut text = fragment(
            "uniform vec3 v;\nuniform vec4 v;\nuniform vec4 w;\nvoid main() {\n\tvec2 x = v + w;\n}\n",
        );
        let before = text.clone();
        assert!(!repair(&mut text, "0:5(13): error: vector size mismatch for arithmetic operator\n"));
        assert_eq!(text, before);
    }

    #[test]
    fn a_statement_over_several_lines_keeps_its_lines() {
        let mut text = fragment(
            "uniform vec4 a;\nuniform vec2 b;\nvoid main() {\n\tvec2 c = a\n\t\t+ b;\n\tgl_FragColor = vec4(c, 0.0, 1.0);\n}\n",
        );
        assert!(repair(&mut text, "0:4(11): error: vector size mismatch for arithmetic operator\n"));
        assert!(text.contains("\tvec2 c = vec2(a)\n\t\t+ b;\n\tgl_FragColor"), "{text}");
    }

    #[test]
    fn comparisons_and_truth_are_what_the_original_makes_of_them() {
        let mut text = fragment(
            "uniform vec3 a;\nuniform vec3 b;\nuniform float t;\nvoid main() {\n\
             \tvec3 near = (a < b) * 2.0;\n\tif (t) discard;\n\tfloat on = t > 0.5 && INVERT;\n}\n",
        );
        let told = |line: usize| format!("0:{line}(9): error: something about kinds\n");
        // HLSL compares vectors a component at a time.
        assert!(repair(&mut text, &told(5)));
        assert!(text.contains("\tvec3 near = vec3(lessThan(a, b)) * 2.0;\n"), "{text}");
        assert!(repair(&mut text, &told(6)));
        assert!(text.contains("\tif (bool(t)) discard;\n"), "{text}");
        assert!(repair(&mut text, &told(7)));
        assert!(text.contains("\tfloat on = float(t > 0.5 && bool(INVERT));\n"), "{text}");
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
        let mut condition = source("#if g_Texture0Resolution.y <= 640\n#endif\n");
        assert!(repair(&mut condition, "0:1(24): preprocessor error: syntax error, unexpected '.'\n"));
        assert!(condition.contains("#line 1\n#if 0\n#endif"));
    }
}
