//! Signal integrity for USB 2.0 high-speed pairs: layout rules read straight off the board, and a channel
//! simulation that turns the layout into an eye diagram checked against the USB eye mask.
//!
//! Each pair is traced from its PHY's pads to its connector's. The layout rules then check impedance (from a
//! 2D field solver run on each cross-section of the routing), skew, vias, stubs, reference planes, length and
//! the capacitance of parts on the lines. The simulation builds a transmission-line model of each line from
//! the same sections, vias, stubs and part capacitance, drives it with a 480 Mb/s PRBS7 pattern at the
//! fastest edges USB allows, and checks the differential eye at the connector against Template 1.
pub mod channel;
pub mod eye;
pub mod geometry;
pub mod route;
pub mod svg;
pub mod xsection;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::Serialize;

use self::channel::{Element, Ends, Line};
use self::eye::{Eye, USB2_TEMPLATE_1};
use self::geometry::{Analyzer, Section};
use self::route::{Route, Step};
use crate::diag::{Diagnostic, Level, Show};
use crate::finding::{Severity, Verdict};
use crate::kicad::pcb::{Layout, PlacedFootprint};
use crate::limits::Library;

/// Where each USB 2.0 limit comes from.
mod cite {
    pub const SPRAAR7: &str = "TI SPRAAR7J High-Speed Interface Layout Guidelines, Table A-1, PDF page 16";
    pub const TUSB1210: &str = "TI TUSB1210 datasheet SLLSE09J, Table 10-1, PDF pages 60-61";
    pub const USB_T1: &str = "USB 2.0 specification, Figure 7-13 (Template 1), PDF page 161";
    pub const USB_DRIVER: &str = "USB 2.0 specification, Table 7-8, PDF page 208";
}

/// USB 2.0 high-speed limits.
mod usb2 {
    /// Differential impedance, Ω (SPRAAR7: 90 Ω ± 10 %).
    pub const Z_MIN: f64 = 81.0;
    pub const Z_MAX: f64 = 99.0;
    /// Length mismatch within the pair, mm (SPRAAR7: 50 mil).
    pub const SKEW: f64 = 1.27;
    /// Vias on each line (SPRAAR7: 4).
    pub const VIAS: usize = 4;
    /// Stub length, mm (TUSB1210 item 2.13: less than 200 mil).
    pub const STUB: f64 = 5.08;
    /// Total length, mm (SPRAAR7: 12000 mil).
    pub const LENGTH: f64 = 304.8;
    /// Distance from a plane split, mm (TUSB1210 item 2.15: 25 mil).
    pub const PLANE_EDGE: f64 = 0.635;
    /// External capacitance on each line, pF (TUSB1210 item 1.06).
    pub const CAPACITANCE: f64 = 4.0;
    /// Unit interval at 480 Mb/s, s.
    pub const UI: f64 = 1.0 / 480e6;
    /// Fastest 10–90 % edge a driver may have (Table 7-8 THSR/THSF minimum), s.
    pub const RISE: f64 = 500e-12;
    /// Driver output resistance, which also terminates the line (Table 7-8 ZHSDRV, 40.5–49.5 Ω), Ω.
    pub const TERMINATION: f64 = 45.0;
    /// Nominal high-speed swing (VHSOH 400 mV), V.
    pub const SWING: f64 = 0.4;
    /// An impedance excursion shorter than this is left to the simulation rather than failed on its own,
    /// mm: about 20 ps on FR4, a twenty-fifth of the fastest USB edge, too short to reflect on its own.
    /// Pairs always open up for a moment at pads and package escapes.
    pub const ESCAPE: f64 = 3.0;
}

pub const RULES: [(&str, &str); 8] = [
    ("usb-impedance", "each stretch of the pair is 90 Ω ± 10 % differential"),
    ("usb-skew", "D+ and D− are within 50 mil of each other's length"),
    (
        "usb-vias",
        "each line has at most four vias, and both lines have the same number",
    ),
    ("usb-stubs", "nothing hangs off a line for 200 mil or more"),
    (
        "usb-reference",
        "every stretch runs over an unbroken plane on the next layer, clear of its edge",
    ),
    ("usb-length", "each line is at most 12 inches long"),
    ("usb-capacitance", "parts on each line add at most 4 pF"),
    (
        "usb-eye",
        "the simulated eye at the connector stays out of the USB 2.0 Template 1 mask",
    ),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub verdict: Verdict,
    pub severity: Severity,
    /// The pair's name, such as `USB1`.
    pub pair: String,
    pub message: String,
}

/// A pair's routing and simulation results.
#[derive(Debug, Clone, Serialize)]
pub struct PairReport {
    pub name: String,
    pub p: String,
    pub n: String,
    /// `REF.PAD` at each end.
    pub from: String,
    pub to: String,
    pub length_p: f64,
    pub length_n: f64,
    pub vias_p: usize,
    pub vias_n: usize,
    #[serde(skip)]
    pub sections: Vec<Section>,
    #[serde(skip)]
    pub eye: Option<Eye>,
    /// Differential impedance seen from the driver, `(seconds, ohms)`, for D+ and D−.
    #[serde(skip)]
    pub tdr: Vec<Vec<(f64, f64)>>,
    pub eye_height_mv: Option<f64>,
    pub eye_width_ui: Option<f64>,
    pub mask_margin_mv: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub pairs: Vec<PairReport>,
    pub findings: Vec<Finding>,
    /// Things the board file didn't say, filled with defaults.
    pub assumptions: Vec<String>,
    /// Differential pairs found that aren't USB, which no rules cover yet.
    pub skipped: Vec<String>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.verdict == Verdict::Fail && f.severity == Severity::Error)
            .count()
    }
}

/// A pair of nets: the name they share and their positive and negative members.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PairNets {
    pub name: String,
    pub p: String,
    pub n: String,
}

/// Pairs of nets whose names differ only in a `D+`/`D−`, `DP`/`DM`, `_P`/`_N` or `+`/`−` ending.
pub fn find_pairs(layout: &Layout) -> Vec<PairNets> {
    let nets: BTreeSet<&str> = layout
        .tracks
        .iter()
        .map(|t| t.net.as_str())
        .chain(
            layout
                .footprints
                .iter()
                .flat_map(|f| f.pads.iter().filter_map(|p| p.net.as_deref())),
        )
        .collect();
    let mut out = Vec::new();
    for &net in &nets {
        for (pos, neg) in [("D+", "D-"), ("DP", "DM"), ("_P", "_N"), ("+", "-")] {
            let upper = net.to_ascii_uppercase();
            if let Some(stem) = upper.strip_suffix(pos) {
                let partner = format!(
                    "{}{}",
                    &net[..stem.len()],
                    net[stem.len()..]
                        .replace(pos, neg)
                        .replace(&pos.to_ascii_lowercase(), &neg.to_ascii_lowercase())
                );
                let partner = nets.iter().find(|n| n.eq_ignore_ascii_case(&partner));
                if let Some(partner) = partner {
                    let name = net[..stem.len()]
                        .trim_end_matches(['_', '/', '-', '.'])
                        .trim_start_matches('/')
                        .to_string();
                    out.push(PairNets {
                        name,
                        p: net.to_string(),
                        n: (*partner).to_string(),
                    });
                    break;
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Whether a pair is USB 2.0 data: a `D+`/`DP` ending, or `USB` in a `_P`/`+` pair's name that doesn't mark it
/// as a SuperSpeed lane (`SS`, `TX`, `RX`), which runs at other speeds under other limits.
fn is_usb(p: &PairNets) -> bool {
    let up = p.p.to_ascii_uppercase();
    if up.ends_with("D+") || up.ends_with("DP") {
        return true;
    }
    let name = p.name.to_ascii_uppercase();
    name.contains("USB") && !["SS", "TX", "RX"].iter().any(|w| name.contains(w))
}

/// Whether a footprint is a connector: a `J` or `P` reference, or a connector library footprint.
fn is_connector(f: &PlacedFootprint) -> bool {
    let r = f.reference.as_str();
    let numbered = |prefix: &str| {
        r.strip_prefix(prefix)
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
    };
    numbered("J") || numbered("P") || f.lib_id.starts_with("Connector")
}

/// The capacitance pads add to a line.
struct PadLoad {
    pf: f64,
    /// Parts with no capacitance figure.
    unknown: Vec<String>,
    /// The figures used, for the report.
    used: Vec<String>,
}

/// Pad capacitance on a line from the part library: per part, the capacitance its pin adds.
struct Loads<'a> {
    layout: &'a Layout,
    lib: &'a Library,
}

impl Loads<'_> {
    /// Capacitance in pF of the pads in `pads` (each part counted once per pin name), with the parts that
    /// have no figure and the figures used. The pair's own driver and connector are left out: their capacitance is part of the
    /// USB budget the templates already cover.
    fn of(&self, pads: &[(String, String)], ends: &[&str]) -> PadLoad {
        let mut seen = BTreeSet::new();
        let (mut total, mut unknown, mut used) = (0.0, Vec::new(), Vec::new());
        for (r, num) in pads {
            if ends.contains(&r.as_str()) {
                continue;
            }
            let Some(fp) = self.layout.footprints.iter().find(|f| &f.reference == r) else {
                continue;
            };
            let Some(part) = self.lib.parts.get(fp.value.trim()) else {
                if seen.insert((r.clone(), String::new())) {
                    unknown.push(format!("{r} ({})", fp.value));
                }
                continue;
            };
            let Some(pin) = part.pins.get(num) else {
                if seen.insert((r.clone(), format!("#{num}"))) {
                    unknown.push(format!("{r} ({}, pad {num} isn't in its part file's pins)", part.mpn));
                }
                continue;
            };
            if !seen.insert((r.clone(), pin.clone())) {
                continue;
            }
            match part
                .capacitance
                .iter()
                .find(|c| c.pins.contains(pin))
                .and_then(|c| c.worst().map(|v| (v, c)))
            {
                Some((v, c)) => {
                    total += v;
                    let kind = if c.max.is_some() { "max" } else { "typ" };
                    used.push(format!("{r} {} {v} pF {kind} (datasheet page {})", part.mpn, c.page));
                }
                None => unknown.push(format!("{r} ({}, no capacitance in its part file)", part.mpn)),
            }
        }
        PadLoad {
            pf: total,
            unknown,
            used,
        }
    }
}

/// Builds the channel model of one line.
fn model(
    route: &Route,
    partner: &Route,
    an: &mut Analyzer<'_>,
    loads: &Loads<'_>,
    ends: &[&str],
) -> (Line, Vec<String>) {
    let others: Vec<_> = partner.pieces().cloned().collect();
    let mut line = Line::default();
    let mut unknown = Vec::new();
    let board = an.span_total();
    let er = an.mean_er();
    let clearance = an.antipad_clearance();
    for step in &route.steps {
        match step {
            Step::Track(piece) => {
                let (secs, bare) = an.piece_sections(piece, &others);
                for s in secs {
                    line.elements.push(Element::Line {
                        z: s.solution.odd.z,
                        er_eff: s.solution.odd.er_eff,
                        length: s.length * 1e-3,
                        width: s.width * 1e-3,
                        loss_tangent: s.loss_tangent,
                    });
                }
                // Copper with no plane anywhere: model it over the far side of the board.
                for b in bare {
                    let z = 2.0 * 60.0 / 1.5f64.sqrt() * (8.0 * board / b.width).ln().max(1.0) / 2.0;
                    line.elements.push(Element::Line {
                        z,
                        er_eff: 2.5,
                        length: b.length() * 1e-3,
                        width: b.width * 1e-3,
                        loss_tangent: 0.02,
                    });
                }
            }
            Step::Via {
                from,
                to,
                drill,
                diameter,
                ..
            } => {
                // Howard Johnson's via models (High-Speed Digital Design, 1993, section 7.4): capacitance
                // C = 1.41 εr T D1 / (D2 − D1) pF and inductance L = 5.08 h (ln(4h/d) + 1) nH, lengths in
                // inches; T is the board thickness, D1 the pad, D2 the plane clearance hole, h the length the
                // current travels and d the drill.
                let inch = 25.4;
                let d1 = diameter / inch;
                let d2 = (diameter + 2.0 * clearance) / inch;
                let c = 1.41e-12 * er * (board / inch) * d1 / (d2 - d1);
                let h = an.span(from, to).unwrap_or(board).max(0.05) / inch;
                let l = 5.08e-9 * h * ((4.0 * h / (drill / inch)).ln() + 1.0).max(0.0);
                line.elements.push(Element::Shunt { c: c / 2.0 });
                line.elements.push(Element::Series { l });
                line.elements.push(Element::Shunt { c: c / 2.0 });
            }
            Step::Stub(stub) => {
                let PadLoad { pf, unknown: unk, .. } = loads.of(&stub.pads, ends);
                unknown.extend(unk);
                let c_end = pf * 1e-12;
                if stub.length < 1e-3 {
                    if c_end > 0.0 {
                        line.elements.push(Element::Shunt { c: c_end });
                    }
                    continue;
                }
                let (layer, width) = (stub.layer.clone().unwrap_or_default(), stub.width.unwrap_or(0.2));
                let (z, er_eff, tan_d) = match an.lone(&layer, width, stub.at) {
                    Some((s, t)) => (s.odd.z, s.odd.er_eff, t),
                    None => (100.0, 2.5, 0.02),
                };
                line.elements.push(Element::Stub {
                    z,
                    er_eff,
                    length: stub.length * 1e-3,
                    width: width * 1e-3,
                    loss_tangent: tan_d,
                    c_end,
                });
            }
            Step::Pad { reference, number } => {
                let PadLoad { pf, unknown: unk, .. } = loads.of(&[(reference.clone(), number.clone())], ends);
                unknown.extend(unk);
                if pf > 0.0 {
                    line.elements.push(Element::Shunt { c: pf * 1e-12 });
                }
            }
        }
    }
    (line, unknown)
}

fn fmt_pt(p: (f64, f64)) -> String {
    format!("({:.2}, {:.2})", p.0, p.1)
}

/// Runs every USB 2.0 rule and the eye simulation on each USB pair of the board.
pub fn check(layout: &Layout, lib: &Library) -> Report {
    let mut report = Report {
        assumptions: layout.assumptions.clone(),
        ..Report::default()
    };
    let mut an = Analyzer::new(layout);
    let loads = Loads { layout, lib };
    let connectors: BTreeSet<&str> = layout
        .footprints
        .iter()
        .filter(|f| is_connector(f))
        .map(|f| f.reference.as_str())
        .collect();
    for pair in find_pairs(layout) {
        if !is_usb(&pair) {
            report.skipped.push(format!(
                "{} ({} / {}): no rules for this kind of pair yet",
                pair.name, pair.p, pair.n
            ));
            continue;
        }
        let name = pair.name.clone();
        let mut push = |rule: &'static str, verdict: Verdict, severity: Severity, message: String| {
            report.findings.push(Finding {
                rule,
                verdict,
                severity,
                pair: name.clone(),
                message,
            });
        };
        // When the pair reaches a connector that looks like a USB one, only those count as the far end.
        let on_net: Vec<&PlacedFootprint> = layout
            .footprints
            .iter()
            .filter(|f| {
                connectors.contains(f.reference.as_str())
                    && f.pads
                        .iter()
                        .any(|pd| pd.net.as_deref().is_some_and(|n| n == pair.p || n == pair.n))
            })
            .collect();
        let usb_like = |f: &PlacedFootprint| {
            f.lib_id.to_ascii_uppercase().contains("USB") || f.value.to_ascii_uppercase().contains("USB")
        };
        let ends_ok: BTreeSet<&str> = if on_net.iter().any(|f| usb_like(f)) {
            on_net
                .iter()
                .filter(|f| usb_like(f))
                .map(|f| f.reference.as_str())
                .collect()
        } else {
            connectors.clone()
        };
        let is_end = |r: &str| ends_ok.contains(r);
        let rp = route::trace(layout, &pair.p, is_end, None);
        let rn = route::trace(layout, &pair.n, is_end, None);
        let (rp, rn) = match (rp, rn) {
            (Ok(p), Ok(n)) => (p, n),
            (p, n) => {
                for e in [p.err(), n.err()].into_iter().flatten() {
                    push(
                        "usb-length",
                        Verdict::Unknown,
                        Severity::Error,
                        format!("can't trace the pair: {e}"),
                    );
                }
                continue;
            }
        };
        let ends = [
            rp.from.0.as_str(),
            rp.to.0.as_str(),
            rn.from.0.as_str(),
            rn.to.0.as_str(),
        ];
        for r in [&rp, &rn] {
            for (refd, pad) in &r.unrouted {
                push(
                    "usb-length",
                    Verdict::Fail,
                    Severity::Error,
                    format!("{}: pad {refd}.{pad} is on the net but no copper reaches it", r.net),
                );
            }
        }

        // Impedance.
        let (sp, bare_p) = an.sections(&rp, Some(&rn));
        let (sn, bare_n) = an.sections(&rn, Some(&rp));
        // Runs of consecutive sections outside the band, as (length, the section farthest out, net).
        let mut runs: Vec<(f64, Section, String)> = Vec::new();
        for (secs, net) in [(&sp, &rp.net), (&sn, &rn.net)] {
            let mut run: Vec<&Section> = Vec::new();
            for s in secs.iter().chain(std::iter::once(&Section::sentinel())) {
                if s.length > 0.0 && !(usb2::Z_MIN..=usb2::Z_MAX).contains(&s.z_diff()) {
                    run.push(s);
                    continue;
                }
                if let Some(far) = run
                    .iter()
                    .max_by(|a, b| (a.z_diff() - 90.0).abs().total_cmp(&(b.z_diff() - 90.0).abs()))
                {
                    runs.push((run.iter().map(|s| s.length).sum(), (*far).clone(), net.clone()));
                }
                run.clear();
            }
        }
        let short_runs = runs.iter().filter(|r| r.0 <= usb2::ESCAPE).count();
        let worst = runs
            .iter()
            .filter(|r| r.0 > usb2::ESCAPE)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(len, s, net)| (*len, s.z_diff(), s, net.as_str()));
        let coupled: Vec<&Section> = sp.iter().filter(|s| s.gap.is_some()).collect();
        let typical = coupled.iter().max_by(|a, b| a.length.total_cmp(&b.length)).map_or_else(
            || "never coupled: the two lines don't run side by side".to_string(),
            |s| {
                format!(
                    "{:.1} Ω over most of its length ({:.3} mm traces, {:.3} mm gap on {})",
                    s.z_diff(),
                    s.width,
                    s.gap.unwrap_or(0.0),
                    s.layer
                )
            },
        );
        match worst {
            Some((len, z, s, net)) => push(
                "usb-impedance",
                Verdict::Fail,
                Severity::Error,
                format!(
                    "{net} is outside {}–{} Ω for {len:.1} mm, reaching {z:.0} Ω differential at {} on {} ({:.3} mm wide, {}) ({}). The pair is {typical}",
                    usb2::Z_MIN,
                    usb2::Z_MAX,
                    fmt_pt(s.start),
                    s.layer,
                    s.width,
                    s.gap
                        .map_or("not coupled to its partner".to_string(), |g| format!("{g:.3} mm gap")),
                    cite::SPRAAR7
                ),
            ),
            None => push(
                "usb-impedance",
                Verdict::Pass,
                Severity::Error,
                format!(
                    "{typical}, within {}–{} Ω ({}){}",
                    usb2::Z_MIN,
                    usb2::Z_MAX,
                    cite::SPRAAR7,
                    if short_runs > 0 {
                        format!(
                            "; {short_runs} excursion(s) shorter than {} mm at pads are left to the eye simulation",
                            usb2::ESCAPE
                        )
                    } else {
                        String::new()
                    }
                ),
            ),
        }

        // Skew.
        let (lp, ln) = (rp.length(), rn.length());
        let skew = (lp - ln).abs();
        push(
            "usb-skew",
            if skew <= usb2::SKEW {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
            Severity::Error,
            format!(
                "{} and {} differ by {skew:.2} mm ({:.0} ps), against {} mm ({}); {lp:.2} mm and {ln:.2} mm long",
                rp.net,
                rn.net,
                skew * 1e-3 * sp.first().map_or(1.8, |s| s.solution.odd.er_eff.sqrt()) / xsection::C0 * 1e12,
                usb2::SKEW,
                cite::SPRAAR7
            ),
        );

        // Vias.
        let (vp, vn) = (rp.vias(), rn.vias());
        let verdict = if vp.max(vn) > usb2::VIAS {
            Verdict::Fail
        } else {
            Verdict::Pass
        };
        push(
            "usb-vias",
            verdict,
            Severity::Error,
            format!(
                "{} has {vp} via(s) and {} has {vn}, against at most {} each ({})",
                rp.net,
                rn.net,
                usb2::VIAS,
                cite::SPRAAR7
            ),
        );
        if vp != vn && verdict == Verdict::Pass {
            push(
                "usb-vias",
                Verdict::Fail,
                Severity::Warning,
                format!(
                    "{} has {vp} via(s) and {} has {vn}; TI asks for the same count on both lines (SPRAAR7J section 3.9)",
                    rp.net, rn.net
                ),
            );
        }

        // Stubs.
        let mut any_stub = false;
        for r in [&rp, &rn] {
            for s in r.stubs() {
                if s.length < 1e-3 {
                    continue;
                }
                any_stub = true;
                let pads: Vec<String> = s.pads.iter().map(|(a, b)| format!("{a}.{b}")).collect();
                let reaches = if pads.is_empty() {
                    "nothing".to_string()
                } else {
                    pads.join(", ")
                };
                let same_connector = !s.pads.is_empty() && s.pads.iter().all(|(a, _)| a == &r.to.0);
                let (verdict, severity, why) = if s.length >= usb2::STUB {
                    (
                        Verdict::Fail,
                        Severity::Error,
                        format!("at or over the {} mm limit ({})", usb2::STUB, cite::TUSB1210),
                    )
                } else if same_connector {
                    (
                        Verdict::Pass,
                        Severity::Warning,
                        "the connector's second contact for the other plug orientation".to_string(),
                    )
                } else {
                    (
                        Verdict::Fail,
                        Severity::Warning,
                        format!("under {} mm, but TI allows none ({})", usb2::STUB, cite::SPRAAR7),
                    )
                };
                push(
                    "usb-stubs",
                    verdict,
                    severity,
                    format!(
                        "{}: a {:.2} mm stub reaches {reaches}{}, {why}; at {}",
                        r.net,
                        s.length,
                        if s.vias > 0 {
                            format!(" through {} via(s)", s.vias)
                        } else {
                            String::new()
                        },
                        fmt_pt(s.at)
                    ),
                );
            }
        }
        if !any_stub {
            push(
                "usb-stubs",
                Verdict::Pass,
                Severity::Error,
                "no stubs on either line".to_string(),
            );
        }

        // Reference planes.
        let mut gaps = Vec::new();
        let mut edge = None::<(f64, String)>;
        for (secs, bare, net) in [(&sp, &bare_p, &rp.net), (&sn, &bare_n, &rn.net)] {
            let mut places = Vec::new();
            for s in secs.iter().filter(|s| s.plane_gap) {
                let refs: Vec<String> = [&s.planes.below, &s.planes.above]
                    .into_iter()
                    .flatten()
                    .map(|(i, n)| format!("{n} on {}", an.layer_name(*i)))
                    .collect();
                places.push(format!(
                    "{:.1} mm at {} on {} (nearest plane: {})",
                    s.length,
                    fmt_pt(s.start),
                    s.layer,
                    if refs.is_empty() {
                        "none".to_string()
                    } else {
                        refs.join(", ")
                    }
                ));
            }
            let bare_len: f64 = bare.iter().map(route::Piece::length).sum();
            if bare_len > 0.0 {
                places.push(format!(
                    "{bare_len:.1} mm with no plane on any layer, from {}",
                    bare.first().map_or(String::new(), |b| fmt_pt(b.start))
                ));
            }
            if !places.is_empty() {
                gaps.push(format!(
                    "{net} loses the plane on the next layer for {}",
                    places.join("; ")
                ));
            }
            for s in secs.iter().filter(|s| !s.plane_gap) {
                if let Some((d, at)) = s.plane_edge
                    && d < usb2::PLANE_EDGE
                    && edge.as_ref().is_none_or(|e| d < e.0)
                {
                    edge = Some((
                        d,
                        format!(
                            "{net} passes {d:.2} mm from the edge of its plane at {} on {}",
                            fmt_pt(at),
                            s.layer
                        ),
                    ));
                }
            }
        }
        if gaps.is_empty() {
            push(
                "usb-reference",
                Verdict::Pass,
                Severity::Error,
                "both lines run over an unbroken plane on the next layer".to_string(),
            );
        } else {
            push(
                "usb-reference",
                Verdict::Fail,
                Severity::Error,
                format!(
                    "{}. The return current has to detour around each gap ({}, item 2.14)",
                    gaps.join(". "),
                    cite::TUSB1210
                ),
            );
        }
        if let Some((_, msg)) = edge {
            push(
                "usb-reference",
                Verdict::Fail,
                Severity::Warning,
                format!(
                    "{msg}, against at least {} mm ({}, item 2.15)",
                    usb2::PLANE_EDGE,
                    cite::TUSB1210
                ),
            );
        }

        // Length.
        let longest = lp.max(ln);
        push(
            "usb-length",
            if longest <= usb2::LENGTH {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
            Severity::Error,
            format!(
                "the longer line is {longest:.1} mm, against {} mm ({})",
                usb2::LENGTH,
                cite::SPRAAR7
            ),
        );

        // Capacitance.
        for r in [&rp, &rn] {
            // Parts hanging off the line and parts the line runs through.
            let mut pads: Vec<(String, String)> = r.stubs().flat_map(|s| s.pads.clone()).collect();
            pads.extend(r.steps.iter().filter_map(|s| match s {
                Step::Pad { reference, number } => Some((reference.clone(), number.clone())),
                _ => None,
            }));
            let PadLoad { pf, unknown, used } = loads.of(&pads, &ends);
            let detail = if used.is_empty() {
                "no parts".to_string()
            } else {
                used.join(", ")
            };
            let (verdict, extra) = if pf > usb2::CAPACITANCE {
                (Verdict::Fail, String::new())
            } else if !unknown.is_empty() {
                (Verdict::Unknown, format!("; no figure for {}", unknown.join(", ")))
            } else {
                (Verdict::Pass, String::new())
            };
            push(
                "usb-capacitance",
                verdict,
                Severity::Error,
                format!(
                    "{}: {pf:.2} pF from {detail}{extra}, against {} pF ({}, item 1.06)",
                    r.net,
                    usb2::CAPACITANCE,
                    cite::TUSB1210
                ),
            );
        }

        // Simulation.
        let (mp, unk_p) = model(&rp, &rn, &mut an, &loads, &ends);
        let (mn, unk_n) = model(&rn, &rp, &mut an, &loads, &ends);
        let term = Ends {
            source: usb2::TERMINATION,
            load: usb2::TERMINATION,
        };
        let wave = channel::simulate(&mp, &mn, term, usb2::UI, usb2::RISE, usb2::SWING, 64);
        let eye = eye::measure(&wave, USB2_TEMPLATE_1);
        let window = 2.0 * mp.delay().max(mn.delay()) + 1.0e-9;
        let tdr = vec![
            channel::tdr(&mp, term, 200e-12, 5e-12, window)
                .into_iter()
                .map(|(t, z)| (t, 2.0 * z))
                .collect(),
            channel::tdr(&mn, term, 200e-12, 5e-12, window)
                .into_iter()
                .map(|(t, z)| (t, 2.0 * z))
                .collect(),
        ];
        let mut unknown: Vec<String> = unk_p.into_iter().chain(unk_n).collect();
        unknown.sort();
        unknown.dedup();
        let verdict = if !eye.passes() {
            Verdict::Fail
        } else if unknown.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Unknown
        };
        let inner = if eye.inner_margin >= 0.0 {
            format!("clears the inner mask by {:.0} mV", eye.inner_margin * 1e3)
        } else {
            format!(
                "enters the inner mask by {:.0} mV (at {:.0} % of the UI)",
                -eye.inner_margin * 1e3,
                eye.inner_worst_at * 100.0
            )
        };
        let outer = if eye.outer_margin >= 0.0 {
            format!("stays {:.0} mV inside the outer limits", eye.outer_margin * 1e3)
        } else {
            format!("overshoots the outer limits by {:.0} mV", -eye.outer_margin * 1e3)
        };
        let mut msg = format!(
            "at {}: eye height {:.0} mV, width {:.2} UI; it {inner} and {outer}",
            rp.to.0,
            eye.height * 1e3,
            eye.width,
        );
        let _ = write!(
            msg,
            " ({}). Driven by an ideal {} Ω, {:.0} mV source with {:.0} ps edges ({}) into {} Ω",
            cite::USB_T1,
            usb2::TERMINATION,
            usb2::SWING * 1e3,
            usb2::RISE * 1e12,
            cite::USB_DRIVER,
            usb2::TERMINATION
        );
        if !unknown.is_empty() {
            let _ = write!(msg, "; not modelled: {}", unknown.join(", "));
        }
        push("usb-eye", verdict, Severity::Error, msg);

        report.pairs.push(PairReport {
            name: pair.name.clone(),
            p: pair.p.clone(),
            n: pair.n.clone(),
            from: format!("{}.{}", rp.from.0, rp.from.1),
            to: format!("{}.{}", rp.to.0, rp.to.1),
            length_p: lp,
            length_n: ln,
            vias_p: vp,
            vias_n: vn,
            sections: sp,
            eye_height_mv: Some(eye.height * 1e3),
            eye_width_ui: Some(eye.width),
            mask_margin_mv: Some(eye.inner_margin.min(eye.outer_margin) * 1e3),
            eye: Some(eye),
            tdr,
        });
    }
    report
}

/// The report as text: each pair, then its errors and warnings, and its unknowns and passes when `show` asks.
pub fn render(report: &Report, show: &Show) -> String {
    let mut out = String::new();
    let style = show.style;
    let width = show.columns();
    for a in &report.assumptions {
        let _ = writeln!(out, "{}: {a}", style.paint("1", "assumption"));
    }
    for s in &report.skipped {
        let _ = writeln!(out, "{}: {s}", style.paint("1", "skipped"));
    }
    if report.pairs.is_empty() && report.findings.is_empty() {
        let _ = writeln!(
            out,
            "no USB pairs found (nets ending in D+/D-, DP/DM, _P/_N with USB in the name)"
        );
    }
    let mut listed = crate::diag::Listed::default();
    let section = |out: &mut String, listed: &mut crate::diag::Listed, header: &str, pair: &str, nets: &[&str]| {
        let _ = writeln!(out, "\n{}", style.paint("1", header));
        let mut sorted: Vec<&Finding> = report.findings.iter().filter(|f| f.pair == pair).collect();
        sorted.sort_by_key(|f| (f.verdict, f.severity));
        for f in sorted {
            let mut d = diagnostic(f, nets);
            if show.lists(d.level) {
                if !out.ends_with("\n\n") {
                    out.push('\n');
                }
                d.location = Some(match d.location.take() {
                    Some(at) => format!("{at}, pair {pair}"),
                    None => format!("pair {pair}"),
                });
                listed.add(&d);
                d.render(out, style, show.verbose, width);
            }
        }
    };
    for p in &report.pairs {
        let header = format!(
            "{} ({} / {}): {} → {}, {:.1} / {:.1} mm, {} / {} vias",
            p.name, p.p, p.n, p.from, p.to, p.length_p, p.length_n, p.vias_p, p.vias_n
        );
        section(&mut out, &mut listed, &header, &p.name, &[p.p.as_str(), p.n.as_str()]);
    }
    // Pairs that couldn't be traced have findings but no report.
    let untraced: BTreeSet<&str> = report
        .findings
        .iter()
        .map(|f| f.pair.as_str())
        .filter(|n| !report.pairs.iter().any(|p| p.name == *n))
        .collect();
    for name in untraced {
        section(&mut out, &mut listed, &format!("{name}: not traced"), name, &[name]);
    }
    let count = |v: Verdict, s: Option<Severity>| {
        report
            .findings
            .iter()
            .filter(|f| f.verdict == v && s.is_none_or(|s| f.severity == s))
            .count()
    };
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
    crate::diag::summary(
        &mut out,
        show,
        [
            count(Verdict::Fail, Some(Severity::Error)),
            count(Verdict::Fail, Some(Severity::Warning)),
            count(Verdict::Unknown, None),
            count(Verdict::Pass, None),
        ],
        &listed,
    );
    out
}

/// One finding as a diagnostic; `nets` are the pair's nets, which messages open with.
fn diagnostic(f: &Finding, nets: &[&str]) -> Diagnostic {
    let level = match (f.verdict, f.severity) {
        (Verdict::Fail, Severity::Error) => Level::Error,
        (Verdict::Fail, Severity::Warning) => Level::Warning,
        (Verdict::Unknown, _) => Level::Unknown,
        (Verdict::Pass, _) => Level::Pass,
    };
    if level == Level::Pass {
        return Diagnostic::line(level, f.rule, &f.message);
    }
    Diagnostic::from_message(level, f.rule, nets, &f.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kicad::pcb::Track;

    fn layout(nets: &[&str]) -> Layout {
        let track = |n: &&str| Track {
            net: (*n).to_string(),
            layer: "F.Cu".into(),
            start: (0.0, 0.0),
            end: (1.0, 0.0),
            width: 0.2,
        };
        Layout {
            tracks: nets.iter().map(track).collect(),
            ..Layout::default()
        }
    }

    #[test]
    fn pairs_are_found_by_their_endings() {
        let l = layout(&[
            "/USB_D+",
            "/USB_D-",
            "OTG_DP",
            "OTG_DM",
            "eth_tx_p",
            "ETH_TX_N",
            "LONELY_D+",
            "GND",
        ]);
        let got: Vec<(String, String, String)> = find_pairs(&l).into_iter().map(|p| (p.name, p.p, p.n)).collect();
        assert_eq!(
            got,
            vec![
                ("OTG".into(), "OTG_DP".into(), "OTG_DM".into()),
                ("USB".into(), "/USB_D+".into(), "/USB_D-".into()),
                ("eth_tx".into(), "eth_tx_p".into(), "ETH_TX_N".into()),
            ]
        );
    }

    #[test]
    fn only_usb_pairs_are_usb() {
        let pair = |p: &str| PairNets {
            name: String::new(),
            p: p.into(),
            n: String::new(),
        };
        assert!(is_usb(&pair("USB1_D+")));
        assert!(is_usb(&pair("HOST_DP")));
        assert!(!is_usb(&pair("ETH_TX_P")));
    }
}
