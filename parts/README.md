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
class = "ic"                        # capacitor, resistor, inductor, diode, tvs, led, connector, oscillator, ic

[datasheet]                         # leave out only when no datasheet can be found; then every value is unknown
title = "TPS560430 ... Step-Down Converter"
document = "SLVSE22B, revised June 2018"   # document number and revision, so pages can be checked
url = "https://www.ti.com/lit/ds/symlink/tps560430.pdf"

[pins]                              # pin number (as on the footprint) = datasheet pin name; list every pin
1 = "CB"
2 = "GND"
5 = "VIN"

[[variant]]                         # optional: another part number with the same limits (another package)
mpn = "OTHER-PACKAGE-MPN"           # placeholder; gives its own pin map, or reuses [pins] without one
pins = { 1 = "CB", 2 = "GND", 3 = "FB", 4 = "EN", 5 = "VIN", 6 = "SW" }

[[limit]]
pins = ["VIN"]                      # datasheet pin names, never numbers, so every package shares the limits
to = "GND"                          # measured from this pin
abs_max = { min = -0.3, max = 38, page = 4 }
recommended = { min = 4, max = 36, page = 4, note = "optional free text" }
```

### Pin map

`[pins]` is the datasheet's pin table for this package, and `gerberus pinout` compares it with the symbol and
the footprint. List every pin, including `NC`, `DNC` and an exposed pad (by the number the footprint gives it),
and write names as the datasheet prints them. Pads that are not pins at all, like a connector's mounting tabs,
stay out. A pin named `DNC` (do not connect) must not be wired to anything; `NC` may be.

### Ranges

A range is either `{ min = ..., max = ..., page = N, note = "..." }` or `{ unknown = "reason" }`.

- `page` is the 1-based page of the PDF at `url` (not the printed page number). It is required whenever a bound
  is known.
- `derived = "<min> .. <max>; explanation"` works out a value the page doesn't print from what it does print.
  Each end is arithmetic on numbers (`+ - * /`, brackets, `sqrt(...)`), such as
  `"3.3 * 0.98 .. 3.3 * 1.02; VOUT(T) x 0.98 to x 1.02"` for an LDO's output range, or a name plus a number as
  the page prints it, such as `"-0.3 .. VDDIOx + 4.0; VDDIOx is VDD for these pins"`. Write `-0.3`-style ends
  even where the bound is printed. `gerberus datasheet verify` evaluates each end and calls a bound that doesn't
  match a MISMATCH; it looks for the arithmetic's numbers (or the printed name and offset) on the page and
  lists any it can't find. A derivation never hides a value printed in the wrong column.
- Each bound is one of:
  - a number: `38`
  - another pin of the same part: `{ pin = "VIN", plus = 0.3 }`, optionally capped:
    `{ pin = "VDD", plus = 3.6, at_most = 5.5 }` means min(VDD + 3.6 V, 5.5 V). The other pin's voltage is
    measured from the limit's `to` pin. `at_most` only goes on `max`.
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
| `bidirectional` | `breakdown` is a magnitude and applies in both polarities. `abs_max` and `recommended` stay signed |

### What a part drives

Limits say what a pin may see. `[[drive]]` entries say what the part itself sets when it works, so a checker
can work out the rails:

```toml
[[drive]]
kind = "regulates"     # a regulator holds `pin` at `range` from `to` (a fixed output, or a feedback pin)
pin = "FB"
to = "GND"
supply = "VIN"         # the pin that powers it; the drive holds only while this pin is in its recommended range
feedback = true        # the output is the far end of the two-resistor divider on this pin
headroom = { duty = 0.98, page = 1 }   # supply × duty − volts must reach the output; an LDO gives `volts`
range = { min = 0.985, max = 1.015, page = 5 }

[[drive]]
kind = "switches"      # a switch node; its range may follow the supply pin
pin = "SW"
to = "GND"
supply = "VIN"
range = { min = { unknown = "undershoot below GND isn't given" }, max = { unknown = "overshoot above VIN isn't given" }, page = 3 }

[[drive]]
kind = "forward"       # a diode conducting from `pin` (anode) to `to` (cathode) drops `range`
pin = "A"
to = "K"
range = { min = { unknown = "only a maximum is given" }, max = 0.6, page = 1, note = "at IF = 1 A" }

[[drive]]
kind = "forward"       # an LED: `current` is the IF its VF range is measured at
pin = "A"
to = "K"
current = 0.020
range = { min = 1.8, max = 2.4, page = 3, note = "VF at IF = 20 mA" }
```

An LED's VF minimum holds only at and above `current`. Below it the LED drops less, so a series resistor's
current is then bounded with VF taken as 0 V.

A resistor, capacitor or inductor can give its tolerance, temperature coefficient and rated temperature range as
top-level keys, before the first table:

```toml
tolerance = { value = 0.01, page = 2, note = "7th code F = ±1 %" }
tcr = { ppm = 100, page = 6 }
temperature = { min = -55, max = 155, page = 5 }
```

### Power and temperature

Any part can give its rated ambient range, its thermal resistance and junction limit, the most current it draws,
and (a switching regulator) a guaranteed efficiency, as top-level keys and `[[supply]]` tables:

```toml
temperature = { min = -40, max = 85, page = 47, note = "TA, suffix 6" }   # operating ambient
theta_ja = { value = 118, page = 111, note = "UFQFPN28" }                # θJA in °C/W, for this package
tj_max = { value = 105, page = 47, note = "TJ, suffix 6" }               # recommended TJ max, else absolute
efficiency = { value = 0.85, page = 7, note = "..." }                   # a guaranteed minimum, 0 to 1

[[supply]]
pins = ["VDD", "VDDIO2"]                # the pins together draw at most `current.max` amps
current = { min = 0, max = 0.0246, page = 50, note = "IDD, Run mode, every peripheral on, max" }
```

`current` takes the datasheet's maximum at the worst operating point the board can reach, never a typical.
Where a datasheet gives only typical figures (as most switching regulators do for efficiency, in curves),
leave the key out: the part's dissipation and anything fed through it come out UNKNOWN. A `derived` range can
add a typical share to a maximum when that is all there is; its note says so.

A pin no voltage limit mentions (other than `NC`/`DNC`) is reported by `gerberus parts` and must be treated
as UNKNOWN by checkers.

### ESD ratings

```toml
[[esd]]
model = "hbm"          # "hbm", "cdm", "iec-contact" or "iec-air"
volts = 4000           # the magnitude: ±4 kV is 4000
pins = ["CANH", "CANL"]  # left out: every pin
page = 4
note = "..."
```

`iec-contact` and `iec-air` are system-level ratings: IEC 61000-4-2, or ISO 10605 as automotive parts print it
(say which in `note`). Only these meet `policy.toml`'s `[esd]` level. A datasheet with no ESD rating gets no
`[[esd]]` entry, and its pins on an external connector come out UNKNOWN.

### Which datasheet number goes where

- `abs_max` takes what the datasheet calls absolute maximum or maximum ratings.
- `recommended` takes recommended operating conditions. When a datasheet has no such table, other operating
  ranges count: a supply range in the electrical characteristics, a receiver's common-mode range, a TVS's
  stand-off voltage (VRWM), and a passive's rated voltage, power or current. Notes say which.
- When a bound is relative to another pin, `recommended` can come out above `abs_max` (an STM32's 5 V-tolerant
  pin at low VDD). Checkers use the tighter of the two at the voltages they work out.
- Rated values of passives and connectors go in `recommended`, not `abs_max`: a capacitor's rated voltage is the
  most it may see continuously, and the withstanding-voltage test above it is a short test, not a rating.

### Checking a part file

Every value is read twice from the datasheet, by readers who haven't seen each other's work, and checked against
its page by `gerberus datasheet verify` (see the main README and `.claude/skills/datasheet-to-part`). Nobody signs
off on a file by hand, so a value that both readings and the page don't back stays `{ unknown = "..." }`.

### Where limits come from

`gerberus parts board.kicad_sch` reports, for every placed part, where its limits come from:

1. **A part file.** The shared `parts/` folder loads first, then a `parts/` folder next to the schematic, then
   any `--parts` folders. A later file for the same part number replaces the earlier one, so a board can
   override the shared library.
2. **A class default.** A capacitor with no part file takes its rated voltage from its schematic `Voltage`
   field, or from its value (`100nF 50V`, `6V3`). It is marked as not checked against a datasheet. Other
   classes have no default.
3. **Nothing.** The part is reported as `UNCHECKED`, never as passing.

### Derating policy

Datasheet limits are facts; derating is project judgement. It lives in `policy.toml` at the repository root,
never in a part file:

```toml
[[derate]]
class = "capacitor"     # or mpn = "..." for one part, which wins over a class rule
quantity = "voltage"
factor = 0.8            # use at most 80 % of the rating
reason = "..."

[junction]
margin = 20             # keep every junction this many °C below its maximum
reason = "..."
```

`quantity` is `voltage` (capacitors), `power` (resistors) or `current` (diode and LED forward current). These
shares and the junction margin are the reliability derating: a FAIL against them is a warning, while a FAIL
against the rating itself is an error.

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

## Net voltages

`gerberus voltages board.kicad_sch` prints the worst-case range of every net; `--explain NET` prints why,
down to the inputs it comes from. It starts from `inputs.toml` (the `to` pins are 0 V) and follows:

- **Diodes** (`forward`): the cathode is the anode minus the drop. A drop with no minimum is taken as 0 V.
- **Regulators** (`regulates`): a `feedback` pin sets the output net through its two-resistor divider to
  ground. Each resistor counts its `tolerance` plus `tcr` drift over the board's temperature range, or its own
  rated range when `inputs.toml` gives none. Anything else on the feedback net makes the output UNKNOWN.
  Without `feedback`, the drive sets the pin's own net. The range holds only while the supply stays inside the
  supply pin's recommended range and meets the `headroom`. Below that, or with no `headroom`, the low end is
  0 V, marked as only a floor: checks can pass on it but not fail on it, and nets fed from it inherit the mark.
  Above the recommended maximum, the top end follows the supply. Line and load regulation aren't modelled.
- **Switch nodes** (`switches`): whatever their range gives; both ends are UNKNOWN on today's parts.
- **Everything else**: the union of what can pull the net: nets reached through resistors, inductors, LEDs and
  jumpers (capacitors count as open, TVS diodes as off), and IC output pins, which reach at most their part's
  highest supply. A pin of any other part that isn't an input makes the net UNKNOWN.

A net with a connector pin that `inputs.toml` doesn't describe is UNKNOWN, as is anything that needs a number
nobody gave. Each unknown end says why. An input sets its whole net, so other connector pins on that net (a
daisy-chain connector) share its range. A pin the outside world doesn't drive, such as a debug probe's
target-voltage sense pin, is listed as `[[undriven]]` with its source:

```toml
[[undriven]]
pin = "J3.5"
source = "hardware/can-encoder/DESIGN.md, J3 row: 5 3V3. Assumes the probe only senses it"
```

An input's `max` can follow another pin instead of being a number. A debug probe drives SWDIO, SWCLK and NRST
at up to the target voltage it senses:

```toml
[[input]]
name = "SWDIO (debug probe)"
pin = "J3.2"
to = "J3.1"
min = 0
max = { pin = "J3.5" }   # the net J3.5 is on; `plus = 0.3` would add 0.3 V
source = "..."
```

Two inputs that move together, such as a CAN bus's two lines, can limit the voltage between them. Checks use it
wherever they compare the two nets, such as a termination resistor across them:

```toml
[[differential]]
pin = "J1.2"   # V(J1.2) − V(J1.3) stays in min..max
to = "J1.3"
min = -0.12
max = 3
source = "..."
```

A **fault case** is a set of inputs with the same `fault` name. They are left out of normal operation. Each case
is worked out on its own: its inputs replace the normal inputs on the nets they touch (and any `[[differential]]`
on those nets), and `gerberus check` runs the damage rules on it: abs max, TVS breakdown, and capacitor ratings
without derating. Resistor power and other passives' ratings run too, but a FAIL there becomes UNKNOWN. Only one
fault is assumed at a time. A `[[differential]]`'s pins must both be on nets a normal input drives.

```toml
[[input]]
name = "CANH shorted to VBUS"
fault = "VBUS shorted onto CANH"
pin = "J1.2"
to = "J1.4"
min = 7
max = 26
source = "..."
```

Every supply input (a normal input with `min` above 0) is also checked connected backwards: the fault case
`<name> reversed` puts `-max` to `-min` on its pin. A connector that can't be plugged in backwards says so with
`reverse = "reason"` on the input, which skips the case.

`inputs.toml` can also give `temperature = { min = 0, max = 70, source = "..." }` for the board's ambient.

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

Found by the blind second reading of every file (November 2026):

- **Resistors' rated voltage** is the rated continuous working voltage √(P·R), capped by the size's maximum
  working voltage (UNI-ROYAL p5), not the maximum working voltage alone. The 120 Ω 0402 is rated 2.74 V.
- **CL05B104KB54PNC** isn't in the Samsung catalogue LCSC serves for it: its "4" and "P" codes aren't defined
  there. Its 50 V comes from voltage code B (p4). Worth checking against Samsung's own page for the part.
- The Samsung catalogue's text layer drops the minus signs in its temperature table (p5), so `datasheet verify`
  reports their −55 °C as NOT FOUND; the rendered page shows them.
- **MT6701CT**: its electrical parameters (p7) give VDD 3.0 to 5.5 V, but its pin table and front page say 3.3 to
  5.0 V. The file uses the table.
- **KT-0603R**: Pd 40 mW is lower than 25 mA × its 2.4 V maximum VF, so power may limit it before IF does.
- **B5819W**: IFRM 1.5 A applies to repetitive peaks only, so the DC current limit stays at IO, 1 A.

- **SWPA252012S100MT** (rev 2 L1): no datasheet reachable. LCSC has none for C37428 and sunlordinc.com is blocked
  from the cloud environment, so every value is unknown.
- **MT6701CT**: the datasheet gives only the VDD rating, so its signal pins are unknown.
- **OT201616MJBA4SL**: LCSC's datasheet for it is the YXC YSO110TR series sheet, which has no absolute maximum
  ratings.
- The CAN bus inputs assume every node's driver (the C610s' too) stays inside the ISO 11898-2 limits the TI
  transceiver datasheets give, which wasn't checked against the C610's own documents. No
  particular debug probe is used, so the probe inputs cover one that follows the target voltage and one fixed at
  3.3 V, assuming none drives above 3.3 V. Two more assumptions aren't checked: the probe only senses the
  board's VCC pin and doesn't power the board through it, and the board is powered while a probe is connected.
  A fixed 3.3 V probe on an unpowered board would drive its pins far above VDD + 0.3 V.
- Resistor power can't FAIL in fault cases: a fault drives the two ends of a resistor from independent ranges,
  which can overstate the voltage across it, so a FAIL there is UNKNOWN.
- Each CAN wire's range is the ISO common-mode range ± half the differential, which is wider than any one wire
  is likely to reach.
- **AP63200WU-7**: no headroom figure (the datasheet only says the duty cycle approaches 100 % near dropout), so
  rev 1's +3V3 has a 0 V low end. Its VFB range is for CCM; the AP63200 runs PFM at light load.
- The LDO output ranges (ME6211, XC6206) are specified at 25 °C.
- Logic inputs on the CAN transceivers have no recommended range in their datasheets (only VIH/VIL thresholds).
