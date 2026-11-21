//! Part families: decoding part numbers, and agreeing with the part files written by hand for the same parts.
use gerberus::design::Component;
use gerberus::limits::family::{ValueKind, parse_value};
use gerberus::limits::{Class, Library, Lookup, Quantity, Range};

fn builtin() -> Library {
    Library::builtin().unwrap()
}

fn family<'a>(lib: &'a Library, name: &str) -> &'a gerberus::limits::family::Family {
    &lib.families.iter().find(|(_, f)| f.family == name).unwrap().1
}

#[test]
fn values_decode() {
    let r = |s| parse_value(ValueKind::Resistance, s);
    assert_eq!(r("10K"), Some(10e3));
    assert_eq!(r("4K7"), Some(4.7e3));
    assert_eq!(r("97R6"), Some(97.6));
    assert_eq!(r("470R"), Some(470.0));
    assert_eq!(r("1M"), Some(1e6));
    assert_eq!(r("0R"), Some(0.0));
    assert_eq!(r("10"), None);
    assert_eq!(r("1K2K"), None);
    let c = |s| parse_value(ValueKind::CapacitancePf, s).map(|f| (f * 1e15).round() / 1e3);
    assert_eq!(c("104"), Some(100_000.0));
    assert_eq!(c("106"), Some(10_000_000.0));
    assert_eq!(c("1R5"), Some(1.5));
    assert_eq!(c("10"), None);
    for bad in ["-R5", "+R5", "0R0", "10R", "109", "108", "R5R", "005"] {
        assert_eq!(c(bad), None, "{bad}");
    }
    assert_eq!(c("475"), Some(4_700_000.0));
    let d = |s| parse_value(ValueKind::ResistanceDigits, s);
    assert_eq!(d("1001"), Some(1e3));
    assert_eq!(d("0103"), Some(10e3));
    assert_eq!(d("4701"), Some(4.7e3));
    assert_eq!(d("100J"), Some(10.0));
    assert_eq!(d("470K"), Some(4.7));
    assert_eq!(d("100L"), Some(0.1));
    assert_eq!(d("1004"), Some(1e6));
    assert_eq!(d("1005"), Some(10e6));
    assert_eq!(d("0000"), Some(0.0));
    for bad in ["100", "10001", "1007", "100Q", "10K0", "1R00"] {
        assert_eq!(d(bad), None, "{bad}");
    }
}

#[test]
fn uniroyal_part_numbers_decode() {
    let lib = builtin();
    let f = family(&lib, "UNI-ROYAL thick film");
    let d = f.decode("0402WGF1001TCE").unwrap().unwrap();
    assert_eq!(d.codes["size"], "0402");
    assert_eq!(d.codes["power"], "WG");
    assert_eq!(d.value, Some(1e3));
    // 10 kohm at 1/8 W: sqrt(0.125 x 10000) = 35.4 V, below 0805's 150 V.
    let p = f.part("0805W8F1002T5E").unwrap().unwrap();
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    let v = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
    assert_eq!(v.recommended.values(), Some((-35.355, 35.355)));
    let w = p.limits.iter().find(|l| l.quantity == Quantity::Power).unwrap();
    assert_eq!(w.recommended.values(), Some((0.0, 0.125)));
    assert_eq!(p.tcr.as_ref().unwrap().ppm, 100.0);
    assert_eq!(p.temperature.as_ref().map(|t| (t.min, t.max)), Some((-55.0, 155.0)));
    // A 5 % value starts with 0: 0103 is 10 kohm.
    let p = f.part("0402WGJ0103TCE").unwrap().unwrap();
    assert_eq!(p.tolerance.as_ref().unwrap().value, 0.05);
    // 1 Mohm on a 0603: sqrt(0.1 x 1e6) = 316 V, capped at 75 V.
    let p = f.part("0603WAF1004T5E").unwrap().unwrap();
    let v = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
    assert_eq!(v.recommended.values(), Some((-75.0, 75.0)));
    // 0805 below 0.1 ohm is the 1/4 W code, with its own TCR rows.
    let p = f.part("0805W4F500MT5E").unwrap().unwrap();
    let w = p.limits.iter().find(|l| l.quantity == Quantity::Power).unwrap();
    assert_eq!(w.recommended.values(), Some((0.0, 0.25)));
    assert_eq!(p.tcr.as_ref().unwrap().ppm, 800.0);
    // TCR boundaries on a 0402: 10 ohm is in the 1-10 ohm row.
    let tcr = |mpn: &str| f.part(mpn).unwrap().unwrap().tcr.unwrap().ppm;
    assert_eq!(tcr("0402WGF100JTCE"), 200.0);
    assert_eq!(tcr("0402WGF102JTCE"), 100.0);
    // A jumper is named as one, not as a value outside the range table.
    let e = f.part("0402WGF0000TCE").unwrap().unwrap_err();
    assert!(e.contains("jumper"), "{e}");
    // 01005: size code 0105, 1/32 W, 15 V, -55 to 125 °C, its own TCR bands.
    let p = f.part("0105WHF1001TCE").unwrap().unwrap();
    let w = p.limits.iter().find(|l| l.quantity == Quantity::Power).unwrap();
    assert_eq!(w.recommended.values(), Some((0.0, 0.03125)));
    assert_eq!(p.temperature.as_ref().map(|t| (t.min, t.max)), Some((-55.0, 125.0)));
    assert_eq!(p.tcr.as_ref().unwrap().ppm, 200.0);
    assert_eq!(f.part("0105WHJ0100TCE").unwrap().unwrap().tcr.unwrap().ppm, 300.0);
    assert!(f.part("0105WHF0100TCE").unwrap().is_ok());
    assert!(f.part("0105WHF910KTCE").unwrap().is_err());
    // Not rated: the wrong power code for the size, a jumper, 0805 1/4 W at 10 ohm, ±0.5 % below 1 ohm, 0201
    // at ±0.5 %, 0805 1/8 W at 10 Mohm (the range stops below it).
    for mpn in [
        "0402WAF1001TCE",
        "0402WGF0000TCE",
        "0805W4F100JT5E",
        "0603WAD470LT5E",
        "0201WMD1001TCE",
        "0805W8F1005T5E",
    ] {
        assert!(f.part(mpn).unwrap().is_err(), "{mpn} was rated");
    }
    // Not this family's: no E at the end, an unknown size.
    assert!(f.decode("0402WGF1001TC").is_none());
    assert!(f.decode("0404WGF1001TCE").is_none());
}

#[test]
fn yageo_part_numbers_decode() {
    let lib = builtin();
    let f = family(&lib, "Yageo RC_L");
    let d = f.decode("RC0805FR-0710KL").unwrap().unwrap();
    assert_eq!(d.codes["size"], "0805");
    assert_eq!(d.codes["tolerance"], "F");
    assert_eq!(d.codes["reel"], "07");
    assert_eq!(d.value, Some(10e3));
    assert!(f.decode("RC0805FR-0710K").is_none());
    assert!(f.decode("RC0806FR-0710KL").is_none());

    // 10 kohm at 1/8 W: sqrt(0.125 x 10000) = 35.4 V, below 0805's 150 V.
    let p = f.part("RC0805FR-0710KL").unwrap().unwrap();
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    let v = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
    assert_eq!(v.recommended.values(), Some((-35.355, 35.355)));
    let w = p.limits.iter().find(|l| l.quantity == Quantity::Power).unwrap();
    assert_eq!(w.recommended.values(), Some((0.0, 0.125)));
    assert_eq!(p.tolerance.as_ref().unwrap().value, 0.01);
    assert_eq!(p.tcr.as_ref().unwrap().ppm, 100.0);

    // 1 Mohm on a 1206: sqrt(0.25 x 1e6) = 500 V, capped at the 200 V maximum working voltage.
    let p = f.part("RC1206FR-071ML").unwrap().unwrap();
    let v = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
    assert_eq!(v.recommended.values(), Some((-200.0, 200.0)));

    // Reel 7W doubles the power.
    let p = f.part("RC0402JR-7W4K7L").unwrap().unwrap();
    let w = p.limits.iter().find(|l| l.quantity == Quantity::Power).unwrap();
    assert_eq!(w.recommended.values(), Some((0.0, 0.125)));
    assert_eq!(p.tcr.as_ref().unwrap().ppm, 200.0);

    // A 1210 has no double-power row, and a jumper has no power rating: neither is rated.
    assert!(f.part("RC1210FR-7W10KL").unwrap().is_err());
    assert!(f.part("RC0603JR-070RL").unwrap().is_err());

    // Part numbers the datasheet doesn't list aren't rated: double power above 1 Mohm, 47 Mohm in 0402,
    // 0.5 ohm, 0.1 % at 1 ohm, 7D on a 0075, ESD-safe packing on a 0603.
    for mpn in [
        "RC0603JR-7W10ML",
        "RC0402FR-0747ML",
        "RC0603FR-070R5L",
        "RC0402BR-071RL",
        "RC0075FS-7D1KL",
        "RC0603FS-0710KL",
    ] {
        let r = f.part(mpn).unwrap();
        assert!(r.is_err(), "{mpn} was rated");
    }

    // TCR boundaries: 10 ohm is in the 1-10 ohm row, just above it the 10 ohm - 10 Mohm row.
    let tcr = |mpn: &str| f.part(mpn).unwrap().unwrap().tcr.unwrap().ppm;
    assert_eq!(tcr("RC0603FR-0710RL"), 200.0);
    assert_eq!(tcr("RC0603FR-0710R2L"), 100.0);
    assert_eq!(tcr("RC0603FR-0710ML"), 100.0);
    assert_eq!(tcr("RC0603JR-0712ML"), 200.0);
    assert_eq!(tcr("RC0805JR-0727ML"), 300.0);
    assert_eq!(tcr("RC0075FS-0799RL"), 600.0);
    assert_eq!(tcr("RC0075FS-07100RL"), 200.0);

    // The working-voltage bound never rounds above the datasheet's sqrt(P x R).
    let p = f.part("RC0805FR-0710KL").unwrap().unwrap();
    let v = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
    assert!(v.recommended.values().unwrap().1 <= (0.125f64 * 1e4).sqrt());
    assert!(matches!(v.recommended, Range::Known { page: Some(7), .. }));
}

#[test]
fn samsung_part_numbers_decode() {
    let lib = builtin();
    let f = family(&lib, "Samsung CL");
    let d = f.decode("CL10A106MP8NNNC").unwrap().unwrap();
    assert_eq!(d.codes["size"], "10");
    assert_eq!(d.codes["voltage"], "P");
    let p = f.part("CL10A106MP8NNNC").unwrap().unwrap();
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    let v = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
    assert_eq!(v.recommended.values(), Some((-10.0, 10.0)));
    assert_eq!(p.tolerance.as_ref().unwrap().value, 0.2);
    assert_eq!(p.temperature.as_ref().map(|t| (t.min, t.max)), Some((-55.0, 85.0)));
    // Below 10 pF, F means +/-1 pF, not 1 %, so no tolerance; Z (+80/-20 %) gives none either.
    assert!(f.part("CL10C5R6FB8NNNC").unwrap().unwrap().tolerance.is_none());
    assert_eq!(
        f.part("CL10C101FB8NNNC").unwrap().unwrap().tolerance.unwrap().value,
        0.01
    );
    assert!(f.part("CL21F105ZO8NNNC").unwrap().unwrap().tolerance.is_none());
    // Y5V's range is -30 to 85 °C.
    let t = f.part("CL21F105ZO8NNNC").unwrap().unwrap().temperature.unwrap();
    assert_eq!((t.min, t.max), (-30.0, 85.0));
    assert!(f.decode("CL10A106MP8NNNX").is_none());
}

#[test]
fn yageo_cc_and_fenghua_part_numbers_decode() {
    let lib = builtin();
    let volts = |p: &gerberus::limits::Part| {
        p.limits
            .iter()
            .find(|l| l.quantity == Quantity::Voltage)
            .and_then(|l| l.recommended.values())
    };
    let y = family(&lib, "Yageo CC X7R");
    let d = y.decode("CC0402KRX7R7BB104").unwrap().unwrap();
    assert_eq!(d.codes["voltage"], "7");
    let p = y.part("CC0402KRX7R7BB104").unwrap().unwrap();
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    assert_eq!(volts(&p), Some((-16.0, 16.0)));
    assert_eq!(p.tolerance.as_ref().unwrap().value, 0.1);
    assert_eq!(p.temperature.as_ref().map(|t| (t.min, t.max)), Some((-55.0, 125.0)));
    assert_eq!(volts(&y.part("CC1206KKX7R5BB106").unwrap().unwrap()), Some((-6.3, 6.3)));
    assert_eq!(
        volts(&y.part("CC1210KKX7RYBB104").unwrap().unwrap()),
        Some((-250.0, 250.0))
    );
    // NP0 is another datasheet, and B isn't a voltage code.
    assert!(y.decode("CC0402JRNPO9BN101").is_none());
    assert!(y.decode("CC0402KRX7RBBB104").is_none());

    let f = family(&lib, "Fenghua general MLCC");
    let p = f.part("0603B104K500NT").unwrap().unwrap();
    assert!(p.problems().is_empty(), "{:?}", p.problems());
    assert_eq!(volts(&p), Some((-50.0, 50.0)));
    assert_eq!(p.temperature.as_ref().map(|t| (t.min, t.max)), Some((-55.0, 125.0)));
    // X5R to 85 °C, X6S to 105 °C; 6R3 is 6.3 V, 250 is 25 V.
    let p = f.part("0402X105K6R3NT").unwrap().unwrap();
    assert_eq!(volts(&p), Some((-6.3, 6.3)));
    assert_eq!(p.temperature.as_ref().map(|t| t.max), Some(85.0));
    let p = f.part("0805DS106K250NT").unwrap().unwrap();
    assert_eq!(volts(&p), Some((-25.0, 25.0)));
    assert_eq!(p.temperature.as_ref().map(|t| t.max), Some(105.0));
    // A picofarad tolerance (C = +/-0.25 pF) gives no fraction.
    assert!(f.part("0402CG1R0C500NT").unwrap().unwrap().tolerance.is_none());
    // The datasheet's own examples (page 30): the two-letter dielectrics aren't read as B plus a value.
    for (mpn, code) in [
        ("0402BS104K250NT", "BS"),
        ("0402BT104K250NT", "BT"),
        ("0402DS104K250NT", "DS"),
        ("0402X104K250NT", "X"),
    ] {
        let d = f.decode(mpn).unwrap().unwrap();
        assert_eq!(d.codes["dielectric"], code, "{mpn}");
        assert_eq!(d.codes["value"], "104", "{mpn}");
    }
    // Not this family's: the code scheme goes above 50 V, but this datasheet's tables stop there; and the
    // termination must be N.
    assert!(f.decode("0603B104K101NT").is_none());
    assert!(f.decode("0603B104K500XT").is_none());
}

/// A family in a board's own parts folder replaces the built-in one; a part file anywhere beats a family.
#[test]
fn family_precedence() {
    let dir = std::env::temp_dir().join(format!("gerberus-family-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("parts/yageo-rc.toml"))
        .unwrap()
        .replace("family = \"Yageo RC_L\"", "family = \"Local RC\"");
    std::fs::write(dir.join("rc.toml"), text).unwrap();
    let mut lib = builtin();
    lib.add_dir(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let mut design = gerberus::design::Design::default();
    for (r, mpn) in [("R1", "RC0603FR-0747KL"), ("C1", "CL05B104KO5NNNC")] {
        let mut c = Component {
            reference: r.into(),
            ..Default::default()
        };
        c.fields.insert("MPN".into(), mpn.into());
        design.components.insert(r.into(), c);
    }
    lib.expand_families(&design);
    assert!(lib.parts["RC0603FR-0747KL"].description.contains("Local RC"));
    assert!(
        lib.files["CL05B104KO5NNNC"]
            .to_string_lossy()
            .ends_with("CL05B104KO5NNNC.toml (built in)")
    );
}

#[test]
fn a_family_part_is_looked_up_and_a_bad_one_says_why() {
    let mut lib = builtin();
    let mut design = gerberus::design::Design::default();
    for (r, mpn) in [
        ("R1", "RC0603FR-0747KL"),
        ("R2", "RC1210FR-7W10KL"),
        ("R3", "NOT-A-PART"),
    ] {
        let mut c = Component {
            reference: r.into(),
            value: "x".into(),
            lib_id: "Device:R".into(),
            in_bom: true,
            on_board: true,
            ..Default::default()
        };
        c.fields.insert("MPN".into(), mpn.into());
        design.components.insert(r.into(), c);
    }
    lib.expand_families(&design);
    assert!(matches!(lib.lookup(&design.components["R1"]), Lookup::Datasheet(p) if p.mpn == "RC0603FR-0747KL"));
    assert!(lib.files["RC0603FR-0747KL"].to_string_lossy().contains("yageo-rc.toml"));
    match lib.lookup(&design.components["R2"]) {
        Lookup::Unchecked(why) => assert!(why.contains("doesn't list RC1210FR-7W10KL"), "{why}"),
        _ => panic!("R2 should be unchecked"),
    }
    match lib.lookup(&design.components["R3"]) {
        Lookup::Unchecked(why) => assert!(why.contains("no part file"), "{why}"),
        _ => panic!("R3 should be unchecked"),
    }
}

/// Where a part file written by hand from a datasheet covers a part number a family also decodes, the two
/// must agree on every rating they both give.
#[test]
fn families_agree_with_hand_written_part_files() {
    let lib = builtin();
    let voltage = |p: &gerberus::limits::Part| {
        p.limits
            .iter()
            .find(|l| l.quantity == Quantity::Voltage)
            .and_then(|l| l.recommended.values())
    };
    let mut compared = 0;
    for (mpn, by_hand) in &lib.parts {
        for (_, f) in &lib.families {
            let Some(made) = f.part(mpn) else { continue };
            let made = made.unwrap_or_else(|e| panic!("{mpn}: {e}"));
            compared += 1;
            assert_eq!(voltage(&made), voltage(by_hand), "{mpn}: rated voltage");
            let tol = |p: &gerberus::limits::Part| p.tolerance.as_ref().map(|t| t.value);
            if tol(&made).is_some() {
                assert_eq!(tol(&made), tol(by_hand), "{mpn}: tolerance");
            }
            let temp = |p: &gerberus::limits::Part| p.temperature.as_ref().map(|t| (t.min, t.max));
            assert_eq!(temp(&made), temp(by_hand), "{mpn}: temperature");
            let power = |p: &gerberus::limits::Part| {
                p.limits
                    .iter()
                    .find(|l| l.quantity == Quantity::Power)
                    .and_then(|l| l.recommended.values())
            };
            if power(&made).is_some() {
                assert_eq!(power(&made), power(by_hand), "{mpn}: power");
            }
            let tcr = |p: &gerberus::limits::Part| p.tcr.as_ref().map(|t| t.ppm);
            if tcr(&made).is_some() {
                assert_eq!(tcr(&made), tcr(by_hand), "{mpn}: tcr");
            }
            let current = |p: &gerberus::limits::Part| {
                p.limits
                    .iter()
                    .find(|l| l.quantity == Quantity::Current)
                    .and_then(|l| l.recommended.values())
            };
            if current(&made).is_some() {
                assert_eq!(current(&made), current(by_hand), "{mpn}: rated current");
            }
            // Every rating it gives is cited; only an inductor's voltage limit is `none`.
            for l in &made.limits {
                let none_ok = made.class == Class::Inductor && l.quantity == Quantity::Voltage;
                assert!(
                    matches!(l.recommended, Range::Known { page: Some(_), .. })
                        || (none_ok && matches!(l.recommended, Range::None(_))),
                    "{mpn}: {:?}",
                    l.recommended
                );
            }
        }
    }
    // The six Samsung CL files, less CL05B104KB54PNC, whose codes 4 and P the 2015 catalogue doesn't list, and
    // the nine UNI-ROYAL files, the two Yageo CC, the one Fenghua and the one TDK MPZ1608 file.
    assert!(compared >= 18, "only {compared} parts compared");
}

/// A ferrite-bead family rates each part number its table lists, with no voltage rating, and refuses one the
/// table doesn't list.
#[test]
fn bead_families_rate_their_table_rows() {
    let lib = builtin();
    let blm = family(&lib, "Murata BLM18H");
    for (mpn, amps) in [
        ("BLM18HG601SN1D", 0.2),
        ("BLM18HE601SN1B", 0.8),
        ("BLM18HD102SN1D", 0.05),
    ] {
        let p = blm.part(mpn).unwrap().unwrap();
        assert_eq!(p.class, Class::Inductor, "{mpn}");
        let current = p.limits.iter().find(|l| l.quantity == Quantity::Current).unwrap();
        assert_eq!(current.recommended.values(), Some((-amps, amps)), "{mpn}");
        let voltage = p.limits.iter().find(|l| l.quantity == Quantity::Voltage).unwrap();
        assert!(matches!(voltage.recommended, Range::None(_)), "{mpn}");
        assert_eq!(
            p.temperature.as_ref().map(|t| (t.min, t.max)),
            Some((-55.0, 125.0)),
            "{mpn}"
        );
    }
    let err = blm.part("BLM18HE121SN1D").unwrap().unwrap_err();
    assert!(err.contains("the datasheet doesn't list BLM18HE121SN1D"), "{err}");
    let mpz = family(&lib, "TDK MPZ1608");
    let p = mpz.part("MPZ1608S260ATAH0").unwrap().unwrap();
    let current = p.limits.iter().find(|l| l.quantity == Quantity::Current).unwrap();
    assert_eq!(current.recommended.values(), Some((-6.0, 6.0)));
    assert!(mpz.part("MPZ1608S221BTA00").unwrap().is_err());
}
