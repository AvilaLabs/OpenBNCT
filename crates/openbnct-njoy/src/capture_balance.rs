// SPDX-License-Identifier: MIT

//! Independent source-level energy accounting for MF=6/MT=102 photons.
//!
//! The calculation does not call NJOY or consume a PENDF. It integrates the
//! exact File 6 photon distributions, reconstructs the average photon-momentum
//! recoil, and compares those terms with the File 3 capture energy budget.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::photon_inventory::{ParsedSection, parse_evaluation_sections};
use crate::photon_moment::{
    EndfPhotonMomentError, InterpolationRegion, TabulatedFunction, control, find_section,
    parse_tab1, parse_tab2, positive_usize, require_consumed, take_control, take_words, value,
};
use crate::{EndfPhotonInventoryError, EndfPhotonProductionInventoryDocument};

pub const ENDF_MF6_CAPTURE_PHOTON_BALANCE_SCHEMA: &str =
    "openbnct.endf-mf6-capture-photon-balance/0.1.0";
pub const DEFAULT_CAPTURE_ENERGY_BALANCE_RELATIVE_TOLERANCE: f64 = 1.0e-2;

const REPORT_ID_SUFFIX: &str = "endf-mf6-capture-photon-balance";
const CAPTURE_MT: u16 = 102;
const ENDF_NEUTRON_REST_ENERGY_EV: f64 = 939_565_420.525_39;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfMf6CapturePhotonBalanceReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub scope: EndfMf6CapturePhotonBalanceScope,
    pub qualification: EndfMf6CapturePhotonBalanceQualification,
    pub evaluated_source_selection: ContentReference,
    pub photon_production_inventory: ContentReference,
    pub nuclide: String,
    pub endf_mat: u16,
    pub reaction_mt: u16,
    pub source_evaluation_sha256: String,
    pub file3_section_sha256: String,
    pub file6_section_sha256: Option<String>,
    pub photon_source: EndfMf6CapturePhotonSource,
    pub file6_reference_frame: Option<EndfMf6CaptureReferenceFrame>,
    pub recoil_model: EndfMf6CaptureRecoilModel,
    pub target_atomic_weight_ratio: f64,
    pub reaction_q_value_ev: f64,
    pub neutron_rest_energy_ev: f64,
    pub normalization_tolerance: f64,
    pub relative_energy_balance_tolerance: f64,
    pub samples: Vec<EndfMf6CapturePhotonBalanceSample>,
    pub sample_count: u64,
    pub failed_normalization_sample_count: u64,
    pub failed_energy_balance_sample_count: u64,
    pub maximum_absolute_normalization_error: f64,
    pub maximum_absolute_relative_energy_residual: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6CapturePhotonBalanceScope {
    SingleLaw1Lct3PhotonProductWithoutExplicitRecoil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6CapturePhotonBalanceQualification {
    MissingCapturePhotonDataRejected,
    SpectrumNormalizationRejected,
    CapturePhotonEnergyBalanceRejected,
    CapturePhotonEnergyBalanceCheckedUnreviewed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6CapturePhotonSource {
    Missing,
    File6Law1PhotonWithoutExplicitRecoil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6CaptureReferenceFrame {
    Lct3LightParticleCenterOfMass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6CaptureRecoilModel {
    IndependentPhotonMomentumSecondMomentApproximation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfMf6CapturePhotonBalanceSample {
    pub incident_energy_ev: f64,
    pub capture_cross_section_barns: f64,
    pub photon_yield: f64,
    pub distribution_normalization: f64,
    pub absolute_normalization_error: f64,
    pub raw_first_photon_energy_moment_ev: f64,
    pub raw_second_photon_energy_moment_ev2: f64,
    pub normalized_mean_photon_energy_ev: f64,
    pub normalized_mean_square_photon_energy_ev2: f64,
    pub total_photon_energy_ev: f64,
    pub photon_momentum_recoil_ev: f64,
    pub center_of_mass_available_energy_ev: f64,
    pub center_of_mass_accounted_energy_ev: f64,
    pub incident_translation_energy_ev: f64,
    pub laboratory_available_energy_ev: f64,
    pub laboratory_accounted_energy_ev: f64,
    pub signed_energy_residual_ev: f64,
    pub absolute_relative_energy_residual: f64,
    pub signed_energy_residual_ev_barns: f64,
    pub status: EndfMf6CapturePhotonBalanceSampleStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6CapturePhotonBalanceSampleStatus {
    WithinTolerance,
    SpectrumNormalizationOutsideTolerance,
    EnergyBalanceOutsideTolerance,
    SpectrumNormalizationAndEnergyBalanceOutsideTolerance,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfMf6CapturePhotonBalanceReportDocument {
    pub report: EndfMf6CapturePhotonBalanceReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfMf6CapturePhotonBalanceResult {
    pub report: EndfMf6CapturePhotonBalanceReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl EndfMf6CapturePhotonBalanceReport {
    pub fn calculate(
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        inventory: &EndfPhotonProductionInventoryDocument,
        nuclide: &str,
        normalization_tolerance: f64,
        relative_energy_balance_tolerance: f64,
    ) -> Result<Self, EndfMf6CapturePhotonBalanceError> {
        validate_tolerances(normalization_tolerance, relative_energy_balance_tolerance)?;
        inventory.verify_against_selection(selection, evaluations_root)?;

        let artifact = selection
            .selection
            .evaluations
            .iter()
            .find(|artifact| artifact.nuclide == nuclide)
            .ok_or_else(|| EndfMf6CapturePhotonBalanceError::MissingNuclide(nuclide.into()))?;
        let inventory_evaluation = inventory
            .inventory
            .evaluations
            .iter()
            .find(|evaluation| evaluation.nuclide == nuclide)
            .ok_or(EndfMf6CapturePhotonBalanceError::InventoryBindingMismatch)?;
        if artifact.endf_mat != inventory_evaluation.endf_mat
            || artifact.sha256 != inventory_evaluation.source_evaluation.sha256
        {
            return Err(EndfMf6CapturePhotonBalanceError::InventoryBindingMismatch);
        }

        let path = evaluations_root.join(&artifact.extracted_filename);
        let bytes = read_regular_file(&path)?;
        let sections = parse_evaluation_sections(
            &bytes,
            artifact.endf_mat,
            &[(3, CAPTURE_MT), (6, CAPTURE_MT)],
        )?;
        let file3_section = find_section(&sections, 3, CAPTURE_MT)?;
        let file3 = parse_file3_capture(file3_section)?;

        let inventory_reaction = inventory_evaluation
            .reactions
            .iter()
            .find(|reaction| reaction.reaction_mt == CAPTURE_MT);
        let inventory_file6 = inventory_evaluation
            .sections
            .iter()
            .find(|section| section.file_number == 6 && section.reaction_mt == CAPTURE_MT);
        let has_file6_photon =
            inventory_reaction.is_some_and(|reaction| !reaction.file6_photon_products.is_empty());
        let has_legacy_capture_photon = inventory_reaction
            .is_some_and(|reaction| reaction.file12.is_some() || reaction.file13.is_some());

        let (file6_section_sha256, photon_source, file6_reference_frame, samples) =
            if has_file6_photon {
                let summary = inventory_file6
                    .ok_or(EndfMf6CapturePhotonBalanceError::InventoryBindingMismatch)?;
                let section = find_section(&sections, 6, CAPTURE_MT)?;
                if section.sha256 != summary.sha256 {
                    return Err(EndfMf6CapturePhotonBalanceError::InventoryBindingMismatch);
                }
                let parsed = parse_file6_capture(section, nuclide)?;
                if !approximately_equal(
                    parsed.target_atomic_weight_ratio,
                    file3.target_atomic_weight_ratio,
                ) {
                    return Err(EndfMf6CapturePhotonBalanceError::SourceSectionMismatch);
                }
                let samples = calculate_samples(
                    &file3,
                    parsed,
                    normalization_tolerance,
                    relative_energy_balance_tolerance,
                )?;
                (
                    Some(section.sha256.clone()),
                    EndfMf6CapturePhotonSource::File6Law1PhotonWithoutExplicitRecoil,
                    Some(EndfMf6CaptureReferenceFrame::Lct3LightParticleCenterOfMass),
                    samples,
                )
            } else {
                if inventory_file6.is_some()
                    || sections.iter().any(|section| section.file_number == 6)
                {
                    return Err(
                        EndfMf6CapturePhotonBalanceError::UnsupportedRepresentation {
                            nuclide: nuclide.into(),
                            message: "MF=6/MT=102 exists but contains no photon product".into(),
                        },
                    );
                }
                if has_legacy_capture_photon {
                    return Err(
                        EndfMf6CapturePhotonBalanceError::UnsupportedRepresentation {
                            nuclide: nuclide.into(),
                            message: "capture photons use File 12 or File 13 rather than File 6"
                                .into(),
                        },
                    );
                }
                (None, EndfMf6CapturePhotonSource::Missing, None, Vec::new())
            };

        let failed_normalization_sample_count = samples
            .iter()
            .filter(|sample| sample.normalization_failed(normalization_tolerance))
            .count() as u64;
        let failed_energy_balance_sample_count = samples
            .iter()
            .filter(|sample| sample.energy_balance_failed(relative_energy_balance_tolerance))
            .count() as u64;
        let qualification = qualification(
            photon_source,
            failed_normalization_sample_count,
            failed_energy_balance_sample_count,
        );
        let report = Self {
            schema_version: ENDF_MF6_CAPTURE_PHOTON_BALANCE_SCHEMA.into(),
            id: report_id(&selection.selection.id, nuclide),
            case_id: selection.selection.case_id.clone(),
            scope:
                EndfMf6CapturePhotonBalanceScope::SingleLaw1Lct3PhotonProductWithoutExplicitRecoil,
            qualification,
            evaluated_source_selection: ContentReference {
                id: selection.selection.id.clone(),
                sha256: selection.sha256.clone(),
            },
            photon_production_inventory: ContentReference {
                id: inventory.inventory.id.clone(),
                sha256: inventory.sha256.clone(),
            },
            nuclide: nuclide.into(),
            endf_mat: artifact.endf_mat,
            reaction_mt: CAPTURE_MT,
            source_evaluation_sha256: artifact.sha256.clone(),
            file3_section_sha256: file3_section.sha256.clone(),
            file6_section_sha256,
            photon_source,
            file6_reference_frame,
            recoil_model:
                EndfMf6CaptureRecoilModel::IndependentPhotonMomentumSecondMomentApproximation,
            target_atomic_weight_ratio: file3.target_atomic_weight_ratio,
            reaction_q_value_ev: file3.reaction_q_value_ev,
            neutron_rest_energy_ev: ENDF_NEUTRON_REST_ENERGY_EV,
            normalization_tolerance,
            relative_energy_balance_tolerance,
            sample_count: samples.len() as u64,
            failed_normalization_sample_count,
            failed_energy_balance_sample_count,
            maximum_absolute_normalization_error: samples
                .iter()
                .map(|sample| sample.absolute_normalization_error)
                .fold(0.0_f64, f64::max),
            maximum_absolute_relative_energy_residual: samples
                .iter()
                .map(|sample| sample.absolute_relative_energy_residual)
                .fold(0.0_f64, f64::max),
            samples,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), EndfMf6CapturePhotonBalanceError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            ENDF_MF6_CAPTURE_PHOTON_BALANCE_SCHEMA,
        ) {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier("nuclide", &self.nuclide)?;
        validate_reference(
            "evaluated_source_selection",
            &self.evaluated_source_selection,
        )?;
        validate_reference(
            "photon_production_inventory",
            &self.photon_production_inventory,
        )?;
        if self.id != report_id(&self.evaluated_source_selection.id, &self.nuclide) {
            return invalid_report("report ID does not bind the source selection and nuclide");
        }
        if self.endf_mat == 0 || self.reaction_mt != CAPTURE_MT {
            return invalid_report("invalid capture target identity");
        }
        validate_sha256("source_evaluation_sha256", &self.source_evaluation_sha256)?;
        validate_sha256("file3_section_sha256", &self.file3_section_sha256)?;
        if let Some(hash) = &self.file6_section_sha256 {
            validate_sha256("file6_section_sha256", hash)?;
        }
        validate_tolerances(
            self.normalization_tolerance,
            self.relative_energy_balance_tolerance,
        )?;
        if !self.target_atomic_weight_ratio.is_finite()
            || self.target_atomic_weight_ratio <= 0.0
            || !self.reaction_q_value_ev.is_finite()
            || self.reaction_q_value_ev <= 0.0
            || !approximately_equal(self.neutron_rest_energy_ev, ENDF_NEUTRON_REST_ENERGY_EV)
        {
            return invalid_report("invalid capture constants");
        }

        match self.photon_source {
            EndfMf6CapturePhotonSource::Missing => {
                if self.file6_section_sha256.is_some()
                    || self.file6_reference_frame.is_some()
                    || !self.samples.is_empty()
                {
                    return invalid_report("missing photon source has File 6 evidence or samples");
                }
            }
            EndfMf6CapturePhotonSource::File6Law1PhotonWithoutExplicitRecoil => {
                if self.file6_section_sha256.is_none()
                    || self.file6_reference_frame
                        != Some(EndfMf6CaptureReferenceFrame::Lct3LightParticleCenterOfMass)
                    || self.samples.is_empty()
                {
                    return invalid_report("File 6 photon source lacks evidence or samples");
                }
            }
        }

        let mut previous_energy = None;
        for sample in &self.samples {
            validate_sample(self, sample, previous_energy)?;
            previous_energy = Some(sample.incident_energy_ev);
        }

        let sample_count = self.samples.len() as u64;
        let failed_normalization_sample_count = self
            .samples
            .iter()
            .filter(|sample| sample.normalization_failed(self.normalization_tolerance))
            .count() as u64;
        let failed_energy_balance_sample_count = self
            .samples
            .iter()
            .filter(|sample| sample.energy_balance_failed(self.relative_energy_balance_tolerance))
            .count() as u64;
        let maximum_absolute_normalization_error = self
            .samples
            .iter()
            .map(|sample| sample.absolute_normalization_error)
            .fold(0.0_f64, f64::max);
        let maximum_absolute_relative_energy_residual = self
            .samples
            .iter()
            .map(|sample| sample.absolute_relative_energy_residual)
            .fold(0.0_f64, f64::max);
        if self.sample_count != sample_count
            || self.failed_normalization_sample_count != failed_normalization_sample_count
            || self.failed_energy_balance_sample_count != failed_energy_balance_sample_count
            || !approximately_equal(
                self.maximum_absolute_normalization_error,
                maximum_absolute_normalization_error,
            )
            || !approximately_equal(
                self.maximum_absolute_relative_energy_residual,
                maximum_absolute_relative_energy_residual,
            )
        {
            return invalid_report("report aggregates do not match its samples");
        }
        let expected_qualification = qualification(
            self.photon_source,
            failed_normalization_sample_count,
            failed_energy_balance_sample_count,
        );
        if self.qualification != expected_qualification {
            return invalid_report("qualification does not match the source and sample results");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<EndfMf6CapturePhotonBalanceResult, EndfMf6CapturePhotonBalanceError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| EndfMf6CapturePhotonBalanceError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| EndfMf6CapturePhotonBalanceError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(EndfMf6CapturePhotonBalanceResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl EndfMf6CapturePhotonBalanceReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EndfMf6CapturePhotonBalanceError> {
        let report: EndfMf6CapturePhotonBalanceReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, EndfMf6CapturePhotonBalanceError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_sources(
        &self,
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        inventory: &EndfPhotonProductionInventoryDocument,
    ) -> Result<(), EndfMf6CapturePhotonBalanceError> {
        let observed = EndfMf6CapturePhotonBalanceReport::calculate(
            selection,
            evaluations_root,
            inventory,
            &self.report.nuclide,
            self.report.normalization_tolerance,
            self.report.relative_energy_balance_tolerance,
        )?;
        if self.report != observed {
            return Err(EndfMf6CapturePhotonBalanceError::ReportMismatch);
        }
        Ok(())
    }
}

impl EndfMf6CapturePhotonBalanceSample {
    fn normalization_failed(&self, tolerance: f64) -> bool {
        self.absolute_normalization_error > tolerance
    }

    fn energy_balance_failed(&self, tolerance: f64) -> bool {
        self.absolute_relative_energy_residual > tolerance
    }
}

struct File3Capture {
    target_atomic_weight_ratio: f64,
    reaction_q_value_ev: f64,
    cross_section: TabulatedFunction,
}

struct File6Capture {
    target_atomic_weight_ratio: f64,
    yield_function: TabulatedFunction,
    distributions: Vec<File6PhotonDistribution>,
}

struct File6PhotonDistribution {
    incident_energy_ev: f64,
    normalization: f64,
    first_moment_ev: f64,
    second_moment_ev2: f64,
}

fn parse_file3_capture(
    section: &ParsedSection,
) -> Result<File3Capture, EndfMf6CapturePhotonBalanceError> {
    let head = control(section, 0)?;
    let target_atomic_weight_ratio = value(head, 1)?;
    let mut cursor = 1_usize;
    let (reaction, cross_section) = parse_tab1(section, &mut cursor)?;
    require_consumed(section, cursor)?;
    let reaction_q_value_ev = value(reaction, 1)?;
    if target_atomic_weight_ratio <= 0.0 || reaction_q_value_ev <= 0.0 {
        return Err(EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue);
    }
    Ok(File3Capture {
        target_atomic_weight_ratio,
        reaction_q_value_ev,
        cross_section,
    })
}

fn parse_file6_capture(
    section: &ParsedSection,
    nuclide: &str,
) -> Result<File6Capture, EndfMf6CapturePhotonBalanceError> {
    let head = control(section, 0)?;
    let target_atomic_weight_ratio = value(head, 1)?;
    if head.l2 != 3 {
        return unsupported(
            nuclide,
            format!(
                "MF=6/MT=102 uses LCT={}; this center-of-mass balance requires LCT=3",
                head.l2
            ),
        );
    }
    if head.n1 != 1 {
        return unsupported(
            nuclide,
            format!(
                "MF=6/MT=102 has {} products; exactly one is supported",
                head.n1
            ),
        );
    }
    let mut cursor = 1_usize;
    let (product, yield_function) = parse_tab1(section, &mut cursor)?;
    let emitted_particle = value(product, 0)?;
    let emitted_particle_mass_ratio = value(product, 1)?;
    if emitted_particle != 0.0
        || emitted_particle_mass_ratio != 0.0
        || product.l1 != 0
        || product.l2 != 1
    {
        return unsupported(
            nuclide,
            "MF=6/MT=102 is not a single LAW=1 photon product without an explicit recoil",
        );
    }
    let (distribution_head, _) = parse_tab2(section, &mut cursor)?;
    if distribution_head.l1 != 1 || distribution_head.l2 != 1 {
        return unsupported(
            nuclide,
            format!(
                "MF=6/MT=102 uses LANG={} LEP={}; only LANG=1 LEP=1 is supported",
                distribution_head.l1, distribution_head.l2
            ),
        );
    }
    let incident_count = positive_usize(distribution_head.n2)?;
    let mut distributions = Vec::with_capacity(incident_count);
    for _ in 0..incident_count {
        let list = take_control(section, &mut cursor)?;
        let incident_energy_ev = value(list, 1)?;
        let discrete_count = nonnegative_usize(list.l1)?;
        let angular_parameter_count = nonnegative_usize(list.l2)?;
        let word_count = positive_usize(list.n1)?;
        let outgoing_energy_count = positive_usize(list.n2)?;
        let expected_word_count = outgoing_energy_count
            .checked_mul(2)
            .ok_or(EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue)?;
        if angular_parameter_count != 0
            || discrete_count > outgoing_energy_count
            || word_count != expected_word_count
        {
            return unsupported(
                nuclide,
                "MF=6/MT=102 LIST is not an isotropic two-column LAW=1 distribution",
            );
        }
        let words = take_words(section, &mut cursor, word_count)?;
        let points = words
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .collect::<Vec<_>>();
        if incident_energy_ev <= 0.0
            || points
                .iter()
                .any(|(energy, density)| *energy < 0.0 || *density < 0.0)
        {
            return Err(EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue);
        }

        let mut normalization = 0.0;
        let mut first_moment_ev = 0.0;
        let mut second_moment_ev2 = 0.0;
        for (energy, probability) in &points[..discrete_count] {
            normalization += probability;
            first_moment_ev += energy * probability;
            second_moment_ev2 += energy * energy * probability;
        }
        let continuum = &points[discrete_count..];
        if !continuum.is_empty() {
            if continuum.len() < 2 {
                return unsupported(
                    nuclide,
                    "MF=6/MT=102 continuum has fewer than two outgoing-energy points",
                );
            }
            let continuum_function = TabulatedFunction {
                interpolation: vec![InterpolationRegion {
                    upper_point_index: continuum.len(),
                    law: distribution_head.l2,
                }],
                points: continuum.to_vec(),
            };
            let (continuum_norm, continuum_first, continuum_second) =
                continuum_function.integrate_moments()?;
            normalization += continuum_norm;
            first_moment_ev += continuum_first;
            second_moment_ev2 += continuum_second;
        }
        if normalization <= 0.0 || first_moment_ev < 0.0 || second_moment_ev2 < 0.0 {
            return Err(EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue);
        }
        distributions.push(File6PhotonDistribution {
            incident_energy_ev,
            normalization,
            first_moment_ev,
            second_moment_ev2,
        });
    }
    require_consumed(section, cursor)?;
    if distributions
        .windows(2)
        .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue);
    }
    Ok(File6Capture {
        target_atomic_weight_ratio,
        yield_function,
        distributions,
    })
}

fn calculate_samples(
    file3: &File3Capture,
    file6: File6Capture,
    normalization_tolerance: f64,
    relative_energy_balance_tolerance: f64,
) -> Result<Vec<EndfMf6CapturePhotonBalanceSample>, EndfMf6CapturePhotonBalanceError> {
    let residual_rest_energy_ev =
        (file3.target_atomic_weight_ratio + 1.0) * ENDF_NEUTRON_REST_ENERGY_EV;
    let mut samples = Vec::with_capacity(file6.distributions.len());
    for distribution in file6.distributions {
        let incident_energy_ev = distribution.incident_energy_ev;
        let capture_cross_section_barns = file3.cross_section.evaluate(incident_energy_ev)?;
        let photon_yield = file6.yield_function.evaluate(incident_energy_ev)?;
        if capture_cross_section_barns < 0.0 || photon_yield < 0.0 {
            return Err(EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue);
        }
        let normalized_mean_photon_energy_ev =
            distribution.first_moment_ev / distribution.normalization;
        let normalized_mean_square_photon_energy_ev2 =
            distribution.second_moment_ev2 / distribution.normalization;
        let total_photon_energy_ev = photon_yield * normalized_mean_photon_energy_ev;
        let photon_momentum_recoil_ev = photon_yield * normalized_mean_square_photon_energy_ev2
            / (2.0 * residual_rest_energy_ev);
        let incident_translation_energy_ev =
            incident_energy_ev / (file3.target_atomic_weight_ratio + 1.0);
        let center_of_mass_available_energy_ev =
            file3.reaction_q_value_ev + incident_energy_ev - incident_translation_energy_ev;
        let center_of_mass_accounted_energy_ev = total_photon_energy_ev + photon_momentum_recoil_ev;
        let laboratory_available_energy_ev = file3.reaction_q_value_ev + incident_energy_ev;
        let laboratory_accounted_energy_ev =
            center_of_mass_accounted_energy_ev + incident_translation_energy_ev;
        let signed_energy_residual_ev =
            center_of_mass_available_energy_ev - center_of_mass_accounted_energy_ev;
        let absolute_relative_energy_residual =
            signed_energy_residual_ev.abs() / center_of_mass_available_energy_ev;
        let absolute_normalization_error = (distribution.normalization - 1.0).abs();
        let normalization_failed = absolute_normalization_error > normalization_tolerance;
        let energy_failed = absolute_relative_energy_residual > relative_energy_balance_tolerance;
        let status = match (normalization_failed, energy_failed) {
            (false, false) => EndfMf6CapturePhotonBalanceSampleStatus::WithinTolerance,
            (true, false) => {
                EndfMf6CapturePhotonBalanceSampleStatus::SpectrumNormalizationOutsideTolerance
            }
            (false, true) => {
                EndfMf6CapturePhotonBalanceSampleStatus::EnergyBalanceOutsideTolerance
            }
            (true, true) => EndfMf6CapturePhotonBalanceSampleStatus::
                SpectrumNormalizationAndEnergyBalanceOutsideTolerance,
        };
        samples.push(EndfMf6CapturePhotonBalanceSample {
            incident_energy_ev,
            capture_cross_section_barns,
            photon_yield,
            distribution_normalization: distribution.normalization,
            absolute_normalization_error,
            raw_first_photon_energy_moment_ev: distribution.first_moment_ev,
            raw_second_photon_energy_moment_ev2: distribution.second_moment_ev2,
            normalized_mean_photon_energy_ev,
            normalized_mean_square_photon_energy_ev2,
            total_photon_energy_ev,
            photon_momentum_recoil_ev,
            center_of_mass_available_energy_ev,
            center_of_mass_accounted_energy_ev,
            incident_translation_energy_ev,
            laboratory_available_energy_ev,
            laboratory_accounted_energy_ev,
            signed_energy_residual_ev,
            absolute_relative_energy_residual,
            signed_energy_residual_ev_barns: signed_energy_residual_ev
                * capture_cross_section_barns,
            status,
        });
    }
    Ok(samples)
}

fn validate_sample(
    report: &EndfMf6CapturePhotonBalanceReport,
    sample: &EndfMf6CapturePhotonBalanceSample,
    previous_energy: Option<f64>,
) -> Result<(), EndfMf6CapturePhotonBalanceError> {
    let nonnegative = [
        sample.incident_energy_ev,
        sample.capture_cross_section_barns,
        sample.photon_yield,
        sample.distribution_normalization,
        sample.absolute_normalization_error,
        sample.raw_first_photon_energy_moment_ev,
        sample.raw_second_photon_energy_moment_ev2,
        sample.normalized_mean_photon_energy_ev,
        sample.normalized_mean_square_photon_energy_ev2,
        sample.total_photon_energy_ev,
        sample.photon_momentum_recoil_ev,
        sample.center_of_mass_available_energy_ev,
        sample.center_of_mass_accounted_energy_ev,
        sample.incident_translation_energy_ev,
        sample.laboratory_available_energy_ev,
        sample.laboratory_accounted_energy_ev,
        sample.absolute_relative_energy_residual,
    ];
    if nonnegative
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
        || sample.incident_energy_ev == 0.0
        || sample.distribution_normalization == 0.0
        || !sample.signed_energy_residual_ev.is_finite()
        || !sample.signed_energy_residual_ev_barns.is_finite()
        || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
    {
        return invalid_report("invalid or unordered capture-balance sample");
    }
    let residual_rest_energy_ev =
        (report.target_atomic_weight_ratio + 1.0) * report.neutron_rest_energy_ev;
    let expected_translation =
        sample.incident_energy_ev / (report.target_atomic_weight_ratio + 1.0);
    let expected_center_available =
        report.reaction_q_value_ev + sample.incident_energy_ev - expected_translation;
    let expected_recoil = sample.photon_yield * sample.normalized_mean_square_photon_energy_ev2
        / (2.0 * residual_rest_energy_ev);
    let expected_center_accounted = sample.total_photon_energy_ev + expected_recoil;
    let expected_residual = expected_center_available - expected_center_accounted;
    if !approximately_equal(
        sample.absolute_normalization_error,
        (sample.distribution_normalization - 1.0).abs(),
    ) || !approximately_equal(
        sample.normalized_mean_photon_energy_ev,
        sample.raw_first_photon_energy_moment_ev / sample.distribution_normalization,
    ) || !approximately_equal(
        sample.normalized_mean_square_photon_energy_ev2,
        sample.raw_second_photon_energy_moment_ev2 / sample.distribution_normalization,
    ) || !approximately_equal(
        sample.total_photon_energy_ev,
        sample.photon_yield * sample.normalized_mean_photon_energy_ev,
    ) || !approximately_equal(sample.photon_momentum_recoil_ev, expected_recoil)
        || !approximately_equal(sample.incident_translation_energy_ev, expected_translation)
        || !approximately_equal(
            sample.center_of_mass_available_energy_ev,
            expected_center_available,
        )
        || !approximately_equal(
            sample.center_of_mass_accounted_energy_ev,
            expected_center_accounted,
        )
        || !approximately_equal(
            sample.laboratory_available_energy_ev,
            report.reaction_q_value_ev + sample.incident_energy_ev,
        )
        || !approximately_equal(
            sample.laboratory_accounted_energy_ev,
            expected_center_accounted + expected_translation,
        )
        || !approximately_equal(sample.signed_energy_residual_ev, expected_residual)
        || !approximately_equal(
            sample.absolute_relative_energy_residual,
            expected_residual.abs() / expected_center_available,
        )
        || !approximately_equal(
            sample.signed_energy_residual_ev_barns,
            expected_residual * sample.capture_cross_section_barns,
        )
    {
        return invalid_report("capture-balance sample derived values do not close");
    }
    let expected_status = match (
        sample.normalization_failed(report.normalization_tolerance),
        sample.energy_balance_failed(report.relative_energy_balance_tolerance),
    ) {
        (false, false) => EndfMf6CapturePhotonBalanceSampleStatus::WithinTolerance,
        (true, false) => {
            EndfMf6CapturePhotonBalanceSampleStatus::SpectrumNormalizationOutsideTolerance
        }
        (false, true) => {
            EndfMf6CapturePhotonBalanceSampleStatus::EnergyBalanceOutsideTolerance
        }
        (true, true) => EndfMf6CapturePhotonBalanceSampleStatus::
            SpectrumNormalizationAndEnergyBalanceOutsideTolerance,
    };
    if sample.status != expected_status {
        return invalid_report("capture-balance sample status does not match its errors");
    }
    Ok(())
}

fn qualification(
    source: EndfMf6CapturePhotonSource,
    failed_normalization_sample_count: u64,
    failed_energy_balance_sample_count: u64,
) -> EndfMf6CapturePhotonBalanceQualification {
    if source == EndfMf6CapturePhotonSource::Missing {
        EndfMf6CapturePhotonBalanceQualification::MissingCapturePhotonDataRejected
    } else if failed_normalization_sample_count > 0 {
        EndfMf6CapturePhotonBalanceQualification::SpectrumNormalizationRejected
    } else if failed_energy_balance_sample_count > 0 {
        EndfMf6CapturePhotonBalanceQualification::CapturePhotonEnergyBalanceRejected
    } else {
        EndfMf6CapturePhotonBalanceQualification::CapturePhotonEnergyBalanceCheckedUnreviewed
    }
}

fn nonnegative_usize(value: i64) -> Result<usize, EndfMf6CapturePhotonBalanceError> {
    usize::try_from(value).map_err(|_| EndfMf6CapturePhotonBalanceError::InvalidPhysicalValue)
}

fn validate_tolerances(
    normalization_tolerance: f64,
    relative_energy_balance_tolerance: f64,
) -> Result<(), EndfMf6CapturePhotonBalanceError> {
    if !normalization_tolerance.is_finite()
        || normalization_tolerance <= 0.0
        || normalization_tolerance > 1.0e-2
    {
        return invalid_report("normalization tolerance must be in (0, 1e-2]");
    }
    if !relative_energy_balance_tolerance.is_finite()
        || relative_energy_balance_tolerance <= 0.0
        || relative_energy_balance_tolerance > 1.0e-1
    {
        return invalid_report("relative energy-balance tolerance must be in (0, 1e-1]");
    }
    Ok(())
}

fn report_id(selection_id: &str, nuclide: &str) -> String {
    format!(
        "{selection_id}.{REPORT_ID_SUFFIX}.{}.mt{CAPTURE_MT}",
        nuclide.to_ascii_lowercase()
    )
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), EndfMf6CapturePhotonBalanceError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), EndfMf6CapturePhotonBalanceError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    value: &str,
) -> Result<(), EndfMf6CapturePhotonBalanceError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_report(format!("{label} is not a lowercase SHA-256 digest"))
    }
}

fn approximately_equal(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= 1.0e-12 * scale
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, EndfMf6CapturePhotonBalanceError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| EndfMf6CapturePhotonBalanceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(EndfMf6CapturePhotonBalanceError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| EndfMf6CapturePhotonBalanceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn unsupported<T>(
    nuclide: &str,
    message: impl Into<String>,
) -> Result<T, EndfMf6CapturePhotonBalanceError> {
    Err(
        EndfMf6CapturePhotonBalanceError::UnsupportedRepresentation {
            nuclide: nuclide.into(),
            message: message.into(),
        },
    )
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, EndfMf6CapturePhotonBalanceError> {
    Err(EndfMf6CapturePhotonBalanceError::InvalidReport(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum EndfMf6CapturePhotonBalanceError {
    #[error(transparent)]
    EvaluatedSource(#[from] EvaluatedSourceError),
    #[error(transparent)]
    PhotonInventory(#[from] EndfPhotonInventoryError),
    #[error(transparent)]
    PhotonMoment(#[from] EndfPhotonMomentError),
    #[error("nuclide {0} is not present in the evaluated-source selection")]
    MissingNuclide(String),
    #[error("capture-balance inventory does not bind the exact source evaluation")]
    InventoryBindingMismatch,
    #[error("MF=3 and MF=6 capture sections disagree on target identity")]
    SourceSectionMismatch,
    #[error("unsupported MF=6 capture representation for {nuclide}: {message}")]
    UnsupportedRepresentation { nuclide: String, message: String },
    #[error("invalid physical value in MF=3 or MF=6 capture data")]
    InvalidPhysicalValue,
    #[error("invalid MF=6 capture photon-balance report: {0}")]
    InvalidReport(String),
    #[error("stored MF=6 capture photon-balance report does not match regenerated source evidence")]
    ReportMismatch,
    #[error("required capture-balance artifact is not a regular file: {0}")]
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
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/endfb81-mf6-mt102-capture-photon-balance.json"
    );
    const JEFF40_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-mf6-mt102-capture-photon-balance.json"
    );

    #[test]
    fn validates_frozen_capture_balance_evidence() {
        let baseline =
            EndfMf6CapturePhotonBalanceReportDocument::from_bytes(BASELINE_REPORT).unwrap();
        let jeff = EndfMf6CapturePhotonBalanceReportDocument::from_bytes(JEFF40_REPORT).unwrap();
        assert_eq!(
            baseline.sha256,
            "2f8a5b6bdf057d110ce4e28987d5c6850df01fb52c7e088a49e6c61938e05858"
        );
        assert_eq!(
            jeff.sha256,
            "306a0d893f7ea8e3b5490a7cc6f5556a6de523e0171bb98dc23571bec1febbce"
        );
        assert_eq!(
            baseline.report.qualification,
            EndfMf6CapturePhotonBalanceQualification::MissingCapturePhotonDataRejected
        );
        assert_eq!(baseline.report.sample_count, 0);
        assert_eq!(
            jeff.report.qualification,
            EndfMf6CapturePhotonBalanceQualification::CapturePhotonEnergyBalanceRejected
        );
        assert_eq!(jeff.report.sample_count, 37);
        assert_eq!(jeff.report.failed_normalization_sample_count, 0);
        assert_eq!(jeff.report.failed_energy_balance_sample_count, 33);
        assert_eq!(
            jeff.report.maximum_absolute_normalization_error,
            3.9270394447399326e-7
        );
        assert_eq!(
            jeff.report.maximum_absolute_relative_energy_residual,
            0.05751207778410636
        );

        let thermal = &jeff.report.samples[0];
        assert_eq!(thermal.incident_energy_ev, 1.0e-5);
        assert_eq!(thermal.normalized_mean_photon_energy_ev, 256_132.7777129009);
        assert_eq!(thermal.photon_momentum_recoil_ev, 24.010345562585304);
        assert_eq!(thermal.signed_energy_residual_ev, -42_304.025277558714);
        let twenty_mev = jeff.report.samples.last().unwrap();
        assert_eq!(twenty_mev.incident_energy_ev, 2.0e7);
        assert_eq!(twenty_mev.photon_momentum_recoil_ev, 2_215.6567900539885);
        assert_eq!(twenty_mev.signed_energy_residual_ev, 1_220_972.2071049511);
    }
}
