//! Checks the tscircuit reader against Circuit JSON that the tscircuit CLI (`tsci export`) wrote.
//!
//! tscircuit works out its own connectivity and writes it on each port as `subcircuit_connectivity_map_key`: ports
//! with the same key are on one net. The reader joins ports from the traces instead, so the two must agree pin
//! for pin, except that the reader also joins pins a part connects inside itself, which tscircuit's keys leave
//! apart.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gerberus::design::{Design, PinKind};
use gerberus::tscircuit;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tscircuit")
        .join(rel)
}

/// Every net as its set of `(part name, pin number)`, from the design.
fn design_nets(design: &Design) -> BTreeSet<BTreeSet<(String, String)>> {
    design.nets.iter().map(|n| n.pins.iter().cloned().collect()).collect()
}

/// Every net as tscircuit's connectivity keys group the ports, with each part's internally connected pins
/// joined.
fn tscircuit_nets(path: &Path) -> BTreeSet<BTreeSet<(String, String)>> {
    let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let elements = json.as_array().unwrap();
    let get = |e: &serde_json::Value, k: &str| e.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let group_names: BTreeMap<String, String> = elements
        .iter()
        .filter(|e| get(e, "type").as_deref() == Some("source_group"))
        .filter_map(|e| Some((get(e, "source_group_id")?, get(e, "name")?)))
        .collect();
    let parts: Vec<&serde_json::Value> = elements
        .iter()
        .filter(|e| get(e, "type").as_deref() == Some("source_component"))
        .collect();
    // The fixtures' groups sit on the board itself, so a repeated part name gets its one group's name.
    let reference = |component: &str| {
        let c = parts
            .iter()
            .find(|c| get(c, "source_component_id").as_deref() == Some(component))
            .unwrap();
        let name = get(c, "name").unwrap();
        let repeated = parts
            .iter()
            .filter(|o| get(o, "name").as_deref() == Some(name.as_str()))
            .count()
            > 1;
        match get(c, "source_group_id").and_then(|g| group_names.get(&g).cloned()) {
            Some(group) if repeated => format!("{name}_{group}"),
            _ => name,
        }
    };
    let mut by_key: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut of_port: BTreeMap<String, String> = BTreeMap::new();
    // Only part ports: a placed via has ports too, which join nothing.
    let part_ids: BTreeSet<String> = parts.iter().filter_map(|c| get(c, "source_component_id")).collect();
    let ports = elements.iter().filter(|e| {
        get(e, "type").as_deref() == Some("source_port")
            && get(e, "source_component_id").is_some_and(|c| part_ids.contains(&c))
    });
    for p in ports {
        let id = get(p, "source_port_id").unwrap();
        let pin = (
            reference(&get(p, "source_component_id").unwrap()),
            p["pin_number"]
                .as_u64()
                .map_or_else(|| get(p, "name").unwrap(), |n| n.to_string()),
        );
        let key = get(p, "subcircuit_connectivity_map_key").unwrap_or_else(|| format!("alone {id}"));
        by_key.entry(key.clone()).or_default().insert(pin);
        of_port.insert(id, key);
    }
    for c in elements
        .iter()
        .filter(|e| get(e, "type").as_deref() == Some("source_component"))
    {
        for set in c
            .get("internally_connected_source_port_ids")
            .and_then(|s| s.as_array())
            .into_iter()
            .flatten()
        {
            let keys: Vec<String> = set
                .as_array()
                .unwrap()
                .iter()
                .map(|p| of_port[p.as_str().unwrap()].clone())
                .collect();
            let mut joined = BTreeSet::new();
            for k in &keys {
                joined.extend(by_key.remove(k).unwrap_or_default());
            }
            by_key.insert(keys[0].clone(), joined);
            for p in set.as_array().unwrap() {
                of_port.insert(p.as_str().unwrap().to_string(), keys[0].clone());
            }
        }
    }
    by_key.into_values().collect()
}

fn assert_matches_tscircuit(rel: &str) {
    let path = fixture(rel);
    let design = tscircuit::load(&path).unwrap();
    let ours = design_nets(&design);
    let theirs = tscircuit_nets(&path);
    let only_ours: Vec<_> = ours.difference(&theirs).collect();
    let only_theirs: Vec<_> = theirs.difference(&ours).collect();
    assert!(
        only_ours.is_empty() && only_theirs.is_empty(),
        "nets only the reader has: {only_ours:#?}\nnets only tscircuit has: {only_theirs:#?}"
    );
}

#[test]
fn parts_board_matches_tscircuit_connectivity() {
    assert_matches_tscircuit("parts/parts.circuit.json");
}

#[test]
fn ldo_board_matches_tscircuit_connectivity() {
    assert_matches_tscircuit("ldo/ldo.circuit.json");
}

#[test]
fn parts_get_kicad_symbols_values_and_pin_names() {
    let d = tscircuit::load(&fixture("parts/parts.circuit.json")).unwrap();
    let symbol = |r: &str| d.components[r].lib_id.as_str();
    assert_eq!(symbol("D1"), "Device:D");
    assert_eq!(symbol("LED1"), "Device:LED");
    assert_eq!(symbol("Q1"), "Device:Q_NMOS");
    assert_eq!(symbol("Q2"), "Device:Q_PNP");
    assert_eq!(symbol("SW1"), "Switch:SW_Push");
    assert_eq!(symbol("C2"), "Device:CP");
    assert_eq!(symbol("C3"), "Device:C");
    assert_eq!(symbol("Y1"), "Device:Crystal");
    assert_eq!(symbol("J1"), "Connector:Conn");
    assert_eq!(symbol("L1"), "Device:L");
    assert_eq!(symbol("F1"), "Device:Fuse");
    assert_eq!(symbol("TP1"), "Connector:TestPoint");
    assert_eq!(symbol("U1"), "tscircuit:simple_chip");

    let c2 = &d.components["C2"];
    assert_eq!((c2.value.as_str(), c2.fields["Voltage"].as_str()), ("10uF", "16V"));
    assert_eq!(d.components["C3"].fields["LCSC"], "C307331");
    assert_eq!(d.components["C3"].footprint, "cap0402_nonpolarized");
    assert_eq!(d.components["D1"].fields["MPN"], "B5819WS");
    assert_eq!(d.components["LED1"].value, "LED");
    assert!(d.components.values().all(|c| c.on_board && c.in_bom && !c.dnp));

    let pin = |r: &str, n: &str| &d.components[r].pins[n];
    // tscircuit numbers a diode anode first.
    assert_eq!((pin("D1", "1").name.as_str(), pin("D1", "2").name.as_str()), ("A", "K"));
    assert_eq!(pin("Q1", "3").name, "G");
    assert_eq!(pin("Q2", "2").name, "B");
    assert_eq!(pin("R1_A", "1").name, "");
    assert_eq!(pin("U1", "1").kind, PinKind::PowerIn);
    assert_eq!(pin("U1", "2").kind, PinKind::PowerIn);
    assert_eq!(pin("U1", "3").kind, PinKind::Input);
    assert_eq!(pin("U1", "4").kind, PinKind::PowerOut);
    assert_eq!(pin("U1", "6").kind, PinKind::Bidirectional);
    assert_eq!(pin("U1", "7").kind, PinKind::Unspecified);
    assert_eq!(pin("J1", "1").kind, PinKind::Passive);
    assert!(pin("U1", "5").no_connect);
}

#[test]
fn nets_are_named_by_subcircuit_and_pins() {
    let d = tscircuit::load(&fixture("parts/parts.circuit.json")).unwrap();
    let net = |r: &str, n: &str| d.pin_net(r, n).unwrap().name.as_str();
    assert_eq!(net("U1", "1"), "VIN");
    assert_eq!(net("C2", "1"), "VIN");
    assert_eq!(net("U1", "4"), "VCC");
    // A group used twice: its parts are told apart by group, and its nets are its own.
    assert_eq!(
        d.warnings,
        [
            "2 parts are named C1; they are called C1_A, C1_B here",
            "2 parts are named R1; they are called R1_A, R1_B here"
        ]
    );
    assert_eq!(net("R1_A", "1"), "/A/VCC");
    assert_eq!(net("R1_B", "1"), "/B/VCC");
    assert_eq!(net("C1_A", "1"), "/A/SIG");
    assert_eq!(net("J1", "1"), "Net-(J1-VBUS)");
    assert_eq!(net("U1", "7"), "unconnected-(U1-Pad7)");
    // The push button joins pins 1 and 2 inside itself.
    assert_eq!(net("SW1", "1"), net("SW1", "2"));
    assert_eq!(d.pin_net("Q2", "2"), d.pin_net("U1", "3"));
}

#[test]
fn without_inputs_the_boards_own_ground_is_0_v() {
    let d = tscircuit::load(&fixture("parts/parts.circuit.json")).unwrap();
    let inputs = gerberus::inputs::BoardInputs::none("parts", &d);
    let pin = &inputs.inputs[0].pin;
    assert_eq!(d.pin_net(&pin.reference, &pin.pin).unwrap().name, "GND");
}

#[test]
fn pinout_finds_the_diode_numbered_unlike_its_datasheet() {
    let d = tscircuit::load(&fixture("parts/parts.circuit.json")).unwrap();
    let lib = gerberus::limits::Library::builtin().unwrap();
    let report = gerberus::pinout::check(&d, None, &lib);
    let d1: Vec<&str> = report
        .findings
        .iter()
        .filter(|f| f.reference == "D1")
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        d1.iter()
            .any(|m| m.contains("the datasheet puts A on pin 2 and K on pin 1")),
        "{d1:#?}"
    );
}

#[test]
fn check_runs_on_a_tscircuit_board() {
    let path = fixture("ldo/ldo.circuit.json");
    let design = tscircuit::load(&path).unwrap();
    assert!(design.warnings.is_empty(), "{:?}", design.warnings);
    let mut lib = gerberus::limits::Library::builtin().unwrap();
    lib.expand_families(&design);
    let inputs = gerberus::inputs::BoardInputs::load(&fixture("ldo/inputs.toml")).unwrap();
    assert!(inputs.problems(&design).is_empty());
    let v = gerberus::voltage::compute(&design, &lib, &inputs);
    let policy = gerberus::policy::Policy::builtin().unwrap();
    let report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    assert_eq!(report.errors(), 0);
    let passes: Vec<String> = report
        .findings
        .iter()
        .filter(|f| f.verdict == gerberus::finding::Verdict::Pass)
        .map(|f| format!("{}: {}", f.rule, f.message))
        .collect();
    for want in [
        "abs-max: U1.VIN (VBUS): 4.75 to 5.5 V",
        "regulator-headroom: U1 (AMS1117-3.3) sets V3_3",
        "cap-voltage: C2.1 (V3_3): 3.166 to 3.434 V",
    ] {
        assert!(
            passes.iter().any(|p| p.starts_with(want)),
            "no pass {want:?} in {passes:#?}"
        );
    }
}

/// `tsci export` on the board's `.tsx`. A board is built inside its tscircuit project, so the copy goes in a folder
/// of `ci/tscircuit`, the project CI installs (`npm ci --prefix ci/tscircuit`), which also keeps the files tsci
/// leaves beside a board out of the fixtures. This needs the tscircuit CLI and Bun.
#[test]
fn reads_a_tsx_board_through_the_tscircuit_cli() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("ci/tscircuit/test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let tsx = dir.join("ldo.circuit.tsx");
    std::fs::copy(fixture("ldo/ldo.circuit.tsx"), &tsx).unwrap();
    let built = tscircuit::build(&tsx);
    let _ = std::fs::remove_dir_all(&dir);
    let built = built.unwrap();
    let saved = tscircuit::load(&fixture("ldo/ldo.circuit.json")).unwrap();
    assert_eq!(design_nets(&built), design_nets(&saved));
    let names = |d: &Design| d.nets.iter().map(|n| n.name.clone()).collect::<BTreeSet<_>>();
    assert_eq!(names(&built), names(&saved));
}
