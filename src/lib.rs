//! Gerberus checks a PCB design for electrical bugs before it is ordered.
// A malformed board file must give an error, not a crash: no `unwrap()` outside tests. `expect("why")` stays
// for invariants the code itself guarantees. Tests may unwrap (clippy.toml). Set here, not in Cargo.toml, so
// it skips the integration tests and examples.
#![warn(clippy::unwrap_used)]

pub const EXIT_CHECK_FAILED: u8 = 1;
pub const EXIT_CANNOT_RUN: u8 = 2;

pub mod bom;
pub mod bringup;
pub mod datasheet;
pub mod decoupling;
pub mod design;
pub mod diag;
pub mod diff;
pub mod doctor;
pub mod drop;
pub mod edges;
pub mod finding;
pub mod html;
pub mod init;
pub mod inputs;
pub mod kicad;
pub mod limits;
pub mod loops;
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
pub mod splits;
pub mod thermal;
pub mod traces;
pub mod tscircuit;
pub mod units;
pub mod voltage;
pub mod waivers;
