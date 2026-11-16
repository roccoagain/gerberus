# Gerberus

Gerberus checks a PCB design for electrical bugs before you order it: parts pushed past their datasheet limits,
missing protection, swapped pins, parts you can't buy, and bad USB routing. It reads KiCad designs, and Altium
schematics for the schematic checks.

## Install

On macOS or Linux, paste this into a terminal:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/roccoagain/gerberus/releases/latest/download/gerberus-installer.sh | sh
```

On Windows, paste this into PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/roccoagain/gerberus/releases/latest/download/gerberus-installer.ps1 | iex"
```

Then open a new terminal window, so it can find the `gerberus` command. Run `gerberus update` later to get the
latest version.

Software developers who have [Rust](https://rustup.rs) installed can build it instead (you don't need this if you
used the installer above; to update a copy built this way, run the same command again):

```sh
cargo install --locked --git https://github.com/roccoagain/gerberus
```

Two commands need another program installed: `gerberus stress` needs [ngspice](https://ngspice.sourceforge.io/),
and `gerberus datasheet verify` needs poppler's `pdftotext`. More in [docs/install.md](docs/install.md).

## Quick start

Try it on one of the example boards. Download them with git:

```sh
git clone https://github.com/roccoagain/gerberus
cd gerberus/tests/fixtures/sniffer
gerberus check sniffer.kicad_sch
```

(No git? On the [GitHub page](https://github.com/roccoagain/gerberus) click **Code → Download ZIP**, unzip it, and
`cd` into `gerberus-main/tests/fixtures/sniffer` instead.)

It prints each problem it found, then a count:

```text
error[abs-max]: U1.CE reaches -5.5 V, past the minimum of -0.3 V
  --> U1.CE (+5V), in fault "VIN reversed"
   = note: worst case -5.5 to -3.6 V
   = note: ME6211C33M5G-N V14 p5
   = help: add a divider, clamp or series resistor, or choose a part rated higher
...
error: 2 errors, 7 warnings, 29 unknown, 96 passed
```

An **error** means a part can be damaged or the board won't work; a **warning** is a risk worth a look. Each one
gives the numbers it compared, the datasheet page they came from, and a fix. **Unknown** means Gerberus couldn't
decide, usually because a part has no part file yet; it never counts as a pass. Add `--unknown` to list those,
`--all` to list the passes too, and `--verbose` to see how each net got its voltage range. The sniffer's 2 errors
are its supply connected backwards, which damages the regulator; its warnings are headers with no ESD protection.

On your own board, point it at the root schematic (`.kicad_sch`) or the Altium project (`.PrjPcb`). `check`
needs an `inputs.toml` next to the schematic that says what voltages arrive on each connector pin (for example, 3.6
to 5.5 V on J1 pin 1); copy the one in `tests/fixtures/sniffer` as a start, and see [the
format](parts/README.md#board-inputs).

## What it checks

| Command | What it checks |
| --- | --- |
| `gerberus check` | Every pin against its datasheet limits at its net's worst-case voltage, TVS diodes, capacitor voltage, resistor power, undriven inputs, regulator headroom, junction temperature, ESD protection on connectors, and a supply connected backwards. [More](docs/voltage-checks.md) |
| `gerberus stress` | Plugging the board in, simulated in SPICE with part tolerances: voltage spikes on every pin. [More](docs/stress.md) |
| `gerberus pinout` | Symbol pins, footprint pads and the datasheet pin table agree. [More](docs/pinout.md) |
| `gerberus bom` | Every part is in stock at LCSC, still listed, and has a second source. [More](docs/bom.md) |
| `gerberus si` | USB 2.0 pairs on the board: impedance, skew, vias, stubs, reference planes, and a simulated eye diagram. [More](docs/usb-si.md) |
| `gerberus datasheet` | A part file against its datasheet, value by value. [More](docs/datasheets.md) |

Part limits come from a built-in library of part files, each value cited to its datasheet page. A part with no
part file comes out UNKNOWN rather than PASS. To add one, see [parts/README.md](parts/README.md).

Run `gerberus --help` for every command and option.

## Links

- [Docs](docs/README.md): every check in detail.
- [Part file format](parts/README.md).
- [Roadmap](ROADMAP.md).
- [Developing Gerberus](docs/development.md): tests, CI and releases.
- [Example boards](https://github.com/roccoagain/hardware).
