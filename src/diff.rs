use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::design::{Component, Design};
use crate::rules::{Finding, Report, Severity, Verdict};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartChange {
    pub reference: String,
    pub kind: &'static str,
    pub what: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NetChange {
    pub kind: &'static str,
    pub net: String,
    pub groups: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FindingChange {
    pub rule: &'static str,
    pub reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub case: Option<String>,
    pub severity: Severity,
    pub old: Option<Verdict>,
    pub new: Option<Verdict>,
    pub old_fails: usize,
    pub new_fails: usize,
    pub old_errors: usize,
    pub new_errors: usize,
    pub old_waived: usize,
    pub new_waived: usize,
    pub message: String,
}

impl FindingChange {
    pub fn added_fails(&self) -> usize {
        self.new_fails.saturating_sub(self.old_fails)
    }

    pub fn added_errors(&self) -> usize {
        self.new_errors.saturating_sub(self.old_errors)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Diff {
    pub parts: Vec<PartChange>,
    pub nets: Vec<NetChange>,
    pub findings: Vec<FindingChange>,
}

impl Diff {
    pub fn new_errors(&self) -> usize {
        self.findings.iter().map(FindingChange::added_errors).sum()
    }
}

pub fn diff(old: &Design, old_report: &Report, new: &Design, new_report: &Report) -> Diff {
    Diff {
        parts: parts(old, new),
        nets: nets(old, new),
        findings: findings(old_report, new_report),
    }
}

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

type Key = (&'static str, String, Option<String>, String);

fn key(f: &Finding) -> Key {
    let prefix = format!("{}.", f.reference);
    let pin = f
        .message
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, ':' | ',' | ';' | '(' | ')' | '"')))
        .find(|w| w.starts_with(&prefix) && w.len() > prefix.len())
        .unwrap_or("")
        .to_string();
    (f.rule, f.reference.clone(), f.case.clone(), pin)
}

#[derive(Default)]
struct Side<'a> {
    worst: Option<&'a Finding>,
    fails: usize,
    errors: usize,
    waived: usize,
    any: Option<&'a Finding>,
}

fn sides(r: &Report) -> BTreeMap<Key, Side<'_>> {
    let mut out: BTreeMap<Key, Side<'_>> = BTreeMap::new();
    for f in &r.findings {
        let s = out.entry(key(f)).or_default();
        if s.worst
            .is_none_or(|g| (f.verdict, f.severity) < (g.verdict, g.severity))
        {
            s.worst = Some(f);
        }
        if f.verdict == Verdict::Fail {
            s.fails += 1;
            if f.severity == Severity::Error {
                s.errors += 1;
            }
        }
    }
    for w in &r.waived {
        let s = out.entry(key(&w.finding)).or_default();
        s.waived += 1;
        s.any.get_or_insert(&w.finding);
    }
    out
}

pub fn findings(old: &Report, new: &Report) -> Vec<FindingChange> {
    let (a, b) = (sides(old), sides(new));
    let keys: BTreeSet<_> = a.keys().chain(b.keys()).cloned().collect();
    let empty = Side::default();
    let mut out = vec![];
    for k in keys {
        let (x, y) = (a.get(&k).unwrap_or(&empty), b.get(&k).unwrap_or(&empty));
        let worst = |s: &Side<'_>| s.worst.map(|f| (f.verdict, f.severity));
        if worst(x) == worst(y) && x.fails == y.fails && x.errors == y.errors && x.waived == y.waived {
            continue;
        }
        let (ov, nv) = (x.worst.map(|f| f.verdict), y.worst.map(|f| f.verdict));
        if ov.is_none_or(|v| v == Verdict::Pass) && nv.is_none_or(|v| v == Verdict::Pass) && x.waived == y.waived {
            continue;
        }
        let Some(f) = y.worst.or(y.any).or(x.worst).or(x.any) else {
            continue;
        };
        out.push(FindingChange {
            rule: f.rule,
            reference: f.reference.clone(),
            case: f.case.clone(),
            severity: f.severity,
            old: ov,
            new: nv,
            old_fails: x.fails,
            new_fails: y.fails,
            old_errors: x.errors,
            new_errors: y.errors,
            old_waived: x.waived,
            new_waived: y.waived,
            message: f.message.clone(),
        });
    }
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
            let count = |v: Option<Verdict>, fails: usize, waived: usize| {
                let mut t = verdict(v).to_string();
                if fails > 1 {
                    let _ = write!(t, " ×{fails}");
                }
                if waived > 0 {
                    let _ = write!(t, " ({waived} waived)");
                }
                t
            };
            let _ = writeln!(
                out,
                "  {} → {} {sev}[{}]{case} {}: {}",
                count(f.old, f.old_fails, f.old_waived),
                count(f.new, f.new_fails, f.new_waived),
                f.rule,
                f.reference,
                f.message
            );
        }
        let _ = writeln!(out);
    }
    let new_fails: usize = d.findings.iter().map(FindingChange::added_fails).sum();
    let fixed: usize = d.findings.iter().map(|f| f.old_fails.saturating_sub(f.new_fails)).sum();
    let waived: usize = d
        .findings
        .iter()
        .map(|f| f.new_waived.saturating_sub(f.old_waived))
        .sum();
    let _ = writeln!(
        out,
        "{} part changes, {} connection changes, {new_fails} new failures, {fixed} fixed{}",
        d.parts.len(),
        d.nets.len(),
        if waived > 0 {
            format!(", {waived} newly waived")
        } else {
            String::new()
        }
    );
    out
}

pub struct OldTree {
    pub path: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl Drop for OldTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

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
    let base = std::env::temp_dir();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = (0..100)
        .map(|i| base.join(format!("gerberus-diff-{}-{stamp}-{i}", std::process::id())))
        .find(|p| std::fs::create_dir(p).is_ok())
        .ok_or_else(|| format!("can't make a temporary folder in {}", base.display()))?;
    let tree = OldTree {
        path: root.join(&rel),
        root: root.clone(),
    };
    let mut archive = Command::new("git")
        .arg("-C")
        .arg(&top)
        .args(["archive", "--format=tar", rev])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run git: {e}"))?;
    let out = archive.stdout.take().ok_or("git archive has no output")?;
    let tar = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(&root)
        .stdin(std::process::Stdio::from(out))
        .output()
        .map_err(|e| format!("can't run tar: {e}"))?;
    let git_out = archive.wait_with_output().map_err(|e| format!("git archive: {e}"))?;
    if !git_out.status.success() {
        return Err(format!(
            "git archive {rev}: {}",
            String::from_utf8_lossy(&git_out.stderr).trim()
        ));
    }
    if !tar.status.success() {
        return Err(format!(
            "tar couldn't unpack the old revision: {}",
            String::from_utf8_lossy(&tar.stderr).trim()
        ));
    }
    Ok(tree)
}
