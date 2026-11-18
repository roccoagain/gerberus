use std::path::Path;

use crate::sexpr::{self, Sexp};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PadNet {
    pub reference: String,
    pub pad: String,
    pub net: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footprint {
    pub reference: String,
    pub lib_id: String,
    pub board_only: bool,
    pub pads: Vec<Pad>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub number: String,
    pub kind: String,
    pub net: Option<String>,
}

impl Footprint {
    pub fn pad_numbers(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self.pads.iter().map(|p| p.number.as_str()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

pub fn load_footprints(path: &Path) -> Result<Vec<Footprint>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let tree = sexpr::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if tree.head() != Some("kicad_pcb") {
        return Err(format!("{}: not a KiCad board", path.display()));
    }
    let mut out = Vec::new();
    for fp in tree.children("footprint") {
        let reference = fp
            .children("property")
            .find(|p| p.arg(1) == Some("Reference"))
            .and_then(|p| p.arg(2))
            .or_else(|| {
                fp.children("fp_text")
                    .find(|t| t.arg(1) == Some("reference"))
                    .and_then(|t| t.arg(2))
            })
            .unwrap_or_default()
            .to_string();
        let pads = fp
            .children("pad")
            .filter_map(|pad| {
                let number = pad.arg(1).unwrap_or_default();
                (!number.is_empty()).then(|| Pad {
                    number: number.to_string(),
                    kind: pad.arg(2).unwrap_or_default().to_string(),
                    net: pad_net(pad),
                })
            })
            .collect();
        out.push(Footprint {
            reference,
            lib_id: fp.arg(1).unwrap_or_default().to_string(),
            board_only: fp
                .child("attr")
                .is_some_and(|a| a.items().iter().skip(1).any(|i| i.text() == Some("board_only"))),
            pads,
        });
    }
    Ok(out)
}

pub fn load_pad_nets(path: &Path) -> Result<Vec<PadNet>, String> {
    let mut out: Vec<PadNet> = load_footprints(path)?
        .into_iter()
        .flat_map(|fp| {
            let reference = fp.reference;
            fp.pads.into_iter().map(move |p| PadNet {
                reference: reference.clone(),
                pad: p.number,
                net: p.net,
            })
        })
        .collect();
    out.sort();
    out.dedup();
    Ok(out)
}

fn pad_net(pad: &Sexp) -> Option<String> {
    let net = pad.child("net")?;
    let name = if net.items().len() >= 3 { net.arg(2) } else { net.arg(1) };
    name.filter(|n| !n.is_empty()).map(str::to_string)
}

pub type Point = (f64, f64);

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub stackup: Vec<StackLayer>,
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    pub zones: Vec<Zone>,
    pub footprints: Vec<PlacedFootprint>,
    pub assumptions: Vec<String>,
    pub stackup_assumed: bool,
    pub zone_nets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StackLayer {
    Copper { name: String, thickness: f64 },
    Dielectric { thickness: f64, er: f64, loss_tangent: f64 },
    Mask { thickness: f64, er: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub net: String,
    pub layer: String,
    pub start: Point,
    pub end: Point,
    pub width: f64,
}

impl Track {
    pub fn length(&self) -> f64 {
        dist(self.start, self.end)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Via {
    pub net: String,
    pub at: Point,
    pub diameter: f64,
    pub drill: f64,
    pub layers: (String, String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub net: String,
    pub layer: String,
    pub polygons: Vec<Vec<Point>>,
    pub clearance: f64,
    pub min_thickness: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedFootprint {
    pub reference: String,
    pub value: String,
    pub lib_id: String,
    pub at: Point,
    pub pads: Vec<PlacedPad>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedPad {
    pub number: String,
    pub net: Option<String>,
    pub at: Point,
    pub size: (f64, f64),
    pub rotation: f64,
    pub layers: Vec<String>,
}

impl PlacedPad {
    pub fn contains(&self, p: Point, margin: f64) -> bool {
        let (dx, dy) = (p.0 - self.at.0, p.1 - self.at.1);
        let a = self.rotation.to_radians();
        let (c, s) = (a.cos(), a.sin());
        let lx = dx * c - dy * s;
        let ly = dx * s + dy * c;
        lx.abs() <= self.size.0 / 2.0 + margin && ly.abs() <= self.size.1 / 2.0 + margin
    }
}

impl Layout {
    pub fn copper_layers(&self) -> Vec<&str> {
        self.stackup
            .iter()
            .filter_map(|l| match l {
                StackLayer::Copper { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn thickness(&self) -> f64 {
        self.stackup
            .iter()
            .map(|l| match l {
                StackLayer::Copper { thickness, .. }
                | StackLayer::Dielectric { thickness, .. }
                | StackLayer::Mask { thickness, .. } => *thickness,
            })
            .sum()
    }
}

pub fn dist(a: Point, b: Point) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

pub fn load_layout(path: &Path) -> Result<Layout, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let tree = sexpr::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if tree.head() != Some("kicad_pcb") {
        return Err(format!("{}: not a KiCad board", path.display()));
    }
    Ok(parse_layout(&tree))
}

struct NetNames(std::collections::BTreeMap<String, String>);

impl NetNames {
    fn of(&self, item: &Sexp) -> Option<String> {
        if let Some(name) = item.child("net_name").and_then(|n| n.arg(1)) {
            return Some(name.to_string()).filter(|n| !n.is_empty());
        }
        let net = item.child("net")?;
        let name = match net.items() {
            [_, _, name, ..] => name.text(),
            [_, Sexp::Atom(id)] => self.0.get(id).map(String::as_str),
            [_, Sexp::Str(name)] => Some(name.as_str()),
            _ => None,
        }?;
        Some(name.to_string()).filter(|n| !n.is_empty())
    }
}

fn point(s: &Sexp, name: &str) -> Option<Point> {
    let c = s.child(name)?;
    Some((c.arg_f64(1)?, c.arg_f64(2)?))
}

fn parse_layout(tree: &Sexp) -> Layout {
    let mut out = Layout::default();
    let nets = NetNames(
        tree.children("net")
            .filter_map(|n| Some((n.arg(1)?.to_string(), n.arg(2)?.to_string())))
            .collect(),
    );

    let mut copper: Vec<String> = tree
        .child("layers")
        .map(|l| {
            l.items()
                .iter()
                .filter_map(|e| e.arg(1))
                .filter(|n| n.ends_with(".Cu"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let rank = |n: &str| match n {
        "F.Cu" => 0,
        "B.Cu" => u32::MAX,
        _ => n
            .strip_prefix("In")
            .and_then(|r| r.strip_suffix(".Cu"))
            .and_then(|r| r.parse().ok())
            .unwrap_or(u32::MAX - 1),
    };
    copper.sort_by_key(|n| rank(n));

    out.stackup = tree
        .child("setup")
        .and_then(|s| s.child("stackup"))
        .map(read_stackup)
        .unwrap_or_default();
    if !out.stackup.iter().any(|l| matches!(l, StackLayer::Dielectric { .. })) {
        let thickness = tree
            .child("general")
            .and_then(|g| g.child("thickness"))
            .and_then(|t| t.arg_f64(1))
            .unwrap_or(1.6);
        out.stackup = default_stackup(&copper, thickness);
        out.stackup_assumed = true;
        out.assumptions.push(format!(
            "the board has no stackup, so it is taken as {thickness} mm of FR4 (εr 4.5, loss tangent 0.02) split \
             evenly between {} copper layers of 35 µm, with no solder mask",
            copper.len()
        ));
    }

    for seg in tree.children("segment") {
        let (Some(start), Some(end), Some(net)) = (point(seg, "start"), point(seg, "end"), nets.of(seg)) else {
            continue;
        };
        out.tracks.push(Track {
            net,
            layer: seg
                .child("layer")
                .and_then(|l| l.arg(1))
                .unwrap_or_default()
                .to_string(),
            start,
            end,
            width: seg.child("width").and_then(|w| w.arg_f64(1)).unwrap_or(0.0),
        });
    }
    for arc in tree.children("arc") {
        let (Some(a), Some(m), Some(b), Some(net)) =
            (point(arc, "start"), point(arc, "mid"), point(arc, "end"), nets.of(arc))
        else {
            continue;
        };
        let layer = arc.child("layer").and_then(|l| l.arg(1)).unwrap_or_default();
        let width = arc.child("width").and_then(|w| w.arg_f64(1)).unwrap_or(0.0);
        let pts = flatten_arc(a, m, b);
        for w in pts.windows(2) {
            out.tracks.push(Track {
                net: net.clone(),
                layer: layer.to_string(),
                start: w[0],
                end: w[1],
                width,
            });
        }
    }
    for v in tree.children("via") {
        let (Some(at), Some(net)) = (point(v, "at"), nets.of(v)) else {
            continue;
        };
        let names: Vec<&str> = v
            .child("layers")
            .map(|l| l.items().iter().skip(1).filter_map(Sexp::text).collect())
            .unwrap_or_default();
        let (from, to) = match names.as_slice() {
            [a, b, ..] => ((*a).to_string(), (*b).to_string()),
            _ => ("F.Cu".to_string(), "B.Cu".to_string()),
        };
        out.vias.push(Via {
            net,
            at,
            diameter: v.child("size").and_then(|s| s.arg_f64(1)).unwrap_or(0.0),
            drill: v.child("drill").and_then(|s| s.arg_f64(1)).unwrap_or(0.0),
            layers: (from, to),
        });
    }
    for z in tree.children("zone") {
        let Some(net) = nets.of(z) else { continue };
        if !out.zone_nets.contains(&net) {
            out.zone_nets.push(net.clone());
        }
        let clearance = z
            .child("connect_pads")
            .and_then(|c| c.child("clearance"))
            .and_then(|c| c.arg_f64(1))
            .unwrap_or(0.5);
        let min_thickness = z.child("min_thickness").and_then(|c| c.arg_f64(1)).unwrap_or(0.25);
        let zone_layer = z.child("layer").and_then(|l| l.arg(1)).map(str::to_string);
        for f in z.children("filled_polygon") {
            let layer = f
                .child("layer")
                .and_then(|l| l.arg(1))
                .map(str::to_string)
                .or_else(|| zone_layer.clone())
                .unwrap_or_default();
            let pts: Vec<Point> = f
                .child("pts")
                .map(|p| {
                    p.children("xy")
                        .filter_map(|xy| Some((xy.arg_f64(1)?, xy.arg_f64(2)?)))
                        .collect()
                })
                .unwrap_or_default();
            if pts.len() < 3 {
                continue;
            }
            match out.zones.iter_mut().find(|o| o.net == net && o.layer == layer) {
                Some(o) => o.polygons.push(pts),
                None => out.zones.push(Zone {
                    net: net.clone(),
                    layer,
                    polygons: vec![pts],
                    clearance,
                    min_thickness,
                }),
            }
        }
    }

    for fp in tree.children("footprint") {
        let (fx, fy, frot) = fp
            .child("at")
            .map(|a| {
                (
                    a.arg_f64(1).unwrap_or(0.0),
                    a.arg_f64(2).unwrap_or(0.0),
                    a.arg_f64(3).unwrap_or(0.0),
                )
            })
            .unwrap_or_default();
        let text_of = |kind: &str| {
            fp.children("property")
                .find(|p| p.arg(1).is_some_and(|k| k.eq_ignore_ascii_case(kind)))
                .and_then(|p| p.arg(2))
                .or_else(|| {
                    fp.children("fp_text")
                        .find(|t| t.arg(1).is_some_and(|k| k.eq_ignore_ascii_case(kind)))
                        .and_then(|t| t.arg(2))
                })
                .unwrap_or_default()
                .to_string()
        };
        let (c, s) = (frot.to_radians().cos(), frot.to_radians().sin());
        let pads = fp
            .children("pad")
            .filter_map(|pad| {
                let number = pad.arg(1).unwrap_or_default();
                let at = pad.child("at")?;
                let (px, py) = (at.arg_f64(1)?, at.arg_f64(2)?);
                let size = pad.child("size").map_or((0.0, 0.0), |s| {
                    (s.arg_f64(1).unwrap_or(0.0), s.arg_f64(2).unwrap_or(0.0))
                });
                let layers: Vec<String> = pad
                    .child("layers")
                    .map(|l| {
                        l.items()
                            .iter()
                            .skip(1)
                            .filter_map(Sexp::text)
                            .flat_map(|n| expand_copper(n, &copper))
                            .collect()
                    })
                    .unwrap_or_default();
                if layers.is_empty() {
                    return None;
                }
                Some(PlacedPad {
                    number: number.to_string(),
                    net: pad_net(pad),
                    at: (fx + px * c + py * s, fy - px * s + py * c),
                    size,
                    rotation: at.arg_f64(3).unwrap_or(0.0),
                    layers,
                })
            })
            .collect();
        out.footprints.push(PlacedFootprint {
            reference: text_of("reference"),
            value: text_of("value"),
            lib_id: fp.arg(1).unwrap_or_default().to_string(),
            at: (fx, fy),
            pads,
        });
    }
    out
}

fn expand_copper(name: &str, copper: &[String]) -> Vec<String> {
    match name {
        "*.Cu" => copper.to_vec(),
        "F&B.Cu" => vec!["F.Cu".to_string(), "B.Cu".to_string()],
        n if n.ends_with(".Cu") => vec![n.to_string()],
        _ => vec![],
    }
}

fn read_stackup(s: &Sexp) -> Vec<StackLayer> {
    let mut out = Vec::new();
    for l in s.children("layer") {
        let name = l.arg(1).unwrap_or_default();
        let kind = l
            .child("type")
            .and_then(|t| t.arg(1))
            .unwrap_or_default()
            .to_ascii_lowercase();
        let thickness = l.child("thickness").and_then(|t| t.arg_f64(1)).unwrap_or(0.0);
        let er = l.child("epsilon_r").and_then(|t| t.arg_f64(1));
        if kind == "copper" {
            out.push(StackLayer::Copper {
                name: name.to_string(),
                thickness,
            });
        } else if kind.contains("solder mask") {
            out.push(StackLayer::Mask {
                thickness,
                er: er.unwrap_or(3.3),
            });
        } else if kind == "core" || kind == "prepreg" || name.starts_with("dielectric") {
            let all = |key: &str| -> Vec<Option<f64>> { l.children(key).map(|t| t.arg_f64(1)).collect() };
            let (mut thicknesses, ers, tans) = (all("thickness"), all("epsilon_r"), all("loss_tangent"));
            if thicknesses.is_empty() {
                thicknesses.push(None);
            }
            for (i, t) in thicknesses.iter().enumerate() {
                out.push(StackLayer::Dielectric {
                    thickness: t.unwrap_or(0.0),
                    er: ers.get(i).copied().flatten().or(er).unwrap_or(4.5),
                    loss_tangent: tans.get(i).copied().flatten().unwrap_or(0.02),
                });
            }
        }
    }
    out
}

fn default_stackup(copper: &[String], thickness: f64) -> Vec<StackLayer> {
    let cu = 0.035;
    let n = copper.len().max(2);
    let dielectric = ((thickness - cu * n as f64) / (n - 1) as f64).max(0.05);
    let mut out = Vec::new();
    for (i, name) in copper.iter().enumerate() {
        if i > 0 {
            out.push(StackLayer::Dielectric {
                thickness: dielectric,
                er: 4.5,
                loss_tangent: 0.02,
            });
        }
        out.push(StackLayer::Copper {
            name: name.clone(),
            thickness: cu,
        });
    }
    out
}

fn flatten_arc(a: Point, m: Point, b: Point) -> Vec<Point> {
    let d = 2.0 * (a.0 * (m.1 - b.1) + m.0 * (b.1 - a.1) + b.0 * (a.1 - m.1));
    if d.abs() < 1e-12 {
        return vec![a, b];
    }
    let sq = |p: Point| p.0 * p.0 + p.1 * p.1;
    let cx = (sq(a) * (m.1 - b.1) + sq(m) * (b.1 - a.1) + sq(b) * (a.1 - m.1)) / d;
    let cy = (sq(a) * (b.0 - m.0) + sq(m) * (a.0 - b.0) + sq(b) * (m.0 - a.0)) / d;
    let r = dist(a, (cx, cy));
    let ang = |p: Point| (p.1 - cy).atan2(p.0 - cx);
    let (t0, tm, t1) = (ang(a), ang(m), ang(b));
    let tau = std::f64::consts::TAU;
    let ccw = |from: f64, to: f64| (to - from).rem_euclid(tau);
    let sweep = if ccw(t0, tm) <= ccw(t0, t1) {
        ccw(t0, t1)
    } else {
        -ccw(t1, t0)
    };
    let steps = (sweep.abs() / 10f64.to_radians()).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|i| {
            if i == steps {
                return b;
            }
            let t = t0 + sweep * i as f64 / steps as f64;
            (cx + r * t.cos(), cy + r * t.sin())
        })
        .collect()
}
