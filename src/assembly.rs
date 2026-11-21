//! Assembly checks on the board: what makes parts solder badly or place inaccurately.
//!
//! - `thermal-relief`: a through-hole pad joined to a pour with copper all round it (no thermal relief). The
//!   plane draws the soldering heat away, so the solder may not wet the hole.
//! - `tombstoning`: a small two-pad part (0603 or smaller) with one pad joined solidly to a pour and the other not.
//!   The pour's pad heats more slowly in reflow, so the other end's solder melts first and can stand the part up.
//! - `fiducials`: a side carrying a fine-pitch part (pads `[assembly] fine_pitch` apart or closer) needs
//!   `[assembly] fiducials` global fiducials for the placement machine to find the board accurately.
//!
//! How a pour joins a pad is read from KiCad's last fill, not from the zone's settings, so DRC-rule overrides,
//! keepouts and higher-priority zones count: the fill is sampled just outside the pad (see [`around`]).
use crate::design::Design;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, PlacedFootprint, PlacedPad, Point, dist, inside};
use crate::policy::AssemblyPolicy;

/// Points 0.05 mm outside the pad: a ring all round it (its circle, or its rectangle for any other shape), and
/// four probes where a thermal relief always leaves a gap (the rectangle's corners, or the circle at 0°, 90°, 180°
/// and 270°: KiCad runs spokes straight out of a rectangle's sides and at 45° from a circle).
fn around(pad: &PlacedPad) -> (Vec<Point>, Vec<Point>) {
    const N: usize = 32;
    const GAP: f64 = 0.05;
    let (w, h) = (pad.size.0 / 2.0 + GAP, pad.size.1 / 2.0 + GAP);
    let (c, s) = (pad.rotation.to_radians().cos(), pad.rotation.to_radians().sin());
    // From the pad's frame to the board's (the inverse of `PlacedPad::contains`).
    let board = |(lx, ly): (f64, f64)| (pad.at.0 + lx * c + ly * s, pad.at.1 - lx * s + ly * c);
    let ring = (0..N)
        .map(|i| {
            let t = i as f64 / N as f64;
            if pad.round {
                let a = t * std::f64::consts::TAU;
                (w * a.cos(), w * a.sin())
            } else {
                // Walk the rectangle's perimeter.
                let d = t * 4.0 * (w + h);
                match d {
                    d if d < 2.0 * w => (-w + d, -h),
                    d if d < 2.0 * w + 2.0 * h => (w, -h + d - 2.0 * w),
                    d if d < 4.0 * w + 2.0 * h => (w - (d - 2.0 * w - 2.0 * h), h),
                    d => (-w, h - (d - 4.0 * w - 2.0 * h)),
                }
            }
        })
        .map(board)
        .collect();
    let probes = if pad.round {
        [(w, 0.0), (0.0, w), (-w, 0.0), (0.0, -w)]
    } else {
        [(w, h), (-w, h), (-w, -h), (w, -h)]
    };
    (ring, probes.into_iter().map(board).collect())
}

/// How the pours join a pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Joint {
    None,
    Relief,
    Solid,
}

/// The most solid joint any pour of the pad's net makes with it, and on which layer.
fn joint<'a>(layout: &'a Layout, pad: &PlacedPad) -> (Joint, &'a str) {
    let Some(net) = pad.net.as_deref() else {
        return (Joint::None, "");
    };
    let (ring, probes) = around(pad);
    layout
        .zones
        .iter()
        .filter(|z| z.net == net && pad.layers.contains(&z.layer))
        .map(|z| {
            let filled = |p: &Point| z.polygons.iter().any(|poly| inside(*p, poly));
            // Copper at a probe means no relief gap there. A solid pad hemmed in by another net's copper on one
            // side still has copper at the other probes.
            let j = if probes.iter().any(filled) {
                Joint::Solid
            } else if ring.iter().any(filled) {
                Joint::Relief
            } else {
                Joint::None
            };
            (j, z.layer.as_str())
        })
        .max_by_key(|(j, _)| *j)
        .unwrap_or((Joint::None, ""))
}

/// A two-pad surface-mount package of 0603 or smaller, by the size code in its footprint name
/// (`Resistor_SMD:R_0402_1005Metric`).
fn small_chip(f: &PlacedFootprint) -> bool {
    let name = f.lib_id.rsplit(':').next().unwrap_or_default();
    f.pads.len() == 2
        && f.pads.iter().all(|p| p.kind == "smd")
        && name.split('_').any(|w| ["01005", "0201", "0402", "0603"].contains(&w))
}

/// A fiducial: from KiCad's `Fiducial` library, or a reference starting `FID`.
fn is_fiducial(f: &PlacedFootprint) -> bool {
    f.lib_id.starts_with("Fiducial:") || f.reference.starts_with("FID")
}

/// The closest two surface-mount pads of a part with at least six, centre to centre: its pin pitch.
fn pitch(f: &PlacedFootprint) -> Option<f64> {
    let smd: Vec<&PlacedPad> = f.pads.iter().filter(|p| p.kind == "smd").collect();
    if smd.len() < 6 {
        return None;
    }
    smd.iter()
        .enumerate()
        .flat_map(|(i, a)| smd[i + 1..].iter().map(move |b| dist(a.at, b.at)))
        .filter(|&d| d > 0.0)
        .min_by(f64::total_cmp)
}

/// The `thermal-relief`, `tombstoning` and `fiducials` findings.
pub fn check(design: &Design, layout: &Layout, policy: &AssemblyPolicy) -> Vec<Finding> {
    let net_index = |name: &str| design.nets.iter().position(|n| n.name == name);
    let fitted = |f: &PlacedFootprint| design.components.get(&f.reference).is_some_and(|c| !c.dnp);
    let finding = |rule, verdict, reference: &str, message: String, nets: Vec<usize>| Finding {
        rule,
        verdict,
        severity: Severity::Warning,
        reference: reference.to_string(),
        message,
        nets,
        case: None,
        headline: None,
    };
    let mut out = vec![];
    for f in layout.footprints.iter().filter(|f| fitted(f)) {
        // A through-hole pad by its number: a part may repeat one (a connector's shield tabs), reported once.
        let mut numbers: Vec<&str> = f
            .pads
            .iter()
            .filter(|p| p.kind == "thru_hole")
            .map(|p| p.number.as_str())
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        for number in numbers {
            let (j, layer, net) = f
                .pads
                .iter()
                .filter(|p| p.kind == "thru_hole" && p.number == number)
                .map(|p| {
                    let (j, layer) = joint(layout, p);
                    (j, layer, p.net.as_deref().unwrap_or_default())
                })
                .max_by_key(|(j, ..)| *j)
                .unwrap_or((Joint::None, "", ""));
            let at = format!("{}.{number} ({net})", f.reference);
            let nets = net_index(net).into_iter().collect();
            match j {
                Joint::None => {}
                Joint::Solid => out.push(finding(
                    "thermal-relief",
                    Verdict::Fail,
                    &f.reference,
                    format!(
                        "{at} joins the {net} pour on {layer} without thermal relief: the plane draws the soldering \
                         heat away, so the solder may not fill the hole"
                    ),
                    nets,
                )),
                Joint::Relief => out.push(finding(
                    "thermal-relief",
                    Verdict::Pass,
                    &f.reference,
                    format!("{at} joins the {net} pour on {layer} through a thermal relief"),
                    nets,
                )),
            }
        }
        if small_chip(f) {
            let joints: Vec<(Joint, &str)> = f.pads.iter().map(|p| joint(layout, p)).collect();
            let nets = f.pads.iter().filter_map(|p| net_index(p.net.as_deref()?)).collect();
            let solid = |i: usize| joints[i].0 == Joint::Solid;
            out.push(if solid(0) == solid(1) {
                finding(
                    "tombstoning",
                    Verdict::Pass,
                    &f.reference,
                    format!("{}: both pads are joined alike", f.reference),
                    nets,
                )
            } else {
                let (s, o) = if solid(0) { (0, 1) } else { (1, 0) };
                let (p, q) = (&f.pads[s], &f.pads[o]);
                let net = p.net.as_deref().unwrap_or_default();
                finding(
                    "tombstoning",
                    Verdict::Fail,
                    &f.reference,
                    format!(
                        "{}: pad {} joins the {net} pour on {} solidly and pad {} doesn't, so pad {} heats more \
                         slowly in reflow and the part can stand up on it",
                        f.reference, p.number, joints[s].1, q.number, p.number
                    ),
                    nets,
                )
            });
        }
    }
    for back in [false, true] {
        let side = if back { "bottom" } else { "top" };
        let finest = layout
            .footprints
            .iter()
            .filter(|f| f.back == back && fitted(f))
            .filter_map(|f| Some((pitch(f)?, f)))
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((p, f)) = finest.filter(|(p, _)| *p <= policy.fine_pitch + 1e-6) else {
            continue;
        };
        let count = layout
            .footprints
            .iter()
            .filter(|g| g.back == back && is_fiducial(g))
            .count();
        let what = format!(
            "the {side} has {count} fiducial{} and carries {} ({}) at a {p:.2} mm pitch",
            if count == 1 { "" } else { "s" },
            f.reference,
            f.value
        );
        out.push(if count >= policy.fiducials {
            finding("fiducials", Verdict::Pass, &f.reference, what, vec![])
        } else {
            finding(
                "fiducials",
                Verdict::Fail,
                &f.reference,
                format!("{what}: fine-pitch parts need {} ({})", policy.fiducials, policy.reason),
                vec![],
            )
        });
    }
    out
}
