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
use crate::inputs::{BoardInputs, Follow, Input, Max};
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
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Why {
    pub text: String,
    /// Nets this step read, so an explanation can show where their ranges came from.
    pub from: Vec<usize>,
    /// What this step sets never rises above the nets in `from`: a resistor pull, an output pin of a part they
    /// supply, a diode or regulator fed from them.
    pub bounded: bool,
    /// This step's low end is only a floor, not a value the net is known to reach.
    pub low_floor: bool,
}

/// What sets a net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Driver {
    /// An input, or a part's drive from its part file.
    #[default]
    Strong,
    /// A resistor path to a driven net, or a push-pull or bidirectional output pin.
    Pulled,
    /// Only open-drain or tri-state outputs, connector pins or parts gerberus can't model: nothing proves the
    /// net is driven.
    Weak,
    /// Nothing at all.
    Floating,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NetVoltage {
    pub span: Span,
    pub why: Vec<Why>,
    /// What sets the net.
    pub driver: Driver,
    /// The low end is only a floor the net can't go below, not a value it is known to reach: a regulator that
    /// isn't held sags somewhere above 0 V. A check can pass on it but must not fail on it.
    pub floor: bool,
    /// Nets this one is joined to only through resistors, LEDs and jumpers, with nothing else driving it and no
    /// capacitor or inductor on it. A node of such parts always sits between the voltages around it, so at
    /// every moment this net is between the lowest and highest of these. Empty when that isn't known: a
    /// capacitor holds a node's old voltage and an inductor overshoots, so either can take it outside them.
    pub between: Vec<usize>,
}

/// Whether a regulator's supply leaves it enough room to hold its output.
#[derive(Debug, Clone, PartialEq)]
pub enum Hold {
    Held(String),
    NotHeld(String),
    Unknown(String),
}

/// One regulator's output, and whether its supply lets it hold it.
#[derive(Debug, Clone, PartialEq)]
pub struct Regulation {
    pub reference: String,
    pub mpn: String,
    /// The net the regulator sets: its output, through the divider when it has one.
    pub output: usize,
    pub supply: usize,
    pub hold: Hold,
    /// The regulated pin, from the part file.
    pub pin: String,
    /// False for a feedback pin's own net, which a report needn't list separately from the output.
    pub sets_output: bool,
}

/// The range of every net in a design, indexed like [`Design::nets`].
#[derive(Debug, Clone)]
pub struct Voltages {
    pub nets: Vec<NetVoltage>,
    pub regulators: Vec<Regulation>,
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

    /// Whether `net` can never rise above `reference`: it is the same net, or every reason for its range reads
    /// only nets that can't rise above `reference` and 0 V grounds. A resistor pull-up to a rail, an output pin
    /// of a part that `reference` alone supplies, a regulator fed from it and a diode from it all qualify.
    pub fn stays_below(&self, net: usize, reference: usize) -> bool {
        self.stays_below_depth(net, reference, 0)
    }

    fn stays_below_depth(&self, net: usize, reference: usize, depth: usize) -> bool {
        if net == reference {
            return true;
        }
        if depth > 16 {
            return false;
        }
        let nv = &self.nets[net];
        !nv.why.is_empty()
            && nv.why.iter().all(|w| {
                w.bounded
                    && !w.from.is_empty()
                    && w.from.iter().all(|&f| {
                        self.nets[f].span == Span::exact(0.0) || self.stays_below_depth(f, reference, depth + 1)
                    })
            })
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

    // 1. Inputs, and the pins they are measured from, which are 0 V. Fault inputs belong to their own case
    //    (see `BoardInputs::case`). An input's own net is set once everything its range reads is known.
    let mut fixed = vec![false; n];
    let mut seeds: BTreeMap<usize, Vec<&Input>> = BTreeMap::new();
    for i in inputs.inputs.iter().filter(|i| i.fault.is_none()) {
        let (Some(pn), Some(tn)) = (net_of(&i.pin.reference, &i.pin.pin), net_of(&i.to.reference, &i.to.pin)) else {
            continue;
        };
        merge(
            &mut out[tn],
            Span::exact(0.0),
            Why {
                text: format!("0 V: the reference of input {} ({})", i.name, i.to),
                ..Default::default()
            },
        );
        seeds.entry(pn).or_default().push(i);
        fixed[tn] = true;
        fixed[pn] = true;
    }
    let follows = |i: &Input| match &i.max {
        Max::Value(_) => None,
        Max::Pin(f) => net_of(&f.pin.reference, &f.pin.pin),
    };
    let seed_deps = |is: &[&Input]| -> Vec<usize> { is.iter().filter_map(|i| follows(i)).collect() };

    // 2. What fitted parts drive, from their part files.
    let mut plans: BTreeMap<usize, Vec<Plan<'_>>> = BTreeMap::new();
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

    // 3. Nets fed through a ferrite bead from a driven net: a bead is a DC short (well under an ohm), so the net
    //    takes the driven net's range. Beads only: an inductor may be a switcher's, with a switch node on one side.
    let mut strong: Vec<bool> = (0..n).map(|i| fixed[i] || plans.contains_key(&i)).collect();
    let mut beads: BTreeMap<usize, Vec<(&str, usize)>> = BTreeMap::new();
    loop {
        let mut grew = false;
        for (r, c) in &design.components {
            if c.dnp || !is_bead(c) {
                continue;
            }
            let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
            if let [a, b] = nets[..] {
                for (from, to) in [(a, b), (b, a)] {
                    if strong[from] && !strong[to] && !beads.contains_key(&to) {
                        beads.entry(to).or_default().push((r.as_str(), from));
                    }
                }
            }
        }
        // A net reached through several beads takes them all, so add them in one pass before marking.
        for (r, c) in &design.components {
            if c.dnp || !is_bead(c) {
                continue;
            }
            let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
            if let [a, b] = nets[..] {
                for (from, to) in [(a, b), (b, a)] {
                    if strong[from]
                        && let Some(list) = beads.get_mut(&to)
                        && !strong[to]
                        && !list.iter().any(|&(x, f)| x == r.as_str() && f == from)
                    {
                        list.push((r.as_str(), from));
                    }
                }
            }
        }
        for &t in beads.keys() {
            if !strong[t] {
                strong[t] = true;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    // 4. Groups of the remaining nets, joined by resistors, inductors, LEDs and jumpers.
    let groups = build_groups(design, lib, inputs, &strong);

    // 5. Resolve whatever has everything it reads, until nothing changes.
    let mut regulators = vec![];
    let mut done_groups = vec![false; groups.len()];
    loop {
        let mut progress = false;
        for (&t, is) in &seeds {
            if out[t].is_some() || !seed_deps(is).iter().all(|&d| out[d].is_some()) {
                continue;
            }
            let mut nv: Option<NetVoltage> = None;
            for i in is {
                let (span, why) = input_range(design, i, follows(i), &out);
                merge(&mut nv, span, why);
            }
            out[t] = nv;
            progress = true;
        }
        for (&t, ps) in &plans {
            if out[t].is_some() || !ps.iter().all(|p| p.deps.iter().all(|&d| out[d].is_some())) {
                continue;
            }
            let mut nv: Option<NetVoltage> = None;
            for p in ps {
                let (span, why) = p.eval(design, &out, &mut regulators);
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
                        ..Default::default()
                    });
                }
            }
            out[t] = nv;
            progress = true;
        }
        for (&t, list) in &beads {
            if out[t].is_some() || !list.iter().all(|&(_, f)| out[f].is_some()) {
                continue;
            }
            let mut nv: Option<NetVoltage> = None;
            for &(r, f) in list {
                let from = out[f].as_ref().expect("checked above");
                merge(
                    &mut nv,
                    from.span,
                    Why {
                        text: format!("through ferrite bead {r} from {}, a DC short", design.nets[f].name),
                        from: vec![f],
                        bounded: true,
                        low_floor: from.floor,
                    },
                );
            }
            if let Some(nv) = nv.as_mut() {
                nv.driver = list
                    .iter()
                    .map(|&(_, f)| out[f].as_ref().map_or(Driver::Floating, |o| o.driver))
                    .min_by_key(|d| *d as u8)
                    .unwrap_or(Driver::Floating);
                // As for a plan's net: a connector pin here can bring in anything.
                for pin in open_connectors(design, lib, inputs, t) {
                    nv.span = Span::unknown();
                    nv.why.push(Why {
                        text: format!(
                            "UNKNOWN: {pin} is a connector pin with no range in inputs.toml, so whatever is \
                             plugged in there can push this net anywhere"
                        ),
                        ..Default::default()
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

    // 6. Anything left reads a net that, in turn, reads it.
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
    for (&t, is) in &seeds {
        if out[t].is_none() {
            late.push((t, unknown(stuck(&seed_deps(is), &out), vec![])));
        }
    }
    for (&t, ps) in &plans {
        if out[t].is_none() {
            let deps: Vec<usize> = ps.iter().flat_map(|p| p.deps.clone()).collect();
            late.push((t, unknown(stuck(&deps, &out), vec![])));
        }
    }
    for (&t, list) in &beads {
        if out[t].is_none() {
            let deps: Vec<usize> = list.iter().map(|&(_, f)| f).collect();
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
        regulators,
    }
}

/// One input's range, and why. `follows` is the net its top follows, if it follows one.
fn input_range(design: &Design, i: &Input, follows: Option<usize>, out: &[Option<NetVoltage>]) -> (Span, Why) {
    let head = format!("input {} on {} from {}", i.name, i.pin, i.to);
    match (&i.max, follows) {
        (Max::Value(max), _) => (
            Span::new(i.min, *max),
            Why {
                text: format!("{head}: {} to {max} V ({})", i.min, i.source),
                ..Default::default()
            },
        ),
        (Max::Pin(Follow { pin, plus }), Some(net)) => {
            let top = span_of(out, net).max.map(|m| m + plus);
            let plus_text = if *plus == 0.0 {
                String::new()
            } else {
                format!(" {plus:+} V")
            };
            let span = Span {
                min: Some(i.min),
                max: top,
            };
            let range = match top {
                Some(t) if t >= i.min => format!("{span}"),
                Some(_) => "UNKNOWN, as the pin it follows can sit below the input's minimum".into(),
                None => "UNKNOWN".into(),
            };
            let span = if top.is_some_and(|t| t < i.min) {
                Span::unknown()
            } else {
                span
            };
            (
                span,
                Why {
                    text: format!(
                        "{head}: {} V up to the voltage on {pin} ({}){plus_text}, so {range} ({})",
                        i.min, design.nets[net].name, i.source
                    ),
                    from: vec![net],
                    bounded: *plus <= 0.0 && i.min <= 0.0,
                    low_floor: false,
                },
            )
        }
        (Max::Pin(Follow { pin, .. }), None) => (
            Span::unknown(),
            Why {
                text: format!("UNKNOWN: {head} follows {pin}, which isn't in the design"),
                ..Default::default()
            },
        ),
    }
}

fn unknown(text: String, from: Vec<usize>) -> NetVoltage {
    NetVoltage {
        span: Span::unknown(),
        why: vec![Why {
            text,
            from,
            ..Default::default()
        }],
        driver: Driver::Strong,
        floor: false,
        between: vec![],
    }
}

/// Adds one source to a net. The low end stays a floor only while every source that reaches it is a floor.
fn merge(slot: &mut Option<NetVoltage>, span: Span, why: Why) {
    match slot {
        Some(nv) => {
            nv.floor = match (nv.span.min, span.min) {
                (Some(a), Some(b)) if b < a => why.low_floor,
                (Some(a), Some(b)) if b == a => nv.floor && why.low_floor,
                (Some(_), Some(_)) => nv.floor,
                _ => false,
            };
            nv.span = nv.span.union(span);
            nv.why.push(why);
        }
        None => {
            *slot = Some(NetVoltage {
                span,
                floor: why.low_floor && span.min.is_some(),
                why: vec![why],
                driver: Driver::Strong,
                between: vec![],
            });
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

/// A ferrite bead, by its symbol (`Device:FerriteBead*`, `Device:Ferrite*`) or its reference (`FB1`).
fn is_bead(c: &Component) -> bool {
    let letters: String = c.reference.chars().take_while(char::is_ascii_alphabetic).collect();
    c.pins.len() == 2 && (c.lib_id.starts_with("Device:Ferrite") || letters == "FB")
}

/// An inductor or ferrite bead by its symbol (`Device:L*`, `Device:FerriteBead*`, `Device:Ferrite*`).
fn is_inductor_symbol(c: &Component) -> bool {
    c.lib_id.starts_with("Device:L") || c.lib_id.starts_with("Device:Ferrite")
}

/// A two-pin part a net's voltage passes through, for the fallback range: resistors, inductors and ferrite
/// beads (and so LEDs, `Device:L*`), and jumpers, which may be closed.
fn is_link(lib: &Library, c: &Component) -> bool {
    c.pins.len() == 2
        && (c.lib_id.starts_with("Device:R")
            || is_inductor_symbol(c)
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

/// The smallest and largest a resistor can be: its value, off by its tolerance plus its temperature drift over
/// the board's temperature range (or its own rated range when `inputs.toml` gives none). Also says where the
/// numbers come from.
pub fn resistance_range(
    lib: &Library,
    inputs: &BoardInputs,
    r: &str,
    c: &Component,
) -> Result<(f64, f64, String), String> {
    let Some(ohms) = parse_ohms(&c.value).filter(|&v| v > 0.0) else {
        return Err(format!("can't read a resistance from {r}'s value {:?}", c.value));
    };
    let Some(part) = lib.part_for(c) else {
        return Err(format!(
            "{r} ({}) has no part file, so its tolerance is unknown",
            crate::limits::component_mpn(c)
        ));
    };
    let Some(tol) = &part.tolerance else {
        return Err(format!("{r}'s part file {} gives no tolerance", part.mpn));
    };
    let Some(tcr) = &part.tcr else {
        return Err(format!("{r}'s part file {} gives no temperature coefficient", part.mpn));
    };
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
            return Err(format!(
                "neither inputs.toml nor {r}'s part file gives a temperature range, so its drift is unknown"
            ));
        }
    };
    let drift = tcr.ppm * 1e-6 * (tmax - 25.0).abs().max((25.0 - tmin).abs());
    let dev = tol.value + drift;
    let doc = part.datasheet.as_ref().map_or("", |d| d.document.as_str());
    let note = format!(
        "{r} {} ±{}% (p{}) plus {} ppm/°C (p{}) over {twhere}, so ±{}% ({} {doc})",
        c.value,
        round(tol.value * 100.0),
        tol.page,
        tcr.ppm,
        tcr.page,
        round(dev * 100.0),
        part.mpn,
    );
    Ok((ohms * (1.0 - dev), ohms * (1.0 + dev), note))
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
    let mut ends = vec![];
    let mut notes = vec![];
    for (r, c) in [(rt_ref, rt), (rb_ref, rb)] {
        match resistance_range(lib, inputs, r, c) {
            Ok((lo, hi, note)) => {
                ends.push((lo, hi));
                notes.push(note);
            }
            Err(e) => return Err((format!("UNKNOWN: {e}"), Some(out))),
        }
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
    fn eval(&self, design: &Design, out: &[Option<NetVoltage>], regulators: &mut Vec<Regulation>) -> (Span, Why) {
        let name = |n: usize| design.nets[n].name.as_str();
        let who = format!("{} ({})", self.reference, self.part.mpn);
        let src = cite(self.part, &self.drive.range);
        match &self.how {
            How::Fails(reason) => {
                if self.drive.kind == DriveKind::Regulates {
                    regulators.push(self.regulation(
                        Hold::Unknown(reason.clone()),
                        *supply_of(&self.how).unwrap_or(&self.target),
                        true,
                    ));
                }
                (
                    Span::unknown(),
                    Why {
                        text: format!("{who}: {reason}"),
                        ..Default::default()
                    },
                )
            }
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
                let raw_min = a.min.zip(vf_hi).map(|(v, d)| v - d);
                // A diode only lets its anode pull the cathode up: with the anode below 0 V (a supply connected
                // backwards) it blocks, and the cathode is left at the 0 V floor.
                let span = Span {
                    min: raw_min.map(|v| v.max(0.0)),
                    max: a.max.map(|v| (v - vf_lo).max(0.0)),
                };
                // Below the drop the diode is off, so 0 V is only a floor; so is a floor on the anode.
                let low_floor = raw_min.is_some_and(|v| v < 0.0) || out[*anode].as_ref().is_some_and(|nv| nv.floor);
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
                        bounded: true,
                        low_floor,
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
                            Bound::Unknown(r) => {
                                let _ = write!(min_note, "; {end} point UNKNOWN: {r}");
                            }
                            _ if v.is_none() => {
                                let _ = write!(
                                    min_note,
                                    "; {end} point UNKNOWN: it follows a pin whose voltage isn't known"
                                );
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
                        // Bounded by the supply only when the datasheet says it swings no higher.
                        bounded: matches!(&self.drive.range, Range::Known { max: Bound::Pin { pin, plus, .. }, .. }
                            if pin == sup && *plus <= 0.0),
                        low_floor: false,
                    },
                )
            }
            How::Regulates {
                supply,
                ground,
                divider,
                on_output,
            } => {
                let sets_output = *on_output || divider.is_none();
                if span_of(out, *ground) != Span::exact(0.0) {
                    regulators.push(self.regulation(
                        Hold::Unknown(format!(
                            "its reference pin is on {}, which isn't a 0 V ground",
                            name(*ground)
                        )),
                        *supply,
                        sets_output,
                    ));
                    return not_ground(&who, name(*ground), *ground);
                }
                let Some((dlo, dhi)) = self.drive.range.values() else {
                    let text = format!("the regulated range of {} isn't given", self.drive.pin);
                    regulators.push(self.regulation(Hold::Unknown(text.clone()), *supply, sets_output));
                    return (
                        Span::unknown(),
                        Why {
                            text: format!("{who}: UNKNOWN: {text}"),
                            ..Default::default()
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
                let hold;
                let min = match (s.min, rec.0, needs) {
                    (Some(sn), Some(rn), Some(nd)) if sn >= rn && sn >= nd => {
                        hold = Hold::Held(format!(
                            "{} stays at or above {} V; the output needs {} V and {} at least {rn} V",
                            name(*supply),
                            round(sn),
                            round(nd),
                            sup
                        ));
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
                        let (why, known) = match (rn, nd) {
                            (Some(rn), _) if sn < rn => (format!("below its recommended minimum of {rn} V"), true),
                            (None, _) => ("and its recommended minimum isn't known".into(), false),
                            (_, Some(nd)) => (format!("below the {} V the output is specified at", round(nd)), true),
                            (_, None) => ("and the part file gives no headroom the output needs".into(), false),
                        };
                        let text = format!("{} can fall to {} V, {why}", name(*supply), round(sn));
                        hold = if known {
                            Hold::NotHeld(text.clone())
                        } else {
                            Hold::Unknown(text.clone())
                        };
                        notes.push(format!(
                            "{text}, so the output isn't held: 0 V is only a floor for its low end, as nothing says how far it sags"
                        ));
                        Some(0.0)
                    }
                    (None, _, _) => {
                        hold = Hold::Unknown(format!("{}'s minimum is UNKNOWN", name(*supply)));
                        notes.push(format!("{}'s minimum is UNKNOWN", name(*supply)));
                        None
                    }
                };
                // A supply below 0 V (connected backwards) can pull the output down with it, through the body
                // diode many CMOS regulators have from output to input. Part files don't say whether this one
                // has it, so the supply's low end is only a floor for the output, and nothing drives it above 0 V.
                let (min, max) = match s.min {
                    Some(sn) if sn < 0.0 => {
                        notes.push(format!(
                            "{} can fall to {} V, below 0 V: the output may follow it down through the regulator, as                              far as the part file says nothing to rule out",
                            name(*supply),
                            round(sn)
                        ));
                        (Some(sn * ratio_hi), max.map(|m| m.max(0.0)))
                    }
                    _ => (min, max),
                };
                let held = matches!(hold, Hold::Held(_));
                regulators.push(self.regulation(hold, *supply, sets_output));
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
                        bounded: true,
                        low_floor: !held,
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
            ..Default::default()
        },
    )
}

fn supply_of(how: &How) -> Option<&usize> {
    match how {
        How::Regulates { supply, .. } | How::Switches { supply, .. } => Some(supply),
        _ => None,
    }
}

impl Plan<'_> {
    fn regulation(&self, hold: Hold, supply: usize, sets_output: bool) -> Regulation {
        Regulation {
            reference: self.reference.to_string(),
            mpn: self.part.mpn.clone(),
            output: self.target,
            supply,
            hold,
            pin: self.drive.pin.clone(),
            sets_output,
        }
    }
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
    /// A capacitor or inductor touches the group, so it can leave the range of the nets around it.
    reactive: bool,
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
        /// A push-pull or bidirectional pin, which can drive both ways; open-drain and tri-state pins can't
        /// be relied on to.
        push_pull: bool,
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
            let mut nv = unknown(text, vec![]);
            nv.driver = Driver::Floating;
            return nv;
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
                        bounded: true,
                        low_floor: out[*net].as_ref().is_some_and(|nv| nv.floor),
                    },
                ),
                Pull::Output { pin, supplies, .. } => {
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
                            bounded: true,
                            low_floor: false,
                        },
                    )
                }
                Pull::Unknown(reason) => (
                    Span::unknown(),
                    Why {
                        text: format!("UNKNOWN: {reason}{shared}"),
                        ..Default::default()
                    },
                ),
            };
            merge(&mut nv, span, why);
        }
        let mut nv = nv.expect("a group with pulls has a range");
        let real = self
            .pulls
            .iter()
            .any(|p| matches!(p, Pull::Net { .. }) || matches!(p, Pull::Output { push_pull, .. } if *push_pull));
        nv.driver = if real { Driver::Pulled } else { Driver::Weak };
        if !self.reactive && self.pulls.iter().all(|p| matches!(p, Pull::Net { .. })) {
            nv.between = self
                .pulls
                .iter()
                .filter_map(|p| match p {
                    Pull::Net { net, .. } => Some(*net),
                    _ => None,
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        nv
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
            let mut reactive = false;
            for &net in &nets {
                for (r, pnum) in &design.nets[net].pins {
                    let Some(c) = design.components.get(r) else { continue };
                    if c.dnp {
                        continue;
                    }
                    if is_capacitor(lib, c) || is_inductor_symbol(c) || class_of(lib, c) == Some(Class::Inductor) {
                        reactive = true;
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
                                        push_pull: matches!(pin.kind, PinKind::Output | PinKind::Bidirectional),
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
            Group { nets, pulls, reactive }
        })
        .collect()
}
