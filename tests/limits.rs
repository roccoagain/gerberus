//! Checks the part-limits library in `parts/` and the board inputs files against the test boards.

use std::path::{Path, PathBuf};

use scoutfly::inputs::BoardInputs;
use scoutfly::kicad;
use scoutfly::limits::{Bound, Library, Quantity, Range, component_mpn};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn library() -> Library {
    Library::load_dir(&root("parts")).unwrap()
}

const BOARDS: [(&str, &str); 3] = [
    ("sniffer", "sniffer.kicad_sch"),
    ("can-encoder", "can-encoder.kicad_sch"),
    ("can-encoder-rev1", "can-encoder.kicad_sch"),
];

#[test]
fn every_part_file_is_sound() {
    let lib = library();
    let mut problems = Vec::new();
    for (mpn, part) in &lib.parts {
        for p in part.problems() {
            problems.push(format!("{}: {p}", lib.files[mpn].display()));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Every part placed on the three boards has a limits file, and the file knows every pin the board uses.
#[test]
fn every_board_part_has_limits() {
    let lib = library();
    let mut problems = Vec::new();
    for (dir, sch) in BOARDS {
        let design = kicad::load(&root(&format!("tests/fixtures/{dir}/{sch}"))).unwrap();
        for (r, c) in &design.components {
            if !is_bought(c) {
                continue;
            }
            let Some(part) = lib.part_for(c) else {
                problems.push(format!("{dir}: {r} ({}) has no file in parts/", component_mpn(c)));
                continue;
            };
            for pin in c.pins.keys() {
                if !part.pins.contains_key(pin) {
                    problems.push(format!("{dir}: {r} pin {pin} is not in {}'s [pins]", part.mpn));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Footprint-only parts (test pads, solder jumpers) are not bought, so they have no datasheet. Rev 1's JP1
/// is in its BOM by mistake, so the test goes by the missing part number as well.
fn is_bought(c: &scoutfly::design::Component) -> bool {
    let has_mpn = c.fields.get("MPN").is_some_and(|m| !m.trim().is_empty());
    has_mpn && !c.dnp && c.in_bom && c.on_board
}

/// The parts left out of the check above are the ones that really aren't bought.
#[test]
fn only_footprint_parts_are_skipped() {
    let mut skipped = Vec::new();
    for (dir, sch) in BOARDS {
        let design = kicad::load(&root(&format!("tests/fixtures/{dir}/{sch}"))).unwrap();
        for (r, c) in &design.components {
            if !is_bought(c) {
                skipped.push(format!("{dir}:{r}"));
            }
        }
    }
    assert_eq!(
        skipped,
        [
            "sniffer:J2",
            "can-encoder:J3",
            "can-encoder:JP1",
            "can-encoder-rev1:J3",
            "can-encoder-rev1:JP1"
        ]
    );
}

#[test]
fn board_inputs_match_their_designs() {
    for (dir, sch) in BOARDS {
        let design = kicad::load(&root(&format!("tests/fixtures/{dir}/{sch}"))).unwrap();
        let inputs = BoardInputs::load(&root(&format!("tests/fixtures/{dir}/inputs.toml"))).unwrap();
        assert!(!inputs.inputs.is_empty(), "{dir}: no inputs");
        let problems = inputs.problems(&design);
        assert!(problems.is_empty(), "{dir}: {}", problems.join("\n"));
    }
}

/// A TVS's breakdown minimum and stand-off voltage, from its cathode-to-anode limit.
fn tvs_numbers(lib: &Library, mpn: &str) -> (f64, f64) {
    let part = &lib.parts[mpn];
    let limit = part.limits_for(Quantity::Voltage, "K").next().unwrap();
    let Some(Range::Known {
        min: Bound::Value(vbr), ..
    }) = &limit.breakdown
    else {
        panic!("{mpn}: no breakdown voltage")
    };
    let Range::Known {
        max: Bound::Value(vrwm),
        ..
    } = &limit.recommended
    else {
        panic!("{mpn}: no stand-off voltage")
    };
    (*vbr, *vrwm)
}

fn vbus_max(dir: &str) -> f64 {
    let inputs = BoardInputs::load(&root(&format!("tests/fixtures/{dir}/inputs.toml"))).unwrap();
    inputs.inputs.iter().find(|i| i.name == "VBUS").unwrap().max
}

/// Rev 1's known bug is visible in the data: its TVS (D2, SMAJ18A) starts conducting below the bus maximum.
/// D2 sits behind the reverse-polarity Schottky D1, whose drop (0.6 V max at 1 A) doesn't close the gap.
/// The rule that walks from J1 through D1 to D2 comes later; this only checks the numbers support it.
#[test]
fn rev1_tvs_conducts_on_the_bus() {
    let lib = library();
    let design = kicad::load(&root("tests/fixtures/can-encoder-rev1/can-encoder.kicad_sch")).unwrap();
    assert_eq!(component_mpn(&design.components["D2"]), "SMAJ18A");
    let (vbr_min, vrwm) = tvs_numbers(&lib, "SMAJ18A");
    let bus = vbus_max("can-encoder-rev1");
    assert!(bus - 0.6 > vbr_min, "bus {bus} V vs breakdown {vbr_min} V");
    assert!(bus > vrwm);
}

/// Rev 2's fix holds up in the same data: the SMF26A's stand-off covers the bus.
#[test]
fn rev2_tvs_stays_off_on_the_bus() {
    let lib = library();
    let design = kicad::load(&root("tests/fixtures/can-encoder/can-encoder.kicad_sch")).unwrap();
    assert_eq!(component_mpn(&design.components["D2"]), "SMF26A");
    let (vbr_min, vrwm) = tvs_numbers(&lib, "SMF26A");
    let bus = vbus_max("can-encoder");
    assert!(
        bus <= vrwm && bus < vbr_min,
        "bus {bus} V vs stand-off {vrwm} V, breakdown {vbr_min} V"
    );
}

#[test]
fn relative_bounds_resolve() {
    let lib = library();
    let part = &lib.parts["STM32G030F6P6TR"];
    let io = part.limits_for(Quantity::Voltage, "PA0").next().unwrap();
    let vdd = |p: &str| (p == "VDD/VDDA").then_some(3.3);
    let Range::Known { max, .. } = &io.recommended else {
        panic!()
    };
    assert_eq!(max.resolve(vdd), Some(5.5)); // min(3.3 + 3.6, 5.5)
    let Range::Known { max, .. } = &io.abs_max else {
        panic!()
    };
    assert!((max.resolve(vdd).unwrap() - 7.3).abs() < 1e-9);
    assert_eq!(max.resolve(|_| None), None);
}

const HEAD: &str = "mpn = \"X\"\nmanufacturer = \"M\"\ndescription = \"D\"\n[pins]\n1 = \"A\"\n2 = \"B\"\n";

#[test]
fn rejects_malformed_files() {
    let bad = [
        // A range with only one end.
        "abs_max = { max = 5, page = 1 }\nrecommended = { unknown = \"none\" }",
        // An unknown with no reason.
        "abs_max = { unknown = \"\" }\nrecommended = { unknown = \"none\" }",
        // An unknown mixed with values.
        "abs_max = { unknown = \"x\", min = 0, max = 1 }\nrecommended = { unknown = \"none\" }",
        // A misspelt field.
        "abs_max = { min = 0, max = 5, pgae = 1 }\nrecommended = { unknown = \"none\" }",
        // A bound that is neither a number, a pin nor an unknown.
        "abs_max = { min = 0, max = { plus = 1 }, page = 1 }\nrecommended = { unknown = \"none\" }",
    ];
    for limit in bad {
        let text = format!("{HEAD}[[limit]]\npins = [\"A\"]\nto = \"B\"\n{limit}");
        assert!(
            toml::from_str::<scoutfly::limits::Part>(&text).is_err(),
            "accepted:\n{text}"
        );
    }
}

#[test]
fn reports_unsound_files() {
    let text = format!(
        "{HEAD}[[limit]]\npins = [\"A\", \"C\"]\nto = \"B\"\nabs_max = {{ min = 0, max = 5 }}\nrecommended = {{ min = 0, max = 6, page = 2 }}"
    );
    let part: scoutfly::limits::Part = toml::from_str(&text).unwrap();
    let problems = part.problems().join("\n");
    assert!(problems.contains("pin \"C\" is not in [pins]"), "{problems}");
    assert!(
        problems.contains("abs_max has values but no datasheet page"),
        "{problems}"
    );
    assert!(
        problems.contains("has values but the part has no datasheet"),
        "{problems}"
    );
    assert!(problems.contains("reaches outside abs_max"), "{problems}");
}
