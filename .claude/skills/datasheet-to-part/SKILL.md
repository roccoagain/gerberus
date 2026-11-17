---
name: datasheet-to-part
description: Turn a datasheet into a Gerberus part file (parts/*.toml), or re-check an existing one, with a blind second reading and `gerberus datasheet verify`. Use when adding a part, filling in missing limits, or auditing part files.
---

# Datasheet to part file

A part file's numbers decide whether `gerberus check` passes a board, so every number must be right. Nobody signs
off on each file by hand. Instead, every value is read twice by readers who haven't seen each other's work, and a
deterministic check finds each value on the page it cites. Read the part file format first: `part-format.md` beside
this file, `.claude/skills/datasheet-to-part/part-format.md` (`parts/README.md` in the Gerberus repository). It defines the format and which datasheet number goes
where; this skill calls it the format guide.

The commands below are `gerberus ...`; in the Gerberus repository itself, `cargo run -- ...` runs the same.

## 1. Get the datasheet

A board's part files go in the `parts/` folder next to its schematic; `parts/<MPN>.toml` below means that folder.
If the board has a `datasheets/` folder beside `parts/`, look there first: a PDF named after the part number (as
`gerberus doctor` names it: characters other than letters, digits and `-_.+` become `_`) or after the part file is
the copy someone already saved, and `gerberus datasheet verify` reads it before downloading anything. Save a PDF
you download there under the name `doctor` gives, so the next reader and `verify` use the same copy.

Use the manufacturer's current datasheet when the host is reachable; otherwise LCSC's copy
(`https://www.lcsc.com/datasheet/<LCSC#>.pdf`; `gerberus datasheet verify` follows its link to the PDF). Note the
document number and revision in `[datasheet]`. LCSC copies can be older revisions, and TI copies there may be cut
to 10 pages, so prefer `ti.com/lit/ds/symlink/<part>.pdf` for TI. Check the PDF is for the exact part: a series
sheet must list the part number or decode it in its ordering section.

## 2. First reading

Start `parts/<MPN>.toml` with `gerberus part new <schematic> <reference>` when the part is on a board: it takes the
pin map, class and datasheet link from the symbol and marks every value "not read from the datasheet yet". Check
the pin map against the datasheet's pin table, then replace each unread value. For each number:

1. Take it from the right table: absolute maximum ratings → `abs_max`; recommended operating conditions →
   `recommended` (or, where there is no such table, the operating ranges the format guide lists).
2. Take MIN or MAX, never TYP. If the table gives only TYP, the value is `{ unknown = "only a typical value is
   given" }`.
3. Convert to base units (V, A, W, °C, fractions for %).
4. Check the conditions: the row for this package and part number, the temperature range, a footnote that
   narrows it. Put them in `note`.
5. Cite the 1-based page of the PDF file (not the printed page number).
6. If the page doesn't print the number but it follows from what is printed (an LDO output from 3.3 V ± 2 %),
   write the result and the arithmetic in `derived = "<min> .. <max>; explanation"` inside the range, such as
   `derived = "3.3 * 0.98 .. 3.3 * 1.02; VOUT(T) x 0.98 to x 1.02"` (see the format guide). `verify` evaluates it.
7. Re-read the source row against what you wrote.

Anything not given is `{ unknown = "reason" }`. Never fill a gap from another part or from experience.

## 3. Verify against the pages

```sh
gerberus datasheet verify parts/<MPN>.toml --all
```

Fix every NOT FOUND and MISMATCH, or confirm it is the tool's mistake by reading the page. Read the evidence of
every PLAUSIBLE line: it says which check it couldn't make (no header, unit or pin name on the row). NO TEXT means
a scanned page; render it (`pdftoppm -r 110 -f N -l N -png file.pdf out`) and read it by eye.

## 4. Blind second reading

Write the blank into a scratch folder outside the board (make the folder first):

```sh
gerberus datasheet blank parts/<MPN>.toml > <scratch>/<MPN>.toml
```

Give the blank file and the PDF to a separate agent (or person) with these instructions: fill in every `"?"`
from the datasheet following the format guide (give its full path) and steps 2 and 3 above, verifying with
`gerberus datasheet verify <scratch>/<MPN>.toml --pdf <the PDF>`, since the board's `datasheets/` folder isn't
next to the scratch copy; don't open any part file
other than the blank, or the git history; add any field the datasheet supports that the blank doesn't ask for and say so.

## 5. Settle every difference

```sh
gerberus datasheet compare parts/<MPN>.toml <scratch>/<MPN>.toml
```

For each difference, go back to the page and decide which reading is right, or whether both are wrong. A
difference is never settled by picking the more cautious number without reading. Fix the part file, run
`verify` again, and run `gerberus check` on any board that uses the part (in the Gerberus repository, also
`cargo test --test limits`). Record anything a later reader should know (a datasheet typo, an ambiguous table) in
the file's comments.
