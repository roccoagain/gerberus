//! What a plain resistor or capacitor is (value, size and ratings) read two ways: from the schematic and the
//! part file of its `MPN` field, and from LCSC's listing of an LCSC number. Comparing the two tells whether an
//! LCSC number is the part the schematic means, and which of LCSC's similar parts could replace it.
use std::collections::BTreeMap;

use crate::design::Component;
use crate::limits::{Part, Quantity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Resistor,
    Capacitor,
}

impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Kind::Resistor => "resistor",
            Kind::Capacitor => "capacitor",
        }
    }
}

/// Everything that is `None` wasn't given, so it isn't compared.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spec {
    pub kind: Option<Kind>,
    /// Ohms or farads.
    pub value: Option<f64>,
    /// Imperial chip size, such as `0603`.
    pub size: Option<String>,
    /// As a fraction: 0.01 is ±1 %.
    pub tolerance: Option<f64>,
    /// A capacitor's rated voltage.
    pub voltage: Option<f64>,
    /// A resistor's rated power, in watts.
    pub power: Option<f64>,
    /// A ceramic capacitor's dielectric, such as `X7R` (`NP0` is written `C0G`).
    pub dielectric: Option<String>,
}

/// Ceramic dielectrics from the most stable to the least. One further down the list is a worse part.
const DIELECTRICS: [&str; 9] = ["C0G", "X8R", "X7R", "X7S", "X6S", "X5R", "X7T", "Y5V", "Z5U"];

fn dielectric_in(text: &str) -> Option<String> {
    let up = text.to_ascii_uppercase();
    up.split(|c: char| !c.is_ascii_alphanumeric()).find_map(|w| match w {
        "NP0" | "NPO" | "C0G" | "COG" => Some("C0G".to_string()),
        w if DIELECTRICS.contains(&w) => Some(w.to_string()),
        _ => None,
    })
}

/// `0603` from a KiCad footprint such as `Resistor_SMD:R_0603_1608Metric`, or a tscircuit one such as `res0603`,
/// `cap0603_nonpolarized` or `0603`.
pub fn footprint_size(footprint: &str) -> Option<String> {
    let name = footprint.rsplit(':').next()?;
    let words: Vec<&str> = name.split('_').collect();
    let four_digits = |w: &str| w.len() == 4 && w.bytes().all(|b| b.is_ascii_digit());
    let kicad = words
        .windows(2)
        .find_map(|w| (four_digits(w[0]) && w[1].ends_with("Metric")).then(|| w[0].to_string()));
    // tscircuit names a chip part by its imperial size, after its kind for a resistor, capacitor or diode.
    let tscircuit = || {
        ["res", "cap", "led", "diode", ""]
            .iter()
            .filter_map(|kind| words.first()?.strip_prefix(kind))
            .find(|size| four_digits(size))
            .map(str::to_string)
    };
    kicad.or_else(tscircuit)
}

/// A number with an optional SI prefix and unit, as LCSC writes parameters: `25V`, `100mW`, `1/10W`, `1kV`.
fn si_number(text: &str, unit: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches(unit).trim();
    if let Some((a, b)) = t.split_once('/') {
        return Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?);
    }
    let (num, mult) = match t.chars().last()? {
        'm' => (&t[..t.len() - 1], 1e-3),
        'k' | 'K' => (&t[..t.len() - 1], 1e3),
        'u' | 'µ' | 'μ' => (&t[..t.len() - t.chars().last()?.len_utf8()], 1e-6),
        _ => (t, 1.0),
    };
    let v = num.trim().parse::<f64>().ok()? * mult;
    v.is_finite().then_some(v)
}

/// `±1%` as 0.01. An absolute tolerance such as `±0.25pF` gives none.
fn percent(text: &str) -> Option<f64> {
    let t = text.trim().trim_start_matches('±').trim();
    let n = t.strip_suffix('%')?;
    Some(n.trim().parse::<f64>().ok()? / 100.0)
}

fn is_size(s: &str) -> bool {
    s.len() == 4 && s.bytes().all(|b| b.is_ascii_digit())
}

impl Spec {
    /// From LCSC's parameter table and package. Gives no kind (so nothing is compared) for a listing that is not
    /// a resistor or capacitor, or that has no parameters, such as one cached by an older gerberus.
    pub fn from_lcsc(params: &BTreeMap<String, String>, package: &str) -> Spec {
        let get = |k: &str| params.get(k).map(String::as_str);
        let mut s = Spec {
            size: is_size(package.trim()).then(|| package.trim().to_string()),
            tolerance: get("Tolerance").and_then(percent),
            ..Spec::default()
        };
        if let Some(r) = get("Resistance") {
            s.kind = Some(Kind::Resistor);
            // LCSC writes milliohms as `10mΩ`, which a schematic value never is.
            s.value = match r.trim().strip_suffix("mΩ") {
                Some(m) => m.trim().parse::<f64>().ok().map(|v| v * 1e-3),
                None => crate::units::parse_ohms(r),
            };
            s.power = get("Power(Watts)").and_then(|p| si_number(p, "W"));
        } else if let Some(c) = get("Capacitance") {
            s.kind = Some(Kind::Capacitor);
            s.value = crate::units::parse_farads(c);
            // A small capacitor's tolerance is in picofarads (`±0.25pF`): as a fraction of its value.
            if s.tolerance.is_none()
                && let (Some(t), Some(v)) = (get("Tolerance"), s.value)
                && let Some(pf) = t.trim().trim_start_matches('±').trim().strip_suffix("pF")
                && let Ok(pf) = pf.trim().parse::<f64>()
            {
                s.tolerance = Some(pf * 1e-12 / v);
            }
            s.voltage = get("Voltage Rating")
                .or(get("Rated Voltage"))
                .and_then(|v| si_number(v, "V"));
            s.dielectric = get("Temperature Coefficient").and_then(dielectric_in);
        }
        s
    }

    /// From a schematic part: its kind from the symbol, value from the value, size from the footprint, and
    /// ratings from its part file when there is one. Only plain resistors and capacitors have a kind.
    pub fn from_component(c: &Component, part: Option<&Part>) -> Spec {
        let symbol = c.lib_id.rsplit(':').next().unwrap_or("");
        let kind = if c.lib_id.starts_with("Device:R") {
            Kind::Resistor
        } else if c.lib_id.starts_with("Device:C") && !symbol.starts_with("C_Polarized") {
            Kind::Capacitor
        } else {
            return Spec::default();
        };
        let mut s = Spec {
            kind: Some(kind),
            value: match kind {
                Kind::Resistor => crate::units::parse_ohms(&c.value),
                Kind::Capacitor => crate::units::parse_farads(&c.value),
            },
            size: footprint_size(&c.footprint),
            ..Spec::default()
        };
        let Some(p) = part else { return s };
        s.tolerance = p.tolerance.as_ref().map(|t| t.value);
        let rated = |q: Quantity| {
            p.limits
                .iter()
                .filter(|l| l.quantity == q)
                .filter_map(|l| l.recommended.values())
                .map(|(lo, hi)| lo.abs().max(hi.abs()))
                .reduce(f64::min)
        };
        match kind {
            Kind::Resistor => s.power = rated(Quantity::Power),
            Kind::Capacitor => {
                s.voltage = rated(Quantity::Voltage);
                let temp_note = p.temperature.as_ref().and_then(|t| t.note.clone()).unwrap_or_default();
                s.dielectric = dielectric_in(&p.description).or_else(|| dielectric_in(&temp_note));
            }
        }
        s
    }

    /// The ratings a part of this kind needs before another can be shown to be as good: tolerance, and power for
    /// a resistor or rated voltage and dielectric for a capacitor.
    pub fn missing_ratings(&self) -> Vec<&'static str> {
        let mut m = vec![];
        if self.value.is_none() {
            m.push("value");
        }
        if self.size.is_none() {
            m.push("size");
        }
        if self.tolerance.is_none() {
            m.push("tolerance");
        }
        match self.kind {
            Some(Kind::Resistor) => {
                if self.power.is_none() {
                    m.push("rated power");
                }
            }
            Some(Kind::Capacitor) => {
                if self.voltage.is_none() {
                    m.push("rated voltage");
                }
                if self.dielectric.is_none() {
                    m.push("dielectric");
                }
            }
            None => m.push("kind"),
        }
        m
    }

    /// `2.2 Ω ±1% 100 mW 0603 resistor`.
    pub fn describe(&self) -> String {
        let mut words = vec![];
        if let Some(v) = self.value {
            words.push(match self.kind {
                Some(Kind::Resistor) => format!("{}Ω", eng(v)),
                Some(Kind::Capacitor) => format!("{}F", eng(v)),
                None => eng(v),
            });
        }
        if let Some(t) = self.tolerance {
            words.push(format!("±{}%", trim(t * 100.0)));
        }
        if let Some(v) = self.voltage {
            words.push(format!("{}V", eng(v)));
        }
        if let Some(p) = self.power {
            words.push(format!("{}W", eng(p)));
        }
        if let Some(d) = &self.dielectric {
            words.push(d.clone());
        }
        if let Some(s) = &self.size {
            words.push(s.clone());
        }
        if let Some(k) = self.kind {
            words.push(k.noun().to_string());
        }
        words.join(" ")
    }
}

/// How an offered part (`have`) stands against the one the schematic means (`want`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// It is a different part: another kind, value or size.
    pub different: Vec<String>,
    /// Same part, rated worse: looser tolerance, lower voltage or power, a less stable dielectric.
    pub worse: Vec<String>,
    /// What both sides gave, so was compared.
    pub compared: Vec<&'static str>,
    /// What only `have` gave, so couldn't be compared.
    pub unchecked: Vec<&'static str>,
}

impl Comparison {
    pub fn fits(&self) -> bool {
        self.different.is_empty() && self.worse.is_empty()
    }
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-3 * a.abs().max(b.abs()) + 1e-15
}

/// Compares an offered part with the wanted one. A rating the offered part doesn't give, when the wanted one
/// has it, counts as worse: it can't be shown to be good enough.
pub fn compare(want: &Spec, have: &Spec) -> Comparison {
    let mut c = Comparison::default();
    if let (Some(w), Some(h)) = (want.kind, have.kind)
        && w != h
    {
        c.different.push(format!("a {}, not a {}", h.noun(), w.noun()));
        return c;
    }
    match (want.value, have.value) {
        (Some(w), Some(h)) => {
            c.compared.push("value");
            if !close(w, h) {
                let unit = if want.kind == Some(Kind::Capacitor) { "F" } else { "Ω" };
                c.different.push(format!(
                    "{}{unit}, not the {}{unit} the schematic gives",
                    eng(h),
                    eng(w)
                ));
            }
        }
        (Some(_), None) => c.different.push("no value given".into()),
        (None, _) => c.unchecked.push("value"),
    }
    match (&want.size, &have.size) {
        (Some(w), Some(h)) => {
            c.compared.push("size");
            if w != h {
                c.different.push(format!("size {h}, not the {w} of the footprint"));
            }
        }
        (Some(w), None) => c
            .different
            .push(format!("no package size given, where the footprint is {w}")),
        (None, _) => c.unchecked.push("size"),
    }
    let mut rating =
        |name: &'static str, w: Option<f64>, h: Option<f64>, higher_is_better: bool, show: &dyn Fn(f64) -> String| {
            match (w, h) {
                (Some(w), Some(h)) => {
                    c.compared.push(name);
                    let worse = if higher_is_better {
                        h < w * (1.0 - 1e-6)
                    } else {
                        h > w * (1.0 + 1e-6)
                    };
                    if worse {
                        c.worse.push(format!(
                            "{name} {}, where the schematic's part has {}",
                            show(h),
                            show(w)
                        ));
                    }
                }
                (Some(w), None) => c
                    .worse
                    .push(format!("no {name} given, where the schematic's part has {}", show(w))),
                (None, _) => c.unchecked.push(name),
            }
        };
    rating("tolerance", want.tolerance, have.tolerance, false, &|t| {
        format!("±{}%", trim(t * 100.0))
    });
    if want.kind != Some(Kind::Resistor) {
        rating("rated voltage", want.voltage, have.voltage, true, &|v| {
            format!("{}V", eng(v))
        });
    }
    if want.kind != Some(Kind::Capacitor) {
        rating("rated power", want.power, have.power, true, &|p| format!("{}W", eng(p)));
    }
    if want.kind == Some(Kind::Capacitor) {
        match (&want.dielectric, &have.dielectric) {
            (Some(w), Some(h)) => {
                c.compared.push("dielectric");
                let rank = |d: &str| DIELECTRICS.iter().position(|x| *x == d);
                let worse = match (rank(w), rank(h)) {
                    (Some(a), Some(b)) => b > a,
                    _ => w != h,
                };
                if worse {
                    c.worse
                        .push(format!("dielectric {h}, where the schematic's part is {w}"));
                }
            }
            (Some(w), None) => c
                .worse
                .push(format!("no dielectric given, where the schematic's part is {w}")),
            (None, _) => c.unchecked.push("dielectric"),
        }
    }
    c
}

/// A number with an SI prefix: `2.2`, `10k`, `100n`, `100m`.
pub fn eng(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let prefixes = [
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
    ];
    let a = v.abs();
    let (scale, p) = prefixes
        .iter()
        .find(|(s, _)| a >= *s * 0.9995)
        .copied()
        .unwrap_or((1e-12, "p"));
    format!("{}{p}", trim(v / scale))
}

/// Up to three significant decimals, without trailing zeros.
fn trim(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn reads_lcsc_parameters() {
        let r = Spec::from_lcsc(
            &params(&[
                ("Resistance", "2.2Ω"),
                ("Tolerance", "±1%"),
                ("Power(Watts)", "100mW"),
                ("Voltage Rating", "75V"),
            ]),
            "0603",
        );
        assert_eq!(r.kind, Some(Kind::Resistor));
        assert_eq!(r.value, Some(2.2));
        assert_eq!(r.power, Some(0.1));
        assert_eq!(r.voltage, None);
        assert_eq!(r.describe(), "2.2Ω ±1% 100mW 0603 resistor");
        let c = Spec::from_lcsc(
            &params(&[
                ("Capacitance", "220nF"),
                ("Tolerance", "±10%"),
                ("Voltage Rating", "25V"),
                ("Temperature Coefficient", "X7R"),
            ]),
            "0603",
        );
        assert_eq!(c.describe(), "220nF ±10% 25V X7R 0603 capacitor");
        assert_eq!(si_number("1/10W", "W"), Some(0.1));
        assert_eq!(si_number("1kV", "V"), Some(1000.0));
        assert_eq!(percent("±0.25pF"), None);
        assert_eq!(dielectric_in("NP0"), Some("C0G".into()));
        assert_eq!(
            Spec::from_lcsc(&params(&[("Resistance", "10mΩ")]), "1206").value,
            Some(0.01)
        );
        let small = Spec::from_lcsc(&params(&[("Capacitance", "10pF"), ("Tolerance", "±0.5pF")]), "0402");
        assert!((small.tolerance.unwrap() - 0.05).abs() < 1e-9);
    }

    #[test]
    fn footprint_sizes() {
        assert_eq!(
            footprint_size("Resistor_SMD:R_0603_1608Metric").as_deref(),
            Some("0603")
        );
        assert_eq!(
            footprint_size("Capacitor_SMD:C_1210_3225Metric_Pad1.33x2.70mm_HandSolder").as_deref(),
            Some("1210")
        );
        assert_eq!(footprint_size("Capacitor_THT:CP_Radial_D5.0mm_P2.00mm"), None);
        assert_eq!(footprint_size("res0402").as_deref(), Some("0402"));
        assert_eq!(footprint_size("cap0805_nonpolarized").as_deref(), Some("0805"));
        assert_eq!(footprint_size("diode0603_color(red)").as_deref(), Some("0603"));
        assert_eq!(footprint_size("0805").as_deref(), Some("0805"));
        assert_eq!(footprint_size("soic8"), None);
    }

    #[test]
    fn comparisons() {
        let want = Spec {
            kind: Some(Kind::Capacitor),
            value: Some(220e-9),
            size: Some("0603".into()),
            tolerance: Some(0.1),
            voltage: Some(50.0),
            dielectric: Some("X7R".into()),
            ..Spec::default()
        };
        let mut have = want.clone();
        assert!(compare(&want, &have).fits());
        have.voltage = Some(25.0);
        let c = compare(&want, &have);
        assert!(c.different.is_empty());
        assert_eq!(c.worse, ["rated voltage 25V, where the schematic's part has 50V"]);
        have.voltage = Some(100.0);
        have.dielectric = Some("C0G".into());
        assert!(
            compare(&want, &have).fits(),
            "a higher rating and a better dielectric fit"
        );
        have.dielectric = Some("X5R".into());
        assert!(!compare(&want, &have).fits());
        have.value = Some(22e-9);
        assert_eq!(
            compare(&want, &have).different,
            ["22nF, not the 220nF the schematic gives"]
        );
        // A value-only line compares value and size, and leaves the ratings to the offered part.
        let bare = Spec {
            kind: Some(Kind::Resistor),
            value: Some(2.2),
            size: Some("0603".into()),
            ..Spec::default()
        };
        let r = Spec {
            power: Some(0.1),
            tolerance: Some(0.01),
            ..bare.clone()
        };
        let c = compare(&bare, &r);
        assert!(c.fits());
        assert_eq!(c.compared, ["value", "size"]);
        assert_eq!(c.unchecked, ["tolerance", "rated power"]);
    }
}
