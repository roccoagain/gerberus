# Gerberus

Gerberus checks a PCB design for electrical bugs before it's ordered. See [ROADMAP.md](ROADMAP.md) for where it's going.

## Status

It reads KiCad schematics (KiCad 10 format, including hierarchical sheets and buses) into a design graph of parts,
pins and nets. The nets and their names match KiCad's own netlist on every test board in `tests/fixtures`.

```sh
cargo run -- netlist path/to/board.kicad_sch
```

Buses work as in KiCad: vector buses (`D[0..7]`), groups (`USB{DP DM}`) and aliases (from the project file or
the sheets). A wire on a bus entry reaches a bus member only through a label naming it; the reader warns about
such wires with no label. Not supported yet: directive labels and netclass flags. The reader lists any it skips
as warnings.

It also reads Altium schematics: a `.PrjPcb` project (flat, hierarchical and multi-channel sheets, buses, signal
harnesses, the project's net naming options) or a single `.SchDoc`. The nets match the boards Altium compiled from
six open-source projects in `tests/fixtures/altium`, apart from differences each test lists by name.

```sh
cargo run -- netlist path/to/project.PrjPcb
```

Part limits live in [`parts/`](parts/README.md): one TOML file per part number, with absolute maximum and
recommended ranges per pin, each citing its datasheet page, and `UNKNOWN` with a reason where the datasheet is
silent. Each test board has an `inputs.toml` giving the voltages on its connector pins.

## Voltage checks

```sh
cargo run -- voltages path/to/board.kicad_sch --explain +3V3
cargo run -- check path/to/board.kicad_sch
```

`voltages` works out the worst-case voltage range of every net from the board's `inputs.toml`, through diodes,
regulators and feedback dividers (see [`parts/README.md`](parts/README.md#net-voltages)). `--explain` prints why.

`check` compares those ranges with the part limits. Each check comes out PASS, FAIL or UNKNOWN, and a FAIL
cites the numbers, their datasheet pages and how each net got its range. The ten rules:

| Rule | FAILs when | Severity |
| --- | --- | --- |
| `abs-max` | a pin can go past its absolute maximum ratings | error |
| `recommended` | a pin can leave its recommended operating range (for passives and connectors: their rating) | warning (error for ratings) |
| `tvs-breakdown` | a rail reaches a TVS or ESD diode's lowest breakdown voltage, so it conducts continuously | error |
| `tvs-standoff` | a rail rises above a TVS diode's stand-off voltage | warning |
| `cap-voltage` | a capacitor sees more than its rated voltage (error), or more than `policy.toml` allows (warning) | error / warning |
| `resistor-power` | a resistor's worst-case V²/R is over its rating (error) or the policy's share of it (warning) | error / warning |
| `floating-input` | an input pin is on a net nothing drives (UNKNOWN when only open-drain or tri-state outputs, connectors or unmodelled parts reach it) | error |
| `unpowered` | a power pin is on a net nothing supplies | error |
| `regulator-headroom` | a regulator's supply can fall below what its datasheet output range needs | warning |
| `unchecked` | (UNKNOWN only) a fitted part or pin has no limits to check against | |

A bound relative to another pin (`VDD + 0.3 V`) passes only if it holds however the two nets move, or if
everything that drives the pin is itself bounded by that pin's net (pull-ups to it, outputs of parts it alone
supplies, diodes and regulators fed from it; not switch nodes that can overshoot it). `bidirectional` makes only
a TVS's breakdown a magnitude; abs max, recommended and stand-off ranges stay signed. A regulator that isn't held leaves its
rail's low end as a floor, so a check can pass on it but never fail on it. `--all` lists passes, `--json` prints
the findings, and the command exits with status 1 when any error FAILs.

The voltage across two pins is narrowed by a `[[differential]]` in `inputs.toml` (a CAN bus's CANH − CANL), and
by nets of passive parts: a net reached only through resistors, LEDs and jumpers, with no capacitor or inductor
on it, sits between the nets around it, so a termination resistor behind a jumper sees at most the bus's differential voltage.

Fault cases in `inputs.toml` (such as VBUS shorted onto CANH) are each worked out on their own and checked
against `abs-max`, `tvs-breakdown` and `cap-voltage` (without derating). Resistor power and other passives'
ratings are checked too, but can only PASS or be UNKNOWN there, since a fault's two ends needn't move together.
Their findings say which fault they come from; `voltages --fault CASE` shows one case's nets.

On the test boards:

- Rev 1 of the CAN encoder fails `tvs-breakdown`: its SMAJ18A (breakdown from 20 V) sits on a bus that reaches
  25.2 V. With VBUS shorted onto CANH or CANL, its TCAN330 (bus pins ±14 V) fails `abs-max`, as its own
  DESIGN.md warned. Rev 2's TCAN1044AV passes both faults.
- Both revisions fail `resistor-power` and `recommended` on R4 (its rated voltage is √(P·R) = 2.74 V), the 0402 120 Ω termination: 5 V of dominant bus differential
  (the TCAN1044A's widest VOD row, at the 2240 Ω arbitration load) is 213 mW against its 62.5 mW; even the
  3 V maximum at 50 to 65 Ω is 77 mW. The check is for a bus held dominant; normal traffic is dominant only part
  of the time.
- The sniffer has no errors.

## Pinout check

```sh
cargo run -- pinout path/to/board.kicad_sch
```

checks that every part's symbol pins, footprint pads (from the `.kicad_pcb` next to the schematic, or `--pcb`)
and datasheet pin table (the `[pins]` map of its part file) agree. Errors: a symbol pin with no pad, a datasheet
pin missing from the symbol, a symbol pin the datasheet doesn't have, a footprint missing a datasheet pin, pins
whose names look swapped, a do-not-connect pin wired to a net, a symbol with no footprint on the board. Warnings,
for a person to look at: pin names that differ without matching another pin, a polarised diode drawn with an
unpolarised symbol (with the net its cathode lands on), extra pads, and a board out of step with the schematic.
It exits with status 1 on any pinout error and 2 when it can't run (bad arguments, unreadable file).

Names are compared loosely: case, KiCad markup (`~{CS}`, `V_{IN}`), alternate functions (`PA9/PA11`,
`PB8-BOOT0`) and a short list of synonyms (`GND`/`VSS`, `VIN`/`IN`, `EN`/`TRI-STATE`, ...) all match. Names with
a polarity sign (`IN+`, `-IN`) never match through a synonym, and hyphenated names (`LED-A`) are split only after
a port pin (`PB8-BOOT0`).

## BOM health

```sh
cargo run -- bom path/to/board.kicad_sch --boards 10
```

For each BOM line (parts grouped by the same part number as `parts/`), it looks up the schematic's `LCSC`
field at LCSC and reports:

- **ERROR** when LCSC has too little stock for the number of boards (default 10), or when the LCSC number is a
  different part from the `MPN` field.
- **WARNING** when LCSC no longer lists the part or lists it with a status other than `normal`, or when LCSC
  shows no similar part from another maker in the same package with enough stock (single source). Plain
  resistors and capacitors are exempt from the single-source warning.
- **UNKNOWN** when there is no LCSC number to look up.

Similar parts are LCSC's suggestions and are shown as candidates only: nobody has checked their pinout or
ratings. LCSC needs no API key, but it can't tell whether the manufacturer has marked a part NRND or EOL, so
manufacturer lifecycle is reported as unknown. Answers are cached in `~/.cache/gerberus/lcsc` for 24 hours;
`--offline` uses only the cache; when a fresh lookup fails, an older cached answer is used with a warning. The
command exits with status 1 when any line has an error and 2 when it can't run.

## USB signal integrity

```sh
cargo run --release -- si path/to/board.kicad_pcb --plots plots/
```

It finds USB 2.0 differential pairs on a `.kicad_pcb` by net name (`D+`/`D-`, `DP`/`DM`, `_P`/`_N`), traces
each line through its tracks, vias and pads from the PHY to the connector, and checks it for high speed
(480 Mb/s):

| Rule | Checks | Limit |
| --- | --- | --- |
| `usb-impedance` | Differential impedance of every stretch of the pair, from a 2D field solver run on its cross-section (trace width, gap, copper and dielectric thickness, εr from the board's stackup, the planes above and below) | 81–99 Ω (TI SPRAAR7J Table A-1) |
| `usb-skew` | D+ and D− length difference | 50 mil (1.27 mm) |
| `usb-vias` | Vias on each line, and equal counts on both | 4 per line (SPRAAR7J) |
| `usb-stubs` | Copper that branches off a line to a test point, ESD part or anything else off the through path | under 200 mil (TUSB1210 Table 10-1, item 2.13) |
| `usb-reference` | An unbroken plane on the next layer under every stretch, and distance from that plane's edge | no gaps; 25 mil from the edge (items 2.14, 2.15) |
| `usb-length` | Total length | 12 inches |
| `usb-capacitance` | Capacitance of parts on each line, from their `capacitance` entries in `parts/` | 4 pF (item 1.06) |
| `usb-eye` | Simulated eye at the connector | USB 2.0 Template 1 (Figure 7-13) |

A stretch outside the impedance band that is 3 mm or shorter (about 20 ps, too short to reflect on its own), such
as where a pair fans out to reach pads, is left to the eye simulation instead of failing the impedance rule. The field solver is a finite-volume Laplace
solver; it agrees with closed-form microstrip and stripline formulas to 0.3 %.

The eye comes from a channel simulation. Each line becomes a cascade of transmission-line sections (impedance,
delay, conductor and dielectric loss from the solver and stackup), via inductance and capacitance, open stubs and
part capacitance. A 480 Mb/s PRBS7 pattern with 500 ps edges (the fastest USB allows, Table 7-8) from a 45 Ω,
400 mV driver goes through it into a 45 Ω termination, and the differential eye at the connector is checked
against the inner hexagon and outer limits of Template 1. The model is odd-mode only and ignores crosstalk from
other nets, so a pass means the layout leaves the eye open, not that a real cable and receiver will. Parts on a
line with no capacitance in `parts/` are listed as not modelled.

`--plots DIR` writes an eye diagram and a TDR plot (the impedance the driver sees along each line) for each pair
as SVG. `--json` prints the report as JSON, and `--all` lists passes as well as failures. The command exits with
status 1 when any rule fails with an error.

The test board `tests/fixtures/usb-si-testbed` (from [roccoagain/hardware](https://github.com/roccoagain/hardware))
has one port routed to TI's guidelines, which passes, and one that breaks every rule above apart from length, which
fails each of them and closes the eye.

## Datasheet checks

```sh
cargo run -- datasheet verify parts/TPS560430YFDBVR.toml
cargo run -- datasheet blank parts/TPS560430YFDBVR.toml > second.toml   # a second reader fills it in blind
cargo run -- datasheet compare parts/TPS560430YFDBVR.toml second.toml
```

Part files are written by reading datasheets, by a person or an LLM. Gerberus checks that reading in two
independent ways, and decides nothing itself:

- `verify` downloads the datasheet (cached in `~/.cache/gerberus/datasheets`; `--pdf` reads a local copy) and
  looks for every cited value on its page with poppler's `pdftotext`. It finds the table's MIN, TYP and MAX
  columns and its UNIT column, so a typical value filed as a limit, a value in the wrong unit or one from the
  wrong page shows up. A temperature on a storage row and a value in the other end's column are mismatches;
  a `derived` range's arithmetic is evaluated against the bound. Each value comes out CONFIRMED (right column, unit and a row naming the pin or part
  number), DERIVED (worked out from printed numbers by the range's `derived` arithmetic, or a non-polar rating's
  negative end), PLAUSIBLE (on the page, with the doubts listed), NO TEXT (a scanned page to read by eye),
  NOT FOUND or MISMATCH. It exits with status 1 on the last two.
- `blank` writes the part file with every datasheet value replaced by `"?"`, and `compare` lists every value
  where two readings disagree. Two readers who haven't seen each other's work rarely make the same mistake.

[`.claude/skills/datasheet-to-part`](.claude/skills/datasheet-to-part/SKILL.md) is the whole procedure for an
agent: read, write, verify, a blind second reading by a separate agent, and settling every difference against
the page.

## Tests

`cargo test` compares each schematic's netlist with the pad nets in the matching `.kicad_pcb`, which KiCad wrote
from the same schematic. It also checks that every part on the test boards has a sound limits file covering
every pin it uses, and that the boards' pinouts agree (no errors, and only the known warnings). BOM health tests
run against a snapshot of LCSC's answers in `tests/fixtures/lcsc`, so they don't need the network. The boards come
from [roccoagain/hardware](https://github.com/roccoagain/hardware).

## Checks

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
