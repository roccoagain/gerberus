use gerberus::design::{Design, Net};
use gerberus::finding::Verdict;
use gerberus::kicad::pcb::{Layout, PlacedFootprint, PlacedPad, StackLayer, Track, Zone};
use gerberus::policy::TracePolicy;
use gerberus::thermal::{Load, Thermal};
use gerberus::traces::{check, ipc2221};

#[test]
fn ipc2221_matches_the_fit() {
    let i = ipc2221(0.254, 0.035, true, 10.0);
    assert!((i - 0.88).abs() < 0.02, "{i}");
    assert!((ipc2221(0.254, 0.035, false, 10.0) - i / 2.0).abs() < 1e-9);
}

fn pad(r: &str, at: (f64, f64)) -> PlacedFootprint {
    PlacedFootprint {
        reference: r.into(),
        value: String::new(),
        lib_id: String::new(),
        at,
        pads: vec![PlacedPad {
            number: "1".into(),
            net: Some("VIN".into()),
            at,
            size: (1.0, 1.0),
            rotation: 0.0,
            layers: vec!["F.Cu".into()],
        }],
    }
}

fn board(width: f64) -> Layout {
    let t = |a, b, w| Track {
        net: "VIN".into(),
        layer: "F.Cu".into(),
        start: a,
        end: b,
        width: w,
    };
    Layout {
        stackup: vec![
            StackLayer::Copper {
                name: "F.Cu".into(),
                thickness: 0.035,
            },
            StackLayer::Copper {
                name: "B.Cu".into(),
                thickness: 0.035,
            },
        ],
        tracks: vec![t((0.0, 0.0), (5.0, 0.0), 1.0), t((5.0, 0.0), (10.0, 0.0), width)],
        footprints: vec![pad("J1", (0.0, 0.0)), pad("U1", (10.0, 0.0))],
        ..Default::default()
    }
}

fn run(layout: &Layout, amps: f64, known: bool) -> gerberus::finding::Finding {
    let design = Design {
        nets: vec![Net {
            name: "VIN".into(),
            pins: vec![],
            labels: vec![],
        }],
        ..Default::default()
    };
    let th = Thermal {
        loads: vec![Load {
            amps,
            parts: vec!["U1".into()],
            missing: if known { vec![] } else { vec!["U2".into()] },
        }],
        ..Default::default()
    };
    let policy = TracePolicy {
        rise: 10.0,
        reason: "test".into(),
    };
    check(&design, layout, &th, &policy).remove(0)
}

#[test]
fn wide_enough_passes() {
    let f = run(&board(0.5), 1.0, true);
    assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
}

#[test]
fn a_narrow_track_on_the_only_path_fails() {
    let f = run(&board(0.2), 1.0, true);
    assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
}

#[test]
fn a_pour_or_an_unknown_current_is_unknown() {
    let mut b = board(0.2);
    b.zones.push(Zone {
        net: "VIN".into(),
        layer: "B.Cu".into(),
        polygons: vec![],
        clearance: 0.2,
        min_thickness: 0.2,
    });
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
    assert_eq!(run(&board(0.5), 0.1, false).verdict, Verdict::Unknown);
}

#[test]
fn a_parallel_path_is_unknown() {
    let mut b = board(0.2);
    b.tracks.push(Track {
        net: "VIN".into(),
        layer: "F.Cu".into(),
        start: (5.0, 0.0),
        end: (10.0, 0.0),
        width: 2.0,
    });
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
}

#[test]
fn a_track_ending_off_centre_in_a_pad_connects() {
    let mut b = board(0.2);
    b.footprints[1].pads[0].size = (3.0, 1.0);
    b.tracks.push(Track {
        net: "VIN".into(),
        layer: "F.Cu".into(),
        start: (5.0, 0.0),
        end: (11.2, 0.0),
        width: 2.0,
    });
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
}

#[test]
fn an_assumed_stackup_is_not_enough_to_pass() {
    let mut b = board(0.5);
    b.stackup_assumed = true;
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
    assert_eq!(run(&b, 0.5, true).verdict, Verdict::Pass);
}

#[test]
fn an_unfilled_zone_counts_as_a_pour() {
    let mut b = board(0.2);
    b.zone_nets.push("VIN".into());
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
}
