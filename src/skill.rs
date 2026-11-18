use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct File {
    pub path: &'static str,
    pub text: &'static str,
}

pub const FILES: &[File] = &[
    File {
        path: "check-board/SKILL.md",
        text: include_str!("../.claude/skills/check-board/SKILL.md"),
    },
    File {
        path: "datasheet-to-part/SKILL.md",
        text: include_str!("../.claude/skills/datasheet-to-part/SKILL.md"),
    },
    File {
        path: "datasheet-to-part/part-format.md",
        text: concat!(
            "<!-- parts/README.md from the Gerberus repository, installed by `gerberus skill install`. Parts of it are\n",
            "     about that repository (its tests, fixtures and policy.toml, \"What's here\", \"Known gaps\") and\n",
            "     don't apply to a board's repository: there, the built-in part files are read with\n",
            "     `gerberus part show <MPN>`, and a board's own part files go in the parts/ folder next to its\n",
            "     schematic. -->\n\n",
            include_str!("../parts/README.md")
        ),
    },
];

pub const MANIFEST: &str = ".gerberus-skills";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Written,
    Unchanged,
    Updated,
    Replaced,
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

pub fn install(dir: &Path, force: bool) -> Result<Vec<(PathBuf, Outcome)>, String> {
    let base = dir.join(".claude").join("skills");
    let manifest_path = base.join(MANIFEST);
    let recorded: BTreeMap<String, String> = std::fs::read_to_string(&manifest_path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(h, p)| (p.to_string(), h.to_string()))
        .collect();
    let mut plan = vec![];
    let mut changed = vec![];
    for f in FILES {
        let path = base.join(f.path);
        let outcome = match std::fs::read_to_string(&path) {
            Ok(old) if old == f.text => Outcome::Unchanged,
            Ok(old) if recorded.get(f.path) == Some(&hash(&old)) => Outcome::Updated,
            Ok(_) => {
                changed.push(path.display().to_string());
                Outcome::Replaced
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Outcome::Written,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        plan.push((path, f.text, outcome));
    }
    if !changed.is_empty() && !force {
        return Err(format!(
            "{} {} been changed since gerberus installed {}, so nothing was written; --force replaces {}",
            changed.join(", "),
            if changed.len() == 1 { "has" } else { "have" },
            if changed.len() == 1 { "it" } else { "them" },
            if changed.len() == 1 { "it" } else { "them" }
        ));
    }
    let mut done = vec![];
    for (path, text, outcome) in &plan {
        if *outcome != Outcome::Unchanged {
            let wrote = |e: String| {
                let so_far: Vec<String> = done.iter().map(|p: &PathBuf| p.display().to_string()).collect();
                if so_far.is_empty() {
                    e
                } else {
                    format!("{e} (already written: {})", so_far.join(", "))
                }
            };
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| wrote(format!("{}: {e}", parent.display())))?;
            }
            std::fs::write(path, text).map_err(|e| wrote(format!("{}: {e}", path.display())))?;
            done.push(path.clone());
        }
    }
    let mut manifest = String::new();
    for f in FILES {
        manifest.push_str(&hash(f.text));
        manifest.push(' ');
        manifest.push_str(f.path);
        manifest.push('\n');
    }
    std::fs::write(&manifest_path, manifest).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    Ok(plan.into_iter().map(|(p, _, o)| (p, o)).collect())
}
