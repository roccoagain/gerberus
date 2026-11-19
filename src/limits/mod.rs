pub mod family;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::design::Component;

pub const NOT_READ: &str = "not read from the datasheet yet";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub mpn: String,
    pub manufacturer: String,
    pub description: String,
    pub class: Class,
    pub datasheet: Option<Datasheet>,
    pub tolerance: Option<Tolerance>,
    pub tcr: Option<Tcr>,
    pub temperature: Option<Temperature>,
    pub pins: BTreeMap<String, String>,
    #[serde(default, rename = "variant")]
    pub variants: Vec<Variant>,
    #[serde(default, rename = "limit")]
    pub limits: Vec<Limit>,
    #[serde(default, rename = "drive")]
    pub drives: Vec<Drive>,
    #[serde(default)]
    pub capacitance: Vec<Capacitance>,
    #[serde(default)]
    pub esd: Vec<Esd>,
    pub theta_ja: Option<Rated>,
    pub tj_max: Option<Rated>,
    #[serde(default)]
    pub supply: Vec<SupplyCurrent>,
    pub efficiency: Option<Rated>,
    #[serde(default, rename = "opamp")]
    pub opamps: Vec<OpAmp>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpAmp {
    pub plus: String,
    pub minus: String,
    pub out: String,
    pub vpos: String,
    pub vneg: String,
    pub swing: Option<Rated>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rated {
    pub value: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyCurrent {
    pub pins: Vec<String>,
    pub current: Range,
}

impl SupplyCurrent {
    pub fn max(&self) -> Option<f64> {
        match &self.current {
            Range::Known {
                max: Bound::Value(v), ..
            } => Some(*v),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EsdModel {
    Hbm,
    Cdm,
    IecContact,
    IecAir,
}

impl EsdModel {
    pub fn name(self) -> &'static str {
        match self {
            EsdModel::Hbm => "HBM",
            EsdModel::Cdm => "CDM",
            EsdModel::IecContact => "IEC 61000-4-2 contact",
            EsdModel::IecAir => "IEC 61000-4-2 air",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Esd {
    #[serde(default)]
    pub pins: Vec<String>,
    pub model: EsdModel,
    pub volts: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capacitance {
    pub pins: Vec<String>,
    pub to: String,
    pub typ: Option<f64>,
    pub max: Option<f64>,
    pub page: u32,
    pub note: Option<String>,
}

impl Capacitance {
    pub fn worst(&self) -> Option<f64> {
        self.max.or(self.typ)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tolerance {
    pub value: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tcr {
    pub ppm: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Temperature {
    pub min: f64,
    pub max: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Headroom {
    #[serde(default)]
    pub volts: f64,
    #[serde(default = "one")]
    pub duty: f64,
    pub page: u32,
    pub note: Option<String>,
}

fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drive {
    pub kind: DriveKind,
    pub pin: String,
    pub to: String,
    pub supply: Option<String>,
    #[serde(default)]
    pub feedback: bool,
    pub headroom: Option<Headroom>,
    pub current: Option<f64>,
    pub gate: Option<String>,
    pub on: Option<OnAt>,
    pub range: Range,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnAt {
    pub vgs: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriveKind {
    Regulates,
    Forward,
    Switches,
    Biases,
    Channel,
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
    pub document: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantity {
    #[default]
    Voltage,
    Current,
    Power,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    #[serde(default)]
    pub quantity: Quantity,
    pub pins: Vec<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub bidirectional: bool,
    pub abs_max: Range,
    pub recommended: Range,
    pub breakdown: Option<Range>,
    pub clamp: Option<Clamp>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clamp {
    pub volts: f64,
    pub current: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(try_from = "RawRange")]
pub enum Range {
    Unknown(String),
    Known {
        min: Bound,
        max: Bound,
        page: Option<u32>,
        note: Option<String>,
        derived: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Bound {
    Value(f64),
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
    derived: Option<String>,
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
                derived: None,
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
                derived,
            } => Ok(Range::Known {
                min,
                max,
                page,
                note,
                derived,
            }),
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
    fn has_values(&self) -> bool {
        match self {
            Range::Unknown(_) => false,
            Range::Known { min, max, .. } => min.is_known() || max.is_known(),
        }
    }

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

    pub fn pins_named(&self, name: &str) -> Vec<&str> {
        self.pins
            .iter()
            .filter(|(_, n)| *n == name)
            .map(|(num, _)| num.as_str())
            .collect()
    }

    pub fn limits_for(&self, quantity: Quantity, name: &str) -> impl Iterator<Item = &Limit> {
        let name = name.to_string();
        self.limits
            .iter()
            .filter(move |l| l.quantity == quantity && l.pins.contains(&name))
    }

    pub fn limits_involving(&self, quantity: Quantity, name: &str) -> impl Iterator<Item = &Limit> {
        let name = name.to_string();
        self.limits
            .iter()
            .filter(move |l| l.quantity == quantity && (l.pins.contains(&name) || l.to.as_ref() == Some(&name)))
    }

    pub fn esd_for(&self, name: &str) -> impl Iterator<Item = &Esd> {
        let name = name.to_string();
        self.esd
            .iter()
            .filter(move |e| e.pins.is_empty() || e.pins.contains(&name))
    }

    pub fn unread(&self) -> bool {
        let unread = |r: &Range| matches!(r, Range::Unknown(why) if why == NOT_READ);
        self.limits
            .iter()
            .any(|l| unread(&l.abs_max) || unread(&l.recommended) || l.breakdown.as_ref().is_some_and(unread))
            || self.drives.iter().any(|d| unread(&d.range))
    }

    pub fn uncovered_pins(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .pins
            .values()
            .map(String::as_str)
            .filter(|n| !matches!(*n, "NC" | "DNC"))
            .filter(|n| self.limits_involving(Quantity::Voltage, n).next().is_none())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

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
        for a in &self.opamps {
            out.extend([
                a.plus.as_str(),
                a.minus.as_str(),
                a.out.as_str(),
                a.vpos.as_str(),
                a.vneg.as_str(),
            ]);
        }
        for d in &self.drives {
            out.extend([d.pin.as_str(), d.to.as_str()]);
            out.extend(d.supply.as_deref());
            out.extend(d.gate.as_deref());
        }
        out
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let names: Vec<&str> = self.pins.values().map(String::as_str).collect();
        if self.pins.is_empty() {
            out.push("no pins".into());
        }
        if let Some(tol) = &self.tolerance
            && (!(tol.value > 0.0 && tol.value < 1.0) || self.datasheet.is_none() || tol.page == 0)
        {
            out.push(format!(
                "tolerance {} needs a datasheet page and must be between 0 and 1",
                tol.value
            ));
        }
        if let Some(t) = &self.tcr
            && (!(t.ppm >= 0.0 && t.ppm.is_finite()) || self.datasheet.is_none() || t.page == 0)
        {
            out.push(format!("tcr {} needs a datasheet page and must be at least 0", t.ppm));
        }
        if let Some(t) = &self.temperature
            && (t.min >= t.max || t.min.is_nan() || t.max.is_nan() || self.datasheet.is_none() || t.page == 0)
        {
            out.push("temperature needs a datasheet page and min below max".into());
        }
        if let Some(e) = &self.efficiency
            && (!(e.value > 0.0 && e.value <= 1.0) || self.datasheet.is_none() || e.page == 0)
        {
            out.push(format!(
                "efficiency {} needs a datasheet page and must be in (0, 1]",
                e.value
            ));
        }
        for (what, r) in [("theta_ja", &self.theta_ja), ("tj_max", &self.tj_max)] {
            if let Some(r) = r
                && (!(r.value > 0.0 && r.value.is_finite()) || self.datasheet.is_none() || r.page == 0)
            {
                out.push(format!("{what} {} needs a datasheet page and must be above 0", r.value));
            }
        }
        for (i, s) in self.supply.iter().enumerate() {
            let page = matches!(&s.current, Range::Known { page: Some(p), .. } if *p > 0);
            if s.pins.is_empty()
                || !s.max().is_some_and(|m| m >= 0.0 && m.is_finite())
                || self.datasheet.is_none()
                || !page
            {
                out.push(format!(
                    "supply {}: needs pins, a datasheet page and a numeric max of at least 0",
                    i + 1
                ));
            }
            for p in &s.pins {
                if !names.contains(&p.as_str()) {
                    out.push(format!("supply {}: pin {p:?} is not in [pins]", i + 1));
                }
            }
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
                missing.sort_unstable();
                missing.dedup();
                if !missing.is_empty() {
                    out.push(format!("variant {:?} has no pin named {}", v.mpn, missing.join(", ")));
                }
            }
        }
        for (i, c) in self.capacitance.iter().enumerate() {
            let at = |what: &str| format!("capacitance {} ({}): {what}", i + 1, c.pins.join(", "));
            for p in c.pins.iter().chain([&c.to]) {
                if !names.contains(&p.as_str()) {
                    out.push(at(&format!("pin {p:?} is not in [pins]")));
                }
            }
            let ok = |v: Option<f64>| v.is_none_or(|v| v.is_finite() && v >= 0.0);
            if c.worst().is_none() || !ok(c.typ) || !ok(c.max) || c.page == 0 || self.datasheet.is_none() {
                out.push(at("needs typ or max in pF (0 or more) and a datasheet page"));
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
            if let Some(c) = &l.clamp {
                match &l.breakdown {
                    None => out.push(at("`clamp` needs a `breakdown` range")),
                    Some(Range::Known {
                        max: Bound::Value(b), ..
                    }) if c.volts <= *b => {
                        out.push(at(&format!("clamp {} V is not above breakdown max {b} V", c.volts)));
                    }
                    _ => {}
                }
                if c.current.is_nan() || c.current <= 0.0 || c.page == 0 {
                    out.push(at("`clamp` needs a positive current and a 1-based page"));
                }
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
        for (i, e) in self.esd.iter().enumerate() {
            let at = |what: &str| format!("esd {} ({}): {what}", i + 1, e.model.name());
            if !(e.volts > 0.0 && e.volts.is_finite()) || e.page == 0 || self.datasheet.is_none() {
                out.push(at("needs volts above 0 and a datasheet page"));
            }
            for p in &e.pins {
                if !names.contains(&p.as_str()) {
                    out.push(at(&format!("pin {p:?} is not in [pins]")));
                }
            }
        }
        for (i, a) in self.opamps.iter().enumerate() {
            let at = |what: &str| format!("opamp {} ({}): {what}", i + 1, a.out);
            for p in [&a.plus, &a.minus, &a.out, &a.vpos, &a.vneg] {
                if !names.contains(&p.as_str()) {
                    out.push(at(&format!("pin {p:?} is not in [pins]")));
                }
            }
            if let Some(s) = &a.swing
                && (!(s.value >= 0.0 && s.value.is_finite()) || s.page == 0 || self.datasheet.is_none())
            {
                out.push(at("swing needs a datasheet page and volts of at least 0"));
            }
        }
        for (i, d) in self.drives.iter().enumerate() {
            let at = |what: &str| format!("drive {} ({}): {what}", i + 1, d.pin);
            if !matches!(d.kind, DriveKind::Forward | DriveKind::Channel) && d.supply.is_none() {
                out.push(at("a regulator, switch node or biased input needs `supply`"));
            }
            if d.kind != DriveKind::Regulates && (d.feedback || d.headroom.is_some()) {
                out.push(at("only a regulator has `feedback` or `headroom`"));
            }
            if d.kind == DriveKind::Channel {
                if d.gate.is_none() {
                    out.push(at("a MOSFET channel needs `gate`"));
                }
                if let Some(o) = &d.on {
                    if o.page == 0 || self.datasheet.is_none() || !o.vgs.is_finite() || o.vgs == 0.0 {
                        out.push(at("`on` needs a datasheet page and a VGS other than 0"));
                    }
                    if let Some((lo, hi)) = d.range.values()
                        && !((o.vgs > 0.0 && lo > 0.0 && o.vgs >= hi) || (o.vgs < 0.0 && hi < 0.0 && o.vgs <= lo))
                    {
                        out.push(at(
                            "`on` must be past the whole threshold range, with the same sign (N-channel above, P-channel below)",
                        ));
                    }
                }
            } else if d.gate.is_some() || d.on.is_some() {
                out.push(at("only a MOSFET channel has `gate` or `on`"));
            }
            if let Some(h) = &d.headroom
                && (h.page == 0 || self.datasheet.is_none() || !(h.duty > 0.0 && h.duty <= 1.0) || h.volts < 0.0)
            {
                out.push(at(
                    "headroom needs a datasheet page, a duty in (0, 1] and volts of at least 0",
                ));
            }
            for p in [&d.pin, &d.to].into_iter().chain(&d.supply).chain(&d.gate) {
                if !names.contains(&p.as_str()) {
                    out.push(at(&format!("pin {p:?} is not in [pins]")));
                }
            }
            if let Range::Known { min, max, page, .. } = &d.range {
                if d.range.has_values() && (page.is_none() || self.datasheet.is_none()) {
                    out.push(at("has values but no datasheet page"));
                }
                if !matches!(d.kind, DriveKind::Switches | DriveKind::Biases)
                    && [min, max].iter().any(|b| matches!(b, Bound::Pin { .. }))
                {
                    out.push(at(
                        "only a switch node's or biased input's range can be relative to a pin",
                    ));
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

const BUILTIN_PARTS: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/builtin_parts.rs"));

pub fn builtin_text(path: &Path) -> Option<&'static str> {
    let name = path.to_str()?.strip_prefix("parts/")?.strip_suffix(" (built in)")?;
    BUILTIN_PARTS.iter().find(|(n, _)| *n == name).map(|(_, t)| *t)
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    pub parts: BTreeMap<String, Part>,
    pub files: BTreeMap<String, PathBuf>,
    pub families: Vec<(PathBuf, family::Family)>,
    pub family_errors: BTreeMap<String, String>,
}

enum Entry {
    Part(Box<Part>),
    Family(Box<family::Family>),
}

fn parse_entry(path: &Path, text: &str) -> Result<Entry, String> {
    let at = |e: String| format!("{}: {e}", path.display());
    let table: toml::Table = toml::from_str(text).map_err(|e| at(e.to_string()))?;
    if table.contains_key("family") {
        let f: family::Family = toml::from_str(text).map_err(|e| at(e.to_string()))?;
        let problems = f.problems();
        if !problems.is_empty() {
            return Err(at(problems.join("; ")));
        }
        Ok(Entry::Family(Box::new(f)))
    } else {
        toml::from_str(text)
            .map(|p| Entry::Part(Box::new(p)))
            .map_err(|e| at(e.to_string()))
    }
}

#[derive(Debug, Clone)]
pub enum Lookup<'a> {
    Datasheet(&'a Part),
    ClassDefault(Box<Part>),
    Unchecked(String),
}

impl Library {
    pub fn load_dir(dir: &Path) -> Result<Library, String> {
        let mut lib = Library::default();
        lib.add_dir(dir)?;
        Ok(lib)
    }

    pub fn load_dirs(dirs: &[&Path]) -> Result<Library, String> {
        let mut lib = Library::default();
        for dir in dirs {
            lib.add_dir(dir)?;
        }
        Ok(lib)
    }

    pub fn builtin() -> Result<Library, String> {
        let mut lib = Library::default();
        let parts = BUILTIN_PARTS.iter().map(|(name, text)| {
            let path = PathBuf::from(format!("parts/{name} (built in)"));
            let part = parse_entry(&path, text);
            (path, part)
        });
        lib.add_parts(parts)?;
        Ok(lib)
    }

    pub fn add_dir(&mut self, dir: &Path) -> Result<(), String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        self.add_parts(paths.into_iter().map(|path| {
            let entry = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|text| parse_entry(&path, &text));
            (path, entry)
        }))
    }

    fn add_parts(&mut self, parts: impl Iterator<Item = (PathBuf, Result<Entry, String>)>) -> Result<(), String> {
        let mut here: BTreeMap<String, PathBuf> = BTreeMap::new();
        for (path, entry) in parts {
            let part = match entry? {
                Entry::Part(p) => *p,
                Entry::Family(f) => {
                    self.families.push((path, *f));
                    continue;
                }
            };
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

    pub fn expand_families(&mut self, design: &crate::design::Design) {
        for c in design.components.values() {
            let mpn = component_mpn(c);
            if self.parts.contains_key(mpn) || self.family_errors.contains_key(mpn) {
                continue;
            }
            let found = self
                .families
                .iter()
                .rev()
                .find_map(|(path, f)| f.part(mpn).map(|r| (path.clone(), r)));
            match found {
                Some((path, Ok(part))) => {
                    let problems = part.problems();
                    if problems.is_empty() {
                        self.files.insert(mpn.to_string(), path);
                        self.parts.insert(mpn.to_string(), part);
                    } else {
                        self.family_errors
                            .insert(mpn.to_string(), format!("{}: {}", path.display(), problems.join("; ")));
                    }
                }
                Some((path, Err(e))) => {
                    self.family_errors
                        .insert(mpn.to_string(), format!("{}: {e}", path.display()));
                }
                None => {}
            }
        }
    }

    pub fn part_for(&self, c: &Component) -> Option<&Part> {
        self.parts.get(component_mpn(c))
    }

    pub fn lookup(&self, c: &Component) -> Lookup<'_> {
        if let Some(p) = self.part_for(c) {
            return Lookup::Datasheet(p);
        }
        if let Some(p) = class_default(c) {
            return Lookup::ClassDefault(Box::new(p));
        }
        let mpn = component_mpn(c);
        match self.family_errors.get(mpn) {
            Some(e) => Lookup::Unchecked(format!("its part family can't rate {mpn:?}: {e}")),
            None => Lookup::Unchecked(format!("no part file for {mpn:?} and no class default")),
        }
    }
}

impl Part {
    fn expand(&self) -> Vec<(String, Part)> {
        let mut base = self.clone();
        base.variants.clear();
        let mut out = vec![(self.mpn.clone(), base.clone())];
        for v in &self.variants {
            let mut p = base.clone();
            p.mpn.clone_from(&v.mpn);
            if let Some(pins) = &v.pins {
                p.pins = pins.clone();
            }
            out.push((v.mpn.clone(), p));
        }
        out
    }
}

pub fn component_mpn(c: &Component) -> &str {
    match c.fields.get("MPN").map(|s| s.trim()) {
        Some(m) if !m.is_empty() => m,
        _ => c.value.trim(),
    }
}

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
        tolerance: None,
        tcr: None,
        temperature: None,
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
                derived: None,
            },
            breakdown: None,
            clamp: None,
        }],
        drives: Vec::new(),
        capacitance: Vec::new(),
        esd: Vec::new(),
        theta_ja: None,
        tj_max: None,
        supply: Vec::new(),
        efficiency: None,
        opamps: vec![],
    })
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum Coverage {
    Datasheet,
    ClassDefault,
    Unchecked { reason: String },
    NotFitted,
    NonElectrical,
}

pub fn is_non_electrical(reference: &str, c: &Component) -> bool {
    let letters: String = reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let symbol = c.lib_id.rsplit(':').next().unwrap_or("");
    let test_point = symbol.starts_with("TestPoint") || (letters == "TP" && c.pins.len() <= 1);
    let net_tie = symbol.starts_with("NetTie") || (letters == "NT" && c.pins.len() == 2);
    test_point || net_tie
}

pub fn coverage(design: &crate::design::Design, lib: &Library) -> BTreeMap<String, Coverage> {
    design
        .components
        .iter()
        .map(|(r, c)| {
            let cov = if c.dnp || !c.in_bom || !c.on_board {
                Coverage::NotFitted
            } else {
                match lib.lookup(c) {
                    Lookup::Datasheet(_) => Coverage::Datasheet,
                    Lookup::ClassDefault(_) => Coverage::ClassDefault,
                    Lookup::Unchecked(_) if is_non_electrical(r, c) => Coverage::NonElectrical,
                    Lookup::Unchecked(reason) => Coverage::Unchecked { reason },
                }
            };
            (r.clone(), cov)
        })
        .collect()
}
