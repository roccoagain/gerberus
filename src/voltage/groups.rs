//! Nets no drive reaches: the range of everything that can pull them, through resistors, inductors, LEDs, jumpers,
//! switches and IC outputs.
use crate::design::Component;
use crate::design::{Design, PinKind};
use crate::inputs::BoardInputs;
use crate::limits::{Class, Library};
use std::collections::{BTreeMap, BTreeSet};

use super::{
    Channel, ChannelState, Driver, NetVoltage, Span, Why, class_of, is_capacitor, is_connector, is_inductor_symbol,
    is_undriven, merge, short_kind, span_of, unknown,
};

/// Nets nothing drives directly, joined through resistors, inductors, LEDs and jumpers. They share one range:
/// the union of everything that can pull any of them.
pub(super) struct Group {
    pub(super) nets: Vec<usize>,
    pulls: Vec<Pull>,
    /// MOSFET channels on these nets that are always off, and why.
    off: Vec<String>,
    /// A capacitor or inductor touches the group, so it can leave the range of the nets around it.
    reactive: bool,
}

enum Pull {
    /// Through a two-pin part to a driven net.
    Net {
        via: String,
        net: usize,
    },
    /// Through a switch to a driven net: the net's range while the switch is closed. While it is open, nothing
    /// passes, so the switch alone doesn't prove the net is driven.
    Switch {
        via: String,
        net: usize,
    },
    /// An IC's output pin, which reaches at most the part's highest supply.
    Output {
        pin: String,
        supplies: Vec<usize>,
        /// A push-pull or bidirectional pin, which can drive both ways; open-drain and tri-state pins can't
        /// be relied on to.
        push_pull: bool,
    },
    /// Through a MOSFET channel that may be on, or is always on, to a driven net.
    Channel {
        via: String,
        net: usize,
        gate: usize,
        /// N-channel: it can raise this net only while the gate is above it. P-channel: lower it only while the
        /// gate is below it.
        n_channel: bool,
        state: ChannelState,
        reason: String,
    },
    Unknown(String),
}

impl Group {
    pub(super) fn deps(&self) -> Vec<usize> {
        self.pulls
            .iter()
            .flat_map(|p| match p {
                Pull::Net { net, .. } | Pull::Switch { net, .. } => vec![*net],
                Pull::Channel { net, gate, .. } => vec![*net, *gate],
                Pull::Output { supplies, .. } => supplies.clone(),
                Pull::Unknown(_) => vec![],
            })
            .collect()
    }

    pub(super) fn eval(&self, design: &Design, out: &[Option<NetVoltage>]) -> NetVoltage {
        let name = |n: usize| design.nets[n].name.as_str();
        if self.pulls.is_empty() {
            let pins: Vec<String> = self
                .nets
                .iter()
                .flat_map(|&n| design.nets[n].pins.iter().map(|(r, p)| format!("{r}.{p}")))
                .collect();
            let text = if !self.off.is_empty() {
                format!(
                    "UNKNOWN: nothing drives it while {} off: {}",
                    if self.off.len() == 1 {
                        "this channel is"
                    } else {
                        "these channels are"
                    },
                    self.off.join("; ")
                )
            } else if pins.len() <= 1 {
                format!("UNKNOWN: only {} is on this net, so nothing sets it", pins.join(""))
            } else {
                format!(
                    "UNKNOWN: nothing drives it: no input, regulator, output pin or resistor path reaches {}",
                    pins.join(", ")
                )
            };
            let mut nv = unknown(text, vec![]);
            nv.driver = Driver::Floating;
            return nv;
        }
        let mut nv: Option<NetVoltage> = None;
        let shared = if self.nets.len() > 1 {
            let names: Vec<&str> = self.nets.iter().map(|&n| name(n)).collect();
            format!(" (shared by {})", names.join(", "))
        } else {
            String::new()
        };
        for p in &self.pulls {
            let (span, why) = match p {
                Pull::Net { via, net } => (
                    span_of(out, *net),
                    Why {
                        text: format!("pulled through {via} toward {}{shared}", name(*net)),
                        from: vec![*net],
                        bounded: true,
                        low_floor: out[*net].as_ref().is_some_and(|nv| nv.floor),
                    },
                ),
                Pull::Switch { via, net } => (
                    span_of(out, *net),
                    Why {
                        text: format!(
                            "through switch {via} from {} while it is closed; while it is open, nothing passes{shared}",
                            name(*net)
                        ),
                        from: vec![*net],
                        bounded: true,
                        low_floor: out[*net].as_ref().is_some_and(|nv| nv.floor),
                    },
                ),
                Pull::Output { pin, supplies, .. } => {
                    let tops: Vec<Option<f64>> = supplies.iter().map(|&s| span_of(out, s).max).collect();
                    let top = tops.iter().try_fold(f64::NEG_INFINITY, |a, t| t.map(|t| a.max(t)));
                    (
                        Span {
                            min: Some(0.0),
                            max: top,
                        },
                        Why {
                            text: format!(
                                "driven by output pin {pin}, which reaches at most its part's highest supply{shared}"
                            ),
                            from: supplies.clone(),
                            bounded: true,
                            low_floor: false,
                        },
                    )
                }
                Pull::Channel {
                    via,
                    net,
                    gate,
                    n_channel,
                    state,
                    reason,
                } => {
                    // The channel conducts only while the gate is past the net it pulls (taking the threshold as
                    // 0 V, which holds at any temperature), so it can't take the net past the gate.
                    let (y, g) = (span_of(out, *net), span_of(out, *gate));
                    let span = if *n_channel {
                        let cap = |v: Option<f64>| v.zip(g.max).map(|(v, c)| v.min(c));
                        Span {
                            min: cap(y.min),
                            max: cap(y.max),
                        }
                    } else {
                        let floor = |v: Option<f64>| v.zip(g.min).map(|(v, f)| v.max(f));
                        Span {
                            min: floor(y.min),
                            max: floor(y.max),
                        }
                    };
                    (
                        span,
                        Why {
                            text: format!(
                                "through {via}'s channel toward {}, {} the gate ({}){shared}; the channel is {}: {reason}",
                                name(*net),
                                if *n_channel { "no higher than" } else { "no lower than" },
                                name(*gate),
                                if *state == ChannelState::On {
                                    "fully on"
                                } else {
                                    "maybe on"
                                }
                            ),
                            from: vec![*net, *gate],
                            bounded: *n_channel,
                            low_floor: out[*net].as_ref().is_some_and(|nv| nv.floor),
                        },
                    )
                }
                Pull::Unknown(reason) => (
                    Span::unknown(),
                    Why {
                        text: format!("UNKNOWN: {reason}{shared}"),
                        ..Default::default()
                    },
                ),
            };
            merge(&mut nv, span, why);
        }
        let mut nv = nv.expect("a group with pulls has a range");
        for o in &self.off {
            nv.why.push(Why {
                text: format!("{o}, so it doesn't pull this net"),
                ..Default::default()
            });
        }
        // A channel that may be off doesn't prove the net is driven.
        let real = self.pulls.iter().any(|p| {
            matches!(p, Pull::Net { .. })
                || matches!(
                    p,
                    Pull::Channel {
                        state: ChannelState::On,
                        ..
                    }
                )
                || matches!(p, Pull::Output { push_pull, .. } if *push_pull)
        });
        nv.driver = if real { Driver::Pulled } else { Driver::Weak };
        if !self.reactive && self.pulls.iter().all(|p| matches!(p, Pull::Net { .. })) {
            nv.between = self
                .pulls
                .iter()
                .filter_map(|p| match p {
                    Pull::Net { net, .. } => Some(*net),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        nv
    }
}

pub(super) fn build_groups(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    strong: &[bool],
    channels: &[Channel],
) -> Vec<Group> {
    let n = design.nets.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        p[x] = r;
        r
    }
    for c in design.components.values() {
        if c.dnp || !is_link(lib, c) {
            continue;
        }
        let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
        if let [a, b] = nets[..]
            && !strong[a]
            && !strong[b]
        {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            parent[ra] = rb;
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in (0..n).filter(|&i| !strong[i]) {
        let r = find(&mut parent, i);
        members.entry(r).or_default().push(i);
    }
    members
        .into_values()
        .map(|nets| {
            let mut pulls = vec![];
            let mut off = vec![];
            let mut reactive = false;
            for &net in &nets {
                for (r, pnum) in &design.nets[net].pins {
                    let Some(c) = design.components.get(r) else { continue };
                    if c.dnp {
                        continue;
                    }
                    // A MOSFET's pins: its channel pulls toward the other side unless it is off; its gate takes.
                    let mine: Vec<&Channel> = channels
                        .iter()
                        .filter(|ch| {
                            &ch.reference == r && (ch.drain == net || ch.source == net || ch.gate == Some(net))
                        })
                        .collect();
                    if !mine.is_empty() {
                        for ch in mine {
                            let other = if ch.drain == net {
                                ch.source
                            } else if ch.source == net {
                                ch.drain
                            } else {
                                continue;
                            };
                            if ch.state == ChannelState::Off {
                                let o = format!("{r}'s channel is off: {}", ch.reason);
                                if !off.contains(&o) {
                                    off.push(o);
                                }
                            } else if nets.contains(&other) {
                                // Both ends are already in this group, which shares one range.
                            } else {
                                let pull = match (ch.gate, ch.n_channel) {
                                    (Some(gate), Some(n_channel)) => Pull::Channel {
                                        via: r.clone(),
                                        net: other,
                                        gate,
                                        n_channel,
                                        state: ch.state,
                                        reason: ch.reason.clone(),
                                    },
                                    _ => Pull::Unknown(format!(
                                        "{r}'s channel may be on, and {}",
                                        if ch.gate.is_none() {
                                            "its gate isn't connected"
                                        } else {
                                            "its part file doesn't say whether it is N- or P-channel"
                                        }
                                    )),
                                };
                                if !pulls
                                    .iter()
                                    .any(|p| matches!(p, Pull::Channel { via, net, .. } if via == r && *net == other))
                                {
                                    pulls.push(pull);
                                }
                            }
                        }
                        continue;
                    }
                    if is_capacitor(lib, c) || is_inductor_symbol(c) || class_of(lib, c) == Some(Class::Inductor) {
                        reactive = true;
                    }
                    let Some(pin) = c.pins.get(pnum) else { continue };
                    if is_switch(c) {
                        // A switch doesn't join two groups: the far side's pulls would then count as driving
                        // this net, which an open switch leaves floating.
                        match c.pins.values().map(|q| q.net).find(|&q| q != net) {
                            Some(o) if strong[o] => pulls.push(Pull::Switch { via: r.clone(), net: o }),
                            Some(o) => pulls.push(Pull::Unknown(format!(
                                "{r} is a switch to {}, which nothing drives directly",
                                design.nets[o].name
                            ))),
                            None => {}
                        }
                    } else if is_link(lib, c) {
                        let other = c.pins.values().map(|q| q.net).find(|&q| q != net);
                        if let Some(o) = other
                            && strong[o]
                        {
                            pulls.push(Pull::Net { via: r.clone(), net: o });
                        }
                    } else if is_connector(lib, c) {
                        if is_undriven(inputs, r, pnum) {
                            continue;
                        }
                        pulls.push(Pull::Unknown(format!(
                            "{r}.{pnum} is a connector pin with no range in inputs.toml"
                        )));
                    } else {
                        match pin.kind {
                            PinKind::Output
                            | PinKind::Bidirectional
                            | PinKind::TriState
                            | PinKind::OpenCollector
                            | PinKind::OpenEmitter => {
                                let supplies: Vec<usize> = c
                                    .pins
                                    .values()
                                    .filter(|q| q.kind == PinKind::PowerIn)
                                    .map(|q| q.net)
                                    .collect::<BTreeSet<_>>()
                                    .into_iter()
                                    .collect();
                                if supplies.is_empty() {
                                    pulls.push(Pull::Unknown(format!(
                                        "{r}.{pnum} is an output, and {r} has no power pin to bound it"
                                    )));
                                } else {
                                    pulls.push(Pull::Output {
                                        pin: format!("{r}.{pnum}"),
                                        supplies,
                                        push_pull: matches!(pin.kind, PinKind::Output | PinKind::Bidirectional),
                                    });
                                }
                            }
                            PinKind::PowerOut => pulls.push(Pull::Unknown(format!(
                                "{r}.{pnum} is a power output, and no part file says what it drives"
                            ))),
                            // Capacitors are open, a TVS only conducts past its breakdown (a check's job),
                            // and power and logic inputs only take.
                            PinKind::PowerIn | PinKind::Input | PinKind::NoConnect => {}
                            _ if is_capacitor(lib, c) || class_of(lib, c) == Some(Class::Tvs) => {}
                            _ => pulls.push(Pull::Unknown(format!(
                                "{r}.{pnum} is a {:?} pin on a part gerberus can't model, so it may pull the net \
                                 anywhere",
                                pin.kind
                            ))),
                        }
                    }
                }
            }
            Group {
                nets,
                pulls,
                off,
                reactive,
            }
        })
        .collect()
}

/// A two-pin part a net's voltage passes through, for the fallback range: resistors, inductors and ferrite
/// beads (and so LEDs, `Device:L*`), net ties and fuses, and jumpers. Switches are [`is_switch`]: an open one is
/// as likely as a closed one.
fn is_link(lib: &Library, c: &Component) -> bool {
    c.pins.len() == 2
        && (c.lib_id.starts_with("Device:R")
            || is_inductor_symbol(c)
            || short_kind(c).is_some()
            || c.lib_id.starts_with("Jumper:")
            || matches!(class_of(lib, c), Some(Class::Resistor | Class::Inductor | Class::Led)))
}

/// A two-pin switch (`Switch:*`, `SW1`): a link while it is closed, nothing while it is open.
fn is_switch(c: &Component) -> bool {
    let letters: String = c.reference.chars().take_while(char::is_ascii_alphabetic).collect();
    c.pins.len() == 2 && (c.lib_id.starts_with("Switch:") || letters.eq_ignore_ascii_case("SW"))
}
