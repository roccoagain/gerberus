//! The worst-case voltage range on every net, and why.
//!
//! Ranges start at the board inputs (`inputs.toml`) and spread through what the part files say parts do:
//!
//! - a diode's forward drop (`[[drive]] kind = "forward"`),
//! - a regulator's output or feedback pin (`kind = "regulates"`), through a two-resistor divider when the
//!   feedback pin has one, using each resistor's tolerance from its part file,
//! - a switch node (`kind = "switches"`).
//!
//! A net none of those reach gets the range of everything that can pull it: nets it reaches through
//! resistors, inductors, LEDs and jumpers, and IC output pins, which reach at most their part's highest supply.
//! Capacitors are treated as open.
//!
//! Nothing is guessed. When a step needs a number nobody gave (a connector pin with no input, a resistor with
//! no tolerance, a datasheet that gives only one end), that end of the range is unknown and the reason says
//! why. Checkers must report UNKNOWN for it, never pass.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::design::{Component, Design, PinKind};
use crate::inputs::BoardInputs;
use crate::limits::{Bound, Class, Drive, DriveKind, Library, Part, Quantity, Range};

/// A voltage range from the board's ground. Either end is `None` when it can't be worked out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Span {
    pub fn new(min: f64, max: f64) -> Span {
        Span {
            min: Some(min),
            max: Some(max),
        }
    }

    pub fn exact(v: f64) -> Span {
        Span::new(v, v)
    }

    pub fn unknown() -> Span {
        Span { min: None, max: None }
    }

    pub fn is_known(&self) -> bool {
        self.min.is_some() && self.max.is_some()
    }

    /// Every voltage either span allows. An unknown end stays unknown.
    pub fn union(self, o: Span) -> Span {
        Span {
            min: self.min.zip(o.min).map(|(a, b)| a.min(b)),
            max: self.max.zip(o.max).map(|(a, b)| a.max(b)),
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let end = |v: Option<f64>| v.map_or("UNKNOWN".to_string(), |v| format!("{}", round(v)));
        match (self.min, self.max) {
            (None, None) => write!(f, "UNKNOWN"),
            (Some(a), Some(b)) if a == b => write!(f, "{} V", round(a)),
            _ => write!(f, "{} to {} V", end(self.min), end(self.max)),
        }
    }
}

/// Three decimals, without trailing zeros.
fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// One step of the reasoning behind a net's range.
#[derive(Debug, Clone, PartialEq)]
pub struct Why {
    pub text: String,
    /// Nets this step read, so an explanation can show where their ranges came from.
    pub from: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetVoltage {
    pub span: Span,
    pub why: Vec<Why>,
}

/// The range of every net in a design, indexed like [`Design::nets`].
#[derive(Debug, Clone)]
pub struct Voltages {
    pub nets: Vec<NetVoltage>,
}

impl Voltages {
    pub fn by_name(&self, design: &Design, name: &str) -> Option<&NetVoltage> {
        let i = design.nets.iter().position(|n| n.name == name)?;
        self.nets.get(i)
    }

    /// The net's range, the reasons for it, and in turn the ranges of the nets those reasons read.
    pub fn explain(&self, design: &Design, net: usize) -> String {
        let mut out = String::new();
        let mut seen = BTreeSet::new();
        self.explain_into(design, net, 0, &mut seen, &mut out);
        out
    }

    fn explain_into(&self, design: &Design, net: usize, depth: usize, seen: &mut BTreeSet<usize>, out: &mut String) {
        let pad = "  ".repeat(depth);
        let nv = &self.nets[net];
        let name = &design.nets[net].name;
        if !seen.insert(net) {
            let _ = writeln!(out, "{pad}{name}: {} (explained above)", nv.span);
            return;
        }
        let _ = writeln!(out, "{pad}{name}: {}", nv.span);
        for w in &nv.why {
            let _ = writeln!(out, "{pad}  - {}", w.text);
            for &f in &w.from {
                self.explain_into(design, f, depth + 2, seen, out);
            }
        }
    }
}

/// Works out every net's range. `inputs` should already pass [`BoardInputs::problems`]; an input whose pins
/// aren't in the design is skipped.
pub fn compute(design: &Design, lib: &Library, inputs: &BoardInputs) -> Voltages {
    let n = design.nets.len();
    let mut out: Vec<Option<NetVoltage>> = vec![None; n];
    let net_of = |r: &str, p: &str| design.components.get(r)?.pins.get(p).map(|p| p.net);

    // 1. Inputs, and the pins they are measured from, which are 0 V.
    let mut fixed = vec![false; n];
    for i in &inputs.inputs {
        let (Some(pn), Some(tn)) = (net_of(&i.pin.reference, &i.pin.pin), net_of(&i.to.reference, &i.to.pin)) else {
            continue;
        };
        merge(
            &mut out[tn],
            Span::exact(0.0),
            Why {
                text: format!("0 V: the reference of input {} ({})", i.name, i.to),
                from: vec![],
            },
        );
        merge(
            &mut out[pn],
            Span::new(i.min, i.max),
            Why {
                text: format!(
                    "input {} on {} from {}: {} to {} V ({})",
                    i.name, i.pin, i.to, i.min, i.max, i.source
                ),
                from: vec![],
            },
        );
        fixed[tn] = true;
        fixed[pn] = true;
    }

    // 2. What fitted parts drive, from their part files.
    let mut plans: BTreeMap<usize, Vec<Plan>> = BTreeMap::new();
    for (r, c) in &design.components {
        if c.dnp {
            continue;
        }
        let Some(part) = lib.part_for(c) else { continue };
        for d in &part.drives {
            for p in plan_drive(design, lib, inputs, r, c, part, d) {
                if !fixed[p.target] {
                    plans.entry(p.target).or_default().push(p);
                }
            }
        }
    }

    // 3. Groups of the remaining nets, joined by resistors, inductors, LEDs and jumpers.
    let strong: Vec<bool> = (0..n).map(|i| fixed[i] || plans.contains_key(&i)).collect();
    let groups = build_groups(design, lib, inputs, &strong);

    // 4. Resolve whatever has everything it reads, until nothing changes.
    let mut done_groups = vec![false; groups.len()];
    loop {
        let mut progress = false;
        for (&t, ps) in &plans {
            if out[t].is_some() || !ps.iter().all(|p| p.deps.iter().all(|&d| out[d].is_some())) {
                continue;
            }
            let mut nv: Option<NetVoltage> = None;
            for p in ps {
                let (span, why) = p.eval(design, &out);
                merge(&mut nv, span, why);
            }
            // A connector pin on a driven net can bring in anything, unless inputs.toml says nothing outside
            // drives it. (An input sets its whole net, so pins on an input's net are covered by it.)
            if let Some(nv) = nv.as_mut() {
                for pin in open_connectors(design, lib, inputs, t) {
                    nv.span = Span::unknown();
                    nv.why.push(Why {
                        text: format!(
                            "UNKNOWN: {pin} is a connector pin with no range in inputs.toml, so whatever is \
                             plugged in there can push this net anywhere"
                        ),
                        from: vec![],
                    });
                }
            }
            out[t] = nv;
            progress = true;
        }
        for (gi, g) in groups.iter().enumerate() {
            if done_groups[gi] || !g.deps().iter().all(|&d| out[d].is_some()) {
                continue;
            }
            let nv = g.eval(design, &out);
            for &net in &g.nets {
                out[net] = Some(nv.clone());
            }
            done_groups[gi] = true;
            progress = true;
        }
        if !progress {
            break;
        }
    }

    // 5. Anything left reads a net that, in turn, reads it.
    let stuck = |deps: &[usize], out: &[Option<NetVoltage>]| -> String {
        let names: BTreeSet<&str> = deps
            .iter()
            .filter(|&&d| out[d].is_none())
            .map(|&d| design.nets[d].name.as_str())
            .collect();
        format!(
            "UNKNOWN: depends on {}, which depend on this net in turn",
            names.into_iter().collect::<Vec<_>>().join(", ")
        )
    };
    let mut late: Vec<(usize, NetVoltage)> = vec![];
    for (&t, ps) in &plans {
        if out[t].is_none() {
            let deps: Vec<usize> = ps.iter().flat_map(|p| p.deps.clone()).collect();
            late.push((t, unknown(stuck(&deps, &out), vec![])));
        }
    }
    for (gi, g) in groups.iter().enumerate() {
        if !done_groups[gi] {
            let nv = unknown(stuck(&g.deps(), &out), vec![]);
            late.extend(g.nets.iter().map(|&net| (net, nv.clone())));
        }
    }
    for (t, nv) in late {
        out[t] = Some(nv);
    }

    Voltages {
        nets: out
            .into_iter()
            .map(|nv| nv.unwrap_or_else(|| unknown("UNKNOWN: not reached".into(), vec![])))
            .collect(),
    }
}

fn unknown(text: String, from: Vec<usize>) -> NetVoltage {
    NetVoltage {
        span: Span::unknown(),
        why: vec![Why { text, from }],
    }
}

fn merge(slot: &mut Option<NetVoltage>, span: Span, why: Why) {
    match slot {
        Some(nv) => {
            nv.span = nv.span.union(span);
            nv.why.push(why);
        }
        None => {
            *slot = Some(NetVoltage { span, why: vec![why] });
        }
    }
}

fn span_of(out: &[Option<NetVoltage>], net: usize) -> Span {
    out[net].as_ref().map_or(Span::unknown(), |nv| nv.span)
}

/// The net a part-file pin name is on, through the part's pin map.
fn part_pin_net(c: &Component, part: &Part, name: &str) -> Option<usize> {
    part.pins_named(name)
        .into_iter()
        .find_map(|num| c.pins.get(num).map(|p| p.net))
}

fn class_of(lib: &Library, c: &Component) -> Option<Class> {
    lib.part_for(c).map(|p| p.class)
}

/// Connector pins on a net that neither an input nor an `undriven` entry describes.
fn open_connectors(design: &Design, lib: &Library, inputs: &BoardInputs, net: usize) -> Vec<String> {
    design.nets[net]
        .pins
        .iter()
        .filter(|(r, p)| {
            design
                .components
                .get(r)
                .is_some_and(|c| !c.dnp && is_connector(lib, c) && !is_undriven(inputs, r, p))
        })
        .map(|(r, p)| format!("{r}.{p}"))
        .collect()
}

fn is_undriven(inputs: &BoardInputs, r: &str, p: &str) -> bool {
    inputs.undriven.iter().any(|u| u.pin.reference == r && u.pin.pin == p)
}

fn is_connector(lib: &Library, c: &Component) -> bool {
    c.lib_id.starts_with("Connector") || class_of(lib, c) == Some(Class::Connector)
}

fn is_resistor(lib: &Library, c: &Component) -> bool {
    c.pins.len() == 2 && (c.lib_id.starts_with("Device:R") || class_of(lib, c) == Some(Class::Resistor))
}

fn is_capacitor(lib: &Library, c: &Component) -> bool {
    c.lib_id.starts_with("Device:C") || class_of(lib, c) == Some(Class::Capacitor)
}

/// A two-pin part a net's voltage passes through, for the fallback range: resistors, inductors (and so LEDs,
/// `Device:L*`), and jumpers, which may be closed.
fn is_link(lib: &Library, c: &Component) -> bool {
    c.pins.len() == 2
        && (c.lib_id.starts_with("Device:R")
            || c.lib_id.starts_with("Device:L")
            || c.lib_id.starts_with("Jumper:")
            || matches!(class_of(lib, c), Some(Class::Resistor | Class::Inductor | Class::Led)))
}

/// The datasheet page a range cites, as text.
fn cite(part: &Part, range: &Range) -> String {
    let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
    match range {
        Range::Known {
            page: Some(p), note, ..
        } => {
            let note = note.as_deref().map(|n| format!(", {n}")).unwrap_or_default();
            format!("{} {doc} p{p}{note}", part.mpn)
        }
        _ => format!("{} {doc}", part.mpn),
    }
}

/// Reads a resistor value such as `47k`, `4k7`, `120`, `2R2` or `1M` as ohms.
pub fn parse_ohms(value: &str) -> Option<f64> {
    let s: String = value
        .trim()
        .trim_end_matches("ohms")
        .trim_end_matches("ohm")
        .trim_end_matches('Ω')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let letter = s.char_indices().find_map(|(i, ch)| {
        let m = match ch {
            'R' | 'r' => 1.0,
            'k' | 'K' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            _ => return None,
        };
        Some((i, ch.len_utf8(), m))
    });
    let (digits, mult) = match letter {
        None => (s.clone(), 1.0),
        Some((i, len, m)) => {
            let (whole, frac) = (&s[..i], &s[i + len..]);
            if frac.is_empty() {
                (whole.to_string(), m)
            } else if whole.contains('.') || !frac.chars().all(|c| c.is_ascii_digit()) {
                return None;
            } else {
                (format!("{whole}.{frac}"), m)
            }
        }
    };
    let v: f64 = digits.parse().ok()?;
    (v.is_finite() && v >= 0.0).then_some(v * mult)
}

/// One way a part sets a net.
struct Plan<'a> {
    target: usize,
    /// Nets that must be known first.
    deps: Vec<usize>,
    reference: &'a str,
    part: &'a Part,
    drive: &'a Drive,
    how: How,
}

enum How {
    /// The drive's `to` (cathode) net is the anode net minus the forward drop.
    Forward {
        anode: usize,
    },
    Regulates {
        supply: usize,
        ground: usize,
        divider: Option<Divider>,
        /// The target is the divider's output, not the regulated pin itself.
        on_output: bool,
    },
    Switches {
        supply: usize,
        ground: usize,
    },
    /// The net can't be worked out, for this reason.
    Fails(String),
}

/// A feedback divider: `top` from the output to the feedback pin, `bottom` from it to ground.
#[derive(Clone)]
struct Divider {
    top: String,
    bottom: String,
    /// Smallest and largest resistance, from the value and the tolerance.
    rt: (f64, f64),
    rb: (f64, f64),
    /// What the explanation cites for each resistor.
    note: String,
}

impl Divider {
    /// The output range for a feedback range `(lo, hi)`.
    fn output(&self, lo: f64, hi: f64) -> (f64, f64) {
        (lo * (1.0 + self.rt.0 / self.rb.1), hi * (1.0 + self.rt.1 / self.rb.0))
    }
}

fn plan_drive<'a>(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    reference: &'a str,
    c: &Component,
    part: &'a Part,
    drive: &'a Drive,
) -> Vec<Plan<'a>> {
    let net = |name: &str| part_pin_net(c, part, name);
    let plan = |target, deps, how| Plan {
        target,
        deps,
        reference,
        part,
        drive,
        how,
    };
    let Some(pin) = net(&drive.pin) else { return vec![] };
    let to = net(&drive.to);
    let supply = drive.supply.as_deref().and_then(net);
    match drive.kind {
        DriveKind::Forward => match to {
            Some(k) if k != pin => vec![plan(k, vec![pin], How::Forward { anode: pin })],
            _ => vec![],
        },
        DriveKind::Switches => match (supply, to) {
            (Some(s), Some(g)) => vec![plan(pin, vec![s, g], How::Switches { supply: s, ground: g })],
            _ => vec![plan(pin, vec![], How::Fails(missing(drive)))],
        },
        DriveKind::Regulates => {
            let (Some(s), Some(g)) = (supply, to) else {
                return vec![plan(pin, vec![], How::Fails(missing(drive)))];
            };
            let found = if drive.feedback {
                find_divider(design, lib, inputs, reference, pin, g)
            } else {
                Ok(None)
            };
            match found {
                Ok(None) => vec![plan(
                    pin,
                    vec![s, g],
                    How::Regulates {
                        supply: s,
                        ground: g,
                        divider: None,
                        on_output: false,
                    },
                )],
                Ok(Some((div, output))) => vec![
                    plan(
                        pin,
                        vec![s, g],
                        How::Regulates {
                            supply: s,
                            ground: g,
                            divider: Some(div.clone()),
                            on_output: false,
                        },
                    ),
                    plan(
                        output,
                        vec![s, g],
                        How::Regulates {
                            supply: s,
                            ground: g,
                            divider: Some(div),
                            on_output: true,
                        },
                    ),
                ],
                Err((reason, output)) => {
                    let mut v = vec![plan(pin, vec![], How::Fails(reason.clone()))];
                    if let Some(o) = output {
                        v.push(plan(o, vec![], How::Fails(reason)));
                    }
                    v
                }
            }
        }
    }
}

fn missing(drive: &Drive) -> String {
    format!(
        "UNKNOWN: the part's {} or supply pin isn't connected in the schematic",
        drive.to
    )
}

/// The divider on a regulator's feedback net, and the net it sets. `Ok(None)` when no resistor is on the pin,
/// so it is tied straight to the output and the drive sets the pin's own net. The error carries the output net
/// when one was found, so it too is reported.
#[allow(clippy::type_complexity)]
fn find_divider(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    regulator: &str,
    fb: usize,
    ground: usize,
) -> Result<Option<(Divider, usize)>, (String, Option<usize>)> {
    let mut resistors: Vec<(&str, &Component, usize)> = vec![];
    for (r, p) in &design.nets[fb].pins {
        if r == regulator {
            continue;
        }
        let Some(c) = design.components.get(r) else { continue };
        if c.dnp || is_capacitor(lib, c) {
            continue;
        }
        if !is_resistor(lib, c) {
            return Err((
                format!(
                    "UNKNOWN: {r}.{p} is on the feedback net {} besides the divider, and gerberus only understands \
                     two resistors and capacitors there",
                    design.nets[fb].name
                ),
                None,
            ));
        }
        let other = c.pins.values().map(|q| q.net).find(|&q| q != fb).unwrap_or(fb);
        resistors.push((r.as_str(), c, other));
    }
    if resistors.is_empty() {
        return Ok(None);
    }
    let bottom: Vec<_> = resistors.iter().filter(|x| x.2 == ground).collect();
    let top: Vec<_> = resistors.iter().filter(|x| x.2 != ground && x.2 != fb).collect();
    let (&&(rb_ref, rb, _), &&(rt_ref, rt, out)) = match (bottom.as_slice(), top.as_slice()) {
        ([b], [t]) if resistors.len() == 2 => (b, t),
        _ => {
            return Err((
                format!(
                    "UNKNOWN: the feedback net {} has {} resistors; only a two-resistor divider to ground is understood",
                    design.nets[fb].name,
                    resistors.len()
                ),
                None,
            ));
        }
    };
    let fail = |s: String| Err((format!("UNKNOWN: {s}"), Some(out)));
    let mut ends = vec![];
    let mut notes = vec![];
    for (r, c) in [(rt_ref, rt), (rb_ref, rb)] {
        let Some(ohms) = parse_ohms(&c.value).filter(|&v| v > 0.0) else {
            return fail(format!("can't read a resistance from {r}'s value {:?}", c.value));
        };
        let Some(part) = lib.part_for(c) else {
            return fail(format!(
                "{r} ({}) has no part file, so its tolerance is unknown",
                crate::limits::component_mpn(c)
            ));
        };
        let Some(tol) = &part.tolerance else {
            return fail(format!("{r}'s part file {} gives no tolerance", part.mpn));
        };
        let Some(tcr) = &part.tcr else {
            return fail(format!("{r}'s part file {} gives no temperature coefficient", part.mpn));
        };
        // Drift from the 25 °C value over the board's temperature range, or the resistor's own rated range.
        let (tmin, tmax, twhere) = match (&inputs.temperature, &part.temperature) {
            (Some(b), _) => (b.min, b.max, format!("the board's {} to {} °C", b.min, b.max)),
            (None, Some(p)) => (
                p.min,
                p.max,
                format!(
                    "its rated {} to {} °C (p{}), as inputs.toml gives no board temperature",
                    p.min, p.max, p.page
                ),
            ),
            (None, None) => {
                return fail(format!(
                    "neither inputs.toml nor {r}'s part file gives a temperature range, so its drift is unknown"
                ));
            }
        };
        let drift = tcr.ppm * 1e-6 * (tmax - 25.0).abs().max((25.0 - tmin).abs());
        let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
        notes.push(format!(
            "{r} {} ±{}% (p{}) plus {} ppm/°C (p{}) over {twhere}, so ±{}% ({} {doc})",
            c.value,
            round(tol.value * 100.0),
            tol.page,
            tcr.ppm,
            tcr.page,
            round((tol.value + drift) * 100.0),
            part.mpn,
        ));
        let dev = tol.value + drift;
        ends.push((ohms * (1.0 - dev), ohms * (1.0 + dev)));
    }
    Ok(Some((
        Divider {
            top: rt_ref.to_string(),
            bottom: rb_ref.to_string(),
            rt: ends[0],
            rb: ends[1],
            note: notes.join(" over "),
        },
        out,
    )))
}

impl Plan<'_> {
    fn eval(&self, design: &Design, out: &[Option<NetVoltage>]) -> (Span, Why) {
        let name = |n: usize| design.nets[n].name.as_str();
        let who = format!("{} ({})", self.reference, self.part.mpn);
        let src = cite(self.part, &self.drive.range);
        match &self.how {
            How::Fails(reason) => (
                Span::unknown(),
                Why {
                    text: format!("{who}: {reason}"),
                    from: vec![],
                },
            ),
            How::Forward { anode } => {
                let a = span_of(out, *anode);
                let (vf_lo, vf_hi, lo_note) = match &self.drive.range {
                    Range::Known { min, max, .. } => {
                        let hi = max.resolve(|_| None);
                        match min.resolve(|_| None) {
                            Some(lo) => (lo, hi, String::new()),
                            None => (0.0, hi, "; the smallest drop isn't given, so it is taken as 0 V".into()),
                        }
                    }
                    Range::Unknown(_) => (0.0, None, "; the drop isn't given".into()),
                };
                let span = Span {
                    min: a.min.zip(vf_hi).map(|(v, d)| (v - d).max(0.0)),
                    max: a.max.map(|v| v - vf_lo),
                };
                let hi_text = vf_hi.map_or("an unknown amount".into(), |v| format!("up to {v} V"));
                (
                    span,
                    Why {
                        text: format!(
                            "{who} conducts from {} to {}, dropping {hi_text} ({src}){lo_note}",
                            name(*anode),
                            name(self.target)
                        ),
                        from: vec![*anode],
                    },
                )
            }
            How::Switches { supply, ground } => {
                if span_of(out, *ground) != Span::exact(0.0) {
                    return not_ground(&who, name(*ground), *ground);
                }
                let s = span_of(out, *supply);
                let sup = self.drive.supply.as_deref().unwrap_or("");
                let (min, max) = match &self.drive.range {
                    Range::Known { min, max, .. } => (bound_at(min, sup, s.min), bound_at(max, sup, s.max)),
                    Range::Unknown(_) => (None, None),
                };
                let mut min_note = String::new();
                if let Range::Known { min: lo, max: hi, .. } = &self.drive.range {
                    for (end, b, v) in [("lowest", lo, min), ("highest", hi, max)] {
                        match b {
                            Bound::Unknown(r) => min_note += &format!("; {end} point UNKNOWN: {r}"),
                            _ if v.is_none() => {
                                min_note +=
                                    &format!("; {end} point UNKNOWN: it follows a pin whose voltage isn't known")
                            }
                            _ => {}
                        }
                    }
                }
                (
                    Span { min, max },
                    Why {
                        text: format!(
                            "{who} switches {} from {} ({src}){min_note}",
                            self.drive.pin,
                            name(*supply)
                        ),
                        from: vec![*supply],
                    },
                )
            }
            How::Regulates {
                supply,
                ground,
                divider,
                on_output,
            } => {
                if span_of(out, *ground) != Span::exact(0.0) {
                    return not_ground(&who, name(*ground), *ground);
                }
                let Some((dlo, dhi)) = self.drive.range.values() else {
                    return (
                        Span::unknown(),
                        Why {
                            text: format!("{who}: UNKNOWN: the regulated range of {} isn't given", self.drive.pin),
                            from: vec![],
                        },
                    );
                };
                let s = span_of(out, *supply);
                let sup = self.drive.supply.as_deref().unwrap_or("");
                let rec = recommended(self.part, sup);
                // The regulated output, which is what VIN has to stay above.
                let (olo, ohi) = divider.as_ref().map_or((dlo, dhi), |d| d.output(dlo, dhi));
                let (lo, hi) = if *on_output { (olo, ohi) } else { (dlo, dhi) };
                // The share of the output the pin sees, when regulation fails and the pin follows VIN.
                let ratio_hi = match (divider, on_output) {
                    (Some(d), false) => d.rb.1 / (d.rb.1 + d.rt.0),
                    _ => 1.0,
                };
                let mut notes = vec![];
                let max = match (s.max, rec.1) {
                    (Some(sm), Some(rm)) if sm <= rm => Some(hi.min(sm * ratio_hi)),
                    (Some(sm), rm) => {
                        notes.push(match rm {
                            Some(rm) => format!(
                                "{} can reach {} V, above its recommended {} V, where regulation isn't specified, so the top end follows it",
                                name(*supply),
                                round(sm),
                                rm
                            ),
                            None => format!(
                                "{}'s recommended maximum isn't known, so regulation can't be assumed at its top end of {} V",
                                sup,
                                round(sm)
                            ),
                        });
                        Some(sm * ratio_hi)
                    }
                    (None, _) => {
                        notes.push(format!("{}'s maximum is UNKNOWN", name(*supply)));
                        None
                    }
                };
                let head = self.drive.headroom.as_ref();
                // The lowest supply the range is specified at.
                let needs = head.map(|h| (ohi + h.volts) / h.duty);
                let min = match (s.min, rec.0, needs) {
                    (Some(sn), Some(rn), Some(nd)) if sn >= rn && sn >= nd => {
                        let h = head.expect("needs comes from headroom");
                        notes.push(format!(
                            "{} stays at or above {} V, which covers the {} V the output needs ({} p{}{}); line and \
                             load regulation aren't modelled",
                            name(*supply),
                            round(sn),
                            round(nd),
                            self.part.mpn,
                            h.page,
                            h.note.as_deref().map(|n| format!(", {n}")).unwrap_or_default()
                        ));
                        Some(lo)
                    }
                    (Some(sn), rn, nd) => {
                        notes.push(format!(
                            "{} can fall to {} V, {}, so the output isn't held and its low end is taken as 0 V",
                            name(*supply),
                            round(sn),
                            match (rn, nd) {
                                (Some(rn), _) if sn < rn => format!("below its recommended minimum of {rn} V"),
                                (None, _) => "and its recommended minimum isn't known".into(),
                                (_, Some(nd)) => format!("below the {} V the output is specified at", round(nd)),
                                (_, None) => "and the part file gives no headroom the output needs".into(),
                            }
                        ));
                        Some(0.0)
                    }
                    (None, _, _) => {
                        notes.push(format!("{}'s minimum is UNKNOWN", name(*supply)));
                        None
                    }
                };
                let what = match (divider, on_output) {
                    (Some(d), true) => format!(
                        "{who} holds {} at {dlo} to {dhi} V ({src}); through the divider {} over {} ({}) the output is {} to {} V",
                        self.drive.pin,
                        d.top,
                        d.bottom,
                        d.note,
                        round(olo),
                        round(ohi)
                    ),
                    _ => format!("{who} holds {} at {dlo} to {dhi} V ({src})", self.drive.pin),
                };
                (
                    Span { min, max },
                    Why {
                        text: format!("{what}, powered from {}: {}", name(*supply), notes.join("; ")),
                        from: vec![*supply],
                    },
                )
            }
        }
    }
}

fn not_ground(who: &str, net: &str, n: usize) -> (Span, Why) {
    (
        Span::unknown(),
        Why {
            text: format!("{who}: UNKNOWN: its reference pin is on {net}, which isn't a 0 V ground"),
            from: vec![n],
        },
    )
}

/// A bound of a drive range, where `{ pin = supply }` reads the supply net's value.
fn bound_at(b: &Bound, supply: &str, v: Option<f64>) -> Option<f64> {
    b.resolve(|p| if p == supply { v } else { None })
}

/// The recommended range of a supply pin, as numbers, when its part file gives them.
fn recommended(part: &Part, pin: &str) -> (Option<f64>, Option<f64>) {
    for l in part.limits_for(Quantity::Voltage, pin) {
        if let Range::Known { min, max, .. } = &l.recommended {
            return (min.resolve(|_| None), max.resolve(|_| None));
        }
    }
    (None, None)
}

/// Nets nothing drives directly, joined through resistors, inductors, LEDs and jumpers. They share one range:
/// the union of everything that can pull any of them.
struct Group {
    nets: Vec<usize>,
    pulls: Vec<Pull>,
}

enum Pull {
    /// Through a two-pin part to a driven net.
    Net {
        via: String,
        net: usize,
    },
    /// An IC's output pin, which reaches at most the part's highest supply.
    Output {
        pin: String,
        supplies: Vec<usize>,
    },
    Unknown(String),
}

impl Group {
    fn deps(&self) -> Vec<usize> {
        self.pulls
            .iter()
            .flat_map(|p| match p {
                Pull::Net { net, .. } => vec![*net],
                Pull::Output { supplies, .. } => supplies.clone(),
                Pull::Unknown(_) => vec![],
            })
            .collect()
    }

    fn eval(&self, design: &Design, out: &[Option<NetVoltage>]) -> NetVoltage {
        let name = |n: usize| design.nets[n].name.as_str();
        if self.pulls.is_empty() {
            let pins: Vec<String> = self
                .nets
                .iter()
                .flat_map(|&n| design.nets[n].pins.iter().map(|(r, p)| format!("{r}.{p}")))
                .collect();
            let text = if pins.len() <= 1 {
                format!("UNKNOWN: only {} is on this net, so nothing sets it", pins.join(""))
            } else {
                format!(
                    "UNKNOWN: nothing drives it: no input, regulator, output pin or resistor path reaches {}",
                    pins.join(", ")
                )
            };
            return unknown(text, vec![]);
        }
        let mut nv: Option<NetVoltage> = None;
        let shared = if self.nets.len() > 1 {
            let names: Vec<&str> = self.nets.iter().map(|&n| name(n)).collect();
            format!(" (shared by {})", names.join(", "))
        } else {
            String::new()
        };
        for p in &self.pulls {
            let (span, why) = match p {
                Pull::Net { via, net } => (
                    span_of(out, *net),
                    Why {
                        text: format!("pulled through {via} toward {}{shared}", name(*net)),
                        from: vec![*net],
                    },
                ),
                Pull::Output { pin, supplies } => {
                    let tops: Vec<Option<f64>> = supplies.iter().map(|&s| span_of(out, s).max).collect();
                    let top = tops.iter().try_fold(f64::NEG_INFINITY, |a, t| t.map(|t| a.max(t)));
                    (
                        Span {
                            min: Some(0.0),
                            max: top,
                        },
                        Why {
                            text: format!(
                                "driven by output pin {pin}, which reaches at most its part's highest supply{shared}"
                            ),
                            from: supplies.clone(),
                        },
                    )
                }
                Pull::Unknown(reason) => (
                    Span::unknown(),
                    Why {
                        text: format!("UNKNOWN: {reason}{shared}"),
                        from: vec![],
                    },
                ),
            };
            merge(&mut nv, span, why);
        }
        nv.expect("a group with pulls has a range")
    }
}

fn build_groups(design: &Design, lib: &Library, inputs: &BoardInputs, strong: &[bool]) -> Vec<Group> {
    let n = design.nets.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        p[x] = r;
        r
    }
    for c in design.components.values() {
        if c.dnp || !is_link(lib, c) {
            continue;
        }
        let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
        if let [a, b] = nets[..]
            && !strong[a]
            && !strong[b]
        {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            parent[ra] = rb;
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in (0..n).filter(|&i| !strong[i]) {
        let r = find(&mut parent, i);
        members.entry(r).or_default().push(i);
    }
    members
        .into_values()
        .map(|nets| {
            let mut pulls = vec![];
            for &net in &nets {
                for (r, pnum) in &design.nets[net].pins {
                    let Some(c) = design.components.get(r) else { continue };
                    if c.dnp {
                        continue;
                    }
                    let Some(pin) = c.pins.get(pnum) else { continue };
                    if is_link(lib, c) {
                        let other = c.pins.values().map(|q| q.net).find(|&q| q != net);
                        if let Some(o) = other
                            && strong[o]
                        {
                            pulls.push(Pull::Net { via: r.clone(), net: o });
                        }
                    } else if is_connector(lib, c) {
                        if is_undriven(inputs, r, pnum) {
                            continue;
                        }
                        pulls.push(Pull::Unknown(format!(
                            "{r}.{pnum} is a connector pin with no range in inputs.toml"
                        )));
                    } else {
                        match pin.kind {
                            PinKind::Output
                            | PinKind::Bidirectional
                            | PinKind::TriState
                            | PinKind::OpenCollector
                            | PinKind::OpenEmitter => {
                                let supplies: Vec<usize> = c
                                    .pins
                                    .values()
                                    .filter(|q| q.kind == PinKind::PowerIn)
                                    .map(|q| q.net)
                                    .collect::<BTreeSet<_>>()
                                    .into_iter()
                                    .collect();
                                if supplies.is_empty() {
                                    pulls.push(Pull::Unknown(format!(
                                        "{r}.{pnum} is an output, and {r} has no power pin to bound it"
                                    )));
                                } else {
                                    pulls.push(Pull::Output {
                                        pin: format!("{r}.{pnum}"),
                                        supplies,
                                    });
                                }
                            }
                            PinKind::PowerOut => pulls.push(Pull::Unknown(format!(
                                "{r}.{pnum} is a power output, and no part file says what it drives"
                            ))),
                            // Capacitors are open, a TVS only conducts past its breakdown (a check's job),
                            // and power and logic inputs only take.
                            PinKind::PowerIn | PinKind::Input | PinKind::NoConnect => {}
                            _ if is_capacitor(lib, c) || class_of(lib, c) == Some(Class::Tvs) => {}
                            _ => pulls.push(Pull::Unknown(format!(
                                "{r}.{pnum} is a {:?} pin on a part gerberus can't model, so it may pull the net \
                                 anywhere",
                                pin.kind
                            ))),
                        }
                    }
                }
            }
            Group { nets, pulls }
        })
        .collect()
}
