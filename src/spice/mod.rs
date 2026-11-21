//! Hot-plug stress in ngspice, with tolerance sweeps.
//!
//! Every supply input in `inputs.toml` is switched on through a cable (its resistance and inductance from
//! `policy.toml`'s `[spice]` table) into the board, and the transient is simulated. A cable's inductance rings
//! with the input capacitors, so a rail can briefly overshoot its steady value, by up to twice in the worst
//! case. Each run draws a fresh set of part values from their tolerances (resistors and capacitors from their
//! part files, regulator outputs over their datasheet range, the cable's inductance over its range), so the
//! worst run shows how far the design can be pushed. The peak voltage across every pin limit is then checked:
//! past an absolute maximum is an error, past a passive's rating a warning.
//!
//! What is modelled: resistors, capacitors, inductors and ferrite beads, jumpers, fuses and net ties (closed),
//! diodes, LEDs and TVS diodes from their forward drop, breakdown and clamp figure, transistor switches (a forward
//! drive) as their channel switched on, step-down regulators as ideal sources that follow their supply when it
//! is too low to hold them, and every IC's supply current. A limit relative to another pin is checked against
//! that pin in the same run. IC signal pins are left open, so their internal clamps don't absorb anything: the
//! result is on the pessimistic side there. Parts gerberus can't model are listed, and nets they touch come out
//! UNKNOWN rather than PASS.
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
pub use run::{Measured, simulate, simulate_all};

/// A small deterministic random number generator (xorshift64*), so a sweep is the same every time.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.max(1))
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let x = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (x >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[lo, hi]`.
    pub fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

fn sig(x: f64) -> String {
    // Numerical dust from the solver reads as 0.
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

/// The worst of all runs for one pair of nets.
#[derive(Debug, Clone, Copy)]
pub struct Worst {
    pub max: f64,
    /// The run that reached `max`.
    pub max_run: usize,
    pub min: f64,
    /// The run that reached `min`.
    pub min_run: usize,
    /// The highest settled value at the end of any run.
    pub end_max: f64,
}

/// The result of a sweep: every run's parameters, and the worst peak across each pair of nets.
#[derive(Debug, Clone)]
pub struct Sweep {
    pub circuit: Circuit,
    /// Each run's drawn values, for the report.
    pub runs: Vec<String>,
    /// The pairs pin limits read, besides the per-net ones added to tell whether a net was powered.
    limits: Vec<Pair>,
    worst: BTreeMap<Pair, Worst>,
}

impl Sweep {
    /// The highest and lowest `V(a) − V(b)` any run reached, and the run that reached each.
    pub fn peak(&self, a: usize, b: Option<usize>) -> Option<Worst> {
        self.worst.get(&(a, b)).copied()
    }

    /// The pairs of nets the pin limits read.
    pub fn limits(&self) -> &[Pair] {
        &self.limits
    }

    /// Whether a pair never moved off 0 V in any run: nothing in the simulation drives it (an IC's signal pin).
    pub fn quiet(&self, a: usize, b: Option<usize>) -> bool {
        self.worst
            .get(&(a, b))
            .is_some_and(|w| w.max.abs() < 1e-3 && w.min.abs() < 1e-3)
    }

    /// The highest settled value at the end of any run.
    pub fn settled(&self, a: usize, b: Option<usize>) -> Option<f64> {
        self.worst.get(&(a, b)).map(|w| w.end_max)
    }
}

/// Builds the circuit and runs the nominal case plus `policy.runs` Monte Carlo runs.
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
    let netlists: Vec<(String, String)> = (0..=policy.runs)
        .map(|run| {
            // Run 0 is nominal, with the largest cable inductance; the rest draw everything.
            let draw = if run == 0 { None } else { Some(&mut rng) };
            circuit.netlist(policy, &pairs, draw)
        })
        .collect();
    let texts: Vec<&str> = netlists.iter().map(|(text, _)| text.as_str()).collect();
    let measured = simulate_all(&texts)?;
    let mut runs = vec![];
    for (run, ((_, about), measured)) in netlists.into_iter().zip(measured).enumerate() {
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

/// What one end of a limit says about the sweep.
enum Side {
    Inside,
    /// The datasheet gives no limit at this end.
    None,
    /// Past the limit: why, and the run that went furthest.
    Past(String, usize),
    Unknown(String),
}

/// `pin + 0.3`, `pin − 0.3` or `pin`, for a bound relative to another pin.
fn relative(pin: &str, plus: f64) -> String {
    if plus.abs() < 1e-9 {
        pin.to_string()
    } else if plus > 0.0 {
        format!("{pin} + {} V", sig(plus))
    } else {
        format!("{pin} − {} V", sig(-plus))
    }
}

/// Where a part's limit is measured, for the checks on each end.
struct At<'a> {
    c: &'a crate::design::Component,
    part: &'a Part,
    a: usize,
    key: Pair,
    kind: &'a str,
}

/// One end of a limit against the sweep: the maximum when `upper`, else the minimum. A bound relative to
/// another pin is checked on the difference to that pin in the same run.
fn side(sweep: &Sweep, at: &At<'_>, bound: &Bound, upper: bool) -> Side {
    let kind = at.kind;
    let Some(Worst {
        max,
        max_run,
        min,
        min_run,
        ..
    }) = sweep.peak(at.key.0, at.key.1)
    else {
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
        Bound::None(_) => Side::None,
        Bound::Unknown(why) => Side::Unknown(format!("its {kind} has no known {end} ({why})")),
        Bound::Pin { pin, plus, at_most } => {
            let Some(rn) = sweep.circuit.pin_net(at.c, at.part, pin) else {
                return Side::Unknown(format!("its {kind} {end} follows {pin}, which isn't on the schematic"));
            };
            let limit = relative(pin, *plus);
            let Worst {
                max: rmax,
                max_run: rmax_run,
                min: rmin,
                min_run: rmin_run,
                ..
            } = match sweep.circuit.key(at.a, rn) {
                // The same net: the difference is always 0.
                None => Worst {
                    max: 0.0,
                    max_run: 0,
                    min: 0.0,
                    min_run: 0,
                    end_max: 0.0,
                },
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

/// A plain-number minimum of a range.
fn value_min(r: &Range) -> Option<f64> {
    match r {
        Range::Known {
            min: Bound::Value(v), ..
        } => Some(*v),
        _ => None,
    }
}

/// The `hot-plug` rule: the peak voltage across every pin limit, over every run, against the limit.
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
            // Passives and connectors are rated on `recommended`; everything else on `abs_max`.
            let (range, kind, severity) = if passive {
                (&limit.recommended, "rating", Severity::Warning)
            } else {
                (&limit.abs_max, "absolute maximum", Severity::Error)
            };
            // A protection part's own stress is its breakdown: past it, it carries current.
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
                let Some(Worst { max, min, .. }) = sweep.peak(an, b) else {
                    finding(
                        Verdict::Unknown,
                        severity,
                        format!("{what}: ngspice didn't measure it ({cite})"),
                        None,
                    );
                    continue;
                };
                // Why the result can't be trusted: a part on the nets isn't modelled, or the run never powered a
                // net the static analysis says is powered (a part in its supply path isn't modelled).
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
                // A net nothing in the simulation drives, and nothing should, says nothing about stress.
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
                    // A protection part's breakdown covers it.
                    Range::Unknown(_) | Range::None(_) if breakdown.is_some() => vec![],
                    Range::None(_) => vec![Side::None],
                    Range::Unknown(why) => vec![Side::Unknown(format!("its {kind} isn't known ({why})"))],
                };
                // The datasheet gives no limit at either end: nothing to report.
                if !sides.is_empty() && sides.iter().all(|s| matches!(s, Side::None)) {
                    continue;
                }
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

/// A summary of what the simulation left out, for the report.
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
