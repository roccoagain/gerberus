//! Checks the KiCad reader against real boards.
//!
//! Each board's `.kicad_pcb` was written by KiCad from the same schematic (DRC reports 0 parity errors), so
//! its pad nets are KiCad's own netlist. The schematic reader must agree with it pin for pin, name for name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gerberus::kicad;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn assert_matches_pcb(sch: &str, pcb: &str) {
    let design = kicad::load(&fixture(sch)).unwrap();
    assert!(design.warnings.is_empty(), "warnings: {:#?}", design.warnings);
    let pads = kicad::pcb::load_pad_nets(&fixture(pcb)).unwrap();

    let mut problems = Vec::new();
    let mut pcb_nets: BTreeMap<(String, String), String> = BTreeMap::new();
    for pad in pads {
        let Some(net) = pad.net else { continue };
        pcb_nets.insert((pad.reference, pad.pad), net);
    }
    for ((r, p), want) in &pcb_nets {
        match design.pin_net(r, p) {
            Some(got) if &got.name == want => {}
            Some(got) => problems.push(format!("{r}.{p}: schematic says {:?}, board says {want:?}", got.name)),
            None => problems.push(format!(
                "{r}.{p}: on net {want:?} in the board but missing from the schematic"
            )),
        }
    }
    for (r, c) in &design.components {
        if !c.on_board {
            continue;
        }
        for p in c.pins.keys() {
            if !pcb_nets.contains_key(&(r.clone(), p.clone())) {
                let net = &design.nets[c.pins[p].net].name;
                problems.push(format!(
                    "{r}.{p}: on {net:?} in the schematic but has no net in the board"
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "{} mismatches:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn sniffer_matches_board() {
    assert_matches_pcb("sniffer/sniffer.kicad_sch", "sniffer/sniffer.kicad_pcb");
}

#[test]
fn can_encoder_matches_board() {
    assert_matches_pcb("can-encoder/can-encoder.kicad_sch", "can-encoder/can-encoder.kicad_pcb");
}

#[test]
fn can_encoder_rev1_matches_board() {
    assert_matches_pcb(
        "can-encoder-rev1/can-encoder.kicad_sch",
        "can-encoder-rev1/can-encoder.kicad_pcb",
    );
}

#[test]
fn testbed_matches_board() {
    assert_matches_pcb("testbed/testbed.kicad_sch", "testbed/testbed.kicad_pcb");
}

/// `expect.md` lists `REF.PIN = NET` (pin is on that net) and `A.1 == B.2` (two pins share a net).
#[test]
fn testbed_meets_expectations() {
    let design = kicad::load(&fixture("testbed/testbed.kicad_sch")).unwrap();
    let text = std::fs::read_to_string(fixture("testbed/expect.md")).unwrap();
    let pin = |s: &str| {
        let (r, p) = s.trim().split_once('.').unwrap();
        design
            .pin_net(r, p)
            .unwrap_or_else(|| panic!("{s} not found"))
            .name
            .clone()
    };
    let mut checked = 0;
    for line in text.lines() {
        let line = line.trim().trim_start_matches("- ").trim_matches('`');
        if let Some((a, b)) = line.split_once("==") {
            assert_eq!(pin(a), pin(b), "{line}");
        } else if let Some((a, net)) = line.split_once('=') {
            assert_eq!(pin(a), net.trim(), "{line}");
        } else {
            continue;
        }
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[test]
fn reads_hierarchical_sheets() {
    let design = kicad::load(&fixture("edge/edge.kicad_sch")).unwrap();
    // R2 is on the root sheet; R1 and R3 live on the child sheet and take their references from its path.
    let refs: Vec<&str> = design.components.keys().map(String::as_str).collect();
    assert_eq!(refs, ["R1", "R2", "R3"]);
    // R2.2 meets the wire labelled W through a junction in the middle of that wire.
    assert_eq!(design.pin_net("R2", "2").unwrap().name, "/W");
    // The child's resistors are not wired to anything.
    assert_eq!(design.pin_net("R1", "1").unwrap().name, "unconnected-(R1-Pad1)");
}

/// A hand-written two-sheet design: a sheet pin wired to R1 meets a hierarchical label on R2 in the child,
/// and a global label of the same name on each sheet joins R1.2 to R2.2.
#[test]
fn joins_sheet_pins_and_global_labels() {
    let design = kicad::load(&fixture("hier/root.kicad_sch")).unwrap();
    let sig = design.pin_net("R1", "1").unwrap();
    assert_eq!(
        sig.pins,
        [("R1".to_string(), "1".to_string()), ("R2".to_string(), "1".to_string())]
    );
    // KiCad ranks a hierarchical label above a sheet pin, so the child's label names the net.
    assert_eq!(sig.name, "/child/SIG");
    let g = design.pin_net("R2", "2").unwrap();
    assert_eq!(g.name, "G");
    assert_eq!(g.pins.len(), 2);
}
