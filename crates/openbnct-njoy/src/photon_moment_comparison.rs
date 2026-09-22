// SPDX-License-Identifier: MIT

//! Comparison of independently calculated source moments with NJOY's bounded-
//! precision diagnostic printout.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    EndfContinuumPhotonMomentReportDocument, EndfPhotonMomentError, NjoyExecutionArtifact,
    NjoyExecutionError, NjoyExecutionReceiptDocument,
};

pub const NJOY_PHOTON_MOMENT_COMPARISON_SCHEMA: &str =
    "openbnct.njoy-continuum-photon-moment-comparison/0.1.0";
pub const DEFAULT_NJOY_PRINT_RELATIVE_TOLERANCE: f64 = 6.0e-5;

const REPORT_ID_SUFFIX: &str = "njoy2016-78-print-comparison";
const TABLE_MARKER: &str = "photon energy (from xsecs) mf13, mt";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyPhotonMomentComparison {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyPhotonMomentComparisonQualification,
    pub independent_moment_report: ContentReference,
    pub execution_receipt: ContentReference,
    pub relative_tolerance: f64,
    pub reactions: Vec<NjoyPhotonMomentComparisonReaction>,
    pub reaction_count: u64,
    pub independent_sample_count: u64,
    pub processor_sample_count: u64,
    pub compared_sample_count: u64,
    pub uncompared_independent_sample_count: u64,
    pub skipped_interpolated_sample_count: u64,
    pub failed_sample_count: u64,
    pub maximum_relative_difference: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyPhotonMomentComparisonQualification {
    IndependentMomentsMatchProcessorPrintUnreviewed,
    ProcessorPrintMismatchRejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyPhotonMomentComparisonReaction {
    pub nuclide: String,
    pub reaction_mt: u16,
    pub processor_report: NjoyExecutionArtifact,
    pub independent_sample_count: u64,
    pub processor_sample_count: u64,
    pub samples: Vec<NjoyPhotonMomentComparisonSample>,
    pub uncompared_independent_sample_count: u64,
    pub skipped_interpolated_sample_count: u64,
    pub failed_sample_count: u64,
    pub maximum_relative_difference: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyPhotonMomentComparisonSample {
    pub incident_energy_ev: f64,
    pub independent_mean_photon_energy_ev: f64,
    pub processor_mean_photon_energy_ev: f64,
    pub mean_energy_relative_difference: f64,
    pub independent_cross_section_barns: f64,
    pub processor_cross_section_barns: f64,
    pub cross_section_relative_difference: f64,
    pub independent_energy_release_ev_barns: f64,
    pub processor_energy_release_ev_barns: f64,
    pub energy_release_relative_difference: f64,
    pub processor_heating_ev_barns: f64,
    pub processor_heating_balance_relative_difference: f64,
    pub status: NjoyPhotonMomentComparisonStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyPhotonMomentComparisonStatus {
    WithinPrintPrecision,
    OutsidePrintPrecision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyPhotonMomentComparisonDocument {
    pub comparison: NjoyPhotonMomentComparison,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyPhotonMomentComparisonResult {
    pub comparison: NjoyPhotonMomentComparison,
    pub comparison_path: PathBuf,
    pub comparison_sha256: String,
}

impl NjoyPhotonMomentComparison {
    pub fn compare(
        moments: &EndfContinuumPhotonMomentReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        relative_tolerance: f64,
    ) -> Result<Self, NjoyPhotonMomentComparisonError> {
        moments.report.validate()?;
        validate_tolerance(relative_tolerance)?;
        execution.verify_execution_root(execution_root)?;
        if moments.report.case_id != execution.receipt.case_id {
            return Err(NjoyPhotonMomentComparisonError::CaseBindingMismatch);
        }

        let mut printed_by_nuclide = BTreeMap::new();
        let mut artifacts_by_nuclide = BTreeMap::new();
        for nuclide in moments
            .report
            .reactions
            .iter()
            .map(|reaction| reaction.nuclide.as_str())
        {
            if printed_by_nuclide.contains_key(nuclide) {
                continue;
            }
            let executed = execution
                .receipt
                .runs
                .iter()
                .find(|run| run.nuclide == nuclide)
                .ok_or_else(|| {
                    NjoyPhotonMomentComparisonError::MissingProcessorRun(nuclide.into())
                })?;
            let path = execution_root.join(&executed.processor_report.path);
            let bytes = read_regular_file(&path)?;
            if bytes.len() as u64 != executed.processor_report.size_bytes
                || sha256_bytes(&bytes) != executed.processor_report.sha256
            {
                return Err(NjoyPhotonMomentComparisonError::ProcessorReportChanged(
                    executed.processor_report.path.clone(),
                ));
            }
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                NjoyPhotonMomentComparisonError::NonUtf8ProcessorReport(path.clone())
            })?;
            printed_by_nuclide.insert(nuclide.to_owned(), parse_processor_tables(text)?);
            artifacts_by_nuclide.insert(nuclide.to_owned(), executed.processor_report.clone());
        }

        let mut reactions = Vec::with_capacity(moments.report.reactions.len());
        for independent_reaction in &moments.report.reactions {
            let printed = printed_by_nuclide
                .get(&independent_reaction.nuclide)
                .and_then(|tables| tables.get(&independent_reaction.reaction_mt))
                .ok_or_else(|| NjoyPhotonMomentComparisonError::MissingProcessorTable {
                    nuclide: independent_reaction.nuclide.clone(),
                    reaction_mt: independent_reaction.reaction_mt,
                })?;
            let mut samples = Vec::with_capacity(printed.len());
            for row in printed {
                let mut matching = independent_reaction.samples.iter().filter(|sample| {
                    relative_difference(sample.incident_energy_ev, row.incident_energy_ev)
                        <= relative_tolerance
                });
                let Some(independent) = matching.next() else {
                    // HEATR prints on its own union grid. This comparison is
                    // intentionally restricted to File 15 incident-energy
                    // nodes shared within the print tolerance so it does not
                    // duplicate HEATR's 2-D spectrum interpolation.
                    continue;
                };
                if matching.next().is_some() {
                    return Err(NjoyPhotonMomentComparisonError::AmbiguousSourceNode {
                        nuclide: independent_reaction.nuclide.clone(),
                        reaction_mt: independent_reaction.reaction_mt,
                        processor_energy_ev: row.incident_energy_ev,
                    });
                }
                let mean_energy_relative_difference = relative_difference(
                    independent.mean_photon_energy_ev,
                    row.mean_photon_energy_ev,
                );
                let cross_section_relative_difference = relative_difference(
                    independent.continuum_cross_section_barns,
                    row.cross_section_barns,
                );
                let energy_release_relative_difference = relative_difference(
                    independent.photon_energy_release_ev_barns,
                    row.energy_release_ev_barns,
                );
                let heating_relative_difference =
                    relative_difference(-row.energy_release_ev_barns, row.heating_ev_barns);
                let maximum = mean_energy_relative_difference
                    .max(cross_section_relative_difference)
                    .max(energy_release_relative_difference)
                    .max(heating_relative_difference);
                let status = if maximum <= relative_tolerance {
                    NjoyPhotonMomentComparisonStatus::WithinPrintPrecision
                } else {
                    NjoyPhotonMomentComparisonStatus::OutsidePrintPrecision
                };
                samples.push(NjoyPhotonMomentComparisonSample {
                    incident_energy_ev: independent.incident_energy_ev,
                    independent_mean_photon_energy_ev: independent.mean_photon_energy_ev,
                    processor_mean_photon_energy_ev: row.mean_photon_energy_ev,
                    mean_energy_relative_difference,
                    independent_cross_section_barns: independent.continuum_cross_section_barns,
                    processor_cross_section_barns: row.cross_section_barns,
                    cross_section_relative_difference,
                    independent_energy_release_ev_barns: independent.photon_energy_release_ev_barns,
                    processor_energy_release_ev_barns: row.energy_release_ev_barns,
                    energy_release_relative_difference,
                    processor_heating_ev_barns: row.heating_ev_barns,
                    processor_heating_balance_relative_difference: heating_relative_difference,
                    status,
                });
            }
            let failed_sample_count = samples
                .iter()
                .filter(|sample| {
                    sample.status == NjoyPhotonMomentComparisonStatus::OutsidePrintPrecision
                })
                .count() as u64;
            let maximum_relative_difference = samples
                .iter()
                .map(sample_maximum_difference)
                .fold(0.0_f64, f64::max);
            if samples.is_empty() {
                return Err(NjoyPhotonMomentComparisonError::NoSharedSourceNode {
                    nuclide: independent_reaction.nuclide.clone(),
                    reaction_mt: independent_reaction.reaction_mt,
                });
            }
            reactions.push(NjoyPhotonMomentComparisonReaction {
                nuclide: independent_reaction.nuclide.clone(),
                reaction_mt: independent_reaction.reaction_mt,
                processor_report: artifacts_by_nuclide[&independent_reaction.nuclide].clone(),
                independent_sample_count: independent_reaction.samples.len() as u64,
                processor_sample_count: printed.len() as u64,
                uncompared_independent_sample_count: independent_reaction.samples.len() as u64
                    - samples.len() as u64,
                skipped_interpolated_sample_count: printed.len() as u64 - samples.len() as u64,
                samples,
                failed_sample_count,
                maximum_relative_difference,
            });
        }

        let failed_sample_count = reactions
            .iter()
            .map(|reaction| reaction.failed_sample_count)
            .sum();
        let comparison = Self {
            schema_version: NJOY_PHOTON_MOMENT_COMPARISON_SCHEMA.into(),
            id: format!("{}.{}", moments.report.id, REPORT_ID_SUFFIX),
            case_id: moments.report.case_id.clone(),
            qualification: if failed_sample_count == 0 {
                NjoyPhotonMomentComparisonQualification::IndependentMomentsMatchProcessorPrintUnreviewed
            } else {
                NjoyPhotonMomentComparisonQualification::ProcessorPrintMismatchRejected
            },
            independent_moment_report: ContentReference {
                id: moments.report.id.clone(),
                sha256: moments.sha256.clone(),
            },
            execution_receipt: ContentReference {
                id: execution.receipt.id.clone(),
                sha256: execution.sha256.clone(),
            },
            relative_tolerance,
            reaction_count: reactions.len() as u64,
            independent_sample_count: reactions
                .iter()
                .map(|reaction| reaction.independent_sample_count)
                .sum(),
            processor_sample_count: reactions
                .iter()
                .map(|reaction| reaction.processor_sample_count)
                .sum(),
            compared_sample_count: reactions
                .iter()
                .map(|reaction| reaction.samples.len() as u64)
                .sum(),
            uncompared_independent_sample_count: reactions
                .iter()
                .map(|reaction| reaction.uncompared_independent_sample_count)
                .sum(),
            skipped_interpolated_sample_count: reactions
                .iter()
                .map(|reaction| reaction.skipped_interpolated_sample_count)
                .sum(),
            failed_sample_count,
            maximum_relative_difference: reactions
                .iter()
                .map(|reaction| reaction.maximum_relative_difference)
                .fold(0.0_f64, f64::max),
            reactions,
        };
        comparison.validate()?;
        Ok(comparison)
    }

    pub fn validate(&self) -> Result<(), NjoyPhotonMomentComparisonError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_PHOTON_MOMENT_COMPARISON_SCHEMA,
        ) {
            return invalid_comparison(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_reference("independent_moment_report", &self.independent_moment_report)?;
        validate_reference("execution_receipt", &self.execution_receipt)?;
        if self.id != format!("{}.{}", self.independent_moment_report.id, REPORT_ID_SUFFIX) {
            return invalid_comparison("comparison ID does not bind the independent report");
        }
        validate_tolerance(self.relative_tolerance)?;
        if self.reactions.is_empty() {
            return invalid_comparison("comparison contains no reactions");
        }

        let mut previous_key: Option<(&str, u16)> = None;
        for reaction in &self.reactions {
            validate_identifier("reactions.nuclide", &reaction.nuclide)?;
            let key = (reaction.nuclide.as_str(), reaction.reaction_mt);
            if reaction.reaction_mt == 0
                || previous_key.is_some_and(|previous| previous >= key)
                || reaction.samples.is_empty()
                || reaction.independent_sample_count
                    != reaction.samples.len() as u64 + reaction.uncompared_independent_sample_count
                || reaction.processor_sample_count
                    != reaction.samples.len() as u64 + reaction.skipped_interpolated_sample_count
            {
                return invalid_comparison("reactions are not canonical");
            }
            previous_key = Some(key);
            validate_sha256(
                "reactions.processor_report",
                &reaction.processor_report.sha256,
            )?;
            let mut previous_energy = None;
            for sample in &reaction.samples {
                let values = [
                    sample.incident_energy_ev,
                    sample.independent_mean_photon_energy_ev,
                    sample.processor_mean_photon_energy_ev,
                    sample.mean_energy_relative_difference,
                    sample.independent_cross_section_barns,
                    sample.processor_cross_section_barns,
                    sample.cross_section_relative_difference,
                    sample.independent_energy_release_ev_barns,
                    sample.processor_energy_release_ev_barns,
                    sample.energy_release_relative_difference,
                    sample.processor_heating_balance_relative_difference,
                ];
                if values
                    .iter()
                    .any(|value| !value.is_finite() || *value < 0.0)
                    || sample.incident_energy_ev == 0.0
                    || !sample.processor_heating_ev_barns.is_finite()
                    || sample.processor_heating_ev_barns > 0.0
                    || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
                {
                    return invalid_comparison("invalid or unordered comparison sample");
                }
                previous_energy = Some(sample.incident_energy_ev);
                if !approximately_equal(
                    sample.mean_energy_relative_difference,
                    relative_difference(
                        sample.independent_mean_photon_energy_ev,
                        sample.processor_mean_photon_energy_ev,
                    ),
                ) || !approximately_equal(
                    sample.cross_section_relative_difference,
                    relative_difference(
                        sample.independent_cross_section_barns,
                        sample.processor_cross_section_barns,
                    ),
                ) || !approximately_equal(
                    sample.energy_release_relative_difference,
                    relative_difference(
                        sample.independent_energy_release_ev_barns,
                        sample.processor_energy_release_ev_barns,
                    ),
                ) || !approximately_equal(
                    sample.processor_heating_balance_relative_difference,
                    relative_difference(
                        -sample.processor_energy_release_ev_barns,
                        sample.processor_heating_ev_barns,
                    ),
                ) {
                    return invalid_comparison("comparison sample differences do not regenerate");
                }
                let expected_status =
                    if sample_maximum_difference(sample) <= self.relative_tolerance {
                        NjoyPhotonMomentComparisonStatus::WithinPrintPrecision
                    } else {
                        NjoyPhotonMomentComparisonStatus::OutsidePrintPrecision
                    };
                if sample.status != expected_status {
                    return invalid_comparison("comparison sample status is inconsistent");
                }
            }
            let failed = reaction
                .samples
                .iter()
                .filter(|sample| {
                    sample.status == NjoyPhotonMomentComparisonStatus::OutsidePrintPrecision
                })
                .count() as u64;
            let maximum = reaction
                .samples
                .iter()
                .map(sample_maximum_difference)
                .fold(0.0_f64, f64::max);
            if reaction.failed_sample_count != failed
                || !approximately_equal(reaction.maximum_relative_difference, maximum)
            {
                return invalid_comparison("reaction aggregates do not match its samples");
            }
        }
        let reaction_count = self.reactions.len() as u64;
        let independent_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.independent_sample_count)
            .sum();
        let processor_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.processor_sample_count)
            .sum();
        let compared_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.samples.len() as u64)
            .sum();
        let uncompared_independent_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.uncompared_independent_sample_count)
            .sum();
        let skipped_interpolated_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.skipped_interpolated_sample_count)
            .sum();
        let failed_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.failed_sample_count)
            .sum();
        let maximum_relative_difference = self
            .reactions
            .iter()
            .map(|reaction| reaction.maximum_relative_difference)
            .fold(0.0_f64, f64::max);
        if self.reaction_count != reaction_count
            || self.independent_sample_count != independent_sample_count
            || self.processor_sample_count != processor_sample_count
            || self.compared_sample_count != compared_sample_count
            || self.uncompared_independent_sample_count != uncompared_independent_sample_count
            || self.skipped_interpolated_sample_count != skipped_interpolated_sample_count
            || self.failed_sample_count != failed_sample_count
            || !approximately_equal(
                self.maximum_relative_difference,
                maximum_relative_difference,
            )
        {
            return invalid_comparison("comparison aggregates do not match reactions");
        }
        let expected_qualification = if failed_sample_count == 0 {
            NjoyPhotonMomentComparisonQualification::IndependentMomentsMatchProcessorPrintUnreviewed
        } else {
            NjoyPhotonMomentComparisonQualification::ProcessorPrintMismatchRejected
        };
        if self.qualification != expected_qualification {
            return invalid_comparison("qualification does not match comparison samples");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyPhotonMomentComparisonResult, NjoyPhotonMomentComparisonError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyPhotonMomentComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyPhotonMomentComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyPhotonMomentComparisonResult {
            comparison: self.clone(),
            comparison_path: path.to_path_buf(),
            comparison_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyPhotonMomentComparisonDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyPhotonMomentComparisonError> {
        let comparison: NjoyPhotonMomentComparison = serde_json::from_slice(bytes)?;
        comparison.validate()?;
        Ok(Self {
            comparison,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyPhotonMomentComparisonError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_evidence(
        &self,
        moments: &EndfContinuumPhotonMomentReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
    ) -> Result<(), NjoyPhotonMomentComparisonError> {
        let observed = NjoyPhotonMomentComparison::compare(
            moments,
            execution,
            execution_root,
            self.comparison.relative_tolerance,
        )?;
        if self.comparison != observed {
            return Err(NjoyPhotonMomentComparisonError::ComparisonMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct PrintedMoment {
    incident_energy_ev: f64,
    mean_photon_energy_ev: f64,
    cross_section_barns: f64,
    energy_release_ev_barns: f64,
    heating_ev_barns: f64,
}

fn parse_processor_tables(
    text: &str,
) -> Result<BTreeMap<u16, Vec<PrintedMoment>>, NjoyPhotonMomentComparisonError> {
    let mut tables = BTreeMap::new();
    let mut current_mt = None;
    let mut active = false;
    let mut rows = Vec::new();
    for line in text.lines() {
        if let Some(marker) = line.find(TABLE_MARKER) {
            finish_table(&mut tables, current_mt.take(), &mut rows)?;
            let value = line[marker + TABLE_MARKER.len()..].trim();
            current_mt = Some(
                value
                    .parse()
                    .map_err(|_| NjoyPhotonMomentComparisonError::UnparsedProcessorTable)?,
            );
            active = false;
            continue;
        }
        if current_mt.is_some() && line.contains("continuum gammas") {
            active = true;
            continue;
        }
        if active {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() == 5 {
                let parsed = fields
                    .iter()
                    .map(|field| field.parse::<f64>())
                    .collect::<Result<Vec<_>, _>>();
                if let Ok(values) = parsed {
                    rows.push(PrintedMoment {
                        incident_energy_ev: values[0],
                        mean_photon_energy_ev: values[1],
                        cross_section_barns: values[2],
                        energy_release_ev_barns: values[3],
                        heating_ev_barns: values[4],
                    });
                    continue;
                }
            }
            if !rows.is_empty() {
                active = false;
            }
        }
    }
    finish_table(&mut tables, current_mt, &mut rows)?;
    Ok(tables)
}

fn finish_table(
    tables: &mut BTreeMap<u16, Vec<PrintedMoment>>,
    reaction_mt: Option<u16>,
    rows: &mut Vec<PrintedMoment>,
) -> Result<(), NjoyPhotonMomentComparisonError> {
    let Some(reaction_mt) = reaction_mt else {
        return Ok(());
    };
    if rows.is_empty()
        || rows.iter().any(|row| {
            !row.incident_energy_ev.is_finite()
                || row.incident_energy_ev <= 0.0
                || !row.mean_photon_energy_ev.is_finite()
                || row.mean_photon_energy_ev < 0.0
                || !row.cross_section_barns.is_finite()
                || row.cross_section_barns < 0.0
                || !row.energy_release_ev_barns.is_finite()
                || row.energy_release_ev_barns < 0.0
                || !row.heating_ev_barns.is_finite()
                || row.heating_ev_barns > 0.0
        })
        || rows
            .windows(2)
            .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
        || tables.insert(reaction_mt, std::mem::take(rows)).is_some()
    {
        return Err(NjoyPhotonMomentComparisonError::UnparsedProcessorTable);
    }
    Ok(())
}

fn sample_maximum_difference(sample: &NjoyPhotonMomentComparisonSample) -> f64 {
    sample
        .mean_energy_relative_difference
        .max(sample.cross_section_relative_difference)
        .max(sample.energy_release_relative_difference)
        .max(sample.processor_heating_balance_relative_difference)
}

fn relative_difference(left: f64, right: f64) -> f64 {
    if left == right {
        return 0.0;
    }
    (left - right).abs() / left.abs().max(right.abs()).max(f64::MIN_POSITIVE)
}

fn validate_tolerance(value: f64) -> Result<(), NjoyPhotonMomentComparisonError> {
    if value.is_finite() && value > 0.0 && value <= 1.0e-3 {
        Ok(())
    } else {
        invalid_comparison("relative print tolerance must be in (0, 1e-3]")
    }
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), NjoyPhotonMomentComparisonError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyPhotonMomentComparisonError> {
    if value.trim().is_empty() {
        invalid_comparison(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyPhotonMomentComparisonError> {
    if value.len() == 64
        && value
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

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyPhotonMomentComparisonError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| NjoyPhotonMomentComparisonError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyPhotonMomentComparisonError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyPhotonMomentComparisonError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_comparison<T>(message: impl Into<String>) -> Result<T, NjoyPhotonMomentComparisonError> {
    Err(NjoyPhotonMomentComparisonError::InvalidComparison(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoyPhotonMomentComparisonError {
    #[error(transparent)]
    Moment(#[from] EndfPhotonMomentError),
    #[error(transparent)]
    Execution(#[from] NjoyExecutionError),
    #[error("independent moment report and execution receipt have different case identities")]
    CaseBindingMismatch,
    #[error("execution receipt has no processor run for {0}")]
    MissingProcessorRun(String),
    #[error("processor report changed after execution verification: {0}")]
    ProcessorReportChanged(String),
    #[error("processor report is not UTF-8 text: {0}")]
    NonUtf8ProcessorReport(PathBuf),
    #[error("NJOY continuum photon diagnostic table could not be parsed uniquely")]
    UnparsedProcessorTable,
    #[error("processor report has no MF=13 moment table for {nuclide} MT={reaction_mt}")]
    MissingProcessorTable { nuclide: String, reaction_mt: u16 },
    #[error(
        "processor and independent reports share no exact File 15 source node for {nuclide} MT={reaction_mt}"
    )]
    NoSharedSourceNode { nuclide: String, reaction_mt: u16 },
    #[error(
        "processor energy {processor_energy_ev} matches multiple File 15 source nodes for {nuclide} MT={reaction_mt}"
    )]
    AmbiguousSourceNode {
        nuclide: String,
        reaction_mt: u16,
        processor_energy_ev: f64,
    },
    #[error("invalid NJOY photon-moment comparison: {0}")]
    InvalidComparison(String),
    #[error("stored NJOY photon-moment comparison does not match regenerated evidence")]
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

    const BASELINE_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/endfb81-vs-njoy2016-78-continuum-photon-moments.json"
    );
    const JEFF40_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-vs-njoy2016-78-continuum-photon-moments.json"
    );

    #[test]
    fn parses_bounded_precision_heatr_table() {
        let text = " photon energy (from xsecs) mf13, mt  4\n\
                    e ebar xsec energy heating\n\
                    1  continuum gammas\n\
                    6.0000E+06 5.2500E+06 1.1522E-01 6.0491E+05 -6.0491E+05\n";
        let tables = parse_processor_tables(text).unwrap();
        assert_eq!(tables[&4].len(), 1);
        assert_eq!(tables[&4][0].energy_release_ev_barns, 604_910.0);
    }

    #[test]
    fn validates_frozen_njoy_print_comparisons() {
        let baseline = NjoyPhotonMomentComparisonDocument::from_bytes(BASELINE_COMPARISON).unwrap();
        let jeff = NjoyPhotonMomentComparisonDocument::from_bytes(JEFF40_COMPARISON).unwrap();
        assert_eq!(
            baseline.sha256,
            "8d0660d519915b5d0dd5ee4ce0fdd8d4973cb51c1f746e0bc0826dab7f5bb809"
        );
        assert_eq!(
            jeff.sha256,
            "c69ae5e033571cc7526fb4c66456370ef596bebb88c451be7bd4a990cd40d555"
        );
        assert_eq!(baseline.comparison.reaction_count, 8);
        assert_eq!(baseline.comparison.independent_sample_count, 92);
        assert_eq!(baseline.comparison.processor_sample_count, 85);
        assert_eq!(baseline.comparison.compared_sample_count, 58);
        assert_eq!(baseline.comparison.uncompared_independent_sample_count, 34);
        assert_eq!(baseline.comparison.skipped_interpolated_sample_count, 27);
        assert_eq!(baseline.comparison.failed_sample_count, 0);
        assert_eq!(
            baseline.comparison.maximum_relative_difference,
            4.827186715582159e-5
        );
        for (baseline_reaction, jeff_reaction) in baseline
            .comparison
            .reactions
            .iter()
            .zip(&jeff.comparison.reactions)
        {
            assert_eq!(baseline_reaction.nuclide, jeff_reaction.nuclide);
            assert_eq!(baseline_reaction.reaction_mt, jeff_reaction.reaction_mt);
            assert_eq!(baseline_reaction.samples, jeff_reaction.samples);
        }
    }
}
