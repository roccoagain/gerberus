//! Gerberus checks a PCB design for electrical bugs before it is ordered.

// A malformed board file must give an error, not a crash: no `unwrap()` outside tests. `expect("why")` stays
// for invariants the code itself guarantees. Tests may unwrap (clippy.toml). Set here, not in Cargo.toml, so
// it skips the integration tests and examples.
#![warn(clippy::unwrap_used)]

pub mod altium;
pub mod bom;
pub mod bringup;
pub mod datasheet;
pub mod design;
pub mod diag;
pub mod diff;
pub mod doctor;
pub mod family;
pub mod init;
pub mod inputs;
pub mod kicad;
pub mod limits;
pub mod mcp;
pub mod part_new;
pub mod pinout;
pub mod policy;
pub mod protection;
pub mod rules;
pub mod sexpr;
pub mod si;
pub mod signal;
pub mod skill;
pub mod spec;
pub mod spice;
pub mod thermal;
pub mod traces;
pub mod update;
pub mod voltage;
pub mod waivers;

/// Loads a design from any supported format, chosen by file extension: a KiCad `.kicad_sch`, or an Altium
/// `.PrjPcb` project or lone `.SchDoc` sheet.
pub fn load(path: &std::path::Path) -> Result<design::Design, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "prjpcb" | "schdoc" => altium::load(path),
        _ => kicad::load(path),
    }
}
