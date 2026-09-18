// SPDX-License-Identifier: Apache-2.0

//! CADIS / FW-CADIS weight-window derivation from the in-house adjoint
//! S_N solve (`openbnct.weight-windows` output, transport-neutral input).
//!
//! The adjoint scalar flux `φ†(r,g)` is the importance of a particle
//! born at `(r,g)` toward a declared response. Classical CADIS
//! (Haghighat & Wagner) sets the local target weight to `w₀ = R/φ†`
//! where `R` normalizes the response; here `w_ref` is instead normalized
//! so that particles emitted by the declared source are born at unit
//! weight at their local target — consistent with the unbiased
//! fixed sources this code base emits. The lower bound is
//! `w₀ / survival_ratio` and the upper bound `w₀`, so a particle
//! rouletted up from below lands exactly on the local target — the
//! CADIS-consistent window shape.
//!
//! FW-CADIS replaces the detector response with the forward-flux-
//! weighted adjoint source `q† = response/φ_fwd`, flattening the
//! transported population for mesh-global tallies.
//!
//! Cells whose adjoint flux is zero or below numerical noise are capped
//! at `target_cap` times the source weight — a kill window, which is the
//! honest CADIS treatment of zero-importance regions (a disabled window
//! would let particles wander unproductively).
//!
//! Everything here is research-verification machinery: the windows are
//! only as good as the declared multigroup data, the P0 scattering
//! model, and the S_N convergence behind them.

use openbnct_core::ContentReference;
use serde::Serialize;
use thiserror::Error;

use crate::model::{ParticleType, TransportCase};
use crate::multigroup::{
    MultigroupData, MultigroupError, MultigroupFlux, SnOptions, material_composition_map,
    solve_multigroup_adjoint, source_coverage,
};
use crate::variance_reduction::{
    AdjointMethod, AdjointResponse, ResolvedWeightWindow, ResolvedWeightWindows,
    VarianceReductionError, VarianceReductionSpec, WEIGHT_WINDOWS_SCHEMA, WeightWindowBounds,
    WeightWindowDerivation,
};

/// Target weights are clamped to this multiple of the unit source
/// weight unless the window's `adjoint` bounds declare `target_cap`
/// otherwise. The cap doubles as a roulette-kill window in cells of
/// zero adjoint flux.
pub const DEFAULT_TARGET_CAP: f64 = 1.0e6;

/// The resolved windows plus the adjoint flux field(s) they were
/// derived from — one entry per `adjoint`-bounds window, in window
/// order, for optional serialization and content binding.
#[derive(Debug)]
pub struct AdjointDerivation {
    pub resolved: ResolvedWeightWindows,
    pub adjoint_fluxes: Vec<MultigroupFlux>,
}

#[derive(Debug, Error)]
pub enum CadisError {
    #[error("{0}")]
    Contract(#[from] VarianceReductionError),
    #[error("{0}")]
    Solve(#[from] MultigroupError),
    #[error("adjoint window {index}: {reason}")]
    Window { index: usize, reason: String },
    #[error(
        "forward_flux bounds resolve from an OpenMC statepoint via `vr resolve`; \
         `vr cadis` handles uniform, explicit, and adjoint bounds"
    )]
    ForwardFluxNeedsStatepoint,
    #[error("fw_cadis requires a forward multigroup flux (--forward-flux)")]
    MissingForwardFlux,
    #[error("geometry: {0}")]
    Geometry(#[from] openbnct_core::ValidationError),
}

/// Resolve a variance-reduction spec through the in-house adjoint
/// solver. `uniform`/`explicit` windows pass through unchanged;
/// `adjoint` windows are solved and filled; `forward_flux` is refused —
/// that path needs a Monte Carlo statepoint and stays with
/// `openmc::resolve_weight_windows`.
///
/// `forward_flux` is required iff any window declares
/// `method = "fw_cadis"`; its artifact reference is recorded in the
/// derivation.
#[allow(clippy::too_many_arguments)]
pub fn resolve_adjoint_windows(
    spec: &VarianceReductionSpec,
    spec_sha256: &str,
    resolved_id: &str,
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    forward_flux: Option<(&MultigroupFlux, ContentReference)>,
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<AdjointDerivation, CadisError> {
    spec.validate()?;
    if let Some(spec_case) = &spec.case_id
        && *spec_case != case.case_id
    {
        return Err(VarianceReductionError::CaseMismatch {
            spec: spec.id.clone(),
            spec_case: spec_case.clone(),
            case: case.case_id.clone(),
        }
        .into());
    }

    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let groups = data.group_count();
    let (data_eff, case_material) =
        material_composition_map(case, data, options.assignment.as_ref())?;
    let data = &data_eff;
    // Source birth cells and group weights — the w_ref normalization.
    let coverage = source_coverage(case, data)?;
    let src_linear = face_cells_linear(geometry, coverage.face, &coverage.cells);
    let src_group_weights = coverage.group_weights;

    let mut windows = Vec::with_capacity(spec.windows.len());
    let mut adjoint_fluxes = Vec::new();
    let mut methods: Vec<String> = Vec::with_capacity(spec.windows.len());
    let mut notes: Vec<String> = Vec::new();
    for (index, window) in spec.windows.iter().enumerate() {
        let (mut lower, mut upper) = match &window.bounds {
            WeightWindowBounds::Uniform { lower_bound } => {
                methods.push("uniform".into());
                let cells = window.energy_groups() * window.mesh.n_bins();
                let lower = vec![*lower_bound; cells];
                let upper = lower
                    .iter()
                    .map(|l| l * window.parameters.survival_ratio)
                    .collect();
                notes.push(format!("window {index} uniform lower={lower_bound}"));
                (lower, upper)
            }
            WeightWindowBounds::Explicit {
                lower_bounds,
                upper_bounds,
            } => {
                methods.push("explicit".into());
                notes.push(format!("window {index} explicit bounds"));
                (lower_bounds.clone(), upper_bounds.clone())
            }
            WeightWindowBounds::ForwardFlux { .. } => {
                return Err(CadisError::ForwardFluxNeedsStatepoint);
            }
            WeightWindowBounds::Adjoint {
                method, response, ..
            } => {
                check_adjoint_window(index, window, case, data)?;
                let base_response =
                    base_response(case, data, &case_material, response, n_cells, groups, index)?;
                let adjoint_source = match method {
                    AdjointMethod::Cadis => base_response,
                    AdjointMethod::FwCadis => {
                        let Some((flux, _)) = forward_flux else {
                            return Err(CadisError::MissingForwardFlux);
                        };
                        check_forward_flux(flux, data, n_cells, groups, index)?;
                        fw_adjoint_source(&base_response, &flux.flux, groups)
                    }
                };
                let adjoint = solve_multigroup_adjoint(
                    case,
                    data,
                    options,
                    &adjoint_source,
                    data_ref.clone(),
                    case_ref.clone(),
                )?;
                if !adjoint.converged {
                    notes.push(format!(
                        "window {index}: adjoint solve did not converge \
                         (residual {:.3e} after {} outer iterations)",
                        adjoint.residual, adjoint.outer_iterations
                    ));
                }
                let cap = window.bounds.target_cap().unwrap_or(DEFAULT_TARGET_CAP);
                let (lower, upper, w_ref, capped) = window_bounds_from_importance(
                    &adjoint.flux,
                    &src_linear,
                    &src_group_weights,
                    window.parameters.survival_ratio,
                    cap,
                    groups,
                )?;
                methods.push(match method {
                    AdjointMethod::Cadis => "cadis".into(),
                    AdjointMethod::FwCadis => "fw_cadis".into(),
                });
                notes.push(format!(
                    "window {index} {}: response={:?}, w_ref={w_ref:.6e}, \
                     target_cap={cap:.1e}, capped_cells={capped}, s_N={}",
                    match method {
                        AdjointMethod::Cadis => "cadis",
                        AdjointMethod::FwCadis => "fw_cadis",
                    },
                    response,
                    options.quadrature_order
                ));
                adjoint_fluxes.push(adjoint);
                (lower, upper)
            }
        };
        let boosted = window.apply_bound_boosts(&mut lower, &mut upper);
        if boosted > 0 {
            notes.push(format!("window {index}: {boosted} cell(s) bound-boosted"));
        }
        windows.push(ResolvedWeightWindow {
            particle: window.particle,
            mesh: window.mesh.clone(),
            energy_bounds_ev: window.energy_bounds_ev.clone(),
            lower_bounds: lower,
            upper_bounds: upper,
            parameters: window.parameters,
        });
    }

    let method = if methods.iter().all(|m| *m == methods[0]) {
        methods[0].clone()
    } else {
        "mixed".to_string()
    };
    let resolved = ResolvedWeightWindows {
        schema_version: WEIGHT_WINDOWS_SCHEMA.into(),
        id: resolved_id.to_string(),
        case_id: spec.case_id.clone(),
        spec: ContentReference {
            id: spec.id.clone(),
            sha256: spec_sha256.to_string(),
        },
        windows,
        derivation: WeightWindowDerivation {
            method,
            source_statepoint: None,
            adjoint_flux: Vec::new(),
            forward_flux: forward_flux.map(|(_, r)| r),
            multigroup_data: Some(data_ref),
            transport_case: Some(case_ref),
            note: notes.join("; "),
        },
        qualification: format!(
            "{}; adjoint-derived windows are research-verification machinery \
             limited by the declared multigroup data and P0 S_N model",
            spec.qualification
        ),
    };
    resolved.validate()?;
    Ok(AdjointDerivation {
        resolved,
        adjoint_fluxes,
    })
}

/// Cell linear indices on a boundary face for transverse `(ju, jv)`
/// columns — the cells source particles are born into.
fn face_cells_linear(
    geometry: &openbnct_core::GridGeometry,
    face: u8,
    cells: &[(u32, u32)],
) -> Vec<usize> {
    let axis = (face / 2) as usize;
    let high = face % 2 == 1;
    let [nx, ny, nz] = geometry.shape.map(|d| d as usize);
    let shape = [nx, ny, nz];
    let along = if high { shape[axis] - 1 } else { 0 };
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    cells
        .iter()
        .map(|(ju, jv)| {
            let mut voxel = [0usize; 3];
            voxel[axis] = along;
            voxel[u] = *ju as usize;
            voxel[v] = *jv as usize;
            voxel[0] + nx * voxel[1] + nx * ny * voxel[2]
        })
        .collect()
}

/// The response part of the adjoint source `q†[cell][group]` before any
/// FW-CADIS forward-flux division.
fn base_response(
    case: &TransportCase,
    data: &MultigroupData,
    case_material: &[usize],
    response: &AdjointResponse,
    n_cells: usize,
    groups: usize,
    index: usize,
) -> Result<Vec<Vec<f64>>, CadisError> {
    let invalid = |reason: String| CadisError::Window { index, reason };
    let source = match response {
        AdjointResponse::DoseComponent { component } => {
            let mut q = vec![vec![0.0; groups]; n_cells];
            for (cell, row) in q.iter_mut().enumerate() {
                let material = &data.materials[case_material[cell]];
                if let Some(resp) = material.dose_response_gy_cm2.get(component) {
                    row.copy_from_slice(resp);
                }
            }
            q
        }
        AdjointResponse::VoxelBox { lower, upper } => {
            let [nx, ny, _] = case.geometry.shape.map(|d| d as usize);
            let mut q = vec![vec![0.0; groups]; n_cells];
            for k in lower[2]..=upper[2] {
                for j in lower[1]..=upper[1] {
                    for i in lower[0]..=upper[0] {
                        let cell = i as usize + nx * j as usize + nx * ny * k as usize;
                        if cell < n_cells {
                            q[cell].fill(1.0);
                        }
                    }
                }
            }
            q
        }
        AdjointResponse::Global => vec![vec![1.0; groups]; n_cells],
    };
    if source.iter().all(|row| row.iter().all(|v| *v <= 0.0)) {
        return Err(invalid(
            "adjoint response produced an all-zero source — check the \
             component name, region, or dose-response declarations"
                .into(),
        ));
    }
    Ok(source)
}

/// `q† = response / (φ_fwd + floor)` — the FW-CADIS adjoint source.
/// The per-group floor is `1e-10 × group max`, bounding the source in
/// cells the forward flux never reached.
fn fw_adjoint_source(base: &[Vec<f64>], forward_flux: &[Vec<f64>], groups: usize) -> Vec<Vec<f64>> {
    let mut floor = vec![0.0_f64; groups];
    for row in forward_flux {
        for (g, f) in floor.iter_mut().enumerate() {
            if row[g] > *f {
                *f = row[g];
            }
        }
    }
    base.iter()
        .zip(forward_flux.iter())
        .map(|(resp, flux)| {
            (0..groups)
                .map(|g| resp[g] / (flux[g] + 1e-10_f64 * floor[g].max(1e-300)))
                .collect()
        })
        .collect()
}

/// CADIS window bounds from a converged adjoint scalar flux.
///
/// `w_ref` is the source-weighted mean adjoint flux over the source
/// cells — the importance of a freshly born particle — so targets are
/// `w₀ = w_ref / φ†`, unity at the source on average. `w₀` is clamped
/// to `[tiny, cap]`; the cap makes zero-importance cells a kill window.
/// Returns `(lower, upper, w_ref, capped_cell_count)` with
/// `lower = w₀/survival_ratio`, `upper = w₀`.
fn window_bounds_from_importance(
    adjoint_flux: &[Vec<f64>],
    source_cells: &[usize],
    source_group_weights: &[f64],
    survival_ratio: f64,
    cap: f64,
    groups: usize,
) -> Result<(Vec<f64>, Vec<f64>, f64, usize), CadisError> {
    let w_ref = {
        let mut num = 0.0;
        let mut den = 0.0;
        for &cell in source_cells {
            for (g, &w) in source_group_weights.iter().enumerate() {
                if w > 0.0 && adjoint_flux[cell][g].is_finite() {
                    num += w * adjoint_flux[cell][g];
                    den += w;
                }
            }
        }
        if den <= 0.0 || num <= 0.0 {
            return Err(CadisError::Window {
                index: 0,
                reason: "adjoint flux is zero at the source cells — the \
                         declared response is unreachable from the source"
                    .into(),
            });
        }
        num / den
    };
    let n_cells = adjoint_flux.len();
    let mut capped = 0usize;
    let mut lower = vec![0.0; n_cells * groups];
    let mut upper = vec![0.0; n_cells * groups];
    // The window contract's energy bins run in ascending-energy order;
    // the multigroup convention is descending — group `g` maps to
    // window energy bin `groups - 1 - g`.
    for (cell, row) in adjoint_flux.iter().enumerate() {
        for (g, &phi_adj) in row.iter().enumerate() {
            let e = groups - 1 - g;
            let w0 = if phi_adj.is_finite() && phi_adj > 0.0 {
                (w_ref / phi_adj).min(cap)
            } else {
                cap
            };
            if w0 >= cap {
                capped += 1;
            }
            lower[e * n_cells + cell] = w0 / survival_ratio;
            upper[e * n_cells + cell] = w0;
        }
    }
    Ok((lower, upper, w_ref, capped))
}

/// The adjoint window must sit on the solver's own grid: same mesh
/// bins, same energy structure, neutron particle, axis-aligned grid.
fn check_adjoint_window(
    index: usize,
    window: &crate::variance_reduction::WeightWindowSpec,
    case: &TransportCase,
    data: &MultigroupData,
) -> Result<(), CadisError> {
    let invalid = |reason: &str| CadisError::Window {
        index,
        reason: reason.to_string(),
    };
    if window.particle != ParticleType::Neutron {
        return Err(invalid(
            "the in-house adjoint solver transports neutrons only",
        ));
    }
    let geometry = &case.geometry;
    if window.mesh.dimensions != geometry.shape {
        return Err(invalid(
            "adjoint windows are derived on the case grid — \
             mesh.dimensions must equal the grid shape",
        ));
    }
    // The solver works in the grid's index frame; a rotated direction
    // would make the axis-aligned window mesh mean something else.
    let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    if geometry
        .direction
        .iter()
        .zip(identity.iter())
        .any(|(a, b)| (a - b).abs() > 1e-9)
    {
        return Err(invalid(
            "adjoint windows require an axis-aligned (identity-direction) grid",
        ));
    }
    for axis in 0..3 {
        let lo = (geometry.origin_mm[axis] - 0.5 * geometry.spacing_mm[axis]) / 10.0;
        let hi = lo + geometry.spacing_mm[axis] * geometry.shape[axis] as f64 / 10.0;
        let tol = 1e-6 * hi.abs().max(1.0);
        if (window.mesh.lower_left_cm[axis] - lo).abs() > tol
            || (window.mesh.upper_right_cm[axis] - hi).abs() > tol
        {
            return Err(invalid(
                "window mesh extents must equal the case grid extents",
            ));
        }
    }
    match &window.energy_bounds_ev {
        Some(bounds) => {
            // Window bounds are ascending; multigroup boundaries are
            // descending — the same structure read in reverse.
            let matches = bounds.len() == data.energy_boundaries_ev.len()
                && bounds
                    .iter()
                    .zip(data.energy_boundaries_ev.iter().rev())
                    .all(|(a, b)| (*a - *b).abs() <= 1e-9 * b.abs());
            if !matches {
                return Err(invalid(
                    "window energy_bounds_ev must be the multigroup \
                     structure in ascending order (no re-binning across \
                     the adjoint solve)",
                ));
            }
        }
        None => {
            if data.group_count() != 1 {
                return Err(invalid(
                    "window declares a single energy group but the \
                     multigroup data has several — declare \
                     energy_bounds_ev matching the data",
                ));
            }
        }
    }
    Ok(())
}

fn check_forward_flux(
    flux: &MultigroupFlux,
    data: &MultigroupData,
    n_cells: usize,
    groups: usize,
    index: usize,
) -> Result<(), CadisError> {
    let invalid = |reason: String| CadisError::Window { index, reason };
    if flux.energy_boundaries_ev != data.energy_boundaries_ev {
        return Err(invalid(
            "forward flux energy structure does not match the multigroup data".into(),
        ));
    }
    if flux.flux.len() != n_cells || flux.flux.iter().any(|row| row.len() != groups) {
        return Err(invalid(
            "forward flux shape does not match the case grid".into(),
        ));
    }
    Ok(())
}

/// Derivation provenance summary for CLI display.
#[derive(Debug, Clone, Serialize)]
pub struct CadisSummary {
    pub windows: usize,
    pub adjoint_solves: usize,
    pub all_converged: bool,
}

pub fn summarize(derivation: &AdjointDerivation) -> CadisSummary {
    CadisSummary {
        windows: derivation.resolved.windows.len(),
        adjoint_solves: derivation.adjoint_fluxes.len(),
        all_converged: derivation.adjoint_fluxes.iter().all(|f| f.converged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multigroup::tests::{data as slab_data, options, slab_case};
    use crate::multigroup::{BoundarySource, level_symmetric_quadrature, solve_sn_problem};
    use crate::variance_reduction::{
        AdjointMethod, AdjointResponse, WeightWindowMesh, WeightWindowParameters, WeightWindowSpec,
    };

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    fn adjoint_spec(bounds: WeightWindowBounds) -> VarianceReductionSpec {
        VarianceReductionSpec {
            schema_version: crate::VARIANCE_REDUCTION_SCHEMA.into(),
            id: "openbnct.test.vr.adjoint".into(),
            description: "test".into(),
            case_id: Some("mg-slab".into()),
            windows: vec![WeightWindowSpec {
                particle: ParticleType::Neutron,
                // The slab grid spans [-0.2,0.2] x [-0.2,0.2] x [-1,1] cm.
                mesh: WeightWindowMesh {
                    dimensions: [4, 4, 20],
                    lower_left_cm: [-0.2, -0.2, -1.0],
                    upper_right_cm: [0.2, 0.2, 1.0],
                },
                energy_bounds_ev: Some(vec![0.001, 1.0]),
                parameters: WeightWindowParameters::default(),
                bounds,
                bound_boosts: vec![],
            }],
            provenance_note: "test".into(),
            qualification: "test_only".into(),
        }
    }

    /// Discrete adjoint consistency: ⟨q†, φ_fwd⟩ ≈ ⟨q_fwd, φ†⟩ up to
    /// diamond-difference truncation error. A wrong scatter transpose
    /// breaks this by O(scatter fraction) — far above the tolerance.
    #[test]
    fn adjoint_reciprocity_holds() {
        let case = slab_case();
        // Two groups with downscatter 0.3 and mild upscatter 0.02.
        let mg = slab_data(&[1.0, 0.6], vec![0.2, 0.3, 0.02, 0.1]);
        mg.validate().unwrap();
        let opts = options();
        let n_cells = 4 * 4 * 20;
        let groups = 2;
        // Volumetric forward source: one interior voxel, group 0.
        let mut q_fwd = vec![vec![0.0; groups]; n_cells];
        // voxel (2,1,5) → 2 + 4·1 + 16·5.
        q_fwd[86][0] = 1.0;
        // Response: one voxel near the exit face, group 1.
        let mut q_adj = vec![vec![0.0; groups]; n_cells];
        // voxel (1,2,18) → 1 + 4·2 + 16·18.
        q_adj[297][1] = 1.0;
        let quadrature = level_symmetric_quadrature(opts.quadrature_order).unwrap();
        let (_, case_material) = material_composition_map(&case, &mg, None).unwrap();
        let forward = solve_sn_problem(
            &case,
            &mg,
            &opts,
            &case_material,
            &quadrature,
            &BoundarySource::new(),
            &q_fwd,
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let adjoint =
            solve_multigroup_adjoint(&case, &mg, &opts, &q_adj, cref("d"), cref("c")).unwrap();
        let r_fwd: f64 = q_adj
            .iter()
            .zip(forward.flux.iter())
            .flat_map(|(qa, fa)| qa.iter().zip(fa.iter()).map(|(a, b)| a * b))
            .sum();
        let r_adj: f64 = q_fwd
            .iter()
            .zip(adjoint.flux.iter())
            .flat_map(|(qa, fa)| qa.iter().zip(fa.iter()).map(|(a, b)| a * b))
            .sum();
        assert!(r_fwd > 0.0 && r_adj > 0.0, "responses must be positive");
        let rel = (r_fwd - r_adj).abs() / r_fwd;
        assert!(
            rel < 0.05,
            "adjoint reciprocity violated: ⟨q†,φ⟩={r_fwd} vs ⟨q,φ†⟩={r_adj} ({:.1}%)",
            rel * 100.0
        );
    }

    /// Importance toward an exit-face response grows monotonically
    /// toward that face in a pure absorber.
    #[test]
    fn importance_grows_toward_response_region() {
        let case = slab_case();
        let mg = slab_data(&[0.5], vec![0.0]);
        let opts = options();
        let n_cells = 4 * 4 * 20;
        // Response: the last z layer.
        let mut q_adj = vec![vec![0.0; 1]; n_cells];
        for i in 0..4 {
            for j in 0..4 {
                q_adj[i + 4 * j + 16 * 19][0] = 1.0;
            }
        }
        let adjoint =
            solve_multigroup_adjoint(&case, &mg, &opts, &q_adj, cref("d"), cref("c")).unwrap();
        assert!(adjoint.converged);
        // Profile along z at a fixed transverse cell — periodic x/y make
        // it uniform, so cell (0,0,k) suffices.
        let phi = |k: usize| adjoint.flux[16 * k][0];
        assert!(phi(0) < phi(10) && phi(10) < phi(19));
    }

    /// A misspelled response component must fail loudly, not emit a
    /// zero-importance window set.
    #[test]
    fn missing_response_component_rejected() {
        let case = slab_case();
        let mg = slab_data(&[0.5], vec![0.0]);
        let spec = adjoint_spec(WeightWindowBounds::Adjoint {
            method: AdjointMethod::Cadis,
            response: AdjointResponse::DoseComponent {
                component: "no_such_component".into(),
            },
            target_cap: None,
        });
        let err = resolve_adjoint_windows(
            &spec,
            &"b".repeat(32),
            "ww",
            &case,
            &mg,
            &options(),
            None,
            cref("d"),
            cref("c"),
        );
        assert!(matches!(err, Err(CadisError::Window { .. })));
    }

    /// End-to-end CADIS: exit-face response → windows tighten toward the
    /// exit and the artifact validates.
    #[test]
    fn cadis_windows_tighten_toward_response() {
        let case = slab_case();
        let mut mg = slab_data(&[0.5], vec![0.0]);
        mg.materials[0]
            .dose_response_gy_cm2
            .insert("boron".into(), vec![1e-4]);
        let spec = adjoint_spec(WeightWindowBounds::Adjoint {
            method: AdjointMethod::Cadis,
            response: AdjointResponse::VoxelBox {
                lower: [0, 0, 18],
                upper: [3, 3, 19],
            },
            target_cap: None,
        });
        let derivation = resolve_adjoint_windows(
            &spec,
            &"b".repeat(32),
            "openbnct.test.ww.cadis",
            &case,
            &mg,
            &options(),
            None,
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let window = &derivation.resolved.windows[0];
        assert_eq!(window.lower_bounds.len(), 4 * 4 * 20);
        let s = window.parameters.survival_ratio;
        // upper = lower × survival_ratio everywhere.
        for (l, u) in window.lower_bounds.iter().zip(window.upper_bounds.iter()) {
            assert!(*l > 0.0 && (*u - *l * s).abs() < 1e-9 * u.abs());
        }
        // Targets shrink toward the response (importance grows).
        let target = |k: usize| window.upper_bounds[16 * k];
        assert!(target(0) > target(10) && target(10) > target(19));
        // Unit-weight birth at the source face: target ≈ 1.
        assert!((target(0) - 1.0).abs() < 0.2, "source target {}", target(0));
        assert_eq!(derivation.resolved.derivation.method, "cadis");
        assert_eq!(derivation.adjoint_fluxes.len(), 1);
        derivation.resolved.validate().unwrap();
    }

    /// FW-CADIS needs the forward flux artifact and flattens the
    /// population relative to plain CADIS.
    #[test]
    fn fw_cadis_requires_and_uses_forward_flux() {
        let case = slab_case();
        let mg = slab_data(&[0.5], vec![0.0]);
        let opts = options();
        let spec = adjoint_spec(WeightWindowBounds::Adjoint {
            method: AdjointMethod::FwCadis,
            response: AdjointResponse::Global,
            target_cap: None,
        });
        // No forward flux → refused.
        let err = resolve_adjoint_windows(
            &spec,
            &"b".repeat(32),
            "ww",
            &case,
            &mg,
            &opts,
            None,
            cref("d"),
            cref("c"),
        );
        assert!(matches!(err, Err(CadisError::MissingForwardFlux)));
        // With the forward solve → windows.
        let flux = crate::solve_multigroup(&case, &mg, &opts, cref("d"), cref("c")).unwrap();
        let derivation = resolve_adjoint_windows(
            &spec,
            &"b".repeat(32),
            "openbnct.test.ww.fwcadis",
            &case,
            &mg,
            &opts,
            Some((&flux, cref("fwd"))),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let window = &derivation.resolved.windows[0];
        assert!(window.lower_bounds.iter().all(|l| *l > 0.0));
        assert_eq!(derivation.resolved.derivation.method, "fw_cadis");
        assert_eq!(
            derivation.resolved.derivation.forward_flux,
            Some(cref("fwd"))
        );
        derivation.resolved.validate().unwrap();
    }

    /// Window spec/grid mismatches are caught before any solve runs.
    #[test]
    fn adjoint_window_grid_contract_enforced() {
        let case = slab_case();
        let mg = slab_data(&[0.5], vec![0.0]);
        let mut spec = adjoint_spec(WeightWindowBounds::Adjoint {
            method: AdjointMethod::Cadis,
            response: AdjointResponse::Global,
            target_cap: None,
        });
        // Photon windows are refused — the solver is neutron-only.
        spec.windows[0].particle = ParticleType::Photon;
        let err = resolve_adjoint_windows(
            &spec,
            &"b".repeat(32),
            "ww",
            &case,
            &mg,
            &options(),
            None,
            cref("d"),
            cref("c"),
        );
        assert!(matches!(err, Err(CadisError::Window { .. })));
        // Mesh dims ≠ grid shape → refused.
        spec.windows[0].particle = ParticleType::Neutron;
        spec.windows[0].mesh.dimensions = [2, 2, 20];
        let err = resolve_adjoint_windows(
            &spec,
            &"b".repeat(32),
            "ww",
            &case,
            &mg,
            &options(),
            None,
            cref("d"),
            cref("c"),
        );
        assert!(matches!(err, Err(CadisError::Window { .. })));
    }
}
