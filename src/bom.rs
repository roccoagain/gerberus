use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::design::{Component, Design};
use crate::limits::{Library, component_mpn};
use crate::spec::{Spec, compare};

pub const LCSC_FIELDS: [&str; 5] = ["LCSC", "LCSC Part", "LCSC Part #", "JLCPCB Part #", "JLC"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub mpn: String,
    pub has_mpn_field: bool,
    pub lcsc: Option<String>,
    pub bad_lcsc: Option<String>,
    pub references: Vec<String>,
    pub commodity: bool,
}

pub fn lines(design: &Design) -> Vec<Line> {
    let mut by_key: BTreeMap<(String, Option<String>, Option<String>), Line> = BTreeMap::new();
    for (r, c) in &design.components {
        if c.dnp || !c.in_bom || !c.on_board {
            continue;
        }
        let mpn = component_mpn(c).to_string();
        let has_mpn_field = c.fields.get("MPN").is_some_and(|m| !m.trim().is_empty());
        let (lcsc, bad_lcsc) = match lcsc_field(c) {
            None => (None, None),
            Some(raw) => match normalize_lcsc(raw) {
                Some(code) => (Some(code), None),
                None => (None, Some(raw.to_string())),
            },
        };
        let line = by_key
            .entry((mpn.clone(), lcsc.clone(), bad_lcsc.clone()))
            .or_insert_with(|| Line {
                mpn,
                has_mpn_field: false,
                lcsc,
                bad_lcsc,
                references: Vec::new(),
                commodity: true,
            });
        line.has_mpn_field |= has_mpn_field;
        line.commodity &= is_commodity(c);
        line.references.push(r.clone());
    }
    let mut out: Vec<Line> = by_key.into_values().collect();
    for l in &mut out {
        l.references.sort_by(|a, b| natural_cmp(a, b));
    }
    out.sort_by(|a, b| natural_cmp(&a.references[0], &b.references[0]));
    out
}

const COMMODITY_SYMBOLS: [&str; 8] = [
    "Device:R",
    "Device:R_Small",
    "Device:R_US",
    "Device:R_Small_US",
    "Device:C",
    "Device:C_Small",
    "Device:C_US",
    "Device:C_Small_US",
];

fn is_commodity(c: &Component) -> bool {
    COMMODITY_SYMBOLS.contains(&c.lib_id.as_str())
}

fn lcsc_field(c: &Component) -> Option<&str> {
    LCSC_FIELDS
        .iter()
        .filter_map(|f| c.fields.get(*f))
        .map(|s| s.trim())
        .find(|s| !s.is_empty() && *s != "~")
}

pub fn normalize_lcsc(s: &str) -> Option<String> {
    let s = s.trim();
    let digits = s.strip_prefix('C').or_else(|| s.strip_prefix('c'))?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| format!("C{digits}"))
}

fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(s: &str) -> (&str, u64, &str) {
        let start = s.find(|c: char| c.is_ascii_digit()).unwrap_or(s.len());
        let end = s[start..]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(s.len(), |e| start + e);
        (&s[..start], s[start..end].parse().unwrap_or(0), &s[end..])
    }
    split(a).cmp(&split(b)).then_with(|| a.cmp(b))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub code: String,
    pub mpn: String,
    pub manufacturer: String,
    pub stock: u64,
    pub status: String,
    pub assembly: Option<String>,
    pub package: String,
    pub similar: Vec<Similar>,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Similar {
    pub code: String,
    pub mpn: String,
    pub manufacturer: String,
    pub stock: u64,
    pub status: String,
    pub package: String,
    pub description: String,
}

impl Listing {
    pub fn from_lcsc_json(json: &serde_json::Value) -> Result<Option<Listing>, String> {
        if json.get("ok").and_then(serde_json::Value::as_bool) != Some(true)
            || json.get("code").and_then(serde_json::Value::as_i64) != Some(200)
        {
            let msg = json.get("msg").and_then(|v| v.as_str()).unwrap_or("no message");
            return Err(format!("LCSC answered with an error: {msg}"));
        }
        let r = match json.get("result") {
            None | Some(serde_json::Value::Null) => return Ok(None),
            Some(r) => r,
        };
        let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        let n = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_u64).unwrap_or(0);
        let need = |k: &str| {
            let v = s(r, k);
            if v.is_empty() {
                Err(format!("LCSC's answer has no {k}"))
            } else {
                Ok(v)
            }
        };
        let similar = r
            .get("alternatePartList")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|p| Similar {
                        code: s(p, "productCode"),
                        mpn: s(p, "productModel"),
                        manufacturer: s(p, "brandNameEn"),
                        stock: n(p, "stockNumber"),
                        status: s(p, "productCycle"),
                        package: s(p, "encapStandard"),
                        description: s(p, "productIntroEn"),
                    })
                    .filter(|p| !p.code.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let params = r
            .get("paramVOList")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .map(|p| (s(p, "paramNameEn"), s(p, "paramValueEn")))
                    .filter(|(k, v)| !k.is_empty() && !v.is_empty() && v != "-")
                    .collect()
            })
            .unwrap_or_default();
        let assembly = match s(r, "smtLabel").as_str() {
            "BASE" => Some("basic".to_string()),
            "EXPAND" => Some("extended".to_string()),
            "" => None,
            other => Some(other.to_lowercase()),
        };
        Ok(Some(Listing {
            code: need("productCode")?,
            mpn: need("productModel")?,
            manufacturer: s(r, "brandNameEn"),
            stock: r
                .get("stockNumber")
                .and_then(serde_json::Value::as_u64)
                .ok_or("LCSC's answer has no stockNumber")?,
            status: need("productCycle")?,
            assembly,
            package: s(r, "encapStandard"),
            similar,
            params,
        }))
    }
}

pub trait Source {
    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String>;
    fn caveat(&self, _code: &str) -> Option<String> {
        None
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    fetched: u64,
    listing: Option<Listing>,
    #[serde(default)]
    version: u32,
}

const CACHE_VERSION: u32 = 2;

pub struct Online {
    pub cache: Option<PathBuf>,
    pub max_age: Duration,
    pub offline: bool,
    pub endpoint: String,
    agent: Option<ureq::Agent>,
    caveats: BTreeMap<String, String>,
}

const LCSC_DETAIL: &str = "https://wmsc.lcsc.com/ftps/wm/product/detail?productCode=";

impl Online {
    pub fn new(cache: Option<PathBuf>, max_age: Duration, offline: bool) -> Online {
        Online {
            endpoint: LCSC_DETAIL.to_string(),
            cache,
            max_age,
            offline,
            agent: None,
            caveats: BTreeMap::new(),
        }
    }

    pub fn default_cache_dir() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
        Some(base.join("gerberus").join("lcsc"))
    }

    fn cache_path(&self, code: &str) -> Option<PathBuf> {
        use std::fmt::Write;
        let mut name = String::new();
        for c in code.chars() {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                name.push(c);
            } else {
                let _ = write!(name, "_{:X}", u32::from(c));
            }
        }
        Some(self.cache.as_ref()?.join(format!("{name}.json")))
    }

    fn read_cache(&self, code: &str) -> Result<Option<CacheEntry>, String> {
        let Some(path) = self.cache_path(code) else {
            return Ok(None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("can't read cache file {}: {e}", path.display())),
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("cache file {} is damaged: {e}", path.display()))
    }

    fn write_cache(&self, code: &str, listing: Option<&Listing>) {
        let Some(path) = self.cache_path(code) else { return };
        let entry = CacheEntry {
            fetched: now(),
            listing: listing.cloned(),
            version: CACHE_VERSION,
        };
        let Ok(text) = serde_json::to_string_pretty(&entry) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, text + "\n").is_ok() && std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    fn agent(&mut self) -> ureq::Agent {
        self.agent
            .get_or_insert_with(|| {
                let tls = ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build();
                ureq::Agent::config_builder()
                    .tls_config(tls)
                    .proxy(ureq::Proxy::try_from_env())
                    .timeout_global(Some(Duration::from_secs(30)))
                    .user_agent(concat!("gerberus/", env!("CARGO_PKG_VERSION")))
                    .http_status_as_error(false)
                    .build()
                    .into()
            })
            .clone()
    }

    fn fetch(&mut self, code: &str) -> Result<Option<Listing>, String> {
        let url = format!("{}{code}", self.endpoint);
        let (status, body) = read(self.agent().get(&url).call(), code)?;
        if !(200..300).contains(&status) {
            let short: String = body.chars().take(300).collect();
            return Err(format!("LCSC answered HTTP {status} for {code}: {short}"));
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("LCSC's answer for {code} is not JSON: {e}"))?;
        Listing::from_lcsc_json(&json).map_err(|e| format!("{code}: {e}"))
    }
}

fn read(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>, code: &str) -> Result<(u16, String), String> {
    let mut resp = resp.map_err(|e| format!("asking LCSC about {code}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("reading LCSC's answer for {code}: {e}"))?;
    Ok((status, body))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Source for Online {
    fn caveat(&self, code: &str) -> Option<String> {
        self.caveats.get(code).cloned()
    }

    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String> {
        self.caveats.remove(code);
        if self.offline {
            return self
                .read_cache(code)?
                .map(|c| c.listing)
                .ok_or_else(|| format!("{code} is not in the LCSC cache and lookups are offline"));
        }
        let cached = self.read_cache(code).ok().flatten();
        if let Some(c) = &cached
            && now().saturating_sub(c.fetched) < self.max_age.as_secs()
            && c.version >= CACHE_VERSION
        {
            return Ok(c.listing.clone());
        }
        match self.fetch(code) {
            Ok(listing) => {
                self.write_cache(code, listing.as_ref());
                Ok(listing)
            }
            Err(e) => match cached {
                Some(c) => {
                    let hours = now().saturating_sub(c.fetched) / 3600;
                    self.caveats.insert(
                        code.to_string(),
                        format!("using LCSC's answer from {hours} h ago, because a fresh lookup failed: {e}"),
                    );
                    Ok(c.listing)
                }
                None => Err(e),
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct Fixed(pub BTreeMap<String, Option<Listing>>);

impl Source for Fixed {
    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String> {
        self.0
            .get(code)
            .cloned()
            .ok_or_else(|| format!("no listing recorded for {code}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Unknown,
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Unknown => "UNKNOWN",
            Severity::Info => "info",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Identity,
    Lifecycle,
    Stock,
    SecondSource,
    Assembly,
}

impl Topic {
    pub fn name(self) -> &'static str {
        match self {
            Topic::Identity => "bom-identity",
            Topic::Lifecycle => "bom-lifecycle",
            Topic::Stock => "bom-stock",
            Topic::SecondSource => "bom-second-source",
            Topic::Assembly => "bom-assembly",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub topic: Topic,
    pub message: String,
    pub notes: Vec<String>,
    pub help: Option<String>,
}

impl Finding {
    fn new(severity: Severity, topic: Topic, message: String) -> Finding {
        Finding {
            severity,
            topic,
            message,
            notes: vec![],
            help: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub line: Line,
    pub listing: Option<Listing>,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub boards: u64,
    pub jlcpcb: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            boards: 10,
            jlcpcb: false,
        }
    }
}

fn same_mpn(distributor: &str, schematic: &str) -> bool {
    let base = |s: &str| squash(s.split([',', '(']).next().unwrap_or(s));
    squash(distributor) == squash(schematic) || base(distributor) == base(schematic)
}

fn squash(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

pub fn lcsc_search_url(query: &str) -> String {
    format!("https://www.lcsc.com/search?q={}", encode(query))
}

pub fn check(design: &Design, source: &mut dyn Source, opts: Options) -> Vec<Health> {
    check_with_parts(design, source, opts, None)
}

pub fn check_with_parts(design: &Design, source: &mut dyn Source, opts: Options, lib: Option<&Library>) -> Vec<Health> {
    let lines = lines(design);
    let key = |l: &Line| l.lcsc.clone();
    let mut per_code: BTreeMap<String, u64> = BTreeMap::new();
    for l in &lines {
        if let Some(code) = key(l) {
            *per_code.entry(code).or_default() += l.references.len() as u64;
        }
    }
    let mut out: Vec<Health> = lines
        .into_iter()
        .map(|line| {
            let mut findings = Vec::new();
            let code = key(&line);
            let listing = match (&code, &line.bad_lcsc) {
                (_, Some(bad)) => {
                    let mut f = Finding::new(
                        if opts.jlcpcb {
                            Severity::Error
                        } else {
                            Severity::Unknown
                        },
                        Topic::Identity,
                        format!("the LCSC field says {bad:?}, which is not an LCSC part number (C followed by digits)"),
                    );
                    f.help = Some(format!("find the part at {}", lcsc_search_url(&line.mpn)));
                    findings.push(f);
                    None
                }
                (None, _) if opts.jlcpcb => {
                    let mut f = Finding::new(
                        Severity::Error,
                        Topic::Assembly,
                        format!("no LCSC part number, so JLCPCB can't fit {}", line.mpn),
                    );
                    f.help = Some(format!(
                        "find {} or an equivalent at {} and put its C number in an LCSC field",
                        line.mpn,
                        lcsc_search_url(&line.mpn)
                    ));
                    findings.push(f);
                    None
                }
                (None, _) => {
                    findings.push(Finding::new(
                        Severity::Unknown,
                        Topic::Identity,
                        format!(
                            "no LCSC part number, so stock and second sources for {} can't be looked up",
                            line.mpn
                        ),
                    ));
                    None
                }
                (Some(code), _) => match source.listing(code) {
                    Err(e) => {
                        findings.push(Finding::new(
                            Severity::Unknown,
                            Topic::Identity,
                            format!("lookup failed: {e}"),
                        ));
                        None
                    }
                    Ok(None) => {
                        let mut f = Finding::new(
                            Severity::Warning,
                            Topic::Lifecycle,
                            format!("LCSC has no listing for {code}: it may be delisted, or the number is wrong"),
                        );
                        if opts.jlcpcb {
                            f.severity = Severity::Error;
                            f.help = Some(format!("find {} at {}", line.mpn, lcsc_search_url(&line.mpn)));
                        }
                        findings.push(f);
                        None
                    }
                    Ok(Some(l)) => Some(l),
                },
            };
            if let Some(code) = &code
                && let Some(c) = source.caveat(code)
            {
                findings.push(Finding::new(Severity::Warning, Topic::Stock, c));
            }
            if let Some(l) = &listing {
                let per_board = code.as_ref().and_then(|c| per_code.get(c)).copied().unwrap_or(0);
                let want = want_spec(design, &line, lib);
                judge(&line, l, per_board, opts, want.as_ref(), source, &mut findings);
                if let Some(w) = &want {
                    other_references(design, &line, l, w, &mut findings);
                }
            }
            Health {
                line,
                listing,
                findings,
            }
        })
        .collect();
    if opts.jlcpcb {
        let extended: Vec<&Health> = out
            .iter()
            .filter(|h| {
                h.listing
                    .as_ref()
                    .is_some_and(|l| l.assembly.as_deref() == Some("extended"))
            })
            .collect();
        let basic = out
            .iter()
            .filter(|h| {
                h.listing
                    .as_ref()
                    .is_some_and(|l| l.assembly.as_deref() == Some("basic"))
            })
            .count();
        if !extended.is_empty() {
            let names: Vec<String> = extended.iter().map(|h| h.line.references[0].clone()).collect();
            let summary = Finding {
                severity: Severity::Info,
                topic: Topic::Assembly,
                message: format!(
                    "{} of {} lines are JLCPCB extended parts ({basic} basic); JLCPCB charges a loading fee for \
                     each extended part, so a basic equivalent saves money: {}",
                    extended.len(),
                    out.len(),
                    names.join(", ")
                ),
                notes: vec![],
                help: None,
            };
            if let Some(last) = out.last_mut() {
                last.findings.push(summary);
            }
        }
    }
    out
}

fn other_references(design: &Design, line: &Line, l: &Listing, first: &Spec, out: &mut Vec<Finding>) {
    let have = Spec::from_lcsc(&l.params, &l.package);
    if have.kind.is_none() {
        return;
    }
    for r in line.references.iter().skip(1) {
        let Some(c) = design.components.get(r) else { continue };
        let s = Spec::from_component(c, None);
        if s.value == first.value && s.size == first.size {
            continue;
        }
        let cmp = compare(&s, &have);
        if !cmp.different.is_empty() {
            let mut f = Finding::new(
                Severity::Error,
                Topic::Identity,
                format!(
                    "for {r}, LCSC {} is a different part: {}",
                    l.code,
                    cmp.different.join("; ")
                ),
            );
            f.notes.push(format!("{} is a {}", l.code, have.describe()));
            out.push(f);
        }
    }
}

fn want_spec(design: &Design, line: &Line, lib: Option<&Library>) -> Option<Spec> {
    if !line.commodity {
        return None;
    }
    let c = design.components.get(line.references.first()?)?;
    let part = lib.and_then(|l| l.parts.get(component_mpn(c)));
    let s = Spec::from_component(c, part);
    s.kind.is_some().then_some(s)
}

const EQUIVALENT_LOOKUPS: usize = 8;

fn judge(
    line: &Line,
    l: &Listing,
    per_board: u64,
    opts: Options,
    want: Option<&Spec>,
    source: &mut dyn Source,
    out: &mut Vec<Finding>,
) {
    let src = "LCSC";
    let code = if l.code.is_empty() { &l.mpn } else { &l.code };
    let have = want.map(|_| Spec::from_lcsc(&l.params, &l.package));
    let mut spec_problem = false;
    let mut fix_rating = false;
    let mut identity: Option<usize> = None;
    match (want, &have) {
        (Some(_), Some(h)) if h.kind.is_none() && !l.params.is_empty() => {
            spec_problem = true;
            let i = push(
                out,
                Severity::Error,
                Topic::Identity,
                format!(
                    "{src} {code} is not a plain resistor or capacitor: {src} gives it no resistance or capacitance"
                ),
            );
            identity = Some(i);
            out[i].help = Some(format!("find the right part at {}", lcsc_search_url(&line.mpn)));
        }
        (Some(_), Some(_)) if l.params.is_empty() => {
            spec_problem = true;
            push(
                out,
                Severity::Unknown,
                Topic::Identity,
                format!(
                    "{src}'s answer for {code} has no parameter table (it was cached by an older gerberus), so it \
                     wasn't compared with the schematic's value, size and ratings: run without --offline to refresh it"
                ),
            );
        }
        (Some(w), Some(h)) => {
            let cmp = compare(w, h);
            let mpn_differs = line.has_mpn_field && !same_mpn(&l.mpn, &line.mpn);
            if cmp.unchecked.contains(&"value") {
                spec_problem = true;
                push(
                    out,
                    Severity::Unknown,
                    Topic::Identity,
                    format!(
                        "gerberus can't read the schematic's value for {}, so {code} ({}) wasn't checked against it",
                        line.mpn,
                        h.describe()
                    ),
                );
            }
            if cmp.unchecked.contains(&"size") && h.size.is_some() {
                spec_problem = true;
                push(
                    out,
                    Severity::Unknown,
                    Topic::Identity,
                    format!(
                        "the footprint gives no chip size, so {code}'s {} size wasn't checked against it",
                        h.size.as_deref().unwrap_or_default()
                    ),
                );
            }
            if !cmp.different.is_empty() {
                spec_problem = true;
                let i = push(
                    out,
                    Severity::Error,
                    Topic::Identity,
                    format!("{src} {code} is a different part: {}", cmp.different.join("; ")),
                );
                identity = Some(i);
                out[i].notes.push(format!("{code} is a {}", h.describe()));
                out[i].notes.push(format!("the schematic gives a {}", w.describe()));
                out[i].help = Some(format!("find the right part at {}", lcsc_search_url(&line.mpn)));
            } else if !cmp.worse.is_empty() {
                spec_problem = true;
                fix_rating = true;
                let i = push(
                    out,
                    Severity::Error,
                    Topic::Identity,
                    format!(
                        "{src} {code} is rated below the schematic's part: {}",
                        cmp.worse.join("; ")
                    ),
                );
                identity = Some(i);
                out[i].notes.push(format!("{code} is a {}", h.describe()));
                out[i]
                    .notes
                    .push(format!("the MPN field's {} is a {}", line.mpn, w.describe()));
            } else if mpn_differs {
                let i = push(
                    out,
                    Severity::Warning,
                    Topic::Identity,
                    format!(
                        "{src} {code} is {} {}, another part than the MPN field's {}, with the same {}",
                        l.manufacturer,
                        l.mpn,
                        line.mpn,
                        join_and(&cmp.compared)
                    ),
                );
                identity = Some(i);
                let unchecked: Vec<&str> = cmp.unchecked.iter().copied().filter(|u| h_has(h, u)).collect();
                if !unchecked.is_empty() {
                    out[i].notes.push(format!(
                        "gerberus doesn't know {}'s {} (no part file, or one that doesn't give them), so they can't be \
                         compared with {}'s {}",
                        line.mpn,
                        join_and(&unchecked),
                        l.mpn,
                        h.describe()
                    ));
                }
                out[i].help = Some(format!(
                    "set the MPN field to {} so the checks use the part JLCPCB will fit",
                    l.mpn
                ));
            }
        }
        _ => {
            if line.has_mpn_field && !same_mpn(&l.mpn, &line.mpn) {
                push(
                    out,
                    Severity::Error,
                    Topic::Identity,
                    format!(
                        "{src} {code} is {} {}, but the MPN field says {}",
                        l.manufacturer, l.mpn, line.mpn
                    ),
                );
            }
        }
    }

    if l.status != "normal" {
        push(
            out,
            Severity::Warning,
            Topic::Lifecycle,
            format!("{src} lists {code} as {:?}, not \"normal\"", l.status),
        );
    }

    let needed = per_board.saturating_mul(opts.boards);
    let shared = if per_board > line.references.len() as u64 {
        ", counting other lines with the same part number"
    } else {
        ""
    };
    let mut stock: Option<usize> = None;
    let basic_for_jlcpcb = opts.jlcpcb && l.assembly.as_deref() == Some("basic");
    let short = if basic_for_jlcpcb {
        Severity::Warning
    } else {
        Severity::Error
    };
    if l.stock == 0 {
        stock = Some(push(
            out,
            short,
            Topic::Stock,
            format!("{src} has none of {code} in stock"),
        ));
    } else if l.stock < needed {
        stock = Some(push(
            out,
            short,
            Topic::Stock,
            format!(
                "{src} has {} of {code}; {} boards need {needed} ({per_board} each{shared})",
                l.stock, opts.boards
            ),
        ));
    }

    if basic_for_jlcpcb && let Some(i) = stock {
        out[i].notes.push(format!(
            "this is LCSC's retail stock; {code} is a JLCPCB basic part, which JLCPCB stocks for assembly \
             separately, so check JLCPCB's parts library before replacing it"
        ));
    }

    if let (Some(w), Some(h)) = (want, &have)
        && h.kind.is_some()
        && (!spec_problem || fix_rating)
    {
        let extended = l.assembly.as_deref() == Some("extended");
        let wanted_basic = opts.jlcpcb && extended;
        if stock.is_some() || fix_rating || wanted_basic {
            let floor = floor_spec(w, h);
            let missing = floor.missing_ratings();
            let looked = if missing.is_empty() {
                equivalents(l, &floor, needed.max(1), source)
            } else {
                Equivalents {
                    found: vec![],
                    unread: 0,
                    capped: false,
                }
            };
            let found = &looked.found;
            let scope = if looked.capped {
                format!("the first {EQUIVALENT_LOOKUPS} of {src}'s similar parts")
            } else {
                format!("{src}'s similar parts")
            };
            let target = stock.or(identity.filter(|_| fix_rating));
            let basic: Vec<&(Listing, Spec)> = found
                .iter()
                .filter(|(e, _)| e.assembly.as_deref() == Some("basic"))
                .collect();
            let pick = |list: &[&(Listing, Spec)]| -> String {
                list.iter()
                    .take(3)
                    .map(|(e, s)| {
                        format!(
                            "{} {} {} ({}, {} in stock{})",
                            e.code,
                            e.manufacturer,
                            e.mpn,
                            s.describe(),
                            e.stock,
                            e.assembly
                                .as_deref()
                                .map(|a| format!(", JLCPCB {a}"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            };
            let all: Vec<&(Listing, Spec)> = found.iter().collect();
            match target {
                Some(i) if !found.is_empty() => {
                    out[i].help = Some(format!(
                        "use an LCSC equivalent with the same value and size and ratings at least as good: {}",
                        pick(&all)
                    ));
                }
                Some(i) if !missing.is_empty() => out[i].notes.push(format!(
                    "no equivalents are offered: neither the MPN field's part file nor {src} gives the {}, so a \
                     replacement can't be checked against it",
                    missing.join(" or ")
                )),
                Some(i) => {
                    out[i].notes.push(format!(
                        "none of {scope} has the same value, size and ratings with enough stock"
                    ));
                    if looked.unread > 0 {
                        out[i].notes.push(format!(
                            "{} of them couldn't be looked up (offline, a failed lookup, or an old cache entry), so \
                             weren't judged",
                            looked.unread
                        ));
                    }
                    if out[i].help.is_none() {
                        out[i].help = Some(format!("find a {} at {}", floor.describe(), lcsc_search_url(&line.mpn)));
                    }
                }
                None if wanted_basic && !basic.is_empty() => {
                    let i = push(
                        out,
                        Severity::Info,
                        Topic::Assembly,
                        format!(
                            "{code} is a JLCPCB extended part; a basic equivalent with the same value and size and \
                             ratings at least as good avoids its loading fee: {}",
                            pick(&basic)
                        ),
                    );
                    let _ = i;
                }
                None => {}
            }
        }
    }

    if l.package.is_empty() {
        let sev = if line.commodity {
            Severity::Info
        } else {
            Severity::Unknown
        };
        push(
            out,
            sev,
            Topic::SecondSource,
            format!("{src} gives no package for {code}, so similar parts can't be matched to it"),
        );
        return;
    }
    if have.as_ref().is_some_and(|h| h.kind.is_some()) {
        return;
    }
    let maker = squash(&l.manufacturer);
    let package = squash(&l.package);
    let others: Vec<&Similar> = l
        .similar
        .iter()
        .filter(|s| {
            squash(&s.manufacturer) != maker
                && s.status == "normal"
                && s.stock >= needed.max(1)
                && squash(&s.package) == package
        })
        .collect();
    if others.is_empty() {
        let why = if l.similar.is_empty() {
            "lists no similar parts".to_string()
        } else {
            "lists no similar part from another maker in the same package with enough stock".to_string()
        };
        if line.commodity {
            push(
                out,
                Severity::Info,
                Topic::SecondSource,
                format!("{src} {why}, but a plain resistor or capacitor of this value and size is made by many others"),
            );
        } else {
            push(
                out,
                Severity::Warning,
                Topic::SecondSource,
                format!("single source: {src} {why}"),
            );
        }
    } else {
        let names: Vec<String> = others
            .iter()
            .take(3)
            .map(|s| format!("{} {} ({}, {} in stock)", s.manufacturer, s.mpn, s.code, s.stock))
            .collect();
        push(
            out,
            Severity::Info,
            Topic::SecondSource,
            format!(
                "candidate second sources, not checked for fit or ratings: {}{}",
                names.join("; "),
                if others.len() > 3 {
                    format!(" and {} more", others.len() - 3)
                } else {
                    String::new()
                }
            ),
        );
    }
}

fn push(out: &mut Vec<Finding>, severity: Severity, topic: Topic, message: String) -> usize {
    out.push(Finding::new(severity, topic, message));
    out.len() - 1
}

fn h_has(h: &Spec, what: &str) -> bool {
    match what {
        "value" => h.value.is_some(),
        "size" => h.size.is_some(),
        "tolerance" => h.tolerance.is_some(),
        "rated voltage" => h.voltage.is_some(),
        "rated power" => h.power.is_some(),
        "dielectric" => h.dielectric.is_some(),
        _ => false,
    }
}

fn floor_spec(want: &Spec, lcsc: &Spec) -> Spec {
    let mut s = want.clone();
    s.tolerance = s.tolerance.or(lcsc.tolerance);
    s.voltage = s.voltage.or(lcsc.voltage);
    s.power = s.power.or(lcsc.power);
    s.dielectric = s.dielectric.clone().or_else(|| lcsc.dielectric.clone());
    s.size = s.size.clone().or_else(|| lcsc.size.clone());
    s
}

struct Equivalents {
    found: Vec<(Listing, Spec)>,
    unread: usize,
    capped: bool,
}

fn equivalents(l: &Listing, floor: &Spec, needed: u64, source: &mut dyn Source) -> Equivalents {
    let candidates: Vec<&Similar> = l
        .similar
        .iter()
        .filter(|s| s.status == "normal" && s.stock >= needed && s.code != l.code)
        .collect();
    let mut unread = 0;
    let mut found = vec![];
    for c in candidates.iter().take(EQUIVALENT_LOOKUPS) {
        let e = match source.listing(&c.code) {
            Ok(Some(e)) if !e.params.is_empty() => e,
            _ => {
                unread += 1;
                continue;
            }
        };
        if e.status != "normal" || e.stock < needed {
            continue;
        }
        let s = Spec::from_lcsc(&e.params, &e.package);
        if compare(floor, &s).fits() {
            found.push((e, s));
        }
    }
    found.sort_by_key(|(e, _)| (e.assembly.as_deref() != Some("basic"), std::cmp::Reverse(e.stock)));
    Equivalents {
        found,
        unread,
        capped: candidates.len() > EQUIVALENT_LOOKUPS,
    }
}

fn join_and(words: &[&str]) -> String {
    match words {
        [] => "nothing".into(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn listing_note(h: &Health) -> Option<String> {
    let l = h.listing.as_ref()?;
    Some(format!(
        "LCSC {} is {} {}: {} in stock, LCSC status {}{}",
        l.code,
        l.manufacturer,
        l.mpn,
        l.stock,
        l.status,
        l.assembly
            .as_deref()
            .map(|a| format!(", JLCPCB {a}"))
            .unwrap_or_default()
    ))
}

pub fn render(report: &[Health], opts: Options, show: &crate::diag::Show) -> String {
    use crate::diag::{Diagnostic, Level};
    let mut out = String::new();
    let (mut errors, mut warnings, mut unknown, mut passed) = (0, 0, 0, 0);
    let mut closing = vec![];
    for h in report {
        let location = format!("{} ({})", h.line.references.join(", "), h.line.mpn);
        let infos: Vec<&Finding> = h.findings.iter().filter(|f| f.severity == Severity::Info).collect();
        let (board, infos): (Vec<&Finding>, Vec<&Finding>) = infos
            .into_iter()
            .partition(|f| f.topic == Topic::Assembly && f.message.contains(" lines are JLCPCB extended"));
        closing.extend(board.iter().map(|f| f.message.clone()));
        match h.findings.iter().map(|f| f.severity).min() {
            Some(Severity::Error) => errors += 1,
            Some(Severity::Warning) => warnings += 1,
            Some(Severity::Unknown) => unknown += 1,
            _ => passed += 1,
        }
        let (todo, infos): (Vec<&Finding>, Vec<&Finding>) = infos.into_iter().partition(|f| f.topic == Topic::Assembly);
        for f in &todo {
            let mut d = Diagnostic::line(
                Level::Todo,
                f.topic.name(),
                &format!("{}: {}", h.line.references[0], f.message),
            );
            d.location = Some(location.clone());
            d.notes = listing_note(h).into_iter().collect();
            d.render(&mut out, show.style, show.verbose, show.columns());
        }
        let mut any = false;
        for f in h.findings.iter().filter(|f| f.severity != Severity::Info) {
            any = true;
            let level = match f.severity {
                Severity::Error => Level::Error,
                Severity::Warning => Level::Warning,
                _ => Level::Unknown,
            };
            if !show.lists(level) {
                continue;
            }
            let mut notes: Vec<String> = listing_note(h).into_iter().collect();
            notes.extend(f.notes.iter().cloned());
            notes.extend(infos.iter().map(|i| i.message.clone()));
            let d = Diagnostic {
                level,
                rule: f.topic.name().to_string(),
                headline: format!("{}: {}", h.line.references[0], f.message),
                location: Some(location.clone()),
                notes,
                help: f.help.clone(),
                details: vec![],
            };
            d.render(&mut out, show.style, show.verbose, show.columns());
        }
        if !any && show.lists(Level::Pass) {
            {
                let what = listing_note(h).unwrap_or_default();
                let d = Diagnostic::line(Level::Pass, "bom", &format!("{location}: {what}"));
                d.render(&mut out, show.style, show.verbose, show.columns());
                for i in &infos {
                    let _ = std::fmt::Write::write_fmt(&mut out, format_args!("   = note: {}\n", i.message));
                }
            }
        }
    }
    let note = |out: &mut String, text: &str| {
        let _ = std::fmt::Write::write_fmt(out, format_args!("note: {text}\n"));
    };
    for c in &closing {
        note(&mut out, c);
    }
    if opts.jlcpcb {
        note(&mut out, JLCPCB_STOCK_NOTE);
    }
    let _ = std::fmt::Write::write_fmt(
        &mut out,
        format_args!(
            "{} lines for {} boards{}; each line is counted once, at its worst finding\n",
            report.len(),
            opts.boards,
            if opts.jlcpcb { ", assembled by JLCPCB" } else { "" }
        ),
    );
    crate::diag::summary(&mut out, show, [errors, warnings, unknown, passed], false);
    out
}

pub const JLCPCB_STOCK_NOTE: &str = "stock is LCSC's; JLCPCB keeps its own assembly stock of the same C numbers, \
which can differ, so check a part LCSC is short of in JLCPCB's parts library before replacing it.";
