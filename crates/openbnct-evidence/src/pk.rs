// SPDX-License-Identifier: MIT

//! Pharmacokinetic-aware irradiation-time evaluation.
//!
//! Standard BNCT planning assumes a fixed boron concentration through the
//! irradiation; measured blood 10B in fact decays on a compartment
//! timescale comparable to the beam-on window, and time-varying-versus-
//! fixed-concentration dose calculations differ by ~11% in published
//! patient studies (JRR rraf038). This module integrates the boron dose
//! component under a declared per-region concentration curve and solves
//! the *implicit* beam-off time: `D_r(t*) = limit` where
//!
//! ```text
//! D_r(t) = S · [ O_r·t + B_r·f_r(t) ] ,
//! f_r(t) = (1/C_plan,r) · ∫₀ᵗ Σᵢ aᵢ·e^(−λᵢτ) dτ
//!        = (1/C_plan,r) · Σᵢ aᵢ·(1 − e^(−λᵢt))/λᵢ
//! ```
//!
//! with `O_r` the region statistic of the non-boron endpoint map,
//! `B_r` the same statistic of the boron map, `S` the source strength,
//! and `C_r(τ) = Σᵢ aᵢ e^(−λᵢτ)` the region's concentration curve in
//! ppm, normalized against the concentration the dose map was computed
//! at. Because `f_r` is uniform across a region's voxels the time-scaled
//! map is `O + f·B` elementwise — the statistic (mean, max, or `D_x`)
//! is evaluated on that map at each solver iteration, so nonlinear
//! statistics stay exact rather than being approximated as
//! `stat(O) + f·stat(B)`.
//!
//! Research software only — the PK curves are declared inputs, not a
//! fitted pharmacokinetic model; no clinical use.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, RegionMask};

use crate::ManifestError;
use crate::limits::{LimitMetric, OrganLimit};

pub const PK_MODEL_SCHEMA: &str = "openbnct.pk-model/0.1.0";
pub const PK_SAMPLES_SCHEMA: &str = "openbnct.pk-samples/0.1.0";
pub const PK_IRRADIATION_SCHEMA: &str = "openbnct.pk-irradiation-report/0.1.0";

/// One measured blood/tissue draw: concentration at `time_s` since
/// beam-on (or since whatever epoch the downstream model declares).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkSample {
    pub time_s: f64,
    pub concentration_ppm: f64,
}

/// One region's measured concentration series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkSampleSeries {
    pub region: String,
    /// Concentration the dose map was computed at. When omitted the
    /// fitted value at t=0 is used.
    #[serde(default)]
    pub planned_concentration_ppm: Option<f64>,
    pub samples: Vec<PkSample>,
}

/// `openbnct.pk-samples/0.1.0` — measured concentration draws per
/// region, the input to [`fit_pk_model`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkSamples {
    pub schema_version: String,
    pub id: String,
    pub regions: Vec<PkSampleSeries>,
    /// Free-text provenance (assay method, draw protocol, …). Required.
    pub basis: String,
}

impl PkSamples {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, PK_SAMPLES_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported pk-samples schema {:?}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() || self.basis.trim().is_empty() {
            return Err(ManifestError::Invalid(
                "pk-samples id and basis must be non-empty".into(),
            ));
        }
        for series in &self.regions {
            if series.samples.is_empty() {
                return Err(ManifestError::Invalid(format!(
                    "region {:?}: no samples",
                    series.region
                )));
            }
            for s in &series.samples {
                if !s.time_s.is_finite()
                    || s.time_s < 0.0
                    || !s.concentration_ppm.is_finite()
                    || s.concentration_ppm <= 0.0
                {
                    return Err(ManifestError::Invalid(format!(
                        "region {:?}: sample times must be finite ≥0 and concentrations finite positive",
                        series.region
                    )));
                }
            }
            if let Some(planned) = series.planned_concentration_ppm
                && (!planned.is_finite() || planned <= 0.0)
            {
                return Err(ManifestError::Invalid(format!(
                    "region {:?}: planned_concentration_ppm must be finite positive",
                    series.region
                )));
            }
        }
        Ok(())
    }
}

/// Fit each sample series to a sum of `exponentials` decaying
/// exponentials, producing a [`PkModel`].
///
/// Monoexponential fits are exact (log-linear least squares).
/// Biexponential fits use separable least squares: a log-spaced rate
/// grid is scanned, amplitudes solved by 2×2 linear least squares at
/// each pair, and the best residual kept — robust without a nonlinear
/// optimizer, and deterministic for a versioned artifact. Negative
/// fitted amplitudes are rejected (a rising compartment would break the
/// dose monotonicity the solver assumes).
pub fn fit_pk_model(
    samples: &PkSamples,
    model_id: &str,
    exponentials: usize,
) -> Result<PkModel, ManifestError> {
    samples.validate()?;
    if !(1..=2).contains(&exponentials) {
        return Err(ManifestError::Invalid("exponentials must be 1 or 2".into()));
    }
    let mut regions = Vec::with_capacity(samples.regions.len());
    for series in &samples.regions {
        if series.samples.len() < exponentials + 1 {
            return Err(ManifestError::Invalid(format!(
                "region {:?}: {} samples cannot constrain {} exponentials",
                series.region,
                series.samples.len(),
                exponentials
            )));
        }
        let (amplitudes, rates, rms) = if exponentials == 1 {
            fit_monoexponential(&series.region, &series.samples)?
        } else {
            fit_biexponential(&series.region, &series.samples)?
        };
        let planned = series
            .planned_concentration_ppm
            .unwrap_or_else(|| amplitudes.iter().sum());
        regions.push(PkRegion {
            region: series.region.clone(),
            planned_concentration_ppm: planned,
            amplitudes_ppm: amplitudes,
            rates_per_s: rates,
        });
        let _ = rms; // carried via basis below
    }
    Ok(PkModel {
        schema_version: PK_MODEL_SCHEMA.into(),
        id: model_id.into(),
        basis: format!("fitted from {} ({})", samples.id, samples.basis),
        regions,
    })
}

/// ln C = ln a − λt → closed-form weighted least squares.
fn fit_monoexponential(
    region: &str,
    samples: &[PkSample],
) -> Result<(Vec<f64>, Vec<f64>, f64), ManifestError> {
    let n = samples.len() as f64;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for s in samples {
        let y = s.concentration_ppm.ln();
        sx += s.time_s;
        sy += y;
        sxx += s.time_s * s.time_s;
        sxy += s.time_s * y;
    }
    let det = n * sxx - sx * sx;
    if det.abs() < f64::EPSILON {
        return Err(ManifestError::Invalid(format!(
            "region {region:?}: sample times are degenerate"
        )));
    }
    let intercept = (sy * sxx - sx * sxy) / det;
    let slope = (n * sxy - sx * sy) / det;
    let (a, lambda) = (intercept.exp(), -slope);
    if !(a.is_finite() && a > 0.0 && lambda.is_finite() && lambda >= 0.0) {
        return Err(ManifestError::Invalid(format!(
            "region {region:?}: samples do not fit a decaying exponential (slope {slope})"
        )));
    }
    let rms = (samples
        .iter()
        .map(|s| (s.concentration_ppm - a * (-lambda * s.time_s).exp()).powi(2))
        .sum::<f64>()
        / n)
        .sqrt();
    Ok((vec![a], vec![lambda], rms))
}

/// Variable-projection-lite: scan a log-spaced (λ₁, λ₂) grid; at each
/// pair solve the 2×2 normal equations for amplitudes; keep the least
/// residual. Deterministic; rates span the sample time window.
fn fit_biexponential(
    region: &str,
    samples: &[PkSample],
) -> Result<(Vec<f64>, Vec<f64>, f64), ManifestError> {
    let t_min = samples
        .iter()
        .map(|s| s.time_s)
        .fold(f64::INFINITY, f64::min);
    let t_max = samples.iter().map(|s| s.time_s).fold(0.0, f64::max);
    let span = (t_max - t_min).max(1.0);
    // Rates from "decays within the window" to "nearly constant".
    let rate_lo = 0.05 / span;
    let rate_hi = 20.0 / span;
    let n_grid = 60usize;
    let log_step = (rate_hi / rate_lo).ln() / (n_grid - 1) as f64;
    let rate_at = |i: usize| rate_lo * (log_step * i as f64).exp();

    let mut best: Option<(f64, f64, f64, f64, f64)> = None; // sse, a1, l1, a2, l2
    for i in 0..n_grid {
        for j in (i + 1)..n_grid {
            let (l1, l2) = (rate_at(i), rate_at(j));
            // Normal equations for amplitudes over the basis e^{-λt}.
            let (mut g11, mut g12, mut g22, mut h1, mut h2) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for s in samples {
                let (e1, e2) = ((-l1 * s.time_s).exp(), (-l2 * s.time_s).exp());
                g11 += e1 * e1;
                g12 += e1 * e2;
                g22 += e2 * e2;
                h1 += e1 * s.concentration_ppm;
                h2 += e2 * s.concentration_ppm;
            }
            let det = g11 * g22 - g12 * g12;
            if det.abs() < f64::EPSILON {
                continue;
            }
            let a1 = (h1 * g22 - h2 * g12) / det;
            let a2 = (h2 * g11 - h1 * g12) / det;
            if a1 <= 0.0 || a2 <= 0.0 {
                continue; // rising compartment — rejected
            }
            let sse: f64 = samples
                .iter()
                .map(|s| {
                    (s.concentration_ppm
                        - a1 * (-l1 * s.time_s).exp()
                        - a2 * (-l2 * s.time_s).exp())
                    .powi(2)
                })
                .sum();
            if best.is_none_or(|b| sse < b.0) {
                best = Some((sse, a1, l1, a2, l2));
            }
        }
    }
    let Some((sse, a1, l1, a2, l2)) = best else {
        return Err(ManifestError::Invalid(format!(
            "region {region:?}: no decaying biexponential fits the samples"
        )));
    };
    Ok((
        vec![a1, a2],
        vec![l1, l2],
        (sse / samples.len() as f64).sqrt(),
    ))
}

/// One region's concentration curve and planning normalization.
///
/// `concentration(t) = Σᵢ amplitudes_ppm[i] · exp(−rates_per_s[i]·t)`
/// with `t` in seconds from beam-on. `planned_concentration_ppm` is the
/// concentration the dose map's boron component was computed at — the
/// curve is a *ratio* to that reference, so absolute calibration error
/// in the dose map cancels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkRegion {
    pub region: String,
    pub planned_concentration_ppm: f64,
    pub amplitudes_ppm: Vec<f64>,
    pub rates_per_s: Vec<f64>,
}

/// `openbnct.pk-model/0.1.0` — declared regional boron-concentration
/// curves over the irradiation window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkModel {
    pub schema_version: String,
    pub id: String,
    pub regions: Vec<PkRegion>,
    /// Free-text provenance: e.g. the population-PK study or measured
    /// blood samples the curves were fit from. Required — a PK model
    /// without a stated basis is not admissible.
    pub basis: String,
}

impl PkModel {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, PK_MODEL_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported pk-model schema {:?}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() || self.basis.trim().is_empty() {
            return Err(ManifestError::Invalid(
                "pk-model id and basis must be non-empty".into(),
            ));
        }
        for region in &self.regions {
            if !region.planned_concentration_ppm.is_finite()
                || region.planned_concentration_ppm <= 0.0
            {
                return Err(ManifestError::Invalid(format!(
                    "region {:?}: planned_concentration_ppm must be finite positive",
                    region.region
                )));
            }
            if region.amplitudes_ppm.is_empty()
                || region.amplitudes_ppm.len() != region.rates_per_s.len()
            {
                return Err(ManifestError::Invalid(format!(
                    "region {:?}: amplitudes_ppm and rates_per_s must be non-empty and equal length",
                    region.region
                )));
            }
            for (amplitude, rate) in region.amplitudes_ppm.iter().zip(region.rates_per_s.iter()) {
                if !amplitude.is_finite() || !rate.is_finite() || *rate < 0.0 {
                    return Err(ManifestError::Invalid(format!(
                        "region {:?}: PK coefficients must be finite with non-negative rates",
                        region.region
                    )));
                }
            }
            if region.amplitudes_ppm.iter().sum::<f64>() <= 0.0 {
                return Err(ManifestError::Invalid(format!(
                    "region {:?}: concentration curve has no positive mass at beam-on",
                    region.region
                )));
            }
        }
        Ok(())
    }

    /// Concentration scaling factor integral
    /// `(1/C_plan)·∫₀ᵗ C(τ) dτ` for `region`; returns 1.0·t-equivalent
    /// (constant concentration at the planned value) when the region
    /// has no declared curve.
    fn boron_time_integral(&self, region: &str, t: f64) -> f64 {
        let Some(pk) = self.regions.iter().find(|r| r.region == region) else {
            return t;
        };
        pk.amplitudes_ppm
            .iter()
            .zip(pk.rates_per_s.iter())
            .map(|(a, l)| {
                if *l == 0.0 {
                    a * t
                } else {
                    a * (1.0 - (-l * t).exp()) / l
                }
            })
            .sum::<f64>()
            / pk.planned_concentration_ppm
    }

    /// Concentration ratio `C_r(t)/C_plan` at time `t` (1.0 when the
    /// region has no declared curve).
    fn concentration_ratio(&self, region: &str, t: f64) -> f64 {
        let Some(pk) = self.regions.iter().find(|r| r.region == region) else {
            return 1.0;
        };
        pk.amplitudes_ppm
            .iter()
            .zip(pk.rates_per_s.iter())
            .map(|(a, l)| a * (-l * t).exp())
            .sum::<f64>()
            / pk.planned_concentration_ppm
    }
}

/// One region's PK-aware admissible irradiation, paired with the
/// constant-concentration answer for deviation reporting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkRegionResult {
    pub region: String,
    pub metric: LimitMetric,
    /// The declared limit, in `endpoint_unit` of the parent report.
    pub limit: f64,
    pub region_voxel_count: u64,
    /// `true` when the PK model declares a curve for this region;
    /// `false` means constant-concentration fallback.
    pub pk_modeled: bool,
    /// Beam-on time solving `D(t) = limit` under the PK curve;
    /// `None` when the asymptotic dose cannot reach the limit.
    pub max_time_s: Option<f64>,
    /// Constant-concentration answer for the same limit
    /// (the `irradiation-time` semantics).
    pub static_max_time_s: Option<f64>,
    /// `(pk − static)/static`; `None` when either side is unbounded.
    pub relative_deviation: Option<f64>,
    /// Boron concentration ratio `C(t*)/C_plan` at the solved beam-off
    /// time — how far washout carried the region below plan.
    pub concentration_ratio_at_beam_off: Option<f64>,
}

/// Deterministic PK-aware organ-limited irradiation-time evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkIrradiationReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// `physical_total`, `biological_total`, or `component:boron`.
    pub quantity: String,
    /// Content binding of the dose artifact evaluated.
    pub source: ContentReference,
    /// Content binding of the PK model applied.
    pub pk_model: ContentReference,
    /// Endpoint unit, copied verbatim; always `*_per_source_particle`.
    pub endpoint_unit: String,
    /// Source strength in source particles per second.
    pub source_strength_per_s: f64,
    pub regions: Vec<PkRegionResult>,
    /// `None` when every region is unbounded.
    pub limiting: Option<crate::limits::LimitingStructure>,
    pub assumptions: Vec<String>,
}

impl PkIrradiationReport {
    /// Evaluate `limits` under `pk` over a component-split endpoint map.
    ///
    /// `total_values` is the endpoint map at the *planned* boron
    /// concentration; `boron_values` is the boron component alone, same
    /// length, so the non-boron remainder is `total − boron` pointwise.
    /// Per-region statistics are recomputed on the time-scaled map at
    /// every solver iteration — max/`D_x` of a scaled sum are evaluated
    /// exactly, not composed from separate statistics.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        case_id: &str,
        quantity: &str,
        source: ContentReference,
        pk_model: ContentReference,
        pk: &PkModel,
        unit: &str,
        total_values: &[f64],
        boron_values: &[f64],
        masks: &[RegionMask],
        limits: &[OrganLimit],
        source_strength: f64,
    ) -> Result<Self, ManifestError> {
        pk.validate()?;
        if !unit.ends_with("per_source_particle") {
            return Err(ManifestError::Invalid(format!(
                "endpoint unit {unit:?} is not per-source-particle; irradiation time needs a rate-capable endpoint"
            )));
        }
        if !source_strength.is_finite() || source_strength <= 0.0 {
            return Err(ManifestError::Invalid(
                "source strength must be finite and positive".into(),
            ));
        }
        if limits.is_empty() {
            return Err(ManifestError::Invalid(
                "at least one organ limit is required".into(),
            ));
        }
        if total_values.len() != boron_values.len() {
            return Err(ManifestError::Invalid(format!(
                "total map has {} voxels but boron map has {}",
                total_values.len(),
                boron_values.len()
            )));
        }
        source
            .validate()
            .map_err(|_| ManifestError::Invalid("dose source reference is invalid".into()))?;
        pk_model
            .validate()
            .map_err(|_| ManifestError::Invalid("pk-model reference is invalid".into()))?;

        // Non-boron remainder map: what the endpoint would be at zero
        // boron. Guarded against negative totals from floating roundoff.
        let other: Vec<f64> = total_values
            .iter()
            .zip(boron_values.iter())
            .map(|(t, b)| (t - b).max(0.0))
            .collect();

        let invalid = |region: &str, e: openbnct_core::ValidationError| {
            ManifestError::Invalid(format!("region {region:?}: {e}"))
        };
        let statistic = |metric: LimitMetric, values: &[f64], region: &str| match metric {
            LimitMetric::Max => Ok(values.iter().copied().fold(0.0, f64::max)),
            LimitMetric::Mean => Ok(openbnct_core::mean(values)),
            LimitMetric::DoseCoverage { percent } => {
                openbnct_core::dose_covering_percent(values, f64::from(percent))
                    .map_err(|e| invalid(region, e))
            }
        };

        // Endpoint dose in `region` for metric `limit` at beam-on time t:
        // statistic over the masked time-scaled map O + f(t)·B.
        let endpoint_at = |mask: &RegionMask,
                           region: &str,
                           metric: LimitMetric,
                           f: f64,
                           other: &[f64],
                           boron: &[f64]|
         -> Result<f64, ManifestError> {
            let mut selected = Vec::new();
            for (inside, (o, b)) in mask.voxels.iter().zip(other.iter().zip(boron.iter())) {
                if *inside {
                    selected.push(o + f * b);
                }
            }
            if selected.is_empty() {
                return Err(invalid(
                    region,
                    openbnct_core::ValidationError::EmptyMask(region.into()),
                ));
            }
            statistic(metric, &selected, region).map(|s| s * source_strength)
        };

        let mut regions = Vec::with_capacity(limits.len());
        for limit in limits {
            if !limit.limit.is_finite() || limit.limit < 0.0 {
                return Err(ManifestError::Invalid(format!(
                    "limit for region {:?} must be finite and non-negative",
                    limit.region
                )));
            }
            let mask = masks
                .iter()
                .find(|mask| mask.name == limit.region)
                .ok_or_else(|| {
                    ManifestError::Invalid(format!("no mask for limit region {:?}", limit.region))
                })?;
            if mask.voxels.len() != total_values.len() {
                return Err(invalid(
                    &limit.region,
                    openbnct_core::ValidationError::MaskValuesLength {
                        mask: limit.region.clone(),
                        mask_voxels: mask.voxels.len(),
                        values: total_values.len(),
                    },
                ));
            }
            let pk_modeled = pk.regions.iter().any(|r| r.region == limit.region);
            let voxel_count = mask.voxels.iter().filter(|v| **v).count() as u64;

            // Constant-concentration reference answer (f ≡ 1).
            let static_statistic =
                endpoint_at(mask, &limit.region, limit.metric, 1.0, &other, boron_values)?;
            let static_max_time_s = if static_statistic == 0.0 {
                None
            } else {
                Some(limit.limit / static_statistic)
            };

            // Implicit solve D(t) = limit under the PK curve. D is
            // monotonically non-decreasing in t for non-negative maps.
            let max_time_s = if pk_modeled {
                // Asymptote: D(t) = S·t·stat(O + (I(t)/t)·B) grows
                // without bound whenever the non-boron map delivers a
                // nonzero rate over the mask (O·t term) or the curve
                // carries a constant (λ=0) amplitude. Only a mask with
                // O ≡ 0 and a fully-decaying curve has a finite
                // asymptote S·I∞·stat(B).
                let o_selected = openbnct_core::masked_values(&mask.name, &other, &mask.voxels)
                    .map_err(|e| invalid(&limit.region, e))?;
                let o_rate = statistic(limit.metric, &o_selected, &limit.region)?;
                let i_inf = pk
                    .regions
                    .iter()
                    .find(|r| r.region == limit.region)
                    .map(|r| {
                        r.amplitudes_ppm
                            .iter()
                            .zip(r.rates_per_s.iter())
                            .map(|(a, l)| {
                                if *l == 0.0 {
                                    if *a > 0.0 { f64::INFINITY } else { 0.0 }
                                } else {
                                    a / l
                                }
                            })
                            .sum::<f64>()
                            / r.planned_concentration_ppm
                    })
                    .unwrap_or(f64::INFINITY);
                if o_rate == 0.0 && i_inf.is_finite() {
                    let b_selected =
                        openbnct_core::masked_values(&mask.name, boron_values, &mask.voxels)
                            .map_err(|e| invalid(&limit.region, e))?;
                    let b_rate = statistic(limit.metric, &b_selected, &limit.region)?;
                    let asymptote = source_strength * i_inf * b_rate;
                    if asymptote < limit.limit {
                        regions.push(PkRegionResult {
                            region: limit.region.clone(),
                            metric: limit.metric,
                            limit: limit.limit,
                            region_voxel_count: voxel_count,
                            pk_modeled,
                            max_time_s: None,
                            static_max_time_s,
                            relative_deviation: None,
                            concentration_ratio_at_beam_off: None,
                        });
                        continue;
                    }
                }
                // Bracket: the static answer is a bound when f ≤ 1
                // (decay curves), but a rising curve can allow longer —
                // grow the bracket geometrically until D(hi) ≥ limit.
                let mut lo = 0.0_f64;
                let mut hi = static_max_time_s.unwrap_or(1.0).max(1.0e-3);
                for _ in 0..200 {
                    let f_int = pk.boron_time_integral(&limit.region, hi);
                    let d = endpoint_at_scaled(
                        mask,
                        &limit.region,
                        limit.metric,
                        &other,
                        boron_values,
                        hi,
                        f_int,
                        source_strength,
                    )?;
                    if d >= limit.limit {
                        break;
                    }
                    lo = hi;
                    hi *= 2.0;
                    if hi > 1.0e9 {
                        return Err(ManifestError::Invalid(format!(
                            "region {:?}: cannot bracket irradiation time below 1e9 s",
                            limit.region
                        )));
                    }
                }
                // Bisection — 80 iterations is far past f64 resolution.
                for _ in 0..80 {
                    let mid = 0.5 * (lo + hi);
                    let f_int = pk.boron_time_integral(&limit.region, mid);
                    let d = endpoint_at_scaled(
                        mask,
                        &limit.region,
                        limit.metric,
                        &other,
                        boron_values,
                        mid,
                        f_int,
                        source_strength,
                    )?;
                    if d >= limit.limit {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                Some(hi)
            } else {
                static_max_time_s
            };

            let relative_deviation = match (max_time_s, static_max_time_s) {
                (Some(pk_t), Some(static_t)) if static_t > 0.0 => {
                    Some((pk_t - static_t) / static_t)
                }
                _ => None,
            };
            let concentration_ratio_at_beam_off =
                max_time_s.map(|t| pk.concentration_ratio(&limit.region, t));

            regions.push(PkRegionResult {
                region: limit.region.clone(),
                metric: limit.metric,
                limit: limit.limit,
                region_voxel_count: voxel_count,
                pk_modeled,
                max_time_s,
                static_max_time_s,
                relative_deviation,
                concentration_ratio_at_beam_off,
            });
        }

        let limiting = regions
            .iter()
            .filter_map(|region| region.max_time_s.map(|time| (region, time)))
            .min_by(|(a, time_a), (b, time_b)| {
                time_a
                    .total_cmp(time_b)
                    .then_with(|| a.region.cmp(&b.region))
            })
            .map(|(region, time)| crate::limits::LimitingStructure {
                region: region.region.clone(),
                metric: region.metric,
                max_time_s: time,
                max_source_particles: time * source_strength,
            });

        Ok(Self {
            schema_version: PK_IRRADIATION_SCHEMA.into(),
            case_id: case_id.into(),
            quantity: quantity.into(),
            source,
            pk_model,
            endpoint_unit: unit.into(),
            source_strength_per_s: source_strength,
            regions,
            limiting,
            assumptions: vec![
                "boron dose scales linearly with regional 10B concentration; other components are concentration-independent".into(),
                "declared PK curves are evaluated from beam-on; T/N ratios are assumed constant within each region over the irradiation".into(),
                "endpoint accumulates linearly with delivered source particles at constant source strength; anatomy and the endpoint map are static".into(),
                "region limits apply to the stated max/mean/D_x statistic only; no inter-fraction recovery or repopulation is modeled".into(),
                "PK curves are declared research inputs with a stated basis; they are not fitted, validated, or clinically qualified here".into(),
            ],
        })
    }
}

/// `S·stat(O·t + B·I(t))` evaluated as `S·t·stat(O + (I(t)/t)·B)` over
/// the mask — the region-uniform concentration ratio scales the boron
/// map elementwise, so nonlinear statistics (`D_x`, max) are computed
/// exactly on the time-scaled map rather than composed.
#[allow(clippy::too_many_arguments)]
fn endpoint_at_scaled(
    mask: &RegionMask,
    region: &str,
    metric: LimitMetric,
    other: &[f64],
    boron: &[f64],
    t: f64,
    boron_integral: f64,
    source_strength: f64,
) -> Result<f64, ManifestError> {
    let f = if t > 0.0 { boron_integral / t } else { 1.0 };
    let mut selected = Vec::new();
    for (inside, (o, b)) in mask.voxels.iter().zip(other.iter().zip(boron.iter())) {
        if *inside {
            selected.push(o + f * b);
        }
    }
    if selected.is_empty() {
        return Err(ManifestError::Invalid(format!(
            "region {region:?}: empty mask"
        )));
    }
    let stat = match metric {
        LimitMetric::Max => selected.iter().copied().fold(0.0, f64::max),
        LimitMetric::Mean => openbnct_core::mean(&selected),
        LimitMetric::DoseCoverage { percent } => {
            openbnct_core::dose_covering_percent(&selected, f64::from(percent))
                .map_err(|e| ManifestError::Invalid(format!("region {region:?}: {e}")))?
        }
    };
    Ok(stat * source_strength * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ContentReference {
        ContentReference {
            id: "test.dose.v1".into(),
            sha256: "ab".repeat(32),
        }
    }

    fn pk_ref() -> ContentReference {
        ContentReference {
            id: "test.pk.v1".into(),
            sha256: "cd".repeat(32),
        }
    }

    fn mask(name: &str, voxels: &[bool]) -> RegionMask {
        RegionMask {
            name: name.into(),
            voxels: voxels.to_vec(),
        }
    }

    fn decaying_model() -> PkModel {
        PkModel {
            schema_version: PK_MODEL_SCHEMA.into(),
            id: "test.pk.decay".into(),
            basis: "unit test".into(),
            regions: vec![PkRegion {
                region: "A".into(),
                planned_concentration_ppm: 10.0,
                // C(t) = 10·e^(−0.1t) ppm — decays on the ~10 s scale
                // of the test irradiation.
                amplitudes_ppm: vec![10.0],
                rates_per_s: vec![0.1],
            }],
        }
    }

    #[test]
    fn pk_solve_integrates_boron_decay() {
        // Endpoint: boron map 2.0/particle in region A, nothing else.
        // Planned concentration 10 ppm, decaying curve — the PK time
        // must exceed the static 5/2 = 2.5 s.
        let total = vec![2.0, 2.0];
        let boron = vec![2.0, 2.0];
        let masks = [mask("A", &[true, true])];
        let limits = [OrganLimit {
            region: "A".into(),
            metric: LimitMetric::Mean,
            limit: 5.0,
        }];
        let report = PkIrradiationReport::evaluate(
            "case",
            "physical_total",
            source(),
            pk_ref(),
            &decaying_model(),
            "gray_per_source_particle",
            &total,
            &boron,
            &masks,
            &limits,
            1.0,
        )
        .unwrap();
        let r = &report.regions[0];
        assert!(r.pk_modeled);
        assert_eq!(r.static_max_time_s, Some(2.5));
        // Exact: 2·I(t) = 5 with I(t) = 10(1−e^(−0.1t)) →
        // t* = −ln(0.75)/0.1 ≈ 2.877 s.
        let expected = -0.75_f64.ln() / 0.1;
        assert!((r.max_time_s.unwrap() - expected).abs() < 1e-6 * expected);
        assert!(r.concentration_ratio_at_beam_off.unwrap() < 1.0);
    }

    #[test]
    fn unmodeled_region_matches_static() {
        let total = vec![3.0];
        let boron = vec![1.0];
        let masks = [mask("B", &[true])];
        let limits = [OrganLimit {
            region: "B".into(),
            metric: LimitMetric::Max,
            limit: 6.0,
        }];
        let report = PkIrradiationReport::evaluate(
            "case",
            "physical_total",
            source(),
            pk_ref(),
            &decaying_model(),
            "gray_per_source_particle",
            &total,
            &boron,
            &masks,
            &limits,
            1.0,
        )
        .unwrap();
        let r = &report.regions[0];
        assert!(!r.pk_modeled);
        assert_eq!(r.max_time_s, r.static_max_time_s);
        assert_eq!(r.relative_deviation, Some(0.0));
    }

    fn samples_doc(concentrations: &[(f64, f64)]) -> PkSamples {
        PkSamples {
            schema_version: PK_SAMPLES_SCHEMA.into(),
            id: "test.samples.v1".into(),
            basis: "unit test".into(),
            regions: vec![PkSampleSeries {
                region: "A".into(),
                planned_concentration_ppm: None,
                samples: concentrations
                    .iter()
                    .map(|(time_s, concentration_ppm)| PkSample {
                        time_s: *time_s,
                        concentration_ppm: *concentration_ppm,
                    })
                    .collect(),
            }],
        }
    }

    #[test]
    fn monoexponential_fit_recovers_lambda() {
        // C(t) = 25·e^(−0.05t), noiseless draws.
        let samples = samples_doc(
            &[10.0f64, 40.0, 90.0]
                .iter()
                .map(|t| (*t, 25.0 * (-0.05 * *t).exp()))
                .collect::<Vec<_>>(),
        );
        let model = fit_pk_model(&samples, "m.v1", 1).unwrap();
        let r = &model.regions[0];
        assert!((r.amplitudes_ppm[0] - 25.0).abs() < 1e-6);
        assert!((r.rates_per_s[0] - 0.05).abs() < 1e-9);
        assert!((r.planned_concentration_ppm - 25.0).abs() < 1e-6);
    }

    #[test]
    fn biexponential_fit_recovers_both_rates() {
        // C(t) = 22·e^(−0.004t) + 8·e^(−0.0002t), noiseless.
        let samples = samples_doc(
            &[60.0f64, 180.0, 300.0, 480.0, 720.0, 900.0, 1200.0]
                .iter()
                .map(|t| (*t, 22.0 * (-0.004 * *t).exp() + 8.0 * (-0.0002 * *t).exp()))
                .collect::<Vec<_>>(),
        );
        let model = fit_pk_model(&samples, "m.v1", 2).unwrap();
        let r = &model.regions[0];
        let mut rates = r.rates_per_s.clone();
        rates.sort_by(f64::total_cmp);
        assert!((rates[0] - 0.0002).abs() < 0.0001);
        assert!((rates[1] - 0.004).abs() < 0.0005);
        assert!((r.amplitudes_ppm.iter().sum::<f64>() - 30.0).abs() < 0.5);
    }

    #[test]
    fn bounded_asymptote_marks_unbounded() {
        // Boron fully washes out; asymptotic boron dose cannot reach
        // the limit — region reported unbounded, not an error.
        let total = vec![1.0];
        let boron = vec![1.0];
        let masks = [mask("A", &[true])];
        let limits = [OrganLimit {
            region: "A".into(),
            metric: LimitMetric::Mean,
            limit: 1.0e6,
        }];
        let report = PkIrradiationReport::evaluate(
            "case",
            "physical_total",
            source(),
            pk_ref(),
            &decaying_model(),
            "gray_per_source_particle",
            &total,
            &boron,
            &masks,
            &limits,
            1.0,
        )
        .unwrap();
        assert_eq!(report.regions[0].max_time_s, None);
        assert!(report.limiting.is_none());
    }
}
