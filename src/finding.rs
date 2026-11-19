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
    /// How bad a FAIL is. An UNKNOWN or PASS carries the severity its FAIL would have had.
    pub severity: Severity,
    /// Reference designator, such as `D2`.
    pub reference: String,
    /// What was compared: the pin, its net, the numbers and their datasheet pages.
    pub message: String,
    /// Nets whose ranges the finding read, so a report can explain them.
    #[serde(skip)]
    pub nets: Vec<usize>,
    /// The fault case the finding comes from, or none for normal operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    /// A short headline for the terminal report, when the message doesn't open with one. Not in the JSON, whose
    /// `message` says it all.
    #[serde(skip)]
    pub headline: Option<String>,
}

/// A finding a waiver accepted, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Waived {
    pub finding: Finding,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}
