// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use openbnct_core::ContentReference;
use openbnct_transport::ToolIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    NjoyInputBundle, NjoyNuclideRun, NjoyPreparationError, TARGET_NJOY_NAME,
    TARGET_NJOY_SOURCE_COMMIT, TARGET_NJOY_VERSION,
};

pub const NJOY_EXECUTION_RECEIPT_SCHEMA: &str = "openbnct.njoy-execution-receipt/0.1.0";
pub const NJOY_EXECUTION_RECEIPT_FILENAME: &str = "openbnct-njoy-execution-receipt.json";
pub const DEFAULT_NJOY_TIMEOUT_SECONDS: u64 = 3_600;

const PROCESSOR_ERROR_MARKER: &str = "***error in ";
const DIAGNOSTIC_REPORT_MARKER: &str = "final kerma factors";
const SPECIAL_KERMA_MT_RANGE: std::ops::RangeInclusive<u16> = 301..=449;

#[derive(Debug, Clone)]
pub struct NjoyExecutionOptions<'a> {
    pub executable: &'a Path,
    pub processor_support_artifacts: &'a [PathBuf],
    pub input_bundle_root: &'a Path,
    pub evaluations_root: &'a Path,
    pub output_root: &'a Path,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyExecutionReceipt {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub qualification: NjoyExecutionQualification,
    pub input_manifest: ContentReference,
    pub processor: NjoyProcessorExecutionIdentity,
    pub environment: NjoyExecutionEnvironment,
    pub runs: Vec<NjoyExecutionRun>,
    pub rejected_run_count: u64,
    pub completed_unix_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyExecutionQualification {
    ExecutionObservedUnreviewed,
    ExecutionObservedDiagnosticsFailed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyProcessorExecutionIdentity {
    pub tool: ToolIdentity,
    pub executable: NjoyProcessorArtifact,
    pub declared_support_artifacts: Vec<NjoyProcessorArtifact>,
    pub recognized_banner: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyProcessorArtifact {
    pub filename: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyExecutionEnvironment {
    pub operating_system: String,
    pub architecture: String,
    pub target_family: String,
    pub inherited_environment: bool,
    pub locale: String,
    pub timezone: String,
    pub timeout_seconds_per_nuclide: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyExecutionRun {
    pub nuclide: String,
    pub endf_mat: u16,
    pub exit_code: i32,
    pub input_deck: NjoyExecutionArtifact,
    pub evaluated_source: NjoyExecutionArtifact,
    pub standard_output: NjoyExecutionArtifact,
    pub standard_error: NjoyExecutionArtifact,
    pub processor_report: NjoyExecutionArtifact,
    pub output_tapes: Vec<NjoyExecutionTape>,
    pub required_special_mf3_mts: Vec<u16>,
    pub observed_special_mf3_mts: Vec<u16>,
    pub diagnostic_status: NjoyRunDiagnosticStatus,
    pub diagnostic_violation_count: u64,
    pub diagnostic_violations: Vec<NjoyKinematicViolation>,
    pub production_diagnostic_pendf_identical: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyRunDiagnosticStatus {
    WithinKinematicLimits,
    KinematicLimitsExceeded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyKinematicViolation {
    pub energy_ev: f64,
    pub response_mt: u16,
    pub direction: NjoyKinematicDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyKinematicDirection {
    Low,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyExecutionTape {
    pub unit: u16,
    pub purpose: NjoyTapePurpose,
    pub artifact: NjoyExecutionArtifact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NjoyTapePurpose {
    ReconstructedPendf,
    BroadenedPendf,
    ProductionHeatrPendf,
    DiagnosticHeatrPendf,
    DiagnosticPlot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NjoyExecutionArtifact {
    pub path: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyExecutionResult {
    pub receipt: NjoyExecutionReceipt,
    pub receipt_path: PathBuf,
    pub receipt_sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NjoyExecutionReceiptDocument {
    pub receipt: NjoyExecutionReceipt,
    pub sha256: String,
}

impl NjoyExecutionReceiptDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NjoyExecutionError> {
        let receipt: NjoyExecutionReceipt = serde_json::from_slice(bytes)?;
        receipt.validate()?;
        Ok(Self {
            receipt,
            sha256: sha256_bytes(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, NjoyExecutionError> {
        let bytes = read_regular_file(path)?;
        Self::from_bytes(&bytes)
    }

    /// Verify the complete execution directory against this receipt. The
    /// receipt itself is part of the exact file set and must be byte-identical
    /// to the document used as the trust anchor.
    pub fn verify_execution_root(&self, root: &Path) -> Result<(), NjoyExecutionError> {
        self.receipt.validate()?;
        let root_metadata =
            fs::symlink_metadata(root).map_err(|source| NjoyExecutionError::Io {
                path: root.to_path_buf(),
                source,
            })?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(NjoyExecutionError::InvalidExecutionRoot(root.to_path_buf()));
        }

        validate_output_root(root, &self.receipt)?;
        let receipt_path = root.join(NJOY_EXECUTION_RECEIPT_FILENAME);
        if sha256_bytes(&read_regular_file(&receipt_path)?) != self.sha256 {
            return Err(NjoyExecutionError::ExecutionReceiptMismatch);
        }

        for run in &self.receipt.runs {
            let run_root = root.join(&run.nuclide);
            validate_run_file_set(&run_root)?;
            for artifact in run_artifacts(run) {
                verify_execution_artifact(root, artifact)?;
            }
        }
        Ok(())
    }
}

impl NjoyExecutionReceipt {
    /// Execute every prepared nuclide sequentially in a fresh evidence root.
    /// The receipt remains unreviewed even when every mechanical gate passes.
    pub fn execute(
        bundle: &NjoyInputBundle,
        options: NjoyExecutionOptions<'_>,
    ) -> Result<NjoyExecutionResult, NjoyExecutionError> {
        if options.timeout_seconds == 0 {
            return Err(NjoyExecutionError::ZeroTimeout);
        }
        bundle.verify_directory(options.input_bundle_root)?;
        validate_output_location(
            options.output_root,
            options.input_bundle_root,
            options.evaluations_root,
        )?;

        let executable = processor_artifact(options.executable)?;
        let executable_path = canonical_regular_file(options.executable)?;
        let mut support_artifacts = options
            .processor_support_artifacts
            .iter()
            .map(|path| processor_artifact(path))
            .collect::<Result<Vec<_>, _>>()?;
        support_artifacts.sort_by(|left, right| {
            left.filename
                .cmp(&right.filename)
                .then(left.sha256.cmp(&right.sha256))
        });
        if support_artifacts
            .windows(2)
            .any(|pair| pair[0].filename == pair[1].filename)
        {
            return Err(NjoyExecutionError::DuplicateProcessorSupportArtifact);
        }

        let input_manifest_file = bundle
            .files
            .iter()
            .find(|file| file.relative_path == "openbnct-njoy-input-manifest.json")
            .ok_or(NjoyExecutionError::MissingInputManifest)?;
        let input_manifest = ContentReference {
            id: bundle.manifest.id.clone(),
            sha256: input_manifest_file.sha256.clone(),
        };

        fs::create_dir(options.output_root).map_err(|source| NjoyExecutionError::Io {
            path: options.output_root.to_path_buf(),
            source,
        })?;

        let timeout = Duration::from_secs(options.timeout_seconds);
        let mut recognized_banner: Option<String> = None;
        let mut runs = Vec::with_capacity(bundle.manifest.runs.len());
        for run in &bundle.manifest.runs {
            let (executed, banner) = execute_run(
                bundle,
                run,
                &executable_path,
                options.evaluations_root,
                options.output_root,
                timeout,
            )?;
            if let Some(expected) = &recognized_banner {
                if expected != &banner {
                    return Err(NjoyExecutionError::InconsistentProcessorBanner {
                        expected: expected.clone(),
                        observed: banner,
                    });
                }
            } else {
                recognized_banner = Some(banner);
            }
            runs.push(executed);
        }

        let final_executable = processor_artifact(&executable_path)?;
        let mut final_support_artifacts = options
            .processor_support_artifacts
            .iter()
            .map(|path| processor_artifact(path))
            .collect::<Result<Vec<_>, _>>()?;
        final_support_artifacts.sort_by(|left, right| {
            left.filename
                .cmp(&right.filename)
                .then(left.sha256.cmp(&right.sha256))
        });
        if final_executable != executable || final_support_artifacts != support_artifacts {
            return Err(NjoyExecutionError::ProcessorArtifactChanged);
        }

        let rejected_run_count = runs
            .iter()
            .filter(|run| run.diagnostic_status == NjoyRunDiagnosticStatus::KinematicLimitsExceeded)
            .count() as u64;
        let qualification = if rejected_run_count == 0 {
            NjoyExecutionQualification::ExecutionObservedUnreviewed
        } else {
            NjoyExecutionQualification::ExecutionObservedDiagnosticsFailed
        };
        let completed_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| NjoyExecutionError::SystemClockBeforeEpoch)?
            .as_secs();
        let receipt = Self {
            schema_version: NJOY_EXECUTION_RECEIPT_SCHEMA.into(),
            id: format!(
                "{}.execution.{}.{:.12}",
                bundle.manifest.id, completed_unix_seconds, executable.sha256
            ),
            case_id: bundle.manifest.case_id.clone(),
            qualification,
            input_manifest,
            processor: NjoyProcessorExecutionIdentity {
                tool: bundle.manifest.processor.clone(),
                executable,
                declared_support_artifacts: support_artifacts,
                recognized_banner: recognized_banner.ok_or(NjoyExecutionError::NoNuclideRuns)?,
            },
            environment: NjoyExecutionEnvironment {
                operating_system: std::env::consts::OS.into(),
                architecture: std::env::consts::ARCH.into(),
                target_family: std::env::consts::FAMILY.into(),
                inherited_environment: false,
                locale: "C".into(),
                timezone: "UTC".into(),
                timeout_seconds_per_nuclide: options.timeout_seconds,
            },
            runs,
            rejected_run_count,
            completed_unix_seconds,
        };
        receipt.validate()?;
        let mut receipt_bytes = serde_json::to_vec_pretty(&receipt)?;
        receipt_bytes.push(b'\n');
        let receipt_path = options.output_root.join(NJOY_EXECUTION_RECEIPT_FILENAME);
        write_new_bytes(&receipt_path, &receipt_bytes)?;
        let document = NjoyExecutionReceiptDocument::from_bytes(&receipt_bytes)?;
        document.verify_execution_root(options.output_root)?;

        Ok(NjoyExecutionResult {
            receipt,
            receipt_path,
            receipt_sha256: document.sha256,
        })
    }

    pub fn validate(&self) -> Result<(), NjoyExecutionError> {
        if !openbnct_core::schema_matches(&self.schema_version, NJOY_EXECUTION_RECEIPT_SCHEMA) {
            return invalid_receipt(format!("unsupported schema {:?}", self.schema_version));
        }
        validate_identifier("id", &self.id)?;
        validate_identifier("case_id", &self.case_id)?;
        validate_identifier("input_manifest.id", &self.input_manifest.id)?;
        validate_sha256("input_manifest.sha256", &self.input_manifest.sha256)?;
        if self.completed_unix_seconds == 0 {
            return invalid_receipt("completed_unix_seconds must be positive");
        }

        let processor = &self.processor;
        if processor.tool.name != TARGET_NJOY_NAME
            || processor.tool.version != TARGET_NJOY_VERSION
            || processor.tool.source_commit != TARGET_NJOY_SOURCE_COMMIT
        {
            return invalid_receipt("processor identity is not the frozen NJOY target");
        }
        if processor.recognized_banner != format!("njoy {TARGET_NJOY_VERSION}") {
            return invalid_receipt("processor banner is not the frozen NJOY version");
        }
        validate_processor_artifact("processor.executable", &processor.executable)?;
        let expected_id = format!(
            "{}.execution.{}.{:.12}",
            self.input_manifest.id, self.completed_unix_seconds, self.processor.executable.sha256
        );
        if self.id != expected_id {
            return invalid_receipt("receipt ID does not bind its input, time, and processor");
        }
        let mut previous_support_filename: Option<&str> = None;
        for artifact in &processor.declared_support_artifacts {
            validate_processor_artifact("processor.declared_support_artifacts", artifact)?;
            if previous_support_filename
                .is_some_and(|previous| previous >= artifact.filename.as_str())
            {
                return invalid_receipt(
                    "declared processor support artifacts are not strictly ordered",
                );
            }
            previous_support_filename = Some(&artifact.filename);
        }

        let environment = &self.environment;
        for (label, value) in [
            (
                "environment.operating_system",
                environment.operating_system.as_str(),
            ),
            (
                "environment.architecture",
                environment.architecture.as_str(),
            ),
            (
                "environment.target_family",
                environment.target_family.as_str(),
            ),
        ] {
            validate_identifier(label, value)?;
        }
        if environment.inherited_environment
            || environment.locale != "C"
            || environment.timezone != "UTC"
            || environment.timeout_seconds_per_nuclide == 0
        {
            return invalid_receipt("execution environment is not controlled");
        }

        if self.runs.is_empty() {
            return Err(NjoyExecutionError::NoNuclideRuns);
        }
        let mut previous_nuclide: Option<&str> = None;
        let mut rejected_run_count = 0_u64;
        for run in &self.runs {
            validate_nuclide_name(&run.nuclide)?;
            if previous_nuclide.is_some_and(|previous| previous >= run.nuclide.as_str()) {
                return invalid_receipt("nuclide runs are not strictly ordered");
            }
            previous_nuclide = Some(&run.nuclide);
            validate_execution_run(run)?;
            if run.diagnostic_status == NjoyRunDiagnosticStatus::KinematicLimitsExceeded {
                rejected_run_count += 1;
            }
        }
        if rejected_run_count != self.rejected_run_count {
            return invalid_receipt("rejected run count does not match run diagnostics");
        }
        let expected_qualification = if rejected_run_count == 0 {
            NjoyExecutionQualification::ExecutionObservedUnreviewed
        } else {
            NjoyExecutionQualification::ExecutionObservedDiagnosticsFailed
        };
        if self.qualification != expected_qualification {
            return invalid_receipt("qualification does not match run diagnostics");
        }
        Ok(())
    }
}

fn validate_execution_run(run: &NjoyExecutionRun) -> Result<(), NjoyExecutionError> {
    if run.endf_mat == 0 || run.exit_code != 0 {
        return invalid_receipt(format!(
            "run {} has an invalid MAT or exit code",
            run.nuclide
        ));
    }
    let prefix = &run.nuclide;
    validate_execution_artifact(
        &run.input_deck,
        &format!("{prefix}/input.njoy"),
        "text/plain",
        false,
    )?;
    validate_execution_artifact(
        &run.evaluated_source,
        &format!("{prefix}/tape20"),
        "application/x-endf",
        false,
    )?;
    validate_execution_artifact(
        &run.standard_output,
        &format!("{prefix}/stdout.log"),
        "text/plain",
        false,
    )?;
    validate_execution_artifact(
        &run.standard_error,
        &format!("{prefix}/stderr.log"),
        "text/plain",
        true,
    )?;
    if run.standard_error.size_bytes != 0 || run.standard_error.sha256 != sha256_bytes(&[]) {
        return invalid_receipt(format!(
            "run {} does not bind empty standard error",
            run.nuclide
        ));
    }
    validate_execution_artifact(
        &run.processor_report,
        &format!("{prefix}/output"),
        "text/plain",
        false,
    )?;

    let expected_tapes = [
        (
            21,
            NjoyTapePurpose::ReconstructedPendf,
            "application/x-endf",
        ),
        (22, NjoyTapePurpose::BroadenedPendf, "application/x-endf"),
        (
            23,
            NjoyTapePurpose::ProductionHeatrPendf,
            "application/x-endf",
        ),
        (
            24,
            NjoyTapePurpose::DiagnosticHeatrPendf,
            "application/x-endf",
        ),
        (25, NjoyTapePurpose::DiagnosticPlot, "text/plain"),
    ];
    if run.output_tapes.len() != expected_tapes.len() {
        return invalid_receipt(format!(
            "run {} does not declare the exact tape set",
            run.nuclide
        ));
    }
    for (tape, (unit, purpose, media_type)) in run.output_tapes.iter().zip(expected_tapes) {
        if tape.unit != unit || tape.purpose != purpose {
            return invalid_receipt(format!(
                "run {} has a noncanonical tape declaration",
                run.nuclide
            ));
        }
        validate_execution_artifact(
            &tape.artifact,
            &format!("{prefix}/tape{unit}"),
            media_type,
            false,
        )?;
    }

    if run.required_special_mf3_mts != run.observed_special_mf3_mts
        || run.required_special_mf3_mts.len() < 2
        || run
            .required_special_mf3_mts
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        || !run.required_special_mf3_mts.contains(&301)
        || !run.required_special_mf3_mts.contains(&443)
    {
        return invalid_receipt(format!(
            "run {} has an invalid special-MF3 section set",
            run.nuclide
        ));
    }
    if !run.production_diagnostic_pendf_identical
        || run.output_tapes[2].artifact.size_bytes != run.output_tapes[3].artifact.size_bytes
        || run.output_tapes[2].artifact.sha256 != run.output_tapes[3].artifact.sha256
    {
        return invalid_receipt(format!(
            "run {} does not prove identical production and diagnostic PENDF",
            run.nuclide
        ));
    }

    if run.diagnostic_violation_count != run.diagnostic_violations.len() as u64 {
        return invalid_receipt(format!(
            "run {} diagnostic count does not match its structured records",
            run.nuclide
        ));
    }
    let expected_status = if run.diagnostic_violations.is_empty() {
        NjoyRunDiagnosticStatus::WithinKinematicLimits
    } else {
        NjoyRunDiagnosticStatus::KinematicLimitsExceeded
    };
    if run.diagnostic_status != expected_status {
        return invalid_receipt(format!(
            "run {} diagnostic status does not match its structured records",
            run.nuclide
        ));
    }
    let mut previous_violation = None;
    for violation in &run.diagnostic_violations {
        if !violation.energy_ev.is_finite()
            || violation.energy_ev <= 0.0
            || !run
                .required_special_mf3_mts
                .contains(&violation.response_mt)
        {
            return invalid_receipt(format!(
                "run {} contains an invalid kinematic violation",
                run.nuclide
            ));
        }
        let direction_order = match violation.direction {
            NjoyKinematicDirection::Low => 0_u8,
            NjoyKinematicDirection::High => 1_u8,
        };
        let current = (
            violation.energy_ev.to_bits(),
            violation.response_mt,
            direction_order,
        );
        if previous_violation.is_some_and(|previous| previous >= current) {
            return invalid_receipt(format!(
                "run {} kinematic violations are not strictly ordered",
                run.nuclide
            ));
        }
        previous_violation = Some(current);
    }
    Ok(())
}

fn validate_execution_artifact(
    artifact: &NjoyExecutionArtifact,
    expected_path: &str,
    expected_media_type: &str,
    allow_empty: bool,
) -> Result<(), NjoyExecutionError> {
    if artifact.path != expected_path
        || artifact.media_type != expected_media_type
        || (!allow_empty && artifact.size_bytes == 0)
    {
        return invalid_receipt(format!("invalid execution artifact {expected_path:?}"));
    }
    validate_normalized_relative_path(&artifact.path)?;
    validate_sha256("execution artifact sha256", &artifact.sha256)
}

fn validate_processor_artifact(
    label: &'static str,
    artifact: &NjoyProcessorArtifact,
) -> Result<(), NjoyExecutionError> {
    validate_filename(label, &artifact.filename)?;
    if artifact.size_bytes == 0 {
        return invalid_receipt(format!("{label} is empty"));
    }
    validate_sha256(label, &artifact.sha256)
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), NjoyExecutionError> {
    if value.trim().is_empty() {
        invalid_receipt(format!("{label} is empty"))
    } else {
        Ok(())
    }
}

fn validate_filename(label: &'static str, value: &str) -> Result<(), NjoyExecutionError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        invalid_receipt(format!("{label} is not a filename"))
    } else {
        Ok(())
    }
}

fn validate_nuclide_name(value: &str) -> Result<(), NjoyExecutionError> {
    validate_filename("run.nuclide", value)?;
    if value.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        invalid_receipt("run.nuclide contains unsupported characters")
    }
}

fn validate_normalized_relative_path(value: &str) -> Result<(), NjoyExecutionError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        invalid_receipt(format!(
            "execution artifact path is not normalized and relative: {value:?}"
        ))
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &'static str, value: &str) -> Result<(), NjoyExecutionError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid_receipt(format!("{label} is not a canonical lowercase SHA-256"))
    }
}

fn invalid_receipt<T>(message: impl Into<String>) -> Result<T, NjoyExecutionError> {
    Err(NjoyExecutionError::InvalidReceipt(message.into()))
}

fn run_artifacts(run: &NjoyExecutionRun) -> impl Iterator<Item = &NjoyExecutionArtifact> {
    [
        &run.input_deck,
        &run.evaluated_source,
        &run.standard_output,
        &run.standard_error,
        &run.processor_report,
    ]
    .into_iter()
    .chain(run.output_tapes.iter().map(|tape| &tape.artifact))
}

fn verify_execution_artifact(
    root: &Path,
    artifact: &NjoyExecutionArtifact,
) -> Result<(), NjoyExecutionError> {
    let path = root.join(&artifact.path);
    let bytes = read_regular_file(&path)?;
    if bytes.len() as u64 != artifact.size_bytes || sha256_bytes(&bytes) != artifact.sha256 {
        return Err(NjoyExecutionError::ExecutionArtifactMismatch(
            artifact.path.clone(),
        ));
    }
    Ok(())
}

fn execute_run(
    bundle: &NjoyInputBundle,
    run: &NjoyNuclideRun,
    executable: &Path,
    evaluations_root: &Path,
    output_root: &Path,
    timeout: Duration,
) -> Result<(NjoyExecutionRun, String), NjoyExecutionError> {
    let run_root = output_root.join(&run.nuclide);
    fs::create_dir(&run_root).map_err(|source| NjoyExecutionError::Io {
        path: run_root.clone(),
        source,
    })?;
    let run_tmp = fs::canonicalize(&run_root).map_err(|source| NjoyExecutionError::Io {
        path: run_root.clone(),
        source,
    })?;

    let generated_deck = bundle
        .files
        .iter()
        .find(|file| file.relative_path == run.input_deck.path)
        .ok_or_else(|| NjoyExecutionError::MissingInputDeck(run.nuclide.clone()))?;
    let deck_path = run_root.join("input.njoy");
    write_new_bytes(&deck_path, &generated_deck.bytes)?;

    let selected_source = evaluations_root.join(&run.source_evaluation.filename);
    verify_selected_source(&selected_source, run)?;
    let tape20_path = run_root.join("tape20");
    copy_new(&selected_source, &tape20_path)?;
    verify_selected_source(&tape20_path, run)?;

    let stdout_path = run_root.join("stdout.log");
    let stderr_path = run_root.join("stderr.log");
    let stdout_file = create_new_file(&stdout_path)?;
    let stderr_file = create_new_file(&stderr_path)?;
    let stdin_file = File::open(&deck_path).map_err(|source| NjoyExecutionError::Io {
        path: deck_path.clone(),
        source,
    })?;

    let mut command = Command::new(executable);
    command
        .current_dir(&run_root)
        .stdin(Stdio::from(stdin_file))
        .stdout(Stdio::from(stdout_file.try_clone().map_err(|source| {
            NjoyExecutionError::Io {
                path: stdout_path.clone(),
                source,
            }
        })?))
        .stderr(Stdio::from(stderr_file.try_clone().map_err(|source| {
            NjoyExecutionError::Io {
                path: stderr_path.clone(),
                source,
            }
        })?))
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("TZ", "UTC")
        .env("TMPDIR", &run_tmp);
    let mut child = command.spawn().map_err(|source| NjoyExecutionError::Io {
        path: executable.to_path_buf(),
        source,
    })?;
    let status = wait_for_child(&mut child, &run.nuclide, timeout)?;
    stdout_file
        .sync_all()
        .map_err(|source| NjoyExecutionError::Io {
            path: stdout_path.clone(),
            source,
        })?;
    stderr_file
        .sync_all()
        .map_err(|source| NjoyExecutionError::Io {
            path: stderr_path.clone(),
            source,
        })?;
    let exit_code = status
        .code()
        .ok_or_else(|| NjoyExecutionError::ProcessorTerminatedBySignal(run.nuclide.clone()))?;
    if !status.success() {
        return Err(NjoyExecutionError::ProcessorFailure {
            nuclide: run.nuclide.clone(),
            exit_code,
        });
    }

    let stdout_bytes = read_regular_file(&stdout_path)?;
    let stderr_bytes = read_regular_file(&stderr_path)?;
    let stdout_text = std::str::from_utf8(&stdout_bytes)
        .map_err(|_| NjoyExecutionError::NonUtf8ProcessorText(stdout_path.clone()))?;
    if !stderr_bytes.is_empty() {
        return Err(NjoyExecutionError::NonemptyStandardError(
            run.nuclide.clone(),
        ));
    }
    let banner = recognized_banner(stdout_text)
        .ok_or_else(|| NjoyExecutionError::UnrecognizedProcessorBanner(run.nuclide.clone()))?;

    let report_path = run_root.join("output");
    let report_bytes = read_regular_file(&report_path)?;
    let report_text = std::str::from_utf8(&report_bytes)
        .map_err(|_| NjoyExecutionError::NonUtf8ProcessorText(report_path.clone()))?;
    if stdout_text.contains(PROCESSOR_ERROR_MARKER) || report_text.contains(PROCESSOR_ERROR_MARKER)
    {
        return Err(NjoyExecutionError::ProcessorErrorMarker(
            run.nuclide.clone(),
        ));
    }
    if !report_text.contains(DIAGNOSTIC_REPORT_MARKER) {
        return Err(NjoyExecutionError::MissingDiagnosticReport(
            run.nuclide.clone(),
        ));
    }
    let diagnostic_violations = kinematic_violations(report_text, &run.generated_kerma_mts)?;
    let diagnostic_violation_count = diagnostic_violations.len() as u64;
    let diagnostic_status = if diagnostic_violations.is_empty() {
        NjoyRunDiagnosticStatus::WithinKinematicLimits
    } else {
        NjoyRunDiagnosticStatus::KinematicLimitsExceeded
    };

    let tape23_path = run_root.join("tape23");
    let tape24_path = run_root.join("tape24");
    let tape23 = read_regular_file(&tape23_path)?;
    let tape24 = read_regular_file(&tape24_path)?;
    let expected_special = run
        .generated_kerma_mts
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let observed_production = special_mf3_mts(&tape23);
    let observed_diagnostic = special_mf3_mts(&tape24);
    if observed_production != expected_special {
        return Err(NjoyExecutionError::SpecialMf3Mismatch {
            nuclide: run.nuclide.clone(),
            expected: expected_special.iter().copied().collect(),
            observed: observed_production.iter().copied().collect(),
        });
    }
    if observed_diagnostic != expected_special {
        return Err(NjoyExecutionError::DiagnosticSpecialMf3Mismatch {
            nuclide: run.nuclide.clone(),
            expected: expected_special.iter().copied().collect(),
            observed: observed_diagnostic.iter().copied().collect(),
        });
    }
    if tape23 != tape24 {
        return Err(NjoyExecutionError::ProductionDiagnosticPendfMismatch(
            run.nuclide.clone(),
        ));
    }

    validate_run_file_set(&run_root)?;
    let input_deck = artifact_from_file(output_root, &deck_path, "text/plain")?;
    let evaluated_source = artifact_from_file(output_root, &tape20_path, "application/x-endf")?;
    let standard_output = artifact_from_file(output_root, &stdout_path, "text/plain")?;
    let standard_error = artifact_from_file(output_root, &stderr_path, "text/plain")?;
    let processor_report = artifact_from_file(output_root, &report_path, "text/plain")?;
    let output_tapes = [
        (
            21,
            NjoyTapePurpose::ReconstructedPendf,
            "application/x-endf",
        ),
        (22, NjoyTapePurpose::BroadenedPendf, "application/x-endf"),
        (
            23,
            NjoyTapePurpose::ProductionHeatrPendf,
            "application/x-endf",
        ),
        (
            24,
            NjoyTapePurpose::DiagnosticHeatrPendf,
            "application/x-endf",
        ),
        (25, NjoyTapePurpose::DiagnosticPlot, "text/plain"),
    ]
    .into_iter()
    .map(|(unit, purpose, media_type)| {
        let path = run_root.join(format!("tape{unit}"));
        Ok(NjoyExecutionTape {
            unit,
            purpose,
            artifact: artifact_from_file(output_root, &path, media_type)?,
        })
    })
    .collect::<Result<Vec<_>, NjoyExecutionError>>()?;

    Ok((
        NjoyExecutionRun {
            nuclide: run.nuclide.clone(),
            endf_mat: run.endf_mat,
            exit_code,
            input_deck,
            evaluated_source,
            standard_output,
            standard_error,
            processor_report,
            output_tapes,
            required_special_mf3_mts: expected_special.iter().copied().collect(),
            observed_special_mf3_mts: observed_production.iter().copied().collect(),
            diagnostic_status,
            diagnostic_violation_count,
            diagnostic_violations,
            production_diagnostic_pendf_identical: true,
        },
        banner,
    ))
}

fn wait_for_child(
    child: &mut std::process::Child,
    nuclide: &str,
    timeout: Duration,
) -> Result<ExitStatus, NjoyExecutionError> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|source| NjoyExecutionError::Io {
            path: PathBuf::from("NJOY child process"),
            source,
        })? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            child.kill().map_err(|source| NjoyExecutionError::Io {
                path: PathBuf::from("NJOY child process"),
                source,
            })?;
            let _ = child.wait();
            return Err(NjoyExecutionError::ProcessorTimeout {
                nuclide: nuclide.into(),
                timeout_seconds: timeout.as_secs(),
            });
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn recognized_banner(stdout: &str) -> Option<String> {
    stdout.lines().map(str::trim).find_map(|line| {
        line.starts_with(&format!("njoy {TARGET_NJOY_VERSION}"))
            .then(|| format!("njoy {TARGET_NJOY_VERSION}"))
    })
}

fn kinematic_violations(
    report: &str,
    response_mts: &[u16],
) -> Result<Vec<NjoyKinematicViolation>, NjoyExecutionError> {
    let diagnostic = report
        .rfind(DIAGNOSTIC_REPORT_MARKER)
        .map(|index| &report[index + DIAGNOSTIC_REPORT_MARKER.len()..])
        .unwrap_or("");
    let marker_count = diagnostic
        .split_ascii_whitespace()
        .filter(|token| *token == "low" || *token == "high")
        .count();
    let mut last_energy = None;
    let mut violations = Vec::with_capacity(marker_count);
    for line in diagnostic.lines() {
        if let Some(first) = line.split_ascii_whitespace().next()
            && let Ok(energy) = first.parse::<f64>()
        {
            last_energy = Some(energy);
        }
        for (index, response_mt) in response_mts.iter().copied().enumerate() {
            let marker_start = 21 + 14 * index;
            let marker_end = marker_start + 4;
            let Some(marker) = line.get(marker_start..marker_end) else {
                continue;
            };
            let direction = match marker.trim() {
                "low" => NjoyKinematicDirection::Low,
                "high" => NjoyKinematicDirection::High,
                _ => continue,
            };
            violations.push(NjoyKinematicViolation {
                energy_ev: last_energy.ok_or(NjoyExecutionError::UnparsedKinematicDiagnostic)?,
                response_mt,
                direction,
            });
        }
    }
    if violations.len() != marker_count {
        return Err(NjoyExecutionError::UnparsedKinematicDiagnostic);
    }
    Ok(violations)
}

fn special_mf3_mts(bytes: &[u8]) -> BTreeSet<u16> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter_map(|line| {
            if line.len() < 75 {
                return None;
            }
            let mf = fixed_width_u16(&line[70..72])?;
            let mt = fixed_width_u16(&line[72..75])?;
            (mf == 3 && SPECIAL_KERMA_MT_RANGE.contains(&mt)).then_some(mt)
        })
        .collect()
}

fn fixed_width_u16(value: &[u8]) -> Option<u16> {
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}

fn verify_selected_source(path: &Path, run: &NjoyNuclideRun) -> Result<(), NjoyExecutionError> {
    let bytes = read_regular_file(path)?;
    if bytes.len() as u64 != run.source_evaluation.size_bytes
        || sha256_bytes(&bytes) != run.source_evaluation.sha256
    {
        return Err(NjoyExecutionError::SelectedSourceMismatch(
            run.nuclide.clone(),
        ));
    }
    Ok(())
}

fn processor_artifact(path: &Path) -> Result<NjoyProcessorArtifact, NjoyExecutionError> {
    let canonical = canonical_regular_file(path)?;
    let bytes = read_regular_file(&canonical)?;
    let filename = canonical
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| NjoyExecutionError::InvalidProcessorArtifact(path.to_path_buf()))?;
    Ok(NjoyProcessorArtifact {
        filename: filename.into(),
        size_bytes: bytes.len() as u64,
        sha256: sha256_bytes(&bytes),
    })
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, NjoyExecutionError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| NjoyExecutionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NjoyExecutionError::InvalidProcessorArtifact(
            path.to_path_buf(),
        ));
    }
    path.canonicalize()
        .map_err(|source| NjoyExecutionError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn validate_run_file_set(run_root: &Path) -> Result<(), NjoyExecutionError> {
    let expected = BTreeSet::from([
        "input.njoy",
        "output",
        "stderr.log",
        "stdout.log",
        "tape20",
        "tape21",
        "tape22",
        "tape23",
        "tape24",
        "tape25",
    ]);
    let mut observed = BTreeSet::new();
    for entry in fs::read_dir(run_root).map_err(|source| NjoyExecutionError::Io {
        path: run_root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| NjoyExecutionError::Io {
            path: run_root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| NjoyExecutionError::Io {
            path: path.clone(),
            source,
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| NjoyExecutionError::UnexpectedRunArtifact(path.clone()))?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || !expected.contains(name.as_str())
        {
            return Err(NjoyExecutionError::UnexpectedRunArtifact(path));
        }
        observed.insert(name);
    }
    if observed.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err(NjoyExecutionError::RunFileSetMismatch(
            run_root.to_path_buf(),
        ));
    }
    Ok(())
}

fn validate_output_location(
    output_root: &Path,
    input_bundle_root: &Path,
    evaluations_root: &Path,
) -> Result<(), NjoyExecutionError> {
    let filename = output_root
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| NjoyExecutionError::UnsafeOutputRoot(output_root.to_path_buf()))?;
    let parent = output_root.parent().unwrap_or_else(|| Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|source| NjoyExecutionError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    let resolved_output = parent.join(filename);
    let input_bundle_root =
        input_bundle_root
            .canonicalize()
            .map_err(|source| NjoyExecutionError::Io {
                path: input_bundle_root.to_path_buf(),
                source,
            })?;
    let evaluations_root =
        evaluations_root
            .canonicalize()
            .map_err(|source| NjoyExecutionError::Io {
                path: evaluations_root.to_path_buf(),
                source,
            })?;
    if resolved_output.starts_with(&input_bundle_root)
        || resolved_output.starts_with(&evaluations_root)
        || input_bundle_root.starts_with(&resolved_output)
        || evaluations_root.starts_with(&resolved_output)
    {
        return Err(NjoyExecutionError::UnsafeOutputRoot(resolved_output));
    }
    Ok(())
}

fn validate_output_root(
    output_root: &Path,
    receipt: &NjoyExecutionReceipt,
) -> Result<(), NjoyExecutionError> {
    let expected = receipt
        .runs
        .iter()
        .map(|run| run.nuclide.as_str())
        .chain(std::iter::once(NJOY_EXECUTION_RECEIPT_FILENAME))
        .collect::<BTreeSet<_>>();
    let mut observed = BTreeSet::new();
    for entry in fs::read_dir(output_root).map_err(|source| NjoyExecutionError::Io {
        path: output_root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| NjoyExecutionError::Io {
            path: output_root.to_path_buf(),
            source,
        })?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| NjoyExecutionError::UnexpectedRunArtifact(entry.path()))?;
        let metadata =
            fs::symlink_metadata(entry.path()).map_err(|source| NjoyExecutionError::Io {
                path: entry.path(),
                source,
            })?;
        let expected_type = if name == NJOY_EXECUTION_RECEIPT_FILENAME {
            metadata.is_file() && !metadata.file_type().is_symlink()
        } else {
            metadata.is_dir() && !metadata.file_type().is_symlink()
        };
        if !expected.contains(name.as_str()) || !expected_type {
            return Err(NjoyExecutionError::UnexpectedRunArtifact(entry.path()));
        }
        observed.insert(name);
    }
    if observed.iter().map(String::as_str).collect::<BTreeSet<_>>() != expected {
        return Err(NjoyExecutionError::ExecutionRootFileSetMismatch);
    }
    Ok(())
}

fn artifact_from_file(
    output_root: &Path,
    path: &Path,
    media_type: &str,
) -> Result<NjoyExecutionArtifact, NjoyExecutionError> {
    let bytes = read_regular_file(path)?;
    if bytes.is_empty() && path.file_name().and_then(|name| name.to_str()) != Some("stderr.log") {
        return Err(NjoyExecutionError::EmptyRunArtifact(path.to_path_buf()));
    }
    let relative = path
        .strip_prefix(output_root)
        .map_err(|_| NjoyExecutionError::UnexpectedRunArtifact(path.to_path_buf()))?;
    let relative = relative
        .to_str()
        .ok_or_else(|| NjoyExecutionError::UnexpectedRunArtifact(relative.to_path_buf()))?
        .replace('\\', "/");
    Ok(NjoyExecutionArtifact {
        path: relative,
        media_type: media_type.into(),
        size_bytes: bytes.len() as u64,
        sha256: sha256_bytes(&bytes),
    })
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, NjoyExecutionError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| NjoyExecutionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(NjoyExecutionError::UnexpectedRunArtifact(
            path.to_path_buf(),
        ));
    }
    fs::read(path).map_err(|source| NjoyExecutionError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn copy_new(source: &Path, destination: &Path) -> Result<(), NjoyExecutionError> {
    let mut input = File::open(source).map_err(|source_error| NjoyExecutionError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let mut output = create_new_file(destination)?;
    io::copy(&mut input, &mut output).map_err(|source_error| NjoyExecutionError::Io {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    output
        .sync_all()
        .map_err(|source_error| NjoyExecutionError::Io {
            path: destination.to_path_buf(),
            source: source_error,
        })
}

fn write_new_bytes(path: &Path, bytes: &[u8]) -> Result<(), NjoyExecutionError> {
    let mut file = create_new_file(path)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| NjoyExecutionError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn create_new_file(path: &Path) -> Result<File, NjoyExecutionError> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| NjoyExecutionError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Error)]
pub enum NjoyExecutionError {
    #[error(transparent)]
    Preparation(#[from] NjoyPreparationError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("NJOY execution timeout must be greater than zero")]
    ZeroTimeout,
    #[error("NJOY input bundle contains no input manifest")]
    MissingInputManifest,
    #[error("NJOY input manifest contains no nuclide runs")]
    NoNuclideRuns,
    #[error("invalid NJOY execution receipt: {0}")]
    InvalidReceipt(String),
    #[error("NJOY execution root is not a real directory: {0}")]
    InvalidExecutionRoot(PathBuf),
    #[error("NJOY execution-root receipt does not match the verification document")]
    ExecutionReceiptMismatch,
    #[error("NJOY execution artifact does not match its receipt: {0}")]
    ExecutionArtifactMismatch(String),
    #[error("processor artifact is not a real regular file: {0}")]
    InvalidProcessorArtifact(PathBuf),
    #[error("the same processor support artifact was declared more than once")]
    DuplicateProcessorSupportArtifact,
    #[error("the NJOY executable or a declared runtime artifact changed during execution")]
    ProcessorArtifactChanged,
    #[error("NJOY output root overlaps an input or evaluation root: {0}")]
    UnsafeOutputRoot(PathBuf),
    #[error("NJOY input deck is missing for {0}")]
    MissingInputDeck(String),
    #[error("selected evaluated source does not match the manifest for {0}")]
    SelectedSourceMismatch(String),
    #[error("NJOY timed out for {nuclide} after {timeout_seconds} seconds")]
    ProcessorTimeout {
        nuclide: String,
        timeout_seconds: u64,
    },
    #[error("NJOY terminated by signal while processing {0}")]
    ProcessorTerminatedBySignal(String),
    #[error("NJOY failed for {nuclide} with exit code {exit_code}")]
    ProcessorFailure { nuclide: String, exit_code: i32 },
    #[error("NJOY output is not UTF-8 text: {0}")]
    NonUtf8ProcessorText(PathBuf),
    #[error("NJOY version banner was not recognized for {0}")]
    UnrecognizedProcessorBanner(String),
    #[error("NJOY banners differ across runs: expected {expected:?}, observed {observed:?}")]
    InconsistentProcessorBanner { expected: String, observed: String },
    #[error("NJOY wrote nonempty standard error for {0}")]
    NonemptyStandardError(String),
    #[error("NJOY emitted its fatal error marker for {0}")]
    ProcessorErrorMarker(String),
    #[error("NJOY diagnostic report marker is missing for {0}")]
    MissingDiagnosticReport(String),
    #[error("NJOY kinematic markers could not be mapped to an energy and response MT")]
    UnparsedKinematicDiagnostic,
    #[error(
        "production PENDF special MF3 sections differ for {nuclide}: expected {expected:?}, observed {observed:?}"
    )]
    SpecialMf3Mismatch {
        nuclide: String,
        expected: Vec<u16>,
        observed: Vec<u16>,
    },
    #[error(
        "diagnostic PENDF special MF3 sections differ for {nuclide}: expected {expected:?}, observed {observed:?}"
    )]
    DiagnosticSpecialMf3Mismatch {
        nuclide: String,
        expected: Vec<u16>,
        observed: Vec<u16>,
    },
    #[error("production and diagnostic PENDF bytes differ for {0}")]
    ProductionDiagnosticPendfMismatch(String),
    #[error("unexpected or unsafe NJOY run artifact: {0}")]
    UnexpectedRunArtifact(PathBuf),
    #[error("NJOY run directory does not contain exactly the expected files: {0}")]
    RunFileSetMismatch(PathBuf),
    #[error("NJOY execution root does not contain exactly the expected runs and receipt")]
    ExecutionRootFileSetMismatch,
    #[error("required NJOY run artifact is empty: {0}")]
    EmptyRunArtifact(PathBuf),
    #[error("system clock is before the Unix epoch")]
    SystemClockBeforeEpoch,
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
    use crate::{GeneratedNjoyFile, NjoyInputManifest, NjoySourceEvaluation};

    const MANIFEST_JSON: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/njoy/openbnct-njoy-input-manifest.json"
    );
    const B10_DECK: &[u8] =
        include_bytes!("../../../benchmarks/synthetic/nf-bnct-001/transport/njoy/B10/input.njoy");
    const FROZEN_EXECUTION_RECEIPT: &[u8] = include_bytes!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-execution-receipt.json"
    );

    #[test]
    fn finds_only_special_mf3_kerma_sections() {
        let mut bytes = Vec::new();
        for (mf, mt) in [(3, 107), (3, 301), (3, 407), (3, 443), (1, 451)] {
            bytes.extend(format!("{:<66}{:>4}{mf:>2}{mt:>3}{:>5}\n", "", 525, 1).bytes());
        }
        assert_eq!(special_mf3_mts(&bytes), BTreeSet::from([301, 407, 443]));
    }

    #[test]
    fn maps_only_final_table_kinematic_direction_markers() {
        let report = format!(
            "high limit outside table\nfinal kerma factors\n{:>14}{:>14}{:>14}\n{:>14.4E}{:>14.4E}{:>14.4E}\n{:21}high\n",
            "e", "301", "443", 1.0e-5, 3.0e3, 2.5e-1, ""
        );
        assert_eq!(
            kinematic_violations(&report, &[301, 443]).unwrap(),
            [NjoyKinematicViolation {
                energy_ev: 1.0e-5,
                response_mt: 301,
                direction: NjoyKinematicDirection::High,
            }]
        );
        assert!(
            kinematic_violations("high low before table", &[301, 443])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rejects_an_unmapped_kinematic_marker() {
        let report = "final kerma factors\nhigh\n";
        assert!(matches!(
            kinematic_violations(report, &[301, 443]),
            Err(NjoyExecutionError::UnparsedKinematicDiagnostic)
        ));
    }

    #[test]
    fn validates_the_frozen_rejected_execution_receipt() {
        let document = NjoyExecutionReceiptDocument::from_bytes(FROZEN_EXECUTION_RECEIPT).unwrap();
        assert_eq!(
            document.sha256,
            "65a21b57507e76a68b77349e92390ae03ebb8c38f6ed6cee66197aa5ee4adea7"
        );
        assert_eq!(document.receipt.runs.len(), 10);
        assert_eq!(document.receipt.rejected_run_count, 4);
        assert_eq!(
            document.receipt.qualification,
            NjoyExecutionQualification::ExecutionObservedDiagnosticsFailed
        );
        assert_eq!(
            document
                .receipt
                .runs
                .iter()
                .map(|run| run.diagnostic_violation_count)
                .sum::<u64>(),
            72
        );
        assert_eq!(
            document
                .receipt
                .runs
                .iter()
                .filter(|run| {
                    run.diagnostic_status == NjoyRunDiagnosticStatus::KinematicLimitsExceeded
                })
                .map(|run| (run.nuclide.as_str(), run.diagnostic_violation_count))
                .collect::<Vec<_>>(),
            [("N15", 10), ("O16", 15), ("O17", 20), ("O18", 27)]
        );
    }

    #[cfg(unix)]
    #[test]
    fn controlled_mock_execution_writes_a_verified_receipt() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let mut manifest: NjoyInputManifest = serde_json::from_slice(MANIFEST_JSON).unwrap();
        manifest.runs.truncate(1);
        let source_bytes = b"mock evaluated source\n";
        manifest.runs[0].source_evaluation = NjoySourceEvaluation {
            filename: "mock-b10.endf".into(),
            size_bytes: source_bytes.len() as u64,
            sha256: sha256_bytes(source_bytes),
        };
        let deck = GeneratedNjoyFile {
            relative_path: "B10/input.njoy".into(),
            media_type: "text/plain".into(),
            sha256: sha256_bytes(B10_DECK),
            bytes: B10_DECK.to_vec(),
        };
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
        manifest_bytes.push(b'\n');
        let manifest_file = GeneratedNjoyFile {
            relative_path: "openbnct-njoy-input-manifest.json".into(),
            media_type: "application/json".into(),
            sha256: sha256_bytes(&manifest_bytes),
            bytes: manifest_bytes,
        };
        let bundle = NjoyInputBundle {
            manifest,
            files: vec![deck, manifest_file],
        };

        let input_root = temporary.path().join("inputs");
        bundle.write_new(&input_root).unwrap();
        let evaluations_root = temporary.path().join("evaluations");
        fs::create_dir(&evaluations_root).unwrap();
        fs::write(evaluations_root.join("mock-b10.endf"), source_bytes).unwrap();

        let executable = temporary.path().join("njoy");
        let script = b"#!/bin/sh\n\
printf ' njoy 2016.78 mock\\n'\n\
printf 'mock\\n' > tape21\n\
printf 'mock\\n' > tape22\n\
{ printf '%66s%4d%2d%3d%5d\\n' '' 525 3 301 1; printf '%66s%4d%2d%3d%5d\\n' '' 525 3 407 1; printf '%66s%4d%2d%3d%5d\\n' '' 525 3 443 1; } > tape23\n\
/bin/cp tape23 tape24\n\
printf 'plot\\n' > tape25\n\
printf 'final kerma factors\\n' > output\n";
        fs::write(&executable, script).unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let output_root = temporary.path().join("execution");
        let result = NjoyExecutionReceipt::execute(
            &bundle,
            NjoyExecutionOptions {
                executable: &executable,
                processor_support_artifacts: &[],
                input_bundle_root: &input_root,
                evaluations_root: &evaluations_root,
                output_root: &output_root,
                timeout_seconds: 5,
            },
        )
        .unwrap();

        assert_eq!(result.receipt.runs.len(), 1);
        assert_eq!(
            result.receipt.runs[0].observed_special_mf3_mts,
            [301, 407, 443]
        );
        assert_eq!(
            result.receipt.qualification,
            NjoyExecutionQualification::ExecutionObservedUnreviewed
        );
        assert_eq!(result.receipt.rejected_run_count, 0);
        assert!(result.receipt_path.is_file());

        let document = NjoyExecutionReceiptDocument::from_path(&result.receipt_path).unwrap();
        document.verify_execution_root(&output_root).unwrap();
        let suitability = crate::NjoySuitabilityReport::assess(&document, &output_root).unwrap();
        assert_eq!(
            suitability.qualification,
            crate::NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed
        );
        fs::write(output_root.join("B10/tape25"), b"tampered\n").unwrap();
        assert!(matches!(
            document.verify_execution_root(&output_root),
            Err(NjoyExecutionError::ExecutionArtifactMismatch(path)) if path == "B10/tape25"
        ));
    }
}
