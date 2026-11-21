# HTML report

`gerberus check <design> --html report.html` writes the findings as one web page, as well as printing them. The
page holds everything (drawings, styles and script), so it can be mailed or attached as a single file.

```sh
gerberus check amp-board.kicad_sch --html report.html
```

## What's on the page

- **The drawings.** KiCad's own `kicad-cli` draws each schematic sheet with parts on it and, when a `.kicad_pcb` of the same name
  sits next to the schematic, the board (copper and silkscreen on both sides, and the board outline). Each has a
  tab. Scroll to zoom, drag to pan and double-click to see the whole drawing.
- **A ring on each finding's part.** On every sheet that holds one of the part's symbols (each unit of a
  multi-unit part gets its own ring) and on the board in the middle of its footprint's pads. The ring is red for
  an error, orange for a warning, blue for UNKNOWN and green for a pass, with an outline so it stands out on
  copper of the same colour. Click a ring to jump to its finding.
- **The findings**, errors first, then warnings, UNKNOWNs and passes. Each gives where it is, its headline and the
  fix, as the terminal does, with its numbers and datasheet citations folded under "details". Each links to its
  rings: click one to open that drawing zoomed in on the part. Where several findings share a part, the worst one's ring is on top.
- **Filters.** Errors and warnings show at first; click the UNKNOWN or passed count to add those, on the list and the
  drawings.
- **Light and dark.** The page follows the system setting; the button at the top right switches it and
  remembers the choice. In dark mode the schematic is drawn light on dark.

Waived findings are left out.

## Needs kicad-cli

`kicad-cli` comes with KiCad 9 or later. It must be on the PATH:

- macOS: `/Applications/KiCad/KiCad.app/Contents/MacOS/kicad-cli`
- Windows: `C:\Program Files\KiCad\<version>\bin\kicad-cli.exe`
- Linux: installed with KiCad as `kicad-cli`

Without it, the page still lists every finding and says why it has no drawings.
