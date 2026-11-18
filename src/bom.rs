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
    pub lifecycle: Option<String>,
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
            lifecycle: None,
            params,
        }))
    }
}

fn text(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(t)) => t.trim().to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn leading_count(t: &str) -> Option<u64> {
    let digits: String = t
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

impl Listing {
    pub fn from_mouser_json(json: &serde_json::Value, mpn: &str) -> Result<Option<Listing>, String> {
        if let Some(errors) = json.get("Errors").and_then(|e| e.as_array())
            && !errors.is_empty()
        {
            let msgs: Vec<String> = errors
                .iter()
                .map(|e| {
                    let m = text(e.get("Message"));
                    if m.is_empty() { e.to_string() } else { m }
                })
                .collect();
            return Err(format!("Mouser answered with an error: {}", msgs.join("; ")));
        }
        let parts = json
            .get("SearchResults")
            .ok_or("Mouser's answer has no SearchResults")?
            .get("Parts")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();
        let want = squash(mpn);
        let best = parts
            .iter()
            .filter(|p| squash(&text(p.get("ManufacturerPartNumber"))) == want)
            .map(|p| {
                let stock = leading_count(&text(p.get("AvailabilityInStock")))
                    .or_else(|| leading_count(&text(p.get("Availability"))))
                    .unwrap_or(0);
                (stock, p)
            })
            .max_by_key(|(stock, _)| *stock);
        let Some((stock, p)) = best else { return Ok(None) };
        let mut lifecycle = Some(text(p.get("LifecycleStatus"))).filter(|l| !l.is_empty());
        if lifecycle.is_none() && text(p.get("IsDiscontinued")).eq_ignore_ascii_case("true") {
            lifecycle = Some("Discontinued".into());
        }
        Ok(Some(Listing {
            code: text(p.get("MouserPartNumber")),
            mpn: text(p.get("ManufacturerPartNumber")),
            manufacturer: text(p.get("Manufacturer")),
            stock,
            status: "normal".into(),
            assembly: None,
            package: String::new(),
            similar: vec![],
            lifecycle,
            params: BTreeMap::new(),
        }))
    }

    pub fn from_digikey_json(json: &serde_json::Value) -> Result<Option<Listing>, String> {
        let p = match json.get("Product") {
            None | Some(serde_json::Value::Null) => return Ok(None),
            Some(p) => p,
        };
        let named = |v: Option<&serde_json::Value>, k: &str| match v {
            Some(o @ serde_json::Value::Object(_)) => text(o.get(k)),
            other => text(other),
        };
        let mpn = text(p.get("ManufacturerProductNumber"));
        if mpn.is_empty() {
            return Err("DigiKey's answer has no ManufacturerProductNumber".into());
        }
        let stock = p
            .get("QuantityAvailable")
            .and_then(serde_json::Value::as_u64)
            .ok_or("DigiKey's answer has no QuantityAvailable")?;
        let code = p
            .get("ProductVariations")
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
            .map(|v| text(v.get("DigiKeyProductNumber")))
            .unwrap_or_default();
        let package = p
            .get("Parameters")
            .and_then(|v| v.as_array())
            .and_then(|a| {
                a.iter()
                    .find(|x| text(x.get("ParameterText")) == "Package / Case")
                    .map(|x| text(x.get("ValueText")))
            })
            .unwrap_or_default();
        Ok(Some(Listing {
            code,
            mpn,
            manufacturer: named(p.get("Manufacturer"), "Name"),
            stock,
            status: "normal".into(),
            assembly: None,
            package,
            similar: vec![],
            lifecycle: Some(named(p.get("ProductStatus"), "Status")).filter(|l| !l.is_empty()),
            params: BTreeMap::new(),
        }))
    }
}

pub trait Source {
    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String>;
    fn name(&self) -> &str;
    fn caveat(&self, _code: &str) -> Option<String> {
        None
    }
    fn by_mpn(&self) -> bool {
        false
    }
    fn lists_similar(&self) -> bool {
        true
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

#[derive(Debug, Clone)]
pub enum Api {
    Lcsc,
    Mouser { key: String },
    DigiKey { client_id: String, client_secret: String },
}

impl Api {
    pub fn name(&self) -> &'static str {
        match self {
            Api::Lcsc => "LCSC",
            Api::Mouser { .. } => "Mouser",
            Api::DigiKey { .. } => "DigiKey",
        }
    }

    fn default_endpoint(&self) -> &'static str {
        match self {
            Api::Lcsc => LCSC_DETAIL,
            Api::Mouser { .. } => MOUSER_SEARCH,
            Api::DigiKey { .. } => DIGIKEY_PRODUCTS,
        }
    }
}

pub struct Online {
    pub api: Api,
    pub cache: Option<PathBuf>,
    pub max_age: Duration,
    pub offline: bool,
    pub endpoint: String,
    agent: Option<ureq::Agent>,
    token: Option<String>,
    last_request: Option<std::time::Instant>,
    caveats: BTreeMap<String, String>,
}

pub type Lcsc = Online;

const LCSC_DETAIL: &str = "https://wmsc.lcsc.com/ftps/wm/product/detail?productCode=";
const MOUSER_SEARCH: &str = "https://api.mouser.com/api/v1/search/partnumber";
const DIGIKEY_PRODUCTS: &str = "https://api.digikey.com/products/v4/search";
const MOUSER_SPACING_UNDER_30_PER_MINUTE: Duration = Duration::from_millis(2100);
const DIGIKEY_TOKEN: &str = "https://api.digikey.com/v1/oauth2/token";

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

impl Online {
    pub fn new(cache: Option<PathBuf>, max_age: Duration, offline: bool) -> Online {
        Online::with_api(Api::Lcsc, cache, max_age, offline)
    }

    pub fn with_api(api: Api, cache: Option<PathBuf>, max_age: Duration, offline: bool) -> Online {
        Online {
            endpoint: api.default_endpoint().to_string(),
            api,
            cache,
            max_age,
            offline,
            agent: None,
            token: None,
            last_request: None,
            caveats: BTreeMap::new(),
        }
    }

    pub fn default_cache_dir() -> Option<PathBuf> {
        Online::cache_dir_for("lcsc")
    }

    pub fn cache_dir_for(name: &str) -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
        Some(base.join("gerberus").join(name))
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
        let name = self.api.name();
        let agent = self.agent();
        let (status, body) = match self.api.clone() {
            Api::Lcsc => {
                let url = format!("{}{code}", self.endpoint);
                read(agent.get(&url).call(), name, code)?
            }
            Api::Mouser { key } => {
                if let Some(t) = self.last_request {
                    std::thread::sleep(MOUSER_SPACING_UNDER_30_PER_MINUTE.saturating_sub(t.elapsed()));
                }
                self.last_request = Some(std::time::Instant::now());
                let url = format!("{}?apiKey={}", self.endpoint, encode(&key));
                let body = serde_json::json!({
                    "SearchByPartRequest": { "mouserPartNumber": code, "partSearchOptions": "Exact" }
                });
                read(
                    agent
                        .post(&url)
                        .header("Content-Type", "application/json")
                        .header("Accept", "application/json")
                        .send(body.to_string()),
                    name,
                    code,
                )?
            }
            Api::DigiKey {
                client_id,
                client_secret,
            } => {
                let token = self.digikey_token(&agent, &client_id, &client_secret)?;
                let url = format!("{}/{}/productdetails", self.endpoint, encode(code));
                read(
                    agent
                        .get(&url)
                        .header("Authorization", format!("Bearer {token}"))
                        .header("X-DIGIKEY-Client-Id", client_id.as_str())
                        .header("X-DIGIKEY-Locale-Site", "US")
                        .header("X-DIGIKEY-Locale-Language", "en")
                        .header("X-DIGIKEY-Locale-Currency", "USD")
                        .header("Accept", "application/json")
                        .call(),
                    name,
                    code,
                )?
            }
        };
        if status == 404 && matches!(self.api, Api::DigiKey { .. }) {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            let short: String = body.chars().take(300).collect();
            return Err(format!("{name} answered HTTP {status} for {code}: {short}"));
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("{name}'s answer for {code} is not JSON: {e}"))?;
        match &self.api {
            Api::Lcsc => Listing::from_lcsc_json(&json),
            Api::Mouser { .. } => Listing::from_mouser_json(&json, code),
            Api::DigiKey { .. } => Listing::from_digikey_json(&json),
        }
        .map_err(|e| format!("{code}: {e}"))
    }

    fn digikey_token(&mut self, agent: &ureq::Agent, id: &str, secret: &str) -> Result<String, String> {
        if let Some(t) = &self.token {
            return Ok(t.clone());
        }
        let form = format!(
            "client_id={}&client_secret={}&grant_type=client_credentials",
            encode(id),
            encode(secret)
        );
        let (status, body) = read(
            agent
                .post(DIGIKEY_TOKEN)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .send(form),
            "DigiKey",
            "an access token",
        )?;
        if status != 200 {
            let short: String = body.chars().take(300).collect();
            return Err(format!(
                "DigiKey refused the client id and secret (HTTP {status}): {short}"
            ));
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("DigiKey's token answer is not JSON: {e}"))?;
        let token = text(json.get("access_token"));
        if token.is_empty() {
            return Err("DigiKey's token answer has no access_token".into());
        }
        self.token = Some(token.clone());
        Ok(token)
    }
}

fn read(
    resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    name: &str,
    what: &str,
) -> Result<(u16, String), String> {
    let mut resp = resp.map_err(|e| format!("asking {name} about {what}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("reading {name}'s answer for {what}: {e}"))?;
    Ok((status, body))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Source for Online {
    fn name(&self) -> &'static str {
        self.api.name()
    }

    fn by_mpn(&self) -> bool {
        !matches!(self.api, Api::Lcsc)
    }

    fn lists_similar(&self) -> bool {
        matches!(self.api, Api::Lcsc)
    }

    fn caveat(&self, code: &str) -> Option<String> {
        self.caveats.get(code).cloned()
    }

    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String> {
        let name = self.api.name();
        self.caveats.remove(code);
        if self.offline {
            return self
                .read_cache(code)?
                .map(|c| c.listing)
                .ok_or_else(|| format!("{code} is not in the {name} cache and lookups are offline"));
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
                        format!("using {name}'s answer from {hours} h ago, because a fresh lookup failed: {e}"),
                    );
                    Ok(c.listing)
                }
                None => Err(e),
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct FixedByMpn(pub BTreeMap<String, Option<Listing>>);

impl Source for FixedByMpn {
    fn name(&self) -> &'static str {
        "Mouser"
    }

    fn by_mpn(&self) -> bool {
        true
    }

    fn lists_similar(&self) -> bool {
        false
    }

    fn listing(&mut self, code: &str) -> Result<Option<Listing>, String> {
        self.0
            .get(code)
            .cloned()
            .ok_or_else(|| format!("no listing recorded for {code}"))
    }
}

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

pub fn lcsc_search_url(query: &str) -> String {
    format!("https://www.lcsc.com/search?q={}", encode(query))
}

pub fn check(design: &Design, source: &mut dyn Source, opts: Options) -> Vec<Health> {
    check_with_parts(design, source, opts, None)
}

pub fn check_with_parts(design: &Design, source: &mut dyn Source, opts: Options, lib: Option<&Library>) -> Vec<Health> {
    let lines = lines(design);
    let by_mpn = source.by_mpn();
    let lcsc = !by_mpn && source.lists_similar();
    let key = |l: &Line| -> Option<String> {
        if by_mpn {
            l.has_mpn_field.then(|| l.mpn.clone())
        } else {
            l.lcsc.clone()
        }
    };
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
                (None, _) if by_mpn => {
                    findings.push(Finding::new(
                        Severity::Unknown,
                        Topic::Identity,
                        format!(
                            "no MPN field, so {} can't look up {}: add the manufacturer part number",
                            source.name(),
                            line.mpn
                        ),
                    ));
                    None
                }
                (_, Some(bad)) if !by_mpn => {
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
                            format!(
                                "{} has no listing for {code}: it may be delisted, or the number is wrong",
                                source.name()
                            ),
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
                let name = source.name().to_string();
                let from = Judge {
                    name: &name,
                    lists_similar: source.lists_similar(),
                    by_mpn,
                };
                let want = lcsc.then(|| want_spec(design, &line, lib)).flatten();
                judge(&line, l, per_board, &from, opts, want.as_ref(), source, &mut findings);
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
    if lcsc && opts.jlcpcb {
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

struct Judge<'a> {
    name: &'a str,
    lists_similar: bool,
    by_mpn: bool,
}

fn lifecycle_severity(status: &str) -> Option<(Severity, &'static str)> {
    let s = status.to_ascii_lowercase();
    let has = |w: &str| s.contains(w);
    let word = |w: &str| s.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w);
    if has("obsolete") {
        Some((Severity::Error, "the manufacturer no longer makes it"))
    } else if has("inactive") || has("not active") || has("not in production") {
        Some((Severity::Warning, "it isn't in production"))
    } else if has("discontinued") || has("end of life") || word("eol") || word("ltb") || has("last time buy") {
        Some((Severity::Warning, "it is being discontinued"))
    } else if has("not recommended") || has("not for new") || word("nrnd") {
        Some((
            Severity::Warning,
            "the manufacturer doesn't recommend it for new designs",
        ))
    } else if has("preliminary") || has("preview") || has("sample") || has("pre-production") || has("preproduction") {
        Some((Severity::Info, "it isn't in full production yet"))
    } else if word("active") || has("new product") || has("new at") || has("in production") {
        None
    } else {
        Some((Severity::Unknown, "gerberus doesn't know this lifecycle status"))
    }
}

const EQUIVALENT_LOOKUPS: usize = 8;

#[expect(
    clippy::too_many_arguments,
    reason = "unrelated inputs; a one-use struct would only rename them"
)]
fn judge(
    line: &Line,
    l: &Listing,
    per_board: u64,
    from: &Judge<'_>,
    opts: Options,
    want: Option<&Spec>,
    source: &mut dyn Source,
    out: &mut Vec<Finding>,
) {
    let src = from.name;
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

    if from.by_mpn {
        match &l.lifecycle {
            None => {
                push(
                    out,
                    Severity::Info,
                    Topic::Lifecycle,
                    format!("{src} gives no lifecycle status for {code}"),
                );
            }
            Some(status) => {
                if let Some((sev, why)) = lifecycle_severity(status) {
                    push(
                        out,
                        sev,
                        Topic::Lifecycle,
                        format!("{src} lists {code} as {status:?}: {why}"),
                    );
                }
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

    if !from.lists_similar {
        push(
            out,
            Severity::Info,
            Topic::SecondSource,
            format!("{src} lists no similar parts, so second sources aren't checked"),
        );
        return;
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

fn listing_note(h: &Health, source: &str) -> Option<String> {
    Some(match &h.listing {
        Some(l) if l.lifecycle.is_none() && source == "LCSC" => format!(
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
        ),
        Some(l) => format!(
            "{source} {} is {} {}: {} in stock, lifecycle {}",
            l.code,
            l.manufacturer,
            l.mpn,
            l.stock,
            l.lifecycle.as_deref().unwrap_or("not given")
        ),
        None => return None,
    })
}

pub fn render(report: &[Health], opts: Options, source: &str, show: &crate::diag::Show) -> String {
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
            d.notes = listing_note(h, source).into_iter().collect();
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
            let mut notes: Vec<String> = listing_note(h, source).into_iter().collect();
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
                let what = listing_note(h, source).unwrap_or_default();
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
    if source == "LCSC" {
        if opts.jlcpcb {
            note(&mut out, JLCPCB_STOCK_NOTE);
        }
        note(&mut out, LIFECYCLE_NOTE);
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

pub const LIFECYCLE_NOTE: &str = "manufacturer lifecycle (active, NRND, EOL) is UNKNOWN for every part: LCSC only \
says whether it still lists a part. --from mouser or --from digikey gives it, with an API key.";

pub const MANUFACTURER_FIELDS: [&str; 4] = ["Manufacturer", "MFR", "Mfr", "Manufacturer_Name"];

fn csv_cell(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

pub fn order_csv(design: &Design, boards: u64) -> String {
    let mut out =
        String::from("Quantity,Manufacturer Part Number,Manufacturer,Reference Designators,Value,LCSC Part Number\n");
    for l in lines(design) {
        let first = &design.components[&l.references[0]];
        let maker = MANUFACTURER_FIELDS
            .iter()
            .filter_map(|f| first.fields.get(*f))
            .map(|s| s.trim())
            .find(|s| !s.is_empty() && *s != "~")
            .unwrap_or_default();
        let row = [
            (l.references.len() as u64).saturating_mul(boards).to_string(),
            if l.has_mpn_field { l.mpn.clone() } else { String::new() },
            maker.to_string(),
            l.references.join(" "),
            first.value.clone(),
            l.lcsc.clone().unwrap_or_default(),
        ];
        let cells: Vec<String> = row.iter().map(|c| csv_cell(c)).collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Severity, lifecycle_severity};

    #[test]
    fn lifecycle_words() {
        let sev = |s: &str| lifecycle_severity(s).map(|(s, _)| s);
        assert_eq!(sev("Active"), None);
        assert_eq!(sev("New Product"), None);
        assert_eq!(sev("In Production"), None);
        assert_eq!(sev("Obsolete"), Some(Severity::Error));
        assert_eq!(sev("Inactive"), Some(Severity::Warning));
        assert_eq!(sev("Not Active"), Some(Severity::Warning));
        assert_eq!(sev("Not in Production"), Some(Severity::Warning));
        assert_eq!(sev("Last Time Buy"), Some(Severity::Warning));
        assert_eq!(sev("EOL"), Some(Severity::Warning));
        assert_eq!(sev("Not Recommended for New Designs"), Some(Severity::Warning));
        assert_eq!(sev("Pre-Production"), Some(Severity::Info));
        assert_eq!(sev("Preliminary"), Some(Severity::Info));
        assert_eq!(sev("Geolocated"), Some(Severity::Unknown));
    }
}
