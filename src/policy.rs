//! Derating policy: how far below a datasheet rating a design should stay. This is project judgement, not
//! datasheet fact, so it lives in its own file (`policy.toml`) and never in `parts/`.
use std::path::Path;

use serde::Deserialize;

use crate::limits::{Class, Limit, Part, Quantity};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default, rename = "derate")]
    pub derates: Vec<Derate>,
    /// The ESD level a pin on an external connector must survive without a clamp.
    pub esd: Option<EsdLevel>,
    /// How far below its maximum a junction is kept.
    pub junction: Option<JunctionMargin>,
    /// The cable and sweep for `gerberus stress`.
    pub spice: Option<SpicePolicy>,
    /// The temperature rise a track may have, for `gerberus traces`.
    pub traces: Option<TracePolicy>,
    /// How near a power pin its decoupling capacitor must be, for `gerberus traces`.
    pub decoupling: Option<DecouplingPolicy>,
    /// The largest input loop a switching regulator may have, for `gerberus traces`.
    pub hot_loop: Option<HotLoopPolicy>,
    /// How fast a digital output's edge is taken to be, for `gerberus traces`.
    pub edges: Option<EdgePolicy>,
}

/// Keep every junction `margin` °C below its maximum temperature: reliability derating.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JunctionMargin {
    pub margin: f64,
    pub reason: String,
}

/// IEC 61000-4-2 levels, in volts. Level 4 is 8 kV contact and 15 kV air.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EsdLevel {
    pub contact: f64,
    pub air: f64,
    pub reason: String,
}

/// Use only `factor` of a rating, for parts of a class (or one part number) and one quantity.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derate {
    pub class: Option<Class>,
    pub mpn: Option<String>,
    pub quantity: Quantity,
    /// Fraction of the rating to use, such as 0.8 for 80 %.
    pub factor: f64,
    /// Why, for the report.
    pub reason: String,
}

impl Policy {
    pub fn load(path: &Path) -> Result<Policy, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Policy::parse(&text, &path.display().to_string())
    }

    /// The `policy.toml` at the root of this repository, built into the binary.
    pub fn builtin() -> Result<Policy, String> {
        Policy::parse(include_str!("../policy.toml"), "policy.toml (built in)")
    }

    /// Parses a policy file's text; `origin` names it in errors.
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

    /// The derating that applies to a part's limit. A rule naming the part number wins over a class rule.
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

/// `policy.toml`'s `[spice]` table: the cable a board is plugged in through, and the sweep.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpicePolicy {
    /// The cable's inductance range in henries, `[min, max]`: each run draws one.
    pub cable_inductance: [f64; 2],
    /// The cable's resistance in ohms, including the connector's contacts.
    pub cable_resistance: f64,
    /// How fast the supply switches on, 0 to its maximum, in seconds. A hot plug is close to instant.
    pub rise_time: f64,
    /// How long to simulate, in seconds.
    pub duration: f64,
    /// How many Monte Carlo runs, besides the one at nominal values.
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

/// How fast a digital output's edge is taken to be, from `policy.toml`'s `[edges]`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgePolicy {
    /// Seconds, 10 % to 90 %.
    pub rise: f64,
    pub reason: String,
}

/// The largest area a switching regulator's input loop may enclose, from `policy.toml`'s `[hot_loop]`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HotLoopPolicy {
    /// Square millimetres.
    pub area: f64,
    pub reason: String,
}

/// How near a power pin its nearest capacitor to ground must be, from `policy.toml`'s `[decoupling]`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecouplingPolicy {
    /// Millimetres between the centres of the pin's pad and the capacitor's pad on the same net.
    pub distance: f64,
    /// Farads: a bigger capacitor is a bulk capacitor, which needn't be close.
    pub largest: f64,
    pub reason: String,
}

/// The temperature rise a track may have, from `policy.toml`'s `[traces]`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePolicy {
    /// °C above the board.
    pub rise: f64,
    pub reason: String,
}
