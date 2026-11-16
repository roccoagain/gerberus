//! A small S-expression reader for KiCad files.
//!
//! KiCad files are a tree of lists. Each list starts with a bare word (its "head") and holds bare words,
//! numbers, quoted strings and more lists. This reader keeps every value as text and lets the caller
//! convert numbers, so no precision is lost on the way in.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Sexp {
    List(Vec<Sexp>),
    /// A bare word or number, such as `yes`, `passive` or `2.54`.
    Atom(String),
    /// A quoted string, with escapes already resolved.
    Str(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Parses one top-level expression. Trailing whitespace is allowed; anything else after it is an error.
pub fn parse(text: &str) -> Result<Sexp, ParseError> {
    let mut p = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        line: 1,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(p.error("unexpected text after the closing parenthesis"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        ParseError {
            line: self.line,
            message: message.to_string(),
        }
    }

    fn skip_ws(&mut self) {
        while let Some(&b) = self.bytes.get(self.pos) {
            if b == b'\n' {
                self.line += 1;
            }
            if !b.is_ascii_whitespace() {
                break;
            }
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Sexp, ParseError> {
        match self.bytes.get(self.pos) {
            None => Err(self.error("unexpected end of file")),
            Some(b'(') => self.list(),
            Some(b')') => Err(self.error("unexpected ')'")),
            Some(b'"') => self.string(),
            Some(_) => Ok(self.atom()),
        }
    }

    fn list(&mut self) -> Result<Sexp, ParseError> {
        let open_line = self.line;
        self.pos += 1; // '('
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            match self.bytes.get(self.pos) {
                None => {
                    return Err(ParseError {
                        line: open_line,
                        message: "list is never closed".to_string(),
                    });
                }
                Some(b')') => {
                    self.pos += 1;
                    return Ok(Sexp::List(items));
                }
                Some(_) => items.push(self.value()?),
            }
        }
    }

    fn string(&mut self) -> Result<Sexp, ParseError> {
        let open_line = self.line;
        self.pos += 1; // opening quote
        let mut out = Vec::new();
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(ParseError {
                    line: open_line,
                    message: "string is never closed".to_string(),
                });
            };
            self.pos += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.bytes.get(self.pos) else {
                        return Err(self.error("string ends inside an escape"));
                    };
                    self.pos += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        other => other,
                    });
                }
                b'\n' => {
                    self.line += 1;
                    out.push(b);
                }
                _ => out.push(b),
            }
        }
        String::from_utf8(out)
            .map(Sexp::Str)
            .map_err(|_| self.error("string is not valid UTF-8"))
    }

    fn atom(&mut self) -> Sexp {
        let start = self.pos;
        while let Some(&b) = self.bytes.get(self.pos) {
            if b.is_ascii_whitespace() || b == b'(' || b == b')' || b == b'"' {
                break;
            }
            self.pos += 1;
        }
        // The input is a &str and we only split on ASCII bytes, so this slice is valid UTF-8.
        Sexp::Atom(String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
    }
}

impl Sexp {
    /// The bare word at the start of a list, such as `symbol` in `(symbol ...)`.
    pub fn head(&self) -> Option<&str> {
        match self {
            Sexp::List(items) => match items.first() {
                Some(Sexp::Atom(a)) => Some(a),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn items(&self) -> &[Sexp] {
        match self {
            Sexp::List(items) => items,
            _ => &[],
        }
    }

    /// Text of an atom or string.
    pub fn text(&self) -> Option<&str> {
        match self {
            Sexp::Atom(s) | Sexp::Str(s) => Some(s),
            Sexp::List(_) => None,
        }
    }

    /// The text of item `i` (item 0 is the head).
    pub fn arg(&self, i: usize) -> Option<&str> {
        self.items().get(i).and_then(Sexp::text)
    }

    pub fn arg_f64(&self, i: usize) -> Option<f64> {
        self.arg(i).and_then(|s| s.parse().ok())
    }

    /// The first child list whose head is `name`.
    pub fn child(&self, name: &str) -> Option<&Sexp> {
        self.items().iter().find(|c| c.head() == Some(name))
    }

    /// Every child list whose head is `name`.
    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.items().iter().filter(move |c| c.head() == Some(name))
    }

    /// Reads a `(name yes|no)` flag, or a bare `name` atom used by older files.
    pub fn flag(&self, name: &str) -> Option<bool> {
        if let Some(c) = self.child(name) {
            return Some(c.arg(1).is_none_or(|v| v == "yes"));
        }
        if self.items().iter().any(|c| matches!(c, Sexp::Atom(a) if a == name)) {
            return Some(true);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_lists_and_strings() {
        let s = parse(r#"(a (b 1 2.5) "x \"y\" \\z" (c))"#).unwrap();
        assert_eq!(s.head(), Some("a"));
        assert_eq!(s.child("b").unwrap().arg_f64(2), Some(2.5));
        assert_eq!(s.arg(2), Some(r#"x "y" \z"#));
        assert!(s.child("c").is_some());
    }

    #[test]
    fn reads_flags_in_both_styles() {
        let s = parse("(pin (hide yes) (dnp no) locked)").unwrap();
        assert_eq!(s.flag("hide"), Some(true));
        assert_eq!(s.flag("dnp"), Some(false));
        assert_eq!(s.flag("locked"), Some(true));
        assert_eq!(s.flag("missing"), None);
    }

    #[test]
    fn reports_line_of_unclosed_list() {
        let err = parse("(a\n(b\n").unwrap_err();
        assert_eq!(err.line, 2);
    }

    #[test]
    fn keeps_utf8_in_strings_and_atoms() {
        let s = parse("(t \"±58 V\" µF)").unwrap();
        assert_eq!(s.arg(1), Some("±58 V"));
        assert_eq!(s.arg(2), Some("µF"));
    }
}
