// SPDX-License-Identifier: MIT

//! Transport-domain-aware interpretation of source-aware NJOY diagnostics.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{
    OpenMcDiagnosticBoundaryPolicy, OpenMcNeutronTransportDomainDocument,
    OpenMcTransportDomainError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    HeatrPhotonSource, NJOY_INPUT_MANIFEST_MIXED_SCHEMA, NJOY_INPUT_MANIFEST_SCHEMA,
    NjoyExecutionArtifact, NjoyExecutionReceiptDocument, NjoyInputManifest, NjoyKinematicViolation,
    NjoyProcessorFindingDisposition, NjoyRunDiagnosticStatus, NjoySourceAwareProcessorFinding,
    NjoySourceAwareSuitabilityReportDocument, NjoySuitabilityQualification,
    NjoySuitabilityReportDocument, NjoySuitabilityStatus, NjoyTransportRequirement,
};

pub const NJOY_DOMAIN_AWARE_SUITABILITY_SCHEMA: &str =
    "openbnct.njoy-transported-photon-suitability/0.3.0";

const REPORT_ID_SUFFIX: &str = "domain-aware-v3";

/// A new assessment layer that scopes only NJOY's kinematic diagnostics to a
/// separately derived transport interval. Source-format and processor-data
/// findings remain rejecting at every energy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyDomainAwareSuitabilityReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub requirement: NjoyTransportRequirement,
    pub qualification: NjoySuitabilityQualification,
    pub source_aware_suitability_report: ContentReference,
    pub legacy_suitability_report: ContentReference,
    pub execution_receipt: ContentReference,
    pub input_manifest: ContentReference,
    pub neutron_transport_domain: ContentReference,
    pub assessment_energy_range_ev: [f64; 2],
    pub diagnostic_boundary_policy: OpenMcDiagnosticBoundaryPolicy,
    pub runs: Vec<NjoyDomainAwareSuitabilityRun>,
    pub rejected_run_count: u64,
    pub reclassified_run_count: u64,
    pub full_evaluation_kinematic_violation_count: u64,
    pub in_domain_kinematic_violation_count: u64,
    pub out_of_domain_kinematic_violation_count: u64,
    pub rejecting_processor_finding_count: u64,
    pub informational_processor_finding_count: u64,
    pub source_format_finding_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyDomainAwareSuitabilityRun {
    pub nuclide: String,
    pub processor_report: NjoyExecutionArtifact,
    pub full_evaluation_diagnostic_status: NjoyRunDiagnosticStatus,
    pub full_evaluation_diagnostic_violation_count: u64,
    pub in_domain_diagnostic_status: NjoyRunDiagnosticStatus,
    pub in_domain_diagnostic_violation_count: u64,
    pub out_of_domain_diagnostic_violations: Vec<NjoyKinematicViolation>,
    pub heatr_photon_source: HeatrPhotonSource,
    pub file13_without_file12_reaction_count: u64,
    pub source_format_findings: Vec<crate::EndfPhotonFormatFinding>,
    pub processor_findings: Vec<NjoySourceAwareProcessorFinding>,
    pub source_aware_suitability: NjoySuitabilityStatus,
    pub suitability: NjoySuitabilityStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyDomainAwareSuitabilityReportDocument {
    pub report: NjoyDomainAwareSuitabilityReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyDomainAwareSuitabilityResult {
    pub report: NjoyDomainAwareSuitabilityReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl NjoyDomainAwareSuitabilityReport {
    /// Classify the exact receipt's diagnostic energies against a transport
    /// domain that is independently bound to the executed material.
    pub fn assess(
        source_aware: &NjoySourceAwareSuitabilityReportDocument,
        legacy: &NjoySuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        input_manifest_bytes: &[u8],
        nuclear_data_manifest_bytes: &[u8],
        material_bytes: &[u8],
        transport_domain: &OpenMcNeutronTransportDomainDocument,
    ) -> Result<Self, NjoyDomainAwareSuitabilityError> {
        transport_domain.verify_against_inputs(nuclear_data_manifest_bytes, material_bytes)?;
        let source_aware_reference = ContentReference {
            id: source_aware.report.id.clone(),
            sha256: source_aware.sha256.clone(),
        };
        let legacy_reference = ContentReference {
            id: legacy.report.id.clone(),
            sha256: legacy.sha256.clone(),
        };
        if source_aware.report.legacy_suitability_report != legacy_reference {
            return Err(NjoyDomainAwareSuitabilityError::LegacyBindingMismatch);
        }

        let execution_reference = ContentReference {
            id: execution.receipt.id.clone(),
            sha256: execution.sha256.clone(),
        };
        if legacy.report.execution_receipt != execution_reference {
            return Err(NjoyDomainAwareSuitabilityError::ExecutionBindingMismatch);
        }

        let input_manifest: NjoyInputManifest = serde_json::from_slice(input_manifest_bytes)?;
        if !matches!(
            input_manifest.schema_version.as_str(),
            NJOY_INPUT_MANIFEST_SCHEMA | NJOY_INPUT_MANIFEST_MIXED_SCHEMA
        ) {
            return Err(NjoyDomainAwareSuitabilityError::InputManifestBindingMismatch);
        }
        let input_manifest_reference = ContentReference {
            id: input_manifest.id.clone(),
            sha256: sha256_bytes(input_manifest_bytes),
        };
        if source_aware.report.input_manifest != input_manifest_reference
            || execution.receipt.input_manifest != input_manifest_reference
        {
            return Err(NjoyDomainAwareSuitabilityError::InputManifestBindingMismatch);
        }
        if input_manifest.bindings.material != transport_domain.domain.material {
            return Err(NjoyDomainAwareSuitabilityError::MaterialDomainBindingMismatch);
        }
        if source_aware.report.case_id != legacy.report.case_id
            || source_aware.report.case_id != execution.receipt.case_id
            || source_aware.report.case_id != input_manifest.case_id
        {
            return Err(NjoyDomainAwareSuitabilityError::CaseBindingMismatch);
        }
        if source_aware.report.requirement != legacy.report.requirement {
            return Err(NjoyDomainAwareSuitabilityError::RequirementBindingMismatch);
        }
        if source_aware.report.runs.len() != execution.receipt.runs.len()
            || source_aware.report.runs.len() != legacy.report.runs.len()
        {
            return Err(NjoyDomainAwareSuitabilityError::NuclideSetMismatch);
        }

        let mut runs = Vec::with_capacity(source_aware.report.runs.len());
        for ((source_run, legacy_run), execution_run) in source_aware
            .report
            .runs
            .iter()
            .zip(&legacy.report.runs)
            .zip(&execution.receipt.runs)
        {
            if source_run.nuclide != legacy_run.nuclide
                || source_run.nuclide != execution_run.nuclide
                || source_run.processor_report != legacy_run.processor_report
                || source_run.processor_report != execution_run.processor_report
                || source_run.diagnostic_status != execution_run.diagnostic_status
                || source_run.diagnostic_violation_count != execution_run.diagnostic_violation_count
            {
                return Err(NjoyDomainAwareSuitabilityError::RunEvidenceMismatch(
                    source_run.nuclide.clone(),
                ));
            }

            let out_of_domain_diagnostic_violations = execution_run
                .diagnostic_violations
                .iter()
                .filter(|violation| {
                    !transport_domain
                        .domain
                        .contains_diagnostic_energy(violation.energy_ev)
                })
                .cloned()
                .collect::<Vec<_>>();
            let out_of_domain_count = out_of_domain_diagnostic_violations.len() as u64;
            let in_domain_diagnostic_violation_count = execution_run
                .diagnostic_violation_count
                .checked_sub(out_of_domain_count)
                .ok_or_else(|| {
                    NjoyDomainAwareSuitabilityError::RunEvidenceMismatch(source_run.nuclide.clone())
                })?;
            let in_domain_diagnostic_status =
                diagnostic_status(in_domain_diagnostic_violation_count);
            let suitability = suitability(
                in_domain_diagnostic_violation_count,
                &source_run.source_format_findings,
                &source_run.processor_findings,
            );

            runs.push(NjoyDomainAwareSuitabilityRun {
                nuclide: source_run.nuclide.clone(),
                processor_report: source_run.processor_report.clone(),
                full_evaluation_diagnostic_status: source_run.diagnostic_status,
                full_evaluation_diagnostic_violation_count: source_run.diagnostic_violation_count,
                in_domain_diagnostic_status,
                in_domain_diagnostic_violation_count,
                out_of_domain_diagnostic_violations,
                heatr_photon_source: source_run.heatr_photon_source,
                file13_without_file12_reaction_count: source_run
                    .file13_without_file12_reaction_count,
                source_format_findings: source_run.source_format_findings.clone(),
                processor_findings: source_run.processor_findings.clone(),
                source_aware_suitability: source_run.suitability,
                suitability,
            });
        }

        let rejected_run_count = count_rejected(&runs);
        let report = Self {
            schema_version: NJOY_DOMAIN_AWARE_SUITABILITY_SCHEMA.into(),
            id: format!("{}.{}", source_aware.report.id, REPORT_ID_SUFFIX),
            case_id: source_aware.report.case_id.clone(),
            requirement: source_aware.report.requirement,
            qualification: qualification(rejected_run_count),
            source_aware_suitability_report: source_aware_reference,
            legacy_suitability_report: legacy_reference,
            execution_receipt: execution_reference,
            input_manifest: input_manifest_reference,
            neutron_transport_domain: ContentReference {
                id: transport_domain.domain.id.clone(),
                sha256: transport_domain.sha256.clone(),
            },
            assessment_energy_range_ev: transport_domain.domain.energy_range_ev,
            diagnostic_boundary_policy: transport_domain.domain.diagnostic_boundary_policy,
            reclassified_run_count: count_reclassified(&runs),
            full_evaluation_kinematic_violation_count: runs
                .iter()
                .map(|run| run.full_evaluation_diagnostic_violation_count)
                .sum(),
            in_domain_kinematic_violation_count: runs
                .iter()
                .map(|run| run.in_domain_diagnostic_violation_count)
                .sum(),
            out_of_domain_kinematic_violation_count: runs
                .iter()
                .map(|run| run.out_of_domain_diagnostic_violations.len() as u64)
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
            rejected_run_count,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), NjoyDomainAwareSuitabilityError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_DOMAIN_AWARE_SUITABILITY_SCHEMA,
        ) {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        for (label, reference) in [
            (
                "source_aware_suitability_report",
                &self.source_aware_suitability_report,
            ),
            ("legacy_suitability_report", &self.legacy_suitability_report),
            ("execution_receipt", &self.execution_receipt),
            ("input_manifest", &self.input_manifest),
            ("neutron_transport_domain", &self.neutron_transport_domain),
        ] {
            validate_identifier(label, &reference.id)?;
            validate_sha256(label, &reference.sha256)?;
        }
        if self.id
            != format!(
                "{}.{}",
                self.source_aware_suitability_report.id, REPORT_ID_SUFFIX
            )
        {
            return invalid_report("report ID does not bind the source-aware report");
        }
        let [lower, upper] = self.assessment_energy_range_ev;
        if !lower.is_finite() || !upper.is_finite() || lower < 0.0 || lower >= upper {
            return invalid_report("assessment energy interval is invalid");
        }
        if self.diagnostic_boundary_policy != OpenMcDiagnosticBoundaryPolicy::ClosedConservative {
            return invalid_report("unsupported diagnostic boundary policy");
        }
        if self.runs.is_empty() {
            return invalid_report("domain-aware report contains no runs");
        }

        let mut previous_nuclide: Option<&str> = None;
        for run in &self.runs {
            validate_identifier("runs.nuclide", &run.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= run.nuclide.as_str()) {
                return invalid_report("runs are not strictly ordered by nuclide");
            }
            previous_nuclide = Some(&run.nuclide);
            validate_run(run, self.assessment_energy_range_ev)?;
        }

        let rejected_run_count = count_rejected(&self.runs);
        let reclassified_run_count = count_reclassified(&self.runs);
        let full_evaluation_kinematic_violation_count: u64 = self
            .runs
            .iter()
            .map(|run| run.full_evaluation_diagnostic_violation_count)
            .sum();
        let in_domain_kinematic_violation_count: u64 = self
            .runs
            .iter()
            .map(|run| run.in_domain_diagnostic_violation_count)
            .sum();
        let out_of_domain_kinematic_violation_count: u64 = self
            .runs
            .iter()
            .map(|run| run.out_of_domain_diagnostic_violations.len() as u64)
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
            || self.reclassified_run_count != reclassified_run_count
            || self.full_evaluation_kinematic_violation_count
                != full_evaluation_kinematic_violation_count
            || self.in_domain_kinematic_violation_count != in_domain_kinematic_violation_count
            || self.out_of_domain_kinematic_violation_count
                != out_of_domain_kinematic_violation_count
            || self.rejecting_processor_finding_count != rejecting_processor_finding_count
            || self.informational_processor_finding_count != informational_processor_finding_count
            || self.source_format_finding_count != source_format_finding_count
        {
            return invalid_report("aggregate counts do not match the runs");
        }
        if self.full_evaluation_kinematic_violation_count
            != self.in_domain_kinematic_violation_count
                + self.out_of_domain_kinematic_violation_count
        {
            return invalid_report("kinematic violation partition is incomplete");
        }
        if self.qualification != qualification(rejected_run_count) {
            return invalid_report("qualification does not match the runs");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyDomainAwareSuitabilityResult, NjoyDomainAwareSuitabilityError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyDomainAwareSuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyDomainAwareSuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyDomainAwareSuitabilityResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyDomainAwareSuitabilityReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyDomainAwareSuitabilityError> {
        let report: NjoyDomainAwareSuitabilityReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyDomainAwareSuitabilityError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn verify_against_evidence(
        &self,
        source_aware: &NjoySourceAwareSuitabilityReportDocument,
        legacy: &NjoySuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        input_manifest_bytes: &[u8],
        nuclear_data_manifest_bytes: &[u8],
        material_bytes: &[u8],
        transport_domain: &OpenMcNeutronTransportDomainDocument,
    ) -> Result<(), NjoyDomainAwareSuitabilityError> {
        let observed = NjoyDomainAwareSuitabilityReport::assess(
            source_aware,
            legacy,
            execution,
            input_manifest_bytes,
            nuclear_data_manifest_bytes,
            material_bytes,
            transport_domain,
        )?;
        if self.report != observed {
            return Err(NjoyDomainAwareSuitabilityError::AssessmentMismatch);
        }
        Ok(())
    }
}

fn validate_run(
    run: &NjoyDomainAwareSuitabilityRun,
    energy_range_ev: [f64; 2],
) -> Result<(), NjoyDomainAwareSuitabilityError> {
    if run.processor_report.size_bytes == 0 {
        return invalid_report("processor report reference is empty");
    }
    validate_sha256("runs.processor_report", &run.processor_report.sha256)?;
    let out_of_domain_count = run.out_of_domain_diagnostic_violations.len() as u64;
    if run.full_evaluation_diagnostic_violation_count
        != run.in_domain_diagnostic_violation_count + out_of_domain_count
    {
        return invalid_report("run diagnostic partition is incomplete");
    }
    if run.full_evaluation_diagnostic_status
        != diagnostic_status(run.full_evaluation_diagnostic_violation_count)
        || run.in_domain_diagnostic_status
            != diagnostic_status(run.in_domain_diagnostic_violation_count)
    {
        return invalid_report("run diagnostic status and count disagree");
    }

    let mut previous = None;
    for violation in &run.out_of_domain_diagnostic_violations {
        if !violation.energy_ev.is_finite()
            || violation.energy_ev <= 0.0
            || violation.response_mt == 0
            || contains_energy(energy_range_ev, violation.energy_ev)
        {
            return invalid_report("run contains an invalid out-of-domain violation");
        }
        let current = (
            violation.energy_ev.to_bits(),
            violation.response_mt,
            match violation.direction {
                crate::NjoyKinematicDirection::Low => 0_u8,
                crate::NjoyKinematicDirection::High => 1_u8,
            },
        );
        if previous.is_some_and(|prior| prior >= current) {
            return invalid_report("out-of-domain violations are not strictly ordered");
        }
        previous = Some(current);
    }

    for interpreted in &run.processor_findings {
        if interpreted.finding.occurrence_count == 0 {
            return invalid_report("processor finding has no occurrences");
        }
        if interpreted.disposition
            == NjoyProcessorFindingDisposition::InformationalFile13Alternative
            && (interpreted.finding.kind
                != crate::NjoySuitabilityFindingKind::MissingPhotonMultiplicityFile
                || run.file13_without_file12_reaction_count == 0
                || !run.heatr_photon_source.transports_secondary_photons()
                || !run.source_format_findings.is_empty())
        {
            return invalid_report("invalid informational File 13 disposition");
        }
    }
    let expected_source_status = suitability(
        run.full_evaluation_diagnostic_violation_count,
        &run.source_format_findings,
        &run.processor_findings,
    );
    let expected_domain_status = suitability(
        run.in_domain_diagnostic_violation_count,
        &run.source_format_findings,
        &run.processor_findings,
    );
    if run.source_aware_suitability != expected_source_status
        || run.suitability != expected_domain_status
    {
        return invalid_report("run suitability does not match its evidence");
    }
    Ok(())
}

fn contains_energy([lower, upper]: [f64; 2], energy_ev: f64) -> bool {
    energy_ev >= lower && energy_ev <= upper
}

fn diagnostic_status(violation_count: u64) -> NjoyRunDiagnosticStatus {
    if violation_count == 0 {
        NjoyRunDiagnosticStatus::WithinKinematicLimits
    } else {
        NjoyRunDiagnosticStatus::KinematicLimitsExceeded
    }
}

fn suitability(
    diagnostic_violation_count: u64,
    source_format_findings: &[crate::EndfPhotonFormatFinding],
    processor_findings: &[NjoySourceAwareProcessorFinding],
) -> NjoySuitabilityStatus {
    if diagnostic_violation_count > 0
        || !source_format_findings.is_empty()
        || processor_findings
            .iter()
            .any(|finding| finding.disposition == NjoyProcessorFindingDisposition::Rejecting)
    {
        NjoySuitabilityStatus::Rejected
    } else {
        NjoySuitabilityStatus::CandidateUnreviewed
    }
}

fn qualification(rejected_run_count: u64) -> NjoySuitabilityQualification {
    if rejected_run_count == 0 {
        NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
    } else {
        NjoySuitabilityQualification::TransportedPhotonKermaRejected
    }
}

fn count_rejected(runs: &[NjoyDomainAwareSuitabilityRun]) -> u64 {
    runs.iter()
        .filter(|run| run.suitability == NjoySuitabilityStatus::Rejected)
        .count() as u64
}

fn count_reclassified(runs: &[NjoyDomainAwareSuitabilityRun]) -> u64 {
    runs.iter()
        .filter(|run| {
            run.source_aware_suitability == NjoySuitabilityStatus::Rejected
                && run.suitability == NjoySuitabilityStatus::CandidateUnreviewed
        })
        .count() as u64
}

fn count_disposition(
    runs: &[NjoyDomainAwareSuitabilityRun],
    disposition: NjoyProcessorFindingDisposition,
) -> u64 {
    runs.iter()
        .flat_map(|run| &run.processor_findings)
        .filter(|finding| finding.disposition == disposition)
        .count() as u64
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyDomainAwareSuitabilityError> {
    if value.trim().is_empty() {
        return invalid_report(format!("{label} must not be empty"));
    }
    Ok(())
}

fn validate_sha256(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyDomainAwareSuitabilityError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_report(format!("{label} is not a lowercase SHA-256 digest"));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyDomainAwareSuitabilityError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| NjoyDomainAwareSuitabilityError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyDomainAwareSuitabilityError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyDomainAwareSuitabilityError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, NjoyDomainAwareSuitabilityError> {
    Err(NjoyDomainAwareSuitabilityError::InvalidReport(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoyDomainAwareSuitabilityError {
    #[error(transparent)]
    TransportDomain(#[from] OpenMcTransportDomainError),
    #[error("source-aware report does not bind the supplied legacy report")]
    LegacyBindingMismatch,
    #[error("legacy suitability report does not bind the supplied execution receipt")]
    ExecutionBindingMismatch,
    #[error("executed input manifest does not match the supplied evidence")]
    InputManifestBindingMismatch,
    #[error("transport domain does not bind the executed material")]
    MaterialDomainBindingMismatch,
    #[error("case identities do not match across domain-aware evidence")]
    CaseBindingMismatch,
    #[error("transport requirements do not match across domain-aware evidence")]
    RequirementBindingMismatch,
    #[error("nuclide sets do not match across domain-aware evidence")]
    NuclideSetMismatch,
    #[error("run evidence does not match for {0}")]
    RunEvidenceMismatch(String),
    #[error("invalid domain-aware transported-photon suitability report: {0}")]
    InvalidReport(String),
    #[error("domain-aware suitability report does not match regenerated evidence")]
    AssessmentMismatch,
    #[error("required domain-aware suitability artifact is not a regular file: {0}")]
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

    const SOURCE_AWARE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-source-aware-suitability.json"
    );
    const LEGACY: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-suitability.json"
    );
    const EXECUTION: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-execution-receipt.json"
    );
    const INPUT_MANIFEST: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/njoy/openbnct-njoy-input-manifest.json"
    );
    const MATERIAL: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const OPENMC_MANIFEST: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json"
    );
    const TRANSPORT_DOMAIN: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-neutron-transport-domain.json"
    );
    const FROZEN_ASSESSMENT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-domain-aware-suitability.json"
    );
    const BASELINE_SOURCE_AWARE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-transported-photon-source-aware-suitability.json"
    );
    const BASELINE_LEGACY: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-transported-photon-suitability.json"
    );
    const BASELINE_EXECUTION: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-execution-receipt.json"
    );
    const BASELINE_INPUT_MANIFEST: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/njoy/openbnct-njoy-input-manifest.json"
    );
    const FROZEN_BASELINE_ASSESSMENT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-transported-photon-domain-aware-suitability.json"
    );

    fn assessment() -> NjoyDomainAwareSuitabilityReport {
        let source_aware =
            NjoySourceAwareSuitabilityReportDocument::from_bytes(SOURCE_AWARE).unwrap();
        let legacy = NjoySuitabilityReportDocument::from_bytes(LEGACY).unwrap();
        let execution = NjoyExecutionReceiptDocument::from_bytes(EXECUTION).unwrap();
        let domain =
            openbnct_openmc::OpenMcNeutronTransportDomain::derive(OPENMC_MANIFEST, MATERIAL)
                .unwrap();
        let domain_bytes = serde_json::to_vec(&domain).unwrap();
        let domain = OpenMcNeutronTransportDomainDocument::from_bytes(&domain_bytes).unwrap();
        NjoyDomainAwareSuitabilityReport::assess(
            &source_aware,
            &legacy,
            &execution,
            INPUT_MANIFEST,
            OPENMC_MANIFEST,
            MATERIAL,
            &domain,
        )
        .unwrap()
    }

    #[test]
    fn only_reclassifies_o16s_strictly_out_of_domain_violation() {
        let report = assessment();
        assert_eq!(report.full_evaluation_kinematic_violation_count, 120);
        assert_eq!(report.in_domain_kinematic_violation_count, 114);
        assert_eq!(report.out_of_domain_kinematic_violation_count, 6);
        assert_eq!(report.reclassified_run_count, 1);
        assert_eq!(report.rejected_run_count, 4);
        let oxygen = report.runs.iter().find(|run| run.nuclide == "O16").unwrap();
        assert_eq!(
            oxygen.source_aware_suitability,
            NjoySuitabilityStatus::Rejected
        );
        assert_eq!(
            oxygen.suitability,
            NjoySuitabilityStatus::CandidateUnreviewed
        );
        assert_eq!(oxygen.in_domain_diagnostic_violation_count, 0);
        assert_eq!(oxygen.out_of_domain_diagnostic_violations.len(), 1);
        assert_eq!(
            oxygen.out_of_domain_diagnostic_violations[0].energy_ev,
            30.0e6
        );
    }

    #[test]
    fn upper_domain_boundary_remains_rejecting() {
        let mut report = assessment();
        let oxygen = report
            .runs
            .iter_mut()
            .find(|run| run.nuclide == "O16")
            .unwrap();
        oxygen.out_of_domain_diagnostic_violations[0].energy_ev = 20.0e6;
        assert!(matches!(
            report.validate(),
            Err(NjoyDomainAwareSuitabilityError::InvalidReport(_))
        ));
    }

    #[test]
    fn rejects_a_domain_for_a_different_material_serialization() {
        let source_aware =
            NjoySourceAwareSuitabilityReportDocument::from_bytes(SOURCE_AWARE).unwrap();
        let legacy = NjoySuitabilityReportDocument::from_bytes(LEGACY).unwrap();
        let execution = NjoyExecutionReceiptDocument::from_bytes(EXECUTION).unwrap();
        let mut material: serde_json::Value = serde_json::from_slice(MATERIAL).unwrap();
        material["density_g_cm3"] = serde_json::json!(1.01);
        let changed = serde_json::to_vec(&material).unwrap();
        let domain =
            openbnct_openmc::OpenMcNeutronTransportDomain::derive(OPENMC_MANIFEST, &changed)
                .unwrap();
        let domain_bytes = serde_json::to_vec(&domain).unwrap();
        let domain = OpenMcNeutronTransportDomainDocument::from_bytes(&domain_bytes).unwrap();
        assert!(matches!(
            NjoyDomainAwareSuitabilityReport::assess(
                &source_aware,
                &legacy,
                &execution,
                INPUT_MANIFEST,
                OPENMC_MANIFEST,
                &changed,
                &domain,
            ),
            Err(NjoyDomainAwareSuitabilityError::MaterialDomainBindingMismatch)
        ));
    }

    #[test]
    fn rejects_a_well_formed_but_underived_narrower_domain() {
        let source_aware =
            NjoySourceAwareSuitabilityReportDocument::from_bytes(SOURCE_AWARE).unwrap();
        let legacy = NjoySuitabilityReportDocument::from_bytes(LEGACY).unwrap();
        let execution = NjoyExecutionReceiptDocument::from_bytes(EXECUTION).unwrap();
        let mut domain =
            openbnct_openmc::OpenMcNeutronTransportDomain::derive(OPENMC_MANIFEST, MATERIAL)
                .unwrap();
        domain.energy_range_ev[1] = 1.0e6;
        let domain_bytes = serde_json::to_vec(&domain).unwrap();
        let domain = OpenMcNeutronTransportDomainDocument::from_bytes(&domain_bytes).unwrap();
        assert!(matches!(
            NjoyDomainAwareSuitabilityReport::assess(
                &source_aware,
                &legacy,
                &execution,
                INPUT_MANIFEST,
                OPENMC_MANIFEST,
                MATERIAL,
                &domain,
            ),
            Err(NjoyDomainAwareSuitabilityError::TransportDomain(
                OpenMcTransportDomainError::DerivationMismatch
            ))
        ));
    }

    #[test]
    fn verifies_frozen_assessment_against_exact_evidence() {
        let source_aware =
            NjoySourceAwareSuitabilityReportDocument::from_bytes(SOURCE_AWARE).unwrap();
        let legacy = NjoySuitabilityReportDocument::from_bytes(LEGACY).unwrap();
        let execution = NjoyExecutionReceiptDocument::from_bytes(EXECUTION).unwrap();
        let domain = OpenMcNeutronTransportDomainDocument::from_bytes(TRANSPORT_DOMAIN).unwrap();
        let assessment =
            NjoyDomainAwareSuitabilityReportDocument::from_bytes(FROZEN_ASSESSMENT).unwrap();
        assert_eq!(
            assessment.sha256,
            "6e46b627d9b766e596ad2219eaafca970bd9f3c5df1d5e400ad644397c44ce55"
        );
        assessment
            .verify_against_evidence(
                &source_aware,
                &legacy,
                &execution,
                INPUT_MANIFEST,
                OPENMC_MANIFEST,
                MATERIAL,
                &domain,
            )
            .unwrap();
    }

    #[test]
    fn applies_the_same_domain_to_the_frozen_baseline() {
        let source_aware =
            NjoySourceAwareSuitabilityReportDocument::from_bytes(BASELINE_SOURCE_AWARE).unwrap();
        let legacy = NjoySuitabilityReportDocument::from_bytes(BASELINE_LEGACY).unwrap();
        let execution = NjoyExecutionReceiptDocument::from_bytes(BASELINE_EXECUTION).unwrap();
        let domain = OpenMcNeutronTransportDomainDocument::from_bytes(TRANSPORT_DOMAIN).unwrap();
        let assessment =
            NjoyDomainAwareSuitabilityReportDocument::from_bytes(FROZEN_BASELINE_ASSESSMENT)
                .unwrap();
        assert_eq!(
            assessment.sha256,
            "e270708da7aabf0be6246d8b89fabf031af4ec01c155b015432e2ee174eb9d09"
        );
        assert_eq!(assessment.report.reclassified_run_count, 0);
        assert_eq!(assessment.report.out_of_domain_kinematic_violation_count, 0);
        assessment
            .verify_against_evidence(
                &source_aware,
                &legacy,
                &execution,
                BASELINE_INPUT_MANIFEST,
                OPENMC_MANIFEST,
                MATERIAL,
                &domain,
            )
            .unwrap();
    }
}
