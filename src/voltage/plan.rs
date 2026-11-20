//! How a part's drive sets a net: a diode's drop, a regulator's output through its feedback divider, a switch node,
//! a bias or a channel.
use crate::design::{Component, Design};
use crate::inputs::BoardInputs;
use crate::limits::Class;
use crate::limits::{Bound, Drive, DriveKind, Library, Part, Quantity, Range};
use std::fmt::Write as _;

use super::{
    HeldAt, Hold, NetVoltage, Regulation, Span, Why, cite, class_of, is_capacitor, part_pin_net, round, span_of,
};

/// One way a part sets a net.
pub(super) struct Plan<'a> {
    pub(super) target: usize,
    /// Nets that must be known first.
    pub(super) deps: Vec<usize>,
    reference: &'a str,
    part: &'a Part,
    pub(super) drive: &'a Drive,
    how: How,
}

pub(super) enum How {
    /// The drive's `to` (cathode) net is the anode net minus the forward drop.
    Forward {
        anode: usize,
    },
    Regulates {
        supply: usize,
        ground: usize,
        divider: Option<Divider>,
        /// The target is the divider's output, not the regulated pin itself.
        on_output: bool,
    },
    Switches {
        supply: usize,
        ground: usize,
    },
    /// The net can't be worked out, for this reason.
    Fails(String),
}

/// A feedback divider: `top` from the output to the feedback pin, `bottom` from it to ground.
#[derive(Clone)]
pub(super) struct Divider {
    top: String,
    bottom: String,
    /// Smallest and largest resistance, from the value and the tolerance.
    rt: (f64, f64),
    rb: (f64, f64),
    /// What the explanation cites for each resistor.
    note: String,
}

impl Divider {
    /// The output range for a feedback range `(lo, hi)`.
    fn output(&self, lo: f64, hi: f64) -> (f64, f64) {
        (lo * (1.0 + self.rt.0 / self.rb.1), hi * (1.0 + self.rt.1 / self.rb.0))
    }
}

pub(super) fn plan_drive<'a>(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    reference: &'a str,
    c: &Component,
    part: &'a Part,
    drive: &'a Drive,
) -> Vec<Plan<'a>> {
    let net = |name: &str| part_pin_net(c, part, name);
    let plan = |target, deps, how| Plan {
        target,
        deps,
        reference,
        part,
        drive,
        how,
    };
    let Some(pin) = net(&drive.pin) else { return vec![] };
    let to = net(&drive.to);
    let supply = drive.supply.as_deref().and_then(net);
    match drive.kind {
        // Worked out from the gate before the nets are, in `compute`.
        DriveKind::Channel => vec![],
        DriveKind::Forward => match to {
            Some(k) if k != pin => vec![plan(k, vec![pin], How::Forward { anode: pin })],
            _ => vec![],
        },
        // A part's own input bias is a high-resistance pull that anything else on the net overrides, so it only
        // sets an input reached through capacitors alone.
        DriveKind::Biases if !design.only_capacitors_on(reference, pin) => vec![],
        DriveKind::Switches | DriveKind::Biases => match (supply, to) {
            (Some(s), Some(g)) => vec![plan(pin, vec![s, g], How::Switches { supply: s, ground: g })],
            _ => vec![plan(pin, vec![], How::Fails(missing(drive)))],
        },
        DriveKind::Regulates => {
            let (Some(s), Some(g)) = (supply, to) else {
                return vec![plan(pin, vec![], How::Fails(missing(drive)))];
            };
            let found = if drive.feedback {
                find_divider(design, lib, inputs, reference, pin, g)
            } else {
                Ok(None)
            };
            match found {
                Ok(None) => vec![plan(
                    pin,
                    vec![s, g],
                    How::Regulates {
                        supply: s,
                        ground: g,
                        divider: None,
                        on_output: false,
                    },
                )],
                Ok(Some((div, output))) => vec![
                    plan(
                        pin,
                        vec![s, g],
                        How::Regulates {
                            supply: s,
                            ground: g,
                            divider: Some(div.clone()),
                            on_output: false,
                        },
                    ),
                    plan(
                        output,
                        vec![s, g],
                        How::Regulates {
                            supply: s,
                            ground: g,
                            divider: Some(div),
                            on_output: true,
                        },
                    ),
                ],
                Err((reason, output)) => {
                    let mut v = vec![plan(pin, vec![], How::Fails(reason.clone()))];
                    if let Some(o) = output {
                        v.push(plan(o, vec![], How::Fails(reason)));
                    }
                    v
                }
            }
        }
    }
}

fn missing(drive: &Drive) -> String {
    format!(
        "UNKNOWN: the part's {} or supply pin isn't connected in the schematic",
        drive.to
    )
}

/// The smallest and largest a resistor can be: its value, off by its tolerance plus its temperature drift over
/// the board's temperature range (or its own rated range when `inputs.toml` gives none). Also says where the
/// numbers come from.
pub fn resistance_range(
    lib: &Library,
    inputs: &BoardInputs,
    r: &str,
    c: &Component,
) -> Result<(f64, f64, String), String> {
    let Some(ohms) = crate::units::parse_ohms(&c.value).filter(|&v| v > 0.0) else {
        return Err(format!("can't read a resistance from {r}'s value {:?}", c.value));
    };
    let Some(part) = lib.part_for(c) else {
        return Err(format!(
            "{r} ({}) has no part file, so its tolerance is unknown",
            crate::limits::component_mpn(c)
        ));
    };
    let Some(tol) = &part.tolerance else {
        return Err(format!("{r}'s part file {} gives no tolerance", part.mpn));
    };
    let Some(tcr) = &part.tcr else {
        return Err(format!("{r}'s part file {} gives no temperature coefficient", part.mpn));
    };
    let (tmin, tmax, twhere) = match (&inputs.temperature, &part.temperature) {
        (Some(b), _) => (b.min, b.max, format!("the board's {} to {} °C", b.min, b.max)),
        (None, Some(p)) => (
            p.min,
            p.max,
            format!(
                "its rated {} to {} °C (p{}), as inputs.toml gives no board temperature",
                p.min, p.max, p.page
            ),
        ),
        (None, None) => {
            return Err(format!(
                "neither inputs.toml nor {r}'s part file gives a temperature range, so its drift is unknown"
            ));
        }
    };
    let drift = tcr.ppm * 1e-6 * (tmax - 25.0).abs().max((25.0 - tmin).abs());
    let dev = tol.value + drift;
    let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
    let note = format!(
        "{r} {} ±{}% (p{}) plus {} ppm/°C (p{}) over {twhere}, so ±{}% ({} {doc})",
        c.value,
        round(tol.value * 100.0),
        tol.page,
        tcr.ppm,
        tcr.page,
        round(dev * 100.0),
        part.mpn,
    );
    Ok((ohms * (1.0 - dev), ohms * (1.0 + dev), note))
}

/// The divider on a regulator's feedback net, and the net it sets. `Ok(None)` when no resistor is on the pin,
/// so it is tied straight to the output and the drive sets the pin's own net. The error carries the output net
/// when one was found, so it too is reported.
#[expect(clippy::type_complexity, reason = "a one-use error type; naming it adds nothing")]
fn find_divider(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    regulator: &str,
    fb: usize,
    ground: usize,
) -> Result<Option<(Divider, usize)>, (String, Option<usize>)> {
    let mut resistors: Vec<(&str, &Component, usize)> = vec![];
    for (r, p) in &design.nets[fb].pins {
        if r == regulator {
            continue;
        }
        let Some(c) = design.components.get(r) else { continue };
        if c.dnp || is_capacitor(lib, c) {
            continue;
        }
        if !is_resistor(lib, c) {
            return Err((
                format!(
                    "UNKNOWN: {r}.{p} is on the feedback net {} besides the divider, and gerberus only understands \
                     two resistors and capacitors there",
                    design.nets[fb].name
                ),
                None,
            ));
        }
        let other = c.pins.values().map(|q| q.net).find(|&q| q != fb).unwrap_or(fb);
        resistors.push((r.as_str(), c, other));
    }
    if resistors.is_empty() {
        return Ok(None);
    }
    let bottom: Vec<_> = resistors.iter().filter(|x| x.2 == ground).collect();
    let top: Vec<_> = resistors.iter().filter(|x| x.2 != ground && x.2 != fb).collect();
    let (&&(rb_ref, rb, _), &&(rt_ref, rt, out)) = match (bottom.as_slice(), top.as_slice()) {
        ([b], [t]) if resistors.len() == 2 => (b, t),
        _ => {
            return Err((
                format!(
                    "UNKNOWN: the feedback net {} has {} resistors; only a two-resistor divider to ground is understood",
                    design.nets[fb].name,
                    resistors.len()
                ),
                None,
            ));
        }
    };
    let mut ends = vec![];
    let mut notes = vec![];
    for (r, c) in [(rt_ref, rt), (rb_ref, rb)] {
        match resistance_range(lib, inputs, r, c) {
            Ok((lo, hi, note)) => {
                ends.push((lo, hi));
                notes.push(note);
            }
            Err(e) => return Err((format!("UNKNOWN: {e}"), Some(out))),
        }
    }
    Ok(Some((
        Divider {
            top: rt_ref.to_string(),
            bottom: rb_ref.to_string(),
            rt: ends[0],
            rb: ends[1],
            note: notes.join(" over "),
        },
        out,
    )))
}

impl Plan<'_> {
    pub(super) fn eval(
        &self,
        design: &Design,
        out: &[Option<NetVoltage>],
        regulators: &mut Vec<Regulation>,
    ) -> (Span, Why) {
        let name = |n: usize| design.nets[n].name.as_str();
        let who = format!("{} ({})", self.reference, self.part.mpn);
        let src = cite(self.part, &self.drive.range);
        match &self.how {
            How::Fails(reason) => {
                if self.drive.kind == DriveKind::Regulates {
                    regulators.push(self.regulation(
                        Hold::Unknown(reason.clone()),
                        *supply_of(&self.how).unwrap_or(&self.target),
                        true,
                    ));
                }
                (
                    Span::unknown(),
                    Why {
                        text: format!("{who}: {reason}"),
                        ..Default::default()
                    },
                )
            }
            How::Forward { anode } => {
                let a = span_of(out, *anode);
                let (vf_lo, vf_hi, lo_note) = match &self.drive.range {
                    Range::Known { min, max, .. } => {
                        let hi = max.resolve(|_| None);
                        match min.resolve(|_| None) {
                            Some(lo) => (lo, hi, String::new()),
                            None => (0.0, hi, "; the smallest drop isn't given, so it is taken as 0 V".into()),
                        }
                    }
                    Range::Unknown(_) | Range::None(_) => (0.0, None, "; the drop isn't given".into()),
                };
                let raw_min = a.min.zip(vf_hi).map(|(v, d)| v - d);
                // A diode only lets its anode pull the cathode up: with the anode below 0 V (a supply connected
                // backwards) it blocks, and the cathode is left at the 0 V floor.
                let span = Span {
                    min: raw_min.map(|v| v.max(0.0)),
                    max: a.max.map(|v| (v - vf_lo).max(0.0)),
                };
                // Below the drop the diode is off, so 0 V is only a floor; so is a floor on the anode.
                let low_floor = raw_min.is_some_and(|v| v < 0.0) || out[*anode].as_ref().is_some_and(|nv| nv.floor);
                let hi_text = vf_hi.map_or("an unknown amount".into(), |v| format!("up to {v} V"));
                (
                    span,
                    Why {
                        text: format!(
                            "{who} conducts from {} to {}, dropping {hi_text} ({src}){lo_note}",
                            name(*anode),
                            name(self.target)
                        ),
                        from: vec![*anode],
                        bounded: true,
                        low_floor,
                    },
                )
            }
            How::Switches { supply, ground } => {
                if span_of(out, *ground) != Span::exact(0.0) {
                    return not_ground(&who, name(*ground), *ground);
                }
                let s = span_of(out, *supply);
                let sup = self.drive.supply.as_deref().unwrap_or("");
                let (min, max) = match &self.drive.range {
                    Range::Known { min, max, .. } => (bound_at(min, sup, s.min), bound_at(max, sup, s.max)),
                    Range::Unknown(_) | Range::None(_) => (None, None),
                };
                let mut min_note = String::new();
                if let Range::Known { min: lo, max: hi, .. } = &self.drive.range {
                    for (end, b, v) in [("lowest", lo, min), ("highest", hi, max)] {
                        match b {
                            Bound::Unknown(r) | Bound::None(r) => {
                                let _ = write!(min_note, "; {end} point UNKNOWN: {r}");
                            }
                            _ if v.is_none() => {
                                let _ = write!(
                                    min_note,
                                    "; {end} point UNKNOWN: it follows a pin whose voltage isn't known"
                                );
                            }
                            _ => {}
                        }
                    }
                }
                (
                    Span { min, max },
                    Why {
                        text: format!(
                            "{who} {} {} from {} ({src}){min_note}",
                            if self.drive.kind == DriveKind::Biases {
                                "biases its input"
                            } else {
                                "switches"
                            },
                            self.drive.pin,
                            name(*supply)
                        ),
                        from: vec![*supply],
                        // Bounded by the supply only when the datasheet says it swings no higher.
                        bounded: matches!(&self.drive.range, Range::Known { max: Bound::Pin { pin, plus, .. }, .. }
                            if pin == sup && *plus <= 0.0),
                        low_floor: false,
                    },
                )
            }
            How::Regulates {
                supply,
                ground,
                divider,
                on_output,
            } => {
                let sets_output = *on_output || divider.is_none();
                if span_of(out, *ground) != Span::exact(0.0) {
                    regulators.push(self.regulation(
                        Hold::Unknown(format!(
                            "its reference pin is on {}, which isn't a 0 V ground",
                            name(*ground)
                        )),
                        *supply,
                        sets_output,
                    ));
                    return not_ground(&who, name(*ground), *ground);
                }
                let Some((dlo, dhi)) = self.drive.range.values() else {
                    let text = format!("the regulated range of {} isn't given", self.drive.pin);
                    regulators.push(self.regulation(Hold::Unknown(text.clone()), *supply, sets_output));
                    return (
                        Span::unknown(),
                        Why {
                            text: format!("{who}: UNKNOWN: {text}"),
                            ..Default::default()
                        },
                    );
                };
                let s = span_of(out, *supply);
                let sup = self.drive.supply.as_deref().unwrap_or("");
                let rec = recommended(self.part, sup);
                // The regulated output, which is what VIN has to stay above.
                let (olo, ohi) = divider.as_ref().map_or((dlo, dhi), |d| d.output(dlo, dhi));
                let (lo, hi) = if *on_output { (olo, ohi) } else { (dlo, dhi) };
                // The share of the output the pin sees, when regulation fails and the pin follows VIN.
                let ratio_hi = match (divider, on_output) {
                    (Some(d), false) => d.rb.1 / (d.rb.1 + d.rt.0),
                    _ => 1.0,
                };
                let mut notes = vec![];
                let max = match (s.max, rec.1) {
                    (Some(sm), Some(rm)) if sm <= rm => Some(hi.min(sm * ratio_hi)),
                    (Some(sm), rm) => {
                        notes.push(match rm {
                            Some(rm) => format!(
                                "{} can reach {} V, above its recommended {} V, where regulation isn't specified, so the top end follows it",
                                name(*supply),
                                round(sm),
                                rm
                            ),
                            None => format!(
                                "{}'s recommended maximum isn't known, so regulation can't be assumed at its top end of {} V",
                                sup,
                                round(sm)
                            ),
                        });
                        Some(sm * ratio_hi)
                    }
                    (None, _) => {
                        notes.push(format!("{}'s maximum is UNKNOWN", name(*supply)));
                        None
                    }
                };
                let head = self.drive.headroom.as_ref();
                // The lowest supply the range is specified at.
                let needs = head.map(|h| (ohi + h.volts) / h.duty);
                let hold;
                let min = match (s.min, rec.0, needs) {
                    (Some(sn), Some(rn), Some(nd)) if sn >= rn && sn >= nd => {
                        hold = Hold::Held(format!(
                            "{} stays at or above {} V; the output needs {} V and {} at least {rn} V",
                            name(*supply),
                            round(sn),
                            round(nd),
                            sup
                        ));
                        let h = head.expect("needs comes from headroom");
                        notes.push(format!(
                            "{} stays at or above {} V, which covers the {} V the output needs ({} p{}{}); line and \
                             load regulation aren't modelled",
                            name(*supply),
                            round(sn),
                            round(nd),
                            self.part.mpn,
                            h.page,
                            h.note.as_deref().map(|n| format!(", {n}")).unwrap_or_default()
                        ));
                        Some(lo)
                    }
                    (Some(sn), rn, nd) => {
                        let (why, known) = match (rn, nd) {
                            (Some(rn), _) if sn < rn => (format!("below its recommended minimum of {rn} V"), true),
                            (None, _) => ("and its recommended minimum isn't known".into(), false),
                            (_, Some(nd)) => (format!("below the {} V the output is specified at", round(nd)), true),
                            (_, None) => ("and the part file gives no headroom the output needs".into(), false),
                        };
                        let text = format!("{} can fall to {} V, {why}", name(*supply), round(sn));
                        hold = if known {
                            Hold::NotHeld(text.clone())
                        } else {
                            Hold::Unknown(text.clone())
                        };
                        notes.push(format!(
                            "{text}, so the output isn't held: 0 V is only a floor for its low end, as nothing says how far it sags"
                        ));
                        Some(0.0)
                    }
                    (None, _, _) => {
                        hold = Hold::Unknown(format!("{}'s minimum is UNKNOWN", name(*supply)));
                        notes.push(format!("{}'s minimum is UNKNOWN", name(*supply)));
                        None
                    }
                };
                // A supply below 0 V (connected backwards) can pull the output down with it, through the body
                // diode many CMOS regulators have from output to input. Part files don't say whether this one
                // has it, so the supply's low end is only a floor for the output, and nothing drives it above 0 V.
                let (min, max) = match s.min {
                    Some(sn) if sn < 0.0 => {
                        notes.push(format!(
                            "{} can fall to {} V, below 0 V: the output may follow it down through the regulator, as                              far as the part file says nothing to rule out",
                            name(*supply),
                            round(sn)
                        ));
                        (Some(sn * ratio_hi), max.map(|m| m.max(0.0)))
                    }
                    _ => (min, max),
                };
                let held = matches!(hold, Hold::Held(_));
                let mut regulation = self.regulation(hold, *supply, sets_output);
                regulation.held_at = Some(HeldAt {
                    min: lo,
                    max: hi,
                    needs,
                });
                regulators.push(regulation);
                let what = match (divider, on_output) {
                    (Some(d), true) => format!(
                        "{who} holds {} at {dlo} to {dhi} V ({src}); through the divider {} over {} ({}) the output is {} to {} V",
                        self.drive.pin,
                        d.top,
                        d.bottom,
                        d.note,
                        round(olo),
                        round(ohi)
                    ),
                    _ => format!("{who} holds {} at {dlo} to {dhi} V ({src})", self.drive.pin),
                };
                (
                    Span { min, max },
                    Why {
                        text: format!("{what}, powered from {}: {}", name(*supply), notes.join("; ")),
                        from: vec![*supply],
                        bounded: true,
                        low_floor: !held,
                    },
                )
            }
        }
    }
}

fn not_ground(who: &str, net: &str, n: usize) -> (Span, Why) {
    (
        Span::unknown(),
        Why {
            text: format!("{who}: UNKNOWN: its reference pin is on {net}, which isn't a 0 V ground"),
            from: vec![n],
            ..Default::default()
        },
    )
}

fn supply_of(how: &How) -> Option<&usize> {
    match how {
        How::Regulates { supply, .. } | How::Switches { supply, .. } => Some(supply),
        _ => None,
    }
}

impl Plan<'_> {
    fn regulation(&self, hold: Hold, supply: usize, sets_output: bool) -> Regulation {
        Regulation {
            reference: self.reference.to_string(),
            mpn: self.part.mpn.clone(),
            output: self.target,
            supply,
            hold,
            pin: self.drive.pin.clone(),
            sets_output,
            held_at: None,
        }
    }
}

/// A bound of a drive range, where `{ pin = supply }` reads the supply net's value.
fn bound_at(b: &Bound, supply: &str, v: Option<f64>) -> Option<f64> {
    b.resolve(|p| if p == supply { v } else { None })
}

/// The recommended range of a supply pin, as numbers, when its part file gives them.
fn recommended(part: &Part, pin: &str) -> (Option<f64>, Option<f64>) {
    for l in part.limits_for(Quantity::Voltage, pin) {
        if let Range::Known { min, max, .. } = &l.recommended {
            return (min.resolve(|_| None), max.resolve(|_| None));
        }
    }
    (None, None)
}

fn is_resistor(lib: &Library, c: &Component) -> bool {
    c.pins.len() == 2 && (c.lib_id.starts_with("Device:R") || class_of(lib, c) == Some(Class::Resistor))
}
