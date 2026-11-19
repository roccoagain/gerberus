use std::path::{Path, PathBuf};
use std::process::Command;

use gerberus::design::Design;
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::{Class, Library, Part, Range};
use gerberus::part_new::{NOT_READ, template};

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn can_encoder() -> Design {
    kicad::load(&root("tests/fixtures/can-encoder/can-encoder.kicad_sch")).unwrap()
}

fn part(design: &Design, which: &str) -> (String, Part) {
    let (mpn, text) = template(design, which, "can-encoder.kicad_sch").unwrap();
    let part: Part = toml::from_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert!(part.problems().is_empty(), "{:?}\n{text}", part.problems());
    assert!(text.contains(NOT_READ));
    (mpn, part)
}

fn unread(r: &Range) -> bool {
    matches!(r, Range::Unknown(why) if why == NOT_READ)
}

#[test]
fn an_ic_gets_its_pins_and_one_unread_limit_from_ground() {
    let d = can_encoder();
    let (mpn, p) = part(&d, "U4");
    assert_eq!(mpn, "TCAN1044AVDDFRQ1");
    assert_eq!(p.class, Class::Ic);
    assert_eq!(p.pins["2"], "GND");
    assert_eq!(p.pins.len(), d.components["U4"].pins.len());
    assert_eq!(p.limits.len(), 1);
    assert_eq!(p.limits[0].to.as_deref(), Some("GND"));
    assert!(!p.limits[0].pins.contains(&"GND".to_string()));
    assert!(unread(&p.limits[0].abs_max) && unread(&p.limits[0].recommended));
    assert_eq!(part(&d, "TCAN1044AVDDFRQ1").0, mpn);
}

#[test]
fn passives_diodes_and_connectors_get_the_limit_their_class_takes() {
    let d = can_encoder();
    let (_, r) = part(&d, "R1");
    assert_eq!(r.class, Class::Resistor);
    assert_eq!(r.limits[0].pins.len(), 2);
    assert!(r.limits[0].to.is_none());
    let (_, j) = part(&d, "J1");
    assert_eq!(j.class, Class::Connector);
    assert!(j.limits[0].to.is_none() && j.limits[0].pins.len() >= 2);
    let (_, diode) = part(&d, "D1");
    assert_eq!(diode.limits[0].pins, ["K"]);
    assert_eq!(diode.limits[0].to.as_deref(), Some("A"));
}

#[test]
fn an_unknown_part_is_an_error() {
    assert!(template(&can_encoder(), "U99", "x").is_err());
}

#[test]
fn doctor_lists_a_template_to_fill_in() {
    let dir = root("tests/fixtures/can-encoder");
    let design = can_encoder();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    let (mpn, p) = part(&design, "U4");
    lib.parts.insert(mpn.clone(), p);
    lib.files
        .insert(mpn.clone(), PathBuf::from("board/parts/TCAN1044AVDDFRQ1.toml"));
    let policy = gerberus::policy::Policy::load(&root("policy.toml")).unwrap();
    let v = gerberus::voltage::compute(&design, &lib, &inputs);
    let report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.reference == "U4" && f.message.contains(NOT_READ))
    );
    let setup = gerberus::doctor::Setup {
        inputs: dir.join("inputs.toml"),
        inputs_missing: false,
        parts: PathBuf::from("board/parts"),
        design: "board/board.kicad_sch".into(),
    };
    let d = gerberus::doctor::diagnose(&design, &lib, &setup, &inputs, &report, &v);
    let item = d.items.iter().find(|i| i.rule == "fill-in").expect("fill-in item");
    assert_eq!(item.references, ["U4"]);
    assert!(
        item.help.contains("datasheet verify board/parts/TCAN1044AVDDFRQ1.toml"),
        "{}",
        item.help
    );
}

#[test]
fn command_writes_the_file_once() {
    let dir = std::env::temp_dir().join(format!("gerberus-part-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        root("tests/fixtures/can-encoder/can-encoder.kicad_sch"),
        dir.join("can-encoder.kicad_sch"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_gerberus"))
            .args(["part", "new", "can-encoder.kicad_sch"])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    let out = run(&["U4"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("already has a part file"));
    let out = run(&["U4", "--force"]);
    assert!(out.status.success(), "{out:?}");
    let file = dir.join("parts/TCAN1044AVDDFRQ1.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains(NOT_READ));
    let out = run(&["U4"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unread_tvs_clamps_nothing() {
    let dir = root("tests/fixtures/kitchen-sink");
    let design = kicad::load(&dir.join("kitchen-sink.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    let (mpn, text) = template(&design, "D6", "kitchen-sink.kicad_sch").unwrap();
    let mut p: Part = toml::from_str(&text).unwrap();
    p.class = Class::Tvs;
    assert!(p.unread());
    lib.parts.insert(mpn, p);
    let policy = gerberus::policy::Policy::load(&root("policy.toml")).unwrap();
    let v = gerberus::voltage::compute(&design, &lib, &inputs);
    let report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    let esd: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.rule == "esd" && f.message.starts_with("J4.1"))
        .collect();
    assert!(!esd.is_empty());
    assert!(
        esd.iter()
            .all(|f| f.verdict != gerberus::rules::Verdict::Pass && !f.message.contains("clamped by D6")),
        "{esd:?}"
    );

    let tmp = std::env::temp_dir().join(format!("gerberus-unread-{}.toml", std::process::id()));
    std::fs::write(&tmp, &text).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["datasheet", "verify"])
        .arg(&tmp)
        .args(["--pdf", "/dev/null"])
        .output()
        .unwrap();
    std::fs::remove_file(&tmp).unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains(NOT_READ));
}

#[test]
fn a_class_default_is_kept() {
    let dir = std::env::temp_dir().join(format!("gerberus-class-default-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::read_to_string(root("tests/fixtures/can-encoder/can-encoder.kicad_sch"))
        .unwrap()
        .replacen("\"CC0805KKX7R9BB225\"", "\"NO-SUCH-CAP\"", 1);
    std::fs::write(dir.join("board.kicad_sch"), text).unwrap();
    let run = |force: bool| {
        Command::new(env!("CARGO_BIN_EXE_gerberus"))
            .args(["part", "new", "board.kicad_sch", "NO-SUCH-CAP", "--stdout"])
            .args(if force { &["--force"][..] } else { &[][..] })
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    let out = run(false);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("already checked against the rating"));
    assert!(run(true).status.success());
    std::fs::remove_dir_all(&dir).unwrap();
}
