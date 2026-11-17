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
use crate::kicad::pcb::{Layout, StackLayer, Track};
use crate::rules::{Finding, Severity, Verdict};
use crate::thermal::Thermal;

/// The temperature rise a track may have, from `policy.toml`'s `[traces]`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePolicy {
    /// °C above the board.
    pub rise: f64,
    pub reason: String,
}

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

fn close(a: crate::kicad::pcb::Point, b: crate::kicad::pcb::Point, r: f64) -> bool {
    (a.0 - b.0).hypot(a.1 - b.1) <= r + 1e-6
}

/// Whether removing track `cut` (an index into `tracks`) separates pad 0 from pad 1.
fn separates(
    tracks: &[&Track],
    pads: &[(crate::kicad::pcb::Point, f64, Vec<String>)],
    vias: &[(crate::kicad::pcb::Point, f64)],
    cut: Option<usize>,
) -> bool {
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
        for end in [t.start, t.end] {
            for (j, u) in tracks.iter().enumerate() {
                if j != i
                    && Some(j) != cut
                    && u.layer == t.layer
                    && (close(end, u.start, 0.0) || close(end, u.end, 0.0))
                {
                    join(Node::Track(i), Node::Track(j));
                }
            }
            for (j, (at, r, layers)) in pads.iter().enumerate() {
                let on = layers.iter().any(|l| l == &t.layer || l == "*.Cu");
                if on && close(end, *at, *r) {
                    join(Node::Track(i), Node::Pad(j));
                }
            }
            for (j, (at, r)) in vias.iter().enumerate() {
                if close(end, *at, *r) {
                    join(Node::Track(i), Node::Via(j));
                }
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
        // Each track's safe current, narrowest first.
        let mut rated: Vec<(usize, f64)> = vec![];
        let mut unknown_layer = None;
        for (i, t) in tracks.iter().enumerate() {
            match cu.get(&t.layer) {
                Some(&(thick, outer)) => rated.push((i, ipc2221(t.width, thick, outer, policy.rise))),
                None => unknown_layer = Some(t.layer.clone()),
            }
        }
        rated.sort_by(|a, b| a.1.total_cmp(&b.1));
        let what = |i: usize, cap: f64| {
            let t = tracks[i];
            format!(
                "its narrowest track, {} mm on {} at ({:.2}, {:.2}), takes {:.2} A for a {} °C rise (IPC-2221, {})",
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
                .map(|p| (p.at, p.size.0.min(p.size.1) / 2.0, p.layers.clone()))
                .collect();
            let vias: Vec<_> = layout
                .vias
                .iter()
                .filter(|v| v.net == net.name)
                .map(|v| (v.at, v.diameter / 2.0))
                .collect();
            let pour = layout.zones.iter().any(|z| z.net == net.name);
            let sure = pads.len() == 2
                && !pour
                && !separates(&tracks, &pads, &vias, None)
                && separates(&tracks, &pads, &vias, Some(i));
            if sure {
                (
                    Verdict::Fail,
                    format!(
                        "{draw}, all of it through every track between its two pads; {}",
                        what(i, cap)
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
