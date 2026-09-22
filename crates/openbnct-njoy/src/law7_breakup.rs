// SPDX-License-Identifier: MIT

//! Independent source-level energy accounting for the deuterium MF=6/MT=16
//! LAW=7 neutron distribution.
//!
//! The JEFF-4.0 evaluation represents the two emitted neutrons but omits the
//! proton product. This calculator integrates the exact laboratory-frame
//! double-differential neutron distribution and determines whether the energy
//! left for that implicit charged residual is nonnegative. It does not call
//! NJOY or consume processor output.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::photon_inventory::{EndfRecord, ParsedSection, parse_evaluation_sections};
use crate::photon_moment::{
    EndfPhotonMomentError, InterpolationRegion, TabulatedFunction, control, find_section,
    parse_tab1, parse_tab2, positive_usize, require_consumed, take_control, take_words, value,
};
use crate::{EndfPhotonInventoryError, EndfPhotonProductionInventoryDocument};

pub const ENDF_MF6_LAW7_IMPLICIT_RESIDUAL_SCHEMA: &str =
    "openbnct.endf-mf6-law7-implicit-residual/0.1.0";
pub const DEFAULT_LAW7_BREAKUP_NORMALIZATION_TOLERANCE: f64 = 1.0e-4;
pub const DEFAULT_LAW7_BREAKUP_RELATIVE_ENERGY_TOLERANCE: f64 = 1.0e-6;

const REPORT_ID_SUFFIX: &str = "endf-mf6-law7-implicit-residual";
const BREAKUP_MT: u16 = 16;
const DEUTERIUM_ZAP: u32 = 1002;
const NEUTRON_ZAP: u32 = 1;
const PROTON_ZAP: u32 = 1001;
const TWO_NEUTRON_MULTIPLICITY: f64 = 2.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfMf6Law7ImplicitResidualReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub scope: EndfMf6Law7ImplicitResidualScope,
    pub qualification: EndfMf6Law7ImplicitResidualQualification,
    pub evaluated_source_selection: ContentReference,
    pub photon_production_inventory: ContentReference,
    pub nuclide: String,
    pub endf_mat: u16,
    pub reaction_mt: u16,
    pub source_evaluation_sha256: String,
    pub file3_section_sha256: String,
    pub file6_section_sha256: String,
    pub target_zap: u32,
    pub target_atomic_weight_ratio: f64,
    pub emitted_neutron_zap: u32,
    pub emitted_neutron_atomic_weight_ratio: f64,
    pub neutron_multiplicity: f64,
    pub implicit_residual_zap: u32,
    pub implicit_residual_atomic_weight_ratio: f64,
    pub explicit_residual_product_present: bool,
    pub photon_production_present_for_reaction: bool,
    pub file6_reference_frame: EndfMf6Law7ReferenceFrame,
    pub file6_product_law: u16,
    pub mass_difference_q_value_ev: f64,
    pub reaction_energy_q_value_ev: f64,
    pub normalization_tolerance: f64,
    pub relative_energy_tolerance: f64,
    pub source_incident_node_count: u64,
    pub zero_cross_section_boundary_node_count: u64,
    pub redundant_zero_density_endpoint_count: u64,
    pub samples: Vec<EndfMf6Law7ImplicitResidualSample>,
    pub sample_count: u64,
    pub failed_normalization_sample_count: u64,
    pub failed_residual_energy_sample_count: u64,
    pub maximum_absolute_normalization_error: f64,
    pub maximum_negative_relative_residual_energy: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6Law7ImplicitResidualScope {
    DeuteriumN2nLaw7WithImplicitProtonResidual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6Law7ImplicitResidualQualification {
    SpectrumNormalizationRejected,
    NegativeImplicitResidualEnergyRejected,
    SpectrumNormalizationAndResidualEnergyRejected,
    ImplicitResidualEnergyCheckedUnreviewed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6Law7ReferenceFrame {
    Laboratory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfMf6Law7ImplicitResidualSample {
    pub incident_energy_ev: f64,
    pub reaction_cross_section_barns: f64,
    pub neutron_yield: f64,
    pub distribution_normalization: f64,
    pub absolute_normalization_error: f64,
    pub raw_first_neutron_energy_moment_ev: f64,
    pub normalized_mean_neutron_energy_ev: f64,
    pub total_transported_neutron_energy_ev: f64,
    pub available_reaction_energy_ev: f64,
    pub implicit_residual_energy_ev: f64,
    pub negative_relative_residual_energy: f64,
    pub implicit_local_kerma_ev_barns: f64,
    pub status: EndfMf6Law7ImplicitResidualSampleStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfMf6Law7ImplicitResidualSampleStatus {
    WithinTolerance,
    SpectrumNormalizationOutsideTolerance,
    NegativeImplicitResidualEnergyOutsideTolerance,
    SpectrumNormalizationAndResidualEnergyOutsideTolerance,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfMf6Law7ImplicitResidualReportDocument {
    pub report: EndfMf6Law7ImplicitResidualReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfMf6Law7ImplicitResidualResult {
    pub report: EndfMf6Law7ImplicitResidualReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

impl EndfMf6Law7ImplicitResidualReport {
    pub fn calculate(
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        inventory: &EndfPhotonProductionInventoryDocument,
        nuclide: &str,
        normalization_tolerance: f64,
        relative_energy_tolerance: f64,
    ) -> Result<Self, EndfMf6Law7ImplicitResidualError> {
        validate_tolerances(normalization_tolerance, relative_energy_tolerance)?;
        inventory.verify_against_selection(selection, evaluations_root)?;

        let artifact = selection
            .selection
            .evaluations
            .iter()
            .find(|artifact| artifact.nuclide == nuclide)
            .ok_or_else(|| EndfMf6Law7ImplicitResidualError::MissingNuclide(nuclide.into()))?;
        let inventory_evaluation = inventory
            .inventory
            .evaluations
            .iter()
            .find(|evaluation| evaluation.nuclide == nuclide)
            .ok_or(EndfMf6Law7ImplicitResidualError::InventoryBindingMismatch)?;
        if artifact.endf_mat != inventory_evaluation.endf_mat
            || artifact.sha256 != inventory_evaluation.source_evaluation.sha256
        {
            return Err(EndfMf6Law7ImplicitResidualError::InventoryBindingMismatch);
        }

        let inventory_section = inventory_evaluation
            .sections
            .iter()
            .find(|section| section.file_number == 6 && section.reaction_mt == BREAKUP_MT)
            .ok_or(EndfMf6Law7ImplicitResidualError::InventoryBindingMismatch)?;
        let inventory_reaction = inventory_evaluation
            .reactions
            .iter()
            .find(|reaction| reaction.reaction_mt == BREAKUP_MT)
            .ok_or(EndfMf6Law7ImplicitResidualError::InventoryBindingMismatch)?;
        let photon_production_present_for_reaction =
            !inventory_reaction.file6_photon_products.is_empty()
                || inventory_reaction.file12.is_some()
                || inventory_reaction.file13.is_some()
                || inventory_reaction.file14.is_some()
                || inventory_reaction.file15.is_some();
        if photon_production_present_for_reaction {
            return unsupported(nuclide, "MT=16 has a photon-production representation");
        }

        let path = evaluations_root.join(&artifact.extracted_filename);
        let bytes = read_regular_file(&path)?;
        let sections = parse_evaluation_sections(
            &bytes,
            artifact.endf_mat,
            &[(3, BREAKUP_MT), (6, BREAKUP_MT)],
        )?;
        let file3_section = find_section(&sections, 3, BREAKUP_MT)?;
        let file6_section = find_section(&sections, 6, BREAKUP_MT)?;
        if file6_section.sha256 != inventory_section.sha256 {
            return Err(EndfMf6Law7ImplicitResidualError::InventoryBindingMismatch);
        }
        let file3 = parse_file3_breakup(file3_section, nuclide)?;
        let file6 = parse_file6_law7_breakup(file6_section, nuclide)?;
        validate_source_topology(&file3, &file6, nuclide)?;

        let mut zero_cross_section_boundary_node_count = 0_u64;
        let mut samples = Vec::with_capacity(file6.distributions.len());
        for distribution in &file6.distributions {
            let incident_energy_ev = distribution.incident_energy_ev;
            let reaction_cross_section_barns = file3.cross_section.evaluate(incident_energy_ev)?;
            let neutron_yield = file6.yield_function.evaluate(incident_energy_ev)?;
            if reaction_cross_section_barns < 0.0 || neutron_yield < 0.0 {
                return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
            }
            if !approximately_equal(neutron_yield, TWO_NEUTRON_MULTIPLICITY) {
                return unsupported(
                    nuclide,
                    format!("MT=16 neutron yield is {neutron_yield}, not exactly two"),
                );
            }
            if reaction_cross_section_barns == 0.0 {
                zero_cross_section_boundary_node_count += 1;
                continue;
            }
            if distribution.normalization <= 0.0 || distribution.first_moment_ev < 0.0 {
                return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
            }
            let absolute_normalization_error = (distribution.normalization - 1.0).abs();
            let normalized_mean_neutron_energy_ev =
                distribution.first_moment_ev / distribution.normalization;
            let total_transported_neutron_energy_ev =
                neutron_yield * normalized_mean_neutron_energy_ev;
            let available_reaction_energy_ev =
                incident_energy_ev + file3.mass_difference_q_value_ev;
            if available_reaction_energy_ev <= 0.0 {
                return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
            }
            let implicit_residual_energy_ev =
                available_reaction_energy_ev - total_transported_neutron_energy_ev;
            let negative_relative_residual_energy =
                (-implicit_residual_energy_ev).max(0.0) / available_reaction_energy_ev.max(1.0);
            let status = sample_status(
                absolute_normalization_error,
                negative_relative_residual_energy,
                normalization_tolerance,
                relative_energy_tolerance,
            );
            samples.push(EndfMf6Law7ImplicitResidualSample {
                incident_energy_ev,
                reaction_cross_section_barns,
                neutron_yield,
                distribution_normalization: distribution.normalization,
                absolute_normalization_error,
                raw_first_neutron_energy_moment_ev: distribution.first_moment_ev,
                normalized_mean_neutron_energy_ev,
                total_transported_neutron_energy_ev,
                available_reaction_energy_ev,
                implicit_residual_energy_ev,
                negative_relative_residual_energy,
                implicit_local_kerma_ev_barns: implicit_residual_energy_ev
                    * reaction_cross_section_barns,
                status,
            });
        }

        let failed_normalization_sample_count = samples
            .iter()
            .filter(|sample| sample.normalization_failed(normalization_tolerance))
            .count() as u64;
        let failed_residual_energy_sample_count = samples
            .iter()
            .filter(|sample| sample.residual_energy_failed(relative_energy_tolerance))
            .count() as u64;
        let report = Self {
            schema_version: ENDF_MF6_LAW7_IMPLICIT_RESIDUAL_SCHEMA.into(),
            id: report_id(&selection.selection.id, nuclide),
            case_id: selection.selection.case_id.clone(),
            scope: EndfMf6Law7ImplicitResidualScope::DeuteriumN2nLaw7WithImplicitProtonResidual,
            qualification: qualification(
                failed_normalization_sample_count,
                failed_residual_energy_sample_count,
            ),
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
            reaction_mt: BREAKUP_MT,
            source_evaluation_sha256: artifact.sha256.clone(),
            file3_section_sha256: file3_section.sha256.clone(),
            file6_section_sha256: file6_section.sha256.clone(),
            target_zap: file3.target_zap,
            target_atomic_weight_ratio: file3.target_atomic_weight_ratio,
            emitted_neutron_zap: file6.emitted_neutron_zap,
            emitted_neutron_atomic_weight_ratio: file6.emitted_neutron_atomic_weight_ratio,
            neutron_multiplicity: TWO_NEUTRON_MULTIPLICITY,
            implicit_residual_zap: PROTON_ZAP,
            implicit_residual_atomic_weight_ratio: file3.target_atomic_weight_ratio + 1.0
                - TWO_NEUTRON_MULTIPLICITY * file6.emitted_neutron_atomic_weight_ratio,
            explicit_residual_product_present: false,
            photon_production_present_for_reaction,
            file6_reference_frame: EndfMf6Law7ReferenceFrame::Laboratory,
            file6_product_law: 7,
            mass_difference_q_value_ev: file3.mass_difference_q_value_ev,
            reaction_energy_q_value_ev: file3.reaction_energy_q_value_ev,
            normalization_tolerance,
            relative_energy_tolerance,
            source_incident_node_count: file6.distributions.len() as u64,
            zero_cross_section_boundary_node_count,
            redundant_zero_density_endpoint_count: file6.redundant_zero_density_endpoint_count,
            sample_count: samples.len() as u64,
            failed_normalization_sample_count,
            failed_residual_energy_sample_count,
            maximum_absolute_normalization_error: samples
                .iter()
                .map(|sample| sample.absolute_normalization_error)
                .fold(0.0_f64, f64::max),
            maximum_negative_relative_residual_energy: samples
                .iter()
                .map(|sample| sample.negative_relative_residual_energy)
                .fold(0.0_f64, f64::max),
            samples,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), EndfMf6Law7ImplicitResidualError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            ENDF_MF6_LAW7_IMPLICIT_RESIDUAL_SCHEMA,
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
        for (label, hash) in [
            ("source_evaluation_sha256", &self.source_evaluation_sha256),
            ("file3_section_sha256", &self.file3_section_sha256),
            ("file6_section_sha256", &self.file6_section_sha256),
        ] {
            validate_sha256(label, hash)?;
        }
        validate_tolerances(self.normalization_tolerance, self.relative_energy_tolerance)?;
        if self.nuclide != "H2"
            || self.endf_mat == 0
            || self.reaction_mt != BREAKUP_MT
            || self.target_zap != DEUTERIUM_ZAP
            || self.emitted_neutron_zap != NEUTRON_ZAP
            || self.implicit_residual_zap != PROTON_ZAP
            || self.file6_reference_frame != EndfMf6Law7ReferenceFrame::Laboratory
            || self.file6_product_law != 7
            || self.explicit_residual_product_present
            || self.photon_production_present_for_reaction
            || !approximately_equal(self.neutron_multiplicity, TWO_NEUTRON_MULTIPLICITY)
        {
            return invalid_report("report is outside the narrow deuterium LAW=7 scope");
        }
        let expected_residual_awr = self.target_atomic_weight_ratio + 1.0
            - self.neutron_multiplicity * self.emitted_neutron_atomic_weight_ratio;
        if !self.target_atomic_weight_ratio.is_finite()
            || self.target_atomic_weight_ratio <= 0.0
            || !self.emitted_neutron_atomic_weight_ratio.is_finite()
            || self.emitted_neutron_atomic_weight_ratio <= 0.0
            || !self.implicit_residual_atomic_weight_ratio.is_finite()
            || self.implicit_residual_atomic_weight_ratio <= 0.0
            || !approximately_equal(
                self.implicit_residual_atomic_weight_ratio,
                expected_residual_awr,
            )
            || !self.mass_difference_q_value_ev.is_finite()
            || self.mass_difference_q_value_ev >= 0.0
            || !self.reaction_energy_q_value_ev.is_finite()
        {
            return invalid_report("invalid deuterium breakup constants");
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
        let failed_residual_energy_sample_count = self
            .samples
            .iter()
            .filter(|sample| sample.residual_energy_failed(self.relative_energy_tolerance))
            .count() as u64;
        let maximum_absolute_normalization_error = self
            .samples
            .iter()
            .map(|sample| sample.absolute_normalization_error)
            .fold(0.0_f64, f64::max);
        let maximum_negative_relative_residual_energy = self
            .samples
            .iter()
            .map(|sample| sample.negative_relative_residual_energy)
            .fold(0.0_f64, f64::max);
        if self.source_incident_node_count
            != sample_count + self.zero_cross_section_boundary_node_count
            || self.zero_cross_section_boundary_node_count == 0
            || self.redundant_zero_density_endpoint_count == 0
            || self.sample_count != sample_count
            || self.failed_normalization_sample_count != failed_normalization_sample_count
            || self.failed_residual_energy_sample_count != failed_residual_energy_sample_count
            || !approximately_equal(
                self.maximum_absolute_normalization_error,
                maximum_absolute_normalization_error,
            )
            || !approximately_equal(
                self.maximum_negative_relative_residual_energy,
                maximum_negative_relative_residual_energy,
            )
        {
            return invalid_report("report aggregates do not match its samples");
        }
        if self.qualification
            != qualification(
                failed_normalization_sample_count,
                failed_residual_energy_sample_count,
            )
        {
            return invalid_report("qualification does not match sample results");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<EndfMf6Law7ImplicitResidualResult, EndfMf6Law7ImplicitResidualError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| EndfMf6Law7ImplicitResidualError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| EndfMf6Law7ImplicitResidualError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(EndfMf6Law7ImplicitResidualResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl EndfMf6Law7ImplicitResidualReportDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EndfMf6Law7ImplicitResidualError> {
        let report: EndfMf6Law7ImplicitResidualReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, EndfMf6Law7ImplicitResidualError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_sources(
        &self,
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        inventory: &EndfPhotonProductionInventoryDocument,
    ) -> Result<(), EndfMf6Law7ImplicitResidualError> {
        let observed = EndfMf6Law7ImplicitResidualReport::calculate(
            selection,
            evaluations_root,
            inventory,
            &self.report.nuclide,
            self.report.normalization_tolerance,
            self.report.relative_energy_tolerance,
        )?;
        if self.report != observed {
            return Err(EndfMf6Law7ImplicitResidualError::ReportMismatch);
        }
        Ok(())
    }
}

impl EndfMf6Law7ImplicitResidualSample {
    fn normalization_failed(&self, tolerance: f64) -> bool {
        self.absolute_normalization_error > tolerance
    }

    fn residual_energy_failed(&self, tolerance: f64) -> bool {
        self.negative_relative_residual_energy > tolerance
    }
}

struct File3Breakup {
    target_zap: u32,
    target_atomic_weight_ratio: f64,
    mass_difference_q_value_ev: f64,
    reaction_energy_q_value_ev: f64,
    cross_section: TabulatedFunction,
}

struct File6Law7Breakup {
    target_zap: u32,
    target_atomic_weight_ratio: f64,
    emitted_neutron_zap: u32,
    emitted_neutron_atomic_weight_ratio: f64,
    yield_function: TabulatedFunction,
    distributions: Vec<Law7DistributionMoment>,
    redundant_zero_density_endpoint_count: u64,
}

struct Law7DistributionMoment {
    incident_energy_ev: f64,
    normalization: f64,
    first_moment_ev: f64,
}

fn parse_file3_breakup(
    section: &ParsedSection,
    nuclide: &str,
) -> Result<File3Breakup, EndfMf6Law7ImplicitResidualError> {
    let head = control(section, 0)?;
    let target_zap = exact_u32(value(head, 0)?)?;
    let target_atomic_weight_ratio = value(head, 1)?;
    let mut cursor = 1_usize;
    let (reaction, cross_section) = parse_tab1(section, &mut cursor)?;
    require_consumed(section, cursor)?;
    let mass_difference_q_value_ev = value(reaction, 0)?;
    let reaction_energy_q_value_ev = value(reaction, 1)?;
    if target_zap != DEUTERIUM_ZAP
        || target_atomic_weight_ratio <= 0.0
        || mass_difference_q_value_ev >= 0.0
    {
        return unsupported(
            nuclide,
            "MF=3/MT=16 is not the expected deuterium breakup section",
        );
    }
    Ok(File3Breakup {
        target_zap,
        target_atomic_weight_ratio,
        mass_difference_q_value_ev,
        reaction_energy_q_value_ev,
        cross_section,
    })
}

fn parse_file6_law7_breakup(
    section: &ParsedSection,
    nuclide: &str,
) -> Result<File6Law7Breakup, EndfMf6Law7ImplicitResidualError> {
    let head = control(section, 0)?;
    let target_zap = exact_u32(value(head, 0)?)?;
    let target_atomic_weight_ratio = value(head, 1)?;
    if target_zap != DEUTERIUM_ZAP || head.l2 != 1 || head.n1 != 1 {
        return unsupported(
            nuclide,
            format!(
                "MF=6/MT=16 requires deuterium, LCT=1, and one explicit product; found ZA={target_zap}, LCT={}, NK={}",
                head.l2, head.n1
            ),
        );
    }
    let mut cursor = 1_usize;
    let (product, yield_function) = parse_tab1(section, &mut cursor)?;
    let emitted_neutron_zap = exact_u32(value(product, 0)?)?;
    let emitted_neutron_atomic_weight_ratio = value(product, 1)?;
    if emitted_neutron_zap != NEUTRON_ZAP
        || !approximately_equal(emitted_neutron_atomic_weight_ratio, 1.0)
        || product.l1 != 0
        || product.l2 != 7
        || yield_function
            .points
            .iter()
            .any(|(_, yield_value)| !approximately_equal(*yield_value, TWO_NEUTRON_MULTIPLICITY))
    {
        return unsupported(
            nuclide,
            "MF=6/MT=16 is not one LAW=7 neutron product with multiplicity two",
        );
    }

    let (incident_head, incident_interpolation) = parse_tab2(section, &mut cursor)?;
    require_linear_interpolation(&incident_interpolation, nuclide, "incident energy")?;
    let incident_count = positive_usize(incident_head.n2)?;
    let mut distributions = Vec::with_capacity(incident_count);
    let mut redundant_zero_density_endpoint_count = 0_u64;
    for incident_index in 0..incident_count {
        let (angle_head, angle_interpolation) =
            parse_tab2(section, &mut cursor).map_err(|source| {
                EndfMf6Law7ImplicitResidualError::Law7Tabulation {
                    context: format!("incident node {incident_index} angle table"),
                    source,
                }
            })?;
        require_linear_interpolation(&angle_interpolation, nuclide, "emission cosine")?;
        let incident_energy_ev = value(angle_head, 1)?;
        let angle_count = positive_usize(angle_head.n2)?;
        let mut angular_normalizations = Vec::with_capacity(angle_count);
        let mut angular_first_moments = Vec::with_capacity(angle_count);
        for angle_index in 0..angle_count {
            let (spectrum_head, spectrum, collapsed_endpoint_count) = parse_law7_spectrum_tab1(
                section,
                &mut cursor,
            )
            .map_err(|source| EndfMf6Law7ImplicitResidualError::Law7Tabulation {
                context: format!(
                    "incident node {incident_index} emission-cosine node {angle_index} spectrum"
                ),
                source,
            })?;
            redundant_zero_density_endpoint_count += collapsed_endpoint_count;
            require_linear_interpolation(&spectrum.interpolation, nuclide, "outgoing energy")?;
            let emission_cosine = value(spectrum_head, 1)?;
            if !(-1.0..=1.0).contains(&emission_cosine) {
                return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
            }
            let (normalization, first_moment_ev, _) =
                spectrum.integrate_moments().map_err(|source| {
                    EndfMf6Law7ImplicitResidualError::Law7Tabulation {
                        context: format!(
                            "incident node {incident_index} emission-cosine node {angle_index} integration"
                        ),
                        source,
                    }
                })?;
            angular_normalizations.push((emission_cosine, normalization));
            angular_first_moments.push((emission_cosine, first_moment_ev));
        }
        let normalization = integrate_angle(&angular_normalizations)?;
        let first_moment_ev = integrate_angle(&angular_first_moments)?;
        distributions.push(Law7DistributionMoment {
            incident_energy_ev,
            normalization,
            first_moment_ev,
        });
    }
    require_consumed(section, cursor)?;
    if distributions.is_empty()
        || distributions
            .windows(2)
            .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
    }
    Ok(File6Law7Breakup {
        target_zap,
        target_atomic_weight_ratio,
        emitted_neutron_zap,
        emitted_neutron_atomic_weight_ratio,
        yield_function,
        distributions,
        redundant_zero_density_endpoint_count,
    })
}

fn parse_law7_spectrum_tab1(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<(EndfRecord, TabulatedFunction, u64), EndfPhotonMomentError> {
    let head = take_control(section, cursor)?;
    let region_count = positive_usize(head.n1)?;
    let point_count = positive_usize(head.n2)?;
    let interpolation_words = take_words(section, cursor, region_count * 2)?;
    let point_words = take_words(section, cursor, point_count * 2)?;

    let mut previous_upper_point_index = 0_usize;
    for pair in interpolation_words.chunks_exact(2) {
        let upper_point_index = exact_usize_word(pair[0])?;
        let law = exact_i64_word(pair[1])?;
        if upper_point_index < 2
            || upper_point_index > point_count
            || upper_point_index <= previous_upper_point_index
            || law != 2
        {
            return Err(EndfPhotonMomentError::InvalidTabulation);
        }
        previous_upper_point_index = upper_point_index;
    }
    if previous_upper_point_index != point_count {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }

    let mut points: Vec<(f64, f64)> = Vec::with_capacity(point_count);
    let mut collapsed_endpoint_count = 0_u64;
    for pair in point_words.chunks_exact(2) {
        let point = (pair[0], pair[1]);
        if !point.0.is_finite() || !point.1.is_finite() {
            return Err(EndfPhotonMomentError::InvalidTabulation);
        }
        if let Some(previous) = points.last() {
            if point.0 < previous.0 {
                return Err(EndfPhotonMomentError::InvalidTabulation);
            }
            if point.0 == previous.0 {
                if point.1 == 0.0 && previous.1 == 0.0 {
                    collapsed_endpoint_count += 1;
                    continue;
                }
                return Err(EndfPhotonMomentError::InvalidTabulation);
            }
        }
        points.push(point);
    }
    if points.len() < 2 {
        return Err(EndfPhotonMomentError::InvalidTabulation);
    }
    let upper_point_index = points.len();
    Ok((
        head,
        TabulatedFunction {
            interpolation: vec![InterpolationRegion {
                upper_point_index,
                law: 2,
            }],
            points,
        },
        collapsed_endpoint_count,
    ))
}

fn exact_usize_word(number: f64) -> Result<usize, EndfPhotonMomentError> {
    if number.is_finite() && number >= 0.0 && number.fract() == 0.0 && number <= usize::MAX as f64 {
        Ok(number as usize)
    } else {
        Err(EndfPhotonMomentError::InvalidTabulation)
    }
}

fn exact_i64_word(number: f64) -> Result<i64, EndfPhotonMomentError> {
    if number.is_finite()
        && number.fract() == 0.0
        && number >= i64::MIN as f64
        && number <= i64::MAX as f64
    {
        Ok(number as i64)
    } else {
        Err(EndfPhotonMomentError::InvalidTabulation)
    }
}

fn validate_source_topology(
    file3: &File3Breakup,
    file6: &File6Law7Breakup,
    nuclide: &str,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
    if file3.target_zap != file6.target_zap
        || !approximately_equal(
            file3.target_atomic_weight_ratio,
            file6.target_atomic_weight_ratio,
        )
    {
        return Err(EndfMf6Law7ImplicitResidualError::SourceSectionMismatch);
    }
    let residual_awr = file3.target_atomic_weight_ratio + 1.0
        - TWO_NEUTRON_MULTIPLICITY * file6.emitted_neutron_atomic_weight_ratio;
    if residual_awr <= 0.0 {
        return unsupported(
            nuclide,
            "mass accounting does not leave a physical proton residual",
        );
    }
    Ok(())
}

fn require_linear_interpolation(
    interpolation: &[InterpolationRegion],
    nuclide: &str,
    coordinate: &str,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
    if interpolation.iter().any(|region| region.law != 2) {
        return unsupported(
            nuclide,
            format!("LAW=7 {coordinate} interpolation is not entirely lin-lin"),
        );
    }
    Ok(())
}

fn integrate_angle(points: &[(f64, f64)]) -> Result<f64, EndfMf6Law7ImplicitResidualError> {
    if points.len() < 2
        || !approximately_equal(points[0].0, -1.0)
        || !approximately_equal(points[points.len() - 1].0, 1.0)
        || points
            .iter()
            .any(|(mu, ordinate)| !mu.is_finite() || !ordinate.is_finite() || *ordinate < 0.0)
        || points.windows(2).any(|pair| pair[0].0 >= pair[1].0)
    {
        return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
    }
    Ok(points
        .windows(2)
        .map(|pair| (pair[0].1 + pair[1].1) * (pair[1].0 - pair[0].0) / 2.0)
        .sum())
}

fn validate_sample(
    report: &EndfMf6Law7ImplicitResidualReport,
    sample: &EndfMf6Law7ImplicitResidualSample,
    previous_energy: Option<f64>,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
    let nonnegative = [
        sample.incident_energy_ev,
        sample.reaction_cross_section_barns,
        sample.neutron_yield,
        sample.distribution_normalization,
        sample.absolute_normalization_error,
        sample.raw_first_neutron_energy_moment_ev,
        sample.normalized_mean_neutron_energy_ev,
        sample.total_transported_neutron_energy_ev,
        sample.available_reaction_energy_ev,
        sample.negative_relative_residual_energy,
    ];
    if nonnegative
        .iter()
        .any(|number| !number.is_finite() || *number < 0.0)
        || sample.incident_energy_ev == 0.0
        || sample.reaction_cross_section_barns == 0.0
        || sample.distribution_normalization == 0.0
        || !sample.implicit_residual_energy_ev.is_finite()
        || !sample.implicit_local_kerma_ev_barns.is_finite()
        || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
    {
        return invalid_report("invalid or unordered LAW=7 source sample");
    }
    let expected_mean =
        sample.raw_first_neutron_energy_moment_ev / sample.distribution_normalization;
    let expected_total = sample.neutron_yield * expected_mean;
    let expected_available = sample.incident_energy_ev + report.mass_difference_q_value_ev;
    let expected_residual = expected_available - expected_total;
    let expected_negative_relative = (-expected_residual).max(0.0) / expected_available.max(1.0);
    if !approximately_equal(sample.neutron_yield, report.neutron_multiplicity)
        || !approximately_equal(
            sample.absolute_normalization_error,
            (sample.distribution_normalization - 1.0).abs(),
        )
        || !approximately_equal(sample.normalized_mean_neutron_energy_ev, expected_mean)
        || !approximately_equal(sample.total_transported_neutron_energy_ev, expected_total)
        || !approximately_equal(sample.available_reaction_energy_ev, expected_available)
        || !approximately_equal(sample.implicit_residual_energy_ev, expected_residual)
        || !approximately_equal(
            sample.negative_relative_residual_energy,
            expected_negative_relative,
        )
        || !approximately_equal(
            sample.implicit_local_kerma_ev_barns,
            expected_residual * sample.reaction_cross_section_barns,
        )
    {
        return invalid_report("LAW=7 source sample derived values do not close");
    }
    if sample.status
        != sample_status(
            sample.absolute_normalization_error,
            sample.negative_relative_residual_energy,
            report.normalization_tolerance,
            report.relative_energy_tolerance,
        )
    {
        return invalid_report("LAW=7 source sample status does not match its errors");
    }
    Ok(())
}

fn sample_status(
    normalization_error: f64,
    negative_relative_residual: f64,
    normalization_tolerance: f64,
    relative_energy_tolerance: f64,
) -> EndfMf6Law7ImplicitResidualSampleStatus {
    match (
        normalization_error > normalization_tolerance,
        negative_relative_residual > relative_energy_tolerance,
    ) {
        (false, false) => EndfMf6Law7ImplicitResidualSampleStatus::WithinTolerance,
        (true, false) => {
            EndfMf6Law7ImplicitResidualSampleStatus::SpectrumNormalizationOutsideTolerance
        }
        (false, true) => {
            EndfMf6Law7ImplicitResidualSampleStatus::NegativeImplicitResidualEnergyOutsideTolerance
        }
        (true, true) => EndfMf6Law7ImplicitResidualSampleStatus::
            SpectrumNormalizationAndResidualEnergyOutsideTolerance,
    }
}

fn qualification(
    failed_normalization_sample_count: u64,
    failed_residual_energy_sample_count: u64,
) -> EndfMf6Law7ImplicitResidualQualification {
    match (
        failed_normalization_sample_count > 0,
        failed_residual_energy_sample_count > 0,
    ) {
        (true, false) => EndfMf6Law7ImplicitResidualQualification::SpectrumNormalizationRejected,
        (false, true) => {
            EndfMf6Law7ImplicitResidualQualification::NegativeImplicitResidualEnergyRejected
        }
        (true, true) => {
            EndfMf6Law7ImplicitResidualQualification::SpectrumNormalizationAndResidualEnergyRejected
        }
        (false, false) => {
            EndfMf6Law7ImplicitResidualQualification::ImplicitResidualEnergyCheckedUnreviewed
        }
    }
}

fn validate_tolerances(
    normalization_tolerance: f64,
    relative_energy_tolerance: f64,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
    if !normalization_tolerance.is_finite()
        || normalization_tolerance <= 0.0
        || normalization_tolerance > 1.0e-2
    {
        return invalid_report("normalization tolerance must be in (0, 1e-2]");
    }
    if !relative_energy_tolerance.is_finite()
        || relative_energy_tolerance <= 0.0
        || relative_energy_tolerance > 1.0e-2
    {
        return invalid_report("relative energy tolerance must be in (0, 1e-2]");
    }
    Ok(())
}

fn exact_u32(number: f64) -> Result<u32, EndfMf6Law7ImplicitResidualError> {
    if !number.is_finite() || number < 0.0 || number.fract() != 0.0 || number > u32::MAX as f64 {
        return Err(EndfMf6Law7ImplicitResidualError::InvalidPhysicalValue);
    }
    Ok(number as u32)
}

fn report_id(selection_id: &str, nuclide: &str) -> String {
    format!(
        "{selection_id}.{REPORT_ID_SUFFIX}.{}.mt{BREAKUP_MT}",
        nuclide.to_ascii_lowercase()
    )
}

fn validate_reference(
    label: &'static str,
    reference: &ContentReference,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_sha256(
    label: &'static str,
    digest: &str,
) -> Result<(), EndfMf6Law7ImplicitResidualError> {
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

fn approximately_equal(left: f64, right: f64) -> bool {
    let scale = left.abs().max(right.abs()).max(1.0);
    (left - right).abs() <= 1.0e-12 * scale
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, EndfMf6Law7ImplicitResidualError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|source| EndfMf6Law7ImplicitResidualError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.file_type().is_file() {
        return Err(EndfMf6Law7ImplicitResidualError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| EndfMf6Law7ImplicitResidualError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn unsupported<T>(
    nuclide: &str,
    message: impl Into<String>,
) -> Result<T, EndfMf6Law7ImplicitResidualError> {
    Err(
        EndfMf6Law7ImplicitResidualError::UnsupportedRepresentation {
            nuclide: nuclide.into(),
            message: message.into(),
        },
    )
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, EndfMf6Law7ImplicitResidualError> {
    Err(EndfMf6Law7ImplicitResidualError::InvalidReport(
        message.into(),
    ))
}

#[derive(Debug, Error)]
pub enum EndfMf6Law7ImplicitResidualError {
    #[error(transparent)]
    EvaluatedSource(#[from] EvaluatedSourceError),
    #[error(transparent)]
    PhotonInventory(#[from] EndfPhotonInventoryError),
    #[error(transparent)]
    PhotonMoment(#[from] EndfPhotonMomentError),
    #[error("nuclide {0} is not present in the evaluated-source selection")]
    MissingNuclide(String),
    #[error("LAW=7 source inventory does not bind the exact source evaluation")]
    InventoryBindingMismatch,
    #[error("MF=3 and MF=6 breakup sections disagree on target identity")]
    SourceSectionMismatch,
    #[error("unsupported MF=6 LAW=7 representation for {nuclide}: {message}")]
    UnsupportedRepresentation { nuclide: String, message: String },
    #[error("invalid physical value in MF=3 or MF=6 breakup data")]
    InvalidPhysicalValue,
    #[error("invalid LAW=7 tabulation at {context}: {source}")]
    Law7Tabulation {
        context: String,
        #[source]
        source: EndfPhotonMomentError,
    },
    #[error("invalid MF=6 LAW=7 implicit-residual report: {0}")]
    InvalidReport(String),
    #[error("stored MF=6 LAW=7 implicit-residual report does not match regenerated evidence")]
    ReportMismatch,
    #[error("required LAW=7 source artifact is not a regular file: {0}")]
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

    const JEFF40_H2_REPORT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-mf6-mt16-law7-implicit-residual.json"
    );

    #[test]
    fn validates_frozen_h2_law7_implicit_residual_evidence() {
        let document =
            EndfMf6Law7ImplicitResidualReportDocument::from_bytes(JEFF40_H2_REPORT).unwrap();
        assert_eq!(
            document.sha256,
            "0cfaaf52c67f359b3fd2c70b147e92dd9e004e3495bb860f9ad5ab7707acd1d5"
        );
        assert_eq!(
            document.report.qualification,
            EndfMf6Law7ImplicitResidualQualification::ImplicitResidualEnergyCheckedUnreviewed
        );
        assert_eq!(document.report.source_incident_node_count, 54);
        assert_eq!(document.report.zero_cross_section_boundary_node_count, 1);
        assert_eq!(document.report.redundant_zero_density_endpoint_count, 1);
        assert_eq!(document.report.sample_count, 53);
        assert_eq!(document.report.failed_normalization_sample_count, 0);
        assert_eq!(document.report.failed_residual_energy_sample_count, 0);
        assert_eq!(
            document.report.maximum_absolute_normalization_error,
            5.673713188159013e-8
        );
        assert_eq!(
            document.report.samples[0].implicit_residual_energy_ev,
            443_111.1989433097
        );
        let nine_mev = document
            .report
            .samples
            .iter()
            .find(|sample| sample.incident_energy_ev == 9.0e6)
            .unwrap();
        assert_eq!(
            nine_mev.normalized_mean_neutron_energy_ev,
            2_228_133.1701294947
        );
        assert_eq!(nine_mev.implicit_residual_energy_ev, 2_319_167.6597410105);
    }

    #[test]
    fn rejects_tampered_h2_law7_derived_energy() {
        let mut report = EndfMf6Law7ImplicitResidualReportDocument::from_bytes(JEFF40_H2_REPORT)
            .unwrap()
            .report;
        report.samples[0].implicit_residual_energy_ev += 1.0;
        assert!(matches!(
            report.validate(),
            Err(EndfMf6Law7ImplicitResidualError::InvalidReport(_))
        ));
    }
}
