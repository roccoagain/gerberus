# Altium test projects

Openly licensed Altium Designer projects that `tests/altium_boards.rs` reads. Each folder holds the project's
`.PrjPcb`, its `.SchDoc` sheets (plus any `.Harness` files), and a `board-nets.tsv`. Nothing else from the
upstream repositories is included.

| Folder | Project | Upstream | License | Copyright |
|---|---|---|---|---|
| `limesdr-mini` | LimeSDR Mini 1v2 | [myriadrf/LimeSDR-Mini](https://github.com/myriadrf/LimeSDR-Mini) at `ade9b06` | CC BY 3.0 Unported | Lime Microsystems Ltd |
| `mixr-power` | MIXR power board | [MIXR-FYDP/mixr-hardware](https://github.com/MIXR-FYDP/mixr-hardware) | MIT (`LICENSE`) | Taiping Li |
| `misko3` | MiSKo3 | [mjankovec/MiSKo3](https://github.com/mjankovec/MiSKo3) | GPL-3.0 (`LICENSE`) | Matic Jankovec |
| `easyinverter` | easyinverter main board | [hildogjr/easyinverter](https://github.com/hildogjr/easyinverter) | GPL-3.0 (`LICENSE`) | Hildo Guillardi Júnior and contributors |
| `cube-sat-eps` | CubeSat EPS | [Dominik-Workshop/cube-sat-eps](https://github.com/Dominik-Workshop/cube-sat-eps) | MIT (`LICENSE`) | Dominik-Workshop |
| `fmc-dio-32ch` | FMC DIO 32ch LVDS A | [ohwr/project/fmc-dio-32chlvdsa](https://gitlab.com/ohwr/project/fmc-dio-32chlvdsa) | CERN OHL v1.1 | CERN |

All but LimeSDR Mini were taken from the copies in
[IntelligentElectron/test-fixtures](https://github.com/IntelligentElectron/test-fixtures) at `c382aba`, whose
`NOTICE.md` records the same sources and licenses. The files are unmodified. They are test data that the tests
read; they are not compiled or linked into Gerberus.

## How the nets were verified

`board-nets.tsv` lists `REFERENCE.PAD<TAB>NET` for every pad with a net in the project's `.PcbDoc`, written by
`cargo run --example pcbdoc_nets -- <board>.PcbDoc`. Altium compiled that board from the same schematics, so it is
an independent reference for which pins share a net and what each net is called. The test compares every pad
with the reader's net for that pin and fails unless the differences are exactly the ones it lists, each with its
reason (footprints with more pads than their symbol has pins, a stale board, one hidden power pin). The `.PcbDoc`
files themselves are not committed because of their size.

Boards used:

| Folder | Board file upstream |
|---|---|
| `limesdr-mini` | `hardware/1v2/PCB/LimeSDR_Mini_1v2_Rounded.PcbDoc` |
| `mixr-power` | `Layout/MIXR - Power.PcbDoc` |
| `misko3` | `Misko 3.PcbDoc` |
| `easyinverter` | `MainBoard_PCB.PcbDoc` |
| `cube-sat-eps` | `pcb/EPS.PcbDoc` |
| `fmc-dio-32ch` | `PCB-Layout/FMC_DIO_32ch_lvds_a.PcbDoc` |
