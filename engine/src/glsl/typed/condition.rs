//! Which branches of `#if`s the preprocessor keeps.

use std::collections::HashMap;

use super::lex::{Class, tokens};
use super::parse::{Node, What, whole};
use super::table::Macro;

/// One `#if` being read: whether its branch is kept (`None`: not known), and whether
/// one of its branches before was.
struct Branch {
    kept: Option<bool>,
    before: Option<bool>,
}

/// The `#if`s around the place being read.
#[derive(Default)]
pub(super) struct Conditions {
    open: Vec<Branch>,
}

impl Conditions {
    /// Whether what stands here is compiled: `false` only when that is sure.
    pub fn compiled(&self) -> bool {
        self.open.iter().all(|branch| branch.kept != Some(false))
    }

    /// Reads a directive; returns whether it was a conditional one.
    pub fn read(&mut self, directive: &str, macros: &HashMap<String, Option<Macro>>) -> bool {
        let text = directive.trim_start_matches('#').trim_start();
        let word_end = text.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(text.len());
        let (word, rest) = text.split_at(word_end);
        let rest = rest.split("//").next().unwrap_or("").trim();
        // Inside a branch that is left out, conditions are not worked out, only counted.
        let outside = self.compiled();
        let condition = |rest: &str| if outside { truth(rest, macros) } else { Some(false) };
        match word {
            "if" => self.open(condition(rest)),
            "ifdef" => self.open(Some(macros.contains_key(rest) && outside)),
            "ifndef" => self.open(Some(!macros.contains_key(rest) && outside)),
            "elif" | "else" => {
                let Some(mut branch) = self.open.pop() else { return true };
                let outside = self.compiled();
                let condition = match (outside, word) {
                    (false, _) => Some(false),
                    (true, "else") => Some(true),
                    (true, _) => truth(rest, macros),
                };
                branch.kept = match branch.before {
                    Some(true) => Some(false),
                    Some(false) => condition,
                    None => condition.filter(|kept| !kept),
                };
                branch.before = or(branch.before, branch.kept);
                self.open.push(branch);
            }
            "endif" => {
                self.open.pop();
            }
            _ => return false,
        }
        true
    }

    fn open(&mut self, kept: Option<bool>) {
        self.open.push(Branch { kept, before: kept });
    }
}

/// Either of two truths, where `None` is not known.
fn or(a: Option<bool>, b: Option<bool>) -> Option<bool> {
    match (a, b) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (Some(false), Some(false)) => Some(false),
        _ => None,
    }
}

/// Whether the condition of an `#if` holds, where that can be worked out.
fn truth(condition: &str, macros: &HashMap<String, Option<Macro>>) -> Option<bool> {
    value(&defined(condition, macros), macros, 0).map(|value| value != 0)
}

/// The condition with each `defined(NAME)` and `defined NAME` replaced by 1 or 0.
fn defined(condition: &str, macros: &HashMap<String, Option<Macro>>) -> String {
    let cut = tokens(condition);
    let mut out = String::new();
    let mut at = 0;
    let mut index = 0;
    while index < cut.len() {
        let token = cut[index];
        if token.text == "defined" {
            let bracketed = cut.get(index + 1).is_some_and(|t| t.text == "(");
            let name = cut.get(index + 1 + usize::from(bracketed)).filter(|t| t.class == Class::Name);
            if let Some(name) = name {
                let end = index + 2 + 2 * usize::from(bracketed);
                out.push_str(&condition[at..token.start]);
                out.push_str(if macros.contains_key(name.text) { "1" } else { "0" });
                at = cut.get(end - 1).map_or(condition.len(), |t| t.end);
                index = end;
                continue;
            }
        }
        index += 1;
    }
    out.push_str(&condition[at..]);
    out
}

/// The whole number `text` is as the preprocessor reads it.
fn value(text: &str, macros: &HashMap<String, Option<Macro>>, depth: usize) -> Option<i64> {
    if depth > 16 {
        return None;
    }
    let cut = tokens(text);
    evaluate(&whole(&cut)?, macros, depth)
}

fn evaluate(node: &Node<'_>, macros: &HashMap<String, Option<Macro>>, depth: usize) -> Option<i64> {
    let at = |node: &Node<'_>| evaluate(node, macros, depth);
    Some(match &node.what {
        What::Number(number) => {
            let number = number.trim_end_matches(['u', 'U']);
            match number.strip_prefix("0x").or_else(|| number.strip_prefix("0X")) {
                Some(hex) => i64::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            }
        }
        // A name nothing defines is nought; one defined as something else is not known.
        What::Name(name) => match macros.get(*name) {
            None => 0,
            Some(Some(defined)) if defined.params.is_none() => value(&defined.body, macros, depth + 1)?,
            Some(_) => return None,
        },
        What::Group(inner) => at(inner)?,
        What::Prefix(op, operand) => {
            let operand = at(operand)?;
            match *op {
                "!" => i64::from(operand == 0),
                "-" => operand.checked_neg()?,
                "+" => operand,
                "~" => !operand,
                _ => return None,
            }
        }
        What::Binary(op, left, right) => {
            let (a, b) = (at(left)?, at(right)?);
            match *op {
                "*" => a.checked_mul(b)?,
                "/" => a.checked_div(b)?,
                "%" => a.checked_rem(b)?,
                "+" => a.checked_add(b)?,
                "-" => a.checked_sub(b)?,
                "<<" => a.checked_shl(u32::try_from(b).ok()?)?,
                ">>" => a.checked_shr(u32::try_from(b).ok()?)?,
                "<" => i64::from(a < b),
                ">" => i64::from(a > b),
                "<=" => i64::from(a <= b),
                ">=" => i64::from(a >= b),
                "==" => i64::from(a == b),
                "!=" => i64::from(a != b),
                "&" => a & b,
                "^" => a ^ b,
                "|" => a | b,
                "&&" => i64::from(a != 0 && b != 0),
                "||" => i64::from(a != 0 || b != 0),
                _ => return None,
            }
        }
        What::Ternary(condition, chosen, other) => {
            if at(condition)? != 0 {
                at(chosen)?
            } else {
                at(other)?
            }
        }
        _ => return None,
    })
}
