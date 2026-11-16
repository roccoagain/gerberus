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

## CI checks

CI (`.github/workflows/ci.yml`) runs on pushes to main, on every pull request, and weekly for new advisories:

- `cargo fmt --check` (`rustfmt.toml`) and `cargo clippy --all-targets -- -D warnings`, with the lint levels in the
  `[lints]` table of `Cargo.toml`: clippy's pedantic group, minus the lints listed there as noise for this code.
- `cargo test` with every proxy pointed at a closed port, so a test that reaches for the network fails.
- `cargo check` on the oldest supported Rust (`rust-version` in `Cargo.toml`). Day-to-day builds use the toolchain
  pinned in `rust-toolchain.toml`.
- `cargo deny check` (`deny.toml`): RustSec advisories, permissive licenses only, crates.io only.
- Line coverage with `cargo llvm-cov`, shown in the job summary and failing below 80% (about 86% today).

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

- `src/kicad/` and `src/altium/` read schematics and boards into the design graph (`src/design.rs`: parts, pins and
  the nets that join them). `src/sexpr.rs` is the S-expression reader for KiCad files.
- `src/limits.rs` loads the part files (`parts/`, format in [`parts/README.md`](../parts/README.md)) and
  `src/policy.rs` the derating policy (`policy.toml`). Both are built into the binary by `build.rs`.
- `src/inputs.rs` reads a board's `inputs.toml`.
- `src/voltage.rs` works out every net's worst-case voltage range; `src/rules.rs` and `src/protection.rs` check
  pins against it (`check`); `src/thermal.rs` adds loads and junction temperature (`loads`).
- `src/spice/` runs the hot-plug stress simulation in ngspice (`stress`).
- `src/pinout.rs` (`pinout`), `src/bom.rs` (`bom`), `src/si/` (`si`), `src/datasheet.rs` (`datasheet`) and
  `src/update.rs` (`update` and the daily release check) each back one command.
- `src/main.rs` is the command line.
