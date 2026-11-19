//! The modules form layers: no module reaches, directly or through others, back to one that uses it. A loop
//! would tie the modules into one piece that can only be understood, tested or split out as a whole.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The top-level module a file belongs to: `src/si/geometry.rs` is in `si`, `src/pinout.rs` in `pinout`.
fn top_module(src: &Path, file: &Path) -> String {
    let first = file.strip_prefix(src).unwrap().components().next().unwrap();
    first.as_os_str().to_string_lossy().trim_end_matches(".rs").to_string()
}

/// Every top-level module each module names through `crate::` in its code (not its comments), leaving out itself.
fn uses() -> BTreeMap<String, BTreeSet<String>> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![];
    rust_sources(&src, &mut files);
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for file in files {
        let module = top_module(&src, &file);
        if module == "main" || module == "lib" {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let used = graph.entry(module.clone()).or_default();
        let code = text.lines().filter(|line| !line.trim_start().starts_with("//"));
        for line in code {
            for (at, _) in line.match_indices("crate::") {
                let name: String = line[at + "crate::".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                assert!(
                    !name.is_empty(),
                    "{}: name crate:: modules one at a time",
                    file.display()
                );
                if name != module {
                    used.insert(name);
                }
            }
        }
    }
    graph
}

/// A path that starts and ends at the same module, if there is one.
fn find_loop(graph: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    fn visit(
        graph: &BTreeMap<String, BTreeSet<String>>,
        module: &str,
        path: &mut Vec<String>,
        done: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if let Some(start) = path.iter().position(|m| m == module) {
            let mut cycle = path[start..].to_vec();
            cycle.push(module.to_string());
            return Some(cycle);
        }
        if done.contains(module) {
            return None;
        }
        path.push(module.to_string());
        for next in graph.get(module).into_iter().flatten() {
            if let Some(cycle) = visit(graph, next, path, done) {
                return Some(cycle);
            }
        }
        path.pop();
        done.insert(module.to_string());
        None
    }
    let mut done = BTreeSet::new();
    graph.keys().find_map(|m| visit(graph, m, &mut vec![], &mut done))
}

#[test]
fn modules_have_no_loops() {
    let graph = uses();
    assert!(graph.contains_key("rules") && graph["rules"].contains("voltage"));
    if let Some(cycle) = find_loop(&graph) {
        panic!("modules use each other in a loop: {}", cycle.join(" -> "));
    }
}

#[test]
fn a_loop_is_found() {
    let graph: BTreeMap<String, BTreeSet<String>> =
        [("a", vec!["b"]), ("b", vec!["c"]), ("c", vec!["a"]), ("d", vec![])]
            .into_iter()
            .map(|(m, used)| (m.to_string(), used.into_iter().map(String::from).collect()))
            .collect();
    assert_eq!(find_loop(&graph).unwrap(), ["a", "b", "c", "a"]);
}
