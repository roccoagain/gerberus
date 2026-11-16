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
    /// Connector pins the outside world doesn't drive, such as a debug probe's target-voltage sense pin.
    #[serde(default)]
    pub undriven: Vec<Undriven>,
    /// The ambient temperature range the board works in. Without it, parts are taken over their own rated
    /// range.
    pub temperature: Option<BoardTemperature>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Undriven {
    pub pin: PinRef,
    /// Why nothing outside drives it.
    pub source: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardTemperature {
    pub min: f64,
    pub max: f64,
    pub source: String,
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
        for (n, i) in self.inputs.iter().enumerate() {
            if self.inputs[..n].iter().any(|j| j.name == i.name) {
                out.push(format!("{}: the name is used twice", i.name));
            }
            let nets: Vec<_> = [&i.pin, &i.to]
                .into_iter()
                .map(|p| {
                    let net = design.pin_net(&p.reference, &p.pin);
                    if net.is_none() {
                        out.push(format!("{}: pin {p} is not in the design", i.name));
                    }
                    net.map(|n| n.name.clone())
                })
                .collect();
            if let [Some(a), Some(b)] = nets.as_slice()
                && a == b
            {
                out.push(format!("{}: {} and {} are on the same net, {a}", i.name, i.pin, i.to));
            }
            if !(i.min.is_finite() && i.max.is_finite()) || i.min > i.max {
                out.push(format!("{}: min {} and max {} are not a range", i.name, i.min, i.max));
            }
            if i.source.trim().is_empty() {
                out.push(format!("{}: no source", i.name));
            }
        }
        for u in &self.undriven {
            if design.pin_net(&u.pin.reference, &u.pin.pin).is_none() {
                out.push(format!("undriven {}: the pin is not in the design", u.pin));
            }
            if self.inputs.iter().any(|i| i.pin == u.pin) {
                out.push(format!("undriven {}: an input also drives it", u.pin));
            }
            if u.source.trim().is_empty() {
                out.push(format!("undriven {}: no source", u.pin));
            }
        }
        if let Some(t) = &self.temperature
            && (t.min >= t.max || t.min.is_nan() || t.max.is_nan() || t.source.trim().is_empty())
        {
            out.push("temperature: needs min below max and a source".into());
        }
        out
    }
}
