//! KiCad reader. Turns a `.kicad_sch` file tree into a [`crate::design::Design`].
pub mod bus;
pub mod netlist;
pub mod pcb;
pub mod schematic;

pub use netlist::load;
