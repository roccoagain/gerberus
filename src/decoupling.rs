//! Decoupling distance: is every power pin near a capacitor to ground on its net?
//!
//! A decoupling capacitor supplies a pin's fast current steps; the track between them adds inductance that
//! stops it doing so. Each power pin's pad is measured, centre to centre, to the nearest pad of a decoupling
//! capacitor whose other pin is on a ground net (0 V): within `policy.toml`'s `[decoupling] distance` passes.
//! A decoupling capacitor is one of at most `[decoupling] largest` farads; a bigger one is a bulk capacitor,
//! which covers slow load changes and needn't be close. Plug-in modules (`A1`) and connectors are left out: a module
//! decouples its own chips, and a connector's power pin feeds the board rather than drawing from it.
use crate::design::{Design, PinKind, is_capacitor};
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, Point, dist};
use crate::limits::Library;
use crate::policy::DecouplingPolicy;
use crate::units::parse_farads;
use crate::voltage::{Voltages, is_connector};

/// The shortest distance between any of `a` and any of `b`.
fn nearest(a: &[Point], b: &[Point]) -> Option<f64> {
    a.iter()
        .flat_map(|&p| b.iter().map(move |&q| dist(p, q)))
        .min_by(f64::total_cmp)
}

/// A capacitor by its distance from a pin: (distance, reference, value).
type Near<'a> = (f64, &'a str, &'a str);

/// The nearest of several.
fn closest<'a>(v: &[Near<'a>]) -> Option<Near<'a>> {
    v.iter().copied().min_by(|a, b| a.0.total_cmp(&b.0))
}

/// The `decoupling` findings: one per power pin on a net that isn't ground.
pub fn check(
    design: &Design,
    lib: &Library,
    volts: &Voltages,
    layout: &Layout,
    policy: &DecouplingPolicy,
) -> Vec<Finding> {
    let ground = |n: usize| {
        let s = volts.nets[n].span;
        s.min == Some(0.0) && s.max == Some(0.0)
    };
    let mut out = vec![];
    for (r, c) in &design.components {
        // A plug-in module (`A1`) carries its own decoupling next to its chips.
        let module = r.chars().take_while(char::is_ascii_alphabetic).collect::<String>() == "A";
        if c.dnp || !c.on_board || module || is_connector(lib, c) {
            continue;
        }
        for (number, pin) in &c.pins {
            if pin.kind != PinKind::PowerIn || ground(pin.net) {
                continue;
            }
            let net = &design.nets[pin.net].name;
            let at = format!("{r}.{number} ({net})");
            let finding = |verdict, message: String| Finding {
                rule: "decoupling",
                verdict,
                severity: Severity::Warning,
                reference: r.clone(),
                message: format!("{at}: {message}"),
                nets: vec![pin.net],
                case: None,
                headline: None,
            };
            let here = layout.pad_centres(r, number);
            if here.is_empty() {
                out.push(finding(Verdict::Unknown, "the pin has no pad on the board".into()));
                continue;
            }
            // Capacitors from this net to ground, by the distance to their pad on this net, split into
            // decoupling capacitors and the rest: bulk ones, or ones whose value can't be read.
            let (mut small, mut other): (Vec<Near<'_>>, Vec<Near<'_>>) = (vec![], vec![]);
            for (cr, cc) in &design.components {
                if cc.dnp || !is_capacitor(cr, cc) || cc.pins.len() != 2 || !cc.pins.values().any(|p| ground(p.net)) {
                    continue;
                }
                let Some((n, _)) = cc.pins.iter().find(|(_, p)| p.net == pin.net) else {
                    continue;
                };
                let Some(d) = nearest(&here, &layout.pad_centres(cr, n)) else {
                    continue;
                };
                // Within a part per billion, so `1000nF` counts as 1 µF.
                let decoupling = parse_farads(&cc.value).is_some_and(|f| f <= policy.largest * (1.0 + 1e-9));
                if decoupling { &mut small } else { &mut other }.push((d, cr, &cc.value));
            }
            let (small, other) = (closest(&small), closest(&other));
            let largest = policy.largest * 1e6;
            let finding = match (small, other) {
                (Some((d, cr, value)), _) if d <= policy.distance => finding(
                    Verdict::Pass,
                    format!(
                        "the nearest decoupling capacitor, {cr} ({value}), is {d:.1} mm away, pad centre to pad \
                         centre; within {} mm ({})",
                        policy.distance, policy.reason
                    ),
                ),
                // A capacitor above `largest`, or of unknown value, sits closer: a large ceramic decouples as well,
                // a bulk electrolytic doesn't, and which this is isn't known.
                (_, Some((d, cr, value))) if d <= policy.distance => finding(
                    Verdict::Unknown,
                    format!(
                        "the nearest capacitor to ground, {cr} ({value}), is {d:.1} mm away but isn't known to be \
                         {largest} µF or less, so whether it decouples the pin depends on its type{}",
                        small
                            .map(|(d, cr, _)| format!("; the nearest that is, {cr}, is {d:.1} mm away"))
                            .unwrap_or_default()
                    ),
                ),
                (Some((d, cr, value)), _) => finding(
                    Verdict::Fail,
                    format!(
                        "the nearest decoupling capacitor, {cr} ({value}), is {d:.1} mm away, pad centre to pad \
                         centre; more than {} mm ({})",
                        policy.distance, policy.reason
                    ),
                ),
                (None, _) => finding(
                    Verdict::Unknown,
                    format!("no placed decoupling capacitor ({largest} µF or less) joins {net} to ground"),
                ),
            };
            out.push(finding);
        }
    }
    out
}
