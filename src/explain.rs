//! `gerberus explain RULE`: what a rule checks and why, as `rustc --explain` does for an error code. The text is
//! the rule's section of `docs/rules.md`, built into the binary, so the docs and the command say the same thing.
const RULES: &str = include_str!("../docs/rules.md");

/// The rule's section of `docs/rules.md`, without its heading, or none for a rule it doesn't explain.
pub fn text(rule: &str) -> Option<&'static str> {
    let head = format!("\n## {rule}\n");
    let start = RULES.find(&head)? + head.len();
    let end = RULES[start..].find("\n## ").map_or(RULES.len(), |e| start + e);
    Some(RULES[start..end].trim())
}

/// Every rule `docs/rules.md` explains, in its order.
pub fn rules() -> Vec<&'static str> {
    RULES.lines().filter_map(|l| l.strip_prefix("## ")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_is_its_section() {
        let esd = text("esd").unwrap();
        assert!(esd.starts_with("A connector pin a person can touch"));
        assert!(!esd.contains("## "));
        assert!(text("no-such-rule").is_none());
        assert_eq!(rules()[0], "abs-max");
    }
}
