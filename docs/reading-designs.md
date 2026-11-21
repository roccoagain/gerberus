# Reading designs

What Gerberus reads, and how it turns a schematic into parts, pins and nets.

It reads KiCad schematics (KiCad 10 format, including hierarchical sheets and buses) into a design graph of parts,
pins and nets. The nets and their names match KiCad's own netlist on every test board in `tests/fixtures`.

Buses work as in KiCad: vector buses (`D[0..7]`), groups (`USB{DP DM}`) and aliases (from the project file or
the sheets). A wire on a bus entry reaches a bus member only through a label naming it; the reader warns about
such wires with no label. Not supported yet: directive labels and netclass flags. The reader lists any it skips
as warnings.

Part limits live in [`parts/`](../parts/README.md): one TOML file per part number, with absolute maximum and
recommended ranges per pin, each citing its datasheet page, and `UNKNOWN` with a reason where the datasheet is
silent. Each test board has an `inputs.toml` giving the voltages on its connector pins.
