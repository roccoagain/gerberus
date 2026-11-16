//! The first ten rules on the test boards. Rev 1's TVS is the golden test: its SMAJ18A sits on the 24 V bus
//! and conducts. The other two boards must come out with no errors.

use std::path::{Path, PathBuf};

use gerberus::design::{Component, Design, Net, Pin, PinKind};
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::policy::Policy;
use gerberus::rules::{RULES, Report, Severity, Verdict, check, render};
use gerberus::voltage::{Span, Voltages, compute};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn board(dir: &str, sch: &str) -> (Design, BoardInputs) {
    let d = root("tests/fixtures").join(dir);
    (
        kicad::load(&d.join(sch)).unwrap(),
        BoardInputs::load(&d.join("inputs.toml")).unwrap(),
    )
}

fn run_on(design: &Design, inputs: &BoardInputs) -> (Voltages, Report) {
    let lib = Library::load_dir(&root("parts")).unwrap();
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let v = compute(design, &lib, inputs);
    let r = check(design, &lib, &policy, inputs, &v);
    (v, r)
}

fn run(dir: &str, sch: &str) -> (Design, Voltages, Report) {
    let (d, inputs) = board(dir, sch);
    let (v, r) = run_on(&d, &inputs);
    (d, v, r)
}

fn fails(r: &Report) -> Vec<String> {
    r.findings
        .iter()
        .filter(|f| f.verdict == Verdict::Fail)
        .map(|f| format!("{} {} {}", f.severity, f.rule, f.message))
        .collect()
}

#[test]
fn rev1_tvs_conducts_on_the_bus() {
    let (d, v, r) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    let f: Vec<_> = r
        .by_rule("tvs-breakdown")
        .filter(|f| f.reference == "D2" && f.case.is_none())
        .collect();
    assert_eq!(f.len(), 1, "{f:?}");
    let f = f[0];
    assert_eq!((f.verdict, f.severity), (Verdict::Fail, Severity::Error));
    for want in ["+12V", "25.2 V", "20 V", "SMAJ18A", "p2"] {
        assert!(f.message.contains(want), "missing {want:?}: {}", f.message);
    }
    // The other errors are the termination resistor (its power and its rated voltage, √(P·R) = 2.74 V) and the
    // two VBUS-on-the-bus faults, and the report explains how +12V gets to 25.2 V.
    assert_eq!(r.errors(), 5, "{:#?}", fails(&r));
    let text = render(&d, &v, &r, false);
    for want in ["D1 (B5819W SL)", "input VBUS on J1.1", "5 errors"] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    // The stand-off is exceeded too, as a warning.
    assert!(
        r.by_rule("tvs-standoff")
            .any(|f| f.reference == "D2" && f.verdict == Verdict::Fail && f.severity == Severity::Warning)
    );
}

#[test]
fn rev2_and_sniffer_errors() {
    // Rev 2's only errors are its 0402 termination resistor's power and rated voltage (below). The sniffer's
    // are its supply connected backwards: nothing blocks VIN, so the ME6211's CE pin sees -5.5 V, and its output
    // sits above its reversed input. The reverse-polarity summary isn't counted again.
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    assert_eq!(r.errors(), 2, "{:#?}", fails(&r));
    let (_, _, r) = run("sniffer", "sniffer.kicad_sch");
    assert_eq!(r.errors(), 2, "{:#?}", fails(&r));
    let f: Vec<_> = r.by_rule("reverse-polarity").collect();
    assert!(
        f.len() == 1 && f[0].verdict == Verdict::Fail && f[0].message.contains("U1"),
        "{f:#?}"
    );
    assert!(r.findings.iter().any(|f| f.case.as_deref() == Some("VIN reversed")
        && f.rule == "abs-max"
        && f.verdict == Verdict::Fail
        && f.message.contains("U1.CE")));
}

/// The bus's differential voltage reaches 5 V (VOD(DOM) at the 2240 Ω arbitration load) across R4, the 120 Ω
/// termination, and 5 V over 117.2 Ω is 213.2 mW against the 0402's 62.5 mW. JP1's side of R4 only connects through R4 and JP1,
/// so it stays between CANH and CANL: whether JP1 is bridged or not, R4 sees at most CANH − CANL.
#[test]
fn termination_resistor_sees_the_bus_differential() {
    for dir in ["can-encoder", "can-encoder-rev1"] {
        let (_, _, r) = run(dir, "can-encoder.kicad_sch");
        let f: Vec<_> = r
            .by_rule("resistor-power")
            .filter(|f| f.reference == "R4" && f.case.is_none())
            .collect();
        assert_eq!(f.len(), 1, "{dir}: {f:?}");
        assert_eq!((f[0].verdict, f[0].severity), (Verdict::Fail, Severity::Error), "{dir}");
        for want in [
            "0 to 5 V across it",
            "213.2 mW",
            "62.5 mW",
            "stays between them",
            "kept to -0.12 to 5 V",
        ] {
            assert!(f[0].message.contains(want), "{dir}: missing {want:?}: {}", f[0].message);
        }
    }
}

/// Rev 1's own DESIGN.md says VBUS shorted onto CANH or CANL would damage its TCAN330 (bus pins ±14 V). Rev 2's
/// TCAN1044AV survives ±58 V. The NUP2105L's 26.2 V breakdown is above the 25.2 V bus, so it doesn't clamp.
#[test]
fn vbus_on_the_bus_faults() {
    let (d, v, r) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    for (fault, pin) in [
        ("VBUS shorted onto CANH", "U4.CANH"),
        ("VBUS shorted onto CANL", "U4.CANL"),
    ] {
        let f: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.case.as_deref() == Some(fault) && f.verdict == Verdict::Fail)
            .collect();
        assert_eq!(f.len(), 1, "{fault}: {f:?}");
        assert_eq!((f[0].rule, f[0].severity), ("abs-max", Severity::Error));
        for want in [pin, "25.2 V", "14 V", "TCAN330", "p5"] {
            assert!(f[0].message.contains(want), "missing {want:?}: {}", f[0].message);
        }
        assert!(
            r.findings.iter().any(|f| f.case.as_deref() == Some(fault)
                && f.rule == "tvs-breakdown"
                && f.reference == "U5"
                && f.verdict == Verdict::Pass),
            "{fault}: the NUP2105L should stay below breakdown"
        );
    }
    // The report explains the fault's net from the fault's own input.
    let text = render(&d, &v, &r, false);
    assert!(text.contains("input CANH shorted to VBUS on J1.2"), "{text}");

    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    let faults: Vec<_> = r.findings.iter().filter(|f| f.case.is_some()).collect();
    assert!(!faults.is_empty());
    // Everything passes but R4's power and rated voltage: with VBUS on one end they can't be proven either way,
    // as the two ends needn't move together in a fault.
    // VBUS connected backwards is blocked by D1, so nothing fails there; its unknowns are the pins at 0 V that
    // have no rated minimum.
    let (reversed, faults): (Vec<_>, Vec<_>) = faults
        .into_iter()
        .partition(|f| f.case.as_deref() == Some("VBUS reversed"));
    assert!(!reversed.is_empty() && reversed.iter().all(|f| f.verdict != Verdict::Fail));
    let (r4, rest): (Vec<&gerberus::rules::Finding>, Vec<_>) = faults
        .iter()
        .partition(|f| f.reference == "R4" && matches!(f.rule, "resistor-power" | "recommended"));
    assert!(rest.iter().all(|f| f.verdict == Verdict::Pass), "{rest:#?}");
    assert_eq!(r4.len(), 4, "{r4:#?}");
    assert!(
        r4.iter()
            .all(|f| f.verdict == Verdict::Unknown && f.message.contains("may overstate it"))
    );
    assert!(
        faults
            .iter()
            .any(|f| f.reference == "U4" && f.message.contains("7 to 26 V"))
    );
    // Only the damage rules run in a fault case, and passives' ratings, which never FAIL there.
    assert!(faults.iter().all(|f| {
        gerberus::rules::FAULT_RULES.contains(&f.rule)
            || (matches!(f.rule, "resistor-power" | "recommended")
                && (f.reference.starts_with('R') || f.reference.starts_with('J')))
    }));
}

/// A fault that only breaks the link between CANH and CANL still reports what reads both.
#[test]
fn a_fault_that_only_drops_a_differential_still_reports() {
    let (d, mut inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let mut unlinked = inputs.inputs.iter().find(|i| i.name == "CANH").unwrap().clone();
    unlinked.name = "CANH unlinked".into();
    unlinked.fault = Some("unlinked".into());
    inputs.inputs.push(unlinked);
    let (_, r) = run_on(&d, &inputs);
    let f: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.case.as_deref() == Some("unlinked"))
        .collect();
    assert!(
        f.iter()
            .any(|f| f.reference == "U4" && f.message.contains("U4.CANH (/CANH) from U4.CANL (/CANL)")),
        "{f:#?}"
    );
}

/// Rev 2's SMF26A replaced the SMAJ18A: 26 V stays below its 28.9 V breakdown and within its 26 V stand-off.
#[test]
fn rev2_tvs_passes() {
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    for rule in ["tvs-breakdown", "tvs-standoff"] {
        let f: Vec<_> = r
            .by_rule(rule)
            .filter(|f| f.reference == "D2" && f.case.is_none())
            .collect();
        assert!(f.len() == 1 && f[0].verdict == Verdict::Pass, "{rule}: {f:?}");
    }
}

/// The warnings on the clean boards are real datasheet findings.
#[test]
fn known_warnings() {
    // XC6206 ±2 % reaches 3.366 V; the oscillator's supply range ends at 3.3 V (YSO110TR sheet p1). The other
    // two are R4's errors.
    // (The ESD warnings on the debug headers are checked in `esd_on_the_test_boards`.)
    let not_esd = |r: &Report| -> Vec<String> { fails(r).into_iter().filter(|f| !f.contains(" esd ")).collect() };
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    assert_eq!(not_esd(&r).len(), 3, "{:#?}", fails(&r));
    assert!(
        r.by_rule("recommended")
            .any(|f| f.reference == "Y1" && f.verdict == Verdict::Fail)
    );
    // The sniffer's 3.6 V input is below the ME6211's specified VOUT + 1 V. The other three are the reversed
    // supply's two errors and their summary.
    let (_, _, r) = run("sniffer", "sniffer.kicad_sch");
    assert_eq!(not_esd(&r).len(), 4, "{:#?}", fails(&r));
    assert_eq!(r.errors(), 2);
    assert!(
        r.by_rule("regulator-headroom")
            .any(|f| f.reference == "U1" && f.verdict == Verdict::Fail)
    );
}

/// A rail that may sag because its regulator isn't held never fails a minimum, only leaves it UNKNOWN.
#[test]
fn a_sagging_rail_is_unknown_not_failed() {
    let (_, _, r) = run("sniffer", "sniffer.kicad_sch");
    let f = r
        .by_rule("recommended")
        .find(|f| f.reference == "U2" && f.message.starts_with("U2.VDD"))
        .unwrap();
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
    assert!(f.message.contains("only a bound"), "{}", f.message);
}

/// A pin limited to `VDD + 0.3 V` passes when everything driving it is bounded by that same VDD.
#[test]
fn pins_bounded_by_their_own_supply_pass() {
    let (_, _, r) = run("sniffer", "sniffer.kicad_sch");
    let f = r
        .by_rule("abs-max")
        .find(|f| f.reference == "U3" && f.message.contains("SCL") && f.case.is_none())
        .unwrap();
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
}

#[test]
fn every_rule_runs() {
    let mut seen = std::collections::BTreeSet::new();
    for (dir, sch) in [
        ("sniffer", "sniffer.kicad_sch"),
        ("can-encoder", "can-encoder.kicad_sch"),
        ("can-encoder-rev1", "can-encoder.kicad_sch"),
    ] {
        let (_, _, r) = run(dir, sch);
        seen.extend(r.findings.iter().map(|f| f.rule));
    }
    for (rule, _) in RULES {
        assert!(seen.contains(rule), "{rule} never ran");
    }
}

fn with_vbus(max: f64) -> BoardInputs {
    let (_, mut inputs) = board("can-encoder", "can-encoder.kicad_sch");
    inputs.inputs[0].max = max.into();
    inputs
}

/// Capacitors fail above their rating, and warn above the policy's 80 %.
#[test]
fn capacitor_ratings_and_derating() {
    let (d, _) = board("can-encoder", "can-encoder.kicad_sch");
    // C1 (CC0805KKX7R9BB225) is rated 50 V (p2) and sits on +VDC.
    let c1 = |max: f64| {
        let (_, r) = run_on(&d, &with_vbus(max));
        r.by_rule("cap-voltage").find(|f| f.reference == "C1").cloned().unwrap()
    };
    let f = c1(26.0);
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
    let f = c1(45.0);
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Warning),
        "{}",
        f.message
    );
    assert!(f.message.contains("80 %"), "{}", f.message);
    let f = c1(60.0);
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        f.message
    );
    // And the regulator's own abs max fails there too.
    let (_, r) = run_on(&d, &with_vbus(60.0));
    assert!(
        r.by_rule("abs-max")
            .any(|f| f.reference == "U1" && f.verdict == Verdict::Fail)
    );
}

fn pin(number: &str, kind: PinKind, net: usize) -> Pin {
    Pin {
        number: number.into(),
        name: String::new(),
        kind,
        no_connect: false,
        stack: None,
        net,
    }
}

fn add(d: &mut Design, reference: &str, lib_id: &str, value: &str, mpn: Option<&str>, pins: Vec<Pin>) {
    let mut c = Component {
        reference: reference.into(),
        value: value.into(),
        lib_id: lib_id.into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    if let Some(m) = mpn {
        c.fields.insert("MPN".into(), m.into());
    }
    for p in pins {
        d.nets[p.net].pins.push((reference.into(), p.number.clone()));
        c.pins.insert(p.number.clone(), p);
    }
    d.components.insert(reference.into(), c);
}

fn new_net(d: &mut Design, name: &str) -> usize {
    d.nets.push(Net {
        name: name.into(),
        pins: vec![],
        labels: vec![],
    });
    d.nets.len() - 1
}

fn net(d: &Design, name: &str) -> usize {
    d.nets.iter().position(|n| n.name == name).unwrap()
}

/// An input pin, and a power pin, on a net nothing drives.
#[test]
fn floating_inputs_and_unpowered_pins_fail() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let a = new_net(&mut d, "/FLOAT");
    let b = new_net(&mut d, "/NOPOWER");
    let gnd = net(&d, "GND");
    add(
        &mut d,
        "U9",
        "Logic:Buffer",
        "X",
        None,
        vec![
            pin("1", PinKind::Input, a),
            pin("2", PinKind::PowerIn, b),
            pin("3", PinKind::PowerIn, gnd),
        ],
    );
    // Something else on /FLOAT that doesn't drive: a capacitor to ground.
    add(
        &mut d,
        "C99",
        "Device:C",
        "100nF",
        None,
        vec![pin("1", PinKind::Passive, a), pin("2", PinKind::Passive, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f = r.by_rule("floating-input").find(|f| f.reference == "U9").unwrap();
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        f.message
    );
    let f = r
        .by_rule("unpowered")
        .find(|f| f.reference == "U9" && f.message.contains("/NOPOWER"))
        .unwrap();
    assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
    // A part with no part file and two of its own power pins tied on a net with only a capacitor, like the
    // MAX9722A's charge-pump output PVSS and SVSS: it may supply that net itself, so nothing proves it unpowered.
    let pump = new_net(&mut d, "/PVSS");
    let v33 = net(&d, "+3V3");
    add(
        &mut d,
        "U8",
        "Amplifier:Pump",
        "X",
        None,
        vec![
            pin("1", PinKind::PowerIn, pump),
            pin("2", PinKind::PowerIn, pump),
            pin("3", PinKind::PowerIn, v33),
        ],
    );
    // The same with nothing else powering the part is a supply net that never reaches a source (a mistyped
    // label): that stays a FAIL.
    let typo = new_net(&mut d, "/VDD_TYPO");
    add(
        &mut d,
        "U7",
        "Amplifier:Pump",
        "X",
        None,
        vec![pin("1", PinKind::PowerIn, typo), pin("2", PinKind::PowerIn, typo)],
    );
    add(
        &mut d,
        "C97",
        "Device:C",
        "100nF 50V",
        None,
        vec![pin("1", PinKind::Passive, typo), pin("2", PinKind::Passive, gnd)],
    );
    add(
        &mut d,
        "C98",
        "Device:C",
        "1uF 50V",
        None,
        vec![pin("1", PinKind::Passive, pump), pin("2", PinKind::Passive, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f = r.by_rule("unpowered").find(|f| f.reference == "U8").unwrap();
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
    let f = r.by_rule("unpowered").find(|f| f.reference == "U7").unwrap();
    assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
    let f = r
        .by_rule("unpowered")
        .find(|f| f.reference == "U9" && f.message.contains("GND"))
        .unwrap();
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
}

/// A 120 Ω 0402 straight across +VDC dissipates watts, far past its 62.5 mW rating.
#[test]
fn resistor_power() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let (vdc, gnd) = (net(&d, "+VDC"), net(&d, "GND"));
    add(
        &mut d,
        "R99",
        "Device:R",
        "120",
        Some("0402WGF1200TCE"),
        vec![pin("1", PinKind::Passive, vdc), pin("2", PinKind::Passive, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f = r.by_rule("resistor-power").find(|f| f.reference == "R99").unwrap();
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        f.message
    );
    assert!(f.message.contains("62.5 mW"), "{}", f.message);
    // The board's own resistors pass.
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    assert!(
        r.by_rule("resistor-power")
            .filter(|f| f.reference != "R4")
            .all(|f| f.verdict == Verdict::Pass)
    );
}

/// Parts with nothing to check against are listed, never passed.
#[test]
fn unchecked_parts_are_listed() {
    let (_, _, r) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    let f = r.by_rule("unchecked").find(|f| f.reference == "JP1").unwrap();
    assert_eq!(f.verdict, Verdict::Unknown);
    assert!(r.by_rule("unchecked").all(|f| f.verdict == Verdict::Unknown));
}

#[test]
fn report_serialises() {
    let (_, _, r) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    let json = serde_json::to_string(&r).unwrap();
    assert!(json.contains("\"rule\":\"tvs-breakdown\"") && json.contains("\"verdict\":\"fail\""));
}

/// A cap rated below a rail fails even when the rail's low end is only a floor: the top end is real.
#[test]
fn floors_only_soften_the_end_they_set() {
    let (mut d, inputs) = board("sniffer", "sniffer.kicad_sch");
    let (v33, gnd) = (net(&d, "+3V3"), net(&d, "GND"));
    add(
        &mut d,
        "C98",
        "Device:C",
        "1uF",
        None,
        vec![pin("1", PinKind::Passive, v33), pin("2", PinKind::Passive, gnd)],
    );
    d.components
        .get_mut("C98")
        .unwrap()
        .fields
        .insert("Voltage".into(), "2.5V".into());
    let (v, r) = run_on(&d, &inputs);
    assert!(v.nets[v33].floor, "+3V3's low end is a floor on the sniffer");
    let f = r.by_rule("cap-voltage").find(|f| f.reference == "C98").unwrap();
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        f.message
    );
}

/// A polarised capacitor fitted backwards fails its negative rating.
#[test]
fn reversed_polarised_capacitor_fails() {
    let dir = std::env::temp_dir().join(format!("gerberus-polcap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("POLCAP.toml"),
        "mpn = \"POLCAP\"\nmanufacturer = \"M\"\ndescription = \"test polarised cap\"\nclass = \"capacitor\"\n\
         [datasheet]\ntitle = \"t\"\ndocument = \"d\"\nurl = \"u\"\n[pins]\n1 = \"+\"\n2 = \"-\"\n\
         [[limit]]\npins = [\"+\"]\nto = \"-\"\nabs_max = { unknown = \"none\" }\n\
         recommended = { min = 0, max = 16, page = 1 }\n",
    )
    .unwrap();
    let lib = Library::load_dirs(&[&root("parts"), &dir]).unwrap();
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let (v5, gnd) = (net(&d, "+5V"), net(&d, "GND"));
    add(
        &mut d,
        "C98",
        "Device:C_Polarized",
        "10uF",
        Some("POLCAP"),
        vec![pin("1", PinKind::Passive, gnd), pin("2", PinKind::Passive, v5)],
    );
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let r = check(&d, &lib, &policy, &inputs, &v);
    std::fs::remove_dir_all(&dir).unwrap();
    let f = r.by_rule("cap-voltage").find(|f| f.reference == "C98").unwrap();
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        f.message
    );
}

/// An input reached only by an open-drain output isn't proven driven.
#[test]
fn open_drain_only_input_is_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let a = new_net(&mut d, "/OD");
    let (v33, gnd) = (net(&d, "+3V3"), net(&d, "GND"));
    add(
        &mut d,
        "U8",
        "Logic:OD",
        "X",
        None,
        vec![
            pin("1", PinKind::OpenCollector, a),
            pin("2", PinKind::PowerIn, v33),
            pin("3", PinKind::PowerIn, gnd),
        ],
    );
    add(
        &mut d,
        "U9",
        "Logic:Buffer",
        "X",
        None,
        vec![
            pin("1", PinKind::Input, a),
            pin("2", PinKind::PowerIn, v33),
            pin("3", PinKind::PowerIn, gnd),
        ],
    );
    let (_, r) = run_on(&d, &inputs);
    let f = r.by_rule("floating-input").find(|f| f.reference == "U9").unwrap();
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
}

/// A regulator whose range the part file doesn't give still gets a headroom finding.
#[test]
fn every_regulator_gets_a_headroom_finding() {
    use gerberus::limits::{DriveKind, Range};
    let (d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    let drive = lib
        .parts
        .get_mut("XC6206P332MR-G")
        .unwrap()
        .drives
        .iter_mut()
        .find(|d| d.kind == DriveKind::Regulates)
        .unwrap();
    drive.range = Range::Unknown("test".into());
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let r = check(&d, &lib, &policy, &inputs, &v);
    let f: Vec<_> = r
        .by_rule("regulator-headroom")
        .filter(|f| f.reference == "U5")
        .collect();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].verdict, Verdict::Unknown, "{}", f[0].message);
}

/// A floor carries through a resistor: a rail fed from a sagging rail doesn't fail a minimum either.
#[test]
fn floors_carry_through_resistors() {
    let (mut d, inputs) = board("sniffer", "sniffer.kicad_sch");
    let v33 = net(&d, "+3V3");
    let f = new_net(&mut d, "+3V3F");
    add(
        &mut d,
        "R98",
        "Device:R",
        "10",
        None,
        vec![pin("1", PinKind::Passive, v33), pin("2", PinKind::Passive, f)],
    );
    // Move the sensor's VDD onto the filtered rail.
    let u3 = d.components.get_mut("U3").unwrap();
    let vdd = u3
        .pins
        .values_mut()
        .find(|p| p.net == v33 && p.kind == PinKind::PowerIn)
        .unwrap();
    let num = vdd.number.clone();
    vdd.net = f;
    d.nets[v33].pins.retain(|(r, p)| !(r == "U3" && *p == num));
    d.nets[f].pins.push(("U3".into(), num));
    let (v, r) = run_on(&d, &inputs);
    assert!(v.nets[f].floor);
    // Only the reversed supply's two errors, as on the unchanged sniffer.
    assert_eq!(r.errors(), 2, "{:#?}", fails(&r));
    assert!(
        r.findings
            .iter()
            .filter(|x| x.reference == "U3" && x.rule == "recommended")
            .all(|x| x.verdict != Verdict::Fail),
        "{:#?}",
        fails(&r)
    );
}

/// A capacitor on a resistor's far end holds that node's old voltage, so the resistor can see the whole step
/// from its supply: the node isn't kept between the nets around it.
#[test]
fn a_capacitor_keeps_a_node_from_tracking_its_pull() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let (vdc, gnd) = (net(&d, "+VDC"), net(&d, "GND"));
    let node = new_net(&mut d, "/RC");
    add(
        &mut d,
        "R99",
        "Device:R",
        "120",
        Some("0402WGF1200TCE"),
        vec![pin("1", PinKind::Passive, vdc), pin("2", PinKind::Passive, node)],
    );
    let (v, _) = run_on(&d, &inputs);
    assert_eq!(v.nets[node].between, [vdc], "only R99 pulls it");
    let (_, r) = run_on(&d, &inputs);
    let f = r.by_rule("resistor-power").find(|f| f.reference == "R99").unwrap();
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);

    add(
        &mut d,
        "C99",
        "Device:C",
        "100nF 50V",
        None,
        vec![pin("1", PinKind::Passive, node), pin("2", PinKind::Passive, gnd)],
    );
    let (v, r) = run_on(&d, &inputs);
    assert!(v.nets[node].between.is_empty());
    let f = r.by_rule("resistor-power").find(|f| f.reference == "R99").unwrap();
    assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
}

/// Connector pins: the bus pins are clamped or rated for system-level ESD; the debug headers and the sniffer's
/// UART reach STM32 pins rated only for handling (HBM), which is a warning.
#[test]
fn esd_on_the_test_boards() {
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    let esd = |r: &Report, at: &str| -> Verdict {
        let f: Vec<_> = r.by_rule("esd").filter(|f| f.message.starts_with(at)).collect();
        assert_eq!(f.len(), 1, "{at}: {f:#?}");
        assert_eq!(f[0].severity, Severity::Warning);
        f[0].verdict
    };
    // VBUS reaches its ICs only through D1, with the SMF26A behind it.
    assert_eq!(esd(&r, "J1.1 (VBUS)"), Verdict::Pass);
    assert_eq!(esd(&r, "J1.2 (/CANH)"), Verdict::Pass);
    assert_eq!(esd(&r, "J3.2 (/SWDIO)"), Verdict::Fail);
    let (_, _, r) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    assert_eq!(esd(&r, "J1.2 (/CANH)"), Verdict::Pass);
    // The NUP2105L breaks down at up to 32 V, above the TCAN330's 14 V bus pins, so it isn't counted as their
    // clamp; the TCAN330's own ±12 kV contact rating passes them.
    let f: Vec<_> = r
        .by_rule("esd")
        .filter(|f| f.message.starts_with("J1.2 (/CANH)"))
        .collect();
    assert!(
        f[0].message.contains("U5 (NUP2105LT1G) breaks down only at up to 32 V"),
        "{}",
        f[0].message
    );
    assert!(
        f[0].message.contains("±12 kV IEC 61000-4-2 contact"),
        "{}",
        f[0].message
    );
    let (_, _, r) = run("sniffer", "sniffer.kicad_sch");
    assert_eq!(esd(&r, "J1.2 (/UART_TX)"), Verdict::Fail);
    // The ME6211 gives no ESD rating.
    assert_eq!(esd(&r, "J1.1 (+5V)"), Verdict::Unknown);
}

/// A part with no part file on a connector's net could be an IC or a clamp, so the net can't pass: it was
/// passing as "only passive parts" (the audio board rev 2's MAX9722A on the headphone jack).
#[test]
fn esd_is_unknown_with_an_undescribed_part() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let canh = net(&d, "/CANH");
    let gnd = net(&d, "GND");
    add(
        &mut d,
        "U7",
        "Amplifier:Unknown",
        "MYSTERY1",
        Some("MYSTERY1"),
        vec![pin("1", PinKind::Input, canh), pin("2", PinKind::PowerIn, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f: Vec<_> = r
        .by_rule("esd")
        .filter(|f| f.message.starts_with("J1.2 (/CANH)"))
        .collect();
    assert_eq!(f[0].verdict, Verdict::Unknown, "{}", f[0].message);
    assert!(f[0].message.contains("U7 (MYSTERY1)"), "{}", f[0].message);
}

/// A part that fails its maximum in normal operation and its minimum when reversed still counts as damaged by
/// the reversal: the two failures are different findings.
#[test]
fn reversal_failures_count_by_finding_not_by_pin() {
    let (d, mut inputs) = board("sniffer", "sniffer.kicad_sch");
    inputs.inputs[0].max = 30.0.into();
    let (_, r) = run_on(&d, &inputs);
    let caps: Vec<_> = r
        .findings
        .iter()
        .filter(|f| f.rule == "cap-voltage" && f.verdict == Verdict::Fail && f.severity == Severity::Error)
        .collect();
    assert!(caps.iter().any(|f| f.case.is_none()), "{caps:#?}");
    let reversed: Vec<_> = caps
        .iter()
        .filter(|f| f.case.as_deref() == Some("VIN reversed"))
        .collect();
    assert!(!reversed.is_empty(), "{caps:#?}");
    let summary = r.by_rule("reverse-polarity").next().unwrap();
    assert!(summary.message.contains(&reversed[0].reference), "{}", summary.message);
}

/// A pull-down on a connector pin doesn't make the connector reach every ground pin on the board.
#[test]
fn esd_does_not_follow_pulls_onto_rails() {
    let (mut d, inputs) = board("sniffer", "sniffer.kicad_sch");
    let swo = d.components["J2"].pins["6"].net;
    let gnd = net(&d, "GND");
    add(
        &mut d,
        "R99",
        "Device:R",
        "10k",
        Some("0402WGF1002TCE"),
        vec![pin("1", PinKind::Passive, swo), pin("2", PinKind::Passive, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f: Vec<_> = r.by_rule("esd").filter(|f| f.message.starts_with("J2.6")).collect();
    assert!(f.len() == 1 && f[0].verdict == Verdict::Pass, "{f:#?}");
}

/// Parts with no part file that can't take the discharge into one pin don't hold a connector's net at UNKNOWN: a
/// generic pull-down to a rail, and a connector named by its symbol library rather than its reference.
#[test]
fn esd_skips_undescribed_pulls_and_connectors() {
    let (mut d, inputs) = board("sniffer", "sniffer.kicad_sch");
    let swo = d.components["J2"].pins["6"].net;
    let gnd = net(&d, "GND");
    add(
        &mut d,
        "R98",
        "Device:R",
        "10k",
        None,
        vec![pin("1", PinKind::Passive, swo), pin("2", PinKind::Passive, gnd)],
    );
    add(
        &mut d,
        "USB2",
        "Connector:USB_C_Receptacle",
        "USB_C",
        None,
        vec![pin("A5", PinKind::Bidirectional, swo), pin("A1", PinKind::Passive, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f: Vec<_> = r.by_rule("esd").filter(|f| f.message.starts_with("J2.6")).collect();
    assert!(f.len() == 1 && f[0].verdict == Verdict::Pass, "{f:#?}");
}

/// A part with no part file behind a series diode whose far side is clamped is sheltered by that clamp, as an IC
/// there would be: can-encoder rev 2's VBUS reaches +VDC through D1, where D2 (SMF26A) clamps.
#[test]
fn esd_shelters_undescribed_parts_behind_a_clamp() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let vdc = net(&d, "+VDC");
    let gnd = net(&d, "GND");
    let before: Vec<_> = run_on(&d, &inputs)
        .1
        .by_rule("esd")
        .filter(|f| f.message.starts_with("J1.1"))
        .map(|f| f.verdict)
        .collect();
    assert_eq!(before, [Verdict::Pass]);
    add(
        &mut d,
        "U98",
        "Regulator:Mystery",
        "MYSTERY2",
        Some("MYSTERY2"),
        vec![pin("1", PinKind::PowerIn, vdc), pin("2", PinKind::PowerIn, gnd)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f: Vec<_> = r.by_rule("esd").filter(|f| f.message.starts_with("J1.1")).collect();
    assert!(f.len() == 1 && f[0].verdict == Verdict::Pass, "{f:#?}");
}

/// A rail fed through a ferrite bead takes the driven rail's range: a bead is a DC short. The amp board's +5V
/// comes from +5V_RAW through FB101 (`Device:FerriteBead`, no part file), and was UNKNOWN.
#[test]
fn ferrite_beads_pass_a_rail_through() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let (v33, gnd) = (net(&d, "+3V3"), net(&d, "GND"));
    let quiet = new_net(&mut d, "/3V3_QUIET");
    let quieter = new_net(&mut d, "/3V3_QUIETER");
    add(
        &mut d,
        "FB98",
        "Device:FerriteBead",
        "600R",
        None,
        vec![pin("1", PinKind::Passive, v33), pin("2", PinKind::Passive, quiet)],
    );
    add(
        &mut d,
        "L98",
        "Device:FerriteBead_Small",
        "600R",
        None,
        vec![pin("1", PinKind::Passive, quiet), pin("2", PinKind::Passive, quieter)],
    );
    add(
        &mut d,
        "C98",
        "Device:C",
        "1uF 16V",
        None,
        vec![pin("1", PinKind::Passive, quieter), pin("2", PinKind::Passive, gnd)],
    );
    let (v, _) = run_on(&d, &inputs);
    assert_eq!(v.nets[quiet].span, v.nets[v33].span, "{:#?}", v.nets[quiet]);
    assert_eq!(v.nets[quieter].span, v.nets[v33].span, "{:#?}", v.nets[quieter]);
    assert!(
        v.nets[quieter].why[0].text.contains("ferrite bead L98"),
        "{:#?}",
        v.nets[quieter]
    );
}

/// A stacked power pin (KiCad 10 `[3,9]`) left on a net nothing drives is one unpowered pin, not a part that may
/// supply its own rail because two of its pins share the net.
#[test]
fn an_open_stacked_power_pin_fails_once() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let open = new_net(&mut d, "/OPEN");
    let v33 = net(&d, "+3V3");
    let stacked = |number: &str, net| Pin {
        stack: Some("3".into()),
        ..pin(number, PinKind::PowerIn, net)
    };
    add(
        &mut d,
        "U6",
        "Logic:Stacked",
        "X",
        None,
        vec![stacked("3", open), stacked("9", open), pin("1", PinKind::PowerIn, v33)],
    );
    let (_, r) = run_on(&d, &inputs);
    let f: Vec<_> = r
        .by_rule("unpowered")
        .filter(|f| f.reference == "U6" && f.message.contains("/OPEN"))
        .collect();
    assert!(f.len() == 1 && f[0].verdict == Verdict::Fail, "{f:#?}");
}

/// A connector on a rail fed through a ferrite bead can bring in anything, as on any driven net.
#[test]
fn a_connector_after_a_bead_is_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let (v33, gnd) = (net(&d, "+3V3"), net(&d, "GND"));
    let ext = new_net(&mut d, "/3V3_EXT");
    add(
        &mut d,
        "FB97",
        "Device:FerriteBead",
        "600R",
        None,
        vec![pin("1", PinKind::Passive, v33), pin("2", PinKind::Passive, ext)],
    );
    add(
        &mut d,
        "J97",
        "Connector:Conn_01x02",
        "OUT",
        None,
        vec![pin("1", PinKind::Passive, ext), pin("2", PinKind::Passive, gnd)],
    );
    let (v, _) = run_on(&d, &inputs);
    assert_eq!(v.nets[ext].span, Span::unknown(), "{:#?}", v.nets[ext]);
}
