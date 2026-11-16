//! What each rail supplies, what each part dissipates, and how hot its junction gets.
//!
//! Currents come from the part files: every IC's worst-case supply current (`[[supply]]`), every LED's current
//! through its series resistor, and every resistor from a rail straight to ground. A regulator passes its
//! output's current on to its input (a buck converter scaled by its efficiency, when its part file gives
//! one), and so does a diode in series with a rail. Nothing is guessed: a part on a rail with no supply
//! current makes that rail's total unknown, and everything fed from it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::design::{Component, Design, PinKind};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, DriveKind, Library, Lookup, Part, Range};
use crate::policy::Policy;
use crate::rules::{Finding, Severity, Verdict};
use crate::voltage::{Voltages, resistance_range};

/// The current a net supplies, worst case.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Load {
    /// Amps, the sum of everything that could be worked out.
    pub amps: f64,
    /// What makes up `amps`, for the report: `U2 24.6 mA (p50)`.
    pub parts: Vec<String>,
    /// Parts on the net whose current isn't known. Non-empty means `amps` is only a lower bound.
    pub missing: Vec<String>,
}

impl Load {
    fn add(&mut self, amps: f64, what: String) {
        self.amps += amps;
        self.parts.push(what);
    }

    fn take(&mut self, other: &Load, via: &str) {
        self.amps += other.amps;
        self.parts.push(format!("{via} {}", ma(other.amps)));
        self.missing.extend(other.missing.iter().cloned());
    }

    pub fn known(&self) -> bool {
        self.missing.is_empty()
    }
}

/// What one part dissipates.
#[derive(Debug, Clone, PartialEq)]
pub struct Dissipation {
    pub reference: String,
    /// Watts, when they could be worked out (a lower bound when `missing` isn't empty).
    pub watts: Option<f64>,
    /// How, for the report.
    pub how: String,
    pub missing: Vec<String>,
}

/// A number to four significant figures, without trailing zeros.
fn sig(x: f64) -> String {
    if x == 0.0 {
        return "0".into();
    }
    #[allow(clippy::cast_possible_truncation)]
    let digits = (3 - x.abs().log10().floor() as i32).max(0);
    let s = format!("{x:.*}", usize::try_from(digits).unwrap_or(0));
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn ma(amps: f64) -> String {
    if amps >= 1.0 {
        format!("{} A", sig(amps))
    } else if amps >= 1e-3 {
        format!("{} mA", sig(amps * 1e3))
    } else {
        format!("{} µA", sig(amps * 1e6))
    }
}

fn mw(watts: f64) -> String {
    format!("{} mW", sig(watts * 1e3))
}

fn v(volts: f64) -> String {
    format!("{} V", sig(volts))
}

fn part_of<'a>(lib: &'a Library, c: &Component) -> Option<&'a Part> {
    if c.dnp {
        return None;
    }
    match lib.lookup(c) {
        Lookup::Datasheet(p) => Some(p),
        _ => None,
    }
}

fn pin_net(c: &Component, part: &Part, name: &str) -> Option<usize> {
    part.pins_named(name)
        .into_iter()
        .find_map(|num| c.pins.get(num).map(|p| p.net))
}

fn number(b: &Bound) -> Option<f64> {
    if let Bound::Value(v) = b { Some(*v) } else { None }
}

fn range_ends(r: &Range) -> (Option<f64>, Option<f64>) {
    match r {
        Range::Known { min, max, .. } => (number(min), number(max)),
        Range::Unknown(_) => (None, None),
    }
}

fn page_of(r: &Range) -> String {
    match r {
        Range::Known { page: Some(p), .. } => format!(" (p{p})"),
        _ => String::new(),
    }
}

fn is_ground(volts: &Voltages, net: usize) -> bool {
    volts.nets[net].span.min == Some(0.0) && volts.nets[net].span.max == Some(0.0)
}

/// A forward drive's (anode, cathode, Vf range) for a diode or LED.
fn forward(part: &Part) -> Option<(&str, &str, &Range)> {
    part.drives
        .iter()
        .find(|d| d.kind == DriveKind::Forward)
        .map(|d| (d.pin.as_str(), d.to.as_str(), &d.range))
}

/// A regulator's own drive and whether it switches (a buck converter has a duty-cycle headroom below 1).
fn regulator_drive<'a>(part: &'a Part, pin: &str) -> Option<&'a crate::limits::Drive> {
    part.drives
        .iter()
        .find(|d| d.kind == DriveKind::Regulates && d.pin == pin)
}

/// Currents and dissipation for a whole design.
#[derive(Debug, Clone, Default)]
pub struct Thermal {
    /// What each net supplies, indexed like [`Design::nets`].
    pub loads: Vec<Load>,
    pub parts: BTreeMap<String, Dissipation>,
    /// Current through each series diode and LED, for its current rating.
    pub forward: BTreeMap<String, (Load, String)>,
}

/// Works out every rail's load and every part's dissipation.
pub fn compute(design: &Design, lib: &Library, inputs: &BoardInputs, volts: &Voltages) -> Thermal {
    let n = design.nets.len();
    let mut base = vec![Load::default(); n];
    let mut own: BTreeMap<String, Dissipation> = BTreeMap::new();
    let mut forward_through: BTreeMap<String, (Load, String)> = BTreeMap::new();
    let vmax = |net: usize| volts.nets[net].span.max;
    let regulators: BTreeSet<&str> = volts.regulators.iter().map(|r| r.reference.as_str()).collect();

    // The rail an IC's supply entry is on, for current it sources from its outputs.
    let supply_rail = |c: &Component, part: &Part| -> Option<usize> {
        part.supply
            .iter()
            .find_map(|s| s.pins.iter().find_map(|p| pin_net(c, part, p)))
    };

    for (reference, c) in &design.components {
        let part = part_of(lib, c);
        let class = part.map(|p| p.class);
        match (part, class) {
            (Some(part), Some(Class::Ic | Class::Oscillator)) => {
                let mut watts = 0.0;
                let mut how = vec![];
                let mut missing = vec![];
                let mut covered: BTreeSet<&str> = BTreeSet::new();
                for s in &part.supply {
                    covered.extend(s.pins.iter().map(String::as_str));
                    let Some(net) = s.pins.iter().find_map(|p| pin_net(c, part, p)) else {
                        continue;
                    };
                    let Some(amps) = s.max() else { continue };
                    base[net].add(amps, format!("{reference} {}{}", ma(amps), page_of(&s.current)));
                    match vmax(net) {
                        Some(volt) => {
                            watts += volt * amps;
                            how.push(format!("{} × {} on {}", ma(amps), v(volt), design.nets[net].name));
                        }
                        None => missing.push(format!("{} has no known voltage", design.nets[net].name)),
                    }
                }
                // Power pins with no supply current in the part file.
                let mut unknown_rails = BTreeSet::new();
                for (num, p) in &c.pins {
                    let name = part.pins.get(num).map_or(num.as_str(), String::as_str);
                    if p.kind == PinKind::PowerIn && !covered.contains(name) && !is_ground(volts, p.net) {
                        unknown_rails.insert(p.net);
                    }
                }
                for net in unknown_rails {
                    let what = format!("{reference} ({} gives no supply current for its pins here)", part.mpn);
                    // A regulator's own ground current is small next to what it passes on; it still counts as
                    // unknown, so the rail's total is a lower bound.
                    base[net].missing.push(what.clone());
                    missing.push(what);
                }
                if !regulators.contains(reference.as_str()) {
                    own.insert(
                        reference.clone(),
                        Dissipation {
                            reference: reference.clone(),
                            watts: Some(watts),
                            how: if how.is_empty() {
                                "no supply current in its part file".into()
                            } else {
                                how.join(" + ")
                            },
                            missing,
                        },
                    );
                }
            }
            // A connector's power pin (a debug probe's VTref) only senses the rail.
            (None, _)
                if !c.dnp
                    && !c.lib_id.starts_with("Connector")
                    && c.pins.values().any(|p| p.kind == PinKind::PowerIn) =>
            {
                for p in c
                    .pins
                    .values()
                    .filter(|p| p.kind == PinKind::PowerIn && !is_ground(volts, p.net))
                {
                    base[p.net].missing.push(format!("{reference} (no part file)"));
                }
            }
            _ => {}
        }
    }

    // LEDs with a series resistor to ground, and resistors from a net straight to ground.
    for (reference, c) in &design.components {
        let Some(part) = part_of(lib, c) else { continue };
        if part.class == Class::Led
            && let Some((a, k, vf)) = forward(part)
            && let (Some(an), Some(kn)) = (pin_net(c, part, a), pin_net(c, part, k))
            && is_ground(volts, kn)
        {
            // The one resistor feeding the anode.
            let feeds: Vec<(&String, &Component)> = design.nets[an]
                .pins
                .iter()
                .filter(|(r, _)| r != reference)
                .filter_map(|(r, _)| design.components.get(r).map(|c| (r, c)))
                .collect();
            let (vf_min, vf_max) = range_ends(vf);
            let what = format!("{reference} ({})", part.mpn);
            match feeds.as_slice() {
                [(r, rc)] if rc.pins.len() == 2 => {
                    let Some(far) = rc.pins.values().map(|p| p.net).find(|&n| n != an) else {
                        continue;
                    };
                    let ohms = resistance_range(lib, inputs, r, rc);
                    let (amps, text) = match (ohms, vmax(far), vf_min) {
                        (Ok((rmin, _, _)), Some(vs), Some(vf)) => {
                            // VF's minimum holds only at and above the current it was measured at; below
                            // that the LED drops less, so the only safe bound is VF ≥ 0.
                            let i = ((vs - vf) / rmin).max(0.0);
                            let tested = part
                                .drives
                                .iter()
                                .find(|d| d.kind == DriveKind::Forward)
                                .and_then(|d| d.current);
                            match tested {
                                Some(t) if i >= t => (
                                    Some(i),
                                    format!(
                                        "{} through {r} from {} ({} − VF {} over {} Ω)",
                                        ma(i),
                                        design.nets[far].name,
                                        v(vs),
                                        v(vf),
                                        sig(rmin)
                                    ),
                                ),
                                _ => {
                                    let bound = vs / rmin;
                                    let why = match tested {
                                        Some(t) => format!(
                                            "VF {} is rated at {}, more than the {} it would give here",
                                            v(vf),
                                            ma(t),
                                            ma(i)
                                        ),
                                        None => {
                                            format!("the part file doesn't say what current VF {} is rated at", v(vf))
                                        }
                                    };
                                    (
                                        Some(bound),
                                        format!(
                                            "{} at most through {r} from {} ({} over {} Ω, counting VF as 0 V: {why})",
                                            ma(bound),
                                            design.nets[far].name,
                                            v(vs),
                                            sig(rmin)
                                        ),
                                    )
                                }
                            }
                        }
                        (Err(e), ..) => (None, e),
                        (_, None, _) => (None, format!("{} has no known voltage", design.nets[far].name)),
                        (.., None) => (None, format!("{} gives no minimum forward voltage", part.mpn)),
                    };
                    // An IC output driving the LED sources its current from that IC's supply.
                    let driver = design.nets[far].pins.iter().find_map(|(dr, dp)| {
                        let dc = design.components.get(dr)?;
                        let dpart = part_of(lib, dc)?;
                        let kind = dc.pins.get(dp)?.kind;
                        (dpart.class == Class::Ic
                            && matches!(kind, PinKind::Output | PinKind::Bidirectional | PinKind::TriState))
                        .then(|| supply_rail(dc, dpart).map(|rail| (dr.clone(), rail)))
                        .flatten()
                    });
                    let rail = driver.as_ref().map_or(far, |(_, rail)| *rail);
                    let via = driver
                        .as_ref()
                        .map(|(d, _)| format!(", sourced by {d}"))
                        .unwrap_or_default();
                    let mut load = Load::default();
                    if let Some(i) = amps {
                        base[rail].add(i, format!("{what} {}{via}", ma(i)));
                        load.add(i, text.clone());
                    } else {
                        base[rail].missing.push(format!("{what}: {text}"));
                        load.missing.push(text.clone());
                    }
                    let watts = amps.zip(vf_max).map(|(i, v)| i * v);
                    own.insert(
                        reference.clone(),
                        Dissipation {
                            reference: reference.clone(),
                            watts,
                            how: match vf_max {
                                Some(vf) => format!("{text}, at VF up to {}", v(vf)),
                                None => format!("{text}; {} gives no maximum forward voltage", part.mpn),
                            },
                            missing: if watts.is_some() { vec![] } else { vec![text.clone()] },
                        },
                    );
                    forward_through.insert(reference.clone(), (load, text));
                }
                _ => {
                    // Not a simple resistor-and-LED chain; its current isn't worked out.
                    if let Some(rail) = feeds
                        .first()
                        .and_then(|(_, fc)| fc.pins.values().map(|p| p.net).find(|&n| n != an))
                    {
                        base[rail].missing.push(format!("{what}: not fed through one resistor"));
                    }
                }
            }
        }
        if part.class == Class::Resistor && c.pins.len() == 2 {
            let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
            let (a, b) = (nets[0], nets[1]);
            let (rail, gnd) = match (is_ground(volts, a), is_ground(volts, b)) {
                (false, true) => (a, b),
                (true, false) => (b, a),
                _ => continue,
            };
            let _ = gnd;
            if let (Ok((rmin, _, _)), Some(vr)) = (resistance_range(lib, inputs, reference, c), vmax(rail)) {
                let i = vr / rmin;
                base[rail].add(i, format!("{reference} {}", ma(i)));
            }
        }
    }

    // Totals: a net's own draws plus everything fed from it through a regulator or a series diode.
    let mut totals: Vec<Option<Load>> = vec![None; n];
    let mut regs: BTreeMap<String, Dissipation> = BTreeMap::new();
    for net in 0..n {
        total(
            net,
            design,
            lib,
            volts,
            &base,
            &mut totals,
            &mut regs,
            &mut forward_through,
            &mut own,
            0,
        );
    }
    own.extend(regs);
    Thermal {
        loads: totals.into_iter().map(Option::unwrap_or_default).collect(),
        parts: own,
        forward: forward_through,
    }
}

#[allow(clippy::too_many_arguments)]
fn total(
    net: usize,
    design: &Design,
    lib: &Library,
    volts: &Voltages,
    base: &[Load],
    totals: &mut Vec<Option<Load>>,
    regs: &mut BTreeMap<String, Dissipation>,
    diodes: &mut BTreeMap<String, (Load, String)>,
    own: &mut BTreeMap<String, Dissipation>,
    depth: usize,
) -> Load {
    if let Some(l) = &totals[net] {
        return l.clone();
    }
    let mut load = base[net].clone();
    if depth > 16 {
        load.missing.push("a supply loop".into());
        return load;
    }
    // Regulators fed from this net.
    for reg in volts.regulators.iter().filter(|r| r.supply == net && r.sets_output) {
        let Some(c) = design.components.get(&reg.reference) else {
            continue;
        };
        let Some(part) = part_of(lib, c) else { continue };
        let out = total(
            reg.output,
            design,
            lib,
            volts,
            base,
            totals,
            regs,
            diodes,
            own,
            depth + 1,
        );
        let vin = volts.nets[net].span;
        let vout = volts.nets[reg.output].span;
        let drive = regulator_drive(part, &reg.pin);
        // A buck converter: a duty-cycle headroom, or a pin other than its input and output on a net with an
        // inductor (its switch node).
        let switches = drive.and_then(|d| d.headroom.as_ref()).is_some_and(|h| h.duty < 1.0)
            || c.pins
                .values()
                .filter(|p| p.net != net && p.net != reg.output)
                .any(|p| {
                    design.nets[p.net].pins.iter().any(|(r, _)| {
                        design.components.get(r).is_some_and(|l| {
                            l.lib_id == "Device:L"
                                || l.lib_id.starts_with("Device:L_")
                                || part_of(lib, l).is_some_and(|lp| lp.class == Class::Inductor)
                        })
                    })
                });
        let label = format!("{} ({})", reg.reference, part.mpn);
        if switches {
            let eff = part.efficiency.as_ref().map(|e| e.value);
            match (eff, vin.min, vout.max) {
                (Some(eta), Some(vi), Some(vo)) if vi > 0.0 => {
                    let iin = out.amps * vo / (eta * vi);
                    let mut fed = out.clone();
                    fed.amps = iin;
                    load.take(&fed, &format!("{label} input for"));
                    let pout = out.amps * vo;
                    regs.insert(
                        reg.reference.clone(),
                        Dissipation {
                            reference: reg.reference.clone(),
                            watts: Some(pout * (1.0 / eta - 1.0)),
                            how: format!(
                                "{} at {} out ({}) at {:.0} % efficiency",
                                mw(pout),
                                v(vo),
                                ma(out.amps),
                                eta * 100.0
                            ),
                            missing: out.missing.clone(),
                        },
                    );
                }
                _ => {
                    let why = format!("{label}: a switching regulator's input current needs its efficiency");
                    load.missing.push(why.clone());
                    regs.insert(
                        reg.reference.clone(),
                        Dissipation {
                            reference: reg.reference.clone(),
                            watts: None,
                            how: format!("supplies {} on {}", ma(out.amps), design.nets[reg.output].name),
                            missing: vec![format!("{} gives no efficiency", part.mpn)],
                        },
                    );
                }
            }
        } else {
            load.take(&out, &format!("{label} output"));
            let iq: f64 = part.supply.iter().filter_map(crate::limits::SupplyCurrent::max).sum();
            // The drop is largest at the top of the supply. When the output isn't held over the whole supply
            // range but is at its top, use the held output there: below the supply it needs, a linear
            // regulator is in dropout and drops less than it does while regulating.
            let vo_low = match (reg.held_at, vin.max) {
                (Some(h), Some(vi)) if vi >= h.needs && vout.min.is_none_or(|m| m < h.min) => Some(h.min),
                _ => vout.min,
            };
            let watts = match (vin.max, vo_low) {
                (Some(vi), Some(vo)) => Some((vi - vo).max(0.0) * out.amps + vi * iq),
                _ => None,
            };
            regs.insert(
                reg.reference.clone(),
                Dissipation {
                    reference: reg.reference.clone(),
                    watts,
                    how: format!(
                        "({} − {}) × {} through it{}{}",
                        vin.max.map_or("?".into(), v),
                        vo_low.map_or("?".into(), v),
                        ma(out.amps),
                        if iq > 0.0 {
                            format!(", plus its own {}", ma(iq))
                        } else {
                            String::new()
                        },
                        if vo_low == vout.min {
                            String::new()
                        } else {
                            format!(
                                " (the output is held at the top of {}; below the {} it needs, the regulator is in \
                                 dropout and drops less)",
                                design.nets[net].name,
                                v(reg.held_at.map_or(0.0, |h| h.needs))
                            )
                        }
                    ),
                    missing: out.missing.clone(),
                },
            );
        }
    }
    // Diodes in series: anode on this net, cathode on a net they feed.
    for (reference, c) in &design.components {
        let Some(part) = part_of(lib, c) else { continue };
        if part.class != Class::Diode {
            continue;
        }
        let Some((a, k, vf)) = forward(part) else { continue };
        let (Some(an), Some(kn)) = (pin_net(c, part, a), pin_net(c, part, k)) else {
            continue;
        };
        if an != net || kn == net || is_ground(volts, kn) || is_ground(volts, an) {
            continue;
        }
        let fed = total(kn, design, lib, volts, base, totals, regs, diodes, own, depth + 1);
        // Only a rail that something draws from: a clamp or flyback diode carries no steady current.
        if fed.parts.is_empty() && fed.missing.is_empty() {
            continue;
        }
        let label = format!("{reference} ({})", part.mpn);
        load.take(&fed, &format!("{label} to {}", design.nets[kn].name));
        let (_, vf_max) = range_ends(vf);
        own.insert(
            reference.clone(),
            Dissipation {
                reference: reference.clone(),
                watts: vf_max.map(|v| v * fed.amps),
                how: format!(
                    "{} through it at VF up to {}",
                    ma(fed.amps),
                    vf_max.map_or("? V".into(), v)
                ),
                missing: fed.missing.clone(),
            },
        );
        diodes.insert(
            reference.clone(),
            (fed.clone(), format!("{} into {}", ma(fed.amps), design.nets[kn].name)),
        );
    }
    totals[net] = Some(load.clone());
    load
}

/// The `junction-temperature` rule: each part's junction, at the hottest ambient, stays below its maximum, and
/// `policy.toml`'s margin below it.
pub fn junction(
    design: &Design,
    lib: &Library,
    policy: &Policy,
    inputs: &BoardInputs,
    th: &Thermal,
    r: &mut Vec<Finding>,
) {
    for (reference, d) in &th.parts {
        let Some(c) = design.components.get(reference) else {
            continue;
        };
        let Some(part) = part_of(lib, c) else { continue };
        let finding = |verdict, severity, message: String| Finding {
            rule: "junction-temperature",
            verdict,
            severity,
            reference: reference.clone(),
            message: format!("{reference} ({}): {message}", part.mpn),
            nets: vec![],
            case: None,
        };
        let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
        // Nothing known at all reads better as "unknown" than as "at least 0 mW".
        let watts = d.watts.filter(|&w| w > 0.0 || d.missing.is_empty());
        let Some(watts) = watts else {
            r.push(finding(
                Verdict::Unknown,
                Severity::Error,
                if d.watts.is_some() {
                    format!("its dissipation is unknown: {}", d.missing.join("; "))
                } else {
                    format!("its dissipation is unknown: {} ({})", d.how, d.missing.join("; "))
                },
            ));
            continue;
        };
        let lower = if d.missing.is_empty() {
            String::new()
        } else {
            format!(" at least (not counting {})", d.missing.join("; "))
        };
        let dissipates = format!("dissipates {}{lower}: {}", mw(watts), d.how);
        let board = inputs.temperature.is_some();
        let (ambient, from) = match (&inputs.temperature, &part.temperature) {
            (Some(t), _) => (t.max, format!("the board's {} °C ({})", t.max, t.source)),
            (None, Some(t)) => (
                t.max,
                format!(
                    "its rated {} °C ambient (p{}), as inputs.toml gives no board temperature",
                    t.max, t.page
                ),
            ),
            (None, None) => {
                r.push(finding(
                    Verdict::Unknown,
                    Severity::Error,
                    format!("{dissipates}; neither inputs.toml nor its part file gives an ambient temperature"),
                ));
                continue;
            }
        };
        let Some(theta) = &part.theta_ja else {
            r.push(finding(
                Verdict::Unknown,
                Severity::Error,
                format!(
                    "{dissipates}; {} gives no junction-to-ambient thermal resistance",
                    part.mpn
                ),
            ));
            continue;
        };
        let tj = ambient + watts * theta.value;
        let text = format!(
            "{dissipates}. At {from} and θJA {} °C/W (p{}), the junction reaches {tj:.0} °C",
            theta.value, theta.page
        );
        let Some(max) = &part.tj_max else {
            r.push(finding(
                Verdict::Unknown,
                Severity::Error,
                format!("{text}; {} gives no maximum junction temperature", part.mpn),
            ));
            continue;
        };
        let cite = format!("{} {doc} p{}", part.mpn, max.page);
        let margin = policy.junction.as_ref();
        // At the part's own rated ambient rather than the board's, a failure only says how warm the board may get.
        let up_to = max.value - watts * theta.value;
        let (verdict, severity, end) = if tj > max.value && !board {
            (
                Verdict::Unknown,
                Severity::Error,
                format!(
                    ", above its {} °C maximum ({cite}): it is only safe up to {:.0} °C ambient, and inputs.toml gives \
                     no board temperature to compare",
                    max.value, up_to
                ),
            )
        } else if tj > max.value {
            (
                Verdict::Fail,
                Severity::Error,
                format!(", above its {} °C maximum ({cite})", max.value),
            )
        } else if let Some(m) = margin.filter(|m| tj > max.value - m.margin && board) {
            (
                Verdict::Fail,
                Severity::Warning,
                format!(
                    ", within {:.0} °C of its {} °C maximum ({cite}); policy.toml keeps {} °C ({})",
                    max.value - tj,
                    max.value,
                    m.margin,
                    m.reason
                ),
            )
        } else {
            (
                Verdict::Pass,
                Severity::Error,
                format!(", below its {} °C maximum ({cite})", max.value),
            )
        };
        // Without a board temperature the margin can't be judged either; say what it would be.
        let end = match margin {
            Some(m) if !board && verdict == Verdict::Pass && tj > max.value - m.margin => format!(
                "{end}; policy.toml's {} °C margin holds only up to {:.0} °C ambient",
                m.margin,
                up_to - m.margin
            ),
            _ => end,
        };
        let (verdict, end) = if verdict == Verdict::Pass && !d.missing.is_empty() {
            (Verdict::Unknown, format!("{end}, but only counting what is known"))
        } else {
            (verdict, end)
        };
        r.push(finding(verdict, severity, format!("{text}{end}")));
    }
}

/// The report's summary of rail loads: every net that supplies something.
pub fn describe(design: &Design, th: &Thermal) -> String {
    let mut out = String::new();
    for (i, l) in th.loads.iter().enumerate() {
        if l.parts.is_empty() && l.missing.is_empty() {
            continue;
        }
        let _ = write!(out, "{}: {}", design.nets[i].name, ma(l.amps));
        if !l.missing.is_empty() {
            let _ = write!(out, " at least");
        }
        if l.parts.is_empty() {
            let _ = writeln!(out);
        } else {
            let _ = writeln!(out, " ({})", l.parts.join(", "));
        }
        for m in &l.missing {
            let _ = writeln!(out, "  not counted: {m}");
        }
    }
    out
}

/// The `forward-current` rule: the current through each series diode and LED stays inside its rating, and
/// inside `policy.toml`'s derating of it.
pub fn forward_current(design: &Design, lib: &Library, policy: &Policy, th: &Thermal, r: &mut Vec<Finding>) {
    for (reference, (load, how)) in &th.forward {
        let Some(c) = design.components.get(reference) else {
            continue;
        };
        let Some(part) = part_of(lib, c) else { continue };
        let Some((a, k, _)) = forward(part) else { continue };
        let finding = |verdict, severity, message: String| Finding {
            rule: "forward-current",
            verdict,
            severity,
            reference: reference.clone(),
            message: format!("{reference} ({}): {message}", part.mpn),
            nets: vec![],
            case: None,
        };
        let limit = part.limits.iter().find(|l| {
            l.quantity == crate::limits::Quantity::Current
                && l.pins.iter().any(|p| p == a)
                && l.to.as_deref() == Some(k)
        });
        let Some(limit) = limit else {
            r.push(finding(
                Verdict::Unknown,
                Severity::Error,
                format!("{how}; {} gives no forward current rating", part.mpn),
            ));
            continue;
        };
        // The rating: the recommended maximum when there is one, else the absolute maximum.
        let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
        let rating = [&limit.recommended, &limit.abs_max]
            .into_iter()
            .find_map(|range| match range {
                Range::Known {
                    max: Bound::Value(v),
                    page,
                    note,
                    ..
                } => Some((
                    *v,
                    format!(
                        "{} {doc} p{}{}",
                        part.mpn,
                        page.unwrap_or(0),
                        note.as_deref().map(|n| format!(", {n}")).unwrap_or_default()
                    ),
                )),
                _ => None,
            });
        let Some((max, cite)) = rating else {
            r.push(finding(
                Verdict::Unknown,
                Severity::Error,
                format!("{how}; {} gives no numeric forward current rating", part.mpn),
            ));
            continue;
        };
        let amps = load.amps;
        if amps <= 0.0 && !load.known() {
            r.push(finding(
                Verdict::Unknown,
                Severity::Error,
                format!("the current through it is unknown: {}", load.missing.join("; ")),
            ));
            continue;
        }
        let lower = if load.known() { "" } else { " at least" };
        // An LED chain's description already starts with its current.
        let text = if how.starts_with(&ma(amps)) {
            how.clone()
        } else {
            format!("{}{lower} ({how})", ma(amps))
        };
        let derate = policy.derate_for(part, limit);
        let (verdict, severity, end) = if amps > max {
            (
                Verdict::Fail,
                Severity::Error,
                format!(", above its {} rating ({cite})", ma(max)),
            )
        } else if let Some(d) = derate.filter(|d| amps > max * d.factor) {
            (
                Verdict::Fail,
                Severity::Warning,
                format!(
                    ", above {:.0} % of its {} rating ({cite}): {}",
                    d.factor * 100.0,
                    ma(max),
                    d.reason
                ),
            )
        } else if !load.known() {
            (
                Verdict::Unknown,
                Severity::Error,
                format!(
                    ", within its {} rating ({cite}) so far, but not everything it carries is known: {}",
                    ma(max),
                    load.missing.join("; ")
                ),
            )
        } else {
            (
                Verdict::Pass,
                Severity::Error,
                format!(", within its {} rating ({cite})", ma(max)),
            )
        };
        r.push(finding(verdict, severity, format!("{text}{end}")));
    }
}
