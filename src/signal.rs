//! Signal levels: how far the signals in `inputs.toml` swing each net, worked out in ngspice.
//!
//! Each `[[signal]]` is an AC source between its two pins. The board's rails are held at their worst-case
//! ends from the voltage engine: once all at their lowest, once all at their highest. In each case ngspice
//! finds every net's DC bias (`.op`) and its gain from each signal across the signal's band (`.ac`). A net
//! then swings from its bias down and up by the sum of each signal's peak times its largest gain: all signals
//! in phase, which is the worst case.
//!
//! What is modelled: resistors, capacitors, inductors and ferrite beads at their marked values, fuses as
//! shorts, and op-amps from their part file's `[[opamp]]` as ideal amplifiers (an open-loop gain of 10⁶ with
//! no bandwidth limit and no clipping, so the levels show where a real one would clip). Everything else is
//! left open: diodes (a signal doesn't reach their forward drop or breakdown on a sound design), and IC pins
//! with no op-amp model, which are taken to draw no current. Those parts are listed.
//!
//! Two rules read the levels:
//! - `clipping` (warning): an op-amp's output must stay inside its supply rails less the datasheet's output
//!   swing. Past the rails it surely clips; inside them, with no swing figure, the result is UNKNOWN.
//! - `signal-level`: every voltage limit on a pin a signal reaches, against the pin's bias plus and minus its
//!   swing: past the absolute maximum is an error, past the recommended range a warning.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::design::{Component, Design};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, Library, Part, Quantity, Range};
use crate::rules::{Finding, Severity, Verdict};
use crate::spice::{parse_farads, parse_henries, simulate};
use crate::voltage::{Driver, Voltages, is_connector, parse_ohms};

/// Smaller gains than this count as no signal at all.
const NO_SIGNAL: f64 = 1e-4;

/// Which end of every rail a run holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    Low,
    High,
}

impl Corner {
    fn name(self) -> &'static str {
        match self {
            Corner::Low => "every rail at its lowest",
            Corner::High => "every rail at its highest",
        }
    }
}

/// One net's level in one corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    /// The DC bias.
    pub bias: f64,
    /// How far the signals take it either way from the bias.
    pub peak: f64,
}

impl Level {
    pub fn low(&self) -> f64 {
        self.bias - self.peak
    }

    pub fn high(&self) -> f64 {
        self.bias + self.peak
    }
}

/// Everything `gerberus signal` worked out.
#[derive(Debug, Clone)]
pub struct Levels {
    /// Per corner, each net's level. A net ngspice gave no value for is missing.
    pub corners: Vec<(Corner, BTreeMap<usize, Level>)>,
    /// Per signal, the nets it reaches and its largest gain to each, in the corner it is largest.
    pub gains: Vec<(String, BTreeMap<usize, f64>)>,
    /// Parts left out of the simulation, and why.
    pub skipped: Vec<String>,
    /// Rails held at their ends, by net.
    pub rails: Vec<usize>,
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
    // Rails: every net the voltage engine knows both ends of and that something holds (an input, a regulator, a
    // diode), apart from the signal inputs themselves.
    let rails: Vec<usize> = (0..design.nets.len())
        .filter(|&n| {
            let nv = &v.nets[n];
            nv.span.is_known() && nv.driver == Driver::Strong && !nv.why.is_empty() && !signal_nets.contains(&n)
        })
        .collect();
    let (elements, skipped) = elements(design, lib);

    let mut corners = vec![];
    let mut gains: Vec<(String, BTreeMap<usize, f64>)> = inputs
        .signals
        .iter()
        .map(|s| (s.name.clone(), BTreeMap::new()))
        .collect();
    for corner in [Corner::Low, Corner::High] {
        let mut bias: BTreeMap<usize, f64> = BTreeMap::new();
        let mut peak: BTreeMap<usize, f64> = BTreeMap::new();
        for (k, &(s, _, _)) in sources.iter().enumerate() {
            let text = netlist(design, v, &rails, &elements, &sources, k, corner);
            let m = simulate(&text)?;
            for n in 0..design.nets.len() {
                if k == 0
                    && let Some(b) = m.get(&format!("v({})", node(v, n)))
                {
                    bias.insert(n, b);
                }
                if let Some(g) = m.get(&format!("g{n}"))
                    && g > NO_SIGNAL
                {
                    *peak.entry(n).or_default() += g * s.peak;
                    let e = gains[k].1.entry(n).or_default();
                    *e = e.max(g);
                }
            }
            // Ground and rails sit at their DC value.
            for &r in &rails {
                bias.insert(r, rail_value(v, r, corner));
            }
        }
        let levels = bias
            .into_iter()
            .map(|(n, b)| {
                (
                    n,
                    Level {
                        bias: b,
                        peak: peak.get(&n).copied().unwrap_or(0.0),
                    },
                )
            })
            .collect();
        corners.push((corner, levels));
    }
    Ok(Levels {
        corners,
        gains,
        skipped,
        rails,
    })
}

fn rail_value(v: &Voltages, net: usize, corner: Corner) -> f64 {
    let s = v.nets[net].span;
    match corner {
        Corner::Low => s.min.unwrap_or(0.0),
        Corner::High => s.max.unwrap_or(0.0),
    }
}

/// The ngspice node of a net: `0` for a 0 V ground.
fn node(v: &Voltages, net: usize) -> String {
    if v.nets[net].span == crate::voltage::Span::exact(0.0) {
        "0".into()
    } else {
        format!("n{net}")
    }
}

/// The parts the simulation models, and what it leaves out.
fn elements(design: &Design, lib: &Library) -> (Vec<Element>, Vec<String>) {
    let mut out = vec![];
    let mut skipped = vec![];
    for (r, c) in &design.components {
        if c.dnp {
            continue;
        }
        let part = lib.part_for(c);
        let class = part.map(|p| p.class);
        let two = c.pins.values().map(|p| p.net).collect::<Vec<_>>();
        let name = sanitize(r);
        let letters: String = r.chars().take_while(char::is_ascii_alphabetic).collect();
        if let Some(p) = part
            && !p.opamps.is_empty()
        {
            for (i, a) in p.opamps.iter().enumerate() {
                let pin = |n: &str| pin_net(c, p, n);
                match (pin(&a.plus), pin(&a.minus), pin(&a.out)) {
                    (Some(plus), Some(minus), Some(out_)) => out.push(Element::OpAmp {
                        name: format!("{name}_{i}"),
                        plus,
                        minus,
                        out: out_,
                    }),
                    _ => skipped.push(format!("{r}: an op-amp's pins aren't all connected")),
                }
            }
            continue;
        }
        let [a, b] = two[..] else {
            if !is_connector(lib, c) {
                skipped.push(format!(
                    "{r} ({}): left open, its pins taken to draw no current",
                    c.value
                ));
            }
            continue;
        };
        if c.lib_id.starts_with("Device:R") || class == Some(Class::Resistor) {
            match parse_ohms(&c.value) {
                Some(ohms) if ohms > 0.0 => out.push(Element::R(name, a, b, ohms)),
                _ => skipped.push(format!(
                    "{r}: can't read a resistance from {:?}, so it is left open",
                    c.value
                )),
            }
        } else if c.lib_id.starts_with("Device:C") || class == Some(Class::Capacitor) {
            match parse_farads(&c.value) {
                Some(f) if f > 0.0 => out.push(Element::C(name, a, b, f)),
                _ => skipped.push(format!(
                    "{r}: can't read a capacitance from {:?}, so it is left open",
                    c.value
                )),
            }
        } else if c.lib_id.starts_with("Device:Ferrite") || letters == "FB" || letters == "F" {
            out.push(Element::R(name, a, b, 1e-3));
        } else if c.lib_id.starts_with("Device:L") || class == Some(Class::Inductor) {
            match parse_henries(&c.value) {
                Some(h) if h > 0.0 => out.push(Element::L(name, a, b, h)),
                _ => skipped.push(format!(
                    "{r}: can't read an inductance from {:?}, so it is left open",
                    c.value
                )),
            }
        } else if !is_connector(lib, c) {
            skipped.push(format!("{r} ({}): left open", c.value));
        }
    }
    (out, skipped)
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

/// The netlist for one corner, with signal `k` as the AC source and every signal's DC at 0 V.
fn netlist(
    design: &Design,
    v: &Voltages,
    rails: &[usize],
    elements: &[Element],
    sources: &[(&crate::inputs::Signal, usize, usize)],
    k: usize,
    corner: Corner,
) -> String {
    let nd = |n: usize| node(v, n);
    let mut t = format!("* gerberus signal: {}, {}\n", sources[k].0.name, corner.name());
    for &r in rails {
        if nd(r) != "0" {
            let _ = writeln!(t, "VR{r} {} 0 DC {}", nd(r), rail_value(v, r, corner));
        }
    }
    for (i, &(_, a, b)) in sources.iter().enumerate() {
        let ac = if i == k { " AC 1" } else { "" };
        let _ = writeln!(t, "VS{i} {} {} DC 0{ac}", nd(a), nd(b));
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
    // Every node gets a very high resistance to ground, so one only capacitors reach still has a DC value.
    let nodes: BTreeSet<String> = (0..design.nets.len()).map(nd).filter(|n| n != "0").collect();
    for n in &nodes {
        let _ = writeln!(t, "RLEAK_{n} {n} 0 1e12");
    }
    let [lo, hi] = sources[k].0.frequency;
    t.push_str(".control\nop\n");
    for n in &nodes {
        let _ = writeln!(t, "print v({n})");
    }
    let _ = writeln!(t, "ac dec 20 {lo} {hi}");
    for n in &nodes {
        let net = n.trim_start_matches('n');
        let _ = writeln!(t, "meas ac g{net} max vm({n})");
    }
    t.push_str(".endc\n.end\n");
    t
}

/// The `clipping` and `signal-level` findings.
pub fn check(design: &Design, lib: &Library, levels: &Levels) -> Vec<Finding> {
    let mut out = vec![];
    let name = |n: usize| design.nets[n].name.as_str();
    let carries = |n: usize| {
        levels
            .corners
            .iter()
            .any(|(_, l)| l.get(&n).is_some_and(|x| x.peak > 0.0))
    };
    for (r, c) in &design.components {
        if c.dnp {
            continue;
        }
        let Some(part) = lib.part_for(c) else { continue };
        let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
        // Op-amp outputs.
        for a in &part.opamps {
            let (Some(o), Some(vp), Some(vn)) = (
                pin_net(c, part, &a.out),
                pin_net(c, part, &a.vpos),
                pin_net(c, part, &a.vneg),
            ) else {
                continue;
            };
            if !carries(o) {
                continue;
            }
            let mut verdict = Verdict::Pass;
            let mut notes = vec![];
            for (corner, l) in &levels.corners {
                let (Some(x), Some(top), Some(bot)) = (l.get(&o), l.get(&vp), l.get(&vn)) else {
                    verdict = Verdict::Unknown;
                    notes.push(format!("{}: a level isn't known", corner.name()));
                    continue;
                };
                let (lo, hi) = (x.low(), x.high());
                let (top, bot) = (top.bias, bot.bias);
                let what = format!(
                    "with {}, {} swings {} to {} V about {} V, between rails of {} and {} V",
                    corner.name(),
                    a.out,
                    round(lo),
                    round(hi),
                    round(x.bias),
                    round(bot),
                    round(top)
                );
                let past_rail = hi > top || lo < bot;
                let v = match &a.swing {
                    _ if past_rail => {
                        notes.push(format!("{what}: past a rail, so it clips"));
                        Verdict::Fail
                    }
                    Some(s) => {
                        if hi > top - s.value || lo < bot + s.value {
                            notes.push(format!(
                                "{what}: within {} V of a rail, closer than its {} V output swing ({} {doc} p{}{})",
                                round((top - hi).min(lo - bot)),
                                s.value,
                                part.mpn,
                                s.page,
                                s.note.as_deref().map(|n| format!(", {n}")).unwrap_or_default()
                            ));
                            Verdict::Fail
                        } else {
                            notes.push(format!(
                                "{what}: at least {} V from each rail, clear of its {} V output swing ({} {doc} p{})",
                                round((top - hi).min(lo - bot)),
                                s.value,
                                part.mpn,
                                s.page
                            ));
                            Verdict::Pass
                        }
                    }
                    None => {
                        notes.push(format!(
                            "{what}: inside the rails, but the part file gives no output swing to compare with"
                        ));
                        Verdict::Unknown
                    }
                };
                verdict = worst(verdict, v);
            }
            out.push(Finding {
                rule: "clipping",
                verdict,
                severity: Severity::Warning,
                reference: r.clone(),
                message: format!("{r}.{} ({}): {}", a.out, name(o), notes.join("; ")),
                nets: vec![o],
                case: None,
                headline: Some(match verdict {
                    Verdict::Fail => format!("{r}'s output {} clips on the loudest signal", name(o)),
                    Verdict::Unknown => format!("{r}'s output {} may clip on the loudest signal", name(o)),
                    Verdict::Pass => format!("{r}'s output {} stays clear of its rails", name(o)),
                }),
            });
        }
        // Voltage limits on pins a signal reaches. Passives are left to `check`: their ratings are far above
        // any signal on a sound design.
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
                if !carries(pn) {
                    continue;
                }
                let mut verdict = Verdict::Pass;
                let mut severity = Severity::Warning;
                let mut notes = vec![];
                for (corner, l) in &levels.corners {
                    let (Some(x), Some(t)) = (l.get(&pn), l.get(&tn)) else {
                        verdict = worst(verdict, Verdict::Unknown);
                        continue;
                    };
                    // The pin against its reference, with the reference's own signal on the other side.
                    let lo = x.low() - t.high();
                    let hi = x.high() - t.low();
                    let resolve = |b: &Bound| {
                        b.resolve(|p| {
                            let n = pin_net(c, part, p)?;
                            Some(l.get(&n)?.bias - t.bias)
                        })
                    };
                    for (range, sev, kind) in [
                        (&limit.abs_max, Severity::Error, "absolute maximum"),
                        (&limit.recommended, Severity::Warning, "recommended range"),
                    ] {
                        let Range::Known { min, max, page, .. } = range else {
                            continue;
                        };
                        let (rmin, rmax) = (resolve(min), resolve(max));
                        let cite = format!(
                            "{} {doc}{}",
                            part.mpn,
                            page.map(|p| format!(" p{p}")).unwrap_or_default()
                        );
                        let past = rmin.is_some_and(|m| lo < m - 1e-9) || rmax.is_some_and(|m| hi > m + 1e-9);
                        if past {
                            notes.push(format!(
                                "with {}, {pin} swings {} to {} V from {to}, past its {kind} of {} to {} V ({cite})",
                                corner.name(),
                                round(lo),
                                round(hi),
                                show(rmin),
                                show(rmax)
                            ));
                            // An absolute maximum broken anywhere makes it an error.
                            if verdict != Verdict::Fail || sev == Severity::Error {
                                severity = sev;
                            }
                            verdict = Verdict::Fail;
                            break;
                        }
                        if kind == "recommended range" && (rmin.is_none() || rmax.is_none()) {
                            verdict = worst(verdict, Verdict::Unknown);
                        }
                    }
                }
                if notes.is_empty() {
                    let l = &levels.corners[0].1;
                    if let (Some(x), Some(t)) = (l.get(&pn), l.get(&tn)) {
                        notes.push(format!(
                            "with {}, {pin} swings {} to {} V from {to}, inside its limits",
                            levels.corners[0].0.name(),
                            round(x.low() - t.high()),
                            round(x.high() - t.low())
                        ));
                    }
                }
                out.push(Finding {
                    rule: "signal-level",
                    verdict,
                    severity,
                    reference: r.clone(),
                    message: format!("{r}.{pin} ({}): {}", name(pn), notes.join("; ")),
                    nets: vec![pn],
                    case: None,
                    headline: None,
                });
            }
        }
    }
    out
}

fn worst(a: Verdict, b: Verdict) -> Verdict {
    match (a, b) {
        (Verdict::Fail, _) | (_, Verdict::Fail) => Verdict::Fail,
        (Verdict::Unknown, _) | (_, Verdict::Unknown) => Verdict::Unknown,
        _ => Verdict::Pass,
    }
}

fn show(v: Option<f64>) -> String {
    v.map_or("UNKNOWN".into(), |v| format!("{}", round(v)))
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The levels as text: each signal's gain to every net it reaches, and each net's bias and swing.
pub fn describe(design: &Design, levels: &Levels) -> String {
    let mut out = String::new();
    for (name, gains) in &levels.gains {
        let _ = writeln!(out, "signal {name}:");
        let mut rows: Vec<(&usize, &f64)> = gains.iter().collect();
        rows.sort_by(|a, b| b.1.total_cmp(a.1));
        for &(&n, &g) in rows.iter().take(40) {
            let lv: Vec<String> = levels
                .corners
                .iter()
                .filter_map(|(_, l)| l.get(&n))
                .map(|l| format!("{} to {} V", round(l.low()), round(l.high())))
                .collect();
            let _ = writeln!(
                out,
                "  {}: gain {} ({:+.1} dB), {}",
                design.nets[n].name,
                round(g),
                20.0 * g.log10(),
                lv.join(" / ")
            );
        }
        if rows.len() > 40 {
            let _ = writeln!(out, "  … and {} more nets", rows.len() - 40);
        }
    }
    if !levels.skipped.is_empty() {
        let _ = writeln!(out, "not simulated (taken as open):");
        for s in &levels.skipped {
            let _ = writeln!(out, "  {s}");
        }
    }
    out
}
