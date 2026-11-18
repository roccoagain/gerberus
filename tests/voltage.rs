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

fn divider(vref: (f64, f64), rt: f64, rb: f64, dev: f64) -> (f64, f64) {
    (
        vref.0 * (1.0 + rt * (1.0 - dev) / (rb * (1.0 + dev))),
        vref.1 * (1.0 + rt * (1.0 + dev) / (rb * (1.0 - dev))),
    )
}

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
    assert!(close(span(&d, &v, "+VDC"), 6.4, 26.0), "{}", span(&d, &v, "+VDC"));
    let (lo, hi) = divider((0.985, 1.015), 47e3, 12e3, DEV);
    assert!(close(span(&d, &v, "+5V"), lo, hi), "{}", span(&d, &v, "+5V"));
    assert!(close(span(&d, &v, "Net-(U1-FB)"), 0.985, 1.015));
    assert!(lo >= 3.366 + 1.0);
    assert!(close(span(&d, &v, "+3V3"), 3.234, 3.366));
    assert_eq!(span(&d, &v, "Net-(U1-SW)"), Span::unknown());
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
    assert!(close(span(&d, &v, "+3V3"), 0.0, 3.366));
    assert!(
        text(&v, &d, "+3V3").contains("below the 4.366 V"),
        "{}",
        text(&v, &d, "+3V3")
    );
    assert!(close(span(&d, &v, "/UART_RX"), 0.0, 3.3));
    assert!(close(span(&d, &v, "/UART_TX"), 0.0, 3.366));
    assert!(close(span(&d, &v, "Net-(D1-A)"), 0.0, 3.366));
}

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

#[test]
fn can_bus_is_unknown_without_an_input() {
    for dir in ["can-encoder", "can-encoder-rev1"] {
        let (d, mut inputs) = board(dir, "can-encoder.kicad_sch");
        inputs
            .inputs
            .retain(|i| i.pin.pin != "2" && i.pin.pin != "3" || i.pin.reference != "J1");
        let lib = Library::load_dir(&root("parts")).unwrap();
        let v = compute(&d, &lib, &inputs);
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

#[test]
fn regulators_hold_only_inside_their_supply_range() {
    let (d, _) = board("can-encoder", "can-encoder.kicad_sch");
    let lib = Library::load_dir(&root("parts")).unwrap();

    let low = compute(&d, &lib, &with_vbus(3.0, 12.0));
    let s = span(&d, &low, "+5V");
    assert_eq!(s.min, Some(0.0), "{s}");
    let why = &low.by_name(&d, "+5V").unwrap().why[0].text;
    assert!(why.contains("below its recommended minimum"), "{why}");

    let high = compute(&d, &lib, &with_vbus(7.0, 40.0));
    assert_eq!(span(&d, &high, "+5V").max, Some(40.0));
    let why = &high.by_name(&d, "+5V").unwrap().why[0].text;
    assert!(why.contains("above its recommended"), "{why}");
}

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
    assert_eq!(span(&d, &v, "/LED").max, None);
}

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
                stack: None,
                net: i,
            },
        );
        d.nets[i].pins.push((reference.into(), num.into()));
    }
    d.components.insert(reference.into(), c);
}

#[test]
fn extra_parts_on_the_feedback_net_are_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_part(&mut d, "D9", "Device:D", "Net-(U1-FB)", "GND");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "Net-(U1-FB)"), Span::unknown());
    assert!(text(&v, &d, "Net-(U1-FB)").contains("D9.1 is on the feedback net"));
}

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
        derived: None,
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

#[test]
fn board_temperature_narrows_divider_drift() {
    let (d, mut inputs) = board("can-encoder", "can-encoder.kicad_sch");
    inputs.temperature = Some(toml::from_str("min = 0\nmax = 70\nsource = \"test\"").unwrap());
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let (lo, hi) = divider((0.985, 1.015), 47e3, 12e3, 0.01 + 100e-6 * 45.0);
    assert!(close(span(&d, &v, "+5V"), lo, hi), "{}", span(&d, &v, "+5V"));
}

#[test]
fn explanations_dont_repeat() {
    let (d, v) = run("can-encoder", "can-encoder.kicad_sch");
    let text = v.explain(&d, net_index(&d, "/CAN_RX"));
    assert!(text.contains("(explained above)"), "{text}");
    assert_eq!(text.matches("input VBUS on J1.1").count(), 1, "{text}");
}

#[test]
fn stays_below_needs_bounded_steps() {
    use gerberus::limits::{Bound, DriveKind, Range};
    let (d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let (sw, vdc) = (net_index(&d, "Net-(U1-SW)"), net_index(&d, "+VDC"));
    let with_plus = |plus: f64| {
        let mut lib = Library::load_dir(&root("parts")).unwrap();
        let drive = lib
            .parts
            .get_mut("TPS560430YFDBVR")
            .unwrap()
            .drives
            .iter_mut()
            .find(|d| d.kind == DriveKind::Switches)
            .unwrap();
        drive.range = Range::Known {
            min: Bound::Value(0.0),
            max: Bound::Pin {
                pin: "VIN".into(),
                plus,
                at_most: None,
            },
            page: Some(3),
            note: None,
            derived: None,
        };
        compute(&d, &lib, &inputs)
    };
    assert!(with_plus(0.0).stays_below(sw, vdc));
    assert!(!with_plus(0.7).stays_below(sw, vdc));
    let (_, v) = run("can-encoder", "can-encoder.kicad_sch");
    assert!(v.stays_below(net_index(&d, "/SPI_CS"), net_index(&d, "+3V3")));
    assert!(
        !v.stays_below(net_index(&d, "/CAN_RX"), net_index(&d, "+3V3")),
        "U4 also runs from +5V"
    );
}

#[test]
fn can_bus_lines() {
    for dir in ["can-encoder", "can-encoder-rev1"] {
        let (d, v) = run(dir, "can-encoder.kicad_sch");
        for (net, lo, hi) in [("/CANH", -2.06, 9.5), ("/CANL", -4.5, 7.06), ("Net-(JP1-B)", -4.5, 9.5)] {
            assert!(close(span(&d, &v, net), lo, hi), "{dir} {net}: {}", span(&d, &v, net));
        }
        let jp = net_index(&d, "Net-(JP1-B)");
        let mut around: Vec<&str> = v.nets[jp].between.iter().map(|&n| d.nets[n].name.as_str()).collect();
        around.sort_unstable();
        assert_eq!(around, ["/CANH", "/CANL"], "{dir}");
        assert!(v.nets[net_index(&d, "/CANH")].between.is_empty());
    }
}

#[test]
fn probe_pins_cover_any_probe() {
    for (dir, sch) in [
        ("can-encoder", "can-encoder.kicad_sch"),
        ("can-encoder-rev1", "can-encoder.kicad_sch"),
        ("sniffer", "sniffer.kicad_sch"),
    ] {
        let (d, v) = run(dir, sch);
        let rail = span(&d, &v, "+3V3").max.unwrap();
        for net in ["/SWDIO", "/SWCLK", "/NRST"] {
            let s = span(&d, &v, net);
            assert_eq!((s.min, s.max), (Some(0.0), Some(rail.max(3.3))), "{dir} {net}");
            assert!(!v.stays_below(net_index(&d, net), net_index(&d, "+3V3")), "{dir} {net}");
            let t = text(&v, &d, net);
            assert!(
                t.contains("up to the voltage on") && t.contains("fixed at 3.3 V"),
                "{dir} {net}: {t}"
            );
        }
    }
}

#[test]
fn fault_cases_replace_normal_inputs() {
    let (d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    assert_eq!(inputs.faults(), ["VBUS shorted onto CANH", "VBUS shorted onto CANL"]);
    let case = inputs.case(&d, "VBUS shorted onto CANH");
    let names: Vec<&str> = case.inputs.iter().map(|i| i.name.as_str()).collect();
    assert!(
        names.contains(&"CANH shorted to VBUS") && names.contains(&"CANL"),
        "{names:?}"
    );
    assert!(
        !names.contains(&"CANH") && !names.contains(&"CANL shorted to VBUS"),
        "{names:?}"
    );
    assert!(case.inputs.iter().all(|i| i.fault.is_none()));
    assert!(case.differential.is_empty());
    assert!(case.problems(&d).is_empty(), "{:?}", case.problems(&d));

    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &case);
    assert!(close(span(&d, &v, "/CANH"), 7.0, 26.0));
    assert!(close(span(&d, &v, "/CANL"), -4.5, 7.06));
    assert!(close(span(&d, &v, "Net-(JP1-B)"), -4.5, 26.0));
}

fn add_fet(d: &mut Design, value: &str, gate: &str, source: &str, drain: &str) {
    use gerberus::design::{Component, Net, Pin, PinKind};
    d.nets.push(Net {
        name: "LOAD".into(),
        pins: vec![],
        labels: vec![],
    });
    add_part(d, "R99", "Device:R", "LOAD", "+5V");
    let mut c = Component {
        reference: "Q9".into(),
        value: value.into(),
        lib_id: "Transistor_FET:2N7002".into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    for (num, name, net) in [("1", "G", gate), ("2", "S", source), ("3", "D", drain)] {
        let i = net_index(d, net);
        c.pins.insert(
            num.into(),
            Pin {
                number: num.into(),
                name: name.into(),
                kind: PinKind::Passive,
                no_connect: false,
                stack: None,
                net: i,
            },
        );
        d.nets[i].pins.push(("Q9".into(), num.into()));
    }
    d.components.insert("Q9".into(), c);
}

#[test]
fn a_mosfet_with_no_part_file_is_unknown() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_fet(&mut d, "IRLML6344", "/LED", "GND", "LOAD");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "LOAD"), Span::unknown());
    assert!(
        text(&v, &d, "LOAD").contains("Q9.3 is a Passive pin"),
        "{}",
        text(&v, &d, "LOAD")
    );
}

#[test]
fn a_mosfet_held_below_threshold_is_off() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_fet(&mut d, "2N7002NXAKR", "GND", "GND", "LOAD");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "LOAD"), span(&d, &v, "+5V"));
    let t = text(&v, &d, "LOAD");
    assert!(t.contains("Q9's channel is off"), "{t}");
    let q9 = v.channels.iter().find(|c| c.reference == "Q9").unwrap();
    assert_eq!(q9.state, gerberus::voltage::ChannelState::Off);
}

#[test]
fn a_mosfet_gate_decides_its_state() {
    use gerberus::voltage::ChannelState;
    let lib = Library::load_dir(&root("parts")).unwrap();
    for (gate, state) in [("/LED", ChannelState::Maybe), ("VBUS", ChannelState::On)] {
        let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
        add_fet(&mut d, "2N7002NXAKR", gate, "GND", "LOAD");
        let v = compute(&d, &lib, &inputs);
        let q9 = v.channels.iter().find(|c| c.reference == "Q9").unwrap();
        assert_eq!(q9.state, state, "{}", q9.reason);
        let top = span(&d, &v, "+5V").max.unwrap();
        assert!(close(span(&d, &v, "LOAD"), 0.0, top), "{}", span(&d, &v, "LOAD"));
    }
}

#[test]
fn a_p_channel_mosfet_reads_its_threshold_negative() {
    use gerberus::voltage::ChannelState;
    let lib = Library::load_dir(&root("parts")).unwrap();
    for (gate, state) in [("VBUS", ChannelState::Off), ("GND", ChannelState::On)] {
        let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
        add_fet(&mut d, "AO3401A", gate, "LOAD", "+5V");
        let v = compute(&d, &lib, &inputs);
        let q9 = v.channels.iter().find(|c| c.reference == "Q9").unwrap();
        assert_eq!(q9.state, state, "{}", q9.reason);
    }
}

#[test]
fn a_channel_cant_pull_past_its_gate() {
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_fet(&mut d, "2N7002NXAKR", "+3V3", "/SPI_CAL", "LOAD");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let cal = span(&d, &v, "/SPI_CAL");
    let gate = span(&d, &v, "+3V3");
    assert!(
        cal.max.is_none() || cal.max <= gate.max,
        "{cal}\n{}",
        text(&v, &d, "/SPI_CAL")
    );
}

#[test]
fn a_floating_gate_leaves_its_channel_maybe_on() {
    use gerberus::voltage::ChannelState;
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    add_fet(&mut d, "2N7002NXAKR", "GND", "GND", "LOAD");
    use gerberus::design::{Component, Net, Pin, PinKind};
    d.nets.push(Net {
        name: "X".into(),
        pins: vec![],
        labels: vec![],
    });
    let mut c = Component {
        reference: "Q8".into(),
        value: "2N7002NXAKR".into(),
        lib_id: "Transistor_FET:2N7002".into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    for (num, name, net) in [("1", "G", "X"), ("2", "S", "GND"), ("3", "D", "/LED")] {
        let i = net_index(&d, net);
        c.pins.insert(
            num.into(),
            Pin {
                number: num.into(),
                name: name.into(),
                kind: PinKind::Passive,
                no_connect: false,
                stack: None,
                net: i,
            },
        );
        d.nets[i].pins.push(("Q8".into(), num.into()));
    }
    d.components.insert("Q8".into(), c);
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    let q8 = v.channels.iter().find(|c| c.reference == "Q8").unwrap();
    assert_eq!(q8.state, ChannelState::Maybe, "{}", q8.reason);
}

#[test]
fn fuses_and_net_ties_pass_a_rail_through_but_switches_dont() {
    use gerberus::design::Net;
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    for name in ["FUSED", "TIED", "SWITCHED"] {
        d.nets.push(Net {
            name: name.into(),
            pins: vec![],
            labels: vec![],
        });
    }
    add_part(&mut d, "F9", "Device:Fuse", "VBUS", "FUSED");
    add_part(&mut d, "NT9", "Device:NetTie_2", "+5V", "TIED");
    add_part(&mut d, "SW9", "Switch:SW_SPST", "+3V3", "SWITCHED");
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "FUSED"), span(&d, &v, "VBUS"));
    assert!(
        text(&v, &d, "FUSED").contains("through fuse F9"),
        "{}",
        text(&v, &d, "FUSED")
    );
    assert_eq!(span(&d, &v, "TIED"), span(&d, &v, "+5V"));
    assert!(
        text(&v, &d, "TIED").contains("through net tie NT9"),
        "{}",
        text(&v, &d, "TIED")
    );
    assert_ne!(span(&d, &v, "SWITCHED"), span(&d, &v, "+3V3"));
    assert!(
        text(&v, &d, "SWITCHED").contains("UNKNOWN"),
        "{}",
        text(&v, &d, "SWITCHED")
    );
}

#[test]
fn a_three_pin_net_tie_joins_every_net() {
    use gerberus::design::{Component, Net, Pin, PinKind};
    let (mut d, inputs) = board("can-encoder", "can-encoder.kicad_sch");
    let mut c = Component {
        reference: "NT8".into(),
        value: "X".into(),
        lib_id: "Device:NetTie_3".into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    for (num, name) in [("1", "+5V"), ("2", "TIE_A"), ("3", "TIE_B")] {
        let i = d.nets.iter().position(|n| n.name == name).unwrap_or_else(|| {
            d.nets.push(Net {
                name: name.into(),
                pins: vec![],
                labels: vec![],
            });
            d.nets.len() - 1
        });
        c.pins.insert(
            num.into(),
            Pin {
                number: num.into(),
                name: String::new(),
                kind: PinKind::Passive,
                no_connect: false,
                net: i,
                stack: None,
            },
        );
        d.nets[i].pins.push(("NT8".into(), num.into()));
    }
    d.components.insert("NT8".into(), c);
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&d, &lib, &inputs);
    assert_eq!(span(&d, &v, "TIE_A"), span(&d, &v, "+5V"));
    assert_eq!(span(&d, &v, "TIE_B"), span(&d, &v, "+5V"));
}
