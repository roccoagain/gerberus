//! Voltage drop: does every power pin still get its recommended minimum after the copper between it and the part
//! that supplies its net?
//!
//! The copper of a net is a resistor network. Each track is split wherever another track, a via or a pad touches
//! it, anywhere along it. A via joins the layers it reaches through its barrel, chained layer to layer by their
//! depth in the stackup. The pads that bring the net its current (a regulator's power output, an input's connector
//! pin, a series part or switch) are joined into one node, held at the net's lowest voltage. Solving the network
//! gives `Z`, the resistance from there to a power pin's pads. In a network of resistors fed from one node, any
//! current makes at most `Z` times itself of drop at the pin, and the pin's own current makes exactly that. So:
//!
//! - PASS: the net's whole current times `Z` (thinnest copper, via barrels at the thinnest plating, hottest board)
//!   leaves the pin its recommended minimum, and the pin's reference net (its ground) has a pour, whose drop is
//!   far smaller than a track's.
//! - FAIL: the pin's own maximum current times `Z` (thickest copper, vias as no resistance, coldest board) is
//!   already too much, on a net with no copper pour (a pour adds paths the network doesn't count, which only lower
//!   `Z`).
//! - Otherwise UNKNOWN.
//!
//! A pour's resistance isn't worked out, so a pin joined to its supply only through a pour is UNKNOWN.
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

/// The copper of one net as a resistor network, in ohms per Ω·mm of resistivity.
pub struct Network {
    /// Edges between nodes, with their resistance.
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
    (t, dist(p, at(a, b, t)))
}

fn at(a: Point, b: Point, t: f64) -> Point {
    (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1))
}

/// The closest points of segments `a` and `b`, as positions along each, and the distance between them.
fn closest(a: (Point, Point), b: (Point, Point)) -> (f64, f64, f64) {
    let cross = |p: Point, q: Point, x: Point| (q.0 - p.0) * (x.1 - p.1) - (q.1 - p.1) * (x.0 - p.0);
    let (d1, d2) = (cross(a.0, a.1, b.0), cross(a.0, a.1, b.1));
    let (d3, d4) = (cross(b.0, b.1, a.0), cross(b.0, b.1, a.1));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        // They cross: where, along each.
        let s = d3 / (d3 - d4);
        let t = d1 / (d1 - d2);
        return (s, t, 0.0);
    }
    // Otherwise the closest pair has an end of one of them.
    let candidates = [(0.0, along(a.0, b.0, b.1)), (1.0, along(a.1, b.0, b.1))]
        .map(|(s, (t, d))| (s, t, d))
        .into_iter()
        .chain([(b.0, 0.0), (b.1, 1.0)].map(|(p, t)| {
            let (s, d) = along(p, a.0, a.1);
            (s, t, d)
        }));
    candidates.min_by(|x, y| x.2.total_cmp(&y.2)).expect("four candidates")
}

impl Network {
    /// The tracks and vias of `net`, with copper `thickness` (mm, by layer name). With `barrels`, each via adds its
    /// barrel between the layers it joins; without, a via joins them with no resistance.
    pub fn build(
        layout: &Layout,
        net: &str,
        pads: &[&PlacedPad],
        thickness: &BTreeMap<String, f64>,
        barrels: bool,
    ) -> Result<Self, String> {
        let tracks: Vec<_> = layout.tracks.iter().filter(|t| t.net == net).collect();
        let vias: Vec<_> = layout.vias.iter().filter(|v| v.net == net).collect();
        // Each copper layer's depth below the top of the board, mm.
        let mut depth = 0.0;
        let mut layers: Vec<(String, f64)> = vec![];
        for l in &layout.stackup {
            match l {
                StackLayer::Copper { name, thickness } => {
                    layers.push((name.clone(), depth + thickness / 2.0));
                    depth += thickness;
                }
                StackLayer::Dielectric { thickness, .. } | StackLayer::Mask { thickness, .. } => depth += thickness,
            }
        }
        if let Some(t) = tracks.iter().find(|t| !thickness.contains_key(&t.layer)) {
            return Err(format!("layer {} isn't in the board's stackup", t.layer));
        }
        let mut nodes = 0;
        fn fresh(nodes: &mut usize) -> usize {
            *nodes += 1;
            *nodes - 1
        }
        let mut edges = vec![];
        let mut joins: Vec<(usize, usize)> = vec![];
        // Each via: a node on each copper layer it reaches, top to bottom, joined through its barrel.
        let mut via_at: Vec<BTreeMap<String, usize>> = vec![];
        for v in &vias {
            let find = |name: &str| layers.iter().position(|(l, _)| l == name);
            let (Some(a), Some(b)) = (find(&v.layers.0), find(&v.layers.1)) else {
                return Err(format!(
                    "a via joins layers {} and {}, which aren't both in the board's stackup",
                    v.layers.0, v.layers.1
                ));
            };
            // The plating's ring around the finished hole, `drill` across before plating.
            let ring = std::f64::consts::PI * (v.drill - PLATING) * PLATING;
            let mut on = BTreeMap::new();
            let mut last: Option<(usize, f64)> = None;
            for (name, z) in &layers[a.min(b)..=a.max(b)] {
                let n = fresh(&mut nodes);
                if let Some((m, zm)) = last {
                    if barrels {
                        edges.push((m, n, (z - zm) / ring));
                    } else {
                        joins.push((m, n));
                    }
                }
                last = Some((n, *z));
                on.insert(name.clone(), n);
            }
            via_at.push(on);
        }
        // Points along each track where something touches it, as (position along it, node).
        let mut points: Vec<Vec<(f64, usize)>> = tracks
            .iter()
            .map(|_| vec![(0.0, fresh(&mut nodes)), (1.0, fresh(&mut nodes))])
            .collect();
        let point = |points: &mut Vec<Vec<(f64, usize)>>, nodes: &mut usize, i: usize, s: f64| {
            let n = fresh(nodes);
            points[i].push((s, n));
            n
        };
        for (i, t) in tracks.iter().enumerate() {
            let half = t.width / 2.0;
            for (j, u) in tracks.iter().enumerate().skip(i + 1) {
                if u.layer != t.layer {
                    continue;
                }
                let (s, w, d) = closest((t.start, t.end), (u.start, u.end));
                if d <= half + u.width / 2.0 + 1e-6 {
                    let (a, b) = (
                        point(&mut points, &mut nodes, i, s),
                        point(&mut points, &mut nodes, j, w),
                    );
                    joins.push((a, b));
                }
            }
            for (k, v) in vias.iter().enumerate() {
                let Some(&on) = via_at[k].get(&t.layer) else { continue };
                let (s, d) = along(v.at, t.start, t.end);
                if d <= v.diameter / 2.0 + half + 1e-6 {
                    let n = point(&mut points, &mut nodes, i, s);
                    joins.push((n, on));
                }
            }
        }
        // Pads: a node each, joined where a track runs into them and to every via inside them.
        let pad_nodes: Vec<usize> = pads.iter().map(|_| fresh(&mut nodes)).collect();
        let mut reached: Vec<bool> = vec![false; pads.len()];
        for (p, pad) in pads.iter().enumerate() {
            for (i, t) in tracks.iter().enumerate() {
                if !pad.layers.iter().any(|l| l == &t.layer || l == "*.Cu") {
                    continue;
                }
                let (s, _) = along(pad.at, t.start, t.end);
                if pad.contains(at(t.start, t.end, s), t.width / 2.0) {
                    let n = point(&mut points, &mut nodes, i, s);
                    joins.push((pad_nodes[p], n));
                    reached[p] = true;
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
            let th = thickness[&t.layer];
            let len = t.length();
            let pts = &mut points[i];
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            for w in pts.windows(2) {
                let piece = (w[1].0 - w[0].0) * len;
                if piece < 1e-6 {
                    // Two points at the same place on the track.
                    joins.push((w[0].1, w[1].1));
                } else {
                    edges.push((w[0].1, w[1].1, piece / (t.width * th)));
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
        Ok(Network {
            edges,
            nodes: next,
            pads,
        })
    }

    /// The resistance from the nodes in `from` (joined) to each group of nodes in `to` (each joined), or `None`
    /// where no copper joins them.
    pub fn resistance(&self, from: &BTreeSet<usize>, to: &[BTreeSet<usize>]) -> Vec<Option<f64>> {
        to.iter().map(|group| self.between(from, group)).collect()
    }

    fn between(&self, from: &BTreeSet<usize>, to: &BTreeSet<usize>) -> Option<f64> {
        if from.is_empty() || to.is_empty() {
            return None;
        }
        if !from.is_disjoint(to) {
            return Some(0.0);
        }
        // Merge each terminal group into one node: `s` and `t`.
        let (s, t) = (self.nodes, self.nodes + 1);
        let map = |n: usize| {
            if from.contains(&n) {
                s
            } else if to.contains(&n) {
                t
            } else {
                n
            }
        };
        let mut edges: Vec<(usize, usize, f64)> = self
            .edges
            .iter()
            .map(|&(a, b, r)| (map(a), map(b), r))
            .filter(|(a, b, _)| a != b)
            .collect();
        reduce(&mut edges, &[s, t]);
        // The conductance matrix over every node but `s`, which is the reference.
        let mut free: Vec<usize> = edges.iter().flat_map(|&(a, b, _)| [a, b]).filter(|&n| n != s).collect();
        free.sort_unstable();
        free.dedup();
        let index: BTreeMap<usize, usize> = free.iter().enumerate().map(|(i, &n)| (n, i)).collect();
        let &ti = index.get(&t)?;
        let k = free.len();
        let mut g = vec![vec![0.0; k]; k];
        for &(a, b, r) in &edges {
            let c = 1.0 / r;
            match (index.get(&a), index.get(&b)) {
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
        // Nodes not joined to `s` make the matrix singular; so does `t` being one of them.
        let l = cholesky(&g)?;
        let mut e = vec![0.0; k];
        e[ti] = 1.0;
        Some(solve(&l, &e)[ti])
    }
}

/// Removes what can't change the resistance between `keep`: nodes with one edge (dead ends) and nodes with two
/// (a chain, merged into one edge), repeated until none is left.
fn reduce(edges: &mut Vec<(usize, usize, f64)>, keep: &[usize]) {
    loop {
        let mut degree: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, &(a, b, _)) in edges.iter().enumerate() {
            degree.entry(a).or_default().push(i);
            degree.entry(b).or_default().push(i);
        }
        let Some((&n, at)) = degree.iter().find(|(n, at)| !keep.contains(n) && at.len() <= 2) else {
            return;
        };
        match at[..] {
            [i] => {
                edges.swap_remove(i);
            }
            [i, j] => {
                let (a, b) = (edges[i], edges[j]);
                let far = |e: (usize, usize, f64)| if e.0 == n { e.1 } else { e.0 };
                let (x, y) = (far(a), far(b));
                let (hi, lo) = (i.max(j), i.min(j));
                edges.swap_remove(hi);
                edges.swap_remove(lo);
                if x != y {
                    edges.push((x, y, a.2 + b.2));
                }
            }
            _ => unreachable!("at most two edges"),
        }
    }
}

/// The lower-triangular `L` with `L Lᵀ = g`, for a symmetric positive-definite `g`; `None` when it isn't.
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

/// The parts that bring a net its current, each with its pins on the net that carry it: the connector pin an
/// input arrives on; a regulator's power output pins; each two-pin part (other than a capacitor) joining the net to
/// a net its voltage comes from or to the switch node of a regulator that sets it (a switcher's inductor); and a
/// MOSFET channel's end.
fn suppliers(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    volts: &Voltages,
    net: usize,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut add = |r: &str, pin: &str| {
        let pins = out.entry(r.to_string()).or_default();
        if !pins.iter().any(|p| p == pin) {
            pins.push(pin.to_string());
        }
    };
    let on_net = |r: &str| -> Vec<(&String, &crate::design::Pin)> {
        design
            .components
            .get(r)
            .map(|c| c.pins.iter().filter(|(_, p)| p.net == net).collect())
            .unwrap_or_default()
    };
    for i in inputs.inputs.iter().filter(|i| i.fault.is_none()) {
        if on_net(&i.pin.reference).iter().any(|(n, _)| **n == i.pin.pin) {
            add(&i.pin.reference, &i.pin.pin);
        }
    }
    let regulators: BTreeSet<&str> = volts
        .regulators
        .iter()
        .filter(|r| r.output == net && r.sets_output)
        .map(|r| r.reference.as_str())
        .collect();
    for r in &regulators {
        for (n, p) in on_net(r) {
            if p.kind == PinKind::PowerOut {
                add(r, n);
            }
        }
    }
    let mut from: BTreeSet<usize> = volts.nets[net]
        .why
        .iter()
        .flat_map(|w| w.from.iter().copied())
        .collect();
    // A switching regulator's switch node, which its inductor joins to the net.
    for r in &regulators {
        let c = &design.components[*r];
        let Some(part) = lib.part_for(c) else { continue };
        for d in part
            .drives
            .iter()
            .filter(|d| d.kind == crate::limits::DriveKind::Switches)
        {
            from.extend(
                part.pins_named(&d.pin)
                    .into_iter()
                    .filter_map(|num| c.pins.get(num))
                    .map(|p| p.net),
            );
        }
    }
    from.remove(&net);
    for (r, c) in &design.components {
        let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
        if !c.dnp && !crate::design::is_capacitor(r, c) && nets.len() == 2 && nets.iter().any(|n| from.contains(n)) {
            for (n, _) in on_net(r) {
                add(r, n);
            }
        }
    }
    for ch in &volts.channels {
        let ends = [ch.drain, ch.source];
        if ends.contains(&net) && ends.iter().any(|n| from.contains(n)) {
            for (n, p) in on_net(&ch.reference) {
                if p.net != ch.gate.unwrap_or(usize::MAX) {
                    add(&ch.reference, n);
                }
            }
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
                    Range::Unknown(_) | Range::None(_) => None,
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
        let supply = suppliers(design, lib, inputs, volts, n);
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
        let [(s, source_pins)] = supply.iter().collect::<Vec<_>>()[..] else {
            let why = if supply.is_empty() {
                "no regulator output, input or series part is found bringing the net its current, so there is \
                 nothing to measure the copper from"
                    .to_string()
            } else {
                format!(
                    "the net is supplied from {}, so how much each supplies isn't known",
                    supply.keys().cloned().collect::<Vec<_>>().join(", ")
                )
            };
            out.extend(sinks.iter().map(|k| finding(&k.0, &k.1, Verdict::Unknown, why.clone())));
            continue;
        };
        let source: Vec<usize> = (0..pads.len())
            .filter(|&i| pads[i].0 == s && source_pins.contains(&pads[i].1.number))
            .collect();
        // Every pad of each sink's pin: a pin split over several pads takes its current through all of them.
        let sink_pads: Vec<Vec<usize>> = sinks
            .iter()
            .map(|k| {
                (0..pads.len())
                    .filter(|&i| pads[i].0 == k.0 && pads[i].1.number == k.1)
                    .collect()
            })
            .collect();
        // Resistance per Ω·mm of resistivity: the thinnest copper with via barrels for a pass, the thickest with
        // none for a fail.
        let unit = |thickness: &BTreeMap<String, f64>, barrels: bool| -> Result<Vec<Option<f64>>, String> {
            let copper = Network::build(layout, &net.name, &pad_refs, thickness, barrels)?;
            let from: BTreeSet<usize> = source.iter().filter_map(|&i| copper.pads[i]).collect();
            let to: Vec<BTreeSet<usize>> = sink_pads
                .iter()
                .map(|g| g.iter().filter_map(|&i| copper.pads[i]).collect())
                .collect();
            Ok(copper.resistance(&from, &to))
        };
        let (z_thin, z_thick) = match (unit(&thin, true), unit(&thick, false)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                out.extend(sinks.iter().map(|k| finding(&k.0, &k.1, Verdict::Unknown, e.clone())));
                continue;
            }
        };
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
            if sink_pads[k].is_empty() {
                push(Verdict::Unknown, format!("{r}.{p} has no pad on the board"));
                continue;
            }
            if source.is_empty() {
                push(
                    Verdict::Unknown,
                    format!("{s}'s pins that bring the net its current have no pads on the board"),
                );
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
            // The current comes back through the reference net: a plane adds far less than a track, but a
            // routed return isn't counted here.
            let return_plane = {
                let name = &design.nets[to].name;
                layout.zones.iter().any(|z| &z.net == name)
            };
            let return_note = if return_plane {
                String::new()
            } else {
                format!(
                    "; {} has no copper pour, so the drop in its return path isn't known",
                    design.nets[to].name
                )
            };
            if load.known() && return_plane && worst <= margin {
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
                         reaches this pin isn't known; {need}; {hot_note}{copper_note}{return_note}",
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
