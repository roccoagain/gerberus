# Layout checks: trace current, voltage drop, decoupling, plane splits, hot loops and line length

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
| UNKNOWN | anything between; a pin joined to its supply only through a pour, whose resistance isn't worked out; a ground return with no pour; a net whose voltage leaves no margin before any copper (the pin's own limit check reports that). |

## Decoupling distance

`traces` also measures each power pin's pad (a `PowerIn` pin on a net that isn't ground) to the nearest pad of a
decoupling capacitor on its net whose other pin is on ground, centre to centre. Within `[decoupling] distance` in
`policy.toml` (5 mm) passes; further is a warning. A capacitor bigger than `[decoupling] largest` (1 µF) is bulk
storage for slow load changes, which needn't be close, so it doesn't count; a pin with no decoupling capacitor on
its net is UNKNOWN. Plug-in modules (`A1`) decouple their own chips, and connectors feed the board, so their pins
are left out.

## Plane splits

A signal's return current flows in the plane next to its track. `traces` follows every track of each signal net (a
net with no zone of its own) a quarter millimetre at a time and notes which filled piece of copper lies beneath on
each neighbouring copper layer. Points over a hole in a piece (a via's or pad's clearance) are skipped, so a hole
doesn't break the piece.

| Result | When |
|---|---|
| PASS | every track has one piece beneath it, on some neighbouring layer. |
| FAIL (warning) | a track runs over two or more separate pieces (ground islands, or a ground and a power pour) on every neighbouring layer with copper under it, so its return current detours around the gap. |
| UNKNOWN | a track has no filled copper under it on any neighbouring layer. Refill zones in KiCad before checking: an unfilled zone has no copper in the file. |

## Hot loop

A switching regulator's input current turns on and off at every switch edge, in a loop from its input capacitor
into its supply pin, through the switches, out of its ground pin and back. For each part with a switch node (a
`switches` drive in its part file, which names the supply pin and the ground), `traces` draws that loop through the
pad centres of the supply pin, a capacitor's two pads and the ground pin, and takes the smallest over the capacitors
between those two nets. The tracks between them can only make the real loop larger. Within `[hot_loop] area` in
`policy.toml` (25 mm²) passes; larger is a warning; no capacitor across the two nets is UNKNOWN.

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

Not yet checked: vias' current and a pour's narrowest neck.
