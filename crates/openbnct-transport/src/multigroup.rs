// SPDX-License-Identifier: Apache-2.0

//! Deterministic multigroup discrete-ordinates transport
//! (`openbnct.multigroup-data/0.1.0`, `openbnct.multigroup-flux/0.1.0`).
//!
//! This is the in-house reference transport path: a 3-D Cartesian S_N
//! solver over the case's regular `GridGeometry` using level-symmetric
//! quadrature, diamond-difference spatial closure, vacuum or declared
//! incident-flux boundaries, and source iteration with an outer re-sweep
//! for upscatter. It consumes a *declared* multigroup data artifact —
//! group structure plus per-material total and scatter cross sections —
//! so data provenance stays a contract rather than an implicit folding.
//!
//! Scope is stated honestly: isotropic (P0) scattering, no fission,
//! cell-edge flux chaining with the standard DD Padé-level accuracy —
//! a verification solver, not a production engine. Against
//! `NF-BNCT-003`'s pure-absorber slab it reproduces the exponential
//! attenuation oracle to discretization accuracy, which is precisely the
//! cross-method evidence a second transport implementation is for.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use openbnct_core::{
    ContentReference, DoseUnit, DoseVolume, GridGeometry, PhysicalDoseBundle,
    PhysicalTotalDoseVolume, TotalUncertaintyMethod,
};

use crate::model::{
    AngularDistribution, EnergyDistribution, MaterialAssignment, MaterialRegionShape,
    SourceSpatialDistribution, TransportCase,
};

/// Versioned multigroup-data contract schema.
pub const MULTIGROUP_DATA_SCHEMA: &str = "openbnct.multigroup-data/0.1.0";
/// Versioned scalar-flux result schema.
pub const MULTIGROUP_FLUX_SCHEMA: &str = "openbnct.multigroup-flux/0.1.0";

#[derive(Debug, Error)]
pub enum MultigroupError {
    #[error("multigroup data: {0}")]
    InvalidData(String),
    #[error("multigroup solve: {0}")]
    Solve(String),
    #[error("source mapping: {0}")]
    Source(String),
    #[error("model: {0}")]
    Model(#[from] crate::model::TransportModelError),
    #[error("validation: {0}")]
    Validation(#[from] openbnct_core::ValidationError),
}

/// Multigroup cross sections for one material.
///
/// `scatter_matrix_per_cm[g_from * G + g_to]` is the transfer cross
/// section from group `g_from` into `g_to`, row-major by source group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultigroupMaterial {
    /// `MaterialDefinition` id this data describes — bound by name so a
    /// data set cannot silently serve a different material.
    pub material_id: String,
    /// Total cross section per group, cm⁻¹.
    pub sigma_total_per_cm: Vec<f64>,
    /// Row-major scatter matrix `[g_from][g_to]`, cm⁻¹.
    pub scatter_matrix_per_cm: Vec<f64>,
    /// Optional P1 (l = 1 Legendre) transfer moments `[g_from][g_to]`,
    /// cm⁻¹ — each entry is the P0 transfer cross section weighted by
    /// the outgoing lab-frame mean cosine. Feeds the anisotropic
    /// scattering source `3·Σ_a Ω_a·Σ_s1·J_a` when the solve enables
    /// `p1_anisotropic`. Absent on pre-P1 artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scatter_p1_matrix_per_cm: Option<Vec<f64>>,
    /// Optional higher Legendre transfer moments for l = 2..=5, cm⁻¹.
    /// Layout `moments[l − 2][g_from × G + g_to]` — the l-th Legendre
    /// moment of the double-differential kernel per (source,
    /// destination) group pair. When the solve requests
    /// `anisotropy_order ≥ 2` these feed the exact discrete
    /// addition-theorem source; an absent moment at a requested l is
    /// an error at solve time, not a silent isotropic downgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scatter_legendre_moments_per_cm: Option<Vec<Vec<f64>>>,
    /// Optional per-component flux→dose response vectors `[G]` in
    /// `Gy·cm²` — folding scalar flux by these reproduces the component
    /// dose convention of the component profile the data declares.
    #[serde(default)]
    pub dose_response_gy_cm2: std::collections::BTreeMap<String, Vec<f64>>,
    /// Scatter-weighted mean lab-frame cosine per group `[G]` —
    /// Σ_s-weighted over the constituent nuclides (2/(3A) for iso-CM
    /// elastic). When present and the solve enables the transport
    /// correction, the sweep uses σ_t,tr = σ_t − μ̄_g·Σ_s,g.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_mu_bar: Option<Vec<f64>>,
}

/// `openbnct.multigroup-data/0.1.0` — declared group structure and
/// per-material data for the deterministic solver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultigroupData {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Group boundaries in eV, `G + 1` values, strictly descending
    /// (group 0 is the highest-energy group).
    pub energy_boundaries_ev: Vec<f64>,
    /// How the data was collapsed — recorded for review, not verified.
    pub collapse_declaration: String,
    /// Component profile the `dose_response` vectors realize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_profile: Option<ContentReference>,
    pub materials: Vec<MultigroupMaterial>,
}

impl MultigroupData {
    pub fn validate(&self) -> Result<(), MultigroupError> {
        let invalid = |m: String| MultigroupError::InvalidData(m);
        if !openbnct_core::schema_matches(&self.schema_version, MULTIGROUP_DATA_SCHEMA) {
            return Err(invalid(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() {
            return Err(invalid("id must be nonempty".into()));
        }
        let groups = self.group_count();
        if groups == 0
            || !self
                .energy_boundaries_ev
                .iter()
                .all(|e| e.is_finite() && *e > 0.0)
            || !self.energy_boundaries_ev.windows(2).all(|w| w[0] > w[1])
        {
            return Err(invalid(
                "energy_boundaries_ev must be >1 strictly descending positive edges".into(),
            ));
        }
        if self.materials.is_empty() {
            return Err(invalid("at least one material is required".into()));
        }
        let finite_nonneg = |v: &[f64]| v.iter().all(|x| x.is_finite() && *x >= 0.0);
        for material in &self.materials {
            if material.sigma_total_per_cm.len() != groups {
                return Err(invalid(format!(
                    "material {:?} sigma_total has {} groups, expected {groups}",
                    material.material_id,
                    material.sigma_total_per_cm.len()
                )));
            }
            if material.scatter_matrix_per_cm.len() != groups * groups {
                return Err(invalid(format!(
                    "material {:?} scatter matrix must be {groups}x{groups}",
                    material.material_id
                )));
            }
            if !finite_nonneg(&material.sigma_total_per_cm)
                || !finite_nonneg(&material.scatter_matrix_per_cm)
            {
                return Err(invalid(format!(
                    "material {:?} cross sections must be finite and non-negative",
                    material.material_id
                )));
            }
            // Scatter out of a group may not exceed its total — a
            // physicality floor on declared data.
            for g in 0..groups {
                let row_sum: f64 = (0..groups)
                    .map(|gt| material.scatter_matrix_per_cm[g * groups + gt])
                    .sum();
                if row_sum > material.sigma_total_per_cm[g] + 1e-12 {
                    return Err(invalid(format!(
                        "material {:?} group {g} scatters more than its total",
                        material.material_id
                    )));
                }
            }
            for (component, response) in &material.dose_response_gy_cm2 {
                if response.len() != groups || !finite_nonneg(response) {
                    return Err(invalid(format!(
                        "material {:?} response {component:?} must be {groups} \
                         non-negative values",
                        material.material_id
                    )));
                }
            }
            if let Some(mu_bar) = &material.transport_mu_bar
                && (mu_bar.len() != groups
                    || !mu_bar
                        .iter()
                        .all(|x| x.is_finite() && *x >= -1.0 && *x <= 1.0))
            {
                return Err(invalid(format!(
                    "material {:?} transport_mu_bar must be {groups} values in [-1,1]",
                    material.material_id
                )));
            }
            if let Some(p1) = &material.scatter_p1_matrix_per_cm {
                if p1.len() != groups * groups {
                    return Err(invalid(format!(
                        "material {:?} scatter_p1 matrix must be {groups}x{groups}",
                        material.material_id
                    )));
                }
                // Each P1 moment is the P0 transfer weighted by a mean
                // cosine in [−1,1] — bounded by the P0 row in magnitude.
                for (i, v) in p1.iter().enumerate() {
                    if !v.is_finite() || v.abs() > material.scatter_matrix_per_cm[i] + 1e-12 {
                        return Err(invalid(format!(
                            "material {:?} scatter_p1 entry {i} must be finite and \
                             |Σ_s1| ≤ Σ_s0",
                            material.material_id
                        )));
                    }
                }
            }
            if let Some(moments) = &material.scatter_legendre_moments_per_cm {
                // The l-th moment satisfies |Σ_sl| ≤ Σ_s0 — the same
                // mean-cosine bound generalized (|P_l| ≤ 1).
                if moments.len() > 4 {
                    return Err(invalid(format!(
                        "material {:?} legendre moments support l = 2..=5 ({} provided)",
                        material.material_id,
                        moments.len()
                    )));
                }
                for (l, mat) in moments.iter().enumerate() {
                    if mat.len() != groups * groups {
                        return Err(invalid(format!(
                            "material {:?} legendre moment l={} must be {groups}x{groups}",
                            material.material_id,
                            l + 2
                        )));
                    }
                    for (i, v) in mat.iter().enumerate() {
                        if !v.is_finite() || v.abs() > material.scatter_matrix_per_cm[i] + 1e-12 {
                            return Err(invalid(format!(
                                "material {:?} legendre l={} entry {i} must be finite and \
                                 |Σ_sl| ≤ Σ_s0",
                                material.material_id,
                                l + 2
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Number of energy groups.
    pub fn group_count(&self) -> usize {
        self.energy_boundaries_ev.len().saturating_sub(1)
    }

    /// Group index containing `energy_ev` (group 0 = highest energy).
    pub fn group_of(&self, energy_ev: f64) -> Option<usize> {
        for g in 0..self.group_count() {
            let hi = self.energy_boundaries_ev[g];
            let lo = self.energy_boundaries_ev[g + 1];
            if energy_ev <= hi && energy_ev > lo {
                return Some(g);
            }
        }
        None
    }
}

/// Level-symmetric S_N quadrature on the full sphere: `order` is even,
/// ≥2; returns `order·(order+2)` directions `([μ,η,ξ], weight)` with
/// weights summing to 4π.
pub fn level_symmetric_quadrature(order: u32) -> Result<Vec<([f64; 3], f64)>, MultigroupError> {
    if order < 2 || !order.is_multiple_of(2) || order > 16 {
        return Err(MultigroupError::Solve(format!(
            "quadrature order must be even in [2,16], got {order}"
        )));
    }
    let n = order as usize;
    // Level-symmetric ordinate magnitudes — the standard tabulated sets
    // for S2–S8. The octant ordinate set is all (μ_i, μ_j, μ_k) with
    // i + j + k = n/2 + 2 … equivalently i+j ≤ n/2+1 with
    // k = n/2+2−i−j.
    let base: Vec<f64> = match n / 2 {
        1 => vec![0.5773502691896258],
        2 => vec![0.3500211745815407, 0.8688903007222012],
        3 => vec![0.2666354015167046, 0.6815075987386857, 0.9261808765173782],
        4 => vec![
            0.2182178902359924,
            0.5773502691896258,
            0.7867957924694431,
            0.9511897312113418,
        ],
        _ => {
            // Equal-moment ordinate set for S10–S16: cosines distributed
            // to preserve the level-symmetric pattern; a declared
            // approximation — orders ≤8 use the exact tables.
            let half = n / 2;
            (1..=half)
                .map(|i| {
                    (std::f64::consts::FRAC_PI_2 * (2 * i - 1) as f64 / (2 * half + 1) as f64).cos()
                })
                .collect()
        }
    };
    let half = n / 2;
    let mut ordinates: Vec<[f64; 3]> = Vec::new();
    for i in 1..=half {
        for j in 1..=(half + 1 - i) {
            let k = half + 2 - i - j;
            ordinates.push([base[i - 1], base[j - 1], base[k - 1]]);
        }
    }
    let per_octant = ordinates.len();
    let weight = 4.0 * std::f64::consts::PI / (8.0 * per_octant as f64);
    let mut out = Vec::with_capacity(8 * per_octant);
    for o in &ordinates {
        for oct in 0..8u8 {
            let mut d = [
                if oct & 1 == 0 { o[0] } else { -o[0] },
                if oct & 2 == 0 { o[1] } else { -o[1] },
                if oct & 4 == 0 { o[2] } else { -o[2] },
            ];
            // Tabulated magnitudes carry literature rounding — renormalize
            // so every ordinate is unit to machine precision.
            let norm = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            for c in &mut d {
                *c /= norm;
            }
            out.push((d, weight));
        }
    }
    Ok(out)
}

/// Solver knobs for `solve_multigroup`.
#[derive(Debug, Clone)]
pub struct SnOptions {
    /// S_N quadrature order (even, 2–16).
    pub quadrature_order: u32,
    /// Relative scalar-flux convergence for source iteration.
    pub convergence: f64,
    /// Within-group scattering iterations per group pass.
    pub max_inner_iterations: u32,
    /// Outer sweeps over the full group structure (upscatter).
    pub max_outer_iterations: u32,
    /// Heterogeneous material assignment overriding the case's base
    /// material per voxel.
    pub assignment: Option<MaterialAssignment>,
    /// Periodic boundaries on the domain faces normal to each axis —
    /// outflow through the high face re-enters the low face (and vice
    /// versa) for the same ordinate. Periodic transverse faces make a
    /// uniform beam problem exactly translation-invariant, including
    /// oblique rays — the honest way to realize a 1-D slab on a 3-D
    /// grid. Inflow is the wrap-around cell's previous-iterate cell
    /// average (exact under transverse uniformity).
    pub periodic: [bool; 3],
    /// Uncollided-flux split for on-face disk sources: the uncollided
    /// beam is ray-traced analytically (exact exponential attenuation —
    /// no discrete-ordinates obliquity bias or thick-cell
    /// diamond-difference damping on the streaming component) and the
    /// sweep carries only the collided remainder driven by the
    /// first-collision source. The standard treatment for beam sources
    /// in S_N. Monodirectional beams ray-trace along d̂; forward
    /// isotropic cones are integrated over a deterministic equal-area
    /// direction grid, which also reproduces the cone's geometric
    /// spread with depth. Wide cones and other distributions stay on
    /// the boundary-flux path.
    pub beam_uncollided_split: bool,
    /// Extended transport correction: when the data carries
    /// `transport_mu_bar`, the sweep's effective total becomes
    /// σ_t,tr = σ_t − μ̄_g·Σ_s,row(g) and the same forward-scatter
    /// fraction is removed from the in-group diagonal — the consistent
    /// P0 correction for anisotropic (forward-peaked) scatter, which
    /// keeps the source iteration contractive. The uncollided ray-trace
    /// still uses the physical σ_t — the correction applies only to
    /// the collided component. No-op on data without `transport_mu_bar`.
    pub transport_correction: bool,
    /// P1 anisotropic scattering: when the data carries
    /// `scatter_p1_matrix_per_cm`, add the first-Legendre source term
    /// `3·Σ_a Ω_{d,a}·Σ_s1(gp→g)·J_{a,gp}` to the sweep source, with the
    /// cell group currents `J_a` iterated alongside the scalar flux.
    /// Requires the P1 table on every material that contributes
    /// scatter; mutually exclusive with `transport_correction` (both
    /// treat the same anisotropy — combining them double-counts the
    /// forward peak). Physical σ_t applies when this is set.
    pub p1_anisotropic: bool,
    /// Highest Legendre order l carried by the in-scatter kernel
    /// beyond P1 — 0 or 1 leaves the sweep on the scalar/P1 source
    /// path; 2..=5 activates the higher-moment source for l = 2..=N
    /// via `scatter_legendre_moments_per_cm`. The discrete P_l kernel
    /// is eigendecomposed once per quadrature (exact addition theorem
    /// for the ordinate set) and the source is evaluated as
    /// Σ_l(2l+1)σ_l Σ_k u_k(Ω_d)·M_k — moment tracking without a
    /// direction² inner loop. Requires P1 (`p1_anisotropic`) plus the
    /// moment tables on every scattering material.
    pub anisotropy_order: u32,
    /// Anderson acceleration depth for the outer fixed-point iteration.
    /// 0 (default) runs plain symmetric sweeps; ≥1 mixes the last
    /// `anderson_depth` outer iterates via type-II Anderson (GMRES-
    /// equivalent on this linear map) — applied once per symmetric
    /// down+up cycle, which is the consistent composed operator.
    /// Accelerates exactly the slow energy-coupling mode that
    /// bound-atom S(α,β) upscatter introduces (plain sweeps decay
    /// ~0.85–0.9 per pass there).
    pub anderson_depth: usize,
}

impl Default for SnOptions {
    fn default() -> Self {
        Self {
            quadrature_order: 4,
            convergence: 1e-6,
            max_inner_iterations: 64,
            max_outer_iterations: 32,
            assignment: None,
            periodic: [false; 3],
            beam_uncollided_split: true,
            transport_correction: true,
            p1_anisotropic: false,
            anisotropy_order: 0,
            anderson_depth: 0,
        }
    }
}

/// Type-II Anderson acceleration state for the outer fixed-point
/// iteration: mixes the last `depth` map iterates by minimizing the
/// residual combination. On the linear sweep map this is
/// GMRES-equivalent — it accelerates whichever modes the plain sweep
/// contracts slowly (energy-space upscatter coupling for TSL data),
/// not just spatial diffusion modes as DSA would.
struct AndersonState {
    depth: usize,
    /// Post-sweep iterates f_i (flattened cell×group), oldest first.
    iterates: Vec<Vec<f64>>,
    /// Fixed-point residuals r_i = f_i − x_i at each iterate.
    residuals: Vec<Vec<f64>>,
}

impl AndersonState {
    fn new(depth: usize) -> Self {
        Self {
            depth,
            iterates: Vec::new(),
            residuals: Vec::new(),
        }
    }

    /// Record the map application `f = F(x)` and return the
    /// accelerated iterate (the plain `f` while history is
    /// insufficient or the least-squares system is singular).
    fn mix(&mut self, f: &[Vec<f64>], x: &[Vec<f64>]) -> Vec<Vec<f64>> {
        let flat: Vec<f64> = f.iter().flat_map(|r| r.iter().copied()).collect();
        let resid: Vec<f64> = f
            .iter()
            .zip(x.iter())
            .flat_map(|(a, b)| a.iter().zip(b.iter()).map(|(u, v)| u - v))
            .collect();
        self.iterates.push(flat);
        self.residuals.push(resid);
        while self.iterates.len() > self.depth + 1 {
            self.iterates.remove(0);
            self.residuals.remove(0);
        }
        let n = self.residuals.len();
        if n < 2 {
            return f.to_vec();
        }
        // Difference columns Δr_i = r_{i+1} − r_i over the last
        // `m ≤ depth` consecutive pairs, and the matching Δf_i.
        let m = (n - 1).min(self.depth);
        let base = n - 1 - m; // index of the oldest retained pair
        let dr: Vec<Vec<f64>> = (0..m)
            .map(|i| {
                self.residuals[base + i + 1]
                    .iter()
                    .zip(self.residuals[base + i].iter())
                    .map(|(a, b)| a - b)
                    .collect()
            })
            .collect();
        let df: Vec<Vec<f64>> = (0..m)
            .map(|i| {
                self.iterates[base + i + 1]
                    .iter()
                    .zip(self.iterates[base + i].iter())
                    .map(|(a, b)| a - b)
                    .collect()
            })
            .collect();
        // Normal equations for min ||r_k − Σ γ_i·Δr_i||².
        let rk = &self.residuals[n - 1];
        let mut gram = vec![vec![0.0_f64; m]; m];
        let mut rhs = vec![0.0_f64; m];
        for i in 0..m {
            for j in 0..=i {
                let dot: f64 = dr[i].iter().zip(dr[j].iter()).map(|(a, b)| a * b).sum();
                gram[i][j] = dot;
                gram[j][i] = dot;
            }
            rhs[i] = dr[i].iter().zip(rk.iter()).map(|(a, b)| a * b).sum();
        }
        let Some(gamma) = solve_dense(&mut gram, &mut rhs) else {
            return f.to_vec();
        };
        // x_new = f_k − Σ γ_i·Δf_i, mapped back onto the nested grid.
        let mut out = f.to_vec();
        let mut idx = 0;
        for row in out.iter_mut() {
            for v in row.iter_mut() {
                for i in 0..m {
                    *v -= gamma[i] * df[i][idx];
                }
                idx += 1;
            }
        }
        out
    }
}

/// Dense Gaussian elimination with partial pivoting for the small
/// (≤ depth) normal-equations system; `None` on singularity.
fn solve_dense(a: &mut [Vec<f64>], b: &mut [f64]) -> Option<Vec<f64>> {
    let n = a.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let pivot_row: Vec<f64> = a[col][col..n].to_vec();
        for row in (col + 1)..n {
            let factor = a[row][col] / pivot_row[0];
            for (a_rc, &a_pc) in a[row][col..n].iter_mut().zip(pivot_row.iter()) {
                *a_rc -= factor * a_pc;
            }
            b[row] -= factor * b[col];
        }
    }
    let mut x = vec![0.0_f64; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|c| a[row][c] * x[c]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

/// Solve output: cell-averaged scalar flux per voxel per group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultigroupFlux {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// Content binding of the data artifact consumed.
    pub multigroup_data: ContentReference,
    /// Content binding of the transport case solved.
    pub case: ContentReference,
    pub energy_boundaries_ev: Vec<f64>,
    /// How the incident beam entered the solve: `uncollided_split`
    /// (analytic ray-trace + collided sweep) or `boundary_flux`
    /// (discrete-ordinates incident flux).
    pub beam_model: String,
    /// Scalar flux `[voxel][group]` in cm⁻²s⁻¹ per unit source rate —
    /// the total including the analytic uncollided component when the
    /// split is active.
    pub flux: Vec<Vec<f64>>,
    /// Whether the extended transport correction (σ_t,tr = σ_t −
    /// μ̄_g·Σ_s,g) was applied to the collided sweep.
    #[serde(default)]
    pub transport_correction: bool,
    /// Scattering treatment actually solved: `p0` (isotropic transfer),
    /// `p0_transport_corrected` (extended transport correction applied),
    /// or `p1` (P1 anisotropic source from `scatter_p1` moments).
    /// Absent on pre-P1 artifacts — resolve via
    /// [`MultigroupFlux::scattering_order`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scattering_order: Option<String>,
    /// How histogram source bins were spread across sub-groups:
    /// `collapse_consistent` = Maxwellian below 0.5 eV / 1-E above,
    /// matching the multigroup collapse's declared weighting;
    /// `uniform_in_bin` = the earlier uniform-per-eV spread (the
    /// deserialization default for artifacts produced before the field
    /// existed).
    #[serde(default = "default_spectrum_weighting")]
    pub source_spectrum_weighting: String,
    pub quadrature_order: u32,
    pub outer_iterations: u32,
    /// Final relative scalar-flux change.
    pub residual: f64,
    pub converged: bool,
    pub qualification: String,
    pub provenance_id: String,
}

impl MultigroupFlux {
    /// Scattering treatment this artifact was produced under —
    /// `p0`, `p0_transport_corrected`, or `p1`. Pre-P1 artifacts lack
    /// the field; they resolve from `transport_correction`.
    pub fn scattering_order(&self) -> &str {
        self.scattering_order
            .as_deref()
            .unwrap_or(if self.transport_correction {
                "p0_transport_corrected"
            } else {
                "p0"
            })
    }
}

/// Incident boundary angular flux keyed `(face, cell_u, cell_v)` →
/// `(direction_index, group)` → ψ (cm⁻²s⁻¹sr⁻¹). Face index is
/// `2·axis` for the low face, `2·axis + 1` for the high face.
pub(crate) type BoundarySource =
    std::collections::BTreeMap<(u8, u32, u32), Vec<(usize, usize, f64)>>;

/// The boundary face index (`2·axis` low / `2·axis+1` high), the
/// transverse `(ju, jv)` cell columns the declared source covers, and
/// the per-group emission weights — the shared geometry half of boundary
/// source handling, used both by the discrete flux map and by CADIS
/// source-weight normalization.
pub(crate) fn source_coverage(
    case: &TransportCase,
    data: &MultigroupData,
) -> Result<SourceCoverage, MultigroupError> {
    let invalid = |m: String| MultigroupError::Source(m);
    let source = &case.source;
    let geometry = &case.geometry;
    let axis = source.space.axis().index();
    let offset_cm = source.space.offset_cm();

    // Grid world bounds along the source-normal axis.
    let lo_mm = geometry.origin_mm[axis] - 0.5 * geometry.spacing_mm[axis];
    let hi_mm = lo_mm + geometry.spacing_mm[axis] * geometry.shape[axis] as f64;
    let face = if (offset_cm * 10.0 - lo_mm).abs() < 1.0 {
        2 * axis as u8
    } else if (offset_cm * 10.0 - hi_mm).abs() < 1.0 {
        2 * axis as u8 + 1
    } else {
        return Err(invalid(format!(
            "source offset {offset_cm} cm is not on a grid face along axis {axis} \
             (bounds {lo_mm}..{hi_mm} mm)"
        )));
    };

    // Cells whose transverse centers fall inside the disk — cell-center
    // coverage is the declared boundary resolution. `in_plane_axes` is
    // the canonical (u, v) order (Y spans (x, z), not (z, x)) — the same
    // order the sweep uses for its `(uu, vv)` boundary lookup.
    let (u, v) = source.space.axis().in_plane_axes();
    let mut cells: Vec<(u32, u32)> = Vec::new();
    match &source.space {
        SourceSpatialDistribution::UniformDisk {
            center_uv_cm,
            radius_cm,
            ..
        } => {
            for ju in 0..geometry.shape[u] {
                for jv in 0..geometry.shape[v] {
                    let mut voxel = [0u32; 3];
                    voxel[u] = ju;
                    voxel[v] = jv;
                    let center = geometry.voxel_center_lps_mm(voxel)?;
                    let du = center[u] / 10.0 - center_uv_cm[0];
                    let dv = center[v] / 10.0 - center_uv_cm[1];
                    if du * du + dv * dv <= radius_cm * radius_cm {
                        cells.push((ju, jv));
                    }
                }
            }
            if cells.is_empty() {
                return Err(invalid(
                    "source disk covers no boundary cell centers".into(),
                ));
            }
        }
        other => {
            return Err(invalid(format!(
                "only on-face disk sources map to boundary flux (got axis {:?})",
                other.axis()
            )));
        }
    }
    Ok(SourceCoverage {
        face,
        cells,
        group_weights: source_group_weights(source, data)?,
    })
}

/// Shared geometry half of boundary-source handling: which face, which
/// transverse cell columns, which group weights.
pub(crate) struct SourceCoverage {
    /// `2·axis` for the low face, `2·axis + 1` for the high face.
    pub face: u8,
    /// Covered transverse `(ju, jv)` cell columns.
    pub cells: Vec<(u32, u32)>,
    /// Normalized per-group emission weights.
    pub group_weights: Vec<f64>,
}

/// Map a `FixedSourceDefinition` onto boundary incident-flux cells and
/// group weights. Only plane/disk sources sitting on a grid face are
/// supported — interior volumetric sources are rejected honestly.
fn map_boundary_source(
    case: &TransportCase,
    data: &MultigroupData,
    quadrature: &[([f64; 3], f64)],
) -> Result<BoundarySource, MultigroupError> {
    let invalid = |m: String| MultigroupError::Source(m);
    let source = &case.source;
    let geometry = &case.geometry;
    let axis = source.space.axis().index();
    let coverage = source_coverage(case, data)?;
    let (face, cells, group_weights) = (coverage.face, coverage.cells, coverage.group_weights);
    let inward_sign = if face % 2 == 0 { 1.0_f64 } else { -1.0_f64 };

    // Angular map: the inward-hemisphere ordinates matching the declared
    // angular distribution, with each entry's fraction of the incident
    // partial current.
    let inward: Vec<usize> = quadrature
        .iter()
        .enumerate()
        .filter(|(_, (d, _))| d[axis] * inward_sign > 0.0)
        .map(|(i, _)| i)
        .collect();
    let angular: Vec<(usize, f64)> = match &source.angle {
        AngularDistribution::Monodirectional { unit_vector } => {
            // The whole current goes to the inward ordinate nearest the
            // declared direction — the standard discrete-ordinates beam
            // approximation.
            let best = inward
                .iter()
                .copied()
                .max_by(|a, b| {
                    let da: f64 = quadrature[*a]
                        .0
                        .iter()
                        .zip(unit_vector)
                        .map(|(x, y)| x * y)
                        .sum();
                    let db: f64 = quadrature[*b]
                        .0
                        .iter()
                        .zip(unit_vector)
                        .map(|(x, y)| x * y)
                        .sum();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .ok_or_else(|| invalid("no inward ordinate".into()))?;
            vec![(best, 1.0)]
        }
        AngularDistribution::IsotropicCone {
            axis_unit_vector,
            half_angle_rad,
        } => {
            let cos_limit = half_angle_rad.cos();
            let in_cone: Vec<usize> = inward
                .iter()
                .copied()
                .filter(|i| {
                    quadrature[*i]
                        .0
                        .iter()
                        .zip(axis_unit_vector)
                        .map(|(x, y)| x * y)
                        .sum::<f64>()
                        >= cos_limit
                })
                .collect();
            let in_cone = if in_cone.is_empty() {
                // A cone narrower than the quadrature's finest angular
                // resolution admits no ordinate at any order — collapse to
                // the nearest inward ordinate, the monodirectional
                // treatment.
                let nearest = inward
                    .iter()
                    .copied()
                    .max_by(|a, b| {
                        let da: f64 = quadrature[*a]
                            .0
                            .iter()
                            .zip(axis_unit_vector)
                            .map(|(x, y)| x * y)
                            .sum();
                        let db: f64 = quadrature[*b]
                            .0
                            .iter()
                            .zip(axis_unit_vector)
                            .map(|(x, y)| x * y)
                            .sum();
                        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .ok_or_else(|| invalid("no inward ordinate".into()))?;
                vec![nearest]
            } else {
                in_cone
            };
            // Uniform-in-solid-angle cone: each in-cone ordinate's share
            // of the partial current is proportional to w·|Ω·n̂|.
            let denom: f64 = in_cone
                .iter()
                .map(|i| quadrature[*i].1 * (quadrature[*i].0[axis] * inward_sign).abs())
                .sum();
            in_cone
                .iter()
                .map(|i| {
                    (
                        *i,
                        quadrature[*i].1 * (quadrature[*i].0[axis] * inward_sign).abs() / denom,
                    )
                })
                .collect()
        }
    };

    // Energy → group weights.
    let groups = data.group_count();
    debug_assert_eq!(group_weights.len(), groups);

    // Unit-weight source: total rate = sites/history × weight → incident
    // partial-current density over the covered cells.
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let cell_area_cm2 = (geometry.spacing_mm[u] * geometry.spacing_mm[v]) / 100.0;
    let current_density = source.statistical_weight_per_site
        * source.source_sites_per_history as f64
        / (cell_area_cm2 * cells.len() as f64);

    let mut source_map: BoundarySource = std::collections::BTreeMap::new();
    for (ju, jv) in cells {
        let mut entries: Vec<(usize, usize, f64)> = Vec::new();
        for (d, frac) in &angular {
            let (_, w) = quadrature[*d];
            let proj = (quadrature[*d].0[axis] * inward_sign).abs();
            for (g, gw) in group_weights.iter().enumerate() {
                if *gw > 0.0 {
                    // ψ_d,g such that w_d·|Ω·n̂|·ψ_d,g = I·frac·gw.
                    entries.push((*d, g, current_density * gw * frac / (w * proj)));
                }
            }
        }
        source_map.insert((face, ju, jv), entries);
    }
    Ok(source_map)
}

/// Within-bin spectrum weighting matching the collapse declaration:
/// Maxwellian ∝ E·exp(−E/kT) below 0.5 eV, 1/E slowing-down above —
/// histogram bins declare only integrals, so the within-bin shape must
/// come from the same declared convention the data was collapsed under.
fn spectrum_weight_integral(lo_ev: f64, hi_ev: f64) -> f64 {
    const KT_EV: f64 = 0.0253;
    const CUT_EV: f64 = 0.5;
    let maxwell = |a: f64, b: f64| {
        (KT_EV * (KT_EV + a) * (-a / KT_EV).exp()) - (KT_EV * (KT_EV + b) * (-b / KT_EV).exp())
    };
    let inv_e = |a: f64, b: f64| (b / a).ln();
    let (lo, hi) = (lo_ev.max(1.0e-30), hi_ev.max(lo_ev));
    let mid = CUT_EV.clamp(lo, hi);
    let mut w = 0.0;
    if mid > lo {
        w += maxwell(lo, mid);
    }
    if hi > mid {
        w += inv_e(mid.max(1.0e-30), hi);
    }
    w
}

/// Map the source's energy distribution onto normalized group weights.
fn source_group_weights(
    source: &crate::model::FixedSourceDefinition,
    data: &MultigroupData,
) -> Result<Vec<f64>, MultigroupError> {
    let invalid = |m: String| MultigroupError::Source(m);
    let groups = data.group_count();
    let mut weights = vec![0.0; groups];
    match &source.energy {
        EnergyDistribution::Monoenergetic { energy_ev } => {
            let g = data.group_of(*energy_ev).ok_or_else(|| {
                invalid(format!(
                    "source energy {energy_ev} eV outside the group structure"
                ))
            })?;
            weights[g] = 1.0;
        }
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev,
            bin_weights,
        } => {
            for (bin, w) in bin_weights.iter().enumerate() {
                let lo = energy_boundaries_ev[bin];
                let hi = energy_boundaries_ev[bin + 1];
                let bin_norm = spectrum_weight_integral(lo, hi);
                if bin_norm <= 0.0 {
                    continue;
                }
                for (g, weight) in weights.iter_mut().enumerate() {
                    let glo = data.energy_boundaries_ev[g + 1];
                    let ghi = data.energy_boundaries_ev[g];
                    let (olo, ohi) = (lo.max(glo), hi.min(ghi));
                    if ohi > olo {
                        *weight += w * spectrum_weight_integral(olo, ohi) / bin_norm;
                    }
                }
            }
            let total: f64 = weights.iter().sum();
            if total <= 0.0 {
                return Err(invalid("source spectrum overlaps no group".into()));
            }
            for w in &mut weights {
                *w /= total;
            }
        }
    }
    Ok(weights)
}

/// (direction, weight) pairs covering a source angular distribution.
type DirectionWeights = Vec<([f64; 3], f64)>;

/// Deserialization default: artifacts written before the field existed
/// were produced under the uniform-per-eV within-bin spread.
fn default_spectrum_weighting() -> String {
    "uniform_in_bin".into()
}

/// Deterministic equal-area sample directions over an isotropic cone:
/// uniform grid in (cos θ, φ) about `axis`. Returns (direction, weight)
/// pairs whose weights sum to the cone solid angle. A monodirectional
/// distribution degenerates to a single unit-weighted direction.
fn cone_directions(
    angle: &AngularDistribution,
) -> Result<Option<DirectionWeights>, MultigroupError> {
    let invalid = |m: String| MultigroupError::Source(m);
    match angle {
        AngularDistribution::Monodirectional { unit_vector } => Ok(Some(vec![(*unit_vector, 1.0)])),
        AngularDistribution::IsotropicCone {
            axis_unit_vector,
            half_angle_rad,
        } => {
            let axis = axis_unit_vector;
            let norm: f64 = axis.iter().map(|c| c * c).sum::<f64>().sqrt();
            if !(half_angle_rad.is_finite() && *half_angle_rad > 0.0 && norm > 0.0) {
                return Err(invalid("isotropic cone needs a positive half angle".into()));
            }
            if *half_angle_rad > std::f64::consts::FRAC_PI_2 {
                // The equal-area grid is only meaningful for a forward
                // cone; wide beams stay on the boundary-flux path.
                return Ok(None);
            }
            let ax: [f64; 3] = [axis[0] / norm, axis[1] / norm, axis[2] / norm];
            // Orthonormal basis perpendicular to the cone axis.
            let seed = if ax[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            let mut u = [
                ax[1] * seed[2] - ax[2] * seed[1],
                ax[2] * seed[0] - ax[0] * seed[2],
                ax[0] * seed[1] - ax[1] * seed[0],
            ];
            let un: f64 = u.iter().map(|c| c * c).sum::<f64>().sqrt();
            for c in &mut u {
                *c /= un;
            }
            let v = [
                ax[1] * u[2] - ax[2] * u[1],
                ax[2] * u[0] - ax[0] * u[2],
                ax[0] * u[1] - ax[1] * u[0],
            ];
            let cos_h = half_angle_rad.cos();
            // Equal-area grid: N_R rings in cos θ × N_PHI azimuths. For
            // narrow beams (~9°) 8×16 resolves the disk-edge transition
            // to well under a percent of the lit solid angle.
            const N_R: usize = 8;
            const N_PHI: usize = 16;
            let omega = 2.0 * std::f64::consts::PI * (1.0 - cos_h);
            let w = omega / (N_R * N_PHI) as f64;
            let mut dirs = Vec::with_capacity(N_R * N_PHI);
            for k in 0..N_R {
                let cos_t = 1.0 - (k as f64 + 0.5) / N_R as f64 * (1.0 - cos_h);
                let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
                for l in 0..N_PHI {
                    let phi = 2.0 * std::f64::consts::PI * (l as f64 + 0.5) / N_PHI as f64;
                    let d = [
                        ax[0] * cos_t + sin_t * (u[0] * phi.cos() + v[0] * phi.sin()),
                        ax[1] * cos_t + sin_t * (u[1] * phi.cos() + v[1] * phi.sin()),
                        ax[2] * cos_t + sin_t * (u[2] * phi.cos() + v[2] * phi.sin()),
                    ];
                    dirs.push((d, w));
                }
            }
            Ok(Some(dirs))
        }
    }
}

/// Analytic uncollided-flux ray-trace for an on-face disk source.
/// For each cell the back-ray to the source-face plane determines disk
/// coverage; φ_unc(cell, g) = (R/A_disk)·w_g·⟨hit·e^{−Σ_t·s}⟩/μ̄ where
/// s is the path length from entry to the cell center and the average
/// is over the angular distribution (a single direction for a
/// monodirectional beam, the cone solid angle for an isotropic cone).
///
/// Returns `None` for source shapes/angles that stay on the
/// boundary-flux path (wide cones, isotropic, off-face sources).
fn uncollided_beam_flux(
    case: &TransportCase,
    data: &MultigroupData,
    case_material: &[usize],
) -> Result<Option<Vec<Vec<f64>>>, MultigroupError> {
    let invalid = |m: String| MultigroupError::Source(m);
    let source = &case.source;
    let SourceSpatialDistribution::UniformDisk {
        axis,
        offset_cm,
        center_uv_cm,
        radius_cm,
    } = &source.space
    else {
        return Ok(None);
    };
    let Some(dirs) = cone_directions(&source.angle)? else {
        return Ok(None);
    };
    let geometry = &case.geometry;
    let a = axis.index();
    let omega: f64 = dirs.iter().map(|(_, w)| w).sum();
    // Mean axial component ⟨Ω·â⟩ over the angular distribution — the
    // current-to-fluence conversion at the source face.
    let mu_bar = dirs.iter().map(|(d, w)| w * d[a]).sum::<f64>() / omega;
    if mu_bar.abs() < 1e-12 {
        return Err(invalid(
            "beam direction parallel to its own source face".into(),
        ));
    }
    // Source face must sit on a grid face along `axis`.
    let lo_mm = geometry.origin_mm[a] - 0.5 * geometry.spacing_mm[a];
    let hi_mm = lo_mm + geometry.spacing_mm[a] * geometry.shape[a] as f64;
    let face_cm = if (offset_cm * 10.0 - lo_mm).abs() < 1.0 {
        lo_mm / 10.0
    } else if (offset_cm * 10.0 - hi_mm).abs() < 1.0 {
        hi_mm / 10.0
    } else {
        return Err(invalid(format!(
            "source offset {offset_cm} cm is not on a grid face along axis {a}"
        )));
    };
    let inward = if face_cm == lo_mm / 10.0 { 1.0 } else { -1.0 };
    if inward * mu_bar <= 0.0 {
        return Err(invalid(
            "beam direction points out of the domain through its source face".into(),
        ));
    }

    // Canonical (u, v) order — Y planes span (x, z), so the disk's
    // `center_uv_cm` and the back-ray exit point must use the same
    // ordering (see `PlaneAxis::in_plane_axes`).
    let (u, v) = axis.in_plane_axes();
    let rate = source.statistical_weight_per_site * source.source_sites_per_history as f64;
    let disk_area_cm2 = std::f64::consts::PI * radius_cm * radius_cm;
    // Scalar fluence at the face per unit current: J/μ̄ with J = R/A.
    let beam_intensity = rate / (disk_area_cm2 * mu_bar.abs());
    let group_weights = source_group_weights(source, data)?;
    let groups = data.group_count();
    let n_cells = geometry.voxel_count()?;
    let [nx, ny, nz] = geometry.shape.map(|d| d as usize);
    let r2 = radius_cm * radius_cm;

    // Per-direction solid-angle average of hit·e^{−Σ_t·s}: only sample
    // directions pointing inward contribute.
    let mut unc = vec![vec![0.0; groups]; n_cells];
    let mut lit = false;
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let center_mm = geometry.voxel_center_lps_mm([i as u32, j as u32, k as u32])?;
                let c: [f64; 3] = [
                    center_mm[0] / 10.0,
                    center_mm[1] / 10.0,
                    center_mm[2] / 10.0,
                ];
                let cell = i + nx * j + nx * ny * k;
                let material = &data.materials[case_material[cell]];
                for (d_hat, w_dir) in &dirs {
                    let d_axis = d_hat[a];
                    if inward * d_axis <= 0.0 {
                        continue;
                    }
                    let s = (c[a] - face_cm) / d_axis;
                    if s <= 0.0 {
                        continue;
                    }
                    let eu = c[u] - d_hat[u] * s;
                    let ev = c[v] - d_hat[v] * s;
                    let du = eu - center_uv_cm[0];
                    let dv = ev - center_uv_cm[1];
                    if du * du + dv * dv > r2 {
                        continue;
                    }
                    lit = true;
                    let frac = w_dir / omega;
                    for (g, w) in group_weights.iter().enumerate() {
                        if *w > 0.0 {
                            unc[cell][g] += beam_intensity
                                * w
                                * frac
                                * (-material.sigma_total_per_cm[g] * s).exp();
                        }
                    }
                }
            }
        }
    }
    if !lit {
        return Err(invalid(
            "uncollided beam illuminates no cell centers — check disk placement".into(),
        ));
    }
    Ok(Some(unc))
}

/// Legendre polynomial P_l(x) by the three-term recurrence.
fn legendre_p(l: u32, x: f64) -> f64 {
    match l {
        0 => 1.0,
        1 => x,
        _ => {
            let (mut p0, mut p1) = (1.0, x);
            for n in 2..=l {
                let n = n as f64;
                let p = ((2.0 * n - 1.0) * x * p1 - (n - 1.0) * p0) / n;
                p0 = p1;
                p1 = p;
            }
            p1
        }
    }
}

/// Jacobi eigendecomposition of a symmetric dense matrix (row-major
/// `n×n`). Returns `(eigenvalue, eigenvector)` pairs sorted by
/// descending |λ|. Classical cyclic Jacobi rotations — adequate for
/// the ≤288-direction quadrature kernels this feeds, and exact
/// enough that the discrete addition theorem holds to ~1e-12.
fn symmetric_jacobi_eigen(mut a: Vec<f64>, n: usize) -> Vec<(f64, Vec<f64>)> {
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..100 {
        let mut off = 0.0;
        for i in 0..n {
            for j in i + 1..n {
                off += a[i * n + j] * a[i * n + j];
            }
        }
        if off < 1e-24 * n as f64 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq.abs() < 1e-30 {
                    continue;
                }
                let app = a[p * n + p];
                let aqq = a[q * n + q];
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for i in 0..n {
                    let aip = a[i * n + p];
                    let aiq = a[i * n + q];
                    a[i * n + p] = c * aip - s * aiq;
                    a[i * n + q] = s * aip + c * aiq;
                }
                for j in 0..n {
                    let apj = a[p * n + j];
                    let aqj = a[q * n + j];
                    a[p * n + j] = c * apj - s * aqj;
                    a[q * n + j] = s * apj + c * aqj;
                }
                for i in 0..n {
                    let vip = v[i * n + p];
                    let viq = v[i * n + q];
                    v[i * n + p] = c * vip - s * viq;
                    v[i * n + q] = s * vip + c * viq;
                }
            }
        }
    }
    let mut pairs: Vec<(f64, Vec<f64>)> = (0..n)
        .map(|k| {
            (
                a[k * n + k],
                (0..n).map(|i| v[i * n + k]).collect::<Vec<f64>>(),
            )
        })
        .collect();
    pairs.sort_by(|x, y| y.0.abs().total_cmp(&x.0.abs()));
    pairs
}

/// Eigendecompose the discrete addition-theorem kernel
/// `K_ab = P_l(Ω_a·Ω_b)` over the quadrature set. P_l has exactly
/// 2l+1 nonzero eigenvalues on the continuous sphere; keeping the
/// eigenpairs with |λ| > tol·λ_max yields the exact discrete moment
/// basis — in-scatter source Σ_l(2l+1)σ_l·Σ_k u_k(a)·M_k with
/// M_k = λ_k·Σ_b w_b u_k(b)ψ_b, no direction² inner loop needed.
fn kernel_eigenbasis(quadrature: &[([f64; 3], f64)], l: u32) -> Vec<(f64, Vec<f64>)> {
    let n = quadrature.len();
    // Eigendecompose the weight-symmetrized kernel
    // B_ab = √w_a·P_l(Ω_a·Ω_b)·√w_b. Its eigenpairs (λ_k, v_k) give the
    // physical modes u_k = v_k/√w, orthonormal under the discrete
    // inner product ⟨u,v⟩_w = Σ w u v — the same product the moment
    // extraction uses, so the expansion is a consistent projection.
    let mut k = vec![0.0; n * n];
    for a in 0..n {
        let (da, wa) = quadrature[a];
        for b in 0..n {
            let (db, wb) = quadrature[b];
            k[a * n + b] =
                (wa * wb).sqrt() * legendre_p(l, da[0] * db[0] + da[1] * db[1] + da[2] * db[2]);
        }
    }
    let mut pairs = symmetric_jacobi_eigen(k, n);
    for (_, v) in &mut pairs {
        for a in 0..n {
            v[a] /= quadrature[a].1.sqrt();
        }
    }
    let lam_max = pairs.first().map(|p| p.0.abs()).unwrap_or(0.0);
    pairs.retain(|(lam, _)| lam.abs() > lam_max * 1e-9);
    pairs.truncate(2 * l as usize + 1);
    pairs
}

/// One diamond-difference sweep of group `g`: fills `psi` with
/// cell-average angular flux per direction. `flux` supplies the scatter
/// source from the current iterate (Jacobi across groups and
/// within-group alike); `psi_prev` supplies periodic-boundary inflow
/// (the wrap-around cell's previous-iterate cell average).
/// `sigma_eff[material][group]` is the effective removal cross section —
/// physical σ_t or the transport-corrected σ_t,tr when the solve
/// enables it and the data carries `transport_mu_bar`.
/// `scatter_eff[material]` mirrors `sigma_eff`: the declared matrix, or
/// the matrix with the μ̄_g·Σ_s,row forward fraction removed from the
/// diagonal under the transport correction.
/// `p1_source[cell][axis]` is the P1 anisotropic source for group `g`
/// (Σ_gp Σ_s1(gp→g)·J_{a,gp}) when the P1 mode is on — `None` under P0.
/// `kernel_source[cell][dir]` is the higher-Legendre in-scatter
/// (Σ_{l≥2}(2l+1)Σ_gp σ_l(gp→g)·Σ_k u_k(d)M_k) when `anisotropy_order
/// ≥ 2` is active — `None` otherwise.
#[allow(clippy::too_many_arguments)]
fn sweep_group(
    g: usize,
    flux: &[Vec<f64>],
    fixed_source: &[Vec<f64>],
    psi_prev: &[Vec<f64>],
    psi: &mut [Vec<f64>],
    case_material: &[usize],
    sigma_eff: &[Vec<f64>],
    scatter_eff: &[Vec<f64>],
    p1_source: Option<&[[f64; 3]]>,
    kernel_source: Option<&[Vec<f64>]>,
    data: &MultigroupData,
    geometry: &GridGeometry,
    quadrature: &[([f64; 3], f64)],
    boundary: &BoundarySource,
    periodic: [bool; 3],
) {
    let [nx, ny, nz] = geometry.shape.map(|d| d as usize);
    let groups = data.group_count();
    let dx = [
        geometry.spacing_mm[0] / 10.0,
        geometry.spacing_mm[1] / 10.0,
        geometry.spacing_mm[2] / 10.0,
    ];
    let face_area = [dx[1] * dx[2], dx[0] * dx[2], dx[0] * dx[1]];
    let volume = dx[0] * dx[1] * dx[2];

    // `psi`/`psi_prev` are [ordinate][cell]: each ordinate's sweep is
    // independent given the lagged iterate, so `par_iter_mut` hands every
    // direction its own row with no shared writes. Per-ordinate work is
    // unchanged math — the parallel split keeps results bit-identical.
    psi.par_iter_mut().enumerate().for_each(|(d, psi_d)| {
        let dir = quadrature[d].0;
        // Sweep order: ascend where the direction points positive,
        // descend where negative.
        let xs: Vec<usize> = if dir[0] > 0.0 {
            (0..nx).collect()
        } else {
            (0..nx).rev().collect()
        };
        let ys: Vec<usize> = if dir[1] > 0.0 {
            (0..ny).collect()
        } else {
            (0..ny).rev().collect()
        };
        let zs: Vec<usize> = if dir[2] > 0.0 {
            (0..nz).collect()
        } else {
            (0..nz).rev().collect()
        };
        // Per-axis *outflow edge* flux for this direction: psi_edge[a]
        // indexes the same grid; entry [cell] is the edge the sweep
        // writes toward the downstream neighbor.
        let mut edge = [
            vec![0.0; nx * ny * nz],
            vec![0.0; nx * ny * nz],
            vec![0.0; nx * ny * nz],
        ];
        for &k in &zs {
            for &j in &ys {
                for &i in &xs {
                    let cell = i + nx * j + nx * ny * k;
                    let coord = [i, j, k];
                    let mut psi_in = [0.0_f64; 3];
                    for a in 0..3 {
                        let positive = dir[a] > 0.0;
                        let inside = if positive {
                            coord[a] > 0
                        } else {
                            coord[a] + 1 < [nx, ny, nz][a]
                        };
                        psi_in[a] = if inside {
                            let mut nc = coord;
                            nc[a] = if positive { nc[a] - 1 } else { nc[a] + 1 };
                            edge[a][nc[0] + nx * nc[1] + nx * ny * nc[2]]
                        } else if periodic[a] {
                            // Periodic face: inflow is the wrap-around
                            // cell's previous-iterate average — exact
                            // under transverse uniformity, lagged
                            // otherwise.
                            let mut wc = coord;
                            wc[a] = if positive { [nx, ny, nz][a] - 1 } else { 0 };
                            psi_prev[d][wc[0] + nx * wc[1] + nx * ny * wc[2]]
                        } else {
                            // Boundary face: declared incident flux or
                            // vacuum.
                            let face = 2 * a as u8 + u8::from(!positive);
                            let (uu, vv) = match a {
                                0 => (j, k),
                                1 => (i, k),
                                _ => (i, j),
                            };
                            boundary
                                .get(&(face, uu as u32, vv as u32))
                                .and_then(|entries| {
                                    entries
                                        .iter()
                                        .find(|(dd, gg, _)| *dd == d && *gg == g)
                                        .map(|(_, _, v)| *v)
                                })
                                .unwrap_or(0.0)
                        };
                    }
                    let mi = case_material[cell];
                    let st = sigma_eff[mi][g];
                    // Scatter source into g from the current iterate plus
                    // the fixed (first-collision or volumetric) source.
                    // The P1 term adds 3·Σ_a Ω_{d,a}·S_a(cell) — the
                    // anisotropic part of the scattering source.
                    let q: f64 = fixed_source[cell][g]
                        + (0..groups)
                            .map(|gp| scatter_eff[mi][gp * groups + g] * flux[cell][gp])
                            .sum::<f64>()
                        + p1_source.map_or(0.0, |s| {
                            3.0 * (dir[0] * s[cell][0] + dir[1] * s[cell][1] + dir[2] * s[cell][2])
                        })
                        + kernel_source.map_or(0.0, |s| s[cell][d]);
                    let (ax, ay, az) = (
                        dir[0].abs() * face_area[0],
                        dir[1].abs() * face_area[1],
                        dir[2].abs() * face_area[2],
                    );
                    let denom = st * volume + 2.0 * (ax + ay + az);
                    let psi_avg = (q * volume
                        + 2.0 * (ax * psi_in[0] + ay * psi_in[1] + az * psi_in[2]))
                        / denom.max(1e-30);
                    let psi_avg = psi_avg.max(0.0);
                    psi_d[cell] = psi_avg;
                    for a in 0..3 {
                        edge[a][cell] = (2.0 * psi_avg - psi_in[a]).max(0.0);
                    }
                }
            }
        }
    });
}

/// Solve the multigroup S_N problem on the case grid.
///
/// Returns cell-averaged scalar flux `[voxel][group]` per unit source
/// rate. Source iteration is Jacobi across the full group structure;
/// upscatter is handled by the outer re-sweep until the relative scalar-
/// flux change falls under `options.convergence`.
pub fn solve_multigroup(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<MultigroupFlux, MultigroupError> {
    case.validate()?;
    data.validate()?;
    solve_multigroup_unchecked(case, data, options, data_ref, case_ref)
}

/// Solver body without input validation — crate-private, for the UQ
/// finite-difference probes whose perturbed cross sections are
/// mathematical probes, not declared data: a perturbed σ_t may sit
/// below its group's scatter row sum without invalidating the probe.
/// Callers must validate the nominal case/data first (the UQ path does
/// at the top of `propagate_uncertainty`).
pub(crate) fn solve_multigroup_unchecked(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<MultigroupFlux, MultigroupError> {
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let groups = data.group_count();
    let quadrature = level_symmetric_quadrature(options.quadrature_order)?;
    let (data_eff, case_material) =
        material_composition_map(case, data, options.assignment.as_ref())?;
    let data = &data_eff;

    // Uncollided beam split or the discrete boundary-flux path.
    let uncollided = if options.beam_uncollided_split {
        uncollided_beam_flux(case, data, &case_material)?
    } else {
        None
    };
    let boundary = if uncollided.is_some() {
        BoundarySource::new()
    } else {
        map_boundary_source(case, data, &quadrature)?
    };
    // First-collision source driven by the uncollided flux.
    let fixed_source: Vec<Vec<f64>> = match &uncollided {
        Some(unc) => (0..n_cells)
            .map(|cell| {
                let material = &data.materials[case_material[cell]];
                (0..groups)
                    .map(|g| {
                        (0..groups)
                            .map(|gp| {
                                material.scatter_matrix_per_cm[gp * groups + g] * unc[cell][gp]
                            })
                            .sum()
                    })
                    .collect()
            })
            .collect(),
        None => vec![vec![0.0; groups]; n_cells],
    };

    let mut result = solve_sn_problem(
        case,
        data,
        options,
        &case_material,
        &quadrature,
        &boundary,
        &fixed_source,
        data_ref,
        case_ref,
    )?;

    // Total flux = collided solve + analytic uncollided component.
    if let Some(unc) = &uncollided {
        for (row, unc_row) in result.flux.iter_mut().zip(unc.iter()) {
            for (f, u) in row.iter_mut().zip(unc_row.iter()) {
                *f += u;
            }
        }
        result.beam_model = "uncollided_split".into();
    } else {
        result.beam_model = "boundary_flux".into();
    }

    Ok(result)
}

/// Adjoint multigroup solve: the scalar importance function for a
/// declared volumetric response source.
///
/// On a reflection-symmetric quadrature the adjoint scalar flux equals
/// the *forward* solve of the transposed problem — scatter matrix
/// transposed (adjoint upscatter where forward downscatters), the same
/// vacuum/periodic faces, and the adjoint source as a volumetric
/// emission density. `adjoint_source` is `[cell][group]` in arbitrary
/// consistent units — importance is only ever used up to a global scale.
pub fn solve_multigroup_adjoint(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    adjoint_source: &[Vec<f64>],
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<MultigroupFlux, MultigroupError> {
    case.validate()?;
    data.validate()?;
    let invalid = |m: String| MultigroupError::Solve(m);
    let n_cells = case.geometry.voxel_count()?;
    let groups = data.group_count();
    if adjoint_source.len() != n_cells || adjoint_source.iter().any(|row| row.len() != groups) {
        return Err(invalid("adjoint source must be [cells][groups]".into()));
    }
    // Transposed scatter: the adjoint equation couples group g's source
    // into g' via Σ_s[g'→g] — the transpose of the forward matrix.
    let mut adjoint_data = data.clone();
    adjoint_data.id = format!("{}.adjoint", data.id);
    for material in &mut adjoint_data.materials {
        let mut transposed = vec![0.0; groups * groups];
        for g in 0..groups {
            for gp in 0..groups {
                transposed[g * groups + gp] = material.scatter_matrix_per_cm[gp * groups + g];
            }
        }
        material.scatter_matrix_per_cm = transposed;
        // The P1 adjoint couples adjoint harmonic moments differently —
        // not a matrix transpose. The adjoint solve stays P0; importance
        // functions for weight windows do not need the P1 fidelity.
        material.scatter_p1_matrix_per_cm = None;
    }
    let (adjoint_data, case_material) =
        material_composition_map(case, &adjoint_data, options.assignment.as_ref())?;
    let quadrature = level_symmetric_quadrature(options.quadrature_order)?;
    let mut adjoint_options = options.clone();
    adjoint_options.p1_anisotropic = false;
    let mut result = solve_sn_problem(
        case,
        &adjoint_data,
        &adjoint_options,
        &case_material,
        &quadrature,
        &BoundarySource::new(),
        adjoint_source,
        data_ref,
        case_ref,
    )?;
    result.beam_model = "adjoint_volumetric".into();
    result.energy_boundaries_ev = data.energy_boundaries_ev.clone();
    Ok(result)
}

/// Score one beam direction against a solved adjoint importance field:
/// the inner product of the direction's uncollided beam flux with φ*.
///
/// One adjoint solve (target region as the adjoint source) plus this
/// ray-trace scores every candidate direction without a per-direction
/// forward solve — the fast first pass of a beam-direction search.
/// Units are importance×fluence; the score is meaningful only
/// comparatively across directions sharing the adjoint, geometry, and
/// aperture.
///
/// The source template's space/angle are replaced by
/// [`crate::positioning::aim_disk_source_at_centroid`]'s aimed disk —
/// the same positioning `plan fields` uses, so the score ranks the
/// beams the sweep would actually solve.
pub fn adjoint_direction_score(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    adjoint: &MultigroupFlux,
    aim: &openbnct_core::RegionMask,
    direction_lps: [f64; 3],
    radius_cm: f64,
) -> Result<f64, MultigroupError> {
    let invalid = |m: String| MultigroupError::Solve(m);
    let n_cells = case.geometry.voxel_count()?;
    if adjoint.flux.len() != n_cells {
        return Err(invalid("adjoint flux grid does not match the case".into()));
    }
    let (aimed_source, _report) = crate::positioning::aim_disk_source_at_centroid(
        &case.source,
        &case.geometry,
        aim,
        direction_lps,
        radius_cm,
    )
    .map_err(|error| invalid(format!("aim direction: {error}")))?;
    let mut aimed_case = case.clone();
    aimed_case.source = aimed_source;
    let (_ad, case_material) =
        material_composition_map(&aimed_case, data, options.assignment.as_ref())?;
    let Some(uncollided) = uncollided_beam_flux(&aimed_case, data, &case_material)? else {
        return Err(invalid(
            "direction does not admit an uncollided beam — needs an on-face disk source".into(),
        ));
    };
    let mut score = 0.0;
    for (beam_groups, adjoint_groups) in uncollided.iter().zip(adjoint.flux.iter()).take(n_cells) {
        for (beam, importance) in beam_groups.iter().zip(adjoint_groups.iter()) {
            score += beam * importance;
        }
    }
    if !score.is_finite() {
        return Err(invalid("adjoint direction score diverged".into()));
    }
    Ok(score)
}

/// Per-voxel material index: base material, then assignment regions.
/// Public so downstream evaluators (UQ, screening, lineal tallies) can
/// resolve the same material map the solver used.
pub fn cell_materials(
    case: &TransportCase,
    data: &MultigroupData,
    assignment: Option<&MaterialAssignment>,
) -> Result<Vec<usize>, MultigroupError> {
    let invalid = |m: String| MultigroupError::Solve(m);
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let material_index = |material_id: &str| -> Result<usize, MultigroupError> {
        data.materials
            .iter()
            .position(|m| m.material_id == material_id)
            .ok_or_else(|| {
                invalid(format!(
                    "multigroup data has no entry for material {material_id:?}"
                ))
            })
    };
    let mut case_material = vec![material_index(&case.material.id)?; n_cells];
    if let Some(assignment) = assignment {
        assignment.validate(geometry)?;
        let [nx, ny, _] = geometry.shape.map(|d| d as usize);
        for region in &assignment.regions {
            let idx = material_index(&region.material.id)?;
            match &region.shape {
                MaterialRegionShape::VoxelBox { lower, upper } => {
                    for k in lower[2]..=upper[2] {
                        for j in lower[1]..=upper[1] {
                            for i in lower[0]..=upper[0] {
                                case_material
                                    [i as usize + nx * j as usize + nx * ny * k as usize] = idx;
                            }
                        }
                    }
                }
                MaterialRegionShape::VoxelSet { indices } => {
                    for vox in indices {
                        case_material
                            [vox[0] as usize + nx * vox[1] as usize + nx * ny * vox[2] as usize] =
                            idx;
                    }
                }
                MaterialRegionShape::VoxelFractions { .. } => {
                    return Err(invalid(
                        "cell_materials cannot express partial-cell fractions — use \
                         material_composition_map which synthesizes the blended tables"
                            .into(),
                    ));
                }
            }
        }
    }
    Ok(case_material)
}

/// Volume-weighted blend of macroscopic material tables: each
/// component contributes `fraction·table` to σ_t, the scatter
/// matrices (P0/P1/l≥2 moments), and the dose responses; `μ̄` blends
/// scatter-weighted. The first-order sub-voxel mixture rule — exact
/// when the components' optical thicknesses are small.
fn blend_material(
    signature: &[(usize, f64)],
    materials: &[MultigroupMaterial],
) -> MultigroupMaterial {
    let groups = materials[signature[0].0].sigma_total_per_cm.len();
    let gg = groups * groups;
    fn weighted(
        signature: &[(usize, f64)],
        materials: &[MultigroupMaterial],
        pick: impl Fn(&MultigroupMaterial) -> Option<&[f64]>,
        size: usize,
    ) -> Option<Vec<f64>> {
        let mut out = vec![0.0; size];
        let mut any = false;
        for &(mi, f) in signature {
            if let Some(v) = pick(&materials[mi]) {
                for (o, &x) in out.iter_mut().zip(v.iter()) {
                    *o += f * x;
                }
                any = true;
            }
        }
        any.then_some(out)
    }
    let sigma_t = weighted(
        signature,
        materials,
        |m| Some(&m.sigma_total_per_cm),
        groups,
    )
    .unwrap();
    let scatter = weighted(signature, materials, |m| Some(&m.scatter_matrix_per_cm), gg).unwrap();
    let p1 = weighted(
        signature,
        materials,
        |m| m.scatter_p1_matrix_per_cm.as_deref(),
        gg,
    );
    // μ̄ blends scatter-row-weighted — not volume-weighted.
    let mut mu_num = vec![0.0; groups];
    let mut mu_den = vec![0.0; groups];
    for &(mi, f) in signature {
        let m = &materials[mi];
        if let Some(mu) = &m.transport_mu_bar {
            for g in 0..groups {
                let ss: f64 = (0..groups)
                    .map(|gp| m.scatter_matrix_per_cm[g * groups + gp])
                    .sum();
                mu_num[g] += f * ss * mu[g];
                mu_den[g] += f * ss;
            }
        }
    }
    let mu_bar = (mu_den.iter().any(|&d| d > 0.0)).then(|| {
        mu_num
            .iter()
            .zip(&mu_den)
            .map(|(&n, &d)| if d > 0.0 { n / d } else { 0.0 })
            .collect()
    });
    // Dose responses blend by volume fraction — the first-order rule;
    // exact when the components share a density (BNCT tissues do to
    // within ~10%).
    let mut dose_keys = std::collections::BTreeSet::new();
    for &(mi, _) in signature {
        dose_keys.extend(materials[mi].dose_response_gy_cm2.keys().cloned());
    }
    let dose: std::collections::BTreeMap<String, Vec<f64>> = dose_keys
        .into_iter()
        .map(|k| {
            let mut v = vec![0.0; groups];
            for &(mi, f) in signature {
                if let Some(r) = materials[mi].dose_response_gy_cm2.get(&k) {
                    for (o, &x) in v.iter_mut().zip(r.iter()) {
                        *o += f * x;
                    }
                }
            }
            (k, v)
        })
        .collect();
    MultigroupMaterial {
        material_id: signature
            .iter()
            .map(|&(mi, f)| format!("{}@{:.3}", materials[mi].material_id, f))
            .collect::<Vec<_>>()
            .join("+"),
        sigma_total_per_cm: sigma_t,
        scatter_matrix_per_cm: scatter,
        scatter_p1_matrix_per_cm: p1,
        scatter_legendre_moments_per_cm: (0..4)
            .map(|li| {
                weighted(
                    signature,
                    materials,
                    |m| {
                        m.scatter_legendre_moments_per_cm
                            .as_ref()
                            .and_then(|v| v.get(li))
                            .map(Vec::as_slice)
                    },
                    gg,
                )
            })
            .collect::<Option<Vec<_>>>(),
        dose_response_gy_cm2: dose,
        transport_mu_bar: mu_bar,
    }
}

/// Per-cell material composition: `composition[cell]` is the cell's
/// `(material_index, volume_fraction)` signature — `[(base, 1.0)]` for
/// unblended cells, the merged region+base mixture for fraction
/// cells. Indices are into `data.materials` (never synthesized rows),
/// so callers can blend any material-attached quantity (production
/// matrices, photon tables) with the same rule.
pub fn cell_compositions(
    case: &TransportCase,
    data: &MultigroupData,
    assignment: Option<&MaterialAssignment>,
) -> Result<Vec<Vec<(usize, f64)>>, MultigroupError> {
    let invalid = |m: String| MultigroupError::Solve(m);
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let material_index = |material_id: &str| -> Result<usize, MultigroupError> {
        data.materials
            .iter()
            .position(|m| m.material_id == material_id)
            .ok_or_else(|| {
                invalid(format!(
                    "multigroup data has no entry for material {material_id:?}"
                ))
            })
    };
    let base_idx = material_index(&case.material.id)?;
    let mut case_material = vec![base_idx; n_cells];
    let mut fractions: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n_cells];
    if let Some(assignment) = assignment {
        assignment.validate(geometry)?;
        let [nx, ny, _] = geometry.shape.map(|d| d as usize);
        for region in &assignment.regions {
            let idx = material_index(&region.material.id)?;
            match &region.shape {
                MaterialRegionShape::VoxelBox { lower, upper } => {
                    for k in lower[2]..=upper[2] {
                        for j in lower[1]..=upper[1] {
                            for i in lower[0]..=upper[0] {
                                case_material
                                    [i as usize + nx * j as usize + nx * ny * k as usize] = idx;
                            }
                        }
                    }
                }
                MaterialRegionShape::VoxelSet { indices } => {
                    for vox in indices {
                        case_material
                            [vox[0] as usize + nx * vox[1] as usize + nx * ny * vox[2] as usize] =
                            idx;
                    }
                }
                MaterialRegionShape::VoxelFractions {
                    indices,
                    fractions: fs,
                } => {
                    for (vox, f) in indices.iter().zip(fs.iter()) {
                        let flat =
                            vox[0] as usize + nx * vox[1] as usize + nx * ny * vox[2] as usize;
                        fractions[flat].push((idx, *f));
                    }
                }
            }
        }
    }
    Ok((0..n_cells)
        .map(|cell| {
            if fractions[cell].is_empty() {
                return vec![(case_material[cell], 1.0)];
            }
            let remainder = 1.0 - fractions[cell].iter().map(|&(_, f)| f).sum::<f64>();
            let mut merged: std::collections::BTreeMap<usize, f64> =
                std::collections::BTreeMap::new();
            for &(m, f) in &fractions[cell] {
                *merged.entry(m).or_insert(0.0) += f;
            }
            *merged.entry(case_material[cell]).or_insert(0.0) += remainder;
            merged.into_iter().collect()
        })
        .collect())
}

/// Effective material list + per-cell material index with
/// partial-cell volume fractions applied: `voxel_fractions` regions
/// contribute `f` of the region material and `1 − Σf` of the base
/// material to each listed voxel. Cells sharing an identical
/// composition signature share one synthesized blend material, so the
/// solver sees a plain index map. Returns `(effective_data,
/// case_material)` where `effective_data` carries the declared
/// materials plus one synthetic row per distinct blend signature —
/// indices may therefore exceed `data.materials.len()` and must index
/// `effective_data.materials`.
pub fn material_composition_map(
    case: &TransportCase,
    data: &MultigroupData,
    assignment: Option<&MaterialAssignment>,
) -> Result<(MultigroupData, Vec<usize>), MultigroupError> {
    let n_cells = case.geometry.voxel_count()?;
    let compositions = cell_compositions(case, data, assignment)?;
    let mut effective = data.clone();
    let mut case_material = vec![0usize; n_cells];
    let mut signatures: std::collections::BTreeMap<Vec<(usize, u64)>, usize> =
        std::collections::BTreeMap::new();
    for (cell, signature) in compositions.iter().enumerate() {
        if signature.len() == 1 && signature[0].1 == 1.0 {
            case_material[cell] = signature[0].0;
            continue;
        }
        let key: Vec<(usize, u64)> = signature.iter().map(|&(m, f)| (m, f.to_bits())).collect();
        let idx = *signatures.entry(key).or_insert_with(|| {
            effective
                .materials
                .push(blend_material(signature, &data.materials));
            effective.materials.len() - 1
        });
        case_material[cell] = idx;
    }
    Ok((effective, case_material))
}

/// Shared iteration core for the forward and adjoint solves: source
/// iteration over `fixed_source` (volumetric or first-collision) plus
/// `boundary` incident flux on the case grid.
#[allow(clippy::too_many_arguments)]
pub(crate) fn solve_sn_problem(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    case_material: &[usize],
    quadrature: &[([f64; 3], f64)],
    boundary: &BoundarySource,
    fixed_source: &[Vec<f64>],
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<MultigroupFlux, MultigroupError> {
    let invalid = |m: String| MultigroupError::Solve(m);
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let groups = data.group_count();
    let n_dirs = quadrature.len();
    if fixed_source.len() != n_cells || fixed_source.iter().any(|row| row.len() != groups) {
        return Err(invalid("fixed source must be [cells][groups]".into()));
    }
    // Effective removal cross section and self-scatter per (material,
    // group) under the extended transport correction: σ_t,tr = σ_t −
    // μ̄_g·Σ_s,row(g) with the same forward-scatter fraction removed
    // from the in-group diagonal σ_s,tr(g→g) = σ_s(g→g) − μ̄_g·Σ_s,row(g)
    // (floored at zero). Reducing only σ_t would leave self-scatter
    // able to exceed σ_t,tr — a divergent source iteration in
    // near-conservative media. With the diagonal reduced consistently
    // the balance σ_t,tr − σ_s,tr(g→g) = σ_a + σ_s,out-of-group is
    // unchanged and the sweep stays contractive. The correction
    // applies to the collided sweep only — the uncollided ray-trace
    // keeps the exact exponential.
    //
    // P1 supersedes the correction: it carries the same anisotropy
    // physics explicitly, so the correction is disabled and physical
    // σ_t applies. Every material that scatters must carry the P1
    // table — an absent table would silently downgrade that material
    // to isotropic.
    let p1 = options.p1_anisotropic;
    let lmax = options.anisotropy_order.min(5);
    if lmax >= 2 {
        if !p1 {
            return Err(invalid(
                "anisotropy_order ≥ 2 requires p1_anisotropic (the l = 1 term)".into(),
            ));
        }
        for material in &data.materials {
            let scatters = material.scatter_matrix_per_cm.iter().any(|&v| v > 0.0);
            if !scatters {
                continue;
            }
            let provided = material
                .scatter_legendre_moments_per_cm
                .as_ref()
                .map_or(0, |m| m.len());
            if provided < (lmax - 1) as usize {
                return Err(invalid(format!(
                    "anisotropy_order {lmax} requires legendre moments l = 2..={lmax} on \
                     every scattering material; {:?} provides l = 2..={}",
                    material.material_id,
                    provided + 1
                )));
            }
        }
    }
    // Discrete addition-theorem eigenbasis per l = 2..=lmax — built
    // once; the 2l+1 significant eigenpairs of K_ab = P_l(Ω_a·Ω_b)
    // carry that l's exact discrete kernel.
    let eigen: Vec<Vec<(f64, Vec<f64>)>> = (2..=lmax)
        .map(|l| kernel_eigenbasis(quadrature, l))
        .collect();
    let n_kernel_moments: usize = eigen.iter().map(|e| e.len()).sum();
    if p1 {
        for material in &data.materials {
            let scatters = material.scatter_matrix_per_cm.iter().any(|&v| v > 0.0);
            if scatters && material.scatter_p1_matrix_per_cm.is_none() {
                return Err(invalid(format!(
                    "p1_anisotropic requires scatter_p1_matrix_per_cm on every \
                     scattering material; {:?} lacks it",
                    material.material_id
                )));
            }
        }
    }
    let corrected = options.transport_correction
        && !p1
        && data.materials.iter().any(|m| m.transport_mu_bar.is_some());
    let sigma_eff: Vec<Vec<f64>> = data
        .materials
        .iter()
        .map(|m| {
            (0..groups)
                .map(|g| {
                    let st = m.sigma_total_per_cm[g];
                    match (corrected, &m.transport_mu_bar) {
                        (true, Some(mu)) => {
                            let ss: f64 = (0..groups)
                                .map(|gp| m.scatter_matrix_per_cm[g * groups + gp])
                                .sum();
                            // Floor at the absorption part so the
                            // operator stays positive.
                            (st - mu[g] * ss).max(st - ss).max(1e-12)
                        }
                        _ => st,
                    }
                })
                .collect()
        })
        .collect();
    let scatter_eff: Vec<Vec<f64>> = data
        .materials
        .iter()
        .map(|m| {
            let mut row = m.scatter_matrix_per_cm.clone();
            if let (true, Some(mu)) = (corrected, &m.transport_mu_bar) {
                for g in 0..groups {
                    let ss: f64 = (0..groups).map(|gp| row[g * groups + gp]).sum();
                    let diag = &mut row[g * groups + g];
                    *diag = (*diag - mu[g] * ss).max(0.0);
                }
            }
            row
        })
        .collect();

    let mut flux = vec![vec![0.0; groups]; n_cells];
    // P1: angle-averaged cell currents J_a[cell][group][axis] —
    // J_a = (1/4π)·Σ_d w_d·Ω_{d,a}·ψ_d, iterated Jacobi-style with the
    // scalar flux.
    let mut current = vec![vec![[0.0_f64; 3]; groups]; n_cells];
    // Higher-Legendre kernel moments M_k[cell][group][kk] with
    // M_k = λ_k·Σ_d w_d·u_k(d)·ψ_d — the discrete addition-theorem
    // moments for l = 2..=lmax, iterated Jacobi-style alongside the
    // currents.
    let mut kernel_moments = vec![vec![vec![0.0_f64; n_kernel_moments]; groups]; n_cells];
    // Angular storage is [ordinate][cell] so the sweep can hand each
    // direction an exclusive row under `par_iter_mut` (see `sweep_group`).
    let mut psi = vec![vec![0.0; n_cells]; n_dirs];
    let mut psi_prev = vec![vec![0.0; n_cells]; n_dirs];
    let mut converged = false;
    let mut residual = f64::MAX;
    let mut outer_done = 0;
    let mut anderson =
        (options.anderson_depth > 0).then(|| AndersonState::new(options.anderson_depth));
    // Origin of the current symmetric down+up cycle — the residual
    // Anderson minimizes is against this point, not the up-pass input.
    let mut cycle_origin: Vec<Vec<f64>> = Vec::new();

    for outer in 0..options.max_outer_iterations {
        let previous = flux.clone();
        if anderson.is_some() && outer % 2 == 0 {
            cycle_origin = previous.clone();
        }
        // Symmetric Gauss-Seidel over the group structure: alternate the
        // sweep direction each outer iteration. Downscatter-only
        // ordering converges one-coupling-per-sweep under bound-atom
        // (S(α,β)) upscatter; alternating carries upscatter information
        // at full speed on the ascending pass.
        let ascending = outer % 2 == 1;
        for g in 0..groups {
            let g = if ascending { groups - 1 - g } else { g };
            // Within-group Jacobi iteration on the scatter source.
            for _inner in 0..options.max_inner_iterations {
                // P1 anisotropic source into group g:
                // S_a(cell) = Σ_gp Σ_s1(gp→g)·J_{a,gp}(cell).
                let p1_source: Option<Vec<[f64; 3]>> = if p1 {
                    let mut src = vec![[0.0_f64; 3]; n_cells];
                    for cell in 0..n_cells {
                        let mi = case_material[cell];
                        if let Some(m) = &data.materials[mi].scatter_p1_matrix_per_cm {
                            for gp in 0..groups {
                                let s = m[gp * groups + g];
                                let j = &current[cell][gp];
                                src[cell][0] += s * j[0];
                                src[cell][1] += s * j[1];
                                src[cell][2] += s * j[2];
                            }
                        }
                    }
                    Some(src)
                } else {
                    None
                };
                // Higher-Legendre in-scatter for group g:
                // q[cell][d] = Σ_l(2l+1)/(4π)·Σ_k u_k(d)·W_k(cell),
                // W_k = Σ_gp σ_l(gp→g)·M_k(cell,gp). Folded over the
                // l-blocks of the flattened moment vector. The 1/(4π)
                // is the per-sr source convention — the P0 path carries
                // it inside σ_0 (σ(μ) = Σ_l(2l+1)/(4π)·σ_l·P_l(μ));
                // without it the in-scatter is 4π× too strong and the
                // inner iteration diverges.
                let kernel_source: Option<Vec<Vec<f64>>> = if lmax >= 2 {
                    let inv_4pi = 1.0 / (4.0 * std::f64::consts::PI);
                    let mut weighted = vec![vec![0.0_f64; n_kernel_moments]; n_cells];
                    let mut kk = 0;
                    for (li, eigs) in eigen.iter().enumerate() {
                        let l = li as u32 + 2;
                        let two_l1 = (2 * l + 1) as f64 * inv_4pi;
                        for cell in 0..n_cells {
                            let mi = case_material[cell];
                            let moments = &data.materials[mi]
                                .scatter_legendre_moments_per_cm
                                .as_deref()
                                .unwrap();
                            for (k, _) in eigs.iter().enumerate() {
                                let mut w_k = 0.0;
                                for gp in 0..groups {
                                    w_k += moments[li][gp * groups + g]
                                        * kernel_moments[cell][gp][kk + k];
                                }
                                weighted[cell][kk + k] = two_l1 * w_k;
                            }
                        }
                        kk += eigs.len();
                    }
                    let mut src = vec![vec![0.0_f64; n_dirs]; n_cells];
                    let mut kk = 0;
                    for eigs in eigen.iter() {
                        for (k, (_, u)) in eigs.iter().enumerate() {
                            for cell in 0..n_cells {
                                let w = weighted[cell][kk + k];
                                if w != 0.0 {
                                    for d in 0..n_dirs {
                                        src[cell][d] += u[d] * w;
                                    }
                                }
                            }
                        }
                        kk += eigs.len();
                    }
                    Some(src)
                } else {
                    None
                };
                std::mem::swap(&mut psi, &mut psi_prev);
                sweep_group(
                    g,
                    &flux,
                    fixed_source,
                    &psi_prev,
                    &mut psi,
                    case_material,
                    &sigma_eff,
                    &scatter_eff,
                    p1_source.as_deref(),
                    kernel_source.as_deref(),
                    data,
                    geometry,
                    quadrature,
                    boundary,
                    options.periodic,
                );
                // Moment reduction is per-cell independent — computed in
                // parallel into an indexed buffer, then applied serially
                // so `change` and the stores stay deterministic.
                let reduced: Vec<(f64, [f64; 3], Vec<f64>)> = (0..n_cells)
                    .into_par_iter()
                    .map(|cell| {
                        let mut new_flux = 0.0_f64;
                        let mut j = [0.0_f64; 3];
                        for d in 0..n_dirs {
                            let (dir, w) = quadrature[d];
                            new_flux += w * psi[d][cell];
                            if p1 {
                                for a in 0..3 {
                                    j[a] += w * dir[a] * psi[d][cell];
                                }
                            }
                        }
                        new_flux /= 4.0 * std::f64::consts::PI;
                        if p1 {
                            for ja in &mut j {
                                *ja /= 4.0 * std::f64::consts::PI;
                            }
                        }
                        let mut moments = Vec::new();
                        if lmax >= 2 {
                            // M_k = λ_k·Σ_d w_d u_k(d)ψ_d — eigenbasis
                            // moments of the refreshed angular flux.
                            moments.reserve(n_kernel_moments);
                            for eigs in eigen.iter() {
                                for (lam, u) in eigs.iter() {
                                    let mut m = 0.0;
                                    for d in 0..n_dirs {
                                        m += quadrature[d].1 * u[d] * psi[d][cell];
                                    }
                                    moments.push(lam * m);
                                }
                            }
                        }
                        (new_flux, j, moments)
                    })
                    .collect();
                let mut change = 0.0_f64;
                for (cell, (new_flux, j, moments)) in reduced.iter().enumerate() {
                    if p1 {
                        current[cell][g] = *j;
                    }
                    if lmax >= 2 {
                        kernel_moments[cell][g].copy_from_slice(moments);
                    }
                    change =
                        change.max((new_flux - flux[cell][g]).abs() / new_flux.abs().max(1e-30));
                    flux[cell][g] = *new_flux;
                }
                if change < options.convergence {
                    break;
                }
            }
        }
        residual = flux
            .iter()
            .zip(previous.iter())
            .flat_map(|(a, b)| a.iter().zip(b.iter()))
            .map(|(x, y)| (x - y).abs() / x.abs().max(1e-30))
            .fold(0.0_f64, f64::max);
        outer_done = outer + 1;
        if residual < options.convergence {
            converged = true;
            break;
        }
        // Anderson mix once per symmetric down+up cycle (the composed
        // map is the consistent operator the accelerator applies to).
        // The convergence check above already used the true map
        // residual r_k = F(x_k) − x_k, so the accelerated iterate only
        // reseeds the next cycle.
        if let Some(acc) = &mut anderson
            && outer % 2 == 1
        {
            flux = acc.mix(&flux, &cycle_origin);
        }
        if std::env::var_os("OPENBNCT_SOLVE_PROGRESS").is_some() {
            eprintln!("[sn-solve] outer {} residual {:.4e}", outer + 1, residual);
        }
    }

    Ok(MultigroupFlux {
        schema_version: MULTIGROUP_FLUX_SCHEMA.into(),
        case_id: case.case_id.clone(),
        multigroup_data: data_ref,
        case: case_ref,
        energy_boundaries_ev: data.energy_boundaries_ev.clone(),
        beam_model: "volumetric_or_boundary".into(),
        flux,
        transport_correction: corrected,
        scattering_order: Some(
            if p1 {
                "p1"
            } else if corrected {
                "p0_transport_corrected"
            } else {
                "p0"
            }
            .into(),
        ),
        source_spectrum_weighting: "collapse_consistent".into(),
        quadrature_order: options.quadrature_order,
        outer_iterations: outer_done,
        residual,
        converged,
        qualification: "research-only: deterministic multigroup flux, not a clinical quantity"
            .into(),
        provenance_id: format!("sn-s{}-{}", options.quadrature_order, case.case_id),
    })
}

/// Fold a converged multigroup flux into a `PhysicalDoseBundle` using the
/// data artifact's declared dose-response vectors. Only components
/// present in every material used by the geometry are emitted; the
/// physical total is their sum with `Unavailable` uncertainty —
/// deterministic flux carries no Monte Carlo σ.
pub fn fold_multigroup_dose(
    case: &TransportCase,
    data: &MultigroupData,
    flux: &MultigroupFlux,
    assignment: Option<&MaterialAssignment>,
    component_profile: ContentReference,
    response_set: ContentReference,
) -> Result<PhysicalDoseBundle, MultigroupError> {
    data.validate()?;
    let invalid = |m: String| MultigroupError::Solve(m);
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let groups = data.group_count();

    // Per-voxel material index — partial-cell fractions synthesize
    // blend rows in the effective data, so dose folding sees the same
    // volume-weighted response the sweep used.
    let (data_eff, case_material) = material_composition_map(case, data, assignment)?;
    let material_of = |cell: usize| -> Result<&MultigroupMaterial, MultigroupError> {
        data_eff
            .materials
            .get(case_material[cell])
            .ok_or_else(|| invalid("material index out of range".into()))
    };
    // Material definitions by id — the boron microdistribution lives
    // on the declared material, not the collapsed table.
    let mut definitions: std::collections::BTreeMap<&str, &crate::MaterialDefinition> =
        std::collections::BTreeMap::new();
    definitions.insert(case.material.id.as_str(), &case.material);
    if let Some(a) = assignment {
        definitions.insert(a.base_material.id.as_str(), &a.base_material);
        for region in &a.regions {
            definitions.insert(region.material.id.as_str(), &region.material);
        }
    }
    // Compound factor per material id — 1.0 when no microdistribution
    // is declared. Per-cell factors weight through the same volume
    // fractions the response blends by.
    let compound_factor = |material_id: &str| -> f64 {
        definitions
            .get(material_id)
            .and_then(|d| d.boron_microdistribution.as_ref())
            .map_or(1.0, |m| m.compound_factor())
    };
    let compositions = cell_compositions(case, data, assignment)?;
    let cell_factor = |cell: usize| -> f64 {
        compositions[cell]
            .iter()
            .map(|&(mi, f)| f * compound_factor(&data.materials[mi].material_id))
            .sum()
    };

    // Component set = intersection over all materials in use.
    let mut components: Vec<String> = material_of(0)?
        .dose_response_gy_cm2
        .keys()
        .cloned()
        .collect();
    for cell in 0..n_cells {
        let mat = material_of(cell)?;
        components.retain(|c| mat.dose_response_gy_cm2.contains_key(c));
    }
    if components.is_empty() {
        return Err(invalid(
            "no dose-response component is common to all assigned materials".into(),
        ));
    }
    components.sort();

    let mut volumes = Vec::with_capacity(components.len());
    for component in &components {
        let mut values = vec![0.0; n_cells];
        for (cell, value) in values.iter_mut().enumerate() {
            let response = &material_of(cell)?.dose_response_gy_cm2[component];
            *value = (0..groups).map(|g| flux.flux[cell][g] * response[g]).sum();
            // The boron component carries the declared microdistribution
            // compound factor — the effective (target-weighted) boron
            // dose, the standard TPS convention.
            if component.strip_prefix("component:").unwrap_or(component) == "boron" {
                *value *= cell_factor(cell);
            }
        }
        let name = component.strip_prefix("component:").unwrap_or(component);
        let dc = match name.to_ascii_lowercase().as_str() {
            "boron" => openbnct_core::DoseComponent::Boron,
            "nitrogen" => openbnct_core::DoseComponent::Nitrogen,
            "hydrogen" => openbnct_core::DoseComponent::Hydrogen,
            "photon" => openbnct_core::DoseComponent::Photon,
            _ => {
                return Err(invalid(format!(
                    "response component {component:?} is not a DoseComponent"
                )));
            }
        };
        volumes.push(DoseVolume {
            component: dc,
            unit: DoseUnit::GrayPerSourceParticle,
            values,
            absolute_standard_uncertainty: None,
        });
    }
    let mut total = vec![0.0; n_cells];
    for volume in &volumes {
        for (t, v) in total.iter_mut().zip(&volume.values) {
            *t += v;
        }
    }
    Ok(PhysicalDoseBundle {
        schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: case.case_id.clone(),
        frame_of_reference_uid: None,
        geometry: geometry.clone(),
        component_profile,
        response_set,
        components: volumes,
        physical_total: PhysicalTotalDoseVolume {
            unit: DoseUnit::GrayPerSourceParticle,
            values: total,
            absolute_standard_uncertainty: None,
            uncertainty_method: TotalUncertaintyMethod::Unavailable,
        },
        provenance_id: flux.provenance_id.clone(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{
        FixedSourceDefinition, MATERIAL_ASSIGNMENT_SCHEMA, MaterialDefinition, MaterialRegion,
        NeutronThermalTreatment, NuclideMassFraction, ParticleType, PlaneAxis,
    };

    const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    pub(crate) fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    fn material(id: &str) -> MaterialDefinition {
        MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: id.into(),
            density_g_cm3: 1.0,
            temperature_k: 294.0,
            nuclides: vec![NuclideMassFraction {
                name: "B10".into(),
                mass_fraction: 1.0,
            }],
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        }
    }

    /// Slab fixture: 4×4×20 cells of 1 mm, beam disk covering the whole
    /// −z face (radius exceeds the transverse half-diagonal so every
    /// boundary column is covered → uniform transverse field → 1-D).
    pub(crate) fn slab_case() -> TransportCase {
        TransportCase {
            schema_version: "openbnct.transport-case/0.1.0".into(),
            case_id: "mg-slab".into(),
            geometry: GridGeometry {
                shape: [4, 4, 20],
                spacing_mm: [1.0; 3],
                origin_mm: [-1.5, -1.5, -9.5],
                direction: IDENTITY,
            },
            material: material("absorber"),
            source: FixedSourceDefinition {
                schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
                id: "beam".into(),
                particle: ParticleType::Neutron,
                source_sites_per_history: 1,
                statistical_weight_per_site: 1.0,
                space: SourceSpatialDistribution::UniformDisk {
                    axis: PlaneAxis::Z,
                    offset_cm: -1.0,
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 0.25,
                },
                angle: AngularDistribution::Monodirectional {
                    unit_vector: [0.0, 0.0, 1.0],
                },
                energy: EnergyDistribution::Monoenergetic { energy_ev: 0.0253 },
            },
            requested_histories: 1,
        }
    }

    pub(crate) fn data(sigma_t: &[f64], scatter: Vec<f64>) -> MultigroupData {
        MultigroupData {
            schema_version: MULTIGROUP_DATA_SCHEMA.into(),
            id: "mg-data".into(),
            energy_boundaries_ev: if sigma_t.len() == 1 {
                vec![1.0, 1.0e-3]
            } else {
                // group 0 spans (0.01, 1.0] eV — contains the 0.0253 eV
                // beam; group 1 is the thermal tail below.
                vec![1.0, 1.0e-2, 1.0e-5]
            },
            collapse_declaration: "test fixture".into(),
            component_profile: None,
            materials: vec![MultigroupMaterial {
                material_id: "absorber".into(),
                sigma_total_per_cm: sigma_t.to_vec(),
                scatter_matrix_per_cm: scatter,
                scatter_p1_matrix_per_cm: None,
                scatter_legendre_moments_per_cm: None,
                dose_response_gy_cm2: Default::default(),
                transport_mu_bar: None,
            }],
        }
    }

    /// Periodic transverse faces make the covered-face beam problem
    /// exactly translation-invariant — the slab tests' intent.
    pub(crate) fn options() -> SnOptions {
        SnOptions {
            quadrature_order: 4,
            convergence: 1e-10,
            max_inner_iterations: 200,
            max_outer_iterations: 20,
            assignment: None,
            periodic: [true, true, false],
            beam_uncollided_split: true,
            transport_correction: true,
            p1_anisotropic: false,
            anisotropy_order: 0,
            anderson_depth: 0,
        }
    }

    /// Two-material data for the fraction-blend tests: "a" is the
    /// base, "b" the region material — distinguishable σ_t, scatter,
    /// and boron response so the blend is checkable.
    fn two_material_data() -> MultigroupData {
        let mut d = data(&[1.0, 0.5], vec![0.2, 0.0, 0.0, 0.1]);
        d.materials[0].material_id = "a".into();
        d.materials[0].dose_response_gy_cm2 =
            std::collections::BTreeMap::from([("boron".into(), vec![0.1, 0.2])]);
        d.materials[0].transport_mu_bar = Some(vec![0.4, 0.4]);
        d.materials.push(MultigroupMaterial {
            material_id: "b".into(),
            sigma_total_per_cm: vec![3.0, 1.5],
            scatter_matrix_per_cm: vec![0.4, 0.0, 0.0, 0.3],
            scatter_p1_matrix_per_cm: Some(vec![0.1, 0.0, 0.0, 0.1]),
            scatter_legendre_moments_per_cm: None,
            dose_response_gy_cm2: std::collections::BTreeMap::from([(
                "boron".into(),
                vec![0.5, 0.9],
            )]),
            transport_mu_bar: Some(vec![0.6, 0.6]),
        });
        d
    }

    fn fraction_assignment(case: &TransportCase) -> MaterialAssignment {
        MaterialAssignment {
            schema_version: MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: case.case_id.clone(),
            base_material: material("a"),
            regions: vec![MaterialRegion {
                name: "partial".into(),
                material: material("b"),
                shape: MaterialRegionShape::VoxelFractions {
                    indices: vec![[0, 0, 10], [1, 0, 10]],
                    fractions: vec![0.25, 0.5],
                },
            }],
            provenance_id: "test".into(),
        }
    }

    #[test]
    fn cell_compositions_blends_fractions_with_base_remainder() {
        let case = slab_case();
        let mut case = case;
        case.material = material("a");
        let data = two_material_data();
        let assignment = fraction_assignment(&case);
        let comps = cell_compositions(&case, &data, Some(&assignment)).unwrap();
        // Voxel [i,j,k] flattens as i + 4j + 16k → [0,0,10] = 160,
        // [1,0,10] = 161.
        let mut sig = comps[160].clone();
        sig.sort_by_key(|x| x.0);
        assert_eq!(sig, vec![(0, 0.75), (1, 0.25)]);
        assert_eq!(comps[161], vec![(0, 0.5), (1, 0.5)]);
        // Untouched cells are pure base.
        assert_eq!(comps[0], vec![(0, 1.0)]);
    }

    #[test]
    fn composition_map_synthesizes_blend_rows_with_weighted_tables() {
        let mut case = slab_case();
        case.material = material("a");
        let data = two_material_data();
        let assignment = fraction_assignment(&case);
        let (eff, case_material) =
            material_composition_map(&case, &data, Some(&assignment)).unwrap();
        // Two distinct blends → two appended materials.
        assert_eq!(eff.materials.len(), 4);
        // [1,0,10] is the 50/50 blend: σ_t = 0.5·[1,0.5] + 0.5·[3,1.5].
        let mi = case_material[161];
        let blend = &eff.materials[mi];
        assert!((blend.sigma_total_per_cm[0] - 2.0).abs() < 1e-12);
        assert!((blend.sigma_total_per_cm[1] - 1.0).abs() < 1e-12);
        // Scatter blends the same way.
        assert!((blend.scatter_matrix_per_cm[0] - 0.3).abs() < 1e-12);
        // Boron dose response: 0.5·0.1 + 0.5·0.5 = 0.3.
        let response = &blend.dose_response_gy_cm2["boron"];
        assert!((response[0] - 0.3).abs() < 1e-12);
        assert!((response[1] - 0.55).abs() < 1e-12);
        // μ̄ blends scatter-row-weighted: a's row sum = 0.2+0 = 0.2,
        // b's = 0.4+0 = 0.4 → (0.5·0.2·0.4 + 0.5·0.4·0.6)/(0.5·0.6).
        let mu = blend.transport_mu_bar.as_ref().unwrap()[0];
        let expected = (0.5 * 0.2 * 0.4 + 0.5 * 0.4 * 0.6) / (0.5 * 0.2 + 0.5 * 0.4);
        assert!((mu - expected).abs() < 1e-12);
        // The P1 matrix blends where present (a lacks it — absent
        // components contribute zero, matching the volume rule).
        assert!(blend.scatter_p1_matrix_per_cm.is_some());
        // The blend id is a deterministic signature.
        assert_eq!(blend.material_id, "a@0.500+b@0.500");
        // The 0.25 blend differs and is the other appended row.
        let mi25 = case_material[160];
        assert_ne!(mi25, mi);
        assert_eq!(eff.materials[mi25].material_id, "a@0.750+b@0.250");
    }

    #[test]
    fn composition_map_is_identity_without_fractions() {
        let mut case = slab_case();
        case.material = material("absorber");
        let data = data(&[1.0], vec![0.0]);
        let (eff, map) = material_composition_map(&case, &data, None).unwrap();
        assert_eq!(eff.materials.len(), 1);
        assert!(map.iter().all(|&m| m == 0));
    }

    #[test]
    fn quadrature_counts_and_symmetry() {
        for (order, expected) in [(2u32, 8usize), (4, 24), (6, 48), (8, 80)] {
            let quad = level_symmetric_quadrature(order).unwrap();
            assert_eq!(quad.len(), expected, "S{order}");
            let weight_sum: f64 = quad.iter().map(|(_, w)| w).sum();
            assert!((weight_sum - 4.0 * std::f64::consts::PI).abs() < 1e-12);
            for (d, _) in &quad {
                let norm: f64 = d.iter().map(|x| x * x).sum::<f64>().sqrt();
                assert!((norm - 1.0).abs() < 1e-12, "non-unit ordinate {d:?}");
            }
        }
        // Level symmetry: every octant reflection of an ordinate is present.
        let quad = level_symmetric_quadrature(4).unwrap();
        let dirs: std::collections::BTreeSet<String> = quad
            .iter()
            .map(|(d, _)| format!("{:.12},{:.12},{:.12}", d[0], d[1], d[2]))
            .collect();
        for (d, _) in &quad {
            for oct in 0..8u8 {
                let mirror = [
                    if oct & 1 == 0 { d[0] } else { -d[0] },
                    if oct & 2 == 0 { d[1] } else { -d[1] },
                    if oct & 4 == 0 { d[2] } else { -d[2] },
                ];
                assert!(dirs.contains(&format!(
                    "{:.12},{:.12},{:.12}",
                    mirror[0], mirror[1], mirror[2]
                )));
            }
        }
        assert!(level_symmetric_quadrature(3).is_err());
        assert!(level_symmetric_quadrature(0).is_err());
    }

    #[test]
    fn invalid_data_rejected() {
        // Scatter exceeds total.
        let mut bad = data(&[0.1], vec![0.2]);
        assert!(bad.validate().is_err());
        // Wrong matrix size.
        bad = data(&[0.1], vec![0.05, 0.05]);
        assert!(bad.validate().is_err());
        // Ascending group boundaries.
        bad = data(&[0.1], vec![0.0]);
        bad.energy_boundaries_ev = vec![1.0e-3, 1.0];
        assert!(bad.validate().is_err());
        // Negative cross section.
        bad = data(&[-0.1], vec![0.0]);
        assert!(bad.validate().is_err());
        // Valid baseline passes.
        data(&[0.1], vec![0.0]).validate().unwrap();
    }

    #[test]
    fn pure_absorber_uncollided_split_is_exact() {
        let case = slab_case();
        let sigma = 0.2308_f64; // cm^-1
        let mg = data(&[sigma], vec![0.0]);
        mg.validate().unwrap();
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        assert_eq!(flux.beam_model, "uncollided_split");

        // Normal beam, μ_z = 1: the uncollided component is the exact
        // exponential — per-cell ratio e^(−Σ_t·Δz) to machine precision.
        let dz = 0.1_f64; // cm
        let expected_ratio = (-sigma * dz).exp();
        let column = |k: usize| flux.flux[5 + 16 * k][0];
        for k in 0..18 {
            let ratio = column(k + 1) / column(k);
            assert!(
                (ratio - expected_ratio).abs() < 1e-9,
                "cell {k}: ratio {ratio} vs exact {expected_ratio}"
            );
        }
        let slope = -(column(18) / column(2)).ln() / (16.0 * dz);
        assert!(
            (slope - sigma).abs() / sigma < 1e-9,
            "slope {slope} vs analytic {sigma}"
        );
    }

    #[test]
    fn y_axis_disk_source_deposits_in_the_right_column() {
        // Regression: the boundary-source map and sweep must share the
        // canonical (u, v) order — for a Y-axis beam that is (x, z).
        // An off-diagonal disk center makes a transposed map land on
        // cells outside the beam instead of merely masking the error.
        let mut case = slab_case();
        case.source.space = SourceSpatialDistribution::UniformDisk {
            axis: PlaneAxis::Y,
            offset_cm: -0.2,             // y-low face
            center_uv_cm: [0.05, -0.05], // (x, z) — exactly cell (i=2, k=9)
            radius_cm: 0.05,
        };
        case.source.angle = AngularDistribution::Monodirectional {
            unit_vector: [0.0, 1.0, 0.0],
        };
        let mg = data(&[0.05_f64], vec![0.0]);
        mg.validate().unwrap();
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        // Beam column: cells (2, j, 9) for j = 0..3 carry the whole
        // pencil — and it must attenuate monotonically with depth.
        let column = |j: usize| flux.flux[2 + 4 * j + 16 * 9][0];
        for j in 0..4 {
            assert!(column(j) > 0.0, "beam cell j={j} carries no flux");
            if j > 0 {
                assert!(column(j) < column(j - 1), "beam must attenuate with depth");
            }
        }
        // A cell transverse to the beam — (0, j, 9) — sees no uncollided
        // flux: the disk center is at x = +0.05, x = −0.15 is outside.
        assert!(flux.flux[16 * 9][0] == 0.0);
    }

    #[test]
    fn histogram_source_uses_collapse_consistent_within_bin_weighting() {
        // A 1/E epithermal bin spread over sub-groups must concentrate
        // weight at the bin's LOW edge — the slowing-down convention —
        // not uniformly per eV.
        let mut case = slab_case();
        case.source.energy = EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: vec![0.5, 10_000.0],
            bin_weights: vec![1.0],
        };
        // 8 groups covering [0.5, 1e4] eV logarithmically-ish.
        let mut mg = data(&[0.5; 8], vec![0.0; 64]);
        mg.energy_boundaries_ev = vec![1e4, 3e3, 1e3, 3e2, 1e2, 3e1, 1e1, 3.0, 0.5];
        mg.validate().unwrap();
        let w = source_group_weights(&case.source, &mg).unwrap();
        assert!(w.iter().all(|x| x.is_finite() && *x >= 0.0));
        assert!((w.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        // 1/E weighting: equal per decade → lowest two groups (0.5–3 eV
        // and 3–10 eV, sub-decade) carry comparable weight to the
        // decade-wide groups; under uniform-per-eV the lowest group
        // would carry ~0.0003 of the bin. Assert the lowest group
        // carries >5% and exceeds the top group's share.
        assert!(
            w[7] > 0.05 && w[7] > w[0],
            "1/E low-end weighting: w_low={} w_high={}",
            w[7],
            w[0]
        );
    }

    #[test]
    fn narrow_cone_uncollided_split_approximates_monodirectional() {
        // A narrow isotropic cone takes the analytic uncollided path:
        // at the entry face its scalar fluence is I/μ̄ with
        // μ̄ = (1 + cos θ_h)/2 — within ~1% of the monodirectional
        // value for an 8.5° beam.
        let mut case = slab_case();
        let half_angle = 0.1491_f64;
        case.source.angle = AngularDistribution::IsotropicCone {
            axis_unit_vector: [0.0, 0.0, 1.0],
            half_angle_rad: half_angle,
        };
        let sigma = 0.2308_f64;
        let mg = data(&[sigma], vec![0.0]);
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        assert_eq!(flux.beam_model, "uncollided_split");

        let mono =
            solve_multigroup(&slab_case(), &mg, &options(), cref("mg"), cref("case")).unwrap();
        let mu_bar = (1.0 + half_angle.cos()) / 2.0;
        for k in 0..4 {
            let cone = flux.flux[5 + 16 * k][0];
            let ray = mono.flux[5 + 16 * k][0];
            // Near-entry cells: cone ≈ monodirectional up to the μ̄
            // normalization and the obliquity of the outer rays.
            let expected = ray / mu_bar;
            assert!(
                (cone - expected).abs() / expected < 0.02,
                "cell {k}: cone {cone} vs monodirectional/mu_bar {expected}"
            );
        }
    }

    #[test]
    fn transport_correction_deepens_scattered_flux() {
        // Two-group cascade with a forward-peaked upper group:
        // σ_t0 = 1.0, σ_s(0→0) = 0.10, σ_s(0→1) = 0.80, μ̄₀ = 0.7
        // → σ_t0,tr = 0.37, σ_s,tr(0→0) = 0 — corrected epithermal flux
        // penetrates farther and feeds the downscatter source deeper,
        // so the deep group-1 flux must exceed the uncorrected solve.
        let case = slab_case();
        // Scatter layout is [src * G + dst].
        let mut mg = data(&[1.0, 0.5], vec![0.10, 0.80, 0.0, 0.30]);
        mg.materials[0].transport_mu_bar = Some(vec![0.7, 0.0]);
        mg.validate().unwrap();

        let mut corrected_opts = options();
        corrected_opts.transport_correction = true;
        let mut raw_opts = options();
        raw_opts.transport_correction = false;
        let corrected =
            solve_multigroup(&case, &mg, &corrected_opts, cref("mg"), cref("case")).unwrap();
        let raw = solve_multigroup(&case, &mg, &raw_opts, cref("mg"), cref("case")).unwrap();
        assert!(corrected.converged && raw.converged);
        assert!(corrected.transport_correction);
        assert!(!raw.transport_correction);
        // Deep cell group-1 flux: the cascade is driven deeper by the
        // corrected epithermal penetration.
        let deep = 5 + 16 * 17;
        let near = 5 + 16;
        assert!(
            corrected.flux[deep][1] > raw.flux[deep][1],
            "corrected deep thermal {} !> raw {}",
            corrected.flux[deep][1],
            raw.flux[deep][1]
        );
        assert!(corrected.flux[near][0] > 0.0 && raw.flux[near][0] > 0.0);
    }

    #[test]
    fn transport_correction_reduces_self_scatter_consistently() {
        // Near-conservative medium: σ_s(g→g) = 0.4 vs σ_t = 0.5, μ̄ = 0.9.
        // A σ_t-only correction (σ_t,tr = 0.5 − 0.36 = 0.14 < σ_s = 0.4)
        // makes the in-group Jacobi diverge; the consistent correction
        // also removes μ̄Σ_s from the diagonal (σ_s,tr = 0.04 < σ_t,tr)
        // and must converge.
        let case = slab_case();
        let mut mg = data(&[0.5], vec![0.4]);
        mg.materials[0].transport_mu_bar = Some(vec![0.9]);
        mg.validate().unwrap();
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        assert!(flux.transport_correction);
    }

    #[test]
    fn p1_zero_matrix_is_p0_equivalent() {
        // P1 enabled but every moment is zero — the anisotropic source
        // vanishes and the solve must reproduce the uncorrected P0
        // result exactly (and disable the transport correction).
        let case = slab_case();
        let mut mg = data(&[1.0, 0.5], vec![0.10, 0.80, 0.0, 0.30]);
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![0.0; 4]);
        mg.materials[0].transport_mu_bar = Some(vec![0.7, 0.0]);
        mg.validate().unwrap();
        let mut p1_opts = options();
        p1_opts.p1_anisotropic = true;
        let mut p0_opts = options();
        p0_opts.transport_correction = false;
        let p1 = solve_multigroup(&case, &mg, &p1_opts, cref("mg"), cref("case")).unwrap();
        let p0 = solve_multigroup(&case, &mg, &p0_opts, cref("mg"), cref("case")).unwrap();
        assert!(p1.converged && p0.converged);
        assert_eq!(p1.scattering_order(), "p1");
        assert_eq!(p0.scattering_order(), "p0");
        for (a, b) in p1.flux.iter().zip(&p0.flux) {
            for (x, y) in a.iter().zip(b) {
                assert_eq!(x, y, "P1 with zero moments diverged from P0");
            }
        }
    }

    #[test]
    fn p1_requires_moments_on_scattering_materials() {
        let case = slab_case();
        // Scatter > 0 but no P1 table — must be refused, not silently
        // downgraded to isotropic.
        let mg = data(&[1.0], vec![0.5]);
        let mut opts = options();
        opts.p1_anisotropic = true;
        let err = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap_err();
        assert!(format!("{err}").contains("scatter_p1"));
    }

    #[test]
    fn p1_forward_peak_deepens_penetration() {
        // Same cascade as the transport-correction test:
        // σ_t = [1.0, 0.5], Σ_s = [[0.10, 0.80], [0, 0.30]] (src, dst).
        // Give every transfer a forward mean cosine μ̄ = 0.7 — the P1
        // source term 3·Ω_z·Σ_s1·J_z adds a downbeam-biased source, so
        // the collided field must penetrate deeper than P0.
        let case = slab_case();
        let scatter = vec![0.10, 0.80, 0.0, 0.30];
        let p1: Vec<f64> = scatter.iter().map(|s| s * 0.7).collect();
        let mut mg = data(&[1.0, 0.5], scatter);
        mg.materials[0].scatter_p1_matrix_per_cm = Some(p1);
        mg.validate().unwrap();
        let mut p1_opts = options();
        p1_opts.p1_anisotropic = true;
        let mut p0_opts = options();
        p0_opts.transport_correction = false;
        let p1f = solve_multigroup(&case, &mg, &p1_opts, cref("mg"), cref("case")).unwrap();
        let p0f = solve_multigroup(&case, &mg, &p0_opts, cref("mg"), cref("case")).unwrap();
        assert!(p1f.converged && p0f.converged);
        let deep = 5 + 16 * 17;
        assert!(
            p1f.flux[deep][1] > p0f.flux[deep][1],
            "P1 deep thermal {} !> P0 {}",
            p1f.flux[deep][1],
            p0f.flux[deep][1]
        );
        // The same forward bias also deepens the epithermal group.
        assert!(
            p1f.flux[deep][0] > p0f.flux[deep][0],
            "P1 deep epi {} !> P0 {}",
            p1f.flux[deep][0],
            p0f.flux[deep][0]
        );
    }

    #[test]
    fn scatter_p1_bounded_by_p0_row() {
        // |Σ_s1(g→g')| ≤ Σ_s0(g→g') — the P1 moment is a cosine-weighted
        // P0 moment; an unphysical entry must fail validation.
        let mut mg = data(&[1.0], vec![0.5]);
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![0.6]);
        assert!(mg.validate().is_err());
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![0.5]);
        mg.validate().unwrap();
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![-0.5]);
        mg.validate().unwrap();
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![f64::NAN]);
        assert!(mg.validate().is_err());
    }

    #[test]
    fn pure_absorber_boundary_flux_matches_diamond_difference() {
        // With the split disabled the beam rides the nearest ordinate —
        // the DD sweep must reproduce the Padé per-cell ratio along ξ.
        let case = slab_case();
        let sigma = 0.2308_f64;
        let mg = data(&[sigma], vec![0.0]);
        let mut opts = options();
        opts.beam_uncollided_split = false;
        let flux = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        assert_eq!(flux.beam_model, "boundary_flux");

        let xi = 0.8688903007222012_f64;
        let dz = 0.1_f64;
        let expected_ratio = (2.0 * xi - sigma * dz) / (2.0 * xi + sigma * dz);
        let column = |k: usize| flux.flux[5 + 16 * k][0];
        for k in 2..18 {
            let ratio = column(k + 1) / column(k);
            assert!(
                (ratio - expected_ratio).abs() < 1e-9,
                "cell {k}: ratio {ratio} vs DD Padé {expected_ratio}"
            );
        }
    }

    #[test]
    fn downscatter_populates_lower_group() {
        let case = slab_case();
        // Group 0 → group 1 only; group 1 has self-scatter + absorption.
        // Σ_s[0→1] = 0.2 of Σ_t0 = 0.3; g1: Σ_t = 0.4, self-scatter 0.1.
        let mg = data(
            &[0.3, 0.4],
            vec![
                0.05, 0.2, // from g0: self 0.05, down 0.2
                0.0, 0.1, // from g1: no up, self 0.1
            ],
        );
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        let deep = 5 + 304;
        assert!(flux.flux[deep][0] > 0.0);
        assert!(flux.flux[deep][1] > 0.0, "downscatter must populate g1");
        // g1 builds with depth before attenuating — interior g1 flux
        // exceeds the boundary-adjacent cell's.
        let near = 5 + 16 * 2;
        assert!(flux.flux[deep][1] > 0.0);
        assert!(flux.flux[near][0] > flux.flux[deep][0], "g0 attenuates");
    }

    #[test]
    fn upscatter_needs_outer_iteration() {
        let case = slab_case();
        // Upscatter g1→g0 forces the outer re-sweep.
        let mg = data(&[0.3, 0.4], vec![0.05, 0.1, 0.1, 0.1]);
        let mut opts = options();
        opts.max_outer_iterations = 1;
        let flux = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert!(!flux.converged, "one outer pass cannot converge upscatter");
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
    }

    #[test]
    fn anderson_accelerates_upscatter_convergence() {
        let case = slab_case();
        // Strongly-coupled 2-group system: most of each group's scatter
        // crosses the boundary in both directions — the
        // weakly-contractive regime S(α,β) data creates. Plain sweeps
        // converge slowly; Anderson should need far fewer cycles.
        let mg = data(
            &[0.5, 0.5],
            vec![
                0.05, 0.35, // g0: self + down to g1
                0.35, 0.05, // g1: up to g0
            ],
        );
        let mut plain = options();
        plain.max_outer_iterations = 60;
        let slow = solve_multigroup(&case, &mg, &plain, cref("mg"), cref("case")).unwrap();
        let mut accel = plain.clone();
        accel.anderson_depth = 3;
        let fast = solve_multigroup(&case, &mg, &accel, cref("mg"), cref("case")).unwrap();
        assert!(slow.converged && fast.converged);
        assert!(
            fast.outer_iterations <= slow.outer_iterations,
            "anderson should not be slower: {} vs {}",
            fast.outer_iterations,
            slow.outer_iterations
        );
        // Same fixed point: accelerated flux matches the plain solve.
        for (a, b) in fast.flux.iter().flatten().zip(slow.flux.iter().flatten()) {
            assert!(
                (a - b).abs() <= b.abs().max(1e-30) * 1e-5 + 1e-12,
                "accelerated flux diverged: {a} vs {b}"
            );
        }
    }

    #[test]
    fn legendre_polynomial_values_and_orthogonality() {
        // Spot values.
        assert!((legendre_p(0, 0.7) - 1.0).abs() < 1e-15);
        assert!((legendre_p(1, 0.7) - 0.7).abs() < 1e-15);
        assert!((legendre_p(2, 0.5) - (3.0 * 0.25 - 1.0) / 2.0).abs() < 1e-15);
        // P3(1) = 1, P4(0) = 3/8.
        assert!((legendre_p(3, 1.0) - 1.0).abs() < 1e-15);
        assert!((legendre_p(4, 0.0) - 0.375).abs() < 1e-15);
        // Parity: P_l(−x) = (−1)^l P_l(x).
        for l in 1..=5 {
            assert!(
                (legendre_p(l, -0.3) - (-1.0f64).powi(l as i32) * legendre_p(l, 0.3)).abs() < 1e-14
            );
        }
    }

    #[test]
    fn kernel_eigenbasis_recovers_2l_plus_1_modes() {
        let quad = level_symmetric_quadrature(4).unwrap();
        for l in 1..=5u32 {
            let basis = kernel_eigenbasis(&quad, l);
            // The continuous P_l kernel has 2l+1 modes; the discrete
            // quadrature resolves at most that many — high-l modes may
            // fall below the quadrature's polynomial coverage and drop
            // to numerically-zero eigenvalues (the basis never invents
            // modes the quadrature cannot represent).
            assert!(
                basis.len() <= 2 * l as usize + 1,
                "P_{l} kernel carried {} modes — exceeds 2l+1",
                basis.len()
            );
            assert!(
                basis.iter().all(|(lam, _)| *lam != 0.0),
                "P_{l} basis carries a zero-eigenvalue mode"
            );
            // The weight-symmetrized kernel's trace is
            // Σ_a w_a·P_l(1) = Σ_a w_a = 4π.
            let trace: f64 = basis.iter().map(|(lam, _)| lam).sum();
            let expected: f64 = quad.iter().map(|(_, w)| w).sum();
            assert!(
                (trace - expected).abs() < 1e-8,
                "P_{l} eigenvalue sum {trace} != Σw {expected}"
            );
        }
    }

    #[test]
    fn anisotropy_order_requires_p1_and_moments() {
        let mut case = slab_case();
        case.material = material("absorber");
        let mut mg = data(&[0.5, 0.5], vec![0.1, 0.05, 0.05, 0.1]);
        // l = 2 requested without P1 → error.
        let mut opts = options();
        opts.anisotropy_order = 2;
        let err = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap_err();
        assert!(err.to_string().contains("p1"), "{err}");
        // With P1 but no moment tables → error naming the requirement.
        opts.p1_anisotropic = true;
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![0.0; 4]);
        let err = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap_err();
        assert!(err.to_string().contains("legendre"), "{err}");
        // Supplying l = 2..=5 moments passes validation and solves.
        mg.materials[0].scatter_legendre_moments_per_cm =
            Some(vec![vec![0.0; 4], vec![0.0; 4], vec![0.0; 4], vec![0.0; 4]]);
        opts.anisotropy_order = 3;
        let flux = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
    }

    #[test]
    fn legendre_l2_solve_converges_and_corrects() {
        // Nonzero l = 2 moments exercise the kernel source — a
        // mis-normalized source (e.g. dropping the 1/(4π) per-sr
        // convention) makes the inner iteration diverge outright.
        let mut case = slab_case();
        case.material = material("absorber");
        let mut mg = data(&[0.5, 0.5], vec![0.2, 0.1, 0.05, 0.15]);
        mg.materials[0].scatter_p1_matrix_per_cm = Some(vec![0.05, 0.0, 0.0, 0.03]);
        // l = 2 moments ≈ a quarter of the P0 rows — a real but modest
        // forward-anisotropy correction.
        mg.materials[0].scatter_legendre_moments_per_cm = Some(vec![
            vec![0.04, 0.01, 0.0, 0.05],
            vec![0.0; 4],
            vec![0.0; 4],
            vec![0.0; 4],
        ]);
        let mut opts = options();
        opts.p1_anisotropic = true;
        opts.anisotropy_order = 2;
        let flux = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert!(
            flux.converged,
            "l=2 solve failed to converge (residual {:.3e})",
            flux.residual
        );
        assert!(
            flux.flux
                .iter()
                .flatten()
                .all(|v| v.is_finite() && *v >= 0.0)
        );
        // P1-only reference: the l=2 correction is small (<10% per cell).
        let mut ref_opts = options();
        ref_opts.p1_anisotropic = true;
        let reference = solve_multigroup(&case, &mg, &ref_opts, cref("mg"), cref("case")).unwrap();
        for (cell, row) in flux.flux.iter().enumerate() {
            for (g, &v) in row.iter().enumerate() {
                let r = reference.flux[cell][g];
                if r > 1e-12 {
                    assert!(
                        (v - r).abs() / r < 0.10,
                        "l=2 moved cell {cell} group {g} by {:.1}%",
                        (v - r).abs() / r * 100.0
                    );
                }
            }
        }
    }

    #[test]
    fn anderson_depth_zero_is_plain() {
        let case = slab_case();
        let mg = data(&[0.3, 0.4], vec![0.05, 0.1, 0.1, 0.1]);
        let plain = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        let mut opts = options();
        opts.anderson_depth = 0;
        let same = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert_eq!(plain.outer_iterations, same.outer_iterations);
        assert_eq!(plain.flux, same.flux);
    }

    #[test]
    fn vacuum_boundaries_leak() {
        // Partial disk coverage + scattering: flux must spread laterally
        // but leak freely — no artificial containment.
        let mut case = slab_case();
        if let SourceSpatialDistribution::UniformDisk { radius_cm, .. } = &mut case.source.space {
            *radius_cm = 0.09; // covers only the 4 center cells
        }
        let mg = data(&[0.3], vec![0.2]); // strongly scattering single group
        let mut opts = options();
        opts.periodic = [false; 3]; // all-vacuum faces
        let flux = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);
        // Near the source face the covered beam column dominates the
        // uncovered corner; by mid-depth scattering has spread flux to
        // the corner (and the oblique beam has drifted out the sides).
        let near = 16;
        let center = flux.flux[5 + near][0];
        let corner = flux.flux[near][0];
        assert!(center > corner, "beam column {center} vs corner {corner}");
        assert!(flux.flux[160][0] > 0.0, "scatter spreads flux laterally");
    }

    #[test]
    fn off_face_source_rejected() {
        let mut case = slab_case();
        if let SourceSpatialDistribution::UniformDisk { offset_cm, .. } = &mut case.source.space {
            *offset_cm = 0.0; // interior — not a boundary face
        }
        let mg = data(&[0.1], vec![0.0]);
        assert!(matches!(
            solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")),
            Err(MultigroupError::Source(_))
        ));
    }

    #[test]
    fn flux_fold_requires_declared_response() {
        let case = slab_case();
        let mg = data(&[0.1], vec![0.0]);
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        // No dose_response declared → folding must fail honestly.
        assert!(
            fold_multigroup_dose(&case, &mg, &flux, None, cref("profile"), cref("rs")).is_err()
        );

        // With declared boron response the fold produces a valid bundle.
        let mut mg = data(&[0.1], vec![0.0]);
        mg.materials[0]
            .dose_response_gy_cm2
            .insert("boron".into(), vec![1.0e-9]);
        let bundle =
            fold_multigroup_dose(&case, &mg, &flux, None, cref("profile"), cref("rs")).unwrap();
        assert_eq!(bundle.components.len(), 1);
        assert_eq!(
            bundle.components[0].component,
            openbnct_core::DoseComponent::Boron
        );
        let mid = 5 + 160;
        let expected = flux.flux[mid][0] * 1.0e-9;
        assert!((bundle.components[0].values[mid] - expected).abs() < expected * 1e-12);
        bundle.validate().unwrap_err(); // missing the other 3 components — honest
    }
}

#[cfg(test)]
mod heterogeneous_tests {
    use super::tests::{cref, slab_case};
    use super::*;
    use crate::model::{
        MATERIAL_ASSIGNMENT_SCHEMA, MaterialAssignment, MaterialDefinition, MaterialRegion,
        NeutronThermalTreatment, NuclideMassFraction,
    };

    fn material(id: &str) -> MaterialDefinition {
        MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: id.into(),
            density_g_cm3: 1.0,
            temperature_k: 294.0,
            nuclides: vec![NuclideMassFraction {
                name: "B10".into(),
                mass_fraction: 1.0,
            }],
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        }
    }

    /// A high-Σ insert at k ∈ [8,12) must steepen the attenuation slope
    /// there and restore the base slope past it.
    #[test]
    fn heterogeneous_insert_steepens_then_restores() {
        let case = slab_case();
        let mg = MultigroupData {
            schema_version: MULTIGROUP_DATA_SCHEMA.into(),
            id: "mg-het".into(),
            energy_boundaries_ev: vec![1.0, 1.0e-3],
            collapse_declaration: "test".into(),
            component_profile: None,
            materials: vec![
                MultigroupMaterial {
                    material_id: "absorber".into(),
                    sigma_total_per_cm: vec![0.1],
                    scatter_matrix_per_cm: vec![0.0],
                    scatter_p1_matrix_per_cm: None,
                    scatter_legendre_moments_per_cm: None,
                    dose_response_gy_cm2: Default::default(),
                    transport_mu_bar: None,
                },
                MultigroupMaterial {
                    material_id: "insert".into(),
                    sigma_total_per_cm: vec![1.0],
                    scatter_matrix_per_cm: vec![0.0],
                    scatter_p1_matrix_per_cm: None,
                    scatter_legendre_moments_per_cm: None,
                    dose_response_gy_cm2: Default::default(),
                    transport_mu_bar: None,
                },
            ],
        };
        let assignment = MaterialAssignment {
            schema_version: MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: case.case_id.clone(),
            base_material: material("absorber"),
            regions: vec![MaterialRegion {
                name: "insert".into(),
                material: material("insert"),
                shape: MaterialRegionShape::VoxelBox {
                    lower: [0, 0, 8],
                    upper: [3, 3, 11],
                },
            }],
            provenance_id: "test".into(),
        };
        let mut opts = super::tests::options();
        opts.assignment = Some(assignment);
        let flux = solve_multigroup(&case, &mg, &opts, cref("mg"), cref("case")).unwrap();
        assert!(flux.converged);

        let column = |k: usize| flux.flux[5 + 16 * k][0];
        // Pure-absorber + uncollided split: exact exponential per region.
        let base_ratio = (-0.1_f64 * 0.1).exp();
        let insert_ratio = (-0.1_f64).exp();
        let r_base = column(3) / column(2);
        let r_insert = column(10) / column(9);
        let r_after = column(17) / column(16);
        assert!((r_base - base_ratio).abs() < 1e-9, "base {r_base}");
        assert!((r_insert - insert_ratio).abs() < 1e-9, "insert {r_insert}");
        assert!((r_after - base_ratio).abs() < 1e-9, "after {r_after}");
    }
}

#[cfg(test)]
mod artifact_tests {
    use super::*;

    /// The committed NF-BNCT-003 multigroup fixture must deserialize and
    /// validate through the real contract.
    #[test]
    fn committed_nf_bnct_003_multigroup_data_validates() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../benchmarks/synthetic/nf-bnct-003/transport/multigroup-data.json"
        );
        let bytes = std::fs::read(path).expect("committed benchmark artifact");
        let data: MultigroupData = serde_json::from_slice(&bytes).unwrap();
        data.validate().unwrap();
        assert_eq!(data.group_count(), 1);
        assert_eq!(
            data.materials[0].material_id,
            "openbnct.nf-bnct-003.material.ideal-b10-absorber.v1"
        );
    }

    /// The committed NF-BNCT-003 covariance artifact must deserialize,
    /// validate, and content-bind to the committed multigroup data.
    #[test]
    fn committed_nf_bnct_003_covariance_validates() {
        use sha2::Digest;
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../benchmarks/synthetic/nf-bnct-003/transport/"
        );
        let data_bytes = std::fs::read(format!("{base}multigroup-data.json")).unwrap();
        let cov_bytes = std::fs::read(format!("{base}multigroup-covariance.json")).unwrap();
        let cov: crate::uq::MultigroupCovariance = serde_json::from_slice(&cov_bytes).unwrap();
        cov.validate().unwrap();
        let sha: String = sha2::Sha256::digest(&data_bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(cov.multigroup_data.sha256, sha);
    }

    /// The committed NF-BNCT-003 screening spec must deserialize,
    /// validate, and content-bind to the committed case + data.
    #[test]
    fn committed_nf_bnct_003_sensitivity_spec_validates() {
        use sha2::Digest;
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../benchmarks/synthetic/nf-bnct-003/transport/"
        );
        let hex = |bytes: &[u8]| -> String {
            sha2::Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let case_bytes = std::fs::read(format!("{base}case.json")).unwrap();
        let data_bytes = std::fs::read(format!("{base}multigroup-data.json")).unwrap();
        let spec_bytes = std::fs::read(format!("{base}sensitivity-spec.json")).unwrap();
        let spec: crate::screening::SensitivitySpec = serde_json::from_slice(&spec_bytes).unwrap();
        spec.validate().unwrap();
        assert_eq!(spec.case.sha256, hex(&case_bytes));
        assert_eq!(spec.multigroup_data.sha256, hex(&data_bytes));
    }

    /// The committed FiR 1 cylindrical-phantom deterministic-validation
    /// fixtures must deserialize and validate: the transport case, its
    /// voxel-set material assignment, the declared three-group data, and
    /// the measurement-comparison report — for both the water and PMMA
    /// phantom compositions.
    #[test]
    fn committed_fir1_cylindrical_fixtures_validate() {
        for (dir, comparison_file) in [
            (
                "fir1-k63-cylindrical-phantom",
                "measurement-comparison-cylindrical.json",
            ),
            ("fir1-k63-pmma-phantom", "measurement-comparison-pmma.json"),
        ] {
            let base = format!("{}/../../validation/{dir}/", env!("CARGO_MANIFEST_DIR"));
            let case: crate::model::TransportCase =
                serde_json::from_slice(&std::fs::read(format!("{base}case.json")).unwrap())
                    .unwrap();
            case.validate().unwrap();
            let assignment: crate::model::MaterialAssignment =
                serde_json::from_slice(&std::fs::read(format!("{base}assignment.json")).unwrap())
                    .unwrap();
            assignment.validate(&case.geometry).unwrap();
            let data: MultigroupData = serde_json::from_slice(
                &std::fs::read(format!("{base}multigroup-data.json")).unwrap(),
            )
            .unwrap();
            data.validate().unwrap();
            assert_eq!(data.group_count(), 3);
            let comparison: crate::measurement::MeasurementComparisonReport =
                serde_json::from_slice(&std::fs::read(format!("{base}{comparison_file}")).unwrap())
                    .unwrap();
            comparison.validate().unwrap();
            assert!(
                comparison
                    .profile_comparisons
                    .iter()
                    .all(|p| p.passed == Some(true)),
                "{dir} profile comparison must pass"
            );
        }
    }
}
