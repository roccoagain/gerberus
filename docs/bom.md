# BOM health

```sh
gerberus bom path/to/board.kicad_sch --boards 10
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
when a fresh lookup fails, an older cached answer is used with a warning. The
command exits with status 1 when any line has an error and 2 when it can't run.

Resistors and capacitors (KiCad `Device:R` and `Device:C` symbols) are compared by what they are rather than
by part number, using LCSC's parameter table: value against the schematic's value, size against the footprint
(`R_0603_1608Metric` is 0603), and tolerance, rated voltage, rated power and dielectric against the part file of
the `MPN` field when gerberus has one (a built-in family such as Yageo RC_L or Samsung CL, or a file in
`parts/`).

- **ERROR** when the LCSC number is a different value or size, or is rated below the schematic's part (a 25 V
  capacitor where the MPN is 50 V, ±5 % where it is ±1 %, X5R where it is X7R).
- **WARNING** when it is another maker's part with the same value, size and ratings: JLCPCB will fit that part,
  but gerberus's other checks read the MPN field's part file, so the help line names the MPN to put there.
  Without a part file for the MPN only value and size are compared, and the note says so.

- **UNKNOWN** when the schematic's value can't be read, the footprint gives no chip size, or LCSC's answer has
  no parameter table (an entry cached by an older gerberus; delete `~/.cache/gerberus/lcsc` to refresh it). An LCSC
  number whose listing has parameters but no resistance or capacitance is an **ERROR**: it isn't a plain
  resistor or capacitor. Every part of a line is compared, so one with another footprint is caught too.

When such a line is short of stock or rated too low, the first 8 of LCSC's similar parts are looked up in full,
and those with the same value and size, ratings at least as good (each rating the schematic's part file doesn't
give is taken from LCSC's listing) and enough stock are offered in a `help:` line, JLCPCB basic parts first.
They are checked against LCSC's parameters, not against datasheets, so read the datasheet of the one you pick.
Similar parts that couldn't be looked up are counted in a note. None are offered when a rating to check against is unknown: LCSC sometimes leaves out a
capacitor's dielectric, and without it (or a part file for the MPN) an X7R part could be offered for a C0G one.

Part numbers match when they differ only by a packing or plating suffix after a comma or bracket
(`PESD5V0S1BA,115`, `S2B-XH-A(LF)(SN)`); letters added to the part number itself (`PESD5V0S1BAF`) are another part.

## Built by JLCPCB

```sh
gerberus bom path/to/board.kicad_sch --jlcpcb --boards 10
```

For a board JLCPCB will assemble from LCSC's stock:

- A part with no LCSC number, an LCSC field that isn't one, or a number LCSC doesn't list is an **ERROR**
  instead of unknown, with a link to LCSC's search for its MPN. gerberus can't search LCSC itself, so picking
  the replacement is up to you; rerun to check it.
- The closing notes count JLCPCB extended parts (JLCPCB charges a loading fee for each one), and a resistor or
  capacitor on an extended part gets a `todo[bom-assembly]` line naming a basic equivalent when LCSC's similar
  parts include one.
- Stock is LCSC's retail stock. JLCPCB keeps its own assembly stock of the same C numbers, which can differ.
  LCSC running short of a JLCPCB basic part is therefore a warning, not an error; check JLCPCB's parts library
  before replacing it.

The report is rustc-style like `check`'s, and its counts are of BOM lines, each at its worst finding: `--all` lists the lines with nothing wrong, and
`--parts DIR` adds a part folder (a `parts/` folder next to the schematic is read anyway).
