//! `gerberus part new`: a part file to fill in, started from a schematic symbol.
//!
//! The file loads as it is written: the symbol gives the part number, the pin map, the class and the datasheet
//! link, and every limit is `{ unknown = "..." }` with [`NOT_READ`] as its reason until someone reads the
//! datasheet. `check` reports those pins UNKNOWN with that reason, and `doctor` lists the file as one to fill in.

use std::fmt::Write as _;

use crate::design::{Component, Design};
use crate::limits::component_mpn;

/// The reason every unread value carries, which `doctor` looks for.
pub const NOT_READ: &str = "not read from the datasheet yet";

/// A part file for the part `which` names: a reference designator, or a part number one of the parts carries.
/// Returns the part number and the file's text.
pub fn template(design: &Design, which: &str, source: &str) -> Result<(String, String), String> {
    let c = design
        .components
        .get(which)
        .or_else(|| {
            design
                .components
                .values()
                .find(|c| component_mpn(c).eq_ignore_ascii_case(which))
        })
        .ok_or_else(|| format!("no part {which:?} in the design: name a reference such as U2, or a part number"))?;
    let mpn = component_mpn(c).to_string();
    if mpn.is_empty() {
        return Err(format!(
            "{} has no MPN field and no value to name its part by",
            c.reference
        ));
    }
    let refs: Vec<&str> = design
        .components
        .values()
        .filter(|o| component_mpn(o) == mpn)
        .map(|o| o.reference.as_str())
        .collect();
    let class = class_of(c);
    let field = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| {
                c.fields
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(n))
                    .map(|(_, v)| v.trim())
            })
            .filter(|v| !v.is_empty() && *v != "~")
            .unwrap_or("")
            .to_string()
    };
    let manufacturer = field(&["Manufacturer", "MFR", "Mfr", "Manufacturer_Name"]);
    let description = field(&["Description", "Desc"]);
    let url = field(&["Datasheet"]);
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else {
        String::new()
    };

    // Pin number to name, from the symbol. A pin with no name goes by its number.
    let mut pins: Vec<(String, String)> = c
        .pins
        .values()
        .map(|p| {
            let name = crate::pinout::shown_name(&p.name).unwrap_or_else(|| p.number.clone());
            (p.number.clone(), name)
        })
        .collect();
    // 1, 2, ... 10, not 1, 10, 2; BGA balls by letter, then number.
    pins.sort_by_key(|(n, _)| natural(n));
    let ground = pins
        .iter()
        .find(|(_, n)| is_ground_name(n))
        .or_else(|| {
            pins.iter().find(|(num, _)| {
                c.pins.get(num).is_some_and(|p| {
                    let net = design.nets[p.net]
                        .name
                        .rsplit('/')
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase();
                    matches!(net.as_str(), "GND" | "0V" | "VSS")
                })
            })
        })
        .map(|(_, n)| n.clone());

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Written by `gerberus part new` from {} in {source}.\n\
         # Fill it in from the datasheet: every value that says \"{NOT_READ}\"\n\
         # is UNKNOWN in `gerberus check` until it is replaced. parts/README.md gives the format;\n\
         # `gerberus datasheet verify` then checks each value against its page, and `gerberus pinout` checks\n\
         # the pin map against the symbol and footprint.",
        refs.join(", ")
    );
    if c.fields.get("MPN").is_none_or(|m| m.trim().is_empty()) {
        let _ = writeln!(
            out,
            "#\n# {} has no MPN field, so this file is matched by its value, {mpn:?}. Give the symbol an MPN field \
             and\n# put the same part number here.",
            c.reference
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "mpn = {}", quote(&mpn));
    let _ = writeln!(
        out,
        "manufacturer = {}{}",
        quote(&manufacturer),
        if manufacturer.is_empty() {
            "  # the maker's name"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "description = {}  # {}",
        quote(&description),
        if description.is_empty() {
            "one line: what it is, its package"
        } else {
            "from the symbol: check it describes the part, not the symbol"
        }
    );
    let _ = writeln!(
        out,
        "class = {}  # guessed from {}; one of capacitor, resistor, inductor, diode, tvs, led, connector, \
         oscillator, ic",
        quote(class),
        c.reference
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "[datasheet]");
    let _ = writeln!(out, "title = \"\"     # the title on its first page");
    let _ = writeln!(
        out,
        "document = \"\"  # document number and revision, so pages can be checked"
    );
    let _ = writeln!(
        out,
        "url = {}{}",
        quote(&url),
        if url.is_empty() { "  # where the PDF is" } else { "" }
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "# The symbol's pins. Check each name against the datasheet's pin table, and add any pad the symbol leaves\n\
         # out, such as an exposed pad."
    );
    let _ = writeln!(out, "[pins]");
    for (num, name) in &pins {
        let _ = writeln!(out, "{} = {}", key(num), quote(name));
    }

    let mut names: Vec<&str> = vec![];
    for (_, n) in &pins {
        if !names.contains(&n.as_str()) && !matches!(n.as_str(), "NC" | "DNC") {
            names.push(n);
        }
    }
    let limit = |out: &mut String, pins: &[&str], to: Option<&str>, comment: &str| {
        let _ = writeln!(out, "\n# {comment}");
        let _ = writeln!(out, "[[limit]]");
        let list: Vec<String> = pins.iter().map(|p| quote(p)).collect();
        let _ = writeln!(out, "pins = [{}]", list.join(", "));
        if let Some(to) = to {
            let _ = writeln!(out, "to = {}", quote(to));
        }
        let _ = writeln!(out, "abs_max = {{ unknown = \"{NOT_READ}\" }}");
        let _ = writeln!(out, "recommended = {{ unknown = \"{NOT_READ}\" }}");
    };
    match class {
        "resistor" | "capacitor" | "inductor" if names.len() == 2 => limit(
            &mut out,
            &names,
            None,
            "The voltage across the part: a resistor's working voltage, a capacitor's rated voltage.",
        ),
        "diode" | "led" | "tvs" if names.contains(&"K") && names.contains(&"A") => limit(
            &mut out,
            &["K"],
            Some("A"),
            "Cathode to anode: the reverse voltage. A TVS also takes `breakdown` and `clamp`.",
        ),
        "diode" | "led" | "tvs" if names.len() == 2 => limit(
            &mut out,
            &names,
            None,
            "Between its two pins. A bidirectional TVS also takes `breakdown`, `clamp` and `bidirectional = true`;\n\
             # a diode with a cathode and anode gives `pins = [\"K\"]` and `to = \"A\"`.",
        ),
        "connector" if names.len() >= 2 => limit(
            &mut out,
            &names,
            None,
            "Between any two contacts: the connector's rated voltage.",
        ),
        _ => match &ground {
            Some(g) => {
                let others: Vec<&str> = names.iter().copied().filter(|n| n != g).collect();
                if others.is_empty() {
                    let _ = writeln!(out, "\n# Add a [[limit]] for each pin from the datasheet.");
                } else {
                    limit(
                        &mut out,
                        &others,
                        Some(g),
                        "Every pin from ground, unread. Split it as the datasheet does: supply pins, then each \
                         group of\n# signal pins with its own range.",
                    );
                }
            }
            None => {
                let _ = writeln!(
                    out,
                    "\n# No ground pin was found on the symbol. Add a [[limit]] for each pin, with `to` naming the pin \
                     it is\n# measured from."
                );
            }
        },
    }
    if class == "ic" {
        let _ = writeln!(
            out,
            "\n# Also from the datasheet, where it gives them: [[drive]] for what the part puts on its outputs, \
             [[supply]]\n# currents, theta_ja and tj_max, and [[esd]] ratings (see parts/README.md)."
        );
    }
    Ok((mpn, out))
}

/// A pin number as a sort key: its letters, then its number.
fn natural(n: &str) -> (String, u64, String) {
    let letters: String = n.chars().take_while(|c| !c.is_ascii_digit()).collect();
    let rest = &n[letters.len()..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (letters, digits.parse().unwrap_or(0), rest[digits.len()..].to_string())
}

fn class_of(c: &Component) -> &'static str {
    let letters: String = c.reference.chars().take_while(char::is_ascii_alphabetic).collect();
    let lib = c.lib_id.to_ascii_lowercase();
    if lib.contains("tvs") {
        return "tvs";
    }
    if lib.contains("led") && letters.starts_with('D') {
        return "led";
    }
    if lib.starts_with("connector") {
        return "connector";
    }
    match letters.as_str() {
        "R" | "RN" => "resistor",
        "C" => "capacitor",
        "L" | "FB" => "inductor",
        "D" => "diode",
        "LED" => "led",
        "J" | "P" | "CN" | "X" if !lib.contains("crystal") => "connector",
        "Y" | "X" => "oscillator",
        _ => "ic",
    }
}

fn is_ground_name(n: &str) -> bool {
    let n = n.to_ascii_uppercase();
    matches!(
        n.as_str(),
        "GND" | "VSS" | "AGND" | "DGND" | "PGND" | "GNDA" | "GNDD" | "0V"
    )
}

/// A TOML string.
fn quote(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// A TOML key: bare when it can be.
fn key(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        s.to_string()
    } else {
        quote(s)
    }
}
