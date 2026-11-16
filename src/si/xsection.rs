//! A 2D quasi-static field solver for the cross-section of one trace, or an edge-coupled pair, over a reference
//! plane (microstrip) or between two (stripline).
//!
//! It solves Laplace's equation for the potential on a graded rectangular grid by finite volumes, once with the
//! real dielectrics and once in air, and gets the capacitance per unit length from the charge on the trace. The
//! impedance is then `1 / (c √(C · C_air))` and the effective permittivity `C / C_air`. This is the method
//! commercial 2D solvers use (they use boundary elements rather than a grid, which changes the speed, not the
//! answer). It is quasi-static: good while the cross-section is much smaller than a wavelength, which holds
//! for PCB traces far beyond USB 2.0's 240 MHz fundamental.
//!
//! A pair is solved on half the domain. Its odd mode (one trace at +1 V, the other at −1 V) puts a 0 V wall on
//! the symmetry plane; its even mode (both at +1 V) puts a mirror there. A single trace is the even mode of a
//! "pair" with no gap, so it uses the same code.

/// Vacuum permittivity, F/m.
const EPS0: f64 = 8.854_187_812_8e-12;
/// Speed of light, m/s.
pub const C0: f64 = 299_792_458.0;

/// One slab of dielectric, measured up from the lower reference plane in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slab {
    pub bottom: f64,
    pub top: f64,
    pub er: f64,
}

/// A trace cross-section. All lengths are in millimetres.
#[derive(Debug, Clone, PartialEq)]
pub struct CrossSection {
    pub width: f64,
    pub thickness: f64,
    /// Edge-to-edge gap to the other trace of an edge-coupled pair, or `None` for a lone trace.
    pub gap: Option<f64>,
    /// Height of the trace's bottom face above the lower plane.
    pub height: f64,
    /// Height of an upper plane, for a stripline. `None` leaves open air above.
    pub top_plane: Option<f64>,
    /// Dielectrics; anything they don't cover is air.
    pub slabs: Vec<Slab>,
}

/// The impedance and effective permittivity of one propagation mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    /// Ω, for one trace.
    pub z: f64,
    pub er_eff: f64,
}

/// What a cross-section solves to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Solution {
    /// A lone trace's characteristic impedance, or a pair's odd mode (half the differential impedance).
    pub odd: Mode,
    /// A pair's even mode (twice the common-mode impedance). `None` for a lone trace.
    pub even: Option<Mode>,
}

impl Solution {
    /// The differential impedance, Ω: twice the odd-mode impedance, or twice a lone trace's.
    pub fn z_diff(&self) -> f64 {
        2.0 * self.odd.z
    }
}

impl CrossSection {
    fn er_at(&self, z: f64) -> f64 {
        self.slabs
            .iter()
            .find(|s| z >= s.bottom && z <= s.top)
            .map_or(1.0, |s| s.er)
    }

    pub fn solve(&self) -> Solution {
        let walls: &[Wall] = if self.gap.is_some() {
            &[Wall::Ground, Wall::Mirror]
        } else {
            &[Wall::Mirror]
        };
        // Every (mode, dielectric or air, grid) solve is independent, so they run on their own threads. The
        // grid error is first order in the cell size (it comes from the field singularity at the trace's
        // edges), so a second solve on a grid twice as coarse extrapolates it away (Richardson).
        let jobs: Vec<(Wall, bool, f64)> = walls
            .iter()
            .flat_map(|&w| [(w, false, 1.0), (w, false, 2.0), (w, true, 1.0), (w, true, 2.0)])
            .collect();
        let caps: Vec<f64> = std::thread::scope(|scope| {
            let handles: Vec<_> = jobs
                .iter()
                .map(|&(wall, air, coarsen)| scope.spawn(move || self.capacitance(wall, air, coarsen)))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("solver thread panicked"))
                .collect()
        });
        let mode = |k: usize| {
            let c = 2.0 * caps[4 * k] - caps[4 * k + 1];
            let c_air = 2.0 * caps[4 * k + 2] - caps[4 * k + 3];
            Mode {
                z: 1.0 / (C0 * (c * c_air).sqrt()),
                er_eff: c / c_air,
            }
        };
        Solution {
            odd: mode(0),
            even: (walls.len() == 2).then(|| mode(1)),
        }
    }

    /// Capacitance per metre of the right-hand trace, with the symmetry plane at x = 0 as `wall`.
    fn capacitance(&self, wall: Wall, air: bool, coarsen: f64) -> f64 {
        let w = self.width;
        let t = self.thickness.max(0.0);
        let x_in = self.gap.map_or(0.0, |g| g / 2.0);
        let x_out = x_in + if self.gap.is_some() { w } else { w / 2.0 };
        let z_bot = self.height;
        let z_top = z_bot + t;

        // Grid lines: every edge and interface, graded finely near the trace and coarsely far away.
        let mut fine = (w / 40.0).min(self.height / 30.0);
        if t > 0.0 {
            fine = fine.min(t / 2.0);
        }
        if let Some(g) = self.gap {
            fine = fine.min(g / 8.0);
        }
        for s in &self.slabs {
            fine = fine.min((s.top - s.bottom) / 4.0);
        }
        let fine = fine.max(1e-4) * coarsen;
        let span = self.top_plane.unwrap_or(z_top).max(z_top);
        let far = 25.0 * span.max(w);
        let x_end = x_out + far;
        let z_end = self.top_plane.unwrap_or(z_top + far);

        let xs = graded(&[(0.0, fine), (x_in, fine), (x_out, fine), (x_end, far)], fine);
        let mut zkeys = vec![(0.0, fine), (z_bot, fine), (z_top, fine), (z_end, far)];
        for s in &self.slabs {
            for z in [s.bottom, s.top] {
                if z > 0.0 && z < z_end {
                    zkeys.push((z, fine));
                }
            }
        }
        let zs = graded(&zkeys, fine);
        let (nx, nz) = (xs.len(), zs.len());

        // Permittivity of each cell, from the slab at its middle height.
        let eps = |i: usize, j: usize| -> f64 {
            if i + 1 >= nx || j + 1 >= nz {
                return 0.0;
            }
            if air {
                1.0
            } else {
                self.er_at(f64::midpoint(zs[j], zs[j + 1]))
            }
        };

        // Fixed nodes: the planes, the far boundary, the trace and (odd mode) the symmetry wall.
        let tol = fine * 1e-3;
        let on_trace = |i: usize, j: usize| {
            xs[i] >= x_in - tol && xs[i] <= x_out + tol && zs[j] >= z_bot - tol && zs[j] <= z_top + tol
        };
        let fixed = |i: usize, j: usize| -> Option<f64> {
            if on_trace(i, j) {
                Some(1.0)
            } else if j == 0 || j == nz - 1 || i == nx - 1 || (i == 0 && wall == Wall::Ground) {
                Some(0.0)
            } else {
                None
            }
        };

        // Number the free nodes and assemble the five-point finite-volume system.
        let mut index = vec![usize::MAX; nx * nz];
        let mut n = 0;
        for j in 0..nz {
            for i in 0..nx {
                if fixed(i, j).is_none() {
                    index[j * nx + i] = n;
                    n += 1;
                }
            }
        }
        // Coupling between a node and each neighbour: ε × (face length) / (distance).
        let couple = |i: usize, j: usize, di: isize, dj: isize| -> f64 {
            let dz = |j: usize| if j + 1 < nz { zs[j + 1] - zs[j] } else { 0.0 };
            let dx = |i: usize| if i + 1 < nx { xs[i + 1] - xs[i] } else { 0.0 };
            match (di, dj) {
                (1, 0) => {
                    let below = if j > 0 { eps(i, j - 1) * dz(j - 1) } else { 0.0 };
                    f64::midpoint(below, eps(i, j) * dz(j)) / dx(i)
                }
                (-1, 0) => {
                    let below = if j > 0 { eps(i - 1, j - 1) * dz(j - 1) } else { 0.0 };
                    f64::midpoint(below, eps(i - 1, j) * dz(j)) / dx(i - 1)
                }
                (0, 1) => {
                    let left = if i > 0 { eps(i - 1, j) * dx(i - 1) } else { 0.0 };
                    f64::midpoint(left, eps(i, j) * dx(i)) / dz(j)
                }
                _ => {
                    let left = if i > 0 { eps(i - 1, j - 1) * dx(i - 1) } else { 0.0 };
                    f64::midpoint(left, eps(i, j - 1) * dx(i)) / dz(j - 1)
                }
            }
        };
        let neighbours = |i: usize, j: usize| {
            let mut out: Vec<(usize, usize, f64)> = Vec::with_capacity(4);
            if i + 1 < nx {
                out.push((i + 1, j, couple(i, j, 1, 0)));
            }
            if i > 0 {
                out.push((i - 1, j, couple(i, j, -1, 0)));
            }
            if j + 1 < nz {
                out.push((i, j + 1, couple(i, j, 0, 1)));
            }
            if j > 0 {
                out.push((i, j - 1, couple(i, j, 0, -1)));
            }
            out
        };

        let mut rows: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
        let mut diag = vec![0.0; n];
        let mut rhs = vec![0.0; n];
        for j in 0..nz {
            for i in 0..nx {
                let k = index[j * nx + i];
                if k == usize::MAX {
                    continue;
                }
                for (ni, nj, g) in neighbours(i, j) {
                    diag[k] += g;
                    match fixed(ni, nj) {
                        Some(v) => rhs[k] += g * v,
                        None => rows[k].push((index[nj * nx + ni], g)),
                    }
                }
            }
        }
        let phi = conjugate_gradient(&rows, &diag, &rhs);

        // Charge on the trace: the flux leaving its nodes into the free ones and the fixed 0 V ones.
        let mut q = 0.0;
        for j in 0..nz {
            for i in 0..nx {
                if !on_trace(i, j) {
                    continue;
                }
                for (ni, nj, g) in neighbours(i, j) {
                    if on_trace(ni, nj) {
                        continue;
                    }
                    let v = match fixed(ni, nj) {
                        Some(v) => v,
                        None => phi[index[nj * nx + ni]],
                    };
                    q += g * (1.0 - v);
                }
            }
        }
        // A lone trace was solved as its right half.
        let q = if self.gap.is_none() { 2.0 * q } else { q };
        EPS0 * q
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wall {
    /// Odd mode: the plane between the traces sits at 0 V.
    Ground,
    /// Even mode, or a lone trace's centre line: no field crosses it.
    Mirror,
}

/// Grid coordinates through every key point, each `(position, cell size wanted there)`. Cells grow by at most
/// 15 % from one to the next, so the grid is fine at edges and interfaces and coarse far from the trace.
fn graded(keys: &[(f64, f64)], fine: f64) -> Vec<f64> {
    let mut keys: Vec<(f64, f64)> = keys.to_vec();
    keys.sort_by(|a, b| a.0.total_cmp(&b.0));
    keys.dedup_by(|b, a| {
        if (a.0 - b.0).abs() < fine * 1e-3 {
            a.1 = a.1.min(b.1);
            true
        } else {
            false
        }
    });
    const GROWTH: f64 = 0.15;
    let size_at = |x: f64| -> f64 {
        keys.iter()
            .map(|&(k, s)| s + (x - k).abs() * GROWTH)
            .fold(f64::INFINITY, f64::min)
    };
    let mut out = vec![keys[0].0];
    for w in keys.windows(2) {
        let (a, b) = (w[0].0, w[1].0);
        let mut x = a;
        loop {
            let step = size_at(x).min(size_at(x + size_at(x)));
            if x + step >= b - 0.5 * step {
                break;
            }
            x += step;
            out.push(x);
        }
        out.push(b);
    }
    out
}

/// Solves `A x = b` for the symmetric positive-definite `A` given as off-diagonal rows (`A[k][m] = −g`) plus
/// its diagonal, by conjugate gradients with a diagonal incomplete-Cholesky preconditioner. Unknowns are
/// numbered row by row, so a row's lower neighbours are those with smaller numbers.
fn conjugate_gradient(rows: &[Vec<(usize, f64)>], diag: &[f64], b: &[f64]) -> Vec<f64> {
    let n = b.len();
    // DIC: d'_k = a_kk − Σ_{j<k} g_kj² / d'_j, and M = (D' − L) D'⁻¹ (D' − U).
    let mut dp = diag.to_vec();
    for k in 0..n {
        for &(m, g) in &rows[k] {
            if m < k {
                dp[k] -= g * g / dp[m];
            }
        }
    }
    let precondition = |r: &[f64], z: &mut [f64]| {
        for k in 0..n {
            let mut s = r[k];
            for &(m, g) in &rows[k] {
                if m < k {
                    s += g * z[m];
                }
            }
            z[k] = s / dp[k];
        }
        for k in (0..n).rev() {
            let mut s = 0.0;
            for &(m, g) in &rows[k] {
                if m > k {
                    s += g * z[m];
                }
            }
            z[k] += s / dp[k];
        }
    };
    let apply = |x: &[f64], out: &mut [f64]| {
        for k in 0..n {
            let mut s = diag[k] * x[k];
            for &(m, g) in &rows[k] {
                s -= g * x[m];
            }
            out[k] = s;
        }
    };
    let mut x = vec![0.0; n];
    let mut r = b.to_vec();
    let mut z = vec![0.0; n];
    precondition(&r, &mut z);
    let mut p = z.clone();
    let mut ap = vec![0.0; n];
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let b_norm = b.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    for _ in 0..20 * n.max(100) {
        apply(&p, &mut ap);
        let alpha = rz / p.iter().zip(&ap).map(|(a, b)| a * b).sum::<f64>();
        for k in 0..n {
            x[k] += alpha * p[k];
            r[k] -= alpha * ap[k];
        }
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() < 1e-9 * b_norm {
            break;
        }
        precondition(&r, &mut z);
        let rz_new: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = rz_new / rz;
        rz = rz_new;
        for k in 0..n {
            p[k] = z[k] + beta * p[k];
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn microstrip(w: f64, h: f64, t: f64, er: f64, gap: Option<f64>) -> CrossSection {
        CrossSection {
            width: w,
            thickness: t,
            gap,
            height: h,
            top_plane: None,
            slabs: vec![Slab {
                bottom: 0.0,
                top: h,
                er,
            }],
        }
    }

    /// Hammerstad and Jensen's closed form for a zero-thickness microstrip, accurate to about 1 % (Hammerstad
    /// & Jensen, "Accurate Models for Microstrip Computer-Aided Design", IEEE MTT-S 1980).
    fn hammerstad(w: f64, h: f64, er: f64) -> (f64, f64) {
        let u = w / h;
        let f = 6.0 + (2.0 * std::f64::consts::PI - 6.0) * (-(30.666 / u).powf(0.7528)).exp();
        let z_air = 60.0 * (f / u + (1.0 + 4.0 / (u * u)).sqrt()).ln();
        let a = 1.0
            + ((u.powi(4) + (u / 52.0).powi(2)) / (u.powi(4) + 0.432)).ln() / 49.0
            + (1.0 + (u / 18.1).powi(3)).ln() / 18.7;
        let b = 0.564 * ((er - 0.9) / (er + 3.0)).powf(0.053);
        let ee = f64::midpoint(er, 1.0) + (er - 1.0) / 2.0 * (1.0 + 10.0 / u).powf(-a * b);
        (z_air / ee.sqrt(), ee)
    }

    #[test]
    fn lone_microstrip_matches_hammerstad_jensen() {
        for (w, h, er) in [(0.3, 0.2, 4.4), (1.0, 0.2, 4.4), (2.9, 1.6, 4.5), (0.15, 0.1, 3.5)] {
            let s = microstrip(w, h, 0.0, er, None).solve();
            let (z, ee) = hammerstad(w, h, er);
            assert!((s.odd.z - z).abs() / z < 0.01, "w {w} h {h}: solver {} vs {z}", s.odd.z);
            assert!(
                (s.odd.er_eff - ee).abs() / ee < 0.02,
                "w {w} h {h}: εeff {} vs {ee}",
                s.odd.er_eff
            );
        }
    }

    #[test]
    fn centred_stripline_matches_the_exact_formula() {
        // Zero-thickness centred stripline has an exact answer: Z = 30π/√εr · K(k)/K(k'), k = sech(πw/2b).
        let (w, b, er) = (0.2_f64, 0.6_f64, 4.0_f64);
        let k = 1.0 / (std::f64::consts::PI * w / (2.0 * b)).cosh();
        let kp = (1.0 - k * k).sqrt();
        let ellip = |m: f64| {
            // Arithmetic-geometric mean: K(m) = π / (2 AGM(1, √(1−m²))).
            let (mut a, mut g) = (1.0_f64, (1.0 - m * m).sqrt());
            for _ in 0..30 {
                let (an, gn) = (f64::midpoint(a, g), (a * g).sqrt());
                a = an;
                g = gn;
            }
            std::f64::consts::PI / (2.0 * a)
        };
        let exact = 30.0 * std::f64::consts::PI / er.sqrt() * ellip(k) / ellip(kp);
        let s = CrossSection {
            width: w,
            thickness: 0.0,
            gap: None,
            height: b / 2.0,
            top_plane: Some(b),
            slabs: vec![Slab {
                bottom: 0.0,
                top: b,
                er,
            }],
        }
        .solve();
        assert!(
            (s.odd.z - exact).abs() / exact < 0.005,
            "solver {} vs exact {exact}",
            s.odd.z
        );
        assert!((s.odd.er_eff - er).abs() < 1e-6);
    }

    #[test]
    fn a_pair_couples_more_as_the_gap_closes() {
        let lone = microstrip(0.3, 0.2, 0.035, 4.4, None).solve().odd.z;
        let mut last = 0.0;
        for gap in [0.1, 0.2, 0.4, 1.0, 3.0] {
            let s = microstrip(0.3, 0.2, 0.035, 4.4, Some(gap)).solve();
            let even = s.even.unwrap().z;
            assert!(
                s.odd.z < lone && even > lone,
                "gap {gap}: odd {} even {even} lone {lone}",
                s.odd.z
            );
            assert!(s.odd.z > last);
            last = s.odd.z;
        }
        // Far apart, the pair is two lone traces.
        assert!((last - lone).abs() / lone < 0.02, "{last} vs {lone}");
    }
}
