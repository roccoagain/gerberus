//! Rail loads, part dissipation, junction temperature and LED current on the test boards.

use std::path::{Path, PathBuf};

use gerberus::design::Design;
use gerberus::inputs::{BoardInputs, BoardTemperature};
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::policy::Policy;
use gerberus::rules::{Finding, Report, Severity, Verdict, check};
use gerberus::thermal::{self, Thermal};
use gerberus::voltage::compute;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn board(dir: &str) -> (Design, BoardInputs) {
    let d = root("tests/fixtures").join(dir);
    let sch = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "kicad_sch"))
        .unwrap();
    (
        kicad::load(&sch).unwrap(),
        BoardInputs::load(&d.join("inputs.toml")).unwrap(),
    )
}

fn analyse(design: &Design, inputs: &BoardInputs) -> (Thermal, Report) {
    let lib = Library::load_dir(&root("parts")).unwrap();
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let v = compute(design, &lib, inputs);
    (
        thermal::compute(design, &lib, inputs, &v),
        check(design, &lib, &policy, inputs, &v),
    )
}

fn finding<'a>(r: &'a Report, rule: &'a str, reference: &str) -> &'a Finding {
    r.by_rule(rule)
        .find(|f| f.reference == reference && f.case.is_none())
        .unwrap_or_else(|| panic!("no {rule} finding for {reference}"))
}

fn net(d: &Design, name: &str) -> usize {
    d.nets.iter().position(|n| n.name == name).unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9_f64.max(b.abs() * 1e-6)
}

fn at(min: f64, max: f64) -> BoardTemperature {
    BoardTemperature {
        min,
        max,
        source: "test".into(),
    }
}

/// +3V3 on the CAN encoder: every supply current from the part files, plus the LED chain.
#[test]
fn rail_load_adds_up_supply_currents() {
    let (d, inputs) = board("can-encoder");
    let (th, _) = analyse(&d, &inputs);
    let rail = &th.loads[net(&d, "+3V3")];
    assert!(rail.missing.is_empty(), "{:?}", rail.missing);
    // U2 24.6 mA + 344 µA, U3 28 mA, U4's VIO 300 µA, Y1 4 mA, and the LED's 3.366 V / 977 Ω.
    let led = 3.366 / 977.0;
    let want = 0.0246 + 0.000_344 + 0.028 + 0.0003 + 0.004 + led;
    assert!(close(rail.amps, want), "{} vs {want}: {:?}", rail.amps, rail.parts);
    // The LED's current comes from U2, which drives it.
    assert!(
        rail.parts
            .iter()
            .any(|p| p.contains("D3") && p.contains("sourced by U2")),
        "{:?}",
        rail.parts
    );
}

/// The LDO passes its output current to its input and dissipates (VIN max − VOUT min) × I plus its own supply.
#[test]
fn ldo_dissipation() {
    let (d, inputs) = board("can-encoder");
    let (th, _) = analyse(&d, &inputs);
    let out = th.loads[net(&d, "+3V3")].amps;
    let input = &th.loads[net(&d, "+5V")];
    assert!(
        input.parts.iter().any(|p| p.contains("U5") && p.contains("output")),
        "{:?}",
        input.parts
    );
    let lib = Library::load_dir(&root("parts")).unwrap();
    let vin = compute(&d, &lib, &inputs).nets[net(&d, "+5V")].span.max.unwrap();
    assert!((vin - 5.178).abs() < 1e-3, "{vin}");
    let u5 = &th.parts["U5"];
    let want = (vin - 3.234) * out + vin * 3e-6;
    assert!(close(u5.watts.unwrap(), want), "{:?} vs {want}", u5.watts);
    assert!(u5.missing.is_empty(), "{:?}", u5.missing);
}

/// An LDO whose supply can sag below what holds its output still dissipates most at the top of its supply,
/// where the output is held, not with its output at the 0 V floor.
#[test]
fn ldo_dissipation_uses_the_held_output_at_the_top_of_its_supply() {
    let (d, inputs) = board("sniffer");
    let (th, _) = analyse(&d, &inputs);
    let out = th.loads[net(&d, "+3V3")].amps;
    let u1 = &th.parts["U1"];
    assert!(close(u1.watts.unwrap(), (5.5 - 3.234) * out), "{:?}", u1.watts);
    assert!(u1.how.contains("dropout"), "{}", u1.how);
}

/// A buck converter needs an efficiency, which these datasheets only give as typical curves.
#[test]
fn switcher_without_efficiency_is_unknown() {
    let (d, inputs) = board("can-encoder");
    let (th, r) = analyse(&d, &inputs);
    assert!(th.parts["U1"].watts.is_none());
    let f = finding(&r, "junction-temperature", "U1");
    assert_eq!(f.verdict, Verdict::Unknown);
    assert!(f.message.contains("gives no efficiency"), "{}", f.message);
    // So the current through the input diode is unknown too, rather than "at least 0".
    let f = finding(&r, "forward-current", "D1");
    assert_eq!(f.verdict, Verdict::Unknown);
    assert!(f.message.contains("is unknown"), "{}", f.message);
    assert!(!th.loads[net(&d, "+VDC")].missing.is_empty());
}

/// Below the current an LED's VF minimum is rated at, VF can be lower, so the current is bounded with VF = 0.
#[test]
fn led_current_below_its_vf_test_point_counts_vf_as_zero() {
    let (d, inputs) = board("sniffer");
    let (_, r) = analyse(&d, &inputs);
    let f = finding(&r, "forward-current", "D1");
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
    for want in [
        "3.445 mA at most",
        "counting VF as 0 V",
        "rated at 5 mA",
        "30 mA rating",
    ] {
        assert!(f.message.contains(want), "missing {want:?}: {}", f.message);
    }
}

/// Without a board temperature the part's own rated ambient is used. A part that would overheat there is
/// UNKNOWN, with the ambient it is safe up to, not a FAIL.
#[test]
fn rated_ambient_is_used_without_a_board_temperature() {
    let (d, inputs) = board("can-encoder-rev1");
    assert!(inputs.temperature.is_none());
    let (_, r) = analyse(&d, &inputs);
    let f = finding(&r, "junction-temperature", "U4");
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
    for want in ["rated 125 °C ambient", "157 °C", "only safe up to 118 °C ambient"] {
        assert!(f.message.contains(want), "missing {want:?}: {}", f.message);
    }
    let f = finding(&r, "junction-temperature", "U2");
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
    assert!(
        f.message.contains("margin holds only up to 81 °C ambient"),
        "{}",
        f.message
    );
}

/// With a board temperature: past TJ max is an error, inside the policy's margin a warning, else a pass.
#[test]
fn board_temperature_sets_the_verdict() {
    let (d, mut inputs) = board("can-encoder-rev1");
    // U4 (TCAN330) dissipates 207.5 mW at θJA 154.4 °C/W: about 32 °C of rise, against TJ max 150 °C.
    let u4 = |inputs: &BoardInputs| finding(&analyse(&d, inputs).1, "junction-temperature", "U4").clone();
    inputs.temperature = Some(at(-20.0, 125.0));
    let f = u4(&inputs);
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        f.message
    );
    inputs.temperature = Some(at(-20.0, 105.0));
    let f = u4(&inputs);
    assert_eq!(
        (f.verdict, f.severity),
        (Verdict::Fail, Severity::Warning),
        "{}",
        f.message
    );
    assert!(f.message.contains("20 °C"), "{}", f.message);
    inputs.temperature = Some(at(-20.0, 60.0));
    let f = u4(&inputs);
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
    assert!(f.message.contains("the board's 60 °C"), "{}", f.message);
}

#[test]
fn loads_summary_lists_each_rail() {
    let (d, inputs) = board("sniffer");
    let (th, _) = analyse(&d, &inputs);
    let text = thermal::describe(&d, &th);
    assert!(text.contains("+3V3: 32.64 mA (U2 11.19 mA (p44)"), "{text}");
    assert!(text.contains("+5V: 32.64 mA at least"), "{text}");
    assert!(
        text.contains("not counted: U1 (ME6211C33M5G-N gives no supply current"),
        "{text}"
    );
}
