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
    /// Missing only when no datasheet could be found; every limit must then be unknown.
    pub datasheet: Option<Datasheet>,
    /// Pin number (as printed in the datasheet and used by the footprint) to datasheet pin name. Several
    /// numbers may share a name, such as two `GND` pads.
    pub pins: BTreeMap<String, String>,
    #[serde(default, rename = "limit")]
    pub limits: Vec<Limit>,
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
    /// Relative to another pin of the same part: `V(pin) + plus`, capped at `at_most` when given.
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

    /// The limits of one quantity that apply to a pin name, either as one of `pins` or as the reference.
    pub fn limits_for(&self, quantity: Quantity, name: &str) -> impl Iterator<Item = &Limit> {
        let name = name.to_string();
        self.limits
            .iter()
            .filter(move |l| l.quantity == quantity && l.pins.contains(&name))
    }

    /// Everything wrong with the file that the parser can't catch. Empty when the file is sound.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let names: Vec<&str> = self.pins.values().map(String::as_str).collect();
        if self.pins.is_empty() {
            out.push("no pins".into());
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
            if let (Some((alo, ahi)), Some((rlo, rhi))) = (l.abs_max.values(), l.recommended.values())
                && (rlo < alo || rhi > ahi)
            {
                out.push(at(&format!(
                    "recommended {rlo}..{rhi} reaches outside abs_max {alo}..{ahi}"
                )));
            }
        }
        out
    }
}

/// Every part file in a directory, keyed by part number.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub parts: BTreeMap<String, Part>,
    /// Where each part came from, for error messages.
    pub files: BTreeMap<String, PathBuf>,
}

impl Library {
    /// Loads every `*.toml` file in `dir`. Fails on the first file that doesn't parse, or on two files with
    /// the same part number.
    pub fn load_dir(dir: &Path) -> Result<Library, String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        let mut lib = Library::default();
        for path in paths {
            let part = Part::load(&path)?;
            if let Some(prev) = lib.files.get(&part.mpn) {
                return Err(format!(
                    "{}: part {:?} is already defined in {}",
                    path.display(),
                    part.mpn,
                    prev.display()
                ));
            }
            lib.files.insert(part.mpn.clone(), path);
            lib.parts.insert(part.mpn.clone(), part);
        }
        Ok(lib)
    }

    /// The part a schematic component uses: its `MPN` field, or its value when it has no `MPN`.
    pub fn part_for(&self, c: &Component) -> Option<&Part> {
        self.parts.get(component_mpn(c))
    }
}

/// The part number a component names: its `MPN` field, or its value when it has no `MPN`.
pub fn component_mpn(c: &Component) -> &str {
    match c.fields.get("MPN").map(|s| s.trim()) {
        Some(m) if !m.is_empty() => m,
        _ => c.value.trim(),
    }
}
