//! Loading a shader and assembling it for the driver.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;

use super::declared::Declared;
use super::{fixups, prelude};

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
        let text = fixups::rename_own_mod(&fixups::rename_reserved(&inlined));
        Self { stage, text, declared }
    }

    /// The complete GLSL for the driver, with `combos` defined.
    pub fn assemble(&self, combos: &Combos) -> String {
        let mut out = String::with_capacity(self.text.len() + 2048);
        out.push_str("#version 330 core\n");
        out.push_str(prelude::COMMON);
        out.push_str(match self.stage {
            Stage::Vertex => prelude::VERTEX,
            Stage::Fragment => prelude::FRAGMENT,
        });
        for (name, value) in combos {
            let _ = writeln!(out, "#define {name} {value}");
        }
        out.push_str("#line 1\n");
        out.push_str(&self.text);
        out
    }
}

/// Copies `text` to `out`, replacing `#include` lines by the files they name.
fn inline(text: &str, include: &dyn Fn(&str) -> Option<String>, seen: &mut HashSet<String>, out: &mut String, depth: usize) {
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
        } else if directive.starts_with("#require") || directive.starts_with("#version") || directive.starts_with("precision ") {
            let _ = writeln!(out, "// {}", directive);
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
}
