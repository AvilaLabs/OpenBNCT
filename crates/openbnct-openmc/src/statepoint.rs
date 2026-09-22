// SPDX-License-Identifier: MIT

//! Statepoint reading and collection into the platform dose bundle.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hdf5_pure::{Dataset, File, Group};
use openbnct_core::{
    ContentReference, DoseComponent, DoseUnit, DoseVolume, GridGeometry, PhysicalDoseBundle,
    PhysicalTotalDoseVolume, TotalUncertaintyMethod,
};
use openbnct_transport::{
    CompletedRun, ComponentDefinitionProfile, ComponentEstimator, MaterialAssignment,
};
use thiserror::Error;

use crate::input::{OpenMcInputManifest, OpenMcTallyContract, sha256_hex};

pub const OPENMC_INPUT_MANIFEST_FILE: &str = "openbnct-input-manifest.json";

/// Resolves a run-directory artifact path, falling back to the `nctforge-*`
/// filename used by run directories written before the OpenBNCT rename.
pub(crate) fn resolve_run_file(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    if path.exists() {
        return path;
    }
    if let Some(legacy) = name.strip_prefix("openbnct-") {
        let legacy_path = directory.join(format!("nctforge-{legacy}"));
        if legacy_path.exists() {
            return legacy_path;
        }
    }
    path
}
const EV_TO_JOULE: f64 = 1.602176634e-19;
const CM_TO_MM: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcStatepointTally {
    pub id: u32,
    pub name: String,
    pub estimator: String,
    pub filter_ids: Vec<u32>,
    pub score_bins: u32,
    pub realizations: u32,
    /// Batch mean of the raw tally score in the manifest's declared raw unit.
    pub mean: Vec<f64>,
    /// One-sigma standard error of the batch mean, same unit as `mean`.
    pub standard_error: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcEnergyFunction {
    pub filter_id: u32,
    pub energy_ev: Vec<f64>,
    pub response: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcStatepoint {
    pub batches: u32,
    pub particles_per_batch: u64,
    pub realizations: u32,
    pub seed: u64,
    pub stride: u64,
    pub energy_mode: String,
    pub run_mode: String,
    /// `openmc_version` root attribute, rendered `major.minor.patch`.
    pub openmc_version: String,
    pub energy_functions: BTreeMap<u32, OpenMcEnergyFunction>,
    /// Bin edges (eV) of each `energy`-type filter, keyed by filter id.
    /// Used to verify declared weight-window energy groups against the
    /// tally they were derived from.
    pub energy_filter_edges: BTreeMap<u32, Vec<f64>>,
    pub tallies: Vec<OpenMcStatepointTally>,
}

impl OpenMcStatepoint {
    pub fn open(path: &Path) -> Result<Self, OpenMcCollectError> {
        let file = File::open(path)
            .map_err(|error| OpenMcCollectError::StatepointIo(format!("{error}")))?;
        let attrs = file
            .root()
            .attrs()
            .map_err(|error| OpenMcCollectError::Read(".".into(), error.to_string()))?;
        let filetype = attrs.get("filetype").and_then(|value| match value {
            hdf5_pure::AttrValue::AsciiString(text) | hdf5_pure::AttrValue::String(text) => {
                Some(text.as_str())
            }
            _ => None,
        });
        if filetype != Some("statepoint") {
            return Err(OpenMcCollectError::NotAStatepoint);
        }
        let openmc_version = match attrs.get("openmc_version") {
            Some(hdf5_pure::AttrValue::I32Array(version)) if version.len() == 3 => {
                format!("{}.{}.{}", version[0], version[1], version[2])
            }
            Some(hdf5_pure::AttrValue::I64Array(version)) if version.len() == 3 => {
                format!("{}.{}.{}", version[0], version[1], version[2])
            }
            _ => return Err(OpenMcCollectError::MissingDataset("openmc_version".into())),
        };
        let root = |name: &str| file.dataset(name);
        let scalar_u64 = |name: &str| -> Result<u64, OpenMcCollectError> {
            let dataset =
                root(name).map_err(|_| OpenMcCollectError::MissingDataset(name.to_string()))?;
            let values = dataset
                .read_u64()
                .map_err(|error| OpenMcCollectError::Read(name.to_string(), error.to_string()))?;
            values
                .first()
                .copied()
                .ok_or_else(|| OpenMcCollectError::EmptyDataset(name.to_string()))
        };
        let scalar_string = |name: &str| -> Result<String, OpenMcCollectError> {
            let dataset =
                root(name).map_err(|_| OpenMcCollectError::MissingDataset(name.to_string()))?;
            let values = dataset
                .read_string()
                .map_err(|error| OpenMcCollectError::Read(name.to_string(), error.to_string()))?;
            values
                .first()
                .cloned()
                .ok_or_else(|| OpenMcCollectError::EmptyDataset(name.to_string()))
        };

        let batches = scalar_u64("n_batches")?;
        let particles_per_batch = scalar_u64("n_particles")?;
        let realizations = scalar_u64("n_realizations")?;
        let seed = scalar_u64("seed")?;
        let stride = scalar_u64("stride")?;
        let energy_mode = scalar_string("energy_mode")?;
        let run_mode = scalar_string("run_mode")?;

        let tallies_group = file
            .group("tallies")
            .map_err(|_| OpenMcCollectError::MissingDataset("tallies".to_string()))?;
        let filters_group = tallies_group
            .group("filters")
            .map_err(|_| OpenMcCollectError::MissingDataset("tallies/filters".to_string()))?;

        let mut energy_functions = BTreeMap::new();
        let mut energy_filter_edges = BTreeMap::new();
        for member in filters_group.groups().map_err(|error| {
            OpenMcCollectError::Read("tallies/filters".into(), error.to_string())
        })? {
            let group = filters_group
                .group(&member)
                .map_err(|error| OpenMcCollectError::Read(member.clone(), error.to_string()))?;
            let kind = read_string_member(&group, "type")?;
            if kind == "energy" {
                let filter_id = member
                    .strip_prefix("filter ")
                    .and_then(|digits| digits.parse::<u32>().ok())
                    .ok_or_else(|| OpenMcCollectError::InvalidMember(member.clone()))?;
                energy_filter_edges.insert(filter_id, read_f64_member(&group, "bins")?);
                continue;
            }
            if kind != "energyfunction" {
                continue;
            }
            let filter_id = member
                .strip_prefix("filter ")
                .and_then(|digits| digits.parse::<u32>().ok())
                .ok_or_else(|| OpenMcCollectError::InvalidMember(member.clone()))?;
            let energy_ev = read_f64_member(&group, "energy")?;
            let response = read_f64_member(&group, "y")?;
            if energy_ev.len() != response.len() || energy_ev.len() < 2 {
                return Err(OpenMcCollectError::InvalidEnergyFunction(filter_id));
            }
            energy_functions.insert(
                filter_id,
                OpenMcEnergyFunction {
                    filter_id,
                    energy_ev,
                    response,
                },
            );
        }

        let mut tallies = Vec::new();
        for member in tallies_group
            .groups()
            .map_err(|error| OpenMcCollectError::Read("tallies".into(), error.to_string()))?
        {
            let Some(digits) = member.strip_prefix("tally ") else {
                continue;
            };
            let id = digits
                .parse::<u32>()
                .map_err(|_| OpenMcCollectError::InvalidMember(member.clone()))?;
            let group = tallies_group
                .group(&member)
                .map_err(|error| OpenMcCollectError::Read(member.clone(), error.to_string()))?;
            let name = read_string_member(&group, "name")?;
            let estimator = read_string_member(&group, "estimator")?;
            let filter_ids = read_u64_member(&group, "filters")?
                .into_iter()
                .map(|value| {
                    u32::try_from(value)
                        .map_err(|_| OpenMcCollectError::InvalidMember(member.clone()))
                })
                .collect::<Result<Vec<u32>, _>>()?;
            let score_bins = read_scalar_member(&group, "n_score_bins")?;
            let tally_realizations = read_scalar_member(&group, "n_realizations")?;
            if tally_realizations != realizations {
                return Err(OpenMcCollectError::RealizationMismatch {
                    tally: name.clone(),
                });
            }
            if tally_realizations < 2 {
                return Err(OpenMcCollectError::InsufficientRealizations(name.clone()));
            }

            let results_dataset = group
                .dataset("results")
                .map_err(|_| OpenMcCollectError::MissingDataset(format!("{member}/results")))?;
            let shape = results_dataset
                .shape()
                .map_err(|error| OpenMcCollectError::Read(member.clone(), error.to_string()))?;
            if shape.len() != 3 || shape[1] != 1 || shape[2] != 2 {
                return Err(OpenMcCollectError::InvalidResultsShape {
                    tally: name.clone(),
                    shape,
                });
            }
            let results = results_dataset
                .read_f64()
                .map_err(|error| OpenMcCollectError::Read(member.clone(), error.to_string()))?;
            let bins = shape[0] as usize;
            let (mean, standard_error) = batch_statistics(&results, bins, realizations)?;

            tallies.push(OpenMcStatepointTally {
                id,
                name,
                estimator,
                filter_ids,
                score_bins: u32::try_from(score_bins)
                    .map_err(|_| OpenMcCollectError::InvalidMember(member.clone()))?,
                realizations: u32::try_from(tally_realizations)
                    .map_err(|_| OpenMcCollectError::InvalidMember(member.clone()))?,
                mean,
                standard_error,
            });
        }
        tallies.sort_by_key(|tally| tally.id);

        Ok(Self {
            batches: u32::try_from(batches)
                .map_err(|_| OpenMcCollectError::InvalidMember("n_batches".into()))?,
            particles_per_batch,
            realizations: u32::try_from(realizations)
                .map_err(|_| OpenMcCollectError::InvalidMember("n_realizations".into()))?,
            seed,
            stride,
            energy_mode,
            run_mode,
            openmc_version,
            energy_functions,
            energy_filter_edges,
            tallies,
        })
    }

    pub fn tally(&self, name: &str) -> Option<&OpenMcStatepointTally> {
        self.tallies.iter().find(|tally| tally.name == name)
    }
}

fn read_string_member(group: &Group, member: &str) -> Result<String, OpenMcCollectError> {
    let dataset: Dataset = group
        .dataset(member)
        .map_err(|_| OpenMcCollectError::MissingDataset(member.to_string()))?;
    let values = dataset
        .read_string()
        .map_err(|error| OpenMcCollectError::Read(member.to_string(), error.to_string()))?;
    values
        .into_iter()
        .next()
        .ok_or_else(|| OpenMcCollectError::EmptyDataset(member.to_string()))
}

fn read_f64_member(group: &Group, member: &str) -> Result<Vec<f64>, OpenMcCollectError> {
    group
        .dataset(member)
        .map_err(|_| OpenMcCollectError::MissingDataset(member.to_string()))?
        .read_f64()
        .map_err(|error| OpenMcCollectError::Read(member.to_string(), error.to_string()))
}

fn read_u64_member(group: &Group, member: &str) -> Result<Vec<u64>, OpenMcCollectError> {
    group
        .dataset(member)
        .map_err(|_| OpenMcCollectError::MissingDataset(member.to_string()))?
        .read_u64()
        .map_err(|error| OpenMcCollectError::Read(member.to_string(), error.to_string()))
}

fn read_scalar_member(group: &Group, member: &str) -> Result<u64, OpenMcCollectError> {
    read_u64_member(group, member)?
        .into_iter()
        .next()
        .ok_or_else(|| OpenMcCollectError::EmptyDataset(member.to_string()))
}

fn batch_statistics(
    results: &[f64],
    bins: usize,
    realizations: u64,
) -> Result<(Vec<f64>, Vec<f64>), OpenMcCollectError> {
    if realizations < 2 {
        return Err(OpenMcCollectError::InvalidResultsShape {
            tally: "batch_statistics".to_string(),
            shape: vec![realizations],
        });
    }
    let n = realizations as f64;
    let mut mean = Vec::with_capacity(bins);
    let mut standard_error = Vec::with_capacity(bins);
    for bin in 0..bins {
        let summed = results[bin * 2];
        let summed_sq = results[bin * 2 + 1];
        let value = summed / n;
        let variance = (summed_sq / n - value * value).max(0.0);
        if !value.is_finite() || !variance.is_finite() {
            return Err(OpenMcCollectError::NonFiniteResult { bin });
        }
        mean.push(value);
        standard_error.push((variance / (n - 1.0)).sqrt());
    }
    Ok((mean, standard_error))
}

/// The normalized per-voxel dose and uncertainty for one contract tally.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectedDose {
    pub values: Vec<f64>,
    pub absolute_standard_uncertainty: Vec<f64>,
}

/// Convert a manifest tally's raw means to gray per source neutron per voxel.
/// `voxel_mass_kg` carries one entry per scoring voxel so heterogeneous
/// material densities normalize heating tallies by the true local mass.
fn normalize_tally(
    contract: &OpenMcTallyContract,
    tally: &OpenMcStatepointTally,
    voxel_volume_cm3: f64,
    voxel_mass_kg: &[f64],
) -> Result<CollectedDose, OpenMcCollectError> {
    if tally.mean.len() != tally.standard_error.len() {
        return Err(OpenMcCollectError::InvalidResultsShape {
            tally: contract.name.clone(),
            shape: vec![tally.mean.len() as u64],
        });
    }
    match contract.collection_normalization {
        crate::input::OpenMcCollectionNormalization::DivideByVoxelVolumeCm3 => {
            let scale = 1.0 / voxel_volume_cm3;
            Ok(CollectedDose {
                values: tally.mean.iter().map(|value| value * scale).collect(),
                absolute_standard_uncertainty: tally
                    .standard_error
                    .iter()
                    .map(|value| value * scale)
                    .collect(),
            })
        }
        crate::input::OpenMcCollectionNormalization::ElectronVoltToJouleDivideByVoxelMassKg => {
            if voxel_mass_kg.len() != tally.mean.len() {
                return Err(OpenMcCollectError::DoseBinCountMismatch {
                    tally: contract.name.clone(),
                    expected: voxel_mass_kg.len(),
                    actual: tally.mean.len(),
                });
            }
            let normalize = |mean: &[f64]| -> Vec<f64> {
                mean.iter()
                    .zip(voxel_mass_kg.iter())
                    .map(|(value, mass)| value * EV_TO_JOULE / mass)
                    .collect()
            };
            Ok(CollectedDose {
                values: normalize(&tally.mean),
                absolute_standard_uncertainty: normalize(&tally.standard_error),
            })
        }
        crate::input::OpenMcCollectionNormalization::None => Err(
            OpenMcCollectError::UncollectableQuantity(contract.name.clone()),
        ),
    }
}

/// Locate the latest statepoint in a completed run directory.
pub fn latest_statepoint(
    working_directory: &Path,
) -> Result<std::path::PathBuf, OpenMcCollectError> {
    let mut best: Option<(u32, std::path::PathBuf)> = None;
    let entries = std::fs::read_dir(working_directory).map_err(|error| {
        OpenMcCollectError::Io(working_directory.display().to_string(), error.to_string())
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| OpenMcCollectError::Io("read_dir".into(), error.to_string()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(batch) = name
            .strip_prefix("statepoint.")
            .and_then(|rest| rest.strip_suffix(".h5"))
            .and_then(|digits| digits.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| batch > *b) {
            best = Some((batch, path));
        }
    }
    best.map(|(_, path)| path)
        .ok_or(OpenMcCollectError::NoStatepoint)
}

/// Collect a completed run's statepoint into the platform dose bundle.
///
/// The deck directory is self-describing: the input manifest written by
/// `openmc generate` carries the tally contracts, scoring mesh, collection
/// normalizations, and content bindings. This function verifies the run
/// header and tally contract against that manifest and refuses to import
/// anything that does not match.
pub fn collect_statepoint(
    working_directory: &Path,
) -> Result<PhysicalDoseBundle, OpenMcCollectError> {
    let manifest_path = resolve_run_file(working_directory, OPENMC_INPUT_MANIFEST_FILE);
    let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| {
        OpenMcCollectError::Io(manifest_path.display().to_string(), error.to_string())
    })?;
    let manifest: OpenMcInputManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            OpenMcCollectError::Manifest(manifest_path.display().to_string(), error.to_string())
        })?;

    let statepoint_path = latest_statepoint(working_directory)?;
    let statepoint_bytes = std::fs::read(&statepoint_path).map_err(|error| {
        OpenMcCollectError::Io(statepoint_path.display().to_string(), error.to_string())
    })?;
    let statepoint_sha256 = sha256_hex(&statepoint_bytes);
    let statepoint = OpenMcStatepoint::open(&statepoint_path)?;

    // Run-header binding.
    if statepoint.batches != manifest.execution.batches
        || statepoint.particles_per_batch != manifest.execution.particles_per_batch
        || statepoint.seed != manifest.execution.seed
        || statepoint.stride != manifest.execution.stride
        || statepoint.openmc_version != manifest.openmc_version
    {
        return Err(OpenMcCollectError::RunHeaderMismatch(
            statepoint_path.display().to_string(),
        ));
    }

    // Tally-contract binding: every manifest tally appears with its id.
    for contract in &manifest.tallies {
        let tally = statepoint
            .tally(&contract.name)
            .ok_or_else(|| OpenMcCollectError::MissingTally(contract.name.clone()))?;
        if tally.id != contract.id {
            return Err(OpenMcCollectError::TallyIdMismatch(contract.name.clone()));
        }
        if contract.quantity == crate::input::OpenMcTallyQuantity::ResponseWeightedTrackLength
            && tally.estimator != "tracklength"
        {
            return Err(OpenMcCollectError::EstimatorMismatch(contract.name.clone()));
        }
        if let Some(bins) = contract.bins
            && tally.mean.len() != bins as usize
        {
            return Err(OpenMcCollectError::DoseBinCountMismatch {
                tally: contract.name.clone(),
                expected: bins as usize,
                actual: tally.mean.len(),
            });
        }
    }

    let mesh = &manifest.scoring_mesh;
    let voxel_count = mesh
        .dimensions
        .iter()
        .try_fold(1usize, |acc, dim| acc.checked_mul(*dim as usize))
        .ok_or(OpenMcCollectError::InvalidMesh)?;
    // Region-material corrections: folded-response tallies score the base
    // material's atom densities, so a covered component's per-voxel dose
    // scales by the region/base atom-density ratio (density × mass fraction)
    // of its backing nuclide, and a residual component — all of whose
    // nuclides keep base fractions — scales by the density ratio alone.
    // Native heating tallies already see the real material; their
    // normalization needs the per-voxel mass, which differs from the base
    // voxel mass whenever a region carries a different density.
    let corrections = if manifest.bindings.material_assignment.is_some() {
        Some(load_region_corrections(working_directory, &manifest)?)
    } else {
        None
    };
    let voxel_mass_kg: Vec<f64> = match &corrections {
        Some(corrections) => corrections
            .voxel_density_g_cm3
            .iter()
            .map(|density| density * mesh.voxel_volume_cm3 * 1.0e-3)
            .collect(),
        None => vec![mesh.voxel_mass_g * 1.0e-3; voxel_count],
    };

    let mut components: Vec<DoseVolume> = Vec::new();
    let mut physical_total: Option<PhysicalTotalDoseVolume> = None;

    for contract in &manifest.tallies {
        let tally = statepoint
            .tally(&contract.name)
            .expect("tally presence checked above");
        if contract.scope == Some(crate::input::OpenMcTallyScope::Acceptance) {
            continue;
        }
        let is_mesh = tally.filter_ids.contains(&mesh.mesh_id);
        let is_dose = matches!(
            contract.quantity,
            crate::input::OpenMcTallyQuantity::ResponseWeightedTrackLength
                | crate::input::OpenMcTallyQuantity::Heating
        );
        if !is_dose {
            continue;
        }
        if !is_mesh {
            return Err(OpenMcCollectError::DoseTallyMissingMesh(
                contract.name.clone(),
            ));
        }
        if tally.mean.len() != voxel_count {
            return Err(OpenMcCollectError::DoseBinCountMismatch {
                tally: contract.name.clone(),
                expected: voxel_count,
                actual: tally.mean.len(),
            });
        }
        let mut dose = normalize_tally(contract, tally, mesh.voxel_volume_cm3, &voxel_mass_kg)?;
        match (contract.component, contract.particle) {
            (Some(component), _) => {
                if let Some(corrections) = &corrections
                    && let Some(factor) = corrections.component_factors.get(&component)
                {
                    for ((value, sigma), factor) in dose
                        .values
                        .iter_mut()
                        .zip(dose.absolute_standard_uncertainty.iter_mut())
                        .zip(factor.iter())
                    {
                        *value *= factor;
                        *sigma *= factor;
                    }
                }
                components.push(DoseVolume {
                    component,
                    unit: DoseUnit::GrayPerSourceParticle,
                    values: dose.values,
                    absolute_standard_uncertainty: Some(dose.absolute_standard_uncertainty),
                })
            }
            // The physical total is the coupled heating tally: no component and
            // no particle filter. Particle-filtered audit heating is collected
            // by the estimator comparison, not the dose bundle.
            (None, None) => {
                if physical_total.is_some() {
                    return Err(OpenMcCollectError::DuplicatePhysicalTotal);
                }
                physical_total = Some(PhysicalTotalDoseVolume {
                    unit: DoseUnit::GrayPerSourceParticle,
                    values: dose.values,
                    absolute_standard_uncertainty: Some(dose.absolute_standard_uncertainty),
                    uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
                });
            }
            (None, Some(_)) => continue,
        }
    }

    components.sort_by_key(|volume| volume.component);
    let mut observed = std::collections::BTreeSet::new();
    for volume in &components {
        if !observed.insert(volume.component) {
            return Err(OpenMcCollectError::DuplicateComponent(volume.component));
        }
    }
    for required in DoseComponent::REQUIRED {
        if !observed.contains(&required) {
            return Err(OpenMcCollectError::MissingComponent(required));
        }
    }
    let physical_total = physical_total.ok_or(OpenMcCollectError::MissingPhysicalTotal)?;

    let spacing_mm = [
        (mesh.upper_right_cm[0] - mesh.lower_left_cm[0]) / mesh.dimensions[0] as f64 * CM_TO_MM,
        (mesh.upper_right_cm[1] - mesh.lower_left_cm[1]) / mesh.dimensions[1] as f64 * CM_TO_MM,
        (mesh.upper_right_cm[2] - mesh.lower_left_cm[2]) / mesh.dimensions[2] as f64 * CM_TO_MM,
    ];
    let geometry = GridGeometry {
        shape: mesh.dimensions,
        spacing_mm,
        // GridGeometry origin is the center of voxel (0,0,0), not the corner.
        origin_mm: [
            (mesh.lower_left_cm[0] * CM_TO_MM) + spacing_mm[0] / 2.0,
            (mesh.lower_left_cm[1] * CM_TO_MM) + spacing_mm[1] / 2.0,
            (mesh.lower_left_cm[2] * CM_TO_MM) + spacing_mm[2] / 2.0,
        ],
        direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    let bundle = PhysicalDoseBundle {
        schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.to_string(),
        case_id: manifest.case_id.clone(),
        frame_of_reference_uid: None,
        geometry,
        component_profile: ContentReference {
            id: manifest.bindings.component_profile.id.clone(),
            sha256: manifest.bindings.component_profile.sha256.clone(),
        },
        response_set: ContentReference {
            id: manifest.bindings.response_set.id.clone(),
            sha256: manifest.bindings.response_set.sha256.clone(),
        },
        components,
        physical_total,
        provenance_id: format!(
            "openmc-input-manifest:{}+statepoint:{}",
            sha256_hex(&manifest_bytes),
            statepoint_sha256
        ),
    };
    bundle
        .validate()
        .map_err(|error| OpenMcCollectError::Bundle(error.to_string()))?;
    Ok(bundle)
}

/// Per-voxel corrections derived from the deck's bound material assignment
/// and component profile: multiplicative factors for each folded-response
/// component plus the per-voxel mass density for heating normalization.
/// Coupled-photon-heating components are absent from the factor map — their
/// native tallies already see the real materials.
pub struct RegionCorrections {
    pub component_factors: BTreeMap<DoseComponent, Vec<f64>>,
    pub voxel_density_g_cm3: Vec<f64>,
}

fn load_region_corrections(
    working_directory: &Path,
    manifest: &OpenMcInputManifest,
) -> Result<RegionCorrections, OpenMcCollectError> {
    let read_bound =
        |name: &str, declared: &ContentReference| -> Result<Vec<u8>, OpenMcCollectError> {
            let path = resolve_run_file(working_directory, name);
            let bytes = std::fs::read(&path).map_err(|error| {
                OpenMcCollectError::Io(path.display().to_string(), error.to_string())
            })?;
            if sha256_hex(&bytes) != declared.sha256 {
                return Err(OpenMcCollectError::BindingMismatch(name.to_owned()));
            }
            Ok(bytes)
        };
    let assignment_bytes = read_bound(
        "openbnct-material-assignment.json",
        &manifest
            .bindings
            .material_assignment
            .clone()
            .expect("checked by caller"),
    )?;
    let profile_bytes = read_bound(
        "openbnct-component-profile.json",
        &manifest.bindings.component_profile,
    )?;
    let assignment: MaterialAssignment =
        serde_json::from_slice(&assignment_bytes).map_err(|error| {
            OpenMcCollectError::Manifest("material assignment".into(), error.to_string())
        })?;
    // The hash binding proves provenance, not physical validity — a bound
    // artifact that declares a non-positive density would silently emit
    // inf/NaN dose through the per-voxel normalization below.
    assignment.base_material.validate().map_err(|error| {
        OpenMcCollectError::Manifest("material assignment".into(), error.to_string())
    })?;
    for region in &assignment.regions {
        region.material.validate().map_err(|error| {
            OpenMcCollectError::Manifest("material assignment".into(), error.to_string())
        })?;
    }
    let profile: ComponentDefinitionProfile =
        serde_json::from_slice(&profile_bytes).map_err(|error| {
            OpenMcCollectError::Manifest("component profile".into(), error.to_string())
        })?;

    let voxel_count = manifest
        .scoring_mesh
        .dimensions
        .iter()
        .map(|d| *d as usize)
        .product::<usize>();
    let [nx, ny, _] = manifest.scoring_mesh.dimensions.map(|d| d as usize);
    let base_fraction = |name: &str| -> f64 {
        assignment
            .base_material
            .nuclides
            .iter()
            .find(|n| n.name == name)
            .map(|n| n.mass_fraction)
            .unwrap_or(0.0)
    };

    // Per-voxel density: base everywhere except inside each region. Region
    // assignment validation guarantees non-overlapping membership, so the
    // last writer cannot mask a conflict.
    let base_density = assignment.base_material.density_g_cm3;
    let mut voxel_density_g_cm3 = vec![base_density; voxel_count];
    for region in &assignment.regions {
        region.for_each_voxel(|voxel| {
            voxel_density_g_cm3
                [voxel[0] as usize + nx * voxel[1] as usize + nx * ny * voxel[2] as usize] =
                region.material.density_g_cm3;
        });
    }

    let mut component_factors = BTreeMap::new();
    for rule in &profile.components {
        // Folded tallies score atom density × response. The region/base
        // atom-density ratio factors as (ρ_r/ρ_b) × (f_r/f_b): covered folds
        // additionally apply their nuclide's fraction ratio inside each
        // region; residual folds cover only nuclides whose fractions the
        // assignment gate pins to base, so the density ratio stands alone.
        // Coupled photon heating is a native tally and needs no factor.
        let covered_nuclide = match &rule.estimator {
            ComponentEstimator::NjoyPartialKermaFluenceFold { nuclide, .. } => {
                if base_fraction(nuclide) <= 0.0 {
                    return Err(OpenMcCollectError::Manifest(
                        "component profile".into(),
                        format!(
                            "covered nuclide {nuclide} absent from the assignment base material"
                        ),
                    ));
                }
                Some(nuclide.as_str())
            }
            ComponentEstimator::ResidualNeutronKermaFluenceFold { .. } => None,
            ComponentEstimator::CoupledPhotonHeating => continue,
        };
        let mut per_voxel: Vec<f64> = voxel_density_g_cm3
            .iter()
            .map(|density| density / base_density)
            .collect();
        if let Some(nuclide) = covered_nuclide {
            let base = base_fraction(nuclide);
            for region in &assignment.regions {
                let region_fraction = region
                    .material
                    .nuclides
                    .iter()
                    .find(|n| n.name == *nuclide)
                    .map(|n| n.mass_fraction)
                    .unwrap_or(0.0);
                let fraction_ratio = region_fraction / base;
                region.for_each_voxel(|voxel| {
                    per_voxel[voxel[0] as usize
                        + nx * voxel[1] as usize
                        + nx * ny * voxel[2] as usize] *= fraction_ratio;
                });
            }
        }
        component_factors.insert(rule.component, per_voxel);
    }
    Ok(RegionCorrections {
        component_factors,
        voxel_density_g_cm3,
    })
}

/// Trait-level entry point used by `OpenMcBackend::collect`.
pub fn collect_completed(
    completed: &CompletedRun,
) -> Result<PhysicalDoseBundle, OpenMcCollectError> {
    if completed.exit_code != 0 {
        return Err(OpenMcCollectError::NonZeroExit(completed.exit_code));
    }
    collect_statepoint(Path::new(&completed.working_directory))
}

#[derive(Debug, Error)]
pub enum OpenMcCollectError {
    #[error("cannot read statepoint: {0}")]
    StatepointIo(String),
    #[error("file is not an OpenMC statepoint (filetype attribute missing or unexpected)")]
    NotAStatepoint,
    #[error("statepoint lacks dataset {0}")]
    MissingDataset(String),
    #[error("statepoint dataset {0} is empty")]
    EmptyDataset(String),
    #[error("cannot read {0}: {1}")]
    Read(String, String),
    #[error("invalid HDF5 member name {0}")]
    InvalidMember(String),
    #[error("energy-function filter {0} has inconsistent or degenerate tables")]
    InvalidEnergyFunction(u32),
    #[error("tally {tally} realization count differs from the statepoint header")]
    RealizationMismatch { tally: String },
    #[error("tally {0} has fewer than two realizations")]
    InsufficientRealizations(String),
    #[error("tally {tally} results shape {shape:?} is not (bins, 1, 2)")]
    InvalidResultsShape { tally: String, shape: Vec<u64> },
    #[error("non-finite tally statistics at bin {bin}")]
    NonFiniteResult { bin: usize },
    #[error("io error on {0}: {1}")]
    Io(String, String),
    #[error("cannot parse input manifest {0}: {1}")]
    Manifest(String, String),
    #[error("bound deck file {0} does not hash to its manifest reference")]
    BindingMismatch(String),
    #[error("run header does not match the input manifest for {0}")]
    RunHeaderMismatch(String),
    #[error("contract tally {0} absent from the statepoint")]
    MissingTally(String),
    #[error("contract tally {0} has a different id in the statepoint")]
    TallyIdMismatch(String),
    #[error("response tally {0} is not a tracklength estimator")]
    EstimatorMismatch(String),
    #[error("no statepoint.*.h5 file in the completed run directory")]
    NoStatepoint,
    #[error("invalid scoring mesh")]
    InvalidMesh,
    #[error("dose tally {0} does not carry the scoring mesh filter")]
    DoseTallyMissingMesh(String),
    #[error("dose tally {tally} has {actual} bins, expected {expected}")]
    DoseBinCountMismatch {
        tally: String,
        expected: usize,
        actual: usize,
    },
    #[error("tally {0} has no dose collection normalization")]
    UncollectableQuantity(String),
    #[error("more than one unassigned heating tally cannot serve as the physical total")]
    DuplicatePhysicalTotal,
    #[error("no coupled-heating tally can serve as the physical total")]
    MissingPhysicalTotal,
    #[error("duplicate dose component {0:?}")]
    DuplicateComponent(DoseComponent),
    #[error("dose component {0:?} absent from the statepoint")]
    MissingComponent(DoseComponent),
    #[error("run completed with exit code {0}")]
    NonZeroExit(i32),
    #[error("collected bundle failed validation: {0}")]
    Bundle(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::DoseComponent;

    const REALIZATIONS: i64 = 5;
    const VOXELS: usize = 2;

    struct TallySpec {
        id: u32,
        name: &'static str,
        filters: &'static [i64],
        bins: usize,
        mean: f64,
        std: f64,
    }

    const RESPONSE_ENERGY: [f64; 3] = [1.0e-5, 1.0, 2.0e7];
    const BORON_CURVE: [f64; 3] = [2.0e-10, 1.0e-10, 5.0e-11];

    fn tally_defs() -> Vec<TallySpec> {
        vec![
            TallySpec {
                id: 1,
                name: "openbnct.component.boron.response",
                filters: &[1, 2, 4],
                bins: VOXELS,
                mean: 1.0e-12,
                std: 1.0e-14,
            },
            TallySpec {
                id: 2,
                name: "openbnct.component.nitrogen.response",
                filters: &[1, 2, 5],
                bins: VOXELS,
                mean: 2.0e-13,
                std: 2.0e-15,
            },
            TallySpec {
                id: 3,
                name: "openbnct.component.hydrogen.response",
                filters: &[1, 2, 6],
                bins: VOXELS,
                mean: 5.0e-13,
                std: 5.0e-15,
            },
            TallySpec {
                id: 4,
                name: "openbnct.audit.neutron_heating",
                filters: &[1, 2],
                bins: VOXELS,
                mean: 170_000.0,
                std: 1_000.0,
            },
            TallySpec {
                id: 5,
                name: "openbnct.component.photon.heating",
                filters: &[1, 3],
                bins: VOXELS,
                mean: 20_000.0,
                std: 400.0,
            },
            TallySpec {
                id: 6,
                name: "openbnct.physical_total.coupled_heating",
                filters: &[1],
                bins: VOXELS,
                mean: 190_000.0,
                std: 1_100.0,
            },
            TallySpec {
                id: 7,
                name: "openbnct.audit.b10_mt107",
                filters: &[1, 2],
                bins: VOXELS,
                mean: 0.004,
                std: 4.0e-5,
            },
            TallySpec {
                id: 8,
                name: "openbnct.diagnostic.neutron_surface_current",
                filters: &[9, 2],
                bins: 6,
                mean: -0.02,
                std: 2.0e-4,
            },
        ]
    }

    fn write_statepoint(path: &Path, tallies: &[TallySpec], seed: i64) {
        let file = hdf5_pure::File::create(path).expect("create statepoint fixture");
        let root = file.root();
        root.set_attr(
            "filetype",
            hdf5_pure::AttrValue::AsciiString("statepoint".into()),
        )
        .unwrap();
        root.set_attr(
            "openmc_version",
            hdf5_pure::AttrValue::I32Array(vec![0, 16, 0]),
        )
        .unwrap();
        let scalar_i64 = |name: &str, value: i64| {
            root.create_dataset(name, |d| {
                d.with_i64_data(&[value]);
            })
            .unwrap();
        };
        scalar_i64("n_batches", REALIZATIONS);
        scalar_i64("n_particles", 200);
        scalar_i64("n_realizations", REALIZATIONS);
        scalar_i64("seed", seed);
        scalar_i64("stride", 152_917);
        root.create_dataset("energy_mode", |d| {
            d.with_vlen_strings(&["continuous-energy"]);
        })
        .unwrap();
        root.create_dataset("run_mode", |d| {
            d.with_vlen_strings(&["fixed source"]);
        })
        .unwrap();

        let tallies_group = root.create_group("tallies").unwrap();
        let filters_group = tallies_group.create_group("filters").unwrap();
        let mesh_filter = filters_group.create_group("filter 1").unwrap();
        mesh_filter
            .create_dataset("type", |d| {
                d.with_vlen_strings(&["mesh"]);
            })
            .unwrap();
        mesh_filter
            .create_dataset("bins", |d| {
                d.with_i64_data(&[0, 1]);
            })
            .unwrap();
        for (filter_id, curve) in [(4, &BORON_CURVE), (5, &BORON_CURVE), (6, &BORON_CURVE)] {
            let filter = filters_group
                .create_group(&format!("filter {filter_id}"))
                .unwrap();
            filter
                .create_dataset("type", |d| {
                    d.with_vlen_strings(&["energyfunction"]);
                })
                .unwrap();
            filter
                .create_dataset("energy", |d| {
                    d.with_f64_data(&RESPONSE_ENERGY);
                })
                .unwrap();
            filter
                .create_dataset("y", |d| {
                    d.with_f64_data(curve);
                })
                .unwrap();
        }
        filters_group
            .create_group("filter 9")
            .unwrap()
            .create_dataset("type", |d| {
                d.with_vlen_strings(&["surface"]);
            })
            .unwrap();

        for spec in tallies {
            let group = tallies_group
                .create_group(&format!("tally {}", spec.id))
                .unwrap();
            group
                .create_dataset("name", |d| {
                    d.with_vlen_strings(&[spec.name]);
                })
                .unwrap();
            group
                .create_dataset("estimator", |d| {
                    d.with_vlen_strings(&["tracklength"]);
                })
                .unwrap();
            group
                .create_dataset("filters", |d| {
                    d.with_i64_data(spec.filters);
                })
                .unwrap();
            group
                .create_dataset("n_filters", |d| {
                    d.with_i64_data(&[spec.filters.len() as i64]);
                })
                .unwrap();
            group
                .create_dataset("n_score_bins", |d| {
                    d.with_i64_data(&[1]);
                })
                .unwrap();
            group
                .create_dataset("n_realizations", |d| {
                    d.with_i64_data(&[REALIZATIONS]);
                })
                .unwrap();
            let n = REALIZATIONS as f64;
            let mut results = Vec::with_capacity(spec.bins * 2);
            for _ in 0..spec.bins {
                results.push(spec.mean * n);
                results.push(n * (spec.mean * spec.mean + (n - 1.0) * spec.std * spec.std));
            }
            group
                .create_dataset("results", |d| {
                    d.with_f64_data(&results)
                        .with_shape(&[spec.bins as u64, 1, 2]);
                })
                .unwrap();
        }
        file.commit().expect("commit statepoint fixture");
    }

    fn tally_entry(spec: &TallySpec) -> serde_json::Value {
        let entry = |component: serde_json::Value,
                     particle: serde_json::Value,
                     quantity: &str,
                     raw_unit: &str,
                     normalization: &str| {
            serde_json::json!({
                "id": spec.id,
                "name": spec.name,
                "component": component,
                "particle": particle,
                "quantity": quantity,
                "raw_unit": raw_unit,
                "collection_normalization": normalization,
            })
        };
        let neutron = serde_json::json!("neutron");
        let heating = (
            "heating",
            "electron_volt_per_source_neutron",
            "electron_volt_to_joule_divide_by_voxel_mass_kg",
        );
        match spec.id {
            1 => entry(
                "boron".into(),
                neutron,
                "response_weighted_track_length",
                "gray_cubic_centimeter_per_source_neutron",
                "divide_by_voxel_volume_cm3",
            ),
            2 => entry(
                "nitrogen".into(),
                neutron,
                "response_weighted_track_length",
                "gray_cubic_centimeter_per_source_neutron",
                "divide_by_voxel_volume_cm3",
            ),
            3 => entry(
                "hydrogen".into(),
                neutron,
                "response_weighted_track_length",
                "gray_cubic_centimeter_per_source_neutron",
                "divide_by_voxel_volume_cm3",
            ),
            4 => entry(
                serde_json::Value::Null,
                neutron,
                heating.0,
                heating.1,
                heating.2,
            ),
            5 => entry(
                "photon".into(),
                "photon".into(),
                heating.0,
                heating.1,
                heating.2,
            ),
            6 => entry(
                serde_json::Value::Null,
                serde_json::Value::Null,
                heating.0,
                heating.1,
                heating.2,
            ),
            7 => entry(
                serde_json::Value::Null,
                neutron,
                "reaction_rate",
                "reactions_per_source_neutron",
                "none",
            ),
            _ => entry(
                serde_json::Value::Null,
                neutron,
                "surface_current",
                "particles_per_source_neutron",
                "none",
            ),
        }
    }

    fn manifest_json(tallies: &[TallySpec]) -> serde_json::Value {
        let tally_entries: Vec<serde_json::Value> = tallies.iter().map(tally_entry).collect();
        serde_json::json!({
            "schema_version": "openbnct.openmc-input-manifest/0.1.0",
            "case_id": "synthetic-case",
            "backend_id": "openmc",
            "openmc_version": "0.16.0",
            "openmc_source_commit": "617d35a5063c57796b43428bc401e627d2011046",
            "bindings": {
                "component_profile": {"id": "profile", "sha256": "a".repeat(64)},
                "material": {"id": "material", "sha256": "b".repeat(64)},
                "source": {"id": "source", "sha256": "c".repeat(64)},
                "response_set": {"id": "response-set", "sha256": "d".repeat(64)},
                "nuclear_data_manifest": {"id": "ndm", "sha256": "e".repeat(64)},
                "response_generation_method": {"id": "method", "sha256": "f".repeat(64)},
                "independent_response_review": {"id": "review", "sha256": "0".repeat(64)},
                "execution_profile": {"id": "profile", "sha256": "1".repeat(64)},
            },
            "execution": {
                "purpose": "smoke_only",
                "requested_histories": 1000,
                "batches": REALIZATIONS,
                "particles_per_batch": 200,
                "seed": 20260831,
                "stride": 152917,
            },
            "scoring_mesh": {
                "mesh_id": 1,
                "dimensions": [2, 1, 1],
                "lower_left_cm": [-1.0, -0.5, -0.5],
                "upper_right_cm": [1.0, 0.5, 0.5],
                "voxel_volume_cm3": 1.0,
                "voxel_mass_g": 1.0,
                "cell_volume_cm3": 2.0,
                "cell_mass_g": 2.0,
            },
            "tallies": tally_entries,
            "xml_artifacts": [],
        })
    }

    fn write_deck(directory: &Path, tallies: &[TallySpec]) {
        std::fs::create_dir_all(directory).unwrap();
        let manifest = manifest_json(tallies);
        std::fs::write(
            directory.join(OPENMC_INPUT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        write_statepoint(&directory.join("statepoint.5.h5"), tallies, 20260831);
    }

    #[test]
    fn collects_components_and_total_with_normalization() {
        let directory = tempfile::tempdir().unwrap();
        write_deck(directory.path(), &tally_defs());
        let bundle = collect_statepoint(directory.path()).unwrap();
        bundle.validate().unwrap();
        assert_eq!(bundle.case_id, "synthetic-case");
        assert_eq!(bundle.components.len(), 4);
        assert_eq!(bundle.geometry.shape, [2, 1, 1],);
        // Origin is the center of voxel (0,0,0): lower_left + half spacing.
        assert_eq!(bundle.geometry.origin_mm, [-5.0, 0.0, 0.0]);
        assert_eq!(bundle.geometry.spacing_mm, [10.0, 10.0, 10.0]);

        let boron = bundle
            .components
            .iter()
            .find(|volume| volume.component == DoseComponent::Boron)
            .unwrap();
        // Gy cm^3 / voxel volume 1.0 cm^3.
        assert_eq!(boron.values, vec![1.0e-12, 1.0e-12]);
        for sigma in boron.absolute_standard_uncertainty.as_deref().unwrap() {
            assert!((sigma - 1.0e-14).abs() / 1.0e-14 < 1.0e-6);
        }

        let photon = bundle
            .components
            .iter()
            .find(|volume| volume.component == DoseComponent::Photon)
            .unwrap();
        // eV * 1.602e-19 J/eV / 1e-3 kg.
        let expected_photon = 20_000.0 * 1.602176634e-19 / 1.0e-3;
        assert!((photon.values[0] - expected_photon).abs() / expected_photon < 1.0e-12);

        assert_eq!(
            bundle.physical_total.uncertainty_method,
            TotalUncertaintyMethod::DedicatedEstimator
        );
        let expected_total = 190_000.0 * 1.602176634e-19 / 1.0e-3;
        assert!(
            (bundle.physical_total.values[0] - expected_total).abs() / expected_total < 1.0e-12
        );
        assert!(bundle.provenance_id.starts_with("openmc-input-manifest:"));
        assert!(bundle.provenance_id.contains("+statepoint:"));
    }

    fn assignment_json() -> serde_json::Value {
        let material = |id: &str, nuclides: serde_json::Value| {
            serde_json::json!({
                "schema_version": "openbnct.material-definition/0.1.0",
                "id": id,
                "density_g_cm3": 1.0,
                "temperature_k": 293.6,
                "nuclides": nuclides,
                "neutron_thermal_treatment": "free_gas",
            })
        };
        serde_json::json!({
            "schema_version": "openbnct.material-assignment/0.2.0",
            "case_id": "synthetic-case",
            "base_material": material("base", serde_json::json!([
                {"name": "B10", "mass_fraction": 0.5},
                {"name": "N14", "mass_fraction": 0.5},
            ])),
            "regions": [{
                "name": "core",
                "material": material("core-unloaded", serde_json::json!([
                    {"name": "N14", "mass_fraction": 1.0},
                ])),
                "shape": {"kind": "voxel_box", "lower": [1, 0, 0], "upper": [1, 0, 0]},
            }],
            "provenance_id": "case:sha256:test",
        })
    }

    fn component_profile_json() -> serde_json::Value {
        let fold = |component: &str, nuclide: &str, mt: u16| {
            serde_json::json!({
                "component": component,
                "estimator": {
                    "kind": "njoy_partial_kerma_fluence_fold",
                    "nuclide": nuclide,
                    "reaction_mt": mt,
                    "heatr_partial_kerma_mt": mt + 300,
                    "photon_energy": "excluded_and_transported",
                },
            })
        };
        serde_json::json!({
            "schema_version": "openbnct.component-definition-profile/0.1.0",
            "id": "profile",
            "spatial_model": "macroscopic_local_charged_particle_kerma",
            "source_normalization": "per_unit_weight_source_neutron",
            "neutron_response": {
                "unit": "gray_square_centimeter",
                "interpolation": "linear_linear",
                "fold_normalization": "divide_track_length_by_scoring_volume_cm3",
                "outside_domain": "reject_run",
            },
            "components": [
                fold("boron", "B10", 107),
                fold("nitrogen", "N14", 103),
                {
                    "component": "hydrogen",
                    "estimator": {
                        "kind": "residual_neutron_kerma_fluence_fold",
                        "heatr_total_kerma_mt": 301,
                        "subtract_components": ["boron", "nitrogen"],
                    },
                },
                {"component": "photon", "estimator": {"kind": "coupled_photon_heating"}},
            ],
            "physical_total": "coupled_heating_without_particle_filter",
        })
    }

    #[test]
    fn region_density_correction_scales_covered_components_only() {
        let directory = tempfile::tempdir().unwrap();
        write_deck(directory.path(), &tally_defs());

        // Bind the assignment and real profile bytes into the manifest.
        let assignment_bytes = serde_json::to_vec_pretty(&assignment_json()).unwrap();
        let profile_bytes = serde_json::to_vec_pretty(&component_profile_json()).unwrap();
        std::fs::write(
            directory.path().join("openbnct-material-assignment.json"),
            &assignment_bytes,
        )
        .unwrap();
        std::fs::write(
            directory.path().join("openbnct-component-profile.json"),
            &profile_bytes,
        )
        .unwrap();
        let manifest_path = directory.path().join(OPENMC_INPUT_MANIFEST_FILE);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["bindings"]["component_profile"]["sha256"] = sha256_hex(&profile_bytes).into();
        manifest["bindings"]["material_assignment"] = serde_json::json!({
            "id": "openbnct.material-assignment/0.2.0",
            "sha256": sha256_hex(&assignment_bytes),
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let bundle = collect_statepoint(directory.path()).unwrap();
        bundle.validate().unwrap();
        // Voxel 1 is inside the boron-free region: boron folds to zero,
        // nitrogen doubles (fraction 1.0 vs base 0.5), hydrogen and photon
        // stay uncorrected, and the native-heating total is untouched.
        fn component<'a>(
            bundle: &'a openbnct_core::PhysicalDoseBundle,
            name: &str,
        ) -> &'a openbnct_core::DoseVolume {
            bundle
                .components
                .iter()
                .find(|v| serde_json::to_value(v.component).unwrap() == serde_json::json!(name))
                .unwrap()
        }
        assert_eq!(component(&bundle, "boron").values, vec![1.0e-12, 0.0]);
        assert_eq!(
            component(&bundle, "nitrogen").values,
            vec![2.0e-13, 4.0e-13]
        );
        assert_eq!(
            component(&bundle, "hydrogen").values,
            vec![5.0e-13, 5.0e-13]
        );
        let expected_photon = 20_000.0 * 1.602176634e-19 / 1.0e-3;
        assert!(
            (component(&bundle, "photon").values[1] - expected_photon).abs() / expected_photon
                < 1.0e-12
        );

        // With a denser region material (2×), the atom-density ratio factors
        // as density × fraction: nitrogen quadruples, the residual hydrogen
        // fold doubles, and the native heating tally divides by the doubled
        // voxel mass — halving the reported dose.
        let mut dense_assignment = assignment_json();
        dense_assignment["regions"][0]["material"]["density_g_cm3"] = serde_json::json!(2.0);
        let assignment_bytes = serde_json::to_vec_pretty(&dense_assignment).unwrap();
        std::fs::write(
            directory.path().join("openbnct-material-assignment.json"),
            &assignment_bytes,
        )
        .unwrap();
        manifest["bindings"]["material_assignment"]["sha256"] =
            sha256_hex(&assignment_bytes).into();
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let bundle = collect_statepoint(directory.path()).unwrap();
        bundle.validate().unwrap();
        assert_eq!(component(&bundle, "boron").values, vec![1.0e-12, 0.0]);
        assert_eq!(
            component(&bundle, "nitrogen").values,
            vec![2.0e-13, 8.0e-13]
        );
        assert_eq!(
            component(&bundle, "hydrogen").values,
            vec![5.0e-13, 1.0e-12]
        );
        let expected_photon = 20_000.0 * 1.602176634e-19 / 2.0e-3;
        assert!(
            (component(&bundle, "photon").values[1] - expected_photon).abs() / expected_photon
                < 1.0e-12
        );
        let expected_total = 190_000.0 * 1.602176634e-19 / 2.0e-3;
        assert!(
            (bundle.physical_total.values[1] - expected_total).abs() / expected_total < 1.0e-12
        );
    }

    #[test]
    fn rejects_bound_assignment_with_nonpositive_density() {
        let directory = tempfile::tempdir().unwrap();
        write_deck(directory.path(), &tally_defs());
        let mut bad = assignment_json();
        bad["regions"][0]["material"]["density_g_cm3"] = serde_json::json!(0.0);
        let assignment_bytes = serde_json::to_vec_pretty(&bad).unwrap();
        std::fs::write(
            directory.path().join("openbnct-material-assignment.json"),
            &assignment_bytes,
        )
        .unwrap();
        let profile_bytes = serde_json::to_vec_pretty(&component_profile_json()).unwrap();
        std::fs::write(
            directory.path().join("openbnct-component-profile.json"),
            &profile_bytes,
        )
        .unwrap();
        let manifest_path = directory.path().join(OPENMC_INPUT_MANIFEST_FILE);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["bindings"]["component_profile"]["sha256"] = sha256_hex(&profile_bytes).into();
        manifest["bindings"]["material_assignment"] = serde_json::json!({
            "id": "openbnct.material-assignment/0.2.0",
            "sha256": sha256_hex(&assignment_bytes),
        });
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            collect_statepoint(directory.path()),
            Err(OpenMcCollectError::Manifest(_, _))
        ));
    }

    #[test]
    fn refuses_missing_tally_and_header_mismatch() {
        let directory = tempfile::tempdir().unwrap();
        let mut tallies = tally_defs();
        write_deck(directory.path(), &tallies);
        // Corrupt the seed recorded in the statepoint.
        write_statepoint(&directory.path().join("statepoint.5.h5"), &tallies, 9999);
        assert!(matches!(
            collect_statepoint(directory.path()),
            Err(OpenMcCollectError::RunHeaderMismatch(_))
        ));

        // Drop the photon tally from the manifest's contract.
        tallies.retain(|spec| spec.id != 5);
        write_deck(directory.path(), &tallies);
        assert!(matches!(
            collect_statepoint(directory.path()),
            Err(OpenMcCollectError::MissingComponent(DoseComponent::Photon))
        ));
    }

    #[test]
    fn refuses_missing_statepoint_total_and_id_mismatch() {
        // No statepoint at all.
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(OPENMC_INPUT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest_json(&tally_defs())).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            collect_statepoint(directory.path()),
            Err(OpenMcCollectError::NoStatepoint)
        ));

        // Manifest declares no coupled-heating tally.
        let directory = tempfile::tempdir().unwrap();
        let mut tallies = tally_defs();
        tallies.retain(|spec| spec.id != 6);
        write_deck(directory.path(), &tallies);
        assert!(matches!(
            collect_statepoint(directory.path()),
            Err(OpenMcCollectError::MissingPhysicalTotal)
        ));

        // Statepoint tally id disagrees with the manifest contract.
        let directory = tempfile::tempdir().unwrap();
        let tallies = tally_defs();
        write_deck(directory.path(), &tallies);
        let manifest_path = directory.path().join(OPENMC_INPUT_MANIFEST_FILE);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["tallies"][0]["id"] = serde_json::json!(99);
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            collect_statepoint(directory.path()),
            Err(OpenMcCollectError::TallyIdMismatch(_))
        ));
    }

    #[test]
    fn refuses_nonzero_exit_and_selects_latest_statepoint() {
        let directory = tempfile::tempdir().unwrap();
        write_deck(directory.path(), &tally_defs());
        let completed = CompletedRun {
            backend_id: "openmc".into(),
            case_id: "synthetic-case".into(),
            working_directory: directory.path().display().to_string(),
            exit_code: 1,
        };
        assert!(matches!(
            collect_completed(&completed),
            Err(OpenMcCollectError::NonZeroExit(1))
        ));

        let latest = latest_statepoint(directory.path()).unwrap();
        assert_eq!(latest.file_name().unwrap(), "statepoint.5.h5");
        write_statepoint(
            &directory.path().join("statepoint.7.h5"),
            &tally_defs(),
            20260831,
        );
        let latest = latest_statepoint(directory.path()).unwrap();
        assert_eq!(latest.file_name().unwrap(), "statepoint.7.h5");
    }
}
