//! A board's `datasheets/` folder: `datasheet verify` reads a PDF saved there before it downloads anything, and
//! `doctor` names the file to save.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gerberus::datasheet::{saved_for, saved_name};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn board(name: &str, url: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gerberus-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("parts")).unwrap();
    let text = std::fs::read_to_string(root("parts/0402WGF1001TCE.toml")).unwrap();
    let text: String = text
        .lines()
        .map(|l| {
            if l.starts_with("url") {
                format!("url = \"{url}\"")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join("parts/0402WGF1001TCE.toml"), text).unwrap();
    dir
}

fn verify(dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["datasheet", "verify", "parts/0402WGF1001TCE.toml"])
        .current_dir(dir)
        .env("GERBERUS_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap()
}

#[test]
fn a_saved_pdf_is_found_in_any_letter_case() {
    assert_eq!(saved_name("TCAN1044/V"), "TCAN1044_V.pdf");
    let dir = board("saved", "https://invalid.invalid/x.pdf");
    let part = dir.join("parts/0402WGF1001TCE.toml");
    assert!(saved_for(&part, "0402WGF1001TCE").is_none());
    std::fs::create_dir_all(dir.join("datasheets")).unwrap();
    std::fs::write(dir.join("datasheets/0402wgf1001tce.PDF"), b"not a pdf").unwrap();
    assert!(saved_for(&part, "0402WGF1001TCE").is_some());
    // The saved file is read instead of the (unreachable) link: the pages don't read, but nothing is downloaded.
    let out = verify(&dir);
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(!all.contains("downloading"), "{all}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn no_link_and_nothing_saved_names_the_file_to_save() {
    let dir = board("nolink", "");
    let out = verify(&dir);
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{err}");
    assert!(
        err.contains(&format!("datasheets{}0402WGF1001TCE.pdf", std::path::MAIN_SEPARATOR)),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
