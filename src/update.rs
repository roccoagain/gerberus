use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const REPO: &str = "roccoagain/gerberus";

pub const MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

pub const WAIT: Duration = Duration::from_secs(1);

pub const OPT_OUT: &str = "GERBERUS_NO_UPDATE_CHECK";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    numbers: [u64; 3],
    pre: Option<String>,
}

impl Version {
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

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(l), Some(c)) => l.is_newer_than(&c),
        _ => false,
    }
}

pub fn default_cache_dir() -> Option<PathBuf> {
    let var = |v: &str| std::env::var_os(v).filter(|x| !x.is_empty()).map(PathBuf::from);
    let local = if cfg!(windows) { var("LOCALAPPDATA") } else { None };
    let base = local
        .or_else(|| var("XDG_CACHE_HOME"))
        .or_else(|| var("HOME").map(|h| h.join(".cache")))
        .or_else(|| var("LOCALAPPDATA"))?;
    Some(base.join("gerberus"))
}

fn cache_file(dir: &Path) -> PathBuf {
    dir.join("latest-release")
}

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

pub fn remember(dir: &Path, tag: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!("latest-release.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{tag}\n")).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let file = cache_file(dir);
    std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display()))
}

pub fn fetch_latest() -> Result<Option<String>, String> {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .proxy(ureq::Proxy::try_from_env())
        .timeout_global(Some(Duration::from_secs(5)))
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

pub fn latest_tag(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(v.get("tag_name")?.as_str()?.to_string())
}

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

pub fn updater_next_to(exe: &Path) -> Option<PathBuf> {
    let path = exe
        .parent()?
        .join(format!("gerberus-update{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

pub fn set_aside(exe: &Path) -> Result<PathBuf, String> {
    let mut name = exe
        .file_name()
        .ok_or("the running binary has no file name")?
        .to_os_string();
    name.push(".old");
    let aside = exe.with_file_name(name);
    if aside.exists() {
        std::fs::remove_file(&aside).map_err(|e| format!("{}: {e}", aside.display()))?;
    }
    std::fs::rename(exe, &aside).map_err(|e| format!("moving {} aside: {e}", exe.display()))?;
    Ok(aside)
}

pub fn restore(exe: &Path, aside: &Path) -> Result<(), String> {
    if exe.exists() {
        return Ok(());
    }
    std::fs::rename(aside, exe).map_err(|e| format!("putting {} back: {e}", exe.display()))
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
    fn sets_the_binary_aside_and_restores_it() {
        let dir = std::env::temp_dir().join(format!("gerberus-aside-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("gerberus.exe");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(dir.join("gerberus.exe.old"), "older").unwrap();
        let aside = set_aside(&exe).unwrap();
        assert_eq!(aside, dir.join("gerberus.exe.old"));
        assert!(!exe.exists());
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "old");
        restore(&exe, &aside).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        let aside = set_aside(&exe).unwrap();
        std::fs::write(&exe, "new").unwrap();
        restore(&exe, &aside).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
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
