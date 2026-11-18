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
    missing: &'static [&'static str],
    fragmented: &'static [&'static str],
    overmerged: &'static [&'static str],
    renamed: &'static [(&'static str, &'static str)],
    board_upper_case: bool,
    warnings: usize,
}

fn check(project: &str, board: &str, expect: &Expect) {
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
    let set = |v: &[&str]| -> BTreeSet<String> { v.iter().map(ToString::to_string).collect() };
    assert_eq!(
        missing.iter().map(ToString::to_string).collect::<BTreeSet<_>>(),
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

#[test]
fn limesdr_mini_matches_board() {
    check(
        "limesdr-mini/LimeSDR_Mini_1v2.PrjPcb",
        "limesdr-mini/board-nets.tsv",
        &Expect {
            board_upper_case: true,
            ..Default::default()
        },
    );
}

#[test]
fn mixr_power_matches_board() {
    check(
        "mixr-power/MIXR Power.PrjPcb",
        "mixr-power/board-nets.tsv",
        &Expect {
            renamed: &[
                ("49V2_EN_7", "49V2_EN"),
                ("ESP32_PWR_PE_GP7_DEBUG_LEG_8", "ESP32_PWR_PE_GP7_DEBUG_LEG"),
                ("USB_CC1_3", "USB_CC1_2"),
                ("USB_CC2_3", "USB_CC2_2"),
            ],
            warnings: 5,
            ..Default::default()
        },
    );
}

#[test]
fn misko3_matches_board() {
    check(
        "misko3/MISKO3.PrjPcb",
        "misko3/board-nets.tsv",
        &Expect {
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

#[test]
fn easyinverter_matches_board() {
    check(
        "easyinverter/MainBoard.PrjPCB",
        "easyinverter/board-nets.tsv",
        &Expect::default(),
    );
}

#[test]
fn cube_sat_eps_matches_board() {
    check(
        "cube-sat-eps/pcb/EPS_board.PrjPcb",
        "cube-sat-eps/board-nets.tsv",
        &Expect {
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

#[test]
fn fmc_dio_matches_board() {
    check(
        "fmc-dio-32ch/FMC_DIO_32ch_lvds_a.PrjPcb",
        "fmc-dio-32ch/board-nets.tsv",
        &Expect {
            fragmented: &["P3V3"],
            renamed: &[("VCC", "P3V3")],
            ..Default::default()
        },
    );
}

#[test]
fn malformed_projects_load() {
    let d = altium::load(&fixture("malformed/self-placing/P.PrjPcb")).unwrap();
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
    assert!(d.pin_net("R1", "1").is_some());

    let d = altium::load(&fixture("malformed/loop/P.PrjPcb")).unwrap();
    assert_eq!(d.warnings.len(), 1, "{:?}", d.warnings);
    assert!(d.pin_net("RA", "1").is_some() && d.pin_net("RB", "1").is_some());

    let d = altium::load(&fixture("malformed/channels/P.PrjPcb")).unwrap();
    assert_eq!(d.pin_net("R1_XA", "1").unwrap().name, "SIG");

    altium::load(&fixture("malformed/huge-coordinate.SchDoc")).unwrap();
    altium::load(&fixture("malformed/huge.SchDoc")).unwrap();
}
