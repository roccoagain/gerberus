//! Turns a design into an ngspice netlist for one run of the sweep.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use super::{Rng, SpicePolicy, parse_farads, parse_henries, part_of, sig};
use crate::design::{Component, Design};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, DriveKind, Library, Part, Quantity, Range};
use crate::voltage::{Voltages, parse_ohms, resistance_range};

/// A part value: fixed, or drawn from `[lo, hi]` on each run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Fixed(f64),
    Range { nominal: f64, lo: f64, hi: f64 },
}

impl Value {
    fn draw(self, rng: Option<&mut Rng>) -> f64 {
        match (self, rng) {
            (Value::Fixed(v), _) => v,
            (Value::Range { nominal, .. }, None) => nominal,
            (Value::Range { lo, hi, .. }, Some(r)) => r.between(lo, hi),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Element {
    R {
        name: String,
        a: usize,
        b: usize,
        ohms: Value,
    },
    C {
        name: String,
        a: usize,
        b: usize,
        farads: Value,
    },
    L {
        name: String,
        a: usize,
        b: usize,
        henries: Value,
    },
    /// A diode from `a` (anode) to `k`, with its model card.
    D {
        name: String,
        a: usize,
        k: usize,
        model: String,
    },
    /// A regulator: holds `output` at `set`, or `drop` below `input` when that is lower.
    Reg {
        name: String,
        input: usize,
        output: usize,
        set: Value,
        drop: f64,
        switching: bool,
    },
    /// An IC's supply current, drawn in full once its rail reaches `on` volts.
    Load {
        name: String,
        rail: usize,
        amps: f64,
        on: f64,
    },
}

/// A supply input switched on through the cable.
#[derive(Debug, Clone, PartialEq)]
struct Source {
    name: String,
    pos: usize,
    neg: usize,
    volts: f64,
}

/// A design as ngspice elements.
#[derive(Debug, Clone)]
pub struct Circuit {
    ground: Vec<bool>,
    elements: Vec<Element>,
    sources: Vec<Source>,
    /// Parts left out, with why.
    skipped: Vec<String>,
    /// For each net, the first part on it that isn't modelled.
    unmodelled: BTreeMap<usize, String>,
    /// For each net, the lowest voltage the static analysis gives it once powered, when that is a firm value.
    expected: Vec<Option<f64>>,
}

fn is_ground(volts: &Voltages, net: usize) -> bool {
    volts.nets[net].span.min == Some(0.0) && volts.nets[net].span.max == Some(0.0)
}

/// A TVS diode's resistance past breakdown, in ohms, when its part file gives no `clamp` figure: a soft guess
/// on the pessimistic side (the SMAJ18A's (29.2 V − 22.1 V) / 13.7 A is 0.52 Ω; small ESD diodes are about
/// 1 Ω). A softer clamp lets the rail behind it go higher.
pub const TVS_OHMS: f64 = 1.0;

/// A TVS diode's resistance past breakdown: from its clamping voltage at its pulse current, measured from the
/// top of its breakdown range, or [`TVS_OHMS`].
fn tvs_ohms(l: &crate::limits::Limit, bv: f64) -> f64 {
    l.clamp
        .as_ref()
        .map(|c| (c.volts - bv) / c.current)
        .filter(|r| r.is_finite() && *r > 0.0)
        .unwrap_or(TVS_OHMS)
}

/// The resistance an IC's forward drive (a transistor's body diode, such as a P-FET reverse-polarity switch)
/// is modelled with: its channel switched on, bypassing the diode, so what passes through is as high as it
/// can be.
pub const SWITCH_OHMS: f64 = 0.05;

/// A part's forward drop and its test current, from its forward drive.
fn forward(part: Option<&Part>, led: bool) -> (f64, f64) {
    let drive = part.and_then(|p| p.drives.iter().find(|d| d.kind == DriveKind::Forward));
    let vf = drive
        .and_then(|d| match &d.range {
            Range::Known { min, max, .. } => match (min, max) {
                (Bound::Value(v), _) | (_, Bound::Value(v)) => Some(*v),
                _ => None,
            },
            Range::Unknown(_) => None,
        })
        .unwrap_or(if led { 1.8 } else { 0.7 });
    let at = drive.and_then(|d| d.current).unwrap_or(if led { 0.02 } else { 1.0 });
    (vf, at)
}

/// A diode's model card: `vf` volts at `at` amps forward, series resistance `rs`, and a reverse breakdown.
fn diode_model(name: &str, vf: f64, at: f64, led: bool, rs: f64, bv: Option<f64>) -> String {
    let n = if led { 2.0 } else { 1.0 };
    let vt = 0.025_85;
    let is = at / ((vf / (n * vt)).exp() - 1.0);
    let bv = bv.map(|v| format!(" BV={} IBV=1m", sig(v))).unwrap_or_default();
    format!(".model {name} D(IS={is:e} N={n} RS={}{bv})", sig(rs))
}

/// The voltage a protection limit breaks down at: its maximum, as a part can clamp that high, or its minimum
/// when only that is given.
fn breakdown_at(r: &Range) -> Option<f64> {
    match r {
        Range::Known { min, max, .. } => match (max, min) {
            (Bound::Value(v), _) | (_, Bound::Value(v)) => Some(*v),
            _ => None,
        },
        Range::Unknown(_) => None,
    }
}

/// The pair of nets `V(a) − V(b)` is measured on, with ground written as `None`; `None` when the difference is
/// always 0.
pub type Pair = (usize, Option<usize>);

impl Circuit {
    /// Whether a net is the simulation's ground.
    pub fn is_ground(&self, net: usize) -> bool {
        self.ground.get(net).copied().unwrap_or(false)
    }

    /// The first part on a net that isn't modelled.
    pub fn unmodelled_on(&self, net: usize) -> Option<&str> {
        self.unmodelled.get(&net).map(String::as_str)
    }

    /// Parts left out of the simulation, with why.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// The inputs switched on, for the report.
    pub fn inputs(&self, design: &Design) -> Vec<String> {
        self.sources
            .iter()
            .map(|s| format!("{} {} V on {}", s.name, sig(s.volts), design.nets[s.pos].name))
            .collect()
    }

    /// The net a part's pin is on, by the pin's datasheet name.
    pub fn pin_net(&self, c: &Component, part: &Part, name: &str) -> Option<usize> {
        part.pins_named(name)
            .into_iter()
            .find_map(|num| c.pins.get(num).map(|p| p.net))
            .or_else(|| c.pins.get(name).map(|p| p.net))
    }

    /// The pair `V(a) − V(b)` is measured on, with ground as `None`; `None` when it is always 0.
    pub fn key(&self, a: usize, b: usize) -> Option<Pair> {
        let b = (!self.is_ground(b)).then_some(b);
        (Some(a) != b && !(self.is_ground(a) && b.is_none())).then_some((a, b))
    }

    /// The lowest voltage the static analysis says a net reaches once powered, when it gives a firm one.
    pub fn expected(&self, net: usize) -> Option<f64> {
        self.expected.get(net).copied().flatten()
    }

    /// Every pair of nets a voltage limit reads: each pin against the limit's reference, and against any
    /// pin a bound is relative to (`max = VDD + 0.3`).
    pub fn pairs(&self, design: &Design, lib: &Library) -> Vec<Pair> {
        let mut out: BTreeSet<Pair> = BTreeSet::new();
        for c in design.components.values() {
            let Some(part) = part_of(lib, c) else { continue };
            for l in part.limits.iter().filter(|l| l.quantity == Quantity::Voltage) {
                let Some(to) = l.to.as_deref() else { continue };
                let Some(b) = self.pin_net(c, &part, to) else { continue };
                let relative: Vec<usize> = [&l.abs_max, &l.recommended]
                    .into_iter()
                    .filter_map(|r| match r {
                        Range::Known { min, max, .. } => Some([min, max]),
                        Range::Unknown(_) => None,
                    })
                    .flatten()
                    .filter_map(|bound| match bound {
                        Bound::Pin { pin, .. } => self.pin_net(c, &part, pin),
                        _ => None,
                    })
                    .collect();
                for p in &l.pins {
                    let Some(a) = self.pin_net(c, &part, p) else { continue };
                    for r in std::iter::once(b).chain(relative.iter().copied()) {
                        out.extend(self.key(a, r));
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// Every net a pair touches, measured from ground, so a check can tell whether the run powered it.
    pub fn with_nets(&self, pairs: &[Pair]) -> Vec<Pair> {
        let mut all: BTreeSet<Pair> = pairs.iter().copied().collect();
        for &(a, b) in pairs {
            for n in std::iter::once(a).chain(b) {
                if !self.is_ground(n) {
                    all.insert((n, None));
                }
            }
        }
        all.into_iter().collect()
    }

    fn node(&self, net: usize) -> String {
        if self.is_ground(net) {
            "0".into()
        } else {
            format!("n{net}")
        }
    }

    /// One run's netlist, with a measurement for every pair, and a line saying what was drawn.
    pub fn netlist(
        &self,
        policy: &SpicePolicy,
        pairs: &[(usize, Option<usize>)],
        mut rng: Option<&mut Rng>,
    ) -> (String, String) {
        let mut s = String::from("* gerberus hot-plug run\n.option rshunt=1e9\n");
        let [lmin, lmax] = policy.cable_inductance;
        let cable = match rng.as_deref_mut() {
            None => lmax,
            Some(r) => r.between(lmin, lmax),
        };
        let about = format!("cable {} µH", sig(cable * 1e6));
        for (i, src) in self.sources.iter().enumerate() {
            let _ = writeln!(
                s,
                "Vin{i} s{i} {} PWL(0 0 {} {})\nRcab{i} s{i} m{i} {}\nLcab{i} m{i} {} {cable:e}",
                self.node(src.neg),
                policy.rise_time,
                src.volts,
                policy.cable_resistance,
                self.node(src.pos)
            );
        }
        let mut models = BTreeSet::new();
        for e in &self.elements {
            match e {
                Element::R { name, a, b, ohms } => {
                    let _ = writeln!(
                        s,
                        "R{name} {} {} {:e}",
                        self.node(*a),
                        self.node(*b),
                        ohms.draw(rng.as_deref_mut())
                    );
                }
                Element::C { name, a, b, farads } => {
                    let _ = writeln!(
                        s,
                        "C{name} {} {} {:e}",
                        self.node(*a),
                        self.node(*b),
                        farads.draw(rng.as_deref_mut())
                    );
                }
                Element::L { name, a, b, henries } => {
                    // A little winding resistance keeps the matrix well behaved.
                    let _ = writeln!(
                        s,
                        "L{name} {} x{name} {:e}\nRL{name} x{name} {} 0.01",
                        self.node(*a),
                        henries.draw(rng.as_deref_mut()),
                        self.node(*b)
                    );
                }
                Element::D { name, a, k, model } => {
                    models.insert(model.clone());
                    let card = model.split_whitespace().nth(1).unwrap_or("");
                    let _ = writeln!(s, "D{name} {} {} {card}", self.node(*a), self.node(*k));
                }
                Element::Reg {
                    name,
                    input,
                    output,
                    set,
                    drop,
                    switching,
                } => {
                    let (i, o) = (self.node(*input), self.node(*output));
                    let v = set.draw(rng.as_deref_mut());
                    let draw = if *switching {
                        // Power in = power out (100 % efficiency): the least it can draw.
                        format!("max(i(VS{name}),0)*v({o})/max(v({i}),1)")
                    } else {
                        format!("max(i(VS{name}),0)")
                    };
                    let _ = writeln!(
                        s,
                        "BR{name} y{name} 0 V=min({v}, max(v({i})-{drop}, 0))\nRR{name} y{name} z{name} 0.01\n\
                         VS{name} z{name} {o} 0\nBI{name} {i} 0 I={draw}"
                    );
                }
                Element::Load { name, rail, amps, on } => {
                    let r = self.node(*rail);
                    let _ = writeln!(s, "BL{name} {r} 0 I={amps:e}*min(1, max(v({r}),0)/{on})");
                }
            }
        }
        for m in models {
            let _ = writeln!(s, "{m}");
        }
        // Every measured net gets a path to ground, so it exists in ngspice even when no modelled part
        // touches it (a measurement on a missing node fails).
        let nets: BTreeSet<usize> = pairs
            .iter()
            .flat_map(|&(a, b)| std::iter::once(a).chain(b))
            .filter(|&n| !self.is_ground(n))
            .collect();
        for n in nets {
            let _ = writeln!(s, "Rmeas{n} n{n} 0 1e9");
        }
        let step = (policy.duration / 20_000.0).min(policy.rise_time / 4.0);
        let _ = writeln!(s, ".tran {step:e} {:e} 0 {step:e}", policy.duration);
        for (i, &(a, b)) in pairs.iter().enumerate() {
            // ngspice's .meas takes a difference only as an expression (`v(a,b)` and `-v(b)` fail), and allows
            // at most 99 of those, so a difference is formed by a source on a node of its own.
            let v = match b {
                None => format!("v({})", self.node(a)),
                Some(b) => {
                    let _ = writeln!(
                        s,
                        "Bp{i} d{i} 0 V=v({})-v({})\nRp{i} d{i} 0 1e9",
                        self.node(a),
                        self.node(b)
                    );
                    format!("v(d{i})")
                }
            };
            let _ = writeln!(
                s,
                ".meas tran p{i}max MAX {v}\n.meas tran p{i}min MIN {v}\n.meas tran p{i}end FIND {v} AT={:e}",
                policy.duration
            );
        }
        s.push_str(".end\n");
        (s, about)
    }
}

fn not_a_supply(input: &crate::inputs::Input) -> bool {
    input.min <= 0.0
}

/// Builds the circuit: every part gerberus can model, and the board's supply inputs.
#[allow(clippy::too_many_lines)]
pub fn build(design: &Design, lib: &Library, inputs: &BoardInputs, volts: &Voltages) -> Result<Circuit, String> {
    let n = design.nets.len();
    let mut c = Circuit {
        ground: (0..n).map(|i| is_ground(volts, i)).collect(),
        elements: vec![],
        sources: vec![],
        skipped: vec![],
        unmodelled: BTreeMap::new(),
        expected: volts
            .nets
            .iter()
            .map(|n| n.span.min.filter(|&v| v > 0.0 && !n.floor))
            .collect(),
    };
    // Supply inputs (a positive minimum) are switched on; signal inputs are left open, as their drivers'
    // own resistance and slew aren't known.
    for input in inputs.inputs.iter().filter(|i| i.fault.is_none()) {
        if not_a_supply(input) {
            c.skipped.push(format!(
                "input {}: a signal, not a supply, so it is left open",
                input.name
            ));
            continue;
        }
        let net = |p: &crate::inputs::PinRef| {
            design
                .components
                .get(&p.reference)
                .and_then(|c| c.pins.get(&p.pin))
                .map(|p| p.net)
        };
        let (Some(pos), Some(neg)) = (net(&input.pin), net(&input.to)) else {
            continue;
        };
        let Some(v) = input.max.value() else {
            c.skipped
                .push(format!("input {}: its voltage follows another pin", input.name));
            continue;
        };
        if !c.is_ground(neg) || c.is_ground(pos) {
            c.skipped
                .push(format!("input {}: it isn't measured from ground", input.name));
            continue;
        }
        match c.sources.iter_mut().find(|s| s.pos == pos) {
            Some(s) if s.volts >= v => {}
            Some(s) => {
                s.volts = v;
                s.name.clone_from(&input.name);
            }
            None => c.sources.push(Source {
                name: input.name.clone(),
                pos,
                neg,
                volts: v,
            }),
        }
    }
    if c.sources.is_empty() {
        return Err("inputs.toml gives no supply input (one with a positive minimum) to switch on".into());
    }
    let regulators: BTreeSet<&str> = volts
        .regulators
        .iter()
        .filter(|r| r.sets_output)
        .map(|r| r.reference.as_str())
        .collect();
    for (reference, comp) in &design.components {
        if comp.dnp || comp.pins.len() < 2 {
            continue;
        }
        let part = part_of(lib, comp);
        let class = part.as_ref().map(|p| p.class);
        let lib_id = comp.lib_id.as_str();
        let nets: Vec<usize> = comp.pins.values().map(|p| p.net).collect();
        let two = comp.pins.len() == 2;
        let skip = |c: &mut Circuit, why: String| {
            for &net in &nets {
                if !c.is_ground(net) {
                    c.unmodelled.entry(net).or_insert_with(|| reference.clone());
                }
            }
            c.skipped.push(format!("{reference}: {why}"));
        };
        let ferrite = lib_id.starts_with("Device:Ferrite") || lib_id.starts_with("Device:FerriteBead");
        let short = lib_id.starts_with("Jumper:")
            || lib_id.starts_with("Device:Fuse")
            || lib_id.starts_with("Device:Polyfuse")
            || lib_id.starts_with("Device:NetTie")
            || ferrite;
        if two && short {
            // Closed: the worst case for stress, as nothing is cut off.
            c.elements.push(Element::R {
                name: reference.clone(),
                a: nets[0],
                b: nets[1],
                ohms: Value::Fixed(if ferrite { 0.05 } else { 0.001 }),
            });
            continue;
        }
        let resistor = class == Some(Class::Resistor) || (class.is_none() && lib_id.starts_with("Device:R"));
        let capacitor = class == Some(Class::Capacitor) || (class.is_none() && lib_id.starts_with("Device:C"));
        let inductor = class == Some(Class::Inductor)
            || (class.is_none() && lib_id.starts_with("Device:L") && !lib_id.starts_with("Device:LED"));
        if two && resistor {
            let ohms = match resistance_range(lib, inputs, reference, comp) {
                Ok((lo, hi, _)) => Some(Value::Range {
                    nominal: f64::midpoint(lo, hi),
                    lo,
                    hi,
                }),
                Err(_) => parse_ohms(&comp.value).filter(|&v| v > 0.0).map(Value::Fixed),
            };
            match ohms {
                Some(ohms) => c.elements.push(Element::R {
                    name: reference.clone(),
                    a: nets[0],
                    b: nets[1],
                    ohms,
                }),
                None => skip(&mut c, format!("can't read a resistance from {:?}", comp.value)),
            }
            continue;
        }
        if two && capacitor {
            match parse_farads(&comp.value) {
                Some(f) => {
                    let farads = match part.as_ref().and_then(|p| p.tolerance.as_ref()) {
                        Some(t) => Value::Range {
                            nominal: f,
                            lo: f * (1.0 - t.value),
                            hi: f * (1.0 + t.value),
                        },
                        None => Value::Fixed(f),
                    };
                    c.elements.push(Element::C {
                        name: reference.clone(),
                        a: nets[0],
                        b: nets[1],
                        farads,
                    });
                }
                None => skip(&mut c, format!("can't read a capacitance from {:?}", comp.value)),
            }
            continue;
        }
        if two && inductor {
            match parse_henries(&comp.value) {
                Some(h) => {
                    let henries = match part.as_ref().and_then(|p| p.tolerance.as_ref()) {
                        Some(t) => Value::Range {
                            nominal: h,
                            lo: h * (1.0 - t.value),
                            hi: h * (1.0 + t.value),
                        },
                        None => Value::Fixed(h),
                    };
                    c.elements.push(Element::L {
                        name: reference.clone(),
                        a: nets[0],
                        b: nets[1],
                        henries,
                    });
                }
                None => skip(&mut c, format!("can't read an inductance from {:?}", comp.value)),
            }
            continue;
        }
        match (part.as_deref(), class) {
            (Some(p), Some(Class::Diode | Class::Led)) => {
                let Some(d) = p.drives.iter().find(|d| d.kind == DriveKind::Forward) else {
                    skip(&mut c, format!("{} gives no forward drop", p.mpn));
                    continue;
                };
                let (Some(a), Some(k)) = (c.pin_net(comp, p, &d.pin), c.pin_net(comp, p, &d.to)) else {
                    skip(&mut c, "its anode or cathode isn't on the schematic".into());
                    continue;
                };
                let led = class == Some(Class::Led);
                let (vf, at) = forward(Some(p), led);
                let model = diode_model(&format!("DM_{reference}"), vf, at, led, 0.05, None);
                c.elements.push(Element::D {
                    name: reference.clone(),
                    a,
                    k,
                    model,
                });
            }
            (Some(p), Some(Class::Tvs)) => {
                let mut any = false;
                let (vf, at) = forward(Some(p), false);
                for (li, l) in p.limits.iter().enumerate() {
                    let Some(bv) = l.breakdown.as_ref().and_then(breakdown_at) else {
                        continue;
                    };
                    let Some(to) = l.to.as_deref() else { continue };
                    let Some(an) = c.pin_net(comp, p, to) else { continue };
                    for (j, pin) in l.pins.iter().enumerate() {
                        let Some(kn) = c.pin_net(comp, p, pin) else { continue };
                        let name = format!("{reference}_{li}_{j}");
                        let model = diode_model(&format!("DM_{name}"), vf, at, false, tvs_ohms(l, bv), Some(bv));
                        any = true;
                        if l.bidirectional {
                            // Two diodes back to back, each breaking down at the rating.
                            let mid = format!("{name}m");
                            c.elements.push(Element::D {
                                name: format!("{name}a"),
                                a: an,
                                k: usize::MAX,
                                model: format!("{model} ;{mid}"),
                            });
                            c.elements.push(Element::D {
                                name: format!("{name}b"),
                                a: kn,
                                k: usize::MAX,
                                model: format!("{model} ;{mid}"),
                            });
                        } else {
                            c.elements.push(Element::D {
                                name,
                                a: an,
                                k: kn,
                                model,
                            });
                        }
                    }
                }
                if !any {
                    skip(&mut c, format!("{} gives no breakdown voltage", p.mpn));
                }
            }
            (Some(p), Some(Class::Ic | Class::Oscillator)) => {
                if regulators.contains(reference.as_str()) {
                    for (k, r) in volts
                        .regulators
                        .iter()
                        .filter(|r| &r.reference == reference && r.sets_output)
                        .enumerate()
                    {
                        let Some(h) = r.held_at else {
                            skip(&mut c, "its output range isn't known".into());
                            c.unmodelled.entry(r.output).or_insert_with(|| reference.clone());
                            continue;
                        };
                        let head = p
                            .drives
                            .iter()
                            .find(|d| d.kind == DriveKind::Regulates && d.pin == r.pin)
                            .and_then(|d| d.headroom.as_ref());
                        let switching = head.is_some_and(|h| h.duty < 1.0);
                        // The model can only step down: its output follows its input when that is lower.
                        if volts.nets[r.supply].span.max.is_some_and(|v| h.min > v) {
                            skip(&mut c, "it steps its input up, which isn't modelled".into());
                            c.unmodelled.entry(r.output).or_insert_with(|| reference.clone());
                            continue;
                        }
                        c.elements.push(Element::Reg {
                            name: format!("{reference}_{k}"),
                            input: r.supply,
                            output: r.output,
                            set: Value::Range {
                                nominal: f64::midpoint(h.min, h.max),
                                lo: h.min,
                                hi: h.max,
                            },
                            drop: h.needs.map_or(0.0, |n| (n - h.max).max(0.0)),
                            switching,
                        });
                    }
                }
                // A transistor's forward drive (a P-FET's body diode, which its channel bypasses once switched
                // on) passes power on: it is the channel, switched on.
                if !regulators.contains(reference.as_str()) {
                    for (k, d) in p.drives.iter().filter(|d| d.kind == DriveKind::Forward).enumerate() {
                        let (Some(a), Some(b)) = (c.pin_net(comp, p, &d.pin), c.pin_net(comp, p, &d.to)) else {
                            continue;
                        };
                        c.elements.push(Element::R {
                            name: format!("{reference}_f{k}"),
                            a,
                            b,
                            ohms: Value::Fixed(SWITCH_OHMS),
                        });
                    }
                }
                // A switch node swings between the supply and ground at the switching frequency; the
                // regulator here is an averaged source, so those pins aren't modelled.
                for d in p.drives.iter().filter(|d| d.kind == DriveKind::Switches) {
                    if let Some(net) = c.pin_net(comp, p, &d.pin) {
                        c.unmodelled
                            .entry(net)
                            .or_insert_with(|| format!("{reference}'s switching on {}", d.pin));
                    }
                }
                for (k, s) in p.supply.iter().enumerate() {
                    let (Some(rail), Some(amps)) = (s.pins.iter().find_map(|q| c.pin_net(comp, p, q)), s.max()) else {
                        continue;
                    };
                    if c.is_ground(rail) {
                        continue;
                    }
                    let on = volts.nets[rail].span.min.filter(|&v| v > 0.1).unwrap_or(1.0);
                    c.elements.push(Element::Load {
                        name: format!("{reference}_{k}"),
                        rail,
                        amps,
                        on,
                    });
                }
            }
            (_, Some(Class::Connector)) => {}
            _ if lib_id.starts_with("Connector")
                || lib_id.starts_with("TestPoint")
                || lib_id.starts_with("Mechanical") => {}
            (None, _) if lib_id.starts_with("Device:D") || lib_id.starts_with("Device:LED") => {
                skip(&mut c, "no part file, so its forward drop isn't known".into());
            }
            _ => skip(&mut c, "gerberus has no model for it".into()),
        }
    }
    // Bidirectional TVS pairs share a middle node, written into the model text after ';'.
    let mut mids: BTreeMap<String, usize> = BTreeMap::new();
    for e in &mut c.elements {
        if let Element::D { k, model, .. } = e
            && *k == usize::MAX
        {
            let (card, mid) = model
                .split_once(" ;")
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap_or_default();
            let next = n + mids.len();
            *k = *mids.entry(mid).or_insert(next);
            *model = card;
        }
    }
    c.ground.resize(n + mids.len(), false);
    Ok(c)
}
