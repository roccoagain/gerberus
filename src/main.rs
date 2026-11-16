use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gerberus::inputs::BoardInputs;
use gerberus::limits::{Coverage, Library, coverage};
use gerberus::pinout;

const USAGE: &str = "usage:
  gerberus netlist <design>
  gerberus parts <design> [--parts DIR]...
  gerberus pinout <design.kicad_sch> [--pcb BOARD.kicad_pcb] [--parts DIR]...
  gerberus voltages <design> [--inputs FILE] [--parts DIR]... [--fault CASE] [--explain NET]...
  gerberus loads <design> [--inputs FILE] [--parts DIR]...
  gerberus check <design> [--inputs FILE] [--parts DIR]... [--all] [--json]
  gerberus bom <design> [--boards N] [--offline] [--cache DIR] [--max-age HOURS]
  gerberus datasheet verify <part.toml>... [--pdf FILE] [--cache DIR] [--all]
  gerberus datasheet blank <part.toml>
  gerberus datasheet compare <first.toml> <second.toml>

<design> is a KiCad root .kicad_sch, an Altium .PrjPcb project, or a single Altium .SchDoc. `pinout` reads
KiCad designs only, since it compares against the KiCad board.

`parts` reports where each part's limits come from. It reads the shared library, then a `parts/` folder next
to the schematic if there is one (its files override the shared ones), then any --parts folders in order.

`pinout` checks that each part's symbol pins, footprint pads and datasheet pin table agree. It reads the board
next to the schematic (same name, .kicad_pcb) unless --pcb names another, and the same part folders as
`parts`. It exits with status 1 when it finds a pinout error.

`voltages` works out the worst-case voltage range of every net, starting from the board inputs (`inputs.toml`
next to the schematic unless --inputs is given). --explain prints the reasoning behind one net. --fault works
them out for one of the fault cases in the inputs instead of normal operation.

`loads` lists the worst-case current each net supplies and what draws it, from the parts' `[[supply]]`
currents, LED chains and resistors to ground. These are the currents the junction-temperature rule uses.

`check` runs the voltage rules: every pin against its datasheet limits at its net's worst-case voltage, TVS
breakdown and stand-off, capacitor voltage and resistor power, undriven inputs and power pins, regulator
headroom, and parts with nothing to check against. Each fault case in the inputs is then checked against
absolute maximum ratings, TVS breakdown and capacitor ratings. FAILs cite the numbers, their datasheet pages and how each
net got its range. --all also lists passes; --json prints the findings as JSON. It exits with status 1 when
any error FAILs.

`bom` looks up every BOM line by its LCSC part number: stock for --boards boards (default 10), LCSC's listing
status, and similar parts from other makers. Answers are cached in --cache (default ~/.cache/gerberus/lcsc) for
--max-age hours (default 24); --offline uses only the cache. It exits with status 1 when any line has an error.

`datasheet` checks part files against their datasheets (see .claude/skills/datasheet-to-part). `verify` downloads each
file's datasheet (cached in --cache, default ~/.cache/gerberus/datasheets), or reads --pdf, and looks for every
cited value on its page: in the right MIN/TYP/MAX column, in the right unit, on a row that names the pin. It
needs poppler's `pdftotext`. --all also lists confirmed values. It exits with status 1 when a value is missing
or mismatched. `blank` prints the part file with every datasheet value replaced by \"?\", for a second reader to
fill in blind; `compare` lists every value where two readings disagree and exits with status 1 if any do.

Every command exits with status 2 when it can't run (bad arguments, unreadable files).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, path] if cmd == "netlist" => netlist(Path::new(path)),
        [cmd, path, rest @ ..] if cmd == "parts" => parts(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "pinout" => pinout(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "voltages" => voltages(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "loads" => loads(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "check" => check(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "bom" => bom(Path::new(path), rest),
        [cmd, sub, rest @ ..] if cmd == "datasheet" => datasheet(sub, rest),
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
    let design = gerberus::load(path)?;
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
    /// `--inputs` and `--explain`, for `voltages`.
    inputs: Option<PathBuf>,
    explain: Vec<String>,
    /// `--fault`, for `voltages`: a fault case to work out instead of normal operation.
    fault: Option<String>,
}

/// Reads `--parts` and whichever of `--pcb`, `--inputs` and `--explain` the command takes.
fn options(path: &Path, rest: &[String], takes: &[&str]) -> Result<Options, String> {
    let mut dirs: Vec<PathBuf> = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")];
    let local = path.parent().unwrap_or(Path::new(".")).join("parts");
    if local.is_dir() {
        dirs.push(local);
    }
    let (mut pcb, mut inputs, mut explain, mut fault) = (None, None, vec![], None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--parts", Some(dir)) => dirs.push(PathBuf::from(dir)),
            ("--pcb", Some(p)) if takes.contains(&"--pcb") => pcb = Some(PathBuf::from(p)),
            ("--inputs", Some(p)) if takes.contains(&"--inputs") => inputs = Some(PathBuf::from(p)),
            ("--explain", Some(n)) if takes.contains(&"--explain") => explain.push(n.clone()),
            ("--fault", Some(n)) if takes.contains(&"--fault") => fault = Some(n.clone()),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    Ok(Options {
        dirs,
        pcb,
        inputs,
        explain,
        fault,
    })
}

fn load_library(dirs: &[PathBuf]) -> Result<Library, String> {
    Library::load_dirs(&dirs.iter().map(PathBuf::as_path).collect::<Vec<_>>())
}

/// The design, its library and its worked-out net voltages.
fn analyse(
    path: &Path,
    o: &Options,
) -> Result<
    (
        gerberus::design::Design,
        Library,
        BoardInputs,
        gerberus::voltage::Voltages,
    ),
    String,
> {
    let lib = load_library(&o.dirs)?;
    let design = gerberus::load(path)?;
    let inputs_path = o
        .inputs
        .clone()
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("inputs.toml"));
    let inputs = BoardInputs::load(&inputs_path)?;
    let problems = inputs.problems(&design);
    if !problems.is_empty() {
        return Err(format!("{}: {}", inputs_path.display(), problems.join("; ")));
    }
    let v = match &o.fault {
        None => gerberus::voltage::compute(&design, &lib, &inputs),
        Some(f) if inputs.faults().contains(&f.as_str()) => {
            gerberus::voltage::compute(&design, &lib, &inputs.case(&design, f))
        }
        Some(f) => {
            return Err(format!(
                "{}: no fault case {f:?}; the cases are {:?}",
                inputs_path.display(),
                inputs.faults()
            ));
        }
    };
    Ok((design, lib, inputs, v))
}

fn check(path: &Path, rest: &[String]) -> Result<bool, String> {
    let all = rest.iter().any(|a| a == "--all");
    let json = rest.iter().any(|a| a == "--json");
    let rest: Vec<String> = rest
        .iter()
        .filter(|a| *a != "--all" && *a != "--json")
        .cloned()
        .collect();
    let o = options(path, &rest, &["--inputs"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("policy.toml"))?;
    let report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::rules::render(&design, &v, &report, all));
    }
    Ok(report.errors() == 0)
}

fn voltages(path: &Path, rest: &[String]) -> Result<bool, String> {
    let o = options(path, rest, &["--inputs", "--explain", "--fault"])?;
    let (design, _lib, _inputs, v) = analyse(path, &o)?;
    if o.explain.is_empty() {
        for (net, nv) in design.nets.iter().zip(&v.nets) {
            println!("{}: {}", net.name, nv.span);
        }
        return Ok(true);
    }
    for name in &o.explain {
        let i = design
            .nets
            .iter()
            .position(|n| &n.name == name)
            .ok_or_else(|| format!("no net named {name:?}"))?;
        print!("{}", v.explain(&design, i));
    }
    Ok(true)
}

fn loads(path: &Path, rest: &[String]) -> Result<bool, String> {
    let o = options(path, rest, &["--inputs"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let th = gerberus::thermal::compute(&design, &lib, &inputs, &v);
    print!("{}", gerberus::thermal::describe(&design, &th));
    Ok(true)
}

fn parts(path: &Path, rest: &[String]) -> Result<bool, String> {
    let lib = load_library(&options(path, rest, &[])?.dirs)?;
    let design = gerberus::load(path)?;
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
    let design = gerberus::load(path)?;
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
    let opts = options(path, rest, &["--pcb"])?;
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

fn datasheet(sub: &str, rest: &[String]) -> Result<bool, String> {
    use gerberus::datasheet;
    use gerberus::limits::Part;
    match (sub, rest) {
        ("blank", [file]) => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            print!("{}", datasheet::blank(&text).map_err(|e| format!("{file}: {e}"))?);
            Ok(true)
        }
        ("compare", [a, b]) => {
            let (first, second) = (Part::load(Path::new(a))?, Part::load(Path::new(b))?);
            let diffs = datasheet::compare(&first, &second);
            for d in &diffs {
                println!("{}: {} vs {}", d.at, d.first, d.second);
            }
            println!("{}: {} difference(s)", first.mpn, diffs.len());
            Ok(diffs.is_empty())
        }
        ("verify", rest) => {
            let (mut files, mut pdf, mut cache, mut all) = (vec![], None, datasheet::default_cache_dir(), false);
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--all" => all = true,
                    "--pdf" => pdf = Some(PathBuf::from(it.next().ok_or("--pdf needs a file")?)),
                    "--cache" => cache = Some(PathBuf::from(it.next().ok_or("--cache needs a directory")?)),
                    f if !f.starts_with("--") => files.push(PathBuf::from(f)),
                    _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
                }
            }
            if files.is_empty() || (pdf.is_some() && files.len() > 1) {
                return Err(format!("name one or more part files (one with --pdf)\n{USAGE}"));
            }
            let mut ok = true;
            for f in &files {
                let part = Part::load(f)?;
                let pdf = match (&pdf, &part.datasheet) {
                    (Some(p), _) => p.clone(),
                    (None, Some(d)) => {
                        let dir = cache.as_ref().ok_or("no cache directory; give --cache")?;
                        datasheet::fetch(&d.url, dir)?
                    }
                    (None, None) => {
                        println!("{}: no datasheet, nothing to verify", part.mpn);
                        continue;
                    }
                };
                let outcomes = datasheet::verify(&part, &pdf);
                ok &= !outcomes.iter().any(datasheet::Outcome::is_problem);
                print!("{}", datasheet::render(&part.mpn, &outcomes, all));
            }
            Ok(ok)
        }
        _ => Err(format!("unknown datasheet command\n{USAGE}")),
    }
}
