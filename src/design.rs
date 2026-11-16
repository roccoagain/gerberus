//! The design graph every checker reads: parts, their pins, and the nets that join them.
//!
//! It holds no KiCad-specific types, so another reader (Altium later) can fill it in the same way.

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
    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }

    /// The net a part pin is on.
    pub fn pin_net(&self, reference: &str, pin: &str) -> Option<&Net> {
        let p = self.components.get(reference)?.pins.get(pin)?;
        self.nets.get(p.net)
    }
}
