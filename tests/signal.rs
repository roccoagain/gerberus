use std::path::Path;

use gerberus::design::{Component, Design, Net, Pin, PinKind};
use gerberus::inputs::BoardInputs;
use gerberus::limits::Library;
use gerberus::rules::Verdict;
use gerberus::signal::{check, levels};
use gerberus::voltage::compute;

fn stage() -> Design {
    let nets = ["GND", "+5V", "IN", "PLUS", "VREF", "OUT", "MINUS"];
    let mut d = Design {
        nets: nets
            .iter()
            .map(|n| Net {
                name: (*n).into(),
                pins: vec![],
                labels: vec![],
            })
            .collect(),
        ..Default::default()
    };
    let net = |name: &str| nets.iter().position(|n| *n == name).unwrap();
    let mut add = |r: &str, lib_id: &str, value: &str, pins: &[(&str, &str, PinKind, &str)]| {
        let mut c = Component {
            reference: r.into(),
            value: value.into(),
            lib_id: lib_id.into(),
            in_bom: true,
            on_board: true,
            ..Default::default()
        };
        for &(num, name, kind, n) in pins {
            let i = net(n);
            c.pins.insert(
                num.into(),
                Pin {
                    number: num.into(),
                    name: name.into(),
                    kind,
                    no_connect: false,
                    net: i,
                    stack: None,
                },
            );
            d.nets[i].pins.push((r.into(), num.into()));
        }
        d.components.insert(r.into(), c);
    };
    let p = PinKind::Passive;
    add(
        "J1",
        "Connector:Conn_01x02",
        "IN",
        &[("1", "1", p, "IN"), ("2", "2", p, "GND")],
    );
    add(
        "J2",
        "Connector:Conn_01x02",
        "PWR",
        &[("1", "1", p, "+5V"), ("2", "2", p, "GND")],
    );
    add("C1", "Device:C", "1u", &[("1", "", p, "IN"), ("2", "", p, "PLUS")]);
    add("R1", "Device:R", "100k", &[("1", "", p, "PLUS"), ("2", "", p, "VREF")]);
    add("R2", "Device:R", "10k", &[("1", "", p, "+5V"), ("2", "", p, "VREF")]);
    add("R3", "Device:R", "10k", &[("1", "", p, "VREF"), ("2", "", p, "GND")]);
    add("C2", "Device:C", "10u", &[("1", "", p, "VREF"), ("2", "", p, "GND")]);
    add("R4", "Device:R", "10k", &[("1", "", p, "OUT"), ("2", "", p, "MINUS")]);
    add("R5", "Device:R", "10k", &[("1", "", p, "MINUS"), ("2", "", p, "VREF")]);
    add(
        "U1",
        "Amplifier_Operational:TLV9061xDBV",
        "TLV9061QDBVRQ1",
        &[
            ("1", "OUT", PinKind::Output, "OUT"),
            ("2", "V-", PinKind::PowerIn, "GND"),
            ("3", "+", PinKind::Input, "PLUS"),
            ("4", "-", PinKind::Input, "MINUS"),
            ("5", "V+", PinKind::PowerIn, "+5V"),
        ],
    );
    d
}

fn inputs(peak: f64) -> BoardInputs {
    toml::from_str(&format!(
        r#"
board = "stage"

[[input]]
name = "5V"
pin = "J2.1"
to = "J2.2"
min = 5
max = 5
source = "test"

[[input]]
name = "sig"
pin = "J1.1"
to = "J1.2"
min = -4
max = 4
source = "test"

[[signal]]
name = "sig"
pin = "J1.1"
to = "J1.2"
peak = {peak}
source = "test"
"#
    ))
    .unwrap()
}

fn run(peak: f64) -> (Design, Vec<gerberus::rules::Finding>, gerberus::signal::Levels) {
    let d = stage();
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    let i = inputs(peak);
    assert!(i.problems(&d).is_empty(), "{:?}", i.problems(&d));
    let v = compute(&d, &lib, &i);
    let l = levels(&d, &lib, &i, &v).unwrap();
    (d.clone(), check(&d, &lib, &l), l)
}

fn clipping(f: &[gerberus::rules::Finding]) -> &gerberus::rules::Finding {
    f.iter().find(|f| f.rule == "clipping").expect("a clipping finding")
}

#[test]
fn a_stage_with_room_passes() {
    let (d, f, l) = run(1.0);
    let out = d.nets.iter().position(|n| n.name == "OUT").unwrap();
    let gnd = d.nets.iter().position(|n| n.name == "GND").unwrap();
    let (lo, hi) = l.dc(out, gnd).unwrap();
    assert!((lo - 2.5).abs() < 1e-3 && (hi - 2.5).abs() < 1e-3, "{lo} {hi}");
    assert!((l.peak[&out] - 2.0).abs() < 0.01, "{:?}", l.peak);
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Pass, "{}", c.message);
    assert!(c.message.contains("p10"), "{}", c.message);
}

#[test]
fn a_stage_inside_its_swing_clips() {
    let (_, f, _) = run(1.245);
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Fail, "{}", c.message);
    assert!(
        c.message.contains("closer than its 0.02 V output swing"),
        "{}",
        c.message
    );
}

#[test]
fn a_stage_past_its_rails_clips() {
    let (_, f, _) = run(1.5);
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Fail, "{}", c.message);
    assert!(c.message.contains("past a rail"), "{}", c.message);
    let plus = f
        .iter()
        .find(|f| f.rule == "signal-level" && f.message.starts_with("U1.+IN"))
        .unwrap();
    assert_eq!(plus.verdict, Verdict::Pass, "{}", plus.message);
}

#[test]
fn a_signal_past_an_absolute_maximum_is_an_error() {
    let (_, f, _) = run(3.2);
    let plus = f
        .iter()
        .find(|f| f.rule == "signal-level" && f.message.starts_with("U1.+IN"))
        .unwrap();
    assert_eq!(plus.verdict, Verdict::Fail, "{}", plus.message);
    assert_eq!(plus.severity, gerberus::rules::Severity::Error, "{}", plus.message);
}

#[test]
fn no_signals_says_what_to_add() {
    let d = stage();
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    let mut i = inputs(1.0);
    i.signals.clear();
    let v = compute(&d, &lib, &i);
    let e = levels(&d, &lib, &i, &v).unwrap_err();
    assert!(e.contains("[[signal]]"), "{e}");
}

fn net(d: &Design, name: &str) -> usize {
    d.nets.iter().position(|n| n.name == name).unwrap()
}

fn rewire(d: &mut Design, r: &str, pin: &str, to: &str) {
    let to = net(d, to);
    let p = d.components.get_mut(r).unwrap().pins.get_mut(pin).unwrap();
    let from = p.net;
    p.net = to;
    d.nets[from].pins.retain(|(x, y)| !(x == r && y == pin));
    d.nets[to].pins.push((r.into(), pin.into()));
}

fn run_on(d: &Design, i: &BoardInputs) -> (Vec<gerberus::rules::Finding>, gerberus::signal::Levels) {
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    let v = compute(d, &lib, i);
    let l = levels(d, &lib, i, &v).unwrap();
    (check(d, &lib, &l), l)
}

#[test]
fn a_rail_range_moves_the_bias() {
    let d = stage();
    let mut i = inputs(1.0);
    i.inputs[0].min = 4.5;
    i.inputs[0].max = gerberus::inputs::Max::Value(5.5);
    let (_, l) = run_on(&d, &i);
    let (lo, hi) = l.dc(net(&d, "OUT"), net(&d, "GND")).unwrap();
    assert!((lo - 2.25).abs() < 1e-3 && (hi - 2.75).abs() < 1e-3, "{lo} {hi}");
    let (lo, hi) = l.dc(net(&d, "OUT"), net(&d, "+5V")).unwrap();
    assert!((lo + 2.75).abs() < 1e-3 && (hi + 2.25).abs() < 1e-3, "{lo} {hi}");
}

#[test]
fn a_net_pulled_to_zero_is_not_ground() {
    let mut d = stage();
    rewire(&mut d, "R1", "2", "GND");
    let (f, l) = run_on(&d, &inputs(0.1));
    assert!(!l.ground.contains(&net(&d, "PLUS")));
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Fail, "{}", c.message);
}

#[test]
fn an_unbiased_input_is_unknown() {
    let mut d = stage();
    d.components.remove("R1");
    for n in &mut d.nets {
        n.pins.retain(|(r, _)| r != "R1");
    }
    let (f, l) = run_on(&d, &inputs(0.1));
    assert!(!l.bias.contains_key(&net(&d, "PLUS")));
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Unknown, "{}", c.message);
}

#[test]
fn an_ac_coupled_source_leaves_the_bias_to_the_board() {
    let mut d = stage();
    rewire(&mut d, "J1", "1", "PLUS");
    let mut i = inputs(1.0);
    i.inputs.retain(|x| x.name != "sig");
    i.signals[0].ac_coupled = true;
    let (f, l) = run_on(&d, &i);
    let (lo, hi) = l.dc(net(&d, "PLUS"), net(&d, "GND")).unwrap();
    assert!((lo - 2.5).abs() < 1e-3 && (hi - 2.5).abs() < 1e-3, "{lo} {hi}");
    assert_eq!(clipping(&f).verdict, Verdict::Pass, "{}", clipping(&f).message);
    i.signals[0].ac_coupled = false;
    let (_, l) = run_on(&d, &i);
    let (lo, _) = l.dc(net(&d, "PLUS"), net(&d, "GND")).unwrap();
    assert!(lo.abs() < 1e-3, "{lo}");
}

#[test]
fn an_open_part_on_the_path_is_reported() {
    let mut d = stage();
    let mut c = d.components["R1"].clone();
    c.reference = "D1".into();
    c.lib_id = "Device:D".into();
    c.value = "1N4148".into();
    d.components.insert("D1".into(), c);
    for n in &mut d.nets {
        if n.pins.iter().any(|(r, _)| r == "R1") {
            let p = n.pins.iter().find(|(r, _)| r == "R1").unwrap().1.clone();
            n.pins.push(("D1".into(), p));
        }
    }
    let (f, _) = run_on(&d, &inputs(0.1));
    let s = f
        .iter()
        .find(|f| f.rule == "signal-path")
        .expect("a signal-path finding");
    assert_eq!(s.verdict, Verdict::Unknown);
    assert!(s.message.contains("D1"), "{}", s.message);
}

fn add(d: &mut Design, r: &str, lib_id: &str, value: &str, pins: &[(&str, PinKind, &str)]) {
    let mut c = Component {
        reference: r.into(),
        value: value.into(),
        lib_id: lib_id.into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    for &(num, kind, n) in pins {
        let i = d.nets.iter().position(|x| x.name == n).unwrap_or_else(|| {
            d.nets.push(Net {
                name: n.into(),
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
                kind,
                no_connect: false,
                net: i,
                stack: None,
            },
        );
        d.nets[i].pins.push((r.into(), num.into()));
    }
    d.components.insert(r.into(), c);
}

#[test]
fn a_dc_coupled_signal_sits_at_its_input() {
    let mut d = stage();
    rewire(&mut d, "J1", "1", "PLUS");
    let mut i = inputs(0.1);
    i.inputs[1].min = 4.9;
    i.inputs[1].max = gerberus::inputs::Max::Value(5.2);
    let (f, l) = run_on(&d, &i);
    let (lo, hi) = l.dc(net(&d, "PLUS"), net(&d, "GND")).unwrap();
    assert!((lo - 5.0).abs() < 1e-3 && (hi - 5.1).abs() < 1e-3, "{lo} {hi}");
    let plus = f
        .iter()
        .find(|f| f.rule == "signal-level" && f.message.starts_with("U1.+IN"))
        .unwrap();
    assert_eq!(plus.verdict, Verdict::Fail, "{}", plus.message);
    i.inputs[1].min = 5.2;
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    let v = compute(&d, &lib, &i);
    assert!(levels(&d, &lib, &i, &v).unwrap_err().contains("narrower"));
}

#[test]
fn two_dc_signals_on_one_pin_are_an_error() {
    let d = stage();
    let mut i = inputs(0.5);
    let mut s2 = i.signals[0].clone();
    s2.name = "sig2".into();
    i.signals.push(s2);
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    let v = compute(&d, &lib, &i);
    assert!(levels(&d, &lib, &i, &v).unwrap_err().contains("ac_coupled"));
}

#[test]
fn an_op_amp_tied_off_is_left_out() {
    let mut d = stage();
    for p in ["1", "3", "4"] {
        rewire(&mut d, "U1", p, "GND");
    }
    let (_, l) = run_on(&d, &inputs(0.5));
    assert!(l.skipped.iter().any(|s| s.reference == "U1"), "{:?}", l.skipped);
}

#[test]
fn an_open_part_driving_a_net_makes_it_unknown() {
    let mut d = stage();
    add(
        &mut d,
        "U9",
        "Logic:Buffer",
        "X",
        &[("1", PinKind::Output, "PLUS"), ("2", PinKind::PowerIn, "+5V")],
    );
    let (f, _) = run_on(&d, &inputs(1.0));
    let plus = f
        .iter()
        .find(|f| f.rule == "signal-level" && f.message.starts_with("U1.+IN"))
        .unwrap();
    assert_eq!(plus.verdict, Verdict::Unknown, "{}", plus.message);
    assert!(plus.message.contains("U9 is left open"), "{}", plus.message);
    let mut d = stage();
    add(
        &mut d,
        "U9",
        "Logic:Buffer",
        "X",
        &[("1", PinKind::Input, "PLUS"), ("2", PinKind::PowerIn, "+5V")],
    );
    let (f, _) = run_on(&d, &inputs(1.0));
    assert!(!f.iter().any(|f| f.rule == "signal-path"));
    assert_eq!(clipping(&f).verdict, Verdict::Pass, "{}", clipping(&f).message);
}

#[test]
fn a_sharp_resonance_is_found() {
    let mut d = stage();
    let f0: f64 = 10f64.powf(3.2251);
    let c = 1e-6;
    let lh = 1.0 / ((2.0 * std::f64::consts::PI * f0).powi(2) * c);
    let q = (lh / c).sqrt();
    let p = PinKind::Passive;
    add(
        &mut d,
        "L9",
        "Device:L",
        &format!("{lh}"),
        &[("1", p, "IN"), ("2", p, "Y")],
    );
    add(&mut d, "R9", "Device:R", "1", &[("1", p, "Y"), ("2", p, "X")]);
    add(&mut d, "C9", "Device:C", "1u", &[("1", p, "X"), ("2", p, "GND")]);
    let (_, l) = run_on(&d, &inputs(1.0));
    let g = l.peak[&net(&d, "X")];
    assert!(g > 0.95 * q, "gain {g}, Q {q}");
}
