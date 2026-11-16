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

pub fn load_pad_nets(path: &Path) -> Result<Vec<PadNet>, String> {
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
        for pad in fp.children("pad") {
            let num = pad.arg(1).unwrap_or_default();
            if num.is_empty() {
                continue;
            }
            out.push(PadNet {
                reference: reference.clone(),
                pad: num.to_string(),
                net: pad_net(pad),
            });
        }
    }
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
