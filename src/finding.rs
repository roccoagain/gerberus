use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Fail,
    Unknown,
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Verdict::Fail => "FAIL",
            Verdict::Unknown => "UNKNOWN",
            Verdict::Pass => "PASS",
        })
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub verdict: Verdict,
    pub severity: Severity,
    pub reference: String,
    pub message: String,
    #[serde(skip)]
    pub nets: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    #[serde(skip)]
    pub headline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waived {
    pub finding: Finding,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}
