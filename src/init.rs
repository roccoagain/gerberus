//! `gerberus init`: a starter inputs.toml, with a guess for every connector pin it can make one for.
//!
//! Each guess comes from the net's name: a ground, a USB VBUS, D+/D- or CC pin, or a supply named by its
//! voltage (`12V`, `+5V`, `3V3`). Every guessed entry's `source` starts with [`GUESS`], so `check` can say that
//! results rest on guesses, and `doctor` lists them to confirm. Pins it can't guess are listed in comments to
//! fill in.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::design::{Design, PinKind};
use crate::limits::{DriveKind, Library, non_electrical};
use crate::part_new::is_ground_name;
use crate::voltage::is_connector;

/// How every guessed entry's `source` starts.
pub const GUESS: &str = "GUESS by gerberus init";

/// The pins of every entry in `inputs` that is still a guess from `init`.
pub fn guesses(inputs: &crate::inputs::BoardInputs) -> Vec<String> {
    let mut out: Vec<String> = inputs
        .inputs
        .iter()
        .filter(|i| i.source.starts_with(GUESS))
        .map(|i| i.pin.to_string())
        .chain(
            inputs
                .undriven
                .iter()
                .filter(|u| u.source.starts_with(GUESS))
                .map(|u| u.pin.to_string()),
        )
        .collect();
    out.dedup();
    out
}

/// A connector pin's net, and what init makes of it.
enum Guess {
    Ground,
    /// An input: name, min, max, why, and a fault case to add beside it.
    Input {
        name: String,
        min: f64,
        max: f64,
        why: String,
        fault: Option<(String, f64, f64, String)>,
    },
    /// Nothing outside drives it.
    Undriven(String),
    /// Nothing to go on.
    Unknown,
}

/// The text of a starter inputs.toml for `design`.
pub fn template(design: &Design, lib: &Library, board: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Written by `gerberus init`. It says what the outside world puts on each connector pin, which is where\n\
         # every net's voltage range starts. Entries whose source starts with \"{GUESS}\" are guesses from\n\
         # net names: check each against the connector's real use and replace its source with where the range\n\
         # comes from (a design document, a spec, a datasheet). `gerberus check` says how many guesses its\n\
         # results rest on. Pins with no guess are listed in comments: give each an [[input]], or an\n\
         # [[undriven]] entry if nothing outside drives it. The format is under \"Board inputs\" and \"Net\n\
         # voltages\" in parts/README.md."
    );
    let _ = writeln!(out, "\nboard = {}", quote(board));
    let _ = writeln!(
        out,
        "\n# The ambient temperature the board works in, if you know it:\n\
         # [temperature]\n# min = 0\n# max = 40\n# source = \"...\""
    );

    let connectors: Vec<&str> = design
        .components
        .values()
        .filter(|c| !c.dnp && c.on_board && is_connector(lib, c))
        .map(|c| c.reference.as_str())
        .collect();
    if connectors.is_empty() {
        let _ = writeln!(
            out,
            "\n# No connectors were found: a part counts as one when its symbol is from a Connector library or its\n\
             # part file has class = \"connector\"."
        );
        return out;
    }
    let net_name = |r: &str, p: &str| design.pin_net(r, p).map_or(String::new(), |n| n.name.clone());
    let short = |n: &str| n.rsplit('/').next().unwrap_or("").to_string();
    // The ground a connector's inputs are measured from: its own ground pin, else any ground pin on the board.
    let board_ground = connectors
        .iter()
        .flat_map(|r| design.components[*r].pins.keys().map(move |p| (*r, p.as_str())))
        .find(|(r, p)| is_ground_name(&short(&net_name(r, p))))
        .map(|(r, p)| format!("{r}.{p}"))
        .or_else(|| {
            design
                .nets
                .iter()
                .find(|n| is_ground_name(n.name.rsplit('/').next().unwrap_or("")))
                .and_then(|n| n.pins.iter().min())
                .map(|(r, p)| format!("{r}.{p}"))
        });

    let mut guessed = 0;
    let mut open = vec![];
    for r in connectors {
        let c = &design.components[r];
        let ground = c
            .pins
            .keys()
            .find(|p| is_ground_name(&short(&net_name(r, p))))
            .map(|p| format!("{r}.{p}"))
            .or_else(|| board_ground.clone());
        let mut pins: Vec<&str> = c.pins.keys().map(String::as_str).collect();
        pins.sort_by_key(|p| natural(p));
        let _ = writeln!(out, "\n# {r} ({}, {})", c.value, c.lib_id);
        if non_electrical(r, c).is_some() {
            for p in &pins {
                let _ = writeln!(
                    out,
                    "\n[[undriven]]\npin = \"{r}.{p}\"\nsource = \"{GUESS}: a test point, which only a probe touches\""
                );
                guessed += 1;
            }
            continue;
        }
        let mut seen = BTreeSet::new();
        for p in pins {
            let pin = &c.pins[p];
            if pin.no_connect || pin.kind == PinKind::NoConnect {
                continue;
            }
            let net = net_name(r, p);
            // One entry per net: a connector's second VBUS or GND pin is on the same net as its first.
            if !seen.insert(pin.net) {
                continue;
            }
            match guess(design, lib, r, pin.net, &short(&net)) {
                Guess::Ground => {}
                Guess::Undriven(why) => {
                    guessed += 1;
                    let _ = writeln!(
                        out,
                        "\n[[undriven]]\npin = \"{r}.{p}\"  # {net}\nsource = {}",
                        quote(&format!("{GUESS}: {why}"))
                    );
                }
                Guess::Input {
                    name,
                    min,
                    max,
                    why,
                    fault,
                } => {
                    let Some(to) = &ground else {
                        open.push(format!("{r}.{p} ({net}): no ground pin to measure it from"));
                        continue;
                    };
                    guessed += 1;
                    let _ = writeln!(
                        out,
                        "\n[[input]]\nname = {}\npin = \"{r}.{p}\"  # {net}\nto = \"{to}\"\nmin = {}\nmax = {}\n\
                         source = {}",
                        quote(&format!("{r}.{p} {name}")),
                        num(min),
                        num(max),
                        quote(&format!("{GUESS}: {why}"))
                    );
                    if let Some((case, fmin, fmax, fwhy)) = fault {
                        let _ = writeln!(
                            out,
                            "\n[[input]]\nname = {}\npin = \"{r}.{p}\"\nto = \"{to}\"\nmin = {}\nmax = {}\n\
                             fault = {}\nsource = {}",
                            quote(&format!("{r}.{p} {name}, fault")),
                            num(fmin),
                            num(fmax),
                            quote(&format!("{r} {case}")),
                            quote(&format!("{GUESS}: {fwhy}"))
                        );
                    }
                }
                Guess::Unknown => {
                    let _ = writeln!(out, "\n# {r}.{p} ({net}): what drives it? No guess from its name.");
                    open.push(format!("{r}.{p} ({net})"));
                }
            }
        }
    }
    let _ = writeln!(
        out,
        "\n# {guessed} guessed entries; {} connector pins with no guess{}",
        open.len(),
        if open.is_empty() {
            String::new()
        } else {
            format!(": {}", open.join(", "))
        }
    );
    out
}

fn guess(design: &Design, lib: &Library, connector: &str, net: usize, name: &str) -> Guess {
    let upper = name.to_ascii_uppercase();
    if is_ground_name(&upper) {
        return Guess::Ground;
    }
    // A net the board drives itself: a regulator's output, or a power output on another part.
    if let Some(driver) = board_drives(design, lib, connector, net) {
        return Guess::Undriven(format!(
            "the board drives this net itself ({driver}), so the connector takes it out rather than bringing \
             it in; if something plugged in can also drive it, make this an [[input]]"
        ));
    }
    let words: Vec<&str> = upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '+' && c != '-' && c != '.')
        .filter(|w| !w.is_empty())
        .collect();
    let has = |w: &str| words.iter().any(|x| x.trim_matches(['+', '-']) == w);
    if upper.contains("VBUS") {
        return Guess::Input {
            name: "VBUS".into(),
            min: 4.0,
            max: 5.5,
            why: "a USB VBUS pin, taken as 4.0 to 5.5 V to cover any USB 5 V source; check it against the USB \
                  spec the port follows and any power delivery contract"
                .into(),
            fault: None,
        };
    }
    let usb_data = [
        "D+", "D-", "DP", "DM", "DP1", "DN1", "DP2", "DN2", "USB_DP", "USB_DM", "USB_D+", "USB_D-",
    ]
    .iter()
    .any(|w| words.contains(w))
        || (upper.contains("USB") && (upper.ends_with("_P") || upper.ends_with("_N")));
    if usb_data {
        return Guess::Input {
            name: "USB data".into(),
            min: 0.0,
            max: 3.6,
            why: "a USB 2.0 data line, taken as 0 to 3.6 V, the most full-speed signalling drives".into(),
            fault: Some((
                "USB data shorted to VBUS".into(),
                0.0,
                5.5,
                "USB 2.0 asks a port's data lines to survive a short to VBUS (taken as up to 5.5 V)".into(),
            )),
        };
    }
    if has("CC1") || has("CC2") || has("CC") {
        return Guess::Input {
            name: "CC".into(),
            min: 0.0,
            max: 5.5,
            why: "a USB Type-C CC pin, taken as 0 to 5.5 V, the most VCONN can be".into(),
            fault: None,
        };
    }
    if let Some(v) = words.iter().find_map(|w| volts_in(w)) {
        let (lo, hi) = if v < 0.0 {
            (v * 1.1, v * 0.9)
        } else {
            (v * 0.9, v * 1.1)
        };
        return Guess::Input {
            name: name.to_string(),
            min: round(lo),
            max: round(hi),
            why: format!(
                "the net is named {name}, so it is taken as {} V ±10 %; replace it with the supply's real range",
                num(v)
            ),
            fault: None,
        };
    }
    Guess::Unknown
}

/// `12V` → 12, `+5V` → 5, `3V3` → 3.3, `3.3V` → 3.3, `-12V` → -12; not `0V`, `V`, `VIN` or `5VA`.
fn volts_in(word: &str) -> Option<f64> {
    let (neg, w) = match word.strip_prefix('-') {
        Some(w) => (true, w),
        None => (false, word.strip_prefix('+').unwrap_or(word)),
    };
    let (whole, frac) = w.split_once('V')?;
    if whole.is_empty() || !whole.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if !frac.chars().all(|c| c.is_ascii_digit()) || (whole.contains('.') && !frac.is_empty()) {
        return None;
    }
    let v: f64 = if frac.is_empty() {
        whole.parse().ok()?
    } else {
        format!("{whole}.{frac}").parse().ok()?
    };
    (v > 0.0 && v <= 1000.0).then_some(if neg { -v } else { v })
}

/// What on the board drives `net`, if anything does: a part file's `regulates` drive, or a power output pin
/// on a part that isn't a connector.
fn board_drives(design: &Design, lib: &Library, connector: &str, net: usize) -> Option<String> {
    for (r, p) in &design.nets[net].pins {
        if r == connector {
            continue;
        }
        let Some(c) = design.components.get(r) else { continue };
        // A part left off the board (a module plugged in by hand) comes from outside, like a connector's.
        if c.dnp || !c.in_bom || !c.on_board || is_connector(lib, c) {
            continue;
        }
        if let Some(part) = lib.part_for(c) {
            let name = part.pins.get(p).map_or("", String::as_str);
            if part
                .drives
                .iter()
                .any(|d| d.kind == DriveKind::Regulates && d.pin == name && !d.feedback)
            {
                return Some(format!("{} ({}) regulates it", r, part.mpn));
            }
        }
        if c.pins.get(p).is_some_and(|q| q.kind == PinKind::PowerOut) {
            return Some(format!("{r}.{p} is a power output"));
        }
    }
    None
}

fn natural(n: &str) -> (String, u64, String) {
    let letters: String = n.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &n[letters.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (letters, digits.parse().unwrap_or(0), rest[digits.len()..].to_string())
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn num(v: f64) -> String {
    let v = round(v);
    if v == v.trunc() {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::volts_in;

    #[test]
    fn voltages_from_net_names() {
        assert_eq!(volts_in("12V"), Some(12.0));
        assert_eq!(volts_in("+5V"), Some(5.0));
        assert_eq!(volts_in("3V3"), Some(3.3));
        assert_eq!(volts_in("3.3V"), Some(3.3));
        assert_eq!(volts_in("-12V"), Some(-12.0));
        for no in ["0V", "V", "VIN", "5VA", "V5", "3.3V3", "VBUS", "12"] {
            assert_eq!(volts_in(no), None, "{no}");
        }
    }
}
