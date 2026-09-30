// SPDX-License-Identifier: MIT

//! Fine-mesh multigroup CMFD (coarse cell = fine cell) acceleration
//! for the S_N solver, with the optimally-diffusive stabilization.
//!
//! Per outer iteration the transport pass supplies, per group, the
//! cell scalar flux φ and the net face currents J of its last sweep.
//! Each interior face i→j (axis a, width h) is closed as
//!
//! ```text
//!   J = -D̃ (φ_j − φ_i) − D̂ (φ_i + φ_j),   D̃ = D_FD + θ(τ),
//!   D_FD = 2 D_i D_j / (h (D_i + D_j)),     D = 1 / (3 σ_eff)
//! ```
//!
//! and D̂ is the exact remainder that makes the low-order face current
//! equal the transport current for the transport flux. Domain-boundary
//! faces use the partial-current form `J_out = D̂_b φ_i − J_in` with
//! `D̂_b = J_out,partial / φ_i ≥ 0` (transport-consistent; the declared
//! incident current stays a fixed source). The sweep also records each
//! cell's exact per-ordinate balance defect (positivity clamps, θ
//! repair, exponential-source shift), and periodic faces — whose
//! inflow is a lagged plane average, not the neighbor's outflow — are
//! closed with the mean current plus a per-cell right-hand-side term
//! for the difference. With those on the right-hand side the transport
//! fixed point is an exact solution of the low-order system, so the
//! acceleration never moves the converged answer.
//!
//! The low-order system is the P0 multigroup balance
//! `Σ_f A J_f + (σ_eff − S_gg) V φ_g − Σ_{g'≠g} S_g'g V φ_g' = Q V + defect`
//! (`σ_eff` and `S` exactly as the sweep uses them; P1/higher scatter
//! sources integrate to zero over the quadrature). It is solved by
//! group Gauss-Seidel with Jacobi-BiCGSTAB per group for the
//! downscatter-only groups and, for the upscatter-coupled block, one
//! BiCGSTAB over all block groups with the exact per-cell energy-
//! coupling block as preconditioner.
//!
//! od-CMFD (Zhu, Xu & Downar, "An optimally diffusive Coarse Mesh
//! Finite Difference method to accelerate neutron transport
//! calculations", Annals of Nuclear Energy, 2016,
//! doi:10.1016/j.anucene.2016.05.004) modifies the diffusion
//! coefficient to D + θ(τ)·h with θ a function of the coarse-cell
//! optical thickness τ = σ h: θ = 0 for optically thin cells (plain
//! CMFD), rising to 1/4 (the pCMFD value) for thick cells. The paper
//! gives θ as a polynomial fit on 0.8 ≤ τ < 14; that fit's
//! coefficients were not available to this implementation, so
//! `theta_od` uses a smooth monotone surrogate honouring the published
//! limits (0 up to τ = 0.8, 1/4 from τ = 14, smoothstep in ln τ
//! between). Since D̂ absorbs any D̃, θ only conditions the low-order
//! system; it does not move the fixed point (θ ≡ 0 was measured not to
//! converge on the layered-head case, so the stabilization matters).

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// Optically-thin end of the od-CMFD ramp (θ = 0 below).
const TAU_THIN: f64 = 0.8;
/// Optically-thick end of the od-CMFD ramp (θ = 1/4 above).
const TAU_THICK: f64 = 14.0;
/// Floor on σ_eff in the diffusion coefficient, cm⁻¹ — bounds D in
/// near-void cells (D̂ carries the true coupling there).
const SIGMA_FLOOR: f64 = 1.0e-3;
/// Chunk length for deterministic reductions.
const RED_CHUNK: usize = 8192;
/// Minimum items per parallel task in the low-order kernels — small
/// grids are dominated by task-dispatch overhead otherwise.
const MIN_LEN: usize = 4096;

/// Counters shared with the solver options for diagnostics.
#[derive(Debug, Default)]
pub struct SnStats {
    /// Within-group transport sweeps performed (one per group per inner).
    pub group_sweeps: AtomicU64,
    /// CMFD low-order solves performed.
    pub cmfd_solves: AtomicU64,
    /// Faces where D̂ was zeroed (φ_i+φ_j ≈ 0 or non-finite).
    pub dhat_guards: AtomicU64,
    /// Cells where φ_cmfd ≤ 0 (or non-finite) kept the transport flux.
    pub positivity_guards: AtomicU64,
    /// Low-order solves abandoned (breakdown, non-finite, bad diagonal).
    pub solve_failures: AtomicU64,
    /// Total BiCGSTAB iterations across all low-order group solves.
    pub lo_iterations: AtomicU64,
    /// f64 bits: at convergence, the largest change (relative to each
    /// group's peak flux) one more low-order solve would make.
    pub probe_change_bits: AtomicU64,
}

impl SnStats {
    pub fn group_sweeps(&self) -> u64 {
        self.group_sweeps.load(Ordering::Relaxed)
    }
    pub fn cmfd_solves(&self) -> u64 {
        self.cmfd_solves.load(Ordering::Relaxed)
    }
    pub fn dhat_guards(&self) -> u64 {
        self.dhat_guards.load(Ordering::Relaxed)
    }
    pub fn positivity_guards(&self) -> u64 {
        self.positivity_guards.load(Ordering::Relaxed)
    }
    pub fn solve_failures(&self) -> u64 {
        self.solve_failures.load(Ordering::Relaxed)
    }
    /// Fixed-point probe: max |φ_cmfd − φ| / peak_g at convergence.
    pub fn probe_change(&self) -> f64 {
        f64::from_bits(self.probe_change_bits.load(Ordering::Relaxed))
    }
    pub fn lo_iterations(&self) -> u64 {
        self.lo_iterations.load(Ordering::Relaxed)
    }
}

/// Grid description for the low-order operator.
#[derive(Clone, Copy)]
pub(crate) struct Geom {
    pub shape: [usize; 3],
    /// Cell widths, cm.
    pub h: [f64; 3],
    pub periodic: [bool; 3],
}

impl Geom {
    fn n(&self) -> usize {
        self.shape[0] * self.shape[1] * self.shape[2]
    }
    fn stride(&self) -> [usize; 3] {
        [1, self.shape[0], self.shape[0] * self.shape[1]]
    }
    fn coord(&self, cell: usize) -> [usize; 3] {
        let [nx, ny, _] = self.shape;
        [cell % nx, (cell / nx) % ny, cell / (nx * ny)]
    }
}

/// od-CMFD artificial diffusion θ(τ): 0 for τ ≤ 0.8, 1/4 for τ ≥ 14,
/// smoothstep in ln τ between (see the module docs for the citation
/// and the surrogate caveat).
pub(crate) fn theta_od(tau: f64) -> f64 {
    if tau.is_nan() || tau <= TAU_THIN {
        return 0.0;
    }
    if tau >= TAU_THICK {
        return 0.25;
    }
    let s = (tau / TAU_THIN).ln() / (TAU_THICK / TAU_THIN).ln();
    0.25 * s * s * (3.0 - 2.0 * s)
}

/// Nonlinear-diffusion face coupling D̃ (dimensionless, = D/h form)
/// between cells of removal σ_i, σ_j and width h.
fn dtilde(sig_i: f64, sig_j: f64, h: f64) -> f64 {
    let di = 1.0 / (3.0 * sig_i.max(SIGMA_FLOOR));
    let dj = 1.0 / (3.0 * sig_j.max(SIGMA_FLOOR));
    let fd = 2.0 * di * dj / (h * (di + dj));
    let tau = 0.5 * (sig_i + sig_j) * h;
    fd + theta_od(tau)
}

/// Per-group transport-consistent closure data.
#[derive(Default, Clone)]
pub(crate) struct GroupClosure {
    /// `[cell*6 + 2a+1]`: D̂ of the high face of the cell along axis a
    /// (interface D̂, or the partial-current outflow coefficient on a
    /// domain boundary). `[cell*6 + 2a]`: outflow coefficient of the
    /// low domain boundary (zero elsewhere).
    pub dh: Vec<f64>,
    /// Per-volume right-hand-side addition: sweep clamp defect and the
    /// declared boundary inflow.
    pub ex: Vec<f64>,
}

/// Build the closure of one group from its last sweep.
/// `faces[cell]` is the sweep's 13-slot accumulator (÷4π applied here).
/// Returns the number of guarded faces.
pub(crate) fn build_group(
    geom: &Geom,
    sigma: &(dyn Fn(usize) -> f64 + Sync),
    phi: &[f64],
    faces: &[[f64; 14]],
    out: &mut GroupClosure,
) -> u64 {
    let n = geom.n();
    let inv4pi = 1.0 / (4.0 * std::f64::consts::PI);
    let volume = geom.h[0] * geom.h[1] * geom.h[2];
    out.dh.resize(n * 6, 0.0);
    out.ex.resize(n, 0.0);
    let stride = geom.stride();
    let guards = AtomicU64::new(0);
    out.dh
        .par_chunks_mut(6)
        .with_min_len(MIN_LEN)
        .zip(out.ex.par_iter_mut().with_min_len(MIN_LEN))
        .enumerate()
        .for_each(|(i, (dh, ex))| {
            let co = geom.coord(i);
            let phi_i = phi[i];
            let mut local_guards = 0u64;
            let mut ex_v = -faces[i][13] * inv4pi / volume;
            dh.fill(0.0);
            for a in 0..3 {
                let na = geom.shape[a];
                let h = geom.h[a];
                // Low domain boundary.
                if co[a] == 0 && !geom.periodic[a] {
                    let out_p = faces[i][2 * a] * inv4pi;
                    let in_p = faces[i][2 * a + 6] * inv4pi;
                    dh[2 * a] = if phi_i > 0.0 && (out_p / phi_i).is_finite() {
                        out_p / phi_i
                    } else {
                        if out_p != 0.0 {
                            local_guards += 1;
                        }
                        0.0
                    };
                    ex_v += in_p / h;
                }
                // Periodic wrap: the sweep's inflow is a lagged plane
                // average, not the neighbor's outflow edge, so the two
                // cells' views of the face differ (the wrap is not
                // exactly conservative). The low-order face carries the
                // mean current; each cell's right-hand side takes the
                // difference to its own view, so the transport state
                // stays an exact low-order solution.
                if geom.periodic[a] && na == 1 {
                    // A one-cell periodic axis has no low-order face
                    // (the cell would couple to itself), but the lagged
                    // wrap still gives the sweep a nonzero net current
                    // through it: carry that on the right-hand side.
                    let net = (faces[i][2 * a + 1] - faces[i][2 * a + 7])
                        + (faces[i][2 * a] - faces[i][2 * a + 6]);
                    ex_v -= net * inv4pi / h;
                }
                if geom.periodic[a] && na > 1 && (co[a] == 0 || co[a] + 1 == na) {
                    let last = if co[a] == 0 {
                        i + (na - 1) * stride[a]
                    } else {
                        i
                    };
                    let first = last - (na - 1) * stride[a];
                    let j_last = faces[last][2 * a + 1] - faces[last][2 * a + 7];
                    let j_first = faces[first][2 * a + 6] - faces[first][2 * a];
                    ex_v -= (j_last - j_first) * inv4pi / (2.0 * h);
                }
                // High face.
                let neighbor = if co[a] + 1 < na {
                    Some(i + stride[a])
                } else if geom.periodic[a] && na > 1 {
                    Some(i - co[a] * stride[a])
                } else {
                    None
                };
                match neighbor {
                    Some(j) => {
                        let j_net = if co[a] + 1 < na {
                            (faces[i][2 * a + 1] - faces[j][2 * a]) * inv4pi
                        } else {
                            // Periodic wrap: the two cells' views of the
                            // face agree once the wrap planes converge;
                            // use their mean meanwhile.
                            0.5 * ((faces[i][2 * a + 1] - faces[i][2 * a + 7])
                                + (faces[j][2 * a + 6] - faces[j][2 * a]))
                                * inv4pi
                        };
                        let dt = dtilde(sigma(i), sigma(j), h);
                        let sum = phi_i + phi[j];
                        let d = -(j_net + dt * (phi[j] - phi_i)) / sum;
                        dh[2 * a + 1] = if sum > 1.0e-300 && d.is_finite() {
                            d
                        } else {
                            local_guards += 1;
                            0.0
                        };
                    }
                    None => {
                        if !geom.periodic[a] {
                            let out_p = faces[i][2 * a + 1] * inv4pi;
                            let in_p = faces[i][2 * a + 7] * inv4pi;
                            dh[2 * a + 1] = if phi_i > 0.0 && (out_p / phi_i).is_finite() {
                                out_p / phi_i
                            } else {
                                if out_p != 0.0 {
                                    local_guards += 1;
                                }
                                0.0
                            };
                            ex_v += in_p / h;
                        }
                    }
                }
            }
            *ex = ex_v;
            if local_guards > 0 {
                guards.fetch_add(local_guards, Ordering::Relaxed);
            }
        });
    guards.load(Ordering::Relaxed)
}

/// 7-point stencil: `off[cell*6 + 2a + s]` multiplies the low (s=0) or
/// high (s=1) neighbor along axis a.
struct Stencil {
    diag: Vec<f64>,
    off: Vec<f64>,
}

/// Everything the low-order solve needs.
pub(crate) struct LoProblem<'a> {
    pub geom: Geom,
    pub groups: usize,
    pub case_material: &'a [usize],
    pub sigma_eff: &'a [Vec<f64>],
    pub scatter_eff: &'a [Vec<f64>],
    /// `[cell][group]` fixed source.
    pub fixed_source: &'a [Vec<f64>],
    /// Quadrature-averaged source weight.
    pub source_scale: f64,
    pub closures: &'a [GroupClosure],
    /// First group of the upscatter-coupled block (`groups` if none).
    pub block_start: usize,
    /// Relative pass tolerance (loosened while the transport iterate is
    /// still far from converged).
    pub tol: f64,
}

fn assemble(p: &LoProblem, g: usize, st: &mut Stencil) -> bool {
    let geom = &p.geom;
    let n = geom.n();
    let stride = geom.stride();
    st.diag.resize(n, 0.0);
    st.off.resize(n * 6, 0.0);
    let groups = p.groups;
    let clo = &p.closures[g];
    let sigma = |c: usize| p.sigma_eff[p.case_material[c]][g];
    let ok = AtomicU64::new(0);
    st.diag
        .par_iter_mut()
        .with_min_len(MIN_LEN)
        .zip(st.off.par_chunks_mut(6))
        .enumerate()
        .for_each(|(i, (diag, off))| {
            let co = geom.coord(i);
            let mi = p.case_material[i];
            let mut dg = p.sigma_eff[mi][g] - p.scatter_eff[mi][g * groups + g];
            off.fill(0.0);
            for a in 0..3 {
                let na = geom.shape[a];
                let h = geom.h[a];
                // Low side.
                let lo_nb = if co[a] > 0 {
                    Some(i - stride[a])
                } else if geom.periodic[a] && na > 1 {
                    Some(i + (na - 1) * stride[a])
                } else {
                    None
                };
                match lo_nb {
                    Some(m) => {
                        // Face (m → i): this cell is the "j" side.
                        let dt = dtilde(sigma(m), sigma(i), h);
                        let d = clo.dh[m * 6 + 2 * a + 1];
                        dg += (dt + d) / h;
                        off[2 * a] = -(dt - d) / h;
                    }
                    None => {
                        if !geom.periodic[a] {
                            dg += clo.dh[i * 6 + 2 * a] / h;
                        }
                    }
                }
                // High side.
                let hi_nb = if co[a] + 1 < na {
                    Some(i + stride[a])
                } else if geom.periodic[a] && na > 1 {
                    Some(i - co[a] * stride[a])
                } else {
                    None
                };
                match hi_nb {
                    Some(j) => {
                        let dt = dtilde(sigma(i), sigma(j), h);
                        let d = clo.dh[i * 6 + 2 * a + 1];
                        dg += (dt - d) / h;
                        off[2 * a + 1] = -(dt + d) / h;
                    }
                    None => {
                        if !geom.periodic[a] {
                            dg += clo.dh[i * 6 + 2 * a + 1] / h;
                        }
                    }
                }
            }
            if dg <= 0.0 || dg.is_nan() || !dg.is_finite() {
                ok.fetch_add(1, Ordering::Relaxed);
            }
            *diag = dg;
        });
    ok.load(Ordering::Relaxed) == 0
}

/// Grids below this many cells run the vector kernels serially: the
/// per-iteration work is then far smaller than rayon's dispatch cost.
/// The choice depends only on the grid, never on the thread count, so
/// results stay bit-identical for any thread pool.
const PAR_MIN_CELLS: usize = 1 << 16;

/// `out[i] = f(i, out[i])`.
fn update(out: &mut [f64], f: impl Fn(usize, f64) -> f64 + Sync + Send) {
    if out.len() >= PAR_MIN_CELLS {
        out.par_iter_mut()
            .with_min_len(MIN_LEN)
            .enumerate()
            .for_each(|(i, o)| *o = f(i, *o));
    } else {
        for (i, o) in out.iter_mut().enumerate() {
            *o = f(i, *o);
        }
    }
}

/// Neighbor table `[cell][2a+s]` (low/high neighbor along axis a, with
/// periodic wrap); a missing neighbor points at the cell itself, whose
/// stencil coefficient is zero there.
fn neighbor_table(geom: &Geom) -> Vec<[u32; 6]> {
    let stride = geom.stride();
    (0..geom.n())
        .map(|i| {
            let co = geom.coord(i);
            let mut t = [i as u32; 6];
            for a in 0..3 {
                let na = geom.shape[a];
                if co[a] > 0 {
                    t[2 * a] = (i - stride[a]) as u32;
                } else if geom.periodic[a] && na > 1 {
                    t[2 * a] = (i + (na - 1) * stride[a]) as u32;
                }
                if co[a] + 1 < na {
                    t[2 * a + 1] = (i + stride[a]) as u32;
                } else if geom.periodic[a] && na > 1 {
                    t[2 * a + 1] = (i - co[a] * stride[a]) as u32;
                }
            }
            t
        })
        .collect()
}

fn matvec(nbr: &[[u32; 6]], st: &Stencil, x: &[f64], y: &mut [f64]) {
    update(y, |i, _| {
        let off = &st.off[i * 6..i * 6 + 6];
        let t = &nbr[i];
        st.diag[i] * x[i]
            + off[0] * x[t[0] as usize]
            + off[1] * x[t[1] as usize]
            + off[2] * x[t[2] as usize]
            + off[3] * x[t[3] as usize]
            + off[4] * x[t[4] as usize]
            + off[5] * x[t[5] as usize]
    });
}

/// Deterministic (thread-count independent) dot product of
/// `f(a_i, b_i)` terms over fixed-size chunks.
fn dot_with(a: &[f64], b: &[f64], f: impl Fn(f64, f64) -> f64 + Sync + Send) -> f64 {
    let chunk = |x: &[f64], y: &[f64]| x.iter().zip(y).map(|(u, v)| f(*u, *v)).sum::<f64>();
    if a.len() >= PAR_MIN_CELLS {
        let parts: Vec<f64> = a
            .par_chunks(RED_CHUNK)
            .zip(b.par_chunks(RED_CHUNK))
            .map(|(x, y)| chunk(x, y))
            .collect();
        parts.iter().sum()
    } else {
        a.chunks(RED_CHUNK)
            .zip(b.chunks(RED_CHUNK))
            .map(|(x, y)| chunk(x, y))
            .sum()
    }
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    dot_with(a, b, |u, v| u * v)
}

/// Right-preconditioned BiCGSTAB warm-started from `x` for an operator
/// given by `apply`, a preconditioner `precond` and a residual `norm`;
/// converges when `norm(r) ≤ tol·norm(b)`. Returns (iterations, ok).
fn bicgstab_with(
    b: &[f64],
    x: &mut [f64],
    tol: f64,
    mut apply: impl FnMut(&[f64], &mut [f64]),
    mut precond: impl FnMut(&[f64], &mut [f64]),
    norm: impl Fn(&[f64]) -> f64,
) -> (u64, bool) {
    let n = x.len();
    let mut r = vec![0.0; n];
    apply(x, &mut r);
    update(&mut r, |i, ri| b[i] - ri);
    let bnorm = norm(b).max(1e-300);
    let mut rn = norm(&r);
    if rn <= tol * bnorm {
        return (0, true);
    }
    let r0 = r.clone();
    let mut p = vec![0.0; n];
    let mut v = vec![0.0; n];
    let mut ph = vec![0.0; n];
    let mut s = vec![0.0; n];
    let mut sh = vec![0.0; n];
    let mut t = vec![0.0; n];
    let (mut rho, mut alpha, mut omega) = (1.0_f64, 1.0_f64, 1.0_f64);
    let max_it = 2000;
    for it in 1..=max_it {
        let rho_new = dot(&r0, &r);
        if rho_new == 0.0 || !rho_new.is_finite() {
            return (it, false);
        }
        let beta = (rho_new / rho) * (alpha / omega);
        update(&mut p, |i, pi| r[i] + beta * (pi - omega * v[i]));
        precond(&p, &mut ph);
        apply(&ph, &mut v);
        let r0v = dot(&r0, &v);
        if r0v == 0.0 || !r0v.is_finite() {
            return (it, false);
        }
        alpha = rho_new / r0v;
        update(&mut s, |i, _| r[i] - alpha * v[i]);
        if norm(&s) <= tol * bnorm {
            update(x, |i, xi| xi + alpha * ph[i]);
            return (it, true);
        }
        precond(&s, &mut sh);
        apply(&sh, &mut t);
        let tt = dot(&t, &t);
        if tt == 0.0 || !tt.is_finite() {
            return (it, false);
        }
        omega = dot(&t, &s) / tt;
        if omega == 0.0 || !omega.is_finite() {
            return (it, false);
        }
        update(x, |i, xi| xi + alpha * ph[i] + omega * sh[i]);
        update(&mut r, |i, _| s[i] - omega * t[i]);
        rn = norm(&r);
        if !rn.is_finite() {
            return (it, false);
        }
        if rn <= tol * bnorm {
            return (it, true);
        }
        rho = rho_new;
    }
    (max_it, false)
}

/// Jacobi-preconditioned BiCGSTAB for one group's stencil, on the
/// diagonal-scaled residual 2-norm. Returns (iterations, ok).
fn bicgstab(nbr: &[[u32; 6]], st: &Stencil, b: &[f64], x: &mut [f64], tol: f64) -> (u64, bool) {
    let d = &st.diag;
    bicgstab_with(
        b,
        x,
        tol,
        |v, out| matvec(nbr, st, v, out),
        |r, z| update(z, |i, _| r[i] / d[i]),
        |v| dot_with(v, d, |u, dd| (u / dd) * (u / dd)).sqrt(),
    )
}

/// The upscatter block solved as one coupled system: unknowns
/// `x[b*n + cell]`, operator = the group stencils minus the
/// between-group scatter, preconditioner = the exact per-cell
/// nb×nb energy-coupling block (spatial coupling is left to the
/// Krylov iteration). `None` when a cell block is singular.
fn block_bicgstab(
    nbr: &[[u32; 6]],
    stencils: &[&Stencil],
    p: &LoProblem,
    bs: usize,
    statics: &[Vec<f64>],
    phi: &mut [Vec<f64>],
    tol: f64,
) -> Option<(u64, bool)> {
    let n = phi[bs].len();
    let nb = stencils.len();
    let groups = p.groups;
    // Per-cell block inverse.
    let mut minv = vec![0.0_f64; n * nb * nb];
    let ok = AtomicU64::new(0);
    minv.par_chunks_mut(nb * nb)
        .with_min_len(256)
        .enumerate()
        .for_each(|(i, blk)| {
            let sc = &p.scatter_eff[p.case_material[i]];
            let mut a = vec![vec![0.0_f64; 2 * nb]; nb];
            for r in 0..nb {
                for c in 0..nb {
                    a[r][c] = if r == c {
                        stencils[r].diag[i]
                    } else {
                        -sc[(bs + c) * groups + (bs + r)]
                    };
                }
                a[r][nb + r] = 1.0;
            }
            // Gauss-Jordan with partial pivoting.
            for col in 0..nb {
                let piv = (col..nb)
                    .max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))
                    .unwrap();
                if a[piv][col].is_nan() || a[piv][col].abs() <= 1e-300 {
                    ok.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                a.swap(col, piv);
                let inv = 1.0 / a[col][col];
                for v in a[col].iter_mut() {
                    *v *= inv;
                }
                let pivot_row = a[col].clone();
                for (r, row) in a.iter_mut().enumerate() {
                    if r != col {
                        let f = row[col];
                        if f != 0.0 {
                            for (v, pv) in row.iter_mut().zip(&pivot_row) {
                                *v -= f * pv;
                            }
                        }
                    }
                }
            }
            for r in 0..nb {
                for c in 0..nb {
                    blk[r * nb + c] = a[r][nb + c];
                }
            }
        });
    if ok.load(Ordering::Relaxed) != 0 {
        return None;
    }
    let mut x: Vec<f64> = Vec::with_capacity(nb * n);
    let mut rhs: Vec<f64> = Vec::with_capacity(nb * n);
    for b in 0..nb {
        x.extend_from_slice(&phi[bs + b]);
        rhs.extend_from_slice(&statics[b]);
    }
    let apply = |v: &[f64], out: &mut [f64]| {
        for b in 0..nb {
            matvec(
                nbr,
                stencils[b],
                &v[b * n..(b + 1) * n],
                &mut out[b * n..(b + 1) * n],
            );
            update(&mut out[b * n..(b + 1) * n], |i, yi| {
                let sc = &p.scatter_eff[p.case_material[i]];
                let mut acc = yi;
                for b2 in 0..nb {
                    if b2 != b {
                        acc -= sc[(bs + b2) * groups + (bs + b)] * v[b2 * n + i];
                    }
                }
                acc
            });
        }
    };
    let precond = |r: &[f64], z: &mut [f64]| {
        for b in 0..nb {
            update(&mut z[b * n..(b + 1) * n], |i, _| {
                let row = &minv[(i * nb + b) * nb..(i * nb + b + 1) * nb];
                let mut acc = 0.0;
                for (b2, m) in row.iter().enumerate() {
                    acc += m * r[b2 * n + i];
                }
                acc
            });
        }
    };
    let norm = |v: &[f64]| -> f64 {
        (0..nb)
            .map(|b| {
                dot_with(&v[b * n..(b + 1) * n], &stencils[b].diag, |u, dd| {
                    (u / dd) * (u / dd)
                })
            })
            .sum::<f64>()
            .sqrt()
    };
    let res = bicgstab_with(&rhs, &mut x, tol, apply, precond, norm);
    if x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    for b in 0..nb {
        phi[bs + b].copy_from_slice(&x[b * n..(b + 1) * n]);
    }
    Some(res)
}

/// `f(i, &mut out[i])` for every i (serial on small grids).
fn each_mut(out: &mut [f64], f: impl Fn(usize, &mut f64) + Sync + Send) {
    if out.len() >= PAR_MIN_CELLS {
        out.par_iter_mut()
            .with_min_len(MIN_LEN)
            .enumerate()
            .for_each(|(i, o)| f(i, o));
    } else {
        for (i, o) in out.iter_mut().enumerate() {
            f(i, o);
        }
    }
}

/// Result of a low-order multigroup solve.
pub(crate) struct LoResult {
    pub iterations: u64,
    pub passes: u32,
}

/// Small type-II Anderson mixer over the flattened upscatter-block
/// unknowns (scaled per group), applied once per forward+backward
/// group-Gauss-Seidel cycle — the same composed map the transport
/// outer iteration accelerates.
struct Mixer {
    depth: usize,
    its: Vec<Vec<f64>>,
    res: Vec<Vec<f64>>,
}

impl Mixer {
    fn mix(&mut self, f: Vec<f64>, x: &[f64]) -> Vec<f64> {
        let r: Vec<f64> = f.iter().zip(x).map(|(a, b)| a - b).collect();
        self.its.push(f);
        self.res.push(r);
        while self.its.len() > self.depth + 1 {
            self.its.remove(0);
            self.res.remove(0);
        }
        let n = self.its.len();
        if n < 2 {
            return self.its[n - 1].clone();
        }
        let m = n - 1;
        let dr: Vec<Vec<f64>> = (0..m)
            .map(|i| {
                self.res[i + 1]
                    .iter()
                    .zip(&self.res[i])
                    .map(|(a, b)| a - b)
                    .collect()
            })
            .collect();
        let df: Vec<Vec<f64>> = (0..m)
            .map(|i| {
                self.its[i + 1]
                    .iter()
                    .zip(&self.its[i])
                    .map(|(a, b)| a - b)
                    .collect()
            })
            .collect();
        let rk = &self.res[n - 1];
        let mut a = vec![vec![0.0; m]; m];
        let mut b = vec![0.0; m];
        for i in 0..m {
            for j in 0..=i {
                let d = dot(&dr[i], &dr[j]);
                a[i][j] = d;
                a[j][i] = d;
            }
            b[i] = dot(&dr[i], rk);
        }
        let tr: f64 = (0..m).map(|i| a[i][i]).sum::<f64>().max(1e-300);
        for (i, row) in a.iter_mut().enumerate() {
            row[i] += 1e-12 * tr;
        }
        let Some(gamma) = solve_small(&mut a, &mut b) else {
            return self.its[n - 1].clone();
        };
        let mut out = self.its[n - 1].clone();
        for (i, g) in gamma.iter().enumerate() {
            for (o, d) in out.iter_mut().zip(&df[i]) {
                *o -= g * d;
            }
        }
        if out.iter().any(|v| !v.is_finite()) {
            return self.its[n - 1].clone();
        }
        out
    }
}

fn solve_small(a: &mut [Vec<f64>], b: &mut [f64]) -> Option<Vec<f64>> {
    let n = a.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            let pivot_row: Vec<f64> = a[col][col..n].to_vec();
            for (dst, src) in a[row][col..n].iter_mut().zip(&pivot_row) {
                *dst -= f * src;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Byte budget for cached per-group stencils; above it they are
/// re-assembled on each use.
const STENCIL_CACHE_BYTES: usize = 1 << 30;

/// Stencil access: cached per group, or re-assembled into a scratch.
struct StencilStore {
    cache_enabled: bool,
    cache: Vec<Option<Stencil>>,
    scratch: Stencil,
}

impl StencilStore {
    fn get(&mut self, p: &LoProblem, g: usize) -> Result<&Stencil, String> {
        let slot: &mut Stencil;
        if self.cache_enabled {
            if self.cache[g].is_none() {
                self.cache[g] = Some(Stencil {
                    diag: Vec::new(),
                    off: Vec::new(),
                });
            }
            let fresh = self.cache[g].as_ref().unwrap().diag.is_empty();
            slot = self.cache[g].as_mut().unwrap();
            if fresh && !assemble(p, g, slot) {
                return Err(format!("non-positive low-order diagonal in group {g}"));
            }
        } else {
            slot = &mut self.scratch;
            if !assemble(p, g, slot) {
                return Err(format!("non-positive low-order diagonal in group {g}"));
            }
        }
        Ok(&*slot)
    }
}

/// Gauss-Seidel over groups with a BiCGSTAB solve per group. The
/// downscatter-only groups converge in one forward pass; the
/// upscatter-coupled block (`block_start..`) is then iterated on its
/// own — alternating order, Anderson-accelerated, with the frozen
/// contribution of the groups above it held in a static right-hand
/// side — and a full forward pass verifies the whole structure.
/// `phi[g][cell]` is warm-start and result. `Err` if any group's
/// operator or solve is unusable (the caller then keeps its iterate).
pub(crate) fn solve_lo(p: &LoProblem, phi: &mut [Vec<f64>]) -> Result<LoResult, String> {
    let n = p.geom.n();
    let groups = p.groups;
    let mut store = StencilStore {
        cache_enabled: n * groups * 7 * 8 <= STENCIL_CACHE_BYTES,
        cache: (0..groups).map(|_| None).collect(),
        scratch: Stencil {
            diag: Vec::new(),
            off: Vec::new(),
        },
    };
    let nbr = neighbor_table(&p.geom);
    let mut rhs = vec![0.0; n];
    let mut total_iters = 0u64;
    let mut passes = 0u32;
    let pass_tol = p.tol.max(1.0e-10);
    let group_tol = (0.1 * pass_tol).max(1.0e-11);
    const MAX_BLOCK_PASSES: u32 = 400;
    const MAX_ROUNDS: u32 = 8;
    let bs = p.block_start.min(groups);
    let nb = groups - bs;
    let peaks: Vec<f64> = (0..groups)
        .map(|g| {
            phi[g]
                .iter()
                .fold(0.0_f64, |m, v| m.max(v.abs()))
                .max(1e-300)
        })
        .collect();
    let rel_change = |new: &[f64], old: &[f64], peak: f64| -> f64 {
        new.iter()
            .zip(old.iter())
            .map(|(a, b)| (a - b).abs() / a.abs().max(1e-6 * peak))
            .fold(0.0_f64, f64::max)
    };
    // static[b][cell]: fixed source + closure defect + scatter from the
    // groups above the block, for block group bs+b.
    let mut statics: Vec<Vec<f64>> = (0..nb).map(|_| vec![0.0; n]).collect();
    let flatten = |phi: &[Vec<f64>]| -> Vec<f64> {
        let mut v = Vec::with_capacity(nb * n);
        for g in bs..groups {
            v.extend(phi[g].iter().map(|x| x / peaks[g]));
        }
        v
    };

    // Whether the block scatters materially into groups above it;
    // if not, one forward pass plus the coupled block solve is exact.
    // A weak (< 5 % of a row) back-scatter is left to the outer
    // iteration: the transport state stays an exact fixed point of one
    // forward pass plus the block solve either way, and the verifying
    // pass costs O(G²·cells).
    let back_coupling = nb > 0
        && p.scatter_eff.iter().any(|row| {
            (bs..groups).any(|gf| {
                let all: f64 = (0..groups).map(|gt| row[gf * groups + gt].abs()).sum();
                let back: f64 = (0..bs).map(|gt| row[gf * groups + gt].abs()).sum();
                back > 0.05 * all
            })
        });
    for round in 0..MAX_ROUNDS {
        // Forward pass over the downscatter-only groups; the block's
        // static terms (fixed source, defect, scatter from above) are
        // refreshed on the way.
        let mut change = 0.0_f64;
        for g in 0..groups {
            let ex = &p.closures[g].ex;
            {
                let phi_ro: &[Vec<f64>] = phi;
                each_mut(&mut rhs, |i, r| {
                    let mi = p.case_material[i];
                    let sc = &p.scatter_eff[mi];
                    let mut v = p.fixed_source[i][g] * p.source_scale + ex[i];
                    for (gp, ph) in phi_ro.iter().enumerate().take(bs.min(groups)) {
                        if gp != g {
                            v += sc[gp * groups + g] * ph[i];
                        }
                    }
                    *r = v;
                });
                if g >= bs && nb > 0 {
                    statics[g - bs].copy_from_slice(&rhs);
                    continue;
                }
                if bs < groups {
                    each_mut(&mut rhs, |i, r| {
                        let sc = &p.scatter_eff[p.case_material[i]];
                        for (gp, ph) in phi_ro.iter().enumerate().skip(bs) {
                            if gp != g {
                                *r += sc[gp * groups + g] * ph[i];
                            }
                        }
                    });
                }
            }
            let st = store.get(p, g)?;
            let old = phi[g].clone();
            let (it, ok) = bicgstab(&nbr, st, &rhs, &mut phi[g], group_tol);
            total_iters += it;
            if !ok && phi[g].iter().any(|v| !v.is_finite()) {
                return Err(format!("low-order solve for group {g} broke down"));
            }
            change = change.max(rel_change(&phi[g], &old, peaks[g]));
        }
        passes += 1;
        if (round > 0 || nb == 0) && change < pass_tol || round + 1 == MAX_ROUNDS {
            break;
        }
        if nb == 0 {
            continue;
        }
        // Coupled solve of the upscatter block: one Krylov iteration
        // over all block groups with the exact per-cell energy-coupling
        // block as preconditioner. The Gauss-Seidel/Anderson loop below
        // is the fallback (uncached stencils, oversized block, breakdown).
        if store.cache_enabled && nb <= 16 {
            for g in bs..groups {
                store.get(p, g)?;
            }
            let stencils: Vec<&Stencil> = (bs..groups)
                .map(|g| store.cache[g].as_ref().unwrap())
                .collect();
            let res = block_bicgstab(&nbr, &stencils, p, bs, &statics, phi, group_tol);
            if let Some((it, ok)) = res {
                total_iters += it;
                passes += 1;
                if ok {
                    if back_coupling {
                        continue;
                    }
                    break;
                }
            }
        }
        // Iterate the upscatter block on its own.
        let mut mixer = Mixer {
            depth: 6,
            its: Vec::new(),
            res: Vec::new(),
        };
        let mut cycle_origin = flatten(phi);
        let mut cycle_change = 0.0_f64;
        for pass in 0..MAX_BLOCK_PASSES {
            passes += 1;
            let mut pass_change = 0.0_f64;
            for bi in 0..nb {
                let b = if pass % 2 == 0 { bi } else { nb - 1 - bi };
                let g = bs + b;
                {
                    let phi_ro: &[Vec<f64>] = phi;
                    let stat = &statics[b];
                    each_mut(&mut rhs, |i, r| {
                        let sc = &p.scatter_eff[p.case_material[i]];
                        let mut v = stat[i];
                        for (gp, ph) in phi_ro.iter().enumerate().skip(bs) {
                            if gp != g {
                                v += sc[gp * groups + g] * ph[i];
                            }
                        }
                        *r = v;
                    });
                }
                let st = store.get(p, g)?;
                let old = phi[g].clone();
                let (it, ok) = bicgstab(&nbr, st, &rhs, &mut phi[g], group_tol);
                total_iters += it;
                if !ok && phi[g].iter().any(|v| !v.is_finite()) {
                    return Err(format!("low-order solve for group {g} broke down"));
                }
                pass_change = pass_change.max(rel_change(&phi[g], &old, peaks[g]));
            }
            cycle_change = cycle_change.max(pass_change);
            if pass % 2 == 1 {
                if cycle_change < pass_tol {
                    break;
                }
                cycle_change = 0.0;
                let f = flatten(phi);
                let mixed = mixer.mix(f, &cycle_origin);
                // Keep the plain iterate wherever mixing would break
                // positivity.
                let positive = (bs..groups).enumerate().all(|(b, g)| {
                    phi[g]
                        .iter()
                        .zip(&mixed[b * n..(b + 1) * n])
                        .all(|(&x, &m)| x <= 0.0 || m > 0.0)
                });
                if positive {
                    for (b, g) in (bs..groups).enumerate() {
                        for (x, m) in phi[g].iter_mut().zip(&mixed[b * n..(b + 1) * n]) {
                            *x = m * peaks[g];
                        }
                    }
                }
                cycle_origin = flatten(phi);
            }
        }
    }
    Ok(LoResult {
        iterations: total_iters,
        passes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theta_od_has_the_published_limits_and_is_monotone() {
        assert_eq!(theta_od(0.0), 0.0);
        assert_eq!(theta_od(0.8), 0.0);
        assert_eq!(theta_od(14.0), 0.25);
        assert_eq!(theta_od(1.0e3), 0.25);
        let mut prev = 0.0;
        for k in 0..200 {
            let tau = 0.5 + 0.1 * k as f64;
            let t = theta_od(tau);
            assert!((0.0..=0.25).contains(&t));
            assert!(t >= prev - 1e-15, "theta not monotone at tau={tau}");
            prev = t;
        }
    }
}
