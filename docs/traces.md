# Trace current

```sh
gerberus traces board.kicad_sch
```

`traces` checks that every track on the KiCad board (the `.kicad_pcb` with the schematic's name, or `--pcb`) is
wide enough for the current its net carries. It is the first of the v0.5 layout checks.

- **The current** is what [`loads`](thermal.md) works out the net supplies: everything its parts draw, at worst
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

Not yet checked: vias' current, a pour's narrowest neck, and the other v0.5 rules (decoupling distance, traces
crossing plane splits, loop area).
