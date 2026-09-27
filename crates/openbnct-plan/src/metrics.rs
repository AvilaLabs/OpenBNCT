// SPDX-License-Identifier: MIT

//! Post-hoc plan metrics — DVH-derived summaries (dose-at-volume
//! quantiles, volume-at-dose, generalized EUD) and declared TCP/NTCP
//! endpoint responses — evaluated on the optimized weighted dose sum
//! and attached to [`crate::optimize::InversePlanResult`].
//!
//! Metrics are *reported*, never optimized: they describe the plan the
//! solver produced without feeding back into the weights. The spec is
//! declared on the objective document (`metrics`), so the same content
//! hash covers exactly which metrics were reported.

use openbnct_bio::{
    AppliedDoseStatistic, EndpointModel, EvaluatedEndpoint, score_endpoint_function,
};
use openbnct_core::{RegionMask, equivalent_uniform_dose};
use serde::{Deserialize, Serialize};

use crate::optimize::{DoseObjective, InversePlanObjective, OptimizeError, objective_mask};

/// Which post-hoc metrics a plan reports. Optional on
/// `openbnct.inverse-plan-objective` — absent means the result carries
/// no metrics block (legacy behavior).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanMetricsSpec {
    /// Volume fractions for dose-at-volume quantiles, each in (0, 1] —
    /// `0.95` reports the classic D95. Defaults to `[0.02, 0.50, 0.95]`.
    #[serde(default = "default_dose_at_volume")]
    pub dose_at_volume: Vec<f64>,
    /// EUD exponents `a` to report per objective mask.
    #[serde(default)]
    pub eud_exponents: Vec<f64>,
    /// Declared TCP/NTCP models evaluated per named mask.
    #[serde(default)]
    pub endpoints: Vec<PlanEndpoint>,
}

fn default_dose_at_volume() -> Vec<f64> {
    vec![0.02, 0.5, 0.95]
}

/// One endpoint model scored over a named mask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanEndpoint {
    /// Region mask the dose distribution is taken over.
    pub mask: String,
    /// The response model; embedded so the objective document's own
    /// hash covers the parameter values evaluated.
    pub model: EndpointModel,
    /// Voxel volume in mm³ — required only for `voxel_poisson_tcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voxel_volume_mm3: Option<f64>,
    /// Dose unit the weighted dose field carries — e.g. `"gy"` for
    /// absolute plans or `"gy_per_source_particle"` for unit-weight
    /// fields; `voxel_poisson_tcp` requires a `*_per_source_particle`
    /// unit.
    pub dose_unit: String,
}

/// A dose-at-volume entry: `dose` is the dose the hottest
/// `volume_fraction` of the mask receives (D95 = fraction 0.95).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantileDose {
    pub volume_fraction: f64,
    pub dose: f64,
}

/// A volume-at-dose entry: `volume_fraction` receives ≥ `dose` —
/// reported at every objective bound applied to this mask (V at the
/// declared target or limit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VolumeAtDose {
    pub dose: f64,
    pub volume_fraction: f64,
}

/// A generalized-EUD entry at exponent `a`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EudValue {
    pub a: f64,
    pub value: f64,
}

/// The scored outcome of one declared endpoint model on one mask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEndpointOutcome {
    /// The endpoint model's artifact id.
    pub model_id: String,
    pub endpoint: EvaluatedEndpoint,
    /// Response probability in `[0, 1]`.
    pub probability: f64,
    /// The volume-collapsed statistic the response evaluated; absent
    /// for `voxel_poisson_tcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dose_statistic: Option<AppliedDoseStatistic>,
}

/// DVH-derived summary of the optimized dose over one mask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionPlanMetrics {
    pub mask: String,
    pub voxel_count: usize,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub dose_at_volume: Vec<QuantileDose>,
    /// Volume fraction at each objective bound declared on this mask,
    /// ascending dose.
    pub volume_at_dose: Vec<VolumeAtDose>,
    pub eud: Vec<EudValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<PlanEndpointOutcome>,
}

/// Evaluate the objective document's declared metrics over the
/// optimized plan for every mask in `masks`. `dose_for_mask` returns
/// the optimized weighted dose sum under that mask's own isoeffective
/// `region_weights` override where the quantity is `isoeffective`, so
/// each region is reported under its declared weighting.
pub fn evaluate_plan_metrics(
    spec: &InversePlanObjective,
    mut dose_for_mask: impl FnMut(&str) -> Vec<f64>,
    masks: &[RegionMask],
) -> Result<Vec<RegionPlanMetrics>, OptimizeError> {
    let metrics = spec.metrics.as_ref().expect("caller checks declared");
    for endpoint in &metrics.endpoints {
        endpoint
            .model
            .validate()
            .map_err(|e| OptimizeError::InvalidObjective(format!("metrics endpoint model: {e}")))?;
        if !masks.iter().any(|m| m.name == endpoint.mask) {
            return Err(OptimizeError::UnknownMask(endpoint.mask.clone()));
        }
        if endpoint.dose_unit.trim().is_empty() {
            return Err(OptimizeError::InvalidObjective(
                "metrics endpoint dose_unit is empty".into(),
            ));
        }
    }
    for &f in &metrics.dose_at_volume {
        if !(f > 0.0 && f <= 1.0) {
            return Err(OptimizeError::InvalidObjective(format!(
                "metrics dose_at_volume fraction {f} outside (0, 1]"
            )));
        }
    }
    for &a in &metrics.eud_exponents {
        if !a.is_finite() || a == 0.0 {
            return Err(OptimizeError::InvalidObjective(format!(
                "metrics eud exponent {a} is not finite and non-zero"
            )));
        }
    }

    let mut out = Vec::with_capacity(masks.len());
    for mask in masks {
        let dose = dose_for_mask(&mask.name);
        let mut selected: Vec<f64> = dose
            .iter()
            .zip(&mask.voxels)
            .filter(|(_, on)| **on)
            .map(|(&d, _)| d)
            .collect();
        if selected.is_empty() {
            return Err(OptimizeError::EmptyMask(mask.name.clone()));
        }
        selected.sort_by(f64::total_cmp);
        let n = selected.len();
        let mean = selected.iter().sum::<f64>() / n as f64;
        let dose_at_volume = metrics
            .dose_at_volume
            .iter()
            .map(|&f| QuantileDose {
                volume_fraction: f,
                dose: selected[(((1.0 - f) * n as f64).floor() as usize).min(n - 1)],
            })
            .collect();
        let mut bounds: Vec<f64> = spec
            .objectives
            .iter()
            .filter(|o| objective_mask(o) == mask.name)
            .map(|o| match o {
                DoseObjective::MinEud { target, .. }
                | DoseObjective::MinDoseAtVolume { target, .. }
                | DoseObjective::Maximin { target, .. } => *target,
                DoseObjective::MaxMean { limit, .. }
                | DoseObjective::MaxDoseAtVolume { limit, .. } => *limit,
            })
            .filter(|b| b.is_finite() && *b > 0.0)
            .collect();
        bounds.sort_by(f64::total_cmp);
        bounds.dedup();
        let volume_at_dose = bounds
            .iter()
            .map(|&b| VolumeAtDose {
                dose: b,
                volume_fraction: selected.iter().filter(|&&d| d >= b).count() as f64 / n as f64,
            })
            .collect();
        let eud = metrics
            .eud_exponents
            .iter()
            .map(|&a| {
                equivalent_uniform_dose(&selected, a)
                    .map(|value| EudValue { a, value })
                    .map_err(|e| OptimizeError::InvalidObjective(format!("metrics eud: {e}")))
            })
            .collect::<Result<_, _>>()?;
        let endpoints = metrics
            .endpoints
            .iter()
            .filter(|e| e.mask == mask.name)
            .map(|e| {
                let voxel_volume = e.voxel_volume_mm3.unwrap_or(1.0);
                if !(voxel_volume.is_finite() && voxel_volume > 0.0) {
                    return Err(OptimizeError::InvalidObjective(format!(
                        "metrics endpoint voxel_volume_mm3 {voxel_volume} is not positive"
                    )));
                }
                score_endpoint_function(&e.model, &selected, voxel_volume, &e.dose_unit)
                    .map(|(probability, dose_statistic)| PlanEndpointOutcome {
                        model_id: e.model.id.clone(),
                        endpoint: match e.model.endpoint {
                            openbnct_bio::EndpointKind::Tcp => EvaluatedEndpoint::Tcp,
                            openbnct_bio::EndpointKind::Ntcp => EvaluatedEndpoint::Ntcp,
                        },
                        probability,
                        dose_statistic,
                    })
                    .map_err(|e| OptimizeError::InvalidObjective(format!("metrics endpoint: {e}")))
            })
            .collect::<Result<_, _>>()?;
        out.push(RegionPlanMetrics {
            mask: mask.name.clone(),
            voxel_count: n,
            mean,
            min: selected[0],
            max: selected[n - 1],
            dose_at_volume,
            volume_at_dose,
            eud,
            endpoints,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{DoseQuantity, INVERSE_PLAN_OBJECTIVE_SCHEMA};
    use openbnct_bio::{DoseStatistic, EndpointFunction, EndpointKind};

    fn spec(metrics: Option<PlanMetricsSpec>) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
            id: "obj".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::PhysicalTotal,
            objectives: vec![
                DoseObjective::MinEud {
                    mask: "T".into(),
                    target: 10.0,
                    eud_a: 5.0,
                    weight: 1.0,
                },
                DoseObjective::MaxMean {
                    mask: "O".into(),
                    limit: 2.0,
                    weight: 1.0,
                },
            ],
            max_iterations: 500,
            gradient_tolerance: 1e-10,
            weight_bound: None,
            weight_regularization: 0.0,
            bio_model: None,
            metrics,
            validity_domain: "unit test".into(),
            provenance_id: "test".into(),
        }
    }

    fn masks() -> Vec<RegionMask> {
        vec![
            RegionMask {
                name: "T".into(),
                voxels: vec![true, true, false, false],
            },
            RegionMask {
                name: "O".into(),
                voxels: vec![false, false, true, true],
            },
        ]
    }

    #[test]
    fn metrics_report_quantiles_bounds_and_eud() {
        // w·A·D_A: tumor doses 10 and 5, organ 1 and 3.
        let spec = spec(Some(PlanMetricsSpec {
            dose_at_volume: vec![0.95, 0.5],
            eud_exponents: vec![1.0],
            endpoints: vec![],
        }));
        let dose = vec![10.0, 5.0, 1.0, 3.0];
        let out = evaluate_plan_metrics(&spec, |_| dose.clone(), &masks()).unwrap();
        let t = &out[0];
        assert_eq!(t.voxel_count, 2);
        assert!((t.mean - 7.5).abs() < 1e-12);
        // D95 of two voxels = the lower one; D50 = the lower one too
        // (idx floor((1-f)·n) = 1→voxel 0? check: (1-0.95)*2=0.1→0→
        // sorted[0]=5; (1-0.5)*2=1→sorted[1]=10)
        assert!((t.dose_at_volume[0].dose - 5.0).abs() < 1e-12);
        assert!((t.dose_at_volume[1].dose - 10.0).abs() < 1e-12);
        // V(10) over T: only the 10-dose voxel → 0.5
        assert_eq!(t.volume_at_dose.len(), 1);
        assert!((t.volume_at_dose[0].volume_fraction - 0.5).abs() < 1e-12);
        // EUD a=1 = arithmetic mean.
        assert!((t.eud[0].value - 7.5).abs() < 1e-12);
        // Organ bound collected from MaxMean limit: doses 1 and 3 vs
        // limit 2 → V(2) = half the organ volume.
        assert_eq!(out[1].volume_at_dose[0].dose, 2.0);
        assert!((out[1].volume_at_dose[0].volume_fraction - 0.5).abs() < 1e-12);
    }

    #[test]
    fn metrics_endpoint_scores_logistic() {
        let spec = spec(Some(PlanMetricsSpec {
            dose_at_volume: vec![],
            eud_exponents: vec![],
            endpoints: vec![PlanEndpoint {
                mask: "O".into(),
                model: EndpointModel {
                    schema_version: openbnct_bio::ENDPOINT_MODEL_SCHEMA.into(),
                    id: "ntcp-o".into(),
                    endpoint: EndpointKind::Ntcp,
                    function: EndpointFunction::Logistic {
                        d50: 2.0,
                        gamma50: 2.0,
                    },
                    dose_statistic: Some(DoseStatistic::Mean),
                    validity_domain: None,
                    derivation: None,
                },
                voxel_volume_mm3: None,
                dose_unit: "gy".into(),
            }],
        }));
        // mean over O = 2 = d50 → logistic probability 0.5.
        let dose = vec![0.0, 0.0, 1.0, 3.0];
        let out = evaluate_plan_metrics(&spec, |_| dose.clone(), &masks()).unwrap();
        let ep = &out[1].endpoints[0];
        assert_eq!(ep.endpoint, EvaluatedEndpoint::Ntcp);
        assert!((ep.probability - 0.5).abs() < 1e-12);
        assert!(ep.dose_statistic.is_some());
    }
}
