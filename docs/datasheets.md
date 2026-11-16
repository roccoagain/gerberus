# Datasheet checks

```sh
gerberus datasheet verify parts/TPS560430YFDBVR.toml
gerberus datasheet blank parts/TPS560430YFDBVR.toml > second.toml   # a second reader fills it in blind
gerberus datasheet compare parts/TPS560430YFDBVR.toml second.toml
```

Part files are written by reading datasheets, by a person or an LLM. Gerberus checks that reading in two
independent ways, and decides nothing itself:

- `verify` downloads the datasheet (cached in `~/.cache/gerberus/datasheets`; `--pdf` reads a local copy) and
  looks for every cited value on its page with poppler's `pdftotext`. It finds the table's MIN, TYP and MAX
  columns and its UNIT column, so a typical value filed as a limit, a value in the wrong unit or one from the
  wrong page shows up. A temperature on a storage row and a value in the other end's column are mismatches;
  a `derived` range's arithmetic is evaluated against the bound. Each value comes out CONFIRMED (right column, unit and a row naming the pin or part
  number), DERIVED (worked out from printed numbers by the range's `derived` arithmetic, or a non-polar rating's
  negative end), PLAUSIBLE (on the page, with the doubts listed), NO TEXT (a scanned page to read by eye),
  NOT FOUND or MISMATCH. It exits with status 1 on the last two.
- `blank` writes the part file with every datasheet value replaced by `"?"`, and `compare` lists every value
  where two readings disagree. Two readers who haven't seen each other's work rarely make the same mistake.

[`.claude/skills/datasheet-to-part`](../.claude/skills/datasheet-to-part/SKILL.md) is the whole procedure for an
agent: read, write, verify, a blind second reading by a separate agent, and settling every difference against
the page.
