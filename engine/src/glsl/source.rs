//! Loading a shader and assembling it for the driver.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;

use super::declared::Declared;
use super::{fixups, prelude};

/// What marks the start of the shader's own text in assembled source.
pub(super) const BODY: &str = "#line 1\n";
/// The same for lenient source.
pub(super) const BODY_LENIENT: &str = "#line 0\n";

/// Combo values by name.
pub type Combos = BTreeMap<String, i32>;

/// Which stage of the pipeline a shader is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Runs per vertex.
    Vertex,
    /// Runs per pixel.
    Fragment,
}

/// A shader with its includes inlined and its declarations read.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// The stage it is for.
    pub stage: Stage,
    /// The text, includes inlined, repairs applied.
    pub text: String,
    /// What its comments declare.
    pub declared: Declared,
}

impl Source {
    /// Prepares shader text: inlines `#include`s (each file once), drops directives
    /// GLSL does not have, applies the repairs and reads the declarations.
    ///
    /// `include` returns the text of a file named in an `#include`.
    pub fn load(stage: Stage, text: &str, include: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut inlined = String::with_capacity(text.len() * 2);
        inline(text, include, &mut HashSet::new(), &mut inlined, 0);
        let declared = Declared::scan(&inlined);
        let text = fixups::hoist_samplers(&fixups::rename_own_mod(&fixups::rename_reserved(&inlined)));
        Self { stage, text, declared }
    }

    /// The complete GLSL for the driver, with `combos` defined.
    ///
    /// `lenient` leaves out the `#version` line and everything that needs one. A
    /// compatibility context then compiles the shader as the language Wallpaper
    /// Engine's authors actually write: GLSL with HLSL's implicit conversions.
    /// Only some drivers take that; see [`crate::gpu::Caps::lenient_shaders`].
    pub fn assemble(&self, combos: &Combos, lenient: bool) -> String {
        let mut out = String::with_capacity(self.text.len() + 2048);
        if !lenient {
            out.push_str("#version 330 core\n");
        }
        out.push_str(prelude::common());
        if !lenient {
            out.push_str(match self.stage {
                Stage::Vertex => prelude::VERTEX,
                Stage::Fragment => prelude::FRAGMENT,
            });
        }
        for (name, value) in combos {
            let _ = writeln!(out, "#define {name} {value}");
        }
        // The body's lines count from 1 for the driver's messages. Before GLSL 3.30
        // `#line n` numbered the next line n + 1.
        out.push_str(if lenient { BODY_LENIENT } else { BODY });
        out.push_str(&self.text);
        out
    }
}

/// Copies `text` to `out`, replacing `#include` lines by the files they name.
fn inline(
    text: &str,
    include: &dyn Fn(&str) -> Option<String>,
    seen: &mut HashSet<String>,
    out: &mut String,
    depth: usize,
) {
    for line in text.lines() {
        let directive = line.trim_start();
        if let Some(rest) = directive.strip_prefix("#include") {
            let name = rest.trim().trim_matches(['"', '<', '>']).to_owned();
            // Headers have no guards: a second include of one would redefine its functions.
            if depth < 16 && seen.insert(name.to_lowercase()) {
                match include(&name) {
                    Some(header) => inline(&header, include, seen, out, depth + 1),
                    None => {
                        let _ = writeln!(out, "// missing include: {name}");
                    }
                }
            }
        } else if let Some(text) =
            directive.strip_prefix("#require").and_then(|name| super::lighting::required(name.trim()))
        {
            out.push_str(text);
        } else if directive.starts_with("#require")
            || directive.starts_with("#version")
            || directive.starts_with("precision ")
        {
            let _ = writeln!(out, "// {}", directive);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
}
