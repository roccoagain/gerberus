---
name: check-board
description: Check a KiCad or Altium schematic with Gerberus and settle its UNKNOWN results - start inputs.toml, list what is missing with doctor, add part files, confirm guesses. Use when asked to check a board's schematic or to make gerberus results complete.
---

# Check a board with Gerberus

`gerberus check` reads a schematic, works out the voltage range on every net, and checks each pin against its
part's datasheet limits. A result it can't know is UNKNOWN, never a pass. This skill takes a board from no setup to
a check with as few UNKNOWNs as the datasheets allow. `<design>` is a KiCad root `.kicad_sch`, an Altium
`.PrjPcb`, or one `.SchDoc`.

## 1. Start inputs.toml

```sh
gerberus init <design>
```

It writes `inputs.toml` next to the schematic: what the outside world puts on each connector pin. Entries whose
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
- `pin-limits`, `drive`: add the limit or `[[drive]]` from the datasheet to the part file it names (copy a
  built-in file first with `gerberus part show <MPN> > parts/<MPN>.toml`).

Every number comes from a datasheet page and is verified with `gerberus datasheet verify`. Never fill a gap from
memory, from a similar part, or from what would make a check pass. A value the datasheet doesn't give stays
`{ unknown = "reason" }`.

## 3. Check

```sh
gerberus check <design>
gerberus check <design> --unknown   # also lists what is still UNKNOWN
```

It exits with status 1 when any check fails. Report each error and warning to the person as `check` gives it,
with the pins and the numbers. Don't change a part file or inputs.toml to make an error go away; an error is a
finding about the board unless the file it rests on is wrong against its datasheet.

Run `gerberus doctor` again after each round: what it says is left, and the UNKNOWNs it can't trace (a circuit
gerberus doesn't model, a limit no datasheet gives), are what to tell the person is still unchecked.
