//! Checks the Altium reader against real, openly licensed projects.
//!
//! Each fixture's `board-nets.tsv` lists the net of every pad in the project's `.PcbDoc`, which Altium compiled
//! from the same schematics (`cargo run --example pcbdoc_nets -- board.PcbDoc` writes it). The board is an
//! independent reference for which pins share a net and what the net is called. Each case lists exactly where
//! the reader and the board disagree, and why, so a new disagreement fails by name. Sources and licenses are in
//! `tests/fixtures/altium/README.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gerberus::altium;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/altium")
        .join(rel)
}

#[derive(Default)]
struct Expect {
    /// Board pads on a net that the schematics have no pin for.
    missing: &'static [&'static str],
    /// Board nets whose pins the reader splits across several nets.
    fragmented: &'static [&'static str],
    /// Reader nets whose pins the board splits.
    overmerged: &'static [&'static str],
    /// `(reader name, board name)` for every net the two name differently.
    renamed: &'static [(&'static str, &'static str)],
    /// The board stores net names upper-cased, as older Altium versions did.
    board_upper_case: bool,
    warnings: usize,
}

fn check(project: &str, board: &str, expect: Expect) {
    let design = altium::load(&fixture(project)).unwrap();
    assert_eq!(
        design.warnings.len(),
        expect.warnings,
        "warnings: {:#?}",
        design.warnings
    );

    let mut pads: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let text = std::fs::read_to_string(fixture(board)).unwrap();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (pad, net) = line.split_once('\t').unwrap();
        pads.entry(pad.to_string()).or_default().insert(net.to_string());
    }

    let mut missing = BTreeSet::new();
    let mut board_to_reader: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut reader_to_board: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut renamed = BTreeSet::new();
    for (pad, nets) in &pads {
        let (r, p) = pad.rsplit_once('.').unwrap();
        let Some(net) = design.pin_net(r, p) else {
            missing.insert(pad.as_str());
            continue;
        };
        // A footprint can repeat a pad name on several nets; the board can't place such a pin on one.
        assert_eq!(nets.len(), 1, "{pad} is on several board nets: {nets:?}");
        let want = nets.iter().next().unwrap().as_str();
        board_to_reader.entry(want).or_default().insert(&net.name);
        reader_to_board.entry(&net.name).or_default().insert(want);
        let same = if expect.board_upper_case {
            net.name.eq_ignore_ascii_case(want)
        } else {
            net.name == want
        };
        if !same {
            renamed.insert((net.name.as_str(), want));
        }
    }
    let split = |m: &BTreeMap<&str, BTreeSet<&str>>| -> BTreeSet<String> {
        m.iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(k, _)| k.to_string())
            .collect()
    };
    let set = |v: &[&str]| -> BTreeSet<String> { v.iter().map(|s| s.to_string()).collect() };
    assert_eq!(
        missing.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        set(expect.missing),
        "missing"
    );
    assert_eq!(split(&board_to_reader), set(expect.fragmented), "fragmented");
    assert_eq!(split(&reader_to_board), set(expect.overmerged), "overmerged");
    assert_eq!(
        renamed,
        expect.renamed.iter().copied().collect::<BTreeSet<_>>(),
        "renamed"
    );
}

/// Global scope (`HierarchyMode=3`) across nine flat sheets, imported with fractional coordinates.
#[test]
fn limesdr_mini_matches_board() {
    check(
        "limesdr-mini/LimeSDR_Mini_1v2.PrjPcb",
        "limesdr-mini/board-nets.tsv",
        Expect {
            board_upper_case: true,
            ..Default::default()
        },
    );
}

/// Automatic scope resolving to hierarchical, with `AppendSheetNumberToLocalNets`. The board is stale with
/// respect to the schematics: it numbers `USB_CC1`/`USB_CC2` with an old sheet number and leaves the other
/// sheet-local labels bare, so only names differ.
#[test]
fn mixr_power_matches_board() {
    check(
        "mixr-power/MIXR Power.PrjPcb",
        "mixr-power/board-nets.tsv",
        Expect {
            renamed: &[
                ("49V2_EN_7", "49V2_EN"),
                ("ESP32_PWR_PE_GP7_DEBUG_LEG_8", "ESP32_PWR_PE_GP7_DEBUG_LEG"),
                ("USB_CC1_3", "USB_CC1_2"),
                ("USB_CC2_3", "USB_CC2_2"),
            ],
            // Five test points were never annotated (`TH_TP?`).
            warnings: 5,
            ..Default::default()
        },
    );
}

/// Hierarchical scope with signal harnesses carrying bus members, harness ports that name nets
/// (`AllowPortNetNames`), power port priority and sheet-numbered local nets.
#[test]
fn misko3_matches_board() {
    check(
        "misko3/MISKO3.PrjPcb",
        "misko3/board-nets.tsv",
        Expect {
            // Buttons whose footprints have four pads for two pins, and mounting holes M1 and M2, which the board
            // ties to nets without a schematic pin.
            missing: &[
                "A1.2",
                "A1.4",
                "BTN_DOWN1.2",
                "BTN_DOWN1.4",
                "BTN_ESC1.2",
                "BTN_ESC1.4",
                "BTN_LEFT1.2",
                "BTN_LEFT1.4",
                "BTN_OK1.2",
                "BTN_OK1.4",
                "BTN_RIGHT1.2",
                "BTN_RIGHT1.4",
                "BTN_UP1.2",
                "BTN_UP1.4",
                "M1.1",
                "M1.2",
                "M1.3",
                "M2.1",
                "M2.2",
                "M2.3",
            ],
            ..Default::default()
        },
    );
}

/// Two plain sheet symbols place one sheet, which places a sheet of its own; harness connectors nest in other
/// connectors' entries. Every pin and every name agrees.
#[test]
fn easyinverter_matches_board() {
    check(
        "easyinverter/MainBoard.PrjPCB",
        "easyinverter/board-nets.tsv",
        Expect::default(),
    );
}

/// Two channels of one sheet by duplicate symbols (`$Component$ChannelAlpha`), `Repeat()` entries and harnesses.
#[test]
fn cube_sat_eps_matches_board() {
    check(
        "cube-sat-eps/pcb/EPS_board.PrjPcb",
        "cube-sat-eps/board-nets.tsv",
        Expect {
            // The Q400 footprints give three schematic pins nine pads, and the board puts pins 2 and 3 on VBUS
            // as well as pads 4 to 9.
            missing: &[
                "Q400A.4", "Q400A.5", "Q400A.6", "Q400A.7", "Q400A.8", "Q400A.9", "Q400B.4", "Q400B.5", "Q400B.6",
                "Q400B.7", "Q400B.8", "Q400B.9",
            ],
            fragmented: &["VBUS"],
            overmerged: &["G_OR_A", "G_OR_B", "VBAT1", "VBAT2"],
            renamed: &[
                ("G_OR_A", "VBUS"),
                ("G_OR_B", "VBUS"),
                ("VBAT1", "VBUS"),
                ("VBAT2", "VBUS"),
            ],
            ..Default::default()
        },
    );
}

/// One buffer sheet placed 32 times by `Repeat()`, fed from buses; channel designators run past `Z`.
#[test]
fn fmc_dio_matches_board() {
    check(
        "fmc-dio-32ch/FMC_DIO_32ch_lvds_a.PrjPcb",
        "fmc-dio-32ch/board-nets.tsv",
        Expect {
            // IC51's hidden pin 5 is named VCC and has no hidden net name; the reader joins it to a net of its
            // name, the board puts it on P3V3.
            fragmented: &["P3V3"],
            renamed: &[("VCC", "P3V3")],
            ..Default::default()
        },
    );
}

/// Hand-written sheets with broken or extreme input: the reader warns or carries on rather than panicking,
/// looping or running out of memory.
#[test]
fn malformed_projects_load() {
    // A sheet that places itself is placed once, with a warning.
    let d = altium::load(&fixture("malformed/self-placing/P.PrjPcb")).unwrap();
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
    assert!(d.pin_net("R1", "1").is_some());

    // Two sheets that only place each other still load, each once.
    let d = altium::load(&fixture("malformed/loop/P.PrjPcb")).unwrap();
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
    assert!(d.pin_net("RA", "1").is_some() && d.pin_net("RB", "1").is_some());

    // A wire from a channel of one sheet to a channel of another reaches two channels, so it keeps its plain
    // name whichever channels they are.
    let d = altium::load(&fixture("malformed/channels/P.PrjPcb")).unwrap();
    assert_eq!(d.pin_net("R1_XA", "1").unwrap().name, "SIG");

    // Coordinates, wires and `Repeat()` counts far beyond any real sheet.
    altium::load(&fixture("malformed/huge-coordinate.SchDoc")).unwrap();
    altium::load(&fixture("malformed/huge.SchDoc")).unwrap();
}
