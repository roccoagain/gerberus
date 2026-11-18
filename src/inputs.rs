use std::path::Path;

use serde::Deserialize;

use crate::design::Design;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardInputs {
    pub board: String,
    #[serde(default, rename = "input")]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub undriven: Vec<Undriven>,
    #[serde(default)]
    pub differential: Vec<Differential>,
    pub temperature: Option<BoardTemperature>,
    #[serde(default, rename = "signal")]
    pub signals: Vec<Signal>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    pub name: String,
    pub pin: PinRef,
    pub to: PinRef,
    pub peak: f64,
    #[serde(default = "audio_band")]
    pub frequency: [f64; 2],
    #[serde(default)]
    pub ac_coupled: bool,
    pub source: String,
}

fn audio_band() -> [f64; 2] {
    [20.0, 20e3]
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Undriven {
    pub pin: PinRef,
    pub source: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Differential {
    pub pin: PinRef,
    pub to: PinRef,
    pub min: f64,
    pub max: f64,
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
    pub name: String,
    pub pin: PinRef,
    pub to: PinRef,
    pub min: f64,
    pub max: Max,
    pub source: String,
    pub fault: Option<String>,
    pub reverse: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Max {
    Value(f64),
    Pin(Follow),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follow {
    pub pin: PinRef,
    #[serde(default)]
    pub plus: f64,
}

impl Max {
    pub fn value(&self) -> Option<f64> {
        match self {
            Max::Value(v) => Some(*v),
            Max::Pin(_) => None,
        }
    }
}

impl From<f64> for Max {
    fn from(v: f64) -> Max {
        Max::Value(v)
    }
}

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
    pub fn none(board: &str, design: &Design) -> BoardInputs {
        let ground = design.nets.iter().find(|n| {
            let name = n.name.rsplit('/').next().unwrap_or("").to_ascii_uppercase();
            matches!(name.as_str(), "GND" | "0V" | "VSS") && !n.pins.is_empty()
        });
        let inputs = ground
            .and_then(|n| n.pins.iter().min())
            .map(|(r, p)| {
                let pin = PinRef {
                    reference: r.clone(),
                    pin: p.clone(),
                };
                Input {
                    name: "ground".into(),
                    pin: pin.clone(),
                    to: pin,
                    min: 0.0,
                    max: Max::Value(0.0),
                    source: format!(
                        "assumed: there is no inputs.toml, and {} is named as a ground",
                        ground.map_or("", |n| n.name.as_str())
                    ),
                    fault: None,
                    reverse: Some("an assumed ground, not a supply".into()),
                }
            })
            .into_iter()
            .collect();
        BoardInputs {
            board: board.to_string(),
            inputs,
            undriven: vec![],
            differential: vec![],
            temperature: None,
            signals: vec![],
        }
    }

    pub fn load(path: &Path) -> Result<BoardInputs, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn faults(&self) -> Vec<&str> {
        let mut out: Vec<&str> = vec![];
        for f in self.inputs.iter().filter_map(|i| i.fault.as_deref()) {
            if !out.contains(&f) {
                out.push(f);
            }
        }
        out
    }

    pub fn reversible(&self) -> Vec<&Input> {
        self.inputs
            .iter()
            .filter(|i| i.fault.is_none() && i.reverse.is_none() && i.min > 0.0)
            .filter(|i| i.max.value().is_some())
            .collect()
    }

    pub fn reversed_case(input: &Input) -> String {
        format!("{} reversed", input.name)
    }

    pub fn with_reversals(&self) -> BoardInputs {
        let mut out = self.clone();
        for i in self.reversible() {
            let case = Self::reversed_case(i);
            if self.inputs.iter().any(|j| j.fault.as_deref() == Some(case.as_str())) {
                continue;
            }
            out.inputs.push(Input {
                name: case.clone(),
                min: -i.max.value().unwrap_or(i.min),
                max: Max::Value(-i.min),
                source: format!("{} connected backwards ({})", i.name, i.source),
                fault: Some(case),
                ..i.clone()
            });
        }
        out
    }

    pub fn case(&self, design: &Design, fault: &str) -> BoardInputs {
        let net = |p: &PinRef| design.pin_net(&p.reference, &p.pin).map(|n| n.name.clone());
        let ours: Vec<&Input> = self
            .inputs
            .iter()
            .filter(|i| i.fault.as_deref() == Some(fault))
            .collect();
        let taken: Vec<Option<String>> = ours.iter().map(|i| net(&i.pin)).collect();
        let mut inputs: Vec<Input> = self
            .inputs
            .iter()
            .filter(|i| i.fault.is_none() && !taken.contains(&net(&i.pin)))
            .cloned()
            .collect();
        inputs.extend(ours.into_iter().map(|i| Input {
            fault: None,
            ..i.clone()
        }));
        let differential = self
            .differential
            .iter()
            .filter(|d| !taken.contains(&net(&d.pin)) && !taken.contains(&net(&d.to)))
            .cloned()
            .collect();
        BoardInputs {
            inputs,
            differential,
            ..self.clone()
        }
    }

    pub fn problems(&self, design: &Design) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.signals {
            for p in [&s.pin, &s.to] {
                if design.pin_net(&p.reference, &p.pin).is_none() {
                    out.push(format!("signal {}: pin {p} is not in the design", s.name));
                }
            }
            if !(s.peak > 0.0 && s.peak.is_finite()) {
                out.push(format!("signal {}: peak must be above 0 V", s.name));
            }
            if !(s.frequency[0] > 0.0 && s.frequency[0] < s.frequency[1] && s.frequency[1].is_finite()) {
                out.push(format!(
                    "signal {}: frequency must be [low, high] in Hz, low above 0",
                    s.name
                ));
            }
        }
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
            let ground_reference = i.pin == i.to && i.min == 0.0 && i.max == Max::Value(0.0);
            if let [Some(a), Some(b)] = nets.as_slice()
                && a == b
                && !ground_reference
            {
                out.push(format!("{}: {} and {} are on the same net, {a}", i.name, i.pin, i.to));
            }
            match &i.max {
                Max::Value(max) => {
                    if !(i.min.is_finite() && max.is_finite()) || i.min > *max {
                        out.push(format!("{}: min {} and max {max} are not a range", i.name, i.min));
                    }
                }
                Max::Pin(Follow { pin, plus }) => {
                    if !i.min.is_finite() || !plus.is_finite() {
                        out.push(format!("{}: min {} and plus {plus} must be numbers", i.name, i.min));
                    }
                    match design.pin_net(&pin.reference, &pin.pin) {
                        None => out.push(format!("{}: max follows pin {pin}, which is not in the design", i.name)),
                        Some(n) if nets.first().cloned().flatten().as_deref() == Some(n.name.as_str()) => out.push(
                            format!("{}: max follows {pin}, which is on the input's own net", i.name),
                        ),
                        Some(_) => {}
                    }
                }
            }
            if i.source.trim().is_empty() {
                out.push(format!("{}: no source", i.name));
            }
            if i.fault.as_deref().is_some_and(|f| f.trim().is_empty()) {
                out.push(format!("{}: an empty fault name", i.name));
            }
        }
        for u in &self.undriven {
            if design.pin_net(&u.pin.reference, &u.pin.pin).is_none() {
                out.push(format!("undriven {}: the pin is not in the design", u.pin));
            }
            if self.inputs.iter().any(|i| i.pin == u.pin && i.fault.is_none()) {
                out.push(format!("undriven {}: an input also drives it", u.pin));
            }
            if u.source.trim().is_empty() {
                out.push(format!("undriven {}: no source", u.pin));
            }
        }
        let driven: Vec<Option<&str>> = self
            .inputs
            .iter()
            .filter(|i| i.fault.is_none())
            .map(|i| design.pin_net(&i.pin.reference, &i.pin.pin).map(|n| n.name.as_str()))
            .collect();
        for d in &self.differential {
            let nets: Vec<Option<&str>> = [&d.pin, &d.to]
                .into_iter()
                .map(|p| design.pin_net(&p.reference, &p.pin).map(|n| n.name.as_str()))
                .collect();
            if nets.iter().any(Option::is_none) {
                out.push(format!(
                    "differential {} to {}: a pin is not in the design",
                    d.pin, d.to
                ));
            } else if nets[0] == nets[1] {
                out.push(format!("differential {} to {}: both pins are on one net", d.pin, d.to));
            } else if nets.iter().any(|n| !driven.contains(n)) {
                out.push(format!(
                    "differential {} to {}: both pins must be on nets a normal input drives",
                    d.pin, d.to
                ));
            }
            if !(d.min.is_finite() && d.max.is_finite()) || d.min > d.max {
                out.push(format!(
                    "differential {} to {}: min {} and max {} are not a range",
                    d.pin, d.to, d.min, d.max
                ));
            }
            if d.source.trim().is_empty() {
                out.push(format!("differential {} to {}: no source", d.pin, d.to));
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
