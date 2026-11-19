//! Hot loop length: does each switching regulator have an input capacitor close across its supply and ground pins?
//!
//! A switching regulator's input current turns on and off at every switch edge. It flows in a loop: from the
//! input capacitor into the supply pin, through the switches, out of the ground pin and back into the capacitor.
//! The loop's size sets how much the edges radiate and ring. For each part with a switch node (a `switches` drive
//! in its part file), the loop is walked through the pad centres of a supply pin, a capacitor's two pads and a ground
//! pin, and back to the supply pin. The loop can be no shorter than that walk, and it can't cancel out the way an
//! area can when the points cross over. Each supply pad takes its shortest loop over the capacitors between the
//! two nets and the ground pads; the part's longest of those is its result. Within `policy.toml`'s
//! `[hot_loop] length` passes; longer is a warning.
use std::collections::BTreeSet;

use crate::design::{Design, is_capacitor};
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, Point, dist};
use crate::limits::{DriveKind, Library};
use crate::policy::HotLoopPolicy;

/// The length round a closed path.
fn perimeter(points: &[Point]) -> f64 {
    (0..points.len())
        .map(|i| dist(points[i], points[(i + 1) % points.len()]))
        .sum()
}

/// The `hot-loop` findings: one per switching regulator on the board.
pub fn check(design: &Design, lib: &Library, layout: &Layout, policy: &HotLoopPolicy) -> Vec<Finding> {
    let mut out = vec![];
    for (r, c) in &design.components {
        let Some(part) = lib.part_for(c) else { continue };
        let drives: Vec<_> = part.drives.iter().filter(|d| d.kind == DriveKind::Switches).collect();
        if drives.is_empty() || c.dnp || !c.on_board {
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
        // The part's pads on its supply and ground pins: (pin number, net, centre).
        let pads_of = |names: &[&str]| -> Vec<(String, usize, Point)> {
            names
                .iter()
                .flat_map(|name| part.pins_named(name))
                .filter_map(|num| Some((num.to_string(), c.pins.get(num)?.net)))
                .flat_map(|(num, net)| {
                    layout
                        .pad_centres(r, &num)
                        .into_iter()
                        .map(move |p| (num.clone(), net, p))
                })
                .collect()
        };
        let Some(supplies) = drives
            .iter()
            .map(|d| d.supply.as_deref())
            .collect::<Option<Vec<&str>>>()
        else {
            out.push(finding(
                Verdict::Unknown,
                "its part file's switch node names no supply pin".into(),
            ));
            continue;
        };
        let grounds: Vec<&str> = drives.iter().map(|d| d.to.as_str()).collect();
        let (supply, ground) = (pads_of(&supplies), pads_of(&grounds));
        let gnd: BTreeSet<usize> = ground.iter().map(|g| g.1).collect();
        // Each supply pad's shortest loop: (length, supply pin, capacitor, ground pin), or None without one.
        let shortest = |(spin, snet, sp): &(String, usize, Point)| {
            let mut best: Option<(f64, String, &str, String)> = None;
            for (cr, cc) in &design.components {
                if cc.dnp || !is_capacitor(cr, cc) || cc.pins.len() != 2 {
                    continue;
                }
                let pins: Vec<(&String, usize)> = cc.pins.iter().map(|(n, p)| (n, p.net)).collect();
                for (a, b) in [(0, 1), (1, 0)] {
                    let ((na, neta), (nb, netb)) = (pins[a], pins[b]);
                    if neta != *snet || !gnd.contains(&netb) {
                        continue;
                    }
                    for ca in layout.pad_centres(cr, na) {
                        for cb in layout.pad_centres(cr, nb) {
                            for (gpin, _, gp) in ground.iter().filter(|g| g.1 == netb) {
                                let l = perimeter(&[*sp, ca, cb, *gp]);
                                if best.as_ref().is_none_or(|b| l < b.0) {
                                    best = Some((l, spin.clone(), cr.as_str(), gpin.clone()));
                                }
                            }
                        }
                    }
                }
            }
            best
        };
        let loops: Vec<_> = supply.iter().map(shortest).collect();
        if loops.is_empty() || loops.iter().any(Option::is_none) {
            out.push(finding(
                Verdict::Unknown,
                format!(
                    "no placed capacitor joins every {} pad to its {} pins' net, so its input loop can't be walked",
                    supplies.join("/"),
                    grounds.join("/")
                ),
            ));
            continue;
        }
        let (l, spin, cap, gpin) = loops
            .into_iter()
            .flatten()
            .max_by(|x, y| x.0.total_cmp(&y.0))
            .expect("checked above");
        let what = format!(
            "its longest input loop, from pin {spin} through {cap} to pin {gpin} and back, is {l:.1} mm round the \
             pad centres"
        );
        out.push(if l <= policy.length {
            finding(
                Verdict::Pass,
                format!("{what}; within {} mm ({})", policy.length, policy.reason),
            )
        } else {
            finding(
                Verdict::Fail,
                format!("{what}; more than {} mm ({})", policy.length, policy.reason),
            )
        });
    }
    out
}
