// SPDX-License-Identifier: MIT

//! Retrospective accumulated-dose reconstruction
//! (`openbnct.dose-replay/0.1.0` spec, `openbnct.dose-replay-report/0.1.0`
//! report).
//!
//! A replay consumes a validated `openbnct.delivery-history/0.1.0` plus a
//! per-beam physical dose bundle (rate maps in
//! `gray_per_source_particle` at a declared reference output) and a
//! declared concentration history, and reconstructs the accumulated
//! dose the delivered irradiation actually deposited:
//!
//! - non-boron components: `rate_v · S·∫ (output(t)/R0) dt` over
//!   beam-on intervals;
//! - boron: `rate_v · S·∫ (output(t)/R0)·(conc(t)/C_ref) dt`, where the
//!   product is integrated *jointly* over time — the product of two
//!   averages is not the integral of the product;
//! - unobserved intervals are reported as gaps and never contribute
//!   dose implicitly.
//!
//! Time-dependent biological interpretation is not reconstructed here:
//! repair-capable models (IsoE protraction factors) assume constant-rate
//! delivery and are *not* applied to arbitrary interrupted histories.
//! The report marks biological equivalence unavailable; the emitted
//! reconstructed bundle is physical dose only.

use std::collections::BTreeMap;

use openbnct_core::{
    ContentReference, DoseComponent, DoseUnit, PhysicalDoseBundle, PhysicalTotalDoseVolume,
    TotalUncertaintyMethod, deserialize_contract_id, schema_matches,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ManifestError;
use crate::delivery_history::{DeliveryHistory, HistorySample, QualityFlag};

/// Spec contract token.
pub const REPLAY_SPEC_SCHEMA: &str = "openbnct.dose-replay/0.1.0";
/// Report contract token.
pub const REPLAY_REPORT_SCHEMA: &str = "openbnct.dose-replay-report/0.1.0";

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("unsupported schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid replay: {0}")]
    Invalid(String),
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error(transparent)]
    History(#[from] crate::delivery_history::HistoryError),
}

/// How pointwise concentration samples extend over session time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcentrationRule {
    /// Piecewise-linear between samples. Outside the sampled range,
    /// `extrapolation` decides: only `hold_last`/`hold_first` are
    /// supported (declared), never a silent zero.
    Linear,
    /// Each sample holds until the next (patient-level step model).
    Step,
}

/// A beam's recorded output mapped onto a declared dose-rate bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayBeam {
    /// Beam id in the delivery history.
    pub beam: String,
    /// Content-bound `openbnct.physical-dose-bundle/0.2.0` of per-voxel
    /// dose rates (`gray_per_source_particle` × source strength gives
    /// Gy/s at the reference output).
    pub dose_bundle: ContentReference,
    /// Rate-bearing stream id (`interval_average_rate`,
    /// `integrated_counts`, or `cumulative_counter`) in history units.
    pub output_stream: String,
    /// Optional `beam_state` stream id gating delivery: only `on`
    /// intervals deliver dose. Absent = the whole observed output
    /// history delivers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam_state_stream: Option<String>,
    /// Declared calibrated source-output mapping:
    /// `dose_rate = bundle_rate · source_strength · output(t) /
    /// reference_output`. The reference output must be in the same
    /// units as the stream's values; a monitor reading is not
    /// inherently fluence — the mapping's validity is the caller's
    /// declared calibration responsibility, bounded by the beam's
    /// calibration validity window.
    pub reference_output: f64,
    /// Source strength (source particles/s at reference output).
    pub source_strength_per_s: f64,
}

/// Concentration history for boron integration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcentrationHistory {
    /// Explicit `(t_session_s, concentration)` samples, sorted —
    /// typically transcribed from an assay stream with its draw times.
    pub points: Vec<(f64, f64)>,
    pub rule: ConcentrationRule,
    /// `µg/g` or the bundle's declared concentration unit.
    pub unit: String,
    /// Reference concentration `C_ref` the dose-bundle boron rate is
    /// quoted at (the plan concentration).
    pub reference_concentration: f64,
    /// Concentration outside `[t_first, t_last]` holds the boundary
    /// value — a declared convention, surfaced in assumptions.
    #[serde(default)]
    pub hold_boundary: bool,
}

/// The replay spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySpec {
    #[serde(deserialize_with = "deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    /// Content-bound delivery history consumed.
    pub history: ContentReference,
    pub beams: Vec<ReplayBeam>,
    pub concentration: ConcentrationHistory,
    /// Optional planned dose bundle for planned-vs-reconstructed deltas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned: Option<ContentReference>,
}

/// One beam's reconstructed integration record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeamReplay {
    pub beam: String,
    /// Beam-on session seconds actually integrated.
    pub delivered_seconds: f64,
    /// `∫ output(t) dt` in stream units·s over delivered intervals.
    pub integrated_output: f64,
    /// `∫ output(t)·conc(t)/C_ref dt` in stream units·s.
    pub integrated_output_conc_ratio: f64,
    /// Unobserved spans inside the beam's delivery window.
    pub gaps: Vec<(f64, f64)>,
    /// Intervals marked suspect (kept, but flagged).
    pub suspect_intervals: Vec<(f64, f64)>,
    /// Delivered intervals flagged outside the beam's calibration
    /// validity window.
    pub expired_calibration_intervals: Vec<(f64, f64)>,
}

/// Planned-vs-reconstructed delta when `--planned` was supplied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayDelta {
    pub metric: String,
    pub planned: f64,
    pub reconstructed: f64,
    pub delta: f64,
    pub relative: f64,
}

/// The reconstruction report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    #[serde(deserialize_with = "deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    pub history: ContentReference,
    /// Content binding of the emitted reconstructed bundle.
    pub reconstructed: ContentReference,
    /// Session coverage `[t_start, t_end]` the report spans.
    pub coverage: (f64, f64),
    pub beams: Vec<BeamReplay>,
    /// Physical dose totals (Gy) per component over all beams.
    pub component_dose_gray: BTreeMap<String, f64>,
    pub total_dose_gray: f64,
    /// `None` when no planned bundle was bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deltas: Option<Vec<ReplayDelta>>,
    /// `true` only when every beam's delivered interval is fully
    /// covered by its output stream — any reported gap makes this
    /// `false`, so partial coverage is never presented as complete.
    pub coverage_complete: bool,
    /// Always true in this package version — repair-capable biological
    /// models require constant-rate protraction and are not applied to
    /// recorded interrupted histories.
    pub biological_equivalence_available: bool,
    pub assumptions: Vec<String>,
}

/// Piecewise concentration value at session time `t`.
fn concentration_at(conc: &ConcentrationHistory, t: f64) -> Option<f64> {
    let pts = &conc.points;
    if pts.is_empty() {
        return None;
    }
    if t < pts[0].0 {
        return conc.hold_boundary.then_some(pts[0].1);
    }
    let last = pts.last().unwrap();
    if t > last.0 {
        return conc.hold_boundary.then_some(last.1);
    }
    if t == last.0 {
        return Some(last.1);
    }
    let idx = pts.partition_point(|(tp, _)| *tp <= t) - 1;
    let (t0, c0) = pts[idx];
    match conc.rule {
        ConcentrationRule::Step => Some(c0),
        ConcentrationRule::Linear => {
            let (t1, c1) = pts[idx + 1];
            Some(c0 + (c1 - c0) * (t - t0) / (t1 - t0))
        }
    }
}

/// `∫_a^b conc(t) dt` under the declared rule — exact for piecewise
/// linear/step concentration, splitting at every sample inside `[a,b)`.
fn integrate_concentration(conc: &ConcentrationHistory, a: f64, b: f64) -> Option<f64> {
    if a.partial_cmp(&b) != Some(std::cmp::Ordering::Less) {
        return Some(0.0);
    }
    let pts = &conc.points;
    if pts.is_empty() {
        return None;
    }
    // Collect breakpoints: a, interior samples, b.
    let mut edges: Vec<f64> = vec![a];
    for (tp, _) in pts {
        if *tp > a && *tp < b {
            edges.push(*tp);
        }
    }
    edges.push(b);
    edges.sort_by(|x, y| x.total_cmp(y));
    edges.dedup();
    let mut sum = 0.0;
    for w in edges.windows(2) {
        let (ta, tb) = (w[0], w[1]);
        match conc.rule {
            ConcentrationRule::Step => {
                sum += concentration_at(conc, ta)? * (tb - ta);
            }
            ConcentrationRule::Linear => {
                let (ca, cb) = (concentration_at(conc, ta)?, concentration_at(conc, tb)?);
                sum += 0.5 * (ca + cb) * (tb - ta);
            }
        }
    }
    Some(sum)
}

/// Intersect rate intervals with beam-on intervals; `None` on-stream
/// means everything delivers.
fn delivered_intervals(
    history: &DeliveryHistory,
    beam: &ReplayBeam,
) -> Result<Vec<crate::delivery_history::RateInterval>, ReplayError> {
    let mut rates = history.interval_rates(&beam.output_stream)?;
    if let Some(state_stream) = &beam.beam_state_stream {
        let on = history.beam_on_intervals(state_stream)?;
        let mut clipped = Vec::new();
        for r in &rates {
            for (a, b) in &on {
                let start = r.t_start_s.max(*a);
                let end = b.map_or(r.t_end_s, |e| r.t_end_s.min(e));
                if start < end {
                    clipped.push(crate::delivery_history::RateInterval {
                        t_start_s: start,
                        t_end_s: end,
                        rate: r.rate,
                        quality: r.quality,
                    });
                }
            }
        }
        rates = clipped;
    }
    Ok(rates)
}

/// Reconstruct one beam's accumulated dose maps. Returns the scaled
/// per-component voxel values plus the integration record.
///
/// `bundle` must be a `gray_per_source_particle` rate map; the
/// reconstructed value per voxel is
/// `rate_v · source_strength · ∫(output/R0) dt` (non-boron) or
/// `rate_v · source_strength · ∫(output/R0)(conc/C_ref) dt` (boron).
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_beam(
    history: &DeliveryHistory,
    beam: &ReplayBeam,
    bundle: &PhysicalDoseBundle,
    conc: &ConcentrationHistory,
) -> Result<(Vec<Vec<f64>>, BeamReplay), ReplayError> {
    if beam.reference_output <= 0.0 || !beam.reference_output.is_finite() {
        return Err(ReplayError::Invalid(format!(
            "beam {:?}: reference_output must be positive and finite",
            beam.beam
        )));
    }
    if beam.source_strength_per_s <= 0.0 || !beam.source_strength_per_s.is_finite() {
        return Err(ReplayError::Invalid(format!(
            "beam {:?}: source_strength_per_s must be positive and finite",
            beam.beam
        )));
    }
    for v in &bundle.components {
        if v.unit != DoseUnit::GrayPerSourceParticle {
            return Err(ReplayError::Invalid(format!(
                "beam {:?}: component {:?} unit {:?} is not gray_per_source_particle",
                beam.beam, v.component, v.unit
            )));
        }
    }
    let history_beam = history
        .beams
        .iter()
        .find(|b| b.id == beam.beam)
        .ok_or_else(|| {
            ReplayError::Invalid(format!("beam {:?} not declared in the history", beam.beam))
        })?;
    let intervals = delivered_intervals(history, beam)?;
    let mut integrated_output = 0.0;
    let mut integrated_output_conc = 0.0;
    let mut delivered = 0.0;
    let mut suspect = Vec::new();
    let mut expired = Vec::new();
    let mut observed_edges: Vec<(f64, f64)> = Vec::new();
    for r in &intervals {
        let dt = r.t_end_s - r.t_start_s;
        delivered += dt;
        integrated_output += r.rate * dt;
        // Joint integral: split the interval at concentration
        // breakpoints so rate·∫conc is exact per segment.
        let iconc = integrate_concentration(conc, r.t_start_s, r.t_end_s).ok_or_else(|| {
            ReplayError::Invalid(format!(
                "beam {:?}: concentration undefined on [{}, {}) — declare hold_boundary or extend the samples",
                beam.beam, r.t_start_s, r.t_end_s
            ))
        })?;
        integrated_output_conc += r.rate * iconc / conc.reference_concentration;
        observed_edges.push((r.t_start_s, r.t_end_s));
        if r.quality == QualityFlag::Suspect {
            suspect.push((r.t_start_s, r.t_end_s));
        }
        if r.t_start_s < history_beam.calibration_valid.0
            || r.t_end_s > history_beam.calibration_valid.1
        {
            expired.push((r.t_start_s, r.t_end_s));
        }
    }
    // Gaps = beam-on spans with no output observation.
    let on_spans: Vec<(f64, f64)> = if let Some(state_stream) = &beam.beam_state_stream {
        history
            .beam_on_intervals(state_stream)?
            .iter()
            .map(|(a, b)| (*a, b.unwrap_or(f64::INFINITY)))
            .collect()
    } else {
        observed_edges.iter().map(|(a, b)| (*a, *b)).collect()
    };
    let mut gaps = Vec::new();
    for (a, b) in &on_spans {
        let mut cursor = *a;
        for (ca, cb) in &observed_edges {
            // Only edges intersecting this span can fill it.
            if cb <= a || ca >= b {
                continue;
            }
            let (ca, cb) = (ca.max(*a), cb.min(*b));
            if ca > cursor {
                gaps.push((cursor, ca));
            }
            cursor = cursor.max(cb);
        }
        if cursor < *b && b.is_finite() {
            gaps.push((cursor, *b));
        }
    }

    let output_scale = beam.source_strength_per_s / beam.reference_output;
    let values: Vec<Vec<f64>> = bundle
        .components
        .iter()
        .map(|v| {
            let time_integral = if v.component == DoseComponent::Boron {
                integrated_output_conc
            } else {
                integrated_output
            };
            v.values
                .iter()
                .map(|rate| rate * output_scale * time_integral)
                .collect()
        })
        .collect();
    Ok((
        values,
        BeamReplay {
            beam: beam.beam.clone(),
            delivered_seconds: delivered,
            integrated_output,
            integrated_output_conc_ratio: integrated_output_conc,
            gaps,
            suspect_intervals: suspect,
            expired_calibration_intervals: expired,
        },
    ))
}

/// Run a full replay: per-beam reconstruction, summed component maps,
/// emitted as a reconstructed `openbnct.physical-dose-bundle/0.2.0`
/// (absolute Gray) plus a report. `bundles` aligns with `spec.beams`.
pub fn run_replay(
    spec: &ReplaySpec,
    history: &DeliveryHistory,
    bundles: &[&PhysicalDoseBundle],
    reconstructed: ContentReference,
    planned: Option<(&PhysicalDoseBundle, f64)>,
) -> Result<(PhysicalDoseBundle, ReplayReport), ReplayError> {
    if !schema_matches(&spec.schema_version, REPLAY_SPEC_SCHEMA) {
        return Err(ReplayError::UnsupportedSchema(spec.schema_version.clone()));
    }
    if spec.beams.len() != bundles.len() {
        return Err(ReplayError::Invalid(format!(
            "spec declares {} beams but {} dose bundles were supplied",
            spec.beams.len(),
            bundles.len()
        )));
    }
    if spec.beams.is_empty() {
        return Err(ReplayError::Invalid("at least one beam is required".into()));
    }
    if spec.concentration.points.is_empty() {
        return Err(ReplayError::Invalid(
            "concentration history needs at least one declared point".into(),
        ));
    }
    if spec.concentration.reference_concentration <= 0.0 {
        return Err(ReplayError::Invalid(
            "concentration.reference_concentration must be positive".into(),
        ));
    }
    history.validate()?;

    let template = bundles[0];
    let voxel_count = template
        .components
        .first()
        .map(|v| v.values.len())
        .unwrap_or(0);
    let mut accum: BTreeMap<DoseComponent, Vec<f64>> = BTreeMap::new();
    let mut records = Vec::new();
    let mut coverage = (f64::INFINITY, f64::NEG_INFINITY);
    for (beam, bundle) in spec.beams.iter().zip(bundles.iter()) {
        if bundle.geometry != template.geometry {
            return Err(ReplayError::Invalid(format!(
                "beam {:?}: dose-bundle geometry differs between beams — resampled accumulation is out of scope",
                beam.beam
            )));
        }
        let (values, record) = reconstruct_beam(history, beam, bundle, &spec.concentration)?;
        for (vol, acc) in bundle.components.iter().zip(values.iter()) {
            accum
                .entry(vol.component)
                .or_insert_with(|| vec![0.0; voxel_count])
                .iter_mut()
                .zip(acc.iter())
                .for_each(|(a, x)| *a += x);
        }
        if let Some((a, b)) = session_range(history, beam) {
            coverage.0 = coverage.0.min(a);
            coverage.1 = coverage.1.max(b);
        }
        records.push(record);
    }
    if !coverage.0.is_finite() || !coverage.1.is_finite() {
        coverage = (0.0, 0.0);
    }

    // Emit the reconstructed bundle in absolute Gray on the template
    // geometry, provenance rebound to the replay spec.
    let components: Vec<_> = template
        .components
        .iter()
        .map(|v| openbnct_core::DoseVolume {
            component: v.component,
            unit: DoseUnit::Gray,
            values: accum
                .get(&v.component)
                .cloned()
                .unwrap_or_else(|| vec![0.0; voxel_count]),
            absolute_standard_uncertainty: None,
        })
        .collect();
    let totals: Vec<f64> = (0..voxel_count)
        .map(|i| components.iter().map(|c| c.values[i]).sum())
        .collect();
    let out_bundle = PhysicalDoseBundle {
        schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: template.case_id.clone(),
        frame_of_reference_uid: template.frame_of_reference_uid.clone(),
        geometry: template.geometry.clone(),
        component_profile: template.component_profile.clone(),
        response_set: template.response_set.clone(),
        components,
        physical_total: PhysicalTotalDoseVolume {
            unit: DoseUnit::Gray,
            values: totals.clone(),
            absolute_standard_uncertainty: None,
            uncertainty_method: TotalUncertaintyMethod::Unavailable,
        },
        provenance_id: format!("{}:replay:{}", spec.provenance_id, spec.id),
    };
    out_bundle
        .validate()
        .map_err(|e| ReplayError::Invalid(format!("reconstructed bundle: {e}")))?;

    let mut component_totals: BTreeMap<String, f64> = BTreeMap::new();
    for c in &out_bundle.components {
        component_totals.insert(
            format!("{:?}", c.component).to_lowercase(),
            c.values.iter().sum(),
        );
    }
    let deltas = planned.map(|(pb, _)| {
        vec![ReplayDelta {
            metric: "total_voxel_sum_gray".into(),
            planned: pb.components.iter().flat_map(|v| v.values.iter()).sum(),
            reconstructed: totals.iter().sum(),
            delta: totals.iter().sum::<f64>()
                - pb.components
                    .iter()
                    .flat_map(|v| v.values.iter())
                    .sum::<f64>(),
            relative: {
                let p: f64 = pb.components.iter().flat_map(|v| v.values.iter()).sum();
                if p != 0.0 {
                    (totals.iter().sum::<f64>() - p) / p
                } else {
                    f64::NAN
                }
            },
        }]
    });
    let coverage_complete = records.iter().all(|b| b.gaps.is_empty());
    let report = ReplayReport {
        schema_version: REPLAY_REPORT_SCHEMA.into(),
        id: spec.id.clone(),
        qualification: spec.qualification.clone(),
        provenance_id: spec.provenance_id.clone(),
        history: spec.history.clone(),
        reconstructed,
        coverage,
        beams: records,
        component_dose_gray: component_totals,
        total_dose_gray: totals.iter().sum(),
        deltas,
        coverage_complete,
        biological_equivalence_available: false,
        assumptions: vec![
            "monitor/output readings are normalized through the declared calibrated reference_output mapping — a monitor reading is not inherently fluence".into(),
            "boron dose integrates output(t)·conc(t) jointly over each delivered interval; other components integrate output(t)".into(),
            "unobserved spans contribute no dose and are reported as gaps".into(),
            "time-dependent biological interpretation is not reconstructed: repair-capable models assume constant-rate protraction and are not applied to interrupted recorded histories".into(),
            if spec.concentration.hold_boundary {
                "concentration outside the sampled range holds the boundary value (declared)".into()
            } else {
                "concentration outside the sampled range is undefined — such intervals fail rather than extrapolate".into()
            },
        ],
    };
    Ok((out_bundle, report))
}

/// Observed coverage of one beam's delivered intervals. Returns `None`
/// when nothing was delivered.
fn observed_range(history: &DeliveryHistory, beam: &ReplayBeam) -> Option<(f64, f64)> {
    let intervals = delivered_intervals(history, beam).ok()?;
    let start = intervals
        .iter()
        .map(|r| r.t_start_s)
        .fold(f64::INFINITY, f64::min);
    let end = intervals
        .iter()
        .map(|r| r.t_end_s)
        .fold(f64::NEG_INFINITY, f64::max);
    (start.is_finite()).then_some((start, end))
}

/// Session window the report is responsible for: beam-on intervals
/// (when a state stream is declared) union delivered intervals. A
/// beam-on window with no output observation still extends coverage —
/// the report can't hide it — and it appears in `gaps`. An open `On`
/// tail is capped at the state stream's last sample time, the
/// observability bound.
fn session_range(history: &DeliveryHistory, beam: &ReplayBeam) -> Option<(f64, f64)> {
    let delivered = observed_range(history, beam);
    let responsibility = match &beam.beam_state_stream {
        Some(stream_id) => {
            let on = history.beam_on_intervals(stream_id).ok()?;
            if on.is_empty() {
                delivered
            } else {
                let last_state_time = history
                    .streams
                    .iter()
                    .find(|s| s.id == *stream_id)
                    .and_then(|s| {
                        s.samples
                            .iter()
                            .filter_map(|p| match p {
                                HistorySample::State { time_s, .. } => {
                                    Some(s.clock.to_session(*time_s))
                                }
                                _ => None,
                            })
                            .next_back()
                    });
                let start = on.iter().map(|(a, _)| *a).fold(f64::INFINITY, f64::min);
                let end = on
                    .iter()
                    .map(|(_, b)| b.or(last_state_time).unwrap_or(f64::NEG_INFINITY))
                    .fold(f64::NEG_INFINITY, f64::max);
                Some((start, end))
            }
        }
        None => delivered,
    };
    match (delivered, responsibility) {
        (Some((a1, b1)), Some((a2, b2))) => Some((a1.min(a2), b1.max(b2))),
        (a, None) | (None, a) => a,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delivery_history::*;

    fn clock() -> ClockBasis {
        ClockBasis {
            name: "accel".into(),
            offset_seconds: 0.0,
            drift_ppm: None,
            timing_uncertainty_seconds: 0.01,
            epoch: None,
        }
    }

    fn history_with(streams: Vec<HistoryStream>) -> DeliveryHistory {
        DeliveryHistory {
            schema_version: DELIVERY_HISTORY_SCHEMA.into(),
            id: "h".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "test".into(),
            session_id: "s".into(),
            coordinate_frame: "f".into(),
            beams: vec![BeamReference {
                id: "b1".into(),
                beam: ContentReference {
                    id: "beam".into(),
                    sha256: "a".repeat(64),
                },
                calibration: ContentReference {
                    id: "cal".into(),
                    sha256: "b".repeat(64),
                },
                calibration_valid: (0.0, 10_000.0),
            }],
            streams,
        }
    }

    fn bundle(boron_rate: f64, other_rate: f64) -> PhysicalDoseBundle {
        use openbnct_core::*;
        PhysicalDoseBundle {
            schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "rate-map".into(),
            frame_of_reference_uid: None,
            geometry: GridGeometry {
                shape: [1, 1, 1],
                spacing_mm: [10.0; 3],
                origin_mm: [0.0; 3],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            component_profile: ComponentProfileReference {
                id: "openbnct.test-profile.v1".into(),
                sha256: "f".repeat(64),
            },
            response_set: ContentReference {
                id: "openbnct.test-response.v1".into(),
                sha256: "9".repeat(64),
            },
            components: [
                (DoseComponent::Boron, boron_rate),
                (DoseComponent::Hydrogen, other_rate),
                (DoseComponent::Nitrogen, other_rate),
                (DoseComponent::Photon, other_rate),
            ]
            .into_iter()
            .map(|(component, rate)| DoseVolume {
                component,
                unit: DoseUnit::GrayPerSourceParticle,
                values: vec![rate],
                absolute_standard_uncertainty: None,
            })
            .collect(),
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values: vec![boron_rate + other_rate],
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::Unavailable,
            },
            provenance_id: "test".into(),
        }
    }

    fn spec(conc: ConcentrationHistory) -> ReplaySpec {
        ReplaySpec {
            schema_version: REPLAY_SPEC_SCHEMA.into(),
            id: "r1".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "test".into(),
            history: ContentReference {
                id: "h".into(),
                sha256: "c".repeat(64),
            },
            beams: vec![ReplayBeam {
                beam: "b1".into(),
                dose_bundle: ContentReference {
                    id: "rate-map".into(),
                    sha256: "d".repeat(64),
                },
                output_stream: "out".into(),
                beam_state_stream: Some("state".into()),
                reference_output: 100.0,
                source_strength_per_s: 1e9,
            }],
            concentration: conc,
            planned: None,
        }
    }

    #[test]
    fn boron_integrates_jointly_not_as_average_product() {
        // Output: 100 units for [0,100), 50 for [100,200).
        // Conc: linear 10 → 30 µg/g over [0,200], C_ref = 20.
        // ∫output dt = 100·100 + 50·100 = 15000
        // ∫output·conc/C dt = (100·∫₀¹⁰⁰ conc/20) + (50·∫₁₀₀²⁰⁰ conc/20)
        //   conc(t)=10+0.1t; ∫₀¹⁰⁰ conc = 1500; ∫₁₀₀²⁰⁰ conc = 2500
        //   = 100·75 + 50·125 = 7500+6250 = 13750
        // avg(output)·avg(conc)·T would give 75·20/20·200 = 15000 ≠ 13750
        let history = history_with(vec![
            HistoryStream {
                id: "out".into(),
                kind: StreamKind::IntervalAverageRate,
                unit: "monitor-units".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![
                    HistorySample::IntervalAverage {
                        t_start_s: 0.0,
                        t_end_s: 100.0,
                        rate: 100.0,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::IntervalAverage {
                        t_start_s: 100.0,
                        t_end_s: 200.0,
                        rate: 50.0,
                        quality: QualityFlag::Good,
                    },
                ],
            },
            HistoryStream {
                id: "state".into(),
                kind: StreamKind::BeamState,
                unit: "1".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![
                    HistorySample::State {
                        time_s: 0.0,
                        state: BeamState::On,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::State {
                        time_s: 200.0,
                        state: BeamState::Off,
                        quality: QualityFlag::Good,
                    },
                ],
            },
        ]);
        let conc = ConcentrationHistory {
            points: vec![(0.0, 10.0), (200.0, 30.0)],
            rule: ConcentrationRule::Linear,
            unit: "ug/g".into(),
            reference_concentration: 20.0,
            hold_boundary: false,
        };
        let s = spec(conc);
        let (recon, report) = run_replay(
            &s,
            &history,
            &[&bundle(2e-12, 1e-12)],
            ContentReference {
                id: "r".into(),
                sha256: "e".repeat(64),
            },
            None,
        )
        .unwrap();
        let br = &report.beams[0];
        assert!((br.integrated_output - 15000.0).abs() < 1e-9);
        assert!((br.integrated_output_conc_ratio - 13750.0).abs() < 1e-9);
        // Photon: 1e-12 · 1e9 · (15000/100) = 1e-12·1e9·150 = 1.5e-1? wait
        // output_scale = 1e9/100 = 1e7; photon = 1e-12·1e7·15000 = 0.15 Gy? no:
        // rate_v·output_scale·integral = 1e-12 · 1e7 · 15000 = 1.5e-1? — 1e-12·1e7=1e-5; ·15000=0.15 Gy per voxel.
        let photon = recon
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Photon)
            .unwrap();
        assert!((photon.values[0] - 0.15).abs() < 1e-9);
        let boron = recon
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Boron)
            .unwrap();
        // 2e-12 · 1e7 · 13750 = 0.275 Gy — not 2e-12·1e7·15000=0.30.
        assert!((boron.values[0] - 0.275).abs() < 1e-9);
    }

    #[test]
    fn interruptions_and_gaps_are_visible() {
        // Beam on [0,60), hold [60,80), on [80,140); output observed
        // only [0,40) and [80,140) — the [0,20) sub-gap? No: observed
        // [0,40) covers part of on [0,60) → gap [40,60) inside the on
        // window is reported; the hold [60,80) delivers nothing.
        let history = history_with(vec![
            HistoryStream {
                id: "out".into(),
                kind: StreamKind::IntervalAverageRate,
                unit: "u".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![
                    HistorySample::IntervalAverage {
                        t_start_s: 0.0,
                        t_end_s: 40.0,
                        rate: 10.0,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::IntervalAverage {
                        t_start_s: 80.0,
                        t_end_s: 140.0,
                        rate: 10.0,
                        quality: QualityFlag::Good,
                    },
                ],
            },
            HistoryStream {
                id: "state".into(),
                kind: StreamKind::BeamState,
                unit: "1".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![
                    HistorySample::State {
                        time_s: 0.0,
                        state: BeamState::On,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::State {
                        time_s: 60.0,
                        state: BeamState::Hold,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::State {
                        time_s: 80.0,
                        state: BeamState::On,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::State {
                        time_s: 140.0,
                        state: BeamState::Off,
                        quality: QualityFlag::Good,
                    },
                ],
            },
        ]);
        let conc = ConcentrationHistory {
            points: vec![(0.0, 20.0)],
            rule: ConcentrationRule::Linear,
            unit: "ug/g".into(),
            reference_concentration: 20.0,
            hold_boundary: true,
        };
        let s = spec(conc);
        let (_, report) = run_replay(
            &s,
            &history,
            &[&bundle(0.0, 1e-12)],
            ContentReference {
                id: "r".into(),
                sha256: "e".repeat(64),
            },
            None,
        )
        .unwrap();
        let br = &report.beams[0];
        // Delivered: [0,40) + [80,140) = 100 s; hold delivers nothing.
        assert!((br.delivered_seconds - 100.0).abs() < 1e-9);
        // Gap inside the on window: [40,60).
        assert_eq!(br.gaps, vec![(40.0, 60.0)]);
        assert!(!report.coverage_complete);
        // Photon dose: 1e-12·(1e9/100)·(10·40 + 10·60) = 1e-2 Gy.
        assert!((report.component_dose_gray["photon"] - 0.01).abs() < 1e-12);
        assert!(!report.biological_equivalence_available);
    }

    fn output_history(samples: Vec<HistorySample>) -> Vec<HistoryStream> {
        vec![
            HistoryStream {
                id: "out".into(),
                kind: StreamKind::IntervalAverageRate,
                unit: "monitor-units".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples,
            },
            HistoryStream {
                id: "state".into(),
                kind: StreamKind::BeamState,
                unit: "1".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![
                    HistorySample::State {
                        time_s: 0.0,
                        state: BeamState::On,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::State {
                        time_s: 10_000.0,
                        state: BeamState::Off,
                        quality: QualityFlag::Good,
                    },
                ],
            },
        ]
    }

    #[test]
    fn constant_output_and_concentration_reproduce_the_static_result() {
        // Constant output 100 for [0,200) at C = C_ref: reconstructed
        // dose must equal rate·scale·(output·T) — the static answer.
        let rate_sample = |a: f64, b: f64| HistorySample::IntervalAverage {
            t_start_s: a,
            t_end_s: b,
            rate: 100.0,
            quality: QualityFlag::Good,
        };
        let history = history_with(output_history(vec![rate_sample(0.0, 200.0)]));
        let conc = ConcentrationHistory {
            points: vec![(0.0, 20.0)],
            rule: ConcentrationRule::Step,
            unit: "ug/g".into(),
            reference_concentration: 20.0,
            hold_boundary: true,
        };
        let s = spec(conc);
        let (recon, _) = run_replay(
            &s,
            &history,
            &[&bundle(2e-12, 1e-12)],
            ContentReference {
                id: "r".into(),
                sha256: "e".repeat(64),
            },
            None,
        )
        .unwrap();
        // photon: 1e-12 · (1e9/100) · (100·200) = 0.2 Gy — the static dose.
        let photon = recon
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Photon)
            .unwrap();
        assert!((photon.values[0] - 0.2).abs() < 1e-9);
        // boron at C_ref: 2e-12 · 1e7 · 20000 = 0.4 Gy.
        let boron = recon
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Boron)
            .unwrap();
        assert!((boron.values[0] - 0.4).abs() < 1e-9);
    }

    #[test]
    fn beam_off_contributes_exactly_zero() {
        // Output observed but the beam state is Off for its span —
        // no dose, zero delivered seconds.
        let history = history_with(vec![
            HistoryStream {
                id: "out".into(),
                kind: StreamKind::IntervalAverageRate,
                unit: "u".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![HistorySample::IntervalAverage {
                    t_start_s: 10.0,
                    t_end_s: 50.0,
                    rate: 99.0,
                    quality: QualityFlag::Good,
                }],
            },
            HistoryStream {
                id: "state".into(),
                kind: StreamKind::BeamState,
                unit: "1".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![HistorySample::State {
                    time_s: 0.0,
                    state: BeamState::Off,
                    quality: QualityFlag::Good,
                }],
            },
        ]);
        let conc = ConcentrationHistory {
            points: vec![(0.0, 20.0)],
            rule: ConcentrationRule::Step,
            unit: "ug/g".into(),
            reference_concentration: 20.0,
            hold_boundary: true,
        };
        let s = spec(conc);
        let (_, report) = run_replay(
            &s,
            &history,
            &[&bundle(2e-12, 1e-12)],
            ContentReference {
                id: "r".into(),
                sha256: "e".repeat(64),
            },
            None,
        )
        .unwrap();
        let br = &report.beams[0];
        assert_eq!(br.delivered_seconds, 0.0);
        assert_eq!(br.integrated_output, 0.0);
        assert_eq!(report.total_dose_gray, 0.0);
    }

    #[test]
    fn splitting_an_identical_interval_preserves_dose() {
        // [0,200) at rate r as one sample vs two [0,100)+[100,200)
        // samples at the same rate — same accumulated dose.
        let rate_sample = |a: f64, b: f64, r: f64| HistorySample::IntervalAverage {
            t_start_s: a,
            t_end_s: b,
            rate: r,
            quality: QualityFlag::Good,
        };
        let conc = || ConcentrationHistory {
            points: vec![(0.0, 10.0), (200.0, 30.0)],
            rule: ConcentrationRule::Linear,
            unit: "ug/g".into(),
            reference_concentration: 20.0,
            hold_boundary: false,
        };
        let s = spec(conc());
        let cref = |n: usize| ContentReference {
            id: "r".into(),
            sha256: n.to_string().repeat(64),
        };
        let one = run_replay(
            &s,
            &history_with(output_history(vec![rate_sample(0.0, 200.0, 80.0)])),
            &[&bundle(2e-12, 1e-12)],
            cref(1),
            None,
        )
        .unwrap();
        let two = run_replay(
            &s,
            &history_with(output_history(vec![
                rate_sample(0.0, 100.0, 80.0),
                rate_sample(100.0, 200.0, 80.0),
            ])),
            &[&bundle(2e-12, 1e-12)],
            cref(2),
            None,
        )
        .unwrap();
        for component in &one.0.components {
            let other = two
                .0
                .components
                .iter()
                .find(|c| c.component == component.component)
                .unwrap();
            assert_eq!(component.values, other.values);
        }
    }

    #[test]
    fn exponential_concentration_matches_the_pk_integral() {
        // C(t) = C0·e^(−λt) sampled under the linear rule converges to
        // the analytic integral C0(1−e^{−λT})/λ. The declared check is
        // a convergent approximation, not an exact match — the
        // trapezoid error for this smooth curve is ~h²·λ²·T·C0/12.
        let (lambda, t_end, c0, c_ref) = (0.01_f64, 200.0_f64, 20.0_f64, 20.0_f64);
        let points: Vec<(f64, f64)> = (0..=2000)
            .map(|i| {
                let t = i as f64 / 10.0;
                (t, c0 * (-lambda * t).exp())
            })
            .collect();
        let conc = ConcentrationHistory {
            points,
            rule: ConcentrationRule::Linear,
            unit: "ug/g".into(),
            reference_concentration: c_ref,
            hold_boundary: false,
        };
        let analytic = c0 * (1.0 - (-lambda * t_end).exp()) / lambda; // ≈ 172.933
        let numeric = integrate_concentration(&conc, 0.0, t_end).unwrap();
        // h=0.1 mesh: trapezoid error h²/12·(f'(T)−f'(0)) ≈ 1.4e-4 —
        // a declared approximation bound, not an exact-match claim.
        assert!((numeric - analytic).abs() < 1e-3);
    }

    #[test]
    fn coverage_spans_the_beam_on_window_not_just_observed_output() {
        // Beam on [0,1000) but output observed only [0,100): the
        // report's coverage must reach 1000 — the unobserved on-window
        // is a gap, and the session span can't quietly end at the last
        // delivered interval.
        let history = history_with(vec![
            HistoryStream {
                id: "out".into(),
                kind: StreamKind::IntervalAverageRate,
                unit: "u".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![HistorySample::IntervalAverage {
                    t_start_s: 0.0,
                    t_end_s: 100.0,
                    rate: 10.0,
                    quality: QualityFlag::Good,
                }],
            },
            HistoryStream {
                id: "state".into(),
                kind: StreamKind::BeamState,
                unit: "1".into(),
                clock: clock(),
                available_at_seconds: None,
                coordinate_frame: None,
                counter_bits: None,
                beam: Some("b1".into()),
                samples: vec![
                    HistorySample::State {
                        time_s: 0.0,
                        state: BeamState::On,
                        quality: QualityFlag::Good,
                    },
                    HistorySample::State {
                        time_s: 1000.0,
                        state: BeamState::Off,
                        quality: QualityFlag::Good,
                    },
                ],
            },
        ]);
        let conc = ConcentrationHistory {
            points: vec![(0.0, 20.0)],
            rule: ConcentrationRule::Step,
            unit: "ug/g".into(),
            reference_concentration: 20.0,
            hold_boundary: true,
        };
        let s = spec(conc);
        let (_, report) = run_replay(
            &s,
            &history,
            &[&bundle(2e-12, 1e-12)],
            ContentReference {
                id: "r".into(),
                sha256: "e".repeat(64),
            },
            None,
        )
        .unwrap();
        assert_eq!(report.coverage, (0.0, 1000.0));
        assert!(!report.coverage_complete);
        assert_eq!(report.beams[0].gaps, vec![(100.0, 1000.0)]);
    }
}
