// SPDX-License-Identifier: MIT

//! Deterministic preparation, controlled execution, and evidence assessment
//! for NJOY2016 partial-KERMA processing runs.
//!
//! The crate keeps input generation, external-processor execution, and
//! transported-photon suitability assessment as explicit qualification stages.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_openmc::{
    DataAcquisitionProfileDocument, DataAcquisitionReceiptDocument,
    EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA, EvaluatedNeutronArtifact,
    EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceError,
};
use openbnct_transport::{
    MaterialDefinition, MethodQualification, ResponseGenerationMethod, ResponseMethodError,
    ToolIdentity,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod capture_balance;
mod capture_balance_comparison;
mod comparison;
mod core_check;
mod diagnostic_triage;
mod diagnostic_triage_check;
mod domain_aware;
mod energy_balance_attribution;
mod evidence_aware;
mod execution;
mod law7_breakup;
mod law7_breakup_comparison;
mod photon_inventory;
mod photon_moment;
mod photon_moment_comparison;
mod reaction_energy_balance;
mod response_tables;
mod source_aware;
mod suitability;

pub use capture_balance::{
    DEFAULT_CAPTURE_ENERGY_BALANCE_RELATIVE_TOLERANCE, ENDF_MF6_CAPTURE_PHOTON_BALANCE_SCHEMA,
    EndfMf6CapturePhotonBalanceError, EndfMf6CapturePhotonBalanceQualification,
    EndfMf6CapturePhotonBalanceReport, EndfMf6CapturePhotonBalanceReportDocument,
    EndfMf6CapturePhotonBalanceResult, EndfMf6CapturePhotonBalanceSample,
    EndfMf6CapturePhotonBalanceSampleStatus, EndfMf6CapturePhotonBalanceScope,
    EndfMf6CapturePhotonSource, EndfMf6CaptureRecoilModel, EndfMf6CaptureReferenceFrame,
};
pub use capture_balance_comparison::{
    DEFAULT_NJOY_CAPTURE_PRINT_RELATIVE_TOLERANCE, NJOY_CAPTURE_PHOTON_MOMENT_COMPARISON_SCHEMA,
    NjoyCapturePhotonMomentComparison, NjoyCapturePhotonMomentComparisonDocument,
    NjoyCapturePhotonMomentComparisonError, NjoyCapturePhotonMomentComparisonQualification,
    NjoyCapturePhotonMomentComparisonResult, NjoyCapturePhotonMomentComparisonSample,
    NjoyCapturePhotonMomentComparisonStatus,
};
pub use comparison::{
    NJOY_SUITABILITY_COMPARISON_SCHEMA, NjoySuitabilityComparison,
    NjoySuitabilityComparisonDocument, NjoySuitabilityComparisonError,
    NjoySuitabilityComparisonOutcome, NjoySuitabilityComparisonQualification,
    NjoySuitabilityComparisonResult, NjoySuitabilityComparisonRun,
};
pub use core_check::{
    NJOY_CANDIDATE_COMPARISON_CHECK_SCHEMA, NJOY_EVIDENCE_AWARE_CHECK_SCHEMA,
    NjoyCandidateComparisonCheckResult, NjoyCandidateComparisonCheckVerification,
    NjoyEvidenceAwareCheckResult, NjoyEvidenceAwareCheckVerification,
};
pub use diagnostic_triage::{
    NJOY_DIAGNOSTIC_TRIAGE_SCHEMA, NjoyDiagnosticTriageDisposition, NjoyDiagnosticTriageError,
    NjoyDiagnosticTriageQualification, NjoyDiagnosticTriageReport,
    NjoyDiagnosticTriageReportDocument, NjoyDiagnosticTriageResult, NjoyDiagnosticTriageRun,
};
pub use diagnostic_triage_check::{
    NJOY_DIAGNOSTIC_TRIAGE_CHECK_SCHEMA, NjoyDiagnosticTriageCheckError,
    NjoyDiagnosticTriageCheckResult, NjoyDiagnosticTriageCheckVerification,
};
pub use domain_aware::{
    NJOY_DOMAIN_AWARE_SUITABILITY_SCHEMA, NjoyDomainAwareSuitabilityError,
    NjoyDomainAwareSuitabilityReport, NjoyDomainAwareSuitabilityReportDocument,
    NjoyDomainAwareSuitabilityResult, NjoyDomainAwareSuitabilityRun,
};
pub use energy_balance_attribution::{
    DEFAULT_NJOY_ENERGY_BALANCE_PRINT_RELATIVE_TOLERANCE, NJOY_ENERGY_BALANCE_ATTRIBUTION_SCHEMA,
    NjoyEnergyBalanceAttribution, NjoyEnergyBalanceAttributionDocument,
    NjoyEnergyBalanceAttributionError, NjoyEnergyBalanceAttributionQualification,
    NjoyEnergyBalanceAttributionResult, NjoyEnergyBalanceAttributionSample,
    NjoyEnergyBalanceAttributionStatus, NjoyEnergyBalanceContribution,
    NjoyEnergyBalanceEvidenceScope, NjoyEnergyBalanceFindingDisposition,
};
pub use evidence_aware::{
    NJOY_EVIDENCE_AWARE_SUITABILITY_SCHEMA, NjoyEvidenceAwareIndependentGate,
    NjoyEvidenceAwareKinematicDisposition, NjoyEvidenceAwareSuitabilityError,
    NjoyEvidenceAwareSuitabilityReport, NjoyEvidenceAwareSuitabilityReportDocument,
    NjoyEvidenceAwareSuitabilityResult, NjoyEvidenceAwareSuitabilityRun,
};

pub use execution::{
    DEFAULT_NJOY_TIMEOUT_SECONDS, NJOY_EXECUTION_RECEIPT_FILENAME, NJOY_EXECUTION_RECEIPT_SCHEMA,
    NjoyExecutionArtifact, NjoyExecutionEnvironment, NjoyExecutionError, NjoyExecutionOptions,
    NjoyExecutionQualification, NjoyExecutionReceipt, NjoyExecutionReceiptDocument,
    NjoyExecutionResult, NjoyExecutionRun, NjoyExecutionTape, NjoyKinematicDirection,
    NjoyKinematicViolation, NjoyProcessorArtifact, NjoyProcessorExecutionIdentity,
    NjoyRunDiagnosticStatus, NjoyTapePurpose,
};
pub use law7_breakup::{
    DEFAULT_LAW7_BREAKUP_NORMALIZATION_TOLERANCE, DEFAULT_LAW7_BREAKUP_RELATIVE_ENERGY_TOLERANCE,
    ENDF_MF6_LAW7_IMPLICIT_RESIDUAL_SCHEMA, EndfMf6Law7ImplicitResidualError,
    EndfMf6Law7ImplicitResidualQualification, EndfMf6Law7ImplicitResidualReport,
    EndfMf6Law7ImplicitResidualReportDocument, EndfMf6Law7ImplicitResidualResult,
    EndfMf6Law7ImplicitResidualSample, EndfMf6Law7ImplicitResidualSampleStatus,
    EndfMf6Law7ImplicitResidualScope, EndfMf6Law7ReferenceFrame,
};
pub use law7_breakup_comparison::{
    DEFAULT_NJOY_LAW7_PRINT_RELATIVE_TOLERANCE, DEFAULT_NJOY_LAW7_SOURCE_RELATIVE_TOLERANCE,
    NJOY_LAW7_IMPLICIT_RESIDUAL_COMPARISON_SCHEMA, NjoyLaw7ImplicitResidualComparison,
    NjoyLaw7ImplicitResidualComparisonDocument, NjoyLaw7ImplicitResidualComparisonError,
    NjoyLaw7ImplicitResidualComparisonQualification, NjoyLaw7ImplicitResidualComparisonResult,
    NjoyLaw7ImplicitResidualComparisonSample, NjoyLaw7ImplicitResidualComparisonStatus,
};
pub use photon_inventory::{
    ENDF_PHOTON_PRODUCTION_INVENTORY_SCHEMA, EndfFile6PhotonProduct, EndfFile12Representation,
    EndfFile13Representation, EndfFile14Representation, EndfFile15Representation,
    EndfPhotonFormatFinding, EndfPhotonFormatFindingKind, EndfPhotonInventoryError,
    EndfPhotonInventoryEvaluation, EndfPhotonInventoryQualification, EndfPhotonInventorySource,
    EndfPhotonProductionInventory, EndfPhotonProductionInventoryDocument,
    EndfPhotonProductionInventoryResult, EndfPhotonReaction, EndfPhotonSection,
    EndfPhotonSectionHeader, HeatrPhotonSource,
};
pub use photon_moment::{
    DEFAULT_SPECTRUM_NORMALIZATION_TOLERANCE, ENDF_CONTINUUM_PHOTON_MOMENT_SCHEMA,
    EndfContinuumPhotonMomentReaction, EndfContinuumPhotonMomentReport,
    EndfContinuumPhotonMomentReportDocument, EndfContinuumPhotonMomentResult,
    EndfContinuumPhotonMomentSample, EndfPhotonMomentError, EndfPhotonMomentQualification,
    EndfPhotonMomentSampleStatus, EndfPhotonMomentScope,
};
pub use photon_moment_comparison::{
    DEFAULT_NJOY_PRINT_RELATIVE_TOLERANCE, NJOY_PHOTON_MOMENT_COMPARISON_SCHEMA,
    NjoyPhotonMomentComparison, NjoyPhotonMomentComparisonDocument,
    NjoyPhotonMomentComparisonError, NjoyPhotonMomentComparisonQualification,
    NjoyPhotonMomentComparisonReaction, NjoyPhotonMomentComparisonResult,
    NjoyPhotonMomentComparisonSample, NjoyPhotonMomentComparisonStatus,
};
pub use reaction_energy_balance::{
    DEFAULT_REACTION_BALANCE_RELATIVE_TOLERANCE, ENDF_REACTION_ENERGY_BALANCE_SCHEMA,
    EndfProductAngularRepresentation, EndfProductDisposition, EndfReactionBalanceError,
    EndfReactionBalanceEvidenceScope, EndfReactionBalanceFindingDisposition,
    EndfReactionBalanceProduct, EndfReactionBalanceQualification, EndfReactionBalanceReaction,
    EndfReactionBalanceReactionStatus, EndfReactionBalanceSample, EndfReactionBalanceSampleStatus,
    EndfReactionEnergyBalanceDocument, EndfReactionEnergyBalanceReport,
    EndfReactionEnergyBalanceResult,
};
pub use response_tables::{
    AVOGADRO_CONSTANT_PER_MOL, CM2_PER_BARN, CarriedFindingSummary, ExtractedTableRecord,
    ExtractedTableRole, NEUTRON_MASS_U, NEUTRON_RESPONSE_SET_SCHEMA,
    NJOY_RESPONSE_SET_REVIEW_SCHEMA, NJOY_RESPONSE_TABLE_GENERATION_SCHEMA,
    NjoyResponseSetCheckStatus, NjoyResponseSetReviewDocument, NjoyResponseSetReviewReport,
    NjoyResponseSetReviewResult, NjoyResponseSetReviewScope, NjoyResponseSetReviewWriteResult,
    NjoyResponseTableGeneration, NjoyResponseTableGenerationReport,
    NjoyResponseTableGenerationResult, NjoyResponseTableInputs, NjoyResponseTableQualification,
    NuclideTableExtraction, PartialChannelExtraction, ResidualComponentRecord, ResponseTableError,
    ResponseTableFindingDisposition, load_generation_report, load_response_set,
};
pub use source_aware::{
    NJOY_SOURCE_AWARE_SUITABILITY_SCHEMA, NjoyProcessorFindingDisposition,
    NjoySourceAwareProcessorFinding, NjoySourceAwareSuitabilityError,
    NjoySourceAwareSuitabilityReport, NjoySourceAwareSuitabilityReportDocument,
    NjoySourceAwareSuitabilityResult, NjoySourceAwareSuitabilityRun,
};
pub use suitability::{
    NJOY_SUITABILITY_REPORT_SCHEMA, NjoyProcessorDataFinding, NjoySuitabilityError,
    NjoySuitabilityFindingKind, NjoySuitabilityQualification, NjoySuitabilityReport,
    NjoySuitabilityReportDocument, NjoySuitabilityResult, NjoySuitabilityRun,
    NjoySuitabilityStatus, NjoyTransportRequirement,
};

pub const NJOY_INPUT_MANIFEST_SCHEMA: &str = "openbnct.njoy-input-manifest/0.1.0";
pub const NJOY_INPUT_MANIFEST_MIXED_SCHEMA: &str = "openbnct.njoy-input-manifest/0.2.0";
pub const TARGET_NJOY_NAME: &str = "NJOY2016";
pub const TARGET_NJOY_VERSION: &str = "2016.78";
pub const TARGET_NJOY_SOURCE_COMMIT: &str = "71a76bc6345fa15f36bacc816ae7900714345d97";

const ENDF_TAPE: u16 = 20;
const RECONSTRUCTED_TAPE: u16 = 21;
const BROADENED_TAPE: u16 = 22;
const HEATR_TAPE: u16 = 23;
const CHECK_TAPE: u16 = 24;
const CHECK_PLOT_TAPE: u16 = 25;
const KINEMATIC_KERMA_MT: u16 = 443;
const NORMAL_HEATR_TEMPERATURE_COUNT: u16 = 0;
const NORMAL_HEATR_PRINT_OPTION: u16 = 0;
const DIAGNOSTIC_HEATR_TEMPERATURE_COUNT: u16 = 1;
const DIAGNOSTIC_HEATR_PRINT_OPTION: u16 = 2;

pub struct NjoyInputArtifacts<'a> {
    pub evaluated_source_selection_json: &'a [u8],
    pub material_json: &'a [u8],
    pub generation_method_json: &'a [u8],
    pub acquisitions: Vec<NjoyAcquisitionArtifacts<'a>>,
}

#[derive(Debug, Clone, Copy)]
pub struct NjoyAcquisitionArtifacts<'a> {
    pub profile_json: &'a [u8],
    pub receipt_json: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyInputManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyInputQualification,
    pub processor: ToolIdentity,
    pub bindings: NjoyInputBindings,
    pub settings: NjoyProcessingSettings,
    pub runs: Vec<NjoyNuclideRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyInputQualification {
    InputPreparationOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyInputBindings {
    pub evaluated_source_selection: ContentReference,
    pub material: ContentReference,
    pub generation_method: ContentReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition_profile_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition_receipt_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisitions: Option<Vec<NjoyAcquisitionBinding>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyAcquisitionBinding {
    pub profile_sha256: String,
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyProcessingSettings {
    pub temperature_k: f64,
    pub reconstruction_tolerance_fraction: f64,
    pub local_photon_deposition: bool,
    pub q_value_override_count: u16,
    pub kinematic_checks: bool,
    pub normal_heatr_temperature_count: u16,
    pub normal_heatr_print_option: u16,
    pub diagnostic_heatr_temperature_count: u16,
    pub diagnostic_heatr_print_option: u16,
    pub total_kerma_mt: u16,
    pub kinematic_kerma_mt: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyNuclideRun {
    pub nuclide: String,
    pub endf_mat: u16,
    pub source_evaluation: NjoySourceEvaluation,
    pub requested_partial_reaction_mts: Vec<u16>,
    pub generated_kerma_mts: Vec<u16>,
    pub input_deck: NjoyManifestArtifact,
    pub tapes: NjoyTapePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoySourceEvaluation {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyManifestArtifact {
    pub path: String,
    pub media_type: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyTapePlan {
    pub endf_input: u16,
    pub reconstructed_pendf: u16,
    pub broadened_pendf: u16,
    pub heatr_pendf: u16,
    pub kinematic_check_pendf: u16,
    pub kinematic_check_plot: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedNjoyFile {
    pub relative_path: String,
    pub media_type: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyInputBundle {
    pub manifest: NjoyInputManifest,
    pub files: Vec<GeneratedNjoyFile>,
}

impl NjoyInputBundle {
    /// Validate all content bindings and source files, then generate byte-stable
    /// NJOY decks. No external program is invoked.
    pub fn generate(
        evaluations_root: &Path,
        artifacts: NjoyInputArtifacts<'_>,
    ) -> Result<Self, NjoyPreparationError> {
        let selection = EvaluatedNeutronSourceSelectionDocument::from_bytes(
            artifacts.evaluated_source_selection_json,
        )?;
        let material: MaterialDefinition = serde_json::from_slice(artifacts.material_json)
            .map_err(|source| NjoyPreparationError::InvalidJson {
                artifact: "material",
                source,
            })?;
        let method: ResponseGenerationMethod =
            serde_json::from_slice(artifacts.generation_method_json).map_err(|source| {
                NjoyPreparationError::InvalidJson {
                    artifact: "generation_method",
                    source,
                }
            })?;
        let mut acquisitions = Vec::with_capacity(artifacts.acquisitions.len());
        for acquisition in &artifacts.acquisitions {
            let profile = DataAcquisitionProfileDocument::from_bytes(acquisition.profile_json)?;
            let receipt = DataAcquisitionReceiptDocument::from_bytes(acquisition.receipt_json)?;
            acquisitions.push((profile, receipt));
        }
        let pairs = acquisitions
            .iter()
            .map(|(profile, receipt)| (profile, receipt))
            .collect::<Vec<_>>();

        selection
            .selection
            .validate_for_material(&material, artifacts.material_json)?;
        selection.selection.validate_acquisitions(&pairs)?;
        selection.selection.verify_files(evaluations_root)?;
        Self::generate_from_documents(
            &selection,
            &material,
            artifacts.material_json,
            &method,
            artifacts.generation_method_json,
        )
    }

    fn generate_from_documents(
        selection: &EvaluatedNeutronSourceSelectionDocument,
        material: &MaterialDefinition,
        material_json: &[u8],
        method: &ResponseGenerationMethod,
        method_json: &[u8],
    ) -> Result<Self, NjoyPreparationError> {
        method.validate()?;
        if method.qualification != MethodQualification::MethodFrozenTablesPending {
            return Err(NjoyPreparationError::UnsupportedMethodQualification);
        }
        if method.processor.name != TARGET_NJOY_NAME
            || method.processor.version != TARGET_NJOY_VERSION
            || method.processor.source_commit != TARGET_NJOY_SOURCE_COMMIT
        {
            return Err(NjoyPreparationError::UnsupportedProcessor);
        }
        if method.material != selection.selection.material
            || method.material.id != material.id
            || method.material.sha256 != sha256_bytes(material_json)
        {
            return Err(NjoyPreparationError::MaterialBindingMismatch);
        }
        if method.evaluated_data_release != selection.selection.evaluated_data_release {
            return Err(NjoyPreparationError::EvaluatedDataBindingMismatch);
        }
        if method.heatr.local_photon_deposition
            || method.heatr.allow_q_value_overrides
            || !method.heatr.kinematic_checks
            || method.heatr.total_kerma_mt != 301
            || method.heatr.kinematic_total_mt != KINEMATIC_KERMA_MT
        {
            return Err(NjoyPreparationError::UnsupportedHeatrSettings);
        }

        let partials_by_nuclide = method
            .heatr
            .partials
            .iter()
            .map(|partial| (partial.nuclide.as_str(), partial.reaction_mt))
            .collect::<BTreeMap<_, _>>();
        let selection_reference = ContentReference {
            id: selection.selection.id.clone(),
            sha256: selection.sha256.clone(),
        };
        let method_reference = ContentReference {
            id: method.id.clone(),
            sha256: sha256_bytes(method_json),
        };
        let material_reference = ContentReference {
            id: material.id.clone(),
            sha256: sha256_bytes(material_json),
        };

        let mut files = Vec::with_capacity(selection.selection.evaluations.len() + 1);
        let mut runs = Vec::with_capacity(selection.selection.evaluations.len());
        for evaluation in &selection.selection.evaluations {
            let requested_partial_reaction_mts = partials_by_nuclide
                .get(evaluation.nuclide.as_str())
                .copied()
                .into_iter()
                .collect::<Vec<_>>();
            let deck_bytes = render_deck(
                evaluation,
                &selection.selection.evaluated_data_release,
                method.temperature_k,
                method.reconstruction_tolerance_fraction,
                &requested_partial_reaction_mts,
            );
            let relative_path = format!("{}/input.njoy", evaluation.nuclide);
            let deck_sha256 = sha256_bytes(&deck_bytes);
            files.push(GeneratedNjoyFile {
                relative_path: relative_path.clone(),
                media_type: "text/plain".into(),
                sha256: deck_sha256.clone(),
                bytes: deck_bytes,
            });

            let mut generated_kerma_mts = requested_partial_reaction_mts
                .iter()
                .map(|reaction| reaction + 300)
                .collect::<Vec<_>>();
            generated_kerma_mts.extend([method.heatr.total_kerma_mt, KINEMATIC_KERMA_MT]);
            generated_kerma_mts.sort_unstable();
            runs.push(NjoyNuclideRun {
                nuclide: evaluation.nuclide.clone(),
                endf_mat: evaluation.endf_mat,
                source_evaluation: NjoySourceEvaluation {
                    filename: evaluation.extracted_filename.clone(),
                    size_bytes: evaluation.size_bytes,
                    sha256: evaluation.sha256.clone(),
                },
                requested_partial_reaction_mts,
                generated_kerma_mts,
                input_deck: NjoyManifestArtifact {
                    path: relative_path,
                    media_type: "text/plain".into(),
                    sha256: deck_sha256,
                },
                tapes: canonical_tape_plan(),
            });
        }

        let acquisition_bindings = selection
            .selection
            .declared_acquisitions()
            .iter()
            .map(|acquisition| NjoyAcquisitionBinding {
                profile_sha256: acquisition.profile_sha256.clone(),
                receipt_sha256: acquisition.receipt_sha256.clone(),
            })
            .collect::<Vec<_>>();
        let (manifest_schema, single_binding, binding_list) = if openbnct_core::schema_matches(
            &selection.selection.schema_version,
            EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA,
        ) {
            (
                NJOY_INPUT_MANIFEST_MIXED_SCHEMA,
                (None, None),
                Some(acquisition_bindings),
            )
        } else {
            let only = acquisition_bindings
                .first()
                .cloned()
                .unwrap_or(NjoyAcquisitionBinding {
                    profile_sha256: String::new(),
                    receipt_sha256: String::new(),
                });
            (
                NJOY_INPUT_MANIFEST_SCHEMA,
                (Some(only.profile_sha256), Some(only.receipt_sha256)),
                None,
            )
        };
        let manifest = NjoyInputManifest {
            schema_version: manifest_schema.into(),
            id: format!("{}.njoy2016-78-inputs", selection.selection.id),
            case_id: selection.selection.case_id.clone(),
            qualification: NjoyInputQualification::InputPreparationOnly,
            processor: method.processor.clone(),
            bindings: NjoyInputBindings {
                evaluated_source_selection: selection_reference,
                material: material_reference,
                generation_method: method_reference,
                acquisition_profile_sha256: single_binding.0,
                acquisition_receipt_sha256: single_binding.1,
                acquisitions: binding_list,
            },
            settings: NjoyProcessingSettings {
                temperature_k: method.temperature_k,
                reconstruction_tolerance_fraction: method.reconstruction_tolerance_fraction,
                local_photon_deposition: method.heatr.local_photon_deposition,
                q_value_override_count: 0,
                kinematic_checks: method.heatr.kinematic_checks,
                normal_heatr_temperature_count: NORMAL_HEATR_TEMPERATURE_COUNT,
                normal_heatr_print_option: NORMAL_HEATR_PRINT_OPTION,
                diagnostic_heatr_temperature_count: DIAGNOSTIC_HEATR_TEMPERATURE_COUNT,
                diagnostic_heatr_print_option: DIAGNOSTIC_HEATR_PRINT_OPTION,
                total_kerma_mt: method.heatr.total_kerma_mt,
                kinematic_kerma_mt: method.heatr.kinematic_total_mt,
            },
            runs,
        };
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        manifest_bytes.push(b'\n');
        files.push(GeneratedNjoyFile {
            relative_path: "openbnct-njoy-input-manifest.json".into(),
            media_type: "application/json".into(),
            sha256: sha256_bytes(&manifest_bytes),
            bytes: manifest_bytes,
        });
        Ok(Self { manifest, files })
    }

    /// Write the generated bundle to a new directory without overwriting any
    /// existing path. A failed write leaves the partial directory for review.
    pub fn write_new(&self, output: &Path) -> Result<(), NjoyPreparationError> {
        fs::create_dir(output).map_err(|source| NjoyPreparationError::Io {
            path: output.to_path_buf(),
            source,
        })?;
        for file in &self.files {
            validate_relative_path(&file.relative_path)?;
            let path = output.join(&file.relative_path);
            if let Some(parent) = path.parent()
                && parent != output
            {
                fs::create_dir(parent).map_err(|source| NjoyPreparationError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let mut stream = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|source| NjoyPreparationError::Io {
                    path: path.clone(),
                    source,
                })?;
            stream
                .write_all(&file.bytes)
                .and_then(|()| stream.sync_all())
                .map_err(|source| NjoyPreparationError::Io { path, source })?;
        }
        Ok(())
    }

    /// Require a directory to contain exactly the generated bundle bytes, with
    /// no symlinks, missing files, extra files, or extra directories.
    pub fn verify_directory(&self, root: &Path) -> Result<(), NjoyPreparationError> {
        let root_metadata =
            fs::symlink_metadata(root).map_err(|source| NjoyPreparationError::Io {
                path: root.to_path_buf(),
                source,
            })?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(NjoyPreparationError::InputBundleRootNotDirectory(
                root.to_path_buf(),
            ));
        }

        let expected = self
            .files
            .iter()
            .map(|file| (file.relative_path.as_str(), file))
            .collect::<BTreeMap<_, _>>();
        if expected.len() != self.files.len() {
            return Err(NjoyPreparationError::DuplicateGeneratedOutputPath);
        }
        let mut observed = BTreeSet::new();
        collect_bundle_files(root, root, &expected, &mut observed)?;
        if observed != expected.keys().copied().collect::<BTreeSet<_>>() {
            return Err(NjoyPreparationError::InputBundleFileSetMismatch);
        }

        for (relative_path, generated) in expected {
            let path = root.join(relative_path);
            let bytes = fs::read(&path).map_err(|source| NjoyPreparationError::Io {
                path: path.clone(),
                source,
            })?;
            if bytes != generated.bytes || sha256_bytes(&bytes) != generated.sha256 {
                return Err(NjoyPreparationError::InputBundleArtifactMismatch(
                    relative_path.into(),
                ));
            }
        }
        Ok(())
    }
}

fn collect_bundle_files<'a>(
    root: &Path,
    directory: &Path,
    expected: &BTreeMap<&'a str, &'a GeneratedNjoyFile>,
    observed: &mut BTreeSet<&'a str>,
) -> Result<(), NjoyPreparationError> {
    let entries = fs::read_dir(directory).map_err(|source| NjoyPreparationError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| NjoyPreparationError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| NjoyPreparationError::UnexpectedInputBundleEntry(path.clone()))?;
        let relative_text = relative.to_str().ok_or_else(|| {
            NjoyPreparationError::UnexpectedInputBundleEntry(relative.to_path_buf())
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| NjoyPreparationError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(NjoyPreparationError::UnexpectedInputBundleEntry(
                relative.to_path_buf(),
            ));
        }
        if metadata.is_file() {
            let expected_path = expected
                .get_key_value(relative_text)
                .map(|(key, _)| *key)
                .ok_or_else(|| {
                    NjoyPreparationError::UnexpectedInputBundleEntry(relative.to_path_buf())
                })?;
            observed.insert(expected_path);
        } else if metadata.is_dir() {
            let prefix = format!("{relative_text}/");
            if !expected.keys().any(|path| path.starts_with(&prefix)) {
                return Err(NjoyPreparationError::UnexpectedInputBundleEntry(
                    relative.to_path_buf(),
                ));
            }
            collect_bundle_files(root, &path, expected, observed)?;
        } else {
            return Err(NjoyPreparationError::UnexpectedInputBundleEntry(
                relative.to_path_buf(),
            ));
        }
    }
    Ok(())
}

fn render_deck(
    evaluation: &EvaluatedNeutronArtifact,
    evaluated_data_release: &str,
    temperature_k: f64,
    tolerance: f64,
    partial_reaction_mts: &[u16],
) -> Vec<u8> {
    let mut requested = partial_reaction_mts
        .iter()
        .map(|reaction_mt| reaction_mt + 300)
        .collect::<Vec<_>>();
    requested.push(KINEMATIC_KERMA_MT);
    requested.sort_unstable();
    let requested_text = requested
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let title = format!("NCTForge {evaluated_data_release} {}", evaluation.nuclide);
    format!(
        "reconr\n\
         {ENDF_TAPE} {RECONSTRUCTED_TAPE}\n\
         '{title} reconstructed pointwise data'/\n\
         {} 2/\n\
         {tolerance:.12e}/\n\
         'NCTForge partial-KERMA response generation'/\n\
         'No Q-value overrides'/\n\
         0/\n\
         broadr\n\
         {ENDF_TAPE} {RECONSTRUCTED_TAPE} {BROADENED_TAPE}\n\
         {} 1 0 0 0. /\n\
         {tolerance:.12e}/\n\
         {temperature_k:.6}\n\
         0/\n\
         heatr\n\
         {ENDF_TAPE} {BROADENED_TAPE} {HEATR_TAPE} 0 /\n\
         {} {} 0 {NORMAL_HEATR_TEMPERATURE_COUNT} 0 {NORMAL_HEATR_PRINT_OPTION} 0 /\n\
         {requested_text} /\n\
         heatr\n\
         {ENDF_TAPE} {BROADENED_TAPE} {CHECK_TAPE} {CHECK_PLOT_TAPE} /\n\
         {} {} 0 {DIAGNOSTIC_HEATR_TEMPERATURE_COUNT} 0 {DIAGNOSTIC_HEATR_PRINT_OPTION} 0 /\n\
         {requested_text} /\n\
         stop\n",
        evaluation.endf_mat,
        evaluation.endf_mat,
        evaluation.endf_mat,
        requested.len(),
        evaluation.endf_mat,
        requested.len(),
    )
    .into_bytes()
}

fn canonical_tape_plan() -> NjoyTapePlan {
    NjoyTapePlan {
        endf_input: ENDF_TAPE,
        reconstructed_pendf: RECONSTRUCTED_TAPE,
        broadened_pendf: BROADENED_TAPE,
        heatr_pendf: HEATR_TAPE,
        kinematic_check_pendf: CHECK_TAPE,
        kinematic_check_plot: CHECK_PLOT_TAPE,
    }
}

fn validate_relative_path(value: &str) -> Result<(), NjoyPreparationError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(NjoyPreparationError::InvalidOutputPath(value.into()));
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Error)]
pub enum NjoyPreparationError {
    #[error("invalid {artifact} JSON: {source}")]
    InvalidJson {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Acquisition(#[from] openbnct_openmc::AcquisitionError),
    #[error(transparent)]
    EvaluatedSource(#[from] EvaluatedSourceError),
    #[error(transparent)]
    ResponseMethod(#[from] ResponseMethodError),
    #[error("response-generation method qualification is unsupported")]
    UnsupportedMethodQualification,
    #[error("response-generation method does not pin NJOY2016.78 at the frozen commit")]
    UnsupportedProcessor,
    #[error("response-generation method material binding does not match the source selection")]
    MaterialBindingMismatch,
    #[error("response-generation method evaluated-data binding does not match the selection")]
    EvaluatedDataBindingMismatch,
    #[error("response-generation HEATR settings do not match the frozen method")]
    UnsupportedHeatrSettings,
    #[error("generated output path is not normalized and relative: {0:?}")]
    InvalidOutputPath(String),
    #[error("NJOY input-bundle root is not a real directory: {0}")]
    InputBundleRootNotDirectory(PathBuf),
    #[error("generated NJOY bundle contains a duplicate output path")]
    DuplicateGeneratedOutputPath,
    #[error("unexpected or unsafe NJOY input-bundle entry: {0}")]
    UnexpectedInputBundleEntry(PathBuf),
    #[error("NJOY input bundle does not contain exactly the generated file set")]
    InputBundleFileSetMismatch,
    #[error("NJOY input-bundle artifact does not match generated bytes: {0}")]
    InputBundleArtifactMismatch(String),
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const SELECTION_JSON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/evaluated-neutron-source-selection.json"
    );
    const MATERIAL_JSON: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const METHOD_JSON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/response-generation-method.json"
    );
    const FROZEN_BUNDLE_ROOT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../benchmarks/synthetic/nf-bnct-001/transport/njoy"
    );

    fn frozen_bundle() -> NjoyInputBundle {
        let selection =
            EvaluatedNeutronSourceSelectionDocument::from_bytes(SELECTION_JSON).unwrap();
        let material: MaterialDefinition = serde_json::from_slice(MATERIAL_JSON).unwrap();
        let method: ResponseGenerationMethod = serde_json::from_slice(METHOD_JSON).unwrap();
        NjoyInputBundle::generate_from_documents(
            &selection,
            &material,
            MATERIAL_JSON,
            &method,
            METHOD_JSON,
        )
        .unwrap()
    }

    #[test]
    fn generates_ten_deterministic_njoy_decks() {
        let first = frozen_bundle();
        let second = frozen_bundle();
        assert_eq!(first, second);
        assert_eq!(first.manifest.runs.len(), 10);
        assert_eq!(first.files.len(), 11);
        assert_eq!(
            first.manifest.processor.source_commit,
            TARGET_NJOY_SOURCE_COMMIT
        );
        assert_eq!(first.manifest.settings.normal_heatr_temperature_count, 0);
        assert_eq!(first.manifest.settings.normal_heatr_print_option, 0);
        assert_eq!(
            first.manifest.settings.diagnostic_heatr_temperature_count,
            1
        );
        assert_eq!(first.manifest.settings.diagnostic_heatr_print_option, 2);

        let boron = first
            .manifest
            .runs
            .iter()
            .find(|run| run.nuclide == "B10")
            .unwrap();
        assert_eq!(boron.endf_mat, 525);
        assert_eq!(boron.requested_partial_reaction_mts, [107]);
        assert_eq!(boron.generated_kerma_mts, [301, 407, 443]);
        let deck = first
            .files
            .iter()
            .find(|file| file.relative_path == "B10/input.njoy")
            .unwrap();
        let deck_text = std::str::from_utf8(&deck.bytes).unwrap();
        assert!(deck_text.contains("525 2 0 0 0 0 0 /\n407 443 /"));
        assert!(deck_text.contains("525 2 0 1 0 2 0 /\n407 443 /"));
        assert!(!deck_text.contains("\n107 443 /"));
        assert!(deck_text.ends_with("stop\n"));

        for generated in &first.files {
            let frozen = fs::read(Path::new(FROZEN_BUNDLE_ROOT).join(&generated.relative_path))
                .unwrap_or_else(|error| {
                    panic!(
                        "failed to read frozen artifact {}: {error}",
                        generated.relative_path
                    )
                });
            if generated.relative_path == "openbnct-njoy-input-manifest.json" {
                // The frozen manifest carries its pre-rename `nctforge.`
                // schema id; parsing normalizes it, so reserialization yields
                // the current canonical bytes.
                let parsed: NjoyInputManifest =
                    serde_json::from_slice(&frozen).expect("parse frozen manifest");
                let mut expected = serde_json::to_vec_pretty(&parsed).unwrap();
                expected.push(b'\n');
                assert_eq!(
                    generated.bytes, expected,
                    "{} drifted",
                    generated.relative_path
                );
            } else {
                assert_eq!(
                    generated.bytes, frozen,
                    "{} drifted",
                    generated.relative_path
                );
            }
        }
        let manifest_file = first
            .files
            .iter()
            .find(|file| file.relative_path == "openbnct-njoy-input-manifest.json")
            .unwrap();
        let parsed_frozen: NjoyInputManifest = serde_json::from_slice(
            &fs::read(Path::new(FROZEN_BUNDLE_ROOT).join("openbnct-njoy-input-manifest.json"))
                .unwrap(),
        )
        .unwrap();
        let mut canonical = serde_json::to_vec_pretty(&parsed_frozen).unwrap();
        canonical.push(b'\n');
        assert_eq!(manifest_file.sha256, sha256_bytes(&canonical));
    }

    #[test]
    fn partial_channels_are_only_requested_for_boron_and_nitrogen() {
        let bundle = frozen_bundle();
        for run in &bundle.manifest.runs {
            match run.nuclide.as_str() {
                "B10" => assert_eq!(run.requested_partial_reaction_mts, [107]),
                "N14" => assert_eq!(run.requested_partial_reaction_mts, [103]),
                _ => assert!(run.requested_partial_reaction_mts.is_empty()),
            }
            assert!(run.generated_kerma_mts.contains(&301));
            assert!(run.generated_kerma_mts.contains(&443));
        }
    }

    #[test]
    fn writer_refuses_an_existing_output_directory() {
        let bundle = frozen_bundle();
        let output = tempfile::tempdir().unwrap();
        assert!(matches!(
            bundle.write_new(output.path()),
            Err(NjoyPreparationError::Io { .. })
        ));
    }

    #[test]
    fn exact_input_bundle_verifier_rejects_extra_files() {
        let bundle = frozen_bundle();
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("bundle");
        bundle.write_new(&output).unwrap();
        bundle.verify_directory(&output).unwrap();

        fs::write(output.join("unexpected.txt"), b"unexpected\n").unwrap();
        assert!(matches!(
            bundle.verify_directory(&output),
            Err(NjoyPreparationError::UnexpectedInputBundleEntry(_))
        ));
    }

    #[test]
    fn exact_input_bundle_verifier_rejects_changed_bytes() {
        let bundle = frozen_bundle();
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("bundle");
        bundle.write_new(&output).unwrap();

        fs::write(output.join("B10/input.njoy"), b"changed\n").unwrap();
        assert!(matches!(
            bundle.verify_directory(&output),
            Err(NjoyPreparationError::InputBundleArtifactMismatch(path)) if path == "B10/input.njoy"
        ));
    }
}
