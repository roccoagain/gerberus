use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gerberus::inputs::BoardInputs;
use gerberus::limits::{Coverage, Library, coverage};

const USAGE: &str = "usage:
  gerberus netlist <design.kicad_sch>
  gerberus parts <design.kicad_sch> [--parts DIR]...
  gerberus voltages <design.kicad_sch> [--inputs FILE] [--parts DIR]... [--explain NET]...

`parts` reports where each part's limits come from. It reads the shared library, then a `parts/` folder next
to the schematic if there is one (its files override the shared ones), then any --parts folders in order.

`voltages` works out the worst-case voltage range of every net, starting from the board inputs (`inputs.toml`
next to the schematic unless --inputs is given). --explain prints the reasoning behind one net.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [cmd, path] if cmd == "netlist" => netlist(Path::new(path)),
        [cmd, path, rest @ ..] if cmd == "parts" => parts(Path::new(path), rest),
        [cmd, path, rest @ ..] if cmd == "voltages" => voltages(Path::new(path), rest),
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

/// Options shared by the commands that read part files.
#[derive(Default)]
struct Opts {
    parts: Vec<PathBuf>,
    inputs: Option<PathBuf>,
    explain: Vec<String>,
}

fn options(rest: &[String], allowed: &[&str]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            (flag, Some(v)) if allowed.contains(&flag) => match flag {
                "--parts" => o.parts.push(PathBuf::from(v)),
                "--inputs" => o.inputs = Some(PathBuf::from(v)),
                _ => o.explain.push(v.clone()),
            },
            _ => return Err(format!("unexpected argument {a:?}\n{USAGE}")),
        }
    }
    Ok(o)
}

/// The shared library, then a `parts/` folder next to the schematic, then any `--parts` folders.
fn library(path: &Path, extra: &[PathBuf]) -> Result<Library, String> {
    let mut dirs: Vec<PathBuf> = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("parts")];
    let local = path.parent().unwrap_or(Path::new(".")).join("parts");
    if local.is_dir() {
        dirs.push(local);
    }
    dirs.extend(extra.iter().cloned());
    Library::load_dirs(&dirs.iter().map(PathBuf::as_path).collect::<Vec<_>>())
}

fn voltages(path: &Path, rest: &[String]) -> Result<(), String> {
    let o = options(rest, &["--parts", "--inputs", "--explain"])?;
    let lib = library(path, &o.parts)?;
    let design = gerberus::kicad::load(path)?;
    let inputs_path = o
        .inputs
        .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")).join("inputs.toml"));
    let inputs = BoardInputs::load(&inputs_path)?;
    let problems = inputs.problems(&design);
    if !problems.is_empty() {
        return Err(format!("{}: {}", inputs_path.display(), problems.join("; ")));
    }
    let v = gerberus::voltage::compute(&design, &lib, &inputs);
    if o.explain.is_empty() {
        for (net, nv) in design.nets.iter().zip(&v.nets) {
            println!("{}: {}", net.name, nv.span);
        }
        return Ok(());
    }
    for name in &o.explain {
        let i = design
            .nets
            .iter()
            .position(|n| &n.name == name)
            .ok_or_else(|| format!("no net named {name:?}"))?;
        print!("{}", v.explain(&design, i));
    }
    Ok(())
}

fn parts(path: &Path, rest: &[String]) -> Result<(), String> {
    let o = options(rest, &["--parts"])?;
    let lib = library(path, &o.parts)?;
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
