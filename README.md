# Scoutfly

Scoutfly checks a PCB design for electrical bugs before it's ordered. See [ROADMAP.md](ROADMAP.md) for where it's going.

## Status

It reads KiCad schematics (KiCad 10 format, including hierarchical sheets) into a design graph of parts, pins and
nets. The nets and their names match KiCad's own netlist on every test board in `tests/fixtures`. No checks run yet.

```sh
cargo run -- netlist path/to/board.kicad_sch
```

Not supported yet: buses, directive labels and netclass flags. The reader lists any it skips as warnings.

Part limits live in [`parts/`](parts/README.md): one TOML file per part number, with absolute maximum and
recommended ranges per pin, each citing its datasheet page, and `UNKNOWN` with a reason where the datasheet is
silent. Each test board has an `inputs.toml` giving the voltages on its connector pins.

## Tests

`cargo test` compares each schematic's netlist with the pad nets in the matching `.kicad_pcb`, which KiCad wrote
from the same schematic. It also checks that every part on the test boards has a sound limits file covering
every pin it uses. The boards come from [roccoagain/hardware](https://github.com/roccoagain/hardware).
