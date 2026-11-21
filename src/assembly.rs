//! Assembly checks on the board: what makes parts solder badly or place inaccurately.
//!
//! - `thermal-relief`: a through-hole pad joined to a pour with copper all round it (no thermal relief). The
//!   plane draws the soldering heat away, so the solder may not wet the hole.
//! - `tombstoning`: a small two-pad part (0603 or smaller) with one pad joined solidly to a pour and the other not.
//!   The pour's pad heats more slowly in reflow, so the other end's solder melts first and can stand the part up.
//! - `fiducials`: a side carrying a fine-pitch part (pads `[assembly] fine_pitch` apart or closer) needs
//!   `[assembly] fiducials` global fiducials for the placement machine to find the board accurately.
use crate::design::Design;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, PadConnection, PlacedFootprint, PlacedPad, dist, inside};
use crate::policy::AssemblyPolicy;

/// How the pours join a pad: the most solid joint of any pour of its net that covers it, with the pour's net and
/// layer. `None` when no pour of its net covers it.
fn joint<'a>(layout: &'a Layout, pad: &PlacedPad) -> Option<(PadConnection, &'a str, &'a str)> {
    let net = pad.net.as_deref()?;
    layout
        .pours
        .iter()
        .filter(|p| p.net == net && pad.layers.contains(&p.layer) && inside(pad.at, &p.outline))
        .map(|p| {
            let c = pad.zone_connect.unwrap_or(p.pads).for_pad(pad.through_hole);
            (c, p.net.as_str(), p.layer.as_str())
        })
        .max_by_key(|(c, ..)| matches!(c, PadConnection::Solid))
}

/// A two-pad surface-mount package of 0603 or smaller, by the size code in its footprint name
/// (`Resistor_SMD:R_0402_1005Metric`).
fn small_chip(f: &PlacedFootprint) -> bool {
    let name = f.lib_id.rsplit(':').next().unwrap_or_default();
    f.pads.len() == 2
        && f.pads.iter().all(|p| !p.through_hole)
        && name.split('_').any(|w| ["01005", "0201", "0402", "0603"].contains(&w))
}

/// A fiducial: from KiCad's `Fiducial` library, or a reference starting `FID`.
fn is_fiducial(f: &PlacedFootprint) -> bool {
    f.lib_id.starts_with("Fiducial:") || f.reference.starts_with("FID")
}

/// The closest two surface-mount pads of a part with at least six, centre to centre: its pin pitch.
fn pitch(f: &PlacedFootprint) -> Option<f64> {
    let smd: Vec<&PlacedPad> = f.pads.iter().filter(|p| !p.through_hole).collect();
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
        for pad in f.pads.iter().filter(|p| p.through_hole) {
            let Some((c, net, layer)) = joint(layout, pad) else {
                continue;
            };
            let at = format!("{}.{} ({net})", f.reference, pad.number);
            let nets = net_index(net).into_iter().collect();
            out.push(if c == PadConnection::Solid {
                finding(
                    "thermal-relief",
                    Verdict::Fail,
                    &f.reference,
                    format!(
                        "{at} joins the {net} pour on {layer} without thermal relief: the plane draws the soldering \
                         heat away, so the solder may not fill the hole"
                    ),
                    nets,
                )
            } else {
                finding(
                    "thermal-relief",
                    Verdict::Pass,
                    &f.reference,
                    format!("{at} joins the {net} pour on {layer} through a thermal relief"),
                    nets,
                )
            });
        }
        if small_chip(f) {
            let solid: Vec<Option<(&str, &str)>> = f
                .pads
                .iter()
                .map(|p| match joint(layout, p) {
                    Some((PadConnection::Solid, net, layer)) => Some((net, layer)),
                    _ => None,
                })
                .collect();
            let nets = f.pads.iter().filter_map(|p| net_index(p.net.as_deref()?)).collect();
            out.push(match (&f.pads[..], &solid[..]) {
                ([p, q], [Some((net, layer)), None]) | ([q, p], [None, Some((net, layer))]) => finding(
                    "tombstoning",
                    Verdict::Fail,
                    &f.reference,
                    format!(
                        "{}: pad {} joins the {net} pour on {layer} solidly and pad {} doesn't, so pad {} heats more \
                         slowly in reflow and the part can stand up on it",
                        f.reference, p.number, q.number, p.number
                    ),
                    nets,
                ),
                _ => finding(
                    "tombstoning",
                    Verdict::Pass,
                    &f.reference,
                    format!("{}: both pads are joined alike", f.reference),
                    nets,
                ),
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
