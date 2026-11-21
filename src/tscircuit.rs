//! tscircuit reader. Turns Circuit JSON (the `circuit.json` that `tsci build` writes) into a
//! [`crate::design::Design`], and runs the tscircuit CLI to get it from a `.tsx` board.
//!
//! Circuit JSON is a flat list of elements. The `source_*` elements are the schematic: each `source_component` is
//! a part, each `source_port` one of its pins, and each `source_trace` joins ports and named `source_net`s. Each
//! part is given the KiCad standard symbol of its kind (`Device:R` for a resistor), so every check reads a
//! tscircuit board as it reads a KiCad one.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::design::{Component, Design, Net, Pin, PinKind};

/// Reads a Circuit JSON file.
pub fn load(path: &Path) -> Result<Design, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs `tsci export` on a `.tsx` board and reads the Circuit JSON it writes.
pub fn build(path: &Path) -> Result<Design, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let out = std::env::temp_dir().join(format!("gerberus-tsci-{}-{nanos}.json", std::process::id()));
    // npm installs the CLI as a `.cmd` script on Windows, which Command only finds by its full name.
    let tsci = if cfg!(windows) { "tsci.cmd" } else { "tsci" };
    // Run in the board's folder, as its designer runs tsci, so the cache tsci keeps (`.tscircuit/`) lands there.
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let file = path.file_name().ok_or_else(|| format!("{}: not a file", path.display()))?;
    let run = Command::new(tsci)
        .current_dir(dir)
        .arg("export")
        .arg(file)
        .args(["--format", "circuit-json", "--output"])
        .arg(&out)
        .output()
        .map_err(|e| {
            format!(
                "can't run tsci ({e}); reading a .tsx board needs the tscircuit CLI on the PATH \
                 (`npm install -g tscircuit`), or give gerberus the circuit.json that `tsci build` writes"
            )
        })?;
    let text = std::fs::read_to_string(&out);
    let _ = std::fs::remove_file(&out);
    if !run.status.success() {
        return Err(format!(
            "tsci export {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&run.stderr).trim()
        ));
    }
    let text = text.map_err(|e| format!("tsci export {} wrote no Circuit JSON: {e}", path.display()))?;
    read(&text).map_err(|e| format!("{} (as tsci exported it): {e}", path.display()))
}

/// Reads Circuit JSON text.
pub fn read(text: &str) -> Result<Design, String> {
    let json: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let elements = json
        .as_array()
        .ok_or("not Circuit JSON: it should be a list of elements")?;
    let of_type = |t: &'static str| elements.iter().filter(move |e| str_of(e, "type") == Some(t));
    let mut warnings = Vec::new();

    let groups = Groups::new(elements);
    let footprints: HashMap<&str, &str> = of_type("cad_component")
        .filter_map(|e| Some((str_of(e, "source_component_id")?, str_of(e, "footprinter_string")?)))
        .collect();
    // A part is fitted unless its board part says not to place it; a part with no board part isn't on the board.
    let placed: HashMap<&str, bool> = of_type("pcb_component")
        .filter_map(|e| {
            let dnp = e.get("do_not_place").and_then(Value::as_bool).unwrap_or(false);
            Some((str_of(e, "source_component_id")?, dnp))
        })
        .collect();

    let mut ports_of: HashMap<&str, Vec<&Value>> = HashMap::new();
    for p in of_type("source_port") {
        if let Some(c) = str_of(p, "source_component_id") {
            ports_of.entry(c).or_default().push(p);
        }
    }

    // Parts without ports, such as printed case parts and screws, are mechanical and not in the design.
    let parts: Vec<&Value> = of_type("source_component")
        .filter(|c| str_of(c, "source_component_id").is_some_and(|id| ports_of.contains_key(id)))
        .collect();
    let references = references(&parts, &groups, &mut warnings)?;

    let mut components = BTreeMap::new();
    // Each port: its part's reference and pin number, and the pin as the design holds it (net filled in later).
    let mut ports: HashMap<&str, (String, Pin)> = HashMap::new();
    for c in &parts {
        let id = str_of(c, "source_component_id").unwrap_or_default();
        let reference = references[id].clone();
        let ftype = str_of(c, "ftype").unwrap_or_default();
        let lib_id = symbol(c, ftype);
        let mut pins = BTreeMap::new();
        for p in &ports_of[id] {
            let Some(port_id) = str_of(p, "source_port_id") else {
                continue;
            };
            let pin = pin(p, ftype);
            if pins.insert(pin.number.clone(), pin.clone()).is_some() {
                warnings.push(format!(
                    "{reference} has two pins numbered {}; using the last",
                    pin.number
                ));
            }
            ports.insert(port_id, (reference.clone(), pin));
        }
        let (on_board, dnp) = match placed.get(id) {
            Some(&dnp) => (true, dnp),
            None => (false, false),
        };
        components.insert(
            reference.clone(),
            Component {
                reference,
                value: value(c, ftype, &lib_id),
                lib_id,
                footprint: footprints.get(id).copied().unwrap_or_default().to_string(),
                fields: fields(c),
                dnp,
                in_bom: on_board,
                on_board,
                pins,
                placed: Vec::new(),
            },
        );
    }

    // Join every port and named net that a trace or a part's internal connection ties together.
    let names: HashMap<&str, String> = of_type("source_net")
        .filter_map(|n| {
            let name = str_of(n, "name")?;
            let path = groups.subcircuit_path(str_of(n, "subcircuit_id"));
            let full = if path.is_empty() {
                name.to_string()
            } else {
                format!("/{}/{name}", path.join("/"))
            };
            Some((str_of(n, "source_net_id")?, full))
        })
        .collect();
    // Every port joins, not only the parts' own: a trace may run through a port that is no part's pin.
    let mut joined = UnionFind::default();
    for p in of_type("source_port") {
        joined.add(str_of(p, "source_port_id").unwrap_or_default());
    }
    for &id in names.keys() {
        joined.add(id);
    }
    // Ids that name no port or net, with how many connections name each.
    let mut missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut join_all = |ids: Vec<&str>| {
        let known: Vec<&str> = ids
            .into_iter()
            .filter(|&id| {
                let ok = joined.has(id);
                if !ok {
                    *missing.entry(id.to_string()).or_default() += 1;
                }
                ok
            })
            .collect();
        for pair in known.windows(2) {
            joined.union(pair[0], pair[1]);
        }
    };
    for t in of_type("source_trace") {
        join_all(
            strings(t, "connected_source_port_ids")
                .chain(strings(t, "connected_source_net_ids"))
                .collect(),
        );
    }
    // Pins a part joins inside itself, such as a push button's two pairs, carry one net.
    for c in &parts {
        if let Some(sets) = c.get("internally_connected_source_port_ids").and_then(Value::as_array) {
            for set in sets {
                join_all(set.as_array().into_iter().flatten().filter_map(Value::as_str).collect());
            }
        }
    }
    for e in of_type("source_component_internal_connection") {
        join_all(strings(e, "source_port_ids").collect());
    }
    for (id, n) in missing {
        warnings.push(format!(
            "{n} connections name {id}, which is no port or net of the design, so they join nothing through it"
        ));
    }

    let mut members: BTreeMap<usize, (Vec<&str>, BTreeSet<String>)> = BTreeMap::new();
    for &port in ports.keys() {
        members.entry(joined.find(port)).or_default().0.push(port);
    }
    for (&net, name) in &names {
        if let Some(m) = members.get_mut(&joined.find(net)) {
            m.1.insert(name.clone());
        }
    }
    let mut nets = Vec::new();
    for (port_ids, labels) in members.into_values() {
        let index = nets.len();
        let mut pins: Vec<(String, String)> = Vec::new();
        let mut named: Vec<(&str, &Pin)> = Vec::new();
        for port in port_ids {
            let (reference, pin) = &ports[port];
            pins.push((reference.clone(), pin.number.clone()));
            named.push((reference, pin));
            if let Some(p) = components.get_mut(reference).and_then(|c| c.pins.get_mut(&pin.number)) {
                p.net = index;
            }
        }
        pins.sort();
        named.sort_by(|a, b| (a.0, &a.1.number).cmp(&(b.0, &b.1.number)));
        // A net from an outer subcircuit names it before one from inside, as fewer `/` show.
        let name = labels
            .iter()
            .min_by_key(|l| (l.matches('/').count(), l.as_str()))
            .cloned()
            .unwrap_or_else(|| pin_net_name(&named));
        nets.push(Net {
            name,
            pins,
            labels: labels.into_iter().collect(),
        });
    }
    Ok(Design {
        components,
        nets,
        warnings,
    })
}

fn str_of<'a>(e: &'a Value, key: &str) -> Option<&'a str> {
    e.get(key).and_then(Value::as_str)
}

fn strings<'a>(e: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    e.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// The groups, so each part and net can be placed in its subcircuit.
struct Groups<'a> {
    /// Group id to (name, parent group id).
    by_id: HashMap<&'a str, (Option<&'a str>, Option<&'a str>)>,
    /// Subcircuit id to the id of the group that is that subcircuit.
    subcircuits: HashMap<&'a str, &'a str>,
}

impl<'a> Groups<'a> {
    fn new(elements: &'a [Value]) -> Groups<'a> {
        let mut by_id = HashMap::new();
        let mut subcircuits = HashMap::new();
        for g in elements.iter().filter(|e| str_of(e, "type") == Some("source_group")) {
            let Some(id) = str_of(g, "source_group_id") else {
                continue;
            };
            by_id.insert(id, (str_of(g, "name"), str_of(g, "parent_source_group_id")));
            if let Some(s) = str_of(g, "subcircuit_id") {
                subcircuits.insert(s, id);
            }
        }
        Groups { by_id, subcircuits }
    }

    /// The names of the groups from the board down to `group`, leaving out the board's own group.
    fn path(&self, group: Option<&str>) -> Vec<&'a str> {
        let mut names = Vec::new();
        let mut at = group.and_then(|g| self.by_id.get_key_value(g));
        while let Some((_, &(name, parent))) = at {
            // The outermost group is the board.
            let Some(parent) = parent else { break };
            names.extend(name);
            at = self.by_id.get_key_value(parent);
        }
        names.reverse();
        names
    }

    fn subcircuit_path(&self, subcircuit: Option<&str>) -> Vec<&'a str> {
        self.path(subcircuit.and_then(|s| self.subcircuits.get(s).copied()))
    }
}

/// Each part's reference: its name, or, where parts in different groups share a name (a group used twice), the
/// name followed by its group path, such as `R1_A` for the R1 in group A.
fn references(
    parts: &[&Value],
    groups: &Groups<'_>,
    warnings: &mut Vec<String>,
) -> Result<HashMap<String, String>, String> {
    let mut count: HashMap<&str, usize> = HashMap::new();
    for c in parts {
        *count.entry(str_of(c, "name").unwrap_or_default()).or_default() += 1;
    }
    let mut out = HashMap::new();
    let mut taken: BTreeMap<String, &str> = BTreeMap::new();
    let mut renamed: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for c in parts {
        let id = str_of(c, "source_component_id").unwrap_or_default();
        let name = str_of(c, "name").unwrap_or_default();
        if name.is_empty() {
            return Err(format!("part {id} has no name"));
        }
        let path = groups.path(str_of(c, "source_group_id"));
        let reference = if count[name] > 1 && !path.is_empty() {
            let r = format!("{name}_{}", path.join("_"));
            renamed.entry(name).or_default().push(r.clone());
            r
        } else {
            name.to_string()
        };
        if let Some(other) = taken.insert(reference.clone(), id) {
            return Err(format!("parts {other} and {id} are both called {reference}"));
        }
        out.insert(id.to_string(), reference);
    }
    for (name, as_) in renamed {
        warnings.push(format!(
            "{} parts are named {name}; they are called {} here",
            count[name],
            as_.join(", ")
        ));
    }
    Ok(out)
}

/// The pin a port is. A port tscircuit names only by number (`pin1`) has no name, as on KiCad's passive
/// symbols, except on diodes and transistors, which take their KiCad pin name (`A`, `K`, `G`, `D`, `S`, `B`, `C`,
/// `E`) from the port's role so pin checks know which side is which.
fn pin(p: &Value, ftype: &str) -> Pin {
    let name = str_of(p, "name").unwrap_or_default();
    let number = p
        .get("pin_number")
        .and_then(Value::as_u64)
        .map_or_else(|| name.to_string(), |n| n.to_string());
    let only_number = name == format!("pin{number}");
    let role = || {
        strings(p, "port_hints").find_map(|h| match (ftype, h) {
            ("simple_diode" | "simple_led", "anode") => Some("A"),
            ("simple_diode" | "simple_led", "cathode") => Some("K"),
            ("simple_mosfet", "gate") => Some("G"),
            ("simple_mosfet", "drain") => Some("D"),
            ("simple_mosfet", "source") => Some("S"),
            ("simple_transistor", "base") => Some("B"),
            ("simple_transistor", "collector") => Some("C"),
            ("simple_transistor", "emitter") => Some("E"),
            _ => None,
        })
    };
    let name = match (only_number, role()) {
        (false, _) => name.to_string(),
        (true, Some(r)) => r.to_string(),
        (true, None) => String::new(),
    };
    Pin {
        number,
        name,
        kind: kind(p, ftype),
        no_connect: flag(p, "do_not_connect"),
        net: 0,
        stack: None,
    }
}

fn flag(e: &Value, key: &str) -> bool {
    e.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The pin's electrical type, from its pin attributes. Without any, the pins of discrete parts are passive, as on
/// KiCad's symbols, and an IC's pins are unspecified.
fn kind(p: &Value, ftype: &str) -> PinKind {
    let attributes = [
        ("requires_power", PinKind::PowerIn),
        ("requires_ground", PinKind::PowerIn),
        ("provides_power", PinKind::PowerOut),
        ("provides_ground", PinKind::PowerOut),
        ("is_using_open_drain", PinKind::OpenCollector),
        ("is_using_open_collector", PinKind::OpenCollector),
        ("is_using_open_emitter", PinKind::OpenEmitter),
        ("is_using_tri_state", PinKind::TriState),
        ("is_bidirectional", PinKind::Bidirectional),
        ("is_input", PinKind::Input),
        ("is_output", PinKind::Output),
        ("is_passive", PinKind::Passive),
    ];
    if let Some(&(_, k)) = attributes.iter().find(|(a, _)| flag(p, a)) {
        return k;
    }
    if symbol_of(ftype, true).is_some() {
        PinKind::Passive
    } else {
        PinKind::Unspecified
    }
}

/// The KiCad standard symbol for a discrete part's kind, or `None` for an IC and anything else tscircuit has no
/// discrete kind for. `polarised` tells a polarised capacitor from a plain one.
fn symbol_of(ftype: &str, polarised: bool) -> Option<&'static str> {
    Some(match ftype {
        "simple_resistor" => "Device:R",
        "simple_capacitor" if polarised => "Device:CP",
        "simple_capacitor" => "Device:C",
        "simple_inductor" => "Device:L",
        "simple_diode" => "Device:D",
        "simple_led" => "Device:LED",
        "simple_fuse" => "Device:Fuse",
        "simple_crystal" => "Device:Crystal",
        "simple_resonator" => "Device:Resonator",
        "simple_potentiometer" => "Device:R_Potentiometer",
        "simple_battery" => "Device:Battery",
        "simple_push_button" => "Switch:SW_Push",
        "simple_switch" => "Switch:SW_SPST",
        "simple_connector" | "simple_pin_header" => "Connector:Conn",
        "simple_test_point" => "Connector:TestPoint",
        _ => return None,
    })
}

/// The part's symbol: the KiCad standard symbol of its kind, or its tscircuit kind (`tscircuit:simple_chip`).
fn symbol(c: &Value, ftype: &str) -> String {
    match ftype {
        "simple_transistor" if str_of(c, "transistor_type") == Some("pnp") => "Device:Q_PNP".into(),
        "simple_transistor" => "Device:Q_NPN".into(),
        "simple_mosfet" if str_of(c, "channel_type") == Some("p") => "Device:Q_PMOS".into(),
        "simple_mosfet" => "Device:Q_NMOS".into(),
        // tscircuit marks a polarised capacitor only by its pins not being interchangeable.
        _ => symbol_of(
            ftype,
            c.get("are_pins_interchangeable").and_then(Value::as_bool) == Some(false),
        )
        .map_or_else(|| format!("tscircuit:{ftype}"), str::to_string),
    }
}

/// The value as a schematic shows it: `10kΩ`, `100nF` or `3.3µH` for a passive, or the part number. A part
/// with neither is named by its symbol, as KiCad's are (`LED`, `Q_NMOS`).
fn value(c: &Value, ftype: &str, lib_id: &str) -> String {
    let quantity = match ftype {
        "simple_resistor" => Some("resistance"),
        "simple_capacitor" => Some("capacitance"),
        "simple_inductor" => Some("inductance"),
        "simple_crystal" | "simple_resonator" => Some("frequency"),
        _ => None,
    };
    let text = |key: &str| match c.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    quantity
        .and_then(|q| text(&format!("display_{q}")).or_else(|| text(q)))
        .or_else(|| text("display_value"))
        .or_else(|| text("manufacturer_part_number"))
        .unwrap_or_else(|| lib_id.rsplit(':').next().unwrap_or_default().to_string())
}

/// The fields the checks read: the part number (`MPN`), the LCSC part number (`LCSC`, which JLCPCB numbers also
/// are) and a capacitor's rated voltage (`Voltage`).
fn fields(c: &Value) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    if let Some(mpn) = str_of(c, "manufacturer_part_number") {
        fields.insert("MPN".into(), mpn.into());
    }
    let suppliers = c.get("supplier_part_numbers");
    let lcsc = ["lcsc", "jlcpcb"].iter().find_map(|s| {
        suppliers
            .and_then(|p| p.get(s))
            .and_then(|n| n.get(0))
            .and_then(Value::as_str)
    });
    if let Some(lcsc) = lcsc {
        fields.insert("LCSC".into(), lcsc.into());
    }
    if let Some(v) = c.get("max_voltage_rating").and_then(Value::as_f64) {
        fields.insert("Voltage".into(), format!("{v}V"));
    }
    fields
}

/// A name for a net no `source_net` names, as KiCad makes one: `Net-(U1-SDA)` from a pin name, `Net-(R1-Pad2)`
/// from a pin number, and `unconnected-(U1-SCL-Pad3)` for a pin on its own.
fn pin_net_name(pins: &[(&str, &Pin)]) -> String {
    if let [(reference, p)] = pins {
        return if p.name.is_empty() {
            format!("unconnected-({reference}-Pad{})", p.number)
        } else {
            format!("unconnected-({reference}-{}-Pad{})", p.name, p.number)
        };
    }
    match pins.iter().find(|(_, p)| !p.name.is_empty()) {
        Some((reference, p)) => format!("Net-({reference}-{})", p.name),
        None => pins
            .first()
            .map(|(reference, p)| format!("Net-({reference}-Pad{})", p.number))
            .unwrap_or_default(),
    }
}

/// Sets of ids, joined two at a time.
#[derive(Default)]
struct UnionFind<'a> {
    index: HashMap<&'a str, usize>,
    parent: Vec<usize>,
}

impl<'a> UnionFind<'a> {
    fn has(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    fn add(&mut self, id: &'a str) {
        let next = self.parent.len();
        if *self.index.entry(id).or_insert(next) == next {
            self.parent.push(next);
        }
    }

    /// A number that stands for `id`'s set.
    fn find(&mut self, id: &str) -> usize {
        let mut root = self.index[id];
        while self.parent[root] != root {
            self.parent[root] = self.parent[self.parent[root]];
            root = self.parent[root];
        }
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        let (ra, rb) = (self.find(a), self.find(b));
        self.parent[ra] = rb;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design(elements: &str) -> Design {
        read(elements).unwrap()
    }

    #[test]
    fn joins_ports_through_traces_and_named_nets() {
        let d = design(
            r#"[
            {"type":"source_component","source_component_id":"c0","ftype":"simple_resistor","name":"R1","resistance":10000,"display_resistance":"10kΩ"},
            {"type":"source_component","source_component_id":"c1","ftype":"simple_chip","name":"U1","manufacturer_part_number":"X1"},
            {"type":"source_port","source_port_id":"p0","source_component_id":"c0","name":"pin1","pin_number":1},
            {"type":"source_port","source_port_id":"p1","source_component_id":"c0","name":"pin2","pin_number":2},
            {"type":"source_port","source_port_id":"p2","source_component_id":"c1","name":"SDA","pin_number":2,"is_bidirectional":true},
            {"type":"source_port","source_port_id":"p3","source_component_id":"c1","name":"VCC","pin_number":1,"requires_power":true},
            {"type":"source_net","source_net_id":"n0","name":"V3_3"},
            {"type":"source_trace","source_trace_id":"t0","connected_source_port_ids":["p0"],"connected_source_net_ids":["n0"]},
            {"type":"source_trace","source_trace_id":"t1","connected_source_port_ids":["p3"],"connected_source_net_ids":["n0"]},
            {"type":"source_trace","source_trace_id":"t2","connected_source_port_ids":["p1","p2"],"connected_source_net_ids":[]}
        ]"#,
        );
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);
        assert_eq!(d.pin_net("R1", "1").unwrap().name, "V3_3");
        assert_eq!(d.pin_net("U1", "1").unwrap().name, "V3_3");
        assert_eq!(d.pin_net("R1", "2").unwrap().name, "Net-(U1-SDA)");
        assert_eq!(d.pin_net("U1", "2"), d.pin_net("R1", "2"));
        let r1 = &d.components["R1"];
        assert_eq!((r1.lib_id.as_str(), r1.value.as_str()), ("Device:R", "10kΩ"));
        assert_eq!(r1.pins["1"].kind, PinKind::Passive);
        let u1 = &d.components["U1"];
        assert_eq!(u1.lib_id, "tscircuit:simple_chip");
        assert_eq!(u1.fields["MPN"], "X1");
        assert_eq!(u1.pins["1"].kind, PinKind::PowerIn);
        assert_eq!(u1.pins["2"].kind, PinKind::Bidirectional);
        // No board part: not on the board.
        assert!(!u1.on_board);
    }

    #[test]
    fn a_lone_pin_is_unconnected() {
        let d = design(
            r#"[
            {"type":"source_component","source_component_id":"c0","ftype":"simple_chip","name":"U1"},
            {"type":"source_port","source_port_id":"p0","source_component_id":"c0","name":"SCL","pin_number":3},
            {"type":"source_port","source_port_id":"p1","source_component_id":"c0","name":"pin4","pin_number":4,"do_not_connect":true}
        ]"#,
        );
        assert_eq!(d.pin_net("U1", "3").unwrap().name, "unconnected-(U1-SCL-Pad3)");
        assert_eq!(d.pin_net("U1", "4").unwrap().name, "unconnected-(U1-Pad4)");
        assert!(d.components["U1"].pins["4"].no_connect);
    }
}
