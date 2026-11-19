//! MOSFET channels: finding them, and deciding from the gate whether each is off, on or maybe on.
use crate::design::{Component, Design};
use crate::limits::{DriveKind, Library};
use std::collections::BTreeSet;

use super::{
    Channel, ChannelState, Driver, GATE_THRESHOLD_TAKEN_AS_ZERO_V_AT_ANY_TEMPERATURE, Voltages, part_pin_net, round,
};

/// The MOSFET channels in a design, all taken as possibly on until [`decide`] works them out. Only part files
/// with a `channel` drive count: with no datasheet, nothing says which way a transistor conducts.
pub(super) fn find_channels(design: &Design, lib: &Library) -> Vec<Channel> {
    let mut out = vec![];
    for (r, c) in &design.components {
        if c.dnp {
            continue;
        }
        let Some(part) = lib.part_for(c) else { continue };
        for d in part.drives.iter().filter(|d| d.kind == DriveKind::Channel) {
            let net = |name: &str| part_pin_net(c, part, name);
            let (Some(drain), Some(source)) = (net(&d.pin), net(&d.to)) else {
                continue;
            };
            let threshold = d.range.values();
            let on = d.on.as_ref().map(|o| o.vgs);
            // An N-channel part turns on with its gate above its source, a P-channel one below.
            let n_channel = match (threshold, on) {
                (Some((lo, _)), _) if lo > 0.0 => Some(true),
                (Some((_, hi)), _) if hi < 0.0 => Some(false),
                (None, Some(o)) => Some(o > 0.0),
                _ => None,
            };
            out.push(Channel {
                reference: r.clone(),
                drain,
                source,
                gate: d.gate.as_deref().and_then(net),
                state: ChannelState::Maybe,
                reason: "not worked out yet".into(),
                n_channel,
                on,
                cite: format!(
                    "{}{}: {}",
                    part.mpn,
                    part.datasheet
                        .as_ref()
                        .map(|x| format!(" {}", x.document))
                        .unwrap_or_default(),
                    d.on.as_ref().map_or_else(
                        || "no RDS(on) condition given".into(),
                        |o| format!("RDS(on) given at VGS = {} V (p{})", o.vgs, o.page)
                    )
                ),
            });
        }
    }
    out
}

/// The drain, source and gate nets of a MOSFET symbol: a `Transistor_FET` symbol, or a `Q` part, whose pins
/// are named G, D and S (or Gate, Drain and Source), and nothing else. Pins sharing a name must share a net.
pub fn fet_pins(c: &Component) -> Option<(usize, usize, usize)> {
    let letters: String = c.reference.chars().take_while(char::is_ascii_alphabetic).collect();
    if !(c.lib_id.starts_with("Transistor_FET") || letters == "Q") {
        return None;
    }
    let role = |names: [&str; 2]| -> Option<usize> {
        let nets: BTreeSet<usize> = c
            .pins
            .values()
            .filter(|p| names.iter().any(|n| p.name.eq_ignore_ascii_case(n)))
            .map(|p| p.net)
            .collect();
        (nets.len() == 1).then(|| *nets.iter().next().expect("one net"))
    };
    let (d, s, g) = (role(["D", "Drain"])?, role(["S", "Source"])?, role(["G", "Gate"])?);
    // Every pin is one of the three, so this is a single MOSFET, not a dual one or an IC.
    c.pins
        .values()
        .all(|p| {
            ["D", "Drain", "S", "Source", "G", "Gate"]
                .iter()
                .any(|n| p.name.eq_ignore_ascii_case(n))
        })
        .then_some((d, s, g))
}

/// Whether a channel conducts, from the gate, drain and source ranges in `v`. Either end of the channel can act
/// as its source (the lower one, on an N-channel part), so the gate is measured against both.
///
/// Off needs the gate never to pass either end in the direction that turns the part on: at 0 V an enhancement
/// MOSFET is off at any temperature, where the datasheet's VGS(th) minimum holds only at 25 °C. Fully on needs
/// the gate always past `on` from one end, taken separately: the end with the larger worst case.
pub(super) fn decide(design: &Design, v: &Voltages, ch: &Channel) -> (ChannelState, String) {
    let maybe = |why: String| (ChannelState::Maybe, why);
    let r = &ch.reference;
    let Some(n) = ch.n_channel else {
        return maybe(format!(
            "{r}'s part file gives neither a gate threshold nor the VGS RDS(on) is given at, so which way it turns on isn't known"
        ));
    };
    let Some(gate) = ch.gate else {
        return maybe(format!("{r}'s gate isn't connected, so the channel may be on"));
    };
    let gv = &v.nets[gate];
    if matches!(gv.driver, Driver::Weak | Driver::Floating) {
        return maybe(format!(
            "nothing is sure to drive {r}'s gate ({}), so the channel may be on",
            design.nets[gate].name
        ));
    }
    let g = gv.span;
    // The gate from each end: its lowest and highest. A gate on that end sits at 0 V from it.
    let mut from = vec![];
    for end in [ch.source, ch.drain] {
        if gate == end {
            from.push((0.0, 0.0));
            continue;
        }
        let e = v.nets[end].span;
        let (Some(gmin), Some(gmax), Some(emin), Some(emax)) = (g.min, g.max, e.min, e.max) else {
            return maybe(format!(
                "{r}'s gate ({}) or channel ({}) range is UNKNOWN, so the channel may be on",
                design.nets[gate].name, design.nets[end].name
            ));
        };
        from.push((gmin - emax, gmax - emin));
    }
    let lo = from.iter().map(|f| f.0).fold(f64::INFINITY, f64::min);
    let hi = from.iter().map(|f| f.1).fold(f64::NEG_INFINITY, f64::max);
    let vgs = format!(
        "the gate ({}, {g}) is {} to {} V from either end of the channel ({} and {})",
        design.nets[gate].name,
        round(lo),
        round(hi),
        design.nets[ch.source].name,
        design.nets[ch.drain].name
    );
    let cite = &ch.cite;
    // Off: the gate never passes either end. Fully on: the gate is always past `on` from the end that acts as the
    // source (the lower one on an N-channel part, the higher on a P-channel one), so from at least one end.
    let (off, on) = if n {
        (
            hi <= GATE_THRESHOLD_TAKEN_AS_ZERO_V_AT_ANY_TEMPERATURE,
            ch.on
                .is_some_and(|o| from.iter().map(|f| f.0).fold(f64::NEG_INFINITY, f64::max) >= o),
        )
    } else {
        (
            lo >= GATE_THRESHOLD_TAKEN_AS_ZERO_V_AT_ANY_TEMPERATURE,
            ch.on
                .is_some_and(|o| from.iter().map(|f| f.1).fold(f64::INFINITY, f64::min) <= o),
        )
    };
    if off {
        return (
            ChannelState::Off,
            format!(
                "{vgs}, never {} it, so this {}-channel part can't turn on ({cite})",
                if n { "above" } else { "below" },
                if n { "N" } else { "P" }
            ),
        );
    }
    if on {
        return (
            ChannelState::On,
            format!(
                "{vgs}, always past the {} V RDS(on) is given at ({cite})",
                ch.on.unwrap_or_default()
            ),
        );
    }
    maybe(format!("{vgs}, so the channel may be on ({cite})"))
}
