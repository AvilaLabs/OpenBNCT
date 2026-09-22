// SPDX-License-Identifier: MIT

//! Independent reaction-level energy-balance calculation for ENDF File 6
//! multi-product LAW=1 evaluations.
//!
//! This calculator does not call NJOY and does not consume processed heating
//! data. It parses the source evaluation's File 3 cross sections and File 6
//! product distributions, computes each product's mean laboratory outgoing
//! energy at a finding energy (including the centre-of-mass to laboratory
//! transform and Kalbach-Mann angular corrections), and forms the independent
//! reaction remainder
//!
//! ```text
//! h_MT(E) = sigma_MT(E) * [ (E + Q_MT) - sum_p yield_p(E) * ebar_p,lab(E) ]
//! ```
//!
//! which is compared against NJOY's printed `ebal` remainders and the MT=301
//! kinematic excess. This is unreviewed source-level evidence: every retained
//! finding still requires independent physical review before any response
//! qualification can resume.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::energy_balance_attribution::NjoyEnergyBalanceAttributionDocument;
use crate::photon_inventory::{EndfRecord, ParsedSection, parse_evaluation_sections};
use crate::photon_moment::{
    EndfPhotonMomentError, InterpolationRegion, find_section, parse_tab2, positive_usize,
    require_consumed, take_control, take_words, value,
};
use crate::{
    NjoyDomainAwareSuitabilityError, NjoyDomainAwareSuitabilityReportDocument, NjoyExecutionError,
    NjoyExecutionReceiptDocument,
};

pub const ENDF_REACTION_ENERGY_BALANCE_SCHEMA: &str = "openbnct.endf-reaction-energy-balance/0.1.0";
pub const DEFAULT_REACTION_BALANCE_RELATIVE_TOLERANCE: f64 = 5.0e-3;

const REPORT_ID_SUFFIX: &str = "endf-reaction-energy-balance-v1";
const PHOTON_ZAP: i64 = 0;
const NEUTRON_ZAP: i64 = 1;
const FILE3: u16 = 3;
const FILE6: u16 = 6;
const EV_PER_MEV: f64 = 1.0e6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfReactionEnergyBalanceReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: EndfReactionBalanceQualification,
    pub evidence_scope: EndfReactionBalanceEvidenceScope,
    pub finding_disposition: EndfReactionBalanceFindingDisposition,
    pub evaluated_source_selection: ContentReference,
    pub energy_balance_attribution: ContentReference,
    pub nuclide: String,
    pub endf_mat: u16,
    pub evaluation_sha256: String,
    pub target_awr_neutron_mass_units: f64,
    pub relative_tolerance: f64,
    pub reaction_mts_evaluated: Vec<u16>,
    pub sample_count: u64,
    pub computed_sample_count: u64,
    pub partially_computed_sample_count: u64,
    pub remainder_matched_sample_count: u64,
    pub remainder_mismatched_sample_count: u64,
    pub maximum_remainder_relative_difference: f64,
    pub maximum_ebar_relative_difference: f64,
    pub ebar_comparison_count: u64,
    pub samples: Vec<EndfReactionBalanceSample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfReactionBalanceQualification {
    SourceRemaindersComputedUnreviewed,
    SourceRemaindersPartiallyComputable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfReactionBalanceEvidenceScope {
    IndependentSourceCalculationUnreviewed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfReactionBalanceFindingDisposition {
    RetainedForIndependentPhysicalValidation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfReactionBalanceSample {
    pub incident_energy_ev: f64,
    pub independent_remainder_sum_ev_barns: f64,
    pub printed_remainder_sum_ev_barns: f64,
    pub processor_mt301_excess_ev_barns: f64,
    pub remainder_printed_relative_difference: f64,
    pub remainder_excess_relative_difference: f64,
    pub reactions: Vec<EndfReactionBalanceReaction>,
    pub status: EndfReactionBalanceSampleStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfReactionBalanceSampleStatus {
    Computed,
    PartiallyComputed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfReactionBalanceReaction {
    pub reaction_mt: u16,
    pub status: EndfReactionBalanceReactionStatus,
    pub not_computable_reason: Option<String>,
    pub q_value_ev: Option<f64>,
    pub cross_section_barns: Option<f64>,
    pub independent_remainder_ev_barns: Option<f64>,
    pub printed_remainder_ev_barns: Option<f64>,
    pub remainder_relative_difference: Option<f64>,
    pub products: Vec<EndfReactionBalanceProduct>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfReactionBalanceReactionStatus {
    Computed,
    NotComputable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfReactionBalanceProduct {
    pub zap: i64,
    pub canonical_za: u32,
    pub awp_neutron_mass_units: f64,
    pub transport_disposition: EndfProductDisposition,
    pub angular_representation: EndfProductAngularRepresentation,
    pub yield_per_reaction: f64,
    pub spectrum_normalization: f64,
    pub mean_cm_energy_ev: f64,
    pub mean_lab_energy_ev: f64,
    pub printed_ebar_ev: Option<f64>,
    pub ebar_relative_difference: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfProductDisposition {
    CarriedAway,
    LocallyDeposited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfProductAngularRepresentation {
    Isotropic,
    KalbachMannTabulatedSlope,
    KalbachMannCalculatedSlope,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfReactionEnergyBalanceDocument {
    pub report: EndfReactionEnergyBalanceReport,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EndfReactionEnergyBalanceResult {
    pub report: EndfReactionEnergyBalanceReport,
    pub report_path: PathBuf,
    pub report_sha256: String,
}

struct Mf3Reaction {
    awr: f64,
    q_value_ev: f64,
    cross_section: ReactionTable,
}

struct Mf6Reaction {
    products: Vec<ProductSubsection>,
}

/// A 1-D tabulated function that tolerates the ENDF-6 discontinuity
/// convention: consecutive points may share the same x, in which case the
/// function steps to the right-hand value at x (used by evaluations to drop
/// a cross section to zero at the evaluation's upper energy bound).
pub(crate) struct ReactionTable {
    interpolation: Vec<InterpolationRegion>,
    points: Vec<(f64, f64)>,
}

impl ReactionTable {
    /// Tabulated x-values (strictly nondecreasing; a repeated x is an ENDF-6
    /// discontinuity whose right-hand value governs).
    pub(crate) fn knot_energies(&self) -> impl Iterator<Item = f64> + '_ {
        self.points.iter().map(|(x, _)| *x)
    }

    /// Interpolation law of each region in order.
    pub(crate) fn interpolation_laws(&self) -> impl Iterator<Item = i64> + '_ {
        self.interpolation.iter().map(|region| region.law)
    }

    pub(crate) fn is_linear_linear(&self) -> bool {
        self.interpolation.iter().all(|region| region.law == 2)
    }

    pub(crate) fn energy_bounds(&self) -> (f64, f64) {
        (
            self.points
                .first()
                .expect("table has at least two points")
                .0,
            self.points.last().expect("table has at least two points").0,
        )
    }

    pub(crate) fn parse(
        section: &ParsedSection,
        cursor: &mut usize,
    ) -> Result<(EndfRecord, Self), EndfReactionBalanceError> {
        let head = take_control_pub(section, cursor)?;
        let region_count = nonnegative_usize(head.n1)?;
        let point_count = positive_usize(head.n2).map_err(map_moment)?;
        let interpolation_words = take_words_pub(section, cursor, region_count * 2)?;
        let point_words = take_words_pub(section, cursor, point_count * 2)?;
        let mut interpolation = Vec::with_capacity(region_count);
        for pair in interpolation_words.chunks_exact(2) {
            let upper_point_index = pair[0] as usize;
            if upper_point_index as f64 != pair[0]
                || upper_point_index < 2
                || upper_point_index > point_count
                || interpolation
                    .last()
                    .is_some_and(|previous: &InterpolationRegion| {
                        previous.upper_point_index >= upper_point_index
                    })
            {
                return Err(EndfReactionBalanceError::InvalidTabulation);
            }
            let law = pair[1] as i64;
            if law as f64 != pair[1] {
                return Err(EndfReactionBalanceError::InvalidTabulation);
            }
            interpolation.push(InterpolationRegion {
                upper_point_index,
                law,
            });
        }
        if interpolation.last().map(|region| region.upper_point_index) != Some(point_count) {
            return Err(EndfReactionBalanceError::InvalidTabulation);
        }
        let points = point_words
            .chunks_exact(2)
            .map(|pair| (pair[0], pair[1]))
            .collect::<Vec<_>>();
        if points.len() < 2
            || points.iter().any(|(x, y)| !x.is_finite() || !y.is_finite())
            || points.windows(2).any(|pair| pair[0].0 > pair[1].0)
        {
            return Err(EndfReactionBalanceError::InvalidTabulation);
        }
        Ok((
            head,
            Self {
                interpolation,
                points,
            },
        ))
    }

    /// Evaluates the function at `x`. A point that shares its x with the next
    /// point (a discontinuity) resolves to the right-hand value; energies
    /// strictly inside a zero-width segment are unreachable.
    pub(crate) fn evaluate(&self, x: f64) -> Option<f64> {
        if !x.is_finite() || x < self.points.first()?.0 || x > self.points.last()?.0 {
            return None;
        }
        if let Some(index) = self.points.iter().rposition(|(point, _)| *point == x) {
            return Some(self.points[index].1);
        }
        let segment = self
            .points
            .windows(2)
            .position(|pair| pair[0].0 < x && x < pair[1].0)?;
        let (x0, y0) = self.points[segment];
        let (x1, y1) = self.points[segment + 1];
        let upper_point_index = segment + 2;
        let law = self
            .interpolation
            .iter()
            .find(|region| upper_point_index <= region.upper_point_index)
            .map(|region| region.law)?;
        match law {
            1 => Some(y0),
            2 => Some(y0 + (y1 - y0) * (x - x0) / (x1 - x0)),
            _ => None,
        }
    }
}

/// Parsed File 6 product subsection for LAW=1 distributions.
struct ProductSubsection {
    zap: i64,
    canonical_za: u32,
    awp: f64,
    lang: i64,
    lep: i64,
    yield_function: ReactionTable,
    incident_interpolation: Vec<InterpolationRegion>,
    distributions: Vec<ProductDistribution>,
}

struct ProductDistribution {
    incident_energy_ev: f64,
    /// Discrete outgoing lines `(E'_d, p_d)`; isotropic by ENDF convention.
    discrete: Vec<(f64, f64)>,
    /// Continuum points `(E', p(E'), angular_params)` where angular params are
    /// the NA Kalbach-Mann/Legendre parameters accompanying each point.
    continuum: Vec<(f64, f64, Vec<f64>)>,
}

#[derive(Debug, Default)]
struct PrintedMtTables {
    /// `ebal` remainders summed over the MT's tables, by incident energy.
    remainders: BTreeMap<u64, f64>,
    /// Printed product `ebar` values by `(particle_id, energy_bits)`.
    ebars: BTreeMap<(u32, u64), f64>,
}

impl EndfReactionEnergyBalanceReport {
    #[allow(clippy::too_many_arguments)]
    pub fn calculate(
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        attribution: &NjoyEnergyBalanceAttributionDocument,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        nuclide: &str,
        relative_tolerance: f64,
    ) -> Result<Self, EndfReactionBalanceError> {
        validate_tolerance(relative_tolerance)?;
        if attribution.attribution.domain_aware_suitability_report.id != domain.report.id
            || attribution
                .attribution
                .domain_aware_suitability_report
                .sha256
                != domain.sha256
            || attribution.attribution.execution_receipt.id != execution.receipt.id
            || attribution.attribution.execution_receipt.sha256 != execution.sha256
            || attribution.attribution.nuclide != nuclide
        {
            return Err(EndfReactionBalanceError::EvidenceBindingMismatch);
        }
        execution.verify_execution_root(execution_root)?;

        let artifact = selection
            .selection
            .evaluations
            .iter()
            .find(|artifact| artifact.nuclide == nuclide)
            .ok_or_else(|| EndfReactionBalanceError::MissingEvaluation(nuclide.into()))?;
        let path = evaluations_root.join(&artifact.extracted_filename);
        let bytes = read_regular_file(&path)?;
        let evaluation_sha256 = sha256_bytes(&bytes);
        if evaluation_sha256 != artifact.sha256 {
            return Err(EndfReactionBalanceError::EvaluationDigestMismatch(
                artifact.extracted_filename.clone(),
            ));
        }

        // Collect the MTs present in both File 3 and File 6 so only those
        // sections need to be parsed.
        let text =
            std::str::from_utf8(&bytes).map_err(|_| EndfReactionBalanceError::NonUtf8Evaluation)?;
        let mut mf3_mts = std::collections::BTreeSet::new();
        let mut mf6_mts = std::collections::BTreeSet::new();
        for line in text.lines() {
            if line.len() < 75 {
                continue;
            }
            if let (Ok(mf), Ok(mt)) = (
                line[70..72].trim().parse::<u16>(),
                line[72..75].trim().parse::<u16>(),
            ) {
                if mf == FILE3 && mt > 0 {
                    mf3_mts.insert(mt);
                } else if mf == FILE6 && mt > 0 {
                    mf6_mts.insert(mt);
                }
            }
        }
        let shared: Vec<u16> = mf3_mts.intersection(&mf6_mts).copied().collect();
        if shared.is_empty() {
            return Err(EndfReactionBalanceError::NoSharedSections);
        }
        let mut selected = Vec::with_capacity(shared.len() * 2);
        for mt in &shared {
            selected.push((FILE3, *mt));
            selected.push((FILE6, *mt));
        }
        let sections = parse_evaluation_sections(&bytes, artifact.endf_mat, &selected)
            .map_err(EndfReactionBalanceError::SectionParse)?;

        let mut reactions = Vec::new();
        for mt in &shared {
            let file3 = find_section(&sections, FILE3, *mt)
                .map_err(|_| EndfReactionBalanceError::NoSharedSections)?;
            let file6 = find_section(&sections, FILE6, *mt)
                .map_err(|_| EndfReactionBalanceError::NoSharedSections)?;
            let parsed = parse_mf3_reaction(file3)
                .and_then(|mf3| parse_mf6_reaction(file6).map(|mf6| (mf3, mf6)));
            reactions.push((*mt, parsed));
        }
        let target_awr = reactions
            .iter()
            .find_map(|(_, parsed)| parsed.as_ref().ok().map(|(mf3, _)| mf3.awr))
            .ok_or_else(|| physics("target AWR"))?;

        let report_path = execution_root.join(&attribution.attribution.processor_report.path);
        let print_bytes = read_regular_file(&report_path)?;
        if print_bytes.len() as u64 != attribution.attribution.processor_report.size_bytes
            || sha256_bytes(&print_bytes) != attribution.attribution.processor_report.sha256
        {
            return Err(EndfReactionBalanceError::ProcessorReportChanged(
                attribution.attribution.processor_report.path.clone(),
            ));
        }
        let print_text = std::str::from_utf8(&print_bytes)
            .map_err(|_| EndfReactionBalanceError::NonUtf8ProcessorReport(report_path.clone()))?;
        let printed = parse_file6_tables(print_text)?;

        let mut samples = Vec::with_capacity(attribution.attribution.samples.len());
        for finding in &attribution.attribution.samples {
            let energy = finding.incident_energy_ev;
            let mut reaction_rows = Vec::new();
            let mut independent_sum = 0.0;
            let mut partial = false;
            for (mt, parsed) in &reactions {
                let printed_remainder = printed
                    .get(mt)
                    .and_then(|tables| tables.remainders.get(&energy.to_bits()).copied());
                let row = match parsed {
                    Err(EndfReactionBalanceError::UnsupportedRepresentation {
                        message, ..
                    }) => {
                        partial = true;
                        EndfReactionBalanceReaction::not_computable(
                            *mt,
                            message.clone(),
                            printed_remainder,
                        )
                    }
                    Err(error) => {
                        return Err(EndfReactionBalanceError::InvalidReactionSection {
                            reaction_mt: *mt,
                            message: error.to_string(),
                        });
                    }
                    Ok((mf3, mf6)) => {
                        match reaction_remainder(mf3, mf6, energy, *mt, target_awr, &printed) {
                            Ok(Some(row)) => {
                                independent_sum +=
                                    row.independent_remainder_ev_barns.unwrap_or(0.0);
                                row
                            }
                            Ok(None) => continue,
                            Err(EndfReactionBalanceError::UnsupportedRepresentation {
                                message,
                                ..
                            }) => {
                                partial = true;
                                EndfReactionBalanceReaction::not_computable(
                                    *mt,
                                    message,
                                    printed_remainder,
                                )
                            }
                            Err(error) => return Err(error),
                        }
                    }
                };
                reaction_rows.push(row);
            }
            let printed_sum = finding.printed_energy_balance_remainder_sum_ev_barns;
            let processor_excess = finding.processor_final_mt301_excess_ev_barns;
            samples.push(EndfReactionBalanceSample {
                incident_energy_ev: energy,
                independent_remainder_sum_ev_barns: independent_sum,
                printed_remainder_sum_ev_barns: printed_sum,
                processor_mt301_excess_ev_barns: processor_excess,
                remainder_printed_relative_difference: relative_difference(
                    independent_sum,
                    printed_sum,
                ),
                remainder_excess_relative_difference: relative_difference(
                    independent_sum,
                    processor_excess,
                ),
                reactions: reaction_rows,
                status: if partial {
                    EndfReactionBalanceSampleStatus::PartiallyComputed
                } else {
                    EndfReactionBalanceSampleStatus::Computed
                },
            });
        }

        let sample_count = samples.len() as u64;
        let computed_sample_count = samples
            .iter()
            .filter(|sample| sample.status == EndfReactionBalanceSampleStatus::Computed)
            .count() as u64;
        let partially_computed_sample_count = sample_count - computed_sample_count;
        let remainder_matched_sample_count = samples
            .iter()
            .filter(|sample| {
                sample.status == EndfReactionBalanceSampleStatus::Computed
                    && sample.remainder_printed_relative_difference <= relative_tolerance
            })
            .count() as u64;
        let remainder_mismatched_sample_count =
            computed_sample_count.saturating_sub(remainder_matched_sample_count);
        let maximum_remainder_relative_difference = samples
            .iter()
            .map(|sample| sample.remainder_printed_relative_difference)
            .fold(0.0_f64, f64::max);
        let mut maximum_ebar_relative_difference = 0.0_f64;
        let mut ebar_comparison_count = 0_u64;
        for sample in &samples {
            for reaction in &sample.reactions {
                for product in &reaction.products {
                    if let Some(difference) = product.ebar_relative_difference {
                        ebar_comparison_count += 1;
                        maximum_ebar_relative_difference =
                            maximum_ebar_relative_difference.max(difference);
                    }
                }
            }
        }

        let report = Self {
            schema_version: ENDF_REACTION_ENERGY_BALANCE_SCHEMA.into(),
            id: format!(
                "{}.{}.{}",
                selection.selection.id,
                nuclide.to_ascii_lowercase(),
                REPORT_ID_SUFFIX
            ),
            case_id: selection.selection.case_id.clone(),
            qualification: if partially_computed_sample_count == 0 {
                EndfReactionBalanceQualification::SourceRemaindersComputedUnreviewed
            } else {
                EndfReactionBalanceQualification::SourceRemaindersPartiallyComputable
            },
            evidence_scope:
                EndfReactionBalanceEvidenceScope::IndependentSourceCalculationUnreviewed,
            finding_disposition:
                EndfReactionBalanceFindingDisposition::RetainedForIndependentPhysicalValidation,
            evaluated_source_selection: ContentReference {
                id: selection.selection.id.clone(),
                sha256: selection.sha256.clone(),
            },
            energy_balance_attribution: ContentReference {
                id: attribution.attribution.id.clone(),
                sha256: attribution.sha256.clone(),
            },
            nuclide: nuclide.into(),
            endf_mat: artifact.endf_mat,
            evaluation_sha256,
            target_awr_neutron_mass_units: target_awr,
            relative_tolerance,
            reaction_mts_evaluated: shared,
            sample_count,
            computed_sample_count,
            partially_computed_sample_count,
            remainder_matched_sample_count,
            remainder_mismatched_sample_count,
            maximum_remainder_relative_difference,
            maximum_ebar_relative_difference,
            ebar_comparison_count,
            samples,
        };
        report.validate()?;
        Ok(report)
    }

    pub fn validate(&self) -> Result<(), EndfReactionBalanceError> {
        if !openbnct_core::schema_matches(&self.schema_version, ENDF_REACTION_ENERGY_BALANCE_SCHEMA)
        {
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
            "energy_balance_attribution",
            &self.energy_balance_attribution,
        )?;
        validate_sha256("evaluation_sha256", &self.evaluation_sha256)?;
        validate_tolerance(self.relative_tolerance)?;
        if self.endf_mat == 0
            || !self.target_awr_neutron_mass_units.is_finite()
            || self.target_awr_neutron_mass_units <= 0.0
            || self.id
                != format!(
                    "{}.{}.{}",
                    self.evaluated_source_selection.id,
                    self.nuclide.to_ascii_lowercase(),
                    REPORT_ID_SUFFIX
                )
            || self.evidence_scope
                != EndfReactionBalanceEvidenceScope::IndependentSourceCalculationUnreviewed
            || self.finding_disposition
                != EndfReactionBalanceFindingDisposition::RetainedForIndependentPhysicalValidation
            || self.reaction_mts_evaluated.is_empty()
            || self
                .reaction_mts_evaluated
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.samples.is_empty()
            || self.sample_count != self.samples.len() as u64
            || self.computed_sample_count + self.partially_computed_sample_count
                != self.sample_count
            || self.remainder_matched_sample_count + self.remainder_mismatched_sample_count
                != self.computed_sample_count
        {
            return invalid_report("report identity, scope, or counts are inconsistent");
        }
        let mut previous_energy = None;
        for sample in &self.samples {
            if !sample.incident_energy_ev.is_finite()
                || sample.incident_energy_ev <= 0.0
                || previous_energy.is_some_and(|previous| previous >= sample.incident_energy_ev)
                || !approximately_equal(
                    sample.remainder_printed_relative_difference,
                    relative_difference(
                        sample.independent_remainder_sum_ev_barns,
                        sample.printed_remainder_sum_ev_barns,
                    ),
                )
                || !approximately_equal(
                    sample.remainder_excess_relative_difference,
                    relative_difference(
                        sample.independent_remainder_sum_ev_barns,
                        sample.processor_mt301_excess_ev_barns,
                    ),
                )
            {
                return invalid_report("invalid or unordered balance sample");
            }
            previous_energy = Some(sample.incident_energy_ev);
            let mut previous_mt = None;
            for reaction in &sample.reactions {
                if reaction.reaction_mt == 0
                    || previous_mt.is_some_and(|previous| previous >= reaction.reaction_mt)
                {
                    return invalid_report("invalid or unordered reaction row");
                }
                previous_mt = Some(reaction.reaction_mt);
                if let (Some(independent), Some(printed)) = (
                    reaction.independent_remainder_ev_barns,
                    reaction.printed_remainder_ev_barns,
                ) && !approximately_equal(
                    reaction.remainder_relative_difference.unwrap_or(f64::NAN),
                    relative_difference(independent, printed),
                ) {
                    return invalid_report("reaction remainder difference does not close");
                }
                for product in &reaction.products {
                    if product.awp_neutron_mass_units < 0.0
                        || !product.awp_neutron_mass_units.is_finite()
                        || !product.mean_cm_energy_ev.is_finite()
                        || !product.mean_lab_energy_ev.is_finite()
                        || !product.spectrum_normalization.is_finite()
                        || !product.yield_per_reaction.is_finite()
                        || product.spectrum_normalization < 0.0
                        || product.yield_per_reaction < 0.0
                        || product.mean_cm_energy_ev < 0.0
                        || product.mean_lab_energy_ev < 0.0
                    {
                        return invalid_report("invalid product energy balance row");
                    }
                    if let (Some(printed), Some(difference)) =
                        (product.printed_ebar_ev, product.ebar_relative_difference)
                        && !approximately_equal(
                            difference,
                            relative_difference(product.mean_lab_energy_ev, printed),
                        )
                    {
                        return invalid_report("product ebar difference does not close");
                    }
                }
            }
        }
        let matched = self
            .samples
            .iter()
            .filter(|sample| {
                sample.status == EndfReactionBalanceSampleStatus::Computed
                    && sample.remainder_printed_relative_difference <= self.relative_tolerance
            })
            .count() as u64;
        if self.remainder_matched_sample_count != matched {
            return invalid_report("remainder match count does not match samples");
        }
        let expected_qualification = if self.partially_computed_sample_count == 0 {
            EndfReactionBalanceQualification::SourceRemaindersComputedUnreviewed
        } else {
            EndfReactionBalanceQualification::SourceRemaindersPartiallyComputable
        };
        if self.qualification != expected_qualification {
            return invalid_report("qualification does not match sample results");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<EndfReactionEnergyBalanceResult, EndfReactionBalanceError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| EndfReactionBalanceError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| EndfReactionBalanceError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(EndfReactionEnergyBalanceResult {
            report: self.clone(),
            report_path: path.to_path_buf(),
            report_sha256: sha256_bytes(&bytes),
        })
    }
}

impl EndfReactionBalanceReaction {
    fn not_computable(reaction_mt: u16, reason: String, printed_remainder: Option<f64>) -> Self {
        Self {
            reaction_mt,
            status: EndfReactionBalanceReactionStatus::NotComputable,
            not_computable_reason: Some(reason),
            q_value_ev: None,
            cross_section_barns: None,
            independent_remainder_ev_barns: None,
            printed_remainder_ev_barns: printed_remainder,
            remainder_relative_difference: None,
            products: Vec::new(),
        }
    }
}

impl EndfReactionEnergyBalanceDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EndfReactionBalanceError> {
        let report: EndfReactionEnergyBalanceReport = serde_json::from_slice(bytes)?;
        report.validate()?;
        Ok(Self {
            report,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, EndfReactionBalanceError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn verify_against_sources(
        &self,
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
        attribution: &NjoyEnergyBalanceAttributionDocument,
        domain: &NjoyDomainAwareSuitabilityReportDocument,
        execution: &NjoyExecutionReceiptDocument,
        execution_root: &Path,
        nuclide: &str,
    ) -> Result<(), EndfReactionBalanceError> {
        let observed = EndfReactionEnergyBalanceReport::calculate(
            selection,
            evaluations_root,
            attribution,
            domain,
            execution,
            execution_root,
            nuclide,
            self.report.relative_tolerance,
        )?;
        if self.report != observed {
            return Err(EndfReactionBalanceError::ReportMismatch);
        }
        Ok(())
    }
}

fn parse_mf3_reaction(section: &ParsedSection) -> Result<Mf3Reaction, EndfReactionBalanceError> {
    let mut cursor = 0_usize;
    let head = take_control_pub(section, &mut cursor)?;
    let awr = value_pub(head, 1)?;
    if !awr.is_finite() || awr <= 0.0 {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    let (tab1_head, cross_section) = ReactionTable::parse(section, &mut cursor)?;
    let qm = value_pub(tab1_head, 0)?;
    let qi = value_pub(tab1_head, 1)?;
    // ENDF-6 File 3 carries the mass-difference Q (C1) and the reaction Q
    // (C2); they may legitimately differ (for example continuum-inelastic
    // MT=91 carries QM=0 and QI=-7.764e6). NJOY's File 6 `ebal` accounting
    // uses the mass-difference Q (C1), verified against the printed `q`.
    if !qm.is_finite() || !qi.is_finite() {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    require_consumed_pub(section, cursor)?;
    Ok(Mf3Reaction {
        awr,
        q_value_ev: qm,
        cross_section,
    })
}

fn parse_mf6_reaction(section: &ParsedSection) -> Result<Mf6Reaction, EndfReactionBalanceError> {
    let mut cursor = 0_usize;
    let head = take_control_pub(section, &mut cursor)?;
    let lct = head.l2;
    if lct != 3 {
        return Err(EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: format!("MF=6 uses LCT={lct}; only LCT=3 is supported"),
        });
    }
    let product_count = positive_usize(head.n1).map_err(map_moment)?;
    let mut products = Vec::with_capacity(product_count);
    for _ in 0..product_count {
        products.push(parse_product(section, &mut cursor)?);
    }
    require_consumed_pub(section, cursor)?;
    Ok(Mf6Reaction { products })
}

fn parse_product(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<ProductSubsection, EndfReactionBalanceError> {
    let (product_head, yield_function) = ReactionTable::parse(section, cursor)?;
    let zap = product_head.c1 as i64;
    if (product_head.c1 - zap as f64).abs() != 0.0 || zap < 0 {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    let awp = value_pub(product_head, 1)?;
    if !awp.is_finite() || awp < 0.0 {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    if product_head.l2 != 1 {
        return Err(EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: format!(
                "MF=6 product uses LAW={}; only LAW=1 is supported",
                product_head.l2
            ),
        });
    }
    let canonical_za = canonical_product_za(zap, awp).ok_or(
        EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: format!("product ZAP={zap} AWP={awp} cannot be canonicalized"),
        },
    )?;
    let (tab2, incident_interpolation) = parse_tab2_pub(section, cursor)?;
    let lang = tab2.l1;
    let lep = tab2.l2;
    if !matches!(lang, 1 | 2) || !matches!(lep, 1 | 2) {
        return Err(EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: format!("MF=6 uses LANG={lang} LEP={lep}; only 1 or 2 are supported"),
        });
    }
    let incident_count = positive_usize(tab2.n2).map_err(map_moment)?;
    let mut distributions = Vec::with_capacity(incident_count);
    for _ in 0..incident_count {
        distributions.push(parse_product_list(section, cursor, lang)?);
    }
    if distributions
        .windows(2)
        .any(|pair| pair[0].incident_energy_ev >= pair[1].incident_energy_ev)
    {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    Ok(ProductSubsection {
        zap,
        canonical_za,
        awp,
        lang,
        lep,
        yield_function,
        incident_interpolation,
        distributions,
    })
}

fn parse_product_list(
    section: &ParsedSection,
    cursor: &mut usize,
    lang: i64,
) -> Result<ProductDistribution, EndfReactionBalanceError> {
    let list = take_control_pub(section, cursor)?;
    let incident_energy_ev = value_pub(list, 1)?;
    let discrete_count = nonnegative_usize(list.l1)?;
    let angular_parameter_count = nonnegative_usize(list.l2)?;
    let word_count = positive_usize(list.n1).map_err(map_moment)?;
    let outgoing_energy_count = positive_usize(list.n2).map_err(map_moment)?;
    if angular_parameter_count > 0 && discrete_count > 0 {
        return Err(EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: "LAW=1 LIST mixes discrete lines with angular parameters".into(),
        });
    }
    if lang == 2 && !matches!(angular_parameter_count, 1 | 2) {
        return Err(EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: format!("LAW=1 LANG=2 requires NA of 1 or 2, found {angular_parameter_count}"),
        });
    }
    if lang == 1 && angular_parameter_count != 0 {
        return Err(EndfReactionBalanceError::UnsupportedRepresentation {
            reaction_mt: section.reaction_mt,
            message: format!("LAW=1 LANG=1 with NA={angular_parameter_count} is not implemented"),
        });
    }
    let expected_words = discrete_count
        .checked_mul(2)
        .and_then(|base| {
            (outgoing_energy_count - discrete_count)
                .checked_mul(angular_parameter_count + 2)
                .and_then(|rest| base.checked_add(rest))
        })
        .ok_or(EndfReactionBalanceError::InvalidTabulation)?;
    if outgoing_energy_count <= discrete_count || word_count != expected_words {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    let words = take_words_pub(section, cursor, word_count)?;
    let mut discrete = Vec::with_capacity(discrete_count);
    for pair in words[..discrete_count * 2].chunks_exact(2) {
        if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] < 0.0 || pair[1] < 0.0 {
            return Err(EndfReactionBalanceError::InvalidTabulation);
        }
        discrete.push((pair[0], pair[1]));
    }
    let mut continuum = Vec::with_capacity(outgoing_energy_count - discrete_count);
    let mut offset = discrete_count * 2;
    for _ in discrete_count..outgoing_energy_count {
        let energy_out = words[offset];
        let density = words[offset + 1];
        let params = words[offset + 2..offset + 2 + angular_parameter_count].to_vec();
        if !energy_out.is_finite()
            || energy_out < 0.0
            || !density.is_finite()
            || density < 0.0
            || params.iter().any(|param| !param.is_finite())
        {
            return Err(EndfReactionBalanceError::InvalidTabulation);
        }
        continuum.push((energy_out, density, params));
        offset += angular_parameter_count + 2;
    }
    if !incident_energy_ev.is_finite()
        || incident_energy_ev < 0.0
        || continuum.windows(2).any(|pair| pair[0].0 >= pair[1].0)
    {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    Ok(ProductDistribution {
        incident_energy_ev,
        discrete,
        continuum,
    })
}

/// Moments of one LAW=1 product spectrum at its tabulated incident energy.
///
/// Returns `(normalization, first_cm_energy_moment, angular_cross_moment)`
/// where the cross moment integrates `p(E') * sqrt(E') * mu_bar(E')` over the
/// continuum. Discrete lines are isotropic and contribute zero cross moment.
/// For Kalbach-Mann (LANG=2) the mean cosine per point is `r * L(a)` where
/// `L` is the Langevin function and `a` is the tabulated slope when NA=2 or
/// the ENDF-6 systematics slope when NA=1.
fn product_spectrum_moments(
    distribution: &ProductDistribution,
    product: &ProductSubsection,
    target_awr: f64,
    reaction_mt: u16,
) -> Result<(f64, f64, f64), EndfReactionBalanceError> {
    let mut normalization = 0.0;
    let mut first_moment = 0.0;
    for (energy, probability) in &distribution.discrete {
        normalization += probability;
        first_moment += energy * probability;
    }
    let mut cross_moment = 0.0;
    if distribution.continuum.len() >= 2 {
        let mut mean_cosines = Vec::with_capacity(distribution.continuum.len());
        for (index, (energy_out, _, params)) in distribution.continuum.iter().enumerate() {
            let mean_cosine = if product.lang == 2 {
                let precompound = params[0];
                if !(0.0..=1.0).contains(&precompound) {
                    return Err(EndfReactionBalanceError::InvalidTabulation);
                }
                // A zero precompound fraction is fully compound (isotropic):
                // the slope is unconstrained there and must not be required.
                if precompound == 0.0 {
                    0.0
                } else {
                    let slope = if params.len() >= 2 {
                        params[1]
                    } else {
                        kalbach_slope(
                            distribution.incident_energy_ev,
                            *energy_out,
                            product.canonical_za,
                            target_awr,
                        )
                        .ok_or(
                            EndfReactionBalanceError::UnsupportedRepresentation {
                                reaction_mt,
                                message: format!(
                                    "Kalbach-Mann slope is undefined at continuum point {index}"
                                ),
                            },
                        )?
                    };
                    if !slope.is_finite() || slope < 0.0 {
                        return Err(EndfReactionBalanceError::InvalidTabulation);
                    }
                    precompound * langevin(slope)
                }
            } else {
                0.0
            };
            mean_cosines.push(mean_cosine);
        }
        for (segment, pair) in distribution.continuum.windows(2).enumerate() {
            let (x0, p0, _) = pair[0];
            let (x1, p1, _) = pair[1];
            match product.lep {
                // Histogram: p and the angular factor are constant on the bin.
                1 => {
                    normalization += p0 * (x1 - x0);
                    first_moment += p0 * (x1 * x1 - x0 * x0) / 2.0;
                    cross_moment +=
                        p0 * mean_cosines[segment] * 2.0 / 3.0 * (x1.powf(1.5) - x0.powf(1.5));
                }
                // Linear-linear: analytic p moments; trapezoid the weighted
                // sqrt(E') integrand with per-point angular factors.
                2 => {
                    normalization += (p0 + p1) * (x1 - x0) / 2.0;
                    let slope = (p1 - p0) / (x1 - x0);
                    first_moment += p0 * (x1 * x1 - x0 * x0) / 2.0
                        + slope
                            * ((x1.powi(3) - x0.powi(3)) / 3.0 - x0 * (x1 * x1 - x0 * x0) / 2.0);
                    let g0 = p0 * x0.sqrt() * mean_cosines[segment];
                    let g1 = p1 * x1.sqrt() * mean_cosines[segment + 1];
                    cross_moment += (g0 + g1) * (x1 - x0) / 2.0;
                }
                law => {
                    return Err(EndfReactionBalanceError::UnsupportedRepresentation {
                        reaction_mt,
                        message: format!("unsupported LEP interpolation law {law}"),
                    });
                }
            }
        }
    }
    if !normalization.is_finite()
        || normalization < 0.0
        || !first_moment.is_finite()
        || first_moment < 0.0
        || !cross_moment.is_finite()
    {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    Ok((normalization, first_moment, cross_moment))
}

/// Interpolated `(normalization, first_moment, cross_moment)` at `energy`,
/// built by the ENDF-6 LAW=1 union-grid convention: the two bracketing
/// spectra are interpolated pointwise on the union of their outgoing-energy
/// grids, and moments are taken of the interpolated spectrum. This is exact
/// for both histogram and linear-linear continuum interpolation.
/// Returns `None` when the energy is outside the incident grid.
fn product_moments_at(
    product: &ProductSubsection,
    target_awr: f64,
    energy: f64,
    reaction_mt: u16,
) -> Result<Option<(f64, f64, f64)>, EndfReactionBalanceError> {
    if product.distributions.is_empty() {
        return Ok(None);
    }
    let energies: Vec<f64> = product
        .distributions
        .iter()
        .map(|distribution| distribution.incident_energy_ev)
        .collect();
    if energy < energies[0] || energy > *energies.last().unwrap_or(&0.0) {
        return Ok(None);
    }
    if let Some(index) = energies.iter().position(|grid| *grid == energy) {
        return product_spectrum_moments(
            &product.distributions[index],
            product,
            target_awr,
            reaction_mt,
        )
        .map(Some);
    }
    let segment = energies
        .windows(2)
        .position(|pair| pair[0] < energy && energy < pair[1])
        .ok_or(EndfReactionBalanceError::InvalidTabulation)?;
    // Incident interpolation law for this segment: the governing region's
    // upper point index (1-based) must cover the segment's right endpoint.
    let upper_point_index = segment + 2;
    let law = product
        .incident_interpolation
        .iter()
        .find(|region| upper_point_index <= region.upper_point_index)
        .map(|region| region.law)
        .ok_or(EndfReactionBalanceError::InvalidTabulation)?;
    let (e0, e1) = (energies[segment], energies[segment + 1]);
    let weight = match law {
        1 => 0.0,
        2 => (energy - e0) / (e1 - e0),
        law => {
            return Err(EndfReactionBalanceError::UnsupportedRepresentation {
                reaction_mt,
                message: format!(
                    "unsupported incident interpolation law {law} between MF=6 spectra"
                ),
            });
        }
    };
    let interpolated = interpolate_spectra(
        &product.distributions[segment],
        &product.distributions[segment + 1],
        weight,
        product,
        energy,
    )?;
    product_spectrum_moments(&interpolated, product, target_awr, reaction_mt).map(Some)
}

/// Builds the ENDF-6 LAW=1 union-grid interpolation of two spectra at weight
/// `w` (w=0 reproduces the lower spectrum).
fn interpolate_spectra(
    lower: &ProductDistribution,
    upper: &ProductDistribution,
    weight: f64,
    product: &ProductSubsection,
    energy: f64,
) -> Result<ProductDistribution, EndfReactionBalanceError> {
    let parameter_count = lower
        .continuum
        .first()
        .or_else(|| upper.continuum.first())
        .map(|(_, _, params)| params.len())
        .unwrap_or(0);
    // Union of discrete-line energies; a line present in only one spectrum
    // contributes zero probability to the other.
    let mut discrete_energies = std::collections::BTreeSet::new();
    for (energy_out, _) in lower.discrete.iter().chain(upper.discrete.iter()) {
        discrete_energies.insert(energy_out.to_bits());
    }
    let mut discrete = Vec::with_capacity(discrete_energies.len());
    for bits in discrete_energies {
        let energy_out = f64::from_bits(bits);
        let p0 = discrete_probability_at(lower, energy_out);
        let p1 = discrete_probability_at(upper, energy_out);
        discrete.push((energy_out, p0 + weight * (p1 - p0)));
    }
    // Union of continuum grids. A union point outside a spectrum's own range
    // evaluates to zero there.
    let mut grid = std::collections::BTreeSet::new();
    for (energy_out, _, _) in lower.continuum.iter().chain(upper.continuum.iter()) {
        grid.insert(energy_out.to_bits());
    }
    let mut continuum = Vec::with_capacity(grid.len());
    for bits in grid {
        let energy_out = f64::from_bits(bits);
        let (p0, params0) = continuum_at(lower, energy_out, product.lep, parameter_count);
        let (p1, params1) = continuum_at(upper, energy_out, product.lep, parameter_count);
        let density = p0 + weight * (p1 - p0);
        let params = params0
            .iter()
            .zip(params1.iter())
            .map(|(a, b)| a + weight * (b - a))
            .collect();
        if density < 0.0 && density > -1.0e-12 * p0.abs().max(p1.abs()).max(1.0) {
            continuum.push((energy_out, 0.0, params));
        } else if density < 0.0 {
            return Err(EndfReactionBalanceError::InvalidTabulation);
        } else {
            continuum.push((energy_out, density, params));
        }
    }
    Ok(ProductDistribution {
        incident_energy_ev: energy,
        discrete,
        continuum,
    })
}

/// A spectrum's discrete-line probability at `energy` (0 when absent).
fn discrete_probability_at(distribution: &ProductDistribution, energy: f64) -> f64 {
    distribution
        .discrete
        .iter()
        .find(|(line, _)| *line == energy)
        .map(|(_, probability)| *probability)
        .unwrap_or(0.0)
}

/// A spectrum's continuum density and angular parameters at `energy_out`,
/// evaluated with the section's LEP law. Outside the spectrum's own range the
/// density and parameters are zero.
fn continuum_at(
    distribution: &ProductDistribution,
    energy_out: f64,
    lep: i64,
    parameter_count: usize,
) -> (f64, Vec<f64>) {
    let zero = (0.0, vec![0.0; parameter_count]);
    let points = &distribution.continuum;
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return zero;
    };
    if energy_out < first.0 || energy_out > last.0 {
        return zero;
    }
    if let Some(index) = points.iter().position(|(x, _, _)| *x == energy_out) {
        return (points[index].1, points[index].2.clone());
    }
    let Some(segment) = points
        .windows(2)
        .position(|pair| pair[0].0 < energy_out && energy_out < pair[1].0)
    else {
        return zero;
    };
    let (x0, p0, params0) = &points[segment];
    let (x1, p1, params1) = &points[segment + 1];
    match lep {
        1 => (*p0, params0.clone()),
        2 => {
            let w = (energy_out - x0) / (x1 - x0);
            (
                p0 + w * (p1 - p0),
                params0
                    .iter()
                    .zip(params1.iter())
                    .map(|(a, b)| a + w * (b - a))
                    .collect(),
            )
        }
        _ => zero,
    }
}

fn reaction_remainder(
    mf3: &Mf3Reaction,
    mf6: &Mf6Reaction,
    incident_energy_ev: f64,
    reaction_mt: u16,
    target_awr: f64,
    printed: &BTreeMap<u16, PrintedMtTables>,
) -> Result<Option<EndfReactionBalanceReaction>, EndfReactionBalanceError> {
    let cross_section = evaluate_below_grid_zero(&mf3.cross_section, incident_energy_ev)
        .ok_or_else(|| physics("reaction cross section"))?;
    if cross_section < 0.0 {
        return Err(EndfReactionBalanceError::InvalidTabulation);
    }
    if cross_section == 0.0 {
        return Ok(None);
    }
    let printed_mt = printed.get(&reaction_mt);
    let printed_remainder = printed_mt.and_then(|tables| {
        tables
            .remainders
            .get(&incident_energy_ev.to_bits())
            .copied()
    });
    let mut carried = 0.0;
    let mut products = Vec::with_capacity(mf6.products.len());
    for product in &mf6.products {
        let yield_at_e = evaluate_below_grid_zero(&product.yield_function, incident_energy_ev)
            .ok_or_else(|| physics("product yield"))?;
        if yield_at_e < 0.0 {
            return Err(EndfReactionBalanceError::InvalidTabulation);
        }
        let moments = product_moments_at(product, target_awr, incident_energy_ev, reaction_mt)?;
        let (norm, mean_cm, mean_lab) = match moments {
            Some((norm, first, cross)) => {
                let mean_cm = if norm > 0.0 { first / norm } else { 0.0 };
                // LCT=3 convention: products tabulated with an angular
                // distribution (LANG=2, Kalbach-Mann) are in the
                // centre-of-mass frame and require the CM-to-lab transform;
                // isotropic products (LANG=1) are tabulated in the
                // laboratory frame directly.
                let mean_lab = if product.lang == 2 {
                    let translation = incident_energy_ev * product.awp / (target_awr + 1.0).powi(2);
                    let mean_cross = if norm > 0.0 { cross / norm } else { 0.0 };
                    mean_cm + translation + 2.0 * translation.sqrt() * mean_cross
                } else {
                    mean_cm
                };
                if !mean_lab.is_finite() || mean_lab < 0.0 {
                    return Err(EndfReactionBalanceError::InvalidTabulation);
                }
                (norm, mean_cm, mean_lab)
            }
            None => {
                if yield_at_e != 0.0 {
                    return Err(EndfReactionBalanceError::UnsupportedRepresentation {
                        reaction_mt,
                        message: "incident energy is outside the product distribution grid".into(),
                    });
                }
                (0.0, 0.0, 0.0)
            }
        };
        let printed_ebar = printed_mt.and_then(|tables| {
            tables
                .ebars
                .get(&(product.canonical_za, incident_energy_ev.to_bits()))
                .copied()
        });
        carried += yield_at_e * mean_lab;
        products.push(EndfReactionBalanceProduct {
            zap: product.zap,
            canonical_za: product.canonical_za,
            awp_neutron_mass_units: product.awp,
            transport_disposition: if product.zap == PHOTON_ZAP
                || product.zap == NEUTRON_ZAP
                || product.canonical_za <= 2004
            {
                EndfProductDisposition::CarriedAway
            } else {
                EndfProductDisposition::LocallyDeposited
            },
            angular_representation: if product.lang == 2 {
                let has_tabulated_slope = product.distributions.iter().any(|distribution| {
                    distribution
                        .continuum
                        .iter()
                        .any(|(_, _, params)| params.len() >= 2)
                });
                if has_tabulated_slope {
                    EndfProductAngularRepresentation::KalbachMannTabulatedSlope
                } else {
                    EndfProductAngularRepresentation::KalbachMannCalculatedSlope
                }
            } else {
                EndfProductAngularRepresentation::Isotropic
            },
            yield_per_reaction: yield_at_e,
            spectrum_normalization: norm,
            mean_cm_energy_ev: mean_cm,
            mean_lab_energy_ev: mean_lab,
            printed_ebar_ev: printed_ebar,
            ebar_relative_difference: printed_ebar
                .map(|printed| relative_difference(mean_lab, printed)),
        });
    }
    let remainder = cross_section * (incident_energy_ev + mf3.q_value_ev - carried);
    Ok(Some(EndfReactionBalanceReaction {
        reaction_mt,
        status: EndfReactionBalanceReactionStatus::Computed,
        not_computable_reason: None,
        q_value_ev: Some(mf3.q_value_ev),
        cross_section_barns: Some(cross_section),
        independent_remainder_ev_barns: Some(remainder),
        printed_remainder_ev_barns: printed_remainder,
        remainder_relative_difference: printed_remainder
            .map(|printed| relative_difference(remainder, printed)),
        products,
    }))
}

/// Canonical `1000*Z + A` identifier for an MF=6 product. JEFF-4.0 writes a
/// nonstandard ZAP for some light ejectiles (for example `1000` for the
/// proton in O-17); the product mass is authoritative.
fn canonical_product_za(zap: i64, awp: f64) -> Option<u32> {
    if zap == PHOTON_ZAP {
        return Some(0);
    }
    if zap == NEUTRON_ZAP {
        return Some(1);
    }
    if zap < 1000 || !awp.is_finite() || awp <= 0.0 {
        return None;
    }
    let z = zap / 1000;
    let a = awp.round() as i64;
    if !(0..=200).contains(&z) || !(1..=300).contains(&a) || z > a {
        return None;
    }
    Some((z * 1000 + a) as u32)
}

/// Langevin function `L(a) = coth(a) - 1/a`: the mean cosine of the symmetric
/// Kalbach-Mann angular component `a/(2 sinh a) * cosh(a*mu)`.
fn langevin(a: f64) -> f64 {
    if a.abs() < 1.0e-8 {
        a / 3.0 - a.powi(3) / 45.0
    } else {
        1.0 / a.tanh() - 1.0 / a
    }
}

/// Kalbach-Mann angular-distribution slope per the ENDF-6 manual File 6
/// LAW=1 LANG=2 systematics, used when the evaluation does not tabulate the
/// slope (NA=1 gives only the precompound fraction).
fn kalbach_slope(
    energy_projectile_ev: f64,
    energy_emitted_ev: f64,
    za_emitted: u32,
    target_awr: f64,
) -> Option<f64> {
    let emitted = AtomicRep::from_za(za_emitted)?;
    let target = AtomicRep {
        z: 8,
        a: target_awr.round() as u32,
    };
    let projectile = AtomicRep { z: 0, a: 1 };
    let compound = projectile + target;
    if emitted.z > compound.z || emitted.a >= compound.a {
        return None;
    }
    let residual = compound - emitted;

    // Channel energies per ENDF-6 section 6.2.3.2. Consistent with the
    // published convention, mass numbers approximate the particle masses.
    let epsilon_a = energy_projectile_ev * target.a as f64 / (target.a + 1) as f64 / EV_PER_MEV;
    let epsilon_b =
        energy_emitted_ev * (residual.a + emitted.a) as f64 / (residual.a as f64 * EV_PER_MEV);

    let s_a = separation_energy(&compound, &target, &projectile)?;
    let s_b = separation_energy(&compound, &residual, &emitted)?;

    let m_factor = match za_emitted {
        1 => 0.5,
        1001 | 1002 | 1003 | 2003 => 1.0,
        2004 => 2.0,
        _ => return None,
    };
    // The projectile is always a neutron here, for which M = 1.
    let big_m = 1.0;
    let e_a = epsilon_a + s_a;
    let e_b = epsilon_b + s_b;
    if e_a <= 0.0 || e_b <= 0.0 {
        return None;
    }
    let x1 = e_a.min(130.0) * e_b / e_a;
    let x3 = e_a.min(41.0) * e_b / e_a;
    Some(0.04 * x1 + 1.8e-6 * x1.powi(3) + 6.7e-7 * big_m * m_factor * x3.powi(4))
}

#[derive(Clone, Copy)]
struct AtomicRep {
    z: u32,
    a: u32,
}

impl AtomicRep {
    fn from_za(za: u32) -> Option<Self> {
        let z = za / 1000;
        let a = za % 1000;
        if z > a {
            return None;
        }
        Some(Self { z, a })
    }
    fn n(&self) -> u32 {
        self.a - self.z
    }
}

impl std::ops::Add for AtomicRep {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            z: self.z + other.z,
            a: self.a + other.a,
        }
    }
}

impl std::ops::Sub for AtomicRep {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self {
            z: self.z - other.z,
            a: self.a - other.a,
        }
    }
}

/// Separation energy per the ENDF-6 File 6 LAW=1 LANG=2 convention (Eq. 4 of
/// Kalbach, doi:10.1103/PhysRevC.37.2350), in MeV.
fn separation_energy(
    compound: &AtomicRep,
    nucleus: &AtomicRep,
    particle: &AtomicRep,
) -> Option<f64> {
    let a_c = compound.a as f64;
    let z_c = compound.z as f64;
    let n_c = compound.n() as f64;
    let a_a = nucleus.a as f64;
    let z_a = nucleus.z as f64;
    let n_a = nucleus.n() as f64;
    if a_a <= 0.0 || a_c <= 0.0 || z_a > a_a {
        return None;
    }
    let i_a = match particle.z * 1000 + particle.a {
        1 | 1001 => 0.0,
        1002 => 2.224566,
        1003 => 8.481798,
        2003 => 7.718043,
        2004 => 28.29566,
        _ => return None,
    };
    Some(
        15.68 * (a_c - a_a)
            - 28.07 * ((n_c - z_c).powi(2) / a_c - (n_a - z_a).powi(2) / a_a)
            - 18.56 * (a_c.powf(2.0 / 3.0) - a_a.powf(2.0 / 3.0))
            + 33.22
                * ((n_c - z_c).powi(2) / a_c.powf(4.0 / 3.0)
                    - (n_a - z_a).powi(2) / a_a.powf(4.0 / 3.0))
            - 0.717 * (z_c.powi(2) / a_c.powf(1.0 / 3.0) - z_a.powi(2) / a_a.powf(1.0 / 3.0))
            + 1.211 * (z_c.powi(2) / a_c - z_a.powi(2) / a_a)
            - i_a,
    )
}

/// Parses NJOY's `file six heating` tables into per-(mt, particle) `ebar`
/// rows and per-mt `ebal` remainders keyed by incident energy.
fn parse_file6_tables(
    text: &str,
) -> Result<BTreeMap<u16, PrintedMtTables>, EndfReactionBalanceError> {
    let lines = text.lines().collect::<Vec<_>>();
    let mut tables: BTreeMap<u16, PrintedMtTables> = BTreeMap::new();
    for (line_index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let Some(tail) = trimmed.strip_prefix("file six heating for mt") else {
            continue;
        };
        let (reaction, particle_and_q) = tail
            .split_once(", particle =")
            .ok_or(EndfReactionBalanceError::UnparsedRemainderTable)?;
        let reaction_mt = reaction
            .trim()
            .parse::<u16>()
            .map_err(|_| EndfReactionBalanceError::UnparsedRemainderTable)?;
        let (particle, q_value) = particle_and_q
            .split_once("q =")
            .ok_or(EndfReactionBalanceError::UnparsedRemainderTable)?;
        let particle_id = particle
            .trim()
            .parse::<u32>()
            .map_err(|_| EndfReactionBalanceError::UnparsedRemainderTable)?;
        if q_value.trim().parse::<f64>().is_err() {
            return Err(EndfReactionBalanceError::UnparsedRemainderTable);
        }
        let mut cursor = line_index + 1;
        let mut current_energy = None;
        let mut found_data = false;
        while cursor < lines.len() {
            let inner = lines[cursor].trim();
            if inner.starts_with("file six heating for mt") {
                break;
            }
            let fields = inner.split_whitespace().collect::<Vec<_>>();
            if fields.len() >= 5
                && let Ok(values) = fields
                    .iter()
                    .map(|field| field.parse::<f64>())
                    .collect::<Result<Vec<_>, _>>()
            {
                if values[0] <= 0.0 || values.iter().any(|value| !value.is_finite()) {
                    return Err(EndfReactionBalanceError::UnparsedRemainderTable);
                }
                current_energy = Some(values[0]);
                tables
                    .entry(reaction_mt)
                    .or_default()
                    .ebars
                    .insert((particle_id, values[0].to_bits()), values[1]);
                found_data = true;
                cursor += 1;
                continue;
            }
            if fields.len() == 2 && fields[0] == "ebal" {
                let incident_energy_ev =
                    current_energy.ok_or(EndfReactionBalanceError::UnparsedRemainderTable)?;
                let remainder = fields[1]
                    .parse::<f64>()
                    .map_err(|_| EndfReactionBalanceError::UnparsedRemainderTable)?;
                if !remainder.is_finite() {
                    return Err(EndfReactionBalanceError::UnparsedRemainderTable);
                }
                *tables
                    .entry(reaction_mt)
                    .or_default()
                    .remainders
                    .entry(incident_energy_ev.to_bits())
                    .or_insert(0.0) += remainder;
                cursor += 1;
                continue;
            }
            if found_data && (inner.is_empty() || !fields.is_empty()) {
                break;
            }
            cursor += 1;
        }
    }
    Ok(tables)
}

/// Evaluates a tabulated function, returning 0.0 for incident energies below
/// the tabulated grid (a reaction or product cannot occur below its first
/// tabulated point). Energies above the grid are an error.
fn evaluate_below_grid_zero(function: &ReactionTable, x: f64) -> Option<f64> {
    let first = function.points.first()?.0;
    let last = function.points.last()?.0;
    if x < first {
        Some(0.0)
    } else if x <= last {
        function.evaluate(x)
    } else {
        None
    }
}

fn physics(message: impl Into<String>) -> EndfReactionBalanceError {
    EndfReactionBalanceError::Physics(message.into())
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

fn nonnegative_usize(value: i64) -> Result<usize, EndfReactionBalanceError> {
    usize::try_from(value).map_err(|_| EndfReactionBalanceError::InvalidTabulation)
}

fn validate_tolerance(value: f64) -> Result<(), EndfReactionBalanceError> {
    if value.is_finite() && value > 0.0 && value <= 5.0e-2 {
        Ok(())
    } else {
        invalid_report("relative tolerance must be in (0, 5e-2]")
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<(), EndfReactionBalanceError> {
    if value.trim().is_empty() {
        invalid_report(format!("{label} must not be empty"))
    } else {
        Ok(())
    }
}

fn validate_reference(
    label: &str,
    reference: &ContentReference,
) -> Result<(), EndfReactionBalanceError> {
    validate_identifier(label, &reference.id)?;
    validate_sha256(label, &reference.sha256)
}

fn validate_sha256(label: &str, value: &str) -> Result<(), EndfReactionBalanceError> {
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

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, EndfReactionBalanceError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| EndfReactionBalanceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(EndfReactionBalanceError::NotRegularFile(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| EndfReactionBalanceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn invalid_report<T>(message: impl Into<String>) -> Result<T, EndfReactionBalanceError> {
    Err(EndfReactionBalanceError::InvalidReport(message.into()))
}

fn map_moment(error: EndfPhotonMomentError) -> EndfReactionBalanceError {
    match error {
        EndfPhotonMomentError::InvalidTabulation => EndfReactionBalanceError::InvalidTabulation,
        other => physics(format!("shared ENDF helper failed: {other}")),
    }
}

pub(crate) fn take_control_pub(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<EndfRecord, EndfReactionBalanceError> {
    take_control(section, cursor).map_err(map_moment)
}

fn take_words_pub(
    section: &ParsedSection,
    cursor: &mut usize,
    word_count: usize,
) -> Result<Vec<f64>, EndfReactionBalanceError> {
    take_words(section, cursor, word_count).map_err(map_moment)
}

fn parse_tab2_pub(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<(EndfRecord, Vec<InterpolationRegion>), EndfReactionBalanceError> {
    parse_tab2(section, cursor).map_err(map_moment)
}

fn require_consumed_pub(
    section: &ParsedSection,
    cursor: usize,
) -> Result<(), EndfReactionBalanceError> {
    require_consumed(section, cursor).map_err(map_moment)
}

fn value_pub(record: EndfRecord, index: usize) -> Result<f64, EndfReactionBalanceError> {
    value(record, index).map_err(map_moment)
}

#[derive(Debug, Error)]
pub enum EndfReactionBalanceError {
    #[error(transparent)]
    EvaluatedSource(#[from] EvaluatedSourceError),
    #[error(transparent)]
    Domain(#[from] NjoyDomainAwareSuitabilityError),
    #[error(transparent)]
    Execution(#[from] NjoyExecutionError),
    #[error("energy-balance inputs do not share the same bound evidence chain")]
    EvidenceBindingMismatch,
    #[error("selection has no evaluation for {0}")]
    MissingEvaluation(String),
    #[error("evaluation file digest changed after selection: {0}")]
    EvaluationDigestMismatch(String),
    #[error("no MT is present in both File 3 and File 6")]
    NoSharedSections,
    #[error("ENDF section parse failed: {0}")]
    SectionParse(#[source] crate::EndfPhotonInventoryError),
    #[error("unsupported representation for MT={reaction_mt}: {message}")]
    UnsupportedRepresentation { reaction_mt: u16, message: String },
    #[error("invalid ENDF tabulation")]
    InvalidTabulation,
    #[error("File 3/File 6 section for MT={reaction_mt} could not be parsed: {message}")]
    InvalidReactionSection { reaction_mt: u16, message: String },
    #[error("processor report changed after execution verification: {0}")]
    ProcessorReportChanged(String),
    #[error("processor report is not UTF-8 text: {0}")]
    NonUtf8ProcessorReport(PathBuf),
    #[error("evaluation is not UTF-8 ENDF text")]
    NonUtf8Evaluation,
    #[error("NJOY File 6 remainder tables could not be parsed")]
    UnparsedRemainderTable,
    #[error("invalid reaction energy-balance report: {0}")]
    InvalidReport(String),
    #[error("stored reaction energy-balance report does not match regenerated source evidence")]
    ReportMismatch,
    #[error("a required physical value was not computable: {0}")]
    Physics(String),
    #[error("required balance artifact is not a regular file: {0}")]
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

    const JEFF40_BALANCE: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-o17-endf-reaction-energy-balance.json"
    );

    #[test]
    fn validates_frozen_jeff40_o17_evidence() {
        let document = EndfReactionEnergyBalanceDocument::from_bytes(JEFF40_BALANCE).unwrap();
        assert_eq!(
            document.sha256,
            "9a102394d11a25a90928b0cebd6b5aee6475704dbbe8ef68c6f52701e0711a55"
        );
        let report = &document.report;
        assert_eq!(
            report.qualification,
            EndfReactionBalanceQualification::SourceRemaindersComputedUnreviewed
        );
        assert_eq!(report.sample_count, 43);
        assert_eq!(report.computed_sample_count, 43);
        assert_eq!(report.partially_computed_sample_count, 0);
        assert_eq!(report.remainder_matched_sample_count, 17);
        assert_eq!(report.reaction_mts_evaluated.len(), 28);
        assert_eq!(report.ebar_comparison_count, 515);
        assert!(
            (report.maximum_remainder_relative_difference - 1.6818844417027717e-2).abs() < 1.0e-18
        );
        assert!((report.maximum_ebar_relative_difference - 0.5096966567045265).abs() < 1.0e-18);
        // Every sample must carry the MT=91 processor-internal convention
        // findings forward: no finding is resolved by this artifact.
        assert!(
            report
                .samples
                .iter()
                .all(|sample| sample.processor_mt301_excess_ev_barns > 0.0)
        );
    }

    fn control(c1: f64, c2: f64, l1: i64, l2: i64, n1: i64, n2: i64) -> EndfRecord {
        EndfRecord {
            values: [
                Some(c1),
                Some(c2),
                Some(l1 as f64),
                Some(l2 as f64),
                Some(n1 as f64),
                Some(n2 as f64),
            ],
            c1,
            l1,
            l2,
            n1,
            n2,
            is_control: true,
        }
    }

    fn data(words: &[f64]) -> Vec<EndfRecord> {
        words
            .chunks(6)
            .map(|chunk| {
                let mut values = [None; 6];
                for (slot, word) in values.iter_mut().zip(chunk.iter()) {
                    *slot = Some(*word);
                }
                EndfRecord {
                    values,
                    c1: 0.0,
                    l1: 0,
                    l2: 0,
                    n1: 0,
                    n2: 0,
                    is_control: false,
                }
            })
            .collect()
    }

    fn section(file_number: u16, reaction_mt: u16, records: Vec<EndfRecord>) -> ParsedSection {
        ParsedSection {
            file_number,
            reaction_mt,
            record_count: records.len() as u64,
            sha256: "0".repeat(64),
            records,
        }
    }

    /// Builds a File 3 section: HEAD + TAB1 (QM/QI head) + cross-section table.
    fn mf3_section(
        reaction_mt: u16,
        awr: f64,
        qm: f64,
        qi: f64,
        points: &[(f64, f64)],
    ) -> ParsedSection {
        let mut records = vec![
            control(8017.0, awr, 0, 0, 0, 0),
            control(qm, qi, 0, 0, 1, points.len() as i64),
            control_marker_data(&[points.len() as f64, 2.0]),
        ];
        let mut table = Vec::new();
        for (x, y) in points {
            table.push(*x);
            table.push(*y);
        }
        records.extend(data(&table));
        section(FILE3, reaction_mt, records)
    }

    fn control_marker_data(words: &[f64]) -> EndfRecord {
        data(words).into_iter().next().unwrap()
    }

    /// Builds one LAW=1 product subsection: yield TAB1, TAB2 header, and one
    /// LIST record per incident energy. `spectra` items are
    /// `(E_i, discrete, continuum)` with continuum entries `(E', p, params)`.
    fn mf6_product_records(
        zap: f64,
        awp: f64,
        lang: i64,
        lep: i64,
        yield_points: &[(f64, f64)],
        spectra: &[Law1Spectrum],
    ) -> Vec<EndfRecord> {
        let mut records = vec![
            control(zap, awp, 0, 1, 1, yield_points.len() as i64),
            control_marker_data(&[yield_points.len() as f64, 2.0]),
        ];
        let mut table = Vec::new();
        for (x, y) in yield_points {
            table.push(*x);
            table.push(*y);
        }
        records.extend(data(&table));
        records.push(control(0.0, 0.0, lang, lep, 1, spectra.len() as i64));
        records.push(control_marker_data(&[spectra.len() as f64, 2.0]));
        for spectrum in spectra {
            let na = spectrum.na;
            let nd = spectrum.discrete.len();
            let nep = nd + spectrum.continuum.len();
            let nw = nd * 2 + spectrum.continuum.len() * (na as usize + 2);
            records.push(control(
                0.0,
                spectrum.incident_energy_ev,
                nd as i64,
                na,
                nw as i64,
                nep as i64,
            ));
            let mut words = Vec::with_capacity(nw);
            for (energy, probability) in &spectrum.discrete {
                words.push(*energy);
                words.push(*probability);
            }
            for (energy, density, params) in &spectrum.continuum {
                words.push(*energy);
                words.push(*density);
                words.extend(params.iter().copied());
            }
            records.extend(data(&words));
        }
        records
    }

    #[derive(Clone)]
    struct Law1Spectrum {
        incident_energy_ev: f64,
        na: i64,
        discrete: Vec<(f64, f64)>,
        continuum: Vec<(f64, f64, Vec<f64>)>,
    }

    /// Duplicates a spectrum at a second incident energy: TAB2 incident
    /// interpolation requires at least two grid points.
    fn two_spectra(spectrum: &Law1Spectrum, second_energy: f64) -> Vec<Law1Spectrum> {
        let mut upper = spectrum.clone();
        upper.incident_energy_ev = second_energy;
        vec![spectrum.clone(), upper]
    }

    fn mf6_section(reaction_mt: u16, products: Vec<Vec<EndfRecord>>) -> ParsedSection {
        let mut records = vec![control(8017.0, 16.85, 0, 3, products.len() as i64, 0)];
        for product in products {
            records.extend(product);
        }
        section(FILE6, reaction_mt, records)
    }

    fn product(
        zap: f64,
        awp: f64,
        lang: i64,
        lep: i64,
        spectra: &[Law1Spectrum],
    ) -> Vec<EndfRecord> {
        mf6_product_records(zap, awp, lang, lep, &[(1.0e-5, 1.0), (2.0e8, 1.0)], spectra)
    }

    fn histogram(energy: f64, span: f64, height: f64, params: &[f64]) -> Law1Spectrum {
        Law1Spectrum {
            incident_energy_ev: energy,
            na: params.len() as i64,
            discrete: Vec::new(),
            continuum: vec![(0.0, height, params.to_vec()), (span, 0.0, params.to_vec())],
        }
    }

    #[test]
    fn reaction_table_resolves_duplicate_x_to_right_value() {
        // Legal ENDF-6 discontinuity: the cross section steps to zero at the
        // duplicated x (JEFF-4.0 O-17 drops every cross section at 3.0e7 eV).
        let records = section(
            FILE3,
            103,
            vec![
                control(1.0e6, 0.5, 0, 0, 1, 4),
                control_marker_data(&[4.0, 2.0]),
            ]
            .into_iter()
            .chain(data(&[1.0e6, 0.5, 3.0e7, 0.4, 3.0e7, 0.0, 4.0e7, 0.0]))
            .collect(),
        );
        let mut cursor = 0;
        let (_, table) = ReactionTable::parse(&records, &mut cursor).unwrap();
        assert_eq!(table.evaluate(3.0e7), Some(0.0));
        let interpolated = 0.5 + (0.4 - 0.5) * (2.0e7 - 1.0e6) / (3.0e7 - 1.0e6);
        assert_eq!(table.evaluate(2.0e7), Some(interpolated));
        assert_eq!(table.evaluate(1.0e6), Some(0.5));
        assert_eq!(table.evaluate(5.0e7), None);
    }

    #[test]
    fn reaction_table_rejects_decreasing_x() {
        let records = section(
            FILE3,
            103,
            vec![
                control(0.0, 0.0, 0, 0, 1, 3),
                control_marker_data(&[3.0, 2.0]),
            ]
            .into_iter()
            .chain(data(&[2.0e6, 1.0, 1.0e6, 1.0, 3.0e6, 1.0]))
            .collect(),
        );
        let mut cursor = 0;
        assert!(matches!(
            ReactionTable::parse(&records, &mut cursor),
            Err(EndfReactionBalanceError::InvalidTabulation)
        ));
    }

    #[test]
    fn mf3_uses_mass_difference_q_not_reaction_q() {
        // MT=91 carries QM=0 with QI=-7.76e6; the balance must use QM.
        let parsed = parse_mf3_reaction(&mf3_section(
            91,
            16.85,
            0.0,
            -7.7636e6,
            &[(1.0e5, 0.1), (2.0e8, 0.1)],
        ))
        .unwrap();
        assert_eq!(parsed.q_value_ev, 0.0);
        assert_eq!(parsed.awr, 16.85);
    }

    #[test]
    fn mf6_parses_law1_products_with_kalbach_and_discrete() {
        // Product 0: LANG=2 Kalbach-Mann continuum with NA=1 precompound
        // fractions. Product 1: LANG=1 isotropic discrete + continuum.
        let kalbach = vec![
            Law1Spectrum {
                incident_energy_ev: 1.0e6,
                na: 1,
                discrete: Vec::new(),
                continuum: vec![(0.0, 1.0e-6, vec![0.3]), (1.0e6, 2.0e-6, vec![0.4])],
            },
            Law1Spectrum {
                incident_energy_ev: 2.0e6,
                na: 1,
                discrete: Vec::new(),
                continuum: vec![(0.0, 1.0e-6, vec![0.5]), (2.0e6, 0.0, vec![0.6])],
            },
        ];
        let isotropic = two_spectra(
            &Law1Spectrum {
                incident_energy_ev: 1.0e6,
                na: 0,
                discrete: vec![(5.0e5, 0.25)],
                continuum: vec![(0.0, 1.0e-6, vec![]), (1.0e6, 2.0e-6, vec![])],
            },
            2.0e6,
        );
        let section = mf6_section(
            103,
            vec![
                product(1001.0, 0.9986, 2, 1, &kalbach),
                product(8016.0, 15.86, 1, 1, &isotropic),
            ],
        );
        let parsed = parse_mf6_reaction(&section).unwrap();
        assert_eq!(parsed.products.len(), 2);
        let product = &parsed.products[0];
        assert_eq!(product.zap, 1001);
        assert_eq!(product.canonical_za, 1001);
        assert_eq!(product.lang, 2);
        assert_eq!(product.distributions.len(), 2);
        assert_eq!(product.distributions[0].continuum.len(), 2);
        assert_eq!(product.distributions[0].continuum[0].2, vec![0.3]);
        let residual = &parsed.products[1];
        assert_eq!(residual.distributions[0].discrete, vec![(5.0e5, 0.25)]);
    }

    #[test]
    fn mf6_rejects_non_lct3_and_non_law1() {
        let mut records = vec![control(8017.0, 16.85, 0, 1, 0, 0)];
        let bad_lct = section(FILE6, 103, records.clone());
        assert!(matches!(
            parse_mf6_reaction(&bad_lct),
            Err(EndfReactionBalanceError::UnsupportedRepresentation { .. })
        ));
        records = product(1001.0, 1.0, 1, 1, &[]);
        records[0].l2 = 3; // LAW=3
        let bad_law = mf6_section(103, vec![records]);
        assert!(matches!(
            parse_mf6_reaction(&bad_law),
            Err(EndfReactionBalanceError::UnsupportedRepresentation { .. })
        ));
    }

    #[test]
    fn canonical_za_recovers_physical_product() {
        assert_eq!(canonical_product_za(0, 0.0), Some(0));
        assert_eq!(canonical_product_za(1, 0.9986), Some(1));
        assert_eq!(canonical_product_za(1001, 0.9986), Some(1001));
        // JEFF-4.0 writes 1002 with AWP=2.99 for the triton; the mass fixes it.
        assert_eq!(canonical_product_za(1002, 2.99), Some(1003));
        assert_eq!(canonical_product_za(2004, 3.968), Some(2004));
        assert_eq!(canonical_product_za(8016, 15.86), Some(8016));
        assert_eq!(canonical_product_za(1000, 0.9986), Some(1001));
        assert_eq!(canonical_product_za(7000, 5.0), None); // z > a
    }

    #[test]
    fn histogram_spectrum_moments_are_exact() {
        // Uniform histogram on [0, 1.0e6]: norm = h*dx, mean = 5.0e5.
        let spectra = two_spectra(&histogram(1.0e6, 1.0e6, 1.0e-6, &[]), 2.0e6);
        let records = product(0.0, 0.0, 1, 1, &spectra);
        let section = mf6_section(28, vec![records]);
        let parsed = parse_mf6_reaction(&section).unwrap();
        let (norm, first, cross) = product_spectrum_moments(
            &parsed.products[0].distributions[0],
            &parsed.products[0],
            16.85,
            28,
        )
        .unwrap();
        assert!((norm - 1.0).abs() < 1.0e-12);
        assert!((first - 5.0e5).abs() < 1.0e-6);
        assert_eq!(cross, 0.0);
    }

    #[test]
    fn linlin_spectrum_moments_are_exact() {
        // p declining linearly from 1.0e-6 at E'=0 to 0 at 2.0e6:
        // norm = base*height/2 = 1.0; mean = base/3.
        let spectra = two_spectra(
            &Law1Spectrum {
                incident_energy_ev: 1.0e6,
                na: 0,
                discrete: Vec::new(),
                continuum: vec![(0.0, 1.0e-6, vec![]), (2.0e6, 0.0, vec![])],
            },
            2.0e6,
        );
        let records = product(0.0, 0.0, 1, 2, &spectra);
        let section = mf6_section(28, vec![records]);
        let parsed = parse_mf6_reaction(&section).unwrap();
        let (norm, first, _) = product_spectrum_moments(
            &parsed.products[0].distributions[0],
            &parsed.products[0],
            16.85,
            28,
        )
        .unwrap();
        assert!((norm - 1.0).abs() < 1.0e-12);
        assert!((first / norm - 2.0e6 / 3.0).abs() < 1.0);
    }

    #[test]
    fn discrete_lines_add_delta_moments() {
        let spectra = two_spectra(
            &Law1Spectrum {
                incident_energy_ev: 1.0e6,
                na: 0,
                discrete: vec![(4.0e5, 0.25), (6.0e5, 0.5)],
                continuum: vec![(0.0, 1.0e-6, vec![]), (1.0e6, 1.0e-6, vec![])],
            },
            2.0e6,
        );
        let records = product(0.0, 0.0, 1, 1, &spectra);
        let section = mf6_section(28, vec![records]);
        let parsed = parse_mf6_reaction(&section).unwrap();
        let (norm, first, cross) = product_spectrum_moments(
            &parsed.products[0].distributions[0],
            &parsed.products[0],
            16.85,
            28,
        )
        .unwrap();
        // discrete 0.75 + continuum uniform 1.0
        assert!((norm - 1.75).abs() < 1.0e-12);
        let expected_first = 4.0e5 * 0.25 + 6.0e5 * 0.5 + 5.0e5;
        assert!((first - expected_first).abs() < 1.0e-6);
        assert_eq!(cross, 0.0);
    }

    #[test]
    fn union_grid_interpolation_blends_spectra_pointwise() {
        // Lower spectrum: uniform histogram on [0, 1e6] height 1e-6.
        // Upper spectrum: uniform histogram on [0, 2e6] height 1e-6.
        // At w=0.5 the interpolated histogram is piecewise:
        //   [0,1e6] -> 1e-6, [1e6,2e6] -> 0.5e-6
        // norm = 1.0 + 0.5 = 1.5; first moment = 5e5 + 0.5*(4e6-1e6)/2*1e-6*1e6...
        let lower = histogram(1.0e6, 1.0e6, 1.0e-6, &[0.0]);
        let upper = histogram(2.0e6, 2.0e6, 1.0e-6, &[0.0]);
        let spectra = vec![lower, upper];
        let records = product(1.0, 0.9986, 2, 1, &spectra);
        let section = mf6_section(16, vec![records]);
        let parsed = parse_mf6_reaction(&section).unwrap();
        let (norm, first, _) = product_moments_at(&parsed.products[0], 16.85, 1.5e6, 16)
            .unwrap()
            .unwrap();
        assert!((norm - 1.5).abs() < 1.0e-12);
        // E' moment: bin [0,1e6] at 1e-6 contributes 0.5e6; bin [1e6,2e6] at
        // 0.5e-6 contributes 0.5e-6 * (4e12-1e12)/2 = 0.75e6.
        assert!((first - 1.25e6).abs() < 1.0);
        // Mean is 1.25e6/1.5 = 833333, between the two tabulated means.
    }

    #[test]
    fn lang2_r_zero_is_isotropic_lab_translation() {
        // Kalbach-Mann with precompound fraction r=0 is isotropic in the CM;
        // the lab mean is the CM mean plus the translation energy.
        let spectra = two_spectra(&histogram(1.0e6, 1.0e6, 1.0e-6, &[0.0]), 2.0e6);
        let mf6 = mf6_section(103, vec![product(1001.0, 1.0, 2, 1, &spectra)]);
        let mf3 = mf3_section(103, 16.85, -1.0e6, -1.0e6, &[(1.0e5, 0.1), (2.0e8, 0.1)]);
        let printed = BTreeMap::new();
        let mf3 = parse_mf3_reaction(&mf3).unwrap();
        let mf6 = parse_mf6_reaction(&mf6).unwrap();
        let row = reaction_remainder(&mf3, &mf6, 1.0e6, 103, 16.85, &printed)
            .unwrap()
            .unwrap();
        let proton = &row.products[0];
        let translation = 1.0e6 * 1.0 / (16.85_f64 + 1.0).powi(2);
        let expected_lab = 5.0e5 + translation;
        assert!((proton.mean_cm_energy_ev - 5.0e5).abs() < 1.0);
        assert!((proton.mean_lab_energy_ev - expected_lab).abs() < 1.0);
        assert_eq!(proton.canonical_za, 1001);
        assert_eq!(
            proton.transport_disposition,
            EndfProductDisposition::CarriedAway
        );
    }

    #[test]
    fn lang1_products_stay_in_lab_frame() {
        // Isotropic lab products keep their tabulated mean (no boost).
        let spectra = two_spectra(&histogram(1.0e6, 1.0e6, 1.0e-6, &[]), 2.0e6);
        let mf6 = mf6_section(103, vec![product(8016.0, 15.86, 1, 1, &spectra)]);
        let mf3 = mf3_section(103, 16.85, -1.0e6, -1.0e6, &[(1.0e5, 0.1), (2.0e8, 0.1)]);
        let printed = BTreeMap::new();
        let mf3 = parse_mf3_reaction(&mf3).unwrap();
        let mf6 = parse_mf6_reaction(&mf6).unwrap();
        let row = reaction_remainder(&mf3, &mf6, 1.0e6, 103, 16.85, &printed)
            .unwrap()
            .unwrap();
        let residual = &row.products[0];
        assert!((residual.mean_lab_energy_ev - 5.0e5).abs() < 1.0);
        assert_eq!(
            residual.transport_disposition,
            EndfProductDisposition::LocallyDeposited
        );
    }

    #[test]
    fn forward_peaked_kalbach_raises_lab_mean() {
        // r=1 forces mu_bar = L(a) > 0; the lab mean must exceed the
        // isotropic translation-only value.
        let isotropic = mf6_section(
            103,
            vec![product(
                1001.0,
                1.0,
                2,
                1,
                &two_spectra(&histogram(1.0e7, 2.0e6, 5.0e-7, &[0.0]), 2.0e7),
            )],
        );
        let forward = mf6_section(
            103,
            vec![product(
                1001.0,
                1.0,
                2,
                1,
                &two_spectra(&histogram(1.0e7, 2.0e6, 5.0e-7, &[1.0]), 2.0e7),
            )],
        );
        let mf3 = parse_mf3_reaction(&mf3_section(
            103,
            16.85,
            -1.0e6,
            -1.0e6,
            &[(1.0e5, 0.1), (2.0e8, 0.1)],
        ))
        .unwrap();
        let printed = BTreeMap::new();
        let mean = |records: &ParsedSection| {
            let mf6 = parse_mf6_reaction(records).unwrap();
            reaction_remainder(&mf3, &mf6, 1.0e7, 103, 16.85, &printed)
                .unwrap()
                .unwrap()
                .products[0]
                .mean_lab_energy_ev
        };
        assert!(mean(&forward) > mean(&isotropic));
    }

    #[test]
    fn remainder_uses_yield_weighted_lab_means() {
        // sigma=0.5, E=1e6, Q=-1e6: available = 1e6 eV.
        // proton (LANG=2, r=0): lab = 5e5 + E_T; yield 1.
        // residual (LANG=1): lab = 5e5; yield 1.
        // photon (LANG=1): discrete 1e5; yield 1.
        let mf3 = mf3_section(103, 16.85, -1.0e6, -1.0e6, &[(1.0e5, 0.5), (2.0e8, 0.5)]);
        let proton_spectra = two_spectra(&histogram(1.0e6, 1.0e6, 1.0e-6, &[0.0]), 2.0e6);
        let residual_spectra = two_spectra(&histogram(1.0e6, 1.0e6, 1.0e-6, &[]), 2.0e6);
        let photon_spectra = two_spectra(
            &Law1Spectrum {
                incident_energy_ev: 1.0e6,
                na: 0,
                discrete: vec![(1.0e5, 1.0)],
                continuum: vec![(0.0, 0.0, vec![]), (1.0e6, 0.0, vec![])],
            },
            2.0e6,
        );
        let mf6 = mf6_section(
            103,
            vec![
                product(1001.0, 1.0, 2, 1, &proton_spectra),
                product(8016.0, 15.86, 1, 1, &residual_spectra),
                product(0.0, 0.0, 1, 1, &photon_spectra),
            ],
        );
        let printed = BTreeMap::new();
        let mf3 = parse_mf3_reaction(&mf3).unwrap();
        let mf6 = parse_mf6_reaction(&mf6).unwrap();
        let row = reaction_remainder(&mf3, &mf6, 1.0e6, 103, 16.85, &printed)
            .unwrap()
            .unwrap();
        let translation = 1.0e6 / (16.85_f64 + 1.0).powi(2);
        let carried = (5.0e5 + translation) + 5.0e5 + 1.0e5;
        let expected = 0.5 * (1.0e6 - 1.0e6 - carried);
        assert!(
            (row.independent_remainder_ev_barns.unwrap() - expected).abs()
                < 1.0e-6 * expected.abs().max(1.0)
        );
        // Below the first MF3 point the cross section is physically zero.
        assert!(
            reaction_remainder(&mf3, &mf6, 1.0e4, 103, 16.85, &printed)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn kalbach_slope_is_positive_for_physical_products() {
        // Proton emitted at 5 MeV from O-17 + n at 14 MeV.
        let slope = kalbach_slope(1.4e7, 5.0e6, 1001, 16.85).unwrap();
        assert!(slope.is_finite() && slope > 0.0);
        // Emitted particle heavier than the compound has no channel.
        assert!(kalbach_slope(1.4e7, 5.0e6, 20004, 16.85).is_none());
    }

    #[test]
    fn langevin_limits_and_midpoint() {
        assert!(langevin(0.0).abs() < 1.0e-12);
        assert!((langevin(1.0) - (1.0 / 1.0_f64.tanh() - 1.0)).abs() < 1.0e-15);
        assert!((langevin(30.0) - (1.0 - 1.0 / 30.0)).abs() < 1.0e-10);
    }

    #[test]
    fn parses_njoy_file6_tables() {
        let text = "\
 file six heating for mt103, particle =  1001     q =  -7.8965E+06
              e          ebar         yield          xsec       heating
     1.0000E+07    9.3382E+05    1.0000E+00    1.0498E-01    9.8041E+04
          ebal  2.5392E+04
     2.0000E+07    1.9000E+06    1.0000E+00    8.0000E-02    1.5200E+05
          ebal  5.0000E+04
 file six heating for mt103, particle =  8016     q =  -7.8965E+06
              e          ebar         yield          xsec       heating
     1.0000E+07    5.9088E+05    1.0000E+00    1.0498E-01    6.2023E+04
          ebal  2.5392E+04
";
        let tables = parse_file6_tables(text).unwrap();
        let mt103 = &tables[&103];
        assert_eq!(mt103.ebars[&(1001, 1.0e7f64.to_bits())], 9.3382e5);
        assert_eq!(mt103.ebars[&(8016, 1.0e7f64.to_bits())], 5.9088e5);
        // ebal rows accumulate across the MT's product tables.
        assert_eq!(mt103.remainders[&1.0e7f64.to_bits()], 2.5392e4 * 2.0);
        assert_eq!(mt103.remainders[&2.0e7f64.to_bits()], 5.0e4);
    }

    fn minimal_report() -> EndfReactionEnergyBalanceReport {
        let digest = "a".repeat(64);
        EndfReactionEnergyBalanceReport {
            schema_version: ENDF_REACTION_ENERGY_BALANCE_SCHEMA.into(),
            id: "sel.o17.endf-reaction-energy-balance-v1".into(),
            case_id: "nf-bnct-001".into(),
            qualification: EndfReactionBalanceQualification::SourceRemaindersComputedUnreviewed,
            evidence_scope:
                EndfReactionBalanceEvidenceScope::IndependentSourceCalculationUnreviewed,
            finding_disposition:
                EndfReactionBalanceFindingDisposition::RetainedForIndependentPhysicalValidation,
            evaluated_source_selection: ContentReference {
                id: "sel".into(),
                sha256: digest.clone(),
            },
            energy_balance_attribution: ContentReference {
                id: "attr".into(),
                sha256: digest.clone(),
            },
            nuclide: "O17".into(),
            endf_mat: 828,
            evaluation_sha256: digest,
            target_awr_neutron_mass_units: 16.8531,
            relative_tolerance: DEFAULT_REACTION_BALANCE_RELATIVE_TOLERANCE,
            reaction_mts_evaluated: vec![103],
            sample_count: 1,
            computed_sample_count: 1,
            partially_computed_sample_count: 0,
            remainder_matched_sample_count: 1,
            remainder_mismatched_sample_count: 0,
            maximum_remainder_relative_difference: 0.0,
            maximum_ebar_relative_difference: 0.0,
            ebar_comparison_count: 0,
            samples: vec![EndfReactionBalanceSample {
                incident_energy_ev: 1.0e7,
                independent_remainder_sum_ev_barns: 25392.0,
                printed_remainder_sum_ev_barns: 25392.0,
                processor_mt301_excess_ev_barns: 25392.0,
                remainder_printed_relative_difference: 0.0,
                remainder_excess_relative_difference: 0.0,
                reactions: vec![EndfReactionBalanceReaction {
                    reaction_mt: 103,
                    status: EndfReactionBalanceReactionStatus::Computed,
                    not_computable_reason: None,
                    q_value_ev: Some(-7.8965e6),
                    cross_section_barns: Some(0.10498),
                    independent_remainder_ev_barns: Some(25392.0),
                    printed_remainder_ev_barns: Some(25392.0),
                    remainder_relative_difference: Some(0.0),
                    products: vec![EndfReactionBalanceProduct {
                        zap: 1001,
                        canonical_za: 1001,
                        awp_neutron_mass_units: 0.9986,
                        transport_disposition: EndfProductDisposition::CarriedAway,
                        angular_representation:
                            EndfProductAngularRepresentation::KalbachMannCalculatedSlope,
                        yield_per_reaction: 1.0,
                        spectrum_normalization: 1.0,
                        mean_cm_energy_ev: 8.9885e5,
                        mean_lab_energy_ev: 9.3382e5,
                        printed_ebar_ev: Some(9.3382e5),
                        ebar_relative_difference: Some(0.0),
                    }],
                }],
                status: EndfReactionBalanceSampleStatus::Computed,
            }],
        }
    }

    #[test]
    fn report_serializes_validates_and_hashes() {
        let report = minimal_report();
        report.validate().unwrap();
        let bytes = serde_json::to_vec_pretty(&report).unwrap();
        let document = EndfReactionEnergyBalanceDocument::from_bytes(&bytes).unwrap();
        assert_eq!(document.sha256, sha256_bytes(&bytes));
        assert_eq!(document.report, report);
    }

    #[test]
    fn report_rejects_inconsistent_counts_and_tampering() {
        let mut report = minimal_report();
        report.remainder_matched_sample_count = 0;
        report.validate().unwrap_err();

        let mut report = minimal_report();
        report.samples[0].remainder_printed_relative_difference = 0.5;
        report.validate().unwrap_err();

        let mut report = minimal_report();
        report.qualification =
            EndfReactionBalanceQualification::SourceRemaindersPartiallyComputable;
        report.validate().unwrap_err();
    }

    #[test]
    fn not_computable_reactions_mark_samples_partial() {
        let mut report = minimal_report();
        report.samples[0].reactions.insert(
            0,
            EndfReactionBalanceReaction {
                reaction_mt: 91,
                status: EndfReactionBalanceReactionStatus::NotComputable,
                not_computable_reason: Some("LAW=4 unsupported".into()),
                q_value_ev: None,
                cross_section_barns: None,
                independent_remainder_ev_barns: None,
                printed_remainder_ev_barns: None,
                remainder_relative_difference: None,
                products: Vec::new(),
            },
        );
        report.reaction_mts_evaluated = vec![91, 103];
        report.samples[0].status = EndfReactionBalanceSampleStatus::PartiallyComputed;
        report.partially_computed_sample_count = 1;
        report.computed_sample_count = 0;
        report.remainder_matched_sample_count = 0;
        report.qualification =
            EndfReactionBalanceQualification::SourceRemaindersPartiallyComputable;
        report.validate().unwrap();
    }
}
