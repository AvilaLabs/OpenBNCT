// SPDX-License-Identifier: MIT

//! Receipt-bound attribution of the deuterium LAW=7 implicit-residual
//! calculation to NJOY2016.78's printed one-particle approximation, energy-
//! balance remainder, final KERMA table, and kinematic findings.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    EndfMf6Law7ImplicitResidualError, EndfMf6Law7ImplicitResidualQualification,
    EndfMf6Law7ImplicitResidualReportDocument, NjoyExecutionArtifact, NjoyExecutionError,
    NjoyExecutionReceiptDocument, NjoyKinematicDirection,
};

pub const NJOY_LAW7_IMPLICIT_RESIDUAL_COMPARISON_SCHEMA: &str =
    "openbnct.njoy-law7-implicit-residual-comparison/0.1.0";
pub const DEFAULT_NJOY_LAW7_SOURCE_RELATIVE_TOLERANCE: f64 = 2.0e-3;
pub const DEFAULT_NJOY_LAW7_PRINT_RELATIVE_TOLERANCE: f64 = 2.0e-4;

const REPORT_ID_SUFFIX: &str = "njoy2016-78-law7-implicit-residual-comparison";
const BREAKUP_MT: u16 = 16;
const TOTAL_HEATING_MT: u16 = 301;
const NEUTRON_PARTICLE_ID: u32 = 1;
const SYNTHESIZED_RECOIL_PARTICLE_ID: u32 = 1002;
const MISSING_RESIDUAL_WARNING: &str = "one-particle recoil approx. used.";
const RECOIL_GENERATION_NOTICE: &str = "generating recoil with one-particle approx.";
const NO_EXPLICIT_PHOTON_NOTICE: &str = "no explicit file 6 photon production for mt 16";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyLaw7ImplicitResidualComparison {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyLaw7ImplicitResidualComparisonQualification,
    pub independent_residual_report: ContentReference,
    pub execution_receipt: ContentReference,
    pub source_relative_tolerance: f64,
    pub print_relative_tolerance: f64,
    pub nuclide: String,
    pub reaction_mt: u16,
    pub response_mt: u16,
    pub processor_report: NjoyExecutionArtifact,
    pub missing_residual_warning_count: u64,
    pub recoil_generation_notice_count: u64,
    pub no_explicit_photon_notice_count: u64,
    pub processor_neutron_particle_id: u32,
    pub processor_synthesized_recoil_particle_id: u32,
    pub processor_q_value_ev: f64,
    pub independent_sample_count: u64,
    pub processor_neutron_sample_count: u64,
    pub processor_recoil_sample_count: u64,
    pub shared_sample_count: u64,
    pub uncompared_independent_sample_count: u64,
    pub skipped_processor_sample_count: u64,
    pub negative_synthesized_recoil_sample_count: u64,
    pub positive_energy_balance_remainder_sample_count: u64,
    pub receipt_violation_count: u64,
    pub processor_violation_count: u64,
    pub attributed_violation_count: u64,
    pub samples: Vec<NjoyLaw7ImplicitResidualComparisonSample>,
    pub failed_sample_count: u64,
    pub maximum_source_neutron_mean_relative_difference: f64,
    pub maximum_energy_balance_identity_relative_difference: f64,
    pub maximum_violation_excess_relative_difference: f64,
    pub maximum_violation_local_kerma_relative_difference: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyLaw7ImplicitResidualComparisonQualification {
    ProcessorApproximationFullyAttributedUnreviewed,
    ProcessorAttributionRejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyLaw7ImplicitResidualComparisonSample {
    pub incident_energy_ev: f64,
    pub receipt_kinematic_violation: bool,
    pub processor_kinematic_direction: Option<NjoyKinematicDirection>,
    pub independent_mean_neutron_energy_ev: f64,
    pub processor_mean_neutron_energy_ev: f64,
    pub source_neutron_mean_relative_difference: f64,
    pub independent_neutron_yield: f64,
    pub processor_neutron_yield: f64,
    pub neutron_yield_relative_difference: f64,
    pub independent_cross_section_barns: f64,
    pub processor_cross_section_barns: f64,
    pub cross_section_relative_difference: f64,
    pub processor_neutron_heating_ev_barns: f64,
    pub processor_synthesized_recoil_mean_energy_ev: f64,
    pub processor_synthesized_recoil_yield: f64,
    pub processor_synthesized_recoil_heating_ev_barns: f64,
    pub recoil_heating_identity_relative_difference: f64,
    pub processor_energy_balance_remainder_ev_barns: f64,
    pub reconstructed_energy_balance_remainder_ev_barns: f64,
    pub energy_balance_identity_relative_difference: f64,
    pub independent_implicit_local_kerma_ev_barns: f64,
    pub processor_corrected_mt16_local_kerma_ev_barns: f64,
    pub violation_local_kerma_relative_difference: f64,
    pub processor_final_mt301_kerma_ev_barns: f64,
    pub processor_final_mt443_kerma_ev_barns: f64,
    pub processor_kinematic_minimum_ev_barns: f64,
    pub processor_kinematic_maximum_ev_barns: f64,
    pub mt443_to_kinematic_maximum_relative_difference: f64,
    pub processor_final_mt301_excess_ev_barns: f64,
    pub violation_excess_relative_difference: f64,
    pub status: NjoyLaw7ImplicitResidualComparisonStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyLaw7ImplicitResidualComparisonStatus {
    WithinTolerance,
    OutsideTolerance,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyLaw7ImplicitResidualComparisonDocument {
    pub comparison: NjoyLaw7ImplicitResidualComparison,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyLaw7ImplicitResidualComparisonResult {
    pub comparison: NjoyLaw7ImplicitResidualComparison,
    pub comparison_path: PathBuf,
    pub comparison_sha256: String,
}

impl NjoyLaw7ImplicitResidualComparison {
    pub fn compare(
        residual: &EndfMf6Law7ImplicitResidualReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        source_relative_tolerance: f64,
        print_relative_tolerance: f64,
    ) -> Result<Self, NjoyLaw7ImplicitResidualComparisonError> {
        residual.report.validate()?;
        validate_tolerances(source_relative_tolerance, print_relative_tolerance)?;
        execution.verify_execution_root(execution_root)?;
        if residual.report.case_id != execution.receipt.case_id {
            return Err(NjoyLaw7ImplicitResidualComparisonError::CaseBindingMismatch);
        }
        if residual.report.qualification
            != EndfMf6Law7ImplicitResidualQualification::ImplicitResidualEnergyCheckedUnreviewed
            || residual.report.failed_normalization_sample_count != 0
            || residual.report.failed_residual_energy_sample_count != 0
        {
            return Err(NjoyLaw7ImplicitResidualComparisonError::RejectedIndependentSource);
        }
        let run = execution
            .receipt
            .runs
            .iter()
            .find(|run| run.nuclide == residual.report.nuclide)
            .ok_or_else(|| {
                NjoyLaw7ImplicitResidualComparisonError::MissingProcessorRun(
                    residual.report.nuclide.clone(),
                )
            })?;
        if run.evaluated_source.sha256 != residual.report.source_evaluation_sha256 {
            return Err(NjoyLaw7ImplicitResidualComparisonError::SourceBindingMismatch);
        }
        let path = execution_root.join(&run.processor_report.path);
        let bytes = read_regular_file(&path)?;
        if bytes.len() as u64 != run.processor_report.size_bytes
            || sha256_bytes(&bytes) != run.processor_report.sha256
        {
            return Err(
                NjoyLaw7ImplicitResidualComparisonError::ProcessorReportChanged(
                    run.processor_report.path.clone(),
                ),
            );
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            NjoyLaw7ImplicitResidualComparisonError::NonUtf8ProcessorReport(path.clone())
        })?;
        let missing_residual_warning_count = text.matches(MISSING_RESIDUAL_WARNING).count() as u64;
        let recoil_generation_notice_count = text.matches(RECOIL_GENERATION_NOTICE).count() as u64;
        let no_explicit_photon_notice_count =
            text.matches(NO_EXPLICIT_PHOTON_NOTICE).count() as u64;

        let tables = parse_processor_tables(text, BREAKUP_MT)?;
        let neutron = unique_table(&tables, NEUTRON_PARTICLE_ID)?;
        let recoil = unique_table(&tables, SYNTHESIZED_RECOIL_PARTICLE_ID)?;
        if neutron.rows.len() != recoil.rows.len()
            || neutron
                .rows
                .iter()
                .zip(&recoil.rows)
                .any(|(left, right)| left.incident_energy_ev != right.incident_energy_ev)
        {
            return Err(NjoyLaw7ImplicitResidualComparisonError::ProcessorGridMismatch);
        }
        if relative_difference(neutron.q_value_ev, recoil.q_value_ev) > print_relative_tolerance
            || relative_difference(
                neutron.q_value_ev,
                residual.report.mass_difference_q_value_ev,
            ) > print_relative_tolerance
        {
            return Err(NjoyLaw7ImplicitResidualComparisonError::ProcessorQValueMismatch);
        }
        let final_rows = parse_final_kerma_table(text)?;
        let processor_violations = final_rows
            .iter()
            .filter_map(|row| {
                row.direction
                    .map(|direction| (row.incident_energy_ev, TOTAL_HEATING_MT, direction))
            })
            .collect::<Vec<_>>();
        let receipt_violations = run
            .diagnostic_violations
            .iter()
            .map(|violation| {
                (
                    violation.energy_ev,
                    violation.response_mt,
                    violation.direction,
                )
            })
            .collect::<Vec<_>>();
        if processor_violations != receipt_violations {
            return Err(NjoyLaw7ImplicitResidualComparisonError::ReceiptViolationMismatch);
        }

        let mut samples = Vec::new();
        for independent in &residual.report.samples {
            let Some(neutron_row) = exact_row(&neutron.rows, independent.incident_energy_ev)?
            else {
                continue;
            };
            let recoil_row = exact_row(&recoil.rows, independent.incident_energy_ev)?
                .ok_or(NjoyLaw7ImplicitResidualComparisonError::ProcessorGridMismatch)?;
            let final_row = exact_final_row(&final_rows, independent.incident_energy_ev)?.ok_or(
                NjoyLaw7ImplicitResidualComparisonError::MissingFinalKermaRow(
                    independent.incident_energy_ev,
                ),
            )?;
            let receipt_kinematic_violation = final_row.direction.is_some();
            if neutron_row.energy_balance_remainder_ev_barns.is_some()
                || neutron_row.heating_ev_barns != 0.0
                || recoil_row.energy_balance_remainder_ev_barns.is_none()
                || recoil_row.mean_energy_ev >= 0.0
                || recoil_row.yield_value != 1.0
                || recoil_row.cross_section_barns != neutron_row.cross_section_barns
            {
                return Err(NjoyLaw7ImplicitResidualComparisonError::UnexpectedProcessorSemantics);
            }
            let processor_energy_balance_remainder_ev_barns = recoil_row
                .energy_balance_remainder_ev_barns
                .expect("checked above");
            let source_neutron_mean_relative_difference = relative_difference(
                independent.normalized_mean_neutron_energy_ev,
                neutron_row.mean_energy_ev,
            );
            let neutron_yield_relative_difference =
                relative_difference(independent.neutron_yield, neutron_row.yield_value);
            let cross_section_relative_difference = relative_difference(
                independent.reaction_cross_section_barns,
                neutron_row.cross_section_barns,
            );
            let recoil_heating_identity_relative_difference = relative_difference(
                recoil_row.heating_ev_barns,
                recoil_row.mean_energy_ev * recoil_row.yield_value * recoil_row.cross_section_barns,
            );
            let reconstructed_energy_balance_remainder_ev_barns = (independent.incident_energy_ev
                + neutron.q_value_ev)
                * neutron_row.cross_section_barns
                - neutron_row.mean_energy_ev
                    * neutron_row.yield_value
                    * neutron_row.cross_section_barns
                - recoil_row.heating_ev_barns;
            let energy_balance_identity_relative_difference = relative_difference(
                processor_energy_balance_remainder_ev_barns,
                reconstructed_energy_balance_remainder_ev_barns,
            );
            let processor_corrected_mt16_local_kerma_ev_barns =
                recoil_row.heating_ev_barns + processor_energy_balance_remainder_ev_barns;
            let violation_local_kerma_relative_difference = relative_difference(
                independent.implicit_local_kerma_ev_barns,
                processor_corrected_mt16_local_kerma_ev_barns,
            );
            let mt443_to_kinematic_maximum_relative_difference = relative_difference(
                final_row.mt443_kerma_ev_barns,
                final_row.kinematic_maximum_ev_barns,
            );
            let processor_final_mt301_excess_ev_barns =
                final_row.mt301_kerma_ev_barns - final_row.kinematic_maximum_ev_barns;
            let violation_excess_relative_difference = relative_difference(
                processor_final_mt301_excess_ev_barns,
                processor_energy_balance_remainder_ev_barns,
            );
            let status = comparison_status(
                receipt_kinematic_violation,
                source_neutron_mean_relative_difference,
                neutron_yield_relative_difference,
                cross_section_relative_difference,
                recoil_heating_identity_relative_difference,
                energy_balance_identity_relative_difference,
                violation_local_kerma_relative_difference,
                mt443_to_kinematic_maximum_relative_difference,
                processor_final_mt301_excess_ev_barns,
                violation_excess_relative_difference,
                source_relative_tolerance,
                print_relative_tolerance,
            );
            samples.push(NjoyLaw7ImplicitResidualComparisonSample {
                incident_energy_ev: independent.incident_energy_ev,
                receipt_kinematic_violation,
                processor_kinematic_direction: final_row.direction,
                independent_mean_neutron_energy_ev: independent.normalized_mean_neutron_energy_ev,
                processor_mean_neutron_energy_ev: neutron_row.mean_energy_ev,
                source_neutron_mean_relative_difference,
                independent_neutron_yield: independent.neutron_yield,
                processor_neutron_yield: neutron_row.yield_value,
                neutron_yield_relative_difference,
                independent_cross_section_barns: independent.reaction_cross_section_barns,
                processor_cross_section_barns: neutron_row.cross_section_barns,
                cross_section_relative_difference,
                processor_neutron_heating_ev_barns: neutron_row.heating_ev_barns,
                processor_synthesized_recoil_mean_energy_ev: recoil_row.mean_energy_ev,
                processor_synthesized_recoil_yield: recoil_row.yield_value,
                processor_synthesized_recoil_heating_ev_barns: recoil_row.heating_ev_barns,
                recoil_heating_identity_relative_difference,
                processor_energy_balance_remainder_ev_barns,
                reconstructed_energy_balance_remainder_ev_barns,
                energy_balance_identity_relative_difference,
                independent_implicit_local_kerma_ev_barns: independent
                    .implicit_local_kerma_ev_barns,
                processor_corrected_mt16_local_kerma_ev_barns,
                violation_local_kerma_relative_difference,
                processor_final_mt301_kerma_ev_barns: final_row.mt301_kerma_ev_barns,
                processor_final_mt443_kerma_ev_barns: final_row.mt443_kerma_ev_barns,
                processor_kinematic_minimum_ev_barns: final_row.kinematic_minimum_ev_barns,
                processor_kinematic_maximum_ev_barns: final_row.kinematic_maximum_ev_barns,
                mt443_to_kinematic_maximum_relative_difference,
                processor_final_mt301_excess_ev_barns,
                violation_excess_relative_difference,
                status,
            });
        }
        if samples.is_empty() {
            return Err(NjoyLaw7ImplicitResidualComparisonError::NoSharedSourceNode);
        }

        let failed_sample_count = samples
            .iter()
            .filter(|sample| {
                sample.status == NjoyLaw7ImplicitResidualComparisonStatus::OutsideTolerance
            })
            .count() as u64;
        let attributed_violation_count = samples
            .iter()
            .filter(|sample| {
                sample.receipt_kinematic_violation
                    && sample.status == NjoyLaw7ImplicitResidualComparisonStatus::WithinTolerance
            })
            .count() as u64;
        let negative_synthesized_recoil_sample_count = recoil
            .rows
            .iter()
            .filter(|row| row.mean_energy_ev < 0.0)
            .count() as u64;
        let positive_energy_balance_remainder_sample_count = recoil
            .rows
            .iter()
            .filter(|row| {
                row.energy_balance_remainder_ev_barns
                    .is_some_and(|remainder| remainder > 0.0)
            })
            .count() as u64;
        let qualification = comparison_qualification(
            failed_sample_count,
            attributed_violation_count,
            receipt_violations.len() as u64,
            missing_residual_warning_count,
            recoil_generation_notice_count,
            no_explicit_photon_notice_count,
            negative_synthesized_recoil_sample_count,
            positive_energy_balance_remainder_sample_count,
            recoil.rows.len() as u64,
        );
        let comparison = Self {
            schema_version: NJOY_LAW7_IMPLICIT_RESIDUAL_COMPARISON_SCHEMA.into(),
            id: format!("{}.{}", residual.report.id, REPORT_ID_SUFFIX),
            case_id: residual.report.case_id.clone(),
            qualification,
            independent_residual_report: ContentReference {
                id: residual.report.id.clone(),
                sha256: residual.sha256.clone(),
            },
            execution_receipt: ContentReference {
                id: execution.receipt.id.clone(),
                sha256: execution.sha256.clone(),
            },
            source_relative_tolerance,
            print_relative_tolerance,
            nuclide: residual.report.nuclide.clone(),
            reaction_mt: BREAKUP_MT,
            response_mt: TOTAL_HEATING_MT,
            processor_report: run.processor_report.clone(),
            missing_residual_warning_count,
            recoil_generation_notice_count,
            no_explicit_photon_notice_count,
            processor_neutron_particle_id: neutron.particle_id,
            processor_synthesized_recoil_particle_id: recoil.particle_id,
            processor_q_value_ev: neutron.q_value_ev,
            independent_sample_count: residual.report.sample_count,
            processor_neutron_sample_count: neutron.rows.len() as u64,
            processor_recoil_sample_count: recoil.rows.len() as u64,
            shared_sample_count: samples.len() as u64,
            uncompared_independent_sample_count: residual.report.sample_count
                - samples.len() as u64,
            skipped_processor_sample_count: neutron.rows.len() as u64 - samples.len() as u64,
            negative_synthesized_recoil_sample_count,
            positive_energy_balance_remainder_sample_count,
            receipt_violation_count: receipt_violations.len() as u64,
            processor_violation_count: processor_violations.len() as u64,
            attributed_violation_count,
            maximum_source_neutron_mean_relative_difference: samples
                .iter()
                .map(|sample| sample.source_neutron_mean_relative_difference)
                .fold(0.0_f64, f64::max),
            maximum_energy_balance_identity_relative_difference: samples
                .iter()
                .map(|sample| sample.energy_balance_identity_relative_difference)
                .fold(0.0_f64, f64::max),
            maximum_violation_excess_relative_difference: samples
                .iter()
                .filter(|sample| sample.receipt_kinematic_violation)
                .map(|sample| sample.violation_excess_relative_difference)
                .fold(0.0_f64, f64::max),
            maximum_violation_local_kerma_relative_difference: samples
                .iter()
                .filter(|sample| sample.receipt_kinematic_violation)
                .map(|sample| sample.violation_local_kerma_relative_difference)
                .fold(0.0_f64, f64::max),
            samples,
            failed_sample_count,
        };
        comparison.validate()?;
        Ok(comparison)
    }

    pub fn validate(&self) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_LAW7_IMPLICIT_RESIDUAL_COMPARISON_SCHEMA,
        ) {
            return invalid_comparison(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier("nuclide", &self.nuclide)?;
        validate_reference(
            "independent_residual_report",
            &self.independent_residual_report,
        )?;
        validate_reference("execution_receipt", &self.execution_receipt)?;
        validate_sha256("processor_report.sha256", &self.processor_report.sha256)?;
        validate_tolerances(
            self.source_relative_tolerance,
            self.print_relative_tolerance,
        )?;
        if self.id
            != format!(
                "{}.{}",
                self.independent_residual_report.id, REPORT_ID_SUFFIX
            )
            || self.nuclide != "H2"
            || self.reaction_mt != BREAKUP_MT
            || self.response_mt != TOTAL_HEATING_MT
            || self.processor_neutron_particle_id != NEUTRON_PARTICLE_ID
            || self.processor_synthesized_recoil_particle_id != SYNTHESIZED_RECOIL_PARTICLE_ID
            || self.missing_residual_warning_count != 2
            || self.recoil_generation_notice_count != 2
            || self.no_explicit_photon_notice_count != 1
            || !self.processor_q_value_ev.is_finite()
            || self.processor_q_value_ev >= 0.0
            || self.processor_neutron_sample_count != self.processor_recoil_sample_count
            || self.processor_neutron_sample_count
                != self.shared_sample_count + self.skipped_processor_sample_count
            || self.independent_sample_count
                != self.shared_sample_count + self.uncompared_independent_sample_count
            || self.shared_sample_count != self.samples.len() as u64
            || self.negative_synthesized_recoil_sample_count != self.processor_recoil_sample_count
            || self.positive_energy_balance_remainder_sample_count
                != self.processor_recoil_sample_count
            || self.receipt_violation_count != self.processor_violation_count
        {
            return invalid_comparison("comparison identity or counts are inconsistent");
        }

        let mut previous_energy = None;
        for sample in &self.samples {
            validate_sample(self, sample, previous_energy)?;
            previous_energy = Some(sample.incident_energy_ev);
        }
        let failed_sample_count = self
            .samples
            .iter()
            .filter(|sample| {
                sample.status == NjoyLaw7ImplicitResidualComparisonStatus::OutsideTolerance
            })
            .count() as u64;
        let attributed_violation_count = self
            .samples
            .iter()
            .filter(|sample| {
                sample.receipt_kinematic_violation
                    && sample.status == NjoyLaw7ImplicitResidualComparisonStatus::WithinTolerance
            })
            .count() as u64;
        let shared_violation_count = self
            .samples
            .iter()
            .filter(|sample| sample.receipt_kinematic_violation)
            .count() as u64;
        let maxima = comparison_maxima(&self.samples);
        if self.failed_sample_count != failed_sample_count
            || self.attributed_violation_count != attributed_violation_count
            || self.receipt_violation_count != shared_violation_count
            || !approximately_equal(
                self.maximum_source_neutron_mean_relative_difference,
                maxima.0,
            )
            || !approximately_equal(
                self.maximum_energy_balance_identity_relative_difference,
                maxima.1,
            )
            || !approximately_equal(self.maximum_violation_excess_relative_difference, maxima.2)
            || !approximately_equal(
                self.maximum_violation_local_kerma_relative_difference,
                maxima.3,
            )
        {
            return invalid_comparison("comparison aggregates do not match samples");
        }
        let expected_qualification = comparison_qualification(
            failed_sample_count,
            attributed_violation_count,
            self.receipt_violation_count,
            self.missing_residual_warning_count,
            self.recoil_generation_notice_count,
            self.no_explicit_photon_notice_count,
            self.negative_synthesized_recoil_sample_count,
            self.positive_energy_balance_remainder_sample_count,
            self.processor_recoil_sample_count,
        );
        if self.qualification != expected_qualification {
            return invalid_comparison("qualification does not match comparison evidence");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyLaw7ImplicitResidualComparisonResult, NjoyLaw7ImplicitResidualComparisonError>
    {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyLaw7ImplicitResidualComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyLaw7ImplicitResidualComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyLaw7ImplicitResidualComparisonResult {
            comparison: self.clone(),
            comparison_path: path.to_path_buf(),
            comparison_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyLaw7ImplicitResidualComparisonDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyLaw7ImplicitResidualComparisonError> {
        let comparison: NjoyLaw7ImplicitResidualComparison = serde_json::from_slice(bytes)?;
        comparison.validate()?;
        Ok(Self {
            comparison,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyLaw7ImplicitResidualComparisonError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_evidence(
        &self,
        residual: &EndfMf6Law7ImplicitResidualReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
    ) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
        let observed = NjoyLaw7ImplicitResidualComparison::compare(
            residual,
            execution,
            execution_root,
            self.comparison.source_relative_tolerance,
            self.comparison.print_relative_tolerance,
        )?;
        if self.comparison != observed {
            return Err(NjoyLaw7ImplicitResidualComparisonError::ComparisonMismatch);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PrintedTable {
    particle_id: u32,
    q_value_ev: f64,
    rows: Vec<PrintedRow>,
}

#[derive(Debug, Clone, Copy)]
struct PrintedRow {
    incident_energy_ev: f64,
    mean_energy_ev: f64,
    yield_value: f64,
    cross_section_barns: f64,
    heating_ev_barns: f64,
    energy_balance_remainder_ev_barns: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct FinalKermaRow {
    incident_energy_ev: f64,
    mt301_kerma_ev_barns: f64,
    mt443_kerma_ev_barns: f64,
    kinematic_minimum_ev_barns: f64,
    kinematic_maximum_ev_barns: f64,
    direction: Option<NjoyKinematicDirection>,
}

fn parse_processor_tables(
    text: &str,
    reaction_mt: u16,
) -> Result<Vec<PrintedTable>, NjoyLaw7ImplicitResidualComparisonError> {
    let lines = text.lines().collect::<Vec<_>>();
    let marker = format!("file six heating for mt {reaction_mt:>2}, particle =");
    let mut tables = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let Some(position) = line.find(&marker) else {
            continue;
        };
        let tail = &line[position + marker.len()..];
        let particle_id = tail
            .split_whitespace()
            .next()
            .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable)?
            .parse::<u32>()
            .map_err(|_| NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable)?;
        let q_position = tail
            .find("q =")
            .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable)?;
        let q_value_ev = tail[q_position + 3..]
            .split_whitespace()
            .next()
            .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable)?
            .parse::<f64>()
            .map_err(|_| NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable)?;
        let mut rows = Vec::new();
        let mut cursor = line_index + 1;
        while cursor < lines.len() {
            let fields = lines[cursor].split_whitespace().collect::<Vec<_>>();
            if fields.len() == 5
                && let Ok(values) = fields
                    .iter()
                    .map(|field| field.parse::<f64>())
                    .collect::<Result<Vec<_>, _>>()
            {
                let mut energy_balance_remainder_ev_barns = None;
                if let Some(next) = lines.get(cursor + 1) {
                    let remainder_fields = next.split_whitespace().collect::<Vec<_>>();
                    if remainder_fields.len() == 2 && remainder_fields[0] == "ebal" {
                        energy_balance_remainder_ev_barns =
                            Some(remainder_fields[1].parse::<f64>().map_err(|_| {
                                NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable
                            })?);
                        cursor += 1;
                    }
                }
                rows.push(PrintedRow {
                    incident_energy_ev: values[0],
                    mean_energy_ev: values[1],
                    yield_value: values[2],
                    cross_section_barns: values[3],
                    heating_ev_barns: values[4],
                    energy_balance_remainder_ev_barns,
                });
                cursor += 1;
                continue;
            }
            if !rows.is_empty() {
                break;
            }
            cursor += 1;
        }
        validate_printed_rows(&rows)?;
        if tables
            .iter()
            .any(|table: &PrintedTable| table.particle_id == particle_id)
        {
            return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable);
        }
        tables.push(PrintedTable {
            particle_id,
            q_value_ev,
            rows,
        });
    }
    if tables.is_empty() {
        return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable);
    }
    Ok(tables)
}

fn unique_table(
    tables: &[PrintedTable],
    particle_id: u32,
) -> Result<&PrintedTable, NjoyLaw7ImplicitResidualComparisonError> {
    let mut matching = tables
        .iter()
        .filter(|table| table.particle_id == particle_id);
    let table = matching
        .next()
        .ok_or(NjoyLaw7ImplicitResidualComparisonError::MissingProcessorTable(particle_id))?;
    if matching.next().is_some() {
        return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable);
    }
    Ok(table)
}

fn validate_printed_rows(
    rows: &[PrintedRow],
) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
    if rows.is_empty()
        || rows.iter().any(|row| {
            !row.incident_energy_ev.is_finite()
                || row.incident_energy_ev <= 0.0
                || !row.mean_energy_ev.is_finite()
                || !row.yield_value.is_finite()
                || row.yield_value < 0.0
                || !row.cross_section_barns.is_finite()
                || row.cross_section_barns < 0.0
                || !row.heating_ev_barns.is_finite()
                || row
                    .energy_balance_remainder_ev_barns
                    .is_some_and(|remainder| !remainder.is_finite())
        })
        || rows
            .windows(2)
            .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedProcessorTable);
    }
    Ok(())
}

fn parse_final_kerma_table(
    text: &str,
) -> Result<Vec<FinalKermaRow>, NjoyLaw7ImplicitResidualComparisonError> {
    let lines = text.lines().collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line.trim() == "final kerma factors")
        .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable)?;
    let header = lines
        .get(start + 1)
        .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable)?
        .split_whitespace()
        .collect::<Vec<_>>();
    if header != ["e", "301", "443"] {
        return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable);
    }
    let mut rows = Vec::new();
    let mut pending_minimum = None;
    let mut cursor = start + 2;
    while cursor < lines.len() {
        let trimmed = lines[cursor].trim();
        if trimmed.starts_with("***") {
            break;
        }
        let fields = trimmed.split_whitespace().collect::<Vec<_>>();
        if fields.len() == 2 && fields[0] == "min" {
            pending_minimum =
                Some(fields[1].parse::<f64>().map_err(|_| {
                    NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable
                })?);
            cursor += 1;
            continue;
        }
        if fields.len() == 3
            && let Ok(values) = fields
                .iter()
                .map(|field| field.parse::<f64>())
                .collect::<Result<Vec<_>, _>>()
        {
            let minimum = pending_minimum
                .take()
                .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable)?;
            let max_index = next_nonempty_line(&lines, cursor + 1)
                .ok_or(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable)?;
            let max_fields = lines[max_index].split_whitespace().collect::<Vec<_>>();
            if max_fields.len() != 2 || max_fields[0] != "max" {
                return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable);
            }
            let maximum = max_fields[1]
                .parse::<f64>()
                .map_err(|_| NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable)?;
            let mut direction = None;
            let mut lookahead = max_index + 1;
            while lookahead < lines.len() {
                let marker = lines[lookahead].trim();
                if marker.starts_with("min ") || marker.starts_with("***") {
                    break;
                }
                let observed = match marker {
                    "low" => Some(NjoyKinematicDirection::Low),
                    "high" => Some(NjoyKinematicDirection::High),
                    _ => None,
                };
                if observed.is_some() {
                    if direction.is_some() {
                        return Err(
                            NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable,
                        );
                    }
                    direction = observed;
                }
                lookahead += 1;
            }
            rows.push(FinalKermaRow {
                incident_energy_ev: values[0],
                mt301_kerma_ev_barns: values[1],
                mt443_kerma_ev_barns: values[2],
                kinematic_minimum_ev_barns: minimum,
                kinematic_maximum_ev_barns: maximum,
                direction,
            });
            cursor = lookahead;
            continue;
        }
        cursor += 1;
    }
    if rows.is_empty()
        || rows.iter().any(|row| {
            !row.incident_energy_ev.is_finite()
                || row.incident_energy_ev <= 0.0
                || !row.mt301_kerma_ev_barns.is_finite()
                || !row.mt443_kerma_ev_barns.is_finite()
                || !row.kinematic_minimum_ev_barns.is_finite()
                || !row.kinematic_maximum_ev_barns.is_finite()
        })
        || rows
            .windows(2)
            .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(NjoyLaw7ImplicitResidualComparisonError::UnparsedFinalKermaTable);
    }
    Ok(rows)
}

fn next_nonempty_line(lines: &[&str], start: usize) -> Option<usize> {
    (start..lines.len()).find(|index| !lines[*index].trim().is_empty())
}

fn exact_row(
    rows: &[PrintedRow],
    energy_ev: f64,
) -> Result<Option<&PrintedRow>, NjoyLaw7ImplicitResidualComparisonError> {
    let mut matching = rows
        .iter()
        .filter(|row| row.incident_energy_ev == energy_ev);
    let row = matching.next();
    if matching.next().is_some() {
        return Err(NjoyLaw7ImplicitResidualComparisonError::AmbiguousSourceNode(energy_ev));
    }
    Ok(row)
}

fn exact_final_row(
    rows: &[FinalKermaRow],
    energy_ev: f64,
) -> Result<Option<&FinalKermaRow>, NjoyLaw7ImplicitResidualComparisonError> {
    let mut matching = rows
        .iter()
        .filter(|row| row.incident_energy_ev == energy_ev);
    let row = matching.next();
    if matching.next().is_some() {
        return Err(NjoyLaw7ImplicitResidualComparisonError::AmbiguousSourceNode(energy_ev));
    }
    Ok(row)
}

#[allow(clippy::too_many_arguments)]
fn comparison_status(
    violation: bool,
    source_neutron_mean_relative_difference: f64,
    neutron_yield_relative_difference: f64,
    cross_section_relative_difference: f64,
    recoil_heating_identity_relative_difference: f64,
    energy_balance_identity_relative_difference: f64,
    violation_local_kerma_relative_difference: f64,
    mt443_to_kinematic_maximum_relative_difference: f64,
    final_excess_ev_barns: f64,
    violation_excess_relative_difference: f64,
    source_relative_tolerance: f64,
    print_relative_tolerance: f64,
) -> NjoyLaw7ImplicitResidualComparisonStatus {
    let common_passes = source_neutron_mean_relative_difference <= source_relative_tolerance
        && neutron_yield_relative_difference <= print_relative_tolerance
        && cross_section_relative_difference <= print_relative_tolerance
        && recoil_heating_identity_relative_difference <= print_relative_tolerance
        && energy_balance_identity_relative_difference <= print_relative_tolerance;
    let violation_passes = !violation
        || (violation_local_kerma_relative_difference <= source_relative_tolerance
            && mt443_to_kinematic_maximum_relative_difference <= print_relative_tolerance
            && final_excess_ev_barns > 0.0
            && violation_excess_relative_difference <= print_relative_tolerance);
    if common_passes && violation_passes {
        NjoyLaw7ImplicitResidualComparisonStatus::WithinTolerance
    } else {
        NjoyLaw7ImplicitResidualComparisonStatus::OutsideTolerance
    }
}

fn validate_sample(
    comparison: &NjoyLaw7ImplicitResidualComparison,
    sample: &NjoyLaw7ImplicitResidualComparisonSample,
    previous_energy: Option<f64>,
) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
    let values = [
        sample.incident_energy_ev,
        sample.independent_mean_neutron_energy_ev,
        sample.processor_mean_neutron_energy_ev,
        sample.source_neutron_mean_relative_difference,
        sample.independent_neutron_yield,
        sample.processor_neutron_yield,
        sample.neutron_yield_relative_difference,
        sample.independent_cross_section_barns,
        sample.processor_cross_section_barns,
        sample.cross_section_relative_difference,
        sample.processor_neutron_heating_ev_barns,
        sample.processor_synthesized_recoil_mean_energy_ev,
        sample.processor_synthesized_recoil_yield,
        sample.processor_synthesized_recoil_heating_ev_barns,
        sample.recoil_heating_identity_relative_difference,
        sample.processor_energy_balance_remainder_ev_barns,
        sample.reconstructed_energy_balance_remainder_ev_barns,
        sample.energy_balance_identity_relative_difference,
        sample.independent_implicit_local_kerma_ev_barns,
        sample.processor_corrected_mt16_local_kerma_ev_barns,
        sample.violation_local_kerma_relative_difference,
        sample.processor_final_mt301_kerma_ev_barns,
        sample.processor_final_mt443_kerma_ev_barns,
        sample.processor_kinematic_minimum_ev_barns,
        sample.processor_kinematic_maximum_ev_barns,
        sample.mt443_to_kinematic_maximum_relative_difference,
        sample.processor_final_mt301_excess_ev_barns,
        sample.violation_excess_relative_difference,
    ];
    if values.iter().any(|value| !value.is_finite())
        || sample.incident_energy_ev <= 0.0
        || sample.receipt_kinematic_violation != sample.processor_kinematic_direction.is_some()
        || sample
            .processor_kinematic_direction
            .is_some_and(|direction| direction != NjoyKinematicDirection::High)
        || sample.independent_mean_neutron_energy_ev < 0.0
        || sample.processor_mean_neutron_energy_ev < 0.0
        || sample.source_neutron_mean_relative_difference < 0.0
        || sample.independent_neutron_yield < 0.0
        || sample.processor_neutron_yield < 0.0
        || sample.neutron_yield_relative_difference < 0.0
        || sample.independent_cross_section_barns <= 0.0
        || sample.processor_cross_section_barns <= 0.0
        || sample.cross_section_relative_difference < 0.0
        || sample.processor_neutron_heating_ev_barns != 0.0
        || sample.processor_synthesized_recoil_mean_energy_ev >= 0.0
        || sample.processor_synthesized_recoil_yield != 1.0
        || sample.processor_synthesized_recoil_heating_ev_barns >= 0.0
        || sample.recoil_heating_identity_relative_difference < 0.0
        || sample.processor_energy_balance_remainder_ev_barns <= 0.0
        || sample.reconstructed_energy_balance_remainder_ev_barns <= 0.0
        || sample.energy_balance_identity_relative_difference < 0.0
        || sample.independent_implicit_local_kerma_ev_barns <= 0.0
        || sample.processor_corrected_mt16_local_kerma_ev_barns <= 0.0
        || sample.violation_local_kerma_relative_difference < 0.0
        || sample.mt443_to_kinematic_maximum_relative_difference < 0.0
        || sample.violation_excess_relative_difference < 0.0
        || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
    {
        return invalid_comparison("invalid or unordered comparison sample");
    }
    let expected_recoil_heating = sample.processor_synthesized_recoil_mean_energy_ev
        * sample.processor_synthesized_recoil_yield
        * sample.processor_cross_section_barns;
    let expected_remainder = (sample.incident_energy_ev + comparison.processor_q_value_ev)
        * sample.processor_cross_section_barns
        - sample.processor_mean_neutron_energy_ev
            * sample.processor_neutron_yield
            * sample.processor_cross_section_barns
        - sample.processor_synthesized_recoil_heating_ev_barns;
    let expected_corrected = sample.processor_synthesized_recoil_heating_ev_barns
        + sample.processor_energy_balance_remainder_ev_barns;
    let expected_excess =
        sample.processor_final_mt301_kerma_ev_barns - sample.processor_kinematic_maximum_ev_barns;
    if !approximately_equal(
        sample.source_neutron_mean_relative_difference,
        relative_difference(
            sample.independent_mean_neutron_energy_ev,
            sample.processor_mean_neutron_energy_ev,
        ),
    ) || !approximately_equal(
        sample.neutron_yield_relative_difference,
        relative_difference(
            sample.independent_neutron_yield,
            sample.processor_neutron_yield,
        ),
    ) || !approximately_equal(
        sample.cross_section_relative_difference,
        relative_difference(
            sample.independent_cross_section_barns,
            sample.processor_cross_section_barns,
        ),
    ) || !approximately_equal(
        sample.recoil_heating_identity_relative_difference,
        relative_difference(
            sample.processor_synthesized_recoil_heating_ev_barns,
            expected_recoil_heating,
        ),
    ) || !approximately_equal(
        sample.reconstructed_energy_balance_remainder_ev_barns,
        expected_remainder,
    ) || !approximately_equal(
        sample.energy_balance_identity_relative_difference,
        relative_difference(
            sample.processor_energy_balance_remainder_ev_barns,
            expected_remainder,
        ),
    ) || !approximately_equal(
        sample.processor_corrected_mt16_local_kerma_ev_barns,
        expected_corrected,
    ) || !approximately_equal(
        sample.violation_local_kerma_relative_difference,
        relative_difference(
            sample.independent_implicit_local_kerma_ev_barns,
            expected_corrected,
        ),
    ) || !approximately_equal(
        sample.mt443_to_kinematic_maximum_relative_difference,
        relative_difference(
            sample.processor_final_mt443_kerma_ev_barns,
            sample.processor_kinematic_maximum_ev_barns,
        ),
    ) || !approximately_equal(
        sample.processor_final_mt301_excess_ev_barns,
        expected_excess,
    ) || !approximately_equal(
        sample.violation_excess_relative_difference,
        relative_difference(
            expected_excess,
            sample.processor_energy_balance_remainder_ev_barns,
        ),
    ) {
        return invalid_comparison("comparison sample derived values do not close");
    }
    let expected_status = comparison_status(
        sample.receipt_kinematic_violation,
        sample.source_neutron_mean_relative_difference,
        sample.neutron_yield_relative_difference,
        sample.cross_section_relative_difference,
        sample.recoil_heating_identity_relative_difference,
        sample.energy_balance_identity_relative_difference,
        sample.violation_local_kerma_relative_difference,
        sample.mt443_to_kinematic_maximum_relative_difference,
        sample.processor_final_mt301_excess_ev_barns,
        sample.violation_excess_relative_difference,
        comparison.source_relative_tolerance,
        comparison.print_relative_tolerance,
    );
    if sample.status != expected_status {
        return invalid_comparison("comparison sample status is inconsistent");
    }
    Ok(())
}

fn comparison_maxima(samples: &[NjoyLaw7ImplicitResidualComparisonSample]) -> (f64, f64, f64, f64) {
    (
        samples
            .iter()
            .map(|sample| sample.source_neutron_mean_relative_difference)
            .fold(0.0_f64, f64::max),
        samples
            .iter()
            .map(|sample| sample.energy_balance_identity_relative_difference)
            .fold(0.0_f64, f64::max),
        samples
            .iter()
            .filter(|sample| sample.receipt_kinematic_violation)
            .map(|sample| sample.violation_excess_relative_difference)
            .fold(0.0_f64, f64::max),
        samples
            .iter()
            .filter(|sample| sample.receipt_kinematic_violation)
            .map(|sample| sample.violation_local_kerma_relative_difference)
            .fold(0.0_f64, f64::max),
    )
}

#[allow(clippy::too_many_arguments)]
fn comparison_qualification(
    failed_sample_count: u64,
    attributed_violation_count: u64,
    receipt_violation_count: u64,
    missing_residual_warning_count: u64,
    recoil_generation_notice_count: u64,
    no_explicit_photon_notice_count: u64,
    negative_recoil_count: u64,
    positive_remainder_count: u64,
    recoil_sample_count: u64,
) -> NjoyLaw7ImplicitResidualComparisonQualification {
    if failed_sample_count == 0
        && receipt_violation_count > 0
        && attributed_violation_count == receipt_violation_count
        && missing_residual_warning_count == 2
        && recoil_generation_notice_count == 2
        && no_explicit_photon_notice_count == 1
        && negative_recoil_count == recoil_sample_count
        && positive_remainder_count == recoil_sample_count
    {
        NjoyLaw7ImplicitResidualComparisonQualification::
            ProcessorApproximationFullyAttributedUnreviewed
    } else {
        NjoyLaw7ImplicitResidualComparisonQualification::ProcessorAttributionRejected
    }
}

fn relative_difference(left: f64, right: f64) -> f64 {
    if left == right {
        0.0
    } else {
        (left - right).abs() / left.abs().max(right.abs()).max(f64::MIN_POSITIVE)
    }
}

fn validate_tolerances(
    source_relative_tolerance: f64,
    print_relative_tolerance: f64,
) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
    if !source_relative_tolerance.is_finite()
        || source_relative_tolerance <= 0.0
        || source_relative_tolerance > 5.0e-3
    {
        return invalid_comparison("source relative tolerance must be in (0, 5e-3]");
    }
    if !print_relative_tolerance.is_finite()
        || print_relative_tolerance <= 0.0
        || print_relative_tolerance > 1.0e-3
    {
        return invalid_comparison("print relative tolerance must be in (0, 1e-3]");
    }
    Ok(())
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
    if value.trim().is_empty() {
        invalid_comparison(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    digest: &str,
) -> Result<(), NjoyLaw7ImplicitResidualComparisonError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_comparison(format!("{label} is not a lowercase SHA-256 digest"))
    }
}

fn approximately_equal(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= 1.0e-12 * scale
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyLaw7ImplicitResidualComparisonError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        NjoyLaw7ImplicitResidualComparisonError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyLaw7ImplicitResidualComparisonError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyLaw7ImplicitResidualComparisonError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_comparison<T>(
    message: impl Into<String>,
) -> Result<T, NjoyLaw7ImplicitResidualComparisonError> {
    Err(NjoyLaw7ImplicitResidualComparisonError::InvalidComparison(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoyLaw7ImplicitResidualComparisonError {
    #[error(transparent)]
    Residual(#[from] EndfMf6Law7ImplicitResidualError),
    #[error(transparent)]
    Execution(#[from] NjoyExecutionError),
    #[error("independent LAW=7 report and execution receipt have different cases")]
    CaseBindingMismatch,
    #[error("independent LAW=7 source report is rejected")]
    RejectedIndependentSource,
    #[error("execution receipt has no processor run for {0}")]
    MissingProcessorRun(String),
    #[error("execution source does not match the independent LAW=7 source")]
    SourceBindingMismatch,
    #[error("processor report changed after execution verification: {0}")]
    ProcessorReportChanged(String),
    #[error("processor report is not UTF-8 text: {0}")]
    NonUtf8ProcessorReport(PathBuf),
    #[error("NJOY MF=6/MT=16 diagnostic tables could not be parsed uniquely")]
    UnparsedProcessorTable,
    #[error("NJOY output has no unique MF=6/MT=16 table for particle {0}")]
    MissingProcessorTable(u32),
    #[error("NJOY MF=6/MT=16 neutron and synthesized-recoil grids differ")]
    ProcessorGridMismatch,
    #[error("NJOY MF=6/MT=16 printed Q value does not match the source")]
    ProcessorQValueMismatch,
    #[error("NJOY final KERMA table could not be parsed uniquely")]
    UnparsedFinalKermaTable,
    #[error("receipt violations do not exactly match the NJOY final KERMA table")]
    ReceiptViolationMismatch,
    #[error("processor and independent reports share no exact source node")]
    NoSharedSourceNode,
    #[error("processor grid ambiguously matches source energy {0}")]
    AmbiguousSourceNode(f64),
    #[error("NJOY final KERMA table has no row for shared source energy {0}")]
    MissingFinalKermaRow(f64),
    #[error("processor MF=6/MT=16 table has unexpected semantics")]
    UnexpectedProcessorSemantics,
    #[error("invalid NJOY LAW=7 implicit-residual comparison: {0}")]
    InvalidComparison(String),
    #[error("stored NJOY LAW=7 comparison does not match regenerated evidence")]
    ComparisonMismatch,
    #[error("required comparison artifact is not a regular file: {0}")]
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

    const JEFF40_H2_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-vs-njoy2016-78-law7-implicit-residual.json"
    );

    #[test]
    fn parses_law7_remainder_and_final_kerma_tables() {
        let text = " file six heating for mt 16, particle =     1     q =  -2.2246E+06\n\
                    e ebar yield xsec heating\n\
                    9.0000E+06 2.2268E+06 2.0000E+00 1.3720E-01 0.0000E+00\n\
                    \n\
                    file six heating for mt 16, particle =  1002 q = -2.2246E+06\n\
                    e ebar yield xsec heating\n\
                    9.0000E+06 -4.4247E+05 1.0000E+00 1.3720E-01 -6.0707E+04\n\
                    ebal 3.7926E+05\n\
                    \n\
                    final kerma factors\n\
                    e 301 443\n\
                    min 2.9550E+06\n\
                    9.0000E+06 3.3343E+06 2.9550E+06\n\
                    max 2.9550E+06\n\
                    high\n\
                    *****************************************************************************\n";
        let tables = parse_processor_tables(text, BREAKUP_MT).unwrap();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].particle_id, NEUTRON_PARTICLE_ID);
        assert_eq!(tables[1].particle_id, SYNTHESIZED_RECOIL_PARTICLE_ID);
        assert_eq!(
            tables[1].rows[0].energy_balance_remainder_ev_barns,
            Some(379_260.0)
        );
        let final_rows = parse_final_kerma_table(text).unwrap();
        assert_eq!(final_rows.len(), 1);
        assert_eq!(final_rows[0].direction, Some(NjoyKinematicDirection::High));
        assert_eq!(final_rows[0].mt301_kerma_ev_barns, 3_334_300.0);
        assert_eq!(final_rows[0].kinematic_maximum_ev_barns, 2_955_000.0);
    }

    #[test]
    fn validates_frozen_h2_law7_processor_attribution() {
        let document =
            NjoyLaw7ImplicitResidualComparisonDocument::from_bytes(JEFF40_H2_COMPARISON).unwrap();
        assert_eq!(
            document.sha256,
            "64b3985ed5fc3d57c7a41c55b58e13f8bba069403c72bafe50235a13e0ae5687"
        );
        assert_eq!(
            document.comparison.qualification,
            NjoyLaw7ImplicitResidualComparisonQualification::
                ProcessorApproximationFullyAttributedUnreviewed
        );
        assert_eq!(document.comparison.processor_neutron_sample_count, 23);
        assert_eq!(document.comparison.shared_sample_count, 22);
        assert_eq!(document.comparison.receipt_violation_count, 15);
        assert_eq!(document.comparison.processor_violation_count, 15);
        assert_eq!(document.comparison.attributed_violation_count, 15);
        assert_eq!(document.comparison.failed_sample_count, 0);
        assert_eq!(
            document
                .comparison
                .maximum_source_neutron_mean_relative_difference,
            0.0018639429185867214
        );
        assert_eq!(
            document
                .comparison
                .maximum_violation_excess_relative_difference,
            0.00010545742156604271
        );
        let nine_mev = document
            .comparison
            .samples
            .iter()
            .find(|sample| sample.incident_energy_ev == 9.0e6)
            .unwrap();
        assert!(nine_mev.receipt_kinematic_violation);
        assert_eq!(
            nine_mev.processor_energy_balance_remainder_ev_barns,
            379_260.0
        );
        assert_eq!(nine_mev.processor_final_mt301_excess_ev_barns, 379_300.0);
    }

    #[test]
    fn rejects_tampered_law7_energy_balance_remainder() {
        let mut comparison =
            NjoyLaw7ImplicitResidualComparisonDocument::from_bytes(JEFF40_H2_COMPARISON)
                .unwrap()
                .comparison;
        comparison.samples[0].processor_energy_balance_remainder_ev_barns += 1.0;
        assert!(matches!(
            comparison.validate(),
            Err(NjoyLaw7ImplicitResidualComparisonError::InvalidComparison(
                _
            ))
        ));
    }
}
