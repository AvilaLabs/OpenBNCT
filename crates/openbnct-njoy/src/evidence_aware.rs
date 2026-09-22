// SPDX-License-Identifier: MIT

//! Evidence-aware transported-photon suitability layered over immutable v0.3
//! domain evidence, the H-2 LAW=7 attribution, and the independent N-15
//! capture-balance gate.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::OpenMcDiagnosticBoundaryPolicy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    EndfMf6CapturePhotonBalanceError, EndfMf6CapturePhotonBalanceQualification,
    EndfMf6CapturePhotonBalanceReportDocument, EndfMf6Law7ImplicitResidualError,
    EndfMf6Law7ImplicitResidualQualification, EndfMf6Law7ImplicitResidualReportDocument,
    NjoyCapturePhotonMomentComparisonDocument, NjoyCapturePhotonMomentComparisonError,
    NjoyCapturePhotonMomentComparisonQualification, NjoyDomainAwareSuitabilityError,
    NjoyDomainAwareSuitabilityReportDocument, NjoyLaw7ImplicitResidualComparisonDocument,
    NjoyLaw7ImplicitResidualComparisonError, NjoyLaw7ImplicitResidualComparisonQualification,
    NjoyProcessorFindingDisposition, NjoySuitabilityQualification, NjoySuitabilityStatus,
    NjoyTransportRequirement,
};

pub const NJOY_EVIDENCE_AWARE_SUITABILITY_SCHEMA: &str =
    "openbnct.njoy-transported-photon-suitability/0.4.0";

const REPORT_ID_SUFFIX: &str = "evidence-aware-v4";
const H2: &str = "H2";
const N15: &str = "N15";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyEvidenceAwareSuitabilityReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub requirement: NjoyTransportRequirement,
    pub qualification: NjoySuitabilityQualification,
    pub domain_aware_suitability_report: ContentReference,
    pub law7_implicit_residual_report: ContentReference,
    pub law7_processor_attribution: ContentReference,
    pub capture_balance_report: ContentReference,
    pub capture_processor_comparison: ContentReference,
    pub execution_receipt: ContentReference,
    pub evaluated_source_selection: ContentReference,
    pub photon_production_inventory: ContentReference,
    pub neutron_transport_domain: ContentReference,
    pub assessment_energy_range_ev: [f64; 2],
    pub diagnostic_boundary_policy: OpenMcDiagnosticBoundaryPolicy,
    pub runs: Vec<NjoyEvidenceAwareSuitabilityRun>,
    pub rejected_run_count: u64,
    pub reclassified_from_domain_run_count: u64,
    pub independently_rejected_from_domain_run_count: u64,
    pub full_evaluation_kinematic_violation_count: u64,
    pub domain_in_scope_kinematic_violation_count: u64,
    pub approximation_attributed_full_evaluation_violation_count: u64,
    pub approximation_attributed_in_domain_violation_count: u64,
    pub approximation_attributed_out_of_domain_violation_count: u64,
    pub remaining_in_domain_kinematic_violation_count: u64,
    pub out_of_domain_kinematic_violation_count: u64,
    pub independent_capture_balance_failed_sample_count: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyEvidenceAwareSuitabilityRun {
    pub nuclide: String,
    pub full_evaluation_diagnostic_violation_count: u64,
    pub in_domain_diagnostic_violation_count: u64,
    pub out_of_domain_diagnostic_violation_count: u64,
    pub approximation_attributed_full_evaluation_violation_count: u64,
    pub approximation_attributed_in_domain_violation_count: u64,
    pub approximation_attributed_out_of_domain_violation_count: u64,
    pub remaining_in_domain_diagnostic_violation_count: u64,
    pub rejecting_nonkinematic_finding_count: u64,
    pub kinematic_disposition: NjoyEvidenceAwareKinematicDisposition,
    pub independent_gate: NjoyEvidenceAwareIndependentGate,
    pub domain_aware_suitability: NjoySuitabilityStatus,
    pub suitability: NjoySuitabilityStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEvidenceAwareKinematicDisposition {
    None,
    Law7ImplicitResidualProcessorApproximation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEvidenceAwareIndependentGate {
    None,
    Mf6CapturePhotonEnergyBalanceRejected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyEvidenceAwareSuitabilityReportDocument {
    pub report: NjoyEvidenceAwareSuitabilityReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyEvidenceAwareSuitabilityResult {
    pub report: NjoyEvidenceAwareSuitabilityReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl NjoyEvidenceAwareSuitabilityReport {
    pub fn assess(
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        law7_residual: &EndfMf6Law7ImplicitResidualReportDocument,
        law7_attribution: &NjoyLaw7ImplicitResidualComparisonDocument,
        capture_balance: &EndfMf6CapturePhotonBalanceReportDocument,
        capture_comparison: &NjoyCapturePhotonMomentComparisonDocument,
    ) -> Result<Self, NjoyEvidenceAwareSuitabilityError> {
        domain.report.validate()?;
        law7_residual.report.validate()?;
        law7_attribution.comparison.validate()?;
        capture_balance.report.validate()?;
        capture_comparison.comparison.validate()?;

        let law7_residual_reference = ContentReference {
            id: law7_residual.report.id.clone(),
            sha256: law7_residual.sha256.clone(),
        };
        let capture_balance_reference = ContentReference {
            id: capture_balance.report.id.clone(),
            sha256: capture_balance.sha256.clone(),
        };
        if law7_attribution.comparison.independent_residual_report != law7_residual_reference
            || capture_comparison.comparison.independent_balance_report != capture_balance_reference
        {
            return Err(NjoyEvidenceAwareSuitabilityError::IndependentReportBindingMismatch);
        }
        if law7_attribution.comparison.execution_receipt != domain.report.execution_receipt
            || capture_comparison.comparison.execution_receipt != domain.report.execution_receipt
        {
            return Err(NjoyEvidenceAwareSuitabilityError::ExecutionBindingMismatch);
        }
        if law7_residual.report.evaluated_source_selection
            != capture_balance.report.evaluated_source_selection
            || law7_residual.report.photon_production_inventory
                != capture_balance.report.photon_production_inventory
        {
            return Err(NjoyEvidenceAwareSuitabilityError::SourceBindingMismatch);
        }
        if domain.report.case_id != law7_residual.report.case_id
            || domain.report.case_id != law7_attribution.comparison.case_id
            || domain.report.case_id != capture_balance.report.case_id
            || domain.report.case_id != capture_comparison.comparison.case_id
        {
            return Err(NjoyEvidenceAwareSuitabilityError::CaseBindingMismatch);
        }
        if law7_residual.report.nuclide != H2
            || law7_residual.report.qualification
                != EndfMf6Law7ImplicitResidualQualification::
                    ImplicitResidualEnergyCheckedUnreviewed
            || law7_attribution.comparison.nuclide != H2
            || law7_attribution.comparison.qualification
                != NjoyLaw7ImplicitResidualComparisonQualification::
                    ProcessorApproximationFullyAttributedUnreviewed
            || law7_attribution.comparison.failed_sample_count != 0
            || law7_attribution.comparison.attributed_violation_count
                != law7_attribution.comparison.receipt_violation_count
        {
            return Err(NjoyEvidenceAwareSuitabilityError::Law7AttributionRejected);
        }
        if capture_balance.report.nuclide != N15
            || capture_balance.report.qualification
                != EndfMf6CapturePhotonBalanceQualification::CapturePhotonEnergyBalanceRejected
            || capture_balance.report.failed_energy_balance_sample_count == 0
            || capture_comparison.comparison.nuclide != N15
            || capture_comparison.comparison.qualification
                != NjoyCapturePhotonMomentComparisonQualification::
                    IndependentCaptureMomentsMatchProcessorPrintUnreviewed
            || capture_comparison.comparison.failed_sample_count != 0
        {
            return Err(NjoyEvidenceAwareSuitabilityError::CaptureGateMismatch);
        }

        let h2_domain = domain
            .report
            .runs
            .iter()
            .find(|run| run.nuclide == H2)
            .ok_or(NjoyEvidenceAwareSuitabilityError::MissingRequiredNuclide(
                H2,
            ))?;
        let n15_domain = domain
            .report
            .runs
            .iter()
            .find(|run| run.nuclide == N15)
            .ok_or(NjoyEvidenceAwareSuitabilityError::MissingRequiredNuclide(
                N15,
            ))?;
        if h2_domain.processor_report != law7_attribution.comparison.processor_report
            || n15_domain.processor_report != capture_comparison.comparison.processor_report
            || h2_domain.full_evaluation_diagnostic_violation_count
                != law7_attribution.comparison.receipt_violation_count
            || !h2_domain.source_format_findings.is_empty()
            || h2_domain
                .processor_findings
                .iter()
                .any(|finding| finding.disposition == NjoyProcessorFindingDisposition::Rejecting)
            || h2_domain.suitability != NjoySuitabilityStatus::Rejected
            || n15_domain.suitability != NjoySuitabilityStatus::CandidateUnreviewed
        {
            return Err(NjoyEvidenceAwareSuitabilityError::RunEvidenceMismatch);
        }

        let attributed_out_of_domain = law7_attribution
            .comparison
            .samples
            .iter()
            .filter(|sample| {
                sample.receipt_kinematic_violation
                    && !contains_energy(
                        domain.report.assessment_energy_range_ev,
                        sample.incident_energy_ev,
                    )
            })
            .map(|sample| {
                (
                    sample.incident_energy_ev,
                    sample
                        .processor_kinematic_direction
                        .expect("receipt violation direction is validated"),
                )
            })
            .collect::<Vec<_>>();
        let domain_h2_out_of_domain = h2_domain
            .out_of_domain_diagnostic_violations
            .iter()
            .map(|violation| (violation.energy_ev, violation.direction))
            .collect::<Vec<_>>();
        if attributed_out_of_domain != domain_h2_out_of_domain {
            return Err(NjoyEvidenceAwareSuitabilityError::DomainPartitionMismatch);
        }
        let attributed_in_domain = law7_attribution
            .comparison
            .samples
            .iter()
            .filter(|sample| {
                sample.receipt_kinematic_violation
                    && contains_energy(
                        domain.report.assessment_energy_range_ev,
                        sample.incident_energy_ev,
                    )
            })
            .count() as u64;
        if attributed_in_domain != h2_domain.in_domain_diagnostic_violation_count {
            return Err(NjoyEvidenceAwareSuitabilityError::DomainPartitionMismatch);
        }

        let mut runs = Vec::with_capacity(domain.report.runs.len());
        for domain_run in &domain.report.runs {
            let is_h2 = domain_run.nuclide == H2;
            let is_n15 = domain_run.nuclide == N15;
            let approximation_attributed_full_evaluation_violation_count = if is_h2 {
                law7_attribution.comparison.attributed_violation_count
            } else {
                0
            };
            let approximation_attributed_in_domain_violation_count =
                if is_h2 { attributed_in_domain } else { 0 };
            let approximation_attributed_out_of_domain_violation_count = if is_h2 {
                attributed_out_of_domain.len() as u64
            } else {
                0
            };
            let remaining_in_domain_diagnostic_violation_count = domain_run
                .in_domain_diagnostic_violation_count
                .checked_sub(approximation_attributed_in_domain_violation_count)
                .ok_or(NjoyEvidenceAwareSuitabilityError::DomainPartitionMismatch)?;
            let rejecting_nonkinematic_finding_count = domain_run.source_format_findings.len()
                as u64
                + domain_run
                    .processor_findings
                    .iter()
                    .filter(|finding| {
                        finding.disposition == NjoyProcessorFindingDisposition::Rejecting
                    })
                    .count() as u64;
            let kinematic_disposition = if is_h2 {
                NjoyEvidenceAwareKinematicDisposition::Law7ImplicitResidualProcessorApproximation
            } else {
                NjoyEvidenceAwareKinematicDisposition::None
            };
            let independent_gate = if is_n15 {
                NjoyEvidenceAwareIndependentGate::Mf6CapturePhotonEnergyBalanceRejected
            } else {
                NjoyEvidenceAwareIndependentGate::None
            };
            let suitability = evidence_aware_suitability(
                remaining_in_domain_diagnostic_violation_count,
                rejecting_nonkinematic_finding_count,
                independent_gate,
            );
            runs.push(NjoyEvidenceAwareSuitabilityRun {
                nuclide: domain_run.nuclide.clone(),
                full_evaluation_diagnostic_violation_count: domain_run
                    .full_evaluation_diagnostic_violation_count,
                in_domain_diagnostic_violation_count: domain_run
                    .in_domain_diagnostic_violation_count,
                out_of_domain_diagnostic_violation_count: domain_run
                    .out_of_domain_diagnostic_violations
                    .len() as u64,
                approximation_attributed_full_evaluation_violation_count,
                approximation_attributed_in_domain_violation_count,
                approximation_attributed_out_of_domain_violation_count,
                remaining_in_domain_diagnostic_violation_count,
                rejecting_nonkinematic_finding_count,
                kinematic_disposition,
                independent_gate,
                domain_aware_suitability: domain_run.suitability,
                suitability,
            });
        }

        let rejected_run_count = count_rejected(&runs);
        let report = Self {
            schema_version: NJOY_EVIDENCE_AWARE_SUITABILITY_SCHEMA.into(),
            id: format!("{}.{}", domain.report.id, REPORT_ID_SUFFIX),
            case_id: domain.report.case_id.clone(),
            requirement: domain.report.requirement,
            qualification: qualification(rejected_run_count),
            domain_aware_suitability_report: ContentReference {
                id: domain.report.id.clone(),
                sha256: domain.sha256.clone(),
            },
            law7_implicit_residual_report: law7_residual_reference,
            law7_processor_attribution: ContentReference {
                id: law7_attribution.comparison.id.clone(),
                sha256: law7_attribution.sha256.clone(),
            },
            capture_balance_report: capture_balance_reference,
            capture_processor_comparison: ContentReference {
                id: capture_comparison.comparison.id.clone(),
                sha256: capture_comparison.sha256.clone(),
            },
            execution_receipt: domain.report.execution_receipt.clone(),
            evaluated_source_selection: law7_residual.report.evaluated_source_selection.clone(),
            photon_production_inventory: law7_residual.report.photon_production_inventory.clone(),
            neutron_transport_domain: domain.report.neutron_transport_domain.clone(),
            assessment_energy_range_ev: domain.report.assessment_energy_range_ev,
            diagnostic_boundary_policy: domain.report.diagnostic_boundary_policy,
            reclassified_from_domain_run_count: count_transition(
                &runs,
                NjoySuitabilityStatus::Rejected,
                NjoySuitabilityStatus::CandidateUnreviewed,
            ),
            independently_rejected_from_domain_run_count: count_transition(
                &runs,
                NjoySuitabilityStatus::CandidateUnreviewed,
                NjoySuitabilityStatus::Rejected,
            ),
            full_evaluation_kinematic_violation_count: sum_runs(&runs, |run| {
                run.full_evaluation_diagnostic_violation_count
            }),
            domain_in_scope_kinematic_violation_count: sum_runs(&runs, |run| {
                run.in_domain_diagnostic_violation_count
            }),
            approximation_attributed_full_evaluation_violation_count: sum_runs(&runs, |run| {
                run.approximation_attributed_full_evaluation_violation_count
            }),
            approximation_attributed_in_domain_violation_count: sum_runs(&runs, |run| {
                run.approximation_attributed_in_domain_violation_count
            }),
            approximation_attributed_out_of_domain_violation_count: sum_runs(&runs, |run| {
                run.approximation_attributed_out_of_domain_violation_count
            }),
            remaining_in_domain_kinematic_violation_count: sum_runs(&runs, |run| {
                run.remaining_in_domain_diagnostic_violation_count
            }),
            out_of_domain_kinematic_violation_count: sum_runs(&runs, |run| {
                run.out_of_domain_diagnostic_violation_count
            }),
            independent_capture_balance_failed_sample_count: capture_balance
                .report
                .failed_energy_balance_sample_count,
            runs,
            rejected_run_count,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), NjoyEvidenceAwareSuitabilityError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_EVIDENCE_AWARE_SUITABILITY_SCHEMA,
        ) {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        for (label, reference) in [
            (
                "domain_aware_suitability_report",
                &self.domain_aware_suitability_report,
            ),
            (
                "law7_implicit_residual_report",
                &self.law7_implicit_residual_report,
            ),
            (
                "law7_processor_attribution",
                &self.law7_processor_attribution,
            ),
            ("capture_balance_report", &self.capture_balance_report),
            (
                "capture_processor_comparison",
                &self.capture_processor_comparison,
            ),
            ("execution_receipt", &self.execution_receipt),
            (
                "evaluated_source_selection",
                &self.evaluated_source_selection,
            ),
            (
                "photon_production_inventory",
                &self.photon_production_inventory,
            ),
            ("neutron_transport_domain", &self.neutron_transport_domain),
        ] {
            validate_identifier(label, &reference.id)?;
            validate_sha256(label, &reference.sha256)?;
        }
        if self.id
            != format!(
                "{}.{}",
                self.domain_aware_suitability_report.id, REPORT_ID_SUFFIX
            )
        {
            return invalid_report("report ID does not bind the domain-aware report");
        }
        let [lower, upper] = self.assessment_energy_range_ev;
        if !lower.is_finite()
            || !upper.is_finite()
            || lower < 0.0
            || lower >= upper
            || self.diagnostic_boundary_policy != OpenMcDiagnosticBoundaryPolicy::ClosedConservative
            || self.runs.is_empty()
            || self.independent_capture_balance_failed_sample_count == 0
        {
            return invalid_report("invalid evidence-aware scope");
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
        if !self.runs.iter().any(|run| run.nuclide == H2)
            || !self.runs.iter().any(|run| run.nuclide == N15)
        {
            return invalid_report("required H2 or N15 run is absent");
        }

        let rejected_run_count = count_rejected(&self.runs);
        let reclassified_from_domain_run_count = count_transition(
            &self.runs,
            NjoySuitabilityStatus::Rejected,
            NjoySuitabilityStatus::CandidateUnreviewed,
        );
        let independently_rejected_from_domain_run_count = count_transition(
            &self.runs,
            NjoySuitabilityStatus::CandidateUnreviewed,
            NjoySuitabilityStatus::Rejected,
        );
        let full_count = sum_runs(&self.runs, |run| {
            run.full_evaluation_diagnostic_violation_count
        });
        let domain_count = sum_runs(&self.runs, |run| run.in_domain_diagnostic_violation_count);
        let attributed_full_count = sum_runs(&self.runs, |run| {
            run.approximation_attributed_full_evaluation_violation_count
        });
        let attributed_domain_count = sum_runs(&self.runs, |run| {
            run.approximation_attributed_in_domain_violation_count
        });
        let attributed_out_of_domain_count = sum_runs(&self.runs, |run| {
            run.approximation_attributed_out_of_domain_violation_count
        });
        let remaining_domain_count = sum_runs(&self.runs, |run| {
            run.remaining_in_domain_diagnostic_violation_count
        });
        let out_of_domain_count = sum_runs(&self.runs, |run| {
            run.out_of_domain_diagnostic_violation_count
        });
        if self.rejected_run_count != rejected_run_count
            || self.reclassified_from_domain_run_count != reclassified_from_domain_run_count
            || self.independently_rejected_from_domain_run_count
                != independently_rejected_from_domain_run_count
            || self.full_evaluation_kinematic_violation_count != full_count
            || self.domain_in_scope_kinematic_violation_count != domain_count
            || self.approximation_attributed_full_evaluation_violation_count
                != attributed_full_count
            || self.approximation_attributed_in_domain_violation_count != attributed_domain_count
            || self.approximation_attributed_out_of_domain_violation_count
                != attributed_out_of_domain_count
            || self.remaining_in_domain_kinematic_violation_count != remaining_domain_count
            || self.out_of_domain_kinematic_violation_count != out_of_domain_count
            || full_count != domain_count + out_of_domain_count
            || domain_count != attributed_domain_count + remaining_domain_count
            || attributed_full_count != attributed_domain_count + attributed_out_of_domain_count
            || self.qualification != qualification(rejected_run_count)
        {
            return invalid_report("aggregate counts do not match the runs");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyEvidenceAwareSuitabilityResult, NjoyEvidenceAwareSuitabilityError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyEvidenceAwareSuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyEvidenceAwareSuitabilityError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyEvidenceAwareSuitabilityResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyEvidenceAwareSuitabilityReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyEvidenceAwareSuitabilityError> {
        let report: NjoyEvidenceAwareSuitabilityReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyEvidenceAwareSuitabilityError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_evidence(
        &self,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        law7_residual: &EndfMf6Law7ImplicitResidualReportDocument,
        law7_attribution: &NjoyLaw7ImplicitResidualComparisonDocument,
        capture_balance: &EndfMf6CapturePhotonBalanceReportDocument,
        capture_comparison: &NjoyCapturePhotonMomentComparisonDocument,
    ) -> Result<(), NjoyEvidenceAwareSuitabilityError> {
        let observed = NjoyEvidenceAwareSuitabilityReport::assess(
            domain,
            law7_residual,
            law7_attribution,
            capture_balance,
            capture_comparison,
        )?;
        if self.report != observed {
            return Err(NjoyEvidenceAwareSuitabilityError::AssessmentMismatch);
        }
        Ok(())
    }
}

fn validate_run(
    run: &NjoyEvidenceAwareSuitabilityRun,
) -> Result<(), NjoyEvidenceAwareSuitabilityError> {
    if run.full_evaluation_diagnostic_violation_count
        != run.in_domain_diagnostic_violation_count + run.out_of_domain_diagnostic_violation_count
        || run.in_domain_diagnostic_violation_count
            != run.approximation_attributed_in_domain_violation_count
                + run.remaining_in_domain_diagnostic_violation_count
        || run.approximation_attributed_full_evaluation_violation_count
            < run.approximation_attributed_in_domain_violation_count
        || run.approximation_attributed_full_evaluation_violation_count
            != run.approximation_attributed_in_domain_violation_count
                + run.approximation_attributed_out_of_domain_violation_count
        || run.approximation_attributed_out_of_domain_violation_count
            > run.out_of_domain_diagnostic_violation_count
        || run.approximation_attributed_full_evaluation_violation_count
            > run.full_evaluation_diagnostic_violation_count
    {
        return invalid_report("run violation partition is inconsistent");
    }
    let expected_domain_status = if run.in_domain_diagnostic_violation_count > 0
        || run.rejecting_nonkinematic_finding_count > 0
    {
        NjoySuitabilityStatus::Rejected
    } else {
        NjoySuitabilityStatus::CandidateUnreviewed
    };
    if run.domain_aware_suitability != expected_domain_status {
        return invalid_report("run domain-aware status does not match its evidence");
    }
    match run.kinematic_disposition {
        NjoyEvidenceAwareKinematicDisposition::None => {
            if run.approximation_attributed_full_evaluation_violation_count != 0
                || run.approximation_attributed_in_domain_violation_count != 0
                || run.approximation_attributed_out_of_domain_violation_count != 0
            {
                return invalid_report("unattributed run removes kinematic violations");
            }
        }
        NjoyEvidenceAwareKinematicDisposition::Law7ImplicitResidualProcessorApproximation => {
            if run.nuclide != H2
                || run.approximation_attributed_full_evaluation_violation_count
                    != run.full_evaluation_diagnostic_violation_count
                || run.approximation_attributed_in_domain_violation_count
                    != run.in_domain_diagnostic_violation_count
                || run.approximation_attributed_out_of_domain_violation_count
                    != run.out_of_domain_diagnostic_violation_count
                || run.rejecting_nonkinematic_finding_count != 0
            {
                return invalid_report("invalid H2 LAW=7 approximation disposition");
            }
        }
    }
    match run.independent_gate {
        NjoyEvidenceAwareIndependentGate::None => {}
        NjoyEvidenceAwareIndependentGate::Mf6CapturePhotonEnergyBalanceRejected => {
            if run.nuclide != N15 {
                return invalid_report("capture-balance rejection applies outside N15");
            }
        }
    }
    if run.nuclide == H2
        && run.kinematic_disposition
            != NjoyEvidenceAwareKinematicDisposition::Law7ImplicitResidualProcessorApproximation
    {
        return invalid_report("H2 is missing its exact approximation disposition");
    }
    if run.nuclide == N15
        && run.independent_gate
            != NjoyEvidenceAwareIndependentGate::Mf6CapturePhotonEnergyBalanceRejected
    {
        return invalid_report("N15 is missing its independent capture gate");
    }
    if run.nuclide != H2 && run.kinematic_disposition != NjoyEvidenceAwareKinematicDisposition::None
    {
        return invalid_report("LAW=7 disposition applies outside H2");
    }
    if run.nuclide != N15 && run.independent_gate != NjoyEvidenceAwareIndependentGate::None {
        return invalid_report("independent capture gate applies outside N15");
    }
    let expected_suitability = evidence_aware_suitability(
        run.remaining_in_domain_diagnostic_violation_count,
        run.rejecting_nonkinematic_finding_count,
        run.independent_gate,
    );
    if run.suitability != expected_suitability {
        return invalid_report("run evidence-aware suitability is inconsistent");
    }
    Ok(())
}

fn evidence_aware_suitability(
    remaining_in_domain_violation_count: u64,
    rejecting_nonkinematic_finding_count: u64,
    independent_gate: NjoyEvidenceAwareIndependentGate,
) -> NjoySuitabilityStatus {
    if remaining_in_domain_violation_count > 0
        || rejecting_nonkinematic_finding_count > 0
        || independent_gate != NjoyEvidenceAwareIndependentGate::None
    {
        NjoySuitabilityStatus::Rejected
    } else {
        NjoySuitabilityStatus::CandidateUnreviewed
    }
}

fn contains_energy([lower, upper]: [f64; 2], energy_ev: f64) -> bool {
    energy_ev >= lower && energy_ev <= upper
}

fn count_rejected(runs: &[NjoyEvidenceAwareSuitabilityRun]) -> u64 {
    runs.iter()
        .filter(|run| run.suitability == NjoySuitabilityStatus::Rejected)
        .count() as u64
}

fn count_transition(
    runs: &[NjoyEvidenceAwareSuitabilityRun],
    from: NjoySuitabilityStatus,
    to: NjoySuitabilityStatus,
) -> u64 {
    runs.iter()
        .filter(|run| run.domain_aware_suitability == from && run.suitability == to)
        .count() as u64
}

fn sum_runs(
    runs: &[NjoyEvidenceAwareSuitabilityRun],
    field: impl Fn(&NjoyEvidenceAwareSuitabilityRun) -> u64,
) -> u64 {
    runs.iter().map(field).sum()
}

fn qualification(rejected_run_count: u64) -> NjoySuitabilityQualification {
    if rejected_run_count == 0 {
        NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
    } else {
        NjoySuitabilityQualification::TransportedPhotonKermaRejected
    }
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyEvidenceAwareSuitabilityError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    digest: &str,
) -> Result<(), NjoyEvidenceAwareSuitabilityError> {
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

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyEvidenceAwareSuitabilityError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| NjoyEvidenceAwareSuitabilityError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyEvidenceAwareSuitabilityError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyEvidenceAwareSuitabilityError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, NjoyEvidenceAwareSuitabilityError> {
    Err(NjoyEvidenceAwareSuitabilityError::InvalidReport(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoyEvidenceAwareSuitabilityError {
    #[error(transparent)]
    Domain(#[from] NjoyDomainAwareSuitabilityError),
    #[error(transparent)]
    Law7Residual(#[from] EndfMf6Law7ImplicitResidualError),
    #[error(transparent)]
    Law7Attribution(#[from] NjoyLaw7ImplicitResidualComparisonError),
    #[error(transparent)]
    CaptureBalance(#[from] EndfMf6CapturePhotonBalanceError),
    #[error(transparent)]
    CaptureComparison(#[from] NjoyCapturePhotonMomentComparisonError),
    #[error("processor comparisons do not bind the supplied independent reports")]
    IndependentReportBindingMismatch,
    #[error("evidence layers do not bind the same execution receipt")]
    ExecutionBindingMismatch,
    #[error("independent reports do not bind the same source selection and inventory")]
    SourceBindingMismatch,
    #[error("case identities do not match across evidence-aware inputs")]
    CaseBindingMismatch,
    #[error("H2 LAW=7 attribution evidence is not passing")]
    Law7AttributionRejected,
    #[error("N15 capture-balance evidence is not the required independently rejected gate")]
    CaptureGateMismatch,
    #[error("required nuclide {0} is absent from domain-aware evidence")]
    MissingRequiredNuclide(&'static str),
    #[error("domain and reaction-level run evidence do not match")]
    RunEvidenceMismatch,
    #[error("H2 attributed violations do not match the transport-domain partition")]
    DomainPartitionMismatch,
    #[error("invalid evidence-aware transported-photon suitability report: {0}")]
    InvalidReport(String),
    #[error("evidence-aware suitability report does not match regenerated evidence")]
    AssessmentMismatch,
    #[error("required evidence-aware artifact is not a regular file: {0}")]
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
    const LAW7_RESIDUAL: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-mf6-mt16-law7-implicit-residual.json"
    );
    const LAW7_ATTRIBUTION: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-vs-njoy2016-78-law7-implicit-residual.json"
    );
    const CAPTURE_BALANCE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-mf6-mt102-capture-photon-balance.json"
    );
    const CAPTURE_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-vs-njoy2016-78-mf6-capture-photon-moments.json"
    );
    const FROZEN_ASSESSMENT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/njoy2016-78-transported-photon-evidence-aware-suitability.json"
    );

    fn inputs() -> (
        NjoyDomainAwareSuitabilityReportDocument,
        EndfMf6Law7ImplicitResidualReportDocument,
        NjoyLaw7ImplicitResidualComparisonDocument,
        EndfMf6CapturePhotonBalanceReportDocument,
        NjoyCapturePhotonMomentComparisonDocument,
    ) {
        (
            NjoyDomainAwareSuitabilityReportDocument::from_bytes(DOMAIN).unwrap(),
            EndfMf6Law7ImplicitResidualReportDocument::from_bytes(LAW7_RESIDUAL).unwrap(),
            NjoyLaw7ImplicitResidualComparisonDocument::from_bytes(LAW7_ATTRIBUTION).unwrap(),
            EndfMf6CapturePhotonBalanceReportDocument::from_bytes(CAPTURE_BALANCE).unwrap(),
            NjoyCapturePhotonMomentComparisonDocument::from_bytes(CAPTURE_COMPARISON).unwrap(),
        )
    }

    fn assessment() -> NjoyEvidenceAwareSuitabilityReport {
        let (domain, law7, attribution, capture, capture_comparison) = inputs();
        NjoyEvidenceAwareSuitabilityReport::assess(
            &domain,
            &law7,
            &attribution,
            &capture,
            &capture_comparison,
        )
        .unwrap()
    }

    #[test]
    fn clears_only_h2_and_preserves_the_n15_independent_rejection() {
        let report = assessment();
        assert_eq!(report.rejected_run_count, 4);
        assert_eq!(report.reclassified_from_domain_run_count, 1);
        assert_eq!(report.independently_rejected_from_domain_run_count, 1);
        assert_eq!(report.domain_in_scope_kinematic_violation_count, 114);
        assert_eq!(
            report.approximation_attributed_in_domain_violation_count,
            12
        );
        assert_eq!(report.remaining_in_domain_kinematic_violation_count, 102);
        assert_eq!(report.independent_capture_balance_failed_sample_count, 33);

        let h2 = report.runs.iter().find(|run| run.nuclide == H2).unwrap();
        assert_eq!(h2.domain_aware_suitability, NjoySuitabilityStatus::Rejected);
        assert_eq!(h2.suitability, NjoySuitabilityStatus::CandidateUnreviewed);
        assert_eq!(h2.remaining_in_domain_diagnostic_violation_count, 0);
        let n15 = report.runs.iter().find(|run| run.nuclide == N15).unwrap();
        assert_eq!(
            n15.domain_aware_suitability,
            NjoySuitabilityStatus::CandidateUnreviewed
        );
        assert_eq!(n15.suitability, NjoySuitabilityStatus::Rejected);
        assert_eq!(
            n15.independent_gate,
            NjoyEvidenceAwareIndependentGate::Mf6CapturePhotonEnergyBalanceRejected
        );
    }

    #[test]
    fn verifies_the_frozen_v4_assessment_against_every_input_report() {
        let (domain, law7, attribution, capture, capture_comparison) = inputs();
        let frozen =
            NjoyEvidenceAwareSuitabilityReportDocument::from_bytes(FROZEN_ASSESSMENT).unwrap();
        assert_eq!(
            frozen.sha256,
            "68b22afd510d477eb997fd514a37bcca9c45730e7fab22fd7ad9186d37f2baa0"
        );
        frozen
            .verify_against_evidence(&domain, &law7, &attribution, &capture, &capture_comparison)
            .unwrap();
    }

    #[test]
    fn rejects_broadening_the_h2_disposition_to_another_nuclide() {
        let mut report = assessment();
        let carbon = report
            .runs
            .iter_mut()
            .find(|run| run.nuclide == "C13")
            .unwrap();
        carbon.kinematic_disposition =
            NjoyEvidenceAwareKinematicDisposition::Law7ImplicitResidualProcessorApproximation;
        carbon.approximation_attributed_full_evaluation_violation_count =
            carbon.full_evaluation_diagnostic_violation_count;
        carbon.approximation_attributed_in_domain_violation_count =
            carbon.in_domain_diagnostic_violation_count;
        carbon.remaining_in_domain_diagnostic_violation_count = 0;
        assert!(matches!(
            report.validate(),
            Err(NjoyEvidenceAwareSuitabilityError::InvalidReport(_))
        ));
    }
}
