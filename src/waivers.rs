//! `waivers.toml`: findings the board's designers have looked at and accepted, each with the reason.
//!
//! A waiver names a rule and a part, and optionally text the finding's message contains and a fault case. Every
//! FAIL or UNKNOWN finding it matches moves out of the report's findings into its `waived` list, so it isn't
//! counted and doesn't fail `check`; `--all` and `--json` still show it with the reason. A waiver that matches
//! nothing is a warning, so a waiver can't outlive the finding it was written for unnoticed.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::design::Design;
use crate::rules::{Finding, Report, Severity, Verdict};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waivers {
    #[serde(default)]
    pub waiver: Vec<Waiver>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    /// The rule, as a report prints it in brackets: `esd` for `error[esd]`.
    pub rule: String,
    /// The part the finding is on, such as `J1`.
    pub reference: String,
    /// Text the finding's message must contain, such as a pin (`J1.1`) or a net name. Left out: every finding
    /// of the rule on the part.
    pub contains: Option<String>,
    /// A fault case's name: only the findings of that case. Left out: normal operation and every case.
    pub case: Option<String>,
    /// Why the finding is acceptable. Required: a waiver nobody can explain is a finding hidden.
    pub reason: String,
    /// Who accepted it, and when, for the record.
    pub by: Option<String>,
}

/// A finding a waiver accepted, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waived {
    pub finding: Finding,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

impl Waivers {
    pub fn load(path: &Path) -> Result<Waivers, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Waivers that can't match anything on this design: a part it doesn't have, or no reason.
    pub fn problems(&self, design: &Design) -> Vec<String> {
        let mut out = vec![];
        for (i, w) in self.waiver.iter().enumerate() {
            let at = format!("waiver {} ({}, {})", i + 1, w.rule, w.reference);
            if !design.components.contains_key(&w.reference) {
                out.push(format!("{at}: the design has no part {}", w.reference));
            }
            if w.reason.trim().is_empty() {
                out.push(format!("{at}: give the reason it is acceptable"));
            }
            if w.contains.as_deref().is_some_and(|c| c.trim().is_empty()) {
                out.push(format!(
                    "{at}: `contains` is empty; leave it out to match every finding"
                ));
            }
        }
        out
    }

    /// Moves every FAIL and UNKNOWN finding a waiver matches into `report.waived`, and adds a warning for each
    /// waiver that matched nothing.
    pub fn apply(&self, report: &mut Report, file: &str) {
        let mut used = vec![false; self.waiver.len()];
        let findings = std::mem::take(&mut report.findings);
        for f in findings {
            let hit = (f.verdict != Verdict::Pass)
                .then(|| self.waiver.iter().position(|w| w.matches(&f)))
                .flatten();
            match hit {
                Some(i) => {
                    used[i] = true;
                    let w = &self.waiver[i];
                    report.waived.push(Waived {
                        finding: f,
                        reason: w.reason.clone(),
                        by: w.by.clone(),
                    });
                }
                None => report.findings.push(f),
            }
        }
        for (w, used) in self.waiver.iter().zip(used) {
            if used {
                continue;
            }
            report.findings.push(Finding {
                rule: "waiver",
                verdict: Verdict::Fail,
                severity: Severity::Warning,
                reference: w.reference.clone(),
                message: format!(
                    "the waiver in {file} for {}[{}]{}{} matches no failed or unknown finding: remove it, or fix it to \
                     match what the report says now",
                    w.reference,
                    w.rule,
                    w.contains
                        .as_ref()
                        .map(|c| format!(" containing {c:?}"))
                        .unwrap_or_default(),
                    w.case.as_ref().map(|c| format!(" in fault {c:?}")).unwrap_or_default(),
                ),
                nets: vec![],
                case: None,
                headline: None,
            });
        }
    }
}

impl Waiver {
    fn matches(&self, f: &Finding) -> bool {
        f.rule == self.rule
            && f.reference == self.reference
            && self.contains.as_ref().is_none_or(|c| f.message.contains(c.as_str()))
            && self.case.as_ref().is_none_or(|c| f.case.as_ref() == Some(c))
    }
}
