// SPDX-License-Identifier: MIT

//! Certified exact planner: the same dose-linear problem
//! [`optimize`](crate::optimize) descends, expressed as a convex
//! quadratic program and solved to an optimality certificate by
//! Clarabel's interior point (`clarabel` crate — pure Rust,
//! wasm-compatible, duals included).
//!
//! Formulation. `D(v) = Σ_i w_i·D_i(v)` is linear in the weights, so
//! every metric built on voxelwise dose is linear or piecewise-linear
//! in `w`. In [`LpMode::Penalty`] the variables are the weights plus a
//! non-negative violation slack `v_j` per objective; the cost is the
//! identical bound-normalized quadratic penalty
//! `Σ_j weight_j·(v_j/bound_j)² + λ·Σ_i w_i` the projected-gradient
//! optimizer minimizes — a diagonal-`P` QP — so the result is the
//! certified global optimum of the same landscape, not a different
//! objective. In [`LpMode::Strict`] the slacks are dropped and the
//! objective bounds are enforced as hard constraints under a
//! `λ·Σ_i w_i` cost; primal infeasibility is then a definitive answer,
//! which a descent method can never give.
//!
//! Dose-at-volume bounds are enforced through the CVaR tail-mean
//! surrogate of Rockafellar–Uryasev (the standard fluence-optimization
//! approximation): `D_f ≥ T` becomes `CVaR_{1−f}^{lower}(d) ≥ T` over
//! the mask, represented with one free auxiliary `t` and per-voxel
//! non-negative slacks `s_v` (`s_v ≥ t − d_v`,
//! `t − (1/(αN))Σ s_v ≥ T`). This is conservative — the tail mean
//! dominating `T` implies the quantile bound, not conversely — and the
//! certificate records which objectives used it. `f = 1` degenerates
//! to exact voxelwise minimum-dose rows (no surrogate). `MinEud` is
//! admitted only at `eud_a = 1` (a masked mean); nonlinear EUD stays
//! with the projected-gradient solver.
//!
//! Scenario sets stay convex: each bound row is replicated per
//! perturbed field set while the violation slack `v_j` is shared, so
//! `v_j` prices the objective's *worst-case* violation — stricter than
//! the composite worst-case penalty the gradient optimizer minimizes.
//!
//! Dual multipliers on the bound rows are exported on the result
//! certificate as `bound_multipliers` — the marginal cost, in penalty
//! units per bound unit, of tightening each objective. Binding rows
//! carry the nonzero prices; slack rows price at zero.

use std::borrow::Cow;
use std::collections::BTreeMap;

use clarabel::algebra::CscMatrix;
use clarabel::solver::{DefaultSettings, DefaultSolver, IPSolver, NonnegativeConeT, SolverStatus};
use openbnct_core::{GridGeometry, RegionMask};

use crate::optimize::{
    BeamDoseField, BeamWeight, DoseObjective, DoseQuantity, InversePlanObjective,
    InversePlanResult, OptimizeError, ResultProvenance, final_outcomes, objective_mask,
    penalty_and_gradient, resolve_inputs,
};
use crate::scenarios::{PlanScenarioSet, perturb_fields, values_component_of};

/// How the QP solver treats the declared objectives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LpMode {
    /// Violation slacks carry the bound-normalized quadratic penalty —
    /// the certified optimum of the same landscape `optimize_weights`
    /// descends. Always feasible.
    Penalty,
    /// Objective bounds are hard constraints; the cost is
    /// `weight_regularization·Σ_i w_i`. Primal infeasibility is a
    /// definitive certificate that no weights satisfy the bounds.
    Strict,
}

/// Optimality certificate exported on [`InversePlanResult`] for the
/// exact-solver methods.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanCertificate {
    /// Solver name/version tag (`clarabel/x.y.z`).
    pub solver: String,
    /// Interior-point termination status (`optimal`,
    /// `almost_optimal`, `infeasible`, …).
    pub status: String,
    /// Primal objective value of the assembled QP (the penalized
    /// landscape value in `Penalty` mode).
    pub primal_objective: f64,
    /// Dual objective bound — `primal_objective − dual_objective` is
    /// the optimality gap certificate.
    pub dual_objective: f64,
    /// Interior-point iteration count.
    pub iterations: u32,
    /// Lagrange multiplier of each objective's bound row(s), in spec
    /// order — the marginal penalty cost of tightening the bound one
    /// unit (multi-row and multi-scenario blocks report the max).
    /// Zero on non-binding objectives.
    pub bound_multipliers: Vec<f64>,
    /// Indices (into `spec.objectives`) whose dose-at-volume bounds
    /// were enforced through the conservative CVaR tail-mean
    /// surrogate rather than the literal quantile.
    pub cvar_surrogate_objectives: Vec<u32>,
}

/// Per-beam dose field under one objective's quantity fold — borrowed
/// for physical/component quantities, an owned `Σ_c w_c·d_c` fold per
/// objective mask under `isoeffective`.
fn effective_fields<'a>(
    spec: &InversePlanObjective,
    objective: &DoseObjective,
    fields: &'a [BeamDoseField],
) -> Vec<Cow<'a, [f64]>> {
    if spec.dose_quantity != DoseQuantity::Isoeffective {
        return fields
            .iter()
            .map(|f| Cow::Borrowed(&f.values[..]))
            .collect();
    }
    let model = spec.bio_model.as_ref().expect("validated");
    let weights_map = model
        .region_weights
        .get(objective_mask(objective))
        .unwrap_or(&model.component_weights);
    let n = fields
        .first()
        .and_then(|f| f.components.as_ref())
        .and_then(|c| c.values().next())
        .map_or(0, Vec::len);
    fields
        .iter()
        .map(|f| {
            let comps = f.components.as_ref().expect("validated");
            let mut eff = vec![0.0; n];
            for (name, &wc) in weights_map {
                for (v, &cd) in comps[name].iter().enumerate() {
                    eff[v] += wc * cd;
                }
            }
            Cow::Owned(eff)
        })
        .collect()
}

/// Sparse LP builder over a triplet list; rows are all `aᵀx ≤ b`
/// (Clarabel's `NonnegativeCone` slack form).
struct Assembly {
    /// `(col, row, val)` triplets; sorted by column at finalize.
    triplets: Vec<(usize, usize, f64)>,
    b: Vec<f64>,
    n_vars: usize,
}

impl Assembly {
    fn row(&mut self) -> usize {
        self.b.push(0.0);
        self.b.len() - 1
    }
    fn at(&mut self, row: usize, col: usize, val: f64) {
        if val != 0.0 {
            self.triplets.push((col, row, val));
        }
    }
    fn finish(self) -> (CscMatrix<f64>, Vec<f64>) {
        let n_rows = self.b.len();
        let mut triplets = self.triplets;
        triplets.sort_by_key(|t| (t.0, t.1));
        let mut colptr = vec![0_usize; self.n_vars + 1];
        let mut rowval = Vec::with_capacity(triplets.len());
        let mut nzval = Vec::with_capacity(triplets.len());
        let mut cursor = 0_usize;
        for (col, row, val) in triplets {
            while cursor < col {
                cursor += 1;
                colptr[cursor] = rowval.len();
            }
            // Merge duplicate (col,row) entries — rows accumulate
            // coefficients from several emission sites.
            if rowval.last() == Some(&row) && colptr[cursor] < rowval.len() {
                *nzval.last_mut().unwrap() += val;
            } else {
                rowval.push(row);
                nzval.push(val);
            }
        }
        while cursor < self.n_vars {
            cursor += 1;
            colptr[cursor] = rowval.len();
        }
        (
            CscMatrix::new(n_rows, self.n_vars, colptr, rowval, nzval),
            self.b,
        )
    }
}

/// Emit one objective's constraint rows for one scenario field set and
/// return the row index whose dual is the bound's shadow price
/// (`None` when the objective contributes only aux rows — currently
/// unreachable since every objective has a bound row). `col_v` is the
/// violation slack's column or `usize::MAX` in strict mode.
fn emit_objective_rows(
    asm: &mut Assembly,
    spec: &InversePlanObjective,
    objective: &DoseObjective,
    mask_voxels: &[usize],
    fields: &[BeamDoseField],
    col_v: usize,
    strict: bool,
) -> Result<Vec<usize>, OptimizeError> {
    let n_mask = mask_voxels.len() as f64;
    let eff = effective_fields(spec, objective, fields);
    let v_coeff = if strict { 0.0 } else { 1.0 };
    let mut bound_rows = Vec::new();
    match objective {
        DoseObjective::MinEud { target, eud_a, .. } => {
            if *eud_a != 1.0 {
                return Err(OptimizeError::InvalidObjective(
                    "lp/qp solver: min_eud requires eud_a = 1.0 (a masked mean); \
                     nonlinear eud stays on the projected-gradient solver"
                        .into(),
                ));
            }
            // mean(mask)·w ≥ target − v → −Σ m_i w_i − v ≤ −target
            let row = asm.row();
            asm.b[row] = -target;
            for (i, e) in eff.iter().enumerate() {
                let m: f64 = mask_voxels.iter().map(|&v| e[v]).sum::<f64>() / n_mask;
                asm.at(row, i, -m);
            }
            asm.at(row, col_v, -v_coeff);
            bound_rows.push(row);
        }
        DoseObjective::MaxMean { limit, .. } => {
            // mean(mask)·w − v ≤ limit
            let row = asm.row();
            asm.b[row] = *limit;
            for (i, e) in eff.iter().enumerate() {
                let m: f64 = mask_voxels.iter().map(|&v| e[v]).sum::<f64>() / n_mask;
                asm.at(row, i, m);
            }
            asm.at(row, col_v, -v_coeff);
            bound_rows.push(row);
        }
        DoseObjective::MinDoseAtVolume {
            volume_fraction,
            target,
            ..
        } if *volume_fraction == 1.0 => {
            // f = 1 is the literal voxelwise minimum — exact rows,
            // no surrogate: −d_v(w) − v ≤ −target for every voxel.
            for &vox in mask_voxels {
                let row = asm.row();
                asm.b[row] = -target;
                for (i, e) in eff.iter().enumerate() {
                    asm.at(row, i, -e[vox]);
                }
                asm.at(row, col_v, -v_coeff);
                bound_rows.push(row);
            }
        }
        DoseObjective::MinDoseAtVolume {
            volume_fraction,
            target,
            ..
        } => {
            // Lower-tail CVaR at α = 1−f: t − (1/(αN))Σ s_v ≥ target − v
            // with s_v ≥ t − d_v(w), s_v ≥ 0, t free.
            let alpha = 1.0 - volume_fraction;
            let col_t = asm.n_vars;
            let col_s0 = asm.n_vars + 1;
            asm.n_vars += 1 + mask_voxels.len();
            for (k, &vox) in mask_voxels.iter().enumerate() {
                // −d_v + t − s_v ≤ 0
                let row = asm.row();
                for (i, e) in eff.iter().enumerate() {
                    asm.at(row, i, -e[vox]);
                }
                asm.at(row, col_t, 1.0);
                asm.at(row, col_s0 + k, -1.0);
                // s_v ≥ 0
                let row = asm.row();
                asm.at(row, col_s0 + k, -1.0);
            }
            // −t + (1/(αN))Σ s_v − v ≤ −target
            let row = asm.row();
            asm.b[row] = -target;
            asm.at(row, col_t, -1.0);
            let c = 1.0 / (alpha * n_mask);
            for k in 0..mask_voxels.len() {
                asm.at(row, col_s0 + k, c);
            }
            asm.at(row, col_v, -v_coeff);
            bound_rows.push(row);
        }
        DoseObjective::MaxDoseAtVolume {
            volume_fraction,
            limit,
            ..
        } => {
            // Upper-tail CVaR at α = f: t + (1/(αN))Σ s_v ≤ limit + v
            // with s_v ≥ d_v(w) − t, s_v ≥ 0.
            let col_t = asm.n_vars;
            let col_s0 = asm.n_vars + 1;
            asm.n_vars += 1 + mask_voxels.len();
            for (k, &vox) in mask_voxels.iter().enumerate() {
                // d_v − t − s_v ≤ 0
                let row = asm.row();
                for (i, e) in eff.iter().enumerate() {
                    asm.at(row, i, e[vox]);
                }
                asm.at(row, col_t, -1.0);
                asm.at(row, col_s0 + k, -1.0);
                let row = asm.row();
                asm.at(row, col_s0 + k, -1.0);
            }
            // t + (1/(αN))Σ s_v − v ≤ limit
            let row = asm.row();
            asm.b[row] = *limit;
            asm.at(row, col_t, 1.0);
            let c = 1.0 / (volume_fraction * n_mask);
            for k in 0..mask_voxels.len() {
                asm.at(row, col_s0 + k, c);
            }
            asm.at(row, col_v, -v_coeff);
            bound_rows.push(row);
        }
    }
    Ok(bound_rows)
}

/// Assemble and solve the QP for a list of scenario field sets
/// (`field_sets[0]` is the nominal). Returns
/// `(weights, certificate)`.
fn solve_qp(
    spec: &InversePlanObjective,
    mask_voxels: &[Vec<usize>],
    field_sets: &[Vec<BeamDoseField>],
    mode: LpMode,
) -> Result<(Vec<f64>, PlanCertificate), OptimizeError> {
    let n_beams = field_sets[0].len();
    let strict = mode == LpMode::Strict;
    let n_obj = spec.objectives.len();
    let mut asm = Assembly {
        triplets: Vec::new(),
        b: Vec::new(),
        // Columns: B weights, J violation slacks (penalty mode only),
        // then CVaR aux blocks allocated per (objective, scenario).
        n_vars: n_beams + if strict { 0 } else { n_obj },
    };
    let col_v = |j: usize| n_beams + j;

    // Bound-row index per objective across scenarios — the multiplier
    // reported is the max over the block.
    let mut bound_rows: Vec<Vec<usize>> = vec![Vec::new(); n_obj];
    let mut cvar_surrogate: Vec<u32> = Vec::new();
    for (j, objective) in spec.objectives.iter().enumerate() {
        let uses_cvar = matches!(
            objective,
            DoseObjective::MinDoseAtVolume {
                volume_fraction,
                ..
            } if *volume_fraction < 1.0
        ) || matches!(objective, DoseObjective::MaxDoseAtVolume { .. });
        if uses_cvar {
            cvar_surrogate.push(j as u32);
        }
        for fields in field_sets.iter() {
            let rows = emit_objective_rows(
                &mut asm,
                spec,
                objective,
                &mask_voxels[j],
                fields,
                if strict { usize::MAX } else { col_v(j) },
                strict,
            )?;
            bound_rows[j].extend(rows);
        }
    }

    // w_i ≥ 0 and optional weight_bound; v_j ≥ 0 (penalty mode).
    for i in 0..n_beams {
        let row = asm.row();
        asm.at(row, i, -1.0);
        if let Some(ub) = spec.weight_bound {
            let row = asm.row();
            asm.b[row] = ub;
            asm.at(row, i, 1.0);
        }
    }
    if !strict {
        for j in 0..n_obj {
            let row = asm.row();
            asm.at(row, col_v(j), -1.0);
        }
    }

    // Cost: ½xᵀPx + qᵀx — diagonal P on the violation slacks
    // (2·weight·scale²), λ on the weights.
    let mut p_triplets: Vec<(usize, usize, f64)> = Vec::new();
    let mut q = vec![0.0; asm.n_vars];
    for qi in q.iter_mut().take(n_beams) {
        *qi = spec.weight_regularization;
    }
    if !strict {
        for (j, objective) in spec.objectives.iter().enumerate() {
            let (weight, bound) = match objective {
                DoseObjective::MinEud { weight, target, .. }
                | DoseObjective::MinDoseAtVolume { weight, target, .. } => (*weight, *target),
                DoseObjective::MaxMean { weight, limit, .. }
                | DoseObjective::MaxDoseAtVolume { weight, limit, .. } => (*weight, *limit),
            };
            let scale = if bound > 0.0 { 1.0 / bound } else { 1.0 };
            p_triplets.push((col_v(j), col_v(j), 2.0 * weight * scale * scale));
        }
    }
    let n_vars = asm.n_vars;
    let p = {
        let mut trip = p_triplets;
        trip.sort_by_key(|t| (t.0, t.1));
        let mut colptr = vec![0_usize; n_vars + 1];
        let mut rowval = Vec::with_capacity(trip.len());
        let mut nzval = Vec::with_capacity(trip.len());
        let mut cursor = 0_usize;
        for (col, row, val) in trip {
            while cursor < col {
                cursor += 1;
                colptr[cursor] = rowval.len();
            }
            rowval.push(row);
            nzval.push(val);
        }
        while cursor < n_vars {
            cursor += 1;
            colptr[cursor] = rowval.len();
        }
        CscMatrix::new(n_vars, n_vars, colptr, rowval, nzval)
    };

    let (a, b) = asm.finish();
    let cones = [NonnegativeConeT(b.len())];
    // Tighten past defaults — plan dimensions are tiny, so the extra
    // iterations are free and interior-point solutions sit visibly off
    // the boundary at looser tolerances.
    let settings = DefaultSettings::<f64> {
        verbose: false,
        tol_gap_abs: 1e-10,
        tol_gap_rel: 1e-10,
        tol_feas: 1e-10,
        tol_ktratio: 1e-10,
        tol_infeas_abs: 1e-10,
        tol_infeas_rel: 1e-10,
        ..Default::default()
    };
    let mut solver = DefaultSolver::new(&p, &q, &a, &b, &cones, settings)
        .map_err(|e| OptimizeError::InvalidObjective(format!("lp solver setup: {e}")))?;
    solver.solve();
    let sol = &solver.solution;
    match sol.status {
        SolverStatus::PrimalInfeasible => {
            return Err(OptimizeError::InvalidObjective(
                "lp solver: primal-infeasible — no weight vector satisfies the \
                 declared bounds (the infeasibility is definitive, not a stall)"
                    .into(),
            ));
        }
        SolverStatus::Solved | SolverStatus::AlmostSolved => {}
        other => {
            return Err(OptimizeError::InvalidObjective(format!(
                "lp solver: termination status {other:?}"
            )));
        }
    }
    let w = sol.x[..n_beams].to_vec();
    let bound_multipliers = bound_rows
        .iter()
        .map(|rows| rows.iter().map(|&r| sol.z[r].abs()).fold(0.0, f64::max))
        .collect();
    Ok((
        w,
        PlanCertificate {
            solver: "clarabel".into(),
            status: format!("{:?}", sol.status).to_lowercase(),
            primal_objective: sol.obj_val,
            dual_objective: sol.obj_val_dual,
            iterations: sol.iterations,
            bound_multipliers,
            cvar_surrogate_objectives: cvar_surrogate,
        },
    ))
}

fn lp_result(
    fields: &[BeamDoseField],
    w: &[f64],
    spec: &InversePlanObjective,
    mask_voxels: &[Vec<usize>],
    provenance: ResultProvenance,
    method: &str,
    certificate: PlanCertificate,
) -> InversePlanResult {
    let n_voxels = fields.first().map(|f| f.values.len()).unwrap_or(0);
    let mut dose = vec![0.0; n_voxels];
    // The recorded penalty and outcomes are the *literal* metrics at
    // the optimized weights — not the CVaR surrogates the solver
    // enforced — so the result stays comparable with pgd runs.
    let (penalty, _) = penalty_and_gradient(fields, w, spec, mask_voxels, &mut dose);
    InversePlanResult {
        schema_version: crate::optimize::INVERSE_PLAN_RESULT_SCHEMA.into(),
        id: provenance.id,
        case_id: spec.case_id.clone(),
        objective: provenance.objective,
        dose_quantity: spec.dose_quantity,
        weights: fields
            .iter()
            .zip(w)
            .map(|(f, &wi)| BeamWeight {
                name: f.name.clone(),
                weight: wi,
            })
            .collect(),
        outcomes: final_outcomes(fields, w, spec, mask_voxels),
        penalty,
        iterations: certificate.iterations,
        converged: matches!(certificate.status.as_str(), "solved" | "almostsolved"),
        method: Some(method.into()),
        certificate: Some(certificate),
        qualification: crate::optimize::INVERSE_PLAN_QUALIFICATION.into(),
        provenance_id: provenance.provenance_id,
    }
}

/// Solve the nominal objective document with the certified QP solver.
/// `fields[i]` is beam `i`'s unit-weight dose field; `masks` supplies
/// every named mask.
pub fn optimize_weights_lp(
    fields: &[BeamDoseField],
    masks: &[RegionMask],
    spec: &InversePlanObjective,
    mode: LpMode,
    provenance: ResultProvenance,
) -> Result<InversePlanResult, OptimizeError> {
    let initial = vec![0.0; fields.len()];
    let (_n_voxels, mask_voxels) = resolve_inputs(fields, masks, spec, &initial)?;
    let (w, certificate) = solve_qp(spec, &mask_voxels, &[fields.to_vec()], mode)?;
    Ok(lp_result(
        fields,
        &w,
        spec,
        &mask_voxels,
        provenance,
        match mode {
            LpMode::Penalty => "qp",
            LpMode::Strict => "lp_strict",
        },
        certificate,
    ))
}

/// Solve against a scenario set's per-objective worst case — each
/// bound row replicated per perturbed field set, the violation slack
/// shared so `v_j` prices the worst-scenario violation.
pub fn optimize_weights_scenarios_lp(
    fields: &[BeamDoseField],
    masks: &[RegionMask],
    spec: &InversePlanObjective,
    scenario_set: &PlanScenarioSet,
    geometry: &GridGeometry,
    mode: LpMode,
    provenance: ResultProvenance,
) -> Result<InversePlanResult, OptimizeError> {
    let initial = vec![0.0; fields.len()];
    let (n_voxels, mask_voxels) = resolve_inputs(fields, masks, spec, &initial)?;
    scenario_set
        .validate()
        .map_err(|e| OptimizeError::InvalidObjective(format!("scenario set: {e}")))?;
    let mask_map: BTreeMap<String, Vec<usize>> = masks
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                m.voxels
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &on)| (on && i < n_voxels).then_some(i))
                    .collect(),
            )
        })
        .collect();
    let mut field_sets: Vec<Vec<BeamDoseField>> = vec![fields.to_vec()];
    for scenario in &scenario_set.scenarios {
        field_sets.push(
            perturb_fields(
                fields,
                scenario,
                geometry,
                &mask_map,
                values_component_of(spec),
            )
            .map_err(|e| OptimizeError::InvalidObjective(format!("scenarios: {e}")))?,
        );
    }
    let (w, certificate) = solve_qp(spec, &mask_voxels, &field_sets, mode)?;
    Ok(lp_result(
        fields,
        &w,
        spec,
        &mask_voxels,
        provenance,
        match mode {
            LpMode::Penalty => "qp_worst_case_scenario",
            LpMode::Strict => "lp_strict_worst_case_scenario",
        },
        certificate,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios::{PLAN_SCENARIO_SET_SCHEMA, PlanScenario, PlanScenarioSet};
    use openbnct_core::ContentReference;

    fn objective(objectives: Vec<DoseObjective>) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: crate::optimize::INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
            id: "obj".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::PhysicalTotal,
            objectives,
            max_iterations: 500,
            gradient_tolerance: 1e-10,
            weight_bound: None,
            weight_regularization: 1e-5,
            bio_model: None,
            validity_domain: "unit test".into(),
            provenance_id: "test".into(),
        }
    }

    fn provenance() -> ResultProvenance {
        ResultProvenance {
            id: "res".into(),
            provenance_id: "test".into(),
            objective: ContentReference {
                id: "objective-doc".into(),
                sha256: "0".repeat(64),
            },
        }
    }

    fn mask(name: &str, voxels: &[bool]) -> RegionMask {
        RegionMask {
            name: name.into(),
            voxels: voxels.to_vec(),
        }
    }

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 1, 1],
            spacing_mm: [1.0; 3],
            origin_mm: [0.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Two decoupled beams: A covers the tumor (voxels 0,1 at 10,5),
    /// B covers only the organ (voxels 2,3 at 10,10).
    fn two_beam() -> (Vec<BeamDoseField>, Vec<RegionMask>) {
        let fields = vec![
            BeamDoseField {
                name: "A".into(),
                values: vec![10.0, 5.0, 0.0, 0.0],
                components: None,
            },
            BeamDoseField {
                name: "B".into(),
                values: vec![0.0, 0.0, 10.0, 10.0],
                components: None,
            },
        ];
        let masks = vec![
            mask("tumor", &[true, true, false, false]),
            mask("oar", &[false, false, true, true]),
            mask("all", &[true, true, true, true]),
        ];
        (fields, masks)
    }

    #[test]
    fn qp_recovers_analytic_optimum_with_certificate() {
        let (fields, masks) = two_beam();
        let spec = objective(vec![
            DoseObjective::MinEud {
                mask: "tumor".into(),
                target: 20.0,
                eud_a: 1.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "oar".into(),
                limit: 5.0,
                weight: 1.0,
            },
        ]);
        let result =
            optimize_weights_lp(&fields, &masks, &spec, LpMode::Penalty, provenance()).unwrap();
        // EUD(a=1) = mean = 7.5·w_A → bound met at w_A = 8/3, up to the
        // regularizer's tiny undershoot (~λT²/2w·dm/dw ≈ 2.7e-4 Gy).
        assert!((result.weights[0].weight - 8.0 / 3.0).abs() < 1e-2);
        assert!(result.weights[1].weight < 1e-3);
        assert!(result.converged);
        assert_eq!(result.method.as_deref(), Some("qp"));
        let cert = result.certificate.expect("qp result carries a certificate");
        assert_eq!(cert.status, "solved");
        // The optimality certificate: primal−dual gap is at solver
        // tolerance — the claim PGD could never make.
        assert!((cert.primal_objective - cert.dual_objective).abs() < 1e-4);
        // The tumor bound carries a nonzero multiplier; the satisfied
        // OAR bound does not.
        assert!(cert.bound_multipliers[0] > 0.0);
        assert!(cert.bound_multipliers[1] < 1e-6);
        assert!(cert.cvar_surrogate_objectives.is_empty());
    }

    #[test]
    fn lp_strict_hits_hard_bounds_exactly() {
        let (fields, masks) = two_beam();
        let mut spec = objective(vec![
            DoseObjective::MinDoseAtVolume {
                mask: "tumor".into(),
                volume_fraction: 1.0,
                target: 10.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "oar".into(),
                limit: 100.0, // slack — never binds
                weight: 1.0,
            },
        ]);
        spec.weight_regularization = 1e-3;
        let result =
            optimize_weights_lp(&fields, &masks, &spec, LpMode::Strict, provenance()).unwrap();
        // f=1 is the voxelwise minimum: 5·w_A ≥ 10 → w_A = 2 exactly;
        // w_B only costs weight.
        assert!((result.weights[0].weight - 2.0).abs() < 1e-5);
        assert!(result.weights[1].weight < 1e-6);
        assert_eq!(result.method.as_deref(), Some("lp_strict"));
        let cert = result.certificate.unwrap();
        assert!(cert.bound_multipliers[0] > 0.0);
        assert!(cert.bound_multipliers[1] < 1e-9);
    }

    #[test]
    fn lp_strict_reports_definitive_infeasibility() {
        let (fields, masks) = two_beam();
        // Tumor floor needs w_A ≥ 2 (5·w_A ≥ 10); the whole-phantom
        // mean cap forces 3.75·w_A ≤ 1 → w_A ≤ 0.267. Impossible.
        let spec = objective(vec![
            DoseObjective::MinDoseAtVolume {
                mask: "tumor".into(),
                volume_fraction: 1.0,
                target: 10.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "all".into(),
                limit: 1.0,
                weight: 1.0,
            },
        ]);
        let error =
            optimize_weights_lp(&fields, &masks, &spec, LpMode::Strict, provenance()).unwrap_err();
        assert!(error.to_string().contains("infeasible"));
    }

    #[test]
    fn cvar_surrogate_is_conservative_and_recorded() {
        // One beam, mask doses [1,2,3,4]·w — D75's literal quantile
        // (sorted idx 1) is 2·w but the CVaR lower-tail mean is the
        // bottom voxel 1·w: the surrogate binds tighter.
        let fields = vec![BeamDoseField {
            name: "A".into(),
            values: vec![1.0, 2.0, 3.0, 4.0],
            components: None,
        }];
        let masks = vec![mask("m", &[true; 4])];
        let mut spec = objective(vec![DoseObjective::MinDoseAtVolume {
            mask: "m".into(),
            volume_fraction: 0.75,
            target: 2.0,
            weight: 1.0,
        }]);
        spec.weight_regularization = 1e-3;
        let result =
            optimize_weights_lp(&fields, &masks, &spec, LpMode::Strict, provenance()).unwrap();
        // Surrogate enforced: w = 2 (tail mean 1·w ≥ 2), and the
        // literal D75 achieved = 2·w = 4 — comfortably satisfied.
        assert!((result.weights[0].weight - 2.0).abs() < 1e-4);
        assert!(result.outcomes[0].satisfied);
        assert!((result.outcomes[0].achieved - 4.0).abs() < 1e-3);
        assert_eq!(
            result.certificate.unwrap().cvar_surrogate_objectives,
            vec![0]
        );
    }

    #[test]
    fn scenario_rows_enforce_per_objective_worst_case() {
        let (fields, masks) = two_beam();
        let mut spec = objective(vec![DoseObjective::MinDoseAtVolume {
            mask: "tumor".into(),
            volume_fraction: 1.0,
            target: 10.0,
            weight: 1.0,
        }]);
        spec.weight_regularization = 1e-3;
        // A −10% output scenario must pull w_A from 2 to 20/9.
        let set = PlanScenarioSet {
            schema_version: PLAN_SCENARIO_SET_SCHEMA.into(),
            id: "set".into(),
            note: None,
            scenarios: vec![PlanScenario {
                name: "output-low".into(),
                note: None,
                dose_scale: Some(0.9),
                component_scales: Default::default(),
                region_scales: Vec::new(),
                shift_mm: None,
            }],
        };
        let result = optimize_weights_scenarios_lp(
            &fields,
            &masks,
            &spec,
            &set,
            &geometry(),
            LpMode::Strict,
            provenance(),
        )
        .unwrap();
        assert!((result.weights[0].weight - 20.0 / 9.0).abs() < 1e-4);
        assert_eq!(
            result.method.as_deref(),
            Some("lp_strict_worst_case_scenario")
        );
        assert!(result.certificate.unwrap().bound_multipliers[0] > 0.0);
    }

    #[test]
    fn nonlinear_eud_is_rejected_not_silenced() {
        let (fields, masks) = two_beam();
        let spec = objective(vec![DoseObjective::MinEud {
            mask: "tumor".into(),
            target: 20.0,
            eud_a: 2.0,
            weight: 1.0,
        }]);
        let error =
            optimize_weights_lp(&fields, &masks, &spec, LpMode::Penalty, provenance()).unwrap_err();
        assert!(error.to_string().contains("eud_a"));
    }
}
