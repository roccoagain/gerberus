# Gerberus roadmap

Gerberus checks a PCB design for electrical bugs before it's ordered. It's an open-source, agent-friendly take on the verification tools that big companies pay for.

Each phase reads one more part of the design. It starts with the schematic, then adds parts, power, layout, manufacturing and workflow. Every checker shares the same design graph and the same Breakdown report.

## v0.1 — Schematic

- [x] Voltage-aware schematic checks: worst-case voltage range on every net, with an explanation of where each range came from, plus the first 10 rules: `gerberus voltages` and `gerberus check`. *(Copies: Siemens Xpedition schematic analysis)*

## v0.2 — Parts

- [x] Check that symbol, footprint and datasheet pinouts agree (catches swapped or missing pins): `gerberus pinout`. *(Copies: Ultra Librarian / SnapEDA verification)*
- [x] BOM health: end-of-life parts, stock, second sources. Stock, LCSC listing status and candidate second sources from LCSC are done; manufacturer lifecycle needs a distributor API key. *(Copies: SiliconExpert, Octopart / Supplyframe)*
- [x] Datasheet → part model using an LLM, checked by a blind second reading and by finding every value on its cited page (`gerberus datasheet`). *(New: no paid tool does this)*

## v0.3 — Power and stress

- [ ] SPICE stress checks on power and analog circuits, with tolerance sweeps. *(Copies: PSpice Advanced Analysis, smoke and Monte Carlo)*
- [ ] Power dissipation → junction temperature for every part. *(Copies: Simcenter Flotherm / Ansys Icepak, simplified)*
- [ ] Derating against reliability standards. *(Copies: Relyence, Windchill reliability prediction)*
- [ ] ESD and reverse-polarity protection on every external connector. *(Copies: what a human design review checks)*

## v0.4 — Layout

- [ ] Layout electrical rules: trace width vs current (IPC-2152), decoupling cap distance, high-speed traces crossing plane splits, loop area. *(Copies: HyperLynx DRC)*
- [ ] Trace impedance and length matching (USB, I²S, SPI). *(Copies: Polar Si9000, HyperLynx SI)*
- [ ] Voltage drop and current density across copper. *(Copies: HyperLynx PI, Altium Power Analyzer)*

## v0.5 — Build

- [ ] Assembly and manufacturing checks: tombstoning, thermal reliefs, fiducials. *(Copies: Siemens Valor NPI)*
- [ ] Test-point coverage on every net. *(Copies: XJTAG, Valor test planning)*
- [ ] Board-vs-enclosure collision check from STEP files. *(Copies: Altium MCAD CoDesigner)*

## v1.0 — Workflow

- [ ] Electrical diff between board revisions. *(Copies: Altium / Cadence design compare)*
- [ ] MCP server so AI agents can query nets, run checks and explain findings. *(New: no paid tool is built for agents)*
- [ ] CI integration and a pass/fail badge for the README.

## Signature features (new, not copies)

- [ ] **Firmware ↔ schematic cross-check.** Read the pin assignments in the firmware (Daisy pin config, STM32CubeMX `.ioc`, ESP-IDF) and confirm they match the netlist.
- [ ] **Bring-up plan generator.** Gerberus already knows every rail and voltage range, so it writes the bring-up checklist: which rail to power first, the current limit to set, and the voltage to expect at each test point.

## Principles

- **Never invent datasheet values.** If a limit is missing, the result is `UNKNOWN` with a reason.
- **Deterministic core.** LLMs only help create part models; they never decide whether a check passes.
- **Every finding is explainable.** It cites the numbers it compared, their datasheet pages, and how the net's voltage was derived.
- **False positives are bugs.** A clean board must produce zero errors.
