//! Embeds the shared part library (`parts/*.toml`) and the default `policy.toml` in the binary, so a released
//! `gerberus` works without a checkout of this repository next to it.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let parts = root.join("parts");
    println!("cargo::rerun-if-changed={}", parts.display());
    let mut files: Vec<_> = std::fs::read_dir(&parts)
        .expect("parts/ is readable")
        .map(|e| e.expect("parts/ entry is readable").path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    let mut out = String::from("&[\n");
    for path in &files {
        println!("cargo::rerun-if-changed={}", path.display());
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("part file names are UTF-8");
        let full = path.to_str().expect("paths are UTF-8");
        writeln!(out, "    ({name:?}, include_str!({full:?})),").unwrap();
    }
    out.push(']');
    let dest = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("builtin_parts.rs");
    std::fs::write(dest, out).expect("OUT_DIR is writable");
}
