# Waivers

Some findings are fine on purpose: a header only ever plugged in on the bench, a part run past a limit the
designers have checked by other means. A `waivers.toml` next to the schematic accepts such a finding, with the
reason, so `check` stops counting it while the reason stays on record.

```toml
[[waiver]]
rule = "esd"                 # the rule in brackets in the report: error[esd]
reference = "U6"             # the part the finding is on: the reference before the dot in `--> U6.PA4`
contains = "U6.PA4"          # optional: text the finding's message contains, such as a pin or a net name
case = "VIN shorted onto CANH"   # optional: only that fault case's findings
reason = "PA4 only sees the sense divider; the 26 V case can't reach it on the assembled board"
by = "Rocco, 2026-11-17"     # optional: who accepted it, and when
```

A waiver matches every FAIL and UNKNOWN finding of its rule on its part (narrowed by `contains` and `case`).
Those findings:

- leave the error, warning and unknown counts, so they no longer fail `check`;
- are listed with their reason by `check --all` (as `waived[rule]`), and under `waived` in `check --json`;
- aren't listed by `doctor` as things to add.

A waiver can't hide something by accident:

- `reason` is required, and a waiver for a part the design doesn't have stops `check` with an error.
- A waiver that matches no failed or unknown finding is a warning (`warning[waiver]`): the finding it was written
  for has gone or changed, so remove the waiver or fix it to match what the report says now.
- A PASS is never waived, so a waiver can't turn a check off.

`--waivers FILE` reads another file instead, for `check` and `doctor`.
