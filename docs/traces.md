# Layout checks: trace current, voltage drop, decoupling, plane splits, hot loops, line length and assembly

```sh
gerberus traces board.kicad_sch
```

`traces` checks that every track on the KiCad board (the `.kicad_pcb` with the schematic's name, or `--pcb`) is
wide enough for the current its net carries. It is the first of the v0.5 layout checks.

- **The current** is what `check` works out ([how](thermal.md)) the net supplies: everything its parts draw, at worst
  case. A net with a part whose current isn't known has only a lower bound, so its results are UNKNOWN.
- **A track's safe current** comes from IPC-2221's fit, `I = k · ΔT^0.44 · A^0.725`, with A the cross-section
  in mil² and k = 0.048 on an outer layer and 0.024 on an inner one. ΔT is `[traces] rise` in `policy.toml`
  (10 °C). The copper thickness is the board's stackup, so set it in KiCad's board setup; with none, a PASS
  has to hold at ½ oz and a FAIL at 2 oz, and anything between is UNKNOWN. IPC-2221's curves are
  more conservative than IPC-2152's.

| Result | When |
|---|---|
| PASS | the net's narrowest track takes the net's whole current. No track carries more than that. |
| FAIL (warning) | a track that can't take it is sure to carry all of it (track ends anywhere in a pad, and tracks that overlap or meet mid-way, count as joined): the net joins exactly two pads, has no copper pour, and the track is on every path between them. |
| UNKNOWN | a narrower track that may carry only part of it (it feeds one branch, or a pour, filled or not, or a parallel track shares it), a net whose current isn't all known, or a layer missing from the stackup. |

## Voltage drop

`traces` also checks that every power pin (a `PowerIn` pin whose part file gives a recommended minimum) still gets
that minimum after the copper between it and the pads that bring its net the current: a regulator's power output
pins, the input's connector pin, or a series part (a switcher's inductor, a bead, a resistor) or switch. A net fed
from more than one part is UNKNOWN.

The net's tracks and vias make a resistor network. Each track is split wherever another track (crossing it or
touching it), a via or a pad meets it. Each via joins the layers it reaches through its barrel, layer to layer by
their depth in the stackup, plated 20 µm (IPC-6012 Class 2's minimum average). Copper is 1.72 µΩ·cm at 20 °C,
rising 0.393 % per °C. Solving the network gives `Z`, the resistance from the supplying pads to the pin's pads. In a
network fed from one place, any current makes at most `Z` times itself of drop at the pin, and the pin's own
current makes exactly that.

| Result | When |
|---|---|
| PASS | the net's whole current times `Z` leaves the pin its minimum, with the thinnest copper (½ oz when there is no stackup) at the hottest board (inputs.toml's temperature, else the pin's part's rated maximum, plus `[traces] rise`), and the pin's ground has a pour, whose drop is far smaller than a track's. |
| FAIL (warning) | the pin's own maximum current (a supply entry for that pin alone) times `Z`, with the thickest copper, vias counted as no resistance, at the coldest board, is already too much, and the net has no copper pour. |
| UNKNOWN | anything between; a pin joined to its supply only through a pour, whose resistance isn't worked out; a ground return with no pour. A net whose voltage leaves no margin before any copper gets no voltage-drop finding: the pin's own limit check reports that. |

## Decoupling distance

`traces` also measures each power pin's pad (a `PowerIn` pin on a net that isn't ground) to the nearest pad of a
decoupling capacitor on its net whose other pin is on ground, centre to centre. Within `[decoupling] distance` in
`policy.toml` (5 mm) passes; further is a warning. A capacitor bigger than `[decoupling] largest` (1 µF) may be
bulk storage for slow load changes, which needn't be close, so it doesn't pass a pin. When one of those, or a
capacitor whose value can't be read, sits within the distance and no decoupling capacitor does, the pin is UNKNOWN:
a large ceramic decouples as well as a small one, an electrolytic doesn't, and the value doesn't say which. A pin
with no decoupling capacitor on its net is UNKNOWN. Plug-in modules (`A1`) decouple their own chips, and connectors
feed the board, so their pins are left out.

## Plane splits

A signal's return current flows in the plane next to its track. `traces` follows every track of each net that
carries edges a quarter millimetre at a time and notes which filled piece of copper lies beneath on each
neighbouring copper layer. A net carries edges when an output pin drives it (output, bidirectional, tri-state or
open collector) and no power pin is on it, or when it's an SPI or I²S line by name; supply rails and straps tied by
resistors are left out, and so are nets with a zone of their own. Pieces that overlap, such as two fills of one
net, are one piece. A point inside a zone's outline with no fill under it is over a hole (a via's or pad's
clearance) and doesn't break the plane; a point outside every outline has no plane under it.

| Result | When |
|---|---|
| PASS | every track has one piece beneath it all along, on some neighbouring layer. |
| FAIL (warning) | a track has no such layer and runs over two or more separate pieces (ground islands, or a ground and a power pour) on one, so its return current detours around the gap. |
| UNKNOWN | a track has no such layer and part of it has no plane under it. Refill zones in KiCad before checking: an unfilled zone has no copper in the file. |

## Hot loop

A switching regulator's input current turns on and off at every switch edge, in a loop from its input capacitor
into its supply pin, through the switches, out of its ground pin and back. For each part with a switch node (a
`switches` drive in its part file, which names the supply pin and the ground), `traces` walks that loop through the
pad centres of a supply pin, a capacitor's two pads and a ground pin, and back to the supply pin. Each supply pad
takes its shortest loop over the capacitors between the two nets, and the part's result is its longest, so a part
with two supply pins needs a capacitor at each. Within `[hot_loop] length` in `policy.toml` (15 mm) passes; longer
is a warning; a supply pad with no capacitor across the two nets is UNKNOWN. The length is measured rather than the
area, which cancels out when the path crosses itself and is near zero for a far capacitor in line with the pins.

## SPI and I²S line length

A track behaves as a lumped wire while a signal's edge is longer than about six times the track's delay; beyond
that, a line driven straight from a push-pull output reflects off its far end and rings. `traces` finds SPI and I²S
lines by their net names, as `si` finds USB pairs (a word such as SCK, SCLK, MOSI, MISO, COPI, CIPO, SDI, SDO, BCLK,
BCK, LRCLK, LRCK, WS, MCLK, SDIN or SDOUT, or one starting SAI or I2S), and adds up each one's tracks. An edge is
taken to be as fast as `[edges] rise` in `policy.toml` (1 ns), as part files don't give output rise times, travelling
at `c / √εr` through the stackup's highest-εr dielectric.

| Result | When |
|---|---|
| PASS | all of the net's tracks together are no longer than a sixth of the edge's length, so no path is. |
| UNKNOWN | longer, so it may ring: whether it does depends on the driver's strength and the far end's load. A series resistor beside the driver damps it. |

## Assembly: thermal reliefs, tombstoning and fiducials

`traces` also checks three things that make a board solder or place badly. How a pour joins a pad is read from
KiCad's last fill, so zone settings, pad and footprint overrides, DRC rules, keepouts and higher-priority zones all
count as KiCad applied them. The fill of each zone of the pad's net, on each of the pad's layers, is sampled 0.05 mm
outside the pad: copper at any of four points where a relief always leaves a gap (a rectangular pad's corners, or a
round pad at 0°, 90°, 180° and 270°, as KiCad's spokes leave a rectangle's sides and a circle at 45°) is a solid
joint; otherwise copper anywhere round the pad is a thermal relief; none is no joint. Refill the zones before checking.

| Rule | Warns when |
|---|---|
| `thermal-relief` | a through-hole pad joins a pour solidly: the plane draws the soldering heat away, so the solder may not fill the hole. A pad on a relief passes. |
| `tombstoning` | a two-pad part of 0603 or smaller (by the size in its footprint name, such as `C_0402_1005Metric`) has one pad joined solidly to a pour and the other not: that pad heats more slowly in reflow, so the other end's solder melts first and can stand the part up. Both alike passes. |
| `fiducials` | a side of the board carries a part of six or more pads whose closest two are `[assembly] fine_pitch` (0.5 mm) or less apart, centre to centre, and has fewer than `[assembly] fiducials` (3) fiducials: footprints from KiCad's `Fiducial` library, or references starting `FID`. |

Not yet checked: vias' current and a pour's narrowest neck.
