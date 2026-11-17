//! One Altium schematic sheet (`.SchDoc`): its parts, wires and connection objects, with every position in
//! hundred-thousandths of an Altium unit (1 unit = 10 mil).

use std::collections::BTreeMap;
use std::path::Path;

use super::records::{self, Record};

/// A point in hundred-thousandths of an Altium unit.
pub type Pt = (i64, i64);

/// One Altium unit.
pub const UNIT: i64 = 100_000;

#[derive(Debug, Clone, Default)]
pub struct SchDoc {
    pub components: Vec<SchComponent>,
    /// Wire polylines.
    pub wires: Vec<Vec<Pt>>,
    /// Bus polylines.
    pub buses: Vec<Vec<Pt>>,
    /// Bus entries, end to end.
    pub bus_entries: Vec<(Pt, Pt)>,
    pub junctions: Vec<Pt>,
    pub labels: Vec<(Pt, String)>,
    pub power_ports: Vec<(Pt, String)>,
    pub ports: Vec<Port>,
    pub sheet_symbols: Vec<SheetSymbol>,
    /// No-ERC markers.
    pub no_erc: Vec<Pt>,
    /// Harness connectors, which gather wires into a signal harness.
    pub harness_connectors: Vec<HarnessConnector>,
    /// Signal harness polylines, which carry bundles between connectors, ports and sheet entries.
    pub harnesses: Vec<Vec<Pt>>,
    /// The sheet's `SheetNumber` parameter, if it has one.
    pub sheet_number: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SchComponent {
    pub designator: String,
    pub lib_ref: String,
    pub description: String,
    /// Which part of a multi-part component this instance draws.
    pub part_id: i64,
    pub part_count: i64,
    pub display_mode: i64,
    /// `COMPONENTKIND`: 0 standard, 1 mechanical, 2 graphical, 3 net tie (in BOM), 4 net tie, 5 standard (no
    /// BOM), 6 jumper.
    pub kind: i64,
    /// Parameters by name, as written. Look them up with [`SchComponent::param`], which ignores case.
    pub params: BTreeMap<String, String>,
    pub footprint: String,
    /// Every pin of every part and display mode; [`SchComponent::drawn`] says which ones this instance draws.
    pub pins: Vec<SchPin>,
}

impl SchComponent {
    /// True if this instance draws `pin`: the pin belongs to its part (or every part) and display mode.
    pub fn drawn(&self, pin: &SchPin) -> bool {
        (pin.part_id <= 0 || pin.part_id == self.part_id) && pin.display_mode == self.display_mode
    }

    /// A parameter, following `=Other` indirections the way Altium shows them.
    pub fn param(&self, name: &str) -> Option<&str> {
        let get = |n: &str| {
            self.params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.as_str())
        };
        let mut v = get(name)?;
        for _ in 0..4 {
            match v.strip_prefix('=').and_then(get) {
                Some(next) => v = next,
                None => break,
            }
        }
        Some(v)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchPin {
    /// The pin number, Altium's `Designator`.
    pub number: String,
    pub name: String,
    /// `ELECTRICAL`: 0 input, 1 I/O, 2 output, 3 open collector, 4 passive, 5 hi-Z, 6 open emitter, 7 power.
    pub electrical: i64,
    pub part_id: i64,
    pub display_mode: i64,
    /// The end on the part body.
    pub base: Pt,
    /// The end a wire connects to.
    pub tip: Pt,
    pub hidden: bool,
    /// A hidden pin joins the net of this name, as older power pins do.
    pub hidden_net: String,
}

#[derive(Debug, Clone, Default)]
pub struct Port {
    pub name: String,
    /// The two ends of the port's bar; a wire may land on either.
    pub ends: [Pt; 2],
    pub harness_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct SheetSymbol {
    /// The symbol's designator, such as `U_Power` or `Repeat(CH,1,4)`.
    pub name: String,
    /// The child document's file name.
    pub file: String,
    pub entries: Vec<SheetEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct HarnessConnector {
    /// Where the bundle leaves the connector.
    pub primary: Pt,
    /// One per signal: a wire lands on the entry, or another harness on a nested bundle.
    pub entries: Vec<SheetEntry>,
}

#[derive(Debug, Clone, Default)]
pub struct SheetEntry {
    pub name: String,
    pub at: Pt,
    pub harness_type: String,
}

/// Reads a binary `.SchDoc`, or the ASCII form Altium can also save.
pub fn load(path: &Path) -> Result<SchDoc, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (recs, additional) = if raw.starts_with(b"|HEADER=") {
        let recs: Vec<Record> = raw
            .split(|&b| b == b'\n')
            .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
            .filter(|l| !l.is_empty())
            .map(records::parse_text)
            .collect();
        (recs, Vec::new())
    } else {
        let header = records::read_stream(path, "FileHeader")?
            .ok_or_else(|| format!("{}: no FileHeader stream; is this a schematic?", path.display()))?;
        let recs = records::parse_stream(&header).map_err(|e| format!("{}: {e}", path.display()))?;
        let additional = match records::read_stream(path, "Additional")? {
            Some(d) => records::parse_stream(&d).map_err(|e| format!("{}: Additional: {e}", path.display()))?,
            None => Vec::new(),
        };
        (recs, additional)
    };
    Ok(from_records(&recs, &additional))
}

/// How far along its side of the box a sheet or harness entry sits: steps of 10 units, plus millionths of one.
fn along(r: &Record) -> i64 {
    r.int("DISTANCEFROMTOP").clamp(-1_000_000, 1_000_000) * 10 * UNIT
        + r.int("DISTANCEFROMTOP_FRAC1").clamp(-1_000_000, 1_000_000) * 10 * UNIT / 1_000_000
}

fn polyline(r: &Record) -> Vec<Pt> {
    // A record holds at most as many vertices as it has fields.
    let n = (r.int("LOCATIONCOUNT").max(0) as usize).min(r.fields.len());
    (1..=n)
        .map(|i| (r.coord(&format!("X{i}")), r.coord(&format!("Y{i}"))))
        .collect()
}

fn location(r: &Record) -> Pt {
    (r.coord("LOCATION.X"), r.coord("LOCATION.Y"))
}

/// Builds a sheet from its records. `recs[0]` is the header; `OWNERINDEX` counts from the record after it.
pub fn from_records(recs: &[Record], additional: &[Record]) -> SchDoc {
    let body = recs.get(1..).unwrap_or(&[]);
    let mut doc = SchDoc::default();
    // Record index of each component, and of each sheet symbol, to the position in `doc`.
    let mut comp_at: BTreeMap<usize, usize> = BTreeMap::new();
    let mut sym_at: BTreeMap<usize, usize> = BTreeMap::new();
    let mut last_component = None;
    for (i, r) in body.iter().enumerate() {
        match r.kind() {
            1 => {
                comp_at.insert(i, doc.components.len());
                last_component = Some(i);
                doc.components.push(SchComponent {
                    lib_ref: r.text("LIBREFERENCE").to_string(),
                    description: r.text("COMPONENTDESCRIPTION").to_string(),
                    part_id: r.int("CURRENTPARTID"),
                    part_count: r.int("PARTCOUNT"),
                    display_mode: r.int("DISPLAYMODE"),
                    kind: r.int("COMPONENTKIND"),
                    ..Default::default()
                });
            }
            15 => {
                sym_at.insert(i, doc.sheet_symbols.len());
                doc.sheet_symbols.push(SheetSymbol::default());
            }
            _ => {}
        }
        // Binary pins name no owner: they follow the component they belong to.
        if r.kind() == 2
            && r.owner().is_none()
            && let Some(c) = last_component
        {
            let mut r = r.clone();
            r.fields.insert("OWNERINDEX".into(), c.to_string());
            pin(&mut doc, &comp_at, &r);
        }
    }
    // The component a record belongs to, following owners up (a footprint sits under an implementation list).
    let owning_component = |r: &Record| {
        let mut owner = r.owner();
        for _ in 0..4 {
            let o = owner?;
            if let Some(&c) = comp_at.get(&o) {
                return Some(c);
            }
            owner = body.get(o)?.owner();
        }
        None
    };
    for r in body {
        match r.kind() {
            2 if r.owner().is_some() => pin(&mut doc, &comp_at, r),
            16 => {
                let Some((owner, &si)) = r.owner().and_then(|o| Some((o, sym_at.get(&o)?))) else {
                    continue;
                };
                let sym = &body[owner];
                let (x, y) = location(sym);
                let (w, h) = (sym.coord("XSIZE"), sym.coord("YSIZE"));
                let along = along(r);
                let at = on_edge((x, y), (w, h), r.int("SIDE"), along);
                doc.sheet_symbols[si].entries.push(SheetEntry {
                    name: r.text("NAME").to_string(),
                    at,
                    harness_type: r.text("HARNESSTYPE").to_string(),
                });
            }
            32 | 33 => {
                if let Some(&si) = r.owner().and_then(|o| sym_at.get(&o)) {
                    let text = r.text("TEXT").to_string();
                    if r.kind() == 32 {
                        doc.sheet_symbols[si].name = text;
                    } else {
                        doc.sheet_symbols[si].file = text;
                    }
                }
            }
            17 => doc.power_ports.push((location(r), r.text("TEXT").to_string())),
            18 => {
                let at = location(r);
                let w = r.coord("WIDTH");
                // Styles 4 and up are vertical ports, drawn upward from their location.
                let end = if r.int("STYLE") >= 4 {
                    (at.0, at.1 + w)
                } else {
                    (at.0 + w, at.1)
                };
                doc.ports.push(Port {
                    name: r.text("NAME").to_string(),
                    ends: [at, end],
                    harness_type: r.text("HARNESSTYPE").to_string(),
                });
            }
            22 => doc.no_erc.push(location(r)),
            25 => doc.labels.push((location(r), r.text("TEXT").to_string())),
            26 => doc.buses.push(polyline(r)),
            27 => doc.wires.push(polyline(r)),
            29 => doc.junctions.push(location(r)),
            37 => doc
                .bus_entries
                .push((location(r), (r.coord("CORNER.X"), r.coord("CORNER.Y")))),
            34 => {
                if let Some(c) = owning_component(r) {
                    doc.components[c].designator = r.text("TEXT").to_string();
                }
            }
            41 => match owning_component(r) {
                Some(c) => {
                    doc.components[c]
                        .params
                        .insert(r.text("NAME").to_string(), r.text("TEXT").to_string());
                }
                None if r.owner().is_none() && r.text("NAME").eq_ignore_ascii_case("SheetNumber") => {
                    doc.sheet_number = Some(r.text("TEXT").to_string());
                }
                None => {}
            },
            45 => {
                if r.text("MODELTYPE").eq_ignore_ascii_case("PCBLIB")
                    && let Some(c) = owning_component(r)
                {
                    let c = &mut doc.components[c];
                    if c.footprint.is_empty() || r.flag("ISCURRENT") {
                        c.footprint = r.text("MODELNAME").to_string();
                    }
                }
            }
            _ => {}
        }
    }
    harness_records(&mut doc, additional);
    doc
}

/// Point on a box's edge: `side` 0 left, 1 right, 2 top, 3 bottom; `along` measured down a vertical edge or
/// rightward along a horizontal one from the top-left corner `(x, y)` (Altium's y axis points up).
fn on_edge((x, y): Pt, (w, h): Pt, side: i64, along: i64) -> Pt {
    match side {
        1 => (x + w, y - along),
        2 => (x + along, y),
        3 => (x + along, y - h),
        _ => (x, y - along),
    }
}

/// Signal harness records, in the `Additional` stream. A connector owns the entries that follow it.
fn harness_records(doc: &mut SchDoc, additional: &[Record]) {
    // The current connector's top-left corner, size and bundle side, for the entries that follow it.
    let mut boxed: Option<(Pt, Pt, i64)> = None;
    for r in additional {
        match r.kind() {
            215 => {
                let at = location(r);
                let size = (r.coord("XSIZE"), r.coord("YSIZE"));
                let side = r.int("HARNESSCONNECTORSIDE");
                doc.harness_connectors.push(HarnessConnector {
                    primary: on_edge(at, size, side, r.coord("PRIMARYCONNECTIONPOSITION")),
                    entries: Vec::new(),
                });
                boxed = Some((at, size, side));
            }
            216 => {
                let (Some(c), Some((at, size, side))) = (doc.harness_connectors.last_mut(), boxed) else {
                    continue;
                };
                // Entries sit on the edge opposite the bundle unless they name their own.
                let entry_side = match r.get("SIDE") {
                    Some(v) => v.trim().parse().unwrap_or(0),
                    None => [1, 0, 3, 2][side.clamp(0, 3) as usize],
                };
                let along = along(r);
                c.entries.push(SheetEntry {
                    name: r.text("NAME").to_string(),
                    at: on_edge(at, size, entry_side, along),
                    harness_type: r.text("HARNESSTYPE").to_string(),
                });
            }
            218 => doc.harnesses.push(polyline(r)),
            _ => {}
        }
    }
}

fn pin(doc: &mut SchDoc, comp_at: &BTreeMap<usize, usize>, r: &Record) {
    let Some(&c) = r.owner().and_then(|o| comp_at.get(&o)) else {
        return;
    };
    let base = location(r);
    let len = r.coord("PINLENGTH");
    let conglomerate = r.int("PINCONGLOMERATE");
    let tip = match conglomerate & 3 {
        0 => (base.0 + len, base.1),
        1 => (base.0, base.1 + len),
        2 => (base.0 - len, base.1),
        _ => (base.0, base.1 - len),
    };
    doc.components[c].pins.push(SchPin {
        number: r.text("DESIGNATOR").to_string(),
        name: r.text("NAME").to_string(),
        electrical: r.int("ELECTRICAL"),
        part_id: r.int("OWNERPARTID"),
        display_mode: r.int("OWNERPARTDISPLAYMODE"),
        base,
        tip,
        hidden: conglomerate & 4 != 0,
        hidden_net: r.text("HIDDENNETNAME").to_string(),
    });
}
