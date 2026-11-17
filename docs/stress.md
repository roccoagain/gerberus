# Hot-plug stress (SPICE)

```sh
gerberus stress path/to/board.kicad_sch --runs 16
```

`stress` simulates plugging the board in, in [ngspice](https://ngspice.sourceforge.io/), which must be installed
(`apt install ngspice` or `brew install ngspice`). Each supply input in `inputs.toml` (one with a positive minimum)
is switched on at its maximum, in 1 µs, through a cable: 0.1 Ω and up to 1 µH, about a metre. These numbers come
from `policy.toml`'s `[spice]` table. The cable's inductance rings with the board's input capacitors, so a rail
can overshoot its settled value, by up to about twice in the worst case.

The first run uses nominal values and the largest cable inductance. Each further run draws every value again:
- resistors over their tolerance and drift;
- capacitors and inductors over their tolerance;
- regulator outputs over their datasheet range;
- the cable inductance over its range.

`--seed` changes the draw. The peak, settled and lowest voltage across every pin limit, over all runs, is checked.
Past an absolute maximum is an error; past a passive's rating is a warning. A failure names the run and what it
drew. `--netlist FILE` writes the nominal run's netlist, so it can be opened in any SPICE tool.

What is modelled:
- resistors, capacitors and inductors from their values;
- ferrite beads, fuses, net ties, jumpers and switches as closed;
- diodes and LEDs from their forward drop at their test current;
- TVS diodes breaking down at the top of their breakdown range, with the resistance their `clamp` figure gives
  (clamping voltage at the pulse current), or 1 Ω when the part file has none;
- a transistor's forward drive (a P-FET reverse-polarity switch) as its channel switched on, 0.05 Ω;
- step-down regulators as ideal sources with 10 mΩ output resistance that drop out when their supply is too low (a
  switcher at 100 % efficiency, the least it can draw), and with no soft start, so the regulator's inrush is high;
- every IC's `[[supply]]` current.

A limit relative to another pin (`max = VDD + 0.3`) is checked against that pin's voltage in the same run. A limit
whose end the datasheet doesn't give comes out UNKNOWN, not PASS.

What isn't:
- Capacitor ESR and IC pins' internal clamps aren't modelled, both of which would damp a peak, so the result
  errs high.
- A step-up regulator's output is UNKNOWN, and so is a switching regulator's switch node.
- Signal inputs are left open.
- Pin limits on nets nothing in the simulation drives (IC signal pins) aren't reported.
- Parts with no model are listed, and the pins on their nets come out UNKNOWN. A net the static voltage analysis
  says is powered but the simulation leaves near 0 V is UNKNOWN too, as something feeding it isn't modelled.
- A TVS that a supply holds past its breakdown minimum once settled fails: it keeps conducting.

On the test boards:
- The sniffer fails. Its 10 µF input capacitor and the cable take VIN from 5.5 V to about 9.9 V, past the ME6211's
  6.5 V absolute maximum. A TVS, a bulk capacitor with some ESR, or a series resistor would fix it.
- Rev 2 of the CAN encoder fails. Its series Schottky D1 holds the input capacitors at the top of the ring, so +VDC
  reaches about 53 V from a 26 V bus: past the TPS560430's 38 V, the 50 V capacitors and D1's 40 V reverse rating.
  The SMF26A (31.9 V top of breakdown) can't hold it, at 2.1 Ω from its clamp figure. A bulk electrolytic
  capacitor across the input, whose ESR damps the ring, is the usual fix.
- Rev 1 fails on its SMAJ18A, which the 25.2 V bus holds past breakdown. Its +12V rail peaks at about 34.8 V,
  just inside the AP63200's 35 V.
