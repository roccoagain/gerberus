//! Plane splits: does every signal track keep one unbroken plane beneath it?
//!
//! A signal's return current flows in the plane next to its track. Where the copper under the track's two ends
//! is two separate pieces (a split between ground islands, or between a ground and a power pour), the return
//! current has to detour around the gap, which makes a loop that radiates and picks up noise.
//!
//! Every quarter millimetre along a track is matched to the filled piece of copper beneath it on each neighbouring
//! copper layer. A track is referenced when, on some neighbouring layer, one piece lies under every point that has
//! copper beneath it: a hole in that piece, such as a via's or pad's clearance, doesn't break it. One finding per
//! signal net (a net with no zone of its own):
//!
//! - PASS: every track is referenced.
//! - FAIL (warning): some track runs over two or more separate pieces on every neighbouring layer that has copper
//!   under it.
//! - UNKNOWN: some track has no filled copper under it on any neighbouring layer.
use std::collections::BTreeSet;

use crate::design::Design;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, Point, StackLayer, Track};

/// Whether `p` is inside `polygon`, by the even-odd rule.
fn inside(p: Point, polygon: &[Point]) -> bool {
    let mut odd = false;
    let n = polygon.len();
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        if (a.1 > p.1) != (b.1 > p.1) {
            let x = a.0 + (p.1 - a.1) / (b.1 - a.1) * (b.0 - a.0);
            if p.0 < x {
                odd = !odd;
            }
        }
    }
    odd
}

/// The filled pieces of copper under `p` on `layer`, as (zone, polygon) indices.
fn pieces(layout: &Layout, layer: &str, p: Point) -> BTreeSet<(usize, usize)> {
    layout
        .zones
        .iter()
        .enumerate()
        .filter(|(_, z)| z.layer == layer)
        .flat_map(|(i, z)| {
            z.polygons
                .iter()
                .enumerate()
                .filter(move |(_, poly)| inside(p, poly))
                .map(move |(j, _)| (i, j))
        })
        .collect()
}

/// How a track is referenced: one piece under it on some neighbouring layer, separate pieces, or no copper.
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum Reference {
    Whole,
    Missing,
    Split,
}

fn reference(layout: &Layout, neighbours: &[&str], t: &Track) -> Reference {
    // Every quarter millimetre along the track, and its ends.
    let steps = (t.length() / 0.25).ceil().max(1.0) as usize;
    let points: Vec<Point> = (0..=steps)
        .map(|k| {
            let f = k as f64 / steps as f64;
            (
                t.start.0 + f * (t.end.0 - t.start.0),
                t.start.1 + f * (t.end.1 - t.start.1),
            )
        })
        .collect();
    let mut covered = false;
    for layer in neighbours {
        // Points over a hole (a via's or pad's clearance) have no copper and are skipped.
        let under: Vec<BTreeSet<(usize, usize)>> = points
            .iter()
            .map(|&p| pieces(layout, layer, p))
            .filter(|s| !s.is_empty())
            .collect();
        let Some(first) = under.first() else { continue };
        covered = true;
        let common = under.iter().skip(1).fold(first.clone(), |acc, s| &acc & s);
        if !common.is_empty() {
            return Reference::Whole;
        }
    }
    if covered { Reference::Split } else { Reference::Missing }
}

/// The `plane-split` findings: one per signal net with tracks.
pub fn check(design: &Design, layout: &Layout) -> Vec<Finding> {
    let layers: Vec<&str> = layout
        .stackup
        .iter()
        .filter_map(|l| match l {
            StackLayer::Copper { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let neighbours = |layer: &str| -> Vec<&str> {
        let Some(i) = layers.iter().position(|l| *l == layer) else {
            return vec![];
        };
        [i.checked_sub(1), Some(i + 1)]
            .into_iter()
            .flatten()
            .filter_map(|j| layers.get(j).copied())
            .collect()
    };
    let mut out = vec![];
    for (n, net) in design.nets.iter().enumerate() {
        if layout.zone_nets.contains(&net.name) || layout.zones.iter().any(|z| z.net == net.name) {
            continue;
        }
        let tracks: Vec<&Track> = layout.tracks.iter().filter(|t| t.net == net.name).collect();
        if tracks.is_empty() {
            continue;
        }
        let worst = tracks
            .iter()
            .map(|t| (reference(layout, &neighbours(&t.layer), t), *t))
            .max_by_key(|(r, _)| *r)
            .expect("the net has tracks");
        let at = |t: &Track| {
            format!(
                "the track on {} from ({:.2}, {:.2}) to ({:.2}, {:.2})",
                t.layer, t.start.0, t.start.1, t.end.0, t.end.1
            )
        };
        let (verdict, message) = match worst {
            (Reference::Whole, _) => (
                Verdict::Pass,
                format!(
                    "{}: every track has one unbroken plane beneath it on a neighbouring layer",
                    net.name
                ),
            ),
            (Reference::Split, t) => (
                Verdict::Fail,
                format!(
                    "{}: {} crosses a split: the copper under it is separate pieces on every neighbouring \
                     layer, so its return current has to go around the gap",
                    net.name,
                    at(t)
                ),
            ),
            (Reference::Missing, t) => (
                Verdict::Unknown,
                format!(
                    "{}: {} has no filled copper under it on any neighbouring layer, so where its return \
                     current flows isn't known",
                    net.name,
                    at(t)
                ),
            ),
        };
        out.push(Finding {
            rule: "plane-split",
            verdict,
            severity: Severity::Warning,
            reference: net.name.clone(),
            message,
            nets: vec![n],
            case: None,
            headline: None,
        });
    }
    out
}
