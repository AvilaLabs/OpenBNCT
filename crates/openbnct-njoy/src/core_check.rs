// SPDX-License-Identifier: MIT

//! A compact, deterministic result for external evidence-loop orchestrators.
//!
//! The result is emitted only after the full evidence-aware report has been
//! regenerated and matched. A rejected scientific candidate is represented
//! as data; it is not confused with failure to perform the verification.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};

use crate::{
    EndfMf6CapturePhotonBalanceReportDocument, EndfMf6Law7ImplicitResidualReportDocument,
    NjoyCapturePhotonMomentComparisonDocument, NjoyDomainAwareSuitabilityReportDocument,
    NjoyEvidenceAwareSuitabilityError, NjoyEvidenceAwareSuitabilityReportDocument,
    NjoyLaw7ImplicitResidualComparisonDocument, NjoySuitabilityComparisonDocument,
    NjoySuitabilityComparisonError, NjoySuitabilityComparisonQualification,
    NjoySuitabilityQualification, NjoySuitabilityReportDocument, NjoyTransportRequirement,
};

pub const NJOY_EVIDENCE_AWARE_CHECK_SCHEMA: &str = "openbnct.njoy-evidence-aware-check/0.1.0";
pub const NJOY_CANDIDATE_COMPARISON_CHECK_SCHEMA: &str =
    "openbnct.njoy-candidate-comparison-check/0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyEvidenceAwareCheckResult {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub source_report: ContentReference,
    pub requirement: NjoyTransportRequirement,
    pub qualification: NjoySuitabilityQualification,
    pub rejected_run_count: u64,
    pub remaining_in_domain_kinematic_violation_count: u64,
    pub verification: NjoyEvidenceAwareCheckVerification,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEvidenceAwareCheckVerification {
    RegeneratedAndMatched,
}

impl NjoyEvidenceAwareCheckResult {
    pub fn verify_and_build(
        report: &NjoyEvidenceAwareSuitabilityReportDocument,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        law7_residual: &EndfMf6Law7ImplicitResidualReportDocument,
        law7_attribution: &NjoyLaw7ImplicitResidualComparisonDocument,
        capture_balance: &EndfMf6CapturePhotonBalanceReportDocument,
        capture_comparison: &NjoyCapturePhotonMomentComparisonDocument,
    ) -> Result<Self, NjoyEvidenceAwareSuitabilityError> {
        report.verify_against_evidence(
            domain,
            law7_residual,
            law7_attribution,
            capture_balance,
            capture_comparison,
        )?;
        Ok(Self::from_verified(report))
    }

    #[must_use]
    fn from_verified(report: &NjoyEvidenceAwareSuitabilityReportDocument) -> Self {
        Self {
            schema_version: NJOY_EVIDENCE_AWARE_CHECK_SCHEMA.into(),
            source_report: ContentReference {
                id: report.report.id.clone(),
                sha256: report.sha256.clone(),
            },
            requirement: report.report.requirement,
            qualification: report.report.qualification,
            rejected_run_count: report.report.rejected_run_count,
            remaining_in_domain_kinematic_violation_count: report
                .report
                .remaining_in_domain_kinematic_violation_count,
            verification: NjoyEvidenceAwareCheckVerification::RegeneratedAndMatched,
            limitations: vec![
                "A mechanically clear result would remain unreviewed; this check does not approve response tables, qualify transport, or establish clinical validity.".into(),
                "The result summarizes one exact evidence-aware report and does not replace its per-nuclide findings or bound source evidence.".into(),
            ],
        }
    }

    pub fn write_new(&self, path: &Path) -> io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()
    }
}

/// Compact result for a verified baseline-versus-candidate comparison.
///
/// A rejected candidate is represented as data so an external orchestrator can
/// distinguish "the candidate was checked and rejected" from "the check itself
/// could not be performed."
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyCandidateComparisonCheckResult {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub source_report: ContentReference,
    pub baseline_report: ContentReference,
    pub candidate_report: ContentReference,
    pub requirement: NjoyTransportRequirement,
    pub candidate_qualification: NjoySuitabilityComparisonQualification,
    pub baseline_rejected_run_count: u64,
    pub candidate_rejected_run_count: u64,
    pub resolved_baseline_rejection_count: u64,
    pub introduced_rejection_count: u64,
    pub baseline_kinematic_violation_count: u64,
    pub candidate_kinematic_violation_count: u64,
    pub verification: NjoyCandidateComparisonCheckVerification,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyCandidateComparisonCheckVerification {
    RegeneratedAndMatched,
}

impl NjoyCandidateComparisonCheckResult {
    pub fn verify_and_build(
        comparison: &NjoySuitabilityComparisonDocument,
        baseline: &NjoySuitabilityReportDocument,
        candidate: &NjoySuitabilityReportDocument,
    ) -> Result<Self, NjoySuitabilityComparisonError> {
        comparison.verify_against_reports(baseline, candidate)?;
        Ok(Self::from_verified(comparison))
    }

    #[must_use]
    fn from_verified(comparison: &NjoySuitabilityComparisonDocument) -> Self {
        Self {
            schema_version: NJOY_CANDIDATE_COMPARISON_CHECK_SCHEMA.into(),
            source_report: ContentReference {
                id: comparison.comparison.id.clone(),
                sha256: comparison.sha256.clone(),
            },
            baseline_report: comparison.comparison.baseline_report.clone(),
            candidate_report: comparison.comparison.candidate_report.clone(),
            requirement: comparison.comparison.requirement,
            candidate_qualification: comparison.comparison.qualification,
            baseline_rejected_run_count: comparison.comparison.baseline_rejected_run_count,
            candidate_rejected_run_count: comparison.comparison.candidate_rejected_run_count,
            resolved_baseline_rejection_count: comparison
                .comparison
                .resolved_baseline_rejection_count,
            introduced_rejection_count: comparison.comparison.introduced_rejection_count,
            baseline_kinematic_violation_count: comparison
                .comparison
                .baseline_kinematic_violation_count,
            candidate_kinematic_violation_count: comparison
                .comparison
                .candidate_kinematic_violation_count,
            verification: NjoyCandidateComparisonCheckVerification::RegeneratedAndMatched,
            limitations: vec![
                "A mechanically clear comparison would remain unreviewed; this check does not approve response tables, qualify transport, or establish clinical validity.".into(),
                "The result summarizes one exact comparison report and does not replace its per-nuclide outcomes or bound source evidence.".into(),
            ],
        }
    }

    pub fn write_new(&self, path: &Path) -> io::Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
        file.write_all(&bytes)?;
        file.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FROZEN_ASSESSMENT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-evidence-aware-suitability.json"
    );

    #[test]
    fn rejected_science_is_data_not_a_verification_error() {
        let report =
            NjoyEvidenceAwareSuitabilityReportDocument::from_bytes(FROZEN_ASSESSMENT).unwrap();
        let result = NjoyEvidenceAwareCheckResult::from_verified(&report);
        assert_eq!(
            result.qualification,
            NjoySuitabilityQualification::TransportedPhotonKermaRejected
        );
        assert_eq!(result.rejected_run_count, 4);
        assert_eq!(result.remaining_in_domain_kinematic_violation_count, 102);
        assert_eq!(
            result.verification,
            NjoyEvidenceAwareCheckVerification::RegeneratedAndMatched
        );
        assert_eq!(
            serde_json::to_value(result).unwrap()["qualification"],
            "transported_photon_kerma_rejected"
        );
    }

    const FROZEN_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/endfb81-tendl2025/provenance/endfb81-vs-endfb81-tendl2025-response-treatment-comparison.json"
    );

    #[test]
    fn rejected_candidate_is_data_not_a_verification_error() {
        let comparison = NjoySuitabilityComparisonDocument::from_bytes(FROZEN_COMPARISON).unwrap();
        let result = NjoyCandidateComparisonCheckResult::from_verified(&comparison);
        assert_eq!(
            result.candidate_qualification,
            NjoySuitabilityComparisonQualification::CandidateRejected
        );
        assert_eq!(result.candidate_rejected_run_count, 4);
        assert_eq!(result.resolved_baseline_rejection_count, 0);
        assert_eq!(result.introduced_rejection_count, 0);
        assert_eq!(
            result.verification,
            NjoyCandidateComparisonCheckVerification::RegeneratedAndMatched
        );
        assert_eq!(
            serde_json::to_value(result).unwrap()["candidate_qualification"],
            "candidate_rejected"
        );
    }
}
