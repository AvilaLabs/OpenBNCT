// SPDX-License-Identifier: MIT

//! Independent ENDF File 13/File 15 continuum photon-energy moments.
//!
//! This calculator does not call NJOY or consume a PENDF. It integrates the
//! source File 15 spectrum, evaluates the matching File 13 production cross
//! section, and forms the photon energy-removal term `Ebar * sigma_gamma`.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::photon_inventory::{EndfRecord, ParsedSection, parse_evaluation};
use crate::{EndfPhotonInventoryError, EndfPhotonProductionInventoryDocument};

pub const ENDF_CONTINUUM_PHOTON_MOMENT_SCHEMA: &str =
    "openbnct.endf-continuum-photon-energy-moment/0.1.0";
pub const DEFAULT_SPECTRUM_NORMALIZATION_TOLERANCE: f64 = 1.0e-4;

const REPORT_ID_SUFFIX: &str = "endf-continuum-photon-energy-moments";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfContinuumPhotonMomentReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub scope: EndfPhotonMomentScope,
    pub qualification: EndfPhotonMomentQualification,
    pub evaluated_source_selection: ContentReference,
    pub photon_production_inventory: ContentReference,
    pub normalization_tolerance: f64,
    pub reactions: Vec<EndfContinuumPhotonMomentReaction>,
    pub reaction_count: u64,
    pub sample_count: u64,
    pub failed_sample_count: u64,
    pub maximum_absolute_normalization_error: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfPhotonMomentScope {
    SingleComponentFile13ContinuumWithFile15,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfPhotonMomentQualification {
    SourceMomentsCheckedUnreviewed,
    SpectrumNormalizationRejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfContinuumPhotonMomentReaction {
    pub nuclide: String,
    pub endf_mat: u16,
    pub reaction_mt: u16,
    pub file13_section_sha256: String,
    pub file15_section_sha256: String,
    pub samples: Vec<EndfContinuumPhotonMomentSample>,
    pub failed_sample_count: u64,
    pub maximum_absolute_normalization_error: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfContinuumPhotonMomentSample {
    pub incident_energy_ev: f64,
    pub component_probability: f64,
    pub spectrum_integral: f64,
    pub weighted_normalization: f64,
    pub absolute_normalization_error: f64,
    pub mean_photon_energy_ev: f64,
    pub continuum_cross_section_barns: f64,
    pub photon_energy_release_ev_barns: f64,
    pub status: EndfPhotonMomentSampleStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfPhotonMomentSampleStatus {
    WithinTolerance,
    OutsideTolerance,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfContinuumPhotonMomentReportDocument {
    pub report: EndfContinuumPhotonMomentReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfContinuumPhotonMomentResult {
    pub report: EndfContinuumPhotonMomentReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl EndfContinuumPhotonMomentReport {
    pub fn calculate(
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        inventory: &EndfPhotonProductionInventoryDocument,
        normalization_tolerance: f64,
    ) -> Result<Self, EndfPhotonMomentError> {
        validate_tolerance(normalization_tolerance)?;
        inventory.verify_against_selection(selection, evaluations_root)?;

        let mut reactions = Vec::new();
        for (artifact, inventory_evaluation) in selection
            .selection
            .evaluations
            .iter()
            .zip(&inventory.inventory.evaluations)
        {
            if artifact.nuclide != inventory_evaluation.nuclide
                || artifact.endf_mat != inventory_evaluation.endf_mat
            {
                return Err(EndfPhotonMomentError::InventoryBindingMismatch);
            }
            let path = evaluations_root.join(&artifact.extracted_filename);
            let bytes = read_regular_file(&path)?;
            let sections = parse_evaluation(&bytes, artifact.endf_mat)?;

            for reaction in &inventory_evaluation.reactions {
                let Some(file13_summary) = inventory_evaluation.sections.iter().find(|section| {
                    section.file_number == 13 && section.reaction_mt == reaction.reaction_mt
                }) else {
                    continue;
                };
                let Some(file13) = reaction.file13.as_ref() else {
                    continue;
                };
                if file13.continuum_subsection_count == 0 {
                    continue;
                }
                if file13.continuum_subsection_count != 1 {
                    return Err(EndfPhotonMomentError::UnsupportedRepresentation {
                        nuclide: artifact.nuclide.clone(),
                        reaction_mt: reaction.reaction_mt,
                        message: "more than one File 13 continuum subsection".into(),
                    });
                }
                let file15_summary = inventory_evaluation
                    .sections
                    .iter()
                    .find(|section| {
                        section.file_number == 15 && section.reaction_mt == reaction.reaction_mt
                    })
                    .ok_or_else(|| EndfPhotonMomentError::UnsupportedRepresentation {
                        nuclide: artifact.nuclide.clone(),
                        reaction_mt: reaction.reaction_mt,
                        message: "File 13 continuum has no File 15 section".into(),
                    })?;
                let file13_section = find_section(&sections, 13, reaction.reaction_mt)?;
                let file15_section = find_section(&sections, 15, reaction.reaction_mt)?;
                if file13_section.sha256 != file13_summary.sha256
                    || file15_section.sha256 != file15_summary.sha256
                {
                    return Err(EndfPhotonMomentError::InventoryBindingMismatch);
                }

                let cross_section = parse_file13_continuum(file13_section)?;
                let spectra = parse_single_component_file15(file15_section)?;
                let mut samples = Vec::with_capacity(spectra.distributions.len());
                for distribution in spectra.distributions {
                    let component_probability = spectra
                        .component_probability
                        .evaluate(distribution.incident_energy_ev)?;
                    let continuum_cross_section_barns =
                        cross_section.evaluate(distribution.incident_energy_ev)?;
                    if component_probability < 0.0 || continuum_cross_section_barns < 0.0 {
                        return Err(EndfPhotonMomentError::NegativePhysicalValue {
                            file_number: file15_section.file_number,
                            reaction_mt: file15_section.reaction_mt,
                        });
                    }
                    let (spectrum_integral, first_energy_moment) =
                        distribution.spectrum.integrate()?;
                    if spectrum_integral <= 0.0 || first_energy_moment < 0.0 {
                        return Err(EndfPhotonMomentError::NegativePhysicalValue {
                            file_number: file15_section.file_number,
                            reaction_mt: file15_section.reaction_mt,
                        });
                    }
                    let weighted_normalization = component_probability * spectrum_integral;
                    let absolute_normalization_error = (weighted_normalization - 1.0).abs();
                    let mean_photon_energy_ev = first_energy_moment / spectrum_integral;
                    let photon_energy_release_ev_barns =
                        mean_photon_energy_ev * continuum_cross_section_barns;
                    let status = if absolute_normalization_error <= normalization_tolerance {
                        EndfPhotonMomentSampleStatus::WithinTolerance
                    } else {
                        EndfPhotonMomentSampleStatus::OutsideTolerance
                    };
                    samples.push(EndfContinuumPhotonMomentSample {
                        incident_energy_ev: distribution.incident_energy_ev,
                        component_probability,
                        spectrum_integral,
                        weighted_normalization,
                        absolute_normalization_error,
                        mean_photon_energy_ev,
                        continuum_cross_section_barns,
                        photon_energy_release_ev_barns,
                        status,
                    });
                }
                let failed_sample_count = samples
                    .iter()
                    .filter(|sample| {
                        sample.status == EndfPhotonMomentSampleStatus::OutsideTolerance
                    })
                    .count() as u64;
                let maximum_absolute_normalization_error = samples
                    .iter()
                    .map(|sample| sample.absolute_normalization_error)
                    .fold(0.0_f64, f64::max);
                reactions.push(EndfContinuumPhotonMomentReaction {
                    nuclide: artifact.nuclide.clone(),
                    endf_mat: artifact.endf_mat,
                    reaction_mt: reaction.reaction_mt,
                    file13_section_sha256: file13_section.sha256.clone(),
                    file15_section_sha256: file15_section.sha256.clone(),
                    samples,
                    failed_sample_count,
                    maximum_absolute_normalization_error,
                });
            }
        }

        if reactions.is_empty() {
            return Err(EndfPhotonMomentError::NoSupportedReactions);
        }
        let failed_sample_count = reactions
            .iter()
            .map(|reaction| reaction.failed_sample_count)
            .sum();
        let report = Self {
            schema_version: ENDF_CONTINUUM_PHOTON_MOMENT_SCHEMA.into(),
            id: format!("{}.{}", selection.selection.id, REPORT_ID_SUFFIX),
            case_id: selection.selection.case_id.clone(),
            scope: EndfPhotonMomentScope::SingleComponentFile13ContinuumWithFile15,
            qualification: if failed_sample_count == 0 {
                EndfPhotonMomentQualification::SourceMomentsCheckedUnreviewed
            } else {
                EndfPhotonMomentQualification::SpectrumNormalizationRejected
            },
            evaluated_source_selection: ContentReference {
                id: selection.selection.id.clone(),
                sha256: selection.sha256.clone(),
            },
            photon_production_inventory: ContentReference {
                id: inventory.inventory.id.clone(),
                sha256: inventory.sha256.clone(),
            },
            normalization_tolerance,
            reaction_count: reactions.len() as u64,
            sample_count: reactions
                .iter()
                .map(|reaction| reaction.samples.len() as u64)
                .sum(),
            failed_sample_count,
            maximum_absolute_normalization_error: reactions
                .iter()
                .map(|reaction| reaction.maximum_absolute_normalization_error)
                .fold(0.0_f64, f64::max),
            reactions,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), EndfPhotonMomentError> {
        if !openbnct_core::schema_matches(&self.schema_version, ENDF_CONTINUUM_PHOTON_MOMENT_SCHEMA)
        {
            return invalid_report(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_reference(
            "evaluated_source_selection",
            &self.evaluated_source_selection,
        )?;
        validate_reference(
            "photon_production_inventory",
            &self.photon_production_inventory,
        )?;
        if self.id
            != format!(
                "{}.{}",
                self.evaluated_source_selection.id, REPORT_ID_SUFFIX
            )
        {
            return invalid_report("report ID does not bind the source selection");
        }
        validate_tolerance(self.normalization_tolerance)?;
        if self.reactions.is_empty() {
            return invalid_report("moment report contains no reactions");
        }

        let mut previous_key: Option<(&str, u16)> = None;
        for reaction in &self.reactions {
            validate_identifier("reactions.nuclide", &reaction.nuclide)?;
            let key = (reaction.nuclide.as_str(), reaction.reaction_mt);
            if reaction.endf_mat == 0
                || reaction.reaction_mt == 0
                || previous_key.is_some_and(|previous| previous >= key)
            {
                return invalid_report("reactions are not in canonical order");
            }
            previous_key = Some(key);
            validate_sha256(
                "reactions.file13_section_sha256",
                &reaction.file13_section_sha256,
            )?;
            validate_sha256(
                "reactions.file15_section_sha256",
                &reaction.file15_section_sha256,
            )?;
            if reaction.samples.is_empty() {
                return invalid_report("reaction contains no moment samples");
            }
            let mut previous_energy = None;
            for sample in &reaction.samples {
                let values = [
                    sample.incident_energy_ev,
                    sample.component_probability,
                    sample.spectrum_integral,
                    sample.weighted_normalization,
                    sample.absolute_normalization_error,
                    sample.mean_photon_energy_ev,
                    sample.continuum_cross_section_barns,
                    sample.photon_energy_release_ev_barns,
                ];
                if values
                    .iter()
                    .any(|value| !value.is_finite() || *value < 0.0)
                    || sample.incident_energy_ev == 0.0
                    || sample.spectrum_integral == 0.0
                    || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
                {
                    return invalid_report("invalid or unordered moment sample");
                }
                previous_energy = Some(sample.incident_energy_ev);
                if !approximately_equal(
                    sample.weighted_normalization,
                    sample.component_probability * sample.spectrum_integral,
                ) || !approximately_equal(
                    sample.absolute_normalization_error,
                    (sample.weighted_normalization - 1.0).abs(),
                ) || !approximately_equal(
                    sample.photon_energy_release_ev_barns,
                    sample.mean_photon_energy_ev * sample.continuum_cross_section_barns,
                ) {
                    return invalid_report("moment sample derived values do not close");
                }
                let expected_status =
                    if sample.absolute_normalization_error <= self.normalization_tolerance {
                        EndfPhotonMomentSampleStatus::WithinTolerance
                    } else {
                        EndfPhotonMomentSampleStatus::OutsideTolerance
                    };
                if sample.status != expected_status {
                    return invalid_report("moment sample status does not match its error");
                }
            }
            let failed = reaction
                .samples
                .iter()
                .filter(|sample| sample.status == EndfPhotonMomentSampleStatus::OutsideTolerance)
                .count() as u64;
            let maximum = reaction
                .samples
                .iter()
                .map(|sample| sample.absolute_normalization_error)
                .fold(0.0_f64, f64::max);
            if reaction.failed_sample_count != failed
                || !approximately_equal(reaction.maximum_absolute_normalization_error, maximum)
            {
                return invalid_report("reaction aggregates do not match its samples");
            }
        }

        let reaction_count = self.reactions.len() as u64;
        let sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.samples.len() as u64)
            .sum();
        let failed_sample_count: u64 = self
            .reactions
            .iter()
            .map(|reaction| reaction.failed_sample_count)
            .sum();
        let maximum_absolute_normalization_error = self
            .reactions
            .iter()
            .map(|reaction| reaction.maximum_absolute_normalization_error)
            .fold(0.0_f64, f64::max);
        if self.reaction_count != reaction_count
            || self.sample_count != sample_count
            || self.failed_sample_count != failed_sample_count
            || !approximately_equal(
                self.maximum_absolute_normalization_error,
                maximum_absolute_normalization_error,
            )
        {
            return invalid_report("report aggregates do not match its reactions");
        }
        let expected_qualification = if failed_sample_count == 0 {
            EndfPhotonMomentQualification::SourceMomentsCheckedUnreviewed
        } else {
            EndfPhotonMomentQualification::SpectrumNormalizationRejected
        };
        if self.qualification != expected_qualification {
            return invalid_report("qualification does not match sample results");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<EndfContinuumPhotonMomentResult, EndfPhotonMomentError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| EndfPhotonMomentError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| EndfPhotonMomentError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(EndfContinuumPhotonMomentResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl EndfContinuumPhotonMomentReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EndfPhotonMomentError> {
        let report: EndfContinuumPhotonMomentReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, EndfPhotonMomentError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_sources(
        &self,
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        inventory: &EndfPhotonProductionInventoryDocument,
    ) -> Result<(), EndfPhotonMomentError> {
        let observed = EndfContinuumPhotonMomentReport::calculate(
            selection,
            evaluations_root,
            inventory,
            self.report.normalization_tolerance,
        )?;
        if self.report != observed {
            return Err(EndfPhotonMomentError::ReportMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TabulatedFunction {
    pub(crate) interpolation: Vec<InterpolationRegion>,
    pub(crate) points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct InterpolationRegion {
    pub(crate) upper_point_index: usize,
    pub(crate) law: i64,
}

impl TabulatedFunction {
    pub(crate) fn evaluate(&self, x: f64) -> Result<f64, EndfPhotonMomentError> {
        if !x.is_finite() {
            return Err(EndfPhotonMomentError::InterpolationOutsideDomain(x));
        }
        if let Some((_, y)) = self.points.iter().find(|(point, _)| *point == x) {
            return Ok(*y);
        }
        let segment = self
            .points
            .windows(2)
            .position(|points| points[0].0 < x && x < points[1].0)
            .ok_or(EndfPhotonMomentError::InterpolationOutsideDomain(x))?;
        let (x0, y0) = self.points[segment];
        let (x1, y1) = self.points[segment + 1];
        interpolate(self.law_for_segment(segment)?, x0, y0, x1, y1, x)
    }

    fn integrate(&self) -> Result<(f64, f64), EndfPhotonMomentError> {
        let (integral, first_moment, _) = self.integrate_moments()?;
        Ok((integral, first_moment))
    }

    pub(crate) fn integrate_moments(&self) -> Result<(f64, f64, f64), EndfPhotonMomentError> {
        let mut integral = 0.0;
        let mut first_moment = 0.0;
        let mut second_moment = 0.0;
        for (segment, points) in self.points.windows(2).enumerate() {
            let (x0, y0) = points[0];
            let (x1, y1) = points[1];
            if x0 < 0.0 || x1 <= x0 || y0 < 0.0 || y1 < 0.0 {
                return Err(EndfPhotonMomentError::InvalidTabulation);
            }
            match self.law_for_segment(segment)? {
                1 => {
                    integral += y0 * (x1 - x0);
                    first_moment += y0 * (x1 * x1 - x0 * x0) / 2.0;
                    second_moment += y0 * (x1.powi(3) - x0.powi(3)) / 3.0;
                }
                2 => {
                    let slope = (y1 - y0) / (x1 - x0);
                    integral += (y0 + y1) * (x1 - x0) / 2.0;
                    first_moment += y0 * (x1 * x1 - x0 * x0) / 2.0
                        + slope
                            * ((x1.powi(3) - x0.powi(3)) / 3.0 - x0 * (x1 * x1 - x0 * x0) / 2.0);
                    second_moment += y0 * (x1.powi(3) - x0.powi(3)) / 3.0
                        + slope
                            * ((x1.powi(4) - x0.powi(4)) / 4.0
                                - x0 * (x1.powi(3) - x0.powi(3)) / 3.0);
                }
                law => return Err(EndfPhotonMomentError::UnsupportedInterpolation(law)),
            }
        }
        Ok((integral, first_moment, second_moment))
    }

    fn law_for_segment(&self, zero_based_segment: usize) -> Result<i64, EndfPhotonMomentError> {
        let upper_point_index = zero_based_segment + 2;
        self.interpolation
            .iter()
            .find(|region| upper_point_index <= region.upper_point_index)
            .map(|region| region.law)
            .ok_or(EndfPhotonMomentError::InvalidTabulation)
    }
}

struct File15Spectra {
    component_probability: TabulatedFunction,
    distributions: Vec<IncidentSpectrum>,
}

struct IncidentSpectrum {
    incident_energy_ev: f64,
    spectrum: TabulatedFunction,
}

fn parse_file13_continuum(
    section: &ParsedSection,
) -> Result<TabulatedFunction, EndfPhotonMomentError> {
    let head = control(section, 0)?;
    let subsection_count = positive_usize(head.n1)?;
    let mut cursor = 1_usize;
    if subsection_count > 1 {
        parse_tab1(section, &mut cursor)?;
    }
    let mut continuum = None;
    for _ in 0..subsection_count {
        let (record, tabulation) = parse_tab1(section, &mut cursor)?;
        if record.c1 == 0.0 && record.l2 == 1 && continuum.replace(tabulation).is_some() {
            return Err(EndfPhotonMomentError::InvalidTabulation);
        }
    }
    require_consumed(section, cursor)?;
    continuum.ok_or(EndfPhotonMomentError::InvalidTabulation)
}

fn parse_single_component_file15(
    section: &ParsedSection,
) -> Result<File15Spectra, EndfPhotonMomentError> {
    let head = control(section, 0)?;
    if head.n1 != 1 {
        return Err(EndfPhotonMomentError::UnsupportedRepresentation {
            nuclide: "unknown".into(),
            reaction_mt: section.reaction_mt,
            message: format!(
                "File 15 has {} components; only one is implemented",
                head.n1
            ),
        });
    }
    let mut cursor = 1_usize;
    let (component, component_probability) = parse_tab1(section, &mut cursor)?;
    if component.l2 != 1 {
        return Err(EndfPhotonMomentError::UnsupportedRepresentation {
            nuclide: "unknown".into(),
            reaction_mt: section.reaction_mt,
            message: format!("unsupported File 15 LF={}", component.l2),
        });
    }
    let (energy_head, _) = parse_tab2(section, &mut cursor)?;
    let energy_count = positive_usize(energy_head.n2)?;
    let mut distributions = Vec::with_capacity(energy_count);
    for _ in 0..energy_count {
        let (distribution, spectrum) = parse_tab1(section, &mut cursor)?;
        let incident_energy_ev = value(distribution, 1)?;
        if incident_energy_ev <= 0.0 {
            return Err(EndfPhotonMomentError::InvalidTabulation);
        }
        distributions.push(IncidentSpectrum {
            incident_energy_ev,
            spectrum,
        });
    }
    require_consumed(section, cursor)?;
    if distributions
        .windows(2)
        .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    Ok(File15Spectra {
        component_probability,
        distributions,
    })
}

pub(crate) fn parse_tab1(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<(EndfRecord, TabulatedFunction), EndfPhotonMomentError> {
    let head = take_control(section, cursor)?;
    let region_count = positive_usize(head.n1)?;
    let point_count = positive_usize(head.n2)?;
    let interpolation_words = take_words(section, cursor, region_count * 2)?;
    let point_words = take_words(section, cursor, point_count * 2)?;
    let interpolation = parse_interpolation(&interpolation_words, point_count)?;
    let points = point_words
        .chunks_exact(2)
        .map(|pair| (pair[0], pair[1]))
        .collect::<Vec<_>>();
    validate_points(&points)?;
    Ok((
        head,
        TabulatedFunction {
            interpolation,
            points,
        },
    ))
}

pub(crate) fn parse_tab2(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<(EndfRecord, Vec<InterpolationRegion>), EndfPhotonMomentError> {
    let head = take_control(section, cursor)?;
    let region_count = positive_usize(head.n1)?;
    let point_count = positive_usize(head.n2)?;
    let words = take_words(section, cursor, region_count * 2)?;
    Ok((head, parse_interpolation(&words, point_count)?))
}

fn parse_interpolation(
    words: &[f64],
    point_count: usize,
) -> Result<Vec<InterpolationRegion>, EndfPhotonMomentError> {
    let mut regions = Vec::with_capacity(words.len() / 2);
    for pair in words.chunks_exact(2) {
        let upper_point_index = exact_usize(pair[0])?;
        let law = exact_i64(pair[1])?;
        if upper_point_index < 2
            || upper_point_index > point_count
            || !matches!(law, 1..=5)
            || regions
                .last()
                .is_some_and(|previous: &InterpolationRegion| {
                    previous.upper_point_index >= upper_point_index
                })
        {
            return Err(EndfPhotonMomentError::InvalidTabulation);
        }
        regions.push(InterpolationRegion {
            upper_point_index,
            law,
        });
    }
    if regions.last().map(|region| region.upper_point_index) != Some(point_count) {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    Ok(regions)
}

fn validate_points(points: &[(f64, f64)]) -> Result<(), EndfPhotonMomentError> {
    if points.len() < 2
        || points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite())
        || points.windows(2).any(|pair| pair[0].0 >= pair[1].0)
    {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    Ok(())
}

pub(crate) fn take_words(
    section: &ParsedSection,
    cursor: &mut usize,
    word_count: usize,
) -> Result<Vec<f64>, EndfPhotonMomentError> {
    let record_count = word_count.div_ceil(6);
    let end = cursor
        .checked_add(record_count)
        .ok_or(EndfPhotonMomentError::InvalidTabulation)?;
    if end > section.records.len() {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    let mut words = Vec::with_capacity(word_count);
    for record in &section.records[*cursor..end] {
        for field in record.values {
            if words.len() == word_count {
                if field.is_some() {
                    return Err(EndfPhotonMomentError::InvalidTabulation);
                }
            } else {
                words.push(field.ok_or(EndfPhotonMomentError::InvalidTabulation)?);
            }
        }
    }
    *cursor = end;
    Ok(words)
}

pub(crate) fn take_control(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<EndfRecord, EndfPhotonMomentError> {
    let record = control(section, *cursor)?;
    *cursor += 1;
    Ok(record)
}

pub(crate) fn control(
    section: &ParsedSection,
    index: usize,
) -> Result<EndfRecord, EndfPhotonMomentError> {
    let record = section
        .records
        .get(index)
        .copied()
        .ok_or(EndfPhotonMomentError::InvalidTabulation)?;
    if !record.is_control {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    Ok(record)
}

pub(crate) fn value(record: EndfRecord, index: usize) -> Result<f64, EndfPhotonMomentError> {
    record.values[index].ok_or(EndfPhotonMomentError::InvalidTabulation)
}

pub(crate) fn find_section(
    sections: &[ParsedSection],
    file_number: u16,
    reaction_mt: u16,
) -> Result<&ParsedSection, EndfPhotonMomentError> {
    sections
        .iter()
        .find(|section| section.file_number == file_number && section.reaction_mt == reaction_mt)
        .ok_or(EndfPhotonMomentError::MissingSection {
            file_number,
            reaction_mt,
        })
}

pub(crate) fn require_consumed(
    section: &ParsedSection,
    cursor: usize,
) -> Result<(), EndfPhotonMomentError> {
    if cursor == section.records.len() {
        Ok(())
    } else {
        Err(EndfPhotonMomentError::InvalidTabulation)
    }
}

fn interpolate(
    law: i64,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    x: f64,
) -> Result<f64, EndfPhotonMomentError> {
    if x1 <= x0 {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    let fraction = (x - x0) / (x1 - x0);
    match law {
        1 => Ok(y0),
        2 => Ok(y0 + fraction * (y1 - y0)),
        3 if x0 > 0.0 && x > 0.0 => Ok(y0 + (y1 - y0) * (x / x0).ln() / (x1 / x0).ln()),
        4 if y0 > 0.0 && y1 > 0.0 => Ok(y0 * (y1 / y0).powf(fraction)),
        5 if x0 > 0.0 && x > 0.0 && y0 > 0.0 && y1 > 0.0 => {
            Ok(y0 * (y1 / y0).powf((x / x0).ln() / (x1 / x0).ln()))
        }
        supported @ 1..=5 => Err(EndfPhotonMomentError::InvalidInterpolationDomain(supported)),
        unsupported => Err(EndfPhotonMomentError::UnsupportedInterpolation(unsupported)),
    }
}

fn exact_usize(value: f64) -> Result<usize, EndfPhotonMomentError> {
    if value.is_finite() && value >= 0.0 && value.fract() == 0.0 && value <= usize::MAX as f64 {
        Ok(value as usize)
    } else {
        Err(EndfPhotonMomentError::InvalidTabulation)
    }
}

fn exact_i64(value: f64) -> Result<i64, EndfPhotonMomentError> {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= i64::MIN as f64
        && value <= i64::MAX as f64
    {
        Ok(value as i64)
    } else {
        Err(EndfPhotonMomentError::InvalidTabulation)
    }
}

pub(crate) fn positive_usize(value: i64) -> Result<usize, EndfPhotonMomentError> {
    usize::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or(EndfPhotonMomentError::InvalidTabulation)
}

fn validate_tolerance(value: f64) -> Result<(), EndfPhotonMomentError> {
    if value.is_finite() && value > 0.0 && value <= 1.0e-2 {
        Ok(())
    } else {
        invalid_report("normalization tolerance must be in (0, 1e-2]")
    }
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), EndfPhotonMomentError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), EndfPhotonMomentError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), EndfPhotonMomentError> {
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

fn read_regular_file(path: &Path) -> Result<Vec<u8>, EndfPhotonMomentError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| EndfPhotonMomentError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(EndfPhotonMomentError::NotRegularFile(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| EndfPhotonMomentError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, EndfPhotonMomentError> {
    Err(EndfPhotonMomentError::InvalidReport(message.into()))
}

#[derive(Debug, Error)]
pub enum EndfPhotonMomentError {
    #[error(transparent)]
    EvaluatedSource(#[from] EvaluatedSourceError),
    #[error(transparent)]
    PhotonInventory(#[from] EndfPhotonInventoryError),
    #[error("photon-moment inventory does not bind the exact source evaluation")]
    InventoryBindingMismatch,
    #[error("missing MF={file_number}/MT={reaction_mt} section")]
    MissingSection { file_number: u16, reaction_mt: u16 },
    #[error("unsupported photon representation for {nuclide} MT={reaction_mt}: {message}")]
    UnsupportedRepresentation {
        nuclide: String,
        reaction_mt: u16,
        message: String,
    },
    #[error("no supported single-component File 13/File 15 continuum reactions were found")]
    NoSupportedReactions,
    #[error("invalid ENDF tabulation")]
    InvalidTabulation,
    #[error("unsupported ENDF interpolation law {0}")]
    UnsupportedInterpolation(i64),
    #[error("ENDF interpolation law {0} has values outside its mathematical domain")]
    InvalidInterpolationDomain(i64),
    #[error("interpolation energy {0} is outside the tabulated domain")]
    InterpolationOutsideDomain(f64),
    #[error("negative physical value in MF={file_number}/MT={reaction_mt}")]
    NegativePhysicalValue { file_number: u16, reaction_mt: u16 },
    #[error("invalid continuum photon-moment report: {0}")]
    InvalidReport(String),
    #[error("stored continuum photon-moment report does not match regenerated source evidence")]
    ReportMismatch,
    #[error("required photon-moment artifact is not a regular file: {0}")]
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
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/endfb81-file13-continuum-photon-moments.json"
    );
    const JEFF40_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-file13-continuum-photon-moments.json"
    );

    #[test]
    fn integrates_histogram_probability_and_first_moment() {
        let function = TabulatedFunction {
            interpolation: vec![InterpolationRegion {
                upper_point_index: 3,
                law: 1,
            }],
            points: vec![(0.0, 0.25), (2.0, 0.25), (4.0, 0.0)],
        };
        let (integral, moment) = function.integrate().unwrap();
        assert_eq!(integral, 1.0);
        assert_eq!(moment, 2.0);
        let (_, _, second_moment) = function.integrate_moments().unwrap();
        assert!((second_moment - 16.0 / 3.0).abs() < 1.0e-14);
    }

    #[test]
    fn integrates_linear_probability_exactly() {
        let function = TabulatedFunction {
            interpolation: vec![InterpolationRegion {
                upper_point_index: 2,
                law: 2,
            }],
            points: vec![(0.0, 0.0), (2.0, 1.0)],
        };
        let (integral, moment) = function.integrate().unwrap();
        assert_eq!(integral, 1.0);
        assert!((moment - 4.0 / 3.0).abs() < 1.0e-15);
        let (_, _, second_moment) = function.integrate_moments().unwrap();
        assert_eq!(second_moment, 2.0);
    }

    #[test]
    fn json_evidence_floats_round_trip_exactly() {
        let value = 1_961_808.809_999_999_8_f64;
        let json = serde_json::to_string(&value).unwrap();
        let parsed: f64 = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.to_bits(), value.to_bits());
    }

    #[test]
    fn validates_frozen_independent_moments() {
        let baseline =
            EndfContinuumPhotonMomentReportDocument::from_bytes(BASELINE_REPORT).unwrap();
        let jeff = EndfContinuumPhotonMomentReportDocument::from_bytes(JEFF40_REPORT).unwrap();
        assert_eq!(
            baseline.sha256,
            "2f3cd758f0b7106f8a859fcf0887a1047cea1646233c4ae5e25fec11563dddee"
        );
        assert_eq!(
            jeff.sha256,
            "6dac7055c0b970addfa1aa9bd89e5fa0f95ce87ffc4901e8a0c817ea2b4c455f"
        );
        assert_eq!(baseline.report.reaction_count, 8);
        assert_eq!(baseline.report.sample_count, 92);
        assert_eq!(baseline.report.failed_sample_count, 0);
        assert_eq!(
            baseline.report.maximum_absolute_normalization_error,
            1.607500000000428e-5
        );
        assert_eq!(baseline.report.reactions.len(), jeff.report.reactions.len());
        for (baseline_reaction, jeff_reaction) in
            baseline.report.reactions.iter().zip(&jeff.report.reactions)
        {
            assert_eq!(baseline_reaction.nuclide, jeff_reaction.nuclide);
            assert_eq!(baseline_reaction.reaction_mt, jeff_reaction.reaction_mt);
            assert_eq!(baseline_reaction.samples, jeff_reaction.samples);
        }
        let six_mev = baseline.report.reactions[0]
            .samples
            .iter()
            .find(|sample| sample.incident_energy_ev == 6.0e6)
            .unwrap();
        assert_eq!(six_mev.mean_photon_energy_ev, 5.25e6);
        assert_eq!(six_mev.continuum_cross_section_barns, 0.11522);
        assert_eq!(six_mev.photon_energy_release_ev_barns, 604_905.0);
    }
}
