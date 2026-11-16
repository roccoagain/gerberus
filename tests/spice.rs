//! `gerberus stress`: the netlist, the ngspice run and the hot-plug findings on the test boards. These need
//! ngspice installed (CI installs it).

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

/// The nominal netlist: the supply through the cable, every passive, the LDO and the loads, and a measurement
/// for each pin limit.
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
    // Signal inputs are left open, and said to be.
    assert!(
        c.skipped()
            .iter()
            .any(|s| s.contains("UART RX") && s.contains("signal"))
    );
}

/// The sniffer's 10 µF input capacitor rings with the cable: VIN overshoots the ME6211's 6.5 V absolute maximum.
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
        "past its absolute maximum of 6.5 V",
        "ME6211C33M5G-N",
        "run 0 (cable 1 µH)",
    ] {
        assert!(u1.message.contains(want), "missing {want:?}: {}", u1.message);
    }
    // The capacitor itself is rated 25 V, so it passes.
    let c1 = finding(&f, "C1.1 (+5V)");
    assert_eq!(c1.verdict, Verdict::Pass, "{}", c1.message);
    // The rail behind the regulator doesn't ring.
    let vdd = finding(&f, "U2.VDD/VDDA (+3V3)");
    assert_eq!(vdd.verdict, Verdict::Pass, "{}", vdd.message);
    assert_eq!(sweep.runs.len(), 3);
    let text = spice::describe(&d, &sweep);
    assert!(text.contains("3 runs") && text.contains("VIN 5.5 V on +5V"), "{text}");
    assert!(text.contains("aren't reported"), "{text}");
}

/// A switch node isn't modelled, so its pin is UNKNOWN; nets nothing drives aren't reported at all.
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

/// The worst run is named, with what it drew; the input capacitors ring above the settled rail.
#[test]
fn worst_run_is_reported() {
    let (d, sweep, f) = stress("can-encoder", 3);
    assert_eq!(sweep.runs.len(), 4);
    assert!(sweep.runs.iter().all(|r| r.starts_with("cable ")));
    let vdc = d.nets.iter().position(|n| n.name == "+VDC").unwrap();
    let (max, _, _, _) = sweep.peak(vdc, None).unwrap();
    assert!(max > sweep.settled(vdc, None).unwrap() + 1.0, "{max}");
    let c1 = finding(&f, "C1.1 (+VDC)");
    assert_eq!(c1.verdict, Verdict::Pass, "{}", c1.message);
}

#[test]
fn a_board_with_no_supply_input_is_an_error() {
    let (d, mut inputs, lib, v) = board("sniffer");
    inputs.inputs.retain(|i| i.min <= 0.0);
    let e = spice::build(&d, &lib, &inputs, &v).unwrap_err();
    assert!(e.contains("no supply input"), "{e}");
}

/// The command line: a netlist file, JSON findings, and status 1 for an absolute maximum passed.
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
    assert!(text.contains("PASS") && text.contains("1 runs"), "{text}");
    let bad = std::process::Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .arg("stress")
        .arg(dir.join("sniffer.kicad_sch"))
        .args(["--runs", "x"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    let _ = std::fs::remove_dir_all(&out_dir);
}
