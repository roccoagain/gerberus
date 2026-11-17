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
