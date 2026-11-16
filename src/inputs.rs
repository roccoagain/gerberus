//! Board inputs: the voltages the outside world puts on a board's connector pins, such as `J1.1` at 7 to
//! 26 V from `J1.4`. A checker starts every net's voltage range from these.
//!
//! The file is TOML and sits next to the board's schematic. See `parts/README.md` for the format.

use std::path::Path;

use serde::Deserialize;

use crate::design::Design;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardInputs {
    /// Which board this describes, for messages.
    pub board: String,
    #[serde(default, rename = "input")]
    pub inputs: Vec<Input>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// A short label, such as `VBUS`.
    pub name: String,
    /// The pin the voltage is applied to, as `REF.PIN`, such as `J1.1`.
    pub pin: PinRef,
    /// The pin it is measured from, usually the connector's ground pin.
    pub to: PinRef,
    /// Volts.
    pub min: f64,
    pub max: f64,
    /// Where the range comes from: a design document, a spec, a battery's chemistry.
    pub source: String,
}

/// A part pin written as `REF.PIN`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct PinRef {
    pub reference: String,
    pub pin: String,
}

impl TryFrom<String> for PinRef {
    type Error = String;

    fn try_from(s: String) -> Result<PinRef, String> {
        match s.split_once('.') {
            Some((r, p)) if !r.is_empty() && !p.is_empty() => Ok(PinRef {
                reference: r.to_string(),
                pin: p.to_string(),
            }),
            _ => Err(format!("{s:?} is not a pin; write it as REF.PIN, such as J1.1")),
        }
    }
}

impl std::fmt::Display for PinRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.reference, self.pin)
    }
}

impl BoardInputs {
    pub fn load(path: &Path) -> Result<BoardInputs, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Everything that doesn't match the design or doesn't make sense. Empty when the file is sound.
    pub fn problems(&self, design: &Design) -> Vec<String> {
        let mut out = Vec::new();
        for i in &self.inputs {
            for p in [&i.pin, &i.to] {
                if design.pin_net(&p.reference, &p.pin).is_none() {
                    out.push(format!("{}: pin {p} is not in the design", i.name));
                }
            }
            if i.pin == i.to {
                out.push(format!("{}: `pin` and `to` are the same pin", i.name));
            }
            if !(i.min.is_finite() && i.max.is_finite()) || i.min > i.max {
                out.push(format!("{}: min {} and max {} are not a range", i.name, i.min, i.max));
            }
            if i.source.trim().is_empty() {
                out.push(format!("{}: no source", i.name));
            }
        }
        out
    }
}
