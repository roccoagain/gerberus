//! Checks `gerberus datasheet`: finding a part file's values on the datasheet pages they cite, and comparing
//! two blind readings. The pages here are made-up tables laid out the way `pdftotext -layout` prints real ones.

use std::path::{Path, PathBuf};

use gerberus::datasheet::{Claim, Claimed, Role, Status, Unit, blank, check_claim, claims, compare, verify_with};
use gerberus::limits::{Library, Part};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn claim(value: f64, role: Role, unit: Unit, context: &[&str]) -> Claim {
    Claim {
        label: "test".into(),
        value: Claimed::Number(value),
        role,
        unit,
        page: 1,
        context: context.iter().map(|s| (*s).to_string()).collect(),
        derived: None,
        other_end: None,
    }
}

const ABS_MAX: &str = "
7.1 Absolute Maximum Ratings
                             PARAMETER                                          MIN                                 MAX                    UNIT
                        VIN to GND                                              –0.3                                 38
Input Voltages          EN to GND                                               –0.3                            VIN + 0.3                      V
                        FB to GND                                               –0.3                                 5.5
TJ                      Junction temperature                                     –40                                 150                   °C
";

const ELECTRICAL: &str = "
7.5 Electrical Characteristics
                   PARAMETER                                    TEST CONDITIONS                   MIN        TYP         MAX     UNIT
VIN             Operation input voltage                                                              4                     36       V
IQ              Operating quiescent current                     VEN = 3.3 V                                     80         120       µA
                                                        TJ = 25 °C                               0.995       1.00       1.005       V
VREF            Reference voltage (FB pin)
                                                        TJ = –40 °C to 125 °C                    0.985       1.00       1.015       V
IOUT            Output current                                                                               600                    mA
";

#[test]
fn a_limit_in_its_column_is_confirmed() {
    let o = check_claim(&claim(38.0, Role::Max, Unit::Volt, &["VIN", "GND"]), ABS_MAX);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
    assert!(o.evidence[0].contains("VIN to GND"));
    let o = check_claim(&claim(-0.3, Role::Min, Unit::Volt, &["FB", "GND"]), ABS_MAX);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
    let o = check_claim(&claim(150.0, Role::Max, Unit::Celsius, &["TJ"]), ABS_MAX);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
}

#[test]
fn a_typical_value_filed_as_a_limit_is_a_mismatch() {
    let o = check_claim(&claim(1.0, Role::Max, Unit::Volt, &["FB"]), ELECTRICAL);
    assert_eq!(o.status, Status::Mismatch, "{o:?}");
    assert!(o.reason.contains("TYP"), "{}", o.reason);
    // The real maximum is confirmed.
    let o = check_claim(&claim(1.015, Role::Max, Unit::Volt, &["FB"]), ELECTRICAL);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
}

#[test]
fn units_come_from_the_unit_column_and_are_scaled() {
    // 120 µA is 0.00012 A.
    let o = check_claim(&claim(120e-6, Role::Max, Unit::Amp, &["VEN"]), ELECTRICAL);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
    // The same number taken as amps isn't there.
    let o = check_claim(&claim(120.0, Role::Max, Unit::Amp, &[]), ELECTRICAL);
    assert_eq!(o.status, Status::NotFound, "{o:?}");
    // A number in a different unit than claimed is a mismatch.
    let o = check_claim(&claim(36.0, Role::Max, Unit::Amp, &["VIN"]), ELECTRICAL);
    assert_eq!(o.status, Status::Mismatch, "{o:?}");
}

#[test]
fn a_row_that_names_no_pin_is_only_plausible() {
    let o = check_claim(&claim(36.0, Role::Max, Unit::Volt, &["CANH"]), ELECTRICAL);
    assert_eq!(o.status, Status::Plausible, "{o:?}");
    assert!(o.reason.contains("doesn't name CANH"), "{}", o.reason);
}

#[test]
fn relative_bounds_are_found_in_their_column() {
    let c = Claim {
        value: Claimed::Pin {
            pin: "VIN".into(),
            plus: 0.3,
        },
        ..claim(0.0, Role::Max, Unit::Volt, &["EN", "GND"])
    };
    assert_eq!(check_claim(&c, ABS_MAX).status, Status::Confirmed);
    let wrong = Claim {
        value: Claimed::Pin {
            pin: "VIN".into(),
            plus: 0.5,
        },
        ..c.clone()
    };
    assert_eq!(check_claim(&wrong, ABS_MAX).status, Status::NotFound);
    // A pin named VDD/VDDA is printed as either.
    let slash = Claim {
        value: Claimed::Pin {
            pin: "VDD/VIN".into(),
            plus: 0.3,
        },
        ..c
    };
    assert_eq!(check_claim(&slash, ABS_MAX).status, Status::Confirmed);
}

#[test]
fn tilde_cells_say_which_end_is_which() {
    let page = "
Absolute Maximum Ratings
             Parameter                     Symbol             Ratings                  Units
           Output Voltage                    VOUT         Vss-0.3～VIN +0.3                  V
    Operating Temperature Range              TOPR           －40～＋150                        ℃
 Part Number     Marking   VRWM (V)    VBR (V) @ IT
 SMAJ18A         BT        18.0        20.00~22.10
";
    let o = check_claim(&claim(-0.3, Role::Min, Unit::Volt, &["VOUT"]), page);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
    let o = check_claim(&claim(-40.0, Role::Min, Unit::Celsius, &["TOPR"]), page);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
    let o = check_claim(&claim(22.1, Role::Max, Unit::Volt, &["SMAJ18A"]), page);
    assert!(o.status >= Status::Plausible, "{o:?}");
    let o = check_claim(&claim(22.1, Role::Min, Unit::Volt, &["SMAJ18A"]), page);
    assert!(o.status <= Status::Plausible, "{o:?}");
    let rel = Claim {
        value: Claimed::Pin {
            pin: "VIN".into(),
            plus: 0.3,
        },
        ..claim(0.0, Role::Max, Unit::Volt, &["VOUT"])
    };
    assert_eq!(check_claim(&rel, page).status, Status::Confirmed);
}

#[test]
fn a_column_per_part_number_counts_as_that_parts_value() {
    let page = "
 Characteristic                                Symbol      B5817WS        B5818WS       B5819WS        Unit
 Working peak reverse voltage                  VRWM           20             30           40             V
";
    let o = check_claim(&claim(40.0, Role::Max, Unit::Volt, &["K", "A", "B5819WS"]), page);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
}

#[test]
fn percentages_fractions_and_plus_minus() {
    let page = "
Characteristics                                       Value              Unit
Tolerance                                              ±1%
Rated power (0402)                                     1/16              W
ESD, human-body model                                  ± 2000             V
";
    let o = check_claim(&claim(0.01, Role::Value, Unit::Fraction, &[]), page);
    assert!(o.status >= Status::Plausible, "{o:?}");
    let o = check_claim(&claim(0.0625, Role::Max, Unit::Watt, &[]), page);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
    let o = check_claim(&claim(-2000.0, Role::Min, Unit::Volt, &[]), page);
    assert_eq!(o.status, Status::Confirmed, "{o:?}");
}

#[test]
fn derived_values_are_checked_by_their_inputs() {
    let page = "
Output voltage accuracy                                          ±2%
Output voltage                         VOUT                3.3                    V
";
    let mut c = claim(3.234, Role::Min, Unit::Volt, &["VOUT"]);
    assert_eq!(check_claim(&c, page).status, Status::NotFound);
    c.derived = Some("3.3 V × (1 − 2 %)".into());
    let o = check_claim(&c, page);
    assert_eq!(o.status, Status::Derived, "{o:?}");
    c.derived = Some("4.1 V ± 3 %".into());
    let o = check_claim(&c, page);
    assert_eq!(o.status, Status::NotFound, "{o:?}");
    assert!(o.reason.contains('3'), "{}", o.reason);
    // A non-polar rating's minimum is the negative of its maximum; a zero is the bottom of a maximum.
    let mut sym = claim(-3.3, Role::Min, Unit::Volt, &[]);
    sym.other_end = Some(3.3);
    assert_eq!(check_claim(&sym, page).status, Status::Derived);
    assert_eq!(
        check_claim(&claim(0.0, Role::Min, Unit::Watt, &[]), page).status,
        Status::Derived
    );
}

#[test]
fn a_page_without_text_needs_eyes() {
    let o = check_claim(&claim(5.0, Role::Max, Unit::Volt, &[]), "\n\n  \u{c}");
    assert_eq!(o.status, Status::NoText);
    assert!(!o.is_problem());
}

#[test]
fn verify_reads_each_page_once_and_reports_failures_to_read() {
    let part: Part = toml::from_str(
        r#"
mpn = "X"
manufacturer = "M"
description = "d"
class = "ic"
[datasheet]
title = "t"
document = "d"
url = "https://example.com/x.pdf"
[pins]
1 = "VIN"
2 = "GND"
3 = "EN"
[[limit]]
pins = ["VIN"]
to = "GND"
abs_max = { min = -0.3, max = 38, page = 4 }
recommended = { min = 4, max = 36, page = 5 }
[[limit]]
pins = ["EN"]
to = "GND"
abs_max = { min = -0.3, max = { pin = "VIN", plus = 0.3 }, page = 4 }
recommended = { min = 0, max = 5, page = 9 }
"#,
    )
    .unwrap();
    let mut reads = Vec::new();
    let outcomes = verify_with(&part, |p| {
        reads.push(p);
        match p {
            4 => Ok(ABS_MAX.to_string()),
            5 => Ok(ELECTRICAL.to_string()),
            _ => Err("no such page".into()),
        }
    });
    assert_eq!(reads, vec![4, 5, 9]);
    let status: Vec<Status> = outcomes.iter().map(|o| o.status).collect();
    assert_eq!(
        status,
        vec![
            Status::Confirmed,
            Status::Confirmed,
            Status::Confirmed,
            Status::Confirmed,
            Status::Confirmed,
            Status::Confirmed,
            Status::NotFound,
            Status::NotFound
        ],
        "{outcomes:#?}"
    );
    let report = gerberus::datasheet::render("X", &outcomes, false);
    assert!(
        report.contains("NOT FOUND X limit 2 (EN to GND) recommended min"),
        "{report}"
    );
    assert!(report.contains("6 confirmed"), "{report}");
}

/// Every value in the shared library cites a page, and a blind copy of every file asks the same questions.
#[test]
fn every_library_file_blanks_and_compares_with_itself() {
    let lib = Library::load_dir(&root("parts")).unwrap();
    for (mpn, path) in &lib.files {
        let part = &lib.parts[mpn];
        if part.mpn != *mpn {
            continue; // a variant; its file is checked under its own part number
        }
        for c in claims(part) {
            assert!(c.page > 0, "{mpn}: {}", c.label);
        }
        assert!(compare(part, part).is_empty(), "{mpn}");
        let text = std::fs::read_to_string(path).unwrap();
        let blanked = blank(&text).unwrap();
        for key in ["abs_max = \"?\"", "recommended = \"?\""] {
            assert_eq!(
                text.matches(&key.replace(" = \"?\"", " = ")).count(),
                blanked.matches(key).count(),
                "{mpn}: {key}"
            );
        }
        assert!(!blanked.contains("page ="), "{mpn}: a page leaked into the blank copy");
    }
}

#[test]
fn compare_lists_every_disagreement() {
    let first: Part = toml::from_str(
        r#"
mpn = "R"
manufacturer = "M"
description = "d"
class = "resistor"
tolerance = { value = 0.01, page = 2 }
[datasheet]
title = "t"
document = "d"
url = "u"
[pins]
1 = "1"
2 = "2"
[[limit]]
pins = ["1"]
to = "2"
abs_max = { unknown = "none" }
recommended = { min = -50, max = 50, page = 5 }
[[limit]]
quantity = "power"
pins = ["1"]
to = "2"
abs_max = { unknown = "none" }
recommended = { min = 0, max = 0.0625, page = 4 }
"#,
    )
    .unwrap();
    let mut second = first.clone();
    second.tolerance.as_mut().unwrap().value = 0.05;
    second.limits[1].recommended = toml::from_str::<toml::Table>("r = { min = 0, max = 0.1, page = 4 }").unwrap()["r"]
        .clone()
        .try_into()
        .unwrap();
    second.limits[0].abs_max = second.limits[0].recommended.clone();
    let diffs = compare(&first, &second);
    let at: Vec<&str> = diffs.iter().map(|d| d.at.as_str()).collect();
    assert_eq!(
        at,
        vec!["tolerance", "limit 1 (1) abs_max", "limit 2 (1) recommended"],
        "{diffs:#?}"
    );
    assert_eq!(diffs[2].first, "0 .. 0.0625 (page 4)");
    assert_eq!(diffs[2].second, "0 .. 0.1 (page 4)");
    second.limits.pop();
    assert!(compare(&first, &second).iter().any(|d| d.at == "limits"));
}

#[test]
fn finds_the_pdf_behind_an_lcsc_datasheet_page() {
    let html = r#"<div><iframe src="https://datasheet.lcsc.com/datasheet/pdf/82c4.pdf?productCode=C123789" loading="lazy"></div>"#;
    assert_eq!(
        gerberus::datasheet::lcsc_pdf_link(html).as_deref(),
        Some("https://datasheet.lcsc.com/datasheet/pdf/82c4.pdf")
    );
    assert_eq!(gerberus::datasheet::lcsc_pdf_link("<html>no link</html>"), None);
}
