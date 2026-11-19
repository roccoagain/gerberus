# Junction temperature and derating

The `junction-temperature`, `forward-current` and derating rules of `gerberus check`.

To find each part's dissipation, `check` adds up what each rail supplies:
- every IC's `[[supply]]` maximum;
- LED chains, at V / R min with VF taken as 0 V, since a datasheet's VF minimum holds only at its test current and
  25 °C. The current is charged to every supply rail of the IC that drives the LED, and the IC also dissipates it
  at up to its rail voltage;
- resistors, at their largest voltage difference over their smallest resistance: to ground, or to a net an IC can
  pull low (a pull-up).

A net fed through a resistor, inductor, ferrite, fuse or jumper passes its load on to the net feeding it. So does a
series diode, unless its anode can never rise above its cathode, as with a reverse-protection diode.

Regulators:
- A linear regulator passes its output's load to its input. It dissipates (VIN max − VOUT min) × I plus its own
  supply current. When its supply can sag out of regulation, the held output is used at the top of the supply, since
  in dropout it drops less.
- A switching regulator needs a guaranteed `efficiency` in its part file. Datasheets give efficiency only as typical
  curves, so the switcher, its input diode and the rail feeding it come out UNKNOWN.
- A regulator with no dropout headroom in its part file, or whose output can be above its supply, isn't worked out.

Anything that can't be worked out, such as an IC with no supply current or an LED that isn't in a resistor-to-ground
chain, is listed as "not counted". Its rail's total is then only a lower bound, and nothing fed from it passes.

`junction-temperature` takes Tj = Ta + P × θJA for every part with a dissipation. Ta is the top of `inputs.toml`'s
`temperature`. Without it, the part's own rated ambient is used, and any result short of a clear pass is UNKNOWN: over
the maximum, or inside the policy margin. The message gives the ambient the part is good up to.

These are estimates, not proven upper bounds:
- The θJA in datasheets is for a JEDEC test board; a real board's copper changes it.
- Most supply currents are datasheet maximums at the worst condition. Some aren't, and their notes say so: XC6206's
  is a 25 °C maximum, SCD41's is an average over its measurement cycle, and the STM32G0 entries add typical
  per-peripheral figures to a maximum.

Derating for reliability is `policy.toml`'s: capacitor voltage at 80 %, resistor power and diode and LED forward
current at 50 %, and junctions 20 °C below their maximum. A FAIL against a derating is a warning.

On the test boards (normal operation, at each part's rated ambient, as no board gives a temperature):

- The STM32s are under their 105 °C maximum at 85 °C ambient, but inside the 20 °C margin. Rev 2's STM32F042, for
  example, dissipates 96 mW at θJA 118 °C/W, reaching 96 °C. So they are UNKNOWN, and good up to 74 to 81 °C ambient.
- Rev 1's TCAN330 (60 mA dominant × 3.46 V at 154.4 °C/W) would reach 157 °C at its rated 125 °C ambient. It is
  UNKNOWN, and safe up to 118 °C.
- The LDOs (XC6206 at 119 mW, ME6211 at 76 mW) and most sensors have no θJA in their datasheets, so they are UNKNOWN
  with their dissipation. The switchers (TPS560430, AP63200) give no efficiency.
- The LEDs pass `forward-current` at 3.4 to 3.5 mA against 25 to 30 mA ratings.
