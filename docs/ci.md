# Checking a board in CI

Gerberus is also a GitHub Action. This workflow, saved as `.github/workflows/gerberus.yml` in the board's
repository, checks the schematic on every push and pull request:

```yaml
name: Gerberus

on:
  push:
    branches: [main]
  pull_request:

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: roccoagain/gerberus@main
        with:
          design: amp-board/amp-board.kicad_sch
```

It installs the latest release and runs `gerberus check`, which fails the job when a check fails with an error
(warnings don't). Accept a finding on purpose with a [waiver](waivers.md) rather than turning the check off. On a
pull request it also runs [`gerberus diff`](diff.md) against the base branch, and fails when the pull request adds an
error the base didn't have. Both reports go in the job's summary page.

| Input | Default | What it does |
|---|---|---|
| `design` | (required) | The design to check, relative to the repository. |
| `args` | none | Extra arguments for `gerberus check`, such as `--unknown`. |
| `diff` | `true` | Run `gerberus diff` against the base branch on a pull request. |

To check more than one board, add a step for each.

## Badge

The badge shows whether the last check on `main` passed. In the board's README, with your repository's owner and
name:

```markdown
[![Gerberus](https://github.com/OWNER/REPO/actions/workflows/gerberus.yml/badge.svg?branch=main)](https://github.com/OWNER/REPO/actions/workflows/gerberus.yml)
```
