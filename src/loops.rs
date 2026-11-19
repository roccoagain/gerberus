//! Hot loop area: does each switching regulator have an input capacitor close across its supply and ground pins?
//!
//! A switching regulator's input current turns on and off at every switch edge. It flows in a loop: from the
//! input capacitor into the supply pin, through the switches, out of the ground pin and back into the capacitor.
//! That loop's area sets how much the edges radiate and ring. For each part with a switch node (a `switches` drive
//! in its part file), the loop is drawn through the pad centres of its supply pin, the capacitor's two pads and its
//! ground pin, and its area is the smallest over the capacitors between those two nets and the part's pads on
//! them. The tracks between the pads can only make the real loop larger. Within `policy.toml`'s `[hot_loop] area`
//! passes; larger is a warning.
use std::collections::BTreeSet;

use crate::design::{Design, is_capacitor};
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, Point};
use crate::limits::{DriveKind, Library};
use crate::policy::HotLoopPolicy;

fn pad_at(layout: &Layout, reference: &str, number: &str) -> Option<Point> {
    layout
        .footprints
        .iter()
        .find(|f| f.reference == reference)?
        .pads
        .iter()
        .find(|p| p.number == number)
        .map(|p| p.at)
}

/// The area enclosed by a polygon, by the shoelace formula.
fn area(points: &[Point]) -> f64 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

/// The `hot-loop` findings: one per switching regulator on the board.
pub fn check(design: &Design, lib: &Library, layout: &Layout, policy: &HotLoopPolicy) -> Vec<Finding> {
    let mut out = vec![];
    for (r, c) in &design.components {
        let Some(part) = lib.part_for(c) else { continue };
        let Some(drive) = part.drives.iter().find(|d| d.kind == DriveKind::Switches) else {
            continue;
        };
        if c.dnp || !c.on_board {
            continue;
        }
        let finding = |verdict, message: String| Finding {
            rule: "hot-loop",
            verdict,
            severity: Severity::Warning,
            reference: r.clone(),
            message: format!("{r} ({}): {message}", part.mpn),
            nets: vec![],
            case: None,
            headline: None,
        };
        // The part's pads on its supply and ground pins, by net.
        let pads_of = |name: &str| -> Vec<(usize, Point)> {
            part.pins_named(name)
                .into_iter()
                .filter_map(|num| Some((c.pins.get(num)?.net, pad_at(layout, r, num)?)))
                .collect()
        };
        let Some(supply_pin) = drive.supply.as_deref() else {
            out.push(finding(
                Verdict::Unknown,
                "its part file's switch node names no supply pin".into(),
            ));
            continue;
        };
        let (supply, ground) = (pads_of(supply_pin), pads_of(&drive.to));
        let nets_of = |pads: &[(usize, Point)]| pads.iter().map(|p| p.0).collect::<BTreeSet<_>>();
        let (vin, gnd) = (nets_of(&supply), nets_of(&ground));
        // Every loop through an input capacitor: (area, capacitor).
        let mut loops: Vec<(f64, &str)> = vec![];
        for (cr, cc) in &design.components {
            if cc.dnp || !is_capacitor(cr, cc) || cc.pins.len() != 2 {
                continue;
            }
            let pins: Vec<(&String, usize)> = cc.pins.iter().map(|(n, p)| (n, p.net)).collect();
            for (a, b) in [(0, 1), (1, 0)] {
                let ((na, neta), (nb, netb)) = (pins[a], pins[b]);
                if !vin.contains(&neta) || !gnd.contains(&netb) {
                    continue;
                }
                let (Some(ca), Some(cb)) = (pad_at(layout, cr, na), pad_at(layout, cr, nb)) else {
                    continue;
                };
                for &(_, sp) in supply.iter().filter(|s| s.0 == neta) {
                    for &(_, gp) in ground.iter().filter(|g| g.0 == netb) {
                        loops.push((area(&[sp, ca, cb, gp]), cr.as_str()));
                    }
                }
            }
        }
        let Some(&(a, cap)) = loops.iter().min_by(|x, y| x.0.total_cmp(&y.0)) else {
            out.push(finding(
                Verdict::Unknown,
                format!(
                    "no placed capacitor joins its {supply_pin} and {} pins' nets, so its input loop can't be drawn",
                    drive.to
                ),
            ));
            continue;
        };
        let what = format!(
            "its input loop through {cap} encloses {a:.1} mm² between the pad centres of {supply_pin}, {cap} and {}",
            drive.to
        );
        out.push(if a <= policy.area {
            finding(
                Verdict::Pass,
                format!("{what}; within {} mm² ({})", policy.area, policy.reason),
            )
        } else {
            finding(
                Verdict::Fail,
                format!("{what}; more than {} mm² ({})", policy.area, policy.reason),
            )
        });
    }
    out
}
