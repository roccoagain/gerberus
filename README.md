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
cites the numbers, their datasheet pages and how each net got its range. The rules:

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
| `esd` | an external connector pin's net has no TVS or ESD clamp, and a part on it (directly, or through one series resistor, inductor or diode with no clamp behind it) has no system-level ESD rating to `policy.toml`'s level | warning |
| `reverse-polarity` | a supply input connected backwards (its fault case `<name> reversed`) takes a part past its absolute maximum ratings | error |
| `junction-temperature` | a part's junction, at the board's ambient plus its dissipation × θJA, passes its maximum (error) or comes within `policy.toml`'s `[junction]` margin of it (warning) | error / warning |
| `forward-current` | a diode or LED carries more than its forward current rating (error) or `policy.toml`'s share of it (warning) | error / warning |
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

Every supply input (one with a positive minimum) is also checked connected backwards, as the fault case
`<name> reversed`, unless its `inputs.toml` entry says why it can't be (`reverse = "keyed connector"`). A series
diode blocks it: its cathode stays at 0 V or above. A regulator fed a negative supply may pass it to its output (many CMOS
regulators have a body diode from output to input), so its output's low end becomes a floor at the supply's
minimum: pins on it come out UNKNOWN rather than PASS. `reverse-polarity` sums the case up, counting only
findings that normal operation doesn't already have; the summary isn't counted again in the error total.

The `esd` rule looks at every connector pin `inputs.toml` names (inputs and undriven pins, not their ground
references). A TVS or ESD diode on the net passes it, if it breaks down below the absolute maximum of the IC
pins beside it (an SMF26A on a 3.3 V logic pin doesn't count). A pull-up or pull-down isn't followed onto its
rail. So does a clamp behind the one series part between the
connector and the ICs it protects (VBUS → D1 → TVS). Otherwise every IC pin on the net needs an IEC 61000-4-2
rating (`[[esd]]` in its part file) at `policy.toml`'s `[esd]` level: an HBM or CDM rating only covers handling
in a factory, so it FAILs as a warning, and a part with no rating at all is UNKNOWN.

On the test boards:

- Rev 1 of the CAN encoder fails `tvs-breakdown`: its SMAJ18A (breakdown from 20 V) sits on a bus that reaches
  25.2 V. With VBUS shorted onto CANH or CANL, its TCAN330 (bus pins ±14 V) fails `abs-max`, as its own
  DESIGN.md warned. Rev 2's TCAN1044AV passes both faults.
- Both revisions fail `resistor-power` and `recommended` on R4 (its rated voltage is √(P·R) = 2.74 V), the 0402 120 Ω termination: 5 V of dominant bus differential
  (the TCAN1044A's widest VOD row, at the 2240 Ω arbitration load) is 213 mW against its 62.5 mW; even the
  3 V maximum at 50 to 65 Ω is 77 mW. The check is for a bus held dominant; normal traffic is dominant only part
  of the time.
- The sniffer's only errors are its supply connected backwards: nothing blocks VIN, so the ME6211's CE pin sees
  -5.5 V and its output sits above its reversed input. Both CAN encoders pass, as D1 blocks a reversed VBUS.
- Every debug header (SWD) and the sniffer's UART reach STM32 pins rated only ±1 to ±2 kV HBM, so they fail `esd`
  as warnings. The CAN pins pass on the transceivers' own system-level ratings: rev 1's TCAN330 is rated ±12 kV contact
  (its NUP2105L breaks down only at up to 32 V, above the TCAN330's 14 V bus pins), and rev 2's TCAN1044AV ±8 kV.

## Junction temperature and derating

```sh
cargo run -- loads path/to/board.kicad_sch
```

`loads` adds up what each rail supplies: every part's `[[supply]]` maximum, LED chains ((V − VF min) / R min, or V / R min
below the current VF is rated at; charged to the supply of the IC that drives them), and resistors to ground. A linear regulator passes its output's
load to its input and dissipates (VIN max − VOUT min) × I plus its own supply current; when its supply can sag out
of regulation, the held output is used at the top of the supply, since in dropout it drops less. A switching
regulator needs a guaranteed `efficiency` in its part file. Datasheets give efficiency only as typical curves, so
the switcher, its input diode and the rail feeding it come out UNKNOWN. A series diode carries its downstream load.

`junction-temperature` takes Tj = Ta + P × θJA for every part with a dissipation. Ta is the top of
`inputs.toml`'s `temperature`; without it, the part's own rated ambient is used. A part that would pass its
maximum there is UNKNOWN, with the ambient it is safe up to, rather than a FAIL. The θJA in datasheets is for a
JEDEC test board; a real board's copper changes it. Supply currents are maximums at the datasheet's worst
condition, so the result is an upper bound.

Derating for reliability is `policy.toml`'s: capacitor voltage at 80 %, resistor power and diode and LED forward
current at 50 %, and junctions 20 °C below their maximum. A FAIL against a derating is a warning.

On the test boards (normal operation, at each part's rated ambient, as no board gives a temperature):

- Rev 2's STM32F042 dissipates 84 mW at θJA 118 °C/W: 95 °C at 85 °C ambient, under its 105 °C, but inside the
  20 °C margin above 75 °C. Rev 1's TCAN330 (60 mA dominant × 3.46 V at 154.4 °C/W) would reach 157 °C at its rated 125 °C,
  so it is UNKNOWN, safe up to 118 °C.
- The LDOs (XC6206 at 114 mW, ME6211 at 74 mW) and most sensors have no θJA in their datasheets, so they are
  UNKNOWN with their dissipation. The switchers (TPS560430, AP63200) give no efficiency.
- The LEDs pass `forward-current` at 3.4 to 3.5 mA against 25 to 30 mA ratings.

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
