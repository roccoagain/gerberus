use rustfft::FftPlanner;
use rustfft::num_complex::Complex64;

use super::xsection::C0;

const MU0: f64 = 4e-7 * std::f64::consts::PI;
const RHO_CU: f64 = 1.72e-8;

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    Line {
        z: f64,
        er_eff: f64,
        length: f64,
        width: f64,
        loss_tangent: f64,
    },
    Shunt {
        c: f64,
    },
    Series {
        l: f64,
    },
    Stub {
        z: f64,
        er_eff: f64,
        length: f64,
        width: f64,
        loss_tangent: f64,
        c_end: f64,
    },
}

type Abcd = [[Complex64; 2]; 2];

fn mul(x: &Abcd, y: &Abcd) -> Abcd {
    [
        [
            x[0][0] * y[0][0] + x[0][1] * y[1][0],
            x[0][0] * y[0][1] + x[0][1] * y[1][1],
        ],
        [
            x[1][0] * y[0][0] + x[1][1] * y[1][0],
            x[1][0] * y[0][1] + x[1][1] * y[1][1],
        ],
    ]
}

fn gamma(f: f64, z: f64, er_eff: f64, width: f64, loss_tangent: f64) -> Complex64 {
    let w = 2.0 * std::f64::consts::PI * f;
    let beta = w * er_eff.sqrt() / C0;
    let rs = (std::f64::consts::PI * f * MU0 * RHO_CU).sqrt();
    let r = 2.0 * rs / width.max(1e-6);
    let alpha = r / (2.0 * z) + beta * loss_tangent / 2.0;
    Complex64::new(alpha, beta)
}

impl Element {
    fn abcd(&self, f: f64) -> Abcd {
        let one = Complex64::new(1.0, 0.0);
        let zero = Complex64::new(0.0, 0.0);
        let jw = Complex64::new(0.0, 2.0 * std::f64::consts::PI * f);
        match *self {
            Element::Line {
                z,
                er_eff,
                length,
                width,
                loss_tangent,
            } => {
                let gl = gamma(f, z, er_eff, width, loss_tangent) * length;
                let (ch, sh) = (gl.cosh(), gl.sinh());
                [[ch, sh * z], [sh / z, ch]]
            }
            Element::Shunt { c } => [[one, zero], [jw * c, one]],
            Element::Series { l } => [[one, jw * l], [zero, one]],
            Element::Stub {
                z,
                er_eff,
                length,
                width,
                loss_tangent,
                c_end,
            } => {
                let t = (gamma(f, z, er_eff, width, loss_tangent) * length).tanh();
                let yc = jw * c_end;
                let y = (yc * z + t) / (z * (one + yc * z * t));
                [[one, zero], [y, one]]
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line {
    pub elements: Vec<Element>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ends {
    pub source: f64,
    pub load: f64,
}

impl Line {
    fn abcd(&self, f: f64) -> Abcd {
        let one = Complex64::new(1.0, 0.0);
        let zero = Complex64::new(0.0, 0.0);
        self.elements
            .iter()
            .fold([[one, zero], [zero, one]], |acc, e| mul(&acc, &e.abcd(f)))
    }

    pub fn transfer(&self, f: f64, ends: Ends) -> Complex64 {
        let m = self.abcd(f);
        let zl = Complex64::new(ends.load, 0.0);
        zl / (m[0][0] * zl + m[0][1] + ends.source * (m[1][0] * zl + m[1][1]))
    }

    pub fn input_impedance(&self, f: f64, ends: Ends) -> Complex64 {
        let m = self.abcd(f);
        let zl = Complex64::new(ends.load, 0.0);
        (m[0][0] * zl + m[0][1]) / (m[1][0] * zl + m[1][1])
    }

    pub fn delay(&self) -> f64 {
        self.elements
            .iter()
            .map(|e| match e {
                Element::Line { er_eff, length, .. } => length * er_eff.sqrt() / C0,
                _ => 0.0,
            })
            .sum()
    }
}

fn filter(x: &[f64], dt: f64, h: impl Fn(f64) -> Complex64) -> Vec<f64> {
    let n = x.len();
    let mut planner = FftPlanner::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut buf: Vec<Complex64> = x.iter().map(|v| Complex64::new(*v, 0.0)).collect();
    fwd.process(&mut buf);
    for (k, b) in buf.iter_mut().enumerate() {
        let (f, conj) = if k <= n / 2 {
            (k as f64 / (n as f64 * dt), false)
        } else {
            ((n - k) as f64 / (n as f64 * dt), true)
        };
        let hk = h(f);
        *b *= if conj { hk.conj() } else { hk };
    }
    inv.process(&mut buf);
    buf.iter().map(|c| c.re / n as f64).collect()
}

pub fn nrz(bits: &[bool], ui: f64, samples_per_ui: usize, rise: f64, amplitude: f64) -> Vec<f64> {
    let full = rise / 0.590_334;
    let n = bits.len() * samples_per_ui;
    let dt = ui / samples_per_ui as f64;
    let level = |b: bool| if b { amplitude } else { -amplitude };
    (0..n)
        .map(|i| {
            let t = i as f64 * dt;
            let k = ((t / ui).floor() as usize).min(bits.len() - 1);
            let now = level(bits[k]);
            let into = t - k as f64 * ui;
            let (prev, next, x) = if into < ui / 2.0 {
                (level(bits[(k + bits.len() - 1) % bits.len()]), now, into)
            } else {
                (now, level(bits[(k + 1) % bits.len()]), into - ui)
            };
            if x.abs() >= full / 2.0 {
                return now;
            }
            let frac = (1.0 - (std::f64::consts::PI * (x / full + 0.5)).cos()) / 2.0;
            prev + (next - prev) * frac
        })
        .collect()
}

pub fn prbs7() -> Vec<bool> {
    let mut s: u8 = 0x7f;
    (0..127)
        .map(|_| {
            let b = ((s >> 6) ^ (s >> 5)) & 1;
            s = ((s << 1) | b) & 0x7f;
            b == 1
        })
        .collect()
}

#[derive(Debug, Clone)]
pub struct Waveform {
    pub dt: f64,
    pub ui: f64,
    pub bits: Vec<bool>,
    pub diff: Vec<f64>,
}

pub fn simulate(p: &Line, n: &Line, ends: Ends, ui: f64, rise: f64, amplitude: f64, samples_per_ui: usize) -> Waveform {
    let bits = prbs7();
    let dt = ui / samples_per_ui as f64;
    let src = nrz(&bits, ui, samples_per_ui, rise, amplitude);
    let vp = filter(&src, dt, |f| p.transfer(f, ends));
    let vn = filter(&src, dt, |f| -n.transfer(f, ends));
    Waveform {
        dt,
        ui,
        bits,
        diff: vp.iter().zip(&vn).map(|(a, b)| a - b).collect(),
    }
}

pub fn tdr(line: &Line, ends: Ends, rise: f64, dt: f64, window: f64) -> Vec<(f64, f64)> {
    let half = ((window * 4.0).max(100e-9) / dt).ceil() as usize;
    let bits = [false, true];
    let ui = half as f64 * dt;
    let src = nrz(&bits, ui, half, rise, 1.0);
    let r = ends.source;
    let port = filter(&src, dt, |f| {
        let zin = line.input_impedance(f, ends);
        zin / (zin + r)
    });
    (0..((window / dt) as usize))
        .map(|i| {
            let k = (half + i) % port.len();
            let rho = (port[k] - src[k] / 2.0).clamp(-0.999, 0.999);
            (i as f64 * dt, r * (1.0 + rho) / (1.0 - rho))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_matched_lossless_line_only_delays() {
        let ends = Ends {
            source: 45.0,
            load: 45.0,
        };
        let line = Line {
            elements: vec![Element::Line {
                z: 45.0,
                er_eff: 3.0,
                length: 0.05,
                width: 1.0,
                loss_tangent: 0.0,
            }],
        };
        for f in [1e6, 2.4e8, 1e9] {
            let h = line.transfer(f, ends);
            assert!((h.norm() - 0.5).abs() < 1e-3, "{f}: {h}");
        }
        assert!((line.delay() - 0.05 * 3f64.sqrt() / C0).abs() < 1e-15);
    }

    #[test]
    fn a_mismatched_line_reflects_what_the_tdr_shows() {
        let ends = Ends {
            source: 45.0,
            load: 45.0,
        };
        let line = Line {
            elements: vec![
                Element::Line {
                    z: 45.0,
                    er_eff: 3.0,
                    length: 0.02,
                    width: 2e-4,
                    loss_tangent: 0.0,
                },
                Element::Line {
                    z: 65.0,
                    er_eff: 3.0,
                    length: 0.03,
                    width: 2e-4,
                    loss_tangent: 0.0,
                },
                Element::Line {
                    z: 45.0,
                    er_eff: 3.0,
                    length: 0.02,
                    width: 2e-4,
                    loss_tangent: 0.0,
                },
            ],
        };
        let profile = tdr(&line, ends, 50e-12, 2e-12, 1e-9);
        let at = |t: f64| {
            profile
                .iter()
                .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
                .unwrap()
                .1
        };
        assert!((at(100e-12) - 45.0).abs() < 1.0, "{}", at(100e-12));
        assert!((at(400e-12) - 65.0).abs() < 2.0, "{}", at(400e-12));
    }

    #[test]
    fn prbs7_is_maximal() {
        let b = prbs7();
        assert_eq!(b.iter().filter(|x| **x).count(), 64);
        let mut longest = 0;
        let mut run = 0;
        for i in 0..254 {
            run = if i > 0 && b[i % 127] == b[(i - 1) % 127] {
                run + 1
            } else {
                1
            };
            longest = longest.max(run);
        }
        assert_eq!(longest, 7);
    }

    #[test]
    fn edges_rise_in_the_time_asked() {
        let bits = [false, true];
        let v = nrz(&bits, 2e-9, 2000, 500e-12, 1.0);
        let t10 = 1000 + v[1000..].iter().position(|x| *x > -0.8).unwrap();
        let t90 = 1000 + v[1000..].iter().position(|x| *x > 0.8).unwrap();
        let rise = (t90 - t10) as f64 * 1e-12;
        assert!((rise - 500e-12).abs() < 3e-12, "{rise}");
    }
}
