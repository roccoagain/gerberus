# Trace current and voltage drop

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
that minimum after the copper between it and the part that supplies its net: the regulator, the input's connector,
or the series part, switch or IC it is fed through. A net fed from more than one part is UNKNOWN.

The net's tracks and vias make a resistor network. Each track is split wherever another track or a via touches it;
each via adds its barrel (the board's thickness, plated 20 µm, IPC-6012 Class 2's minimum average). Copper is
1.72 µΩ·cm at 20 °C, rising 0.393 % per °C. Solving the network gives `Z`, the resistance from the supplying part's
pads to the pin's pad. In a network fed from one place, any current makes at most `Z` times itself of drop at the
pin, and the pin's own current makes exactly that.

| Result | When |
|---|---|
| PASS | the net's whole current times `Z` leaves the pin its minimum, with the thinnest copper (½ oz when there is no stackup) at the hottest board (inputs.toml's temperature, else the pin's part's rated maximum, plus `[traces] rise`). |
| FAIL (warning) | the pin's own maximum current (a supply entry for that pin alone) times `Z`, with the thickest copper at the coldest board, is already too much, and the net has no copper pour. |
| UNKNOWN | anything between; a pin joined to its supply only through a pour, whose resistance isn't worked out; a net whose voltage leaves no margin before any copper (the pin's own limit check reports that). |

The ground return isn't counted: it is taken to be a plane, whose drop is far smaller than a track's.

Not yet checked: vias' current, a pour's narrowest neck, and the other v0.5 rules (decoupling distance, traces
crossing plane splits, loop area).
