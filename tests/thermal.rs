use std::path::{Path, PathBuf};

use gerberus::design::Design;
use gerberus::finding::{Finding, Severity, Verdict};
use gerberus::inputs::{BoardInputs, BoardTemperature};
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::policy::Policy;
use gerberus::rules::{Report, check};
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

#[test]
fn rail_load_adds_up_supply_currents() {
    let (d, inputs) = board("can-encoder");
    let (th, _) = analyse(&d, &inputs);
    let rail = &th.loads[net(&d, "+3V3")];
    assert!(rail.missing.is_empty(), "{:?}", rail.missing);
    let led = 3.366 / 977.0;
    let pull = rail.parts.iter().find(|p| p.starts_with("R5 ")).expect("R5 counted");
    assert!(pull.contains("to /SPI_CS"), "{pull}");
    let want = 0.0246 + 0.000_344 + 0.028 + 0.0003 + 0.004 + led + 0.000_344_5;
    assert!(
        (rail.amps - want).abs() < 1e-6,
        "{} vs {want}: {:?}",
        rail.amps,
        rail.parts
    );
    assert!(
        rail.parts
            .iter()
            .any(|p| p.contains("D3") && p.contains("sourced by U2")),
        "{:?}",
        rail.parts
    );
}

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

#[test]
fn ldo_dissipation_uses_the_held_output_at_the_top_of_its_supply() {
    let (d, inputs) = board("sniffer");
    let (th, _) = analyse(&d, &inputs);
    let out = th.loads[net(&d, "+3V3")].amps;
    let u1 = &th.parts["U1"];
    assert!(close(u1.watts.unwrap(), (5.5 - 3.234) * out), "{:?}", u1.watts);
    assert!(u1.how.contains("dropout"), "{}", u1.how);
}

#[test]
fn switcher_without_efficiency_is_unknown() {
    let (d, inputs) = board("can-encoder");
    let (th, r) = analyse(&d, &inputs);
    assert!(th.parts["U1"].watts.is_none());
    let f = finding(&r, "junction-temperature", "U1");
    assert_eq!(f.verdict, Verdict::Unknown);
    assert!(f.message.contains("gives no efficiency"), "{}", f.message);
    let f = finding(&r, "forward-current", "D1");
    assert_eq!(f.verdict, Verdict::Unknown);
    assert!(f.message.contains("is unknown"), "{}", f.message);
    assert!(!th.loads[net(&d, "+VDC")].missing.is_empty());
}

#[test]
fn led_current_below_its_vf_test_point_counts_vf_as_zero() {
    let (d, inputs) = board("sniffer");
    let (_, r) = analyse(&d, &inputs);
    let f = finding(&r, "forward-current", "D1");
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
    for want in [
        "3.445 mA at most",
        "counting VF as 0 V",
        "only at 5 mA and 25 °C",
        "30 mA rating",
    ] {
        assert!(f.message.contains(want), "missing {want:?}: {}", f.message);
    }
}

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
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
    for want in [
        "below its 105 °C maximum",
        "holds only up to 80 °C ambient",
        "3.54 mA sourced",
    ] {
        assert!(f.message.contains(want), "missing {want:?}: {}", f.message);
    }
}

#[test]
fn board_temperature_sets_the_verdict() {
    let (d, mut inputs) = board("can-encoder-rev1");
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
fn each_rail_adds_up_its_loads() {
    let (d, inputs) = board("sniffer");
    let (th, _) = analyse(&d, &inputs);
    let load = |name: &str| &th.loads[d.nets.iter().position(|n| n.name == name).unwrap()];
    let rail_3v3 = load("+3V3");
    assert!((rail_3v3.amps * 1e3 - 33.32).abs() < 0.01, "{rail_3v3:?}");
    assert!(
        rail_3v3.parts.iter().any(|p| p.starts_with("U2 11.19 mA (p44)")),
        "{rail_3v3:?}"
    );
    assert!(
        rail_3v3.parts.iter().any(|p| p.contains("R2 344.5 µA to /I2C_SDA")),
        "{rail_3v3:?}"
    );
    let rail_5v = load("+5V");
    assert!((rail_5v.amps * 1e3 - 33.32).abs() < 0.01, "{rail_5v:?}");
    assert!(
        rail_5v
            .missing
            .iter()
            .any(|m| m.starts_with("U1 (ME6211C33M5G-N gives no supply current")),
        "{rail_5v:?}"
    );
}

use gerberus::design::{Component, Net, Pin, PinKind};

fn new_net(d: &mut Design, name: &str) -> usize {
    d.nets.push(Net {
        name: name.into(),
        pins: vec![],
        labels: vec![],
    });
    d.nets.len() - 1
}

fn move_pin(d: &mut Design, reference: &str, number: &str, to: usize) {
    let from = d.components[reference].pins[number].net;
    d.nets[from].pins.retain(|(r, p)| !(r == reference && p == number));
    d.nets[to].pins.push((reference.into(), number.into()));
    d.components
        .get_mut(reference)
        .unwrap()
        .pins
        .get_mut(number)
        .unwrap()
        .net = to;
}

fn add(d: &mut Design, reference: &str, lib_id: &str, mpn: Option<&str>, pins: &[(&str, usize)]) {
    let mut c = Component {
        reference: reference.into(),
        value: String::new(),
        lib_id: lib_id.into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    if let Some(m) = mpn {
        c.fields.insert("MPN".into(), m.into());
    }
    for &(number, net) in pins {
        d.nets[net].pins.push((reference.into(), number.into()));
        c.pins.insert(
            number.into(),
            Pin {
                number: number.into(),
                name: String::new(),
                kind: PinKind::Passive,
                no_connect: false,
                net,
                stack: None,
            },
        );
    }
    d.components.insert(reference.into(), c);
}

fn pin_of(d: &Design, reference: &str, name: &str) -> String {
    let lib = Library::load_dir(&root("parts")).unwrap();
    let c = &d.components[reference];
    let mpn = c.fields.get("MPN").or_else(|| c.fields.get("MPN#")).cloned();
    let part = lib
        .parts
        .values()
        .find(|p| Some(&p.mpn) == mpn.as_ref() || c.fields.values().any(|v| v == &p.mpn))
        .unwrap();
    part.pins_named(name).into_iter().next().unwrap().to_string()
}

#[test]
fn an_led_off_ground_is_unknown_not_dropped() {
    let (mut d, inputs) = board("can-encoder");
    let k = pin_of(&d, "D3", "K");
    let low = new_net(&mut d, "LED_LOW");
    move_pin(&mut d, "D3", &k, low);
    let (th, r) = analyse(&d, &inputs);
    assert!(!th.loads[net(&d, "+3V3")].missing.is_empty());
    let f = finding(&r, "forward-current", "D3");
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
}

#[test]
fn current_passes_through_an_inductor() {
    let (mut d, inputs) = board("can-encoder");
    let before = analyse(&d, &inputs).0.loads[net(&d, "+5V")].amps;
    let vin = pin_of(&d, "U5", "VIN");
    let filtered = new_net(&mut d, "+5V_F");
    move_pin(&mut d, "U5", &vin, filtered);
    let five = net(&d, "+5V");
    add(&mut d, "L9", "Device:L", None, &[("1", five), ("2", filtered)]);
    let (th, _) = analyse(&d, &inputs);
    let after = &th.loads[five];
    assert!(
        (after.amps - before).abs() < 1e-3,
        "{} vs {before}: {:?}",
        after.amps,
        after.parts
    );
    assert!(
        after.parts.iter().any(|p| p.starts_with("L9 to +5V_F")),
        "{:?}",
        after.parts
    );
}

#[test]
fn a_reverse_biased_diode_carries_nothing() {
    let (mut d, inputs) = board("can-encoder");
    let (three, vbus) = (net(&d, "+3V3"), net(&d, "VBUS"));
    let a = pin_of(&d, "D1", "A");
    let k = pin_of(&d, "D1", "K");
    add(
        &mut d,
        "D9",
        "Device:D_Schottky",
        Some("B5819WS"),
        &[(&a, three), (&k, vbus)],
    );
    let (th, r) = analyse(&d, &inputs);
    assert!(!th.forward.contains_key("D9"));
    assert!(r.by_rule("forward-current").all(|f| f.reference != "D9"));
}

#[test]
fn a_diode_looping_the_rails_does_not_pass() {
    let (mut d, inputs) = board("can-encoder");
    let (three, five) = (net(&d, "+3V3"), net(&d, "+5V"));
    let a = pin_of(&d, "D1", "A");
    let k = pin_of(&d, "D1", "K");
    add(
        &mut d,
        "D9",
        "Device:D_Schottky",
        Some("B5819WS"),
        &[(&a, three), (&k, five)],
    );
    let (_, r) = analyse(&d, &inputs);
    assert!(
        r.by_rule("forward-current")
            .all(|f| f.reference != "D9" || f.verdict != Verdict::Pass)
    );
}

#[test]
fn an_ic_with_no_supply_current_is_unknown() {
    let (mut d, mut inputs) = board("can-encoder");
    inputs.temperature = Some(at(-20.0, 60.0));
    for p in d.components.get_mut("U4").unwrap().pins.values_mut() {
        p.kind = PinKind::Passive;
    }
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    lib.parts.get_mut("TCAN1044AVDDFRQ1").unwrap().supply.clear();
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let r = check(&d, &lib, &policy, &inputs, &v);
    let f = finding(&r, "junction-temperature", "U4");
    assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
    let th = thermal::compute(&d, &lib, &inputs, &v);
    assert!(th.loads[net(&d, "+5V")].missing.iter().any(|m| m.contains("U4")));
}
