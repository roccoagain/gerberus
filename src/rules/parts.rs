//! Rules for whole parts: a TVS's breakdown, a capacitor's voltage rating and a resistor's power.
use crate::finding::{Severity, Verdict};
use crate::limits::{Library, Limit, Range};
use crate::policy::Policy;
use crate::voltage::{Span, resistance_range};

use super::compare::{magnitude, magnitude_floor, unless_floor, v, value_of};
use super::{Ctx, Report};

pub(super) fn tvs(
    ctx: &Ctx<'_>,
    limit: &Limit,
    span: Span,
    floor: (bool, bool),
    head: &str,
    nets: &[usize],
    r: &mut Report,
) {
    // Breakdown: a rail that can sit at or above the lowest breakdown voltage drives current through the part
    // continuously, and it burns. A bidirectional part breaks down either way, so its magnitude counts.
    if let Some(bd) = &limit.breakdown {
        let lowest = match bd {
            Range::Known { min, .. } => value_of(min),
            Range::Unknown(_) | Range::None(_) => None,
        };
        let (seen, on_floor) = if limit.bidirectional {
            (magnitude(span).max, magnitude_floor(span, floor))
        } else {
            (span.max, floor.1)
        };
        let (vd, msg) = match (seen, lowest) {
            (Some(x), Some(b)) if x >= b => (
                Verdict::Fail,
                format!(
                    "{head}: reaches {}, at or above its lowest breakdown voltage of {} ({})",
                    v(x),
                    v(b),
                    ctx.cite(bd)
                ),
            ),
            (Some(x), Some(b)) => (
                Verdict::Pass,
                format!(
                    "{head}: at most {}, below the lowest breakdown of {} ({})",
                    v(x),
                    v(b),
                    ctx.cite(bd)
                ),
            ),
            (None, _) => (Verdict::Unknown, format!("{head}: the net's maximum is UNKNOWN")),
            (_, None) => (
                Verdict::Unknown,
                format!("{head}: the datasheet gives no lowest breakdown voltage"),
            ),
        };
        let (vd, msg) = unless_floor(vd, msg, on_floor);
        r.findings
            .push(ctx.finding("tvs-breakdown", vd, Severity::Error, msg, nets.to_vec()));
    }
    // Stand-off: outside VRWM the leakage is no longer specified, and it is a sign the TVS is too small. Signed,
    // like every recommended range.
    let (lo, hi) = match &limit.recommended {
        Range::Known { min, max, .. } => (value_of(min), value_of(max)),
        Range::Unknown(_) | Range::None(_) => (None, None),
    };
    let src = ctx.cite(&limit.recommended);
    let mut fails = vec![];
    let mut unknown = vec![];
    match (span.max, hi) {
        (Some(x), Some(s)) if x > s + 1e-9 => {
            fails.push((format!("reaches {}, above the stand-off of {}", v(x), v(s)), floor.1));
        }
        (Some(_), Some(_)) => {}
        (None, _) => unknown.push("the net's maximum is UNKNOWN".to_string()),
        (_, None) => unknown.push("the datasheet gives no stand-off voltage".to_string()),
    }
    if limit.bidirectional {
        match (span.min, lo) {
            (Some(x), Some(s)) if x < s - 1e-9 => {
                fails.push((format!("falls to {}, below the stand-off of {}", v(x), v(s)), floor.0));
            }
            (Some(_), Some(_)) => {}
            (None, _) => unknown.push("the net's minimum is UNKNOWN".to_string()),
            (_, None) => unknown.push("the datasheet gives no negative stand-off voltage".to_string()),
        }
    }
    let proven: Vec<&String> = fails.iter().filter(|(_, f)| !f).map(|(m, _)| m).collect();
    let (vd, msg) = if !proven.is_empty() {
        (
            Verdict::Fail,
            format!(
                "{head}: {} ({src})",
                proven.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("; ")
            ),
        )
    } else if !fails.is_empty() || !unknown.is_empty() {
        let mut parts: Vec<String> = fails
            .iter()
            .map(|(m, _)| format!("{m}, but that end is only a bound: a regulator isn't held"))
            .collect();
        parts.extend(unknown);
        (Verdict::Unknown, format!("{head}: {} ({src})", parts.join("; ")))
    } else {
        (Verdict::Pass, format!("{head}: within the stand-off voltage ({src})"))
    };
    r.findings
        .push(ctx.finding("tvs-standoff", vd, Severity::Warning, msg, nets.to_vec()));
}

/// Capacitors: the signed voltage against the signed rating, then against the derating policy.
#[expect(
    clippy::too_many_arguments,
    reason = "unrelated inputs; a one-use struct would only rename them"
)]
pub(super) fn cap_voltage(
    ctx: &Ctx<'_>,
    policy: &Policy,
    limit: &Limit,
    span: Span,
    floor: (bool, bool),
    head: &str,
    nets: &[usize],
    r: &mut Report,
) {
    let (rmin, rmax) = match &limit.recommended {
        Range::Known { min, max, .. } => (value_of(min), value_of(max)),
        Range::Unknown(_) | Range::None(_) => (None, None),
    };
    let source = if ctx.part.datasheet.is_some() {
        ctx.cite(&limit.recommended)
    } else {
        format!("{}, not checked against a datasheet", ctx.part.description)
    };
    let factor = policy.derate_for(ctx.part, limit).map(|d| (d.factor, d.reason.clone()));
    // Each end: (seen, rating, which way is past it, is that end a floor).
    let ends = [(span.max, rmax, true, floor.1), (span.min, rmin, false, floor.0)];
    let mut worst: Option<(Verdict, Severity, String)> = None;
    let mut unknown = vec![];
    for (seen, rating, up, on_floor) in ends {
        let (Some(x), Some(rv)) = (seen, rating) else {
            if seen.is_none() {
                unknown.push(format!(
                    "the voltage across it is UNKNOWN at its {} end",
                    if up { "top" } else { "bottom" }
                ));
            } else {
                unknown.push(format!(
                    "no rated {} voltage ({source})",
                    if up { "positive" } else { "negative" }
                ));
            }
            continue;
        };
        let past = |limit: f64| if up { x > limit + 1e-9 } else { x < limit - 1e-9 };
        let found = if past(rv) {
            Some((
                Severity::Error,
                format!("{} across it, past its {} rating ({source})", v(x), v(rv)),
            ))
        } else if let Some((f, reason)) = &factor
            && past(rv * f)
        {
            Some((
                Severity::Warning,
                format!(
                    "{} across it, past {} % of its {} rating ({source}); {reason}",
                    v(x),
                    f * 100.0,
                    v(rv)
                ),
            ))
        } else {
            None
        };
        if let Some((sev, m)) = found {
            let (vd, m) = unless_floor(Verdict::Fail, m, on_floor);
            if worst.as_ref().is_none_or(|(wv, ws, _)| (vd, sev) < (*wv, *ws)) {
                worst = Some((vd, sev, m));
            }
        }
    }
    let (vd, severity, msg) = match worst {
        Some((Verdict::Fail, s, m)) => (Verdict::Fail, s, format!("{head}: {m}")),
        Some((vd, s, m)) => (vd, s, format!("{head}: {m}")),
        None if !unknown.is_empty() => (
            Verdict::Unknown,
            Severity::Error,
            format!("{head}: {}", unknown.join("; ")),
        ),
        None => (
            Verdict::Pass,
            Severity::Error,
            format!("{head}: within its rating ({source})"),
        ),
    };
    r.findings
        .push(ctx.finding("cap-voltage", vd, severity, msg, nets.to_vec()));
}

pub(super) fn resistor_power(ctx: &Ctx<'_>, policy: &Policy, limit: &Limit, r: &mut Report) {
    let a = limit.pins.first().and_then(|p| ctx.net(p));
    let b = limit.to.as_deref().and_then(|p| ctx.net(p));
    let (Some(a), Some(b)) = (a, b) else {
        r.findings.push(ctx.finding(
            "resistor-power",
            Verdict::Unknown,
            Severity::Error,
            format!(
                "{}: its power limit names pins {:?} to {:?}, which aren't both in the schematic",
                ctx.reference, limit.pins, limit.to
            ),
            vec![],
        ));
        return;
    };
    let (diff, notes) = ctx.diff_why(a, b);
    let across = magnitude(diff);
    // A drop worked out from the load doesn't rest on how far a rail sags.
    let on_floor = magnitude_floor(diff, (ctx.volts.nets[a].floor, ctx.volts.nets[b].floor))
        && !matches!(ctx.series_feed(a, b), Some(Ok(_)));
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join("; "))
    };
    let head = format!(
        "{} ({} to {}): {} across it{notes}",
        ctx.reference,
        ctx.net_name(a),
        ctx.net_name(b),
        across
    );
    let rated = match &limit.recommended {
        Range::Known { max, .. } => value_of(max),
        Range::Unknown(_) | Range::None(_) => None,
    };
    let (vd, severity, msg) = match (across.max, smallest_resistance(ctx), rated) {
        (None, ..) => (
            Verdict::Unknown,
            Severity::Error,
            format!("{head}: the voltage across it is UNKNOWN"),
        ),
        (_, Err(e), _) => (Verdict::Unknown, Severity::Error, format!("{head}: {e}")),
        (_, _, None) => (Verdict::Unknown, Severity::Error, format!("{head}: no power rating")),
        (Some(x), Ok((rmin, note)), Some(p)) => {
            let watts = x * x / rmin;
            let derate = policy.derate_for(ctx.part, limit);
            let w = |p: f64| format!("{} mW", (p * 1e4).round() / 10.0);
            let worst = format!(
                "{} at {} over {} Ω ({note})",
                w(watts),
                v(x),
                (rmin * 10.0).round() / 10.0
            );
            if watts > p {
                (
                    Verdict::Fail,
                    Severity::Error,
                    format!(
                        "{head}: up to {}, above its {} rating ({}); {worst}",
                        w(watts),
                        w(p),
                        ctx.cite(&limit.recommended)
                    ),
                )
            } else if let Some(d) = derate.filter(|d| watts > p * d.factor) {
                (
                    Verdict::Fail,
                    Severity::Warning,
                    format!(
                        "{head}: up to {}, above {} % of its {} rating; {worst}; {}",
                        w(watts),
                        d.factor * 100.0,
                        w(p),
                        d.reason
                    ),
                )
            } else {
                (
                    Verdict::Pass,
                    Severity::Error,
                    format!(
                        "{head}: up to {}, within its {} rating ({}); {worst}",
                        w(watts),
                        w(p),
                        ctx.cite(&limit.recommended)
                    ),
                )
            }
        }
    };
    let (vd, msg) = unless_floor(vd, msg, on_floor);
    let (vd, msg) = super::on_bound(vd, msg, ctx.only_bound(a, b).as_ref());
    r.findings
        .push(ctx.finding("resistor-power", vd, severity, msg, vec![a, b]));
}

/// The smallest the resistor can be, from its tolerance and temperature drift, and where that comes from.
fn smallest_resistance(ctx: &Ctx<'_>) -> Result<(f64, String), String> {
    let mut lib = Library::default();
    lib.parts
        .insert(crate::limits::component_mpn(ctx.c).to_string(), ctx.part.clone());
    resistance_range(&lib, ctx.inputs, ctx.reference, ctx.c).map(|range| (range.min, range.note))
}
