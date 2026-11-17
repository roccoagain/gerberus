# Signal levels (SPICE)

```sh
gerberus signal path/to/board.kicad_sch
```

`signal` sends the board's audio or analog signals through it in [ngspice](https://ngspice.sourceforge.io/), which
must be installed. It checks two things: whether an op-amp clips on the loudest signal, and whether any IC pin
the signal reaches goes past its limits.

Each signal is a `[[signal]]` in `inputs.toml` (see [Signals](../parts/README.md#signals)): its pins, its largest
peak and its band. For example, a guitar's 1.5 V peak at J1's tip.

How it works:

- Every rail is held at a steady voltage. A rail is a net the voltage engine knows both ends of and that an input,
  regulator or diode holds. The run is done twice, once with every rail at its lowest and once at its highest.
- ngspice finds each net's DC bias, then its gain from each signal across the signal's band.
- A net swings its bias plus and minus the sum of each signal's peak times its largest gain. That treats all
  signals as in phase, which is the worst case.

The output lists each signal's gain to every net it reaches, and that net's range in both runs.

## Rules

| Rule | Fails when | Severity |
|---|---|---|
| `clipping` | an op-amp's output reaches past its supply rails, or closer to one than the datasheet's output swing. It is UNKNOWN when the output stays inside the rails but the part file gives no swing. | warning |
| `signal-level` | the swing on a pin with a voltage limit goes past the limit, measured from the limit's reference pin. Past the absolute maximum it is an error; past the recommended range it is a warning. | error / warning |

`check` already covers passives, so `signal-level` leaves them out.

## What is modelled

- Resistors, capacitors and inductors, at their marked values.
- Ferrite beads and fuses as shorts.
- Op-amps from their part file's `[[opamp]]`, as ideal amplifiers: an open-loop gain of 10⁶ with no bandwidth
  limit. The model has no clipping, so the levels show where a real op-amp would clip.

## What isn't

- Diodes and TVS parts are left open. A signal on a sound design stays below their forward drop and breakdown, but
  a fault-sized signal would be clamped in reality and isn't here.
- IC pins with no `[[opamp]]` are left open and taken to draw no current. That covers codec inputs, modules, and
  amplifiers whose inputs are virtual grounds. The output lists every part left out.
- Tolerances aren't swept, and an op-amp's gain-bandwidth, slew rate, offset and bias current aren't modelled.
- Swing figures are given at one supply and load, usually 25 °C. The note on each part file says which.
