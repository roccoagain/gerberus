//! Signal levels: how far the signals in `inputs.toml` swing each net, worked out in ngspice.
//!
//! Each `[[signal]]` is an AC source between its two pins. Every rail (a net the voltage engine knows both ends
//! of, held by an input, a regulator or a diode) is a DC source. The circuit is linear, so ngspice finds each
//! net's bias with every rail at its lowest, and then how far each rail moves it on its own going to its highest;
//! the worst case of any mix of rail ends follows from those. It also finds each net's gain from each signal
//! across the signal's band (`.ac`), and the gain of the difference between the two pins of every limit
//! directly, so their phases count. A pin then reaches its bias plus and minus the sum of each signal's peak
//! times its largest gain: every signal in phase, the worst case.
//!
//! A net whose bias only the simulator's 10¹² Ω leak to ground sets (a capacitor-coupled input that biases
//! itself, a net past a part left open) has no bias: its findings are UNKNOWN.
//!
//! What is modelled: resistors, capacitors and inductors at their marked values; ferrite beads, fuses and net
//! ties as shorts; and op-amps from their part file's `[[opamp]]` as ideal amplifiers (an open-loop gain of 10⁶
//! with no bandwidth limit and no clipping, so the levels show where a real one would clip). Everything else is
//! left open: diodes, and IC pins with no op-amp model. A part left open on a net a signal reaches is an UNKNOWN
//! `signal-path` finding, as nothing past it is checked, and its own pins' levels can't be trusted.
//!
//! Rules:
//! - `clipping` (warning): an op-amp's output must stay inside its supply rails less the datasheet's output
//!   swing. Past the rails it surely clips; inside them, with no swing figure, the result is UNKNOWN.
//! - `signal-level`: every voltage limit on a pin a signal reaches. Past the absolute maximum is an error, past
//!   the recommended range a warning; a limit the part file doesn't give is UNKNOWN.
//! - `signal-path` (warning, UNKNOWN): a part the simulation leaves open on a net a signal reaches.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::design::{Component, Design};
use crate::inputs::{BoardInputs, Signal};
use crate::limits::{Bound, Class, Library, Part, Quantity, Range};
use crate::rules::{Finding, Severity, Verdict};
use crate::spice::{parse_farads, parse_henries, simulate};
use crate::voltage::{Driver, Span, Voltages, is_connector, parse_ohms};

/// Smaller gains than this count as no signal at all.
const NO_SIGNAL: f64 = 1e-4;

/// How far a net's bias may move when the leak's return moves 1 V before it counts as set by the leak alone.
const LEAK_SET: f64 = 1e-3;

/// A rail held at a DC voltage anywhere in `lo..=hi`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rail {
    pub net: usize,
    pub lo: f64,
    pub hi: f64,
}

/// A net's bias as the rails move: `b0` with every rail at its lowest, plus `delta[i]` as rail `i` alone goes to
/// its highest. Linear, so any mix of rail ends is the sum.
#[derive(Debug, Clone, PartialEq)]
pub struct Bias {
    pub b0: f64,
    pub delta: Vec<f64>,
}

/// A part the simulation leaves open.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub reference: String,
    pub why: String,
    pub nets: Vec<usize>,
}

/// Everything `gerberus signal` worked out.
#[derive(Debug, Clone)]
pub struct Levels {
    pub rails: Vec<Rail>,
    /// Nets on the simulation's ground: rails held at exactly 0 V.
    pub ground: BTreeSet<usize>,
    /// Each net's bias; missing when only the leak sets it.
    pub bias: BTreeMap<usize, Bias>,
    /// Each net's peak from ground, summed over the signals.
    pub peak: BTreeMap<usize, f64>,
    /// The peak of the difference between two nets, summed over the signals, for the pairs limits compare.
    pub pair_peak: BTreeMap<(usize, usize), f64>,
    /// Per signal, the largest gain to each net it reaches.
    pub gains: Vec<(String, BTreeMap<usize, f64>)>,
    pub skipped: Vec<Skipped>,
}

impl Levels {
    fn bias_of(&self, net: usize) -> Option<Bias> {
        if self.ground.contains(&net) {
            return Some(Bias {
                b0: 0.0,
                delta: vec![0.0; self.rails.len()],
            });
        }
        self.bias.get(&net).cloned()
    }

    /// The lowest and highest DC voltage of `a` from `b` with the rails anywhere in their ranges.
    pub fn dc(&self, a: usize, b: usize) -> Option<(f64, f64)> {
        let (x, y) = (self.bias_of(a)?, self.bias_of(b)?);
        let base = x.b0 - y.b0;
        let (mut lo, mut hi) = (base, base);
        for (dx, dy) in x.delta.iter().zip(&y.delta) {
            let d = dx - dy;
            lo += d.min(0.0);
            hi += d.max(0.0);
        }
        Some((lo, hi))
    }

    /// The signals' peak on `a` measured from `b`.
    pub fn swing(&self, a: usize, b: usize) -> f64 {
        if a == b {
            return 0.0;
        }
        let key = (a.min(b), a.max(b));
        self.pair_peak.get(&key).copied().unwrap_or_else(|| {
            // Not measured as a pair: the two peaks added, as if in opposite phase.
            self.peak.get(&a).copied().unwrap_or(0.0) + self.peak.get(&b).copied().unwrap_or(0.0)
        })
    }

    /// The lowest and highest `a` reaches from `b`: its DC range and the signals' peak.
    pub fn reach(&self, a: usize, b: usize) -> Option<(f64, f64)> {
        let (lo, hi) = self.dc(a, b)?;
        let p = self.swing(a, b);
        Some((lo - p, hi + p))
    }

    fn carries(&self, net: usize) -> bool {
        self.peak.get(&net).is_some_and(|&p| p > 0.0)
    }
}

/// One element of the small-signal circuit.
enum Element {
    R(String, usize, usize, f64),
    C(String, usize, usize, f64),
    L(String, usize, usize, f64),
    OpAmp {
        name: String,
        plus: usize,
        minus: usize,
        out: usize,
    },
}

/// Simulates the signals in `inputs` on the design. Fails when there are no signals, or ngspice can't run.
pub fn levels(design: &Design, lib: &Library, inputs: &BoardInputs, v: &Voltages) -> Result<Levels, String> {
    if inputs.signals.is_empty() {
        return Err(
            "inputs.toml has no [[signal]]: add one for each signal input (see \"Signals\" in parts/README.md)".into(),
        );
    }
    let net_of = |r: &str, p: &str| design.components.get(r)?.pins.get(p).map(|p| p.net);
    let mut sources = vec![];
    for s in &inputs.signals {
        let (Some(a), Some(b)) = (net_of(&s.pin.reference, &s.pin.pin), net_of(&s.to.reference, &s.to.pin)) else {
            return Err(format!("signal {}: its pins aren't in the design", s.name));
        };
        sources.push((s, a, b));
    }
    let signal_nets: BTreeSet<usize> = sources.iter().map(|&(_, a, _)| a).collect();
    let mut rails = vec![];
    let mut ground = BTreeSet::new();
    for n in 0..design.nets.len() {
        let nv = &v.nets[n];
        let (Some(lo), Some(hi)) = (nv.span.min, nv.span.max) else {
            continue;
        };
        if nv.driver != Driver::Strong || nv.why.is_empty() || signal_nets.contains(&n) {
            continue;
        }
        if nv.span == Span::exact(0.0) {
            ground.insert(n);
        } else {
            rails.push(Rail { net: n, lo, hi });
        }
    }
    let (elements, skipped) = elements(design, lib);
    let pairs = pairs(design, lib);
    let nets: Vec<usize> = (0..design.nets.len()).filter(|n| !ground.contains(n)).collect();

    let mut bias = BTreeMap::new();
    let mut peak: BTreeMap<usize, f64> = BTreeMap::new();
    let mut pair_peak: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    let mut gains: Vec<(String, BTreeMap<usize, f64>)> = vec![];
    for (k, &(s, _, _)) in sources.iter().enumerate() {
        let text = netlist(&ground, &rails, &nets, &elements, &sources, &pairs, k);
        let m = simulate(&text)?;
        if k == 0 {
            for &n in &nets {
                let Some(b0) = m.get(&format!("v(n{n})")) else { continue };
                let leaked = m.get(&format!("lk_n{n}")).is_none_or(|x| (x - b0).abs() > LEAK_SET);
                if leaked {
                    continue;
                }
                let delta = (0..rails.len())
                    .map(|i| m.get(&format!("r{i}_n{n}")).map_or(0.0, |x| x - b0))
                    .collect();
                bias.insert(n, Bias { b0, delta });
            }
        }
        let mut g = BTreeMap::new();
        for &n in &nets {
            if let Some(x) = m.get(&format!("g{n}"))
                && x > NO_SIGNAL
            {
                *peak.entry(n).or_default() += x * s.peak;
                g.insert(n, x);
            }
        }
        for (j, &key) in pairs.iter().enumerate() {
            if let Some(x) = m.get(&format!("q{j}")) {
                *pair_peak.entry(key).or_default() += x * s.peak;
            }
        }
        gains.push((s.name.clone(), g));
    }
    Ok(Levels {
        rails,
        ground,
        bias,
        peak,
        pair_peak,
        gains,
        skipped,
    })
}

/// The parts the simulation models, and what it leaves open.
fn elements(design: &Design, lib: &Library) -> (Vec<Element>, Vec<Skipped>) {
    let mut out = vec![];
    let mut skipped = vec![];
    for (r, c) in &design.components {
        if c.dnp || is_connector(lib, c) {
            continue;
        }
        let part = lib.part_for(c);
        let class = part.map(|p| p.class);
        let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
        let name = sanitize(r);
        let mut skip = |why: String| {
            skipped.push(Skipped {
                reference: r.clone(),
                why,
                nets: nets.iter().copied().collect::<BTreeSet<_>>().into_iter().collect(),
            });
        };
        if let Some(p) = part
            && !p.opamps.is_empty()
        {
            for (i, a) in p.opamps.iter().enumerate() {
                let pin = |n: &str| pin_net(c, p, n);
                match (pin(&a.plus), pin(&a.minus), pin(&a.out)) {
                    (Some(plus), Some(minus), Some(o)) => out.push(Element::OpAmp {
                        name: format!("{name}_{i}"),
                        plus,
                        minus,
                        out: o,
                    }),
                    _ => skip("an op-amp's pins aren't all connected".into()),
                }
            }
            continue;
        }
        let [a, b] = nets[..] else {
            skip("no model, so its pins are taken to draw no current".into());
            continue;
        };
        let lib_id = c.lib_id.as_str();
        let letters: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
        let short = lib_id.starts_with("Device:Ferrite")
            || lib_id.starts_with("Device:L_Ferrite")
            || lib_id.starts_with("Device:Fuse")
            || lib_id.starts_with("Device:Polyfuse")
            || lib_id.starts_with("Device:NetTie")
            || letters == "FB";
        let symbol_is = |base: &str| lib_id == base || lib_id.starts_with(&format!("{base}_"));
        if short {
            out.push(Element::R(name, a, b, 1e-3));
        } else if class == Some(Class::Resistor) || (class.is_none() && symbol_is("Device:R")) {
            match parse_ohms(&c.value) {
                Some(ohms) if ohms > 0.0 => out.push(Element::R(name, a, b, ohms)),
                _ => skip(format!("can't read a resistance from {:?}", c.value)),
            }
        } else if class == Some(Class::Capacitor)
            || (class.is_none() && (symbol_is("Device:C") || symbol_is("Device:CP") || symbol_is("Device:C_Polarized")))
        {
            match parse_farads(&c.value) {
                Some(f) if f > 0.0 => out.push(Element::C(name, a, b, f)),
                _ => skip(format!("can't read a capacitance from {:?}", c.value)),
            }
        } else if class == Some(Class::Inductor) || (class.is_none() && symbol_is("Device:L")) {
            match parse_henries(&c.value) {
                Some(h) if h > 0.0 => out.push(Element::L(name, a, b, h)),
                _ => skip(format!("can't read an inductance from {:?}", c.value)),
            }
        } else {
            skip("no model for a two-pin part like it".into());
        }
    }
    (out, skipped)
}

/// The pairs of nets whose difference a limit compares: every voltage limit's pin and reference, a pin and the
/// pin a bound follows, and each op-amp's output and supply pins. Keyed with the smaller net first.
fn pairs(design: &Design, lib: &Library) -> Vec<(usize, usize)> {
    let mut out = BTreeSet::new();
    for c in design.components.values() {
        if c.dnp {
            continue;
        }
        let Some(part) = lib.part_for(c) else { continue };
        let net = |n: &str| pin_net(c, part, n);
        let mut add = |a: Option<usize>, b: Option<usize>| {
            if let (Some(a), Some(b)) = (a, b)
                && a != b
            {
                out.insert((a.min(b), a.max(b)));
            }
        };
        for l in part.limits.iter().filter(|l| l.quantity == Quantity::Voltage) {
            let Some(to) = l.to.as_deref() else { continue };
            for p in &l.pins {
                add(net(p), net(to));
                for r in [&l.abs_max, &l.recommended] {
                    if let Range::Known { min, max, .. } = r {
                        for b in [min, max] {
                            if let Bound::Pin { pin, .. } = b {
                                add(net(p), net(pin));
                            }
                        }
                    }
                }
            }
        }
        for a in &part.opamps {
            add(net(&a.out), net(&a.vpos));
            add(net(&a.out), net(&a.vneg));
        }
    }
    out.into_iter().collect()
}

fn pin_net(c: &Component, part: &Part, name: &str) -> Option<usize> {
    part.pins_named(name)
        .into_iter()
        .find_map(|num| c.pins.get(num).map(|p| p.net))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The netlist for signal `k`: its source carries the AC, and the first one also sweeps the rails and the leak.
fn netlist(
    ground: &BTreeSet<usize>,
    rails: &[Rail],
    nets: &[usize],
    elements: &[Element],
    sources: &[(&Signal, usize, usize)],
    pairs: &[(usize, usize)],
    k: usize,
) -> String {
    let nd = |n: usize| {
        if ground.contains(&n) {
            "0".to_string()
        } else {
            format!("n{n}")
        }
    };
    let mut t = format!("* gerberus signal: {}\n", sources[k].0.name);
    for (i, r) in rails.iter().enumerate() {
        let _ = writeln!(t, "VR{i} {} 0 DC {}", nd(r.net), r.lo);
    }
    for (i, &(s, a, b)) in sources.iter().enumerate() {
        let ac = if i == k { " AC 1" } else { "" };
        if s.ac_coupled {
            // Through a large capacitor, so the board sets the pin's bias.
            let _ = writeln!(t, "VS{i} s{i} {} DC 0{ac}", nd(b));
            let _ = writeln!(t, "CS{i} s{i} {} 1", nd(a));
        } else {
            let _ = writeln!(t, "VS{i} {} {} DC 0{ac}", nd(a), nd(b));
        }
    }
    for e in elements {
        match e {
            Element::R(n, a, b, x) => {
                let _ = writeln!(t, "R{n} {} {} {x}", nd(*a), nd(*b));
            }
            Element::C(n, a, b, x) => {
                let _ = writeln!(t, "C{n} {} {} {x}", nd(*a), nd(*b));
            }
            Element::L(n, a, b, x) => {
                let _ = writeln!(t, "L{n} {} {} {x}", nd(*a), nd(*b));
            }
            Element::OpAmp { name, plus, minus, out } => {
                let _ = writeln!(t, "E{name} {} 0 {} {} 1e6", nd(*out), nd(*plus), nd(*minus));
            }
        }
    }
    // Every node gets a very high resistance to the leak's return, so one only capacitors reach still has a DC
    // value; moving the return shows which nets only the leak sets.
    t.push_str("VLEAK nleak 0 DC 0\n");
    for &n in nets {
        let _ = writeln!(t, "RLEAK{n} n{n} nleak 1e12");
    }
    t.push_str(".control\n");
    if k == 0 {
        t.push_str("op\n");
        for &n in nets {
            let _ = writeln!(t, "print v(n{n})");
        }
        for (i, r) in rails.iter().enumerate() {
            if r.hi == r.lo {
                continue;
            }
            let _ = writeln!(t, "alter vr{i} dc = {}\nop", r.hi);
            for &n in nets {
                let _ = writeln!(t, "let r{i}_n{n} = v(n{n})\nprint r{i}_n{n}");
            }
            let _ = writeln!(t, "alter vr{i} dc = {}", r.lo);
        }
        t.push_str("alter vleak dc = 1\nop\n");
        for &n in nets {
            let _ = writeln!(t, "let lk_n{n} = v(n{n})\nprint lk_n{n}");
        }
        t.push_str("alter vleak dc = 0\n");
    }
    let [lo, hi] = sources[k].0.frequency;
    let _ = writeln!(t, "ac dec 20 {lo} {hi}");
    for &n in nets {
        let _ = writeln!(t, "meas ac g{n} max vm(n{n})");
    }
    for (j, &(a, b)) in pairs.iter().enumerate() {
        let v = |n: usize| {
            if ground.contains(&n) {
                None
            } else {
                Some(format!("v(n{n})"))
            }
        };
        let expr = match (v(a), v(b)) {
            (Some(x), Some(y)) => format!("{x} - {y}"),
            (Some(x), None) | (None, Some(x)) => x,
            (None, None) => continue,
        };
        let _ = writeln!(t, "let p{j} = mag({expr})\nmeas ac q{j} max p{j}");
    }
    t.push_str(".endc\n.end\n");
    t
}

/// The `clipping`, `signal-level` and `signal-path` findings.
pub fn check(design: &Design, lib: &Library, levels: &Levels) -> Vec<Finding> {
    let mut out = vec![];
    let name = |n: usize| design.nets[n].name.as_str();
    let open: BTreeMap<&str, &Skipped> = levels.skipped.iter().map(|s| (s.reference.as_str(), s)).collect();
    for s in &levels.skipped {
        let reached: Vec<&str> = s
            .nets
            .iter()
            .filter(|&&n| levels.carries(n))
            .map(|&n| name(n))
            .collect();
        if reached.is_empty() {
            continue;
        }
        out.push(Finding {
            rule: "signal-path",
            verdict: Verdict::Unknown,
            severity: Severity::Warning,
            reference: s.reference.clone(),
            message: format!(
                "{} isn't simulated: it is on {}, which a signal reaches, and the simulation leaves it open ({}), \
                 so whatever the signal reaches through it isn't checked, and the levels on its own pins leave out \
                 its load",
                s.reference,
                reached.join(", "),
                s.why
            ),
            nets: vec![],
            case: None,
            headline: Some(format!("the signal isn't followed through {}", s.reference)),
        });
    }
    for (r, c) in &design.components {
        if c.dnp {
            continue;
        }
        let Some(part) = lib.part_for(c) else { continue };
        let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
        let cite = |page: Option<u32>| {
            format!(
                "{} {doc}{}",
                part.mpn,
                page.map(|p| format!(" p{p}")).unwrap_or_default()
            )
        };
        for a in &part.opamps {
            let (Some(o), Some(vp), Some(vn)) = (
                pin_net(c, part, &a.out),
                pin_net(c, part, &a.vpos),
                pin_net(c, part, &a.vneg),
            ) else {
                continue;
            };
            if !levels.carries(o) {
                continue;
            }
            let (verdict, message) = match (levels.reach(o, vp), levels.reach(o, vn)) {
                (Some((_, top)), Some((bot, _))) => {
                    let what = format!(
                        "with the rails anywhere in their ranges and the loudest signals, {} reaches {} V from {} and \
                         {} V from {}",
                        a.out,
                        round(top),
                        a.vpos,
                        round(bot),
                        a.vneg
                    );
                    if top > 0.0 || bot < 0.0 {
                        (Verdict::Fail, format!("{what}: past a rail, so it clips"))
                    } else if let Some(s) = &a.swing {
                        let room = (-top).min(bot);
                        let at = format!(
                            "its {} V output swing ({}{})",
                            s.value,
                            cite(Some(s.page)),
                            s.note.as_deref().map(|n| format!(", {n}")).unwrap_or_default()
                        );
                        if room < s.value {
                            (
                                Verdict::Fail,
                                format!("{what}: within {} V of a rail, closer than {at}", round(room)),
                            )
                        } else {
                            (
                                Verdict::Pass,
                                format!(
                                    "{what}: at least {} V from each rail, clear of {at}; the swing holds at the \
                                     note's supply and load",
                                    round(room)
                                ),
                            )
                        }
                    } else {
                        (
                            Verdict::Unknown,
                            format!(
                                "{what}: inside the rails, but the part file gives no output swing to compare with"
                            ),
                        )
                    }
                }
                _ => (
                    Verdict::Unknown,
                    format!(
                        "the bias of {} or its supply pins isn't set by anything but the simulation's leak, so how \
                         close it gets to a rail isn't known",
                        a.out
                    ),
                ),
            };
            out.push(Finding {
                rule: "clipping",
                verdict,
                severity: Severity::Warning,
                reference: r.clone(),
                message: format!("{r}.{} ({}): {message}", a.out, name(o)),
                nets: vec![o],
                case: None,
                headline: Some(match verdict {
                    Verdict::Fail => format!("{r}'s output {} clips on the loudest signal", name(o)),
                    Verdict::Unknown => format!("{r}'s output {} may clip on the loudest signal", name(o)),
                    Verdict::Pass => format!("{r}'s output {} stays clear of its rails", name(o)),
                }),
            });
        }
        // Passives are left to `check`: their ratings are far above any signal on a sound design.
        if matches!(
            part.class,
            Class::Resistor | Class::Capacitor | Class::Inductor | Class::Connector
        ) {
            continue;
        }
        for limit in part.limits.iter().filter(|l| l.quantity == Quantity::Voltage) {
            let Some(to) = limit.to.as_deref() else { continue };
            let Some(tn) = pin_net(c, part, to) else { continue };
            for pin in &limit.pins {
                let Some(pn) = pin_net(c, part, pin) else { continue };
                if !levels.carries(pn) {
                    continue;
                }
                let (verdict, severity, message) = level(levels, c, part, pin, pn, to, tn, limit, &cite);
                let (verdict, message) = match open.get(r.as_str()) {
                    Some(_) if verdict == Verdict::Fail => (
                        Verdict::Unknown,
                        format!("{message}; but the simulation leaves {r} open, so this level leaves out its load"),
                    ),
                    _ => (verdict, message),
                };
                out.push(Finding {
                    rule: "signal-level",
                    verdict,
                    severity,
                    reference: r.clone(),
                    message: format!("{r}.{pin} ({}): {message}", name(pn)),
                    nets: vec![pn],
                    case: None,
                    headline: None,
                });
            }
        }
    }
    out
}

/// One pin against one limit: the verdict, its severity and what was compared.
#[allow(clippy::too_many_arguments)]
fn level(
    levels: &Levels,
    c: &Component,
    part: &Part,
    pin: &str,
    pn: usize,
    to: &str,
    tn: usize,
    limit: &crate::limits::Limit,
    cite: &dyn Fn(Option<u32>) -> String,
) -> (Verdict, Severity, String) {
    let Some((lo, hi)) = levels.reach(pn, tn) else {
        return (
            Verdict::Unknown,
            Severity::Warning,
            format!(
                "the bias of {pin} or {to} isn't set by anything but the simulation's leak, so the level isn't known"
            ),
        );
    };
    let head = format!("{pin} reaches {} to {} V from {to}", round(lo), round(hi));
    let mut fails: Vec<(Severity, String)> = vec![];
    let mut unknown: Vec<String> = vec![];
    for (range, sev, kind) in [
        (&limit.abs_max, Severity::Error, "absolute maximum"),
        (&limit.recommended, Severity::Warning, "recommended range"),
    ] {
        let Range::Known { min, max, page, .. } = range else {
            unknown.push(format!("the part file gives no {kind}"));
            continue;
        };
        let at = cite(*page);
        for (bound, upper) in [(min, false), (max, true)] {
            match bound {
                Bound::Value(x) => {
                    if (upper && hi > x + 1e-9) || (!upper && lo < x - 1e-9) {
                        fails.push((
                            sev,
                            format!(
                                "past its {kind} {} of {x} V ({at})",
                                if upper { "maximum" } else { "minimum" }
                            ),
                        ));
                    }
                }
                Bound::Pin { pin: q, plus, at_most } => {
                    let Some(qn) = pin_net(c, part, q) else {
                        unknown.push(format!("{q}, which its {kind} follows, isn't connected"));
                        continue;
                    };
                    let Some((dlo, dhi)) = levels.reach(pn, qn) else {
                        unknown.push(format!("{q}'s bias isn't known"));
                        continue;
                    };
                    if (upper && dhi > plus + 1e-9) || (!upper && dlo < plus - 1e-9) {
                        fails.push((
                            sev,
                            format!(
                                "{} {q} by {} V, past its {kind} of {q} {plus:+} V ({at})",
                                if upper { "above" } else { "below" },
                                round(if upper { dhi } else { dlo })
                            ),
                        ));
                    }
                    if let Some(cap) = at_most
                        && upper
                        && hi > cap + 1e-9
                    {
                        fails.push((sev, format!("past its {kind} cap of {cap} V ({at})")));
                    }
                }
                Bound::Unknown(why) => unknown.push(format!("its {kind} doesn't give that end: {why}")),
            }
        }
    }
    if !fails.is_empty() {
        let severity = if fails.iter().any(|f| f.0 == Severity::Error) {
            Severity::Error
        } else {
            Severity::Warning
        };
        let what: Vec<String> = fails.into_iter().map(|f| f.1).collect();
        return (Verdict::Fail, severity, format!("{head}: {}", what.join("; ")));
    }
    if !unknown.is_empty() {
        return (
            Verdict::Unknown,
            Severity::Error,
            format!("{head}, inside what is known, but {}", unknown.join("; ")),
        );
    }
    (Verdict::Pass, Severity::Error, format!("{head}, inside its limits"))
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The levels as text: each signal's gain to every net it reaches, and each net's range from ground.
pub fn describe(design: &Design, levels: &Levels) -> String {
    let mut out = String::new();
    let ground = levels.ground.iter().next().copied();
    for (name, gains) in &levels.gains {
        let _ = writeln!(out, "signal {name}:");
        let mut rows: Vec<(usize, f64)> = gains.iter().map(|(&n, &g)| (n, g)).collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        for &(n, g) in rows.iter().take(40) {
            let range = ground.and_then(|gnd| levels.reach(n, gnd)).map_or(
                "bias not set (only the simulation's leak reaches it)".into(),
                |(lo, hi)| format!("{} to {} V", round(lo), round(hi)),
            );
            let _ = writeln!(
                out,
                "  {}: gain {} ({:+.1} dB), {range}",
                design.nets[n].name,
                round(g),
                20.0 * g.log10()
            );
        }
        if rows.len() > 40 {
            let _ = writeln!(out, "  … and {} more nets", rows.len() - 40);
        }
    }
    if !levels.skipped.is_empty() {
        let _ = writeln!(out, "left open in the simulation:");
        for s in &levels.skipped {
            let _ = writeln!(out, "  {}: {}", s.reference, s.why);
        }
    }
    out
}
