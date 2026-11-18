use std::collections::HashMap;

pub type Aliases = HashMap<String, Vec<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusKind {
    Vector,
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub local: String,
    pub name: String,
    pub index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bus {
    pub kind: BusKind,
    pub members: Vec<Member>,
}

const MAX_VECTOR: i64 = 4096;

impl Bus {
    pub fn find(&self, other: &Member) -> Option<usize> {
        match self.kind {
            BusKind::Vector => self.members.iter().position(|m| m.index == other.index),
            BusKind::Group => self
                .members
                .iter()
                .position(|m| m.local == other.local)
                .or_else(|| (other.index < self.members.len()).then_some(other.index)),
        }
    }

    pub fn contains_all(&self, other: &Bus) -> bool {
        other
            .members
            .iter()
            .all(|o| self.members.iter().any(|m| m.name == o.name))
    }
}

pub fn parse(label: &str, aliases: &Aliases) -> Option<Bus> {
    if let Some((_, names)) = parse_vector(label) {
        let members = names
            .into_iter()
            .enumerate()
            .map(|(index, name)| Member {
                local: name.clone(),
                name,
                index,
            })
            .collect();
        return Some(Bus {
            kind: BusKind::Vector,
            members,
        });
    }
    let (group, names) = parse_group(label)?;
    let prefix = if group.is_empty() {
        String::new()
    } else {
        format!("{group}.")
    };
    let mut members = Vec::new();
    for name in &names {
        match aliases.get(name) {
            Some(alias) => {
                for a in alias {
                    add_member(&mut members, &prefix, a);
                }
            }
            None => add_member(&mut members, &prefix, name),
        }
    }
    Some(Bus {
        kind: BusKind::Group,
        members,
    })
}

fn add_member(members: &mut Vec<Member>, prefix: &str, name: &str) {
    if let Some((_, names)) = parse_vector(name) {
        for (index, n) in names.into_iter().enumerate() {
            members.push(Member {
                name: format!("{prefix}{n}"),
                local: n,
                index,
            });
        }
    } else if parse_group(name).is_some() {
    } else {
        members.push(Member {
            local: name.to_string(),
            name: format!("{prefix}{name}"),
            index: 0,
        });
    }
}

fn is_markup(c: char) -> bool {
    matches!(c, '_' | '^' | '~')
}

fn is_quote(s: &[char], i: usize) -> bool {
    s[i] == '"' && (i == 0 || s[i - 1] != '\\')
}

fn parse_vector(text: &str) -> Option<(String, Vec<String>)> {
    let s: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut prefix: Vec<char> = Vec::new();
    let mut nesting = 0i32;
    let mut markup_wraps_name = false;
    let mut in_quotes = false;
    let mut found_bracket = false;
    while i < s.len() {
        let c = s[i];
        if is_quote(&s, i) {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if in_quotes {
            if c == '\\' && i + 1 < s.len() {
                i += 1;
            }
            prefix.push(s[i]);
            i += 1;
            continue;
        }
        match c {
            '{' if i > 0 && is_markup(s[i - 1]) => {
                nesting += 1;
                prefix.push('{');
            }
            '{' | ' ' | ']' => return None,
            '}' => {
                nesting -= 1;
                prefix.push('}');
            }
            '\\' if s.get(i + 1) == Some(&' ') => {
                prefix.push(' ');
                i += 1;
            }
            '[' => {
                if nesting > 0
                    && let Some(open) = prefix.iter().rposition(|&p| p == '{')
                    && open > 0
                    && is_markup(prefix[open - 1])
                {
                    if open == prefix.len() - 1 {
                        prefix.truncate(open - 1);
                    } else {
                        markup_wraps_name = true;
                    }
                }
                found_bracket = true;
                break;
            }
            _ => prefix.push(c),
        }
        i += 1;
    }
    if !found_bracket {
        return None;
    }
    i += 1;
    let number = |stop: &str, i: &mut usize| -> Option<String> {
        let mut digits = String::new();
        while *i < s.len() {
            if s[*i..].starts_with(&stop.chars().collect::<Vec<_>>()) {
                *i += stop.len();
                return (!digits.is_empty()).then_some(digits);
            }
            if !s[*i].is_ascii_digit() {
                return None;
            }
            digits.push(s[*i]);
            *i += 1;
        }
        None
    };
    let first = number("..", &mut i)?;
    let last = number("]", &mut i)?;
    let mut suffix = String::new();
    for &c in &s[i..] {
        match c {
            '}' => {
                nesting -= 1;
                if markup_wraps_name {
                    suffix.push('}');
                }
            }
            '+' | '-' | 'P' | 'N' => suffix.push(c),
            _ => return None,
        }
    }
    if nesting != 0 {
        return None;
    }
    let width = first.len().max(last.len());
    let padded = (first.len() > 1 && first.starts_with('0')) || (last.len() > 1 && last.starts_with('0'));
    let (mut begin, mut end): (i64, i64) = (first.parse().ok()?, last.parse().ok()?);
    if begin == end || (end - begin).abs() >= MAX_VECTOR {
        return None;
    }
    if begin > end {
        std::mem::swap(&mut begin, &mut end);
    }
    let prefix: String = prefix.into_iter().collect();
    let names = (begin..=end)
        .map(|n| {
            let n = n.to_string();
            let pad = if padded { width.saturating_sub(n.len()) } else { 0 };
            format!("{prefix}{}{n}{suffix}", "0".repeat(pad))
        })
        .collect();
    Some((prefix, names))
}

fn parse_group(text: &str) -> Option<(String, Vec<String>)> {
    let s: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut prefix = String::new();
    let mut nesting = 0i32;
    let mut in_quotes = false;
    while i < s.len() {
        let c = s[i];
        if is_quote(&s, i) {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if in_quotes {
            if c == '\\' && i + 1 < s.len() {
                i += 1;
            }
            prefix.push(s[i]);
            i += 1;
            continue;
        }
        match c {
            '{' if i > 0 && is_markup(s[i - 1]) => {
                nesting += 1;
                prefix.push('{');
            }
            '{' => break,
            '}' => {
                nesting -= 1;
                prefix.push('}');
            }
            '\\' if s.get(i + 1) == Some(&' ') => {
                prefix.push(' ');
                i += 1;
            }
            ' ' | '[' | ']' => return None,
            _ => prefix.push(c),
        }
        i += 1;
    }
    if nesting != 0 {
        return None;
    }
    i += 1;
    let mut members = Vec::new();
    let mut tmp = String::new();
    in_quotes = false;
    while i < s.len() {
        let c = s[i];
        if is_quote(&s, i) {
            in_quotes = !in_quotes;
            i += 1;
            continue;
        }
        if in_quotes {
            if c == '\\' && i + 1 < s.len() {
                i += 1;
            }
            tmp.push(s[i]);
            i += 1;
            continue;
        }
        match c {
            '{' if i > 0 && is_markup(s[i - 1]) => {
                nesting += 1;
                tmp.push('{');
            }
            '{' => return None,
            '}' if nesting > 0 => {
                nesting -= 1;
                tmp.push('}');
            }
            '}' => {
                if !tmp.is_empty() {
                    members.push(tmp);
                }
                return Some((prefix, members));
            }
            '\\' if s.get(i + 1) == Some(&' ') => {
                tmp.push(' ');
                i += 1;
            }
            ' ' | ',' => {
                if !tmp.is_empty() {
                    members.push(std::mem::take(&mut tmp));
                }
            }
            _ => tmp.push(c),
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(label: &str) -> Option<Vec<String>> {
        parse(label, &Aliases::new()).map(|b| b.members.into_iter().map(|m| m.name).collect())
    }

    #[test]
    fn reads_vectors() {
        assert_eq!(names("D[0..2]").unwrap(), ["D0", "D1", "D2"]);
        assert_eq!(names("D[2..0]").unwrap(), ["D0", "D1", "D2"]);
        assert_eq!(names("A[08..10]").unwrap(), ["A08", "A09", "A10"]);
        assert_eq!(names("LVDS[0..1]P").unwrap(), ["LVDS0P", "LVDS1P"]);
        assert_eq!(names("[1..2]").unwrap(), ["1", "2"]);
        assert_eq!(names("~{BE[0..1]}").unwrap(), ["~{BE0}", "~{BE1}"]);
        assert_eq!(names("D_{[1..2]}").unwrap(), ["D1", "D2"]);
        for net in ["D[1..1]", "D[0..1]X", "D[0..1", "D0", "D[0..1] X"] {
            assert_eq!(names(net), None, "{net}");
        }
    }

    #[test]
    fn reads_groups() {
        assert_eq!(names("USB{DP DM}").unwrap(), ["USB.DP", "USB.DM"]);
        assert_eq!(names("{SDA,SCL}").unwrap(), ["SDA", "SCL"]);
        assert_eq!(names("MEM{D[0..1] ~{WE}}").unwrap(), ["MEM.D0", "MEM.D1", "MEM.~{WE}"]);
        assert_eq!(names(r#"{"A B" C\ D}"#).unwrap(), ["A B", "C D"]);
        assert_eq!(names("~{X}"), None);
        assert_eq!(names("X{A B"), None);
    }

    #[test]
    fn expands_aliases_inside_groups() {
        let aliases = Aliases::from([("CTRL".to_string(), vec!["EN".to_string(), "D[0..1]".to_string()])]);
        let bus = parse("PWR{CTRL RST}", &aliases).unwrap();
        let got: Vec<&str> = bus.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(got, ["PWR.EN", "PWR.D0", "PWR.D1", "PWR.RST"]);
        assert_eq!(parse("CTRL", &aliases), None);
    }

    #[test]
    fn pairs_members_like_kicad() {
        let aliases = Aliases::new();
        let d = parse("D[0..3]", &aliases).unwrap();
        let b = parse("B[4..7]", &aliases).unwrap();
        assert_eq!(d.find(&b.members[1]), Some(1));
        let g = parse("USB{DP DM}", &aliases).unwrap();
        let h = parse("X{DM DP}", &aliases).unwrap();
        assert_eq!(g.find(&h.members[0]), Some(1));
        assert!(d.contains_all(&parse("D[1..2]", &aliases).unwrap()));
    }
}
