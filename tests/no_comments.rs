use std::path::{Path, PathBuf};

const TOOL_DIRECTIVES: [&str; 3] = ["# shellcheck ", "# zizmor:", "#!"];

const CONFIG_FILES: [&str; 10] = [
    "Cargo.toml",
    "rustfmt.toml",
    "clippy.toml",
    "deny.toml",
    "dist-workspace.toml",
    "rust-toolchain.toml",
    "action.yml",
    ".github/dependabot.yml",
    ".github/workflows/ci.yml",
    ".githooks/pre-commit",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() && path.file_name().is_some_and(|n| n != "fixtures") {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn starts_raw_string(rest: &[u8]) -> Option<usize> {
    let after_prefix = match rest {
        [b'b', b'r', ..] => 2,
        [b'r', ..] => 1,
        _ => return None,
    };
    let hashes = rest[after_prefix..].iter().take_while(|&&b| b == b'#').count();
    (rest.get(after_prefix + hashes) == Some(&b'"')).then_some(hashes)
}

fn skip_raw_string(bytes: &[u8], start: usize, hashes: usize) -> usize {
    let open_quote = start + bytes[start..].iter().position(|&b| b == b'"').unwrap();
    let closing: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
    let body = &bytes[open_quote + 1..];
    open_quote + 1 + body.windows(closing.len()).position(|w| w == closing).unwrap() + closing.len()
}

fn skip_string(bytes: &[u8], open_quote: usize) -> usize {
    let mut i = open_quote + 1;
    while bytes[i] != b'"' {
        i += if bytes[i] == b'\\' { 2 } else { 1 };
    }
    i + 1
}

fn skip_char_literal_or_lifetime(text: &str, quote: usize) -> usize {
    let bytes = text.as_bytes();
    if bytes.get(quote + 1) == Some(&b'\\') {
        return quote + 3 + bytes[quote + 3..].iter().position(|&b| b == b'\'').unwrap() + 1;
    }
    let char_len = text[quote + 1..].chars().next().map_or(1, char::len_utf8);
    if bytes.get(quote + 1 + char_len) == Some(&b'\'') {
        quote + 2 + char_len
    } else {
        quote + 1
    }
}

fn skip_block_comment(bytes: &[u8], start: usize) -> usize {
    let mut depth = 0;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if bytes[i..].starts_with(b"*/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += 1;
        }
    }
    i
}

fn comment_lines_in_rust(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let line_of = |i: usize| text[..i].matches('\n').count() + 1;
    let mut found = vec![];
    let mut i = 0;
    while i < bytes.len() {
        let follows_identifier = i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
        let rest = &bytes[i..];
        if rest.starts_with(b"//") {
            found.push(line_of(i));
            i += rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
        } else if rest.starts_with(b"/*") {
            found.push(line_of(i));
            i = skip_block_comment(bytes, i);
        } else if let Some(hashes) = starts_raw_string(rest).filter(|_| !follows_identifier) {
            i = skip_raw_string(bytes, i, hashes);
        } else if rest.starts_with(b"b\"") && !follows_identifier {
            i = skip_string(bytes, i + 1);
        } else if bytes[i] == b'"' {
            i = skip_string(bytes, i);
        } else if bytes[i] == b'\'' {
            i = skip_char_literal_or_lifetime(text, i);
        } else {
            i += 1;
        }
    }
    found
}

fn comment_lines_in_config(text: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim_start();
            trimmed.starts_with('#') && !TOOL_DIRECTIVES.iter().any(|d| trimmed.starts_with(d))
        })
        .map(|(index, _)| index + 1)
        .collect()
}

#[test]
fn rust_code_has_no_comments() {
    let root = repo_root();
    let mut files = vec![root.join("build.rs")];
    rust_sources(&root.join("src"), &mut files);
    rust_sources(&root.join("tests"), &mut files);
    let offenders: Vec<String> = files
        .iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(path).unwrap();
            let shown = path.strip_prefix(&root).unwrap().display().to_string();
            comment_lines_in_rust(&text)
                .into_iter()
                .map(move |line| format!("{shown}:{line}"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "comments are not allowed; rename or restructure the code instead:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn tooling_config_has_no_comments() {
    let root = repo_root();
    let offenders: Vec<String> = CONFIG_FILES
        .iter()
        .flat_map(|name| {
            let text = std::fs::read_to_string(root.join(name)).unwrap();
            comment_lines_in_config(&text)
                .into_iter()
                .map(move |line| format!("{name}:{line}"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "comments are not allowed in tooling config:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn detector_finds_comments_but_not_comment_markers_inside_literals() {
    let code = "let url = \"https://x\";\nlet raw = r#\"// not\"#;\nlet c = '/';\nfn f<'a>() {}\nlet x = 1; // yes\n/* yes */\n";
    assert_eq!(comment_lines_in_rust(code), [5, 6]);
}

#[test]
fn detector_handles_escaped_quotes_and_multi_line_block_comments() {
    let code = "let v = ['\\'','\"']; let s = \"// a\";\n/* a\n \" b /* c */ */\nlet y = 2;\n";
    assert_eq!(comment_lines_in_rust(code), [2]);
}
