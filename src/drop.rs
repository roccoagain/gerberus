//! Voltage drop: does every power pin still get its recommended minimum after the copper between it and the part
//! that supplies its net?
//!
//! The copper of a net is a resistor network: each track is split wherever another track or a via touches it,
//! and each via adds its barrel between the layers it joins. The supplying part's pads on the net are joined into
//! one node (`s`), held at the net's lowest voltage. Solving the network gives `Z`, the resistance from `s` to a
//! power pin's pad. In a network of resistors fed from one node, the drop a current into any pad makes at this
//! pad is at most `Z` times that current, and the pin's own current makes exactly `Z` times its current. So:
//!
//! - PASS: the net's whole current times `Z` (thinnest copper, hottest board) leaves the pin its recommended
//!   minimum.
//! - FAIL: the pin's own maximum current times `Z` (thickest copper, coldest board) is already too much, on a net
//!   with no copper pour (a pour adds paths the network doesn't count, which only lower `Z`).
//! - Otherwise UNKNOWN.
//!
//! Tracks and vias only: a pour's resistance isn't worked out, so a pin joined to the supply only through a pour
//! is UNKNOWN. The ground return isn't counted; it is taken to be a plane, whose drop is far smaller than a
//! track's.
use std::collections::{BTreeMap, BTreeSet};

use crate::design::{Design, PinKind};
use crate::finding::{Finding, Severity, Verdict};
use crate::inputs::BoardInputs;
use crate::kicad::pcb::{Layout, PlacedPad, Point, StackLayer, dist};
use crate::limits::{Bound, Library, Quantity, Range};
use crate::policy::TracePolicy;
use crate::thermal::Thermal;
use crate::voltage::Voltages;

/// Copper's resistivity at 20 °C, Ω·m, and its temperature coefficient, per °C.
const RHO_20: f64 = 1.72e-8;
const ALPHA: f64 = 0.00393;
/// Via barrel plating, mm: IPC-6012 Class 2's minimum average.
const PLATING: f64 = 0.020;

/// Copper's resistivity at `t` °C, Ω·mm.
fn rho(t: f64) -> f64 {
    RHO_20 * (1.0 + ALPHA * (t - 20.0)) * 1e3
}

/// The copper of one net as a resistor network.
pub struct Network {
    /// Edges between nodes, with their resistance in ohms.
    edges: Vec<(usize, usize, f64)>,
    nodes: usize,
    /// The node each pad (by its index in the `pads` given to [`Network::build`]) is on, if any copper reaches it.
    pub pads: Vec<Option<usize>>,
}

/// Where `p` falls along the segment `a`–`b` (0 at `a`, 1 at `b`), and how far it is from it.
fn along(p: Point, a: Point, b: Point) -> (f64, f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    (t, dist(p, (a.0 + t * dx, a.1 + t * dy)))
}

impl Network {
    /// The tracks and vias of `net`, with copper `thickness` (mm, by layer name) and resistivity `rho` (Ω·mm).
    pub fn build(layout: &Layout, net: &str, pads: &[&PlacedPad], thickness: &BTreeMap<String, f64>, rho: f64) -> Self {
        let tracks: Vec<_> = layout.tracks.iter().filter(|t| t.net == net).collect();
        let vias: Vec<_> = layout.vias.iter().filter(|v| v.net == net).collect();
        let layers: Vec<String> = layout
            .stackup
            .iter()
            .filter_map(|l| match l {
                StackLayer::Copper { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let mut nodes = 0;
        let mut node = || {
            nodes += 1;
            nodes - 1
        };
        // Each via: a hub, and a node on each layer it reaches, half its barrel from the hub.
        let mut via_at: Vec<BTreeMap<String, usize>> = vec![];
        let mut edges = vec![];
        let board = layout.thickness();
        for v in &vias {
            let hub = node();
            let (top, bottom) = (
                layers.iter().position(|l| *l == v.layers.0),
                layers.iter().position(|l| *l == v.layers.1),
            );
            let span = match (top, bottom) {
                (Some(a), Some(b)) => &layers[a.min(b)..=a.max(b)],
                _ => &layers[..],
            };
            // Barrel: the plating's ring around the drill, the board's thickness long.
            let ring = std::f64::consts::PI * (v.drill + PLATING) * PLATING;
            let barrel = rho * board / ring;
            let mut on = BTreeMap::new();
            for l in span {
                let n = node();
                edges.push((hub, n, barrel / 2.0));
                on.insert(l.clone(), n);
            }
            via_at.push(on);
        }
        // Points along each track where something touches it, as (position along it, node).
        let ends: Vec<(usize, usize)> = tracks.iter().map(|_| (node(), node())).collect();
        let end_node = |i: usize, end: bool| if end { ends[i].1 } else { ends[i].0 };
        let mut points: Vec<Vec<(f64, usize)>> = ends.iter().map(|&(a, b)| vec![(0.0, a), (1.0, b)]).collect();
        let mut joins: Vec<(usize, usize)> = vec![];
        for (i, t) in tracks.iter().enumerate() {
            let half = t.width / 2.0;
            // Another track's end touching this one.
            for (j, u) in tracks.iter().enumerate() {
                if j == i || u.layer != t.layer {
                    continue;
                }
                for (end, p) in [(false, u.start), (true, u.end)] {
                    let (s, d) = along(p, t.start, t.end);
                    if d <= half + u.width / 2.0 + 1e-6 {
                        let n = node();
                        points[i].push((s, n));
                        joins.push((n, end_node(j, end)));
                    }
                }
            }
            // A via on it.
            for (k, v) in vias.iter().enumerate() {
                let Some(&on) = via_at[k].get(&t.layer) else { continue };
                let (s, d) = along(v.at, t.start, t.end);
                if d <= v.diameter / 2.0 + half + 1e-6 {
                    let n = node();
                    points[i].push((s, n));
                    joins.push((n, on));
                }
            }
        }
        // Pads: a node each, joined to every track end and via inside them.
        let pad_nodes: Vec<usize> = pads.iter().map(|_| node()).collect();
        let mut reached: Vec<bool> = vec![false; pads.len()];
        for (p, pad) in pads.iter().enumerate() {
            for (i, t) in tracks.iter().enumerate() {
                let on = pad.layers.iter().any(|l| l == &t.layer || l == "*.Cu");
                let half = t.width / 2.0;
                for (end, at) in [(false, t.start), (true, t.end)] {
                    if on && pad.contains(at, half) {
                        joins.push((pad_nodes[p], end_node(i, end)));
                        reached[p] = true;
                    }
                }
            }
            for (k, v) in vias.iter().enumerate() {
                if pad.contains(v.at, v.diameter / 2.0) {
                    for (l, &n) in &via_at[k] {
                        if pad.layers.iter().any(|pl| pl == l || pl == "*.Cu") {
                            joins.push((pad_nodes[p], n));
                            reached[p] = true;
                        }
                    }
                }
            }
        }
        // Each track as a chain through the points along it.
        for (i, t) in tracks.iter().enumerate() {
            let Some(&th) = thickness.get(&t.layer) else { continue };
            let len = t.length();
            let pts = &mut points[i];
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            for w in pts.windows(2) {
                let piece = (w[1].0 - w[0].0) * len;
                if piece < 1e-6 {
                    // Two points at the same place on the track.
                    joins.push((w[0].1, w[1].1));
                } else {
                    edges.push((w[0].1, w[1].1, rho * piece / (t.width * th)));
                }
            }
        }
        // Touching copper is one node.
        let mut parent: Vec<usize> = (0..nodes).collect();
        fn root(p: &mut [usize], mut n: usize) -> usize {
            while p[n] != n {
                p[n] = p[p[n]];
                n = p[n];
            }
            n
        }
        for (a, b) in joins {
            let (x, y) = (root(&mut parent, a), root(&mut parent, b));
            parent[x] = y;
        }
        let mut index = BTreeMap::new();
        let mut next = 0;
        let mut id = |n: usize, parent: &mut [usize]| {
            let r = root(parent, n);
            *index.entry(r).or_insert_with(|| {
                next += 1;
                next - 1
            })
        };
        let edges: Vec<(usize, usize, f64)> = edges
            .into_iter()
            .map(|(a, b, r)| (id(a, &mut parent), id(b, &mut parent), r))
            .filter(|(a, b, _)| a != b)
            .collect();
        let pads = pad_nodes
            .iter()
            .zip(&reached)
            .map(|(&n, &r)| r.then(|| id(n, &mut parent)))
            .collect();
        Network {
            edges,
            nodes: next,
            pads,
        }
    }

    /// The resistance from the nodes in `from` (joined) to each node in `to`, ohms; `None` where no copper joins
    /// them.
    pub fn resistance(&self, from: &BTreeSet<usize>, to: &[usize]) -> Vec<Option<f64>> {
        // Nodes reachable from `from`: the rest can't carry current.
        let mut adj: Vec<Vec<usize>> = vec![vec![]; self.nodes];
        for &(a, b, _) in &self.edges {
            adj[a].push(b);
            adj[b].push(a);
        }
        let mut seen = vec![false; self.nodes];
        let mut stack: Vec<usize> = from.iter().copied().collect();
        for &n in &stack {
            seen[n] = true;
        }
        while let Some(n) = stack.pop() {
            for &m in &adj[n] {
                if !seen[m] {
                    seen[m] = true;
                    stack.push(m);
                }
            }
        }
        // The other reachable nodes, numbered for the conductance matrix; `from` is the reference.
        let free: Vec<usize> = (0..self.nodes).filter(|&n| seen[n] && !from.contains(&n)).collect();
        let at: BTreeMap<usize, usize> = free.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let k = free.len();
        let mut g = vec![vec![0.0; k]; k];
        for &(a, b, r) in &self.edges {
            let c = 1.0 / r;
            match (at.get(&a), at.get(&b)) {
                (Some(&i), Some(&j)) => {
                    g[i][i] += c;
                    g[j][j] += c;
                    g[i][j] -= c;
                    g[j][i] -= c;
                }
                (Some(&i), None) | (None, Some(&i)) => g[i][i] += c,
                (None, None) => {}
            }
        }
        let Some(l) = cholesky(&g) else {
            return to.iter().map(|_| None).collect();
        };
        to.iter()
            .map(|&n| {
                if from.contains(&n) {
                    return Some(0.0);
                }
                let &i = at.get(&n)?;
                // Z_ii: the voltage at i for 1 A into it.
                let mut e = vec![0.0; k];
                e[i] = 1.0;
                let v = solve(&l, &e);
                Some(v[i])
            })
            .collect()
    }
}

/// The lower-triangular `L` with `L Lᵀ = g`, for a symmetric positive-definite `g`.
fn cholesky(g: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let k = g.len();
    let mut l = vec![vec![0.0; k]; k];
    for i in 0..k {
        for j in 0..=i {
            let s: f64 = (0..j).map(|m| l[i][m] * l[j][m]).sum();
            if i == j {
                let d = g[i][i] - s;
                if d <= 0.0 {
                    return None;
                }
                l[i][i] = d.sqrt();
            } else {
                l[i][j] = (g[i][j] - s) / l[j][j];
            }
        }
    }
    Some(l)
}

/// Solves `L Lᵀ x = b`.
fn solve(l: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let k = b.len();
    let mut y = vec![0.0; k];
    for i in 0..k {
        y[i] = (b[i] - (0..i).map(|m| l[i][m] * y[m]).sum::<f64>()) / l[i][i];
    }
    let mut x = vec![0.0; k];
    for i in (0..k).rev() {
        x[i] = (y[i] - (i + 1..k).map(|m| l[m][i] * x[m]).sum::<f64>()) / l[i][i];
    }
    x
}

fn number(b: &Bound) -> Option<f64> {
    match b {
        Bound::Value(v) => Some(*v),
        _ => None,
    }
}

/// The parts that supply a net: the regulator that sets it, the connector an input arrives on, and each two-pin
/// part (other than a capacitor) or MOSFET channel joining it to a net its voltage comes from.
fn suppliers(design: &Design, inputs: &BoardInputs, volts: &Voltages, net: usize) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = volts
        .regulators
        .iter()
        .filter(|r| r.output == net && r.sets_output)
        .map(|r| r.reference.clone())
        .collect();
    for i in inputs.inputs.iter().filter(|i| i.fault.is_none()) {
        if design
            .components
            .get(&i.pin.reference)
            .and_then(|c| c.pins.get(&i.pin.pin))
            .is_some_and(|p| p.net == net)
        {
            out.insert(i.pin.reference.clone());
        }
    }
    let from: BTreeSet<usize> = volts.nets[net]
        .why
        .iter()
        .flat_map(|w| w.from.iter().copied())
        .filter(|&n| n != net)
        .collect();
    for (r, c) in &design.components {
        let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
        if !c.dnp
            && !crate::design::is_capacitor(r, c)
            && nets.len() == 2
            && nets.contains(&net)
            && nets.iter().any(|n| from.contains(n))
        {
            out.insert(r.clone());
        }
    }
    for ch in &volts.channels {
        let ends = [ch.drain, ch.source];
        if ends.contains(&net) && ends.iter().any(|n| from.contains(n)) {
            out.insert(ch.reference.clone());
        }
    }
    out
}

/// The `voltage-drop` findings: one per power pin with a recommended minimum, on a net with tracks.
pub fn check(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    volts: &Voltages,
    layout: &Layout,
    th: &Thermal,
    policy: &TracePolicy,
) -> Vec<Finding> {
    let mut out = vec![];
    let layers: Vec<(String, f64)> = layout
        .stackup
        .iter()
        .filter_map(|l| match l {
            StackLayer::Copper { name, thickness } => Some((name.clone(), *thickness)),
            _ => None,
        })
        .collect();
    // With no stackup in the board file the copper could be anything from ½ oz to 2 oz: a pass takes the
    // thinnest, a fail the thickest.
    let (thin, thick): (BTreeMap<String, f64>, BTreeMap<String, f64>) = if layout.stackup_assumed {
        (
            layers.iter().map(|(n, _)| (n.clone(), 0.0175)).collect(),
            layers.iter().map(|(n, _)| (n.clone(), 0.070)).collect(),
        )
    } else {
        (layers.iter().cloned().collect(), layers.iter().cloned().collect())
    };
    let copper_note = if layout.stackup_assumed {
        "; the board file has no stackup, so this takes ½ oz copper to pass and 2 oz to fail"
    } else {
        ""
    };
    for (n, net) in design.nets.iter().enumerate() {
        if !layout.tracks.iter().any(|t| t.net == net.name) {
            continue;
        }
        // Power pins on the net with a recommended minimum to their reference pin.
        let mut sinks = vec![];
        for (r, p) in &net.pins {
            let Some(c) = design.components.get(r) else { continue };
            if c.dnp || c.pins.get(p).is_none_or(|q| q.kind != PinKind::PowerIn) {
                continue;
            }
            let Some(part) = lib.part_for(c) else { continue };
            let name = part.pins.get(p).map_or(p.as_str(), String::as_str);
            let Some((limit, min)) = part
                .limits_for(Quantity::Voltage, name)
                .find_map(|l| match &l.recommended {
                    Range::Known { min, .. } => number(min).map(|m| (l, m)),
                    Range::Unknown(_) => None,
                })
            else {
                continue;
            };
            let Some(to) = limit
                .to
                .as_deref()
                .and_then(|t| part.pins_named(t).into_iter().find_map(|num| c.pins.get(num)))
                .map(|q| q.net)
            else {
                continue;
            };
            // Its own maximum current: a supply entry for this pin alone.
            let own = part
                .supply
                .iter()
                .find(|s| s.pins.len() == 1 && s.pins[0] == name)
                .and_then(crate::limits::SupplyCurrent::max);
            sinks.push((r.clone(), p.clone(), part, limit, min, to, own));
        }
        if sinks.is_empty() {
            continue;
        }
        let pads: Vec<(&str, &PlacedPad)> = layout
            .footprints
            .iter()
            .flat_map(|f| f.pads.iter().map(move |p| (f.reference.as_str(), p)))
            .filter(|(_, p)| p.net.as_deref() == Some(net.name.as_str()))
            .collect();
        let pad_refs: Vec<&PlacedPad> = pads.iter().map(|(_, p)| *p).collect();
        let pour = layout.zones.iter().any(|z| z.net == net.name) || layout.zone_nets.contains(&net.name);
        let supply = suppliers(design, inputs, volts, n);
        let load = &th.loads[n];
        let span = volts.nets[n].span;
        let finding = |r: &str, p: &str, verdict, message: String| Finding {
            rule: "voltage-drop",
            verdict,
            severity: Severity::Warning,
            reference: r.to_string(),
            message: format!("{r}.{p} ({}): {message}", net.name),
            nets: vec![n],
            case: None,
            headline: None,
        };
        if span.min.is_none() {
            out.extend(
                sinks
                    .iter()
                    .map(|k| finding(&k.0, &k.1, Verdict::Unknown, "the net's voltage isn't known".into())),
            );
            continue;
        }
        let [s] = supply.iter().collect::<Vec<_>>()[..] else {
            let why = if supply.is_empty() {
                "no regulator, input or series part is found supplying the net, so there is nothing to measure the \
                 copper from"
                    .to_string()
            } else {
                format!(
                    "the net is supplied from {}, so how much each supplies isn't known",
                    supply.iter().cloned().collect::<Vec<_>>().join(", ")
                )
            };
            out.extend(sinks.iter().map(|k| finding(&k.0, &k.1, Verdict::Unknown, why.clone())));
            continue;
        };
        // Resistance per ohm·mm of resistivity from the supplier's pads to each sink's pad, for the thinnest and
        // the thickest copper.
        let source: Vec<usize> = (0..pads.len()).filter(|&i| pads[i].0 == s).collect();
        let sink_pad: Vec<Option<usize>> = sinks
            .iter()
            .map(|k| pads.iter().position(|(r, pad)| *r == k.0 && pad.number == k.1))
            .collect();
        let unit = |thickness: &BTreeMap<String, f64>| -> Vec<Option<f64>> {
            let copper = Network::build(layout, &net.name, &pad_refs, thickness, 1.0);
            let from: BTreeSet<usize> = source.iter().filter_map(|&i| copper.pads[i]).collect();
            let to: Vec<Option<usize>> = sink_pad.iter().map(|i| i.and_then(|i| copper.pads[i])).collect();
            if from.is_empty() {
                return to.iter().map(|_| None).collect();
            }
            let known: Vec<usize> = to.iter().flatten().copied().collect();
            let mut z = copper.resistance(&from, &known).into_iter();
            to.iter().map(|t| t.and_then(|_| z.next().flatten())).collect()
        };
        let z_thin = unit(&thin);
        let z_thick = unit(&thick);
        for (k, (r, p, part, limit, min, to, own)) in sinks.into_iter().enumerate() {
            let mut push = |verdict, message| out.push(finding(&r, &p, verdict, message));
            let (Some(low), Some(gnd)) = (span.min, volts.nets[to].span.max) else {
                push(Verdict::Unknown, "the net's voltage isn't known".into());
                continue;
            };
            let margin = low - gnd - min;
            if margin <= 0.0 {
                push(
                    Verdict::Unknown,
                    format!(
                        "the net's lowest {low} V leaves nothing above the {min} V it needs, before any copper (the \
                         pin's own limit check reports that)"
                    ),
                );
                continue;
            }
            let (hot, cold, from) = match (&inputs.temperature, &part.temperature) {
                (Some(t), _) => (t.max, t.min, format!("the board's {} to {} °C", t.min, t.max)),
                (None, Some(t)) => (
                    t.max,
                    t.min,
                    format!(
                        "{}'s rated {} to {} °C (p{}), as inputs.toml gives no board temperature",
                        part.mpn, t.min, t.max, t.page
                    ),
                ),
                (None, None) => {
                    push(
                        Verdict::Unknown,
                        "neither inputs.toml nor its part file gives a temperature for the copper".into(),
                    );
                    continue;
                }
            };
            if sink_pad[k].is_none() {
                push(Verdict::Unknown, format!("{r}.{p} has no pad on the board"));
                continue;
            }
            let (Some(z_hot), Some(z_cold)) = (
                z_thin[k].map(|z| z * rho(hot + policy.rise)),
                z_thick[k].map(|z| z * rho(cold)),
            ) else {
                push(
                    Verdict::Unknown,
                    format!(
                        "no track or via joins it to {s}{}",
                        if pour {
                            ", only a copper pour, whose resistance gerberus doesn't work out"
                        } else {
                            ""
                        }
                    ),
                );
                continue;
            };
            let mohm = |z: f64| format!("{:.1} mΩ", z * 1e3);
            let mv = |v: f64| format!("{:.0} mV", v * 1e3);
            let page = match &limit.recommended {
                Range::Known { page: Some(p), .. } => format!(" p{p}"),
                _ => String::new(),
            };
            let low = (low * 1e3).round() / 1e3;
            let need = format!(
                "it needs {min} V to {} ({}{page}), {} below the net's lowest {low} V",
                limit.to.as_deref().unwrap_or("its reference"),
                part.mpn,
                mv(margin),
            );
            let hot_note = format!(
                "copper at {} °C: {from}, plus the {} °C a track may rise",
                hot + policy.rise,
                policy.rise
            );
            let worst = load.amps * z_hot;
            let floor = volts.nets[n].floor;
            if load.known() && worst <= margin {
                push(
                    Verdict::Pass,
                    format!(
                        "drops at most {}: the net's {:.3} A through {} of copper from {s}; {need}; {hot_note}{copper_note}",
                        mv(worst),
                        load.amps,
                        mohm(z_hot)
                    ),
                );
            } else if let Some(i) = own.filter(|&i| !pour && !floor && i * z_cold > margin) {
                push(
                    Verdict::Fail,
                    format!(
                        "drops {} on its own {i:.3} A through {} of copper from {s}; {need}; copper at {cold} °C: \
                         {from}{copper_note}",
                        mv(i * z_cold),
                        mohm(z_cold)
                    ),
                );
            } else {
                push(
                    Verdict::Unknown,
                    format!(
                        "may drop up to {}: {}the net's {:.3} A through {} of copper from {s}, but how much of it \
                         reaches this pin isn't known; {need}; {hot_note}{copper_note}",
                        mv(worst),
                        if load.known() { "" } else { "at least " },
                        load.amps,
                        mohm(z_hot)
                    ),
                );
            }
        }
    }
    out
}
