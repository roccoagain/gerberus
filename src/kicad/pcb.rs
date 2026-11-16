//! Reads pad-to-net assignments from a `.kicad_pcb` file.
//!
//! Layout checks will need much more of the board later. For now this lets tests compare the schematic
//! netlist with the one KiCad wrote into the board.

use std::path::Path;

use crate::sexpr::{self, Sexp};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PadNet {
    pub reference: String,
    pub pad: String,
    /// `None` for pads with no net, such as mounting pads.
    pub net: Option<String>,
}

/// One footprint placed on the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footprint {
    pub reference: String,
    /// Library footprint, such as `Package_SO:SOIC-8_3.9x4.9mm_P1.27mm`.
    pub lib_id: String,
    /// Pads with a number, in file order. Unnumbered pads (holes, paste-only shapes) are left out. A number
    /// can repeat, such as an exposed pad split into several shapes.
    pub pads: Vec<Pad>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub number: String,
    /// `smd`, `thru_hole`, `np_thru_hole` or `connect`.
    pub kind: String,
    /// `None` for pads with no net, such as mounting pads.
    pub net: Option<String>,
}

impl Footprint {
    /// Each pad number once, sorted.
    pub fn pad_numbers(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.pads.iter().map(|p| p.number.as_str()).collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Every footprint on the board, in file order.
pub fn load_footprints(path: &Path) -> Result<Vec<Footprint>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let tree = sexpr::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if tree.head() != Some("kicad_pcb") {
        return Err(format!("{}: not a KiCad board", path.display()));
    }
    let mut out = Vec::new();
    for fp in tree.children("footprint") {
        let reference = fp
            .children("property")
            .find(|p| p.arg(1) == Some("Reference"))
            .and_then(|p| p.arg(2))
            .or_else(|| {
                fp.children("fp_text")
                    .find(|t| t.arg(1) == Some("reference"))
                    .and_then(|t| t.arg(2))
            })
            .unwrap_or_default()
            .to_string();
        let pads = fp
            .children("pad")
            .filter_map(|pad| {
                let number = pad.arg(1).unwrap_or_default();
                (!number.is_empty()).then(|| Pad {
                    number: number.to_string(),
                    kind: pad.arg(2).unwrap_or_default().to_string(),
                    net: pad_net(pad),
                })
            })
            .collect();
        out.push(Footprint {
            reference,
            lib_id: fp.arg(1).unwrap_or_default().to_string(),
            pads,
        });
    }
    Ok(out)
}

pub fn load_pad_nets(path: &Path) -> Result<Vec<PadNet>, String> {
    let mut out: Vec<PadNet> = load_footprints(path)?
        .into_iter()
        .flat_map(|fp| {
            let reference = fp.reference;
            fp.pads.into_iter().map(move |p| PadNet {
                reference: reference.clone(),
                pad: p.number,
                net: p.net,
            })
        })
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

/// KiCad 10 writes `(net "name")`; older versions write `(net 3 "name")`.
fn pad_net(pad: &Sexp) -> Option<String> {
    let net = pad.child("net")?;
    let name = if net.items().len() >= 3 { net.arg(2) } else { net.arg(1) };
    name.filter(|n| !n.is_empty()).map(str::to_string)
}
