//! BOM health against a snapshot of LCSC's answers in `tests/fixtures/lcsc` (taken 2026-11-16), so these
//! tests never touch the network. Refresh the snapshot with
//! `gerberus bom <board> --cache tests/fixtures/lcsc --max-age 0` and update the expected findings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gerberus::bom::{
    Fixed, FixedByMpn, Health, Lcsc, Listing, Options, Severity, Similar, Source, Topic, check, lines, normalize_lcsc,
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
    assert_eq!(l.stock, 629_540);
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
        lifecycle: None,
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

#[test]
fn stock_covers_every_line_that_shares_an_lcsc_number() {
    let mut a = part("R1", "Device:R", "", "C1");
    a.value = "10k".into();
    let mut b = part("R2", "Device:R", "", "C1");
    b.value = "10K".into();
    let l = listing("C1", "R10K", "Acme", 15, "normal", vec![]);
    let r = run(vec![a, b], vec![("C1", Some(l))], 10);
    assert_eq!(r.len(), 2);
    assert!(
        errors(&r)
            .iter()
            .all(|e| e.contains("10 boards need 20 (2 each, counting other lines")),
        "{:?}",
        errors(&r)
    );
    assert_eq!(errors(&r).len(), 2);
}

#[test]
fn only_plain_resistors_and_capacitors_are_commodities() {
    let l = listing("C1", "X1", "Acme", 1000, "normal", vec![]);
    let r = run(vec![part("Y1", "Device:Crystal", "X1", "C1")], vec![("C1", Some(l))], 1);
    assert!(
        r[0].findings
            .iter()
            .any(|f| f.severity == Severity::Warning && f.topic == Topic::SecondSource)
    );
}

#[test]
fn one_mpn_field_in_a_line_is_enough_to_compare() {
    // A0 has no MPN field and its value happens to equal the other's MPN, so both land on one line.
    let mut a = part("A0", "X:Y", "", "C1");
    a.value = "WRONG".into();
    let b = part("A1", "X:Y", "WRONG", "C1");
    let l = listing("C1", "RIGHT", "Acme", 1000, "normal", vec![]);
    let r = run(vec![a, b], vec![("C1", Some(l))], 1);
    assert_eq!(r.len(), 1);
    assert_eq!(errors(&r), ["A0: LCSC C1 is Acme RIGHT, but the MPN field says WRONG"]);
}

#[test]
fn a_missing_package_is_unknown_not_a_match() {
    let mut l = listing(
        "C1",
        "A",
        "Acme",
        1000,
        "normal",
        vec![similar("C2", "Other", 1000, "")],
    );
    let r = run(vec![part("U1", "X:Y", "A", "C1")], vec![("C1", Some(l.clone()))], 1);
    assert!(
        r[0].findings
            .iter()
            .any(|f| f.severity == Severity::Warning && f.topic == Topic::SecondSource)
    );
    l.package.clear();
    let r = run(vec![part("U1", "X:Y", "A", "C1")], vec![("C1", Some(l))], 1);
    assert!(
        r[0].findings
            .iter()
            .any(|f| f.severity == Severity::Unknown && f.topic == Topic::SecondSource)
    );
}

fn temp_cache(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gerberus-bom-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_damaged_cache_file_is_reported_offline() {
    let dir = temp_cache("damaged");
    std::fs::write(dir.join("C1.json"), "{\"fetched\": 1, \"listing\": ").unwrap();
    let mut s = Lcsc::new(Some(dir.clone()), Duration::ZERO, true);
    assert!(s.listing("C1").unwrap_err().contains("damaged"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failed_lookup_falls_back_to_an_old_answer_with_a_warning() {
    let dir = temp_cache("stale");
    std::fs::copy(root("tests/fixtures/lcsc/C1525.json"), dir.join("C1525.json")).unwrap();
    let mut s = Lcsc::new(Some(dir.clone()), Duration::ZERO, false);
    s.endpoint = "http://127.0.0.1:9/?productCode=".into();
    let l = s.listing("C1525").unwrap().unwrap();
    assert_eq!(l.mpn, "CL05B104KO5NNNC");
    assert!(s.caveat("C1525").unwrap().contains("fresh lookup failed"));
    assert!(s.listing("C2").is_err(), "nothing cached and no network is an error");
    let _ = std::fs::remove_dir_all(dir);
}

fn json(rel: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(root(rel)).unwrap()).unwrap()
}

#[test]
fn reads_a_mouser_answer() {
    let raw = json("tests/fixtures/distributors/mouser-LM358DR2G.json");
    let l = Listing::from_mouser_json(&raw, "LM358DR2G").unwrap().unwrap();
    // Only the exact part number counts; LM358DR2GH is another part.
    assert_eq!(l.code, "863-LM358DR2G");
    assert_eq!(l.manufacturer, "onsemi");
    assert_eq!(l.stock, 12345);
    assert_eq!(l.lifecycle, None);
    assert!(Listing::from_mouser_json(&raw, "LM324").unwrap().is_none());
    let err = serde_json::json!({"Errors": [{"Code": "Invalid", "Message": "Invalid unique identifier."}]});
    assert!(
        Listing::from_mouser_json(&err, "X")
            .unwrap_err()
            .contains("Invalid unique identifier")
    );
    // Stock given only as text.
    let text = serde_json::json!({"Errors": [], "SearchResults": {"Parts": [
        {"ManufacturerPartNumber": "A-1", "Availability": "1,500 In Stock", "LifecycleStatus": "Obsolete"}
    ]}});
    let l = Listing::from_mouser_json(&text, "a1").unwrap().unwrap();
    assert_eq!((l.stock, l.lifecycle.as_deref()), (1500, Some("Obsolete")));
}

#[test]
fn reads_a_digikey_answer() {
    let raw = json("tests/fixtures/distributors/digikey-AO3400A.json");
    let l = Listing::from_digikey_json(&raw).unwrap().unwrap();
    assert_eq!(l.code, "785-1000-1-ND");
    assert_eq!(l.mpn, "AO3400A");
    assert_eq!(l.manufacturer, "Alpha & Omega Semiconductor Inc.");
    assert_eq!(l.stock, 81234);
    assert_eq!(l.lifecycle.as_deref(), Some("Active"));
    assert_eq!(l.package, "TO-236-3, SC-59, SOT-23-3");
    // A status given as plain text reads the same.
    let flat = serde_json::json!({"Product": {"ManufacturerProductNumber": "X", "QuantityAvailable": 0,
        "Manufacturer": "Acme", "ProductStatus": "Obsolete"}});
    let l = Listing::from_digikey_json(&flat).unwrap().unwrap();
    assert_eq!(
        (l.manufacturer.as_str(), l.lifecycle.as_deref()),
        ("Acme", Some("Obsolete"))
    );
}

fn by_mpn(parts: Vec<Component>, listings: Vec<(&str, Option<Listing>)>) -> Vec<Health> {
    let design = Design {
        components: parts.into_iter().map(|c| (c.reference.clone(), c)).collect(),
        ..Default::default()
    };
    let mut src = FixedByMpn(listings.into_iter().map(|(k, v)| (k.to_string(), v)).collect());
    check(&design, &mut src, Options { boards: 10 })
}

fn mouser(mpn: &str, stock: u64, lifecycle: Option<&str>) -> Listing {
    Listing {
        code: format!("595-{mpn}"),
        lifecycle: lifecycle.map(str::to_string),
        package: String::new(),
        ..listing("", mpn, "Acme", stock, "normal", vec![])
    }
}

/// Mouser and DigiKey are asked by the MPN field, and give the manufacturer's lifecycle.
#[test]
fn lookups_by_mpn_judge_lifecycle_and_need_an_mpn() {
    let parts = vec![
        part("U1", "X:Y", "OLD1", ""),
        part("U2", "X:Y", "NRND1", ""),
        part("U3", "X:Y", "GOOD1", "C5"),
        part("R1", "Device:R", "", ""),
    ];
    let r = by_mpn(
        parts,
        vec![
            ("OLD1", Some(mouser("OLD1", 100, Some("Obsolete")))),
            (
                "NRND1",
                Some(mouser("NRND1", 100, Some("Not Recommended for New Designs"))),
            ),
            ("GOOD1", Some(mouser("GOOD1", 100, None))),
        ],
    );
    assert_eq!(
        findings(&r),
        vec![
            ("R1".to_string(), Severity::Unknown, Topic::Identity),
            ("U1".to_string(), Severity::Error, Topic::Lifecycle),
            ("U2".to_string(), Severity::Warning, Topic::Lifecycle),
        ]
    );
    let u1 = r.iter().find(|h| h.line.references[0] == "U1").unwrap();
    assert!(
        u1.findings
            .iter()
            .any(|f| f.message.contains("595-OLD1") && f.message.contains("Obsolete"))
    );
    // No similar parts from Mouser: second sources aren't judged, so no single-source warning.
    let u3 = r.iter().find(|h| h.line.references[0] == "U3").unwrap();
    assert!(
        u3.findings.iter().all(|f| f.severity == Severity::Info),
        "{:#?}",
        u3.findings
    );
}

#[test]
fn order_csv_lists_quantities_for_the_build() {
    let mut a = part("U1", "X:Y", "LM358DR2G", "C7950");
    a.fields.insert("Manufacturer".into(), "onsemi".into());
    let b = part("U2", "X:Y", "LM358DR2G", "C7950");
    let mut r = part("R1", "Device:R", "", "");
    r.value = "10k, 1%".into();
    let design = Design {
        components: [a, b, r].into_iter().map(|c| (c.reference.clone(), c)).collect(),
        ..Default::default()
    };
    let csv = gerberus::bom::order_csv(&design, 5);
    assert_eq!(
        csv,
        "Quantity,Manufacturer Part Number,Manufacturer,Reference Designators,Value,LCSC Part Number\n\
         5,,,R1,\"10k, 1%\",\n\
         10,LM358DR2G,onsemi,U1 U2,x,C7950\n"
    );
}

#[test]
fn a_mouser_lookup_without_a_key_says_where_to_put_it() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["bom", "tests/fixtures/sniffer/sniffer.kicad_sch", "--from", "mouser"])
        .current_dir(root(""))
        .env_remove("MOUSER_API_KEY")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("MOUSER_API_KEY"));
}
