---
name: check-board
description: Check a KiCad or Altium schematic with Gerberus and settle its UNKNOWN results - start inputs.toml, list what is missing with doctor, add part files, confirm guesses. Use when asked to check a board's schematic or to make gerberus results complete.
---

# Check a board with Gerberus

`gerberus check` reads a schematic, works out the voltage range on every net, and checks each pin against its
part's datasheet limits. A result it can't know is UNKNOWN, never a pass. This skill takes a board from no setup to
a check with as few UNKNOWNs as the datasheets allow. `<design>` is a KiCad root `.kicad_sch`, an Altium
`.PrjPcb`, or one `.SchDoc`. The board's own part files go in the `parts/` folder next to it, and its datasheet
PDFs in `datasheets/` beside that.

Where gerberus says to see `parts/README.md`, read `.claude/skills/datasheet-to-part/part-format.md`: it is the
same guide, installed with this skill.

## 1. Start inputs.toml

```sh
gerberus init <design>
```

If the board already has an `inputs.toml` next to the schematic, keep it and skip to step 2 (`init` won't replace
it unless `--force`). Otherwise `init` writes one: what the outside world puts on each connector pin. Entries whose
`source` starts "GUESS by gerberus init" were guessed from net names; pins it couldn't guess are comments. For
each one, find what really plugs in (the board's README, the system drawing, the person who designed it) and write
that range and where it comes from in `source`. Never leave a guess in place just because it looks reasonable: a
check that rests on it is only as good as the guess. If nobody can say, ask the person rather than invent one.

## 2. List what is missing

```sh
gerberus doctor <design>
```

It lists the things to add, the ones that settle the most UNKNOWN results first, each with the file to change.
`--json` gives the same list for a script. Work down it:

- `guess`, `inputs`: step 1.
- `part-file`: run the command its help gives (`gerberus part new <design> <reference>`), then fill the file in
  with the datasheet-to-part skill. The note names the file to save the PDF as in `datasheets/`; save it there.
- `fill-in`: a file `part new` started; fill it in with the datasheet-to-part skill.
- `pin-limits`, `drive`: add the limit or `[[drive]]` from the datasheet to the part file it names. For a
  built-in file, its help gives the `gerberus part show <MPN> --out <file>` command that copies it next to the
  schematic first; use the path it gives.

Every number comes from a datasheet page and is verified with `gerberus datasheet verify`. Never fill a gap from
memory, from a similar part, or from what would make a check pass. A value the datasheet doesn't give stays
`{ unknown = "reason" }`.

## 3. Check

```sh
gerberus check <design>
gerberus check <design> --unknown   # also lists what is still UNKNOWN
```

It exits with status 1 when a check fails with an error (warnings don't change it). Report each error and warning to the person as `check` gives it,
with the pins and the numbers. Don't change a part file or inputs.toml to make an error go away; an error is a
finding about the board unless the file it rests on is wrong against its datasheet.

Run `gerberus doctor` again after each round: what it says is left, and the UNKNOWNs it can't trace (a circuit
gerberus doesn't model, a limit no datasheet gives), are what to tell the person is still unchecked.

## 4. Waivers

When a finding looks acceptable (a header only ever used on the bench, say), don't decide that yourself. Propose a
waiver in `waivers.toml` next to the schematic, with a `proposed` line naming you and the date:

```toml
[[waiver]]
rule = "esd"
reference = "J5"
contains = "J5.2"
reason = "the SWD header is only connected on the bench, by the designer"
proposed = "Claude, 2026-11-17"
```

A proposed waiver doesn't apply. The finding still counts, and its message says that a waiver is waiting. Tell the
person what you proposed and why. Only a person approves it, by deleting the `proposed` line and adding
`by = "<name>, <date>"`. Never delete or edit a `proposed` line yourself, and never write a waiver without one.
