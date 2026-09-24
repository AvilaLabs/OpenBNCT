// SPDX-License-Identifier: MIT
//
//! Discrete scenario-set plan evaluation — the complement to
//! [`crate::robustness`]'s first-order Gaussian propagation.
//!
//! A `plan-robustness` report assumes every uncertainty is a Gaussian
//! per-voxel σ combined in quadrature — the right model for
//! small random errors but the wrong shape for BNCT's dominant
//! structured uncertainties: boron uptake inferred from a single
//! pre-treatment scan (bounded, possibly asymmetric), T/N ratios that
//! vary per structure, and whole-field positioning offsets. Those are
//! *scenarios*: named, discrete perturbations evaluated exactly by
//! rescaling and resampling the beam fields, not linearized around the
//! nominal.
//!
//! A [`PlanScenarioSet`] declares the scenarios; [`evaluate_scenarios`]
//! applies each one to the unit-weight beam fields, refolds the
//! optimized weights, and re-evaluates every objective's achieved
//! metric — producing per-scenario outcomes and per-objective bands
//! (nominal, min/max/mean, worst scenario, violated set). No transport
//! is re-solved: component scales are exact (dose is linear in
//! concentration and in component yield), and `shift_mm` is a declared
//! trilinear-resampling approximation of a whole-field displacement —
//! honest for millimetre-scale offsets, recorded as such.

use std::collections::BTreeMap;

use openbnct_core::{ContentReference, GridGeometry};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::optimize::{
    BeamDoseField, InversePlanObjective, InversePlanResult, objective_mask, objective_metric,
    objective_view,
};

/// Scenario-set artifact schema token.
pub const PLAN_SCENARIO_SET_SCHEMA: &str = "openbnct.scenario-set/0.1.0";
/// Scenario-report artifact schema token.
pub const PLAN_SCENARIO_REPORT_SCHEMA: &str = "openbnct.scenario-report/0.1.0";

/// Per-mask component rescaling — T/N-ratio uncertainty, where the
/// scale only applies inside the named region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionComponentScale {
    /// Region mask name (resolved against the supplied masks).
    pub mask: String,
    /// Component name → multiplicative scale at masked voxels.
    pub component_scales: BTreeMap<String, f64>,
}

/// One named discrete perturbation of the delivered plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanScenario {
    /// Unique scenario name (`"nominal"` is reserved — the unperturbed
    /// evaluation is always reported first).
    pub name: String,
    /// Free-text basis note (measurement spread, literature range).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Uniform scale on every component and the physical total —
    /// source-strength / output-factor uncertainty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dose_scale: Option<f64>,
    /// Component name → multiplicative scale applied everywhere —
    /// global boron-uptake or component-yield uncertainty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub component_scales: BTreeMap<String, f64>,
    /// Per-region component scales — each entry multiplies the named
    /// components at that mask's voxels, in declared order. This is
    /// how T/N uncertainty is expressed: tumor uptake scaling that
    /// does not touch normal tissue.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region_scales: Vec<RegionComponentScale>,
    /// Whole-field displacement in LPS millimetres: the delivered
    /// field value at patient point `p` is sampled at `p − shift_mm`
    /// by trilinear interpolation on the beam's own grid. A declared
    /// approximation of a positioning offset — dose transported
    /// *through* shifted anatomy is not re-solved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift_mm: Option<[f64; 3]>,
}

/// A versioned set of named plan scenarios.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanScenarioSet {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Free-text basis note for the set (uncertainty budget source).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub scenarios: Vec<PlanScenario>,
}

impl PlanScenarioSet {
    pub fn validate(&self) -> Result<(), ScenarioError> {
        if !openbnct_core::schema_matches(&self.schema_version, PLAN_SCENARIO_SET_SCHEMA) {
            return Err(ScenarioError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(ScenarioError::Invalid("scenario-set id is empty".into()));
        }
        if self.scenarios.is_empty() {
            return Err(ScenarioError::Invalid(
                "scenario set declares no scenarios".into(),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for scenario in &self.scenarios {
            if scenario.name.trim().is_empty() {
                return Err(ScenarioError::Invalid("scenario name is empty".into()));
            }
            if scenario.name == "nominal" {
                return Err(ScenarioError::Invalid(
                    "\"nominal\" is reserved for the unperturbed evaluation".into(),
                ));
            }
            if !names.insert(&scenario.name) {
                return Err(ScenarioError::Invalid(format!(
                    "duplicate scenario name {:?}",
                    scenario.name
                )));
            }
            let scales = [&scenario.dose_scale]
                .into_iter()
                .flatten()
                .chain(scenario.component_scales.values())
                .chain(
                    scenario
                        .region_scales
                        .iter()
                        .flat_map(|r| r.component_scales.values()),
                );
            for scale in scales {
                if !scale.is_finite() || *scale < 0.0 {
                    return Err(ScenarioError::Invalid(format!(
                        "scenario {:?}: scales must be non-negative finite values",
                        scenario.name
                    )));
                }
            }
            for region in &scenario.region_scales {
                if region.mask.trim().is_empty() {
                    return Err(ScenarioError::Invalid(format!(
                        "scenario {:?}: region-scale mask name is empty",
                        scenario.name
                    )));
                }
            }
            if let Some(shift) = scenario.shift_mm
                && shift.iter().any(|v| !v.is_finite())
            {
                return Err(ScenarioError::Invalid(format!(
                    "scenario {:?}: shift_mm must be finite",
                    scenario.name
                )));
            }
        }
        Ok(())
    }
}

/// One objective's achieved metric under one scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioObjectiveOutcome {
    pub kind: String,
    pub mask: String,
    pub bound: f64,
    pub achieved: f64,
    pub satisfied: bool,
}

/// All objective outcomes under one scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioEvaluation {
    /// `"nominal"` for the unperturbed evaluation, else the scenario name.
    pub scenario: String,
    pub objectives: Vec<ScenarioObjectiveOutcome>,
}

/// One objective's cross-scenario band.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectiveBand {
    pub kind: String,
    pub mask: String,
    pub bound: f64,
    pub nominal_achieved: f64,
    pub min_achieved: f64,
    pub max_achieved: f64,
    pub mean_achieved: f64,
    /// The scenario name at the bound-violating extreme — minimum
    /// achieved for `min_*` objectives, maximum for `max_*`.
    pub worst_scenario: String,
    /// Names of scenarios (excluding nominal) whose achieved value
    /// violates the bound.
    pub violated_scenarios: Vec<String>,
}

/// A versioned scenario-evaluation report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanScenarioReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The inverse-plan result whose weights are evaluated.
    pub result: ContentReference,
    /// The objective document the plan was optimized under.
    pub objective: ContentReference,
    /// The scenario set evaluated.
    pub scenario_set: ContentReference,
    /// Per-beam dose-bundle references, in result order.
    pub dose_references: Vec<ContentReference>,
    /// Nominal evaluation followed by one entry per declared scenario.
    pub evaluations: Vec<ScenarioEvaluation>,
    /// Per-objective cross-scenario bands, parallel to the objective
    /// document's objectives.
    pub bands: Vec<ObjectiveBand>,
    pub qualification: String,
    pub provenance_id: String,
}

/// Scenario evaluation errors.
#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error("unsupported scenario-set schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid scenario input: {0}")]
    Invalid(String),
}

fn objective_kind(objective: &crate::optimize::DoseObjective) -> &'static str {
    match objective {
        crate::optimize::DoseObjective::MinEud { .. } => "min_eud",
        crate::optimize::DoseObjective::MaxMean { .. } => "max_mean",
        crate::optimize::DoseObjective::MinDoseAtVolume { .. } => "min_dose_at_volume",
        crate::optimize::DoseObjective::MaxDoseAtVolume { .. } => "max_dose_at_volume",
    }
}

fn objective_bound(objective: &crate::optimize::DoseObjective) -> f64 {
    match objective {
        crate::optimize::DoseObjective::MinEud { target, .. }
        | crate::optimize::DoseObjective::MinDoseAtVolume { target, .. } => *target,
        crate::optimize::DoseObjective::MaxMean { limit, .. }
        | crate::optimize::DoseObjective::MaxDoseAtVolume { limit, .. } => *limit,
    }
}

/// Trilinear resample of a grid-ordered field: value at each voxel
/// center `p` becomes the field's interpolated value at `p − shift_mm`.
/// Samples outside the grid return 0 — a shifted field delivers no
/// dose beyond its extent.
fn resample_shifted(
    values: &[f64],
    geometry: &GridGeometry,
    shift_mm: [f64; 3],
) -> Result<Vec<f64>, ScenarioError> {
    let n = geometry.voxel_count().map_err(|e| {
        ScenarioError::Invalid(format!("beam field geometry is not a usable grid: {e}"))
    })?;
    if values.len() != n {
        return Err(ScenarioError::Invalid(format!(
            "beam field has {} voxels, grid declares {n}",
            values.len()
        )));
    }
    let d = geometry.direction;
    // Direction rows are orthonormal axis vectors (validated grid
    // contract), so the world→local map is the transpose.
    let mut out = vec![0.0; n];
    let [sx, sy, sz] = geometry.spacing_mm;
    for (flat, value) in out.iter_mut().enumerate() {
        let i = (flat % geometry.shape[0] as usize) as f64;
        let j = ((flat / geometry.shape[0] as usize) % geometry.shape[1] as usize) as f64;
        let k = (flat / (geometry.shape[0] as usize * geometry.shape[1] as usize)) as f64;
        let local = [i * sx, j * sy, k * sz];
        let center = [
            geometry.origin_mm[0] + d[0].mul_add(local[0], d[1].mul_add(local[1], d[2] * local[2])),
            geometry.origin_mm[1] + d[3].mul_add(local[0], d[4].mul_add(local[1], d[5] * local[2])),
            geometry.origin_mm[2] + d[6].mul_add(local[0], d[7].mul_add(local[1], d[8] * local[2])),
        ];
        let q = [
            center[0] - shift_mm[0] - geometry.origin_mm[0],
            center[1] - shift_mm[1] - geometry.origin_mm[1],
            center[2] - shift_mm[2] - geometry.origin_mm[2],
        ];
        // Fractional voxel coordinates of the sample point.
        let f = [
            (d[0] * q[0] + d[3] * q[1] + d[6] * q[2]) / sx,
            (d[1] * q[0] + d[4] * q[1] + d[7] * q[2]) / sy,
            (d[2] * q[0] + d[5] * q[1] + d[8] * q[2]) / sz,
        ];
        let lo = [f[0].floor(), f[1].floor(), f[2].floor()];
        let frac = [f[0] - lo[0], f[1] - lo[1], f[2] - lo[2]];
        let sample = |di: i64, dj: i64, dk: i64| -> f64 {
            let (x, y, z) = (lo[0] as i64 + di, lo[1] as i64 + dj, lo[2] as i64 + dk);
            if x < 0
                || y < 0
                || z < 0
                || x >= geometry.shape[0] as i64
                || y >= geometry.shape[1] as i64
                || z >= geometry.shape[2] as i64
            {
                return 0.0;
            }
            values[(x + geometry.shape[0] as i64 * (y + geometry.shape[1] as i64 * z)) as usize]
        };
        *value = (0i64..2)
            .flat_map(|dk| {
                (0i64..2).flat_map(move |dj| {
                    (0i64..2).map(move |di| {
                        let w = if di == 0 { 1.0 - frac[0] } else { frac[0] }
                            * if dj == 0 { 1.0 - frac[1] } else { frac[1] }
                            * if dk == 0 { 1.0 - frac[2] } else { frac[2] };
                        w * sample(di, dj, dk)
                    })
                })
            })
            .sum();
    }
    Ok(out)
}

/// Apply one scenario to every beam field. Component maps are scaled
/// globally, then per-region at mask voxels; a uniform `dose_scale`
/// multiplies everything; `shift_mm` resamples each component (and
/// `values` when no component map exists). `values` is rebuilt as the
/// component sum when a component map exists — the bundle's own
/// physical-total convention.
fn perturb_fields(
    fields: &[BeamDoseField],
    scenario: &PlanScenario,
    geometry: &GridGeometry,
    mask_voxels: &BTreeMap<String, Vec<usize>>,
) -> Result<Vec<BeamDoseField>, ScenarioError> {
    let dose_scale = scenario.dose_scale.unwrap_or(1.0);
    let needs_components = !scenario.component_scales.is_empty()
        || scenario
            .region_scales
            .iter()
            .any(|r| !r.component_scales.is_empty());
    let mut out = Vec::with_capacity(fields.len());
    for field in fields {
        let (values, components) = match &field.components {
            Some(components) => {
                let mut perturbed: BTreeMap<String, Vec<f64>> = BTreeMap::new();
                for (name, map) in components {
                    let mut v = map.clone();
                    let scale =
                        scenario.component_scales.get(name).copied().unwrap_or(1.0) * dose_scale;
                    if scale != 1.0 {
                        for x in &mut v {
                            *x *= scale;
                        }
                    }
                    perturbed.insert(name.clone(), v);
                }
                for region in &scenario.region_scales {
                    let voxels = mask_voxels.get(&region.mask).ok_or_else(|| {
                        ScenarioError::Invalid(format!(
                            "scenario {:?}: region-scale mask {:?} has no voxels",
                            scenario.name, region.mask
                        ))
                    })?;
                    for (name, &scale) in &region.component_scales {
                        let map = perturbed.get_mut(name).ok_or_else(|| {
                            ScenarioError::Invalid(format!(
                                "scenario {:?}: region scale names unknown component {name:?}",
                                scenario.name
                            ))
                        })?;
                        for &voxel in voxels {
                            if voxel < map.len() {
                                map[voxel] *= scale;
                            }
                        }
                    }
                }
                let mut total = vec![0.0; field.values.len()];
                for map in perturbed.values() {
                    for (t, &x) in total.iter_mut().zip(map) {
                        *t += x;
                    }
                }
                (total, Some(perturbed))
            }
            None => {
                if needs_components {
                    return Err(ScenarioError::Invalid(format!(
                        "scenario {:?} declares component or region scales but beam field {:?} carries no component map",
                        scenario.name, field.name
                    )));
                }
                let values: Vec<f64> = field.values.iter().map(|v| v * dose_scale).collect();
                (values, None)
            }
        };
        let (values, components) = match scenario.shift_mm {
            Some(shift) if shift.iter().any(|s| *s != 0.0) => {
                let shifted_components = components
                    .map(|components| {
                        components
                            .into_iter()
                            .map(|(name, map)| {
                                resample_shifted(&map, geometry, shift)
                                    .map(|resampled| (name, resampled))
                            })
                            .collect::<Result<BTreeMap<_, _>, _>>()
                    })
                    .transpose()?;
                let total = match &shifted_components {
                    // Keep `values` consistent with the shifted
                    // component sum rather than its own resample.
                    Some(components) => {
                        let mut total = vec![0.0; values.len()];
                        for map in components.values() {
                            for (t, &x) in total.iter_mut().zip(map) {
                                *t += x;
                            }
                        }
                        total
                    }
                    None => resample_shifted(&values, geometry, shift)?,
                };
                (total, shifted_components)
            }
            _ => (values, components),
        };
        out.push(BeamDoseField {
            name: field.name.clone(),
            values,
            components,
        });
    }
    Ok(out)
}

/// Evaluate the optimized weight assignment under every declared
/// scenario plus the nominal plan.
///
/// `fields` are the unit-weight beam dose fields in
/// [`InversePlanResult::weights`] order; `geometry` is their shared
/// grid (required for `shift_mm` resampling); `mask_voxels` resolves
/// both objective masks and scenario region-scale masks.
pub fn evaluate_scenarios(
    result: &InversePlanResult,
    spec: &InversePlanObjective,
    fields: &[BeamDoseField],
    geometry: &GridGeometry,
    scenario_set: &PlanScenarioSet,
    mask_voxels: &BTreeMap<String, Vec<usize>>,
) -> Result<(Vec<ScenarioEvaluation>, Vec<ObjectiveBand>), ScenarioError> {
    scenario_set.validate()?;
    if fields.len() != result.weights.len() {
        return Err(ScenarioError::Invalid(format!(
            "expected {} beam fields (result order), got {}",
            result.weights.len(),
            fields.len()
        )));
    }
    let weights: Vec<f64> = result.weights.iter().map(|b| b.weight).collect();

    let nominal = PlanScenario {
        name: "nominal".into(),
        note: None,
        dose_scale: None,
        component_scales: BTreeMap::new(),
        region_scales: Vec::new(),
        shift_mm: None,
    };
    let all = std::iter::once(&nominal).chain(scenario_set.scenarios.iter());
    let mut evaluations = Vec::with_capacity(scenario_set.scenarios.len() + 1);
    for scenario in all {
        let perturbed = perturb_fields(fields, scenario, geometry, mask_voxels)?;
        let n = perturbed.first().map(|f| f.values.len()).unwrap_or(0);
        let mut shared = vec![0.0; n];
        for (field, &w) in perturbed.iter().zip(&weights) {
            for (d, &v) in shared.iter_mut().zip(&field.values) {
                *d += w * v;
            }
        }
        let mut objectives = Vec::with_capacity(spec.objectives.len());
        for objective in &spec.objectives {
            let voxels = mask_voxels.get(objective_mask(objective)).ok_or_else(|| {
                ScenarioError::Invalid(format!(
                    "objective mask {:?} has no voxels",
                    objective_mask(objective)
                ))
            })?;
            let (dose, _effective) = objective_view(&perturbed, &weights, &shared, spec, objective);
            let (achieved, _) = objective_metric(objective, &dose, voxels);
            let bound = objective_bound(objective);
            let satisfied = match objective {
                crate::optimize::DoseObjective::MinEud { .. }
                | crate::optimize::DoseObjective::MinDoseAtVolume { .. } => achieved >= bound,
                crate::optimize::DoseObjective::MaxMean { .. }
                | crate::optimize::DoseObjective::MaxDoseAtVolume { .. } => achieved <= bound,
            };
            objectives.push(ScenarioObjectiveOutcome {
                kind: objective_kind(objective).into(),
                mask: objective_mask(objective).into(),
                bound,
                achieved,
                satisfied,
            });
        }
        evaluations.push(ScenarioEvaluation {
            scenario: scenario.name.clone(),
            objectives,
        });
    }

    let mut bands = Vec::with_capacity(spec.objectives.len());
    for (index, objective) in spec.objectives.iter().enumerate() {
        let bound = objective_bound(objective);
        let per_scenario: Vec<(&str, f64, bool)> = evaluations
            .iter()
            .map(|e| {
                (
                    e.scenario.as_str(),
                    e.objectives[index].achieved,
                    e.objectives[index].satisfied,
                )
            })
            .collect();
        let achieved: Vec<f64> = per_scenario.iter().map(|(_, a, _)| *a).collect();
        let min = achieved.iter().copied().fold(f64::INFINITY, f64::min);
        let max = achieved.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mean = achieved.iter().sum::<f64>() / achieved.len() as f64;
        let extreme_low = matches!(
            objective,
            crate::optimize::DoseObjective::MinEud { .. }
                | crate::optimize::DoseObjective::MinDoseAtVolume { .. }
        );
        let worst = per_scenario
            .iter()
            .reduce(|a, b| {
                if extreme_low {
                    if a.1 <= b.1 { a } else { b }
                } else if a.1 >= b.1 {
                    a
                } else {
                    b
                }
            })
            .map(|(name, _, _)| (*name).to_owned())
            .unwrap_or_default();
        bands.push(ObjectiveBand {
            kind: objective_kind(objective).into(),
            mask: objective_mask(objective).into(),
            bound,
            nominal_achieved: evaluations[0].objectives[index].achieved,
            min_achieved: min,
            max_achieved: max,
            mean_achieved: mean,
            worst_scenario: worst,
            violated_scenarios: per_scenario
                .iter()
                .filter(|(name, _, satisfied)| *name != "nominal" && !satisfied)
                .map(|(name, _, _)| (*name).to_owned())
                .collect(),
        });
    }
    Ok((evaluations, bands))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{
        BeamWeight, DoseObjective, DoseQuantity, INVERSE_PLAN_OBJECTIVE_SCHEMA,
        INVERSE_PLAN_RESULT_SCHEMA,
    };
    use openbnct_core::GridGeometry;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 1, 1],
            spacing_mm: [1.0; 3],
            origin_mm: [0.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn field() -> BeamDoseField {
        BeamDoseField {
            name: "beam".into(),
            values: vec![3.0, 3.0, 3.0, 3.0],
            components: Some(BTreeMap::from([
                ("boron".into(), vec![1.0, 1.0, 1.0, 1.0]),
                ("hydrogen".into(), vec![2.0, 2.0, 2.0, 2.0]),
            ])),
        }
    }

    fn spec() -> InversePlanObjective {
        InversePlanObjective {
            schema_version: INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
            id: "spec".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::PhysicalTotal,
            objectives: vec![
                DoseObjective::MaxMean {
                    mask: "all".into(),
                    limit: 2.5,
                    weight: 1.0,
                },
                DoseObjective::MinEud {
                    mask: "tumor".into(),
                    target: 2.0,
                    eud_a: 2.0,
                    weight: 1.0,
                },
            ],
            max_iterations: 1,
            gradient_tolerance: 1e-6,
            weight_bound: None,
            weight_regularization: 0.0,
            bio_model: None,
            validity_domain: "test".into(),
            provenance_id: "test".into(),
        }
    }

    fn result() -> InversePlanResult {
        InversePlanResult {
            schema_version: INVERSE_PLAN_RESULT_SCHEMA.into(),
            id: "result".into(),
            case_id: "case".into(),
            objective: ContentReference {
                id: "spec".into(),
                sha256: "ef".repeat(32),
            },
            dose_quantity: DoseQuantity::PhysicalTotal,
            weights: vec![BeamWeight {
                name: "beam".into(),
                weight: 1.0,
            }],
            outcomes: vec![],
            penalty: 0.0,
            iterations: 0,
            converged: true,
            qualification: "test".into(),
            provenance_id: "test".into(),
        }
    }

    fn masks() -> BTreeMap<String, Vec<usize>> {
        BTreeMap::from([
            ("all".into(), vec![0, 1, 2, 3]),
            ("tumor".into(), vec![0, 1]),
        ])
    }

    fn set(scenarios: Vec<PlanScenario>) -> PlanScenarioSet {
        PlanScenarioSet {
            schema_version: PLAN_SCENARIO_SET_SCHEMA.into(),
            id: "set".into(),
            note: None,
            scenarios,
        }
    }

    fn scenario(name: &str) -> PlanScenario {
        PlanScenario {
            name: name.into(),
            note: None,
            dose_scale: None,
            component_scales: BTreeMap::new(),
            region_scales: Vec::new(),
            shift_mm: None,
        }
    }

    #[test]
    fn component_scales_reach_metrics_and_bands() {
        let mut low_boron = scenario("low-boron");
        low_boron.component_scales.insert("boron".into(), 0.0);
        let mut high_dose = scenario("high-dose");
        high_dose.dose_scale = Some(1.5);
        let (evals, bands) = evaluate_scenarios(
            &result(),
            &spec(),
            &[field()],
            &geometry(),
            &set(vec![low_boron, high_dose]),
            &masks(),
        )
        .unwrap();
        assert_eq!(evals.len(), 3);
        // Nominal: mean over "all" = 3.0 > 2.5 → violates.
        assert_eq!(evals[0].scenario, "nominal");
        assert!((evals[0].objectives[0].achieved - 3.0).abs() < 1e-12);
        assert!(!evals[0].objectives[0].satisfied);
        // low-boron: boron zeroed → mean 2.0 ≤ 2.5 → satisfied.
        assert!((evals[1].objectives[0].achieved - 2.0).abs() < 1e-12);
        assert!(evals[1].objectives[0].satisfied);
        // high-dose: 4.5 everywhere → MaxMean violated.
        assert!((evals[2].objectives[0].achieved - 4.5).abs() < 1e-12);
        // Band over all three evals: min 2.0 (low-boron), max 4.5 (high-dose).
        let band = &bands[0];
        assert!((band.min_achieved - 2.0).abs() < 1e-12);
        assert!((band.max_achieved - 4.5).abs() < 1e-12);
        assert_eq!(band.worst_scenario, "high-dose");
        // MaxMean bound 2.5 violated by nominal + high-dose — but the
        // violated list excludes the nominal entry.
        assert_eq!(band.violated_scenarios, vec!["high-dose".to_string()]);
        // min_eud over "tumor" (voxels 0-1, all equal): EUD = mean.
        let eud_band = &bands[1];
        assert!((eud_band.nominal_achieved - 3.0).abs() < 1e-12);
        assert!((eud_band.min_achieved - 2.0).abs() < 1e-12);
        assert_eq!(eud_band.worst_scenario, "low-boron");
    }

    #[test]
    fn region_scales_apply_only_at_masked_voxels() {
        let mut s = scenario("tumor-low-uptake");
        s.region_scales.push(RegionComponentScale {
            mask: "tumor".into(),
            component_scales: BTreeMap::from([("boron".into(), 0.0)]),
        });
        let (evals, _) = evaluate_scenarios(
            &result(),
            &spec(),
            &[field()],
            &geometry(),
            &set(vec![s]),
            &masks(),
        )
        .unwrap();
        // Tumor voxels (0,1): boron 0 + hydrogen 2 → 2.0.
        assert!((evals[1].objectives[1].achieved - 2.0).abs() < 1e-12);
        // Whole mask (0-3): (2+2+3+3)/4 = 2.5 → exactly at bound, satisfied.
        assert!((evals[1].objectives[0].achieved - 2.5).abs() < 1e-12);
        assert!(evals[1].objectives[0].satisfied);
    }

    #[test]
    fn shift_by_one_voxel_is_exact() {
        // Field ramp: values i — a +1mm shift on 1mm spacing moves the
        // field one voxel downstream.
        let ramp = BeamDoseField {
            name: "beam".into(),
            values: vec![0.0, 1.0, 2.0, 3.0],
            components: Some(BTreeMap::from([(
                "hydrogen".into(),
                vec![0.0, 1.0, 2.0, 3.0],
            )])),
        };
        let mut s = scenario("shift-plus-x");
        s.shift_mm = Some([1.0, 0.0, 0.0]);
        let spec = InversePlanObjective {
            objectives: vec![DoseObjective::MaxMean {
                mask: "all".into(),
                limit: 100.0,
                weight: 1.0,
            }],
            ..spec()
        };
        let (evals, _) = evaluate_scenarios(
            &result(),
            &spec,
            &[ramp],
            &geometry(),
            &set(vec![s]),
            &masks(),
        )
        .unwrap();
        // Shifted field: [0,0,1,2] → mean 0.75.
        assert!((evals[1].objectives[0].achieved - 0.75).abs() < 1e-12);
    }

    #[test]
    fn scenario_set_rejects_reserved_names_and_bad_scales() {
        let mut bad = scenario("nominal");
        assert!(set(vec![bad.clone()]).validate().is_err());
        bad = scenario("s");
        bad.component_scales.insert("boron".into(), -0.5);
        assert!(set(vec![bad.clone()]).validate().is_err());
        bad.component_scales.insert("boron".into(), f64::NAN);
        assert!(set(vec![bad.clone()]).validate().is_err());
        bad.component_scales.clear();
        bad.dose_scale = Some(0.5);
        bad.shift_mm = Some([f64::INFINITY, 0.0, 0.0]);
        assert!(set(vec![bad]).validate().is_err());
    }

    #[test]
    fn component_scales_require_component_maps() {
        let mut s = scenario("low-boron");
        s.component_scales.insert("boron".into(), 0.5);
        let bare = BeamDoseField {
            name: "beam".into(),
            values: vec![1.0; 4],
            components: None,
        };
        assert!(
            evaluate_scenarios(
                &result(),
                &spec(),
                &[bare],
                &geometry(),
                &set(vec![s]),
                &masks(),
            )
            .is_err()
        );
    }
}
