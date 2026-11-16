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
