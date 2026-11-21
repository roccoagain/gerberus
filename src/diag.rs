//! Terminal output in the style of rustc's diagnostics:
//!
//! ```text
//! error[abs-max]: U6.PA4 reaches 26 V, past the maximum of 4 V
//!   --> U6.PA4 (/mcu/VSENSE)
//!    = help: add a divider, clamp or series resistor, or choose a part rated higher
//! ```
//!
//! The checkers write each finding as one sentence that cites its numbers. [`Diagnostic::from_message`] splits
//! that sentence into a headline, where it is, and notes. A report prints the headline, location and help;
//! `--verbose` adds the notes (the worst case, datasheet citations) and longer explanations, and
//! `gerberus explain RULE` says what a rule checks and why. The JSON output keeps the message whole.
use std::fmt::Write as _;
use std::io::IsTerminal as _;

/// How bad a diagnostic is, which picks its label and colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warning,
    Unknown,
    Pass,
    /// Something to add, from `doctor`.
    Todo,
    /// A finding a waiver accepted.
    Waived,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
            Level::Unknown => "unknown",
            Level::Pass => "pass",
            Level::Todo => "todo",
            Level::Waived => "waived",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Level::Error => RED,
            Level::Warning => YELLOW,
            Level::Unknown => CYAN,
            Level::Pass => GREEN,
            Level::Todo | Level::Waived => BLUE,
        }
    }
}

const RED: &str = "1;31";
const YELLOW: &str = "1;33";
const CYAN: &str = "1;36";
const GREEN: &str = "1;32";
pub const BLUE: &str = "1;34";
pub const BOLD: &str = "1";

/// Whether to write ANSI colour codes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
}

impl Style {
    /// Colours only a terminal, and never when `NO_COLOR` is set to anything (<https://no-color.org>).
    pub fn detect() -> Style {
        Style {
            color: std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) && std::io::stdout().is_terminal(),
        }
    }

    pub fn paint(self, code: &str, text: &str) -> String {
        if self.color && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
}

/// What a report shows besides errors and warnings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Show {
    /// List UNKNOWNs and passes, not just count them.
    pub all: bool,
    /// Print the longer explanations: how each net in a FAIL got its range, and long lists in full.
    pub verbose: bool,
    pub style: Style,
    /// Columns to wrap notes at; 0 means 100.
    pub width: usize,
    /// Rules whose warnings `--allow` silences, as a report prints them in brackets; `warnings` silences all.
    pub allow: Vec<String>,
}

impl Show {
    /// Everything, uncoloured: what `--all --verbose` prints with `NO_COLOR` set.
    pub fn everything() -> Show {
        Show {
            all: true,
            verbose: true,
            style: Style::default(),
            width: 0,
            allow: vec![],
        }
    }

    /// The column notes wrap at.
    pub fn columns(&self) -> usize {
        if self.width == 0 { 100 } else { self.width }
    }

    /// Reads `--all`, `--verbose`/`-v` and `--allow RULE` out of `args`, returning the rest.
    pub fn parse(args: &[String]) -> Result<(Show, Vec<String>), String> {
        let mut show = Show::default();
        let mut rest = vec![];
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--all" => show.all = true,
                "--verbose" | "-v" => show.verbose = true,
                "--allow" => show.allow.push(
                    it.next()
                        .filter(|r| !r.starts_with('-'))
                        .ok_or("--allow takes a rule, such as esd, or `warnings` for every warning")?
                        .clone(),
                ),
                _ => rest.push(a.clone()),
            }
        }
        show.style = Style::detect();
        show.width = width();
        Ok((show, rest))
    }

    /// Whether `--allow` silences the warnings of `rule`.
    pub fn allows(&self, rule: &str) -> bool {
        self.allow.iter().any(|a| a == rule || a == "warnings")
    }

    /// Takes the warnings `--allow` silences out of `items`, where `warning` gives an item's rule when it is a
    /// warning, and returns how many it took.
    pub fn silence<T>(&self, items: &mut Vec<T>, warning: impl Fn(&T) -> Option<&str>) -> usize {
        let before = items.len();
        items.retain(|i| !warning(i).is_some_and(|rule| self.allows(rule)));
        before - items.len()
    }

    /// The closing note for `--allow`: how many warnings it silenced, or that it matched none, which catches a
    /// misspelt rule.
    pub fn silenced_note(&self, n: usize) -> String {
        if self.allow.is_empty() {
            return String::new();
        }
        let what = if n == 0 {
            "no warnings matched".to_string()
        } else {
            format!("{} silenced by", plural(n, "warning"))
        };
        format!(
            "   {} {}: {what} --allow {}\n",
            self.style.paint(BLUE, "="),
            self.style.paint(BOLD, "note"),
            self.allow.join(", --allow ")
        )
    }

    /// Whether a finding at `level` is listed.
    pub fn lists(&self, level: Level) -> bool {
        match level {
            Level::Error | Level::Warning | Level::Todo => true,
            Level::Unknown | Level::Pass | Level::Waived => self.all,
        }
    }
}

/// One finding laid out for the terminal.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub level: Level,
    pub rule: String,
    pub headline: String,
    /// The pin, part or pair it is about, after `-->`.
    pub location: Option<String>,
    pub notes: Vec<String>,
    pub help: Option<String>,
    /// Longer explanations shown only with `--verbose`: a title and its lines, printed as they are.
    pub details: Vec<(String, Vec<String>)>,
}

impl Diagnostic {
    /// A diagnostic that is just its headline, as a pass is.
    pub fn line(level: Level, rule: &str, text: &str) -> Diagnostic {
        Diagnostic {
            level,
            rule: rule.to_string(),
            headline: text.to_string(),
            location: None,
            notes: vec![],
            help: None,
            details: vec![],
        }
    }

    /// Splits a checker's message into headline, location and notes. `subjects` are what the finding is about: a
    /// reference designator, or a pair's nets. A message that opens with one and a colon, like
    /// `U6.PA4 (/mcu/VSENSE): ...`, gives its location from that opening.
    pub fn from_message(level: Level, rule: &str, subjects: &[&str], message: &str) -> Diagnostic {
        let mut segs = split_top(message, ": ");
        let mut location = None;
        let mut notes = vec![];
        if segs.len() > 1 && is_locator(&segs[0], subjects) {
            location = Some(segs.remove(0));
            if segs.len() > 1 && is_span(&strip_asides(&segs[0]).0) {
                let (span, extra) = strip_asides(&segs.remove(0));
                notes.push(format!("worst case {span}"));
                notes.extend(extra);
            }
        }
        let mut claim = if segs.is_empty() { String::new() } else { segs.remove(0) };
        // A bare opening like `at J6` is a place, not a claim: the claim is what follows it.
        if location.is_none() && !segs.is_empty() && claim.split_whitespace().count() <= 3 {
            claim = format!("{claim}: {}", segs.remove(0));
        }
        // An aside closing the whole claim, like a datasheet citation, covers every part of it, unless earlier
        // parts have their own.
        let mut pieces: Vec<String> = split_top(&claim, "; ")
            .into_iter()
            .flat_map(|p| split_top(&p, ". "))
            .collect();
        let mut shared = vec![];
        if pieces.len() > 1
            && pieces[..pieces.len() - 1].iter().all(|p| strip_asides(p).1.is_empty())
            && let Some(last) = pieces.pop()
        {
            match strip_last_aside(&last) {
                (text, Some(a)) if is_aside(&a) => {
                    pieces.push(text);
                    shared.push(a);
                }
                _ => pieces.push(last),
            }
        }
        let mut pieces = pieces.into_iter();
        let (lead, cited) = strip_asides(&pieces.next().unwrap_or_default());
        let headline = match &location {
            Some(loc) => {
                let who = loc.split(' ').next().unwrap_or_default();
                if starts_with_verb(&lead) {
                    format!("{who} {lead}")
                } else {
                    format!("{who}: {lead}")
                }
            }
            None => lead,
        };
        notes.extend(cited);
        let mut also: Vec<String> = pieces.map(|p| tidy(&p)).collect();
        // A short last clause like `not modelled` labels the text after the next colon: keep them together.
        if !segs.is_empty() && also.last().is_some_and(|l| l.split_whitespace().count() <= 3) {
            let label = also.pop().unwrap_or_default();
            segs[0] = format!("{label}: {}", segs[0]);
        }
        notes.extend(also);
        notes.extend(shared);
        if !segs.is_empty() {
            let tail = segs.join(": ");
            notes.extend(split_top(&tail, ". ").iter().map(|s| tidy(s)));
        }
        let location = location.or_else(|| leading_pin(&headline, subjects));
        // The location names the net, so the headline needn't: `J1.2 has no TVS`, not `J1.2 (/UART_TX) has no TVS`.
        let headline = match &location {
            Some(loc) if loc.contains(" (") => match headline.strip_prefix(loc.as_str()) {
                Some(rest) => format!("{}{rest}", loc.split(' ').next().unwrap_or_default()),
                None => headline,
            },
            _ => headline,
        };
        Diagnostic {
            level,
            rule: rule.to_string(),
            headline: tidy(&headline),
            location,
            notes: notes.into_iter().filter(|n| !n.is_empty()).collect(),
            help: None,
            details: vec![],
        }
    }

    /// Writes the diagnostic, wrapping notes at `width` columns, and a blank line after it. Notes and details are
    /// written only when `verbose`. A pass is one line.
    pub fn render(&self, out: &mut String, style: Style, verbose: bool, width: usize) {
        let label = style.paint(self.level.color(), &format!("{}[{}]", self.level.label(), self.rule));
        let _ = writeln!(
            out,
            "{label}{} {}",
            style.paint(BOLD, ":"),
            style.paint(BOLD, &self.headline)
        );
        if self.level == Level::Pass {
            return;
        }
        if let Some(loc) = &self.location {
            let _ = writeln!(out, "  {} {loc}", style.paint(BLUE, "-->"));
        }
        let eq = style.paint(BLUE, "=");
        let line = |out: &mut String, kind: &str, text: &str| {
            let head = format!("   {eq} {}: ", style.paint(BOLD, kind));
            let indent = 3 + 2 + kind.len() + 2;
            for (i, l) in wrap(text, width.saturating_sub(indent).max(30)).iter().enumerate() {
                if i == 0 {
                    let _ = writeln!(out, "{head}{l}");
                } else {
                    let _ = writeln!(out, "{:indent$}{l}", "");
                }
            }
        };
        if verbose {
            for n in &self.notes {
                line(out, "note", n);
            }
            for (title, lines) in &self.details {
                line(out, "note", title);
                for l in lines {
                    let _ = writeln!(out, "       {l}");
                }
            }
        }
        if let Some(h) = &self.help {
            line(out, "help", h);
        }
        out.push('\n');
    }
}

/// What a report's listed diagnostics leave for its summary to point at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    /// Some notes or details were left for `--verbose`.
    pub hidden: bool,
    /// The first error or warning's rule, for the `gerberus explain` hint.
    pub rule: Option<String>,
}

impl Listed {
    /// Counts in a diagnostic the report lists.
    pub fn add(&mut self, d: &Diagnostic) {
        self.hidden |= !d.notes.is_empty() || !d.details.is_empty();
        if self.rule.is_none()
            && matches!(d.level, Level::Error | Level::Warning)
            && crate::explain::text(&d.rule).is_some()
        {
            self.rule = Some(d.rule.clone());
        }
    }
}

/// The closing line, cargo-style: `error: 18 errors, 9 warnings, 110 unknown, 402 passed`, then notes on what
/// was left out and how to show it, and, as rustc ends with `rustc --explain`, where to read more about a rule.
pub fn summary(out: &mut String, show: &Show, counts: [usize; 4], listed: &Listed) {
    let [errors, warnings, unknown, passed] = counts;
    let style = show.style;
    let (level, word) = if errors > 0 {
        (RED, "error")
    } else if warnings > 0 {
        (YELLOW, "warning")
    } else if unknown > 0 {
        // Nothing failed, but nothing unknown counts as a pass either.
        (CYAN, "unknown")
    } else {
        (GREEN, "ok")
    };
    let _ = writeln!(
        out,
        "{}{} {}",
        style.paint(level, word),
        style.paint(BOLD, ":"),
        style.paint(
            BOLD,
            &format!(
                "{}, {}, {unknown} unknown, {passed} passed",
                plural(errors, "error"),
                plural(warnings, "warning")
            )
        )
    );
    let eq = style.paint(BLUE, "=");
    let note = style.paint(BOLD, "note");
    let mut hidden = vec![];
    if unknown > 0 && !show.all {
        hidden.push(format!("{unknown} unknown"));
    }
    if passed > 0 && !show.all {
        hidden.push(format!("{passed} passed"));
    }
    if !hidden.is_empty() {
        let _ = writeln!(
            out,
            "   {eq} {note}: {} not shown; --all lists them",
            hidden.join(" and ")
        );
    }
    if listed.hidden && !show.verbose {
        let _ = writeln!(
            out,
            "   {eq} {note}: --verbose adds each finding's numbers and sources, and how each net got its range"
        );
    }
    if let Some(rule) = &listed.rule {
        let _ = writeln!(
            out,
            "   {eq} {}: for more about a rule, try `gerberus explain {rule}`",
            style.paint(BOLD, "help")
        );
    }
}

pub fn plural(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// The terminal width for wrapping: `COLUMNS` if set, else 100.
pub fn width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .filter(|&c: &usize| c >= 40)
        .unwrap_or(100)
}

/// Splits `s` on `sep` where it isn't inside parentheses.
fn split_top(s: &str, sep: &str) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth = (depth - 1).max(0),
            _ => {}
        }
        if depth == 0 && bytes[i..].starts_with(sep.as_bytes()) {
            out.push(s[start..i].to_string());
            i += sep.len();
            start = i;
            continue;
        }
        i += 1;
    }
    out.push(s[start..].to_string());
    out
}

/// Takes out parenthesised asides long enough to be citations or derivations, returning the text without them
/// and the asides. Short ones, like a net name, stay.
fn strip_asides(s: &str) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut asides = vec![];
    let mut rest = s;
    while let Some(open) = top_open(rest) {
        let Some(close) = matching(rest, open) else {
            break;
        };
        let inner = &rest[open + 1..close];
        if is_aside(inner) {
            text.push_str(rest[..open].trim_end());
            asides.push(inner.to_string());
        } else {
            text.push_str(&rest[..=close]);
        }
        rest = &rest[close + 1..];
    }
    text.push_str(rest);
    (text, asides)
}

/// Whether a parenthesised part is a citation or derivation rather than a name: long, or ending in a page.
fn is_aside(inner: &str) -> bool {
    let page = |w: &str| {
        w.strip_prefix('p')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    // A net name like `unconnected-(J5-SBU1-PadA8)` has no spaces, however long; a citation always does.
    inner.contains(' ') && (inner.chars().count() > 24 || inner.split([' ', ',']).any(page))
}

/// Takes the aside that ends `s` off it.
fn strip_last_aside(s: &str) -> (String, Option<String>) {
    let t = s.trim_end();
    let mut open = None;
    let mut depth = 0;
    for (i, c) in t.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    open = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    match open {
        Some(i) if t.ends_with(')') && i > 0 => {
            (t[..i].trim_end().to_string(), Some(t[i + 1..t.len() - 1].to_string()))
        }
        _ => (s.to_string(), None),
    }
}

/// The first `(` outside parentheses.
fn top_open(s: &str) -> Option<usize> {
    s.find('(')
}

/// The `)` that closes the `(` at `open`.
fn matching(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Verbs a claim can open with, so the headline reads `U6.PA4 reaches 26 V` rather than `U6.PA4: reaches 26 V`.
const VERBS: [&str; 18] = [
    "is",
    "are",
    "has",
    "have",
    "sets",
    "holds",
    "reaches",
    "falls",
    "dissipates",
    "takes",
    "drives",
    "damages",
    "connected",
    "survive",
    "survives",
    "stays",
    "gives",
    "needs",
];

/// Whether an opening like `U6.PA4 (/mcu/VSENSE)` or `R15.1 (/LED0) from R15.2 (/LED0_A)` only says where.
fn is_locator(seg: &str, subjects: &[&str]) -> bool {
    let bare = without_parens(seg);
    let words: Vec<&str> = bare.split_whitespace().collect();
    words.first().is_some_and(|w| names(w, subjects))
        && (words.len() == 1 || ["from", "to", "between"].contains(&words[1]))
}

/// Whether `word` is one of `subjects`, or a pin of one: `U6` or `U6.PA4` for `U6`.
fn names(word: &str, subjects: &[&str]) -> bool {
    subjects
        .iter()
        .any(|s| !s.is_empty() && word.strip_prefix(s).is_some_and(|r| r.is_empty() || r.starts_with('.')))
}

fn without_parens(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Whether a segment is just a worst-case range, like `0 to 26 V` or `0 to 3.366 V across it`.
fn is_span(s: &str) -> bool {
    let words: Vec<&str> = s.split_whitespace().collect();
    words.iter().any(|w| w.parse::<f64>().is_ok() || *w == "UNKNOWN")
        && words
            .iter()
            .all(|w| w.parse::<f64>().is_ok() || ["to", "V", "mV", "UNKNOWN", "across", "it", "A", "mA"].contains(w))
}

fn starts_with_verb(s: &str) -> bool {
    s.split_whitespace().next().is_some_and(|w| VERBS.contains(&w))
}

/// The pin or part a sentence opens with, like `J3.1 (+3V3)` in `J3.1 (+3V3) has no TVS ...`.
fn leading_pin(s: &str, subjects: &[&str]) -> Option<String> {
    let first = s.split_whitespace().next()?;
    if !names(first, subjects) {
        return None;
    }
    let after = &s[first.len()..];
    let paren = after
        .strip_prefix(" (")
        .and_then(|_| matching(after, 1))
        .map(|close| &after[..=close])
        .unwrap_or_default();
    Some(format!("{first}{paren}"))
}

/// Trims stray separators left from splitting.
fn tidy(s: &str) -> String {
    s.trim().trim_end_matches([',', ';', ':']).trim().to_string()
}

/// Greedy word wrap.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(rule: &str, subject: &str, msg: &str) -> Diagnostic {
        Diagnostic::from_message(Level::Error, rule, &[subject], msg)
    }

    #[test]
    fn a_pin_over_its_rating() {
        let g = d(
            "abs-max",
            "U6",
            "U6.PA4 (/mcu/VSENSE): 0 to 26 V: reaches 26 V, past the maximum of 4 V (STM32G0B1KBT6TR DS13560 Rev 3 \
             p66, any pin other than FT_xx)",
        );
        assert_eq!(g.headline, "U6.PA4 reaches 26 V, past the maximum of 4 V");
        assert_eq!(g.location.as_deref(), Some("U6.PA4 (/mcu/VSENSE)"));
        assert_eq!(
            g.notes,
            [
                "worst case 0 to 26 V",
                "STM32G0B1KBT6TR DS13560 Rev 3 p66, any pin other than FT_xx"
            ]
        );
    }

    #[test]
    fn both_ends_and_a_shared_citation() {
        let g = d(
            "recommended",
            "R15",
            "R15.1 (/LED0) from R15.2 (/thermal/LED0_A): -3.366 to 3.366 V: reaches -3.366 V, past the minimum of \
             -2.739 V; reaches 3.366 V, past the maximum of 2.739 V (0402WGF1200TCE V.3, February 12, 2019 p5)",
        );
        assert_eq!(g.headline, "R15.1 reaches -3.366 V, past the minimum of -2.739 V");
        assert_eq!(
            g.location.as_deref(),
            Some("R15.1 (/LED0) from R15.2 (/thermal/LED0_A)")
        );
        assert_eq!(g.notes[1], "reaches 3.366 V, past the maximum of 2.739 V");
    }

    #[test]
    fn a_sentence_keeps_its_pin_as_location() {
        let g = d(
            "floating-input",
            "U3",
            "U3.3 (CE) is on unconnected-(U3-CE-Pad3), and nothing drives that net: UNKNOWN: only U3.3 is on this \
             net, so nothing sets it",
        );
        // The location names the pin, so the headline leaves its name out.
        assert_eq!(
            g.headline,
            "U3.3 is on unconnected-(U3-CE-Pad3), and nothing drives that net"
        );
        assert_eq!(g.location.as_deref(), Some("U3.3 (CE)"));
        assert_eq!(g.notes, ["UNKNOWN: only U3.3 is on this net, so nothing sets it"]);
    }

    #[test]
    fn long_net_names_stay_in_place() {
        let g = d(
            "esd",
            "J5",
            "J5.A8 (unconnected-(J5-SBU1-PadA8)) has no clamp, and nothing says what U1.X (FOO gives no ESD rating \
             for it) survive",
        );
        assert_eq!(g.location.as_deref(), Some("J5.A8 (unconnected-(J5-SBU1-PadA8))"));
        assert!(g.headline.starts_with("J5.A8 has no clamp"));
    }

    #[test]
    fn a_short_label_keeps_its_text() {
        let g = d(
            "usb-eye",
            "USB",
            "at J6: eye height 0 mV; driven into 45 Ω; not modelled: TP1 (TP)",
        );
        assert_eq!(g.notes, ["driven into 45 Ω", "not modelled: TP1 (TP)"]);
    }

    #[test]
    fn notes_wrap_and_color_is_optional() {
        let g = d(
            "cap-voltage",
            "C2",
            "C2.1 (+VDC): 6.4 to 26 V: 26 V across it, past its 25 V rating",
        );
        let mut plain = String::new();
        g.render(&mut plain, Style::default(), false, 100);
        assert_eq!(
            plain,
            "error[cap-voltage]: C2.1: 26 V across it, past its 25 V rating\n  --> C2.1 (+VDC)\n\n"
        );
        let mut verbose = String::new();
        g.render(&mut verbose, Style::default(), true, 100);
        assert!(verbose.ends_with("  --> C2.1 (+VDC)\n   = note: worst case 6.4 to 26 V\n\n"));
        let mut colored = String::new();
        g.render(&mut colored, Style { color: true }, false, 100);
        assert!(colored.contains("\x1b[1;31merror[cap-voltage]\x1b[0m"));
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
    }

    #[test]
    fn flags() {
        let args: Vec<String> = ["--all", "-v", "--allow", "esd", "x"].map(String::from).to_vec();
        let (show, rest) = Show::parse(&args).unwrap();
        assert!(show.all && show.verbose);
        assert!(show.allows("esd") && !show.allows("abs-max"));
        assert_eq!(rest, ["x"]);
        assert!(Show::parse(&["--allow".to_string()]).is_err());
        assert!(Show::parse(&["--allow".to_string(), "--json".to_string()]).is_err());
    }
}
