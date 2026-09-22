// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use openbnct_core::GridGeometry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

mod analytic_oracle;
mod bundle;
mod compare;
mod dvh;
mod gamma;
mod limits;
mod metamorphic;
mod metrics;
mod pk;

pub use analytic_oracle::{
    ANALYTIC_EVALUATION_SCHEMA, ANALYTIC_ORACLE_SCHEMA, AnalyticLaw, AnalyticOracle,
    AnalyticOracleEvaluation, evaluate_analytic_oracle,
};
pub use bundle::{
    BundleInput, EVIDENCE_BUNDLE_MANIFEST_NAME, EVIDENCE_BUNDLE_MANIFEST_SCHEMA,
    EvidenceBundleManifest, export_evidence_bundle,
};
pub use compare::{
    ComparisonInput, DOSE_COMPARISON_SCHEMA, DoseComparison, QuantityAgreement,
    compare_dose_bundles,
};
pub use dvh::{DVH_SCHEMA, DoseVolumeHistogram};
pub use gamma::{
    GAMMA_EVALUATION_SCHEMA, GammaCriteria, GammaEvaluation, GammaNormalization,
    GammaQuantityResult, evaluate_gamma,
};
pub use limits::{
    IRRADIATION_TIME_SCHEMA, IrradiationTimeReport, LimitMetric, LimitingStructure, OrganLimit,
    RegionLimitResult,
};
pub use metamorphic::{
    METAMORPHIC_SCHEMA, MetamorphicEvaluation, MetamorphicOracle, OracleQuantity,
    evaluate_metamorphic,
};
pub use metrics::{
    CoverageMetric, DOSE_METRICS_SCHEMA, EudMetric, RegionDoseMetrics, VolumeMetric,
};
pub use pk::{
    PK_IRRADIATION_SCHEMA, PK_MODEL_SCHEMA, PkIrradiationReport, PkModel, PkRegion, PkRegionResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecord {
    pub role: String,
    pub path: String,
    pub sha256: String,
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatientCoordinateSystem {
    DicomLpsMillimeters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureRecord {
    pub number: i32,
    pub name: String,
    pub voxel_count: usize,
    pub volume_cm3: f64,
    pub centroid_lps_mm: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaseManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub qualification: QualificationBoundary,
    pub coordinate_system: PatientCoordinateSystem,
    pub geometry: GridGeometry,
    pub frame_of_reference_uid: String,
    pub study_instance_uid: String,
    pub imaging_series_instance_uid: String,
    pub structure_set_series_instance_uid: String,
    pub structure_set_instance_uid: String,
    pub material_model_id: String,
    pub source_model_id: String,
    pub structures: Vec<StructureRecord>,
    pub artifacts: Vec<ArtifactRecord>,
}

impl CaseManifest {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema_version.is_empty() {
            return Err(ManifestError::Invalid("schema_version is empty".into()));
        }
        for (label, value) in [
            ("case_id", self.case_id.as_str()),
            (
                "frame_of_reference_uid",
                self.frame_of_reference_uid.as_str(),
            ),
            ("study_instance_uid", self.study_instance_uid.as_str()),
            (
                "imaging_series_instance_uid",
                self.imaging_series_instance_uid.as_str(),
            ),
            (
                "structure_set_series_instance_uid",
                self.structure_set_series_instance_uid.as_str(),
            ),
            (
                "structure_set_instance_uid",
                self.structure_set_instance_uid.as_str(),
            ),
            ("material_model_id", self.material_model_id.as_str()),
            ("source_model_id", self.source_model_id.as_str()),
        ] {
            if value.is_empty() {
                return Err(ManifestError::Invalid(format!("{label} is empty")));
            }
        }
        self.geometry
            .voxel_count()
            .map_err(|error| ManifestError::Invalid(format!("invalid geometry: {error}")))?;

        let mut roi_numbers = BTreeSet::new();
        let mut roi_names = BTreeSet::new();
        for structure in &self.structures {
            if !roi_numbers.insert(structure.number) {
                return Err(ManifestError::Invalid(format!(
                    "duplicate structure number {}",
                    structure.number
                )));
            }
            if structure.name.is_empty() || !roi_names.insert(structure.name.as_str()) {
                return Err(ManifestError::Invalid(format!(
                    "empty or duplicate structure name {:?}",
                    structure.name
                )));
            }
            if structure.voxel_count == 0
                || !structure.volume_cm3.is_finite()
                || structure.volume_cm3 <= 0.0
                || structure
                    .centroid_lps_mm
                    .iter()
                    .any(|value| !value.is_finite())
            {
                return Err(ManifestError::Invalid(format!(
                    "structure {:?} has invalid mask statistics",
                    structure.name
                )));
            }
        }

        if self.artifacts.is_empty() {
            return Err(ManifestError::Invalid("artifacts are empty".into()));
        }
        let mut artifact_paths = BTreeSet::new();
        for artifact in &self.artifacts {
            if artifact.role.is_empty() {
                return Err(ManifestError::Invalid(format!(
                    "artifact {:?} has an empty role",
                    artifact.path
                )));
            }
            validate_relative_path(&artifact.path)?;
            if !artifact_paths.insert(artifact.path.as_str()) {
                return Err(ManifestError::Invalid(format!(
                    "duplicate artifact path {:?}",
                    artifact.path
                )));
            }
            if artifact.sha256.len() != 64
                || !artifact
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(ManifestError::Invalid(format!(
                    "artifact {:?} has an invalid lowercase SHA-256 digest",
                    artifact.path
                )));
            }
        }
        Ok(())
    }

    /// Verify every declared artifact without permitting symlink escape from
    /// the supplied evidence root.
    pub fn verify_artifacts(&self, root: &Path) -> Result<(), ManifestError> {
        self.validate()?;
        let canonical_root = std::fs::canonicalize(root).map_err(|source| ManifestError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        for artifact in &self.artifacts {
            let unresolved = root.join(&artifact.path);
            let resolved =
                std::fs::canonicalize(&unresolved).map_err(|source| ManifestError::Io {
                    path: unresolved.clone(),
                    source,
                })?;
            if !resolved.starts_with(&canonical_root) {
                return Err(ManifestError::ArtifactEscapesRoot(artifact.path.clone()));
            }
            if !resolved
                .metadata()
                .map_err(|source| ManifestError::Io {
                    path: resolved.clone(),
                    source,
                })?
                .is_file()
            {
                return Err(ManifestError::Invalid(format!(
                    "artifact {:?} is not a regular file",
                    artifact.path
                )));
            }
            let observed = sha256_file(&resolved).map_err(|source| ManifestError::Io {
                path: resolved,
                source,
            })?;
            if observed != artifact.sha256 {
                return Err(ManifestError::HashMismatch {
                    path: artifact.path.clone(),
                    expected: artifact.sha256.clone(),
                    observed,
                });
            }
        }
        Ok(())
    }

    /// Byte-map variant of [`Self::verify_artifacts`] for hosts without a
    /// filesystem (web drops, archive extraction). `files` keys are the
    /// manifest-relative paths (`ct/ct-000.dcm`, `rtstruct.dcm`, …); the
    /// same digest gates apply, and absence of a declared path is an error.
    pub fn verify_artifacts_map(
        &self,
        files: &std::collections::BTreeMap<String, &[u8]>,
    ) -> Result<(), ManifestError> {
        self.validate()?;
        for artifact in &self.artifacts {
            let bytes = files.get(artifact.path.as_str()).ok_or_else(|| {
                ManifestError::Invalid(format!(
                    "declared artifact {:?} is absent from the supplied file set",
                    artifact.path
                ))
            })?;
            let observed = sha256_hex(bytes);
            if observed != artifact.sha256 {
                return Err(ManifestError::HashMismatch {
                    path: artifact.path.clone(),
                    expected: artifact.sha256.clone(),
                    observed,
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub run_id: String,
    pub case_id: String,
    pub backend_id: String,
    pub backend_version: String,
    pub nuclear_data_id: String,
    pub artifacts: Vec<ArtifactRecord>,
    pub qualification: QualificationBoundary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationBoundary {
    SyntheticResearchOnly,
    CrossCodeResearchOnly,
    ExperimentallyValidatedResearchOnly,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("invalid manifest: {0}")]
    Invalid(String),
    #[error("manifest artifact path escapes the evidence root: {0}")]
    ArtifactEscapesRoot(String),
    #[error("artifact {path} hash mismatch: expected {expected}, observed {observed}")]
    HashMismatch {
        path: String,
        expected: String,
        observed: String,
    },
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
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

fn validate_relative_path(path: &str) -> Result<(), ManifestError> {
    let path_value = Path::new(path);
    if path.is_empty()
        || path_value.is_absolute()
        || path_value.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
                    | Component::CurDir
            )
        })
    {
        return Err(ManifestError::Invalid(format!(
            "artifact path must be normalized and relative: {path:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [1, 1, 1],
            spacing_mm: [1.0; 3],
            origin_mm: [0.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn case_manifest(artifact_path: &str, artifact_hash: String) -> CaseManifest {
        CaseManifest {
            schema_version: "test".into(),
            case_id: "test".into(),
            qualification: QualificationBoundary::SyntheticResearchOnly,
            coordinate_system: PatientCoordinateSystem::DicomLpsMillimeters,
            geometry: geometry(),
            frame_of_reference_uid: "2.25.1".into(),
            study_instance_uid: "2.25.2".into(),
            imaging_series_instance_uid: "2.25.3".into(),
            structure_set_series_instance_uid: "2.25.4".into(),
            structure_set_instance_uid: "2.25.5".into(),
            material_model_id: "material".into(),
            source_model_id: "source".into(),
            structures: vec![StructureRecord {
                number: 1,
                name: "ROI".into(),
                voxel_count: 1,
                volume_cm3: 0.001,
                centroid_lps_mm: [0.0; 3],
            }],
            artifacts: vec![ArtifactRecord {
                role: "input".into(),
                path: artifact_path.into(),
                sha256: artifact_hash,
                media_type: None,
            }],
        }
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(
            sha256_hex(b"NCTForge"),
            "c2408160f40e5432661a0c32e6b6c133c18109af762769e5ca989341fda04961"
        );
    }

    #[test]
    fn case_manifest_rejects_path_traversal() {
        let manifest = case_manifest("../outside", "0".repeat(64));

        assert!(matches!(
            manifest.validate(),
            Err(ManifestError::Invalid(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn artifact_verification_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("temporary directory");
        let root = temp.path().join("case");
        std::fs::create_dir(&root).expect("case root");
        let outside = temp.path().join("outside.dcm");
        std::fs::write(&outside, b"outside").expect("outside artifact");
        symlink(&outside, root.join("linked.dcm")).expect("artifact symlink");
        let manifest = case_manifest("linked.dcm", sha256_hex(b"outside"));

        assert!(matches!(
            manifest.verify_artifacts(&root),
            Err(ManifestError::ArtifactEscapesRoot(path)) if path == "linked.dcm"
        ));
    }
}
