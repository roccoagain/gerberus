use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::design::{Design, PinKind};
use crate::limits::{Library, is_non_electrical};
use crate::voltage::is_connector;

pub const GUESS: &str = "GUESS by gerberus init";

pub const ASSUMED: &str = "assumed by gerberus init";

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
    out.sort();
    out.dedup();
    out
}

enum Guess {
    Ground,
    Input {
        name: String,
        min: f64,
        max: f64,
        why: String,
        reverse: Option<String>,
        fault: Option<(String, f64, f64, String)>,
    },
    Undriven(String),
    Unknown,
}

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

    let alone = crate::voltage::compute(design, lib, &crate::inputs::BoardInputs::none(board, design));
    let driven = |net: usize| -> Option<String> {
        alone.nets.get(net)?.why.iter().find_map(|w| {
            let t = w.text.as_str();
            let skip = t.starts_with("UNKNOWN")
                || t.contains("the reference of input")
                || t.starts_with("pulled")
                || t.strip_prefix("driven by output pin ")
                    .is_some_and(|rest| !drives_out(design, rest));
            (!skip).then(|| {
                let short: String = t.chars().take(90).collect();
                if short.len() < t.len() {
                    format!("{short}...")
                } else {
                    short
                }
            })
        })
    };

    let connectors: Vec<&str> = design
        .components
        .values()
        .filter(|c| !c.dnp && is_connector(lib, c))
        .map(|c| c.reference.as_str())
        .collect();
    let net_name = |r: &str, p: &str| design.pin_net(r, p).map_or(String::new(), |n| n.name.clone());
    let short = |n: &str| n.rsplit('/').next().unwrap_or("").to_string();
    let board_ground = connectors
        .iter()
        .flat_map(|r| design.components[*r].pins.keys().map(move |p| (*r, p.as_str())))
        .find(|(r, p)| is_ground_net(&short(&net_name(r, p))))
        .map(|(r, p)| format!("{r}.{p}"))
        .or_else(|| {
            design
                .nets
                .iter()
                .filter(|n| !n.pins.is_empty())
                .find(|n| is_ground_net(n.name.rsplit('/').next().unwrap_or("")))
                .and_then(|n| n.pins.iter().min())
                .map(|(r, p)| format!("{r}.{p}"))
        });
    if let Some(g) = &board_ground {
        let _ = writeln!(
            out,
            "\n# Ground, at 0 V.\n[[input]]\nname = \"ground\"\npin = \"{g}\"  # {}\nto = \"{g}\"\nmin = 0\nmax = 0\n\
             reverse = \"a ground reference, not a supply\"\nsource = {}",
            short(&net_name(
                g.split('.').next().unwrap_or(""),
                g.split_once('.').map_or("", |x| x.1)
            )),
            quote(&format!("{ASSUMED}: the net is named as a ground"))
        );
    }
    if connectors.is_empty() {
        let _ = writeln!(
            out,
            "\n# No connectors were found: a part counts as one when its symbol is from a Connector library or its\n\
             # part file has class = \"connector\"."
        );
        return out;
    }

    let mut guessed = 0;
    let mut open = vec![];
    for r in connectors {
        let c = &design.components[r];
        let ground = c
            .pins
            .keys()
            .find(|p| is_ground_net(&short(&net_name(r, p))))
            .map(|p| format!("{r}.{p}"))
            .or_else(|| board_ground.clone());
        let usb = c.lib_id.to_ascii_uppercase().contains("USB")
            || c.pins
                .keys()
                .any(|p| is_vbus(&short(&net_name(r, p)).to_ascii_uppercase()));
        let mut pins: Vec<&str> = c.pins.keys().map(String::as_str).collect();
        pins.sort_by_key(|p| natural(p));
        let _ = writeln!(out, "\n# {r} ({}, {})", c.value, c.lib_id);
        if is_non_electrical(r, c) {
            for p in &pins {
                if is_ground_net(&short(&net_name(r, p))) {
                    continue;
                }
                let _ = writeln!(
                    out,
                    "\n[[undriven]]\npin = \"{r}.{p}\"\nsource = \"{ASSUMED}: a test point, which only a probe touches\""
                );
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
            let g = guess(&short(&net), usb, driven(pin.net));
            if !matches!(g, Guess::Undriven(_)) && !seen.insert(pin.net) {
                continue;
            }
            match g {
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
                    reverse,
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
                    if let Some(rev) = reverse {
                        let _ = writeln!(out, "reverse = {}", quote(&rev));
                    }
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

fn drives_out(design: &Design, step: &str) -> bool {
    let pin = step.split([',', ' ']).next().unwrap_or("");
    let Some((r, p)) = pin.split_once('.') else {
        return false;
    };
    design
        .components
        .get(r)
        .is_some_and(|c| !c.dnp && c.in_bom && c.on_board && c.pins.get(p).is_some_and(|q| q.kind == PinKind::Output))
}

fn is_ground_net(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("GND") || matches!(n.as_str(), "AGND" | "DGND" | "PGND" | "SGND" | "0V" | "VSS")
}

fn is_vbus(upper: &str) -> bool {
    let words: Vec<&str> = upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().any(|w| {
        w.strip_prefix("VBUS")
            .is_some_and(|d| d.chars().all(|c| c.is_ascii_digit()))
    }) && !words.iter().any(|w| SIGNAL_WORDS.contains(w))
}

const SIGNAL_WORDS: &[&str] = &[
    "EN", "PG", "OK", "SENSE", "SNS", "DET", "DETECT", "TX", "RX", "SDA", "SCL", "CS", "CLK", "INT", "RST", "RESET",
    "IO", "LED", "FLT", "FAULT", "ALERT", "MON", "ADC", "SIG", "CTRL", "SEL", "OC", "PWR_EN", "ON", "OFF",
];

fn guess(name: &str, usb: bool, driven: Option<String>) -> Guess {
    let upper = name.to_ascii_uppercase();
    if is_ground_net(&upper) {
        return Guess::Ground;
    }
    if let Some(step) = driven {
        return Guess::Undriven(format!(
            "the board drives this net itself ({step}), so the connector takes it out rather than bringing it \
             in; if something plugged in can also drive it, make this an [[input]]"
        ));
    }
    let words: Vec<&str> = upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '+' && c != '-' && c != '.')
        .filter(|w| !w.is_empty())
        .collect();
    let signal = words.iter().any(|w| SIGNAL_WORDS.contains(&w.trim_matches(['+', '-'])));
    if is_vbus(&upper) {
        return Guess::Input {
            name: "VBUS".into(),
            min: 4.0,
            max: 5.5,
            why: "a USB VBUS pin, taken as 4.0 to 5.5 V to cover any USB 5 V source; check it against the USB \
                  spec the port follows and any power delivery contract"
                .into(),
            reverse: usb.then(|| "a USB plug can't put VBUS on its ground contacts".to_string()),
            fault: None,
        };
    }
    if usb && !signal {
        let data = ["D+", "D-", "DP", "DM", "DN", "DP1", "DN1", "DP2", "DN2", "DM1", "DM2"]
            .iter()
            .any(|w| words.contains(w));
        if data {
            return Guess::Input {
                name: "USB data".into(),
                min: 0.0,
                max: 3.6,
                why: "a USB 2.0 data line, taken as 0 to 3.6 V, the most full-speed signalling drives".into(),
                reverse: None,
                fault: Some((
                    "USB data shorted to VBUS".into(),
                    0.0,
                    5.5,
                    "USB 2.0 asks a port's data lines to survive a short to VBUS (taken as up to 5.5 V)".into(),
                )),
            };
        }
        if ["CC", "CC1", "CC2"].iter().any(|w| words.contains(w)) {
            return Guess::Input {
                name: "CC".into(),
                min: 0.0,
                max: 5.5,
                why: "a USB Type-C CC pin, taken as 0 to 5.5 V, the most VCONN can be".into(),
                reverse: None,
                fault: None,
            };
        }
    }
    if !signal && let Some(v) = words.iter().find_map(|w| volts_in(w)) {
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
                "the net is named {name} and nothing on the board drives it, so it is taken as a {} V supply \
                 ±10 %; replace it with the supply's real range",
                num(v)
            ),
            reverse: None,
            fault: None,
        };
    }
    Guess::Unknown
}

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
    use super::{Guess, guess, is_ground_net, is_vbus, volts_in};

    fn kind(name: &str, usb: bool) -> &'static str {
        match guess(name, usb, None) {
            Guess::Ground => "ground",
            Guess::Input { name, .. } if name == "VBUS" => "vbus",
            Guess::Input { name, .. } if name == "USB data" => "data",
            Guess::Input { name, .. } if name == "CC" => "cc",
            Guess::Input { .. } => "supply",
            Guess::Undriven(_) => "undriven",
            Guess::Unknown => "unknown",
        }
    }

    #[test]
    fn guesses_from_net_names() {
        assert_eq!(kind("+12V", false), "supply");
        assert_eq!(kind("3V3_D", false), "supply");
        for signal in ["UART_TX_3V3", "EN_3V3", "PG_5V", "VCC_5V_EN", "SDA_3V3"] {
            assert_eq!(kind(signal, false), "unknown", "{signal}");
        }
        assert_eq!(kind("VBUS1", true), "vbus");
        assert_eq!(kind("VBUS_DET", true), "unknown");
        assert_eq!(kind("USB1_D+", true), "data");
        assert_eq!(kind("USB_DN", true), "data");
        assert_eq!(kind("DP_AUX", false), "unknown");
        assert_eq!(kind("CC_LED", false), "unknown");
        assert_eq!(kind("CC1", true), "cc");
        assert_eq!(kind("GND", false), "ground");
        assert!(matches!(
            guess("+5V", false, Some("U1 regulates it".into())),
            Guess::Undriven(_)
        ));
        assert!(is_ground_net("AGND") && is_ground_net("VSS") && !is_ground_net("VSS_15"));
        assert!(is_vbus("USB_VBUS") && !is_vbus("VBUS_SENSE"));
    }

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
