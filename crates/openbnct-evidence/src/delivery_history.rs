// SPDX-License-Identifier: MIT

//! Normalized retrospective delivery history
//! (`openbnct.delivery-history/0.1.0`) and the strict CSV importer
//! (`openbnct.delivery-csv-import/0.1.0`).
//!
//! A history records what was *measured* during an irradiation session:
//! per-beam references with their calibration validity windows, a shared
//! coordinate frame, and typed streams of samples. Each stream declares
//! its clock basis (offset, optional drift, timing uncertainty) and its
//! sample kind — instantaneous readings, interval-average rates,
//! integrated counts, cumulative counters, or beam-state events — and
//! each sample carries a quality flag. Acquisition time is separate
//! from result-availability time (e.g. a blood assay's lab timestamp).
//!
//! Missing data stays missing: `stream_gaps` reports uncovered spans,
//! `interval_rates` never fabricates a zero output over a gap, and
//! counter rollovers require either declared `counter_bits` or an
//! explicit `reset` flag — ambiguity stops the conversion, silently
//! carrying a stale value forward is not a supported state.

use std::collections::BTreeSet;

use openbnct_core::{ContentReference, deserialize_contract_id, schema_matches};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Contract token for [`DeliveryHistory`].
pub const DELIVERY_HISTORY_SCHEMA: &str = "openbnct.delivery-history/0.1.0";
/// Contract token for [`CsvImportSpec`].
pub const CSV_IMPORT_SPEC_SCHEMA: &str = "openbnct.delivery-csv-import/0.1.0";

/// Per-sample quality, carried verbatim from the source record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityFlag {
    Good,
    Suspect,
    /// Interpolated or derived, not measured.
    Estimated,
    /// Declared missing — a segment that exists but has no value. It is
    /// *not* a zero.
    Unavailable,
}

/// What a stream's samples mean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Point readings at instants (`monitor current`, `position`).
    Instantaneous,
    /// Average rate over each `[start, end)` interval.
    IntervalAverageRate,
    /// Integrated counts/charge over each interval.
    IntegratedCounts,
    /// Cumulative counter readings — converted to interval rates by
    /// difference, with explicit rollover/reset handling.
    CumulativeCounter,
    /// Beam-state events (on/off/hold) marking interval edges.
    BeamState,
}

/// Beam state marker for `beam_state` streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeamState {
    On,
    Off,
    Hold,
}

/// One typed sample. The variant set is closed; every kind's time
/// semantics are declared on the stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HistorySample {
    /// Reading at an instant (stream clock seconds).
    Instantaneous {
        time_s: f64,
        value: f64,
        quality: QualityFlag,
    },
    /// Average rate over `[t_start_s, t_end_s)`.
    IntervalAverage {
        t_start_s: f64,
        t_end_s: f64,
        rate: f64,
        quality: QualityFlag,
    },
    /// Integrated counts/charge over `[t_start_s, t_end_s)`.
    Integrated {
        t_start_s: f64,
        t_end_s: f64,
        counts: f64,
        quality: QualityFlag,
    },
    /// Cumulative counter reading at an instant. `reset` marks a
    /// declared counter reset: the previous cumulative value does not
    /// carry forward through it.
    Cumulative {
        time_s: f64,
        cumulative: f64,
        reset: bool,
        quality: QualityFlag,
    },
    /// Beam-state event at an instant.
    State {
        time_s: f64,
        state: BeamState,
        quality: QualityFlag,
    },
}

/// Clock basis of one stream: session time is
/// `t_session = offset_seconds + stream_time * (1 + drift_ppm/1e6)`,
/// each instant uncertain by `timing_uncertainty_seconds`. The epoch a
/// stream clock starts counting from is `epoch` (documentation-grade —
/// numeric stream time is what downstream consumers align on).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockBasis {
    /// Clock name (`accelerator_monotonic`, `lab_ntp`, `assay_release`).
    pub name: String,
    /// Stream-time value that maps to session t=0.
    pub offset_seconds: f64,
    /// Optional linear drift in ppm (clock runs fast/slow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_ppm: Option<f64>,
    /// One-sigma timing uncertainty applied at alignment, seconds.
    pub timing_uncertainty_seconds: f64,
    /// Human-readable epoch (`2026-01-01T00:00:00Z`); the declared
    /// timezone is part of the epoch string — an unzoned epoch is
    /// rejected as ambiguous.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch: Option<String>,
}

impl ClockBasis {
    /// Map stream time to session seconds.
    pub fn to_session(&self, t_stream: f64) -> f64 {
        self.offset_seconds + t_stream * (1.0 + self.drift_ppm.unwrap_or(0.0) * 1e-6)
    }
}

/// One measured stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryStream {
    pub id: String,
    pub kind: StreamKind,
    /// Physical unit of `value`/`rate`/`counts` (`uA`, `Hz`, `counts`,
    /// `mm`, `1` for states).
    pub unit: String,
    pub clock: ClockBasis,
    /// When results became usable — for assays, the lab release time
    /// (session seconds), distinct from when the sample was drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_at_seconds: Option<f64>,
    /// Declared coordinate frame for position streams; must equal the
    /// history's frame when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinate_frame: Option<String>,
    /// Counter modulus (`2^bits`) for `cumulative_counter` streams.
    /// A cumulative decrease is then a rollover, not a reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_bits: Option<u32>,
    /// Which beam this stream belongs to, when beam-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam: Option<String>,
    pub samples: Vec<HistorySample>,
}

/// One beam used in the session, with its calibration identity and
/// validity window (session seconds). Samples outside the window are
/// flagged as expired-calibration evidence, not silently trusted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamReference {
    pub id: String,
    /// Content-bound beam description.
    pub beam: ContentReference,
    /// Content-bound calibration record.
    pub calibration: ContentReference,
    /// `[start_s, end_s]` calibration validity in session seconds.
    pub calibration_valid: (f64, f64),
}

/// The normalized session record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryHistory {
    #[serde(deserialize_with = "deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: String,
    pub provenance_id: String,
    pub session_id: String,
    /// Coordinate frame every position-bearing stream resolves to
    /// (`iec61217-table-top`, `patient-dicom`). Streams declaring a
    /// different frame are rejected at validation — there is no
    /// implicit transform.
    pub coordinate_frame: String,
    #[serde(default)]
    pub beams: Vec<BeamReference>,
    #[serde(default)]
    pub streams: Vec<HistoryStream>,
}

/// A delivered interval of (average) rate, session seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct RateInterval {
    pub t_start_s: f64,
    pub t_end_s: f64,
    pub rate: f64,
    pub quality: QualityFlag,
}

#[derive(Debug, Error)]
pub enum HistoryError {
    #[error("unsupported delivery-history schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid delivery history: {0}")]
    Invalid(String),
}

fn sample_time(s: &HistorySample) -> f64 {
    match s {
        HistorySample::Instantaneous { time_s, .. }
        | HistorySample::Cumulative { time_s, .. }
        | HistorySample::State { time_s, .. } => *time_s,
        HistorySample::IntervalAverage { t_start_s, .. }
        | HistorySample::Integrated { t_start_s, .. } => *t_start_s,
    }
}

fn sample_kind_ok(stream_kind: StreamKind, s: &HistorySample) -> bool {
    matches!(
        (stream_kind, s),
        (
            StreamKind::Instantaneous,
            HistorySample::Instantaneous { .. }
        ) | (
            StreamKind::IntervalAverageRate,
            HistorySample::IntervalAverage { .. }
        ) | (
            StreamKind::IntegratedCounts,
            HistorySample::Integrated { .. }
        ) | (
            StreamKind::CumulativeCounter,
            HistorySample::Cumulative { .. }
        ) | (StreamKind::BeamState, HistorySample::State { .. })
    )
}

impl DeliveryHistory {
    pub fn validate(&self) -> Result<(), HistoryError> {
        if !schema_matches(&self.schema_version, DELIVERY_HISTORY_SCHEMA) {
            return Err(HistoryError::UnsupportedSchema(self.schema_version.clone()));
        }
        for (label, value) in [
            ("id", &self.id),
            ("provenance_id", &self.provenance_id),
            ("session_id", &self.session_id),
            ("coordinate_frame", &self.coordinate_frame),
        ] {
            if value.trim().is_empty() {
                return Err(HistoryError::Invalid(format!("{label} is required")));
            }
        }
        let mut beam_ids = BTreeSet::new();
        for beam in &self.beams {
            if !beam_ids.insert(beam.id.as_str()) {
                return Err(HistoryError::Invalid(format!(
                    "duplicate beam id {:?}",
                    beam.id
                )));
            }
            beam.beam
                .validate()
                .map_err(|e| HistoryError::Invalid(e.to_string()))?;
            beam.calibration
                .validate()
                .map_err(|e| HistoryError::Invalid(e.to_string()))?;
            if beam
                .calibration_valid
                .0
                .partial_cmp(&beam.calibration_valid.1)
                != Some(std::cmp::Ordering::Less)
            {
                return Err(HistoryError::Invalid(format!(
                    "beam {:?}: calibration validity must satisfy start < end",
                    beam.id
                )));
            }
        }
        let mut stream_ids = BTreeSet::new();
        for stream in &self.streams {
            if !stream_ids.insert(stream.id.as_str()) {
                return Err(HistoryError::Invalid(format!(
                    "duplicate stream id {:?}",
                    stream.id
                )));
            }
            if stream.id.trim().is_empty() || stream.unit.trim().is_empty() {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?} needs id and unit",
                    stream.id
                )));
            }
            if let Some(beam_id) = &stream.beam
                && !beam_ids.contains(beam_id.as_str())
            {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?} references undeclared beam {beam_id:?}",
                    stream.id
                )));
            }
            if let Some(frame) = &stream.coordinate_frame
                && frame != &self.coordinate_frame
            {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?} declares frame {frame:?}; history frame is {:?} — mismatched frames are not transformed implicitly",
                    stream.id, self.coordinate_frame
                )));
            }
            let clock = &stream.clock;
            if clock.name.trim().is_empty()
                || !clock.timing_uncertainty_seconds.is_finite()
                || clock.timing_uncertainty_seconds < 0.0
            {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?} needs a named clock and finite non-negative timing uncertainty",
                    stream.id
                )));
            }
            if let Some(epoch) = &clock.epoch {
                // An epoch must declare its zone (`Z` or ±hh:mm offset);
                // a bare datetime is ambiguous.
                let zoned = epoch.ends_with('Z')
                    || epoch[epoch.len().saturating_sub(6)..].contains('+')
                    || epoch[epoch.len().saturating_sub(6)..].contains('-');
                if !zoned {
                    return Err(HistoryError::Invalid(format!(
                        "stream {:?}: epoch {epoch:?} lacks a timezone declaration",
                        stream.id
                    )));
                }
            }
            if let Some(bits) = stream.counter_bits
                && (stream.kind != StreamKind::CumulativeCounter || !(1..=64).contains(&bits))
            {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?}: counter_bits applies to cumulative counters only, in 1..=64",
                    stream.id
                )));
            }
            // Ordered, non-duplicated, kind-matched, finite samples.
            let mut prev_time: Option<f64> = None;
            let mut prev_cumulative: Option<f64> = None;
            for (i, sample) in stream.samples.iter().enumerate() {
                if !sample_kind_ok(stream.kind, sample) {
                    return Err(HistoryError::Invalid(format!(
                        "stream {:?} sample {i}: sample kind does not match stream kind {:?}",
                        stream.id, stream.kind
                    )));
                }
                let t = sample_time(sample);
                if !t.is_finite() {
                    return Err(HistoryError::Invalid(format!(
                        "stream {:?} sample {i}: non-finite time",
                        stream.id
                    )));
                }
                if let Some(prev) = prev_time
                    && t <= prev
                {
                    return Err(HistoryError::Invalid(format!(
                        "stream {:?} sample {i}: timestamps must be strictly increasing (t={t}, previous={prev}) — duplicates and reordering are resolved at import, not silently here",
                        stream.id
                    )));
                }
                prev_time = Some(t);
                match sample {
                    HistorySample::Instantaneous { value, .. } if !value.is_finite() => {
                        return Err(HistoryError::Invalid(format!(
                            "stream {:?} sample {i}: non-finite value",
                            stream.id
                        )));
                    }
                    HistorySample::IntervalAverage {
                        t_start_s,
                        t_end_s,
                        rate,
                        ..
                    }
                    | HistorySample::Integrated {
                        t_start_s,
                        t_end_s,
                        counts: rate,
                        ..
                    } if t_end_s.partial_cmp(t_start_s) != Some(std::cmp::Ordering::Greater)
                        || !rate.is_finite() =>
                    {
                        return Err(HistoryError::Invalid(format!(
                            "stream {:?} sample {i}: interval needs start < end and a finite value",
                            stream.id
                        )));
                    }
                    HistorySample::Cumulative {
                        cumulative, reset, ..
                    } => {
                        if !cumulative.is_finite() {
                            return Err(HistoryError::Invalid(format!(
                                "stream {:?} sample {i}: non-finite cumulative",
                                stream.id
                            )));
                        }
                        if let Some(prev) = prev_cumulative {
                            let rollover = stream.counter_bits.is_some_and(|bits| {
                                *cumulative + 2f64.powi(bits as i32) - prev >= 0.0
                            }) && *cumulative < prev;
                            if *cumulative < prev && !*reset && !rollover {
                                return Err(HistoryError::Invalid(format!(
                                    "stream {:?} sample {i}: cumulative counter decreased without a declared reset or counter_bits rollover",
                                    stream.id
                                )));
                            }
                        }
                        prev_cumulative = Some(*cumulative);
                    }
                    _ => {}
                }
            }
            // Interval streams must not overlap: strict increase plus
            // next start >= previous end.
            if matches!(
                stream.kind,
                StreamKind::IntervalAverageRate | StreamKind::IntegratedCounts
            ) {
                for w in stream.samples.windows(2) {
                    let (a_end, b_start) = match (&w[0], &w[1]) {
                        (
                            HistorySample::IntervalAverage { t_end_s, .. },
                            HistorySample::IntervalAverage { t_start_s, .. },
                        )
                        | (
                            HistorySample::Integrated { t_end_s, .. },
                            HistorySample::Integrated { t_start_s, .. },
                        ) => (*t_end_s, *t_start_s),
                        _ => continue,
                    };
                    if b_start < a_end {
                        return Err(HistoryError::Invalid(format!(
                            "stream {:?}: overlapping intervals [{}, {}) and [{}, ...)",
                            stream.id, b_start, a_end, b_start
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Convert the rate-bearing stream kinds into session-time rate
    /// intervals. `cumulative_counter` differences become rates;
    /// `integrated_counts` divide by duration — each conversion is a
    /// single application, never double-integrated. `unavailable`
    /// samples produce no interval. Returns an error when a cumulative
    /// decrease cannot be explained by bits rollover or a declared
    /// reset.
    pub fn interval_rates(&self, stream_id: &str) -> Result<Vec<RateInterval>, HistoryError> {
        let stream = self
            .streams
            .iter()
            .find(|s| s.id == stream_id)
            .ok_or_else(|| HistoryError::Invalid(format!("unknown stream {stream_id:?}")))?;
        let clock = &stream.clock;
        match stream.kind {
            StreamKind::IntervalAverageRate => Ok(stream
                .samples
                .iter()
                .filter_map(|s| match s {
                    HistorySample::IntervalAverage {
                        t_start_s,
                        t_end_s,
                        rate,
                        quality,
                    } if *quality != QualityFlag::Unavailable => Some(RateInterval {
                        t_start_s: clock.to_session(*t_start_s),
                        t_end_s: clock.to_session(*t_end_s),
                        rate: *rate,
                        quality: *quality,
                    }),
                    _ => None,
                })
                .collect()),
            StreamKind::IntegratedCounts => Ok(stream
                .samples
                .iter()
                .filter_map(|s| match s {
                    HistorySample::Integrated {
                        t_start_s,
                        t_end_s,
                        counts,
                        quality,
                    } if *quality != QualityFlag::Unavailable => Some(RateInterval {
                        t_start_s: clock.to_session(*t_start_s),
                        t_end_s: clock.to_session(*t_end_s),
                        rate: counts / (t_end_s - t_start_s),
                        quality: *quality,
                    }),
                    _ => None,
                })
                .collect()),
            StreamKind::CumulativeCounter => {
                let mut intervals = Vec::new();
                let mut prev: Option<(f64, f64)> = None; // (t, cumulative)
                for s in &stream.samples {
                    let HistorySample::Cumulative {
                        time_s,
                        cumulative,
                        reset,
                        quality,
                    } = s
                    else {
                        continue;
                    };
                    if let Some((t0, cum0)) = prev {
                        let mut delta = cumulative - cum0;
                        if delta < 0.0 {
                            if *reset {
                                // Reset: the interval boundary restarts
                                // at this sample — no rate is claimed
                                // across the reset.
                                prev = Some((*time_s, *cumulative));
                                continue;
                            }
                            if let Some(bits) = stream.counter_bits {
                                delta += 2f64.powi(bits as i32);
                            }
                        }
                        if delta < 0.0 {
                            return Err(HistoryError::Invalid(format!(
                                "stream {stream_id:?}: unexplained counter decrease at t={time_s}"
                            )));
                        }
                        intervals.push(RateInterval {
                            t_start_s: clock.to_session(t0),
                            t_end_s: clock.to_session(*time_s),
                            rate: delta / (time_s - t0),
                            quality: *quality,
                        });
                    }
                    prev = Some((*time_s, *cumulative));
                }
                intervals.retain(|i| i.quality != QualityFlag::Unavailable);
                Ok(intervals)
            }
            StreamKind::Instantaneous => Err(HistoryError::Invalid(format!(
                "stream {stream_id:?} is instantaneous — it has no interval rates"
            ))),
            StreamKind::BeamState => Err(HistoryError::Invalid(format!(
                "stream {stream_id:?} is a beam-state event stream — use beam_on_intervals"
            ))),
        }
    }

    /// Beam-on intervals (session seconds) for a `beam_state` stream.
    /// A dangling `on` with no closing event reports its interval as
    /// open — `t_end_s` = None.
    pub fn beam_on_intervals(
        &self,
        stream_id: &str,
    ) -> Result<Vec<(f64, Option<f64>)>, HistoryError> {
        let stream = self
            .streams
            .iter()
            .find(|s| s.id == stream_id)
            .ok_or_else(|| HistoryError::Invalid(format!("unknown stream {stream_id:?}")))?;
        if stream.kind != StreamKind::BeamState {
            return Err(HistoryError::Invalid(format!(
                "stream {stream_id:?} is {:?}, not a beam-state stream",
                stream.kind
            )));
        }
        let mut out = Vec::new();
        let mut on_at: Option<f64> = None;
        for s in &stream.samples {
            if let HistorySample::State {
                time_s,
                state,
                quality,
            } = s
            {
                if *quality == QualityFlag::Unavailable {
                    continue;
                }
                match state {
                    BeamState::On if on_at.is_none() => on_at = Some(*time_s),
                    BeamState::Off | BeamState::Hold => {
                        if let Some(t0) = on_at.take() {
                            out.push((
                                stream.clock.to_session(t0),
                                Some(stream.clock.to_session(*time_s)),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(t0) = on_at {
            out.push((stream.clock.to_session(t0), None));
        }
        Ok(out)
    }

    /// Uncovered spans of `[t_start, t_end]` (session seconds) for one
    /// stream — the explicit gaps where nothing was observed. A gap is
    /// information, not a zero-output interval.
    pub fn stream_gaps(
        &self,
        stream_id: &str,
        t_start: f64,
        t_end: f64,
    ) -> Result<Vec<(f64, f64)>, HistoryError> {
        let stream = self
            .streams
            .iter()
            .find(|s| s.id == stream_id)
            .ok_or_else(|| HistoryError::Invalid(format!("unknown stream {stream_id:?}")))?;
        let covered: Vec<(f64, f64)> = match stream.kind {
            StreamKind::IntervalAverageRate | StreamKind::IntegratedCounts => stream
                .samples
                .iter()
                .map(|s| {
                    let (a, b) = match s {
                        HistorySample::IntervalAverage {
                            t_start_s, t_end_s, ..
                        }
                        | HistorySample::Integrated {
                            t_start_s, t_end_s, ..
                        } => (*t_start_s, *t_end_s),
                        _ => unreachable!(),
                    };
                    (stream.clock.to_session(a), stream.clock.to_session(b))
                })
                .collect(),
            _ => {
                let rates = self.interval_rates(stream_id)?;
                rates.iter().map(|r| (r.t_start_s, r.t_end_s)).collect()
            }
        };
        let mut gaps = Vec::new();
        let mut cursor = t_start;
        for (a, b) in covered {
            if a > cursor {
                gaps.push((cursor, a));
            }
            cursor = cursor.max(b);
        }
        if cursor < t_end {
            gaps.push((cursor, t_end));
        }
        Ok(gaps)
    }

    /// Beams whose calibration window does not cover `t_session` —
    /// expired-calibration observations must stay visible.
    pub fn expired_calibrations(&self, t_session: f64) -> Vec<&BeamReference> {
        self.beams
            .iter()
            .filter(|b| t_session < b.calibration_valid.0 || t_session > b.calibration_valid.1)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Strict CSV import
// ---------------------------------------------------------------------------

/// Which columns map onto a sample in a strict CSV import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CsvColumns {
    /// Instant/interval-start column name.
    pub time: String,
    /// Interval-end column (rate/count streams).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_end: Option<String>,
    /// Value column (rate, counts, cumulative, or state text).
    pub value: String,
    /// Optional quality column (`good|suspect|estimated|unavailable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    /// Optional boolean reset-marker column for counter streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<String>,
}

/// One stream's import mapping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CsvStreamSpec {
    pub stream_id: String,
    pub kind: StreamKind,
    pub unit: String,
    pub clock: ClockBasis,
    /// CSV file path, relative to the spec file's directory.
    pub csv: String,
    pub columns: CsvColumns,
    /// Optional beam scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub beam: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_at_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter_bits: Option<u32>,
}

/// The strict importer configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CsvImportSpec {
    #[serde(deserialize_with = "deserialize_contract_id")]
    pub schema_version: String,
    pub history_id: String,
    pub session_id: String,
    pub coordinate_frame: String,
    pub qualification: String,
    pub provenance_id: String,
    #[serde(default)]
    pub beams: Vec<BeamReference>,
    /// Field delimiter (declared — commonly `,`).
    #[serde(default = "default_delimiter")]
    pub delimiter: String,
    /// Lines beginning with this char are comments.
    #[serde(default = "default_comment")]
    pub comment: String,
    pub streams: Vec<CsvStreamSpec>,
}

fn default_delimiter() -> String {
    ",".into()
}
fn default_comment() -> String {
    "#".into()
}

/// What the importer did — kept inspectable alongside the history.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportDiagnostics {
    pub stream_id: String,
    pub rows_parsed: u64,
    /// `(row number, reason)` for every rejected row.
    pub rows_rejected: Vec<(u64, String)>,
    /// Samples that arrived out of order and were re-sorted.
    pub reordered: u64,
    /// Duplicate timestamps dropped.
    pub duplicates_dropped: u64,
    /// Counter rollovers resolved via `counter_bits`.
    pub rollovers_resolved: u64,
    /// Declared resets honored.
    pub resets_honored: u64,
    /// Extra header columns that were declared absent in the spec —
    /// ignored but reported.
    pub extra_columns: Vec<String>,
}

impl CsvImportSpec {
    pub fn validate(&self) -> Result<(), HistoryError> {
        if !schema_matches(&self.schema_version, CSV_IMPORT_SPEC_SCHEMA) {
            return Err(HistoryError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.delimiter.len() != 1 || self.comment.len() != 1 {
            return Err(HistoryError::Invalid(
                "delimiter and comment must be single characters".into(),
            ));
        }
        if self.streams.is_empty() {
            return Err(HistoryError::Invalid(
                "at least one stream is required".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for s in &self.streams {
            if !ids.insert(s.stream_id.as_str()) || s.stream_id.trim().is_empty() {
                return Err(HistoryError::Invalid(format!(
                    "stream id {:?} missing or duplicated",
                    s.stream_id
                )));
            }
            if matches!(
                s.kind,
                StreamKind::IntervalAverageRate | StreamKind::IntegratedCounts
            ) && s.columns.time_end.is_none()
            {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?}: interval kinds need a time_end column",
                    s.stream_id
                )));
            }
            if s.counter_bits.is_some() && s.kind != StreamKind::CumulativeCounter {
                return Err(HistoryError::Invalid(format!(
                    "stream {:?}: counter_bits applies to cumulative counters only",
                    s.stream_id
                )));
            }
        }
        Ok(())
    }
}

fn parse_quality(text: &str) -> Result<QualityFlag, HistoryError> {
    match text.trim().to_ascii_lowercase().as_str() {
        "good" | "ok" | "1" => Ok(QualityFlag::Good),
        "suspect" => Ok(QualityFlag::Suspect),
        "estimated" => Ok(QualityFlag::Estimated),
        "unavailable" | "missing" | "nan" | "" => Ok(QualityFlag::Unavailable),
        other => Err(HistoryError::Invalid(format!(
            "unknown quality flag {other:?}"
        ))),
    }
}

fn parse_state(text: &str) -> Result<BeamState, HistoryError> {
    match text.trim().to_ascii_lowercase().as_str() {
        "on" | "beam_on" => Ok(BeamState::On),
        "off" | "beam_off" => Ok(BeamState::Off),
        "hold" | "beam_hold" => Ok(BeamState::Hold),
        other => Err(HistoryError::Invalid(format!(
            "unknown beam state {other:?}"
        ))),
    }
}

/// Strictly import one stream's CSV text. Rows are `delimiter`-split
/// (no quoted fields — a quoting character in data is an error, so
/// malformed exports surface rather than misalign). Header names must
/// contain every declared column; extra columns are reported, not
/// followed. Out-of-order samples are re-sorted and counted;
/// duplicate timestamps are dropped and counted; rows that fail to
/// parse are rejected with their row number.
pub fn import_stream_csv(
    spec: &CsvStreamSpec,
    delimiter: char,
    comment: char,
    text: &str,
) -> Result<(HistoryStream, ImportDiagnostics), HistoryError> {
    let mut diagnostics = ImportDiagnostics {
        stream_id: spec.stream_id.clone(),
        ..Default::default()
    };
    let mut lines = text.lines().enumerate().peekable();
    // Skip blank/comment lines before the header.
    let mut header: Option<Vec<String>> = None;
    for (_, line) in lines.by_ref() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(comment) {
            continue;
        }
        header = Some(
            line.split(delimiter)
                .map(|c| c.trim().to_string())
                .collect(),
        );
        break;
    }
    let header = header.ok_or_else(|| {
        HistoryError::Invalid(format!("stream {:?}: no header row", spec.stream_id))
    })?;
    if header.iter().any(|c| c.contains('"') || c.contains('\'')) {
        return Err(HistoryError::Invalid(
            "quoted fields are outside the strict reference format".into(),
        ));
    }
    let declared: BTreeSet<&str> = [
        Some(spec.columns.time.as_str()),
        spec.columns.time_end.as_deref(),
        Some(spec.columns.value.as_str()),
        spec.columns.quality.as_deref(),
        spec.columns.reset.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    for col in &header {
        if !declared.contains(col.as_str()) {
            diagnostics.extra_columns.push(col.clone());
        }
    }
    let col_index = |name: &str| -> Result<usize, HistoryError> {
        header.iter().position(|c| c == name).ok_or_else(|| {
            HistoryError::Invalid(format!(
                "stream {:?}: required column {name:?} not in header {header:?}",
                spec.stream_id
            ))
        })
    };
    let i_time = col_index(&spec.columns.time)?;
    let i_time_end = spec
        .columns
        .time_end
        .as_deref()
        .map(col_index)
        .transpose()?;
    let i_value = col_index(&spec.columns.value)?;
    let i_quality = spec.columns.quality.as_deref().map(col_index).transpose()?;
    let i_reset = spec.columns.reset.as_deref().map(col_index).transpose()?;

    let mut samples: Vec<(u64, HistorySample)> = Vec::new();
    for (row, line) in lines {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with(comment) {
            continue;
        }
        let fields: Vec<&str> = line.split(delimiter).map(|f| f.trim()).collect();
        let rowno = row as u64 + 1;
        if fields.iter().any(|f| f.contains('"') || f.contains('\'')) {
            diagnostics.rows_rejected.push((
                rowno,
                "quoted fields are outside the strict reference format".into(),
            ));
            continue;
        }
        if fields.len() < header.len() {
            diagnostics
                .rows_rejected
                .push((rowno, "fewer fields than the header".into()));
            continue;
        }
        let fnum = |idx: usize, what: &str| -> Result<f64, String> {
            fields[idx]
                .parse::<f64>()
                .map_err(|_| format!("{what} {v:?} is not a number", v = fields[idx]))
        };
        let t0 = match fnum(i_time, "time") {
            Ok(v) => v,
            Err(e) => {
                diagnostics.rows_rejected.push((rowno, e));
                continue;
            }
        };
        let quality = match i_quality {
            Some(idx) => match parse_quality(fields[idx]) {
                Ok(q) => q,
                Err(e) => {
                    diagnostics.rows_rejected.push((rowno, e.to_string()));
                    continue;
                }
            },
            None => QualityFlag::Good,
        };
        let reset = match i_reset {
            Some(idx) => matches!(
                fields[idx].trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "reset"
            ),
            None => false,
        };
        let sample = match spec.kind {
            StreamKind::Instantaneous => match fnum(i_value, "value") {
                Ok(v) => HistorySample::Instantaneous {
                    time_s: t0,
                    value: v,
                    quality,
                },
                Err(e) => {
                    diagnostics.rows_rejected.push((rowno, e));
                    continue;
                }
            },
            StreamKind::IntervalAverageRate | StreamKind::IntegratedCounts => {
                let tend = match fnum(i_time_end.expect("validated"), "time_end") {
                    Ok(v) => v,
                    Err(e) => {
                        diagnostics.rows_rejected.push((rowno, e));
                        continue;
                    }
                };
                match fnum(i_value, "value") {
                    Ok(v) if spec.kind == StreamKind::IntervalAverageRate => {
                        HistorySample::IntervalAverage {
                            t_start_s: t0,
                            t_end_s: tend,
                            rate: v,
                            quality,
                        }
                    }
                    Ok(v) => HistorySample::Integrated {
                        t_start_s: t0,
                        t_end_s: tend,
                        counts: v,
                        quality,
                    },
                    Err(e) => {
                        diagnostics.rows_rejected.push((rowno, e));
                        continue;
                    }
                }
            }
            StreamKind::CumulativeCounter => match fnum(i_value, "cumulative") {
                Ok(v) => HistorySample::Cumulative {
                    time_s: t0,
                    cumulative: v,
                    reset,
                    quality,
                },
                Err(e) => {
                    diagnostics.rows_rejected.push((rowno, e));
                    continue;
                }
            },
            StreamKind::BeamState => match parse_state(fields[i_value]) {
                Ok(state) => HistorySample::State {
                    time_s: t0,
                    state,
                    quality,
                },
                Err(e) => {
                    diagnostics.rows_rejected.push((rowno, e.to_string()));
                    continue;
                }
            },
        };
        samples.push((rowno, sample));
        diagnostics.rows_parsed += 1;
    }
    // Re-order policy: sort by stream time, counting moves; duplicates
    // are dropped (first wins) and counted.
    let mut ordered = samples.clone();
    ordered.sort_by(|a, b| sample_time(&a.1).total_cmp(&sample_time(&b.1)));
    diagnostics.reordered = ordered
        .iter()
        .zip(&samples)
        .filter(|(a, b)| a.0 != b.0)
        .count() as u64;
    let mut dedup: Vec<HistorySample> = Vec::new();
    let mut seen = BTreeSet::new();
    for (_, s) in ordered {
        let key = sample_time(&s).to_bits();
        if !seen.insert(key) {
            diagnostics.duplicates_dropped += 1;
            continue;
        }
        dedup.push(s);
    }
    let stream = HistoryStream {
        id: spec.stream_id.clone(),
        kind: spec.kind,
        unit: spec.unit.clone(),
        clock: spec.clock.clone(),
        available_at_seconds: spec.available_at_seconds,
        coordinate_frame: None,
        counter_bits: spec.counter_bits,
        beam: spec.beam.clone(),
        samples: dedup,
    };
    Ok((stream, diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> ClockBasis {
        ClockBasis {
            name: "accelerator_monotonic".into(),
            offset_seconds: 100.0,
            drift_ppm: None,
            timing_uncertainty_seconds: 0.01,
            epoch: None,
        }
    }

    fn history(streams: Vec<HistoryStream>) -> DeliveryHistory {
        DeliveryHistory {
            schema_version: DELIVERY_HISTORY_SCHEMA.into(),
            id: "h1".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "test".into(),
            session_id: "s1".into(),
            coordinate_frame: "iec61217".into(),
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
                calibration_valid: (0.0, 3600.0),
            }],
            streams,
        }
    }

    #[test]
    fn counter_rollover_and_reset_have_explicit_policies() {
        // 2-bit counter (modulus 4): 3 → 0 is rollover (+4 − 3 = 1
        // count over 2 s → rate 0.5); a declared reset drops the
        // bridging interval; an unexplained decrease is rejected.
        let stream = HistoryStream {
            id: "ctr".into(),
            kind: StreamKind::CumulativeCounter,
            unit: "counts".into(),
            clock: clock(),
            available_at_seconds: None,
            coordinate_frame: None,
            counter_bits: Some(2),
            beam: Some("b1".into()),
            samples: vec![
                HistorySample::Cumulative {
                    time_s: 0.0,
                    cumulative: 1.0,
                    reset: false,
                    quality: QualityFlag::Good,
                },
                HistorySample::Cumulative {
                    time_s: 2.0,
                    cumulative: 3.0,
                    reset: false,
                    quality: QualityFlag::Good,
                },
                HistorySample::Cumulative {
                    time_s: 4.0,
                    cumulative: 0.0, // rollover: 3 → 0 (mod 4) = +1
                    reset: false,
                    quality: QualityFlag::Good,
                },
            ],
        };
        let h = history(vec![stream.clone()]);
        h.validate().unwrap();
        let rates = h.interval_rates("ctr").unwrap();
        assert_eq!(rates.len(), 2);
        assert!((rates[0].rate - 1.0).abs() < 1e-12); // (3−1)/2 s
        assert!((rates[1].rate - 0.5).abs() < 1e-12); // (0+4−3)/2 s
        // Session clock offset applied.
        assert_eq!(rates[0].t_start_s, 100.0);
        assert_eq!(rates[1].t_end_s, 104.0);

        // Unexplained decrease (no bits, no reset) fails validation.
        let mut bad = history(vec![stream]);
        bad.streams[0].counter_bits = None;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn integrated_counts_convert_once_to_rate() {
        // 200 counts over [10, 30) s → 10 counts/s, exactly once.
        let h = history(vec![HistoryStream {
            id: "ic".into(),
            kind: StreamKind::IntegratedCounts,
            unit: "counts".into(),
            clock: clock(),
            available_at_seconds: None,
            coordinate_frame: None,
            counter_bits: None,
            beam: None,
            samples: vec![
                HistorySample::Integrated {
                    t_start_s: 10.0,
                    t_end_s: 30.0,
                    counts: 200.0,
                    quality: QualityFlag::Good,
                },
                HistorySample::Integrated {
                    t_start_s: 40.0,
                    t_end_s: 60.0,
                    counts: 100.0,
                    quality: QualityFlag::Good,
                },
            ],
        }]);
        h.validate().unwrap();
        let rates = h.interval_rates("ic").unwrap();
        assert_eq!(rates.len(), 2);
        assert!((rates[0].rate - 10.0).abs() < 1e-12);
        assert!((rates[1].rate - 5.0).abs() < 1e-12);
        // The gap [130, 140) session s is reported, never zeroed.
        let gaps = h.stream_gaps("ic", 100.0, 200.0).unwrap();
        assert_eq!(gaps, vec![(100.0, 110.0), (130.0, 140.0), (160.0, 200.0)]);
    }

    #[test]
    fn duplicate_and_unordered_samples_rejected_after_import() {
        // Validation is strict — the importer is what resolves them.
        let h = history(vec![HistoryStream {
            id: "i".into(),
            kind: StreamKind::Instantaneous,
            unit: "uA".into(),
            clock: clock(),
            available_at_seconds: None,
            coordinate_frame: None,
            counter_bits: None,
            beam: None,
            samples: vec![
                HistorySample::Instantaneous {
                    time_s: 1.0,
                    value: 10.0,
                    quality: QualityFlag::Good,
                },
                HistorySample::Instantaneous {
                    time_s: 1.0,
                    value: 11.0,
                    quality: QualityFlag::Good,
                },
            ],
        }]);
        assert!(h.validate().is_err());
    }

    #[test]
    fn beam_state_stream_reports_open_interval() {
        let h = history(vec![HistoryStream {
            id: "st".into(),
            kind: StreamKind::BeamState,
            unit: "1".into(),
            clock: clock(),
            available_at_seconds: None,
            coordinate_frame: None,
            counter_bits: None,
            beam: None,
            samples: vec![
                HistorySample::State {
                    time_s: 0.0,
                    state: BeamState::On,
                    quality: QualityFlag::Good,
                },
                HistorySample::State {
                    time_s: 10.0,
                    state: BeamState::Off,
                    quality: QualityFlag::Good,
                },
                HistorySample::State {
                    time_s: 20.0,
                    state: BeamState::On,
                    quality: QualityFlag::Good,
                },
            ],
        }]);
        h.validate().unwrap();
        let on = h.beam_on_intervals("st").unwrap();
        assert_eq!(on, vec![(100.0, Some(110.0)), (120.0, None)]);
    }

    #[test]
    fn csv_import_reorders_counts_and_reports_policies() {
        let spec = CsvStreamSpec {
            stream_id: "monitor".into(),
            kind: StreamKind::CumulativeCounter,
            unit: "counts".into(),
            clock: clock(),
            csv: "monitor.csv".into(),
            columns: CsvColumns {
                time: "t_s".into(),
                time_end: None,
                value: "cum".into(),
                quality: Some("q".into()),
                reset: Some("reset".into()),
            },
            beam: None,
            available_at_seconds: None,
            counter_bits: Some(4),
        };
        let csv = "t_s,cum,q,reset,extra\n\
                   0,10,good,0,x\n\
                   4,25,good,0,x\n\
                   2,15,good,0,x\n\
                   2,15,good,0,x\n\
                   6,1,good,reset,x\n\
                   8,5,good,0,x\n";
        let (stream, diag) = import_stream_csv(&spec, ',', '#', csv).unwrap();
        assert_eq!(diag.rows_parsed, 6);
        assert!(diag.reordered > 0);
        assert_eq!(diag.duplicates_dropped, 1);
        assert_eq!(diag.extra_columns, vec!["extra".to_string()]);
        // Sorted: 0,2,4,6(reset),8 → the reset drops the 4→6 bridge.
        let rates = history(vec![stream]).interval_rates("monitor").unwrap();
        assert_eq!(rates.len(), 3);
        assert!((rates[0].rate - 2.5).abs() < 1e-12); // (15−10)/2
        assert!((rates[1].rate - 5.0).abs() < 1e-12); // (25−15)/2
        assert!((rates[2].rate - 2.0).abs() < 1e-12); // (5−1)/2 — reset boundary
    }

    #[test]
    fn frame_and_calibration_policies() {
        // Mismatched stream frame rejected.
        let mut h = history(vec![]);
        h.streams.push(HistoryStream {
            id: "pos".into(),
            kind: StreamKind::Instantaneous,
            unit: "mm".into(),
            clock: clock(),
            available_at_seconds: None,
            coordinate_frame: Some("other-frame".into()),
            counter_bits: None,
            beam: None,
            samples: vec![HistorySample::Instantaneous {
                time_s: 0.0,
                value: 1.0,
                quality: QualityFlag::Good,
            }],
        });
        assert!(h.validate().is_err());
        // Expired calibration is a queryable fact, not a silent pass.
        let h = history(vec![]);
        assert!(h.expired_calibrations(3700.0).len() == 1);
        assert!(h.expired_calibrations(1800.0).is_empty());
    }
}
