"""Writes the bus test schematics in this folder.

The files use KiCad 7's format (version 20230121) so that `kicad-cli` 7, the version Ubuntu ships, can check
them: `kicad-cli sch export netlist --format kicadsexpr -o expect.net root.kicad_sch`. The reader reads the
same items in KiCad 10 files. Run `python3 gen.py` here, then regenerate `expect.net` with the command above.
"""

import itertools

PROJECT = "bus"
ROOT_UUID = "10000000-0000-0000-0000-000000000000"
SHEET_UUID = "10000000-0000-0000-0000-0000000000c1"
_ids = itertools.count(1)


def uid():
    return f"20000000-0000-0000-0000-{next(_ids):012d}"


FONT = "(effects (font (size 1.27 1.27)))"
LIB = f"""(lib_symbols
    (symbol "Device:R" (pin_numbers hide) (pin_names (offset 0)) (in_bom yes) (on_board yes)
      (property "Reference" "R" (at 2.032 0 90) {FONT})
      (property "Value" "R" (at 0 0 90) {FONT})
      (property "Footprint" "" (at 0 0 0) {FONT})
      (property "Datasheet" "~" (at 0 0 0) {FONT})
      (symbol "R_0_1" (rectangle (start -1.016 -2.54) (end 1.016 2.54) (stroke (width 0.254) (type default)) (fill (type none))))
      (symbol "R_1_1"
        (pin passive line (at 0 3.81 270) (length 1.27) (name "~" {FONT}) (number "1" {FONT}))
        (pin passive line (at 0 -3.81 90) (length 1.27) (name "~" {FONT}) (number "2" {FONT})))))"""


class Sheet:
    def __init__(self, uuid, inst_path):
        self.uuid = uuid
        self.inst_path = inst_path
        self.items = []

    def bus(self, a, b):
        self.items.append(f"(bus (pts (xy {a[0]} {a[1]}) (xy {b[0]} {b[1]})) (stroke (width 0) (type default)) (uuid {uid()}))")

    def wire(self, a, b):
        self.items.append(f"(wire (pts (xy {a[0]} {a[1]}) (xy {b[0]} {b[1]})) (stroke (width 0) (type default)) (uuid {uid()}))")

    def label(self, kind, name, at):
        shape = "" if kind == "label" else "(shape bidirectional) "
        self.items.append(f'({kind} "{name}" {shape}(at {at[0]} {at[1]} 0) (effects (font (size 1.27 1.27)) (justify left bottom)) (uuid {uid()}))')

    def resistor(self, ref, at):
        self.items.append(
            f'(symbol (lib_id "Device:R") (at {at[0]} {at[1]} 0) (unit 1) (in_bom yes) (on_board yes) (dnp no) (uuid {uid()})\n'
            f'    (property "Reference" "{ref}" (at {at[0] + 2.54} {at[1]} 0) {FONT})\n'
            f'    (property "Value" "1k" (at {at[0] + 5.08} {at[1]} 0) {FONT})\n'
            f'    (pin "1" (uuid {uid()})) (pin "2" (uuid {uid()}))\n'
            f'    (instances (project "{PROJECT}" (path "{self.inst_path}" (reference "{ref}") (unit 1)))))'
        )

    def tap(self, x, y, ref, label=None):
        """A bus entry at (x, y) on a horizontal bus, a wire down to resistor `ref` pin 1, and an optional label
        on that pin."""
        ex, ey = round(x + 2.54, 2), round(y + 2.54, 2)
        self.items.append(f"(bus_entry (at {x} {y}) (size 2.54 2.54) (stroke (width 0) (type default)) (uuid {uid()}))")
        pin = (ex, round(y + 12.7, 2))
        self.wire((ex, ey), pin)
        self.resistor(ref, (pin[0], round(pin[1] + 3.81, 2)))
        if label:
            self.label("label", label, pin)

    def pin_label(self, ref, at, label, kind="label"):
        """Resistor `ref` with a label straight on pin 1, no wire."""
        self.resistor(ref, (at[0], round(at[1] + 3.81, 2)))
        self.label(kind, label, at)

    def write(self, path, extra=""):
        body = "\n  ".join(self.items)
        with open(path, "w") as f:
            f.write(
                f'(kicad_sch (version 20230121) (generator eeschema)\n  (uuid {self.uuid})\n  (paper "A3")\n'
                f"  {LIB}\n  {extra}{body}\n)\n"
            )


root = Sheet(ROOT_UUID, f"/{ROOT_UUID}")
child = Sheet("10000000-0000-0000-0000-0000000000c2", f"/{ROOT_UUID}/{SHEET_UUID}")

# A: vector bus D[0..3] on the root meets the child's B[0..3] through a sheet pin. Members pair by index.
root.bus((20, 40), (140, 40))
root.label("label", "D[0..3]", (20, 40))
root.tap(30, 40, "R1", "D0")
root.tap(40, 40, "R2", "D2")
root.tap(50, 40, "R3")  # no label: a bus entry alone joins nothing
child.bus((20, 40), (70, 40))
child.label("hierarchical_label", "B[0..3]", (20, 40))
child.tap(30, 40, "R10", "B0")
child.tap(40, 40, "R11", "B1")
child.tap(50, 40, "R18")

# B: a named group bus. Members are USB.DP and USB.DM.
root.bus((20, 70), (140, 70))
root.label("label", "USB{DP DM}", (20, 70))
root.tap(30, 70, "R7", "USB.DP")
child.bus((20, 70), (70, 70))
child.label("hierarchical_label", "USB{DP DM}", (20, 70))
child.tap(30, 70, "R12", "USB.DM")

# C: one bus with two vector labels. Members pair by index, so A0 and ADDR0 are one net.
root.bus((20, 100), (80, 100))
root.label("label", "ADDR[0..1]", (20, 100))
root.label("label", "A[0..1]", (80, 100))
root.tap(30, 100, "R4", "A0")
root.tap(40, 100, "R5", "ADDR1")
root.pin_label("R6", (100, 100), "ADDR0")

# D: a bus alias inside a group, through the sheet.
root.bus((20, 130), (140, 130))
root.label("label", "{CTRL}", (20, 130))
root.tap(30, 130, "R8", "EN")
child.bus((20, 130), (70, 130))
child.label("hierarchical_label", "{CTRL}", (20, 130))
child.tap(30, 130, "R15", "RST")

# E: a global bus label makes its members global, and a local label on that sheet joins a member by name.
root.bus((20, 160), (60, 160))
root.label("global_label", "G[0..1]", (20, 160))
root.tap(30, 160, "R9", "G0")
root.pin_label("R17", (100, 160), "G1")
child.bus((20, 160), (60, 160))
child.label("global_label", "G[0..1]", (20, 160))
child.tap(30, 160, "R16", "G1")

# F: a vector inside a named group, through the sheet. Members pair by name.
root.bus((20, 190), (140, 190))
root.label("label", "MEM{D[0..1] CLK}", (20, 190))
root.tap(30, 190, "R20", "MEM.D1")
root.tap(40, 190, "R21", "MEM.CLK")
child.bus((20, 190), (70, 190))
child.label("hierarchical_label", "MEM{D[0..1] CLK}", (20, 190))
child.tap(30, 190, "R22", "MEM.D1")
child.tap(40, 190, "R23", "MEM.CLK")

# G: the root bus has only the sheet pin; the child bus has its hierarchical label and a local label too.
root.bus((100, 220), (140, 220))
root.tap(110, 220, "R24", "S0")
child.bus((20, 220), (80, 220))
child.label("hierarchical_label", "S[0..1]", (20, 220))
child.label("label", "T[0..1]", (80, 220))
child.tap(30, 220, "R25", "T1")
child.tap(40, 220, "R26", "S0")

# H: a net on a child-only bus that also leaves the sheet through a plain hierarchical label. The bus member
# names it on the child sheet, but the root's local label still wins across the hierarchy.
child.bus((20, 250), (70, 250))
child.label("label", "K[0..1]", (20, 250))
child.tap(30, 250, "R27", "K0")
child.label("hierarchical_label", "HH", (32.54, 252.54))
root.wire((140, 250), (120, 250))
root.label("label", "PARENT", (120, 250))
root.resistor("R28", (120, 253.81))

FONT_L = "(effects (font (size 1.27 1.27)) (justify left))"
sheet = f"""(sheet (at 140 30) (size 30 230) (fields_autoplaced) (stroke (width 0.1524) (type solid)) (fill (color 0 0 0 0.0000)) (uuid {SHEET_UUID})
    (property "Sheetname" "child" (at 140 29 0) {FONT_L})
    (property "Sheetfile" "child.kicad_sch" (at 140 261 0) {FONT_L})
    (pin "B[0..3]" bidirectional (at 140 40 180) {FONT_L} (uuid {uid()}))
    (pin "USB{{DP DM}}" bidirectional (at 140 70 180) {FONT_L} (uuid {uid()}))
    (pin "{{CTRL}}" bidirectional (at 140 130 180) {FONT_L} (uuid {uid()}))
    (pin "MEM{{D[0..1] CLK}}" bidirectional (at 140 190 180) {FONT_L} (uuid {uid()}))
    (pin "S[0..1]" bidirectional (at 140 220 180) {FONT_L} (uuid {uid()}))
    (pin "HH" bidirectional (at 140 250 180) {FONT_L} (uuid {uid()}))
    (instances (project "{PROJECT}" (path "/{ROOT_UUID}" (page "2")))))
  """
alias = '(bus_alias "CTRL" (members "EN" "RST"))\n  '
root.items.append(sheet)
root.items.append('(sheet_instances (path "/" (page "1")))')
root.write("root.kicad_sch", alias)
child.write("child.kicad_sch", alias)
