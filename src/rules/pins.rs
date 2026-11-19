//! Rules for pins: inputs nothing drives, and regulators without headroom.
use crate::design::{Design, PinKind};
use crate::finding::{Finding, Severity, Verdict};
use crate::limits::{Class, Library, Part};
use crate::voltage::{Driver, Hold, Span, Voltages};
use std::collections::BTreeSet;

use super::Report;

/// Input and power pins on nets nothing drives.
pub(super) fn undriven_pins(design: &Design, lib: &Library, volts: &Voltages, r: &mut Report) {
    for (reference, c) in &design.components {
        if c.dnp || c.lib_id.starts_with("Connector") || lib.part_for(c).is_some_and(|p| p.class == Class::Connector) {
            continue;
        }
        for pin in c.pins.values() {
            // A stacked pin's pads are one pin: report it once, under its first number.
            if pin.no_connect || pin.stack.as_ref().is_some_and(|s| *s != pin.number) {
                continue;
            }
            let rule = match pin.kind {
                PinKind::Input => "floating-input",
                PinKind::PowerIn => "unpowered",
                _ => continue,
            };
            let nv = &volts.nets[pin.net];
            let net = &design.nets[pin.net].name;
            let name = if pin.name.is_empty() {
                pin.number.clone()
            } else {
                format!("{} ({})", pin.number, pin.name)
            };
            // A part with no part file may supply the net itself: a charge pump's output or an internal
            // regulator's pin that its symbol marks as a power input (the MAX9722A's PVSS).
            // Two or more of its own pins on the net (PVSS and SVSS tied) set this apart from a lone power pin
            // left unconnected, which stays a FAIL. The part must also take power from a driven rail on another
            // power pin: a part whose only supply net never reaches a source (a mistyped label, a VDDA island
            // with a forgotten ferrite) still FAILs, however many of its pins and decoupling caps sit on it.
            let own_pins = design.nets[pin.net]
                .pins
                .iter()
                .filter(|(r2, _)| r2 == reference)
                .filter_map(|(_, p2)| c.pins.get(p2))
                .map(|p2| p2.stack.as_ref().unwrap_or(&p2.number))
                .collect::<BTreeSet<_>>()
                .len();
            let powered_elsewhere = c.pins.values().any(|q| {
                q.kind == PinKind::PowerIn
                    && q.net != pin.net
                    && match volts.nets[q.net].driver {
                        // A driven rail of unknown voltage (a board with no inputs.toml) may well be powered.
                        Driver::Strong | Driver::Pulled => volts.nets[q.net].span.max.is_none_or(|v| v > 0.0),
                        Driver::Weak => true,
                        Driver::Floating => false,
                    }
            });
            let own_rail = lib.part_for(c).is_none_or(Part::unread)
                && own_pins >= 2
                && powered_elsewhere
                && design.nets[pin.net].pins.iter().all(|(r2, p2)| {
                    r2 == reference
                        || design
                            .components
                            .get(r2)
                            .and_then(|c2| c2.pins.get(p2))
                            .is_some_and(|p2| p2.kind == PinKind::Passive)
                });
            // An input reached only through a capacitor from a signal is AC-coupled. So is one AC-grounded through
            // a capacitor to a rail, when another input of the same part is AC-coupled (the other leg of a
            // differential input). Many amplifiers and codecs bias such inputs themselves; without a part file,
            // nothing says whether this one does. A lone input with only a decoupling capacitor stays a FAIL.
            let ac_coupled = lib.part_for(c).is_none_or(Part::unread)
                && pin.kind == PinKind::Input
                && design.only_capacitors_on(reference, pin.net)
                && (coupled_to_signal(design, volts, reference, pin.net)
                    || c.pins.values().any(|q| {
                        q.kind == PinKind::Input
                            && q.net != pin.net
                            && coupled_to_signal(design, volts, reference, q.net)
                    }));
            let (vd, msg) = match nv.driver {
                Driver::Floating if ac_coupled => (
                    Verdict::Unknown,
                    format!(
                        "{reference}.{name} is on {net}, which only capacitors reach: it is AC-coupled. {reference} \
                         has no part file read from its datasheet, so nothing says whether it biases this input itself; if its datasheet \
                         says it does, give it a `biases` drive"
                    ),
                ),
                Driver::Floating if own_rail => (
                    Verdict::Unknown,
                    format!(
                        "{reference}.{name} is on {net}, which only {reference}'s own pins and passive parts reach. \
                         {reference} has no part file read from its datasheet, so nothing says whether it supplies that net itself (a charge \
                         pump or internal regulator output)"
                    ),
                ),
                Driver::Floating => (
                    Verdict::Fail,
                    format!(
                        "{reference}.{name} is on {net}, and nothing drives that net: {}",
                        nv.why[0].text
                    ),
                ),
                Driver::Weak => (
                    Verdict::Unknown,
                    format!(
                        "{reference}.{name} is on {net}, which only open-drain or tri-state outputs, connector pins \
                         or parts gerberus can't model reach, so nothing proves it is driven"
                    ),
                ),
                Driver::Strong | Driver::Pulled => (
                    Verdict::Pass,
                    format!("{reference}.{name} is on {net}, which is driven ({})", nv.span),
                ),
            };
            r.findings.push(Finding {
                rule,
                verdict: vd,
                severity: Severity::Error,
                reference: reference.clone(),
                message: msg,
                nets: vec![pin.net],
                case: None,
                headline: None,
            });
        }
    }
}

/// Whether `net` (reached only through capacitors from `reference`) has a capacitor carrying a signal in: its
/// far side is on a driven net that isn't a fixed 0 V rail.
fn coupled_to_signal(design: &Design, volts: &Voltages, reference: &str, net: usize) -> bool {
    design.only_capacitors_on(reference, net)
        && design.others_on(reference, net).any(|(_, c2, p2)| {
            c2.pins.values().any(|q| {
                q.number != p2
                    && q.net != net
                    && volts.nets[q.net].driver != Driver::Floating
                    && volts.nets[q.net].span != Span::exact(0.0)
            })
        })
}

pub(super) fn regulator_headroom(design: &Design, volts: &Voltages, r: &mut Report) {
    let mut seen = BTreeSet::new();
    for reg in volts.regulators.iter().filter(|r| r.sets_output) {
        if !seen.insert((&reg.reference, &reg.pin)) {
            continue;
        }
        let out = &design.nets[reg.output].name;
        let (vd, text) = match &reg.hold {
            Hold::Held(t) => (Verdict::Pass, t),
            Hold::NotHeld(t) => (Verdict::Fail, t),
            Hold::Unknown(t) => (Verdict::Unknown, t),
        };
        r.findings.push(Finding {
            rule: "regulator-headroom",
            verdict: vd,
            severity: Severity::Warning,
            reference: reg.reference.clone(),
            message: format!("{} ({}) sets {out}: {text}", reg.reference, reg.mpn),
            nets: vec![reg.supply],
            case: None,
            headline: (vd == Verdict::Fail).then(|| format!("{} may not hold {out}: {text}", reg.reference)),
        });
    }
}
