# Working on Gerberus

Read [docs/development.md](docs/development.md) first.

## No comments

Comments are banned in code: no `//`, `///`, `//!` or `/* */` in Rust, and no `#` comment lines in the build and CI
config. When you feel the need for a comment, make the code say it instead: a better name, a named constant, an
extracted function, or a simpler design. `tests/no_comments.rs` enforces this, so `cargo test` fails on any comment.

Allowed: tool directives (`#!` shebangs, `# shellcheck`, `# zizmor:`), the version tag after a pinned action's SHA
(Dependabot reads it), and the `reason` of an `#[expect(...)]`. Part files, `policy.toml` and test fixtures are data,
not code; their notes cite datasheets and stay.

## No nuance

Each piece of code does one clear thing. Don't add special cases, hedged heuristics or rarely used options; when
something only matters sometimes, make it always matter or delete it.
