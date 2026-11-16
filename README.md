# Gerberus

Gerberus checks a PCB design for electrical bugs before you order it: parts pushed past their datasheet limits,
missing protection, swapped pins, parts you can't buy, and bad USB routing. It reads KiCad and Altium designs.

## Install

On macOS or Linux, paste this into a terminal:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/roccoagain/gerberus/releases/latest/download/gerberus-installer.sh | sh
```

On Windows, paste this into PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/roccoagain/gerberus/releases/latest/download/gerberus-installer.ps1 | iex"
```

Or, if you have [Rust](https://rustup.rs) installed, build it yourself:

```sh
cargo install --locked --git https://github.com/roccoagain/gerberus
```

Run `gerberus update` to get the latest version. More in [docs/install.md](docs/install.md).

## Quick start

Try it on one of the example boards. Download them with git (or download the repository as a ZIP from GitHub):

```sh
git clone https://github.com/roccoagain/gerberus
cd gerberus/tests/fixtures/sniffer
gerberus check sniffer.kicad_sch
```

Each finding says PASS, FAIL or UNKNOWN. A FAIL gives the numbers it compared and the datasheet page they came
from. The sniffer fails when its supply is connected backwards, and warns that its headers have no ESD protection.

On your own board, point it at the root schematic (`.kicad_sch`) or the Altium project (`.PrjPcb`). Put an
`inputs.toml` next to the schematic that says what voltages arrive on each connector pin; the one in
`tests/fixtures/sniffer` is a short example, and [the format is here](parts/README.md#board-inputs).

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
