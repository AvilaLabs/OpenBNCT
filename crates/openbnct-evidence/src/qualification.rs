// SPDX-License-Identifier: MIT

//! Qualification-readiness record
//! (`openbnct.qualification-record/0.1.0`).
//!
//! A claim/evidence matrix over the BENCH-01 catalogue: every positive
//! claim names its supporting record and applicability domain; claim
//! levels separate software conformance, numerical verification,
//! measurement comparison, and external reproduction from anything
//! clinical or facility-scoped — which this record can only ever list
//! as an external dependency, never as held.

use std::collections::BTreeSet;

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Record contract token.
pub const QUALIFICATION_RECORD_SCHEMA: &str = "openbnct.qualification-record/0.1.0";

#[derive(Debug, Error)]
pub enum QualificationError {
    #[error("unsupported schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid qualification record: {0}")]
    Invalid(String),
}

/// The strength of a claim — ordered weakest to strongest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimLevel {
    /// Code exists and conforms to its contract.
    SoftwareConformance,
    /// Verified against an analytic or independent numerical result.
    NumericalVerification,
    /// Compared against a measurement or benchmark record.
    MeasurementComparison,
    /// Independently reproduced outside this codebase.
    ExternalReproduction,
    /// Clinical or facility qualification — never `held` in this tree.
    ClinicalQualification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Supported by at least one bound evidence record now.
    Held,
    /// Assessed but the evidence does not establish it.
    NotHeld,
    /// Requires activity outside this repository — explicitly not a
    /// claim the software makes.
    ExternalDependency,
    /// Planned or unexecuted — must never be reported as held.
    Planned,
}

/// Content-bound evidence for a claim: a catalogue entry id plus the
/// artifact hash — and optionally the specific artifact `file` name
/// within the entry, so the binding names what it means rather than
/// matching any hash the entry happens to carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimEvidence {
    #[serde(flatten)]
    pub reference: ContentReference,
    /// File name of the artifact within the entry, e.g.
    /// `comparison.json`. When set, verification requires the named
    /// artifact — not just any entry evidence — to carry the hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
}

/// One claim with its evidence and scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationClaim {
    pub id: String,
    /// What is claimed, in one sentence.
    pub claim: String,
    pub level: ClaimLevel,
    pub status: ClaimStatus,
    /// Supporting evidence — catalogue entry ids or content-bound
    /// artifact references. Required for `held`.
    #[serde(default)]
    pub evidence: Vec<ClaimEvidence>,
    /// Applicability domain — the input domain, geometry, model family,
    /// or dataset class the claim extends to. Required for `held`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applicability: Option<String>,
    /// Known failure modes / unresolved assumptions.
    #[serde(default)]
    pub limitations: Vec<String>,
    /// For `external_dependency` / `planned`: what external activity
    /// the claim waits on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_requirement: Option<String>,
}

/// The readiness record: claims, the software/data versions evaluated,
/// and the honest inventory of what exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationRecord {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Software version(s) the record evaluates (crate versions /
    /// commit ids).
    pub software_versions: Vec<String>,
    /// Catalogue/artifact versions the claims draw on.
    pub data_versions: Vec<String>,
    pub claims: Vec<QualificationClaim>,
    /// Records the project does NOT have — listed honestly, never
    /// omitted (e.g. "no per-case data-license declarations").
    #[serde(default)]
    pub absent_records: Vec<String>,
    pub qualification: String,
    pub provenance_id: String,
}

impl QualificationRecord {
    pub fn validate(&self) -> Result<(), QualificationError> {
        if !openbnct_core::schema_matches(&self.schema_version, QUALIFICATION_RECORD_SCHEMA) {
            return Err(QualificationError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        let mut ids = BTreeSet::new();
        for claim in &self.claims {
            if !ids.insert(claim.id.as_str()) {
                return Err(invalid(format!("duplicate claim id {:?}", claim.id)));
            }
            match claim.status {
                ClaimStatus::Held => {
                    if claim.evidence.is_empty() {
                        return Err(invalid(format!(
                            "claim {:?} is held but binds no evidence record",
                            claim.id
                        )));
                    }
                    if claim.applicability.is_none() {
                        return Err(invalid(format!(
                            "claim {:?} is held but declares no applicability domain",
                            claim.id
                        )));
                    }
                    if claim.level == ClaimLevel::ClinicalQualification {
                        return Err(invalid(format!(
                            "claim {:?}: clinical/facility qualification cannot be `held` in a research record — record it as external_dependency",
                            claim.id
                        )));
                    }
                }
                ClaimStatus::Planned | ClaimStatus::ExternalDependency => {
                    if claim.external_requirement.is_none() {
                        return Err(invalid(format!(
                            "claim {:?}: {:?} status requires external_requirement",
                            claim.id, claim.status
                        )));
                    }
                }
                ClaimStatus::NotHeld => {}
            }
        }
        Ok(())
    }

    /// Cross-check every claim's evidence against the benchmark
    /// catalogue: the referenced entry must exist AND the bound hash
    /// must match an artifact that entry actually carries — a claim
    /// bound to a hash the catalogue doesn't know is stale or
    /// misbound, not verified.
    pub fn verify_against(
        &self,
        catalogue: &crate::BenchmarkCatalogue,
    ) -> Result<Vec<String>, QualificationError> {
        let entries: BTreeSet<&str> = catalogue.entries.iter().map(|e| e.id.as_str()).collect();
        let mut missing = Vec::new();
        for claim in &self.claims {
            for evidence in &claim.evidence {
                let reference = &evidence.reference;
                if !entries.contains(reference.id.as_str()) {
                    missing.push(format!(
                        "claim {:?}: evidence {:?} is not a catalogue entry",
                        claim.id, reference.id
                    ));
                    continue;
                }
                let entry = catalogue
                    .entries
                    .iter()
                    .find(|e| e.id == reference.id)
                    .expect("checked above");
                let sha = reference.sha256.trim_start_matches("sha256:");
                let hash_match = |item: &crate::catalogue::EvidenceItem| {
                    item.sha256.as_deref() == Some(sha)
                        && evidence
                            .artifact
                            .as_deref()
                            .is_none_or(|name| item.file.as_deref() == Some(name))
                };
                if !entry.evidence.iter().any(hash_match) {
                    missing.push(format!(
                        "claim {:?}: evidence {}{} does not match an artifact in catalogue entry {:?}",
                        claim.id,
                        reference.sha256,
                        evidence
                            .artifact
                            .as_deref()
                            .map(|a| format!(" ({a})"))
                            .unwrap_or_default(),
                        reference.id
                    ));
                }
            }
        }
        Ok(missing)
    }
}

fn invalid(msg: impl Into<String>) -> QualificationError {
    QualificationError::Invalid(msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ref_(id: &str) -> ClaimEvidence {
        ClaimEvidence {
            reference: ContentReference {
                id: id.into(),
                sha256: "ab".repeat(32),
            },
            artifact: None,
        }
    }

    fn record(claims: Vec<QualificationClaim>) -> QualificationRecord {
        QualificationRecord {
            schema_version: QUALIFICATION_RECORD_SCHEMA.into(),
            id: "qr".into(),
            software_versions: vec!["0.2.2".into()],
            data_versions: vec!["catalogue-0.1.0".into()],
            claims,
            absent_records: vec!["no per-case licenses".into()],
            qualification: "research_only".into(),
            provenance_id: "t".into(),
        }
    }

    #[test]
    fn held_claim_needs_evidence_and_applicability() {
        let c = QualificationClaim {
            id: "c1".into(),
            claim: "analytic benchmark agreement".into(),
            level: ClaimLevel::NumericalVerification,
            status: ClaimStatus::Held,
            evidence: vec![],
            applicability: None,
            limitations: vec![],
            external_requirement: None,
        };
        assert!(record(vec![c.clone()]).validate().is_err());
        let mut c2 = c.clone();
        c2.evidence = vec![ref_("entry")];
        assert!(record(vec![c2]).validate().is_err()); // no applicability
        let mut c3 = c;
        c3.evidence = vec![ref_("entry")];
        c3.applicability = Some("synthetic phantom".into());
        record(vec![c3]).validate().unwrap();
    }

    #[test]
    fn clinical_qualification_cannot_be_held() {
        let c = QualificationClaim {
            id: "c1".into(),
            claim: "clinical TPS equivalence".into(),
            level: ClaimLevel::ClinicalQualification,
            status: ClaimStatus::Held,
            evidence: vec![ref_("entry")],
            applicability: Some("none".into()),
            limitations: vec![],
            external_requirement: None,
        };
        assert!(record(vec![c]).validate().is_err());
    }

    #[test]
    fn planned_requires_external_requirement() {
        let c = QualificationClaim {
            id: "c1".into(),
            claim: "phantom measurement comparison".into(),
            level: ClaimLevel::MeasurementComparison,
            status: ClaimStatus::Planned,
            evidence: vec![],
            applicability: None,
            limitations: vec![],
            external_requirement: None,
        };
        assert!(record(vec![c]).validate().is_err());
    }
}
