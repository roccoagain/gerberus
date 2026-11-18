use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Design {
    pub components: BTreeMap<String, Component>,
    pub nets: Vec<Net>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Component {
    pub reference: String,
    pub value: String,
    pub lib_id: String,
    pub footprint: String,
    pub fields: BTreeMap<String, String>,
    pub dnp: bool,
    pub in_bom: bool,
    pub on_board: bool,
    pub pins: BTreeMap<String, Pin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub number: String,
    pub name: String,
    pub kind: PinKind,
    pub no_connect: bool,
    pub net: usize,
    pub stack: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PinKind {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    Free,
    Unspecified,
    PowerIn,
    PowerOut,
    OpenCollector,
    OpenEmitter,
    NoConnect,
}

impl PinKind {
    pub fn from_kicad(s: &str) -> PinKind {
        match s {
            "input" => PinKind::Input,
            "output" => PinKind::Output,
            "bidirectional" => PinKind::Bidirectional,
            "tri_state" => PinKind::TriState,
            "passive" => PinKind::Passive,
            "free" => PinKind::Free,
            "power_in" => PinKind::PowerIn,
            "power_out" => PinKind::PowerOut,
            "open_collector" => PinKind::OpenCollector,
            "open_emitter" => PinKind::OpenEmitter,
            "no_connect" => PinKind::NoConnect,
            _ => PinKind::Unspecified,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Net {
    pub name: String,
    pub pins: Vec<(String, String)>,
    pub labels: Vec<String>,
}

impl Design {
    pub fn others_on(&self, reference: &str, net: usize) -> impl Iterator<Item = (&str, &Component, &str)> {
        self.nets[net].pins.iter().filter_map(move |(r, p)| {
            let c = self.components.get(r)?;
            (r != reference && !c.dnp).then_some((r.as_str(), c, p.as_str()))
        })
    }

    pub fn only_capacitors_on(&self, reference: &str, net: usize) -> bool {
        let mut others = self.others_on(reference, net).peekable();
        others.peek().is_some() && others.all(|(r, c, _)| is_capacitor(r, c))
    }

    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }

    pub fn pin_net(&self, reference: &str, pin: &str) -> Option<&Net> {
        let p = self.components.get(reference)?.pins.get(pin)?;
        self.nets.get(p.net)
    }
}

pub fn is_capacitor(reference: &str, c: &Component) -> bool {
    c.lib_id.starts_with("Device:C")
        || reference
            .strip_prefix('C')
            .is_some_and(|n| n.starts_with(|ch: char| ch.is_ascii_digit()))
}
