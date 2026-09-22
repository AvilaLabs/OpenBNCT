// SPDX-License-Identifier: MIT

//! Fail-closed triage of the kinematic findings that remain after the v0.4
//! reaction-evidence-aware assessment.
//!
//! This layer does not waive or explain a numerical finding. It separates
//! runs that are already blocked by an exact absence of transported-photon
//! source data from runs that still require an independent reaction-level
//! diagnostic. Both classes remain visible and unsuitable.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    HeatrPhotonSource, NjoyDomainAwareSuitabilityError, NjoyDomainAwareSuitabilityReportDocument,
    NjoyEvidenceAwareSuitabilityError, NjoyEvidenceAwareSuitabilityReportDocument,
    NjoyProcessorFindingDisposition, NjoySuitabilityFindingKind, NjoySuitabilityQualification,
    NjoySuitabilityStatus,
};

pub const NJOY_DIAGNOSTIC_TRIAGE_SCHEMA: &str = "openbnct.njoy-diagnostic-triage/0.1.0";

const REPORT_ID_SUFFIX: &str = "diagnostic-triage-v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyDiagnosticTriageReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyDiagnosticTriageQualification,
    pub response_qualification: NjoySuitabilityQualification,
    pub evidence_aware_suitability_report: ContentReference,
    pub domain_aware_suitability_report: ContentReference,
    pub execution_receipt: ContentReference,
    pub assessment_energy_range_ev: [f64; 2],
    pub runs: Vec<NjoyDiagnosticTriageRun>,
    pub original_remaining_in_domain_kinematic_violation_count: u64,
    pub source_data_blocked_in_domain_kinematic_violation_count: u64,
    pub independent_diagnostic_required_in_domain_kinematic_violation_count: u64,
    pub source_data_blocked_run_count: u64,
    pub independent_diagnostic_required_run_count: u64,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyDiagnosticTriageQualification {
    IndependentReactionDiagnosticsRequired,
    IndependentReactionDiagnosticQueueClearUnreviewed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyDiagnosticTriageRun {
    pub nuclide: String,
    pub heatr_photon_source: HeatrPhotonSource,
    pub response_suitability: NjoySuitabilityStatus,
    pub remaining_in_domain_kinematic_violation_count: u64,
    pub rejecting_nonkinematic_finding_count: u64,
    pub missing_photon_production_finding_count: u64,
    pub disposition: NjoyDiagnosticTriageDisposition,
    pub source_data_blocked_in_domain_kinematic_violation_count: u64,
    pub independent_diagnostic_required_in_domain_kinematic_violation_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyDiagnosticTriageDisposition {
    NoRemainingKinematicFinding,
    BlockedByMissingPhotonProductionSource,
    IndependentReactionDiagnosticRequired,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyDiagnosticTriageReportDocument {
    pub report: NjoyDiagnosticTriageReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyDiagnosticTriageResult {
    pub report: NjoyDiagnosticTriageReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl NjoyDiagnosticTriageReport {
    pub fn assess(
        evidence: &NjoyEvidenceAwareSuitabilityReportDocument,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
    ) -> Result<Self, NjoyDiagnosticTriageError> {
        evidence.report.validate()?;
        domain.report.validate()?;

        let domain_reference = ContentReference {
            id: domain.report.id.clone(),
            sha256: domain.sha256.clone(),
        };
        if evidence.report.domain_aware_suitability_report != domain_reference {
            return Err(NjoyDiagnosticTriageError::DomainBindingMismatch);
        }
        if evidence.report.case_id != domain.report.case_id
            || evidence.report.execution_receipt != domain.report.execution_receipt
            || evidence.report.assessment_energy_range_ev
                != domain.report.assessment_energy_range_ev
            || evidence.report.runs.len() != domain.report.runs.len()
        {
            return Err(NjoyDiagnosticTriageError::EvidenceBindingMismatch);
        }

        let mut runs = Vec::with_capacity(evidence.report.runs.len());
        for (evidence_run, domain_run) in evidence.report.runs.iter().zip(&domain.report.runs) {
            let rejecting_nonkinematic_finding_count = domain_run.source_format_findings.len()
                as u64
                + domain_run
                    .processor_findings
                    .iter()
                    .filter(|finding| {
                        finding.disposition == NjoyProcessorFindingDisposition::Rejecting
                    })
                    .count() as u64;
            if evidence_run.nuclide != domain_run.nuclide
                || evidence_run.full_evaluation_diagnostic_violation_count
                    != domain_run.full_evaluation_diagnostic_violation_count
                || evidence_run.in_domain_diagnostic_violation_count
                    != domain_run.in_domain_diagnostic_violation_count
                || evidence_run.out_of_domain_diagnostic_violation_count
                    != domain_run.out_of_domain_diagnostic_violations.len() as u64
                || evidence_run.rejecting_nonkinematic_finding_count
                    != rejecting_nonkinematic_finding_count
                || evidence_run.domain_aware_suitability != domain_run.suitability
            {
                return Err(NjoyDiagnosticTriageError::RunBindingMismatch(
                    evidence_run.nuclide.clone(),
                ));
            }

            let missing_photon_production_finding_count = domain_run
                .processor_findings
                .iter()
                .filter(|finding| {
                    finding.disposition == NjoyProcessorFindingDisposition::Rejecting
                        && finding.finding.kind
                            == NjoySuitabilityFindingKind::NoPhotonProductionLocalFallback
                })
                .count() as u64;
            let exact_missing_source_blocker = domain_run.heatr_photon_source
                == HeatrPhotonSource::LocalDepositionFallback
                && domain_run.source_format_findings.is_empty()
                && missing_photon_production_finding_count == 1
                && rejecting_nonkinematic_finding_count == 1;
            let remaining = evidence_run.remaining_in_domain_diagnostic_violation_count;
            let disposition = if exact_missing_source_blocker {
                NjoyDiagnosticTriageDisposition::BlockedByMissingPhotonProductionSource
            } else if remaining > 0 {
                NjoyDiagnosticTriageDisposition::IndependentReactionDiagnosticRequired
            } else {
                NjoyDiagnosticTriageDisposition::NoRemainingKinematicFinding
            };
            let source_data_blocked = if exact_missing_source_blocker {
                remaining
            } else {
                0
            };
            let independent_diagnostic_required = if remaining > 0 && !exact_missing_source_blocker
            {
                remaining
            } else {
                0
            };

            runs.push(NjoyDiagnosticTriageRun {
                nuclide: evidence_run.nuclide.clone(),
                heatr_photon_source: domain_run.heatr_photon_source,
                response_suitability: evidence_run.suitability,
                remaining_in_domain_kinematic_violation_count: remaining,
                rejecting_nonkinematic_finding_count,
                missing_photon_production_finding_count,
                disposition,
                source_data_blocked_in_domain_kinematic_violation_count: source_data_blocked,
                independent_diagnostic_required_in_domain_kinematic_violation_count:
                    independent_diagnostic_required,
            });
        }

        let independent_count = sum_runs(&runs, |run| {
            run.independent_diagnostic_required_in_domain_kinematic_violation_count
        });
        let report = Self {
            schema_version: NJOY_DIAGNOSTIC_TRIAGE_SCHEMA.into(),
            id: format!("{}.{}", evidence.report.id, REPORT_ID_SUFFIX),
            case_id: evidence.report.case_id.clone(),
            qualification: qualification(independent_count),
            response_qualification: evidence.report.qualification,
            evidence_aware_suitability_report: ContentReference {
                id: evidence.report.id.clone(),
                sha256: evidence.sha256.clone(),
            },
            domain_aware_suitability_report: domain_reference,
            execution_receipt: evidence.report.execution_receipt.clone(),
            assessment_energy_range_ev: evidence.report.assessment_energy_range_ev,
            original_remaining_in_domain_kinematic_violation_count: sum_runs(&runs, |run| {
                run.remaining_in_domain_kinematic_violation_count
            }),
            source_data_blocked_in_domain_kinematic_violation_count: sum_runs(&runs, |run| {
                run.source_data_blocked_in_domain_kinematic_violation_count
            }),
            independent_diagnostic_required_in_domain_kinematic_violation_count:
                independent_count,
            source_data_blocked_run_count: count_runs(&runs, |run| {
                run.disposition
                    == NjoyDiagnosticTriageDisposition::BlockedByMissingPhotonProductionSource
            }),
            independent_diagnostic_required_run_count: count_runs(&runs, |run| {
                run.disposition
                    == NjoyDiagnosticTriageDisposition::IndependentReactionDiagnosticRequired
            }),
            runs,
            limitations: vec![
                "A source-data-blocked finding is preserved, not waived or numerically explained; the exact evaluation cannot support transported-photon KERMA for that run.".into(),
                "A clear independent-diagnostic queue would not make a rejected response candidate suitable or approve response-table generation.".into(),
            ],
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), NjoyDiagnosticTriageError> {
        if !openbnct_core::schema_matches(&self.schema_version, NJOY_DIAGNOSTIC_TRIAGE_SCHEMA) {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        for (label, reference) in [
            (
                "evidence_aware_suitability_report",
                &self.evidence_aware_suitability_report,
            ),
            (
                "domain_aware_suitability_report",
                &self.domain_aware_suitability_report,
            ),
            ("execution_receipt", &self.execution_receipt),
        ] {
            validate_identifier(label, &reference.id)?;
            validate_sha256(label, &reference.sha256)?;
        }
        if self.id
            != format!(
                "{}.{}",
                self.evidence_aware_suitability_report.id, REPORT_ID_SUFFIX
            )
        {
            return invalid_report("report ID does not bind the evidence-aware report");
        }
        let [lower, upper] = self.assessment_energy_range_ev;
        if !lower.is_finite() || !upper.is_finite() || lower < 0.0 || upper <= lower {
            return invalid_report("assessment energy range is invalid");
        }
        if self.runs.is_empty() {
            return invalid_report("diagnostic triage contains no runs");
        }
        if self.limitations.len() != 2 || self.limitations.iter().any(|item| item.trim().is_empty())
        {
            return invalid_report("diagnostic triage limitations are incomplete");
        }

        let mut previous_nuclide: Option<&str> = None;
        for run in &self.runs {
            validate_identifier("runs.nuclide", &run.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= run.nuclide.as_str()) {
                return invalid_report("runs are not strictly ordered by nuclide");
            }
            previous_nuclide = Some(&run.nuclide);
            validate_run(run)?;
        }

        let original = sum_runs(&self.runs, |run| {
            run.remaining_in_domain_kinematic_violation_count
        });
        let source_blocked = sum_runs(&self.runs, |run| {
            run.source_data_blocked_in_domain_kinematic_violation_count
        });
        let independent = sum_runs(&self.runs, |run| {
            run.independent_diagnostic_required_in_domain_kinematic_violation_count
        });
        let source_runs = count_runs(&self.runs, |run| {
            run.disposition
                == NjoyDiagnosticTriageDisposition::BlockedByMissingPhotonProductionSource
        });
        let independent_runs = count_runs(&self.runs, |run| {
            run.disposition
                == NjoyDiagnosticTriageDisposition::IndependentReactionDiagnosticRequired
        });
        if self.original_remaining_in_domain_kinematic_violation_count != original
            || self.source_data_blocked_in_domain_kinematic_violation_count != source_blocked
            || self.independent_diagnostic_required_in_domain_kinematic_violation_count
                != independent
            || self.source_data_blocked_run_count != source_runs
            || self.independent_diagnostic_required_run_count != independent_runs
            || original != source_blocked + independent
            || self.qualification != qualification(independent)
        {
            return invalid_report("aggregate counts do not match the triaged runs");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyDiagnosticTriageResult, NjoyDiagnosticTriageError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyDiagnosticTriageError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyDiagnosticTriageError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyDiagnosticTriageResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyDiagnosticTriageReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyDiagnosticTriageError> {
        let report: NjoyDiagnosticTriageReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyDiagnosticTriageError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_evidence(
        &self,
        evidence: &NjoyEvidenceAwareSuitabilityReportDocument,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
    ) -> Result<(), NjoyDiagnosticTriageError> {
        let observed = NjoyDiagnosticTriageReport::assess(evidence, domain)?;
        if self.report != observed {
            return Err(NjoyDiagnosticTriageError::AssessmentMismatch);
        }
        Ok(())
    }
}

fn validate_run(run: &NjoyDiagnosticTriageRun) -> Result<(), NjoyDiagnosticTriageError> {
    let remaining = run.remaining_in_domain_kinematic_violation_count;
    let source_blocked = run.source_data_blocked_in_domain_kinematic_violation_count;
    let independent = run.independent_diagnostic_required_in_domain_kinematic_violation_count;
    if remaining != source_blocked + independent {
        return invalid_report("run triage partition is inconsistent");
    }
    match run.disposition {
        NjoyDiagnosticTriageDisposition::NoRemainingKinematicFinding => {
            if remaining != 0 || source_blocked != 0 || independent != 0 {
                return invalid_report("empty diagnostic disposition has findings");
            }
        }
        NjoyDiagnosticTriageDisposition::BlockedByMissingPhotonProductionSource => {
            if run.heatr_photon_source != HeatrPhotonSource::LocalDepositionFallback
                || run.missing_photon_production_finding_count != 1
                || run.rejecting_nonkinematic_finding_count != 1
                || source_blocked != remaining
                || independent != 0
                || run.response_suitability != NjoySuitabilityStatus::Rejected
            {
                return invalid_report("invalid missing-photon-source blocker disposition");
            }
        }
        NjoyDiagnosticTriageDisposition::IndependentReactionDiagnosticRequired => {
            if remaining == 0
                || source_blocked != 0
                || independent != remaining
                || (run.heatr_photon_source == HeatrPhotonSource::LocalDepositionFallback
                    && run.missing_photon_production_finding_count == 1
                    && run.rejecting_nonkinematic_finding_count == 1)
                || run.response_suitability != NjoySuitabilityStatus::Rejected
            {
                return invalid_report("invalid independent-diagnostic disposition");
            }
        }
    }
    Ok(())
}

fn qualification(independent_count: u64) -> NjoyDiagnosticTriageQualification {
    if independent_count == 0 {
        NjoyDiagnosticTriageQualification::IndependentReactionDiagnosticQueueClearUnreviewed
    } else {
        NjoyDiagnosticTriageQualification::IndependentReactionDiagnosticsRequired
    }
}

fn sum_runs(
    runs: &[NjoyDiagnosticTriageRun],
    field: impl Fn(&NjoyDiagnosticTriageRun) -> u64,
) -> u64 {
    runs.iter().map(field).sum()
}

fn count_runs(
    runs: &[NjoyDiagnosticTriageRun],
    predicate: impl Fn(&NjoyDiagnosticTriageRun) -> bool,
) -> u64 {
    runs.iter().filter(|run| predicate(run)).count() as u64
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), NjoyDiagnosticTriageError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &'static str, digest: &str) -> Result<(), NjoyDiagnosticTriageError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_report(format!("{label} is not a lowercase SHA-256 digest"))
    }
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyDiagnosticTriageError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| NjoyDiagnosticTriageError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyDiagnosticTriageError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyDiagnosticTriageError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, NjoyDiagnosticTriageError> {
    Err(NjoyDiagnosticTriageError::InvalidReport(message.into()))
}

#[derive(Debug, Error)]
pub enum NjoyDiagnosticTriageError {
    #[error(transparent)]
    EvidenceAware(#[from] NjoyEvidenceAwareSuitabilityError),
    #[error(transparent)]
    DomainAware(#[from] NjoyDomainAwareSuitabilityError),
    #[error("triage evidence does not bind the supplied domain-aware report")]
    DomainBindingMismatch,
    #[error("triage inputs do not bind the same case, receipt, energy range, and run set")]
    EvidenceBindingMismatch,
    #[error("evidence layers disagree for nuclide {0}")]
    RunBindingMismatch(String),
    #[error("invalid NJOY diagnostic triage report: {0}")]
    InvalidReport(String),
    #[error("NJOY diagnostic triage does not match regenerated evidence")]
    AssessmentMismatch,
    #[error("required diagnostic triage artifact is not a regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-domain-aware-suitability.json"
    );
    const EVIDENCE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-evidence-aware-suitability.json"
    );
    const FROZEN_TRIAGE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-diagnostic-triage.json"
    );

    fn assessment() -> NjoyDiagnosticTriageReport {
        let domain = NjoyDomainAwareSuitabilityReportDocument::from_bytes(DOMAIN).unwrap();
        let evidence = NjoyEvidenceAwareSuitabilityReportDocument::from_bytes(EVIDENCE).unwrap();
        NjoyDiagnosticTriageReport::assess(&evidence, &domain).unwrap()
    }

    #[test]
    fn preserves_source_blocked_findings_and_focuses_the_independent_queue() {
        let report = assessment();
        assert_eq!(
            report.original_remaining_in_domain_kinematic_violation_count,
            102
        );
        assert_eq!(
            report.source_data_blocked_in_domain_kinematic_violation_count,
            59
        );
        assert_eq!(
            report.independent_diagnostic_required_in_domain_kinematic_violation_count,
            43
        );
        assert_eq!(report.source_data_blocked_run_count, 2);
        assert_eq!(report.independent_diagnostic_required_run_count, 1);
        assert_eq!(
            report.qualification,
            NjoyDiagnosticTriageQualification::IndependentReactionDiagnosticsRequired
        );

        let carbon = report.runs.iter().find(|run| run.nuclide == "C13").unwrap();
        assert_eq!(
            carbon.disposition,
            NjoyDiagnosticTriageDisposition::BlockedByMissingPhotonProductionSource
        );
        let oxygen_17 = report.runs.iter().find(|run| run.nuclide == "O17").unwrap();
        assert_eq!(
            oxygen_17.disposition,
            NjoyDiagnosticTriageDisposition::IndependentReactionDiagnosticRequired
        );
    }

    #[test]
    fn cannot_relabel_o17_as_a_missing_source_blocker() {
        let mut report = assessment();
        let oxygen = report
            .runs
            .iter_mut()
            .find(|run| run.nuclide == "O17")
            .unwrap();
        oxygen.disposition =
            NjoyDiagnosticTriageDisposition::BlockedByMissingPhotonProductionSource;
        oxygen.source_data_blocked_in_domain_kinematic_violation_count =
            oxygen.remaining_in_domain_kinematic_violation_count;
        oxygen.independent_diagnostic_required_in_domain_kinematic_violation_count = 0;
        assert!(matches!(
            report.validate(),
            Err(NjoyDiagnosticTriageError::InvalidReport(_))
        ));
    }

    #[test]
    fn verifies_the_frozen_triage_against_both_parent_layers() {
        let domain = NjoyDomainAwareSuitabilityReportDocument::from_bytes(DOMAIN).unwrap();
        let evidence = NjoyEvidenceAwareSuitabilityReportDocument::from_bytes(EVIDENCE).unwrap();
        let frozen = NjoyDiagnosticTriageReportDocument::from_bytes(FROZEN_TRIAGE).unwrap();
        assert_eq!(
            frozen.sha256,
            "6ba92bce735cf290dd3dbe3e068ceff1e25cbc1869b21d5ecd64db8b8d206020"
        );
        frozen.verify_against_evidence(&evidence, &domain).unwrap();
    }
}
