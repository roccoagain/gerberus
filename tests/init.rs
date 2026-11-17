//! `gerberus init`: a starter inputs.toml guessed from net names, which loads as written and says it is a guess.

use std::path::{Path, PathBuf};
use std::process::Command;

use gerberus::inputs::{BoardInputs, Max};
use gerberus::kicad;
use gerberus::limits::Library;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn kitchen_sink() -> (gerberus::design::Design, BoardInputs, String) {
    let design = kicad::load(&root("tests/fixtures/kitchen-sink/kitchen-sink.kicad_sch")).unwrap();
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    lib.expand_families(&design);
    let text = gerberus::init::template(&design, &lib, "kitchen-sink");
    let inputs: BoardInputs = toml::from_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert!(inputs.problems(&design).is_empty(), "{:?}", inputs.problems(&design));
    (design, inputs, text)
}

#[test]
fn usb_pins_get_usb_ranges_and_a_short_to_vbus() {
    let (_, inputs, _) = kitchen_sink();
    let at = |pin: &str, fault: bool| {
        inputs
            .inputs
            .iter()
            .find(|i| i.pin.to_string() == pin && i.fault.is_some() == fault)
            .unwrap_or_else(|| panic!("no input on {pin}"))
    };
    let vbus = at("J5.A4", false);
    assert_eq!((vbus.min, vbus.max.clone()), (4.0, Max::Value(5.5)));
    assert!(vbus.source.starts_with(gerberus::init::GUESS));
    let dp = at("J5.A6", false);
    assert_eq!(dp.max, Max::Value(3.6));
    let short = at("J5.A6", true);
    assert_eq!(short.max, Max::Value(5.5));
    assert_eq!(short.fault.as_deref(), Some("J5 USB data shorted to VBUS"));
    // Every entry is measured from a ground pin.
    for i in &inputs.inputs {
        assert_ne!(i.pin, i.to);
    }
}

#[test]
fn nets_the_board_drives_and_unnamed_signals() {
    let (_, inputs, text) = kitchen_sink();
    // J3.1 is on +3V3, which the board's own regulator makes.
    assert!(inputs.undriven.iter().any(|u| u.pin.to_string() == "J3.1"), "{text}");
    // A UART pin's name says nothing about its voltage: it is left for a person, in a comment.
    assert!(text.contains("# J4.1 (/mcu/UART_TX): what drives it?"), "{text}");
    assert!(!inputs.inputs.iter().any(|i| i.pin.to_string() == "J4.1"));
    // Each guessed pin is listed once, though a USB data pin has a fault-case entry too.
    let guesses = gerberus::init::guesses(&inputs);
    assert_eq!(
        guesses.len(),
        inputs.inputs.iter().filter(|i| i.fault.is_none()).count() + inputs.undriven.len()
    );
}

/// The command writes inputs.toml once; `check` then says its results rest on guesses, and `doctor` lists them
/// to confirm first.
#[test]
fn command_writes_and_check_says_guesses() {
    let dir = std::env::temp_dir().join(format!("gerberus-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        root("tests/fixtures/can-encoder/can-encoder.kicad_sch"),
        dir.join("can-encoder.kicad_sch"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_gerberus"))
            .args(args)
            .current_dir(&dir)
            .env("GERBERUS_NO_UPDATE_CHECK", "1")
            .output()
            .unwrap()
    };
    let out = run(&["init", "can-encoder.kicad_sch"]);
    assert!(out.status.success(), "{out:?}");
    assert!(dir.join("inputs.toml").exists());
    let again = run(&["init", "can-encoder.kicad_sch"]);
    assert_eq!(again.status.code(), Some(2));
    let check = run(&["check", "can-encoder.kicad_sch"]);
    assert!(
        String::from_utf8_lossy(&check.stderr).contains("still guesses from `gerberus init`"),
        "{check:?}"
    );
    let doctor = run(&["doctor", "can-encoder.kicad_sch", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(v["items"][0]["rule"], "guess");
    std::fs::remove_dir_all(&dir).unwrap();
}
