use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::design::Design;
use crate::rules::{Finding, Report, SUMMARY_RULES, Severity, Verdict};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waivers {
    #[serde(default)]
    pub waiver: Vec<Waiver>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Waiver {
    pub rule: String,
    pub reference: String,
    pub contains: Option<String>,
    pub case: Option<String>,
    pub reason: String,
    pub by: Option<String>,
    pub proposed: Option<String>,
}

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
            if w.proposed.as_deref().is_some_and(|c| c.trim().is_empty()) {
                out.push(format!(
                    "{at}: `proposed` is empty; say who proposed it, or delete the line to approve it"
                ));
            }
            if w.proposed.is_some() && w.by.is_some() {
                out.push(format!(
                    "{at}: it has both `proposed` and `by`; a person approves it by deleting `proposed`"
                ));
            }
            if w.contains.as_deref().is_some_and(|c| c.trim().is_empty()) {
                out.push(format!(
                    "{at}: `contains` is empty; leave it out to match every finding"
                ));
            }
        }
        out
    }

    pub fn apply(&self, report: &mut Report, file: &str) {
        let mut used = vec![false; self.waiver.len()];
        report.waivers_file = file.to_string();
        let findings = std::mem::take(&mut report.findings);
        for mut f in findings {
            let hits: Vec<usize> = if f.verdict == Verdict::Pass {
                vec![]
            } else {
                (0..self.waiver.len()).filter(|&i| self.waiver[i].matches(&f)).collect()
            };
            for &i in &hits {
                used[i] = true;
            }
            let approved = hits.iter().any(|&i| self.waiver[i].proposed.is_none());
            for &i in hits.iter().filter(|&&i| !approved && self.waiver[i].proposed.is_some()) {
                let w = &self.waiver[i];
                let _ = write!(
                    f.message,
                    "; a waiver proposed by {} in {file} is waiting for a person to approve it (delete its \
                     `proposed` line): {}",
                    w.proposed.as_deref().unwrap_or_default(),
                    w.reason
                );
            }
            match hits.iter().find(|&&i| self.waiver[i].proposed.is_none()) {
                Some(&i) => {
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
        let all = std::mem::take(&mut report.findings);
        let settled: Vec<bool> = all
            .iter()
            .map(|f| {
                SUMMARY_RULES.contains(&f.rule)
                    && f.verdict == Verdict::Fail
                    && f.case.is_some()
                    && report.waived.iter().any(|w| w.finding.case == f.case)
                    && !all.iter().any(|g| {
                        g.case == f.case
                            && !SUMMARY_RULES.contains(&g.rule)
                            && g.verdict == Verdict::Fail
                            && g.severity == Severity::Error
                    })
            })
            .collect();
        for (f, settled) in all.into_iter().zip(settled) {
            if settled {
                report.waived.push(Waived {
                    finding: f,
                    reason: "every failure in this fault case is waived".into(),
                    by: None,
                });
            } else {
                report.findings.push(f);
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
