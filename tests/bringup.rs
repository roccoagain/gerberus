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
