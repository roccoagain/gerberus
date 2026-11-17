// No `unwrap()` outside tests; see src/lib.rs.
#![warn(clippy::unwrap_used)]

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
  gerberus bringup <design> [--inputs FILE] [--parts DIR]...
  gerberus stress <design> [--inputs FILE] [--parts DIR]... [--runs N] [--seed N] [--netlist FILE] [REPORT]
  gerberus signal <design> [--inputs FILE] [--parts DIR]... [--json] [REPORT]
  gerberus check <design> [--inputs FILE] [--waivers FILE] [--parts DIR]... [REPORT]
  gerberus init <design> [--parts DIR]... [--stdout] [--force]
  gerberus diff <old design> <new design> [--parts DIR]... [--json]
  gerberus diff <design> --rev REV [--parts DIR]... [--json]
  gerberus doctor <design> [--inputs FILE] [--waivers FILE] [--parts DIR]... [--json] [--color WHEN]
  gerberus bom <design> [--boards N] [--jlcpcb] [--from lcsc|mouser|digikey] [--offline] [--cache DIR] [--max-age HOURS] [REPORT]
  gerberus bom <design> --csv FILE [--boards N]
  gerberus traces <design.kicad_sch> [--pcb BOARD.kicad_pcb] [--inputs FILE] [--parts DIR]... [REPORT]
  gerberus si <board.kicad_pcb> [--parts DIR]... [REPORT] [--plots DIR]
  gerberus part show <MPN> [--parts DIR]... [--out FILE] [--force]
  gerberus part new <design> <REF or MPN> [--parts DIR]... [--stdout] [--force]
  gerberus datasheet verify <part.toml>... [--pdf FILE] [--cache DIR] [--all]
  gerberus datasheet blank <part.toml>
  gerberus datasheet compare <first.toml> <second.toml>
  gerberus mcp
  gerberus skill install [DIR] [--force]
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

`mcp` serves the commands to an AI agent over the Model Context Protocol, on stdin and stdout (see docs/mcp.md).

`traces` checks that every track on the KiCad board (same name as the schematic, .kicad_pcb, unless --pcb names
another) is wide enough for the worst-case current its net supplies, by IPC-2221 at policy.toml's temperature rise.

`diff` compares two revisions of a design, or one against a git revision with --rev: the parts added, removed or
changed, the connections made or broken (nets matched by their pins, not their names), and the `check` findings
whose verdict changed. It exits with status 1 when the new revision adds an error.

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
net got its range (with --verbose). It exits with status 1 when any error FAILs. A waivers.toml next to the
schematic (or --waivers FILE) accepts findings the designers have looked at, each with a reason: those aren't
counted, --all lists them, and a waiver that matches nothing is a warning. `doctor` reads it too.

`init` writes a starter inputs.toml next to the schematic: an entry for every connector pin it can guess from
the net's name (ground, USB VBUS, D+/D- and CC, a supply named by its voltage such as 12V or 3V3, a net the board
drives itself, a test point), each with a source that starts \"GUESS by gerberus init\", and a comment for each pin
it can't guess. `check` and `doctor` say how many guesses remain. It won't replace an inputs.toml unless --force;
--stdout prints it instead.

`doctor` lists what to add so `check` can settle its UNKNOWN results, most useful first: a part file for each
part number that has none (with the symbol's datasheet link), what plugs into each connector pin inputs.toml
doesn't describe (or inputs.toml itself), and limits a part file is missing. Each says how many unknown results
wait on it. --json prints the list as JSON.

`bom` looks up every BOM line by its LCSC part number: stock for --boards boards (default 10), LCSC's listing
status, and similar parts from other makers. Resistors and capacitors are checked by value, size and ratings
against LCSC's listing, and short or wrongly rated ones get checked LCSC equivalents. --jlcpcb is for a board
JLCPCB assembles: a part with no LCSC number is an error, and extended parts are counted. --from mouser or --from
digikey looks lines up by their MPN field instead, which gives the manufacturer's lifecycle (active, NRND,
obsolete) too; they need an API key in MOUSER_API_KEY, or DIGIKEY_CLIENT_ID and DIGIKEY_CLIENT_SECRET. Answers are
cached in --cache (default ~/.cache/gerberus/<distributor>) for --max-age hours (default 24); --offline uses only
the cache. It exits with status 1 when any line has an error. --csv FILE writes the BOM, with quantities for
--boards boards, as a CSV file for DigiKey's or Mouser's BOM upload, and looks nothing up.

`si` checks every USB 2.0 pair on a KiCad board (nets ending in D+/D-, DP/DM or _P/_N with USB in the name):
impedance from a 2D field solver run on the stackup, skew, vias, stubs, reference planes, length and the
capacitance of parts on the lines, then simulates each pair at 480 Mb/s and checks the eye at the connector
against the USB 2.0 Template 1 mask. --plots writes each pair's eye diagram and impedance profile (TDR) as SVG
files into DIR. It reads part capacitance from the same part folders as `parts`, matched by footprint value.
It exits with status 1 when any error FAILs.
`part show` prints the part file the library has for a part number: the file itself, or the one its part family
writes for it (see parts/README.md, \"Part families\"). --out FILE writes it to a file instead, making its folder
(`--out board/parts/<MPN>.toml` makes a copy that overrides the built-in one, to add to); it won't replace a file
unless --force. Run `datasheet verify` on it to check a family's values against its datasheet.

`part new` starts a part file for a part on the schematic, named by its reference (U2) or part number: the pin
map, class and datasheet link from the symbol, and every limit unknown until it is read from the datasheet. It
writes `parts/<MPN>.toml` next to the schematic, or prints it with --stdout. It won't replace a file, or write
one for a part the library, a part family or the symbol's own rating already covers, unless --force; and never
one for a part number another file in the board's part folders already defines.

`datasheet` checks part files against their datasheets (see the datasheet-to-part skill). `verify` downloads each
file's datasheet (cached in --cache, default ~/.cache/gerberus/datasheets), or reads --pdf, or a PDF saved in the
board's datasheets/ folder (beside parts/, named after the part number or the part file, with .pdf; read before
any download), and looks for every cited value on its page: in the right MIN/TYP/MAX column, in the right unit, on a row that names the pin. It
needs poppler's `pdftotext`. --all also lists confirmed values. It exits with status 1 when a value is missing
or mismatched. `blank` prints the part file with every datasheet value replaced by \"?\", for a second reader to
fill in blind; `compare` lists every value where two readings disagree and exits with status 1 if any do.

`skill install` writes the agent skills that ship with gerberus into DIR/.claude/skills (DIR defaults to the
current folder, the board's repository): check-board (init, doctor, part files, check) and datasheet-to-part
(reading a datasheet into a part file, verified and read twice). Run it again after `gerberus update` to bring
them up to date: it replaces the files it wrote, but not one someone has changed since, unless --force.

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
        [cmd, path, rest @ ..] if cmd == "bringup" => bringup(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "stress" => stress(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "signal" => signal(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "check" => check(Path::new(path), rest),
        [cmd] if cmd == "mcp" => mcp(),
        [cmd, path, rest @ ..] if cmd == "traces" => traces(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "diff" => diff(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "doctor" => doctor(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "init" => init(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "bom" => bom(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "si" => si(Path::new(path), rest),
        [cmd, sub, rest @ ..] if cmd == "datasheet" => datasheet(sub, rest),
        [cmd, sub, rest @ ..] if cmd == "skill" && sub == "install" => skill_install(rest),
        [cmd, sub, mpn, rest @ ..] if cmd == "part" && sub == "show" && !mpn.starts_with("--") => part_show(mpn, rest),
        [cmd, sub, path, which, rest @ ..] if cmd == "part" && sub == "new" && !which.starts_with("--") => {
            part_new(Path::new(path), which, rest)
        }
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

fn part_new(path: &Path, which: &str, rest: &[String]) -> Result<bool, String> {
    let (mut stdout, mut force) = (false, false);
    let mut extra = vec![];
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--stdout" => stdout = true,
            "--force" => force = true,
            "--parts" => extra.push(it.next().ok_or("--parts takes a folder")?.clone()),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    let design = gerberus::load(path)?;
    let source = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into());
    let (mpn, text) = gerberus::part_new::template(&design, which, &source)?;
    // What `check` would load, so the file is never one it rejects.
    let part: gerberus::limits::Part = toml::from_str(&text).map_err(|e| format!("the template doesn't parse: {e}"))?;
    let problems = part.problems();
    if !problems.is_empty() {
        return Err(format!("the template has problems: {}", problems.join("; ")));
    }
    let dir = path.parent().unwrap_or(Path::new(".")).join("parts");
    let file = dir.join(gerberus::doctor::file_name(&mpn));

    // A part the library already covers, by a file, a family or the schematic's own rating, doesn't need one
    // unless asked. A part file in the board's own folders under another name is never doubled: the library
    // would refuse to load with the part defined twice.
    let mut flags = vec![];
    for d in &extra {
        flags.push("--parts".to_string());
        flags.push(d.clone());
    }
    let o = options(path, &flags, &[])?;
    let mut lib = load_library(&o.dirs)?;
    if let Some(existing) = lib.files.get(&mpn).cloned()
        && !existing.to_string_lossy().ends_with("(built in)")
    {
        if existing != file {
            return Err(format!(
                "{mpn} is already in {}: edit that file instead, since a second file for it would stop the library \
                 loading",
                existing.display()
            ));
        }
        if !force && !stdout {
            return Err(format!("{} already exists; --force replaces it", file.display()));
        }
    }
    if file.exists() {
        let other = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| t.parse::<toml::Table>().ok())
            .and_then(|t| t.get("mpn").and_then(|m| m.as_str()).map(str::to_string));
        if other.as_deref().is_some_and(|m| m != mpn) {
            return Err(format!(
                "{} holds another part, {:?}; move it or write this one by hand under another name",
                file.display(),
                other.unwrap_or_default()
            ));
        }
        if !force && !stdout {
            return Err(format!("{} already exists; --force replaces it", file.display()));
        }
    }
    lib.expand_families(&design);
    let c = design
        .components
        .get(which)
        .or_else(|| {
            design
                .components
                .values()
                .find(|c| gerberus::limits::component_mpn(c) == mpn)
        })
        .expect("the template found it");
    if !force {
        match lib.lookup(c) {
            gerberus::limits::Lookup::Datasheet(_) if !file.exists() => {
                return Err(format!(
                    "{mpn} already has a part file: {} (`gerberus part show {mpn}` prints it); --force writes a \
                     new one anyway",
                    lib.files[&mpn].display()
                ));
            }
            gerberus::limits::Lookup::ClassDefault(_) => {
                return Err(format!(
                    "{} is already checked against the rating its schematic symbol gives (`gerberus parts` shows \
                     it); a part file would replace that. --force writes one anyway",
                    c.reference
                ));
            }
            _ => {}
        }
    }
    if stdout {
        print!("{text}");
        return Ok(true);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::write(&file, &text).map_err(|e| format!("{}: {e}", file.display()))?;
    println!("wrote {}", file.display());
    println!(
        "next: fill in each \"{}\" from the datasheet (parts/README.md), then run `gerberus datasheet verify {}`",
        gerberus::part_new::NOT_READ,
        file.display()
    );
    Ok(true)
}

fn skill_install(rest: &[String]) -> Result<bool, String> {
    let (mut dir, mut force) = (None, false);
    for a in rest {
        match a.as_str() {
            "--force" => force = true,
            d if !d.starts_with("--") && dir.is_none() => dir = Some(PathBuf::from(d)),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    for (path, outcome) in gerberus::skill::install(&dir, force)? {
        let what = match outcome {
            gerberus::skill::Outcome::Written => "wrote",
            gerberus::skill::Outcome::Updated => "updated",
            gerberus::skill::Outcome::Replaced => "replaced (--force)",
            gerberus::skill::Outcome::Unchanged => "up to date",
        };
        println!("{what}: {}", path.display());
    }
    println!(
        "an agent working in {} now has the check-board and datasheet-to-part skills",
        dir.display()
    );
    Ok(true)
}

fn part_show(mpn: &str, rest: &[String]) -> Result<bool, String> {
    let (mut dirs, mut out, mut force) = (vec![], None, false);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" => force = true,
            "--parts" => dirs.push(PathBuf::from(it.next().ok_or("--parts needs a folder")?)),
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a file")?)),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    let lib = load_library(&dirs)?;
    let text = part_text(&lib, mpn)?;
    let Some(out) = out else {
        print!("{text}");
        return Ok(true);
    };
    if out.exists() && !force {
        return Err(format!("{} already exists; --force replaces it", out.display()));
    }
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(true)
}

/// The part file the library has for `mpn`: the file itself, or the one its part family writes.
fn part_text(lib: &Library, mpn: &str) -> Result<String, String> {
    if let Some(path) = lib.files.get(mpn) {
        return Ok(gerberus::limits::builtin_text(path)
            .map(str::to_string)
            .or_else(|| std::fs::read_to_string(path).ok())
            .unwrap_or_else(|| format!("# {mpn} is in {}\n", path.display())));
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
            return Ok(format!(
                "# Written by the {} family in {} for {mpn}.\n{text}",
                family.family,
                path.display()
            ));
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
    /// Don't print the note about a missing inputs.toml: `doctor` lists it as a thing to add.
    quiet: bool,
    /// `--waivers`, for `check` and `doctor`.
    waivers: Option<PathBuf>,
}

/// Reads `--parts` and whichever of `--pcb`, `--inputs` and `--explain` the command takes.
fn options(path: &Path, rest: &[String], takes: &[&str]) -> Result<Options, String> {
    let mut dirs: Vec<PathBuf> = vec![];
    let local = path.parent().unwrap_or(Path::new(".")).join("parts");
    if local.is_dir() {
        dirs.push(local);
    }
    let (mut pcb, mut inputs, mut explain, mut fault, mut waivers) = (None, None, vec![], None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--parts", Some(dir)) => dirs.push(PathBuf::from(dir)),
            ("--pcb", Some(p)) if takes.contains(&"--pcb") => pcb = Some(PathBuf::from(p)),
            ("--inputs", Some(p)) if takes.contains(&"--inputs") => inputs = Some(PathBuf::from(p)),
            ("--explain", Some(n)) if takes.contains(&"--explain") => explain.push(n.clone()),
            ("--fault", Some(n)) if takes.contains(&"--fault") => fault = Some(n.clone()),
            ("--waivers", Some(p)) if takes.contains(&"--waivers") => waivers = Some(PathBuf::from(p)),
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    Ok(Options {
        dirs,
        pcb,
        inputs,
        explain,
        fault,
        quiet: false,
        waivers,
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
        if !o.quiet {
            eprintln!(
                "note: no {} found, so nothing is known about what plugs into the connectors: {ground}, every net \
             only a connector reaches is UNKNOWN, and reverse-polarity, esd, fault cases and `stress` are \
             skipped.\n\
             help: run `gerberus init {}` to start an inputs.toml next to the schematic, with each connector \
             pin's voltage range guessed from its net name for you to check (see \"Board inputs\" in \
             parts/README.md)",
                inputs_path.display(),
                path.display()
            );
        }
        inputs
    } else {
        let inputs = BoardInputs::load(&inputs_path)?;
        let problems = inputs.problems(&design);
        if !problems.is_empty() {
            return Err(format!("{}: {}", inputs_path.display(), problems.join("; ")));
        }
        let guesses = gerberus::init::guesses(&inputs);
        if !guesses.is_empty() && !o.quiet {
            eprintln!(
                "note: {} entries in {} are still guesses from `gerberus init` ({}), so results that rest on them \
                 are only as good as the guesses",
                guesses.len(),
                inputs_path.display(),
                guesses.join(", ")
            );
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
    let o = options(path, &rest, &["--inputs", "--waivers"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::builtin()?;
    let mut report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    waive(path, &o, &design, &mut report)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::rules::render(&design, &v, &report, &show));
    }
    Ok(report.errors() == 0)
}

/// Applies `--waivers`, or a `waivers.toml` next to the design when there is one.
fn waive(
    path: &Path,
    o: &Options,
    design: &gerberus::design::Design,
    report: &mut gerberus::rules::Report,
) -> Result<(), String> {
    let file = o
        .waivers
        .clone()
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("waivers.toml"));
    if o.waivers.is_none() && !file.exists() {
        return Ok(());
    }
    let waivers = gerberus::waivers::Waivers::load(&file)?;
    let problems = waivers.problems(design);
    if !problems.is_empty() {
        return Err(format!("{}: {}", file.display(), problems.join("; ")));
    }
    waivers.apply(report, &file.display().to_string());
    Ok(())
}

fn traces(path: &Path, rest: &[String]) -> Result<bool, String> {
    let (show, rest) = Show::parse(rest)?;
    let json = rest.iter().any(|a| a == "--json");
    let rest: Vec<String> = rest.into_iter().filter(|a| a != "--json").collect();
    let o = options(path, &rest, &["--pcb", "--inputs"])?;
    let pcb = o.pcb.clone().unwrap_or_else(|| path.with_extension("kicad_pcb"));
    let layout = gerberus::kicad::pcb::load_layout(&pcb)?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::builtin()?;
    let tp = policy.traces.ok_or("policy.toml has no [traces]")?;
    let th = gerberus::thermal::compute(&design, &lib, &inputs, &v);
    let report = gerberus::rules::Report {
        findings: gerberus::traces::check(&design, &layout, &th, &tp),
        ..Default::default()
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        for a in &layout.assumptions {
            eprintln!("note: {a}");
        }
        print!("{}", gerberus::rules::render(&design, &v, &report, &show));
    }
    Ok(report.errors() == 0)
}

fn mcp() -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| format!("can't find this program: {e}"))?;
    let stdin = std::io::stdin();
    gerberus::mcp::serve(&exe, stdin.lock(), std::io::stdout())?;
    Ok(true)
}

/// The design's check report, with its own inputs.toml, waivers.toml and parts/ folder.
fn report(path: &Path, rest: &[String]) -> Result<(gerberus::design::Design, gerberus::rules::Report), String> {
    let mut o = options(path, rest, &[])?;
    o.quiet = true;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::builtin()?;
    let mut report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    waive(path, &o, &design, &mut report)?;
    Ok((design, report))
}

fn diff(path: &Path, rest: &[String]) -> Result<bool, String> {
    let json = rest.iter().any(|a| a == "--json");
    let mut rest: Vec<String> = rest.iter().filter(|a| *a != "--json").cloned().collect();
    // A temporary copy of the old revision, removed when this returns.
    let mut _old_tree = None;
    let (old, new) = if let Some(i) = rest.iter().position(|a| a == "--rev") {
        let rev = rest.get(i + 1).ok_or("--rev takes a git revision")?.clone();
        rest.drain(i..=i + 1);
        let tree = gerberus::diff::checkout(path, &rev)?;
        if !tree.path.exists() {
            println!(
                "{} isn't in revision {rev}: it is a new design, so there is nothing to compare it with",
                path.display()
            );
            return Ok(true);
        }
        let old = tree.path.clone();
        _old_tree = Some(tree);
        (old, path.to_path_buf())
    } else {
        let Some(new) = rest.first().filter(|a| !a.starts_with("--")).cloned() else {
            return Err(format!("diff takes two designs, or one and --rev REV\n{USAGE}"));
        };
        rest.remove(0);
        (path.to_path_buf(), PathBuf::from(new))
    };
    let (od, or) = report(&old, &rest)?;
    let (nd, nr) = report(&new, &rest)?;
    let d = gerberus::diff::diff(&od, &or, &nd, &nr);
    if json {
        println!("{}", serde_json::to_string_pretty(&d).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::diff::render(&d));
    }
    Ok(d.new_errors() == 0)
}

fn init(path: &Path, rest: &[String]) -> Result<bool, String> {
    let (mut stdout, mut force) = (false, false);
    let mut others = vec![];
    for a in rest {
        match a.as_str() {
            "--stdout" => stdout = true,
            "--force" => force = true,
            _ => others.push(a.clone()),
        }
    }
    let o = options(path, &others, &[])?;
    let mut lib = load_library(&o.dirs)?;
    let design = gerberus::load(path)?;
    lib.expand_families(&design);
    let board = path
        .file_stem()
        .map_or_else(|| path.display().to_string(), |f| f.to_string_lossy().into());
    let text = gerberus::init::template(&design, &lib, &board);
    // What `check` would load, so the file is never one it rejects.
    let inputs: BoardInputs = toml::from_str(&text).map_err(|e| format!("the starter file doesn't parse: {e}"))?;
    let problems = inputs.problems(&design);
    if !problems.is_empty() {
        return Err(format!("the starter file has problems: {}", problems.join("; ")));
    }
    if stdout {
        print!("{text}");
        return Ok(true);
    }
    let file = path.parent().unwrap_or(Path::new(".")).join("inputs.toml");
    if file.exists() && !force {
        return Err(format!("{} already exists; --force replaces it", file.display()));
    }
    std::fs::write(&file, &text).map_err(|e| format!("{}: {e}", file.display()))?;
    let guesses = gerberus::init::guesses(&inputs);
    println!("wrote {} with {} guessed entries", file.display(), guesses.len());
    println!(
        "next: check each guess (its source starts \"{}\") and fill in the pins listed in comments, then run \
         `gerberus doctor {}`",
        gerberus::init::GUESS,
        path.display()
    );
    Ok(true)
}

fn doctor(path: &Path, rest: &[String]) -> Result<bool, String> {
    let (show, rest) = Show::parse(rest)?;
    let json = rest.iter().any(|a| a == "--json");
    let rest: Vec<String> = rest.into_iter().filter(|a| a != "--json").collect();
    let mut o = options(path, &rest, &["--inputs", "--waivers"])?;
    o.quiet = true;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let policy = gerberus::policy::Policy::builtin()?;
    let mut report = gerberus::rules::check(&design, &lib, &policy, &inputs, &v);
    waive(path, &o, &design, &mut report)?;
    let setup = gerberus::doctor::setup(path, o.inputs.as_deref());
    let d = gerberus::doctor::diagnose(&design, &lib, &setup, &inputs, &report, &v);
    if json {
        println!("{}", serde_json::to_string_pretty(&d).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::doctor::render(&d, &show));
    }
    Ok(true)
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
        waived: vec![],
        waivers_file: String::new(),
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

fn signal(path: &Path, rest: &[String]) -> Result<bool, String> {
    let (show, rest) = Show::parse(rest)?;
    let mut json = false;
    let mut others = vec![];
    for a in rest {
        match a.as_str() {
            "--json" => json = true,
            _ => others.push(a.clone()),
        }
    }
    let o = options(path, &others, &["--inputs"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let levels = gerberus::signal::levels(&design, &lib, &inputs, &v)?;
    let report = gerberus::rules::Report {
        findings: gerberus::signal::check(&design, &lib, &levels),
        waived: vec![],
        waivers_file: String::new(),
        cases: vec![],
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?);
    } else {
        print!("{}", gerberus::signal::describe(&design, &levels));
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

fn bringup(path: &Path, rest: &[String]) -> Result<bool, String> {
    let o = options(path, rest, &["--inputs"])?;
    let (design, lib, inputs, v) = analyse(path, &o)?;
    let th = gerberus::thermal::compute(&design, &lib, &inputs, &v);
    let plan = gerberus::bringup::plan(&design, &lib, &inputs, &v, &th);
    print!("{}", gerberus::bringup::render(&design, &plan));
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
    use gerberus::bom::{Api, Online, Options, Severity, check_with_parts};
    let (show, rest) = Show::parse(rest)?;
    let rest = &rest[..];
    let mut opts = Options::default();
    let mut parts: Vec<PathBuf> = vec![];
    let mut offline = false;
    let mut cache = None;
    let mut from = "lcsc".to_string();
    let mut csv = None;
    let mut max_age_hours: u64 = 24;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.clone().next()) {
            ("--offline", _) => offline = true,
            ("--jlcpcb", _) => opts.jlcpcb = true,
            ("--parts", Some(dir)) => {
                parts.push(PathBuf::from(dir));
                it.next();
            }
            ("--boards", Some(n)) => {
                opts.boards = n
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or(format!("--boards needs a positive number, not {n:?}"))?;
                it.next();
            }
            ("--from", Some(d)) => {
                from = d.to_ascii_lowercase();
                it.next();
            }
            ("--csv", Some(f)) => {
                csv = Some(PathBuf::from(f));
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
            ("--boards" | "--cache" | "--max-age" | "--from" | "--csv" | "--parts", None) => {
                return Err(format!("{a} needs a value\n{USAGE}"));
            }
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    let max_age = max_age_hours
        .checked_mul(3600)
        .map(std::time::Duration::from_secs)
        .ok_or(format!("--max-age {max_age_hours} is too large"))?;
    let design = gerberus::load(path)?;
    if let Some(file) = csv {
        std::fs::write(&file, gerberus::bom::order_csv(&design, opts.boards))
            .map_err(|e| format!("can't write {}: {e}", file.display()))?;
        println!(
            "wrote {} lines for {} boards to {}",
            gerberus::bom::lines(&design).len(),
            opts.boards,
            file.display()
        );
        return Ok(true);
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    // Offline, only the cache is read, so no key is needed.
    let key = |k: &str| -> Result<String, String> {
        match env(k) {
            Some(v) => Ok(v),
            None if offline => Ok(String::new()),
            None => Err(format!(
                "--from {from} needs an API key in {k}; see docs/bom.md for how to get one"
            )),
        }
    };
    let api = match from.as_str() {
        "lcsc" => Api::Lcsc,
        "mouser" => Api::Mouser {
            key: key("MOUSER_API_KEY")?,
        },
        "digikey" => Api::DigiKey {
            client_id: key("DIGIKEY_CLIENT_ID")?,
            client_secret: key("DIGIKEY_CLIENT_SECRET")?,
        },
        other => return Err(format!("--from takes lcsc, mouser or digikey, not {other:?}")),
    };
    // Answers are cached per part number, so each distributor needs its own folder: under --cache, Mouser's and
    // DigiKey's go in a subfolder named after them (LCSC's stay at the top, where earlier versions put them).
    let cache = match cache {
        Some(dir) if from != "lcsc" => Some(dir.join(&from)),
        Some(dir) => Some(dir),
        None => Online::cache_dir_for(&from),
    };
    let mut source = Online::with_api(api, cache, max_age, offline);
    // Part files give resistors' and capacitors' ratings, to compare with LCSC's listing.
    let local = path.parent().unwrap_or(Path::new(".")).join("parts");
    let mut dirs: Vec<PathBuf> = vec![];
    if local.is_dir() {
        dirs.push(local);
    }
    dirs.extend(parts);
    // A broken part file shouldn't stop a BOM check: without the library, ratings just aren't compared.
    let lib = match load_library(&dirs) {
        Ok(mut lib) => {
            lib.expand_families(&design);
            Some(lib)
        }
        Err(e) => {
            eprintln!("warning: part files not read, so resistor and capacitor ratings aren't compared: {e}");
            None
        }
    };
    let report = check_with_parts(&design, &mut source, opts, lib.as_ref());
    let source_name = gerberus::bom::Source::name(&source).to_string();
    print!("{}", gerberus::bom::render(&report, opts, &source_name, &show));
    Ok(report
        .iter()
        .all(|h| h.findings.iter().all(|f| f.severity != Severity::Error)))
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
                // A file `part new` started still has values nobody has read: say so, and don't pass it.
                if part.unread() {
                    ok = false;
                    println!(
                        "{}: values still say \"{}\": read them from the datasheet first",
                        part.mpn,
                        gerberus::part_new::NOT_READ
                    );
                }
                // A PDF saved in the board's datasheets/ folder comes before a download.
                let saved = || datasheet::saved_for(f, &part.mpn);
                let pdf = match (&pdf, &part.datasheet) {
                    (Some(p), _) => p.clone(),
                    (None, Some(_)) if saved().is_some() => {
                        let p = saved().unwrap_or_default();
                        println!("{}: reading the saved copy {}, not the url", part.mpn, p.display());
                        p
                    }
                    (None, Some(d)) if d.url.trim().is_empty() => {
                        return Err(format!(
                            "{}: [datasheet] has no url; fill it in, save the PDF as {}, or give it with --pdf",
                            f.display(),
                            datasheet::save_as(f, &part.mpn).display()
                        ));
                    }
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
