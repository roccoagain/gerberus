use std::collections::BTreeSet;

const REQUIRED_CHECK_JOB: &str = "ci-ok";

fn workflow() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/.github/workflows/ci.yml")).unwrap()
}

fn job_ids(workflow: &str) -> BTreeSet<String> {
    workflow
        .lines()
        .skip_while(|line| *line != "jobs:")
        .filter_map(|line| line.strip_prefix("  ")?.strip_suffix(':'))
        .filter(|id| !id.starts_with(' '))
        .map(str::to_string)
        .collect()
}

fn required_check_needs(workflow: &str) -> BTreeSet<String> {
    let job = workflow.split(&format!("\n  {REQUIRED_CHECK_JOB}:\n")).nth(1).unwrap();
    let needs = job
        .lines()
        .find_map(|line| line.trim().strip_prefix("needs: "))
        .unwrap();
    needs
        .trim_matches(['[', ']'])
        .split(',')
        .map(|id| id.trim().to_string())
        .collect()
}

#[test]
fn the_required_check_waits_for_every_other_job() {
    let text = workflow();
    let mut others = job_ids(&text);
    others.remove(REQUIRED_CHECK_JOB);
    assert_eq!(required_check_needs(&text), others);
}
