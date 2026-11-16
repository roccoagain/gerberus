//! How one net is routed: the copper path from the driver's pad to the connector's, and everything that hangs
//! off it (stubs).

use std::collections::{BTreeMap, BinaryHeap, HashMap};

use crate::kicad::pcb::{Layout, Point, dist};

/// Distance under which two points count as the same, mm. KiCad stores coordinates in nanometres.
const SAME: f64 = 1e-4;

/// One straight piece of copper on the path or a stub.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    pub layer: String,
    pub start: Point,
    pub end: Point,
    pub width: f64,
}

impl Piece {
    pub fn length(&self) -> f64 {
        dist(self.start, self.end)
    }
}

/// What the signal meets, in order from the driver's pad.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Track(Piece),
    /// A via, entered on one layer and left on another (the same one when the via only hangs off the path).
    Via {
        at: Point,
        from: String,
        to: String,
        drill: f64,
        diameter: f64,
    },
    /// A pad the path runs through, such as an ESD diode placed on the trace.
    Pad {
        reference: String,
        number: String,
    },
    /// Copper hanging off the path here.
    Stub(Stub),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stub {
    /// Where it leaves the path.
    pub at: Point,
    /// Track length from the path to its farthest end, mm.
    pub length: f64,
    /// The layer and width of its first track, when it has one.
    pub layer: Option<String>,
    pub width: Option<f64>,
    pub vias: usize,
    /// Pads it reaches, as `(reference, pad number)`.
    pub pads: Vec<(String, String)>,
}

/// A net routed from one pad to another.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub net: String,
    /// `(reference, pad number)` of the two ends.
    pub from: (String, String),
    pub to: (String, String),
    pub steps: Vec<Step>,
    /// Pads on the net that no copper reaches.
    pub unrouted: Vec<(String, String)>,
}

impl Route {
    /// Track length from end to end, mm.
    pub fn length(&self) -> f64 {
        self.pieces().map(Piece::length).sum()
    }

    pub fn pieces(&self) -> impl Iterator<Item = &Piece> {
        self.steps.iter().filter_map(|s| match s {
            Step::Track(p) => Some(p),
            _ => None,
        })
    }

    pub fn vias(&self) -> usize {
        self.steps.iter().filter(|s| matches!(s, Step::Via { .. })).count()
    }

    pub fn stubs(&self) -> impl Iterator<Item = &Stub> {
        self.steps.iter().filter_map(|s| match s {
            Step::Stub(b) => Some(b),
            _ => None,
        })
    }
}

#[derive(Debug, Clone)]
enum Kind {
    Track(Piece),
    /// From a via's hub to its copper on one layer.
    Via(usize),
    /// From a pad to copper that touches it.
    Pad,
}

#[derive(Debug, Clone)]
struct Edge {
    a: usize,
    b: usize,
    length: f64,
    kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Copper { layer: String, at: Point },
    ViaHub(usize),
    Pad { reference: String, number: String },
}

struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    adj: Vec<Vec<usize>>,
    lookup: HashMap<(String, i64, i64), usize>,
}

impl Graph {
    fn copper(&mut self, layer: &str, at: Point) -> usize {
        let key = (
            layer.to_string(),
            (at.0 / SAME).round() as i64,
            (at.1 / SAME).round() as i64,
        );
        if let Some(&n) = self.lookup.get(&key) {
            return n;
        }
        self.nodes.push(Node::Copper {
            layer: layer.to_string(),
            at,
        });
        self.adj.push(Vec::new());
        self.lookup.insert(key, self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    fn other(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.adj.push(Vec::new());
        self.nodes.len() - 1
    }

    fn link(&mut self, a: usize, b: usize, length: f64, kind: Kind) {
        if a == b {
            return;
        }
        self.edges.push(Edge { a, b, length, kind });
        let e = self.edges.len() - 1;
        self.adj[a].push(e);
        self.adj[b].push(e);
    }

    fn far(&self, e: usize, from: usize) -> usize {
        let edge = &self.edges[e];
        if edge.a == from { edge.b } else { edge.a }
    }
}

/// Nearest point to `p` on segment `a`–`b`, as the fraction along it.
fn project(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return 0.0;
    }
    (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
}

fn lerp(a: Point, b: Point, t: f64) -> Point {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

/// Builds the copper graph of `net`.
fn graph(layout: &Layout, net: &str) -> (Graph, Vec<(usize, String, String)>) {
    let copper = layout.copper_layers();
    let span = |a: &str, b: &str| -> Vec<String> {
        let (i, j) = (
            copper.iter().position(|l| *l == a).unwrap_or(0),
            copper
                .iter()
                .position(|l| *l == b)
                .unwrap_or(copper.len().saturating_sub(1)),
        );
        let (i, j) = (i.min(j), i.max(j));
        copper[i..=j].iter().map(|s| (*s).to_string()).collect()
    };
    let tracks: Vec<_> = layout.tracks.iter().filter(|t| t.net == net).collect();
    let vias: Vec<_> = layout.vias.iter().filter(|v| v.net == net).collect();
    let pads: Vec<_> = layout
        .footprints
        .iter()
        .flat_map(|f| f.pads.iter().map(move |p| (f, p)))
        .filter(|(_, p)| p.net.as_deref() == Some(net))
        .collect();

    let mut g = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
        adj: Vec::new(),
        lookup: HashMap::new(),
    };

    // Split each track wherever another track ends on it, a via sits on it or it runs across a pad's centre
    // line, so that everything joins at a node.
    for t in &tracks {
        let mut cuts: Vec<(f64, Point)> = vec![(0.0, t.start), (1.0, t.end)];
        let near = |p: Point, slack: f64| {
            let f = project(p, t.start, t.end);
            (dist(lerp(t.start, t.end, f), p) <= slack).then_some(f)
        };
        for o in &tracks {
            if o.layer != t.layer {
                continue;
            }
            for p in [o.start, o.end] {
                if let Some(f) = near(p, t.width / 2.0 + SAME) {
                    cuts.push((f, p));
                }
            }
        }
        for v in &vias {
            if span(&v.layers.0, &v.layers.1).contains(&t.layer)
                && let Some(f) = near(v.at, v.diameter / 2.0)
            {
                cuts.push((f, v.at));
            }
        }
        for (_, pad) in &pads {
            if !pad.layers.contains(&t.layer) {
                continue;
            }
            let f = project(pad.at, t.start, t.end);
            let p = lerp(t.start, t.end, f);
            if pad.contains(p, 0.0) && !pad.contains(t.start, 0.0) && !pad.contains(t.end, 0.0) {
                cuts.push((f, p));
            }
        }
        cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        cuts.dedup_by(|b, a| dist(a.1, b.1) < SAME);
        for w in cuts.windows(2) {
            let piece = Piece {
                layer: t.layer.clone(),
                start: w[0].1,
                end: w[1].1,
                width: t.width,
            };
            let (a, b) = (g.copper(&t.layer, piece.start), g.copper(&t.layer, piece.end));
            g.link(a, b, piece.length(), Kind::Track(piece));
        }
    }

    // A via joins its copper on every layer it passes through.
    for (i, v) in vias.iter().enumerate() {
        let hub = g.other(Node::ViaHub(i));
        for layer in span(&v.layers.0, &v.layers.1) {
            let ends: Vec<usize> = (0..g.nodes.len())
                .filter(|&n| match &g.nodes[n] {
                    Node::Copper { layer: l, at } => *l == layer && dist(*at, v.at) <= v.diameter / 2.0 + SAME,
                    _ => false,
                })
                .collect();
            if ends.is_empty() {
                let n = g.copper(&layer, v.at);
                g.link(hub, n, 0.0, Kind::Via(i));
            }
            for n in ends {
                g.link(hub, n, 0.0, Kind::Via(i));
            }
        }
    }

    // A pad joins the copper ending on it.
    let mut pad_nodes = Vec::new();
    for (f, pad) in &pads {
        let node = g.other(Node::Pad {
            reference: f.reference.clone(),
            number: pad.number.clone(),
        });
        pad_nodes.push((node, f.reference.clone(), pad.number.clone()));
        let touching: Vec<(usize, f64)> = (0..g.nodes.len())
            .filter_map(|n| match &g.nodes[n] {
                Node::Copper { layer, at } if pad.layers.contains(layer) && pad.contains(*at, SAME) => {
                    Some((n, dist(*at, pad.at)))
                }
                _ => None,
            })
            .collect();
        for (n, d) in touching {
            g.link(node, n, d, Kind::Pad);
        }
    }
    (g, pad_nodes)
}

#[derive(PartialEq)]
struct Visit(f64, usize);
impl Eq for Visit {}
impl PartialOrd for Visit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Visit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}

/// Shortest track distance from `sources` to every node, and the edge each was reached by.
fn dijkstra(g: &Graph, sources: &[usize]) -> (Vec<f64>, Vec<Option<usize>>) {
    let mut d = vec![f64::INFINITY; g.nodes.len()];
    let mut via = vec![None; g.nodes.len()];
    let mut heap = BinaryHeap::new();
    for &s in sources {
        d[s] = 0.0;
        heap.push(Visit(0.0, s));
    }
    while let Some(Visit(du, u)) = heap.pop() {
        if du > d[u] {
            continue;
        }
        for &e in &g.adj[u] {
            let v = g.far(e, u);
            let dv = du + g.edges[e].length;
            if dv < d[v] - 1e-12 {
                d[v] = dv;
                via[v] = Some(e);
                heap.push(Visit(dv, v));
            }
        }
    }
    (d, via)
}

/// Routes `net` from the pad `driver` picks out to the nearest pad `is_end` accepts. With no driver given,
/// the path starts at the pad farthest from every `is_end` pad, which is the PHY on a USB line.
pub fn trace(
    layout: &Layout,
    net: &str,
    is_end: impl Fn(&str) -> bool,
    driver: Option<&(String, String)>,
) -> Result<Route, String> {
    let (g, pads) = graph(layout, net);
    let ends: Vec<usize> = pads.iter().filter(|(_, r, _)| is_end(r)).map(|(n, _, _)| *n).collect();
    if ends.is_empty() {
        return Err(format!("{net}: no connector pad on the net"));
    }
    let (d, _) = dijkstra(&g, &ends);
    let unrouted: Vec<(String, String)> = pads
        .iter()
        .filter(|(n, _, _)| d[*n].is_infinite())
        .map(|(_, r, p)| (r.clone(), p.clone()))
        .collect();
    let start = match driver {
        Some((r, p)) => pads
            .iter()
            .find(|(_, pr, pp)| pr == r && pp == p)
            .map(|(n, _, _)| *n)
            .ok_or_else(|| format!("{net}: no pad {r}.{p}"))?,
        None => pads
            .iter()
            .filter(|(n, r, _)| !is_end(r) && d[*n].is_finite())
            .max_by(|a, b| d[a.0].total_cmp(&d[b.0]))
            .map(|(n, _, _)| *n)
            .ok_or_else(|| format!("{net}: no routed pad other than the connector's"))?,
    };
    if d[start].is_infinite() {
        return Err(format!("{net}: the driver's pad has no copper path to the connector"));
    }

    // Walk from the driver to the connector along the shortest path.
    let (_, back) = dijkstra(&g, &[start]);
    let end = *ends
        .iter()
        .min_by(|a, b| {
            let da = path_len(&g, &back, **a);
            let db = path_len(&g, &back, **b);
            da.total_cmp(&db)
        })
        .expect("ends is not empty");
    let mut path_edges = Vec::new();
    let mut n = end;
    while n != start {
        let e = back[n].expect("end is reachable from start");
        path_edges.push(e);
        n = g.far(e, n);
    }
    path_edges.reverse();
    let mut path_nodes = vec![start];
    for &e in &path_edges {
        let last = *path_nodes.last().expect("not empty");
        path_nodes.push(g.far(e, last));
    }
    let on_path: std::collections::BTreeSet<usize> = path_nodes.iter().copied().collect();
    let path_edge_set: std::collections::BTreeSet<usize> = path_edges.iter().copied().collect();

    let node_at = |n: usize| -> Option<Point> {
        match &g.nodes[n] {
            Node::Copper { at, .. } => Some(*at),
            _ => None,
        }
    };
    let via_list: Vec<_> = layout.vias.iter().filter(|v| v.net == net).collect();
    let mut steps = Vec::new();
    let mut seen_branch = std::collections::BTreeSet::new();
    for (k, &node) in path_nodes.iter().enumerate() {
        // Stubs leaving this node.
        for &e in &g.adj[node] {
            if path_edge_set.contains(&e) || seen_branch.contains(&e) {
                continue;
            }
            let first = g.far(e, node);
            if on_path.contains(&first) {
                continue;
            }
            let stub = explore(&g, node, e, &on_path, &mut seen_branch, node_at(node));
            if stub.length > SAME || !stub.pads.is_empty() || stub.vias > 0 {
                steps.push(Step::Stub(stub));
            }
        }
        if let Node::Pad { reference, number } = &g.nodes[node]
            && k > 0
            && k + 1 < path_nodes.len()
        {
            steps.push(Step::Pad {
                reference: reference.clone(),
                number: number.clone(),
            });
        }
        let Some(&e) = path_edges.get(k) else { break };
        let next = path_nodes[k + 1];
        match &g.edges[e].kind {
            Kind::Track(p) => {
                // Orient the piece the way the signal travels.
                let forward = node_at(node).is_some_and(|a| dist(a, p.start) < SAME);
                let mut piece = p.clone();
                if !forward {
                    std::mem::swap(&mut piece.start, &mut piece.end);
                }
                steps.push(Step::Track(piece));
            }
            Kind::Via(i) => {
                // Entering the hub; the step is recorded with the layer it is left on.
                if let Node::ViaHub(_) = g.nodes[next] {
                    let v = via_list[*i];
                    let from = layer_of(&g, node).unwrap_or_default();
                    let out_node = path_nodes.get(k + 2).copied();
                    let to = out_node.and_then(|o| layer_of(&g, o)).unwrap_or_else(|| from.clone());
                    steps.push(Step::Via {
                        at: v.at,
                        from,
                        to,
                        drill: v.drill,
                        diameter: v.diameter,
                    });
                }
            }
            Kind::Pad => {}
        }
    }

    let pad_of = |n: usize| match &g.nodes[n] {
        Node::Pad { reference, number } => (reference.clone(), number.clone()),
        _ => (String::new(), String::new()),
    };
    Ok(Route {
        net: net.to_string(),
        from: pad_of(start),
        to: pad_of(end),
        steps,
        unrouted,
    })
}

fn layer_of(g: &Graph, n: usize) -> Option<String> {
    match &g.nodes[n] {
        Node::Copper { layer, .. } => Some(layer.clone()),
        _ => None,
    }
}

fn path_len(g: &Graph, back: &[Option<usize>], mut n: usize) -> f64 {
    let mut total = 0.0;
    let mut guard = 0;
    while let Some(e) = back[n] {
        total += g.edges[e].length;
        n = g.far(e, n);
        guard += 1;
        if guard > g.edges.len() {
            return f64::INFINITY;
        }
    }
    total
}

/// Everything reachable from `root` through `first` without touching the path again.
fn explore(
    g: &Graph,
    root: usize,
    first: usize,
    on_path: &std::collections::BTreeSet<usize>,
    seen: &mut std::collections::BTreeSet<usize>,
    at: Option<Point>,
) -> Stub {
    let mut stub = Stub {
        at: at.unwrap_or_default(),
        length: 0.0,
        layer: None,
        width: None,
        vias: 0,
        pads: Vec::new(),
    };
    if let Kind::Track(p) = &g.edges[first].kind {
        stub.layer = Some(p.layer.clone());
        stub.width = Some(p.width);
    }
    let mut depth: BTreeMap<usize, f64> = BTreeMap::new();
    depth.insert(root, 0.0);
    let mut stack = vec![(root, first)];
    let mut vias = std::collections::BTreeSet::new();
    while let Some((from, e)) = stack.pop() {
        if !seen.insert(e) {
            continue;
        }
        let to = g.far(e, from);
        if on_path.contains(&to) {
            continue;
        }
        let d = depth[&from] + g.edges[e].length;
        if depth.get(&to).is_some_and(|&old| old <= d) {
            continue;
        }
        depth.insert(to, d);
        match &g.nodes[to] {
            Node::Pad { reference, number } => stub.pads.push((reference.clone(), number.clone())),
            Node::ViaHub(i) => {
                vias.insert(*i);
            }
            Node::Copper { .. } => {}
        }
        if stub.layer.is_none()
            && let Kind::Track(p) = &g.edges[e].kind
        {
            stub.layer = Some(p.layer.clone());
            stub.width = Some(p.width);
        }
        for &next in &g.adj[to] {
            if next != e {
                stack.push((to, next));
            }
        }
    }
    stub.vias = vias.len();
    stub.length = depth.values().copied().fold(0.0, f64::max);
    stub.pads.sort();
    stub.pads.dedup();
    stub
}
