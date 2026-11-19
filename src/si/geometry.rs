//! What each stretch of a routed line looks like electrically: its layer and width, how close its partner
//! runs, which planes it sits over, and the impedance the field solver gives that cross-section.
use std::collections::HashMap;

use super::route::{Piece, Route};
use super::xsection::{CrossSection, Slab, Solution};
use crate::kicad::pcb::{Layout, Point, StackLayer, Zone, dist};

/// Length of the samples a line is cut into, mm.
pub const SAMPLE: f64 = 0.1;

/// A gap wider than this many times the height over the plane leaves the two traces uncoupled.
const COUPLED_GAP: f64 = 5.0;

/// One stretch of a line with a single cross-section.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub layer: String,
    pub start: Point,
    pub end: Point,
    pub length: f64,
    pub width: f64,
    /// Edge-to-edge gap to the partner trace when it runs alongside.
    pub gap: Option<f64>,
    /// The planes it is referenced to: the nearest copper layer with plane copper under it on each side.
    pub planes: Planes,
    /// The plane copper is missing on a layer next to this one, so the reference is farther away.
    pub plane_gap: bool,
    /// Closest distance to the edge of its reference plane, mm, and where.
    pub plane_edge: Option<(f64, Point)>,
    pub solution: Solution,
    /// Loss tangent of the dielectric under it.
    pub loss_tangent: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Planes {
    /// Copper layer index (top = 0) of the plane below, and the net it carries.
    pub below: Option<(usize, String)>,
    pub above: Option<(usize, String)>,
}

impl Section {
    /// An empty section that ends a run.
    pub fn sentinel() -> Section {
        Section {
            layer: String::new(),
            start: (0.0, 0.0),
            end: (0.0, 0.0),
            length: 0.0,
            width: 0.0,
            gap: None,
            planes: Planes {
                below: None,
                above: None,
            },
            plane_gap: false,
            plane_edge: None,
            solution: Solution {
                odd: super::xsection::Mode { z: 45.0, er_eff: 1.0 },
                even: None,
            },
            loss_tangent: 0.0,
        }
    }

    /// Whether `next` carries straight on from this section with the same cross-section.
    fn joins(&self, next: &Section) -> bool {
        self.layer == next.layer
            && self.width == next.width
            && self.planes == next.planes
            && self.plane_gap == next.plane_gap
            && self.gap.map(quantize_gap) == next.gap.map(quantize_gap)
            && dist(self.end, next.start) < 1e-6
    }

    /// Differential impedance when coupled, or twice the lone trace's impedance.
    pub fn z_diff(&self) -> f64 {
        self.solution.z_diff()
    }
}

/// Depth (from the board's top surface, mm) of the top and bottom of each copper layer, and the dielectric
/// after it.
struct Stack {
    copper: Vec<(String, f64, f64)>,
    items: Vec<(f64, f64, StackLayer)>,
}

impl Stack {
    fn new(layout: &Layout) -> Stack {
        let mut depth = 0.0;
        let mut copper = Vec::new();
        let mut items = Vec::new();
        for l in &layout.stackup {
            let t = match l {
                StackLayer::Copper { thickness, .. }
                | StackLayer::Dielectric { thickness, .. }
                | StackLayer::Mask { thickness, .. } => *thickness,
            };
            if let StackLayer::Copper { name, .. } = l {
                copper.push((name.clone(), depth, depth + t));
            }
            items.push((depth, depth + t, l.clone()));
            depth += t;
        }
        Stack { copper, items }
    }

    fn index(&self, layer: &str) -> Option<usize> {
        self.copper.iter().position(|c| c.0 == layer)
    }

    /// The permittivity at a depth, for copper that isn't the trace or a plane: the dielectric next to it.
    fn er_near(&self, depth: f64) -> (f64, f64) {
        let mut best = (4.5, 0.02, f64::INFINITY);
        for (a, b, l) in &self.items {
            if let StackLayer::Dielectric { er, loss_tangent, .. } = l {
                let d = if depth < *a {
                    a - depth
                } else if depth > *b {
                    depth - b
                } else {
                    0.0
                };
                if d < best.2 {
                    best = (*er, *loss_tangent, d);
                }
            }
        }
        (best.0, best.1)
    }

    /// The cross-section of a trace on copper layer `i` between plane layers `below` and `above` (indices).
    fn cross_section(
        &self,
        i: usize,
        width: f64,
        gap: Option<f64>,
        below: Option<usize>,
        above: Option<usize>,
    ) -> (CrossSection, f64) {
        let (_, t0, t1) = self.copper[i];
        // Measure height from the near face of the nearer plane, toward the trace.
        let (origin, sign, other) = match (below, above) {
            (Some(b), a) => (self.copper[b].1, -1.0, a.map(|a| self.copper[a].2)),
            (None, Some(a)) => (self.copper[a].2, 1.0, None),
            (None, None) => unreachable!("callers only ask with a plane"),
        };
        let z = |depth: f64| sign * (depth - origin);
        let (z_t0, z_t1) = (z(t0), z(t1));
        let height = z_t0.min(z_t1);
        let thickness = (z_t1 - z_t0).abs();
        let top_plane = other.map(z);
        let lo = 0.0;
        let hi = top_plane.unwrap_or(f64::INFINITY);
        let mut slabs = Vec::new();
        let outer = i == 0 || i + 1 == self.copper.len();
        let mut tan_d = 0.0;
        for (a, b, l) in &self.items {
            let (za, zb) = (z(*a).min(z(*b)), z(*a).max(z(*b)));
            let (za, zb) = (za.max(lo), zb.min(hi));
            if zb <= za {
                continue;
            }
            match l {
                StackLayer::Dielectric { er, loss_tangent, .. } => {
                    slabs.push(Slab {
                        bottom: za,
                        top: zb,
                        er: *er,
                    });
                    if za < height {
                        tan_d = *loss_tangent;
                    }
                }
                StackLayer::Mask { er, .. } => {
                    slabs.push(Slab {
                        bottom: za,
                        top: zb,
                        er: *er,
                    });
                }
                StackLayer::Copper { name, .. } => {
                    // The trace's own layer between traces: solder mask on an outer layer, the neighbouring
                    // dielectric inside the board. Unused copper layers count as dielectric too.
                    let er = if *name == self.copper[i].0 && outer {
                        self.items
                            .iter()
                            .find_map(|(_, _, l)| match l {
                                StackLayer::Mask { er, .. } => Some(*er),
                                _ => None,
                            })
                            .unwrap_or(1.0)
                    } else {
                        self.er_near((a + b) / 2.0).0
                    };
                    slabs.push(Slab {
                        bottom: za,
                        top: zb,
                        er,
                    });
                }
            }
        }
        if tan_d == 0.0 {
            tan_d = self.er_near(f64::midpoint(t0, t1)).1;
        }
        (
            CrossSection {
                width,
                thickness,
                gap,
                height,
                top_plane,
                slabs,
            },
            tan_d,
        )
    }
}

fn closer(a: Option<(f64, Point)>, b: Option<(f64, Point)>) -> Option<(f64, Point)> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.0 < x.0 { y } else { x }),
        (x, y) => x.or(y),
    }
}

/// A gap rounded to the grid the solver uses: 5 µm, or 2 % of the gap when that is coarser.
fn quantize_gap(g: f64) -> f64 {
    let step = 0.005_f64.max(0.02 * g);
    let step = 0.005 * (step / 0.005).round();
    (g / step).round() * step
}

/// Whether the even-odd rule puts `p` inside the zone's copper.
fn zone_has(z: &Zone, p: Point) -> bool {
    z.polygons.iter().any(|poly| poly_has(poly, p))
}

/// Even-odd test against one filled polygon. Each of KiCad's filled polygons carries its own holes (joined by
/// slits), so polygons are tested one at a time: where two zones of the same net overlap, the copper is solid.
fn poly_has(poly: &[Point], p: Point) -> bool {
    let mut inside = false;
    let n = poly.len();
    for k in 0..n {
        let (a, b) = (poly[k], poly[(k + 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < a.0 + (p.1 - a.1) * (b.0 - a.0) / (b.1 - a.1) {
            inside = !inside;
        }
    }
    inside
}

/// The zone's real outline: every edge except the zero-width slits KiCad cuts to join each hole to its
/// outline, which come in pairs running back and forth along the same line (to within a few nanometres).
fn outline(z: &Zone) -> Vec<(Point, Point)> {
    const CELL: f64 = 0.01;
    let cell = |p: Point| ((p.0 / CELL).floor() as i64, (p.1 / CELL).floor() as i64);
    let edges: Vec<(usize, Point, Point)> = z
        .polygons
        .iter()
        .enumerate()
        .flat_map(|(i, poly)| (0..poly.len()).map(move |k| (i, poly[k], poly[(k + 1) % poly.len()])))
        .collect();
    let mut by_start: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (_, a, _)) in edges.iter().enumerate() {
        by_start.entry(cell(*a)).or_default().push(i);
    }
    let close = |p: Point, q: Point| dist(p, q) < 2e-3;
    edges
        .iter()
        .filter(|(i, a, b)| {
            // An edge inside another polygon of the same net (an overlapping zone) borders copper on both sides.
            let mid = (f64::midpoint(a.0, b.0), f64::midpoint(a.1, b.1));
            !z.polygons
                .iter()
                .enumerate()
                .any(|(j, poly)| j != *i && poly_has(poly, mid))
        })
        .filter(|(_, a, b)| {
            let (cx, cy) = cell(*b);
            let reversed = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dy| (cx + dx, cy + dy)))
                .any(|c| {
                    by_start
                        .get(&c)
                        .is_some_and(|list| list.iter().any(|&j| close(edges[j].1, *b) && close(edges[j].2, *a)))
                });
            !reversed
        })
        .map(|&(_, a, b)| (a, b))
        .collect()
}

/// Distance from `p` to the nearest of `edges`, leaving out the clearance holes around `voids` (vias and
/// through-hole pads).
fn zone_edge(edges: &[(Point, Point)], p: Point, voids: &[(Point, f64)]) -> f64 {
    let hole = |q: Point| voids.iter().any(|(c, r)| dist(*c, q) <= *r);
    let mut nearest = f64::INFINITY;
    for (a, b) in edges {
        // The hole test costs a pass over every void, so it only runs for an edge that would be the nearest.
        let d = seg_dist(p, *a, *b);
        if d < nearest && !(hole(*a) && hole(*b)) {
            nearest = d;
        }
    }
    nearest
}

fn seg_dist(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    dist(p, (a.0 + dx * t, a.1 + dy * t))
}

/// What a cross-section solve depends on: layer, width and gap (quantised), and the planes below and above.
type SolveKey = (usize, i64, Option<i64>, Option<usize>, Option<usize>);

/// Cuts routes into sections, solving each distinct cross-section once and remembering the result.
pub struct Analyzer<'a> {
    layout: &'a Layout,
    stack: Stack,
    /// Holes a plane leaves around vias and through-hole pads, by copper layer: centre and radius, mm.
    voids: Vec<Vec<(Point, f64)>>,
    /// Each zone's outline without its slits, in the order of `layout.zones`.
    outlines: Vec<Vec<(Point, Point)>>,
    cache: HashMap<SolveKey, (Solution, f64)>,
}

impl<'a> Analyzer<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        let stack = Stack::new(layout);
        let mut voids = vec![Vec::new(); stack.copper.len()];
        for (j, (name, _, _)) in stack.copper.iter().enumerate() {
            // The hole is the clearance, plus any sliver thinner than the fill's minimum width beside it.
            let clearance = layout
                .zones
                .iter()
                .filter(|z| &z.layer == name)
                .map(|z| z.clearance + z.min_thickness)
                .fold(f64::NAN, f64::min);
            if clearance.is_nan() {
                continue;
            }
            for v in &layout.vias {
                let (a, b) = (stack.index(&v.layers.0), stack.index(&v.layers.1));
                if let (Some(a), Some(b)) = (a, b)
                    && (a.min(b)..=a.max(b)).contains(&j)
                {
                    voids[j].push((v.at, v.diameter / 2.0 + clearance));
                }
            }
            for f in &layout.footprints {
                for p in f.pads.iter().filter(|p| p.layers.len() > 1 && p.layers.contains(name)) {
                    voids[j].push((p.at, p.size.0.max(p.size.1) / 2.0 + clearance));
                }
            }
        }
        let outlines = layout.zones.iter().map(outline).collect();
        Analyzer {
            layout,
            stack,
            voids,
            outlines,
            cache: HashMap::new(),
        }
    }

    /// The planes under and over `p` on layer `i`: for each side, the nearest copper layer with zone copper
    /// at `p`, and whether that is farther than the next layer.
    fn planes(&self, i: usize, p: Point) -> (Planes, bool, Option<f64>) {
        let n = self.stack.copper.len();
        // A via's or through-hole pad's clearance hole counts as plane: the via model covers it, and a trace
        // always crosses its own via's hole.
        let in_void = |j: usize| self.voids[j].iter().any(|(c, r)| dist(*c, p) <= *r);
        let covered = |j: usize| -> Option<usize> {
            let name = &self.stack.copper[j].0;
            let zones = || {
                self.layout
                    .zones
                    .iter()
                    .enumerate()
                    .filter(move |(_, z)| &z.layer == name)
            };
            zones().find(|(_, z)| zone_has(z, p)).map(|(k, _)| k).or_else(|| {
                let reach = self.voids[j].iter().map(|v| v.1).fold(0.0, f64::max);
                if in_void(j) {
                    zones()
                        .find(|(k, _)| zone_edge(&self.outlines[*k], p, &[]) < reach)
                        .map(|(k, _)| k)
                } else {
                    None
                }
            })
        };
        let below = (i + 1..n).find_map(|j| covered(j).map(|z| (j, z)));
        let above = (0..i).rev().find_map(|j| covered(j).map(|z| (j, z)));
        // A gap: the layer right next to the trace has no plane here, though the trace has a plane side.
        let gap_below = i + 1 < n && below.is_none_or(|(j, _)| j != i + 1);
        let gap_above = i > 0 && above.is_none_or(|(j, _)| j + 1 != i);
        let outer = i == 0 || i + 1 == n;
        let plane_gap = if outer {
            (i == 0 && gap_below) || (i + 1 == n && gap_above)
        } else {
            gap_below && gap_above
        };
        let near = match (below, above) {
            (Some((j, z)), _) if j == i + 1 => Some(z),
            (_, Some((j, z))) if j + 1 == i => Some(z),
            _ => None,
        };
        let near_layer = match (&below, &above) {
            (Some((j, _)), _) if *j == i + 1 => Some(*j),
            (_, Some((j, _))) if *j + 1 == i => Some(*j),
            _ => None,
        };
        let edge = near
            .zip(near_layer)
            .map(|(k, j)| zone_edge(&self.outlines[k], p, &self.voids[j]));
        (
            Planes {
                below: below.map(|(j, k)| (j, self.layout.zones[k].net.clone())),
                above: above.map(|(j, k)| (j, self.layout.zones[k].net.clone())),
            },
            plane_gap,
            edge,
        )
    }

    /// Solves (or recalls) the cross-section of a trace.
    pub fn solve(&mut self, i: usize, width: f64, gap: Option<f64>, planes: &Planes) -> Option<(Solution, f64)> {
        let (b, a) = (planes.below.as_ref().map(|p| p.0), planes.above.as_ref().map(|p| p.0));
        if b.is_none() && a.is_none() {
            return None;
        }
        // Gaps are solved on a grid of 5 µm, or 2 % of the gap when that is coarser: either moves a pair's
        // impedance by well under 1 %, and wide gaps (where the pair splits around a pad) don't each need a
        // solve of their own.
        let gap = gap.map(quantize_gap);
        let key = (
            i,
            (width * 1e4).round() as i64,
            gap.map(|g| (g * 1e4).round() as i64),
            b,
            a,
        );
        if let Some(s) = self.cache.get(&key) {
            return Some(*s);
        }
        let (x, tan_d) = self.stack.cross_section(i, width, gap, b, a);
        let s = (x.solve(), tan_d);
        self.cache.insert(key, s);
        Some(s)
    }

    /// The sections of `route`, with `partner` (the pair's other line) for coupling, merged where they
    /// match. Pieces with no plane on either side are left out and returned in the second list.
    pub fn sections(&mut self, route: &Route, partner: Option<&Route>) -> (Vec<Section>, Vec<Piece>) {
        let others: Vec<Piece> = partner.map(|r| r.pieces().cloned().collect()).unwrap_or_default();
        let mut out: Vec<Section> = Vec::new();
        let mut unreferenced = Vec::new();
        for piece in route.pieces() {
            let (secs, bare) = self.piece_sections(piece, &others);
            unreferenced.extend(bare);
            for sec in secs {
                match out.last_mut() {
                    Some(l) if l.joins(&sec) => {
                        l.end = sec.end;
                        l.length += sec.length;
                        l.plane_edge = closer(l.plane_edge, sec.plane_edge);
                    }
                    _ => out.push(sec),
                }
            }
        }
        (out, unreferenced)
    }

    /// The sections of one piece, in order from its start.
    pub fn piece_sections(&mut self, piece: &Piece, others: &[Piece]) -> (Vec<Section>, Vec<Piece>) {
        let mut out: Vec<Section> = Vec::new();
        let mut unreferenced = Vec::new();
        let Some(i) = self.stack.index(&piece.layer) else {
            return (out, unreferenced);
        };
        let len = piece.length();
        if len <= 0.0 {
            return (out, unreferenced);
        }
        let n = (len / SAMPLE).ceil().max(1.0) as usize;
        let step = len / n as f64;
        let (dx, dy) = ((piece.end.0 - piece.start.0) / len, (piece.end.1 - piece.start.1) / len);
        for k in 0..n {
            let s = k as f64 * step;
            let a = (piece.start.0 + dx * s, piece.start.1 + dy * s);
            let b = (a.0 + dx * step, a.1 + dy * step);
            let mid = (f64::midpoint(a.0, b.0), f64::midpoint(a.1, b.1));
            let (planes, plane_gap, edge) = self.planes(i, mid);
            let h = self.height(i, &planes).unwrap_or(1.0);
            // The partner's nearest parallel piece on the same layer.
            let gap = others
                .iter()
                .filter(|o| o.layer == piece.layer && o.length() > 0.0)
                .filter_map(|o| {
                    let ol = o.length();
                    let cos = ((o.end.0 - o.start.0) * dx + (o.end.1 - o.start.1) * dy) / ol;
                    // Only where the partner runs alongside: the sample projects onto it, not past its ends.
                    let t = ((mid.0 - o.start.0) * (o.end.0 - o.start.0) + (mid.1 - o.start.1) * (o.end.1 - o.start.1))
                        / (ol * ol);
                    (cos.abs() > 0.9 && (0.0..=1.0).contains(&t))
                        .then(|| seg_dist(mid, o.start, o.end) - f64::midpoint(piece.width, o.width))
                })
                .filter(|g| *g > 0.0 && *g < COUPLED_GAP * h)
                .min_by(f64::total_cmp);
            let Some((solution, loss_tangent)) = self.solve(i, piece.width, gap, &planes) else {
                unreferenced.push(Piece {
                    layer: piece.layer.clone(),
                    start: a,
                    end: b,
                    width: piece.width,
                });
                continue;
            };
            let sec = Section {
                layer: piece.layer.clone(),
                start: a,
                end: b,
                length: step,
                width: piece.width,
                gap,
                planes,
                plane_gap,
                plane_edge: edge.map(|d| (d, mid)),
                solution,
                loss_tangent,
            };
            match out.last_mut() {
                Some(l) if l.joins(&sec) => {
                    l.end = b;
                    l.length += step;
                    l.plane_edge = closer(l.plane_edge, edge.map(|d| (d, mid)));
                }
                _ => out.push(sec),
            }
        }
        (out, unreferenced)
    }

    /// Height of a trace on layer `i` over its nearer plane, mm.
    fn height(&self, i: usize, planes: &Planes) -> Option<f64> {
        let (_, t0, t1) = self.stack.copper[i];
        let below = planes.below.as_ref().map(|(j, _)| self.stack.copper[*j].1 - t1);
        let above = planes.above.as_ref().map(|(j, _)| t0 - self.stack.copper[*j].2);
        match (below, above) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The impedance of a lone trace at `p` on `layer`, for stubs.
    pub fn lone(&mut self, layer: &str, width: f64, p: Point) -> Option<(Solution, f64)> {
        let i = self.stack.index(layer)?;
        let (planes, _, _) = self.planes(i, p);
        self.solve(i, width, None, &planes)
    }

    /// The distance between two copper layers, mm, for via models.
    pub fn span(&self, a: &str, b: &str) -> Option<f64> {
        let (i, j) = (self.stack.index(a)?, self.stack.index(b)?);
        let (i, j) = (i.min(j), i.max(j));
        Some(self.stack.copper[j].2 - self.stack.copper[i].1)
    }

    /// The name of copper layer `i` (top = 0).
    pub fn layer_name(&self, i: usize) -> &str {
        &self.stack.copper[i].0
    }

    /// Board thickness, mm.
    pub fn span_total(&self) -> f64 {
        self.layout.thickness()
    }

    /// The gap planes keep around a via, mm: the smallest zone clearance on the board (KiCad's default is
    /// 0.5 mm).
    pub fn antipad_clearance(&self) -> f64 {
        self.layout
            .zones
            .iter()
            .map(|z| z.clearance)
            .fold(f64::INFINITY, f64::min)
            .min(0.5)
    }

    /// The permittivity of the board's dielectric, thickness-weighted, for via capacitance.
    pub fn mean_er(&self) -> f64 {
        let (mut sum, mut t) = (0.0, 0.0);
        for (a, b, l) in &self.stack.items {
            if let StackLayer::Dielectric { er, .. } = l {
                sum += er * (b - a);
                t += b - a;
            }
        }
        if t > 0.0 { sum / t } else { 4.5 }
    }
}
