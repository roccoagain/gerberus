//! The assembly checks on the build-checks board (from roccoagain/hardware), which has a fail and a pass case for
//! each, and on the sniffer, whose 0603 decoupling capacitors have their GND pads poured solid.
use std::path::Path;

use gerberus::finding::{Finding, Verdict};
use gerberus::policy::Policy;

fn run(board: &str) -> Vec<Finding> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(board);
    let design = gerberus::kicad::load(&dir.join(format!("{board}.kicad_sch"))).unwrap();
    let layout = gerberus::kicad::pcb::load_layout(&dir.join(format!("{board}.kicad_pcb"))).unwrap();
    let policy = Policy::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("policy.toml")).unwrap();
    gerberus::assembly::check(&design, &layout, &policy.assembly.unwrap())
}

fn verdict(r: &[Finding], rule: &str, reference: &str) -> Verdict {
    let f: Vec<_> = r
        .iter()
        .filter(|f| f.rule == rule && f.reference == reference)
        .collect();
    assert_eq!(f.len(), 1, "{rule} {reference}: {f:#?}");
    f[0].verdict
}

#[test]
fn through_hole_pads_need_thermal_reliefs() {
    let r = run("build-checks");
    assert_eq!(verdict(&r, "thermal-relief", "J1"), Verdict::Fail);
    assert_eq!(verdict(&r, "thermal-relief", "J2"), Verdict::Pass);
    // Only pads in a pour are checked: the +5V pins have none.
    assert!(r.iter().all(|f| !f.message.contains(".1 (+5V)")));
}

#[test]
fn small_chips_need_both_pads_joined_alike() {
    let r = run("build-checks");
    assert_eq!(verdict(&r, "tombstoning", "C1"), Verdict::Fail);
    assert_eq!(verdict(&r, "tombstoning", "C2"), Verdict::Pass);
    // The sniffer pours GND solid on every pad: each 0603 capacitor to GND tombstones; its resistors don't touch GND.
    let r = run("sniffer");
    for c in ["C3", "C4", "C5", "C6"] {
        assert_eq!(verdict(&r, "tombstoning", c), Verdict::Fail, "{c}");
    }
}

#[test]
fn a_fine_pitch_side_needs_three_fiducials() {
    let r = run("build-checks");
    let f: Vec<_> = r.iter().filter(|f| f.rule == "fiducials").collect();
    assert_eq!(f.len(), 2, "{f:#?}");
    assert!(
        f.iter()
            .any(|f| f.verdict == Verdict::Fail && f.message.starts_with("the top has 0 fiducials"))
    );
    assert!(
        f.iter()
            .any(|f| f.verdict == Verdict::Pass && f.message.starts_with("the bottom has 3 fiducials"))
    );
    // The sniffer's finest pitch is the STM32's 0.65 mm TSSOP: no fiducials needed.
    assert!(run("sniffer").iter().all(|f| f.rule != "fiducials"));
}
