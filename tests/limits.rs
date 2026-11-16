//! Checks the part-limits library in `parts/` and the board inputs files against the test boards.

use std::path::{Path, PathBuf};

use gerberus::design::{Component, Pin, PinKind};
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::{
    Bound, Coverage, DriveKind, Library, Lookup, Quantity, Range, class_default, component_mpn, coverage,
};
use gerberus::policy::Policy;

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
fn is_bought(c: &Component) -> bool {
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
    inputs
        .inputs
        .iter()
        .find(|i| i.name == "VBUS")
        .unwrap()
        .max
        .value()
        .unwrap()
}

/// Rev 1's known bug is visible in the data: its TVS (D2, SMAJ18A) starts conducting below the bus maximum.
/// D2 sits behind the reverse-polarity Schottky D1, so it sees VBUS less D1's forward drop. Even the largest
/// drop the B5819W file gives doesn't close the gap. The rule that walks from J1 through D1 to D2 comes later;
/// this only checks the numbers support it.
#[test]
fn rev1_tvs_conducts_on_the_bus() {
    let lib = library();
    let design = kicad::load(&root("tests/fixtures/can-encoder-rev1/can-encoder.kicad_sch")).unwrap();
    assert_eq!(component_mpn(&design.components["D2"]), "SMAJ18A");
    let d1 = lib.part_for(&design.components["D1"]).unwrap();
    let drop = d1.drives.iter().find(|d| d.kind == DriveKind::Forward).unwrap();
    let Range::Known {
        max: Bound::Value(vf_max),
        ..
    } = &drop.range
    else {
        panic!("no VF max")
    };
    let (vbr_min, vrwm) = tvs_numbers(&lib, "SMAJ18A");
    let bus = vbus_max("can-encoder-rev1");
    assert!(
        bus - vf_max > vbr_min,
        "bus {bus} V - {vf_max} V vs breakdown {vbr_min} V"
    );
    assert!(bus - vf_max > vrwm);
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

const HEAD: &str =
    "mpn = \"X\"\nmanufacturer = \"M\"\ndescription = \"D\"\nclass = \"ic\"\n[pins]\n1 = \"A\"\n2 = \"B\"\n";

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
            toml::from_str::<gerberus::limits::Part>(&text).is_err(),
            "accepted:\n{text}"
        );
    }
}

#[test]
fn reports_unsound_files() {
    let text = format!(
        "{HEAD}[[limit]]\npins = [\"A\", \"C\"]\nto = \"B\"\nabs_max = {{ min = 0, max = 5 }}\nrecommended = {{ min = 0, max = 6, page = 2 }}"
    );
    let part: gerberus::limits::Part = toml::from_str(&text).unwrap();
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

/// Each capacitor's schematic `Voltage` field agrees with its datasheet rating.
#[test]
fn capacitor_voltage_fields_match_datasheets() {
    let lib = library();
    let mut problems = Vec::new();
    for (dir, sch) in BOARDS {
        let design = kicad::load(&root(&format!("tests/fixtures/{dir}/{sch}"))).unwrap();
        for (r, c) in &design.components {
            let Some(field) = c.fields.get("Voltage") else { continue };
            let part = lib.part_for(c).unwrap();
            let from_sheet = part.limits[0].recommended.values().unwrap().1;
            let from_field = class_default(c).unwrap().limits[0].recommended.values().unwrap().1;
            if from_sheet != from_field {
                problems.push(format!("{dir}: {r} says {field}, datasheet says {from_sheet} V"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

fn capacitor(value: &str, voltage: Option<&str>) -> Component {
    let mut c = Component {
        reference: "C9".into(),
        value: value.into(),
        lib_id: "Device:C_Small".into(),
        ..Default::default()
    };
    if let Some(v) = voltage {
        c.fields.insert("Voltage".into(), v.into());
    }
    for n in ["1", "2"] {
        c.pins.insert(
            n.into(),
            Pin {
                number: n.into(),
                name: String::new(),
                kind: PinKind::Passive,
                no_connect: false,
                net: 0,
            },
        );
    }
    c
}

#[test]
fn capacitors_without_a_file_get_a_class_default() {
    let lib = library();
    for (value, field, want) in [
        ("1uF", Some("35V"), Some(35.0)),
        ("100nF 6V3", None, Some(6.3)),
        ("10uF", None, None),
    ] {
        let c = capacitor(value, field);
        match lib.lookup(&c) {
            Lookup::ClassDefault(p) => {
                assert_eq!(p.limits[0].recommended.values(), Some((-want.unwrap(), want.unwrap())));
            }
            Lookup::Unchecked(_) => assert_eq!(want, None, "{value}"),
            Lookup::Datasheet(p) => panic!("{value} matched {}", p.mpn),
        }
    }
}

/// A part with no limits shows as unchecked instead of passing silently.
#[test]
fn coverage_reports_unchecked_parts() {
    let lib = library();
    let design = kicad::load(&root("tests/fixtures/can-encoder-rev1/can-encoder.kicad_sch")).unwrap();
    let cov = coverage(&design, &lib);
    // JP1 is a solder jumper with no part number that rev 1 left in its BOM.
    assert!(matches!(cov["JP1"], Coverage::Unchecked { .. }), "{:?}", cov["JP1"]);
    assert_eq!(cov["J3"], Coverage::NotFitted);
    assert_eq!(cov["D2"], Coverage::Datasheet { mpn: "SMAJ18A".into() });
    let unchecked: Vec<&String> = cov
        .iter()
        .filter(|(_, c)| matches!(c, Coverage::Unchecked { .. }))
        .map(|(r, _)| r)
        .collect();
    assert_eq!(unchecked, ["JP1"]);
}

fn scratch_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gerberus-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A board's own parts folder overrides the shared library, and a variant reuses the limits by pin name.
#[test]
fn board_files_override_and_variants_share_limits() {
    let board = scratch_dir("override");
    std::fs::write(
        board.join("SMAJ18A.toml"),
        format!(
            "{}\n[[variant]]\nmpn = \"SMAJ18A-ALT\"\npins = {{ 2 = \"K\", 1 = \"A\" }}\n",
            std::fs::read_to_string(root("parts/SMAJ18A.toml"))
                .unwrap()
                .replace("MDD (Microdiode)", "Board copy")
        ),
    )
    .unwrap();
    let lib = Library::load_dirs(&[&root("parts"), &board]).unwrap();
    assert_eq!(lib.parts["SMAJ18A"].manufacturer, "Board copy");
    assert_eq!(lib.parts["SMAJ18A-ALT"].pins_named("K"), ["2"]);
    assert_eq!(lib.parts["SMAJ18A-ALT"].limits.len(), lib.parts["SMAJ18A"].limits.len());
    assert!(lib.parts["SMAJ18A"].problems().is_empty());
    // Two files for one part number in the same folder is an error.
    std::fs::copy(board.join("SMAJ18A.toml"), board.join("copy.toml")).unwrap();
    assert!(Library::load_dirs(&[&board]).is_err());
    std::fs::remove_dir_all(&board).unwrap();
}

#[test]
fn variant_pin_maps_must_name_every_limited_pin() {
    let text = format!(
        "{HEAD}[[variant]]\nmpn = \"Y\"\npins = {{ 1 = \"A\" }}\n[[limit]]\npins = [\"A\"]\nto = \"B\"\nabs_max = {{ unknown = \"x\" }}\nrecommended = {{ unknown = \"x\" }}"
    );
    let part: gerberus::limits::Part = toml::from_str(&text).unwrap();
    assert!(
        part.problems().join("\n").contains("variant \"Y\" has no pin named B"),
        "{:?}",
        part.problems()
    );
}

#[test]
fn derating_policy_is_separate_from_limits() {
    let lib = library();
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let cap = &lib.parts["CL21A226MAQNNNE"];
    let d = policy.derate_for(cap, &cap.limits[0]).unwrap();
    assert_eq!(d.factor, 0.8);
    let tvs = &lib.parts["SMAJ18A"];
    assert!(policy.derate_for(tvs, &tvs.limits[0]).is_none());

    let dir = scratch_dir("policy");
    std::fs::write(
        dir.join("p.toml"),
        "[[derate]]\nquantity = \"voltage\"\nfactor = 1.5\nreason = \"x\"\n",
    )
    .unwrap();
    let err = Policy::load(&dir.join("p.toml")).unwrap_err();
    assert!(
        err.contains("exactly one of `class` and `mpn`") && err.contains("not in (0, 1]"),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The regulators say what they hold their outputs at, so a checker can start the rails from them.
#[test]
fn regulators_and_diodes_describe_what_they_drive() {
    let lib = library();
    for (mpn, pin, lo, hi) in [
        ("TPS560430YFDBVR", "FB", 0.985, 1.015),
        ("AP63200WU-7", "FB", 0.792, 0.808),
        ("ME6211C33M5G-N", "VOUT", 3.234, 3.366),
        ("XC6206P332MR-G", "VOUT", 3.234, 3.366),
    ] {
        let d = lib.parts[mpn]
            .drives
            .iter()
            .find(|d| d.kind == DriveKind::Regulates)
            .unwrap();
        assert_eq!((d.pin.as_str(), d.range.values()), (pin, Some((lo, hi))), "{mpn}");
    }
}

/// Every pin a part has is either limited or reported as uncovered, and the gaps are the known ones.
#[test]
fn uncovered_pins_are_the_known_gaps() {
    let lib = library();
    let mut gaps = Vec::new();
    for (mpn, part) in &lib.parts {
        for pin in part.uncovered_pins() {
            gaps.push(format!("{mpn}.{pin}"));
        }
    }
    // Inductors have only a current limit; ground and reference pins appear as `to`.
    assert_eq!(
        gaps,
        [
            "FNR4030S6R8MT.1",
            "FNR4030S6R8MT.2",
            "SWPA252012S100MT.1",
            "SWPA252012S100MT.2"
        ]
    );
}

#[test]
fn inputs_reject_same_net_and_repeated_names() {
    let design = kicad::load(&root("tests/fixtures/can-encoder/can-encoder.kicad_sch")).unwrap();
    let text = "board = \"x\"\n[[input]]\nname = \"A\"\npin = \"J1.1\"\nto = \"J2.1\"\nmin = 0\nmax = 1\nsource = \"s\"\n\
                [[input]]\nname = \"A\"\npin = \"J1.1\"\nto = \"J1.4\"\nmin = 0\nmax = 1\nsource = \"s\"\n";
    let inputs: BoardInputs = toml::from_str(text).unwrap();
    let problems = inputs.problems(&design).join("\n");
    assert!(
        problems.contains("J1.1 and J2.1 are on the same net, VBUS"),
        "{problems}"
    );
    assert!(problems.contains("A: the name is used twice"), "{problems}");
}

#[test]
fn inputs_reject_bad_probe_faults_and_differentials() {
    let design = kicad::load(&root("tests/fixtures/can-encoder/can-encoder.kicad_sch")).unwrap();
    let text = r#"board = "x"
[[input]]
name = "P"
pin = "J3.2"
to = "J3.1"
min = 0
max = { pin = "J9.9" }
source = "s"

[[input]]
name = "Q"
pin = "J3.3"
to = "J3.1"
min = 0
max = { pin = "J3.3" }
source = "s"

[[input]]
name = "F"
fault = " "
pin = "J1.2"
to = "J1.4"
min = 0
max = 1
source = "s"

[[differential]]
pin = "J1.2"
to = "J2.2"
min = 1
max = 0
source = ""

[[differential]]
pin = "J1.2"
to = "J3.2"
min = 0
max = 1
source = "s"
"#;
    let inputs: BoardInputs = toml::from_str(text).unwrap();
    let problems = inputs.problems(&design).join("\n");
    for want in [
        "P: max follows pin J9.9, which is not in the design",
        "Q: max follows J3.3, which is on the input's own net",
        "F: an empty fault name",
        "differential J1.2 to J2.2: both pins are on one net",
        "differential J1.2 to J2.2: min 1 and max 0 are not a range",
        "differential J1.2 to J2.2: no source",
        "differential J1.2 to J3.2: both pins must be on nets a normal input drives",
    ] {
        assert!(problems.contains(want), "missing {want:?} in\n{problems}");
    }
}

/// A typo inside `max = { ... }` is an error, not a silently dropped `plus`.
#[test]
fn follow_rejects_unknown_keys() {
    let text = "board = \"x\"\n[[input]]\nname = \"P\"\npin = \"J3.2\"\nto = \"J3.1\"\nmin = 0\n\
                max = { pin = \"J3.5\", pls = 0.3 }\nsource = \"s\"\n";
    assert!(toml::from_str::<BoardInputs>(text).is_err());
}

/// Regulators and switch nodes name the pin that powers them, and only a switch node's range may follow it.
#[test]
fn drives_name_their_supply() {
    let parse = |extra: &str| -> String {
        // Top-level keys go before the tables.
        let (top, tail) = if extra.starts_with('[') {
            ("", extra)
        } else {
            (extra, "")
        };
        let text = format!("{top}\n{HEAD}[datasheet]\ntitle = \"t\"\ndocument = \"d\"\nurl = \"u\"\n{tail}");
        let part: gerberus::limits::Part = toml::from_str(&text).unwrap();
        part.problems().join("\n")
    };
    let p = parse("[[drive]]\nkind = \"regulates\"\npin = \"A\"\nto = \"B\"\nrange = { min = 1, max = 2, page = 1 }");
    assert!(p.contains("needs `supply`"), "{p}");
    let p = parse(
        "[[drive]]\nkind = \"regulates\"\npin = \"A\"\nto = \"B\"\nsupply = \"C\"\nrange = { min = 1, max = 2, page = 1 }",
    );
    assert!(p.contains("\"C\""), "{p}");
    let p = parse(
        "[[drive]]\nkind = \"regulates\"\npin = \"A\"\nto = \"B\"\nsupply = \"B\"\nrange = { min = 1, max = { pin = \"B\" }, page = 1 }",
    );
    assert!(p.contains("only a switch node's range can be relative"), "{p}");
    let p = parse(
        "[[drive]]\nkind = \"switches\"\npin = \"A\"\nto = \"B\"\nsupply = \"B\"\nrange = { min = { unknown = \"x\" }, max = { pin = \"B\" }, page = 1 }",
    );
    assert!(p.is_empty(), "{p}");
    let p = parse(
        "[[drive]]\nkind = \"forward\"\npin = \"A\"\nto = \"B\"\nfeedback = true\nrange = { min = 0, max = 1, page = 1 }",
    );
    assert!(p.contains("only a regulator has `feedback`"), "{p}");
    let p = parse(
        "[[drive]]\nkind = \"regulates\"\npin = \"A\"\nto = \"B\"\nsupply = \"B\"\nheadroom = { duty = 1.5, page = 1 }\nrange = { min = 1, max = 2, page = 1 }",
    );
    assert!(p.contains("headroom needs"), "{p}");
    let p = parse("tcr = { ppm = -1, page = 1 }");
    assert!(p.contains("tcr"), "{p}");
    let p = parse("tolerance = { value = 1.5, page = 1 }");
    assert!(p.contains("tolerance"), "{p}");
}

/// Resistors carry the tolerance their part number encodes, for divider math.
#[test]
fn resistors_give_their_tolerance() {
    let lib = library();
    let mut missing = vec![];
    for (mpn, part) in &lib.parts {
        if part.class == gerberus::limits::Class::Resistor {
            match &part.tolerance {
                Some(t) => {
                    assert_eq!(t.value, 0.01, "{mpn}");
                    assert_eq!(part.tcr.as_ref().map(|t| t.ppm), Some(100.0), "{mpn}");
                    assert!(part.temperature.is_some(), "{mpn}");
                }
                None => missing.push(mpn.as_str()),
            }
        }
    }
    assert!(missing.is_empty(), "no tolerance: {missing:?}");
}
