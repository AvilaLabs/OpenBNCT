// SPDX-License-Identifier: MIT

//! Content-addressed neutron transport-domain evidence.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_transport::MaterialDefinition;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    NuclearDataError, NuclearDataManifest, TARGET_OPENMC_SOURCE_COMMIT, TARGET_OPENMC_VERSION,
};

pub const OPENMC_NEUTRON_TRANSPORT_DOMAIN_SCHEMA: &str =
    "openbnct.openmc-neutron-transport-domain/0.1.0";

const DOMAIN_ID_SUFFIX: &str = "neutron-transport-domain";

/// The common incident-neutron interval supported by every table selected for
/// one exact material and OpenMC nuclear-data manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcNeutronTransportDomain {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub backend_id: String,
    pub openmc_version: String,
    pub openmc_source_commit: String,
    pub material: ContentReference,
    pub nuclear_data_manifest: ContentReference,
    pub energy_range_ev: [f64; 2],
    pub derivation: OpenMcTransportDomainDerivation,
    pub diagnostic_boundary_policy: OpenMcDiagnosticBoundaryPolicy,
    pub qualification: OpenMcTransportDomainQualification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcTransportDomainDerivation {
    CommonSelectedNuclideTemperatureGridIntersection,
}

/// Diagnostic findings at either data-grid endpoint remain in scope. This is
/// deliberately conservative at the upper boundary: only energies strictly
/// above the common OpenMC interval can be classified out of domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcDiagnosticBoundaryPolicy {
    ClosedConservative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenMcTransportDomainQualification {
    BackendCapabilityDerivedUnreviewed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcNeutronTransportDomainDocument {
    pub domain: OpenMcNeutronTransportDomain,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenMcNeutronTransportDomainResult {
    pub domain: OpenMcNeutronTransportDomain,
    pub domain_path: PathBuf,
    pub domain_sha256: String,
}

impl OpenMcNeutronTransportDomain {
    /// Derive the domain from exact serialized inputs so both bindings retain
    /// their byte-level identities.
    pub fn derive(
        nuclear_data_manifest_bytes: &[u8],
        material_bytes: &[u8],
    ) -> Result<Self, OpenMcTransportDomainError> {
        let manifest: NuclearDataManifest =
            parse_json("nuclear_data_manifest", nuclear_data_manifest_bytes)?;
        let material: MaterialDefinition = parse_json("material", material_bytes)?;
        let energy_range_ev = manifest.neutron_transport_energy_range_for_material(&material)?;

        let domain = Self {
            schema_version: OPENMC_NEUTRON_TRANSPORT_DOMAIN_SCHEMA.into(),
            id: format!("{}.{}.{}", manifest.id, material.id, DOMAIN_ID_SUFFIX),
            backend_id: "openmc".into(),
            openmc_version: manifest.openmc_version.clone(),
            openmc_source_commit: manifest.openmc_source_commit.clone(),
            material: content_reference(&material.id, material_bytes),
            nuclear_data_manifest: content_reference(&manifest.id, nuclear_data_manifest_bytes),
            energy_range_ev,
            derivation:
                OpenMcTransportDomainDerivation::CommonSelectedNuclideTemperatureGridIntersection,
            diagnostic_boundary_policy: OpenMcDiagnosticBoundaryPolicy::ClosedConservative,
            qualification: OpenMcTransportDomainQualification::BackendCapabilityDerivedUnreviewed,
        };
        domain.validate()?;
        Ok(domain)
    }

    pub fn validate(&self) -> Result<(), OpenMcTransportDomainError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            OPENMC_NEUTRON_TRANSPORT_DOMAIN_SCHEMA,
        ) {
            return invalid_domain(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("material.id", &self.material.id)?;
        validate_sha256("material.sha256", &self.material.sha256)?;
        validate_identifier("nuclear_data_manifest.id", &self.nuclear_data_manifest.id)?;
        validate_sha256(
            "nuclear_data_manifest.sha256",
            &self.nuclear_data_manifest.sha256,
        )?;
        if self.id
            != format!(
                "{}.{}.{}",
                self.nuclear_data_manifest.id, self.material.id, DOMAIN_ID_SUFFIX
            )
        {
            return invalid_domain("domain ID does not bind the manifest and material");
        }
        if self.backend_id != "openmc" {
            return invalid_domain("transport-domain backend must be openmc");
        }
        if self.openmc_version != TARGET_OPENMC_VERSION {
            return invalid_domain("transport domain has an unsupported OpenMC version");
        }
        if self.openmc_source_commit != TARGET_OPENMC_SOURCE_COMMIT {
            return invalid_domain("transport domain has an unsupported OpenMC source commit");
        }
        let [lower, upper] = self.energy_range_ev;
        if !lower.is_finite() || !upper.is_finite() || lower < 0.0 || lower >= upper {
            return invalid_domain("transport-domain energy interval is invalid");
        }
        Ok(())
    }

    /// Test the conservative closed interval used only for classifying
    /// processor diagnostics. OpenMC run preparation retains its own source-
    /// energy endpoint rules.
    #[must_use]
    pub fn contains_diagnostic_energy(&self, energy_ev: f64) -> bool {
        let [lower, upper] = self.energy_range_ev;
        energy_ev.is_finite() && energy_ev >= lower && energy_ev <= upper
    }

    pub fn write_new(
        &self,
        path: &Path,
    ) -> Result<OpenMcNeutronTransportDomainResult, OpenMcTransportDomainError> {
        self.validate()?;
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|source| OpenMcTransportDomainError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| OpenMcTransportDomainError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(OpenMcNeutronTransportDomainResult {
            domain: self.clone(),
            domain_path: path.to_path_buf(),
            domain_sha256: sha256_bytes(&bytes),
        })
    }
}

impl OpenMcNeutronTransportDomainDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, OpenMcTransportDomainError> {
        let domain: OpenMcNeutronTransportDomain = serde_json::from_slice(bytes)?;
        domain.validate()?;
        Ok(Self {
            domain,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, OpenMcTransportDomainError> {
        Self::from_bytes(&read_regular_file(path)?)
    }

    pub fn verify_against_inputs(
        &self,
        nuclear_data_manifest_bytes: &[u8],
        material_bytes: &[u8],
    ) -> Result<(), OpenMcTransportDomainError> {
        let observed =
            OpenMcNeutronTransportDomain::derive(nuclear_data_manifest_bytes, material_bytes)?;
        if self.domain != observed {
            return Err(OpenMcTransportDomainError::DerivationMismatch);
        }
        Ok(())
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(
    artifact: &'static str,
    bytes: &[u8],
) -> Result<T, OpenMcTransportDomainError> {
    serde_json::from_slice(bytes)
        .map_err(|source| OpenMcTransportDomainError::InvalidJson { artifact, source })
}

fn content_reference(id: &str, bytes: &[u8]) -> ContentReference {
    ContentReference {
        id: id.into(),
        sha256: sha256_bytes(bytes),
    }
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), OpenMcTransportDomainError> {
    if value.trim().is_empty() {
        return invalid_domain(format!("{label} must not be empty"));
    }
    Ok(())
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), OpenMcTransportDomainError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_domain(format!("{label} is not a lowercase SHA-256 digest"));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, OpenMcTransportDomainError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OpenMcTransportDomainError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(OpenMcTransportDomainError::NotRegularFile(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| OpenMcTransportDomainError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn invalid_domain<T>(message: impl Into<String>) -> Result<T, OpenMcTransportDomainError> {
    Err(OpenMcTransportDomainError::InvalidDomain(message.into()))
}

#[derive(Debug, Error)]
pub enum OpenMcTransportDomainError {
    #[error(transparent)]
    NuclearData(#[from] NuclearDataError),
    #[error("invalid JSON for {artifact}: {source}")]
    InvalidJson {
        artifact: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid OpenMC neutron transport domain: {0}")]
    InvalidDomain(String),
    #[error("OpenMC neutron transport domain does not match regenerated inputs")]
    DerivationMismatch,
    #[error("required OpenMC transport-domain artifact is not a regular file: {0}")]
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

    const MATERIAL: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const MANIFEST: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json"
    );
    const FROZEN_DOMAIN: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-neutron-transport-domain.json"
    );

    #[test]
    fn derives_closed_common_case_domain_from_exact_inputs() {
        let domain = OpenMcNeutronTransportDomain::derive(MANIFEST, MATERIAL).unwrap();
        assert_eq!(domain.energy_range_ev, [9.999_999_999_999_999e-6, 20.0e6]);
        assert!(domain.contains_diagnostic_energy(20.0e6));
        assert!(!domain.contains_diagnostic_energy(20.0e6 + 1.0));
        assert_eq!(
            domain.material.sha256,
            "096e236d234acabc18f3027ae53be3c94f5608a86a1dec866cefc8bb330db813"
        );
        assert_eq!(
            domain.nuclear_data_manifest.sha256,
            "3eaae09921172199c34f3fb236ae082ea5ace4567e0e04d2afcce357add73fb1"
        );
    }

    #[test]
    fn detects_modified_derivation_inputs() {
        let domain = OpenMcNeutronTransportDomain::derive(MANIFEST, MATERIAL).unwrap();
        let bytes = serde_json::to_vec(&domain).unwrap();
        let document = OpenMcNeutronTransportDomainDocument::from_bytes(&bytes).unwrap();
        let mut material: serde_json::Value = serde_json::from_slice(MATERIAL).unwrap();
        material["density_g_cm3"] = serde_json::json!(1.01);
        let changed = serde_json::to_vec(&material).unwrap();
        assert!(matches!(
            document.verify_against_inputs(MANIFEST, &changed),
            Err(OpenMcTransportDomainError::DerivationMismatch)
        ));
    }

    #[test]
    fn verifies_frozen_domain_against_exact_inputs() {
        let document = OpenMcNeutronTransportDomainDocument::from_bytes(FROZEN_DOMAIN).unwrap();
        assert_eq!(
            document.sha256,
            "1554dfb3167c0aa804cd6c893ce22a363cefbc0cba1b8f7781eeae1c2dccf89e"
        );
        document.verify_against_inputs(MANIFEST, MATERIAL).unwrap();
    }
}
