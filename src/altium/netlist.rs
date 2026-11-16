//! Works out which pins connect across every sheet of an Altium project, and names the nets as Altium does.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::project::{self, Project, Scope};
use super::schematic::{self, Pt, SchComponent, SchDoc, UNIT};
use crate::design::{Component, Design, Net, Pin, PinKind};

/// Objects closer than half a unit touch. Imported designs place objects up to 0.315 units off the grid.
const TOUCH: i64 = UNIT / 2;

/// Loads a `.PrjPcb` and every schematic it lists, or a lone `.SchDoc`.
pub fn load(path: &Path) -> Result<Design, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let project = match ext.as_str() {
        "prjpcb" => project::load(path)?,
        "schdoc" => Project::single(path),
        _ => return Err(format!("{}: expected a .PrjPcb or .SchDoc file", path.display())),
    };
    let mut warnings = Vec::new();
    let mut docs = Vec::new();
    for p in &project.documents {
        match schematic::load(p) {
            Ok(d) => docs.push((p.clone(), d)),
            Err(e) if docs.is_empty() && project.documents.len() == 1 => return Err(e),
            Err(e) => warnings.push(format!("skipped a sheet: {e}")),
        }
    }
    if docs.is_empty() {
        return Err(format!("{}: no schematic sheet could be read", path.display()));
    }
    Ok(build(&project, &docs, warnings))
}

/// One level of an instance's channel order: the room as (prefix, number, suffix), then the symbol's position.
type OrderKey = ((String, u64, String), std::cmp::Reverse<(usize, usize)>);

/// One placement of a sheet: the document no symbol places, or a channel of a sheet symbol.
#[derive(Debug, Clone)]
struct Instance {
    doc: usize,
    /// Unique key, such as `/0` or `/0/3.2` (document, then symbol index and channel down the tree).
    key: String,
    /// The designator of the sheet symbol (and channel) that placed it: its room, if the sheet is multi-channel.
    room: String,
    /// Sorts instances of one document into channel order: level by level, by room in natural order, then,
    /// between symbols of one designator, the one later in the project first.
    order: Vec<OrderKey>,
    /// The instances each sheet symbol on this sheet places, by symbol index, with each one's channel number
    /// (from `Repeat(name,a,b)`, or 1).
    children: Vec<Vec<(usize, i64)>>,
}

/// Splits `Repeat(NAME,a,b)` into its parts.
fn parse_repeat(s: &str) -> Option<(String, Vec<String>)> {
    let inner = s
        .trim()
        .strip_prefix("Repeat(")
        .or_else(|| s.trim().strip_prefix("REPEAT("))?;
    let inner = inner.strip_suffix(')')?;
    let mut parts = inner.split(',').map(|p| p.trim().to_string());
    let name = parts.next()?;
    Some((name, parts.collect()))
}

/// The most channels a `Repeat()` or members a bus range may have; a larger one is read as a plain name.
const MAX_CHANNELS: i64 = 4096;

/// Channels a sheet symbol places: `(room, channel number)`.
fn channels(designator: &str) -> Vec<(String, i64)> {
    if let Some((name, args)) = parse_repeat(designator)
        && let [a, b] = args.as_slice()
        && let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>())
        && b > a
        && b - a < MAX_CHANNELS
    {
        return (a..=b).map(|i| (format!("{name}{i}"), i)).collect();
    }
    vec![(designator.to_string(), 1)]
}

/// Whether channel instances all lie on one branch of the sheet tree (each placed, maybe indirectly, by the
/// next), so a net among them stays inside one channel.
fn one_channel_path(insts: &BTreeSet<usize>, instances: &[Instance]) -> bool {
    let mut keys: Vec<&str> = insts.iter().map(|&i| instances[i].key.as_str()).collect();
    keys.sort_by_key(|k| k.len());
    keys.windows(2)
        .all(|w| w[1].starts_with(w[0]) && w[1][w[0].len()..].starts_with('/'))
}

/// Expands `NAME[a..b]` into its members.
fn expand_range(s: &str) -> Option<Vec<String>> {
    let open = s.rfind('[')?;
    let inner = s[open + 1..].strip_suffix(']')?;
    let (a, b) = inner.split_once("..")?;
    let (a, b) = (a.trim().parse::<i64>().ok()?, b.trim().parse::<i64>().ok()?);
    if a.abs_diff(b) >= MAX_CHANNELS as u64 {
        return None;
    }
    let prefix = &s[..open];
    Some((a.min(b)..=a.max(b)).map(|i| format!("{prefix}{i}")).collect())
}

fn range_prefix(s: &str) -> Option<&str> {
    expand_range(s)?;
    s.rfind('[').map(|i| &s[..i])
}

fn key(s: &str) -> String {
    s.to_uppercase()
}

struct UnionFind(Vec<usize>);

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind((0..n).collect())
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

fn dist2_to_segment(p: Pt, a: Pt, b: Pt) -> f64 {
    let (px, py) = (p.0 as f64, p.1 as f64);
    let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    (px - cx).powi(2) + (py - cy).powi(2)
}

fn touches(p: Pt, a: Pt, b: Pt) -> bool {
    dist2_to_segment(p, a, b) <= (TOUCH as f64).powi(2)
}

/// Segments bucketed on a coarse grid, so a point only tests the segments near it.
struct SegIndex {
    segs: Vec<(Pt, Pt, usize)>,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

const CELL: i64 = 20 * UNIT;

impl SegIndex {
    fn new() -> Self {
        SegIndex {
            segs: Vec::new(),
            cells: HashMap::new(),
        }
    }
    fn add(&mut self, a: Pt, b: Pt, node: usize) {
        let i = self.segs.len();
        self.segs.push((a, b, node));
        // Step along the segment a cell at a time, adding the cells within reach of each step, so a long
        // diagonal costs its length rather than its bounding box.
        let steps = (b.0 - a.0).abs().max((b.1 - a.1).abs()) / CELL + 1;
        let mut cells = BTreeSet::new();
        for k in 0..=steps {
            let x = a.0 + (b.0 - a.0) * k / steps;
            let y = a.1 + (b.1 - a.1) * k / steps;
            for cx in (x - CELL / 2 - TOUCH).div_euclid(CELL)..=(x + CELL / 2 + TOUCH).div_euclid(CELL) {
                for cy in (y - CELL / 2 - TOUCH).div_euclid(CELL)..=(y + CELL / 2 + TOUCH).div_euclid(CELL) {
                    cells.insert((cx, cy));
                }
            }
        }
        for c in cells {
            self.cells.entry(c).or_default().push(i);
        }
    }
    /// Nodes with a segment touching `p`.
    fn at(&self, p: Pt) -> impl Iterator<Item = usize> + '_ {
        let cell = (p.0.div_euclid(CELL), p.1.div_euclid(CELL));
        self.cells
            .get(&cell)
            .into_iter()
            .flatten()
            .filter(move |&&i| touches(p, self.segs[i].0, self.segs[i].1))
            .map(move |&i| self.segs[i].2)
    }
}

/// Points bucketed by grid cell, to find points that coincide.
fn coincident(points: &[(Pt, usize)], mut f: impl FnMut(usize, usize)) {
    let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (p, _)) in points.iter().enumerate() {
        cells
            .entry((p.0.div_euclid(UNIT), p.1.div_euclid(UNIT)))
            .or_default()
            .push(i);
    }
    for (i, (p, n)) in points.iter().enumerate() {
        let (cx, cy) = (p.0.div_euclid(UNIT), p.1.div_euclid(UNIT));
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in cells.get(&(cx + dx, cy + dy)).into_iter().flatten() {
                    let q = points[j].0;
                    if j > i && (p.0 - q.0).abs() <= TOUCH && (p.1 - q.1).abs() <= TOUCH {
                        f(*n, points[j].1);
                    }
                }
            }
        }
    }
}

/// How strongly a name names its net. Lower wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    /// A power port, when `PowerPortNamesTakePriority` is on.
    Power0,
    /// A member of a labelled harness, `<label>.<entry>`.
    Harness,
    Label,
    Power,
    Port,
}

#[derive(Debug, Clone)]
struct Name {
    rank: Rank,
    name: String,
    /// The instance whose object gives the name.
    inst: usize,
}

#[derive(Debug, Clone)]
struct PinRef {
    reference: String,
    number: String,
    name: String,
    kind: PinKind,
}

/// One connected group of objects on one sheet instance, or a link-only group joining two identifiers.
#[derive(Debug, Default)]
struct Subgraph {
    pins: Vec<PinRef>,
    names: Vec<Name>,
    links: Vec<String>,
    /// Instances in which this group has a port, a harness or a project-wide identifier that carries it off
    /// its sheet.
    leaves: bool,
    no_erc: bool,
    /// The sheet instance the group is on; `None` for link-only groups.
    inst: Option<usize>,
    /// Harness signals on the group: `(bundle key, member name)`. They become links once bundles are resolved.
    bundle_links: Vec<(String, String)>,
}

pub fn pin_kind(electrical: i64) -> PinKind {
    match electrical {
        0 => PinKind::Input,
        1 => PinKind::Bidirectional,
        2 => PinKind::Output,
        3 => PinKind::OpenCollector,
        4 => PinKind::Passive,
        5 => PinKind::TriState,
        6 => PinKind::OpenEmitter,
        7 => PinKind::PowerIn,
        _ => PinKind::Unspecified,
    }
}

/// Splits a designator into prefix, number and suffix, so `R9` sorts before `R11`.
fn designator_key(s: &str) -> (String, u64, String) {
    let start = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
    let end = s[start..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|e| start + e)
        .unwrap_or(s.len());
    (
        sort_text(&s[..start]),
        s[start..end].parse().unwrap_or(0),
        sort_text(&s[end..]),
    )
}

/// Altium compares text ignoring case, with `_` after digits but before letters: `USB2_N` before `USB_N`, and
/// `SUMPB_U1` before `SUMPBX_C1`.
fn sort_text(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '_' => '@',
            c => c.to_ascii_uppercase(),
        })
        .collect()
}

fn cmp_pin_number(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        _ => sort_text(a).cmp(&sort_text(b)),
    }
}

/// `$ChannelAlpha`: A to Z, then the characters after Z.
fn channel_alpha(i: usize) -> String {
    char::from_u32(64 + i as u32).map(String::from).unwrap_or_default()
}

/// Applies the project's channel designator format to a designator.
fn channel_designator(format: &str, designator: &str, room: &str, index: usize) -> String {
    let split = designator.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let tokens = [
        ("$ComponentPrefix", designator[..split].to_string()),
        ("$ComponentIndex", designator[split..].to_string()),
        ("$Component", designator.to_string()),
        ("$RoomName", room.to_string()),
        ("$ChannelIndex", index.to_string()),
        ("$ChannelAlpha", channel_alpha(index)),
    ];
    let mut out = String::new();
    let mut rest = format;
    'outer: while !rest.is_empty() {
        for (t, v) in &tokens {
            if rest.len() >= t.len() && rest[..t.len()].eq_ignore_ascii_case(t) {
                out.push_str(v);
                rest = &rest[t.len()..];
                continue 'outer;
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn build(project: &Project, docs: &[(PathBuf, SchDoc)], mut warnings: Vec<String>) -> Design {
    // Which document each sheet symbol places, by file name.
    let by_name: HashMap<String, usize> = docs
        .iter()
        .enumerate()
        .filter_map(|(i, (p, _))| Some((p.file_name()?.to_str()?.to_ascii_lowercase(), i)))
        .collect();
    let child_doc = |file: &str| {
        let base = file.rsplit(['\\', '/']).next().unwrap_or(file).to_ascii_lowercase();
        by_name.get(&base).copied()
    };
    let mut placed = HashSet::new();
    for (path, d) in docs {
        for s in &d.sheet_symbols {
            match child_doc(&s.file) {
                Some(c) => {
                    placed.insert(c);
                }
                None => warnings.push(format!(
                    "{}: sheet symbol `{}` places `{}`, which is not in the project",
                    path.display(),
                    s.name,
                    s.file
                )),
            }
        }
    }

    let mut instances: Vec<Instance> = Vec::new();
    #[allow(clippy::too_many_arguments)]
    fn place(
        doc: usize,
        key: String,
        room: String,
        order: Vec<OrderKey>,
        docs: &[(PathBuf, SchDoc)],
        child_doc: &dyn Fn(&str) -> Option<usize>,
        stack: &mut Vec<usize>,
        out: &mut Vec<Instance>,
        warnings: &mut Vec<String>,
    ) -> Option<usize> {
        if stack.contains(&doc) {
            warnings.push(format!("{}: sheet places itself", docs[doc].0.display()));
            return None;
        }
        let me = out.len();
        out.push(Instance {
            doc,
            key: key.clone(),
            room,
            order: order.clone(),
            children: Vec::new(),
        });
        stack.push(doc);
        let mut children = Vec::new();
        for (si, s) in docs[doc].1.sheet_symbols.iter().enumerate() {
            let mut placed = Vec::new();
            if let Some(c) = child_doc(&s.file) {
                for (room, n) in channels(&s.name) {
                    let child_key = format!("{key}/{si}.{n}");
                    let mut path = order.clone();
                    path.push((designator_key(&room), std::cmp::Reverse((doc, si))));
                    if let Some(i) = place(c, child_key, room, path, docs, child_doc, stack, out, warnings) {
                        placed.push((i, n));
                    }
                }
            }
            children.push(placed);
        }
        stack.pop();
        out[me].children = children;
        Some(me)
    }
    for (i, _) in docs.iter().enumerate() {
        if !placed.contains(&i) {
            let order = vec![(Default::default(), std::cmp::Reverse((i, 0)))];
            place(
                i,
                format!("/{i}"),
                String::new(),
                order,
                docs,
                &child_doc,
                &mut Vec::new(),
                &mut instances,
                &mut warnings,
            );
        }
    }
    // Sheets that only place each other in a loop have no root: start each such loop at its first sheet.
    for i in 0..docs.len() {
        if !instances.iter().any(|inst| inst.doc == i) {
            let order = vec![(Default::default(), std::cmp::Reverse((i, 0)))];
            place(
                i,
                format!("/{i}"),
                String::new(),
                order,
                docs,
                &child_doc,
                &mut Vec::new(),
                &mut instances,
                &mut warnings,
            );
        }
    }

    // A document placed more than once is multi-channel: its parts get one designator per channel, numbered in
    // instance order.
    let mut per_doc: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, inst) in instances.iter().enumerate() {
        per_doc.entry(inst.doc).or_default().push(i);
    }
    let mut channel_of: HashMap<usize, (String, usize)> = HashMap::new();
    for list in per_doc.values().filter(|l| l.len() > 1) {
        let mut sorted = list.clone();
        sorted.sort_by(|&a, &b| instances[a].order.cmp(&instances[b].order));
        for (n, &i) in sorted.iter().enumerate() {
            channel_of.insert(i, (instances[i].room.clone(), n + 1));
        }
    }

    // Scope.
    let scope = match project.scope {
        Scope::Automatic => {
            let entries = docs
                .iter()
                .any(|(_, d)| d.sheet_symbols.iter().any(|s| !s.entries.is_empty()));
            let ports = docs.iter().any(|(_, d)| !d.ports.is_empty());
            if entries {
                Scope::Hierarchical
            } else if ports {
                Scope::Flat
            } else {
                Scope::Global
            }
        }
        s => s,
    };
    let ports_global = matches!(scope, Scope::Flat | Scope::Global);
    let power_global = scope != Scope::StrictHierarchical;
    let power_names: HashSet<String> = if power_global {
        docs.iter()
            .flat_map(|(_, d)| d.power_ports.iter().map(|(_, n)| key(n)))
            .collect()
    } else {
        HashSet::new()
    };

    let mut b = Builder {
        project,
        scope,
        ports_global,
        power_global,
        power_names,
        subgraphs: Vec::new(),
        components: BTreeMap::new(),
        all_pins: BTreeMap::new(),
        drawn_parts: HashMap::new(),
        warnings,
        bundle_unions: Vec::new(),
        bundle_labels: Vec::new(),
    };
    for i in 0..instances.len() {
        b.sheet(&instances, i, docs, channel_of.get(&i));
    }
    b.finish(&instances, docs, &channel_of)
}

struct Builder<'a> {
    project: &'a Project,
    scope: Scope,
    ports_global: bool,
    power_global: bool,
    /// Every power port name in the project, upper-cased, when power ports are global.
    power_names: HashSet<String>,
    subgraphs: Vec<Subgraph>,
    components: BTreeMap<String, Component>,
    /// Every pin of every part of each component, whether drawn or not.
    all_pins: BTreeMap<String, BTreeMap<String, PinRef>>,
    /// `(reference, part)` → the sheet that draws it, to spot duplicate designators.
    drawn_parts: HashMap<(String, i64), String>,
    warnings: Vec<String>,
    /// Pairs of bundle keys that are one bundle.
    bundle_unions: Vec<(String, String)>,
    /// Names for a bundle's members, `<name>.<member>`: a label drawn on a signal harness, or a harness port
    /// when ports may name nets. By a key of the bundle.
    bundle_labels: Vec<(String, Name)>,
}

/// What sits at a connection point on a sheet.
#[derive(Debug)]
enum Item {
    Pin(PinRef, Option<String>),
    Label(String),
    Power(String),
    Port(usize),
    Entry(usize, usize),
    /// A harness connector's entry: the wire on it is that member of the connector's bundle.
    HarnessEntry(usize, usize),
    Junction,
    NoErc,
}

impl Builder<'_> {
    fn sheet(
        &mut self,
        instances: &[Instance],
        ii: usize,
        docs: &[(PathBuf, SchDoc)],
        channel: Option<&(String, usize)>,
    ) {
        let inst = &instances[ii];
        let (path, doc) = &docs[inst.doc];
        let sheet_name = path.file_name().and_then(|f| f.to_str()).unwrap_or("?").to_string();
        let ik = &inst.key;

        // Nodes: wires first, then items.
        let nw = doc.wires.len();
        let mut items: Vec<Item> = Vec::new();
        let mut points: Vec<(Pt, usize)> = Vec::new(); // item connection points, by node
        let mut segs = SegIndex::new();
        for (wi, w) in doc.wires.iter().enumerate() {
            for s in w.windows(2) {
                segs.add(s[0], s[1], wi);
            }
        }
        // Pins: a wire may join anywhere along a pin, so pins get segments that only wire ends test.
        let mut pin_segs = SegIndex::new();
        for c in &doc.components {
            if c.kind == 2 {
                continue;
            }
            let reference = match channel {
                Some((room, index)) => {
                    channel_designator(&self.project.channel_designator_format, &c.designator, room, *index)
                }
                None => c.designator.clone(),
            };
            self.add_component(&reference, c, &sheet_name);
            for p in c.pins.iter().filter(|p| c.drawn(p)) {
                let node = nw + items.len();
                let pr = PinRef {
                    reference: reference.clone(),
                    number: p.number.clone(),
                    name: p.name.clone(),
                    kind: pin_kind(p.electrical),
                };
                // A hidden pin joins the net its `HiddenNetName` gives. Older libraries leave that empty; their
                // hidden power pins, and hidden pins named after a supply, join the net of their name.
                let hidden_net = if p.hidden_net.is_empty() {
                    &p.name
                } else {
                    &p.hidden_net
                };
                let joins = !p.hidden_net.is_empty() || p.electrical == 7 || self.power_names.contains(&key(&p.name));
                let hidden = (p.hidden && joins && !hidden_net.is_empty()).then(|| hidden_net.clone());
                if hidden.is_none() {
                    pin_segs.add(p.base, p.tip, node);
                    points.push((p.tip, node));
                }
                items.push(Item::Pin(pr, hidden));
            }
        }
        // Labels on a bus name the bus, not a wire.
        let mut bus_segs = SegIndex::new();
        for (bi, bus) in doc.buses.iter().enumerate() {
            for s in bus.windows(2) {
                bus_segs.add(s[0], s[1], bi);
            }
        }
        let mut harness_segs = SegIndex::new();
        for (hi, h) in doc.harnesses.iter().enumerate() {
            for s in h.windows(2) {
                harness_segs.add(s[0], s[1], hi);
            }
        }
        let mut bus_labels: Vec<(usize, String)> = Vec::new();
        let mut harness_labels: Vec<(usize, String)> = Vec::new();
        for (p, name) in &doc.labels {
            if let Some(h) = harness_segs.at(*p).next() {
                harness_labels.push((h, name.clone()));
                continue;
            }
            if let Some(bus) = bus_segs.at(*p).next() {
                bus_labels.push((bus, name.clone()));
                continue;
            }
            points.push((*p, nw + items.len()));
            items.push(Item::Label(name.clone()));
        }
        for (p, name) in &doc.power_ports {
            points.push((*p, nw + items.len()));
            items.push(Item::Power(name.clone()));
        }
        // Ports and entries on a bus carry the bus's members instead of joining a wire.
        let mut bus_ids: Vec<(usize, Item)> = Vec::new();
        for (pi, port) in doc.ports.iter().enumerate() {
            if let Some(bus) = port.ends.iter().find_map(|e| bus_segs.at(*e).next()) {
                bus_ids.push((bus, Item::Port(pi)));
                continue;
            }
            if !port.harness_type.is_empty() {
                continue;
            }
            for e in port.ends {
                points.push((e, nw + items.len()));
            }
            items.push(Item::Port(pi));
        }
        for (si, s) in doc.sheet_symbols.iter().enumerate() {
            for (ei, e) in s.entries.iter().enumerate() {
                if let Some(bus) = bus_segs.at(e.at).next() {
                    bus_ids.push((bus, Item::Entry(si, ei)));
                    continue;
                }
                if !e.harness_type.is_empty() {
                    continue;
                }
                points.push((e.at, nw + items.len()));
                items.push(Item::Entry(si, ei));
            }
        }
        for (ci, c) in doc.harness_connectors.iter().enumerate() {
            for (ei, e) in c.entries.iter().enumerate() {
                if let Some(bus) = bus_segs.at(e.at).next() {
                    bus_ids.push((bus, Item::HarnessEntry(ci, ei)));
                    continue;
                }
                points.push((e.at, nw + items.len()));
                items.push(Item::HarnessEntry(ci, ei));
            }
        }
        for p in &doc.junctions {
            points.push((*p, nw + items.len()));
            items.push(Item::Junction);
        }
        for p in &doc.no_erc {
            points.push((*p, nw + items.len()));
            items.push(Item::NoErc);
        }

        let mut uf = UnionFind::new(nw + items.len());
        // A wire end joins any wire it lands on, and any pin along its length.
        for (wi, w) in doc.wires.iter().enumerate() {
            for v in w {
                for n in segs.at(*v).chain(pin_segs.at(*v)).collect::<Vec<_>>() {
                    uf.union(wi, n);
                }
            }
        }
        // Every other object joins the wires its point lands on and the objects at the same point.
        for (p, n) in &points {
            for w in segs.at(*p).collect::<Vec<_>>() {
                uf.union(*n, w);
            }
        }
        coincident(&points, |a, b| uf.union(a, b));

        // Buses: joined where a vertex of one lands on another.
        let mut bus_uf = UnionFind::new(doc.buses.len());
        for (bi, bus) in doc.buses.iter().enumerate() {
            for v in bus {
                for o in bus_segs.at(*v).collect::<Vec<_>>() {
                    bus_uf.union(bi, o);
                }
            }
        }

        let mut groups: BTreeMap<usize, Subgraph> = BTreeMap::new();
        for (i, item) in items.iter().enumerate() {
            let g = groups.entry(uf.find(nw + i)).or_default();
            match item {
                Item::Pin(p, hidden) => {
                    g.pins.push(p.clone());
                    if let Some(net) = hidden {
                        self.power(g, ii, ik, net);
                    }
                }
                Item::Label(name) => self.label(g, ii, ik, name),
                Item::Power(name) => self.power(g, ii, ik, name),
                Item::Port(pi) => {
                    let name = &doc.ports[*pi].name;
                    self.port(g, ii, ik, name);
                }
                Item::Entry(si, ei) => {
                    let name = doc.sheet_symbols[*si].entries[*ei].name.clone();
                    let children = &inst.children[*si];
                    match parse_repeat(&name) {
                        // A `Repeat(NAME)` entry on a labelled wire hands member `<label><n>` to channel n.
                        Some((port, _)) => {
                            let root = uf.find(nw + i);
                            let labels: Vec<String> = items_in_group(&items, &mut uf, nw, root);
                            if let Some(label) = labels.first() {
                                for (c, n) in children {
                                    self.subgraphs.push(Subgraph {
                                        links: vec![
                                            format!("L:{ik}:{}", key(&format!("{label}{n}"))),
                                            format!("X:{}:{}", instances[*c].key, key(&port)),
                                        ],
                                        ..Default::default()
                                    });
                                }
                            }
                        }
                        None => {
                            for (c, _) in children {
                                g.links.push(format!("X:{}:{}", instances[*c].key, key(&name)));
                            }
                        }
                    }
                }
                Item::HarnessEntry(ci, ei) => {
                    let name = &doc.harness_connectors[*ci].entries[*ei].name;
                    g.bundle_links.push((format!("C:{ik}:{ci}"), name.clone()));
                    g.leaves = true;
                }
                Item::Junction => {}
                Item::NoErc => g.no_erc = true,
            }
        }
        self.bundles(instances, ii, doc, &harness_segs, &harness_labels);

        // Each bus: the members its ranges name, linked through every identifier on it.
        let mut bus_groups: BTreeMap<usize, (Vec<String>, Vec<&Item>)> = BTreeMap::new();
        for (bus, name) in &bus_labels {
            bus_groups.entry(bus_uf.find(*bus)).or_default().0.push(name.clone());
        }
        for (bus, item) in &bus_ids {
            let name = match item {
                Item::Port(pi) => doc.ports[*pi].name.clone(),
                Item::Entry(si, ei) => doc.sheet_symbols[*si].entries[*ei].name.clone(),
                Item::HarnessEntry(ci, ei) => doc.harness_connectors[*ci].entries[*ei].name.clone(),
                _ => continue,
            };
            let g = bus_groups.entry(bus_uf.find(*bus)).or_default();
            g.0.push(name);
            g.1.push(item);
        }
        for (names, ids) in bus_groups.values() {
            let members: BTreeSet<String> = names.iter().filter_map(|n| expand_range(n)).flatten().collect();
            let prefixes: BTreeSet<&str> = names.iter().filter_map(|n| range_prefix(n)).collect();
            // Each member net links through the identifiers on its bus, as it would through a plain port or entry
            // of the member's name; a bus reaching a harness puts its members in the bundle.
            let member_group = |m: &str| Subgraph {
                links: vec![format!("L:{ik}:{}", key(m))],
                leaves: true,
                ..Default::default()
            };
            for id in ids {
                match id {
                    Item::Port(pi) if !doc.ports[*pi].harness_type.is_empty() => {
                        let port = key(&doc.ports[*pi].name);
                        for m in &members {
                            let mut g = member_group(m);
                            g.bundle_links.push((format!("HX:{ik}:{port}"), m.clone()));
                            if self.ports_global {
                                self.bundle_unions
                                    .push((format!("HX:{ik}:{port}"), format!("HG:{port}")));
                            }
                            self.subgraphs.push(g);
                        }
                    }
                    Item::Entry(si, ei) if !doc.sheet_symbols[*si].entries[*ei].harness_type.is_empty() => {
                        let entry = key(&doc.sheet_symbols[*si].entries[*ei].name);
                        for m in &members {
                            for (c, _) in &inst.children[*si] {
                                let mut g = member_group(m);
                                g.bundle_links
                                    .push((format!("HX:{}:{entry}", instances[*c].key), m.clone()));
                                self.subgraphs.push(g);
                            }
                        }
                    }
                    Item::HarnessEntry(ci, _) => {
                        for m in &members {
                            let mut g = member_group(m);
                            g.bundle_links.push((format!("C:{ik}:{ci}"), m.clone()));
                            self.subgraphs.push(g);
                        }
                    }
                    Item::Port(_) => {
                        for m in &members {
                            let mut g = member_group(m);
                            self.port(&mut g, ii, ik, m);
                            g.names.clear();
                            self.subgraphs.push(g);
                        }
                    }
                    Item::Entry(si, ei) => {
                        let name = &doc.sheet_symbols[*si].entries[*ei].name;
                        let children = &inst.children[*si];
                        if let Some((port, _)) = parse_repeat(name) {
                            for (c, n) in children {
                                let member = match prefixes.iter().next() {
                                    Some(p) if prefixes.len() == 1 => format!("{p}{n}"),
                                    _ => format!("{port}{n}"),
                                };
                                let mut g = member_group(&member);
                                g.links.push(format!("X:{}:{}", instances[*c].key, key(&port)));
                                self.subgraphs.push(g);
                            }
                        } else {
                            for m in &members {
                                let mut g = member_group(m);
                                g.links.extend(
                                    children
                                        .iter()
                                        .map(|(c, _)| format!("X:{}:{}", instances[*c].key, key(m))),
                                );
                                self.subgraphs.push(g);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        for (_, mut g) in groups {
            g.inst = Some(ii);
            if !g.pins.is_empty() || !g.links.is_empty() || !g.bundle_links.is_empty() {
                self.subgraphs.push(g);
            }
        }
    }

    /// Works out which harness objects on a sheet carry one bundle, and records the keys that join bundles
    /// across sheets.
    fn bundles(
        &mut self,
        instances: &[Instance],
        ii: usize,
        doc: &SchDoc,
        harness_segs: &SegIndex,
        harness_labels: &[(usize, String)],
    ) {
        let inst = &instances[ii];
        let ik = &inst.key;
        let nl = doc.harnesses.len();
        // Bundle nodes after the harness lines, each with its points and the keys it gives its bundle.
        let mut nodes: Vec<(Vec<Pt>, Vec<String>)> = Vec::new();
        for (ci, c) in doc.harness_connectors.iter().enumerate() {
            nodes.push((vec![c.primary], vec![format!("C:{ik}:{ci}")]));
            // An entry another harness lands on carries a nested bundle: that member of this one.
            for e in &c.entries {
                nodes.push((vec![e.at], vec![format!("C:{ik}:{ci}|{}", key(&e.name))]));
            }
        }
        for port in doc.ports.iter().filter(|p| !p.harness_type.is_empty()) {
            let mut keys = vec![format!("HX:{ik}:{}", key(&port.name))];
            if self.project.allow_port_net_names && !port.name.is_empty() {
                let name = Name {
                    rank: Rank::Port,
                    name: port.name.clone(),
                    inst: ii,
                };
                self.bundle_labels.push((keys[0].clone(), name));
            }
            if self.ports_global {
                keys.push(format!("HG:{}", key(&port.name)));
            }
            nodes.push((port.ends.to_vec(), keys));
        }
        for (si, s) in doc.sheet_symbols.iter().enumerate() {
            for e in s.entries.iter().filter(|e| !e.harness_type.is_empty()) {
                let keys = inst.children[si]
                    .iter()
                    .map(|(c, _)| format!("HX:{}:{}", instances[*c].key, key(&e.name)))
                    .collect();
                nodes.push((vec![e.at], keys));
            }
        }
        let mut uf = UnionFind::new(nl + nodes.len());
        for (hi, h) in doc.harnesses.iter().enumerate() {
            for v in h {
                for o in harness_segs.at(*v).collect::<Vec<_>>() {
                    uf.union(hi, o);
                }
            }
        }
        let mut points: Vec<(Pt, usize)> = Vec::new();
        for (ni, (pts, _)) in nodes.iter().enumerate() {
            for p in pts {
                points.push((*p, nl + ni));
                for h in harness_segs.at(*p).collect::<Vec<_>>() {
                    uf.union(nl + ni, h);
                }
            }
        }
        coincident(&points, |a, b| uf.union(a, b));
        let mut first: HashMap<usize, String> = HashMap::new();
        for (ni, (_, keys)) in nodes.iter().enumerate() {
            let root = uf.find(nl + ni);
            for k in keys {
                match first.get(&root) {
                    Some(f) => self.bundle_unions.push((f.clone(), k.clone())),
                    None => {
                        first.insert(root, k.clone());
                    }
                }
            }
        }
        for (h, label) in harness_labels {
            if let Some(f) = first.get(&uf.find(*h)) {
                let name = Name {
                    rank: Rank::Harness,
                    name: label.clone(),
                    inst: ii,
                };
                self.bundle_labels.push((f.clone(), name));
            }
        }
    }

    fn label(&self, g: &mut Subgraph, ii: usize, ik: &str, name: &str) {
        if name.trim().is_empty() {
            return;
        }
        let k = key(name);
        g.links.push(format!("L:{ik}:{k}"));
        if self.scope == Scope::Global {
            g.links.push(format!("P:{k}"));
            g.leaves = true;
        } else if self.power_names.contains(&k) {
            // A label spelled as a global power port names that supply.
            g.links.push(format!("P:{k}"));
        }
        g.names.push(Name {
            rank: Rank::Label,
            name: name.to_string(),
            inst: ii,
        });
    }

    fn power(&self, g: &mut Subgraph, ii: usize, ik: &str, name: &str) {
        if name.trim().is_empty() {
            return;
        }
        let k = key(name);
        g.links.push(format!("L:{ik}:{k}"));
        if self.power_global {
            g.links.push(format!("P:{k}"));
        }
        let rank = if self.project.power_port_names_take_priority {
            Rank::Power0
        } else {
            Rank::Power
        };
        g.names.push(Name {
            rank,
            name: name.to_string(),
            inst: ii,
        });
    }

    fn port(&self, g: &mut Subgraph, ii: usize, ik: &str, name: &str) {
        if name.trim().is_empty() {
            return;
        }
        let k = key(name);
        g.links.push(format!("X:{ik}:{k}"));
        if self.ports_global {
            g.links.push(format!("G:{k}"));
        }
        g.leaves = true;
        if self.project.allow_port_net_names {
            g.names.push(Name {
                rank: Rank::Port,
                name: name.to_string(),
                inst: ii,
            });
        }
    }

    fn add_component(&mut self, reference: &str, c: &SchComponent, sheet: &str) {
        if reference.ends_with('?') {
            self.warnings.push(format!(
                "sheet {sheet}: part `{}` has no designator ({reference}); annotate the schematic",
                c.lib_ref
            ));
        }
        let part = (reference.to_string(), c.part_id);
        if let Some(other) = self.drawn_parts.insert(part.clone(), sheet.to_string())
            && !reference.ends_with('?')
        {
            self.warnings.push(format!(
                "designator {reference} (part {}) is used on both {other} and {sheet}; their pins are merged",
                c.part_id
            ));
        }
        let pins = self.all_pins.entry(reference.to_string()).or_default();
        for p in &c.pins {
            if p.display_mode == c.display_mode {
                pins.entry(p.number.clone()).or_insert_with(|| PinRef {
                    reference: reference.to_string(),
                    number: p.number.clone(),
                    name: p.name.clone(),
                    kind: pin_kind(p.electrical),
                });
            }
        }
        let comp = self.components.entry(reference.to_string()).or_insert_with(|| {
            let mut fields: BTreeMap<String, String> = c.params.clone();
            let value = c
                .param("Value")
                .or_else(|| c.param("Comment"))
                .unwrap_or_default()
                .to_string();
            if let Some(comment) = c.param("Comment") {
                fields.insert("Comment".into(), comment.to_string());
            }
            fields.insert("Footprint".into(), c.footprint.clone());
            if !c.description.is_empty() {
                fields
                    .entry("Description".into())
                    .or_insert_with(|| c.description.clone());
            }
            Component {
                reference: reference.to_string(),
                value,
                lib_id: c.lib_ref.clone(),
                footprint: c.footprint.clone(),
                fields,
                dnp: false,
                in_bom: !matches!(c.kind, 4 | 5),
                on_board: true,
                pins: BTreeMap::new(),
            }
        });
        for (k, v) in &c.params {
            comp.fields.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    fn finish(
        mut self,
        instances: &[Instance],
        docs: &[(PathBuf, SchDoc)],
        channel_of: &HashMap<usize, (String, usize)>,
    ) -> Design {
        // Every pin of a part no sheet draws is a net of its own.
        let mut drawn: HashSet<(String, String)> = HashSet::new();
        for g in &self.subgraphs {
            for p in &g.pins {
                drawn.insert((p.reference.clone(), p.number.clone()));
            }
        }
        for (r, pins) in &self.all_pins {
            for (n, p) in pins {
                if !drawn.contains(&(r.clone(), n.clone())) {
                    self.subgraphs.push(Subgraph {
                        pins: vec![p.clone()],
                        ..Default::default()
                    });
                }
            }
        }

        // Harness signals become links once every bundle is resolved.
        let mut bundles = Bundles::default();
        for (a, b) in &self.bundle_unions {
            bundles.union(a, b);
        }
        let mut named: HashMap<usize, Vec<Name>> = HashMap::new();
        let labels = std::mem::take(&mut self.bundle_labels);
        let mut queries: Vec<String> = labels.iter().map(|(k, _)| k.clone()).collect();
        for g in &self.subgraphs {
            queries.extend(g.bundle_links.iter().map(|(k, m)| format!("{k}|{}", key(m))));
        }
        bundles.close(&queries);
        for (k, name) in labels {
            let root = bundles.canon(&k);
            named.entry(root).or_default().push(name);
        }
        for g in &mut self.subgraphs {
            for (k, m) in std::mem::take(&mut g.bundle_links) {
                let root = bundles.canon(&k);
                let member = bundles.canon(&format!("{k}|{}", key(&m)));
                g.links.push(format!("H:{member}"));
                for n in named.get(&root).into_iter().flatten() {
                    g.names.push(Name {
                        name: format!("{}.{m}", n.name),
                        ..n.clone()
                    });
                }
            }
        }

        let n = self.subgraphs.len();
        let mut uf = UnionFind::new(n);
        let mut first: HashMap<&str, usize> = HashMap::new();
        for (i, g) in self.subgraphs.iter().enumerate() {
            for link in &g.links {
                match first.get(link.as_str()) {
                    Some(&j) => uf.union(i, j),
                    None => {
                        first.insert(link, i);
                    }
                }
            }
        }
        let mut merged: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..n {
            merged.entry(uf.find(i)).or_default().push(i);
        }

        let mut nets: Vec<Net> = Vec::new();
        let mut pin_kinds: HashMap<(String, String), (String, PinKind)> = HashMap::new();
        let mut no_connect: HashSet<(String, String)> = HashSet::new();
        for members in merged.values() {
            let mut pins: Vec<PinRef> = Vec::new();
            let mut names: Vec<Name> = Vec::new();
            let mut leaves = false;
            let mut nc = false;
            // The channel instances the net reaches.
            let mut channels_on: BTreeSet<usize> = BTreeSet::new();
            for &m in members {
                let g = &mut self.subgraphs[m];
                if let Some(i) = g.inst.filter(|i| channel_of.contains_key(i)) {
                    channels_on.insert(i);
                }
                pins.append(&mut g.pins);
                names.extend(g.names.iter().cloned());
                leaves |= g.leaves;
                nc |= g.no_erc;
            }
            if pins.is_empty() {
                continue;
            }
            pins.sort_by(|a, b| {
                designator_key(&a.reference)
                    .cmp(&designator_key(&b.reference))
                    .then(cmp_pin_number(&a.number, &b.number))
            });
            pins.dedup_by(|a, b| a.reference == b.reference && a.number == b.number);
            let best = names.iter().min_by(|a, b| {
                a.rank
                    .cmp(&b.rank)
                    .then(sort_text(&a.name).cmp(&sort_text(&b.name)))
                    .then(a.name.cmp(&b.name))
            });
            let name = match best {
                Some(nm) => {
                    let mut name = nm.name.clone();
                    if self.project.append_sheet_number_to_local_nets
                        && nm.rank == Rank::Label
                        && !leaves
                        && !self.power_names.contains(&key(&nm.name))
                    {
                        let num = docs[instances[nm.inst].doc]
                            .1
                            .sheet_number
                            .clone()
                            .unwrap_or("*".into());
                        name = append_sheet_number(&name, &num);
                    }
                    // A net that stays inside one channel of a multi-channel sheet is named the way the
                    // channel's parts are, unless it is a supply.
                    if let Some((room, index)) = channel_of.get(&nm.inst)
                        && one_channel_path(&channels_on, instances)
                        && !self.power_names.contains(&key(&nm.name))
                    {
                        name = channel_name(&self.project.channel_designator_format, &name, room, *index);
                    }
                    name
                }
                None => format!("Net{}_{}", pins[0].reference, pins[0].number),
            };
            for p in &pins {
                pin_kinds.insert((p.reference.clone(), p.number.clone()), (p.name.clone(), p.kind));
                if nc {
                    no_connect.insert((p.reference.clone(), p.number.clone()));
                }
            }
            let labels: BTreeSet<String> = names.into_iter().map(|n| n.name).collect();
            nets.push(Net {
                name,
                pins: pins.iter().map(|p| (p.reference.clone(), p.number.clone())).collect(),
                labels: labels.into_iter().collect(),
            });
        }

        // Nets that end up sharing a name without joining keep it once; the rest are numbered.
        let mut taken: HashMap<String, usize> = HashMap::new();
        let all: HashSet<String> = nets.iter().map(|n| key(&n.name)).collect();
        for net in &mut nets {
            let k = key(&net.name);
            let count = taken.entry(k.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                let mut i = *count;
                while all.contains(&key(&format!("{}_{i}", net.name))) {
                    i += 1;
                }
                net.name = format!("{}_{i}", net.name);
            }
        }

        for (index, net) in nets.iter().enumerate() {
            for (r, num) in &net.pins {
                if let Some(c) = self.components.get_mut(r) {
                    let (name, kind) = pin_kinds[&(r.clone(), num.clone())].clone();
                    c.pins.entry(num.clone()).or_insert(Pin {
                        number: num.clone(),
                        name,
                        kind,
                        no_connect: no_connect.contains(&(r.clone(), num.clone())),
                        net: index,
                    });
                }
            }
        }
        Design {
            components: self.components,
            nets,
            warnings: self.warnings,
        }
    }
}

/// Harness bundles, by key. A key is an object that carries a bundle (`C:<sheet>:<connector>`,
/// `HX:<sheet>:<port>`) or a member of one (`<bundle>|<member>`). Wherever two bundles join, their members of
/// one name join too.
#[derive(Default)]
struct Bundles {
    ids: HashMap<String, usize>,
    names: Vec<String>,
    parent: Vec<usize>,
}

impl Bundles {
    fn id(&mut self, k: &str) -> usize {
        if let Some(&i) = self.ids.get(k) {
            return i;
        }
        let i = self.names.len();
        self.ids.insert(k.to_string(), i);
        self.names.push(k.to_string());
        self.parent.push(i);
        i
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }
    fn union(&mut self, a: &str, b: &str) -> bool {
        let (a, b) = (self.id(a), self.id(b));
        let (a, b) = (self.find(a), self.find(b));
        if a == b {
            return false;
        }
        self.parent[a.max(b)] = a.min(b);
        true
    }
    /// The key's member path rebuilt on its bundle's representative, joined to the key itself.
    fn rebuild(&mut self, k: &str) -> bool {
        let Some((prefix, member)) = k.rsplit_once('|') else {
            return false;
        };
        self.rebuild(prefix);
        let id = self.id(prefix);
        let root = self.find(id);
        let canonical = format!("{}|{member}", self.names[root]);
        self.union(k, &canonical)
    }
    /// Joins members of joined bundles until nothing changes.
    fn close(&mut self, queries: &[String]) {
        for q in queries {
            self.id(q);
        }
        loop {
            let keys: Vec<String> = self.names.clone();
            let mut changed = false;
            for k in &keys {
                changed |= self.rebuild(k);
            }
            if !changed {
                break;
            }
        }
    }
    /// The representative of a key's bundle or member.
    fn canon(&mut self, k: &str) -> usize {
        self.rebuild(k);
        let i = self.id(k);
        self.find(i)
    }
}

/// A net name inside a channel: the channel designator format applied to it, with a differential-pair suffix
/// kept last.
fn channel_name(format: &str, name: &str, room: &str, index: usize) -> String {
    for suffix in ["_P", "_N"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return format!("{}{suffix}", channel_designator(format, base, room, index));
        }
    }
    channel_designator(format, name, room, index)
}

/// `VBAT` on sheet 8 is `VBAT_8`; a differential-pair suffix stays last, so `HV_P` on sheet 3 is `HV_3_P`.
fn append_sheet_number(name: &str, number: &str) -> String {
    for suffix in ["_P", "_N"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return format!("{base}_{number}{suffix}");
        }
    }
    format!("{name}_{number}")
}

/// Label names in the group rooted at `root`.
fn items_in_group(items: &[Item], uf: &mut UnionFind, nw: usize, root: usize) -> Vec<String> {
    let mut out: Vec<String> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| match it {
            Item::Label(n) if uf.find(nw + i) == root => Some(n.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out
}
