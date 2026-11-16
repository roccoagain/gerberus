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

On the test boards, rev 1 of the CAN encoder fails `tvs-breakdown`: its SMAJ18A (breakdown from 20 V) sits on a
bus that reaches 25.2 V. Rev 2 and the sniffer have no errors.

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

## Tests

`cargo test` compares each schematic's netlist with the pad nets in the matching `.kicad_pcb`, which KiCad wrote
from the same schematic. It also checks that every part on the test boards has a sound limits file covering
every pin it uses, and that the boards' pinouts agree (no errors, and only the known warnings). BOM health tests
run against a snapshot of LCSC's answers in `tests/fixtures/lcsc`, so they don't need the network. The boards come
from [roccoagain/hardware](https://github.com/roccoagain/hardware).
