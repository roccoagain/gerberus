//! Part families: decoding part numbers, and agreeing with the part files written by hand for the same parts.

use gerberus::design::Component;
use gerberus::family::{ValueKind, parse_value};
use gerberus::limits::{Library, Lookup, Quantity, Range};

fn builtin() -> Library {
    Library::builtin().unwrap()
}

fn family<'a>(lib: &'a Library, name: &str) -> &'a gerberus::family::Family {
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
}

#[test]
fn yageo_part_numbers_decode() {
    let lib = builtin();
    let f = family(&lib, "Yageo RC_L");
    let d = f.decode("RC0805FR-0710KL").unwrap();
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
        Lookup::Unchecked(why) => assert!(why.contains("no row gives") && why.contains("power"), "{why}"),
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
            assert!(matches!(made.limits[0].recommended, Range::Known { page: Some(_), .. }));
        }
    }
    // The six Samsung CL files, less CL05B104KB54PNC, whose codes 4 and P the 2015 catalogue doesn't list.
    assert!(compared >= 5, "only {compared} parts compared");
}
