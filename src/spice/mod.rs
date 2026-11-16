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
//! What is modelled: resistors, capacitors, inductors and ferrite beads, jumpers and fuses (closed), diodes,
//! LEDs and TVS diodes from their forward drop and breakdown, regulators as ideal sources that follow their
//! supply when it is too low to hold them, and every IC's supply current. IC signal pins are left open, so
//! their internal clamps don't absorb anything: the result is on the pessimistic side there. Parts gerberus
//! can't model are listed, and nets they touch come out UNKNOWN rather than PASS.

mod netlist;
mod run;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Deserialize;

use crate::design::Design;
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, Library, Lookup, Part, Quantity, Range};
use crate::rules::{Finding, Severity, Verdict};
use crate::voltage::Voltages;

pub use netlist::{Circuit, build};
pub use run::{Measured, simulate};

/// `policy.toml`'s `[spice]` table: the cable a board is plugged in through, and the sweep.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpicePolicy {
    /// The cable's inductance range in henries, `[min, max]`: each run draws one.
    pub cable_inductance: [f64; 2],
    /// The cable's resistance in ohms, including the connector's contacts.
    pub cable_resistance: f64,
    /// How fast the supply switches on, 0 to its maximum, in seconds. A hot plug is close to instant.
    pub rise_time: f64,
    /// How long to simulate, in seconds.
    pub duration: f64,
    /// How many Monte Carlo runs, besides the one at nominal values.
    pub runs: usize,
    pub reason: String,
}

impl Default for SpicePolicy {
    fn default() -> SpicePolicy {
        SpicePolicy {
            cable_inductance: [0.1e-6, 1e-6],
            cable_resistance: 0.1,
            rise_time: 1e-6,
            duration: 2e-3,
            runs: 16,
            reason: "a hot plug through about a metre of cable".into(),
        }
    }
}

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
        #[allow(clippy::cast_precision_loss)]
        let f = (x >> 11) as f64 / (1u64 << 53) as f64;
        f
    }

    /// Uniform in `[lo, hi]`.
    pub fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

/// Reads a capacitance such as `100nF`, `2u2`, `4.7µF 16V` or `10p` as farads.
pub fn parse_farads(value: &str) -> Option<f64> {
    parse_si(value, 'F')
}

/// Reads an inductance such as `6.8uH`, `4u7` or `100n` as henries.
pub fn parse_henries(value: &str) -> Option<f64> {
    parse_si(value, 'H')
}

/// A number with an SI prefix, as a value field writes it: the first word that holds a number, with the
/// prefix either after it (`100n`, `100nF`) or in place of the decimal point (`4u7`).
fn parse_si(value: &str, unit: char) -> Option<f64> {
    let word = value
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit() || c == '.'))?;
    let word = word.trim_end_matches(unit).trim_end_matches(unit.to_ascii_lowercase());
    let prefix = |c: char| match c {
        'p' => Some(1e-12),
        'n' => Some(1e-9),
        'u' | 'µ' | 'μ' => Some(1e-6),
        'm' => Some(1e-3),
        _ => None,
    };
    let at = word.char_indices().find(|(_, c)| !(c.is_ascii_digit() || *c == '.'));
    let v = match at {
        None => word.parse::<f64>().ok()?,
        Some((i, c)) => {
            let mult = prefix(c)?;
            let (whole, rest) = (&word[..i], &word[i + c.len_utf8()..]);
            if rest.is_empty() {
                whole.parse::<f64>().ok()? * mult
            } else if !whole.contains('.') && rest.chars().all(|c| c.is_ascii_digit()) {
                format!("{whole}.{rest}").parse::<f64>().ok()? * mult
            } else {
                return None;
            }
        }
    };
    (v.is_finite() && v > 0.0).then_some(v)
}

fn sig(x: f64) -> String {
    // Numerical dust from the solver reads as 0.
    if x.abs() < 1e-6 {
        return "0".into();
    }
    #[allow(clippy::cast_possible_truncation)]
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
struct Worst {
    max: f64,
    max_run: usize,
    min: f64,
    min_run: usize,
    end_max: f64,
}

/// The result of a sweep: every run's parameters, and the worst peak across each pair of nets.
#[derive(Debug, Clone)]
pub struct Sweep {
    pub circuit: Circuit,
    /// Each run's drawn values, for the report.
    pub runs: Vec<String>,
    worst: BTreeMap<(usize, Option<usize>), Worst>,
}

impl Sweep {
    /// The highest and lowest `V(a) − V(b)` any run reached, and the run that reached each.
    pub fn peak(&self, a: usize, b: Option<usize>) -> Option<(f64, usize, f64, usize)> {
        self.worst.get(&(a, b)).map(|w| (w.max, w.max_run, w.min, w.min_run))
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
    let pairs = circuit.pairs(design, lib);
    let mut rng = Rng::new(seed);
    let mut worst: BTreeMap<(usize, Option<usize>), Worst> = BTreeMap::new();
    let mut runs = vec![];
    for run in 0..=policy.runs {
        // Run 0 is nominal, with the largest cable inductance; the rest draw everything.
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
    Ok(Sweep { circuit, runs, worst })
}

/// A part's limit as numbers when both ends are plain values (or one is unknown).
fn numeric(r: &Range) -> Option<(Option<f64>, Option<f64>, Option<u32>)> {
    match r {
        Range::Known { min, max, page, .. } => {
            let n = |b: &Bound| if let Bound::Value(v) = b { Some(*v) } else { None };
            let (lo, hi) = (n(min), n(max));
            (lo.is_some() || hi.is_some()).then_some((lo, hi, *page))
        }
        Range::Unknown(_) => None,
    }
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

/// The `hot-plug` rule: the peak voltage across every pin limit, over every run, against the limit.
pub fn check(design: &Design, lib: &Library, sweep: &Sweep, policy: &SpicePolicy) -> Vec<Finding> {
    let mut out = vec![];
    let net_name = |n: usize| design.nets[n].name.as_str();
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
            let Some(bn) = sweep.circuit.pin_net(c, &part, to) else {
                continue;
            };
            // Passives and connectors are rated on `recommended`; everything else on `abs_max`.
            let (range, kind, severity) = if passive {
                (&limit.recommended, "rating", Severity::Warning)
            } else {
                (&limit.abs_max, "absolute maximum", Severity::Error)
            };
            let Some((lo, hi, page)) = numeric(range) else { continue };
            for pin in &limit.pins {
                let Some(an) = sweep.circuit.pin_net(c, &part, pin) else {
                    continue;
                };
                if an == bn {
                    continue;
                }
                let b = (!sweep.circuit.is_ground(bn)).then_some(bn);
                if sweep.circuit.is_ground(an) && b.is_none() {
                    continue;
                }
                // A net nothing in the simulation drives says nothing about stress.
                if sweep.quiet(an, b) && sweep.circuit.unmodelled_on(an).is_none() {
                    continue;
                }
                let Some((max, max_run, min, min_run)) = sweep.peak(an, b) else {
                    continue;
                };
                let what = match b {
                    None => format!("{reference}.{pin} ({})", net_name(an)),
                    Some(b) => format!("{reference}.{pin} ({}) from {to} ({})", net_name(an), net_name(b)),
                };
                let cite = format!("{}{doc} p{}", part.mpn, page.unwrap_or(0));
                let unknown = sweep
                    .circuit
                    .unmodelled_on(an)
                    .or_else(|| b.and_then(|b| sweep.circuit.unmodelled_on(b)));
                let settled = sweep.settled(an, b).unwrap_or(max);
                let span = format!(
                    "peaks at {} V, settles at {} V, dips to {} V",
                    sig(max),
                    sig(settled),
                    sig(min)
                );
                let (verdict, sev, msg) = if let Some(h) = hi.filter(|&h| max > h) {
                    (
                        Verdict::Fail,
                        severity,
                        format!(
                            "{what}: {span}: {} V is past its {kind} of {} V ({cite}), in run {max_run} ({})",
                            sig(max),
                            sig(h),
                            sweep.runs[max_run]
                        ),
                    )
                } else if let Some(l) = lo.filter(|&l| min < l) {
                    (
                        Verdict::Fail,
                        severity,
                        format!(
                            "{what}: {span}: {} V is past its {kind} minimum of {} V ({cite}), in run {min_run} ({})",
                            sig(min),
                            sig(l),
                            sweep.runs[min_run]
                        ),
                    )
                } else if let Some(u) = unknown {
                    (
                        Verdict::Unknown,
                        severity,
                        format!("{what}: {span}, inside its {kind} ({cite}), but {u} isn't modelled"),
                    )
                } else {
                    (
                        Verdict::Pass,
                        severity,
                        format!("{what}: {span}, inside its {kind} ({cite})"),
                    )
                };
                out.push(Finding {
                    rule: "hot-plug",
                    verdict,
                    severity: sev,
                    reference: reference.clone(),
                    message: format!("{msg}; {}", policy.reason),
                    nets: vec![],
                    case: None,
                });
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
    let quiet = sweep.worst.keys().filter(|&&(a, b)| sweep.quiet(a, b)).count();
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
