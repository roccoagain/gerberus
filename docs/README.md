# Gerberus docs

The [README](../README.md) gets you running. These pages go into detail.

Using it:

- [Installing and updating](install.md): platforms, where it installs, updating.
- [Reading designs](reading-designs.md): KiCad support and buses.
- [Board inputs and part files](../parts/README.md): the `inputs.toml` format ([Board inputs](../parts/README.md#board-inputs)),
  the part file format, the derating policy, and how net voltages are worked out.
- [What to add first](doctor.md): `doctor`, the ranked list of missing part files, inputs and limits.
- [Revision diff](diff.md): `diff`, the parts, connections and findings that changed between two revisions.
- [HTML report](html-report.md): `check --html`, the findings ringed on the schematic and board in one web page.
- [Waivers](waivers.md): accepting a finding on purpose, with the reason on record.
- [Voltage checks](voltage-checks.md): `voltages` and `check`, every rule, fault cases, reverse polarity and ESD.
- [Junction temperature and derating](thermal.md): junction temperature, forward current.
- [Bring-up plan](bringup.md): `bringup`, the order to power a new board in and what to measure.
- [Hot-plug stress](stress.md): `stress`, the SPICE simulation and what it models.
- [Signal levels](signal.md): `signal`, op-amp clipping and the levels signals put on IC pins, in SPICE.
- [Pinout check](pinout.md): `pinout`.
- [BOM health](bom.md): `bom`, stock, listing status and second sources at LCSC.
- [Trace current](traces.md): `traces`, track width against the net's current (IPC-2221), voltage drop to power pins, decoupling distance, plane splits, switcher hot loops and SPI/I²S line length.
- [USB signal integrity](usb-si.md): `si`, impedance, skew, stubs and the eye diagram.
- [Datasheet checks](datasheets.md): `datasheet verify`, `blank` and `compare`, for adding part files.

Working on Gerberus:

- [Developing Gerberus](development.md): tests, the kitchen-sink board, CI, releasing, source layout.
- [Roadmap](../ROADMAP.md).
