//! The schematic rules. Each compares a net's worst-case voltage (from [`crate::voltage`]) with a part's
//! datasheet limits (from [`crate::limits`]), or checks that something drives a pin at all.
//!
//! Every check comes out PASS, FAIL or UNKNOWN. A FAIL cites the numbers it compared and their datasheet pages;
//! an UNKNOWN says what was missing. Nothing unknown is ever counted as a pass.
mod compare;
mod parts;
mod pins;

use crate::limits::Limit;
use compare::{Other, compare, magnitude, verdict};
use parts::resistor_power;
use pins::{regulator_headroom, undriven_pins};

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde::Serialize;

use crate::design::{Component, Design};
use crate::diag::{Diagnostic, Level, Show};
use crate::finding::{Finding, Severity, Verdict};
use crate::inputs::BoardInputs;
use crate::limits::{Class, Coverage, Library, Lookup, Part, Quantity, Range, coverage};
use crate::policy::Policy;
use crate::voltage::{Span, Voltages};
use parts::{cap_voltage, tvs};

/// The rules, in report order: name and what it checks.
pub const RULES: [(&str, &str); 14] = [
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
    (
        "junction-temperature",
        "every part's junction stays below its maximum, and policy.toml's margin below it, at the hottest ambient",
    ),
    (
        "forward-current",
        "the current through each series diode and LED stays inside its rating and policy.toml's derating",
    ),
    ("unchecked", "every fitted part and pin has limits to check against"),
    (
        "esd",
        "every external connector pin is clamped, or reaches only parts rated to the policy's IEC 61000-4-2 level",
    ),
    (
        "reverse-polarity",
        "every supply input connected backwards stays inside every part's absolute maximum ratings",
    ),
];

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Findings a waiver in `waivers.toml` accepted, taken out of `findings` so nothing counts them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waived: Vec<crate::finding::Waived>,
    /// The file the waivers came from, for the report's note.
    #[serde(skip)]
    pub waivers_file: String,
    /// Each fault case's net voltages, which its findings' `nets` index into.
    #[serde(skip)]
    pub cases: Vec<(String, Voltages)>,
}

impl Report {
    /// Counts findings, leaving out summaries (`reverse-polarity`), whose FAILs are the fault case's own
    /// findings, already counted.
    fn count(&self, verdict: Verdict, severity: Option<Severity>) -> usize {
        self.findings
            .iter()
            .filter(|f| !SUMMARY_RULES.contains(&f.rule))
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

/// Rules whose findings sum up other findings, and so aren't counted again in a report's totals.
pub const SUMMARY_RULES: [&str; 1] = ["reverse-polarity"];

/// The rules a fault case runs: only damage. A fault is checked against absolute maximum ratings, TVS breakdown
/// and capacitor ratings (without derating). Operating ranges of ICs don't apply while a fault lasts.
/// Resistor power and the voltage ratings of other passives are checked too, but a FAIL there becomes UNKNOWN:
/// a fault drives the two ends of a part from ranges that needn't move together, which can overstate the
/// voltage across it.
pub const FAULT_RULES: [&str; 3] = ["abs-max", "tvs-breakdown", "cap-voltage"];

/// Runs every rule on a design, then the damage rules on each fault case in `inputs`.
pub fn check(design: &Design, lib: &Library, policy: &Policy, inputs: &BoardInputs, volts: &Voltages) -> Report {
    let mut r = Report::default();
    let cov = coverage(design, lib);
    part_rules(design, lib, policy, inputs, volts, false, &mut r);
    for (reference, c) in &design.components {
        if c.dnp || !matches!(cov.get(reference), Some(Coverage::Datasheet)) {
            continue;
        }
        let Lookup::Datasheet(part) = lib.lookup(c) else {
            continue;
        };
        for pin in part.uncovered_pins() {
            r.findings.push(Finding {
                rule: "unchecked",
                verdict: Verdict::Unknown,
                severity: Severity::Error,
                reference: reference.clone(),
                message: format!("{reference}.{pin}: {} has no voltage limit for this pin", part.mpn),
                nets: vec![],
                case: None,
                headline: None,
            });
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
                case: None,
                headline: None,
            });
        }
    }
    undriven_pins(design, lib, volts, &mut r);
    regulator_headroom(design, volts, &mut r);

    crate::protection::esd(design, lib, policy, inputs, &mut r.findings);
    let th = crate::thermal::compute(design, lib, inputs, volts);
    crate::thermal::junction(design, lib, policy, inputs, &th, &mut r.findings);
    crate::thermal::forward_current(design, lib, policy, &th, &mut r.findings);

    // Fault cases, including each supply connected backwards. A finding that reads only nets the fault doesn't
    // change says nothing new, so it is left out.
    let all = inputs.with_reversals();
    let reversed: Vec<(String, String)> = inputs
        .reversible()
        .into_iter()
        .map(|i| (BoardInputs::reversed_case(i), i.name.clone()))
        .collect();
    for fault in all.faults() {
        let case_inputs = all.case(design, fault);
        let cv = crate::voltage::compute(design, lib, &case_inputs);
        let mut fr = Report::default();
        part_rules(design, lib, &Policy::default(), &case_inputs, &cv, true, &mut fr);
        if let Some((_, input)) = reversed.iter().find(|(c, _)| c == fault) {
            let summary =
                crate::protection::reverse_polarity(design, lib, inputs, input, fault, &fr.findings, &r.findings, &cv);
            r.findings.push(summary);
        }
        for mut f in fr.findings {
            // The same finding, word for word, in normal operation: the fault changes nothing it read.
            let same = |n: &Finding| {
                n.case.is_none()
                    && n.rule == f.rule
                    && n.verdict == f.verdict
                    && n.reference == f.reference
                    && n.message == f.message
            };
            if r.findings.iter().any(same) {
                continue;
            }
            f.message = format!("fault \"{fault}\": {}", f.message);
            f.case = Some(fault.to_string());
            r.findings.push(f);
        }
        r.cases.push((fault.to_string(), cv));
    }

    r.findings.sort_by(|a, b| {
        (
            a.verdict,
            a.severity,
            rule_index(a.rule),
            &a.case,
            &a.reference,
            &a.message,
        )
            .cmp(&(
                b.verdict,
                b.severity,
                rule_index(b.rule),
                &b.case,
                &b.reference,
                &b.message,
            ))
    });
    r
}

/// The rules that compare each part's limits with its nets. `fault` keeps only [`FAULT_RULES`].
fn part_rules(
    design: &Design,
    lib: &Library,
    policy: &Policy,
    inputs: &BoardInputs,
    volts: &Voltages,
    fault: bool,
    r: &mut Report,
) {
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
        let start = r.findings.len();
        for limit in &part.limits {
            match limit.quantity {
                Quantity::Voltage => voltage_rules(&ctx, policy, limit, r),
                Quantity::Power if part.class == Class::Resistor => resistor_power(&ctx, policy, limit, r),
                _ => {}
            }
        }
        if fault {
            let passive = matches!(part.class, Class::Resistor | Class::Inductor | Class::Connector);
            let mut kept = vec![];
            for mut f in r.findings.drain(start..) {
                if FAULT_RULES.contains(&f.rule) {
                    kept.push(f);
                } else if passive && matches!(f.rule, "recommended" | "resistor-power") {
                    if f.verdict == Verdict::Fail {
                        f.verdict = Verdict::Unknown;
                        f.message += "; but in a fault case the two ends' ranges needn't move together, so this \
                                      may overstate it";
                    }
                    kept.push(f);
                }
            }
            r.findings.extend(kept);
        }
    }
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
        self.diff_why(a, b).0
    }

    /// `V(a) − V(b)` at its worst, and what narrowed it below the two ranges' extremes, for messages.
    ///
    /// Two things can: a `[[differential]]` in inputs.toml, and a net of passive parts (see
    /// [`crate::voltage::NetVoltage::between`]), which at every moment sits between the nets around it.
    fn diff_why(&self, a: usize, b: usize) -> (Span, Vec<String>) {
        let mut notes = vec![];
        let s = self.diff_depth(a, b, 0, &mut notes);
        notes.dedup();
        (s, notes)
    }

    fn diff_depth(&self, a: usize, b: usize, depth: usize, notes: &mut Vec<String>) -> Span {
        if a == b {
            return Span::exact(0.0);
        }
        let (x, y) = (self.span(a), self.span(b));
        let mut s = Span {
            min: x.min.zip(y.max).map(|(p, q)| p - q),
            max: x.max.zip(y.min).map(|(p, q)| p - q),
        };
        let net = |p: &crate::inputs::PinRef| Some(self.design.components.get(&p.reference)?.pins.get(&p.pin)?.net);
        for d in &self.inputs.differential {
            let (Some(p), Some(t)) = (net(&d.pin), net(&d.to)) else {
                continue;
            };
            let c = if (p, t) == (a, b) {
                Span::new(d.min, d.max)
            } else if (p, t) == (b, a) {
                Span::new(-d.max, -d.min)
            } else {
                continue;
            };
            let narrowed = intersect(s, c);
            if narrowed != s {
                notes.push(format!(
                    "{} − {} kept to {} to {} V by inputs.toml ({})",
                    self.net_name(p),
                    self.net_name(t),
                    d.min,
                    d.max,
                    d.source
                ));
            }
            s = narrowed;
        }
        if depth < 4 {
            for (side, n) in [(false, a), (true, b)] {
                let around = &self.volts.nets[n].between;
                if around.is_empty() {
                    continue;
                }
                let mut sub = vec![];
                let hull = around
                    .iter()
                    .map(|&m| {
                        if side {
                            self.diff_depth(a, m, depth + 1, &mut sub)
                        } else {
                            self.diff_depth(m, b, depth + 1, &mut sub)
                        }
                    })
                    .reduce(Span::union)
                    .unwrap_or(s);
                let narrowed = intersect(s, hull);
                if narrowed != s {
                    let names: Vec<&str> = around.iter().map(|&m| self.net_name(m)).collect();
                    notes.push(format!(
                        "{} only connects through passive parts to {}, so it stays between them",
                        self.net_name(n),
                        names.join(" and ")
                    ));
                    notes.extend(sub);
                }
                s = narrowed;
            }
        }
        s
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
            case: None,
            headline: None,
        }
    }
}

/// What to do about a FAIL of each rule, for the report's `help` line.
pub fn help(rule: &str) -> Option<&'static str> {
    Some(match rule {
        "abs-max" => "add a divider, clamp or series resistor, or choose a part rated higher",
        "recommended" => "bring the pin inside its recommended range, or choose a part whose rating covers it",
        "tvs-breakdown" => "choose a TVS whose lowest breakdown voltage is above the rail's highest voltage",
        "tvs-standoff" => "choose a TVS whose stand-off voltage is at or above the rail's highest voltage",
        "cap-voltage" => "use a capacitor rated above the voltage across it",
        "resistor-power" => "use a larger package or a higher power rating, or raise the resistance",
        "floating-input" => "tie the pin to a defined level, directly or through a pull resistor",
        "unpowered" => "connect the pin to a supply, or add what should drive that net",
        "regulator-headroom" => "raise the regulator's input, or use a regulator with less dropout",
        "junction-temperature" => "cut the part's dissipation, or give it a lower thermal resistance",
        "forward-current" => "raise the series resistor until the current is inside the rating",
        "esd" => "add a TVS or ESD clamp next to the connector pin",
        "hot-plug" => {
            "damp the plug-in ringing: a TVS at the input, a bulk electrolytic or RC snubber, or a load switch"
        }
        "reverse-polarity" => "add reverse-polarity protection on the input: a series diode or an ideal-diode MOSFET",
        _ => return None,
    })
}

/// A finding laid out as a diagnostic, with `help` for a FAIL and, for `--verbose`, how each net it read got its
/// range.
pub fn diagnostic(design: &Design, volts: &Voltages, report: &Report, f: &Finding) -> Diagnostic {
    let level = match (f.verdict, f.severity) {
        (Verdict::Fail, Severity::Error) => Level::Error,
        (Verdict::Fail, Severity::Warning) => Level::Warning,
        (Verdict::Unknown, _) => Level::Unknown,
        (Verdict::Pass, _) => Level::Pass,
    };
    let message = match &f.case {
        Some(c) => f
            .message
            .strip_prefix(&format!("fault \"{c}\": "))
            .unwrap_or(&f.message),
        None => &f.message,
    };
    if level == Level::Pass {
        return Diagnostic::line(level, f.rule, &f.message);
    }
    let mut d = Diagnostic::from_message(level, f.rule, &[&f.reference], message);
    if let Some(h) = &f.headline {
        d.headline.clone_from(h);
        d.notes.retain(|n| !h.contains(n.as_str()));
    }
    if let Some(c) = &f.case {
        let at = d.location.take().unwrap_or_else(|| f.reference.clone());
        d.location = Some(format!("{at}, in fault \"{c}\""));
    }
    if f.verdict == Verdict::Fail {
        d.help = help(f.rule).map(str::to_string);
        let volts = f
            .case
            .as_ref()
            .and_then(|c| report.cases.iter().find(|(name, _)| name == c))
            .map_or(volts, |(_, v)| v);
        let mut seen = BTreeSet::new();
        for &n in &f.nets {
            if seen.insert(n) && volts.nets[n].span != Span::exact(0.0) {
                d.details.push((
                    format!("how {} gets {}:", design.nets[n].name, volts.nets[n].span),
                    volts.explain(design, n).lines().map(str::to_string).collect(),
                ));
            }
        }
    }
    d
}

/// The report as text: errors, then warnings, then (if `show` asks) unknowns and passes, then a summary.
pub fn render(design: &Design, volts: &Voltages, report: &Report, show: &Show) -> String {
    let mut out = String::new();
    let width = show.columns();
    let mut details = false;
    // Errors, then warnings, then unknowns and passes: `check` sorts its findings so, but `stress` doesn't.
    let mut sorted: Vec<&Finding> = report.findings.iter().collect();
    sorted.sort_by_key(|f| (f.verdict, f.severity));
    for f in sorted {
        let d = diagnostic(design, volts, report, f);
        if !show.lists(d.level) {
            continue;
        }
        details |= !d.details.is_empty();
        d.render(&mut out, show.style, show.verbose, width);
    }
    if show.all {
        for w in &report.waived {
            let d = Diagnostic::line(
                Level::Waived,
                w.finding.rule,
                &format!("{} (waived: {})", w.finding.message, w.reason),
            );
            d.render(&mut out, show.style, show.verbose, width);
        }
    }
    crate::diag::summary(
        &mut out,
        show,
        [report.errors(), report.warnings(), report.unknowns(), report.passes()],
        details,
    );
    if !report.waived.is_empty() {
        let _ = writeln!(
            out,
            "{} {} {}",
            show.style.paint(crate::diag::BLUE, "="),
            show.style.paint(crate::diag::BOLD, "note:"),
            format_args!(
                "{} waived in {}, not counted{}",
                report.waived.len(),
                report.waivers_file,
                if show.all { "" } else { " (--all lists them)" }
            )
        );
    }
    out
}

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

fn voltage_rules(ctx: &Ctx<'_>, policy: &Policy, limit: &Limit, r: &mut Report) {
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
