//! The conversions the original makes by itself, written out.
//!
//! Wallpaper Engine's compiler treats its shaders like HLSL: a longer vector is cut to
//! a shorter one, a number is spread over a vector, `int`, `uint`, `float` and `bool`
//! mix freely, a comparison is a number. GLSL 3.30 takes none of that, and a strict
//! driver says only that two operands did not fit, not which kinds they were. So the
//! kinds are worked out here: what the shader declares is read ([`table`]), the
//! refused statements are parsed ([`parse`]) and typed from the inside out, and where
//! the original would convert, the conversion is written in: `v_TexCoord - CAST2(0.5)`
//! with a `vec4` varying becomes `vec2(v_TexCoord) - CAST2(0.5)`.
//!
//! Where a kind is not known for certain (a name declared with two kinds, a macro or
//! function nothing here knows), nothing that depends on it is changed.

mod condition;
mod kinds;
mod lex;
mod parse;
mod table;

use kinds::{BOOL, Base, Kind, builtin, common, converts, cost, implicit, operand, scalar};
use lex::{Class, Token, tokens};
use parse::{Node, Parser, What};
use table::{Function, Macro, Table};

/// Writes out the conversions the original makes by itself in the statements of
/// `source[start..end]`. Returns the new text of that stretch, or `None` when nothing
/// in it is sure to need one. Every line break stays where it was.
pub(super) fn convert(source: &str, start: usize, end: usize) -> Option<String> {
    let table = Table::scan(source);
    let walk = Walk {
        table: &table,
        function: table.function_at(start),
        text: &source[start..end],
        bound: Vec::new(),
        expanding: Vec::new(),
    };
    let cut = tokens(walk.text);
    if cut.iter().any(|t| t.class == Class::Directive) {
        return None;
    }
    let mut out = walk.text.to_owned();
    let mut edits = walk.statements(&cut);
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    for (from, to, text) in edits {
        out.replace_range(from..to, &text);
    }
    (out != walk.text).then_some(out)
}

/// An expression typed and written back, with its conversions.
#[derive(Debug, Clone)]
struct Typed {
    kind: Option<Kind>,
    text: String,
    /// The text is in brackets of its own: a conversion needs no more.
    grouped: bool,
    /// A whole number written in digits, which converts by a suffix.
    digits: bool,
}

impl Typed {
    fn new(kind: Option<Kind>, text: String) -> Self {
        Self { kind, text, grouped: false, digits: false }
    }
}

/// `typed` made a value of the kind `to`, as a constructor of `to` makes it.
fn to(typed: Typed, to: &Kind) -> String {
    if typed.kind.as_ref() == Some(to) {
        return typed.text;
    }
    if typed.digits {
        match to {
            Kind::Value(Base::Uint, 1) => return format!("{}u", typed.text),
            Kind::Value(Base::Float, 1) => return format!("{}.0", typed.text),
            _ => {}
        }
    }
    match to.word() {
        Some(word) if typed.grouped => format!("{word}{}", typed.text),
        Some(word) => format!("{word}({})", typed.text),
        None => typed.text,
    }
}

/// `typed` where a value of the kind `to` is wanted: as it is where GLSL takes it so,
/// converted where the original would convert it, as it is where that is not sure.
fn fit(typed: Typed, to_kind: &Kind) -> String {
    match &typed.kind {
        Some(kind) if !implicit(kind, to_kind) && converts(kind, to_kind) => to(typed, to_kind),
        _ => typed.text,
    }
}

/// `typed` where a truth is wanted: a number is true when it is not nought.
fn truth(typed: Typed) -> String {
    match typed.kind {
        Some(Kind::Value(base, 1)) if base != Base::Bool => to(typed, &BOOL),
        _ => typed.text,
    }
}

/// The variables GLSL declares itself.
fn built_in_variable(name: &str) -> Option<Kind> {
    Some(match name {
        "gl_Position" | "gl_FragCoord" => Kind::Value(Base::Float, 4),
        "gl_PointCoord" => Kind::Value(Base::Float, 2),
        "gl_FragDepth" | "gl_PointSize" => kinds::FLOAT,
        "gl_VertexID" | "gl_InstanceID" => scalar(Base::Int),
        "gl_FrontFacing" => BOOL,
        _ => return None,
    })
}

/// The width a swizzle such as `.xyz` reads from a vector of `width`.
fn swizzle(field: &str, width: u8) -> Option<u8> {
    let sets = ["xyzw", "rgba", "stpq"];
    let set = sets.iter().find(|set| field.chars().all(|c| set.contains(c)))?;
    let fits = field.chars().all(|c| set.find(c).is_some_and(|at| at < usize::from(width)));
    (fits && (1..=4).contains(&field.len()) && width > 1).then_some(field.len() as u8)
}

/// The signs an expression can start with.
const STARTS: [&str; 7] = ["(", "-", "+", "!", "~", "++", "--"];

/// Words that start no expression and need nothing converted where they stand.
const SKIPPED: [&str; 17] = [
    "else",
    "do",
    "for",
    "switch",
    "break",
    "continue",
    "discard",
    "const",
    "highp",
    "mediump",
    "lowp",
    "in",
    "out",
    "inout",
    "uniform",
    "varying",
    "attribute",
];

/// Typing in one text: the refused statements, or the body of a macro.
struct Walk<'a> {
    table: &'a Table,
    /// The function the statements are in.
    function: Option<&'a Function>,
    /// The text the nodes point into.
    text: &'a str,
    /// The parameters of the macro being typed, with the kinds of its arguments.
    bound: Vec<(String, Option<Kind>)>,
    /// The macros being typed: inside its own body a macro's name is not the macro.
    expanding: Vec<String>,
}

impl<'a> Walk<'a> {
    fn slice(&self, node: &Node<'_>) -> &'a str {
        &self.text[node.start..node.end]
    }

    /// The text of `node` with the text of some of its parts replaced.
    fn splice(&self, node: &Node<'_>, parts: Vec<(&Node<'_>, String)>) -> String {
        let mut out = String::new();
        let mut at = node.start;
        for (part, text) in parts {
            out.push_str(&self.text[at..part.start]);
            out.push_str(&text);
            at = part.end;
        }
        out.push_str(&self.text[at..node.end]);
        out
    }

    /// Whether a line break stands between the parts of `node` that are not `parts`:
    /// written anew, they would lose it.
    fn breaks_between(&self, node: &Node<'_>, parts: &[&Node<'_>]) -> bool {
        let mut at = node.start;
        for part in parts {
            if self.text[at..part.start].contains('\n') {
                return true;
            }
            at = part.end;
        }
        self.text[at..node.end].contains('\n')
    }

    /// The statements in `cut`, with each place where the text changes.
    fn statements(&self, cut: &[Token<'_>]) -> Vec<(usize, usize, String)> {
        let mut edits = Vec::new();
        let mut parser = Parser::new(cut);
        while let Some(token) = parser.peek() {
            let from = parser.at;
            match token.text {
                "if" | "while" => {
                    parser.at += 1;
                    if parser.take("(")
                        && let Some(condition) = parser.expression()
                    {
                        self.keep(&mut edits, &condition, truth(self.walk(&condition)));
                    }
                }
                "return" => {
                    parser.at += 1;
                    if !parser.sees(";")
                        && let Some(value) = parser.expression()
                    {
                        let typed = self.walk(&value);
                        let text = match self.function.map(|f| &f.gives).filter(|k| **k != Kind::Void) {
                            Some(gives) => fit(typed, gives),
                            None => typed.text,
                        };
                        self.keep(&mut edits, &value, text);
                    }
                }
                // Nothing in a struct's members or a function's parameters converts.
                "struct" => break,
                "case" | "default" => {
                    while parser.peek().is_some_and(|t| t.text != ":") {
                        parser.at += 1;
                    }
                }
                _ if token.class == Class::Sign && !STARTS.contains(&token.text)
                    || SKIPPED.contains(&token.text) =>
                {
                    parser.at += 1;
                }
                word => match self.table.kind_of(word) {
                    Some(kind) if cut.get(from + 1).is_some_and(|t| t.class == Class::Name) => {
                        if cut.get(from + 2).is_some_and(|t| t.text == "(") {
                            break;
                        }
                        parser.at += 1;
                        self.declarators(&mut parser, &kind, &mut edits);
                    }
                    _ => match parser.expression() {
                        Some(expression) => self.keep(&mut edits, &expression, self.walk(&expression).text),
                        None => parser.at = from + 1,
                    },
                },
            }
            if parser.at == from {
                parser.at += 1;
            }
        }
        edits
    }

    /// Notes the new `text` of `node`, when it is new.
    fn keep(&self, edits: &mut Vec<(usize, usize, String)>, node: &Node<'_>, text: String) {
        if text != self.slice(node) {
            edits.push((node.start, node.end, text));
        }
    }

    /// The names a declaration of `kind` declares, from the first: each initializer is
    /// made that kind.
    fn declarators(&self, parser: &mut Parser<'_, '_>, kind: &Kind, edits: &mut Vec<(usize, usize, String)>) {
        while parser.peek().is_some_and(|t| t.class == Class::Name) {
            parser.at += 1;
            let mut array = false;
            while parser.take("[") {
                array = true;
                while parser.peek().is_some_and(|t| t.text != "]") {
                    parser.at += 1;
                }
                parser.take("]");
            }
            if parser.take("=")
                && let Some(value) = parser.expression()
            {
                let typed = self.walk(&value);
                let text = if array { typed.text } else { fit(typed, kind) };
                self.keep(edits, &value, text);
            }
            if !parser.take(",") {
                break;
            }
        }
    }

    /// Types `node` and writes it back with the conversions it needs.
    fn walk(&self, node: &Node<'_>) -> Typed {
        match &node.what {
            What::Name(name) => Typed::new(self.variable(name), self.slice(node).to_owned()),
            What::Number(text) => {
                let lower = text.to_ascii_lowercase();
                let hex = lower.starts_with("0x");
                let base = if lower.ends_with('u') {
                    Base::Uint
                } else if !hex && (lower.contains(['.', 'e']) || lower.ends_with('f')) {
                    Base::Float
                } else {
                    Base::Int
                };
                let mut typed = Typed::new(Some(scalar(base)), (*text).to_owned());
                typed.digits = text.bytes().all(|b| b.is_ascii_digit());
                typed
            }
            What::Group(inner) => {
                let typed = self.walk(inner);
                let mut group = Typed::new(typed.kind.clone(), self.splice(node, vec![(inner, typed.text)]));
                group.grouped = true;
                group
            }
            What::Member(object, field) => {
                let typed = self.walk(object);
                let kind = match &typed.kind {
                    Some(Kind::Value(base, width)) => swizzle(field, *width).map(|w| Kind::Value(*base, w)),
                    Some(Kind::Struct(name)) => self
                        .table
                        .structs
                        .get(name)
                        .and_then(|members| members.iter().find(|(member, _)| member == field))
                        .map(|(_, kind)| kind.clone()),
                    _ => None,
                };
                Typed::new(kind, self.splice(node, vec![(object, typed.text)]))
            }
            What::Index(object, index) => {
                let (typed, at) = (self.walk(object), self.walk(index));
                let kind = match &typed.kind {
                    Some(Kind::Array(kind)) => Some((**kind).clone()),
                    Some(Kind::Value(base, width)) if *width > 1 => Some(scalar(*base)),
                    Some(Kind::Matrix(_, rows)) => Some(Kind::Value(Base::Float, *rows)),
                    _ => None,
                };
                // The original counts in whole numbers there.
                let at = match at.kind {
                    Some(Kind::Value(Base::Float | Base::Bool, 1)) => to(at, &scalar(Base::Int)),
                    _ => at.text,
                };
                Typed::new(kind, self.splice(node, vec![(object, typed.text), (index, at)]))
            }
            What::Prefix(op, operand) => {
                let typed = self.walk(operand);
                if *op == "!" {
                    return Typed::new(Some(BOOL), self.splice(node, vec![(operand, truth(typed))]));
                }
                let kind = typed.kind.clone().filter(|k| k.value().is_none_or(|v| v.0 != Base::Bool));
                Typed::new(kind, self.splice(node, vec![(operand, typed.text)]))
            }
            What::Postfix(_, operand) => {
                let typed = self.walk(operand);
                Typed::new(typed.kind.clone(), self.splice(node, vec![(operand, typed.text)]))
            }
            What::Binary(op, left, right) => self.binary(node, op, left, right),
            What::Ternary(condition, chosen, other) => self.ternary(node, condition, chosen, other),
            What::Assign(op, target, value) => self.assign(node, op, target, value),
            What::Call(name, arguments) => self.call(node, name, arguments),
        }
    }

    /// The kind of the variable or macro `name`.
    fn variable(&self, name: &str) -> Option<Kind> {
        if let Some((_, kind)) = self.bound.iter().find(|(bound, _)| bound == name) {
            return kind.clone();
        }
        if !self.expanding.iter().any(|e| e == name)
            && let Some(defined) = self.table.macros.get(name)
        {
            return match defined {
                Some(defined) if defined.params.is_none() => self.expand(name, defined, Vec::new()),
                _ => None,
            };
        }
        if let Some(kind) = self.function.and_then(|f| f.locals.get(name)) {
            return kind.clone();
        }
        if let Some(kind) = self.table.globals.get(name) {
            return kind.clone();
        }
        built_in_variable(name)
    }

    /// The kind of the macro `name` where it is used, its parameters `bound` to the
    /// kinds of its arguments. A body that would need a conversion of its own is not
    /// known: it cannot be repaired from where it is used.
    fn expand(&self, name: &str, defined: &'a Macro, bound: Vec<(String, Option<Kind>)>) -> Option<Kind> {
        if self.expanding.len() >= 16 {
            return None;
        }
        let cut = tokens(&defined.body);
        let body = parse::whole(&cut)?;
        let mut expanding = self.expanding.clone();
        expanding.push(name.to_owned());
        let walk = Walk { table: self.table, function: self.function, text: &defined.body, bound, expanding };
        let typed = walk.walk(&body);
        (typed.text == walk.slice(&body)).then_some(typed.kind).flatten()
    }

    fn binary(&self, node: &Node<'_>, op: &str, left: &Node<'_>, right: &Node<'_>) -> Typed {
        let (l, r) = (self.walk(left), self.walk(right));
        let unchanged = |l: Typed, r: Typed, kind: Option<Kind>| {
            Typed::new(kind, self.splice(node, vec![(left, l.text), (right, r.text)]))
        };
        let values = l.kind.as_ref().and_then(Kind::value).zip(r.kind.as_ref().and_then(Kind::value));
        match op {
            "+" | "-" | "*" | "/" => {
                let Some((a, b)) = values else {
                    let kind = matrix_product(op, l.kind.as_ref(), r.kind.as_ref());
                    return unchanged(l, r, kind);
                };
                let (base, width) = common(a, b);
                let (lt, rt) = (fit(l, &operand(base, width, a.1)), fit(r, &operand(base, width, b.1)));
                Typed::new(Some(Kind::Value(base, width)), self.splice(node, vec![(left, lt), (right, rt)]))
            }
            "%" | "&" | "|" | "^" => match values {
                Some((a, b)) if [a.0, b.0].iter().all(|base| matches!(base, Base::Int | Base::Uint)) => {
                    let (base, width) = common(a, b);
                    let (lt, rt) = (fit(l, &operand(base, width, a.1)), fit(r, &operand(base, width, b.1)));
                    Typed::new(
                        Some(Kind::Value(base, width)),
                        self.splice(node, vec![(left, lt), (right, rt)]),
                    )
                }
                _ => unchanged(l, r, None),
            },
            "<<" | ">>" => {
                let kind = l.kind.clone();
                unchanged(l, r, kind)
            }
            "<" | ">" | "<=" | ">=" | "==" | "!=" => {
                let Some((a, b)) = values else { return unchanged(l, r, None) };
                let (base, width) = common(a, b);
                let base = if a.0 == b.0 { a.0 } else { base };
                if width == 1 {
                    let (lt, rt) = (fit(l, &scalar(base)), fit(r, &scalar(base)));
                    return Typed::new(Some(BOOL), self.splice(node, vec![(left, lt), (right, rt)]));
                }
                // The original compares vectors a component at a time.
                let name = match op {
                    "<" => "lessThan",
                    ">" => "greaterThan",
                    "<=" => "lessThanEqual",
                    ">=" => "greaterThanEqual",
                    // GLSL compares whole vectors of one kind; nobody counts on more.
                    _ if l.kind == r.kind => return unchanged(l, r, Some(BOOL)),
                    _ => return unchanged(l, r, None),
                };
                if self.breaks_between(node, &[left, right]) || base == Base::Bool {
                    return unchanged(l, r, None);
                }
                let each = Kind::Value(base, width);
                let (lt, rt) = (fit(l, &each), fit(r, &each));
                Typed::new(Some(Kind::Value(Base::Bool, width)), format!("{name}({lt}, {rt})"))
            }
            "&&" | "||" | "^^" => {
                Typed::new(Some(BOOL), self.splice(node, vec![(left, truth(l)), (right, truth(r))]))
            }
            _ => unchanged(l, r, None),
        }
    }

    fn ternary(&self, node: &Node<'_>, condition: &Node<'_>, chosen: &Node<'_>, other: &Node<'_>) -> Typed {
        let (c, a, b) = (self.walk(condition), self.walk(chosen), self.walk(other));
        let values = a.kind.as_ref().and_then(Kind::value).zip(b.kind.as_ref().and_then(Kind::value));
        match (c.kind.as_ref().and_then(Kind::value), values) {
            // A vector chooses a component at a time.
            (Some((base, width)), Some((x, y))) if width > 1 => {
                let width = [x.1, y.1].into_iter().filter(|w| *w > 1).fold(width, u8::min);
                let floats = common(x, y).0 == Base::Float;
                if !floats || self.breaks_between(node, &[condition, chosen, other]) {
                    return Typed::new(
                        None,
                        self.splice(node, vec![(condition, c.text), (chosen, a.text), (other, b.text)]),
                    );
                }
                let each = Kind::Value(Base::Float, width);
                let choice = if base == Base::Bool
                    && width == c.kind.as_ref().and_then(Kind::value).map_or(0, |v| v.1)
                {
                    c.text
                } else {
                    to(c, &Kind::Value(Base::Bool, width))
                };
                let (a, b) = (to(a, &each), to(b, &each));
                Typed::new(Some(each), format!("mix({b}, {a}, {choice})"))
            }
            (condition_kind, values) => {
                let choice = if condition_kind.is_some() { truth(c) } else { c.text };
                let (kind, a, b) = match values {
                    Some((x, y)) => {
                        let (base, width) = common(x, y);
                        let base = if x.0 == y.0 { x.0 } else { base };
                        let each = Kind::Value(base, width);
                        (Some(each.clone()), to(a, &each), to(b, &each))
                    }
                    None if a.kind.is_some() && a.kind == b.kind => (a.kind.clone(), a.text, b.text),
                    None => (None, a.text, b.text),
                };
                Typed::new(kind, self.splice(node, vec![(condition, choice), (chosen, a), (other, b)]))
            }
        }
    }

    fn assign(&self, node: &Node<'_>, op: &str, target: &Node<'_>, value: &Node<'_>) -> Typed {
        let (t, v) = (self.walk(target), self.walk(value));
        let kind = t.kind.clone();
        let plain = |t: Typed, v: String| {
            Typed::new(t.kind.clone(), self.splice(node, vec![(target, t.text), (value, v)]))
        };
        let Some(to_kind) = kind.clone() else { return plain(t, v.text) };
        if op == "=" {
            return plain(t, fit(v, &to_kind));
        }
        let (Some((tb, tw)), Some((vb, vw))) = (to_kind.value(), v.kind.as_ref().and_then(Kind::value))
        else {
            return plain(t, v.text);
        };
        if vw != 1 && vw < tw || tb == Base::Bool || vb == Base::Bool && tb != Base::Float {
            return plain(t, v.text);
        }
        // A number stays one, a longer vector is cut to what the target holds.
        let width = if vw == 1 { 1 } else { tw };
        let integers = matches!(tb, Base::Int | Base::Uint);
        if integers && vb == Base::Float {
            // `i *= 0.5` is `i = int(i * 0.5)` in the original: the fraction counts.
            if !["+=", "-=", "*=", "/="].contains(&op) || self.breaks_between(node, &[target, value]) {
                return plain(t, v.text);
            }
            let Some(word) = to_kind.word() else { return plain(t, v.text) };
            let v = fit(v, &Kind::Value(Base::Float, width));
            // The value was all on the right of the sign; written out, it keeps together.
            let v = match value.what {
                What::Binary(..) | What::Ternary(..) | What::Assign(..) => format!("({v})"),
                _ => v,
            };
            let sign = &op[..1];
            let text = format!("{} = {word}({} {sign} {v})", t.text, t.text);
            return Typed::new(kind, text);
        }
        plain(t, fit(v, &Kind::Value(tb, width)))
    }

    fn call(&self, node: &Node<'_>, name: &str, arguments: &[Node<'_>]) -> Typed {
        let typed: Vec<Typed> = arguments.iter().map(|a| self.walk(a)).collect();
        let given: Vec<Option<Kind>> = typed.iter().map(|t| t.kind.clone()).collect();
        let written = |texts: Vec<String>| self.splice(node, arguments.iter().zip(texts).collect());
        let as_they_are = |typed: Vec<Typed>| typed.into_iter().map(|t| t.text).collect::<Vec<_>>();
        // `frac` is `fract`, `max` is the prelude's `wepaper_max`.
        let mut callee = name;
        for _ in 0..8 {
            match self.table.alias(callee) {
                Some(alias) if alias != callee && !self.expanding.iter().any(|e| e == callee) => {
                    callee = alias
                }
                _ => break,
            }
        }
        if let Some(Some(defined)) = self.table.macros.get(callee)
            && let Some(params) = &defined.params
            && !self.expanding.iter().any(|e| e == callee)
        {
            let kind = (params.len() == given.len())
                .then(|| self.expand(callee, defined, params.iter().cloned().zip(given).collect()))
                .flatten();
            return Typed::new(kind, written(as_they_are(typed)));
        }
        if let Some(kind) = self.table.kind_of(callee) {
            return Typed::new(Some(kind), written(as_they_are(typed)));
        }
        let candidates: Vec<&Function> = self
            .table
            .functions
            .iter()
            .filter(|f| f.name == callee && f.params.len() == given.len())
            .collect();
        if !candidates.is_empty() {
            return match overload(&candidates, &given) {
                Some(chosen) => {
                    let texts = typed
                        .into_iter()
                        .zip(&chosen.params)
                        .map(|(t, param)| if param.writes { t.text } else { fit(t, &param.kind) })
                        .collect();
                    Typed::new(Some(chosen.gives.clone()), written(texts))
                }
                None => {
                    let gives = &candidates[0].gives;
                    let kind = candidates.iter().all(|f| f.gives == *gives).then(|| gives.clone());
                    Typed::new(kind, written(as_they_are(typed)))
                }
            };
        }
        match builtin(callee, &given) {
            Some(signature) => {
                let texts = typed
                    .into_iter()
                    .zip(&signature.takes)
                    .map(|(t, takes)| match takes {
                        Some(kind) => fit(t, kind),
                        None => t.text,
                    })
                    .collect();
                Typed::new(Some(signature.gives), written(texts))
            }
            None => Typed::new(None, written(as_they_are(typed))),
        }
    }
}

/// The kind of a product or a sum where a matrix takes part.
fn matrix_product(op: &str, left: Option<&Kind>, right: Option<&Kind>) -> Option<Kind> {
    match (left?, right?) {
        (Kind::Matrix(columns, rows), Kind::Value(Base::Float, width)) if op == "*" && width == columns => {
            Some(Kind::Value(Base::Float, *rows))
        }
        (Kind::Value(Base::Float, width), Kind::Matrix(columns, rows)) if op == "*" && width == rows => {
            Some(Kind::Value(Base::Float, *columns))
        }
        (Kind::Matrix(c, r), Kind::Matrix(columns, rows)) if op == "*" && c == rows => {
            Some(Kind::Matrix(*columns, *r))
        }
        (Kind::Matrix(c, r), Kind::Matrix(columns, rows)) if c == columns && r == rows => {
            Some(Kind::Matrix(*c, *r))
        }
        (matrix @ Kind::Matrix(..), Kind::Value(_, 1)) | (Kind::Value(_, 1), matrix @ Kind::Matrix(..)) => {
            Some(matrix.clone())
        }
        _ => None,
    }
}

/// The overload of a function the original would call with arguments of the kinds
/// `given`: the one that converts least. `None` when two convert as little, or a kind
/// is not known and more than one could be meant.
fn overload<'f>(candidates: &[&'f Function], given: &[Option<Kind>]) -> Option<&'f Function> {
    if given.iter().any(Option::is_none) {
        return (candidates.len() == 1).then(|| candidates[0]);
    }
    let priced = candidates.iter().filter_map(|f| {
        let price = f
            .params
            .iter()
            .zip(given)
            .map(|(param, kind)| {
                let kind = kind.as_ref()?;
                if param.writes { (*kind == param.kind).then_some(0.0) } else { cost(kind, &param.kind) }
            })
            .sum::<Option<f32>>()?;
        Some((price, *f))
    });
    let mut best: Option<(f32, &Function)> = None;
    let mut tied = false;
    for (price, f) in priced {
        match best {
            Some((least, chosen)) if price == least && chosen.params != f.params => tied = true,
            Some((least, _)) if price >= least => {}
            _ => {
                best = Some((price, f));
                tied = false;
            }
        }
    }
    best.filter(|_| !tied).map(|(_, f)| f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass over line `number` (from 0) of `source`.
    fn line(source: &str, number: usize) -> Option<String> {
        let start: usize = source.split_inclusive('\n').take(number).map(str::len).sum();
        let end = start + source[start..].find('\n').map_or(source.len() - start, |n| n + 1);
        convert(source, start, end)
    }

    #[test]
    fn a_whole_number_takes_the_fraction_it_is_counted_with_before_it_is_whole_again() {
        let source = "uniform float g_Audio[16];\nvoid main() {\nint i = 3;\nfloat f = 0.5;\n\
                      \ti *= f + g_Audio[f * 4.0];\n}\n";
        assert_eq!(line(source, 4).as_deref(), Some("\ti = int(i * (f + g_Audio[int(f * 4.0)]));\n"));
    }

    #[test]
    fn a_macro_is_typed_from_its_body_and_its_arguments() {
        let source = "#define SCALE(v) ((v) * 2.0)\n#define HALF (THREE / 2.0)\n#define THREE vec3(3.0)\n\
                      #define SWAP(v) v.zyxw\n#define LEFT RIGHT\n#define RIGHT LEFT\n\
                      uniform vec4 a;\nvoid main() {\n\tvec2 b = SCALE(a) + HALF + SWAP(a) + LEFT(a);\n}\n";
        // Macros that stand for each other stand for nothing known: the sum they are in
        // is not known either, and only the parts whose kinds are known are converted.
        assert_eq!(
            line(source, 8).as_deref(),
            Some("\tvec2 b = vec3(SCALE(a)) + HALF + vec3(SWAP(a)) + LEFT(a);\n")
        );
        let source = source.replace(" + LEFT(a)", "");
        assert_eq!(
            line(&source, 8).as_deref(),
            Some("\tvec2 b = vec2(vec3(SCALE(a)) + HALF + vec3(SWAP(a)));\n")
        );
    }

    #[test]
    fn nothing_changes_where_nothing_needs_to() {
        let source =
            "uniform vec4 a;\nuniform mat4 m;\nvoid main() {\n\tvec4 b = m * a + vec4(a.xyz, 1.0) * 2;\n}\n";
        assert_eq!(line(source, 3), None);
    }
}
