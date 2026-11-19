//! Small layouts built in code, one per way a real board can trip up the USB checks.
use std::path::Path;

use gerberus::diag::Show;
use gerberus::finding::{Severity, Verdict};
use gerberus::kicad::pcb::{Layout, PlacedFootprint, PlacedPad, Point, StackLayer, Track, Zone};
use gerberus::limits::Library;
use gerberus::si::{self, Report};

const YP: f64 = 0.0;
const YN: f64 = 0.37;

/// Two layers with a GND plane under everything.
fn base() -> Layout {
    Layout {
        stackup: vec![
            StackLayer::Mask {
                thickness: 0.01,
                er: 3.8,
            },
            StackLayer::Copper {
                name: "F.Cu".into(),
                thickness: 0.035,
            },
            StackLayer::Dielectric {
                thickness: 0.2,
                er: 4.3,
                loss_tangent: 0.02,
            },
            StackLayer::Copper {
                name: "B.Cu".into(),
                thickness: 0.035,
            },
        ],
        zones: vec![zone((-10.0, -10.0), (60.0, 10.0))],
        ..Layout::default()
    }
}

fn zone(a: Point, b: Point) -> Zone {
    Zone {
        net: "GND".into(),
        layer: "B.Cu".into(),
        polygons: vec![vec![a, (b.0, a.1), b, (a.0, b.1)]],
        outlines: vec![],
        clearance: 0.2,
        min_thickness: 0.2,
    }
}

fn pad(num: &str, net: &str, at: Point, size: f64) -> PlacedPad {
    PlacedPad {
        number: num.into(),
        net: Some(net.into()),
        at,
        size: (size, size),
        rotation: 0.0,
        layers: vec!["F.Cu".into()],
    }
}

fn fp(reference: &str, value: &str, pads: Vec<PlacedPad>) -> PlacedFootprint {
    PlacedFootprint {
        reference: reference.into(),
        value: value.into(),
        lib_id: "lib:fp".into(),
        at: (0.0, 0.0),
        pads,
    }
}

fn track(net: &str, a: Point, b: Point) -> Track {
    Track {
        net: net.into(),
        layer: "F.Cu".into(),
        start: a,
        end: b,
        width: 0.22,
    }
}

/// A 40 mm pair from U1 (a PHY) to J1 (a USB connector), with `d_plus` drawn instead of the straight D+.
fn board(d_plus: Vec<Track>) -> Layout {
    let mut l = base();
    if d_plus.is_empty() {
        l.tracks.push(track("USB_D+", (0.0, YP), (40.0, YP)));
    }
    l.tracks.extend(d_plus);
    l.tracks.push(track("USB_D-", (0.0, YN), (40.0, YN)));
    l.footprints.push(fp(
        "U1",
        "PHY",
        vec![pad("1", "USB_D+", (0.0, YP), 0.3), pad("2", "USB_D-", (0.0, YN), 0.3)],
    ));
    l.footprints.push(fp(
        "J1",
        "USB",
        vec![
            pad("A6", "USB_D+", (40.0, YP), 0.3),
            pad("A7", "USB_D-", (40.0, YN), 0.3),
        ],
    ));
    l
}

fn check(l: &Layout) -> Report {
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    si::check(l, &lib)
}

fn verdict(r: &Report, rule: &str, needle: &str) -> Verdict {
    r.findings
        .iter()
        .find(|f| f.rule == rule && f.message.contains(needle))
        .unwrap_or_else(|| {
            panic!(
                "no {rule} finding mentioning {needle}:\n{}",
                si::render(r, &Show::everything())
            )
        })
        .verdict
}

#[test]
fn a_clean_pair_passes() {
    let r = check(&board(vec![]));
    assert_eq!(r.errors(), 0, "{}", si::render(&r, &Show::everything()));
    assert_eq!(verdict(&r, "usb-capacitance", "USB_D+"), Verdict::Pass);
}

#[test]
fn an_unrouted_second_connector_pad_is_reported_not_a_panic() {
    // USB-C puts D+ on A6 and B6; only A6 is routed.
    let mut l = board(vec![]);
    l.footprints[1].pads.push(pad("B6", "USB_D+", (45.0, YP), 0.3));
    let r = check(&l);
    assert_eq!(r.pairs.len(), 1);
    assert_eq!(r.pairs[0].to, "J1.A6");
    assert_eq!(verdict(&r, "usb-length", "J1.B6"), Verdict::Fail);
}

#[test]
fn a_part_the_line_runs_through_loads_it() {
    // D+ is broken at x = 20 and both ends land inside pad 4 of a 155 pF TVS.
    let mut l = board(vec![
        track("USB_D+", (0.0, YP), (19.7, YP)),
        track("USB_D+", (20.3, YP), (40.0, YP)),
    ]);
    let mut tvs = pad("4", "USB_D+", (20.0, YP), 0.3);
    tvs.size = (1.0, 0.3);
    l.footprints.push(fp("D1", "TVS0500DRVR", vec![tvs]));
    let r = check(&l);
    assert_eq!(verdict(&r, "usb-capacitance", "USB_D+"), Verdict::Fail);
    assert_eq!(verdict(&r, "usb-eye", ""), Verdict::Fail);
}

#[test]
fn a_pad_missing_from_the_part_file_is_unknown() {
    let mut l = board(vec![]);
    l.tracks.push(track("USB_D+", (20.0, YP), (20.0, YP - 0.5)));
    l.footprints
        .push(fp("D1", "TVS0500DRVR", vec![pad("8", "USB_D+", (20.0, YP - 0.5), 0.3)]));
    let r = check(&l);
    assert_eq!(verdict(&r, "usb-capacitance", "USB_D+"), Verdict::Unknown);
}

#[test]
fn overlapping_zones_of_one_net_are_solid_copper() {
    let mut l = board(vec![]);
    l.zones.push(zone((15.0, -5.0), (25.0, 5.0)));
    let r = check(&l);
    assert_eq!(r.errors(), 0, "{}", si::render(&r, &Show::everything()));
    assert!(
        r.findings
            .iter()
            .filter(|f| f.rule == "usb-reference")
            .all(|f| f.verdict == Verdict::Pass)
    );
}

#[test]
fn a_pair_with_no_connector_is_shown_as_untraced() {
    let mut l = board(vec![]);
    l.footprints[1].reference = "U2".into();
    let r = check(&l);
    let unknown = Show {
        all: true,
        ..Show::default()
    };
    let text = si::render(&r, &unknown);
    assert!(
        text.contains("USB: not traced") && text.contains("no connector pad"),
        "{text}"
    );
}

#[test]
fn a_header_tapped_near_the_phy_is_a_stub_not_the_far_end() {
    let mut l = board(vec![]);
    l.tracks.push(track("USB_D+", (2.0, YP), (2.0, YP - 3.0)));
    l.tracks.push(track("USB_D-", (2.0, YN), (2.0, YN + 3.0)));
    l.footprints.push(fp(
        "J2",
        "HDR",
        vec![
            pad("1", "USB_D+", (2.0, YP - 3.0), 1.0),
            pad("2", "USB_D-", (2.0, YN + 3.0), 1.0),
        ],
    ));
    let r = check(&l);
    assert_eq!(r.pairs[0].to, "J1.A6", "{}", si::render(&r, &Show::everything()));
    // The 3 mm tap is reported as a stub (a warning: under TUSB1210's 200 mil, but TI's guide allows none).
    let stub = r
        .findings
        .iter()
        .find(|f| f.rule == "usb-stubs" && f.message.contains("3.00 mm stub"))
        .unwrap();
    assert_eq!((stub.verdict, stub.severity), (Verdict::Fail, Severity::Warning));
}

#[test]
fn superspeed_lanes_are_skipped() {
    let mut l = base();
    for n in ["USB_SSTX_P", "USB_SSTX_N"] {
        l.tracks.push(track(n, (0.0, 0.0), (1.0, 0.0)));
    }
    let r = check(&l);
    assert!(r.pairs.is_empty() && r.findings.is_empty());
    assert_eq!(r.skipped.len(), 1, "{:?}", r.skipped);
}
