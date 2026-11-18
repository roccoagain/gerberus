use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Sexp {
    List(Vec<Sexp>),
    Atom(String),
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

pub fn parse(text: &str) -> Result<Sexp, ParseError> {
    let mut p = Parser {
        bytes: text.as_bytes(),
        pos: 0,
        line: 1,
        depth: 0,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos < p.bytes.len() {
        return Err(p.error("unexpected text after the closing parenthesis"));
    }
    Ok(value)
}

pub const MAX_DEPTH: usize = 256;

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    depth: usize,
}

impl Parser<'_> {
    fn skip_byte(&mut self, expected: u8) {
        debug_assert_eq!(self.bytes.get(self.pos), Some(&expected));
        self.pos += 1;
    }

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
        if self.depth >= MAX_DEPTH {
            return Err(self.error(&format!("lists are nested more than {MAX_DEPTH} deep")));
        }
        self.depth += 1;
        self.skip_byte(b'(');
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
                    self.depth -= 1;
                    return Ok(Sexp::List(items));
                }
                Some(_) => items.push(self.value()?),
            }
        }
    }

    fn string(&mut self) -> Result<Sexp, ParseError> {
        let open_line = self.line;
        self.skip_byte(b'"');
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
        Sexp::Atom(String::from_utf8_lossy(&self.bytes[start..self.pos]).into_owned())
    }
}

impl Sexp {
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

    pub fn text(&self) -> Option<&str> {
        match self {
            Sexp::Atom(s) | Sexp::Str(s) => Some(s),
            Sexp::List(_) => None,
        }
    }

    pub fn arg(&self, i: usize) -> Option<&str> {
        self.items().get(i).and_then(Sexp::text)
    }

    pub fn arg_f64(&self, i: usize) -> Option<f64> {
        self.arg(i).and_then(|s| s.parse().ok())
    }

    pub fn child(&self, name: &str) -> Option<&Sexp> {
        self.items().iter().find(|c| c.head() == Some(name))
    }

    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Sexp> + 'a {
        self.items().iter().filter(move |c| c.head() == Some(name))
    }

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
    fn rejects_deep_nesting_without_overflowing() {
        let deep = "(".repeat(1_000_000);
        let err = parse(&deep).unwrap_err();
        assert!(err.message.contains("nested"), "{err}");
        let ok = format!("{}{}", "(a ".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
        assert!(parse(&ok).is_ok());
    }

    #[test]
    fn keeps_utf8_in_strings_and_atoms() {
        let s = parse("(t \"±58 V\" µF)").unwrap();
        assert_eq!(s.arg(1), Some("±58 V"));
        assert_eq!(s.arg(2), Some("µF"));
    }
}
