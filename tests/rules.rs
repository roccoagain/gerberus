//! The first ten rules on the test boards. Rev 1's TVS is the golden test: its SMAJ18A sits on the 24 V bus
//! and conducts. The other two boards must come out with no errors.

use std::path::{Path, PathBuf};

use gerberus::design::{Component, Design, Net, Pin, PinKind};
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::policy::Policy;
use gerberus::rules::{RULES, Report, Severity, Verdict, check, render};
use gerberus::voltage::{Voltages, compute};

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
    let r = check(design, &lib, &policy, &v);
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
    let f: Vec<_> = r.by_rule("tvs-breakdown").filter(|f| f.reference == "D2").collect();
    assert_eq!(f.len(), 1, "{f:?}");
    let f = f[0];
    assert_eq!((f.verdict, f.severity), (Verdict::Fail, Severity::Error));
    for want in ["+12V", "25.2 V", "20 V", "SMAJ18A", "p2"] {
        assert!(f.message.contains(want), "missing {want:?}: {}", f.message);
    }
    // It is the only error, and the report explains how +12V gets to 25.2 V.
    assert_eq!(r.errors(), 1, "{:#?}", fails(&r));
    let text = render(&d, &v, &r, false);
    for want in ["D1 (B5819W SL)", "input VBUS on J1.1", "1 errors"] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    // The stand-off is exceeded too, as a warning.
    assert!(
        r.by_rule("tvs-standoff")
            .any(|f| f.reference == "D2" && f.verdict == Verdict::Fail && f.severity == Severity::Warning)
    );
}

#[test]
fn rev2_and_sniffer_have_no_errors() {
    for (dir, sch) in [
        ("can-encoder", "can-encoder.kicad_sch"),
        ("sniffer", "sniffer.kicad_sch"),
    ] {
        let (_, _, r) = run(dir, sch);
        assert_eq!(r.errors(), 0, "{dir}: {:#?}", fails(&r));
    }
}

/// Rev 2's SMF26A replaced the SMAJ18A: 26 V stays below its 28.9 V breakdown and within its 26 V stand-off.
#[test]
fn rev2_tvs_passes() {
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    for rule in ["tvs-breakdown", "tvs-standoff"] {
        let f: Vec<_> = r.by_rule(rule).filter(|f| f.reference == "D2").collect();
        assert!(f.len() == 1 && f[0].verdict == Verdict::Pass, "{rule}: {f:?}");
    }
}

/// The warnings on the clean boards are real datasheet findings.
#[test]
fn known_warnings() {
    // XC6206 ±2 % reaches 3.366 V; the oscillator's supply range ends at 3.3 V (YSO110TR sheet p1).
    let (_, _, r) = run("can-encoder", "can-encoder.kicad_sch");
    assert_eq!(fails(&r).len(), 1, "{:#?}", fails(&r));
    assert!(
        r.by_rule("recommended")
            .any(|f| f.reference == "Y1" && f.verdict == Verdict::Fail)
    );
    // The sniffer's 3.6 V input is below the ME6211's specified VOUT + 1 V.
    let (_, _, r) = run("sniffer", "sniffer.kicad_sch");
    assert_eq!(fails(&r).len(), 1, "{:#?}", fails(&r));
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
        .find(|f| f.reference == "U3" && f.message.contains("SCL"))
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
    inputs.inputs[0].max = max;
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
