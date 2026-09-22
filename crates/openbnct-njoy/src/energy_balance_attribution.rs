// SPDX-License-Identifier: MIT

//! Receipt-bound attribution of NJOY kinematic diagnostics to the printed
//! File 6 energy-balance remainders that are included in MT=301.
//!
//! This is an internal processor-accounting check, not an independent
//! physical validation of the evaluated reaction data. A successful report
//! therefore retains every attributed finding for independent review.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    NjoyDomainAwareSuitabilityError, NjoyDomainAwareSuitabilityReportDocument,
    NjoyExecutionArtifact, NjoyExecutionError, NjoyExecutionReceiptDocument,
    NjoyKinematicDirection,
};

pub const NJOY_ENERGY_BALANCE_ATTRIBUTION_SCHEMA: &str =
    "openbnct.njoy-energy-balance-attribution/0.1.0";
pub const DEFAULT_NJOY_ENERGY_BALANCE_PRINT_RELATIVE_TOLERANCE: f64 = 2.0e-3;

const REPORT_ID_SUFFIX: &str = "njoy-energy-balance-attribution-v1";
const TOTAL_HEATING_MT: u16 = 301;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyEnergyBalanceAttribution {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyEnergyBalanceAttributionQualification,
    pub evidence_scope: NjoyEnergyBalanceEvidenceScope,
    pub finding_disposition: NjoyEnergyBalanceFindingDisposition,
    pub domain_aware_suitability_report: ContentReference,
    pub execution_receipt: ContentReference,
    pub print_relative_tolerance: f64,
    pub nuclide: String,
    pub assessment_energy_range_ev: [f64; 2],
    pub response_mt: u16,
    pub processor_report: NjoyExecutionArtifact,
    pub full_evaluation_violation_count: u64,
    pub in_domain_violation_count: u64,
    pub out_of_domain_violation_count: u64,
    pub processor_final_table_violation_count: u64,
    pub printed_remainder_table_count: u64,
    pub contributing_reaction_mts: Vec<u16>,
    pub attributed_in_domain_violation_count: u64,
    pub failed_sample_count: u64,
    pub physical_validation_required_count: u64,
    pub waived_violation_count: u64,
    pub maximum_remainder_excess_relative_difference: f64,
    pub maximum_mt443_kinematic_maximum_relative_difference: f64,
    pub samples: Vec<NjoyEnergyBalanceAttributionSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEnergyBalanceAttributionQualification {
    ProcessorAccountingMechanismAttributedPhysicalValidationRequired,
    ProcessorAccountingAttributionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEnergyBalanceEvidenceScope {
    ProcessorInternalPrintAccountingOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEnergyBalanceFindingDisposition {
    RetainedForIndependentPhysicalValidation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyEnergyBalanceAttributionSample {
    pub incident_energy_ev: f64,
    pub response_mt: u16,
    pub processor_kinematic_direction: NjoyKinematicDirection,
    pub processor_final_mt301_kerma_ev_barns: f64,
    pub processor_final_mt443_kerma_ev_barns: f64,
    pub processor_kinematic_maximum_ev_barns: f64,
    pub processor_final_mt301_excess_ev_barns: f64,
    pub printed_energy_balance_remainder_sum_ev_barns: f64,
    pub remainder_excess_relative_difference: f64,
    pub mt443_kinematic_maximum_relative_difference: f64,
    pub contributions: Vec<NjoyEnergyBalanceContribution>,
    pub status: NjoyEnergyBalanceAttributionStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyEnergyBalanceContribution {
    pub reaction_mt: u16,
    pub particle_id: u32,
    pub printed_remainder_ev_barns: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyEnergyBalanceAttributionStatus {
    AttributedWithinPrintTolerance,
    NotAttributed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyEnergyBalanceAttributionDocument {
    pub attribution: NjoyEnergyBalanceAttribution,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyEnergyBalanceAttributionResult {
    pub attribution: NjoyEnergyBalanceAttribution,
    pub attribution_path: PathBuf,
    pub attribution_sha256: String,
}

impl NjoyEnergyBalanceAttribution {
    pub fn attribute(
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        nuclide: &str,
        print_relative_tolerance: f64,
    ) -> Result<Self, NjoyEnergyBalanceAttributionError> {
        domain.report.validate()?;
        validate_tolerance(print_relative_tolerance)?;
        execution.verify_execution_root(execution_root)?;
        validate_identifier("nuclide", nuclide)?;

        let execution_reference = ContentReference {
            id: execution.receipt.id.clone(),
            sha256: execution.sha256.clone(),
        };
        if domain.report.execution_receipt != execution_reference {
            return Err(NjoyEnergyBalanceAttributionError::ExecutionBindingMismatch);
        }
        if domain.report.case_id != execution.receipt.case_id {
            return Err(NjoyEnergyBalanceAttributionError::CaseBindingMismatch);
        }

        let domain_run = domain
            .report
            .runs
            .iter()
            .find(|run| run.nuclide == nuclide)
            .ok_or_else(|| NjoyEnergyBalanceAttributionError::MissingDomainRun(nuclide.into()))?;
        let execution_run = execution
            .receipt
            .runs
            .iter()
            .find(|run| run.nuclide == nuclide)
            .ok_or_else(|| {
                NjoyEnergyBalanceAttributionError::MissingExecutionRun(nuclide.into())
            })?;
        if domain_run.processor_report != execution_run.processor_report
            || domain_run.full_evaluation_diagnostic_violation_count
                != execution_run.diagnostic_violation_count
        {
            return Err(NjoyEnergyBalanceAttributionError::RunEvidenceMismatch(
                nuclide.into(),
            ));
        }

        let in_domain_violations = execution_run
            .diagnostic_violations
            .iter()
            .filter(|violation| {
                contains_energy(
                    domain.report.assessment_energy_range_ev,
                    violation.energy_ev,
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        let out_of_domain_violations = execution_run
            .diagnostic_violations
            .iter()
            .filter(|violation| {
                !contains_energy(
                    domain.report.assessment_energy_range_ev,
                    violation.energy_ev,
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        if domain_run.in_domain_diagnostic_violation_count != in_domain_violations.len() as u64
            || domain_run.out_of_domain_diagnostic_violations != out_of_domain_violations
        {
            return Err(NjoyEnergyBalanceAttributionError::DomainPartitionMismatch(
                nuclide.into(),
            ));
        }
        if in_domain_violations.is_empty() {
            return Err(NjoyEnergyBalanceAttributionError::NoInDomainViolation(
                nuclide.into(),
            ));
        }
        if in_domain_violations.iter().any(|violation| {
            violation.response_mt != TOTAL_HEATING_MT
                || violation.direction != NjoyKinematicDirection::High
        }) {
            return Err(NjoyEnergyBalanceAttributionError::UnsupportedViolation(
                nuclide.into(),
            ));
        }

        let path = execution_root.join(&execution_run.processor_report.path);
        let bytes = read_regular_file(&path)?;
        if bytes.len() as u64 != execution_run.processor_report.size_bytes
            || sha256_bytes(&bytes) != execution_run.processor_report.sha256
        {
            return Err(NjoyEnergyBalanceAttributionError::ProcessorReportChanged(
                execution_run.processor_report.path.clone(),
            ));
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| NjoyEnergyBalanceAttributionError::NonUtf8ProcessorReport(path.clone()))?;
        let (remainders, printed_remainder_table_count) = parse_file6_remainders(text)?;
        let final_rows = parse_final_kerma_table(text)?;
        let processor_violations = final_rows
            .iter()
            .filter_map(|row| {
                row.direction
                    .map(|direction| crate::NjoyKinematicViolation {
                        energy_ev: row.incident_energy_ev,
                        response_mt: TOTAL_HEATING_MT,
                        direction,
                    })
            })
            .collect::<Vec<_>>();
        if processor_violations != execution_run.diagnostic_violations {
            return Err(NjoyEnergyBalanceAttributionError::ReceiptViolationMismatch);
        }

        let mut samples = Vec::with_capacity(in_domain_violations.len());
        for violation in &in_domain_violations {
            let final_row = exact_final_row(&final_rows, violation.energy_ev)?.ok_or(
                NjoyEnergyBalanceAttributionError::MissingFinalKermaRow(violation.energy_ev),
            )?;
            let mut matching = remainders
                .iter()
                .filter(|remainder| remainder.incident_energy_ev == violation.energy_ev)
                .map(|remainder| NjoyEnergyBalanceContribution {
                    reaction_mt: remainder.reaction_mt,
                    particle_id: remainder.particle_id,
                    printed_remainder_ev_barns: remainder.remainder_ev_barns,
                })
                .collect::<Vec<_>>();
            matching
                .sort_by_key(|contribution| (contribution.reaction_mt, contribution.particle_id));
            if matching.windows(2).any(|pair| {
                pair[0].reaction_mt == pair[1].reaction_mt
                    && pair[0].particle_id == pair[1].particle_id
            }) {
                return Err(NjoyEnergyBalanceAttributionError::AmbiguousRemainder(
                    violation.energy_ev,
                ));
            }
            let printed_energy_balance_remainder_sum_ev_barns = matching
                .iter()
                .map(|contribution| contribution.printed_remainder_ev_barns)
                .sum::<f64>();
            let processor_final_mt301_excess_ev_barns =
                final_row.mt301_kerma_ev_barns - final_row.kinematic_maximum_ev_barns;
            let remainder_excess_relative_difference = relative_difference(
                printed_energy_balance_remainder_sum_ev_barns,
                processor_final_mt301_excess_ev_barns,
            );
            let mt443_kinematic_maximum_relative_difference = relative_difference(
                final_row.mt443_kerma_ev_barns,
                final_row.kinematic_maximum_ev_barns,
            );
            let status = attribution_status(
                final_row.direction,
                processor_final_mt301_excess_ev_barns,
                matching.is_empty(),
                remainder_excess_relative_difference,
                mt443_kinematic_maximum_relative_difference,
                print_relative_tolerance,
            );
            samples.push(NjoyEnergyBalanceAttributionSample {
                incident_energy_ev: violation.energy_ev,
                response_mt: violation.response_mt,
                processor_kinematic_direction: violation.direction,
                processor_final_mt301_kerma_ev_barns: final_row.mt301_kerma_ev_barns,
                processor_final_mt443_kerma_ev_barns: final_row.mt443_kerma_ev_barns,
                processor_kinematic_maximum_ev_barns: final_row.kinematic_maximum_ev_barns,
                processor_final_mt301_excess_ev_barns,
                printed_energy_balance_remainder_sum_ev_barns,
                remainder_excess_relative_difference,
                mt443_kinematic_maximum_relative_difference,
                contributions: matching,
                status,
            });
        }

        let attributed_in_domain_violation_count = samples
            .iter()
            .filter(|sample| {
                sample.status == NjoyEnergyBalanceAttributionStatus::AttributedWithinPrintTolerance
            })
            .count() as u64;
        let failed_sample_count = samples.len() as u64 - attributed_in_domain_violation_count;
        let contributing_reaction_mts = samples
            .iter()
            .flat_map(|sample| {
                sample
                    .contributions
                    .iter()
                    .map(|contribution| contribution.reaction_mt)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let qualification = attribution_qualification(
            attributed_in_domain_violation_count,
            in_domain_violations.len() as u64,
            failed_sample_count,
        );
        let attribution = Self {
            schema_version: NJOY_ENERGY_BALANCE_ATTRIBUTION_SCHEMA.into(),
            id: format!(
                "{}.{}.{}",
                domain.report.id,
                nuclide.to_ascii_lowercase(),
                REPORT_ID_SUFFIX
            ),
            case_id: domain.report.case_id.clone(),
            qualification,
            evidence_scope: NjoyEnergyBalanceEvidenceScope::ProcessorInternalPrintAccountingOnly,
            finding_disposition:
                NjoyEnergyBalanceFindingDisposition::RetainedForIndependentPhysicalValidation,
            domain_aware_suitability_report: ContentReference {
                id: domain.report.id.clone(),
                sha256: domain.sha256.clone(),
            },
            execution_receipt: execution_reference,
            print_relative_tolerance,
            nuclide: nuclide.into(),
            assessment_energy_range_ev: domain.report.assessment_energy_range_ev,
            response_mt: TOTAL_HEATING_MT,
            processor_report: execution_run.processor_report.clone(),
            full_evaluation_violation_count: execution_run.diagnostic_violation_count,
            in_domain_violation_count: in_domain_violations.len() as u64,
            out_of_domain_violation_count: out_of_domain_violations.len() as u64,
            processor_final_table_violation_count: processor_violations.len() as u64,
            printed_remainder_table_count,
            contributing_reaction_mts,
            attributed_in_domain_violation_count,
            failed_sample_count,
            physical_validation_required_count: in_domain_violations.len() as u64,
            waived_violation_count: 0,
            maximum_remainder_excess_relative_difference: samples
                .iter()
                .map(|sample| sample.remainder_excess_relative_difference)
                .fold(0.0_f64, f64::max),
            maximum_mt443_kinematic_maximum_relative_difference: samples
                .iter()
                .map(|sample| sample.mt443_kinematic_maximum_relative_difference)
                .fold(0.0_f64, f64::max),
            samples,
        };
        attribution.validate()?;
        Ok(attribution)
    }

    pub fn validate(&self) -> Result<(), NjoyEnergyBalanceAttributionError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_ENERGY_BALANCE_ATTRIBUTION_SCHEMA,
        ) {
            return invalid_attribution(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier("nuclide", &self.nuclide)?;
        validate_reference(
            "domain_aware_suitability_report",
            &self.domain_aware_suitability_report,
        )?;
        validate_reference("execution_receipt", &self.execution_receipt)?;
        validate_identifier("processor_report.path", &self.processor_report.path)?;
        validate_identifier(
            "processor_report.media_type",
            &self.processor_report.media_type,
        )?;
        validate_sha256("processor_report.sha256", &self.processor_report.sha256)?;
        validate_tolerance(self.print_relative_tolerance)?;
        if !self.assessment_energy_range_ev[0].is_finite()
            || !self.assessment_energy_range_ev[1].is_finite()
            || self.assessment_energy_range_ev[0] <= 0.0
            || self.assessment_energy_range_ev[0] >= self.assessment_energy_range_ev[1]
            || self.id
                != format!(
                    "{}.{}.{}",
                    self.domain_aware_suitability_report.id,
                    self.nuclide.to_ascii_lowercase(),
                    REPORT_ID_SUFFIX
                )
            || self.evidence_scope
                != NjoyEnergyBalanceEvidenceScope::ProcessorInternalPrintAccountingOnly
            || self.finding_disposition
                != NjoyEnergyBalanceFindingDisposition::RetainedForIndependentPhysicalValidation
            || self.response_mt != TOTAL_HEATING_MT
            || self.processor_report.size_bytes == 0
            || self.in_domain_violation_count == 0
            || self.full_evaluation_violation_count
                != self.in_domain_violation_count + self.out_of_domain_violation_count
            || self.full_evaluation_violation_count != self.processor_final_table_violation_count
            || self.in_domain_violation_count != self.samples.len() as u64
            || self.attributed_in_domain_violation_count + self.failed_sample_count
                != self.in_domain_violation_count
            || self.physical_validation_required_count != self.in_domain_violation_count
            || self.waived_violation_count != 0
            || self.printed_remainder_table_count == 0
            || self.contributing_reaction_mts.is_empty()
            || self
                .contributing_reaction_mts
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
        {
            return invalid_attribution("attribution identity or counts are inconsistent");
        }

        let mut previous_energy = None;
        for sample in &self.samples {
            validate_sample(self, sample, previous_energy)?;
            previous_energy = Some(sample.incident_energy_ev);
        }
        let attributed_count = self
            .samples
            .iter()
            .filter(|sample| {
                sample.status == NjoyEnergyBalanceAttributionStatus::AttributedWithinPrintTolerance
            })
            .count() as u64;
        let observed_reaction_mts = self
            .samples
            .iter()
            .flat_map(|sample| {
                sample
                    .contributions
                    .iter()
                    .map(|contribution| contribution.reaction_mt)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let maximum_remainder_difference = self
            .samples
            .iter()
            .map(|sample| sample.remainder_excess_relative_difference)
            .fold(0.0_f64, f64::max);
        let maximum_mt443_difference = self
            .samples
            .iter()
            .map(|sample| sample.mt443_kinematic_maximum_relative_difference)
            .fold(0.0_f64, f64::max);
        if self.attributed_in_domain_violation_count != attributed_count
            || self.failed_sample_count != self.samples.len() as u64 - attributed_count
            || self.contributing_reaction_mts != observed_reaction_mts
            || !approximately_equal(
                self.maximum_remainder_excess_relative_difference,
                maximum_remainder_difference,
            )
            || !approximately_equal(
                self.maximum_mt443_kinematic_maximum_relative_difference,
                maximum_mt443_difference,
            )
            || self.qualification
                != attribution_qualification(
                    attributed_count,
                    self.in_domain_violation_count,
                    self.failed_sample_count,
                )
        {
            return invalid_attribution("attribution aggregates do not match samples");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyEnergyBalanceAttributionResult, NjoyEnergyBalanceAttributionError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyEnergyBalanceAttributionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyEnergyBalanceAttributionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyEnergyBalanceAttributionResult {
            attribution: self.clone(),
            attribution_path: path.to_path_buf(),
            attribution_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyEnergyBalanceAttributionDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyEnergyBalanceAttributionError> {
        let attribution: NjoyEnergyBalanceAttribution = serde_json::from_slice(bytes)?;
        attribution.validate()?;
        Ok(Self {
            attribution,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyEnergyBalanceAttributionError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_evidence(
        &self,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
    ) -> Result<(), NjoyEnergyBalanceAttributionError> {
        let observed = NjoyEnergyBalanceAttribution::attribute(
            domain,
            execution,
            execution_root,
            &self.attribution.nuclide,
            self.attribution.print_relative_tolerance,
        )?;
        if self.attribution != observed {
            return Err(NjoyEnergyBalanceAttributionError::AttributionMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct PrintedRemainder {
    reaction_mt: u16,
    particle_id: u32,
    incident_energy_ev: f64,
    remainder_ev_barns: f64,
}

#[derive(Debug, Clone, Copy)]
struct FinalKermaRow {
    incident_energy_ev: f64,
    mt301_kerma_ev_barns: f64,
    mt443_kerma_ev_barns: f64,
    kinematic_maximum_ev_barns: f64,
    direction: Option<NjoyKinematicDirection>,
}

fn parse_file6_remainders(
    text: &str,
) -> Result<(Vec<PrintedRemainder>, u64), NjoyEnergyBalanceAttributionError> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut remainders = Vec::new();
    let mut table_count = 0_u64;
    for (line_index, line) in lines.iter().enumerate() {
        let Some((reaction_mt, particle_id)) = parse_file6_header(line)? else {
            continue;
        };
        let mut cursor = line_index + 1;
        let mut current_energy = None;
        let mut found_remainder = false;
        let mut found_data = false;
        while cursor < lines.len() {
            if parse_file6_header(lines[cursor])?.is_some() {
                break;
            }
            let fields = lines[cursor].split_whitespace().collect::<Vec<_>>();
            if fields.len() >= 5
                && let Ok(values) = fields
                    .iter()
                    .map(|field| field.parse::<f64>())
                    .collect::<Result<Vec<_>, _>>()
            {
                if values[0] <= 0.0 || values.iter().any(|value| !value.is_finite()) {
                    return Err(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable);
                }
                current_energy = Some(values[0]);
                found_data = true;
                cursor += 1;
                continue;
            }
            if fields.len() == 2 && fields[0] == "ebal" {
                let incident_energy_ev = current_energy
                    .ok_or(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
                let remainder_ev_barns = fields[1]
                    .parse::<f64>()
                    .map_err(|_| NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
                if !remainder_ev_barns.is_finite() {
                    return Err(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable);
                }
                remainders.push(PrintedRemainder {
                    reaction_mt,
                    particle_id,
                    incident_energy_ev,
                    remainder_ev_barns,
                });
                found_remainder = true;
                cursor += 1;
                continue;
            }
            if found_data && (lines[cursor].trim().is_empty() || !fields.is_empty()) {
                break;
            }
            cursor += 1;
        }
        if found_remainder {
            table_count += 1;
        }
    }
    if remainders.is_empty() || table_count == 0 {
        return Err(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable);
    }
    Ok((remainders, table_count))
}

fn parse_file6_header(line: &str) -> Result<Option<(u16, u32)>, NjoyEnergyBalanceAttributionError> {
    let trimmed = line.trim();
    let Some(tail) = trimmed.strip_prefix("file six heating for mt") else {
        return Ok(None);
    };
    let (reaction, particle_and_q) = tail
        .split_once(", particle =")
        .ok_or(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
    let reaction_mt = reaction
        .trim()
        .parse::<u16>()
        .map_err(|_| NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
    let (particle, q_value) = particle_and_q
        .split_once("q =")
        .ok_or(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
    let particle_id = particle
        .trim()
        .parse::<u32>()
        .map_err(|_| NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
    let parsed_q = q_value
        .trim()
        .parse::<f64>()
        .map_err(|_| NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)?;
    if reaction_mt == 0 || !parsed_q.is_finite() {
        return Err(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable);
    }
    Ok(Some((reaction_mt, particle_id)))
}

fn parse_final_kerma_table(
    text: &str,
) -> Result<Vec<FinalKermaRow>, NjoyEnergyBalanceAttributionError> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut starts = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim() == "final kerma factors")
        .map(|(index, _)| index);
    let start = starts
        .next()
        .ok_or(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable)?;
    if starts.next().is_some() {
        return Err(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable);
    }
    let header = lines
        .get(start + 1)
        .ok_or(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable)?
        .split_whitespace()
        .collect::<Vec<_>>();
    if header != ["e", "301", "443"] {
        return Err(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable);
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
            pending_minimum = Some(
                fields[1]
                    .parse::<f64>()
                    .map_err(|_| NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable)?,
            );
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
                .ok_or(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable)?;
            if !minimum.is_finite() {
                return Err(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable);
            }
            let max_index = next_nonempty_line(&lines, cursor + 1)
                .ok_or(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable)?;
            let max_fields = lines[max_index].split_whitespace().collect::<Vec<_>>();
            if max_fields.len() != 2 || max_fields[0] != "max" {
                return Err(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable);
            }
            let maximum = max_fields[1]
                .parse::<f64>()
                .map_err(|_| NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable)?;
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
                        return Err(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable);
                    }
                    direction = observed;
                }
                lookahead += 1;
            }
            rows.push(FinalKermaRow {
                incident_energy_ev: values[0],
                mt301_kerma_ev_barns: values[1],
                mt443_kerma_ev_barns: values[2],
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
                || !row.kinematic_maximum_ev_barns.is_finite()
        })
        || rows
            .windows(2)
            .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(NjoyEnergyBalanceAttributionError::UnparsedFinalKermaTable);
    }
    Ok(rows)
}

fn next_nonempty_line(lines: &[&str], start: usize) -> Option<usize> {
    (start..lines.len()).find(|index| !lines[*index].trim().is_empty())
}

fn exact_final_row(
    rows: &[FinalKermaRow],
    energy_ev: f64,
) -> Result<Option<&FinalKermaRow>, NjoyEnergyBalanceAttributionError> {
    let mut matching = rows
        .iter()
        .filter(|row| row.incident_energy_ev == energy_ev);
    let row = matching.next();
    if matching.next().is_some() {
        return Err(NjoyEnergyBalanceAttributionError::AmbiguousFinalKermaRow(
            energy_ev,
        ));
    }
    Ok(row)
}

fn validate_sample(
    attribution: &NjoyEnergyBalanceAttribution,
    sample: &NjoyEnergyBalanceAttributionSample,
    previous_energy: Option<f64>,
) -> Result<(), NjoyEnergyBalanceAttributionError> {
    let values = [
        sample.incident_energy_ev,
        sample.processor_final_mt301_kerma_ev_barns,
        sample.processor_final_mt443_kerma_ev_barns,
        sample.processor_kinematic_maximum_ev_barns,
        sample.processor_final_mt301_excess_ev_barns,
        sample.printed_energy_balance_remainder_sum_ev_barns,
        sample.remainder_excess_relative_difference,
        sample.mt443_kinematic_maximum_relative_difference,
    ];
    if values.iter().any(|value| !value.is_finite())
        || sample.incident_energy_ev <= 0.0
        || !contains_energy(
            attribution.assessment_energy_range_ev,
            sample.incident_energy_ev,
        )
        || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
        || sample.response_mt != attribution.response_mt
        || sample.processor_kinematic_direction != NjoyKinematicDirection::High
        || sample.processor_final_mt301_excess_ev_barns <= 0.0
        || sample.remainder_excess_relative_difference < 0.0
        || sample.mt443_kinematic_maximum_relative_difference < 0.0
        || sample.contributions.is_empty()
    {
        return invalid_attribution("invalid or unordered attribution sample");
    }
    let mut previous_contribution = None;
    for contribution in &sample.contributions {
        if contribution.reaction_mt == 0
            || !contribution.printed_remainder_ev_barns.is_finite()
            || previous_contribution.is_some_and(|previous| {
                previous >= (contribution.reaction_mt, contribution.particle_id)
            })
        {
            return invalid_attribution("invalid or unordered energy-balance contribution");
        }
        previous_contribution = Some((contribution.reaction_mt, contribution.particle_id));
    }
    let expected_sum = sample
        .contributions
        .iter()
        .map(|contribution| contribution.printed_remainder_ev_barns)
        .sum::<f64>();
    let expected_excess =
        sample.processor_final_mt301_kerma_ev_barns - sample.processor_kinematic_maximum_ev_barns;
    let expected_remainder_difference = relative_difference(expected_sum, expected_excess);
    let expected_mt443_difference = relative_difference(
        sample.processor_final_mt443_kerma_ev_barns,
        sample.processor_kinematic_maximum_ev_barns,
    );
    if !approximately_equal(
        sample.printed_energy_balance_remainder_sum_ev_barns,
        expected_sum,
    ) || !approximately_equal(
        sample.processor_final_mt301_excess_ev_barns,
        expected_excess,
    ) || !approximately_equal(
        sample.remainder_excess_relative_difference,
        expected_remainder_difference,
    ) || !approximately_equal(
        sample.mt443_kinematic_maximum_relative_difference,
        expected_mt443_difference,
    ) || sample.status
        != attribution_status(
            Some(sample.processor_kinematic_direction),
            expected_excess,
            false,
            expected_remainder_difference,
            expected_mt443_difference,
            attribution.print_relative_tolerance,
        )
    {
        return invalid_attribution("attribution sample derived values do not close");
    }
    Ok(())
}

fn attribution_status(
    direction: Option<NjoyKinematicDirection>,
    final_excess_ev_barns: f64,
    contributions_empty: bool,
    remainder_excess_relative_difference: f64,
    mt443_kinematic_maximum_relative_difference: f64,
    print_relative_tolerance: f64,
) -> NjoyEnergyBalanceAttributionStatus {
    if direction == Some(NjoyKinematicDirection::High)
        && final_excess_ev_barns > 0.0
        && !contributions_empty
        && remainder_excess_relative_difference <= print_relative_tolerance
        && mt443_kinematic_maximum_relative_difference <= print_relative_tolerance
    {
        NjoyEnergyBalanceAttributionStatus::AttributedWithinPrintTolerance
    } else {
        NjoyEnergyBalanceAttributionStatus::NotAttributed
    }
}

fn attribution_qualification(
    attributed_count: u64,
    in_domain_count: u64,
    failed_count: u64,
) -> NjoyEnergyBalanceAttributionQualification {
    if in_domain_count > 0 && attributed_count == in_domain_count && failed_count == 0 {
        NjoyEnergyBalanceAttributionQualification::
            ProcessorAccountingMechanismAttributedPhysicalValidationRequired
    } else {
        NjoyEnergyBalanceAttributionQualification::ProcessorAccountingAttributionMismatch
    }
}

fn contains_energy([lower, upper]: [f64; 2], energy_ev: f64) -> bool {
    energy_ev >= lower && energy_ev <= upper
}

fn relative_difference(left: f64, right: f64) -> f64 {
    if left == right {
        0.0
    } else {
        (left - right).abs() / left.abs().max(right.abs()).max(f64::MIN_POSITIVE)
    }
}

fn approximately_equal(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= 1.0e-12 * scale
}

fn validate_tolerance(value: f64) -> Result<(), NjoyEnergyBalanceAttributionError> {
    if !value.is_finite() || value <= 0.0 || value > 5.0e-3 {
        return invalid_attribution("print relative tolerance must be in (0, 5e-3]");
    }
    Ok(())
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), NjoyEnergyBalanceAttributionError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyEnergyBalanceAttributionError> {
    if value.trim().is_empty() {
        invalid_attribution(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    digest: &str,
) -> Result<(), NjoyEnergyBalanceAttributionError> {
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_attribution(format!("{label} is not a lowercase SHA-256 digest"))
    }
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyEnergyBalanceAttributionError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| NjoyEnergyBalanceAttributionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyEnergyBalanceAttributionError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyEnergyBalanceAttributionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_attribution<T>(
    message: impl Into<String>,
) -> Result<T, NjoyEnergyBalanceAttributionError> {
    Err(NjoyEnergyBalanceAttributionError::InvalidAttribution(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoyEnergyBalanceAttributionError {
    #[error(transparent)]
    Domain(#[from] NjoyDomainAwareSuitabilityError),
    #[error(transparent)]
    Execution(#[from] NjoyExecutionError),
    #[error("domain-aware report does not bind the supplied execution receipt")]
    ExecutionBindingMismatch,
    #[error("domain-aware report and execution receipt have different cases")]
    CaseBindingMismatch,
    #[error("domain-aware report has no run for {0}")]
    MissingDomainRun(String),
    #[error("execution receipt has no run for {0}")]
    MissingExecutionRun(String),
    #[error("domain and execution run evidence does not match for {0}")]
    RunEvidenceMismatch(String),
    #[error("domain diagnostic partition does not match the receipt for {0}")]
    DomainPartitionMismatch(String),
    #[error("execution receipt has no in-domain diagnostic for {0}")]
    NoInDomainViolation(String),
    #[error("{0} has a diagnostic other than a high MT=301 kinematic finding")]
    UnsupportedViolation(String),
    #[error("processor report changed after execution verification: {0}")]
    ProcessorReportChanged(String),
    #[error("processor report is not UTF-8 text: {0}")]
    NonUtf8ProcessorReport(PathBuf),
    #[error("NJOY File 6 energy-balance remainder tables could not be parsed")]
    UnparsedRemainderTable,
    #[error("NJOY final KERMA table could not be parsed uniquely")]
    UnparsedFinalKermaTable,
    #[error("receipt violations do not exactly match the NJOY final KERMA table")]
    ReceiptViolationMismatch,
    #[error("NJOY final KERMA table has no row for receipt energy {0}")]
    MissingFinalKermaRow(f64),
    #[error("NJOY final KERMA table ambiguously matches receipt energy {0}")]
    AmbiguousFinalKermaRow(f64),
    #[error("NJOY File 6 remainder tables ambiguously match receipt energy {0}")]
    AmbiguousRemainder(f64),
    #[error("invalid NJOY energy-balance attribution: {0}")]
    InvalidAttribution(String),
    #[error("stored NJOY energy-balance attribution does not match regenerated evidence")]
    AttributionMismatch,
    #[error("required attribution artifact is not a regular file: {0}")]
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

    const JEFF40_O17_ATTRIBUTION: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-o17-njoy-energy-balance-attribution.json"
    );

    #[test]
    fn parses_and_attributes_summed_remainders() {
        let text = " file six heating for mt 16, particle =     0     q =  -4.1431E+06\n\
                    e ebar yield xsec heating\n\
                    1.0000E+07 3.1590E+06 1.3093E-01 5.6046E-01 0.0000E+00\n\
                    ebal 2.7642E+05\n\
                    \n\
                    file six heating for mt107, particle =     0     q =   1.6000E+06\n\
                    e ebar yield xsec heating\n\
                    1.0000E+07 7.5000E+02 1.0000E-09 1.0000E+00 0.0000E+00\n\
                    ebal 7.9888E+05\n\
                    \n\
                    final kerma factors\n\
                    e 301 443\n\
                    min 2.0000E+06\n\
                    1.0000E+07 3.0753E+06 2.0000E+06\n\
                    max 2.0000E+06\n\
                    high\n\
                    *****************************************************************************\n";
        let (remainders, table_count) = parse_file6_remainders(text).unwrap();
        assert_eq!(table_count, 2);
        assert_eq!(remainders.len(), 2);
        assert_eq!(remainders[0].reaction_mt, 16);
        assert_eq!(remainders[1].reaction_mt, 107);
        assert_eq!(
            remainders
                .iter()
                .map(|remainder| remainder.remainder_ev_barns)
                .sum::<f64>(),
            1_075_300.0
        );
        let rows = parse_final_kerma_table(text).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].direction, Some(NjoyKinematicDirection::High));
        assert_eq!(
            rows[0].mt301_kerma_ev_barns - rows[0].kinematic_maximum_ev_barns,
            1_075_300.0
        );
    }

    #[test]
    fn rejects_malformed_file6_header() {
        let text = "file six heating for mt nope, particle = 0 q = 0.0\n";
        assert!(matches!(
            parse_file6_remainders(text),
            Err(NjoyEnergyBalanceAttributionError::UnparsedRemainderTable)
        ));
    }

    #[test]
    fn validates_frozen_o17_processor_accounting_without_waiving_findings() {
        let document =
            NjoyEnergyBalanceAttributionDocument::from_bytes(JEFF40_O17_ATTRIBUTION).unwrap();
        assert_eq!(
            document.sha256,
            "1c38d1e5fb6a6b26e5d99fc1505bd3aa15b25a2b01116e47aed5566381e093d8"
        );
        assert_eq!(
            document.attribution.qualification,
            NjoyEnergyBalanceAttributionQualification::
                ProcessorAccountingMechanismAttributedPhysicalValidationRequired
        );
        assert_eq!(document.attribution.full_evaluation_violation_count, 45);
        assert_eq!(document.attribution.in_domain_violation_count, 43);
        assert_eq!(document.attribution.out_of_domain_violation_count, 2);
        assert_eq!(
            document.attribution.attributed_in_domain_violation_count,
            43
        );
        assert_eq!(document.attribution.physical_validation_required_count, 43);
        assert_eq!(document.attribution.waived_violation_count, 0);
        assert_eq!(document.attribution.failed_sample_count, 0);
        assert_eq!(
            document
                .attribution
                .maximum_remainder_excess_relative_difference,
            0.0003721276397804447
        );
        let thermal = &document.attribution.samples[0];
        assert_eq!(thermal.incident_energy_ev, 1.0e-5);
        assert_eq!(thermal.contributions.len(), 1);
        assert_eq!(thermal.contributions[0].reaction_mt, 107);
        assert_eq!(
            thermal.printed_energy_balance_remainder_sum_ev_barns,
            9_347_200.0
        );
    }

    #[test]
    fn rejects_tampered_o17_remainder_sum() {
        let mut attribution =
            NjoyEnergyBalanceAttributionDocument::from_bytes(JEFF40_O17_ATTRIBUTION)
                .unwrap()
                .attribution;
        attribution.samples[0].contributions[0].printed_remainder_ev_barns += 1.0;
        assert!(matches!(
            attribution.validate(),
            Err(NjoyEnergyBalanceAttributionError::InvalidAttribution(_))
        ));
    }
}
