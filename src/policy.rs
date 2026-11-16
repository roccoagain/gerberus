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
        let policy: Policy = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let problems = policy.problems();
        if problems.is_empty() {
            Ok(policy)
        } else {
            Err(format!("{}: {}", path.display(), problems.join("; ")))
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
