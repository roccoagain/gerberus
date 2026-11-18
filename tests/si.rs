use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use gerberus::diag::Show;
use gerberus::kicad::pcb::load_layout;
use gerberus::limits::Library;
use gerberus::rules::{Severity, Verdict};
use gerberus::si::geometry::Section;
use gerberus::si::{self, PairNets, Report};

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

fn report() -> &'static Report {
    static REPORT: OnceLock<Report> = OnceLock::new();
    REPORT.get_or_init(run)
}

fn run() -> Report {
    let layout = load_layout(&fixture("usb-si-testbed/usb-si-testbed.kicad_pcb")).unwrap();
    let lib = Library::load_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")).unwrap();
    si::check(&layout, &lib)
}

fn failures<'a>(r: &'a Report, pair: &str) -> Vec<&'a str> {
    r.findings
        .iter()
        .filter(|f| f.pair == pair && f.verdict == Verdict::Fail)
        .map(|f| f.rule)
        .collect()
}

#[test]
fn the_test_board_has_two_usb_pairs() {
    let layout = load_layout(&fixture("usb-si-testbed/usb-si-testbed.kicad_pcb")).unwrap();
    let pairs = si::find_pairs(&layout);
    let want = |n: &str| PairNets {
        name: n.into(),
        p: format!("{n}_D+"),
        n: format!("{n}_D-"),
    };
    assert_eq!(pairs, vec![want("USB1"), want("USB2")]);
}

#[test]
fn the_good_port_passes_and_the_bad_port_fails_every_rule_it_breaks() {
    let r = report();
    assert_eq!(
        r.pairs.len(),
        2,
        "{:#?}",
        r.pairs.iter().map(|p| &p.name).collect::<Vec<_>>()
    );

    assert!(failures(r, "USB1").is_empty(), "{}", si::render(r, &Show::default()));
    let good = &r.pairs[0];
    assert_eq!(good.name, "USB1");
    let runs: Vec<f64> = good
        .sections
        .iter()
        .filter(|s| s.length > 5.0)
        .map(Section::z_diff)
        .collect();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs.iter().all(|z| (88.0..93.0).contains(z)), "{runs:?}");
    assert!((good.length_p - good.length_n).abs() < 0.1);
    assert_eq!((good.vias_p, good.vias_n), (0, 0));
    let eye = good.eye.as_ref().unwrap();
    assert!(eye.passes(), "{eye:?}");
    assert!(eye.height > 0.35, "{}", eye.height);

    let bad = failures(r, "USB2");
    for rule in [
        "usb-impedance",
        "usb-skew",
        "usb-stubs",
        "usb-reference",
        "usb-capacitance",
        "usb-eye",
    ] {
        assert!(
            bad.contains(&rule),
            "{rule} should fail: {}",
            si::render(r, &Show::default())
        );
    }
    let worst = &r.pairs[1];
    assert!(
        worst.length_n - worst.length_p > 9.0,
        "{} vs {}",
        worst.length_n,
        worst.length_p
    );
    assert_eq!((worst.vias_p, worst.vias_n), (4, 2));
    assert!(!worst.eye.as_ref().unwrap().passes());
    assert!(r.errors() >= 6);
    assert!(
        r.findings
            .iter()
            .any(|f| f.pair == "USB2" && f.severity == Severity::Error)
    );
}

#[test]
fn the_tdr_reads_each_ports_impedance() {
    let r = report();
    let early = |pair: usize| {
        let line = &r.pairs[pair].tdr[0];
        let pts: Vec<f64> = line
            .iter()
            .filter(|(t, _)| (50e-12..150e-12).contains(t))
            .map(|p| p.1)
            .collect();
        pts.iter().sum::<f64>() / pts.len() as f64
    };
    assert!((85.0..95.0).contains(&early(0)), "{}", early(0));
    assert!((120.0..145.0).contains(&early(1)), "{}", early(1));
}

#[test]
fn the_report_and_plots_describe_both_ports() {
    let r = report();
    let text = si::render(r, &Show::everything());
    assert!(text.contains("USB1") && text.contains("USB2"), "{text}");
    for pair in &r.pairs {
        let eye = si::svg::eye(pair).unwrap();
        let tdr = si::svg::tdr(pair).unwrap();
        for svg in [&eye, &tdr] {
            assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"), "{svg}");
            assert!(svg.contains(&pair.name));
        }
        assert!(eye.contains("<polygon"), "the mask is drawn");
    }
}
