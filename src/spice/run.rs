//! Runs ngspice in batch mode and reads its measurements back.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::process::{Command, Stdio};

/// One pair's measurements in one run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairResult {
    pub max: f64,
    pub min: f64,
    pub end: f64,
}

/// Every `.meas` result of one run, by name.
#[derive(Debug, Clone, Default)]
pub struct Measured {
    values: BTreeMap<String, f64>,
}

impl Measured {
    /// Reads the `name = value` lines ngspice prints for `.meas` results.
    pub fn parse(output: &str) -> Measured {
        let mut values = BTreeMap::new();
        for line in output.lines() {
            let Some((name, rest)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                continue;
            }
            if let Some(v) = rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) {
                values.insert(name.to_ascii_lowercase(), v);
            }
        }
        Measured { values }
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.values.get(name).copied()
    }

    /// Pair `i`'s peak, trough and final value.
    pub fn pair(&self, i: usize) -> Option<PairResult> {
        Some(PairResult {
            max: self.get(&format!("p{i}max"))?,
            min: self.get(&format!("p{i}min"))?,
            end: self.get(&format!("p{i}end"))?,
        })
    }
}

/// Runs one netlist through `ngspice -b`, fed on its standard input.
pub fn simulate(netlist: &str) -> Result<Measured, String> {
    let mut child = Command::new("ngspice")
        .arg("-b")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run ngspice ({e}); install it (apt install ngspice, brew install ngspice)"))?;
    child
        .stdin
        .take()
        .ok_or("ngspice has no standard input")?
        .write_all(netlist.as_bytes())
        .map_err(|e| format!("can't write the netlist to ngspice: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("ngspice didn't finish: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let err = String::from_utf8_lossy(&out.stderr);
    let measured = Measured::parse(&text);
    if measured.values.is_empty() {
        let why: Vec<&str> = text
            .lines()
            .chain(err.lines())
            .filter(|l| l.to_ascii_lowercase().contains("error"))
            .take(5)
            .collect();
        return Err(format!("ngspice measured nothing: {}", why.join("; ")));
    }
    Ok(measured)
}
