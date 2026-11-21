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
        back: false,
        pads: vec![PlacedPad {
            number: "1".into(),
            net: Some("VIN".into()),
            at,
            size: (1.0, 1.0),
            rotation: 0.0,
            layers: vec!["F.Cu".into()],
            kind: "smd".into(),
            round: false,
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
        outlines: vec![],
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
            kind: "smd".into(),
            round: false,
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

mod decoupling {
    use std::path::Path;

    use gerberus::finding::{Finding, Verdict};
    use gerberus::policy::DecouplingPolicy;

    fn run(distance: f64, largest: f64) -> Vec<Finding> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/can-encoder");
        let design = gerberus::kicad::load(&dir.join("can-encoder.kicad_sch")).unwrap();
        let lib = gerberus::limits::Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
        let inputs = gerberus::inputs::BoardInputs::load(&dir.join("inputs.toml")).unwrap();
        let v = gerberus::voltage::compute(&design, &lib, &inputs);
        let layout = gerberus::kicad::pcb::load_layout(&dir.join("can-encoder.kicad_pcb")).unwrap();
        let policy = DecouplingPolicy {
            distance,
            largest,
            reason: "test".into(),
        };
        gerberus::decoupling::check(&design, &lib, &v, &layout, &policy)
    }

    fn at<'a>(r: &'a [Finding], pin: &str) -> &'a Finding {
        r.iter().find(|f| f.message.starts_with(pin)).unwrap()
    }

    /// The MCU's 3.3 V pin has C7 (100 nF) 1.3 mm away.
    #[test]
    fn near_passes_and_far_fails() {
        let f = at(&run(5.0, 1e-6), "U2.17 ").clone();
        assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
        assert!(f.message.contains("C7"), "{}", f.message);
        let f = at(&run(1.0, 1e-6), "U2.17 ").clone();
        assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
    }

    /// The LDO's input has C4, a 22 µF capacitor, nearer than any 100 nF: whether it decouples depends on whether
    /// it's a ceramic, which isn't known.
    #[test]
    fn a_nearer_large_capacitor_is_unknown() {
        let r = run(5.0, 1e-6);
        let f = at(&r, "U5.3 ");
        assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
        assert!(f.message.contains("C4"), "{}", f.message);
    }

    /// A capacitor bigger than `largest` isn't taken as decoupling.
    #[test]
    fn bulk_capacitors_do_not_pass() {
        let r = run(5.0, 10e-9);
        let f = at(&r, "U2.17 ");
        assert_eq!(f.verdict, Verdict::Unknown, "{}", f.message);
    }
}

mod plane_split {
    use gerberus::design::{Component, Design, Net, Pin, PinKind};
    use gerberus::finding::Verdict;
    use gerberus::kicad::pcb::{Layout, Point, StackLayer, Track, Zone};

    /// A GND zone on B.Cu: its fills, and its outline.
    fn zone(polygons: Vec<Vec<Point>>, outline: Vec<Point>) -> Zone {
        Zone {
            net: "GND".into(),
            layer: "B.Cu".into(),
            polygons,
            outlines: vec![outline],
            clearance: 0.2,
            min_thickness: 0.2,
        }
    }

    fn square(x0: f64, x1: f64) -> Vec<Point> {
        vec![(x0, 0.0), (x1, 0.0), (x1, 10.0), (x0, 10.0)]
    }

    /// One filled square zone.
    fn plane(x0: f64, x1: f64) -> Zone {
        zone(vec![square(x0, x1)], square(x0, x1))
    }

    /// A track on F.Cu along y = 5.5 from x = 1 to x = 9, driven by U1's output, over whatever B.Cu has.
    fn verdict(zones: Vec<Zone>) -> Verdict {
        let layout = Layout {
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
            tracks: vec![Track {
                net: "SIG".into(),
                layer: "F.Cu".into(),
                start: (1.0, 5.5),
                end: (9.0, 5.5),
                width: 0.2,
            }],
            zones,
            ..Default::default()
        };
        let mut u1 = Component::default();
        u1.pins.insert(
            "1".into(),
            Pin {
                number: "1".into(),
                name: "OUT".into(),
                kind: PinKind::Output,
                no_connect: false,
                net: 0,
                stack: None,
            },
        );
        let design = Design {
            nets: vec![Net {
                name: "SIG".into(),
                pins: vec![("U1".into(), "1".into())],
                labels: vec![],
            }],
            components: [("U1".to_string(), u1)].into_iter().collect(),
            ..Default::default()
        };
        gerberus::splits::check(&design, &layout)[0].verdict
    }

    #[test]
    fn one_plane_passes() {
        assert_eq!(verdict(vec![plane(0.0, 10.0)]), Verdict::Pass);
    }

    /// A hole in the plane (a via's clearance, joined to the outline by KiCad's zero-width slit) doesn't split it,
    /// even where the track runs inside it.
    #[test]
    fn a_hole_is_not_a_split() {
        let holed = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 5.0),
            (4.0, 5.0),
            (4.0, 6.0),
            (6.0, 6.0),
            (6.0, 4.0),
            (4.0, 4.0),
            (4.0, 5.0),
            (0.0, 5.0),
        ];
        assert_eq!(verdict(vec![zone(vec![holed], square(0.0, 10.0))]), Verdict::Pass);
    }

    /// Two pieces with a gap between them under the track: its return current has to go around.
    #[test]
    fn a_gap_between_two_pieces_fails() {
        assert_eq!(verdict(vec![plane(0.0, 4.0), plane(6.0, 10.0)]), Verdict::Fail);
    }

    /// Two fills of one net that overlap are one piece of copper.
    #[test]
    fn overlapping_fills_are_one_plane() {
        assert_eq!(verdict(vec![plane(0.0, 6.0), plane(4.0, 10.0)]), Verdict::Pass);
    }

    /// The track runs off the plane's edge.
    #[test]
    fn running_off_the_plane_is_unknown() {
        assert_eq!(verdict(vec![plane(0.0, 5.0)]), Verdict::Unknown);
        assert_eq!(verdict(vec![]), Verdict::Unknown);
    }

    /// A net with no output on it, such as a strap tied by resistors, has no edges and isn't checked.
    #[test]
    fn undriven_nets_are_left_out() {
        let layout = Layout {
            tracks: vec![Track {
                net: "STRAP".into(),
                layer: "F.Cu".into(),
                start: (1.0, 5.5),
                end: (9.0, 5.5),
                width: 0.2,
            }],
            ..Default::default()
        };
        let design = Design {
            nets: vec![Net {
                name: "STRAP".into(),
                pins: vec![],
                labels: vec![],
            }],
            ..Default::default()
        };
        assert!(gerberus::splits::check(&design, &layout).is_empty());
    }
}

mod hot_loop {
    use std::path::Path;

    use gerberus::finding::Verdict;
    use gerberus::policy::HotLoopPolicy;

    /// can-encoder's buck U1 has C2 beside its VIN and GND pins. A tighter limit fails it.
    #[test]
    fn the_buck_input_loop() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/can-encoder");
        let design = gerberus::kicad::load(&dir.join("can-encoder.kicad_sch")).unwrap();
        let lib = gerberus::limits::Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
        let layout = gerberus::kicad::pcb::load_layout(&dir.join("can-encoder.kicad_pcb")).unwrap();
        let run = |length: f64| {
            let policy = HotLoopPolicy {
                length,
                reason: "test".into(),
            };
            gerberus::loops::check(&design, &lib, &layout, &policy).remove(0)
        };
        let f = run(15.0);
        assert_eq!(f.verdict, Verdict::Pass, "{}", f.message);
        assert!(
            f.message.contains("C2") && f.message.contains("9.5 mm"),
            "{}",
            f.message
        );
        assert_eq!(run(1.0).verdict, Verdict::Fail);
    }

    /// Moved 20 mm away in line with the pins, the input capacitors C1 and C2 make a long loop that encloses next to
    /// no area: the loop's length still fails it.
    #[test]
    fn a_far_capacitor_in_line_fails() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/can-encoder");
        let design = gerberus::kicad::load(&dir.join("can-encoder.kicad_sch")).unwrap();
        let lib = gerberus::limits::Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
        let mut layout = gerberus::kicad::pcb::load_layout(&dir.join("can-encoder.kicad_pcb")).unwrap();
        let u1 = layout.footprints.iter().find(|f| f.reference == "U1").unwrap().at;
        for c in layout
            .footprints
            .iter_mut()
            .filter(|f| f.reference == "C1" || f.reference == "C2")
        {
            for (i, p) in c.pads.iter_mut().enumerate() {
                p.at = (u1.0 + 20.0 + i as f64, u1.1);
            }
        }
        let policy = HotLoopPolicy {
            length: 15.0,
            reason: "test".into(),
        };
        let f = gerberus::loops::check(&design, &lib, &layout, &policy).remove(0);
        assert_eq!(f.verdict, Verdict::Fail, "{}", f.message);
    }
}

mod line_length {
    use gerberus::design::{Design, Net};
    use gerberus::finding::Verdict;
    use gerberus::kicad::pcb::{Layout, StackLayer, Track};
    use gerberus::policy::EdgePolicy;

    /// A 1 ns edge in εr 4 travels 150 mm/ns, so it is lumped up to 25 mm: SPI_SCK with 20 mm passes, with 40 mm
    /// it may ring; a net that isn't SPI or I²S isn't checked.
    #[test]
    fn spi_and_i2s_lines_against_the_edge() {
        let net = |name: &str| Net {
            name: name.into(),
            pins: vec![],
            labels: vec![],
        };
        let design = Design {
            nets: vec![net("/SPI_SCK"), net("/I2S_BCLK"), net("/LED")],
            ..Default::default()
        };
        let track = |net: &str, len: f64| Track {
            net: net.into(),
            layer: "F.Cu".into(),
            start: (0.0, 0.0),
            end: (len, 0.0),
            width: 0.2,
        };
        let layout = Layout {
            stackup: vec![StackLayer::Dielectric {
                thickness: 1.5,
                er: 4.0,
                loss_tangent: 0.02,
            }],
            tracks: vec![track("/SPI_SCK", 20.0), track("/I2S_BCLK", 40.0), track("/LED", 100.0)],
            ..Default::default()
        };
        let policy = EdgePolicy {
            rise: 1e-9,
            reason: "test".into(),
        };
        let r = gerberus::edges::check(&design, &layout, &policy);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].verdict, Verdict::Pass, "{}", r[0].message);
        assert_eq!(r[1].verdict, Verdict::Unknown, "{}", r[1].message);
    }
}
