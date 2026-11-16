//! Pinout agreement: a part's schematic symbol, its footprint on the board and its datasheet pin table must
//! describe the same pins.
//!
//! Three sources are compared, each by pin number:
//!
//! - the symbol's pins (number and name), from the schematic;
//! - the footprint's pads, from the `.kicad_pcb` file, which is what gets built;
//! - the datasheet pin table, from the part's `[pins]` map in `parts/` (see `parts/README.md`).
//!
//! Numbers must line up: a symbol pin with no pad never reaches copper, and a datasheet pin with no symbol pin
//! is left floating on the board. Names are compared loosely (case, KiCad markup, alternate functions written
//! as `PA9/PA11`, and a few common synonyms such as `GND`/`VSS`), because symbols rarely copy the datasheet
//! letter for letter. A name that matches a *different* datasheet pin is reported as a swap, which is an
//! error. Any other name difference is a warning for a person to look at, never a pass or a fail by guesswork.

use std::collections::{BTreeMap, BTreeSet};

use crate::design::{Component, Design, PinKind};
use crate::kicad::pcb::Footprint;
use crate::limits::{Library, Part};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Reference designator, such as `U2`.
    pub reference: String,
    pub severity: Severity,
    pub message: String,
}

/// What one component was compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// The footprint on the board was compared with the symbol.
    pub footprint: bool,
    /// The part file whose `[pins]` map was compared, by part number.
    pub datasheet: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Every schematic component, keyed by reference.
    pub coverage: BTreeMap<String, Coverage>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Error).count()
    }

    pub fn warnings(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Warning).count()
    }

    /// Findings for one reference.
    pub fn for_ref<'a>(&'a self, reference: &'a str) -> impl Iterator<Item = &'a Finding> + 'a {
        self.findings.iter().filter(move |f| f.reference == reference)
    }
}

/// Compares every component's symbol with its footprint (when `footprints` is given) and with its part
/// file's pin map (when the library has one for its part number).
pub fn check(design: &Design, footprints: Option<&[Footprint]>, lib: &Library) -> Report {
    let mut report = Report::default();
    let mut by_ref: BTreeMap<&str, Vec<&Footprint>> = BTreeMap::new();
    for fp in footprints.unwrap_or_default() {
        by_ref.entry(fp.reference.as_str()).or_default().push(fp);
    }

    for (r, c) in &design.components {
        let mut out = Findings {
            reference: r,
            list: Vec::new(),
        };
        let part = lib.part_for(c);
        let fp = if footprints.is_some() && c.on_board {
            match by_ref.get(r.as_str()).map(Vec::as_slice) {
                Some([one]) => Some(*one),
                Some(many) if many.len() > 1 => {
                    out.error(format!("{} footprints on the board carry this reference", many.len()));
                    None
                }
                _ => {
                    out.error(
                        "the symbol is marked as on the board but no footprint on the PCB has this reference".into(),
                    );
                    None
                }
            }
        } else {
            None
        };

        if let Some(fp) = fp {
            symbol_vs_footprint(c, fp, part, &mut out);
        }
        if let Some(part) = part {
            symbol_vs_datasheet(design, c, part, fp, &mut out);
            if let Some(fp) = fp {
                footprint_vs_datasheet(c, fp, part, &mut out);
            }
        }

        report.coverage.insert(
            r.clone(),
            Coverage {
                footprint: fp.is_some(),
                datasheet: part.map(|p| p.mpn.clone()),
            },
        );
        report.findings.extend(out.list);
    }

    if footprints.is_some() {
        for (r, fps) in &by_ref {
            let numbered = fps.iter().any(|f| !f.pads.is_empty());
            if numbered && !design.components.contains_key(*r) {
                report.findings.push(Finding {
                    reference: r.to_string(),
                    severity: Severity::Warning,
                    message: "footprint on the PCB has pads but no symbol in the schematic (update the PCB from the schematic?)".into(),
                });
            }
        }
    }
    report
}

struct Findings<'a> {
    reference: &'a str,
    list: Vec<Finding>,
}

impl Findings<'_> {
    fn error(&mut self, message: String) {
        self.push(Severity::Error, message);
    }

    fn warning(&mut self, message: String) {
        self.push(Severity::Warning, message);
    }

    fn push(&mut self, severity: Severity, message: String) {
        self.list.push(Finding {
            reference: self.reference.to_string(),
            severity,
            message,
        });
    }
}

fn symbol_vs_footprint(c: &Component, fp: &Footprint, part: Option<&Part>, out: &mut Findings) {
    if !c.footprint.is_empty() && !fp.lib_id.is_empty() && c.footprint != fp.lib_id {
        out.warning(format!(
            "the schematic names footprint {} but the board has {} (update the PCB from the schematic?)",
            c.footprint, fp.lib_id
        ));
    }
    let pads: BTreeSet<&str> = fp.pad_numbers().into_iter().collect();
    for (num, pin) in &c.pins {
        if !pads.contains(num.as_str()) {
            let msg = format!(
                "symbol pin {} has no pad on footprint {}, so its net never reaches copper",
                pin_label(num, &pin.name),
                fp.lib_id
            );
            if pin.kind == PinKind::NoConnect {
                out.warning(msg);
            } else {
                out.error(msg);
            }
        }
    }
    // With a part file, pads the symbol lacks are judged against the datasheet in `symbol_vs_datasheet`.
    if part.is_some() {
        return;
    }
    for pad in pads {
        if !c.pins.contains_key(pad) && !is_mechanical_pad(pad) {
            out.warning(format!(
                "pad {pad} of {} has no symbol pin and no part file says what it is, so it is left unconnected",
                fp.lib_id
            ));
        }
    }
}

fn symbol_vs_datasheet(design: &Design, c: &Component, part: &Part, fp: Option<&Footprint>, out: &mut Findings) {
    let mpn = &part.mpn;
    let ds = &part.pins;
    let has_pad = |n: &str| fp.is_some_and(|f| f.pads.iter().any(|p| p.number == n));

    for (num, name) in ds {
        if !c.pins.contains_key(num) && !is_no_connect_name(name) {
            let pad = if has_pad(num) {
                "its pad is left unconnected"
            } else {
                "nothing connects to it"
            };
            out.error(format!(
                "datasheet pin {} of {mpn} is missing from the symbol, so {pad}",
                pin_label(num, name)
            ));
        }
    }
    for (num, pin) in &c.pins {
        if !ds.contains_key(num) {
            out.error(format!(
                "symbol pin {} is not a pin of {mpn} (its datasheet pins are {}): wrong symbol or package?",
                pin_label(num, &pin.name),
                number_list(ds.keys())
            ));
        }
    }

    // A polarised two-terminal part drawn with a symbol that has no polarity, such as a bidirectional TVS
    // symbol on a unidirectional part. Reported once, instead of as two name differences.
    let ds_names: BTreeSet<&str> = ds.values().map(String::as_str).collect();
    let sym_names: Vec<String> = c.pins.values().filter_map(|p| shown_name(&p.name)).collect();
    let unpolarised_symbol = ds_names == BTreeSet::from(["A", "K"])
        && !sym_names.is_empty()
        && !sym_names.iter().any(|s| names_match(s, "A") || names_match(s, "K"));
    if unpolarised_symbol {
        let side = |name: &str| {
            part.pins_named(name)
                .iter()
                .map(|n| {
                    let net = design
                        .pin_net(&c.reference, n)
                        .map_or("no net".to_string(), |net| net.name.clone());
                    format!("pad {n} on {net}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.warning(format!(
            "the symbol's pins ({}) show no polarity but {mpn} is polarised: cathode (K) is {}, anode (A) is {}; \
             check that is the way round you meant",
            sym_names.join(", "),
            side("K"),
            side("A")
        ));
    }

    // Names, for numbers both sides have.
    let mut differ: Vec<String> = Vec::new();
    for (num, pin) in c.pins.iter().filter(|_| !unpolarised_symbol) {
        let Some(ds_name) = ds.get(num) else { continue };
        let Some(sym) = shown_name(&pin.name) else { continue };
        if names_match(&sym, ds_name) {
            continue;
        }
        let swapped_with = ds.iter().find(|(m, other)| {
            *m != num
                && names_match(&sym, other)
                && c.pins
                    .get(*m)
                    .and_then(|p| shown_name(&p.name))
                    .is_none_or(|s| !names_match(&s, other))
        });
        match swapped_with {
            Some((m, other)) => {
                let theirs = c.pins.get(m).and_then(|p| shown_name(&p.name));
                out.error(format!(
                    "symbol pin {num} is {sym:?}, but the datasheet puts {other} on pin {m} and {ds_name} on pin {num}{}: \
                     pins look swapped",
                    theirs.map(|t| format!(" (the symbol's pin {m} is {t:?})")).unwrap_or_default()
                ));
            }
            None => differ.push(format!("{num} {sym:?} (datasheet {ds_name})")),
        }
    }
    if !differ.is_empty() {
        out.warning(format!(
            "pin names differ from the {mpn} datasheet: {}",
            differ.join(", ")
        ));
    }

    // Pins the datasheet says must not be connected.
    for (num, name) in ds {
        if !is_do_not_connect(name) {
            continue;
        }
        let Some(net) = design.pin_net(&c.reference, num) else {
            continue;
        };
        let others: Vec<String> = net
            .pins
            .iter()
            .filter(|(r, p)| !(r == &c.reference && p == num))
            .map(|(r, p)| format!("{r}.{p}"))
            .collect();
        if !others.is_empty() {
            out.error(format!(
                "datasheet pin {} of {mpn} must not be connected, but the symbol joins it to net {} ({})",
                pin_label(num, name),
                net.name,
                others.join(" ")
            ));
        }
    }
}

fn footprint_vs_datasheet(c: &Component, fp: &Footprint, part: &Part, out: &mut Findings) {
    let pads: BTreeSet<&str> = fp.pad_numbers().into_iter().collect();
    let extra: Vec<&str> = pads
        .iter()
        .copied()
        .filter(|p| !part.pins.contains_key(*p) && !c.pins.contains_key(*p) && !is_mechanical_pad(p))
        .collect();
    if !extra.is_empty() {
        out.warning(format!(
            "footprint {} has pad {} that is not a pin of {} and has no symbol pin: footprint for a different package?",
            fp.lib_id,
            extra.join(", "),
            part.mpn
        ));
    }
    let missing: Vec<String> = part
        .pins
        .iter()
        .filter(|(n, _)| !pads.contains(n.as_str()))
        .map(|(n, name)| pin_label(n, name))
        .collect();
    if !missing.is_empty() {
        out.error(format!(
            "footprint {} has no pad for datasheet pin {} of {}: footprint for a different package?",
            fp.lib_id,
            missing.join(", "),
            part.mpn
        ));
    }
}

fn pin_label(num: &str, name: &str) -> String {
    match shown_name(name) {
        Some(n) if n != num => format!("{num} ({n})"),
        _ => num.to_string(),
    }
}

fn number_list<'a>(nums: impl Iterator<Item = &'a String>) -> String {
    let mut v: Vec<&str> = nums.map(String::as_str).collect();
    v.sort_by(|a, b| natural(a).cmp(&natural(b)));
    v.join(", ")
}

fn natural(s: &str) -> (u64, &str) {
    (s.parse().unwrap_or(u64::MAX), s)
}

/// Pads that KiCad's libraries use for mechanical anchors: `MP`, `MH`, `SH` and numbered forms like `MP1`.
fn is_mechanical_pad(pad: &str) -> bool {
    let up = pad.to_ascii_uppercase();
    ["MP", "MH", "SH"].iter().any(|p| {
        up.strip_prefix(p)
            .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
    })
}

fn is_no_connect_name(name: &str) -> bool {
    matches!(compact(name).as_str(), "NC" | "DNC" | "NOCONNECT" | "DONOTCONNECT")
}

fn is_do_not_connect(name: &str) -> bool {
    matches!(compact(name).as_str(), "DNC" | "DONOTCONNECT")
}

/// A pin name as a person reads it, without KiCad markup (`~{CS}`, `V_{IN}`). `None` for an unnamed pin
/// (`~` or empty), whose name says nothing about the datasheet.
pub fn shown_name(name: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = name.trim().chars().peekable();
    while let Some(ch) = chars.next() {
        if matches!(ch, '~' | '_' | '^') && chars.peek() == Some(&'{') {
            chars.next();
            continue;
        }
        if ch == '}' {
            continue;
        }
        out.push(ch);
    }
    let out = out.trim().to_string();
    (!out.is_empty() && out != "~").then_some(out)
}

/// Upper case with only letters, digits and polarity signs kept: `TRI-STATE` is `TRISTATE`, but `V+` and `V-`
/// stay apart.
fn compact(name: &str) -> String {
    let shown = shown_name(name).unwrap_or_default();
    let n = shown.chars().count();
    shown
        .chars()
        .enumerate()
        .filter(|(i, c)| c.is_ascii_alphanumeric() || *c == '+' || (*c == '-' && i + 1 == n))
        .map(|(_, c)| c.to_ascii_uppercase())
        .collect()
}

/// The words in a name: `PA9/PA11`, `PA11 [PA9]` and `PB8-BOOT0` each name two signals. A trailing `-` or
/// `+` (`V-`, `IN+`) stays part of the word. `Pin_3` is `3`.
fn tokens(name: &str) -> BTreeSet<String> {
    let shown = shown_name(name).unwrap_or_default().to_ascii_uppercase();
    let mut out = BTreeSet::new();
    for word in shown.split(|c: char| c.is_whitespace() || "/[](),|".contains(c)) {
        if word.is_empty() {
            continue;
        }
        let parts: Vec<&str> = word.split('-').collect();
        let pieces: Vec<&str> = if parts.len() > 1 && parts.iter().all(|p| !p.is_empty()) {
            parts
        } else {
            vec![word]
        };
        for p in pieces {
            let p = p
                .strip_prefix("PIN_")
                .or_else(|| p.strip_prefix("PIN"))
                .filter(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(p);
            out.insert(p.to_string());
        }
    }
    out
}

/// Names that mean the same pin across vendors. Kept short on purpose: every entry is a way for a real
/// mistake to go unreported.
const SYNONYMS: &[&[&str]] = &[
    &["GND", "VSS", "VSSA", "AGND", "DGND", "PGND", "GROUND"],
    &["VIN", "IN", "VI"],
    &["VOUT", "OUT", "VO", "OUTPUT"],
    &["VDD", "VCC"],
    &["EN", "CE", "ENABLE", "OE", "TRISTATE"],
    &["K", "CATHODE"],
    &["A", "ANODE"],
    &["NC", "DNC", "NOCONNECT", "DONOTCONNECT"],
];

fn canonical(word: &str) -> String {
    let flat: String = word.chars().filter(char::is_ascii_alphanumeric).collect();
    SYNONYMS
        .iter()
        .find(|g| g.contains(&flat.as_str()))
        .map_or(word.to_string(), |g| g[0].to_string())
}

/// Whether a symbol pin name and a datasheet pin name plausibly name the same pin.
pub fn names_match(a: &str, b: &str) -> bool {
    let (ca, cb) = (compact(a), compact(b));
    if !ca.is_empty() && (ca == cb || canonical(&ca) == canonical(&cb)) {
        return true;
    }
    let ta: BTreeSet<String> = tokens(a).iter().map(|t| canonical(t)).collect();
    let tb: BTreeSet<String> = tokens(b).iter().map(|t| canonical(t)).collect();
    ta.intersection(&tb).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_ignore_markup_case_and_alternates() {
        assert!(names_match("~{CSN}", "CSN"));
        assert!(names_match("V_{IN}", "VIN"));
        assert!(names_match("Vdd", "VDD"));
        assert!(names_match("PA9/PA11", "PA11 [PA9]"));
        assert!(names_match("PB8", "PB8-BOOT0"));
        assert!(names_match("VDD", "VDD/VDDA"));
        assert!(names_match("Pin_3", "3"));
        assert!(names_match("VSSA", "VSS"));
        assert!(names_match("EN", "TRI-STATE"));
        assert!(names_match("OUT", "OUTPUT"));
        assert!(names_match("V-", "V-"));
    }

    #[test]
    fn different_pins_do_not_match() {
        assert!(!names_match("CANH", "CANL"));
        assert!(!names_match("V+", "V-"));
        assert!(!names_match("A1", "K"));
        assert!(!names_match("VDD", "VDDH"));
        assert!(!names_match("S", "STB"));
        assert!(!names_match("PA9", "PA10"));
        assert!(!names_match("TXD", "RXD"));
    }

    #[test]
    fn unnamed_pins_have_no_name() {
        assert_eq!(shown_name("~"), None);
        assert_eq!(shown_name(""), None);
        assert_eq!(shown_name("~{RESET}").as_deref(), Some("RESET"));
    }

    #[test]
    fn mechanical_pads() {
        assert!(is_mechanical_pad("MP"));
        assert!(is_mechanical_pad("MP2"));
        assert!(!is_mechanical_pad("1"));
        assert!(!is_mechanical_pad("MPX"));
    }
}
