// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    NjoyExecutionArtifact, NjoyExecutionError, NjoyExecutionReceiptDocument,
    NjoyRunDiagnosticStatus,
};

pub const NJOY_SUITABILITY_REPORT_SCHEMA: &str =
    "openbnct.njoy-transported-photon-suitability/0.1.0";

const REPORT_ID_SUFFIX: &str = "transported-photon-kerma-suitability";
const NO_PHOTON_PRODUCTION_MESSAGE: &str =
    "no photon production files...all photon energy will be deposited locally.";
const NO_FILE_12_MESSAGE: &str = "---message from gheat---no file 12 for this material.";
const INCOMPLETE_DISCRETE_PHOTON_MESSAGE: &str = "discrete photon data may be incomplete";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySuitabilityReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub requirement: NjoyTransportRequirement,
    pub qualification: NjoySuitabilityQualification,
    pub execution_receipt: ContentReference,
    pub runs: Vec<NjoySuitabilityRun>,
    pub rejected_run_count: u64,
    pub kinematic_violation_count: u64,
    pub processor_finding_count: u64,
    pub processor_finding_occurrence_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyTransportRequirement {
    TransportedPhotonKermaWithCoupledPhotonTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoySuitabilityQualification {
    TransportedPhotonKermaCandidateUnreviewed,
    TransportedPhotonKermaRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySuitabilityRun {
    pub nuclide: String,
    pub processor_report: NjoyExecutionArtifact,
    pub diagnostic_status: NjoyRunDiagnosticStatus,
    pub diagnostic_violation_count: u64,
    pub processor_data_findings: Vec<NjoyProcessorDataFinding>,
    pub suitability: NjoySuitabilityStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoySuitabilityStatus {
    CandidateUnreviewed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyProcessorDataFinding {
    pub kind: NjoySuitabilityFindingKind,
    pub file_number: Option<u16>,
    pub reaction_mt: Option<u16>,
    pub occurrence_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoySuitabilityFindingKind {
    NoPhotonProductionLocalFallback,
    MissingPhotonMultiplicityFile,
    IncompleteDiscretePhotonData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NjoySuitabilityReportDocument {
    pub report: NjoySuitabilityReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NjoySuitabilityResult {
    pub report: NjoySuitabilityReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl NjoySuitabilityReport {
    /// Assess the exact processor reports in a previously verified execution
    /// root. The result is deterministic for a fixed receipt and root.
    pub fn assess(
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
    ) -> Result<Self, NjoySuitabilityError> {
        execution.verify_execution_root(execution_root)?;

        let mut runs = Vec::with_capacity(execution.receipt.runs.len());
        for executed in &execution.receipt.runs {
            let report_path = execution_root.join(&executed.processor_report.path);
            let report_bytes = read_regular_file(&report_path)?;
            if report_bytes.len() as u64 != executed.processor_report.size_bytes
                || sha256_bytes(&report_bytes) != executed.processor_report.sha256
            {
                return Err(NjoySuitabilityError::ProcessorReportChanged(
                    executed.processor_report.path.clone(),
                ));
            }
            let report_text = std::str::from_utf8(&report_bytes)
                .map_err(|_| NjoySuitabilityError::NonUtf8ProcessorReport(report_path))?;
            let processor_data_findings = processor_data_findings(report_text)?;
            let suitability = if executed.diagnostic_status
                == NjoyRunDiagnosticStatus::KinematicLimitsExceeded
                || !processor_data_findings.is_empty()
            {
                NjoySuitabilityStatus::Rejected
            } else {
                NjoySuitabilityStatus::CandidateUnreviewed
            };
            runs.push(NjoySuitabilityRun {
                nuclide: executed.nuclide.clone(),
                processor_report: executed.processor_report.clone(),
                diagnostic_status: executed.diagnostic_status,
                diagnostic_violation_count: executed.diagnostic_violation_count,
                processor_data_findings,
                suitability,
            });
        }

        let rejected_run_count = runs
            .iter()
            .filter(|run| run.suitability == NjoySuitabilityStatus::Rejected)
            .count() as u64;
        let kinematic_violation_count = runs.iter().map(|run| run.diagnostic_violation_count).sum();
        let processor_finding_count = runs
            .iter()
            .map(|run| run.processor_data_findings.len() as u64)
            .sum();
        let processor_finding_occurrence_count = runs
            .iter()
            .flat_map(|run| &run.processor_data_findings)
            .map(|finding| finding.occurrence_count)
            .sum();
        let qualification = if rejected_run_count == 0 {
            NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
        } else {
            NjoySuitabilityQualification::TransportedPhotonKermaRejected
        };
        let report = Self {
            schema_version: NJOY_SUITABILITY_REPORT_SCHEMA.into(),
            id: format!("{}.{}", execution.receipt.id, REPORT_ID_SUFFIX),
            case_id: execution.receipt.case_id.clone(),
            requirement: NjoyTransportRequirement::TransportedPhotonKermaWithCoupledPhotonTransport,
            qualification,
            execution_receipt: ContentReference {
                id: execution.receipt.id.clone(),
                sha256: execution.sha256.clone(),
            },
            runs,
            rejected_run_count,
            kinematic_violation_count,
            processor_finding_count,
            processor_finding_occurrence_count,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), NjoySuitabilityError> {
        if !openbnct_core::schema_matches(&self.schema_version, NJOY_SUITABILITY_REPORT_SCHEMA) {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier("execution_receipt.id", &self.execution_receipt.id)?;
        validate_sha256("execution_receipt.sha256", &self.execution_receipt.sha256)?;
        if self.id
            != format!(
                "{}.{}",
                self.execution_receipt.id.as_str(),
                REPORT_ID_SUFFIX
            )
        {
            return invalid_report("report ID does not bind the execution receipt");
        }
        if self.runs.is_empty() {
            return invalid_report("suitability report contains no nuclide runs");
        }

        let mut previous_nuclide: Option<&str> = None;
        let mut rejected_run_count = 0_u64;
        let mut kinematic_violation_count = 0_u64;
        let mut processor_finding_count = 0_u64;
        let mut processor_finding_occurrence_count = 0_u64;
        for run in &self.runs {
            validate_nuclide_name(&run.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= run.nuclide.as_str()) {
                return invalid_report("suitability runs are not strictly ordered");
            }
            previous_nuclide = Some(&run.nuclide);
            validate_run(run)?;
            if run.suitability == NjoySuitabilityStatus::Rejected {
                rejected_run_count += 1;
            }
            kinematic_violation_count += run.diagnostic_violation_count;
            processor_finding_count += run.processor_data_findings.len() as u64;
            processor_finding_occurrence_count += run
                .processor_data_findings
                .iter()
                .map(|finding| finding.occurrence_count)
                .sum::<u64>();
        }
        if rejected_run_count != self.rejected_run_count
            || kinematic_violation_count != self.kinematic_violation_count
            || processor_finding_count != self.processor_finding_count
            || processor_finding_occurrence_count != self.processor_finding_occurrence_count
        {
            return invalid_report("suitability aggregate counts do not match the run records");
        }
        let expected_qualification = if rejected_run_count == 0 {
            NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
        } else {
            NjoySuitabilityQualification::TransportedPhotonKermaRejected
        };
        if self.qualification != expected_qualification {
            return invalid_report("suitability qualification does not match the run records");
        }
        Ok(())
    }

    pub fn write_new(&self, path: &Path) -> Result<NjoySuitabilityResult, NjoySuitabilityError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoySuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoySuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoySuitabilityResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoySuitabilityReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoySuitabilityError> {
        let report: NjoySuitabilityReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoySuitabilityError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_execution(
        &self,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
    ) -> Result<(), NjoySuitabilityError> {
        self.report.validate()?;
        let observed = NjoySuitabilityReport::assess(execution, execution_root)?;
        if observed != self.report {
            return Err(NjoySuitabilityError::AssessmentMismatch);
        }
        Ok(())
    }
}

fn validate_run(run: &NjoySuitabilityRun) -> Result<(), NjoySuitabilityError> {
    let expected_report_path = format!("{}/output", run.nuclide);
    if run.processor_report.path != expected_report_path
        || run.processor_report.media_type != "text/plain"
        || run.processor_report.size_bytes == 0
    {
        return invalid_report(format!(
            "run {} has an invalid processor-report reference",
            run.nuclide
        ));
    }
    validate_normalized_relative_path(&run.processor_report.path)?;
    validate_sha256("run.processor_report.sha256", &run.processor_report.sha256)?;

    let expected_diagnostic_failure = run.diagnostic_violation_count > 0;
    if (run.diagnostic_status == NjoyRunDiagnosticStatus::KinematicLimitsExceeded)
        != expected_diagnostic_failure
    {
        return invalid_report(format!(
            "run {} diagnostic status and count disagree",
            run.nuclide
        ));
    }

    let mut previous_finding = None;
    for finding in &run.processor_data_findings {
        validate_finding(finding)?;
        let current = finding_order(finding);
        if previous_finding.is_some_and(|previous| previous >= current) {
            return invalid_report(format!(
                "run {} processor findings are not strictly ordered",
                run.nuclide
            ));
        }
        previous_finding = Some(current);
    }
    let expected_status = if expected_diagnostic_failure || !run.processor_data_findings.is_empty()
    {
        NjoySuitabilityStatus::Rejected
    } else {
        NjoySuitabilityStatus::CandidateUnreviewed
    };
    if run.suitability != expected_status {
        return invalid_report(format!(
            "run {} suitability does not match its evidence",
            run.nuclide
        ));
    }
    Ok(())
}

fn validate_finding(finding: &NjoyProcessorDataFinding) -> Result<(), NjoySuitabilityError> {
    if finding.occurrence_count == 0 {
        return invalid_report("processor finding occurrence count must be positive");
    }
    let valid_shape = match finding.kind {
        NjoySuitabilityFindingKind::NoPhotonProductionLocalFallback => {
            finding.file_number.is_none() && finding.reaction_mt.is_none()
        }
        NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile => {
            finding.file_number == Some(12) && finding.reaction_mt.is_none()
        }
        NjoySuitabilityFindingKind::IncompleteDiscretePhotonData => {
            finding.file_number == Some(12) && finding.reaction_mt.is_some_and(|mt| mt > 0)
        }
    };
    if valid_shape {
        Ok(())
    } else {
        invalid_report("processor finding fields do not match its kind")
    }
}

fn processor_data_findings(
    report: &str,
) -> Result<Vec<NjoyProcessorDataFinding>, NjoySuitabilityError> {
    let no_photon_count = report.matches(NO_PHOTON_PRODUCTION_MESSAGE).count() as u64;
    let no_file_12_count = report.matches(NO_FILE_12_MESSAGE).count() as u64;
    let lines = report.lines().collect::<Vec<_>>();
    let mut incomplete = BTreeMap::<(u16, u16), u64>::new();
    for (index, line) in lines.iter().enumerate() {
        if !line.contains(INCOMPLETE_DISCRETE_PHOTON_MESSAGE) {
            continue;
        }
        let preceding = lines[..index]
            .iter()
            .rev()
            .find(|candidate| !candidate.trim().is_empty())
            .ok_or(NjoySuitabilityError::UnparsedProcessorDataFinding)?;
        let (file_number, reaction_mt) = parse_missing_discrete_section(preceding)
            .ok_or(NjoySuitabilityError::UnparsedProcessorDataFinding)?;
        *incomplete.entry((file_number, reaction_mt)).or_default() += 1;
    }

    let mut findings = Vec::new();
    if no_photon_count > 0 {
        findings.push(NjoyProcessorDataFinding {
            kind: NjoySuitabilityFindingKind::NoPhotonProductionLocalFallback,
            file_number: None,
            reaction_mt: None,
            occurrence_count: no_photon_count,
        });
    }
    if no_file_12_count > 0 {
        findings.push(NjoyProcessorDataFinding {
            kind: NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile,
            file_number: Some(12),
            reaction_mt: None,
            occurrence_count: no_file_12_count,
        });
    }
    for ((file_number, reaction_mt), occurrence_count) in incomplete {
        findings.push(NjoyProcessorDataFinding {
            kind: NjoySuitabilityFindingKind::IncompleteDiscretePhotonData,
            file_number: Some(file_number),
            reaction_mt: Some(reaction_mt),
            occurrence_count,
        });
    }
    findings.sort_by_key(finding_order);
    Ok(findings)
}

fn parse_missing_discrete_section(line: &str) -> Option<(u16, u16)> {
    let marker = "---message from hconvr---mf";
    let tail = line.trim().strip_prefix(marker)?;
    let (file, tail) = tail.split_once(", mt")?;
    let reaction = tail.strip_suffix(" may be missing")?;
    Some((file.parse().ok()?, reaction.parse().ok()?))
}

fn finding_order(finding: &NjoyProcessorDataFinding) -> (u8, u16, u16) {
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

fn validate_identifier(label: &'static str, value: &str) -> Result<(), NjoySuitabilityError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} is empty"))
    } else {
        Ok(())
    }
}

fn validate_nuclide_name(value: &str) -> Result<(), NjoySuitabilityError> {
    let path = Path::new(value);
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        invalid_report("run.nuclide is not a canonical path-safe name")
    } else {
        Ok(())
    }
}

fn validate_normalized_relative_path(value: &str) -> Result<(), NjoySuitabilityError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        invalid_report(format!(
            "processor report path is not normalized and relative: {value:?}"
        ))
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), NjoySuitabilityError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_report(format!("{label} is not a canonical lowercase SHA-256"))
    }
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoySuitabilityError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| NjoySuitabilityError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NjoySuitabilityError::NotRegularFile(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| NjoySuitabilityError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, NjoySuitabilityError> {
    Err(NjoySuitabilityError::InvalidReport(message.into()))
}

#[derive(Debug, Error)]
pub enum NjoySuitabilityError {
    #[error(transparent)]
    Execution(#[from] NjoyExecutionError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid NJOY transported-photon suitability report: {0}")]
    InvalidReport(String),
    #[error("NJOY processor report is not UTF-8 text: {0}")]
    NonUtf8ProcessorReport(PathBuf),
    #[error("NJOY processor report changed after execution verification: {0}")]
    ProcessorReportChanged(String),
    #[error("NJOY processor data-suitability message could not be parsed")]
    UnparsedProcessorDataFinding,
    #[error("NJOY suitability report does not match the verified execution evidence")]
    AssessmentMismatch,
    #[error("required suitability artifact is not a real regular file: {0}")]
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

    const FROZEN_SUITABILITY_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-transported-photon-suitability.json"
    );

    #[test]
    fn structures_the_frozen_heatr_data_warnings() {
        let report = "no photon production files...all photon energy will be deposited locally.\n\
                      ---message from gheat---no file 12 for this material.\n\
                      ---message from hconvr---mf12, mt51 may be missing\n\
                                                discrete photon data may be incomplete\n";
        assert_eq!(
            processor_data_findings(report).unwrap(),
            [
                NjoyProcessorDataFinding {
                    kind: NjoySuitabilityFindingKind::NoPhotonProductionLocalFallback,
                    file_number: None,
                    reaction_mt: None,
                    occurrence_count: 1,
                },
                NjoyProcessorDataFinding {
                    kind: NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile,
                    file_number: Some(12),
                    reaction_mt: None,
                    occurrence_count: 1,
                },
                NjoyProcessorDataFinding {
                    kind: NjoySuitabilityFindingKind::IncompleteDiscretePhotonData,
                    file_number: Some(12),
                    reaction_mt: Some(51),
                    occurrence_count: 1,
                },
            ]
        );
    }

    #[test]
    fn rejects_an_unmapped_incomplete_photon_warning() {
        assert!(matches!(
            processor_data_findings("discrete photon data may be incomplete"),
            Err(NjoySuitabilityError::UnparsedProcessorDataFinding)
        ));
    }

    #[test]
    fn validates_the_frozen_transported_photon_rejection() {
        let document =
            NjoySuitabilityReportDocument::from_bytes(FROZEN_SUITABILITY_REPORT).unwrap();
        assert_eq!(
            document.sha256,
            "39f32c071e715d4b712a92a25faf1424ba99f548aeabe88c934e84b5d2e48e22"
        );
        assert_eq!(document.report.runs.len(), 10);
        assert_eq!(document.report.rejected_run_count, 4);
        assert_eq!(document.report.kinematic_violation_count, 72);
        assert_eq!(document.report.processor_finding_count, 4);
        assert_eq!(document.report.processor_finding_occurrence_count, 8);
        assert_eq!(
            document.report.qualification,
            NjoySuitabilityQualification::TransportedPhotonKermaRejected
        );
    }
}
