//! `gerberus skill install`: the agent skills that ship with gerberus, written into a board's repository so an
//! agent working there checks the board and adds parts the same way as in gerberus's own.

use std::path::{Path, PathBuf};

/// One file of a skill: where it goes under `.claude/skills/`, and its text.
pub struct File {
    pub path: &'static str,
    pub text: &'static str,
}

/// The skills, built in.
pub const FILES: &[File] = &[
    File {
        path: "check-board/SKILL.md",
        text: include_str!("../.claude/skills/check-board/SKILL.md"),
    },
    File {
        path: "datasheet-to-part/SKILL.md",
        text: include_str!("../.claude/skills/datasheet-to-part/SKILL.md"),
    },
    // The format guide the datasheet skill reads, which a board's repository doesn't have.
    File {
        path: "datasheet-to-part/part-format.md",
        text: include_str!("../parts/README.md"),
    },
];

/// What installing did to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Written,
    Unchanged,
    Replaced,
}

/// Writes the skills into `<dir>/.claude/skills/`. A file already there with other text is kept, and nothing is
/// written, unless `force`.
pub fn install(dir: &Path, force: bool) -> Result<Vec<(PathBuf, Outcome)>, String> {
    let base = dir.join(".claude").join("skills");
    let mut plan = vec![];
    let mut changed = vec![];
    for f in FILES {
        let path = base.join(f.path);
        let outcome = match std::fs::read_to_string(&path) {
            Ok(old) if old == f.text => Outcome::Unchanged,
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
            "{} differ{} from the skills this gerberus ships (edited by hand, or from another version); --force \
             replaces {}",
            changed.join(", "),
            if changed.len() == 1 { "s" } else { "" },
            if changed.len() == 1 { "it" } else { "them" }
        ));
    }
    for (path, text, outcome) in &plan {
        if *outcome == Outcome::Unchanged {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(plan.into_iter().map(|(p, _, o)| (p, o)).collect())
}
