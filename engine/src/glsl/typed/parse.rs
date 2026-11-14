//! Expressions read from tokens, with GLSL's precedence.

use super::lex::{Class, Token};

/// An expression, and the stretch of text it was read from.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Node<'a> {
    pub start: usize,
    pub end: usize,
    pub what: What<'a>,
}

/// What an expression is.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum What<'a> {
    Name(&'a str),
    Number(&'a str),
    /// `( inner )`.
    Group(Box<Node<'a>>),
    /// A function, a constructor or a macro called with its arguments.
    Call(&'a str, Vec<Node<'a>>),
    Index(Box<Node<'a>>, Box<Node<'a>>),
    /// `object.field`: a swizzle or a member of a struct.
    Member(Box<Node<'a>>, &'a str),
    Prefix(&'a str, Box<Node<'a>>),
    Postfix(&'a str, Box<Node<'a>>),
    Binary(&'a str, Box<Node<'a>>, Box<Node<'a>>),
    Ternary(Box<Node<'a>>, Box<Node<'a>>, Box<Node<'a>>),
    Assign(&'a str, Box<Node<'a>>, Box<Node<'a>>),
}

/// The binary operators by how tightly they bind, the loosest first.
const LEVELS: [&[&str]; 11] = [
    &["||"],
    &["^^"],
    &["&&"],
    &["|"],
    &["^"],
    &["&"],
    &["==", "!="],
    &["<", ">", "<=", ">="],
    &["<<", ">>"],
    &["+", "-"],
    &["*", "/", "%"],
];

const ASSIGNMENTS: [&str; 11] = ["=", "+=", "-=", "*=", "/=", "%=", "<<=", ">>=", "&=", "|=", "^="];

/// Reads expressions out of a run of tokens.
pub(super) struct Parser<'t, 'a> {
    tokens: &'t [Token<'a>],
    pub at: usize,
}

impl<'t, 'a> Parser<'t, 'a> {
    pub fn new(tokens: &'t [Token<'a>]) -> Self {
        Self { tokens, at: 0 }
    }

    /// The token at the reading place.
    pub fn peek(&self) -> Option<Token<'a>> {
        self.tokens.get(self.at).copied()
    }

    /// Whether the token at the reading place is the sign or word `text`.
    pub fn sees(&self, text: &str) -> bool {
        self.peek().is_some_and(|token| token.text == text && token.class != Class::Number)
    }

    /// Steps over `text` when it is next.
    pub fn take(&mut self, text: &str) -> bool {
        let seen = self.sees(text);
        if seen {
            self.at += 1;
        }
        seen
    }

    /// One expression without a comma at its top: an assignment at the loosest.
    pub fn expression(&mut self) -> Option<Node<'a>> {
        let target = self.ternary()?;
        let Some(op) = self.peek().filter(|t| t.class == Class::Sign && ASSIGNMENTS.contains(&t.text)) else {
            return Some(target);
        };
        self.at += 1;
        let value = self.expression()?;
        Some(Node {
            start: target.start,
            end: value.end,
            what: What::Assign(op.text, target.into(), value.into()),
        })
    }

    fn ternary(&mut self) -> Option<Node<'a>> {
        let condition = self.binary(0)?;
        if !self.take("?") {
            return Some(condition);
        }
        let chosen = self.expression()?;
        if !self.take(":") {
            return None;
        }
        // GLSL reads `a ? b : c = d` with the assignment outside; nobody writes it.
        let other = self.ternary()?;
        Some(Node {
            start: condition.start,
            end: other.end,
            what: What::Ternary(condition.into(), chosen.into(), other.into()),
        })
    }

    fn binary(&mut self, level: usize) -> Option<Node<'a>> {
        if level == LEVELS.len() {
            return self.prefix();
        }
        let mut left = self.binary(level + 1)?;
        while let Some(op) = self.peek().filter(|t| t.class == Class::Sign && LEVELS[level].contains(&t.text))
        {
            self.at += 1;
            let right = self.binary(level + 1)?;
            left = Node {
                start: left.start,
                end: right.end,
                what: What::Binary(op.text, left.into(), right.into()),
            };
        }
        Some(left)
    }

    fn prefix(&mut self) -> Option<Node<'a>> {
        let token = self.peek()?;
        if token.class == Class::Sign && ["-", "+", "!", "~", "++", "--"].contains(&token.text) {
            self.at += 1;
            let operand = self.prefix()?;
            return Some(Node {
                start: token.start,
                end: operand.end,
                what: What::Prefix(token.text, operand.into()),
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Option<Node<'a>> {
        let mut node = self.primary()?;
        loop {
            if self.take("[") {
                let index = self.expression()?;
                let close = self.peek().filter(|t| t.text == "]")?;
                self.at += 1;
                node =
                    Node { start: node.start, end: close.end, what: What::Index(node.into(), index.into()) };
            } else if self.take(".") {
                let field = self.peek().filter(|t| t.class == Class::Name)?;
                self.at += 1;
                // `.length()` is a method, which nothing here reads.
                if self.sees("(") {
                    return None;
                }
                node =
                    Node { start: node.start, end: field.end, what: What::Member(node.into(), field.text) };
            } else if let Some(op) =
                self.peek().filter(|t| t.class == Class::Sign && ["++", "--"].contains(&t.text))
            {
                self.at += 1;
                node = Node { start: node.start, end: op.end, what: What::Postfix(op.text, node.into()) };
            } else {
                return Some(node);
            }
        }
    }

    fn primary(&mut self) -> Option<Node<'a>> {
        let token = self.peek()?;
        self.at += 1;
        let (start, end) = (token.start, token.end);
        match token.class {
            Class::Number => Some(Node { start, end, what: What::Number(token.text) }),
            Class::Name if self.take("(") => {
                let mut arguments = Vec::new();
                if !self.sees(")") {
                    loop {
                        arguments.push(self.expression()?);
                        if !self.take(",") {
                            break;
                        }
                    }
                }
                let close = self.peek().filter(|t| t.text == ")")?;
                self.at += 1;
                Some(Node { start, end: close.end, what: What::Call(token.text, arguments) })
            }
            Class::Name => Some(Node { start, end, what: What::Name(token.text) }),
            Class::Sign if token.text == "(" => {
                let inner = self.expression()?;
                let close = self.peek().filter(|t| t.text == ")")?;
                self.at += 1;
                Some(Node { start, end: close.end, what: What::Group(inner.into()) })
            }
            _ => None,
        }
    }
}

/// The whole of `tokens` as one expression, or nothing when they are not one.
pub(super) fn whole<'a>(tokens: &[Token<'a>]) -> Option<Node<'a>> {
    let mut parser = Parser::new(tokens);
    let node = parser.expression()?;
    (parser.at == tokens.len()).then_some(node)
}

#[cfg(test)]
mod tests {
    use super::super::lex::tokens;
    use super::*;

    /// The tree written back with every operation in brackets.
    fn shape(node: &Node<'_>) -> String {
        match &node.what {
            What::Name(name) | What::Number(name) => (*name).to_owned(),
            What::Group(inner) => shape(inner),
            What::Call(name, arguments) => {
                format!("{name}({})", arguments.iter().map(shape).collect::<Vec<_>>().join(", "))
            }
            What::Index(object, index) => format!("{}[{}]", shape(object), shape(index)),
            What::Member(object, field) => format!("{}.{field}", shape(object)),
            What::Prefix(op, operand) => format!("({op}{})", shape(operand)),
            What::Postfix(op, operand) => format!("({}{op})", shape(operand)),
            What::Binary(op, left, right) | What::Assign(op, left, right) => {
                format!("({} {op} {})", shape(left), shape(right))
            }
            What::Ternary(condition, chosen, other) => {
                format!("({} ? {} : {})", shape(condition), shape(chosen), shape(other))
            }
        }
    }

    #[test]
    fn expressions_bind_as_glsl_binds_them() {
        let read = |text: &str| whole(&tokens(text)).map(|node| shape(&node));
        assert_eq!(
            read("a = b + c * -d.xy[1] < e && f ? g : h").as_deref(),
            Some("(a = ((((b + (c * (-d.xy[1]))) < e) && f) ? g : h))")
        );
        assert_eq!(read("x += f(1, (y - z)) / 2.0").as_deref(), Some("(x += (f(1, (y - z)) / 2.0))"));
        assert_eq!(read("i++ - --j").as_deref(), Some("((i++) - (--j))"));
        assert_eq!(read("a ? b : c ? d : e").as_deref(), Some("(a ? b : (c ? d : e))"));
        assert_eq!(read("a b"), None, "two expressions are not one");
        assert_eq!(read("v.length()"), None);
    }

    #[test]
    fn a_node_knows_where_it_stands_in_the_text() {
        let text = "  max(a ,b)  * c";
        let node = whole(&tokens(text)).expect("an expression");
        assert_eq!(&text[node.start..node.end], "max(a ,b)  * c");
        let What::Binary(_, left, _) = &node.what else { panic!("{node:?}") };
        assert_eq!(&text[left.start..left.end], "max(a ,b)");
    }
}
