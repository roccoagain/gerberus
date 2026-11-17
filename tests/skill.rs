//! `gerberus skill install`: the shipped skills written into a board's repository, never over a file someone
//! changed unless forced.

use std::process::{Command, Output};

fn run(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["skill", "install"])
        .args(args)
        .current_dir(dir)
        .env("GERBERUS_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap()
}

#[test]
fn installs_once_and_keeps_edits() {
    let dir = std::env::temp_dir().join(format!("gerberus-skill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = run(&dir, &[]);
    assert!(out.status.success(), "{out:?}");
    let skills = dir.join(".claude/skills");
    for f in gerberus::skill::FILES {
        assert_eq!(std::fs::read_to_string(skills.join(f.path)).unwrap(), f.text);
    }
    // Every skill starts with its name and a description, as agents need.
    for f in gerberus::skill::FILES.iter().filter(|f| f.path.ends_with("SKILL.md")) {
        let name = f.path.split('/').next().unwrap();
        assert!(
            f.text.starts_with(&format!("---\nname: {name}\ndescription: ")),
            "{}",
            f.path
        );
    }
    // Again: nothing to do.
    let out = run(&dir, &[]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("up to date"));
    // An edited file is kept, and nothing else is written, unless --force.
    let skill = skills.join("check-board/SKILL.md");
    std::fs::write(&skill, "edited").unwrap();
    std::fs::remove_file(skills.join("datasheet-to-part/SKILL.md")).unwrap();
    let out = run(&dir, &[]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), "edited");
    assert!(!skills.join("datasheet-to-part/SKILL.md").exists());
    let out = run(&dir, &["--force"]);
    assert!(out.status.success(), "{out:?}");
    assert_ne!(std::fs::read_to_string(&skill).unwrap(), "edited");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The skills only name commands gerberus has.
#[test]
fn skills_name_real_commands() {
    let usage = Command::new(env!("CARGO_BIN_EXE_gerberus")).output().unwrap();
    let usage = String::from_utf8_lossy(&usage.stderr).to_string();
    for f in gerberus::skill::FILES.iter().filter(|f| f.path.ends_with("SKILL.md")) {
        // In code: a `gerberus x` span, or a line of a code block.
        let mut words = vec![];
        for (k, part) in f.text.split('`').enumerate() {
            for line in part.lines() {
                let line = line.trim_start();
                if (k % 2 == 1 || part.starts_with("sh\n")) && line.starts_with("gerberus ") {
                    let w = line[9..].split_whitespace().next().unwrap_or("");
                    if w.chars().all(|c| c.is_ascii_lowercase()) {
                        words.push(w.to_string());
                    }
                }
            }
        }
        assert!(!words.is_empty(), "{}", f.path);
        for w in words {
            assert!(usage.contains(&format!("gerberus {w}")), "{}: gerberus {w}", f.path);
        }
    }
}
