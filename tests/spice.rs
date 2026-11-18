use std::path::{Path, PathBuf};

use gerberus::design::Design;
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::rules::{Finding, Severity, Verdict};
use gerberus::spice::{self, Measured, Rng, SpicePolicy, parse_farads, parse_henries};
use gerberus::voltage::{Voltages, compute};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn board(dir: &str) -> (Design, BoardInputs, Library, Voltages) {
    let d = root("tests/fixtures").join(dir);
    let sch = std::fs::read_dir(&d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "kicad_sch"))
        .unwrap();
    let design = kicad::load(&sch).unwrap();
    let inputs = BoardInputs::load(&d.join("inputs.toml")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&design, &lib, &inputs);
    (design, inputs, lib, v)
}

fn policy(runs: usize) -> SpicePolicy {
    let mut p = gerberus::policy::Policy::load(&root("policy.toml"))
        .unwrap()
        .spice
        .expect("policy.toml has a [spice] table");
    p.runs = runs;
    p
}

fn stress(dir: &str, runs: usize) -> (Design, spice::Sweep, Vec<Finding>) {
    let (d, inputs, lib, v) = board(dir);
    let p = policy(runs);
    let sweep = spice::sweep(&d, &lib, &inputs, &v, &p, 7).unwrap();
    let f = spice::check(&d, &lib, &sweep, &p);
    (d, sweep, f)
}

fn finding<'a>(f: &'a [Finding], start: &str) -> &'a Finding {
    f.iter()
        .find(|f| f.message.starts_with(start))
        .unwrap_or_else(|| panic!("no finding for {start}"))
}

#[test]
fn values_with_si_prefixes() {
    let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < b * 1e-9);
    assert!(close(parse_farads("100nF"), 100e-9));
    assert!((parse_farads("2u2").unwrap() - 2.2e-6).abs() < 1e-18);
    assert!(close(parse_farads("4.7µF 16V"), 4.7e-6));
    assert!(close(parse_farads("10p"), 10e-12));
    assert_eq!(parse_farads("DNP"), None);
    assert_eq!(parse_farads("1u2u"), None);
    assert!(close(parse_henries("6.8uH"), 6.8e-6));
    assert!((parse_henries("4u7").unwrap() - 4.7e-6).abs() < 1e-18);
}

#[test]
fn reads_ngspice_measurements() {
    let out = "Note: something = else here\np0max               =  8.010808e+00 at=  1.055000e-05\n\
               p0min               =  0.000000e+00 at=  0.000000e+00\np0end               =  5.000000e+00\n";
    let m = Measured::parse(out);
    let p = m.pair(0).unwrap();
    assert_eq!((p.max, p.min, p.end), (8.010_808, 0.0, 5.0));
    assert!(m.pair(1).is_none());
}

#[test]
fn the_sweep_is_repeatable() {
    let (mut a, mut b) = (Rng::new(3), Rng::new(3));
    for _ in 0..10 {
        let x = a.between(1.0, 2.0);
        assert_eq!(x, b.between(1.0, 2.0));
        assert!((1.0..=2.0).contains(&x));
    }
}

#[test]
fn netlist_of_the_sniffer() {
    let (d, _, lib, _) = board("sniffer");
    let (d2, inputs, _, v) = board("sniffer");
    let c = spice::build(&d2, &lib, &inputs, &v).unwrap();
    let pairs = c.pairs(&d, &lib);
    let (text, about) = c.netlist(&policy(0), &pairs, None);
    assert_eq!(about, "cable 1 µH");
    for want in [
        "Vin0 s0 0 PWL(0 0 0.000001 5.5)",
        "Lcab0 m0",
        "BRU1_0",
        "BLU2_0",
        ".model DM_D1 D(",
        ".tran",
        ".meas tran p0max MAX",
    ] {
        assert!(text.contains(want), "missing {want:?} in\n{text}");
    }
    assert!(c.inputs(&d).iter().any(|i| i.starts_with("VIN 5.5 V")));
    assert!(
        c.skipped()
            .iter()
            .any(|s| s.contains("UART RX") && s.contains("signal"))
    );
}

#[test]
fn sniffer_ldo_input_overshoots_on_hot_plug() {
    let (d, sweep, f) = stress("sniffer", 2);
    let u1 = finding(&f, "U1.VIN (+5V)");
    assert_eq!(
        (u1.verdict, u1.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        u1.message
    );
    for want in [
        "settles at 5.497 V",
        "peaks at 9.894 V",
        "past its absolute maximum of 6.5 V",
        "ME6211C33M5G-N",
        "run 0 (cable 1 µH)",
    ] {
        assert!(u1.message.contains(want), "missing {want:?}: {}", u1.message);
    }
    let c1 = finding(&f, "C1.1 (+5V)");
    assert_eq!(c1.verdict, Verdict::Pass, "{}", c1.message);
    let vdd = finding(&f, "U2.VDD/VDDA (+3V3)");
    assert_eq!(vdd.verdict, Verdict::Pass, "{}", vdd.message);
    assert_eq!(sweep.runs.len(), 3);
    let text = spice::describe(&d, &sweep);
    assert!(text.contains("3 runs") && text.contains("VIN 5.5 V on +5V"), "{text}");
    assert!(text.contains("aren't reported"), "{text}");
}

#[test]
fn limits_between_two_pins_are_measured() {
    let (d, sweep, f) = stress("can-encoder-rev1", 0);
    finding(&f, "D1.K (+12V) from A (VBUS)");
    let (max, _, min, _) = sweep.peak(net(&d, "+12V"), Some(net(&d, "VBUS"))).unwrap();
    assert!(max > 5.0 && min < -1.0, "{max} {min}");
    assert!(!sweep.limits().is_empty());
    for &(a, b) in sweep.limits() {
        assert!(sweep.peak(a, b).is_some(), "{} unmeasured", d.nets[a].name);
    }
}

#[test]
fn netlist_names_and_tvs_models() {
    let (d, inputs, lib, v) = board("can-encoder-rev1");
    let c = spice::build(&d, &lib, &inputs, &v).unwrap();
    let pairs = c.pairs(&d, &lib);
    let (text, _) = c.netlist(&policy(0), &pairs, None);
    let mut names: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with(['.', '*']))
        .filter_map(|l| l.split_whitespace().next())
        .map(str::trim)
        .collect();
    let all = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), all, "duplicate names in\n{text}");
    assert!(text.contains("RS=0.5182 BV=22.1"), "{text}");
    let line = |start: &str| {
        text.lines()
            .find(|l| l.starts_with(start))
            .unwrap_or_else(|| panic!("{start}"))
    };
    let (a, b) = (line("DU5_0_0a "), line("DU5_0_0b "));
    let mid = |l: &str| l.split_whitespace().nth(2).unwrap().to_string();
    assert_eq!(mid(a), mid(b), "{a} / {b}");
    assert!(text.contains("RS=1.6 BV=32"), "{text}");
}

#[test]
fn a_tvs_held_past_breakdown_fails() {
    let (_, _, f) = stress("can-encoder-rev1", 0);
    let d2 = finding(&f, "D2.K (+12V)");
    assert_eq!(
        (d2.verdict, d2.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        d2.message
    );
    assert!(
        d2.message.contains("breakdown minimum of 20 V (SMAJ18A Rev:2025A6 p2)"),
        "{}",
        d2.message
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

fn net(d: &Design, name: &str) -> usize {
    d.nets.iter().position(|n| n.name == name).unwrap()
}

fn rerun(d: &Design, inputs: &BoardInputs, lib: &Library) -> (spice::Sweep, Vec<Finding>) {
    let v = compute(d, lib, inputs);
    let p = policy(0);
    let sweep = spice::sweep(d, lib, inputs, &v, &p, 7).unwrap();
    let f = spice::check(d, lib, &sweep, &p);
    (sweep, f)
}

#[test]
fn a_pfet_in_the_power_path_passes_power() {
    let (mut d, inputs, lib, _) = board("can-encoder");
    let (vbus, vdc, gnd) = (net(&d, "VBUS"), net(&d, "+VDC"), net(&d, "GND"));
    let spare = new_net(&mut d, "SPARE");
    for pin in ["1", "2"] {
        move_pin(&mut d, "D1", pin, spare);
    }
    let mut q = Component {
        reference: "Q9".into(),
        lib_id: "Device:Q_PMOS".into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    q.fields.insert("MPN".into(), "AO3401A".into());
    for (number, n) in [("1", gnd), ("2", vdc), ("3", vbus)] {
        d.nets[n].pins.push(("Q9".into(), number.into()));
        q.pins.insert(
            number.into(),
            Pin {
                number: number.into(),
                name: String::new(),
                kind: PinKind::Passive,
                no_connect: false,
                net: n,
                stack: None,
            },
        );
    }
    d.components.insert("Q9".into(), q);
    let (sweep, f) = rerun(&d, &inputs, &lib);
    assert!(sweep.settled(vdc, None).unwrap() > 20.0);
    let u1 = finding(&f, "U1.VIN (+VDC)");
    assert_ne!(u1.verdict, Verdict::Unknown, "{}", u1.message);
}

#[test]
fn a_relative_maximum_is_checked_against_its_pin() {
    let (mut d, inputs, lib, _) = board("can-encoder");
    let (_, _, f) = stress("can-encoder", 0);
    assert_eq!(finding(&f, "U3.CSN").verdict, Verdict::Pass);
    let five = net(&d, "+5V");
    let csn = d.components["U3"]
        .pins
        .iter()
        .find(|(_, p)| d.nets[p.net].name == "/SPI_CS")
        .map(|(n, _)| n.clone())
        .unwrap();
    move_pin(&mut d, "U3", &csn, five);
    let (_, f) = rerun(&d, &inputs, &lib);
    let u3 = finding(&f, "U3.CSN");
    assert_eq!(u3.verdict, Verdict::Fail, "{}", u3.message);
    assert!(u3.message.contains("above VDD"), "{}", u3.message);
}

#[test]
fn switch_nodes_are_unknown() {
    let (_, _, f) = stress("can-encoder-rev1", 1);
    let sw = finding(&f, "U1.SW");
    assert_eq!(sw.verdict, Verdict::Unknown, "{}", sw.message);
    assert!(sw.message.contains("switching on SW isn't modelled"), "{}", sw.message);
    assert!(!f.iter().any(|f| f.message.starts_with("U4.CANH")));
    let vin = finding(&f, "U1.VIN (+12V)");
    assert_eq!(vin.verdict, Verdict::Pass, "{}", vin.message);
}

#[test]
fn worst_run_is_reported() {
    let (d, sweep, f) = stress("can-encoder", 3);
    assert_eq!(sweep.runs.len(), 4);
    assert!(sweep.runs.iter().all(|r| r.starts_with("cable ")));
    let vdc = d.nets.iter().position(|n| n.name == "+VDC").unwrap();
    let (max, _, _, _) = sweep.peak(vdc, None).unwrap();
    assert!(max > sweep.settled(vdc, None).unwrap() + 1.0, "{max}");
    let u1 = finding(&f, "U1.VIN (+VDC)");
    assert_eq!(
        (u1.verdict, u1.severity),
        (Verdict::Fail, Severity::Error),
        "{}",
        u1.message
    );
    let c1 = finding(&f, "C1.1 (+VDC)");
    assert_eq!(
        (c1.verdict, c1.severity),
        (Verdict::Fail, Severity::Warning),
        "{}",
        c1.message
    );
}

#[test]
fn a_board_with_no_supply_input_is_an_error() {
    let (d, mut inputs, lib, v) = board("sniffer");
    inputs.inputs.retain(|i| i.min <= 0.0);
    let e = spice::build(&d, &lib, &inputs, &v).unwrap_err();
    assert!(e.contains("no supply input"), "{e}");
}

#[test]
fn stress_command() {
    let dir = root("tests/fixtures/sniffer");
    let out_dir = std::env::temp_dir().join(format!("gerberus-stress-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).unwrap();
    let netlist = out_dir.join("nominal.cir");
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .arg("stress")
        .arg(dir.join("sniffer.kicad_sch"))
        .args(["--runs", "1", "--seed", "5", "--json", "--netlist"])
        .arg(&netlist)
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(1), "{}", String::from_utf8_lossy(&run.stderr));
    let json: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert!(
        json["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["rule"] == "hot-plug")
    );
    assert!(std::fs::read_to_string(&netlist).unwrap().contains(".tran"));
    let text = std::process::Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .arg("stress")
        .arg(dir.join("sniffer.kicad_sch"))
        .args(["--runs", "0", "--all"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(text.contains("pass[hot-plug]") && text.contains("1 runs"), "{text}");
    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .arg("stress")
        .arg(dir.join("sniffer.kicad_sch"))
        .args(["--runs", "x"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&out_dir);
}
