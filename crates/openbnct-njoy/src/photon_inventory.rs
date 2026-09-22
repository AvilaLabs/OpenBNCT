// SPDX-License-Identifier: MIT

//! Deterministic, source-bound inventory of ENDF photon-production records.
//!
//! The inventory deliberately stops short of qualifying an evaluation. It
//! distinguishes the ENDF-6 representations that HEATR can consume, including
//! the valid File 13 path that NJOY announces with its easily-misread
//! "no file 12" informational message.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const ENDF_PHOTON_PRODUCTION_INVENTORY_SCHEMA: &str =
    "openbnct.endf-photon-production-inventory/0.1.0";

const INVENTORY_ID_SUFFIX: &str = "endf-photon-production-inventory";
const RELEVANT_FILES: [u16; 5] = [6, 12, 13, 14, 15];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonProductionInventory {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: EndfPhotonInventoryQualification,
    pub evaluated_source_selection: ContentReference,
    pub evaluations: Vec<EndfPhotonInventoryEvaluation>,
    pub section_count: u64,
    pub reaction_count: u64,
    pub evaluations_with_heatr_photon_source_count: u64,
    pub format_finding_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfPhotonInventoryQualification {
    SourceInventoryUnreviewed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonInventoryEvaluation {
    pub nuclide: String,
    pub endf_mat: u16,
    pub source_evaluation: EndfPhotonInventorySource,
    pub sections: Vec<EndfPhotonSection>,
    pub reactions: Vec<EndfPhotonReaction>,
    pub heatr_photon_source: HeatrPhotonSource,
    pub file13_without_file12_reaction_count: u64,
    pub format_findings: Vec<EndfPhotonFormatFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonInventorySource {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonSection {
    pub file_number: u16,
    pub reaction_mt: u16,
    pub record_count: u64,
    pub sha256: String,
    pub header: EndfPhotonSectionHeader,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonSectionHeader {
    pub l1: i64,
    pub l2: i64,
    pub n1: i64,
    pub n2: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonReaction {
    pub reaction_mt: u16,
    pub file6_photon_products: Vec<EndfFile6PhotonProduct>,
    pub file12: Option<EndfFile12Representation>,
    pub file13: Option<EndfFile13Representation>,
    pub file14: Option<EndfFile14Representation>,
    pub file15: Option<EndfFile15Representation>,
    pub format_findings: Vec<EndfPhotonFormatFindingKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfFile6PhotonProduct {
    pub product_modifier: i64,
    pub law: i64,
    pub yield_point_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfFile12Representation {
    pub representation_option: i64,
    pub multiplicity_subsection_count: Option<u64>,
    pub transition_probability_mode: Option<i64>,
    pub transition_lower_level_count: Option<u64>,
    pub continuum_subsection_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfFile13Representation {
    pub subsection_count: u64,
    pub continuum_subsection_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfFile14Representation {
    pub isotropic: bool,
    pub angular_representation: i64,
    pub subsection_count: u64,
    pub isotropic_subsection_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfFile15Representation {
    pub component_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeatrPhotonSource {
    LocalDepositionFallback,
    File6Only,
    File12Or13Only,
    MixedFile6AndFile12Or13,
}

impl HeatrPhotonSource {
    pub fn transports_secondary_photons(self) -> bool {
        self != Self::LocalDepositionFallback
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndfPhotonFormatFinding {
    pub reaction_mt: u16,
    pub kind: EndfPhotonFormatFindingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndfPhotonFormatFindingKind {
    MissingAngularDistribution,
    MissingContinuumEnergyDistribution,
    OrphanAngularDistribution,
    OrphanEnergyDistribution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndfPhotonProductionInventoryDocument {
    pub inventory: EndfPhotonProductionInventory,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndfPhotonProductionInventoryResult {
    pub inventory: EndfPhotonProductionInventory,
    pub inventory_path: PathBuf,
    pub inventory_sha256: String,
}

impl EndfPhotonProductionInventory {
    /// Inspect the exact evaluation files selected by a content-bound source
    /// manifest. No processor output is trusted or consulted.
    pub fn inspect(
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
    ) -> Result<Self, EndfPhotonInventoryError> {
        selection.selection.verify_files(evaluations_root)?;

        let mut evaluations = Vec::with_capacity(selection.selection.evaluations.len());
        for artifact in &selection.selection.evaluations {
            let path = evaluations_root.join(&artifact.extracted_filename);
            let bytes = read_regular_file(&path)?;
            let parsed = parse_evaluation(&bytes, artifact.endf_mat)?;

            let mut reactions_by_mt: BTreeMap<u16, ReactionBuilder> = BTreeMap::new();
            let mut sections = Vec::with_capacity(parsed.len());
            for section in parsed {
                let reaction = reactions_by_mt.entry(section.reaction_mt).or_default();
                match section.file_number {
                    6 => reaction.file6_photon_products = parse_file6(&section)?,
                    12 => reaction.file12 = Some(parse_file12(&section)?),
                    13 => reaction.file13 = Some(parse_file13(&section)?),
                    14 => reaction.file14 = Some(parse_file14(&section)?),
                    15 => reaction.file15 = Some(parse_file15(&section)?),
                    _ => unreachable!("parser only retains relevant files"),
                }
                sections.push(section.public_summary());
            }

            let mut format_findings = Vec::new();
            let mut reactions = Vec::with_capacity(reactions_by_mt.len());
            let mut has_file6_photons = false;
            let mut has_file12_or_13 = false;
            let mut file13_without_file12_reaction_count = 0_u64;
            for (reaction_mt, reaction) in reactions_by_mt {
                has_file6_photons |= !reaction.file6_photon_products.is_empty();
                has_file12_or_13 |= reaction.file12.is_some() || reaction.file13.is_some();
                file13_without_file12_reaction_count +=
                    u64::from(reaction.file13.is_some() && reaction.file12.is_none());

                let production_in_legacy_files =
                    reaction.file12.is_some() || reaction.file13.is_some();
                let continuum_count = reaction
                    .file12
                    .as_ref()
                    .map_or(0, |value| value.continuum_subsection_count)
                    + reaction
                        .file13
                        .as_ref()
                        .map_or(0, |value| value.continuum_subsection_count);
                let mut findings = BTreeSet::new();
                if production_in_legacy_files && reaction.file14.is_none() {
                    findings.insert(EndfPhotonFormatFindingKind::MissingAngularDistribution);
                }
                if continuum_count > 0 && reaction.file15.is_none() {
                    findings
                        .insert(EndfPhotonFormatFindingKind::MissingContinuumEnergyDistribution);
                }
                if !production_in_legacy_files
                    && reaction.file6_photon_products.is_empty()
                    && reaction.file14.is_some()
                {
                    findings.insert(EndfPhotonFormatFindingKind::OrphanAngularDistribution);
                }
                if continuum_count == 0 && reaction.file15.is_some() {
                    findings.insert(EndfPhotonFormatFindingKind::OrphanEnergyDistribution);
                }
                let findings = findings.into_iter().collect::<Vec<_>>();
                format_findings.extend(findings.iter().map(|kind| EndfPhotonFormatFinding {
                    reaction_mt,
                    kind: *kind,
                }));
                reactions.push(EndfPhotonReaction {
                    reaction_mt,
                    file6_photon_products: reaction.file6_photon_products,
                    file12: reaction.file12,
                    file13: reaction.file13,
                    file14: reaction.file14,
                    file15: reaction.file15,
                    format_findings: findings,
                });
            }

            let heatr_photon_source = match (has_file6_photons, has_file12_or_13) {
                (false, false) => HeatrPhotonSource::LocalDepositionFallback,
                (true, false) => HeatrPhotonSource::File6Only,
                (false, true) => HeatrPhotonSource::File12Or13Only,
                (true, true) => HeatrPhotonSource::MixedFile6AndFile12Or13,
            };
            evaluations.push(EndfPhotonInventoryEvaluation {
                nuclide: artifact.nuclide.clone(),
                endf_mat: artifact.endf_mat,
                source_evaluation: EndfPhotonInventorySource {
                    filename: artifact.extracted_filename.clone(),
                    size_bytes: artifact.size_bytes,
                    sha256: artifact.sha256.clone(),
                },
                sections,
                reactions,
                heatr_photon_source,
                file13_without_file12_reaction_count,
                format_findings,
            });
        }

        let inventory = Self {
            schema_version: ENDF_PHOTON_PRODUCTION_INVENTORY_SCHEMA.into(),
            id: format!("{}.{}", selection.selection.id, INVENTORY_ID_SUFFIX),
            case_id: selection.selection.case_id.clone(),
            qualification: EndfPhotonInventoryQualification::SourceInventoryUnreviewed,
            evaluated_source_selection: ContentReference {
                id: selection.selection.id.clone(),
                sha256: selection.sha256.clone(),
            },
            section_count: evaluations
                .iter()
                .map(|evaluation| evaluation.sections.len() as u64)
                .sum(),
            reaction_count: evaluations
                .iter()
                .map(|evaluation| evaluation.reactions.len() as u64)
                .sum(),
            evaluations_with_heatr_photon_source_count: evaluations
                .iter()
                .filter(|evaluation| {
                    evaluation
                        .heatr_photon_source
                        .transports_secondary_photons()
                })
                .count() as u64,
            format_finding_count: evaluations
                .iter()
                .map(|evaluation| evaluation.format_findings.len() as u64)
                .sum(),
            evaluations,
        };
        inventory.validate()?;
        Ok(inventory)
    }

    pub fn validate(&self) -> Result<(), EndfPhotonInventoryError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            ENDF_PHOTON_PRODUCTION_INVENTORY_SCHEMA,
        ) {
            return invalid_inventory(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier(
            "evaluated_source_selection.id",
            &self.evaluated_source_selection.id,
        )?;
        validate_sha256(
            "evaluated_source_selection.sha256",
            &self.evaluated_source_selection.sha256,
        )?;
        if self.id
            != format!(
                "{}.{}",
                self.evaluated_source_selection.id, INVENTORY_ID_SUFFIX
            )
        {
            return invalid_inventory("inventory ID does not bind the source selection");
        }
        if self.evaluations.is_empty() {
            return invalid_inventory("inventory contains no evaluations");
        }

        let mut previous_nuclide: Option<&str> = None;
        for evaluation in &self.evaluations {
            validate_identifier("evaluations.nuclide", &evaluation.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= evaluation.nuclide.as_str()) {
                return invalid_inventory("evaluations are not strictly ordered by nuclide");
            }
            previous_nuclide = Some(&evaluation.nuclide);
            if evaluation.endf_mat == 0 || evaluation.endf_mat > 9_999 {
                return invalid_inventory("evaluation has an invalid ENDF MAT number");
            }
            validate_identifier(
                "evaluations.source_evaluation.filename",
                &evaluation.source_evaluation.filename,
            )?;
            if evaluation.source_evaluation.size_bytes == 0 {
                return invalid_inventory("evaluation source is empty");
            }
            validate_sha256(
                "evaluations.source_evaluation.sha256",
                &evaluation.source_evaluation.sha256,
            )?;

            let mut previous_section: Option<(u16, u16)> = None;
            for section in &evaluation.sections {
                if !RELEVANT_FILES.contains(&section.file_number)
                    || section.reaction_mt == 0
                    || section.record_count == 0
                {
                    return invalid_inventory("invalid photon inventory section");
                }
                let key = (section.file_number, section.reaction_mt);
                if previous_section.is_some_and(|previous| previous >= key) {
                    return invalid_inventory("sections are not strictly ordered");
                }
                previous_section = Some(key);
                validate_sha256("evaluations.sections.sha256", &section.sha256)?;
            }

            let mut previous_mt = None;
            let mut flattened_findings = Vec::new();
            let mut file13_without_file12_reaction_count = 0_u64;
            let mut has_file6_photons = false;
            let mut has_file12_or_13 = false;
            for reaction in &evaluation.reactions {
                if reaction.reaction_mt == 0
                    || previous_mt.is_some_and(|previous| previous >= reaction.reaction_mt)
                {
                    return invalid_inventory("reactions are not strictly ordered");
                }
                previous_mt = Some(reaction.reaction_mt);
                has_file6_photons |= !reaction.file6_photon_products.is_empty();
                has_file12_or_13 |= reaction.file12.is_some() || reaction.file13.is_some();
                file13_without_file12_reaction_count +=
                    u64::from(reaction.file13.is_some() && reaction.file12.is_none());
                if reaction
                    .format_findings
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                {
                    return invalid_inventory("reaction findings are not strictly ordered");
                }
                flattened_findings.extend(reaction.format_findings.iter().map(|kind| {
                    EndfPhotonFormatFinding {
                        reaction_mt: reaction.reaction_mt,
                        kind: *kind,
                    }
                }));
            }
            if flattened_findings != evaluation.format_findings
                || file13_without_file12_reaction_count
                    != evaluation.file13_without_file12_reaction_count
            {
                return invalid_inventory("evaluation aggregates do not match reactions");
            }
            let expected_source = match (has_file6_photons, has_file12_or_13) {
                (false, false) => HeatrPhotonSource::LocalDepositionFallback,
                (true, false) => HeatrPhotonSource::File6Only,
                (false, true) => HeatrPhotonSource::File12Or13Only,
                (true, true) => HeatrPhotonSource::MixedFile6AndFile12Or13,
            };
            if evaluation.heatr_photon_source != expected_source {
                return invalid_inventory("HEATR photon-source summary does not match reactions");
            }
        }

        let section_count: u64 = self
            .evaluations
            .iter()
            .map(|evaluation| evaluation.sections.len() as u64)
            .sum();
        let reaction_count: u64 = self
            .evaluations
            .iter()
            .map(|evaluation| evaluation.reactions.len() as u64)
            .sum();
        let evaluations_with_heatr_photon_source_count = self
            .evaluations
            .iter()
            .filter(|evaluation| {
                evaluation
                    .heatr_photon_source
                    .transports_secondary_photons()
            })
            .count() as u64;
        let format_finding_count: u64 = self
            .evaluations
            .iter()
            .map(|evaluation| evaluation.format_findings.len() as u64)
            .sum();
        if self.section_count != section_count
            || self.reaction_count != reaction_count
            || self.evaluations_with_heatr_photon_source_count
                != evaluations_with_heatr_photon_source_count
            || self.format_finding_count != format_finding_count
        {
            return invalid_inventory("inventory aggregate counts do not match evaluations");
        }
        Ok(())
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<EndfPhotonProductionInventoryResult, EndfPhotonInventoryError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| EndfPhotonInventoryError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| EndfPhotonInventoryError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(EndfPhotonProductionInventoryResult {
            inventory: self.clone(),
            inventory_path: path.to_path_buf(),
            inventory_sha256: sha256_bytes(&bytes),
        })
    }
}

impl EndfPhotonProductionInventoryDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EndfPhotonInventoryError> {
        let inventory: EndfPhotonProductionInventory = serde_json::from_slice(bytes)?;
        inventory.validate()?;
        Ok(Self {
            inventory,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, EndfPhotonInventoryError> {
        let bytes = read_regular_file(path)?;
        Self::from_bytes(&bytes)
    }

    pub fn verify_against_selection(
        &self,
        selection: &EvaluatedNeutronSourceSelectionDocument,
        evaluations_root: &Path,
    ) -> Result<(), EndfPhotonInventoryError> {
        let observed = EndfPhotonProductionInventory::inspect(selection, evaluations_root)?;
        if self.inventory != observed {
            return Err(EndfPhotonInventoryError::InventoryMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ReactionBuilder {
    file6_photon_products: Vec<EndfFile6PhotonProduct>,
    file12: Option<EndfFile12Representation>,
    file13: Option<EndfFile13Representation>,
    file14: Option<EndfFile14Representation>,
    file15: Option<EndfFile15Representation>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParsedSection {
    pub(crate) file_number: u16,
    pub(crate) reaction_mt: u16,
    pub(crate) record_count: u64,
    pub(crate) sha256: String,
    pub(crate) records: Vec<EndfRecord>,
}

impl ParsedSection {
    fn public_summary(&self) -> EndfPhotonSection {
        let head = self.records[0];
        EndfPhotonSection {
            file_number: self.file_number,
            reaction_mt: self.reaction_mt,
            record_count: self.record_count,
            sha256: self.sha256.clone(),
            header: EndfPhotonSectionHeader {
                l1: head.l1,
                l2: head.l2,
                n1: head.n1,
                n2: head.n2,
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct EndfRecord {
    pub(crate) values: [Option<f64>; 6],
    pub(crate) c1: f64,
    pub(crate) l1: i64,
    pub(crate) l2: i64,
    pub(crate) n1: i64,
    pub(crate) n2: i64,
    pub(crate) is_control: bool,
}

pub(crate) fn parse_evaluation(
    bytes: &[u8],
    expected_mat: u16,
) -> Result<Vec<ParsedSection>, EndfPhotonInventoryError> {
    parse_evaluation_matching(bytes, expected_mat, |file_number, reaction_mt| {
        RELEVANT_FILES.contains(&file_number) && reaction_mt > 0
    })
}

pub(crate) fn parse_evaluation_sections(
    bytes: &[u8],
    expected_mat: u16,
    selected_sections: &[(u16, u16)],
) -> Result<Vec<ParsedSection>, EndfPhotonInventoryError> {
    parse_evaluation_matching(bytes, expected_mat, |file_number, reaction_mt| {
        selected_sections.contains(&(file_number, reaction_mt))
    })
}

fn parse_evaluation_matching(
    bytes: &[u8],
    expected_mat: u16,
    mut include: impl FnMut(u16, u16) -> bool,
) -> Result<Vec<ParsedSection>, EndfPhotonInventoryError> {
    let mut sections = Vec::new();
    let mut current: Option<SectionBuilder> = None;
    let mut seen = BTreeSet::new();

    for (zero_based_line, raw_line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let line_number = zero_based_line + 1;
        let mut line = raw_line;
        if line.last() == Some(&b'\n') {
            line = &line[..line.len() - 1];
        }
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if line.len() < 75 {
            continue;
        }
        let Some(mat) = try_parse_u16_field(&line[66..70]) else {
            if current.is_some() {
                return Err(EndfPhotonInventoryError::InvalidField {
                    line: line_number,
                    label: "MAT",
                });
            }
            continue;
        };
        let mf = parse_u16_field(&line[70..72], line_number, "MF")?;
        let mt = parse_u16_field(&line[72..75], line_number, "MT")?;

        if let Some(builder) = current.as_mut() {
            if mf == builder.file_number && mt == 0 {
                builder.hasher.update(raw_line);
                let finished = current.take().expect("current section exists").finish();
                sections.push(finished);
                continue;
            }
            if mf != builder.file_number || mt != builder.reaction_mt {
                return Err(EndfPhotonInventoryError::MalformedEndf {
                    line: line_number,
                    message: "selected ENDF section changed before SEND".into(),
                });
            }
            if mat != expected_mat {
                return Err(EndfPhotonInventoryError::UnexpectedEndfMaterial {
                    line: line_number,
                    expected: expected_mat,
                    observed: mat,
                });
            }
            builder.push(parse_control_record(line, line_number)?, raw_line);
            continue;
        }

        if include(mf, mt) {
            if mat != expected_mat {
                return Err(EndfPhotonInventoryError::UnexpectedEndfMaterial {
                    line: line_number,
                    expected: expected_mat,
                    observed: mat,
                });
            }
            if !seen.insert((mf, mt)) {
                return Err(EndfPhotonInventoryError::MalformedEndf {
                    line: line_number,
                    message: format!("duplicate MF={mf}/MT={mt} section"),
                });
            }
            let mut builder = SectionBuilder::new(mf, mt);
            builder.push(parse_control_record(line, line_number)?, raw_line);
            current = Some(builder);
        }
    }
    if let Some(builder) = current {
        return Err(EndfPhotonInventoryError::MalformedEndf {
            line: 0,
            message: format!(
                "MF={}/MT={} section has no SEND record",
                builder.file_number, builder.reaction_mt
            ),
        });
    }
    sections.sort_by_key(|section| (section.file_number, section.reaction_mt));
    Ok(sections)
}

struct SectionBuilder {
    file_number: u16,
    reaction_mt: u16,
    records: Vec<EndfRecord>,
    hasher: Sha256,
}

impl SectionBuilder {
    fn new(file_number: u16, reaction_mt: u16) -> Self {
        Self {
            file_number,
            reaction_mt,
            records: Vec::new(),
            hasher: Sha256::new(),
        }
    }

    fn push(&mut self, record: EndfRecord, raw_line: &[u8]) {
        self.records.push(record);
        self.hasher.update(raw_line);
    }

    fn finish(self) -> ParsedSection {
        ParsedSection {
            file_number: self.file_number,
            reaction_mt: self.reaction_mt,
            record_count: self.records.len() as u64,
            sha256: format!("{:x}", self.hasher.finalize()),
            records: self.records,
        }
    }
}

fn parse_file6(
    section: &ParsedSection,
) -> Result<Vec<EndfFile6PhotonProduct>, EndfPhotonInventoryError> {
    let head = section_head(section)?;
    let product_count = nonnegative_usize(section, head.n1, "File 6 product count")?;
    let mut cursor = 1_usize;
    let mut photons = Vec::new();
    for _ in 0..product_count {
        let product = take_tab1(section, &mut cursor)?;
        if is_zero(product.c1) {
            photons.push(EndfFile6PhotonProduct {
                product_modifier: product.l1,
                law: product.l2,
                yield_point_count: nonnegative_u64(
                    section,
                    product.n2,
                    "File 6 yield point count",
                )?,
            });
        }
        skip_file6_law(section, &mut cursor, product.l2)?;
    }
    require_consumed(section, cursor)?;
    Ok(photons)
}

fn skip_file6_law(
    section: &ParsedSection,
    cursor: &mut usize,
    law: i64,
) -> Result<(), EndfPhotonInventoryError> {
    match law {
        value if value <= 0 => Ok(()),
        1 | 2 => {
            let tab2 = take_tab2(section, cursor)?;
            let count = nonnegative_usize(section, tab2.n2, "File 6 incident-energy count")?;
            for _ in 0..count {
                take_list(section, cursor)?;
            }
            Ok(())
        }
        3 | 4 => Ok(()),
        6 => {
            take_record(section, cursor)?;
            Ok(())
        }
        7 => {
            let incident = take_tab2(section, cursor)?;
            let incident_count =
                nonnegative_usize(section, incident.n2, "File 6 incident-energy count")?;
            for _ in 0..incident_count {
                let angular = take_tab2(section, cursor)?;
                let angular_count = nonnegative_usize(section, angular.n2, "File 6 cosine count")?;
                for _ in 0..angular_count {
                    take_tab1(section, cursor)?;
                }
            }
            Ok(())
        }
        unsupported => Err(section_error(
            section,
            format!("unsupported File 6 LAW={unsupported}"),
        )),
    }
}

fn parse_file12(
    section: &ParsedSection,
) -> Result<EndfFile12Representation, EndfPhotonInventoryError> {
    let head = section_head(section)?;
    let option = head.l1;
    let subsection_count = nonnegative_u64(section, head.n1, "File 12 subsection count")?;
    if option == 2 {
        let transition_probability_mode = head.l2;
        if !matches!(transition_probability_mode, 1 | 2) {
            return Err(section_error(
                section,
                format!("unsupported File 12 LG={transition_probability_mode}"),
            ));
        }
        let transition_lower_level_count =
            nonnegative_u64(section, head.n1, "File 12 lower-level count")?;
        let mut cursor = 1_usize;
        let transitions = take_list(section, &mut cursor)?;
        let transition_count =
            nonnegative_u64(section, transitions.n2, "File 12 transition count")?;
        let expected_words = (u64::try_from(transition_probability_mode).expect("LG is positive")
            + 1)
        .checked_mul(transition_count)
        .ok_or_else(|| section_error(section, "File 12 transition word count overflow"))?;
        if u64::try_from(transitions.n1).ok() != Some(expected_words) {
            return Err(section_error(
                section,
                "File 12 transition LIST word count does not match LG and NT",
            ));
        }
        require_consumed(section, cursor)?;
        return Ok(EndfFile12Representation {
            representation_option: option,
            multiplicity_subsection_count: None,
            transition_probability_mode: Some(transition_probability_mode),
            transition_lower_level_count: Some(transition_lower_level_count),
            continuum_subsection_count: 0,
        });
    }
    if option != 1 {
        return Err(section_error(
            section,
            format!("unsupported File 12 LO={option}"),
        ));
    }
    let (parsed_count, continuum_subsection_count) =
        parse_legacy_photon_subsections(section, subsection_count as usize)?;
    Ok(EndfFile12Representation {
        representation_option: option,
        multiplicity_subsection_count: Some(parsed_count),
        transition_probability_mode: None,
        transition_lower_level_count: None,
        continuum_subsection_count,
    })
}

fn parse_file13(
    section: &ParsedSection,
) -> Result<EndfFile13Representation, EndfPhotonInventoryError> {
    let count = nonnegative_u64(
        section,
        section_head(section)?.n1,
        "File 13 subsection count",
    )?;
    let (subsection_count, continuum_subsection_count) =
        parse_legacy_photon_subsections(section, count as usize)?;
    Ok(EndfFile13Representation {
        subsection_count,
        continuum_subsection_count,
    })
}

fn parse_legacy_photon_subsections(
    section: &ParsedSection,
    subsection_count: usize,
) -> Result<(u64, u64), EndfPhotonInventoryError> {
    let mut cursor = 1_usize;
    if subsection_count > 1 {
        take_tab1(section, &mut cursor)?;
    }
    let mut continuum_count = 0_u64;
    for _ in 0..subsection_count {
        let subsection = take_tab1(section, &mut cursor)?;
        if is_zero(subsection.c1) && subsection.l2 == 1 {
            continuum_count += 1;
        }
    }
    require_consumed(section, cursor)?;
    Ok((subsection_count as u64, continuum_count))
}

fn parse_file14(
    section: &ParsedSection,
) -> Result<EndfFile14Representation, EndfPhotonInventoryError> {
    let head = section_head(section)?;
    Ok(EndfFile14Representation {
        isotropic: head.l1 == 1,
        angular_representation: head.l2,
        subsection_count: nonnegative_u64(section, head.n1, "File 14 subsection count")?,
        isotropic_subsection_count: nonnegative_u64(
            section,
            head.n2,
            "File 14 isotropic subsection count",
        )?,
    })
}

fn parse_file15(
    section: &ParsedSection,
) -> Result<EndfFile15Representation, EndfPhotonInventoryError> {
    Ok(EndfFile15Representation {
        component_count: nonnegative_u64(
            section,
            section_head(section)?.n1,
            "File 15 component count",
        )?,
    })
}

fn take_tab1(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<EndfRecord, EndfPhotonInventoryError> {
    let head = take_record(section, cursor)?;
    skip_words(
        section,
        cursor,
        checked_words(section, head.n1, 2, "TAB1 interpolation words")?,
    )?;
    skip_words(
        section,
        cursor,
        checked_words(section, head.n2, 2, "TAB1 value words")?,
    )?;
    Ok(head)
}

fn take_tab2(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<EndfRecord, EndfPhotonInventoryError> {
    let head = take_record(section, cursor)?;
    skip_words(
        section,
        cursor,
        checked_words(section, head.n1, 2, "TAB2 interpolation words")?,
    )?;
    Ok(head)
}

fn take_list(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<EndfRecord, EndfPhotonInventoryError> {
    let head = take_record(section, cursor)?;
    skip_words(
        section,
        cursor,
        nonnegative_usize(section, head.n1, "LIST word count")?,
    )?;
    Ok(head)
}

fn take_record(
    section: &ParsedSection,
    cursor: &mut usize,
) -> Result<EndfRecord, EndfPhotonInventoryError> {
    let record = section
        .records
        .get(*cursor)
        .copied()
        .ok_or_else(|| section_error(section, "record structure extends beyond the section"))?;
    if !record.is_control {
        return Err(section_error(
            section,
            format!("record {} is not a valid control record", *cursor + 1),
        ));
    }
    *cursor += 1;
    Ok(record)
}

fn section_head(section: &ParsedSection) -> Result<EndfRecord, EndfPhotonInventoryError> {
    let head = section
        .records
        .first()
        .copied()
        .ok_or_else(|| section_error(section, "section is empty"))?;
    if !head.is_control {
        return Err(section_error(section, "HEAD is not a valid control record"));
    }
    Ok(head)
}

fn skip_words(
    section: &ParsedSection,
    cursor: &mut usize,
    words: usize,
) -> Result<(), EndfPhotonInventoryError> {
    let records = words.div_ceil(6);
    let end = cursor
        .checked_add(records)
        .ok_or_else(|| section_error(section, "record count overflow"))?;
    if end > section.records.len() {
        return Err(section_error(
            section,
            "record structure extends beyond the section",
        ));
    }
    *cursor = end;
    Ok(())
}

fn checked_words(
    section: &ParsedSection,
    value: i64,
    multiplier: usize,
    label: &str,
) -> Result<usize, EndfPhotonInventoryError> {
    nonnegative_usize(section, value, label)?
        .checked_mul(multiplier)
        .ok_or_else(|| section_error(section, format!("{label} overflow")))
}

fn nonnegative_usize(
    section: &ParsedSection,
    value: i64,
    label: &str,
) -> Result<usize, EndfPhotonInventoryError> {
    usize::try_from(value).map_err(|_| section_error(section, format!("negative {label}")))
}

fn nonnegative_u64(
    section: &ParsedSection,
    value: i64,
    label: &str,
) -> Result<u64, EndfPhotonInventoryError> {
    u64::try_from(value).map_err(|_| section_error(section, format!("negative {label}")))
}

fn require_consumed(
    section: &ParsedSection,
    cursor: usize,
) -> Result<(), EndfPhotonInventoryError> {
    if cursor != section.records.len() {
        return Err(section_error(
            section,
            format!(
                "parsed {cursor} of {} records; structure is not canonical",
                section.records.len()
            ),
        ));
    }
    Ok(())
}

fn section_error(section: &ParsedSection, message: impl Into<String>) -> EndfPhotonInventoryError {
    EndfPhotonInventoryError::InvalidSection {
        file_number: section.file_number,
        reaction_mt: section.reaction_mt,
        message: message.into(),
    }
}

fn parse_control_record(
    line: &[u8],
    line_number: usize,
) -> Result<EndfRecord, EndfPhotonInventoryError> {
    let values = [
        parse_optional_endf_float(&line[0..11], line_number, "field 1")?,
        parse_optional_endf_float(&line[11..22], line_number, "field 2")?,
        parse_optional_endf_float(&line[22..33], line_number, "field 3")?,
        parse_optional_endf_float(&line[33..44], line_number, "field 4")?,
        parse_optional_endf_float(&line[44..55], line_number, "field 5")?,
        parse_optional_endf_float(&line[55..66], line_number, "field 6")?,
    ];
    let integer_fields = [
        parse_optional_i64_field(&line[22..33]),
        parse_optional_i64_field(&line[33..44]),
        parse_optional_i64_field(&line[44..55]),
        parse_optional_i64_field(&line[55..66]),
    ];
    let is_control = integer_fields.iter().all(Option::is_some);
    Ok(EndfRecord {
        values,
        c1: values[0].ok_or(EndfPhotonInventoryError::InvalidField {
            line: line_number,
            label: "C1",
        })?,
        l1: integer_fields[0].unwrap_or(0),
        l2: integer_fields[1].unwrap_or(0),
        n1: integer_fields[2].unwrap_or(0),
        n2: integer_fields[3].unwrap_or(0),
        is_control,
    })
}

fn parse_optional_endf_float(
    field: &[u8],
    line: usize,
    label: &'static str,
) -> Result<Option<f64>, EndfPhotonInventoryError> {
    if std::str::from_utf8(field)
        .map_err(|_| EndfPhotonInventoryError::InvalidField { line, label })?
        .trim()
        .is_empty()
    {
        Ok(None)
    } else {
        parse_endf_float(field, line, label).map(Some)
    }
}

fn parse_endf_float(
    field: &[u8],
    line: usize,
    label: &'static str,
) -> Result<f64, EndfPhotonInventoryError> {
    let value = std::str::from_utf8(field)
        .map_err(|_| EndfPhotonInventoryError::InvalidField { line, label })?
        .trim();
    if value.is_empty() {
        return Err(EndfPhotonInventoryError::InvalidField { line, label });
    }
    let normalized = value.replace(['d', 'D'], "E");
    if let Ok(parsed) = normalized.parse::<f64>() {
        return Ok(parsed);
    }
    let exponent = normalized
        .char_indices()
        .rfind(|(index, character)| *index > 0 && matches!(character, '+' | '-'))
        .map(|(index, _)| index)
        .ok_or(EndfPhotonInventoryError::InvalidField { line, label })?;
    let mut with_exponent = normalized;
    with_exponent.insert(exponent, 'E');
    with_exponent
        .parse()
        .map_err(|_| EndfPhotonInventoryError::InvalidField { line, label })
}

fn parse_optional_i64_field(field: &[u8]) -> Option<i64> {
    std::str::from_utf8(field).ok()?.trim().parse().ok()
}

fn parse_u16_field(
    field: &[u8],
    line: usize,
    label: &'static str,
) -> Result<u16, EndfPhotonInventoryError> {
    try_parse_u16_field(field).ok_or(EndfPhotonInventoryError::InvalidField { line, label })
}

fn try_parse_u16_field(field: &[u8]) -> Option<u16> {
    std::str::from_utf8(field).ok()?.trim().parse().ok()
}

fn is_zero(value: f64) -> bool {
    value == 0.0
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), EndfPhotonInventoryError> {
    if value.trim().is_empty() {
        return invalid_inventory(format!("{label} must not be empty"));
    }
    Ok(())
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), EndfPhotonInventoryError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_inventory(format!("{label} is not a lowercase SHA-256 digest"));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, EndfPhotonInventoryError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| EndfPhotonInventoryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(EndfPhotonInventoryError::NotRegularFile(path.to_path_buf()));
    }
    fs::read(path).map_err(|source| EndfPhotonInventoryError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_inventory<T>(message: impl Into<String>) -> Result<T, EndfPhotonInventoryError> {
    Err(EndfPhotonInventoryError::InvalidInventory(message.into()))
}

#[derive(Debug, Error)]
pub enum EndfPhotonInventoryError {
    #[error(transparent)]
    EvaluatedSource(#[from] EvaluatedSourceError),
    #[error("I/O error for {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("path is not a regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("invalid ENDF {label} field at line {line}")]
    InvalidField { line: usize, label: &'static str },
    #[error(
        "ENDF material changed at line {line}: expected MAT={expected}, observed MAT={observed}"
    )]
    UnexpectedEndfMaterial {
        line: usize,
        expected: u16,
        observed: u16,
    },
    #[error("malformed ENDF at line {line}: {message}")]
    MalformedEndf { line: usize, message: String },
    #[error("invalid MF={file_number}/MT={reaction_mt} section: {message}")]
    InvalidSection {
        file_number: u16,
        reaction_mt: u16,
        message: String,
    },
    #[error("invalid photon-production inventory: {0}")]
    InvalidInventory(String),
    #[error("stored photon-production inventory does not match regenerated source evidence")]
    InventoryMismatch,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE_INVENTORY: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/endfb81-endf-photon-production-inventory.json"
    );
    const JEFF40_INVENTORY: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-endf-photon-production-inventory.json"
    );

    fn record(c1: &str, c2: &str, l1: i64, l2: i64, n1: i64, n2: i64) -> EndfRecord {
        let c1 = c1.parse().unwrap();
        let c2 = c2.parse().unwrap();
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

    fn section(file_number: u16, records: Vec<EndfRecord>) -> ParsedSection {
        ParsedSection {
            file_number,
            reaction_mt: 102,
            record_count: records.len() as u64,
            sha256: "0".repeat(64),
            records,
        }
    }

    #[test]
    fn parses_endf_numbers_without_an_e_marker() {
        assert_eq!(parse_endf_float(b" 7.015000+3", 1, "C1").unwrap(), 7015.0);
        assert_eq!(parse_endf_float(b" 1.00000-19", 1, "C1").unwrap(), 1.0e-19);
        assert_eq!(parse_endf_float(b" 0.000000+0", 1, "C1").unwrap(), 0.0);
    }

    #[test]
    fn recognizes_file13_continuum_with_matching_file15_semantics() {
        let parsed = parse_file13(&section(
            13,
            vec![
                record("7015", "14.871", 0, 0, 1, 0),
                record("0", "0", 0, 1, 1, 2),
                record("0", "0", 0, 0, 0, 0),
                record("1e-5", "1", 0, 0, 0, 0),
            ],
        ))
        .unwrap();
        assert_eq!(parsed.subsection_count, 1);
        assert_eq!(parsed.continuum_subsection_count, 1);
    }

    #[test]
    fn recognizes_file6_photon_product() {
        let parsed = parse_file6(&section(
            6,
            vec![
                record("7015", "14.871", 0, 1, 1, 0),
                record("0", "0", 0, 3, 1, 2),
                record("0", "0", 0, 0, 0, 0),
                record("1e-5", "1", 0, 0, 0, 0),
            ],
        ))
        .unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].law, 3);
        assert_eq!(parsed[0].yield_point_count, 2);
    }

    #[test]
    fn validates_frozen_source_inventories() {
        let baseline =
            EndfPhotonProductionInventoryDocument::from_bytes(BASELINE_INVENTORY).unwrap();
        let jeff = EndfPhotonProductionInventoryDocument::from_bytes(JEFF40_INVENTORY).unwrap();
        assert_eq!(
            baseline.sha256,
            "8ccf4da3f29d879e473b49f72fc14f979d002a05cb09947be21ba7624ec697cc"
        );
        assert_eq!(
            jeff.sha256,
            "8e03f3f9ca894a3e6aafae59f3568a8c5b1f09d9c890279e15e4407c760bdd92"
        );
        assert_eq!(baseline.inventory.section_count, 106);
        assert_eq!(jeff.inventory.section_count, 167);
        assert_eq!(baseline.inventory.format_finding_count, 0);
        assert_eq!(jeff.inventory.format_finding_count, 0);
        let jeff_n15 = jeff
            .inventory
            .evaluations
            .iter()
            .find(|evaluation| evaluation.nuclide == "N15")
            .unwrap();
        assert_eq!(jeff_n15.file13_without_file12_reaction_count, 8);
        assert_eq!(
            jeff_n15.heatr_photon_source,
            HeatrPhotonSource::MixedFile6AndFile12Or13
        );
    }
}
