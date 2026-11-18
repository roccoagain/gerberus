use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::limits::{Bound, DriveKind, Part, Quantity, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Volt,
    Amp,
    Watt,
    Celsius,
    Ppm,
    Fraction,
    CelsiusPerWatt,
    Farad,
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
            Unit::CelsiusPerWatt => "°C/W",
            Unit::Farad => "F",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Min,
    Max,
    Value,
    Typ,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Claimed {
    Number(f64),
    Pin { pin: String, plus: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub label: String,
    pub value: Claimed,
    pub role: Role,
    pub unit: Unit,
    pub page: u32,
    pub context: Vec<String>,
    pub derived: Option<String>,
    pub other_end: Option<f64>,
    pub avoid: Vec<String>,
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

pub fn claims(part: &Part) -> Vec<Claim> {
    let mut out = Vec::new();
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
            context: vec!["tolerance", "accuracy"]
                .into_iter()
                .map(String::from)
                .chain([part.mpn.clone()])
                .collect(),
            derived: None,
            other_end: None,
            avoid: vec![],
        });
    }
    if let Some(t) = &part.tcr {
        out.push(Claim {
            label: "tcr".into(),
            value: Claimed::Number(t.ppm),
            role: Role::Value,
            unit: Unit::Ppm,
            page: t.page,
            context: vec!["TCR", "coefficient", "T.C.R"]
                .into_iter()
                .map(String::from)
                .chain([part.mpn.clone()])
                .collect(),
            derived: None,
            other_end: None,
            avoid: vec![],
        });
    }
    for (i, c) in part.capacitance.iter().enumerate() {
        let mut pins: Vec<&str> = c.pins.iter().map(String::as_str).collect();
        pins.push(&c.to);
        let context = named(&pins);
        for (role, v, end) in [(Role::Typ, c.typ, "typ"), (Role::Max, c.max, "max")] {
            let Some(v) = v else { continue };
            out.push(Claim {
                label: format!("capacitance {} ({} to {}) {end}", i + 1, c.pins.join(", "), c.to),
                value: Claimed::Number(v * 1e-12),
                role,
                unit: Unit::Farad,
                page: c.page,
                context: context.clone(),
                derived: None,
                other_end: None,
                avoid: vec![],
            });
        }
    }
    if let Some(t) = &part.temperature {
        for (role, v, end) in [(Role::Min, t.min, "min"), (Role::Max, t.max, "max")] {
            out.push(Claim {
                label: format!("temperature {end}"),
                value: Claimed::Number(v),
                role,
                unit: Unit::Celsius,
                page: t.page,
                context: vec!["operating", "TOPR", "TA", "TJ", "ambient", "junction"]
                    .into_iter()
                    .map(String::from)
                    .chain([part.mpn.clone()])
                    .collect(),
                derived: None,
                other_end: None,
                avoid: vec!["storage".into(), "TSTG".into()],
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
        if let Some(c) = &l.clamp {
            for (v, unit, name) in [(c.volts, Unit::Volt, "volts"), (c.current, Unit::Amp, "current")] {
                out.push(Claim {
                    label: format!("limit {} ({what}) clamp {name}", i + 1),
                    value: Claimed::Number(v),
                    role: Role::Value,
                    unit,
                    page: c.page,
                    context: ["VC", "clamping", "IPP", "VCL"]
                        .into_iter()
                        .map(String::from)
                        .chain(context.iter().cloned())
                        .collect(),
                    derived: None,
                    other_end: None,
                    avoid: vec![],
                });
            }
        }
    }
    for (i, e) in part.esd.iter().enumerate() {
        let pins: Vec<&str> = e.pins.iter().map(String::as_str).collect();
        let mut context = named(&pins);
        context.insert(0, e.model.name().split(' ').next().unwrap_or("").to_string());
        out.push(Claim {
            label: format!(
                "esd {} ({}, {})",
                i + 1,
                e.model.name(),
                if pins.is_empty() {
                    "all pins".to_string()
                } else {
                    pins.join(", ")
                }
            ),
            value: Claimed::Number(e.volts),
            role: Role::Value,
            unit: Unit::Volt,
            page: e.page,
            context,
            derived: None,
            other_end: None,
            avoid: vec![],
        });
    }
    let thermal = [
        (
            "theta_ja",
            &part.theta_ja,
            Unit::CelsiusPerWatt,
            ["thermal", "θJA", "RθJA"],
        ),
        ("tj_max", &part.tj_max, Unit::Celsius, ["junction", "TJ", "Tj"]),
    ];
    if let Some(e) = &part.efficiency {
        out.push(Claim {
            label: "efficiency".into(),
            value: Claimed::Number(e.value),
            role: Role::Min,
            unit: Unit::Fraction,
            page: e.page,
            context: vec!["efficiency".into(), part.mpn.clone()],
            derived: None,
            other_end: None,
            avoid: vec![],
        });
    }
    for (label, r, unit, words) in thermal {
        if let Some(r) = r {
            out.push(Claim {
                label: label.into(),
                value: Claimed::Number(r.value),
                role: if label == "tj_max" { Role::Max } else { Role::Value },
                unit,
                page: r.page,
                context: words
                    .iter()
                    .map(|w| (*w).to_string())
                    .chain([part.mpn.clone()])
                    .collect(),
                derived: None,
                other_end: None,
                avoid: if label == "tj_max" {
                    vec!["storage".into(), "TSTG".into()]
                } else {
                    vec![]
                },
            });
        }
    }
    for (i, s) in part.supply.iter().enumerate() {
        let pins: Vec<&str> = s.pins.iter().map(String::as_str).collect();
        let context = named(&pins);
        range_claims(
            &format!("supply {} ({})", i + 1, s.pins.join(", ")),
            &s.current,
            Unit::Amp,
            &context,
            &mut out,
        );
    }
    for (i, a) in part.opamps.iter().enumerate() {
        if let Some(s) = &a.swing {
            out.push(Claim {
                label: format!("opamp {} ({}) output swing from rail", i + 1, a.out),
                value: Claimed::Number(s.value),
                role: Role::Max,
                unit: Unit::Volt,
                page: s.page,
                context: named(&[a.out.as_str()]),
                derived: None,
                other_end: None,
                avoid: vec![],
            });
        }
    }
    for (i, d) in part.drives.iter().enumerate() {
        let kind = match d.kind {
            DriveKind::Regulates => "regulates",
            DriveKind::Forward => "forward",
            DriveKind::Switches => "switches",
            DriveKind::Biases => "biases",
            DriveKind::Channel => "channel",
        };
        let context = named(&[d.pin.as_str(), d.to.as_str()]);
        let label = format!("drive {} ({kind} {} to {})", i + 1, d.pin, d.to);
        range_claims(&label, &d.range, Unit::Volt, &context, &mut out);
        if let (Some(amps), Range::Known { page: Some(page), .. }) = (d.current, &d.range) {
            out.push(Claim {
                label: format!("{label} test current"),
                value: Claimed::Number(amps),
                role: Role::Value,
                unit: Unit::Amp,
                page: *page,
                context: context.clone(),
                derived: None,
                other_end: None,
                avoid: vec![],
            });
        }
        if let Some(o) = &d.on {
            out.push(Claim {
                label: format!("{label} on at VGS"),
                value: Claimed::Number(o.vgs),
                role: Role::Value,
                unit: Unit::Volt,
                page: o.page,
                context: context.clone(),
                derived: None,
                other_end: None,
                avoid: vec![],
            });
        }
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
                    avoid: vec![],
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
                    avoid: vec![],
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
                        avoid: vec![],
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
            avoid: vec![],
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Mismatch,
    NotFound,
    NoText,
    Plausible,
    Derived,
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
    pub reason: String,
    pub evidence: Vec<String>,
}

impl Outcome {
    pub fn is_problem(&self) -> bool {
        matches!(self.status, Status::Mismatch | Status::NotFound)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Min,
    Typ,
    Max,
    Value,
    Unit,
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct Token {
    start: usize,
    end: usize,
    value: f64,
    plus_minus: bool,
    unit: Option<(Unit, f64)>,
    cell: Option<Role>,
}

fn is_minus(c: char) -> bool {
    matches!(c, '-' | '–' | '−' | '‑' | '—' | '－')
}

fn unit_at(chars: &[char], i: usize) -> Option<(Unit, f64, usize)> {
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
    if scale != 1.0 && !matches!(unit, Unit::Volt | Unit::Amp | Unit::Watt | Unit::Farad) {
        return None;
    }
    let prefix = usize::from(scale != 1.0);
    let scale = if unit == Unit::Fraction { 0.01 } else { scale };
    Some((unit, scale, i + prefix + len))
}

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
        let mut start = i;
        let mut plus_minus = false;
        let mut k = i;
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
    let apart = value_spans.windows(2).any(|w| w[1].0 >= w[0].1 + 2);
    let labelled = words(line).iter().any(|(_, _, w)| {
        let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_ascii_uppercase();
        matches!(w.as_str(), "PARAMETER" | "PARAMETERS" | "SYMBOL")
    });
    let ok = (values >= 2 && apart) || (values == 1 && (cols.iter().any(|(_, c)| *c == Col::Unit) || labelled));
    let others = cols.iter().filter(|(_, c)| *c == Col::Other).count();
    (ok && others <= 10).then_some(cols)
}

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
                let mut probe = vec!['1'];
                probe.extend(&chars[s..]);
                unit_at(&probe, 1).map(|(u, sc, _)| ((u, sc), same_line))
            })
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-15_f64.max(1e-6 * a.abs().max(b.abs()))
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum DerivedEnd {
    Number { value: f64, inputs: Vec<f64> },
    Pin { pin: String, plus: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub min: DerivedEnd,
    pub max: DerivedEnd,
    pub explanation: String,
}

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
        let spaced = [" + ", " - "]
            .iter()
            .filter_map(|op| text.rfind(op).map(|i| i + 1))
            .max();
        let mut split = spaced.unwrap_or_else(|| text.find(['+', '-']).unwrap_or(text.len()));
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

fn row_mentions(claim: &Claim, lines: &[&str], n: usize) -> bool {
    let lo = n.saturating_sub(2);
    let hi = (n + 2).min(lines.len().saturating_sub(1));
    lines[lo..=hi]
        .iter()
        .any(|l| claim.context.iter().any(|p| mentions(l, p)))
}

fn role_fits(role: Role, col: Col, plus_minus: bool) -> bool {
    match (role, col) {
        (_, Col::Value)
        | (Role::Min, Col::Min)
        | (Role::Max, Col::Max)
        | (Role::Typ, Col::Typ)
        | (Role::Value, Col::Min | Col::Max) => true,
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
    let own = &claim.context[claim.context.len().saturating_sub(1)..];
    let col = column(lines, n, t.start, t.end, own);
    let unit = t
        .unit
        .map(|u| (u, true))
        .or_else(|| col.and_then(|(_, h)| unit_column(lines, n, h, own)));
    let nearby_other = unit.filter(|((u, _), printed)| !printed && *u != claim.unit);
    let (value_matches, unit_known, unit_ok, unit_printed) =
        if let Some(((u, scale), printed)) = unit.filter(|_| nearby_other.is_none()) {
            let v = t.value * scale;
            let matches = close(v, want) || (t.plus_minus && close(-v, want));
            (matches, true, u == claim.unit, printed)
        } else {
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
        Some(end) if end == claim.role || claim.role == Role::Value => Some((Col::Value, n)),
        Some(Role::Min) => Some((Col::Min, n)),
        Some(_) => Some((Col::Max, n)),
        None => col,
    };
    if !unit_ok && unit_printed {
        return None;
    }
    match col {
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

fn trimmed_after_cell(rest: &str) -> bool {
    let r = rest.trim_start();
    let r = r.strip_prefix('+').map_or(r, |x| {
        x.trim_start()
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
    });
    r.trim_start().starts_with(['~', '～'])
}

fn judge_relative(claim: &Claim, pin: &str, plus: f64, lines: &[&str], n: usize) -> Option<(Status, String)> {
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

pub fn verify(part: &Part, pdf: &Path) -> Vec<Outcome> {
    verify_with(part, |p| page_text(pdf, p))
}

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

const VALUE_KEYS: [&str; 21] = [
    "on",
    "swing",
    "efficiency",
    "theta_ja",
    "tj_max",
    "current",
    "description",
    "note",
    "volts",
    "page",
    "abs_max",
    "recommended",
    "breakdown",
    "clamp",
    "range",
    "headroom",
    "tolerance",
    "tcr",
    "temperature",
    "typ",
    "max",
];

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
    let rated = |r: &Option<crate::limits::Rated>| r.as_ref().map(|r| format!("{} (page {})", r.value, r.page));
    opt("theta_ja", rated(&first.theta_ja), rated(&second.theta_ja), &mut out);
    opt("tj_max", rated(&first.tj_max), rated(&second.tj_max), &mut out);
    opt(
        "efficiency",
        rated(&first.efficiency),
        rated(&second.efficiency),
        &mut out,
    );
    if first.supply.len() == second.supply.len() {
        for (i, (a, b)) in first.supply.iter().zip(&second.supply).enumerate() {
            let at = format!("supply {} ({})", i + 1, a.pins.join(", "));
            if a.pins == b.pins {
                compare_range(&format!("{at} current"), Some(&a.current), Some(&b.current), &mut out);
            } else {
                out.push(Difference {
                    at,
                    first: format!("{:?}", a.pins),
                    second: format!("{:?}", b.pins),
                });
            }
        }
    } else {
        out.push(Difference {
            at: "supply".into(),
            first: format!("{} entries", first.supply.len()),
            second: format!("{} entries", second.supply.len()),
        });
    }
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
        let clamp = |c: Option<&crate::limits::Clamp>| {
            c.map_or("absent".to_string(), |c| {
                format!("{} V at {} A (page {})", c.volts, c.current, c.page)
            })
        };
        let (ca, cb) = (clamp(a.clamp.as_ref()), clamp(b.clamp.as_ref()));
        if ca != cb {
            out.push(Difference {
                at: format!("{at} clamp"),
                first: ca,
                second: cb,
            });
        }
    }
    if first.drives.len() != second.drives.len() {
        out.push(Difference {
            at: "drives".into(),
            first: format!("{} drives", first.drives.len()),
            second: format!("{} drives", second.drives.len()),
        });
    }
    let swings = |p: &Part| -> Vec<String> {
        p.opamps
            .iter()
            .map(|a| {
                let s = a
                    .swing
                    .as_ref()
                    .map_or("none".into(), |s| format!("{} V (page {})", s.value, s.page));
                format!("{} {} {} {} {}: swing {s}", a.plus, a.minus, a.out, a.vpos, a.vneg)
            })
            .collect()
    };
    if swings(first) != swings(second) {
        out.push(Difference {
            at: "opamps".into(),
            first: swings(first).join("; "),
            second: swings(second).join("; "),
        });
    }
    let esd = |p: &Part| -> Vec<String> {
        let mut v: Vec<String> = p
            .esd
            .iter()
            .map(|e| format!("{} {:?} ±{} V (page {})", e.model.name(), e.pins, e.volts, e.page))
            .collect();
        v.sort();
        v
    };
    let (ea, eb) = (esd(first), esd(second));
    for x in ea.iter().filter(|x| !eb.contains(x)) {
        out.push(Difference {
            at: "esd".into(),
            first: x.clone(),
            second: "absent or different".into(),
        });
    }
    for x in eb.iter().filter(|x| !ea.contains(x)) {
        out.push(Difference {
            at: "esd".into(),
            first: "absent or different".into(),
            second: x.clone(),
        });
    }
    let capacitance = |p: &Part| -> Vec<String> {
        let mut v: Vec<String> = p
            .capacitance
            .iter()
            .map(|c| {
                format!(
                    "{:?} to {}: typ {:?} max {:?} pF (page {})",
                    c.pins, c.to, c.typ, c.max, c.page
                )
            })
            .collect();
        v.sort();
        v
    };
    let (ca, cb) = (capacitance(first), capacitance(second));
    for x in ca.iter().filter(|x| !cb.contains(x)) {
        out.push(Difference {
            at: "capacitance".into(),
            first: x.clone(),
            second: "absent or different".into(),
        });
    }
    for x in cb.iter().filter(|x| !ca.contains(x)) {
        out.push(Difference {
            at: "capacitance".into(),
            first: "absent or different".into(),
            second: x.clone(),
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
        opt(
            &format!("{at} on at VGS"),
            a.on.as_ref().map(|o| format!("{} V (page {})", o.vgs, o.page)),
            b.on.as_ref().map(|o| format!("{} V (page {})", o.vgs, o.page)),
            &mut out,
        );
        opt(&format!("{at} gate"), a.gate.clone(), b.gate.clone(), &mut out);
        opt(
            &format!("{at} test current"),
            a.current.map(|c| c.to_string()),
            b.current.map(|c| c.to_string()),
            &mut out,
        );
    }
    out
}

pub fn default_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
    Some(base.join("gerberus").join("datasheets"))
}

const ARITHMETIC_CONSTANTS: [f64; 3] = [0.0, 1.0, 100.0];

fn differs_beyond_four_significant_figures(value: f64, want: f64) -> bool {
    (value - want).abs() > 5e-4 * value.abs().max(want.abs()) + 1e-12
}

fn fnv1a_hash_stable_across_rust_versions(text: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    text.bytes()
        .fold(FNV_OFFSET_BASIS, |h, b| (h ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

fn cache_name(url: &str) -> String {
    let h = fnv1a_hash_stable_across_rust_versions(url);
    let tail: String = url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    format!("{h:016x}-{tail}")
}

pub fn lcsc_pdf_link(html: &str) -> Option<String> {
    for host in ["https://datasheet.lcsc.com/", "https://wmsc.lcsc.com/"] {
        if let Some(s) = html.find(host) {
            let rest = &html[s..];
            let end = rest.find(['"', '\'', ' ', '<', '>'])?;
            let link = rest[..end].split('?').next().unwrap_or("");
            if link.to_ascii_lowercase().ends_with(".pdf") {
                return Some(link.to_string());
            }
        }
    }
    None
}

pub const FOLDER: &str = "datasheets";

pub fn saved_name(mpn: &str) -> String {
    let toml = crate::doctor::file_name(mpn);
    format!("{}.pdf", toml.strip_suffix(".toml").unwrap_or(&toml))
}

pub fn saved_in(dir: &Path, names: &[String]) -> Option<PathBuf> {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    let name = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()).map(str::to_string);
    names.iter().find_map(|want| {
        files
            .iter()
            .find(|p| name(p).as_deref() == Some(want.as_str()))
            .or_else(|| {
                files
                    .iter()
                    .find(|p| name(p).is_some_and(|n| n.eq_ignore_ascii_case(want)))
            })
            .cloned()
    })
}

pub fn folder_beside(parts: &Path) -> PathBuf {
    let named = matches!(parts.components().next_back(), Some(std::path::Component::Normal(_)));
    let parts = if named {
        parts.to_path_buf()
    } else {
        std::fs::canonicalize(if parts.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parts
        })
        .unwrap_or_else(|_| parts.to_path_buf())
    };
    match parts.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(FOLDER),
        _ => PathBuf::from(FOLDER),
    }
}

pub fn saved_for(part_file: &Path, mpn: &str) -> Option<PathBuf> {
    let dir = part_file.parent().unwrap_or(Path::new(""));
    let mut names = vec![saved_name(mpn)];
    if let Some(stem) = part_file.file_stem().and_then(|s| s.to_str()) {
        let by_file = format!("{stem}.pdf");
        if !names.contains(&by_file) {
            names.push(by_file);
        }
    }
    let inside = if dir.as_os_str().is_empty() {
        PathBuf::from(FOLDER)
    } else {
        dir.join(FOLDER)
    };
    saved_in(&folder_beside(dir), &names).or_else(|| saved_in(&inside, &names))
}

pub fn save_as(part_file: &Path, mpn: &str) -> PathBuf {
    folder_beside(part_file.parent().unwrap_or(Path::new(""))).join(saved_name(mpn))
}

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
