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
mod compare;
mod derived;
mod files;
mod reading;

pub use compare::{Difference, blank, compare};
pub use derived::{Derived, DerivedEnd, parse_derived};
pub use files::{
    FOLDER, default_cache_dir, fetch, file_name, folder_beside, lcsc_pdf_link, save_as, saved_for, saved_in, saved_name,
};
pub use reading::check_claim;

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::Path;

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
    /// Thermal resistance, °C/W (or K/W).
    CelsiusPerWatt,
    /// Farads. Part files give capacitance in pF; claims carry it in farads.
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

/// Which end of a range a value is, and so which table column it should come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Min,
    Max,
    /// A single figure, such as a tolerance.
    Value,
    /// A typical value, which belongs in a TYP column.
    Typ,
}

/// What the part file says.
#[derive(Debug, Clone, PartialEq)]
pub enum Claimed {
    Number(f64),
    Pin { pin: String, plus: f64 },
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
    /// Words that mark a row as some other quantity: a storage temperature is not an operating one.
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

#[cfg(test)]
mod tests {
    use super::reading::{close, mentions, tokens};
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
