//! Plan-level systematic-uncertainty propagation.
//!
//! `plan optimize` produces deterministic weights; this module answers
//! the robustness question: under declared systematic sources on each
//! beam's per-component dose, how uncertain is each objective's
//! achieved metric — and what is the Gaussian violation probability
//! against its bound?
//!
//! Propagation model:
//!
//! - Per beam `i`, declared sources contribute component σ maps
//!   `σ_{i,c}(v)` (quadrature-combined over sources). Sources are the
//!   same kinds `uq apply` evaluates: relative component σ
//!   (`rel·D_c(v)`), positioning σ (`|∇D_c|·σ_mm`), and a boron-field
//!   concentration σ on the boron component.
//! - Per objective, component σ maps fold with the objective's
//!   isoeffective weights — including `region_weights` resolved
//!   against the objective's mask — into `σ_{i,eff}(v)² =
//!   Σ_c w_c²·σ_{i,c}(v)²`. Physical-quantity objectives fold with all
//!   weights equal to 1.
//! - Beam weights fold as independent systematic contributions:
//!   `σ_plan(v)² = Σ_i w_i²·σ_{i,eff}(v)²`. Sources declared on
//!   multiple beams are treated as *independent* — the conservative
//!   same-source-across-beams correlation is a documented limitation,
//!   matching the fully-correlated-across-voxels region convention of
//!   [`SystematicUncertaintyReport`].
//! - First-order metric σ: `σ_m = |Σ_v (∂m/∂D_v)·σ_plan(v)|` with the
//!   analytic metric gradient — the fully-correlated linear
//!   propagation, consistent with the region-mean convention.
//! - Violation probability is the one-sided Gaussian tail of the
//!   metric against its bound: `Φ((bound−μ)/σ)` for `min_*` bounds,
//!   `Φ((μ−bound)/σ)` for `max_*` bounds.
//!
//! Emits `openbnct.plan-robustness/0.1.0`, binding the inverse-plan
//! result, its objective document, and every beam dose bundle by
//! content hash. The dose fields are verified against the result's
//! recorded `ContentReference`s — a mismatched bundle refuses rather
//! than silently propagating a different field set.
//!
//! [`SystematicUncertaintyReport`]: openbnct_core::SystematicUncertaintyReport

use std::collections::BTreeMap;

use openbnct_bio::BiologicalModel;
use openbnct_core::{
    CategoryDisposition, ContentReference, DoseComponent, JointUncertaintyInput, SourceSensitivity,
    SourceSharing, UncertaintySource, propagate_first_order,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::optimize::{
    BeamDoseField, DoseObjective, InversePlanObjective, InversePlanResult, component_name,
    objective_mask, objective_metric, objective_view,
};

/// Robustness artifact schema token.
pub const PLAN_ROBUSTNESS_SCHEMA: &str = "openbnct.plan-robustness/0.1.0";

/// Per-beam systematic σ on each dose component, in grid order.
///
/// Assembled caller-side from the same sources `uq apply` evaluates;
/// `components` keys are component names (`boron`, `photon`, …) and
/// every entry is a per-voxel 1σ map already quadrature-combined over
/// that beam's declared sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamSigmaField {
    /// Beam name (matching [`InversePlanResult::beams`] order).
    pub beam: String,
    /// Component name → per-voxel systematic 1σ, same unit as the dose.
    pub components: BTreeMap<String, Vec<f64>>,
}

/// One objective's robustness outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectiveRobustness {
    /// Objective kind tag (`min_eud`, `max_mean`, …).
    pub kind: String,
    /// Region mask the metric evaluates over.
    pub mask: String,
    /// Declared bound (target or limit).
    pub bound: f64,
    /// Achieved metric under the optimized weights.
    pub achieved: f64,
    /// First-order fully-correlated σ of the achieved metric.
    pub sigma_1sigma: f64,
    /// One-sided Gaussian violation probability `P(metric crosses bound)`.
    pub violation_probability: f64,
}

/// A versioned plan-robustness report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanRobustnessReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The inverse-plan result whose weights are propagated.
    pub result: ContentReference,
    /// The objective document the plan was optimized under.
    pub objective: ContentReference,
    /// Per-beam dose-bundle references, in result order.
    pub dose_references: Vec<ContentReference>,
    /// Declared sources (same vocabulary as `uq apply` reports).
    pub sources: Vec<UncertaintySource>,
    /// Per-objective outcomes, parallel to `spec.objectives`.
    pub objectives: Vec<ObjectiveRobustness>,
    /// Propagation convention tag.
    pub method: String,
    pub qualification: String,
    pub provenance_id: String,
}

/// Robustness evaluation errors.
#[derive(Debug, Error)]
pub enum RobustnessError {
    #[error("unsupported plan-robustness schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid plan-robustness report: {0}")]
    Invalid(String),
    #[error("optimizer input error: {0}")]
    Optimize(#[from] crate::optimize::OptimizeError),
}

/// Joint-robustness artifact schema token.
pub const JOINT_ROBUSTNESS_SCHEMA: &str = "openbnct.joint-robustness/0.1.0";

/// One source's signed per-voxel sensitivity map on one beam:
/// `s(v) = ∂D_beam(v)/∂(source draw)` in the evaluated dose unit per
/// unit draw. For a relative-σ scale source this is `rel·D_c(v)` —
/// callers compose it with [`relative_scale_sensitivities`]; a
/// `component` of `None` means the map is already expressed in the
/// objective's evaluated quantity (physical total, or the isoeffective
/// combination), `Some(c)` is weighted by the objective's component
/// weight for `c`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamSourceSensitivity {
    /// Beam name (matching [`InversePlanResult::beams`] order).
    pub beam: String,
    /// `source_id` → sensitivity entries for that beam.
    pub sensitivities: Vec<SourceSensitivityMap>,
}

/// One signed sensitivity map for a (beam, source) pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSensitivityMap {
    /// `JointSource::id` this sensitivity applies to.
    pub source_id: String,
    /// `None` = the map is in the evaluated dose quantity directly;
    /// `Some(name)` = per-component map weighted by the objective's
    /// component weights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Signed per-voxel map in grid order.
    pub values: Vec<f64>,
}

/// Build the sensitivity maps for a scale source that multiplies dose
/// component `component` by `(1 + draw·rel)` — `s_i(v) =
/// rel·D_{i,component}(v)` per beam. For a dimensionless scale draw
/// `rel` is 1.0; for a draw declared in percent it is 0.01 — `rel`
/// converts one unit of draw into a dose multiplier. `component: None`
/// scales the field's evaluated total (`rel·D_total(v)`).
pub fn relative_scale_sensitivities(
    fields: &[BeamDoseField],
    source_id: &str,
    component: Option<&str>,
    rel: f64,
) -> Result<Vec<BeamSourceSensitivity>, RobustnessError> {
    if !rel.is_finite() || rel < 0.0 {
        return Err(RobustnessError::Invalid(
            "relative scale must be finite and non-negative".into(),
        ));
    }
    fields
        .iter()
        .map(|field| {
            let base: &Vec<f64> = match component {
                Some(name) => field
                    .components
                    .as_ref()
                    .and_then(|c| c.get(name))
                    .ok_or_else(|| {
                        RobustnessError::Invalid(format!(
                            "beam {:?} carries no {name:?} component map",
                            field.name
                        ))
                    })?,
                None => &field.values,
            };
            Ok(BeamSourceSensitivity {
                beam: field.name.clone(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: source_id.into(),
                    component: component.map(str::to_string),
                    values: base.iter().map(|v| rel * v).collect(),
                }],
            })
        })
        .collect()
}

/// One objective's outcome under the joint-correlated path — same
/// fields as [`ObjectiveRobustness`] plus the variance contributions
/// per source/correlation group (nonnegative, in metric² units).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointObjectiveRobustness {
    #[serde(flatten)]
    pub robustness: ObjectiveRobustness,
    /// Variance contribution per ungrouped source id, and one entry
    /// per correlation group keyed by the joined member ids — reported
    /// alongside the total, never normalized into shares.
    pub contributions: BTreeMap<String, f64>,
}

/// `openbnct.joint-robustness/0.1.0` — first-order joint-uncertainty
/// robustness report binding the inverse-plan result, objective
/// document, beam dose bundles, and the joint input by hash.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointRobustnessReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// The joint-uncertainty input propagated.
    pub input: ContentReference,
    /// The inverse-plan result whose weights are propagated.
    pub result: ContentReference,
    /// The objective document the plan was optimized under.
    pub objective: ContentReference,
    /// Per-beam dose-bundle references, in result order.
    pub dose_references: Vec<ContentReference>,
    /// Full input echoed for standalone interpretation.
    pub joint_input: JointUncertaintyInput,
    /// Uncertainty-category coverage declared by the input.
    pub categories: Vec<CategoryDisposition>,
    /// Per-objective outcomes, parallel to `spec.objectives`.
    pub objectives: Vec<JointObjectiveRobustness>,
    /// Propagation convention tag (`first_order_gaussian`).
    pub method: String,
    /// Declared assumptions — the same honesty list the ensemble path
    /// carries.
    pub assumptions: Vec<String>,
    pub qualification: String,
    pub provenance_id: String,
}

impl JointRobustnessReport {
    pub fn validate(&self) -> Result<(), RobustnessError> {
        if !openbnct_core::schema_matches(&self.schema_version, JOINT_ROBUSTNESS_SCHEMA) {
            return Err(RobustnessError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("case_id", self.case_id.as_str()),
            ("method", self.method.as_str()),
            ("qualification", self.qualification.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(RobustnessError::Invalid(format!(
                    "{label} must be non-empty"
                )));
            }
        }
        self.input
            .validate()
            .map_err(|e| RobustnessError::Invalid(format!("input reference: {e}")))?;
        self.result
            .validate()
            .map_err(|e| RobustnessError::Invalid(format!("result reference: {e}")))?;
        self.objective
            .validate()
            .map_err(|e| RobustnessError::Invalid(format!("objective reference: {e}")))?;
        self.joint_input
            .validate()
            .map_err(|e| RobustnessError::Invalid(format!("joint input: {e}")))?;
        Ok(())
    }
}

/// Plan robustness under a declared joint-uncertainty input — the
/// correlated counterpart of [`plan_robustness`]. Per objective, the
/// per-(beam, source, component) sensitivity maps fold with the beam
/// weights, the objective's component weights, and the analytic metric
/// gradient into first-order coefficients
/// `a = Σ_{i,c,v} w_i·w_c·(∂m/∂D_v)·s_{i,c,s}(v)`, then
/// [`propagate_first_order`] folds them through the declared
/// covariance: shared draws add linearly (a shared calibration does
/// not shrink when the field is subdivided), `independent_per_target`
/// draws fold in quadrature per beam, and correlation groups form
/// joint ρ·σᵢσⱼ blocks.
///
/// First-order Gaussian assumptions apply: linearized propagation and
/// one-sided normal violation probabilities. Run the ensemble path
/// (`evaluate_joint_dose_ensemble`) for nonlinear or support-
/// constrained behavior; the two paths are complementary, and this
/// function's results are *not* a second implementation of the same
/// quantity — they are the same first-order answer computed without
/// sampling.
#[allow(clippy::too_many_arguments)]
pub fn plan_robustness_joint(
    result: &InversePlanResult,
    spec: &InversePlanObjective,
    fields: &[BeamDoseField],
    input: &JointUncertaintyInput,
    sensitivities: &[BeamSourceSensitivity],
    mask_voxels: &BTreeMap<String, Vec<usize>>,
) -> Result<Vec<JointObjectiveRobustness>, RobustnessError> {
    input
        .validate()
        .map_err(|e| RobustnessError::Invalid(format!("joint input: {e}")))?;
    if fields.len() != result.weights.len() || sensitivities.len() != fields.len() {
        return Err(RobustnessError::Invalid(format!(
            "expected {} beam fields/sensitivities (result order), got {}/{}",
            result.weights.len(),
            fields.len(),
            sensitivities.len()
        )));
    }
    let weights: Vec<f64> = result.weights.iter().map(|b| b.weight).collect();
    let n = fields
        .first()
        .map(|f| {
            if f.values.is_empty() {
                f.components
                    .as_ref()
                    .and_then(|c| c.values().next().map(Vec::len))
                    .unwrap_or(0)
            } else {
                f.values.len()
            }
        })
        .ok_or_else(|| RobustnessError::Invalid("no beam fields".into()))?;
    for (beam_sens, field) in sensitivities.iter().zip(fields) {
        if beam_sens.beam != field.name {
            return Err(RobustnessError::Invalid(format!(
                "sensitivity beam {:?} does not match field {:?} (result order)",
                beam_sens.beam, field.name
            )));
        }
        for map in &beam_sens.sensitivities {
            if map.values.len() != n {
                return Err(RobustnessError::Invalid(format!(
                    "sensitivity {:?} on beam {:?} has {} voxels, expected {n}",
                    map.source_id,
                    beam_sens.beam,
                    map.values.len()
                )));
            }
            if map.values.iter().any(|v| !v.is_finite()) {
                return Err(RobustnessError::Invalid(format!(
                    "sensitivity {:?} on beam {:?} has non-finite values",
                    map.source_id, beam_sens.beam
                )));
            }
            if !input.sources.iter().any(|s| s.id == map.source_id) {
                return Err(RobustnessError::Invalid(format!(
                    "sensitivity references unknown joint source {:?}",
                    map.source_id
                )));
            }
        }
    }

    let mut shared = vec![0.0; n];
    for (field, &w) in fields.iter().zip(&weights) {
        for (d, &v) in shared.iter_mut().zip(&field.values) {
            *d += w * v;
        }
    }

    let isoeffective = spec.dose_quantity == crate::optimize::DoseQuantity::Isoeffective;
    let model: Option<&BiologicalModel> = spec.bio_model.as_ref();

    let mut outcomes = Vec::with_capacity(spec.objectives.len());
    for objective in &spec.objectives {
        let mask_name = objective_mask(objective);
        let voxels = mask_voxels.get(mask_name).ok_or_else(|| {
            RobustnessError::Invalid(format!("objective mask {mask_name:?} has no voxels"))
        })?;
        let (dose, _eff) = objective_view(fields, &weights, &shared, spec, objective);
        let (metric, gradient) = objective_metric(objective, &dose, voxels);
        let weights_map: BTreeMap<String, f64> = if isoeffective {
            let model = model.expect("objective document validated with a bio_model");
            model
                .region_weights
                .get(mask_name)
                .unwrap_or(&model.component_weights)
                .clone()
        } else {
            [
                DoseComponent::Boron,
                DoseComponent::Nitrogen,
                DoseComponent::Hydrogen,
                DoseComponent::Photon,
            ]
            .iter()
            .map(|c| (component_name(*c).to_string(), 1.0))
            .collect()
        };

        // First-order coefficients: for each (beam, source, component)
        // map, a = w_i·w_c·Σ_v∇m·s(v) — signed, summed per draw before
        // the covariance fold.
        let mut coefficients: Vec<SourceSensitivity> = Vec::new();
        for (beam_sens, &w) in sensitivities.iter().zip(&weights) {
            if w == 0.0 {
                continue;
            }
            let beam_index = fields
                .iter()
                .position(|f| f.name == beam_sens.beam)
                .expect("validated");
            for map in &beam_sens.sensitivities {
                let source_index = input
                    .sources
                    .iter()
                    .position(|s| s.id == map.source_id)
                    .expect("validated");
                let wc = match &map.component {
                    Some(component) => weights_map.get(component).copied().unwrap_or(0.0),
                    None => 1.0,
                };
                let a: f64 = w
                    * wc
                    * voxels
                        .iter()
                        .map(|&v| gradient[v] * map.values[v])
                        .sum::<f64>();
                if a == 0.0 {
                    continue;
                }
                let source = &input.sources[source_index];
                let target = match source.sharing {
                    SourceSharing::Shared => None,
                    SourceSharing::IndependentPerTarget => Some(fields[beam_index].name.clone()),
                };
                coefficients.push(SourceSensitivity {
                    source: source_index,
                    target,
                    coefficient: a,
                });
            }
        }
        let (contributions, sigma2) = propagate_first_order(input, &coefficients)
            .map_err(|e| RobustnessError::Invalid(format!("propagation: {e}")))?;
        let sigma_metric = sigma2.sqrt();

        let (bound, sense) = objective_bound(objective);
        let violation_probability = if sigma_metric > 0.0 {
            normal_cdf(sense * (metric - bound) / sigma_metric)
        } else {
            f64::from(sense * (metric - bound) > 0.0)
        };
        outcomes.push(JointObjectiveRobustness {
            robustness: ObjectiveRobustness {
                kind: objective_kind(objective).to_string(),
                mask: mask_name.to_string(),
                bound,
                achieved: metric,
                sigma_1sigma: sigma_metric,
                violation_probability,
            },
            contributions: contributions.into_iter().collect(),
        });
    }
    Ok(outcomes)
}

/// Gaussian Φ(x) — Abramowitz–Stegun 7.1.26, |ε| < 1.5e-7; reporting-
/// grade accuracy for violation probabilities.
fn normal_cdf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.2316419 * x.abs());
    let poly = t
        * (0.319381530
            + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    let pdf = (-0.5 * x * x).exp() / (2.0_f64 * std::f64::consts::PI).sqrt();
    let cdf = 1.0 - pdf * poly;
    if x >= 0.0 { cdf } else { 1.0 - cdf }
}

/// The bound and one-sided sense an objective declares.
fn objective_bound(objective: &DoseObjective) -> (f64, f64) {
    // Returns (bound, sense) where sense = +1 penalizes metric > bound
    // (max_* limits) and −1 penalizes metric < bound (min_* targets).
    match objective {
        DoseObjective::MinEud { target, .. }
        | DoseObjective::MinDoseAtVolume { target, .. }
        | DoseObjective::Maximin { target, .. } => (*target, -1.0),
        DoseObjective::MaxMean { limit, .. } | DoseObjective::MaxDoseAtVolume { limit, .. } => {
            (*limit, 1.0)
        }
    }
}

fn objective_kind(objective: &DoseObjective) -> &'static str {
    match objective {
        DoseObjective::MinEud { .. } => "min_eud",
        DoseObjective::MaxMean { .. } => "max_mean",
        DoseObjective::MinDoseAtVolume { .. } => "min_dose_at_volume",
        DoseObjective::MaxDoseAtVolume { .. } => "max_dose_at_volume",
        DoseObjective::Maximin { .. } => "maximin",
    }
}

/// Evaluate plan robustness: propagate per-beam component σ maps
/// through the optimized weights into per-objective metric σ and
/// Gaussian violation probabilities.
///
/// `fields` are the unit-weight beam dose fields (component-populated
/// under isoeffective objectives) in the same order as
/// `result.beams`; `sigmas` is the matching per-beam σ assembly.
/// `mask_voxels` maps each mask name to its voxel index list — the
/// same resolution `optimize_weights` performs.
#[allow(clippy::too_many_arguments)]
pub fn plan_robustness(
    result: &InversePlanResult,
    spec: &InversePlanObjective,
    fields: &[BeamDoseField],
    sigmas: &[BeamSigmaField],
    mask_voxels: &BTreeMap<String, Vec<usize>>,
) -> Result<Vec<ObjectiveRobustness>, RobustnessError> {
    if fields.len() != result.weights.len() || sigmas.len() != fields.len() {
        return Err(RobustnessError::Invalid(format!(
            "expected {} beam fields/sigmas (result order), got {}/{}",
            result.weights.len(),
            fields.len(),
            sigmas.len()
        )));
    }
    let weights: Vec<f64> = result.weights.iter().map(|b| b.weight).collect();
    let n = fields
        .first()
        .map(|f| {
            if f.values.is_empty() {
                // Isoeffective fields may carry components only.
                f.components
                    .as_ref()
                    .and_then(|c| c.values().next().map(Vec::len))
                    .unwrap_or(0)
            } else {
                f.values.len()
            }
        })
        .ok_or_else(|| RobustnessError::Invalid("no beam fields".into()))?;
    for (beam, sigma) in fields.iter().zip(sigmas) {
        for (name, map) in &sigma.components {
            if map.len() != n {
                return Err(RobustnessError::Invalid(format!(
                    "sigma map {name:?} on beam {:?} has {} voxels, expected {n}",
                    beam.name,
                    map.len()
                )));
            }
        }
    }

    // Plan dose Σ_i w_i·D_i (physical basis) — shared input to
    // objective_view, exactly as the optimizer builds it.
    let mut shared = vec![0.0; n];
    for (field, &w) in fields.iter().zip(&weights) {
        for (d, &v) in shared.iter_mut().zip(&field.values) {
            *d += w * v;
        }
    }

    let isoeffective = spec.dose_quantity == crate::optimize::DoseQuantity::Isoeffective;
    let model: Option<&BiologicalModel> = spec.bio_model.as_ref();

    let mut outcomes = Vec::with_capacity(spec.objectives.len());
    for objective in &spec.objectives {
        let mask_name = objective_mask(objective);
        let voxels = mask_voxels.get(mask_name).ok_or_else(|| {
            RobustnessError::Invalid(format!("objective mask {mask_name:?} has no voxels"))
        })?;

        // Effective fields under this objective's weight convention.
        let (dose, _eff) = objective_view(fields, &weights, &shared, spec, objective);
        let (metric, gradient) = objective_metric(objective, &dose, voxels);

        // The objective's component-weight map: region_weights override
        // for the objective's mask, else component_weights; all-ones for
        // physical quantities.
        let weights_map: BTreeMap<String, f64> = if isoeffective {
            let model = model.expect("objective document validated with a bio_model");
            model
                .region_weights
                .get(mask_name)
                .unwrap_or(&model.component_weights)
                .clone()
        } else {
            [
                DoseComponent::Boron,
                DoseComponent::Nitrogen,
                DoseComponent::Hydrogen,
                DoseComponent::Photon,
            ]
            .iter()
            .map(|c| (component_name(*c).to_string(), 1.0))
            .collect()
        };

        // σ_plan(v)² = Σ_i w_i² Σ_c w_c² σ_{i,c}(v)² — per-objective
        // weight convention; missing component maps are zero-σ.
        let mut sigma_plan = vec![0.0; n];
        for (sigma, &w) in sigmas.iter().zip(&weights) {
            if w == 0.0 {
                continue;
            }
            for (name, &wc) in &weights_map {
                if let Some(map) = sigma.components.get(name) {
                    let factor = w * w * wc * wc;
                    for (s, &sc) in sigma_plan.iter_mut().zip(map) {
                        *s += factor * sc * sc;
                    }
                }
            }
        }
        for s in &mut sigma_plan {
            *s = s.sqrt();
        }

        // First-order, fully-correlated metric σ: |Σ_v ∂m/∂D_v σ(v)|.
        let sigma_metric: f64 = voxels
            .iter()
            .map(|&v| gradient[v] * sigma_plan[v])
            .sum::<f64>()
            .abs();

        let (bound, sense) = objective_bound(objective);
        let violation_probability = if sigma_metric > 0.0 {
            // max_* violates above the bound; min_* below.
            normal_cdf(sense * (metric - bound) / sigma_metric)
        } else {
            // Zero σ: deterministic — violated iff the sense*metric
            // already crosses the bound.
            f64::from(sense * (metric - bound) > 0.0)
        };

        outcomes.push(ObjectiveRobustness {
            kind: objective_kind(objective).to_string(),
            mask: mask_name.to_string(),
            bound,
            achieved: metric,
            sigma_1sigma: sigma_metric,
            violation_probability,
        });
    }
    Ok(outcomes)
}

/// Standard normal CDF, exposed for tests.
#[cfg(test)]
pub(crate) fn cdf(x: f64) -> f64 {
    normal_cdf(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{BeamDoseField, BeamWeight, DoseQuantity, InversePlanResult};
    use openbnct_bio::{BiologicalModel, WeightMap};

    fn component_field(name: &str, boron: &[f64], photon: &[f64]) -> BeamDoseField {
        BeamDoseField {
            name: name.to_string(),
            values: boron.iter().zip(photon).map(|(&b, &p)| b + p).collect(),
            components: Some(
                [
                    ("boron".to_string(), boron.to_vec()),
                    ("photon".to_string(), photon.to_vec()),
                    ("nitrogen".to_string(), vec![0.0; boron.len()]),
                    ("hydrogen".to_string(), vec![0.0; boron.len()]),
                ]
                .into_iter()
                .collect(),
            ),
        }
    }

    fn bio_model() -> BiologicalModel {
        BiologicalModel {
            schema_version: openbnct_bio::BIOLOGICAL_MODEL_SCHEMA.into(),
            id: "test".into(),
            weight_semantics: openbnct_bio::WeightSemantics::PhotonIsoeffective,
            input_unit: openbnct_core::DoseUnit::GrayPerSourceParticle,
            component_weights: WeightMap::from([
                ("boron".into(), 2.0),
                ("nitrogen".into(), 1.0),
                ("hydrogen".into(), 1.0),
                ("photon".into(), 1.0),
            ]),
            region_weights: Default::default(),
            region_priority: Vec::new(),
            component_weight_uncertainty: BTreeMap::new(),
            derivation: None,
            validity_domain: Some("test".into()),
            fractionation: None,
        }
    }

    fn isoeffective_spec(objectives: Vec<DoseObjective>) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: "openbnct.inverse-plan-objective/0.1.0".into(),
            id: "spec".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::Isoeffective,
            objectives,
            max_iterations: 100,
            gradient_tolerance: 1e-8,
            weight_bound: None,
            weight_regularization: 0.0,
            bio_model: Some(bio_model()),
            metrics: None,
            validity_domain: "test".into(),
            provenance_id: "test".into(),
        }
    }

    fn result(weights: Vec<f64>) -> InversePlanResult {
        InversePlanResult {
            schema_version: "openbnct.inverse-plan-result/0.1.0".into(),
            id: "res".into(),
            case_id: "case".into(),
            objective: openbnct_core::ContentReference {
                id: "spec".into(),
                sha256: "sha256:0".repeat(4),
            },
            dose_quantity: DoseQuantity::Isoeffective,
            weights: weights
                .into_iter()
                .enumerate()
                .map(|(i, w)| BeamWeight {
                    name: format!("beam{i}"),
                    weight: w,
                })
                .collect(),
            outcomes: Vec::new(),
            penalty: 0.0,
            iterations: 0,
            converged: true,
            method: None,
            certificate: None,
            metrics: None,
            qualification: "test".into(),
            provenance_id: "test".into(),
        }
    }

    #[test]
    fn relative_sigma_propagates_through_weights_and_eud() {
        // Beam A: boron dose 2.0 at voxel 0; beam B: 1.0. Weights 0.5
        // each → plan boron dose 1.5 → isoeffective (w_B = 2.0) EUD
        // over a single-voxel mask = 3.0. A 10% boron σ on each beam
        // gives σ_plan = sqrt((0.5·2·0.2)² + (0.5·2·0.1)²) = sqrt(0.05)
        // at the mask — mean metric σ equals it exactly.
        let fields = vec![
            component_field("beam0", &[2.0], &[0.0]),
            component_field("beam1", &[1.0], &[0.0]),
        ];
        let sigmas = vec![
            BeamSigmaField {
                beam: "beam0".into(),
                components: [("boron".to_string(), vec![0.2])].into_iter().collect(),
            },
            BeamSigmaField {
                beam: "beam1".into(),
                components: [("boron".to_string(), vec![0.1])].into_iter().collect(),
            },
        ];
        let spec = isoeffective_spec(vec![DoseObjective::MinEud {
            mask: "tumor".into(),
            target: 2.0,
            eud_a: 1.0,
            weight: 1.0,
        }]);
        let masks = [("tumor".to_string(), vec![0usize])].into_iter().collect();
        let outcomes =
            plan_robustness(&result(vec![0.5, 0.5]), &spec, &fields, &sigmas, &masks).unwrap();
        let o = &outcomes[0];
        assert!((o.achieved - 3.0).abs() < 1e-12);
        let expected = (0.05_f64).sqrt();
        assert!((o.sigma_1sigma - expected).abs() < 1e-12);
        // achieved 3.0 vs bound 2.0 → Φ((3−2)/σ) deep inside the tail.
        assert!(o.violation_probability < 1e-4);
    }

    #[test]
    fn max_objective_violation_probability_one_sided() {
        // Achieved above a max_* bound must report P ≈ 1 even at zero σ.
        let fields = vec![component_field("beam0", &[1.0], &[0.0])];
        let sigmas = vec![BeamSigmaField {
            beam: "beam0".into(),
            components: Default::default(),
        }];
        let spec = isoeffective_spec(vec![DoseObjective::MaxMean {
            mask: "oar".into(),
            limit: 1.0,
            weight: 1.0,
        }]);
        let masks = [("oar".to_string(), vec![0usize])].into_iter().collect();
        let outcomes =
            plan_robustness(&result(vec![1.0]), &spec, &fields, &sigmas, &masks).unwrap();
        // isoeffective dose = 2.0 vs limit 1.0, σ = 0 → P = 1.
        assert_eq!(outcomes[0].violation_probability, 1.0);
    }

    #[test]
    fn normal_cdf_is_standard() {
        assert!((cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((cdf(1.96) - 0.9750021).abs() < 1e-4);
        assert!((cdf(-1.96) - 0.0249979).abs() < 1e-4);
    }

    // --- UQ-01 joint-robustness fixtures -------------------------------

    use openbnct_core::{
        CategoryStatus, CorrelationGroup, Distribution, JointSource, SourceEvidence, SourceTarget,
        Support, UncertaintyCategory,
    };

    fn physical_spec() -> InversePlanObjective {
        InversePlanObjective {
            schema_version: "openbnct.inverse-plan-objective/0.1.0".into(),
            id: "spec".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::PhysicalTotal,
            objectives: vec![DoseObjective::MaxMean {
                mask: "tumor".into(),
                limit: 10.0,
                weight: 1.0,
            }],
            max_iterations: 100,
            gradient_tolerance: 1e-8,
            weight_bound: None,
            weight_regularization: 0.0,
            bio_model: None,
            validity_domain: "test".into(),
            provenance_id: "test".into(),
        }
    }

    fn joint_source(id: &str, sharing: SourceSharing, scope: Vec<SourceTarget>) -> JointSource {
        JointSource {
            id: id.into(),
            category: UncertaintyCategory::InputParameter,
            scope,
            sharing,
            unit: "1".into(),
            // First-order propagation uses σ only; the declared real
            // support is honest — a Normal scale is never sampled here.
            support: Support::Real,
            distribution: Distribution::Normal {
                mean: 1.0,
                std_dev: 0.1,
            },
            evidence: SourceEvidence {
                basis: "synthetic fixture".into(),
                reference: None,
            },
        }
    }

    fn joint_input(
        sources: Vec<JointSource>,
        correlations: Vec<CorrelationGroup>,
    ) -> JointUncertaintyInput {
        JointUncertaintyInput {
            schema_version: openbnct_core::JOINT_UNCERTAINTY_INPUT_SCHEMA.into(),
            id: "joint.test".into(),
            subjects: Vec::new(),
            sources,
            correlations,
            category_disposition: [
                UncertaintyCategory::StatisticalSampling,
                UncertaintyCategory::StructuralModel,
                UncertaintyCategory::ModelDiscrepancy,
            ]
            .iter()
            .map(|category| CategoryDisposition {
                category: *category,
                status: CategoryStatus::Unassessed,
                note: "out of scope for this fixture".into(),
            })
            .collect(),
            qualification: openbnct_core::JOINT_UNCERTAINTY_QUALIFICATION.into(),
            provenance_id: "test".into(),
        }
    }

    fn field(name: &str, dose: f64) -> BeamDoseField {
        BeamDoseField {
            name: name.into(),
            values: vec![dose],
            components: None,
        }
    }

    #[test]
    fn joint_shared_vs_independent_two_field_fixture() {
        // The handoff's canonical check: fields 2 and 3 with relative σ
        // 0.1 — shared → σ = 0.1·(2+3) = 0.5; independent →
        // sqrt(0.2²+0.3²) = sqrt(0.13) ≈ 0.3606.
        let fields = vec![field("beam0", 2.0), field("beam1", 3.0)];
        let weights = result(vec![1.0, 1.0]);
        let spec = physical_spec();
        let masks = [("tumor".to_string(), vec![0usize])].into_iter().collect();

        // Shared: one draw scales both fields — a = 0.1·2 + 0.1·3 = 0.5.
        let shared = joint_input(
            vec![joint_source(
                "output",
                SourceSharing::Shared,
                vec![SourceTarget::Global],
            )],
            Vec::new(),
        );
        let sens: Vec<BeamSourceSensitivity> = fields
            .iter()
            .map(|f| BeamSourceSensitivity {
                beam: f.name.clone(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: "output".into(),
                    component: None,
                    values: f.values.clone(),
                }],
            })
            .collect();
        let out = plan_robustness_joint(&weights, &spec, &fields, &shared, &sens, &masks).unwrap();
        assert!((out[0].robustness.achieved - 5.0).abs() < 1e-12);
        assert!((out[0].robustness.sigma_1sigma - 0.5).abs() < 1e-12);
        assert!((out[0].contributions["output"] - 0.25).abs() < 1e-12);

        // Independent per beam: two separate draws fold in quadrature.
        let independent = joint_input(
            vec![joint_source(
                "output",
                SourceSharing::IndependentPerTarget,
                vec![
                    SourceTarget::Field {
                        name: "beam0".into(),
                    },
                    SourceTarget::Field {
                        name: "beam1".into(),
                    },
                ],
            )],
            Vec::new(),
        );
        let out =
            plan_robustness_joint(&weights, &spec, &fields, &independent, &sens, &masks).unwrap();
        let expected = (0.13_f64).sqrt();
        assert!((out[0].robustness.sigma_1sigma - expected).abs() < 1e-12);
        assert!((out[0].contributions["output"] - 0.13).abs() < 1e-12);

        // Two ungrouped shared sources, one per field — same quadrature.
        let two_shared = joint_input(
            vec![
                joint_source(
                    "cal-a",
                    SourceSharing::Shared,
                    vec![SourceTarget::Field {
                        name: "beam0".into(),
                    }],
                ),
                joint_source(
                    "cal-b",
                    SourceSharing::Shared,
                    vec![SourceTarget::Field {
                        name: "beam1".into(),
                    }],
                ),
            ],
            Vec::new(),
        );
        let sens2 = vec![
            BeamSourceSensitivity {
                beam: "beam0".into(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: "cal-a".into(),
                    component: None,
                    values: vec![2.0],
                }],
            },
            BeamSourceSensitivity {
                beam: "beam1".into(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: "cal-b".into(),
                    component: None,
                    values: vec![3.0],
                }],
            },
        ];
        let out =
            plan_robustness_joint(&weights, &spec, &fields, &two_shared, &sens2, &masks).unwrap();
        assert!((out[0].robustness.sigma_1sigma - expected).abs() < 1e-12);
    }

    #[test]
    fn joint_shared_sigma_survives_field_splitting() {
        // Splitting a field must not shrink a shared uncertainty:
        // [2, 3] under one shared draw gives the same σ as
        // [2, 1.5, 1.5]; three independent draws do not.
        let spec = physical_spec();
        let masks = [("tumor".to_string(), vec![0usize])].into_iter().collect();
        let fields3 = vec![
            field("beam0", 2.0),
            field("beam1a", 1.5),
            field("beam1b", 1.5),
        ];
        let weights3 = result(vec![1.0, 1.0, 1.0]);
        let sens3: Vec<BeamSourceSensitivity> = fields3
            .iter()
            .map(|f| BeamSourceSensitivity {
                beam: f.name.clone(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: "output".into(),
                    component: None,
                    values: f.values.clone(),
                }],
            })
            .collect();

        let shared = joint_input(
            vec![joint_source(
                "output",
                SourceSharing::Shared,
                vec![SourceTarget::Global],
            )],
            Vec::new(),
        );
        let out =
            plan_robustness_joint(&weights3, &spec, &fields3, &shared, &sens3, &masks).unwrap();
        // 0.2 + 0.15 + 0.15 = 0.5 — identical to the unsplit case.
        assert!((out[0].robustness.sigma_1sigma - 0.5).abs() < 1e-12);

        let per_target = joint_input(
            vec![joint_source(
                "output",
                SourceSharing::IndependentPerTarget,
                vec![
                    SourceTarget::Field {
                        name: "beam0".into(),
                    },
                    SourceTarget::Field {
                        name: "beam1a".into(),
                    },
                    SourceTarget::Field {
                        name: "beam1b".into(),
                    },
                ],
            )],
            Vec::new(),
        );
        let out =
            plan_robustness_joint(&weights3, &spec, &fields3, &per_target, &sens3, &masks).unwrap();
        // sqrt(0.2² + 0.15² + 0.15²) = sqrt(0.085).
        let expected = (0.085_f64).sqrt();
        assert!((out[0].robustness.sigma_1sigma - expected).abs() < 1e-12);
    }

    #[test]
    fn joint_correlation_group_interpolates_between_limits() {
        // ρ=1 must reproduce the shared-draw answer; ρ=0 the quadrature
        // answer; ρ=0.5 lies between, and the group contribution is the
        // full quadratic form.
        let fields = vec![field("beam0", 2.0), field("beam1", 3.0)];
        let weights = result(vec![1.0, 1.0]);
        let spec = physical_spec();
        let masks = [("tumor".to_string(), vec![0usize])].into_iter().collect();
        let sens = vec![
            BeamSourceSensitivity {
                beam: "beam0".into(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: "a".into(),
                    component: None,
                    values: vec![2.0],
                }],
            },
            BeamSourceSensitivity {
                beam: "beam1".into(),
                sensitivities: vec![SourceSensitivityMap {
                    source_id: "b".into(),
                    component: None,
                    values: vec![3.0],
                }],
            },
        ];
        for (rho, expected_var) in [(1.0, 0.25), (0.0, 0.13), (0.5, 0.19)] {
            let input = joint_input(
                vec![
                    joint_source("a", SourceSharing::Shared, vec![SourceTarget::Global]),
                    joint_source("b", SourceSharing::Shared, vec![SourceTarget::Global]),
                ],
                vec![CorrelationGroup {
                    sources: vec!["a".into(), "b".into()],
                    correlation: vec![1.0, rho, rho, 1.0],
                }],
            );
            let out =
                plan_robustness_joint(&weights, &spec, &fields, &input, &sens, &masks).unwrap();
            assert!(
                (out[0].robustness.sigma_1sigma.powi(2) - expected_var).abs() < 1e-12,
                "ρ={rho}: σ² {} != {expected_var}",
                out[0].robustness.sigma_1sigma.powi(2)
            );
            // The group's quadratic-form contribution is the whole σ²
            // (its two members carry all declared variance).
            assert!((out[0].contributions["a+b"] - expected_var).abs() < 1e-12);
        }
    }
}
