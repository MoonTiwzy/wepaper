//! The parser.

use std::fmt;

use crate::{Map, Value};

/// Arrays and objects nested deeper than this are refused instead of overflowing the stack.
const MAX_DEPTH: usize = 256;

/// Why a document did not parse, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The line of the offending character, from 1.
    pub line: usize,
    /// Its column in characters, from 1.
    pub column: usize,
    /// What was wrong.
    pub message: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at line {}, column {}", self.message, self.line, self.column)
    }
}

impl std::error::Error for Error {}

/// Parses a document.
///
/// Accepted beyond strict JSON: a byte order mark, `//` and `/* */` comments, trailing
/// commas, numbers written `+5`, `.5` or `5.`, and the non-numbers `nan`, `inf` and
/// `-nan(ind)`, which read as `0`.
pub fn parse(text: &str) -> Result<Value, Error> {
    let mut parser = Parser { text, bytes: text.as_bytes(), at: 0 };
    if text.starts_with('\u{feff}') {
        parser.at = 3;
    }
    parser.skip_blank();
    let value = parser.value(0)?;
    parser.skip_blank();
    if parser.at < parser.bytes.len() {
        return Err(parser.error("unexpected text after the document"));
    }
    Ok(value)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn error(&self, message: &'static str) -> Error {
        let before = &self.text[..self.at.min(self.text.len())];
        let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        Error { line, column, message }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_blank(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.at += 1,
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'/') => {
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.at += 1;
                    }
                }
                Some(b'/') if self.bytes.get(self.at + 1) == Some(&b'*') => {
                    self.at += 2;
                    while self.at < self.bytes.len() && !self.bytes[self.at..].starts_with(b"*/") {
                        self.at += 1;
                    }
                    self.at = (self.at + 2).min(self.bytes.len());
                }
                _ => return,
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        match self.peek() {
            None => Err(self.error("unexpected end of the document")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(_) => self.scalar(),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, Error> {
        self.at += 1;
        let mut map = Map::new();
        loop {
            self.skip_blank();
            match self.peek() {
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Value::Object(map));
                }
                Some(b'"') => {}
                Some(_) => return Err(self.error("expected a key in quotes or '}'")),
                None => return Err(self.error("unclosed object")),
            }
            let key = self.string()?;
            self.skip_blank();
            if self.peek() != Some(b':') {
                return Err(self.error("expected ':' after a key"));
            }
            self.at += 1;
            self.skip_blank();
            let value = self.value(depth + 1)?;
            map.insert(key, value);
            self.skip_blank();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {}
                Some(_) => return Err(self.error("expected ',' or '}'")),
                None => return Err(self.error("unclosed object")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, Error> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.skip_blank();
            match self.peek() {
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                None => return Err(self.error("unclosed array")),
                Some(_) => {}
            }
            items.push(self.value(depth + 1)?);
            self.skip_blank();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {}
                Some(_) => return Err(self.error("expected ',' or ']'")),
                None => return Err(self.error("unclosed array")),
            }
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let start = self.at;
            while !matches!(self.peek(), None | Some(b'"' | b'\\')) {
                self.at += 1;
            }
            // Both ends sit on ASCII bytes, so this is a character boundary.
            out.push_str(&self.text[start..self.at]);
            match self.peek() {
                None => return Err(self.error("unclosed string")),
                Some(b'"') => {
                    self.at += 1;
                    return Ok(out);
                }
                Some(_) => self.escape(&mut out)?,
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), Error> {
        self.at += 1;
        let Some(code) = self.peek() else { return Err(self.error("unclosed string")) };
        self.at += 1;
        out.push(match code {
            b'n' => '\n',
            b't' => '\t',
            b'r' => '\r',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'u' => {
                let high = self.hex4()?;
                // A surrogate pair is two escapes for one character.
                if (0xd800..0xdc00).contains(&high) && self.bytes[self.at..].starts_with(b"\\u") {
                    self.at += 2;
                    let low = self.hex4()?;
                    let scalar = 0x10000 + ((high - 0xd800) << 10) + (low.wrapping_sub(0xdc00) & 0x3ff);
                    char::from_u32(scalar).unwrap_or('\u{fffd}')
                } else {
                    char::from_u32(high).unwrap_or('\u{fffd}')
                }
            }
            // `\"`, `\\`, `\/`, and anything else a sloppy writer escaped.
            other if other.is_ascii() => other as char,
            _ => return Err(self.error("broken escape")),
        });
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let digits = self.text.get(self.at..self.at + 4).ok_or_else(|| self.error("broken \\u escape"))?;
        let value = u32::from_str_radix(digits, 16).map_err(|_| self.error("broken \\u escape"))?;
        self.at += 4;
        Ok(value)
    }

    /// A number, a literal, or one of the things written where a number was meant.
    fn scalar(&mut self) -> Result<Value, Error> {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'(' | b')' | b'#' | b'_'))
        {
            self.at += 1;
        }
        let token = &self.text[start..self.at];
        match token {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            "null" => return Ok(Value::Null),
            "" => return Err(self.error("unexpected character")),
            _ => {}
        }
        let lower = token.to_ascii_lowercase();
        let unsigned = lower.trim_start_matches(['+', '-']);
        if unsigned.starts_with("nan") || unsigned.starts_with("inf") || unsigned.contains("#ind") || unsigned.contains("#qnan") {
            return Ok(Value::Number(0.0));
        }
        match lower.trim_start_matches('+').parse::<f64>() {
            Ok(n) if n.is_finite() => Ok(Value::Number(n)),
            Ok(_) => Ok(Value::Number(0.0)),
            Err(_) => {
                self.at = start;
                Err(self.error("unexpected token"))
            }
        }
    }
}
