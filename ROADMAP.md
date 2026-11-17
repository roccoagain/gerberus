# Gerberus roadmap

Gerberus checks a PCB design for electrical bugs before it's ordered. It's an open-source, agent-friendly take on the verification tools that big companies pay for.

Each phase reads one more part of the design. It starts with the schematic, then adds parts, power, making any board work, layout, manufacturing and workflow. Every checker shares the same design graph and the same Breakdown report.

## v0.1 — Schematic

- [x] Voltage-aware schematic checks: worst-case voltage range on every net, with an explanation of where each range came from, plus the first 10 rules: `gerberus voltages` and `gerberus check`. *(Copies: Siemens Xpedition schematic analysis)*

## v0.2 — Parts

- [x] Check that symbol, footprint and datasheet pinouts agree (catches swapped or missing pins): `gerberus pinout`. *(Copies: Ultra Librarian / SnapEDA verification)*
- [x] BOM health: end-of-life parts, stock, second sources. Stock, LCSC listing status and candidate second sources from LCSC are done; manufacturer lifecycle needs a distributor API key. *(Copies: SiliconExpert, Octopart / Supplyframe)*
- [x] Datasheet → part model using an LLM, checked by a blind second reading and by finding every value on its cited page (`gerberus datasheet`). *(New: no paid tool does this)*

## v0.3 — Power and stress

- [x] SPICE stress checks on power and analog circuits, with tolerance sweeps. Power is done (`gerberus stress`: hot-plug transients in ngspice, Monte Carlo over part tolerances); analog signal paths moved to v0.4. *(Copies: PSpice Advanced Analysis, smoke and Monte Carlo)*
- [x] Power dissipation → junction temperature for every part. *(Copies: Simcenter Flotherm / Ansys Icepak, simplified)*
- [x] Derating against reliability standards. *(Copies: Relyence, Windchill reliability prediction)*
- [x] ESD and reverse-polarity protection on every external connector. *(Copies: what a human design review checks)*

## v0.4 — Any board

Point Gerberus at a new board and get useful answers in minutes. Every `UNKNOWN` says exactly what to add and where, and adding it takes one command. These items come from running Gerberus on the capstone audio board rev 2 and the amp board (in roccoagain/hardware): 39 of rev 2's 58 parts were unchecked, `check` wouldn't run until someone wrote an `inputs.toml`, and the amp board gave 6 false `floating-input` errors.

- [x] `gerberus doctor <design>`: one to-do list of everything missing, ranked by how many unknown results each item settles: parts with no part file (with the symbol's datasheet link, and a hint when several share one datasheet and could be a family), connector pins with no input range, pins a part file gives no limit or no `[[drive]]`. `--json` for agents. *(New: paid tools assume a librarian already did this)*
- [x] `check` runs without an `inputs.toml`, and says what that leaves out. Pointing at `gerberus init` waits for `init`.
- [ ] `gerberus init <design>`: writes a starter `inputs.toml` with every external connector pin and a range guessed from the connector type and net names, each marked as a guess with its reason. USB VBUS is 4.0–5.5 V (USB 3.x at the device), D+/D− 0–3.6 V with a "D± shorted to VBUS" fault case (USB 2.0 §7.1.1), and a net named like `12V` is taken at ±10 %. Findings that rest on a guess say so until a person replaces it.
- [x] Output readable like rustc's: each finding with the parts and nets involved, the numbers compared, and a `help:` line naming the fix.
- [x] Part families: one file covers a whole series by decoding its part number exactly, and writes each part's file with every value cited to the series datasheet (`gerberus part show`). Yageo RC_L resistors and Samsung CL capacitors are built in; the 28 resistors on rev 2 are all Yageo RC_L.
- [ ] More families: UniRoyal 0402WGF/0603WAF, Murata GRM, KEMET C-series ceramics, and inductors.
- [ ] Bulk library: JLCPCB basic parts first, then the most-used extended parts, each through `datasheet verify` and a blind second reading. Track what share of the test boards' parts are covered.
- [x] Test points are skipped instead of `UNCHECKED`. (Mounting holes, fiducials and logos left out of the BOM are already skipped.)
- [ ] Net ties join their two nets in the voltage engine and in `stress`, instead of staying `UNCHECKED`.
- [ ] Modules and dev boards (Daisy Seed, ESP32 and STM32 modules, Raspberry Pi Pico) as parts with their header pinout and limits.
- [ ] A `[[variant]]` the datasheet doesn't list (PESD5V0S1BAF), allowed only with a source from the maker that it is the same part.
- [x] `gerberus part new <design> <REF or MPN>`: writes a part file into the project's `parts/` folder from the schematic (pins from the symbol, class from the reference, every value `{ unknown = "not read from the datasheet yet" }`), ready to fill in and `datasheet verify`; `doctor` lists it until it is filled in. Reading a PDF given with `--pdf` waits for the `datasheets/` folder.
- [ ] A `datasheets/` folder next to the schematic for PDFs that can't be downloaded (MAX9722A, Daisy Seed), read by `datasheet verify` and `doctor` without `--pdf`; `doctor` then names the file to save for each datasheet that can't be downloaded.
- [ ] Ship the datasheet-to-part agent skill with each release (`gerberus skill install`), so an agent working in a board's own repo adds parts the same checked way.
- [ ] Waivers: a `waivers.toml` next to the schematic accepts a finding with a reason, who, and when. Waived findings still show, marked waived; a waiver that no longer matches anything is flagged. *(Copies: Siemens Xpedition waivers)*

Behaviour Gerberus doesn't model yet:

- [x] Inputs that bias themselves: a `biases` drive in the part file with the cited bias range, for pins such as the AC-coupled inputs of the TPA3118 and TPA6130A2. The net then gets a voltage the other rules can check, and `floating-input` passes.
- [ ] MOSFET channels in the voltage engine (`stress` already models them at 0.05 Ω; `check` sees only the body diode). On past the VGS that RDS(on) is given at, off below VGS(th) minimum, and both cases checked in between.
- [ ] Analog signal paths in SPICE (gain stages, filters), not only power. Moved here from v0.3.
- [ ] BOM health by manufacturer part number for DigiKey- or Mouser-sourced boards, not only LCSC codes (needs a distributor API key, as v0.2's lifecycle check does).

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
