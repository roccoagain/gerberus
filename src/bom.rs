//! BOM health: whether each part on the board can still be bought, and from whom.
//!
//! Parts are grouped into BOM lines by the same part number the limits library uses ([`component_mpn`]),
//! then looked up by their distributor part number. The only source today is LCSC, read through the JSON
//! endpoint behind its product pages, which needs no API key. It gives LCSC's stock, LCSC's own listing status
//! and a list of similar parts. It does **not** give the manufacturer's lifecycle (active, NRND, EOL), so that
//! is always reported as unknown rather than guessed from LCSC's status.
//!
//! Lookups are cached on disk, so a check can run offline against the last answer and tests never touch the
//! network.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::design::{Component, Design};
use crate::limits::component_mpn;

/// Schematic fields that may hold an LCSC part number, in the order they are tried. KiCad libraries and
/// JLCPCB's tooling use all of these.
pub const LCSC_FIELDS: [&str; 5] = ["LCSC", "LCSC Part", "LCSC Part #", "JLCPCB Part #", "JLC"];

/// One BOM line: every fitted part that shares a part number and a distributor part number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The part number from the `MPN` field, or the value when there is none (as the limits library does).
    pub mpn: String,
    /// Whether `mpn` came from an `MPN` field rather than the value.
    pub has_mpn_field: bool,
    /// The LCSC part number, such as `C1525`, if the schematic gives one.
    pub lcsc: Option<String>,
    /// The raw text of an LCSC field that isn't an LCSC part number.
    pub bad_lcsc: Option<String>,
    pub references: Vec<String>,
    /// A plain resistor or non-polarised capacitor (KiCad `Device:R…` or `Device:C…` symbol). Many makers
    /// build the same value and size, so a single LCSC listing is not a sourcing risk.
    pub commodity: bool,
}

/// Groups the fitted parts of a design into BOM lines. Parts that are not fitted, not in the BOM or not on the
/// board are left out.
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
        // One part with an MPN field is enough to compare against LCSC; one part that isn't a plain
        // resistor or capacitor makes the whole line a sourcing question.
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

/// KiCad's plain resistor and non-polarised capacitor symbols. Anything else (crystals, shunts, trimmers,
/// polarised capacitors) is not a commodity.
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

/// `C1525`, `c1525` and ` C1525 ` are all `C1525`. Anything else is not an LCSC part number.
pub fn normalize_lcsc(s: &str) -> Option<String> {
    let s = s.trim();
    let digits = s.strip_prefix('C').or_else(|| s.strip_prefix('c'))?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| format!("C{digits}"))
}

/// Orders `C2` before `C10`.
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

/// What a distributor says about one of its part numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    /// The distributor's part number, such as `C1525`.
    pub code: String,
    /// The manufacturer part number the distributor sells under this code.
    pub mpn: String,
    pub manufacturer: String,
    /// Units in stock.
    pub stock: u64,
    /// The distributor's own listing status (LCSC's `productCycle`, `normal` for a part it sells normally).
    /// This is not the manufacturer's lifecycle.
    pub status: String,
    /// JLCPCB assembly class: `basic` or `extended`, when LCSC gives one.
    pub assembly: Option<String>,
    pub package: String,
    /// Parts the distributor lists as similar. Nobody has checked that they fit or meet the same ratings.
    pub similar: Vec<Similar>,
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
    /// Reads LCSC's product detail JSON. `Ok(None)` means LCSC answered but has no listing for the code.
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
        }))
    }
}

/// Where listings come from.
pub trait Source {
    /// `Ok(None)` when the distributor has no listing for `code`.
    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String>;
    /// A short name for reports, such as `LCSC`.
    fn name(&self) -> &str;
    /// Why the last answer for `code` may be out of date, such as a cached answer used because a fresh
    /// lookup failed.
    fn caveat(&self, _code: &str) -> Option<String> {
        None
    }
}

/// One cached answer. `listing: None` caches "LCSC has no such part" too.
#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    /// Seconds since the Unix epoch.
    fetched: u64,
    listing: Option<Listing>,
}

/// LCSC, read through the JSON endpoint its product pages use. No key is needed.
pub struct Lcsc {
    /// One JSON file per LCSC code. Without it every lookup goes to the network.
    pub cache: Option<PathBuf>,
    /// Cached answers younger than this are used without asking LCSC again.
    pub max_age: Duration,
    /// Use only the cache, whatever its age; a code that isn't cached is an error.
    pub offline: bool,
    /// The product detail endpoint, followed by the LCSC code. Tests point it somewhere unreachable.
    pub endpoint: String,
    agent: Option<ureq::Agent>,
    caveats: BTreeMap<String, String>,
}

const LCSC_DETAIL: &str = "https://wmsc.lcsc.com/ftps/wm/product/detail?productCode=";

impl Lcsc {
    pub fn new(cache: Option<PathBuf>, max_age: Duration, offline: bool) -> Lcsc {
        Lcsc {
            cache,
            max_age,
            offline,
            endpoint: LCSC_DETAIL.to_string(),
            agent: None,
            caveats: BTreeMap::new(),
        }
    }

    /// `$XDG_CACHE_HOME/gerberus/lcsc`, or `~/.cache/gerberus/lcsc`.
    pub fn default_cache_dir() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
        Some(base.join("gerberus").join("lcsc"))
    }

    fn cache_path(&self, code: &str) -> Option<PathBuf> {
        Some(self.cache.as_ref()?.join(format!("{code}.json")))
    }

    /// `Ok(None)` when nothing is cached; `Err` when a cache file exists but can't be read.
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
        // A cache that can't be written only costs a repeat lookup, so it is not an error. Writing to a
        // temporary file and renaming it means a reader never sees half a file.
        let Some(path) = self.cache_path(code) else { return };
        let entry = CacheEntry {
            fetched: now(),
            listing: listing.cloned(),
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

    fn fetch(&mut self, code: &str) -> Result<Option<Listing>, String> {
        let agent = self.agent.get_or_insert_with(|| {
            let tls = ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build();
            ureq::Agent::config_builder()
                .tls_config(tls)
                .proxy(ureq::Proxy::try_from_env())
                .timeout_global(Some(Duration::from_secs(30)))
                .user_agent(concat!("gerberus/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        });
        let url = format!("{}{code}", self.endpoint);
        let mut resp = agent
            .get(&url)
            .call()
            .map_err(|e| format!("asking LCSC about {code}: {e}"))?;
        let body = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("reading LCSC's answer for {code}: {e}"))?;
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("LCSC's answer for {code} is not JSON: {e}"))?;
        Listing::from_lcsc_json(&json).map_err(|e| format!("{code}: {e}"))
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Source for Lcsc {
    fn name(&self) -> &'static str {
        "LCSC"
    }

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
        // A damaged cache file is refetched rather than trusted.
        let cached = self.read_cache(code).ok().flatten();
        if let Some(c) = &cached
            && now().saturating_sub(c.fetched) < self.max_age.as_secs()
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

/// A fixed set of listings, for tests.
#[derive(Debug, Default)]
pub struct Fixed(pub BTreeMap<String, Option<Listing>>);

impl Source for Fixed {
    fn name(&self) -> &'static str {
        "LCSC"
    }

    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String> {
        self.0
            .get(code)
            .cloned()
            .ok_or_else(|| format!("no listing recorded for {code}"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The board can't be built as drawn.
    Error,
    /// Worth a look before ordering.
    Warning,
    /// Not enough data to say.
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
    /// The schematic's distributor number and part number disagree, or one is missing.
    Identity,
    Lifecycle,
    Stock,
    SecondSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub topic: Topic,
    pub message: String,
}

/// The health of one BOM line.
#[derive(Debug, Clone, PartialEq)]
pub struct Health {
    pub line: Line,
    /// `None` when there was nothing to look up, or the lookup failed (see `findings`).
    pub listing: Option<Listing>,
    pub findings: Vec<Finding>,
}

/// Options for [`check`].
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// How many boards will be built. Stock below `boards ×` the parts per board is an error.
    pub boards: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options { boards: 10 }
    }
}

/// Upper case with everything but letters and digits removed, so `SM04B-GHS-TB(LF)(SN)` and
/// `SM04B-GHS-TB_LF_SN` match.
fn squash(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Checks every BOM line of a design against a source.
pub fn check(design: &Design, source: &mut dyn Source, opts: Options) -> Vec<Health> {
    let lines = lines(design);
    // Lines with different part numbers can share an LCSC number (`10k` and `10K`); stock must cover them all.
    let mut per_code: BTreeMap<String, u64> = BTreeMap::new();
    for l in &lines {
        if let Some(code) = &l.lcsc {
            *per_code.entry(code.clone()).or_default() += l.references.len() as u64;
        }
    }
    lines
        .into_iter()
        .map(|line| {
            let mut findings = Vec::new();
            let mut f = |severity, topic, message: String| {
                findings.push(Finding {
                    severity,
                    topic,
                    message,
                });
            };
            let listing = match (&line.lcsc, &line.bad_lcsc) {
                (_, Some(bad)) => {
                    f(
                        Severity::Unknown,
                        Topic::Identity,
                        format!("the LCSC field says {bad:?}, which is not an LCSC part number (C followed by digits)"),
                    );
                    None
                }
                (None, None) => {
                    f(
                        Severity::Unknown,
                        Topic::Identity,
                        format!(
                            "no LCSC part number, so stock and second sources for {} can't be looked up",
                            line.mpn
                        ),
                    );
                    None
                }
                (Some(code), None) => match source.listing(code) {
                    Err(e) => {
                        f(Severity::Unknown, Topic::Identity, format!("lookup failed: {e}"));
                        None
                    }
                    Ok(None) => {
                        f(
                            Severity::Warning,
                            Topic::Lifecycle,
                            format!(
                                "{} has no listing for {code}: it may be delisted, or the number is wrong",
                                source.name()
                            ),
                        );
                        None
                    }
                    Ok(Some(l)) => Some(l),
                },
            };
            if let Some(code) = &line.lcsc
                && let Some(c) = source.caveat(code)
            {
                f(Severity::Warning, Topic::Stock, c);
            }
            if let Some(l) = &listing {
                let per_board = line.lcsc.as_ref().and_then(|c| per_code.get(c)).copied().unwrap_or(0);
                judge(&line, l, per_board, source.name(), opts, &mut f);
            }
            Health {
                line,
                listing,
                findings,
            }
        })
        .collect()
}

/// `per_board` counts every part on the board that uses this LCSC number, across all lines.
fn judge(
    line: &Line,
    l: &Listing,
    per_board: u64,
    src: &str,
    opts: Options,
    f: &mut impl FnMut(Severity, Topic, String),
) {
    let code = &l.code;
    if line.has_mpn_field && squash(&l.mpn) != squash(&line.mpn) {
        f(
            Severity::Error,
            Topic::Identity,
            format!(
                "{src} {code} is {} {}, but the MPN field says {}",
                l.manufacturer, l.mpn, line.mpn
            ),
        );
    }

    if l.status != "normal" {
        f(
            Severity::Warning,
            Topic::Lifecycle,
            format!("{src} lists {code} as {:?}, not \"normal\"", l.status),
        );
    }

    let needed = per_board.saturating_mul(opts.boards);
    let shared = if per_board > line.references.len() as u64 {
        ", counting other lines with the same LCSC number"
    } else {
        ""
    };
    if l.stock == 0 {
        f(
            Severity::Error,
            Topic::Stock,
            format!("{src} has none of {code} in stock"),
        );
    } else if l.stock < needed {
        f(
            Severity::Error,
            Topic::Stock,
            format!(
                "{src} has {} of {code}; {} boards need {needed} ({per_board} each{shared})",
                l.stock, opts.boards
            ),
        );
    }

    if l.package.is_empty() {
        let sev = if line.commodity {
            Severity::Info
        } else {
            Severity::Unknown
        };
        f(
            sev,
            Topic::SecondSource,
            format!("{src} gives no package for {code}, so similar parts can't be matched to it"),
        );
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
            f(
                Severity::Info,
                Topic::SecondSource,
                format!("{src} {why}, but a plain resistor or capacitor of this value and size is made by many others"),
            );
        } else {
            f(
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
        f(
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

/// Said once per report: the source can't tell an active part from one the manufacturer is ending.
pub const LIFECYCLE_NOTE: &str = "manufacturer lifecycle (active, NRND, EOL) is UNKNOWN for every part: LCSC only \
says whether it still lists a part. It needs a Nexar/Octopart, Mouser or DigiKey API key.";
