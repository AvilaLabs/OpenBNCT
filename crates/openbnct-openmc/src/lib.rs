// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use openbnct_core::PhysicalDoseBundle;
use openbnct_transport::{
    BackendDescriptor, CompletedRun, PreparedRun, TransportBackend, TransportCase,
};
use thiserror::Error;

mod acceptance;
mod acquisition;
mod data;
mod domain;
mod endf_mf3;
pub mod endf_mf33;
pub mod endf_mf7;
mod evaluated;
mod input;
mod mgcollapse;
mod photoncollapse;
mod statepoint;
mod variance_reduction;

pub use acceptance::{
    ACCEPTANCE_CONTRACT_FILE, ACCEPTANCE_REPORT_SCHEMA, OpenMcAcceptanceError,
    OpenMcAcceptanceReport, OpenMcChiSquare, OpenMcEstimatorComparison, OpenMcRegionResult,
    OpenMcRunBinding, OpenMcVoxelPrecision, evaluate_runs,
};
pub use acquisition::{
    ACQUISITION_PROFILE_SCHEMA, ACQUISITION_RECEIPT_SCHEMA, AcquiredData, AcquiredDataArtifact,
    AcquisitionError, AcquisitionEvidenceState, AcquisitionProgress, DataAcquisitionProbe,
    DataAcquisitionProfile, DataAcquisitionProfileDocument, DataAcquisitionReceipt,
    DataAcquisitionReceiptDocument, DataPublication, DataTransferEvidence, DigestAlgorithm,
    PublishedDataArtifact, PublishedDigest, PublisherDigestStatus, SizeEvidence, UpstreamRecipe,
};
// The HTTP acquisition client requires reqwest's blocking API —
// unavailable on wasm; evidence types above are pure data.
#[cfg(not(target_arch = "wasm32"))]
pub use acquisition::DataAcquisitionClient;
pub use data::{
    DataArtifact, DataDistributionIdentity, DataInspectionIdentity, NeutronTableCapability,
    NuclearDataError, NuclearDataManifest, PhotonTableCapability, TARGET_ACQUISITION_PROFILE_ID,
    TARGET_ACQUISITION_PROFILE_SHA256, TARGET_DATA_HDF5_VERSION,
    TARGET_DISTRIBUTION_ARCHIVE_SIZE_BYTES, TARGET_DISTRIBUTION_SOURCE_URI,
    TARGET_EVALUATED_DATA_RELEASE, TARGET_INSPECTION_METHOD, TARGET_NUCLEAR_DATA_MANIFEST_SCHEMA,
    TARGET_OPENMC_SOURCE_COMMIT, TARGET_OPENMC_VERSION, TEMPERATURE_TOLERANCE_K,
};
pub use domain::{
    OPENMC_NEUTRON_TRANSPORT_DOMAIN_SCHEMA, OpenMcDiagnosticBoundaryPolicy,
    OpenMcNeutronTransportDomain, OpenMcNeutronTransportDomainDocument,
    OpenMcNeutronTransportDomainResult, OpenMcTransportDomainDerivation,
    OpenMcTransportDomainError, OpenMcTransportDomainQualification,
};
pub use evaluated::{
    EVALUATED_SOURCE_SELECTION_CANDIDATE_SCHEMA, EVALUATED_SOURCE_SELECTION_MIXED_SCHEMA,
    EVALUATED_SOURCE_SELECTION_SCHEMA, EvaluatedNeutronArtifact, EvaluatedNeutronSourceSelection,
    EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceAcquisition, EvaluatedSourceError,
    EvaluatedSourceQualification,
};
pub use input::{
    ACCEPTANCE_CONTRACT_SCHEMA, CANDIDATE_REFERENCE_SEEDS, GeneratedOpenMcFile,
    OPENMC_DEFAULT_STRIDE, OpenMcAcceptanceContract, OpenMcAcceptanceGates, OpenMcAcceptanceRegion,
    OpenMcCollectionNormalization, OpenMcElectronTreatment, OpenMcEnergyMode,
    OpenMcEvaluatedDepositedEnergies, OpenMcExecutionProfile, OpenMcExecutionPurpose,
    OpenMcInputArtifacts, OpenMcInputBindings, OpenMcInputDeck, OpenMcInputError,
    OpenMcInputManifest, OpenMcInputManifestArtifact, OpenMcProfileError, OpenMcRawTallyUnit,
    OpenMcRegionBounds, OpenMcRoiMesh, OpenMcRunControls, OpenMcRunMode, OpenMcScoringMesh,
    OpenMcTallyContract, OpenMcTallyQuantity, OpenMcTallyScope, OpenMcTemperatureMethod,
};
pub use mgcollapse::{CollapseError, CollapseOptions, WeightingSpectrum, collapse_multigroup};
pub use photoncollapse::{PhotonCollapseOptions, collapse_photon};
pub use statepoint::{
    CollectedDose, OPENMC_INPUT_MANIFEST_FILE, OpenMcCollectError, OpenMcEnergyFunction,
    OpenMcStatepoint, OpenMcStatepointTally, collect_completed, collect_statepoint,
    latest_statepoint,
};
pub use variance_reduction::{
    RESOLVED_WW_FILE, VR_VALIDATION_SCHEMA, VarianceReductionError, VrComparison,
    VrReferenceBinding, VrRunBinding, VrValidationReport, resolve_weight_windows,
    validate_variance_reduction,
};

/// Resolved input artifacts and environment for backend-driven runs.
///
/// `prepare` loads these files, verifies every content binding, and writes a
/// deterministic deck; `execute` runs the configured executable with the
/// configured environment overlay and freezes a run receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMcBackendConfig {
    pub component_profile: PathBuf,
    pub material: PathBuf,
    pub source: PathBuf,
    pub response_set: PathBuf,
    pub nuclear_data_manifest: PathBuf,
    pub execution_profile: PathBuf,
    pub acceptance: Option<PathBuf>,
    /// DICOM-derived voxel-box material assignment for structure-derived cases.
    pub material_assignment: Option<PathBuf>,
    /// Resolved `openbnct.weight-windows` artifact enabling weight-window
    /// splitting/roulette for this run.
    pub variance_reduction: Option<PathBuf>,
    pub nuclear_data_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct OpenMcBackend {
    executable: PathBuf,
    environment: Vec<(String, String)>,
    config: Option<OpenMcBackendConfig>,
}

impl OpenMcBackend {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            environment: Vec::new(),
            config: None,
        }
    }

    /// Attach the artifact set `prepare` generates decks from.
    pub fn configured(mut self, config: OpenMcBackendConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Add an environment variable overlay for `execute` (for example
    /// `OPENMC_CROSS_SECTIONS` or `LD_LIBRARY_PATH`).
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.environment.push((key.into(), value.into()));
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn config(&self) -> Option<&OpenMcBackendConfig> {
        self.config.as_ref()
    }
}

impl Default for OpenMcBackend {
    fn default() -> Self {
        Self::new("openmc")
    }
}

/// Frozen execution evidence for one backend-driven OpenMC run.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRunReceipt {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub backend_id: String,
    pub case_id: String,
    pub input_manifest_sha256: String,
    pub executable: String,
    pub executable_sha256: String,
    pub environment_overlay: Vec<(String, String)>,
    pub started_unix_seconds: u64,
    pub finished_unix_seconds: u64,
    pub exit_code: i32,
    pub logs: Vec<OpenMcRunArtifact>,
    pub statepoints: Vec<OpenMcRunArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRunArtifact {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

pub const OPENMC_RUN_RECEIPT_FILE: &str = "openbnct-openmc-run-receipt.json";

impl TransportBackend for OpenMcBackend {
    type BackendError = OpenMcError;

    fn descriptor(&self) -> BackendDescriptor {
        BackendDescriptor {
            id: "openmc".into(),
            display_name: "OpenMC".into(),
            version: None,
            can_prepare: self.config.is_some(),
            can_execute: true,
            can_import: true,
        }
    }

    fn prepare(
        &self,
        case: &TransportCase,
        working_directory: &Path,
    ) -> Result<PreparedRun, Self::BackendError> {
        let config = self.config.clone().ok_or(OpenMcError::NotConfigured)?;
        let read = |path: &PathBuf| -> Result<Vec<u8>, OpenMcError> {
            std::fs::read(path)
                .map_err(|error| OpenMcError::Io(format!("{}: {error}", path.display())))
        };
        let acceptance_json = config.acceptance.as_ref().map(&read).transpose()?;
        let assignment_json = config.material_assignment.as_ref().map(&read).transpose()?;
        let vr_json = config.variance_reduction.as_ref().map(&read).transpose()?;
        let deck = OpenMcInputDeck::generate(
            case,
            &config.nuclear_data_root,
            OpenMcInputArtifacts {
                component_profile_json: &read(&config.component_profile)?,
                material_json: &read(&config.material)?,
                source_json: &read(&config.source)?,
                response_set_json: &read(&config.response_set)?,
                nuclear_data_manifest_json: &read(&config.nuclear_data_manifest)?,
                execution_profile_json: &read(&config.execution_profile)?,
                acceptance_json: acceptance_json.as_deref(),
                material_assignment_json: assignment_json.as_deref(),
                variance_reduction_json: vr_json.as_deref(),
            },
        )?;
        deck.write_new(working_directory)?;
        Ok(PreparedRun {
            backend_id: "openmc".into(),
            case_id: case.case_id.clone(),
            working_directory: working_directory.display().to_string(),
        })
    }

    fn execute(&self, prepared: &PreparedRun) -> Result<CompletedRun, Self::BackendError> {
        if prepared.backend_id != "openmc" {
            return Err(OpenMcError::BackendMismatch(prepared.backend_id.clone()));
        }
        let directory = Path::new(&prepared.working_directory);
        let manifest_path =
            crate::statepoint::resolve_run_file(directory, OPENMC_INPUT_MANIFEST_FILE);
        let manifest_bytes = std::fs::read(&manifest_path)
            .map_err(|e| OpenMcError::Io(format!("{}: {e}", manifest_path.display())))?;
        let manifest: OpenMcInputManifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| OpenMcError::Manifest(format!("{e}")))?;

        let stdout_path = directory.join("openmc.stdout.log");
        let stderr_path = directory.join("openmc.stderr.log");
        let stdout = std::fs::File::create_new(&stdout_path)
            .map_err(|e| OpenMcError::Io(format!("{}: {e}", stdout_path.display())))?;
        let stderr = std::fs::File::create_new(&stderr_path)
            .map_err(|e| OpenMcError::Io(format!("{}: {e}", stderr_path.display())))?;

        let started = unix_seconds();
        let mut command = std::process::Command::new(&self.executable);
        command
            .current_dir(directory)
            .stdout(std::process::Stdio::from(stdout))
            .stderr(std::process::Stdio::from(stderr));
        for (key, value) in &self.environment {
            command.env(key, value);
        }
        let status = command
            .status()
            .map_err(|e| OpenMcError::Io(format!("spawn {}: {e}", self.executable.display())))?;
        let finished = unix_seconds();
        let exit_code = status.code().unwrap_or(-1);

        let hash_file = |path: &Path| -> Result<OpenMcRunArtifact, OpenMcError> {
            let bytes = std::fs::read(path)
                .map_err(|e| OpenMcError::Io(format!("{}: {e}", path.display())))?;
            Ok(OpenMcRunArtifact {
                path: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                sha256: crate::input::sha256_hex(&bytes),
                size_bytes: bytes.len() as u64,
            })
        };
        let executable_bytes = std::fs::read(&self.executable)
            .map_err(|e| OpenMcError::Io(format!("{}: {e}", self.executable.display())))?;
        let mut statepoints = Vec::new();
        for entry in std::fs::read_dir(directory)
            .map_err(|e| OpenMcError::Io(format!("{}: {e}", directory.display())))?
        {
            let path = entry
                .map_err(|e| OpenMcError::Io(format!("read_dir: {e}")))?
                .path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("statepoint.") && n.ends_with(".h5"))
            {
                statepoints.push(hash_file(&path)?);
            }
        }
        statepoints.sort_by(|a, b| a.path.cmp(&b.path));

        let receipt = OpenMcRunReceipt {
            schema_version: "openbnct.openmc-run-receipt/0.1.0".into(),
            backend_id: "openmc".into(),
            case_id: manifest.case_id.clone(),
            input_manifest_sha256: crate::input::sha256_hex(&manifest_bytes),
            executable: self.executable.display().to_string(),
            executable_sha256: crate::input::sha256_hex(&executable_bytes),
            environment_overlay: self.environment.clone(),
            started_unix_seconds: started,
            finished_unix_seconds: finished,
            exit_code,
            logs: vec![hash_file(&stdout_path)?, hash_file(&stderr_path)?],
            statepoints,
        };
        let receipt_path = directory.join(OPENMC_RUN_RECEIPT_FILE);
        let mut bytes = serde_json::to_vec_pretty(&receipt)
            .map_err(|e| OpenMcError::Manifest(e.to_string()))?;
        bytes.push(b'\n');
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&receipt_path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(&bytes).and_then(|_| f.sync_all())
            })
            .map_err(|e| OpenMcError::Io(format!("{}: {e}", receipt_path.display())))?;

        Ok(CompletedRun {
            backend_id: "openmc".into(),
            case_id: manifest.case_id,
            working_directory: prepared.working_directory.clone(),
            exit_code,
        })
    }

    fn collect(&self, completed: &CompletedRun) -> Result<PhysicalDoseBundle, Self::BackendError> {
        if completed.backend_id != "openmc" {
            return Err(OpenMcError::BackendMismatch(completed.backend_id.clone()));
        }
        Ok(collect_completed(completed)?)
    }
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, Error)]
pub enum OpenMcError {
    #[error("OpenMC adapter milestone not implemented: {0}")]
    NotImplemented(&'static str),
    #[error("completed run belongs to backend {0}, not openmc")]
    BackendMismatch(String),
    #[error("backend is not configured with input artifacts")]
    NotConfigured,
    #[error("io error: {0}")]
    Io(String),
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error(transparent)]
    Input(#[from] OpenMcInputError),
    #[error(transparent)]
    Collect(#[from] OpenMcCollectError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::tests::{
        COMPONENT_PROFILE_JSON, MATERIAL_JSON, PROFILE_JSON, SOURCE_JSON, case, input_bytes,
    };
    use std::io::Write;

    fn configured_backend(
        executable: &Path,
    ) -> (OpenMcBackend, tempfile::TempDir, tempfile::TempDir) {
        let inputs = input_bytes();
        let files = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = files.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let config = OpenMcBackendConfig {
            component_profile: write("component-profile.json", COMPONENT_PROFILE_JSON),
            material: write("material.json", MATERIAL_JSON),
            source: write("source.json", SOURCE_JSON),
            response_set: write("response-set.json", &inputs.response_set_json),
            nuclear_data_manifest: write("nuclear-data.json", &inputs.nuclear_data_json),
            execution_profile: write("profile.json", PROFILE_JSON),
            acceptance: None,
            material_assignment: None,
            variance_reduction: None,
            nuclear_data_root: inputs.data_root.path().to_path_buf(),
        };
        (
            OpenMcBackend::new(executable.to_path_buf()).configured(config),
            files,
            inputs.data_root,
        )
    }

    fn fake_executable(directory: &Path, exit_code: i32) -> PathBuf {
        let path = directory.join("fake-openmc.sh");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(format!("#!/bin/sh\necho fake run\nexit {exit_code}\n").as_bytes())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn advertises_no_unimplemented_capability() {
        let descriptor = OpenMcBackend::default().descriptor();
        assert_eq!(descriptor.id, "openmc");
        // Execution is available; deck preparation needs a configured
        // artifact set, so the default backend does not advertise it.
        assert!(descriptor.can_execute);
        assert!(!descriptor.can_prepare);
    }

    #[test]
    fn prepare_writes_deterministic_deck() {
        let temp = tempfile::tempdir().unwrap();
        let (backend, _files, _data) = configured_backend(&temp.path().join("openmc"));
        let first = temp.path().join("run-a");
        let second = temp.path().join("run-b");
        let prepared = backend.prepare(&case(), &first).unwrap();
        assert_eq!(prepared.backend_id, "openmc");
        backend.prepare(&case(), &second).unwrap();
        for entry in std::fs::read_dir(&first).unwrap() {
            let name = entry.unwrap().file_name();
            assert_eq!(
                std::fs::read(first.join(&name)).unwrap(),
                std::fs::read(second.join(&name)).unwrap(),
                "{} differs",
                name.to_string_lossy()
            );
        }
        assert!(first.join(OPENMC_INPUT_MANIFEST_FILE).is_file());
        // The deck directory must not already exist.
        assert!(matches!(
            backend.prepare(&case(), &first),
            Err(OpenMcError::Input(OpenMcInputError::Io { .. }))
        ));
    }

    #[test]
    fn execute_records_receipt_with_hashed_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let executable = fake_executable(temp.path(), 0);
        let (backend, _files, _data) = configured_backend(&executable);
        let run_dir = temp.path().join("run");
        let prepared = backend.prepare(&case(), &run_dir).unwrap();
        let completed = backend.execute(&prepared).unwrap();
        assert_eq!(completed.exit_code, 0);

        let receipt_path = run_dir.join(OPENMC_RUN_RECEIPT_FILE);
        let receipt: OpenMcRunReceipt =
            serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
        assert_eq!(receipt.schema_version, "openbnct.openmc-run-receipt/0.1.0");
        assert_eq!(receipt.case_id, "nf-bnct-001");
        assert_eq!(receipt.exit_code, 0);
        assert_eq!(receipt.logs.len(), 2);
        assert!(receipt.statepoints.is_empty());
        assert!(
            receipt
                .logs
                .iter()
                .any(|log| log.path == "openmc.stdout.log" && log.size_bytes > 0)
        );
        assert_eq!(receipt.executable_sha256.len(), 64);
        assert!(receipt.finished_unix_seconds >= receipt.started_unix_seconds);

        // A nonzero exit code is still recorded in a fresh run directory.
        let failing = fake_executable(temp.path(), 3);
        let (backend, _files, _data) = configured_backend(&failing);
        let run_dir = temp.path().join("run-fail");
        let prepared = backend.prepare(&case(), &run_dir).unwrap();
        let completed = backend.execute(&prepared).unwrap();
        assert_eq!(completed.exit_code, 3);
        let receipt: OpenMcRunReceipt =
            serde_json::from_slice(&std::fs::read(run_dir.join(OPENMC_RUN_RECEIPT_FILE)).unwrap())
                .unwrap();
        assert_eq!(receipt.exit_code, 3);
    }

    #[test]
    fn prepare_requires_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let backend = OpenMcBackend::default();
        assert!(matches!(
            backend.prepare(&case(), &temp.path().join("run")),
            Err(OpenMcError::NotConfigured)
        ));
    }
}
