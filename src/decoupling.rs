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
use crate::policy::DecouplingPolicy;
use crate::units::parse_farads;
use crate::voltage::Voltages;

/// A pad's centre on the board.
fn pad_at(layout: &Layout, reference: &str, number: &str) -> Option<Point> {
    layout
        .footprints
        .iter()
        .find(|f| f.reference == reference)?
        .pads
        .iter()
        .find(|p| p.number == number)
        .map(|p| p.at)
}

/// The `decoupling` findings: one per power pin on a net that isn't ground.
pub fn check(design: &Design, volts: &Voltages, layout: &Layout, policy: &DecouplingPolicy) -> Vec<Finding> {
    let ground = |n: usize| {
        let s = volts.nets[n].span;
        s.min == Some(0.0) && s.max == Some(0.0)
    };
    let mut out = vec![];
    for (r, c) in &design.components {
        // A plug-in module (`A1`) carries its own decoupling next to its chips.
        let module = r.chars().take_while(char::is_ascii_alphabetic).collect::<String>() == "A";
        if c.dnp || !c.on_board || module || c.lib_id.starts_with("Connector") {
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
            let Some(here) = pad_at(layout, r, number) else {
                out.push(finding(Verdict::Unknown, "the pin has no pad on the board".into()));
                continue;
            };
            // Capacitors from this net to ground, by the pad on this net.
            let nearest = design
                .components
                .iter()
                .filter(|(cr, cc)| {
                    !cc.dnp
                        && is_capacitor(cr, cc)
                        && parse_farads(&cc.value).is_some_and(|f| f <= policy.largest)
                        && cc.pins.len() == 2
                        && cc.pins.values().any(|p| p.net == pin.net)
                        && cc.pins.values().any(|p| ground(p.net))
                })
                .filter_map(|(cr, cc)| {
                    let (n, _) = cc.pins.iter().find(|(_, p)| p.net == pin.net)?;
                    Some((dist(here, pad_at(layout, cr, n)?), cr, cc))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((d, cr, cc)) = nearest else {
                out.push(finding(
                    Verdict::Unknown,
                    format!(
                        "no placed decoupling capacitor (at most {} µF) joins {net} to ground",
                        policy.largest * 1e6
                    ),
                ));
                continue;
            };
            let what = format!(
                "the nearest capacitor to ground, {cr} ({}), is {d:.1} mm away, pad centre to pad centre",
                cc.value
            );
            out.push(if d <= policy.distance {
                finding(
                    Verdict::Pass,
                    format!("{what}; within {} mm ({})", policy.distance, policy.reason),
                )
            } else {
                finding(
                    Verdict::Fail,
                    format!("{what}; more than {} mm ({})", policy.distance, policy.reason),
                )
            });
        }
    }
    out
}
