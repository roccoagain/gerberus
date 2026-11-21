# Rules

What each rule a report names in brackets (`error[abs-max]`) checks, and why. `gerberus explain RULE` prints a
rule's section, as `rustc --explain` does for an error code. A report gives each finding's headline, where it is and
what to do; `--verbose` adds its numbers and datasheet citations.

## abs-max

A pin can go past its absolute maximum rating: the voltage beyond which the datasheet says the part may be damaged,
even briefly. The pin's net is taken at its worst-case range (`gerberus voltages --explain NET` shows how it got
it), in normal operation and in each fault case of `inputs.toml`. A limit relative to another pin (`VDD + 0.3 V`)
holds only if it holds however both nets move. See `docs/voltage-checks.md`.

Fix it with a divider, clamp or series resistor that keeps the pin inside its rating, or with a part rated higher.

## recommended

A pin can leave its recommended operating range. The part isn't damaged, but its datasheet no longer promises it
works to spec. For passives and connectors the range is their voltage rating, so past it is an error: a resistor's
rated voltage is √(P·R), capped at its maximum working voltage.

## tvs-breakdown

A rail reaches the lowest breakdown voltage of a TVS or ESD diode on it. Breakdown is where the diode starts to
conduct; a rail held there drives current through it continuously, which a TVS, sized for short pulses, can't
dissipate, so it overheats and fails, often shorted.

Choose a TVS whose lowest breakdown voltage is above the rail's highest voltage.

## tvs-standoff

A rail rises above a TVS diode's stand-off (working) voltage, the highest voltage at which the datasheet promises
only a small leakage current. Above it the diode leaks more, and nearer breakdown it starts to conduct.

## cap-voltage

A capacitor sees more than its rated voltage (an error), or more than `policy.toml`'s derating allows (a warning;
80 % of the rating by default). Ceramic capacitors also lose capacitance as their DC voltage rises, so a part run
near its rating may be far below its marked value.

## resistor-power

A resistor's worst-case power, the largest voltage across it squared over its smallest resistance (tolerance and
temperature drift included), is over its rating (an error) or over `policy.toml`'s share of it (a warning; 50 %
by default). A resistor run hot drifts in value and ages faster, so reliability derating keeps it at or below half
its rated power. Ratings are often given at 70 °C ambient and fall above it.

Use a larger package or a higher power rating, or raise the resistance.

## floating-input

An input pin is on a net that nothing drives: no supply, output pin or resistor reaches it. A floating CMOS input
picks up noise, can switch at random, and can draw current while it sits between the logic levels. It is UNKNOWN
when only open-drain or tri-state outputs, connectors or parts with no part file reach it, as they may drive it.

Tie the pin to a defined level, directly or through a pull resistor.

## unpowered

A power pin is on a net nothing supplies: no input, regulator, output pin or resistor path reaches it, so the part
never powers up.

## regulator-headroom

A regulator's input can fall below what its output needs: its output voltage plus its dropout, at the load its
datasheet specifies. Below that the regulator drops out and its output follows its input down.

Raise the regulator's input, or use a regulator with less dropout.

## esd

A connector pin a person can touch reaches an IC pin with no TVS or ESD clamp on the net. A discharge through a
connector is a system-level event, rated by IEC 61000-4-2: level 4, the usual target for a connector a person plugs
cables into, is 8 kV contact and 15 kV air. An IC's HBM or CDM rating (±1 kV or ±2 kV, say) covers only handling in
a factory, so it isn't enough. The IC needs its own IEC 61000-4-2 rating at `policy.toml`'s `[esd]` level, or a clamp
beside the connector. The rule follows one series resistor, inductor or diode from the connector. See
`docs/voltage-checks.md`.

Add a TVS or ESD clamp next to the connector pin, breaking down below the absolute maximum of the IC pins it guards.

## reverse-polarity

A supply input connected backwards takes a part past its absolute maximum ratings. Every supply input is checked
reversed, as the fault case `<name> reversed`, unless its `inputs.toml` entry says why it can't be
(`reverse = "keyed connector"`). The case's own findings say which pins fail.

Add reverse-polarity protection on the input: a series diode, or an ideal-diode MOSFET for less drop.

## junction-temperature

A part's junction, at the board's ambient (the top of `inputs.toml`'s `temperature`) plus its dissipation × θJA,
passes its maximum (an error) or comes within `policy.toml`'s `[junction]` margin of it (a warning; 20 °C). θJA is
the datasheet's figure for a JEDEC test board, so a real board's copper changes it. See `docs/thermal.md`.

Cut the part's dissipation, or give it a lower thermal resistance: more copper under it, or a bigger package.

## forward-current

A diode or LED carries more than its forward current rating (an error) or `policy.toml`'s share of it (a warning;
50 %). An LED's current is taken at V / R with its forward voltage counted as 0 V, as a datasheet's VF minimum holds
only at its test current and 25 °C.

Raise the series resistor until the current is inside the rating.

## unchecked

UNKNOWN only: a fitted part or pin has no limits to check against. Give the part a part file (`gerberus part new`).

## trace-current

A track can't carry its net's current without heating more than `policy.toml`'s `[traces] rise` (10 °C), by
IPC-2221's fit for its width and copper weight. See `docs/traces.md`.

Widen the track, or move it to an outer layer or thicker copper.

## voltage-drop

The copper between a power pin and its supply drops so much that the pin falls below its recommended minimum, at
the pin's own maximum current. See `docs/traces.md`.

Widen or shorten the tracks from the supply, or feed the pin from a plane.

## decoupling

A power pin's nearest decoupling capacitor is further than `policy.toml`'s `[decoupling] distance` (5 mm), pad
centre to pad centre. A decoupling capacitor supplies the pin's fast current steps; the track between them adds
inductance that undoes it. A capacitor larger than `[decoupling] largest` (1 µF) is taken as bulk storage, which
needn't be close.

Move a decoupling capacitor next to the pin, or add one there.

## plane-split

A track carrying signal edges runs over two separate pieces of plane on a neighbouring layer (ground islands, or a
ground and a power pour), and no neighbouring layer has one unbroken plane under it. A signal's return current
flows in the plane right under its track; at the gap it has to go around, which makes a loop that radiates and
picks up noise. Refill zones in KiCad before checking.

Route the track over one unbroken plane, or bridge the gap with a stitching capacitor beside the track.

## hot-loop

A switching regulator's input loop, from its input capacitor into its supply pin and out of its ground pin, is
longer than `policy.toml`'s `[hot_loop] length` (15 mm) round the pad centres. Its current switches on and off at
every edge, so the loop's inductance rings and radiates. Each supply pin needs an input capacitor within a few
millimetres of it and the ground pin.

Move the input capacitor right beside the regulator's supply and ground pins.

## line-length

UNKNOWN only: an SPI or I²S line is longer than a sixth of its edge's length (from `[edges] rise`), so a push-pull
driver's edge may reflect off its far end and ring. Whether it does depends on the driver and the load.

Add a series resistor beside the driver, or shorten the line.

## thermal-relief

A through-hole pad joins a copper pour solidly. The plane draws the soldering heat away, so the solder may not wet
the pad or fill the hole.

Set the pad's zone connection to thermal relief.

## tombstoning

A two-pad part of 0603 or smaller has one pad joined solidly to a pour and the other not. The solid pad heats more
slowly in reflow, so the other pad's solder melts first, and its surface tension can stand the part up on its end.

Join both pads to the pour the same way (thermal reliefs on both), or neither.

## fiducials

A side of the board carries a fine-pitch part (`[assembly] fine_pitch`, 0.5 mm) but fewer than `[assembly]
fiducials` (3) fiducials. Three global fiducials let the placement machine correct the board's offset, rotation and
stretch, which fine-pitch parts need to land on their pads.

Add fiducials (KiCad's `Fiducial` library) near three corners of that side.

## hot-plug

Plugging the board in, simulated in ngspice, takes a pin past its rating: the cable's inductance rings with the
input capacitors, and a rail can overshoot its settled value, up to about twice it. See `docs/stress.md`.

Damp the ringing: a TVS at the input, a bulk electrolytic capacitor (whose ESR damps it) or an RC snubber, or a load
switch that turns on slowly.

## waiver

A waiver in `waivers.toml` matches no failed or unknown finding: the problem was fixed, the board changed, or its
`rule` or `contains` is misspelt, so it accepts nothing. See `docs/waivers.md`.

Remove the waiver, or fix it to match what the report says now.

## clipping

An op-amp's output, on the loudest signal of `inputs.toml`, reaches past its supply rails or closer to one than the
datasheet's output swing, so it clips. See `docs/signal.md`.

## signal-level

A signal's swing on an IC pin goes past the pin's absolute maximum (an error) or recommended range (a warning),
measured from the limit's reference pin.

## signal-path

The signal simulation leaves a part open on a net a signal reaches, so nothing past it is checked.

## pinout

A part's symbol pins, footprint pads and datasheet pin table (the `[pins]` of its part file) don't agree: a pin
with no pad, pins whose names look swapped, a do-not-connect pin wired to a net. A swapped pin is a board that
doesn't work. See `docs/pinout.md`.

## usb-impedance

A stretch of a USB 2.0 pair is outside 81–99 Ω differential (TI SPRAAR7J Table A-1). An impedance step reflects
part of each edge back to the driver and closes the eye. See `docs/usb-si.md`.

Set the trace width and gap for 90 Ω on the board's stackup, with the pair coupled all along.

## usb-skew

The two lines of a USB pair differ in length by more than 1.27 mm (50 mil). The difference turns part of the
differential signal into common mode, which radiates and closes the eye.

Lengthen the shorter line with a small serpentine near where the mismatch starts.

## usb-vias

A USB line has more than 4 vias, or the two lines have different counts. Each via is a small impedance step, and an
unequal count unbalances the pair.

## usb-stubs

Copper branches off a USB line for 5.08 mm (200 mil) or more to a test point, ESD part or anything else off the
through path (TUSB1210 Table 10-1). A stub reflects each edge back onto the line.

Put the part on the line itself, so the line passes through its pads.

## usb-reference

A stretch of a USB pair has no unbroken plane on the next layer, or runs within 0.64 mm (25 mil) of the plane's
edge. Its return current then has no path right under it, so the impedance jumps and the pair radiates.

## usb-length

A USB pair is longer than 12 inches.

## usb-capacitance

Parts on a USB line add more than 4 pF, which slows its edges.

## usb-eye

The simulated eye at the connector breaks USB 2.0's Template 1: the signal can't be read reliably at 480 Mb/s.

## bom-identity

The LCSC number in the schematic is a different part from its `MPN` field: another value, size or a lower rating.
JLCPCB fits the LCSC part, while gerberus checks the MPN's part file. See `docs/bom.md`.

## bom-stock

LCSC has too little stock of the part for the number of boards.

## bom-lifecycle

LCSC no longer lists the part, or lists it with a status other than normal. With `--jlcpcb`, a part LCSC doesn't
list can't be assembled.

## bom-second-source

LCSC lists no similar part from another maker.

## bom-assembly

A resistor or capacitor is a JLCPCB extended part, which costs a loading fee, and LCSC lists a basic equivalent.
