# Developing Gerberus

For working on Gerberus itself. Build with `cargo build`; run a command from a checkout with `cargo run -- <command>`.

## Tests

`cargo test` compares each schematic's netlist with the pad nets in the matching `.kicad_pcb`, which KiCad wrote
from the same schematic. It also checks that every part on the test boards has a sound limits file covering
every pin it uses, and that the boards' pinouts agree (no errors, and only the known warnings). BOM health tests
run against a snapshot of LCSC's answers in `tests/fixtures/lcsc`, so they don't need the network. The boards come
from [roccoagain/hardware](https://github.com/roccoagain/hardware).

## Kitchen sink

`tests/fixtures/kitchen-sink` (from [roccoagain/hardware](https://github.com/roccoagain/hardware)) is one board
with a deliberate pass case and fail case for every command, one schematic sheet per feature: supply inputs and
protection, the CAN bus and its faults, debug and serial headers, two USB ports, LED current and junction
temperature, and an unpowered sensor. `tests/kitchen_sink.rs` asserts every FAIL that `check`, `pinout`, `si` and
`bom` report on it, and that each section's pass case passes; the board's README says what each part is for.

## Code style

Keep each piece of code to one clear job, with no rarely used options, unused flags or hedged wording. Edge cases that real
boards need belong in a rule when they make its results more correct. See [CLAUDE.md](../CLAUDE.md).

Modules form layers: `tests/module_layers.rs` fails if modules use each other in a loop.

## CI checks

CI (`.github/workflows/ci.yml`) runs on pushes to main, on every pull request, and weekly for new advisories:

- `cargo fmt --check` (`rustfmt.toml`) and `cargo clippy --all-targets -- -D warnings`, with the lint levels in the
  `[lints]` table of `Cargo.toml`: clippy's pedantic group, minus the lints listed there as noise for this code.
  A one-off exception is `#[expect(lint, reason = "...")]`, never `#[allow]`: it fails once the code no longer
  needs it. Shipped code may not `unwrap()` (a bad board file must give an error, not a crash); tests may.
- `cargo test` with every proxy pointed at a closed port, so a test that reaches for the network fails. The tests
  also run on macOS and Windows, which releases ship for.
- `cargo check` on the oldest supported Rust (`rust-version` in `Cargo.toml`). Day-to-day builds use the toolchain
  pinned in `rust-toolchain.toml`.
- `cargo deny check` (`deny.toml`): RustSec advisories, permissive licenses only, crates.io only.
- Line coverage with `cargo llvm-cov`, shown in the job summary and failing below 80% (about 86% today).
- `cargo machete`: no unused dependencies.
- `CI OK` passes only when every job above passed; it is the one check the main ruleset requires. Add new jobs
  to its `needs`.
- `actionlint` and `zizmor` on the workflows and `action.yml`: mistakes, script injection, unpinned actions.
  Actions are pinned to commit hashes; Dependabot (`.github/dependabot.yml`) proposes weekly updates for them and
  for `Cargo.lock`, each a week after release.

To run the formatting and clippy checks before each commit (on the working tree, not only what is staged), turn
on the hook once per clone:

```sh
git config core.hooksPath .githooks
```

## Releasing

Releases are built by [cargo-dist](https://opensource.axo.dev/cargo-dist/) (`dist-workspace.toml`,
`.github/workflows/release.yml`). To cut one, bump `version` in `Cargo.toml` on main, then push a tag for it:

```sh
git tag v0.2.0
git push origin v0.2.0
```

The release workflow builds every target, makes the installers, and publishes them as a GitHub Release named after
the tag. A tag with a pre-release suffix (`v0.2.0-rc.1`) makes a pre-release, which the installer's `latest` link
and the update check skip. After changing `dist-workspace.toml`, run `dist generate` to regenerate the workflow;
CI's `dist plan` step fails when the two disagree.

## Source layout

Every command reads a design into one design graph and checks that.

- `src/kicad/` reads schematics and boards into the design graph (`src/design.rs`: parts, pins and
  the nets that join them). `src/sexpr.rs` is the S-expression reader for KiCad files.
- `src/limits/` loads the part files (`parts/`, format in [`parts/README.md`](../parts/README.md)), with part
  families in `src/limits/family.rs`, and `src/policy.rs` the derating policy (`policy.toml`), including the
  `stress` and `traces` settings. Both are built into the binary by `build.rs`.
- `src/finding.rs` holds what every checker reports (`Finding`, `Verdict`, `Severity`, `Waived`), and
  `src/units.rs` reads component values (`10k`, `100nF`, `4.7uH`).
- `src/inputs.rs` reads a board's `inputs.toml`.
- `src/voltage/` works out every net's worst-case voltage range. `check` runs the rules in `src/rules/`
  against it, with the `esd` and `reverse-polarity` rules from `src/protection.rs` and the junction-temperature and
  forward-current rules from `src/thermal.rs`.
- `src/spice/` runs the hot-plug stress simulation in ngspice (`stress`).
- `src/doctor.rs` (`doctor`) traces each UNKNOWN finding back to what it waits on.
- `src/pinout.rs` (`pinout`), `src/bom.rs` (`bom`), `src/si/` (`si`) and `src/datasheet/` (`datasheet`) each
  back one command.
- `src/main.rs` is the command line. `src/lib.rs` declares the modules, so tests can use them.

Modules depend on each other in one direction only: no two modules reach into each other.
