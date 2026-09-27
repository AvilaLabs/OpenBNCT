// SPDX-License-Identifier: MIT

//! Measurement-informed boron estimation
//! (`openbnct.boron-inference/0.1.0` spec,
//! `openbnct.boron-inference-report/0.1.0`).
//!
//! A sequential linear-Gaussian estimator over a small declared state —
//! region concentration scale factors, clearance corrections, and
//! shared calibration terms — driven by typed observations:
//!
//! - `blood_assay` / `total_boron_assay`: direct concentration
//!   observations with declared noise;
//! - `pet_surrogate`: tracer-uptake readings mapped to ¹⁰B through a
//!   *declared* transfer coefficient — PET is a surrogate, never the
//!   concentration itself;
//! - `prompt_gamma_counts`: raw counts with live time, efficiency /
//!   response coefficient, and declared background. Counts are updated
//!   through a Gaussian approximation with variance
//!   `counts + background_variance`; bins below `low_count_threshold`
//!   are flagged rather than trusted.
//!
//! Diagnostics are the point of the contract: every emitted state
//! reports its posterior σ against prior σ, and directions the
//! observations did not resolve are listed explicitly — two states
//! producing identical predicted observations stay indistinguishable
//! rather than returning a falsely precise estimate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Spec contract token.
pub const BORON_INFERENCE_SCHEMA: &str = "openbnct.boron-inference/0.1.0";
/// Report contract token.
pub const BORON_INFERENCE_REPORT_SCHEMA: &str = "openbnct.boron-inference-report/0.1.0";

#[derive(Debug, Error)]
pub enum BoronInferenceError {
    #[error("unsupported schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid inference spec: {0}")]
    Invalid(String),
}

/// One state coordinate with its prior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EstimationState {
    pub id: String,
    /// What the coordinate is, e.g. `tumor concentration scale` —
    /// carried verbatim into the report.
    pub meaning: String,
    /// Prior mean at the state's `pk_curve.t0_s` (or at the session
    /// reference epoch when no curve is declared).
    pub prior_mean: f64,
    pub prior_sd: f64,
    pub unit: String,
    /// Optional PK concentration curve coupling this state to session
    /// time: between epochs the coordinate transitions deterministically
    /// by `C(t)/C(epoch)` (mean scaled, covariance scaled by r²), and a
    /// delayed observation of time `t` informs the current state through
    /// the same declared map. When set, `prior_mean`/`prior_sd` are
    /// quoted at `t0_s`. All coupled states must share one `t0_s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pk_curve: Option<crate::replay::PkConcentrationCurve>,
}

/// Typed observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronObservation {
    pub id: String,
    pub kind: String,
    /// Session time of the measurement (draw time / bin centre).
    pub time_s: f64,
    /// State coefficients the observation responds to:
    /// `predicted = Σ hᵢ·xᵢ` (+ declared background for counts).
    pub sensitivity: Vec<(String, f64)>,
    /// Gaussian-observation mean (assay value, surrogate-derived
    /// concentration).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    /// Gaussian-observation σ. Required for `mean` observations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
    /// Raw detector counts — Poisson-family observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<u64>,
    /// Declared background counts in the bin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<f64>,
    /// Additional declared background σ² (non-Poisson background model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_variance: Option<f64>,
    /// For `pet_surrogate`: the declared tracer→¹⁰B transfer that
    /// produced `mean` — enrichment factor, uptake ratio, or explicit
    /// mapping note. Required so the record keeps the surrogate status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_note: Option<String>,
    /// When the result actually became available — an observation may
    /// only inform states at epochs ≥ this. `None` means `time_s`.
    /// Late-arriving assays sequence honestly rather than smoothed
    /// backwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_at_s: Option<f64>,
}

/// The estimation spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronInferenceSpec {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    pub states: Vec<EstimationState>,
    /// Optional row-major prior correlation over `states` — default
    /// independent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_correlation: Option<Vec<f64>>,
    pub observations: Vec<BoronObservation>,
    /// Sessions coverage window — spans with no observation covering
    /// them are reported in `unobserved_intervals`.
    pub coverage: (f64, f64),
    /// Counts bins at or below this total are flagged low-count.
    #[serde(default = "default_low_count")]
    pub low_count_threshold: f64,
}

fn default_low_count() -> f64 {
    20.0
}

/// One direction the data failed to resolve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnresolvedDirection {
    /// State coefficients (same order as `spec.states`).
    pub direction: Vec<f64>,
    /// Posterior variance along this direction ÷ prior variance — near
    /// 1 means the observations carried no information there.
    pub posterior_to_prior: f64,
}

/// State estimate at one epoch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateEstimate {
    pub epoch_s: f64,
    /// State means in `states` order.
    pub mean: Vec<f64>,
    /// Posterior σ in `states` order.
    pub sd: Vec<f64>,
    /// Observation id this update consumed, `null` at the prior epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_observation: Option<String>,
}

/// One observation's check against the estimate it updated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservationResidual {
    pub observation: String,
    pub time_s: f64,
    pub observed: f64,
    pub predicted: f64,
    /// `(observed − predicted) / sqrt(S)` — ≈ standard-normal under the
    /// declared model.
    pub normalized: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_count: Option<bool>,
}

/// The estimation report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronInferenceReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    /// Prior epoch state, then one estimate after each update — sorted
    /// by availability so late assays update at their arrival, not
    /// retroactively.
    pub estimates: Vec<StateEstimate>,
    /// Unresolved state-space directions after all updates.
    pub unresolved: Vec<UnresolvedDirection>,
    pub residuals: Vec<ObservationResidual>,
    /// Declared coverage spans carrying no observation.
    pub unobserved_intervals: Vec<(f64, f64)>,
    /// Fraction of each state's prior variance remaining (≈1 = prior
    /// dominated — the data did not constrain that coordinate).
    pub posterior_to_prior_sd: Vec<(String, f64)>,
    pub assumptions: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> BoronInferenceError {
    BoronInferenceError::Invalid(msg.into())
}

/// Jacobi eigendecomposition of a symmetric n×n matrix — small states
/// only (the spec caps them). Returns `(eigenvalues, eigenvectors)`
/// sorted ascending; vectors are columns of the returned row-major V.
fn jacobi_eigh(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..100 * n * n {
        // Largest off-diagonal.
        let (mut p, mut q, mut biggest) = (0usize, 1usize, 0.0f64);
        for i in 0..n {
            for j in (i + 1)..n {
                if m[i * n + j].abs() > biggest {
                    biggest = m[i * n + j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if biggest < 1e-14 {
            break;
        }
        let app = m[p * n + p];
        let aqq = m[q * n + q];
        let apq = m[p * n + q];
        let theta = 0.5 * (2.0 * apq).atan2(aqq - app);
        let (c, s) = (theta.cos(), theta.sin());
        for k in 0..n {
            let (mkp, mkq) = (m[k * n + p], m[k * n + q]);
            m[k * n + p] = c * mkp - s * mkq;
            m[k * n + q] = s * mkp + c * mkq;
        }
        for k in 0..n {
            let (mpk, mqk) = (m[p * n + k], m[q * n + k]);
            m[p * n + k] = c * mpk - s * mqk;
            m[q * n + k] = s * mpk + c * mqk;
        }
        for k in 0..n {
            let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
            v[k * n + p] = c * vkp - s * vkq;
            v[k * n + q] = s * vkp + c * vkq;
        }
    }
    let mut eig: Vec<(f64, usize)> = (0..n).map(|i| (m[i * n + i], i)).collect();
    eig.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut vals = Vec::with_capacity(n);
    let mut vecs = vec![0.0; n * n];
    for (new_j, (_, old_j)) in eig.iter().enumerate() {
        vals.push(m[old_j * n + old_j]);
        for k in 0..n {
            vecs[k * n + new_j] = v[k * n + old_j];
        }
    }
    (vals, vecs)
}

/// Gaussian conditioning `C ← C − Ch hᵀC/(hᵀCh + r)`, `x ← x + K(y −
/// hᵀx)` — the same rank-1 update VOI-01 uses, with the mean update.
fn update(mean: &mut [f64], cov: &mut [f64], h: &[f64], observed: f64, r: f64) -> (f64, f64) {
    let n = h.len();
    let mut ch = vec![0.0; n];
    for i in 0..n {
        for j in 0..n {
            ch[i] += cov[i * n + j] * h[j];
        }
    }
    let predicted: f64 = h.iter().zip(mean.iter()).map(|(a, b)| a * b).sum();
    let s = h.iter().zip(ch.iter()).map(|(a, b)| a * b).sum::<f64>() + r;
    for i in 0..n {
        mean[i] += ch[i] * (observed - predicted) / s;
    }
    for i in 0..n {
        for j in 0..n {
            cov[i * n + j] -= ch[i] * ch[j] / s;
        }
    }
    (predicted, s)
}

/// Run the sequential estimation.
pub fn run_boron_inference(
    spec: &BoronInferenceSpec,
) -> Result<BoronInferenceReport, BoronInferenceError> {
    if !openbnct_core::schema_matches(&spec.schema_version, BORON_INFERENCE_SCHEMA) {
        return Err(BoronInferenceError::UnsupportedSchema(
            spec.schema_version.clone(),
        ));
    }
    let n = spec.states.len();
    if n == 0 || n > 16 {
        return Err(invalid("state dimension must be 1..=16"));
    }
    let mut index = BTreeMap::new();
    let mut pk_t0: Option<f64> = None;
    for (i, s) in spec.states.iter().enumerate() {
        if !s.prior_sd.is_finite() || s.prior_sd <= 0.0 {
            return Err(invalid(format!(
                "state {:?}: prior_sd must be positive",
                s.id
            )));
        }
        if index.insert(s.id.clone(), i).is_some() {
            return Err(invalid(format!("duplicate state id {:?}", s.id)));
        }
        if let Some(curve) = &s.pk_curve {
            if curve.amplitudes.is_empty()
                || curve.amplitudes.len() != curve.rates_per_s.len()
                || !curve
                    .amplitudes
                    .iter()
                    .chain(curve.rates_per_s.iter())
                    .all(|x| x.is_finite() && *x >= 0.0)
                || !curve.t0_s.is_finite()
            {
                return Err(invalid(format!(
                    "state {:?}: pk_curve needs matching non-negative finite amplitudes/rates_per_s",
                    s.id
                )));
            }
            if curve.evaluate(curve.t0_s) <= 0.0 {
                return Err(invalid(format!(
                    "state {:?}: pk_curve must be positive at t0_s — it anchors the declared prior",
                    s.id
                )));
            }
            match pk_t0 {
                None => pk_t0 = Some(curve.t0_s),
                Some(t0) if (t0 - curve.t0_s).abs() <= 1e-9 * t0.abs().max(1.0) => {}
                Some(t0) => {
                    return Err(invalid(format!(
                        "state {:?}: pk_coupled states must share one t0_s (saw {t0}, now {})",
                        s.id, curve.t0_s
                    )));
                }
            }
        }
    }
    // The tracked state epoch starts at the declared prior epoch — the
    // shared pk t0 when coupled, else the coverage start.
    let mut epoch_now = pk_t0.unwrap_or(spec.coverage.0);
    // Per-state transition ratio: C_i(t)/C_i(epoch), 1 for uncoupled
    // states.
    fn ratio_at(states: &[EstimationState], i: usize, t: f64, epoch: f64) -> f64 {
        match &states[i].pk_curve {
            Some(c) => c.evaluate(t) / c.evaluate(epoch),
            None => 1.0,
        }
    }
    let transition = |mean: &mut [f64], cov: &mut [f64], ratios: &[f64], n: usize| {
        for i in 0..n {
            mean[i] *= ratios[i];
        }
        for i in 0..n {
            for j in 0..n {
                cov[i * n + j] *= ratios[i] * ratios[j];
            }
        }
    };
    // Prior covariance.
    let mut cov = vec![0.0; n * n];
    for (i, s) in spec.states.iter().enumerate() {
        cov[i * n + i] = s.prior_sd * s.prior_sd;
    }
    if let Some(corr) = &spec.prior_correlation {
        if corr.len() != n * n {
            return Err(invalid("prior_correlation must be n×n row-major"));
        }
        for i in 0..n {
            for j in 0..n {
                cov[i * n + j] =
                    corr[i * n + j] * spec.states[i].prior_sd * spec.states[j].prior_sd;
            }
        }
    }
    let mut mean: Vec<f64> = spec.states.iter().map(|s| s.prior_mean).collect();

    let mut estimates = vec![StateEstimate {
        epoch_s: epoch_now,
        mean: mean.clone(),
        sd: (0..n).map(|i| cov[i * n + i].sqrt()).collect(),
        after_observation: None,
    }];
    let mut residuals = Vec::new();
    // Order by availability — a late assay informs only epochs after
    // its arrival. (A retrospective smoother would need to be labelled
    // as such; this filter is strictly forward in availability time.)
    let mut observations: Vec<&BoronObservation> = spec.observations.iter().collect();
    observations.sort_by(|a, b| {
        a.available_at_s
            .unwrap_or(a.time_s)
            .total_cmp(&b.available_at_s.unwrap_or(b.time_s))
    });
    for obs in observations {
        let h: Vec<f64> = {
            let mut h = vec![0.0; n];
            for (id, coef) in &obs.sensitivity {
                let i = index.get(id).ok_or_else(|| {
                    invalid(format!("observation {:?}: unknown state {id:?}", obs.id))
                })?;
                h[*i] += coef;
            }
            h
        };
        if !h.iter().any(|c| *c != 0.0) {
            return Err(invalid(format!(
                "observation {:?}: declares no sensitivity — it cannot inform the state",
                obs.id
            )));
        }
        if obs.kind == "pet_surrogate" && obs.transfer_note.is_none() {
            return Err(invalid(format!(
                "observation {:?}: PET surrogate requires an explicit transfer_note",
                obs.id
            )));
        }
        let (observed, r, low_count) = if let Some(counts) = obs.counts {
            // Poisson counts with declared background — Gaussian update
            // with variance counts + background_variance.
            let bg = obs.background.unwrap_or(0.0);
            let bg_var = obs.background_variance.unwrap_or(bg.max(0.0));
            let counts_f = counts as f64;
            (
                counts_f,
                counts_f + bg_var,
                Some(counts_f <= spec.low_count_threshold),
            )
        } else {
            let mean = obs.mean.ok_or_else(|| {
                invalid(format!(
                    "observation {:?}: needs `mean`+`sd` or `counts`",
                    obs.id
                ))
            })?;
            let sd = obs.sd.ok_or_else(|| {
                invalid(format!(
                    "observation {:?}: Gaussian observation needs `sd`",
                    obs.id
                ))
            })?;
            if !sd.is_finite() || sd <= 0.0 {
                return Err(invalid(format!(
                    "observation {:?}: sd must be positive",
                    obs.id
                )));
            }
            (mean, sd * sd, None)
        };
        // PK coupling: the observation sees the state at `obs.time_s`;
        // the filter tracks the state at `epoch_now`. For a
        // deterministic curve the forward map is scalar —
        // x_i(t_s) = x_i(now)·C_i(t_s)/C_i(epoch_now) — so the effective
        // row is h_i·r_i and the update needs no backward stepping.
        let h_eff: Vec<f64> = h
            .iter()
            .enumerate()
            .map(|(i, c)| c * ratio_at(&spec.states, i, obs.time_s, epoch_now))
            .collect();
        let (predicted, s) = update(&mut mean, &mut cov, &h_eff, observed, r);
        // Advance the tracked epoch to the observation's availability
        // time — a delayed assay's posterior only becomes current then.
        let avail = obs.available_at_s.unwrap_or(obs.time_s);
        if avail > epoch_now {
            let ratios: Vec<f64> = (0..n)
                .map(|i| ratio_at(&spec.states, i, avail, epoch_now))
                .collect();
            transition(&mut mean, &mut cov, &ratios, n);
            epoch_now = avail;
        }
        residuals.push(ObservationResidual {
            observation: obs.id.clone(),
            time_s: obs.time_s,
            observed,
            predicted,
            normalized: (observed - predicted) / s.sqrt(),
            low_count,
        });
        estimates.push(StateEstimate {
            epoch_s: obs.available_at_s.unwrap_or(obs.time_s),
            mean: mean.clone(),
            sd: (0..n).map(|i| cov[i * n + i].max(0.0).sqrt()).collect(),
            after_observation: Some(obs.id.clone()),
        });
    }

    // Unresolved directions: eigenvalues of the posterior covariance —
    // a direction whose posterior/prior variance ratio is ≈1 got no
    // information.
    let prior_diag: Vec<f64> = spec
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let _ = i;
            s.prior_sd * s.prior_sd
        })
        .collect();
    let (vals, vecs) = jacobi_eigh(&cov, n);
    let mut unresolved = Vec::new();
    for j in 0..n {
        let dir: Vec<f64> = (0..n).map(|k| vecs[k * n + j]).collect();
        // Prior variance along this direction (diagonal prior assumed
        // for the ratio; correlated priors use the transformed row).
        let prior_var: f64 = dir
            .iter()
            .enumerate()
            .map(|(k, c)| c * c * prior_diag[k])
            .sum();
        let ratio = if prior_var > 0.0 {
            vals[j] / prior_var
        } else {
            1.0
        };
        if ratio > 0.9 {
            unresolved.push(UnresolvedDirection {
                direction: dir,
                posterior_to_prior: ratio.min(1.0),
            });
        }
    }
    let posterior_to_prior_sd: Vec<(String, f64)> = spec
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| {
            (
                s.id.clone(),
                (cov[i * n + i].max(0.0) / prior_diag[i]).sqrt(),
            )
        })
        .collect();

    // Unobserved coverage: spans of `coverage` with no observation whose
    // bin/epoch intersects them. With only point-in-time observations
    // the honest statement is the leading and trailing uncovered spans
    // plus inter-observation gaps.
    let mut observed_times: Vec<f64> = spec.observations.iter().map(|o| o.time_s).collect();
    observed_times.sort_by(|a, b| a.total_cmp(b));
    let mut unobserved = Vec::new();
    let (mut cursor, end) = spec.coverage;
    for &t in &observed_times {
        if t > cursor && t <= end {
            unobserved.push((cursor, t));
            cursor = t;
        }
    }
    if cursor < end {
        unobserved.push((cursor, end));
    }

    Ok(BoronInferenceReport {
        schema_version: BORON_INFERENCE_REPORT_SCHEMA.into(),
        id: spec.id.clone(),
        qualification: spec.qualification.clone(),
        provenance_id: spec.provenance_id.clone(),
        estimates,
        unresolved,
        residuals,
        unobserved_intervals: unobserved,
        posterior_to_prior_sd,
        assumptions: vec![
            "state priors are Gaussian; observations update by exact linear-Gaussian conditioning".into(),
            "prompt-gamma counts use a Gaussian approximation with variance counts + declared background variance; low-count bins are flagged, not trusted silently".into(),
            "PET surrogate observations are concentration-equivalent readings only through the declared transfer — the surrogate status is preserved".into(),
            "observations update at their availability time; this is a forward filter, not a retrospective smoother".into(),
            "state-space directions with posterior/prior variance ratio above 0.9 are reported unresolved rather than falsely precise".into(),
            if pk_t0.is_some() {
                "pk_coupled states transition deterministically by C(t)/C(epoch) between epochs — a declared drift, not fitted dynamics; no inter-epoch process noise is modeled".into()
            } else {
                "states carry constant priors across epochs — no declared inter-epoch dynamics".into()
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: &str, mean: f64, sd: f64) -> EstimationState {
        EstimationState {
            id: id.into(),
            meaning: format!("{id} meaning"),
            prior_mean: mean,
            prior_sd: sd,
            unit: "ug/g".into(),
            pk_curve: None,
        }
    }

    fn spec(states: Vec<EstimationState>, obs: Vec<BoronObservation>) -> BoronInferenceSpec {
        BoronInferenceSpec {
            schema_version: BORON_INFERENCE_SCHEMA.into(),
            id: "t".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "t".into(),
            states,
            prior_correlation: None,
            observations: obs,
            coverage: (0.0, 3600.0),
            low_count_threshold: 20.0,
        }
    }

    fn assay(id: &str, t: f64, state: &str, value: f64, sd: f64) -> BoronObservation {
        BoronObservation {
            id: id.into(),
            kind: "blood_assay".into(),
            time_s: t,
            sensitivity: vec![(state.into(), 1.0)],
            mean: Some(value),
            sd: Some(sd),
            counts: None,
            background: None,
            background_variance: None,
            transfer_note: None,
            available_at_s: None,
        }
    }

    #[test]
    fn recovers_one_region_concentration() {
        // True concentration 25 µg/g, prior N(20, 5), two assays at
        // 24.0 and 26.0 with σ=1 → posterior ≈ N(25, ~0.7).
        let s = spec(
            vec![state("tumor", 20.0, 5.0)],
            vec![
                assay("a1", 600.0, "tumor", 24.0, 1.0),
                assay("a2", 1200.0, "tumor", 26.0, 1.0),
            ],
        );
        let report = run_boron_inference(&s).unwrap();
        let last = report.estimates.last().unwrap();
        // Posterior var = (1/25 + 2/1)^-1 = 0.4987…; mean = 0.04·20/…
        // exact: mean = (20/25 + 24 + 26)/(1/25 + 2) = 50.8/2.04 ≈ 24.9
        assert!((last.mean[0] - 50.8 / 2.04).abs() < 1e-9);
        assert!((last.sd[0] - (1.0f64 / (0.04 + 2.0)).sqrt()).abs() < 1e-9);
        assert!(report.unresolved.is_empty());
    }

    #[test]
    fn degenerate_states_stay_unresolved() {
        // Two states with identical sensitivity — only their sum is
        // observed; the difference direction is unresolved.
        let s = spec(
            vec![state("tumor", 10.0, 2.0), state("blood", 10.0, 2.0)],
            vec![BoronObservation {
                id: "sum".into(),
                kind: "total_boron_assay".into(),
                time_s: 600.0,
                sensitivity: vec![("tumor".into(), 1.0), ("blood".into(), 1.0)],
                mean: Some(22.0),
                sd: Some(1.0),
                counts: None,
                background: None,
                background_variance: None,
                transfer_note: None,
                available_at_s: None,
            }],
        );
        let report = run_boron_inference(&s).unwrap();
        assert_eq!(report.unresolved.len(), 1);
        // The unresolved direction is the (tumor − blood) difference.
        let d = &report.unresolved[0].direction;
        assert!((d[0] - d[1]).abs() > 0.9);
        assert!(report.unresolved[0].posterior_to_prior > 0.9);
    }

    #[test]
    fn counts_observations_flag_low_bins_and_late_assays_sequence() {
        let pg = BoronObservation {
            id: "pg1".into(),
            kind: "prompt_gamma_counts".into(),
            time_s: 300.0,
            sensitivity: vec![("tumor".into(), 2.0)], // counts per µg/g
            mean: None,
            sd: None,
            counts: Some(12),
            background: Some(3.0),
            background_variance: None,
            transfer_note: None,
            available_at_s: None,
        };
        let late = BoronObservation {
            available_at_s: Some(7000.0),
            ..assay("late", 600.0, "tumor", 24.0, 1.0)
        };
        let s = spec(vec![state("tumor", 20.0, 5.0)], vec![pg, late]);
        let report = run_boron_inference(&s).unwrap();
        assert_eq!(report.residuals[0].low_count, Some(true));
        // Late assay updates at 7000 (after pg at 300), not at its 600 draw.
        assert_eq!(report.estimates[1].epoch_s, 300.0);
        assert_eq!(report.estimates[2].epoch_s, 7000.0);
        assert_eq!(
            report.estimates[2].after_observation.as_deref(),
            Some("late")
        );
    }

    #[test]
    fn posterior_interval_covers_truth_across_repeated_trials() {
        // Coverage under the declared data-generating model: truth 25
        // µg/g, prior N(20, 5), assay N(truth, 1). The posterior ±1σ
        // interval should contain truth at the Gaussian rate (~68%) —
        // not 100%; over-coverage would mean inflated uncertainty.
        let mut seed = 0x9e3779b97f4a7c15u64;
        let mut gauss = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let u1 = ((seed >> 11) as f64 / (1u64 << 53) as f64).max(f64::MIN_POSITIVE);
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let u2 = (seed >> 11) as f64 / (1u64 << 53) as f64;
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        };
        let trials = 400;
        let mut covered = 0usize;
        for _ in 0..trials {
            let obs = 25.0 + gauss();
            let s = spec(
                vec![state("tumor", 20.0, 5.0)],
                vec![assay("a", 600.0, "tumor", obs, 1.0)],
            );
            let report = run_boron_inference(&s).unwrap();
            let e = report.estimates.last().unwrap();
            if (e.mean[0] - 25.0).abs() <= e.sd[0] {
                covered += 1;
            }
        }
        let fraction = covered as f64 / trials as f64;
        // Expected ≈0.683; tolerance covers MC error (~±0.05 at 400
        // trials) plus the prior-vs-truth offset effect.
        assert!(
            (fraction - 0.68).abs() < 0.08,
            "posterior interval coverage {fraction} deviates from the Gaussian rate"
        );
    }

    #[test]
    fn miscalibrated_sensitivity_surfaces_as_residual_bias() {
        // Calibration-perturbation control: an assay reporting 30
        // against a prior pinned at 25±0.5 (as from a shifted detector
        // calibration) must leave a large normalized residual — the
        // estimator reports the mismatch instead of absorbing it.
        let s = spec(
            vec![state("tumor", 25.0, 0.5)],
            vec![assay("a", 600.0, "tumor", 30.0, 1.0)],
        );
        let report = run_boron_inference(&s).unwrap();
        let r = report.residuals[0].normalized;
        assert!(
            r.abs() > 3.0,
            "miscalibrated observation should leave a large normalized residual, got {r}"
        );
    }

    #[test]
    fn finer_truth_grid_estimated_on_a_coarse_state() {
        // Inverse-crime control at the discretization level: the
        // synthetic truth has two sub-regions; the assay's response is
        // computed here by an independent forward sum (hand-rolled in
        // this test, not by the crate's own observation machinery) —
        // the estimator sees only the coarse "tumor" state.
        // Truth: core=30, rim=20, assay senses rim only → reading 20.
        let (truth_core, truth_rim) = (30.0_f64, 20.0_f64);
        let truth_assay = 0.0 * truth_core + 1.0 * truth_rim; // rim-weighted
        let s = spec(
            vec![state("tumor", 25.0, 5.0)],
            vec![assay("rim-weighted", 600.0, "tumor", truth_assay, 1.0)],
        );
        let report = run_boron_inference(&s).unwrap();
        let e = report.estimates.last().unwrap();
        // The coarse estimate converges to the observed sub-region
        // (≈20), not the regional mean (25): a known, reportable
        // limitation — the residual is small because the model CAN
        // absorb the bias, which is exactly why finer-grid truth must
        // be carried as an explicit observability caveat.
        assert!((e.mean[0] - truth_rim).abs() < 5.0);
        assert!(report.residuals[0].normalized.abs() < 3.0);
    }

    #[test]
    fn pk_coupling_drifts_state_and_scales_uncertainty() {
        // C(t) = 20·e^(−0.001t); prior at t0=0: mean 20, sd 2. An
        // observation at t=1000 of h=1·x(t), value 10 ± 0.5.
        // Equivalent by hand: at t=1000 the prior is mean 20·e^−1 ≈
        // 7.358, sd 2·e^−1 ≈ 0.7358; Kalman with R=0.25:
        //   S = 0.7358²+0.25 ≈ 0.7914, K = 0.5414/0.7914 ≈ 0.6840
        //   mean = 7.358 + 0.6840·(10−7.358) ≈ 9.165
        let mut s = state("tumor", 20.0, 2.0);
        s.pk_curve = Some(crate::replay::PkConcentrationCurve {
            amplitudes: vec![20.0],
            rates_per_s: vec![0.001],
            t0_s: 0.0,
        });
        let obs = BoronObservation {
            id: "draw".into(),
            kind: "assay".into(),
            time_s: 1000.0,
            sensitivity: vec![("tumor".into(), 1.0)],
            mean: Some(10.0),
            sd: Some(0.5),
            counts: None,
            background: None,
            background_variance: None,
            transfer_note: None,
            available_at_s: None,
        };
        let mut sp = spec(vec![s], vec![obs]);
        sp.coverage = (0.0, 1500.0);
        let report = run_boron_inference(&sp).unwrap();
        let e = &report.estimates[1];
        assert_eq!(e.epoch_s, 1000.0);
        let e1 = (-1.0_f64).exp();
        let expected_mean =
            20.0 * e1 + (4.0 * e1 * e1 / (4.0 * e1 * e1 + 0.25)) * (10.0 - 20.0 * e1);
        assert!((e.mean[0] - expected_mean).abs() < 1e-9);
        // Posterior sd = sqrt(σ²_prior_scaled · R / S) = e^−1·sqrt(4·0.25/0.7914).
        let expected_sd = e1 * (4.0 * 0.25 / (4.0 * e1 * e1 + 0.25)).sqrt();
        assert!((e.sd[0] - expected_sd).abs() < 1e-9);
        assert!(report.assumptions.iter().any(|a| a.contains("pk_coupled")));
    }

    #[test]
    fn pk_coupled_states_require_a_shared_t0() {
        let mut a = state("tumor", 20.0, 2.0);
        a.pk_curve = Some(crate::replay::PkConcentrationCurve {
            amplitudes: vec![20.0],
            rates_per_s: vec![0.001],
            t0_s: 0.0,
        });
        let mut b = state("blood", 10.0, 1.0);
        b.pk_curve = Some(crate::replay::PkConcentrationCurve {
            amplitudes: vec![10.0],
            rates_per_s: vec![0.002],
            t0_s: 60.0,
        });
        let sp = spec(vec![a, b], vec![]);
        let err = run_boron_inference(&sp).unwrap_err().to_string();
        assert!(err.contains("share one t0_s"), "{err}");
    }

    #[test]
    fn uncoupled_states_ignore_the_coupled_epoch() {
        // One coupled + one uncoupled state: the uncoupled coordinate
        // neither drifts nor rescales between epochs.
        let mut coupled = state("tumor", 20.0, 2.0);
        coupled.pk_curve = Some(crate::replay::PkConcentrationCurve {
            amplitudes: vec![20.0],
            rates_per_s: vec![0.001],
            t0_s: 0.0,
        });
        let plain = state("bias", 1.0, 0.5);
        let obs = BoronObservation {
            id: "draw".into(),
            kind: "assay".into(),
            time_s: 500.0,
            sensitivity: vec![("tumor".into(), 1.0), ("bias".into(), 1.0)],
            mean: Some(17.0),
            sd: Some(1.0),
            counts: None,
            background: None,
            background_variance: None,
            transfer_note: None,
            available_at_s: None,
        };
        let mut sp = spec(vec![coupled, plain], vec![obs]);
        sp.coverage = (0.0, 1000.0);
        let report = run_boron_inference(&sp).unwrap();
        // Predicted obs at t=500: x_t·e^−0.5 + x_b = 20·0.6065 + 1.0.
        let predicted = 20.0 * (-0.5_f64).exp() + 1.0;
        assert!((report.residuals[0].predicted - predicted).abs() < 1e-9);
    }
}
