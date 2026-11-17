//! Protection on a board's external connectors: ESD clamping, and what a supply connected backwards does.
//!
//! The external connector pins are the ones `inputs.toml` describes: every input and undriven pin. Nothing
//! else is guessed to be external.

use std::collections::{BTreeMap, BTreeSet};

use crate::design::{Design, PinKind};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, DriveKind, EsdModel, Library, Lookup, Part, Quantity, Range};
use crate::policy::Policy;
use crate::rules::{Finding, Severity, Verdict};
use crate::voltage::Voltages;

/// The part file of a fitted component, if it has one.
fn part_of<'a>(design: &Design, lib: &'a Library, reference: &str) -> Option<&'a Part> {
    let c = design.components.get(reference)?;
    if c.dnp {
        return None;
    }
    match lib.lookup(c) {
        // A file `part new` started and nobody has read in yet says nothing about clamping or surviving.
        Lookup::Datasheet(p) if !p.unread() => Some(p),
        _ => None,
    }
}

/// The datasheet name of a component's pin, from its part file.
fn pin_name<'a>(part: &'a Part, number: &'a str) -> &'a str {
    part.pins.get(number).map_or(number, String::as_str)
}

/// The nets an external connector pin is on, with the pins that reach each, leaving out the inputs' reference
/// (ground) nets.
fn external_nets(design: &Design, inputs: &BoardInputs) -> BTreeMap<usize, Vec<String>> {
    let net = |r: &str, p: &str| design.components.get(r)?.pins.get(p).map(|p| p.net);
    let grounds: BTreeSet<usize> = inputs
        .inputs
        .iter()
        .filter_map(|i| net(&i.to.reference, &i.to.pin))
        .collect();
    let mut out: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let pins = inputs
        .inputs
        .iter()
        .filter(|i| i.fault.is_none())
        .map(|i| &i.pin)
        .chain(inputs.undriven.iter().map(|u| &u.pin));
    for p in pins {
        if let Some(n) = net(&p.reference, &p.pin)
            && !grounds.contains(&n)
        {
            let label = p.to_string();
            let list = out.entry(n).or_default();
            if !list.contains(&label) {
                list.push(label);
            }
        }
    }
    out
}

/// An IC pin an external net reaches, directly or through one series resistor, inductor or diode.
struct Reached<'a> {
    reference: String,
    pin: String,
    part: &'a Part,
    through: Option<String>,
}

/// A net with a power pin on it: a supply rail or ground.
fn is_rail(design: &Design, net: usize) -> bool {
    design.nets[net].pins.iter().any(|(r, p)| {
        design
            .components
            .get(r)
            .and_then(|c| c.pins.get(p))
            .is_some_and(|p| matches!(p.kind, PinKind::PowerIn | PinKind::PowerOut))
    })
}

/// The TVS clamps and IC pins an external net reaches. Clamps are keyed by the series part they sit behind
/// (`None` for the net itself): a clamp behind a series resistor or diode protects what is behind that part, not
/// a pin on the connector's side of it.
type Clamps = BTreeMap<Option<String>, Vec<String>>;

/// A connector, test point, jumper or net tie: by its reference (`J3`, `P1`, `TP1`, `CN2`, `JP1`, `NT1`,
/// `USB1`), its symbol library (`Connector:…`, as `undriven_pins` reads it), its part file's class, or because
/// inputs.toml names it. It carries the discharge rather than taking it, so it needs no part file here.
fn is_connector_like(design: &Design, lib: &Library, reference: &str, named: &BTreeSet<String>) -> bool {
    let letters: String = reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    named.contains(reference)
        || matches!(
            letters.as_str(),
            "J" | "P" | "TP" | "CN" | "CON" | "USB" | "JP" | "SJ" | "NT"
        )
        || design.components.get(reference).is_some_and(|c| {
            c.lib_id.starts_with("Connector") || lib.part_for(c).is_some_and(|p| p.class == Class::Connector)
        })
}

/// A two-pin resistor, capacitor, inductor or ferrite (by reference) with its far side on a rail or ground: a
/// pull, decoupling or filter part. The discharge goes into the rail, not into one pin, so it needs no part file.
fn to_rail(design: &Design, reference: &str, net: usize) -> bool {
    let letters: String = reference.chars().take_while(char::is_ascii_alphabetic).collect();
    let Some(c) = design.components.get(reference) else {
        return false;
    };
    matches!(letters.as_str(), "R" | "C" | "L" | "FB")
        && c.pins.len() == 2
        && c.pins.values().any(|p| p.net != net && is_rail(design, p.net))
}

/// A fitted part with no part file and no class default on a net the discharge reaches (not a connector, test
/// point or passive to a rail): it might be an IC or a clamp, so the rule can't pass the net.
fn unmodelled(design: &Design, lib: &Library, reference: &str, net: usize, named: &BTreeSet<String>) -> Option<String> {
    let c = design.components.get(reference)?;
    if c.dnp || is_connector_like(design, lib, reference, named) || to_rail(design, reference, net) {
        return None;
    }
    match lib.lookup(c) {
        Lookup::Unchecked(_) => Some(format!("{reference} ({})", c.value)),
        Lookup::Datasheet(p) if p.unread() => Some(format!("{reference} ({})", c.value)),
        _ => None,
    }
}

/// Parts with no part file, keyed like [`Clamps`] by the series part they sit behind.
type Unknown = BTreeMap<Option<String>, Vec<String>>;

/// What the discharge reaches from a net: clamps (by the series part they sit behind), ICs, clamps that break
/// down too high, and parts with no part file.
type Reach<'a> = (Clamps, Vec<Reached<'a>>, Vec<String>, Unknown);

fn reached<'a>(design: &Design, lib: &'a Library, net: usize, named: &BTreeSet<String>) -> Reach<'a> {
    let mut clamps = Clamps::new();
    let mut ics = vec![];
    let mut unknown = Unknown::new();
    let mut raw: BTreeMap<Option<String>, Vec<(String, Option<f64>)>> = BTreeMap::new();
    let visit = |n: usize,
                 through: Option<String>,
                 raw: &mut BTreeMap<Option<String>, Vec<(String, Option<f64>)>>,
                 ics: &mut Vec<Reached<'a>>,
                 unknown: &mut Unknown| {
        for (r, p) in &design.nets[n].pins {
            let Some(part) = part_of(design, lib, r) else {
                if let Some(u) = unmodelled(design, lib, r, n, named) {
                    unknown
                        .entry(through.clone())
                        .or_default()
                        .push(format!("{u} has no part file"));
                }
                continue;
            };
            match part.class {
                Class::Tvs => {
                    // The breakdown maximum is what it is compared with; a part that gives only a minimum breakdown
                    // or a clamping voltage (USB ESD arrays) still clamps, but can't be compared.
                    let breakdown = part
                        .limits
                        .iter()
                        .filter_map(|l| match &l.breakdown {
                            Some(Range::Known {
                                max: Bound::Value(v), ..
                            }) => Some(*v),
                            _ => None,
                        })
                        .reduce(f64::max);
                    let rated = part.limits.iter().any(|l| {
                        l.clamp.is_some()
                            || matches!(
                                &l.breakdown,
                                Some(
                                    Range::Known {
                                        min: Bound::Value(_),
                                        ..
                                    } | Range::Known {
                                        max: Bound::Value(_),
                                        ..
                                    }
                                )
                            )
                    });
                    // A TVS whose breakdown isn't known can't be said to clamp anything.
                    if !rated {
                        unknown.entry(through.clone()).or_default().push(format!(
                            "{r} ({}) gives no breakdown voltage or clamping voltage",
                            part.mpn
                        ));
                        continue;
                    }
                    raw.entry(through.clone())
                        .or_default()
                        .push((format!("{r} ({})", part.mpn), breakdown));
                }
                Class::Ic => ics.push(Reached {
                    reference: r.clone(),
                    pin: p.clone(),
                    part,
                    through: through.clone(),
                }),
                _ => {}
            }
        }
    };
    visit(net, None, &mut raw, &mut ics, &mut unknown);
    for (r, _) in &design.nets[net].pins {
        let Some(c) = design.components.get(r) else { continue };
        let class = part_of(design, lib, r).map(|p| p.class);
        let series = matches!(class, Some(Class::Resistor | Class::Inductor | Class::Diode));
        if !series || c.pins.len() != 2 {
            continue;
        }
        let pull = class == Some(Class::Resistor);
        // A pull-up or pull-down ends on a rail or ground, which every IC on the board shares: the discharge goes
        // into that rail's decoupling, not into one pin, so the far side isn't followed.
        for p in c
            .pins
            .values()
            .filter(|p| p.net != net && !(pull && is_rail(design, p.net)))
        {
            visit(p.net, Some(r.clone()), &mut raw, &mut ics, &mut unknown);
        }
    }
    // A TVS only protects the pins beside it when it breaks down before they do: an SMF26A (up to 32 V) on a
    // 3.3 V logic pin lets the pin take the discharge first.
    let mut too_high = vec![];
    for (through, list) in raw {
        let weakest = ics
            .iter()
            .filter(|ic| ic.through == through)
            .filter_map(|ic| {
                let name = pin_name(ic.part, &ic.pin);
                ic.part
                    .limits
                    .iter()
                    .filter(|l| l.quantity == Quantity::Voltage && l.pins.iter().any(|p| p == name))
                    .filter_map(|l| match &l.abs_max {
                        Range::Known {
                            max: Bound::Value(v), ..
                        } => Some((*v, format!("{}.{name}", ic.reference))),
                        _ => None,
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        for (label, breakdown) in list {
            match (&weakest, breakdown) {
                (Some((pin_max, pin)), Some(b)) if b > *pin_max => {
                    too_high.push(format!(
                        "{label} breaks down only at up to {b} V, above the {pin_max} V {pin} survives"
                    ));
                }
                _ => clamps.entry(through.clone()).or_default().push(label),
            }
        }
    }
    for v in clamps.values_mut() {
        v.sort();
        v.dedup();
    }
    for v in unknown.values_mut() {
        v.sort();
        v.dedup();
    }
    (clamps, ics, too_high, unknown)
}

/// The `esd` rule: every net an external connector pin is on is clamped by a TVS or ESD diode, or everything on
/// it is rated to the policy's IEC 61000-4-2 level.
pub fn esd(design: &Design, lib: &Library, policy: &Policy, inputs: &BoardInputs, r: &mut Vec<Finding>) {
    let external = external_nets(design, inputs);
    let named: BTreeSet<String> = external
        .values()
        .flatten()
        .filter_map(|p| p.split('.').next())
        .map(str::to_string)
        .collect();
    for (net, pins) in external {
        let at = format!("{} ({})", pins.join(", "), design.nets[net].name);
        let (clamps, ics, too_high, unknown) = reached(design, lib, net, &named);
        // A part with no part file behind a series part clamped on its far side is sheltered by that clamp, like
        // an IC there.
        let unknown: Vec<String> = unknown
            .into_iter()
            .filter(|(t, _)| t.is_none() || !clamps.contains_key(t))
            .flat_map(|(_, u)| u)
            .collect();
        let no_file = unknown.join("; ");
        let at = if too_high.is_empty() {
            at
        } else {
            format!("{at} ({})", too_high.join("; "))
        };
        let finding = |verdict, reference: &str, message: String| Finding {
            rule: "esd",
            verdict,
            severity: Severity::Warning,
            reference: reference.to_string(),
            message,
            nets: vec![net],
            case: None,
            headline: None,
        };
        let connector = pins[0].split('.').next().unwrap_or_default().to_string();
        if let Some(direct) = clamps.get(&None) {
            r.push(finding(
                Verdict::Pass,
                &connector,
                format!("{at} is clamped by {}", direct.join(", ")),
            ));
            continue;
        }
        // ICs behind a series part with a clamp on its far side are protected by that clamp.
        let (behind, ics): (Vec<Reached<'_>>, Vec<Reached<'_>>) = ics.into_iter().partition(|ic| {
            ic.through
                .as_ref()
                .is_some_and(|t| clamps.contains_key(&Some(t.clone())))
        });
        let sheltered: Vec<String> = clamps
            .iter()
            .filter_map(|(t, c)| Some(format!("{} behind {}", c.join(", "), t.as_ref()?)))
            .collect();
        if ics.is_empty() && !sheltered.is_empty() && unknown.is_empty() {
            r.push(finding(
                Verdict::Pass,
                &connector,
                format!(
                    "{at} reaches its ICs only through series parts clamped on the far side: {}",
                    sheltered.join("; ")
                ),
            ));
            continue;
        }
        let Some(level) = &policy.esd else {
            r.push(finding(
                Verdict::Unknown,
                &connector,
                format!("{at} has no clamp, and policy.toml sets no ESD level to hold its parts to"),
            ));
            continue;
        };
        if ics.is_empty() {
            r.push(if unknown.is_empty() {
                finding(
                    Verdict::Pass,
                    &connector,
                    format!("{at} has no clamp, but reaches no IC, only passive parts"),
                )
            } else {
                finding(
                    Verdict::Unknown,
                    &connector,
                    if sheltered.is_empty() {
                        format!("{at} has no clamp that a part file describes, and it reaches parts nothing describes: {no_file}")
                    } else {
                        format!(
                            "{at} is clamped only behind series parts ({}), and it reaches parts nothing describes \
                             outside them: {no_file}",
                            sheltered.join("; ")
                        )
                    },
                )
            });
            continue;
        }
        let (mut weak, mut unrated) = (vec![], vec![]);
        let mut fine: Vec<String> = behind
            .iter()
            .map(|ic| {
                let t = ic.through.clone().unwrap_or_default();
                let c = clamps.get(&Some(t.clone())).map(|c| c.join(", ")).unwrap_or_default();
                format!(
                    "{}.{} is behind {t}, clamped by {c}",
                    ic.reference,
                    pin_name(ic.part, &ic.pin)
                )
            })
            .collect();
        for ic in &ics {
            let name = pin_name(ic.part, &ic.pin);
            let via = ic.through.as_ref().map(|t| format!(" through {t}")).unwrap_or_default();
            let who = format!("{}.{name}{via}", ic.reference);
            let cite = |model: EsdModel, v: f64, page: u32| {
                let doc = ic.part.datasheet.as_ref().map_or("", |d| d.document.as_str());
                format!("±{} kV {} ({} {doc} p{page})", v / 1000.0, model.name(), ic.part.mpn)
            };
            let ratings: Vec<_> = ic.part.esd_for(name).collect();
            let system = ratings.iter().find(|e| match e.model {
                EsdModel::IecContact => e.volts >= level.contact,
                EsdModel::IecAir => e.volts >= level.air,
                _ => false,
            });
            if let Some(e) = system {
                fine.push(format!("{who} is rated {}", cite(e.model, e.volts, e.page)));
            } else if let Some(best) = ratings.iter().max_by(|a, b| a.volts.total_cmp(&b.volts)) {
                weak.push(format!(
                    "{who} is rated only {}",
                    cite(best.model, best.volts, best.page)
                ));
            } else {
                unrated.push(format!("{who} ({} gives no ESD rating for it)", ic.part.mpn));
            }
        }
        let need = format!("{} V contact or {} V air ({})", level.contact, level.air, level.reason);
        let (verdict, text) = if !weak.is_empty() {
            (
                Verdict::Fail,
                format!(
                    "{at} has no TVS or ESD clamp, and {}: a component rating covers handling in a factory, not a \
                     discharge through a connector, which needs {need}",
                    weak.join("; ")
                ),
            )
        } else if !unrated.is_empty() || !unknown.is_empty() {
            let mut why = vec![];
            if !unrated.is_empty() {
                why.push(format!("nothing says what {} survive", unrated.join(", ")));
            }
            if !unknown.is_empty() {
                why.push(no_file.clone());
            }
            (Verdict::Unknown, format!("{at} has no clamp, and {}", why.join("; ")))
        } else {
            (Verdict::Pass, format!("{at} has no clamp, but {}", fine.join("; ")))
        };
        r.push(finding(verdict, &connector, text));
    }
}

/// A diode or TVS conducting forward straight across a reversed supply: both its ends are on nets an input
/// or its reference holds, so nothing on the board limits the current.
fn crowbars(design: &Design, lib: &Library, case: &BoardInputs, volts: &Voltages) -> Vec<String> {
    let net = |r: &str, p: &str| design.components.get(r)?.pins.get(p).map(|p| p.net);
    let held: BTreeSet<usize> = case
        .inputs
        .iter()
        .flat_map(|i| [net(&i.pin.reference, &i.pin.pin), net(&i.to.reference, &i.to.pin)])
        .flatten()
        .collect();
    let mut out = vec![];
    for (reference, c) in &design.components {
        let Some(part) = part_of(design, lib, reference) else {
            continue;
        };
        // (anode, cathode) pin names: a forward drive, or a unidirectional TVS's breakdown limit (K from A).
        let mut pairs: Vec<(&str, &str)> = part
            .drives
            .iter()
            .filter(|d| d.kind == DriveKind::Forward)
            .map(|d| (d.pin.as_str(), d.to.as_str()))
            .collect();
        pairs.extend(
            part.limits
                .iter()
                .filter(|l| l.breakdown.is_some() && !l.bidirectional)
                .filter_map(|l| Some((l.to.as_deref()?, l.pins.first()?.as_str()))),
        );
        for (a, k) in pairs {
            let nets = |name: &str| -> Vec<usize> {
                part.pins_named(name)
                    .iter()
                    .filter_map(|num| c.pins.get(*num).map(|p| p.net))
                    .collect()
            };
            for &an in &nets(a) {
                for &kn in &nets(k) {
                    let (sa, sk) = (&volts.nets[an].span, &volts.nets[kn].span);
                    let conducts = sa.min.zip(sk.max).is_some_and(|(lo, hi)| lo - hi > 0.3);
                    if conducts && held.contains(&an) && held.contains(&kn) {
                        out.push(format!(
                            "{reference} ({}) from {} ({}) to {} ({})",
                            part.mpn, design.nets[an].name, sa, design.nets[kn].name, sk
                        ));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Diodes the reversed supply meets in series and that block it: anode on the input's net, cathode free to
/// sit at 0 V.
fn blockers(design: &Design, lib: &Library, input_net: usize) -> Vec<String> {
    let mut out = vec![];
    for (reference, c) in &design.components {
        let Some(part) = part_of(design, lib, reference) else {
            continue;
        };
        for d in part.drives.iter().filter(|d| d.kind == DriveKind::Forward) {
            let on = |name: &str| {
                part.pins_named(name)
                    .iter()
                    .any(|num| c.pins.get(*num).is_some_and(|p| p.net == input_net))
            };
            if on(&d.pin) && !on(&d.to) {
                out.push(format!("{reference} ({})", part.mpn));
            }
        }
    }
    out
}

/// What a finding checked: its rule, part and the pins before the numbers, such as `D1.K (+VDC) from D1.A
/// (VBUS)`.
fn subject(f: &Finding) -> (&str, &str, &str) {
    let what = f.message.split(": ").next().unwrap_or("");
    let what = what.split(" (").next().unwrap_or(what);
    (f.rule, f.reference.as_str(), what)
}

/// The `reverse-polarity` rule for one supply input connected backwards: `findings` are the damage rules'
/// findings in that fault case, `normal` the findings of normal operation, and `volts` the case's net
/// voltages. Only what the reversal makes worse counts: a check that already fails or can't be made in normal
/// operation says nothing about the reversal.
#[allow(clippy::too_many_arguments)]
pub fn reverse_polarity(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    input_name: &str,
    case_name: &str,
    findings: &[Finding],
    normal: &[Finding],
    volts: &Voltages,
) -> Finding {
    // A finding is new to the reversed case unless normal operation has the same one: same rule, part, pin,
    // range and reason. Matching on the part and pin alone would hide a pin that fails its maximum normally and
    // its minimum when reversed.
    let prefix = format!("fault {case_name:?}: ");
    let same_as_normal = |f: &Finding| {
        let text = f.message.strip_prefix(&prefix).unwrap_or(&f.message);
        normal
            .iter()
            .any(|n| n.case.is_none() && n.rule == f.rule && n.verdict == f.verdict && n.message == text)
    };
    let input = inputs.inputs.iter().find(|i| i.name == input_name);
    let at = input.map_or_else(|| input_name.to_string(), |i| format!("{input_name} on {}", i.pin));
    let reference = input.map_or_else(String::new, |i| i.pin.reference.clone());
    let case = inputs.with_reversals().case(design, case_name);
    let input_net = input
        .and_then(|i| design.pin_net(&i.pin.reference, &i.pin.pin))
        .and_then(|n| design.nets.iter().position(|m| m.name == n.name));
    let crow = crowbars(design, lib, &case, volts);
    let failed: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.verdict == Verdict::Fail && f.severity == Severity::Error && !same_as_normal(f))
        .collect();
    // An UNKNOWN counts only when the same pin passed in normal operation: a pin with no rated minimum is
    // UNKNOWN either way, and listing it here would only repeat that.
    let passed_before = |f: &Finding| {
        normal
            .iter()
            .filter(|n| n.case.is_none() && subject(n) == subject(f))
            .all(|n| n.verdict == Verdict::Pass)
    };
    let unknown: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.verdict == Verdict::Unknown && passed_before(f))
        .collect();
    let refs = |fs: &[&Finding]| {
        let mut v: Vec<&str> = fs.iter().map(|f| f.reference.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v.join(", ")
    };
    let (verdict, message) = if !failed.is_empty() {
        (
            Verdict::Fail,
            format!(
                "{at} connected backwards damages {}: see the \"{case_name}\" findings",
                refs(&failed)
            ),
        )
    } else if !crow.is_empty() {
        (
            Verdict::Unknown,
            format!(
                "{at} connected backwards drives {} forward straight across the supply. It survives only if the \
                 supply's current is limited or a fuse opens first, and inputs.toml gives neither",
                crow.join(", ")
            ),
        )
    } else if !unknown.is_empty() {
        (
            Verdict::Unknown,
            format!(
                "{at} connected backwards: nothing is shown to fail, but {} can't be checked (see the \
                 \"{case_name}\" findings)",
                refs(&unknown)
            ),
        )
    } else {
        let blocked = input_net.map(|n| blockers(design, lib, n)).unwrap_or_default();
        let how = if blocked.is_empty() {
            String::new()
        } else {
            format!(", blocked by {}", blocked.join(", "))
        };
        (
            Verdict::Pass,
            format!("{at} connected backwards stays inside every part's absolute maximum ratings{how}"),
        )
    };
    Finding {
        rule: "reverse-polarity",
        verdict,
        severity: Severity::Error,
        reference,
        message,
        nets: input_net.into_iter().collect(),
        case: Some(case_name.to_string()),
        headline: None,
    }
}
