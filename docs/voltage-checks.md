# Voltage checks

`gerberus voltages` and `gerberus check`.

```sh
gerberus voltages path/to/board.kicad_sch --explain +3V3
gerberus check path/to/board.kicad_sch
```

`voltages` works out the worst-case voltage range of every net from the board's `inputs.toml`, through diodes,
regulators and feedback dividers (see [`parts/README.md`](../parts/README.md#net-voltages)). `--explain` prints why.

`check` compares those ranges with the part limits. Each check comes out PASS, FAIL or UNKNOWN, and a FAIL
cites the numbers, their datasheet pages and how each net got its range. The report reads like rustc's:

```text
error[abs-max]: U6.PA4 reaches 26 V, past the maximum of 4 V
  --> U6.PA4 (/mcu/VSENSE)
   = help: add a divider, clamp or series resistor, or choose a part rated higher

error: 18 errors, 9 warnings, 45 unknown, 429 passed
   = note: 45 unknown and 429 passed not shown; --all lists them
   = note: --verbose adds each finding's numbers and sources, and how each net got its range
   = help: for more about a rule, try `gerberus explain abs-max`
```

Errors come first, then warnings. UNKNOWNs and passes are only counted unless `--all` asks for them. Each finding
is its headline, where it is and a fix; `--verbose` (`-v`) adds its worst case, datasheet citations and how each
net got its range. `gerberus explain RULE` says what a rule checks and why, as `rustc --explain` does; the same
text is in [rules](rules.md). Colour is on when
the output is a terminal and `NO_COLOR` isn't set. `--allow RULE` silences a rule's warnings, as a compiler's
`-A` does (`--allow esd`; `--allow warnings` silences all of them), in the report, `--json` and `--html`, and the
summary says how many it silenced, or that it matched none. Errors can't be silenced: a [waiver](waivers.md)
accepts one, with its reason. `stress`, `signal`, `si`, `traces`, `bom` and `pinout` print the same way and take
the same flags. The rules:

| Rule | FAILs when | Severity |
| --- | --- | --- |
| `abs-max` | a pin can go past its absolute maximum ratings | error |
| `recommended` | a pin can leave its recommended operating range (for passives and connectors: their rating) | warning (error for ratings) |
| `tvs-breakdown` | a rail reaches a TVS or ESD diode's lowest breakdown voltage, so it conducts continuously | error |
| `tvs-standoff` | a rail rises above a TVS diode's stand-off voltage | warning |
| `cap-voltage` | a capacitor sees more than its rated voltage (error), or more than `policy.toml` allows (warning) | error / warning |
| `resistor-power` | a resistor's worst-case V²/R is over its rating (error) or the policy's share of it (warning) | error / warning |
| `floating-input` | an input pin is on a net nothing drives (UNKNOWN when only open-drain or tri-state outputs, connectors or unmodelled parts reach it, or when it is AC-coupled through capacitors into a part with no part file, which may bias it itself) | error |
| `unpowered` | a power pin is on a net nothing supplies (UNKNOWN when two or more pins of one part with no part file are tied on it with only passives: it may supply the net itself, like a charge pump's output) | error |
| `regulator-headroom` | a regulator's supply can fall below what its datasheet output range needs | warning |
| `esd` | an external connector pin's net has no TVS or ESD clamp, and a part on it (directly, or through one series resistor, inductor or diode with no clamp behind it) has no system-level ESD rating to `policy.toml`'s level | warning |
| `reverse-polarity` | a supply input connected backwards (its fault case `<name> reversed`) takes a part past its absolute maximum ratings | error |
| `junction-temperature` | a part's junction, at the board's ambient plus its dissipation × θJA, passes its maximum (error) or comes within `policy.toml`'s `[junction]` margin of it (warning) | error / warning |
| `forward-current` | a diode or LED carries more than its forward current rating (error) or `policy.toml`'s share of it (warning) | error / warning |
| `unchecked` | (UNKNOWN only) a fitted part or pin has no limits to check against | |

A bound relative to another pin (`VDD + 0.3 V`) passes only if it holds however the two nets move, or if
everything that drives the pin is itself bounded by that pin's net (pull-ups to it, outputs of parts it alone
supplies, diodes and regulators fed from it; not switch nodes that can overshoot it). `bidirectional` makes only
a TVS's breakdown a magnitude; abs max, recommended and stand-off ranges stay signed. A regulator that isn't held leaves its
rail's low end as a floor, so a check can pass on it but never fail on it. `--json` prints the findings (the
same fields whatever the terminal flags), and the command exits with status 1 when any error FAILs.

The voltage across two pins is narrowed by a `[[differential]]` in `inputs.toml` (a CAN bus's CANH − CANL), and
by nets of passive parts: a net reached only through resistors, LEDs and jumpers, with no capacitor or inductor
on it, sits between the nets around it, so a termination resistor behind a jumper sees at most the bus's differential voltage.

Fault cases in `inputs.toml` (such as VBUS shorted onto CANH) are each worked out on their own and checked
against `abs-max`, `tvs-breakdown` and `cap-voltage` (without derating). Resistor power and other passives'
ratings are checked too, but can only PASS or be UNKNOWN there, since a fault's two ends needn't move together.
Their findings say which fault they come from; `voltages --fault CASE` shows one case's nets.

Every supply input (one with a positive minimum) is also checked connected backwards, as the fault case
`<name> reversed`, unless its `inputs.toml` entry says why it can't be (`reverse = "keyed connector"`). A series
diode blocks it: its cathode stays at 0 V or above. A regulator fed a negative supply may pass it to its output (many CMOS
regulators have a body diode from output to input), so its output's low end becomes a floor at the supply's
minimum: pins on it come out UNKNOWN rather than PASS. `reverse-polarity` sums the case up, counting only
findings that normal operation doesn't already have; the summary isn't counted again in the error total. When it
FAILs, the case's UNKNOWNs are left out: protection at the input settles the whole case. A connector that can't
be plugged in backwards takes `reverse = "..."` in `inputs.toml` rather than a waiver, which drops the case.

The `esd` rule looks at every connector pin `inputs.toml` names (inputs and undriven pins, not their ground
references). An undriven test point is left out: only a probe touches it, at the bench. So is a connector left
out of the BOM, which is pads only (a Tag-Connect footprint, SWD pads): a probe is pressed on it at the bench,
and no cable a person plugs in reaches it. A TVS or ESD diode on the net passes it, if it breaks down below the absolute maximum of the IC
pins beside it (an SMF26A on a 3.3 V logic pin doesn't count). A pull-up or pull-down isn't followed onto its
rail. So does a clamp behind the one series part between the
connector and the ICs it protects (VBUS → D1 → TVS). A part with no part file on the way (other than a connector, test point or jumper) makes the net UNKNOWN,
since it could be an IC or a clamp. Otherwise every IC pin on the net needs an IEC 61000-4-2
rating (`[[esd]]` in its part file) at `policy.toml`'s `[esd]` level: an HBM or CDM rating only covers handling
in a factory, so it FAILs as a warning, and a part with no rating at all is UNKNOWN.

On the test boards:

- Rev 1 of the CAN encoder fails `tvs-breakdown`: its SMAJ18A (breakdown from 20 V) sits on a bus that reaches
  25.2 V. With VBUS shorted onto CANH or CANL, its TCAN330 (bus pins ±14 V) fails `abs-max`, as its own
  DESIGN.md warned. Rev 2's TCAN1044AV passes both faults.
- Both revisions fail `resistor-power` and `recommended` on R4 (its rated voltage is √(P·R) = 2.74 V), the 0402 120 Ω termination: 5 V of dominant bus differential
  (the TCAN1044A's widest VOD row, at the 2240 Ω arbitration load) is 213 mW against its 62.5 mW; even the
  3 V maximum at 50 to 65 Ω is 77 mW. The check is for a bus held dominant; normal traffic is dominant only part
  of the time.
- The sniffer's only errors are its supply connected backwards: nothing blocks VIN, so the ME6211's CE pin sees
  -5.5 V and its output sits above its reversed input. Both CAN encoders pass, as D1 blocks a reversed VBUS.
- The sniffer's UART reaches STM32 pins rated only ±1 kV HBM, so it fails `esd` as a warning. The debug pads
  (the CAN encoder's SWD pads, the sniffer's Tag-Connect) are out of the BOM, so they aren't checked. The CAN pins pass on the transceivers' own system-level ratings: rev 1's TCAN330 is rated ±12 kV contact
  (its NUP2105L breaks down only at up to 32 V, above the TCAN330's 14 V bus pins), and rev 2's TCAN1044AV ±8 kV.
