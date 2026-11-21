//! `check --html`'s page, on drawings made up for the test (kicad-cli draws the real ones).
use std::path::{Path, PathBuf};

use gerberus::design::Placed;
use gerberus::finding::{Finding, Severity, Verdict};
use gerberus::html::{Drawing, render};
use gerberus::kicad;
use gerberus::rules::Report;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn finding(reference: &str, verdict: Verdict, message: &str) -> Finding {
    Finding {
        rule: "test-rule",
        verdict,
        severity: Severity::Error,
        reference: reference.into(),
        message: message.into(),
        nets: vec![],
        case: None,
        headline: None,
    }
}

fn report(findings: Vec<Finding>) -> Report {
    Report {
        findings,
        ..Default::default()
    }
}

fn sheet(sheet: &str) -> Drawing {
    Drawing {
        title: sheet.into(),
        sheet: Some(sheet.into()),
        svg: r#"<?xml version="1.0"?><!DOCTYPE svg><svg viewBox="0 0 297 210"></svg>"#.into(),
    }
}

#[test]
fn each_symbol_is_placed_on_its_sheet() {
    let d = kicad::load(&root("tests/fixtures/hier/root.kicad_sch")).unwrap();
    let placed = |r: &str| d.components[r].placed.clone();
    assert_eq!(
        placed("R1"),
        vec![Placed {
            sheet: "/".into(),
            at: (50.0, 50.0)
        }]
    );
    assert_eq!(
        placed("R2"),
        vec![Placed {
            sheet: "/child/".into(),
            at: (50.0, 50.0)
        }]
    );
}

#[test]
fn a_finding_is_ringed_on_its_sheet_and_linked_both_ways() {
    let d = kicad::load(&root("tests/fixtures/hier/root.kicad_sch")).unwrap();
    let page = render(
        &d,
        &report(vec![finding("R2", Verdict::Fail, "R2 <hot>")]),
        &[sheet("/"), sheet("/child/")],
        None,
        "t",
        None,
    );
    // The ring is on the child sheet only, at the symbol, and links to the finding.
    assert!(!page.contains("<?xml"));
    assert!(page.contains(r#"<circle id="m0-1-0" class="mark error" cx="50" cy="50" r="6">"#));
    assert!(page.contains(r##"<a class="ring error" href="#f0"><circle class="halo" cx="50" cy="50" r="6"/>"##));
    assert!(!page.contains("m0-0-0"));
    // The finding links to the ring, and its message is escaped.
    assert!(page.contains(r##"href="#m0-1-0" data-view="1""##));
    assert!(page.contains("R2 &lt;hot&gt;"));
}

#[test]
fn a_finding_is_ringed_on_the_board_at_its_footprint() {
    let dir = root("tests/fixtures/can-encoder");
    let d = kicad::load(&dir.join("can-encoder.kicad_sch")).unwrap();
    let layout = kicad::pcb::load_layout(&dir.join("can-encoder.kicad_pcb")).unwrap();
    let u5 = layout.footprints.iter().find(|f| f.reference == "U5").unwrap();
    let xs = u5.pads.iter().map(|p| p.at.0);
    let ys = u5.pads.iter().map(|p| p.at.1);
    let mid = |v: Vec<f64>| {
        f64::midpoint(
            v.iter().copied().fold(f64::MAX, f64::min),
            v.iter().copied().fold(f64::MIN, f64::max),
        )
    };
    let (x, y) = (mid(xs.collect()), mid(ys.collect()));
    let board = Drawing {
        title: "Board".into(),
        sheet: None,
        svg: r#"<svg width="297mm" viewBox="0 0 297 210"></svg>"#.into(),
    };
    let page = render(
        &d,
        &report(vec![finding("U5", Verdict::Unknown, "m")]),
        &[board],
        Some(&layout),
        "t",
        None,
    );
    assert!(page.contains(&format!(r#"class="mark unknown" cx="{x}" cy="{y}" r="3""#)));
    // The board opens on its footprints, not the whole page.
    assert!(!page.contains(r#"viewBox="0 0 297 210""#));
}

/// Where an error and a pass share a part, the error's ring is drawn last, over the pass's.
#[test]
fn the_worst_ring_is_on_top() {
    let d = kicad::load(&root("tests/fixtures/hier/root.kicad_sch")).unwrap();
    let page = render(
        &d,
        &report(vec![
            finding("R1", Verdict::Pass, "p"),
            finding("R1", Verdict::Fail, "e"),
        ]),
        &[sheet("/")],
        None,
        "t",
        None,
    );
    let (error, pass) = (page.find("mark error").unwrap(), page.find("mark pass").unwrap());
    assert!(pass < error);
}

#[test]
fn without_drawings_the_page_says_why() {
    let d = kicad::load(&root("tests/fixtures/hier/root.kicad_sch")).unwrap();
    let page = render(
        &d,
        &report(vec![finding("R1", Verdict::Fail, "m")]),
        &[],
        None,
        "t",
        Some("can't run kicad-cli"),
    );
    assert!(page.contains("No drawings: can't run kicad-cli"));
    assert!(page.contains(r#"<li id="f0" class="finding error">"#));
}
