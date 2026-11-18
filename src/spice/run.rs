use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(120);

fn spawn() -> Result<Child, String> {
    let names: &[&str] = if cfg!(windows) {
        &["ngspice_con", "ngspice"]
    } else {
        &["ngspice"]
    };
    let mut last = None;
    for name in names {
        match Command::new(name)
            .arg("-b")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(e) => last = Some(e),
        }
    }
    let e = last.map_or_else(String::new, |e| e.to_string());
    Err(format!(
        "can't run ngspice ({e}); install it (apt install ngspice, brew install ngspice, or ngspice.sourceforge.io)"
    ))
}

fn wait(mut child: Child) -> Result<(Vec<u8>, Vec<u8>), String> {
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < TIMEOUT => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("ngspice didn't finish in {} s", TIMEOUT.as_secs()));
            }
            Err(e) => return Err(format!("ngspice didn't finish: {e}")),
        }
    }
    let join = |h: std::thread::JoinHandle<Vec<u8>>| h.join().unwrap_or_default();
    Ok((join(out), join(err)))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairResult {
    pub max: f64,
    pub min: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Measured {
    values: BTreeMap<String, f64>,
}

impl Measured {
    pub fn parse(output: &str) -> Measured {
        let mut values = BTreeMap::new();
        for line in output.lines() {
            let Some((name, rest)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                continue;
            }
            if let Some(v) = rest.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()) {
                values.insert(name.to_ascii_lowercase(), v);
            }
        }
        Measured { values }
    }

    pub fn get(&self, name: &str) -> Option<f64> {
        self.values.get(name).copied()
    }

    pub fn pair(&self, i: usize) -> Option<PairResult> {
        Some(PairResult {
            max: self.get(&format!("p{i}max"))?,
            min: self.get(&format!("p{i}min"))?,
            end: self.get(&format!("p{i}end"))?,
        })
    }
}

pub fn simulate(netlist: &str) -> Result<Measured, String> {
    let mut child = spawn()?;
    child
        .stdin
        .take()
        .ok_or("ngspice has no standard input")?
        .write_all(netlist.as_bytes())
        .map_err(|e| format!("can't write the netlist to ngspice: {e}"))?;
    let out = wait(child)?;
    let text = String::from_utf8_lossy(&out.0);
    let err = String::from_utf8_lossy(&out.1);
    let measured = Measured::parse(&text);
    let aborted = text.contains("simulation(s) aborted") || err.contains("simulation(s) aborted");
    if measured.values.is_empty() || aborted {
        let why: Vec<&str> = text
            .lines()
            .chain(err.lines())
            .filter(|l| {
                let l = l.to_ascii_lowercase();
                l.contains("error") || l.contains("timestep too small") || l.contains("aborted")
            })
            .take(5)
            .collect();
        let what = if aborted {
            "ngspice stopped"
        } else {
            "ngspice measured nothing"
        };
        return Err(format!("{what}: {}", why.join("; ")));
    }
    Ok(measured)
}
