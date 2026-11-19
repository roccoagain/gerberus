//! Line length against edge speed: is every SPI and I²S line short enough not to ring?
//!
//! A track behaves as a lumped wire while a signal's edge is longer than about six times the track's delay; beyond
//! that, a line driven straight from a push-pull output reflects off its far end and rings, which a series resistor
//! at the driver damps. SPI and I²S lines are found by their net names, as `si` finds USB pairs: a name with a word
//! such as SCK, SCLK, MOSI, MISO, COPI, CIPO, SDI, SDO, BCLK, BCK, LRCLK, LRCK, WS, MCLK, SDIN or SDOUT, or one
//! starting SAI or I2S. An edge is taken to be as fast as `policy.toml`'s `[edges] rise` (part files don't give output rise times),
//! and a signal to travel at `c / √εr` through the stackup's highest-εr dielectric, its slowest.
//!
//! - PASS: all of the net's tracks together are no longer than a sixth of the edge's length: no path is longer.
//! - UNKNOWN: longer, so it may ring. Whether it does depends on the driver's strength and the far end's load,
//!   which aren't known: put a series resistor beside the driver, or check it on a scope.
use crate::design::Design;
use crate::finding::{Finding, Severity, Verdict};
use crate::kicad::pcb::{Layout, StackLayer, Track};
use crate::policy::EdgePolicy;

/// Light's speed in vacuum, mm per second.
const C: f64 = 2.998e11;

/// An SPI or I²S line, by a word of its net's name.
pub fn is_bus_line(name: &str) -> bool {
    const WORDS: [&str; 16] = [
        "SCK", "SCLK", "MOSI", "MISO", "COPI", "CIPO", "SDI", "SDO", "BCLK", "BCK", "LRCLK", "LRCK", "WS", "MCLK",
        "SDIN", "SDOUT",
    ];
    name.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| WORDS.contains(&w) || w.starts_with("SAI") || w.starts_with("I2S"))
}

/// The `line-length` findings: one per SPI or I²S net with tracks.
pub fn check(design: &Design, layout: &Layout, policy: &EdgePolicy) -> Vec<Finding> {
    let er = layout
        .stackup
        .iter()
        .filter_map(|l| match l {
            StackLayer::Dielectric { er, .. } => Some(*er),
            _ => None,
        })
        .fold(1.0_f64, f64::max);
    let speed = C / er.sqrt();
    let critical = policy.rise * speed / 6.0;
    let mut out = vec![];
    for (n, net) in design.nets.iter().enumerate() {
        if !is_bus_line(&net.name) {
            continue;
        }
        let length: f64 = layout
            .tracks
            .iter()
            .filter(|t| t.net == net.name)
            .map(Track::length)
            .sum();
        if length == 0.0 {
            continue;
        }
        let edge = format!(
            "a {} ns edge ({}) travelling at {:.0} mm/ns (εr {er}) is lumped up to {critical:.1} mm",
            policy.rise * 1e9,
            policy.reason,
            speed * 1e-9
        );
        let (verdict, message) = if length <= critical {
            (
                Verdict::Pass,
                format!("{}: {length:.1} mm of track in all; {edge}", net.name),
            )
        } else {
            (
                Verdict::Unknown,
                format!(
                    "{}: up to {length:.1} mm of track, so it may ring; {edge}; whether it does depends on the \
                     driver's strength and the far end's load",
                    net.name
                ),
            )
        };
        out.push(Finding {
            rule: "line-length",
            verdict,
            severity: Severity::Warning,
            reference: net.name.clone(),
            message,
            nets: vec![n],
            case: None,
            headline: None,
        });
    }
    out
}
