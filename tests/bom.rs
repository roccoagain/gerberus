//! BOM health against a snapshot of LCSC's answers in `tests/fixtures/lcsc` (taken 2026-11-16), so these
//! tests never touch the network. Refresh the snapshot with
//! `gerberus bom <board> --cache tests/fixtures/lcsc --max-age 0` and update the expected findings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gerberus::bom::{
    Fixed, Health, Lcsc, Listing, Options, Severity, Similar, Source, Topic, check, lines, normalize_lcsc,
};
use gerberus::design::{Component, Design};
use gerberus::kicad;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn snapshot() -> Lcsc {
    Lcsc::new(Some(root("tests/fixtures/lcsc")), Duration::ZERO, true)
}

fn board(rel: &str) -> Vec<Health> {
    let design = kicad::load(&root(rel)).unwrap();
    check(&design, &mut snapshot(), Options::default())
}

/// `(first reference, severity, topic)` for every finding at or above `Unknown`.
fn findings(report: &[Health]) -> Vec<(String, Severity, Topic)> {
    report
        .iter()
        .flat_map(|h| {
            h.findings
                .iter()
                .filter(|f| f.severity != Severity::Info)
                .map(|f| (h.line.references[0].clone(), f.severity, f.topic))
        })
        .collect()
}

fn errors(report: &[Health]) -> Vec<String> {
    report
        .iter()
        .flat_map(|h| {
            h.findings
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .map(|f| format!("{}: {}", h.line.references[0], f.message))
        })
        .collect()
}

#[test]
fn reads_lcsc_product_detail() {
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root("tests/fixtures/lcsc-raw/C22624.json")).unwrap()).unwrap();
    let l = Listing::from_lcsc_json(&raw).unwrap().unwrap();
    assert_eq!(l.code, "C22624");
    assert_eq!(l.mpn, "B5819WS");
    assert_eq!(l.manufacturer, "JSCJ");
    assert_eq!(l.stock, 629540);
    assert_eq!(l.status, "normal");
    assert_eq!(l.assembly.as_deref(), Some("extended"));
    assert_eq!(l.package, "SOD-323");
    assert_eq!(l.similar.len(), 1);
    assert_eq!(l.similar[0].mpn, "B1040WS");
    assert_eq!(l.similar[0].stock, 6000);
}

#[test]
fn lcsc_without_a_listing_or_with_an_error() {
    let none = serde_json::json!({"code": 200, "msg": null, "result": null, "ok": true});
    assert_eq!(Listing::from_lcsc_json(&none).unwrap(), None);
    let err = serde_json::json!({"code": 404, "msg": "gone", "result": null, "ok": false});
    assert!(Listing::from_lcsc_json(&err).unwrap_err().contains("gone"));
    let partial = serde_json::json!({"code": 200, "ok": true, "result": {"productCode": "C1"}});
    assert!(Listing::from_lcsc_json(&partial).is_err());
}

#[test]
fn lcsc_numbers() {
    assert_eq!(normalize_lcsc(" c1525 ").as_deref(), Some("C1525"));
    assert_eq!(normalize_lcsc("C1525"), Some("C1525".into()));
    assert_eq!(normalize_lcsc("C"), None);
    assert_eq!(normalize_lcsc("1525"), None);
    assert_eq!(normalize_lcsc("C15 25"), None);
}

#[test]
fn offline_lookup_of_an_uncached_part_is_an_error_not_a_guess() {
    let mut s = snapshot();
    assert!(s.listing("C999999999").unwrap_err().contains("not in the LCSC cache"));
}

#[test]
fn lines_group_by_part_number_and_skip_unfitted_parts() {
    let design = kicad::load(&root("tests/fixtures/can-encoder/can-encoder.kicad_sch")).unwrap();
    let l = lines(&design);
    let caps = l.iter().find(|l| l.mpn == "CL05B104KO5NNNC").unwrap();
    assert_eq!(caps.references, ["C3", "C6", "C7", "C9", "C10", "C11", "C12"]);
    assert_eq!(caps.lcsc.as_deref(), Some("C1525"));
    assert!(caps.commodity);
    for line in &l {
        for r in &line.references {
            let c = &design.components[r];
            assert!(!c.dnp && c.in_bom && c.on_board, "{r} is not fitted but has a BOM line");
        }
    }
}

#[test]
fn can_encoder_rev2() {
    let r = board("tests/fixtures/can-encoder/can-encoder.kicad_sch");
    assert_eq!(
        errors(&r),
        [
            "U3: LCSC has none of C2932578 in stock",
            "U4: LCSC has none of C3236207 in stock",
        ]
    );
    let f = findings(&r);
    assert!(f.contains(&("L1".into(), Severity::Warning, Topic::Lifecycle)), "{f:?}");
    // Passives from one maker are not flagged as single source.
    assert!(
        !f.iter()
            .any(|(r, _, t)| r.starts_with('R') && *t == Topic::SecondSource),
        "{f:?}"
    );
    assert!(
        f.contains(&("U1".into(), Severity::Warning, Topic::SecondSource)),
        "{f:?}"
    );
}

#[test]
fn can_encoder_rev1_and_sniffer_have_no_errors() {
    for b in [
        "tests/fixtures/can-encoder-rev1/can-encoder.kicad_sch",
        "tests/fixtures/sniffer/sniffer.kicad_sch",
    ] {
        assert_eq!(errors(&board(b)), Vec::<String>::new(), "{b}");
    }
    let f = findings(&board("tests/fixtures/sniffer/sniffer.kicad_sch"));
    assert!(
        f.contains(&("U3".into(), Severity::Warning, Topic::Lifecycle)),
        "SCD41 has no LCSC listing: {f:?}"
    );
    let f = findings(&board("tests/fixtures/can-encoder-rev1/can-encoder.kicad_sch"));
    assert!(
        f.contains(&("JP1".into(), Severity::Unknown, Topic::Identity)),
        "the jumper has no LCSC number: {f:?}"
    );
}

#[test]
fn testbed_catches_an_lcsc_number_for_a_different_part() {
    let r = board("tests/fixtures/testbed/testbed.kicad_sch");
    assert_eq!(
        errors(&r),
        ["U1: LCSC C7950 is onsemi LM358DR2G, but the MPN field says LM358DR"]
    );
    // Parts named only by value can't be compared with LCSC's part number, so they aren't.
    assert!(
        r.iter()
            .find(|h| h.line.mpn == "1uF")
            .unwrap()
            .findings
            .iter()
            .all(|f| f.topic != Topic::Identity)
    );
}

fn part(reference: &str, lib_id: &str, mpn: &str, lcsc: &str) -> Component {
    let mut fields = BTreeMap::new();
    if !mpn.is_empty() {
        fields.insert("MPN".to_string(), mpn.to_string());
    }
    if !lcsc.is_empty() {
        fields.insert("LCSC".to_string(), lcsc.to_string());
    }
    Component {
        reference: reference.into(),
        value: "x".into(),
        lib_id: lib_id.into(),
        fields,
        in_bom: true,
        on_board: true,
        ..Default::default()
    }
}

fn listing(code: &str, mpn: &str, maker: &str, stock: u64, status: &str, similar: Vec<Similar>) -> Listing {
    Listing {
        code: code.into(),
        mpn: mpn.into(),
        manufacturer: maker.into(),
        stock,
        status: status.into(),
        assembly: None,
        package: "SOT-23".into(),
        similar,
    }
}

fn similar(code: &str, maker: &str, stock: u64, package: &str) -> Similar {
    Similar {
        code: code.into(),
        mpn: format!("{maker}-PART"),
        manufacturer: maker.into(),
        stock,
        status: "normal".into(),
        package: package.into(),
        description: String::new(),
    }
}

fn run(parts: Vec<Component>, listings: Vec<(&str, Option<Listing>)>, boards: u64) -> Vec<Health> {
    let design = Design {
        components: parts.into_iter().map(|c| (c.reference.clone(), c)).collect(),
        ..Default::default()
    };
    let mut src = Fixed(listings.into_iter().map(|(k, v)| (k.to_string(), v)).collect());
    check(&design, &mut src, Options { boards })
}

#[test]
fn stock_is_checked_against_the_build_quantity() {
    let parts = vec![part("U1", "X:Y", "A", "C1"), part("U2", "X:Y", "A", "C1")];
    let l = listing("C1", "A", "Acme", 30, "normal", vec![]);
    let r = run(parts.clone(), vec![("C1", Some(l.clone()))], 10);
    assert_eq!(errors(&r), Vec::<String>::new());
    let r = run(parts, vec![("C1", Some(l))], 20);
    assert_eq!(errors(&r), ["U1: LCSC has 30 of C1; 20 boards need 40 (2 each)"]);
}

#[test]
fn listing_status_other_than_normal_is_a_warning() {
    let l = listing("C1", "A", "Acme", 1000, "discontinued", vec![]);
    let r = run(vec![part("U1", "X:Y", "A", "C1")], vec![("C1", Some(l))], 1);
    assert!(r[0].findings.iter().any(|f| f.severity == Severity::Warning
        && f.topic == Topic::Lifecycle
        && f.message.contains("\"discontinued\"")));
}

#[test]
fn second_sources_need_another_maker_the_same_package_and_stock() {
    let sims = vec![
        similar("C2", "Acme", 1000, "SOT-23"),    // same maker
        similar("C3", "Other", 1000, "SOT-23-5"), // another package
        similar("C4", "Third", 5, "SOT-23"),      // not enough for 10 boards
    ];
    let l = listing("C1", "A", "Acme", 1000, "normal", sims.clone());
    let r = run(vec![part("U1", "X:Y", "A", "C1")], vec![("C1", Some(l))], 10);
    assert!(
        r[0].findings
            .iter()
            .any(|f| f.severity == Severity::Warning && f.topic == Topic::SecondSource)
    );

    let mut ok = sims;
    ok.push(similar("C5", "Fourth", 1000, "SOT-23"));
    let l = listing("C1", "A", "Acme", 1000, "normal", ok);
    let r = run(vec![part("U1", "X:Y", "A", "C1")], vec![("C1", Some(l))], 10);
    let f = r[0].findings.iter().find(|f| f.topic == Topic::SecondSource).unwrap();
    assert_eq!(f.severity, Severity::Info);
    assert!(
        f.message.contains("Fourth") && !f.message.contains("Third"),
        "{}",
        f.message
    );
}

#[test]
fn a_commodity_resistor_from_one_maker_is_not_a_sourcing_risk() {
    let l = listing("C1", "R1K", "Acme", 1000, "normal", vec![]);
    let r = run(vec![part("R1", "Device:R", "R1K", "C1")], vec![("C1", Some(l))], 1);
    assert!(
        r[0].findings.iter().all(|f| f.severity == Severity::Info),
        "{:?}",
        r[0].findings
    );
}

#[test]
fn bad_or_missing_lcsc_numbers_are_unknown() {
    let r = run(
        vec![part("U1", "X:Y", "A", "LCSC-123"), part("U2", "X:Y", "B", "")],
        vec![],
        1,
    );
    assert!(
        r.iter()
            .all(|h| h.listing.is_none() && h.findings.len() == 1 && h.findings[0].severity == Severity::Unknown)
    );
    assert!(r[0].findings[0].message.contains("LCSC-123"));
}
