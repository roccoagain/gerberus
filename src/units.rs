pub fn parse_ohms(value: &str) -> Option<f64> {
    let s: String = value
        .trim()
        .trim_end_matches("ohms")
        .trim_end_matches("ohm")
        .trim_end_matches('Ω')
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let letter = s.char_indices().find_map(|(i, ch)| {
        let m = match ch {
            'R' | 'r' => 1.0,
            'k' | 'K' => 1e3,
            'M' => 1e6,
            'G' => 1e9,
            _ => return None,
        };
        Some((i, ch.len_utf8(), m))
    });
    let (digits, mult) = match letter {
        None => (s.clone(), 1.0),
        Some((i, len, m)) => {
            let (whole, frac) = (&s[..i], &s[i + len..]);
            if frac.is_empty() {
                (whole.to_string(), m)
            } else if whole.contains('.') || !frac.chars().all(|c| c.is_ascii_digit()) {
                return None;
            } else {
                (format!("{whole}.{frac}"), m)
            }
        }
    };
    let v: f64 = digits.parse().ok()?;
    (v.is_finite() && v >= 0.0).then_some(v * mult)
}

pub fn parse_farads(value: &str) -> Option<f64> {
    parse_si(value, 'F')
}

pub fn parse_henries(value: &str) -> Option<f64> {
    parse_si(value, 'H')
}

fn parse_si(value: &str, unit: char) -> Option<f64> {
    let word = value
        .split_whitespace()
        .find(|w| w.starts_with(|c: char| c.is_ascii_digit() || c == '.'))?;
    let word = word.trim_end_matches(unit).trim_end_matches(unit.to_ascii_lowercase());
    let prefix = |c: char| match c {
        'p' => Some(1e-12),
        'n' => Some(1e-9),
        'u' | 'µ' | 'μ' => Some(1e-6),
        'm' => Some(1e-3),
        _ => None,
    };
    let at = word.char_indices().find(|(_, c)| !(c.is_ascii_digit() || *c == '.'));
    let v = match at {
        None => word.parse::<f64>().ok()?,
        Some((i, c)) => {
            let mult = prefix(c)?;
            let (whole, rest) = (&word[..i], &word[i + c.len_utf8()..]);
            if rest.is_empty() {
                whole.parse::<f64>().ok()? * mult
            } else if !whole.contains('.') && rest.chars().all(|c| c.is_ascii_digit()) {
                format!("{whole}.{rest}").parse::<f64>().ok()? * mult
            } else {
                return None;
            }
        }
    };
    (v.is_finite() && v > 0.0).then_some(v)
}
