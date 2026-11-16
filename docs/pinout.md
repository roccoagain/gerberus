# Pinout check

```sh
gerberus pinout path/to/board.kicad_sch
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
