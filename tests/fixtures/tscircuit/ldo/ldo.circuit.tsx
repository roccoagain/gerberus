// A 5 V to 3.3 V regulator board with datasheet part files for every part that carries a voltage, for
// tests/tscircuit.rs to run `check` on. J1 brings in USB VBUS; inputs.toml says what it carries.
export default () => (
  <board width="20mm" height="15mm">
    <pinheader name="J1" pinCount={2} pitch="2.54mm" pinLabels={["VBUS", "GND"]} />
    <chip
      name="U1"
      footprint="sot223"
      manufacturerPartNumber="AMS1117-3.3"
      pinLabels={{ pin1: "GND", pin2: "VOUT", pin3: "VIN" }}
      pinAttributes={{ VIN: { requiresPower: true }, GND: { requiresGround: true }, VOUT: { providesPower: true } }}
    />
    <capacitor name="C1" capacitance="10uF" footprint="0805" manufacturerPartNumber="CL21A106KAYNNNE" />
    <capacitor name="C2" capacitance="100nF" footprint="0402" manufacturerPartNumber="CL05B104KB54PNC" />
    <capacitor name="C3" capacitance="10uF" footprint="0805" manufacturerPartNumber="CL21A106KAYNNNE" />
    <resistor name="R1" resistance="1k" footprint="0402" manufacturerPartNumber="0402WGF1001TCE" />
    <testpoint name="TP1" footprintVariant="pad" padDiameter="1mm" />
    <trace from=".J1 > .VBUS" to="net.VBUS" />
    <trace from=".J1 > .GND" to="net.GND" />
    <trace from=".U1 > .VIN" to="net.VBUS" />
    <trace from=".U1 > .GND" to="net.GND" />
    <trace from=".U1 > .VOUT" to="net.V3_3" />
    <trace from=".C1 > .pin1" to="net.VBUS" />
    <trace from=".C1 > .pin2" to="net.GND" />
    <trace from=".C2 > .pin1" to="net.V3_3" />
    <trace from=".C2 > .pin2" to="net.GND" />
    <trace from=".C3 > .pin1" to="net.V3_3" />
    <trace from=".C3 > .pin2" to="net.GND" />
    <trace from=".R1 > .pin1" to="net.V3_3" />
    <trace from=".R1 > .pin2" to=".TP1 > .pin1" />
  </board>
)
