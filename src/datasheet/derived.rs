//! Values a part file works out from others (`derived = "..."`): parsing the arithmetic.

/// One end of a `derived` range: a number worked out from printed ones, or a pin plus an offset as the page
/// names it (`VDDIOx + 4.0`).
#[derive(Debug, Clone, PartialEq)]
pub enum DerivedEnd {
    Number { value: f64, inputs: Vec<f64> },
    Pin { pin: String, plus: f64 },
}

/// A range's `derived` text, `<min> .. <max>; explanation`, with each end worked out.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub min: DerivedEnd,
    pub max: DerivedEnd,
    pub explanation: String,
}

/// Reads a `derived` text: `<min> .. <max>; explanation`. Each end is arithmetic on numbers (`+ - * /`,
/// brackets and `sqrt(...)`), or a pin name plus or minus a number as the page prints it.
///
/// # Errors
///
/// Says what is wrong when the text doesn't have that shape.
pub fn parse_derived(text: &str) -> Result<Derived, String> {
    let (ends, explanation) = text.split_once(';').ok_or("no `;` before the explanation")?;
    let (min, max) = ends.split_once("..").ok_or("no `..` between the two ends")?;
    let explanation = explanation.trim();
    if explanation.is_empty() {
        return Err("no explanation after the `;`".into());
    }
    Ok(Derived {
        min: parse_end(min)?,
        max: parse_end(max)?,
        explanation: explanation.to_string(),
    })
}

fn parse_end(text: &str) -> Result<DerivedEnd, String> {
    let text = text.trim();
    let first = text.chars().next().ok_or("an empty end")?;
    if first.is_ascii_alphabetic() && !text.starts_with("sqrt") {
        // A pin name can carry a sign of its own (`V+`, `V-`), so an offset after one is set off by spaces:
        // `V+ + 0.5`. Otherwise the first sign starts the offset: `VDD+0.3`.
        let spaced = [" + ", " - "]
            .iter()
            .filter_map(|op| text.rfind(op).map(|i| i + 1))
            .max();
        let mut split = spaced.unwrap_or_else(|| text.find(['+', '-']).unwrap_or(text.len()));
        // With no offset after it, a trailing sign is part of the name: `V+`.
        if spaced.is_none() && text[split..].trim_start_matches(['+', '-']).trim().is_empty() {
            split = text.len();
        }
        let pin = text[..split].trim();
        let signed_name = spaced.is_some() || split == text.len();
        let name_char =
            |c: char| c.is_ascii_alphanumeric() || c == '_' || (signed_name && matches!(c, '+' | '-' | '–'));
        if pin.is_empty() || !pin.chars().all(name_char) {
            return Err(format!("{text:?} isn't a pin plus a number"));
        }
        let rest = text[split..].replace(' ', "");
        let plus = if rest.is_empty() {
            0.0
        } else {
            rest.parse::<f64>()
                .map_err(|_| format!("{text:?} isn't a pin plus a number"))?
        };
        return Ok(DerivedEnd::Pin {
            pin: pin.to_string(),
            plus,
        });
    }
    let mut p = Arith {
        chars: text.chars().filter(|c| !c.is_whitespace()).collect(),
        at: 0,
        inputs: vec![],
    };
    let value = p.sum()?;
    if p.at != p.chars.len() {
        return Err(format!(
            "can't read {text:?} past {:?}",
            p.chars[p.at..].iter().collect::<String>()
        ));
    }
    Ok(DerivedEnd::Number {
        value,
        inputs: p.inputs,
    })
}

struct Arith {
    chars: Vec<char>,
    at: usize,
    /// The numbers the arithmetic starts from.
    inputs: Vec<f64>,
}

impl Arith {
    fn eat(&mut self, c: char) -> bool {
        if self.chars.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn sum(&mut self) -> Result<f64, String> {
        let mut v = self.product()?;
        loop {
            if self.eat('+') {
                v += self.product()?;
            } else if self.eat('-') {
                v -= self.product()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn product(&mut self) -> Result<f64, String> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v *= self.unary()?;
            } else if self.eat('/') {
                v /= self.unary()?;
            } else {
                return Ok(v);
            }
        }
    }

    fn unary(&mut self) -> Result<f64, String> {
        if self.eat('-') {
            return Ok(-self.unary()?);
        }
        if self.chars[self.at..].starts_with(&['s', 'q', 'r', 't', '(']) {
            self.at += 4;
            let v = self.unary()?;
            return if v < 0.0 {
                Err("sqrt of a negative number".into())
            } else {
                Ok(v.sqrt())
            };
        }
        if self.eat('(') {
            let v = self.sum()?;
            return if self.eat(')') {
                Ok(v)
            } else {
                Err("a `(` without its `)`".into())
            };
        }
        let start = self.at;
        while self.chars.get(self.at).is_some_and(|c| c.is_ascii_digit() || *c == '.') {
            self.at += 1;
        }
        let text: String = self.chars[start..self.at].iter().collect();
        let v = text.parse::<f64>().map_err(|_| {
            format!(
                "expected a number at {:?}",
                self.chars[start..].iter().collect::<String>()
            )
        })?;
        self.inputs.push(v);
        Ok(v)
    }
}

pub(super) const ARITHMETIC_CONSTANTS: [f64; 3] = [0.0, 1.0, 100.0];
