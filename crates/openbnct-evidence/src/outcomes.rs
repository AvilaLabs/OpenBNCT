// SPDX-License-Identifier: MIT

//! Dose-to-outcome research exports
//! (`openbnct.outcomes-export/0.1.0`).
//!
//! A study export links participants → courses → sessions → dose
//! records → outcome observations, with endpoint definitions and
//! versions, censoring/competing events, and explicit missingness
//! reasons. Validation enforces the linkage and chronology rules that
//! make repeated sessions count once, missing follow-up distinct from
//! no toxicity, and endpoint systems unmixable.
//!
//! `export_fields` applies a declared whitelist — only configured
//! fields are emitted and every exclusion is reported. That is a
//! software check, not a certification of anonymization.

use std::collections::{BTreeMap, BTreeSet};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Export contract token.
pub const OUTCOMES_EXPORT_SCHEMA: &str = "openbnct.outcomes-export/0.1.0";

#[derive(Debug, Error)]
pub enum OutcomesError {
    #[error("unsupported schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid outcomes export: {0}")]
    Invalid(String),
}

/// A participant. `id` must be stable *within the approved study* —
/// the export carries no identity fields of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Participant {
    pub id: String,
}

/// One lesion/region of interest the outcomes are scored against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionOfInterest {
    pub id: String,
    pub participant: String,
    /// Region definition basis — the contour/ROI source name and the
    /// mask definition it refers to, so a mismatched definition is
    /// detectable rather than silently shared.
    pub definition: String,
}

/// A treatment course — one participant's sequence of sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Course {
    pub id: String,
    pub participant: String,
    /// Drug protocol identity (compound, enrichment, dose).
    pub drug_protocol: String,
}

/// One irradiation session in a course. A repeated session links to
/// the same participant through the course — it is never an
/// independent record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: String,
    pub course: String,
    /// Session index (1-based, monotonic within the course).
    pub fraction: u32,
    /// Days since course start — relative time, never a date.
    pub day: f64,
}

/// A dose record bound to a session and ROI: planned vs reconstructed
/// stay separately accessible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseRecord {
    pub id: String,
    pub session: String,
    pub roi: String,
    /// `planned` or `reconstructed` — never conflated.
    pub kind: String,
    /// Content reference to the dose artifact (bundle or report).
    pub dose: ContentReference,
    /// Biological model identity applied, if any — the model id and
    /// version stay with the result so a future model can be evaluated
    /// without replacing this record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biological_model: Option<String>,
}

/// Endpoint definition identity — system + version. Different systems
/// (CTCAE v4 vs v5, RANO vs RECIST) are *not* silently combined.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointDefinition {
    pub name: String,
    pub system: String,
    pub version: String,
}

/// One outcome observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeObservation {
    pub id: String,
    pub participant: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roi: Option<String>,
    /// Endpoint definition this observation was scored under.
    pub endpoint: EndpointDefinition,
    /// Days since the course started — never a calendar date.
    pub day: f64,
    /// The value (grade, category, measure) under `endpoint`'s system.
    pub value: String,
    /// `observed` | `censored` | `competing_event` | `missing`.
    pub status: String,
    /// Required for `status: "missing"` — an absent follow-up must
    /// carry a reason, never silently read as no event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missingness_reason: Option<String>,
}

/// The export document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomesExport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Study identifier — the authorization scope for these ids.
    pub study: String,
    pub qualification: String,
    pub provenance_id: String,
    #[serde(default)]
    pub participants: Vec<Participant>,
    #[serde(default)]
    pub rois: Vec<RegionOfInterest>,
    #[serde(default)]
    pub courses: Vec<Course>,
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub dose_records: Vec<DoseRecord>,
    #[serde(default)]
    pub observations: Vec<OutcomeObservation>,
}

impl OutcomesExport {
    /// Structural validation: linkage, chronology, endpoint-system
    /// consistency, missingness semantics.
    pub fn validate(&self) -> Result<(), OutcomesError> {
        if !openbnct_core::schema_matches(&self.schema_version, OUTCOMES_EXPORT_SCHEMA) {
            return Err(OutcomesError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        let participants: BTreeSet<&str> =
            self.participants.iter().map(|p| p.id.as_str()).collect();
        let rois: BTreeMap<&str, &RegionOfInterest> =
            self.rois.iter().map(|r| (r.id.as_str(), r)).collect();
        let courses: BTreeMap<&str, &Course> =
            self.courses.iter().map(|c| (c.id.as_str(), c)).collect();
        let sessions: BTreeMap<&str, &Session> =
            self.sessions.iter().map(|s| (s.id.as_str(), s)).collect();

        for roi in &self.rois {
            if !participants.contains(roi.participant.as_str()) {
                return Err(invalid(format!(
                    "roi {:?}: unknown participant {:?}",
                    roi.id, roi.participant
                )));
            }
        }
        for course in &self.courses {
            if !participants.contains(course.participant.as_str()) {
                return Err(invalid(format!(
                    "course {:?}: unknown participant {:?}",
                    course.id, course.participant
                )));
            }
        }
        // Session linkage: valid course, monotonic fraction↔day.
        let mut per_course: BTreeMap<&str, Vec<&Session>> = BTreeMap::new();
        for session in &self.sessions {
            if !courses.contains_key(session.course.as_str()) {
                return Err(invalid(format!(
                    "session {:?}: unknown course {:?}",
                    session.id, session.course
                )));
            }
            per_course
                .entry(session.course.as_str())
                .or_default()
                .push(session);
        }
        for (course_id, mut list) in per_course {
            list.sort_by_key(|s| s.fraction);
            for w in list.windows(2) {
                if w[0].fraction == w[1].fraction {
                    return Err(invalid(format!(
                        "course {course_id:?}: duplicate fraction {}",
                        w[0].fraction
                    )));
                }
                if w[1].day < w[0].day {
                    return Err(invalid(format!(
                        "course {course_id:?}: fraction {} dated before fraction {} — impossible chronology",
                        w[1].fraction, w[0].fraction
                    )));
                }
            }
        }
        for record in &self.dose_records {
            let session = sessions
                .get(record.session.as_str())
                .ok_or_else(|| invalid(format!("dose record {:?}: unknown session", record.id)))?;
            let roi = rois
                .get(record.roi.as_str())
                .ok_or_else(|| invalid(format!("dose record {:?}: unknown roi", record.id)))?;
            let course = courses.get(session.course.as_str()).expect("validated");
            if roi.participant != course.participant {
                return Err(invalid(format!(
                    "dose record {:?}: roi {:?} belongs to participant {:?} but session {:?} is in {:?}'s course",
                    record.id, roi.id, roi.participant, session.id, course.participant
                )));
            }
            if record.kind != "planned" && record.kind != "reconstructed" {
                return Err(invalid(format!(
                    "dose record {:?}: kind must be `planned` or `reconstructed`, got {:?}",
                    record.id, record.kind
                )));
            }
        }
        // Observations: known participant/ROI, chronology sane, endpoint
        // consistency, and missingness honesty.
        let mut endpoint_systems: BTreeMap<&str, BTreeSet<(&str, &str)>> = BTreeMap::new();
        for obs in &self.observations {
            if !participants.contains(obs.participant.as_str()) {
                return Err(invalid(format!(
                    "observation {:?}: unknown participant {:?}",
                    obs.id, obs.participant
                )));
            }
            if let Some(roi) = &obs.roi {
                let r = rois
                    .get(roi.as_str())
                    .ok_or_else(|| invalid(format!("observation {:?}: unknown roi", obs.id)))?;
                if r.participant != obs.participant {
                    return Err(invalid(format!(
                        "observation {:?}: roi {:?} belongs to a different participant",
                        obs.id, roi
                    )));
                }
            }
            if obs.day < 0.0 {
                return Err(invalid(format!(
                    "observation {:?}: day {} precedes course start",
                    obs.id, obs.day
                )));
            }
            match obs.status.as_str() {
                "observed" | "censored" | "competing_event" | "missing" => {}
                other => {
                    return Err(invalid(format!(
                        "observation {:?}: unknown status {other:?} — use observed|censored|competing_event|missing",
                        obs.id
                    )));
                }
            }
            if obs.status == "missing" && obs.missingness_reason.is_none() {
                return Err(invalid(format!(
                    "observation {:?}: missing follow-up requires a missingness_reason — never encode it as no event",
                    obs.id
                )));
            }
            if obs.status != "missing" && obs.missingness_reason.is_some() {
                return Err(invalid(format!(
                    "observation {:?}: missingness_reason on a {obs} — only `missing` carries one",
                    obs.id,
                    obs = obs.status
                )));
            }
            endpoint_systems
                .entry(obs.endpoint.name.as_str())
                .or_default()
                .insert((obs.endpoint.system.as_str(), obs.endpoint.version.as_str()));
        }
        for (name, systems) in endpoint_systems {
            if systems.len() > 1 {
                return Err(invalid(format!(
                    "endpoint {name:?} is scored under {} different system+version pairs {systems:?} — incompatible systems cannot be combined in one export",
                    systems.len()
                )));
            }
        }
        Ok(())
    }
}

fn invalid(msg: impl Into<String>) -> OutcomesError {
    OutcomesError::Invalid(msg.into())
}

/// Whitelisted top-level field → per-record sub-fields. `export_fields`
/// serializes the export to a JSON value, keeps only the configured
/// fields, and returns the emitted value plus the paths that were
/// excluded — the exclusion list is part of the output, not a footnote.
pub fn export_fields(
    export: &OutcomesExport,
    whitelist: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(serde_json::Value, Vec<String>), OutcomesError> {
    export.validate()?;
    let full = serde_json::to_value(export).map_err(|e| invalid(format!("serialization: {e}")))?;
    let mut kept = serde_json::Map::new();
    let mut excluded = Vec::new();
    if let serde_json::Value::Object(map) = &full {
        for (key, value) in map {
            match whitelist.get(key) {
                // Whole section excluded.
                None => excluded.push(key.clone()),
                Some(fields) if fields.is_empty() => {
                    kept.insert(key.clone(), value.clone());
                }
                // Section kept with sub-field filtering.
                Some(fields) => {
                    if let serde_json::Value::Array(rows) = value {
                        let filtered: Vec<serde_json::Value> = rows
                            .iter()
                            .map(|row| {
                                if let serde_json::Value::Object(obj) = row {
                                    let mut out = serde_json::Map::new();
                                    for (k, v) in obj {
                                        if fields.contains(k) {
                                            out.insert(k.clone(), v.clone());
                                        } else {
                                            excluded.push(format!("{key}.{k}"));
                                        }
                                    }
                                    serde_json::Value::Object(out)
                                } else {
                                    row.clone()
                                }
                            })
                            .collect();
                        kept.insert(key.clone(), serde_json::Value::Array(filtered));
                    } else {
                        kept.insert(key.clone(), value.clone());
                    }
                }
            }
        }
    }
    excluded.sort();
    excluded.dedup();
    Ok((serde_json::Value::Object(kept), excluded))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "ab".repeat(32),
        }
    }

    /// Two-lesion, two-session synthetic case.
    fn study() -> OutcomesExport {
        OutcomesExport {
            schema_version: OUTCOMES_EXPORT_SCHEMA.into(),
            id: "study".into(),
            study: "synthetic-01".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "t".into(),
            participants: vec![Participant { id: "p1".into() }],
            rois: vec![
                RegionOfInterest {
                    id: "lesion-a".into(),
                    participant: "p1".into(),
                    definition: "contour-ct-v1".into(),
                },
                RegionOfInterest {
                    id: "lesion-b".into(),
                    participant: "p1".into(),
                    definition: "contour-ct-v1".into(),
                },
            ],
            courses: vec![Course {
                id: "c1".into(),
                participant: "p1".into(),
                drug_protocol: "bpa-400mg".into(),
            }],
            sessions: vec![
                Session {
                    id: "s1".into(),
                    course: "c1".into(),
                    fraction: 1,
                    day: 0.0,
                },
                Session {
                    id: "s2".into(),
                    course: "c1".into(),
                    fraction: 2,
                    day: 7.0,
                },
            ],
            dose_records: vec![
                DoseRecord {
                    id: "d1".into(),
                    session: "s1".into(),
                    roi: "lesion-a".into(),
                    kind: "planned".into(),
                    dose: ref_("plan-dose"),
                    biological_model: Some("cbe-proto-1".into()),
                },
                DoseRecord {
                    id: "d2".into(),
                    session: "s2".into(),
                    roi: "lesion-b".into(),
                    kind: "reconstructed".into(),
                    dose: ref_("replay-dose"),
                    biological_model: None,
                },
            ],
            observations: vec![
                OutcomeObservation {
                    id: "o1".into(),
                    participant: "p1".into(),
                    roi: Some("lesion-a".into()),
                    endpoint: EndpointDefinition {
                        name: "local_response".into(),
                        system: "rano".into(),
                        version: "2.0".into(),
                    },
                    day: 30.0,
                    value: "partial".into(),
                    status: "observed".into(),
                    missingness_reason: None,
                },
                OutcomeObservation {
                    id: "o2".into(),
                    participant: "p1".into(),
                    roi: Some("lesion-b".into()),
                    endpoint: EndpointDefinition {
                        name: "local_response".into(),
                        system: "rano".into(),
                        version: "2.0".into(),
                    },
                    day: 90.0,
                    value: "".into(),
                    status: "missing".into(),
                    missingness_reason: Some("lost_to_followup".into()),
                },
            ],
        }
    }

    #[test]
    fn multi_lesion_multi_session_export_validates() {
        study().validate().unwrap();
    }

    #[test]
    fn missing_followup_needs_a_reason() {
        let mut e = study();
        e.observations[1].missingness_reason = None;
        assert!(e.validate().is_err());
    }

    #[test]
    fn incompatible_endpoint_systems_rejected() {
        let mut e = study();
        e.observations[1].endpoint.system = "recist".into();
        assert!(e.validate().is_err());
    }

    #[test]
    fn impossible_chronology_rejected() {
        let mut e = study();
        e.sessions[1].day = -3.0; // fraction 2 before fraction 1
        assert!(e.validate().is_err());
    }

    #[test]
    fn cross_participant_dose_reference_rejected() {
        let mut e = study();
        e.participants.push(Participant { id: "p2".into() });
        e.rois.push(RegionOfInterest {
            id: "other".into(),
            participant: "p2".into(),
            definition: "other".into(),
        });
        e.dose_records[0].roi = "other".into();
        assert!(e.validate().is_err());
    }

    #[test]
    fn whitelist_export_reports_exclusions() {
        let e = study();
        let whitelist = BTreeMap::from([
            (
                "participants".to_string(),
                BTreeSet::from(["id".to_string()]),
            ),
            ("sessions".to_string(), BTreeSet::new()),
        ]);
        let (out, excluded) = export_fields(&e, &whitelist).unwrap();
        assert!(out.get("participants").is_some());
        assert!(out.get("observations").is_none());
        assert!(excluded.contains(&"observations".to_string()));
        assert!(excluded.contains(&"dose_records".to_string()));
        // Sessions kept wholesale (empty field set = keep all).
        assert_eq!(out["sessions"].as_array().unwrap().len(), 2);
    }
}
