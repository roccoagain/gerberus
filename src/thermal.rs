//! What each rail supplies, what each part dissipates, and how hot its junction gets.
//!
//! Currents come from the part files: every IC's worst-case supply current (`[[supply]]`), every LED's current
//! through its series resistor, and every resistor from a rail straight to ground. A regulator passes its
//! output's current on to its input (a buck converter scaled by its efficiency, when its part file gives
//! one), and so does a diode in series with a rail. Nothing is guessed: a part on a rail with no supply
//! current makes that rail's total unknown, and everything fed from it.
use std::collections::{BTreeMap, BTreeSet};

use crate::design::{Component, Design, PinKind};
use crate::finding::{Finding, Severity, Verdict};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, DriveKind, Library, Lookup, Part, Range};
use crate::policy::Policy;
use crate::voltage::{Driver, Hold, Voltages, resistance_range};

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
        Range::Unknown(_) | Range::None(_) => (None, None),
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

/// A regulator's `regulates` drive on `pin`.
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

/// A supply pin by its name, for symbols that draw their power pins as passive.
fn looks_like_supply(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    [
        "VDD", "VCC", "VIN", "VIO", "VBAT", "AVDD", "DVDD", "VS", "V+", "VPP", "VBUS",
    ]
    .iter()
    .any(|p| u.starts_with(p))
}

/// A two-pin part current flows straight through: an inductor or ferrite, a fuse, a jumper.
fn is_series_link(lib: &Library, c: &Component) -> bool {
    let class = part_of(lib, c).map(|p| p.class);
    if matches!(class, Some(Class::Led | Class::Diode | Class::Tvs | Class::Capacitor)) {
        return false;
    }
    c.pins.len() == 2
        && ((c.lib_id.starts_with("Device:L") && !c.lib_id.starts_with("Device:LED"))
            || c.lib_id.starts_with("Device:Ferrite")
            || crate::voltage::short_kind(c).is_some()
            || c.lib_id.starts_with("Device:Fuse")
            || c.lib_id.starts_with("Jumper:")
            || class == Some(Class::Inductor))
}

fn is_resistor(lib: &Library, c: &Component) -> bool {
    c.pins.len() == 2
        && !c.lib_id.starts_with("Device:RTD")
        && (c.lib_id.starts_with("Device:R") || part_of(lib, c).is_some_and(|p| p.class == Class::Resistor))
}

/// Adds a regulator channel's dissipation to the part's total; a dual regulator has one per output.
fn add_dissipation(map: &mut BTreeMap<String, Dissipation>, d: Dissipation) {
    match map.get_mut(&d.reference) {
        Some(e) => {
            e.watts = e.watts.zip(d.watts).map(|(a, b)| a + b);
            e.how = format!("{}; {}", e.how, d.how);
            e.missing.extend(d.missing);
        }
        None => {
            map.insert(d.reference.clone(), d);
        }
    }
}

/// Works out every rail's load and every part's dissipation.
pub fn compute(design: &Design, lib: &Library, inputs: &BoardInputs, volts: &Voltages) -> Thermal {
    let n = design.nets.len();
    let mut base = vec![Load::default(); n];
    let mut own: BTreeMap<String, Dissipation> = BTreeMap::new();
    let mut forward_through: BTreeMap<String, (Load, String)> = BTreeMap::new();
    // Resistors in an LED chain: their current is the LED's.
    let mut in_chain: BTreeSet<String> = BTreeSet::new();
    let vmax = |net: usize| volts.nets[net].span.max;
    let regulators: BTreeSet<&str> = volts.regulators.iter().map(|r| r.reference.as_str()).collect();

    // The rails an IC's supply entries are on, for current it sources from its outputs.
    let supply_rails = |c: &Component, part: &Part| -> Vec<usize> {
        let mut rails: Vec<usize> = part
            .supply
            .iter()
            .flat_map(|s| s.pins.iter().filter_map(|p| pin_net(c, part, p)))
            .filter(|&n| !is_ground(volts, n))
            .collect();
        rails.sort_unstable();
        rails.dedup();
        rails
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
                if part.drives.iter().any(|d| d.kind == DriveKind::Channel) {
                    // A MOSFET dissipates its channel current times RDS(on), not a supply current.
                    missing.push("the current through its channel isn't worked out".into());
                } else if part.supply.is_empty() {
                    missing.push(format!("{} gives no supply current", part.mpn));
                }
                for s in &part.supply {
                    covered.extend(s.pins.iter().map(String::as_str));
                    let Some(net) = s.pins.iter().find_map(|p| pin_net(c, part, p)) else {
                        missing.push(format!("its supply pins {} aren't on the schematic", s.pins.join(", ")));
                        continue;
                    };
                    let Some(amps) = s.max() else {
                        let what = format!("{reference} ({} gives no maximum for {})", part.mpn, s.pins.join(", "));
                        base[net].missing.push(what.clone());
                        missing.push(what);
                        continue;
                    };
                    base[net].add(amps, format!("{reference} {}{}", ma(amps), page_of(&s.current)));
                    match vmax(net) {
                        Some(volt) => {
                            watts += volt * amps;
                            how.push(format!("{} × {} on {}", ma(amps), v(volt), design.nets[net].name));
                        }
                        None => missing.push(format!("{} has no known voltage", design.nets[net].name)),
                    }
                }
                // Power pins with no supply current in the part file, including ones the symbol draws as
                // passive.
                let mut unknown_rails = BTreeSet::new();
                for (num, p) in &c.pins {
                    let name = part.pins.get(num).map_or(num.as_str(), String::as_str);
                    let power = p.kind == PinKind::PowerIn || looks_like_supply(name);
                    if power && !covered.contains(name) && !is_ground(volts, p.net) {
                        unknown_rails.insert(p.net);
                    }
                }
                for net in unknown_rails {
                    let what = format!("{reference} ({} gives no supply current for its pins here)", part.mpn);
                    base[net].missing.push(what.clone());
                    if !missing.contains(&what) && !part.supply.is_empty() {
                        missing.push(what);
                    }
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

    // LEDs: a resistor from a supply or an IC output to the anode, and the cathode on ground.
    let mut sourced: Vec<(String, f64, Vec<usize>, String)> = vec![];
    for (reference, c) in &design.components {
        let Some(part) = part_of(lib, c) else { continue };
        if part.class != Class::Led {
            continue;
        }
        let what = format!("{reference} ({})", part.mpn);
        let ends = forward(part).and_then(|(a, k, vf)| Some((pin_net(c, part, a)?, pin_net(c, part, k)?, vf)));
        let chain = match ends {
            Some((an, kn, vf)) if is_ground(volts, kn) => {
                let feeds: Vec<(&String, &Component)> = design.nets[an]
                    .pins
                    .iter()
                    .filter(|(r, _)| r != reference)
                    .filter_map(|(r, _)| design.components.get(r).map(|c| (r, c)))
                    .collect();
                match feeds.as_slice() {
                    [(r, rc)] if is_resistor(lib, rc) => rc
                        .pins
                        .values()
                        .map(|p| p.net)
                        .find(|&n| n != an)
                        .map(|far| (an, vf, (*r).clone(), *rc, far))
                        .ok_or_else(|| format!("{r} has both ends on its anode")),
                    _ => Err("its anode isn't fed through one resistor".to_string()),
                }
            }
            Some(_) => Err("its cathode isn't on ground".into()),
            None => Err(format!("{} gives no forward drive for its pins", part.mpn)),
        };
        let (an, vf, r, rc, far) = match chain {
            Ok(x) => x,
            Err(why) => {
                // Not worked out: every net it touches, and its own rating, are unknown.
                let text = format!("its current isn't worked out: {why}");
                // Every net it touches, directly or through one resistor, and the rails of any IC output there.
                let mut nets: BTreeSet<usize> = BTreeSet::new();
                for p in c.pins.values() {
                    nets.insert(p.net);
                    for (r, _) in &design.nets[p.net].pins {
                        if let Some(rc) = design
                            .components
                            .get(r)
                            .filter(|rc| r != reference && is_resistor(lib, rc))
                        {
                            nets.extend(rc.pins.values().map(|p| p.net));
                        }
                    }
                }
                let mut charged: BTreeSet<usize> = BTreeSet::new();
                for &net in &nets {
                    charged.insert(net);
                    for (dr, dp) in &design.nets[net].pins {
                        let Some(dc) = design.components.get(dr) else { continue };
                        let Some(dpart) = part_of(lib, dc) else { continue };
                        let out = dc.pins.get(dp).is_some_and(|p| {
                            matches!(p.kind, PinKind::Output | PinKind::Bidirectional | PinKind::TriState)
                        });
                        if dpart.class == Class::Ic && out {
                            charged.extend(supply_rails(dc, dpart));
                        }
                    }
                }
                for net in charged.into_iter().filter(|&n| !is_ground(volts, n)) {
                    base[net].missing.push(format!("{what}: {text}"));
                }
                forward_through.insert(
                    reference.clone(),
                    (
                        Load {
                            missing: vec![text.clone()],
                            ..Load::default()
                        },
                        text.clone(),
                    ),
                );
                own.insert(
                    reference.clone(),
                    Dissipation {
                        reference: reference.clone(),
                        watts: None,
                        how: text.clone(),
                        missing: vec![text],
                    },
                );
                continue;
            }
        };
        let _ = an;
        in_chain.insert(r.clone());
        let (vf_min, vf_max) = range_ends(vf);
        // VF's minimum is a figure at one current and 25 °C: lower at less current, and lower again when hot.
        // The only safe bound on the current is VF ≥ 0.
        let tested = part
            .drives
            .iter()
            .find(|d| d.kind == DriveKind::Forward)
            .and_then(|d| d.current);
        let (amps, text) = match (resistance_range(lib, inputs, &r, rc), vmax(far)) {
            (Ok((rmin, _, _)), Some(vs)) => {
                let i = (vs / rmin).max(0.0);
                let why = match (vf_min, tested) {
                    (Some(vf), Some(t)) => format!("its {} minimum holds only at {} and 25 °C", v(vf), ma(t)),
                    (Some(vf), None) => format!("its {} minimum holds only at its test current and 25 °C", v(vf)),
                    _ => "no minimum is given".into(),
                };
                (
                    Some(i),
                    format!(
                        "{} at most through {r} from {} ({} over {} Ω, counting VF as 0 V: {why})",
                        ma(i),
                        design.nets[far].name,
                        v(vs),
                        sig(rmin)
                    ),
                )
            }
            (Err(e), _) => (None, e),
            (_, None) => (None, format!("{} has no known voltage", design.nets[far].name)),
        };
        // An IC output driving the LED sources its current from that IC's supply. Which supply pin feeds the
        // output isn't in the part file, so every one of them is charged.
        let driver = design.nets[far].pins.iter().find_map(|(dr, dp)| {
            let dc = design.components.get(dr)?;
            let dpart = part_of(lib, dc)?;
            let kind = dc.pins.get(dp)?.kind;
            (dpart.class == Class::Ic && matches!(kind, PinKind::Output | PinKind::Bidirectional | PinKind::TriState))
                .then(|| (dr.clone(), supply_rails(dc, dpart)))
        });
        let mut load = Load::default();
        match (amps, &driver) {
            (Some(i), Some((d, rails))) if !rails.is_empty() => {
                let names: Vec<&str> = rails.iter().map(|&n| design.nets[n].name.as_str()).collect();
                for &rail in rails {
                    base[rail].add(i, format!("{what} {}, sourced by {d}", ma(i)));
                }
                sourced.push((d.clone(), i, rails.clone(), names.join(" or ")));
                load.add(i, text.clone());
            }
            (Some(i), _) => {
                base[far].add(i, format!("{what} {}", ma(i)));
                load.add(i, text.clone());
            }
            (None, _) => {
                let rail = match &driver {
                    Some((_, rails)) if !rails.is_empty() => rails.clone(),
                    _ => vec![far],
                };
                for r in rail {
                    base[r].missing.push(format!("{what}: {text}"));
                }
                load.missing.push(text.clone());
            }
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
    // The driving IC dissipates what its output drops, at most the whole rail voltage times the current.
    for (d, i, rails, names) in sourced {
        let Some(e) = own.get_mut(&d) else { continue };
        match rails.iter().map(|&r| vmax(r)).collect::<Option<Vec<f64>>>() {
            Some(vs) => {
                let top = vs.into_iter().fold(0.0_f64, f64::max);
                if let Some(w) = e.watts.as_mut() {
                    *w += top * i;
                }
                e.how = format!("{} + {} sourced at up to {} from {names}", e.how, ma(i), v(top));
            }
            None => e.missing.push(format!("{names} has no known voltage")),
        }
    }

    // Totals: a net's own draws plus everything fed from it through a regulator, diode, resistor or link.
    let ctx = Ctx {
        design,
        lib,
        inputs,
        volts,
        base: &base,
        in_chain: &in_chain,
    };
    let mut st = State {
        totals: vec![None; n],
        regs: BTreeMap::new(),
        forward: forward_through,
        own,
        stack: vec![],
    };
    for net in 0..n {
        ctx.total(net, &mut st);
    }
    let State {
        totals,
        regs,
        forward,
        mut own,
        ..
    } = st;
    own.extend(regs);
    Thermal {
        loads: totals.into_iter().map(Option::unwrap_or_default).collect(),
        parts: own,
        forward,
    }
}

struct Ctx<'a> {
    design: &'a Design,
    lib: &'a Library,
    inputs: &'a BoardInputs,
    volts: &'a Voltages,
    base: &'a [Load],
    in_chain: &'a BTreeSet<String>,
}

struct State {
    totals: Vec<Option<Load>>,
    regs: BTreeMap<String, Dissipation>,
    forward: BTreeMap<String, (Load, String)>,
    own: BTreeMap<String, Dissipation>,
    /// The nets being totalled, so a loop back through a link isn't followed.
    stack: Vec<usize>,
}

impl Ctx<'_> {
    /// `other` takes its current from `net` through a passive part: nothing drives it strongly, no IC output
    /// is on it, and its voltage comes from `net`.
    fn fed_from(&self, other: usize, net: usize) -> bool {
        let nv = &self.volts.nets[other];
        nv.driver != Driver::Strong
            && nv.why.iter().any(|w| w.from.contains(&net))
            && !self.design.nets[other].pins.iter().any(|(r, p)| {
                self.design
                    .components
                    .get(r)
                    .and_then(|c| c.pins.get(p))
                    .is_some_and(|p| {
                        matches!(
                            p.kind,
                            PinKind::Output | PinKind::Bidirectional | PinKind::TriState | PinKind::PowerOut
                        )
                    })
            })
    }

    /// `net` gets its voltage from `other` alone, never rising above it: nothing else feeds it.
    fn fed_only_from(&self, net: usize, other: usize) -> bool {
        let nv = &self.volts.nets[net];
        self.fed_from(net, other) && nv.why.iter().all(|w| w.bounded && w.from == [other])
    }

    /// `net` is held at `volts` or above by its own source: a regulator that holds it, or a board input.
    fn held_at_or_above(&self, net: usize, volts: f64) -> bool {
        if self.volts.nets[net].span.min.is_some_and(|m| m >= volts) {
            return true;
        }
        let held = self.volts.regulators.iter().any(|r| {
            r.output == net
                && r.sets_output
                && matches!(r.hold, Hold::Held(_))
                && r.held_at.is_some_and(|h| h.min >= volts)
        });
        let input = self.inputs.inputs.iter().any(|i| {
            i.fault.is_none()
                && i.min >= volts
                && self
                    .design
                    .components
                    .get(&i.pin.reference)
                    .and_then(|c| c.pins.get(&i.pin.pin))
                    .is_some_and(|p| p.net == net)
        });
        held || input
    }

    fn total(&self, net: usize, st: &mut State) -> Load {
        let (design, lib, volts) = (self.design, self.lib, self.volts);
        if let Some(l) = &st.totals[net] {
            return l.clone();
        }
        let mut load = self.base[net].clone();
        if st.stack.len() > 32 {
            load.missing.push("a supply chain too deep to follow".into());
            return load;
        }
        st.stack.push(net);
        // Regulators fed from this net.
        for reg in volts.regulators.iter().filter(|r| r.supply == net && r.sets_output) {
            let Some(c) = design.components.get(&reg.reference) else {
                continue;
            };
            let Some(part) = part_of(lib, c) else { continue };
            let label = format!("{} ({})", reg.reference, part.mpn);
            if st.stack.contains(&reg.output) {
                load.missing
                    .push(format!("{label}: its output loops back to its input"));
                continue;
            }
            let out = self.total(reg.output, st);
            let vin = volts.nets[net].span;
            let vout = volts.nets[reg.output].span;
            let head = regulator_drive(part, &reg.pin).and_then(|d| d.headroom.as_ref());
            // A buck converter: a duty-cycle headroom, or a pin other than its input and output on a net with an
            // inductor (its switch node).
            let switches = head.is_some_and(|h| h.duty < 1.0)
                || c.pins
                    .values()
                    .filter(|p| p.net != net && p.net != reg.output)
                    .any(|p| {
                        design.nets[p.net].pins.iter().any(|(r, _)| {
                            design.components.get(r).is_some_and(|l| {
                                (l.lib_id.starts_with("Device:L") && !l.lib_id.starts_with("Device:LED"))
                                    || part_of(lib, l).is_some_and(|lp| lp.class == Class::Inductor)
                            })
                        })
                    });
            // Linear only when the part file gives a dropout headroom and the output stays below the supply's top;
            // anything else (a boost, a charge pump) isn't worked out.
            let linear = !switches && head.is_some() && vout.max.zip(vin.max).is_some_and(|(o, i)| o <= i);
            if switches {
                let eff = part.efficiency.as_ref().map(|e| e.value);
                match (eff, vin.min, vout.max) {
                    (Some(eta), Some(vi), Some(vo)) if vi > 0.0 => {
                        let iin = out.amps * vo / (eta * vi);
                        let mut fed = out.clone();
                        fed.amps = iin;
                        load.take(&fed, &format!("{label} input for"));
                        let pout = out.amps * vo;
                        add_dissipation(
                            &mut st.regs,
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
                        load.missing.push(format!(
                            "{label}: a switching regulator's input current needs its efficiency"
                        ));
                        add_dissipation(
                            &mut st.regs,
                            Dissipation {
                                reference: reg.reference.clone(),
                                watts: None,
                                how: format!("supplies {} on {}", ma(out.amps), design.nets[reg.output].name),
                                missing: vec![format!("{} gives no efficiency", part.mpn)],
                            },
                        );
                    }
                }
            } else if linear {
                load.take(&out, &format!("{label} output"));
                // Its own supply current counts once, not once per output.
                let iq: f64 = if st.regs.contains_key(&reg.reference) {
                    0.0
                } else {
                    part.supply.iter().filter_map(crate::limits::SupplyCurrent::max).sum()
                };
                // The drop is largest at the top of the supply. When the output isn't held over the whole supply
                // range but is at its top, use the held output there: below the supply it needs, a linear
                // regulator is in dropout and drops less than it does while regulating.
                let vo_low = match (reg.held_at, vin.max) {
                    (Some(h), Some(vi)) if h.needs.is_some_and(|n| vi >= n) && vout.min.is_none_or(|m| m < h.min) => {
                        Some(h.min)
                    }
                    _ => vout.min,
                };
                let watts = match (vin.max, vo_low) {
                    (Some(vi), Some(vo)) => Some((vi - vo).max(0.0) * out.amps + vi * iq),
                    _ => None,
                };
                add_dissipation(
                    &mut st.regs,
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
                                    " (the output is held at the top of {}; below the {} it needs, the regulator is \
                                     in dropout and drops less)",
                                    design.nets[net].name,
                                    v(reg.held_at.and_then(|h| h.needs).unwrap_or(0.0))
                                )
                            }
                        ),
                        missing: out.missing.clone(),
                    },
                );
            } else {
                let why = if head.is_none() {
                    format!(
                        "{} gives no dropout headroom, so it can't be told from a switching regulator",
                        part.mpn
                    )
                } else {
                    format!(
                        "its output can be above {}, so it isn't a linear regulator",
                        design.nets[net].name
                    )
                };
                load.missing.push(format!("{label}: {why}"));
                add_dissipation(
                    &mut st.regs,
                    Dissipation {
                        reference: reg.reference.clone(),
                        watts: None,
                        how: format!("supplies {} on {}", ma(out.amps), design.nets[reg.output].name),
                        missing: vec![why],
                    },
                );
            }
        }
        // Diodes in series: anode on this net, cathode on a net they feed, and able to conduct.
        for (reference, c) in &design.components {
            let Some(part) = part_of(lib, c) else { continue };
            if part.class != Class::Diode {
                continue;
            }
            let Some((a, k, vf)) = forward(part) else { continue };
            let (Some(an), Some(kn)) = (pin_net(c, part, a), pin_net(c, part, k)) else {
                continue;
            };
            if an != net || kn == net || is_ground(volts, kn) || is_ground(volts, an) || st.stack.contains(&kn) {
                continue;
            }
            // Never forward biased: the anode stays at or below the cathode (a reverse-protection or clamp diode).
            // The cathode's own source counts here, not the range the diode itself adds to it.
            let amax = volts.nets[an].span.max;
            if amax.is_some_and(|a| self.held_at_or_above(kn, a)) {
                continue;
            }
            if amax.is_none() || volts.nets[kn].span.min.is_none() {
                let why = format!(
                    "{reference} ({}): whether it conducts from {} to {} isn't known, as their voltages aren't",
                    part.mpn, design.nets[an].name, design.nets[kn].name
                );
                load.missing.push(why.clone());
                st.forward.insert(
                    reference.clone(),
                    (
                        Load {
                            missing: vec![why.clone()],
                            ..Load::default()
                        },
                        why,
                    ),
                );
                continue;
            }
            let fed = self.total(kn, st);
            // Only a rail that something draws from: a clamp or flyback diode carries no steady current.
            if fed.parts.is_empty() && fed.missing.is_empty() {
                continue;
            }
            let label = format!("{reference} ({})", part.mpn);
            load.take(&fed, &format!("{label} to {}", design.nets[kn].name));
            let (_, vf_max) = range_ends(vf);
            st.own.insert(
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
            st.forward.insert(
                reference.clone(),
                (fed.clone(), format!("{} into {}", ma(fed.amps), design.nets[kn].name)),
            );
        }
        // Resistors, inductors, ferrites, fuses and jumpers: a net fed through one passes its load on; a resistor
        // between two driven nets carries at most their difference over its resistance.
        for (reference, c) in &design.components {
            if c.dnp || c.pins.len() != 2 || self.in_chain.contains(reference) {
                continue;
            }
            let resistor = is_resistor(lib, c);
            if !resistor && !is_series_link(lib, c) {
                continue;
            }
            let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
            let other = match (nets[0] == net, nets[1] == net) {
                (true, false) => nets[1],
                (false, true) => nets[0],
                _ => continue,
            };
            // A net past a DC short (bead, fuse, net tie) takes this net's range, so it counts as driven, but
            // everything on it still draws through the short.
            let through_short =
                crate::voltage::short_kind(c).is_some() && volts.nets[other].why.iter().any(|w| w.from.contains(&net));
            if !is_ground(volts, other) && !st.stack.contains(&other) {
                if through_short || self.fed_from(other, net) {
                    let fed = self.total(other, st);
                    if !fed.parts.is_empty() || !fed.missing.is_empty() {
                        load.take(&fed, &format!("{reference} to {}", design.nets[other].name));
                        continue;
                    }
                } else if volts.nets[other].driver != Driver::Strong
                    && volts.nets[other].why.iter().all(|w| w.from.is_empty())
                {
                    // A net whose voltage gerberus couldn't trace may still draw through this part.
                    let fed = self.total(other, st);
                    if !fed.parts.is_empty() || !fed.missing.is_empty() {
                        load.missing.push(format!(
                            "{reference}: {} may draw {} or more through it, but its voltage isn't known",
                            design.nets[other].name,
                            ma(fed.amps)
                        ));
                        continue;
                    }
                }
            }
            // The resistor this net is fed through, from `other` alone, brings current in; it isn't a load.
            if !resistor || st.stack.contains(&other) || self.fed_only_from(net, other) {
                continue;
            }
            match (
                resistance_range(lib, self.inputs, reference, c),
                volts.nets[net].span.max,
                volts.nets[other].span.min,
            ) {
                (Ok((rmin, _, _)), Some(a), Some(b)) => {
                    let i = (a - b) / rmin;
                    if i > 0.0 {
                        load.add(i, format!("{reference} {} to {}", ma(i), design.nets[other].name));
                    }
                }
                (Err(e), ..) => load.missing.push(format!("{reference}: {e}")),
                _ => load.missing.push(format!(
                    "{reference}: {} or {} has no known voltage",
                    design.nets[net].name, design.nets[other].name
                )),
            }
        }
        st.stack.pop();
        st.totals[net] = Some(load.clone());
        load
    }
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
            headline: None,
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
        // Without a board temperature the margin can't be judged either: UNKNOWN, with the ambient it holds to.
        let (verdict, end) = match margin {
            Some(m) if !board && verdict == Verdict::Pass && tj > max.value - m.margin => (
                Verdict::Unknown,
                format!(
                    "{end}, but within policy.toml's {} °C margin: it holds only up to {:.0} °C ambient, and \
                     inputs.toml gives no board temperature to compare",
                    m.margin,
                    up_to - m.margin
                ),
            ),
            _ => (verdict, end),
        };
        let (verdict, end) = if verdict == Verdict::Pass && !d.missing.is_empty() {
            (Verdict::Unknown, format!("{end}, but only counting what is known"))
        } else {
            (verdict, end)
        };
        let mut f = finding(verdict, severity, format!("{text}{end}"));
        if verdict == Verdict::Fail {
            let past = if tj > max.value {
                "above".to_string()
            } else {
                format!("within {:.0} °C of", max.value - tj)
            };
            f.headline = Some(format!(
                "{reference} dissipates {}{lower}, so its junction reaches {tj:.0} °C at {ambient} °C ambient, {past} its \
                 {} °C maximum",
                mw(watts),
                max.value
            ));
        }
        r.push(f);
    }
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
            headline: None,
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
