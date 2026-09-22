// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use openbnct_transport::{MaterialDefinition, TransportCase};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::acquisition::{AcquisitionEvidenceState, PublisherDigestStatus};

pub const TARGET_OPENMC_VERSION: &str = "0.16.0";
pub const TARGET_OPENMC_SOURCE_COMMIT: &str = "617d35a5063c57796b43428bc401e627d2011046";
pub const TARGET_EVALUATED_DATA_RELEASE: &str = "ENDF/B-VIII.1";
pub const TARGET_DATA_HDF5_VERSION: [u16; 2] = [3, 0];
pub const TARGET_NUCLEAR_DATA_MANIFEST_SCHEMA: &str = "openbnct.openmc-nuclear-data-manifest/0.3.0";
pub const TARGET_INSPECTION_METHOD: &str = "openbnct-openmc-data-inspector/0.3.0";
pub const TARGET_ACQUISITION_PROFILE_ID: &str = "openmc-endfb81-official-library-2025-12-18";
pub const TARGET_ACQUISITION_PROFILE_SHA256: &str =
    "237a45d81b7f57dbbb0f1acace641e5dcbda13757e9bfcef686b4daf145ecab7";
pub const TARGET_DISTRIBUTION_SOURCE_URI: &str =
    "https://anl.box.com/shared/static/6qr7jezzihkj9p9esl5jn19qgpujyjyz.xz";
pub const TARGET_DISTRIBUTION_ARCHIVE_SIZE_BYTES: u64 = 9_661_406_540;
/// OpenMC rounds HDF5 kT values to integer kelvin when selecting tables.
pub const TEMPERATURE_TOLERANCE_K: f64 = 0.5;
/// ENDF permits B-10 alpha photon production on aggregate MT 107 or the
/// first-excited-state component MT 801 (whose cross section contributes to 107).
const B10_ALPHA_PHOTON_PRODUCTION_MTS: [u16; 2] = [107, 801];

/// Case-scoped, content-addressed description of the OpenMC data actually used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NuclearDataManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub openmc_version: String,
    pub openmc_source_commit: String,
    pub evaluated_data_release: String,
    pub inspection: DataInspectionIdentity,
    pub distribution: DataDistributionIdentity,
    pub cross_sections: DataArtifact,
    pub neutron_tables: Vec<NeutronTableCapability>,
    pub photon_tables: Vec<PhotonTableCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataInspectionIdentity {
    pub method: String,
    pub source_sha256: String,
    pub python_version: String,
    pub numpy_version: String,
    pub h5py_version: String,
    pub hdf5_library_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataDistributionIdentity {
    pub id: String,
    pub source_uri: String,
    pub archive_size_bytes: u64,
    pub archive_sha256: String,
    pub acquisition_profile_id: String,
    pub acquisition_profile_sha256: String,
    pub acquisition_receipt_sha256: String,
    pub publisher_digest_status: PublisherDigestStatus,
    pub acquisition_evidence_state: AcquisitionEvidenceState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataArtifact {
    /// Normalized path relative to the data root supplied to preflight.
    pub relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeutronTableCapability {
    pub nuclide: String,
    pub artifact: DataArtifact,
    pub hdf5_version: [u16; 2],
    pub atomic_weight_ratio: f64,
    /// Temperatures exposed to OpenMC, in kelvin and strictly increasing.
    pub temperatures_k: Vec<f64>,
    /// Incident-neutron energy bounds for each corresponding temperature grid.
    pub energy_ranges_ev: Vec<[f64; 2]>,
    /// Available incident-neutron reaction MT numbers, strictly increasing.
    pub reactions_mt: Vec<u16>,
    /// Reaction MT numbers with transported photon products.
    pub photon_production_mts: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotonTableCapability {
    pub element: String,
    pub artifact: DataArtifact,
    pub hdf5_version: [u16; 2],
    /// Available incident-photon reaction MT numbers, strictly increasing.
    pub reactions_mt: Vec<u16>,
    pub has_atomic_relaxation_data: bool,
    pub has_compton_profile_data: bool,
}

impl NuclearDataManifest {
    pub fn validate(&self) -> Result<(), NuclearDataError> {
        if !openbnct_core::schema_matches(&self.schema_version, TARGET_NUCLEAR_DATA_MANIFEST_SCHEMA)
        {
            return Err(NuclearDataError::UnsupportedManifestSchema(
                self.schema_version.clone(),
            ));
        }
        validate_identifier("nuclear_data.id", &self.id)?;
        if self.openmc_version != TARGET_OPENMC_VERSION {
            return Err(NuclearDataError::UnsupportedOpenMcVersion(
                self.openmc_version.clone(),
            ));
        }
        if self.openmc_source_commit != TARGET_OPENMC_SOURCE_COMMIT {
            return Err(NuclearDataError::UnsupportedOpenMcCommit(
                self.openmc_source_commit.clone(),
            ));
        }
        if self.evaluated_data_release != TARGET_EVALUATED_DATA_RELEASE {
            return Err(NuclearDataError::UnsupportedEvaluatedDataRelease(
                self.evaluated_data_release.clone(),
            ));
        }

        if !openbnct_core::schema_matches(&self.inspection.method, TARGET_INSPECTION_METHOD) {
            return Err(NuclearDataError::UnsupportedInspectionMethod(
                self.inspection.method.clone(),
            ));
        }
        validate_sha256("inspection.source_sha256", &self.inspection.source_sha256)?;
        validate_identifier("inspection.python_version", &self.inspection.python_version)?;
        validate_identifier("inspection.numpy_version", &self.inspection.numpy_version)?;
        validate_identifier("inspection.h5py_version", &self.inspection.h5py_version)?;
        validate_identifier(
            "inspection.hdf5_library_version",
            &self.inspection.hdf5_library_version,
        )?;

        validate_identifier("distribution.id", &self.distribution.id)?;
        if self.distribution.source_uri != TARGET_DISTRIBUTION_SOURCE_URI {
            return Err(NuclearDataError::UnsupportedDistributionUri(
                self.distribution.source_uri.clone(),
            ));
        }
        if self.distribution.archive_size_bytes != TARGET_DISTRIBUTION_ARCHIVE_SIZE_BYTES {
            return Err(NuclearDataError::UnsupportedArchiveSize(
                self.distribution.archive_size_bytes,
            ));
        }
        validate_sha256(
            "distribution.archive_sha256",
            &self.distribution.archive_sha256,
        )?;
        if self.distribution.acquisition_profile_id != TARGET_ACQUISITION_PROFILE_ID {
            return Err(NuclearDataError::UnsupportedAcquisitionProfileId(
                self.distribution.acquisition_profile_id.clone(),
            ));
        }
        if self.distribution.acquisition_profile_sha256 != TARGET_ACQUISITION_PROFILE_SHA256 {
            return Err(NuclearDataError::UnsupportedAcquisitionProfileHash(
                self.distribution.acquisition_profile_sha256.clone(),
            ));
        }
        validate_sha256(
            "distribution.acquisition_receipt_sha256",
            &self.distribution.acquisition_receipt_sha256,
        )?;
        if self.distribution.publisher_digest_status != PublisherDigestStatus::Unavailable {
            return Err(NuclearDataError::UnexpectedPublisherDigestStatus);
        }
        self.cross_sections.validate("cross_sections")?;

        if self.neutron_tables.is_empty() {
            return Err(NuclearDataError::EmptyTableSet("neutron_tables"));
        }
        if self.photon_tables.is_empty() {
            return Err(NuclearDataError::EmptyTableSet("photon_tables"));
        }
        if self
            .neutron_tables
            .windows(2)
            .any(|pair| pair[0].nuclide >= pair[1].nuclide)
        {
            return Err(NuclearDataError::NoncanonicalTableOrder("neutron_tables"));
        }
        if self
            .photon_tables
            .windows(2)
            .any(|pair| pair[0].element >= pair[1].element)
        {
            return Err(NuclearDataError::NoncanonicalTableOrder("photon_tables"));
        }

        for table in &self.neutron_tables {
            validate_identifier("neutron_table.nuclide", &table.nuclide)?;
            table.artifact.validate("neutron_table.artifact")?;
            if table.hdf5_version != TARGET_DATA_HDF5_VERSION {
                return Err(NuclearDataError::UnsupportedHdf5Version {
                    table: table.nuclide.clone(),
                    version: table.hdf5_version,
                });
            }
            if !table.atomic_weight_ratio.is_finite() || table.atomic_weight_ratio <= 0.0 {
                return Err(NuclearDataError::InvalidAtomicWeightRatio(
                    table.nuclide.clone(),
                ));
            }
            if table.temperatures_k.is_empty()
                || table
                    .temperatures_k
                    .iter()
                    .any(|temperature| !temperature.is_finite() || *temperature <= 0.0)
                || !strictly_increasing(&table.temperatures_k)
            {
                return Err(NuclearDataError::InvalidTemperatures(table.nuclide.clone()));
            }
            if table.energy_ranges_ev.len() != table.temperatures_k.len()
                || table.energy_ranges_ev.iter().any(|[lower, upper]| {
                    !lower.is_finite() || !upper.is_finite() || *lower < 0.0 || *lower >= *upper
                })
            {
                return Err(NuclearDataError::InvalidEnergyRanges(table.nuclide.clone()));
            }
            if !strictly_increasing(&table.reactions_mt)
                || !strictly_increasing(&table.photon_production_mts)
            {
                return Err(NuclearDataError::NoncanonicalMtOrder(table.nuclide.clone()));
            }
        }
        for table in &self.photon_tables {
            if !is_element_name(&table.element) {
                return Err(NuclearDataError::InvalidElement(table.element.clone()));
            }
            table.artifact.validate("photon_table.artifact")?;
            if table.hdf5_version != TARGET_DATA_HDF5_VERSION {
                return Err(NuclearDataError::UnsupportedHdf5Version {
                    table: table.element.clone(),
                    version: table.hdf5_version,
                });
            }
            if !strictly_increasing(&table.reactions_mt) {
                return Err(NuclearDataError::NoncanonicalPhotonMtOrder(
                    table.element.clone(),
                ));
            }
        }

        let mut artifacts = BTreeMap::new();
        for artifact in self.artifacts() {
            if let Some(existing) = artifacts.insert(&artifact.relative_path, &artifact.sha256)
                && existing != &artifact.sha256
            {
                return Err(NuclearDataError::ConflictingArtifactHash(
                    artifact.relative_path.clone(),
                ));
            }
        }

        Ok(())
    }

    /// Check that this exact data selection can represent the supplied material.
    pub fn validate_for_material(
        &self,
        material: &MaterialDefinition,
    ) -> Result<(), NuclearDataError> {
        self.validate()?;
        material
            .validate()
            .map_err(|error| NuclearDataError::InvalidMaterial(error.to_string()))?;
        self.validate_material_capabilities(material)
    }

    /// Check that this exact data selection can represent the supplied case.
    pub fn validate_for_case(&self, case: &TransportCase) -> Result<(), NuclearDataError> {
        self.validate()?;
        case.validate()
            .map_err(|error| NuclearDataError::InvalidTransportCase(error.to_string()))?;
        self.validate_material_capabilities(&case.material)
    }

    fn validate_material_capabilities(
        &self,
        material: &MaterialDefinition,
    ) -> Result<(), NuclearDataError> {
        let required_nuclides = material
            .nuclides
            .iter()
            .map(|nuclide| nuclide.name.as_str())
            .collect::<BTreeSet<_>>();
        let available_nuclides = self
            .neutron_tables
            .iter()
            .map(|table| table.nuclide.as_str())
            .collect::<BTreeSet<_>>();
        require_exact_set(
            &required_nuclides,
            &available_nuclides,
            NuclearDataError::MissingNuclide,
            NuclearDataError::UnexpectedNuclide,
        )?;

        for table in &self.neutron_tables {
            self.require_reaction(&table.nuclide, 301)?;
            selected_temperature_index(table, material.temperature_k)?;
        }
        self.require_reaction("B10", 107)?;
        self.require_reaction("N14", 103)?;
        self.require_photon_production("H1", 102)?;
        self.require_any_photon_production("B10", &B10_ALPHA_PHOTON_PRODUCTION_MTS)?;

        let required_elements = required_nuclides
            .iter()
            .map(|nuclide| element_from_nuclide(nuclide))
            .collect::<BTreeSet<_>>();
        let available_elements = self
            .photon_tables
            .iter()
            .map(|table| table.element.as_str())
            .collect::<BTreeSet<_>>();
        let required_element_refs = required_elements
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        require_exact_set(
            &required_element_refs,
            &available_elements,
            NuclearDataError::MissingPhotonElement,
            NuclearDataError::UnexpectedPhotonElement,
        )?;
        for table in &self.photon_tables {
            for mt in [502, 504, 522] {
                if table.reactions_mt.binary_search(&mt).is_err() {
                    return Err(NuclearDataError::MissingPhotonReaction {
                        element: table.element.clone(),
                        mt,
                    });
                }
            }
            if !table.has_atomic_relaxation_data {
                return Err(NuclearDataError::MissingAtomicRelaxationData(
                    table.element.clone(),
                ));
            }
            if !table.has_compton_profile_data {
                return Err(NuclearDataError::MissingComptonProfileData(
                    table.element.clone(),
                ));
            }
        }

        Ok(())
    }

    /// Return the energy interval OpenMC can transport for all selected
    /// neutron tables at the case material temperature.
    pub fn neutron_transport_energy_range_for_case(
        &self,
        case: &TransportCase,
    ) -> Result<[f64; 2], NuclearDataError> {
        case.validate()
            .map_err(|error| NuclearDataError::InvalidTransportCase(error.to_string()))?;
        self.neutron_transport_energy_range_for_material(&case.material)
    }

    /// Return the common energy interval exposed by every selected neutron
    /// table at the material temperature.
    pub fn neutron_transport_energy_range_for_material(
        &self,
        material: &MaterialDefinition,
    ) -> Result<[f64; 2], NuclearDataError> {
        self.validate_for_material(material)?;

        let mut common_lower = 0.0_f64;
        let mut common_upper = f64::INFINITY;
        for table in &self.neutron_tables {
            let index = selected_temperature_index(table, material.temperature_k)?;
            let [lower, upper] = table.energy_ranges_ev[index];
            common_lower = common_lower.max(lower);
            common_upper = common_upper.min(upper);
        }
        if common_lower >= common_upper {
            return Err(NuclearDataError::EmptyCommonNeutronEnergyRange {
                lower_ev: common_lower,
                upper_ev: common_upper,
            });
        }
        Ok([common_lower, common_upper])
    }

    /// Verify all selected files and cross-check their cross_sections.xml map.
    pub fn verify_files(&self, data_root: &Path) -> Result<(), NuclearDataError> {
        self.validate()?;
        let canonical_root =
            std::fs::canonicalize(data_root).map_err(|source| NuclearDataError::Io {
                path: data_root.to_path_buf(),
                source,
            })?;

        let mut resolved_artifacts = BTreeMap::new();
        for artifact in self.artifacts() {
            if resolved_artifacts.contains_key(&artifact.relative_path) {
                continue;
            }
            let resolved = verify_artifact(&canonical_root, artifact)?;
            resolved_artifacts.insert(artifact.relative_path.clone(), resolved);
        }

        let cross_sections_path = resolved_artifacts
            .get(&self.cross_sections.relative_path)
            .expect("validated cross-sections artifact is present");
        let xml = std::fs::read_to_string(cross_sections_path).map_err(|source| {
            NuclearDataError::Io {
                path: cross_sections_path.clone(),
                source,
            }
        })?;
        let listing: CrossSectionsListing = quick_xml::de::from_str(&xml)
            .map_err(|error| NuclearDataError::InvalidCrossSectionsXml(error.to_string()))?;
        let base = cross_sections_base(cross_sections_path, listing.directory.as_deref());

        for table in &self.neutron_tables {
            verify_library_mapping(
                &listing.libraries,
                &base,
                "neutron",
                &table.nuclide,
                resolved_artifacts
                    .get(&table.artifact.relative_path)
                    .expect("validated neutron artifact is present"),
            )?;
        }
        for table in &self.photon_tables {
            verify_library_mapping(
                &listing.libraries,
                &base,
                "photon",
                &table.element,
                resolved_artifacts
                    .get(&table.artifact.relative_path)
                    .expect("validated photon artifact is present"),
            )?;
        }

        Ok(())
    }

    fn require_reaction(&self, nuclide: &str, mt: u16) -> Result<(), NuclearDataError> {
        let table = self.table(nuclide)?;
        if table.reactions_mt.binary_search(&mt).is_err() {
            return Err(NuclearDataError::MissingReaction {
                nuclide: nuclide.into(),
                mt,
            });
        }
        Ok(())
    }

    fn require_photon_production(&self, nuclide: &str, mt: u16) -> Result<(), NuclearDataError> {
        let table = self.table(nuclide)?;
        if table.photon_production_mts.binary_search(&mt).is_err() {
            return Err(NuclearDataError::MissingPhotonProduction {
                nuclide: nuclide.into(),
                mt,
            });
        }
        Ok(())
    }

    fn require_any_photon_production(
        &self,
        nuclide: &str,
        mts: &[u16],
    ) -> Result<(), NuclearDataError> {
        let table = self.table(nuclide)?;
        if mts
            .iter()
            .any(|mt| table.photon_production_mts.binary_search(mt).is_ok())
        {
            return Ok(());
        }
        Err(NuclearDataError::MissingAlternativePhotonProduction {
            nuclide: nuclide.into(),
            mts: mts.to_vec(),
        })
    }

    fn table(&self, nuclide: &str) -> Result<&NeutronTableCapability, NuclearDataError> {
        self.neutron_tables
            .iter()
            .find(|table| table.nuclide == nuclide)
            .ok_or_else(|| NuclearDataError::MissingNuclide(nuclide.into()))
    }

    fn artifacts(&self) -> impl Iterator<Item = &DataArtifact> {
        std::iter::once(&self.cross_sections)
            .chain(self.neutron_tables.iter().map(|table| &table.artifact))
            .chain(self.photon_tables.iter().map(|table| &table.artifact))
    }
}

fn selected_temperature_index(
    table: &NeutronTableCapability,
    requested_k: f64,
) -> Result<usize, NuclearDataError> {
    let mut matches = table
        .temperatures_k
        .iter()
        .enumerate()
        .filter(|(_, available)| (**available - requested_k).abs() < TEMPERATURE_TOLERANCE_K)
        .map(|(index, _)| index);
    let Some(index) = matches.next() else {
        return Err(NuclearDataError::MissingTemperature {
            nuclide: table.nuclide.clone(),
            temperature_k: requested_k,
        });
    };
    if matches.next().is_some() {
        return Err(NuclearDataError::AmbiguousTemperature {
            nuclide: table.nuclide.clone(),
            temperature_k: requested_k,
        });
    }
    Ok(index)
}

impl DataArtifact {
    fn validate(&self, label: &'static str) -> Result<(), NuclearDataError> {
        if !is_normalized_relative_path(&self.relative_path) {
            return Err(NuclearDataError::InvalidArtifactPath(
                self.relative_path.clone(),
            ));
        }
        validate_sha256(label, &self.sha256)
    }
}

fn require_exact_set<'a, F, G>(
    required: &BTreeSet<&'a str>,
    available: &BTreeSet<&'a str>,
    missing: F,
    unexpected: G,
) -> Result<(), NuclearDataError>
where
    F: Fn(String) -> NuclearDataError,
    G: Fn(String) -> NuclearDataError,
{
    if let Some(value) = required.difference(available).next() {
        return Err(missing((*value).into()));
    }
    if let Some(value) = available.difference(required).next() {
        return Err(unexpected((*value).into()));
    }
    Ok(())
}

fn verify_artifact(
    canonical_root: &Path,
    artifact: &DataArtifact,
) -> Result<PathBuf, NuclearDataError> {
    let unresolved = canonical_root.join(&artifact.relative_path);
    let resolved = std::fs::canonicalize(&unresolved).map_err(|source| NuclearDataError::Io {
        path: unresolved,
        source,
    })?;
    if !resolved.starts_with(canonical_root) {
        return Err(NuclearDataError::ArtifactEscapesRoot(
            artifact.relative_path.clone(),
        ));
    }
    if !resolved
        .metadata()
        .map_err(|source| NuclearDataError::Io {
            path: resolved.clone(),
            source,
        })?
        .is_file()
    {
        return Err(NuclearDataError::ArtifactNotFile(
            artifact.relative_path.clone(),
        ));
    }
    let observed = sha256_file(&resolved).map_err(|source| NuclearDataError::Io {
        path: resolved.clone(),
        source,
    })?;
    if observed != artifact.sha256 {
        return Err(NuclearDataError::HashMismatch {
            path: artifact.relative_path.clone(),
            expected: artifact.sha256.clone(),
            observed,
        });
    }
    Ok(resolved)
}

#[derive(Debug, Deserialize)]
struct CrossSectionsListing {
    #[serde(default)]
    directory: Option<String>,
    #[serde(rename = "library", default)]
    libraries: Vec<CrossSectionsLibrary>,
}

#[derive(Debug, Deserialize)]
struct CrossSectionsLibrary {
    #[serde(rename = "@materials")]
    materials: String,
    #[serde(rename = "@path")]
    path: String,
    #[serde(rename = "@type")]
    library_type: String,
}

fn cross_sections_base(cross_sections: &Path, directory: Option<&str>) -> PathBuf {
    let parent = cross_sections.parent().unwrap_or_else(|| Path::new("."));
    match directory.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) if Path::new(value).is_absolute() => PathBuf::from(value),
        Some(value) => parent.join(value),
        None => parent.to_path_buf(),
    }
}

fn verify_library_mapping(
    libraries: &[CrossSectionsLibrary],
    base: &Path,
    library_type: &'static str,
    material: &str,
    expected_path: &Path,
) -> Result<(), NuclearDataError> {
    let mut matches = 0_usize;
    for library in libraries {
        if library.library_type != library_type
            || !library
                .materials
                .split_ascii_whitespace()
                .any(|listed| listed == material)
        {
            continue;
        }
        let listed = Path::new(&library.path);
        let unresolved = if listed.is_absolute() {
            listed.to_path_buf()
        } else {
            base.join(listed)
        };
        let resolved =
            std::fs::canonicalize(&unresolved).map_err(|source| NuclearDataError::Io {
                path: unresolved,
                source,
            })?;
        if resolved != expected_path {
            return Err(NuclearDataError::CrossSectionsPathMismatch {
                material: material.into(),
                expected: expected_path.to_path_buf(),
                observed: resolved,
            });
        }
        matches += 1;
    }
    if matches != 1 {
        return Err(NuclearDataError::CrossSectionsMappingCount {
            library_type,
            material: material.into(),
            count: matches,
        });
    }
    Ok(())
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), NuclearDataError> {
    if value.trim().is_empty() {
        Err(NuclearDataError::EmptyIdentifier(label))
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), NuclearDataError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(NuclearDataError::InvalidSha256(label))
    }
}

fn strictly_increasing<T: PartialOrd>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn is_normalized_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.contains('\\')
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_element_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes, [first] if first.is_ascii_uppercase())
        || matches!(bytes, [first, second] if first.is_ascii_uppercase() && second.is_ascii_lowercase())
}

fn element_from_nuclide(value: &str) -> String {
    let bytes = value.as_bytes();
    let length = if bytes.get(1).is_some_and(u8::is_ascii_lowercase) {
        2
    } else {
        1
    };
    value[..length].into()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Debug, Error)]
pub enum NuclearDataError {
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("{0} must be a canonical lowercase SHA-256 digest")]
    InvalidSha256(&'static str),
    #[error("nuclear-data manifest schema {0:?} is not supported")]
    UnsupportedManifestSchema(String),
    #[error("OpenMC version {0:?} is not supported by this adapter profile")]
    UnsupportedOpenMcVersion(String),
    #[error("OpenMC source commit {0:?} is not supported by this adapter profile")]
    UnsupportedOpenMcCommit(String),
    #[error("evaluated-data release {0:?} is not supported by this adapter profile")]
    UnsupportedEvaluatedDataRelease(String),
    #[error("nuclear-data inspection method {0:?} is not supported")]
    UnsupportedInspectionMethod(String),
    #[error("nuclear-data distribution URI {0:?} is not the frozen official source")]
    UnsupportedDistributionUri(String),
    #[error("nuclear-data archive size {0} does not match the frozen distribution")]
    UnsupportedArchiveSize(u64),
    #[error("nuclear-data acquisition profile ID {0:?} is unsupported")]
    UnsupportedAcquisitionProfileId(String),
    #[error("nuclear-data acquisition profile hash {0:?} is unsupported")]
    UnsupportedAcquisitionProfileHash(String),
    #[error("the frozen distribution profile has no publisher digest")]
    UnexpectedPublisherDigestStatus,
    #[error("artifact path is not normalized and relative: {0:?}")]
    InvalidArtifactPath(String),
    #[error("table {table} uses unsupported OpenMC HDF5 data version {version:?}")]
    UnsupportedHdf5Version { table: String, version: [u16; 2] },
    #[error("{0} is empty")]
    EmptyTableSet(&'static str),
    #[error("{0} must be strictly ordered without duplicates")]
    NoncanonicalTableOrder(&'static str),
    #[error("nuclide {0} has an invalid atomic-weight ratio")]
    InvalidAtomicWeightRatio(String),
    #[error("nuclide {0} has invalid or non-increasing temperatures")]
    InvalidTemperatures(String),
    #[error("nuclide {0} has invalid or temperature-misaligned neutron energy ranges")]
    InvalidEnergyRanges(String),
    #[error("nuclide {0} has unsorted or duplicate MT capabilities")]
    NoncanonicalMtOrder(String),
    #[error("element {0} has unsorted or duplicate photon MT capabilities")]
    NoncanonicalPhotonMtOrder(String),
    #[error("invalid element name {0:?}")]
    InvalidElement(String),
    #[error("artifact {0:?} is declared with conflicting hashes")]
    ConflictingArtifactHash(String),
    #[error("transport case is invalid: {0}")]
    InvalidTransportCase(String),
    #[error("material definition is invalid: {0}")]
    InvalidMaterial(String),
    #[error("required neutron table {0} is missing")]
    MissingNuclide(String),
    #[error("unrequested neutron table {0} is present in the case-scoped manifest")]
    UnexpectedNuclide(String),
    #[error("nuclide {nuclide} has no table within 0.5 K of {temperature_k} K")]
    MissingTemperature { nuclide: String, temperature_k: f64 },
    #[error("nuclide {nuclide} has multiple tables within 0.5 K of {temperature_k} K")]
    AmbiguousTemperature { nuclide: String, temperature_k: f64 },
    #[error(
        "selected neutron tables have no common transport energy range ({lower_ev} to {upper_ev} eV)"
    )]
    EmptyCommonNeutronEnergyRange { lower_ev: f64, upper_ev: f64 },
    #[error("nuclide {nuclide} lacks required reaction MT {mt}")]
    MissingReaction { nuclide: String, mt: u16 },
    #[error("nuclide {nuclide} lacks transported photon production for MT {mt}")]
    MissingPhotonProduction { nuclide: String, mt: u16 },
    #[error(
        "nuclide {nuclide} lacks transported photon production for every acceptable MT in {mts:?}"
    )]
    MissingAlternativePhotonProduction { nuclide: String, mts: Vec<u16> },
    #[error("required photon table for element {0} is missing")]
    MissingPhotonElement(String),
    #[error("unrequested photon table for element {0} is present in the case-scoped manifest")]
    UnexpectedPhotonElement(String),
    #[error("element {element} lacks required photon reaction MT {mt}")]
    MissingPhotonReaction { element: String, mt: u16 },
    #[error("element {0} lacks atomic-relaxation data")]
    MissingAtomicRelaxationData(String),
    #[error("element {0} lacks Compton-profile data")]
    MissingComptonProfileData(String),
    #[error("artifact path escapes the nuclear-data root: {0}")]
    ArtifactEscapesRoot(String),
    #[error("artifact is not a regular file: {0}")]
    ArtifactNotFile(String),
    #[error("artifact {path} hash mismatch: expected {expected}, observed {observed}")]
    HashMismatch {
        path: String,
        expected: String,
        observed: String,
    },
    #[error("cross_sections.xml is invalid: {0}")]
    InvalidCrossSectionsXml(String),
    #[error(
        "cross_sections.xml has {count} {library_type} mappings for {material}; expected exactly one"
    )]
    CrossSectionsMappingCount {
        library_type: &'static str,
        material: String,
        count: usize,
    },
    #[error("cross_sections.xml maps {material} to {observed:?}; manifest requires {expected:?}")]
    CrossSectionsPathMismatch {
        material: String,
        expected: PathBuf,
        observed: PathBuf,
    },
    #[error("I/O operation failed for {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use openbnct_core::GridGeometry;
    use openbnct_transport::{FixedSourceDefinition, MaterialDefinition};

    use super::*;

    const MATERIAL_JSON: &str =
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const SOURCE_JSON: &str =
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/source.json");
    const OFFICIAL_PROCESSED_MANIFEST_JSON: &str = include_str!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json"
    );

    fn case() -> TransportCase {
        TransportCase {
            schema_version: "openbnct.transport-case/0.1.0".into(),
            case_id: "nf-bnct-001".into(),
            geometry: GridGeometry {
                shape: [40; 3],
                spacing_mm: [5.0; 3],
                origin_mm: [-97.5; 3],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            material: serde_json::from_str::<MaterialDefinition>(MATERIAL_JSON).unwrap(),
            source: serde_json::from_str::<FixedSourceDefinition>(SOURCE_JSON).unwrap(),
            requested_histories: 1_000,
        }
    }

    fn artifact(path: impl Into<String>) -> DataArtifact {
        DataArtifact {
            relative_path: path.into(),
            sha256: "a".repeat(64),
        }
    }

    fn manifest() -> NuclearDataManifest {
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
            .map(|nuclide| element_from_nuclide(&nuclide.name))
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
            schema_version: TARGET_NUCLEAR_DATA_MANIFEST_SCHEMA.into(),
            id: "openbnct.nf-bnct-001.endf-b-viii.1.v1".into(),
            openmc_version: TARGET_OPENMC_VERSION.into(),
            openmc_source_commit: TARGET_OPENMC_SOURCE_COMMIT.into(),
            evaluated_data_release: TARGET_EVALUATED_DATA_RELEASE.into(),
            inspection: DataInspectionIdentity {
                method: TARGET_INSPECTION_METHOD.into(),
                source_sha256: "c".repeat(64),
                python_version: "3.13.7".into(),
                numpy_version: "2.5.2".into(),
                h5py_version: "3.14.0".into(),
                hdf5_library_version: "1.14.6".into(),
            },
            distribution: DataDistributionIdentity {
                id: "openmc-endf-b-viii.1".into(),
                source_uri: TARGET_DISTRIBUTION_SOURCE_URI.into(),
                archive_size_bytes: TARGET_DISTRIBUTION_ARCHIVE_SIZE_BYTES,
                archive_sha256: "b".repeat(64),
                acquisition_profile_id: TARGET_ACQUISITION_PROFILE_ID.into(),
                acquisition_profile_sha256: TARGET_ACQUISITION_PROFILE_SHA256.into(),
                acquisition_receipt_sha256: "e".repeat(64),
                publisher_digest_status: PublisherDigestStatus::Unavailable,
                acquisition_evidence_state: AcquisitionEvidenceState::AcquisitionOnly,
            },
            cross_sections: artifact("cross_sections.xml"),
            neutron_tables,
            photon_tables,
        }
    }

    #[test]
    fn accepts_complete_case_scoped_capability_manifest() {
        assert!(manifest().validate_for_case(&case()).is_ok());
    }

    #[test]
    fn official_processed_manifest_passes_bnct_transport_capabilities() {
        let manifest: NuclearDataManifest =
            serde_json::from_str(OFFICIAL_PROCESSED_MANIFEST_JSON).unwrap();
        assert!(manifest.validate().is_ok());
        assert!(manifest.validate_for_material(&case().material).is_ok());
    }

    #[test]
    fn rejects_self_asserted_acquisition_provenance() {
        let mut wrong_hash_manifest = manifest();
        wrong_hash_manifest.distribution.acquisition_profile_sha256 = "0".repeat(64);
        assert!(matches!(
            wrong_hash_manifest.validate(),
            Err(NuclearDataError::UnsupportedAcquisitionProfileHash(_))
        ));

        let mut wrong_status_manifest = manifest();
        wrong_status_manifest.distribution.publisher_digest_status = PublisherDigestStatus::Matched;
        assert!(matches!(
            wrong_status_manifest.validate(),
            Err(NuclearDataError::UnexpectedPublisherDigestStatus)
        ));
    }

    #[test]
    fn rejects_missing_reaction_capability() {
        let mut manifest = manifest();
        manifest
            .neutron_tables
            .iter_mut()
            .find(|table| table.nuclide == "B10")
            .unwrap()
            .reactions_mt
            .remove(0);

        assert!(matches!(
            manifest.validate_for_case(&case()),
            Err(NuclearDataError::MissingReaction {
                nuclide,
                mt: 107
            }) if nuclide == "B10"
        ));
    }

    #[test]
    fn rejects_missing_neutron_heating_capability() {
        let mut manifest = manifest();
        manifest
            .neutron_tables
            .iter_mut()
            .find(|table| table.nuclide == "C12")
            .unwrap()
            .reactions_mt
            .clear();

        assert!(matches!(
            manifest.validate_for_case(&case()),
            Err(NuclearDataError::MissingReaction {
                nuclide,
                mt: 301
            }) if nuclide == "C12"
        ));
    }

    #[test]
    fn returns_common_selected_neutron_energy_range() {
        let mut manifest = manifest();
        manifest.neutron_tables[0].energy_ranges_ev[0] = [2.0e-5, 19.0e6];

        assert_eq!(
            manifest
                .neutron_transport_energy_range_for_material(&case().material)
                .unwrap(),
            [2.0e-5, 19.0e6]
        );
        assert_eq!(
            manifest
                .neutron_transport_energy_range_for_case(&case())
                .unwrap(),
            [2.0e-5, 19.0e6]
        );
    }

    #[test]
    fn rejects_missing_secondary_photon_capability() {
        let mut manifest = manifest();
        manifest
            .neutron_tables
            .iter_mut()
            .find(|table| table.nuclide == "H1")
            .unwrap()
            .photon_production_mts
            .clear();

        assert!(matches!(
            manifest.validate_for_case(&case()),
            Err(NuclearDataError::MissingPhotonProduction {
                nuclide,
                mt: 102
            }) if nuclide == "H1"
        ));
    }

    #[test]
    fn rejects_missing_b10_alpha_branch_photon_capability() {
        let mut manifest = manifest();
        manifest
            .neutron_tables
            .iter_mut()
            .find(|table| table.nuclide == "B10")
            .unwrap()
            .photon_production_mts
            .clear();

        assert!(matches!(
            manifest.validate_for_case(&case()),
            Err(NuclearDataError::MissingAlternativePhotonProduction { nuclide, mts })
                if nuclide == "B10" && mts == vec![107, 801]
        ));
    }

    #[test]
    fn rejects_incomplete_photoatomic_capability() {
        let mut manifest = manifest();
        manifest.photon_tables[0].reactions_mt.remove(1);
        let element = manifest.photon_tables[0].element.clone();

        assert!(matches!(
            manifest.validate_for_case(&case()),
            Err(NuclearDataError::MissingPhotonReaction {
                element: observed,
                mt: 504
            }) if observed == element
        ));
    }

    #[test]
    fn verifies_hashes_and_cross_sections_mappings() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        std::fs::create_dir(root.join("neutron")).unwrap();
        std::fs::create_dir(root.join("photon")).unwrap();

        let mut manifest = manifest();
        let mut libraries = String::new();
        for table in &mut manifest.neutron_tables {
            let path = root.join(&table.artifact.relative_path);
            std::fs::write(&path, table.nuclide.as_bytes()).unwrap();
            table.artifact.sha256 = sha256_file(&path).unwrap();
            libraries.push_str(&format!(
                "  <library materials=\"{}\" path=\"{}\" type=\"neutron\"/>\n",
                table.nuclide, table.artifact.relative_path
            ));
        }
        for table in &mut manifest.photon_tables {
            let path = root.join(&table.artifact.relative_path);
            std::fs::write(&path, table.element.as_bytes()).unwrap();
            table.artifact.sha256 = sha256_file(&path).unwrap();
            libraries.push_str(&format!(
                "  <library materials=\"{}\" path=\"{}\" type=\"photon\"/>\n",
                table.element, table.artifact.relative_path
            ));
        }
        let cross_sections = format!("<cross_sections>\n{libraries}</cross_sections>\n");
        let cross_sections_path = root.join("cross_sections.xml");
        std::fs::write(&cross_sections_path, cross_sections).unwrap();
        manifest.cross_sections.sha256 = sha256_file(&cross_sections_path).unwrap();

        assert!(manifest.verify_files(root).is_ok());

        std::fs::write(root.join("neutron/B10.h5"), b"tampered").unwrap();
        assert!(matches!(
            manifest.verify_files(root),
            Err(NuclearDataError::HashMismatch { path, .. }) if path == "neutron/B10.h5"
        ));
    }
}
