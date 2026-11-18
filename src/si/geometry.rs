use std::collections::HashMap;

use super::route::{Piece, Route};
use super::xsection::{CrossSection, Slab, Solution};
use crate::kicad::pcb::{Layout, Point, StackLayer, Zone, dist};

pub const SAMPLE: f64 = 0.1;

const COUPLED_GAP: f64 = 5.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub layer: String,
    pub start: Point,
    pub end: Point,
    pub length: f64,
    pub width: f64,
    pub gap: Option<f64>,
    pub planes: Planes,
    pub plane_gap: bool,
    pub plane_edge: Option<(f64, Point)>,
    pub solution: Solution,
    pub loss_tangent: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Planes {
    pub below: Option<(usize, String)>,
    pub above: Option<(usize, String)>,
}

impl Section {
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

    fn joins(&self, next: &Section) -> bool {
        self.layer == next.layer
            && self.width == next.width
            && self.planes == next.planes
            && self.plane_gap == next.plane_gap
            && self.gap.map(quantize_gap) == next.gap.map(quantize_gap)
            && dist(self.end, next.start) < 1e-6
    }

    pub fn z_diff(&self) -> f64 {
        self.solution.z_diff()
    }
}

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

    fn cross_section(
        &self,
        i: usize,
        width: f64,
        gap: Option<f64>,
        below: Option<usize>,
        above: Option<usize>,
    ) -> (CrossSection, f64) {
        let (_, t0, t1) = self.copper[i];
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

fn quantize_gap(g: f64) -> f64 {
    let step = 0.005_f64.max(0.02 * g);
    let step = 0.005 * (step / 0.005).round();
    (g / step).round() * step
}

fn zone_has(z: &Zone, p: Point) -> bool {
    z.polygons.iter().any(|poly| poly_has(poly, p))
}

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

fn zone_edge(edges: &[(Point, Point)], p: Point, voids: &[(Point, f64)]) -> f64 {
    let hole = |q: Point| voids.iter().any(|(c, r)| dist(*c, q) <= *r);
    edges
        .iter()
        .filter(|(a, b)| !(hole(*a) && hole(*b)))
        .map(|(a, b)| seg_dist(p, *a, *b))
        .fold(f64::INFINITY, f64::min)
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

type SolveKey = (usize, i64, Option<i64>, Option<usize>, Option<usize>);

pub struct Analyzer<'a> {
    layout: &'a Layout,
    stack: Stack,
    voids: Vec<Vec<(Point, f64)>>,
    outlines: Vec<Vec<(Point, Point)>>,
    cache: HashMap<SolveKey, (Solution, f64)>,
}

impl<'a> Analyzer<'a> {
    pub fn new(layout: &'a Layout) -> Self {
        let stack = Stack::new(layout);
        let mut voids = vec![Vec::new(); stack.copper.len()];
        for (j, (name, _, _)) in stack.copper.iter().enumerate() {
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

    fn planes(&self, i: usize, p: Point) -> (Planes, bool, Option<f64>) {
        let n = self.stack.copper.len();
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

    pub fn solve(&mut self, i: usize, width: f64, gap: Option<f64>, planes: &Planes) -> Option<(Solution, f64)> {
        let (b, a) = (planes.below.as_ref().map(|p| p.0), planes.above.as_ref().map(|p| p.0));
        if b.is_none() && a.is_none() {
            return None;
        }
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
            let gap = others
                .iter()
                .filter(|o| o.layer == piece.layer && o.length() > 0.0)
                .filter_map(|o| {
                    let ol = o.length();
                    let cos = ((o.end.0 - o.start.0) * dx + (o.end.1 - o.start.1) * dy) / ol;
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

    fn height(&self, i: usize, planes: &Planes) -> Option<f64> {
        let (_, t0, t1) = self.stack.copper[i];
        let below = planes.below.as_ref().map(|(j, _)| self.stack.copper[*j].1 - t1);
        let above = planes.above.as_ref().map(|(j, _)| t0 - self.stack.copper[*j].2);
        match (below, above) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub fn lone(&mut self, layer: &str, width: f64, p: Point) -> Option<(Solution, f64)> {
        let i = self.stack.index(layer)?;
        let (planes, _, _) = self.planes(i, p);
        self.solve(i, width, None, &planes)
    }

    pub fn span(&self, a: &str, b: &str) -> Option<f64> {
        let (i, j) = (self.stack.index(a)?, self.stack.index(b)?);
        let (i, j) = (i.min(j), i.max(j));
        Some(self.stack.copper[j].2 - self.stack.copper[i].1)
    }

    pub fn layer_name(&self, i: usize) -> &str {
        &self.stack.copper[i].0
    }

    pub fn span_total(&self) -> f64 {
        self.layout.thickness()
    }

    pub fn antipad_clearance(&self) -> f64 {
        self.layout
            .zones
            .iter()
            .map(|z| z.clearance)
            .fold(f64::INFINITY, f64::min)
            .min(0.5)
    }

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
