//! Works out which pins connect, across every sheet of a KiCad schematic, and names the nets as KiCad does.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::schematic::{self, LabelKind, LibPin, LibSymbol, PowerScope, Pt, SchematicFile};
use crate::design::{Component, Design, Net, Pin, PinKind};
use crate::sexpr;

/// Loads a root `.kicad_sch` and every sheet below it.
pub fn load(path: &Path) -> Result<Design, String> {
    let mut files: HashMap<PathBuf, SchematicFile> = HashMap::new();
    read_tree(path, &mut files, &mut Vec::new())?;
    let root = &files[path];
    let mut builder = Builder::default();
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
    /// Depth of the sheet it sits on; a shallower name wins a tie.
    depth: usize,
    name: String,
}

#[derive(Debug, Clone)]
struct PlacedPin {
    reference: String,
    number: String,
    name: String,
    kind: PinKind,
    no_connect: bool,
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
    subgraphs: Vec<Subgraph>,
    components: BTreeMap<String, Component>,
    warnings: Vec<String>,
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
    let cross = (b.0 - a.0) as i128 * (p.1 - a.1) as i128 - (b.1 - a.1) as i128 * (p.0 - a.0) as i128;
    cross == 0 && p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0) && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1)
}

/// What sits at a connection point on a sheet.
enum Item {
    Pin(PlacedPin, Option<(PowerScope, String)>),
    Label(LabelKind, String),
    SheetPin { sheet_path: String, name: String },
    Junction,
    NoConnect,
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
        let prefix = lib.name.split_once(':').map(|(l, _)| l).unwrap_or("");
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
            for lp in pins_for(lib, &f.lib_symbols, unit, sym.body_style) {
                let pin = PlacedPin {
                    reference: reference.clone(),
                    number: lp.number.clone(),
                    name: lp.name.clone(),
                    kind: PinKind::from_kicad(&lp.kind),
                    no_connect: false,
                };
                let at = sym.place(lp.at);
                if is_part && lp.hidden && pin.kind == PinKind::PowerIn && !lp.name.is_empty() {
                    // Older libraries connect hidden power pins to a global net named after the pin.
                    items.push((at, Item::Pin(pin.clone(), Some((PowerScope::Global, lp.name.clone())))));
                    continue;
                }
                if is_part {
                    items.push((at, Item::Pin(pin, None)));
                } else if pin.kind == PinKind::PowerIn {
                    // Only a power symbol's power-input pin names a net. PWR_FLAG's output pin does not.
                    if let Some(power) = &power {
                        items.push((at, Item::Pin(pin, Some(power.clone()))));
                    }
                }
            }
        }
        for l in &f.labels {
            items.push((l.at, Item::Label(l.kind, l.name.clone())));
        }
        for s in &f.sheets {
            for (name, at) in &s.pins {
                items.push((
                    *at,
                    Item::SheetPin {
                        sheet_path: format!("{inst_path}/{}", s.uuid),
                        name: name.clone(),
                    },
                ));
            }
        }
        items.extend(f.junctions.iter().map(|&p| (p, Item::Junction)));
        items.extend(f.no_connects.iter().map(|&p| (p, Item::NoConnect)));

        // Nodes 0..wires are wire segments; the rest are items.
        let nw = f.wires.len();
        let mut uf = UnionFind::new(nw + items.len());
        let mut by_point: HashMap<Pt, Vec<usize>> = HashMap::new();
        for (i, (a, b)) in f.wires.iter().enumerate() {
            by_point.entry(*a).or_default().push(i);
            by_point.entry(*b).or_default().push(i);
        }
        for (i, (p, _)) in items.iter().enumerate() {
            by_point.entry(*p).or_default().push(nw + i);
        }
        for (p, nodes) in &by_point {
            for w in nodes.windows(2) {
                uf.union(w[0], w[1]);
            }
            // A wire end, pin, label or junction anywhere along a wire joins it.
            for (wi, (a, b)) in f.wires.iter().enumerate() {
                if on_segment(*p, *a, *b) {
                    uf.union(nodes[0], wi);
                }
            }
        }

        let mut groups: BTreeMap<usize, Subgraph> = BTreeMap::new();
        for (i, (_, item)) in items.into_iter().enumerate() {
            let g = groups.entry(uf.find(nw + i)).or_default();
            match item {
                Item::Pin(pin, None) => g.pins.push(pin),
                Item::Pin(_, Some((scope, name))) => {
                    let (priority, link) = match scope {
                        PowerScope::Global => (Priority::GlobalPower, format!("global:{name}")),
                        PowerScope::Local => (Priority::LocalPower, format!("local:{inst_path}:{name}")),
                    };
                    let shown = if scope == PowerScope::Global {
                        name.clone()
                    } else {
                        format!("{name_path}{name}")
                    };
                    g.candidates.push(Candidate {
                        priority,
                        depth,
                        name: shown,
                    });
                    g.labels.push(name);
                    g.links.push(link);
                }
                Item::Label(kind, name) => {
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
                    g.candidates.push(Candidate {
                        priority,
                        depth,
                        name: shown,
                    });
                    g.labels.push(name);
                    g.links.push(link);
                }
                Item::SheetPin { sheet_path, name } => {
                    g.candidates.push(Candidate {
                        priority: Priority::SheetPin,
                        depth,
                        name: format!("{name_path}{name}"),
                    });
                    g.links.push(format!("port:{sheet_path}:{name}"));
                }
                Item::Junction => {}
                Item::NoConnect => g.no_connect = true,
            }
        }
        for (_, mut g) in groups {
            if g.no_connect {
                for p in &mut g.pins {
                    p.no_connect = true;
                }
            }
            if !g.pins.is_empty() || !g.links.is_empty() {
                self.subgraphs.push(g);
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
            let mut candidates: Vec<Candidate> = Vec::new();
            let mut labels = BTreeSet::new();
            for &m in members {
                let g = &mut self.subgraphs[m];
                pins.append(&mut g.pins);
                candidates.extend(g.candidates.iter().cloned());
                labels.extend(g.labels.iter().cloned());
            }
            if pins.is_empty() {
                continue;
            }
            pins.sort_by(|a, b| (&a.reference, &a.number).cmp(&(&b.reference, &b.number)));
            pins.dedup_by(|a, b| a.reference == b.reference && a.number == b.number);
            let name = net_name(&candidates, &pins);
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

fn pin_label(p: &PlacedPin) -> String {
    if p.name.is_empty() || p.name == "~" {
        format!("{}-Pad{}", p.reference, escape(&p.number))
    } else {
        format!("{}-{}", p.reference, escape(&p.name))
    }
}

fn net_name(candidates: &[Candidate], pins: &[PlacedPin]) -> String {
    let best = candidates.iter().min_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then(a.depth.cmp(&b.depth))
            .then(a.name.cmp(&b.name))
    });
    if let Some(c) = best {
        return c.name.clone();
    }
    if pins.len() == 1 {
        let p = &pins[0];
        let name = if p.name.is_empty() || p.name == "~" {
            format!("{}-Pad{}", p.reference, escape(&p.number))
        } else {
            format!("{}-{}-Pad{}", p.reference, escape(&p.name), escape(&p.number))
        };
        return format!("unconnected-({name})");
    }
    // Like KiCad, prefer a name built from a pin name (`U1-SW`) over one built from a pad number (`C3-Pad1`).
    let best = pins
        .iter()
        .map(pin_label)
        .min_by_key(|n| (n.contains("-Pad"), n.clone()))
        .unwrap_or_default();
    format!("Net-({best})")
}
