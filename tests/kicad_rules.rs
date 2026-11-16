//! Small hand-written schematics, one per KiCad connectivity or naming rule.
//!
//! Each rule follows KiCad 10's source (`connection_graph.cpp`, `sch_pin.cpp`, `sch_symbol.cpp`). None of these
//! cases appear in the real boards under `tests/fixtures`, so they are pinned down here.

use std::path::PathBuf;

use gerberus::design::Design;
use gerberus::kicad;

const ROOT: &str = "00000000-0000-0000-0000-000000000001";

const LIBS: &str = r#"(lib_symbols
  (symbol "Device:R"
    (symbol "R_1_1"
      (pin passive line (at 0 3.81 270) (length 1.27) (name "~") (number "1"))
      (pin passive line (at 0 -3.81 90) (length 1.27) (name "~") (number "2"))))
  (symbol "Test:IC"
    (symbol "IC_1_1"
      (pin input line (at -5.08 0 0) (length 2.54) (name "IN") (number "1"))
      (pin power_in line (at 0 5.08 270) (length 2.54) (hide yes) (name "AVDD") (number "8"))))
  (symbol "power:VDD" (power global)
    (symbol "VDD_0_1"
      (pin power_in line (at 0 0 90) (length 0) (hide yes) (name "VDD") (number "1"))))
  (symbol "Test:Q"
    (symbol "Q_1_1"
      (pin passive line (at -5.08 0 0) (length 2.54) (name "S") (number "1"))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "S") (number "2"))
      (pin passive line (at 0 5.08 270) (length 2.54) (name "3") (number "3"))))
  (symbol "Test:DUAL"
    (symbol "DUAL_1_1"
      (pin output line (at 5.08 0 180) (length 2.54) (name "OUT") (number "1")))
    (symbol "DUAL_2_1"
      (pin output line (at 5.08 0 180) (length 2.54) (name "OUT") (number "7"))))
  (symbol "Test:JP"
    (duplicate_pin_numbers_are_jumpers yes)
    (jumper_pin_groups ("2" "3"))
    (symbol "JP_1_1"
      (pin passive line (at -5.08 0 0) (length 2.54) (name "~") (number "1"))
      (pin passive line (at 5.08 0 180) (length 2.54) (name "~") (number "1"))
      (pin passive line (at -5.08 -2.54 0) (length 2.54) (name "~") (number "2"))
      (pin passive line (at 5.08 -2.54 180) (length 2.54) (name "~") (number "3")))))"#;

fn sym(lib: &str, reference: &str, x: f64, y: f64) -> String {
    sym_unit(lib, reference, x, y, 1)
}

fn sym_unit(lib: &str, reference: &str, x: f64, y: f64, unit: u32) -> String {
    // A power symbol's value is its net name.
    let value = lib.strip_prefix("power:").unwrap_or(reference);
    format!(
        r#"(symbol (lib_id "{lib}") (at {x} {y} 0) (unit {unit}) (uuid "{reference}-{x}-{y}")
  (property "Reference" "{reference}") (property "Value" "{value}")
  (instances (project "t" (path "/{ROOT}" (reference "{reference}") (unit {unit})))))"#
    )
}

fn load(body: &[String]) -> Design {
    let text = format!(
        "(kicad_sch (version 20260306) (uuid \"{ROOT}\")\n{LIBS}\n{}\n)",
        body.join("\n")
    );
    let dir = std::env::temp_dir().join(format!("gerberus-rules-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path: PathBuf = dir.join(format!("{:016x}.kicad_sch", hash(&text)));
    std::fs::write(&path, text).unwrap();
    kicad::load(&path).unwrap()
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn wire(a: (f64, f64), b: (f64, f64)) -> String {
    format!("(wire (pts (xy {} {}) (xy {} {})))", a.0, a.1, b.0, b.1)
}

fn net<'a>(d: &'a Design, r: &str, p: &str) -> &'a gerberus::design::Net {
    d.pin_net(r, p).unwrap_or_else(|| panic!("{r}.{p} has no net"))
}

/// A hidden power-input pin on a part stays a pin of the part and joins the global net named after it. When a
/// power symbol sits on the same net, KiCad names the net after the power symbol even if the pin's name sorts first.
#[test]
fn hidden_power_pins_stay_on_the_part() {
    let d = load(&[sym("Test:IC", "U1", 50.0, 50.0), sym("power:VDD", "#PWR1", 50.0, 44.92)]);
    let n = net(&d, "U1", "8");
    assert_eq!(n.name, "VDD");
    assert_eq!(n.pins, [("U1".to_string(), "8".to_string())]);
    assert!(d.components["U1"].pins.contains_key("8"));
}

/// KiCad joins a local label and a hierarchical label with the same name on one sheet.
#[test]
fn local_and_hierarchical_labels_join_by_name() {
    let d = load(&[
        sym("Device:R", "R1", 50.0, 50.0),
        sym("Device:R", "R2", 70.0, 50.0),
        "(label \"X\" (at 50 46.19 0))".to_string(),
        "(hierarchical_label \"X\" (shape input) (at 70 46.19 0))".to_string(),
    ]);
    assert_eq!(net(&d, "R1", "1").pins, net(&d, "R2", "1").pins);
    assert_eq!(d.nets.iter().filter(|n| n.name == "/X").count(), 1);
}

/// A pin touching the middle of a wire is not connected unless a junction sits there.
#[test]
fn mid_wire_needs_a_junction() {
    let parts = [
        sym("Device:R", "R1", 50.0, 50.0),
        sym("Device:R", "R2", 80.0, 50.0),
        wire((40.0, 46.19), (80.0, 46.19)),
    ];
    let d = load(&parts);
    assert_eq!(net(&d, "R1", "1").name, "unconnected-(R1-Pad1)");
    assert_eq!(net(&d, "R2", "1").name, "unconnected-(R2-Pad1)");

    let mut with_junction = parts.to_vec();
    with_junction.push("(junction (at 50 46.19))".to_string());
    let d = load(&with_junction);
    assert_eq!(net(&d, "R1", "1").pins, net(&d, "R2", "1").pins);
}

/// A label on the middle of a single wire does not connect to it; on the crossing of two wires it joins both.
#[test]
fn labels_join_wires_only_where_two_pass() {
    let one = [
        sym("Device:R", "R1", 80.0, 50.0),
        wire((40.0, 46.19), (80.0, 46.19)),
        "(label \"L\" (at 60 46.19 0))".into(),
    ];
    let d = load(&one);
    assert_eq!(net(&d, "R1", "1").name, "unconnected-(R1-Pad1)");

    let two = [
        sym("Device:R", "R1", 80.0, 50.0),
        sym("Device:R", "R2", 60.0, 80.0),
        wire((40.0, 46.19), (80.0, 46.19)),
        wire((60.0, 30.0), (60.0, 76.19)),
        "(label \"L\" (at 60 46.19 0))".into(),
    ];
    let d = load(&two);
    assert_eq!(net(&d, "R1", "1").name, "/L");
    assert_eq!(net(&d, "R2", "1").name, "/L");
}

/// Names KiCad builds from pins: `-Pad<n>` for repeated pin names, the pad form when a pin is named after its
/// number, and the unit letter for multi-unit parts.
#[test]
fn generated_names_follow_kicad() {
    let d = load(&[
        sym("Test:Q", "Q1", 50.0, 50.0),
        sym("Device:R", "R1", 40.0, 53.81),
        wire((44.92, 50.0), (40.0, 50.0)),
        sym_unit("Test:DUAL", "U2", 100.0, 50.0, 1),
        sym_unit("Test:DUAL", "U2", 100.0, 80.0, 2),
        sym("Device:R", "R2", 110.0, 53.81),
        wire((105.08, 50.0), (110.0, 50.0)),
    ]);
    assert_eq!(net(&d, "Q1", "1").name, "Net-(Q1-S-Pad1)");
    assert_eq!(net(&d, "Q1", "3").name, "unconnected-(Q1-Pad3)");
    assert_eq!(net(&d, "U2", "1").name, "Net-(U2A-OUT)");
    assert_eq!(net(&d, "U2", "7").name, "unconnected-(U2B-OUT-Pad7)");
}

/// KiCad 10 jumper symbols join pins inside the part.
#[test]
fn jumper_pins_join() {
    let d = load(&[
        sym("Test:JP", "JP1", 50.0, 50.0),
        "(label \"A\" (at 44.92 50 0))".into(),
        "(label \"B\" (at 55.08 50 0))".into(),
        "(label \"C\" (at 44.92 52.54 0))".into(),
        "(label \"D\" (at 55.08 52.54 0))".into(),
    ]);
    assert!(d.warnings.is_empty(), "{:?}", d.warnings);
    let one = net(&d, "JP1", "1");
    assert_eq!(one.labels, ["A", "B"]);
    assert_eq!(net(&d, "JP1", "2").labels, ["C", "D"]);
    assert_eq!(net(&d, "JP1", "2").pins, net(&d, "JP1", "3").pins);
}

/// Unannotated or repeated references are reported, since their pins get merged.
#[test]
fn duplicate_references_warn() {
    let d = load(&[sym("Device:R", "R?", 50.0, 50.0), sym("Device:R", "R?", 70.0, 50.0)]);
    assert!(
        d.warnings.iter().any(|w| w.contains("no reference")),
        "{:?}",
        d.warnings
    );
    assert!(
        d.warnings.iter().any(|w| w.contains("more than one symbol")),
        "{:?}",
        d.warnings
    );

    let d = load(&[sym("Device:R", "R1", 50.0, 50.0), sym("Device:R", "R1", 70.0, 50.0)]);
    assert!(
        d.warnings.iter().any(|w| w.contains("more than one symbol")),
        "{:?}",
        d.warnings
    );
}

/// A local label and a global power symbol with the same name on one sheet are one net, named after the power.
#[test]
fn local_label_joins_power_symbol_by_name() {
    let d = load(&[
        sym("Device:R", "R1", 50.0, 50.0),
        sym("power:VDD", "#PWR1", 50.0, 46.19),
        sym("Device:R", "R2", 70.0, 50.0),
        "(label \"VDD\" (at 70 46.19 0))".into(),
    ]);
    assert_eq!(net(&d, "R2", "1").name, "VDD");
    assert_eq!(net(&d, "R1", "1").pins, net(&d, "R2", "1").pins);
}

/// The power-symbol preference only applies inside one group. Across groups KiCad compares names, so a hidden
/// `AVDD` pin elsewhere renames the whole net `AVDD`.
#[test]
fn power_symbol_preference_stays_within_one_group() {
    let d = load(&[
        sym("Test:IC", "U1", 50.0, 50.0),
        sym("power:VDD", "#PWR1", 50.0, 44.92),
        sym("Test:IC", "U2", 100.0, 50.0),
    ]);
    let n = net(&d, "U1", "8");
    assert_eq!(n.name, "AVDD");
    assert_eq!(
        n.pins,
        [("U1".to_string(), "8".to_string()), ("U2".to_string(), "8".to_string())]
    );
}

/// Two jumpered pins that share a number are two pins to KiCad, so the net is not `unconnected-`.
#[test]
fn jumpered_pins_alone_are_not_unconnected() {
    let d = load(&[sym("Test:JP", "JP1", 50.0, 50.0)]);
    assert_eq!(net(&d, "JP1", "1").name, "Net-(JP1-Pad1)");
}
