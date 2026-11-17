# Revision diff

```sh
gerberus diff old/board.kicad_sch new/board.kicad_sch
gerberus diff board.kicad_sch --rev v1.0      # against a git revision of the same design
```

`diff` compares two revisions of a design and lists three things:

- **Parts** added (`+`), removed (`-`) or changed (`~`): value, symbol, footprint, MPN, DNP and pin count.
- **Connections** made or broken. Nets are matched by the pins on them, not by name, because KiCad names an
  unlabelled net after a pin (`Net-(R1-Pad1)`) and those names change when parts are renumbered. A net is
  *split* when its pins are now on more than one net, and *joins* pins that were apart when the other way round.
  A pin moved from one net to another shows as both. Renaming a net is not a change.
- **Findings** from `check` whose verdict changed: new FAILs and UNKNOWNs first, then what was fixed. Each
  revision is checked with its own `inputs.toml`, `waivers.toml` and `parts/` folder. A finding is matched by its
  rule, part, fault case and the pin its message opens with (`U1.VIN`), so a number that moved inside the same
  check is a change of that finding, not a new one. Passes that come or go with a part aren't listed.

With `--rev`, the git repository holding the design is extracted at that revision (`git archive`) into a
temporary folder, which is removed afterwards. Any revision git understands works: a tag, a branch, `HEAD~3`.

`--json` prints the same as JSON. It exits with status 1 when the new revision has an error-severity FAIL the
old one didn't, so CI can fail a pull request that adds one.
