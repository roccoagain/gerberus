//! The design graph every checker reads: parts, their pins, and the nets that join them.
//!
//! It holds no KiCad-specific types, so another reader can fill it in the same way.
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Design {
    /// Parts keyed by reference designator, such as `U1`. Power symbols and flags are not parts.
    pub components: BTreeMap<String, Component>,
    pub nets: Vec<Net>,
    /// Things the reader skipped or could not resolve. Checkers should treat affected nets as unknown.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Component {
    pub reference: String,
    pub value: String,
    /// Library symbol, such as `Device:R`.
    pub lib_id: String,
    pub footprint: String,
    /// Every field on the symbol, including `Value`, `Footprint`, `Datasheet` and custom ones like `LCSC`.
    pub fields: BTreeMap<String, String>,
    pub dnp: bool,
    pub in_bom: bool,
    pub on_board: bool,
    /// Pins keyed by pin number.
    pub pins: BTreeMap<String, Pin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub number: String,
    pub name: String,
    pub kind: PinKind,
    /// The pin carries a no-connect marker.
    pub no_connect: bool,
    /// Index into [`Design::nets`].
    pub net: usize,
    /// For one pad of a KiCad 10 stacked pin (`[3,9,10]`), the stack's first number: the pads are one pin.
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
    /// The name KiCad gives the net, such as `GND`, `/SDA` or `Net-(U1-PA4)`.
    pub name: String,
    /// `(reference, pin number)` for every part pin on the net, sorted.
    pub pins: Vec<(String, String)>,
    /// Every label and power symbol name on the net, so checkers can explain where a name came from.
    pub labels: Vec<String>,
}

impl Design {
    /// The fitted parts other than `reference` on `net`: what `reference`'s pin there connects to.
    pub fn others_on(&self, reference: &str, net: usize) -> impl Iterator<Item = (&str, &Component, &str)> {
        self.nets[net].pins.iter().filter_map(move |(r, p)| {
            let c = self.components.get(r)?;
            (r != reference && !c.dnp).then_some((r.as_str(), c, p.as_str()))
        })
    }

    /// Whether `reference`'s pin on `net` is reached only through capacitors: there is at least one fitted
    /// part besides `reference` on it, and every one is a capacitor.
    pub fn only_capacitors_on(&self, reference: &str, net: usize) -> bool {
        let mut others = self.others_on(reference, net).peekable();
        others.peek().is_some() && others.all(|(r, c, _)| is_capacitor(r, c))
    }

    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }

    /// The net a part pin is on.
    pub fn pin_net(&self, reference: &str, pin: &str) -> Option<&Net> {
        let p = self.components.get(reference)?.pins.get(pin)?;
        self.nets.get(p.net)
    }
}

/// A capacitor, by its symbol (`Device:C…`) or its reference (`C12`).
pub fn is_capacitor(reference: &str, c: &Component) -> bool {
    c.lib_id.starts_with("Device:C")
        || reference
            .strip_prefix('C')
            .is_some_and(|n| n.starts_with(|ch: char| ch.is_ascii_digit()))
}
