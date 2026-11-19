//! Trace current: is every track wide enough for the current its net carries?
//!
//! A track's safe current comes from the IPC-2221 fit, `I = k · ΔT^0.44 · A^0.725` (A in mil², k = 0.048 on an
//! outer layer and 0.024 on an inner one), for the temperature rise `policy.toml` allows and the copper thickness
//! in the board's stackup. IPC-2221's curves are the conservative ones IPC-2152 later replaced; they suit a check
//! that must not pass a track that could overheat.
//!
//! The current is what `loads` works out the net supplies: everything its parts draw. No track carries more, so a
//! net whose narrowest track takes that current passes. A narrower track fails only when it is sure to carry all
//! of it: the net joins exactly two pads, has no copper pour, and the track is on every path between them.
//! Otherwise the current through that track isn't known (it may feed only one branch, or a pour may carry it
//! alongside), and the result is UNKNOWN. A net whose current isn't fully known is UNKNOWN too.
use std::collections::BTreeMap;

use crate::design::Design;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, PlacedPad, StackLayer, Track};
use crate::policy::TracePolicy;
use crate::thermal::Thermal;

/// Copper thickness of each copper layer, mm, and whether it is an outer layer.
fn copper(layout: &Layout) -> BTreeMap<String, (f64, bool)> {
    let names: Vec<(&str, f64)> = layout
        .stackup
        .iter()
        .filter_map(|l| match l {
            StackLayer::Copper { name, thickness } => Some((name.as_str(), *thickness)),
            _ => None,
        })
        .collect();
    let last = names.len().saturating_sub(1);
    names
        .iter()
        .enumerate()
        .map(|(i, &(n, t))| (n.to_string(), (t, i == 0 || i == last)))
        .collect()
}

/// IPC-2221's safe current, amps, for a track `width` mm wide and `thickness` mm thick.
pub fn ipc2221(width: f64, thickness: f64, outer: bool, rise: f64) -> f64 {
    let mil = 1.0 / 0.0254;
    let area = width * mil * thickness * mil;
    let k = if outer { 0.048 } else { 0.024 };
    k * rise.powf(0.44) * area.powf(0.725)
}

/// Contacts between tracks, pads and vias.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Node {
    Track(usize),
    Pad(usize),
    Via(usize),
}

type Pt = crate::kicad::pcb::Point;

/// Distance from `p` to the segment `a`–`b`.
fn to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

/// Whether two segments come within `r` of each other (touching, crossing or overlapping copper).
fn segments_touch(a: &Track, b: &Track, r: f64) -> bool {
    let cross = |p: Pt, q: Pt, x: Pt| (q.0 - p.0) * (x.1 - p.1) - (q.1 - p.1) * (x.0 - p.0);
    let (d1, d2) = (cross(a.start, a.end, b.start), cross(a.start, a.end, b.end));
    let (d3, d4) = (cross(b.start, b.end, a.start), cross(b.start, b.end, a.end));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return true;
    }
    [
        to_segment(a.start, b.start, b.end),
        to_segment(a.end, b.start, b.end),
        to_segment(b.start, a.start, a.end),
        to_segment(b.end, a.start, a.end),
    ]
    .into_iter()
    .any(|d| d <= r + 1e-6)
}

/// Whether removing track `cut` (an index into `tracks`) separates pad 0 from pad 1. Contacts are taken
/// generously (any copper overlap), so a track is only ever found to separate them when it surely does.
fn separates(tracks: &[&Track], pads: &[&PlacedPad], vias: &[(Pt, f64)], cut: Option<usize>) -> bool {
    let nodes: Vec<Node> = (0..tracks.len())
        .map(Node::Track)
        .chain((0..pads.len()).map(Node::Pad))
        .chain((0..vias.len()).map(Node::Via))
        .collect();
    let index: BTreeMap<Node, usize> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    fn root(p: &mut [usize], mut n: usize) -> usize {
        while p[n] != n {
            p[n] = p[p[n]];
            n = p[n];
        }
        n
    }
    let mut join = |a: Node, b: Node| {
        let (x, y) = (root(&mut parent, index[&a]), root(&mut parent, index[&b]));
        parent[x] = y;
    };
    for (i, t) in tracks.iter().enumerate() {
        if Some(i) == cut {
            continue;
        }
        let half = t.width / 2.0;
        for (j, u) in tracks.iter().enumerate() {
            if j > i && Some(j) != cut && u.layer == t.layer && segments_touch(t, u, half + u.width / 2.0) {
                join(Node::Track(i), Node::Track(j));
            }
        }
        for (j, p) in pads.iter().enumerate() {
            let on = p.layers.iter().any(|l| l == &t.layer || l == "*.Cu");
            if on && (p.contains(t.start, half) || p.contains(t.end, half)) {
                join(Node::Track(i), Node::Pad(j));
            }
        }
        for (j, &(at, r)) in vias.iter().enumerate() {
            if to_segment(at, t.start, t.end) <= r + half + 1e-6 {
                join(Node::Track(i), Node::Via(j));
            }
        }
    }
    let (a, b) = (
        root(&mut parent, index[&Node::Pad(0)]),
        root(&mut parent, index[&Node::Pad(1)]),
    );
    a != b
}

/// The `trace-current` findings, one per net with tracks and a current.
pub fn check(design: &Design, layout: &Layout, th: &Thermal, policy: &TracePolicy) -> Vec<Finding> {
    let mut out = vec![];
    let cu = copper(layout);
    for (n, net) in design.nets.iter().enumerate() {
        let load = &th.loads[n];
        if load.parts.is_empty() && load.missing.is_empty() {
            continue;
        }
        let tracks: Vec<&Track> = layout.tracks.iter().filter(|t| t.net == net.name).collect();
        if tracks.is_empty() {
            continue;
        }
        let amps = load.amps;
        // Each track's safe current, narrowest first. With no stackup in the board file the copper could be
        // anything from ½ oz to 2 oz: a pass takes the thinnest, a fail the thickest.
        let (thin, thick_cu) = if layout.stackup_assumed {
            (Some(0.0175), Some(0.070))
        } else {
            (None, None)
        };
        let mut rated: Vec<(usize, f64)> = vec![];
        let mut rated_thick: BTreeMap<usize, f64> = BTreeMap::new();
        let mut unknown_layer = None;
        for (i, t) in tracks.iter().enumerate() {
            match cu.get(&t.layer) {
                Some(&(thick, outer)) => {
                    rated.push((i, ipc2221(t.width, thin.unwrap_or(thick), outer, policy.rise)));
                    rated_thick.insert(i, ipc2221(t.width, thick_cu.unwrap_or(thick), outer, policy.rise));
                }
                None => unknown_layer = Some(t.layer.clone()),
            }
        }
        rated.sort_by(|a, b| a.1.total_cmp(&b.1));
        let copper_note = if layout.stackup_assumed {
            "; the board file has no stackup, so this takes ½ oz copper to pass and 2 oz to fail"
        } else {
            ""
        };
        let what = |i: usize, cap: f64| {
            let t = tracks[i];
            format!(
                "the track {} mm wide on {} at ({:.2}, {:.2}) takes {:.2} A for a {} °C rise (IPC-2221, {}){copper_note}",
                t.width, t.layer, t.start.0, t.start.1, cap, policy.rise, policy.reason
            )
        };
        let draw = if load.known() {
            format!("{} supplies at most {:.3} A", net.name, amps)
        } else {
            format!(
                "{} supplies at least {:.3} A, and more from {} part(s) whose current isn't known",
                net.name,
                amps,
                load.missing.len()
            )
        };
        let (verdict, message) = if let Some(l) = unknown_layer {
            (
                Verdict::Unknown,
                format!("{draw}, but layer {l} isn't in the board's stackup, so its copper thickness isn't known"),
            )
        } else if let Some(&(i, cap)) = rated.first()
            && cap >= amps
        {
            if load.known() {
                (Verdict::Pass, format!("{draw}; {}", what(i, cap)))
            } else {
                (Verdict::Unknown, format!("{draw}; {}", what(i, cap)))
            }
        } else {
            let &(i, cap) = rated.first().expect("tracks have layers");
            // Sure to carry all of it: two pads, no pour, and this track on the only path between them.
            let pads: Vec<_> = layout
                .footprints
                .iter()
                .flat_map(|f| f.pads.iter())
                .filter(|p| p.net.as_deref() == Some(net.name.as_str()))
                .collect();
            let vias: Vec<_> = layout
                .vias
                .iter()
                .filter(|v| v.net == net.name)
                .map(|v| (v.at, v.diameter / 2.0))
                .collect();
            let pour = layout.zones.iter().any(|z| z.net == net.name) || layout.zone_nets.contains(&net.name);
            // A track that can't take it even at the thickest copper, and is sure to carry all of it.
            let sure = (pads.len() == 2 && !pour && !separates(&tracks, &pads, &vias, None))
                .then(|| {
                    rated
                        .iter()
                        .map(|&(j, _)| (j, rated_thick[&j]))
                        .filter(|&(_, c)| c < amps)
                        .find(|&(j, _)| separates(&tracks, &pads, &vias, Some(j)))
                })
                .flatten();
            if let Some((j, c)) = sure {
                (
                    Verdict::Fail,
                    format!(
                        "{draw}, all of it through every track between its two pads; {}",
                        what(j, c)
                    ),
                )
            } else {
                (
                    Verdict::Unknown,
                    format!(
                        "{draw}; {}, which is less, but how much of the net's current goes through it isn't \
                         known ({})",
                        what(i, cap),
                        if pour {
                            "a copper pour on the net may carry part of it"
                        } else {
                            "it may feed only one branch"
                        }
                    ),
                )
            }
        };
        out.push(Finding {
            rule: "trace-current",
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
