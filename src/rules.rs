//! The first ten schematic rules. Each compares a net's worst-case voltage (from [`crate::voltage`]) with a part's
//! datasheet limits (from [`crate::limits`]), or checks that something drives a pin at all.
//!
//! Every check comes out PASS, FAIL or UNKNOWN. A FAIL cites the numbers it compared and their datasheet pages;
//! an UNKNOWN says what was missing. Nothing unknown is ever counted as a pass.

use std::collections::BTreeSet;
use std::fmt;

use serde::Serialize;

use crate::design::{Component, Design, PinKind};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, Coverage, Library, Limit, Lookup, Part, Quantity, Range, coverage};
use crate::policy::Policy;
use crate::voltage::{Driver, Hold, Span, Voltages, resistance_range};

/// The rules, in report order: name and what it checks.
pub const RULES: [(&str, &str); 10] = [
    ("abs-max", "every pin stays inside its absolute maximum ratings"),
    (
        "recommended",
        "every pin stays inside its recommended operating range or rating",
    ),
    (
        "tvs-breakdown",
        "no rail reaches a TVS or ESD diode's breakdown voltage, where it conducts",
    ),
    ("tvs-standoff", "no rail rises above a TVS diode's stand-off voltage"),
    (
        "cap-voltage",
        "no capacitor sees more than its rated voltage, or more than the derating policy allows",
    ),
    (
        "resistor-power",
        "no resistor dissipates more than its rated power at the worst-case voltage across it",
    ),
    ("floating-input", "every input pin has something driving it"),
    ("unpowered", "every power pin is on a net something supplies"),
    (
        "regulator-headroom",
        "every regulator's supply stays high enough for it to hold its output",
    ),
    ("unchecked", "every fitted part and pin has limits to check against"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Fail,
    Unknown,
    Pass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Verdict::Fail => "FAIL",
            Verdict::Unknown => "UNKNOWN",
            Verdict::Pass => "PASS",
        })
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub verdict: Verdict,
    /// How bad a FAIL is. An UNKNOWN or PASS carries the severity its FAIL would have had.
    pub severity: Severity,
    /// Reference designator, such as `D2`.
    pub reference: String,
    /// What was compared: the pin, its net, the numbers and their datasheet pages.
    pub message: String,
    /// Nets whose ranges the finding read, so a report can explain them.
    #[serde(skip)]
    pub nets: Vec<usize>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    fn count(&self, verdict: Verdict, severity: Option<Severity>) -> usize {
        self.findings
            .iter()
            .filter(|f| f.verdict == verdict && severity.is_none_or(|s| f.severity == s))
            .count()
    }

    /// FAILs that are errors: what makes a board not ready to order.
    pub fn errors(&self) -> usize {
        self.count(Verdict::Fail, Some(Severity::Error))
    }

    pub fn warnings(&self) -> usize {
        self.count(Verdict::Fail, Some(Severity::Warning))
    }

    pub fn unknowns(&self) -> usize {
        self.count(Verdict::Unknown, None)
    }

    pub fn passes(&self) -> usize {
        self.count(Verdict::Pass, None)
    }

    pub fn by_rule<'a>(&'a self, rule: &'a str) -> impl Iterator<Item = &'a Finding> + 'a {
        self.findings.iter().filter(move |f| f.rule == rule)
    }
}

/// Runs every rule on a design.
pub fn check(design: &Design, lib: &Library, policy: &Policy, inputs: &BoardInputs, volts: &Voltages) -> Report {
    let mut r = Report::default();
    let cov = coverage(design, lib);
    for (reference, c) in &design.components {
        if c.dnp {
            continue;
        }
        let part = match lib.lookup(c) {
            Lookup::Datasheet(p) => std::borrow::Cow::Borrowed(p),
            Lookup::ClassDefault(p) => std::borrow::Cow::Owned(*p),
            Lookup::Unchecked(_) => continue,
        };
        let ctx = Ctx {
            design,
            volts,
            inputs,
            reference,
            c,
            part: &part,
        };
        for limit in &part.limits {
            match limit.quantity {
                Quantity::Voltage => voltage_rules(&ctx, policy, limit, &mut r),
                Quantity::Power if part.class == Class::Resistor => resistor_power(&ctx, policy, limit, &mut r),
                _ => {}
            }
        }
        if matches!(cov.get(reference), Some(Coverage::Datasheet { .. })) {
            for pin in part.uncovered_pins() {
                r.findings.push(Finding {
                    rule: "unchecked",
                    verdict: Verdict::Unknown,
                    severity: Severity::Error,
                    reference: reference.clone(),
                    message: format!("{reference}.{pin}: {} has no voltage limit for this pin", part.mpn),
                    nets: vec![],
                });
            }
        }
    }
    for (reference, cv) in &cov {
        if let Coverage::Unchecked { reason } = cv {
            r.findings.push(Finding {
                rule: "unchecked",
                verdict: Verdict::Unknown,
                severity: Severity::Error,
                reference: reference.clone(),
                message: format!("{reference}: nothing to check its pins against ({reason})"),
                nets: vec![],
            });
        }
    }
    undriven_pins(design, lib, volts, &mut r);
    regulator_headroom(design, volts, &mut r);
    r.findings.sort_by(|a, b| {
        (a.verdict, a.severity, rule_index(a.rule), &a.reference, &a.message).cmp(&(
            b.verdict,
            b.severity,
            rule_index(b.rule),
            &b.reference,
            &b.message,
        ))
    });
    r
}

fn rule_index(rule: &str) -> usize {
    RULES.iter().position(|(n, _)| *n == rule).unwrap_or(RULES.len())
}

struct Ctx<'a> {
    design: &'a Design,
    volts: &'a Voltages,
    inputs: &'a BoardInputs,
    reference: &'a str,
    c: &'a Component,
    part: &'a Part,
}

impl Ctx<'_> {
    /// The net a part-file pin name is on.
    fn net(&self, name: &str) -> Option<usize> {
        self.part
            .pins_named(name)
            .into_iter()
            .find_map(|num| self.c.pins.get(num).map(|p| p.net))
    }

    fn span(&self, net: usize) -> Span {
        self.volts.nets[net].span
    }

    fn net_name(&self, net: usize) -> &str {
        &self.design.nets[net].name
    }

    /// `V(a) − V(b)` at its worst. Two pins on one net are always 0 V apart.
    fn diff(&self, a: usize, b: usize) -> Span {
        if a == b {
            return Span::exact(0.0);
        }
        let (x, y) = (self.span(a), self.span(b));
        Span {
            min: x.min.zip(y.max).map(|(p, q)| p - q),
            max: x.max.zip(y.min).map(|(p, q)| p - q),
        }
    }

    /// Where a pin is and what it sees, for messages: `U1.VIN (+5V)`.
    fn at(&self, name: &str, net: usize) -> String {
        format!("{}.{name} ({})", self.reference, self.net_name(net))
    }

    fn cite(&self, range: &Range) -> String {
        let doc = self.part.datasheet.as_ref().map_or("", |d| d.document.as_str());
        match range {
            Range::Known {
                page: Some(p), note, ..
            } => {
                let note = note.as_deref().map(|n| format!(", {n}")).unwrap_or_default();
                format!("{} {doc} p{p}{note}", self.part.mpn)
            }
            Range::Known { note, .. } => {
                format!(
                    "{}{}",
                    self.part.mpn,
                    note.as_deref().map(|n| format!(", {n}")).unwrap_or_default()
                )
            }
            Range::Unknown(_) => self.part.mpn.clone(),
        }
    }

    fn finding(
        &self,
        rule: &'static str,
        verdict: Verdict,
        severity: Severity,
        message: String,
        nets: Vec<usize>,
    ) -> Finding {
        Finding {
            rule,
            verdict,
            severity,
            reference: self.reference.to_string(),
            message,
            nets,
        }
    }
}

/// The result of comparing a voltage with one end of a range.
enum End {
    Ok,
    Fail(String),
    Unknown(String),
}

fn v(x: f64) -> String {
    format!("{} V", (x * 1000.0).round() / 1000.0)
}

/// Compares the voltage `seen` (the pin from the limit's `to`) with a range. `other` gives the voltage of
/// another pin of the part, from the same `to`, for a bound like `{ pin = "VDD", plus = 0.3 }`; `same` says
/// whether that pin is on the checked pin's net, so the two move together.
/// `floor` says which ends of `seen` are only floors (low, high): a FAIL there becomes UNKNOWN.
fn compare(seen: Span, floor: (bool, bool), range: &Range, other: &dyn Fn(&str) -> Option<Other>) -> Vec<End> {
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
struct Other {
    span: Span,
    /// On the checked pin's net.
    same: bool,
    /// The checked pin's net can never rise above it.
    below: bool,
}

fn bound(seen: Option<f64>, b: &Bound, other: &dyn Fn(&str) -> Option<Other>, is_max: bool) -> End {
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
fn verdict(ends: &[End]) -> (Verdict, String) {
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
struct Seen {
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
fn seen(ctx: &Ctx, limit: &Limit) -> Vec<Seen> {
    let mut out = vec![];
    match &limit.to {
        Some(to) => {
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
        }
        None => {
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
    }
    out
}

/// The size of a voltage of either sign.
fn magnitude(s: Span) -> Span {
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

fn voltage_rules(ctx: &Ctx, policy: &Policy, limit: &Limit, r: &mut Report) {
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
fn magnitude_floor(s: Span, floor: (bool, bool)) -> bool {
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
fn unless_floor(vd: Verdict, msg: String, on_floor: bool) -> (Verdict, String) {
    if vd == Verdict::Fail && on_floor {
        (
            Verdict::Unknown,
            format!("{msg}; but that end is only a bound: a regulator isn't held and nothing says how far it sags"),
        )
    } else {
        (vd, msg)
    }
}

fn value_of(b: &Bound) -> Option<f64> {
    match b {
        Bound::Value(x) => Some(*x),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn tvs(ctx: &Ctx, limit: &Limit, span: Span, floor: (bool, bool), head: &str, nets: &[usize], r: &mut Report) {
    // Breakdown: a rail that can sit at or above the lowest breakdown voltage drives current through the part
    // continuously, and it burns. A bidirectional part breaks down either way, so its magnitude counts.
    if let Some(bd) = &limit.breakdown {
        let lowest = match bd {
            Range::Known { min, .. } => value_of(min),
            Range::Unknown(_) => None,
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
                    "{head}: reaches {}, at or above the lowest breakdown voltage of {} ({}), so it conducts \
                     continuously and overheats",
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
        Range::Unknown(_) => (None, None),
    };
    let src = ctx.cite(&limit.recommended);
    let mut fails = vec![];
    let mut unknown = vec![];
    match (span.max, hi) {
        (Some(x), Some(s)) if x > s + 1e-9 => {
            fails.push((format!("reaches {}, above the stand-off of {}", v(x), v(s)), floor.1))
        }
        (Some(_), Some(_)) => {}
        (None, _) => unknown.push("the net's maximum is UNKNOWN".to_string()),
        (_, None) => unknown.push("the datasheet gives no stand-off voltage".to_string()),
    }
    if limit.bidirectional {
        match (span.min, lo) {
            (Some(x), Some(s)) if x < s - 1e-9 => {
                fails.push((format!("falls to {}, below the stand-off of {}", v(x), v(s)), floor.0))
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
#[allow(clippy::too_many_arguments)]
fn cap_voltage(
    ctx: &Ctx,
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
        Range::Unknown(_) => (None, None),
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
                    "{} across it, past {} % of its {} rating ({source}) that policy.toml allows: {reason}",
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

fn resistor_power(ctx: &Ctx, policy: &Policy, limit: &Limit, r: &mut Report) {
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
    let diff = ctx.diff(a, b);
    let across = magnitude(diff);
    let on_floor = magnitude_floor(diff, (ctx.volts.nets[a].floor, ctx.volts.nets[b].floor));
    let head = format!(
        "{} ({} to {}): {} across it",
        ctx.reference,
        ctx.net_name(a),
        ctx.net_name(b),
        across
    );
    let rated = match &limit.recommended {
        Range::Known { max, .. } => value_of(max),
        Range::Unknown(_) => None,
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
            let detail = format!(
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
                        "{head}: up to {detail}, above its {} rating ({})",
                        w(p),
                        ctx.cite(&limit.recommended)
                    ),
                )
            } else if let Some(d) = derate.filter(|d| watts > p * d.factor) {
                (
                    Verdict::Fail,
                    Severity::Warning,
                    format!(
                        "{head}: up to {detail}, above {} % of its {} rating that policy.toml allows: {}",
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
                        "{head}: up to {detail}, within its {} rating ({})",
                        w(p),
                        ctx.cite(&limit.recommended)
                    ),
                )
            }
        }
    };
    let (vd, msg) = unless_floor(vd, msg, on_floor);
    r.findings
        .push(ctx.finding("resistor-power", vd, severity, msg, vec![a, b]));
}

/// The smallest the resistor can be, from its tolerance and temperature drift, and where that comes from.
fn smallest_resistance(ctx: &Ctx) -> Result<(f64, String), String> {
    let mut lib = Library::default();
    lib.parts
        .insert(crate::limits::component_mpn(ctx.c).to_string(), ctx.part.clone());
    resistance_range(&lib, ctx.inputs, ctx.reference, ctx.c).map(|(lo, _, note)| (lo, note))
}

/// Input and power pins on nets nothing drives.
fn undriven_pins(design: &Design, lib: &Library, volts: &Voltages, r: &mut Report) {
    for (reference, c) in &design.components {
        if c.dnp || c.lib_id.starts_with("Connector") || lib.part_for(c).is_some_and(|p| p.class == Class::Connector) {
            continue;
        }
        for pin in c.pins.values() {
            if pin.no_connect {
                continue;
            }
            let rule = match pin.kind {
                PinKind::Input => "floating-input",
                PinKind::PowerIn => "unpowered",
                _ => continue,
            };
            let nv = &volts.nets[pin.net];
            let net = &design.nets[pin.net].name;
            let name = if pin.name.is_empty() {
                pin.number.clone()
            } else {
                format!("{} ({})", pin.number, pin.name)
            };
            let (vd, msg) = match nv.driver {
                Driver::Floating => (
                    Verdict::Fail,
                    format!(
                        "{reference}.{name} is on {net}, and nothing drives that net: {}",
                        nv.why[0].text
                    ),
                ),
                Driver::Weak => (
                    Verdict::Unknown,
                    format!(
                        "{reference}.{name} is on {net}, which only open-drain or tri-state outputs, connector pins \
                         or parts gerberus can't model reach, so nothing proves it is driven"
                    ),
                ),
                Driver::Strong | Driver::Pulled => (
                    Verdict::Pass,
                    format!("{reference}.{name} is on {net}, which is driven ({})", nv.span),
                ),
            };
            r.findings.push(Finding {
                rule,
                verdict: vd,
                severity: Severity::Error,
                reference: reference.clone(),
                message: msg,
                nets: vec![pin.net],
            });
        }
    }
}

fn regulator_headroom(design: &Design, volts: &Voltages, r: &mut Report) {
    let mut seen = BTreeSet::new();
    for reg in volts.regulators.iter().filter(|r| r.sets_output) {
        if !seen.insert((&reg.reference, &reg.pin)) {
            continue;
        }
        let out = &design.nets[reg.output].name;
        let (vd, text) = match &reg.hold {
            Hold::Held(t) => (Verdict::Pass, t),
            Hold::NotHeld(t) => (Verdict::Fail, t),
            Hold::Unknown(t) => (Verdict::Unknown, t),
        };
        r.findings.push(Finding {
            rule: "regulator-headroom",
            verdict: vd,
            severity: Severity::Warning,
            reference: reg.reference.clone(),
            message: format!("{} ({}) sets {out}: {text}", reg.reference, reg.mpn),
            nets: vec![reg.supply],
        });
    }
}

/// The report as text: failures first with how their nets got their ranges, then unknowns, then a summary.
/// `all` also lists passes.
pub fn render(design: &Design, volts: &Voltages, report: &Report, all: bool) -> String {
    let mut out = String::new();
    for f in &report.findings {
        if f.verdict == Verdict::Pass && !all {
            continue;
        }
        let label = match f.verdict {
            Verdict::Fail => format!("FAIL {}", f.severity),
            other => other.to_string(),
        };
        out += &format!("{label:<13} {:<18} {}\n", f.rule, f.message);
        if f.verdict == Verdict::Fail {
            let mut seen = BTreeSet::new();
            for &n in &f.nets {
                if seen.insert(n) && volts.nets[n].span != Span::exact(0.0) {
                    for line in volts.explain(design, n).lines() {
                        out += &format!("{:<32} {line}\n", "");
                    }
                }
            }
        }
    }
    out += &format!(
        "\n{} errors, {} warnings, {} unknown, {} passed\n",
        report.errors(),
        report.warnings(),
        report.unknowns(),
        report.passes()
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other(span: Span, same: bool, below: bool) -> impl Fn(&str) -> Option<Other> {
        move |_| Some(Other { span, same, below })
    }

    fn pin_bound(plus: f64, at_most: Option<f64>) -> Bound {
        Bound::Pin {
            pin: "VDD".into(),
            plus,
            at_most,
        }
    }

    fn is(e: &End) -> &'static str {
        match e {
            End::Ok => "ok",
            End::Fail(_) => "fail",
            End::Unknown(_) => "unknown",
        }
    }

    /// `VDD + 0.3 V` with VDD at 3.0 to 3.6 V: 3.2 V always passes, 4.0 V always fails, 3.5 V depends on how
    /// the two move.
    #[test]
    fn relative_bounds() {
        let vdd = other(Span::new(3.0, 3.6), false, false);
        let b = pin_bound(0.3, None);
        assert_eq!(is(&bound(Some(3.2), &b, &vdd, true)), "ok");
        assert_eq!(is(&bound(Some(4.0), &b, &vdd, true)), "fail");
        assert_eq!(is(&bound(Some(3.5), &b, &vdd, true)), "unknown");
        // Bounded by VDD itself: passes even at 3.5 V.
        let tracks = other(Span::new(3.0, 3.6), false, true);
        assert_eq!(is(&bound(Some(3.5), &b, &tracks, true)), "ok");
        // `at_most` caps it: min(VDD + 4, 5.5) fails at 5.6 V even when bounded by VDD.
        let capped = pin_bound(4.0, Some(5.5));
        assert_eq!(is(&bound(Some(5.6), &capped, &tracks, true)), "fail");
        // With VDD from 1.0 V the cap is between 5.0 and 5.5 V.
        let ft = other(Span::new(1.0, 3.6), false, false);
        assert_eq!(is(&bound(Some(5.6), &capped, &ft, true)), "fail");
        assert_eq!(is(&bound(Some(5.2), &capped, &ft, true)), "unknown");
        assert_eq!(is(&bound(Some(4.9), &capped, &ft, true)), "ok");
    }

    /// On the same net, only `plus` matters: `VDD − 0.3 V` as a maximum can never hold.
    #[test]
    fn same_net() {
        let same = other(Span::new(3.0, 3.6), true, true);
        assert_eq!(is(&bound(Some(3.6), &pin_bound(0.3, None), &same, true)), "ok");
        assert_eq!(is(&bound(Some(3.6), &pin_bound(-0.3, None), &same, true)), "fail");
    }

    /// An unknown net against a limit the datasheet doesn't give is UNKNOWN, never a pass.
    #[test]
    fn unknown_against_unknown() {
        let none = |_: &str| None;
        assert_eq!(is(&bound(None, &Bound::Unknown("x".into()), &none, true)), "unknown");
    }

    /// A FAIL on an end that is only a floor becomes UNKNOWN; the other end still fails.
    #[test]
    fn floors_soften_their_own_end() {
        let none = |_: &str| None;
        let range = Range::Known {
            min: Bound::Value(1.7),
            max: Bound::Value(3.3),
            page: Some(1),
            note: None,
        };
        let ends = compare(Span::new(0.0, 3.5), (true, false), &range, &none);
        assert_eq!(ends.iter().map(is).collect::<Vec<_>>(), ["unknown", "fail"]);
        assert_eq!(verdict(&ends).0, Verdict::Fail);
        let ends = compare(Span::new(0.0, 3.2), (true, false), &range, &none);
        assert_eq!(verdict(&ends).0, Verdict::Unknown);
    }

    /// Magnitude takes its floor from the end it comes from.
    #[test]
    fn magnitude_floors() {
        assert!(!magnitude_floor(Span::new(0.0, 3.3), (true, false)));
        assert!(magnitude_floor(Span::new(-5.0, 3.3), (true, false)));
    }
}
