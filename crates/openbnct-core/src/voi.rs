// SPDX-License-Identifier: MIT

//! Expected value of information (`openbnct.voi-evaluation/0.1.0`):
//! rank candidate *proposed* measurements by the exact posterior
//! variance reduction they would deliver on declared linear metrics of
//! a `joint/0.1.0` uncertainty state.
//!
//! Model: the joint input's elementary draws form a Gaussian state
//! with covariance `C` (non-Gaussian marginals are represented by
//! their first two moments — recorded in `assumptions`). A candidate
//! observation is `y = hᵀx + b + n` where `h` is a declared linear
//! combination of sources, `b` is an optional *shared calibration
//! bias* (itself a joint source, so replicates cannot reduce below
//! its floor), and `n ~ N(0, r)` is independent measurement noise,
//! `repeats` times iid. For this conjugate family the posterior
//! covariance does not depend on the observed value, so the *expected*
//! variance reduction is exact — no sampling, no substituting "true"
//! values for uncertain inputs.
//!
//! This command compares research measurement designs. It does not
//! schedule care, change a plan, or control equipment; cost is
//! reported separately unless the spec explicitly declares a
//! cost-adjusted objective.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::deserialize_contract_id;
use crate::joint::{
    CoordinateKey, JointError, JointUncertaintyInput, SourceSensitivity, elementary_covariance,
    sensitivity_coordinate,
};

/// Spec contract token.
pub const VOI_EVALUATION_SCHEMA: &str = "openbnct.voi-evaluation/0.1.0";
/// Report contract token.
pub const VOI_REPORT_SCHEMA: &str = "openbnct.voi-report/0.1.0";

/// How to combine expected utility with declared cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostObjective {
    /// `expected_variance_reduction / cost`; requires every compared
    /// candidate to declare a positive cost.
    ReductionPerUnitCost,
}

/// One proposed measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementProposal {
    pub id: String,
    /// Free-form kind label (`blood_assay`, `output_calibration`, …)
    /// carried into the report.
    pub kind: String,
    /// Sources the observation responds to: `y = Σ gᵢ·xᵢ + b + n`.
    /// Targets on per-target sources pick one elementary draw.
    pub sensitivity: Vec<SourceSensitivity>,
    /// Independent measurement-noise σ. `None` is honest "precision
    /// unknown" and produces an *unavailable* result rather than an
    /// arbitrary default.
    pub measurement_sd: Option<f64>,
    /// Optional shared calibration bias: id of a `Shared` joint source
    /// that also enters `y`. Replicates reduce `n` but can never
    /// resolve `b` — the bias floor is retained exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_bias_source: Option<String>,
    /// Independent replicates of the same observation model (shared
    /// bias applies to all).
    #[serde(default = "one")]
    pub repeats: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_unit: Option<String>,
}

fn one() -> u32 {
    1
}

/// A declared linear metric of the joint sources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiMetric {
    pub id: String,
    pub sensitivity: Vec<SourceSensitivity>,
}

/// The evaluation spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiEvaluationSpec {
    #[serde(deserialize_with = "deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    /// The joint uncertainty state the metrics and observations act on.
    pub joint: JointUncertaintyInput,
    pub metrics: Vec<VoiMetric>,
    pub candidates: Vec<MeasurementProposal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_objective: Option<CostObjective>,
}

/// Per (candidate, metric) evaluation outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateUtility {
    pub proposal: String,
    pub metric: String,
    /// `false` when the observation model was insufficiently declared
    /// (e.g. unknown precision) — never silently defaulted.
    pub evaluated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub prior_variance: f64,
    pub expected_posterior_variance: f64,
    pub expected_variance_reduction: f64,
    pub repeats: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_unit: Option<String>,
    /// Only present when the spec declared a cost-adjusted objective.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_adjusted_utility: Option<f64>,
}

/// The evaluation report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiReport {
    #[serde(deserialize_with = "deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    /// Candidates sorted by expected variance reduction, per metric.
    pub rankings: Vec<CandidateUtility>,
    pub assumptions: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> JointError {
    crate::joint::invalid(msg)
}

/// Build the observation row `h` over elementary coordinates.
fn observation_row(
    input: &JointUncertaintyInput,
    keys: &[CoordinateKey],
    sensitivity: &[SourceSensitivity],
    bias_source: Option<&str>,
) -> Result<Vec<f64>, JointError> {
    let key_index: BTreeMap<(usize, Option<String>), usize> = keys
        .iter()
        .cloned()
        .enumerate()
        .map(|(c, k)| (k, c))
        .collect();
    let mut h = vec![0.0; keys.len()];
    for sens in sensitivity {
        let key = sensitivity_coordinate(input, sens)?;
        h[key_index[&key]] += sens.coefficient;
    }
    if let Some(bias_id) = bias_source {
        let index = input
            .sources
            .iter()
            .position(|s| s.id == bias_id)
            .ok_or_else(|| {
                invalid(format!(
                    "shared bias source {bias_id:?} is not a joint source"
                ))
            })?;
        let coord = *key_index.get(&(index, None)).ok_or_else(|| {
            invalid(format!(
                "shared bias source {bias_id:?} must be a Shared source"
            ))
        })?;
        h[coord] += 1.0;
    }
    Ok(h)
}

/// Rank-1 Gaussian conditioning `C ← C − Ch hᵀC/(hᵀCh + r)` applied
/// `repeats` times (iid replicates of the same observation model).
fn condition_covariance(cov: &mut [f64], h: &[f64], r: f64, repeats: u32) {
    let n = h.len();
    for _ in 0..repeats {
        // Ch
        let mut ch = vec![0.0; n];
        for i in 0..n {
            for j in 0..n {
                ch[i] += cov[i * n + j] * h[j];
            }
        }
        let hch: f64 = h.iter().zip(ch.iter()).map(|(a, b)| a * b).sum();
        let denom = hch + r;
        if denom <= 0.0 {
            continue;
        }
        for i in 0..n {
            for j in 0..n {
                cov[i * n + j] -= ch[i] * ch[j] / denom;
            }
        }
    }
}

/// `gᵀCg`.
fn quadratic_form(cov: &[f64], g: &[f64]) -> f64 {
    let n = g.len();
    let mut total = 0.0;
    for i in 0..n {
        for j in 0..n {
            total += g[i] * cov[i * n + j] * g[j];
        }
    }
    total
}

/// Evaluate the spec into a ranked report.
pub fn evaluate_voi(spec: &VoiEvaluationSpec) -> Result<VoiReport, JointError> {
    spec.joint.validate()?;
    if spec.metrics.is_empty() {
        return Err(invalid("at least one metric is required"));
    }
    if spec.candidates.is_empty() {
        return Err(invalid("at least one candidate measurement is required"));
    }
    let mut candidate_ids = std::collections::BTreeSet::new();
    for c in &spec.candidates {
        if !candidate_ids.insert(c.id.as_str()) {
            return Err(invalid(format!("duplicate candidate id {:?}", c.id)));
        }
        if c.repeats == 0 {
            return Err(invalid(format!(
                "candidate {:?}: repeats must be ≥ 1",
                c.id
            )));
        }
        if let Some(sd) = c.measurement_sd
            && (!sd.is_finite() || sd < 0.0)
        {
            return Err(invalid(format!(
                "candidate {:?}: measurement_sd must be finite and ≥ 0",
                c.id
            )));
        }
        if spec.cost_objective == Some(CostObjective::ReductionPerUnitCost)
            && c.cost.is_none_or(|c| !c.is_finite() || c <= 0.0)
        {
            return Err(invalid(format!(
                "candidate {:?}: cost-adjusted objective requires a positive finite cost",
                c.id
            )));
        }
    }
    let (keys, prior_cov) = elementary_covariance(&spec.joint)?;

    let mut rankings = Vec::new();
    for candidate in &spec.candidates {
        let h = observation_row(
            &spec.joint,
            &keys,
            &candidate.sensitivity,
            candidate.shared_bias_source.as_deref(),
        )?;
        for metric in &spec.metrics {
            let g = observation_row(&spec.joint, &keys, &metric.sensitivity, None)?;
            let prior = quadratic_form(&prior_cov, &g);
            match candidate.measurement_sd {
                None => rankings.push(CandidateUtility {
                    proposal: candidate.id.clone(),
                    metric: metric.id.clone(),
                    evaluated: false,
                    reason: Some("measurement precision undeclared — no default assumed".into()),
                    prior_variance: prior,
                    expected_posterior_variance: prior,
                    expected_variance_reduction: 0.0,
                    repeats: candidate.repeats,
                    cost: candidate.cost,
                    cost_unit: candidate.cost_unit.clone(),
                    cost_adjusted_utility: None,
                }),
                Some(sd) => {
                    let mut post = prior_cov.clone();
                    condition_covariance(&mut post, &h, sd * sd, candidate.repeats);
                    let posterior = quadratic_form(&post, &g);
                    let reduction = (prior - posterior).max(0.0);
                    let cost_adjusted = spec.cost_objective.map(|obj| match obj {
                        CostObjective::ReductionPerUnitCost => {
                            reduction / candidate.cost.expect("validated positive")
                        }
                    });
                    rankings.push(CandidateUtility {
                        proposal: candidate.id.clone(),
                        metric: metric.id.clone(),
                        evaluated: true,
                        reason: None,
                        prior_variance: prior,
                        expected_posterior_variance: posterior,
                        expected_variance_reduction: reduction,
                        repeats: candidate.repeats,
                        cost: candidate.cost,
                        cost_unit: candidate.cost_unit.clone(),
                        cost_adjusted_utility: cost_adjusted,
                    });
                }
            }
        }
    }
    // Rank each metric's candidates independently, best first; keep
    // unevaluated candidates at the tail.
    rankings.sort_by(|a, b| {
        a.metric
            .cmp(&b.metric)
            .then_with(|| {
                b.expected_variance_reduction
                    .total_cmp(&a.expected_variance_reduction)
            })
            .then_with(|| a.proposal.cmp(&b.proposal))
    });
    let mut assumptions = vec![
        "linear-Gaussian model: elementary draws are treated as jointly Gaussian with the declared covariance; non-Gaussian marginals are represented by their first two moments".into(),
        "for a linear-Gaussian observation the posterior covariance is outcome-independent, so expected variance reduction is exact — no ensemble was sampled".into(),
        "replicates are assumed independent measurement noise sharing the declared calibration bias; exact duplicates of one realized observation are not independently informative".into(),
        "candidate measurement designs are compared on the declared model; this does not schedule care, alter a plan, or control equipment".into(),
    ];
    if spec.cost_objective.is_none() {
        assumptions.push(
            "no cost-adjusted objective was declared — cost is reported alongside, never folded into the utility".into(),
        );
    }
    if spec.joint.sources.iter().any(|s| {
        !matches!(
            s.distribution,
            crate::joint::Distribution::Normal { .. } | crate::joint::Distribution::Point { .. }
        )
    }) {
        assumptions.push(
            "non-Gaussian marginals present: results are a two-moment Gaussian approximation, not an exact posterior".into(),
        );
    }
    Ok(VoiReport {
        schema_version: VOI_REPORT_SCHEMA.into(),
        id: spec.id.clone(),
        qualification: spec.qualification.clone(),
        provenance_id: spec.provenance_id.clone(),
        rankings,
        assumptions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::joint::*;

    fn source(id: &str, sd: f64) -> JointSource {
        JointSource {
            id: id.into(),
            category: UncertaintyCategory::InputParameter,
            scope: vec![SourceTarget::Global],
            sharing: SourceSharing::Shared,
            unit: "1".into(),
            support: Support::Real,
            distribution: Distribution::Normal {
                mean: 0.0,
                std_dev: sd,
            },
            evidence: SourceEvidence {
                basis: "test".into(),
                reference: None,
            },
        }
    }

    fn joint(sources: Vec<JointSource>) -> JointUncertaintyInput {
        JointUncertaintyInput {
            schema_version: JOINT_UNCERTAINTY_INPUT_SCHEMA.into(),
            id: "voi.test".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "test".into(),
            subjects: vec![crate::ContentReference {
                id: "dose".into(),
                sha256: "f".repeat(64),
            }],
            sources,
            correlations: vec![],
            category_disposition: [
                UncertaintyCategory::StatisticalSampling,
                UncertaintyCategory::StructuralModel,
                UncertaintyCategory::ModelDiscrepancy,
            ]
            .into_iter()
            .map(|category| CategoryDisposition {
                category,
                status: CategoryStatus::Unassessed,
                note: "fixture".into(),
            })
            .collect(),
        }
    }

    fn sens(source: usize) -> SourceSensitivity {
        SourceSensitivity {
            source,
            coefficient: 1.0,
            target: None,
        }
    }

    fn proposal(id: &str, source: usize, sd: f64) -> MeasurementProposal {
        MeasurementProposal {
            id: id.into(),
            kind: "assay".into(),
            sensitivity: vec![sens(source)],
            measurement_sd: Some(sd),
            shared_bias_source: None,
            repeats: 1,
            cost: None,
            cost_unit: None,
        }
    }

    fn spec(
        joint: JointUncertaintyInput,
        metric_source: usize,
        candidates: Vec<MeasurementProposal>,
    ) -> VoiEvaluationSpec {
        VoiEvaluationSpec {
            schema_version: VOI_EVALUATION_SCHEMA.into(),
            id: "v1".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "test".into(),
            joint,
            metrics: vec![VoiMetric {
                id: "metric".into(),
                sensitivity: vec![sens(metric_source)],
            }],
            candidates,
            cost_objective: None,
        }
    }

    #[test]
    fn scalar_posterior_matches_conjugate_formula() {
        // Prior σ=2 → v0=4; measurement σ=1 → r=1.
        // Posterior variance = (1/4 + 1/1)^-1 = 0.8.
        let s = spec(
            joint(vec![source("x", 2.0)]),
            0,
            vec![proposal("m", 0, 1.0)],
        );
        let report = evaluate_voi(&s).unwrap();
        let u = &report.rankings[0];
        assert!((u.expected_posterior_variance - 0.8).abs() < 1e-12);
        assert!((u.expected_variance_reduction - 3.2).abs() < 1e-12);
    }

    #[test]
    fn orthogonal_observation_gives_zero_gain() {
        // Metric on source 0; observation of unrelated source 1.
        let s = spec(
            joint(vec![source("x", 1.0), source("unrelated", 1.0)]),
            0,
            vec![proposal("m", 1, 0.5)],
        );
        let report = evaluate_voi(&s).unwrap();
        assert_eq!(report.rankings[0].expected_variance_reduction, 0.0);
    }

    #[test]
    fn noisier_measurement_ranks_lower_and_zero_noise_is_exact() {
        let s = spec(
            joint(vec![source("x", 1.0)]),
            0,
            vec![
                proposal("precise", 0, 0.1),
                proposal("noisy", 0, 100.0),
                proposal("perfect", 0, 0.0),
            ],
        );
        let report = evaluate_voi(&s).unwrap();
        let red = |id: &str| {
            report
                .rankings
                .iter()
                .find(|u| u.proposal == id)
                .unwrap()
                .expected_variance_reduction
        };
        assert!(red("precise") > red("noisy"));
        assert!(red("noisy") < 1e-2);
        // Perfect observation eliminates the metric's variance.
        assert!((red("perfect") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn shared_bias_limits_replicate_gain() {
        // x ~ N(0,1) drives the metric; bias b ~ N(0, 0.3²) enters the
        // observation but NOT the metric. Replicated assays reduce noise
        // toward the bias-imposed floor on x's posterior.
        let j = joint(vec![source("x", 1.0), source("bias", 0.3)]);
        // metric observes x only.
        let mut m = proposal("assay", 0, 0.1);
        m.shared_bias_source = Some("bias".into());
        m.repeats = 100;
        let mut m1 = m.clone();
        m1.repeats = 1;
        m1.id = "single".into();
        let s2 = spec(j, 0, vec![m1, m]);
        let report = evaluate_voi(&s2).unwrap();
        let single = report
            .rankings
            .iter()
            .find(|u| u.proposal == "single")
            .unwrap();
        let hundred = report
            .rankings
            .iter()
            .find(|u| u.proposal == "assay")
            .unwrap();
        assert!(hundred.expected_variance_reduction > single.expected_variance_reduction);
        // Posterior on x saturates at the information the bias allows:
        // Var_post(x) = v0·(v_b + r/n)/(v0 + v_b + r/n) → with r/n→0:
        // v0·v_b/(v0+v_b) = 1·0.09/1.09 = 0.0826.
        assert!((hundred.expected_posterior_variance - 0.09 / 1.09).abs() < 5e-3);
    }

    #[test]
    fn unknown_precision_is_unavailable_not_defaulted() {
        let mut p = proposal("m", 0, 1.0);
        p.measurement_sd = None;
        let s = spec(joint(vec![source("x", 1.0)]), 0, vec![p]);
        let report = evaluate_voi(&s).unwrap();
        assert!(!report.rankings[0].evaluated);
        assert_eq!(report.rankings[0].expected_variance_reduction, 0.0);
    }

    #[test]
    fn duplicate_candidate_ids_rejected() {
        let s = spec(
            joint(vec![source("x", 1.0)]),
            0,
            vec![proposal("same", 0, 1.0), proposal("same", 0, 1.0)],
        );
        assert!(evaluate_voi(&s).is_err());
    }
}
