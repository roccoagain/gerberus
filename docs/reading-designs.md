# Reading designs

What Gerberus reads, and how it turns a schematic into parts, pins and nets.

It reads KiCad schematics (KiCad 10 format, including hierarchical sheets and buses) into a design graph of parts,
pins and nets. The nets and their names match KiCad's own netlist on every test board in `tests/fixtures`.

Buses work as in KiCad: vector buses (`D[0..7]`), groups (`USB{DP DM}`) and aliases (from the project file or
the sheets). A wire on a bus entry reaches a bus member only through a label naming it; the reader warns about
such wires with no label. Not supported yet: directive labels and netclass flags. The reader lists any it skips
as warnings.

## tscircuit

Gerberus reads a [tscircuit](https://tscircuit.com) board as its Circuit JSON, the `circuit.json` that `tsci build`
writes, or as the board's `.tsx`, which it hands to the tscircuit CLI (`tsci export`; install it with
`npm install -g tscircuit`) to get that Circuit JSON. tsci runs in the board's folder and leaves there what it always does (its `.tscircuit/`
cache and a `.circuit.json` copy). Pass either where a command takes a design:

```sh
gerberus check index.circuit.tsx
gerberus check dist/index/circuit.json
```

The reader takes the schematic side of Circuit JSON: parts (`source_component`), their pins (`source_port`), and
the traces and named nets (`source_trace`, `source_net`) that join them. Its nets match tscircuit's own
connectivity on every board in `tests/fixtures/tscircuit` and on the 123-part RP2350 handheld in
[tscircuit/tscircuitboy-advance](https://github.com/tscircuit/tscircuitboy-advance), pin for pin. A few
things follow from how tscircuit works:

- Each part gets the KiCad standard symbol of its kind (`Device:R`, `Device:LED`, `Connector:TestPoint`), so the
  checks treat it as they treat the same part in KiCad. A capacitor whose pins aren't interchangeable is
  polarised (`Device:CP`), and its `maxVoltageRating` is its rated voltage. Chips and other parts keep their
  tscircuit kind, such as `tscircuit:simple_chip`.
- The value is the resistance, capacitance, inductance or frequency, and the part number is
  `manufacturerPartNumber`. A JLCPCB or LCSC number in `supplierPartNumbers` is the part's LCSC number for `bom`.
- Pin numbers are tscircuit's, which are the footprint's pad numbers. Diode and transistor pins are named by
  their role (`A`, `K`; `G`, `D`, `S`; `B`, `C`, `E`) so `pinout` can compare them with a part file. tscircuit
  numbers a diode's anode 1, where most diode datasheets number the cathode 1, and `pinout` reports that.
- Pin attributes give the pin types: `requiresPower` and `requiresGround` are power inputs, `providesPower` a
  power output, `isInput` an input, and so on. A `noConnect` pin is marked not connected.
- A `subcircuit` group keeps its nets to itself, as tscircuit does: `net.VCC` inside group A is a net of its
  own, named `/A/VCC`. Parts of a group used twice share names, so each is named after its group too (`R1_A`).
- Pins a part joins inside itself (`internallyConnectedPins`) are on one net.
- A part with no board footprint is not on the board, and one with `doNotPlace` is not fitted.

`traces`, `si`, the footprint half of `pinout` and the board drawing of `check --html` read KiCad's board file, so
they don't run on a tscircuit board yet.

## Part limits

Part limits live in [`parts/`](../parts/README.md): one TOML file per part number, with absolute maximum and
recommended ranges per pin, each citing its datasheet page, and `UNKNOWN` with a reason where the datasheet is
silent. Each test board has an `inputs.toml` giving the voltages on its connector pins.
