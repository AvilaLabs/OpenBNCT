// SPDX-License-Identifier: Apache-2.0

//! Measurement record import and measurement-vs-computation comparison
//! (`openbnct.measurement-record`, `openbnct.measurement-comparison`).
//!
//! Verification in BNCT ultimately means code-versus-measurement, not
//! only code-versus-code. This module defines:
//!
//! - a versioned **measurement record** covering the field's standard
//!   instruments — activation foils, ion chambers, thermoluminescent
//!   dosimeters, and tissue-equivalent proportional counters (TEPC
//!   lineal-energy spectra) — with explicit units, declared one-sigma
//!   uncertainty, and position provenance;
//! - a **comparison record** that binds a measurement record and a
//!   computed artifact (currently a beam-quality report) by content
//!   reference and reports per-point relative difference, difference in
//!   sigma units, and a chi-square summary over points that carry an
//!   uncertainty.
//!
//! A measurement that does not state an uncertainty (`absolute_
//! uncertainty_1sigma: null`) is compared by relative difference only —
//! the comparison records `passed: null` rather than inventing a sigma.
//! Digitized literature values must say so in provenance; this is
//! characterization evidence, not commissioning data.

use openbnct_core::{ContentReference, ValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::beam::Citation;
use crate::beam_quality::BeamQualityReport;

pub const MEASUREMENT_RECORD_SCHEMA: &str = "openbnct.measurement-record/0.1.0";
pub const MEASUREMENT_COMPARISON_SCHEMA: &str = "openbnct.measurement-comparison/0.1.0";

/// A versioned set of measurements taken on one subject — typically a
/// facility beam — with provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementRecord {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// What was measured — e.g. the content binding of the beam
    /// description this record characterizes. Optional: a record may
    /// describe a facility that has no encoded beam yet.
    pub subject: Option<ContentReference>,
    pub measurements: Vec<Measurement>,
    pub provenance: MeasurementProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    /// Record-local identifier, unique within the record.
    pub id: String,
    /// Canonical metric name. Where the measured quantity is a beam
    /// quality metric, the name matches the `InAirMetrics` /
    /// `InPhantomMetrics` field name (e.g.
    /// `epithermal_fluence_rate_cm2_s`) so comparisons can resolve it.
    pub metric: String,
    pub method: MeasurementMethod,
    pub value: MeasurementValue,
    /// Unit of `value` — required and free-text; canonical metric names
    /// imply their units (`*_cm2_s` → cm⁻² s⁻¹) but the explicit unit is
    /// what the comparison checks.
    pub unit: String,
    /// Where the measurement was taken, if the source states it.
    pub position: Option<MeasurementPosition>,
    pub note: Option<String>,
}

/// The measured quantity: a scalar with optional one-sigma absolute
/// uncertainty, or a binned spectrum (TEPC lineal-energy, spectrometry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeasurementValue {
    Scalar {
        value: f64,
        /// One-sigma absolute uncertainty in `unit`. `null` is honest:
        /// the source did not state one (digitized figure, summary value).
        absolute_uncertainty_1sigma: Option<f64>,
    },
    Histogram {
        /// n+1 bin edges in `unit` (e.g. keV/µm for lineal energy).
        bin_edges: Vec<f64>,
        /// n bin contents; interpretation (counts, dose fraction) is
        /// fixed by `metric` and `unit`.
        bin_values: Vec<f64>,
        /// Optional per-bin one-sigma uncertainties, same contents units.
        bin_uncertainties_1sigma: Option<Vec<f64>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeasurementMethod {
    /// Activation foil or wire; `material` names the foil (e.g. "Au",
    /// "Au+Cd cover") when the source states it.
    ActivationFoil {
        material: Option<String>,
    },
    IonChamber,
    ThermoluminescentDosimeter,
    /// Tissue-equivalent proportional counter — microdosimetric spectra.
    TissueEquivalentProportionalCounter,
    /// Fission chamber / beam monitor.
    FissionChamber,
    /// Anything else; `description` is required so the method is never
    /// silently anonymous.
    Other {
        description: String,
    },
}

/// Where a measurement was taken. `frame` names the coordinate
/// convention (`port_plane`, `phantom_axis`, or a source-described
/// frame); `point_cm` is the position within it and `depth_cm` the
/// along-beam depth for in-phantom points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementPosition {
    pub frame: String,
    pub point_cm: [f64; 3],
    pub depth_cm: Option<f64>,
}

/// Where the measurement numbers came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MeasurementProvenance {
    /// Digitized from a publication — `derivation_note` must state what
    /// was digitized (table vs figure), and whether stated uncertainties
    /// were carried over.
    DigitizedLiterature {
        citations: Vec<Citation>,
        derivation_note: String,
    },
    /// An actual measurement dataset (logbook export, facility QA file).
    Acquired { derivation_note: String },
}

/// One measurement compared against a computed artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementComparison {
    pub measurement_id: String,
    pub metric: String,
    /// `None` for non-scalar measurements that resolve to no metric —
    /// serialized `null`, never dropped.
    pub measured: Option<f64>,
    /// One-sigma absolute uncertainty when the record states one.
    pub uncertainty_1sigma: Option<f64>,
    /// `None` when the computed artifact has no such metric — the
    /// measurement is then reported unmatched, not silently dropped.
    pub computed: Option<f64>,
    /// `|computed − measured| / |measured|`; `None` when unmatched.
    pub relative_difference: Option<f64>,
    /// `|computed − measured| / σ`; `None` without a stated σ.
    pub difference_sigma: Option<f64>,
    /// `Some` only when both a computed value and a stated σ exist:
    /// `difference_sigma <= sigma_tolerance`.
    pub passed: Option<bool>,
}

/// Comparison of a histogram-valued depth-profile measurement against a
/// computed depth profile from a beam-quality report. Both sides are
/// peak-normalized before comparison: published activation profiles are
/// reported in relative units, so this is a shape comparison, which is
/// the convention for foil-scan-versus-calculation validation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileComparison {
    pub measurement_id: String,
    pub metric: String,
    /// `"peak"`: both sides peak-normalized (foil-scan convention).
    /// `"absolute"`: raw values on the record's stated units — used when
    /// the metric carries an absolute scale (e.g. the flux-derived
    /// thermal fluence profile scaled by the declared source rate).
    #[serde(default = "default_peak_normalization")]
    pub normalization: String,
    /// Bin centers in the measurement's edge unit (e.g. cm). Only bins
    /// whose center lies inside the computed profile's depth range are
    /// carried — out-of-range bins are not comparable and are dropped.
    pub bin_centers: Vec<f64>,
    /// Measured bin contents — peak-normalized (max = 1) under
    /// `"peak"`, raw under `"absolute"`.
    pub measured_normalized: Vec<f64>,
    /// Computed profile interpolated at the bin centers — same scale
    /// convention as `measured_normalized`.
    pub computed_normalized: Vec<f64>,
    /// Per-bin |computed − measured| / measured; inf where measured = 0.
    pub relative_differences: Vec<f64>,
    /// Per-bin |computed − measured| / σ on the normalized scale; `None`
    /// when the record states no bin uncertainties.
    pub difference_sigma: Option<Vec<f64>>,
    /// Largest per-bin relative difference across carried bins.
    pub max_relative_difference: f64,
    /// Σ((measured − computed)/σ)² over bins with σ; `None` without σ.
    pub chi_square: Option<f64>,
    /// `Some(true)` when every σ-bearing bin satisfies the sigma
    /// tolerance; `Some(false)` when any fails; `None` without σ.
    pub passed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonSummary {
    /// Measurements that named a metric the computed artifact provides.
    pub compared: usize,
    /// Measurements the artifact could not resolve.
    pub unmatched: usize,
    pub passed: usize,
    pub failed: usize,
    /// Compared points lacking a stated σ — reported, never auto-passed.
    pub without_uncertainty: usize,
    /// Σ((measured − computed)/σ)² over compared points with σ; `None`
    /// when no point carries an uncertainty.
    pub chi_square: Option<f64>,
    /// Degrees of freedom: the number of σ-bearing compared points (no
    /// parameters are fitted here).
    pub degrees_of_freedom: usize,
    /// Histogram-valued measurements resolved to a computed depth
    /// profile; details live in `profile_comparisons`. `0` in records
    /// written before profile comparison existed.
    #[serde(default)]
    pub profiles_compared: usize,
}

/// A versioned comparison record binding a measurement record to the
/// computed artifact it was checked against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementComparisonReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding of the measurement record.
    pub measurements: ContentReference,
    /// Content binding of the computed artifact (e.g. a beam-quality
    /// report JSON).
    pub computed: ContentReference,
    /// The declared pass criterion: |difference_sigma| ≤ this value.
    pub sigma_tolerance: f64,
    pub comparisons: Vec<MeasurementComparison>,
    /// Depth-profile comparisons for histogram-valued measurements.
    /// Empty in records written before profile comparison existed.
    #[serde(default)]
    pub profile_comparisons: Vec<ProfileComparison>,
    pub summary: ComparisonSummary,
}

impl MeasurementRecord {
    pub fn validate(&self) -> Result<(), MeasurementError> {
        if !openbnct_core::schema_matches(&self.schema_version, MEASUREMENT_RECORD_SCHEMA) {
            return Err(MeasurementError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        non_empty("record.id", &self.id)?;
        if self.measurements.is_empty() {
            return Err(MeasurementError::EmptyRecord);
        }
        let mut ids = std::collections::BTreeSet::new();
        for measurement in &self.measurements {
            non_empty("measurement.id", &measurement.id)?;
            if !ids.insert(measurement.id.as_str()) {
                return Err(MeasurementError::DuplicateMeasurementId(
                    measurement.id.clone(),
                ));
            }
            non_empty("measurement.metric", &measurement.metric)?;
            non_empty("measurement.unit", &measurement.unit)?;
            measurement.value.validate(&measurement.id)?;
            if let MeasurementMethod::Other { description } = &measurement.method
                && description.trim().is_empty()
            {
                return Err(MeasurementError::AnonymousMethod(measurement.id.clone()));
            }
            if let Some(position) = &measurement.position {
                non_empty("position.frame", &position.frame)?;
                if !position.point_cm.iter().all(|c| c.is_finite()) {
                    return Err(MeasurementError::NonFinite("position.point_cm"));
                }
            }
        }
        match &self.provenance {
            MeasurementProvenance::DigitizedLiterature {
                citations,
                derivation_note,
            } => {
                if citations.is_empty() {
                    return Err(MeasurementError::InvalidCitation);
                }
                non_empty("provenance.derivation_note", derivation_note)?;
            }
            MeasurementProvenance::Acquired { derivation_note } => {
                non_empty("provenance.derivation_note", derivation_note)?;
            }
        }
        Ok(())
    }
}

impl MeasurementValue {
    fn validate(&self, measurement_id: &str) -> Result<(), MeasurementError> {
        match self {
            MeasurementValue::Scalar {
                value,
                absolute_uncertainty_1sigma,
            } => {
                if !value.is_finite() {
                    return Err(MeasurementError::NonFinite("measurement.value"));
                }
                if let Some(sigma) = absolute_uncertainty_1sigma
                    && (!sigma.is_finite() || *sigma <= 0.0)
                {
                    return Err(MeasurementError::NonPositiveUncertainty(
                        measurement_id.into(),
                    ));
                }
            }
            MeasurementValue::Histogram {
                bin_edges,
                bin_values,
                bin_uncertainties_1sigma,
            } => {
                if bin_edges.len() < 2 || bin_edges.len() != bin_values.len() + 1 {
                    return Err(MeasurementError::MalformedHistogram(measurement_id.into()));
                }
                if !bin_edges.iter().all(|e| e.is_finite())
                    || !bin_edges.windows(2).all(|w| w[1] > w[0])
                {
                    return Err(MeasurementError::MalformedHistogram(measurement_id.into()));
                }
                if !bin_values.iter().all(|v| v.is_finite()) {
                    return Err(MeasurementError::NonFinite("histogram.bin_values"));
                }
                if let Some(sigmas) = bin_uncertainties_1sigma
                    && (sigmas.len() != bin_values.len()
                        || !sigmas.iter().all(|s| s.is_finite() && *s > 0.0))
                {
                    return Err(MeasurementError::MalformedHistogram(measurement_id.into()));
                }
            }
        }
        Ok(())
    }
}

/// Resolve a canonical metric name against a beam-quality report.
pub fn beam_quality_metric(report: &BeamQualityReport, metric: &str) -> Option<f64> {
    let in_air = &report.in_air;
    Some(match metric {
        "thermal_fluence_rate_cm2_s" => in_air.thermal_fluence_rate_cm2_s,
        "epithermal_fluence_rate_cm2_s" => in_air.epithermal_fluence_rate_cm2_s,
        "fast_fluence_rate_cm2_s" => in_air.fast_fluence_rate_cm2_s,
        "total_fluence_rate_cm2_s" => in_air.total_fluence_rate_cm2_s,
        "thermal_fraction" => in_air.thermal_fraction,
        "fast_fraction" => in_air.fast_fraction,
        "current_to_fluence_ratio" => in_air.current_to_fluence_ratio,
        "mean_energy_ev" => in_air.mean_energy_ev,
        "port_area_cm2" => in_air.port_area_cm2,
        "advantage_depth_cm" => report.in_phantom.as_ref()?.advantage_depth_cm,
        "advantage_ratio" => report.in_phantom.as_ref()?.advantage_ratio,
        "peak_therapeutic_ratio" => report.in_phantom.as_ref()?.peak_therapeutic_ratio,
        // Depth of the thermal-fluence maximum: under uniform dilute
        // boron loading the boron-capture dose is proportional to the
        // thermal fluence, so its argmax is the measured thermal peak.
        "thermal_fluence_max_depth_cm" => {
            let in_phantom = report.in_phantom.as_ref()?;
            in_phantom
                .boron_dose_profile
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| in_phantom.depth_cm[i])?
        }
        _ => return None,
    })
}

/// Resolve a depth-profile metric name to the computed (depth_cm,
/// profile values) pair in a beam-quality report. The boron-capture
/// dose profile serves as the thermal-fluence profile proxy: under
/// uniform dilute boron loading it is proportional to the thermal
/// neutron fluence.
pub fn beam_quality_profile<'a>(
    report: &'a BeamQualityReport,
    metric: &str,
) -> Option<(&'a [f64], &'a [f64])> {
    let in_phantom = report.in_phantom.as_ref()?;
    let values = match metric {
        "thermal_fluence_depth_profile" | "boron_dose_depth_profile" => {
            in_phantom.boron_dose_profile.as_slice()
        }
        "tumor_dose_depth_profile" => in_phantom.tumor_dose_profile.as_slice(),
        "normal_tissue_dose_depth_profile" => in_phantom.normal_tissue_dose_profile.as_slice(),
        // Absolute-scale comparison: requires the report to carry the
        // flux-derived absolute profile (`--flux` at `beam qa` time).
        "thermal_fluence_depth_profile_absolute" => in_phantom
            .thermal_fluence_depth_profile_absolute_cm2_s
            .as_deref()?,
        _ => {
            // Transverse scans: `thermal_fluence_transverse_profile_d<mm>`
            // (and `_absolute` suffix) resolves the report's
            // flux-derived transverse profile nearest the declared
            // depth. The lateral axis stands in for depth_cm here.
            let base = metric.strip_suffix("_absolute").unwrap_or(metric);
            let (head, depth_mm) = base.rsplit_once("_d")?;
            if head != "thermal_fluence_transverse_profile" {
                return None;
            }
            let target_cm = depth_mm.parse::<f64>().ok()? / 10.0;
            let profile = in_phantom
                .thermal_fluence_transverse_profiles_cm2_s
                .iter()
                .min_by(|a, b| {
                    (a.depth_cm - target_cm)
                        .abs()
                        .partial_cmp(&(b.depth_cm - target_cm).abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                })?;
            // Only a profile within half a centimetre of the declared
            // depth resolves — the depth coordinate is part of the
            // metric identity, not a fuzzy match.
            if (profile.depth_cm - target_cm).abs() > 0.5 || profile.lateral_cm.is_empty() {
                return None;
            }
            return Some((
                profile.lateral_cm.as_slice(),
                profile.values_cm2_s.as_slice(),
            ));
        }
    };
    if values.is_empty() {
        return None;
    }
    Some((in_phantom.depth_cm.as_slice(), values))
}

/// Whether a profile metric compares on an absolute scale (`true`) or
/// after peak normalization (`false`, the foil-scan convention).
fn profile_is_absolute(metric: &str) -> bool {
    metric.ends_with("_absolute")
}

fn default_peak_normalization() -> String {
    "peak".into()
}

/// Linear interpolation of `(xs, ys)` at `x`; `None` outside the range.
fn interpolate(xs: &[f64], ys: &[f64], x: f64) -> Option<f64> {
    if xs.len() != ys.len() || xs.is_empty() {
        return None;
    }
    if x < xs[0] || x > *xs.last()? {
        return None;
    }
    if x == xs[0] {
        return Some(ys[0]);
    }
    let i = xs.partition_point(|&v| v < x);
    let (x0, x1) = (xs[i - 1], xs[i]);
    let (y0, y1) = (ys[i - 1], ys[i]);
    if x1 == x0 {
        return Some(y0);
    }
    Some(y0 + (y1 - y0) * (x - x0) / (x1 - x0))
}

/// Compare a histogram-valued depth-profile measurement against a
/// computed profile. `None` when the metric resolves to no profile.
/// Both sides are peak-normalized; bins outside the computed depth
/// range are dropped.
fn compare_profile(
    measurement: &Measurement,
    edges: &[f64],
    values: &[f64],
    uncertainties: Option<&[f64]>,
    report: &BeamQualityReport,
    sigma_tolerance: f64,
) -> Option<ProfileComparison> {
    let (depth_cm, computed) = beam_quality_profile(report, &measurement.metric)?;
    let absolute = profile_is_absolute(&measurement.metric);
    let (measured_scale, computed_scale) = if absolute {
        (1.0, 1.0)
    } else {
        (
            values.iter().copied().fold(0.0_f64, f64::max),
            computed.iter().copied().fold(0.0_f64, f64::max),
        )
    };
    if measured_scale <= 0.0 || computed_scale <= 0.0 {
        return None;
    }
    let mut bin_centers = Vec::new();
    let mut measured_normalized = Vec::new();
    let mut computed_normalized = Vec::new();
    let mut relative_differences = Vec::new();
    let mut difference_sigma = uncertainties.map(|_| Vec::new());
    let mut chi_square = 0.0;
    let mut any_sigma = false;
    let mut all_pass = true;
    for bin in 0..values.len() {
        let center = (edges[bin] + edges[bin + 1]) / 2.0;
        let Some(computed_at) = interpolate(depth_cm, computed, center) else {
            continue;
        };
        let m = values[bin] / measured_scale;
        let c = computed_at / computed_scale;
        bin_centers.push(center);
        measured_normalized.push(m);
        computed_normalized.push(c);
        relative_differences.push(if m == 0.0 {
            f64::INFINITY
        } else {
            (c - m).abs() / m
        });
        if let (Some(sigmas), Some(raw)) = (difference_sigma.as_mut(), uncertainties) {
            let sigma = raw[bin] / measured_scale;
            let d = (c - m).abs() / sigma.max(f64::MIN_POSITIVE);
            sigmas.push(d);
            chi_square += d * d;
            any_sigma = true;
            if d > sigma_tolerance {
                all_pass = false;
            }
        }
    }
    if bin_centers.is_empty() {
        return None;
    }
    Some(ProfileComparison {
        measurement_id: measurement.id.clone(),
        metric: measurement.metric.clone(),
        normalization: if absolute { "absolute" } else { "peak" }.into(),
        bin_centers,
        measured_normalized,
        computed_normalized,
        max_relative_difference: relative_differences
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
        relative_differences,
        difference_sigma,
        chi_square: any_sigma.then_some(chi_square),
        passed: any_sigma.then_some(all_pass),
    })
}

/// Compare each scalar measurement in `record` against the beam-quality
/// report. Histogram-valued measurements that resolve to a computed
/// depth profile are compared in `profile_comparisons`; those that do
/// not resolve stay here and surface as unmatched.
pub fn compare_with_beam_quality(
    record: &MeasurementRecord,
    report: &BeamQualityReport,
    sigma_tolerance: f64,
) -> Vec<MeasurementComparison> {
    record
        .measurements
        .iter()
        .filter_map(|measurement| {
            if let MeasurementValue::Histogram { .. } = &measurement.value
                && beam_quality_profile(report, &measurement.metric).is_some()
            {
                // Resolvable depth profile — compared via
                // `compare_profile`, not here.
                return None;
            }
            let (measured, sigma) = match &measurement.value {
                MeasurementValue::Scalar {
                    value,
                    absolute_uncertainty_1sigma,
                } => (Some(*value), *absolute_uncertainty_1sigma),
                // Spectral measurements have no beam-quality metric to
                // resolve against — reported as unmatched (computed: None).
                MeasurementValue::Histogram { .. } => (None, None),
            };
            let computed = if measured.is_none() {
                None
            } else {
                beam_quality_metric(report, &measurement.metric)
            };
            let relative_difference = measured.and_then(|measured| {
                computed.map(|c| {
                    if measured == 0.0 {
                        if c == 0.0 { 0.0 } else { f64::INFINITY }
                    } else {
                        (c - measured).abs() / measured.abs()
                    }
                })
            });
            let difference_sigma = measured.and_then(|measured| {
                computed
                    .zip(sigma)
                    .map(|(c, s)| (c - measured).abs() / s.max(f64::MIN_POSITIVE))
            });
            Some(MeasurementComparison {
                measurement_id: measurement.id.clone(),
                metric: measurement.metric.clone(),
                measured,
                uncertainty_1sigma: sigma,
                computed,
                relative_difference,
                difference_sigma,
                passed: difference_sigma.map(|d| d <= sigma_tolerance),
            })
        })
        .collect()
}

/// Build the versioned comparison record.
pub fn compare_measurement_record(
    report_id: &str,
    record: &MeasurementRecord,
    measurements_reference: ContentReference,
    report: &BeamQualityReport,
    computed_reference: ContentReference,
    sigma_tolerance: f64,
) -> Result<MeasurementComparisonReport, MeasurementError> {
    record.validate()?;
    if !sigma_tolerance.is_finite() || sigma_tolerance <= 0.0 {
        return Err(MeasurementError::InvalidSigmaTolerance);
    }
    let comparisons = compare_with_beam_quality(record, report, sigma_tolerance);
    let mut profile_comparisons: Vec<ProfileComparison> = Vec::new();
    for measurement in &record.measurements {
        let MeasurementValue::Histogram {
            bin_edges,
            bin_values,
            bin_uncertainties_1sigma,
        } = &measurement.value
        else {
            continue;
        };
        if let Some(comparison) = compare_profile(
            measurement,
            bin_edges,
            bin_values,
            bin_uncertainties_1sigma.as_deref(),
            report,
            sigma_tolerance,
        ) {
            profile_comparisons.push(comparison);
        }
        // When the report carries an absolute-scale profile for the
        // same physical quantity (metric + "_absolute"), emit a second
        // comparison without peak normalization — the record's values
        // and σ are already absolute.
        if !profile_is_absolute(&measurement.metric) {
            let mut absolute_measurement = measurement.clone();
            absolute_measurement.metric = format!("{}_absolute", measurement.metric);
            if let Some(comparison) = compare_profile(
                &absolute_measurement,
                bin_edges,
                bin_values,
                bin_uncertainties_1sigma.as_deref(),
                report,
                sigma_tolerance,
            ) {
                profile_comparisons.push(comparison);
            }
        }
    }
    let mut summary = ComparisonSummary {
        compared: 0,
        unmatched: 0,
        passed: 0,
        failed: 0,
        without_uncertainty: 0,
        chi_square: None,
        degrees_of_freedom: 0,
        profiles_compared: profile_comparisons.len(),
    };
    let mut chi_square = 0.0;
    for comparison in &comparisons {
        if comparison.computed.is_none() {
            summary.unmatched += 1;
            continue;
        }
        summary.compared += 1;
        match comparison.passed {
            Some(true) => {
                summary.passed += 1;
                summary.degrees_of_freedom += 1;
                chi_square += comparison.difference_sigma.unwrap_or(0.0).powi(2);
            }
            Some(false) => {
                summary.failed += 1;
                summary.degrees_of_freedom += 1;
                chi_square += comparison.difference_sigma.unwrap_or(0.0).powi(2);
            }
            None => summary.without_uncertainty += 1,
        }
    }
    if summary.degrees_of_freedom > 0 {
        summary.chi_square = Some(chi_square);
    }
    Ok(MeasurementComparisonReport {
        schema_version: MEASUREMENT_COMPARISON_SCHEMA.into(),
        id: report_id.into(),
        measurements: measurements_reference,
        computed: computed_reference,
        sigma_tolerance,
        comparisons,
        profile_comparisons,
        summary,
    })
}

impl MeasurementComparisonReport {
    pub fn validate(&self) -> Result<(), MeasurementError> {
        if !openbnct_core::schema_matches(&self.schema_version, MEASUREMENT_COMPARISON_SCHEMA) {
            return Err(MeasurementError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        non_empty("report.id", &self.id)?;
        Ok(())
    }
}

fn non_empty(field: &'static str, value: &str) -> Result<(), MeasurementError> {
    if value.trim().is_empty() {
        return Err(MeasurementError::Validation(
            ValidationError::EmptyIdentifier(field),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum MeasurementError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("unsupported schema {0:?}")]
    UnsupportedSchema(String),
    #[error("measurement record contains no measurements")]
    EmptyRecord,
    #[error("duplicate measurement id {0:?}")]
    DuplicateMeasurementId(String),
    #[error("non-finite or missing value in {0}")]
    NonFinite(&'static str),
    #[error("uncertainty must be positive and finite in measurement {0:?}")]
    NonPositiveUncertainty(String),
    #[error(
        "malformed histogram in measurement {0:?}: edges must be increasing and match bin count"
    )]
    MalformedHistogram(String),
    #[error("measurement {0:?} uses method 'other' without a description")]
    AnonymousMethod(String),
    #[error("measurement provenance requires at least one citation")]
    InvalidCitation,
    #[error("sigma tolerance must be positive and finite")]
    InvalidSigmaTolerance,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beam::{
        BeamDescription, BeamProvenance, NormalizationBasis, PortGeometry, PortShape,
    };
    use crate::beam_quality::{ComponentWeights, InPhantomMetrics, evaluate_beam_quality};
    use crate::model::{
        AngularDistribution, EnergyDistribution, FixedSourceDefinition, PlaneAxis,
        SourceSpatialDistribution,
    };

    fn beam() -> BeamDescription {
        BeamDescription {
            schema_version: crate::beam::BEAM_DESCRIPTION_SCHEMA.into(),
            id: "b".into(),
            name: "b".into(),
            facility: "f".into(),
            source: FixedSourceDefinition {
                schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
                id: "s".into(),
                particle: crate::model::ParticleType::Neutron,
                source_sites_per_history: 1,
                statistical_weight_per_site: 1.0,
                space: SourceSpatialDistribution::UniformDisk {
                    axis: PlaneAxis::Z,
                    offset_cm: 0.0,
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 7.0,
                },
                angle: AngularDistribution::IsotropicCone {
                    axis_unit_vector: [0.0, 0.0, 1.0],
                    half_angle_rad: 0.1491,
                },
                energy: EnergyDistribution::TabulatedHistogram {
                    energy_boundaries_ev: vec![1e-5, 0.5, 1.0e4, 1.69e7],
                    bin_weights: vec![0.0611, 0.9093, 0.0296],
                },
            },
            port: PortGeometry {
                axis: PlaneAxis::Z,
                offset_cm: 0.0,
                shape: PortShape::Circle {
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 7.0,
                },
            },
            normalization: NormalizationBasis::FluenceRateAtPort {
                fluence_rate_cm2_s: 1.1769e9,
            },
            provenance: BeamProvenance::PublishedLiterature {
                citations: vec![Citation {
                    authors: "a".into(),
                    title: "t".into(),
                    venue: "v".into(),
                    year: 2000,
                    doi: None,
                    url: None,
                }],
                derivation_note: "note".into(),
            },
        }
    }

    fn content(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "sha256:".to_string() + &"ab".repeat(32),
        }
    }

    fn scalar(id: &str, metric: &str, value: f64, sigma: Option<f64>) -> Measurement {
        Measurement {
            id: id.into(),
            metric: metric.into(),
            method: MeasurementMethod::ActivationFoil {
                material: Some("Au".into()),
            },
            value: MeasurementValue::Scalar {
                value,
                absolute_uncertainty_1sigma: sigma,
            },
            unit: "cm^-2 s^-1".into(),
            position: None,
            note: None,
        }
    }

    fn record(measurements: Vec<Measurement>) -> MeasurementRecord {
        MeasurementRecord {
            schema_version: MEASUREMENT_RECORD_SCHEMA.into(),
            id: "rec".into(),
            subject: Some(content("beam")),
            measurements,
            provenance: MeasurementProvenance::DigitizedLiterature {
                citations: vec![Citation {
                    authors: "a".into(),
                    title: "t".into(),
                    venue: "v".into(),
                    year: 2000,
                    doi: None,
                    url: None,
                }],
                derivation_note: "digitized table values".into(),
            },
        }
    }

    fn report() -> BeamQualityReport {
        evaluate_beam_quality("rep", &beam(), content("beam"), None, None).unwrap()
    }

    /// Report with a synthetic in-phantom block: boron-capture profile
    /// peaking at 2 cm depth, standing in for the thermal fluence shape.
    fn phantom_report() -> BeamQualityReport {
        let mut report = report();
        let weights = ComponentWeights {
            boron: 3.8,
            nitrogen: 1.0,
            hydrogen: 1.0,
            photon: 1.0,
        };
        report.in_phantom = Some(InPhantomMetrics {
            tumor_dose_profile: vec![1.0, 2.0, 4.0, 2.4, 1.2, 0.4],
            normal_tissue_dose_profile: vec![2.0, 1.9, 1.6, 1.2, 0.8, 0.5],
            depth_cm: vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
            advantage_depth_cm: 3.0,
            advantage_ratio: 1.5,
            peak_therapeutic_ratio: 2.5,
            boron_dose_profile: vec![0.1, 0.5, 1.0, 0.6, 0.3, 0.1],
            tumor_weights: weights.clone(),
            normal_weights: weights,
            dose: content("dose"),
            thermal_fluence_depth_profile_absolute_cm2_s: None,
            thermal_fluence_transverse_profiles_cm2_s: Vec::new(),
            absolute_fluence_note: None,
        });
        report
    }

    #[test]
    fn comparison_reports_sigma_and_relative_differences() {
        // Epithermal computed = 0.9093 * 1.1769e9.
        let computed_epi = 0.9093 * 1.1769e9;
        let measured = computed_epi * 1.01; // 1% high
        let sigma = computed_epi * 0.02; // 2% sigma → |Δ|/σ ≈ 0.5
        let rec = record(vec![scalar(
            "epi",
            "epithermal_fluence_rate_cm2_s",
            measured,
            Some(sigma),
        )]);
        let comparisons = compare_with_beam_quality(&rec, &report(), 2.0);
        assert_eq!(comparisons.len(), 1);
        let c = &comparisons[0];
        // |c − m|/|m| with m = 1.01·c → 0.01/1.01.
        assert!((c.relative_difference.unwrap() - 0.01 / 1.01).abs() < 1e-9);
        assert!((c.difference_sigma.unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(c.passed, Some(true));
    }

    #[test]
    fn missing_uncertainty_is_reported_not_passed() {
        let rec = record(vec![scalar(
            "epi",
            "epithermal_fluence_rate_cm2_s",
            1.07e9,
            None,
        )]);
        let report =
            compare_measurement_record("cmp", &rec, content("m"), &report(), content("r"), 2.0)
                .unwrap();
        assert_eq!(report.comparisons[0].passed, None);
        assert_eq!(report.comparisons[0].difference_sigma, None);
        assert_eq!(report.summary.without_uncertainty, 1);
        assert_eq!(report.summary.compared, 1);
        assert_eq!(report.summary.chi_square, None);
    }

    #[test]
    fn unmatched_metric_and_failed_sigma() {
        let rec = record(vec![
            scalar("x", "no_such_metric", 1.0, Some(0.1)),
            // J/Phi measured 0.77 vs modeled ~0.9945 at 1σ=0.02 → fails.
            scalar("j", "current_to_fluence_ratio", 0.77, Some(0.02)),
        ]);
        let report =
            compare_measurement_record("cmp", &rec, content("m"), &report(), content("r"), 2.0)
                .unwrap();
        assert_eq!(report.comparisons[0].computed, None);
        assert_eq!(report.summary.unmatched, 1);
        assert_eq!(report.comparisons[1].passed, Some(false));
        assert_eq!(report.summary.failed, 1);
        assert!(report.summary.chi_square.unwrap() > 100.0);
    }

    #[test]
    fn histogram_measurement_is_unmatched_but_valid() {
        let mut rec = record(vec![Measurement {
            id: "tepc".into(),
            metric: "lineal_energy_spectrum".into(),
            method: MeasurementMethod::TissueEquivalentProportionalCounter,
            value: MeasurementValue::Histogram {
                bin_edges: vec![0.1, 1.0, 10.0, 100.0],
                bin_values: vec![10.0, 20.0, 10.0],
                bin_uncertainties_1sigma: Some(vec![1.0, 2.0, 1.0]),
            },
            unit: "keV/um".into(),
            position: Some(MeasurementPosition {
                frame: "port_plane".into(),
                point_cm: [0.0, 0.0, 0.0],
                depth_cm: None,
            }),
            note: None,
        }]);
        rec.validate().unwrap();
        let report =
            compare_measurement_record("cmp", &rec, content("m"), &report(), content("r"), 2.0)
                .unwrap();
        assert_eq!(report.summary.unmatched, 1);
        assert_eq!(report.comparisons[0].measured, None);
        // The unmatched row must round-trip through JSON: NaN serializes
        // as `null`, which only an Option field can read back.
        let json = serde_json::to_string(&report).unwrap();
        let back: MeasurementComparisonReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
        // Malformed bins rejected:
        rec.measurements[0].value = MeasurementValue::Histogram {
            bin_edges: vec![0.1, 1.0, 0.5],
            bin_values: vec![1.0, 2.0],
            bin_uncertainties_1sigma: None,
        };
        assert!(rec.validate().is_err());
    }

    #[test]
    fn depth_profile_histogram_compares_against_boron_profile() {
        // Measured activation profile digitized on 1 cm edges over
        // 0.5..3.5 cm — centers at 1, 2, 3 cm. Values chosen to match
        // the synthetic boron profile's normalized shape exactly.
        let rec = record(vec![Measurement {
            id: "mn-profile".into(),
            metric: "thermal_fluence_depth_profile".into(),
            method: MeasurementMethod::ActivationFoil {
                material: Some("Mn-55".into()),
            },
            value: MeasurementValue::Histogram {
                bin_edges: vec![0.5, 1.5, 2.5, 3.5],
                bin_values: vec![0.5, 1.0, 0.6],
                bin_uncertainties_1sigma: Some(vec![0.05, 0.1, 0.06]),
            },
            unit: "cm".into(),
            position: Some(MeasurementPosition {
                frame: "phantom_axis".into(),
                point_cm: [0.0, 0.0, 0.0],
                depth_cm: None,
            }),
            note: None,
        }]);
        let report = compare_measurement_record(
            "cmp",
            &rec,
            content("m"),
            &phantom_report(),
            content("r"),
            2.0,
        )
        .unwrap();
        // Routed to profile_comparisons, not counted among scalars.
        assert!(report.comparisons.is_empty());
        assert_eq!(report.summary.profiles_compared, 1);
        assert_eq!(report.summary.unmatched, 0);
        let profile = &report.profile_comparisons[0];
        assert_eq!(profile.bin_centers, vec![1.0, 2.0, 3.0]);
        assert_eq!(profile.measured_normalized, vec![0.5, 1.0, 0.6]);
        assert_eq!(profile.computed_normalized, vec![0.5, 1.0, 0.6]);
        assert!(profile.relative_differences.iter().all(|d| d.abs() < 1e-12));
        assert_eq!(profile.max_relative_difference, 0.0);
        assert_eq!(profile.chi_square, Some(0.0));
        assert_eq!(profile.passed, Some(true));
    }

    #[test]
    fn depth_profile_reports_real_differences() {
        // Same bins but measured values shifted low at depth → nonzero
        // chi-square and a real max relative difference.
        let rec = record(vec![Measurement {
            id: "mn-profile".into(),
            metric: "thermal_fluence_depth_profile".into(),
            method: MeasurementMethod::ActivationFoil {
                material: Some("Mn-55".into()),
            },
            value: MeasurementValue::Histogram {
                bin_edges: vec![0.5, 1.5, 2.5, 3.5],
                bin_values: vec![0.25, 1.0, 0.3],
                bin_uncertainties_1sigma: Some(vec![0.025, 0.1, 0.03]),
            },
            unit: "cm".into(),
            position: None,
            note: None,
        }]);
        let report = compare_measurement_record(
            "cmp",
            &rec,
            content("m"),
            &phantom_report(),
            content("r"),
            2.0,
        )
        .unwrap();
        let profile = &report.profile_comparisons[0];
        // Peak-normalized measured: [0.25, 1.0, 0.3] vs computed
        // [0.5, 1.0, 0.6] → per-bin rel diff [1.0, 0.0, 1.0].
        assert!((profile.max_relative_difference - 1.0).abs() < 1e-12);
        // σ = [0.025, 0.1, 0.03] (measured peak is 1.0); diffs
        // [0.25, 0, 0.3] → sigma diffs [10, 0, 10] → chi-square 200,
        // fails at 2σ.
        let chi = profile.chi_square.unwrap();
        assert!((chi - 200.0).abs() < 1e-9);
        assert_eq!(profile.passed, Some(false));
    }

    #[test]
    fn record_validation_rejects_bad_inputs() {
        // Duplicate ids.
        let dup = record(vec![
            scalar("a", "m1", 1.0, None),
            scalar("a", "m2", 2.0, None),
        ]);
        assert!(matches!(
            dup.validate(),
            Err(MeasurementError::DuplicateMeasurementId(_))
        ));
        // Zero sigma.
        let zero = record(vec![scalar("a", "m", 1.0, Some(0.0))]);
        assert!(zero.validate().is_err());
        // Anonymous "other" method.
        let mut anon = record(vec![scalar("a", "m", 1.0, None)]);
        anon.measurements[0].method = MeasurementMethod::Other {
            description: "  ".into(),
        };
        assert!(matches!(
            anon.validate(),
            Err(MeasurementError::AnonymousMethod(_))
        ));
        // Empty record.
        assert!(matches!(
            record(vec![]).validate(),
            Err(MeasurementError::EmptyRecord)
        ));
        // Non-positive sigma tolerance.
        let rec = record(vec![scalar(
            "a",
            "total_fluence_rate_cm2_s",
            1.0,
            Some(0.1),
        )]);
        assert!(matches!(
            compare_measurement_record("c", &rec, content("m"), &report(), content("r"), 0.0),
            Err(MeasurementError::InvalidSigmaTolerance)
        ));
    }

    #[test]
    fn comparison_report_round_trips_and_validates() {
        let rec = record(vec![scalar(
            "tot",
            "total_fluence_rate_cm2_s",
            1.1769e9,
            Some(1.0e7),
        )]);
        let report =
            compare_measurement_record("cmp", &rec, content("m"), &report(), content("r"), 2.0)
                .unwrap();
        report.validate().unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let back: MeasurementComparisonReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back, report);
        assert_eq!(back.summary.passed, 1);
    }
}
