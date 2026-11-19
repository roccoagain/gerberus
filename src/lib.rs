#![warn(clippy::unwrap_used)]

pub const EXIT_CHECK_FAILED: u8 = 1;
pub const EXIT_CANNOT_RUN: u8 = 2;

pub mod bom;
pub mod bringup;
pub mod datasheet;
pub mod design;
pub mod diag;
pub mod diff;
pub mod doctor;
pub mod family;
pub mod init;
pub mod inputs;
pub mod kicad;
pub mod limits;
pub mod part_new;
pub mod pinout;
pub mod policy;
pub mod protection;
pub mod rules;
pub mod sexpr;
pub mod si;
pub mod signal;
pub mod skill;
pub mod spec;
pub mod spice;
pub mod thermal;
pub mod traces;
pub mod voltage;
pub mod waivers;

pub use kicad::load;
