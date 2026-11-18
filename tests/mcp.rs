use std::path::Path;

use gerberus::mcp::{handle, serve};
use serde_json::{Value, json};

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_gerberus"))
}

fn kitchen_sink() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/kitchen-sink/kitchen-sink.kicad_sch")
        .display()
        .to_string()
}

#[test]
fn initialize_and_list_tools() {
    let r = handle(
        exe(),
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}}),
    )
    .unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert_eq!(r["result"]["serverInfo"]["name"], "gerberus");
    assert!(handle(exe(), &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
    let r = handle(exe(), &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
    let names: Vec<&str> = r["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for n in ["check", "doctor", "voltages", "diff", "part_show"] {
        assert!(names.contains(&n), "{names:?}");
    }
    let r = handle(exe(), &json!({"jsonrpc": "2.0", "id": 3, "method": "nope"})).unwrap();
    assert_eq!(r["error"]["code"], -32601);
}

#[test]
fn a_tool_call_runs_the_command() {
    let design = kitchen_sink();
    let r = handle(
        exe(),
        &json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": {"name": "voltages", "arguments": {"design": design}}}),
    )
    .unwrap();
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(r["result"]["isError"], false, "{text}");
    assert!(text.contains("GND: 0 V"), "{text}");
}

#[test]
fn bad_arguments_are_a_tool_error() {
    let r = handle(
        exe(),
        &json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "check", "arguments": {}}}),
    )
    .unwrap();
    assert_eq!(r["result"]["isError"], true);
    let r = handle(
        exe(),
        &json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
                "params": {"name": "voltages", "arguments": {"design": "--inputs"}}}),
    )
    .unwrap();
    assert_eq!(r["result"]["isError"], true);
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("can't start with")
    );
}

#[test]
fn serve_answers_line_by_line() {
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\nnot json\n";
    let mut out = vec![];
    serve(exe(), &input[..], &mut out).unwrap();
    let lines: Vec<Value> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["result"], json!({}));
    assert_eq!(lines[1]["error"]["code"], -32700);
}

#[test]
fn edge_cases_follow_json_rpc() {
    let r = handle(exe(), &json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}])).unwrap();
    assert_eq!(r["error"]["code"], -32600);
    let r = handle(exe(), &json!({"jsonrpc": "2.0", "id": 2})).unwrap();
    assert_eq!(r["error"]["code"], -32600);
    let r = handle(
        exe(),
        &json!({"jsonrpc": "2.0", "id": 3, "method": "initialize", "params": {"protocolVersion": "1999-01-01"}}),
    )
    .unwrap();
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
}

#[test]
fn a_negative_rail_can_be_explained() {
    let r = handle(
        exe(),
        &json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": {"name": "voltages", "arguments": {"design": kitchen_sink(), "explain": ["-12V"]}}}),
    )
    .unwrap();
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(!text.contains("can't start with"), "{text}");
}
