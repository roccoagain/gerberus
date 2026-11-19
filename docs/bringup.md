# Bring-up plan

```sh
gerberus bringup path/to/board.kicad_sch
```

`bringup` writes the checklist for powering a new board the first time. Everything in it comes from what
`voltages` and `check` already work out, so it needs the board's `inputs.toml`. It adds no new data.

The plan has these steps:

1. **Check for shorts.** With nothing connected, measure the resistance from each supply input and rail to
   ground.
2. **Power each supply input on its own.** An input counts as a supply when its range stays above 0 V (or below it, for a negative supply); the plan lists the inputs it leaves out, and why. A DC signal input that never reaches 0 V would count too, so check the list. Each one
   is a step:
   - Set the bench supply inside the input's range.
   - Set the current limit just above what the board's parts draw from it, when every part gives its supply
     current.
   - If some parts give no supply current, they are named. Start low and raise the limit while watching.
3. **Measure each rail fed from that input**, in the order power flows (a regulator's output after its
   supply). Each step gives:
   - the range to expect;
   - where to probe: a test point on the net, else a connector pin, else a pin of a part on it.

   A rail whose regulator may not hold its output (its supply can fall below what it needs) gets a note.

A rail is a net that something holds (an input, a regulator, or a diode or ferrite bead from one), and that
either powers a part or is a regulator's output. A rail the voltage engine can't reach, for example behind a
part with no part file, isn't listed. `gerberus doctor` says what would settle it.
