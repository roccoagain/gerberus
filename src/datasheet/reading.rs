//! Reading a datasheet page's text: its tokens, table columns and units, and whether a claimed value sits where
//! the part file says.
use std::fmt;

use super::derived::{DerivedEnd, parse_derived};
use super::{Claim, Claimed, Outcome, Role, Status, Unit};

/// A table column, from a header line such as `PARAMETER  MIN  TYP  MAX  UNIT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Col {
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
pub(super) struct Token {
    /// Character columns, start inclusive, end exclusive.
    start: usize,
    end: usize,
    pub(super) value: f64,
    /// Printed with `±`.
    pub(super) plus_minus: bool,
    /// Unit printed right after it, with its scale: `mV` is (Volt, 1e-3).
    pub(super) unit: Option<(Unit, f64)>,
    /// Which end of a `min~max` cell it is (`20.00~22.10`), which settles its role whatever column it is in.
    cell: Option<Role>,
}

fn is_minus(c: char) -> bool {
    matches!(c, '-' | '–' | '−' | '‑' | '—' | '－')
}

/// Reads a unit right after a number (`mV`, ` V`, `%`, `°C`, `ppm`). `None` when there isn't one.
fn unit_at(chars: &[char], i: usize) -> Option<(Unit, f64, usize)> {
    // Right after the number, or after one space; or, as a table's next cell, after a wider gap when the cell
    // is nothing but a unit.
    let gap = chars[i.min(chars.len())..].iter().take_while(|c| **c == ' ').count();
    if gap <= 1 {
        return unit_word(chars, i + gap);
    }
    if gap > 16 {
        return None;
    }
    let found = unit_word(chars, i + gap)?;
    let alone = chars.get(found.2).is_none_or(|c| c.is_whitespace());
    alone.then_some(found)
}

fn unit_word(chars: &[char], i: usize) -> Option<(Unit, f64, usize)> {
    let rest: String = chars[i.min(chars.len())..].iter().take(8).collect();
    for (form, len) in [("°C/W", 4), ("ºC/W", 4), ("℃/W", 3), ("K/W", 3)] {
        if rest.starts_with(form) {
            return Some((Unit::CelsiusPerWatt, 1.0, i + len));
        }
    }
    let (scale, after) = match rest.chars().next()? {
        'm' if !rest.starts_with("mm") && !rest.starts_with("min") => (1e-3, &rest[1..]),
        'µ' | 'μ' => (1e-6, &rest['µ'.len_utf8()..]),
        'u' => (1e-6, &rest[1..]),
        'n' => (1e-9, &rest[1..]),
        'p' if !rest.starts_with("ppm") => (1e-12, &rest[1..]),
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
    } else if after.starts_with('F') && boundary(after, 1) {
        (Unit::Farad, 1)
    } else if after.starts_with('℃') {
        (Unit::Celsius, 1)
    } else if after.starts_with("°C") || after.starts_with("ºC") {
        (Unit::Celsius, 2)
    } else if after.starts_with(['%', '％']) {
        (Unit::Fraction, 1)
    } else {
        return None;
    };
    // A scale prefix only goes with V, A, W and F.
    if scale != 1.0 && !matches!(unit, Unit::Volt | Unit::Amp | Unit::Watt | Unit::Farad) {
        return None;
    }
    let prefix = usize::from(scale != 1.0);
    let scale = if unit == Unit::Fraction { 0.01 } else { scale };
    Some((unit, scale, i + prefix + len))
}

/// Whether the text ends with a pin name, such as `VIN` or `VDDIOx` or `V(BAT)`: a sign after it makes an offset
/// (`VIN + 0.3`). Words like `to` and units (`-0.3 V – 6.0 V`) make a range instead.
fn names_a_pin(chars: &[char]) -> bool {
    let text: String = chars.iter().collect();
    let word = text.rsplit(char::is_whitespace).next().unwrap_or("");
    let bare = word.trim_end_matches(')');
    bare.chars().count() >= 2
        && bare.chars().next().is_some_and(char::is_alphabetic)
        && bare.chars().any(|c| c.is_ascii_uppercase())
        && bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '(' | '_'))
        && !matches!(bare, "TO" | "AND" | "OR")
        && unit_word(&bare.chars().collect::<Vec<_>>(), 0).is_none_or(|(_, _, end)| end < bare.chars().count())
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
pub(super) fn tokens(line: &str) -> Vec<Token> {
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
        // Thousands separators: `2,000`, `15,000`.
        let mut grouped = false;
        while j - i <= 3 + 4 * usize::from(grouped)
            && chars.get(j) == Some(&',')
            && (1..=3).all(|d| chars.get(j + d).is_some_and(char::is_ascii_digit))
            && !chars.get(j + 4).is_some_and(char::is_ascii_digit)
        {
            grouped = true;
            j += 4;
        }
        if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(char::is_ascii_digit) {
            j += 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
        }
        let mut text: String = chars[i..j].iter().filter(|c| **c != ',').collect();
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
        // The offset in `VIN + 0.3` or `VDD - 0.4` belongs to a relative bound, not a number of its own. (A wide
        // gap before the sign makes it a table cell instead: `voltage     - 0.3`.)
        let mut q = i;
        if q > 0 && chars[q - 1] == ' ' {
            q -= 1;
        }
        if q > 0 && (matches!(chars[q - 1], '+' | '＋') || is_minus(chars[q - 1])) {
            let sign = q - 1;
            let mut r = sign;
            if r > 0 && chars[r - 1] == ' ' {
                r -= 1;
            }
            if names_a_pin(&chars[..r]) && !(is_minus(chars[sign]) && ends_with_ground(&chars[..sign])) {
                i = j.max(i + 1);
                continue;
            }
        }
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
            let before_ok = k < 2 || !chars[k - 2].is_ascii_alphanumeric();
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
            // The number's own extent: a unit in the next cell mustn't pull it towards the UNIT column.
            end: j,
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
    let mut value_spans: Vec<(usize, usize)> = Vec::new();
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
            value_spans.push((s, e));
        }
        cols.push(((s + e) as f64 / 2.0, col));
    }
    // Table columns stand apart. Value words that only ever follow each other with a single space are a
    // title, such as "ESD maximum ratings" above the rows of a limiting values table.
    let apart = value_spans.windows(2).any(|w| w[1].0 >= w[0].1 + 2);
    // One value column is enough beside a UNIT column, or under a table's own PARAMETER or SYMBOL heading
    // ("Parameter  Conditions  Value").
    let labelled = words(line).iter().any(|(_, _, w)| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_uppercase();
        matches!(w.as_str(), "PARAMETER" | "PARAMETERS" | "SYMBOL")
    });
    let ok = (values >= 2 && apart) || (values == 1 && (cols.iter().any(|(_, c)| *c == Col::Unit) || labelled));
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

/// Whether two values are the same number. The tolerance is relative, so picofarads and nanoamps compare as
/// finely as volts; the tiny floor only absorbs rounding around zero.
pub(super) fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-15_f64.max(1e-6 * a.abs().max(b.abs()))
}

/// Whether `name` appears in `text` as a word of its own (case-insensitive): `VIN` in `VIN to GND`, but not in
/// `VINT`.
pub(super) fn mentions(text: &str, name: &str) -> bool {
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
    // A worked-out value is checked by its arithmetic, but never hides a value printed in the wrong place.
    if best != Status::Mismatch
        && claim.derived.is_some()
        && let Some(o) = derivation(claim, page)
        && (o.status == Status::Mismatch || o.status > best)
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
    let outcome = |status: Status, reason: String| Outcome {
        claim: claim.clone(),
        status,
        reason,
        evidence: vec![],
    };
    if let Some(text) = &claim.derived {
        let d = match parse_derived(text) {
            Ok(d) => d,
            Err(e) => return Some(outcome(Status::Mismatch, format!("can't read the derived text: {e}"))),
        };
        let end = if claim.role == Role::Min { &d.min } else { &d.max };
        return Some(match (end, &claim.value) {
            (DerivedEnd::Number { value, inputs }, Claimed::Number(want)) => {
                if differs_beyond_four_significant_figures(*value, *want) {
                    return Some(outcome(
                        Status::Mismatch,
                        format!("the derived arithmetic gives {value:.4}, not {want}"),
                    ));
                }
                // A percentage counts as itself and as a fraction: `2 %` backs an input of 0.02.
                let printed: Vec<f64> = page
                    .lines()
                    .flat_map(tokens)
                    .flat_map(|t| {
                        let v = t.value.abs();
                        [
                            Some(v),
                            matches!(t.unit, Some((Unit::Fraction, _))).then_some(v / 100.0),
                        ]
                    })
                    .flatten()
                    .collect();
                // 0, 1 and 100 are arithmetic (1 − 2 %, 100 %), not readings.
                let missing: Vec<String> = inputs
                    .iter()
                    .filter(|v| !ARITHMETIC_CONSTANTS.contains(*v) && !printed.iter().any(|p| close(*p, **v)))
                    .map(ToString::to_string)
                    .collect();
                if missing.is_empty() {
                    outcome(Status::Derived, format!("derived: {}", d.explanation))
                } else {
                    outcome(
                        Status::Plausible,
                        format!(
                            "derived: {} ({} not on this page; check where they come from)",
                            d.explanation,
                            missing.join(", ")
                        ),
                    )
                }
            }
            (DerivedEnd::Pin { pin, plus }, Claimed::Pin { plus: want, .. }) => {
                if !close(*plus, *want) {
                    return Some(outcome(
                        Status::Mismatch,
                        format!("the derived text gives {pin} + {plus}, but the part file adds {want}"),
                    ));
                }
                let as_printed = Claim {
                    value: Claimed::Pin {
                        pin: pin.clone(),
                        plus: *plus,
                    },
                    derived: None,
                    ..claim.clone()
                };
                let found = check_claim(&as_printed, page);
                if found.status >= Status::Plausible {
                    outcome(
                        found.status.min(Status::Derived),
                        format!("printed as {pin} + {plus}: {}", d.explanation),
                    )
                } else {
                    outcome(Status::NotFound, format!("{pin} + {plus} isn't on the page either"))
                }
            }
            // A cap on a relative bound is an ordinary number; the derived text is about the bound.
            (DerivedEnd::Pin { .. }, Claimed::Number(_)) => return None,
            (DerivedEnd::Number { .. }, Claimed::Pin { .. }) => outcome(
                Status::Mismatch,
                "the derived text gives a number, but the part file has a pin bound".into(),
            ),
        });
    }
    let Claimed::Number(want) = claim.value else {
        return None;
    };
    if want == 0.0 {
        return Some(outcome(
            Status::Derived,
            "zero, the bottom of a rating that only gives a maximum".into(),
        ));
    }
    if claim.role == Role::Min && claim.other_end.is_some_and(|o| close(o, -want)) {
        return Some(outcome(
            Status::Derived,
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
        // A one-figure rating (a tolerance, a duty cycle) is often printed in a MIN or MAX column on its own.
        (_, Col::Value)
        | (Role::Min, Col::Min)
        | (Role::Max, Col::Max)
        | (Role::Typ, Col::Typ)
        | (Role::Value, Col::Min | Col::Max) => true,
        // ±15 kV in a MAX column bounds both ends.
        (Role::Min, Col::Max) => plus_minus,
        _ => false,
    }
}

fn as_role(role: Role) -> &'static str {
    match role {
        Role::Min => "a minimum",
        Role::Max => "a maximum",
        Role::Value => "a rating",
        Role::Typ => "a typical value",
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
    // A unit from a nearby line that isn't the claim's is likely another row's: read the number as unitless,
    // with that as a doubt.
    let nearby_other = unit.filter(|((u, _), printed)| !printed && *u != claim.unit);
    let (value_matches, unit_known, unit_ok, unit_printed) =
        if let Some(((u, scale), printed)) = unit.filter(|_| nearby_other.is_none()) {
            let v = t.value * scale;
            let matches = close(v, want) || (t.plus_minus && close(-v, want));
            (matches, true, u == claim.unit, printed)
        } else {
            // With no unit in sight, a current or voltage may be printed in milli- or micro- units (a table whose
            // UNIT column sits on another line). That stays a doubt: the unit isn't printed beside it.
            let scales: &[f64] = if matches!(claim.unit, Unit::Volt | Unit::Amp | Unit::Watt) {
                &[1.0, 1e-3, 1e-6]
            } else {
                &[1.0]
            };
            let matches = scales
                .iter()
                .any(|k| close(t.value * k, want) || (t.plus_minus && close(-t.value * k, want)))
                || (claim.unit == Unit::Fraction && close(t.value / 100.0, want));
            (matches, false, nearby_other.is_none(), false)
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
    // A number printed with another unit is another quantity, not this one.
    if !unit_ok && unit_printed {
        return None;
    }
    match col {
        // A typical value filed as a limit, or one end filed as the other, is the mistake that matters most.
        // (On a row about some other pin, the number is most likely some other quantity.)
        Some((c @ (Col::Typ | Col::Min | Col::Max), _)) if row && !role_fits(claim.role, c, t.plus_minus) => {
            return Some((
                Status::Mismatch,
                0,
                format!(
                    "found only as {c}, but the part file uses it as {}",
                    as_role(claim.role)
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
    if let Some(bad) = claim.avoid.iter().find(|w| mentions(lines[n], w))
        && !claim.context[..claim.context.len().saturating_sub(1)]
            .iter()
            .any(|w| mentions(lines[n], w))
    {
        return Some((
            Status::Mismatch,
            0,
            format!("found only on a row about {bad}, which isn't what the part file uses it as"),
        ));
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
        // TI prints a pin in brackets when it carries a sign: `(V+) + 0.5`.
        let trimmed = rest.trim_start();
        let trimmed = if line[..s].ends_with('(') {
            trimmed.strip_prefix(')').map_or(trimmed, str::trim_start)
        } else {
            trimmed
        };
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

/// Part files round worked-out values to four significant figures.
fn differs_beyond_four_significant_figures(value: f64, want: f64) -> bool {
    (value - want).abs() > 5e-4 * value.abs().max(want.abs()) + 1e-12
}

pub(super) const ARITHMETIC_CONSTANTS: [f64; 3] = [0.0, 1.0, 100.0];

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
