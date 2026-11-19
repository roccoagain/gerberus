use gerberus::design::{Component, Design, Net, Pin, PinKind};
use gerberus::diff::{diff, render};
use gerberus::finding::{Finding, Severity, Verdict};
use gerberus::rules::Report;

type PartSpec<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

fn design(parts: &[PartSpec<'_>]) -> Design {
    let mut d = Design::default();
    for &(r, value, pins) in parts {
        let mut c = Component {
            reference: r.into(),
            value: value.into(),
            lib_id: "Device:R".into(),
            in_bom: true,
            on_board: true,
            ..Default::default()
        };
        for &(num, net) in pins {
            let i = d.nets.iter().position(|n| n.name == net).unwrap_or_else(|| {
                d.nets.push(Net {
                    name: net.into(),
                    pins: vec![],
                    labels: vec![],
                });
                d.nets.len() - 1
            });
            d.nets[i].pins.push((r.into(), num.into()));
            c.pins.insert(
                num.into(),
                Pin {
                    number: num.into(),
                    name: String::new(),
                    kind: PinKind::Passive,
                    no_connect: false,
                    net: i,
                    stack: None,
                },
            );
        }
        d.components.insert(r.into(), c);
    }
    d
}

fn finding(rule: &'static str, r: &str, verdict: Verdict, message: &str) -> Finding {
    Finding {
        rule,
        verdict,
        severity: Severity::Error,
        reference: r.into(),
        message: message.into(),
        nets: vec![],
        case: None,
        headline: None,
    }
}

fn report(f: Vec<Finding>) -> Report {
    Report {
        findings: f,
        ..Default::default()
    }
}

#[test]
fn parts_added_removed_and_changed() {
    let old = design(&[
        ("R1", "10k", &[("1", "A"), ("2", "B")]),
        ("R2", "1k", &[("1", "A"), ("2", "B")]),
    ]);
    let new = design(&[
        ("R1", "4k7", &[("1", "A"), ("2", "B")]),
        ("R3", "1k", &[("1", "A"), ("2", "B")]),
    ]);
    let d = diff(&old, &Report::default(), &new, &Report::default());
    let got: Vec<(&str, &str)> = d.parts.iter().map(|p| (p.reference.as_str(), p.kind)).collect();
    assert_eq!(got, [("R1", "changed"), ("R2", "removed"), ("R3", "added")]);
    assert_eq!(d.parts[0].what, ["value: 10k → 4k7"]);
}

#[test]
fn a_moved_pin_splits_one_net_and_joins_another() {
    let old = design(&[
        ("R1", "1k", &[("1", "A"), ("2", "B")]),
        ("R2", "1k", &[("1", "A"), ("2", "B")]),
        ("R3", "1k", &[("1", "A"), ("2", "C")]),
        ("R4", "1k", &[("1", "A"), ("2", "C")]),
    ]);
    let new = design(&[
        ("R1", "1k", &[("1", "VIN"), ("2", "B")]),
        ("R2", "1k", &[("1", "VIN"), ("2", "C")]),
        ("R3", "1k", &[("1", "VIN"), ("2", "C")]),
        ("R4", "1k", &[("1", "VIN"), ("2", "C")]),
    ]);
    let d = diff(&old, &Report::default(), &new, &Report::default());
    assert_eq!(d.nets.len(), 2, "{:?}", d.nets);
    assert_eq!((d.nets[0].kind, d.nets[0].net.as_str()), ("split", "B"));
    assert_eq!((d.nets[1].kind, d.nets[1].net.as_str()), ("joined", "C"));
    assert!(
        d.nets[1].groups.contains(&("B".into(), vec!["R2.2".into()])),
        "{:?}",
        d.nets[1]
    );
}

#[test]
fn findings_new_fixed_and_unchanged() {
    let d0 = design(&[]);
    let old = report(vec![
        finding("recommended", "U1", Verdict::Fail, "U1.VIN (+12V): reaches 14 V"),
        finding("recommended", "U2", Verdict::Pass, "U2.VIN (+5V): reaches 5.1 V"),
        finding("recommended", "U3", Verdict::Unknown, "U3.EN (EN): UNKNOWN"),
    ]);
    let new = report(vec![
        finding("recommended", "U1", Verdict::Pass, "U1.VIN (+12V): reaches 12.6 V"),
        finding("recommended", "U2", Verdict::Fail, "U2.VIN (+5V): reaches 6 V"),
        finding("recommended", "U3", Verdict::Unknown, "U3.EN (EN): UNKNOWN still"),
    ]);
    let d = diff(&d0, &old, &d0, &new);
    let got: Vec<(&str, Option<Verdict>, Option<Verdict>)> = d
        .findings
        .iter()
        .map(|f| (f.reference.as_str(), f.old, f.new))
        .collect();
    assert_eq!(
        got,
        [
            ("U2", Some(Verdict::Pass), Some(Verdict::Fail)),
            ("U1", Some(Verdict::Fail), Some(Verdict::Pass)),
        ]
    );
    assert_eq!(d.new_errors(), 1);
    let text = render(&d);
    assert!(text.contains("PASS → FAIL error[recommended] U2"), "{text}");
    assert!(text.contains("1 new failures, 1 fixed"), "{text}");
}

#[test]
fn nothing_changed() {
    let a = design(&[("R1", "1k", &[("1", "A"), ("2", "B")])]);
    let d = diff(&a, &Report::default(), &a, &Report::default());
    assert_eq!(render(&d), "No changes to parts, connections or findings.\n");
}

#[test]
fn a_second_fail_in_a_fault_case_is_new() {
    let d0 = design(&[]);
    let mut f1 = finding(
        "recommended",
        "U1",
        Verdict::Fail,
        "fault \"5V reversed\": U1.VIN (+5V): -5 V",
    );
    f1.case = Some("5V reversed".into());
    let mut f2 = finding(
        "recommended",
        "U1",
        Verdict::Fail,
        "fault \"5V reversed\": U1.EN (EN): -5 V",
    );
    f2.case = Some("5V reversed".into());
    let old = report(vec![f1.clone()]);
    let new = report(vec![f1, f2]);
    let d = diff(&d0, &old, &d0, &new);
    assert_eq!(d.new_errors(), 1, "{:?}", d.findings);
}

#[test]
fn a_new_waiver_shows() {
    let d0 = design(&[]);
    let f = finding("esd", "J1", Verdict::Fail, "J1.1 (SWDIO): no clamp");
    let old = report(vec![f.clone()]);
    let mut new = Report::default();
    new.waived.push(gerberus::finding::Waived {
        finding: f,
        reason: "bench only".into(),
        by: Some("Rocco".into()),
    });
    let d = diff(&d0, &old, &d0, &new);
    let text = render(&d);
    assert!(text.contains("FAIL → none (1 waived)"), "{text}");
    assert!(text.contains("1 newly waived"), "{text}");
    assert_eq!(d.new_errors(), 0);
}
