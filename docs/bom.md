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
`--offline` uses only the cache; when a fresh lookup fails, an older cached answer is used with a warning. The
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

When such a line is short of stock or rated too low, LCSC's similar parts are looked up in full and the ones
with the same value and size, ratings at least as good and enough stock are offered in a `help:` line, JLCPCB
basic parts first. These are checked against the numbers above, so unlike other parts' candidates they can be
used as they are. None are offered when a rating to check against is unknown: LCSC sometimes leaves out a
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
  capacitor on an extended part gets a basic equivalent when LCSC's similar parts include one.
- Stock is LCSC's retail stock. JLCPCB keeps its own assembly stock of the same C numbers, which can differ.
  LCSC running short of a JLCPCB basic part is therefore a warning, not an error; check JLCPCB's parts library
  before replacing it.

The report is rustc-style like `check`'s: `--all` lists the lines with nothing wrong, `--color` sets colour, and
`--parts DIR` adds a part folder (a `parts/` folder next to the schematic is read anyway).

## Mouser and DigiKey

```sh
MOUSER_API_KEY=... gerberus bom path/to/board.kicad_sch --from mouser
DIGIKEY_CLIENT_ID=... DIGIKEY_CLIENT_SECRET=... gerberus bom path/to/board.kicad_sch --from digikey
```

For a board bought from Mouser or DigiKey, `--from` looks each line up by its `MPN` field instead of an LCSC
number. These give the manufacturer's lifecycle, which LCSC can't:

- **ERROR** when the part is obsolete, or stock is short for the build.
- **WARNING** when it is being discontinued (end of life, last time buy) or not recommended for new designs.
- **UNKNOWN** for a line with no `MPN` field (a value such as `10k` isn't a part number), or a lifecycle status
  gerberus doesn't recognise.

Neither lists similar parts, so second sources aren't judged; use LCSC for that. Answers are cached in
`~/.cache/gerberus/mouser` or `~/.cache/gerberus/digikey` (or `mouser` and `digikey` folders inside `--cache`),
and `--offline` needs no key. Mouser lookups are spaced 2 s apart to stay inside its limit, so a 50-line BOM
takes under two minutes the first time.

Getting a key (both are free):

- **Mouser**: sign in at mouser.com, open the API Hub (mouser.com/api-hub), and request a Search API key. Put it
  in `MOUSER_API_KEY`. Mouser allows about 30 lookups a minute and 1000 a day.
- **DigiKey**: sign in at developer.digikey.com, create an organization and a production app with the Product
  Information v4 API, and put its client id and secret in `DIGIKEY_CLIENT_ID` and `DIGIKEY_CLIENT_SECRET`.

These lookups were written against the APIs' documented answers, not tested against the live services (the
development environment can't reach them). If one fails, the error shows the distributor's answer; please report
it.

## Order list

```sh
gerberus bom path/to/board.kicad_sch --csv order.csv --boards 10
```

writes the BOM as a CSV file with quantities for the build: `Quantity`, `Manufacturer Part Number`,
`Manufacturer` (from a `Manufacturer` or `MFR` field), `Reference Designators`, `Value` and `LCSC Part Number`.
Upload it to DigiKey's BOM Manager or Mouser's BOM tool, and match the columns when asked. Lines with no `MPN`
field leave that column empty, so they are matched by hand. `--csv` looks nothing up.
