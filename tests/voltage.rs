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

/// `vref * (1 + Rtop / Rbottom)` at the tolerance ends that make it smallest and largest.
fn divider(vref: (f64, f64), rt: f64, rb: f64, tol: f64) -> (f64, f64) {
    (
        vref.0 * (1.0 + rt * (1.0 - tol) / (rb * (1.0 + tol))),
        vref.1 * (1.0 + rt * (1.0 + tol) / (rb * (1.0 - tol))),
    )
}

#[test]
fn rev2_rails() {
    let (d, v) = run("can-encoder", "can-encoder.kicad_sch");
    assert!(close(span(&d, &v, "VBUS"), 7.0, 26.0));
    assert!(close(span(&d, &v, "GND"), 0.0, 0.0));
    // B5819WS drops at most 0.6 V (p1), and no minimum is given.
    assert!(close(span(&d, &v, "+VDC"), 6.4, 26.0), "{}", span(&d, &v, "+VDC"));
    let (lo, hi) = divider((0.985, 1.015), 47e3, 12e3, 0.01);
    assert!(close(span(&d, &v, "+5V"), lo, hi), "{}", span(&d, &v, "+5V"));
    assert!(close(span(&d, &v, "Net-(U1-FB)"), 0.985, 1.015));
    assert!(close(span(&d, &v, "+3V3"), 3.234, 3.366));
    // The switch node reaches VIN; how far below ground it rings isn't given.
    let sw = span(&d, &v, "Net-(U1-SW)");
    assert_eq!((sw.min, sw.max), (None, Some(26.0)));
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
    let (lo, hi) = divider((0.792, 0.808), 47e3, 15e3, 0.01);
    assert!(close(span(&d, &v, "+3V3"), lo, hi), "{}", span(&d, &v, "+3V3"));
    assert!(close(span(&d, &v, "/LED"), 0.0, hi));
}

#[test]
fn sniffer_rails() {
    let (d, v) = run("sniffer", "sniffer.kicad_sch");
    assert!(close(span(&d, &v, "+5V"), 3.6, 5.5));
    assert!(close(span(&d, &v, "+3V3"), 3.234, 3.366));
    assert!(close(span(&d, &v, "/UART_RX"), 0.0, 3.3));
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
