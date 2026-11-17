//! What changed between two revisions of a design: parts added, removed or changed, connections made or broken,
//! and the check findings that are new, fixed or changed verdict.
//!
//! Nets are matched by the pins on them, not by name, since KiCad names unlabelled nets after a pin
//! (`Net-(R1-Pad1)`) and those names move when parts are renumbered. A pin on both revisions keeps its place if
//! the pins it shares a net with are the same; otherwise its old net was split or its new net joins pins that
//! were apart.
//!
//! Findings are matched by rule, part, fault case and the pin a message opens with (`U1.VIN`), so a number that
//! moves inside a message is a change of the same finding, not a new one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::design::{Component, Design};
use crate::rules::{Finding, Report, Severity, Verdict};

/// A part on one revision only, or on both with something changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartChange {
    pub reference: String,
    /// `added`, `removed` or `changed`.
    pub kind: &'static str,
    /// For `changed`: each field as `name: old → new`. For the others: the part's value and symbol.
    pub what: Vec<String>,
}

/// Pins that were on one net and now are on several, or the other way round.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetChange {
    /// `split` (an old net's pins are now on several nets) or `joined` (a new net has pins from several).
    pub kind: &'static str,
    /// The net named in the revision it is whole in: the old one for `split`, the new one for `joined`.
    pub net: String,
    /// Each group of pins, and the net it is on in the other revision.
    pub groups: Vec<(String, Vec<String>)>,
}

/// A finding whose verdict changed, or that is on one revision only.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingChange {
    pub rule: &'static str,
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    pub severity: Severity,
    pub old: Option<Verdict>,
    pub new: Option<Verdict>,
    /// The new revision's message, or the old one's when the finding is gone.
    pub message: String,
}

impl FindingChange {
    /// A FAIL the old revision didn't have.
    pub fn is_new_fail(&self) -> bool {
        self.new == Some(Verdict::Fail) && self.old != Some(Verdict::Fail)
    }
}

/// Everything that changed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Diff {
    pub parts: Vec<PartChange>,
    pub nets: Vec<NetChange>,
    pub findings: Vec<FindingChange>,
}

impl Diff {
    /// FAILs with error severity that the old revision didn't have.
    pub fn new_errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.is_new_fail() && f.severity == Severity::Error)
            .count()
    }
}

/// Compares two revisions and their check reports.
pub fn diff(old: &Design, old_report: &Report, new: &Design, new_report: &Report) -> Diff {
    Diff {
        parts: parts(old, new),
        nets: nets(old, new),
        findings: findings(old_report, new_report),
    }
}

/// Fields a part change lists, besides its pins.
fn fields(c: &Component) -> Vec<(&'static str, String)> {
    let mpn = ["MPN", "Manufacturer Part Number", "Part Number", "MFR Part Number"]
        .iter()
        .find_map(|k| c.fields.get(*k))
        .cloned()
        .unwrap_or_default();
    vec![
        ("value", c.value.clone()),
        ("symbol", c.lib_id.clone()),
        ("footprint", c.footprint.clone()),
        ("MPN", mpn),
        ("DNP", if c.dnp { "yes".into() } else { "no".into() }),
    ]
}

pub fn parts(old: &Design, new: &Design) -> Vec<PartChange> {
    let mut out = vec![];
    let refs: BTreeSet<&String> = old.components.keys().chain(new.components.keys()).collect();
    for r in refs {
        match (old.components.get(r), new.components.get(r)) {
            (Some(a), Some(b)) => {
                let mut what: Vec<String> = fields(a)
                    .into_iter()
                    .zip(fields(b))
                    .filter(|(x, y)| x.1 != y.1)
                    .map(|((k, x), (_, y))| format!("{k}: {} → {}", show(&x), show(&y)))
                    .collect();
                let pa: BTreeSet<&String> = a.pins.keys().collect();
                let pb: BTreeSet<&String> = b.pins.keys().collect();
                if pa != pb {
                    what.push(format!("pins: {} → {}", pa.len(), pb.len()));
                }
                if !what.is_empty() {
                    out.push(PartChange {
                        reference: r.clone(),
                        kind: "changed",
                        what,
                    });
                }
            }
            (Some(c), None) | (None, Some(c)) => out.push(PartChange {
                reference: r.clone(),
                kind: if new.components.contains_key(r) {
                    "added"
                } else {
                    "removed"
                },
                what: vec![format!("{} ({})", c.value, c.lib_id)],
            }),
            (None, None) => {}
        }
    }
    out
}

fn show(s: &str) -> &str {
    if s.is_empty() { "(none)" } else { s }
}

/// Every pin, as `REF.PIN`, with its net. A pin left alone on its own net counts, as that is a connection broken.
fn pin_nets(d: &Design) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for (r, c) in &d.components {
        for (num, p) in &c.pins {
            out.insert(format!("{r}.{num}"), p.net);
        }
    }
    out
}

pub fn nets(old: &Design, new: &Design) -> Vec<NetChange> {
    let (po, pn) = (pin_nets(old), pin_nets(new));
    // Pins on both revisions, by their net on each.
    let common: Vec<(&String, usize, usize)> = po.iter().filter_map(|(p, &a)| pn.get(p).map(|&b| (p, a, b))).collect();
    let mut out = vec![];
    let mut group = |by_whole: bool| {
        let mut whole: BTreeMap<usize, BTreeMap<usize, Vec<String>>> = BTreeMap::new();
        for &(p, a, b) in &common {
            let (w, other) = if by_whole { (a, b) } else { (b, a) };
            whole.entry(w).or_default().entry(other).or_default().push(p.clone());
        }
        for (w, parts) in whole {
            if parts.len() < 2 {
                continue;
            }
            let (whole_d, other_d) = if by_whole { (old, new) } else { (new, old) };
            out.push(NetChange {
                kind: if by_whole { "split" } else { "joined" },
                net: whole_d.nets[w].name.clone(),
                groups: parts
                    .into_iter()
                    .map(|(o, pins)| (other_d.nets[o].name.clone(), pins))
                    .collect(),
            });
        }
    };
    group(true);
    group(false);
    out
}

/// What a finding is about: its rule, part, case and the pin its message opens with.
fn key(f: &Finding) -> (&'static str, String, Option<String>, String) {
    let first = f.message.split_whitespace().next().unwrap_or("");
    let pin = if first.starts_with(&format!("{}.", f.reference)) {
        first.trim_end_matches([':', ',']).to_string()
    } else {
        String::new()
    };
    (f.rule, f.reference.clone(), f.case.clone(), pin)
}

/// The worst finding for each key.
fn worst(r: &Report) -> BTreeMap<(&'static str, String, Option<String>, String), &Finding> {
    let mut out: BTreeMap<_, &Finding> = BTreeMap::new();
    for f in &r.findings {
        out.entry(key(f))
            .and_modify(|g| {
                if (f.verdict, f.severity) < (g.verdict, g.severity) {
                    *g = f;
                }
            })
            .or_insert(f);
    }
    out
}

pub fn findings(old: &Report, new: &Report) -> Vec<FindingChange> {
    let (a, b) = (worst(old), worst(new));
    let keys: BTreeSet<_> = a.keys().chain(b.keys()).cloned().collect();
    let mut out = vec![];
    for k in keys {
        let (x, y) = (a.get(&k), b.get(&k));
        let (ov, nv) = (x.map(|f| f.verdict), y.map(|f| f.verdict));
        if ov == nv {
            continue;
        }
        // Passes that come and go with a part aren't worth listing.
        if ov.is_none_or(|v| v == Verdict::Pass) && nv.is_none_or(|v| v == Verdict::Pass) {
            continue;
        }
        let f = y.or(x).expect("one side has it");
        out.push(FindingChange {
            rule: f.rule,
            reference: f.reference.clone(),
            case: f.case.clone(),
            severity: f.severity,
            old: ov,
            new: nv,
            message: f.message.clone(),
        });
    }
    // New FAILs first, then new UNKNOWNs, then what was fixed.
    out.sort_by_key(|c| (c.new.map_or(3, |v| v as u8), c.severity, c.reference.clone()));
    out
}

fn verdict(v: Option<Verdict>) -> &'static str {
    match v {
        Some(Verdict::Fail) => "FAIL",
        Some(Verdict::Unknown) => "UNKNOWN",
        Some(Verdict::Pass) => "PASS",
        None => "none",
    }
}

/// The diff as text.
pub fn render(d: &Diff) -> String {
    let mut out = String::new();
    if d.parts.is_empty() && d.nets.is_empty() && d.findings.is_empty() {
        return "No changes to parts, connections or findings.\n".into();
    }
    if !d.parts.is_empty() {
        let _ = writeln!(out, "Parts:");
        for p in &d.parts {
            let sign = match p.kind {
                "added" => "+",
                "removed" => "-",
                _ => "~",
            };
            let _ = writeln!(out, "  {sign} {}: {}", p.reference, p.what.join("; "));
        }
        let _ = writeln!(out);
    }
    if !d.nets.is_empty() {
        let _ = writeln!(out, "Connections:");
        for n in &d.nets {
            let what = if n.kind == "split" {
                format!("{} is split; its pins are now on", n.net)
            } else {
                format!("{} joins pins that were on", n.net)
            };
            let _ = writeln!(out, "  {what}:");
            for (net, pins) in &n.groups {
                let shown: Vec<&str> = pins.iter().take(8).map(String::as_str).collect();
                let more = if pins.len() > 8 {
                    format!(" and {} more", pins.len() - 8)
                } else {
                    String::new()
                };
                let _ = writeln!(out, "    {net}: {}{more}", shown.join(", "));
            }
        }
        let _ = writeln!(out);
    }
    if !d.findings.is_empty() {
        let _ = writeln!(out, "Findings:");
        for f in &d.findings {
            let case = f.case.as_deref().map(|c| format!(" [{c}]")).unwrap_or_default();
            let sev = match f.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            let _ = writeln!(
                out,
                "  {} → {} {sev}[{}]{case} {}: {}",
                verdict(f.old),
                verdict(f.new),
                f.rule,
                f.reference,
                f.message
            );
        }
        let _ = writeln!(out);
    }
    let new_fails = d.findings.iter().filter(|f| f.is_new_fail()).count();
    let fixed = d
        .findings
        .iter()
        .filter(|f| f.old == Some(Verdict::Fail) && f.new != Some(Verdict::Fail))
        .count();
    let _ = writeln!(
        out,
        "{} part changes, {} connection changes, {new_fails} new failures, {fixed} fixed",
        d.parts.len(),
        d.nets.len()
    );
    out
}

/// A design's folder as it was at a git revision, in a temporary folder removed on drop.
pub struct OldTree {
    /// The design file inside the temporary folder.
    pub path: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl Drop for OldTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Extracts the git repository holding `design` at `rev` into a temporary folder: the whole tree, so the old
/// revision's sheets, `parts/`, `inputs.toml` and `waivers.toml` come with it.
pub fn checkout(design: &std::path::Path, rev: &str) -> Result<OldTree, String> {
    use std::process::Command;
    let abs = std::fs::canonicalize(design).map_err(|e| format!("{}: {e}", design.display()))?;
    let dir = abs.parent().ok_or("the design has no folder")?;
    let git = |args: &[&str]| -> Result<String, String> {
        let o = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map_err(|e| format!("can't run git: {e}"))?;
        if !o.status.success() {
            return Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let top = std::path::PathBuf::from(git(&["rev-parse", "--show-toplevel"])?);
    git(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?;
    let rel = abs
        .strip_prefix(std::fs::canonicalize(&top).map_err(|e| e.to_string())?)
        .map_err(|_| "the design isn't inside its git repository".to_string())?
        .to_path_buf();
    let root = std::env::temp_dir().join(format!("gerberus-diff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let tree = OldTree {
        path: root.join(&rel),
        root: root.clone(),
    };
    let archive = Command::new("git")
        .arg("-C")
        .arg(&top)
        .args(["archive", "--format=tar", rev])
        .output()
        .map_err(|e| format!("can't run git: {e}"))?;
    if !archive.status.success() {
        return Err(format!(
            "git archive {rev}: {}",
            String::from_utf8_lossy(&archive.stderr).trim()
        ));
    }
    let mut tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(&root)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run tar: {e}"))?;
    {
        use std::io::Write as _;
        let mut stdin = tar.stdin.take().ok_or("tar has no input")?;
        stdin.write_all(&archive.stdout).map_err(|e| format!("tar: {e}"))?;
    }
    let status = tar.wait().map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err("tar couldn't unpack the old revision".into());
    }
    if !tree.path.exists() {
        return Err(format!("{} isn't in revision {rev}", rel.display()));
    }
    Ok(tree)
}
