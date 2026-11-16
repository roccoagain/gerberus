use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gerberus::limits::{Coverage, Library, coverage};
use gerberus::pinout;

const USAGE: &str = "usage:
  gerberus netlist <design.kicad_sch>
  gerberus parts <design.kicad_sch> [--parts DIR]...
  gerberus pinout <design.kicad_sch> [--pcb BOARD.kicad_pcb] [--parts DIR]...

`parts` reports where each part's limits come from. It reads the shared library, then a `parts/` folder next
to the schematic if there is one (its files override the shared ones), then any --parts folders in order.

`pinout` checks that each part's symbol pins, footprint pads and datasheet pin table agree. It reads the board
next to the schematic (same name, .kicad_pcb) unless --pcb names another, and the same part folders as
`parts`. It exits with status 1 when it finds an error.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, path] if cmd == "netlist" => netlist(Path::new(path)),
        [cmd, path, rest @ ..] if cmd == "parts" => parts(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "pinout" => match pinout(Path::new(path), rest) {
            Ok(true) => Ok(()),
            Ok(false) => return ExitCode::FAILURE,
            Err(e) => Err(e),
        },
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn netlist(path: &Path) -> Result<(), String> {
    let design = gerberus::kicad::load(path)?;
    for net in &design.nets {
        let pins: Vec<String> = net.pins.iter().map(|(r, p)| format!("{r}.{p}")).collect();
        println!("{}: {}", net.name, pins.join(" "));
    }
    for w in &design.warnings {
        eprintln!("warning: {w}");
    }
    Ok(())
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

fn parts(path: &Path, rest: &[String]) -> Result<(), String> {
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
    Ok(())
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
