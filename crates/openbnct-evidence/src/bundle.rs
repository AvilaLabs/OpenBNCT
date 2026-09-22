// SPDX-License-Identifier: MIT

//! Deterministic evidence-bundle export.
//!
//! An evidence bundle is a fresh directory whose every payload file is
//! recorded in `artifact-manifest.json` by relative path and SHA-256, under a
//! declared `QualificationBoundary`. The manifest is written last so a bundle
//! on disk is complete and verifiable as soon as the manifest exists.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{ArtifactRecord, ManifestError, QualificationBoundary, sha256_file};

pub const EVIDENCE_BUNDLE_MANIFEST_SCHEMA: &str = "openbnct.evidence-bundle-manifest/0.1.0";
pub const EVIDENCE_BUNDLE_MANIFEST_NAME: &str = "artifact-manifest.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceBundleManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub qualification: QualificationBoundary,
    pub artifacts: Vec<ArtifactRecord>,
}

impl EvidenceBundleManifest {
    /// Load and hash-check every artifact under `root`.
    pub fn load_verified(root: &Path) -> Result<Self, ManifestError> {
        let manifest_path = root.join(EVIDENCE_BUNDLE_MANIFEST_NAME);
        let bytes = fs::read(&manifest_path).map_err(|source| ManifestError::Io {
            path: manifest_path,
            source,
        })?;
        let manifest: Self = serde_json::from_slice(&bytes)
            .map_err(|e| ManifestError::Invalid(format!("bundle manifest: {e}")))?;
        manifest.validate()?;
        verify_artifact_hashes(&manifest.artifacts, root)?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, EVIDENCE_BUNDLE_MANIFEST_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported evidence-bundle schema {:?}",
                self.schema_version
            )));
        }
        if self.case_id.trim().is_empty() {
            return Err(ManifestError::Invalid("bundle case_id is empty".into()));
        }
        validate_records(&self.artifacts)
    }
}

fn validate_records(artifacts: &[ArtifactRecord]) -> Result<(), ManifestError> {
    for artifact in artifacts {
        if artifact.role.trim().is_empty() || artifact.path.trim().is_empty() {
            return Err(ManifestError::Invalid(
                "artifact role and path must be non-empty".into(),
            ));
        }
        let path = Path::new(&artifact.path);
        if path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(ManifestError::ArtifactEscapesRoot(artifact.path.clone()));
        }
        if artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(ManifestError::Invalid(format!(
                "artifact {:?} has an invalid lowercase SHA-256 digest",
                artifact.path
            )));
        }
    }
    Ok(())
}

fn verify_artifact_hashes(artifacts: &[ArtifactRecord], root: &Path) -> Result<(), ManifestError> {
    let canonical_root = fs::canonicalize(root).map_err(|source| ManifestError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    for artifact in artifacts {
        let resolved =
            fs::canonicalize(root.join(&artifact.path)).map_err(|source| ManifestError::Io {
                path: root.join(&artifact.path),
                source,
            })?;
        if !resolved.starts_with(&canonical_root) {
            return Err(ManifestError::ArtifactEscapesRoot(artifact.path.clone()));
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

/// One file to place in the bundle: source path on disk plus the
/// bundle-relative destination path.
pub struct BundleInput {
    pub role: String,
    pub source: PathBuf,
    pub relative_path: String,
    pub media_type: Option<String>,
}

/// Copy `inputs` into a new directory `root` and freeze an
/// `artifact-manifest.json` over them. The root must not already exist;
/// destination paths may not escape it; the manifest excludes itself.
pub fn export_evidence_bundle(
    root: &Path,
    case_id: &str,
    qualification: QualificationBoundary,
    inputs: &[BundleInput],
) -> Result<EvidenceBundleManifest, ManifestError> {
    if inputs.is_empty() {
        return Err(ManifestError::Invalid(
            "an evidence bundle needs at least one artifact".into(),
        ));
    }
    if root.exists() {
        return Err(ManifestError::Invalid(format!(
            "evidence bundle root {} already exists",
            root.display()
        )));
    }
    for input in inputs {
        let dest = Path::new(&input.relative_path);
        if dest.is_absolute()
            || dest
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || input.relative_path == EVIDENCE_BUNDLE_MANIFEST_NAME
        {
            return Err(ManifestError::ArtifactEscapesRoot(
                input.relative_path.clone(),
            ));
        }
    }

    fs::create_dir_all(root).map_err(|source| ManifestError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    let mut artifacts = Vec::with_capacity(inputs.len());
    for input in inputs {
        let destination = root.join(&input.relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| ManifestError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(&input.source, &destination).map_err(|source| ManifestError::Io {
            path: input.source.clone(),
            source,
        })?;
        artifacts.push(ArtifactRecord {
            role: input.role.clone(),
            path: input.relative_path.clone(),
            sha256: sha256_file(&destination).map_err(|source| ManifestError::Io {
                path: destination.clone(),
                source,
            })?,
            media_type: input.media_type.clone(),
        });
    }
    artifacts.sort_by(|a, b| a.path.cmp(&b.path));

    let manifest = EvidenceBundleManifest {
        schema_version: EVIDENCE_BUNDLE_MANIFEST_SCHEMA.into(),
        case_id: case_id.into(),
        qualification,
        artifacts,
    };
    manifest.validate()?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| ManifestError::Invalid(format!("manifest serialization: {e}")))?;
    let manifest_path = root.join(EVIDENCE_BUNDLE_MANIFEST_NAME);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&manifest_path)
        .map_err(|source| ManifestError::Io {
            path: manifest_path.clone(),
            source,
        })?;
    file.write_all(&manifest_bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|source| ManifestError::Io {
            path: manifest_path,
            source,
        })?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(role: &str, relative: &str) -> BundleInput {
        BundleInput {
            role: role.into(),
            source: PathBuf::new(),
            relative_path: relative.into(),
            media_type: None,
        }
    }

    #[test]
    fn export_copies_and_verifies() {
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("source.json");
        fs::write(&source, b"{\"ok\":true}").unwrap();
        let root = scratch.path().join("bundle");
        let mut bundle_input = input("payload", "inputs/source.json");
        bundle_input.source = source;
        let manifest = export_evidence_bundle(
            &root,
            "nf-bnct-001",
            QualificationBoundary::SyntheticResearchOnly,
            &[bundle_input],
        )
        .unwrap();
        assert_eq!(manifest.artifacts.len(), 1);
        assert_eq!(manifest.artifacts[0].path, "inputs/source.json");
        let loaded = EvidenceBundleManifest::load_verified(&root).unwrap();
        assert_eq!(loaded, manifest);
    }

    #[test]
    fn export_rejects_escapes_existing_roots_and_tampering() {
        let scratch = tempfile::tempdir().unwrap();
        let source = scratch.path().join("s.json");
        fs::write(&source, b"{}").unwrap();
        let root = scratch.path().join("bundle");

        let mut bad = input("payload", "../escape.json");
        bad.source = source.clone();
        assert!(
            export_evidence_bundle(
                &root,
                "c",
                QualificationBoundary::SyntheticResearchOnly,
                &[bad],
            )
            .is_err()
        );
        assert!(!root.exists());

        let mut ok = input("payload", "a.json");
        ok.source = source;
        export_evidence_bundle(
            &root,
            "c",
            QualificationBoundary::SyntheticResearchOnly,
            &[ok],
        )
        .unwrap();
        // Second export to the same root refuses.
        assert!(
            export_evidence_bundle(
                &root,
                "c",
                QualificationBoundary::SyntheticResearchOnly,
                &[],
            )
            .is_err()
        );
        // Tamper and verify detection.
        fs::write(root.join("a.json"), b"changed").unwrap();
        assert!(EvidenceBundleManifest::load_verified(&root).is_err());
    }
}
