use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gerberus::diag::{Diagnostic, Level, Show};
use gerberus::inputs::BoardInputs;
use gerberus::limits::{Coverage, Library, coverage};
use gerberus::pinout;

const USAGE: &str = "usage:
  gerberus netlist <design>
  gerberus parts <design> [--parts DIR]...
  gerberus pinout <design.kicad_sch> [--pcb BOARD.kicad_pcb] [--parts DIR]... [--verbose] [--color WHEN]
  gerberus voltages <design> [--inputs FILE] [--parts DIR]... [--fault CASE] [--explain NET]...
  gerberus loads <design> [--inputs FILE] [--parts DIR]...
  gerberus stress <design> [--inputs FILE] [--parts DIR]... [--runs N] [--seed N] [--netlist FILE] [REPORT]
  gerberus check <design> [--inputs FILE] [--parts DIR]... [REPORT]
  gerberus bom <design> [--boards N] [--offline] [--cache DIR] [--max-age HOURS]
  gerberus si <board.kicad_pcb> [--parts DIR]... [REPORT] [--plots DIR]
  gerberus part show <MPN> [--parts DIR]...
  gerberus datasheet verify <part.toml>... [--pdf FILE] [--cache DIR] [--all]
  gerberus datasheet blank <part.toml>
  gerberus datasheet compare <first.toml> <second.toml>
  gerberus update
  gerberus --version

REPORT is any of --unknown, --all, --verbose (-v), --color auto|always|never and --json. Reports list errors
then warnings, rustc-style. --unknown also lists UNKNOWNs, --all lists UNKNOWNs and passes, and --verbose adds the
long explanations (how each net got its range). Colour is on for a terminal unless NO_COLOR is set. --json prints
the findings as JSON instead.

<design> is a KiCad root .kicad_sch, an Altium .PrjPcb project, or a single Altium .SchDoc. `pinout` reads
KiCad designs only, since it compares against the KiCad board.

`parts` reports where each part's limits come from. It reads the shared library (built into gerberus), then a
`parts/` folder next to the schematic if there is one (its files override the shared ones), then any --parts
folders in order.

`pinout` checks that each part's symbol pins, footprint pads and datasheet pin table agree. It reads the board
next to the schematic (same name, .kicad_pcb) unless --pcb names another, and the same part folders as
`parts`. It exits with status 1 when it finds a pinout error.

`voltages` works out the worst-case voltage range of every net, starting from the board inputs (`inputs.toml`
next to the schematic unless --inputs is given). --explain prints the reasoning behind one net. --fault works
them out for one of the fault cases in the inputs instead of normal operation.

`loads` lists the worst-case current each net supplies and what draws it, from the parts' `[[supply]]`
currents, LED chains and resistors to ground. These are the currents the junction-temperature rule uses.

`stress` switches each supply input in `inputs.toml` on through a cable (policy.toml's `[spice]`) in ngspice,
once at nominal values and then --runs times (default from policy.toml) with every part value drawn from its
tolerance, and checks the peak voltage on every pin against its limits. --netlist writes the nominal run's
netlist. It needs ngspice installed, and exits with status 1 when an absolute maximum is passed.

`check` runs the voltage rules: every pin against its datasheet limits at its net's worst-case voltage, TVS
breakdown and stand-off, capacitor voltage and resistor power, undriven inputs and power pins, regulator
headroom, and parts with nothing to check against. Each fault case in the inputs is then checked against
absolute maximum ratings, TVS breakdown and capacitor ratings. FAILs cite the numbers, their datasheet pages and how each
net got its range (with --verbose). It exits with status 1 when any error FAILs.

`bom` looks up every BOM line by its LCSC part number: stock for --boards boards (default 10), LCSC's listing
status, and similar parts from other makers. Answers are cached in --cache (default ~/.cache/gerberus/lcsc) for
--max-age hours (default 24); --offline uses only the cache. It exits with status 1 when any line has an error.

`si` checks every USB 2.0 pair on a KiCad board (nets ending in D+/D-, DP/DM or _P/_N with USB in the name):
impedance from a 2D field solver run on the stackup, skew, vias, stubs, reference planes, length and the
capacitance of parts on the lines, then simulates each pair at 480 Mb/s and checks the eye at the connector
against the USB 2.0 Template 1 mask. --plots writes each pair's eye diagram and impedance profile (TDR) as SVG
files into DIR. It reads part capacitance from the same part folders as `parts`, matched by footprint value.
It exits with status 1 when any error FAILs.
`part show` prints the part file the library has for a part number: the file itself, or the one its part family
writes for it (see parts/README.md, \"Part families\"). Save it and run `datasheet verify` on it to check a
family's values against its datasheet.

`datasheet` checks part files against their datasheets (see .claude/skills/datasheet-to-part). `verify` downloads each
file's datasheet (cached in --cache, default ~/.cache/gerberus/datasheets), or reads --pdf, and looks for every
cited value on its page: in the right MIN/TYP/MAX column, in the right unit, on a row that names the pin. It
needs poppler's `pdftotext`. --all also lists confirmed values. It exits with status 1 when a value is missing
or mismatched. `blank` prints the part file with every datasheet value replaced by \"?\", for a second reader to
fill in blind; `compare` lists every value where two readings disagree and exits with status 1 if any do.

`update` updates gerberus to the latest release with `gerberus-update`, which the installers put next to it.
Once a day, gerberus also asks GitHub whether a newer release is out and says so after a command finishes; set
GERBERUS_NO_UPDATE_CHECK=1 to turn that off. It never checks when its output isn't a terminal or in CI.

Every command exits with status 2 when it can't run (bad arguments, unreadable files).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag] = args.as_slice()
        && (flag == "--version" || flag == "-V")
    {
        println!("gerberus {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if let [cmd] = args.as_slice()
        && cmd == "update"
    {
        return update();
    }
    let check = UpdateCheck::start();
    let code = run(&args);
    if let Some(check) = check {
        check.finish();
    }
    code
}

fn run(args: &[String]) -> ExitCode {
    let result = match args {
        [cmd, path] if cmd == "netlist" => netlist(Path::new(path)),
        [cmd, path, rest @ ..] if cmd == "parts" => parts(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "pinout" => pinout(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "voltages" => voltages(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "loads" => loads(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "stress" => stress(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "check" => check(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "bom" => bom(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "si" => si(Path::new(path), rest),
        [cmd, sub, rest @ ..] if cmd == "datasheet" => datasheet(sub, rest),
        [cmd, sub, mpn, rest @ ..] if cmd == "part" && sub == "show" && !mpn.starts_with("--") => part_show(mpn, rest),
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

fn part_show(mpn: &str, rest: &[String]) -> Result<bool, String> {
    let mut dirs = vec![];
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--parts", Some(d)) => dirs.push(PathBuf::from(d)),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    let lib = load_library(&dirs)?;
    if let Some(path) = lib.files.get(mpn) {
        match gerberus::limits::builtin_text(path)
            .map(str::to_string)
            .or_else(|| std::fs::read_to_string(path).ok())
        {
            Some(text) => print!("{text}"),
            None => println!("# {mpn} is in {}", path.display()),
        }
        return Ok(true);
    }
    for (path, family) in lib.families.iter().rev() {
        if let Some(text) = family.part_text(mpn) {
            let at = |e: String| format!("{}: {e}", path.display());
            let text = text.map_err(at)?;
            // The same checks `check` runs on it, so this never prints a part that `check` would reject.
            let part = family.part(mpn).expect("it decoded above").map_err(at)?;
            let problems = part.problems();
            if !problems.is_empty() {
                return Err(at(problems.join("; ")));
            }
            println!(
                "# Written by the {} family in {} for {mpn}.",
                family.family,
                path.display()
            );
            print!("{text}");
            return Ok(true);
        }
    }
    Err(format!("no part file or part family has {mpn:?}"))
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

/// The once-a-day check for a newer release, running beside the command.
struct UpdateCheck {
    /// The remembered latest release, to compare with this build once the command is done.
    remembered: Option<String>,
    /// GitHub's answer, when the remembered one was too old and GitHub is being asked again.
    fresh: Option<std::sync::mpsc::Receiver<Option<String>>>,
    started: std::time::Instant,
}

impl UpdateCheck {
    fn start() -> Option<UpdateCheck> {
        use gerberus::update;
        use std::io::IsTerminal;
        let off = |v: &str| std::env::var_os(v).is_some_and(|x| !x.is_empty() && x != "0");
        if off(update::OPT_OUT) || off("CI") || !std::io::stderr().is_terminal() || !std::io::stdout().is_terminal() {
            return None;
        }
        let dir = update::default_cache_dir()?;
        let (remembered, stale) = update::remembered(&dir);
        let fresh = stale.then(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            // Mark today's check as done before asking, so a slow or unreachable GitHub (whose answer this run
            // may not wait for) is asked once a day, not on every run. A failed answer keeps the last tag.
            let _ = update::remember(&dir, remembered.as_deref().unwrap_or(""));
            std::thread::spawn(move || {
                if let Ok(tag) = update::fetch_latest() {
                    let _ = update::remember(&dir, tag.as_deref().unwrap_or(""));
                    let _ = tx.send(tag);
                }
            });
            rx
        });
        Some(UpdateCheck {
            remembered,
            fresh,
            started: std::time::Instant::now(),
        })
    }

    /// Prints a notice when a newer release is out, waiting for GitHub at most `update::WAIT` more.
    fn finish(self) {
        use gerberus::update;
        let fresh = self.fresh.and_then(|rx| {
            let left = update::WAIT.saturating_sub(self.started.elapsed());
            rx.recv_timeout(left).ok().flatten()
        });
        let Some(latest) = fresh.or(self.remembered) else {
            return;
        };
        let has_updater = std::env::current_exe()
            .ok()
            .and_then(|exe| update::updater_next_to(&exe))
            .is_some();
        if let Some(n) = update::notice(&latest, env!("CARGO_PKG_VERSION"), has_updater) {
            eprintln!("\n{n}");
        }
    }
}

/// `gerberus update`: runs `gerberus-update` from beside this binary, where the installers put it.
fn update() -> ExitCode {
    let exe = std::env::current_exe().ok();
    let Some(updater) = exe.as_deref().and_then(gerberus::update::updater_next_to) else {
        eprintln!(
            "error: gerberus-update isn't installed next to {}, so this copy of gerberus wasn't installed by its \
             installer. Install the latest release as described at https://github.com/{}#install, or rebuild it \
             the way you built it.",
            exe.as_deref()
                .map_or_else(|| "gerberus".to_string(), |e| e.display().to_string()),
            gerberus::update::REPO
        );
        return ExitCode::from(2);
    };
    // Windows won't let the installer overwrite this running gerberus.exe, but it does let it be renamed, so
    // move it aside for the installer and put it back if the update fails.
    let aside = match exe.as_deref().filter(|_| cfg!(windows)) {
        Some(exe) => match gerberus::update::set_aside(exe) {
            Ok(aside) => Some((exe, aside)),
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        },
        None => None,
    };
    let (code, ok) = match std::process::Command::new(&updater).status() {
        Ok(s) if s.success() => (ExitCode::SUCCESS, true),
        Ok(s) => {
            eprintln!("error: {} exited with {s}", updater.display());
            (ExitCode::from(1), false)
        }
        Err(e) => {
            eprintln!("error: {}: {e}", updater.display());
            (ExitCode::from(2), false)
        }
    };
    if let Some((exe, aside)) = aside
        && !ok
        && let Err(e) = gerberus::update::restore(exe, &aside)
    {
        eprintln!("error: {e}");
    }
    code
}

/// Command-line options shared by the commands that read part files.
struct Options {
    /// A `parts/` folder next to the schematic, then any `--parts` folders, read after the built-in library.
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
    let mut dirs: Vec<PathBuf> = vec![];
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

/// The built-in shared library, then each folder in `dirs`, a later file replacing an earlier one.
fn load_library(dirs: &[PathBuf]) -> Result<Library, String> {
    let mut lib = Library::builtin()?;
    for dir in dirs {
        lib.add_dir(dir)?;
    }
    Ok(lib)
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
    let mut lib = load_library(&o.dirs)?;
    let design = gerberus::load(path)?;
    lib.expand_families(&design);
    let inputs_path = o
        .inputs
        .clone()
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("inputs.toml"));
    // Without an inputs.toml next to the design, run anyway with no inputs, and say what that leaves out.
    let inputs = if o.inputs.is_none() && !inputs_path.exists() {
        let inputs = BoardInputs::none(&path.display().to_string(), &design);
        let ground = match inputs.inputs.first() {
            Some(i) => format!(
                "{} is taken as 0 V",
                design.nets[design.components[&i.pin.reference].pins[&i.pin.pin].net].name
            ),
            None => "no net is named GND, 0V or VSS, so not even ground is known".into(),
        };
        eprintln!(
            "note: no {} found, so nothing is known about what plugs into the connectors: {ground}, every net \
             only a connector reaches is UNKNOWN, and reverse-polarity, esd, fault cases and `stress` are \
             skipped.\n\
             help: write an inputs.toml next to the schematic giving each connector pin's voltage range (see \
             \"Board inputs\" in parts/README.md)",
            inputs_path.display()
        );
        inputs
    } else {
        let inputs = BoardInputs::load(&inputs_path)?;
        let problems = inputs.problems(&design);
        if !problems.is_empty() {
            return Err(format!("{}: {}", inputs_path.display(), problems.join("; ")));
        }
        inputs
    };
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
    let (show, rest) = Show::parse(rest)?;
    let json = rest.iter().any(|a| a == "--json");
    let rest: Vec<String> = rest.into_iter().filter(|a| a != "--json").collect();
    let o = options(path, &rest, &["--inputs"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::builtin()?;
    let report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::rules::render(&design, &v, &report, &show));
    }
    Ok(report.errors() == 0)
}

fn stress(path: &Path, rest: &[String]) -> Result<bool, String> {
    let (show, rest) = Show::parse(rest)?;
    let mut json = false;
    let mut runs = None;
    let mut seed = 1;
    let mut netlist = None;
    let mut others = vec![];
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--runs" => {
                runs = Some(
                    it.next()
                        .and_then(|n| n.parse::<usize>().ok())
                        .ok_or("--runs takes a number")?,
                );
            }
            "--seed" => {
                seed = it
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .ok_or("--seed takes a number")?;
            }
            "--netlist" => netlist = Some(PathBuf::from(it.next().ok_or("--netlist takes a file")?)),
            _ => others.push(a.clone()),
        }
    }
    let o = options(path, &others, &["--inputs"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::builtin()?;
    let mut sp = policy.spice.clone().unwrap_or_default();
    if let Some(n) = runs {
        sp.runs = n;
    }
    if let Some(file) = netlist {
        let circuit = gerberus::spice::build(&design, &lib, &inputs, &v)?;
        let pairs = circuit.pairs(&design, &lib);
        let (text, _) = circuit.netlist(&sp, &pairs, None);
        std::fs::write(&file, text).map_err(|e| format!("{}: {e}", file.display()))?;
    }
    let sweep = gerberus::spice::sweep(&design, &lib, &inputs, &v, &sp, seed)?;
    let report = gerberus::rules::Report {
        findings: gerberus::spice::check(&design, &lib, &sweep, &sp),
        cases: vec![],
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::spice::describe(&design, &sweep));
        print!("{}", gerberus::rules::render(&design, &v, &report, &show));
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
    let mut lib = load_library(&options(path, rest, &[])?.dirs)?;
    let design = gerberus::load(path)?;
    lib.expand_families(&design);
    let cov = coverage(&design, &lib);
    let (mut sheet, mut default, mut unchecked, mut bare) = (0, 0, 0, 0);
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
            Coverage::NonElectrical(what) => {
                bare += 1;
                println!("{r}: nothing to check ({what})");
            }
        }
    }
    println!(
        "{sheet} with datasheet limits, {default} with class defaults, {unchecked} unchecked, {bare} with nothing to check"
    );
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
    let (show, rest) = Show::parse(rest)?;
    let opts = options(path, &rest, &["--pcb"])?;
    let mut lib = load_library(&opts.dirs)?;
    let design = gerberus::kicad::load(path)?;
    lib.expand_families(&design);
    let pcb = opts
        .pcb
        .or_else(|| Some(path.with_extension("kicad_pcb")).filter(|p| p.is_file()));
    let footprints = pcb.as_deref().map(gerberus::kicad::pcb::load_footprints).transpose()?;
    match &pcb {
        Some(p) => println!("board: {}", p.display()),
        None => println!("board: none found next to the schematic, so footprints are not checked"),
    }
    let report = pinout::check(&design, footprints.as_deref(), &lib);
    println!();
    let mut out = String::new();
    let mut sorted: Vec<&pinout::Finding> = report.findings.iter().collect();
    sorted.sort_by_key(|f| f.severity);
    for f in sorted {
        let level = match f.severity {
            pinout::Severity::Error => Level::Error,
            pinout::Severity::Warning => Level::Warning,
        };
        let mut d = Diagnostic::from_message(level, "pinout", &[&f.reference], &f.message);
        let mpn = design.components.get(&f.reference).map(gerberus::limits::component_mpn);
        d.location = Some(match mpn {
            Some(m) if !m.is_empty() => format!("{} ({m})", f.reference),
            _ => f.reference.clone(),
        });
        d.render(&mut out, show.style, show.verbose, show.columns());
    }
    print!("{out}");
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
    let style = show.style;
    let (word, color) = match (report.errors(), report.warnings()) {
        (0, 0) => ("ok", "1;32"),
        (0, _) => ("warning", "1;33"),
        _ => ("error", "1;31"),
    };
    println!(
        "{}{} {}",
        style.paint(color, word),
        style.paint("1", ":"),
        style.paint(
            "1",
            &format!(
                "{}, {}",
                count(report.errors(), "error"),
                count(report.warnings(), "warning")
            )
        )
    );
    let eq = style.paint("1;34", "=");
    let note = style.paint("1", "note");
    println!(
        "   {eq} {note}: compared {all} parts with symbol, footprint and datasheet; {fp_only} symbol and \
         footprint only; {sheet_only} symbol and datasheet only; {none} not compared"
    );
    if !no_sheet.is_empty() {
        println!(
            "   {eq} {note}: no part file, so no datasheet pinout: {}",
            no_sheet.join(" ")
        );
    }
    for w in &design.warnings {
        eprintln!("warning: {w}");
    }
    Ok(report.errors() == 0)
}

fn count(n: usize, what: &str) -> String {
    gerberus::diag::plural(n, what)
}

fn si(path: &Path, rest: &[String]) -> Result<bool, String> {
    let (show, rest) = Show::parse(rest)?;
    let json = rest.iter().any(|a| a == "--json");
    let mut plots = None;
    let mut others = Vec::new();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => {}
            "--plots" => plots = Some(PathBuf::from(it.next().ok_or("--plots needs a folder")?)),
            _ => others.push(a.clone()),
        }
    }
    let o = options(path, &others, &[])?;
    let lib = load_library(&o.dirs)?;
    let layout = gerberus::kicad::pcb::load_layout(path)?;
    let report = gerberus::si::check(&layout, &lib);
    if let Some(dir) = plots {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for p in &report.pairs {
            let safe: String = p
                .name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            for (kind, svg) in [("eye", gerberus::si::svg::eye(p)), ("tdr", gerberus::si::svg::tdr(p))] {
                if let Some(svg) = svg {
                    let file = dir.join(format!("{safe}-{kind}.svg"));
                    std::fs::write(&file, svg).map_err(|e| format!("{}: {e}", file.display()))?;
                    eprintln!("wrote {}", file.display());
                }
            }
        }
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::si::render(&report, &show));
    }
    Ok(report.errors() == 0)
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
