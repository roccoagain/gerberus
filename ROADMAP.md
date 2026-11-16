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

- [x] SPICE stress checks on power and analog circuits, with tolerance sweeps. Power is done (`gerberus stress`: hot-plug transients in ngspice, Monte Carlo over part tolerances); analog signal paths aren't modelled yet. *(Copies: PSpice Advanced Analysis, smoke and Monte Carlo)*
- [x] Power dissipation → junction temperature for every part. *(Copies: Simcenter Flotherm / Ansys Icepak, simplified)*
- [x] Derating against reliability standards. *(Copies: Relyence, Windchill reliability prediction)*
- [x] ESD and reverse-polarity protection on every external connector. *(Copies: what a human design review checks)*

## v0.4 — Any board

Point Gerberus at any KiCad or Altium design and get useful answers in minutes. Every `UNKNOWN` says exactly what to add and where, and adding it takes one command. Found by running Gerberus on the capstone audio board rev 2: 39 of 58 parts unchecked, `check` refusing to run until someone wrote an `inputs.toml`, and 6 false errors from behaviour Gerberus can't describe.

**Tell the user what's missing**

- [ ] `gerberus doctor <design>`: one to-do list of everything missing, ranked by how many results it unlocks: parts with no part file, datasheets that can't be downloaded (the file name to save and the folder to save it in), connector pins with no input range. `--json` for agents.
- [ ] No `inputs.toml` isn't an error. `check` runs, says which connector pins it had to assume nothing about, and points at `gerberus init`.
- [ ] `gerberus init <design>`: writes a starter `inputs.toml` with every external connector pin, a guessed range from the connector type and net names (USB → VBUS 4.75–5.5 V, D+/D− 0–3.6 V; barrel jack; JST battery), and `guess = "why"` on each. Findings that rest on a guess say so until a person replaces it.
- [ ] Output readable like rustc's: each finding with the parts and nets involved, the numbers compared, and a `help:` line naming the fix. *(Another thread is doing this.)*

**Grow the part library**

- [ ] Part families: one file covers a whole series by decoding the part number (Yageo RC, UniRoyal, Samsung CL, Murata GRM, KEMET). Size gives power and working voltage; the voltage code gives a capacitor's rating. The 28 resistors on rev 2 are one Yageo family.
- [ ] Bulk library: JLCPCB basic parts first, then the most-used extended parts, each through `datasheet verify` and a blind second reading. Track what share of the test boards' parts are covered.
- [ ] Parts with nothing electrical to check (test points, mounting holes, fiducials, logos, net ties) are skipped instead of `UNCHECKED`.
- [ ] Modules and dev boards (Daisy Seed, ESP32 and STM32 modules, Raspberry Pi Pico) as parts with their header pinout and limits.
- [ ] A variant missing from its maker's datasheet (PESD5V0S1BAF): a part file can map it to the base part with a source for why.
- [ ] BOM health by manufacturer part number through DigiKey or Mouser, not only LCSC codes.

**Add your own parts**

- [ ] `gerberus part new <mpn> [--pdf FILE]`: writes a part file into the project's `parts/` folder from the schematic (pins from the symbol, class from the reference, every value `unknown`), ready to fill in and `datasheet verify`.
- [ ] A `datasheets/` folder next to the schematic for PDFs that can't be downloaded (MAX9722A, Daisy Seed), read by `datasheet verify` and `doctor` without `--pdf`.
- [ ] Ship the datasheet-to-part agent skill with each release (`gerberus skill install`), so an agent working in a board's own repo can add parts the same checked way.

**Accept a finding on purpose**

- [ ] Waivers: `waivers.toml` next to the schematic accepts a finding with a reason, who, and when. Waived findings still show, marked waived; a waiver that no longer matches anything is flagged.

**Behaviour Gerberus doesn't model yet**

- [ ] Inputs that bias themselves: a part-file field for pins the part biases internally, such as the AC-coupled inputs of the TPA3118 and TPA6130A2, so `floating-input` doesn't fire on them.
- [ ] MOSFET channels: a FET that is switched on passes its rail through (load switches, reverse-polarity P-FETs).
- [ ] Analog signal paths in SPICE (gain stages, filters), not only power.

## v0.5 — Layout

- [ ] Layout electrical rules: trace width vs current (IPC-2152), decoupling cap distance, high-speed traces crossing plane splits, loop area. *(Copies: HyperLynx DRC)*
- [x] Trace impedance and length matching for USB 2.0 high speed, with an eye-diagram simulation (`gerberus si`). I²S and SPI still to do. *(Copies: Polar Si9000, HyperLynx SI)*
- [ ] Voltage drop and current density across copper. *(Copies: HyperLynx PI, Altium Power Analyzer)*

## v0.6 — Build

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
