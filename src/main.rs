use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gerberus::limits::{Coverage, Library, coverage};
use gerberus::pinout;

const USAGE: &str = "usage:
  gerberus netlist <design.kicad_sch>
  gerberus parts <design.kicad_sch> [--parts DIR]...
  gerberus pinout <design.kicad_sch> [--pcb BOARD.kicad_pcb] [--parts DIR]...
  gerberus bom <design.kicad_sch> [--boards N] [--offline] [--cache DIR] [--max-age HOURS]

`parts` reports where each part's limits come from. It reads the shared library, then a `parts/` folder next
to the schematic if there is one (its files override the shared ones), then any --parts folders in order.

`pinout` checks that each part's symbol pins, footprint pads and datasheet pin table agree. It reads the board
next to the schematic (same name, .kicad_pcb) unless --pcb names another, and the same part folders as
`parts`. It exits with status 1 when it finds a pinout error.

`bom` looks up every BOM line by its LCSC part number: stock for --boards boards (default 10), LCSC's listing
status, and similar parts from other makers. Answers are cached in --cache (default ~/.cache/gerberus/lcsc) for
--max-age hours (default 24); --offline uses only the cache. It exits with status 1 when any line has an error.

Every command exits with status 2 when it can't run (bad arguments, unreadable files).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, path] if cmd == "netlist" => netlist(Path::new(path)),
        [cmd, path, rest @ ..] if cmd == "parts" => parts(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "pinout" => pinout(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "bom" => bom(Path::new(path), rest),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

fn netlist(path: &Path) -> Result<bool, String> {
    let design = gerberus::kicad::load(path)?;
    for net in &design.nets {
        let pins: Vec<String> = net.pins.iter().map(|(r, p)| format!("{r}.{p}")).collect();
        println!("{}: {}", net.name, pins.join(" "));
    }
    for w in &design.warnings {
        eprintln!("warning: {w}");
    }
    Ok(true)
}

/// Command-line options shared by the commands that read part files.
struct Options {
    /// The shared library, a `parts/` folder next to the schematic, then any `--parts` folders.
    dirs: Vec<PathBuf>,
    /// `--pcb`, when the command takes it and it was given.
    pcb: Option<PathBuf>,
}

fn options(path: &Path, rest: &[String], takes_pcb: bool) -> Result<Options, String> {
    let mut dirs: Vec<PathBuf> = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")];
    let local = path.parent().unwrap_or(Path::new(".")).join("parts");
    if local.is_dir() {
        dirs.push(local);
    }
    let mut pcb = None;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--parts", Some(dir)) => dirs.push(PathBuf::from(dir)),
            ("--pcb", Some(p)) if takes_pcb => pcb = Some(PathBuf::from(p)),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    Ok(Options { dirs, pcb })
}

fn load_library(dirs: &[PathBuf]) -> Result<Library, String> {
    Library::load_dirs(&dirs.iter().map(PathBuf::as_path).collect::<Vec<_>>())
}

fn parts(path: &Path, rest: &[String]) -> Result<bool, String> {
    let lib = load_library(&options(path, rest, false)?.dirs)?;
    let design = gerberus::kicad::load(path)?;
    let cov = coverage(&design, &lib);
    let (mut sheet, mut default, mut unchecked) = (0, 0, 0);
    for (r, c) in &cov {
        match c {
            Coverage::Datasheet { mpn } => {
                sheet += 1;
                println!("{r}: datasheet limits ({mpn}, {})", lib.files[mpn].display());
                let gaps = lib.parts[mpn].uncovered_pins();
                if !gaps.is_empty() {
                    println!("  no voltage limit on {}: these pins are UNKNOWN", gaps.join(", "));
                }
            }
            Coverage::ClassDefault { note } => {
                default += 1;
                println!("{r}: class default ({note})");
            }
            Coverage::Unchecked { reason } => {
                unchecked += 1;
                println!("{r}: UNCHECKED ({reason})");
            }
            Coverage::NotFitted => println!("{r}: not fitted"),
        }
    }
    println!("{sheet} with datasheet limits, {default} with class defaults, {unchecked} unchecked");
    Ok(true)
}

fn bom(path: &Path, rest: &[String]) -> Result<bool, String> {
    use gerberus::bom::{Lcsc, Options, Severity, check};
    let mut opts = Options::default();
    let mut offline = false;
    let mut cache = Lcsc::default_cache_dir();
    let mut max_age_hours: u64 = 24;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.clone().next()) {
            ("--offline", _) => offline = true,
            ("--boards", Some(n)) => {
                opts.boards = n
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or(format!("--boards needs a positive number, not {n:?}"))?;
                it.next();
            }
            ("--cache", Some(dir)) => {
                cache = Some(PathBuf::from(dir));
                it.next();
            }
            ("--max-age", Some(h)) => {
                max_age_hours = h
                    .parse()
                    .map_err(|_| format!("--max-age needs a number of hours, not {h:?}"))?;
                it.next();
            }
            ("--boards" | "--cache" | "--max-age", None) => return Err(format!("{a} needs a value\n{USAGE}")),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    let max_age = max_age_hours
        .checked_mul(3600)
        .map(std::time::Duration::from_secs)
        .ok_or(format!("--max-age {max_age_hours} is too large"))?;
    let design = gerberus::kicad::load(path)?;
    let mut source = Lcsc::new(cache, max_age, offline);
    let report = check(&design, &mut source, opts);
    let mut count = std::collections::BTreeMap::new();
    for h in &report {
        let what = match &h.listing {
            Some(l) => format!(
                "{} {}, {} in stock, LCSC status {}{}",
                l.code,
                l.mpn,
                l.stock,
                l.status,
                l.assembly
                    .as_deref()
                    .map(|a| format!(", JLCPCB {a}"))
                    .unwrap_or_default()
            ),
            None => h.line.lcsc.clone().unwrap_or_else(|| "no LCSC number".into()),
        };
        println!("{} ({}): {what}", h.line.references.join(", "), h.line.mpn);
        for f in &h.findings {
            println!("  {}: {}", f.severity, f.message);
            *count.entry(f.severity).or_insert(0) += 1;
        }
    }
    println!("note: {}", gerberus::bom::LIFECYCLE_NOTE);
    let n = |s| count.get(&s).copied().unwrap_or(0);
    println!(
        "{} lines for {} boards: {} errors, {} warnings, {} unknown",
        report.len(),
        opts.boards,
        n(Severity::Error),
        n(Severity::Warning),
        n(Severity::Unknown)
    );
    Ok(n(Severity::Error) == 0)
}

/// Returns whether the board passed (no errors).
fn pinout(path: &Path, rest: &[String]) -> Result<bool, String> {
    let opts = options(path, rest, true)?;
    let lib = load_library(&opts.dirs)?;
    let design = gerberus::kicad::load(path)?;
    let pcb = opts
        .pcb
        .or_else(|| Some(path.with_extension("kicad_pcb")).filter(|p| p.is_file()));
    let footprints = pcb.as_deref().map(gerberus::kicad::pcb::load_footprints).transpose()?;
    match &pcb {
        Some(p) => println!("board: {}", p.display()),
        None => println!("board: none found next to the schematic, so footprints are not checked"),
    }
    let report = pinout::check(&design, footprints.as_deref(), &lib);
    for f in &report.findings {
        let mpn = design.components.get(&f.reference).map(gerberus::limits::component_mpn);
        match mpn {
            Some(m) if !m.is_empty() => println!("{} ({m}): {}: {}", f.reference, f.severity, f.message),
            _ => println!("{}: {}: {}", f.reference, f.severity, f.message),
        }
    }
    let (mut all, mut fp_only, mut sheet_only, mut none) = (0, 0, 0, 0);
    let mut no_sheet: Vec<&str> = Vec::new();
    for (r, c) in &report.coverage {
        match (c.footprint, c.datasheet.is_some()) {
            (true, true) => all += 1,
            (true, false) => fp_only += 1,
            (false, true) => sheet_only += 1,
            (false, false) => none += 1,
        }
        if c.datasheet.is_none() {
            no_sheet.push(r);
        }
    }
    println!(
        "{}, {}. Compared: {all} parts with symbol, footprint and datasheet; {fp_only} symbol and \
         footprint only; {sheet_only} symbol and datasheet only; {none} not compared",
        count(report.errors(), "error"),
        count(report.warnings(), "warning")
    );
    if !no_sheet.is_empty() {
        println!("no part file, so no datasheet pinout: {}", no_sheet.join(" "));
    }
    for w in &design.warnings {
        eprintln!("warning: {w}");
    }
    Ok(report.errors() == 0)
}

fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}
