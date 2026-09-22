// SPDX-License-Identifier: MIT

//! Deterministic comparison of a response-treatment candidate against a
//! rejected transported-photon suitability baseline.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    NjoySuitabilityQualification, NjoySuitabilityReportDocument, NjoySuitabilityStatus,
    NjoyTransportRequirement,
};

pub const NJOY_SUITABILITY_COMPARISON_SCHEMA: &str =
    "openbnct.response-treatment-candidate-comparison/0.1.0";

const COMPARISON_ID_SEGMENT: &str = "response-treatment-comparison";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySuitabilityComparison {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub requirement: NjoyTransportRequirement,
    pub qualification: NjoySuitabilityComparisonQualification,
    pub baseline_report: ContentReference,
    pub candidate_report: ContentReference,
    pub runs: Vec<NjoySuitabilityComparisonRun>,
    pub baseline_rejected_run_count: u64,
    pub candidate_rejected_run_count: u64,
    pub resolved_baseline_rejection_count: u64,
    pub introduced_rejection_count: u64,
    pub baseline_kinematic_violation_count: u64,
    pub candidate_kinematic_violation_count: u64,
    pub baseline_processor_finding_count: u64,
    pub candidate_processor_finding_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoySuitabilityComparisonQualification {
    CandidateRejected,
    CandidateMechanicalGateClearUnreviewed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySuitabilityComparisonRun {
    pub nuclide: String,
    pub baseline_suitability: NjoySuitabilityStatus,
    pub candidate_suitability: NjoySuitabilityStatus,
    pub baseline_kinematic_violation_count: u64,
    pub candidate_kinematic_violation_count: u64,
    pub baseline_processor_finding_count: u64,
    pub candidate_processor_finding_count: u64,
    pub outcome: NjoySuitabilityComparisonOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoySuitabilityComparisonOutcome {
    RemainedCandidateUnreviewed,
    BaselineRejectionResolvedPendingReview,
    RejectionIntroduced,
    RemainedRejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NjoySuitabilityComparisonDocument {
    pub comparison: NjoySuitabilityComparison,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NjoySuitabilityComparisonResult {
    pub comparison: NjoySuitabilityComparison,
    pub comparison_path: PathBuf,
    pub comparison_sha256: String,
}

impl NjoySuitabilityComparison {
    pub fn compare(
        baseline: &NjoySuitabilityReportDocument,
        candidate: &NjoySuitabilityReportDocument,
    ) -> Result<Self, NjoySuitabilityComparisonError> {
        baseline.report.validate()?;
        candidate.report.validate()?;
        if baseline.report.qualification
            != NjoySuitabilityQualification::TransportedPhotonKermaRejected
        {
            return invalid_comparison("baseline report is not rejected evidence");
        }
        if baseline.report.case_id != candidate.report.case_id
            || baseline.report.requirement != candidate.report.requirement
        {
            return invalid_comparison("baseline and candidate scopes differ");
        }
        if baseline.sha256 == candidate.sha256 {
            return invalid_comparison("baseline and candidate reports are identical");
        }
        if baseline.report.runs.len() != candidate.report.runs.len() {
            return invalid_comparison("baseline and candidate nuclide sets differ");
        }

        let mut runs = Vec::with_capacity(baseline.report.runs.len());
        for (baseline_run, candidate_run) in baseline.report.runs.iter().zip(&candidate.report.runs)
        {
            if baseline_run.nuclide != candidate_run.nuclide {
                return invalid_comparison("baseline and candidate nuclide sets differ");
            }
            runs.push(NjoySuitabilityComparisonRun {
                nuclide: baseline_run.nuclide.clone(),
                baseline_suitability: baseline_run.suitability,
                candidate_suitability: candidate_run.suitability,
                baseline_kinematic_violation_count: baseline_run.diagnostic_violation_count,
                candidate_kinematic_violation_count: candidate_run.diagnostic_violation_count,
                baseline_processor_finding_count: baseline_run.processor_data_findings.len() as u64,
                candidate_processor_finding_count: candidate_run.processor_data_findings.len()
                    as u64,
                outcome: comparison_outcome(baseline_run.suitability, candidate_run.suitability),
            });
        }

        let resolved_baseline_rejection_count = runs
            .iter()
            .filter(|run| {
                run.outcome
                    == NjoySuitabilityComparisonOutcome::BaselineRejectionResolvedPendingReview
            })
            .count() as u64;
        let introduced_rejection_count = runs
            .iter()
            .filter(|run| run.outcome == NjoySuitabilityComparisonOutcome::RejectionIntroduced)
            .count() as u64;
        let qualification = if candidate.report.rejected_run_count == 0 {
            NjoySuitabilityComparisonQualification::CandidateMechanicalGateClearUnreviewed
        } else {
            NjoySuitabilityComparisonQualification::CandidateRejected
        };
        let comparison = Self {
            schema_version: NJOY_SUITABILITY_COMPARISON_SCHEMA.into(),
            id: format!(
                "{}.{}.{}",
                candidate.report.id,
                COMPARISON_ID_SEGMENT,
                &baseline.sha256[..12]
            ),
            case_id: baseline.report.case_id.clone(),
            requirement: baseline.report.requirement,
            qualification,
            baseline_report: ContentReference {
                id: baseline.report.id.clone(),
                sha256: baseline.sha256.clone(),
            },
            candidate_report: ContentReference {
                id: candidate.report.id.clone(),
                sha256: candidate.sha256.clone(),
            },
            runs,
            baseline_rejected_run_count: baseline.report.rejected_run_count,
            candidate_rejected_run_count: candidate.report.rejected_run_count,
            resolved_baseline_rejection_count,
            introduced_rejection_count,
            baseline_kinematic_violation_count: baseline.report.kinematic_violation_count,
            candidate_kinematic_violation_count: candidate.report.kinematic_violation_count,
            baseline_processor_finding_count: baseline.report.processor_finding_count,
            candidate_processor_finding_count: candidate.report.processor_finding_count,
        };
        comparison.validate()?;
        Ok(comparison)
    }

    pub fn validate(&self) -> Result<(), NjoySuitabilityComparisonError> {
        if !openbnct_core::schema_matches(&self.schema_version, NJOY_SUITABILITY_COMPARISON_SCHEMA)
        {
            return invalid_comparison(format!("unsupported schema {:?}", self.schema_version));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("case_id", self.case_id.as_str()),
            ("baseline_report.id", self.baseline_report.id.as_str()),
            ("candidate_report.id", self.candidate_report.id.as_str()),
        ] {
            if value.trim().is_empty() {
                return invalid_comparison(format!("{label} is empty"));
            }
        }
        validate_sha256("baseline_report.sha256", &self.baseline_report.sha256)?;
        validate_sha256("candidate_report.sha256", &self.candidate_report.sha256)?;
        if self.baseline_report == self.candidate_report {
            return invalid_comparison("baseline and candidate references are identical");
        }
        let expected_id = format!(
            "{}.{}.{}",
            self.candidate_report.id,
            COMPARISON_ID_SEGMENT,
            &self.baseline_report.sha256[..12]
        );
        if self.id != expected_id {
            return invalid_comparison("comparison ID does not bind both input reports");
        }
        if self.runs.is_empty() {
            return invalid_comparison("comparison contains no nuclide runs");
        }

        let mut previous_nuclide: Option<&str> = None;
        let mut baseline_rejected = 0_u64;
        let mut candidate_rejected = 0_u64;
        let mut resolved = 0_u64;
        let mut introduced = 0_u64;
        let mut baseline_violations = 0_u64;
        let mut candidate_violations = 0_u64;
        let mut baseline_findings = 0_u64;
        let mut candidate_findings = 0_u64;
        for run in &self.runs {
            validate_nuclide(&run.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= run.nuclide.as_str()) {
                return invalid_comparison("comparison runs are not strictly ordered");
            }
            previous_nuclide = Some(&run.nuclide);
            validate_run(run)?;
            baseline_rejected +=
                u64::from(run.baseline_suitability == NjoySuitabilityStatus::Rejected);
            candidate_rejected +=
                u64::from(run.candidate_suitability == NjoySuitabilityStatus::Rejected);
            resolved += u64::from(
                run.outcome
                    == NjoySuitabilityComparisonOutcome::BaselineRejectionResolvedPendingReview,
            );
            introduced +=
                u64::from(run.outcome == NjoySuitabilityComparisonOutcome::RejectionIntroduced);
            baseline_violations += run.baseline_kinematic_violation_count;
            candidate_violations += run.candidate_kinematic_violation_count;
            baseline_findings += run.baseline_processor_finding_count;
            candidate_findings += run.candidate_processor_finding_count;
        }
        if baseline_rejected == 0 {
            return invalid_comparison("baseline comparison evidence is not rejected");
        }
        if baseline_rejected != self.baseline_rejected_run_count
            || candidate_rejected != self.candidate_rejected_run_count
            || resolved != self.resolved_baseline_rejection_count
            || introduced != self.introduced_rejection_count
            || baseline_violations != self.baseline_kinematic_violation_count
            || candidate_violations != self.candidate_kinematic_violation_count
            || baseline_findings != self.baseline_processor_finding_count
            || candidate_findings != self.candidate_processor_finding_count
        {
            return invalid_comparison("comparison aggregate counts do not match run records");
        }
        let expected_qualification = if candidate_rejected == 0 {
            NjoySuitabilityComparisonQualification::CandidateMechanicalGateClearUnreviewed
        } else {
            NjoySuitabilityComparisonQualification::CandidateRejected
        };
        if self.qualification != expected_qualification {
            return invalid_comparison("comparison qualification does not match candidate runs");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoySuitabilityComparisonResult, NjoySuitabilityComparisonError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoySuitabilityComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoySuitabilityComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoySuitabilityComparisonResult {
            comparison: self.clone(),
            comparison_path: path.to_path_buf(),
            comparison_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoySuitabilityComparisonDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoySuitabilityComparisonError> {
        let comparison: NjoySuitabilityComparison = serde_json::from_slice(bytes)?;
        comparison.validate()?;
        Ok(Self {
            comparison,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoySuitabilityComparisonError> {
        let metadata =
            fs::symlink_metadata(path).map_err(|source| NjoySuitabilityComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(NjoySuitabilityComparisonError::NotRegularFile(
                path.to_path_buf(),
            ));
        }
        let bytes = fs::read(path).map_err(|source| NjoySuitabilityComparisonError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }

    pub fn verify_against_reports(
        &self,
        baseline: &NjoySuitabilityReportDocument,
        candidate: &NjoySuitabilityReportDocument,
    ) -> Result<(), NjoySuitabilityComparisonError> {
        let observed = NjoySuitabilityComparison::compare(baseline, candidate)?;
        if observed != self.comparison {
            return Err(NjoySuitabilityComparisonError::ComparisonMismatch);
        }
        Ok(())
    }
}

fn validate_run(run: &NjoySuitabilityComparisonRun) -> Result<(), NjoySuitabilityComparisonError> {
    let baseline_rejected =
        run.baseline_kinematic_violation_count > 0 || run.baseline_processor_finding_count > 0;
    let candidate_rejected =
        run.candidate_kinematic_violation_count > 0 || run.candidate_processor_finding_count > 0;
    if baseline_rejected != (run.baseline_suitability == NjoySuitabilityStatus::Rejected)
        || candidate_rejected != (run.candidate_suitability == NjoySuitabilityStatus::Rejected)
    {
        return invalid_comparison(format!(
            "run {} suitability does not match its evidence counts",
            run.nuclide
        ));
    }
    let expected = comparison_outcome(run.baseline_suitability, run.candidate_suitability);
    if run.outcome != expected {
        return invalid_comparison(format!(
            "run {} outcome does not match its suitability transition",
            run.nuclide
        ));
    }
    Ok(())
}

fn comparison_outcome(
    baseline: NjoySuitabilityStatus,
    candidate: NjoySuitabilityStatus,
) -> NjoySuitabilityComparisonOutcome {
    match (baseline, candidate) {
        (
            NjoySuitabilityStatus::CandidateUnreviewed,
            NjoySuitabilityStatus::CandidateUnreviewed,
        ) => NjoySuitabilityComparisonOutcome::RemainedCandidateUnreviewed,
        (NjoySuitabilityStatus::Rejected, NjoySuitabilityStatus::CandidateUnreviewed) => {
            NjoySuitabilityComparisonOutcome::BaselineRejectionResolvedPendingReview
        }
        (NjoySuitabilityStatus::CandidateUnreviewed, NjoySuitabilityStatus::Rejected) => {
            NjoySuitabilityComparisonOutcome::RejectionIntroduced
        }
        (NjoySuitabilityStatus::Rejected, NjoySuitabilityStatus::Rejected) => {
            NjoySuitabilityComparisonOutcome::RemainedRejected
        }
    }
}

fn validate_nuclide(value: &str) -> Result<(), NjoySuitabilityComparisonError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        invalid_comparison("run.nuclide is not a canonical path-safe name")
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), NjoySuitabilityComparisonError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_comparison(format!("{label} is not a canonical lowercase SHA-256"))
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_comparison<T>(message: impl Into<String>) -> Result<T, NjoySuitabilityComparisonError> {
    Err(NjoySuitabilityComparisonError::InvalidComparison(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoySuitabilityComparisonError {
    #[error(transparent)]
    Suitability(#[from] crate::NjoySuitabilityError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid NJOY response-treatment comparison: {0}")]
    InvalidComparison(String),
    #[error("NJOY response-treatment comparison does not match its input reports")]
    ComparisonMismatch,
    #[error("required comparison artifact is not a real regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-transported-photon-suitability.json"
    );
    const JEFF40_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-suitability.json"
    );
    const FROZEN_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/endfb81-vs-jeff40-response-treatment-comparison.json"
    );

    #[test]
    fn compares_jeff40_against_the_rejected_endfb81_baseline() {
        let baseline = NjoySuitabilityReportDocument::from_bytes(BASELINE_REPORT).unwrap();
        let candidate = NjoySuitabilityReportDocument::from_bytes(JEFF40_REPORT).unwrap();
        let comparison = NjoySuitabilityComparison::compare(&baseline, &candidate).unwrap();

        assert_eq!(comparison.baseline_rejected_run_count, 4);
        assert_eq!(comparison.candidate_rejected_run_count, 6);
        assert_eq!(comparison.resolved_baseline_rejection_count, 0);
        assert_eq!(comparison.introduced_rejection_count, 2);
        assert_eq!(comparison.baseline_kinematic_violation_count, 72);
        assert_eq!(comparison.candidate_kinematic_violation_count, 120);
        assert_eq!(comparison.baseline_processor_finding_count, 4);
        assert_eq!(comparison.candidate_processor_finding_count, 3);
        assert_eq!(
            comparison.qualification,
            NjoySuitabilityComparisonQualification::CandidateRejected
        );
        assert_eq!(
            comparison
                .runs
                .iter()
                .find(|run| run.nuclide == "C13")
                .unwrap()
                .outcome,
            NjoySuitabilityComparisonOutcome::RejectionIntroduced
        );
        assert_eq!(
            comparison
                .runs
                .iter()
                .find(|run| run.nuclide == "N15")
                .unwrap()
                .outcome,
            NjoySuitabilityComparisonOutcome::RemainedRejected
        );
    }

    #[test]
    fn rejects_a_comparison_with_itself_or_tampered_aggregates() {
        let baseline = NjoySuitabilityReportDocument::from_bytes(BASELINE_REPORT).unwrap();
        assert!(matches!(
            NjoySuitabilityComparison::compare(&baseline, &baseline),
            Err(NjoySuitabilityComparisonError::InvalidComparison(_))
        ));

        let candidate = NjoySuitabilityReportDocument::from_bytes(JEFF40_REPORT).unwrap();
        let mut comparison = NjoySuitabilityComparison::compare(&baseline, &candidate).unwrap();
        comparison.candidate_rejected_run_count = 0;
        assert!(matches!(
            comparison.validate(),
            Err(NjoySuitabilityComparisonError::InvalidComparison(_))
        ));
    }

    #[test]
    fn frozen_comparison_regenerates_exactly() {
        let baseline = NjoySuitabilityReportDocument::from_bytes(BASELINE_REPORT).unwrap();
        let candidate = NjoySuitabilityReportDocument::from_bytes(JEFF40_REPORT).unwrap();
        let document = NjoySuitabilityComparisonDocument::from_bytes(FROZEN_COMPARISON).unwrap();

        document
            .verify_against_reports(&baseline, &candidate)
            .unwrap();
        assert_eq!(
            document.sha256,
            "bd6c63ac973f83e4872c9c17175dc8c2b10a815f095e3c6febb4023426698b03"
        );
    }
}
