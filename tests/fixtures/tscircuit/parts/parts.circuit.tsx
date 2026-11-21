// One of every part kind the tscircuit reader maps, wired so each pin's net is known: tests/tscircuit.rs reads
// the circuit.json that `tsci export` wrote from it.
const Pull = (props: { name: string }) => (
  <group name={props.name} subcircuit>
    <resistor name="R1" resistance="10k" footprint="0402" connections={{ pin1: "net.VCC", pin2: "net.SIG" }} />
    <capacitor name="C1" capacitance="1nF" footprint="0402" connections={{ pin1: "net.SIG", pin2: "net.GND" }} />
  </group>
)

export default () => (
  <board width="40mm" height="40mm">
    <diode name="D1" footprint="sod123" manufacturerPartNumber="B5819WS" />
    <led name="LED1" footprint="0603" color="red" />
    <mosfet name="Q1" channelType="n" mosfetMode="enhancement" footprint="sot23" />
    <transistor name="Q2" type="pnp" footprint="sot23" />
    <pushbutton name="SW1" footprint="pushbutton" internallyConnectedPins={[["pin1", "pin2"], ["pin3", "pin4"]]} />
    <capacitor name="C2" capacitance="10uF" polarized footprint="0805" maxVoltageRating="16V" />
    <capacitor name="C3" capacitance="100nF" footprint="0402" supplierPartNumbers={{ jlcpcb: ["C307331"] }} />
    <crystal name="Y1" frequency="12MHz" loadCapacitance="18pF" footprint="hc49" />
    <pinheader name="J1" pinCount={3} pitch="2.54mm" pinLabels={["VBUS", "DP", "GND"]} />
    <inductor name="L1" inductance="10uH" footprint="0805" />
    <fuse name="F1" currentRating="1A" footprint="1206" />
    <testpoint name="TP1" footprintVariant="pad" padDiameter="1mm" />
    <chip
      name="U1"
      footprint="soic8"
      manufacturerPartNumber="TEST-REG"
      pinLabels={{ pin1: "VIN", pin2: "GND", pin3: "EN", pin4: "VOUT", pin6: "SDA" }}
      pinAttributes={{
        VIN: { requiresPower: true },
        GND: { requiresGround: true },
        VOUT: { providesPower: true },
        EN: { isInput: true },
        SDA: { isBidirectional: true },
      }}
      noConnect={["pin5"]}
    />
    <Pull name="A" />
    <Pull name="B" />
    <trace from=".J1 > .VBUS" to=".F1 > .pin1" />
    <trace from=".F1 > .pin2" to=".D1 > .anode" />
    <trace from=".D1 > .cathode" to="net.VIN" />
    <trace from=".U1 > .VIN" to="net.VIN" />
    <trace from=".U1 > .GND" to="net.GND" />
    <trace from=".J1 > .GND" to="net.GND" />
    <trace from=".C2 > .pos" to="net.VIN" />
    <trace from=".C2 > .neg" to="net.GND" />
    <trace from=".C3 > .pin1" to="net.VCC" />
    <trace from=".C3 > .pin2" to="net.GND" />
    <trace from=".U1 > .VOUT" to="net.VCC" />
    <trace from=".SW1 > .pin1" to=".U1 > .EN" />
    <trace from=".SW1 > .pin2" to=".Q2 > .base" />
    <trace from=".SW1 > .pin3" to="net.VIN" />
    <trace from=".LED1 > .anode" to="net.VCC" />
    <trace from=".LED1 > .cathode" to=".Q1 > .drain" />
    <trace from=".Q1 > .source" to="net.GND" />
    <trace from=".Q2 > .emitter" to="net.VCC" />
    <trace from=".L1 > .pin1" to="net.VCC" />
    <trace from=".TP1 > .pin1" to="net.VCC" />
    <trace from=".Y1 > .pin1" to="net.GND" />
    <trace from=".U1 > .SDA" to=".J1 > .DP" />
  </board>
)
