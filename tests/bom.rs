use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gerberus::bom::{
    Fixed, Health, Listing, Online, Options, Severity, Similar, Source, Topic, check, lines, normalize_lcsc,
};
use gerberus::design::{Component, Design};
use gerberus::kicad;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn snapshot() -> Online {
    Online::new(Some(root("tests/fixtures/lcsc")), Duration::ZERO, true)
}

fn board(rel: &str) -> Vec<Health> {
    let design = kicad::load(&root(rel)).unwrap();
    check(&design, &mut snapshot(), Options::default())
}

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
            "C4: LCSC has none of C45783 in stock",
            "R5: LCSC has none of C25744 in stock",
            "U3: LCSC has none of C2932578 in stock",
            "U4: LCSC has none of C3236207 in stock",
        ]
    );
    let f = findings(&r);
    assert!(f.contains(&("L1".into(), Severity::Warning, Topic::Lifecycle)), "{f:?}");
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
fn can_encoder_rev1_and_sniffer_have_only_stock_errors() {
    assert_eq!(
        errors(&board("tests/fixtures/sniffer/sniffer.kicad_sch")),
        [
            "R1: LCSC has none of C25804 in stock",
            "R3: LCSC has none of C21190 in stock"
        ]
    );
    assert_eq!(
        errors(&board("tests/fixtures/can-encoder-rev1/can-encoder.kicad_sch")),
        [
            "C4: LCSC has none of C45783 in stock",
            "R5: LCSC has none of C25744 in stock"
        ]
    );
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
        [
            "R1: LCSC has none of C25804 in stock",
            "U1: LCSC C7950 is onsemi LM358DR2G, but the MPN field says LM358DR"
        ]
    );
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
        params: BTreeMap::new(),
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
    check(
        &design,
        &mut src,
        Options {
            boards,
            ..Options::default()
        },
    )
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
        similar("C2", "Acme", 1000, "SOT-23"),
        similar("C3", "Other", 1000, "SOT-23-5"),
        similar("C4", "Third", 5, "SOT-23"),
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
        r[0].findings
            .iter()
            .filter(|f| f.topic == Topic::SecondSource)
            .all(|f| f.severity == Severity::Info),
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
    let mut s = Online::new(Some(dir.clone()), Duration::ZERO, true);
    assert!(s.listing("C1").unwrap_err().contains("damaged"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failed_lookup_falls_back_to_an_old_answer_with_a_warning() {
    let dir = temp_cache("stale");
    std::fs::copy(root("tests/fixtures/lcsc/C1525.json"), dir.join("C1525.json")).unwrap();
    let mut s = Online::new(Some(dir.clone()), Duration::ZERO, false);
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
fn reads_lcsc_parameters() {
    let l = Listing::from_lcsc_json(&json("tests/fixtures/lcsc-raw/C21120.json"))
        .unwrap()
        .unwrap();
    assert_eq!(l.params.get("Capacitance").map(String::as_str), Some("220nF"));
    assert_eq!(l.params.get("Voltage Rating").map(String::as_str), Some("25V"));
    let s = gerberus::spec::Spec::from_lcsc(&l.params, &l.package);
    assert_eq!(s.describe(), "220nF ±10% 25V X7R 0603 capacitor");
}

fn passive(reference: &str, value: &str, footprint: &str, mpn: &str, lcsc: &str) -> Component {
    let lib_id = if reference.starts_with('R') {
        "Device:R"
    } else {
        "Device:C"
    };
    Component {
        value: value.into(),
        footprint: footprint.into(),
        ..part(reference, lib_id, mpn, lcsc)
    }
}

fn resistor(code: &str, mpn: &str, ohms: &str, size: &str, stock: u64, basic: bool, similar: Vec<Similar>) -> Listing {
    Listing {
        package: size.into(),
        assembly: Some(if basic { "basic" } else { "extended" }.into()),
        params: [("Resistance", ohms), ("Tolerance", "±1%"), ("Power(Watts)", "100mW")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        ..listing(code, mpn, "UNI-ROYAL", stock, "normal", similar)
    }
}

fn jlcpcb(parts: Vec<Component>, listings: Vec<(&str, Option<Listing>)>) -> Vec<Health> {
    let design = Design {
        components: parts.into_iter().map(|c| (c.reference.clone(), c)).collect(),
        ..Default::default()
    };
    let mut src = Fixed(listings.into_iter().map(|(k, v)| (k.to_string(), v)).collect());
    check(
        &design,
        &mut src,
        Options {
            boards: 10,
            jlcpcb: true,
        },
    )
}

const R0603: &str = "Resistor_SMD:R_0603_1608Metric";

#[test]
fn an_equivalent_resistor_from_another_maker_is_a_warning_with_the_mpn_to_use() {
    let r = jlcpcb(
        vec![passive("R1", "10k", R0603, "RMCF0603FT10K0", "C25804")],
        vec![(
            "C25804",
            Some(resistor("C25804", "0603WAF1002T5E", "10kΩ", "0603", 5000, true, vec![])),
        )],
    );
    let f = &r[0].findings[0];
    assert_eq!((f.severity, f.topic), (Severity::Warning, Topic::Identity), "{f:?}");
    assert!(f.message.contains("same value and size"), "{}", f.message);
    assert!(
        f.notes[0].contains("doesn't know RMCF0603FT10K0's tolerance and rated power"),
        "{:?}",
        f.notes
    );
    assert_eq!(
        f.help.as_deref(),
        Some("set the MPN field to 0603WAF1002T5E so the checks use the part JLCPCB will fit")
    );
}

#[test]
fn an_lcsc_number_for_another_value_or_size_is_an_error() {
    let r = jlcpcb(
        vec![
            passive("R1", "2.2", R0603, "", "C22939"),
            passive("R2", "10k", "Resistor_SMD:R_0805_2012Metric", "", "C25804"),
        ],
        vec![
            (
                "C22939",
                Some(resistor("C22939", "0603WAF220JT5E", "22Ω", "0603", 5000, true, vec![])),
            ),
            (
                "C25804",
                Some(resistor("C25804", "0603WAF1002T5E", "10kΩ", "0603", 5000, true, vec![])),
            ),
        ],
    );
    assert_eq!(
        errors(&r),
        [
            "R1: LCSC C22939 is a different part: 22Ω, not the 2.2Ω the schematic gives",
            "R2: LCSC C25804 is a different part: size 0603, not the 0805 of the footprint",
        ]
    );
}

#[test]
fn a_capacitor_rated_below_the_mpn_is_an_error() {
    let mut lib = gerberus::limits::Library::builtin().unwrap();
    let design = Design {
        components: [passive(
            "C1",
            "220nF",
            "Capacitor_SMD:C_0603_1608Metric",
            "CL10B224KB8NNNC",
            "C21120",
        )]
        .into_iter()
        .map(|c| (c.reference.clone(), c))
        .collect(),
        ..Default::default()
    };
    lib.expand_families(&design);
    let l = Listing::from_lcsc_json(&json("tests/fixtures/lcsc-raw/C21120.json"))
        .unwrap()
        .unwrap();
    let mut src = Fixed([("C21120".to_string(), Some(l))].into_iter().collect());
    let r = gerberus::bom::check_with_parts(&design, &mut src, Options::default(), Some(&lib));
    let f = &r[0].findings[0];
    assert_eq!(f.severity, Severity::Error);
    assert_eq!(
        f.message,
        "LCSC C21120 is rated below the schematic's part: rated voltage 25V, where the schematic's part has 50V"
    );
    let r = check(&design, &mut src, Options::default());
    assert_eq!(r[0].findings[0].severity, Severity::Warning);
}

#[test]
fn out_of_stock_resistors_get_checked_equivalents_basic_first() {
    let sims = vec![
        similar("C2", "Other", 9000, "0603"),
        similar("C3", "Other", 9000, "0603"),
        similar("C4", "Other", 9000, "0603"),
    ];
    let r = jlcpcb(
        vec![passive("R1", "10k", R0603, "", "C1")],
        vec![
            ("C1", Some(resistor("C1", "A", "10kΩ", "0603", 0, true, sims))),
            ("C2", Some(resistor("C2", "B", "10kΩ", "0603", 9000, false, vec![]))),
            ("C3", Some(resistor("C3", "C", "10kΩ", "0603", 9000, true, vec![]))),
            ("C4", Some(resistor("C4", "D", "1kΩ", "0603", 9000, true, vec![]))),
        ],
    );
    let stock = r[0].findings.iter().find(|f| f.topic == Topic::Stock).unwrap();
    let help = stock.help.as_deref().unwrap();
    assert!(
        help.contains("C3 UNI-ROYAL C") && help.contains("C2 UNI-ROYAL B"),
        "{help}"
    );
    assert!(
        help.find("C3 ").unwrap() < help.find("C2 ").unwrap(),
        "basic first: {help}"
    );
    assert!(!help.contains("C4 "), "{help}");
}

#[test]
fn for_jlcpcb_a_missing_lcsc_number_is_an_error_with_a_search_link() {
    let r = jlcpcb(
        vec![part("J1", "Connector:X", "S2B-XH-A", ""), part("U1", "X:Y", "A", "C9")],
        vec![(
            "C9",
            Some(Listing {
                assembly: Some("extended".into()),
                ..listing("C9", "A", "Acme", 900, "normal", vec![])
            }),
        )],
    );
    let f = &r[0].findings[0];
    assert_eq!((f.severity, f.topic), (Severity::Error, Topic::Assembly));
    assert!(
        f.help
            .as_deref()
            .unwrap()
            .contains("https://www.lcsc.com/search?q=S2B-XH-A")
    );
    let last = r.last().unwrap().findings.last().unwrap();
    assert!(
        last.message
            .starts_with("1 of 2 lines are JLCPCB extended parts (0 basic)"),
        "{}",
        last.message
    );
    let text = gerberus::bom::render(
        &r,
        Options {
            boards: 10,
            jlcpcb: true,
        },
        &gerberus::diag::Show::everything(),
    );
    assert!(
        text.contains("error[bom-assembly]: J1: no LCSC part number, so JLCPCB can't fit S2B-XH-A"),
        "{text}"
    );
    assert!(
        text.contains("error: 1 error, 1 warning, 0 unknown, 0 passed"),
        "{text}"
    );
}

#[test]
fn packing_suffixes_after_a_comma_or_bracket_are_the_same_part() {
    let r = run(
        vec![
            part("D1", "X:Y", "PESD5V0S1BA", "C1"),
            part("J1", "X:Y", "S2B-XH-A", "C2"),
            part("D2", "X:Y", "PESD5V0S1BAF", "C1"),
        ],
        vec![
            (
                "C1",
                Some(listing("C1", "PESD5V0S1BA,115", "Nexperia", 9000, "normal", vec![])),
            ),
            (
                "C2",
                Some(listing("C2", "S2B-XH-A(LF)(SN)", "JST", 9000, "normal", vec![])),
            ),
        ],
        10,
    );
    assert_eq!(
        errors(&r),
        ["D2: LCSC C1 is Nexperia PESD5V0S1BA,115, but the MPN field says PESD5V0S1BAF"]
    );
}

#[test]
fn no_capacitor_equivalent_is_offered_without_a_known_dielectric() {
    let cap = |code: &str, stock: u64, diel: Option<&str>, similar: Vec<Similar>| Listing {
        package: "1206".into(),
        params: [
            ("Capacitance", "470nF"),
            ("Tolerance", "±5%"),
            ("Voltage Rating", "25V"),
        ]
        .into_iter()
        .chain(diel.map(|d| ("Temperature Coefficient", d)))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect(),
        ..listing(code, code, "Acme", stock, "normal", similar)
    };
    let r = jlcpcb(
        vec![passive("C1", "470nF", "Capacitor_SMD:C_1206_3216Metric", "", "C1")],
        vec![
            (
                "C1",
                Some(cap("C1", 7, None, vec![similar("C2", "Other", 9000, "1206")])),
            ),
            ("C2", Some(cap("C2", 9000, Some("X7R"), vec![]))),
        ],
    );
    let stock = r[0].findings.iter().find(|f| f.topic == Topic::Stock).unwrap();
    assert_eq!(stock.help, None);
    assert!(
        stock.notes.iter().any(|n| n.contains("gives the dielectric")),
        "{:?}",
        stock.notes
    );
}

#[test]
fn for_jlcpcb_lcsc_running_out_of_a_basic_part_is_a_warning() {
    let r = jlcpcb(
        vec![
            passive("R1", "10k", R0603, "", "C1"),
            passive("R2", "1k", R0603, "", "C2"),
        ],
        vec![
            ("C1", Some(resistor("C1", "A", "10kΩ", "0603", 0, true, vec![]))),
            ("C2", Some(resistor("C2", "B", "1kΩ", "0603", 0, false, vec![]))),
        ],
    );
    let sev = |i: usize| r[i].findings.iter().find(|f| f.topic == Topic::Stock).unwrap().severity;
    assert_eq!((sev(0), sev(1)), (Severity::Warning, Severity::Error));
}

#[test]
fn every_part_of_a_line_and_every_unreadable_value_is_accounted_for() {
    let ic = Listing {
        package: "SOT-23".into(),
        params: [("Supply Voltage".to_string(), "5V".to_string())].into_iter().collect(),
        ..listing("C3", "IC", "Acme", 9000, "normal", vec![])
    };
    let r = jlcpcb(
        vec![
            passive("R1", "10k", R0603, "", "C1"),
            passive("R2", "10k", "Resistor_SMD:R_0805_2012Metric", "", "C1"),
            passive("R3", "ten", R0603, "", "C2"),
            passive("R4", "1k", R0603, "", "C3"),
        ],
        vec![
            ("C1", Some(resistor("C1", "A", "10kΩ", "0603", 9000, true, vec![]))),
            ("C2", Some(resistor("C2", "B", "10kΩ", "0603", 9000, true, vec![]))),
            ("C3", Some(ic)),
        ],
    );
    let f = findings(&r);
    assert_eq!(
        errors(&r)[0],
        "R1: for R2, LCSC C1 is a different part: size 0603, not the 0805 of the footprint"
    );
    assert!(f.contains(&("R3".into(), Severity::Unknown, Topic::Identity)), "{f:?}");
    assert!(f.contains(&("R4".into(), Severity::Error, Topic::Identity)), "{f:?}");
    let text = gerberus::bom::render(
        &r,
        Options {
            boards: 10,
            jlcpcb: true,
        },
        &gerberus::diag::Show::everything(),
    );
    assert!(
        text.contains("error: 2 errors, 0 warnings, 1 unknown, 0 passed"),
        "{text}"
    );
}
