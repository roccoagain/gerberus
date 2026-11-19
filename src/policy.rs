use std::path::Path;

use serde::Deserialize;

use crate::limits::{Class, Limit, Part, Quantity};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default, rename = "derate")]
    pub derates: Vec<Derate>,
    pub esd: Option<EsdLevel>,
    pub junction: Option<JunctionMargin>,
    pub spice: Option<SpicePolicy>,
    pub traces: Option<TracePolicy>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JunctionMargin {
    pub margin: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EsdLevel {
    pub contact: f64,
    pub air: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derate {
    pub class: Option<Class>,
    pub mpn: Option<String>,
    pub quantity: Quantity,
    pub factor: f64,
    pub reason: String,
}

impl Policy {
    pub fn load(path: &Path) -> Result<Policy, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Policy::parse(&text, &path.display().to_string())
    }

    pub fn builtin() -> Result<Policy, String> {
        Policy::parse(include_str!("../policy.toml"), "policy.toml (built in)")
    }

    pub fn parse(text: &str, origin: &str) -> Result<Policy, String> {
        let policy: Policy = toml::from_str(text).map_err(|e| format!("{origin}: {e}"))?;
        let problems = policy.problems();
        if problems.is_empty() {
            Ok(policy)
        } else {
            Err(format!("{origin}: {}", problems.join("; ")))
        }
    }

    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (i, d) in self.derates.iter().enumerate() {
            if d.class.is_none() == d.mpn.is_none() {
                out.push(format!("derate {}: give exactly one of `class` and `mpn`", i + 1));
            }
            if !(d.factor > 0.0 && d.factor <= 1.0) {
                out.push(format!("derate {}: factor {} is not in (0, 1]", i + 1, d.factor));
            }
        }
        out
    }

    pub fn derate_for(&self, part: &Part, limit: &Limit) -> Option<&Derate> {
        let matching = |d: &&Derate| d.quantity == limit.quantity;
        self.derates
            .iter()
            .filter(matching)
            .find(|d| d.mpn.as_deref() == Some(part.mpn.as_str()))
            .or_else(|| {
                self.derates
                    .iter()
                    .filter(matching)
                    .find(|d| d.class == Some(part.class))
            })
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpicePolicy {
    pub cable_inductance: [f64; 2],
    pub cable_resistance: f64,
    pub rise_time: f64,
    pub duration: f64,
    pub runs: usize,
    pub reason: String,
}

impl Default for SpicePolicy {
    fn default() -> SpicePolicy {
        SpicePolicy {
            cable_inductance: [0.1e-6, 1e-6],
            cable_resistance: 0.1,
            rise_time: 1e-6,
            duration: 2e-3,
            runs: 16,
            reason: "a hot plug through about a metre of cable".into(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePolicy {
    pub rise: f64,
    pub reason: String,
}
