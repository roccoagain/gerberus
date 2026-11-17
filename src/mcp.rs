//! `gerberus mcp`: a Model Context Protocol server on stdin and stdout, so an AI agent can run the checks, read
//! net voltages and their reasons, and see what to add next, without parsing a shell's output.
//!
//! Each tool runs the `gerberus` command of the same name in a child process and returns what it prints, so the
//! tools behave exactly as the commands do. Messages are JSON-RPC 2.0, one per line.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

/// The protocol version answered when the client asks for one this server doesn't know.
const PROTOCOL: &str = "2025-06-18";

/// A tool: its name, what it does, its extra arguments, and how they become command-line flags.
struct Tool {
    name: &'static str,
    description: &'static str,
    /// `(argument, JSON type, description, flag)`. A boolean argument adds its flag when true; a string adds the
    /// flag and its value; an array of strings adds the flag once per item. An empty flag is a positional.
    args: &'static [(&'static str, &'static str, &'static str, &'static str)],
    /// The command's words before the design path.
    command: &'static [&'static str],
    /// The tool takes a `design` path.
    design: bool,
}

const REPORT: (&str, &str, &str, &str) = (
    "show",
    "string",
    "Which results to list besides errors and warnings: \"unknown\" adds UNKNOWNs, \"all\" adds UNKNOWNs and passes.",
    "",
);

const TOOLS: &[Tool] = &[
    Tool {
        name: "check",
        description: "Check every pin of a schematic against its datasheet limits at its net's worst-case voltage, \
                      plus TVS, capacitor voltage, resistor power, regulator headroom, junction temperature, ESD \
                      and reverse polarity. A result that can't be known is UNKNOWN, never a pass.",
        args: &[REPORT],
        command: &["check"],
        design: true,
    },
    Tool {
        name: "doctor",
        description: "List what to add to settle the most UNKNOWN results first: part files, inputs.toml entries, \
                      limits, each with the file to change and the command to run.",
        args: &[],
        command: &["doctor"],
        design: true,
    },
    Tool {
        name: "voltages",
        description: "The worst-case voltage range of every net. With `explain`, why each named net has its range, \
                      down to the inputs it comes from.",
        args: &[
            (
                "explain",
                "array",
                "Net names to explain, such as \"/SDA\" or \"+3V3\".",
                "--explain",
            ),
            (
                "fault",
                "string",
                "A fault case from inputs.toml to work out instead of normal operation.",
                "--fault",
            ),
        ],
        command: &["voltages"],
        design: true,
    },
    Tool {
        name: "loads",
        description: "The worst-case current each net supplies, and what draws it.",
        args: &[],
        command: &["loads"],
        design: true,
    },
    Tool {
        name: "signal",
        description: "Send the board's [[signal]]s from inputs.toml through it in ngspice: where an op-amp clips, \
                      and how far each IC pin a signal reaches swings against its limits.",
        args: &[REPORT],
        command: &["signal"],
        design: true,
    },
    Tool {
        name: "bringup",
        description: "A first-power checklist: which supply to connect first, the current limit to set, and the \
                      voltage to expect on each rail and where to probe it.",
        args: &[],
        command: &["bringup"],
        design: true,
    },
    Tool {
        name: "parts",
        description: "Where each part's limits come from: a datasheet part file, a class default, or nothing.",
        args: &[],
        command: &["parts"],
        design: true,
    },
    Tool {
        name: "netlist",
        description: "Every net and the part pins on it.",
        args: &[],
        command: &["netlist"],
        design: true,
    },
    Tool {
        name: "traces",
        description: "Check that every track on the KiCad board is wide enough for its net's worst-case current \
                      (IPC-2221).",
        args: &[REPORT],
        command: &["traces"],
        design: true,
    },
    Tool {
        name: "pinout",
        description: "Check that each part's symbol pins, footprint pads and datasheet pin table agree (KiCad).",
        args: &[],
        command: &["pinout"],
        design: true,
    },
    Tool {
        name: "diff",
        description: "What changed between two revisions of a design: parts, connections made or broken, and \
                      findings that are new or fixed. Give `new` for a second design, or `rev` for a git revision \
                      of the same one.",
        args: &[
            ("new", "string", "The new revision's design file.", ""),
            (
                "rev",
                "string",
                "A git revision (tag, branch, commit) to compare the design against.",
                "--rev",
            ),
        ],
        command: &["diff"],
        design: true,
    },
    Tool {
        name: "part_show",
        description: "The part file for a manufacturer part number, from the built-in library.",
        args: &[("mpn", "string", "The manufacturer part number.", "")],
        command: &["part", "show"],
        design: false,
    },
];

fn schema(t: &Tool) -> Value {
    let mut props = serde_json::Map::new();
    let mut required = vec![];
    if t.design {
        props.insert(
            "design".into(),
            json!({"type": "string", "description": "Path to a KiCad root .kicad_sch, an Altium .PrjPcb, or one .SchDoc."}),
        );
        required.push("design");
    }
    for &(name, ty, desc, flag) in t.args {
        let p = if ty == "array" {
            json!({"type": "array", "items": {"type": "string"}, "description": desc})
        } else if name == "show" {
            json!({"type": "string", "enum": ["unknown", "all"], "description": desc})
        } else {
            json!({"type": ty, "description": desc})
        };
        props.insert(name.into(), p);
        if flag.is_empty() && name == "mpn" {
            required.push(name);
        }
    }
    json!({"type": "object", "properties": props, "required": required})
}

/// The command-line arguments for a call, or what is wrong with it.
fn argv(t: &Tool, args: &Value) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = t.command.iter().map(|s| (*s).to_string()).collect();
    let get = |k: &str| args.get(k).filter(|v| !v.is_null());
    if t.design {
        let d = get("design").and_then(Value::as_str).ok_or("`design` is required")?;
        out.push(d.into());
    }
    for &(name, ty, _, flag) in t.args {
        let Some(v) = get(name) else { continue };
        if name == "show" {
            match v.as_str() {
                Some("unknown") => out.push("--unknown".into()),
                Some("all") => out.push("--all".into()),
                _ => return Err("`show` is \"unknown\" or \"all\"".into()),
            }
            continue;
        }
        let values: Vec<String> = if ty == "array" {
            v.as_array()
                .ok_or(format!("`{name}` is a list of strings"))?
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(String::from)
                        .ok_or(format!("`{name}` is a list of strings"))
                })
                .collect::<Result<_, _>>()?
        } else {
            vec![v.as_str().ok_or(format!("`{name}` is a string"))?.to_string()]
        };
        for x in values {
            if x.starts_with('-') {
                return Err(format!("`{name}` can't start with '-'"));
            }
            if !flag.is_empty() {
                out.push(flag.into());
            }
            out.push(x);
        }
    }
    if t.name == "check" {
        out.extend(["--color".into(), "never".into()]);
    }
    Ok(out)
}

/// Runs a tool and returns the MCP result: the command's output, and whether it failed.
fn call(exe: &Path, t: &Tool, args: &Value) -> Value {
    let argv = match argv(t, args) {
        Ok(a) => a,
        Err(e) => return json!({"content": [{"type": "text", "text": e}], "isError": true}),
    };
    let out = match Command::new(exe).args(&argv).env("NO_COLOR", "1").output() {
        Ok(o) => o,
        Err(e) => {
            return json!({"content": [{"type": "text", "text": format!("can't run gerberus: {e}")}], "isError": true});
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut text = stdout.trim_end().to_string();
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(stderr.trim_end());
    }
    // Status 1 is a finding (a check that failed), not a broken call; 2 is an error.
    let code = out.status.code();
    if code == Some(1) {
        text.push_str("\n\n(exit status 1: a check failed with an error)");
    }
    json!({"content": [{"type": "text", "text": text}], "isError": code.is_none_or(|c| c > 1)})
}

/// The answer to one message, or none for a notification.
pub fn handle(exe: &Path, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let ok = |result: Value| Some(json!({"jsonrpc": "2.0", "id": id.clone(), "result": result}));
    let err = |code: i64, message: String| {
        Some(json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": code, "message": message}}))
    };
    id.as_ref()?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL)
                .to_string();
            ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "gerberus", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Gerberus checks PCB schematics against datasheet limits. Start with `doctor` on a \
                                 design to see what is missing, then `check`. UNKNOWN means not checked, never \
                                 passed; don't invent datasheet values to settle one.",
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({
            "tools": TOOLS
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": schema(t)}))
                .collect::<Vec<_>>()
        })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let Some(t) = TOOLS.iter().find(|t| t.name == name) else {
                return err(-32602, format!("no tool named {name:?}"));
            };
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            ok(call(exe, t, &args))
        }
        _ => err(-32601, format!("no method {method:?}")),
    }
}

/// Serves until `input` closes.
pub fn serve(exe: &Path, input: impl BufRead, mut output: impl Write) -> Result<(), String> {
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(exe, &msg),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": e.to_string()}})),
        };
        if let Some(r) = reply {
            writeln!(output, "{r}").map_err(|e| e.to_string())?;
            output.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
