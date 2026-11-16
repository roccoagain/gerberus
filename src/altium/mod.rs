//! Altium Designer reader. Turns a `.PrjPcb` project, or a lone `.SchDoc`, into a [`crate::design::Design`].

pub mod netlist;
pub mod pcb;
pub mod project;
pub mod records;
pub mod schematic;

pub use netlist::load;
