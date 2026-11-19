# USB signal integrity

```sh
gerberus si path/to/board.kicad_pcb --plots plots/
```

It finds USB 2.0 differential pairs on a `.kicad_pcb` by net name (`D+`/`D-`, `DP`/`DM`, or `_P`/`_N` with USB
in the name; SuperSpeed lanes named `SS`, `TX` or `RX` are skipped). It traces each line through its tracks, vias
and pads from the PHY (an IC) to the farthest connector on it (a USB one when there is one), and checks it for high
speed (480 Mb/s). Anything else on the line, a header or a test point, counts as a stub:

| Rule | Checks | Limit |
| --- | --- | --- |
| `usb-impedance` | Differential impedance of every stretch of the pair, from a 2D field solver run on its cross-section (trace width, gap, copper and dielectric thickness, εr from the board's stackup, the planes above and below) | 81–99 Ω (TI SPRAAR7J Table A-1) |
| `usb-skew` | D+ and D− length difference | 50 mil (1.27 mm) |
| `usb-vias` | Vias on each line, and equal counts on both | 4 per line (SPRAAR7J) |
| `usb-stubs` | Copper that branches off a line to a test point, ESD part or anything else off the through path | under 200 mil (TUSB1210 Table 10-1, item 2.13) |
| `usb-reference` | An unbroken plane on the next layer under every stretch, and distance from that plane's edge | no gaps; 25 mil from the edge (items 2.14, 2.15) |
| `usb-length` | Total length | 12 inches |
| `usb-capacitance` | Capacitance of parts on each line, from their `capacitance` entries in `parts/` | 4 pF (item 1.06) |
| `usb-eye` | Simulated eye at the connector | USB 2.0 Template 1 (Figure 7-13) |

A stretch outside the impedance band that is 3 mm or shorter (about 20 ps, too short to reflect on its own), such
as where a pair fans out to reach pads, is left to the eye simulation instead of failing the impedance rule. The field solver is a finite-volume Laplace
solver; it agrees with closed-form microstrip and stripline formulas to 0.3 %.

The eye comes from a channel simulation. Each line becomes a cascade of transmission-line sections (impedance,
delay, conductor and dielectric loss from the solver and stackup), via inductance and capacitance, open stubs and
part capacitance. A 480 Mb/s PRBS7 pattern with 500 ps edges (the fastest USB allows, Table 7-8) from a 45 Ω,
400 mV driver goes through it into a 45 Ω termination, and the differential eye at the connector is checked
against the inner hexagon and outer limits of Template 1. The model is odd-mode only and ignores crosstalk from
other nets, so a pass means the layout leaves the eye open, not that a real cable and receiver will. Parts on a
line with no capacitance in `parts/` are listed as not modelled.

`--plots DIR` writes an eye diagram and a TDR plot (the impedance the driver sees along each line) for each pair
as SVG. `--json` prints the report as JSON, `--all` lists UNKNOWNs and passes. The command exits with
status 1 when any rule fails with an error.

The test board `tests/fixtures/usb-si-testbed` (from [roccoagain/hardware](https://github.com/roccoagain/hardware))
has one port routed to TI's guidelines, which passes, and one that breaks every rule above apart from length, which
fails each of them and closes the eye.
