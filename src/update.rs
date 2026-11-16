//! Telling the user a newer release is out, and `gerberus update`.
//!
//! The check is at most once a day: it asks GitHub for the latest release, remembers the answer in the cache
//! folder, and waits for GitHub at most [`WAIT`] after the command itself has finished. Updating itself is done
//! by `gerberus-update`, which the installers put next to `gerberus` (cargo-dist's updater, axoupdater).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Where releases are published.
pub const REPO: &str = "roccoagain/gerberus";

/// How long a remembered answer is trusted before GitHub is asked again.
pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The longest the check may hold up a command that has already finished.
pub const WAIT: Duration = Duration::from_secs(1);

/// The environment variable that turns the check off.
pub const OPT_OUT: &str = "GERBERUS_NO_UPDATE_CHECK";

/// A version as `major.minor.patch`, with an optional pre-release (`-rc.1`), which sorts before the release.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    numbers: [u64; 3],
    pre: Option<String>,
}

impl Version {
    /// Reads `1.2.3`, `v1.2.3` or `1.2.3-rc.1`; build metadata (`+...`) is ignored.
    fn parse(text: &str) -> Option<Version> {
        let text = text.trim().trim_start_matches('v');
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_string())),
            None => (text, None),
        };
        let mut numbers = [0; 3];
        let mut parts = core.split('.');
        for n in &mut numbers {
            *n = parts.next()?.parse().ok()?;
        }
        parts.next().is_none().then_some(Version { numbers, pre })
    }

    fn is_newer_than(&self, other: &Version) -> bool {
        match self.numbers.cmp(&other.numbers) {
            std::cmp::Ordering::Equal => other.pre.is_some() && self.pre.is_none(),
            o => o.is_gt(),
        }
    }
}

/// Whether `latest` (a release tag such as `v0.2.0`) is a newer version than `current`. Anything that isn't a
/// version is never newer.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(l), Some(c)) => l.is_newer_than(&c),
        _ => false,
    }
}

/// The folder the latest release tag is remembered in: the user's cache folder, `gerberus` in it.
pub fn default_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))?;
    Some(base.join("gerberus"))
}

fn cache_file(dir: &Path) -> PathBuf {
    dir.join("latest-release")
}

/// The remembered latest release tag, and whether it is older than [`MAX_AGE`] (or missing), so GitHub
/// should be asked again.
pub fn remembered(dir: &Path) -> (Option<String>, bool) {
    let file = cache_file(dir);
    let tag = std::fs::read_to_string(&file)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let stale = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_none_or(|age| age > MAX_AGE);
    (tag, stale)
}

/// Remembers `tag` as the latest release. The file is replaced in one step, so a run that exits half way
/// through leaves the old answer, not half a new one.
pub fn remember(dir: &Path, tag: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!("latest-release.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{tag}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let file = cache_file(dir);
    std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display()))
}

/// Asks GitHub for the latest release's tag. `Ok(None)` when there is no release yet.
pub fn fetch_latest() -> Result<Option<String>, String> {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(ureq::Proxy::try_from_env())
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .user_agent(concat!("gerberus/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("{url}: {e}"))?;
    match response.status().as_u16() {
        404 => return Ok(None),
        200 => {}
        s => return Err(format!("{url}: HTTP {s}")),
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("{url}: {e}"))?;
    latest_tag(&body)
        .map(Some)
        .ok_or_else(|| format!("{url}: no tag_name in the answer"))
}

/// The `tag_name` of a GitHub release, from the API's JSON.
pub fn latest_tag(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(v.get("tag_name")?.as_str()?.to_string())
}

/// The line to print when `latest` is newer than this build, saying how to update.
pub fn notice(latest: &str, current: &str, has_updater: bool) -> Option<String> {
    if !is_newer(latest, current) {
        return None;
    }
    let how = if has_updater {
        "run `gerberus update`".to_string()
    } else {
        format!("see https://github.com/{REPO}#install")
    };
    Some(format!(
        "gerberus {} is out (you have {current}); {how}. Set {OPT_OUT}=1 to stop these checks.",
        latest.trim_start_matches('v')
    ))
}

/// `gerberus-update` (with `.exe` on Windows) in the folder `exe` is in, where the installers put it.
pub fn updater_next_to(exe: &Path) -> Option<PathBuf> {
    let path = exe
        .parent()?
        .join(format!("gerberus-update{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("v0.10.0", "0.9.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.2.0-rc.1"));
        assert!(!is_newer("v0.2.0-rc.1", "0.2.0"));
        assert!(!is_newer("v0.2.0+build.5", "0.2.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(!is_newer("v0.2", "0.1.0"));
        assert!(!is_newer("v0.2.0.1", "0.1.0"));
    }

    #[test]
    fn reads_tag_name() {
        assert_eq!(
            latest_tag(r#"{"tag_name": "v0.2.0", "name": "0.2.0"}"#).as_deref(),
            Some("v0.2.0")
        );
        assert_eq!(latest_tag(r#"{"message": "Not Found"}"#), None);
        assert_eq!(latest_tag("not json"), None);
    }

    #[test]
    fn notice_says_how_to_update() {
        let n = notice("v0.2.0", "0.1.0", true).unwrap();
        assert!(n.starts_with("gerberus 0.2.0 is out (you have 0.1.0); run `gerberus update`."));
        assert!(n.contains(OPT_OUT));
        let n = notice("v0.2.0", "0.1.0", false).unwrap();
        assert!(n.contains("https://github.com/roccoagain/gerberus#install"));
        assert_eq!(notice("v0.1.0", "0.1.0", true), None);
    }

    #[test]
    fn remembers_the_latest_tag() {
        let dir = std::env::temp_dir().join(format!("gerberus-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(remembered(&dir), (None, true));
        remember(&dir, "v0.2.0").unwrap();
        assert_eq!(remembered(&dir), (Some("v0.2.0".to_string()), false));
        remember(&dir, "v0.3.0").unwrap();
        assert_eq!(remembered(&dir).0.as_deref(), Some("v0.3.0"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_the_updater_beside_the_binary() {
        let dir = std::env::temp_dir().join(format!("gerberus-updater-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(format!("gerberus{}", std::env::consts::EXE_SUFFIX));
        assert_eq!(updater_next_to(&exe), None);
        let updater = dir.join(format!("gerberus-update{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&updater, "").unwrap();
        assert_eq!(updater_next_to(&exe), Some(updater));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
