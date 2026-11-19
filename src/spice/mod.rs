mod netlist;
mod run;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::policy::SpicePolicy;

use crate::design::Design;
use crate::finding::{Finding, Severity, Verdict};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, Library, Lookup, Part, Quantity, Range};
use crate::voltage::Voltages;

pub use netlist::{Circuit, Pair, SWITCH_OHMS, TVS_OHMS, build};
pub use run::{Measured, simulate};

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

fn sig(x: f64) -> String {
    if x.abs() < 1e-6 {
        return "0".into();
    }
    let digits = (3 - x.abs().log10().floor() as i32).max(0);
    let s = format!("{x:.*}", usize::try_from(digits).unwrap_or(0));
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[derive(Debug, Clone, Copy)]
struct Worst {
    max: f64,
    max_run: usize,
    min: f64,
    min_run: usize,
    end_max: f64,
}

#[derive(Debug, Clone)]
pub struct Sweep {
    pub circuit: Circuit,
    pub runs: Vec<String>,
    limits: Vec<Pair>,
    worst: BTreeMap<Pair, Worst>,
}

impl Sweep {
    pub fn peak(&self, a: usize, b: Option<usize>) -> Option<(f64, usize, f64, usize)> {
        self.worst.get(&(a, b)).map(|w| (w.max, w.max_run, w.min, w.min_run))
    }

    pub fn limits(&self) -> &[Pair] {
        &self.limits
    }

    pub fn quiet(&self, a: usize, b: Option<usize>) -> bool {
        self.worst
            .get(&(a, b))
            .is_some_and(|w| w.max.abs() < 1e-3 && w.min.abs() < 1e-3)
    }

    pub fn settled(&self, a: usize, b: Option<usize>) -> Option<f64> {
        self.worst.get(&(a, b)).map(|w| w.end_max)
    }
}

pub fn sweep(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    volts: &Voltages,
    policy: &SpicePolicy,
    seed: u64,
) -> Result<Sweep, String> {
    let circuit = build(design, lib, inputs, volts)?;
    let limits = circuit.pairs(design, lib);
    let pairs = circuit.with_nets(&limits);
    let mut rng = Rng::new(seed);
    let mut worst: BTreeMap<Pair, Worst> = BTreeMap::new();
    let mut runs = vec![];
    for run in 0..=policy.runs {
        let draw = if run == 0 { None } else { Some(&mut rng) };
        let (text, about) = circuit.netlist(policy, &pairs, draw);
        let measured = simulate(&text)?;
        runs.push(about);
        for (i, &(a, b)) in pairs.iter().enumerate() {
            let Some(m) = measured.pair(i) else { continue };
            let w = worst.entry((a, b)).or_insert(Worst {
                max: f64::NEG_INFINITY,
                max_run: 0,
                min: f64::INFINITY,
                min_run: 0,
                end_max: f64::NEG_INFINITY,
            });
            if m.max > w.max {
                w.max = m.max;
                w.max_run = run;
            }
            if m.min < w.min {
                w.min = m.min;
                w.min_run = run;
            }
            w.end_max = w.end_max.max(m.end);
        }
    }
    Ok(Sweep {
        circuit,
        runs,
        limits,
        worst,
    })
}

fn part_of<'a>(lib: &'a Library, c: &crate::design::Component) -> Option<std::borrow::Cow<'a, Part>> {
    if c.dnp {
        return None;
    }
    match lib.lookup(c) {
        Lookup::Datasheet(p) => Some(std::borrow::Cow::Borrowed(p)),
        Lookup::ClassDefault(p) => Some(std::borrow::Cow::Owned(*p)),
        Lookup::Unchecked(_) => None,
    }
}

enum Side {
    Inside,
    Past(String, usize),
    Unknown(String),
}

fn relative(pin: &str, plus: f64) -> String {
    if plus.abs() < 1e-9 {
        pin.to_string()
    } else if plus > 0.0 {
        format!("{pin} + {} V", sig(plus))
    } else {
        format!("{pin} − {} V", sig(-plus))
    }
}

struct At<'a> {
    c: &'a crate::design::Component,
    part: &'a Part,
    a: usize,
    key: Pair,
    kind: &'a str,
}

fn side(sweep: &Sweep, at: &At<'_>, bound: &Bound, upper: bool) -> Side {
    let kind = at.kind;
    let Some((max, max_run, min, min_run)) = sweep.peak(at.key.0, at.key.1) else {
        return Side::Unknown("ngspice didn't measure it".into());
    };
    let end = if upper { "maximum" } else { "minimum" };
    match bound {
        Bound::Value(v) if upper && max > *v => {
            Side::Past(format!("{} V is past its {kind} of {} V", sig(max), sig(*v)), max_run)
        }
        Bound::Value(v) if !upper && min < *v => Side::Past(
            format!("{} V is past its {kind} minimum of {} V", sig(min), sig(*v)),
            min_run,
        ),
        Bound::Value(_) => Side::Inside,
        Bound::Unknown(why) => Side::Unknown(format!("its {kind} has no known {end} ({why})")),
        Bound::Pin { pin, plus, at_most } => {
            let Some(rn) = sweep.circuit.pin_net(at.c, at.part, pin) else {
                return Side::Unknown(format!("its {kind} {end} follows {pin}, which isn't on the schematic"));
            };
            let limit = relative(pin, *plus);
            let (rmax, rmax_run, rmin, rmin_run) = match sweep.circuit.key(at.a, rn) {
                None => (0.0, 0, 0.0, 0),
                Some((x, y)) => match sweep.peak(x, y) {
                    Some(p) => p,
                    None => return Side::Unknown(format!("ngspice didn't measure it against {pin}")),
                },
            };
            if upper && rmax > *plus {
                return Side::Past(
                    format!("it goes {} V above {pin}, past its {kind} of {limit}", sig(rmax)),
                    rmax_run,
                );
            }
            if !upper && rmin < *plus {
                return Side::Past(
                    format!(
                        "it goes to {} V from {pin}, past its {kind} minimum of {limit}",
                        sig(rmin)
                    ),
                    rmin_run,
                );
            }
            match at_most {
                Some(m) if upper && max > *m => Side::Past(
                    format!("{} V is past its {kind} cap of {} V", sig(max), sig(*m)),
                    max_run,
                ),
                _ => Side::Inside,
            }
        }
    }
}

fn value_min(r: &Range) -> Option<f64> {
    match r {
        Range::Known {
            min: Bound::Value(v), ..
        } => Some(*v),
        _ => None,
    }
}

pub fn check(design: &Design, lib: &Library, sweep: &Sweep, policy: &SpicePolicy) -> Vec<Finding> {
    let mut out = vec![];
    let net_name = |n: usize| design.nets[n].name.as_str();
    let circuit = &sweep.circuit;
    for (reference, c) in &design.components {
        let Some(part) = part_of(lib, c) else { continue };
        let passive = matches!(
            part.class,
            Class::Capacitor | Class::Resistor | Class::Inductor | Class::Connector
        );
        let doc = part
            .datasheet
            .as_ref()
            .map_or(String::new(), |d| format!(" {}", d.document));
        for limit in part.limits.iter().filter(|l| l.quantity == Quantity::Voltage) {
            let Some(to) = limit.to.as_deref() else { continue };
            let Some(bn) = circuit.pin_net(c, &part, to) else {
                continue;
            };
            let (range, kind, severity) = if passive {
                (&limit.recommended, "rating", Severity::Warning)
            } else {
                (&limit.abs_max, "absolute maximum", Severity::Error)
            };
            let breakdown = (part.class == Class::Tvs)
                .then(|| limit.breakdown.as_ref().and_then(value_min))
                .flatten();
            let cite_of = |r: Option<&Range>| match r {
                Some(Range::Known { page: Some(p), .. }) => format!("{}{doc} p{p}", part.mpn),
                _ => format!("{}{doc}", part.mpn),
            };
            let cite = cite_of(Some(range));
            for pin in &limit.pins {
                let Some(an) = circuit.pin_net(c, &part, pin) else {
                    continue;
                };
                let Some(key) = circuit.key(an, bn) else { continue };
                let b = key.1;
                let what = match b {
                    None => format!("{reference}.{pin} ({})", net_name(an)),
                    Some(b) => format!("{reference}.{pin} ({}) from {to} ({})", net_name(an), net_name(b)),
                };
                let mut finding = |verdict, severity, msg: String, headline: Option<String>| {
                    out.push(Finding {
                        rule: "hot-plug",
                        verdict,
                        severity,
                        reference: reference.clone(),
                        message: format!("{msg}; {}", policy.reason),
                        nets: vec![],
                        case: None,
                        headline,
                    });
                };
                let Some((max, _, min, _)) = sweep.peak(an, b) else {
                    finding(
                        Verdict::Unknown,
                        severity,
                        format!("{what}: ngspice didn't measure it ({cite})"),
                        None,
                    );
                    continue;
                };
                let mut unsure: Vec<String> = vec![];
                for n in std::iter::once(an).chain(b) {
                    if let Some(u) = circuit.unmodelled_on(n) {
                        unsure.push(format!("{u} isn't modelled"));
                    }
                    let reached = sweep.settled(n, None).unwrap_or(0.0);
                    if let Some(e) = circuit.expected(n).filter(|&e| e > 0.5 && reached < e / 2.0) {
                        unsure.push(format!(
                            "the simulation leaves {} at {} V where the voltage analysis gives it at least {} V, so \
                             something feeding it isn't modelled",
                            net_name(n),
                            sig(reached),
                            sig(e)
                        ));
                    }
                }
                if sweep.quiet(an, b) && unsure.is_empty() {
                    continue;
                }
                let settled = sweep.settled(an, b).unwrap_or(max);
                let span = format!(
                    "peaks at {} V, settles at {} V, dips to {} V",
                    sig(max),
                    sig(settled),
                    sig(min)
                );
                if let Some(bmin) = breakdown.filter(|&v| settled >= v) {
                    finding(
                        Verdict::Fail,
                        Severity::Error,
                        format!(
                            "{what}: {span}: it settles at or past its breakdown minimum of {} V ({}), so it keeps \
                             conducting after the plug-in, which a part rated for pulses won't survive",
                            sig(bmin),
                            cite_of(limit.breakdown.as_ref())
                        ),
                        Some(format!(
                            "{reference}.{pin} settles at {} V after hot plug, at or past its {} V breakdown minimum",
                            sig(settled),
                            sig(bmin)
                        )),
                    );
                    continue;
                }
                let at = At {
                    c,
                    part: &part,
                    a: an,
                    key,
                    kind,
                };
                let sides = match range {
                    Range::Known { min, max, .. } => vec![side(sweep, &at, max, true), side(sweep, &at, min, false)],
                    Range::Unknown(_) if breakdown.is_some() => vec![],
                    Range::Unknown(why) => vec![Side::Unknown(format!("its {kind} isn't known ({why})"))],
                };
                if let Some((why, run)) = sides.iter().find_map(|s| match s {
                    Side::Past(why, run) => Some((why, *run)),
                    _ => None,
                }) {
                    finding(
                        Verdict::Fail,
                        severity,
                        format!("{what}: {span}: {why} ({cite}), in run {run} ({})", sweep.runs[run]),
                        Some(format!("{reference}.{pin} on hot plug: {why}")),
                    );
                    continue;
                }
                unsure.extend(sides.into_iter().filter_map(|s| match s {
                    Side::Unknown(why) => Some(why),
                    _ => None,
                }));
                let inside = match breakdown {
                    Some(v) => format!("below its breakdown of {} V", sig(v)),
                    None => format!("inside its {kind}"),
                };
                if unsure.is_empty() {
                    finding(
                        Verdict::Pass,
                        severity,
                        format!("{what}: {span}, {inside} ({cite})"),
                        None,
                    );
                } else {
                    finding(
                        Verdict::Unknown,
                        severity,
                        format!("{what}: {span} ({cite}), but {}", unsure.join("; ")),
                        None,
                    );
                }
            }
        }
    }
    out
}

pub fn describe(design: &Design, sweep: &Sweep) -> String {
    let mut out = String::new();
    let skipped = sweep.circuit.skipped();
    if !skipped.is_empty() {
        let _ = writeln!(out, "not modelled: {}", skipped.join("; "));
    }
    let quiet = sweep.limits.iter().filter(|&&(a, b)| sweep.quiet(a, b)).count();
    if quiet > 0 {
        let _ = writeln!(
            out,
            "{quiet} pin limits are on nets nothing in the simulation drives (IC signal pins), so they aren't reported"
        );
    }
    let _ = writeln!(
        out,
        "{} runs; run 0 is nominal. Inputs: {}",
        sweep.runs.len(),
        sweep.circuit.inputs(design).join(", ")
    );
    out
}
