use std::path::Path;
use std::process::{Command, Output};

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["skill", "install"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gerberus-skill-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn installs_once_and_keeps_edits() {
    let dir = temp("edits");
    let out = run(&dir, &[]);
    assert!(out.status.success(), "{out:?}");
    let skills = dir.join(".claude/skills");
    for f in gerberus::skill::FILES {
        assert_eq!(std::fs::read_to_string(skills.join(f.path)).unwrap(), f.text);
    }
    let out = run(&dir, &[]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("up to date"));
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

#[test]
fn an_earlier_install_is_updated() {
    let dir = temp("update");
    let skills = dir.join(".claude/skills");
    let skill = skills.join("check-board/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "old text").unwrap();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in b"old text" {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    std::fs::write(
        skills.join(gerberus::skill::MANIFEST),
        format!("{h:016x} check-board/SKILL.md\n"),
    )
    .unwrap();
    let out = run(&dir, &[]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("updated"));
    assert_eq!(std::fs::read_to_string(&skill).unwrap(), gerberus::skill::FILES[0].text);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn skills_are_complete() {
    let files = gerberus::skill::FILES;
    for f in files.iter().filter(|f| f.path.ends_with("SKILL.md")) {
        let name = f.path.split('/').next().unwrap();
        assert!(
            f.text.starts_with(&format!("---\nname: {name}\ndescription: ")),
            "{}",
            f.path
        );
        if f.text.contains("part-format.md") {
            assert!(files.iter().any(|g| g.path == "datasheet-to-part/part-format.md"));
        }
    }
}

#[test]
fn skills_name_real_commands() {
    let usage = Command::new(env!("CARGO_BIN_EXE_gerberus")).output().unwrap();
    let usage = String::from_utf8_lossy(&usage.stderr).to_string();
    let mut seen = 0;
    for f in gerberus::skill::FILES.iter().filter(|f| f.path.ends_with("SKILL.md")) {
        for (k, part) in f.text.split('`').enumerate() {
            for line in part.lines() {
                let line = line.trim_start();
                if !((k % 2 == 1 || part.starts_with("sh\n")) && line.starts_with("gerberus ")) {
                    continue;
                }
                let words: Vec<&str> = line.split_whitespace().skip(1).collect();
                let Some(cmd) = words.first().filter(|w| w.chars().all(|c| c.is_ascii_lowercase())) else {
                    continue;
                };
                let mut want = format!("gerberus {cmd}");
                if matches!(*cmd, "part" | "datasheet" | "skill") {
                    want = format!("{want} {}", words.get(1).copied().unwrap_or(""));
                }
                assert!(usage.contains(&want), "{}: {line}", f.path);
                let usage_line = usage.lines().find(|l| l.trim_start().starts_with(&want)).unwrap();
                for opt in words.iter().filter(|w| w.starts_with("--")) {
                    assert!(
                        usage_line.contains(*opt)
                            || usage.contains(&format!("{opt},"))
                            || usage.contains(&format!("{opt} ")),
                        "{}: {line}",
                        f.path
                    );
                }
                seen += 1;
            }
        }
    }
    assert!(seen >= 8, "{seen}");
}
