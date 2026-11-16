//! Part limits: the voltage, current and power ranges each part's pins may see, taken from its datasheet.
//!
//! One TOML file per part number lives in `parts/`. Every known value cites the datasheet page it came from.
//! A value the datasheet does not give is written as `{ unknown = "reason" }` instead of being guessed, so a
//! checker can report `UNKNOWN` with that reason. See `parts/README.md` for the file format.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::design::Component;

/// One part number's datasheet limits.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    /// What kind of part this is. A derating policy picks parts by class.
    pub class: Class,
    /// Missing only when no datasheet could be found; every limit must then be unknown.
    pub datasheet: Option<Datasheet>,
    /// Pin number (as printed in the datasheet and used by the footprint) to datasheet pin name. Several
    /// numbers may share a name, such as two `GND` pads. Limits name pins, never numbers, so other packages
    /// of the same die only need their own pin map (see `variants`).
    pub pins: BTreeMap<String, String>,
    /// Other part numbers covered by the same datasheet limits, such as other packages or reel sizes. Each
    /// gives its own pin map, or reuses `pins` when it has none.
    #[serde(default, rename = "variant")]
    pub variants: Vec<Variant>,
    #[serde(default, rename = "limit")]
    pub limits: Vec<Limit>,
    /// What the part holds its pins at when it is working: a regulator's output or feedback voltage, a
    /// diode's forward drop. A checker uses these to work out the voltage on the nets the part drives.
    #[serde(default, rename = "drive")]
    pub drives: Vec<Drive>,
}

/// A voltage the part itself sets between two of its pins while operating.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drive {
    pub kind: DriveKind,
    pub pin: String,
    pub to: String,
    /// `V(pin) - V(to)` in volts, with its page. Conditions (load current, temperature) go in the note.
    pub range: Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveKind {
    /// A regulator holds `pin` at this voltage from `to`: a fixed output, or a feedback pin that sets the
    /// output through a divider.
    Regulates,
    /// A diode conducting from `pin` (anode) to `to` (cathode) drops this much.
    Forward,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub mpn: String,
    pub pins: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Capacitor,
    Resistor,
    Inductor,
    Diode,
    Tvs,
    Led,
    Connector,
    Oscillator,
    Ic,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Datasheet {
    pub title: String,
    /// Document number and revision, so a page number can be checked against the right edition.
    pub document: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantity {
    /// Volts on `pins` measured from `to`.
    #[default]
    Voltage,
    /// Amps flowing into the part at `pins` (out of it at `to`, for a two-terminal part).
    Current,
    /// Watts dissipated in the part between `pins` and `to`.
    Power,
}

/// The limits on one quantity for a group of pins.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    #[serde(default)]
    pub quantity: Quantity,
    /// Datasheet pin names the limit applies to, each on its own.
    pub pins: Vec<String>,
    /// The reference pin name. When absent, a voltage limit holds between any two of `pins` (a connector's
    /// contact rating) and a current limit holds for each pin on its own.
    pub to: Option<String>,
    /// For protection parts: the breakdown range applies in both polarities.
    #[serde(default)]
    pub bidirectional: bool,
    /// Beyond this range the part may be damaged.
    pub abs_max: Range,
    /// The range the datasheet says to operate in.
    pub recommended: Range,
    /// For TVS, Zener and ESD parts: the voltage at which the part starts to conduct. A rail that can sit
    /// above `breakdown.min` will drive current through the part continuously.
    pub breakdown: Option<Range>,
}

/// A min/max pair, or a reason why the datasheet doesn't give one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "RawRange")]
pub enum Range {
    Unknown(String),
    Known {
        min: Bound,
        max: Bound,
        /// 1-based page of the datasheet PDF the values are on.
        page: Option<u32>,
        note: Option<String>,
    },
}

/// One end of a range.
#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    Value(f64),
    /// Relative to another pin of the same part: `V(pin) + plus`, capped at `at_most` when given. `V(pin)`
    /// is measured from the same reference as the limit (its `to` pin). `at_most` only makes sense on `max`.
    Pin {
        pin: String,
        plus: f64,
        at_most: Option<f64>,
    },
    Unknown(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRange {
    unknown: Option<String>,
    min: Option<Bound>,
    max: Option<Bound>,
    page: Option<u32>,
    note: Option<String>,
}

impl TryFrom<RawRange> for Range {
    type Error = String;

    fn try_from(r: RawRange) -> Result<Range, String> {
        match r {
            RawRange {
                unknown: Some(reason),
                min: None,
                max: None,
                page: None,
                note: None,
            } => {
                if reason.trim().is_empty() {
                    Err("`unknown` needs a reason".into())
                } else {
                    Ok(Range::Unknown(reason))
                }
            }
            RawRange { unknown: Some(_), .. } => Err("`unknown` can't be combined with other fields".into()),
            RawRange {
                unknown: None,
                min: Some(min),
                max: Some(max),
                page,
                note,
            } => Ok(Range::Known { min, max, page, note }),
            _ => Err("a range needs both `min` and `max` (use `{ unknown = \"reason\" }` for a missing one)".into()),
        }
    }
}

impl<'de> Deserialize<'de> for Bound {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Bound, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Value(f64),
            Table(RawBound),
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawBound {
            pin: Option<String>,
            plus: Option<f64>,
            at_most: Option<f64>,
            unknown: Option<String>,
        }
        let bad = || {
            serde::de::Error::custom(
                "a bound is a number, `{ pin = \"NAME\", plus = X, at_most = Y }` or `{ unknown = \"reason\" }`",
            )
        };
        match Raw::deserialize(d).map_err(|_| bad())? {
            Raw::Value(v) => Ok(Bound::Value(v)),
            Raw::Table(RawBound {
                pin: Some(pin),
                plus,
                at_most,
                unknown: None,
            }) => Ok(Bound::Pin {
                pin,
                plus: plus.unwrap_or(0.0),
                at_most,
            }),
            Raw::Table(RawBound {
                pin: None,
                plus: None,
                at_most: None,
                unknown: Some(reason),
            }) if !reason.trim().is_empty() => Ok(Bound::Unknown(reason)),
            Raw::Table(_) => Err(bad()),
        }
    }
}

impl Bound {
    pub fn is_known(&self) -> bool {
        !matches!(self, Bound::Unknown(_))
    }

    /// The bound in volts, given a way to look up another pin's voltage. `None` when the bound is unknown
    /// or the other pin's voltage is.
    pub fn resolve(&self, pin_voltage: impl Fn(&str) -> Option<f64>) -> Option<f64> {
        match self {
            Bound::Value(v) => Some(*v),
            Bound::Pin { pin, plus, at_most } => {
                let v = pin_voltage(pin)? + plus;
                Some(at_most.map_or(v, |cap| v.min(cap)))
            }
            Bound::Unknown(_) => None,
        }
    }
}

impl Range {
    /// Whether any bound is known, so the range needs a datasheet page.
    fn has_values(&self) -> bool {
        match self {
            Range::Unknown(_) => false,
            Range::Known { min, max, .. } => min.is_known() || max.is_known(),
        }
    }

    /// Both ends as plain numbers, when they are.
    pub fn values(&self) -> Option<(f64, f64)> {
        match self {
            Range::Known {
                min: Bound::Value(lo),
                max: Bound::Value(hi),
                ..
            } => Some((*lo, *hi)),
            _ => None,
        }
    }
}

impl Part {
    pub fn load(path: &Path) -> Result<Part, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Pin numbers that carry a datasheet pin name.
    pub fn pins_named(&self, name: &str) -> Vec<&str> {
        self.pins
            .iter()
            .filter(|(_, n)| *n == name)
            .map(|(num, _)| num.as_str())
            .collect()
    }

    /// The limits of one quantity that list a pin name in `pins`. Limits where it is only the reference
    /// (`to`) are in [`Part::limits_involving`].
    pub fn limits_for(&self, quantity: Quantity, name: &str) -> impl Iterator<Item = &Limit> {
        let name = name.to_string();
        self.limits
            .iter()
            .filter(move |l| l.quantity == quantity && l.pins.contains(&name))
    }

    /// The limits of one quantity that name a pin in `pins` or as `to`.
    pub fn limits_involving(&self, quantity: Quantity, name: &str) -> impl Iterator<Item = &Limit> {
        let name = name.to_string();
        self.limits
            .iter()
            .filter(move |l| l.quantity == quantity && (l.pins.contains(&name) || l.to.as_ref() == Some(&name)))
    }

    /// Pin names no voltage limit mentions, apart from `NC` and `DNC`. A checker must report these as
    /// UNKNOWN; the coverage report lists them.
    pub fn uncovered_pins(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .pins
            .values()
            .map(String::as_str)
            .filter(|n| !matches!(*n, "NC" | "DNC"))
            .filter(|n| self.limits_involving(Quantity::Voltage, n).next().is_none())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Every pin name a limit refers to, in `pins`, `to` or a relative bound.
    fn used_pin_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for l in &self.limits {
            out.extend(l.pins.iter().chain(&l.to).map(String::as_str));
            for r in [Some(&l.abs_max), Some(&l.recommended), l.breakdown.as_ref()]
                .into_iter()
                .flatten()
            {
                if let Range::Known { min, max, .. } = r {
                    for b in [min, max] {
                        if let Bound::Pin { pin, .. } = b {
                            out.push(pin.as_str());
                        }
                    }
                }
            }
        }
        for d in &self.drives {
            out.extend([d.pin.as_str(), d.to.as_str()]);
        }
        out
    }

    /// Everything wrong with the file that the parser can't catch. Empty when the file is sound.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let names: Vec<&str> = self.pins.values().map(String::as_str).collect();
        if self.pins.is_empty() {
            out.push("no pins".into());
        }
        for v in &self.variants {
            if v.mpn == self.mpn || self.variants.iter().filter(|w| w.mpn == v.mpn).count() > 1 {
                out.push(format!("variant {:?} is listed twice", v.mpn));
            }
            if let Some(pins) = &v.pins {
                let mut missing: Vec<&str> = self
                    .used_pin_names()
                    .into_iter()
                    .filter(|n| !pins.values().any(|p| p == n))
                    .collect();
                missing.sort();
                missing.dedup();
                if !missing.is_empty() {
                    out.push(format!("variant {:?} has no pin named {}", v.mpn, missing.join(", ")));
                }
            }
        }
        for (i, l) in self.limits.iter().enumerate() {
            let at = |what: &str| format!("limit {} ({}): {what}", i + 1, l.pins.join(", "));
            if l.pins.is_empty() {
                out.push(at("no pins"));
            }
            for p in l.pins.iter().chain(&l.to) {
                if !names.contains(&p.as_str()) {
                    out.push(at(&format!("pin {p:?} is not in [pins]")));
                }
            }
            if l.to.as_ref().is_some_and(|t| l.pins.contains(t)) {
                out.push(at("`to` is also listed in `pins`"));
            }
            if l.to.is_none() && l.quantity == Quantity::Voltage && l.pins.len() < 2 {
                out.push(at("a voltage limit without `to` needs at least two pins"));
            }
            if l.to.is_none() && l.quantity == Quantity::Power {
                out.push(at("a power limit needs `to`"));
            }
            if l.bidirectional && l.breakdown.is_none() {
                out.push(at("`bidirectional` only applies to a `breakdown` range"));
            }
            if l.breakdown.is_some() && l.quantity != Quantity::Voltage {
                out.push(at("`breakdown` only applies to voltage limits"));
            }
            let ranges = [
                ("abs_max", Some(&l.abs_max)),
                ("recommended", Some(&l.recommended)),
                ("breakdown", l.breakdown.as_ref()),
            ];
            for (label, r) in ranges {
                let Some(r) = r else { continue };
                if let Range::Known { min, max, page, .. } = r {
                    if r.has_values() && page.is_none() {
                        out.push(at(&format!("{label} has values but no datasheet page")));
                    }
                    if r.has_values() && self.datasheet.is_none() {
                        out.push(at(&format!("{label} has values but the part has no datasheet")));
                    }
                    if page == &Some(0) {
                        out.push(at(&format!("{label}: pages count from 1")));
                    }
                    if let Bound::Pin { at_most: Some(_), .. } = min {
                        out.push(at(&format!("{label}: `at_most` only applies to `max`")));
                    }
                    for b in [min, max] {
                        if let Bound::Pin { pin, .. } = b
                            && !names.contains(&pin.as_str())
                        {
                            out.push(at(&format!("{label} refers to pin {pin:?}, which is not in [pins]")));
                        }
                        if let Bound::Value(v) = b
                            && !v.is_finite()
                        {
                            out.push(at(&format!("{label} has a non-finite value")));
                        }
                    }
                    if let Some((lo, hi)) = r.values()
                        && lo > hi
                    {
                        out.push(at(&format!("{label} min {lo} is above max {hi}")));
                    }
                }
            }
            if let (
                Some(Range::Known {
                    min: Bound::Value(vbr), ..
                }),
                Range::Known {
                    max: Bound::Value(vrwm),
                    ..
                },
            ) = (&l.breakdown, &l.recommended)
                && vrwm >= vbr
            {
                out.push(at(&format!("recommended max {vrwm} is not below breakdown min {vbr}")));
            }
            // Ends are compared where both are plain numbers. With pin-relative ends a checker must use the
            // tighter of `abs_max` and `recommended` at the voltages it works out.
            if let (
                Range::Known {
                    min: amin, max: amax, ..
                },
                Range::Known {
                    min: rmin, max: rmax, ..
                },
            ) = (&l.abs_max, &l.recommended)
            {
                if let (Bound::Value(a), Bound::Value(r)) = (amin, rmin)
                    && r < a
                {
                    out.push(at(&format!("recommended min {r} reaches outside abs_max min {a}")));
                }
                if let (Bound::Value(a), Bound::Value(r)) = (amax, rmax)
                    && r > a
                {
                    out.push(at(&format!("recommended max {r} reaches outside abs_max max {a}")));
                }
            }
        }
        for (i, d) in self.drives.iter().enumerate() {
            let at = |what: &str| format!("drive {} ({}): {what}", i + 1, d.pin);
            for p in [&d.pin, &d.to] {
                if !names.contains(&p.as_str()) {
                    out.push(at(&format!("pin {p:?} is not in [pins]")));
                }
            }
            if let Range::Known { min, max, page, .. } = &d.range {
                if d.range.has_values() && (page.is_none() || self.datasheet.is_none()) {
                    out.push(at("has values but no datasheet page"));
                }
                if [min, max].iter().any(|b| matches!(b, Bound::Pin { .. })) {
                    out.push(at("a drive range must be numbers or unknown"));
                }
                if let Some((lo, hi)) = d.range.values()
                    && lo > hi
                {
                    out.push(at(&format!("min {lo} is above max {hi}")));
                }
            }
        }
        out
    }
}

/// Every part file in one or more directories, keyed by part number (variants included).
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub parts: BTreeMap<String, Part>,
    /// Where each part came from, for error messages.
    pub files: BTreeMap<String, PathBuf>,
}

/// Where a component's limits come from.
#[derive(Debug, Clone)]
pub enum Lookup<'a> {
    /// A part file, from a datasheet.
    Datasheet(&'a Part),
    /// No part file, but the part's class gives a generic limit, such as a capacitor's rated voltage from its
    /// schematic `Voltage` field. Not checked against a datasheet.
    ClassDefault(Box<Part>),
    /// Nothing to check against. Checkers must report the part's pins as unchecked, not as passing.
    Unchecked(String),
}

impl Library {
    /// Loads every `*.toml` file in `dir`. Fails on the first file that doesn't parse, or on two files with
    /// the same part number.
    pub fn load_dir(dir: &Path) -> Result<Library, String> {
        let mut lib = Library::default();
        lib.add_dir(dir)?;
        Ok(lib)
    }

    /// Loads several directories in order. A later directory's file replaces an earlier one with the same
    /// part number, so a board's own `parts/` folder can override the shared library.
    pub fn load_dirs(dirs: &[&Path]) -> Result<Library, String> {
        let mut lib = Library::default();
        for dir in dirs {
            lib.add_dir(dir)?;
        }
        Ok(lib)
    }

    fn add_dir(&mut self, dir: &Path) -> Result<(), String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        let mut here: BTreeMap<String, PathBuf> = BTreeMap::new();
        for path in paths {
            let part = Part::load(&path)?;
            for (mpn, variant) in part.expand() {
                if let Some(prev) = here.get(&mpn) {
                    return Err(format!(
                        "{}: part {mpn:?} is already defined in {}",
                        path.display(),
                        prev.display()
                    ));
                }
                here.insert(mpn.clone(), path.clone());
                self.files.insert(mpn.clone(), path.clone());
                self.parts.insert(mpn, variant);
            }
        }
        Ok(())
    }

    /// The part file a schematic component uses: its `MPN` field, or its value when it has no `MPN`.
    pub fn part_for(&self, c: &Component) -> Option<&Part> {
        self.parts.get(component_mpn(c))
    }

    /// Where a component's limits come from: its part file, a class default, or nowhere.
    pub fn lookup(&self, c: &Component) -> Lookup<'_> {
        if let Some(p) = self.part_for(c) {
            return Lookup::Datasheet(p);
        }
        if let Some(p) = class_default(c) {
            return Lookup::ClassDefault(Box::new(p));
        }
        Lookup::Unchecked(format!("no part file for {:?} and no class default", component_mpn(c)))
    }
}

impl Part {
    /// This part and each variant as parts of their own, keyed by part number.
    fn expand(&self) -> Vec<(String, Part)> {
        let mut base = self.clone();
        base.variants.clear();
        let mut out = vec![(self.mpn.clone(), base.clone())];
        for v in &self.variants {
            let mut p = base.clone();
            p.mpn = v.mpn.clone();
            if let Some(pins) = &v.pins {
                p.pins = pins.clone();
            }
            out.push((v.mpn.clone(), p));
        }
        out
    }
}

/// The part number a component names: its `MPN` field, or its value when it has no `MPN`.
pub fn component_mpn(c: &Component) -> &str {
    match c.fields.get("MPN").map(|s| s.trim()) {
        Some(m) if !m.is_empty() => m,
        _ => c.value.trim(),
    }
}

/// A generic limit from the part's class, for a two-pin capacitor whose rated voltage is in its `Voltage`
/// field or its value (`100nF 50V`). Other classes have no default: a resistor's power or an inductor's
/// current can't be read off the schematic reliably.
pub fn class_default(c: &Component) -> Option<Part> {
    let is_cap = c.lib_id.starts_with("Device:C") || c.lib_id.starts_with("Device:CP");
    if !is_cap || c.pins.len() != 2 {
        return None;
    }
    let (volts, from) = match c.fields.get("Voltage").and_then(|v| parse_volts(v)) {
        Some(v) => (v, "Voltage field"),
        None => (c.value.split_whitespace().find_map(parse_volts)?, "value"),
    };
    let numbers: Vec<String> = c.pins.keys().cloned().collect();
    let polarised = c.lib_id.starts_with("Device:CP");
    let min = if polarised {
        Bound::Unknown("a polarised capacitor's reverse rating is not in the schematic".into())
    } else {
        Bound::Value(-volts)
    };
    Some(Part {
        mpn: component_mpn(c).to_string(),
        manufacturer: String::new(),
        description: format!("class default for {}", c.reference),
        class: Class::Capacitor,
        datasheet: None,
        pins: numbers.iter().map(|n| (n.clone(), n.clone())).collect(),
        variants: Vec::new(),
        limits: vec![Limit {
            quantity: Quantity::Voltage,
            pins: vec![numbers[0].clone()],
            to: Some(numbers[1].clone()),
            bidirectional: false,
            abs_max: Range::Unknown("class default: only the rated voltage is known".into()),
            recommended: Range::Known {
                min,
                max: Bound::Value(volts),
                page: None,
                note: Some(format!(
                    "rated voltage from the schematic's {from}, not checked against a datasheet"
                )),
            },
            breakdown: None,
        }],
        drives: Vec::new(),
    })
}

/// `50V`, `6.3V`, `6V3` or `50 V` as volts.
fn parse_volts(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s
        .strip_suffix(['V', 'v'])
        .map(str::trim_end)
        .map(str::to_string)
        .or_else(|| {
            let (a, b) = s.split_once(['V', 'v'])?;
            (!a.is_empty() && !b.is_empty() && b.chars().all(|c| c.is_ascii_digit())).then(|| format!("{a}.{b}"))
        })?;
    let v: f64 = s.parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v)
}

/// How one placed component is covered by limits.
#[derive(Debug, Clone, PartialEq)]
pub enum Coverage {
    Datasheet {
        mpn: String,
    },
    ClassDefault {
        note: String,
    },
    Unchecked {
        reason: String,
    },
    /// Not fitted, not on the board or not in the BOM.
    NotFitted,
}

/// How every component of a design is covered, keyed by reference.
pub fn coverage(design: &crate::design::Design, lib: &Library) -> BTreeMap<String, Coverage> {
    design
        .components
        .iter()
        .map(|(r, c)| {
            let cov = if c.dnp || !c.in_bom || !c.on_board {
                Coverage::NotFitted
            } else {
                match lib.lookup(c) {
                    Lookup::Datasheet(p) => Coverage::Datasheet { mpn: p.mpn.clone() },
                    Lookup::ClassDefault(p) => Coverage::ClassDefault {
                        note: match &p.limits[0].recommended {
                            Range::Known {
                                note: Some(n),
                                max: Bound::Value(v),
                                ..
                            } => format!("{v} V; {n}"),
                            _ => String::new(),
                        },
                    },
                    Lookup::Unchecked(reason) => Coverage::Unchecked { reason },
                }
            };
            (r.clone(), cov)
        })
        .collect()
}
