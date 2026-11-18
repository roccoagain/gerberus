use std::path::{Path, PathBuf};

use gerberus::design::Design;
use gerberus::diag::Show;
use gerberus::doctor::{Doctor, Setup, diagnose, render};
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::policy::Policy;
use gerberus::rules::check;
use gerberus::voltage::compute;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn doctor(design: &Design, lib: &Library, inputs: &BoardInputs, missing: bool) -> Doctor {
    let policy = Policy::load(&root("policy.toml")).unwrap();
    let v = compute(design, lib, inputs);
    let report = check(design, lib, &policy, inputs, &v);
    let setup = Setup {
        inputs: PathBuf::from("board/inputs.toml"),
        inputs_missing: missing,
        parts: PathBuf::from("board/parts"),
        design: "board/board.kicad_sch".into(),
    };
    let d = diagnose(design, lib, &setup, inputs, &report, &v);
    assert_eq!(d.unknown, report.unknowns());
    assert!(d.settled <= d.unknown);
    d
}

fn can_encoder() -> (Design, BoardInputs) {
    let dir = root("tests/fixtures/can-encoder");
    (
        kicad::load(&dir.join("can-encoder.kicad_sch")).unwrap(),
        BoardInputs::load(&dir.join("inputs.toml")).unwrap(),
    )
}

#[test]
fn missing_inputs_come_first() {
    let (design, _) = can_encoder();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let inputs = BoardInputs::none("can-encoder", &design);
    let d = doctor(&design, &lib, &inputs, true);
    let first = &d.items[0];
    assert_eq!(first.rule, "inputs");
    assert_eq!(first.file, "board/inputs.toml");
    assert_eq!(first.references, ["J1", "J2", "J3"]);
    assert!(first.pins.contains(&"J1.1".to_string()), "{:?}", first.pins);
    assert!(first.unknown > 100, "{}", first.unknown);
    assert_eq!(d.items.iter().filter(|i| i.rule == "inputs").count(), 1);
    assert!(d.settled * 2 > d.unknown, "{} of {}", d.settled, d.unknown);
}

#[test]
fn missing_part_file_is_listed_with_its_references() {
    let (design, inputs) = can_encoder();
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    lib.parts.remove("TCAN1044AVDDFRQ1");
    lib.files.remove("TCAN1044AVDDFRQ1");
    let d = doctor(&design, &lib, &inputs, false);
    let item = d
        .items
        .iter()
        .find(|i| i.mpn.as_deref() == Some("TCAN1044AVDDFRQ1"))
        .expect("TCAN1044AVDDFRQ1 is listed");
    assert_eq!(item.rule, "part-file");
    assert_eq!(item.references, ["U4"]);
    assert_eq!(
        item.file,
        Path::new("board/parts")
            .join("TCAN1044AVDDFRQ1.toml")
            .display()
            .to_string()
    );
    assert!(item.unknown >= 1);
    let pdf = Path::new("board")
        .join("datasheets")
        .join("TCAN1044AVDDFRQ1.pdf")
        .display()
        .to_string();
    assert!(item.notes.iter().any(|n| n.contains(&pdf)), "{:?}", item.notes);
    assert!(
        item.title.contains("write a part file for TCAN1044AVDDFRQ1"),
        "{}",
        item.title
    );
    assert!(d.items.iter().all(|i| i.rule != "inputs"), "{:?}", d.items);
    assert!(d.items.windows(2).all(|w| w[0].unknown >= w[1].unknown));
}

#[test]
fn kitchen_sink_gaps_in_part_files() {
    let dir = root("tests/fixtures/kitchen-sink");
    let design = kicad::load(&dir.join("kitchen-sink.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let d = doctor(&design, &lib, &inputs, false);
    let pins = d.items.iter().find(|i| i.rule == "pin-limits").expect("pin limits");
    assert_eq!(pins.references, ["L1"]);
    assert_eq!(pins.pins, ["1", "2"]);
    assert!(d.items.iter().all(|i| i.rule != "drive"), "{:?}", d.items);
    assert!(
        d.items.iter().all(|i| i.references.iter().all(|r| !r.starts_with('Q'))),
        "{:?}",
        d.items
    );
    let text = render(&d, &Show::everything());
    assert!(
        text.contains("todo[pin-limits]: give FNR4030S6R8MT a voltage limit for 1 and 2"),
        "{text}"
    );
    assert!(text.contains("wait on things doctor doesn't trace yet"), "{text}");
}

#[test]
fn nothing_to_add() {
    let dir = root("tests/fixtures/sniffer");
    let design = kicad::load(&dir.join("sniffer.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let d = doctor(&design, &lib, &inputs, false);
    assert!(d.items.is_empty(), "{:?}", d.items);
    assert!(render(&d, &Show::everything()).starts_with("ok: nothing to add"));
}

#[test]
fn missing_inputs_are_always_listed() {
    let design = kicad::load(&root("tests/fixtures/hier/root.kicad_sch")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let inputs = BoardInputs::none("hier", &design);
    let d = doctor(&design, &lib, &inputs, true);
    let item = d.items.iter().find(|i| i.rule == "inputs").expect("inputs item");
    assert!(item.title.contains("aren't checked without it"), "{}", item.title);
    assert!(!render(&d, &Show::everything()).contains("check knows every range"));
}

#[test]
fn command_prints_json() {
    let dir = std::env::temp_dir().join(format!("gerberus-doctor-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        root("tests/fixtures/can-encoder/can-encoder.kicad_sch"),
        dir.join("can-encoder.kicad_sch"),
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["doctor", "can-encoder.kicad_sch", "--json"])
        .current_dir(&dir)
        .env("GERBERUS_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["items"][0]["rule"], "inputs");
    assert_eq!(v["items"][0]["file"], "inputs.toml");
    assert!(v["settled"].as_u64().unwrap() <= v["unknown"].as_u64().unwrap());
}
