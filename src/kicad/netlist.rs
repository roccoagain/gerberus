//! Works out which pins connect, across every sheet of a KiCad schematic, and names the nets as KiCad does.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::bus::{self, Aliases, Bus};
use super::schematic::{self, LabelKind, LibPin, LibSymbol, PowerScope, Pt, SchematicFile};
use crate::design::{Component, Design, Net, Pin, PinKind};
use crate::sexpr;

/// Loads a root `.kicad_sch` and every sheet below it.
pub fn load(path: &Path) -> Result<Design, String> {
    let mut files: HashMap<PathBuf, SchematicFile> = HashMap::new();
    read_tree(path, &mut files, &mut Vec::new())?;
    let root = &files[path];
    let mut builder = Builder::default();
    builder.aliases = bus_aliases(path, &files, &mut builder.warnings);
    builder.walk(path, &format!("/{}", root.uuid), "/", &files);
    Ok(builder.finish())
}

/// Reads a sheet file and, recursively, every sheet file it uses.
fn read_tree(path: &Path, files: &mut HashMap<PathBuf, SchematicFile>, stack: &mut Vec<PathBuf>) -> Result<(), String> {
    if stack.iter().any(|p| p == path) {
        return Err(format!("{}: sheet includes itself", path.display()));
    }
    if !files.contains_key(path) {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let tree = sexpr::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = schematic::read(&tree).map_err(|e| format!("{}: {e}", path.display()))?;
        files.insert(path.to_path_buf(), file);
    }
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let children: Vec<PathBuf> = files[path].sheets.iter().map(|s| dir.join(&s.file)).collect();
    stack.push(path.to_path_buf());
    for child in children {
        read_tree(&child, files, stack)?;
    }
    stack.pop();
    Ok(())
}

/// Every sheet file in hierarchy order: the root, then each sheet's files depth first, each file once.
fn hierarchy_order(path: &Path, files: &HashMap<PathBuf, SchematicFile>, out: &mut Vec<PathBuf>) {
    if out.iter().any(|p| p == path) {
        return;
    }
    out.push(path.to_path_buf());
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    for sheet in &files[path].sheets {
        hierarchy_order(&dir.join(&sheet.file), files, out);
    }
}

/// Bus aliases come from the project file (KiCad 10 and later) when it defines a `schematic.bus_aliases`
/// table, and otherwise from `bus_alias` items in the sheets, as in KiCad.
fn bus_aliases(root: &Path, files: &HashMap<PathBuf, SchematicFile>, warnings: &mut Vec<String>) -> Aliases {
    let project = root.with_extension("kicad_pro");
    if let Ok(text) = std::fs::read_to_string(&project) {
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(json) => {
                // Project files before schema 4 wrote an empty table without meaning "no aliases", so KiCad
                // reads such a table as unset.
                let schema = json
                    .pointer("/meta/version")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if let Some(table) = json
                    .pointer("/schematic/bus_aliases")
                    .and_then(|t| t.as_object())
                    .filter(|t| schema >= 4 || !t.is_empty())
                {
                    // Like KiCad, trim names and members and drop empty ones.
                    return table
                        .iter()
                        .filter_map(|(name, members)| {
                            let members = members.as_array()?;
                            let name = name.trim();
                            let members = members
                                .iter()
                                .filter_map(|v| v.as_str())
                                .map(str::trim)
                                .filter(|m| !m.is_empty())
                                .map(str::to_string)
                                .collect();
                            (!name.is_empty()).then(|| (name.to_string(), members))
                        })
                        .collect();
                }
            }
            Err(e) => warnings.push(format!(
                "{}: {e}; bus aliases were read from the schematic instead",
                project.display()
            )),
        }
    }
    let mut order = Vec::new();
    hierarchy_order(root, files, &mut order);
    // KiCad loads sheets in hierarchy order and a later alias of the same name replaces an earlier one.
    let mut aliases = Aliases::new();
    for path in order {
        for (name, members) in &files[&path].bus_aliases {
            if let Some(have) = aliases.get(name)
                && have != members
            {
                warnings.push(format!(
                    "{}: bus alias `{name}` differs from an earlier sheet's alias of that name; using this one, \
                     as KiCad does",
                    path.display()
                ));
            }
            aliases.insert(name.clone(), members.clone());
        }
    }
    aliases
}

/// Strength of a net name source. A stronger one names the net. Mirrors KiCad's driver priorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Priority {
    SheetPin = 2,
    HierLabel = 3,
    LocalLabel = 4,
    LocalPower = 5,
    GlobalPower = 6,
    GlobalLabel = 7,
}

#[derive(Debug, Clone)]
struct Candidate {
    priority: Priority,
    /// Among power pins, KiCad prefers a global power symbol (0), then a local one (1), then a hidden power pin
    /// on an ordinary part (2). Every other candidate is 0.
    rank: u8,
    /// Depth of the sheet it sits on; a shallower name wins a tie.
    depth: usize,
    name: String,
    /// A bus member's name. KiCad renames the nets joined to a bus member on its sheet after that member,
    /// unless they have a global name.
    bus: bool,
    /// Instance path of the sheet it sits on.
    sheet: String,
}

#[derive(Debug, Clone)]
struct PlacedPin {
    reference: String,
    /// Reference with the unit letter for multi-unit parts, such as `U1A`. KiCad uses it in names built
    /// from pin names.
    unit_reference: String,
    number: String,
    name: String,
    kind: PinKind,
    no_connect: bool,
    /// Another pin on the same unit has this pin's name but a different number, split by whether that
    /// other pin is a no-connect type pin. KiCad then adds `-Pad<n>` to names built from this pin.
    name_repeated: (bool, bool),
}

/// One group of connected items on one sheet instance.
#[derive(Debug, Default)]
struct Subgraph {
    pins: Vec<PlacedPin>,
    candidates: Vec<Candidate>,
    /// Keys that join this group to groups elsewhere: label names, power names and sheet ports.
    links: Vec<String>,
    labels: Vec<String>,
    no_connect: bool,
}

#[derive(Default)]
struct Builder {
    /// Every `(reference, unit)` seen so far, to spot duplicate references.
    claimed: HashSet<(String, u32)>,
    subgraphs: Vec<Subgraph>,
    components: BTreeMap<String, Component>,
    warnings: Vec<String>,
    aliases: Aliases,
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

/// True if `p` lies on the segment from `a` to `b`, ends included.
fn on_segment(p: Pt, a: Pt, b: Pt) -> bool {
    let cross = i128::from(b.0 - a.0) * i128::from(p.1 - a.1) - i128::from(b.1 - a.1) * i128::from(p.0 - a.0);
    cross == 0 && p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
}

/// KiCad's default unit letters: 1 → A, 26 → Z, 27 → AA. Projects can switch to `.1`-style suffixes and units
/// can carry display names (`unit_name`); neither is read yet, so generated names on such parts may differ.
fn unit_letter(mut unit: u32) -> String {
    let mut out = Vec::new();
    while unit > 0 {
        unit -= 1;
        out.push(b'A' + (unit % 26) as u8);
        unit /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// KiCad shows a pin named `~` as having no name.
fn shown_name(name: &str) -> &str {
    if name == "~" { "" } else { name }
}

/// What sits at a connection point on a sheet.
enum Item {
    /// A pin on a part, with how it names a net, if it does.
    Pin(PlacedPin, Option<PinDriver>),
    /// The power-input pin of a power symbol, such as `GND` or `+3V3`.
    PowerSymbol(PowerScope, String),
    /// A label, read as a bus when its name is one.
    Label(LabelKind, String, Option<Bus>),
    SheetPin {
        sheet_path: String,
        name: String,
        bus: Option<Bus>,
    },
    /// The wire end of a bus entry. `on_bus` is false for an entry that touches no bus.
    BusEntry {
        on_bus: bool,
    },
    Junction,
    NoConnect,
}

/// Where a sheet instance sits in the hierarchy.
#[derive(Clone, Copy)]
struct Here<'a> {
    /// UUID path, such as `/<root>/<sheet>`.
    inst_path: &'a str,
    /// Name path, such as `/power/`.
    name_path: &'a str,
    depth: usize,
}

/// A label or sheet pin that names a bus.
struct BusLabel {
    priority: Priority,
    kind: BusSource,
    name: String,
    bus: Bus,
}

enum BusSource {
    Label(LabelKind),
    /// A sheet pin, with the instance path of the sheet it leads into.
    SheetPin(String),
}

/// Formats a point in millimetres for messages.
fn mm(p: Pt) -> String {
    format!("({}, {}) mm", p.0 as f64 / 10_000.0, p.1 as f64 / 10_000.0)
}

enum PinDriver {
    /// A hidden power-input pin on a part joins the global net named after the pin.
    HiddenPower(String),
}

/// KiCad escapes `/` in generated net names.
fn escape(s: &str) -> String {
    s.replace('/', "{slash}")
}

fn pins_for<'a>(lib: &'a LibSymbol, libs: &'a HashMap<String, LibSymbol>, unit: u32, style: u32) -> Vec<&'a LibPin> {
    let mut source = lib;
    // A derived symbol takes its pins from its parent.
    if source.pins.is_empty()
        && let Some(parent) = &lib.extends
    {
        let prefix = lib.name.split_once(':').map_or("", |(l, _)| l);
        let full = if prefix.is_empty() {
            parent.clone()
        } else {
            format!("{prefix}:{parent}")
        };
        if let Some(p) = libs.get(&full) {
            source = p;
        }
    }
    source
        .pins
        .iter()
        .filter(|p| (p.unit == 0 || p.unit == unit) && (p.body_style == 0 || p.body_style == style))
        .collect()
}

impl Builder {
    fn walk(&mut self, path: &Path, inst_path: &str, name_path: &str, files: &HashMap<PathBuf, SchematicFile>) {
        let file = &files[path];
        self.sheet(file, inst_path, name_path);
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        for sheet in &file.sheets {
            let child_inst = format!("{inst_path}/{}", sheet.uuid);
            let child_name = format!("{name_path}{}/", sheet.name);
            self.walk(&dir.join(&sheet.file), &child_inst, &child_name, files);
        }
    }

    /// Finds connected groups on one sheet instance.
    fn sheet(&mut self, f: &SchematicFile, inst_path: &str, name_path: &str) {
        let depth = name_path.matches('/').count() - 1;
        for u in &f.unsupported {
            self.warnings.push(format!(
                "sheet {name_path}: `{u}` items are not supported yet and were skipped"
            ));
        }
        let mut items: Vec<(Pt, Item)> = Vec::new();
        // Pairs of item indices joined inside a part (jumpers), not by wires.
        let mut internal: Vec<(usize, usize)> = Vec::new();
        for sym in &f.symbols {
            let lib_key = sym.lib_name.as_deref().unwrap_or(&sym.lib_id);
            let Some(lib) = f.lib_symbols.get(lib_key) else {
                self.warnings.push(format!(
                    "sheet {name_path}: no library data for symbol `{}`",
                    sym.lib_id
                ));
                continue;
            };
            let (reference, unit) = sym.reference_at(inst_path);
            if !reference.starts_with('#') && lib.power.is_none() {
                if reference.ends_with('?') {
                    self.warnings.push(format!(
                        "sheet {name_path}: symbol `{}` has no reference ({reference}); annotate the schematic",
                        sym.lib_id
                    ));
                }
                let key = (reference.clone(), unit);
                match self.claimed.get(&key) {
                    Some(_) => self.warnings.push(format!(
                        "sheet {name_path}: reference {reference} (unit {unit}) is used by more than one symbol; \
                         their pins are merged"
                    )),
                    None => {
                        self.claimed.insert(key);
                    }
                }
            }
            let unit_reference = if lib.unit_count() > 1 {
                format!("{reference}{}", unit_letter(unit))
            } else {
                reference.clone()
            };
            let power = lib
                .power
                .map(|scope| (scope, sym.field("Value").unwrap_or_default().to_string()));
            let is_part = power.is_none() && !reference.starts_with('#');
            if is_part {
                let c = self.components.entry(reference.clone()).or_insert_with(|| Component {
                    reference: reference.clone(),
                    value: sym.field("Value").unwrap_or_default().to_string(),
                    lib_id: sym.lib_id.clone(),
                    footprint: sym.field("Footprint").unwrap_or_default().to_string(),
                    fields: sym.fields.iter().cloned().collect(),
                    dnp: sym.dnp,
                    in_bom: sym.in_bom,
                    on_board: sym.on_board,
                    pins: BTreeMap::new(),
                });
                for (k, v) in &sym.fields {
                    c.fields.entry(k.clone()).or_insert_with(|| v.clone());
                }
            }
            let lib_pins = pins_for(lib, &f.lib_symbols, unit, sym.body_style);
            // Item indices of this symbol's pins, by pin number, for jumpers.
            let mut by_number: HashMap<&str, Vec<usize>> = HashMap::new();
            for lp in &lib_pins {
                let repeated = |nc: bool| {
                    lib_pins.iter().any(|o| {
                        shown_name(&o.name) == shown_name(&lp.name)
                            && o.number != lp.number
                            && (o.kind == "no_connect") == nc
                    })
                };
                let pin = PlacedPin {
                    reference: reference.clone(),
                    unit_reference: unit_reference.clone(),
                    number: lp.number.clone(),
                    name: shown_name(&lp.name).to_string(),
                    kind: PinKind::from_kicad(&lp.kind),
                    no_connect: false,
                    name_repeated: (repeated(false), repeated(true)),
                };
                let at = sym.place(lp.at);
                let item = if is_part {
                    // Older libraries connect a hidden power-input pin on a part to a global net named after it.
                    let implicit = lp.hidden && pin.kind == PinKind::PowerIn && !pin.name.is_empty();
                    let name = pin.name.clone();
                    Item::Pin(pin, implicit.then_some(PinDriver::HiddenPower(name)))
                } else if pin.kind == PinKind::PowerIn
                    && let Some((scope, name)) = &power
                {
                    // Only a power symbol's power-input pin names a net. PWR_FLAG's output pin does not.
                    Item::PowerSymbol(*scope, name.clone())
                } else {
                    continue;
                };
                by_number.entry(lp.number.as_str()).or_default().push(items.len());
                items.push((at, item));
            }
            // KiCad 10 jumpers: pins joined inside the part.
            if lib.duplicate_pins_are_jumpers {
                for group in by_number.values() {
                    for w in group.windows(2) {
                        internal.push((w[0], w[1]));
                    }
                }
            }
            for group in &lib.jumper_pin_groups {
                let members: Vec<usize> = group
                    .iter()
                    .filter_map(|n| by_number.get(n.as_str()))
                    .flatten()
                    .copied()
                    .collect();
                for w in members.windows(2) {
                    internal.push((w[0], w[1]));
                }
            }
        }
        for l in &f.labels {
            let bus = bus::parse(&l.name, &self.aliases);
            items.push((l.at, Item::Label(l.kind, l.name.clone(), bus)));
        }
        for s in &f.sheets {
            for (name, at) in &s.pins {
                items.push((
                    *at,
                    Item::SheetPin {
                        sheet_path: format!("{inst_path}/{}", s.uuid),
                        name: name.clone(),
                        bus: bus::parse(name, &self.aliases),
                    },
                ));
            }
        }
        items.extend(f.junctions.iter().map(|&p| (p, Item::Junction)));
        items.extend(f.no_connects.iter().map(|&p| (p, Item::NoConnect)));
        // A bus entry joins the wire at its free end and nothing at the end that touches a bus. Like KiCad, the
        // wire then reaches a bus member only through a label naming that member.
        let on_bus = |p: Pt| f.buses.iter().any(|&(a, b)| on_segment(p, a, b));
        for &(a, b) in &f.bus_entries {
            match (on_bus(a), on_bus(b)) {
                (true, false) => items.push((b, Item::BusEntry { on_bus: true })),
                (false, true) => items.push((a, Item::BusEntry { on_bus: true })),
                (false, false) => {
                    // Not on a bus, so it is just a short wire.
                    internal.push((items.len(), items.len() + 1));
                    items.push((a, Item::BusEntry { on_bus: false }));
                    items.push((b, Item::BusEntry { on_bus: false }));
                }
                (true, true) => {}
            }
        }

        // Nodes 0..nw are wire segments, nw..base are bus segments, and the rest are items.
        let nw = f.wires.len();
        let base = nw + f.buses.len();
        let mut uf = UnionFind::new(base + items.len());
        let mut wire_points: HashMap<Pt, Vec<usize>> = HashMap::new();
        let mut bus_points: HashMap<Pt, Vec<usize>> = HashMap::new();
        for (i, (a, b)) in f.wires.iter().enumerate() {
            wire_points.entry(*a).or_default().push(i);
            wire_points.entry(*b).or_default().push(i);
        }
        for (i, (a, b)) in f.buses.iter().enumerate() {
            bus_points.entry(*a).or_default().push(nw + i);
            bus_points.entry(*b).or_default().push(nw + i);
        }
        for (i, (p, item)) in items.iter().enumerate() {
            wire_points.entry(*p).or_default().push(base + i);
            // Part pins, no-connect flags and bus entries never join a bus line.
            if matches!(item, Item::Label(..) | Item::SheetPin { .. } | Item::Junction) {
                bus_points.entry(*p).or_default().push(base + i);
            }
        }
        // Like KiCad, items join only where their connection points coincide (a line's points are its two
        // ends), and a wire never joins a bus. The exceptions: a junction joins every line passing through it,
        // and a label joins the lines passing through it when there are at least two.
        for nodes in wire_points.values().chain(bus_points.values()) {
            for w in nodes.windows(2) {
                uf.union(w[0], w[1]);
            }
        }
        let lines: Vec<(Pt, Pt)> = f.wires.iter().chain(&f.buses).copied().collect();
        for (i, (p, item)) in items.iter().enumerate() {
            let through: Vec<usize> = match item {
                Item::Junction | Item::Label(..) => (0..base)
                    .filter(|&li| on_segment(*p, lines[li].0, lines[li].1))
                    .collect(),
                _ => continue,
            };
            if matches!(item, Item::Label(..)) && through.len() < 2 {
                continue;
            }
            for li in through {
                uf.union(base + i, li);
            }
        }
        for (a, b) in internal {
            uf.union(base + a, base + b);
        }

        let mut groups: BTreeMap<usize, Vec<(Pt, Item)>> = BTreeMap::new();
        for (i, item) in items.into_iter().enumerate() {
            groups.entry(uf.find(base + i)).or_default().push(item);
        }
        let bus_roots: HashSet<usize> = (nw..base).map(|n| uf.find(n)).collect();
        let at = Here {
            inst_path,
            name_path,
            depth,
        };
        for (root, members) in groups {
            let has_pins = members
                .iter()
                .any(|(_, it)| matches!(it, Item::Pin(..) | Item::PowerSymbol(..)));
            let has_bus_name = members
                .iter()
                .any(|(_, it)| matches!(it, Item::Label(_, _, Some(_)) | Item::SheetPin { bus: Some(_), .. }));
            if !has_pins && (bus_roots.contains(&root) || has_bus_name) {
                self.bus_group(members, &at);
            } else {
                self.net_group(members, &at);
            }
        }
    }

    /// One connected group of wires, pins and labels.
    fn net_group(&mut self, members: Vec<(Pt, Item)>, at: &Here<'_>) {
        let Here {
            inst_path,
            name_path,
            depth,
        } = *at;
        let mut g = Subgraph::default();
        let mut bus_entry = None;
        for (p, item) in members {
            match item {
                Item::Pin(pin, None) => g.pins.push(pin),
                Item::Pin(pin, Some(PinDriver::HiddenPower(name))) => {
                    g.pins.push(pin);
                    g.links.push(format!("local:{inst_path}:{name}"));
                    g.candidates.push(Candidate {
                        priority: Priority::GlobalPower,
                        rank: 2,
                        depth,
                        name: name.clone(),
                        bus: false,
                        sheet: inst_path.to_string(),
                    });
                    g.links.push(format!("global:{name}"));
                    g.labels.push(name);
                }
                Item::PowerSymbol(scope, name) => {
                    if scope == PowerScope::Global {
                        g.links.push(format!("local:{inst_path}:{name}"));
                    }
                    let (priority, rank, link, shown) = match scope {
                        PowerScope::Global => (Priority::GlobalPower, 0, format!("global:{name}"), name.clone()),
                        PowerScope::Local => (
                            Priority::LocalPower,
                            1,
                            format!("local:{inst_path}:{name}"),
                            format!("{name_path}{name}"),
                        ),
                    };
                    g.candidates.push(Candidate {
                        priority,
                        rank,
                        depth,
                        name: shown,
                        bus: false,
                        sheet: inst_path.to_string(),
                    });
                    g.labels.push(name);
                    g.links.push(link);
                }
                Item::Label(_, name, Some(_)) | Item::SheetPin { name, bus: Some(_), .. } => {
                    self.warnings.push(format!(
                        "sheet {name_path}: bus name `{name}` at {} is on a wire, not a bus; it was ignored",
                        mm(p)
                    ));
                }
                Item::Label(kind, name, None) => {
                    let (priority, shown, link) = match kind {
                        LabelKind::Global => (Priority::GlobalLabel, name.clone(), format!("global:{name}")),
                        LabelKind::Local => (
                            Priority::LocalLabel,
                            format!("{name_path}{name}"),
                            format!("local:{inst_path}:{name}"),
                        ),
                        LabelKind::Hierarchical => (
                            Priority::HierLabel,
                            format!("{name_path}{name}"),
                            format!("port:{inst_path}:{name}"),
                        ),
                    };
                    // KiCad joins every label and power name on one sheet by bare name, whatever its kind.
                    if kind != LabelKind::Local {
                        g.links.push(format!("local:{inst_path}:{name}"));
                    }
                    g.candidates.push(Candidate {
                        priority,
                        rank: 0,
                        depth,
                        name: shown,
                        bus: false,
                        sheet: inst_path.to_string(),
                    });
                    g.labels.push(name);
                    g.links.push(link);
                }
                Item::SheetPin {
                    sheet_path,
                    name,
                    bus: None,
                } => {
                    g.candidates.push(Candidate {
                        priority: Priority::SheetPin,
                        rank: 0,
                        depth,
                        name: format!("{name_path}{name}"),
                        bus: false,
                        sheet: inst_path.to_string(),
                    });
                    g.links.push(format!("port:{sheet_path}:{name}"));
                }
                Item::BusEntry { on_bus } => {
                    if on_bus {
                        bus_entry = Some(p);
                    }
                }
                Item::Junction => {}
                Item::NoConnect => g.no_connect = true,
            }
        }
        if let Some(p) = bus_entry
            && g.candidates.is_empty()
            && !g.pins.is_empty()
        {
            self.warnings.push(format!(
                "sheet {name_path}: the wire on the bus entry at {} has no label, so it joins no bus member",
                mm(p)
            ));
        }
        if g.no_connect {
            for p in &mut g.pins {
                p.no_connect = true;
            }
        }
        if !g.pins.is_empty() || !g.links.is_empty() {
            self.subgraphs.push(g);
        }
    }

    /// One connected group of bus lines and bus labels. Each member of the bus becomes a group with no pins
    /// that joins, by name or through the hierarchy, the nets that carry that member.
    fn bus_group(&mut self, members: Vec<(Pt, Item)>, at: &Here<'_>) {
        let Here {
            inst_path,
            name_path,
            depth,
        } = *at;
        let mut labels: Vec<BusLabel> = Vec::new();
        for (p, item) in members {
            match item {
                Item::Label(kind, name, Some(bus)) => labels.push(BusLabel {
                    priority: match kind {
                        LabelKind::Global => Priority::GlobalLabel,
                        LabelKind::Local => Priority::LocalLabel,
                        LabelKind::Hierarchical => Priority::HierLabel,
                    },
                    kind: BusSource::Label(kind),
                    name,
                    bus,
                }),
                Item::SheetPin {
                    sheet_path,
                    name,
                    bus: Some(bus),
                } => labels.push(BusLabel {
                    priority: Priority::SheetPin,
                    kind: BusSource::SheetPin(sheet_path),
                    name,
                    bus,
                }),
                Item::Label(_, name, None) | Item::SheetPin { name, .. } => {
                    self.warnings.push(format!(
                        "sheet {name_path}: `{name}` at {} is on a bus but does not name a bus; it was ignored",
                        mm(p)
                    ));
                }
                _ => {}
            }
        }
        let shown = |l: &BusLabel| match l.kind {
            BusSource::Label(LabelKind::Global) => l.name.clone(),
            _ => format!("{name_path}{}", l.name),
        };
        // KiCad's driver choice for buses: priority, then the bus that holds the other, then the name.
        let Some(driver) = labels.iter().min_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| match (a.bus.contains_all(&b.bus), b.bus.contains_all(&a.bus)) {
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    _ => Ordering::Equal,
                })
                .then_with(|| shown(a).cmp(&shown(b)))
        }) else {
            return;
        };
        let start = self.subgraphs.len();
        for m in &driver.bus.members {
            let name = match driver.kind {
                BusSource::Label(LabelKind::Global) => m.name.clone(),
                _ => format!("{name_path}{}", m.name),
            };
            self.subgraphs.push(Subgraph {
                candidates: vec![Candidate {
                    priority: driver.priority,
                    rank: 0,
                    depth,
                    name,
                    bus: true,
                    sheet: inst_path.to_string(),
                }],
                ..Default::default()
            });
        }
        for l in &labels {
            if l.bus.kind != driver.bus.kind {
                self.warnings.push(format!(
                    "sheet {name_path}: bus `{}` is a different kind of bus from `{}` on the same bus line; \
                     it was ignored",
                    l.name, driver.name
                ));
                continue;
            }
            let is_driver = std::ptr::eq(l, driver);
            for (j, m) in l.bus.members.iter().enumerate() {
                let Some(s) = driver.bus.find(m) else { continue };
                let g = &mut self.subgraphs[start + s];
                // KiCad matches a sheet pin's members to nets by name only when the pin names the bus.
                if is_driver || !matches!(l.kind, BusSource::SheetPin(_)) {
                    g.links.push(format!("local:{inst_path}:{}", m.name));
                    g.labels.push(m.name.clone());
                }
                match &l.kind {
                    BusSource::Label(LabelKind::Global) => g.links.push(format!("global:{}", m.name)),
                    BusSource::Label(LabelKind::Hierarchical) => {
                        g.links.push(format!("busport:{inst_path}:{}:{j}", l.name));
                    }
                    BusSource::SheetPin(child) => g.links.push(format!("busport:{child}:{}:{j}", l.name)),
                    BusSource::Label(LabelKind::Local) => {}
                }
            }
        }
    }

    /// Joins groups that share a name or a sheet port, then names each net.
    fn finish(mut self) -> Design {
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

        let mut nets = Vec::new();
        for members in merged.values() {
            let mut pins: Vec<PlacedPin> = Vec::new();
            // Each group's own name source, chosen as KiCad does within one group.
            let mut drivers: Vec<Candidate> = Vec::new();
            let mut labels = BTreeSet::new();
            for &m in members {
                let g = &mut self.subgraphs[m];
                pins.append(&mut g.pins);
                drivers.extend(best_candidate(&g.candidates, true).cloned());
                labels.extend(g.labels.iter().cloned());
            }
            if pins.is_empty() {
                continue;
            }
            // Jumpered pins that share a number are separate pins to KiCad, so count before merging them.
            let pin_items = pins.len();
            pins.sort_by(|a, b| (&a.reference, &a.number).cmp(&(&b.reference, &b.number)));
            pins.dedup_by(|a, b| a.reference == b.reference && a.number == b.number);
            let name = net_name(&drivers, &pins, pin_items);
            let index = nets.len();
            for p in &pins {
                if let Some(c) = self.components.get_mut(&p.reference) {
                    let entry = c.pins.entry(p.number.clone()).or_insert_with(|| Pin {
                        number: p.number.clone(),
                        name: p.name.clone(),
                        kind: p.kind,
                        no_connect: p.no_connect,
                        net: index,
                    });
                    entry.no_connect |= p.no_connect;
                }
            }
            nets.push(Net {
                name,
                pins: pins.iter().map(|p| (p.reference.clone(), p.number.clone())).collect(),
                labels: labels.into_iter().collect(),
            });
        }
        // A pin number used twice in one symbol on two different nets is a schematic error KiCad also flags.
        for (r, c) in &self.components {
            for (num, pin) in &c.pins {
                let count = nets
                    .iter()
                    .filter(|n| n.pins.iter().any(|(pr, pn)| pr == r && pn == num))
                    .count();
                if count > 1 {
                    self.warnings.push(format!(
                        "{r} pin {num} is on {count} different nets; using {}",
                        nets[pin.net].name
                    ));
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

/// KiCad's `SCH_PIN::GetDefaultNetName`, without the `Net-(` or `unconnected-(` prefix.
fn pin_net_name(p: &PlacedPin, unconnected: bool) -> String {
    let unconnected = unconnected || p.kind == PinKind::NoConnect;
    let repeated = if unconnected {
        p.name_repeated.1
    } else {
        p.name_repeated.0
    };
    if !p.name.is_empty() && p.name != p.number {
        // Pin names can repeat between units, so these names carry the unit letter.
        let mut name = format!("{}-{}", p.unit_reference, escape(&p.name));
        if unconnected || repeated {
            let _ = write!(name, "-Pad{}", escape(&p.number));
        }
        name
    } else {
        format!("{}-Pad{}", p.reference, escape(&p.number))
    }
}

fn low_quality(name: &str) -> bool {
    name.contains("-Pad")
}

/// Picks a name source as KiCad's `ResolveDrivers` does. The power-symbol rank only applies within one group
/// (`with_rank`); across groups KiCad compares priority and then name.
fn best_candidate(candidates: &[Candidate], with_rank: bool) -> Option<&Candidate> {
    candidates.iter().min_by(|a, b| {
        let rank = if with_rank {
            a.rank.cmp(&b.rank)
        } else {
            Ordering::Equal
        };
        b.priority
            .cmp(&a.priority)
            .then(rank)
            .then(a.depth.cmp(&b.depth))
            .then(low_quality(&a.name).cmp(&low_quality(&b.name)))
            .then(a.name.cmp(&b.name))
    })
}

/// `drivers` holds one name source per joined group. `pin_items` counts part pins before pins that share a
/// number were merged.
fn net_name(drivers: &[Candidate], pins: &[PlacedPin], pin_items: usize) -> String {
    // KiCad first renames every net joined to a bus member on the bus's sheet after that member, unless the net
    // has a global name, and only then picks the best name across the hierarchy. So a sheet with a bus member
    // on this net offers only the member's name, and global names.
    let bus_sheets: HashSet<&str> = drivers.iter().filter(|c| c.bus).map(|c| c.sheet.as_str()).collect();
    let offered: Vec<Candidate> = drivers
        .iter()
        .filter(|c| c.bus || c.priority >= Priority::GlobalPower || !bus_sheets.contains(c.sheet.as_str()))
        .cloned()
        .collect();
    if let Some(c) = best_candidate(&offered, false) {
        return c.name.clone();
    }
    if let ([p], 1) = (pins, pin_items) {
        return format!("unconnected-({})", pin_net_name(p, true));
    }
    // Like KiCad, prefer a name built from a pin name (`U1-SW`) over one built from a pad number (`C3-Pad1`).
    pins.iter()
        .map(|p| {
            let name = pin_net_name(p, false);
            let prefix = if p.kind == PinKind::NoConnect {
                "unconnected"
            } else {
                "Net"
            };
            format!("{prefix}-({name})")
        })
        .min_by_key(|n| (low_quality(n), n.clone()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet_with_alias(members: &[&str], child: Option<&str>) -> SchematicFile {
        SchematicFile {
            bus_aliases: vec![("PWR".to_string(), members.iter().map(ToString::to_string).collect())],
            sheets: child
                .map(|file| schematic::Sheet {
                    file: file.to_string(),
                    ..Default::default()
                })
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn project_bus_aliases_replace_sheet_ones() {
        let dir = std::env::temp_dir().join(format!("gerberus-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("board.kicad_sch");
        let files = HashMap::from([(root.clone(), sheet_with_alias(&["A"], None))]);
        let mut warnings = Vec::new();
        let project = dir.join("board.kicad_pro");
        let aliases = |warnings: &mut Vec<String>, files: &HashMap<PathBuf, SchematicFile>| {
            bus_aliases(&root, files, warnings).get("PWR").cloned()
        };

        // No project file, or an empty table in an older project file: the sheet's alias.
        let _ = std::fs::remove_file(&project);
        assert_eq!(aliases(&mut warnings, &files).unwrap(), ["A"]);
        std::fs::write(
            &project,
            r#"{"meta": {"version": 3}, "schematic": {"bus_aliases": {}}}"#,
        )
        .unwrap();
        assert_eq!(aliases(&mut warnings, &files).unwrap(), ["A"]);

        // From schema 4 an empty table means no aliases at all.
        std::fs::write(
            &project,
            r#"{"meta": {"version": 4}, "schematic": {"bus_aliases": {}}}"#,
        )
        .unwrap();
        assert_eq!(aliases(&mut warnings, &files), None);

        // A project table replaces every sheet alias, trimmed as KiCad trims it.
        std::fs::write(
            &project,
            r#"{"meta": {"version": 4}, "schematic": {"bus_aliases": {" PWR ": ["B", " C ", ""]}}}"#,
        )
        .unwrap();
        assert_eq!(aliases(&mut warnings, &files).unwrap(), ["B", "C"]);
        assert!(warnings.is_empty(), "{warnings:?}");

        // Two sheets that disagree: the later one in the hierarchy wins, with a warning. The child's name sorts
        // first, so this is not path order.
        std::fs::remove_file(&project).unwrap();
        let files = HashMap::from([
            (root.clone(), sheet_with_alias(&["A"], Some("a_child.kicad_sch"))),
            (dir.join("a_child.kicad_sch"), sheet_with_alias(&["Z"], None)),
        ]);
        assert_eq!(aliases(&mut warnings, &files).unwrap(), ["Z"]);
        assert_eq!(warnings.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
