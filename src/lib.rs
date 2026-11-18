#![warn(clippy::unwrap_used)]

pub const EXIT_CHECK_FAILED: u8 = 1;
pub const EXIT_CANNOT_RUN: u8 = 2;

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
