//! The blind second reading: a blanked copy of a part file, and the differences between two readings.
use crate::limits::{Bound, Part, Range};
use std::fmt::Write as _;

use super::reading::close;

/// The keys of a part file whose values a second reader fills in.
const VALUE_KEYS: [&str; 21] = [
    "on",
    "swing",
    "efficiency",
    "theta_ja",
    "tj_max",
    "current",
    "description",
    "note",
    "volts",
    "page",
    "abs_max",
    "recommended",
    "breakdown",
    "clamp",
    "range",
    "headroom",
    "tolerance",
    "tcr",
    "temperature",
    "typ",
    "max",
];

/// A copy of a part file's TOML with every value from the datasheet replaced by `"?"`, for a second reader
/// to fill in without seeing the first reader's numbers. Pins, limit groups and drive kinds stay, so both
/// readers answer the same questions.
pub fn blank(toml_text: &str) -> Result<String, String> {
    let mut doc: toml::Table = toml::from_str(toml_text).map_err(|e| e.to_string())?;
    fn clear(t: &mut toml::Table) {
        for (k, v) in t.iter_mut() {
            if VALUE_KEYS.contains(&k.as_str()) {
                *v = toml::Value::String("?".into());
            } else if let toml::Value::Array(a) = v {
                for item in a {
                    if let toml::Value::Table(t) = item {
                        clear(t);
                    }
                }
            }
        }
    }
    clear(&mut doc);
    let mut out = String::from(
        "# Second reading: replace every \"?\" with what the datasheet says, in the format of parts/README.md.\n\
         # Write { none = \"reason\" } where it gives nothing. Don't look at the first reading.\n",
    );
    out.push_str(&toml::to_string(&doc).map_err(|e| e.to_string())?);
    Ok(out)
}

/// One place where two readings of a datasheet disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    pub at: String,
    pub first: String,
    pub second: String,
}

fn show_bound(b: &Bound) -> String {
    match b {
        Bound::Value(v) => format!("{v}"),
        Bound::Pin { pin, plus, at_most } => {
            let mut s = pin.clone();
            if *plus != 0.0 {
                let _ = write!(s, " + {plus}");
            }
            if let Some(c) = at_most {
                let _ = write!(s, " (at most {c})");
            }
            s
        }
        Bound::Unknown(_) => "unknown".into(),
        Bound::None(_) => "none".into(),
    }
}

fn same_bound(a: &Bound, b: &Bound) -> bool {
    match (a, b) {
        (Bound::Value(x), Bound::Value(y)) => close(*x, *y),
        (
            Bound::Pin {
                pin: p1,
                plus: x1,
                at_most: c1,
            },
            Bound::Pin {
                pin: p2,
                plus: x2,
                at_most: c2,
            },
        ) => {
            p1 == p2
                && close(*x1, *x2)
                && match (c1, c2) {
                    (Some(a), Some(b)) => close(*a, *b),
                    (None, None) => true,
                    _ => false,
                }
        }
        (Bound::Unknown(_) | Bound::None(_), Bound::Unknown(_) | Bound::None(_)) => true,
        _ => false,
    }
}

fn compare_range(at: &str, a: Option<&Range>, b: Option<&Range>, out: &mut Vec<Difference>) {
    let show = |r: Option<&Range>| match r {
        None => "absent".to_string(),
        Some(Range::Unknown(_)) => "unknown".to_string(),
        Some(Range::None(_)) => "none".to_string(),
        Some(Range::Known { min, max, page, .. }) => format!(
            "{} .. {} (page {})",
            show_bound(min),
            show_bound(max),
            page.map_or("?".into(), |p| p.to_string())
        ),
    };
    // A range whose ends are both unknown says the same as an unknown range.
    let blank = |r: Option<&Range>| match r {
        Some(Range::Known { min, max, .. }) if !min.is_known() && !max.is_known() => {
            Some(Range::Unknown(String::new()))
        }
        other => other.cloned(),
    };
    let (a2, b2) = (blank(a), blank(b));
    let same = match (a2.as_ref(), b2.as_ref()) {
        (None, None) | (Some(Range::Unknown(_) | Range::None(_)), Some(Range::Unknown(_) | Range::None(_))) => true,
        (
            Some(Range::Known {
                min: m1,
                max: x1,
                page: p1,
                ..
            }),
            Some(Range::Known {
                min: m2,
                max: x2,
                page: p2,
                ..
            }),
        ) => {
            // A page only matters when it backs a number.
            let numbers = m1.is_known() || x1.is_known();
            same_bound(m1, m2) && same_bound(x1, x2) && (p1 == p2 || !numbers)
        }
        _ => false,
    };
    if !same {
        out.push(Difference {
            at: at.to_string(),
            first: show(a),
            second: show(b),
        });
    }
}

/// Every value where two readings of the same datasheet disagree: the numbers, which ends are unknown, and
/// the pages. Limits and drives are matched by position, so both readings must come from the same
/// [`blank`] file.
pub fn compare(first: &Part, second: &Part) -> Vec<Difference> {
    let mut out = Vec::new();
    let opt = |label: &str, a: Option<String>, b: Option<String>, out: &mut Vec<Difference>| {
        if a != b {
            out.push(Difference {
                at: label.to_string(),
                first: a.unwrap_or_else(|| "absent".into()),
                second: b.unwrap_or_else(|| "absent".into()),
            });
        }
    };
    opt(
        "tolerance",
        first
            .tolerance
            .as_ref()
            .map(|t| format!("{} (page {})", t.value, t.page)),
        second
            .tolerance
            .as_ref()
            .map(|t| format!("{} (page {})", t.value, t.page)),
        &mut out,
    );
    opt(
        "tcr",
        first.tcr.as_ref().map(|t| format!("{} ppm (page {})", t.ppm, t.page)),
        second.tcr.as_ref().map(|t| format!("{} ppm (page {})", t.ppm, t.page)),
        &mut out,
    );
    opt(
        "temperature",
        first
            .temperature
            .as_ref()
            .map(|t| format!("{} .. {} °C (page {})", t.min, t.max, t.page)),
        second
            .temperature
            .as_ref()
            .map(|t| format!("{} .. {} °C (page {})", t.min, t.max, t.page)),
        &mut out,
    );
    let rated = |r: &Option<crate::limits::Rated>| r.as_ref().map(|r| format!("{} (page {})", r.value, r.page));
    opt("theta_ja", rated(&first.theta_ja), rated(&second.theta_ja), &mut out);
    opt("tj_max", rated(&first.tj_max), rated(&second.tj_max), &mut out);
    opt(
        "efficiency",
        rated(&first.efficiency),
        rated(&second.efficiency),
        &mut out,
    );
    if first.supply.len() == second.supply.len() {
        for (i, (a, b)) in first.supply.iter().zip(&second.supply).enumerate() {
            let at = format!("supply {} ({})", i + 1, a.pins.join(", "));
            if a.pins == b.pins {
                compare_range(&format!("{at} current"), Some(&a.current), Some(&b.current), &mut out);
            } else {
                out.push(Difference {
                    at,
                    first: format!("{:?}", a.pins),
                    second: format!("{:?}", b.pins),
                });
            }
        }
    } else {
        out.push(Difference {
            at: "supply".into(),
            first: format!("{} entries", first.supply.len()),
            second: format!("{} entries", second.supply.len()),
        });
    }
    if first.limits.len() != second.limits.len() {
        out.push(Difference {
            at: "limits".into(),
            first: format!("{} limits", first.limits.len()),
            second: format!("{} limits", second.limits.len()),
        });
    }
    for (i, (a, b)) in first.limits.iter().zip(&second.limits).enumerate() {
        let at = format!("limit {} ({})", i + 1, a.pins.join(", "));
        if a.pins != b.pins || a.to != b.to || a.quantity != b.quantity {
            out.push(Difference {
                at: at.clone(),
                first: format!("{:?} to {:?}", a.pins, a.to),
                second: format!("{:?} to {:?}", b.pins, b.to),
            });
            continue;
        }
        compare_range(&format!("{at} abs_max"), Some(&a.abs_max), Some(&b.abs_max), &mut out);
        compare_range(
            &format!("{at} recommended"),
            Some(&a.recommended),
            Some(&b.recommended),
            &mut out,
        );
        compare_range(
            &format!("{at} breakdown"),
            a.breakdown.as_ref(),
            b.breakdown.as_ref(),
            &mut out,
        );
        let clamp = |c: Option<&crate::limits::Clamp>| {
            c.map_or("absent".to_string(), |c| {
                format!("{} V at {} A (page {})", c.volts, c.current, c.page)
            })
        };
        let (ca, cb) = (clamp(a.clamp.as_ref()), clamp(b.clamp.as_ref()));
        if ca != cb {
            out.push(Difference {
                at: format!("{at} clamp"),
                first: ca,
                second: cb,
            });
        }
    }
    if first.drives.len() != second.drives.len() {
        out.push(Difference {
            at: "drives".into(),
            first: format!("{} drives", first.drives.len()),
            second: format!("{} drives", second.drives.len()),
        });
    }
    let swings = |p: &Part| -> Vec<String> {
        p.opamps
            .iter()
            .map(|a| {
                let s = a
                    .swing
                    .as_ref()
                    .map_or("none".into(), |s| format!("{} V (page {})", s.value, s.page));
                format!("{} {} {} {} {}: swing {s}", a.plus, a.minus, a.out, a.vpos, a.vneg)
            })
            .collect()
    };
    if swings(first) != swings(second) {
        out.push(Difference {
            at: "opamps".into(),
            first: swings(first).join("; "),
            second: swings(second).join("; "),
        });
    }
    let esd = |p: &Part| -> Vec<String> {
        let mut v: Vec<String> = p
            .esd
            .iter()
            .map(|e| format!("{} {:?} ±{} V (page {})", e.model.name(), e.pins, e.volts, e.page))
            .collect();
        v.sort();
        v
    };
    let (ea, eb) = (esd(first), esd(second));
    for x in ea.iter().filter(|x| !eb.contains(x)) {
        out.push(Difference {
            at: "esd".into(),
            first: x.clone(),
            second: "absent or different".into(),
        });
    }
    for x in eb.iter().filter(|x| !ea.contains(x)) {
        out.push(Difference {
            at: "esd".into(),
            first: "absent or different".into(),
            second: x.clone(),
        });
    }
    let capacitance = |p: &Part| -> Vec<String> {
        let mut v: Vec<String> = p
            .capacitance
            .iter()
            .map(|c| {
                format!(
                    "{:?} to {}: typ {:?} max {:?} pF (page {})",
                    c.pins, c.to, c.typ, c.max, c.page
                )
            })
            .collect();
        v.sort();
        v
    };
    let (ca, cb) = (capacitance(first), capacitance(second));
    for x in ca.iter().filter(|x| !cb.contains(x)) {
        out.push(Difference {
            at: "capacitance".into(),
            first: x.clone(),
            second: "absent or different".into(),
        });
    }
    for x in cb.iter().filter(|x| !ca.contains(x)) {
        out.push(Difference {
            at: "capacitance".into(),
            first: "absent or different".into(),
            second: x.clone(),
        });
    }
    for (i, (a, b)) in first.drives.iter().zip(&second.drives).enumerate() {
        let at = format!("drive {} ({})", i + 1, a.pin);
        compare_range(&format!("{at} range"), Some(&a.range), Some(&b.range), &mut out);
        let h = |d: &crate::limits::Drive| {
            d.headroom
                .as_ref()
                .map(|h| format!("duty {} volts {} (page {})", h.duty, h.volts, h.page))
        };
        opt(&format!("{at} headroom"), h(a), h(b), &mut out);
        opt(
            &format!("{at} on at VGS"),
            a.on.as_ref().map(|o| format!("{} V (page {})", o.vgs, o.page)),
            b.on.as_ref().map(|o| format!("{} V (page {})", o.vgs, o.page)),
            &mut out,
        );
        opt(&format!("{at} gate"), a.gate.clone(), b.gate.clone(), &mut out);
        opt(
            &format!("{at} test current"),
            a.current.map(|c| c.to_string()),
            b.current.map(|c| c.to_string()),
            &mut out,
        );
    }
    out
}
