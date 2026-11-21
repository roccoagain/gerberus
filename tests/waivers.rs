//! `waivers.toml`: accepted findings leave the counts with their reason, and a waiver that matches nothing is a
//! warning.
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use gerberus::diag::Show;
use gerberus::finding::{Severity, Verdict};
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::policy::Policy;
use gerberus::rules::{check, render};
use gerberus::voltage::compute;
use gerberus::waivers::Waivers;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

#[test]
fn a_waiver_takes_its_findings_out_of_the_counts() {
    let dir = root("tests/fixtures/kitchen-sink");
    let design = kicad::load(&dir.join("kitchen-sink.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&design, &lib, &inputs);
    let report = check(&design, &lib, &Policy::builtin().unwrap(), &inputs, &v);
    let target = report
        .findings
        .iter()
        .find(|f| f.verdict != Verdict::Pass && f.case.is_none())
        .expect("the kitchen sink has a failed or unknown finding")
        .clone();
    let before = (report.errors(), report.warnings(), report.unknowns());
    let waivers: Waivers = toml::from_str(&format!(
        "[[waiver]]\nrule = {:?}\nreference = {:?}\ncontains = {:?}\nreason = \"checked by hand\"\nby = \"test\"\n\n\
         [[waiver]]\nrule = \"esd\"\nreference = {:?}\ncontains = \"no such pin\"\nreason = \"stale\"\n",
        target.rule,
        target.reference,
        &target.message[..target.message.len().min(30)],
        target.reference
    ))
    .unwrap();
    assert!(waivers.problems(&design).is_empty());
    let mut waived = report.clone();
    waivers.apply(&mut waived, "waivers.toml");
    assert!(
        waived
            .waived
            .iter()
            .any(|w| w.finding == target && w.reason == "checked by hand")
    );
    assert!(!waived.findings.contains(&target));
    let after = (waived.errors(), waived.warnings(), waived.unknowns());
    let gone = waived.waived.len();
    assert_eq!(
        before.0 + before.1 + before.2 + 1,
        after.0 + after.1 + after.2 + gone,
        "the unused waiver adds one warning"
    );
    // The second waiver matched nothing: a warning names it.
    assert!(
        waived
            .findings
            .iter()
            .any(|f| f.rule == "waiver" && f.message.contains("no such pin"))
    );
    // JSON carries the waived findings with their reasons.
    let json = serde_json::to_string(&waived).unwrap();
    assert!(json.contains("\"waived\"") && json.contains("checked by hand"));
}

#[test]
fn bad_waivers_are_refused() {
    let design = kicad::load(&root("tests/fixtures/can-encoder/can-encoder.kicad_sch")).unwrap();
    let w: Waivers = toml::from_str("[[waiver]]\nrule = \"esd\"\nreference = \"U99\"\nreason = \" \"\n").unwrap();
    let p = w.problems(&design);
    assert_eq!(p.len(), 2, "{p:?}");
    assert!(toml::from_str::<Waivers>("[[waiver]]\nrule = \"esd\"\nreference = \"U4\"\n").is_err());
}

/// `check` reads waivers.toml next to the schematic, and --all lists what it waived.
#[test]
fn check_reads_the_file_next_to_the_schematic() {
    let dir = std::env::temp_dir().join(format!("gerberus-waivers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = root("tests/fixtures/kitchen-sink");
    for f in std::fs::read_dir(&src).unwrap() {
        let f = f.unwrap().path();
        if f.is_file() {
            std::fs::copy(&f, dir.join(f.file_name().unwrap())).unwrap();
        }
    }
    let run = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_gerberus"))
            .args(["check", "kitchen-sink.kicad_sch", "--json"])
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    let json: serde_json::Value = serde_json::from_slice(&run(&[]).stdout).unwrap();
    let f = json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["verdict"] != "pass" && f.get("case").is_none())
        .unwrap()
        .clone();
    std::fs::write(
        dir.join("waivers.toml"),
        format!(
            "[[waiver]]\nrule = {}\nreference = {}\nreason = \"accepted\"\n",
            f["rule"], f["reference"]
        ),
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&run(&[]).stdout).unwrap();
    assert!(!json["waived"].as_array().unwrap().is_empty());
    let out = Command::new(env!("CARGO_BIN_EXE_gerberus"))
        .args(["check", "kitchen-sink.kicad_sch", "--all"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("waived[") && text.contains("waived: accepted"), "{text}");
    // A waiver for a part the design doesn't have stops the check.
    std::fs::write(
        dir.join("waivers.toml"),
        "[[waiver]]\nrule = \"esd\"\nreference = \"U999\"\nreason = \"x\"\n",
    )
    .unwrap();
    assert_eq!(run(&[]).status.code(), Some(2));
    std::fs::remove_dir_all(&dir).unwrap();
}

fn kitchen_sink() -> (
    gerberus::design::Design,
    gerberus::voltage::Voltages,
    gerberus::rules::Report,
) {
    let dir = root("tests/fixtures/kitchen-sink");
    let design = kicad::load(&dir.join("kitchen-sink.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&design, &lib, &inputs);
    let report = check(&design, &lib, &Policy::builtin().unwrap(), &inputs, &v);
    (design, v, report)
}

/// Two waivers that match the same finding are both in use: neither is reported as matching nothing.
#[test]
fn overlapping_waivers_are_both_used() {
    let (design, v, report) = kitchen_sink();
    let target = report
        .findings
        .iter()
        .find(|f| f.verdict != Verdict::Pass && f.case.is_none())
        .unwrap()
        .clone();
    let waivers: Waivers = toml::from_str(&format!(
        "[[waiver]]\nrule = {:?}\nreference = {:?}\nreason = \"all of them\"\n\n\
         [[waiver]]\nrule = {:?}\nreference = {:?}\ncontains = {:?}\nreason = \"this one\"\n",
        target.rule,
        target.reference,
        target.rule,
        target.reference,
        &target.message[..target.message.len().min(30)],
    ))
    .unwrap();
    let mut waived = report.clone();
    waivers.apply(&mut waived, "custom.toml");
    assert!(
        !waived.findings.iter().any(|f| f.rule == "waiver"),
        "{:#?}",
        waived.findings
    );
    // The note names the file the waivers came from.
    let text = render(&design, &v, &waived, &Show::default());
    assert!(text.contains("waived in custom.toml"), "{text}");
}

/// A reverse-polarity summary that FAILs only because of its fault case's FAILs is waived with them.
#[test]
fn waiving_a_fault_cases_failures_waives_its_summary() {
    let (_, _, report) = kitchen_sink();
    let summary = report
        .findings
        .iter()
        .find(|f| f.rule == "reverse-polarity" && f.verdict == Verdict::Fail)
        .expect("the kitchen sink has a reverse-polarity FAIL")
        .clone();
    let case = summary.case.clone().unwrap();
    let mut toml_text = String::new();
    for f in report.findings.iter().filter(|f| {
        f.case.as_ref() == Some(&case)
            && f.rule != "reverse-polarity"
            && f.verdict == Verdict::Fail
            && f.severity == Severity::Error
    }) {
        writeln!(
            toml_text,
            "[[waiver]]\nrule = {:?}\nreference = {:?}\ncase = {case:?}\nreason = \"bench only\"\n",
            f.rule, f.reference
        )
        .unwrap();
    }
    let waivers: Waivers = toml::from_str(&toml_text).unwrap();
    let mut waived = report.clone();
    waivers.apply(&mut waived, "waivers.toml");
    assert!(!waived.findings.contains(&summary), "{toml_text}");
    assert!(waived.waived.iter().any(|w| w.finding == summary));
}

/// A waiver an agent proposed doesn't apply until a person deletes its `proposed` line: the finding still
/// counts, with a note that names who proposed it and why.
#[test]
fn a_proposed_waiver_waits_for_a_person() {
    let dir = root("tests/fixtures/kitchen-sink");
    let design = kicad::load(&dir.join("kitchen-sink.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&dir.join("inputs.toml")).unwrap();
    let lib = Library::load_dir(&root("parts")).unwrap();
    let v = compute(&design, &lib, &inputs);
    let report = check(&design, &lib, &Policy::builtin().unwrap(), &inputs, &v);
    let target = report
        .findings
        .iter()
        .find(|f| f.verdict != Verdict::Pass && f.case.is_none())
        .unwrap()
        .clone();
    let text = |proposed: &str| {
        format!(
            "[[waiver]]\nrule = {:?}\nreference = {:?}\nreason = \"only on the bench\"\n{proposed}",
            target.rule, target.reference
        )
    };
    let pending: Waivers = toml::from_str(&text("proposed = \"Claude, 2026-11-17\"\n")).unwrap();
    assert!(pending.problems(&design).is_empty());
    let mut r = report.clone();
    pending.apply(&mut r, "waivers.toml");
    assert!(r.waived.is_empty());
    assert_eq!(
        (r.errors(), r.warnings(), r.unknowns()),
        (report.errors(), report.warnings(), report.unknowns())
    );
    let still = r
        .findings
        .iter()
        .find(|f| f.rule == target.rule && f.reference == target.reference && f.verdict == target.verdict)
        .unwrap();
    assert!(
        still
            .message
            .contains("a waiver proposed by Claude, 2026-11-17 in waivers.toml is waiting for a person"),
        "{}",
        still.message
    );
    // Approved: the line is gone, and the finding leaves the counts.
    let approved: Waivers = toml::from_str(&text("by = \"Rocco\"\n")).unwrap();
    let mut r = report.clone();
    approved.apply(&mut r, "waivers.toml");
    assert!(!r.waived.is_empty());
    let empty: Waivers = toml::from_str(&text("proposed = \" \"\n")).unwrap();
    assert!(!empty.problems(&design).is_empty());
    // Both at once is a mistake: approving means deleting `proposed`.
    let both: Waivers = toml::from_str(&text("by = \"Rocco\"\nproposed = \"Claude\"\n")).unwrap();
    assert!(!both.problems(&design).is_empty());
}

/// `--allow RULE` silences that rule's warnings, as a compiler's `-A` does, and says how many; errors stay.
#[test]
fn allow_silences_warnings_but_not_errors() {
    let run = |args: &[&str]| -> (Option<i32>, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_gerberus"))
            .args(["check", "sniffer.kicad_sch"])
            .args(args)
            .current_dir(root("tests/fixtures/sniffer"))
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let (code, text) = run(&["--allow", "esd"]);
    assert_eq!(code, Some(1), "{text}");
    assert!(
        !text.contains("warning[esd]") && text.contains("warning[regulator-headroom]"),
        "{text}"
    );
    assert!(text.contains("2 warnings silenced by --allow esd"), "{text}");
    let (_, text) = run(&["--allow", "warnings", "--allow", "abs-max"]);
    assert!(
        !text.contains("\nwarning[") && text.contains("error[abs-max]"),
        "{text}"
    );
    assert!(text.contains("error: 2 errors, 0 warnings"), "{text}");
    assert_eq!(run(&["--allow"]).0, Some(2));
}
