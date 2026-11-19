//! Eye diagrams and impedance profiles as standalone SVG images.
use std::fmt::Write as _;

use super::PairReport;

const W: f64 = 640.0;
const H: f64 = 400.0;
const PAD: f64 = 56.0;

fn frame(title: &str) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {W} {H}\" width=\"{W}\" height=\"{H}\" \
         font-family=\"sans-serif\" font-size=\"12\">\n<rect width=\"{W}\" height=\"{H}\" fill=\"#ffffff\"/>\n\
         <text x=\"{PAD}\" y=\"24\" font-size=\"14\" font-weight=\"bold\">{}</text>\n",
        escape(title)
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The pair's eye at the connector, with the template's inner hexagon and outer limits.
pub fn eye(pair: &PairReport) -> Option<String> {
    let eye = pair.eye.as_ref()?;
    let t = eye.template;
    let vmax = 0.6_f64.max(eye.traces.iter().flatten().map(|p| p.1.abs()).fold(0.0, f64::max) * 1.05);
    let x = |u: f64| PAD + u * (W - 2.0 * PAD);
    let y = |v: f64| H / 2.0 - v / vmax * (H / 2.0 - PAD);
    let mut s = frame(&format!(
        "{} eye at {}: {} ({})",
        pair.name,
        pair.to,
        if eye.passes() { "passes" } else { "fails" },
        t.name
    ));
    // Axes and grid.
    for v in [-0.4, -0.2, 0.0, 0.2, 0.4] {
        let _ = writeln!(
            s,
            "<line x1=\"{:.1}\" x2=\"{:.1}\" y1=\"{:.1}\" y2=\"{:.1}\" stroke=\"#ddd\"/><text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"end\">{:.0} mV</text>",
            x(0.0),
            x(1.0),
            y(v),
            y(v),
            x(0.0) - 6.0,
            y(v) + 4.0,
            v * 1e3
        );
    }
    for u in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let _ = writeln!(
            s,
            "<line x1=\"{0:.1}\" x2=\"{0:.1}\" y1=\"{1:.1}\" y2=\"{2:.1}\" stroke=\"#ddd\"/><text x=\"{0:.1}\" y=\"{3:.1}\" text-anchor=\"middle\">{4:.2} UI</text>",
            x(u),
            y(vmax),
            y(-vmax),
            H - PAD + 18.0,
            u
        );
    }
    // Waveform.
    for trace in &eye.traces {
        let pts: Vec<String> = trace
            .iter()
            .map(|(u, v)| format!("{:.1},{:.1}", x(*u), y(*v)))
            .collect();
        let _ = writeln!(
            s,
            "<polyline points=\"{}\" fill=\"none\" stroke=\"#1f6fb2\" stroke-opacity=\"0.25\" stroke-width=\"1\"/>",
            pts.join(" ")
        );
    }
    // Mask.
    let hex: Vec<String> = t
        .hexagon()
        .iter()
        .map(|(u, v)| format!("{:.1},{:.1}", x(*u), y(*v)))
        .collect();
    let _ = writeln!(
        s,
        "<polygon points=\"{}\" fill=\"#d62728\" fill-opacity=\"0.25\" stroke=\"#d62728\"/>",
        hex.join(" ")
    );
    for lim in [t.outer_after_transition, t.outer] {
        for sign in [1.0, -1.0] {
            let _ = writeln!(
                s,
                "<line x1=\"{:.1}\" x2=\"{:.1}\" y1=\"{2:.1}\" y2=\"{2:.1}\" stroke=\"#d62728\" stroke-dasharray=\"4 3\"/>",
                x(0.0),
                x(1.0),
                y(sign * lim)
            );
        }
    }
    let _ = writeln!(
        s,
        "<text x=\"{PAD}\" y=\"{:.1}\">height {:.0} mV · width {:.2} UI · mask margin {:+.0} mV</text>",
        H - 12.0,
        eye.height * 1e3,
        eye.width,
        eye.inner_margin.min(eye.outer_margin) * 1e3
    );
    s.push_str("</svg>\n");
    Some(s)
}

/// The differential impedance each line shows the driver, against time, with the 81–99 Ω band.
pub fn tdr(pair: &PairReport) -> Option<String> {
    if pair.tdr.is_empty() {
        return None;
    }
    let tmax = pair.tdr.iter().flatten().map(|p| p.0).fold(0.0, f64::max).max(1e-12);
    let (zlo, zhi) = (0.0, 200.0);
    let x = |t: f64| PAD + t / tmax * (W - 2.0 * PAD);
    let y = |z: f64| H - PAD - (z.clamp(zlo, zhi) - zlo) / (zhi - zlo) * (H - 2.0 * PAD);
    let mut s = frame(&format!(
        "{} differential impedance from {} (TDR, 200 ps edge)",
        pair.name, pair.from
    ));
    let _ = writeln!(
        s,
        "<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"#2ca02c\" fill-opacity=\"0.15\"/>",
        x(0.0),
        y(super::usb2::Z_MAX),
        x(tmax) - x(0.0),
        y(super::usb2::Z_MIN) - y(super::usb2::Z_MAX)
    );
    for z in [0.0, 45.0, 90.0, 135.0, 180.0] {
        let _ = writeln!(
            s,
            "<line x1=\"{:.1}\" x2=\"{:.1}\" y1=\"{2:.1}\" y2=\"{2:.1}\" stroke=\"#ddd\"/><text x=\"{3:.1}\" y=\"{4:.1}\" text-anchor=\"end\">{5:.0} Ω</text>",
            x(0.0),
            x(tmax),
            y(z),
            x(0.0) - 6.0,
            y(z) + 4.0,
            z
        );
    }
    let steps = 5;
    for k in 0..=steps {
        let t = tmax * f64::from(k) / f64::from(steps);
        let _ = writeln!(
            s,
            "<text x=\"{:.1}\" y=\"{:.1}\" text-anchor=\"middle\">{:.0} ps</text>",
            x(t),
            H - PAD + 18.0,
            t * 1e12
        );
    }
    for (line, color) in pair.tdr.iter().zip(["#1f6fb2", "#ff7f0e"]) {
        let pts: Vec<String> = line.iter().map(|(t, z)| format!("{:.1},{:.1}", x(*t), y(*z))).collect();
        let _ = writeln!(
            s,
            "<polyline points=\"{}\" fill=\"none\" stroke=\"{color}\" stroke-width=\"1.5\"/>",
            pts.join(" ")
        );
    }
    let _ = writeln!(
        s,
        "<text x=\"{PAD}\" y=\"{:.1}\"><tspan fill=\"#1f6fb2\">{}</tspan>  <tspan fill=\"#ff7f0e\">{}</tspan>  (round-trip time; green band 81–99 Ω)</text>",
        H - 12.0,
        escape(&pair.p),
        escape(&pair.n)
    );
    s.push_str("</svg>\n");
    Some(s)
}
