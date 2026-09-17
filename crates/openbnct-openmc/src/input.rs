// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::io;
use std::path::Path;

use openbnct_core::{ContentReference, DoseComponent, GridGeometry};
use openbnct_transport::{
    AngularDistribution, ComponentDefinitionProfile, EnergyDistribution, FixedSourceDefinition,
    MATERIAL_ASSIGNMENT_SCHEMA, MaterialAssignment, MaterialDefinition, NeutronResponseSet,
    ParticleType, ResolvedWeightWindows, SourceSpatialDistribution, TransportCase,
};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    NuclearDataError, NuclearDataManifest, TARGET_OPENMC_SOURCE_COMMIT, TARGET_OPENMC_VERSION,
    TEMPERATURE_TOLERANCE_K,
};

pub const OPENMC_DEFAULT_STRIDE: u64 = 152_917;
pub const CANDIDATE_REFERENCE_SEEDS: [u64; 3] = [20260831, 314159265, 271828182];
const EXECUTION_PROFILE_SCHEMA: &str = "openbnct.openmc-execution-profile/0.1.0";
const EXECUTION_PROFILE_SCHEMA_V2: &str = "openbnct.openmc-execution-profile/0.2.0";
const INPUT_MANIFEST_SCHEMA: &str = "openbnct.openmc-input-manifest/0.1.0";
const INPUT_MANIFEST_SCHEMA_V2: &str = "openbnct.openmc-input-manifest/0.2.0";
pub const ACCEPTANCE_CONTRACT_SCHEMA: &str = "openbnct.acceptance-contract/0.1.0";
const XML_MEDIA_TYPE: &str = "application/xml";
const JSON_MEDIA_TYPE: &str = "application/json";
const IDENTITY_DIRECTION: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
const DIRECTION_TOLERANCE: f64 = 1.0e-12;

const MESH_ID: u32 = 1;
const MESH_FILTER_ID: u32 = 1;
const NEUTRON_FILTER_ID: u32 = 2;
const PHOTON_FILTER_ID: u32 = 3;
const BORON_RESPONSE_FILTER_ID: u32 = 4;
const NITROGEN_RESPONSE_FILTER_ID: u32 = 5;
const HYDROGEN_RESPONSE_FILTER_ID: u32 = 6;
const NEUTRON_ENERGY_FILTER_ID: u32 = 7;
const PHOTON_ENERGY_FILTER_ID: u32 = 8;
const SURFACE_FILTER_ID: u32 = 9;

const BORON_TALLY_ID: u32 = 1;
const NITROGEN_TALLY_ID: u32 = 2;
const HYDROGEN_TALLY_ID: u32 = 3;
const NEUTRON_HEATING_TALLY_ID: u32 = 4;
const PHOTON_HEATING_TALLY_ID: u32 = 5;
const COUPLED_HEATING_TALLY_ID: u32 = 6;
const BORON_REACTION_TALLY_ID: u32 = 7;
const NITROGEN_REACTION_TALLY_ID: u32 = 8;
const NEUTRON_FLUX_TALLY_ID: u32 = 9;
const PHOTON_FLUX_TALLY_ID: u32 = 10;
const NEUTRON_LEAKAGE_TALLY_ID: u32 = 11;
const PHOTON_LEAKAGE_TALLY_ID: u32 = 12;

// Acceptance ROI meshes and their mesh filters start here; each region gets
// one mesh, one mesh filter, and ROI_TALLIES_PER_REGION tallies.
const ROI_MESH_ID_BASE: u32 = 2;
const ROI_MESH_FILTER_ID_BASE: u32 = 10;
const ROI_TALLY_ID_BASE: u32 = 21;
/// Mesh ids for weight-window meshes declared inside `settings.xml`.
/// Kept far above the scoring/ROI mesh ids so a declared window mesh can
/// never collide with a tally-side mesh.
const WW_MESH_ID_BASE: u32 = 1_000_000;
const ROI_TALLIES_PER_REGION: u32 = 9;

/// Versioned controls that materially affect one OpenMC input deck.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcExecutionProfile {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub purpose: OpenMcExecutionPurpose,
    pub openmc_version: String,
    pub openmc_source_commit: String,
    pub run_mode: OpenMcRunMode,
    pub batches: u32,
    pub seed: u64,
    pub stride: u64,
    pub photon_transport: bool,
    pub atomic_relaxation: bool,
    pub electron_treatment: OpenMcElectronTreatment,
    pub energy_mode: OpenMcEnergyMode,
    pub probability_tables: bool,
    pub survival_biasing: bool,
    pub temperature_method: OpenMcTemperatureMethod,
    pub temperature_tolerance_k: f64,
    pub temperature_multipole: bool,
    pub confidence_intervals: bool,
    pub write_summary: bool,
    pub write_ascii_tallies: bool,
    pub write_sourcepoint: bool,
    pub event_based: bool,
    pub neutron_diagnostic_energy_grid_ev: Vec<f64>,
    pub photon_diagnostic_energy_grid_ev: Vec<f64>,
    /// Run scale override introduced by profile schema 0.2.0. When absent the
    /// case's `requested_histories` applies. Required for candidate-reference
    /// executions, whose history count is larger than the smoke case value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_histories: Option<u64>,
}

impl OpenMcExecutionProfile {
    pub fn validate(&self) -> Result<(), OpenMcProfileError> {
        if !openbnct_core::schema_matches(&self.schema_version, EXECUTION_PROFILE_SCHEMA)
            && !openbnct_core::schema_matches(&self.schema_version, EXECUTION_PROFILE_SCHEMA_V2)
        {
            return Err(OpenMcProfileError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.requested_histories.is_some()
            && !openbnct_core::schema_matches(&self.schema_version, EXECUTION_PROFILE_SCHEMA_V2)
        {
            return Err(OpenMcProfileError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(OpenMcProfileError::EmptyId);
        }
        if self.openmc_version != TARGET_OPENMC_VERSION {
            return Err(OpenMcProfileError::UnsupportedOpenMcVersion(
                self.openmc_version.clone(),
            ));
        }
        if self.openmc_source_commit != TARGET_OPENMC_SOURCE_COMMIT {
            return Err(OpenMcProfileError::UnsupportedOpenMcCommit(
                self.openmc_source_commit.clone(),
            ));
        }
        if self.batches < 2 {
            return Err(OpenMcProfileError::InsufficientBatches(self.batches));
        }
        if self.purpose == OpenMcExecutionPurpose::CandidateReference {
            if self.batches < 50 {
                return Err(OpenMcProfileError::InsufficientCandidateBatches(
                    self.batches,
                ));
            }
            if !CANDIDATE_REFERENCE_SEEDS.contains(&self.seed) {
                return Err(OpenMcProfileError::UnregisteredCandidateSeed(self.seed));
            }
            match self.requested_histories {
                None => return Err(OpenMcProfileError::MissingCandidateHistories),
                Some(histories) if !histories.is_multiple_of(u64::from(self.batches)) => {
                    return Err(OpenMcProfileError::HistoriesNotDivisibleByBatches {
                        histories,
                        batches: self.batches,
                    });
                }
                _ => {}
            }
        }
        if self.seed == 0 {
            return Err(OpenMcProfileError::ZeroSeed);
        }
        if self.stride != OPENMC_DEFAULT_STRIDE {
            return Err(OpenMcProfileError::UnsupportedSetting("stride"));
        }
        for (accepted, label) in [
            (self.photon_transport, "photon_transport"),
            (self.atomic_relaxation, "atomic_relaxation"),
            (self.probability_tables, "probability_tables"),
            (!self.survival_biasing, "survival_biasing"),
            (!self.temperature_multipole, "temperature_multipole"),
            (!self.confidence_intervals, "confidence_intervals"),
            (self.write_summary, "write_summary"),
            (!self.write_ascii_tallies, "write_ascii_tallies"),
            (!self.write_sourcepoint, "write_sourcepoint"),
            (!self.event_based, "event_based"),
        ] {
            if !accepted {
                return Err(OpenMcProfileError::UnsupportedSetting(label));
            }
        }
        if self.temperature_tolerance_k != TEMPERATURE_TOLERANCE_K {
            return Err(OpenMcProfileError::UnsupportedSetting(
                "temperature_tolerance_k",
            ));
        }
        validate_energy_grid(
            "neutron",
            &self.neutron_diagnostic_energy_grid_ev,
            &[0.5, 1_000.0, 10_000.0],
        )?;
        validate_energy_grid(
            "photon",
            &self.photon_diagnostic_energy_grid_ev,
            &[477_000.0, 479_000.0, 2_223_000.0, 2_225_000.0],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcExecutionPurpose {
    SmokeOnly,
    CandidateReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcRunMode {
    FixedSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcElectronTreatment {
    Led,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenMcEnergyMode {
    ContinuousEnergy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcTemperatureMethod {
    Nearest,
}

/// Predeclared acceptance contract bound into a candidate-reference deck.
///
/// Declares the single-bin reporting regions (which become their own tally
/// meshes so ROI sums carry proper batch statistics), the central-axis depth
/// profile selection, the evaluated mean deposited energies for the
/// reaction-rate audits, the gate tolerances, and the registered seed set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcAcceptanceContract {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub regions: Vec<OpenMcAcceptanceRegion>,
    pub evaluated_mean_deposited_energy_ev: OpenMcEvaluatedDepositedEnergies,
    pub gates: OpenMcAcceptanceGates,
    pub seeds: Vec<u64>,
    pub min_batches: u32,
}

impl OpenMcAcceptanceContract {
    pub fn validate(&self) -> Result<(), OpenMcInputError> {
        if !openbnct_core::schema_matches(&self.schema_version, ACCEPTANCE_CONTRACT_SCHEMA) {
            return Err(OpenMcInputError::UnsupportedAcceptanceSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() || self.case_id.trim().is_empty() {
            return Err(OpenMcInputError::InvalidAcceptance(
                "id and case_id must be nonempty".into(),
            ));
        }
        if self.regions.len() < 2 || self.seeds.len() < 3 || self.min_batches < 50 {
            return Err(OpenMcInputError::InvalidAcceptance(
                "acceptance requires at least two single-bin regions, three seeds, and fifty batches"
                    .into(),
            ));
        }
        for seed in &self.seeds {
            if !CANDIDATE_REFERENCE_SEEDS.contains(seed) {
                return Err(OpenMcInputError::InvalidAcceptance(format!(
                    "seed {seed} is not a registered candidate-reference seed"
                )));
            }
        }
        let mut names = std::collections::BTreeSet::new();
        for region in &self.regions {
            if !names.insert(region.name.as_str()) {
                return Err(OpenMcInputError::InvalidAcceptance(format!(
                    "duplicate region {}",
                    region.name
                )));
            }
            for axis in [
                &region.bounds_cm.x_cm,
                &region.bounds_cm.y_cm,
                &region.bounds_cm.z_cm,
            ] {
                if !axis.iter().all(|v| v.is_finite()) || axis[0] >= axis[1] {
                    return Err(OpenMcInputError::InvalidAcceptance(format!(
                        "region {} has degenerate bounds",
                        region.name
                    )));
                }
            }
            if region.dimensions.iter().any(|dim| *dim == 0 || *dim > 1024) {
                return Err(OpenMcInputError::InvalidAcceptance(format!(
                    "region {} has unsupported dimensions {:?}",
                    region.name, region.dimensions
                )));
            }
        }
        if !self
            .regions
            .iter()
            .any(|region| region.precision_gated && region.dimensions == [1, 1, 1])
        {
            return Err(OpenMcInputError::InvalidAcceptance(
                "at least one single-bin region must carry the precision gates".into(),
            ));
        }
        for gate in [
            self.gates.roi_relative_standard_uncertainty_max,
            self.gates.voxel_median_relative_uncertainty_max,
            self.gates.voxel_p95_relative_uncertainty_max,
            self.gates.reaction_rate_agreement,
            self.gates.neutron_heating_agreement,
            self.gates.coupled_heating_agreement,
            self.gates.chi_square_p_min,
        ] {
            if !gate.is_finite() || gate <= 0.0 {
                return Err(OpenMcInputError::InvalidAcceptance(
                    "gate tolerances must be positive".into(),
                ));
            }
        }
        if !(0.0..1.0).contains(&self.gates.voxel_max_fraction) {
            return Err(OpenMcInputError::InvalidAcceptance(
                "voxel_max_fraction must lie in (0,1)".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcAcceptanceRegion {
    /// Lowercase region token used in tally names (`openbnct.roi.<name>.`).
    pub name: String,
    pub bounds_cm: OpenMcRegionBounds,
    /// Mesh dimensions for the region. `[1,1,1]` gives a single-bin region
    /// whose tally carries proper batch statistics for the ROI sum; a depth
    /// profile uses e.g. `[1,1,40]` so each bin is its own batch-summed slice.
    pub dimensions: [u32; 3],
    /// When true the predeclared precision gates apply to this region's bins.
    pub precision_gated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRegionBounds {
    pub x_cm: [f64; 2],
    pub y_cm: [f64; 2],
    pub z_cm: [f64; 2],
}

/// Evaluated mean deposited energies for the reaction-rate audits, bound to
/// the evidence that derived them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcEvaluatedDepositedEnergies {
    pub b10_mt107_ev: f64,
    pub n14_mt103_ev: f64,
    /// SHA-256 of the evidence document the constants were taken from.
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcAcceptanceGates {
    /// ROI-mean one-sigma relative sampling uncertainty ceiling.
    pub roi_relative_standard_uncertainty_max: f64,
    /// Voxels at or above this fraction of a component's maximum enter the
    /// voxel-level precision gates.
    pub voxel_max_fraction: f64,
    pub voxel_median_relative_uncertainty_max: f64,
    pub voxel_p95_relative_uncertainty_max: f64,
    /// Reaction-rate times evaluated energy versus response estimator.
    pub reaction_rate_agreement: f64,
    /// B+N+H response sum versus dedicated neutron heating.
    pub neutron_heating_agreement: f64,
    /// Component sum versus dedicated coupled heating.
    pub coupled_heating_agreement: f64,
    /// Reduced-chi-square consistency floor across independent seeds.
    pub chi_square_p_min: f64,
}

/// Exact JSON bytes used to create a deck. Hashes are calculated before parse.
#[derive(Debug, Clone, Copy)]
pub struct OpenMcInputArtifacts<'a> {
    pub component_profile_json: &'a [u8],
    pub material_json: &'a [u8],
    pub source_json: &'a [u8],
    pub response_set_json: &'a [u8],
    pub nuclear_data_manifest_json: &'a [u8],
    pub execution_profile_json: &'a [u8],
    /// Acceptance contract required for `candidate_reference` decks and
    /// forbidden otherwise, so smoke manifests remain byte-identical.
    pub acceptance_json: Option<&'a [u8]>,
    /// DICOM-derived voxel-box material assignment. Present only for
    /// structure-derived cases; the deck then emits one CSG cell per region
    /// and one OpenMC material per distinct region material.
    pub material_assignment_json: Option<&'a [u8]>,
    /// Resolved `openbnct.weight-windows/0.1.0` artifact. When present the
    /// deck declares each window mesh in `settings.xml` and emits the
    /// OpenMC `<weight_windows>` entries that enable splitting/roulette.
    pub variance_reduction_json: Option<&'a [u8]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcInputManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub backend_id: String,
    pub openmc_version: String,
    pub openmc_source_commit: String,
    pub bindings: OpenMcInputBindings,
    pub execution: OpenMcRunControls,
    pub scoring_mesh: OpenMcScoringMesh,
    pub tallies: Vec<OpenMcTallyContract>,
    /// Single-bin acceptance region meshes, present only when the deck binds
    /// an acceptance contract (schema 0.2.0).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rois: Vec<OpenMcRoiMesh>,
    pub xml_artifacts: Vec<OpenMcInputManifestArtifact>,
}

/// A single-bin acceptance region realized as its own OpenMC mesh so the
/// region sum carries proper batch statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRoiMesh {
    pub name: String,
    pub mesh_id: u32,
    pub mesh_filter_id: u32,
    pub dimensions: [u32; 3],
    pub lower_left_cm: [f64; 3],
    pub upper_right_cm: [f64; 3],
    pub volume_cm3: f64,
    pub mass_g: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcInputBindings {
    pub component_profile: ContentReference,
    pub material: ContentReference,
    pub source: ContentReference,
    pub response_set: ContentReference,
    pub nuclear_data_manifest: ContentReference,
    pub response_generation_method: ContentReference,
    pub independent_response_review: ContentReference,
    pub execution_profile: ContentReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<ContentReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material_assignment: Option<ContentReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variance_reduction: Option<ContentReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRunControls {
    pub purpose: OpenMcExecutionPurpose,
    pub requested_histories: u64,
    pub batches: u32,
    pub particles_per_batch: u64,
    pub seed: u64,
    pub stride: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcScoringMesh {
    pub mesh_id: u32,
    pub dimensions: [u32; 3],
    pub lower_left_cm: [f64; 3],
    pub upper_right_cm: [f64; 3],
    pub voxel_volume_cm3: f64,
    pub voxel_mass_g: f64,
    pub cell_volume_cm3: f64,
    pub cell_mass_g: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcInputManifestArtifact {
    pub path: String,
    pub sha256: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcTallyContract {
    pub id: u32,
    pub name: String,
    pub component: Option<DoseComponent>,
    pub particle: Option<ParticleType>,
    pub quantity: OpenMcTallyQuantity,
    pub raw_unit: OpenMcRawTallyUnit,
    pub collection_normalization: OpenMcCollectionNormalization,
    /// `dose` tallies feed the physical dose bundle on the scoring mesh;
    /// `acceptance` tallies are validated and evaluated but never collected.
    /// Absent in schema 0.1.0 manifests, where all tallies follow the
    /// historical dose/audit/diagnostic conventions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<OpenMcTallyScope>,
    /// Acceptance region this tally reports on, when `scope` is `acceptance`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roi: Option<String>,
    /// Expected result bins (one for single-bin region tallies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bins: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcTallyScope {
    Dose,
    Acceptance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcTallyQuantity {
    ResponseWeightedTrackLength,
    Heating,
    ReactionRate,
    EnergyBinnedTrackLength,
    SurfaceCurrent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcRawTallyUnit {
    GrayCubicCentimeterPerSourceNeutron,
    ElectronVoltPerSourceNeutron,
    ReactionsPerSourceNeutron,
    CentimeterPerSourceNeutron,
    ParticlesPerSourceNeutron,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcCollectionNormalization {
    DivideByVoxelVolumeCm3,
    ElectronVoltToJouleDivideByVoxelMassKg,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedOpenMcFile {
    pub relative_path: String,
    pub media_type: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcInputDeck {
    pub manifest: OpenMcInputManifest,
    pub files: Vec<GeneratedOpenMcFile>,
}

impl OpenMcInputDeck {
    /// Generate byte-stable OpenMC 0.16 inputs after validating the supplied
    /// JSON bindings and verifying every selected nuclear-data file. This does
    /// not execute OpenMC or qualify results.
    pub fn generate(
        case: &TransportCase,
        nuclear_data_root: &Path,
        artifacts: OpenMcInputArtifacts<'_>,
    ) -> Result<Self, OpenMcInputError> {
        case.validate()
            .map_err(|error| OpenMcInputError::InvalidTransportCase(error.to_string()))?;

        let component_profile: ComponentDefinitionProfile =
            parse_json("component_profile", artifacts.component_profile_json)?;
        component_profile
            .validate()
            .map_err(|error| OpenMcInputError::InvalidComponentProfile(error.to_string()))?;
        let material: MaterialDefinition = parse_json("material", artifacts.material_json)?;
        material
            .validate()
            .map_err(|error| OpenMcInputError::InvalidMaterial(error.to_string()))?;
        let source: FixedSourceDefinition = parse_json("source", artifacts.source_json)?;
        source
            .validate()
            .map_err(|error| OpenMcInputError::InvalidSource(error.to_string()))?;
        let response_set: NeutronResponseSet =
            parse_json("response_set", artifacts.response_set_json)?;
        response_set
            .validate_for_folding()
            .map_err(|error| OpenMcInputError::InvalidResponseSet(error.to_string()))?;
        let nuclear_data: NuclearDataManifest = parse_json(
            "nuclear_data_manifest",
            artifacts.nuclear_data_manifest_json,
        )?;
        let execution_profile: OpenMcExecutionProfile =
            parse_json("execution_profile", artifacts.execution_profile_json)?;
        execution_profile.validate()?;

        // Contract presence and profile purpose must agree before any
        // contract-internal consistency check runs.
        if execution_profile.purpose == OpenMcExecutionPurpose::CandidateReference
            && artifacts.acceptance_json.is_none()
        {
            return Err(OpenMcInputError::CandidateReferenceRequiresAcceptance);
        }
        if execution_profile.purpose != OpenMcExecutionPurpose::CandidateReference
            && artifacts.acceptance_json.is_some()
        {
            return Err(OpenMcInputError::AcceptanceRequiresCandidateReference);
        }
        let acceptance = match artifacts.acceptance_json {
            Some(bytes) => {
                let contract: OpenMcAcceptanceContract = parse_json("acceptance", bytes)?;
                contract.validate()?;
                if contract.case_id != case.case_id {
                    return Err(OpenMcInputError::InvalidAcceptance(format!(
                        "acceptance case_id {} does not match case {}",
                        contract.case_id, case.case_id
                    )));
                }
                if !contract.seeds.contains(&execution_profile.seed) {
                    return Err(OpenMcInputError::InvalidAcceptance(format!(
                        "profile seed {} is not in the acceptance seed set",
                        execution_profile.seed
                    )));
                }
                if execution_profile.batches < contract.min_batches {
                    return Err(OpenMcInputError::InvalidAcceptance(format!(
                        "profile batches {} below acceptance minimum {}",
                        execution_profile.batches, contract.min_batches
                    )));
                }
                Some(contract)
            }
            None => None,
        };

        let variance_reduction = match artifacts.variance_reduction_json {
            Some(bytes) => {
                let resolved: ResolvedWeightWindows = parse_json("variance_reduction", bytes)?;
                resolved.validate().map_err(|error| {
                    OpenMcInputError::InvalidVarianceReduction(error.to_string())
                })?;
                if let Some(vr_case) = &resolved.case_id
                    && vr_case != &case.case_id
                {
                    return Err(OpenMcInputError::InvalidVarianceReduction(format!(
                        "weight-windows case_id {vr_case} does not match case {}",
                        case.case_id
                    )));
                }
                Some(resolved)
            }
            None => None,
        };

        if case.material != material {
            return Err(OpenMcInputError::CaseMaterialMismatch);
        }
        if case.source != source {
            return Err(OpenMcInputError::CaseSourceMismatch);
        }

        // DICOM-derived material regions override the base material on their
        // voxels. Folded-response tallies encode the base material's atom
        // densities, so a region may only change the mass fraction of a
        // nuclide that a fluence-fold estimator explicitly covers —
        // collection rescales those components by the region/base
        // atom-density ratio (density × mass fraction). Every other nuclide
        // fraction must match the base material exactly; per-voxel mass then
        // differs only through the region density, which collection uses for
        // heating normalization and residual-component scaling.
        let covered_nuclides: Vec<&str> = component_profile
            .components
            .iter()
            .filter_map(|rule| match &rule.estimator {
                openbnct_transport::ComponentEstimator::NjoyPartialKermaFluenceFold {
                    nuclide,
                    ..
                } => Some(nuclide.as_str()),
                _ => None,
            })
            .collect();
        let material_assignment = match artifacts.material_assignment_json {
            Some(bytes) => {
                let assignment: MaterialAssignment = parse_json("material_assignment", bytes)?;
                assignment
                    .validate(&case.geometry)
                    .map_err(|error| OpenMcInputError::InvalidAssignment(error.to_string()))?;
                if assignment.case_id != case.case_id {
                    return Err(OpenMcInputError::InvalidAssignment(format!(
                        "assignment case_id {} does not match case {}",
                        assignment.case_id, case.case_id
                    )));
                }
                if assignment.base_material != material {
                    return Err(OpenMcInputError::InvalidAssignment(
                        "assignment base material differs from the bound material artifact".into(),
                    ));
                }
                for region in &assignment.regions {
                    // Region densities may differ: collection normalizes
                    // heating by per-voxel mass and rescales folded-response
                    // components by atom-density ratios. Temperature must
                    // still match — the cross sections are bound to the base
                    // material's temperature.
                    if region.material.temperature_k != material.temperature_k {
                        return Err(OpenMcInputError::InvalidAssignment(format!(
                            "region {} temperature differs from the base material",
                            region.name
                        )));
                    }
                    for nuclide in &region.material.nuclides {
                        let base_fraction = material
                            .nuclides
                            .iter()
                            .find(|n| n.name == nuclide.name)
                            .map(|n| n.mass_fraction);
                        let Some(base_fraction) = base_fraction else {
                            return Err(OpenMcInputError::InvalidAssignment(format!(
                                "region {} introduces nuclide {} absent from the base material; response tables cannot cover it",
                                region.name, nuclide.name
                            )));
                        };
                        if !covered_nuclides.contains(&nuclide.name.as_str())
                            && (nuclide.mass_fraction - base_fraction).abs() > 1.0e-12
                        {
                            return Err(OpenMcInputError::InvalidAssignment(format!(
                                "region {} changes uncovered nuclide {} ({} vs base {}); only response-covered nuclide fractions may differ",
                                region.name, nuclide.name, nuclide.mass_fraction, base_fraction
                            )));
                        }
                    }
                    for base in &material.nuclides {
                        if covered_nuclides.contains(&base.name.as_str()) {
                            continue;
                        }
                        let region_fraction = region
                            .material
                            .nuclides
                            .iter()
                            .find(|n| n.name == base.name)
                            .map(|n| n.mass_fraction)
                            .unwrap_or(0.0);
                        if (region_fraction - base.mass_fraction).abs() > 1.0e-12 {
                            return Err(OpenMcInputError::InvalidAssignment(format!(
                                "region {} drops or changes uncovered nuclide {} ({} vs base {}); residual response tables assume base fractions",
                                region.name, base.name, region_fraction, base.mass_fraction
                            )));
                        }
                    }
                }
                Some(assignment)
            }
            None => None,
        };

        let component_reference =
            content_reference(&component_profile.id, artifacts.component_profile_json);
        let material_reference = content_reference(&material.id, artifacts.material_json);
        let source_reference = content_reference(&source.id, artifacts.source_json);
        let response_reference = content_reference(&response_set.id, artifacts.response_set_json);
        let nuclear_data_reference =
            content_reference(&nuclear_data.id, artifacts.nuclear_data_manifest_json);
        let execution_reference =
            content_reference(&execution_profile.id, artifacts.execution_profile_json);

        require_binding(
            "response_set.component_profile",
            &response_set.component_profile,
            &component_reference,
        )?;
        require_binding(
            "response_set.material",
            &response_set.material,
            &material_reference,
        )?;
        require_binding(
            "response_set.nuclear_data_manifest",
            &response_set.nuclear_data_manifest,
            &nuclear_data_reference,
        )?;

        nuclear_data.validate_for_case(case)?;
        nuclear_data.verify_files(nuclear_data_root)?;
        let data_energy_range = nuclear_data.neutron_transport_energy_range_for_case(case)?;
        let response_energy_range = response_set.transport_energy_range_ev;
        if response_energy_range[0] > data_energy_range[0]
            || response_energy_range[1] < data_energy_range[1]
        {
            return Err(OpenMcInputError::ResponseEnergyRangeDoesNotCoverData {
                response_ev: response_energy_range,
                data_ev: data_energy_range,
            });
        }

        if source.particle != ParticleType::Neutron {
            return Err(OpenMcInputError::UnsupportedSourceParticle(source.particle));
        }
        // Every emitted source energy must sit inside the selected data
        // range; for a histogram that is the outer bin edges.
        let source_edges_ev: Vec<f64> = match &source.energy {
            EnergyDistribution::Monoenergetic { energy_ev } => vec![*energy_ev],
            EnergyDistribution::TabulatedHistogram {
                energy_boundaries_ev,
                ..
            } => energy_boundaries_ev
                .first()
                .into_iter()
                .chain(energy_boundaries_ev.last())
                .copied()
                .collect(),
        };
        for source_ev in source_edges_ev {
            if source_ev < data_energy_range[0] || source_ev >= data_energy_range[1] {
                return Err(OpenMcInputError::SourceEnergyOutsideDataRange {
                    source_ev,
                    data_ev: data_energy_range,
                });
            }
        }

        let scoring_mesh = scoring_mesh(case)?;
        validate_source_containment(&source, &scoring_mesh)?;
        let batch_count = u64::from(execution_profile.batches);
        let requested_histories = execution_profile
            .requested_histories
            .unwrap_or(case.requested_histories);
        if !requested_histories.is_multiple_of(batch_count) {
            return Err(OpenMcInputError::HistoriesNotDivisibleByBatches {
                histories: requested_histories,
                batches: execution_profile.batches,
            });
        }
        let particles_per_batch = requested_histories / batch_count;
        if particles_per_batch == 0 || particles_per_batch > i64::MAX as u64 {
            return Err(OpenMcInputError::InvalidParticlesPerBatch(
                particles_per_batch,
            ));
        }

        // Realize each acceptance region as a single-bin OpenMC mesh so the
        // region sum carries proper batch statistics; summing scoring-mesh
        // voxels would discard within-batch covariance.
        let density_g_cm3 = scoring_mesh.voxel_mass_g / scoring_mesh.voxel_volume_cm3;
        let mut roi_meshes: Vec<OpenMcRoiMesh> = Vec::new();
        if let Some(contract) = &acceptance {
            for (index, region) in contract.regions.iter().enumerate() {
                if !region
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    return Err(OpenMcInputError::InvalidAcceptance(format!(
                        "region name {} is not a lowercase tally token",
                        region.name
                    )));
                }
                let bounds = &region.bounds_cm;
                for (axis, lo, mesh_lo, mesh_hi) in [
                    (
                        "x",
                        bounds.x_cm,
                        scoring_mesh.lower_left_cm[0],
                        scoring_mesh.upper_right_cm[0],
                    ),
                    (
                        "y",
                        bounds.y_cm,
                        scoring_mesh.lower_left_cm[1],
                        scoring_mesh.upper_right_cm[1],
                    ),
                    (
                        "z",
                        bounds.z_cm,
                        scoring_mesh.lower_left_cm[2],
                        scoring_mesh.upper_right_cm[2],
                    ),
                ] {
                    if lo[0] < mesh_lo || lo[1] > mesh_hi {
                        return Err(OpenMcInputError::InvalidAcceptance(format!(
                            "region {} {axis} bounds exceed the scoring mesh",
                            region.name
                        )));
                    }
                }
                let volume_cm3 = (bounds.x_cm[1] - bounds.x_cm[0])
                    * (bounds.y_cm[1] - bounds.y_cm[0])
                    * (bounds.z_cm[1] - bounds.z_cm[0]);
                roi_meshes.push(OpenMcRoiMesh {
                    name: region.name.clone(),
                    mesh_id: ROI_MESH_ID_BASE + index as u32,
                    mesh_filter_id: ROI_MESH_FILTER_ID_BASE + index as u32,
                    dimensions: region.dimensions,
                    lower_left_cm: [bounds.x_cm[0], bounds.y_cm[0], bounds.z_cm[0]],
                    upper_right_cm: [bounds.x_cm[1], bounds.y_cm[1], bounds.z_cm[1]],
                    volume_cm3,
                    mass_g: volume_cm3 * density_g_cm3,
                });
            }
        }

        let geometry_xml = geometry_xml(case, &scoring_mesh, material_assignment.as_ref())?;
        let materials_xml = materials_xml(&material, material_assignment.as_ref())?;
        let settings_xml = settings_xml(
            &source,
            &execution_profile,
            particles_per_batch,
            execution_profile.batches,
            variance_reduction.as_ref(),
        )?;
        let tallies_xml = tallies_xml(
            &response_set,
            &execution_profile,
            &roi_meshes,
            &scoring_mesh,
        )?;

        let mut files = vec![
            generated_file("geometry.xml", XML_MEDIA_TYPE, geometry_xml),
            generated_file("materials.xml", XML_MEDIA_TYPE, materials_xml),
            generated_file("settings.xml", XML_MEDIA_TYPE, settings_xml),
            generated_file("tallies.xml", XML_MEDIA_TYPE, tallies_xml),
        ];
        // The component profile rides with the deck so collection can map
        // components to their covered nuclides for region-density correction.
        if material_assignment.is_some() {
            files.push(generated_file(
                "openbnct-component-profile.json",
                JSON_MEDIA_TYPE,
                artifacts.component_profile_json.to_vec(),
            ));
        }
        if let Some(bytes) = artifacts.acceptance_json {
            files.push(generated_file(
                "openbnct-acceptance-contract.json",
                JSON_MEDIA_TYPE,
                bytes.to_vec(),
            ));
        }
        if let Some(bytes) = artifacts.material_assignment_json {
            files.push(generated_file(
                "openbnct-material-assignment.json",
                JSON_MEDIA_TYPE,
                bytes.to_vec(),
            ));
        }
        if let Some(bytes) = artifacts.variance_reduction_json {
            files.push(generated_file(
                crate::variance_reduction::RESOLVED_WW_FILE,
                JSON_MEDIA_TYPE,
                bytes.to_vec(),
            ));
        }
        let xml_artifacts = files
            .iter()
            .map(|file| OpenMcInputManifestArtifact {
                path: file.relative_path.clone(),
                sha256: file.sha256.clone(),
                media_type: file.media_type.clone(),
            })
            .collect();
        let independent_response_review = response_set
            .independent_review
            .clone()
            .expect("folding validation requires independent review");
        let manifest = OpenMcInputManifest {
            schema_version: if acceptance.is_some()
                || material_assignment.is_some()
                || variance_reduction.is_some()
            {
                INPUT_MANIFEST_SCHEMA_V2
            } else {
                INPUT_MANIFEST_SCHEMA
            }
            .into(),
            case_id: case.case_id.clone(),
            backend_id: "openmc".into(),
            openmc_version: TARGET_OPENMC_VERSION.into(),
            openmc_source_commit: TARGET_OPENMC_SOURCE_COMMIT.into(),
            bindings: OpenMcInputBindings {
                component_profile: component_reference,
                material: material_reference,
                source: source_reference,
                response_set: response_reference,
                nuclear_data_manifest: nuclear_data_reference,
                response_generation_method: response_set.generation_method.clone(),
                independent_response_review,
                execution_profile: execution_reference,
                acceptance: acceptance.as_ref().map(|contract| {
                    content_reference(&contract.id, artifacts.acceptance_json.unwrap())
                }),
                material_assignment: material_assignment.as_ref().map(|_| {
                    content_reference(
                        MATERIAL_ASSIGNMENT_SCHEMA,
                        artifacts.material_assignment_json.unwrap(),
                    )
                }),
                variance_reduction: variance_reduction.as_ref().map(|resolved| {
                    content_reference(&resolved.id, artifacts.variance_reduction_json.unwrap())
                }),
            },
            execution: OpenMcRunControls {
                purpose: execution_profile.purpose,
                requested_histories,
                batches: execution_profile.batches,
                particles_per_batch,
                seed: execution_profile.seed,
                stride: execution_profile.stride,
            },
            scoring_mesh,
            tallies: tally_contracts(&roi_meshes),
            rois: roi_meshes,
            xml_artifacts,
        };
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(OpenMcInputError::ManifestSerialization)?;
        manifest_bytes.push(b'\n');
        files.push(generated_file(
            "openbnct-input-manifest.json",
            JSON_MEDIA_TYPE,
            manifest_bytes,
        ));

        Ok(Self { manifest, files })
    }

    #[must_use]
    pub fn file(&self, relative_path: &str) -> Option<&GeneratedOpenMcFile> {
        self.files
            .iter()
            .find(|file| file.relative_path == relative_path)
    }

    /// Write the generated deck to a new directory without overwriting any
    /// existing path. A failed write leaves the partial directory for review.
    pub fn write_new(&self, output: &Path) -> Result<(), OpenMcInputError> {
        use std::io::Write;

        fs::create_dir(output).map_err(|source| OpenMcInputError::Io {
            path: output.to_path_buf(),
            source,
        })?;
        for file in &self.files {
            let relative = Path::new(&file.relative_path);
            if file.relative_path.is_empty()
                || file.relative_path.contains('\\')
                || relative.is_absolute()
                || !relative
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
            {
                return Err(OpenMcInputError::UnsafeRelativePath(
                    file.relative_path.clone(),
                ));
            }
            let path = output.join(relative);
            if let Some(parent) = path.parent()
                && parent != output
            {
                fs::create_dir(parent).map_err(|source| OpenMcInputError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let mut stream = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .map_err(|source| OpenMcInputError::Io {
                    path: path.clone(),
                    source,
                })?;
            stream
                .write_all(&file.bytes)
                .and_then(|()| stream.sync_all())
                .map_err(|source| OpenMcInputError::Io { path, source })?;
        }
        Ok(())
    }
}

fn validate_energy_grid(
    particle: &'static str,
    grid: &[f64],
    required_boundaries: &[f64],
) -> Result<(), OpenMcProfileError> {
    if grid.len() < 2
        || grid.iter().any(|value| !value.is_finite() || *value < 0.0)
        || grid.windows(2).any(|pair| pair[0] >= pair[1])
        || grid[0] != 0.0
        || grid[grid.len() - 1] < 20.0e6
    {
        return Err(OpenMcProfileError::InvalidDiagnosticEnergyGrid(particle));
    }
    for boundary in required_boundaries {
        if !grid.contains(boundary) {
            return Err(OpenMcProfileError::MissingDiagnosticBoundary {
                particle,
                boundary_ev: *boundary,
            });
        }
    }
    Ok(())
}

fn parse_json<T: DeserializeOwned>(
    artifact: &'static str,
    bytes: &[u8],
) -> Result<T, OpenMcInputError> {
    serde_json::from_slice(bytes)
        .map_err(|source| OpenMcInputError::InvalidJson { artifact, source })
}

fn content_reference(id: &str, bytes: &[u8]) -> ContentReference {
    ContentReference {
        id: id.into(),
        sha256: sha256_hex(bytes),
    }
}

fn require_binding(
    label: &'static str,
    declared: &ContentReference,
    observed: &ContentReference,
) -> Result<(), OpenMcInputError> {
    if declared != observed {
        return Err(OpenMcInputError::ContentBindingMismatch {
            label,
            declared: declared.clone(),
            observed: observed.clone(),
        });
    }
    Ok(())
}

fn scoring_mesh(case: &TransportCase) -> Result<OpenMcScoringMesh, OpenMcInputError> {
    require_identity_direction(&case.geometry)?;
    let mut lower_left_cm = [0.0; 3];
    let mut upper_right_cm = [0.0; 3];
    for axis in 0..3 {
        lower_left_cm[axis] =
            (case.geometry.origin_mm[axis] - 0.5 * case.geometry.spacing_mm[axis]) / 10.0;
        upper_right_cm[axis] = lower_left_cm[axis]
            + f64::from(case.geometry.shape[axis]) * case.geometry.spacing_mm[axis] / 10.0;
    }
    let voxel_volume_cm3 = case
        .geometry
        .spacing_mm
        .iter()
        .map(|spacing| spacing / 10.0)
        .product::<f64>();
    let voxel_mass_g = voxel_volume_cm3 * case.material.density_g_cm3;
    let voxel_count = case
        .geometry
        .shape
        .iter()
        .map(|extent| f64::from(*extent))
        .product::<f64>();
    Ok(OpenMcScoringMesh {
        mesh_id: MESH_ID,
        dimensions: case.geometry.shape,
        lower_left_cm,
        upper_right_cm,
        voxel_volume_cm3,
        voxel_mass_g,
        cell_volume_cm3: voxel_volume_cm3 * voxel_count,
        cell_mass_g: voxel_mass_g * voxel_count,
    })
}

fn require_identity_direction(geometry: &GridGeometry) -> Result<(), OpenMcInputError> {
    if geometry
        .direction
        .iter()
        .zip(IDENTITY_DIRECTION)
        .any(|(observed, expected)| (*observed - expected).abs() > DIRECTION_TOLERANCE)
    {
        return Err(OpenMcInputError::UnsupportedGeometryDirection(
            geometry.direction,
        ));
    }
    Ok(())
}

fn validate_source_containment(
    source: &FixedSourceDefinition,
    mesh: &OpenMcScoringMesh,
) -> Result<(), OpenMcInputError> {
    let contained = match &source.space {
        SourceSpatialDistribution::UniformDisk {
            axis,
            offset_cm,
            center_uv_cm,
            radius_cm,
        } => {
            let (u_axis, v_axis) = axis.in_plane_axes();
            let axis_index = axis.index();
            center_uv_cm[0] - *radius_cm > mesh.lower_left_cm[u_axis]
                && center_uv_cm[0] + *radius_cm < mesh.upper_right_cm[u_axis]
                && center_uv_cm[1] - *radius_cm > mesh.lower_left_cm[v_axis]
                && center_uv_cm[1] + *radius_cm < mesh.upper_right_cm[v_axis]
                && *offset_cm > mesh.lower_left_cm[axis_index]
                && *offset_cm < mesh.upper_right_cm[axis_index]
        }
        space => {
            let Some((plane_axis, offset_cm, u_range_cm, v_range_cm)) = space.plane_parts() else {
                return Err(OpenMcInputError::UnsupportedSourceSpace);
            };
            let (u_axis, v_axis) = plane_axis.in_plane_axes();
            u_range_cm[0] > mesh.lower_left_cm[u_axis]
                && u_range_cm[1] < mesh.upper_right_cm[u_axis]
                && v_range_cm[0] > mesh.lower_left_cm[v_axis]
                && v_range_cm[1] < mesh.upper_right_cm[v_axis]
                && offset_cm > mesh.lower_left_cm[plane_axis.index()]
                && offset_cm < mesh.upper_right_cm[plane_axis.index()]
        }
    };
    if !contained {
        return Err(OpenMcInputError::SourceOutsideGeometry);
    }
    Ok(())
}

/// Region-box surface IDs start at `REGION_SURFACE_BASE + 6 * region_index`;
/// cells and materials follow the same ordering (`2 + index`).
const REGION_SURFACE_BASE: u32 = 101;

/// Each region's material gets an OpenMC material ID — identical region
/// materials share one material element.
fn region_material_ids(assignment: Option<&MaterialAssignment>) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut materials: Vec<&MaterialDefinition> = Vec::new();
    if let Some(assignment) = assignment {
        for region in &assignment.regions {
            let index = materials
                .iter()
                .position(|existing| **existing == region.material)
                .unwrap_or_else(|| {
                    materials.push(&region.material);
                    materials.len() - 1
                });
            ids.push(2 + index as u32);
        }
    }
    ids
}

/// Lattice mode assigns materials per voxel element; element universes and
/// their cells use IDs `LATTICE_UNIVERSE_BASE + material_index` and the
/// lattice itself carries `LATTICE_ID`, all outside the CSG ID ranges.
const LATTICE_ID: u32 = 100;
const LATTICE_UNIVERSE_BASE: u32 = 1000;

fn geometry_xml(
    case: &TransportCase,
    mesh: &OpenMcScoringMesh,
    assignment: Option<&MaterialAssignment>,
) -> Result<Vec<u8>, OpenMcInputError> {
    xml_document("geometry", |writer| {
        // Lattice mode: any voxel-set region forces the whole grid into a
        // rectilinear lattice so every voxel carries its assigned material
        // exactly; box-only assignments use exact CSG cells instead.
        let lattice_mode = assignment.is_some_and(|assignment| {
            assignment
                .regions
                .iter()
                .any(|region| !region.is_axis_aligned_box())
        });

        // Base cell: outer box with every region box subtracted (CSG mode),
        // or the plain outer box filled with the material lattice.
        let mut base_region = "1 -2 3 -4 5 -6".to_owned();
        let mut cells = Vec::new();
        if let Some(assignment) = assignment
            && !lattice_mode
        {
            let material_ids = region_material_ids(Some(assignment));
            for (index, region) in assignment.regions.iter().enumerate() {
                let first_surface = REGION_SURFACE_BASE + 6 * index as u32;
                let halfspaces = format!(
                    "{first} -{second} {third} -{fourth} {fifth} -{sixth}",
                    first = first_surface,
                    second = first_surface + 1,
                    third = first_surface + 2,
                    fourth = first_surface + 3,
                    fifth = first_surface + 4,
                    sixth = first_surface + 5,
                );
                base_region.push_str(&format!(" ~({halfspaces})"));
                cells.push((2 + index as u32, region, material_ids[index], halfspaces));
            }
        }

        let mut cell = BytesStart::new("cell");
        cell.push_attribute(("id", "1"));
        cell.push_attribute(("name", case.case_id.as_str()));
        if lattice_mode {
            cell.push_attribute(("fill", LATTICE_ID.to_string().as_str()));
        } else {
            cell.push_attribute(("material", "1"));
        }
        cell.push_attribute(("region", base_region.as_str()));
        cell.push_attribute(("universe", "1"));
        writer.write_event(Event::Empty(cell))?;
        for (cell_id, region, material_id, halfspaces) in &cells {
            let mut element = BytesStart::new("cell");
            element.push_attribute(("id", cell_id.to_string().as_str()));
            element.push_attribute(("name", region.name.as_str()));
            element.push_attribute(("material", material_id.to_string().as_str()));
            element.push_attribute(("region", halfspaces.as_str()));
            element.push_attribute(("universe", "1"));
            writer.write_event(Event::Empty(element))?;
        }
        if lattice_mode && let Some(assignment) = assignment {
            // One universe per distinct material: base material (id 1) fills
            // universe LATTICE_UNIVERSE_BASE, region material id m fills
            // BASE + m - 1. Each element cell fills its lattice element
            // entirely (no region attribute).
            let material_ids = region_material_ids(Some(assignment));
            let voxel_universe = |region_index: Option<usize>| -> u32 {
                match region_index {
                    None => LATTICE_UNIVERSE_BASE,
                    Some(index) => LATTICE_UNIVERSE_BASE + material_ids[index] - 1,
                }
            };
            let [nx, ny, nz] = case.geometry.shape.map(|d| d as usize);
            let mut owner = vec![None::<usize>; nx * ny * nz];
            for (index, region) in assignment.regions.iter().enumerate() {
                region.for_each_voxel(|voxel| {
                    owner[voxel[0] as usize
                        + nx * voxel[1] as usize
                        + nx * ny * voxel[2] as usize] = Some(index);
                });
            }
            for (offset, material_id) in (1..=region_material_count(Some(assignment))).enumerate() {
                let universe = (LATTICE_UNIVERSE_BASE + offset as u32).to_string();
                let mut element = BytesStart::new("cell");
                element.push_attribute(("id", universe.as_str()));
                element.push_attribute(("material", material_id.to_string().as_str()));
                element.push_attribute(("universe", universe.as_str()));
                writer.write_event(Event::Empty(element))?;
            }
            // OpenMC's XML universes list runs x-fastest with the y index
            // reversed (src/lattice.cpp): word i + nx*iy + nx*ny*iz fills
            // element (i, ny-1-iy, iz).
            let mut words = String::new();
            for k in 0..nz {
                for j in (0..ny).rev() {
                    for i in 0..nx {
                        words
                            .push_str(&voxel_universe(owner[i + nx * j + nx * ny * k]).to_string());
                        words.push(' ');
                    }
                    words.push('\n');
                }
            }
            let mut lattice = BytesStart::new("lattice");
            lattice.push_attribute(("id", LATTICE_ID.to_string().as_str()));
            lattice.push_attribute(("name", "material_map"));
            writer.write_event(Event::Start(lattice))?;
            for (tag, values) in [
                (
                    "dimension",
                    format!(
                        "{} {} {}",
                        case.geometry.shape[0], case.geometry.shape[1], case.geometry.shape[2]
                    ),
                ),
                (
                    "lower_left",
                    (0..3)
                        .map(|axis| {
                            format_float(
                                (case.geometry.origin_mm[axis]
                                    - 0.5 * case.geometry.spacing_mm[axis])
                                    / 10.0,
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                (
                    "pitch",
                    case.geometry
                        .spacing_mm
                        .iter()
                        .map(|spacing| format_float(spacing / 10.0))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                ("universes", words.trim_end().to_owned()),
            ] {
                writer.write_event(Event::Start(BytesStart::new(tag)))?;
                writer.write_event(Event::Text(BytesText::new(&values)))?;
                writer.write_event(Event::End(BytesEnd::new(tag)))?;
            }
            writer.write_event(Event::End(BytesEnd::new("lattice")))?;
        }

        let mut surfaces = vec![
            (1_u32, "x-plane", mesh.lower_left_cm[0], true),
            (2, "x-plane", mesh.upper_right_cm[0], true),
            (3, "y-plane", mesh.lower_left_cm[1], true),
            (4, "y-plane", mesh.upper_right_cm[1], true),
            (5, "z-plane", mesh.lower_left_cm[2], true),
            (6, "z-plane", mesh.upper_right_cm[2], true),
        ];
        if let Some(assignment) = assignment
            && !lattice_mode
        {
            for (index, region) in assignment.regions.iter().enumerate() {
                let Some((lower_mm, upper_mm)) = region.world_bounds_mm(&case.geometry) else {
                    continue;
                };
                let first = REGION_SURFACE_BASE + 6 * index as u32;
                surfaces.extend([
                    (first, "x-plane", lower_mm[0] / 10.0, false),
                    (first + 1, "x-plane", upper_mm[0] / 10.0, false),
                    (first + 2, "y-plane", lower_mm[1] / 10.0, false),
                    (first + 3, "y-plane", upper_mm[1] / 10.0, false),
                    (first + 4, "z-plane", lower_mm[2] / 10.0, false),
                    (first + 5, "z-plane", upper_mm[2] / 10.0, false),
                ]);
            }
        }
        for (id, kind, coefficient, boundary) in surfaces {
            let id = id.to_string();
            let coefficient = format_float(coefficient);
            let mut surface = BytesStart::new("surface");
            surface.push_attribute(("id", id.as_str()));
            surface.push_attribute(("type", kind));
            // Vacuum boundary belongs on the outer envelope only; region
            // surfaces are interior interfaces.
            if boundary {
                surface.push_attribute(("boundary", "vacuum"));
            }
            surface.push_attribute(("coeffs", coefficient.as_str()));
            writer.write_event(Event::Empty(surface))?;
        }
        Ok(())
    })
}

/// Number of distinct material elements in a deck: the base material plus
/// each distinct region material.
fn region_material_count(assignment: Option<&MaterialAssignment>) -> u32 {
    1 + region_material_ids(assignment)
        .iter()
        .max()
        .map(|max| max - 1)
        .unwrap_or(0)
}

fn materials_xml(
    material: &MaterialDefinition,
    assignment: Option<&MaterialAssignment>,
) -> Result<Vec<u8>, OpenMcInputError> {
    xml_document("materials", |writer| {
        let mut emitted: Vec<&MaterialDefinition> = Vec::new();
        let mut queue: Vec<(u32, &MaterialDefinition)> = vec![(1, material)];
        if let Some(assignment) = assignment {
            for region in &assignment.regions {
                if !emitted.iter().any(|existing| **existing == region.material)
                    && region.material != *material
                {
                    emitted.push(&region.material);
                }
            }
            for (index, material) in emitted.iter().enumerate() {
                queue.push((2 + index as u32, material));
            }
        }
        for (id, material) in queue {
            let temperature = format_float(material.temperature_k);
            let mut material_element = BytesStart::new("material");
            material_element.push_attribute(("id", id.to_string().as_str()));
            material_element.push_attribute(("name", material.id.as_str()));
            material_element.push_attribute(("temperature", temperature.as_str()));
            writer.write_event(Event::Start(material_element))?;

            let density = format_float(material.density_g_cm3);
            let mut density_element = BytesStart::new("density");
            density_element.push_attribute(("value", density.as_str()));
            density_element.push_attribute(("units", "g/cm3"));
            writer.write_event(Event::Empty(density_element))?;

            for nuclide in &material.nuclides {
                let fraction = format_float(nuclide.mass_fraction);
                let mut element = BytesStart::new("nuclide");
                element.push_attribute(("name", nuclide.name.as_str()));
                element.push_attribute(("wo", fraction.as_str()));
                writer.write_event(Event::Empty(element))?;
            }
            writer.write_event(Event::End(BytesEnd::new("material")))?;
        }
        Ok(())
    })
}

fn settings_xml(
    source: &FixedSourceDefinition,
    profile: &OpenMcExecutionProfile,
    particles_per_batch: u64,
    batches: u32,
    variance_reduction: Option<&ResolvedWeightWindows>,
) -> Result<Vec<u8>, OpenMcInputError> {
    xml_document("settings", |writer| {
        text_element(writer, "run_mode", "fixed source")?;
        text_element(writer, "particles", &particles_per_batch.to_string())?;
        text_element(writer, "batches", &batches.to_string())?;

        let mut source_element = BytesStart::new("source");
        source_element.push_attribute(("type", "independent"));
        source_element.push_attribute(("strength", "1.0"));
        source_element.push_attribute(("particle", "neutron"));
        writer.write_event(Event::Start(source_element))?;

        match &source.space {
            SourceSpatialDistribution::UniformCartesianPlane { .. }
            | SourceSpatialDistribution::UniformAxisPlane { .. } => {
                // Any rectangular planar spatial distribution emits as an
                // axis-aligned box collapsed along the plane axis (zero
                // thickness -> plane source).
                let Some((plane_axis, offset_cm, u_range_cm, v_range_cm)) =
                    source.space.plane_parts()
                else {
                    unreachable!("rectangular source variants always have plane parts")
                };
                let (u_axis, v_axis) = plane_axis.in_plane_axes();
                let mut lower = [0.0; 3];
                let mut upper = [0.0; 3];
                lower[plane_axis.index()] = offset_cm;
                upper[plane_axis.index()] = offset_cm;
                lower[u_axis] = u_range_cm[0];
                upper[u_axis] = u_range_cm[1];
                lower[v_axis] = v_range_cm[0];
                upper[v_axis] = v_range_cm[1];
                let mut space = BytesStart::new("space");
                space.push_attribute(("type", "box"));
                writer.write_event(Event::Start(space))?;
                text_element(
                    writer,
                    "parameters",
                    &format_numbers(&[lower[0], lower[1], lower[2], upper[0], upper[1], upper[2]]),
                )?;
                writer.write_event(Event::End(BytesEnd::new("space")))?;
            }
            SourceSpatialDistribution::UniformDisk {
                axis,
                offset_cm,
                center_uv_cm,
                radius_cm,
            } => {
                // A disk emits as a cylindrical distribution on the port
                // plane: r ~ powerlaw(n=1) samples area uniformly,
                // phi ~ uniform, z degenerate at the plane.
                let (u_axis, v_axis) = axis.in_plane_axes();
                let mut origin = [0.0; 3];
                origin[axis.index()] = *offset_cm;
                origin[u_axis] = center_uv_cm[0];
                origin[v_axis] = center_uv_cm[1];
                let mut z_dir = [0.0; 3];
                z_dir[axis.index()] = 1.0;
                let mut r_dir = [0.0; 3];
                r_dir[u_axis] = 1.0;
                let mut space = BytesStart::new("space");
                space.push_attribute(("type", "cylindrical"));
                writer.write_event(Event::Start(space))?;
                text_element(writer, "origin", &format_numbers(&origin))?;
                text_element(writer, "z_dir", &format_numbers(&z_dir))?;
                text_element(writer, "r_dir", &format_numbers(&r_dir))?;
                univariate_element(writer, "r", "powerlaw", &[0.0, *radius_cm, 1.0])?;
                univariate_element(writer, "phi", "uniform", &[0.0, std::f64::consts::TAU])?;
                univariate_element(writer, "z", "discrete", &[0.0, 1.0])?;
                writer.write_event(Event::End(BytesEnd::new("space")))?;
            }
        }

        match &source.angle {
            AngularDistribution::Monodirectional { unit_vector } => {
                let mut angle = BytesStart::new("angle");
                angle.push_attribute(("type", "monodirectional"));
                writer.write_event(Event::Start(angle))?;
                text_element(writer, "reference_uvw", &format_numbers(unit_vector))?;
                writer.write_event(Event::End(BytesEnd::new("angle")))?;
            }
            AngularDistribution::IsotropicCone {
                axis_unit_vector,
                half_angle_rad,
            } => {
                // Uniform-in-solid-angle cone: mu ~ U(cos θ0, 1) about
                // the cone axis, phi ~ U(0, 2π).
                let mut angle = BytesStart::new("angle");
                angle.push_attribute(("type", "mu-phi"));
                writer.write_event(Event::Start(angle))?;
                text_element(writer, "reference_uvw", &format_numbers(axis_unit_vector))?;
                univariate_element(writer, "mu", "uniform", &[half_angle_rad.cos(), 1.0])?;
                univariate_element(writer, "phi", "uniform", &[0.0, std::f64::consts::TAU])?;
                writer.write_event(Event::End(BytesEnd::new("angle")))?;
            }
        }

        match &source.energy {
            EnergyDistribution::Monoenergetic { energy_ev } => {
                let mut energy = BytesStart::new("energy");
                energy.push_attribute(("type", "discrete"));
                writer.write_event(Event::Start(energy))?;
                text_element(
                    writer,
                    "parameters",
                    &format!("{} 1.0", format_float(*energy_ev)),
                )?;
                writer.write_event(Event::End(BytesEnd::new("energy")))?;
            }
            EnergyDistribution::TabulatedHistogram {
                energy_boundaries_ev,
                bin_weights,
            } => {
                // OpenMC tabular parameters are the x array followed by
                // the p array. For histogram interpolation p is a density,
                // so convert bin weights to weights-per-eV; the trailing p
                // entry is ignored, so pad it with zero.
                let mut parameters = String::new();
                for value in energy_boundaries_ev {
                    parameters.push_str(&format_float(*value));
                    parameters.push(' ');
                }
                for (index, weight) in bin_weights.iter().enumerate() {
                    let width = energy_boundaries_ev[index + 1] - energy_boundaries_ev[index];
                    parameters.push_str(&format_float(*weight / width));
                    parameters.push(' ');
                }
                parameters.push('0');
                let mut energy = BytesStart::new("energy");
                energy.push_attribute(("type", "tabular"));
                writer.write_event(Event::Start(energy))?;
                text_element(writer, "interpolation", "histogram")?;
                text_element(writer, "parameters", &parameters)?;
                writer.write_event(Event::End(BytesEnd::new("energy")))?;
            }
        }
        writer.write_event(Event::End(BytesEnd::new("source")))?;

        writer.write_event(Event::Start(BytesStart::new("output")))?;
        text_element(writer, "summary", bool_text(profile.write_summary))?;
        text_element(writer, "tallies", bool_text(profile.write_ascii_tallies))?;
        writer.write_event(Event::End(BytesEnd::new("output")))?;

        writer.write_event(Event::Start(BytesStart::new("state_point")))?;
        text_element(writer, "batches", &batches.to_string())?;
        writer.write_event(Event::End(BytesEnd::new("state_point")))?;
        writer.write_event(Event::Start(BytesStart::new("source_point")))?;
        text_element(writer, "write", bool_text(profile.write_sourcepoint))?;
        writer.write_event(Event::End(BytesEnd::new("source_point")))?;

        text_element(
            writer,
            "confidence_intervals",
            bool_text(profile.confidence_intervals),
        )?;
        text_element(writer, "electron_treatment", "led")?;
        text_element(
            writer,
            "atomic_relaxation",
            bool_text(profile.atomic_relaxation),
        )?;
        text_element(writer, "energy_mode", "continuous-energy")?;
        text_element(
            writer,
            "photon_transport",
            bool_text(profile.photon_transport),
        )?;
        text_element(writer, "ptables", bool_text(profile.probability_tables))?;
        text_element(writer, "seed", &profile.seed.to_string())?;
        text_element(writer, "stride", &profile.stride.to_string())?;
        text_element(
            writer,
            "survival_biasing",
            bool_text(profile.survival_biasing),
        )?;
        text_element(writer, "temperature_method", "nearest")?;
        text_element(
            writer,
            "temperature_multipole",
            bool_text(profile.temperature_multipole),
        )?;
        text_element(
            writer,
            "temperature_tolerance",
            &format_float(profile.temperature_tolerance_k),
        )?;
        text_element(writer, "event_based", bool_text(profile.event_based))?;

        // Weight windows: OpenMC reads <mesh> elements from settings.xml
        // before <weight_windows>, so both the window meshes and the
        // window definitions live here rather than in tallies.xml. A
        // mesh referenced by several windows is emitted once.
        if let Some(vr) = variance_reduction {
            let mut mesh_ids: Vec<u32> = Vec::with_capacity(vr.windows.len());
            for (index, window) in vr.windows.iter().enumerate() {
                let mesh_id = vr
                    .windows
                    .iter()
                    .take(index)
                    .position(|prior| prior.mesh == window.mesh)
                    .map(|prior| WW_MESH_ID_BASE + prior as u32)
                    .unwrap_or(WW_MESH_ID_BASE + index as u32);
                mesh_ids.push(mesh_id);
                if mesh_ids[..index].contains(&mesh_id) {
                    continue;
                }
                let mut mesh_element = BytesStart::new("mesh");
                mesh_element.push_attribute(("id", mesh_id.to_string().as_str()));
                mesh_element.push_attribute(("type", "regular"));
                writer.write_event(Event::Start(mesh_element))?;
                text_element(
                    writer,
                    "dimension",
                    &format_integers(&window.mesh.dimensions),
                )?;
                text_element(
                    writer,
                    "lower_left",
                    &format_numbers(&window.mesh.lower_left_cm),
                )?;
                text_element(
                    writer,
                    "upper_right",
                    &format_numbers(&window.mesh.upper_right_cm),
                )?;
                writer.write_event(Event::End(BytesEnd::new("mesh")))?;
            }
            for (index, window) in vr.windows.iter().enumerate() {
                writer.write_event(Event::Start(BytesStart::new("weight_windows")))?;
                text_element(writer, "id", &index.to_string())?;
                text_element(writer, "mesh", &mesh_ids[index].to_string())?;
                let particle = match window.particle {
                    ParticleType::Neutron => "neutron",
                    ParticleType::Photon => "photon",
                };
                text_element(writer, "particle_type", particle)?;
                if let Some(bounds) = &window.energy_bounds_ev {
                    text_element(writer, "energy_bounds", &format_numbers(bounds))?;
                }
                text_element(
                    writer,
                    "lower_ww_bounds",
                    &format_numbers(&window.lower_bounds),
                )?;
                text_element(
                    writer,
                    "upper_ww_bounds",
                    &format_numbers(&window.upper_bounds),
                )?;
                text_element(
                    writer,
                    "survival_ratio",
                    &format_float(window.parameters.survival_ratio),
                )?;
                text_element(
                    writer,
                    "max_split",
                    &window.parameters.max_split.to_string(),
                )?;
                text_element(
                    writer,
                    "weight_cutoff",
                    &format_float(window.parameters.weight_cutoff),
                )?;
                if let Some(ratio) = window.parameters.max_lower_bound_ratio {
                    text_element(writer, "max_lower_bound_ratio", &format_float(ratio))?;
                }
                writer.write_event(Event::End(BytesEnd::new("weight_windows")))?;
            }
        }
        Ok(())
    })
}

fn tallies_xml(
    response: &NeutronResponseSet,
    profile: &OpenMcExecutionProfile,
    rois: &[OpenMcRoiMesh],
    mesh: &OpenMcScoringMesh,
) -> Result<Vec<u8>, OpenMcInputError> {
    xml_document("tallies", |writer| {
        let mut mesh_element = BytesStart::new("mesh");
        mesh_element.push_attribute(("id", "1"));
        writer.write_event(Event::Start(mesh_element))?;
        text_element(writer, "dimension", &format_integers(&mesh.dimensions))?;
        text_element(writer, "lower_left", &format_numbers(&mesh.lower_left_cm))?;
        text_element(writer, "upper_right", &format_numbers(&mesh.upper_right_cm))?;
        writer.write_event(Event::End(BytesEnd::new("mesh")))?;

        for roi in rois {
            let mesh_id = roi.mesh_id.to_string();
            let mut mesh_element = BytesStart::new("mesh");
            mesh_element.push_attribute(("id", mesh_id.as_str()));
            writer.write_event(Event::Start(mesh_element))?;
            text_element(writer, "dimension", &format_integers(&roi.dimensions))?;
            text_element(writer, "lower_left", &format_numbers(&roi.lower_left_cm))?;
            text_element(writer, "upper_right", &format_numbers(&roi.upper_right_cm))?;
            writer.write_event(Event::End(BytesEnd::new("mesh")))?;
        }

        filter_with_bins(writer, MESH_FILTER_ID, "mesh", "1")?;
        for roi in rois {
            filter_with_bins(writer, roi.mesh_filter_id, "mesh", &roi.mesh_id.to_string())?;
        }
        filter_with_bins(writer, NEUTRON_FILTER_ID, "particle", "neutron")?;
        filter_with_bins(writer, PHOTON_FILTER_ID, "particle", "photon")?;
        energy_function_filter(
            writer,
            BORON_RESPONSE_FILTER_ID,
            &response.energy_ev,
            &response.boron_gy_cm2,
        )?;
        energy_function_filter(
            writer,
            NITROGEN_RESPONSE_FILTER_ID,
            &response.energy_ev,
            &response.nitrogen_gy_cm2,
        )?;
        energy_function_filter(
            writer,
            HYDROGEN_RESPONSE_FILTER_ID,
            &response.energy_ev,
            &response.hydrogen_gy_cm2,
        )?;
        filter_with_bins(
            writer,
            NEUTRON_ENERGY_FILTER_ID,
            "energy",
            &format_numbers(&profile.neutron_diagnostic_energy_grid_ev),
        )?;
        filter_with_bins(
            writer,
            PHOTON_ENERGY_FILTER_ID,
            "energy",
            &format_numbers(&profile.photon_diagnostic_energy_grid_ev),
        )?;
        filter_with_bins(writer, SURFACE_FILTER_ID, "surface", "1 2 3 4 5 6")?;

        tally(
            writer,
            BORON_TALLY_ID,
            "openbnct.component.boron.response",
            &[MESH_FILTER_ID, NEUTRON_FILTER_ID, BORON_RESPONSE_FILTER_ID],
            &[],
            &["flux"],
            "tracklength",
        )?;
        tally(
            writer,
            NITROGEN_TALLY_ID,
            "openbnct.component.nitrogen.response",
            &[
                MESH_FILTER_ID,
                NEUTRON_FILTER_ID,
                NITROGEN_RESPONSE_FILTER_ID,
            ],
            &[],
            &["flux"],
            "tracklength",
        )?;
        tally(
            writer,
            HYDROGEN_TALLY_ID,
            "openbnct.component.hydrogen.response",
            &[
                MESH_FILTER_ID,
                NEUTRON_FILTER_ID,
                HYDROGEN_RESPONSE_FILTER_ID,
            ],
            &[],
            &["flux"],
            "tracklength",
        )?;
        tally(
            writer,
            NEUTRON_HEATING_TALLY_ID,
            "openbnct.audit.neutron_heating",
            &[MESH_FILTER_ID, NEUTRON_FILTER_ID],
            &[],
            &["heating"],
            "tracklength",
        )?;
        tally(
            writer,
            PHOTON_HEATING_TALLY_ID,
            "openbnct.component.photon.heating",
            &[MESH_FILTER_ID, PHOTON_FILTER_ID],
            &[],
            &["heating"],
            "collision",
        )?;
        tally(
            writer,
            COUPLED_HEATING_TALLY_ID,
            "openbnct.physical_total.coupled_heating",
            &[MESH_FILTER_ID],
            &[],
            &["heating"],
            "collision",
        )?;
        tally(
            writer,
            BORON_REACTION_TALLY_ID,
            "openbnct.audit.b10_mt107",
            &[MESH_FILTER_ID, NEUTRON_FILTER_ID],
            &["B10"],
            &["(n,a)"],
            "tracklength",
        )?;
        tally(
            writer,
            NITROGEN_REACTION_TALLY_ID,
            "openbnct.audit.n14_mt103",
            &[MESH_FILTER_ID, NEUTRON_FILTER_ID],
            &["N14"],
            &["(n,p)"],
            "tracklength",
        )?;
        tally(
            writer,
            NEUTRON_FLUX_TALLY_ID,
            "openbnct.diagnostic.neutron_fluence",
            &[MESH_FILTER_ID, NEUTRON_FILTER_ID, NEUTRON_ENERGY_FILTER_ID],
            &[],
            &["flux"],
            "tracklength",
        )?;
        tally(
            writer,
            PHOTON_FLUX_TALLY_ID,
            "openbnct.diagnostic.photon_fluence",
            &[MESH_FILTER_ID, PHOTON_FILTER_ID, PHOTON_ENERGY_FILTER_ID],
            &[],
            &["flux"],
            "tracklength",
        )?;
        tally(
            writer,
            NEUTRON_LEAKAGE_TALLY_ID,
            "openbnct.diagnostic.neutron_surface_current",
            &[SURFACE_FILTER_ID, NEUTRON_FILTER_ID],
            &[],
            &["current"],
            "analog",
        )?;
        tally(
            writer,
            PHOTON_LEAKAGE_TALLY_ID,
            "openbnct.diagnostic.photon_surface_current",
            &[SURFACE_FILTER_ID, PHOTON_FILTER_ID],
            &[],
            &["current"],
            "analog",
        )?;
        for (index, roi) in rois.iter().enumerate() {
            let base = ROI_TALLY_ID_BASE + index as u32 * ROI_TALLIES_PER_REGION;
            let mesh_filter = roi.mesh_filter_id;
            let prefix = format!("openbnct.roi.{}", roi.name);
            for roi_tally in roi_tally_plan(&prefix, mesh_filter) {
                tally(
                    writer,
                    base + roi_tally.offset,
                    &roi_tally.name,
                    &roi_tally.filters,
                    &roi_tally.nuclides,
                    &roi_tally.scores,
                    roi_tally.estimator,
                )?;
            }
        }
        Ok(())
    })
}

/// One acceptance tally's placement within a region's tally block.
struct RoiTally {
    offset: u32,
    name: String,
    filters: Vec<u32>,
    nuclides: Vec<&'static str>,
    scores: Vec<&'static str>,
    estimator: &'static str,
}

/// The nine single-bin acceptance tallies emitted per ROI region: the three
/// neutron response components, photon heating, neutron-heating and
/// coupled-heating audits, the B-10/N-14 reaction-rate audits, and the
/// energy-integrated neutron fluence.
fn roi_tally_plan(prefix: &str, mesh_filter: u32) -> [RoiTally; 9] {
    let neutron = NEUTRON_FILTER_ID;
    let photon = PHOTON_FILTER_ID;
    let entry = |offset, name: String, filters, nuclides, scores, estimator| RoiTally {
        offset,
        name,
        filters,
        nuclides,
        scores,
        estimator,
    };
    [
        entry(
            0,
            format!("{prefix}.component.boron.response"),
            vec![mesh_filter, neutron, BORON_RESPONSE_FILTER_ID],
            vec![],
            vec!["flux"],
            "tracklength",
        ),
        entry(
            1,
            format!("{prefix}.component.nitrogen.response"),
            vec![mesh_filter, neutron, NITROGEN_RESPONSE_FILTER_ID],
            vec![],
            vec!["flux"],
            "tracklength",
        ),
        entry(
            2,
            format!("{prefix}.component.hydrogen.response"),
            vec![mesh_filter, neutron, HYDROGEN_RESPONSE_FILTER_ID],
            vec![],
            vec!["flux"],
            "tracklength",
        ),
        entry(
            3,
            format!("{prefix}.component.photon.heating"),
            vec![mesh_filter, photon],
            vec![],
            vec!["heating"],
            "collision",
        ),
        entry(
            4,
            format!("{prefix}.audit.neutron_heating"),
            vec![mesh_filter, neutron],
            vec![],
            vec!["heating"],
            "tracklength",
        ),
        entry(
            5,
            format!("{prefix}.physical_total.coupled_heating"),
            vec![mesh_filter],
            vec![],
            vec!["heating"],
            "collision",
        ),
        entry(
            6,
            format!("{prefix}.audit.b10_mt107"),
            vec![mesh_filter, neutron],
            vec!["B10"],
            vec!["(n,a)"],
            "tracklength",
        ),
        entry(
            7,
            format!("{prefix}.audit.n14_mt103"),
            vec![mesh_filter, neutron],
            vec!["N14"],
            vec!["(n,p)"],
            "tracklength",
        ),
        entry(
            8,
            format!("{prefix}.diagnostic.neutron_fluence"),
            vec![mesh_filter, neutron],
            vec![],
            vec!["flux"],
            "tracklength",
        ),
    ]
}

fn energy_function_filter(
    writer: &mut Writer<Vec<u8>>,
    id: u32,
    energy_ev: &[f64],
    response: &[f64],
) -> io::Result<()> {
    let id = id.to_string();
    let mut filter = BytesStart::new("filter");
    filter.push_attribute(("id", id.as_str()));
    filter.push_attribute(("type", "energyfunction"));
    writer.write_event(Event::Start(filter))?;
    text_element(writer, "energy", &format_numbers(energy_ev))?;
    text_element(writer, "y", &format_numbers(response))?;
    text_element(writer, "interpolation", "linear-linear")?;
    writer.write_event(Event::End(BytesEnd::new("filter")))
}

fn filter_with_bins(
    writer: &mut Writer<Vec<u8>>,
    id: u32,
    filter_type: &str,
    bins: &str,
) -> io::Result<()> {
    let id = id.to_string();
    let mut filter = BytesStart::new("filter");
    filter.push_attribute(("id", id.as_str()));
    filter.push_attribute(("type", filter_type));
    writer.write_event(Event::Start(filter))?;
    text_element(writer, "bins", bins)?;
    writer.write_event(Event::End(BytesEnd::new("filter")))
}

fn tally(
    writer: &mut Writer<Vec<u8>>,
    id: u32,
    name: &str,
    filters: &[u32],
    nuclides: &[&str],
    scores: &[&str],
    estimator: &str,
) -> io::Result<()> {
    let id = id.to_string();
    let mut tally = BytesStart::new("tally");
    tally.push_attribute(("id", id.as_str()));
    tally.push_attribute(("name", name));
    writer.write_event(Event::Start(tally))?;
    if !filters.is_empty() {
        text_element(writer, "filters", &format_integers(filters))?;
    }
    if !nuclides.is_empty() {
        text_element(writer, "nuclides", &nuclides.join(" "))?;
    }
    text_element(writer, "scores", &scores.join(" "))?;
    text_element(writer, "estimator", estimator)?;
    writer.write_event(Event::End(BytesEnd::new("tally")))
}

fn xml_document<F>(root: &str, body: F) -> Result<Vec<u8>, OpenMcInputError>
where
    F: FnOnce(&mut Writer<Vec<u8>>) -> io::Result<()>,
{
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(OpenMcInputError::XmlWrite)?;
    writer
        .write_event(Event::Start(BytesStart::new(root)))
        .map_err(OpenMcInputError::XmlWrite)?;
    body(&mut writer).map_err(OpenMcInputError::XmlWrite)?;
    writer
        .write_event(Event::End(BytesEnd::new(root)))
        .map_err(OpenMcInputError::XmlWrite)?;
    let mut bytes = writer.into_inner();
    bytes.push(b'\n');
    Ok(bytes)
}

/// Emit `<name type="kind"><parameters>...</parameters></name>` — the
/// univariate distribution shape used inside `space`/`angle` source
/// elements.
fn univariate_element(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    kind: &str,
    parameters: &[f64],
) -> io::Result<()> {
    let mut element = BytesStart::new(name);
    element.push_attribute(("type", kind));
    writer.write_event(Event::Start(element))?;
    text_element(writer, "parameters", &format_numbers(parameters))?;
    writer.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

fn text_element(writer: &mut Writer<Vec<u8>>, name: &str, text: &str) -> io::Result<()> {
    writer.write_event(Event::Start(BytesStart::new(name)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(name)))
}

fn tally_contracts(rois: &[OpenMcRoiMesh]) -> Vec<OpenMcTallyContract> {
    let mut contracts = vec![
        response_tally_contract(
            BORON_TALLY_ID,
            "openbnct.component.boron.response",
            DoseComponent::Boron,
        ),
        response_tally_contract(
            NITROGEN_TALLY_ID,
            "openbnct.component.nitrogen.response",
            DoseComponent::Nitrogen,
        ),
        response_tally_contract(
            HYDROGEN_TALLY_ID,
            "openbnct.component.hydrogen.response",
            DoseComponent::Hydrogen,
        ),
        heating_tally_contract(
            NEUTRON_HEATING_TALLY_ID,
            "openbnct.audit.neutron_heating",
            None,
            Some(ParticleType::Neutron),
        ),
        heating_tally_contract(
            PHOTON_HEATING_TALLY_ID,
            "openbnct.component.photon.heating",
            Some(DoseComponent::Photon),
            Some(ParticleType::Photon),
        ),
        heating_tally_contract(
            COUPLED_HEATING_TALLY_ID,
            "openbnct.physical_total.coupled_heating",
            None,
            None,
        ),
        reaction_tally_contract(BORON_REACTION_TALLY_ID, "openbnct.audit.b10_mt107"),
        reaction_tally_contract(NITROGEN_REACTION_TALLY_ID, "openbnct.audit.n14_mt103"),
        flux_tally_contract(
            NEUTRON_FLUX_TALLY_ID,
            "openbnct.diagnostic.neutron_fluence",
            ParticleType::Neutron,
        ),
        flux_tally_contract(
            PHOTON_FLUX_TALLY_ID,
            "openbnct.diagnostic.photon_fluence",
            ParticleType::Photon,
        ),
        leakage_tally_contract(
            NEUTRON_LEAKAGE_TALLY_ID,
            "openbnct.diagnostic.neutron_surface_current",
            ParticleType::Neutron,
        ),
        leakage_tally_contract(
            PHOTON_LEAKAGE_TALLY_ID,
            "openbnct.diagnostic.photon_surface_current",
            ParticleType::Photon,
        ),
    ];
    for (index, roi) in rois.iter().enumerate() {
        let base = ROI_TALLY_ID_BASE + index as u32 * ROI_TALLIES_PER_REGION;
        let prefix = format!("openbnct.roi.{}", roi.name);
        let bins = roi.dimensions.iter().product();
        let acceptance = |mut contract: OpenMcTallyContract, offset: u32| {
            contract.id = base + offset;
            contract.scope = Some(OpenMcTallyScope::Acceptance);
            contract.roi = Some(roi.name.clone());
            contract.bins = Some(bins);
            contract
        };
        contracts.push(acceptance(
            response_tally_contract(
                0,
                &format!("{prefix}.component.boron.response"),
                DoseComponent::Boron,
            ),
            0,
        ));
        contracts.push(acceptance(
            response_tally_contract(
                0,
                &format!("{prefix}.component.nitrogen.response"),
                DoseComponent::Nitrogen,
            ),
            1,
        ));
        contracts.push(acceptance(
            response_tally_contract(
                0,
                &format!("{prefix}.component.hydrogen.response"),
                DoseComponent::Hydrogen,
            ),
            2,
        ));
        contracts.push(acceptance(
            heating_tally_contract(
                0,
                &format!("{prefix}.component.photon.heating"),
                Some(DoseComponent::Photon),
                Some(ParticleType::Photon),
            ),
            3,
        ));
        contracts.push(acceptance(
            heating_tally_contract(
                0,
                &format!("{prefix}.audit.neutron_heating"),
                None,
                Some(ParticleType::Neutron),
            ),
            4,
        ));
        contracts.push(acceptance(
            heating_tally_contract(
                0,
                &format!("{prefix}.physical_total.coupled_heating"),
                None,
                None,
            ),
            5,
        ));
        contracts.push(acceptance(
            reaction_tally_contract(0, &format!("{prefix}.audit.b10_mt107")),
            6,
        ));
        contracts.push(acceptance(
            reaction_tally_contract(0, &format!("{prefix}.audit.n14_mt103")),
            7,
        ));
        contracts.push(acceptance(
            flux_tally_contract(
                0,
                &format!("{prefix}.diagnostic.neutron_fluence"),
                ParticleType::Neutron,
            ),
            8,
        ));
    }
    contracts
}

fn response_tally_contract(id: u32, name: &str, component: DoseComponent) -> OpenMcTallyContract {
    OpenMcTallyContract {
        id,
        name: name.into(),
        component: Some(component),
        particle: Some(ParticleType::Neutron),
        quantity: OpenMcTallyQuantity::ResponseWeightedTrackLength,
        raw_unit: OpenMcRawTallyUnit::GrayCubicCentimeterPerSourceNeutron,
        collection_normalization: OpenMcCollectionNormalization::DivideByVoxelVolumeCm3,
        scope: None,
        roi: None,
        bins: None,
    }
}

fn heating_tally_contract(
    id: u32,
    name: &str,
    component: Option<DoseComponent>,
    particle: Option<ParticleType>,
) -> OpenMcTallyContract {
    OpenMcTallyContract {
        id,
        name: name.into(),
        component,
        particle,
        quantity: OpenMcTallyQuantity::Heating,
        raw_unit: OpenMcRawTallyUnit::ElectronVoltPerSourceNeutron,
        collection_normalization:
            OpenMcCollectionNormalization::ElectronVoltToJouleDivideByVoxelMassKg,
        scope: None,
        roi: None,
        bins: None,
    }
}

fn reaction_tally_contract(id: u32, name: &str) -> OpenMcTallyContract {
    OpenMcTallyContract {
        id,
        name: name.into(),
        component: None,
        particle: Some(ParticleType::Neutron),
        quantity: OpenMcTallyQuantity::ReactionRate,
        raw_unit: OpenMcRawTallyUnit::ReactionsPerSourceNeutron,
        collection_normalization: OpenMcCollectionNormalization::None,
        scope: None,
        roi: None,
        bins: None,
    }
}

fn flux_tally_contract(id: u32, name: &str, particle: ParticleType) -> OpenMcTallyContract {
    OpenMcTallyContract {
        id,
        name: name.into(),
        component: None,
        particle: Some(particle),
        quantity: OpenMcTallyQuantity::EnergyBinnedTrackLength,
        raw_unit: OpenMcRawTallyUnit::CentimeterPerSourceNeutron,
        collection_normalization: OpenMcCollectionNormalization::DivideByVoxelVolumeCm3,
        scope: None,
        roi: None,
        bins: None,
    }
}

fn leakage_tally_contract(id: u32, name: &str, particle: ParticleType) -> OpenMcTallyContract {
    OpenMcTallyContract {
        id,
        name: name.into(),
        component: None,
        particle: Some(particle),
        quantity: OpenMcTallyQuantity::SurfaceCurrent,
        raw_unit: OpenMcRawTallyUnit::ParticlesPerSourceNeutron,
        collection_normalization: OpenMcCollectionNormalization::None,
        scope: None,
        roi: None,
        bins: None,
    }
}

fn generated_file(path: &str, media_type: &str, bytes: Vec<u8>) -> GeneratedOpenMcFile {
    GeneratedOpenMcFile {
        relative_path: path.into(),
        media_type: media_type.into(),
        sha256: sha256_hex(&bytes),
        bytes,
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn format_float(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        value.to_string()
    }
}

fn format_numbers(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| format_float(*value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn format_integers<T: ToString>(values: &[T]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn bool_text(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[derive(Debug, Error, PartialEq)]
pub enum OpenMcProfileError {
    #[error("unsupported OpenMC execution-profile schema {0:?}")]
    UnsupportedSchema(String),
    #[error("OpenMC execution-profile ID is empty")]
    EmptyId,
    #[error("OpenMC version {0:?} is unsupported by this execution profile")]
    UnsupportedOpenMcVersion(String),
    #[error("OpenMC source commit {0:?} is unsupported by this execution profile")]
    UnsupportedOpenMcCommit(String),
    #[error("at least two active tally batches are required; observed {0}")]
    InsufficientBatches(u32),
    #[error("candidate-reference runs require at least 50 active batches; observed {0}")]
    InsufficientCandidateBatches(u32),
    #[error("candidate-reference seed {0} is not in the frozen three-seed set")]
    UnregisteredCandidateSeed(u64),
    #[error("candidate-reference profiles must declare requested_histories (schema 0.2.0)")]
    MissingCandidateHistories,
    #[error("requested histories {histories} do not divide into {batches} batches")]
    HistoriesNotDivisibleByBatches { histories: u64, batches: u32 },
    #[error("OpenMC seed must be nonzero")]
    ZeroSeed,
    #[error("OpenMC execution setting {0} is outside the frozen profile")]
    UnsupportedSetting(&'static str),
    #[error("{0} diagnostic energy grid is invalid or does not cover 0 to 20 MeV")]
    InvalidDiagnosticEnergyGrid(&'static str),
    #[error("{particle} diagnostic grid lacks required boundary {boundary_ev} eV")]
    MissingDiagnosticBoundary {
        particle: &'static str,
        boundary_ev: f64,
    },
}

#[derive(Debug, Error)]
pub enum OpenMcInputError {
    #[error("transport case is invalid: {0}")]
    InvalidTransportCase(String),
    #[error("{artifact} JSON is invalid: {source}")]
    InvalidJson {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("component profile is invalid: {0}")]
    InvalidComponentProfile(String),
    #[error("material artifact is invalid: {0}")]
    InvalidMaterial(String),
    #[error("source artifact is invalid: {0}")]
    InvalidSource(String),
    #[error("neutron response set is invalid or unreviewed: {0}")]
    InvalidResponseSet(String),
    #[error("resolved weight windows are invalid: {0}")]
    InvalidVarianceReduction(String),
    #[error(transparent)]
    InvalidExecutionProfile(#[from] OpenMcProfileError),
    #[error(transparent)]
    InvalidNuclearData(#[from] NuclearDataError),
    #[error("transport case material differs from the content-bound material artifact")]
    CaseMaterialMismatch,
    #[error("transport case source differs from the content-bound source artifact")]
    CaseSourceMismatch,
    #[error("{label} content binding mismatch: declared {declared:?}, observed {observed:?}")]
    ContentBindingMismatch {
        label: &'static str,
        declared: ContentReference,
        observed: ContentReference,
    },
    #[error(
        "reviewed response energy range {response_ev:?} does not cover selected neutron-data range {data_ev:?} eV"
    )]
    ResponseEnergyRangeDoesNotCoverData {
        response_ev: [f64; 2],
        data_ev: [f64; 2],
    },
    #[error("source energy {source_ev} eV is outside selected neutron-data range {data_ev:?}")]
    SourceEnergyOutsideDataRange { source_ev: f64, data_ev: [f64; 2] },
    #[error("source particle {0:?} is unsupported by the first OpenMC profile")]
    UnsupportedSourceParticle(ParticleType),
    #[error("first OpenMC profile supports only an identity LPS direction; observed {0:?}")]
    UnsupportedGeometryDirection([f64; 9]),
    #[error("source plane must lie strictly inside all six transport boundaries")]
    SourceOutsideGeometry,
    #[error("source spatial distribution is not representable in the OpenMC emit path")]
    UnsupportedSourceSpace,
    #[error("{histories} histories are not divisible by {batches} active batches")]
    HistoriesNotDivisibleByBatches { histories: u64, batches: u32 },
    #[error("particles per batch must fit a positive signed 64-bit OpenMC count; observed {0}")]
    InvalidParticlesPerBatch(u64),
    #[error("failed to write deterministic OpenMC XML: {0}")]
    XmlWrite(#[source] io::Error),
    #[error("failed to serialize NCTForge input manifest: {0}")]
    ManifestSerialization(#[source] serde_json::Error),
    #[error("generated file path is not a safe relative path: {0}")]
    UnsafeRelativePath(String),
    #[error("failed to write generated OpenMC input at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("unsupported acceptance-contract schema {0:?}")]
    UnsupportedAcceptanceSchema(String),
    #[error("acceptance contract is invalid: {0}")]
    InvalidAcceptance(String),
    #[error("material assignment is invalid: {0}")]
    InvalidAssignment(String),
    #[error("candidate-reference decks require a bound acceptance contract")]
    CandidateReferenceRequiresAcceptance,
    #[error("acceptance contracts may only bind candidate-reference decks")]
    AcceptanceRequiresCandidateReference,
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeSet;

    use openbnct_core::GridGeometry;
    use openbnct_transport::{ResponseInterpolation, ResponseSetQualification, ResponseUnit};
    use quick_xml::Reader;

    use crate::{
        DataArtifact, DataDistributionIdentity, DataInspectionIdentity, NeutronTableCapability,
        PhotonTableCapability, TARGET_DATA_HDF5_VERSION, TARGET_EVALUATED_DATA_RELEASE,
        TARGET_INSPECTION_METHOD,
    };

    use super::*;

    pub(crate) const COMPONENT_PROFILE_JSON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/component-profile.json"
    );
    pub(crate) const MATERIAL_JSON: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    pub(crate) const SOURCE_JSON: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/source.json");
    pub(crate) const PROFILE_JSON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/openmc-smoke-profile.json"
    );

    pub(crate) fn case() -> TransportCase {
        TransportCase {
            schema_version: "openbnct.transport-case/0.1.0".into(),
            case_id: "nf-bnct-001".into(),
            geometry: GridGeometry {
                shape: [40; 3],
                spacing_mm: [5.0; 3],
                origin_mm: [-97.5; 3],
                direction: IDENTITY_DIRECTION,
            },
            material: serde_json::from_slice(MATERIAL_JSON).unwrap(),
            source: serde_json::from_slice(SOURCE_JSON).unwrap(),
            requested_histories: 1_000,
        }
    }

    fn artifact(path: impl Into<String>) -> DataArtifact {
        DataArtifact {
            relative_path: path.into(),
            sha256: "a".repeat(64),
        }
    }

    fn nuclear_data() -> NuclearDataManifest {
        let case = case();
        let mut neutron_tables = case
            .material
            .nuclides
            .iter()
            .map(|nuclide| NeutronTableCapability {
                nuclide: nuclide.name.clone(),
                artifact: artifact(format!("neutron/{}.h5", nuclide.name)),
                hdf5_version: TARGET_DATA_HDF5_VERSION,
                atomic_weight_ratio: 1.0,
                temperatures_k: vec![294.0],
                energy_ranges_ev: vec![[1.0e-5, 20.0e6]],
                reactions_mt: match nuclide.name.as_str() {
                    "B10" => vec![107, 301],
                    "N14" => vec![103, 301],
                    _ => vec![301],
                },
                photon_production_mts: match nuclide.name.as_str() {
                    "B10" => vec![107],
                    "H1" => vec![102],
                    _ => Vec::new(),
                },
            })
            .collect::<Vec<_>>();
        neutron_tables.sort_by(|left, right| left.nuclide.cmp(&right.nuclide));
        let elements = case
            .material
            .nuclides
            .iter()
            .map(|nuclide| {
                nuclide
                    .name
                    .chars()
                    .take_while(char::is_ascii_alphabetic)
                    .collect::<String>()
            })
            .collect::<BTreeSet<_>>();
        let photon_tables = elements
            .into_iter()
            .map(|element| PhotonTableCapability {
                artifact: artifact(format!("photon/{element}.h5")),
                element,
                hdf5_version: TARGET_DATA_HDF5_VERSION,
                reactions_mt: vec![502, 504, 522],
                has_atomic_relaxation_data: true,
                has_compton_profile_data: true,
            })
            .collect();
        NuclearDataManifest {
            schema_version: crate::data::TARGET_NUCLEAR_DATA_MANIFEST_SCHEMA.into(),
            id: "openbnct.nf-bnct-001.endf-b-viii.1.test".into(),
            openmc_version: TARGET_OPENMC_VERSION.into(),
            openmc_source_commit: TARGET_OPENMC_SOURCE_COMMIT.into(),
            evaluated_data_release: TARGET_EVALUATED_DATA_RELEASE.into(),
            inspection: DataInspectionIdentity {
                method: TARGET_INSPECTION_METHOD.into(),
                source_sha256: "b".repeat(64),
                python_version: "3.14.4".into(),
                numpy_version: "2.5.2".into(),
                h5py_version: "3.16.0".into(),
                hdf5_library_version: "2.0.0".into(),
            },
            distribution: DataDistributionIdentity {
                id: "synthetic-test-data".into(),
                source_uri: crate::data::TARGET_DISTRIBUTION_SOURCE_URI.into(),
                archive_size_bytes: crate::data::TARGET_DISTRIBUTION_ARCHIVE_SIZE_BYTES,
                archive_sha256: "c".repeat(64),
                acquisition_profile_id: crate::data::TARGET_ACQUISITION_PROFILE_ID.into(),
                acquisition_profile_sha256: crate::data::TARGET_ACQUISITION_PROFILE_SHA256.into(),
                acquisition_receipt_sha256: "0".repeat(64),
                publisher_digest_status: crate::PublisherDigestStatus::Unavailable,
                acquisition_evidence_state: crate::AcquisitionEvidenceState::AcquisitionOnly,
            },
            cross_sections: artifact("cross_sections.xml"),
            neutron_tables,
            photon_tables,
        }
    }

    fn json_bytes<T: Serialize>(value: &T) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(value).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn response_set(nuclear_data_json: &[u8]) -> NeutronResponseSet {
        let component: ComponentDefinitionProfile =
            serde_json::from_slice(COMPONENT_PROFILE_JSON).unwrap();
        let material: MaterialDefinition = serde_json::from_slice(MATERIAL_JSON).unwrap();
        let nuclear_data: NuclearDataManifest = serde_json::from_slice(nuclear_data_json).unwrap();
        let boron = vec![1.0e-12, 2.0e-12, 3.0e-12];
        let nitrogen = vec![2.0e-12, 3.0e-12, 4.0e-12];
        let hydrogen = vec![3.0e-12, 4.0e-12, 5.0e-12];
        let total_neutron_gy_cm2 = boron
            .iter()
            .zip(&nitrogen)
            .zip(&hydrogen)
            .map(|((boron, nitrogen), hydrogen)| boron + nitrogen + hydrogen)
            .collect();
        NeutronResponseSet {
            schema_version: "openbnct.neutron-response-set/0.1.0".into(),
            id: "openbnct.nf-bnct-001.response.test".into(),
            qualification: ResponseSetQualification::IndependentlyReviewed,
            component_profile: content_reference(&component.id, COMPONENT_PROFILE_JSON),
            material: content_reference(&material.id, MATERIAL_JSON),
            nuclear_data_manifest: content_reference(&nuclear_data.id, nuclear_data_json),
            generation_method: ContentReference {
                id: "openbnct.nf-bnct-001.response-generation.test".into(),
                sha256: "d".repeat(64),
            },
            independent_review: Some(ContentReference {
                id: "openbnct.nf-bnct-001.response-review.test".into(),
                sha256: "e".repeat(64),
            }),
            transport_energy_range_ev: [1.0e-5, 20.0e6],
            energy_ev: vec![1.0e-5, 1_000.0, 20.0e6],
            unit: ResponseUnit::GraySquareCentimeter,
            interpolation: ResponseInterpolation::LinearLinear,
            boron_gy_cm2: boron,
            nitrogen_gy_cm2: nitrogen,
            hydrogen_gy_cm2: hydrogen,
            total_neutron_gy_cm2,
        }
    }

    pub(crate) struct InputBytes {
        pub(crate) data_root: tempfile::TempDir,
        pub(crate) nuclear_data_json: Vec<u8>,
        pub(crate) response_set_json: Vec<u8>,
    }

    pub(crate) fn input_bytes() -> InputBytes {
        let data_root = tempfile::tempdir().unwrap();
        std::fs::create_dir(data_root.path().join("neutron")).unwrap();
        std::fs::create_dir(data_root.path().join("photon")).unwrap();
        let mut nuclear_data = nuclear_data();
        let mut libraries = Vec::new();
        for table in &mut nuclear_data.neutron_tables {
            let bytes = format!("synthetic neutron table {}\n", table.nuclide).into_bytes();
            std::fs::write(data_root.path().join(&table.artifact.relative_path), &bytes).unwrap();
            table.artifact.sha256 = sha256_hex(&bytes);
            libraries.push(format!(
                "  <library materials=\"{}\" path=\"{}\" type=\"neutron\"/>",
                table.nuclide, table.artifact.relative_path
            ));
        }
        for table in &mut nuclear_data.photon_tables {
            let bytes = format!("synthetic photon table {}\n", table.element).into_bytes();
            std::fs::write(data_root.path().join(&table.artifact.relative_path), &bytes).unwrap();
            table.artifact.sha256 = sha256_hex(&bytes);
            libraries.push(format!(
                "  <library materials=\"{}\" path=\"{}\" type=\"photon\"/>",
                table.element, table.artifact.relative_path
            ));
        }
        let cross_sections = format!(
            "<cross_sections>\n{}\n</cross_sections>\n",
            libraries.join("\n")
        );
        std::fs::write(
            data_root.path().join("cross_sections.xml"),
            cross_sections.as_bytes(),
        )
        .unwrap();
        nuclear_data.cross_sections.sha256 = sha256_hex(cross_sections.as_bytes());

        let nuclear_data_json = json_bytes(&nuclear_data);
        let response_set_json = json_bytes(&response_set(&nuclear_data_json));
        InputBytes {
            data_root,
            nuclear_data_json,
            response_set_json,
        }
    }

    fn generate() -> OpenMcInputDeck {
        let inputs = input_bytes();
        OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap()
    }

    /// A candidate-reference variant of the smoke profile at schema 0.2.0.
    fn candidate_profile_json(seed: u64) -> Vec<u8> {
        let mut profile: serde_json::Value = serde_json::from_slice(PROFILE_JSON).unwrap();
        profile["schema_version"] = serde_json::json!("openbnct.openmc-execution-profile/0.2.0");
        profile["id"] = serde_json::json!("openbnct.test.candidate-reference.v1");
        profile["purpose"] = serde_json::json!("candidate_reference");
        profile["batches"] = serde_json::json!(50);
        profile["seed"] = serde_json::json!(seed);
        profile["requested_histories"] = serde_json::json!(1_000_000u64);
        serde_json::to_vec_pretty(&profile).unwrap()
    }

    fn acceptance_contract_json() -> Vec<u8> {
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": ACCEPTANCE_CONTRACT_SCHEMA,
            "id": "openbnct.test.acceptance.v1",
            "case_id": "nf-bnct-001",
            "regions": [
                {"name": "core",
                 "bounds_cm": {"x_cm": [-2.0, 2.0], "y_cm": [-2.0, 2.0], "z_cm": [-2.0, 2.0]},
                 "dimensions": [1, 1, 1], "precision_gated": true},
                {"name": "axis",
                 "bounds_cm": {"x_cm": [-0.5, 0.5], "y_cm": [-0.5, 0.5], "z_cm": [-10.0, 10.0]},
                 "dimensions": [1, 1, 40], "precision_gated": false}
            ],
            "evaluated_mean_deposited_energy_ev": {
                "b10_mt107_ev": 2_341_900.4411541675,
                "n14_mt103_ev": 625_976.8493398946,
                "evidence_sha256": "a".repeat(64)
            },
            "gates": {
                "roi_relative_standard_uncertainty_max": 0.01,
                "voxel_max_fraction": 0.2,
                "voxel_median_relative_uncertainty_max": 0.03,
                "voxel_p95_relative_uncertainty_max": 0.05,
                "reaction_rate_agreement": 0.02,
                "neutron_heating_agreement": 0.03,
                "coupled_heating_agreement": 0.03,
                "chi_square_p_min": 0.001
            },
            "seeds": CANDIDATE_REFERENCE_SEEDS,
            "min_batches": 50
        }))
        .unwrap()
    }

    fn generate_acceptance() -> OpenMcInputDeck {
        let inputs = input_bytes();
        let profile = candidate_profile_json(CANDIDATE_REFERENCE_SEEDS[0]);
        let contract = acceptance_contract_json();
        OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: &profile,
                acceptance_json: Some(&contract),
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn acceptance_deck_adds_roi_meshes_and_contract() {
        let deck = generate_acceptance();
        let manifest = &deck.manifest;
        assert_eq!(
            manifest.schema_version,
            "openbnct.openmc-input-manifest/0.2.0"
        );
        assert_eq!(manifest.rois.len(), 2);
        assert_eq!(manifest.rois[0].name, "core");
        assert_eq!(manifest.rois[0].dimensions, [1, 1, 1]);
        assert_eq!(manifest.rois[0].volume_cm3, 64.0);
        assert_eq!(manifest.rois[1].dimensions, [1, 1, 40]);
        assert_eq!(manifest.rois[1].volume_cm3, 20.0);
        assert_eq!(manifest.execution.particles_per_batch, 20_000);
        assert!(manifest.bindings.acceptance.is_some());
        // 12 scoring tallies + 9 acceptance tallies per region.
        assert_eq!(manifest.tallies.len(), 12 + 2 * 9);
        let roi_tallies: Vec<_> = manifest
            .tallies
            .iter()
            .filter(|t| t.scope == Some(OpenMcTallyScope::Acceptance))
            .collect();
        assert_eq!(roi_tallies.len(), 18);
        assert!(
            roi_tallies
                .iter()
                .all(|t| t.bins == Some(1) || t.bins == Some(40))
        );
        assert!(manifest.tallies.iter().any(|t| t.name
            == "openbnct.roi.axis.physical_total.coupled_heating"
            && t.bins == Some(40)));
        assert!(deck.file("openbnct-acceptance-contract.json").is_some());
        let tallies = std::str::from_utf8(&deck.file("tallies.xml").unwrap().bytes).unwrap();
        assert!(tallies.contains("openbnct.roi.core.component.boron.response"));
        assert!(tallies.contains("<mesh id=\"3\""));
        assert!(tallies.contains("<filter id=\"11\" type=\"mesh\">"));
    }

    #[test]
    fn acceptance_deck_is_deterministic() {
        assert_eq!(generate_acceptance(), generate_acceptance());
    }

    #[test]
    fn rejects_acceptance_contract_on_smoke_profile() {
        let inputs = input_bytes();
        let contract = acceptance_contract_json();
        let error = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: Some(&contract),
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::AcceptanceRequiresCandidateReference
        ));
    }

    #[test]
    fn rejects_candidate_reference_without_contract() {
        let inputs = input_bytes();
        let profile = candidate_profile_json(CANDIDATE_REFERENCE_SEEDS[0]);
        let error = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: &profile,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::CandidateReferenceRequiresAcceptance
        ));
    }

    #[test]
    fn validates_acceptance_contract_contents() {
        let mut contract: OpenMcAcceptanceContract =
            serde_json::from_slice(&acceptance_contract_json()).unwrap();
        contract.validate().unwrap();

        // Duplicate region names are rejected.
        let mut dup = contract.clone();
        dup.regions.push(dup.regions[0].clone());
        assert!(dup.validate().is_err());
        // Degenerate bounds are rejected.
        let mut bad_bounds = contract.clone();
        bad_bounds.regions[0].bounds_cm.x_cm = [1.0, 1.0];
        assert!(bad_bounds.validate().is_err());
        // At least one precision-gated single-bin region is required.
        contract.regions[0].precision_gated = false;
        assert!(contract.validate().is_err());
        // Seeds outside the frozen set are rejected.
        let mut bad_seed: OpenMcAcceptanceContract =
            serde_json::from_slice(&acceptance_contract_json()).unwrap();
        bad_seed.seeds = vec![1, 2, 3];
        assert!(bad_seed.validate().is_err());
    }

    #[test]
    fn expanded_benchmark_acceptance_contracts_validate() {
        // Every frozen benchmark's predeclared contract must parse and
        // validate — schema drift in a committed artifact is a defect.
        for case in ["nf-bnct-001", "nf-bnct-002", "nf-bnct-003"] {
            let bytes = std::fs::read(format!(
                "{}/../../benchmarks/synthetic/{}/transport/openmc-acceptance-contract.json",
                env!("CARGO_MANIFEST_DIR"),
                case
            ))
            .unwrap();
            let contract: OpenMcAcceptanceContract = serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("{case} contract parse: {e}"));
            contract
                .validate()
                .unwrap_or_else(|e| panic!("{case} contract validate: {e}"));
            assert_eq!(contract.case_id, case);
        }
    }

    /// A material-assignment fixture: CORE is boron-free with the mass moved
    /// to N14 — both nuclides are covered by folded-response estimators.
    fn assignment_json() -> Vec<u8> {
        let base: serde_json::Value = serde_json::from_slice(MATERIAL_JSON).unwrap();
        let mut region_material = base.clone();
        region_material["id"] = serde_json::json!("openbnct.test.core-unloaded.v1");
        let nuclides = region_material["nuclides"].as_array_mut().unwrap();
        nuclides.retain(|n| n["name"] != "B10");
        for n in nuclides.iter_mut() {
            if n["name"] == "N14" {
                // Base N14 0.02589697162573985 + removed B10 4e-5.
                n["mass_fraction"] = serde_json::json!(0.02589697162573985 + 4.0e-5);
            }
        }
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "openbnct.material-assignment/0.2.0",
            "case_id": "nf-bnct-001",
            "base_material": base,
            "regions": [{
                "name": "core",
                "material": region_material,
                "shape": {"kind": "voxel_box", "lower": [16, 16, 16], "upper": [23, 23, 23]},
            }],
            "provenance_id": "case:sha256:test",
        }))
        .unwrap()
    }

    fn generate_assigned(assignment_json: &[u8]) -> Result<OpenMcInputDeck, OpenMcInputError> {
        let inputs = input_bytes();
        OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: Some(assignment_json),
                variance_reduction_json: None,
            },
        )
    }

    #[test]
    fn assigned_deck_carves_regions_and_binds_assignment() {
        let deck = generate_assigned(&assignment_json()).unwrap();
        let manifest = &deck.manifest;
        assert_eq!(
            manifest.schema_version,
            "openbnct.openmc-input-manifest/0.2.0"
        );
        assert!(manifest.bindings.material_assignment.is_some());
        assert!(deck.file("openbnct-material-assignment.json").is_some());
        let materials = std::str::from_utf8(&deck.file("materials.xml").unwrap().bytes).unwrap();
        assert!(materials.contains("<material id=\"1\""));
        assert!(materials.contains("<material id=\"2\""));
        assert!(!materials.contains("<material id=\"3\""));
        let geometry = std::str::from_utf8(&deck.file("geometry.xml").unwrap().bytes).unwrap();
        // Base cell carries the region complement; the region cell fills the box.
        assert!(geometry.contains("material=\"1\""));
        assert!(geometry.contains("material=\"2\""));
        assert!(geometry.contains("~("));
        // Six additional interior planes for the region box.
        assert!(geometry.contains("<surface id=\"101\" type=\"x-plane\""));
        assert!(geometry.contains("<surface id=\"106\" type=\"z-plane\""));
        // Region planes are interior interfaces — no vacuum boundary.
        assert!(!geometry.contains("<surface id=\"101\" type=\"x-plane\" boundary"));
    }

    #[test]
    fn voxel_set_assignment_emits_material_lattice() {
        // A non-box region (an L of three voxels) forces lattice mode: one
        // fill cell over the outer box, one universe per distinct material,
        // and a rectilinear lattice whose XML universes list carries the
        // region's universe only at the member voxels.
        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        assignment["regions"][0]["shape"] = serde_json::json!({
            "kind": "voxel_set",
            "indices": [[5, 3, 2], [6, 3, 2], [5, 4, 2]],
        });
        let deck = generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()).unwrap();
        let geometry = std::str::from_utf8(&deck.file("geometry.xml").unwrap().bytes).unwrap();
        assert!(geometry.contains("fill=\"100\""));
        assert!(geometry.contains("<lattice id=\"100\" name=\"material_map\">"));
        assert!(geometry.contains("<dimension>40 40 40</dimension>"));
        assert!(geometry.contains("<pitch>0.5 0.5 0.5</pitch>"));
        assert!(geometry.contains("<lower_left>-10 -10 -10</lower_left>"));
        // Element universes: 1000 = base material (id 1), 1001 = region
        // material (id 2). No CSG region cells or interior surfaces.
        assert!(geometry.contains("<cell id=\"1000\" material=\"1\" universe=\"1000\"/>"));
        assert!(geometry.contains("<cell id=\"1001\" material=\"2\" universe=\"1001\"/>"));
        assert!(!geometry.contains("~("));
        assert!(!geometry.contains("id=\"101\""));

        // Universe ordering: the XML word at flat index i + nx*iy + nx*ny*iz
        // fills lattice element (i, ny-1-iy, iz) — OpenMC's reversed-y
        // convention. The member voxels (5,3,2), (6,3,2), (5,4,2) therefore
        // sit at word indices 5 + 40*36 + 1600*2, 6 + 40*36 + 1600*2, and
        // 5 + 40*35 + 1600*2.
        let universes: Vec<u32> = geometry
            .split("<universes>")
            .nth(1)
            .unwrap()
            .split("</universes>")
            .next()
            .unwrap()
            .split_whitespace()
            .map(|word| word.parse().unwrap())
            .collect();
        assert_eq!(universes.len(), 40 * 40 * 40);
        for (flat, expected) in [
            (5 + 40 * 36 + 1600 * 2, 1001),
            (6 + 40 * 36 + 1600 * 2, 1001),
            (5 + 40 * 35 + 1600 * 2, 1001),
        ] {
            assert_eq!(universes[flat], expected, "word {flat}");
        }
        assert_eq!(universes.iter().filter(|&&u| u == 1001).count(), 3);
        assert_eq!(
            universes.iter().filter(|&&u| u == 1000).count(),
            40 * 40 * 40 - 3
        );
    }

    #[test]
    fn rejects_uncovered_nuclide_changes_in_assignment() {
        // Moving the removed B10 mass to O16 — uncovered by the response
        // profile — must be rejected.
        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        let nuclides = assignment["regions"][0]["material"]["nuclides"]
            .as_array_mut()
            .unwrap();
        for n in nuclides.iter_mut() {
            if n["name"] == "N14" {
                n["mass_fraction"] = serde_json::json!(0.02589697162573985);
            }
            if n["name"] == "O16" {
                n["mass_fraction"] =
                    serde_json::json!(n["mass_fraction"].as_f64().unwrap() + 4.0e-5);
            }
        }
        let error =
            generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()).unwrap_err();
        assert!(matches!(error, OpenMcInputError::InvalidAssignment(_)));
        assert!(error.to_string().contains("uncovered nuclide"));

        // Dropping an uncovered nuclide entirely is equally invalid: the
        // residual hydrogen estimator assumes base fractions.
        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        let nuclides = assignment["regions"][0]["material"]["nuclides"]
            .as_array_mut()
            .unwrap();
        nuclides.retain(|n| n["name"] != "H1");
        for n in nuclides.iter_mut() {
            if n["name"] == "N14" {
                n["mass_fraction"] =
                    serde_json::json!(n["mass_fraction"].as_f64().unwrap() + 0.10113647042677168);
            }
        }
        let error =
            generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()).unwrap_err();
        assert!(matches!(error, OpenMcInputError::InvalidAssignment(_)));

        // Introducing a nuclide absent from the base material is rejected.
        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        let nuclides = assignment["regions"][0]["material"]["nuclides"]
            .as_array_mut()
            .unwrap();
        nuclides.push(serde_json::json!({"name": "Li6", "mass_fraction": 0.001}));
        for n in nuclides.iter_mut() {
            if n["name"] == "N14" {
                n["mass_fraction"] =
                    serde_json::json!(n["mass_fraction"].as_f64().unwrap() - 0.001);
            }
        }
        let error =
            generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()).unwrap_err();
        assert!(error.to_string().contains("absent from the base material"));
    }

    #[test]
    fn rejects_assignment_case_and_base_mismatch() {
        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        assignment["case_id"] = serde_json::json!("other-case");
        assert!(matches!(
            generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()),
            Err(OpenMcInputError::InvalidAssignment(_))
        ));

        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        assignment["base_material"]["density_g_cm3"] = serde_json::json!(0.5);
        assert!(matches!(
            generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()),
            Err(OpenMcInputError::InvalidAssignment(_))
        ));

        // Region density may differ — collection normalizes heating by the
        // per-voxel mass and rescales folded components by the atom-density
        // ratio. Temperature must still match the bound cross sections.
        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        assignment["regions"][0]["material"]["density_g_cm3"] = serde_json::json!(1.2);
        generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()).unwrap();

        let mut assignment: serde_json::Value = serde_json::from_slice(&assignment_json()).unwrap();
        assignment["regions"][0]["material"]["temperature_k"] = serde_json::json!(300.0);
        assert!(matches!(
            generate_assigned(&serde_json::to_vec_pretty(&assignment).unwrap()),
            Err(OpenMcInputError::InvalidAssignment(_))
        ));
    }

    #[test]
    fn generates_deterministic_complete_openmc_deck() {
        let first = generate();
        let second = generate();
        assert_eq!(first, second);
        assert_eq!(first.files.len(), 5);
        assert_eq!(first.manifest.execution.particles_per_batch, 200);
        assert_eq!(first.manifest.scoring_mesh.lower_left_cm, [-10.0; 3]);
        assert_eq!(first.manifest.scoring_mesh.upper_right_cm, [10.0; 3]);
        assert_eq!(first.manifest.scoring_mesh.voxel_volume_cm3, 0.125);
        assert_eq!(first.manifest.scoring_mesh.voxel_mass_g, 0.125);
        assert_eq!(first.manifest.tallies.len(), 12);

        for file in &first.files {
            assert_eq!(file.sha256, sha256_hex(&file.bytes));
            if file.relative_path.ends_with(".xml") {
                let mut reader = Reader::from_reader(file.bytes.as_slice());
                loop {
                    if matches!(reader.read_event().unwrap(), Event::Eof) {
                        break;
                    }
                }
            }
        }
        assert_eq!(
            first
                .files
                .iter()
                .map(|file| (file.relative_path.as_str(), file.sha256.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "geometry.xml",
                    "ef76acd269a40e33e97ee43879da974e909354d8e26dd8b7e563b81bc0a5eef9"
                ),
                (
                    "materials.xml",
                    "d1451578d62dc078b97780b8ccfd16f7db27105ec14811ec02770f54d5dcfdd0"
                ),
                (
                    "settings.xml",
                    "dc0149c33efb12668f68d70f039da6580969f6de93d75005698fb9d53dfe8d02"
                ),
                (
                    "tallies.xml",
                    "e0729b9874a2fc5d58572879bb1603216bd3fd48f8e9ecb63fd2d45a3860e8f2"
                ),
                (
                    "openbnct-input-manifest.json",
                    "402229bdf539be3aeaf74bca5c9120a76a2e70e48cbb28778bc78264b243044b"
                ),
            ]
        );

        let settings = std::str::from_utf8(&first.file("settings.xml").unwrap().bytes).unwrap();
        assert!(settings.contains("<run_mode>fixed source</run_mode>"));
        assert!(settings.contains("<particles>200</particles>"));
        assert!(settings.contains("<electron_treatment>led</electron_treatment>"));
        assert!(settings.contains("<photon_transport>true</photon_transport>"));

        let tallies = std::str::from_utf8(&first.file("tallies.xml").unwrap().bytes).unwrap();
        assert!(tallies.contains("type=\"energyfunction\""));
        assert!(tallies.contains("<scores>(n,a)</scores>"));
        assert!(tallies.contains("<scores>(n,p)</scores>"));
        assert!(tallies.contains("openbnct.physical_total.coupled_heating"));
    }

    #[test]
    fn rejects_content_binding_drift() {
        let inputs = input_bytes();
        let mut response = response_set(&inputs.nuclear_data_json);
        response.material.sha256 = "f".repeat(64);
        let response_json = json_bytes(&response);
        let error = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &response_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::ContentBindingMismatch {
                label: "response_set.material",
                ..
            }
        ));
    }

    #[test]
    fn rejects_tampered_selected_nuclear_data() {
        let inputs = input_bytes();
        std::fs::write(inputs.data_root.path().join("neutron/B10.h5"), b"tampered").unwrap();
        let error = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::InvalidNuclearData(NuclearDataError::HashMismatch {
                path,
                ..
            }) if path == "neutron/B10.h5"
        ));
    }

    #[test]
    fn rejects_response_domain_that_would_score_silent_zeros() {
        let inputs = input_bytes();
        let mut response = response_set(&inputs.nuclear_data_json);
        response.transport_energy_range_ev[1] = 19.0e6;
        let response_json = json_bytes(&response);
        let error = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &response_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::ResponseEnergyRangeDoesNotCoverData { .. }
        ));
    }

    #[test]
    fn rejects_nonidentity_first_profile_geometry() {
        let inputs = input_bytes();
        let mut case = case();
        case.geometry.direction = [0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let error = OpenMcInputDeck::generate(
            &case,
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::UnsupportedGeometryDirection(_)
        ));
    }

    #[test]
    fn rejects_fractional_batch_partition() {
        let inputs = input_bytes();
        let mut case = case();
        case.requested_histories = 1_001;
        let error = OpenMcInputDeck::generate(
            &case,
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::HistoriesNotDivisibleByBatches {
                histories: 1_001,
                batches: 5
            }
        ));
    }

    #[test]
    fn write_new_materializes_exact_deck_and_refuses_existing_output() {
        let deck = generate();
        let output = tempfile::tempdir().unwrap();
        let deck_dir = output.path().join("deck");
        deck.write_new(&deck_dir).unwrap();
        for file in &deck.files {
            assert_eq!(
                fs::read(deck_dir.join(&file.relative_path)).unwrap(),
                file.bytes,
                "{}",
                file.relative_path
            );
        }
        let mut written: Vec<String> = fs::read_dir(&deck_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let mut expected: Vec<String> = deck
            .files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect();
        written.sort();
        expected.sort();
        assert_eq!(written, expected);
        assert!(matches!(
            deck.write_new(&deck_dir),
            Err(OpenMcInputError::Io { .. })
        ));
    }

    /// The reference case with a caller-supplied source definition; the
    /// source artifact JSON is regenerated to keep content binding honest.
    fn generate_with_source(source: &openbnct_transport::FixedSourceDefinition) -> OpenMcInputDeck {
        let inputs = input_bytes();
        let source_json = serde_json::to_vec_pretty(source).unwrap();
        let mut case = case();
        case.source = source.clone();
        OpenMcInputDeck::generate(
            &case,
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: &source_json,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap()
    }

    fn settings_text(deck: &OpenMcInputDeck) -> String {
        String::from_utf8(deck.file("settings.xml").unwrap().bytes.clone()).unwrap()
    }

    #[test]
    fn emits_monodirectional_angle_as_reference_uvw_element() {
        // reference_uvw is a child element in the OpenMC schema; an
        // attribute of the same name is silently ignored (defaults +z).
        let settings = settings_text(&generate());
        assert!(settings.contains("<angle type=\"monodirectional\">"));
        assert!(settings.contains("<reference_uvw>0 0 1</reference_uvw>"));
        assert!(!settings.contains("reference_uvw="));
    }

    #[test]
    fn emits_disk_cone_and_tabulated_source() {
        let mut source: openbnct_transport::FixedSourceDefinition =
            serde_json::from_slice(SOURCE_JSON).unwrap();
        source.space = openbnct_transport::SourceSpatialDistribution::UniformDisk {
            axis: openbnct_transport::PlaneAxis::Z,
            offset_cm: -9.999999,
            center_uv_cm: [0.0, 0.0],
            radius_cm: 7.0,
        };
        source.angle = AngularDistribution::IsotropicCone {
            axis_unit_vector: [0.0, 0.0, 1.0],
            half_angle_rad: 0.1,
        };
        source.energy = EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: vec![0.5, 1.0e4, 1.0e6],
            bin_weights: vec![3.0, 1.0],
        };
        let settings = settings_text(&generate_with_source(&source));

        // Disk: cylindrical spatial distribution, r ~ powerlaw(n=1) for
        // uniform area, z degenerate on the port plane.
        assert!(settings.contains("<space type=\"cylindrical\">"));
        assert!(settings.contains("<origin>0 0 -9.999999</origin>"));
        assert!(settings.contains("<z_dir>0 0 1</z_dir>"));
        assert!(settings.contains("<r type=\"powerlaw\">"));
        assert!(settings.contains("<parameters>0 7 1</parameters>"));
        assert!(settings.contains("<phi type=\"uniform\">"));
        assert!(settings.contains("<parameters>0 6.283185307179586</parameters>"));
        assert!(settings.contains("<z type=\"discrete\">"));
        assert!(settings.contains("<parameters>0 1</parameters>"));

        // Cone: mu-phi about +z with mu uniform on [cos 0.1, 1].
        assert!(settings.contains("<angle type=\"mu-phi\">"));
        assert!(settings.contains("<reference_uvw>0 0 1</reference_uvw>"));
        assert!(settings.contains("<mu type=\"uniform\">"));
        assert!(settings.contains("<parameters>0.9950041652780258 1</parameters>"));

        // Spectrum: histogram tabular; p values are densities so bin
        // probabilities equal the declared weights despite unequal widths.
        assert!(settings.contains("<energy type=\"tabular\">"));
        assert!(settings.contains("<interpolation>histogram</interpolation>"));
        assert!(settings.contains(
            "<parameters>0.5 10000 1000000 0.0003000150007500375 0.00000101010101010101 0</parameters>"
        ));
    }

    #[test]
    fn rejects_tabulated_source_energy_outside_data_range() {
        let mut source: openbnct_transport::FixedSourceDefinition =
            serde_json::from_slice(SOURCE_JSON).unwrap();
        source.energy = EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: vec![0.5, 1.0e4, 25.0e6],
            bin_weights: vec![1.0, 1.0],
        };
        let inputs = input_bytes();
        let source_json = serde_json::to_vec_pretty(&source).unwrap();
        let mut case = case();
        case.source = source;
        let error = OpenMcInputDeck::generate(
            &case,
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: &source_json,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: None,
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::SourceEnergyOutsideDataRange { source_ev, .. }
                if source_ev == 25.0e6
        ));
    }

    fn resolved_ww_json() -> Vec<u8> {
        // One photon window over the scoring mesh: 2 energy groups, 8
        // cells — small enough to read in the emitted XML.
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "openbnct.weight-windows/0.1.0",
            "id": "openbnct.test.ww.v1",
            "case_id": "nf-bnct-001",
            "spec": {"id": "openbnct.test.vr.v1", "sha256": "00".repeat(32)},
            "windows": [{
                "particle": "photon",
                "mesh": {
                    "dimensions": [2, 2, 2],
                    "lower_left_cm": [-10.0, -10.0, -10.0],
                    "upper_right_cm": [10.0, 10.0, 10.0]
                },
                "energy_bounds_ev": [0.0, 1.0e6, 2.0e7],
                "lower_bounds": [0.1, 0.2, -1.0, 0.4, 0.5, 0.6, 0.7, 0.8,
                                 0.9, 0.9, 0.9, 0.9, 0.9, 0.9, 0.9, 0.9],
                "upper_bounds": [0.3, 0.6, -1.0, 1.2, 1.5, 1.8, 2.1, 2.4,
                                 2.7, 2.7, 2.7, 2.7, 2.7, 2.7, 2.7, 2.7],
                "parameters": {
                    "survival_ratio": 3.0,
                    "max_split": 10,
                    "weight_cutoff": 1e-38
                }
            }],
            "derivation": {
                "method": "explicit",
                "note": "unit test fixture"
            },
            "qualification": "variance_reduction_research_only_not_clinical"
        }))
        .unwrap()
    }

    #[test]
    fn emits_weight_windows_mesh_and_entries_in_settings() {
        let inputs = input_bytes();
        let vr = resolved_ww_json();
        let deck = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: Some(&vr),
            },
        )
        .unwrap();
        let settings = settings_text(&deck);
        // Window mesh is declared in settings.xml (parsed before the
        // weight_windows entries that reference it).
        assert!(settings.contains("<mesh id=\"1000000\" type=\"regular\">"));
        assert!(settings.contains("<weight_windows>"));
        assert!(settings.contains("<particle_type>photon</particle_type>"));
        assert!(settings.contains("<mesh>1000000</mesh>"));
        assert!(settings.contains("<energy_bounds>0 1000000 20000000</energy_bounds>"));
        assert!(settings.contains("<survival_ratio>3</survival_ratio>"));
        assert!(settings.contains("<max_split>10</max_split>"));
        assert!(settings.contains("<weight_cutoff>"));
        // Manifest binds the artifact and ships it in the deck.
        assert!(deck.file("openbnct-weight-windows.json").is_some());
        assert!(deck.manifest.bindings.variance_reduction.is_some());
    }

    #[test]
    fn weight_windows_reject_case_mismatch_and_bad_pairs() {
        let inputs = input_bytes();
        let mut vr: serde_json::Value = serde_json::from_slice(&resolved_ww_json()).unwrap();
        vr["case_id"] = serde_json::json!("other-case");
        let vr_json = serde_json::to_vec(&vr).unwrap();
        let error = OpenMcInputDeck::generate(
            &case(),
            inputs.data_root.path(),
            OpenMcInputArtifacts {
                component_profile_json: COMPONENT_PROFILE_JSON,
                material_json: MATERIAL_JSON,
                source_json: SOURCE_JSON,
                response_set_json: &inputs.response_set_json,
                nuclear_data_manifest_json: &inputs.nuclear_data_json,
                execution_profile_json: PROFILE_JSON,
                acceptance_json: None,
                material_assignment_json: None,
                variance_reduction_json: Some(&vr_json),
            },
        )
        .unwrap_err();
        assert!(matches!(
            error,
            OpenMcInputError::InvalidVarianceReduction(_)
        ));
    }
}
