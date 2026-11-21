//! The worst-case voltage range on every net, and why.
//!
//! Ranges start at the board inputs (`inputs.toml`) and spread through what the part files say parts do:
//!
//! - a diode's forward drop (`[[drive]] kind = "forward"`),
//! - a regulator's output or feedback pin (`kind = "regulates"`), through a two-resistor divider when the
//!   feedback pin has one, using each resistor's tolerance from its part file,
//! - a switch node (`kind = "switches"`),
//! - an input the part biases itself, when only capacitors reach it (`kind = "biases"`),
//! - a MOSFET channel (`kind = "channel"`), from its gate against both ends of the channel: off when the gate
//!   never passes either end in the direction that turns it on, fully on when it is always past the VGS that
//!   RDS(on) is given at, and otherwise maybe on. A channel that isn't off pulls each end toward the other, but
//!   no further than the gate (an N-channel part can't raise a net above its gate, a P-channel one can't lower
//!   it below), as its threshold is taken as 0 V: the datasheet's minimum holds only at 25 °C. A MOSFET with
//!   no part file stays a part gerberus can't model.
//!
//! A net none of those reach gets the range of everything that can pull it: nets it reaches through
//! resistors, inductors, LEDs and jumpers, and IC output pins, which reach at most their part's highest supply.
//! Capacitors are treated as open.
//!
//! Nothing is guessed. When a step needs a number nobody gave (a connector pin with no input, a resistor with
//! no tolerance, a datasheet that gives only one end), that end of the range is unknown and the reason says
//! why. Checkers must report UNKNOWN for it, never pass.
mod channels;
mod groups;
mod plan;

pub use channels::fet_pins;
use channels::{decide, find_channels};
use groups::build_groups;
pub use plan::resistance_range;
use plan::{Plan, plan_drive};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

use crate::design::{Component, Design};
use crate::inputs::{BoardInputs, Follow, Input, Max};
use crate::limits::{Class, DriveKind, Library, Part, Range};

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
    /// Only open-drain or tri-state outputs, switches, connector pins or parts gerberus can't model: nothing proves the
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
    /// The one resistor that sets this net, and the net on its other end, when nothing else drives it (a
    /// supply filtered through a series resistor). In steady state that resistor drops only the current the
    /// net's parts draw. Capacitors on the net don't change that, only how it gets there at power-on.
    pub fed_through: Option<(String, usize)>,
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
    /// The output's range while it is held, and the lowest supply that holds it when that is known.
    pub held_at: Option<HeldAt>,
}

/// A regulator's output while it regulates, and the supply it needs for that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldAt {
    pub min: f64,
    pub max: f64,
    /// The lowest supply the output range is specified at, when the part file gives a headroom.
    pub needs: Option<f64>,
}

/// The range of every net in a design, indexed like [`Design::nets`].
#[derive(Debug, Clone)]
pub struct Voltages {
    pub nets: Vec<NetVoltage>,
    pub regulators: Vec<Regulation>,
    /// Every MOSFET channel, and whether it can conduct.
    pub channels: Vec<Channel>,
}

/// Whether a MOSFET channel conducts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelState {
    /// The gate never passes either end of the channel in the direction that turns it on: only the body diode,
    /// if the part file gives one, conducts.
    Off,
    /// It may be on or off, or partly on: each end can be pulled toward the other, as far as the gate allows.
    Maybe,
    /// VGS is always past the voltage RDS(on) is given at: the channel surely conducts, with a drop that
    /// depends on a current gerberus doesn't know.
    On,
}

/// One MOSFET's channel, from its part file's `channel` drive.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub reference: String,
    pub drain: usize,
    pub source: usize,
    pub gate: Option<usize>,
    pub state: ChannelState,
    /// Why it is in this state.
    pub reason: String,
    /// N-channel (true) or P-channel, from the signs of the threshold or `on`; unknown when neither is given.
    n_channel: Option<bool>,
    /// The VGS RDS(on) is given at.
    on: Option<f64>,
    /// The part and the page its numbers come from.
    cite: String,
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
    // Every channel starts as possibly on, which gives each net the widest range it can have. Channels worked out
    // as off or on from those ranges are fixed, and the rest worked out again from the narrower ranges that
    // follow, a few times at most.
    let mut channels = find_channels(design, lib);
    let v = settle_channels(design, lib, inputs, &mut channels);
    let mut v = reopen_stale_channel_decisions(design, lib, inputs, &mut channels, v);
    v.channels = channels;
    v
}

const CHANNEL_SETTLING_PASSES: usize = 4;

fn settle_channels(design: &Design, lib: &Library, inputs: &BoardInputs, channels: &mut [Channel]) -> Voltages {
    let mut v = compute_with(design, lib, inputs, channels);
    for _ in 0..CHANNEL_SETTLING_PASSES {
        let mut changed = false;
        for ch in channels.iter_mut().filter(|c| c.state == ChannelState::Maybe) {
            let (state, reason) = decide(design, &v, ch);
            ch.reason = reason;
            if state != ChannelState::Maybe {
                ch.state = state;
                changed = true;
            }
        }
        if !changed {
            break;
        }
        v = compute_with(design, lib, inputs, channels);
    }
    v
}

fn reopen_stale_channel_decisions(
    design: &Design,
    lib: &Library,
    inputs: &BoardInputs,
    channels: &mut [Channel],
    mut v: Voltages,
) -> Voltages {
    // A decision rests on ranges that the decisions themselves narrow, so check each against the final ranges:
    // one that no longer holds (a gate that ends up floating) goes back to possibly on. States only move back,
    // so this ends.
    loop {
        let mut undone = false;
        for ch in channels.iter_mut().filter(|c| c.state != ChannelState::Maybe) {
            let (state, reason) = decide(design, &v, ch);
            if state != ch.state {
                ch.state = ChannelState::Maybe;
                ch.reason = reason;
                undone = true;
            }
        }
        if !undone {
            break;
        }
        v = compute_with(design, lib, inputs, channels);
    }
    v
}

fn compute_with(design: &Design, lib: &Library, inputs: &BoardInputs, channels: &[Channel]) -> Voltages {
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

    // 3. Nets fed through a ferrite bead, net tie or fuse from a driven net: each is a DC short (well under an ohm), so the net
    //    takes the driven net's range. Not inductors: an inductor may be a switcher's, with a switch node on one side.
    let mut strong: Vec<bool> = (0..n).map(|i| fixed[i] || plans.contains_key(&i)).collect();
    let shorts = find_shorts(design);
    let beads = spread_through_shorts(&shorts, &mut strong);

    // 4. Groups of the remaining nets, joined by resistors, inductors, LEDs, jumpers and channels that may be on.
    let groups = build_groups(design, lib, inputs, &strong, channels);

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
            // A part's own input bias is a high-resistance pull.
            if let Some(nv) = nv.as_mut()
                && ps.iter().all(|p| p.drive.kind == DriveKind::Biases)
            {
                nv.driver = Driver::Pulled;
            }
            // A connector pin on a driven net can bring in anything, unless inputs.toml says nothing outside
            // drives it. (An input sets its whole net, so pins on an input's net are covered by it.)
            if let Some(nv) = nv.as_mut() {
                mark_open_connector_pins(nv, design, lib, inputs, t);
            }
            out[t] = nv;
            progress = true;
        }
        for (&t, list) in &beads {
            if out[t].is_some() || !list.iter().all(|&(_, f)| out[f].is_some()) {
                continue;
            }
            let mut nv: Option<NetVoltage> = None;
            for &(k, f) in list {
                let from = out[f].as_ref().expect("checked above");
                let s = &shorts[k];
                merge(
                    &mut nv,
                    from.span,
                    Why {
                        text: format!("through {} from {}{}", s.via, design.nets[f].name, s.note),
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
                mark_open_connector_pins(nv, design, lib, inputs, t);
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
        channels: vec![],
    }
}

fn find_shorts(design: &Design) -> Vec<Short> {
    let mut shorts: Vec<Short> = vec![];
    for (r, c) in &design.components {
        let nets: Vec<usize> = c.pins.values().map(|p| p.net).collect();
        let Some(what) = short_kind(c).filter(|_| !c.dnp) else {
            continue;
        };
        // A net tie with more than two pins joins every net it touches to the first.
        let first_net = nets[0];
        for &other_net in nets.iter().skip(1).filter(|&&b| b != first_net) {
            shorts.push(Short {
                via: format!("{what} {r}"),
                note: if what == "fuse" {
                    ", a DC short while it is intact".into()
                } else {
                    ", a DC short".into()
                },
                a: first_net,
                b: other_net,
            });
        }
    }
    shorts
}

fn spread_through_shorts(shorts: &[Short], strong: &mut [bool]) -> BTreeMap<usize, Vec<(usize, usize)>> {
    let mut beads: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    loop {
        let reached_this_pass = shorted_nets_reached_from_driven(shorts, strong, &mut beads);
        let mut grew = false;
        for t in reached_this_pass {
            if !strong[t] {
                strong[t] = true;
                grew = true;
            }
        }
        if !grew {
            return beads;
        }
    }
}

fn shorted_nets_reached_from_driven(
    shorts: &[Short],
    strong: &[bool],
    beads: &mut BTreeMap<usize, Vec<(usize, usize)>>,
) -> Vec<usize> {
    // A net reached through several shorts takes them all, so add them in one pass before marking.
    for (k, s) in shorts.iter().enumerate() {
        for (from, to) in [(s.a, s.b), (s.b, s.a)] {
            if strong[from] && !strong[to] {
                let list = beads.entry(to).or_default();
                if !list.iter().any(|&(x, f)| x == k && f == from) {
                    list.push((k, from));
                }
            }
        }
    }
    beads.keys().copied().collect()
}

fn mark_open_connector_pins(nv: &mut NetVoltage, design: &Design, lib: &Library, inputs: &BoardInputs, net: usize) {
    for pin in open_connectors(design, lib, inputs, net) {
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

/// A part that joins two nets with next to no resistance: a ferrite bead, net tie or fuse.
struct Short {
    /// `ferrite bead FB1`.
    via: String,
    /// What follows the net's name in the explanation.
    note: String,
    a: usize,
    b: usize,
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
        fed_through: None,
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
                fed_through: None,
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

/// A connector, by its symbol (`Connector…`) or its part file's class.
pub fn is_connector(lib: &Library, c: &Component) -> bool {
    c.lib_id.starts_with("Connector") || class_of(lib, c) == Some(Class::Connector)
}

fn is_capacitor(lib: &Library, c: &Component) -> bool {
    c.lib_id.starts_with("Device:C") || class_of(lib, c) == Some(Class::Capacitor)
}

/// A two-pin part that is a DC short, and what it is: a ferrite bead, a net tie (`Device:NetTie*`, `NT1`) or a
/// fuse (`Device:Fuse*`, `Device:Polyfuse*`, `F1`).
pub fn short_kind(c: &Component) -> Option<&'static str> {
    let letters: String = c
        .reference
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_uppercase();
    let symbol = c.lib_id.rsplit(':').next().unwrap_or("");
    if c.pins.len() >= 2 && (symbol.starts_with("NetTie") || (letters == "NT" && c.pins.len() == 2)) {
        return Some("net tie");
    }
    if c.pins.len() != 2 {
        return None;
    }
    if is_bead(c) {
        Some("ferrite bead")
    } else if c.lib_id.starts_with("Device:Fuse") || c.lib_id.starts_with("Device:Polyfuse") || letters == "F" {
        Some("fuse")
    } else {
        None
    }
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
