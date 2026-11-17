//! `gerberus doctor`: what to add so `check` can settle its UNKNOWN results, most useful first.
//!
//! Every UNKNOWN finding is traced back to what it waits on: a connector pin inputs.toml doesn't describe, a
//! part with no part file, or a pin its part file gives no limit. A finding on a net is traced through the
//! net's explanation (each step's `from` nets) to the steps that made it UNKNOWN, and the references those
//! steps name. Each thing to add is then ranked by how many UNKNOWN findings wait on it. A finding can wait on
//! several things, so adding one of them may not be enough to settle it: the count is an upper bound.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::design::Design;
use crate::diag::{Diagnostic, Level, Show, plural};
use crate::limits::{Coverage, Library, component_mpn, coverage};
use crate::rules::{Report, Verdict};
use crate::voltage::Voltages;

/// Where the board's files are, to say where each fix goes.
pub struct Setup {
    /// The inputs file `check` reads.
    pub inputs: PathBuf,
    /// No inputs file was found, so `check` ran knowing only ground.
    pub inputs_missing: bool,
    /// The `parts/` folder next to the schematic.
    pub parts: PathBuf,
    /// The design, as given on the command line, for the commands the help suggests.
    pub design: String,
}

/// One thing to add.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// `inputs`, `part-file`, `fill-in`, `pin-limits` or `drive`.
    pub rule: &'static str,
    pub title: String,
    /// UNKNOWN findings that wait on this.
    pub unknown: usize,
    pub references: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    /// Connector pins for `inputs`, part pin names for `pin-limits`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<String>,
    /// The datasheet link on the schematic symbol, if it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasheet: Option<String>,
    /// The file to write or change.
    pub file: String,
    pub notes: Vec<String>,
    pub help: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Doctor {
    pub items: Vec<Item>,
    /// UNKNOWN findings in all.
    pub unknown: usize,
    /// UNKNOWN findings some item waits on.
    pub settled: usize,
    /// Pins whose inputs.toml entry is still a guess from `gerberus init`.
    pub guesses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    /// A connector, by reference.
    Inputs(String),
    /// A part number with no part file.
    Part(String),
    /// A part number whose part file has no limit for some pins.
    Pins(String),
    /// A part whose power output no part file says the voltage of, by reference.
    Drive(String),
    /// A part number whose part file, started by `part new`, still has values not read from the datasheet.
    Fill(String),
}

pub fn diagnose(
    design: &Design,
    lib: &Library,
    setup: &Setup,
    inputs: &crate::inputs::BoardInputs,
    report: &Report,
    volts: &Voltages,
) -> Doctor {
    let cov = coverage(design, lib);
    let unchecked = |r: &str| matches!(cov.get(r), Some(Coverage::Unchecked { .. }));
    let mpn = |r: &str| component_mpn(&design.components[r]).to_string();
    // A part whose file `part new` started and nobody has finished.
    let unread = |r: &str| {
        design
            .components
            .get(r)
            .and_then(|c| lib.part_for(c))
            .is_some_and(crate::limits::Part::unread)
    };
    let mut waits: BTreeMap<Key, BTreeSet<usize>> = BTreeMap::new();
    let mut pins: BTreeMap<Key, BTreeSet<String>> = BTreeMap::new();
    let mut unknown = 0;
    let mut settled = 0;
    for (i, f) in report.findings.iter().enumerate() {
        // `check` counts summary findings (reverse polarity) apart from the rest, so they aren't counted here.
        if f.verdict != Verdict::Unknown || crate::rules::SUMMARY_RULES.contains(&f.rule) {
            continue;
        }
        unknown += 1;
        let mut keys = BTreeSet::new();
        if f.rule == "unchecked"
            && f.message.contains("has no voltage limit for this pin")
            && let Some(pin) = f
                .message
                .split(':')
                .next()
                .and_then(|p| p.strip_prefix(&format!("{}.", f.reference)))
        {
            let k = Key::Pins(mpn(&f.reference));
            pins.entry(k.clone()).or_default().insert(pin.to_string());
            keys.insert(k);
        }
        if unread(&f.reference) {
            keys.insert(Key::Fill(mpn(&f.reference)));
        }
        if design.components.contains_key(&f.reference) && unchecked(&f.reference) {
            keys.insert(Key::Part(mpn(&f.reference)));
        }
        for r in references_in(&f.message, design) {
            if unchecked(&r) {
                keys.insert(Key::Part(mpn(&r)));
            }
            if unread(&r) {
                keys.insert(Key::Fill(mpn(&r)));
            }
        }
        let v = match &f.case {
            None => volts,
            Some(case) => report
                .cases
                .iter()
                .find(|(name, _)| name == case)
                .map_or(volts, |(_, v)| v),
        };
        for text in roots(v, &f.nets) {
            if let Some((r, p)) = connector_pin(text) {
                let k = Key::Inputs(r.to_string());
                pins.entry(k.clone()).or_default().insert(format!("{r}.{p}"));
                keys.insert(k);
                continue;
            }
            // A transistor's channel isn't modelled, so a part file for it wouldn't settle what its pins do to
            // a net.
            let cant_model = text.contains("on a part gerberus can't model");
            for r in references_in(text, design) {
                if unchecked(&r) && !(cant_model && is_transistor(&design.components[&r])) {
                    keys.insert(Key::Part(mpn(&r)));
                }
                if unread(&r) && !cant_model {
                    keys.insert(Key::Fill(mpn(&r)));
                }
            }
            if let Some((r, p)) = power_output(text)
                && !unchecked(r)
            {
                let k = Key::Drive(r.to_string());
                pins.entry(k.clone()).or_default().insert(format!("{r}.{p}"));
                keys.insert(k);
            }
        }
        if !keys.is_empty() {
            settled += 1;
        }
        for k in keys {
            waits.entry(k).or_default().insert(i);
        }
    }

    let refs_of = |m: &str| -> Vec<String> {
        cov.iter()
            .filter(|(r, c)| !matches!(c, Coverage::NotFitted) && component_mpn(&design.components[*r]) == m)
            .map(|(r, _)| r.clone())
            .collect()
    };
    let mut items = vec![];
    let mut connectors: Vec<(String, BTreeSet<String>, BTreeSet<usize>)> = vec![];
    for (k, found) in &waits {
        match k {
            Key::Inputs(r) => connectors.push((r.clone(), pins[k].clone(), found.clone())),
            Key::Part(m) => items.push(part_item(design, lib, setup, m, refs_of(m), found.len())),
            Key::Drive(r) => {
                let p: Vec<String> = pins[k].iter().cloned().collect();
                let m = mpn(r);
                let file = lib.files.get(&m).map(|f| f.display().to_string());
                let fitted = !matches!(cov.get(r.as_str()), Some(Coverage::NotFitted));
                items.push(Item {
                    rule: "drive",
                    title: format!(
                        "say what {r} puts on {}: {}",
                        list(&p, "and"),
                        waiting(found.len())
                    ),
                    unknown: found.len(),
                    references: vec![r.clone()],
                    mpn: Some(m.clone()),
                    pins: p.clone(),
                    datasheet: None,
                    file: file.clone().unwrap_or_else(|| setup.parts.join(file_name(&m)).display().to_string()),
                    notes: match (&file, fitted) {
                        (Some(f), _) => vec![format!(
                            "{f} has no [[drive]] for {}, so nothing says what the part puts on {}",
                            if p.len() == 1 { "it" } else { "them" },
                            if p.len() == 1 { "its net" } else { "their nets" }
                        )],
                        (None, false) => vec![format!(
                            "{r} isn't fitted (DNP or not in the BOM), so nothing says what its power outputs \
                             give, but the board relies on them"
                        )],
                        (None, true) => vec![format!("{r} has no part file to say what its power outputs give")],
                    },
                    help: match (&file, fitted) {
                        (Some(f), _) if f.ends_with("(built in)") => format!(
                            "the shared library's file has no [[drive]] there: copy it next to the schematic with \
                             `gerberus part show {m} > {}` and add one for the pin (see \"What a part drives\" in \
                             parts/README.md)",
                            setup.parts.join(file_name(&m)).display()
                        ),
                        (Some(_), _) => format!(
                            "give {m}'s part file a [[drive]] for the pin (see \"What a part drives\" in parts/README.md)"
                        ),
                        (None, false) => "if it is a module plugged in by hand, give the net it supplies an [[input]] in \
                                          inputs.toml; otherwise write a part file for it with a [[drive]] for the pin \
                                          (see \"What a part drives\" in parts/README.md)"
                            .into(),
                        (None, true) => "write a part file for it with a [[drive]] for the pin (see \"What a part \
                                         drives\" in parts/README.md)"
                            .into(),
                    },
                });
            }
            Key::Fill(m) => {
                let file = lib.files.get(m).map(|f| f.display().to_string()).unwrap_or_default();
                let datasheet = lib
                    .parts
                    .get(m)
                    .and_then(|p| p.datasheet.as_ref())
                    .map(|d| d.url.clone());
                items.push(Item {
                    rule: "fill-in",
                    title: format!("fill in {file}: {}", waiting(found.len())),
                    unknown: found.len(),
                    references: refs_of(m),
                    mpn: Some(m.clone()),
                    pins: vec![],
                    datasheet: datasheet.clone().filter(|u| !u.is_empty()),
                    file: file.clone(),
                    notes: vec![
                        format!("it still has values marked \"{}\"", crate::part_new::NOT_READ),
                        saved_note(setup, m),
                    ],
                    help: format!(
                        "read them from the datasheet{}, then run `gerberus datasheet verify {file}`",
                        datasheet
                            .filter(|u| !u.is_empty())
                            .map(|u| format!(" ({u})"))
                            .unwrap_or_default()
                    ),
                });
            }
            Key::Pins(m) => {
                let file = lib.files.get(m).map(|f| f.display().to_string()).unwrap_or_default();
                let names: Vec<String> = pins[k].iter().cloned().collect();
                let built_in = file.ends_with("(built in)");
                items.push(Item {
                    rule: "pin-limits",
                    title: format!(
                        "give {m} a voltage limit for {}: {}",
                        list(&names, "and"),
                        waiting(found.len())
                    ),
                    unknown: found.len(),
                    references: refs_of(m),
                    mpn: Some(m.clone()),
                    pins: names,
                    datasheet: None,
                    file: file.clone(),
                    notes: vec![format!("its part file is {file}")],
                    help: if built_in {
                        format!(
                            "the shared library's file has no limit there: copy it next to the schematic with `gerberus part \
                             show {m} > {}` and add a [[limit]] naming the pins, from the datasheet",
                            setup.parts.join(file_name(m)).display()
                        )
                    } else {
                        "add a [[limit]] naming the pins, from the datasheet (see parts/README.md)".into()
                    },
                });
            }
        }
    }
    if setup.inputs_missing {
        let found: BTreeSet<usize> = connectors.iter().flat_map(|(_, _, f)| f.iter().copied()).collect();
        let all: Vec<String> = connectors.iter().flat_map(|(_, p, _)| p.iter().cloned()).collect();
        items.push(Item {
            rule: "inputs",
            title: if found.is_empty() {
                format!(
                    "write {}: reverse polarity, ESD and fault cases aren't checked without it",
                    setup.inputs.display()
                )
            } else {
                format!("write {}: {}", setup.inputs.display(), waiting(found.len()))
            },
            unknown: found.len(),
            references: connectors.iter().map(|(r, _, _)| r.clone()).collect(),
            mpn: None,
            pins: all.clone(),
            datasheet: None,
            file: setup.inputs.display().to_string(),
            notes: {
                let mut notes = vec![];
                if !all.is_empty() {
                    notes.push(format!("nothing says what plugs into {}", list(&all, "or")));
                }
                if all.is_empty() {
                    notes.push("without it, ground is the only voltage known".into());
                } else {
                    notes.push(
                        "without it, ground is the only voltage known, and reverse polarity, ESD and fault cases \
                         aren't checked, so more results wait on it than these"
                            .into(),
                    );
                }
                notes
            },
            help: "give each connector pin's voltage range as an [[input]], or an [[undriven]] entry if nothing \
                   outside drives it (see \"Board inputs\" and \"Net voltages\" in parts/README.md)"
                .into(),
        });
    } else {
        for (r, p, found) in connectors {
            let p: Vec<String> = p.into_iter().collect();
            let mut notes = vec![format!(
                "{} {} no range in {}",
                list(&p, "and"),
                if p.len() == 1 { "has" } else { "have" },
                setup.inputs.display()
            )];
            if crate::limits::non_electrical(&r, &design.components[&r]).is_some() {
                notes.push(format!(
                    "{r} is a test point: if only a probe touches it, an [[undriven]] entry is enough"
                ));
            }
            items.push(Item {
                rule: "inputs",
                title: format!("say what plugs into {r}: {}", waiting(found.len())),
                unknown: found.len(),
                references: vec![r.clone()],
                mpn: None,
                pins: p.clone(),
                datasheet: None,
                file: setup.inputs.display().to_string(),
                notes,
                help: "add an [[input]] for each pin with its voltage range, or an [[undriven]] entry if nothing \
                       outside drives it (see \"Board inputs\" and \"Net voltages\" in parts/README.md)"
                    .into(),
            });
        }
    }
    // Part numbers that share a datasheet are often one series, which a part family covers in one file.
    let mut by_sheet: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for i in items.iter().filter(|i| i.rule == "part-file") {
        if let (Some(d), Some(m)) = (&i.datasheet, &i.mpn) {
            by_sheet.entry(d.clone()).or_default().push(m.clone());
        }
    }
    for i in items.iter_mut().filter(|i| i.rule == "part-file") {
        let (Some(d), Some(m)) = (&i.datasheet, &i.mpn) else {
            continue;
        };
        let others: Vec<String> = by_sheet[d].iter().filter(|o| *o != m).cloned().collect();
        if !others.is_empty() {
            i.notes.push(format!(
                "{} share{} its datasheet: if they are one series, a part family file covers them all (see \
                 \"Part families\" in parts/README.md)",
                list(&others, "and"),
                if others.len() == 1 { "s" } else { "" }
            ));
        }
    }
    items.sort_by(|a, b| b.unknown.cmp(&a.unknown).then_with(|| a.title.cmp(&b.title)));
    // Guesses come first whatever they settle: every result that rests on one is only as good as the guess.
    let guesses = crate::init::guesses(inputs);
    if !guesses.is_empty() {
        items.insert(
            0,
            Item {
                rule: "guess",
                title: format!(
                    "confirm the {} {} `gerberus init` made in {}: results that rest on them are only as good as \
                     the guesses",
                    guesses.len(),
                    if guesses.len() == 1 { "guess" } else { "guesses" },
                    setup.inputs.display()
                ),
                unknown: 0,
                references: vec![],
                mpn: None,
                pins: guesses.clone(),
                datasheet: None,
                file: setup.inputs.display().to_string(),
                notes: vec![format!("guessed: {}", list(&guesses, "and"))],
                help: format!(
                    "check each against what really plugs in, and replace its source (which starts \"{}\") with \
                     where the range comes from",
                    crate::init::GUESS
                ),
            },
        );
    }
    Doctor {
        items,
        unknown,
        settled,
        guesses,
    }
}

/// Where the part's datasheet is saved in the board's `datasheets/` folder, or the name to save it under.
fn saved_note(setup: &Setup, mpn: &str) -> String {
    let dir = crate::datasheet::folder_beside(&setup.parts);
    match crate::datasheet::saved_in(&dir, mpn) {
        Some(p) => format!("datasheet saved: {}", p.display()),
        None => format!(
            "save the PDF as {}: `datasheet verify` reads it from there, and an agent reads it to fill the file in",
            dir.join(crate::datasheet::saved_name(mpn)).display()
        ),
    }
}

fn part_item(design: &Design, lib: &Library, setup: &Setup, m: &str, refs: Vec<String>, found: usize) -> Item {
    let c = &design.components[&refs[0]];
    let datasheet = refs
        .iter()
        .filter_map(|r| design.components[r].fields.get("Datasheet"))
        .map(|d| d.trim())
        .find(|d| d.starts_with("http://") || d.starts_with("https://"))
        .map(str::to_string);
    let mut notes = vec![];
    match &datasheet {
        Some(url) => notes.push(format!("datasheet: {url}")),
        None => notes.push("the symbol has no datasheet link: find the PDF by the part number".into()),
    }
    notes.push(saved_note(setup, m));
    if let Some(e) = lib.family_errors.get(m) {
        notes.push(format!("a built-in part family matches {m:?} but can't rate it: {e}"));
    }
    let letters: String = c.reference.chars().take_while(char::is_ascii_alphabetic).collect();
    if matches!(letters.as_str(), "J" | "P" | "CN" | "X" | "USB") {
        notes.push(
            "if it is a connector, its part file can be short: class = \"connector\", its pins, and any rated \
             voltage; then inputs.toml says what plugs into it"
                .into(),
        );
    }
    let file = setup.parts.join(file_name(m));
    Item {
        rule: "part-file",
        title: format!("write a part file for {m}: {}", waiting(found)),
        unknown: found,
        references: refs,
        mpn: Some(m.to_string()),
        pins: vec![],
        datasheet,
        file: file.display().to_string(),
        notes,
        help: format!(
            "start {} with `gerberus part new {} {}`, then fill it in from the datasheet: parts/README.md says \
             how, and an agent can do it with the datasheet-to-part skill",
            file.display(),
            if setup.design.contains(char::is_whitespace) {
                format!("\"{}\"", setup.design)
            } else {
                setup.design.clone()
            },
            c.reference
        ),
    }
}

/// A file name for a part number: characters a file name can't hold become `_`.
pub fn file_name(mpn: &str) -> String {
    let safe: String = mpn
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.+".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.toml")
}

/// The steps that made these nets UNKNOWN, following each net's explanation back through the nets it read.
fn roots<'a>(volts: &'a Voltages, nets: &[usize]) -> Vec<&'a str> {
    let mut seen = BTreeSet::new();
    let mut stack = nets.to_vec();
    let mut out = vec![];
    while let Some(n) = stack.pop() {
        if !seen.insert(n) {
            continue;
        }
        let Some(nv) = volts.nets.get(n) else { continue };
        if nv.span.min.is_some() && nv.span.max.is_some() {
            continue;
        }
        for w in &nv.why {
            if w.text.starts_with("UNKNOWN") {
                out.push(w.text.as_str());
            }
            stack.extend(&w.from);
        }
    }
    out
}

/// `J1`, `3` from `UNKNOWN: J1.3 is a connector pin with no range in inputs.toml...`.
fn connector_pin(text: &str) -> Option<(&str, &str)> {
    let head = &text[..text.find(" is a connector pin")?];
    head.rsplit(' ').next()?.split_once('.')
}

/// The references a message names, as `D1` or a pin `D1.2`.
fn references_in(text: &str, design: &Design) -> BTreeSet<String> {
    text.split(|c: char| c.is_whitespace() || ",;:()\"'".contains(c))
        .filter_map(|w| w.split('.').next())
        .filter(|r| design.components.contains_key(*r))
        .map(str::to_string)
        .collect()
}

/// A transistor, whose channel no [[drive]] can describe: gerberus doesn't model MOSFET or BJT channels yet.
fn is_transistor(c: &crate::design::Component) -> bool {
    let letters: String = c.reference.chars().take_while(char::is_ascii_alphabetic).collect();
    letters == "Q" || c.lib_id.starts_with("Transistor")
}

/// `3 unknown results wait on it`.
fn waiting(n: usize) -> String {
    format!(
        "{} {} on it",
        plural(n, "unknown result"),
        if n == 1 { "waits" } else { "wait" }
    )
}

/// `A1`, `38` from `UNKNOWN: A1.38 is a power output, and no part file says what it drives`.
fn power_output(text: &str) -> Option<(&str, &str)> {
    let end = text.find(" is a power output, and no part file says")?;
    text[..end].rsplit(' ').next()?.split_once('.')
}

/// `a, b and c`.
fn list(items: &[String], and: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// The to-do list for the terminal.
pub fn render(doctor: &Doctor, show: &Show) -> String {
    let mut out = String::new();
    for item in &doctor.items {
        let d = Diagnostic {
            level: Level::Todo,
            rule: item.rule.to_string(),
            headline: item.title.clone(),
            location: (!item.references.is_empty()).then(|| item.references.join(", ")),
            notes: item.notes.clone(),
            help: Some(item.help.clone()),
            details: vec![],
        };
        d.render(&mut out, show.style, show.verbose, show.columns());
    }
    let style = show.style;
    if doctor.items.is_empty() {
        let _ = writeln!(
            out,
            "{}: nothing to add: {}",
            style.paint("1;32", "ok"),
            if doctor.unknown == 0 {
                "check knows every range and limit it needs".to_string()
            } else {
                format!(
                    "the {} left wait on things doctor doesn't trace yet: limits or thermal data the part \
                     files don't give, or circuits gerberus can't model; `gerberus check --unknown` lists them",
                    plural(doctor.unknown, "unknown result")
                )
            }
        );
        return out;
    }
    let _ = writeln!(
        out,
        "{}: {}",
        style.paint("1;34", "todo"),
        style.paint(
            "1",
            &format!(
                "{} to add; {} of {} wait on them",
                plural(doctor.items.len(), "thing"),
                doctor.settled,
                plural(doctor.unknown, "unknown result")
            )
        )
    );
    let left = doctor.unknown - doctor.settled;
    if left > 0 {
        let _ = writeln!(
            out,
            "   {} {}: the other {left} wait on things doctor doesn't trace yet: limits or thermal \
             data the part files don't give, or circuits gerberus can't model; `gerberus check --unknown` lists \
             them all",
            style.paint("1;34", "="),
            style.paint("1", "note"),
        );
    }
    out
}

/// Where `check` looks for a board's files.
pub fn setup(design: &Path, inputs: Option<&Path>) -> Setup {
    let dir = design.parent().unwrap_or(Path::new("."));
    let path = inputs.map_or_else(|| dir.join("inputs.toml"), Path::to_path_buf);
    Setup {
        inputs_missing: !path.exists(),
        inputs: path,
        parts: dir.join("parts"),
        design: design.display().to_string(),
    }
}
