# Part limits

One TOML file per part number. Each says which voltages, currents and power a part's pins may see, and where in
the datasheet that comes from. Checkers compare these with the voltage range they work out for each net.

**Never invent a value.** If the datasheet doesn't give it, write `{ unknown = "reason" }`. A checker reports
`UNKNOWN` with that reason instead of passing or failing.

## Format

```toml
mpn = "TPS560430YFDBVR"            # matched against the schematic's MPN field (or Value, if there's no MPN)
manufacturer = "Texas Instruments"
description = "36 V 600 mA buck, SOT-23-6"

[datasheet]                         # leave out only when no datasheet can be found; then every value is unknown
title = "TPS560430 ... Step-Down Converter"
document = "SLVSE22B, revised June 2018"   # document number and revision, so pages can be checked
url = "https://www.ti.com/lit/ds/symlink/tps560430.pdf"

[pins]                              # pin number (as on the footprint) = datasheet pin name
1 = "CB"
2 = "GND"
5 = "VIN"

[[limit]]
pins = ["VIN"]                      # datasheet pin names; each gets the limit on its own
to = "GND"                          # measured from this pin
abs_max = { min = -0.3, max = 38, page = 4 }
recommended = { min = 4, max = 36, page = 4, note = "optional free text" }
```

### Ranges

A range is either `{ min = ..., max = ..., page = N, note = "..." }` or `{ unknown = "reason" }`.

- `page` is the 1-based page of the PDF at `url` (not the printed page number). It is required whenever a bound
  is known.
- Each bound is one of:
  - a number: `38`
  - another pin of the same part: `{ pin = "VIN", plus = 0.3 }`, optionally capped:
    `{ pin = "VDD", plus = 3.6, at_most = 5.5 }` means min(VDD + 3.6 V, 5.5 V)
  - `{ unknown = "reason" }`, for a datasheet that gives only one end

### Fields of a limit

| Field | Meaning |
| --- | --- |
| `quantity` | `"voltage"` (default, volts), `"current"` (amps into the part at `pins`) or `"power"` (watts between `pins` and `to`) |
| `pins` | Datasheet pin names the limit applies to |
| `to` | Reference pin. Without it, a voltage limit holds between any two of `pins` (a connector's contact rating) and a current limit holds for each pin |
| `abs_max` | Beyond this the part may be damaged |
| `recommended` | The range the datasheet says to operate in |
| `breakdown` | TVS, Zener and ESD parts: where the part starts to conduct (VBR). A rail that can sit above `breakdown.min` drives current through the part continuously |
| `bidirectional` | `breakdown` applies in both polarities |

### Which datasheet number goes where

- `abs_max` takes what the datasheet calls absolute maximum or maximum ratings.
- `recommended` takes recommended operating conditions. When a datasheet has no such table, other operating
  ranges count: a supply range in the electrical characteristics, a receiver's common-mode range, a TVS's
  stand-off voltage (VRWM), and a passive's rated voltage, power or current. Notes say which.
- Rated values of passives and connectors go in `recommended`, not `abs_max`: a capacitor's rated voltage is the
  most it may see continuously, and the withstanding-voltage test above it is a short test, not a rating.

### Board inputs

Each board has an `inputs.toml` next to its schematic (see `tests/fixtures/*/inputs.toml`). It gives the
voltages the outside world puts on its connector pins:

```toml
board = "can-encoder rev 2"

[[input]]
name = "VBUS"
pin = "J1.1"        # REF.PIN
to = "J1.4"
min = 7
max = 26
source = "hardware/can-encoder/DESIGN.md: input range 7–26 V"
```

## What's here

Datasheets are LCSC's copy for the exact LCSC part number unless the file names another URL. Values were checked
against the PDF at that URL.

| Board | Parts |
| --- | --- |
| sniffer | ME6211C33M5G-N, STM32G030F6P6TR, SCD41-D-R2, KT-0805G, SM04B-SRSS-TB, CL21A106KAYNNNE, CL10A475KO8NNNC, CC0603KRX7R9BB104, 0603WAF1002T5E, 0603WAF1001T5E |
| can-encoder rev 2 | TPS560430YFDBVR, STM32F042G6U6TR, MT6835GT-STD-R, TCAN1044AVDDFRQ1, XC6206P332MR-G, SMF26A, B5819WS, KT-0603R, SM04B-GHS-TB, OT201616MJBA4SL, SWPA252012S100MT, CC0805KKX7R9BB225, CL05B104KB54PNC, CL05B104KO5NNNC, CL21A226MAQNNNE, CL05A105KA5NQNC, 0402WGF4702TCE, 0402WGF1202TCE, 0402WGF1001TCE, 0402WGF1200TCE, 0402WGF1002TCE |
| can-encoder rev 1 | AP63200WU-7, STM32G0B1KBT6TR, MT6701CT-STD-R, TCAN330GDCNR, NUP2105LT1G, SMAJ18A, B5819W SL, KT-0805G, SM04B-GHS-TB, OT201616MJBA4SL, FNR4030S6R8MT, GRM21BR61H106KE43L, 0402CG101J500NT, CL05B104KO5NNNC, CL21A226MAQNNNE, CL05A105KA5NQNC, 0402WGF4702TCE, 0402WGF1502TCE, 0402WGF1001TCE, 0402WGF1200TCE, 0402WGF1002TCE |

Not covered: footprint-only parts with no part number (Tag-Connect pads, SWD pads, solder jumpers).

### Known gaps

- **SWPA252012S100MT** (rev 2 L1): no datasheet reachable. LCSC has none for C37428 and sunlordinc.com is blocked
  from the cloud environment, so every value is unknown.
- **MT6701CT**: the datasheet gives only the VDD rating, so its signal pins are unknown.
- **OT201616MJBA4SL**: LCSC's datasheet for it is the YXC YSO110TR series sheet, which has no absolute maximum
  ratings.
- Logic inputs on the CAN transceivers have no recommended range in their datasheets (only VIH/VIL thresholds).
