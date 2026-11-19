//! Comparing a net's worst-case voltage with a pin's limits: each end of the range against each bound, and the
//! verdict and message that come out.
use crate::finding::{Severity, Verdict};
use crate::limits::{Bound, Class, Limit, Range};
use crate::policy::Policy;
use crate::voltage::Span;
use std::collections::BTreeSet;

use super::parts::{cap_voltage, tvs};
use super::{Ctx, Report};

/// Both bounds at once. An unknown end of one side leaves the other side's end.
pub(super) fn intersect(a: Span, b: Span) -> Span {
    let pick = |p: Option<f64>, q: Option<f64>, f: fn(f64, f64) -> f64| match (p, q) {
        (Some(p), Some(q)) => Some(f(p, q)),
        (p, q) => p.or(q),
    };
    Span {
        min: pick(a.min, b.min, f64::max),
        max: pick(a.max, b.max, f64::min),
    }
}

/// The result of comparing a voltage with one end of a range.
pub(super) enum End {
    Ok,
    Fail(String),
    Unknown(String),
}

pub(super) fn v(x: f64) -> String {
    format!("{} V", (x * 1000.0).round() / 1000.0)
}

/// Compares the voltage `seen` (the pin from the limit's `to`) with a range. `other` gives the voltage of
/// another pin of the part, from the same `to`, for a bound like `{ pin = "VDD", plus = 0.3 }`; `same` says
/// whether that pin is on the checked pin's net, so the two move together.
/// `floor` says which ends of `seen` are only floors (low, high): a FAIL there becomes UNKNOWN.
pub(super) fn compare(
    seen: Span,
    floor: (bool, bool),
    range: &Range,
    other: &dyn Fn(&str) -> Option<Other>,
) -> Vec<End> {
    let Range::Known { min, max, .. } = range else {
        let Range::Unknown(reason) = range else { unreachable!() };
        return vec![End::Unknown(format!("the datasheet gives no range: {reason}"))];
    };
    let soften = |e: End, f: bool| match e {
        End::Fail(why) if f => End::Unknown(format!(
            "{why}, but that end is only a bound: a regulator isn't held and nothing says how far it sags"
        )),
        e => e,
    };
    vec![
        soften(bound(seen.min, min, other, false), floor.0),
        soften(bound(seen.max, max, other, true), floor.1),
    ]
}

/// Another pin a bound is relative to: its voltage, and how the checked pin relates to it.
pub(super) struct Other {
    pub(super) span: Span,
    /// On the checked pin's net.
    pub(super) same: bool,
    /// The checked pin's net can never rise above it.
    pub(super) below: bool,
}

pub(super) fn bound(seen: Option<f64>, b: &Bound, other: &dyn Fn(&str) -> Option<Other>, is_max: bool) -> End {
    let word = if is_max { "maximum" } else { "minimum" };
    let Some(x) = seen else {
        return match b {
            Bound::Unknown(reason) => End::Unknown(format!(
                "the net's {word} is UNKNOWN, and the datasheet gives none: {reason}"
            )),
            _ => End::Unknown(format!("the net's {word} is UNKNOWN")),
        };
    };
    let beyond = |limit: f64| if is_max { x > limit + 1e-9 } else { x < limit - 1e-9 };
    match b {
        Bound::Unknown(reason) => End::Unknown(format!("the datasheet gives no {word}: {reason}")),
        Bound::Value(limit) => {
            if beyond(*limit) {
                End::Fail(format!("reaches {}, past the {word} of {}", v(x), v(*limit)))
            } else {
                End::Ok
            }
        }
        Bound::Pin { pin, plus, at_most } => {
            let cap = |y: f64| at_most.map_or(y, |c| y.min(c));
            let what = format!(
                "{pin}{}{}",
                if *plus == 0.0 {
                    String::new()
                } else if *plus > 0.0 {
                    format!(" + {plus} V")
                } else {
                    format!(" − {} V", -plus)
                },
                at_most.map(|c| format!(" (at most {})", v(c))).unwrap_or_default()
            );
            let Some(Other { span: r, same, below }) = other(pin) else {
                return End::Unknown(format!("the {word} is {what}, and {pin} isn't connected"));
            };
            if is_max && below && !same && *plus >= 0.0 && r.max.is_some_and(|hi| !beyond(cap(hi + plus))) {
                // Everything that sets this net is bounded by the other pin's net, so it can't rise above it.
                return match at_most {
                    Some(c) if beyond(*c) => End::Fail(format!("reaches {}, past the {word} of {}", v(x), v(*c))),
                    _ => End::Ok,
                };
            }
            if same {
                // The pins share a net, so V(pin) − V(other) is 0 and only `plus` and the cap matter.
                let off = if is_max { *plus >= 0.0 } else { *plus <= 0.0 };
                return match (off, at_most) {
                    (false, _) => End::Fail(format!("the {word} is {what}, and {pin} is on the same net")),
                    (true, Some(c)) if is_max && beyond(*c) => {
                        End::Fail(format!("reaches {}, past the {word} of {}", v(x), v(*c)))
                    }
                    _ => End::Ok,
                };
            }
            // On different nets, the two may not move together: pass only if the worst pairing passes.
            let (Some(lo), Some(hi)) = (r.min, r.max) else {
                return End::Unknown(format!("the {word} is {what}, and {pin}'s range is UNKNOWN"));
            };
            let (strict, loose) = if is_max {
                (cap(lo + plus), cap(hi + plus))
            } else {
                (hi + plus, lo + plus)
            };
            if !beyond(strict) {
                End::Ok
            } else if beyond(loose) {
                End::Fail(format!(
                    "reaches {}, past the {word} of {what}, which is at most {}",
                    v(x),
                    v(loose)
                ))
            } else {
                End::Unknown(format!(
                    "reaches {}; the {word} is {what}, between {} and {}, and nothing says how the two move together",
                    v(x),
                    v(strict.min(loose)),
                    v(strict.max(loose))
                ))
            }
        }
    }
}

/// One verdict from several ends: any FAIL fails, else any UNKNOWN is unknown.
pub(super) fn verdict(ends: &[End]) -> (Verdict, String) {
    let fails: Vec<&str> = ends
        .iter()
        .filter_map(|e| if let End::Fail(s) = e { Some(s.as_str()) } else { None })
        .collect();
    if !fails.is_empty() {
        return (Verdict::Fail, fails.join("; "));
    }
    let unknowns: Vec<&str> = ends
        .iter()
        .filter_map(|e| {
            if let End::Unknown(s) = e {
                Some(s.as_str())
            } else {
                None
            }
        })
        .collect();
    if !unknowns.is_empty() {
        return (Verdict::Unknown, unknowns.join("; "));
    }
    (Verdict::Pass, String::new())
}

/// What one limited pin (or set of pins) sees.
pub(super) struct Seen {
    what: String,
    span: Option<Span>,
    /// Which ends are only floors (low, high).
    floor: (bool, bool),
    nets: Vec<usize>,
    /// The limit's `to` net, for bounds relative to another pin.
    to: Option<usize>,
}

/// The voltage each limited pin sees, from the limit's `to` pin or, with no `to`, between each pair of its
/// pins. Each entry: what it describes, the span, the nets it reads, and the `to` net for relative bounds.
fn seen(ctx: &Ctx<'_>, limit: &Limit) -> Vec<Seen> {
    let mut out = vec![];
    if let Some(to) = &limit.to {
        let tn = ctx.net(to);
        for p in &limit.pins {
            match (ctx.net(p), tn) {
                (Some(pn), Some(tn)) => {
                    // Signed: `bidirectional` only makes `breakdown` a magnitude.
                    let s = ctx.diff(pn, tn);
                    let floor = (ctx.volts.nets[pn].floor, ctx.volts.nets[tn].floor);
                    let what = if ctx.span(tn) == Span::exact(0.0) {
                        ctx.at(p, pn)
                    } else {
                        format!("{} from {}", ctx.at(p, pn), ctx.at(to, tn))
                    };
                    out.push(Seen {
                        what,
                        span: Some(s),
                        floor,
                        nets: vec![pn, tn],
                        to: Some(tn),
                    });
                }
                _ => out.push(Seen {
                    what: format!("{}.{p}", ctx.reference),
                    span: None,
                    floor: (false, false),
                    nets: vec![],
                    to: None,
                }),
            }
        }
    } else {
        let nets: Vec<(String, usize)> = limit
            .pins
            .iter()
            .filter_map(|p| ctx.net(p).map(|n| (p.clone(), n)))
            .collect();
        let mut worst: Option<f64> = Some(0.0);
        let mut read = BTreeSet::new();
        for (i, (_, a)) in nets.iter().enumerate() {
            for (_, b) in &nets[i + 1..] {
                let d = magnitude(ctx.diff(*a, *b));
                read.insert(*a);
                read.insert(*b);
                worst = worst.zip(d.max).map(|(w, m)| w.max(m));
            }
        }
        let worst = Some(Span {
            min: Some(0.0),
            max: worst,
        });
        let names: Vec<&str> = nets.iter().map(|(p, _)| p.as_str()).collect();
        let any_floor = read.iter().any(|&n| ctx.volts.nets[n].floor);
        out.push(Seen {
            what: format!("{} between pins {}", ctx.reference, names.join(", ")),
            span: worst,
            floor: (any_floor, any_floor),
            nets: read.into_iter().collect(),
            to: None,
        });
    }
    out
}

/// The size of a voltage of either sign.
pub(super) fn magnitude(s: Span) -> Span {
    match (s.min, s.max) {
        (Some(a), Some(b)) => {
            let hi = a.abs().max(b.abs());
            let lo = if a <= 0.0 && b >= 0.0 {
                0.0
            } else {
                a.abs().min(b.abs())
            };
            Span::new(lo, hi)
        }
        _ => Span::unknown(),
    }
}

pub(super) fn voltage_rules(ctx: &Ctx<'_>, policy: &Policy, limit: &Limit, r: &mut Report) {
    let class = ctx.part.class;
    let passive = matches!(
        class,
        Class::Capacitor | Class::Resistor | Class::Inductor | Class::Connector
    );
    for Seen {
        what,
        span,
        floor,
        nets,
        to,
    } in seen(ctx, limit)
    {
        let Some(span) = span else {
            r.findings.push(ctx.finding(
                "abs-max",
                Verdict::Unknown,
                Severity::Error,
                format!("{what}: the pin isn't in the schematic, or its reference pin isn't"),
                nets,
            ));
            continue;
        };
        let other = |pin: &str| -> Option<Other> {
            let n = ctx.net(pin)?;
            let me = *nets.first()?;
            // Tracking only holds when both are measured from a 0 V ground.
            let grounded = to.is_none_or(|t| ctx.span(t) == Span::exact(0.0));
            Some(Other {
                span: to.map_or(ctx.span(n), |t| ctx.diff(n, t)),
                same: me == n,
                below: grounded && ctx.volts.stays_below(me, n),
            })
        };
        let head = format!("{what}: {span}");

        // abs-max. Passives and TVS diodes put their ratings in `recommended` and `breakdown`, so an unknown
        // abs_max there is expected, not a gap.
        let skip_abs = matches!(limit.abs_max, Range::Unknown(_)) && (passive || class == Class::Tvs);
        if !skip_abs {
            let (vd, why) = verdict(&compare(span, floor, &limit.abs_max, &other));
            let msg = match vd {
                Verdict::Pass => format!("{head}, inside abs max ({})", ctx.cite(&limit.abs_max)),
                _ => format!("{head}: {why} ({})", ctx.cite(&limit.abs_max)),
            };
            r.findings
                .push(ctx.finding("abs-max", vd, Severity::Error, msg, nets.clone()));
        }

        match class {
            Class::Capacitor => cap_voltage(ctx, policy, limit, span, floor, &head, &nets, r),
            Class::Tvs => tvs(ctx, limit, span, floor, &head, &nets, r),
            _ => {
                let (vd, why) = verdict(&compare(span, floor, &limit.recommended, &other));
                let severity = if passive { Severity::Error } else { Severity::Warning };
                let msg = match vd {
                    Verdict::Pass => format!(
                        "{head}, inside the recommended range ({})",
                        ctx.cite(&limit.recommended)
                    ),
                    _ => format!("{head}: {why} ({})", ctx.cite(&limit.recommended)),
                };
                r.findings
                    .push(ctx.finding("recommended", vd, severity, msg, nets.clone()));
            }
        }
    }
}

/// Whether the end of `s` its magnitude comes from is only a floor.
pub(super) fn magnitude_floor(s: Span, floor: (bool, bool)) -> bool {
    match (s.min, s.max) {
        (Some(lo), Some(hi)) => {
            if hi.abs() >= lo.abs() {
                floor.1
            } else {
                floor.0
            }
        }
        _ => false,
    }
}

/// A FAIL that rests on a floor isn't proven, so it becomes UNKNOWN.
pub(super) fn unless_floor(vd: Verdict, msg: String, on_floor: bool) -> (Verdict, String) {
    if vd == Verdict::Fail && on_floor {
        (
            Verdict::Unknown,
            format!("{msg}; but that end is only a bound: a regulator isn't held and nothing says how far it sags"),
        )
    } else {
        (vd, msg)
    }
}

pub(super) fn value_of(b: &Bound) -> Option<f64> {
    match b {
        Bound::Value(x) => Some(*x),
        _ => None,
    }
}
