---
name: datasheet-to-part
description: Turn a datasheet into a Gerberus part file (parts/*.toml), or re-check an existing one, with a blind second reading and `gerberus datasheet verify`. Use when adding a part, filling in missing limits, or auditing part files.
---

# Datasheet to part file

A part file's numbers decide whether `gerberus check` passes a board, so every number must be right. Nobody signs
off on each file by hand. Instead, every value is read twice by readers who haven't seen each other's work, and a
deterministic check finds each value on the page it cites. Read `parts/README.md` first: it defines the format and
which datasheet number goes where.

## 1. Get the datasheet

Use the manufacturer's current datasheet when the host is reachable; otherwise LCSC's copy
(`https://www.lcsc.com/datasheet/<LCSC#>.pdf`; `gerberus datasheet verify` follows its link to the PDF). Note the
document number and revision in `[datasheet]`. LCSC copies can be older revisions, and TI copies there may be cut
to 10 pages, so prefer `ti.com/lit/ds/symlink/<part>.pdf` for TI. Check the PDF is for the exact part: a series
sheet must list the part number or decode it in its ordering section.

## 2. First reading

Write `parts/<MPN>.toml`. For each number:

1. Take it from the right table: absolute maximum ratings → `abs_max`; recommended operating conditions →
   `recommended` (or, where there is no such table, the operating ranges `parts/README.md` lists).
2. Take MIN or MAX, never TYP. If the table gives only TYP, the value is `{ unknown = "only a typical value is
   given" }`.
3. Convert to base units (V, A, W, °C, fractions for %).
4. Check the conditions: the row for this package and part number, the temperature range, a footnote that
   narrows it. Put them in `note`.
5. Cite the 1-based page of the PDF file (not the printed page number).
6. If the page doesn't print the number but it follows from what is printed (an LDO output from 3.3 V ± 2 %),
   write the result and say how in `derived = "..."` inside the range.
7. Re-read the source row against what you wrote.

Anything not given is `{ unknown = "reason" }`. Never fill a gap from another part or from experience.

## 3. Verify against the pages

```sh
cargo run -- datasheet verify parts/<MPN>.toml --all
```

Fix every NOT FOUND and MISMATCH, or confirm it is the tool's mistake by reading the page. Read the evidence of
every PLAUSIBLE line: it says which check it couldn't make (no header, unit or pin name on the row). NO TEXT means
a scanned page; render it (`pdftoppm -r 110 -f N -l N -png file.pdf out`) and read it by eye.

## 4. Blind second reading

```sh
cargo run -- datasheet blank parts/<MPN>.toml > /tmp/second/<MPN>.toml
```

Give the blank file and the PDF to a separate agent (or person) with these instructions: fill in every `"?"`
from the datasheet following `parts/README.md` and steps 2 and 3 above; don't open `parts/` (other than
README.md) or the git history; add any field the datasheet supports that the blank doesn't ask for and say so.

## 5. Settle every difference

```sh
cargo run -- datasheet compare parts/<MPN>.toml /tmp/second/<MPN>.toml
```

For each difference, go back to the page and decide which reading is right, or whether both are wrong. A
difference is never settled by picking the more cautious number without reading. Fix the part file, run
`verify` again, and run `cargo test --test limits` and `cargo run -- check` on any board that uses the part.
Record anything a later reader should know (a datasheet typo, an ambiguous table) in the file's comments or
`parts/README.md`'s known gaps.
