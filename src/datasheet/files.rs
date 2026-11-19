//! Where datasheet PDFs are kept and cached, and how they are fetched.
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where downloaded datasheets are kept: `$XDG_CACHE_HOME/gerberus/datasheets`, or
/// `~/.cache/gerberus/datasheets`.
pub fn default_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
    Some(base.join("gerberus").join("datasheets"))
}

/// FNV-1a: stable across Rust versions, unlike the standard hasher.
fn fnv1a_hash_stable_across_rust_versions(text: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    text.bytes()
        .fold(FNV_OFFSET_BASIS, |h, b| (h ^ u64::from(b)).wrapping_mul(FNV_PRIME))
}

/// A file name for a URL that is the same every run.
fn cache_name(url: &str) -> String {
    let h = fnv1a_hash_stable_across_rust_versions(url);
    let tail: String = url
        .rsplit('/')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .collect();
    format!("{h:016x}-{tail}")
}

/// The PDF link in an LCSC datasheet page (`https://www.lcsc.com/datasheet/C1525.pdf` serves HTML that
/// points at the real file).
pub fn lcsc_pdf_link(html: &str) -> Option<String> {
    for host in ["https://datasheet.lcsc.com/", "https://wmsc.lcsc.com/"] {
        if let Some(s) = html.find(host) {
            let rest = &html[s..];
            let end = rest.find(['"', '\'', ' ', '<', '>'])?;
            // `...pdf?productCode=C1525`: the query only names the part.
            let link = rest[..end].split('?').next().unwrap_or("");
            if link.to_ascii_lowercase().ends_with(".pdf") {
                return Some(link.to_string());
            }
        }
    }
    None
}

/// The folder a board keeps its datasheet PDFs in, beside its `parts/` folder. `datasheet verify` reads a part's
/// PDF from there before it downloads anything, and `doctor` says what name to save each one under.
const FOLDER: &str = "datasheets";

/// The name a part's datasheet is saved under: the part number, cleaned as a part file's name is, with `.pdf`.
pub fn saved_name(mpn: &str) -> String {
    let toml = file_name(mpn);
    format!("{}.pdf", toml.strip_suffix(".toml").unwrap_or(&toml))
}

/// The first of `names` saved in `dir`: an exact match first, then one in any letter case.
fn saved_in(dir: &Path, names: &[String]) -> Option<PathBuf> {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    let name = |p: &PathBuf| p.file_name().and_then(|n| n.to_str()).map(str::to_string);
    names.iter().find_map(|want| {
        files
            .iter()
            .find(|p| name(p).as_deref() == Some(want.as_str()))
            .or_else(|| {
                files
                    .iter()
                    .find(|p| name(p).is_some_and(|n| n.eq_ignore_ascii_case(want)))
            })
            .cloned()
    })
}

/// The folder datasheets go in for part files kept in `parts`: `datasheets/` beside it. A folder named only by
/// `.` or `..` is resolved first, so `parts/` itself still finds the board's folder.
fn folder_beside(parts: &Path) -> PathBuf {
    let named = matches!(parts.components().next_back(), Some(std::path::Component::Normal(_)));
    let parts = if named {
        parts.to_path_buf()
    } else {
        std::fs::canonicalize(if parts.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parts
        })
        .unwrap_or_else(|_| parts.to_path_buf())
    };
    match parts.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.join(FOLDER),
        _ => PathBuf::from(FOLDER),
    }
}

/// The datasheet saved for the part file at `part_file`, whose part number is `mpn`: named after the part number
/// or after the file, in `datasheets/` beside the file's folder or in a `datasheets/` inside it.
pub fn saved_for(part_file: &Path, mpn: &str) -> Option<PathBuf> {
    let dir = part_file.parent().unwrap_or(Path::new(""));
    let mut names = vec![saved_name(mpn)];
    if let Some(stem) = part_file.file_stem().and_then(|s| s.to_str()) {
        let by_file = format!("{stem}.pdf");
        if !names.contains(&by_file) {
            names.push(by_file);
        }
    }
    let inside = if dir.as_os_str().is_empty() {
        PathBuf::from(FOLDER)
    } else {
        dir.join(FOLDER)
    };
    saved_in(&folder_beside(dir), &names).or_else(|| saved_in(&inside, &names))
}

/// Where to save the datasheet for the part file at `part_file`: the name `verify` looks for first.
pub fn save_as(part_file: &Path, mpn: &str) -> PathBuf {
    folder_beside(part_file.parent().unwrap_or(Path::new(""))).join(saved_name(mpn))
}

/// Downloads a datasheet into `cache` (or returns the copy already there) and gives its path.
pub fn fetch(url: &str, cache: &Path) -> Result<PathBuf, String> {
    let path = cache.join(cache_name(url));
    if path.is_file() {
        return Ok(path);
    }
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(ureq::Proxy::try_from_env())
        .timeout_global(Some(Duration::from_secs(60)))
        .user_agent(concat!("gerberus/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let get = |u: &str| -> Result<Vec<u8>, String> {
        agent
            .get(u)
            .call()
            .map_err(|e| format!("downloading {u}: {e}"))?
            .body_mut()
            .with_config()
            .limit(64 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| format!("reading {u}: {e}"))
    };
    let mut bytes = get(url)?;
    if !bytes.starts_with(b"%PDF") {
        let html = String::from_utf8_lossy(&bytes);
        let link = lcsc_pdf_link(&html).ok_or_else(|| format!("{url} is not a PDF and links to none"))?;
        bytes = get(&link)?;
        if !bytes.starts_with(b"%PDF") {
            return Err(format!("{link} is not a PDF"));
        }
    }
    std::fs::create_dir_all(cache).map_err(|e| format!("{}: {e}", cache.display()))?;
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// A file name for a part number: characters a file name can't hold become `_`.
pub fn file_name(mpn: &str) -> String {
    let safe: String = mpn
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.+".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.toml")
}
