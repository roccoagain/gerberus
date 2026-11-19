use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Deserialize;

use crate::limits::{Class, Datasheet, Part};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Family {
    pub family: String,
    pub manufacturer: String,
    pub description: String,
    pub class: Class,
    pub mpn: String,
    pub datasheet: Datasheet,
    pub field: BTreeMap<String, Field>,
    #[serde(default)]
    pub rating: Vec<Rating>,
    #[serde(default)]
    pub tcr: Vec<TcrRow>,
    #[serde(default)]
    pub range: Vec<RangeRow>,
    pub rcwv_page: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeRow {
    #[serde(default)]
    pub when: When,
    pub ohms: Ohms,
    pub page: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub page: u32,
    #[serde(default)]
    pub codes: Codes,
    #[serde(default)]
    pub values: BTreeMap<String, f64>,
    pub kind: Option<ValueKind>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Codes {
    List(Vec<String>),
    Values(BTreeMap<String, f64>),
}

impl Default for Codes {
    fn default() -> Codes {
        Codes::List(vec![])
    }
}

impl Codes {
    fn all(&self) -> Vec<&str> {
        match self {
            Codes::List(l) => l.iter().map(String::as_str).collect(),
            Codes::Values(m) => m.keys().map(String::as_str).collect(),
        }
    }

    fn value(&self, code: &str) -> Option<f64> {
        match self {
            Codes::List(_) => None,
            Codes::Values(m) => m.get(code).copied(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValueKind {
    Resistance,
    ResistanceDigits,
    CapacitancePf,
}

type When = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ohms {
    pub min: Option<f64>,
    pub above: Option<f64>,
    pub max: Option<f64>,
    pub below: Option<f64>,
}

impl Ohms {
    fn contains(&self, r: f64) -> bool {
        self.min.is_none_or(|m| r >= m)
            && self.above.is_none_or(|m| r > m)
            && self.max.is_none_or(|m| r <= m)
            && self.below.is_none_or(|m| r < m)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinMax {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rating {
    #[serde(default)]
    pub when: When,
    pub ohms: Option<Ohms>,
    pub power: Option<f64>,
    pub max_working_voltage: Option<f64>,
    pub voltage: Option<f64>,
    pub temperature: Option<MinMax>,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcrRow {
    #[serde(default)]
    pub when: When,
    pub ohms: Option<Ohms>,
    pub ppm: f64,
    pub page: u32,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub codes: BTreeMap<String, String>,
    pub value: Option<f64>,
}

enum Piece<'a> {
    Text(&'a str),
    Field(&'a str),
}

impl Family {
    fn pieces(&self) -> Vec<Piece<'_>> {
        let mut out = vec![];
        let mut rest = self.mpn.as_str();
        while let Some(open) = rest.find('{') {
            if open > 0 {
                out.push(Piece::Text(&rest[..open]));
            }
            let close = rest[open..].find('}').map_or(rest.len(), |c| open + c);
            out.push(Piece::Field(&rest[open + 1..close]));
            rest = rest.get(close + 1..).unwrap_or("");
        }
        if !rest.is_empty() {
            out.push(Piece::Text(rest));
        }
        out
    }

    pub fn problems(&self) -> Vec<String> {
        let mut out = vec![];
        let pieces = self.pieces();
        for p in &pieces {
            if let Piece::Field(f) = p
                && !self.field.contains_key(*f)
            {
                out.push(format!("mpn names field {{{f}}}, which has no [field.{f}]"));
            }
        }
        for (name, f) in &self.field {
            if f.kind.is_none() && f.codes.all().is_empty() {
                out.push(format!("[field.{name}] needs `codes` or a value `kind`"));
            }
            if f.page == 0 {
                out.push(format!("[field.{name}] needs the page that explains its codes"));
            }
        }
        let rows = self
            .rating
            .iter()
            .map(|r| (&r.when, r.page))
            .chain(self.tcr.iter().map(|t| (&t.when, t.page)))
            .chain(self.range.iter().map(|t| (&t.when, t.page)));
        for (when, page) in rows {
            if page == 0 {
                out.push("every rating and tcr row needs a page".into());
            }
            for (name, codes) in when {
                match self.field.get(name) {
                    None => out.push(format!("a row's `when` names {name:?}, which isn't a field")),
                    Some(f) => {
                        let all = f.codes.all();
                        for c in codes {
                            if !all.contains(&c.as_str()) {
                                out.push(format!(
                                    "a row's `when` gives {name} {c:?}, which isn't one of its codes"
                                ));
                            }
                        }
                    }
                }
            }
        }
        match self.class {
            Class::Resistor => {
                if !self.field.contains_key("tolerance")
                    || !self
                        .field
                        .values()
                        .any(|f| matches!(f.kind, Some(ValueKind::Resistance | ValueKind::ResistanceDigits)))
                {
                    out.push("a resistor family needs a `tolerance` field and a field of kind \"resistance\" or \"resistance-digits\"".into());
                }
            }
            Class::Capacitor => {
                if !self.field.contains_key("tolerance") {
                    out.push("a capacitor family needs a `tolerance` field".into());
                }
            }
            _ => out.push("only resistor and capacitor families are supported".into()),
        }
        out
    }

    pub fn decode(&self, mpn: &str) -> Option<Result<Decoded, String>> {
        let pieces = self.pieces();
        let mut found = vec![];
        self.matches(&pieces, mpn, &mut BTreeMap::new(), &mut found);
        let codes = match found.len() {
            0 => return None,
            1 => found.remove(0),
            _ => {
                return Some(Err(format!(
                    "{} family: {mpn} can be read {} ways ({})",
                    self.family,
                    found.len(),
                    found
                        .iter()
                        .map(|c| c.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", "))
                        .collect::<Vec<_>>()
                        .join("; ")
                )));
            }
        };
        let value = self
            .field
            .iter()
            .find_map(|(name, f)| f.kind.map(|k| (name, k)))
            .and_then(|(name, k)| parse_value(k, &codes[name]));
        Some(Ok(Decoded { codes, value }))
    }

    fn matches(
        &self,
        pieces: &[Piece<'_>],
        rest: &str,
        codes: &mut BTreeMap<String, String>,
        found: &mut Vec<BTreeMap<String, String>>,
    ) {
        if found.len() >= 2 {
            return;
        }
        let Some((first, more)) = pieces.split_first() else {
            if rest.is_empty() {
                found.push(codes.clone());
            }
            return;
        };
        match first {
            Piece::Text(t) => {
                if let Some(r) = rest.strip_prefix(t) {
                    self.matches(more, r, codes, found);
                }
            }
            Piece::Field(name) => {
                let f = &self.field[*name];
                let candidates: Vec<usize> = match f.kind {
                    Some(k) => (1..=rest.len())
                        .filter(|&n| rest.is_char_boundary(n) && parse_value(k, &rest[..n]).is_some())
                        .collect(),
                    None => f
                        .codes
                        .all()
                        .iter()
                        .filter(|c| rest.starts_with(**c))
                        .map(|c| c.len())
                        .collect(),
                };
                for n in candidates {
                    codes.insert((*name).to_string(), rest[..n].to_string());
                    self.matches(more, &rest[n..], codes, found);
                }
                codes.remove(*name);
            }
        }
    }

    fn row_applies(when: &When, ohms: Option<&Ohms>, d: &Decoded) -> bool {
        when.iter()
            .all(|(f, allowed)| d.codes.get(f).is_some_and(|c| allowed.contains(c)))
            && ohms.is_none_or(|o| d.value.is_some_and(|v| o.contains(v)))
    }

    pub fn part_text(&self, mpn: &str) -> Option<Result<String, String>> {
        Some(self.decode(mpn)?.and_then(|d| self.render(mpn, &d)))
    }

    pub fn part(&self, mpn: &str) -> Option<Result<Part, String>> {
        let text = match self.part_text(mpn)? {
            Ok(t) => t,
            Err(e) => return Some(Err(e)),
        };
        Some(
            toml::from_str::<Part>(&text)
                .map_err(|e| format!("{} family: the part file for {mpn} doesn't load: {e}", self.family)),
        )
    }

    fn render(&self, mpn: &str, d: &Decoded) -> Result<String, String> {
        let codes_text = d
            .codes
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ");
        let pick = |what: &str, has: &dyn Fn(&Rating) -> bool| -> Result<Option<&Rating>, String> {
            let rows: Vec<&Rating> = self
                .rating
                .iter()
                .filter(|r| has(r) && Self::row_applies(&r.when, r.ohms.as_ref(), d))
                .collect();
            match rows.as_slice() {
                [] => Ok(None),
                [r] => Ok(Some(*r)),
                _ => Err(format!(
                    "{} family: {} rows give {mpn} ({codes_text}) its {what}, on pages {}",
                    self.family,
                    rows.len(),
                    rows.iter().map(|r| r.page.to_string()).collect::<Vec<_>>().join(", ")
                )),
            }
        };
        let missing = |what: &str| format!("{} family: no row gives {mpn} ({codes_text}) its {what}", self.family);
        if self.class == Class::Resistor && d.value.is_some_and(|v| v <= 0.0) {
            return Err(format!(
                "{} family: {mpn} is a 0 Ω jumper, which has a current rating, not a power one",
                self.family
            ));
        }
        if !self.range.is_empty() && !self.range.iter().any(|r| Self::row_applies(&r.when, Some(&r.ohms), d)) {
            return Err(format!(
                "{} family: the datasheet doesn't list {mpn} ({codes_text}): its value is outside the range made in \
                 that size and tolerance",
                self.family
            ));
        }
        let temperature = pick("temperature range", &|r| r.temperature.is_some())?;
        let tol_field = &self.field["tolerance"];
        let tol_code = &d.codes["tolerance"];
        let tol = tol_field
            .codes
            .value(tol_code)
            .or_else(|| tol_field.values.get(tol_code).copied());
        let ds = &self.datasheet;
        let mut t = String::new();
        let note = |n: &Option<String>| n.as_ref().map(|n| format!(", note = {}", q(n))).unwrap_or_default();
        let _ = writeln!(t, "mpn = {}", q(mpn));
        let _ = writeln!(t, "manufacturer = {}", q(&self.manufacturer));
        let size = d.codes.get("size").map(|s| format!(", {s}")).unwrap_or_default();
        match self.class {
            Class::Resistor => {
                let ohms = d
                    .value
                    .ok_or_else(|| format!("{} family: can't read the resistance in {mpn}", self.family))?;
                if ohms <= 0.0 {
                    return Err(format!(
                        "{} family: {mpn} is a 0 Ω jumper, which has a current rating, not a power one",
                        self.family
                    ));
                }
                let _ = writeln!(
                    t,
                    "description = {}",
                    q(&format!(
                        "{} {}{size} ({}, from the part number)",
                        fmt_ohms(ohms),
                        self.description,
                        self.family
                    ))
                );
                let _ = writeln!(t, "class = \"resistor\"");
                if let Some(v) = tol {
                    let _ = writeln!(
                        t,
                        "tolerance = {{ value = {v}, page = {}, note = {} }}",
                        tol_field.page,
                        q(&format!("tolerance code {tol_code}"))
                    );
                }
                let tcrs: Vec<&TcrRow> = self
                    .tcr
                    .iter()
                    .filter(|r| Self::row_applies(&r.when, r.ohms.as_ref(), d))
                    .collect();
                match tcrs.as_slice() {
                    [r] => {
                        let _ = writeln!(t, "tcr = {{ ppm = {}, page = {}{} }}", r.ppm, r.page, note(&r.note));
                    }
                    [] if self.tcr.is_empty() => {}
                    [] => return Err(missing("temperature coefficient")),
                    rows => {
                        return Err(format!(
                            "{} family: {} rows give {mpn} ({codes_text}) its temperature coefficient, on pages {}",
                            self.family,
                            rows.len(),
                            rows.iter().map(|r| r.page.to_string()).collect::<Vec<_>>().join(", ")
                        ));
                    }
                }
                temperature_line(&mut t, temperature);
                datasheet_section(&mut t, ds);
                let rating = pick("power", &|r| r.power.is_some())?.ok_or_else(|| missing("power"))?;
                let p = rating.power.unwrap_or_default();
                let mwv_row = pick("maximum working voltage", &|r| r.max_working_voltage.is_some())?;
                let rcwv = (p * ohms).sqrt();
                let limit = match mwv_row.and_then(|r| r.max_working_voltage) {
                    Some(mwv) if mwv <= rcwv => format!(
                        "recommended = {{ min = -{mwv}, max = {mwv}, page = {}, note = {} }}",
                        mwv_row.map_or(0, |r| r.page),
                        q(&format!(
                            "the size's maximum working voltage, below sqrt(P x R) = {} V; non-polar, so both directions",
                            round(rcwv)
                        ))
                    ),
                    mwv => format!(
                        "recommended = {{ min = {lo}, max = {hi}, page = {}, derived = {}, note = {} }}",
                        self.rcwv_page.unwrap_or(rating.page),
                        q(&format!(
                            "-sqrt({p} * {ohms}) .. sqrt({p} * {ohms}); RCWV = sqrt(P x R), with P = {p} W from p{} and R from the part number{}; non-polar, so the same magnitude applies in both directions",
                            rating.page,
                            mwv.map(|m| format!(", below the size's {m} V maximum working voltage"))
                                .unwrap_or_default()
                        )),
                        q(&format!("rated continuous working voltage at P = {p} W")),
                        lo = -toward_zero(rcwv),
                        hi = toward_zero(rcwv),
                    ),
                };
                let _ = writeln!(t, "\n[pins]\n1 = \"1\"\n2 = \"2\"\n");
                let _ = writeln!(
                    t,
                    "[[limit]]\npins = [\"1\"]\nto = \"2\"\nabs_max = {{ unknown = \"the datasheet gives an overload test, not an absolute maximum\" }}\n{limit}\n"
                );
                let _ = writeln!(
                    t,
                    "[[limit]]\nquantity = \"power\"\npins = [\"1\"]\nto = \"2\"\nabs_max = {{ unknown = \"the datasheet gives only the power rating\" }}\nrecommended = {{ min = 0, max = {p}, page = {}{} }}",
                    rating.page,
                    note(&rating.note)
                );
            }
            Class::Capacitor => {
                let rating =
                    pick("rated voltage", &|r| r.voltage.is_some())?.ok_or_else(|| missing("rated voltage"))?;
                let v = rating.voltage.unwrap_or_default();
                let farads = d.value.map(fmt_farads).unwrap_or_default();
                let _ = writeln!(
                    t,
                    "description = {}",
                    q(format!(
                        "{farads} {v} V {}{size} ({}, from the part number)",
                        self.description, self.family
                    )
                    .trim_start())
                );
                let _ = writeln!(t, "class = \"capacitor\"");
                if let Some(tv) = tol.filter(|_| {
                    d.value
                        .is_none_or(|f| f >= SMALLEST_CAPACITANCE_WITH_FRACTIONAL_TOLERANCE)
                }) {
                    let _ = writeln!(
                        t,
                        "tolerance = {{ value = {tv}, page = {}, note = {} }}",
                        tol_field.page,
                        q(&format!("tolerance code {tol_code}"))
                    );
                }
                temperature_line(&mut t, temperature);
                datasheet_section(&mut t, ds);
                let _ = writeln!(t, "\n[pins]\n1 = \"1\"\n2 = \"2\"\n");
                let _ = writeln!(
                    t,
                    "[[limit]]\npins = [\"1\"]\nto = \"2\"\nabs_max = {{ unknown = \"the datasheet gives a rated voltage and a withstanding-voltage test, not an absolute maximum\" }}\nrecommended = {{ min = -{v}, max = {v}, page = {}{} }}",
                    rating.page,
                    note(&rating.note)
                );
            }
            _ => {
                return Err(format!(
                    "{} family: only resistors and capacitors are supported",
                    self.family
                ));
            }
        }
        Ok(t)
    }
}

fn temperature_line(t: &mut String, row: Option<&Rating>) {
    if let Some(r) = row
        && let Some(tr) = &r.temperature
    {
        let note = r
            .note
            .as_ref()
            .map(|n| format!(", note = {}", q(n)))
            .unwrap_or_default();
        let _ = writeln!(
            t,
            "temperature = {{ min = {}, max = {}, page = {}{note} }}",
            tr.min, tr.max, r.page
        );
    }
}

fn datasheet_section(t: &mut String, ds: &Datasheet) {
    let _ = writeln!(
        t,
        "\n[datasheet]\ntitle = {}\ndocument = {}\nurl = {}",
        q(&ds.title),
        q(&ds.document),
        q(&ds.url)
    );
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn toward_zero(v: f64) -> f64 {
    (v * 1000.0).trunc() / 1000.0
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn fmt_ohms(r: f64) -> String {
    let (v, unit) = if r >= 1e6 {
        (r / 1e6, "Mohm")
    } else if r >= 1e3 {
        (r / 1e3, "kohm")
    } else {
        (r, "ohm")
    };
    format!("{} {unit}", round(v))
}

fn fmt_farads(f: f64) -> String {
    let (v, unit) = if f >= 1e-6 {
        (f * 1e6, "uF")
    } else if f >= 1e-9 {
        (f * 1e9, "nF")
    } else {
        (f * 1e12, "pF")
    };
    format!("{} {unit}", round(v))
}

pub fn parse_value(kind: ValueKind, s: &str) -> Option<f64> {
    match kind {
        ValueKind::Resistance => {
            let at = s.find(['R', 'K', 'M'])?;
            let (int, frac) = (&s[..at], &s[at + 1..]);
            let digits = |x: &str| x.chars().all(|c| c.is_ascii_digit());
            if !digits(int) || !digits(frac) || (int.is_empty() && frac.is_empty()) {
                return None;
            }
            let mult = match &s[at..=at] {
                "R" => 1.0,
                "K" => 1e3,
                _ => 1e6,
            };
            let v: f64 = format!(
                "{}.{}",
                if int.is_empty() { "0" } else { int },
                if frac.is_empty() { "0" } else { frac }
            )
            .parse()
            .ok()?;
            Some(v * mult)
        }
        ValueKind::ResistanceDigits => {
            let b = s.as_bytes();
            if b.len() != 4 || !b[..3].iter().all(u8::is_ascii_digit) {
                return None;
            }
            let exp = match b[3] {
                d @ b'0'..=b'6' => i32::from(d - b'0'),
                b'J' => -1,
                b'K' => -2,
                b'L' => -3,
                b'M' => -4,
                b'N' => -5,
                b'P' => -6,
                _ => return None,
            };
            let sig: f64 = s[..3].parse().ok()?;
            Some(round_to_picohm(sig * 10f64.powi(exp)))
        }
        ValueKind::CapacitancePf => {
            let digits = s.chars().filter(char::is_ascii_digit).count();
            let rs = s.matches('R').count();
            if s.len() != 3 || digits + rs != 3 || rs > 1 || s.ends_with('R') {
                return None;
            }
            if rs == 1 {
                let v: f64 = s.replacen('R', ".", 1).parse().ok()?;
                return (v > 0.0).then_some(v * 1e-12);
            }
            let sig: f64 = s[..2].parse().ok()?;
            let exp: i32 = s[2..].parse().ok()?;
            (sig > 0.0 && exp <= LARGEST_EIA_CAPACITANCE_EXPONENT).then(|| sig * 10f64.powi(exp) * 1e-12)
        }
    }
}

const SMALLEST_CAPACITANCE_WITH_FRACTIONAL_TOLERANCE: f64 = 10e-12;
const LARGEST_EIA_CAPACITANCE_EXPONENT: i32 = 7;

fn round_to_picohm(ohms: f64) -> f64 {
    (ohms * 1e12).round() / 1e12
}
