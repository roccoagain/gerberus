//! Checking a part file against its datasheet.
//!
//! A person or an LLM reads the datasheet and writes the part file. Nothing here reads a datasheet for
//! meaning; it only checks what someone else wrote, in two independent ways:
//!
//! 1. [`verify`] looks for every cited number on the page it cites, in the PDF's own text. It finds the
//!    table's MIN, TYP and MAX columns, so a typical value filed as a maximum, a value in the wrong unit, or
//!    a value from the wrong page is caught. It reads the row too: a value whose row doesn't name the pin is
//!    only plausible.
//! 2. [`blank`] writes a copy of the part file with every value replaced by `"?"`. A second reader who has
//!    not seen the first fills it in from the datasheet, and [`compare`] lists every value where the two
//!    disagree.
//!
//! Neither decides that a part file is right. A value passes when both readers agree and its number sits in
//! the right column of the cited page; anything else goes back to a careful reading of the datasheet.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::limits::{Bound, DriveKind, Part, Quantity, Range};

/// The base unit a value is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Volt,
    Amp,
    Watt,
    Celsius,
    /// Parts per million (per °C), for a resistor's TCR.
    Ppm,
    /// A fraction (0.01) that a datasheet prints as a percentage (1 %).
    Fraction,
}

impl Unit {
    fn of(q: Quantity) -> Unit {
        match q {
            Quantity::Voltage => Unit::Volt,
            Quantity::Current => Unit::Amp,
            Quantity::Power => Unit::Watt,
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Unit::Volt => "V",
            Unit::Amp => "A",
            Unit::Watt => "W",
            Unit::Celsius => "°C",
            Unit::Ppm => "ppm",
            Unit::Fraction => "",
        }
    }
}

/// Which end of a range a value is, and so which table column it should come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Min,
    Max,
    /// A single figure, such as a tolerance.
    Value,
}

/// What the part file says.
#[derive(Debug, Clone, PartialEq)]
pub enum Claimed {
    Number(f64),
    /// `V(pin) + plus`, which a datasheet prints as `VIN + 0.3`.
    Pin {
        pin: String,
        plus: f64,
    },
}

/// One value a part file cites from its datasheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    /// Where it is in the part file, such as `limit 1 (VIN to GND) abs_max max`.
    pub label: String,
    pub value: Claimed,
    pub role: Role,
    pub unit: Unit,
    /// 1-based page of the PDF.
    pub page: u32,
    /// Pin names the value's table row should mention, and the part number.
    pub context: Vec<String>,
    /// The range's `derived` text: how a value the page doesn't print was worked out.
    pub derived: Option<String>,
    /// The other end of the same range, when it is a number: a non-polar rating's minimum is the negative of
    /// its maximum, which the page needn't print.
    pub other_end: Option<f64>,
}

impl fmt::Display for Claimed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Claimed::Number(v) => write!(f, "{v}"),
            Claimed::Pin { pin, plus } if *plus == 0.0 => write!(f, "{pin}"),
            Claimed::Pin { pin, plus } => write!(f, "{pin} + {plus}"),
        }
    }
}

/// Every value in a part file that cites a datasheet page, in file order.
pub fn claims(part: &Part) -> Vec<Claim> {
    let mut out = Vec::new();
    // Pins named only by number ("1", "2") say nothing a table row would print.
    // The part number counts too: a TVS or a resistor series prints one row per part number.
    let named = |names: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = names
            .iter()
            .filter(|n| n.chars().any(char::is_alphabetic))
            .map(|n| (*n).to_string())
            .collect();
        v.sort();
        v.dedup();
        v.push(part.mpn.clone());
        v
    };
    if let Some(t) = &part.tolerance {
        out.push(Claim {
            label: "tolerance".into(),
            value: Claimed::Number(t.value),
            role: Role::Value,
            unit: Unit::Fraction,
            page: t.page,
            context: vec![],
            derived: None,
            other_end: None,
        });
    }
    if let Some(t) = &part.tcr {
        out.push(Claim {
            label: "tcr".into(),
            value: Claimed::Number(t.ppm),
            role: Role::Value,
            unit: Unit::Ppm,
            page: t.page,
            context: vec![],
            derived: None,
            other_end: None,
        });
    }
    if let Some(t) = &part.temperature {
        for (role, v, end) in [(Role::Min, t.min, "min"), (Role::Max, t.max, "max")] {
            out.push(Claim {
                label: format!("temperature {end}"),
                value: Claimed::Number(v),
                role,
                unit: Unit::Celsius,
                page: t.page,
                context: vec![],
                derived: None,
                other_end: None,
            });
        }
    }
    for (i, l) in part.limits.iter().enumerate() {
        let mut names: Vec<&str> = l.pins.iter().map(String::as_str).collect();
        let what = match &l.to {
            Some(to) => {
                names.push(to);
                format!("{} to {to}", l.pins.join(", "))
            }
            None => l.pins.join(", "),
        };
        let context = named(&names);
        let unit = Unit::of(l.quantity);
        for (label, r) in [
            ("abs_max", Some(&l.abs_max)),
            ("recommended", Some(&l.recommended)),
            ("breakdown", l.breakdown.as_ref()),
        ] {
            if let Some(r) = r {
                range_claims(
                    &format!("limit {} ({what}) {label}", i + 1),
                    r,
                    unit,
                    &context,
                    &mut out,
                );
            }
        }
    }
    for (i, d) in part.drives.iter().enumerate() {
        let kind = match d.kind {
            DriveKind::Regulates => "regulates",
            DriveKind::Forward => "forward",
            DriveKind::Switches => "switches",
        };
        let context = named(&[d.pin.as_str(), d.to.as_str()]);
        let label = format!("drive {} ({kind} {} to {})", i + 1, d.pin, d.to);
        range_claims(&label, &d.range, Unit::Volt, &context, &mut out);
        if let Some(h) = &d.headroom {
            if h.duty < 1.0 {
                out.push(Claim {
                    label: format!("{label} headroom duty"),
                    value: Claimed::Number(h.duty),
                    role: Role::Max,
                    unit: Unit::Fraction,
                    page: h.page,
                    context: vec![],
                    derived: None,
                    other_end: None,
                });
            }
            if h.volts > 0.0 {
                out.push(Claim {
                    label: format!("{label} headroom volts"),
                    value: Claimed::Number(h.volts),
                    role: Role::Value,
                    unit: Unit::Volt,
                    page: h.page,
                    context: vec![],
                    derived: None,
                    other_end: None,
                });
            }
        }
    }
    out
}

fn range_claims(label: &str, r: &Range, unit: Unit, context: &[String], out: &mut Vec<Claim>) {
    let Range::Known {
        min,
        max,
        page: Some(page),
        derived,
        ..
    } = r
    else {
        return;
    };
    let number = |b: &Bound| if let Bound::Value(v) = b { Some(*v) } else { None };
    for (role, b, end, other) in [
        (Role::Min, min, "min", number(max)),
        (Role::Max, max, "max", number(min)),
    ] {
        let value = match b {
            Bound::Value(v) => Claimed::Number(*v),
            Bound::Pin { pin, plus, at_most } => {
                if let Some(cap) = at_most {
                    out.push(Claim {
                        label: format!("{label} {end} cap"),
                        value: Claimed::Number(*cap),
                        role,
                        unit,
                        page: *page,
                        context: context.to_vec(),
                        derived: derived.clone(),
                        other_end: None,
                    });
                }
                Claimed::Pin {
                    pin: pin.clone(),
                    plus: *plus,
                }
            }
            Bound::Unknown(_) => continue,
        };
        out.push(Claim {
            label: format!("{label} {end}"),
            value,
            role,
            unit,
            page: *page,
            context: context.to_vec(),
            derived: derived.clone(),
            other_end: other,
        });
    }
}

/// How well the cited page backs a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// The page prints the value in the wrong column (a TYP filed as a MAX) or a different unit, and nowhere
    /// in the right one.
    Mismatch,
    /// The value isn't on the page at all.
    NotFound,
    /// The page has no text to search (a scanned image); someone must read it.
    NoText,
    /// The number is on the page, but its column, unit or row couldn't be confirmed. Read the evidence.
    Plausible,
    /// The page doesn't print the value, but it follows from what it does print: the `derived` text's numbers
    /// are on the page, or the value is zero or the negative of the other end (a non-polar rating). Check
    /// the arithmetic.
    Derived,
    /// In the right column, in the right unit, on a row that names the pin.
    Confirmed,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::Mismatch => "MISMATCH",
            Status::NotFound => "NOT FOUND",
            Status::NoText => "NO TEXT",
            Status::Plausible => "PLAUSIBLE",
            Status::Derived => "DERIVED",
            Status::Confirmed => "CONFIRMED",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub claim: Claim,
    pub status: Status,
    /// Why, in a few words.
    pub reason: String,
    /// The page's lines the value was found on, trimmed.
    pub evidence: Vec<String>,
}

impl Outcome {
    /// Whether this value needs fixing or a person: a mismatch or a value that isn't there.
    pub fn is_problem(&self) -> bool {
        matches!(self.status, Status::Mismatch | Status::NotFound)
    }
}

/// A table column, from a header line such as `PARAMETER  MIN  TYP  MAX  UNIT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Min,
    Typ,
    Max,
    /// A single-value column (`VALUE`, `RATINGS`).
    Value,
    Unit,
    /// Any other header word: parameter, symbol, conditions.
    Other,
}

impl fmt::Display for Col {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Col::Min => "MIN",
            Col::Typ => "TYP",
            Col::Max => "MAX",
            Col::Value => "VALUE",
            Col::Unit => "UNIT",
            Col::Other => "non-value",
        })
    }
}

/// A number printed on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Token {
    /// Character columns, start inclusive, end exclusive.
    start: usize,
    end: usize,
    value: f64,
    /// Printed with `±`.
    plus_minus: bool,
    /// Unit printed right after it, with its scale: `mV` is (Volt, 1e-3).
    unit: Option<(Unit, f64)>,
    /// Which end of a `min~max` cell it is (`20.00~22.10`), which settles its role whatever column it is in.
    cell: Option<Role>,
}

fn is_minus(c: char) -> bool {
    matches!(c, '-' | '–' | '−' | '‑' | '—' | '－')
}

/// Reads a unit right after a number (`mV`, ` V`, `%`, `°C`, `ppm`). `None` when there isn't one.
fn unit_at(chars: &[char], mut i: usize) -> Option<(Unit, f64, usize)> {
    if chars.get(i) == Some(&' ') {
        i += 1;
    }
    let rest: String = chars[i.min(chars.len())..].iter().take(8).collect();
    let (scale, after) = match rest.chars().next()? {
        'm' if !rest.starts_with("mm") && !rest.starts_with("min") => (1e-3, &rest[1..]),
        'µ' | 'μ' => (1e-6, &rest['µ'.len_utf8()..]),
        'u' => (1e-6, &rest[1..]),
        'n' => (1e-9, &rest[1..]),
        'k' | 'K' => (1e3, &rest[1..]),
        _ => (1.0, rest.as_str()),
    };
    let boundary = |s: &str, n: usize| s[n..].chars().next().is_none_or(|c| !c.is_alphanumeric());
    let (unit, len) = if after.starts_with("ppm") {
        (Unit::Ppm, 3)
    } else if after.starts_with('V') && boundary(after, 1) {
        (Unit::Volt, 1)
    } else if after.starts_with('A') && boundary(after, 1) {
        (Unit::Amp, 1)
    } else if after.starts_with('W') && boundary(after, 1) {
        (Unit::Watt, 1)
    } else if after.starts_with("°C") || after.starts_with('℃') || after.starts_with("ºC") {
        (Unit::Celsius, 1)
    } else if after.starts_with(['%', '％']) {
        (Unit::Fraction, 1)
    } else {
        return None;
    };
    // A scale prefix only goes with V, A and W.
    if scale != 1.0 && !matches!(unit, Unit::Volt | Unit::Amp | Unit::Watt) {
        return None;
    }
    let scale = if unit == Unit::Fraction { 0.01 } else { scale };
    Some((unit, scale, i + len))
}

/// Whether the text ends with a ground pin's name, so `VSS - 0.3` reads as -0.3 V.
fn ends_with_ground(chars: &[char]) -> bool {
    let text: String = chars.iter().collect();
    let word = text
        .trim_end()
        .rsplit(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or("");
    matches!(
        word.to_ascii_uppercase().as_str(),
        "VSS" | "GND" | "VSSA" | "AGND" | "PGND" | "DGND" | "COM"
    )
}

/// The numbers on a line, with where they sit. Digits that are part of a name (`SOT-23`, `C5446`, `VDD2`)
/// are left out.
fn tokens(line: &str) -> Vec<Token> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let starts_number = c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit));
        if !starts_number {
            i += 1;
            continue;
        }
        let glued_name =
            i > 1 && chars[i - 1] == '-' && chars[i - 2].is_ascii_alphabetic() && !ends_with_ground(&chars[..i - 1]);
        if glued_name || (i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j].is_ascii_digit() {
            j += 1;
        }
        if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
            j += 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
        }
        let mut text: String = chars[i..j].iter().collect();
        // 1/16 W
        if chars.get(j) == Some(&'/') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
            let mut k = j + 1;
            while k < chars.len() && chars[k].is_ascii_digit() {
                k += 1;
            }
            let den: String = chars[j + 1..k].iter().collect();
            if let (Ok(n), Ok(d)) = (text.parse::<f64>(), den.parse::<f64>())
                && d != 0.0
            {
                text = (n / d).to_string();
                j = k;
            }
        }
        // A digit run glued to letters that aren't a unit is a name (0402WGF, 25C08).
        let unit = unit_at(&chars, j);
        if unit.is_none() && chars.get(j).is_some_and(|c| c.is_alphabetic()) {
            let glued = !matches!(chars.get(j), Some('x' | 'X'));
            if glued && j > i {
                i = j;
                continue;
            }
        }
        let Ok(mut value) = text.parse::<f64>() else {
            i = j;
            continue;
        };
        // Look back for a sign: `-0.3`, `– 0.3`, `±2000`, `± 15`.
        let mut start = i;
        let mut plus_minus = false;
        let mut k = i;
        // A sign set apart by one space: `± 15`, or `- 0.3` in a table cell (but not `VDD - 0.3`, which the
        // space before the minus would make a cell of its own only when the gap is wide).
        if k > 2 && chars[k - 3..k] == ['+', '/', '-'] {
            plus_minus = true;
            start = k - 3;
            k = 0;
        } else if (k > 1 && chars[k - 1] == ' ' && matches!(chars[k - 2], '±' | '+'))
            || (k > 3
                && chars[k - 1] == ' '
                && is_minus(chars[k - 2])
                && ((chars[k - 3] == ' ' && chars[k - 4] == ' ' && chars[..k - 2].iter().any(|c| !c.is_whitespace()))
                    || ends_with_ground(&chars[..k - 2])))
        {
            k -= 1;
        }
        if k > 0 {
            let s = chars[k - 1];
            let before_ok = k < 2 || !chars[k - 2].is_alphanumeric();
            if s == '±' {
                plus_minus = true;
                start = k - 1;
            } else if is_minus(s) && (before_ok || ends_with_ground(&chars[..k - 1])) {
                value = -value;
                start = k - 1;
            } else if matches!(s, '+' | '＋') && before_ok {
                start = k - 1;
            }
        }
        let end = unit.map_or(j, |(_, _, e)| e);
        let after = chars[end.max(j)..].iter().find(|c| **c != ' ');
        let before = chars[..start].iter().rev().find(|c| **c != ' ');
        let cell = if matches!(after, Some('~' | '～')) {
            Some(Role::Min)
        } else if matches!(before, Some('~' | '～')) {
            Some(Role::Max)
        } else {
            None
        };
        out.push(Token {
            start,
            end: end.max(j),
            value,
            plus_minus,
            unit: unit.map(|(u, s, _)| (u, s)),
            cell,
        });
        i = end.max(j);
    }
    out
}

/// The words of a line with their character columns.
fn words(line: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    let chars: Vec<char> = line.chars().collect();
    for (i, c) in chars.iter().chain(std::iter::once(&' ')).enumerate() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                out.push((s, i, chars[s..i].iter().collect()));
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// The columns of a table header line, by their centre: `None` when the line isn't a header. A header word
/// that is one of `own` (the part number, in a table with a column per part) is that part's value column.
fn header(line: &str, own: &[String]) -> Option<Vec<(f64, Col)>> {
    let mut cols = Vec::new();
    let mut values = 0;
    for (s, e, w) in words(line) {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_uppercase();
        let col = match w.as_str() {
            _ if !w.is_empty() && own.iter().any(|o| o.eq_ignore_ascii_case(&w)) => Col::Value,
            "MIN" | "MINIMUM" => Col::Min,
            "TYP" | "TYPICAL" | "NOM" => Col::Typ,
            "MAX" | "MAXIMUM" => Col::Max,
            "VALUE" | "VALUES" | "RATING" | "RATINGS" | "LIMIT" | "LIMITS" | "SPEC" => Col::Value,
            "UNIT" | "UNITS" => Col::Unit,
            "" => continue,
            _ => Col::Other,
        };
        if matches!(col, Col::Min | Col::Typ | Col::Max | Col::Value) {
            values += 1;
        }
        cols.push(((s + e) as f64 / 2.0, col));
    }
    let ok = values >= 2 || (values == 1 && cols.iter().any(|(_, c)| *c == Col::Unit));
    // A header is mostly header words; a sentence that happens to contain "max" is not one.
    let others = cols.iter().filter(|(_, c)| *c == Col::Other).count();
    (ok && others <= 10).then_some(cols)
}

/// The column a span of characters sits under, from the nearest header line above it.
fn column(lines: &[&str], at: usize, start: usize, end: usize, own: &[String]) -> Option<(Col, usize)> {
    let centre = (start + end) as f64 / 2.0;
    (0..=at).rev().take(60).find_map(|i| {
        let cols = header(lines[i], own)?;
        let (_, col) = cols
            .iter()
            .min_by(|a, b| (a.0 - centre).abs().total_cmp(&(b.0 - centre).abs()))?;
        Some((*col, i))
    })
}

/// The unit a number without one of its own takes: the nearest unit in the table's UNIT column, on its line
/// or the few lines around it (merged cells print the unit once, beside the middle row).
fn unit_column(lines: &[&str], at: usize, header_line: usize, own: &[String]) -> Option<((Unit, f64), bool)> {
    let cols = header(lines[header_line], own)?;
    let (unit_centre, _) = cols.iter().find(|(_, c)| *c == Col::Unit)?;
    let mut order = vec![at];
    for d in 1..=4 {
        order.push(at + d);
        if at >= d && at - d > header_line {
            order.push(at - d);
        }
    }
    order.into_iter().filter(|&i| i < lines.len()).find_map(|i| {
        let same_line = i == at;
        let chars: Vec<char> = lines[i].chars().collect();
        words(lines[i])
            .into_iter()
            .filter(|(s, e, _)| ((*s + *e) as f64 / 2.0 - unit_centre).abs() < 8.0)
            .find_map(|(s, _, w)| {
                if w.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    return None;
                }
                // Read the unit as if a number stood just before it.
                let mut probe = vec!['1'];
                probe.extend(&chars[s..]);
                unit_at(&probe, 1).map(|(u, sc, _)| ((u, sc), same_line))
            })
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9_f64.max(1e-6 * a.abs().max(b.abs()))
}

/// Whether `name` appears in `text` as a word of its own (case-insensitive): `VIN` in `VIN to GND`, but not in
/// `VINT`.
fn mentions(text: &str, name: &str) -> bool {
    let t = text.to_ascii_uppercase();
    let n = name.to_ascii_uppercase();
    if n.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(pos) = t[from..].find(&n) {
        let s = from + pos;
        let e = s + n.len();
        let before = t[..s].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric());
        let after = t[e..].chars().next().is_none_or(|c| !c.is_ascii_alphanumeric());
        if before && after {
            return true;
        }
        from = s + n.len().max(1);
    }
    false
}

/// Checks one claim against the text of its page, as `pdftotext -layout` prints it.
pub fn check_claim(claim: &Claim, page: &str) -> Outcome {
    let lines: Vec<&str> = page.lines().collect();
    if page.split_whitespace().count() < 5 {
        return Outcome {
            claim: claim.clone(),
            status: Status::NoText,
            reason: "the page has no text layer; read it by eye".into(),
            evidence: vec![],
        };
    }
    // (line, status, score, reason) for each place the value turns up. Among hits of one status, a higher
    // score met more of the checks.
    let mut hits: Vec<(usize, Status, u8, String)> = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        match &claim.value {
            Claimed::Number(want) => {
                for t in tokens(line) {
                    if let Some((st, score, why)) = judge_number(claim, *want, &t, &lines, n) {
                        hits.push((n, st, score, why));
                    }
                }
            }
            Claimed::Pin { pin, plus } => {
                if let Some((st, why)) = judge_relative(claim, pin, *plus, &lines, n) {
                    hits.push((n, st, 0, why));
                }
            }
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    let best = hits.first().map(|h| h.1);
    let Some(best) = best else {
        if let Some(o) = derivation(claim, page) {
            return o;
        }
        return Outcome {
            claim: claim.clone(),
            status: Status::NotFound,
            reason: format!("{} {} isn't on page {}", claim.value, claim.unit.symbol(), claim.page),
            evidence: vec![],
        };
    };
    // A worked-out value that happens to be printed elsewhere on the page in another unit is still worked out.
    if best == Status::Mismatch
        && claim.derived.is_some()
        && let Some(o) = derivation(claim, page)
    {
        return o;
    }
    let chosen: Vec<&(usize, Status, u8, String)> = hits.iter().filter(|h| h.1 == best).take(3).collect();
    Outcome {
        claim: claim.clone(),
        status: best,
        reason: chosen[0].3.clone(),
        evidence: chosen.iter().map(|h| lines[h.0].trim().to_string()).collect(),
    }
}

/// A value the page doesn't print but that follows from it.
fn derivation(claim: &Claim, page: &str) -> Option<Outcome> {
    let outcome = |reason: String| Outcome {
        claim: claim.clone(),
        status: Status::Derived,
        reason,
        evidence: vec![],
    };
    if let Some(text) = &claim.derived {
        let printed: Vec<f64> = page.lines().flat_map(tokens).map(|t| t.value).collect();
        let own = match claim.value {
            Claimed::Number(v) => Some(v.abs()),
            Claimed::Pin { .. } => None,
        };
        // 0, 1 and 100 are arithmetic (1 − 2 %, 100 %), and the value itself is the result, not an input.
        let inputs: Vec<f64> = tokens(text)
            .iter()
            .map(|t| t.value.abs())
            .filter(|v| ![0.0, 1.0, 100.0].contains(v) && own.is_none_or(|o| !close(o, *v)))
            .collect();
        let (here, elsewhere): (Vec<f64>, Vec<f64>) =
            inputs.iter().partition(|v| printed.iter().any(|p| close(p.abs(), **v)));
        // Some inputs come from another page or the part number (a resistor's value), so one printed input is
        // enough to tie the derivation to this page; the rest are listed for whoever checks the arithmetic.
        let list = |v: &[f64]| v.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        return Some(if here.is_empty() && !inputs.is_empty() {
            Outcome {
                claim: claim.clone(),
                status: Status::NotFound,
                reason: format!("derived from {text:?}, but none of {} is on the page", list(&inputs)),
                evidence: vec![],
            }
        } else if elsewhere.is_empty() {
            outcome(format!("derived: {text}"))
        } else {
            outcome(format!("derived: {text} ({} not on this page)", list(&elsewhere)))
        });
    }
    let Claimed::Number(want) = claim.value else {
        return None;
    };
    if want == 0.0 {
        return Some(outcome("zero, the bottom of a rating that only gives a maximum".into()));
    }
    if claim.role == Role::Min && claim.other_end.is_some_and(|o| close(o, -want)) {
        return Some(outcome(
            "the negative of the maximum: the rating applies in both polarities".into(),
        ));
    }
    None
}

/// Whether the row around line `n` names one of the claim's pins: its own line or two either side, the
/// way a table's merged cells print a pin name once beside several lines.
fn row_mentions(claim: &Claim, lines: &[&str], n: usize) -> bool {
    let lo = n.saturating_sub(2);
    let hi = (n + 2).min(lines.len().saturating_sub(1));
    lines[lo..=hi]
        .iter()
        .any(|l| claim.context.iter().any(|p| mentions(l, p)))
}

fn role_fits(role: Role, col: Col, plus_minus: bool) -> bool {
    match (role, col) {
        // A one-figure rating (a tolerance, a duty cycle) is often printed as the only number on its row.
        (_, Col::Value)
        | (Role::Min, Col::Min)
        | (Role::Max, Col::Max)
        | (Role::Value, Col::Min | Col::Max | Col::Typ) => true,
        // ±15 kV in a MAX column bounds both ends.
        (Role::Min, Col::Max) => plus_minus,
        _ => false,
    }
}

fn judge_number(claim: &Claim, want: f64, t: &Token, lines: &[&str], n: usize) -> Option<(Status, u8, String)> {
    // Every reading of the number that could be the claimed value, with its unit and how sure that is.
    let own = &claim.context[claim.context.len().saturating_sub(1)..];
    let col = column(lines, n, t.start, t.end, own);
    // The unit printed right after the number, or in the UNIT column of its own line, is certain; one from a
    // line nearby is only likely, since merged cells and neighbouring tables can put another unit near it.
    let unit = t
        .unit
        .map(|u| (u, true))
        .or_else(|| col.and_then(|(_, h)| unit_column(lines, n, h, own)));
    let (value_matches, unit_known, unit_ok, unit_printed) = if let Some(((u, scale), printed)) = unit {
        let v = t.value * scale;
        let matches = close(v, want) || (t.plus_minus && close(-v, want));
        (matches, true, u == claim.unit, printed)
    } else {
        let matches = close(t.value, want)
            || (t.plus_minus && close(-t.value, want))
            || (claim.unit == Unit::Fraction && close(t.value / 100.0, want));
        (matches, false, true, false)
    };
    if !value_matches {
        return None;
    }
    let row = claim.context.is_empty() || row_mentions(claim, lines, n);
    let mut doubts = Vec::new();
    let col = match t.cell {
        // A `min~max` cell says which end it is, whatever the header above says.
        Some(end) if end == claim.role || claim.role == Role::Value => Some((Col::Value, n)),
        Some(Role::Min) => Some((Col::Min, n)),
        Some(_) => Some((Col::Max, n)),
        None => col,
    };
    match col {
        // A typical value filed as a limit is the mistake that matters most.
        Some((Col::Typ, _)) if claim.role != Role::Value => {
            return Some((
                Status::Mismatch,
                0,
                format!(
                    "found only in the TYP column, but the part file uses it as {}",
                    match claim.role {
                        Role::Min => "a minimum",
                        Role::Max => "a maximum",
                        Role::Value => "a rating",
                    }
                ),
            ));
        }
        Some((c, _)) if !role_fits(claim.role, c, t.plus_minus) => {
            doubts.push(format!("it sits under a {c} heading"));
        }
        Some(_) => {}
        None => doubts.push("no MIN/TYP/MAX header above it".to_string()),
    }
    let other_unit = unit.map_or("?", |((u, _), _)| u.symbol());
    if !unit_ok && unit_printed {
        return Some((Status::Mismatch, 0, format!("the number is there, but in {other_unit}")));
    }
    if !unit_ok {
        doubts.push(format!("the UNIT column near it says {other_unit}"));
    }
    if !unit_known && claim.unit != Unit::Fraction {
        doubts.push("no unit printed beside it".into());
    }
    if !row {
        doubts.push(format!("its row doesn't name {}", claim.context.join(" or ")));
    }
    let score = 4u8.saturating_sub(doubts.len() as u8);
    if doubts.is_empty() {
        let where_ = col.map_or(String::new(), |(c, _)| format!(" in the {c} column"));
        Some((Status::Confirmed, score, format!("found{where_}")))
    } else {
        Some((Status::Plausible, score, format!("found, but {}", doubts.join("; "))))
    }
}

/// Whether a relative bound is the low end of a `min ~ max` cell: `VIN + 0.3 ~ ...`.
fn trimmed_after_cell(rest: &str) -> bool {
    let r = rest.trim_start();
    let r = r.strip_prefix('+').map_or(r, |x| {
        x.trim_start()
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
    });
    r.trim_start().starts_with(['~', '～'])
}

fn judge_relative(claim: &Claim, pin: &str, plus: f64, lines: &[&str], n: usize) -> Option<(Status, String)> {
    // A pin named `VDD/VDDA` is printed as either.
    pin.split('/')
        .filter(|p| !p.is_empty())
        .find_map(|p| judge_relative_one(claim, p.trim(), plus, lines, n))
}

fn judge_relative_one(claim: &Claim, pin: &str, plus: f64, lines: &[&str], n: usize) -> Option<(Status, String)> {
    let line = lines[n];
    let upper = line.to_ascii_uppercase();
    let pin_up = pin.to_ascii_uppercase();
    let mut from = 0;
    while let Some(pos) = upper[from..].find(&pin_up) {
        let s = from + pos;
        from = s + pin_up.len();
        if !mentions(&line[s..], pin) {
            continue;
        }
        let rest = &line[from..];
        let trimmed = rest.trim_start();
        let found_plus = if plus == 0.0 {
            !trimmed.starts_with('+')
        } else if let Some(r) = trimmed.strip_prefix('+') {
            tokens(r.trim_start())
                .first()
                .is_some_and(|t| close(t.value * t.unit.map_or(1.0, |u| u.1), plus))
        } else {
            false
        };
        if !found_plus {
            continue;
        }
        let start = line[..s].chars().count();
        let end = start + pin.chars().count();
        let own = &claim.context[claim.context.len().saturating_sub(1)..];
        let before = line[..s].trim_end();
        let col = if before.ends_with(['~', '～']) {
            Some((if claim.role == Role::Max { Col::Max } else { Col::Min }, n))
        } else if trimmed_after_cell(&line[from..]) {
            Some((if claim.role == Role::Min { Col::Min } else { Col::Max }, n))
        } else {
            column(lines, n, start, end, own)
        };
        return match col {
            Some((c, _)) if role_fits(claim.role, c, false) => {
                Some((Status::Confirmed, format!("found in the {c} column")))
            }
            Some((c, _)) => Some((Status::Mismatch, format!("found only in the {c} column"))),
            None => Some((Status::Plausible, "found, but no MIN/TYP/MAX header above it".into())),
        };
    }
    None
}

/// The text of one page of a PDF, laid out as printed, from poppler's `pdftotext`.
pub fn page_text(pdf: &Path, page: u32) -> Result<String, String> {
    let out = std::process::Command::new("pdftotext")
        .args([
            "-layout",
            "-enc",
            "UTF-8",
            "-f",
            &page.to_string(),
            "-l",
            &page.to_string(),
        ])
        .arg(pdf)
        .arg("-")
        .output()
        .map_err(|e| format!("can't run pdftotext (install poppler-utils): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pdftotext failed on {} page {page}: {}",
            pdf.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Checks every cited value of a part file against the PDF. `pages` gives each page's text; tests pass
/// their own, [`verify`] reads the PDF.
pub fn verify_with(part: &Part, mut pages: impl FnMut(u32) -> Result<String, String>) -> Vec<Outcome> {
    let mut cache: BTreeMap<u32, Result<String, String>> = BTreeMap::new();
    claims(part)
        .into_iter()
        .map(|c| {
            let text = cache.entry(c.page).or_insert_with(|| pages(c.page));
            match text {
                Ok(t) => check_claim(&c, t),
                Err(e) => Outcome {
                    reason: e.clone(),
                    claim: c,
                    status: Status::NotFound,
                    evidence: vec![],
                },
            }
        })
        .collect()
}

/// Checks every cited value of a part file against its datasheet PDF.
pub fn verify(part: &Part, pdf: &Path) -> Vec<Outcome> {
    verify_with(part, |p| page_text(pdf, p))
}

/// A report of [`verify`]'s outcomes, problems first.
pub fn render(mpn: &str, outcomes: &[Outcome], all: bool) -> String {
    let mut s = String::new();
    let mut sorted: Vec<&Outcome> = outcomes.iter().collect();
    sorted.sort_by_key(|o| o.status);
    for o in sorted {
        if !all && o.status == Status::Confirmed {
            continue;
        }
        let _ = writeln!(
            s,
            "{} {mpn} {}: {} {} (page {}): {}",
            o.status,
            o.claim.label,
            o.claim.value,
            o.claim.unit.symbol(),
            o.claim.page,
            o.reason
        );
        for e in &o.evidence {
            let _ = writeln!(s, "    | {e}");
        }
    }
    let count = |st: Status| outcomes.iter().filter(|o| o.status == st).count();
    let _ = writeln!(
        s,
        "{mpn}: {} values: {} confirmed, {} derived, {} plausible, {} with no text, {} not found, {} mismatched",
        outcomes.len(),
        count(Status::Confirmed),
        count(Status::Derived),
        count(Status::Plausible),
        count(Status::NoText),
        count(Status::NotFound),
        count(Status::Mismatch)
    );
    s
}

/// The keys of a part file whose values a second reader fills in.
const VALUE_KEYS: [&str; 8] = [
    "abs_max",
    "recommended",
    "breakdown",
    "range",
    "headroom",
    "tolerance",
    "tcr",
    "temperature",
];

/// A copy of a part file's TOML with every value from the datasheet replaced by `"?"`, for a second reader
/// to fill in without seeing the first reader's numbers. Pins, limit groups and drive kinds stay, so both
/// readers answer the same questions.
pub fn blank(toml_text: &str) -> Result<String, String> {
    let mut doc: toml::Table = toml::from_str(toml_text).map_err(|e| e.to_string())?;
    fn clear(t: &mut toml::Table) {
        for (k, v) in t.iter_mut() {
            if VALUE_KEYS.contains(&k.as_str()) {
                *v = toml::Value::String("?".into());
            } else if let toml::Value::Array(a) = v {
                for item in a {
                    if let toml::Value::Table(t) = item {
                        clear(t);
                    }
                }
            }
        }
    }
    clear(&mut doc);
    let mut out = String::from(
        "# Second reading: replace every \"?\" with what the datasheet says, in the format of parts/README.md.\n\
         # Write { unknown = \"reason\" } where it gives nothing. Don't look at the first reading.\n",
    );
    out.push_str(&toml::to_string(&doc).map_err(|e| e.to_string())?);
    Ok(out)
}

/// One place where two readings of a datasheet disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub at: String,
    pub first: String,
    pub second: String,
}

fn show_bound(b: &Bound) -> String {
    match b {
        Bound::Value(v) => format!("{v}"),
        Bound::Pin { pin, plus, at_most } => {
            let mut s = pin.clone();
            if *plus != 0.0 {
                let _ = write!(s, " + {plus}");
            }
            if let Some(c) = at_most {
                let _ = write!(s, " (at most {c})");
            }
            s
        }
        Bound::Unknown(_) => "unknown".into(),
    }
}

fn same_bound(a: &Bound, b: &Bound) -> bool {
    match (a, b) {
        (Bound::Value(x), Bound::Value(y)) => close(*x, *y),
        (
            Bound::Pin {
                pin: p1,
                plus: x1,
                at_most: c1,
            },
            Bound::Pin {
                pin: p2,
                plus: x2,
                at_most: c2,
            },
        ) => {
            p1 == p2
                && close(*x1, *x2)
                && match (c1, c2) {
                    (Some(a), Some(b)) => close(*a, *b),
                    (None, None) => true,
                    _ => false,
                }
        }
        (Bound::Unknown(_), Bound::Unknown(_)) => true,
        _ => false,
    }
}

fn compare_range(at: &str, a: Option<&Range>, b: Option<&Range>, out: &mut Vec<Difference>) {
    let show = |r: Option<&Range>| match r {
        None => "absent".to_string(),
        Some(Range::Unknown(_)) => "unknown".to_string(),
        Some(Range::Known { min, max, page, .. }) => format!(
            "{} .. {} (page {})",
            show_bound(min),
            show_bound(max),
            page.map_or("?".into(), |p| p.to_string())
        ),
    };
    // A range whose ends are both unknown says the same as an unknown range.
    let blank = |r: Option<&Range>| match r {
        Some(Range::Known { min, max, .. }) if !min.is_known() && !max.is_known() => {
            Some(Range::Unknown(String::new()))
        }
        other => other.cloned(),
    };
    let (a2, b2) = (blank(a), blank(b));
    let same = match (a2.as_ref(), b2.as_ref()) {
        (None, None) | (Some(Range::Unknown(_)), Some(Range::Unknown(_))) => true,
        (
            Some(Range::Known {
                min: m1,
                max: x1,
                page: p1,
                ..
            }),
            Some(Range::Known {
                min: m2,
                max: x2,
                page: p2,
                ..
            }),
        ) => {
            // A page only matters when it backs a number.
            let numbers = m1.is_known() || x1.is_known();
            same_bound(m1, m2) && same_bound(x1, x2) && (p1 == p2 || !numbers)
        }
        _ => false,
    };
    if !same {
        out.push(Difference {
            at: at.to_string(),
            first: show(a),
            second: show(b),
        });
    }
}

/// Every value where two readings of the same datasheet disagree: the numbers, which ends are unknown, and
/// the pages. Limits and drives are matched by position, so both readings must come from the same
/// [`blank`] file.
pub fn compare(first: &Part, second: &Part) -> Vec<Difference> {
    let mut out = Vec::new();
    let opt = |label: &str, a: Option<String>, b: Option<String>, out: &mut Vec<Difference>| {
        if a != b {
            out.push(Difference {
                at: label.to_string(),
                first: a.unwrap_or_else(|| "absent".into()),
                second: b.unwrap_or_else(|| "absent".into()),
            });
        }
    };
    opt(
        "tolerance",
        first
            .tolerance
            .as_ref()
            .map(|t| format!("{} (page {})", t.value, t.page)),
        second
            .tolerance
            .as_ref()
            .map(|t| format!("{} (page {})", t.value, t.page)),
        &mut out,
    );
    opt(
        "tcr",
        first.tcr.as_ref().map(|t| format!("{} ppm (page {})", t.ppm, t.page)),
        second.tcr.as_ref().map(|t| format!("{} ppm (page {})", t.ppm, t.page)),
        &mut out,
    );
    opt(
        "temperature",
        first
            .temperature
            .as_ref()
            .map(|t| format!("{} .. {} °C (page {})", t.min, t.max, t.page)),
        second
            .temperature
            .as_ref()
            .map(|t| format!("{} .. {} °C (page {})", t.min, t.max, t.page)),
        &mut out,
    );
    if first.limits.len() != second.limits.len() {
        out.push(Difference {
            at: "limits".into(),
            first: format!("{} limits", first.limits.len()),
            second: format!("{} limits", second.limits.len()),
        });
    }
    for (i, (a, b)) in first.limits.iter().zip(&second.limits).enumerate() {
        let at = format!("limit {} ({})", i + 1, a.pins.join(", "));
        if a.pins != b.pins || a.to != b.to || a.quantity != b.quantity {
            out.push(Difference {
                at: at.clone(),
                first: format!("{:?} to {:?}", a.pins, a.to),
                second: format!("{:?} to {:?}", b.pins, b.to),
            });
            continue;
        }
        compare_range(&format!("{at} abs_max"), Some(&a.abs_max), Some(&b.abs_max), &mut out);
        compare_range(
            &format!("{at} recommended"),
            Some(&a.recommended),
            Some(&b.recommended),
            &mut out,
        );
        compare_range(
            &format!("{at} breakdown"),
            a.breakdown.as_ref(),
            b.breakdown.as_ref(),
            &mut out,
        );
    }
    if first.drives.len() != second.drives.len() {
        out.push(Difference {
            at: "drives".into(),
            first: format!("{} drives", first.drives.len()),
            second: format!("{} drives", second.drives.len()),
        });
    }
    for (i, (a, b)) in first.drives.iter().zip(&second.drives).enumerate() {
        let at = format!("drive {} ({})", i + 1, a.pin);
        compare_range(&format!("{at} range"), Some(&a.range), Some(&b.range), &mut out);
        let h = |d: &crate::limits::Drive| {
            d.headroom
                .as_ref()
                .map(|h| format!("duty {} volts {} (page {})", h.duty, h.volts, h.page))
        };
        opt(&format!("{at} headroom"), h(a), h(b), &mut out);
    }
    out
}

/// Where downloaded datasheets are kept: `$XDG_CACHE_HOME/gerberus/datasheets`, or
/// `~/.cache/gerberus/datasheets`.
pub fn default_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
    Some(base.join("gerberus").join("datasheets"))
}

/// A file name for a URL that is the same every run.
fn cache_name(url: &str) -> String {
    // FNV-1a: stable across Rust versions, unlike the standard hasher.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    let tail: String = url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    format!("{h:016x}-{tail}")
}

/// The PDF link in an LCSC datasheet page (`https://www.lcsc.com/datasheet/C1525.pdf` serves HTML that
/// points at the real file).
pub fn lcsc_pdf_link(html: &str) -> Option<String> {
    for host in ["https://datasheet.lcsc.com/", "https://wmsc.lcsc.com/"] {
        if let Some(s) = html.find(host) {
            let rest = &html[s..];
            let end = rest.find(['"', '\'', ' ', '<', '>'])?;
            // `...pdf?productCode=C1525`: the query only names the part.
            let link = rest[..end].split('?').next().unwrap_or("");
            if link.to_ascii_lowercase().ends_with(".pdf") {
                return Some(link.to_string());
            }
        }
    }
    None
}

/// Downloads a datasheet into `cache` (or returns the copy already there) and gives its path.
pub fn fetch(url: &str, cache: &Path) -> Result<PathBuf, String> {
    let path = cache.join(cache_name(url));
    if path.is_file() {
        return Ok(path);
    }
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(ureq::Proxy::try_from_env())
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(concat!("gerberus/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let get = |u: &str| -> Result<Vec<u8>, String> {
        agent
            .get(u)
            .call()
            .map_err(|e| format!("downloading {u}: {e}"))?
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| format!("reading {u}: {e}"))
    };
    let mut bytes = get(url)?;
    if !bytes.starts_with(b"%PDF") {
        let html = String::from_utf8_lossy(&bytes);
        let link = lcsc_pdf_link(&html).ok_or_else(|| format!("{url} is not a PDF and links to none"))?;
        bytes = get(&link)?;
        if !bytes.starts_with(b"%PDF") {
            return Err(format!("{link} is not a PDF"));
        }
    }
    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_numbers_with_signs_and_units() {
        let t = tokens("VIN to GND   –0.3   38 V   ±2000   −1.5mV  SOT-23  C5446  1/16W  5%");
        let got: Vec<(f64, bool, Option<Unit>)> =
            t.iter().map(|t| (t.value, t.plus_minus, t.unit.map(|u| u.0))).collect();
        assert_eq!(
            got,
            vec![
                (-0.3, false, None),
                (38.0, false, Some(Unit::Volt)),
                (2000.0, true, None),
                (-1.5, false, Some(Unit::Volt)),
                (0.0625, false, Some(Unit::Watt)),
                (5.0, false, Some(Unit::Fraction)),
            ]
        );
        assert!(close(t[3].unit.unwrap().1, 1e-3));
        assert_eq!(tokens("± 15 kV")[0].value, 15.0);
        assert!(tokens("± 15 kV")[0].plus_minus);
        assert!(tokens("0402WGF1001TCE").is_empty());
        assert_eq!(tokens("-40 to 125°C")[1].unit.map(|u| u.0), Some(Unit::Celsius));
        assert_eq!(tokens("100 ppm/°C")[0].unit.map(|u| u.0), Some(Unit::Ppm));
    }

    #[test]
    fn mentions_whole_words_only() {
        assert!(mentions("VIN to GND", "VIN"));
        assert!(mentions("VDD–VSS external", "vdd"));
        assert!(!mentions("VINT", "VIN"));
        assert!(!mentions("anything", ""));
    }
}
