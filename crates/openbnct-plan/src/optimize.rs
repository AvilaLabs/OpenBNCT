// SPDX-License-Identifier: MIT

//! Inverse planning v1: non-negative beam-weight optimization against
//! dose-volume objectives (`openbnct.inverse-plan-objective/0.1.0`,
//! `openbnct.inverse-plan-result/0.1.0`).
//!
//! The accumulated dose of an [`openbnct_core::ExposurePlan`] is linear
//! in the exposure weights — `D(v) = Σ_i w_i·D_i(v)` — so every dose
//! metric the objectives express is a closed-form function of the
//! weight vector with an analytic gradient or subgradient. The
//! optimizer is projected gradient descent with an Armijo backtracking
//! line search: deterministic (no sampling), cheap (one matrix-free
//! pass per iteration), and honest about what it is — a research
//! optimizer, not a commissioned treatment-planning engine.
//!
//! Scope conventions, matching the rest of the workspace:
//!
//! - Objectives address *dose-quantity* fields — `physical_total` or a
//!   named component — declared in the objective document. Bounds are
//!   expressed in the bundle's own dose unit (often
//!   `gray_per_source_particle`); the optimizer does not convert.
//! - Only smooth or piecewise-linear metrics are objectives: mean dose,
//!   generalized EUD, and dose-at-volume quantiles (D95-style, which is
//!   piecewise linear in `w` — the optimizer descends a subgradient).
//! - The result document content-binds the objective, records the final
//!   penalty, per-objective satisfaction, and the research-only
//!   qualification.

use std::collections::BTreeMap;

use openbnct_bio::{BiologicalModel, WeightSemantics};
use openbnct_core::{ContentReference, DoseComponent, RegionMask};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current schema token for inverse-plan objective documents.
pub const INVERSE_PLAN_OBJECTIVE_SCHEMA: &str = "openbnct.inverse-plan-objective/0.1.0";
/// Current schema token for inverse-plan result documents.
pub const INVERSE_PLAN_RESULT_SCHEMA: &str = "openbnct.inverse-plan-result/0.1.0";
/// Qualification asserted on every emitted result.
pub const INVERSE_PLAN_QUALIFICATION: &str = "inverse_planning_research_only_not_clinical";

const DEFAULT_MAX_ITERATIONS: u32 = 500;
const DEFAULT_GRADIENT_TOLERANCE: f64 = 1e-10;
const BACKTRACK_FACTOR: f64 = 0.5;
const MAX_BACKTRACKS: u32 = 40;

/// Which accumulated dose quantity the objectives evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoseQuantity {
    /// The bundle's dedicated physical total.
    PhysicalTotal,
    /// One named dose component (boron, nitrogen, hydrogen, photon).
    Component(DoseComponent),
    /// Biologically weighted dose — the `Σ_c w_c·D_c` sum under the
    /// spec's embedded [`BiologicalModel`], with per-mask
    /// `region_weights` overrides applied to each objective's mask.
    /// Linear in the beam weights, so all objectives keep their analytic
    /// gradients. Requires `bio_model` on the document and
    /// component-resolved beam fields.
    Isoeffective,
}

/// One dose-volume objective. `weight` is the penalty weight in the
/// composite objective — objectives are soft constraints, so the
/// planner's priorities live here, not in a feasibility set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DoseObjective {
    /// `EUD(mask) ≥ target`: generalized equivalent uniform dose
    /// `(Σ_v d_v^a / N)^(1/a)` — the tumor-coverage objective; `a`
    /// conventionally ~1–10 for targets, negative for serial organs.
    MinEud {
        mask: String,
        target: f64,
        /// EUD exponent `a`.
        eud_a: f64,
        weight: f64,
    },
    /// `mean(mask) ≤ limit` — organ-at-risk mean-dose constraint.
    MaxMean {
        mask: String,
        limit: f64,
        weight: f64,
    },
    /// `D_f(mask) ≥ target` — the dose received by the hottest
    /// `volume_fraction` of the mask must reach `target` (coverage
    /// style: `volume_fraction` 0.95 is the classic D95).
    MinDoseAtVolume {
        mask: String,
        volume_fraction: f64,
        target: f64,
        weight: f64,
    },
    /// `D_f(mask) ≤ limit` — dose to the hottest `volume_fraction` is
    /// capped (small fractions bound hot spots).
    MaxDoseAtVolume {
        mask: String,
        volume_fraction: f64,
        limit: f64,
        weight: f64,
    },
}

/// A versioned inverse-planning objective document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InversePlanObjective {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The case the dose fields belong to.
    pub case_id: String,
    /// Which accumulated quantity the objectives evaluate.
    pub dose_quantity: DoseQuantity,
    pub objectives: Vec<DoseObjective>,
    /// Iteration cap for the coordinate-descent solver.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    /// Infinity-norm convergence threshold on the projected gradient —
    /// O(1) scale: violations are normalized by their bounds inside the
    /// penalty, so the same tolerance works at any dose magnitude.
    #[serde(default = "default_gradient_tolerance")]
    pub gradient_tolerance: f64,
    /// Optional upper bound applied to every weight (a delivery limit).
    pub weight_bound: Option<f64>,
    /// Linear pull on `Σ w_i` added to the composite penalty. Feasibility
    /// alone is a plateau — every weight vector meeting all objectives
    /// has zero penalty — so a small positive regularization selects the
    /// minimum-total-weight plan among feasible ones and pins the
    /// iterates to the constraint boundaries rather than overshooting
    /// into slack. `0` (the default) keeps pure feasibility semantics.
    /// The pull competes with bound-normalized violation gradients, so
    /// values ~1e-4–1e-1 are meaningful at any dose scale.
    #[serde(default)]
    pub weight_regularization: f64,
    /// Embedded biological model — required when `dose_quantity` is
    /// `isoeffective`, rejected otherwise. The whole model is embedded so
    /// the objective document's own content hash covers the weight
    /// values the optimizer used. `microdosimetric_kinetic` semantics
    /// and any `fractionation` schedule are refused: per-voxel metrics
    /// need the linear weighted sum.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio_model: Option<BiologicalModel>,
    /// Free-text validity note — what the objectives encode and for
    /// which research scenario. Required and non-empty.
    pub validity_domain: String,
    pub provenance_id: String,
}

fn default_max_iterations() -> u32 {
    DEFAULT_MAX_ITERATIONS
}
fn default_gradient_tolerance() -> f64 {
    DEFAULT_GRADIENT_TOLERANCE
}

/// Errors from objective validation or the optimizer.
#[derive(Debug, Error)]
pub enum OptimizeError {
    #[error("unsupported inverse-plan objective schema {0:?}")]
    UnsupportedObjectiveSchema(String),
    #[error("invalid inverse-plan objective: {0}")]
    InvalidObjective(String),
    #[error("beam dose field {0:?} has no voxels")]
    EmptyField(String),
    #[error("beam fields disagree on voxel count ({0} vs {1})")]
    FieldLengthMismatch(usize, usize),
    #[error("objective references unknown mask {0:?}")]
    UnknownMask(String),
    #[error("mask {0:?} has no selected voxels in the field")]
    EmptyMask(String),
    #[error("initial weights length {0} does not match beam count {1}")]
    InitialLengthMismatch(usize, usize),
}

/// One beam's unit-weight dose field.
#[derive(Debug, Clone)]
pub struct BeamDoseField {
    pub name: String,
    /// Voxel values in grid order `i + nx·j + nx·ny·k` — the selected
    /// `dose_quantity` values for `physical_total` and `component`
    /// quantities (unused under `isoeffective`; may be empty then).
    pub values: Vec<f64>,
    /// Component-resolved voxel values keyed by component name —
    /// required under `dose_quantity: isoeffective`, where the effective
    /// field per objective is `Σ_c w_c(mask)·d_c`. Exactly the four
    /// [`DoseComponent::REQUIRED`] names are expected.
    pub components: Option<BTreeMap<String, Vec<f64>>>,
}

/// Achieved metric and bound for one objective at the optimized weights.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectiveOutcome {
    /// The objective kind token (`min_eud`, `max_mean`, …).
    pub kind: String,
    pub mask: String,
    /// The achieved metric value at the optimized weights.
    pub achieved: f64,
    /// The bound the objective declared (target or limit).
    pub bound: f64,
    /// Whether the achieved value satisfies the bound.
    pub satisfied: bool,
    /// Constraint violation: `max(0, bound−achieved)` for `min`,
    /// `max(0, achieved−bound)` for `max` objectives.
    pub violation: f64,
    pub weight: f64,
}

/// The optimized weight assignment for one beam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeamWeight {
    pub name: String,
    pub weight: f64,
}

/// A versioned inverse-planning result document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InversePlanResult {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// Content binding to the objective document consumed.
    pub objective: ContentReference,
    pub dose_quantity: DoseQuantity,
    pub weights: Vec<BeamWeight>,
    pub outcomes: Vec<ObjectiveOutcome>,
    /// Composite penalty at the optimized weights.
    pub penalty: f64,
    pub iterations: u32,
    pub converged: bool,
    pub qualification: String,
    pub provenance_id: String,
}

/// Metadata the caller stamps on the emitted result.
#[derive(Debug, Clone)]
pub struct ResultProvenance {
    pub id: String,
    pub provenance_id: String,
    pub objective: ContentReference,
}

impl InversePlanObjective {
    /// Structural validation; called by every consumer.
    pub fn validate(&self) -> Result<(), OptimizeError> {
        if !openbnct_core::schema_matches(&self.schema_version, INVERSE_PLAN_OBJECTIVE_SCHEMA) {
            return Err(OptimizeError::UnsupportedObjectiveSchema(
                self.schema_version.clone(),
            ));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("case_id", self.case_id.as_str()),
            ("validity_domain", self.validity_domain.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(OptimizeError::InvalidObjective(format!(
                    "{label} is required and must not be empty"
                )));
            }
        }
        if self.objectives.is_empty() {
            return Err(OptimizeError::InvalidObjective(
                "at least one objective is required".into(),
            ));
        }
        if self.max_iterations == 0 {
            return Err(OptimizeError::InvalidObjective(
                "max_iterations must be positive".into(),
            ));
        }
        if !self.gradient_tolerance.is_finite() || self.gradient_tolerance <= 0.0 {
            return Err(OptimizeError::InvalidObjective(
                "gradient_tolerance must be finite and positive".into(),
            ));
        }
        if let Some(bound) = self.weight_bound
            && (!bound.is_finite() || bound <= 0.0)
        {
            return Err(OptimizeError::InvalidObjective(
                "weight_bound must be finite and positive".into(),
            ));
        }
        if !self.weight_regularization.is_finite() || self.weight_regularization < 0.0 {
            return Err(OptimizeError::InvalidObjective(
                "weight_regularization must be finite and non-negative".into(),
            ));
        }
        match (self.dose_quantity, &self.bio_model) {
            (DoseQuantity::Isoeffective, None) => {
                return Err(OptimizeError::InvalidObjective(
                    "dose_quantity isoeffective requires an embedded bio_model".into(),
                ));
            }
            (DoseQuantity::Isoeffective, Some(model)) => {
                model.validate().map_err(|error| {
                    OptimizeError::InvalidObjective(format!("bio_model: {error}"))
                })?;
                if model.weight_semantics == WeightSemantics::MicrodosimetricKinetic {
                    return Err(OptimizeError::InvalidObjective(
                        "microdosimetric_kinetic models are nonlinear; the optimizer needs component weights".into(),
                    ));
                }
                if model.fractionation.is_some() {
                    return Err(OptimizeError::InvalidObjective(
                        "fractionation transforms the bundle total, not per-voxel objectives — omit it for isoeffective planning".into(),
                    ));
                }
            }
            (_, Some(_)) => {
                return Err(OptimizeError::InvalidObjective(
                    "bio_model is only meaningful with dose_quantity isoeffective".into(),
                ));
            }
            _ => {}
        }
        for objective in &self.objectives {
            let (mask, bound, weight) = match objective {
                DoseObjective::MinEud {
                    mask,
                    target,
                    weight,
                    ..
                } => (mask, target, weight),
                DoseObjective::MaxMean {
                    mask,
                    limit,
                    weight,
                } => (mask, limit, weight),
                DoseObjective::MinDoseAtVolume {
                    mask,
                    target,
                    weight,
                    ..
                } => (mask, target, weight),
                DoseObjective::MaxDoseAtVolume {
                    mask,
                    limit,
                    weight,
                    ..
                } => (mask, limit, weight),
            };
            if mask.trim().is_empty() {
                return Err(OptimizeError::InvalidObjective(
                    "objective mask name must not be empty".into(),
                ));
            }
            if !bound.is_finite() || *bound < 0.0 {
                return Err(OptimizeError::InvalidObjective(
                    "objective bound must be a non-negative finite value".into(),
                ));
            }
            if !weight.is_finite() || *weight <= 0.0 {
                return Err(OptimizeError::InvalidObjective(
                    "objective weight must be finite and positive".into(),
                ));
            }
            match objective {
                DoseObjective::MinEud { eud_a, .. } => {
                    if !eud_a.is_finite() || *eud_a == 0.0 {
                        return Err(OptimizeError::InvalidObjective(
                            "min_eud.eud_a must be finite and non-zero".into(),
                        ));
                    }
                }
                DoseObjective::MinDoseAtVolume {
                    volume_fraction, ..
                }
                | DoseObjective::MaxDoseAtVolume {
                    volume_fraction, ..
                } => {
                    if !volume_fraction.is_finite()
                        || *volume_fraction <= 0.0
                        || *volume_fraction > 1.0
                    {
                        return Err(OptimizeError::InvalidObjective(
                            "volume_fraction must lie in (0, 1]".into(),
                        ));
                    }
                }
                DoseObjective::MaxMean { .. } => {}
            }
        }
        Ok(())
    }
}

/// Evaluate one objective's metric against an accumulated dose field.
/// Returns `(metric, gradient)` — `gradient[v]` is `d(metric)/d(d_v)`
/// for mask voxels, zero elsewhere.
pub(crate) fn objective_metric(
    objective: &DoseObjective,
    dose: &[f64],
    mask_voxels: &[usize],
) -> (f64, Vec<f64>) {
    let mut gradient = vec![0.0; dose.len()];
    match objective {
        DoseObjective::MinEud { eud_a, .. } => {
            let a = *eud_a;
            let n = mask_voxels.len() as f64;
            let sum: f64 = mask_voxels.iter().map(|&v| dose[v].max(0.0).powf(a)).sum();
            let mean_power = sum / n;
            let eud = mean_power.powf(1.0 / a);
            if eud > 0.0 {
                let eud_pow = eud.powf(a - 1.0);
                for &v in mask_voxels {
                    gradient[v] = dose[v].max(0.0).powf(a - 1.0) / (n * eud_pow);
                }
            }
            (eud, gradient)
        }
        DoseObjective::MaxMean { .. } => {
            let n = mask_voxels.len() as f64;
            let mean: f64 = mask_voxels.iter().map(|&v| dose[v]).sum::<f64>() / n;
            for &v in mask_voxels {
                gradient[v] = 1.0 / n;
            }
            (mean, gradient)
        }
        DoseObjective::MinDoseAtVolume {
            volume_fraction, ..
        }
        | DoseObjective::MaxDoseAtVolume {
            volume_fraction, ..
        } => {
            // D_f = dose at the (1−f) quantile of the mask's dose
            // distribution — the hottest f-fraction floor. Piecewise
            // linear in w: the subgradient is the quantile voxel's own
            // dose derivative (1 at the quantile voxel, 0 elsewhere).
            let mut sorted: Vec<(f64, usize)> = mask_voxels.iter().map(|&v| (dose[v], v)).collect();
            sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
            let covered = (mask_voxels.len() as f64 * (1.0 - volume_fraction)).floor() as usize;
            let idx = covered.min(sorted.len() - 1);
            let (_, voxel) = sorted[idx];
            gradient[voxel] = 1.0;
            (sorted[idx].0, gradient)
        }
    }
}

/// Accumulate `Σ w_i·D_i` into `dose`.
fn accumulate(fields: &[BeamDoseField], weights: &[f64], dose: &mut [f64]) {
    for (field, &w) in fields.iter().zip(weights) {
        for (d, &v) in dose.iter_mut().zip(&field.values) {
            *d += w * v;
        }
    }
}

/// The name under which a [`DoseComponent`] appears in
/// [`BeamDoseField::components`] — the serde `snake_case` token.
pub(crate) fn component_name(component: DoseComponent) -> &'static str {
    match component {
        DoseComponent::Boron => "boron",
        DoseComponent::Nitrogen => "nitrogen",
        DoseComponent::Hydrogen => "hydrogen",
        DoseComponent::Photon => "photon",
    }
}

pub(crate) fn objective_mask(objective: &DoseObjective) -> &str {
    match objective {
        DoseObjective::MinEud { mask, .. }
        | DoseObjective::MaxMean { mask, .. }
        | DoseObjective::MinDoseAtVolume { mask, .. }
        | DoseObjective::MaxDoseAtVolume { mask, .. } => mask,
    }
}

/// The dose array and per-beam fields one objective evaluates.
///
/// Physical quantities share the accumulated `Σ w_i·D_i` and each
/// beam's `values`. `Isoeffective` folds the embedded model's component
/// weights — with the objective mask's `region_weights` override when
/// declared — into per-beam effective fields `E_i(v) = Σ_c w_c·D_ic(v)`
/// and the accumulated `Σ_i w_i·E_i(v)`, both linear in `w`.
pub(crate) fn objective_view<'a>(
    fields: &'a [BeamDoseField],
    weights: &[f64],
    shared_dose: &'a [f64],
    spec: &InversePlanObjective,
    objective: &DoseObjective,
) -> (
    std::borrow::Cow<'a, [f64]>,
    Vec<std::borrow::Cow<'a, [f64]>>,
) {
    if spec.dose_quantity != DoseQuantity::Isoeffective {
        return (
            std::borrow::Cow::Borrowed(shared_dose),
            fields
                .iter()
                .map(|f| std::borrow::Cow::Borrowed(&f.values[..]))
                .collect(),
        );
    }
    let model = spec.bio_model.as_ref().expect("validated");
    let weights_map = model
        .region_weights
        .get(objective_mask(objective))
        .unwrap_or(&model.component_weights);
    let n = shared_dose.len();
    let mut dose = vec![0.0; n];
    let mut effective = Vec::with_capacity(fields.len());
    for (field, &wi) in fields.iter().zip(weights) {
        let comps = field.components.as_ref().expect("validated");
        let mut eff = vec![0.0; n];
        for (name, &wc) in weights_map {
            for (v, &cd) in comps[name].iter().enumerate() {
                eff[v] += wc * cd;
                dose[v] += wi * wc * cd;
            }
        }
        effective.push(std::borrow::Cow::Owned(eff));
    }
    (std::borrow::Cow::Owned(dose), effective)
}

/// The composite penalty `Σ_k weight_k·(violation_k/bound_k)²` and its
/// gradient in weight space. Violations are bound-normalized so the
/// penalty is scale-free — the same `weight_regularization` and
/// `gradient_tolerance` apply at any dose magnitude.
fn penalty_and_gradient(
    fields: &[BeamDoseField],
    weights: &[f64],
    spec: &InversePlanObjective,
    mask_voxels: &[Vec<usize>],
    dose: &mut [f64],
) -> (f64, Vec<f64>) {
    dose.fill(0.0);
    if spec.dose_quantity != DoseQuantity::Isoeffective {
        accumulate(fields, weights, dose);
    }
    let mut penalty = 0.0;
    let mut grad = vec![0.0; fields.len()];
    for (objective, voxels) in spec.objectives.iter().zip(mask_voxels) {
        let (odose, ofields) = objective_view(fields, weights, dose, spec, objective);
        let (metric, dmetric_dd) = objective_metric(objective, &odose, voxels);
        // Violations are normalized by the objective bound so the
        // composite penalty is O(1) regardless of the dose units or
        // per-source scale — an absolute quadratic penalty forces
        // `weight_regularization` and `gradient_tolerance` to be
        // re-tuned per problem. A nonpositive bound falls back to the
        // absolute violation (relative is undefined there).
        let (violation, sense, scale) = match objective {
            DoseObjective::MinEud { target, weight, .. }
            | DoseObjective::MinDoseAtVolume { target, weight, .. } => {
                let v = (target - metric).max(0.0);
                let s = if *target > 0.0 { 1.0 / *target } else { 1.0 };
                penalty += weight * v * v * s * s;
                (v, -1.0_f64, s)
            }
            DoseObjective::MaxMean { limit, weight, .. }
            | DoseObjective::MaxDoseAtVolume { limit, weight, .. } => {
                let v = (metric - limit).max(0.0);
                let s = if *limit > 0.0 { 1.0 / *limit } else { 1.0 };
                penalty += weight * v * v * s * s;
                (v, 1.0_f64, s)
            }
        };
        let weight = match objective {
            DoseObjective::MinEud { weight, .. }
            | DoseObjective::MaxMean { weight, .. }
            | DoseObjective::MinDoseAtVolume { weight, .. }
            | DoseObjective::MaxDoseAtVolume { weight, .. } => *weight,
        };
        if violation > 0.0 {
            // d(penalty)/dw_i = 2·weight·violation·scale²·sense·
            // Σ_v dm/dd_v·D_i(v) — under isoeffective, D_i(v) is beam
            // i's effective field Σ_c w_c(mask)·d_ic(v) for this
            // objective.
            for (i, field) in ofields.iter().enumerate() {
                let dm_dwi: f64 = voxels.iter().map(|&v| dmetric_dd[v] * field[v]).sum();
                grad[i] += 2.0 * weight * violation * scale * scale * sense * dm_dwi;
            }
        }
    }
    if spec.weight_regularization > 0.0 {
        for (i, &wi) in weights.iter().enumerate() {
            penalty += spec.weight_regularization * wi;
            grad[i] += spec.weight_regularization;
        }
    }
    (penalty, grad)
}

/// Optimize non-negative beam weights against the objective document.
///
/// `fields[i]` is beam `i`'s unit-weight dose field for the declared
/// `dose_quantity`; `masks` supplies every mask the objectives name.
/// `initial` seeds the iterate (clamped to the feasible box). The
/// solver is deterministic: fixed Armijo backtracking, no sampling.
pub fn optimize_weights(
    fields: &[BeamDoseField],
    masks: &[RegionMask],
    spec: &InversePlanObjective,
    initial: &[f64],
    provenance: ResultProvenance,
) -> Result<InversePlanResult, OptimizeError> {
    spec.validate()?;
    if fields.is_empty() {
        return Err(OptimizeError::InvalidObjective(
            "at least one beam dose field is required".into(),
        ));
    }
    let isoeffective = spec.dose_quantity == DoseQuantity::Isoeffective;
    // Voxel count comes from `values` for physical quantities, from any
    // component volume under isoeffective (where `values` may be empty).
    let n_voxels = {
        let n = fields[0].values.len();
        if n > 0 {
            n
        } else {
            fields[0]
                .components
                .as_ref()
                .and_then(|c| c.values().next())
                .map_or(0, Vec::len)
        }
    };
    if n_voxels == 0 {
        return Err(OptimizeError::EmptyField(fields[0].name.clone()));
    }
    for field in fields {
        if !field.values.is_empty() && field.values.len() != n_voxels {
            return Err(OptimizeError::FieldLengthMismatch(
                field.values.len(),
                n_voxels,
            ));
        }
        if !isoeffective && field.values.is_empty() {
            return Err(OptimizeError::EmptyField(field.name.clone()));
        }
        if isoeffective {
            let comps = field.components.as_ref().ok_or_else(|| {
                OptimizeError::InvalidObjective(format!(
                    "beam field {:?} lacks component-resolved doses required by isoeffective",
                    field.name
                ))
            })?;
            let present: std::collections::BTreeSet<&str> =
                comps.keys().map(String::as_str).collect();
            let required: std::collections::BTreeSet<&str> = DoseComponent::REQUIRED
                .iter()
                .map(|c| component_name(*c))
                .collect();
            if present != required {
                return Err(OptimizeError::InvalidObjective(format!(
                    "beam field {:?} components must be exactly {required:?}; observed {present:?}",
                    field.name
                )));
            }
            for values in comps.values() {
                if values.len() != n_voxels {
                    return Err(OptimizeError::FieldLengthMismatch(values.len(), n_voxels));
                }
            }
        }
    }
    if initial.len() != fields.len() {
        return Err(OptimizeError::InitialLengthMismatch(
            initial.len(),
            fields.len(),
        ));
    }

    // Resolve every mask the objectives name once.
    let mask_voxels: Vec<Vec<usize>> = spec
        .objectives
        .iter()
        .map(|objective| {
            let name = match objective {
                DoseObjective::MinEud { mask, .. }
                | DoseObjective::MaxMean { mask, .. }
                | DoseObjective::MinDoseAtVolume { mask, .. }
                | DoseObjective::MaxDoseAtVolume { mask, .. } => mask.as_str(),
            };
            let mask = masks
                .iter()
                .find(|m| m.name == name)
                .ok_or_else(|| OptimizeError::UnknownMask(name.to_owned()))?;
            let voxels: Vec<usize> = mask
                .voxels
                .iter()
                .enumerate()
                .filter_map(|(i, &on)| if on && i < n_voxels { Some(i) } else { None })
                .collect();
            if voxels.is_empty() {
                return Err(OptimizeError::EmptyMask(name.to_owned()));
            }
            Ok(voxels)
        })
        .collect::<Result<_, OptimizeError>>()?;

    // A bio_model's region overrides apply by mask name — every key
    // must resolve to a supplied mask or the weight would silently
    // never fire.
    if let Some(model) = &spec.bio_model {
        for region in model.region_weights.keys() {
            if !masks.iter().any(|m| m.name == *region) {
                return Err(OptimizeError::UnknownMask(region.clone()));
            }
        }
    }

    let upper = spec.weight_bound.unwrap_or(f64::INFINITY);
    let project = |w: &mut [f64]| {
        for wi in w.iter_mut() {
            *wi = wi.clamp(0.0, upper);
        }
    };

    let mut w: Vec<f64> = initial.to_vec();
    project(&mut w);
    let mut dose = vec![0.0; n_voxels];
    let (mut penalty, mut grad) = penalty_and_gradient(fields, &w, spec, &mask_voxels, &mut dose);

    let mut iterations = 0_u32;
    let mut converged = false;
    for _ in 0..spec.max_iterations {
        // Projected-gradient KKT residual: |g| on interior weights,
        // the inward-pointing part on the boundary.
        let pg_norm: f64 = w
            .iter()
            .zip(&grad)
            .map(|(&wi, &gi)| {
                if wi > 0.0 && wi < upper {
                    gi.abs()
                } else if wi <= 0.0 {
                    gi.min(0.0).abs()
                } else {
                    gi.max(0.0).abs()
                }
            })
            .fold(0.0, f64::max);
        if pg_norm < spec.gradient_tolerance {
            converged = true;
            break;
        }
        // Cyclic coordinate descent — one pass over the weights per
        // iteration. Each coordinate takes its own Armijo-bounded
        // projected step (normalized so the trial move is O(‖w‖)) and
        // the gradient is refreshed after every acceptance. A joint
        // step would let a weight pinned at a sharp feasible minimum
        // veto the whole update — the shared step either overshoots
        // that minimum or shrinks until every coordinate crawls.
        let w_scale = w.iter().fold(1.0_f64, |m, &x| m.max(x));
        let mut improved = false;
        for i in 0..fields.len() {
            let gi = grad[i];
            if !gi.is_finite() || gi == 0.0 {
                continue;
            }
            let mut step_i = w_scale / gi.abs();
            for _ in 0..MAX_BACKTRACKS {
                let mut trial = w.clone();
                trial[i] = w[i] - step_i * gi;
                project(&mut trial);
                let (p2, g2) = penalty_and_gradient(fields, &trial, spec, &mask_voxels, &mut dose);
                if p2 < penalty {
                    w = trial;
                    penalty = p2;
                    grad = g2;
                    improved = true;
                    break;
                }
                step_i *= BACKTRACK_FACTOR;
            }
        }
        if !improved {
            // No coordinate has a strictly-improving move within
            // backtrack resolution — the coordinate-wise local minimum.
            converged = true;
            break;
        }
        iterations += 1;
    }

    // Final outcomes at the optimized weights.
    dose.fill(0.0);
    if spec.dose_quantity != DoseQuantity::Isoeffective {
        accumulate(fields, &w, &mut dose);
    }
    let outcomes: Vec<ObjectiveOutcome> = spec
        .objectives
        .iter()
        .zip(&mask_voxels)
        .map(|(objective, voxels)| {
            let (odose, _) = objective_view(fields, &w, &dose, spec, objective);
            let (metric, _) = objective_metric(objective, &odose, voxels);
            let (kind, bound, satisfied, violation, weight) = match objective {
                DoseObjective::MinEud {
                    mask: _,
                    target,
                    weight,
                    ..
                } => (
                    "min_eud",
                    *target,
                    metric >= *target,
                    (target - metric).max(0.0),
                    *weight,
                ),
                DoseObjective::MinDoseAtVolume {
                    mask: _,
                    target,
                    weight,
                    ..
                } => (
                    "min_dose_at_volume",
                    *target,
                    metric >= *target,
                    (target - metric).max(0.0),
                    *weight,
                ),
                DoseObjective::MaxMean {
                    mask: _,
                    limit,
                    weight,
                } => (
                    "max_mean",
                    *limit,
                    metric <= *limit,
                    (metric - limit).max(0.0),
                    *weight,
                ),
                DoseObjective::MaxDoseAtVolume {
                    mask: _,
                    limit,
                    weight,
                    ..
                } => (
                    "max_dose_at_volume",
                    *limit,
                    metric <= *limit,
                    (metric - limit).max(0.0),
                    *weight,
                ),
            };
            let mask = match objective {
                DoseObjective::MinEud { mask, .. }
                | DoseObjective::MaxMean { mask, .. }
                | DoseObjective::MinDoseAtVolume { mask, .. }
                | DoseObjective::MaxDoseAtVolume { mask, .. } => mask.clone(),
            };
            ObjectiveOutcome {
                kind: kind.to_owned(),
                mask,
                achieved: metric,
                bound,
                satisfied,
                violation,
                weight,
            }
        })
        .collect();

    Ok(InversePlanResult {
        schema_version: INVERSE_PLAN_RESULT_SCHEMA.into(),
        id: provenance.id,
        case_id: spec.case_id.clone(),
        objective: provenance.objective,
        dose_quantity: spec.dose_quantity,
        weights: fields
            .iter()
            .zip(&w)
            .map(|(f, &wi)| BeamWeight {
                name: f.name.clone(),
                weight: wi,
            })
            .collect(),
        outcomes,
        penalty,
        iterations,
        converged,
        qualification: INVERSE_PLAN_QUALIFICATION.into(),
        provenance_id: provenance.provenance_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn objective(objectives: Vec<DoseObjective>) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
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

    /// Two beams: A covers the tumor (voxels 0,1), B covers only the
    /// organ (voxels 2,3). Decoupled fields make the optimum analytic:
    /// w_A meets the tumor objective, w_B sits on its organ constraint.
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
        ];
        (fields, masks)
    }

    #[test]
    fn tumor_eud_and_oar_mean_converge_to_analytic_optimum() {
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
        let result = optimize_weights(&fields, &masks, &spec, &[1.0, 1.0], provenance()).unwrap();

        // Tumor EUD(a=1) = mean = (10w_A + 5w_A)/2 = 7.5 w_A → w_A = 8/3.
        // Beam B deposits only organ dose — the regularizer turns it
        // off entirely (any w_B > 0 wastes weight and harms the OAR).
        assert!((result.weights[0].weight - 8.0 / 3.0).abs() < 1e-2);
        assert!(result.weights[1].weight < 1e-2);
        assert!(result.outcomes.iter().all(|o| o.violation < 1e-3));
    }

    #[test]
    fn dose_at_volume_quantile_tracks_hottest_fraction() {
        let (fields, masks) = two_beam();
        // D50 of the tumor — the hottest-half floor — is the cooler
        // voxel's dose 5 w_A; target 10 → w_A = 2.
        let spec = objective(vec![DoseObjective::MinDoseAtVolume {
            mask: "tumor".into(),
            volume_fraction: 0.5,
            target: 10.0,
            weight: 1.0,
        }]);
        let result = optimize_weights(&fields, &masks, &spec, &[0.5, 0.5], provenance()).unwrap();
        // Regularization keeps the iterate a hair below the boundary —
        // achieved ≈ 10 with a sub-1e-4 residual violation.
        assert!((result.outcomes[0].achieved - 10.0).abs() < 1e-2);
        assert!(result.outcomes[0].violation < 1e-3);
    }

    #[test]
    fn violated_only_objective_pulls_weight_to_zero() {
        let (fields, masks) = two_beam();
        // B is the only field touching the organ — a mean limit of 0
        // drives w_B to the non-negativity boundary.
        let spec = objective(vec![DoseObjective::MaxMean {
            mask: "oar".into(),
            limit: 0.0,
            weight: 1.0,
        }]);
        let result = optimize_weights(&fields, &masks, &spec, &[1.0, 1.0], provenance()).unwrap();
        assert!(result.weights[1].weight < 1e-6);
    }

    #[test]
    fn optimizer_is_deterministic_and_validates() {
        let (fields, masks) = two_beam();
        let spec = objective(vec![DoseObjective::MaxMean {
            mask: "oar".into(),
            limit: 3.0,
            weight: 1.0,
        }]);
        let a = optimize_weights(&fields, &masks, &spec, &[1.0, 1.0], provenance()).unwrap();
        let b = optimize_weights(&fields, &masks, &spec, &[1.0, 1.0], provenance()).unwrap();
        assert_eq!(a.weights, b.weights);

        assert!(matches!(
            optimize_weights(&fields, &masks, &spec, &[1.0], provenance()),
            Err(OptimizeError::InitialLengthMismatch(..))
        ));
        let bad = objective(vec![DoseObjective::MaxMean {
            mask: "missing".into(),
            limit: 1.0,
            weight: 1.0,
        }]);
        assert!(matches!(
            optimize_weights(&fields, &masks, &bad, &[1.0, 1.0], provenance()),
            Err(OptimizeError::UnknownMask(_))
        ));
    }

    #[test]
    fn empty_objectives_rejected() {
        let spec = objective(vec![]);
        assert!(matches!(
            spec.validate(),
            Err(OptimizeError::InvalidObjective(_))
        ));
    }

    fn bio_model(region_weights: BTreeMap<String, openbnct_bio::WeightMap>) -> BiologicalModel {
        BiologicalModel {
            schema_version: openbnct_bio::BIOLOGICAL_MODEL_SCHEMA.into(),
            id: "cbe-model".into(),
            weight_semantics: WeightSemantics::PhotonIsoeffective,
            input_unit: openbnct_core::DoseUnit::GrayPerSourceParticle,
            component_weights: [
                ("boron".into(), 3.0),
                ("nitrogen".into(), 1.0),
                ("hydrogen".into(), 1.0),
                ("photon".into(), 1.0),
            ]
            .into_iter()
            .collect(),
            region_weights,
            region_priority: Vec::new(),
            component_weight_uncertainty: BTreeMap::new(),
            derivation: None,
            validity_domain: Some("unit test".into()),
            fractionation: None,
        }
    }

    fn component_field(name: &str, boron: &[f64], photon: &[f64]) -> BeamDoseField {
        let zeros = vec![0.0; boron.len()];
        BeamDoseField {
            name: name.into(),
            values: boron.iter().zip(photon).map(|(&b, &p)| b + p).collect(),
            components: Some(
                [
                    ("boron".into(), boron.to_vec()),
                    ("nitrogen".into(), zeros.clone()),
                    ("hydrogen".into(), zeros),
                    ("photon".into(), photon.to_vec()),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }

    #[test]
    fn isoeffective_fold_uses_component_and_region_weights() {
        // Beam A: boron 9 + photon 1 on tumor voxel 0, photon 5 on
        // voxel 1 → default eff [28,5] (boron weight 3). Tumor-region
        // override boron=5 → eff [46,5], EUD(a=1) = 25.5·w_A.
        let fields = vec![
            component_field("A", &[9.0, 0.0, 0.0, 0.0], &[1.0, 5.0, 0.0, 0.0]),
            component_field("B", &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 4.0, 4.0]),
        ];
        let masks = vec![
            mask("tumor", &[true, true, false, false]),
            mask("oar", &[false, false, true, true]),
        ];
        let mut spec = objective(vec![
            DoseObjective::MinEud {
                mask: "tumor".into(),
                target: 51.0,
                eud_a: 1.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "oar".into(),
                limit: 8.0,
                weight: 1.0,
            },
        ]);
        spec.dose_quantity = DoseQuantity::Isoeffective;
        spec.bio_model = Some(bio_model(
            [(
                "tumor".into(),
                [
                    ("boron".into(), 5.0),
                    ("nitrogen".into(), 1.0),
                    ("hydrogen".into(), 1.0),
                    ("photon".into(), 1.0),
                ]
                .into_iter()
                .collect(),
            )]
            .into_iter()
            .collect(),
        ));
        spec.validate().unwrap();

        let result = optimize_weights(&fields, &masks, &spec, &[1.0, 1.0], provenance()).unwrap();
        // Tumor sees eff [46,5] → EUD 25.5·w_A = 51 → w_A = 2.
        // OAR sees defaults → mean 4·w_B ≤ 8; regularization turns the
        // useless beam off.
        assert!((result.weights[0].weight - 2.0).abs() < 1e-2);
        assert!(
            result.weights[1].weight < 1e-2,
            "w_B = {}",
            result.weights[1].weight
        );
        assert!(result.outcomes.iter().all(|o| o.violation < 1e-3));
    }

    #[test]
    fn isoeffective_validation_is_strict() {
        let (_fields, masks) = two_beam();
        let fields = vec![
            component_field("A", &[9.0, 0.0, 0.0, 0.0], &[1.0, 5.0, 0.0, 0.0]),
            component_field("B", &[0.0, 0.0, 0.0, 0.0], &[0.0, 0.0, 4.0, 4.0]),
        ];
        let mut spec = objective(vec![DoseObjective::MaxMean {
            mask: "oar".into(),
            limit: 5.0,
            weight: 1.0,
        }]);

        // Isoeffective without a model is rejected.
        spec.dose_quantity = DoseQuantity::Isoeffective;
        assert!(matches!(
            spec.validate(),
            Err(OptimizeError::InvalidObjective(_))
        ));

        // A model on a physical quantity is rejected.
        spec.dose_quantity = DoseQuantity::PhysicalTotal;
        spec.bio_model = Some(bio_model(BTreeMap::new()));
        assert!(matches!(
            spec.validate(),
            Err(OptimizeError::InvalidObjective(_))
        ));

        // MKM semantics rejected; fractionation rejected.
        spec.dose_quantity = DoseQuantity::Isoeffective;
        let mut model = bio_model(BTreeMap::new());
        model.weight_semantics = WeightSemantics::MicrodosimetricKinetic;
        spec.bio_model = Some(model);
        assert!(matches!(
            spec.validate(),
            Err(OptimizeError::InvalidObjective(_))
        ));
        let mut model = bio_model(BTreeMap::new());
        model.fractionation = Some(openbnct_bio::Fractionation {
            fraction_count: 5,
            source_particles_per_fraction: 1e12,
            default_alpha_beta: 10.0,
            region_alpha_beta: BTreeMap::new(),
        });
        spec.bio_model = Some(model);
        assert!(matches!(
            spec.validate(),
            Err(OptimizeError::InvalidObjective(_))
        ));

        // Region names must resolve against supplied masks.
        spec.bio_model = Some(bio_model(
            [(
                "unknown".into(),
                bio_model(BTreeMap::new()).component_weights,
            )]
            .into_iter()
            .collect(),
        ));
        spec.validate().unwrap();
        assert!(matches!(
            optimize_weights(&fields, &masks, &spec, &[1.0, 1.0], provenance()),
            Err(OptimizeError::UnknownMask(_))
        ));

        // Component-resolved fields are mandatory.
        spec.bio_model = Some(bio_model(BTreeMap::new()));
        let (plain_fields, _) = two_beam();
        assert!(matches!(
            optimize_weights(&plain_fields, &masks, &spec, &[1.0, 1.0], provenance()),
            Err(OptimizeError::InvalidObjective(_))
        ));
    }
}
