//! Symbol, footprint and datasheet pinout agreement, on the real boards and on boards broken on purpose.
use std::path::{Path, PathBuf};

use gerberus::design::{Design, Pin, PinKind};
use gerberus::kicad;
use gerberus::kicad::pcb::{Footprint, Pad};
use gerberus::limits::Library;
use gerberus::pinout::{Report, Severity, check};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn library() -> Library {
    Library::load_dir(&root("parts")).unwrap()
}

fn board(dir: &str, name: &str) -> (Design, Vec<Footprint>) {
    let base = root(&format!("tests/fixtures/{dir}/{name}"));
    let design = kicad::load(&base.with_extension("kicad_sch")).unwrap();
    let fps = kicad::pcb::load_footprints(&base.with_extension("kicad_pcb")).unwrap();
    (design, fps)
}

fn run(design: &Design, fps: &[Footprint], lib: &Library) -> Report {
    check(design, Some(fps), lib)
}

fn messages(report: &Report, severity: Severity) -> Vec<String> {
    report
        .findings
        .iter()
        .filter(|f| f.severity == severity)
        .map(|f| format!("{}: {}", f.reference, f.message))
        .collect()
}

fn has(report: &Report, reference: &str, severity: Severity, needle: &str) -> bool {
    report
        .for_ref(reference)
        .any(|f| f.severity == severity && f.message.contains(needle))
}

fn footprint_mut<'a>(fps: &'a mut [Footprint], reference: &str) -> &'a mut Footprint {
    fps.iter_mut().find(|f| f.reference == reference).unwrap()
}

/// The real boards have no pinout errors. Their warnings are each a real difference a person should see,
/// listed here so a new one shows up as a test failure.
#[test]
fn real_boards_have_no_errors_and_known_warnings() {
    let lib = library();
    let cases: [(&str, &str, &[&str]); 4] = [
        ("can-encoder", "can-encoder", &["D2", "U4"]),
        ("can-encoder-rev1", "can-encoder", &["D2", "U5"]),
        ("sniffer", "sniffer", &["U3"]),
        ("testbed", "testbed", &[]),
    ];
    for (dir, name, warned) in cases {
        let (design, fps) = board(dir, name);
        let report = run(&design, &fps, &lib);
        assert_eq!(messages(&report, Severity::Error), Vec::<String>::new(), "{dir}");
        let mut refs: Vec<&str> = report.findings.iter().map(|f| f.reference.as_str()).collect();
        refs.dedup();
        assert_eq!(refs, warned, "{dir}: {:#?}", report.findings);
        for (r, c) in &report.coverage {
            if design.components[r].on_board {
                assert!(c.footprint, "{dir} {r} footprint not compared");
            }
        }
    }
}

#[test]
fn unidirectional_tvs_on_a_bidirectional_symbol_names_its_cathode_net() {
    let (design, fps) = board("can-encoder-rev1", "can-encoder");
    let report = run(&design, &fps, &library());
    assert!(has(&report, "D2", Severity::Warning, "cathode (K) is pad 1 on +12V"));
}

#[test]
fn sensor_supply_pin_named_after_another_pin_is_a_warning_not_a_swap() {
    // SCD41 pin 19 is VDDH; the symbol calls it VDD, which is also pin 7's name.
    let (design, fps) = board("sniffer", "sniffer");
    let report = run(&design, &fps, &library());
    assert!(has(&report, "U3", Severity::Warning, "19 \"VDD\" (datasheet VDDH)"));
}

/// A differential pair swapped on the symbol must not hide behind the `OUT`/`IN` synonyms.
#[test]
fn swapped_differential_pair_is_an_error() {
    let (mut design, fps) = board("can-encoder", "can-encoder");
    let mut lib = library();
    let part = lib.parts.get_mut("TCAN1044AVDDFRQ1").unwrap();
    part.pins.insert("6".into(), "OUT-".into());
    part.pins.insert("7".into(), "OUT+".into());
    let u4 = design.components.get_mut("U4").unwrap();
    u4.pins.get_mut("6").unwrap().name = "OUT+".into();
    u4.pins.get_mut("7").unwrap().name = "OUT-".into();
    let report = run(&design, &fps, &lib);
    assert!(
        has(&report, "U4", Severity::Error, "look swapped"),
        "{:#?}",
        report.findings
    );
}

#[test]
fn swapped_datasheet_pins_are_an_error() {
    let (design, fps) = board("can-encoder", "can-encoder");
    let mut lib = library();
    let part = lib.parts.get_mut("TCAN1044AVDDFRQ1").unwrap();
    part.pins.insert("6".into(), "CANH".into());
    part.pins.insert("7".into(), "CANL".into());
    let report = run(&design, &fps, &lib);
    assert!(
        has(&report, "U4", Severity::Error, "look swapped"),
        "{:#?}",
        report.findings
    );
}

#[test]
fn symbol_pin_without_a_pad_is_an_error() {
    let (design, mut fps) = board("can-encoder", "can-encoder");
    footprint_mut(&mut fps, "U4").pads.retain(|p| p.number != "3");
    let report = run(&design, &fps, &library());
    assert!(has(&report, "U4", Severity::Error, "symbol pin 3 (VCC) has no pad"));
    assert!(has(&report, "U4", Severity::Error, "no pad for datasheet pin 3 (VCC)"));
}

#[test]
fn datasheet_pin_missing_from_the_symbol_is_an_error() {
    let (mut design, fps) = board("can-encoder", "can-encoder");
    design.components.get_mut("U4").unwrap().pins.remove("2");
    let report = run(&design, &fps, &library());
    assert!(has(
        &report,
        "U4",
        Severity::Error,
        "datasheet pin 2 (GND) of TCAN1044AVDDFRQ1 is missing from the symbol, so its pad is left unconnected"
    ));
}

#[test]
fn footprint_for_another_package_is_reported() {
    // A SOT-23-5 footprint under a SOT-23-3 regulator: two pads the datasheet doesn't have.
    let (design, mut fps) = board("can-encoder", "can-encoder");
    let fp = footprint_mut(&mut fps, "U5");
    for n in ["4", "5"] {
        fp.pads.push(Pad {
            number: n.into(),
            kind: "smd".into(),
            net: None,
        });
    }
    let report = run(&design, &fps, &library());
    assert!(has(
        &report,
        "U5",
        Severity::Warning,
        "pad 4, 5 that is not a pin of XC6206P332MR-G"
    ));
}

#[test]
fn symbol_pin_the_datasheet_does_not_have_is_an_error() {
    let (mut design, fps) = board("can-encoder", "can-encoder");
    let u5 = design.components.get_mut("U5").unwrap();
    let mut extra = u5.pins["1"].clone();
    extra.number = "4".into();
    extra.name = "NC".into();
    u5.pins.insert("4".into(), extra);
    let report = run(&design, &fps, &library());
    assert!(has(
        &report,
        "U5",
        Severity::Error,
        "symbol pin 4 (NC) is not a pin of XC6206P332MR-G"
    ));
    assert!(has(&report, "U5", Severity::Error, "symbol pin 4 (NC) has no pad"));
}

#[test]
fn do_not_connect_pin_on_a_net_is_an_error() {
    // Give the SCD41's DNC pin 1 a symbol pin on the GND net.
    let (mut design, fps) = board("sniffer", "sniffer");
    let gnd = design.components["U3"].pins["20"].net;
    design.components.get_mut("U3").unwrap().pins.insert(
        "1".into(),
        Pin {
            number: "1".into(),
            name: "DNC".into(),
            kind: PinKind::NoConnect,
            no_connect: false,
            stack: None,
            net: gnd,
        },
    );
    design.nets[gnd].pins.push(("U3".into(), "1".into()));
    let report = run(&design, &fps, &library());
    assert!(has(
        &report,
        "U3",
        Severity::Error,
        "datasheet pin 1 (DNC) of SCD41-D-R2 must not be connected"
    ));
}

#[test]
fn board_and_schematic_out_of_step() {
    let (mut design, mut fps) = board("testbed", "testbed");
    fps.retain(|f| f.reference != "R2");
    footprint_mut(&mut fps, "R1").lib_id = "Resistor_SMD:R_0402_1005Metric".into();
    let tp = |reference: &str, board_only: bool| Footprint {
        reference: reference.into(),
        lib_id: "TestPoint:TestPoint_Pad_D1.0mm".into(),
        board_only,
        pads: vec![Pad {
            number: "1".into(),
            kind: "smd".into(),
            net: None,
        }],
    };
    fps.push(tp("TP9", false));
    fps.push(tp("TP10", true));
    design.components.get_mut("C1").unwrap().pins.remove("2");
    let report = run(&design, &fps, &library());
    assert!(has(&report, "R2", Severity::Error, "no footprint on the PCB"));
    assert!(has(
        &report,
        "R1",
        Severity::Warning,
        "the board has Resistor_SMD:R_0402_1005Metric"
    ));
    assert!(has(&report, "TP9", Severity::Warning, "no symbol in the schematic"));
    assert_eq!(
        report.for_ref("TP10").count(),
        0,
        "board-only footprints belong only on the board"
    );
    assert!(has(
        &report,
        "C1",
        Severity::Warning,
        "pad 2 of Capacitor_SMD:C_0603_1608Metric has no symbol pin"
    ));
}

#[test]
fn without_a_board_only_the_datasheet_is_compared() {
    let (design, _) = board("can-encoder", "can-encoder");
    let report = check(&design, None, &library());
    assert_eq!(report.errors(), 0);
    assert!(report.coverage.values().all(|c| !c.footprint));
    assert_eq!(report.coverage["U2"].datasheet.as_deref(), Some("STM32F042G6U6TR"));
}

#[test]
fn mounting_pads_need_no_symbol_pin() {
    // JST connectors have two MP pads that are not in the symbol or the part file.
    let (design, fps) = board("can-encoder", "can-encoder");
    assert!(
        footprint_mut(&mut fps.clone(), "J1")
            .pads
            .iter()
            .any(|p| p.number == "MP")
    );
    let report = run(&design, &fps, &library());
    assert_eq!(report.for_ref("J1").count(), 0);
}
