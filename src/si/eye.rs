//! Eye diagrams: folding a simulated waveform into one unit interval and checking it against a mask.
use super::channel::Waveform;

/// A USB 2.0 high-speed eye template (USB 2.0 specification, section 7.1.2.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Template {
    pub name: &'static str,
    pub source: &'static str,
    /// Inner hexagon: zero volts at `edge` and `1 − edge` of the UI, `inner` volts from `flat` to `1 − flat`.
    pub edge: f64,
    pub flat: f64,
    pub inner: f64,
    /// Outer limits, volts: in the UI after a transition, and in every other UI.
    pub outer_after_transition: f64,
    pub outer: f64,
}

/// Template 1: the transmit eye for a hub at TP2 and for a device without a captive cable at TP3, both at the
/// mated pins of the connector.
pub const USB2_TEMPLATE_1: Template = Template {
    name: "USB 2.0 Template 1",
    source: "USB 2.0 specification, Figure 7-13, PDF page 161",
    edge: 0.075,
    flat: 0.375,
    inner: 0.300,
    outer_after_transition: 0.525,
    outer: 0.475,
};

impl Template {
    /// The inner mask's half-height at a fraction `u` of the UI, volts (0 outside the hexagon).
    pub fn inner_at(&self, u: f64) -> f64 {
        if u <= self.edge || u >= 1.0 - self.edge {
            0.0
        } else if u < self.flat {
            self.inner * (u - self.edge) / (self.flat - self.edge)
        } else if u <= 1.0 - self.flat {
            self.inner
        } else {
            self.inner * (1.0 - self.edge - u) / (self.flat - self.edge)
        }
    }

    /// The hexagon's vertices as `(fraction of UI, volts)`, for drawing.
    pub fn hexagon(&self) -> [(f64, f64); 6] {
        [
            (self.edge, 0.0),
            (self.flat, self.inner),
            (1.0 - self.flat, self.inner),
            (1.0 - self.edge, 0.0),
            (1.0 - self.flat, -self.inner),
            (self.flat, -self.inner),
        ]
    }
}

/// What an eye looks like against a template.
#[derive(Debug, Clone, PartialEq)]
pub struct Eye {
    pub template: Template,
    /// Smallest distance from the waveform out to the inner mask, volts. Negative when it enters the mask.
    pub inner_margin: f64,
    /// Where the waveform comes closest to the inner mask, as a fraction of the UI.
    pub inner_worst_at: f64,
    /// Smallest distance from the waveform in to the outer limits, volts. Negative when it overshoots them.
    pub outer_margin: f64,
    /// The smallest |D+ − D−| in the middle 10 % of the UI, volts.
    pub height: f64,
    /// The UI less the spread of zero crossings, as a fraction of the UI.
    pub width: f64,
    /// Each UI as `(fraction of UI, volts)` samples, for drawing.
    pub traces: Vec<Vec<(f64, f64)>>,
}

impl Eye {
    pub fn passes(&self) -> bool {
        self.inner_margin >= 0.0 && self.outer_margin >= 0.0
    }
}

/// Folds `w` into unit intervals aligned on its zero crossings and measures it against `template`.
pub fn measure(w: &Waveform, template: Template) -> Eye {
    let n = w.diff.len();
    let period = n as f64 * w.dt;
    // Zero crossings, interpolated between samples, as phases within the UI.
    let mut phases = Vec::new();
    for i in 0..n {
        let (a, b) = (w.diff[i], w.diff[(i + 1) % n]);
        if (a < 0.0) != (b < 0.0) && a != b {
            let t = (i as f64 + a / (a - b)) * w.dt;
            phases.push((t % w.ui) / w.ui);
        }
    }
    // Their circular mean is where each UI starts.
    let (s, c) = phases.iter().fold((0.0, 0.0), |(s, c), p| {
        let a = p * std::f64::consts::TAU;
        (s + a.sin(), c + a.cos())
    });
    let start = if phases.is_empty() {
        0.0
    } else {
        s.atan2(c).rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU
    };
    let spread = phases
        .iter()
        .map(|p| {
            let d = p - start;
            d - d.round()
        })
        .fold((0.0_f64, 0.0_f64), |(lo, hi), d| (lo.min(d), hi.max(d)));
    let width = if phases.is_empty() {
        0.0
    } else {
        1.0 - (spread.1 - spread.0)
    };

    let uis = (period / w.ui).round() as usize;
    let value_at = |t: f64| -> f64 {
        let x = (t / w.dt).rem_euclid(n as f64);
        let i = x.floor() as usize % n;
        let f = x - x.floor();
        w.diff[i] * (1.0 - f) + w.diff[(i + 1) % n] * f
    };
    let per_ui = (w.ui / w.dt).round() as usize * 2;
    let mut traces = Vec::with_capacity(uis);
    let mut centres = Vec::with_capacity(uis);
    for k in 0..uis {
        let t0 = (start + k as f64) * w.ui;
        centres.push(value_at(t0 + 0.5 * w.ui));
        traces.push(
            (0..=per_ui)
                .map(|j| {
                    let u = j as f64 / per_ui as f64;
                    (u, value_at(t0 + u * w.ui))
                })
                .collect::<Vec<_>>(),
        );
    }
    let mut inner_margin = f64::INFINITY;
    let mut inner_worst_at = 0.5;
    let mut outer_margin = f64::INFINITY;
    let mut height = f64::INFINITY;
    for k in 0..uis {
        let transition = (centres[k] < 0.0) != (centres[(k + uis - 1) % uis] < 0.0);
        let outer = if transition {
            template.outer_after_transition
        } else {
            template.outer
        };
        for &(u, v) in &traces[k] {
            let mask = template.inner_at(u);
            if mask > 0.0 && v.abs() - mask < inner_margin {
                inner_margin = v.abs() - mask;
                inner_worst_at = u;
            }
            // Where the trace runs into the next UI's crossing, the outer limit of this UI still applies.
            outer_margin = outer_margin.min(outer - v.abs());
            if (0.45..=0.55).contains(&u) {
                height = height.min(v.abs());
            }
        }
    }
    Eye {
        template,
        inner_margin,
        inner_worst_at,
        outer_margin,
        height,
        width,
        traces,
    }
}

#[cfg(test)]
mod tests {
    use super::super::channel::{Ends, Line, simulate};
    use super::*;

    #[test]
    fn the_template_matches_the_spec_figure() {
        let t = USB2_TEMPLATE_1;
        assert_eq!(t.inner_at(0.05), 0.0);
        assert!((t.inner_at(0.225) - 0.15).abs() < 1e-12);
        assert_eq!(t.inner_at(0.5), 0.3);
        assert!((t.inner_at(0.775) - 0.15).abs() < 1e-12);
    }

    #[test]
    fn a_perfect_channel_has_an_open_eye() {
        // No elements: the driver straight into the load.
        let w = simulate(
            &Line::default(),
            &Line::default(),
            Ends {
                source: 45.0,
                load: 45.0,
            },
            1.0 / 480e6,
            500e-12,
            0.4,
            64,
        );
        let eye = measure(&w, USB2_TEMPLATE_1);
        assert!(eye.passes(), "{eye:?}");
        assert!((eye.height - 0.4).abs() < 1e-3, "{}", eye.height);
        assert!(eye.width > 0.99, "{}", eye.width);
    }

    #[test]
    fn a_big_capacitor_closes_the_eye() {
        use super::super::channel::Element;
        let heavy = Line {
            elements: vec![Element::Shunt { c: 155e-12 }],
        };
        let w = simulate(
            &heavy,
            &heavy,
            Ends {
                source: 45.0,
                load: 45.0,
            },
            1.0 / 480e6,
            500e-12,
            0.4,
            64,
        );
        let eye = measure(&w, USB2_TEMPLATE_1);
        assert!(!eye.passes());
        assert!(eye.inner_margin < -0.1, "{}", eye.inner_margin);
    }
}
