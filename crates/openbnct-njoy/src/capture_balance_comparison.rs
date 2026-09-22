// SPDX-License-Identifier: MIT

//! Receipt-bound comparison of independent MF=6 capture photon moments with
//! NJOY's bounded-precision photon and synthesized-recoil print tables.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    EndfMf6CapturePhotonBalanceError, EndfMf6CapturePhotonBalanceReportDocument,
    EndfMf6CapturePhotonSource, NjoyExecutionArtifact, NjoyExecutionError,
    NjoyExecutionReceiptDocument,
};

pub const NJOY_CAPTURE_PHOTON_MOMENT_COMPARISON_SCHEMA: &str =
    "openbnct.njoy-mf6-capture-photon-moment-comparison/0.1.0";
pub const DEFAULT_NJOY_CAPTURE_PRINT_RELATIVE_TOLERANCE: f64 = 6.0e-5;

const REPORT_ID_SUFFIX: &str = "njoy2016-78-capture-print-comparison";
const PHOTON_PARTICLE_ID: u32 = 0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyCapturePhotonMomentComparison {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyCapturePhotonMomentComparisonQualification,
    pub independent_balance_report: ContentReference,
    pub execution_receipt: ContentReference,
    pub relative_tolerance: f64,
    pub nuclide: String,
    pub reaction_mt: u16,
    pub processor_report: NjoyExecutionArtifact,
    pub processor_recoil_particle_id: u32,
    pub independent_sample_count: u64,
    pub processor_photon_sample_count: u64,
    pub processor_recoil_sample_count: u64,
    pub samples: Vec<NjoyCapturePhotonMomentComparisonSample>,
    pub compared_sample_count: u64,
    pub uncompared_independent_sample_count: u64,
    pub skipped_processor_sample_count: u64,
    pub failed_sample_count: u64,
    pub maximum_relative_difference: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyCapturePhotonMomentComparisonQualification {
    IndependentCaptureMomentsMatchProcessorPrintUnreviewed,
    ProcessorCapturePrintMismatchRejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyCapturePhotonMomentComparisonSample {
    pub incident_energy_ev: f64,
    pub independent_raw_mean_photon_energy_ev: f64,
    pub processor_mean_photon_energy_ev: f64,
    pub mean_photon_energy_relative_difference: f64,
    pub independent_photon_yield: f64,
    pub processor_photon_yield: f64,
    pub photon_yield_relative_difference: f64,
    pub independent_photon_momentum_recoil_ev: f64,
    pub processor_photon_momentum_recoil_ev: f64,
    pub photon_momentum_recoil_relative_difference: f64,
    pub processor_photon_heating_ev_barns: f64,
    pub status: NjoyCapturePhotonMomentComparisonStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyCapturePhotonMomentComparisonStatus {
    WithinPrintPrecision,
    OutsidePrintPrecision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyCapturePhotonMomentComparisonDocument {
    pub comparison: NjoyCapturePhotonMomentComparison,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyCapturePhotonMomentComparisonResult {
    pub comparison: NjoyCapturePhotonMomentComparison,
    pub comparison_path: PathBuf,
    pub comparison_sha256: String,
}

impl NjoyCapturePhotonMomentComparison {
    pub fn compare(
        balance: &EndfMf6CapturePhotonBalanceReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        relative_tolerance: f64,
    ) -> Result<Self, NjoyCapturePhotonMomentComparisonError> {
        balance.report.validate()?;
        validate_tolerance(relative_tolerance)?;
        execution.verify_execution_root(execution_root)?;
        if balance.report.case_id != execution.receipt.case_id {
            return Err(NjoyCapturePhotonMomentComparisonError::CaseBindingMismatch);
        }
        if balance.report.photon_source
            != EndfMf6CapturePhotonSource::File6Law1PhotonWithoutExplicitRecoil
            || balance.report.samples.is_empty()
        {
            return Err(NjoyCapturePhotonMomentComparisonError::MissingIndependentMoments);
        }
        let run = execution
            .receipt
            .runs
            .iter()
            .find(|run| run.nuclide == balance.report.nuclide)
            .ok_or_else(|| {
                NjoyCapturePhotonMomentComparisonError::MissingProcessorRun(
                    balance.report.nuclide.clone(),
                )
            })?;
        let path = execution_root.join(&run.processor_report.path);
        let bytes = read_regular_file(&path)?;
        if bytes.len() as u64 != run.processor_report.size_bytes
            || sha256_bytes(&bytes) != run.processor_report.sha256
        {
            return Err(
                NjoyCapturePhotonMomentComparisonError::ProcessorReportChanged(
                    run.processor_report.path.clone(),
                ),
            );
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| {
            NjoyCapturePhotonMomentComparisonError::NonUtf8ProcessorReport(path.clone())
        })?;
        let tables = parse_processor_tables(text, balance.report.reaction_mt)?;
        let photon = tables
            .iter()
            .find(|table| table.particle_id == PHOTON_PARTICLE_ID)
            .ok_or(NjoyCapturePhotonMomentComparisonError::MissingPhotonTable)?;
        let mut recoils = tables
            .iter()
            .filter(|table| table.particle_id != PHOTON_PARTICLE_ID);
        let recoil = recoils
            .next()
            .ok_or(NjoyCapturePhotonMomentComparisonError::MissingRecoilTable)?;
        if recoils.next().is_some() {
            return Err(NjoyCapturePhotonMomentComparisonError::AmbiguousRecoilTable);
        }
        if photon.rows.len() != recoil.rows.len()
            || photon
                .rows
                .iter()
                .zip(&recoil.rows)
                .any(|(left, right)| left.incident_energy_ev != right.incident_energy_ev)
        {
            return Err(NjoyCapturePhotonMomentComparisonError::ProcessorGridMismatch);
        }

        let mut samples = Vec::new();
        for independent in &balance.report.samples {
            let mut matching = photon.rows.iter().zip(&recoil.rows).filter(|(photon, _)| {
                relative_difference(independent.incident_energy_ev, photon.incident_energy_ev)
                    <= relative_tolerance
            });
            let Some((photon_row, recoil_row)) = matching.next() else {
                continue;
            };
            if matching.next().is_some() {
                return Err(NjoyCapturePhotonMomentComparisonError::AmbiguousSourceNode(
                    independent.incident_energy_ev,
                ));
            }
            if photon_row.heating_ev_barns != 0.0 || recoil_row.yield_value != 1.0 {
                return Err(NjoyCapturePhotonMomentComparisonError::UnexpectedProcessorSemantics);
            }
            let mean_photon_energy_relative_difference = relative_difference(
                independent.raw_first_photon_energy_moment_ev,
                photon_row.mean_energy_ev,
            );
            let photon_yield_relative_difference =
                relative_difference(independent.photon_yield, photon_row.yield_value);
            let photon_momentum_recoil_relative_difference = relative_difference(
                independent.photon_momentum_recoil_ev,
                recoil_row.mean_energy_ev,
            );
            let maximum = mean_photon_energy_relative_difference
                .max(photon_yield_relative_difference)
                .max(photon_momentum_recoil_relative_difference);
            let status = if maximum <= relative_tolerance {
                NjoyCapturePhotonMomentComparisonStatus::WithinPrintPrecision
            } else {
                NjoyCapturePhotonMomentComparisonStatus::OutsidePrintPrecision
            };
            samples.push(NjoyCapturePhotonMomentComparisonSample {
                incident_energy_ev: independent.incident_energy_ev,
                independent_raw_mean_photon_energy_ev: independent
                    .raw_first_photon_energy_moment_ev,
                processor_mean_photon_energy_ev: photon_row.mean_energy_ev,
                mean_photon_energy_relative_difference,
                independent_photon_yield: independent.photon_yield,
                processor_photon_yield: photon_row.yield_value,
                photon_yield_relative_difference,
                independent_photon_momentum_recoil_ev: independent.photon_momentum_recoil_ev,
                processor_photon_momentum_recoil_ev: recoil_row.mean_energy_ev,
                photon_momentum_recoil_relative_difference,
                processor_photon_heating_ev_barns: photon_row.heating_ev_barns,
                status,
            });
        }
        if samples.is_empty() {
            return Err(NjoyCapturePhotonMomentComparisonError::NoSharedSourceNode);
        }
        let failed_sample_count = samples
            .iter()
            .filter(|sample| {
                sample.status == NjoyCapturePhotonMomentComparisonStatus::OutsidePrintPrecision
            })
            .count() as u64;
        let maximum_relative_difference = samples
            .iter()
            .map(sample_maximum_difference)
            .fold(0.0_f64, f64::max);
        let comparison = Self {
            schema_version: NJOY_CAPTURE_PHOTON_MOMENT_COMPARISON_SCHEMA.into(),
            id: format!("{}.{}", balance.report.id, REPORT_ID_SUFFIX),
            case_id: balance.report.case_id.clone(),
            qualification: if failed_sample_count == 0 {
                NjoyCapturePhotonMomentComparisonQualification::
                    IndependentCaptureMomentsMatchProcessorPrintUnreviewed
            } else {
                NjoyCapturePhotonMomentComparisonQualification::
                    ProcessorCapturePrintMismatchRejected
            },
            independent_balance_report: ContentReference {
                id: balance.report.id.clone(),
                sha256: balance.sha256.clone(),
            },
            execution_receipt: ContentReference {
                id: execution.receipt.id.clone(),
                sha256: execution.sha256.clone(),
            },
            relative_tolerance,
            nuclide: balance.report.nuclide.clone(),
            reaction_mt: balance.report.reaction_mt,
            processor_report: run.processor_report.clone(),
            processor_recoil_particle_id: recoil.particle_id,
            independent_sample_count: balance.report.sample_count,
            processor_photon_sample_count: photon.rows.len() as u64,
            processor_recoil_sample_count: recoil.rows.len() as u64,
            compared_sample_count: samples.len() as u64,
            uncompared_independent_sample_count: balance.report.sample_count - samples.len() as u64,
            skipped_processor_sample_count: photon.rows.len() as u64 - samples.len() as u64,
            samples,
            failed_sample_count,
            maximum_relative_difference,
        };
        comparison.validate()?;
        Ok(comparison)
    }

    pub fn validate(&self) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            NJOY_CAPTURE_PHOTON_MOMENT_COMPARISON_SCHEMA,
        ) {
            return invalid_comparison(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier("nuclide", &self.nuclide)?;
        validate_reference(
            "independent_balance_report",
            &self.independent_balance_report,
        )?;
        validate_reference("execution_receipt", &self.execution_receipt)?;
        if self.id
            != format!(
                "{}.{}",
                self.independent_balance_report.id, REPORT_ID_SUFFIX
            )
        {
            return invalid_comparison("comparison ID does not bind the independent report");
        }
        validate_tolerance(self.relative_tolerance)?;
        validate_sha256("processor_report.sha256", &self.processor_report.sha256)?;
        if self.reaction_mt != 102
            || self.processor_recoil_particle_id == PHOTON_PARTICLE_ID
            || self.samples.is_empty()
            || self.processor_photon_sample_count != self.processor_recoil_sample_count
            || self.independent_sample_count
                != self.compared_sample_count + self.uncompared_independent_sample_count
            || self.processor_photon_sample_count
                != self.compared_sample_count + self.skipped_processor_sample_count
            || self.compared_sample_count != self.samples.len() as u64
        {
            return invalid_comparison("comparison counts or identities are inconsistent");
        }

        let mut previous_energy = None;
        for sample in &self.samples {
            let values = [
                sample.incident_energy_ev,
                sample.independent_raw_mean_photon_energy_ev,
                sample.processor_mean_photon_energy_ev,
                sample.mean_photon_energy_relative_difference,
                sample.independent_photon_yield,
                sample.processor_photon_yield,
                sample.photon_yield_relative_difference,
                sample.independent_photon_momentum_recoil_ev,
                sample.processor_photon_momentum_recoil_ev,
                sample.photon_momentum_recoil_relative_difference,
            ];
            if values
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
                || sample.incident_energy_ev == 0.0
                || sample.processor_photon_heating_ev_barns != 0.0
                || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
            {
                return invalid_comparison("invalid or unordered comparison sample");
            }
            previous_energy = Some(sample.incident_energy_ev);
            if !approximately_equal(
                sample.mean_photon_energy_relative_difference,
                relative_difference(
                    sample.independent_raw_mean_photon_energy_ev,
                    sample.processor_mean_photon_energy_ev,
                ),
            ) || !approximately_equal(
                sample.photon_yield_relative_difference,
                relative_difference(
                    sample.independent_photon_yield,
                    sample.processor_photon_yield,
                ),
            ) || !approximately_equal(
                sample.photon_momentum_recoil_relative_difference,
                relative_difference(
                    sample.independent_photon_momentum_recoil_ev,
                    sample.processor_photon_momentum_recoil_ev,
                ),
            ) {
                return invalid_comparison("comparison differences do not regenerate");
            }
            let expected_status = if sample_maximum_difference(sample) <= self.relative_tolerance {
                NjoyCapturePhotonMomentComparisonStatus::WithinPrintPrecision
            } else {
                NjoyCapturePhotonMomentComparisonStatus::OutsidePrintPrecision
            };
            if sample.status != expected_status {
                return invalid_comparison("comparison status is inconsistent");
            }
        }
        let failed_sample_count = self
            .samples
            .iter()
            .filter(|sample| {
                sample.status == NjoyCapturePhotonMomentComparisonStatus::OutsidePrintPrecision
            })
            .count() as u64;
        let maximum_relative_difference = self
            .samples
            .iter()
            .map(sample_maximum_difference)
            .fold(0.0_f64, f64::max);
        if self.failed_sample_count != failed_sample_count
            || !approximately_equal(
                self.maximum_relative_difference,
                maximum_relative_difference,
            )
        {
            return invalid_comparison("comparison aggregates do not match samples");
        }
        let expected_qualification = if failed_sample_count == 0 {
            NjoyCapturePhotonMomentComparisonQualification::
                IndependentCaptureMomentsMatchProcessorPrintUnreviewed
        } else {
            NjoyCapturePhotonMomentComparisonQualification::ProcessorCapturePrintMismatchRejected
        };
        if self.qualification != expected_qualification {
            return invalid_comparison("qualification does not match comparison samples");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<NjoyCapturePhotonMomentComparisonResult, NjoyCapturePhotonMomentComparisonError>
    {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| NjoyCapturePhotonMomentComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| NjoyCapturePhotonMomentComparisonError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(NjoyCapturePhotonMomentComparisonResult {
            comparison: self.clone(),
            comparison_path: path.to_path_buf(),
            comparison_sha256: sha256_bytes(&bytes),
        })
    }
}

impl NjoyCapturePhotonMomentComparisonDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyCapturePhotonMomentComparisonError> {
        let comparison: NjoyCapturePhotonMomentComparison = serde_json::from_slice(bytes)?;
        comparison.validate()?;
        Ok(Self {
            comparison,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyCapturePhotonMomentComparisonError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_evidence(
        &self,
        balance: &EndfMf6CapturePhotonBalanceReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
    ) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
        let observed = NjoyCapturePhotonMomentComparison::compare(
            balance,
            execution,
            execution_root,
            self.comparison.relative_tolerance,
        )?;
        if self.comparison != observed {
            return Err(NjoyCapturePhotonMomentComparisonError::ComparisonMismatch);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PrintedTable {
    particle_id: u32,
    rows: Vec<PrintedRow>,
}

#[derive(Debug, Clone, Copy)]
struct PrintedRow {
    incident_energy_ev: f64,
    mean_energy_ev: f64,
    yield_value: f64,
    heating_ev_barns: f64,
}

fn parse_processor_tables(
    text: &str,
    reaction_mt: u16,
) -> Result<Vec<PrintedTable>, NjoyCapturePhotonMomentComparisonError> {
    let marker = format!("file six heating for mt{reaction_mt}, particle =");
    let mut tables = Vec::new();
    let mut current_particle = None;
    let mut rows = Vec::new();
    for line in text.lines() {
        if let Some(position) = line.find(&marker) {
            finish_table(&mut tables, current_particle.take(), &mut rows)?;
            let particle = line[position + marker.len()..]
                .split_whitespace()
                .next()
                .ok_or(NjoyCapturePhotonMomentComparisonError::UnparsedProcessorTable)?
                .parse()
                .map_err(|_| NjoyCapturePhotonMomentComparisonError::UnparsedProcessorTable)?;
            current_particle = Some(particle);
            continue;
        }
        if current_particle.is_some() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() == 5
                && let Ok(values) = fields
                    .iter()
                    .map(|field| field.parse::<f64>())
                    .collect::<Result<Vec<_>, _>>()
            {
                rows.push(PrintedRow {
                    incident_energy_ev: values[0],
                    mean_energy_ev: values[1],
                    yield_value: values[2],
                    heating_ev_barns: values[4],
                });
                continue;
            }
            if !rows.is_empty() {
                finish_table(&mut tables, current_particle.take(), &mut rows)?;
            }
        }
    }
    finish_table(&mut tables, current_particle, &mut rows)?;
    if tables.is_empty() {
        return Err(NjoyCapturePhotonMomentComparisonError::UnparsedProcessorTable);
    }
    Ok(tables)
}

fn finish_table(
    tables: &mut Vec<PrintedTable>,
    particle_id: Option<u32>,
    rows: &mut Vec<PrintedRow>,
) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
    let Some(particle_id) = particle_id else {
        return Ok(());
    };
    if rows.is_empty()
        || rows.iter().any(|row| {
            !row.incident_energy_ev.is_finite()
                || row.incident_energy_ev <= 0.0
                || !row.mean_energy_ev.is_finite()
                || row.mean_energy_ev < 0.0
                || !row.yield_value.is_finite()
                || row.yield_value < 0.0
                || !row.heating_ev_barns.is_finite()
        })
        || rows
            .windows(2)
            .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
        || tables.iter().any(|table| table.particle_id == particle_id)
    {
        return Err(NjoyCapturePhotonMomentComparisonError::UnparsedProcessorTable);
    }
    tables.push(PrintedTable {
        particle_id,
        rows: std::mem::take(rows),
    });
    Ok(())
}

fn sample_maximum_difference(sample: &NjoyCapturePhotonMomentComparisonSample) -> f64 {
    sample
        .mean_photon_energy_relative_difference
        .max(sample.photon_yield_relative_difference)
        .max(sample.photon_momentum_recoil_relative_difference)
}

fn relative_difference(left: f64, right: f64) -> f64 {
    if left == right {
        0.0
    } else {
        (left - right).abs() / left.abs().max(right.abs()).max(f64::MIN_POSITIVE)
    }
}

fn validate_tolerance(value: f64) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
    if value.is_finite() && value > 0.0 && value <= 1.0e-3 {
        Ok(())
    } else {
        invalid_comparison("relative print tolerance must be in (0, 1e-3]")
    }
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
    if value.trim().is_empty() {
        invalid_comparison(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    value: &str,
) -> Result<(), NjoyCapturePhotonMomentComparisonError> {
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

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyCapturePhotonMomentComparisonError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        NjoyCapturePhotonMomentComparisonError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(NjoyCapturePhotonMomentComparisonError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyCapturePhotonMomentComparisonError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_comparison<T>(
    message: impl Into<String>,
) -> Result<T, NjoyCapturePhotonMomentComparisonError> {
    Err(NjoyCapturePhotonMomentComparisonError::InvalidComparison(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum NjoyCapturePhotonMomentComparisonError {
    #[error(transparent)]
    Balance(#[from] EndfMf6CapturePhotonBalanceError),
    #[error(transparent)]
    Execution(#[from] NjoyExecutionError),
    #[error("independent capture-balance report and execution receipt have different cases")]
    CaseBindingMismatch,
    #[error("independent report contains no MF=6 capture photon moments")]
    MissingIndependentMoments,
    #[error("execution receipt has no processor run for {0}")]
    MissingProcessorRun(String),
    #[error("processor report changed after execution verification: {0}")]
    ProcessorReportChanged(String),
    #[error("processor report is not UTF-8 text: {0}")]
    NonUtf8ProcessorReport(PathBuf),
    #[error("NJOY MF=6 capture diagnostic tables could not be parsed uniquely")]
    UnparsedProcessorTable,
    #[error("processor report has no MF=6 capture photon table")]
    MissingPhotonTable,
    #[error("processor report has no synthesized MF=6 capture recoil table")]
    MissingRecoilTable,
    #[error("processor report has more than one non-photon MF=6 capture table")]
    AmbiguousRecoilTable,
    #[error("processor photon and recoil grids differ")]
    ProcessorGridMismatch,
    #[error("processor and independent reports share no exact source node")]
    NoSharedSourceNode,
    #[error("processor grid ambiguously matches source energy {0}")]
    AmbiguousSourceNode(f64),
    #[error("processor photon or recoil table has unexpected semantics")]
    UnexpectedProcessorSemantics,
    #[error("invalid NJOY MF=6 capture moment comparison: {0}")]
    InvalidComparison(String),
    #[error("stored NJOY MF=6 capture moment comparison does not match regenerated evidence")]
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

    const JEFF40_COMPARISON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-vs-njoy2016-78-mf6-capture-photon-moments.json"
    );

    #[test]
    fn parses_capture_photon_and_recoil_tables() {
        let text = " file six heating for mt102, particle =     0     q = 2.49E+06\n\
                         e ebar yield xsec heating\n\
                   1.0000E-05 2.5613E+05 9.8866E+00 1.2082E-03 0.0000E+00\n\
                   \n\
                   file six heating for mt102, particle =  7016 q = 2.49E+06\n\
                   e ebar yield xsec heating\n\
                   1.0000E-05 2.4010E+01 1.0000E+00 1.2082E-03 2.9010E-02\n";
        let tables = parse_processor_tables(text, 102).unwrap();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].particle_id, 0);
        assert_eq!(tables[1].particle_id, 7016);
        assert_eq!(tables[1].rows[0].mean_energy_ev, 24.01);
    }

    #[test]
    fn validates_frozen_capture_print_comparison() {
        let comparison =
            NjoyCapturePhotonMomentComparisonDocument::from_bytes(JEFF40_COMPARISON).unwrap();
        assert_eq!(
            comparison.sha256,
            "e3b995922e91214d07f708c307c38f19166fe4b51c38e0611c6fcc01d5bdd831"
        );
        assert_eq!(
            comparison.comparison.qualification,
            NjoyCapturePhotonMomentComparisonQualification::
                IndependentCaptureMomentsMatchProcessorPrintUnreviewed
        );
        assert_eq!(comparison.comparison.independent_sample_count, 37);
        assert_eq!(comparison.comparison.processor_photon_sample_count, 52);
        assert_eq!(comparison.comparison.processor_recoil_sample_count, 52);
        assert_eq!(comparison.comparison.compared_sample_count, 23);
        assert_eq!(
            comparison.comparison.uncompared_independent_sample_count,
            14
        );
        assert_eq!(comparison.comparison.skipped_processor_sample_count, 29);
        assert_eq!(comparison.comparison.failed_sample_count, 0);
        assert_eq!(
            comparison.comparison.maximum_relative_difference,
            4.5422371274853783e-5
        );
    }
}
