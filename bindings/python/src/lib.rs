// SPDX-License-Identifier: MIT

//! Narrow PyO3 boundary over the authoritative NCTForge Rust crates.
//!
//! This module implements no dose, geometry, evidence, or qualification logic
//! of its own (ADR 0015). Every function below delegates to the same Rust
//! contracts used by the CLI and GUI so Python users observe identical
//! acceptance, rejection, serialization, and content-identity behavior.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};

use openbnct_bio::{
    AppliedFractionation, BiologicalDoseBundle, BiologicalModel, IsoeffectiveModel, LinealSpectrum,
    LinealTallySpec, MicrodosimetricModel, RegionMask, SpectrumInput, apply_biological_model,
    apply_isoeffective_model, apply_microdosimetric_model,
};
use openbnct_boron::{BoronMicrodistribution, MicrodistributionCorrection};
use openbnct_core::{ContentReference, PhysicalDoseBundle, ResampleMethod};
use openbnct_dicom::{
    BenchmarkReport, RtPlanSummary, VerifiedBenchmarkCase, load_nf_bnct_001,
    synthetic::generate_nf_bnct_001, verify_nf_bnct_001,
};
use openbnct_evidence::{
    AnalyticOracle, AnalyticOracleEvaluation, CaseManifest, EvidenceBundleManifest,
    GammaEvaluation, MetamorphicEvaluation, sha256_file,
};
use openbnct_nifti::ComponentNiftiManifest;
use openbnct_openmc::OpenMcBackend;
use openbnct_transport::{
    AcceleratorSource, BackendDescriptor, BeamDescription, BeamQualityReport, BeamShapingAssembly,
    BsaSweepRecord, CompletedRun, ComponentDefinitionProfile, DoseUncertaintyBudget,
    FixedSourceDefinition, MaterialAssignment, MaterialDefinition, MeasurementComparisonReport,
    MeasurementRecord, MultigroupCovariance, MultigroupData, MultigroupFlux, NeutronResponseSet,
    ResolvedWeightWindows, ResponseGenerationMethod, SensitivityScreening, SensitivitySpec,
    TransportBackend, TransportCase,
};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use serde::de::DeserializeOwned;

create_exception!(
    openbnct,
    NctForgeError,
    PyException,
    "An NCTForge contract, verification, or evidence check failed."
);

fn reject(error: impl Display) -> PyErr {
    NctForgeError::new_err(error.to_string())
}

/// Write `contract` as pretty JSON, refusing to overwrite an existing file.
fn write_json_new(output: &Path, contract: &impl serde::Serialize) -> PyResult<()> {
    let bytes = serde_json::to_vec_pretty(contract).map_err(reject)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(reject)?;
    use std::io::Write as _;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(reject)
}

/// SHA-256 of a contract's canonical serialization — binds the exact
/// artifact content consumed, matching `write`/`to_json` output.
fn content_reference(id: &str, contract: &impl serde::Serialize) -> PyResult<ContentReference> {
    let bytes = serde_json::to_vec(contract).map_err(reject)?;
    use sha2::Digest;
    Ok(ContentReference {
        id: id.into(),
        sha256: format!("{:x}", sha2::Sha256::digest(&bytes)),
    })
}

/// Load `(name, path)` region-mask pairs with name agreement enforced.
fn load_named_masks(pairs: Vec<(String, PathBuf)>) -> PyResult<Vec<RegionMask>> {
    pairs
        .into_iter()
        .map(|(name, path)| {
            let mask: RegionMask =
                serde_json::from_slice(&fs::read(&path).map_err(reject)?).map_err(reject)?;
            if mask.name != name {
                return Err(reject(format!(
                    "region mask {} is named {:?}, expected {name:?}",
                    path.display(),
                    mask.name
                )));
            }
            Ok(mask)
        })
        .collect()
}

fn load_contract<T>(path: PathBuf) -> PyResult<T>
where
    T: DeserializeOwned + ContractCheck,
{
    let bytes = fs::read(&path).map_err(reject)?;
    let contract: T = serde_json::from_slice(&bytes).map_err(reject)?;
    contract.check().map_err(reject)?;
    Ok(contract)
}

/// Uniform "deserialize then run the contract's own validation" rule so no
/// validation rule is reimplemented at the language boundary.
trait ContractCheck {
    fn check(&self) -> Result<(), String>;
}

macro_rules! contract_check {
    ($type:ty, $method:ident, $error:ty) => {
        impl ContractCheck for $type {
            fn check(&self) -> Result<(), String> {
                self.$method().map_err(|error: $error| error.to_string())
            }
        }
    };
}

contract_check!(
    MaterialDefinition,
    validate,
    openbnct_transport::TransportModelError
);
contract_check!(
    FixedSourceDefinition,
    validate,
    openbnct_transport::TransportModelError
);
contract_check!(
    ComponentDefinitionProfile,
    validate,
    openbnct_transport::ResponseMethodError
);
contract_check!(
    ResponseGenerationMethod,
    validate,
    openbnct_transport::ResponseMethodError
);
contract_check!(
    NeutronResponseSet,
    validate,
    openbnct_transport::ResponseSetError
);
contract_check!(CaseManifest, validate, openbnct_evidence::ManifestError);
contract_check!(
    openbnct_core::ExposurePlan,
    validate,
    openbnct_core::ExposurePlanError
);
contract_check!(
    MultigroupData,
    validate,
    openbnct_transport::MultigroupError
);
contract_check!(MultigroupCovariance, validate, openbnct_transport::UqError);
contract_check!(DoseUncertaintyBudget, validate, openbnct_transport::UqError);
contract_check!(
    SensitivitySpec,
    validate,
    openbnct_transport::ScreeningError
);
contract_check!(
    SensitivityScreening,
    validate,
    openbnct_transport::ScreeningError
);
contract_check!(
    ResolvedWeightWindows,
    validate,
    openbnct_transport::VarianceReductionError
);
contract_check!(
    MeasurementRecord,
    validate,
    openbnct_transport::MeasurementError
);
contract_check!(
    MeasurementComparisonReport,
    validate,
    openbnct_transport::MeasurementError
);
contract_check!(BeamDescription, validate, openbnct_transport::BeamError);
contract_check!(
    BeamQualityReport,
    validate,
    openbnct_transport::BeamQualityError
);
contract_check!(
    AcceleratorSource,
    validate,
    openbnct_transport::AcceleratorError
);
contract_check!(BeamShapingAssembly, validate, openbnct_transport::BsaError);
contract_check!(BsaSweepRecord, validate, openbnct_transport::BsaError);
contract_check!(LinealSpectrum, validate, openbnct_bio::BioError);
contract_check!(LinealTallySpec, validate, openbnct_bio::LinealTallyError);
contract_check!(
    MetamorphicEvaluation,
    validate,
    openbnct_evidence::ManifestError
);
contract_check!(AnalyticOracle, validate, openbnct_evidence::ManifestError);
contract_check!(GammaEvaluation, validate, openbnct_evidence::ManifestError);
contract_check!(
    BoronMicrodistribution,
    validate,
    openbnct_boron::MicrodistributionError
);
contract_check!(
    MicrodistributionCorrection,
    validate,
    openbnct_boron::MicrodistributionError
);

/// Schema-token check for report types produced by the CLI — they are
/// validated by their producer, so the boundary only pins the schema.
impl ContractCheck for MultigroupFlux {
    fn check(&self) -> Result<(), String> {
        openbnct_core::schema_matches(
            &self.schema_version,
            openbnct_transport::MULTIGROUP_FLUX_SCHEMA,
        )
        .then_some(())
        .ok_or_else(|| format!("unsupported schema_version {:?}", self.schema_version))
    }
}

impl ContractCheck for AnalyticOracleEvaluation {
    fn check(&self) -> Result<(), String> {
        openbnct_core::schema_matches(
            &self.schema_version,
            openbnct_evidence::ANALYTIC_EVALUATION_SCHEMA,
        )
        .then_some(())
        .ok_or_else(|| format!("unsupported schema_version {:?}", self.schema_version))
    }
}

impl ContractCheck for RtPlanSummary {
    fn check(&self) -> Result<(), String> {
        openbnct_core::schema_matches(&self.schema_version, openbnct_dicom::RTPLAN_SUMMARY_SCHEMA)
            .then_some(())
            .ok_or_else(|| format!("unsupported schema_version {:?}", self.schema_version))
    }
}

impl ContractCheck for ComponentNiftiManifest {
    fn check(&self) -> Result<(), String> {
        openbnct_core::schema_matches(
            &self.schema_version,
            openbnct_nifti::COMPONENT_NIFTI_MANIFEST_SCHEMA,
        )
        .then_some(())
        .ok_or_else(|| format!("unsupported schema_version {:?}", self.schema_version))
    }
}

/// Schema-token check for artifact types whose validation ran at import.
impl ContractCheck for openbnct_core::ExternalDoseBundle {
    fn check(&self) -> Result<(), String> {
        (self.schema_version == openbnct_core::EXTERNAL_DOSE_SCHEMA)
            .then_some(())
            .ok_or_else(|| format!("unsupported schema_version {:?}", self.schema_version))
    }
}

impl ContractCheck for openbnct_bio::BedBundle {
    fn check(&self) -> Result<(), String> {
        (self.schema_version == openbnct_bio::BED_BUNDLE_SCHEMA)
            .then_some(())
            .ok_or_else(|| format!("unsupported schema_version {:?}", self.schema_version))
    }
}

/// Transport-backend descriptor with its current capability flags.
///
/// Flags are reported exactly as the Rust backend advertises them; an action
/// that is not implemented remains `False` rather than silently succeeding.
#[pyclass(frozen, name = "Backend")]
struct PyBackend {
    inner: BackendDescriptor,
}

#[pymethods]
impl PyBackend {
    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    #[getter]
    fn display_name(&self) -> &str {
        &self.inner.display_name
    }

    #[getter]
    fn version(&self) -> Option<String> {
        self.inner.version.clone()
    }

    #[getter]
    fn can_prepare(&self) -> bool {
        self.inner.can_prepare
    }

    #[getter]
    fn can_execute(&self) -> bool {
        self.inner.can_execute
    }

    #[getter]
    fn can_import(&self) -> bool {
        self.inner.can_import
    }

    fn __repr__(&self) -> String {
        format!(
            "Backend(id={:?}, can_prepare={}, can_execute={}, can_import={})",
            self.inner.id, self.inner.can_prepare, self.inner.can_execute, self.inner.can_import
        )
    }
}

/// Descriptors for every compiled-in transport backend.
#[pyfunction]
fn backends() -> Vec<PyBackend> {
    vec![PyBackend {
        inner: OpenMcBackend::default().descriptor(),
    }]
}

/// SHA-256 of a file's exact bytes, lowercase hex.
#[pyfunction]
fn file_sha256(path: PathBuf) -> PyResult<String> {
    sha256_file(&path).map_err(reject)
}

/// Verified ROI summary for a structure in the frozen benchmark case.
#[pyclass(frozen, name = "Structure")]
struct PyStructure {
    number: i32,
    name: String,
    voxel_count: usize,
    volume_cm3: f64,
    centroid_lps_mm: [f64; 3],
}

#[pymethods]
impl PyStructure {
    #[getter]
    fn number(&self) -> i32 {
        self.number
    }

    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    #[getter]
    fn voxel_count(&self) -> usize {
        self.voxel_count
    }

    #[getter]
    fn volume_cm3(&self) -> f64 {
        self.volume_cm3
    }

    #[getter]
    fn centroid_lps_mm(&self) -> (f64, f64, f64) {
        self.centroid_lps_mm.into()
    }

    fn __repr__(&self) -> String {
        format!(
            "Structure(name={:?}, voxel_count={}, volume_cm3={})",
            self.name, self.voxel_count, self.volume_cm3
        )
    }
}

/// Result of the independent `NF-BNCT-001` verification oracle.
#[pyclass(frozen, name = "CaseVerification")]
struct PyCaseVerification {
    inner: BenchmarkReport,
}

#[pymethods]
impl PyCaseVerification {
    #[getter]
    fn case_id(&self) -> &str {
        self.inner.case_id
    }

    /// [columns, rows, slices]
    #[getter]
    fn shape(&self) -> (u32, u32, u32) {
        self.inner.shape.into()
    }

    /// Voxel spacing in millimetres as [column, row, slice].
    #[getter]
    fn spacing_mm(&self) -> (f64, f64, f64) {
        self.inner.spacing_mm.into()
    }

    /// LPS position of the centre of voxel [0, 0, 0], millimetres.
    #[getter]
    fn origin_mm(&self) -> (f64, f64, f64) {
        self.inner.origin_mm.into()
    }

    #[getter]
    fn ct_slice_count(&self) -> usize {
        self.inner.ct_slice_count
    }

    #[getter]
    fn verified_artifact_count(&self) -> usize {
        self.inner.verified_artifact_count
    }

    #[getter]
    fn structures(&self) -> Vec<PyStructure> {
        self.inner
            .rois
            .iter()
            .map(|roi| PyStructure {
                number: roi.number,
                name: roi.name.clone(),
                voxel_count: roi.voxel_count,
                volume_cm3: roi.volume_cm3,
                centroid_lps_mm: roi.centroid_lps_mm,
            })
            .collect()
    }

    fn __repr__(&self) -> String {
        format!(
            "CaseVerification(case_id={:?}, shape={:?}, verified_artifacts={})",
            self.inner.case_id, self.inner.shape, self.inner.verified_artifact_count
        )
    }
}

/// A fully verified `NF-BNCT-001` case with geometry and structure access.
///
/// Construction runs the complete verification oracle; a case that fails any
/// gate cannot be loaded through this object.
#[pyclass(frozen, name = "VerifiedCase")]
struct PyVerifiedCase {
    inner: VerifiedBenchmarkCase,
}

#[pymethods]
impl PyVerifiedCase {
    #[getter]
    fn report(&self) -> PyCaseVerification {
        PyCaseVerification {
            inner: self.inner.report.clone(),
        }
    }

    /// CT lattice: shape, spacing, origin, and direction in LPS millimetres.
    #[getter]
    fn geometry(&self) -> PyGeometry {
        PyGeometry {
            inner: self.inner.ct.geometry.clone(),
        }
    }

    /// Frame of Reference UID shared by the CT series.
    #[getter]
    fn frame_of_reference_uid(&self) -> &str {
        &self.inner.ct.frame_of_reference_uid
    }

    /// Structure summaries computed from the rasterized ROI masks.
    #[getter]
    fn structures(&self) -> Vec<PyStructure> {
        self.inner
            .structures
            .rois
            .iter()
            .map(|roi| PyStructure {
                number: roi.number,
                name: roi.name.clone(),
                voxel_count: roi.voxel_count(),
                volume_cm3: roi.volume_cm3(&self.inner.ct),
                centroid_lps_mm: roi.centroid_lps_mm(&self.inner.ct).unwrap_or([f64::NAN; 3]),
            })
            .collect()
    }

    /// Modality value at voxel [column, row, slice], after rescale.
    fn ct_value(&self, column: u32, row: u32, slice: u32) -> PyResult<f64> {
        let shape = self.inner.ct.geometry.shape;
        if column >= shape[0] || row >= shape[1] || slice >= shape[2] {
            return Err(reject(format!(
                "voxel [{column}, {row}, {slice}] outside shape {shape:?}"
            )));
        }
        let index = (slice as usize) * (shape[0] as usize) * (shape[1] as usize)
            + (row as usize) * (shape[0] as usize)
            + column as usize;
        Ok(self
            .inner
            .ct
            .modality_value(self.inner.ct.stored_pixels[index]))
    }

    /// Boolean mask for the named structure, columns fastest then rows, slices.
    fn structure_mask(&self, name: &str) -> PyResult<Vec<bool>> {
        self.inner
            .structures
            .roi(name)
            .map(|roi| roi.voxels.clone())
            .ok_or_else(|| reject(format!("unknown structure {name:?}")))
    }
}

/// Validated CT lattice geometry in the DICOM LPS patient frame.
#[pyclass(frozen, name = "Geometry")]
struct PyGeometry {
    inner: openbnct_core::GridGeometry,
}

#[pymethods]
impl PyGeometry {
    /// [columns, rows, slices]
    #[getter]
    fn shape(&self) -> (u32, u32, u32) {
        self.inner.shape.into()
    }

    #[getter]
    fn spacing_mm(&self) -> (f64, f64, f64) {
        self.inner.spacing_mm.into()
    }

    #[getter]
    fn origin_mm(&self) -> (f64, f64, f64) {
        self.inner.origin_mm.into()
    }

    #[getter]
    fn direction(&self) -> (f64, f64, f64, f64, f64, f64, f64, f64, f64) {
        self.inner.direction.into()
    }

    #[getter]
    fn voxel_count(&self) -> PyResult<usize> {
        self.inner.voxel_count().map_err(reject)
    }

    /// LPS position of a voxel centre in millimetres.
    fn voxel_center_lps_mm(&self, column: u32, row: u32, slice: u32) -> PyResult<(f64, f64, f64)> {
        self.inner
            .voxel_center_lps_mm([column, row, slice])
            .map(Into::into)
            .map_err(reject)
    }
}

/// Summary of a generated `NF-BNCT-001` case directory.
#[pyclass(frozen, name = "GeneratedCase")]
struct PyGeneratedCase {
    root: PathBuf,
    ct_file_count: usize,
    rtstruct_file: PathBuf,
    manifest_file: PathBuf,
}

#[pymethods]
impl PyGeneratedCase {
    #[getter]
    fn root(&self) -> PathBuf {
        self.root.clone()
    }

    #[getter]
    fn ct_file_count(&self) -> usize {
        self.ct_file_count
    }

    #[getter]
    fn rtstruct_file(&self) -> PathBuf {
        self.rtstruct_file.clone()
    }

    #[getter]
    fn manifest_file(&self) -> PathBuf {
        self.manifest_file.clone()
    }
}

/// Generate the deterministic synthetic `NF-BNCT-001` DICOM case.
///
/// Refuses to overwrite an existing destination, matching the CLI.
#[pyfunction]
fn generate_case(destination: PathBuf) -> PyResult<PyGeneratedCase> {
    let generated = generate_nf_bnct_001(&destination).map_err(reject)?;
    Ok(PyGeneratedCase {
        root: generated.root,
        ct_file_count: generated.ct_files.len(),
        rtstruct_file: generated.rtstruct_file,
        manifest_file: generated.manifest_file,
    })
}

/// Verify a generated `NF-BNCT-001` case against the independent frozen oracle.
#[pyfunction]
fn verify_case(root: PathBuf) -> PyResult<PyCaseVerification> {
    let report = verify_nf_bnct_001(&root).map_err(reject)?;
    Ok(PyCaseVerification { inner: report })
}

/// Load `NF-BNCT-001` only after all geometry and artifact gates pass.
#[pyfunction]
fn load_case(root: PathBuf) -> PyResult<PyVerifiedCase> {
    let case = load_nf_bnct_001(&root).map_err(reject)?;
    Ok(PyVerifiedCase { inner: case })
}

/// One artifact binding inside a verified case manifest.
#[pyclass(frozen, name = "Artifact")]
struct PyArtifact {
    role: String,
    path: String,
    sha256: String,
    media_type: Option<String>,
}

#[pymethods]
impl PyArtifact {
    #[getter]
    fn role(&self) -> &str {
        &self.role
    }

    #[getter]
    fn path(&self) -> &str {
        &self.path
    }

    #[getter]
    fn sha256(&self) -> &str {
        &self.sha256
    }

    #[getter]
    fn media_type(&self) -> Option<String> {
        self.media_type.clone()
    }
}

/// A parsed and schema-validated `case.json` manifest.
#[pyclass(frozen, name = "CaseManifest")]
struct PyCaseManifest {
    inner: CaseManifest,
}

#[pymethods]
impl PyCaseManifest {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    #[getter]
    fn qualification(&self) -> PyResult<String> {
        serde_json::to_value(&self.inner.qualification)
            .map_err(reject)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| reject("qualification is not a string"))
    }

    #[getter]
    fn coordinate_system(&self) -> PyResult<String> {
        serde_json::to_value(&self.inner.coordinate_system)
            .map_err(reject)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| reject("coordinate_system is not a string"))
    }

    #[getter]
    fn frame_of_reference_uid(&self) -> &str {
        &self.inner.frame_of_reference_uid
    }

    #[getter]
    fn material_model_id(&self) -> &str {
        &self.inner.material_model_id
    }

    #[getter]
    fn source_model_id(&self) -> &str {
        &self.inner.source_model_id
    }

    #[getter]
    fn geometry(&self) -> PyGeometry {
        PyGeometry {
            inner: self.inner.geometry.clone(),
        }
    }

    #[getter]
    fn structures(&self) -> Vec<PyStructure> {
        self.inner
            .structures
            .iter()
            .map(|record| PyStructure {
                number: record.number,
                name: record.name.clone(),
                voxel_count: record.voxel_count,
                volume_cm3: record.volume_cm3,
                centroid_lps_mm: record.centroid_lps_mm,
            })
            .collect()
    }

    #[getter]
    fn artifacts(&self) -> Vec<PyArtifact> {
        self.inner
            .artifacts
            .iter()
            .map(|record| PyArtifact {
                role: record.role.clone(),
                path: record.path.clone(),
                sha256: record.sha256.clone(),
                media_type: record.media_type.clone(),
            })
            .collect()
    }

    /// Canonical JSON bytes as produced by the Rust contract, as text.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    /// Re-verify every bound artifact under `case_root`; returns the count.
    fn verify_artifacts(&self, case_root: PathBuf) -> PyResult<usize> {
        self.inner.verify_artifacts(&case_root).map_err(reject)?;
        Ok(self.inner.artifacts.len())
    }
}

/// Read and validate a `case.json` manifest.
#[pyfunction]
fn read_manifest(path: PathBuf) -> PyResult<PyCaseManifest> {
    Ok(PyCaseManifest {
        inner: load_contract(path)?,
    })
}

// PyO3 requires the struct name to match the `name = "..."` attribute target.
macro_rules! contract_wrapper {
    ($py_name:literal, $rust_struct:ident, $inner:ty, $doc:literal) => {
        #[doc = $doc]
        #[pyclass(frozen, name = $py_name)]
        struct $rust_struct {
            inner: $inner,
        }

        #[pymethods]
        impl $rust_struct {
            #[getter]
            fn schema_version(&self) -> &str {
                &self.inner.schema_version
            }

            #[getter]
            fn id(&self) -> &str {
                &self.inner.id
            }

            /// Canonical JSON bytes as produced by the Rust contract, as text.
            fn to_json(&self) -> PyResult<String> {
                serde_json::to_string_pretty(&self.inner).map_err(reject)
            }
        }
    };
}

contract_wrapper!(
    "Material",
    PyMaterial,
    MaterialDefinition,
    "A validated explicit-nuclide material contract."
);
contract_wrapper!(
    "FixedSource",
    PyFixedSource,
    FixedSourceDefinition,
    "A validated backend-neutral fixed-source contract."
);
contract_wrapper!(
    "ComponentProfile",
    PyComponentProfile,
    ComponentDefinitionProfile,
    "A validated four-component dose-definition profile."
);
contract_wrapper!(
    "ResponseGenerationMethod",
    PyResponseGenerationMethod,
    ResponseGenerationMethod,
    "A validated, versioned response-generation recipe."
);

/// A validated material-specific neutron response set.
///
/// `validate()` is enforced on load. `folding_ready` additionally requires the
/// independent-review gate used before dose folding; a set that parses but has
/// not passed review loads successfully but reports `folding_ready == False`.
#[pyclass(frozen, name = "ResponseSet")]
struct PyResponseSet {
    inner: NeutronResponseSet,
}

#[pymethods]
impl PyResponseSet {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    #[getter]
    fn qualification(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.qualification)
            .map_err(reject)?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| reject("qualification is not a string"))
    }

    /// True only when the set passes the independent-review folding gate.
    #[getter]
    fn folding_ready(&self) -> bool {
        self.inner.validate_for_folding().is_ok()
    }

    /// [lower, upper] transported-energy interval the grid must cover, eV.
    #[getter]
    fn transport_energy_range_ev(&self) -> (f64, f64) {
        self.inner.transport_energy_range_ev.into()
    }

    #[getter]
    fn energy_knot_count(&self) -> usize {
        self.inner.energy_ev.len()
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Read and validate an explicit-nuclide material contract.
#[pyfunction]
fn load_material(path: PathBuf) -> PyResult<PyMaterial> {
    Ok(PyMaterial {
        inner: load_contract(path)?,
    })
}

/// Read and validate a fixed-source contract.
#[pyfunction]
fn load_fixed_source(path: PathBuf) -> PyResult<PyFixedSource> {
    Ok(PyFixedSource {
        inner: load_contract(path)?,
    })
}

/// A deterministic report of how a source was positioned on a case.
#[pyclass(frozen, name = "PositionReport")]
struct PyPositionReport {
    inner: openbnct_transport::PositionReport,
}

#[pymethods]
impl PyPositionReport {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    #[getter]
    fn target_region(&self) -> &str {
        &self.inner.target_region
    }

    /// Beam propagation direction, a unit vector in LPS.
    #[getter]
    fn beam_direction_lps(&self) -> (f64, f64, f64) {
        self.inner.beam_direction_lps.into()
    }

    #[getter]
    fn target_centroid_lps_mm(&self) -> (f64, f64, f64) {
        self.inner.target_centroid_lps_mm.into()
    }

    /// Entry face axis (`x`/`y`/`z`) and side (`low`/`high`).
    #[getter]
    fn entry(&self) -> PyResult<(String, String)> {
        let axis = serde_json::to_value(self.inner.entry_axis)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)?;
        let side = serde_json::to_value(self.inner.entry_side)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)?;
        Ok((axis, side))
    }

    #[getter]
    fn entry_point_lps_mm(&self) -> (f64, f64, f64) {
        self.inner.entry_point_lps_mm.into()
    }

    #[getter]
    fn source_to_centroid_mm(&self) -> f64 {
        self.inner.source_to_centroid_mm
    }

    #[getter]
    fn aperture_half_widths_cm(&self) -> (f64, f64) {
        self.inner.aperture_half_widths_cm.into()
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Aim a source so the beam axis passes through a mask's centroid (same path
/// as `openbnct position aim`). `approach` is `+x|-x|+y|-y|+z|-z` or
/// `direction_lps` an arbitrary `(dx, dy, dz)`. `case_id` is stamped into the
/// report. Returns `(positioned_source, report)`.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (source, geometry, mask, case_id, approach=None, direction_lps=None, half_widths_cm=(1.0, 1.0), margin_cm=0.01))]
fn aim_source(
    source: &PyFixedSource,
    geometry: &PyGeometry,
    mask: PathBuf,
    case_id: &str,
    approach: Option<&str>,
    direction_lps: Option<(f64, f64, f64)>,
    half_widths_cm: (f64, f64),
    margin_cm: f64,
) -> PyResult<(PyFixedSource, PyPositionReport)> {
    let mask: RegionMask =
        serde_json::from_slice(&fs::read(&mask).map_err(reject)?).map_err(reject)?;
    let direction = if let Some(direction) = direction_lps {
        [direction.0, direction.1, direction.2]
    } else if let Some(approach) = approach {
        openbnct_transport::AxisApproach::parse(approach)
            .map_err(|e| PyValueError::new_err(e.to_string()))?
            .unit_vector()
    } else {
        return Err(PyValueError::new_err(
            "supply approach (+x|-x|+y|-y|+z|-z) or direction_lps",
        ));
    };
    let (positioned, mut report) = openbnct_transport::aim_source_at_centroid(
        &source.inner,
        &geometry.inner,
        &mask,
        direction,
        [half_widths_cm.0, half_widths_cm.1],
        margin_cm,
    )
    .map_err(reject)?;
    report.case_id = case_id.into();
    Ok((
        PyFixedSource { inner: positioned },
        PyPositionReport { inner: report },
    ))
}

/// Rotate a source about a world axis through `center_lps_mm` by a multiple
/// of 90 degrees (same path as `openbnct position rotate`).
#[pyfunction]
fn rotate_source(
    source: &PyFixedSource,
    axis: &str,
    center_lps_mm: (f64, f64, f64),
    degrees: f64,
) -> PyResult<PyFixedSource> {
    let axis = match axis {
        "x" => openbnct_transport::PlaneAxis::X,
        "y" => openbnct_transport::PlaneAxis::Y,
        "z" => openbnct_transport::PlaneAxis::Z,
        other => {
            return Err(PyValueError::new_err(format!(
                "axis must be x|y|z, got {other:?}"
            )));
        }
    };
    let rotated = openbnct_transport::rotate_source(
        &source.inner,
        [center_lps_mm.0, center_lps_mm.1, center_lps_mm.2],
        axis,
        degrees,
    )
    .map_err(reject)?;
    Ok(PyFixedSource { inner: rotated })
}

/// Read and validate a component-definition profile.
#[pyfunction]
fn load_component_profile(path: PathBuf) -> PyResult<PyComponentProfile> {
    Ok(PyComponentProfile {
        inner: load_contract(path)?,
    })
}

/// Read and validate a response-generation method contract.
#[pyfunction]
fn load_response_generation_method(path: PathBuf) -> PyResult<PyResponseGenerationMethod> {
    Ok(PyResponseGenerationMethod {
        inner: load_contract(path)?,
    })
}

/// Read and validate a neutron response set (base validation only).
#[pyfunction]
fn load_response_set(path: PathBuf) -> PyResult<PyResponseSet> {
    Ok(PyResponseSet {
        inner: load_contract(path)?,
    })
}

contract_check!(PhysicalDoseBundle, validate, openbnct_core::ValidationError);
contract_check!(BiologicalModel, validate, openbnct_bio::BioError);
contract_check!(BiologicalDoseBundle, validate, openbnct_bio::BioError);
contract_check!(
    openbnct_bio::BioModelComparison,
    validate,
    openbnct_bio::BioError
);

/// One component's dose values over the case grid.
#[pyclass(frozen, name = "DoseVolume")]
struct PyDoseVolume {
    component: String,
    unit: String,
    values: Vec<f64>,
    absolute_standard_uncertainty: Option<Vec<f64>>,
}

#[pymethods]
impl PyDoseVolume {
    #[getter]
    fn component(&self) -> &str {
        &self.component
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.unit
    }

    /// Per-voxel values in row-major `[column, row, slice]` grid order.
    #[getter]
    fn values(&self) -> Vec<f64> {
        self.values.clone()
    }

    /// Per-voxel one-sigma absolute uncertainty, when present.
    #[getter]
    fn absolute_standard_uncertainty(&self) -> Option<Vec<f64>> {
        self.absolute_standard_uncertainty.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "DoseVolume(component={:?}, unit={:?}, voxels={})",
            self.component,
            self.unit,
            self.values.len()
        )
    }
}

/// Serialize a snake_case-tagged enum to its token (`delivered_fraction`,
/// `independent_exposures`, ...). Falls back to an empty string on
/// non-string representations, which cannot happen for these enums.
fn snake_token(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn dose_unit_name(unit: openbnct_core::DoseUnit) -> String {
    match unit {
        openbnct_core::DoseUnit::Gray => "gray".into(),
        openbnct_core::DoseUnit::GrayPerSourceParticle => "gray_per_source_particle".into(),
    }
}

/// A validated physical dose bundle produced by statepoint collection or
/// imported interchange.
#[pyclass(frozen, name = "PhysicalDoseBundle")]
struct PyPhysicalDoseBundle {
    inner: PhysicalDoseBundle,
}

#[pymethods]
impl PyPhysicalDoseBundle {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    #[getter]
    fn geometry(&self) -> PyGeometry {
        PyGeometry {
            inner: self.inner.geometry.clone(),
        }
    }

    #[getter]
    fn components(&self) -> Vec<PyDoseVolume> {
        self.inner
            .components
            .iter()
            .map(|volume| PyDoseVolume {
                component: serde_json::to_value(volume.component)
                    .and_then(serde_json::from_value::<String>)
                    .unwrap_or_else(|_| "unknown".into()),
                unit: dose_unit_name(volume.unit),
                values: volume.values.clone(),
                absolute_standard_uncertainty: volume.absolute_standard_uncertainty.clone(),
            })
            .collect()
    }

    /// Dedicated physical-total dose volume, kept separate from component sums.
    #[getter]
    fn physical_total(&self) -> PyDoseVolume {
        PyDoseVolume {
            component: "physical_total".into(),
            unit: dose_unit_name(self.inner.physical_total.unit),
            values: self.inner.physical_total.values.clone(),
            absolute_standard_uncertainty: self
                .inner
                .physical_total
                .absolute_standard_uncertainty
                .clone(),
        }
    }

    #[getter]
    fn provenance_id(&self) -> &str {
        &self.inner.provenance_id
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Load and validate a `openbnct.physical-dose-bundle/0.2.0` artifact.
#[pyfunction]
fn load_physical_dose_bundle(path: PathBuf) -> PyResult<PyPhysicalDoseBundle> {
    Ok(PyPhysicalDoseBundle {
        inner: load_contract(path)?,
    })
}

/// Collect a completed OpenMC run directory into a validated physical dose
/// bundle, using the same `OpenMcBackend::collect` path as the CLI.
#[pyfunction]
fn collect_run(working_directory: PathBuf) -> PyResult<PyPhysicalDoseBundle> {
    let completed = CompletedRun {
        backend_id: "openmc".into(),
        case_id: String::new(),
        working_directory: working_directory.display().to_string(),
        exit_code: 0,
    };
    let bundle = OpenMcBackend::default()
        .collect(&completed)
        .map_err(reject)?;
    Ok(PyPhysicalDoseBundle { inner: bundle })
}

#[pyclass(frozen, name = "Exposure")]
struct PyExposure {
    inner: openbnct_core::Exposure,
}

#[pymethods]
impl PyExposure {
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    #[getter]
    fn dose_bundle_path(&self) -> &str {
        &self.inner.dose_bundle.path
    }

    #[getter]
    fn dose_bundle_id(&self) -> &str {
        &self.inner.dose_bundle.id
    }

    #[getter]
    fn dose_bundle_sha256(&self) -> &str {
        &self.inner.dose_bundle.sha256
    }

    #[getter]
    fn weight(&self) -> f64 {
        self.inner.weight
    }

    #[getter]
    fn weight_basis(&self) -> String {
        snake_token(&self.inner.weight_basis)
    }

    #[getter]
    fn duration_s(&self) -> Option<f64> {
        self.inner.duration_s
    }

    #[getter]
    fn boron_assumption(&self) -> Option<&str> {
        self.inner.boron_assumption.as_deref()
    }
}

/// A validated `openbnct.exposure-plan/0.1.0` weighted-exposure plan.
#[pyclass(frozen, name = "ExposurePlan")]
struct PyExposurePlan {
    inner: openbnct_core::ExposurePlan,
}

#[pymethods]
impl PyExposurePlan {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    #[getter]
    fn covariance(&self) -> String {
        snake_token(&self.inner.covariance)
    }

    #[getter]
    fn exposures(&self) -> Vec<PyExposure> {
        self.inner
            .exposures
            .iter()
            .map(|exposure| PyExposure {
                inner: exposure.clone(),
            })
            .collect()
    }

    /// Every detectable plan issue, not just the first.
    fn validate_diagnostics(&self) -> Vec<String> {
        self.inner
            .validate_diagnostics()
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Load a `openbnct.exposure-plan/0.1.0` document.
#[pyfunction]
fn load_exposure_plan(path: PathBuf) -> PyResult<PyExposurePlan> {
    Ok(PyExposurePlan {
        inner: load_contract(path)?,
    })
}

/// Inspect a possibly-malformed plan file and return every detectable
/// issue — the Python counterpart of `openbnct plan validate`. Unlike
/// `load_exposure_plan`, this reports on the raw document without rejecting
/// it, so diagnostics are reachable for broken plans.
#[pyfunction]
fn exposure_plan_diagnostics(path: PathBuf) -> PyResult<Vec<String>> {
    let bytes = fs::read(&path).map_err(reject)?;
    let plan: openbnct_core::ExposurePlan = serde_json::from_slice(&bytes).map_err(reject)?;
    Ok(plan
        .validate_diagnostics()
        .iter()
        .map(ToString::to_string)
        .collect())
}

/// Run a saved exposure plan end to end — verify each bound bundle's
/// recorded hash, then accumulate the weighted exposures — using the same
/// Rust path as `openbnct accumulate`.
#[pyfunction]
fn accumulate_exposures(plan_path: PathBuf) -> PyResult<PyPhysicalDoseBundle> {
    Ok(PyPhysicalDoseBundle {
        inner: openbnct_plan::accumulate_plan_file(&plan_path).map_err(reject)?,
    })
}

/// Import a `.csv`/`.xlsx` exposure table into an exposure-plan JSON file
/// at `output` (same path as `openbnct plan import`).
#[pyfunction]
#[pyo3(signature = (table, output, id=None, case_id=None, bundles_dir=None))]
fn plan_table_read(
    table: PathBuf,
    output: PathBuf,
    id: Option<String>,
    case_id: Option<String>,
    bundles_dir: Option<PathBuf>,
) -> PyResult<()> {
    let options = openbnct_plan::TableImportOptions {
        id,
        case_id,
        bundles_dir,
    };
    let plan = openbnct_plan::read_table(&table, &options).map_err(reject)?;
    let bytes = serde_json::to_vec_pretty(&plan).map_err(reject)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .and_then(|mut file| {
            use std::io::Write;
            file.write_all(&bytes).and_then(|()| file.write_all(b"\n"))
        })
        .map_err(reject)
}

/// Export an exposure-plan JSON file to a `.csv` or `.xlsx` exposure table
/// (same path as `openbnct plan export`).
#[pyfunction]
fn plan_table_write(plan: PathBuf, output: PathBuf) -> PyResult<()> {
    let plan: openbnct_core::ExposurePlan = load_contract(plan)?;
    openbnct_plan::write_table(&output, &plan).map_err(reject)
}

/// Import a `openbnct.component-dose-interchange/0.1.0` document produced by
/// an external transport pipeline into a validated physical dose bundle
/// (same path as `openbnct import interchange`).
#[pyfunction]
fn import_component_dose(interchange: PathBuf) -> PyResult<PyPhysicalDoseBundle> {
    let bytes = fs::read(&interchange).map_err(reject)?;
    let document: openbnct_core::ComponentDoseInterchange =
        serde_json::from_slice(&bytes).map_err(reject)?;
    use sha2::Digest;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
    Ok(PyPhysicalDoseBundle {
        inner: openbnct_core::import_component_dose(&document, &sha256).map_err(reject)?,
    })
}

fn dose_unit(unit: &str) -> PyResult<openbnct_core::DoseUnit> {
    match unit {
        "gray" => Ok(openbnct_core::DoseUnit::Gray),
        "gray_per_source_particle" => Ok(openbnct_core::DoseUnit::GrayPerSourceParticle),
        other => Err(PyValueError::new_err(format!(
            "unknown dose unit {other:?}"
        ))),
    }
}

fn dose_component(name: &str) -> PyResult<openbnct_core::DoseComponent> {
    match name {
        "boron" => Ok(openbnct_core::DoseComponent::Boron),
        "nitrogen" => Ok(openbnct_core::DoseComponent::Nitrogen),
        "hydrogen" => Ok(openbnct_core::DoseComponent::Hydrogen),
        "photon" => Ok(openbnct_core::DoseComponent::Photon),
        other => Err(PyValueError::new_err(format!(
            "unknown dose component {other:?}"
        ))),
    }
}

/// Lift MCNP meshtal component tallies into a physical dose bundle (same
/// path as `openbnct import mcnp`). `components` maps each component name to
/// `(meshtal_path, tally_number)` or `(meshtal_path, tally_number,
/// energy_bin)`.
#[pyfunction]
#[pyo3(signature = (components, case_id, unit, normalization, producer_version=None, frame_of_reference_uid=None))]
fn import_mcnp_meshtal(
    components: HashMap<String, Vec<Bound<'_, PyAny>>>,
    case_id: &str,
    unit: &str,
    normalization: &str,
    producer_version: Option<String>,
    frame_of_reference_uid: Option<String>,
) -> PyResult<PyPhysicalDoseBundle> {
    let mut sources = Vec::new();
    for (name, fields) in components {
        let bad = || {
            PyValueError::new_err(format!(
                "component {name:?}: expected (file, tally[, energy_bin])"
            ))
        };
        if fields.len() < 2 || fields.len() > 3 {
            return Err(bad());
        }
        sources.push(openbnct_mcnp::ComponentSource {
            component: dose_component(&name)?,
            file: fields[0].extract::<PathBuf>().map_err(|_| bad())?,
            tally: fields[1].extract::<u32>().map_err(|_| bad())?,
            energy_bin: if fields.len() == 3 {
                Some(fields[2].extract::<usize>().map_err(|_| bad())?)
            } else {
                None
            },
        });
    }
    let document = openbnct_mcnp::interchange_from_meshtals(
        &sources,
        case_id,
        dose_unit(unit)?,
        normalization,
        frame_of_reference_uid,
        producer_version,
    )
    .map_err(reject)?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(reject)?;
    use sha2::Digest;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
    Ok(PyPhysicalDoseBundle {
        inner: openbnct_core::import_component_dose(&document, &sha256).map_err(reject)?,
    })
}

/// Lift PHITS xyz-mesh tally files into a physical dose bundle (same path as
/// `openbnct import phits`). `components` maps each component name to a file
/// path or `(path, energy_index)` tuple; `FILE_err.ext` siblings supply
/// relative errors when present. `producer_version` is required.
#[pyfunction]
#[pyo3(signature = (components, case_id, unit, normalization, producer_version, frame_of_reference_uid=None))]
fn import_phits(
    components: HashMap<String, Bound<'_, PyAny>>,
    case_id: &str,
    unit: &str,
    normalization: &str,
    producer_version: &str,
    frame_of_reference_uid: Option<String>,
) -> PyResult<PyPhysicalDoseBundle> {
    let mut sources = Vec::new();
    for (name, spec) in components {
        let bad = || {
            PyValueError::new_err(format!(
                "component {name:?}: expected file path or (path, energy_index)"
            ))
        };
        let (file, energy_bin) = if let Ok(path) = spec.extract::<PathBuf>() {
            (path, None)
        } else if let Ok((path, ebin)) = spec.extract::<(PathBuf, usize)>() {
            (path, Some(ebin))
        } else {
            return Err(bad());
        };
        sources.push(openbnct_phits::ComponentSource {
            component: dose_component(&name)?,
            file,
            energy_bin,
        });
    }
    let document = openbnct_phits::interchange_from_phits(
        &sources,
        case_id,
        dose_unit(unit)?,
        normalization,
        frame_of_reference_uid,
        producer_version,
    )
    .map_err(reject)?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(reject)?;
    use sha2::Digest;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
    Ok(PyPhysicalDoseBundle {
        inner: openbnct_core::import_component_dose(&document, &sha256).map_err(reject)?,
    })
}

/// Import per-component NIfTI dose volumes into a physical dose bundle
/// (same path as `openbnct import nifti`). `components` maps a component
/// name (`boron`, `nitrogen`, `hydrogen`, `photon`) to either a `.nii`/
/// `.nii.gz` path or a `(path, sigma_path)` tuple pairing the value volume
/// with an absolute one-sigma volume on the same grid. `producer_system`
/// is required because NIfTI headers carry no producer identity.
#[pyfunction]
#[pyo3(signature = (components, case_id, unit, normalization, producer_system, producer_version=None, frame_of_reference_uid=None))]
fn import_nifti(
    components: HashMap<String, Bound<'_, PyAny>>,
    case_id: &str,
    unit: &str,
    normalization: &str,
    producer_system: &str,
    producer_version: Option<String>,
    frame_of_reference_uid: Option<String>,
) -> PyResult<PyPhysicalDoseBundle> {
    let mut sources = Vec::new();
    for (name, spec) in components {
        let bad = || {
            PyValueError::new_err(format!(
                "component {name:?}: expected file path or (path, sigma_path)"
            ))
        };
        let (file, sigma_file) = if let Ok(path) = spec.extract::<PathBuf>() {
            (path, None)
        } else if let Ok((path, sigma)) = spec.extract::<(PathBuf, PathBuf)>() {
            (path, Some(sigma))
        } else {
            return Err(bad());
        };
        sources.push(openbnct_nifti::NiftiComponentSource {
            component: dose_component(&name)?,
            file,
            sigma_file,
        });
    }
    let document = openbnct_nifti::interchange_from_niftis(
        &sources,
        case_id,
        dose_unit(unit)?,
        normalization,
        producer_system,
        producer_version,
        frame_of_reference_uid,
    )
    .map_err(reject)?;
    let bytes = serde_json::to_vec_pretty(&document).map_err(reject)?;
    use sha2::Digest;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
    Ok(PyPhysicalDoseBundle {
        inner: openbnct_core::import_component_dose(&document, &sha256).map_err(reject)?,
    })
}

/// Emit an MCNP input deck for a transport case (same path as
/// `openbnct export mcnp`). The deck scores flux on the case mesh; component
/// folding is the external pipeline's declared step before `import_mcnp_meshtal`
/// re-ingests the meshtal. Returns the deck text; also writes it to `output`.
#[pyfunction]
#[pyo3(signature = (case, output, assignment=None, xs_suffix=None, seed=None))]
fn export_mcnp_deck(
    case: PathBuf,
    output: PathBuf,
    assignment: Option<PathBuf>,
    xs_suffix: Option<String>,
    seed: Option<u64>,
) -> PyResult<String> {
    let case_bytes = std::fs::read(&case).map_err(reject)?;
    let case_doc: TransportCase = serde_json::from_slice(&case_bytes).map_err(reject)?;
    let assignment_doc = assignment
        .map(|path| {
            std::fs::read(&path).map_err(reject).and_then(|bytes| {
                serde_json::from_slice::<MaterialAssignment>(&bytes).map_err(reject)
            })
        })
        .transpose()?;
    use sha2::Digest;
    let deck = openbnct_mcnp::deck::export_mcnp_deck(
        &case_doc,
        assignment_doc.as_ref(),
        &openbnct_mcnp::deck::McnpDeckOptions {
            xs_suffix,
            seed,
            case_sha256: format!("sha256:{:x}", sha2::Sha256::digest(&case_bytes)),
        },
    )
    .map_err(reject)?;
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .map_err(reject)?;
    file.write_all(deck.as_bytes()).map_err(reject)?;
    file.sync_all().map_err(reject)?;
    Ok(deck)
}

/// A validated external-dose bundle (`openbnct.external-dose/0.1.0`).
#[pyclass(frozen, name = "ExternalDoseBundle")]
struct PyExternalDoseBundle {
    inner: openbnct_core::ExternalDoseBundle,
}

#[pymethods]
impl PyExternalDoseBundle {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// `physical` or `rbe_weighted` — the declared basis of the dose field.
    #[getter]
    fn quantity(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.quantity)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    /// Declared fraction count of the external course.
    #[getter]
    fn fractions(&self) -> usize {
        self.inner.fraction_count()
    }

    #[getter]
    fn values(&self) -> Vec<f64> {
        self.inner.values.clone()
    }

    #[getter]
    fn absolute_standard_uncertainty(&self) -> Option<Vec<f64>> {
        self.inner.absolute_standard_uncertainty.clone()
    }

    #[getter]
    fn geometry(&self) -> PyGeometry {
        PyGeometry {
            inner: self.inner.geometry.clone(),
        }
    }

    #[getter]
    fn provenance_id(&self) -> &str {
        &self.inner.provenance_id
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Import a `openbnct.external-dose/0.1.0` document into a provenance-bound
/// bundle (same path as `openbnct import dose`).
#[pyfunction]
fn import_external_dose(file: PathBuf) -> PyResult<PyExternalDoseBundle> {
    let bytes = fs::read(&file).map_err(reject)?;
    let document: openbnct_core::ExternalDoseDocument =
        serde_json::from_slice(&bytes).map_err(reject)?;
    use sha2::Digest;
    let sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
    Ok(PyExternalDoseBundle {
        inner: openbnct_core::import_external_dose(&document, &sha256).map_err(reject)?,
    })
}

/// Load an already-imported external dose bundle (e.g. one written by
/// `openbnct import dose`).
#[pyfunction]
fn load_external_dose_bundle(path: PathBuf) -> PyResult<PyExternalDoseBundle> {
    Ok(PyExternalDoseBundle {
        inner: load_contract(path)?,
    })
}

/// A BED or EQD2 field derived from an external dose course.
#[pyclass(frozen, name = "BedBundle")]
struct PyBedBundle {
    inner: openbnct_bio::BedBundle,
}

#[pymethods]
impl PyBedBundle {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// `bed` or `eqd2` — the biological quantity this field expresses.
    #[getter]
    fn quantity(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.quantity)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    /// `physical` or `rbe_weighted` — basis of the source dose.
    #[getter]
    fn quantity_basis(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.quantity_basis)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    #[getter]
    fn alpha_beta(&self) -> f64 {
        self.inner.alpha_beta
    }

    #[getter]
    fn fractions(&self) -> usize {
        self.inner.fractions
    }

    #[getter]
    fn values(&self) -> Vec<f64> {
        self.inner.values.clone()
    }

    #[getter]
    fn absolute_standard_uncertainty(&self) -> Option<Vec<f64>> {
        self.inner.absolute_standard_uncertainty.clone()
    }

    #[getter]
    fn geometry(&self) -> PyGeometry {
        PyGeometry {
            inner: self.inner.geometry.clone(),
        }
    }

    #[getter]
    fn external_dose_provenance(&self) -> &str {
        &self.inner.external_dose_provenance
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Load an external BED/EQD2 bundle (e.g. one written by `openbnct bio bed`).
#[pyfunction]
fn load_bed_bundle(path: PathBuf) -> PyResult<PyBedBundle> {
    Ok(PyBedBundle {
        inner: load_contract(path)?,
    })
}

/// Convert an external dose course to a BED or EQD2 field (same path as
/// `openbnct bio bed`). `region_alpha_beta` maps region names to α/β ratios;
/// each region needs a matching `(name, mask_path)` entry in `region_masks`.
/// `quantity` is `"bed"` or `"eqd2"` (default).
#[pyfunction]
#[pyo3(signature = (dose, alpha_beta, region_alpha_beta=None, region_masks=None, quantity="eqd2", region_priority=None))]
fn bed_from_external_dose(
    dose: &PyExternalDoseBundle,
    alpha_beta: f64,
    region_alpha_beta: Option<HashMap<String, f64>>,
    region_masks: Option<Vec<(String, PathBuf)>>,
    quantity: &str,
    region_priority: Option<Vec<String>>,
) -> PyResult<PyBedBundle> {
    let masks = load_named_masks(region_masks.unwrap_or_default())?;
    let overrides: BTreeMap<String, f64> =
        region_alpha_beta.unwrap_or_default().into_iter().collect();
    let quantity = match quantity {
        "bed" => openbnct_bio::BedQuantity::Bed,
        "eqd2" => openbnct_bio::BedQuantity::Eqd2,
        other => {
            return Err(PyValueError::new_err(format!(
                "quantity {other:?} must be bed or eqd2"
            )));
        }
    };
    Ok(PyBedBundle {
        inner: openbnct_bio::bed_from_external(
            &dose.inner,
            alpha_beta,
            &overrides,
            &masks,
            quantity,
            &region_priority.unwrap_or_default(),
        )
        .map_err(reject)?,
    })
}

/// A combined BNCT + external-course biological evaluation (`eqd2`).
#[pyclass(frozen, name = "CombinedDoseBundle")]
struct PyCombinedDoseBundle {
    inner: openbnct_bio::CombinedDoseBundle,
}

#[pymethods]
impl PyCombinedDoseBundle {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// Combined quantity label; currently `eqd2`.
    #[getter]
    fn quantity(&self) -> &str {
        &self.inner.quantity
    }

    #[getter]
    fn values(&self) -> Vec<f64> {
        self.inner.values.clone()
    }

    #[getter]
    fn absolute_standard_uncertainty(&self) -> Option<Vec<f64>> {
        self.inner.absolute_standard_uncertainty.clone()
    }

    /// `(role, id, sha256, provenance_id)` for each consumed input.
    #[getter]
    fn inputs(&self) -> Vec<(String, String, String, String)> {
        self.inner
            .inputs
            .iter()
            .map(|input| {
                (
                    input.role.clone(),
                    input.content.id.clone(),
                    input.content.sha256.clone(),
                    input.provenance_id.clone(),
                )
            })
            .collect()
    }

    /// `trilinear` when the external field was resampled, else `None`.
    #[getter]
    fn external_resampling(&self) -> Option<String> {
        self.inner.external_resampling.map(|_| "trilinear".into())
    }

    /// `physical` or `rbe_weighted` — basis of the external course.
    #[getter]
    fn external_quantity_basis(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.external_quantity_basis)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    /// The operator-declared additivity assumption.
    #[getter]
    fn additivity_assumption(&self) -> &str {
        &self.inner.additivity_assumption
    }

    #[getter]
    fn qualification(&self) -> &str {
        &self.inner.qualification
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Add an external EQD2 course to a photon-isoeffective BNCT EQD2 bundle
/// (same path as `openbnct bio combine`). `resample` is `None` or
/// `"trilinear"`; `assumption` is a required operator statement recorded in
/// the output. Input content references bind the canonical serialization of
/// the artifacts consumed.
#[pyfunction]
#[pyo3(signature = (primary, external, resample=None, assumption=""))]
fn combine_biological_doses(
    primary: &PyBiologicalDoseBundle,
    external: &PyBedBundle,
    resample: Option<&str>,
    assumption: &str,
) -> PyResult<PyCombinedDoseBundle> {
    let resample = match resample {
        None => None,
        Some("trilinear") => Some(ResampleMethod::Trilinear),
        Some(other) => {
            return Err(PyValueError::new_err(format!(
                "resample method {other:?} is not supported (available: trilinear)"
            )));
        }
    };
    Ok(PyCombinedDoseBundle {
        inner: openbnct_bio::combine_biological_doses(
            &primary.inner,
            &external.inner,
            content_reference("biological-dose-bundle", &primary.inner)?,
            content_reference("bed-bundle", &external.inner)?,
            resample,
            assumption,
        )
        .map_err(reject)?,
    })
}

/// `(quantity, unit, max_abs, mean_abs, rms, max_normalized,
/// within_sigma_fraction)` row returned by `DoseComparison.quantities`.
type QuantityComparisonRow = (String, String, f64, f64, f64, f64, Option<f64>);

/// A cross-code dose-comparison record (`openbnct.dose-comparison/0.1.0`).
#[pyclass(frozen, name = "DoseComparison")]
struct PyDoseComparison {
    inner: openbnct_evidence::DoseComparison,
}

#[pymethods]
impl PyDoseComparison {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    #[getter]
    fn sigma_level(&self) -> f64 {
        self.inner.sigma_level
    }

    #[getter]
    fn voxel_count(&self) -> u64 {
        self.inner.voxel_count
    }

    /// `(role, id, sha256, provenance_id)` for each compared input.
    #[getter]
    fn inputs(&self) -> Vec<(String, String, String, String)> {
        self.inner
            .inputs
            .iter()
            .map(|input| {
                (
                    input.role.clone(),
                    input.content.id.clone(),
                    input.content.sha256.clone(),
                    input.provenance_id.clone(),
                )
            })
            .collect()
    }

    /// `(quantity, unit, max_abs, mean_abs, rms, max_normalized,
    /// within_sigma_fraction)` per component plus `physical_total`.
    #[getter]
    fn quantities(&self) -> Vec<QuantityComparisonRow> {
        self.inner
            .quantities
            .iter()
            .map(|q| {
                (
                    q.quantity.clone(),
                    q.unit.clone(),
                    q.max_abs_difference,
                    q.mean_abs_difference,
                    q.rms_difference,
                    q.max_normalized_difference,
                    q.within_sigma_fraction,
                )
            })
            .collect()
    }

    #[getter]
    fn qualification(&self) -> &str {
        &self.inner.qualification
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Compare two physical dose bundles on the same frozen case (same path as
/// `openbnct compare`). Records voxelwise agreement per component and total
/// under both inputs' content hashes — a research record, never an
/// equivalence claim.
#[pyfunction]
#[pyo3(signature = (reference, candidate, sigma_level=2.0))]
fn compare_dose_bundles(
    reference: &PyPhysicalDoseBundle,
    candidate: &PyPhysicalDoseBundle,
    sigma_level: f64,
) -> PyResult<PyDoseComparison> {
    Ok(PyDoseComparison {
        inner: openbnct_evidence::compare_dose_bundles(
            &reference.inner,
            &candidate.inner,
            content_reference("reference", &reference.inner)?,
            content_reference("candidate", &candidate.inner)?,
            sigma_level,
        )
        .map_err(reject)?,
    })
}

/// A gamma-index evaluation record.
#[pyclass(frozen, name = "GammaEvaluation")]
struct PyGammaEvaluation {
    inner: openbnct_evidence::GammaEvaluation,
}

/// `(quantity, unit, voxels_evaluated, voxels_excluded, pass_rate,
/// mean_gamma, p95_gamma, max_gamma)` per component plus `physical_total`.
type GammaRow = (
    String,
    String,
    u64,
    u64,
    f64,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

#[pymethods]
impl PyGammaEvaluation {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// `(dose_difference_percent, distance_to_agreement_mm, normalization,
    /// dose_threshold_percent)`.
    #[getter]
    fn criteria(&self) -> (f64, f64, String, Option<f64>) {
        (
            self.inner.criteria.dose_difference_percent,
            self.inner.criteria.distance_to_agreement_mm,
            match self.inner.criteria.normalization {
                openbnct_evidence::GammaNormalization::Global => "global".to_string(),
                openbnct_evidence::GammaNormalization::Local => "local".to_string(),
            },
            self.inner.criteria.dose_threshold_percent,
        )
    }

    /// `(role, id, sha256, provenance_id)` for each compared input.
    #[getter]
    fn inputs(&self) -> Vec<(String, String, String, String)> {
        self.inner
            .inputs
            .iter()
            .map(|input| {
                (
                    input.role.clone(),
                    input.content.id.clone(),
                    input.content.sha256.clone(),
                    input.provenance_id.clone(),
                )
            })
            .collect()
    }

    #[getter]
    fn results(&self) -> Vec<GammaRow> {
        self.inner
            .results
            .iter()
            .map(|r| {
                (
                    r.quantity.clone(),
                    r.unit.clone(),
                    r.voxels_evaluated,
                    r.voxels_excluded,
                    r.pass_rate,
                    r.mean_gamma,
                    r.p95_gamma,
                    r.max_gamma,
                )
            })
            .collect()
    }

    /// Per-voxel γ for one quantity (aligned to grid order), when the
    /// volume was emitted; `None` entries mark threshold-excluded voxels.
    fn gamma_volume(&self, quantity: &str) -> Option<Vec<Option<f64>>> {
        self.inner
            .results
            .iter()
            .find(|r| r.quantity == quantity)
            .and_then(|r| r.gamma_volume.clone())
    }

    #[getter]
    fn qualification(&self) -> &str {
        &self.inner.qualification
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Evaluate the Low gamma index between two physical dose bundles on the
/// same frozen case (same path as `openbnct gamma`). `normalization` is
/// `"global"` (percent of the reference maximum) or `"local"` (percent of
/// the evaluated reference voxel); `dose_threshold_percent` excludes
/// low-dose reference voxels. A research record, never an equivalence
/// claim.
#[pyfunction]
#[pyo3(signature = (reference, candidate, dose_difference_percent=3.0, distance_to_agreement_mm=3.0, normalization="global", dose_threshold_percent=None, emit_gamma_volume=false))]
fn evaluate_gamma(
    reference: &PyPhysicalDoseBundle,
    candidate: &PyPhysicalDoseBundle,
    dose_difference_percent: f64,
    distance_to_agreement_mm: f64,
    normalization: &str,
    dose_threshold_percent: Option<f64>,
    emit_gamma_volume: bool,
) -> PyResult<PyGammaEvaluation> {
    let normalization = match normalization {
        "global" => openbnct_evidence::GammaNormalization::Global,
        "local" => openbnct_evidence::GammaNormalization::Local,
        other => {
            return Err(reject(openbnct_evidence::ManifestError::Invalid(format!(
                "normalization must be \"global\" or \"local\", got {other:?}"
            ))));
        }
    };
    Ok(PyGammaEvaluation {
        inner: openbnct_evidence::evaluate_gamma(
            &reference.inner,
            &candidate.inner,
            content_reference("reference", &reference.inner)?,
            content_reference("candidate", &candidate.inner)?,
            openbnct_evidence::GammaCriteria {
                dose_difference_percent,
                distance_to_agreement_mm,
                normalization,
                dose_threshold_percent,
            },
            emit_gamma_volume,
        )
        .map_err(reject)?,
    })
}

/// A validated biological model contract.
#[pyclass(frozen, name = "BiologicalModel")]
struct PyBiologicalModel {
    inner: BiologicalModel,
    bytes: Vec<u8>,
}

#[pymethods]
impl PyBiologicalModel {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Load and validate a `openbnct.biological-model/0.2.0` artifact.
#[pyfunction]
fn load_biological_model(path: PathBuf) -> PyResult<PyBiologicalModel> {
    let bytes = fs::read(&path).map_err(reject)?;
    let model: BiologicalModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyBiologicalModel {
        inner: model,
        bytes,
    })
}

/// Validate a biological-model document authored in Python (a `dict` matching
/// the `openbnct.biological-model/0.2.0` schema) into a usable model object —
/// the external-experiment path: researchers supply their own weights and
/// fractionation without writing a JSON file, and validation, content
/// hashing, and `apply_model` behavior stay identical to the file path.
#[pyfunction]
fn make_biological_model(document: Bound<'_, PyAny>) -> PyResult<PyBiologicalModel> {
    let json = if let Ok(text) = document.extract::<String>() {
        text
    } else {
        let module = document.py().import("json").map_err(reject)?;
        module
            .call_method1("dumps", (&document,))
            .and_then(|v| v.extract::<String>())
            .map_err(reject)?
    };
    let bytes = json.into_bytes();
    let model: BiologicalModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyBiologicalModel {
        inner: model,
        bytes,
    })
}

/// A validated cross-model biological comparison artifact.
#[pyclass(frozen, name = "BioModelComparison")]
struct PyBioModelComparison {
    inner: openbnct_bio::BioModelComparison,
}

#[pymethods]
impl PyBioModelComparison {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    #[getter]
    fn max_voxelwise_ratio(&self) -> Option<f64> {
        self.inner.max_voxelwise_ratio
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Read and validate a `openbnct.bio-model-comparison/0.1.0` artifact.
#[pyfunction]
fn load_bio_model_comparison(path: PathBuf) -> PyResult<PyBioModelComparison> {
    Ok(PyBioModelComparison {
        inner: load_contract(path)?,
    })
}

/// A validated biological dose bundle; weighted values never alias physical dose.
#[pyclass(frozen, name = "BiologicalDoseBundle")]
struct PyBiologicalDoseBundle {
    inner: BiologicalDoseBundle,
}

/// The fractionation schedule a model applied to a bundle's total.
#[pyclass(frozen, name = "AppliedFractionation")]
struct PyAppliedFractionation {
    inner: AppliedFractionation,
}

#[pymethods]
impl PyAppliedFractionation {
    #[getter]
    fn fraction_count(&self) -> u32 {
        self.inner.fraction_count
    }

    #[getter]
    fn source_particles_per_fraction(&self) -> f64 {
        self.inner.source_particles_per_fraction
    }

    #[getter]
    fn regions_applied(&self) -> Vec<String> {
        self.inner.regions_applied.clone()
    }
}

#[pymethods]
impl PyBiologicalDoseBundle {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// Weighted unit label, deliberately never `gray`.
    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    /// `fixed_per_component` or `photon_isoeffective`.
    #[getter]
    fn weight_semantics(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.weight_semantics)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    /// The applied fractionation schedule, when the model declared one.
    #[getter]
    fn fractionation(&self) -> Option<PyAppliedFractionation> {
        self.inner
            .fractionation
            .clone()
            .map(|inner| PyAppliedFractionation { inner })
    }

    #[getter]
    fn geometry(&self) -> PyGeometry {
        PyGeometry {
            inner: self.inner.geometry.clone(),
        }
    }

    #[getter]
    fn components(&self) -> Vec<PyDoseVolume> {
        self.inner
            .components
            .iter()
            .map(|volume| PyDoseVolume {
                component: serde_json::to_value(volume.component)
                    .and_then(serde_json::from_value::<String>)
                    .unwrap_or_else(|_| "unknown".into()),
                unit: volume.unit.clone(),
                values: volume.values.clone(),
                absolute_standard_uncertainty: volume.absolute_standard_uncertainty.clone(),
            })
            .collect()
    }

    /// Biological-total dose volume with correlated component-sum uncertainty.
    #[getter]
    fn biological_total(&self) -> PyDoseVolume {
        PyDoseVolume {
            component: "biological_total".into(),
            unit: self.inner.total.unit.clone(),
            values: self.inner.total.values.clone(),
            absolute_standard_uncertainty: self.inner.total.absolute_standard_uncertainty.clone(),
        }
    }

    #[getter]
    fn physical_bundle_provenance(&self) -> &str {
        &self.inner.physical_bundle_provenance
    }

    #[getter]
    fn regions_applied(&self) -> Vec<String> {
        self.inner.regions_applied.clone()
    }

    #[getter]
    fn qualification(&self) -> &str {
        &self.inner.qualification
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    /// Write the bundle JSON; refuses to overwrite an existing file.
    fn write(&self, output: PathBuf) -> PyResult<()> {
        let bytes = serde_json::to_vec_pretty(&self.inner).map_err(reject)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(reject)?;
        use std::io::Write as _;
        file.write_all(&bytes)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(reject)
    }
}

/// Apply a biological model to a physical bundle using the authoritative Rust
/// path. `region_masks` maps each model region name to a RegionMask JSON file.
#[pyfunction]
fn apply_model(
    model: &PyBiologicalModel,
    physical: &PyPhysicalDoseBundle,
    region_masks: Vec<(String, PathBuf)>,
) -> PyResult<PyBiologicalDoseBundle> {
    let masks = load_region_masks(region_masks)?;
    let bundle = apply_biological_model(&model.inner, &model.bytes, &physical.inner, &masks)
        .map_err(reject)?;
    Ok(PyBiologicalDoseBundle { inner: bundle })
}

fn load_region_masks(region_masks: Vec<(String, PathBuf)>) -> PyResult<Vec<RegionMask>> {
    let mut masks = Vec::new();
    for (name, path) in region_masks {
        let mask: RegionMask =
            serde_json::from_slice(&fs::read(&path).map_err(reject)?).map_err(reject)?;
        if mask.name != name {
            return Err(reject(format!(
                "region mask {} is named {:?}, expected {name:?}",
                path.display(),
                mask.name
            )));
        }
        masks.push(mask);
    }
    Ok(masks)
}

/// A validated microdosimetric (linearized-MKM) model contract.
#[pyclass(frozen, name = "MicrodosimetricModel")]
struct PyMicrodosimetricModel {
    inner: MicrodosimetricModel,
    bytes: Vec<u8>,
}

#[pymethods]
impl PyMicrodosimetricModel {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Load and validate an `openbnct.microdosimetric-model/0.1.0` artifact.
#[pyfunction]
fn load_microdosimetric_model(path: PathBuf) -> PyResult<PyMicrodosimetricModel> {
    let bytes = fs::read(&path).map_err(reject)?;
    let model: MicrodosimetricModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyMicrodosimetricModel {
        inner: model,
        bytes,
    })
}

/// A validated González & Santa Cruz photon-isoeffective model contract.
#[pyclass(frozen, name = "IsoeffectiveModel")]
struct PyIsoeffectiveModel {
    inner: IsoeffectiveModel,
    bytes: Vec<u8>,
}

#[pymethods]
impl PyIsoeffectiveModel {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Load and validate an `openbnct.isoeffective-model/0.1.0` artifact.
#[pyfunction]
fn load_isoeffective_model(path: PathBuf) -> PyResult<PyIsoeffectiveModel> {
    let bytes = fs::read(&path).map_err(reject)?;
    let model: IsoeffectiveModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyIsoeffectiveModel {
        inner: model,
        bytes,
    })
}

/// Validate a microdosimetric-model document authored in Python (a `dict`
/// matching `openbnct.microdosimetric-model/0.1.0`, or a JSON string).
#[pyfunction]
fn make_microdosimetric_model(document: Bound<'_, PyAny>) -> PyResult<PyMicrodosimetricModel> {
    let json = if let Ok(text) = document.extract::<String>() {
        text
    } else {
        let module = document.py().import("json").map_err(reject)?;
        module
            .call_method1("dumps", (&document,))
            .and_then(|v| v.extract::<String>())
            .map_err(reject)?
    };
    let bytes = json.into_bytes();
    let model: MicrodosimetricModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyMicrodosimetricModel {
        inner: model,
        bytes,
    })
}

/// Validate an isoeffective-model document authored in Python (a `dict`
/// matching `openbnct.isoeffective-model/0.1.0`, or a JSON string).
#[pyfunction]
fn make_isoeffective_model(document: Bound<'_, PyAny>) -> PyResult<PyIsoeffectiveModel> {
    let json = if let Ok(text) = document.extract::<String>() {
        text
    } else {
        let module = document.py().import("json").map_err(reject)?;
        module
            .call_method1("dumps", (&document,))
            .and_then(|v| v.extract::<String>())
            .map_err(reject)?
    };
    let bytes = json.into_bytes();
    let model: IsoeffectiveModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyIsoeffectiveModel {
        inner: model,
        bytes,
    })
}

/// Apply a linearized-MKM model via the authoritative Rust path —
/// combined-effect inversion, not per-component. `spectra` are
/// `openbnct.lineal-spectrum/0.1.0` file paths for components declaring
/// a spectrum source.
#[pyfunction]
fn apply_mkm_model(
    model: &PyMicrodosimetricModel,
    physical: &PyPhysicalDoseBundle,
    region_masks: Vec<(String, PathBuf)>,
    spectra: Vec<PathBuf>,
) -> PyResult<PyBiologicalDoseBundle> {
    let masks = load_region_masks(region_masks)?;
    let mut spectrum_docs = Vec::with_capacity(spectra.len());
    for path in &spectra {
        let bytes = fs::read(path).map_err(reject)?;
        let spectrum: LinealSpectrum = serde_json::from_slice(&bytes)
            .map_err(|e| reject(format!("spectrum {}: {e}", path.display())))?;
        spectrum_docs.push((spectrum, bytes));
    }
    let inputs: Vec<SpectrumInput<'_>> = spectrum_docs
        .iter()
        .map(|(spectrum, bytes)| SpectrumInput {
            spectrum,
            document_bytes: bytes,
        })
        .collect();
    let bundle =
        apply_microdosimetric_model(&model.inner, &model.bytes, &physical.inner, &masks, &inputs)
            .map_err(reject)?;
    Ok(PyBiologicalDoseBundle { inner: bundle })
}

/// Apply a González & Santa Cruz photon-isoeffective model via the
/// authoritative Rust path — mixed-field exponent with √rbe_beta synergy
/// and the Lea–Catcheside repair factor, inverted once against the
/// photon LQ.
#[pyfunction]
fn apply_isoeffective(
    model: &PyIsoeffectiveModel,
    physical: &PyPhysicalDoseBundle,
    region_masks: Vec<(String, PathBuf)>,
) -> PyResult<PyBiologicalDoseBundle> {
    let masks = load_region_masks(region_masks)?;
    let bundle = apply_isoeffective_model(&model.inner, &model.bytes, &physical.inner, &masks)
        .map_err(reject)?;
    Ok(PyBiologicalDoseBundle { inner: bundle })
}

/// A deterministic dose-volume histogram over a named voxel mask.
#[pyclass(frozen, name = "DoseVolumeHistogram")]
struct PyDoseVolumeHistogram {
    inner: openbnct_evidence::DoseVolumeHistogram,
}

#[pymethods]
impl PyDoseVolumeHistogram {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn region(&self) -> &str {
        &self.inner.region
    }

    #[getter]
    fn quantity(&self) -> &str {
        &self.inner.quantity
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    #[getter]
    fn dose_edges(&self) -> Vec<f64> {
        self.inner.dose_edges.clone()
    }

    #[getter]
    fn differential_volume_fraction(&self) -> Vec<f64> {
        self.inner.differential_volume_fraction.clone()
    }

    /// `V(d)`: fraction of the region receiving at least each edge dose.
    #[getter]
    fn cumulative_volume_fraction(&self) -> Vec<f64> {
        self.inner.cumulative_volume_fraction.clone()
    }

    #[getter]
    fn region_voxel_count(&self) -> u64 {
        self.inner.region_voxel_count
    }

    #[getter]
    fn region_volume_mm3(&self) -> f64 {
        self.inner.region_volume_mm3
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Compute a dose-volume histogram for `quantity` over `mask_voxels` in
/// `bundle`'s grid order. `quantity` is `component:NAME`, `physical_total`,
/// or `biological_total`.
#[pyfunction]
fn compute_dvh(
    physical: &PyPhysicalDoseBundle,
    quantity: &str,
    mask_name: &str,
    mask_voxels: Vec<bool>,
    bins: usize,
) -> PyResult<PyDoseVolumeHistogram> {
    let bundle = &physical.inner;
    let (values, unit) = if let Some(name) = quantity.strip_prefix("component:") {
        let component = bundle
            .components
            .iter()
            .find(|volume| {
                serde_json::to_value(volume.component)
                    .map(|v| v == serde_json::Value::String(name.into()))
                    .unwrap_or(false)
            })
            .ok_or_else(|| reject(format!("bundle lacks component {name}")))?;
        (component.values.as_slice(), dose_unit_name(component.unit))
    } else if quantity == "physical_total" {
        (
            bundle.physical_total.values.as_slice(),
            dose_unit_name(bundle.physical_total.unit),
        )
    } else {
        return Err(reject(format!("unknown physical quantity {quantity:?}")));
    };
    let source = openbnct_core::ContentReference {
        id: bundle.case_id.clone(),
        sha256: sha256_hex_of_json(bundle)?,
    };
    let voxel_volume: f64 = bundle.geometry.spacing_mm.iter().product();
    let histogram = openbnct_evidence::DoseVolumeHistogram::compute(
        &bundle.case_id,
        mask_name,
        quantity,
        source,
        &unit,
        values,
        &mask_voxels,
        voxel_volume,
        bins,
    )
    .map_err(reject)?;
    Ok(PyDoseVolumeHistogram { inner: histogram })
}

/// Same as `compute_dvh` for a biological bundle.
#[pyfunction]
fn compute_dvh_biological(
    bundle: &PyBiologicalDoseBundle,
    quantity: &str,
    mask_name: &str,
    mask_voxels: Vec<bool>,
    bins: usize,
) -> PyResult<PyDoseVolumeHistogram> {
    let inner = &bundle.inner;
    let (values, unit) = if let Some(name) = quantity.strip_prefix("component:") {
        let component = inner
            .components
            .iter()
            .find(|volume| {
                serde_json::to_value(volume.component)
                    .map(|v| v == serde_json::Value::String(name.into()))
                    .unwrap_or(false)
            })
            .ok_or_else(|| reject(format!("bundle lacks component {name}")))?;
        (component.values.as_slice(), component.unit.clone())
    } else if quantity == "biological_total" {
        (inner.total.values.as_slice(), inner.total.unit.clone())
    } else {
        return Err(reject(format!("unknown biological quantity {quantity:?}")));
    };
    let source = openbnct_core::ContentReference {
        id: inner.case_id.clone(),
        sha256: sha256_hex_of_json(inner)?,
    };
    let voxel_volume: f64 = inner.geometry.spacing_mm.iter().product();
    let histogram = openbnct_evidence::DoseVolumeHistogram::compute(
        &inner.case_id,
        mask_name,
        quantity,
        source,
        &unit,
        values,
        &mask_voxels,
        voxel_volume,
        bins,
    )
    .map_err(reject)?;
    Ok(PyDoseVolumeHistogram { inner: histogram })
}

fn sha256_hex_of_json<T: serde::Serialize>(value: &T) -> PyResult<String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(reject)?;
    Ok(openbnct_evidence::sha256_hex(&bytes))
}

/// Resolve `(values, unit)` for a quantity over a physical bundle.
fn physical_selection<'a>(
    bundle: &'a openbnct_core::PhysicalDoseBundle,
    quantity: &str,
) -> PyResult<(&'a [f64], String)> {
    if let Some(name) = quantity.strip_prefix("component:") {
        let component = bundle
            .components
            .iter()
            .find(|volume| {
                serde_json::to_value(volume.component)
                    .map(|v| v == serde_json::Value::String(name.into()))
                    .unwrap_or(false)
            })
            .ok_or_else(|| reject(format!("bundle lacks component {name}")))?;
        return Ok((component.values.as_slice(), dose_unit_name(component.unit)));
    }
    if quantity == "physical_total" {
        return Ok((
            bundle.physical_total.values.as_slice(),
            dose_unit_name(bundle.physical_total.unit),
        ));
    }
    Err(reject(format!("unknown physical quantity {quantity:?}")))
}

/// Resolve `(values, unit)` for a quantity over a biological bundle.
fn biological_selection<'a>(
    bundle: &'a BiologicalDoseBundle,
    quantity: &str,
) -> PyResult<(&'a [f64], String)> {
    if let Some(name) = quantity.strip_prefix("component:") {
        let component = bundle
            .components
            .iter()
            .find(|volume| {
                serde_json::to_value(volume.component)
                    .map(|v| v == serde_json::Value::String(name.into()))
                    .unwrap_or(false)
            })
            .ok_or_else(|| reject(format!("bundle lacks component {name}")))?;
        return Ok((component.values.as_slice(), component.unit.clone()));
    }
    if quantity == "biological_total" {
        return Ok((bundle.total.values.as_slice(), bundle.total.unit.clone()));
    }
    Err(reject(format!("unknown biological quantity {quantity:?}")))
}

/// Exact dose-volume metrics over a named voxel mask.
#[pyclass(frozen, name = "RegionDoseMetrics")]
struct PyRegionDoseMetrics {
    inner: openbnct_evidence::RegionDoseMetrics,
}

#[pymethods]
impl PyRegionDoseMetrics {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn region(&self) -> &str {
        &self.inner.region
    }

    #[getter]
    fn quantity(&self) -> &str {
        &self.inner.quantity
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    #[getter]
    fn minimum_dose(&self) -> f64 {
        self.inner.minimum_dose
    }

    #[getter]
    fn mean_dose(&self) -> f64 {
        self.inner.mean_dose
    }

    #[getter]
    fn maximum_dose(&self) -> f64 {
        self.inner.maximum_dose
    }

    /// Requested `D_x` readings as `(percent, dose)` pairs.
    #[getter]
    fn dx(&self) -> Vec<(f64, f64)> {
        self.inner
            .dx
            .iter()
            .map(|metric| (metric.percent, metric.dose))
            .collect()
    }

    /// Requested `V_x` readings as `(level, volume_fraction)` pairs.
    #[getter]
    fn vx(&self) -> Vec<(f64, f64)> {
        self.inner
            .vx
            .iter()
            .map(|metric| (metric.level, metric.volume_fraction))
            .collect()
    }

    /// Requested EUD readings as `(a, dose)` pairs.
    #[getter]
    fn eud(&self) -> Vec<(f64, f64)> {
        self.inner
            .eud
            .iter()
            .map(|metric| (metric.a, metric.dose))
            .collect()
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

#[allow(clippy::too_many_arguments)]
fn region_dose_metrics(
    case_id: &str,
    quantity: &str,
    mask_name: &str,
    mask_voxels: &[bool],
    source_sha256: String,
    unit: &str,
    values: &[f64],
    voxel_volume: f64,
    dx: Vec<f64>,
    vx: Vec<f64>,
    eud: Vec<f64>,
) -> PyResult<PyRegionDoseMetrics> {
    let source = openbnct_core::ContentReference {
        id: case_id.to_string(),
        sha256: source_sha256,
    };
    let metrics = openbnct_evidence::RegionDoseMetrics::compute(
        case_id,
        mask_name,
        quantity,
        source,
        unit,
        values,
        mask_voxels,
        voxel_volume,
        &dx,
        &vx,
        &eud,
    )
    .map_err(reject)?;
    Ok(PyRegionDoseMetrics { inner: metrics })
}

/// Compute exact dose-volume metrics (D_x, V_x, min/mean/max, EUD) for
/// `quantity` over `mask_voxels` in `bundle`'s grid order.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn compute_metrics(
    physical: &PyPhysicalDoseBundle,
    quantity: &str,
    mask_name: &str,
    mask_voxels: Vec<bool>,
    dx: Vec<f64>,
    vx: Vec<f64>,
    eud: Vec<f64>,
) -> PyResult<PyRegionDoseMetrics> {
    let bundle = &physical.inner;
    let (values, unit) = physical_selection(bundle, quantity)?;
    region_dose_metrics(
        &bundle.case_id,
        quantity,
        mask_name,
        &mask_voxels,
        sha256_hex_of_json(bundle)?,
        &unit,
        values,
        bundle.geometry.spacing_mm.iter().product(),
        dx,
        vx,
        eud,
    )
}

/// Same as `compute_metrics` for a biological bundle.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn compute_metrics_biological(
    bundle: &PyBiologicalDoseBundle,
    quantity: &str,
    mask_name: &str,
    mask_voxels: Vec<bool>,
    dx: Vec<f64>,
    vx: Vec<f64>,
    eud: Vec<f64>,
) -> PyResult<PyRegionDoseMetrics> {
    let inner = &bundle.inner;
    let (values, unit) = biological_selection(inner, quantity)?;
    region_dose_metrics(
        &inner.case_id,
        quantity,
        mask_name,
        &mask_voxels,
        sha256_hex_of_json(inner)?,
        &unit,
        values,
        inner.geometry.spacing_mm.iter().product(),
        dx,
        vx,
        eud,
    )
}

/// A validated `openbnct.endpoint-model/0.1.0` artifact with its source bytes.
#[pyclass(frozen, name = "EndpointModel")]
struct PyEndpointModel {
    inner: openbnct_bio::EndpointModel,
    bytes: Vec<u8>,
}

#[pymethods]
impl PyEndpointModel {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn id(&self) -> &str {
        &self.inner.id
    }

    /// `tcp` or `ntcp`.
    #[getter]
    fn endpoint(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.endpoint)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Load and validate an `openbnct.endpoint-model/0.1.0` artifact.
#[pyfunction]
fn load_endpoint_model(path: PathBuf) -> PyResult<PyEndpointModel> {
    let bytes = fs::read(&path).map_err(reject)?;
    let model: openbnct_bio::EndpointModel = serde_json::from_slice(&bytes).map_err(reject)?;
    model.validate().map_err(reject)?;
    Ok(PyEndpointModel {
        inner: model,
        bytes,
    })
}

/// The scalar dose statistic a volume-collapsed endpoint consumed.
#[pyclass(frozen, name = "AppliedDoseStatistic")]
struct PyAppliedDoseStatistic {
    inner: openbnct_bio::AppliedDoseStatistic,
}

#[pymethods]
impl PyAppliedDoseStatistic {
    #[getter]
    fn kind(&self) -> &str {
        &self.inner.kind
    }

    /// EUD organ parameter when `kind` is `eud`.
    #[getter]
    fn parameter(&self) -> Option<f64> {
        self.inner.parameter
    }

    #[getter]
    fn value(&self) -> f64 {
        self.inner.value
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }
}

/// A scored `openbnct.endpoint-evaluation/0.1.0` report.
#[pyclass(frozen, name = "EndpointEvaluation")]
struct PyEndpointEvaluation {
    inner: openbnct_bio::EndpointEvaluation,
    bytes: Vec<u8>,
}

#[pymethods]
impl PyEndpointEvaluation {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// `tcp`, `ntcp`, or `utcp`.
    #[getter]
    fn endpoint(&self) -> PyResult<String> {
        serde_json::to_value(self.inner.endpoint)
            .and_then(serde_json::from_value::<String>)
            .map_err(reject)
    }

    #[getter]
    fn region(&self) -> &str {
        &self.inner.region
    }

    #[getter]
    fn quantity(&self) -> &str {
        &self.inner.quantity
    }

    #[getter]
    fn probability(&self) -> f64 {
        self.inner.probability
    }

    /// The consumed scalar statistic; absent for `voxel_poisson_tcp` and
    /// UTCP combinations.
    #[getter]
    fn dose_statistic(&self) -> Option<PyAppliedDoseStatistic> {
        self.inner
            .dose_statistic
            .clone()
            .map(|inner| PyAppliedDoseStatistic { inner })
    }

    #[getter]
    fn qualification(&self) -> &str {
        &self.inner.qualification
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    /// Write the evaluation JSON; refuses to overwrite an existing file.
    fn write(&self, output: PathBuf) -> PyResult<()> {
        use std::io::Write as _;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(reject)?;
        file.write_all(&self.bytes).map_err(reject)?;
        file.write_all(b"\n").map_err(reject)?;
        file.sync_all().map_err(reject)
    }
}

/// Load and validate an endpoint-evaluation report (e.g. produced by the
/// CLI), keeping its file bytes as the content identity.
#[pyfunction]
fn load_endpoint_evaluation(path: PathBuf) -> PyResult<PyEndpointEvaluation> {
    let bytes = fs::read(&path).map_err(reject)?;
    let evaluation: openbnct_bio::EndpointEvaluation =
        serde_json::from_slice(&bytes).map_err(reject)?;
    evaluation.validate().map_err(reject)?;
    Ok(PyEndpointEvaluation {
        inner: evaluation,
        bytes,
    })
}

#[allow(clippy::too_many_arguments)]
fn run_endpoint_evaluation(
    model: &PyEndpointModel,
    case_id: &str,
    mask_name: &str,
    mask_voxels: &[bool],
    quantity: &str,
    unit: &str,
    values: &[f64],
    voxel_volume: f64,
    source_sha256: String,
) -> PyResult<PyEndpointEvaluation> {
    let mask = RegionMask {
        name: mask_name.to_string(),
        voxels: mask_voxels.to_vec(),
    };
    let source = openbnct_core::ContentReference {
        id: case_id.to_string(),
        sha256: source_sha256,
    };
    let evaluation = openbnct_bio::evaluate_endpoint(
        &model.inner,
        &model.bytes,
        case_id,
        &mask,
        quantity,
        unit,
        values,
        voxel_volume,
        source,
    )
    .map_err(reject)?;
    let bytes = serde_json::to_vec_pretty(&evaluation).map_err(reject)?;
    Ok(PyEndpointEvaluation {
        inner: evaluation,
        bytes,
    })
}

/// Score an endpoint model over a physical bundle's `quantity` restricted
/// to `mask_voxels` in grid order.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn evaluate_endpoint(
    model: &PyEndpointModel,
    physical: &PyPhysicalDoseBundle,
    quantity: &str,
    mask_name: &str,
    mask_voxels: Vec<bool>,
) -> PyResult<PyEndpointEvaluation> {
    let bundle = &physical.inner;
    let (values, unit) = physical_selection(bundle, quantity)?;
    run_endpoint_evaluation(
        model,
        &bundle.case_id,
        mask_name,
        &mask_voxels,
        quantity,
        &unit,
        values,
        bundle.geometry.spacing_mm.iter().product(),
        sha256_hex_of_json(bundle)?,
    )
}

/// Same as `evaluate_endpoint` for a biological bundle.
#[pyfunction]
fn evaluate_endpoint_biological(
    model: &PyEndpointModel,
    bundle: &PyBiologicalDoseBundle,
    quantity: &str,
    mask_name: &str,
    mask_voxels: Vec<bool>,
) -> PyResult<PyEndpointEvaluation> {
    let inner = &bundle.inner;
    let (values, unit) = biological_selection(inner, quantity)?;
    run_endpoint_evaluation(
        model,
        &inner.case_id,
        mask_name,
        &mask_voxels,
        quantity,
        &unit,
        values,
        inner.geometry.spacing_mm.iter().product(),
        sha256_hex_of_json(inner)?,
    )
}

/// Combine a TCP and an NTCP evaluation into a UTCP report.
/// `combination` is `p_plus` (TCP·(1−NTCP)) or `difference` (TCP−NTCP).
#[pyfunction]
fn combine_utcp(
    tcp: &PyEndpointEvaluation,
    ntcp: &PyEndpointEvaluation,
    combination: &str,
) -> PyResult<PyEndpointEvaluation> {
    let combination = match combination {
        "p_plus" => openbnct_bio::UtcpCombination::PPlus,
        "difference" => openbnct_bio::UtcpCombination::Difference,
        other => {
            return Err(reject(format!(
                "unknown UTCP combination {other:?}; use p_plus or difference"
            )));
        }
    };
    let evaluation = openbnct_bio::combine_utcp(
        &tcp.inner,
        &tcp.bytes,
        &ntcp.inner,
        &ntcp.bytes,
        combination,
    )
    .map_err(reject)?;
    let bytes = serde_json::to_vec_pretty(&evaluation).map_err(reject)?;
    Ok(PyEndpointEvaluation {
        inner: evaluation,
        bytes,
    })
}

/// A `openbnct.bio-sensitivity-sweep/0.1.0` record.
#[pyclass(name = "SensitivitySweep")]
struct PySensitivitySweep {
    inner: openbnct_bio::SensitivitySweep,
}

#[pymethods]
impl PySensitivitySweep {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    #[getter]
    fn region(&self) -> &str {
        &self.inner.region
    }

    /// Canonical parameter label (`component:boron`, `alpha_beta:tumor`, ...).
    #[getter]
    fn parameter(&self) -> &str {
        &self.inner.parameter
    }

    /// The total quantity scored (`biological_total` or `weighted_eqd2`).
    #[getter]
    fn quantity(&self) -> &str {
        &self.inner.quantity
    }

    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    #[getter]
    fn model_sha256(&self) -> &str {
        &self.inner.model.sha256
    }

    #[getter]
    fn dose_bundle_sha256(&self) -> &str {
        &self.inner.dose_bundle.sha256
    }

    /// Sweep points as `(value, region_voxel_count, min, mean, max)` tuples.
    #[getter]
    fn points(&self) -> Vec<(f64, u64, f64, f64, f64)> {
        self.inner
            .points
            .iter()
            .map(|point| {
                (
                    point.value,
                    point.region_voxel_count,
                    point.minimum,
                    point.mean,
                    point.maximum,
                )
            })
            .collect()
    }

    #[getter]
    fn qualification(&self) -> &str {
        &self.inner.qualification
    }

    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }

    /// Write the sweep JSON; refuses to overwrite an existing file.
    fn write(&self, output: PathBuf) -> PyResult<()> {
        write_json_new(&output, &self.inner)
    }
}

/// Sweep one biological-model parameter over an explicit value list,
/// recording the region-masked min/mean/max of the biological total at
/// each point. `parameter` is `component:<name>`,
/// `region_weight:<region>:<component>`, `alpha_beta:default`,
/// `alpha_beta:<region>`, `fraction_count`, or
/// `source_particles_per_fraction`.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn sweep_biological_model(
    model: &PyBiologicalModel,
    physical: &PyPhysicalDoseBundle,
    region_masks: Vec<(String, PathBuf)>,
    region: &str,
    parameter: &str,
    values: Vec<f64>,
) -> PyResult<PySensitivitySweep> {
    let masks = load_named_masks(region_masks)?;
    let parameter = openbnct_bio::SweepParameter::parse(parameter).map_err(reject)?;
    let sweep = openbnct_bio::run_sweep(
        &model.inner,
        &model.bytes,
        &physical.inner,
        &sha256_hex_of_json(&physical.inner)?,
        &masks,
        region,
        &parameter,
        &values,
    )
    .map_err(reject)?;
    Ok(PySensitivitySweep { inner: sweep })
}

/// Re-hash every artifact declared by a bundle's manifest; returns the
/// verified manifest's case id and artifact count.
#[pyfunction]
fn verify_evidence_bundle(root: PathBuf) -> PyResult<(String, usize)> {
    let manifest = EvidenceBundleManifest::load_verified(&root).map_err(reject)?;
    Ok((manifest.case_id.clone(), manifest.artifacts.len()))
}

contract_wrapper!(
    "MultigroupData",
    PyMultigroupData,
    MultigroupData,
    "A validated multigroup cross-section artifact for the deterministic S_N solver."
);
contract_wrapper!(
    "MultigroupCovariance",
    PyMultigroupCovariance,
    MultigroupCovariance,
    "A validated declared-uncertainty artifact over multigroup parameters."
);
contract_wrapper!(
    "DoseUncertaintyBudget",
    PyDoseUncertaintyBudget,
    DoseUncertaintyBudget,
    "A nuclear-data propagated dose-uncertainty budget artifact."
);
contract_wrapper!(
    "SensitivitySpec",
    PySensitivitySpec,
    SensitivitySpec,
    "A validated declared-input sensitivity-screening specification."
);
contract_wrapper!(
    "SensitivityScreening",
    PySensitivityScreening,
    SensitivityScreening,
    "A Morris/Sobol screening report artifact."
);
contract_wrapper!(
    "ResolvedWeightWindows",
    PyResolvedWeightWindows,
    ResolvedWeightWindows,
    "A validated mesh weight-window artifact (CADIS, FW-CADIS, or manual)."
);
contract_wrapper!(
    "MeasurementRecord",
    PyMeasurementRecord,
    MeasurementRecord,
    "A validated published-measurement record with provenance."
);
contract_wrapper!(
    "MeasurementComparisonReport",
    PyMeasurementComparisonReport,
    MeasurementComparisonReport,
    "A computed-versus-measured comparison report artifact."
);
contract_wrapper!(
    "BeamDescription",
    PyBeamDescription,
    BeamDescription,
    "A validated beam-description contract (spectrum, angular, port)."
);
contract_wrapper!(
    "BeamQualityReport",
    PyBeamQualityReport,
    BeamQualityReport,
    "A beam-quality metrics report artifact."
);
contract_wrapper!(
    "AcceleratorSource",
    PyAcceleratorSource,
    AcceleratorSource,
    "A validated accelerator neutron-source contract."
);
contract_wrapper!(
    "BeamShapingAssembly",
    PyBeamShapingAssembly,
    BeamShapingAssembly,
    "A validated beam-shaping-assembly contract."
);
contract_wrapper!(
    "BsaSweepRecord",
    PyBsaSweepRecord,
    BsaSweepRecord,
    "A BSA parameter-sweep result record."
);
contract_wrapper!(
    "LinealSpectrum",
    PyLinealSpectrum,
    LinealSpectrum,
    "A validated lineal-energy spectrum artifact (MKM input)."
);
contract_wrapper!(
    "LinealTallySpec",
    PyLinealTallySpec,
    LinealTallySpec,
    "A validated lineal-tally specification for transport-derived spectra."
);
contract_wrapper!(
    "MetamorphicEvaluation",
    PyMetamorphicEvaluation,
    MetamorphicEvaluation,
    "A metamorphic-relation oracle evaluation report."
);
contract_wrapper!(
    "AnalyticOracle",
    PyAnalyticOracle,
    AnalyticOracle,
    "A validated analytic-transport oracle specification."
);
contract_wrapper!(
    "AnalyticOracleEvaluation",
    PyAnalyticOracleEvaluation,
    AnalyticOracleEvaluation,
    "An analytic-oracle evaluation report artifact."
);
contract_wrapper!(
    "BoronMicrodistribution",
    PyBoronMicrodistribution,
    BoronMicrodistribution,
    "A validated subcellular 10B microdistribution model."
);
contract_wrapper!(
    "MicrodistributionCorrection",
    PyMicrodistributionCorrection,
    MicrodistributionCorrection,
    "An evaluated microdistribution correction record."
);

/// A multigroup scalar-flux artifact produced by `sn solve`.
#[pyclass(frozen, name = "MultigroupFlux")]
struct PyMultigroupFlux {
    inner: MultigroupFlux,
}

#[pymethods]
impl PyMultigroupFlux {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// `uncollided_split` or `boundary_flux`.
    #[getter]
    fn beam_model(&self) -> &str {
        &self.inner.beam_model
    }

    #[getter]
    fn quadrature_order(&self) -> u32 {
        self.inner.quadrature_order
    }

    #[getter]
    fn outer_iterations(&self) -> u32 {
        self.inner.outer_iterations
    }

    /// Final relative scalar-flux change.
    #[getter]
    fn residual(&self) -> f64 {
        self.inner.residual
    }

    #[getter]
    fn converged(&self) -> bool {
        self.inner.converged
    }

    /// Number of energy groups.
    #[getter]
    fn group_count(&self) -> usize {
        self.inner.energy_boundaries_ev.len().saturating_sub(1)
    }

    /// Number of spatial voxels in the `[voxel][group]` flux layout.
    #[getter]
    fn voxel_count(&self) -> usize {
        self.inner.flux.len()
    }

    /// Flattened `[voxel][group]` scalar flux, cm⁻²s⁻¹ per unit source rate.
    fn flux(&self) -> Vec<Vec<f64>> {
        self.inner.flux.clone()
    }

    /// Canonical JSON bytes as produced by the Rust contract, as text.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// `(beam_number, name, gantry_angle_deg, metersets)` per beam.
type RtPlanBeamRow = (i32, Option<String>, Option<f64>, Vec<f64>);

/// An RTPLAN summary record produced by `dicom rtplan-info` or written by
/// `dicom export-rtplan` and read back.
#[pyclass(frozen, name = "RtPlanSummary")]
struct PyRtPlanSummary {
    inner: RtPlanSummary,
}

#[pymethods]
impl PyRtPlanSummary {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn sop_instance_uid(&self) -> &str {
        &self.inner.sop_instance_uid
    }

    #[getter]
    fn rt_plan_label(&self) -> &str {
        &self.inner.rt_plan_label
    }

    #[getter]
    fn rt_plan_name(&self) -> Option<String> {
        self.inner.rt_plan_name.clone()
    }

    #[getter]
    fn beam_count(&self) -> usize {
        self.inner.beams.len()
    }

    /// `(beam_number, name, gantry_angle_deg, metersets)` per beam.
    /// `gantry_angle_deg` comes from the first control point; `metersets`
    /// follows fraction-group order.
    fn beams(&self) -> Vec<RtPlanBeamRow> {
        self.inner
            .beams
            .iter()
            .map(|beam| {
                (
                    beam.beam_number,
                    beam.beam_name.clone(),
                    beam.control_point
                        .as_ref()
                        .and_then(|point| point.gantry_angle_deg),
                    beam.metersets.clone(),
                )
            })
            .collect()
    }

    /// Canonical JSON bytes as produced by the Rust contract, as text.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// `(component, file, sha256, sigma_file, sigma_sha256)` per exported NIfTI.
type ComponentNiftiFileRow = (String, String, String, Option<String>, Option<String>);

/// Manifest binding an exported per-component NIfTI set to its dose bundle.
#[pyclass(frozen, name = "ComponentNiftiManifest")]
struct PyComponentNiftiManifest {
    inner: ComponentNiftiManifest,
}

#[pymethods]
impl PyComponentNiftiManifest {
    #[getter]
    fn schema_version(&self) -> &str {
        &self.inner.schema_version
    }

    #[getter]
    fn case_id(&self) -> &str {
        &self.inner.case_id
    }

    /// Serialized `DoseUnit` token shared by every component volume.
    #[getter]
    fn unit(&self) -> &str {
        &self.inner.unit
    }

    /// `(component, file, sha256, sigma_file, sigma_sha256)` per exported
    /// NIfTI volume.
    fn files(&self) -> Vec<ComponentNiftiFileRow> {
        self.inner
            .files
            .iter()
            .map(|file| {
                (
                    file.component.clone(),
                    file.file.clone(),
                    file.sha256.clone(),
                    file.sigma_file.clone(),
                    file.sigma_sha256.clone(),
                )
            })
            .collect()
    }

    /// Canonical JSON bytes as produced by the Rust contract, as text.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string_pretty(&self.inner).map_err(reject)
    }
}

/// Read and validate a multigroup-data artifact.
#[pyfunction]
fn load_multigroup_data(path: PathBuf) -> PyResult<PyMultigroupData> {
    Ok(PyMultigroupData {
        inner: load_contract(path)?,
    })
}

/// Read a multigroup scalar-flux artifact produced by `sn solve`.
#[pyfunction]
fn load_multigroup_flux(path: PathBuf) -> PyResult<PyMultigroupFlux> {
    Ok(PyMultigroupFlux {
        inner: load_contract(path)?,
    })
}

/// Read and validate a multigroup-covariance artifact.
#[pyfunction]
fn load_multigroup_covariance(path: PathBuf) -> PyResult<PyMultigroupCovariance> {
    Ok(PyMultigroupCovariance {
        inner: load_contract(path)?,
    })
}

/// Read and validate a dose-uncertainty-budget artifact.
#[pyfunction]
fn load_dose_uncertainty_budget(path: PathBuf) -> PyResult<PyDoseUncertaintyBudget> {
    Ok(PyDoseUncertaintyBudget {
        inner: load_contract(path)?,
    })
}

/// Read and validate a sensitivity-screening specification.
#[pyfunction]
fn load_sensitivity_spec(path: PathBuf) -> PyResult<PySensitivitySpec> {
    Ok(PySensitivitySpec {
        inner: load_contract(path)?,
    })
}

/// Read and validate a sensitivity-screening report.
#[pyfunction]
fn load_sensitivity_screening(path: PathBuf) -> PyResult<PySensitivityScreening> {
    Ok(PySensitivityScreening {
        inner: load_contract(path)?,
    })
}

/// Read and validate a resolved weight-window artifact.
#[pyfunction]
fn load_weight_windows(path: PathBuf) -> PyResult<PyResolvedWeightWindows> {
    Ok(PyResolvedWeightWindows {
        inner: load_contract(path)?,
    })
}

/// Read and validate a published-measurement record.
#[pyfunction]
fn load_measurement_record(path: PathBuf) -> PyResult<PyMeasurementRecord> {
    Ok(PyMeasurementRecord {
        inner: load_contract(path)?,
    })
}

/// Read and validate a computed-versus-measured comparison report.
#[pyfunction]
fn load_measurement_comparison(path: PathBuf) -> PyResult<PyMeasurementComparisonReport> {
    Ok(PyMeasurementComparisonReport {
        inner: load_contract(path)?,
    })
}

/// Read and validate a beam-description contract.
#[pyfunction]
fn load_beam_description(path: PathBuf) -> PyResult<PyBeamDescription> {
    Ok(PyBeamDescription {
        inner: load_contract(path)?,
    })
}

/// Read and validate a beam-quality report.
#[pyfunction]
fn load_beam_quality_report(path: PathBuf) -> PyResult<PyBeamQualityReport> {
    Ok(PyBeamQualityReport {
        inner: load_contract(path)?,
    })
}

/// Read and validate an accelerator-source contract.
#[pyfunction]
fn load_accelerator_source(path: PathBuf) -> PyResult<PyAcceleratorSource> {
    Ok(PyAcceleratorSource {
        inner: load_contract(path)?,
    })
}

/// Read and validate a beam-shaping-assembly contract.
#[pyfunction]
fn load_beam_shaping_assembly(path: PathBuf) -> PyResult<PyBeamShapingAssembly> {
    Ok(PyBeamShapingAssembly {
        inner: load_contract(path)?,
    })
}

/// Read and validate a BSA sweep record.
#[pyfunction]
fn load_bsa_sweep(path: PathBuf) -> PyResult<PyBsaSweepRecord> {
    Ok(PyBsaSweepRecord {
        inner: load_contract(path)?,
    })
}

/// Read and validate a lineal-energy spectrum artifact.
#[pyfunction]
fn load_lineal_spectrum(path: PathBuf) -> PyResult<PyLinealSpectrum> {
    Ok(PyLinealSpectrum {
        inner: load_contract(path)?,
    })
}

/// Read and validate a lineal-tally specification.
#[pyfunction]
fn load_lineal_tally_spec(path: PathBuf) -> PyResult<PyLinealTallySpec> {
    Ok(PyLinealTallySpec {
        inner: load_contract(path)?,
    })
}

/// Read and validate a metamorphic-evaluation report.
#[pyfunction]
fn load_metamorphic_evaluation(path: PathBuf) -> PyResult<PyMetamorphicEvaluation> {
    Ok(PyMetamorphicEvaluation {
        inner: load_contract(path)?,
    })
}

/// Read and validate an analytic-oracle specification.
#[pyfunction]
fn load_analytic_oracle(path: PathBuf) -> PyResult<PyAnalyticOracle> {
    Ok(PyAnalyticOracle {
        inner: load_contract(path)?,
    })
}

/// Read an analytic-oracle evaluation report.
#[pyfunction]
fn load_analytic_oracle_evaluation(path: PathBuf) -> PyResult<PyAnalyticOracleEvaluation> {
    Ok(PyAnalyticOracleEvaluation {
        inner: load_contract(path)?,
    })
}

/// Read and validate a gamma-index evaluation report.
#[pyfunction]
fn load_gamma_evaluation(path: PathBuf) -> PyResult<PyGammaEvaluation> {
    Ok(PyGammaEvaluation {
        inner: load_contract(path)?,
    })
}

/// Read and validate a boron-microdistribution model.
#[pyfunction]
fn load_boron_microdistribution(path: PathBuf) -> PyResult<PyBoronMicrodistribution> {
    Ok(PyBoronMicrodistribution {
        inner: load_contract(path)?,
    })
}

/// Read and validate a microdistribution-correction record.
#[pyfunction]
fn load_microdistribution_correction(path: PathBuf) -> PyResult<PyMicrodistributionCorrection> {
    Ok(PyMicrodistributionCorrection {
        inner: load_contract(path)?,
    })
}

/// Read an RTPLAN summary record.
#[pyfunction]
fn load_rtplan_summary(path: PathBuf) -> PyResult<PyRtPlanSummary> {
    Ok(PyRtPlanSummary {
        inner: load_contract(path)?,
    })
}

/// Summarize a DICOM RTPLAN file — the `dicom rtplan-info` surface.
#[pyfunction]
fn summarize_rtplan(path: PathBuf) -> PyResult<PyRtPlanSummary> {
    Ok(PyRtPlanSummary {
        inner: openbnct_dicom::summarize_rt_plan(&path).map_err(reject)?,
    })
}

/// Read a per-component NIfTI export manifest.
#[pyfunction]
fn load_component_nifti_manifest(path: PathBuf) -> PyResult<PyComponentNiftiManifest> {
    Ok(PyComponentNiftiManifest {
        inner: load_contract(path)?,
    })
}

/// NCTForge Python boundary over the authoritative Rust implementation.
///
/// Research software only: not a medical device, not commissioned, and not a
/// dose calculator. Transport actions stay unavailable until the same Rust
/// capability and evidence gates used by the CLI and GUI pass.
/// Export a case + material assignment + avify spec to the engine's
/// voxel plan (`<prefix>_arrays.npz`, `<prefix>_meta.json`,
/// `<prefix>.plan.json`). Returns a JSON object with the artifact
/// paths, SHA-256 bindings, and per-class voxel counts. The declared
/// uptake set passes through verbatim — corner maps stay in the
/// separately licensed engine.
#[pyfunction]
#[pyo3(signature = (case, assignment, spec, prefix))]
fn avify_export_plan(
    case: PathBuf,
    assignment: PathBuf,
    spec: PathBuf,
    prefix: PathBuf,
) -> PyResult<String> {
    let (export, plan_path) =
        openbnct_avify::export_pipeline(&case, &assignment, &spec, &prefix).map_err(reject)?;
    serde_json::to_string_pretty(&serde_json::json!({
        "arrays_path": export.arrays_path,
        "meta_path": export.meta_path,
        "plan_path": plan_path,
        "arrays_sha256": export.arrays_sha256,
        "meta_sha256": export.meta_sha256,
        "class_voxels": export.class_voxels,
    }))
    .map_err(reject)
}

/// Export, then run the separately licensed Avify Dose engine as a
/// bounded child process (timeout kills and reaps it). Writes
/// `certificate.json` and the `avify-run.json` receipt under `outdir`;
/// returns a JSON object with the run record and the parsed
/// certificate — an empirical two-evaluation envelope, research only.
#[pyfunction]
#[pyo3(signature = (case, assignment, spec, outdir, engine_cmd="avify-dose", threads=None, timeout_s=21600))]
#[allow(clippy::too_many_arguments)]
fn avify_verify(
    case: PathBuf,
    assignment: PathBuf,
    spec: PathBuf,
    outdir: PathBuf,
    engine_cmd: &str,
    threads: Option<u32>,
    timeout_s: u64,
) -> PyResult<String> {
    let output = openbnct_avify::verify_pipeline(
        &case,
        &assignment,
        &spec,
        &outdir,
        engine_cmd,
        timeout_s,
        threads,
    )
    .map_err(reject)?;
    serde_json::to_string_pretty(&serde_json::json!({
        "certificate_path": output.outcome.certificate_path,
        "receipt_path": output.receipt_path,
        "elapsed_s": output.outcome.elapsed.as_secs_f64(),
        "engine_version": output.receipt.engine.version,
        "timing": {
            "export_s": output.receipt.timing.export_s,
            "engine_s": output.receipt.timing.engine_s,
            "bind_s": output.receipt.timing.bind_s,
            "total_s": output.receipt.timing.total_s,
        },
        "resources": {
            "available_parallelism": output.receipt.resources.available_parallelism,
            "threads": output.receipt.engine.threads,
            "timeout_s": output.receipt.engine.timeout_s,
        },
        // The certificate is a passthrough document (Deserialize-only
        // in the connector) — embed the engine's bytes verbatim.
        "certificate": serde_json::from_slice::<serde_json::Value>(
            &fs::read(&output.outcome.certificate_path).map_err(reject)?,
        )
        .map_err(reject)?,
    }))
    .map_err(reject)
}

/// Check a `avify-run.json` receipt against the filesystem — JSON
/// object with per-input CURRENT/CHANGED/MISSING states and a `stale`
/// verdict.
#[pyfunction]
fn avify_status(receipt: PathBuf) -> PyResult<String> {
    let receipt = openbnct_avify::AvifyRunReceipt::load(&receipt).map_err(reject)?;
    let base = receipt
        .certificate
        .path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let states = openbnct_avify::check_staleness(&receipt, &base);
    let mut per_input = serde_json::Map::new();
    for (name, state) in &states {
        let value = match state {
            openbnct_avify::InputState::Current => serde_json::json!("current"),
            openbnct_avify::InputState::Changed(d) => {
                serde_json::json!({"state": "changed", "current_sha256": d})
            }
            openbnct_avify::InputState::Missing => serde_json::json!("missing"),
        };
        per_input.insert(name.clone(), value);
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "engine_version": receipt.engine.version,
        "engine_argv0": receipt.engine.argv0,
        "certificate_sha256": receipt.certificate.sha256,
        "engine_elapsed_s": receipt.engine_elapsed_s,
        "timing": {
            "export_s": receipt.timing.export_s,
            "engine_s": receipt.timing.engine_s,
            "bind_s": receipt.timing.bind_s,
            "total_s": receipt.timing.total_s,
        },
        "resources": {
            "available_parallelism": receipt.resources.available_parallelism,
            "threads": receipt.engine.threads,
            "timeout_s": receipt.engine.timeout_s,
        },
        "stale": openbnct_avify::is_stale(&states),
        "cold_start": receipt.cold_start,
        "warnings": receipt.warnings,
        "inputs": per_input,
        "review": match openbnct_avify::review_state(&base).map_err(reject)? {
            openbnct_avify::ReviewState::Missing => serde_json::json!("missing"),
            openbnct_avify::ReviewState::Current(r) => serde_json::json!({
                "state": "reviewed",
                "reviewer": r.reviewer,
                "note": r.note,
                "created_unix_seconds": r.created_unix_seconds,
            }),
            openbnct_avify::ReviewState::Stale { review, .. } => serde_json::json!({
                "state": "stale",
                "reviewer": review.reviewer,
            }),
        },
    }))
    .map_err(reject)
}

/// Mark a run's certificate as reviewed — writes a hash-bound
/// `review.json` next to it. Returns the marker as a JSON string.
#[pyfunction]
#[pyo3(signature = (outdir, reviewer, note=""))]
fn avify_review(outdir: PathBuf, reviewer: &str, note: &str) -> PyResult<String> {
    let review =
        openbnct_avify::write_review(&outdir, reviewer, note).map_err(reject)?;
    serde_json::to_string_pretty(&review).map_err(reject)
}

/// Compare two run directories (or `avify-run.json` paths) — per-ROI
/// certified-interval/action changes plus which bound inputs differ,
/// as a JSON object. Nothing is recomputed; the diff is verbatim from
/// the two receipts/certificates.
#[pyfunction]
fn avify_diff(before: PathBuf, after: PathBuf) -> PyResult<String> {
    let d = openbnct_avify::diff_runs(&before, &after).map_err(reject)?;
    serde_json::to_string_pretty(&d).map_err(reject)
}

/// Load the engine's `certificate.json` — returned verbatim as JSON;
/// the surface never re-derives engine results.
#[pyfunction]
fn avify_load_certificate(certificate: PathBuf) -> PyResult<String> {
    let bytes = fs::read(&certificate).map_err(reject)?;
    let _: openbnct_avify::AvifyCertificate = serde_json::from_slice(&bytes).map_err(reject)?;
    String::from_utf8(bytes).map_err(reject)
}

#[pymodule]
fn _openbnct(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("NctForgeError", m.py().get_type::<NctForgeError>())?;
    m.add_class::<PyBackend>()?;
    m.add_class::<PyStructure>()?;
    m.add_class::<PyCaseVerification>()?;
    m.add_class::<PyVerifiedCase>()?;
    m.add_class::<PyGeometry>()?;
    m.add_class::<PyGeneratedCase>()?;
    m.add_class::<PyArtifact>()?;
    m.add_class::<PyCaseManifest>()?;
    m.add_class::<PyMaterial>()?;
    m.add_class::<PyFixedSource>()?;
    m.add_class::<PyComponentProfile>()?;
    m.add_class::<PyResponseGenerationMethod>()?;
    m.add_class::<PyResponseSet>()?;
    m.add_class::<PyDoseVolume>()?;
    m.add_class::<PyPhysicalDoseBundle>()?;
    m.add_class::<PyBiologicalModel>()?;
    m.add_class::<PyBiologicalDoseBundle>()?;
    m.add_class::<PyBioModelComparison>()?;
    m.add_class::<PyAppliedFractionation>()?;
    m.add_class::<PyDoseVolumeHistogram>()?;
    m.add_class::<PyRegionDoseMetrics>()?;
    m.add_class::<PyEndpointModel>()?;
    m.add_class::<PyAppliedDoseStatistic>()?;
    m.add_class::<PyEndpointEvaluation>()?;
    m.add_class::<PySensitivitySweep>()?;
    m.add_class::<PyExposure>()?;
    m.add_class::<PyExposurePlan>()?;
    m.add_class::<PyExternalDoseBundle>()?;
    m.add_class::<PyBedBundle>()?;
    m.add_class::<PyCombinedDoseBundle>()?;
    m.add_class::<PyDoseComparison>()?;
    m.add_class::<PyGammaEvaluation>()?;
    m.add_class::<PyPositionReport>()?;
    m.add_class::<PyMultigroupData>()?;
    m.add_class::<PyMultigroupFlux>()?;
    m.add_class::<PyMultigroupCovariance>()?;
    m.add_class::<PyDoseUncertaintyBudget>()?;
    m.add_class::<PySensitivitySpec>()?;
    m.add_class::<PySensitivityScreening>()?;
    m.add_class::<PyResolvedWeightWindows>()?;
    m.add_class::<PyMeasurementRecord>()?;
    m.add_class::<PyMeasurementComparisonReport>()?;
    m.add_class::<PyBeamDescription>()?;
    m.add_class::<PyBeamQualityReport>()?;
    m.add_class::<PyAcceleratorSource>()?;
    m.add_class::<PyBeamShapingAssembly>()?;
    m.add_class::<PyBsaSweepRecord>()?;
    m.add_class::<PyLinealSpectrum>()?;
    m.add_class::<PyMicrodosimetricModel>()?;
    m.add_class::<PyIsoeffectiveModel>()?;
    m.add_class::<PyLinealTallySpec>()?;
    m.add_class::<PyMetamorphicEvaluation>()?;
    m.add_class::<PyAnalyticOracle>()?;
    m.add_class::<PyAnalyticOracleEvaluation>()?;
    m.add_class::<PyBoronMicrodistribution>()?;
    m.add_class::<PyMicrodistributionCorrection>()?;
    m.add_class::<PyRtPlanSummary>()?;
    m.add_class::<PyComponentNiftiManifest>()?;
    m.add_function(wrap_pyfunction!(backends, m)?)?;
    m.add_function(wrap_pyfunction!(file_sha256, m)?)?;
    m.add_function(wrap_pyfunction!(generate_case, m)?)?;
    m.add_function(wrap_pyfunction!(verify_case, m)?)?;
    m.add_function(wrap_pyfunction!(load_case, m)?)?;
    m.add_function(wrap_pyfunction!(read_manifest, m)?)?;
    m.add_function(wrap_pyfunction!(load_material, m)?)?;
    m.add_function(wrap_pyfunction!(load_fixed_source, m)?)?;
    m.add_function(wrap_pyfunction!(aim_source, m)?)?;
    m.add_function(wrap_pyfunction!(rotate_source, m)?)?;
    m.add_function(wrap_pyfunction!(load_component_profile, m)?)?;
    m.add_function(wrap_pyfunction!(load_response_generation_method, m)?)?;
    m.add_function(wrap_pyfunction!(load_response_set, m)?)?;
    m.add_function(wrap_pyfunction!(load_physical_dose_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(collect_run, m)?)?;
    m.add_function(wrap_pyfunction!(load_biological_model, m)?)?;
    m.add_function(wrap_pyfunction!(load_bio_model_comparison, m)?)?;
    m.add_function(wrap_pyfunction!(make_biological_model, m)?)?;
    m.add_function(wrap_pyfunction!(apply_model, m)?)?;
    m.add_function(wrap_pyfunction!(compute_dvh, m)?)?;
    m.add_function(wrap_pyfunction!(compute_dvh_biological, m)?)?;
    m.add_function(wrap_pyfunction!(compute_metrics, m)?)?;
    m.add_function(wrap_pyfunction!(compute_metrics_biological, m)?)?;
    m.add_function(wrap_pyfunction!(load_endpoint_model, m)?)?;
    m.add_function(wrap_pyfunction!(load_endpoint_evaluation, m)?)?;
    m.add_function(wrap_pyfunction!(evaluate_endpoint, m)?)?;
    m.add_function(wrap_pyfunction!(evaluate_endpoint_biological, m)?)?;
    m.add_function(wrap_pyfunction!(combine_utcp, m)?)?;
    m.add_function(wrap_pyfunction!(verify_evidence_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(load_exposure_plan, m)?)?;
    m.add_function(wrap_pyfunction!(exposure_plan_diagnostics, m)?)?;
    m.add_function(wrap_pyfunction!(accumulate_exposures, m)?)?;
    m.add_function(wrap_pyfunction!(plan_table_read, m)?)?;
    m.add_function(wrap_pyfunction!(plan_table_write, m)?)?;
    m.add_function(wrap_pyfunction!(import_component_dose, m)?)?;
    m.add_function(wrap_pyfunction!(import_mcnp_meshtal, m)?)?;
    m.add_function(wrap_pyfunction!(import_nifti, m)?)?;
    m.add_function(wrap_pyfunction!(import_phits, m)?)?;
    m.add_function(wrap_pyfunction!(export_mcnp_deck, m)?)?;
    m.add_function(wrap_pyfunction!(import_external_dose, m)?)?;
    m.add_function(wrap_pyfunction!(load_external_dose_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(bed_from_external_dose, m)?)?;
    m.add_function(wrap_pyfunction!(load_bed_bundle, m)?)?;
    m.add_function(wrap_pyfunction!(combine_biological_doses, m)?)?;
    m.add_function(wrap_pyfunction!(compare_dose_bundles, m)?)?;
    m.add_function(wrap_pyfunction!(evaluate_gamma, m)?)?;
    m.add_function(wrap_pyfunction!(sweep_biological_model, m)?)?;
    m.add_function(wrap_pyfunction!(load_multigroup_data, m)?)?;
    m.add_function(wrap_pyfunction!(load_multigroup_flux, m)?)?;
    m.add_function(wrap_pyfunction!(load_multigroup_covariance, m)?)?;
    m.add_function(wrap_pyfunction!(load_dose_uncertainty_budget, m)?)?;
    m.add_function(wrap_pyfunction!(load_sensitivity_spec, m)?)?;
    m.add_function(wrap_pyfunction!(load_sensitivity_screening, m)?)?;
    m.add_function(wrap_pyfunction!(load_weight_windows, m)?)?;
    m.add_function(wrap_pyfunction!(load_measurement_record, m)?)?;
    m.add_function(wrap_pyfunction!(load_measurement_comparison, m)?)?;
    m.add_function(wrap_pyfunction!(load_beam_description, m)?)?;
    m.add_function(wrap_pyfunction!(load_beam_quality_report, m)?)?;
    m.add_function(wrap_pyfunction!(load_accelerator_source, m)?)?;
    m.add_function(wrap_pyfunction!(load_beam_shaping_assembly, m)?)?;
    m.add_function(wrap_pyfunction!(load_bsa_sweep, m)?)?;
    m.add_function(wrap_pyfunction!(load_lineal_spectrum, m)?)?;
    m.add_function(wrap_pyfunction!(load_microdosimetric_model, m)?)?;
    m.add_function(wrap_pyfunction!(load_isoeffective_model, m)?)?;
    m.add_function(wrap_pyfunction!(apply_mkm_model, m)?)?;
    m.add_function(wrap_pyfunction!(apply_isoeffective, m)?)?;
    m.add_function(wrap_pyfunction!(make_microdosimetric_model, m)?)?;
    m.add_function(wrap_pyfunction!(make_isoeffective_model, m)?)?;
    m.add_function(wrap_pyfunction!(load_lineal_tally_spec, m)?)?;
    m.add_function(wrap_pyfunction!(load_metamorphic_evaluation, m)?)?;
    m.add_function(wrap_pyfunction!(load_analytic_oracle, m)?)?;
    m.add_function(wrap_pyfunction!(load_analytic_oracle_evaluation, m)?)?;
    m.add_function(wrap_pyfunction!(load_gamma_evaluation, m)?)?;
    m.add_function(wrap_pyfunction!(load_boron_microdistribution, m)?)?;
    m.add_function(wrap_pyfunction!(load_microdistribution_correction, m)?)?;
    m.add_function(wrap_pyfunction!(load_rtplan_summary, m)?)?;
    m.add_function(wrap_pyfunction!(summarize_rtplan, m)?)?;
    m.add_function(wrap_pyfunction!(load_component_nifti_manifest, m)?)?;
    m.add_function(wrap_pyfunction!(avify_export_plan, m)?)?;
    m.add_function(wrap_pyfunction!(avify_verify, m)?)?;
    m.add_function(wrap_pyfunction!(avify_status, m)?)?;
    m.add_function(wrap_pyfunction!(avify_review, m)?)?;
    m.add_function(wrap_pyfunction!(avify_load_certificate, m)?)?;
    m.add_function(wrap_pyfunction!(avify_diff, m)?)?;
    Ok(())
}
