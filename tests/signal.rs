//! Signal levels through an op-amp stage, simulated in ngspice.

use std::path::Path;

use gerberus::design::{Component, Design, Net, Pin, PinKind};
use gerberus::inputs::BoardInputs;
use gerberus::limits::Library;
use gerberus::rules::Verdict;
use gerberus::signal::{check, levels};
use gerberus::voltage::compute;

/// A non-inverting stage with a gain of 2 about a mid-rail VREF: J1's signal through C1 onto U1's +IN, which R1
/// biases at VREF (R2/R3 from +5V, decoupled by C2). R4/R5 set the gain against VREF, so the output sits at
/// VREF and swings twice the signal.
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
min = -3
max = 3
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

/// The output sits at VREF (2.5 V) and swings twice the signal in band.
#[test]
fn a_stage_with_room_passes() {
    let (d, f, l) = run(1.0);
    let out = d.nets.iter().position(|n| n.name == "OUT").unwrap();
    let lv = l.corners[0].1[&out];
    assert!((lv.bias - 2.5).abs() < 1e-3, "{lv:?}");
    assert!((lv.peak - 2.0).abs() < 0.01, "{lv:?}");
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Pass, "{}", c.message);
    assert!(c.message.contains("p10"), "{}", c.message);
}

/// 1.245 V peak takes the output to within 10 mV of each rail, inside TLV9061-Q1's 20 mV swing (p10).
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

/// Past the rails it clips whatever the swing figure, and the op-amp's inputs go past their recommended range.
#[test]
fn a_stage_past_its_rails_clips() {
    let (_, f, _) = run(1.5);
    let c = clipping(&f);
    assert_eq!(c.verdict, Verdict::Fail, "{}", c.message);
    assert!(c.message.contains("past a rail"), "{}", c.message);
    // +IN swings 2.5 ± 1.5 V: inside V– − 0.1 to V+ + 0.1 (p8), so it passes.
    let plus = f
        .iter()
        .find(|f| f.rule == "signal-level" && f.message.starts_with("U1.+IN"))
        .unwrap();
    assert_eq!(plus.verdict, Verdict::Pass, "{}", plus.message);
}

/// A signal big enough to take +IN past V+ + 0.5 V breaks the absolute maximum, an error.
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

/// No [[signal]] in inputs.toml says what to add.
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
