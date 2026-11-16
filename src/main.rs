use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: gerberus netlist <design.kicad_sch>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, path] if cmd == "netlist" => match gerberus::kicad::load(&PathBuf::from(path)) {
            Ok(design) => {
                for net in &design.nets {
                    let pins: Vec<String> = net.pins.iter().map(|(r, p)| format!("{r}.{p}")).collect();
                    println!("{}: {}", net.name, pins.join(" "));
                }
                for w in &design.warnings {
                    eprintln!("warning: {w}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
