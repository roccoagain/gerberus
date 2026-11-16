//! Worst-case net voltages on the test boards, worked out from their inputs and part files.

use std::path::{Path, PathBuf};

use gerberus::design::Design;
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::voltage::{Span, Voltages, compute, parse_ohms};

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

fn run(dir: &str, sch: &str) -> (Design, Voltages) {
    let (design, inputs) = board(dir, sch);
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&design, &lib, &inputs);
    (design, v)
}

fn span(design: &Design, v: &Voltages, net: &str) -> Span {
    v.by_name(design, net).unwrap_or_else(|| panic!("no net {net}")).span
}

fn close(s: Span, lo: f64, hi: f64) -> bool {
    let near = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-9);
    near(s.min, lo) && near(s.max, hi)
}

/// `vref * (1 + Rtop / Rbottom)` at the ends that make it smallest and largest, where each resistor can be
/// off by `dev` (tolerance plus temperature drift).
fn divider(vref: (f64, f64), rt: f64, rb: f64, dev: f64) -> (f64, f64) {
    (
        vref.0 * (1.0 + rt * (1.0 - dev) / (rb * (1.0 + dev))),
        vref.1 * (1.0 + rt * (1.0 + dev) / (rb * (1.0 - dev))),
    )
}

/// UNI-ROYAL 0402: ±1 % (p2) plus 100 ppm/°C (p6) over the rated -55 to 155 °C (p5), 130 °C from 25 °C.
const DEV: f64 = 0.01 + 100e-6 * 130.0;

fn net_index(d: &Design, name: &str) -> usize {
    d.nets.iter().position(|n| n.name == name).unwrap()
}

fn text(v: &Voltages, d: &Design, net: &str) -> String {
    v.by_name(d, net)
        .unwrap()
        .why
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn rev2_rails() {
    let (d, v) = run("can-encoder", "can-encoder.kicad_sch");
    assert!(close(span(&d, &v, "VBUS"), 7.0, 26.0));
    assert!(close(span(&d, &v, "GND"), 0.0, 0.0));
    // B5819WS drops at most 0.6 V (p1), and no minimum is given.
    assert!(close(span(&d, &v, "+VDC"), 6.4, 26.0), "{}", span(&d, &v, "+VDC"));
    let (lo, hi) = divider((0.985, 1.015), 47e3, 12e3, DEV);
    assert!(close(span(&d, &v, "+5V"), lo, hi), "{}", span(&d, &v, "+5V"));
    assert!(close(span(&d, &v, "Net-(U1-FB)"), 0.985, 1.015));
    // +5V's low end clears the XC6206's VOUT + 1 V headroom (p4), so +3V3 is held.
    assert!(lo >= 3.366 + 1.0);
    assert!(close(span(&d, &v, "+3V3"), 3.234, 3.366));
    // How far the switch node rings past ground and VIN isn't given.
    assert_eq!(span(&d, &v, "Net-(U1-SW)"), Span::unknown());
    // Logic nets reach at most the highest supply of the parts driving them.
    assert!(close(span(&d, &v, "/SPI_MISO"), 0.0, 3.366));
    assert!(
        close(span(&d, &v, "/CAN_RX"), 0.0, hi),
        "the transceiver also runs from +5V"
    );
}

#[test]
fn rev1_rails() {
    let (d, v) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    assert!(close(span(&d, &v, "+12V"), 6.4, 25.2), "{}", span(&d, &v, "+12V"));
    // The AP63200's datasheet gives no headroom figure, so +3V3 isn't assumed to be held at its low end.
    let (_, hi) = divider((0.792, 0.808), 47e3, 15e3, DEV);
    assert!(close(span(&d, &v, "+3V3"), 0.0, hi), "{}", span(&d, &v, "+3V3"));
    assert!(
        text(&v, &d, "+3V3").contains("gives no headroom"),
        "{}",
        text(&v, &d, "+3V3")
    );
    assert!(close(span(&d, &v, "/LED"), 0.0, hi));
}

#[test]
fn sniffer_rails() {
    let (d, v) = run("sniffer", "sniffer.kicad_sch");
    assert!(close(span(&d, &v, "+5V"), 3.6, 5.5));
    // The ME6211's output is specified at VIN = VOUT + 1 V (p8), but VIN can be as low as 3.6 V.
    assert!(close(span(&d, &v, "+3V3"), 0.0, 3.366));
    assert!(
        text(&v, &d, "+3V3").contains("below the 4.366 V"),
        "{}",
        text(&v, &d, "+3V3")
    );
    assert!(close(span(&d, &v, "/UART_RX"), 0.0, 3.3));
    // J1.2 is listed as undriven: only the host's RX listens on it.
    assert!(close(span(&d, &v, "/UART_TX"), 0.0, 3.366));
    // The LED's anode is reached through R3 from the MCU pin and through the LED from ground.
    assert!(close(span(&d, &v, "Net-(D1-A)"), 0.0, 3.366));
}

/// The rev 1 bug: the explanation for +12V names the diode, its drop, and the bus it comes from.
#[test]
fn rev1_explanation_names_the_chain() {
    let (d, v) = run("can-encoder-rev1", "can-encoder.kicad_sch");
    let i = d.nets.iter().position(|n| n.name == "+12V").unwrap();
    let text = v.explain(&d, i);
    for want in [
        "+12V: 6.4 to 25.2 V",
        "D1 (B5819W SL)",
        "0.6 V",
        "p1",
        "VBUS: 7 to 25.2 V",
        "input VBUS on J1.1",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
}

/// No CAN bus range is in the inputs yet, so the bus nets stay unknown and say why.
#[test]
fn can_bus_is_unknown_without_an_input() {
    for dir in ["can-encoder", "can-encoder-rev1"] {
        let (d, v) = run(dir, "can-encoder.kicad_sch");
        for net in ["/CANH", "/CANL", "Net-(JP1-B)"] {
            let nv = v.by_name(&d, net).unwrap();
            assert_eq!(nv.span, Span::unknown(), "{dir} {net}");
            assert!(
                nv.why
                    .iter()
                    .any(|w| w.text.contains("connector pin with no range in inputs.toml")),
                "{dir} {net}: {:?}",
                nv.why
            );
        }
    }
}

/// Every net has a range, or says why one of its ends is unknown.
#[test]
fn every_net_is_known_or_says_why() {
    for (dir, sch) in [
        ("sniffer", "sniffer.kicad_sch"),
        ("can-encoder", "can-encoder.kicad_sch"),
        ("can-encoder-rev1", "can-encoder.kicad_sch"),
    ] {
        let (d, v) = run(dir, sch);
        assert_eq!(v.nets.len(), d.nets.len());
        for (net, nv) in d.nets.iter().zip(&v.nets) {
            assert!(!nv.why.is_empty(), "{dir} {}: no reason", net.name);
            if !nv.span.is_known() {
                let text = v.explain(&d, d.nets.iter().position(|n| n == net).unwrap());
                assert!(text.contains("UNKNOWN"), "{dir} {}:\n{text}", net.name);
            }
            if let (Some(lo), Some(hi)) = (nv.span.min, nv.span.max) {
                assert!(lo <= hi, "{dir} {}: {lo} > {hi}", net.name);
            }
        }
    }
}

fn with_vbus(min: f64, max: f64) -> BoardInputs {
    toml::from_str(&format!(
        "board = \"test\"\n[[input]]\nname = \"VBUS\"\npin = \"J1.1\"\nto = \"J1.4\"\nmin = {min}\nmax = {max}\nsource = \"test\"\n"
    ))
    .unwrap()
}

/// A regulator only holds its output while its supply is in range. Below that the low end drops to 0 V;
/// above the recommended maximum the output can follow the supply.
#[test]
fn regulators_hold_only_inside_their_supply_range() {
    let (d, _) = board("can-encoder", "can-encoder.kicad_sch");
    let lib = Library::load_dir(&root("parts")).unwrap();

    let low = compute(&d, &lib, &with_vbus(3.0, 12.0));
    let s = span(&d, &low, "+5V");
    assert_eq!(s.min, Some(0.0), "{s}");
    let why = &low.by_name(&d, "+5V").unwrap().why[0].text;
    assert!(why.contains("below its recommended minimum"), "{why}");

    // The TPS560430 is recommended up to 36 V (p4).
    let high = compute(&d, &lib, &with_vbus(7.0, 40.0));
    assert_eq!(span(&d, &high, "+5V").max, Some(40.0));
    let why = &high.by_name(&d, "+5V").unwrap().why[0].text;
    assert!(why.contains("above its recommended"), "{why}");
}

/// Without a resistor's tolerance the divider's output can't be bounded, and the rail says so.
#[test]
fn divider_needs_resistor_tolerances() {
    let (d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    lib.parts.remove("0402WGF1202TCE");
    let v = compute(&d, &lib, &inputs);
    let nv = v.by_name(&d, "+5V").unwrap();
    assert_eq!(nv.span, Span::unknown());
    assert!(
        nv.why[0].text.contains("R2 (0402WGF1202TCE) has no part file"),
        "{:?}",
        nv.why
    );
    // Whatever +5V powers is unknown at the top too.
    assert_eq!(span(&d, &v, "+3V3").max, None);
}

#[test]
fn resistor_values() {
    for (s, want) in [
        ("47k", 47e3),
        ("4k7", 4.7e3),
        ("120", 120.0),
        ("2R2", 2.2),
        ("1M", 1e6),
        ("10 kΩ", 10e3),
        ("1.5k", 1.5e3),
        ("0R", 0.0),
    ] {
        assert_eq!(parse_ohms(s), Some(want), "{s}");
    }
    for s in ["", "k", "1.5k7", "abc", "4k7x"] {
        assert_eq!(parse_ohms(s), None, "{s}");
    }
}

/// A connector pin on a driven net that inputs.toml doesn't describe makes the net UNKNOWN.
#[test]
fn connector_on_a_driven_net_is_unknown() {
    let (d, mut inputs) = board("can-encoder", "can-encoder.kicad_sch");
    inputs.undriven.clear();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "+3V3"), Span::unknown());
    assert!(
        text(&v, &d, "+3V3").contains("J3.5 is a connector pin"),
        "{}",
        text(&v, &d, "+3V3")
    );
    // And so is everything that reads +3V3.
    assert_eq!(span(&d, &v, "/LED").max, None);
}

/// Adds a two-pin part between two nets.
fn add_part(d: &mut Design, reference: &str, lib_id: &str, a: &str, b: &str) {
    use gerberus::design::{Component, Pin, PinKind};
    let mut c = Component {
        reference: reference.into(),
        value: "X".into(),
        lib_id: lib_id.into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    for (num, net) in [("1", a), ("2", b)] {
        let i = net_index(d, net);
        c.pins.insert(
            num.into(),
            Pin {
                number: num.into(),
                name: String::new(),
                kind: PinKind::Passive,
                no_connect: false,
                net: i,
            },
        );
        d.nets[i].pins.push((reference.into(), num.into()));
    }
    d.components.insert(reference.into(), c);
}

/// Anything on a feedback net besides the divider and capacitors makes the output UNKNOWN, not a guess.
#[test]
fn extra_parts_on_the_feedback_net_are_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_part(&mut d, "D9", "Device:D", "Net-(U1-FB)", "GND");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "Net-(U1-FB)"), Span::unknown());
    assert!(text(&v, &d, "Net-(U1-FB)").contains("D9.1 is on the feedback net"));
}

/// A part gerberus can't model (a plain diode here) may pull a net anywhere.
#[test]
fn unmodelled_passive_pins_make_a_net_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_part(&mut d, "D9", "Device:D", "+5V", "/BOOT0");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "/BOOT0"), Span::unknown());
    assert!(
        text(&v, &d, "/BOOT0").contains("D9.2 is a Passive pin"),
        "{}",
        text(&v, &d, "/BOOT0")
    );
}

/// A regulator fed from its own output can't be worked out.
#[test]
fn loops_are_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let out = net_index(&d, "+3V3");
    d.components.get_mut("U5").unwrap().pins.get_mut("3").unwrap().net = out;
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "+3V3"), Span::unknown());
    assert!(
        text(&v, &d, "+3V3").contains("depend on this net in turn"),
        "{}",
        text(&v, &d, "+3V3")
    );
}

/// A switch node's range that follows a pin other than its supply has no value for it.
#[test]
fn switch_range_needs_its_supply() {
    use gerberus::limits::{Bound, DriveKind, Range};
    let (d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    let sw = lib
        .parts
        .get_mut("TPS560430YFDBVR")
        .unwrap()
        .drives
        .iter_mut()
        .find(|d| d.kind == DriveKind::Switches)
        .unwrap();
    sw.range = Range::Known {
        min: Bound::Value(0.0),
        max: Bound::Pin {
            pin: "EN".into(),
            plus: 0.0,
            at_most: None,
        },
        page: Some(3),
        note: None,
    };
    let v = compute(&d, &lib, &inputs);
    assert_eq!(
        span(&d, &v, "Net-(U1-SW)"),
        Span {
            min: Some(0.0),
            max: None
        }
    );
    assert!(text(&v, &d, "Net-(U1-SW)").contains("highest point UNKNOWN"));
}

/// A board temperature range narrows the resistors' drift.
#[test]
fn board_temperature_narrows_divider_drift() {
    let (d, mut inputs) = board("can-encoder", "can-encoder.kicad_sch");
    inputs.temperature = Some(toml::from_str("min = 0\nmax = 70\nsource = \"test\"").unwrap());
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let (lo, hi) = divider((0.985, 1.015), 47e3, 12e3, 0.01 + 100e-6 * 45.0);
    assert!(close(span(&d, &v, "+5V"), lo, hi), "{}", span(&d, &v, "+5V"));
}

/// A net read twice in one explanation is spelled out once.
#[test]
fn explanations_dont_repeat() {
    let (d, v) = run("can-encoder", "can-encoder.kicad_sch");
    let text = v.explain(&d, net_index(&d, "/CAN_RX"));
    assert!(text.contains("(explained above)"), "{text}");
    assert_eq!(text.matches("input VBUS on J1.1").count(), 1, "{text}");
}
