//! What a shader declares: variables, functions, structs and macros, with their kinds.

use std::collections::HashMap;

use super::condition::Conditions;
use super::kinds::Kind;
use super::lex::{Class, Token, tokens};

/// A parameter of a function.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Param {
    pub kind: Kind,
    /// `out` or `inout`: the function writes into what it is given, which must then
    /// be a variable of this very kind.
    pub writes: bool,
}

/// A function the shader (or the prelude) declares.
#[derive(Debug, Clone)]
pub(super) struct Function {
    pub name: String,
    pub gives: Kind,
    pub params: Vec<Param>,
    /// Where its body is in the source.
    pub body: Option<(usize, usize)>,
    /// Its parameters and the variables its body declares; `None` for a name that is
    /// declared with two kinds.
    pub locals: HashMap<String, Option<Kind>>,
}

/// A `#define`.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Macro {
    /// The names of its parameters, for one that is called like a function.
    pub params: Option<Vec<String>>,
    pub body: String,
}

/// Everything the source declares.
#[derive(Debug, Default)]
pub(super) struct Table {
    /// The variables outside functions; `None` for a name declared with two kinds.
    pub globals: HashMap<String, Option<Kind>>,
    pub functions: Vec<Function>,
    /// `None` for a name defined twice, differently.
    pub macros: HashMap<String, Option<Macro>>,
    pub structs: HashMap<String, Vec<(String, Kind)>>,
}

/// Words in front of a declaration that do not change its kind.
const QUALIFIERS: [&str; 18] = [
    "const",
    "uniform",
    "varying",
    "attribute",
    "in",
    "out",
    "inout",
    "flat",
    "smooth",
    "noperspective",
    "centroid",
    "invariant",
    "highp",
    "mediump",
    "lowp",
    "precise",
    "static",
    "extern",
];

/// Adds `name` with `kind`; a name met again with another kind is not known.
fn record(names: &mut HashMap<String, Option<Kind>>, name: &str, kind: Kind) {
    names
        .entry(name.to_owned())
        .and_modify(|known| {
            if known.as_ref() != Some(&kind) {
                *known = None;
            }
        })
        .or_insert(Some(kind));
}

/// Where the bracket that closes the one at `open` is.
fn closing(code: &[Token<'_>], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (at, token) in code.iter().enumerate().skip(open) {
        match token.text {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

impl Table {
    /// Reads the declarations of a whole assembled source.
    pub fn scan(source: &str) -> Self {
        let mut table = Self::default();
        // What the preprocessor leaves out is no part of the shader the driver compiled.
        let mut conditions = Conditions::default();
        let mut code = Vec::new();
        for token in tokens(source) {
            if token.class != Class::Directive {
                if conditions.compiled() {
                    code.push(token);
                }
            } else if !conditions.read(token.text, &table.macros) && conditions.compiled() {
                table.define(token.text);
            }
        }
        let mut at = 0;
        while at < code.len() {
            at = table.declaration(&code, at);
        }
        table
    }

    /// Reads one `#define`.
    fn define(&mut self, directive: &str) {
        let Some(rest) =
            directive.strip_prefix('#').map(str::trim_start).and_then(|r| r.strip_prefix("define"))
        else {
            return;
        };
        if !rest.starts_with([' ', '\t']) {
            return;
        }
        let rest = rest.trim_start();
        let name_end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len());
        let (name, rest) = rest.split_at(name_end);
        // A bracket right after the name makes a macro that is called like a function.
        let (params, body) = match rest.strip_prefix('(').and_then(|r| r.split_once(')')) {
            Some((list, body)) => {
                (Some(list.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()), body)
            }
            None => (None, rest),
        };
        let body = body.replace("\\\n", " ").trim().to_owned();
        let defined = Macro { params, body };
        self.macros
            .entry(name.to_owned())
            .and_modify(|known| {
                if known.as_ref() != Some(&defined) {
                    *known = None;
                }
            })
            .or_insert(Some(defined));
    }

    /// The kind a word names: a GLSL kind, a struct, or a macro that stands for one
    /// (`float3`).
    pub fn kind_of(&self, word: &str) -> Option<Kind> {
        let mut word = word;
        for _ in 0..8 {
            if let Some(kind) = Kind::named(word) {
                return Some(kind);
            }
            if self.structs.contains_key(word) {
                return Some(Kind::Struct(word.to_owned()));
            }
            word = self.alias(word)?;
        }
        None
    }

    /// The single name a macro without parameters stands for.
    pub fn alias(&self, word: &str) -> Option<&str> {
        let defined = self.macros.get(word)?.as_ref().filter(|m| m.params.is_none())?;
        let body = defined.body.as_str();
        (!body.is_empty() && body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')).then_some(body)
    }

    /// The function whose body holds the place `at` of the source.
    pub fn function_at(&self, at: usize) -> Option<&Function> {
        self.functions.iter().find(|f| f.body.is_some_and(|(start, end)| (start..end).contains(&at)))
    }

    /// Reads the declaration outside functions that starts at `at`; returns where the
    /// next one starts.
    fn declaration(&mut self, code: &[Token<'_>], mut at: usize) -> usize {
        let start = at;
        while let Some(token) = code.get(at) {
            if token.text == "layout" && code.get(at + 1).is_some_and(|t| t.text == "(") {
                at = closing(code, at + 1).map_or(code.len(), |close| close + 1);
            } else if QUALIFIERS.contains(&token.text) {
                at += 1;
            } else {
                break;
            }
        }
        let Some(token) = code.get(at) else { return code.len() };
        if token.text == "struct" {
            return self.structure(code, at);
        }
        let name = code.get(at + 1).filter(|t| t.class == Class::Name);
        let Some((kind, name)) = self.kind_of(token.text).zip(name) else {
            return skip_statement(code, at.max(start + 1).min(code.len()));
        };
        if code.get(at + 2).is_some_and(|t| t.text == "(") {
            return self.function(code, kind, name.text, at + 2);
        }
        let mut globals = std::mem::take(&mut self.globals);
        let next = declarators(code, at + 1, &kind, &mut globals);
        self.globals = globals;
        next
    }

    /// Reads `struct Name { members };`.
    fn structure(&mut self, code: &[Token<'_>], at: usize) -> usize {
        let Some(name) = code.get(at + 1).filter(|t| t.class == Class::Name) else {
            return skip_statement(code, at + 1);
        };
        let Some(close) = code.get(at + 2).filter(|t| t.text == "{").and_then(|_| closing(code, at + 2))
        else {
            return skip_statement(code, at + 1);
        };
        let mut members = HashMap::new();
        let mut inside = at + 3;
        while inside < close {
            match self.kind_of(code[inside].text) {
                Some(kind) => inside = declarators(&code[..close], inside + 1, &kind, &mut members),
                None => inside = skip_statement(&code[..close], inside + 1),
            }
        }
        let members = members.into_iter().filter_map(|(name, kind)| Some((name, kind?))).collect();
        self.structs.insert(name.text.to_owned(), members);
        skip_statement(code, close + 1)
    }

    /// Reads a function whose parameter list opens at `open`, with its body.
    fn function(&mut self, code: &[Token<'_>], gives: Kind, name: &str, open: usize) -> usize {
        let Some(close) = closing(code, open) else { return code.len() };
        let mut params = Vec::new();
        let mut locals = HashMap::new();
        for param in code[open + 1..close].split(|t| t.text == ",") {
            let writes = param.iter().any(|t| matches!(t.text, "out" | "inout"));
            let mut words = param.iter().filter(|t| !QUALIFIERS.contains(&t.text));
            let Some(kind) = words.next().and_then(|t| self.kind_of(t.text)) else { continue };
            if kind == Kind::Void {
                continue;
            }
            let array = param.iter().any(|t| t.text == "[");
            let kind = if array { Kind::Array(kind.into()) } else { kind };
            if let Some(named) = words.next().filter(|t| t.class == Class::Name) {
                record(&mut locals, named.text, kind.clone());
            }
            params.push(Param { kind, writes });
        }
        let (body, next) = match code.get(close + 1) {
            Some(t) if t.text == "{" => match closing(code, close + 1) {
                Some(end) => {
                    self.locals(&code[close + 2..end], &mut locals);
                    (Some((t.start, code[end].end)), end + 1)
                }
                None => (None, code.len()),
            },
            _ => (None, skip_statement(code, close + 1)),
        };
        self.functions.push(Function { name: name.to_owned(), gives, params, body, locals });
        next
    }

    /// Reads the variables a function body declares: wherever a kind is followed by
    /// a name.
    fn locals(&self, body: &[Token<'_>], locals: &mut HashMap<String, Option<Kind>>) {
        let mut at = 0;
        while at + 1 < body.len() {
            let declares = body[at + 1].class == Class::Name
                && (at == 0 || body[at - 1].text != ".")
                && body.get(at + 2).is_none_or(|t| t.text != "(");
            match self.kind_of(body[at].text).filter(|_| declares) {
                Some(kind) => at = declarators(body, at + 1, &kind, locals),
                None => at += 1,
            }
        }
    }
}

/// Reads the names a declaration of `kind` declares, from the first name at `at`:
/// `a = 1.0, b[4], c;`. Returns where the declaration ends.
fn declarators(
    code: &[Token<'_>],
    mut at: usize,
    kind: &Kind,
    names: &mut HashMap<String, Option<Kind>>,
) -> usize {
    while let Some(name) = code.get(at).filter(|t| t.class == Class::Name) {
        at += 1;
        let mut declared = kind.clone();
        while code.get(at).is_some_and(|t| t.text == "[") {
            at = closing(code, at).map_or(code.len(), |close| close + 1);
            declared = Kind::Array(declared.into());
        }
        record(names, name.text, declared);
        // An initializer runs to the comma or the end that is not inside brackets.
        let mut depth = 0i32;
        while let Some(token) = code.get(at) {
            match token.text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" if depth == 0 => return at,
                ")" | "]" | "}" => depth -= 1,
                ";" if depth == 0 => return at + 1,
                "," if depth == 0 => break,
                _ => {}
            }
            at += 1;
        }
        at += 1;
    }
    at
}

/// Where the statement that `at` is in ends: after its `;`, or after the block it
/// opens.
fn skip_statement(code: &[Token<'_>], mut at: usize) -> usize {
    while let Some(token) = code.get(at) {
        match token.text {
            ";" => return at + 1,
            "{" => return closing(code, at).map_or(code.len(), |close| close + 1),
            _ => at += 1,
        }
    }
    code.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_preprocessor_leaves_out_declares_nothing() {
        let source = "#define LEFT 1\n#if LEFT == 1\nfloat bar;\n#else\nint bar;\n#define LEFT 2\n#endif\n\
                      #ifdef NOTHING\nvec2 gone;\n#elif defined(LEFT) && !(2 < 1)\nvec3 kept;\n#else\nvec4 kept;\n#endif\n\
                      #if WHATEVER(1)\nvec2 both;\n#else\nvec3 both;\n#endif\n\
                      #ifndef LEFT\n#if 1\nvec2 inside;\n#endif\n#endif\n";
        let table = Table::scan(source);
        assert_eq!(table.globals["bar"], Kind::named("float"));
        assert_eq!(table.macros["LEFT"].as_ref().map(|m| m.body.as_str()), Some("1"));
        assert!(!table.globals.contains_key("gone"));
        assert_eq!(table.globals["kept"], Kind::named("vec3"));
        // A condition that cannot be worked out leaves both branches in.
        assert_eq!(table.globals["both"], None);
        assert!(!table.globals.contains_key("inside"));
    }

    #[test]
    fn declarations_are_found_with_their_kinds_and_their_places() {
        let source = "#define float3 vec3\n#define RESOLUTION 64\n#define CAST2(x) vec2(x)\n\
                      struct Light { vec3 colour; float range; };\n\
                      uniform float g_Audio[64]; // {\"material\":\"a\"}\n\
                      varying vec2 v_TexCoord, v_Other;\nuniform vec3 twice;\nuniform vec4 twice;\n\
                      float3 tint(in vec2 uv, out float alpha) {\n\tfloat3 c = vec3(uv, 1.0), d;\n\
                      \tfor (int i = 0; i < 2; i++) { Light l; }\n\treturn c;\n}\nvoid main() { }\n";
        let table = Table::scan(source);
        assert_eq!(table.globals["g_Audio"], Some(Kind::Array(Kind::named("float").expect("float").into())));
        assert_eq!(table.globals["v_Other"], Kind::named("vec2"));
        assert_eq!(table.globals["twice"], None, "two kinds: not known");
        assert_eq!(table.structs["Light"].len(), 2);
        let tint = &table.functions[0];
        assert_eq!((tint.name.as_str(), &tint.gives), ("tint", &Kind::named("vec3").expect("vec3")));
        assert_eq!(
            tint.params,
            [
                Param { kind: Kind::named("vec2").expect("vec2"), writes: false },
                Param { kind: Kind::named("float").expect("float"), writes: true }
            ]
        );
        for (name, kind) in [("uv", "vec2"), ("alpha", "float"), ("c", "vec3"), ("d", "vec3"), ("i", "int")] {
            assert_eq!(tint.locals[name], Kind::named(kind), "{name}");
        }
        assert_eq!(tint.locals["l"], Some(Kind::Struct("Light".to_owned())));
        let inside = source.find("return c").expect("in the source");
        assert_eq!(table.function_at(inside).map(|f| f.name.as_str()), Some("tint"));
        assert!(table.function_at(source.find("void main").expect("main")).is_none());
        assert_eq!(table.macros["CAST2"].as_ref().map(|m| m.body.as_str()), Some("vec2(x)"));
        assert_eq!(table.kind_of("float3"), Kind::named("vec3"));
    }
}
