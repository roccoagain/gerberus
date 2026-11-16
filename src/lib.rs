//! Gerberus checks a PCB design for electrical bugs before it is ordered.

pub mod altium;
pub mod bom;
pub mod datasheet;
pub mod design;
pub mod inputs;
pub mod kicad;
pub mod limits;
pub mod pinout;
pub mod policy;
pub mod protection;
pub mod rules;
pub mod sexpr;
pub mod si;
pub mod voltage;

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
