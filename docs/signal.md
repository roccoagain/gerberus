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
  regulator or diode holds. Rails at exactly 0 V are the simulation's ground; a net that is only pulled to 0 V
  through a resistor is not.
- The circuit is linear, so ngspice finds each net's DC bias with every rail at its lowest, then how far each rail
  alone moves it going to its highest. Any mix of rail ends is the sum, so the bias range of a pin from its
  reference pin covers every mix, not just all-low and all-high.
- A net whose bias only the simulator's tiny leak to ground sets (a capacitor-coupled input with no bias resistor,
  say) has no bias, and its findings are UNKNOWN.
- ngspice then finds each net's gain from each signal across the signal's band, and the gain of the difference
  between the two pins of every limit directly, so their phases count.
- Rails joined by a ferrite bead, fuse or net tie are one rail. Two rails where one is worked out from the other
  (a regulator's output and its input, say) are still swept apart, which overstates how far apart they get: a
  FAIL that rests on that, or on a regulator's low end that is only a floor, is UNKNOWN.
- The AC sweep takes 1000 points a decade, enough to find a resonance up to a Q of about 100 within a few
  percent. Inductors and capacitors are ideal, so an LC with no resistance at all reads lower than the infinite
  peak it would have.
- A pin reaches its bias plus and minus the sum of each signal's peak times its largest gain. That treats all
  signals as in phase, which is the worst case.

The output lists each signal's gain to every net it reaches, and that net's range from ground.

## Rules

| Rule | Fails when | Severity |
|---|---|---|
| `clipping` | an op-amp's output reaches past its supply rails, or closer to one than the datasheet's output swing. It is UNKNOWN when the output stays inside the rails but the part file gives no swing. | warning |
| `signal-level` | the swing on a pin with a voltage limit goes past the limit, measured from the limit's reference pin. Past the absolute maximum it is an error; past the recommended range it is a warning. A limit the part file doesn't give is UNKNOWN. | error / warning |
| `signal-path` | never fails: it is UNKNOWN for each part the simulation leaves open on a net a signal reaches, as nothing past it is checked. | warning |

`check` already covers passives, so `signal-level` leaves them out.

## What is modelled

- Resistors, capacitors and inductors, at their marked values.
- Ferrite beads, fuses and net ties as shorts.
- Op-amps from their part file's `[[opamp]]`, as ideal amplifiers: an open-loop gain of 10⁶ with no bandwidth
  limit. The model has no clipping, so the levels show where a real op-amp would clip.

## What isn't

- Diodes and TVS parts are left open. A signal on a sound design stays below their forward drop and breakdown, but
  a fault-sized signal would be clamped in reality and isn't here.
- IC pins with no `[[opamp]]` are left open and taken to draw no current. That covers codec inputs, modules, and
  amplifiers whose inputs are virtual grounds. A pin marked as an input loads its net with nothing, which is
  right; any other pin (an output, a bidirectional GPIO, a passive pin) could drive or load the net, so every
  level read on that net is UNKNOWN, and the part is a `signal-path` finding. An op-amp whose output is tied to
  ground or a rail (an unused half) drives nothing and is left out.
- Op-amps never clip in the model. When one's output goes past a rail, the FAILs after it are UNKNOWN, as the real
  levels there are lower.
- Tolerances aren't swept, and an op-amp's gain-bandwidth, slew rate, offset and bias current aren't modelled.
- Swing figures are given at one supply and load, usually 25 °C. The note on each part file says which.
