//! `gerberus traces`: track width against the net's current, by IPC-2221.
use gerberus::design::{Design, Net};
use gerberus::finding::Verdict;
use gerberus::kicad::pcb::{Layout, PlacedFootprint, PlacedPad, StackLayer, Track, Zone};
use gerberus::policy::TracePolicy;
use gerberus::thermal::{Load, Thermal};
use gerberus::traces::{check, ipc2221};

/// 10 mil of 1 oz copper on an outer layer takes about 0.88 A for a 10 °C rise: IPC-2221's fit.
#[test]
fn ipc2221_matches_the_fit() {
    let i = ipc2221(0.254, 0.035, true, 10.0);
    assert!((i - 0.88).abs() < 0.02, "{i}");
    // Inner layers take half.
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

/// Two pads joined by a 0.2 mm track and a 1 mm one, in series.
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

/// The 0.2 mm track is the only path between the two pads, so it carries all 1 A, more than its ~0.74 A.
#[test]
fn a_narrow_track_on_the_only_path_fails() {
    let f = run(&board(0.2), 1.0, true);
    assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
}

/// A pour on the net may carry the current alongside the track, so a narrow track is only UNKNOWN; so is a net
/// whose current isn't all known, even when its known part fits.
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

/// With a second, wide track in parallel, the narrow one may carry only part of it.
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

/// A wide track ending inside a long pad, off its centre, still reaches the pad: with it in parallel the narrow
/// track may carry only part of the current.
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

/// With no stackup in the board file, the copper may be ½ oz: a track that passes only at 1 oz is UNKNOWN.
#[test]
fn an_assumed_stackup_is_not_enough_to_pass() {
    let mut b = board(0.5);
    b.stackup_assumed = true;
    // 0.5 mm of 1 oz takes about 1.4 A; ½ oz about 0.85 A.
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
    assert_eq!(run(&b, 0.5, true).verdict, Verdict::Pass);
}

/// An unfilled zone on the net still counts as a pour.
#[test]
fn an_unfilled_zone_counts_as_a_pour() {
    let mut b = board(0.2);
    b.zone_nets.push("VIN".into());
    assert_eq!(run(&b, 1.0, true).verdict, Verdict::Unknown);
}

mod voltage_drop {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use gerberus::drop::Network;
    use gerberus::finding::Verdict;
    use gerberus::kicad::pcb::{Layout, PlacedPad, StackLayer, Track, Via};

    fn pad(at: (f64, f64), layer: &str) -> PlacedPad {
        PlacedPad {
            number: "1".into(),
            net: Some("VIN".into()),
            at,
            size: (0.5, 0.5),
            rotation: 0.0,
            layers: vec![layer.into()],
        }
    }

    fn track(a: (f64, f64), b: (f64, f64), w: f64, layer: &str) -> Track {
        Track {
            net: "VIN".into(),
            layer: layer.into(),
            start: a,
            end: b,
            width: w,
        }
    }

    fn layout(tracks: Vec<Track>, vias: Vec<Via>) -> Layout {
        Layout {
            stackup: vec![
                StackLayer::Copper {
                    name: "F.Cu".into(),
                    thickness: 0.035,
                },
                StackLayer::Dielectric {
                    thickness: 1.5,
                    er: 4.5,
                    loss_tangent: 0.02,
                },
                StackLayer::Copper {
                    name: "B.Cu".into(),
                    thickness: 0.035,
                },
            ],
            tracks,
            vias,
            ..Default::default()
        }
    }

    /// Resistance from the first pad to the second, at 1 Ω·mm.
    fn z(l: &Layout, pads: &[PlacedPad]) -> Option<f64> {
        let thick: BTreeMap<String, f64> = [("F.Cu".to_string(), 0.035), ("B.Cu".to_string(), 0.035)].into();
        let refs: Vec<&PlacedPad> = pads.iter().collect();
        let n = Network::build(l, "VIN", &refs, &thick, true).unwrap();
        let from: BTreeSet<usize> = [n.pads[0]?].into();
        n.resistance(&from, &[[n.pads[1]?].into()])[0]
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * b.abs()
    }

    /// A 10 mm track 0.2 mm wide of 35 µm copper: 10 / (0.2 × 0.035) per Ω·mm.
    #[test]
    fn one_track() {
        let l = layout(vec![track((0.0, 0.0), (10.0, 0.0), 0.2, "F.Cu")], vec![]);
        let pads = [pad((0.0, 0.0), "F.Cu"), pad((10.0, 0.0), "F.Cu")];
        assert!(close(z(&l, &pads).unwrap(), 10.0 / (0.2 * 0.035)));
    }

    /// Two tracks side by side halve it; a track in two pieces doesn't change it.
    #[test]
    fn parallel_and_series() {
        let mut l = layout(
            vec![
                track((0.0, 0.0), (4.0, 0.0), 0.2, "F.Cu"),
                track((4.0, 0.0), (10.0, 0.0), 0.2, "F.Cu"),
            ],
            vec![],
        );
        let pads = [pad((0.0, 0.0), "F.Cu"), pad((10.0, 0.0), "F.Cu")];
        let one = 10.0 / (0.2 * 0.035);
        assert!(close(z(&l, &pads).unwrap(), one));
        l.tracks.push(track((0.0, 0.0), (0.0, 5.0), 0.2, "F.Cu"));
        l.tracks.push(track((0.0, 5.0), (10.0, 5.0), 0.2, "F.Cu"));
        l.tracks.push(track((10.0, 5.0), (10.0, 0.0), 0.2, "F.Cu"));
        // The detour is 20 mm, twice the straight path: 1 ∥ 2 = 2/3.
        assert!(close(z(&l, &pads).unwrap(), one * 2.0 / 3.0));
    }

    /// A branch leaving a track midway uses only the part of the track up to it.
    #[test]
    fn a_branch_midway_splits_the_track() {
        let l = layout(
            vec![
                track((0.0, 0.0), (10.0, 0.0), 0.2, "F.Cu"),
                track((5.0, 0.0), (5.0, 5.0), 0.2, "F.Cu"),
            ],
            vec![],
        );
        let pads = [pad((0.0, 0.0), "F.Cu"), pad((5.0, 5.0), "F.Cu")];
        assert!(close(z(&l, &pads).unwrap(), 10.0 / (0.2 * 0.035)));
    }

    /// Two tracks crossing in an X are joined where they cross; a track running through a pad joins it there.
    #[test]
    fn crossings_and_pads_along_a_track_join() {
        let l = layout(
            vec![
                track((0.0, 0.0), (10.0, 0.0), 0.2, "F.Cu"),
                track((5.0, -5.0), (5.0, 5.0), 0.2, "F.Cu"),
            ],
            vec![],
        );
        let pads = [pad((0.0, 0.0), "F.Cu"), pad((5.0, 5.0), "F.Cu")];
        assert!(close(z(&l, &pads).unwrap(), 10.0 / (0.2 * 0.035)));
        let pads = [pad((0.0, 0.0), "F.Cu"), pad((7.0, 0.0), "F.Cu")];
        assert!(close(z(&l, &pads).unwrap(), 7.0 / (0.2 * 0.035)));
    }

    /// A via adds its barrel; with no copper between them there is no resistance.
    #[test]
    fn a_via_adds_its_barrel_and_a_gap_is_none() {
        let via = Via {
            net: "VIN".into(),
            at: (10.0, 0.0),
            diameter: 0.6,
            drill: 0.3,
            layers: ("F.Cu".into(), "B.Cu".into()),
        };
        let l = layout(
            vec![
                track((0.0, 0.0), (10.0, 0.0), 0.2, "F.Cu"),
                track((10.0, 0.0), (20.0, 0.0), 0.2, "B.Cu"),
            ],
            vec![via],
        );
        let pads = [pad((0.0, 0.0), "F.Cu"), pad((20.0, 0.0), "B.Cu")];
        // Between the copper layers' centres, through the plating's ring inside the 0.3 mm hole.
        let barrel = 1.535 / (std::f64::consts::PI * 0.28 * 0.02);
        assert!(close(z(&l, &pads).unwrap(), 20.0 / (0.2 * 0.035) + barrel));
        let gap = layout(vec![track((0.0, 0.0), (4.0, 0.0), 0.2, "F.Cu")], vec![]);
        assert_eq!(z(&gap, &pads), None);
    }

    /// On can-encoder, U5's 3.3 V output reaches the MCU's supply pins through short tracks: a few mV. Made
    /// a thousand times narrower, the drop can't be passed, and fails where a pin's own current is known.
    #[test]
    fn on_a_test_board() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/can-encoder");
        let design = gerberus::kicad::load(&dir.join("can-encoder.kicad_sch")).unwrap();
        let lib = gerberus::limits::Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
        let inputs = gerberus::inputs::BoardInputs::load(&dir.join("inputs.toml")).unwrap();
        let v = gerberus::voltage::compute(&design, &lib, &inputs);
        let th = gerberus::thermal::compute(&design, &lib, &inputs, &v);
        let mut layout = gerberus::kicad::pcb::load_layout(&dir.join("can-encoder.kicad_pcb")).unwrap();
        let policy = gerberus::policy::TracePolicy {
            rise: 10.0,
            reason: "test".into(),
        };
        let pin = |layout: &Layout, at: &str| {
            gerberus::drop::check(&design, &lib, &inputs, &v, layout, &th, &policy)
                .into_iter()
                .find(|f| f.message.starts_with(at))
                .unwrap()
        };
        let f = pin(&layout, "U2.17 ");
        assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
        assert!(f.message.contains("from U5"), "{}", f.message);
        // The buck's output comes through its inductor, not its feedback pin.
        let f = pin(&layout, "U5.3 ");
        assert!(f.message.contains("from L1"), "{}", f.message);
        for t in layout.tracks.iter_mut().filter(|t| t.net == "+3V3") {
            t.width /= 1000.0;
        }
        // U2's supply current is given for several pins together, so its own share isn't known.
        let f = pin(&layout, "U2.17 ");
        assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
        // U3 draws up to 28 mA through its one VDD pin, with only 234 mV to spare.
        let f = pin(&layout, "U3.9 ");
        assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
    }
}
