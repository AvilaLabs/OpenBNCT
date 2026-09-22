// SPDX-License-Identifier: MIT

//! Compact, deterministic diagnostic-triage result for external orchestrators.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};

use crate::{
    EndfMf6CapturePhotonBalanceReportDocument, EndfMf6Law7ImplicitResidualReportDocument,
    NjoyCapturePhotonMomentComparisonDocument, NjoyDiagnosticTriageError,
    NjoyDiagnosticTriageQualification, NjoyDiagnosticTriageReportDocument,
    NjoyDomainAwareSuitabilityReportDocument, NjoyEvidenceAwareSuitabilityError,
    NjoyEvidenceAwareSuitabilityReportDocument, NjoyLaw7ImplicitResidualComparisonDocument,
    NjoySuitabilityQualification,
};

pub const NJOY_DIAGNOSTIC_TRIAGE_CHECK_SCHEMA: &str = "openbnct.njoy-diagnostic-triage-check/0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyDiagnosticTriageCheckResult {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub source_report: ContentReference,
    pub response_qualification: NjoySuitabilityQualification,
    pub triage_qualification: NjoyDiagnosticTriageQualification,
    pub original_remaining_in_domain_kinematic_violation_count: u64,
    pub source_data_blocked_in_domain_kinematic_violation_count: u64,
    pub independent_diagnostic_required_in_domain_kinematic_violation_count: u64,
    pub verification: NjoyDiagnosticTriageCheckVerification,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyDiagnosticTriageCheckVerification {
    RegeneratedAndMatched,
}

impl NjoyDiagnosticTriageCheckResult {
    #[allow(clippy::too_many_arguments)]
    pub fn verify_and_build(
        triage: &NjoyDiagnosticTriageReportDocument,
        evidence: &NjoyEvidenceAwareSuitabilityReportDocument,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        law7_residual: &EndfMf6Law7ImplicitResidualReportDocument,
        law7_attribution: &NjoyLaw7ImplicitResidualComparisonDocument,
        capture_balance: &EndfMf6CapturePhotonBalanceReportDocument,
        capture_comparison: &NjoyCapturePhotonMomentComparisonDocument,
    ) -> Result<Self, NjoyDiagnosticTriageCheckError> {
        evidence.verify_against_evidence(
            domain,
            law7_residual,
            law7_attribution,
            capture_balance,
            capture_comparison,
        )?;
        triage.verify_against_evidence(evidence, domain)?;
        Ok(Self::from_verified(triage))
    }

    fn from_verified(triage: &NjoyDiagnosticTriageReportDocument) -> Self {
        Self {
            schema_version: NJOY_DIAGNOSTIC_TRIAGE_CHECK_SCHEMA.into(),
            source_report: ContentReference {
                id: triage.report.id.clone(),
                sha256: triage.sha256.clone(),
            },
            response_qualification: triage.report.response_qualification,
            triage_qualification: triage.report.qualification,
            original_remaining_in_domain_kinematic_violation_count: triage
                .report
                .original_remaining_in_domain_kinematic_violation_count,
            source_data_blocked_in_domain_kinematic_violation_count: triage
                .report
                .source_data_blocked_in_domain_kinematic_violation_count,
            independent_diagnostic_required_in_domain_kinematic_violation_count: triage
                .report
                .independent_diagnostic_required_in_domain_kinematic_violation_count,
            verification: NjoyDiagnosticTriageCheckVerification::RegeneratedAndMatched,
            limitations: vec![
                "Source-data-blocked findings remain rejecting and are not waived by diagnostic triage.".into(),
                "This check does not approve response tables, qualify transport, or establish clinical validity.".into(),
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

#[derive(Debug, thiserror::Error)]
pub enum NjoyDiagnosticTriageCheckError {
    #[error(transparent)]
    EvidenceAware(#[from] NjoyEvidenceAwareSuitabilityError),
    #[error(transparent)]
    DiagnosticTriage(#[from] NjoyDiagnosticTriageError),
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIAGE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-diagnostic-triage.json"
    );

    #[test]
    fn rejected_response_and_open_queue_are_machine_data() {
        let triage = NjoyDiagnosticTriageReportDocument::from_bytes(TRIAGE).unwrap();
        let result = NjoyDiagnosticTriageCheckResult::from_verified(&triage);
        assert_eq!(
            result.response_qualification,
            NjoySuitabilityQualification::TransportedPhotonKermaRejected
        );
        assert_eq!(
            result.triage_qualification,
            NjoyDiagnosticTriageQualification::IndependentReactionDiagnosticsRequired
        );
        assert_eq!(
            result.original_remaining_in_domain_kinematic_violation_count,
            102
        );
        assert_eq!(
            result.source_data_blocked_in_domain_kinematic_violation_count,
            59
        );
        assert_eq!(
            result.independent_diagnostic_required_in_domain_kinematic_violation_count,
            43
        );
    }
}
