// SPDX-License-Identifier: MIT

//! Case-scoped identity and verification for evaluated neutron source files.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};

use openbnct_core::ContentReference;
use openbnct_transport::MaterialDefinition;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::acquisition::{
    AcquisitionEvidenceState, DataAcquisitionProfileDocument, DataAcquisitionReceiptDocument,
    PublisherDigestStatus,
};
use crate::data::TARGET_EVALUATED_DATA_RELEASE;

pub const EVALUATED_SOURCE_SELECTION_SCHEMA: &str =
    "openbnct.evaluated-neutron-source-selection/0.1.0";
pub const EVALUATED_SOURCE_SELECTION_CANDIDATE_SCHEMA: &str =
    "openbnct.evaluated-neutron-source-selection/0.2.0";
pub const EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA: &str =
    "openbnct.evaluated-neutron-source-selection/0.3.0";

const ENDFB_ACQUISITION_ROLE: &str = "endfb_incident_neutron_evaluations";
const CANDIDATE_ACQUISITION_ROLE: &str = "incident_neutron_evaluations";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedNeutronSourceSelection {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: EvaluatedSourceQualification,
    pub evaluated_data_release: String,
    pub material: ContentReference,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<EvaluatedSourceAcquisition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisitions: Option<Vec<EvaluatedSourceAcquisition>>,
    pub evaluations: Vec<EvaluatedNeutronArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatedSourceQualification {
    CandidateArchiveEquivalenceUnresolved,
    ResponseTreatmentCandidateUnreviewed,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedSourceAcquisition {
    pub profile_id: String,
    pub profile_sha256: String,
    pub receipt_sha256: String,
    pub archive_filename: String,
    pub archive_size_bytes: u64,
    pub archive_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluatedNeutronArtifact {
    pub nuclide: String,
    pub endf_mat: u16,
    pub archive_path: String,
    pub extracted_filename: String,
    pub size_bytes: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedNeutronSourceSelectionDocument {
    pub selection: EvaluatedNeutronSourceSelection,
    pub sha256: String,
}

impl EvaluatedNeutronSourceSelectionDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, EvaluatedSourceError> {
        let selection: EvaluatedNeutronSourceSelection = serde_json::from_slice(bytes)?;
        selection.validate()?;
        Ok(Self {
            selection,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, EvaluatedSourceError> {
        let bytes = fs::read(path).map_err(|source| EvaluatedSourceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }
}

impl EvaluatedNeutronSourceSelection {
    pub fn validate(&self) -> Result<(), EvaluatedSourceError> {
        match self.schema_version.as_str() {
            EVALUATED_SOURCE_SELECTION_SCHEMA => {
                if self.qualification
                    != EvaluatedSourceQualification::CandidateArchiveEquivalenceUnresolved
                {
                    return Err(EvaluatedSourceError::QualificationSchemaMismatch);
                }
                if self.evaluated_data_release != TARGET_EVALUATED_DATA_RELEASE {
                    return Err(EvaluatedSourceError::UnsupportedEvaluatedDataRelease(
                        self.evaluated_data_release.clone(),
                    ));
                }
            }
            EVALUATED_SOURCE_SELECTION_CANDIDATE_SCHEMA
            | EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA => {
                if self.qualification
                    != EvaluatedSourceQualification::ResponseTreatmentCandidateUnreviewed
                {
                    return Err(EvaluatedSourceError::QualificationSchemaMismatch);
                }
            }
            _ => {
                return Err(EvaluatedSourceError::UnsupportedSchema(
                    self.schema_version.clone(),
                ));
            }
        }
        let mixed = openbnct_core::schema_matches(
            &self.schema_version,
            EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA,
        );
        match (mixed, &self.acquisition, &self.acquisitions) {
            (false, Some(_), None) => {}
            (false, _, _) => {
                return Err(EvaluatedSourceError::SingleAcquisitionRequired(
                    self.schema_version.clone(),
                ));
            }
            (true, None, Some(acquisitions)) if !acquisitions.is_empty() => {}
            (true, _, _) => {
                return Err(EvaluatedSourceError::AcquisitionListRequired(
                    self.schema_version.clone(),
                ));
            }
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("case_id", self.case_id.as_str()),
            (
                "evaluated_data_release",
                self.evaluated_data_release.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(EvaluatedSourceError::EmptyIdentifier(label));
            }
        }
        self.material
            .validate()
            .map_err(|_| EvaluatedSourceError::InvalidMaterialReference)?;
        let declared_acquisitions = self.declared_acquisitions();
        for acquisition in declared_acquisitions {
            validate_acquisition_block(acquisition)?;
        }
        if mixed {
            let mut archive_digests = BTreeSet::new();
            for (index, acquisition) in declared_acquisitions.iter().enumerate() {
                if index > 0
                    && declared_acquisitions[index - 1].archive_sha256 >= acquisition.archive_sha256
                {
                    return Err(EvaluatedSourceError::NoncanonicalAcquisitionOrder);
                }
                archive_digests.insert(acquisition.archive_sha256.as_str());
            }
            let mut referenced = BTreeSet::new();
            for evaluation in &self.evaluations {
                let reference = evaluation.acquisition_sha256.as_deref().ok_or_else(|| {
                    EvaluatedSourceError::UnboundEvaluationAcquisition {
                        nuclide: evaluation.nuclide.clone(),
                    }
                })?;
                if !archive_digests.contains(reference) {
                    return Err(EvaluatedSourceError::UnboundEvaluationAcquisition {
                        nuclide: evaluation.nuclide.clone(),
                    });
                }
                referenced.insert(reference);
            }
            if let Some(unreferenced) = archive_digests.difference(&referenced).next() {
                return Err(EvaluatedSourceError::UnreferencedAcquisition(
                    (*unreferenced).into(),
                ));
            }
        }
        if self.evaluations.is_empty() {
            return Err(EvaluatedSourceError::EmptySelection);
        }

        let mut archive_paths = BTreeSet::new();
        let mut filenames = BTreeSet::new();
        let mut endf_materials = BTreeSet::new();
        for (index, evaluation) in self.evaluations.iter().enumerate() {
            if evaluation.nuclide.trim().is_empty() {
                return Err(EvaluatedSourceError::EmptyIdentifier("evaluations.nuclide"));
            }
            if index > 0 && self.evaluations[index - 1].nuclide >= evaluation.nuclide {
                return Err(EvaluatedSourceError::NoncanonicalNuclideOrder);
            }
            validate_relative_path("evaluations.archive_path", &evaluation.archive_path)?;
            validate_filename(
                "evaluations.extracted_filename",
                &evaluation.extracted_filename,
            )?;
            if Path::new(&evaluation.archive_path).file_name()
                != Some(evaluation.extracted_filename.as_ref())
            {
                return Err(EvaluatedSourceError::ArchiveFilenameMismatch {
                    nuclide: evaluation.nuclide.clone(),
                });
            }
            if !archive_paths.insert(evaluation.archive_path.as_str())
                || !filenames.insert(evaluation.extracted_filename.as_str())
            {
                return Err(EvaluatedSourceError::DuplicateArtifactIdentity);
            }
            if evaluation.endf_mat == 0 || evaluation.endf_mat > 9_999 {
                return Err(EvaluatedSourceError::InvalidEndfMaterialNumber {
                    nuclide: evaluation.nuclide.clone(),
                    value: evaluation.endf_mat,
                });
            }
            if !endf_materials.insert(evaluation.endf_mat) {
                return Err(EvaluatedSourceError::DuplicateEndfMaterial(
                    evaluation.endf_mat,
                ));
            }
            if evaluation.size_bytes == 0 {
                return Err(EvaluatedSourceError::EmptyEvaluation(
                    evaluation.nuclide.clone(),
                ));
            }
            validate_sha256("evaluations.sha256", &evaluation.sha256)?;
            if !mixed && evaluation.acquisition_sha256.is_some() {
                return Err(EvaluatedSourceError::UnexpectedEvaluationAcquisition {
                    nuclide: evaluation.nuclide.clone(),
                });
            }
        }
        Ok(())
    }

    /// All acquisitions bound by this selection, in declared order.
    pub fn declared_acquisitions(&self) -> &[EvaluatedSourceAcquisition] {
        if let Some(acquisitions) = &self.acquisitions {
            acquisitions
        } else if let Some(acquisition) = &self.acquisition {
            std::slice::from_ref(acquisition)
        } else {
            &[]
        }
    }

    pub fn validate_for_material(
        &self,
        material: &MaterialDefinition,
        material_bytes: &[u8],
    ) -> Result<(), EvaluatedSourceError> {
        self.validate()?;
        material
            .validate()
            .map_err(|error| EvaluatedSourceError::InvalidMaterial(error.to_string()))?;
        let observed_reference = ContentReference {
            id: material.id.clone(),
            sha256: sha256_bytes(material_bytes),
        };
        if self.material != observed_reference {
            return Err(EvaluatedSourceError::MaterialBindingMismatch);
        }

        let required = material
            .nuclides
            .iter()
            .map(|nuclide| nuclide.name.as_str())
            .collect::<BTreeSet<_>>();
        let selected = self
            .evaluations
            .iter()
            .map(|evaluation| evaluation.nuclide.as_str())
            .collect::<BTreeSet<_>>();
        if let Some(missing) = required.difference(&selected).next() {
            return Err(EvaluatedSourceError::MissingNuclide((*missing).into()));
        }
        if let Some(unexpected) = selected.difference(&required).next() {
            return Err(EvaluatedSourceError::UnexpectedNuclide(
                (*unexpected).into(),
            ));
        }
        Ok(())
    }

    pub fn validate_acquisition(
        &self,
        profile: &DataAcquisitionProfileDocument,
        receipt: &DataAcquisitionReceiptDocument,
    ) -> Result<(), EvaluatedSourceError> {
        self.validate_acquisitions(&[(profile, receipt)])
    }

    /// Validate every bound acquisition against its profile and receipt pair.
    ///
    /// For the mixed schema, a publisher that distributes no digest yields an
    /// `unavailable` receipt status; a declared digest must still be matched.
    /// Single-acquisition schemas keep the stricter matched-digest rule.
    pub fn validate_acquisitions(
        &self,
        pairs: &[(
            &DataAcquisitionProfileDocument,
            &DataAcquisitionReceiptDocument,
        )],
    ) -> Result<(), EvaluatedSourceError> {
        self.validate()?;
        let mixed = openbnct_core::schema_matches(
            &self.schema_version,
            EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA,
        );
        let mut observed = Vec::with_capacity(pairs.len());
        for (profile, receipt) in pairs {
            receipt
                .validate_for_profile(profile)
                .map_err(|error| EvaluatedSourceError::InvalidAcquisition(error.to_string()))?;
            let role_allowed = match self.schema_version.as_str() {
                EVALUATED_SOURCE_SELECTION_SCHEMA => {
                    profile.profile.artifact_role == ENDFB_ACQUISITION_ROLE
                }
                EVALUATED_SOURCE_SELECTION_CANDIDATE_SCHEMA => {
                    profile.profile.artifact_role == CANDIDATE_ACQUISITION_ROLE
                }
                EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA => matches!(
                    profile.profile.artifact_role.as_str(),
                    ENDFB_ACQUISITION_ROLE | CANDIDATE_ACQUISITION_ROLE
                ),
                _ => {
                    return Err(EvaluatedSourceError::UnsupportedSchema(
                        self.schema_version.clone(),
                    ));
                }
            };
            let digest_acceptable = match receipt.receipt.publisher_digest_status {
                PublisherDigestStatus::Matched => true,
                PublisherDigestStatus::Unavailable => {
                    mixed && profile.profile.artifact.publisher_digest.is_none()
                }
            };
            if !role_allowed
                || !digest_acceptable
                || receipt.receipt.evidence_state != AcquisitionEvidenceState::AcquisitionOnly
            {
                return Err(EvaluatedSourceError::InvalidAcquisitionState);
            }
            observed.push(EvaluatedSourceAcquisition {
                profile_id: profile.profile.id.clone(),
                profile_sha256: profile.sha256.clone(),
                receipt_sha256: receipt.sha256.clone(),
                archive_filename: receipt.receipt.artifact.path.clone(),
                archive_size_bytes: receipt.receipt.artifact.size_bytes,
                archive_sha256: receipt.receipt.artifact.sha256.clone(),
            });
        }
        let declared = self.declared_acquisitions();
        if declared.len() != observed.len()
            || declared.iter().collect::<BTreeSet<_>>() != observed.iter().collect::<BTreeSet<_>>()
        {
            return Err(EvaluatedSourceError::AcquisitionBindingMismatch);
        }
        Ok(())
    }

    /// Verify that the directory contains exactly the selected regular files.
    pub fn verify_files(&self, root: &Path) -> Result<(), EvaluatedSourceError> {
        self.validate()?;
        let canonical_root = fs::canonicalize(root).map_err(|source| EvaluatedSourceError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        if !canonical_root.is_dir() {
            return Err(EvaluatedSourceError::SelectionRootNotDirectory(
                root.to_path_buf(),
            ));
        }

        let expected = self
            .evaluations
            .iter()
            .map(|evaluation| evaluation.extracted_filename.as_str())
            .collect::<BTreeSet<_>>();
        let mut observed = BTreeSet::new();
        for entry in fs::read_dir(&canonical_root).map_err(|source| EvaluatedSourceError::Io {
            path: canonical_root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| EvaluatedSourceError::Io {
                path: canonical_root.clone(),
                source,
            })?;
            let name = entry.file_name().into_string().map_err(|_| {
                EvaluatedSourceError::UnexpectedSelectionEntry("non-UTF-8 filename".into())
            })?;
            if !entry
                .file_type()
                .map_err(|source| EvaluatedSourceError::Io {
                    path: entry.path(),
                    source,
                })?
                .is_file()
            {
                return Err(EvaluatedSourceError::UnexpectedSelectionEntry(name));
            }
            observed.insert(name);
        }
        let expected_owned = expected.iter().map(|name| (*name).to_owned()).collect();
        if observed != expected_owned {
            return Err(EvaluatedSourceError::SelectionDirectoryMismatch);
        }

        for evaluation in &self.evaluations {
            let path = canonical_root.join(&evaluation.extracted_filename);
            let canonical_path =
                fs::canonicalize(&path).map_err(|source| EvaluatedSourceError::Io {
                    path: path.clone(),
                    source,
                })?;
            if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
                return Err(EvaluatedSourceError::ArtifactEscapesRoot(
                    evaluation.extracted_filename.clone(),
                ));
            }
            let size_bytes = canonical_path
                .metadata()
                .map_err(|source| EvaluatedSourceError::Io {
                    path: canonical_path.clone(),
                    source,
                })?
                .len();
            if size_bytes != evaluation.size_bytes {
                return Err(EvaluatedSourceError::SizeMismatch {
                    path: evaluation.extracted_filename.clone(),
                    expected: evaluation.size_bytes,
                    observed: size_bytes,
                });
            }
            let observed_sha256 =
                sha256_file(&canonical_path).map_err(|source| EvaluatedSourceError::Io {
                    path: canonical_path.clone(),
                    source,
                })?;
            if observed_sha256 != evaluation.sha256 {
                return Err(EvaluatedSourceError::HashMismatch {
                    path: evaluation.extracted_filename.clone(),
                    expected: evaluation.sha256.clone(),
                    observed: observed_sha256,
                });
            }
            let observed_mat = endf_material(&canonical_path)?;
            if observed_mat != evaluation.endf_mat {
                return Err(EvaluatedSourceError::EndfMaterialMismatch {
                    nuclide: evaluation.nuclide.clone(),
                    expected: evaluation.endf_mat,
                    observed: observed_mat,
                });
            }
        }
        Ok(())
    }
}

fn endf_material(path: &Path) -> Result<u16, EvaluatedSourceError> {
    let file = File::open(path).map_err(|source| EvaluatedSourceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::with_capacity(82);
    loop {
        line.clear();
        let count =
            reader
                .read_until(b'\n', &mut line)
                .map_err(|source| EvaluatedSourceError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
        if count == 0 {
            return Err(EvaluatedSourceError::MissingEndfMaterial(
                path.to_path_buf(),
            ));
        }
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop();
        }
        if line.len() < 75 {
            continue;
        }
        let mat = fixed_width_u16(&line[66..70]);
        let mf = fixed_width_u16(&line[70..72]);
        let mt = fixed_width_u16(&line[72..75]);
        if mf == Some(1) && mt == Some(451) {
            return mat
                .filter(|value| *value > 0)
                .ok_or_else(|| EvaluatedSourceError::InvalidEndfMaterial(path.to_path_buf()));
        }
    }
}

fn fixed_width_u16(value: &[u8]) -> Option<u16> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}

fn validate_acquisition_block(
    acquisition: &EvaluatedSourceAcquisition,
) -> Result<(), EvaluatedSourceError> {
    if acquisition.profile_id.trim().is_empty() {
        return Err(EvaluatedSourceError::EmptyIdentifier(
            "acquisition.profile_id",
        ));
    }
    validate_sha256("acquisition.profile_sha256", &acquisition.profile_sha256)?;
    validate_sha256("acquisition.receipt_sha256", &acquisition.receipt_sha256)?;
    validate_filename(
        "acquisition.archive_filename",
        &acquisition.archive_filename,
    )?;
    if acquisition.archive_size_bytes == 0 {
        return Err(EvaluatedSourceError::EmptyArchive);
    }
    validate_sha256("acquisition.archive_sha256", &acquisition.archive_sha256)
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), EvaluatedSourceError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(EvaluatedSourceError::InvalidSha256(label));
    }
    Ok(())
}

fn validate_filename(label: &'static str, value: &str) -> Result<(), EvaluatedSourceError> {
    let path = Path::new(value);
    let mut components = path.components();
    if value.is_empty()
        || value.contains('\\')
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(EvaluatedSourceError::InvalidPath(label, value.into()));
    }
    Ok(())
}

fn validate_relative_path(label: &'static str, value: &str) -> Result<(), EvaluatedSourceError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(EvaluatedSourceError::InvalidPath(label, value.into()));
    }
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[derive(Debug, Error)]
pub enum EvaluatedSourceError {
    #[error("failed to parse evaluated-source selection JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("evaluated-source selection schema {0:?} is unsupported")]
    UnsupportedSchema(String),
    #[error("evaluated-source qualification does not match its schema version")]
    QualificationSchemaMismatch,
    #[error("required evaluated-source identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("evaluated-data release {0:?} is unsupported")]
    UnsupportedEvaluatedDataRelease(String),
    #[error("material content reference is invalid")]
    InvalidMaterialReference,
    #[error("{0} must be a canonical lowercase SHA-256 digest")]
    InvalidSha256(&'static str),
    #[error("{0} is not a normalized path: {1:?}")]
    InvalidPath(&'static str, String),
    #[error("evaluated-source archive must not be empty")]
    EmptyArchive,
    #[error("evaluated-source selection must not be empty")]
    EmptySelection,
    #[error("evaluated-source nuclides must be strictly ordered without duplicates")]
    NoncanonicalNuclideOrder,
    #[error("archive and extracted filenames differ for {nuclide}")]
    ArchiveFilenameMismatch { nuclide: String },
    #[error("evaluated-source paths or filenames contain a duplicate")]
    DuplicateArtifactIdentity,
    #[error("ENDF MAT {value} for {nuclide} is outside the supported 1..=9999 range")]
    InvalidEndfMaterialNumber { nuclide: String, value: u16 },
    #[error("ENDF MAT {0} is assigned to more than one selected evaluation")]
    DuplicateEndfMaterial(u16),
    #[error("evaluation for {0} is empty")]
    EmptyEvaluation(String),
    #[error("material definition is invalid: {0}")]
    InvalidMaterial(String),
    #[error("evaluated-source selection does not match the frozen material bytes")]
    MaterialBindingMismatch,
    #[error("required evaluated source {0} is missing")]
    MissingNuclide(String),
    #[error("unrequested evaluated source {0} is present")]
    UnexpectedNuclide(String),
    #[error("acquisition evidence is invalid: {0}")]
    InvalidAcquisition(String),
    #[error("evaluated-source schema {0} requires a single acquisition object")]
    SingleAcquisitionRequired(String),
    #[error("evaluated-source schema {0} requires a non-empty acquisitions list")]
    AcquisitionListRequired(String),
    #[error("evaluated-source acquisitions must be ordered by archive SHA-256 without duplicates")]
    NoncanonicalAcquisitionOrder,
    #[error("evaluation for {nuclide} does not reference a bound acquisition")]
    UnboundEvaluationAcquisition { nuclide: String },
    #[error("per-evaluation acquisition_sha256 is not permitted for {nuclide} under this schema")]
    UnexpectedEvaluationAcquisition { nuclide: String },
    #[error("bound acquisition {0} is not referenced by any evaluation")]
    UnreferencedAcquisition(String),
    #[error("evaluated-source acquisition is not matched publisher evidence")]
    InvalidAcquisitionState,
    #[error("evaluated-source selection does not match the acquisition profile and receipt")]
    AcquisitionBindingMismatch,
    #[error("evaluated-source root is not a directory: {0}")]
    SelectionRootNotDirectory(PathBuf),
    #[error("unexpected entry in evaluated-source directory: {0:?}")]
    UnexpectedSelectionEntry(String),
    #[error("evaluated-source directory does not contain exactly the selected files")]
    SelectionDirectoryMismatch,
    #[error("evaluated-source artifact escapes its verified root: {0}")]
    ArtifactEscapesRoot(String),
    #[error("evaluated-source size mismatch for {path}: expected {expected}, observed {observed}")]
    SizeMismatch {
        path: String,
        expected: u64,
        observed: u64,
    },
    #[error("evaluated-source hash mismatch for {path}: expected {expected}, observed {observed}")]
    HashMismatch {
        path: String,
        expected: String,
        observed: String,
    },
    #[error("no MF=1 MT=451 material header was found in {0}")]
    MissingEndfMaterial(PathBuf),
    #[error("the MF=1 MT=451 material header is invalid in {0}")]
    InvalidEndfMaterial(PathBuf),
    #[error(
        "ENDF MAT mismatch for {nuclide}: selection requires {expected}, file declares {observed}"
    )]
    EndfMaterialMismatch {
        nuclide: String,
        expected: u16,
        observed: u16,
    },
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

    const MATERIAL_BYTES: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const PROFILE_BYTES: &[u8] =
        include_bytes!("../../../profiles/openmc/endfb81-neutron-evaluations.json");
    const RECEIPT_BYTES: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/endfb81-neutron-acquisition-receipt.json"
    );
    const SELECTION_BYTES: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/evaluated-neutron-source-selection.json"
    );
    const JEFF40_PROFILE_BYTES: &[u8] =
        include_bytes!("../../../profiles/njoy/jeff40-neutron-evaluations.json");
    const JEFF40_RECEIPT_BYTES: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/provenance/jeff40-neutron-acquisition-receipt.json"
    );
    const JEFF40_SELECTION_BYTES: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/candidates/jeff40/evaluated-neutron-source-selection.json"
    );

    fn frozen_selection() -> EvaluatedNeutronSourceSelection {
        EvaluatedNeutronSourceSelectionDocument::from_bytes(SELECTION_BYTES)
            .unwrap()
            .selection
    }

    #[test]
    fn frozen_selection_binds_material_and_acquisition() {
        let selection = frozen_selection();
        let material: MaterialDefinition = serde_json::from_slice(MATERIAL_BYTES).unwrap();
        let profile = DataAcquisitionProfileDocument::from_bytes(PROFILE_BYTES).unwrap();
        let receipt = DataAcquisitionReceiptDocument::from_bytes(RECEIPT_BYTES).unwrap();

        selection
            .validate_for_material(&material, MATERIAL_BYTES)
            .unwrap();
        selection.validate_acquisition(&profile, &receipt).unwrap();
        assert_eq!(selection.evaluations.len(), material.nuclides.len());
    }

    #[test]
    fn versioned_candidate_selection_binds_material_and_matched_acquisition() {
        let selection = EvaluatedNeutronSourceSelectionDocument::from_bytes(JEFF40_SELECTION_BYTES)
            .unwrap()
            .selection;
        let material: MaterialDefinition = serde_json::from_slice(MATERIAL_BYTES).unwrap();
        let profile = DataAcquisitionProfileDocument::from_bytes(JEFF40_PROFILE_BYTES).unwrap();
        let receipt = DataAcquisitionReceiptDocument::from_bytes(JEFF40_RECEIPT_BYTES).unwrap();

        selection
            .validate_for_material(&material, MATERIAL_BYTES)
            .unwrap();
        selection.validate_acquisition(&profile, &receipt).unwrap();
        assert_eq!(selection.evaluated_data_release, "JEFF-4.0");
        assert_eq!(
            selection.qualification,
            EvaluatedSourceQualification::ResponseTreatmentCandidateUnreviewed
        );
        assert_eq!(selection.evaluations.len(), material.nuclides.len());
    }

    #[test]
    fn candidate_schema_rejects_baseline_qualification() {
        let mut selection = frozen_selection();
        selection.schema_version = EVALUATED_SOURCE_SELECTION_CANDIDATE_SCHEMA.into();
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::QualificationSchemaMismatch)
        ));
    }

    #[test]
    fn rejects_material_or_acquisition_substitution() {
        let mut selection = frozen_selection();
        let material: MaterialDefinition = serde_json::from_slice(MATERIAL_BYTES).unwrap();
        selection.material.sha256 = "0".repeat(64);
        assert!(matches!(
            selection.validate_for_material(&material, MATERIAL_BYTES),
            Err(EvaluatedSourceError::MaterialBindingMismatch)
        ));

        let mut selection = frozen_selection();
        let profile = DataAcquisitionProfileDocument::from_bytes(PROFILE_BYTES).unwrap();
        let receipt = DataAcquisitionReceiptDocument::from_bytes(RECEIPT_BYTES).unwrap();
        selection.acquisition.as_mut().unwrap().archive_sha256 = "0".repeat(64);
        assert!(matches!(
            selection.validate_acquisition(&profile, &receipt),
            Err(EvaluatedSourceError::AcquisitionBindingMismatch)
        ));
    }

    fn observed_acquisition(
        profile: &DataAcquisitionProfileDocument,
        receipt: &DataAcquisitionReceiptDocument,
    ) -> EvaluatedSourceAcquisition {
        EvaluatedSourceAcquisition {
            profile_id: profile.profile.id.clone(),
            profile_sha256: profile.sha256.clone(),
            receipt_sha256: receipt.sha256.clone(),
            archive_filename: receipt.receipt.artifact.path.clone(),
            archive_size_bytes: receipt.receipt.artifact.size_bytes,
            archive_sha256: receipt.receipt.artifact.sha256.clone(),
        }
    }

    fn endfb_acquisition_pair() -> (
        DataAcquisitionProfileDocument,
        DataAcquisitionReceiptDocument,
    ) {
        (
            DataAcquisitionProfileDocument::from_bytes(PROFILE_BYTES).unwrap(),
            DataAcquisitionReceiptDocument::from_bytes(RECEIPT_BYTES).unwrap(),
        )
    }

    fn synthetic_candidate_pair(
        archive_sha256: &str,
        with_digest: bool,
    ) -> (
        DataAcquisitionProfileDocument,
        DataAcquisitionReceiptDocument,
    ) {
        let publisher_digest = if with_digest {
            serde_json::json!({
                "algorithm": "sha256",
                "value": archive_sha256,
                "evidence": "test digest listing"
            })
        } else {
            serde_json::Value::Null
        };
        let profile_json = serde_json::json!({
            "schema_version": "openbnct.data-acquisition-profile/0.2.0",
            "id": "test-candidate-profile",
            "artifact_role": "incident_neutron_evaluations",
            "publication": {
                "publisher": "test publisher",
                "release_page_uri": "https://example.com/release",
                "source_uri": "https://example.com/artifacts/candidate.tgz",
                "allowed_https_host_suffixes": ["example.com"]
            },
            "artifact": {
                "filename": "candidate.tgz",
                "media_type": "application/gzip",
                "expected_size_bytes": 4096,
                "expected_content_disposition_filename": null,
                "publisher_digest": publisher_digest,
                "known_prior_digests": []
            },
            "size_evidence": {
                "method": "https_head_content_length",
                "observed_on": "2026-09-11"
            },
            "upstream_recipe": null
        });
        let profile =
            DataAcquisitionProfileDocument::from_bytes(&serde_json::to_vec(&profile_json).unwrap())
                .unwrap();
        let receipt_json = serde_json::json!({
            "schema_version": "openbnct.data-acquisition-receipt/0.1.0",
            "profile_id": profile.profile.id,
            "profile_sha256": profile.sha256,
            "artifact_role": "incident_neutron_evaluations",
            "artifact": {
                "path": "candidate.tgz",
                "media_type": "application/gzip",
                "size_bytes": 4096,
                "sha256": archive_sha256,
                "publisher_digest": publisher_digest
            },
            "transfer": {
                "requested_uri": "https://example.com/artifacts/candidate.tgz",
                "final_origin": "https://example.com",
                "resumed_from_bytes": 4096,
                "content_disposition_filename": null,
                "etag": null,
                "last_modified": null
            },
            "publisher_digest_status": if with_digest { "matched" } else { "unavailable" },
            "evidence_state": "acquisition_only",
            "completed_at_unix_seconds": 1
        });
        let receipt =
            DataAcquisitionReceiptDocument::from_bytes(&serde_json::to_vec(&receipt_json).unwrap())
                .unwrap();
        (profile, receipt)
    }

    fn mixed_selection() -> EvaluatedNeutronSourceSelection {
        let base = frozen_selection();
        let (endfb_profile, endfb_receipt) = endfb_acquisition_pair();
        let (candidate_profile, candidate_receipt) =
            synthetic_candidate_pair(&"f".repeat(64), false);
        let endfb_acquisition = observed_acquisition(&endfb_profile, &endfb_receipt);
        let candidate_acquisition = observed_acquisition(&candidate_profile, &candidate_receipt);
        let mut evaluations = base.evaluations.clone();
        for (index, evaluation) in evaluations.iter_mut().enumerate() {
            evaluation.acquisition_sha256 = Some(if index < 6 {
                endfb_acquisition.archive_sha256.clone()
            } else {
                candidate_acquisition.archive_sha256.clone()
            });
        }
        EvaluatedNeutronSourceSelection {
            schema_version: EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA.into(),
            id: "test.mixed-selection".into(),
            case_id: base.case_id.clone(),
            qualification: EvaluatedSourceQualification::ResponseTreatmentCandidateUnreviewed,
            evaluated_data_release: "ENDF/B-VIII.1+TEST".into(),
            material: base.material.clone(),
            acquisition: None,
            acquisitions: Some(vec![endfb_acquisition, candidate_acquisition]),
            evaluations,
        }
    }

    #[test]
    fn mixed_selection_validates_structure() {
        let selection = mixed_selection();
        selection.validate().unwrap();
        assert_eq!(selection.declared_acquisitions().len(), 2);
    }

    #[test]
    fn mixed_selection_binds_all_acquisition_pairs() {
        let selection = mixed_selection();
        let (endfb_profile, endfb_receipt) = endfb_acquisition_pair();
        let (candidate_profile, candidate_receipt) =
            synthetic_candidate_pair(&"f".repeat(64), false);
        selection
            .validate_acquisitions(&[
                (&endfb_profile, &endfb_receipt),
                (&candidate_profile, &candidate_receipt),
            ])
            .unwrap();

        let (other_profile, other_receipt) = synthetic_candidate_pair(&"e".repeat(64), false);
        assert!(matches!(
            selection.validate_acquisitions(&[
                (&endfb_profile, &endfb_receipt),
                (&other_profile, &other_receipt),
            ]),
            Err(EvaluatedSourceError::AcquisitionBindingMismatch)
        ));
        assert!(matches!(
            selection.validate_acquisitions(&[(&endfb_profile, &endfb_receipt)]),
            Err(EvaluatedSourceError::AcquisitionBindingMismatch)
        ));
    }

    #[test]
    fn mixed_schema_accepts_undeclared_digest_but_candidate_schema_does_not() {
        let selection = mixed_selection();
        let (endfb_profile, endfb_receipt) = endfb_acquisition_pair();
        let (undigested_profile, undigested_receipt) =
            synthetic_candidate_pair(&"f".repeat(64), false);
        selection
            .validate_acquisitions(&[
                (&endfb_profile, &endfb_receipt),
                (&undigested_profile, &undigested_receipt),
            ])
            .unwrap();

        let (digested_profile, digested_receipt) = synthetic_candidate_pair(&"f".repeat(64), true);
        let mut digested_selection = mixed_selection();
        digested_selection.acquisitions.as_mut().unwrap()[1] =
            observed_acquisition(&digested_profile, &digested_receipt);
        digested_selection
            .validate_acquisitions(&[
                (&endfb_profile, &endfb_receipt),
                (&digested_profile, &digested_receipt),
            ])
            .unwrap();

        let candidate = EvaluatedNeutronSourceSelectionDocument::from_bytes(JEFF40_SELECTION_BYTES)
            .unwrap()
            .selection;
        assert!(matches!(
            candidate.validate_acquisitions(&[(&undigested_profile, &undigested_receipt)]),
            Err(EvaluatedSourceError::InvalidAcquisitionState)
        ));
    }

    #[test]
    fn mixed_selection_rejects_single_acquisition_shape() {
        let mut selection = mixed_selection();
        selection.acquisition = selection.acquisitions.take().map(|mut list| list.remove(0));
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::AcquisitionListRequired(_))
        ));
    }

    #[test]
    fn mixed_selection_rejects_unsorted_acquisitions() {
        let mut selection = mixed_selection();
        selection.acquisitions.as_mut().unwrap().reverse();
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::NoncanonicalAcquisitionOrder)
        ));
    }

    #[test]
    fn mixed_selection_rejects_unbound_or_unreferenced_acquisitions() {
        let mut selection = mixed_selection();
        selection.evaluations[0].acquisition_sha256 = Some("9".repeat(64));
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::UnboundEvaluationAcquisition { .. })
        ));

        let mut selection = mixed_selection();
        selection.evaluations[9].acquisition_sha256 = None;
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::UnboundEvaluationAcquisition { .. })
        ));

        let mut selection = mixed_selection();
        for evaluation in &mut selection.evaluations {
            evaluation.acquisition_sha256 = Some("f".repeat(64));
        }
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::UnreferencedAcquisition(_))
        ));
    }

    #[test]
    fn single_schemas_reject_acquisition_list_and_eval_references() {
        let mut selection = frozen_selection();
        selection.acquisitions = Some(vec![selection.acquisition.clone().unwrap()]);
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::SingleAcquisitionRequired(_))
        ));

        let mut selection = frozen_selection();
        selection.evaluations[0].acquisition_sha256 = Some("f".repeat(64));
        assert!(matches!(
            selection.validate(),
            Err(EvaluatedSourceError::UnexpectedEvaluationAcquisition { .. })
        ));
    }

    #[test]
    fn selected_file_verifier_rejects_tampering_and_extras() {
        let root = tempfile::tempdir().unwrap();
        let mut selection = frozen_selection();
        selection.evaluations = vec![EvaluatedNeutronArtifact {
            nuclide: "H1".into(),
            endf_mat: 125,
            archive_path: "archive/n-001_H_001.endf".into(),
            extracted_filename: "n-001_H_001.endf".into(),
            size_bytes: 81,
            sha256: String::new(),
            acquisition_sha256: None,
        }];
        let body = format!("{:<66}{:>4}{:>2}{:>3}{:>5}\n", "", 125, 1, 451, 1);
        selection.evaluations[0].size_bytes = body.len() as u64;
        selection.evaluations[0].sha256 = sha256_bytes(body.as_bytes());
        fs::write(root.path().join("n-001_H_001.endf"), body.as_bytes()).unwrap();
        selection.verify_files(root.path()).unwrap();

        fs::write(root.path().join("unexpected.endf"), body.as_bytes()).unwrap();
        assert!(matches!(
            selection.verify_files(root.path()),
            Err(EvaluatedSourceError::SelectionDirectoryMismatch)
        ));
    }

    #[test]
    fn selected_file_verifier_rejects_endf_material_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let body = format!("{:<66}{:>4}{:>2}{:>3}{:>5}\n", "", 128, 1, 451, 1);
        let mut selection = frozen_selection();
        selection.evaluations = vec![EvaluatedNeutronArtifact {
            nuclide: "H1".into(),
            endf_mat: 125,
            archive_path: "archive/n-001_H_001.endf".into(),
            extracted_filename: "n-001_H_001.endf".into(),
            size_bytes: body.len() as u64,
            sha256: sha256_bytes(body.as_bytes()),
            acquisition_sha256: None,
        }];
        fs::write(root.path().join("n-001_H_001.endf"), body.as_bytes()).unwrap();

        assert!(matches!(
            selection.verify_files(root.path()),
            Err(EvaluatedSourceError::EndfMaterialMismatch {
                expected: 125,
                observed: 128,
                ..
            })
        ));
    }
}
