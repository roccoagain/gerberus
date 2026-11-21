//! The bring-up plan on the kitchen-sink and CAN encoder boards.
use std::path::{Path, PathBuf};

use gerberus::bringup::{plan, rails_by_name, render};
use gerberus::inputs::BoardInputs;
use gerberus::kicad;
use gerberus::limits::Library;
use gerberus::voltage::compute;

fn root(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn run(dir: &str, sch: &str) -> (gerberus::design::Design, gerberus::bringup::Plan) {
    let d = root("tests/fixtures").join(dir);
    let design = kicad::load(&d.join(sch)).unwrap();
    let inputs = BoardInputs::load(&d.join("inputs.toml")).unwrap();
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    lib.expand_families(&design);
    let v = compute(&design, &lib, &inputs);
    let th = gerberus::thermal::compute(&design, &lib, &inputs, &v);
    let p = plan(&design, &lib, &inputs, &v, &th);
    (design, p)
}

/// Each rail follows the one that feeds it: +VDC from VIN through the diode, +5V from the buck on +VDC, +3V3 from
/// the LDO on +5V.
#[test]
fn rails_come_in_the_order_power_flows() {
    let (d, p) = run("can-encoder", "can-encoder.kicad_sch");
    let first = &p.stages[0];
    let names: Vec<&str> = first.rails.iter().map(|r| d.nets[r.net].name.as_str()).collect();
    let pos = |n: &str| {
        names
            .iter()
            .position(|x| *x == n)
            .unwrap_or_else(|| panic!("{n} not in {names:?}"))
    };
    assert!(pos("+VDC") < pos("+5V"), "{names:?}");
    assert!(pos("+5V") < pos("+3V3"), "{names:?}");
    let rails = rails_by_name(&d, &p);
    assert_eq!(rails["+3V3"], gerberus::voltage::Span::new(3.234, 3.366));
}

/// The plan names the supply's range, flags a regulator that may not hold its output, and lists every rail for
/// the short check.
#[test]
fn the_plan_says_what_to_set_and_expect() {
    let (d, p) = run("kitchen-sink", "kitchen-sink.kicad_sch");
    let text = render(&d, &p);
    assert!(
        text.contains("Power VIN: connect a bench supply to J1.1 (+) to J1.4 (−), set inside 7 to 26 V"),
        "{text}"
    );
    // The ME6211 on VAUX is planted with too little headroom.
    assert!(text.contains("U3 may not hold it"), "{text}");
    assert!(text.contains("- +3V3 at"), "{text}");
    assert!(text.contains("Power one input at a time"), "{text}");
}

/// A fuse in front of the input diode doesn't hide the load behind it: VBUS still counts what is past the fuse,
/// and the rails past it are still in the plan.
#[test]
fn a_fuse_before_the_rails_hides_nothing() {
    use gerberus::design::{Component, Net, Pin, PinKind};
    let d0 = root("tests/fixtures/can-encoder");
    let mut d = kicad::load(&d0.join("can-encoder.kicad_sch")).unwrap();
    let inputs = BoardInputs::load(&d0.join("inputs.toml")).unwrap();
    let mut lib = Library::load_dir(&root("parts")).unwrap();
    lib.expand_families(&d);
    let summary = |d: &gerberus::design::Design| {
        let v = compute(d, &lib, &inputs);
        let th = gerberus::thermal::compute(d, &lib, &inputs, &v);
        let vbus = d.nets.iter().position(|n| n.name == "VBUS").unwrap();
        let p = plan(d, &lib, &inputs, &v, &th);
        (th.loads[vbus].missing.len(), rails_by_name(d, &p).len())
    };
    let before = summary(&d);
    assert!(before.0 > 0 && before.1 > 0, "{before:?}");
    // Move D1's anode onto a new net FUSED, and put F9 from VBUS to it.
    d.nets.push(Net {
        name: "FUSED".into(),
        pins: vec![],
        labels: vec![],
    });
    let fused = d.nets.len() - 1;
    let vbus = d.nets.iter().position(|n| n.name == "VBUS").unwrap();
    let anode = d.components["D1"]
        .pins
        .iter()
        .find(|(_, p)| p.net == vbus)
        .map(|(k, _)| k.clone())
        .unwrap();
    d.nets[vbus].pins.retain(|(r, p)| !(r == "D1" && *p == anode));
    d.nets[fused].pins.push(("D1".into(), anode.clone()));
    d.components.get_mut("D1").unwrap().pins.get_mut(&anode).unwrap().net = fused;
    let mut f = Component {
        reference: "F9".into(),
        value: "F".into(),
        lib_id: "Device:Fuse".into(),
        in_bom: true,
        on_board: true,
        ..Default::default()
    };
    for (num, net) in [("1", vbus), ("2", fused)] {
        f.pins.insert(
            num.into(),
            Pin {
                number: num.into(),
                name: String::new(),
                kind: PinKind::Passive,
                no_connect: false,
                stack: None,
                net,
            },
        );
        d.nets[net].pins.push(("F9".into(), num.into()));
    }
    d.components.insert("F9".into(), f);
    assert_eq!(summary(&d), before);
}

/// A rail whose regulator may not hold it is to be measured against the regulator's output range, not the 0 V
/// floor its low end has while the regulator may be out of regulation.
#[test]
fn an_unheld_rail_expects_the_regulated_range() {
    let (d, p) = run("sniffer", "sniffer.kicad_sch");
    let text = render(&d, &p);
    assert!(
        text.contains("Measure +3V3 (from +5V) at J2.1: expect 3.234 to 3.366 V."),
        "{text}"
    );
    assert!(text.contains("U1 may not hold it"), "{text}");
}
