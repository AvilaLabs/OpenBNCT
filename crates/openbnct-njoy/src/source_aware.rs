// SPDX-License-Identifier: MIT

//! Source-aware interpretation of NJOY transported-photon diagnostics.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    EndfPhotonFormatFinding, EndfPhotonInventoryError, EndfPhotonProductionInventoryDocument,
    HeatrPhotonSource, NJOY_INPUT_MANIFEST_MIXED_SCHEMA, NJOY_INPUT_MANIFEST_SCHEMA,
    NjoyExecutionArtifact, NjoyExecutionReceiptDocument, NjoyInputManifest,
    NjoyProcessorDataFinding, NjoyRunDiagnosticStatus, NjoySuitabilityError,
    NjoySuitabilityFindingKind, NjoySuitabilityQualification, NjoySuitabilityReportDocument,
    NjoySuitabilityStatus, NjoyTransportRequirement,
};

pub const NJOY_SOURCE_AWARE_SUITABILITY_SCHEMA: &str =
    "openbnct.njoy-transported-photon-suitability/0.2.0";

const REPORT_ID_SUFFIX: &str = "source-aware-v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySourceAwareSuitabilityReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub requirement: NjoyTransportRequirement,
    pub qualification: NjoySuitabilityQualification,
    pub legacy_suitability_report: ContentReference,
    pub photon_production_inventory: ContentReference,
    pub input_manifest: ContentReference,
    pub runs: Vec<NjoySourceAwareSuitabilityRun>,
    pub rejected_run_count: u64,
    pub kinematic_violation_count: u64,
    pub processor_finding_count: u64,
    pub rejecting_processor_finding_count: u64,
    pub informational_processor_finding_count: u64,
    pub source_format_finding_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySourceAwareSuitabilityRun {
    pub nuclide: String,
    pub processor_report: NjoyExecutionArtifact,
    pub diagnostic_status: NjoyRunDiagnosticStatus,
    pub diagnostic_violation_count: u64,
    pub heatr_photon_source: HeatrPhotonSource,
    pub file13_without_file12_reaction_count: u64,
    pub source_format_findings: Vec<EndfPhotonFormatFinding>,
    pub processor_findings: Vec<NjoySourceAwareProcessorFinding>,
    pub suitability: NjoySuitabilityStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySourceAwareProcessorFinding {
    pub finding: NjoyProcessorDataFinding,
    pub disposition: NjoyProcessorFindingDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyProcessorFindingDisposition {
    Rejecting,
    InformationalFile13Alternative,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NjoySourceAwareSuitabilityReportDocument {
    pub report: NjoySourceAwareSuitabilityReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NjoySourceAwareSuitabilityResult {
    pub report: NjoySourceAwareSuitabilityReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl NjoySourceAwareSuitabilityReport {
    /// Reinterpret a verified v0.1 processor report using the exact source
    /// evaluation inventory bound by the executed input manifest.
    #[allow(clippy::too_many_arguments)]
    pub fn assess(
        legacy: &NjoySuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        input_manifest_bytes: &[u8],
        inventory: &EndfPhotonProductionInventoryDocument,
    ) -> Result<Self, NjoySourceAwareSuitabilityError> {
        legacy.verify_against_execution(execution, execution_root)?;
        inventory.inventory.validate()?;

        let execution_reference = ContentReference {
            id: execution.receipt.id.clone(),
            sha256: execution.sha256.clone(),
        };
        if legacy.report.execution_receipt != execution_reference {
            return Err(NjoySourceAwareSuitabilityError::ExecutionBindingMismatch);
        }

        let input_manifest: NjoyInputManifest = serde_json::from_slice(input_manifest_bytes)?;
        if !matches!(
            input_manifest.schema_version.as_str(),
            NJOY_INPUT_MANIFEST_SCHEMA | NJOY_INPUT_MANIFEST_MIXED_SCHEMA
        ) {
            return Err(NjoySourceAwareSuitabilityError::InputManifestBindingMismatch);
        }
        let input_manifest_reference = ContentReference {
            id: input_manifest.id.clone(),
            sha256: sha256_bytes(input_manifest_bytes),
        };
        if execution.receipt.input_manifest != input_manifest_reference
            || input_manifest.bindings.evaluated_source_selection
                != inventory.inventory.evaluated_source_selection
        {
            return Err(NjoySourceAwareSuitabilityError::InputManifestBindingMismatch);
        }
        if legacy.report.case_id != inventory.inventory.case_id
            || legacy.report.case_id != input_manifest.case_id
            || legacy.report.case_id != execution.receipt.case_id
        {
            return Err(NjoySourceAwareSuitabilityError::CaseBindingMismatch);
        }
        if legacy.report.runs.len() != inventory.inventory.evaluations.len() {
            return Err(NjoySourceAwareSuitabilityError::NuclideSetMismatch);
        }

        let mut runs = Vec::with_capacity(legacy.report.runs.len());
        for (legacy_run, source) in legacy
            .report
            .runs
            .iter()
            .zip(&inventory.inventory.evaluations)
        {
            if legacy_run.nuclide != source.nuclide {
                return Err(NjoySourceAwareSuitabilityError::NuclideSetMismatch);
            }
            let local_fallback_finding = legacy_run.processor_data_findings.iter().any(|finding| {
                finding.kind == NjoySuitabilityFindingKind::NoPhotonProductionLocalFallback
            });
            if local_fallback_finding
                != (source.heatr_photon_source == HeatrPhotonSource::LocalDepositionFallback)
            {
                return Err(NjoySourceAwareSuitabilityError::SourceProcessorConflict(
                    source.nuclide.clone(),
                ));
            }

            let processor_findings = legacy_run
                .processor_data_findings
                .iter()
                .map(|finding| {
                    let disposition = if finding.kind
                        == NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile
                        && source.file13_without_file12_reaction_count > 0
                        && source.heatr_photon_source.transports_secondary_photons()
                        && source.format_findings.is_empty()
                    {
                        NjoyProcessorFindingDisposition::InformationalFile13Alternative
                    } else {
                        NjoyProcessorFindingDisposition::Rejecting
                    };
                    NjoySourceAwareProcessorFinding {
                        finding: finding.clone(),
                        disposition,
                    }
                })
                .collect::<Vec<_>>();
            let suitability = if legacy_run.diagnostic_violation_count > 0
                || !source.format_findings.is_empty()
                || processor_findings.iter().any(|finding| {
                    finding.disposition == NjoyProcessorFindingDisposition::Rejecting
                }) {
                NjoySuitabilityStatus::Rejected
            } else {
                NjoySuitabilityStatus::CandidateUnreviewed
            };
            runs.push(NjoySourceAwareSuitabilityRun {
                nuclide: legacy_run.nuclide.clone(),
                processor_report: legacy_run.processor_report.clone(),
                diagnostic_status: legacy_run.diagnostic_status,
                diagnostic_violation_count: legacy_run.diagnostic_violation_count,
                heatr_photon_source: source.heatr_photon_source,
                file13_without_file12_reaction_count: source.file13_without_file12_reaction_count,
                source_format_findings: source.format_findings.clone(),
                processor_findings,
                suitability,
            });
        }

        let rejected_run_count = runs
            .iter()
            .filter(|run| run.suitability == NjoySuitabilityStatus::Rejected)
            .count() as u64;
        let report = Self {
            schema_version: NJOY_SOURCE_AWARE_SUITABILITY_SCHEMA.into(),
            id: format!("{}.{}", legacy.report.id, REPORT_ID_SUFFIX),
            case_id: legacy.report.case_id.clone(),
            requirement: legacy.report.requirement,
            qualification: if rejected_run_count == 0 {
                NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
            } else {
                NjoySuitabilityQualification::TransportedPhotonKermaRejected
            },
            legacy_suitability_report: ContentReference {
                id: legacy.report.id.clone(),
                sha256: legacy.sha256.clone(),
            },
            photon_production_inventory: ContentReference {
                id: inventory.inventory.id.clone(),
                sha256: inventory.sha256.clone(),
            },
            input_manifest: input_manifest_reference,
            rejected_run_count,
            kinematic_violation_count: runs.iter().map(|run| run.diagnostic_violation_count).sum(),
            processor_finding_count: runs
                .iter()
                .map(|run| run.processor_findings.len() as u64)
                .sum(),
            rejecting_processor_finding_count: count_disposition(
                &runs,
                NjoyProcessorFindingDisposition::Rejecting,
            ),
            informational_processor_finding_count: count_disposition(
                &runs,
                NjoyProcessorFindingDisposition::InformationalFile13Alternative,
            ),
            source_format_finding_count: runs
                .iter()
                .map(|run| run.source_format_findings.len() as u64)
                .sum(),
            runs,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), NjoySourceAwareSuitabilityError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_SOURCE_AWARE_SUITABILITY_SCHEMA,
        ) {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        for (label, reference) in [
            ("legacy_suitability_report", &self.legacy_suitability_report),
            (
                "photon_production_inventory",
                &self.photon_production_inventory,
            ),
            ("input_manifest", &self.input_manifest),
        ] {
            validate_identifier(label, &reference.id)?;
            validate_sha256(label, &reference.sha256)?;
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        if self.id != format!("{}.{}", self.legacy_suitability_report.id, REPORT_ID_SUFFIX) {
            return invalid_report("report ID does not bind the legacy report");
        }
        if self.runs.is_empty() {
            return invalid_report("source-aware report contains no runs");
        }

        let mut previous_nuclide: Option<&str> = None;
        for run in &self.runs {
            validate_identifier("runs.nuclide", &run.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= run.nuclide.as_str()) {
                return invalid_report("runs are not strictly ordered by nuclide");
            }
            previous_nuclide = Some(&run.nuclide);
            let expected_diagnostic_failure = run.diagnostic_violation_count > 0;
            if (run.diagnostic_status == NjoyRunDiagnosticStatus::KinematicLimitsExceeded)
                != expected_diagnostic_failure
            {
                return invalid_report("diagnostic status and count disagree");
            }
            if run.processor_report.size_bytes == 0 {
                return invalid_report("processor report reference is empty");
            }
            validate_sha256("runs.processor_report", &run.processor_report.sha256)?;

            let mut previous_finding = None;
            for interpreted in &run.processor_findings {
                if interpreted.finding.occurrence_count == 0 {
                    return invalid_report("processor finding has no occurrences");
                }
                let key = processor_finding_order(&interpreted.finding);
                if previous_finding.is_some_and(|previous| previous >= key) {
                    return invalid_report("processor findings are not strictly ordered");
                }
                previous_finding = Some(key);
                let informational_valid = interpreted.disposition
                    == NjoyProcessorFindingDisposition::InformationalFile13Alternative
                    && interpreted.finding.kind
                        == NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile
                    && run.file13_without_file12_reaction_count > 0
                    && run.heatr_photon_source.transports_secondary_photons()
                    && run.source_format_findings.is_empty();
                if interpreted.disposition
                    == NjoyProcessorFindingDisposition::InformationalFile13Alternative
                    && !informational_valid
                {
                    return invalid_report("invalid informational File 13 disposition");
                }
            }
            let expected_status = if expected_diagnostic_failure
                || !run.source_format_findings.is_empty()
                || run.processor_findings.iter().any(|finding| {
                    finding.disposition == NjoyProcessorFindingDisposition::Rejecting
                }) {
                NjoySuitabilityStatus::Rejected
            } else {
                NjoySuitabilityStatus::CandidateUnreviewed
            };
            if run.suitability != expected_status {
                return invalid_report("run suitability does not match its evidence");
            }
        }

        let rejected_run_count = self
            .runs
            .iter()
            .filter(|run| run.suitability == NjoySuitabilityStatus::Rejected)
            .count() as u64;
        let kinematic_violation_count: u64 = self
            .runs
            .iter()
            .map(|run| run.diagnostic_violation_count)
            .sum();
        let processor_finding_count: u64 = self
            .runs
            .iter()
            .map(|run| run.processor_findings.len() as u64)
            .sum();
        let rejecting_processor_finding_count =
            count_disposition(&self.runs, NjoyProcessorFindingDisposition::Rejecting);
        let informational_processor_finding_count = count_disposition(
            &self.runs,
            NjoyProcessorFindingDisposition::InformationalFile13Alternative,
        );
        let source_format_finding_count: u64 = self
            .runs
            .iter()
            .map(|run| run.source_format_findings.len() as u64)
            .sum();
        if self.rejected_run_count != rejected_run_count
            || self.kinematic_violation_count != kinematic_violation_count
            || self.processor_finding_count != processor_finding_count
            || self.rejecting_processor_finding_count != rejecting_processor_finding_count
            || self.informational_processor_finding_count != informational_processor_finding_count
            || self.source_format_finding_count != source_format_finding_count
        {
            return invalid_report("aggregate counts do not match the runs");
        }
        let expected_qualification = if rejected_run_count == 0 {
            NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
        } else {
            NjoySuitabilityQualification::TransportedPhotonKermaRejected
        };
        if self.qualification != expected_qualification {
            return invalid_report("qualification does not match the runs");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoySourceAwareSuitabilityResult, NjoySourceAwareSuitabilityError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoySourceAwareSuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoySourceAwareSuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoySourceAwareSuitabilityResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoySourceAwareSuitabilityReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoySourceAwareSuitabilityError> {
        let report: NjoySourceAwareSuitabilityReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoySourceAwareSuitabilityError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn verify_against_evidence(
        &self,
        legacy: &NjoySuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        input_manifest_bytes: &[u8],
        inventory: &EndfPhotonProductionInventoryDocument,
    ) -> Result<(), NjoySourceAwareSuitabilityError> {
        let observed = NjoySourceAwareSuitabilityReport::assess(
            legacy,
            execution,
            execution_root,
            input_manifest_bytes,
            inventory,
        )?;
        if self.report != observed {
            return Err(NjoySourceAwareSuitabilityError::AssessmentMismatch);
        }
        Ok(())
    }
}

fn count_disposition(
    runs: &[NjoySourceAwareSuitabilityRun],
    disposition: NjoyProcessorFindingDisposition,
) -> u64 {
    runs.iter()
        .flat_map(|run| &run.processor_findings)
        .filter(|finding| finding.disposition == disposition)
        .count() as u64
}

fn processor_finding_order(finding: &NjoyProcessorDataFinding) -> (u8, u16, u16) {
    let kind = match finding.kind {
        NjoySuitabilityFindingKind::NoPhotonProductionLocalFallback => 0,
        NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile => 1,
        NjoySuitabilityFindingKind::IncompleteDiscretePhotonData => 2,
    };
    (
        kind,
        finding.file_number.unwrap_or(0),
        finding.reaction_mt.unwrap_or(0),
    )
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoySourceAwareSuitabilityError> {
    if value.trim().is_empty() {
        return invalid_report(format!("{label} must not be empty"));
    }
    Ok(())
}

fn validate_sha256(
    label: &'static str,
    value: &str,
) -> Result<(), NjoySourceAwareSuitabilityError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_report(format!("{label} is not a lowercase SHA-256 digest"));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoySourceAwareSuitabilityError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| NjoySourceAwareSuitabilityError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(NjoySourceAwareSuitabilityError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoySourceAwareSuitabilityError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, NjoySourceAwareSuitabilityError> {
    Err(NjoySourceAwareSuitabilityError::InvalidReport(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoySourceAwareSuitabilityError {
    #[error(transparent)]
    LegacySuitability(#[from] NjoySuitabilityError),
    #[error(transparent)]
    PhotonInventory(#[from] EndfPhotonInventoryError),
    #[error("legacy suitability report does not bind the supplied execution receipt")]
    ExecutionBindingMismatch,
    #[error("executed input manifest does not bind the supplied photon-production inventory")]
    InputManifestBindingMismatch,
    #[error("case identities do not match across source-aware evidence")]
    CaseBindingMismatch,
    #[error("nuclide sets do not match across source-aware evidence")]
    NuclideSetMismatch,
    #[error("source photon inventory conflicts with NJOY processor diagnostics for {0}")]
    SourceProcessorConflict(String),
    #[error("invalid source-aware transported-photon suitability report: {0}")]
    InvalidReport(String),
    #[error("source-aware suitability report does not match regenerated evidence")]
    AssessmentMismatch,
    #[error("required source-aware suitability artifact is not a regular file: {0}")]
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

    const BASELINE_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-transported-photon-source-aware-suitability.json"
    );
    const JEFF40_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-source-aware-suitability.json"
    );

    #[test]
    fn schema_is_distinct_from_the_legacy_log_only_assessment() {
        assert_eq!(
            NJOY_SOURCE_AWARE_SUITABILITY_SCHEMA,
            "openbnct.njoy-transported-photon-suitability/0.2.0"
        );
        assert_ne!(
            NJOY_SOURCE_AWARE_SUITABILITY_SCHEMA,
            crate::NJOY_SUITABILITY_REPORT_SCHEMA
        );
        assert_eq!(
            crate::ENDF_PHOTON_PRODUCTION_INVENTORY_SCHEMA,
            "openbnct.endf-photon-production-inventory/0.1.0"
        );
    }

    #[test]
    fn validates_frozen_source_aware_assessments() {
        let baseline =
            NjoySourceAwareSuitabilityReportDocument::from_bytes(BASELINE_REPORT).unwrap();
        let jeff = NjoySourceAwareSuitabilityReportDocument::from_bytes(JEFF40_REPORT).unwrap();
        assert_eq!(
            baseline.sha256,
            "6bd6cdef99fd940e386ffce46964f98e5e5f77a82c40517478a4aaa234d1d680"
        );
        assert_eq!(
            jeff.sha256,
            "3bc909a8285f8654fd62d776c427d7b7ef0825f5608b19744740bcbbc8babe92"
        );
        assert_eq!(baseline.report.rejected_run_count, 4);
        assert_eq!(jeff.report.rejected_run_count, 5);
        let jeff_n15 = jeff
            .report
            .runs
            .iter()
            .find(|run| run.nuclide == "N15")
            .unwrap();
        assert_eq!(
            jeff_n15.suitability,
            NjoySuitabilityStatus::CandidateUnreviewed
        );
        assert_eq!(
            jeff_n15.processor_findings[0].disposition,
            NjoyProcessorFindingDisposition::InformationalFile13Alternative
        );
    }
}
