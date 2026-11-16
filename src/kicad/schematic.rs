//! Reads one `.kicad_sch` file into plain structs. Connectivity is worked out in `netlist.rs`.

use std::collections::HashMap;

use crate::sexpr::Sexp;

/// A point in schematic internal units (1 = 0.1 µm), so that coordinates compare exactly.
pub type Pt = (i64, i64);

pub fn to_iu(mm: f64) -> i64 {
    (mm * 10_000.0).round() as i64
}

#[derive(Debug, Clone, Default)]
pub struct SchematicFile {
    pub uuid: String,
    pub lib_symbols: HashMap<String, LibSymbol>,
    pub symbols: Vec<SymbolInstance>,
    pub wires: Vec<(Pt, Pt)>,
    /// Bus segments. KiCad saves bus-to-bus entries as bus segments too.
    pub buses: Vec<(Pt, Pt)>,
    /// Wire-to-bus entries, from `at` to `at + size`.
    pub bus_entries: Vec<(Pt, Pt)>,
    /// `(bus_alias "NAME" (members ...))` items. KiCad 10 keep aliases in the project file instead.
    pub bus_aliases: Vec<(String, Vec<String>)>,
    pub junctions: Vec<Pt>,
    pub no_connects: Vec<Pt>,
    pub labels: Vec<Label>,
    pub sheets: Vec<Sheet>,
    /// Items found that this reader does not support yet, such as directive labels.
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LibSymbol {
    pub name: String,
    pub power: Option<PowerScope>,
    pub extends: Option<String>,
    pub pins: Vec<LibPin>,
    /// KiCad 10: pins that share a number are joined inside the part.
    pub duplicate_pins_are_jumpers: bool,
    /// KiCad 10: groups of pin numbers joined inside the part, such as a closed solder jumper.
    pub jumper_pin_groups: Vec<Vec<String>>,
}

impl LibSymbol {
    /// Number of units, such as 2 for a dual op-amp. Pins shared by every unit count as unit 0.
    pub fn unit_count(&self) -> u32 {
        self.pins.iter().map(|p| p.unit).max().unwrap_or(0).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerScope {
    Global,
    Local,
}

#[derive(Debug, Clone)]
pub struct LibPin {
    /// 0 means the pin belongs to every unit.
    pub unit: u32,
    /// 0 means the pin belongs to every body style.
    pub body_style: u32,
    pub number: String,
    pub name: String,
    pub kind: String,
    pub hidden: bool,
    /// Connection point in library coordinates, in mm, Y up as written in the file.
    pub at: (f64, f64),
}

#[derive(Debug, Clone, Default)]
pub struct SymbolInstance {
    pub lib_id: String,
    pub lib_name: Option<String>,
    pub at: (f64, f64),
    pub rotation: i32,
    pub mirror: Option<char>,
    pub unit: u32,
    pub body_style: u32,
    pub dnp: bool,
    pub in_bom: bool,
    pub on_board: bool,
    pub fields: Vec<(String, String)>,
    /// `(sheet instance path, reference, unit)` for each place this symbol is used.
    pub instances: Vec<(String, String, u32)>,
}

impl SymbolInstance {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The reference and unit used on the sheet instance at `path`.
    pub fn reference_at(&self, path: &str) -> (String, u32) {
        match self.instances.iter().find(|(p, _, _)| p == path) {
            Some((_, r, u)) => (r.clone(), *u),
            None => (self.field("Reference").unwrap_or("?").to_string(), self.unit),
        }
    }

    /// Converts a library pin position to a schematic point.
    ///
    /// Matches KiCad (`SCH_SYMBOL::SetOrientation`): flip Y, rotate counter-clockwise, mirror on screen, then
    /// move to the symbol's position.
    pub fn place(&self, lib: (f64, f64)) -> Pt {
        let (x, y) = (lib.0, -lib.1);
        let (mut x, mut y) = match self.rotation.rem_euclid(360) {
            90 => (y, -x),
            180 => (-x, -y),
            270 => (-y, x),
            _ => (x, y),
        };
        match self.mirror {
            Some('x') => y = -y,
            Some('y') => x = -x,
            _ => {}
        }
        (to_iu(self.at.0 + x), to_iu(self.at.1 + y))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LabelKind {
    Local,
    Global,
    Hierarchical,
}

#[derive(Debug, Clone)]
pub struct Label {
    pub kind: LabelKind,
    pub name: String,
    pub at: Pt,
}

#[derive(Debug, Clone, Default)]
pub struct Sheet {
    pub uuid: String,
    pub name: String,
    pub file: String,
    pub pins: Vec<(String, Pt)>,
}

fn at_of(s: &Sexp) -> Option<(f64, f64, i32)> {
    let at = s.child("at")?;
    Some((
        at.arg_f64(1)?,
        at.arg_f64(2)?,
        at.arg_f64(3).unwrap_or(0.0).round() as i32,
    ))
}

fn pt_of(s: &Sexp) -> Option<Pt> {
    at_of(s).map(|(x, y, _)| (to_iu(x), to_iu(y)))
}

fn property(s: &Sexp, name: &str) -> Option<String> {
    s.children("property")
        .find(|p| p.arg(1) == Some(name))
        .and_then(|p| p.arg(2))
        .map(str::to_string)
}

pub fn read(root: &Sexp) -> Result<SchematicFile, String> {
    if root.head() != Some("kicad_sch") {
        return Err("not a KiCad schematic (expected `(kicad_sch ...)`)".to_string());
    }
    let mut f = SchematicFile {
        uuid: root
            .child("uuid")
            .and_then(|u| u.arg(1))
            .unwrap_or_default()
            .to_string(),
        ..Default::default()
    };
    if let Some(libs) = root.child("lib_symbols") {
        for s in libs.children("symbol") {
            let lib = read_lib_symbol(s);
            f.lib_symbols.insert(lib.name.clone(), lib);
        }
    }
    for item in root.items() {
        match item.head() {
            Some("symbol") => f.symbols.push(read_symbol(item)),
            Some(kind @ ("wire" | "bus")) => {
                let pts: Vec<Pt> = item
                    .child("pts")
                    .map(|p| {
                        p.children("xy")
                            .filter_map(|xy| Some((to_iu(xy.arg_f64(1)?), to_iu(xy.arg_f64(2)?))))
                            .collect()
                    })
                    .unwrap_or_default();
                let lines = if kind == "wire" { &mut f.wires } else { &mut f.buses };
                for w in pts.windows(2) {
                    lines.push((w[0], w[1]));
                }
            }
            Some("bus_entry") => {
                let size = item.child("size");
                if let (Some((x, y, _)), Some(dx), Some(dy)) = (
                    at_of(item),
                    size.and_then(|s| s.arg_f64(1)),
                    size.and_then(|s| s.arg_f64(2)),
                ) {
                    f.bus_entries
                        .push(((to_iu(x), to_iu(y)), (to_iu(x + dx), to_iu(y + dy))));
                }
            }
            Some("bus_alias") => {
                if let Some(name) = item.arg(1) {
                    let members = item
                        .child("members")
                        .map(|m| {
                            m.items()
                                .iter()
                                .skip(1)
                                .filter_map(Sexp::text)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default();
                    f.bus_aliases.push((name.to_string(), members));
                }
            }
            Some("junction") => f.junctions.extend(pt_of(item)),
            Some("no_connect") => f.no_connects.extend(pt_of(item)),
            Some(kind @ ("label" | "global_label" | "hierarchical_label")) => {
                let kind = match kind {
                    "label" => LabelKind::Local,
                    "global_label" => LabelKind::Global,
                    _ => LabelKind::Hierarchical,
                };
                if let (Some(name), Some(at)) = (item.arg(1), pt_of(item)) {
                    f.labels.push(Label {
                        kind,
                        name: name.to_string(),
                        at,
                    });
                }
            }
            Some("sheet") => f.sheets.push(Sheet {
                uuid: item
                    .child("uuid")
                    .and_then(|u| u.arg(1))
                    .unwrap_or_default()
                    .to_string(),
                name: property(item, "Sheetname")
                    .or_else(|| property(item, "Sheet name"))
                    .unwrap_or_default(),
                file: property(item, "Sheetfile")
                    .or_else(|| property(item, "Sheet file"))
                    .unwrap_or_default(),
                pins: item
                    .children("pin")
                    .filter_map(|p| Some((p.arg(1)?.to_string(), pt_of(p)?)))
                    .collect(),
            }),
            Some(other @ ("directive_label" | "netclass_flag")) => f.unsupported.push(other.to_string()),
            _ => {}
        }
    }
    Ok(f)
}

fn read_lib_symbol(s: &Sexp) -> LibSymbol {
    let name = s.arg(1).unwrap_or_default().to_string();
    let power = s.child("power").map(|p| match p.arg(1) {
        Some("local") => PowerScope::Local,
        _ => PowerScope::Global,
    });
    let mut pins = Vec::new();
    for unit in s.children("symbol") {
        // Unit symbols are named `<name>_<unit>_<body style>`.
        let uname = unit.arg(1).unwrap_or_default();
        let mut parts = uname.rsplitn(3, '_');
        let body_style = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let unit_no = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        for p in unit.children("pin") {
            let Some((x, y, _)) = at_of(p) else { continue };
            let number = p.child("number").and_then(|n| n.arg(1)).unwrap_or_default();
            // A KiCad 10 stacked pin (`[3,9,10,13]`, `[1-4]`) is one pin per number at the same point.
            for number in stacked_numbers(number) {
                pins.push(LibPin {
                    unit: unit_no,
                    body_style,
                    number,
                    name: p.child("name").and_then(|n| n.arg(1)).unwrap_or_default().to_string(),
                    kind: p.arg(1).unwrap_or("unspecified").to_string(),
                    hidden: p.flag("hide").unwrap_or(false),
                    at: (x, y),
                });
            }
        }
    }
    LibSymbol {
        name,
        power,
        extends: s.child("extends").and_then(|e| e.arg(1)).map(str::to_string),
        pins,
        duplicate_pins_are_jumpers: s.flag("duplicate_pin_numbers_are_jumpers").unwrap_or(false),
        jumper_pin_groups: s
            .child("jumper_pin_groups")
            .map(|g| {
                g.items()
                    .iter()
                    .filter(|grp| matches!(grp, Sexp::List(_)))
                    .map(|grp| grp.items().iter().filter_map(Sexp::text).map(str::to_string).collect())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn read_symbol(s: &Sexp) -> SymbolInstance {
    let (x, y, rotation) = at_of(s).unwrap_or((0.0, 0.0, 0));
    let mut instances = Vec::new();
    if let Some(inst) = s.child("instances") {
        for project in inst.children("project") {
            for path in project.children("path") {
                let p = path.arg(1).unwrap_or_default().to_string();
                let r = path
                    .child("reference")
                    .and_then(|r| r.arg(1))
                    .unwrap_or_default()
                    .to_string();
                let u = path.child("unit").and_then(|u| u.arg_f64(1)).map_or(1, |u| u as u32);
                instances.push((p, r, u));
            }
        }
    }
    SymbolInstance {
        lib_id: s.child("lib_id").and_then(|l| l.arg(1)).unwrap_or_default().to_string(),
        lib_name: s.child("lib_name").and_then(|l| l.arg(1)).map(str::to_string),
        at: (x, y),
        rotation,
        mirror: s.child("mirror").and_then(|m| m.arg(1)).and_then(|m| m.chars().next()),
        unit: s.child("unit").and_then(|u| u.arg_f64(1)).map_or(1, |u| u as u32),
        body_style: s
            .child("body_style")
            .or_else(|| s.child("convert"))
            .and_then(|c| c.arg_f64(1))
            .map_or(1, |c| c as u32),
        dnp: s.flag("dnp").unwrap_or(false),
        in_bom: s.flag("in_bom").unwrap_or(true),
        on_board: s.flag("on_board").unwrap_or(true),
        fields: s
            .children("property")
            .filter_map(|p| Some((p.arg(1)?.to_string(), p.arg(2)?.to_string())))
            .collect(),
        instances,
    }
}

/// The pad numbers of a pin: KiCad 10 writes a stacked pin as `[3,9,10,13]` or `[1-4]` (ranges of plain
/// numbers, or of numbers with the same letter prefix, `[A1-A3]`). Anything else is one number, as written.
pub fn stacked_numbers(number: &str) -> Vec<String> {
    let Some(inner) = number.strip_prefix('[').and_then(|n| n.strip_suffix(']')) else {
        return vec![number.to_string()];
    };
    let mut out = Vec::new();
    for part in inner.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let range = part.split_once('-').and_then(|(a, b)| {
            let split = |t: &str| {
                let digits = t.trim_start_matches(|c: char| !c.is_ascii_digit());
                let prefix = &t[..t.len() - digits.len()];
                digits.parse::<u32>().ok().map(|n| (prefix.to_string(), n))
            };
            let ((pa, a), (pb, b)) = (split(a.trim())?, split(b.trim())?);
            (pa == pb && a <= b && b - a < 1000).then(|| (a..=b).map(|n| format!("{pa}{n}")).collect::<Vec<_>>())
        });
        match range {
            Some(numbers) => out.extend(numbers),
            None => out.push(part.to_string()),
        }
    }
    if out.is_empty() { vec![number.to_string()] } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(rotation: i32, mirror: Option<char>) -> SymbolInstance {
        SymbolInstance {
            at: (100.0, 50.0),
            rotation,
            mirror,
            ..Default::default()
        }
    }

    #[test]
    fn places_pins_like_kicad() {
        // Device:R pin 2 is at (0, -3.81) in the library, which is below the body on screen.
        assert_eq!(sym(0, None).place((0.0, -3.81)), (to_iu(100.0), to_iu(53.81)));
        // Rotated 90° counter-clockwise, "below" becomes "right".
        assert_eq!(sym(90, None).place((0.0, -3.81)), (to_iu(103.81), to_iu(50.0)));
        assert_eq!(sym(180, None).place((0.0, -3.81)), (to_iu(100.0), to_iu(46.19)));
        assert_eq!(sym(270, None).place((0.0, -3.81)), (to_iu(96.19), to_iu(50.0)));
        // Mirror Y swaps left and right; mirror X swaps up and down.
        assert_eq!(sym(0, Some('y')).place((5.08, 0.0)), (to_iu(94.92), to_iu(50.0)));
        assert_eq!(sym(0, Some('x')).place((0.0, -3.81)), (to_iu(100.0), to_iu(46.19)));
        // KiCad rotates first, then mirrors on screen. With (at 100 50 90) (mirror y) KiCad's transform is
        // (0, -1, -1, 0), which puts a library pin at (5.08, 0) at (100, 44.92).
        assert_eq!(sym(90, Some('y')).place((5.08, 0.0)), (to_iu(100.0), to_iu(44.92)));
        // (at 100 50 90) (mirror x) gives the transform (0, 1, 1, 0).
        assert_eq!(sym(90, Some('x')).place((0.0, -3.81)), (to_iu(103.81), to_iu(50.0)));
        assert_eq!(sym(270, Some('y')).place((5.08, 0.0)), (to_iu(100.0), to_iu(55.08)));
    }

    #[test]
    fn expands_stacked_pin_numbers() {
        assert_eq!(stacked_numbers("3"), ["3"]);
        assert_eq!(stacked_numbers("[3,9,10,13]"), ["3", "9", "10", "13"]);
        assert_eq!(stacked_numbers("[15, 16]"), ["15", "16"]);
        assert_eq!(stacked_numbers("[1-4]"), ["1", "2", "3", "4"]);
        assert_eq!(stacked_numbers("[A1-A3,B2]"), ["A1", "A2", "A3", "B2"]);
        // Mismatched prefixes and backwards ranges are kept as written.
        assert_eq!(stacked_numbers("[A1-B3]"), ["A1-B3"]);
        assert_eq!(stacked_numbers("[4-1]"), ["4-1"]);
        assert_eq!(stacked_numbers("[]"), ["[]"]);
    }
}
