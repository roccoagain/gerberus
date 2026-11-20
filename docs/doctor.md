# What to add first: `gerberus doctor`

```
gerberus doctor <design> [--inputs FILE] [--parts DIR]... [--json]
```

`check` marks a result UNKNOWN when it can't know a voltage or a limit. `doctor` runs the same checks and traces
every UNKNOWN back to what it waits on, then lists those things to add, the ones that settle the most results
first. On the capstone audio board rev 2 it starts:

```
todo[part-file]: write a part file for PESD5V0S1BAF: 20 unknown results wait on it
  --> D1, D2, D3, D4, D5
   = note: datasheet: https://assets.nexperia.com/documents/data-sheet/PESD5V0S1BA.pdf
   = note: if the link doesn't download, or you already have the PDF, save the PDF as
           audio-board-rev2/datasheets/PESD5V0S1BAF.pdf: `datasheet verify` reads it from there, and an agent
           reads it to fill the file in
   = help: start audio-board-rev2/parts/PESD5V0S1BAF.toml with `gerberus part new audio-board-rev2/board.kicad_sch
           D1`, then fill it in from the datasheet: parts/README.md says how, and an agent can do it with the
           datasheet-to-part skill

todo[part-file]: write a part file for MAX9722AEUE+: 17 unknown results wait on it
  --> U2
   ...

todo[review]: check by hand what gerberus can't settle, and waive each result that holds: 2 unknown results wait on it
   ...

todo: 13 things to do; 42 unknown results wait on them
```

On a board with no `inputs.toml`, the file itself usually comes first (the CAN encoder test board):

```
todo[inputs]: write inputs.toml: 151 unknown results wait on it
  --> J1, J2, J3
   = note: nothing says what plugs into J1.1, J1.2, J1.3, J2.1, J2.2, J2.3, J3.2, J3.3, J3.4 or J3.5
   = note: without it, ground is the only voltage known, and reverse polarity, ESD and fault cases aren't
           checked, so more results wait on it than these
   = help: give each connector pin's voltage range as an [[input]], or an [[undriven]] entry if nothing
           outside drives it (see "Board inputs" and "Net voltages" in parts/README.md)
```

What it lists:

| Kind | What to add | Where |
| --- | --- | --- |
| `guess` | Entries `gerberus init` guessed, listed first whatever they settle: every result that rests on one is only as good as the guess. | `inputs.toml` |
| `inputs` | What plugs into each connector pin: an `[[input]]` with its voltage range, or `[[undriven]]` if nothing outside drives it. With no `inputs.toml` at all, one item for the whole file, always listed. | `inputs.toml` next to the schematic ([inputs](../parts/README.md#board-inputs), [undriven](../parts/README.md#net-voltages)) |
| `part-file` | A part file for a part number that has none, started with `gerberus part new`. The symbol's `Datasheet` link is shown when it has one, and the name to save the PDF under in `datasheets/` beside `parts/` (or where it is saved, once it is). Part numbers that share a datasheet are pointed at [part families](../parts/README.md#part-families). | `parts/<MPN>.toml` next to the schematic ([format](../parts/README.md)) |
| `fill-in` | Values a part file marks not known yet: "not read from the datasheet yet" in a file `gerberus part new` started, or any other `{ unknown = "..." }`. | that part file |
| `pin-limits` | A voltage limit for pins a part file doesn't cover. | the part file, or a copy of a built-in one made with `gerberus part show <MPN> --out <file>` |
| `drive` | A `[[drive]]` saying what a power output gives. For a module that isn't fitted (plugged in by hand), an `[[input]]` for the net it supplies. | the part file, or a copy of a built-in one ([What a part drives](../parts/README.md#what-a-part-drives)) |
| `part-data` | Data a check needs beyond the limits: a supply current, an efficiency, a thermal resistance, an ESD rating, a forward current rating. If the datasheet doesn't give it, the results waiting on it are checked by hand and waived. | the part file |
| `review` | Everything nothing above settles: circuits gerberus can't work out (a bootstrap pin, a loop through a MOSFET and a Zener diode, a resistor whose two ends move apart in a fault case), with the nets whose range is UNKNOWN. Always last. Check each by hand and add a [waiver](waivers.md) for the ones that hold. | `waivers.toml` next to the schematic |

How it counts: each UNKNOWN finding on a net is followed back through how the net got its range (what
`gerberus voltages --explain` prints) to the steps that made it unknown, and to the parts and connector pins those
steps name. A finding about a part with no part file, or naming one, waits on that part too. A bipolar
transistor or a dual MOSFET isn't modelled, so a step that only says its pin can't be modelled isn't counted
against its part file. A single MOSFET's is: a `channel` drive in its part file models it. A finding can wait on
several things at once, so adding one may not settle it: each count is how many findings wait on that item, at
most.

Every UNKNOWN is listed: under the things to add it waits on, or else under `review`. A limit the datasheet
doesn't give at all is `{ none = "..." }` in the part file and makes no result, so it never shows up here.

Without an `inputs.toml`, `check` skips reverse polarity, ESD and fault cases, so writing one settles more results
than the count shows.

`--json` prints the list for an agent: each item's `rule`, `title`, `unknown` (the count), `references`, `mpn`,
`pins`, `datasheet`, `file` (what to write or change), `notes` and `help`, then `unknown` for the whole board. `doctor` exits with status 0 whatever it finds, and 2 when it can't run.
