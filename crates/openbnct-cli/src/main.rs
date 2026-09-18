// SPDX-License-Identifier: Apache-2.0

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use openbnct_bio::{
    BedQuantity, BiologicalModel, LinealSpectrum, LinealWeighting, MicrodosimetricModel,
    RegionMask, SpectrumInput, SweepParameter, apply_biological_model, apply_microdosimetric_model,
    bed_from_external, combine_biological_doses,
};
use openbnct_core::{ExposurePlan, PhysicalDoseBundle, ResampleMethod};
use openbnct_dicom::synthetic::generate_nf_bnct_001;
use openbnct_dicom::{load_nf_bnct_001, verify_nf_bnct_001};
use openbnct_nifti::{
    Interpolation, NiftiImage, read_nifti_file, read_target_geometry, resample_to_grid, write_nifti,
};
use openbnct_njoy::{
    DEFAULT_CAPTURE_ENERGY_BALANCE_RELATIVE_TOLERANCE,
    DEFAULT_LAW7_BREAKUP_NORMALIZATION_TOLERANCE, DEFAULT_LAW7_BREAKUP_RELATIVE_ENERGY_TOLERANCE,
    DEFAULT_NJOY_CAPTURE_PRINT_RELATIVE_TOLERANCE,
    DEFAULT_NJOY_ENERGY_BALANCE_PRINT_RELATIVE_TOLERANCE,
    DEFAULT_NJOY_LAW7_PRINT_RELATIVE_TOLERANCE, DEFAULT_NJOY_LAW7_SOURCE_RELATIVE_TOLERANCE,
    DEFAULT_NJOY_PRINT_RELATIVE_TOLERANCE, DEFAULT_NJOY_TIMEOUT_SECONDS,
    DEFAULT_REACTION_BALANCE_RELATIVE_TOLERANCE, DEFAULT_SPECTRUM_NORMALIZATION_TOLERANCE,
    EndfContinuumPhotonMomentReport, EndfContinuumPhotonMomentReportDocument,
    EndfMf6CapturePhotonBalanceQualification, EndfMf6CapturePhotonBalanceReport,
    EndfMf6CapturePhotonBalanceReportDocument, EndfMf6Law7ImplicitResidualQualification,
    EndfMf6Law7ImplicitResidualReport, EndfMf6Law7ImplicitResidualReportDocument,
    EndfPhotonProductionInventory, EndfPhotonProductionInventoryDocument,
    EndfReactionBalanceQualification, EndfReactionEnergyBalanceDocument,
    EndfReactionEnergyBalanceReport, NjoyAcquisitionArtifacts, NjoyCandidateComparisonCheckResult,
    NjoyCapturePhotonMomentComparison, NjoyCapturePhotonMomentComparisonDocument,
    NjoyDiagnosticTriageCheckResult, NjoyDiagnosticTriageReport,
    NjoyDiagnosticTriageReportDocument, NjoyDomainAwareSuitabilityReport,
    NjoyDomainAwareSuitabilityReportDocument, NjoyEnergyBalanceAttribution,
    NjoyEnergyBalanceAttributionDocument, NjoyEnergyBalanceAttributionQualification,
    NjoyEvidenceAwareCheckResult, NjoyEvidenceAwareSuitabilityReport,
    NjoyEvidenceAwareSuitabilityReportDocument, NjoyExecutionOptions, NjoyExecutionReceipt,
    NjoyExecutionReceiptDocument, NjoyInputArtifacts, NjoyInputBundle,
    NjoyLaw7ImplicitResidualComparison, NjoyLaw7ImplicitResidualComparisonDocument,
    NjoyLaw7ImplicitResidualComparisonQualification, NjoyPhotonMomentComparison,
    NjoyPhotonMomentComparisonDocument, NjoyResponseSetReviewDocument, NjoyResponseSetReviewReport,
    NjoyResponseTableGeneration, NjoyResponseTableInputs, NjoySourceAwareSuitabilityReport,
    NjoySourceAwareSuitabilityReportDocument, NjoySuitabilityComparison,
    NjoySuitabilityComparisonDocument, NjoySuitabilityComparisonQualification,
    NjoySuitabilityQualification, NjoySuitabilityReport, NjoySuitabilityReportDocument,
    load_generation_report, load_response_set,
};
use openbnct_openmc::{
    DataAcquisitionClient, DataAcquisitionProfileDocument, DataAcquisitionReceiptDocument,
    EvaluatedNeutronSourceSelectionDocument, EvaluatedSourceQualification, NuclearDataManifest,
    OpenMcBackend, OpenMcInputArtifacts, OpenMcInputDeck, OpenMcNeutronTransportDomain,
    OpenMcNeutronTransportDomainDocument, evaluate_runs,
};
use openbnct_transport::{
    CompletedRun, ComponentDefinitionProfile, MATERIAL_ASSIGNMENT_SCHEMA, MaterialAssignment,
    MaterialDefinition, MaterialRegion, ResponseGenerationMethod, TransportBackend, TransportCase,
};

#[derive(Debug, Parser)]
#[command(
    name = "openbnct",
    version,
    about = "Transport-neutral BNCT research and verification workbench"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show the current transport-adapter boundary.
    Backends,
    /// Generate or verify frozen public benchmark cases.
    Benchmark(BenchmarkArgs),
    /// Inspect and bind versioned facility beam descriptions
    /// (`openbnct.beam-description/0.1.0`).
    Beam(BeamArgs),
    /// Evaluate parametric accelerator-target neutron sources
    /// (`openbnct.accelerator-source/0.1.0`).
    Accelerator(AcceleratorArgs),
    /// Rasterize and sweep beam-shaping assemblies
    /// (`openbnct.beam-shaping-assembly/0.1.0`).
    Bsa(BsaArgs),
    /// Inspect measurement records and compare them against computed
    /// artifacts (`openbnct.measurement-record/0.1.0`).
    Measurement(MeasurementArgs),
    /// Export dose bundles as DICOM RT Dose objects.
    Dicom(DicomArgs),
    /// Prepare and audit OpenMC-specific research artifacts.
    Openmc(OpenMcArgs),
    /// Prepare deterministic NJOY response-generation artifacts.
    Njoy(NjoyArgs),
    /// Apply a separately versioned biological model to a physical dose bundle.
    Bio(BioArgs),
    /// Compute a dose-volume histogram over a named voxel mask.
    Dvh {
        /// Physical or biological dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `component:boron|nitrogen|hydrogen|photon`, `physical_total`, or
        /// `biological_total` (the last only for biological bundles).
        #[arg(long)]
        quantity: String,
        /// RegionMask JSON (`name` + per-voxel `voxels` booleans).
        #[arg(long)]
        mask: PathBuf,
        /// Number of equal-width dose bins over [0, max].
        #[arg(long, default_value_t = 100)]
        bins: usize,
        /// New output path for the DVH JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Import, inspect, and export NIfTI-1 volumes on the patient grid.
    Nifti(NiftiArgs),
    /// Accumulate weighted exposures (fields/fractions) into one physical
    /// dose bundle under a declared exposure plan.
    Accumulate {
        /// `openbnct.exposure-plan/0.1.0` JSON; bundle paths resolve
        /// relative to this file's directory.
        #[arg(long)]
        plan: PathBuf,
        /// New output path for the accumulated physical dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Export or verify a deterministic evidence bundle.
    Evidence(EvidenceArgs),
    /// Combine or construct RegionMask volumes (subtraction, union,
    /// intersection, CT-threshold regions) for limiting-organ construction.
    Mask(MaskArgs),
    /// Import, validate, and export structured exposure plans
    /// (CSV/XLSX table interchange for `openbnct.exposure-plan/0.1.0`).
    Plan(PlanArgs),
    /// Aim a fixed source at a region centroid or rotate a source about a
    /// patient axis (research positioning helpers).
    Position(PositionArgs),
    /// Import external transport results into a validated physical dose
    /// bundle (interchange document, MCNP meshtal, or PHITS tally output).
    Import(ImportArgs),
    /// Export transport input to an external code (MCNP deck).
    Export(ExportArgs),
    /// Compare two physical dose bundles on the same frozen case
    /// (cross-code agreement record; no equivalence claim).
    Compare {
        /// Reference physical dose bundle JSON.
        #[arg(long)]
        reference: PathBuf,
        /// Candidate physical dose bundle JSON.
        #[arg(long)]
        candidate: PathBuf,
        /// Combined-uncertainty multiplier for the within-sigma fraction.
        #[arg(long, default_value_t = 2.0)]
        sigma_level: f64,
        /// New output path for the dose-comparison JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Evaluate the Low gamma index between two physical dose bundles on
    /// the same frozen case (agreement record; no equivalence claim).
    Gamma {
        /// Reference physical dose bundle JSON.
        #[arg(long)]
        reference: PathBuf,
        /// Candidate physical dose bundle JSON.
        #[arg(long)]
        candidate: PathBuf,
        /// Dose-difference criterion in percent.
        #[arg(long, default_value_t = 3.0)]
        dose_difference_percent: f64,
        /// Distance-to-agreement criterion in millimetres.
        #[arg(long, default_value_t = 3.0)]
        distance_to_agreement_mm: f64,
        /// Dose criterion normalization: `global` (percent of the
        /// reference maximum) or `local` (percent of the evaluated
        /// reference voxel).
        #[arg(long, default_value = "global")]
        normalization: String,
        /// Exclude reference voxels below this percent of the reference
        /// maximum (the standard low-dose cutoff).
        #[arg(long)]
        dose_threshold_percent: Option<f64>,
        /// Emit the per-voxel gamma field inside the record.
        #[arg(long)]
        gamma_volume: bool,
        /// New output path for the gamma-evaluation JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Evaluate a metamorphic transport oracle — a symmetry relation a
    /// correct run must satisfy — and emit an
    /// `openbnct.metamorphic-evaluation/0.1.0` record.
    Metamorphic {
        /// Oracle kind: `reflection` (bundle vs its own axis mirror),
        /// `rotation` (reference vs a source-rotated run),
        /// `superposition` (combined run vs the sum of two runs), or
        /// `reciprocity` (source↔detector voxel-pair interchange).
        #[arg(long)]
        oracle: String,
        /// Axis for `reflection`/`rotation`: `x`, `y`, or `z`.
        #[arg(long)]
        axis: Option<String>,
        /// Quarter-turns for `rotation` (1–3).
        #[arg(long)]
        turns: Option<u8>,
        /// Required justification of the symmetry premise for
        /// `reflection` — why this problem is symmetric about the axis.
        #[arg(long)]
        declared_symmetry: Option<String>,
        /// Reciprocity voxel index in the candidate run.
        #[arg(long)]
        voxel_a: Option<u64>,
        /// Reciprocity voxel index in the reference run.
        #[arg(long)]
        voxel_b: Option<u64>,
        /// Reference physical dose bundle JSON.
        #[arg(long)]
        reference: PathBuf,
        /// Candidate bundle(s): one for `rotation`/`reciprocity`, two
        /// for `superposition`, none for `reflection`; repeatable.
        #[arg(long)]
        candidate: Vec<PathBuf>,
        /// Record id.
        #[arg(long)]
        id: String,
        /// Combined-uncertainty multiplier for the within-sigma fraction.
        #[arg(long, default_value_t = 2.0)]
        sigma_level: f64,
        /// New output path for the metamorphic-evaluation JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Evaluate a declared analytic oracle — a closed-form expectation
    /// such as exponential attenuation — against a dose bundle and emit
    /// an `openbnct.analytic-oracle-evaluation/0.1.0` record.
    Analytic {
        /// `openbnct.analytic-oracle/0.1.0` declaration JSON.
        #[arg(long)]
        oracle: PathBuf,
        /// Physical dose bundle JSON to evaluate.
        #[arg(long)]
        dose: PathBuf,
        /// Record id.
        #[arg(long)]
        id: String,
        /// New output path for the evaluation JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Deterministic multigroup S_N transport — the in-house reference
    /// solver (research-only).
    Sn(SnArgs),
    /// Compute exact dose-volume metrics (D_x, V_x, min/mean/max, EUD)
    /// over a named voxel mask.
    Metrics {
        /// Physical or biological dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `component:NAME`, `physical_total`, or `biological_total`.
        #[arg(long)]
        quantity: String,
        /// RegionMask JSON (`name` + per-voxel `voxels` booleans).
        #[arg(long)]
        mask: PathBuf,
        /// Coverage percent(s) in (0, 100] for `D_x` readings;
        /// repeatable or comma-separated.
        #[arg(long = "dx", value_delimiter = ',')]
        dx: Vec<f64>,
        /// Dose level(s) in the dose unit for `V_x` readings.
        #[arg(long = "vx", value_delimiter = ',')]
        vx: Vec<f64>,
        /// Niemierko organ parameter(s) for EUD readings (a=1 mean,
        /// a>0 serial, a<0 parallel, a=0 geometric mean).
        #[arg(long = "eud-a", value_delimiter = ',', allow_hyphen_values = true)]
        eud: Vec<f64>,
        /// New output path for the dose-metrics JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Score a TCP/NTCP endpoint model over a dose volume, or combine a
    /// TCP and NTCP evaluation into a UTCP report.
    Endpoint(EndpointArgs),
    /// Rigid image co-registration: landmark fitting, declared
    /// transforms, and transform application to NIfTI volumes
    /// (`openbnct.registration/0.1.0`).
    Register(RegisterArgs),
    /// PET-derived boron: map a co-registered SUV volume to a per-voxel
    /// B-10 field with stated uncertainty, or realize a field as a
    /// material assignment (`openbnct.boron-uptake-model/0.1.0`,
    /// `openbnct.boron-field/0.1.0`).
    Boron(BoronArgs),
    /// Propagate declared systematic uncertainties (boron concentration,
    /// positioning, component-relative) into per-voxel and region-mean
    /// dose uncertainty (`openbnct.systematic-uncertainty/0.1.0`).
    Uq(UqArgs),
    /// Variance reduction: resolve a `openbnct.variance-reduction/0.1.0`
    /// spec into a `openbnct.weight-windows/0.1.0` artifact and validate a
    /// variance-reduced run against an analog reference.
    Vr(VrArgs),
    /// Evaluate organ-limited irradiation time over a per-source-particle
    /// dose endpoint, reporting the limiting structure and assumptions.
    IrradiationTime {
        /// Physical or biological dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `component:NAME`, `physical_total`, or `biological_total`.
        #[arg(long)]
        quantity: String,
        /// Source strength in source particles per second.
        #[arg(long)]
        source_strength: f64,
        /// Region limit `NAME=max|mean:LIMIT` in endpoint dose units;
        /// repeatable.
        #[arg(long = "limit", required = true)]
        limits: Vec<String>,
        /// RegionMask binding `NAME=path`; repeatable.
        #[arg(long = "mask", required = true)]
        masks: Vec<String>,
        /// New output path for the irradiation-time report JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct EndpointArgs {
    #[command(subcommand)]
    command: EndpointCommand,
}

#[derive(Debug, Subcommand)]
enum EndpointCommand {
    /// Apply an `openbnct.endpoint-model/0.1.0` artifact to a dose volume
    /// over a region mask, emitting an endpoint-evaluation report.
    Evaluate {
        /// Endpoint model JSON.
        #[arg(long)]
        model: PathBuf,
        /// Physical or biological dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `component:NAME`, `physical_total`, or `biological_total`.
        #[arg(long)]
        quantity: String,
        /// RegionMask JSON (`name` + per-voxel `voxels` booleans).
        #[arg(long)]
        mask: PathBuf,
        /// New output path for the evaluation JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Combine a TCP and an NTCP evaluation over the same dose
    /// distribution into a UTCP report.
    Utcp {
        /// TCP endpoint-evaluation JSON.
        #[arg(long)]
        tcp: PathBuf,
        /// NTCP endpoint-evaluation JSON.
        #[arg(long)]
        ntcp: PathBuf,
        /// `p_plus` (TCP·(1−NTCP)) or `difference` (TCP−NTCP).
        #[arg(long)]
        combination: String,
        /// New output path for the UTCP evaluation JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct RegisterArgs {
    #[command(subcommand)]
    command: RegisterCommand,
}

#[derive(Debug, Subcommand)]
enum RegisterCommand {
    /// Fit a rigid transform over paired landmarks (closed-form least
    /// squares) and emit a registration document.
    Landmarks {
        /// JSON array of landmark pairs
        /// (`{name?, moving_lps_mm, fixed_lps_mm}` in millimetres).
        #[arg(long)]
        pairs: PathBuf,
        /// Registration id.
        #[arg(long)]
        id: String,
        /// Moving image file whose content hash is bound into the
        /// record (any file; typically `.nii`).
        #[arg(long)]
        moving: Option<PathBuf>,
        /// Fixed (target) image file whose content hash is bound.
        #[arg(long)]
        fixed: Option<PathBuf>,
        /// Free-text provenance note (fiducial system, method).
        #[arg(long)]
        note: Option<String>,
        /// New output path for the registration JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Record an operator-declared rigid transform (e.g. transcribed
    /// from an external system's registration matrix).
    Declare {
        /// Registration id.
        #[arg(long)]
        id: String,
        /// Row-major rotation as nine comma-separated values.
        #[arg(long, value_delimiter = ',')]
        rotation: Vec<f64>,
        /// Translation in millimetres as three comma-separated values.
        #[arg(long, value_delimiter = ',')]
        translation_mm: Vec<f64>,
        /// Moving image file whose content hash is bound into the
        /// record.
        #[arg(long)]
        moving: Option<PathBuf>,
        /// Fixed (target) image file whose content hash is bound.
        #[arg(long)]
        fixed: Option<PathBuf>,
        /// Free-text provenance note (external tool, version).
        #[arg(long)]
        note: Option<String>,
        /// New output path for the registration JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Print a registration document's transform and evidence.
    Info {
        /// `openbnct.registration/0.1.0` JSON document.
        #[arg(long)]
        registration: PathBuf,
    },
    /// Resample a moving NIfTI volume onto a target grid through the
    /// registered transform.
    Apply {
        /// Moving NIfTI volume (`.nii` or `.nii.gz`).
        #[arg(long)]
        moving: PathBuf,
        /// `openbnct.registration/0.1.0` JSON document.
        #[arg(long)]
        registration: PathBuf,
        /// Target grid source: a transport-case or dose-bundle JSON.
        #[arg(long)]
        target_grid: PathBuf,
        /// Interpolation: `trilinear` (default) or `nearest` (masks,
        /// label images).
        #[arg(long, default_value = "trilinear")]
        interpolation: String,
        /// New output path for the resampled NIfTI volume.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct BoronArgs {
    #[command(subcommand)]
    command: BoronCommand,
}

#[derive(Debug, Subcommand)]
enum BoronCommand {
    /// Validate a boron uptake model JSON and print its parameters.
    Info {
        /// `openbnct.boron-uptake-model/0.1.0` JSON document.
        #[arg(long)]
        model: PathBuf,
    },
    /// Apply an uptake model to an SUV volume, emitting a per-voxel
    /// B-10 field (µg/g) with propagated 1σ on the transport grid.
    Apply {
        /// `openbnct.boron-uptake-model/0.1.0` JSON document.
        #[arg(long)]
        model: PathBuf,
        /// Transport-case JSON whose grid and case_id the field binds.
        #[arg(long)]
        case: PathBuf,
        /// SUV NIfTI volume (`.nii`/`.nii.gz`). Required unless the model
        /// mapping is `uniform`. When `--registration` is supplied the
        /// image is first moved through that transform; it is then
        /// resampled onto the case grid if the geometry differs.
        #[arg(long)]
        suv: Option<PathBuf>,
        /// Optional `openbnct.registration/0.1.0` applied to the SUV
        /// volume before resampling (e.g. PET→CT registration).
        #[arg(long)]
        registration: Option<PathBuf>,
        /// Interpolation for SUV resampling: `trilinear` (default) or
        /// `nearest`.
        #[arg(long, default_value = "trilinear")]
        interpolation: String,
        /// Field id.
        #[arg(long)]
        id: String,
        /// New output path for the `openbnct.boron-field/0.1.0` JSON.
        #[arg(long)]
        output: PathBuf,
        /// Optional NIfTI export of the concentration field.
        #[arg(long)]
        nifti_output: Option<PathBuf>,
    },
    /// Realize a boron field as a material assignment: voxels are binned
    /// into `--tiers` linearly-spaced concentration regions (voxel sets),
    /// each tier's material carrying the tier-center B10 mass fraction.
    Materialize {
        /// `openbnct.boron-field/0.1.0` JSON document.
        #[arg(long)]
        field: PathBuf,
        /// Transport-case JSON supplying the base material and binding
        /// the field's case_id/geometry.
        #[arg(long)]
        case: PathBuf,
        /// Number of concentration tiers.
        #[arg(long, default_value_t = 8)]
        tiers: u32,
        /// New output path for the material-assignment JSON (feed to
        /// `openmc generate --assignment`).
        #[arg(long)]
        output: PathBuf,
    },
    /// Evaluate a ¹⁰B subcellular microdistribution model: per-compartment
    /// α/⁷Li energy-deposition fractions to the nucleus and the
    /// nucleus-dose factor relative to uniform concentration.
    Microdistribution {
        /// `openbnct.boron-microdistribution/0.1.0` JSON document.
        #[arg(long)]
        model: PathBuf,
        /// Correction record id.
        #[arg(long)]
        id: String,
        /// New output path for the
        /// `openbnct.microdistribution-correction/0.1.0` JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct UqArgs {
    #[command(subcommand)]
    command: UqCommand,
}

#[derive(Debug, Subcommand)]
enum UqCommand {
    /// Propagate declared systematic sources over a physical dose bundle,
    /// emitting a `openbnct.systematic-uncertainty/0.1.0` report.
    Apply {
        /// Physical dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `openbnct.boron-field/0.1.0` JSON: its fractional per-voxel
        /// uncertainty scales the boron dose component.
        #[arg(long)]
        boron_field: Option<PathBuf>,
        /// Declared relative 1σ on a component as `name=sigma` (e.g.
        /// `photon=0.05`); repeatable.
        #[arg(long)]
        relative: Vec<String>,
        /// Declared positioning 1σ in millimetres; contributes
        /// `|∇D|·σ_mm` per voxel.
        #[arg(long)]
        positioning_sigma_mm: Option<f64>,
        /// Registration document bound as positioning provenance; its
        /// RMS landmark residual supplies σ when
        /// `--positioning-sigma-mm` is absent.
        #[arg(long)]
        positioning_registration: Option<PathBuf>,
        /// Region masks as `NAME=path` for region-mean results;
        /// repeatable.
        #[arg(long = "mask")]
        masks: Vec<String>,
        /// Report id.
        #[arg(long)]
        id: String,
        /// New output path for the report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate and print a systematic-uncertainty report.
    Info {
        /// `openbnct.systematic-uncertainty/0.1.0` JSON document.
        #[arg(long)]
        report: PathBuf,
    },
    /// Propagate a `openbnct.multigroup-covariance/0.1.0` artifact
    /// through the deterministic S_N solve (central-difference
    /// sensitivities) into a `openbnct.dose-uncertainty-budget/0.1.0`
    /// report on one dose component's integrated response.
    Propagate {
        /// `openbnct.transport-case/0.1.0` JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data/0.1.0` JSON.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.multigroup-covariance/0.1.0` JSON.
        #[arg(long)]
        covariance: PathBuf,
        /// Dose component whose folded response is propagated.
        #[arg(long)]
        component: String,
        /// `openbnct.material-assignment/0.1.0` JSON (overrides the
        /// case's default assignment).
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// S_N quadrature order (even, 2–16).
        #[arg(long, default_value_t = 4)]
        order: u32,
        /// Relative scalar-flux convergence target.
        #[arg(long, default_value_t = 1e-6)]
        convergence: f64,
        /// Within-group iterations per group pass.
        #[arg(long, default_value_t = 64)]
        max_inner: u32,
        /// Outer sweeps over the group structure.
        #[arg(long, default_value_t = 32)]
        max_outer: u32,
        /// Axis treated as periodic; repeatable or comma-separated
        /// (x, y, z).
        #[arg(long, value_delimiter = ',')]
        periodic: Vec<String>,
        /// Disable the analytic uncollided-beam split.
        #[arg(long)]
        no_uncollided_split: bool,
        /// Precomputed `openbnct.multigroup-flux/0.1.0` nominal forward
        /// solve to reuse instead of solving again.
        #[arg(long)]
        forward_flux: Option<PathBuf>,
        /// Declared relative 1σ statistical contribution folded in as an
        /// independent variance.
        #[arg(long)]
        statistical_rel_std: Option<f64>,
        /// Budget id.
        #[arg(long)]
        id: String,
        /// New output path for the budget JSON.
        #[arg(long)]
        output: PathBuf,
        /// Optional output path for the nominal forward-flux artifact;
        /// the artifact is content-bound into the budget.
        #[arg(long)]
        flux: Option<PathBuf>,
    },
    /// Validate and print a dose-uncertainty budget.
    BudgetInfo {
        /// `openbnct.dose-uncertainty-budget/0.1.0` JSON document.
        #[arg(long)]
        budget: PathBuf,
    },
    /// Run a `openbnct.sensitivity-spec/0.1.0` screening design —
    /// Morris elementary-effects or Saltelli-Sobol indices over
    /// declared input ranges — emitting a
    /// `openbnct.sensitivity-screening/0.1.0` report.
    Screen {
        /// `openbnct.transport-case/0.1.0` JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data/0.1.0` JSON.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.sensitivity-spec/0.1.0` JSON.
        #[arg(long)]
        spec: PathBuf,
        /// `openbnct.material-assignment/0.1.0` JSON (overrides the
        /// case's default assignment).
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// S_N quadrature order (even, 2–16).
        #[arg(long, default_value_t = 4)]
        order: u32,
        /// Relative scalar-flux convergence target.
        #[arg(long, default_value_t = 1e-6)]
        convergence: f64,
        /// Within-group iterations per group pass.
        #[arg(long, default_value_t = 64)]
        max_inner: u32,
        /// Outer sweeps over the group structure.
        #[arg(long, default_value_t = 32)]
        max_outer: u32,
        /// Axis treated as periodic; repeatable or comma-separated
        /// (x, y, z).
        #[arg(long, value_delimiter = ',')]
        periodic: Vec<String>,
        /// Disable the analytic uncollided-beam split.
        #[arg(long)]
        no_uncollided_split: bool,
        /// Report id.
        #[arg(long)]
        id: String,
        /// New output path for the screening report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate and print a sensitivity-screening report.
    ScreeningInfo {
        /// `openbnct.sensitivity-screening/0.1.0` JSON document.
        #[arg(long)]
        report: PathBuf,
    },
}

#[derive(Debug, Args)]
struct VrArgs {
    #[command(subcommand)]
    command: VrCommand,
}

#[derive(Debug, Subcommand)]
enum VrCommand {
    /// Resolve a variance-reduction spec into a concrete weight-windows
    /// artifact. `--run` is required when any window derives bounds from
    /// a completed run's forward-flux tally.
    Resolve {
        /// `openbnct.variance-reduction/0.1.0` spec JSON.
        #[arg(long)]
        spec: PathBuf,
        /// Completed OpenMC run directory (manifest + statepoint) to
        /// derive `forward_flux` bounds from.
        #[arg(long)]
        run: Option<PathBuf>,
        /// Resolved artifact id, e.g. `openbnct.nf-bnct-001.ww.v1`.
        #[arg(long)]
        id: String,
        /// New output path for the `openbnct.weight-windows` JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Resolve `adjoint`-bounds windows through the in-house S_N adjoint
    /// solver (CADIS/FW-CADIS). `uniform`/`explicit` windows pass
    /// through; `forward_flux` windows stay with `vr resolve`.
    Cadis {
        /// `openbnct.variance-reduction/0.1.0` spec JSON.
        #[arg(long)]
        spec: PathBuf,
        /// `openbnct.transport-case/0.1.0` case JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data/0.1.0` data JSON.
        #[arg(long)]
        data: PathBuf,
        /// Forward `openbnct.multigroup-flux/0.1.0` JSON — required for
        /// `fw_cadis` windows.
        #[arg(long)]
        forward_flux: Option<PathBuf>,
        /// Optional `openbnct.material-assignment/0.2.0` for
        /// heterogeneous geometry.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// S_N quadrature order (even, 2–16).
        #[arg(long, default_value_t = 4)]
        order: u32,
        /// Relative scalar-flux convergence target.
        #[arg(long, default_value_t = 1e-6)]
        convergence: f64,
        /// Within-group iterations per group pass.
        #[arg(long, default_value_t = 64)]
        max_inner: u32,
        /// Outer sweeps over the group structure (adjoint upscatter).
        #[arg(long, default_value_t = 32)]
        max_outer: u32,
        /// Axis treated as periodic; repeatable or comma-separated
        /// (x, y, z).
        #[arg(long, value_delimiter = ',')]
        periodic: Vec<String>,
        /// Resolved artifact id, e.g. `openbnct.nf-bnct-003.ww.cadis.v1`.
        #[arg(long)]
        id: String,
        /// Output path for the `openbnct.weight-windows` JSON.
        #[arg(long)]
        output: PathBuf,
        /// Optional output path for each adjoint flux solve; window i's
        /// field lands at `<path>.<i>.json` and is content-bound into
        /// the resolved artifact.
        #[arg(long)]
        adjoint_flux: Option<PathBuf>,
    },
    /// Validate and print a spec or resolved weight-windows artifact.
    Info {
        /// `openbnct.variance-reduction` or `openbnct.weight-windows` JSON.
        #[arg(long)]
        document: PathBuf,
    },
    /// Validate a completed variance-reduced run against an analog
    /// acceptance report: every shared region/tally mean must agree
    /// within `z_limit` combined sigma, and the run must have used fewer
    /// histories than the reference.
    Validate {
        /// Completed variance-reduced run directory.
        #[arg(long)]
        vr_run: PathBuf,
        /// Exit code the run finished with.
        #[arg(long, default_value_t = 0)]
        exit_code: i32,
        /// Reference `openbnct.openmc-acceptance-report` JSON from the
        /// analog campaign.
        #[arg(long)]
        reference_report: PathBuf,
        /// Histories in one reference run (each seed's particle count —
        /// the report itself does not record it).
        #[arg(long)]
        reference_histories: u64,
        /// Sigma-normalized agreement limit per comparison.
        #[arg(long, default_value_t = 3.0)]
        z_limit: f64,
        /// New output path for the `openbnct.vr-validation` report JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct BeamArgs {
    #[command(subcommand)]
    command: BeamCommand,
}

#[derive(Debug, Args)]
struct AcceleratorArgs {
    #[command(subcommand)]
    command: AcceleratorCommand,
}

#[derive(Debug, Subcommand)]
enum AcceleratorCommand {
    /// Evaluate a parametric ⁷Li(p,n)⁷Be thick-target source: forward
    /// neutron spectrum and yield from the Liskien–Paulsen 0° cross
    /// sections integrated over the proton slowing path.
    Source {
        /// Document identifier, e.g. `openbnct.accelerator-source.x.v1`.
        #[arg(long)]
        id: String,
        /// Proton energy incident on the target, MeV.
        #[arg(long)]
        proton_energy_mev: f64,
        /// Proton current on target, mA.
        #[arg(long)]
        proton_current_ma: f64,
        /// Lithium target thickness, µm. Omit for thick-target (protons
        /// stop below threshold inside the Li).
        #[arg(long)]
        target_thickness_um: Option<f64>,
        /// Beam axis: `x`, `y`, or `z`.
        #[arg(long, default_value = "z")]
        axis: String,
        /// World coordinate of the source plane along `axis`, cm.
        #[arg(long, default_value_t = 0.0)]
        plane_offset_cm: f64,
        /// Propagation direction sign along `axis`: +1 or −1.
        #[arg(long, default_value_t = 1)]
        direction_sign: i8,
        /// Port radius (emitting disk), cm.
        #[arg(long)]
        port_radius_cm: f64,
        /// Port center in the plane's in-plane world coordinates `u,v`, cm.
        #[arg(long, default_value = "0,0")]
        port_center_uv_cm: String,
        /// Emission cone half-angle, degrees.
        #[arg(long, default_value_t = 30.0)]
        half_angle_deg: f64,
        /// Spectrum histogram bin count.
        #[arg(long, default_value_t = 128)]
        spectrum_bins: u32,
        /// New output path for the accelerator-source JSON.
        #[arg(long)]
        output: PathBuf,
        /// Also emit a ready-to-use beam-description JSON at this path.
        #[arg(long)]
        beam_output: Option<PathBuf>,
        /// Identifier for the emitted beam description (required with
        /// --beam-output).
        #[arg(long, requires = "beam_output")]
        beam_id: Option<String>,
    },
    /// Emit a `BeamDescription` from an evaluated accelerator source —
    /// `computed_model` provenance binds the source artifact by hash.
    Beam {
        /// `openbnct.accelerator-source/0.1.0` JSON document.
        #[arg(long)]
        source: PathBuf,
        /// Identifier for the emitted beam description.
        #[arg(long)]
        beam_id: String,
        /// Human-readable beam name.
        #[arg(long)]
        name: String,
        /// Facility label for the emitted beam.
        #[arg(long, default_value = "accelerator-based source (parametric)")]
        facility: String,
        /// New output path for the beam-description JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct BsaArgs {
    #[command(subcommand)]
    command: BsaCommand,
}

#[derive(Debug, Subcommand)]
enum BsaCommand {
    /// Validate a beam-shaping assembly JSON and print its stack.
    Info {
        /// `openbnct.beam-shaping-assembly/0.1.0` JSON document.
        #[arg(long)]
        assembly: PathBuf,
    },
    /// Rasterize an assembly onto a transport case's grid, emitting a
    /// `openbnct.material-assignment` with the assembly's layers as
    /// voxel regions. The case supplies the grid and base material.
    Rasterize {
        /// `openbnct.beam-shaping-assembly/0.1.0` JSON document.
        #[arg(long)]
        assembly: PathBuf,
        /// `openbnct.transport-case/0.1.0` JSON — supplies the scoring
        /// grid and base material.
        #[arg(long)]
        case: PathBuf,
        /// New output path for the material-assignment JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Enumerate a BSA sweep: every combination of the declared layer
    /// thicknesses becomes a variant assembly written to the output
    /// directory, plus a `openbnct.bsa-sweep` record binding them all.
    Sweep {
        /// `openbnct.bsa-sweep/0.1.0` spec JSON (base ref + parameters).
        #[arg(long)]
        spec: PathBuf,
        /// The base `openbnct.beam-shaping-assembly/0.1.0` JSON.
        #[arg(long)]
        base: PathBuf,
        /// Directory the variant documents are written into.
        #[arg(long)]
        output_dir: PathBuf,
        /// New output path for the sweep record JSON.
        #[arg(long)]
        record: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum BeamCommand {
    /// Validate a beam-description JSON and print its declared beam.
    Info {
        /// `openbnct.beam-description/0.1.0` JSON document.
        #[arg(long)]
        beam: PathBuf,
    },
    /// List every valid beam description found in a registry directory.
    List {
        /// Directory of beam-description JSON files (repo `beams/` when
        /// invoked from the repository root).
        #[arg(long, default_value = "beams")]
        registry: PathBuf,
    },
    /// Bind a beam description to a transport case: writes a new case
    /// JSON whose source is placed just inside the entry face, centered
    /// on that face, with the declared port aperture.
    Bind {
        /// `openbnct.beam-description/0.1.0` JSON document.
        #[arg(long)]
        beam: PathBuf,
        /// `openbnct.transport-case/0.1.0` JSON the beam enters.
        #[arg(long)]
        case: PathBuf,
        /// New output path for the bound transport-case JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Characterize a beam: TECDOC-1223-style in-air metrics (exact from
    /// the declared source) plus optional in-phantom metrics from a dose
    /// bundle and an optional reference-value comparison.
    Qa {
        /// `openbnct.beam-description/0.1.0` JSON document.
        #[arg(long)]
        beam: PathBuf,
        /// Report identifier, e.g. `openbnct.beam-quality.fir1-k63.v1`.
        #[arg(long)]
        report_id: String,
        /// Optional physical dose bundle JSON from a phantom run of this
        /// beam; enables in-phantom metrics.
        #[arg(long)]
        dose: Option<PathBuf>,
        /// Optional multigroup-flux JSON from the same run (requires
        /// --dose): attaches an absolute-scale thermal fluence depth
        /// profile scaled by the declared source rate — port fluence
        /// rate × current-to-fluence × port area.
        #[arg(long, requires = "dose")]
        flux: Option<PathBuf>,
        /// Thermal/epithermal boundary for the absolute fluence profile
        /// in eV (groups whose upper edge lies at or below it).
        #[arg(long, requires = "flux", default_value = "0.5")]
        thermal_edge_ev: f64,
        /// Depths (cm from the phantom entry face) at which to attach
        /// absolute thermal-fluence transverse profiles through the
        /// port axis; repeatable or comma-separated.
        #[arg(long, requires = "flux", value_delimiter = ',')]
        transverse_depth_cm: Vec<f64>,
        /// Per-component tumor weights `B=N,H=N,N=N,P=N` (required with
        /// --dose); compound biological effectiveness factors.
        #[arg(long, requires = "dose")]
        tumor_weights: Option<String>,
        /// Per-component normal-tissue weights `B=N,H=N,N=N,P=N`.
        #[arg(long, requires = "dose")]
        normal_weights: Option<String>,
        /// Optional reference-values JSON: `{values: [{metric, value,
        /// relative_tolerance}]}`.
        #[arg(long)]
        reference: Option<PathBuf>,
        /// New output path for the beam-quality report JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct MeasurementArgs {
    #[command(subcommand)]
    command: MeasurementCommand,
}

#[derive(Debug, Subcommand)]
enum MeasurementCommand {
    /// Validate a measurement-record JSON and print its contents.
    Info {
        /// `openbnct.measurement-record/0.1.0` JSON document.
        #[arg(long)]
        record: PathBuf,
    },
    /// Compare a measurement record against a computed artifact —
    /// currently a `openbnct.beam-quality/0.1.0` report — and write a
    /// versioned `openbnct.measurement-comparison/0.1.0` record.
    Compare {
        /// `openbnct.measurement-record/0.1.0` JSON document.
        #[arg(long)]
        record: PathBuf,
        /// Computed artifact JSON to compare against (a beam-quality
        /// report).
        #[arg(long)]
        against: PathBuf,
        /// Comparison record identifier.
        #[arg(long)]
        report_id: String,
        /// Pass criterion in sigma units: |computed − measured| ≤ k·σ.
        /// Points without a stated σ are reported, never auto-passed.
        #[arg(long, default_value = "2.0")]
        sigma_tolerance: f64,
        /// New output path for the comparison record JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct DicomArgs {
    #[command(subcommand)]
    command: DicomCommand,
}

#[derive(Debug, Subcommand)]
enum DicomCommand {
    /// Write one dose volume of a physical dose bundle as a DICOM RT
    /// Dose file (multi-frame, 32-bit pixels with DoseGridScaling).
    /// Research export — not for clinical treatment use.
    ExportRtdose {
        /// `openbnct.physical-dose-bundle` JSON document.
        #[arg(long)]
        bundle: PathBuf,
        /// Which volume to export: `total` (default) or a component
        /// `B`, `N`, `H`, `P`.
        #[arg(long, default_value = "total")]
        component: String,
        /// Directory of source CT slices; their SOP Instance UIDs are
        /// emitted as ReferencedSOPSequence and their frame-of-reference
        /// is used when --frame-of-reference-uid is not given.
        #[arg(long)]
        ct_series: Option<PathBuf>,
        /// Frame of Reference UID override; defaults to the bundle's.
        #[arg(long)]
        frame_of_reference_uid: Option<String>,
        #[arg(long, default_value = "OPENBNCT^RESEARCH")]
        patient_name: String,
        #[arg(long)]
        patient_id: Option<String>,
        /// Output `.dcm` path.
        #[arg(long)]
        output: PathBuf,
    },
    /// Summarize a DICOM RT Plan file — plan identity, fraction groups,
    /// and per-beam delivery geometry — as an
    /// `openbnct.rtplan-summary/0.1.0` record.
    RtplanInfo {
        /// RT Plan `.dcm` file.
        #[arg(long)]
        input: PathBuf,
        /// Optional output path for the summary JSON; the summary is
        /// always printed either way.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Write a minimal static-beam DICOM RT Plan (one fraction group,
    /// declared beams with IEC angles and metersets). Research export —
    /// not a commissioned treatment-planning product.
    ExportRtplan {
        /// RT Plan label (also seeds deterministic `2.25.*` UIDs when the
        /// UID options are left empty).
        #[arg(long)]
        plan_label: String,
        #[arg(long, default_value = "")]
        plan_name: String,
        /// Planned fraction count for the single fraction group.
        #[arg(long, default_value_t = 1)]
        fractions: i32,
        /// Beam declaration:
        /// `name,gantry_deg,collimator_deg,couch_deg,iso_x,iso_y,iso_z,sad_mm,ssd_mm,radiation_type,meterset[,energy_mev]`.
        /// Repeatable; at least one required.
        #[arg(long)]
        beam: Vec<String>,
        /// Frame of Reference UID the isocenters live in.
        #[arg(long)]
        frame_of_reference_uid: String,
        /// DICOM PlanIntent (e.g. `VERIFICATION`, `CURATIVE`).
        #[arg(long, default_value = "VERIFICATION")]
        plan_intent: String,
        #[arg(long, default_value = "OPENBNCT")]
        machine: String,
        #[arg(long, default_value = "OPENBNCT^RESEARCH")]
        patient_name: String,
        #[arg(long)]
        patient_id: Option<String>,
        /// Output `.dcm` path.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct NiftiArgs {
    #[command(subcommand)]
    command: NiftiCommand,
}

#[derive(Debug, Subcommand)]
enum NiftiCommand {
    /// Print a NIfTI file's grid, transform provenance, and datatype.
    Info {
        /// `.nii` or gzip-compressed `.nii.gz` file.
        #[arg(long)]
        input: PathBuf,
    },
    /// Convert a NIfTI volume to a RegionMask (nonzero voxels included).
    ToMask {
        /// `.nii` or gzip-compressed `.nii.gz` file.
        #[arg(long)]
        input: PathBuf,
        /// Mask name recorded in the RegionMask JSON.
        #[arg(long)]
        name: String,
        /// New output path for the mask JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Export a dose-bundle component or total as a float64 `.nii` volume.
    ExportDose {
        /// Physical dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `component:boron|nitrogen|hydrogen|photon` or `physical_total`.
        #[arg(long)]
        quantity: String,
        /// New output path (`.nii`, or `.nii.gz` for gzip).
        #[arg(long)]
        output: PathBuf,
    },
    /// Export every component of a dose bundle as float64 NIfTI volumes —
    /// the per-component plus sigma-companion convention `import nifti`
    /// consumes — with a content-hashed
    /// `openbnct.component-nifti-manifest/0.1.0` manifest.
    ExportComponents {
        /// Physical dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// Directory to write the component volumes and manifest into.
        #[arg(long)]
        output_dir: PathBuf,
        /// Write gzip-compressed `.nii.gz` files.
        #[arg(long)]
        gzip: bool,
    },
    /// Resample a NIfTI volume onto a transport-case or dose-bundle grid.
    Resample {
        /// `.nii` or gzip-compressed `.nii.gz` file.
        #[arg(long)]
        input: PathBuf,
        /// Transport case JSON (CT-aligned grid) or dose bundle JSON
        /// supplying the target grid.
        #[arg(long)]
        target: PathBuf,
        /// `nearest` (masks/labels) or `trilinear` (dose/intensity).
        #[arg(long)]
        interpolation: String,
        /// New output path for the resampled `.nii` file.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct PositionArgs {
    #[command(subcommand)]
    command: PositionCommand,
}

#[derive(Debug, Subcommand)]
enum PositionCommand {
    /// Derive a source aimed so the beam axis passes through a mask's
    /// centroid, emitting a positioned source JSON and a position report.
    Aim {
        /// Transport-case JSON supplying the patient grid.
        #[arg(long)]
        case: PathBuf,
        /// Source JSON whose particle/energy/id the result inherits.
        #[arg(long)]
        source: PathBuf,
        /// RegionMask JSON whose centroid is the aim target.
        #[arg(long)]
        mask: PathBuf,
        /// Axis approach `+x|-x|+y|-y|+z|-z` (conflicts with --direction).
        #[arg(
            long,
            conflicts_with = "direction",
            required_unless_present = "direction"
        )]
        approach: Option<String>,
        /// Arbitrary beam direction `dx,dy,dz` in LPS (conflicts with --approach).
        #[arg(long)]
        direction: Option<String>,
        /// Aperture half-widths `HU,HV` in cm along the plane's two axes.
        #[arg(long, value_delimiter = ',')]
        half_widths_cm: Vec<f64>,
        /// How far inside the entry face the source plane sits, in cm.
        #[arg(long, default_value_t = 0.01)]
        margin_cm: f64,
        /// New output path for the positioned source JSON.
        #[arg(long)]
        output_source: PathBuf,
        /// New output path for the position report JSON.
        #[arg(long)]
        output_report: PathBuf,
    },
    /// Rotate a source's plane, aperture, and beam direction about a world
    /// axis by a multiple of 90 degrees (right-hand rule).
    Rotate {
        /// Source JSON to rotate.
        #[arg(long)]
        source: PathBuf,
        /// World axis to rotate about: `x`, `y`, or `z`.
        #[arg(long)]
        axis: String,
        /// Rotation in degrees; must be a multiple of 90.
        #[arg(long)]
        degrees: f64,
        /// Rotation center `x,y,z` in LPS mm (default: world origin).
        #[arg(long, value_delimiter = ',', default_values_t = [0.0, 0.0, 0.0])]
        center_mm: Vec<f64>,
        /// New output path for the rotated source JSON.
        #[arg(long)]
        output_source: PathBuf,
    },
}

#[derive(Debug, Args)]
struct MaskArgs {
    #[command(subcommand)]
    command: MaskCommand,
}

#[derive(Debug, Subcommand)]
enum MaskCommand {
    /// Subtract one or more masks from an input mask (e.g. organ minus tumor).
    Subtract {
        /// RegionMask JSON to subtract from.
        #[arg(long)]
        input: PathBuf,
        /// RegionMask JSON subtracted from the input; repeatable.
        #[arg(long, required = true)]
        minus: Vec<PathBuf>,
        /// Name recorded in the output mask.
        #[arg(long)]
        name: String,
        /// New output path for the mask JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Union two or more masks.
    Union {
        /// RegionMask JSONs to combine; repeatable, at least two.
        #[arg(long, required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
        /// Name recorded in the output mask.
        #[arg(long)]
        name: String,
        /// New output path for the mask JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Intersect two or more masks.
    Intersect {
        /// RegionMask JSONs to intersect; repeatable, at least two.
        #[arg(long, required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,
        /// Name recorded in the output mask.
        #[arg(long)]
        name: String,
        /// New output path for the mask JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Construct a mask from a DICOM CT series whose rescaled modality
    /// values (HU) fall inside an inclusive window.
    Threshold {
        /// Directory containing the CT slice DICOM files.
        #[arg(long)]
        ct_dir: PathBuf,
        /// Inclusive lower bound of the modality-value window.
        #[arg(long, allow_hyphen_values = true)]
        min: f64,
        /// Inclusive upper bound of the modality-value window.
        #[arg(long, allow_hyphen_values = true)]
        max: f64,
        /// Name recorded in the output mask.
        #[arg(long)]
        name: String,
        /// New output path for the mask JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct ImportArgs {
    #[command(subcommand)]
    command: ImportCommand,
}

#[derive(Debug, Args)]
struct ExportArgs {
    #[command(subcommand)]
    command: ExportCommand,
}

#[derive(Debug, Subcommand)]
enum ExportCommand {
    /// Emit an MCNP input deck for a transport case.
    ///
    /// The deck carries the case grid as an RPP box (voxel-box regions as
    /// carved RPP cells, voxel-set regions via a LAT=1 lattice fill), M
    /// cards from the declared nuclide fractions, the plane source as an
    /// SDEF card, and FMESH flux tallies on the case mesh. Component-dose
    /// folding is deliberately not emitted — it is the external pipeline's
    /// declared step before `openbnct import mcnp` re-ingests the meshtal.
    Mcnp {
        /// `openbnct.transport-case/0.1.0` document.
        #[arg(long)]
        case: PathBuf,
        /// Optional `openbnct.material-assignment/0.2.0` region assignment.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Cross-section table suffix appended to every ZAID (for example
        /// `80c`); omit to emit bare ZAIDs resolved by xsdir defaults.
        #[arg(long)]
        xs_suffix: Option<String>,
        /// Optional `RAND SEED=` value for the deck.
        #[arg(long)]
        seed: Option<u64>,
        /// New output path for the deck.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ImportCommand {
    /// Import a `openbnct.component-dose-interchange/0.1.0` document.
    Interchange {
        /// Interchange document produced by an external transport pipeline.
        #[arg(long)]
        file: PathBuf,
        /// New output path for the physical dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Lift MCNP meshtal component tallies into a physical dose bundle.
    ///
    /// Each `--component NAME=FILE:TALLY[:ENERGY_BIN]` selects one
    /// rectangular-mesh tally (column layout, `Result`/`Rel Error` rows) for
    /// that dose component. All four components are required; the selected
    /// tallies must share one uniform mesh.
    Mcnp {
        /// `component=meshtal-path:tally-number[:energy-bin]`; repeatable,
        /// once per component (boron/nitrogen/hydrogen/photon).
        #[arg(long = "component")]
        components: Vec<String>,
        /// Accumulated case identifier.
        #[arg(long)]
        case_id: String,
        /// `gray_per_source_particle` or `gray`.
        #[arg(long)]
        unit: String,
        /// Declared dose semantics: normalization basis and any folding or
        /// kerma-response treatment applied by the producer.
        #[arg(long)]
        normalization: String,
        /// MCNP version label; must match the file banner when both exist.
        #[arg(long)]
        producer_version: Option<String>,
        /// Optional DICOM frame-of-reference UID carried into the bundle.
        #[arg(long)]
        frame_of_reference_uid: Option<String>,
        /// New output path for the physical dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Lift PHITS xyz-mesh tally files into a physical dose bundle.
    ///
    /// Each `--component NAME=FILE[:ENERGY_INDEX]` selects one PHITS tally
    /// output file (`mesh = xyz`, two-dimensional `axis`); a `FILE_err.ext`
    /// sibling supplies relative errors when present.
    Phits {
        /// `component=phits-output-path[:energy-index]`; repeatable, once per
        /// component (boron/nitrogen/hydrogen/photon).
        #[arg(long = "component")]
        components: Vec<String>,
        /// Accumulated case identifier.
        #[arg(long)]
        case_id: String,
        /// `gray_per_source_particle` or `gray`; must agree with each file's
        /// `unit =` code (0 = Gy/source).
        #[arg(long)]
        unit: String,
        /// Declared dose semantics: normalization basis and any folding or
        /// kerma-response treatment applied by the producer.
        #[arg(long)]
        normalization: String,
        /// PHITS version label (required; tally files do not record it).
        #[arg(long)]
        producer_version: String,
        /// Optional DICOM frame-of-reference UID carried into the bundle.
        #[arg(long)]
        frame_of_reference_uid: Option<String>,
        /// New output path for the physical dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Lift per-component NIfTI dose volumes into a physical dose bundle.
    ///
    /// Each `--component NAME=FILE` selects one scalar `.nii`/`.nii.gz`
    /// volume (value grid) for that dose component; all four components are
    /// required and must share one grid geometry. `--component-sigma
    /// NAME=FILE` optionally supplies a paired absolute one-sigma volume —
    /// the convention used by pipelines such as OpenPINT's
    /// `get_dose_component_sigmas` outputs.
    Nifti {
        /// `component=nifti-path`; repeatable, once per component
        /// (boron/nitrogen/hydrogen/photon).
        #[arg(long = "component")]
        components: Vec<String>,
        /// `component=sigma-nifti-path`; optional, repeatable.
        #[arg(long = "component-sigma")]
        component_sigmas: Vec<String>,
        /// Accumulated case identifier.
        #[arg(long)]
        case_id: String,
        /// `gray_per_source_particle` or `gray`.
        #[arg(long)]
        unit: String,
        /// Declared dose semantics: normalization basis and any folding or
        /// kerma-response treatment applied by the producer.
        #[arg(long)]
        normalization: String,
        /// Producing system name — required; NIfTI files carry no producer
        /// identity (for example `openpint`).
        #[arg(long)]
        producer_system: String,
        /// Producer version label (optional; recorded as `undeclared` when
        /// absent since the files cannot state one).
        #[arg(long)]
        producer_version: Option<String>,
        /// Optional DICOM frame-of-reference UID carried into the bundle.
        #[arg(long)]
        frame_of_reference_uid: Option<String>,
        /// New output path for the physical dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Import a `openbnct.external-dose/0.1.0` document (single absolute-dose
    /// field with declared fractionation, e.g. a photon/hadron course).
    Dose {
        /// External-dose document produced by an external pipeline.
        #[arg(long)]
        file: PathBuf,
        /// New output path for the external dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct PlanArgs {
    #[command(subcommand)]
    command: PlanCommand,
}

#[derive(Debug, Subcommand)]
enum PlanCommand {
    /// Import a `.csv` or `.xlsx` exposure table into an
    /// `openbnct.exposure-plan/0.1.0` JSON plan.
    Import {
        /// Exposure table (`name,dose_bundle_path,...,weight_basis` columns).
        #[arg(long)]
        table: PathBuf,
        /// Plan identifier; overrides table `# id:`/`plan` sheet metadata.
        #[arg(long)]
        id: Option<String>,
        /// Accumulated case identifier; overrides table metadata.
        #[arg(long)]
        case_id: Option<String>,
        /// Directory used to hash bundle files whose `dose_bundle_sha256`
        /// cell is blank.
        #[arg(long)]
        bundles_dir: Option<PathBuf>,
        /// New output path for the plan JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Export an exposure plan to a `.csv` or `.xlsx` exposure table.
    Export {
        /// `openbnct.exposure-plan/0.1.0` JSON.
        #[arg(long)]
        plan: PathBuf,
        /// New output table path (`.csv` or `.xlsx`).
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate an exposure plan and report every detected issue.
    Validate {
        /// `openbnct.exposure-plan/0.1.0` JSON.
        #[arg(long)]
        plan: PathBuf,
    },
}

#[derive(Debug, Args)]
struct EvidenceArgs {
    #[command(subcommand)]
    command: EvidenceCommand,
}

#[derive(Debug, Subcommand)]
enum EvidenceCommand {
    /// Copy artifacts into a new directory and freeze an artifact manifest.
    Export {
        /// New bundle directory; it must not already exist.
        #[arg(long)]
        root: PathBuf,
        /// Case identifier recorded in the manifest.
        #[arg(long)]
        case_id: String,
        /// `synthetic_research_only`, `cross_code_research_only`, or
        /// `experimentally_validated_research_only`.
        #[arg(long)]
        qualification: String,
        /// One `role=SOURCE:DEST` triple per artifact; DEST is the
        /// bundle-relative path and may not escape the root.
        #[arg(long = "artifact", required = true)]
        artifacts: Vec<String>,
    },
    /// Re-hash every artifact declared by a bundle's manifest.
    Verify {
        /// Bundle directory containing artifact-manifest.json.
        #[arg(long)]
        root: PathBuf,
    },
}

#[derive(Debug, Args)]
struct BenchmarkArgs {
    #[command(subcommand)]
    command: BenchmarkCommand,
}

#[derive(Debug, Args)]
struct SnArgs {
    #[command(subcommand)]
    command: SnCommand,
}

#[derive(Debug, Subcommand)]
enum SnCommand {
    /// Solve a declared `openbnct.multigroup-data/0.1.0` problem on the
    /// transport-case grid and emit `openbnct.multigroup-flux/0.1.0`.
    /// With `--dose`, also folds the flux through the data's declared
    /// dose-response vectors into a `PhysicalDoseBundle`.
    Solve {
        /// `openbnct.transport-case/0.1.0` case JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data/0.1.0` data JSON.
        #[arg(long)]
        data: PathBuf,
        /// Optional `openbnct.material-assignment/0.2.0` for
        /// heterogeneous geometry.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// S_N quadrature order (even, 2–16).
        #[arg(long, default_value_t = 4)]
        order: u32,
        /// Relative scalar-flux convergence target.
        #[arg(long, default_value_t = 1e-6)]
        convergence: f64,
        /// Within-group iterations per group pass.
        #[arg(long, default_value_t = 64)]
        max_inner: u32,
        /// Outer sweeps over the group structure (upscatter).
        #[arg(long, default_value_t = 32)]
        max_outer: u32,
        /// Axis treated as periodic; repeatable or comma-separated
        /// (x, y, z). Periodic transverse faces realize an infinite slab.
        #[arg(long, value_delimiter = ',')]
        periodic: Vec<String>,
        /// Disable the analytic uncollided-beam split (the beam then
        /// enters as discrete-ordinates boundary flux on the nearest
        /// ordinate).
        #[arg(long)]
        no_uncollided_split: bool,
        /// Disable the extended transport correction (σ_t,tr = σ_t −
        /// μ̄·Σ_s) on the collided sweep — applies only when the data
        /// declares `transport_mu_bar`.
        #[arg(long)]
        no_transport_correction: bool,
        /// Also write a folded `openbnct.physical-dose-bundle/0.2.0` to
        /// this path (the data must declare `dose_response_gy_cm2` and a
        /// `component_profile` binding).
        #[arg(long)]
        dose: Option<PathBuf>,
        /// Output path for the multigroup-flux JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Fold an existing `openbnct.multigroup-flux/0.1.0` through its
    /// data's declared dose-response vectors into a
    /// `openbnct.physical-dose-bundle/0.2.0` — the same fold `sn solve
    /// --dose` performs, without re-solving.
    Fold {
        /// `openbnct.transport-case/0.1.0` case JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data/0.1.0` data JSON; must declare
        /// `component_profile` and dose-response vectors.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.multigroup-flux/0.1.0` from a prior `sn solve`.
        #[arg(long)]
        flux: PathBuf,
        /// The `openbnct.material-assignment/0.2.0` the solve used.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Output path for the dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Collapse processed pointwise neutron HDF5 tables into a declared
    /// multigroup-data artifact for one material.
    ///
    /// Emits `openbnct.multigroup-data/0.1.0` with weighting-collapsed
    /// σt, an analytic P0 isotropic-in-CM elastic transfer matrix
    /// (no thermal upscatter — declared), and mass-kerma dose
    /// responses: boron (n,α), nitrogen (n,p), hydrogen recoil, and
    /// photon capture-γ local kerma (no photon transport).
    Collapse {
        /// Directory of `<Nuclide>.h5` incident-neutron tables (294 K).
        #[arg(long)]
        library: PathBuf,
        /// ENDF-6 tape for a nuclide outside the processed library —
        /// `--endf Ca40=/path/n-020_Ca_040.endf`, repeatable. Raw
        /// evaluations or NJOY PENDF tapes both parse.
        #[arg(long, value_name = "NUCLIDE=PATH")]
        endf: Vec<String>,
        /// Material definition artifact (openbnct.material/0.1.0) —
        /// repeat for every material the solve requires.
        #[arg(long, required = true)]
        material: Vec<PathBuf>,
        /// Energy boundaries in eV, strictly descending — e.g.
        /// `--boundaries 1.7e7,1e4,0.5,1e-5` (group 0 = highest).
        #[arg(long, value_delimiter = ',', required = true)]
        boundaries: Vec<f64>,
        /// Artifact id for the emitted multigroup-data artifact.
        #[arg(long)]
        id: String,
        /// Component-definition-profile JSON to content-bind; required
        /// for `--dose` folding at solve time.
        #[arg(long)]
        component_profile: Option<PathBuf>,
        /// Free-text appended to the collapse declaration.
        #[arg(long)]
        note: Option<String>,
        /// Artifact path; printed to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum BenchmarkCommand {
    /// Generate deterministic DICOM inputs for NF-BNCT-001.
    Generate {
        /// New destination directory; it must not already exist.
        output: PathBuf,
    },
    /// Import and verify an NF-BNCT-001 directory against the frozen oracle.
    Verify {
        /// Directory containing ct/*.dcm and rtstruct.dcm.
        input: PathBuf,
    },
    /// Derive a material assignment from a verified case's RT structure set.
    /// ROIs that fill their bounding box become exact CSG box regions; other
    /// masks become exact voxel-set regions realized as lattice elements.
    /// Emits the assignment plus a derived transport case whose base material
    /// is the supplied file.
    DeriveMaterials {
        /// Verified NF-BNCT-001 case directory (ct/, rtstruct.dcm, case.json).
        #[arg(long)]
        case_root: PathBuf,
        /// Transport-case JSON supplying geometry, source, and histories.
        #[arg(long)]
        case: PathBuf,
        /// Base material filling all voxels outside mapped regions.
        #[arg(long)]
        base_material: PathBuf,
        /// JSON object {"regions": {"ROI_NAME": "material-file.json"}};
        /// material paths resolve relative to this file's directory.
        #[arg(long)]
        map: PathBuf,
        /// Region masks as `NAME=path` pairs (RegionMask JSON, e.g. from
        /// `nifti to-mask`). When supplied, map keys name these masks
        /// instead of RT Structure Set ROIs.
        #[arg(long = "mask")]
        masks: Vec<String>,
        /// New output path for the material-assignment JSON.
        #[arg(long)]
        output_assignment: PathBuf,
        /// New output path for the derived transport-case JSON.
        #[arg(long)]
        output_case: PathBuf,
    },
}

#[derive(Debug, Args)]
struct OpenMcArgs {
    #[command(subcommand)]
    command: OpenMcCommand,
}

#[derive(Debug, Subcommand)]
enum OpenMcCommand {
    /// Probe or acquire externally published nuclear data.
    Data(OpenMcDataArgs),
    /// Generate a deterministic OpenMC input deck bound to a reviewed response set.
    Generate {
        /// Transport-case JSON embedding the frozen geometry, material, and source.
        #[arg(long)]
        case: PathBuf,
        /// Component definition profile bound by the response set.
        #[arg(long)]
        component_profile: PathBuf,
        /// Exact material JSON bound by the case and response set.
        #[arg(long)]
        material: PathBuf,
        /// Exact source JSON bound by the case.
        #[arg(long)]
        source: PathBuf,
        /// Reviewed neutron response set (must satisfy `validate_for_folding`).
        #[arg(long)]
        response_set: PathBuf,
        /// Case-scoped OpenMC nuclear-data manifest.
        #[arg(long)]
        nuclear_data_manifest: PathBuf,
        /// Frozen OpenMC execution profile (for example the smoke profile).
        #[arg(long)]
        execution_profile: PathBuf,
        /// Root containing cross_sections.xml and every selected HDF5 file.
        #[arg(long)]
        nuclear_data_root: PathBuf,
        /// Predeclared acceptance contract (required for candidate-reference
        /// profiles, forbidden otherwise).
        #[arg(long)]
        acceptance: Option<PathBuf>,
        /// DICOM-derived material assignment (structure-derived
        /// cases only).
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Resolved `openbnct.weight-windows` artifact enabling weight-window
        /// splitting/roulette in this deck.
        #[arg(long)]
        vr: Option<PathBuf>,
        /// New output directory for the generated deck; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Prepare, execute, and collect a run in one step, optionally exporting
    /// an evidence bundle over every input and output artifact.
    Run {
        /// Transport-case JSON embedding the frozen geometry, material, and source.
        #[arg(long)]
        case: PathBuf,
        /// Component definition profile bound by the response set.
        #[arg(long)]
        component_profile: PathBuf,
        /// Exact material JSON bound by the case and response set.
        #[arg(long)]
        material: PathBuf,
        /// Exact source JSON bound by the case.
        #[arg(long)]
        source: PathBuf,
        /// Reviewed neutron response set.
        #[arg(long)]
        response_set: PathBuf,
        /// Case-scoped OpenMC nuclear-data manifest.
        #[arg(long)]
        nuclear_data_manifest: PathBuf,
        /// Frozen OpenMC execution profile.
        #[arg(long)]
        execution_profile: PathBuf,
        /// Predeclared acceptance contract (required for candidate-reference
        /// profiles, forbidden otherwise).
        #[arg(long)]
        acceptance: Option<PathBuf>,
        /// DICOM-derived material assignment (structure-derived
        /// cases only).
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Resolved `openbnct.weight-windows` artifact enabling weight-window
        /// splitting/roulette in this run.
        #[arg(long)]
        vr: Option<PathBuf>,
        /// Root containing cross_sections.xml and every selected HDF5 file.
        #[arg(long)]
        nuclear_data_root: PathBuf,
        /// OpenMC executable to launch.
        #[arg(long)]
        openmc: PathBuf,
        /// Environment overlay as KEY=VALUE; may repeat (for example
        /// `LD_LIBRARY_PATH=...` or `OMP_NUM_THREADS=...`).
        #[arg(long = "env")]
        environment: Vec<String>,
        /// New working directory for the run; it must not already exist.
        #[arg(long)]
        working_directory: PathBuf,
        /// New output path for the collected physical dose bundle JSON.
        #[arg(long)]
        dose_output: PathBuf,
        /// New directory for a hash-bound evidence bundle over the run.
        #[arg(long)]
        evidence_root: Option<PathBuf>,
    },
    /// Collect a completed run's statepoint into a normalized dose bundle.
    Collect {
        /// Completed run directory containing the deck manifest and statepoint.
        #[arg(long)]
        working_directory: PathBuf,
        /// Exit code recorded for the run (refuses collection unless zero).
        #[arg(long, default_value_t = 0)]
        exit_code: i32,
        /// New output path for the physical dose bundle JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Evaluate completed candidate-reference runs against their bound
    /// acceptance contract (precision, estimator, and seed-consistency gates).
    Evaluate {
        /// Completed run directory; repeat once per evaluated seed.
        #[arg(long = "run", required = true)]
        runs: Vec<PathBuf>,
        /// Exit code for each run, in the same order (default zero for all).
        #[arg(long = "exit-code")]
        exit_codes: Vec<i32>,
        /// New output path for the acceptance report JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct OpenMcDataArgs {
    #[command(subcommand)]
    command: OpenMcDataCommand,
}

#[derive(Debug, Args)]
struct BioArgs {
    #[command(subcommand)]
    command: BioCommand,
}

#[derive(Debug, Subcommand)]
enum BioCommand {
    /// Produce a biological dose bundle from a physical dose bundle.
    Apply {
        /// Biological model JSON (`openbnct.biological-model/0.2.0`) or a
        /// microdosimetric model (`openbnct.microdosimetric-model/0.1.0`);
        /// routed on `schema_version`.
        #[arg(long)]
        model: PathBuf,
        /// Physical dose bundle JSON produced by `openmc collect`.
        #[arg(long)]
        physical_bundle: PathBuf,
        /// Region mask as `name=path` pairs; required when the model
        /// declares region weight or LQ overrides.
        #[arg(long = "region-mask")]
        region_masks: Vec<String>,
        /// Lineal spectrum JSON (`openbnct.lineal-spectrum/0.1.0`);
        /// repeatable. Required when a microdosimetric model names
        /// spectrum-sourced lineal energies.
        #[arg(long = "spectrum")]
        spectra: Vec<PathBuf>,
        /// New output path for the biological dose bundle JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Extract a histogram measurement into a versioned lineal spectrum.
    Spectrum {
        /// Measurement record JSON (`openbnct.measurement-record/0.1.0`).
        #[arg(long)]
        record: PathBuf,
        /// `id` of the histogram-valued measurement to extract.
        #[arg(long)]
        measurement: String,
        /// Spectrum id for the emitted artifact (default: measurement id).
        #[arg(long)]
        id: Option<String>,
        /// Bin-content interpretation: `event_frequency` or
        /// `dose_weighted` (default `event_frequency`).
        #[arg(long, default_value = "event_frequency")]
        weighting: String,
        /// New output path for the lineal spectrum JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Report the frequency- and dose-mean lineal energies of a spectrum.
    LinealMean {
        /// Lineal spectrum JSON (`openbnct.lineal-spectrum/0.1.0`).
        #[arg(long)]
        spectrum: PathBuf,
    },
    /// Tally a transport-derived lineal-energy spectrum: a
    /// `openbnct.lineal-tally-spec/0.1.0` artifact (declared spherical
    /// site + per-component charged-secondary table) evaluated over a
    /// deterministic multigroup flux into `openbnct.lineal-spectrum/0.1.0`,
    /// consumable by `bio apply` MKM `computed_spectrum` sources.
    LinealTally {
        /// `openbnct.transport-case/0.1.0` JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data/0.1.0` JSON.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.multigroup-flux/0.1.0` JSON from `sn solve`.
        #[arg(long)]
        flux: PathBuf,
        /// `openbnct.lineal-tally-spec/0.1.0` JSON.
        #[arg(long)]
        spec: PathBuf,
        /// `openbnct.material-assignment/0.2.0` JSON used in the solve.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Spectrum id for the emitted artifact.
        #[arg(long)]
        id: String,
        /// New output path for the lineal spectrum JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Convert an imported external dose bundle to a BED or EQD2 field.
    Bed {
        /// External dose bundle produced by `import dose`.
        #[arg(long)]
        dose: PathBuf,
        /// α/β ratio in Gy applied to unmasked voxels.
        #[arg(long)]
        alpha_beta: f64,
        /// Region α/β override as `name=Gy`; repeatable. Each named region
        /// requires a matching `--region-mask name=path`.
        #[arg(long = "region-alpha-beta")]
        region_alpha_beta: Vec<String>,
        /// Region mask as `name=path` pairs.
        #[arg(long = "region-mask")]
        region_masks: Vec<String>,
        /// Output quantity: `bed` or `eqd2` (default `eqd2`).
        #[arg(long, default_value = "eqd2")]
        quantity: String,
        /// New output path for the BED bundle JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Add an external EQD2 course to a photon-isoeffective BNCT EQD2 bundle.
    Combine {
        /// Biological dose bundle (photon-isoeffective, `weighted_eqd2`).
        #[arg(long)]
        primary: PathBuf,
        /// External BED bundle (`eqd2` quantity) produced by `bio bed`.
        #[arg(long)]
        external: PathBuf,
        /// Co-registration method when grids differ: `trilinear`.
        #[arg(long)]
        resample: Option<String>,
        /// Operator-declared additivity assumption recorded in the output
        /// (required; for example "full-repair additive EQD2").
        #[arg(long)]
        assumption: String,
        /// New output path for the combined-dose JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Compare two biological dose bundles derived from the same
    /// physical bundle — the cross-model spread (e.g. protocol-CBE vs
    /// microdosimetric-kinetic) recorded as a checkable artifact.
    Compare {
        /// First biological dose bundle JSON.
        #[arg(long)]
        a: PathBuf,
        /// Second biological dose bundle JSON — must share the first
        /// bundle's physical-bundle provenance and geometry.
        #[arg(long)]
        b: PathBuf,
        /// Region mask as `name=path` pairs; an `all` whole-phantom row
        /// is always emitted.
        #[arg(long = "region-mask")]
        region_masks: Vec<String>,
        /// Totals below this level are excluded from the pointwise
        /// max-ratio report (ratios of near-zero doses are noise).
        #[arg(long, default_value = "0.0")]
        significant_floor: f64,
        /// New output path for the comparison JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Sweep one model parameter and record region-masked total stats.
    Sweep {
        /// Biological model JSON (`openbnct.biological-model/0.2.0`).
        #[arg(long)]
        model: PathBuf,
        /// Physical dose bundle JSON produced by `openmc collect`.
        #[arg(long)]
        physical_bundle: PathBuf,
        /// Region mask as `name=path` pairs; required when the model
        /// declares region weight overrides.
        #[arg(long = "region-mask")]
        region_masks: Vec<String>,
        /// Region whose masked min/mean/max is recorded per point.
        #[arg(long)]
        region: String,
        /// Parameter spec: `component:<name>`,
        /// `region_weight:<region>:<component>`, `alpha_beta:default`,
        /// `alpha_beta:<region>`, `fraction_count`, or
        /// `source_particles_per_fraction`.
        #[arg(long)]
        parameter: String,
        /// Parameter values; repeatable or comma-separated.
        #[arg(long = "value", value_delimiter = ',')]
        values: Vec<f64>,
        /// New output path for the sensitivity-sweep JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct NjoyArgs {
    #[command(subcommand)]
    command: NjoyCommand,
}

#[derive(Debug, Subcommand)]
enum NjoyCommand {
    /// Verify every binding and write deterministic per-nuclide NJOY decks.
    Prepare {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Exact material JSON bound by the response-generation method.
        #[arg(long)]
        material: PathBuf,
        /// Frozen response-generation method JSON.
        #[arg(long)]
        generation_method: PathBuf,
        /// Reviewed acquisition profile bound by the source selection; repeat
        /// once per bound acquisition, paired positionally with --receipt.
        #[arg(long)]
        profile: Vec<PathBuf>,
        /// Acquisition receipt bound by the source selection; repeat once per
        /// bound acquisition, paired positionally with --profile.
        #[arg(long)]
        receipt: Vec<PathBuf>,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// New output directory; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Inventory MF=6/12/13/14/15 photon-production records in exact ENDF sources.
    InventoryPhotonData {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// New source-bound JSON inventory path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a source-bound ENDF photon-production inventory.
    VerifyPhotonInventory {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Inventory to validate and regenerate.
        #[arg(long)]
        inventory: PathBuf,
    },
    /// Independently integrate File 15 spectra and fold them with File 13 cross sections.
    CalculatePhotonMoments {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Verified source-bound photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// Maximum accepted absolute error in weighted spectrum normalization.
        #[arg(long, default_value_t = DEFAULT_SPECTRUM_NORMALIZATION_TOLERANCE)]
        normalization_tolerance: f64,
        /// New source-moment JSON report path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify an independent continuum photon-moment report.
    VerifyPhotonMoments {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Verified source-bound photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// Continuum photon-moment report to validate and regenerate.
        #[arg(long)]
        moment_report: PathBuf,
    },
    /// Compare independent source moments with NJOY's diagnostic print tables.
    ComparePhotonMoments {
        /// Independently calculated continuum photon-moment report.
        #[arg(long)]
        moment_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Relative tolerance appropriate to NJOY's five-significant-digit printout.
        #[arg(long, default_value_t = DEFAULT_NJOY_PRINT_RELATIVE_TOLERANCE)]
        relative_tolerance: f64,
        /// New content-bound comparison JSON path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify an NJOY photon-moment print comparison.
    VerifyPhotonMomentComparison {
        /// Independently calculated continuum photon-moment report.
        #[arg(long)]
        moment_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Comparison report to validate and regenerate.
        #[arg(long)]
        comparison_report: PathBuf,
    },
    /// Independently test an MF=6/MT=102 photon source against its capture energy budget.
    CalculateCapturePhotonBalance {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Verified source-bound photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// Nuclide identifier in the source selection (for example, N15).
        #[arg(long)]
        nuclide: String,
        /// Maximum accepted absolute spectrum-normalization error.
        #[arg(long, default_value_t = DEFAULT_SPECTRUM_NORMALIZATION_TOLERANCE)]
        normalization_tolerance: f64,
        /// Maximum accepted relative residual in the capture energy budget.
        #[arg(long, default_value_t = DEFAULT_CAPTURE_ENERGY_BALANCE_RELATIVE_TOLERANCE)]
        relative_energy_tolerance: f64,
        /// New source-bound capture-balance JSON report; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify an independent MF=6 capture photon-balance report.
    VerifyCapturePhotonBalance {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Verified source-bound photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// Capture photon-balance report to validate and regenerate.
        #[arg(long)]
        balance_report: PathBuf,
    },
    /// Integrate deuterium MF=6/MT=16 LAW=7 and test the implicit proton energy.
    CalculateLaw7ImplicitResidual {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Verified source-bound photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// Deuterium nuclide identifier in the source selection (H2).
        #[arg(long, default_value = "H2")]
        nuclide: String,
        /// Maximum accepted absolute joint-distribution normalization error.
        #[arg(long, default_value_t = DEFAULT_LAW7_BREAKUP_NORMALIZATION_TOLERANCE)]
        normalization_tolerance: f64,
        /// Maximum accepted relative negative implicit-residual energy.
        #[arg(long, default_value_t = DEFAULT_LAW7_BREAKUP_RELATIVE_ENERGY_TOLERANCE)]
        relative_energy_tolerance: f64,
        /// New source-bound implicit-residual JSON report; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a deuterium LAW=7 implicit-residual report.
    VerifyLaw7ImplicitResidual {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Verified source-bound photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// LAW=7 implicit-residual report to validate and regenerate.
        #[arg(long)]
        residual_report: PathBuf,
    },
    /// Attribute H-2 LAW=7 warnings to NJOY's printed residual approximation.
    CompareLaw7ImplicitResidual {
        /// Independently calculated deuterium LAW=7 residual report.
        #[arg(long)]
        residual_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Maximum relative difference between source integration and NJOY quadrature.
        #[arg(long, default_value_t = DEFAULT_NJOY_LAW7_SOURCE_RELATIVE_TOLERANCE)]
        source_relative_tolerance: f64,
        /// Maximum relative difference for five-significant-digit print identities.
        #[arg(long, default_value_t = DEFAULT_NJOY_LAW7_PRINT_RELATIVE_TOLERANCE)]
        print_relative_tolerance: f64,
        /// New receipt-bound comparison JSON path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify the H-2 LAW=7 processor attribution.
    VerifyLaw7ImplicitResidualComparison {
        /// Independently calculated deuterium LAW=7 residual report.
        #[arg(long)]
        residual_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Comparison report to validate and regenerate.
        #[arg(long)]
        comparison_report: PathBuf,
    },
    /// Attribute in-domain MT=301 flags to NJOY's printed File 6 accounting.
    AttributeEnergyBalance {
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Nuclide whose high MT=301 findings will be attributed.
        #[arg(long, default_value = "O17")]
        nuclide: String,
        /// Relative tolerance for NJOY's five-significant-digit print identities.
        #[arg(long, default_value_t = DEFAULT_NJOY_ENERGY_BALANCE_PRINT_RELATIVE_TOLERANCE)]
        print_relative_tolerance: f64,
        /// New receipt-bound attribution JSON path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a processor-only energy-balance attribution.
    VerifyEnergyBalanceAttribution {
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Energy-balance attribution to validate and regenerate.
        #[arg(long)]
        attribution_report: PathBuf,
    },
    /// Integrate File 3/File 6 reaction-level energy balances independent of NJOY.
    CalculateReactionEnergyBalance {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing the selected extracted evaluations.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Receipt-bound processor energy-balance attribution for the nuclide.
        #[arg(long)]
        attribution_report: PathBuf,
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Nuclide whose reaction remainders will be integrated.
        #[arg(long, default_value = "O17")]
        nuclide: String,
        /// Relative tolerance for the printed ebal/ebar comparisons.
        #[arg(long, default_value_t = DEFAULT_REACTION_BALANCE_RELATIVE_TOLERANCE)]
        relative_tolerance: f64,
        /// New unreviewed balance JSON path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify an independent reaction energy-balance report.
    VerifyReactionEnergyBalance {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Directory containing the selected extracted evaluations.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Receipt-bound processor energy-balance attribution for the nuclide.
        #[arg(long)]
        attribution_report: PathBuf,
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Nuclide whose reaction remainders were integrated.
        #[arg(long, default_value = "O17")]
        nuclide: String,
        /// Energy-balance report to validate and regenerate.
        #[arg(long)]
        balance_report: PathBuf,
    },
    /// Generate the material neutron response set from production-HEATR PENDF tables.
    GenerateResponseTables {
        /// Exact material JSON bound by the response-generation method.
        #[arg(long)]
        material: PathBuf,
        /// Component-definition profile JSON bound by the method.
        #[arg(long)]
        component_profile: PathBuf,
        /// Frozen response-generation method JSON.
        #[arg(long)]
        generation_method: PathBuf,
        /// Processed nuclear-data manifest JSON supplying atomic weight ratios.
        #[arg(long)]
        nuclear_data_manifest: PathBuf,
        /// Derived neutron transport-domain JSON.
        #[arg(long)]
        transport_domain: PathBuf,
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Verified domain-aware suitability report carrying the findings.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// New unreviewed response-set JSON path; it must not already exist.
        #[arg(long)]
        response_set_output: PathBuf,
        /// New generation-report JSON path; it must not already exist.
        #[arg(long)]
        report_output: PathBuf,
    },
    /// Regenerate and verify a response set, then emit the review report and
    /// the independently reviewed response set that binds it.
    VerifyResponseTables {
        /// Exact material JSON bound by the response-generation method.
        #[arg(long)]
        material: PathBuf,
        /// Component-definition profile JSON bound by the method.
        #[arg(long)]
        component_profile: PathBuf,
        /// Frozen response-generation method JSON.
        #[arg(long)]
        generation_method: PathBuf,
        /// Processed nuclear-data manifest JSON supplying atomic weight ratios.
        #[arg(long)]
        nuclear_data_manifest: PathBuf,
        /// Derived neutron transport-domain JSON.
        #[arg(long)]
        transport_domain: PathBuf,
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Verified domain-aware suitability report carrying the findings.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Unreviewed response set to regenerate and validate.
        #[arg(long)]
        response_set: PathBuf,
        /// Generation report to regenerate and validate.
        #[arg(long)]
        generation_report: PathBuf,
        /// New review-report JSON path; it must not already exist.
        #[arg(long)]
        review_output: PathBuf,
        /// New independently reviewed response-set JSON path; it must not
        /// already exist.
        #[arg(long)]
        reviewed_set_output: PathBuf,
    },
    /// Compare independent capture moments with NJOY's photon and recoil print tables.
    CompareCapturePhotonMoments {
        /// Independently calculated MF=6 capture photon-balance report.
        #[arg(long)]
        balance_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Relative tolerance appropriate to NJOY's five-significant-digit printout.
        #[arg(long, default_value_t = DEFAULT_NJOY_CAPTURE_PRINT_RELATIVE_TOLERANCE)]
        relative_tolerance: f64,
        /// New content-bound comparison JSON path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify an NJOY MF=6 capture-moment print comparison.
    VerifyCapturePhotonMomentComparison {
        /// Independently calculated MF=6 capture photon-balance report.
        #[arg(long)]
        balance_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Comparison report to validate and regenerate.
        #[arg(long)]
        comparison_report: PathBuf,
    },
    /// Execute a verified input bundle and emit an unreviewed evidence receipt.
    Execute {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Exact material JSON bound by the response-generation method.
        #[arg(long)]
        material: PathBuf,
        /// Frozen response-generation method JSON.
        #[arg(long)]
        generation_method: PathBuf,
        /// Reviewed acquisition profile bound by the source selection; repeat
        /// once per bound acquisition, paired positionally with --receipt.
        #[arg(long)]
        profile: Vec<PathBuf>,
        /// Acquisition receipt bound by the source selection; repeat once per
        /// bound acquisition, paired positionally with --profile.
        #[arg(long)]
        receipt: Vec<PathBuf>,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
        /// Exact prepared bundle to verify before execution.
        #[arg(long)]
        input_bundle: PathBuf,
        /// Real regular NJOY executable to invoke with an empty environment.
        #[arg(long)]
        njoy_executable: PathBuf,
        /// Additional processor/runtime artifact to bind by hash; repeatable.
        #[arg(long = "processor-support-artifact")]
        processor_support_artifacts: Vec<PathBuf>,
        /// Per-nuclide wall-clock timeout.
        #[arg(long, default_value_t = DEFAULT_NJOY_TIMEOUT_SECONDS)]
        timeout_seconds: u64,
        /// New evidence directory; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify every execution artifact against an external receipt.
    VerifyExecution {
        /// Receipt used as the independent trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory, including its byte-identical receipt.
        #[arg(long)]
        execution_directory: PathBuf,
    },
    /// Derive a transported-photon KERMA suitability report from verified logs.
    AssessExecution {
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// New JSON report path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and compare a transported-photon suitability report.
    VerifySuitability {
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Suitability report to validate and regenerate.
        #[arg(long)]
        suitability_report: PathBuf,
    },
    /// Reinterpret verified diagnostics using source-bound ENDF photon records.
    AssessSourceAware {
        /// Verified legacy v0.1 transported-photon suitability report.
        #[arg(long)]
        legacy_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Exact NJOY input manifest bound by the execution receipt.
        #[arg(long)]
        input_manifest: PathBuf,
        /// Source-bound ENDF photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// New v0.2 JSON report path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a source-aware v0.2 suitability report.
    VerifySourceAware {
        /// Verified legacy v0.1 transported-photon suitability report.
        #[arg(long)]
        legacy_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Exact NJOY input manifest bound by the execution receipt.
        #[arg(long)]
        input_manifest: PathBuf,
        /// Source-bound ENDF photon-production inventory.
        #[arg(long)]
        photon_inventory: PathBuf,
        /// Source-aware v0.2 report to validate and regenerate.
        #[arg(long)]
        source_aware_report: PathBuf,
    },
    /// Scope source-aware kinematic findings to a content-bound transport domain.
    AssessDomainAware {
        /// Verified source-aware v0.2 transported-photon suitability report.
        #[arg(long)]
        source_aware_report: PathBuf,
        /// Verified legacy v0.1 transported-photon suitability report.
        #[arg(long)]
        legacy_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Exact NJOY input manifest bound by the execution receipt.
        #[arg(long)]
        input_manifest: PathBuf,
        /// Exact OpenMC nuclear-data manifest used to derive the domain.
        #[arg(long)]
        nuclear_data_manifest: PathBuf,
        /// Exact material JSON shared by the NJOY run and transport domain.
        #[arg(long)]
        material: PathBuf,
        /// Derived OpenMC neutron transport-domain document.
        #[arg(long)]
        transport_domain: PathBuf,
        /// New v0.3 JSON report path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a transport-domain-aware v0.3 suitability report.
    VerifyDomainAware {
        /// Verified source-aware v0.2 transported-photon suitability report.
        #[arg(long)]
        source_aware_report: PathBuf,
        /// Verified legacy v0.1 transported-photon suitability report.
        #[arg(long)]
        legacy_report: PathBuf,
        /// External execution receipt used as the trust anchor.
        #[arg(long)]
        receipt: PathBuf,
        /// Complete execution directory bound by the receipt.
        #[arg(long)]
        execution_directory: PathBuf,
        /// Exact NJOY input manifest bound by the execution receipt.
        #[arg(long)]
        input_manifest: PathBuf,
        /// Exact OpenMC nuclear-data manifest used to derive the domain.
        #[arg(long)]
        nuclear_data_manifest: PathBuf,
        /// Exact material JSON shared by the NJOY run and transport domain.
        #[arg(long)]
        material: PathBuf,
        /// Derived OpenMC neutron transport-domain document.
        #[arg(long)]
        transport_domain: PathBuf,
        /// Domain-aware v0.3 report to validate and regenerate.
        #[arg(long)]
        domain_aware_report: PathBuf,
    },
    /// Apply reaction-level H-2 and N-15 evidence over immutable v0.3 suitability.
    AssessEvidenceAware {
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// Independent H-2 LAW=7 implicit-residual report.
        #[arg(long)]
        law7_residual_report: PathBuf,
        /// Receipt-bound H-2 LAW=7 processor attribution.
        #[arg(long)]
        law7_comparison_report: PathBuf,
        /// Independent N-15 MF=6 capture-balance report.
        #[arg(long)]
        capture_balance_report: PathBuf,
        /// Receipt-bound N-15 capture-moment comparison.
        #[arg(long)]
        capture_comparison_report: PathBuf,
        /// New v0.4 JSON report path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a reaction-evidence-aware v0.4 suitability report.
    VerifyEvidenceAware {
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// Independent H-2 LAW=7 implicit-residual report.
        #[arg(long)]
        law7_residual_report: PathBuf,
        /// Receipt-bound H-2 LAW=7 processor attribution.
        #[arg(long)]
        law7_comparison_report: PathBuf,
        /// Independent N-15 MF=6 capture-balance report.
        #[arg(long)]
        capture_balance_report: PathBuf,
        /// Receipt-bound N-15 capture-moment comparison.
        #[arg(long)]
        capture_comparison_report: PathBuf,
        /// Evidence-aware v0.4 report to validate and regenerate.
        #[arg(long)]
        evidence_aware_report: PathBuf,
    },
    /// Separate source-data blockers from findings needing independent diagnostics.
    AssessDiagnosticTriage {
        /// Verified reaction-evidence-aware v0.4 suitability report.
        #[arg(long)]
        evidence_aware_report: PathBuf,
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// New diagnostic-triage JSON report; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a diagnostic-triage report.
    VerifyDiagnosticTriage {
        /// Verified reaction-evidence-aware v0.4 suitability report.
        #[arg(long)]
        evidence_aware_report: PathBuf,
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// Diagnostic-triage report to validate and regenerate.
        #[arg(long)]
        triage_report: PathBuf,
    },
    /// Verify the complete triage evidence chain and write a compact machine result.
    CheckDiagnosticTriage {
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// Independent H-2 LAW=7 implicit-residual report.
        #[arg(long)]
        law7_residual_report: PathBuf,
        /// Receipt-bound H-2 LAW=7 processor attribution.
        #[arg(long)]
        law7_comparison_report: PathBuf,
        /// Independent N-15 MF=6 capture-balance report.
        #[arg(long)]
        capture_balance_report: PathBuf,
        /// Receipt-bound N-15 capture-moment comparison.
        #[arg(long)]
        capture_comparison_report: PathBuf,
        /// Verified reaction-evidence-aware v0.4 suitability report.
        #[arg(long)]
        evidence_aware_report: PathBuf,
        /// Verified diagnostic-triage report.
        #[arg(long)]
        triage_report: PathBuf,
        /// New deterministic JSON result; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify v0.4 evidence and write a compact machine-facing check result.
    CheckEvidenceAware {
        /// Verified domain-aware v0.3 transported-photon suitability report.
        #[arg(long)]
        domain_aware_report: PathBuf,
        /// Independent H-2 LAW=7 implicit-residual report.
        #[arg(long)]
        law7_residual_report: PathBuf,
        /// Receipt-bound H-2 LAW=7 processor attribution.
        #[arg(long)]
        law7_comparison_report: PathBuf,
        /// Independent N-15 MF=6 capture-balance report.
        #[arg(long)]
        capture_balance_report: PathBuf,
        /// Receipt-bound N-15 capture-moment comparison.
        #[arg(long)]
        capture_comparison_report: PathBuf,
        /// Evidence-aware v0.4 report to validate and regenerate.
        #[arg(long)]
        evidence_aware_report: PathBuf,
        /// New deterministic JSON result; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Compare a candidate suitability report against a rejected baseline.
    CompareSuitability {
        /// Rejected baseline transported-photon suitability report.
        #[arg(long)]
        baseline_report: PathBuf,
        /// Candidate transported-photon suitability report.
        #[arg(long)]
        candidate_report: PathBuf,
        /// New JSON comparison path; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a response-treatment candidate comparison.
    VerifyComparison {
        /// Rejected baseline transported-photon suitability report.
        #[arg(long)]
        baseline_report: PathBuf,
        /// Candidate transported-photon suitability report.
        #[arg(long)]
        candidate_report: PathBuf,
        /// Comparison report to validate and regenerate.
        #[arg(long)]
        comparison_report: PathBuf,
    },
    /// Verify a candidate comparison and write a compact machine result.
    CheckCandidateComparison {
        /// Rejected baseline transported-photon suitability report.
        #[arg(long)]
        baseline_report: PathBuf,
        /// Candidate transported-photon suitability report.
        #[arg(long)]
        candidate_report: PathBuf,
        /// Comparison report to validate and regenerate.
        #[arg(long)]
        comparison_report: PathBuf,
        /// New deterministic JSON result; it must not already exist.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum OpenMcDataCommand {
    /// Make a one-byte range probe and retain no response body.
    Probe {
        /// Reviewed NCTForge data-acquisition profile.
        #[arg(long)]
        profile: PathBuf,
    },
    /// Download or resume an artifact and emit a content-addressed receipt.
    Acquire {
        /// Reviewed NCTForge data-acquisition profile.
        #[arg(long)]
        profile: PathBuf,
        /// Existing directory for the artifact, partial file, and receipt.
        #[arg(long)]
        output_directory: PathBuf,
        /// Exact byte count reported by `data probe`; required as a size guard.
        #[arg(long)]
        confirm_size_bytes: u64,
    },
    /// Verify a case-scoped evaluated-neutron selection and every extracted file.
    VerifySelection {
        /// Case-scoped evaluated-neutron source-selection manifest.
        #[arg(long)]
        selection: PathBuf,
        /// Exact material JSON bound by the selection.
        #[arg(long)]
        material: PathBuf,
        /// Reviewed acquisition profile bound by the receipt; repeat once per
        /// bound acquisition, paired positionally with --receipt.
        #[arg(long)]
        profile: Vec<PathBuf>,
        /// Acquisition receipt checked into the case provenance; repeat once
        /// per bound acquisition, paired positionally with --profile.
        #[arg(long)]
        receipt: Vec<PathBuf>,
        /// Directory containing exactly the selected extracted ENDF files.
        #[arg(long)]
        evaluations_directory: PathBuf,
    },
    /// Verify a processed-data manifest, selected files, and optional material capabilities.
    VerifyManifest {
        /// Case-scoped OpenMC nuclear-data manifest generated by the inspector.
        #[arg(long)]
        manifest: PathBuf,
        /// Root containing cross_sections.xml and every selected HDF5 file.
        #[arg(long)]
        data_root: PathBuf,
        /// Optional material definition whose required capabilities must pass.
        #[arg(long)]
        material: Option<PathBuf>,
    },
    /// Derive the common neutron transport interval for an exact material.
    DeriveTransportDomain {
        /// Case-scoped OpenMC nuclear-data capability manifest.
        #[arg(long)]
        manifest: PathBuf,
        /// Exact material definition selecting nuclides and temperature.
        #[arg(long)]
        material: PathBuf,
        /// New content-bound transport-domain JSON path.
        #[arg(long)]
        output: PathBuf,
    },
    /// Regenerate and verify a neutron transport-domain document.
    VerifyTransportDomain {
        /// Case-scoped OpenMC nuclear-data capability manifest.
        #[arg(long)]
        manifest: PathBuf,
        /// Exact material definition selecting nuclides and temperature.
        #[arg(long)]
        material: PathBuf,
        /// Transport-domain document to validate and regenerate.
        #[arg(long)]
        transport_domain: PathBuf,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Pair each `--profile` with the `--receipt` at the same position; mixed
/// selections pass one pair per bound acquisition.
fn acquisition_pairs<'a>(
    profiles: &'a [PathBuf],
    receipts: &'a [PathBuf],
) -> Result<Vec<(&'a PathBuf, &'a PathBuf)>, Box<dyn Error>> {
    if profiles.is_empty() || profiles.len() != receipts.len() {
        return Err(
            io::Error::other("each --profile requires one --receipt, and vice versa").into(),
        );
    }
    Ok(profiles.iter().zip(receipts).collect())
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Some(Command::Backends) => {
            let backend = OpenMcBackend::default();
            let descriptor = backend.descriptor();
            println!(
                "{} ({}) prepare={} execute={} import={}",
                descriptor.display_name,
                descriptor.id,
                descriptor.can_prepare,
                descriptor.can_execute,
                descriptor.can_import
            );
        }
        Some(Command::Benchmark(args)) => match args.command {
            BenchmarkCommand::Generate { output } => {
                let generated = generate_nf_bnct_001(&output)?;
                println!("generated NF-BNCT-001 at {}", generated.root.display());
                println!("CT slices: {}", generated.ct_files.len());
                println!("RT Structure Set: {}", generated.rtstruct_file.display());
                println!("Case manifest: {}", generated.manifest_file.display());
            }
            BenchmarkCommand::Verify { input } => {
                let report = verify_nf_bnct_001(&input)?;
                println!(
                    "verified {}: shape={:?}, spacing_mm={:?}, CT slices={}",
                    report.case_id, report.shape, report.spacing_mm, report.ct_slice_count
                );
                println!(
                    "artifact integrity: {} files verified",
                    report.verified_artifact_count
                );
                for roi in report.rois {
                    println!(
                        "ROI {}: voxels={}, volume_cm3={}, centroid_lps_mm={:?}",
                        roi.name, roi.voxel_count, roi.volume_cm3, roi.centroid_lps_mm
                    );
                }
            }
            BenchmarkCommand::DeriveMaterials {
                case_root,
                case,
                base_material,
                map,
                masks,
                output_assignment,
                output_case,
            } => {
                let verified = load_nf_bnct_001(&case_root)?;
                let mut derived_case: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                let base: MaterialDefinition = serde_json::from_slice(&fs::read(&base_material)?)?;
                if derived_case.geometry != verified.ct.geometry {
                    return Err(io::Error::other(
                        "transport-case geometry differs from the verified DICOM geometry",
                    )
                    .into());
                }
                derived_case.material = base;
                // External RegionMask files (e.g. from `nifti to-mask`)
                // supplement or replace RT Structure Set ROIs when given.
                let external_masks: std::collections::BTreeMap<String, openbnct_core::RegionMask> =
                    masks
                        .iter()
                        .map(|binding| {
                            let (name, path) = binding.split_once('=').ok_or_else(|| {
                                io::Error::other("--mask entries must be NAME=path")
                            })?;
                            let mask: openbnct_core::RegionMask =
                                serde_json::from_slice(&fs::read(path)?)?;
                            if mask.name != name {
                                return Err(io::Error::other(format!(
                                    "--mask {name}: mask file names itself {:?}",
                                    mask.name
                                )));
                            }
                            let expected_voxels = verified
                                .ct
                                .geometry
                                .voxel_count()
                                .map_err(|error| io::Error::other(error.to_string()))?;
                            if mask.voxels.len() != expected_voxels {
                                return Err(io::Error::other(format!(
                                    "--mask {name}: {} voxels, grid expects {}",
                                    mask.voxels.len(),
                                    expected_voxels,
                                )));
                            }
                            Ok((name.to_owned(), mask))
                        })
                        .collect::<Result<_, io::Error>>()?;
                let map_bytes = fs::read(&map)?;
                let mapping: serde_json::Value = serde_json::from_slice(&map_bytes)?;
                let regions = mapping
                    .get("regions")
                    .and_then(|value| value.as_object())
                    .ok_or_else(|| {
                        io::Error::other(
                            "material map must be {\"regions\": {\"ROI\": \"material.json\"}}",
                        )
                    })?;
                let map_dir = map.parent().unwrap_or(Path::new("."));
                let geometry = &verified.ct.geometry;
                let [nx, ny, _] = geometry.shape.map(|v| v as usize);

                let mut material_regions = Vec::with_capacity(regions.len());
                for (name, material_path) in regions {
                    let material_path = material_path.as_str().ok_or_else(|| {
                        io::Error::other(format!("region {name:?} must map to a file path"))
                    })?;
                    let material: MaterialDefinition =
                        serde_json::from_slice(&fs::read(map_dir.join(material_path))?).map_err(
                            |error| io::Error::other(format!("region {name:?} material: {error}")),
                        )?;
                    let mask_voxels: &[bool] = if let Some(mask) = external_masks.get(name.as_str())
                    {
                        &mask.voxels
                    } else if external_masks.is_empty() {
                        &verified
                            .structures
                            .roi(name)
                            .ok_or_else(|| io::Error::other(format!("no ROI named {name:?}")))?
                            .voxels
                    } else {
                        return Err(io::Error::other(format!("no --mask named {name:?}")).into());
                    };

                    // Rasterize the mask to voxel indices; when it fills its
                    // own bounding box exactly it becomes a CSG box region,
                    // otherwise an exact voxel-set region realized as
                    // per-voxel lattice elements.
                    let mut lower = [u32::MAX; 3];
                    let mut upper = [0_u32; 3];
                    let mut indices = Vec::new();
                    for (index, included) in mask_voxels.iter().copied().enumerate() {
                        if !included {
                            continue;
                        }
                        let k = index / (nx * ny);
                        let j = (index % (nx * ny)) / nx;
                        let i = index % nx;
                        indices.push([i as u32, j as u32, k as u32]);
                        for (axis, value) in [i, j, k].iter().enumerate() {
                            lower[axis] = lower[axis].min(*value as u32);
                            upper[axis] = upper[axis].max(*value as u32);
                        }
                    }
                    if indices.is_empty() {
                        return Err(io::Error::other(format!("ROI {name:?} is empty")).into());
                    }
                    let box_voxels: u64 = (0..3)
                        .map(|axis| u64::from(upper[axis] - lower[axis] + 1))
                        .product();
                    let shape = if box_voxels == indices.len() as u64 {
                        openbnct_transport::MaterialRegionShape::VoxelBox { lower, upper }
                    } else {
                        openbnct_transport::MaterialRegionShape::VoxelSet { indices }
                    };
                    material_regions.push(MaterialRegion {
                        name: name.clone(),
                        material,
                        shape,
                    });
                }

                let case_sha = openbnct_evidence::sha256_file(&case_root.join("case.json"))?;
                let assignment = MaterialAssignment {
                    schema_version: MATERIAL_ASSIGNMENT_SCHEMA.into(),
                    // The assignment binds the transport case it is validated
                    // against; the DICOM case is bound through provenance_id.
                    case_id: derived_case.case_id.clone(),
                    base_material: derived_case.material.clone(),
                    regions: material_regions,
                    provenance_id: format!("case:sha256:{case_sha}"),
                };
                assignment.validate(geometry).map_err(|error| {
                    io::Error::other(format!("derived assignment is invalid: {error}"))
                })?;
                derived_case.validate().map_err(|error| {
                    io::Error::other(format!("derived transport case is invalid: {error}"))
                })?;
                for (path, document) in [
                    (&output_assignment, serde_json::to_value(&assignment)?),
                    (&output_case, serde_json::to_value(&derived_case)?),
                ] {
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)?;
                    serde_json::to_writer_pretty(&mut file, &document)?;
                    file.write_all(b"\n")?;
                }
                println!(
                    "derived {} material region(s) for {}",
                    assignment.regions.len(),
                    assignment.case_id
                );
                for region in &assignment.regions {
                    println!(
                        "region {}: {} voxel(s) ({}) -> {}",
                        region.name,
                        region.voxel_count(),
                        match &region.shape {
                            openbnct_transport::MaterialRegionShape::VoxelBox { lower, upper } =>
                                format!("box {lower:?}..{upper:?}"),
                            openbnct_transport::MaterialRegionShape::VoxelSet { .. } =>
                                "voxel set".to_owned(),
                        },
                        region.material.id
                    );
                }
                println!("assignment: {}", output_assignment.display());
                println!("derived case: {}", output_case.display());
            }
        },
        Some(Command::Beam(args)) => match args.command {
            BeamCommand::Info { beam } => {
                let beam: openbnct_transport::BeamDescription =
                    serde_json::from_slice(&fs::read(&beam)?)?;
                beam.validate()
                    .map_err(|error| io::Error::other(format!("beam: {error}")))?;
                print_beam_summary(&beam);
            }
            BeamCommand::List { registry } => {
                let mut entries: Vec<PathBuf> = fs::read_dir(&registry)?
                    .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                    .filter(|path| {
                        path.extension().is_some_and(|ext| ext == "json")
                            && path.file_name().is_some_and(|name| name != "manifest.json")
                    })
                    .collect();
                entries.sort();
                if entries.is_empty() {
                    println!("no beam descriptions in {}", registry.display());
                }
                for path in entries {
                    match serde_json::from_slice::<openbnct_transport::BeamDescription>(&fs::read(
                        &path,
                    )?) {
                        Ok(beam) => match beam.validate() {
                            Ok(()) => {
                                println!("{} — {}", beam.id, path.display());
                                println!("  {} · {}", beam.name, beam.facility);
                            }
                            Err(error) => {
                                println!("{} — INVALID: {error}", path.display())
                            }
                        },
                        Err(error) => {
                            println!("{} — unreadable: {error}", path.display())
                        }
                    }
                }
            }
            BeamCommand::Bind { beam, case, output } => {
                let beam: openbnct_transport::BeamDescription =
                    serde_json::from_slice(&fs::read(&beam)?)?;
                let mut bound: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                bound.source = beam
                    .bound_source(&bound.geometry)
                    .map_err(|error| io::Error::other(format!("beam bind: {error}")))?;
                bound
                    .validate()
                    .map_err(|error| io::Error::other(format!("bound case is invalid: {error}")))?;
                write_new_json(&output, &bound)?;
                println!("bound {} onto {}", beam.id, bound.case_id);
                println!("case: {}", output.display());
            }
            BeamCommand::Qa {
                beam,
                report_id,
                dose,
                flux,
                thermal_edge_ev,
                transverse_depth_cm,
                tumor_weights,
                normal_weights,
                reference,
                output,
            } => {
                let beam_bytes = fs::read(&beam)?;
                let beam: openbnct_transport::BeamDescription =
                    serde_json::from_slice(&beam_bytes)?;
                let beam_reference = openbnct_transport::ContentReference {
                    id: beam.id.clone(),
                    sha256: format!("sha256:{}", openbnct_evidence::sha256_hex(&beam_bytes)),
                };
                let dose_inputs = match dose {
                    Some(dose_path) => {
                        let dose_bytes = fs::read(&dose_path)?;
                        let bundle: openbnct_core::PhysicalDoseBundle =
                            serde_json::from_slice(&dose_bytes)?;
                        let dose_reference = openbnct_transport::ContentReference {
                            id: format!("dose:{}", bundle.provenance_id),
                            sha256: format!(
                                "sha256:{}",
                                openbnct_evidence::sha256_hex(&dose_bytes)
                            ),
                        };
                        let tumor = parse_component_weights(
                            tumor_weights.as_deref().unwrap_or_default(),
                            "--tumor-weights",
                        )?;
                        let normal = parse_component_weights(
                            normal_weights.as_deref().unwrap_or_default(),
                            "--normal-weights",
                        )?;
                        Some((bundle, dose_reference, tumor, normal))
                    }
                    None => None,
                };
                let reference = reference
                    .map(|path| {
                        let reference: openbnct_transport::BeamQualityReference =
                            serde_json::from_slice(&fs::read(&path)?)?;
                        Ok::<_, io::Error>(reference)
                    })
                    .transpose()?;
                let mut report = openbnct_transport::evaluate_beam_quality(
                    &report_id,
                    &beam,
                    beam_reference,
                    dose_inputs
                        .as_ref()
                        .map(|(bundle, reference, tumor, normal)| {
                            (bundle, reference.clone(), tumor.clone(), normal.clone())
                        }),
                    reference.as_ref(),
                )
                .map_err(|error| io::Error::other(format!("beam qa: {error}")))?;
                if let Some(flux_path) = &flux {
                    let bundle = &dose_inputs.as_ref().expect("--flux requires --dose").0;
                    let flux_model: openbnct_transport::MultigroupFlux =
                        serde_json::from_slice(&fs::read(flux_path)?)?;
                    // Declared absolute scale: port fluence rate × the
                    // beam's forward current/fluence ratio × port area
                    // → source neutrons per second.
                    let jphi = report.in_air.current_to_fluence_ratio;
                    let area = report.in_air.port_area_cm2;
                    let rate = match &beam.normalization {
                        openbnct_transport::NormalizationBasis::FluenceRateAtPort {
                            fluence_rate_cm2_s,
                        } => fluence_rate_cm2_s * jphi * area,
                        other => {
                            return Err(io::Error::other(format!(
                                "--flux requires a declared fluence-rate normalization, found {other:?}"
                            ))
                            .into());
                        }
                    };
                    let note = format!(
                        "absolute scale: declared port fluence rate {:.4e} cm^-2 s^-1 × \
                         current-to-fluence {:.4} × port area {:.2} cm^2 → {:.4e} source \
                         n/s; thermal groups below {thermal_edge_ev} eV; footprint-averaged",
                        rate / (jphi * area),
                        jphi,
                        area,
                        rate
                    );
                    openbnct_transport::attach_absolute_fluence_profile(
                        &mut report,
                        &beam,
                        &bundle.geometry,
                        &flux_model,
                        thermal_edge_ev,
                        rate,
                        &note,
                    )
                    .map_err(|error| {
                        io::Error::other(format!("beam qa absolute fluence: {error}"))
                    })?;
                    if !transverse_depth_cm.is_empty() {
                        openbnct_transport::attach_transverse_fluence_profiles(
                            &mut report,
                            &beam,
                            &bundle.geometry,
                            &flux_model,
                            thermal_edge_ev,
                            rate,
                            &transverse_depth_cm,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("beam qa transverse fluence: {error}"))
                        })?;
                    }
                }
                report
                    .validate()
                    .map_err(|error| io::Error::other(format!("beam qa report: {error}")))?;
                write_new_json(&output, &report)?;
                println!("beam qa: {}", report.id);
                println!(
                    "  epithermal {} / thermal {} / fast {} cm^-2 s^-1",
                    report.in_air.epithermal_fluence_rate_cm2_s,
                    report.in_air.thermal_fluence_rate_cm2_s,
                    report.in_air.fast_fluence_rate_cm2_s
                );
                println!("  J/Phi = {:.4}", report.in_air.current_to_fluence_ratio);
                if let Some(in_phantom) = &report.in_phantom {
                    println!(
                        "  AD {:.2} cm  AR {:.3}  PTR {:.3}",
                        in_phantom.advantage_depth_cm,
                        in_phantom.advantage_ratio,
                        in_phantom.peak_therapeutic_ratio
                    );
                }
                if let Some(comparisons) = &report.reference_comparison {
                    for comparison in comparisons {
                        println!(
                            "  {} {}: computed {:.4} vs reference {:.4} (rel diff {:.3}, tol {:.3})",
                            if comparison.passed { "PASS" } else { "FAIL" },
                            comparison.metric,
                            comparison.computed,
                            comparison.reference,
                            comparison.relative_difference,
                            comparison.relative_tolerance
                        );
                    }
                }
                println!("report: {}", output.display());
            }
        },
        Some(Command::Accelerator(args)) => match args.command {
            AcceleratorCommand::Source {
                id,
                proton_energy_mev,
                proton_current_ma,
                target_thickness_um,
                axis,
                plane_offset_cm,
                direction_sign,
                port_radius_cm,
                port_center_uv_cm,
                half_angle_deg,
                spectrum_bins,
                output,
                beam_output,
                beam_id,
            } => {
                let axis = parse_plane_axis(&axis)?;
                let center = parse_f64_pair(&port_center_uv_cm)?;
                let source = openbnct_transport::evaluate_accelerator_source(
                    &id,
                    openbnct_transport::AcceleratorSourceSpec {
                        proton_energy_mev,
                        proton_current_ma,
                        target_thickness_um,
                        axis,
                        plane_offset_cm,
                        direction_sign,
                        port_radius_cm,
                        port_center_uv_cm: center,
                        half_angle_deg,
                        spectrum_bins,
                    },
                )
                .map_err(|error| io::Error::other(format!("accelerator source: {error}")))?;
                write_new_json(&output, &source)?;
                let source_bytes = serde_json::to_vec_pretty(&source)?;
                let source_reference = openbnct_transport::ContentReference {
                    id: source.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&source_bytes),
                };
                println!("accelerator source: {}", source.id);
                println!(
                    "  7Li(p,n): Ep={} MeV, I={} mA, {}",
                    source.spec.proton_energy_mev,
                    source.spec.proton_current_ma,
                    if source.derived.thick_target {
                        format!(
                            "thick target ({:.1} µm required)",
                            source.derived.thick_target_depth_um
                        )
                    } else {
                        format!(
                            "partially thick (Ep exit {:.3} MeV)",
                            source.derived.proton_exit_energy_mev
                        )
                    }
                );
                println!(
                    "  forward yield {:.3e} n/p/sr · cone yield {:.3e} n/s · En {:.0}–{:.0} keV",
                    source.derived.forward_yield_per_proton_sr,
                    source.derived.cone_yield_per_s,
                    source.derived.neutron_energy_range_ev[0] / 1000.0,
                    source.derived.neutron_energy_range_ev[1] / 1000.0
                );
                println!("source: {}", output.display());
                if let Some(beam_path) = beam_output {
                    let beam = source
                        .to_beam_description(
                            beam_id.as_deref().unwrap_or(""),
                            &format!("parametric {} mA Li(p,n) source", proton_current_ma),
                            "accelerator-based source (parametric)",
                            source_reference,
                        )
                        .map_err(|error| io::Error::other(format!("beam derive: {error}")))?;
                    write_new_json(&beam_path, &beam)?;
                    println!("beam: {}", beam_path.display());
                }
            }
            AcceleratorCommand::Beam {
                source,
                beam_id,
                name,
                facility,
                output,
            } => {
                let source_bytes = fs::read(&source)?;
                let source: openbnct_transport::AcceleratorSource =
                    serde_json::from_slice(&source_bytes)?;
                let reference = openbnct_transport::ContentReference {
                    id: source.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&source_bytes),
                };
                let beam = source
                    .to_beam_description(&beam_id, &name, &facility, reference)
                    .map_err(|error| io::Error::other(format!("beam derive: {error}")))?;
                write_new_json(&output, &beam)?;
                println!("beam: {}", output.display());
            }
        },
        Some(Command::Bsa(args)) => match args.command {
            BsaCommand::Info { assembly } => {
                let assembly: openbnct_transport::BeamShapingAssembly =
                    serde_json::from_slice(&fs::read(&assembly)?)?;
                assembly
                    .validate()
                    .map_err(|error| io::Error::other(format!("bsa: {error}")))?;
                println!(
                    "bsa: {} ({} layers, {:.1} cm depth)",
                    assembly.id,
                    assembly.layers.len(),
                    assembly.total_depth_cm()
                );
                for layer in &assembly.layers {
                    println!(
                        "  {} [{}]: {:.2} cm {}",
                        layer.name,
                        serde_json::to_value(layer.kind)
                            .and_then(serde_json::from_value::<String>)
                            .unwrap_or_default(),
                        layer.thickness_cm,
                        layer.material.id
                    );
                }
            }
            BsaCommand::Rasterize {
                assembly,
                case,
                output,
            } => {
                let assembly: openbnct_transport::BeamShapingAssembly =
                    serde_json::from_slice(&fs::read(&assembly)?)?;
                let case: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                let mut assignment = assembly
                    .to_material_assignment(
                        &case.geometry,
                        case.material.clone(),
                        &format!("bsa:{}", assembly.id),
                    )
                    .map_err(|error| io::Error::other(format!("bsa rasterize: {error}")))?;
                assignment.case_id = case.case_id.clone();
                assignment
                    .validate(&case.geometry)
                    .map_err(|error| io::Error::other(format!("assignment: {error}")))?;
                write_new_json(&output, &assignment)?;
                println!(
                    "assignment: {} ({} regions)",
                    output.display(),
                    assignment.regions.len()
                );
            }
            BsaCommand::Sweep {
                spec,
                base,
                output_dir,
                record,
            } => {
                let spec_bytes = fs::read(&spec)?;
                let sweep: openbnct_transport::BsaSweep = serde_json::from_slice(&spec_bytes)?;
                let base_bytes = fs::read(&base)?;
                let base: openbnct_transport::BeamShapingAssembly =
                    serde_json::from_slice(&base_bytes)?;
                let variants = openbnct_transport::enumerate_bsa_sweep(&sweep, &base)
                    .map_err(|error| io::Error::other(format!("bsa sweep: {error}")))?;
                fs::create_dir_all(&output_dir)?;
                let mut records = Vec::new();
                for variant in &variants {
                    let bytes = serde_json::to_vec_pretty(variant)?;
                    let path = output_dir.join(format!("{}.json", variant.id));
                    write_new_text(&path, &bytes)?;
                    records.push(openbnct_transport::BsaSweepVariant {
                        id: variant.id.clone(),
                        parameters: openbnct_transport::sweep_variant_assignment(&sweep, variant),
                        content: openbnct_transport::ContentReference {
                            id: variant.id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&bytes),
                        },
                    });
                }
                let record_doc = openbnct_transport::BsaSweepRecord {
                    schema_version: openbnct_transport::BSA_SWEEP_SCHEMA.into(),
                    id: format!("{}.record", sweep.id),
                    sweep: openbnct_transport::ContentReference {
                        id: sweep.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&spec_bytes),
                    },
                    base: openbnct_transport::ContentReference {
                        id: base.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&base_bytes),
                    },
                    variants: records,
                    qualification: "enumerated beam-shaping-assembly variants for research \
                        screening — transport execution and beam-quality evaluation run on the \
                        existing paths; no equivalence or clinical claim"
                        .into(),
                };
                record_doc
                    .validate()
                    .map_err(|error| io::Error::other(format!("sweep record: {error}")))?;
                write_new_json(&record, &record_doc)?;
                println!(
                    "sweep {}: {} variants into {}",
                    sweep.id,
                    record_doc.variants.len(),
                    output_dir.display()
                );
                println!("record: {}", record.display());
            }
        },
        Some(Command::Measurement(args)) => match args.command {
            MeasurementCommand::Info { record } => {
                let record: openbnct_transport::MeasurementRecord =
                    serde_json::from_slice(&fs::read(&record)?)?;
                record
                    .validate()
                    .map_err(|error| io::Error::other(format!("measurement record: {error}")))?;
                println!(
                    "{} — {} measurement(s)",
                    record.id,
                    record.measurements.len()
                );
                for measurement in &record.measurements {
                    let value = match &measurement.value {
                        openbnct_transport::MeasurementValue::Scalar {
                            value,
                            absolute_uncertainty_1sigma,
                        } => match absolute_uncertainty_1sigma {
                            Some(sigma) => format!("{value} ± {sigma}"),
                            None => format!("{value} (σ not stated)"),
                        },
                        openbnct_transport::MeasurementValue::Histogram { bin_values, .. } => {
                            format!("histogram, {} bins", bin_values.len())
                        }
                    };
                    println!(
                        "  {} [{}] {} {}",
                        measurement.id, measurement.metric, value, measurement.unit
                    );
                }
            }
            MeasurementCommand::Compare {
                record,
                against,
                report_id,
                sigma_tolerance,
                output,
            } => {
                let record_bytes = fs::read(&record)?;
                let record: openbnct_transport::MeasurementRecord =
                    serde_json::from_slice(&record_bytes)?;
                let record_reference = openbnct_transport::ContentReference {
                    id: record.id.clone(),
                    sha256: format!("sha256:{}", openbnct_evidence::sha256_hex(&record_bytes)),
                };
                let against_bytes = fs::read(&against)?;
                let report: openbnct_transport::BeamQualityReport =
                    serde_json::from_slice(&against_bytes)?;
                let computed_reference = openbnct_transport::ContentReference {
                    id: report.id.clone(),
                    sha256: format!("sha256:{}", openbnct_evidence::sha256_hex(&against_bytes)),
                };
                let comparison = openbnct_transport::compare_measurement_record(
                    &report_id,
                    &record,
                    record_reference,
                    &report,
                    computed_reference,
                    sigma_tolerance,
                )
                .map_err(|error| io::Error::other(format!("measurement compare: {error}")))?;
                comparison
                    .validate()
                    .map_err(|error| io::Error::other(format!("comparison record: {error}")))?;
                write_new_json(&output, &comparison)?;
                println!("measurement compare: {}", comparison.id);
                for entry in &comparison.comparisons {
                    let status = match entry.passed {
                        Some(true) => "PASS",
                        Some(false) => "FAIL",
                        None => "----",
                    };
                    let detail = match (entry.computed, entry.difference_sigma) {
                        (Some(computed), Some(sigma)) => format!(
                            "computed {computed:.4} vs measured {:.4} ({:.2}σ)",
                            entry.measured.unwrap_or(f64::NAN),
                            sigma
                        ),
                        (Some(computed), None) => format!(
                            "computed {computed:.4} vs measured {:.4} (rel diff {:.3}, no σ)",
                            entry.measured.unwrap_or(f64::NAN),
                            entry.relative_difference.unwrap_or(f64::NAN)
                        ),
                        (None, _) => "unmatched metric".to_string(),
                    };
                    println!("  {status} {}: {detail}", entry.measurement_id);
                }
                for profile in &comparison.profile_comparisons {
                    let status = match profile.passed {
                        Some(true) => "PASS",
                        Some(false) => "FAIL",
                        None => "----",
                    };
                    let detail = match &profile.chi_square {
                        Some(chi) => format!(
                            "profile: {} bins, max rel diff {:.3}, chi-square {chi:.3}",
                            profile.bin_centers.len(),
                            profile.max_relative_difference
                        ),
                        None => format!(
                            "profile: {} bins, max rel diff {:.3} (no σ)",
                            profile.bin_centers.len(),
                            profile.max_relative_difference
                        ),
                    };
                    println!("  {status} {}: {detail}", profile.measurement_id);
                }
                println!(
                    "  {} compared / {} passed / {} failed / {} unmatched / {} without σ / {} profiles",
                    comparison.summary.compared,
                    comparison.summary.passed,
                    comparison.summary.failed,
                    comparison.summary.unmatched,
                    comparison.summary.without_uncertainty,
                    comparison.summary.profiles_compared
                );
                if let Some(chi_square) = comparison.summary.chi_square {
                    println!(
                        "  chi-square {chi_square:.3} over {} dof",
                        comparison.summary.degrees_of_freedom
                    );
                }
                println!("report: {}", output.display());
            }
        },
        Some(Command::Dicom(args)) => match args.command {
            DicomCommand::ExportRtdose {
                bundle,
                component,
                ct_series,
                frame_of_reference_uid,
                patient_name,
                patient_id,
                output,
            } => {
                let bundle: openbnct_core::PhysicalDoseBundle =
                    serde_json::from_slice(&fs::read(&bundle)?)?;
                let selection = match component.trim().to_ascii_lowercase().as_str() {
                    "total" => openbnct_dicom::DoseSelection::PhysicalTotal,
                    "b" | "boron" => openbnct_dicom::DoseSelection::Component(
                        openbnct_core::DoseComponent::Boron,
                    ),
                    "n" | "nitrogen" => openbnct_dicom::DoseSelection::Component(
                        openbnct_core::DoseComponent::Nitrogen,
                    ),
                    "h" | "hydrogen" => openbnct_dicom::DoseSelection::Component(
                        openbnct_core::DoseComponent::Hydrogen,
                    ),
                    "p" | "photon" => openbnct_dicom::DoseSelection::Component(
                        openbnct_core::DoseComponent::Photon,
                    ),
                    other => {
                        return Err(io::Error::other(format!(
                            "--component must be total|B|N|H|P, not {other:?}"
                        ))
                        .into());
                    }
                };
                let mut options = openbnct_dicom::RtDoseExportOptions {
                    patient_name,
                    patient_id: patient_id.unwrap_or_else(|| bundle.case_id.clone()),
                    ..Default::default()
                };
                if let Some(dir) = &ct_series {
                    let mut slices = fs::read_dir(dir)?
                        .map(|entry| entry.map(|entry| entry.path()))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    slices.sort();
                    let ct = openbnct_dicom::import_ct_series(&slices)
                        .map_err(|error| io::Error::other(format!("ct series: {error}")))?;
                    options.referenced_ct_instance_uids = ct.slice_sop_instance_uids;
                    options.study_instance_uid = ct.study_instance_uid;
                    if options.frame_of_reference_uid.is_empty() {
                        options.frame_of_reference_uid = ct.frame_of_reference_uid;
                    }
                }
                if let Some(uid) = frame_of_reference_uid {
                    options.frame_of_reference_uid = uid;
                }
                let result = openbnct_dicom::export_rt_dose(&bundle, selection, &options, &output)
                    .map_err(|error| io::Error::other(format!("rtdose export: {error}")))?;
                println!("rtdose: {}", result.path.display());
                println!(
                    "  units {} · dose grid scaling {:.6e}",
                    result.dose_units, result.dose_grid_scaling
                );
            }
            DicomCommand::RtplanInfo { input, output } => {
                let summary = openbnct_dicom::summarize_rt_plan(&input)
                    .map_err(|error| io::Error::other(format!("rtplan: {error}")))?;
                println!(
                    "rtplan {} — {} beam(s), {} fraction group(s)",
                    summary.rt_plan_label,
                    summary.beams.len(),
                    summary.fraction_groups.len()
                );
                for beam in &summary.beams {
                    let cp = beam.control_point.as_ref();
                    println!(
                        "  beam {} {:?}: {:?} gantry={:?}° metersets={:?}",
                        beam.beam_number,
                        beam.beam_name.as_deref().unwrap_or("?"),
                        beam.radiation_type.as_deref().unwrap_or("?"),
                        cp.and_then(|c| c.gantry_angle_deg),
                        beam.metersets
                    );
                }
                if let Some(path) = output {
                    write_new_json(&path, &summary)?;
                    println!("summary at {}", path.display());
                }
            }
            DicomCommand::ExportRtplan {
                plan_label,
                plan_name,
                fractions,
                beam,
                frame_of_reference_uid,
                plan_intent,
                machine,
                patient_name,
                patient_id,
                output,
            } => {
                let mut beams = Vec::with_capacity(beam.len());
                for spec in &beam {
                    let fields: Vec<&str> = spec.split(',').collect();
                    if !(11..=12).contains(&fields.len()) {
                        return Err(io::Error::other(format!(
                            "--beam expects 11 or 12 comma-separated fields \
                             (name,gantry,collimator,couch,iso_x,iso_y,iso_z,sad,ssd,radiation,meterset[,energy]); \
                             got {} in {spec:?}",
                            fields.len()
                        ))
                        .into());
                    }
                    let parse = |i: usize| -> Result<f64, Box<dyn std::error::Error>> {
                        fields[i].trim().parse::<f64>().map_err(|e| {
                            io::Error::other(format!("--beam field {i} in {spec:?}: {e}")).into()
                        })
                    };
                    beams.push(openbnct_dicom::RtPlanBeamSpec {
                        name: fields[0].trim().to_owned(),
                        gantry_angle_deg: parse(1)?,
                        collimator_angle_deg: parse(2)?,
                        patient_support_angle_deg: parse(3)?,
                        isocenter_position_mm: [parse(4)?, parse(5)?, parse(6)?],
                        source_axis_distance_mm: parse(7)?,
                        source_to_surface_distance_mm: parse(8)?,
                        radiation_type: fields[9].trim().to_owned(),
                        meterset: parse(10)?,
                        nominal_beam_energy_mev: (fields.len() == 12)
                            .then(|| parse(11))
                            .transpose()?,
                    });
                }
                let options = openbnct_dicom::RtPlanExportOptions {
                    patient_name,
                    patient_id: patient_id.unwrap_or_default(),
                    study_instance_uid: String::new(),
                    series_instance_uid: String::new(),
                    sop_instance_uid: String::new(),
                    frame_of_reference_uid,
                    plan_label,
                    plan_name,
                    plan_intent,
                    number_of_fractions: fractions,
                    treatment_machine_name: machine,
                    beams,
                };
                openbnct_dicom::export_rt_plan(&output, &options)
                    .map_err(|error| io::Error::other(format!("rtplan export: {error}")))?;
                println!("rtplan: {}", output.display());
            }
        },
        Some(Command::Openmc(args)) => match args.command {
            OpenMcCommand::Data(args) => match args.command {
                OpenMcDataCommand::Probe { profile } => {
                    let document = DataAcquisitionProfileDocument::from_path(&profile)?;
                    let result = DataAcquisitionClient::new()?.probe(&document)?;
                    println!("profile: {}", result.profile_id);
                    println!(
                        "artifact: {} ({} bytes; {:.2} GiB)",
                        result.expected_filename,
                        result.size_bytes,
                        bytes_to_gib(result.size_bytes)
                    );
                    println!("range resume: {}", result.accepts_ranges);
                    println!("final HTTPS origin: {}", result.final_origin);
                    if document.profile.artifact.publisher_digest.is_none() {
                        println!("publisher digest: unavailable; acquisition remains unqualified");
                    } else {
                        println!("publisher digest: pinned in profile and checked on acquisition");
                    }
                    println!(
                        "acquisition requires --confirm-size-bytes {}",
                        result.size_bytes
                    );
                }
                OpenMcDataCommand::Acquire {
                    profile,
                    output_directory,
                    confirm_size_bytes,
                } => {
                    let document = DataAcquisitionProfileDocument::from_path(&profile)?;
                    let client = DataAcquisitionClient::new()?;
                    let total = document.profile.artifact.expected_size_bytes;
                    let mut next_report = 0_u64;
                    let acquired = client.acquire_with_progress(
                        &document,
                        &output_directory,
                        confirm_size_bytes,
                        |progress| {
                            if progress.completed_bytes >= next_report
                                || progress.completed_bytes == progress.total_bytes
                            {
                                eprintln!(
                                    "acquired {} / {} bytes ({:.1}%)",
                                    progress.completed_bytes,
                                    progress.total_bytes,
                                    100.0 * progress.completed_bytes as f64
                                        / progress.total_bytes as f64
                                );
                                next_report = progress
                                    .completed_bytes
                                    .saturating_add((total / 100).max(64 * 1024 * 1024));
                            }
                        },
                    )?;
                    println!("artifact: {}", acquired.artifact_path.display());
                    println!("SHA-256: {}", acquired.receipt.artifact.sha256);
                    println!("receipt: {}", acquired.receipt_path.display());
                    println!("evidence state: acquisition_only");
                }
                OpenMcDataCommand::VerifySelection {
                    selection,
                    material,
                    profile,
                    receipt,
                    evaluations_directory,
                } => {
                    let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                    let material_bytes = fs::read(&material)?;
                    let material: MaterialDefinition = serde_json::from_slice(&material_bytes)?;
                    let pairs = acquisition_pairs(&profile, &receipt)?;
                    let pairs = pairs
                        .iter()
                        .map(|(profile, receipt)| {
                            Ok::<_, Box<dyn Error>>((
                                DataAcquisitionProfileDocument::from_path(profile)?,
                                DataAcquisitionReceiptDocument::from_path(receipt)?,
                            ))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let pair_refs = pairs
                        .iter()
                        .map(|(profile, receipt)| (profile, receipt))
                        .collect::<Vec<_>>();

                    selection
                        .selection
                        .validate_for_material(&material, &material_bytes)?;
                    selection.selection.validate_acquisitions(&pair_refs)?;
                    selection.selection.verify_files(&evaluations_directory)?;

                    println!("selection: {}", selection.selection.id);
                    println!("selection SHA-256: {}", selection.sha256);
                    println!(
                        "verified evaluations: {}",
                        selection.selection.evaluations.len()
                    );
                    for acquisition in selection.selection.declared_acquisitions() {
                        println!("archive SHA-256: {}", acquisition.archive_sha256);
                    }
                    println!(
                        "qualification: {}",
                        match selection.selection.qualification {
                            EvaluatedSourceQualification::CandidateArchiveEquivalenceUnresolved =>
                                "candidate_archive_equivalence_unresolved",
                            EvaluatedSourceQualification::ResponseTreatmentCandidateUnreviewed =>
                                "response_treatment_candidate_unreviewed",
                        }
                    );
                }
                OpenMcDataCommand::VerifyManifest {
                    manifest,
                    data_root,
                    material,
                } => {
                    let manifest_json = fs::read(&manifest)?;
                    let manifest: NuclearDataManifest = serde_json::from_slice(&manifest_json)?;
                    manifest.verify_files(&data_root)?;

                    println!("manifest: {}", manifest.id);
                    println!(
                        "verified processed-data artifacts: {}",
                        1 + manifest.neutron_tables.len() + manifest.photon_tables.len()
                    );
                    println!("archive SHA-256: {}", manifest.distribution.archive_sha256);
                    println!("qualification: processed_data_identity_verified");

                    if let Some(material) = material {
                        let material_json = fs::read(material)?;
                        let material: MaterialDefinition = serde_json::from_slice(&material_json)?;
                        manifest.validate_for_material(&material)?;
                        println!("material capabilities: verified");
                    }
                }
                OpenMcDataCommand::DeriveTransportDomain {
                    manifest,
                    material,
                    output,
                } => {
                    let manifest_bytes = fs::read(manifest)?;
                    let material_bytes = fs::read(material)?;
                    let domain =
                        OpenMcNeutronTransportDomain::derive(&manifest_bytes, &material_bytes)?;
                    let result = domain.write_new(&output)?;
                    println!("derived OpenMC neutron transport domain");
                    println!("domain: {}", result.domain_path.display());
                    println!("domain SHA-256: {}", result.domain_sha256);
                    println!(
                        "closed diagnostic interval: [{}, {}] eV",
                        result.domain.energy_range_ev[0], result.domain.energy_range_ev[1]
                    );
                    println!("qualification: backend_capability_derived_unreviewed");
                }
                OpenMcDataCommand::VerifyTransportDomain {
                    manifest,
                    material,
                    transport_domain,
                } => {
                    let manifest_bytes = fs::read(manifest)?;
                    let material_bytes = fs::read(material)?;
                    let domain =
                        OpenMcNeutronTransportDomainDocument::from_path(&transport_domain)?;
                    domain.verify_against_inputs(&manifest_bytes, &material_bytes)?;
                    println!(
                        "verified OpenMC neutron transport domain {}",
                        transport_domain.display()
                    );
                    println!("domain SHA-256: {}", domain.sha256);
                    println!(
                        "closed diagnostic interval: [{}, {}] eV",
                        domain.domain.energy_range_ev[0], domain.domain.energy_range_ev[1]
                    );
                    println!("qualification: backend_capability_derived_unreviewed");
                }
            },
            OpenMcCommand::Generate {
                case,
                component_profile,
                material,
                source,
                response_set,
                nuclear_data_manifest,
                execution_profile,
                nuclear_data_root,
                acceptance,
                assignment,
                vr,
                output,
            } => {
                let case_json = fs::read(&case)?;
                let case: TransportCase = serde_json::from_slice(&case_json)?;
                let component_profile_json = fs::read(&component_profile)?;
                let material_json = fs::read(&material)?;
                let source_json = fs::read(&source)?;
                let response_set_json = fs::read(&response_set)?;
                let nuclear_data_manifest_json = fs::read(&nuclear_data_manifest)?;
                let execution_profile_json = fs::read(&execution_profile)?;
                let acceptance_json = acceptance.as_ref().map(fs::read).transpose()?;
                let assignment_json = assignment.as_ref().map(fs::read).transpose()?;
                let vr_json = vr.as_ref().map(fs::read).transpose()?;
                let deck = OpenMcInputDeck::generate(
                    &case,
                    &nuclear_data_root,
                    OpenMcInputArtifacts {
                        component_profile_json: &component_profile_json,
                        material_json: &material_json,
                        source_json: &source_json,
                        response_set_json: &response_set_json,
                        nuclear_data_manifest_json: &nuclear_data_manifest_json,
                        execution_profile_json: &execution_profile_json,
                        acceptance_json: acceptance_json.as_deref(),
                        material_assignment_json: assignment_json.as_deref(),
                        variance_reduction_json: vr_json.as_deref(),
                    },
                )?;
                deck.write_new(&output)?;
                println!(
                    "generated deterministic OpenMC input deck at {}",
                    output.display()
                );
                println!("case: {}", deck.manifest.case_id);
                println!("xml artifacts: {}", deck.manifest.xml_artifacts.len());
                println!("tallies: {}", deck.manifest.tallies.len());
                println!(
                    "particles per batch: {}",
                    deck.manifest.execution.particles_per_batch
                );
            }
            OpenMcCommand::Run {
                case,
                component_profile,
                material,
                source,
                response_set,
                nuclear_data_manifest,
                execution_profile,
                acceptance,
                assignment,
                vr,
                nuclear_data_root,
                openmc,
                environment,
                working_directory,
                dose_output,
                evidence_root,
            } => {
                let case_json = fs::read(&case)?;
                let case_document: TransportCase = serde_json::from_slice(&case_json)?;
                let config = openbnct_openmc::OpenMcBackendConfig {
                    component_profile,
                    material,
                    source,
                    response_set,
                    nuclear_data_manifest,
                    execution_profile,
                    acceptance,
                    material_assignment: assignment,
                    variance_reduction: vr,
                    nuclear_data_root,
                };
                let mut backend = OpenMcBackend::new(&openmc).configured(config);
                for pair in &environment {
                    let (key, value) = pair.split_once('=').ok_or_else(|| {
                        io::Error::other(format!(
                            "environment overlay {pair:?} must be written as KEY=VALUE"
                        ))
                    })?;
                    backend = backend.with_env(key, value);
                }
                let prepared = backend.prepare(&case_document, &working_directory)?;
                println!("prepared deck in {}", prepared.working_directory);
                let completed = backend.execute(&prepared)?;
                if completed.exit_code != 0 {
                    return Err(io::Error::other(format!(
                        "openmc exited with code {}",
                        completed.exit_code
                    ))
                    .into());
                }
                println!("execution finished with exit code 0");
                let bundle = backend.collect(&completed)?;
                let json = serde_json::to_vec_pretty(&bundle)?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&dose_output)?;
                file.write_all(&json)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                println!(
                    "collected physical dose bundle at {}",
                    dose_output.display()
                );
                println!("provenance: {}", bundle.provenance_id);

                if let Some(evidence_root) = evidence_root {
                    let config = backend
                        .config()
                        .expect("backend was configured above")
                        .clone();
                    let working = working_directory.clone();
                    let mut artifacts: Vec<(&str, PathBuf, String)> = vec![
                        ("case", case.clone(), "case.json".into()),
                        (
                            "component_profile",
                            config.component_profile.clone(),
                            "component-profile.json".into(),
                        ),
                        ("material", config.material.clone(), "material.json".into()),
                        ("source", config.source.clone(), "source.json".into()),
                        (
                            "response_set",
                            config.response_set.clone(),
                            "response-tables/response-set.json".into(),
                        ),
                        (
                            "nuclear_data_manifest",
                            config.nuclear_data_manifest.clone(),
                            "nuclear-data-manifest.json".into(),
                        ),
                        (
                            "execution_profile",
                            config.execution_profile.clone(),
                            "run-settings.json".into(),
                        ),
                        (
                            "input_manifest",
                            working.join(openbnct_openmc::OPENMC_INPUT_MANIFEST_FILE),
                            "inputs/openbnct-input-manifest.json".into(),
                        ),
                        (
                            "run_receipt",
                            working.join(openbnct_openmc::OPENMC_RUN_RECEIPT_FILE),
                            "openbnct-openmc-run-receipt.json".into(),
                        ),
                        (
                            "dose_bundle",
                            dose_output.clone(),
                            "normalized-dose.json".into(),
                        ),
                    ];
                    if let Some(acceptance) = &config.acceptance {
                        artifacts.push((
                            "acceptance_contract",
                            acceptance.clone(),
                            "openbnct-acceptance-contract.json".into(),
                        ));
                    }
                    for xml in [
                        "settings.xml",
                        "materials.xml",
                        "geometry.xml",
                        "tallies.xml",
                    ] {
                        artifacts.push(("deck", working.join(xml), xml.into()));
                    }
                    for entry in fs::read_dir(&working)? {
                        let path = entry?.path();
                        let name = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .to_string();
                        if name.ends_with(".log") {
                            artifacts.push(("log", path, format!("logs/{name}")));
                        } else if name.starts_with("statepoint.") && name.ends_with(".h5") {
                            artifacts.push((
                                "statepoint",
                                path,
                                format!("statepoints-or-native-results/{name}"),
                            ));
                        }
                    }
                    let inputs: Vec<openbnct_evidence::BundleInput> = artifacts
                        .into_iter()
                        .map(|(role, source, dest)| openbnct_evidence::BundleInput {
                            role: role.into(),
                            source,
                            relative_path: dest,
                            media_type: None,
                        })
                        .collect();
                    let manifest = openbnct_evidence::export_evidence_bundle(
                        &evidence_root,
                        &bundle.case_id,
                        openbnct_evidence::QualificationBoundary::SyntheticResearchOnly,
                        &inputs,
                    )?;
                    println!("evidence bundle exported at {}", evidence_root.display());
                    println!("artifacts: {}", manifest.artifacts.len());
                }
            }
            OpenMcCommand::Collect {
                working_directory,
                exit_code,
                output,
            } => {
                let completed = CompletedRun {
                    backend_id: "openmc".into(),
                    case_id: String::new(),
                    working_directory: working_directory.display().to_string(),
                    exit_code,
                };
                let bundle = OpenMcBackend::default().collect(&completed)?;
                let json = serde_json::to_vec_pretty(&bundle)?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                file.write_all(&json)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                println!("collected physical dose bundle at {}", output.display());
                println!("case: {}", bundle.case_id);
                println!("components: {}", bundle.components.len());
                println!("provenance: {}", bundle.provenance_id);
            }
            OpenMcCommand::Evaluate {
                runs,
                exit_codes,
                output,
            } => {
                let exit_codes = if exit_codes.is_empty() {
                    vec![0; runs.len()]
                } else {
                    exit_codes
                };
                let run_refs: Vec<&Path> = runs.iter().map(PathBuf::as_path).collect();
                let report = evaluate_runs(&run_refs, &exit_codes)?;
                let json = serde_json::to_vec_pretty(&report)?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                file.write_all(&json)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                println!("acceptance report at {}", output.display());
                println!("case: {}", report.case_id);
                println!("runs evaluated: {}", report.runs.len());
                println!(
                    "estimator comparisons: {} ({} failed)",
                    report.estimator_comparisons.len(),
                    report
                        .estimator_comparisons
                        .iter()
                        .filter(|c| !c.passed)
                        .count()
                );
                println!("gates passed: {}", report.gates_passed);
            }
        },
        Some(Command::Njoy(args)) => match args.command {
            NjoyCommand::Prepare {
                selection,
                material,
                generation_method,
                profile,
                receipt,
                evaluations_directory,
                output,
            } => {
                let selection_json = fs::read(selection)?;
                let material_json = fs::read(material)?;
                let generation_method_json = fs::read(generation_method)?;
                let acquisition_bytes = acquisition_pairs(&profile, &receipt)?
                    .iter()
                    .map(|(profile, receipt)| {
                        Ok::<_, io::Error>((fs::read(profile)?, fs::read(receipt)?))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let acquisitions = acquisition_bytes
                    .iter()
                    .map(|(profile, receipt)| NjoyAcquisitionArtifacts {
                        profile_json: profile,
                        receipt_json: receipt,
                    })
                    .collect();
                let bundle = NjoyInputBundle::generate(
                    &evaluations_directory,
                    NjoyInputArtifacts {
                        evaluated_source_selection_json: &selection_json,
                        material_json: &material_json,
                        generation_method_json: &generation_method_json,
                        acquisitions,
                    },
                )?;
                bundle.write_new(&output)?;
                println!("prepared NJOY2016.78 inputs at {}", output.display());
                println!("nuclide runs: {}", bundle.manifest.runs.len());
                println!(
                    "source selection SHA-256: {}",
                    bundle.manifest.bindings.evaluated_source_selection.sha256
                );
                println!("qualification: input_preparation_only");
            }
            NjoyCommand::InventoryPhotonData {
                selection,
                evaluations_directory,
                output,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventory::inspect(&selection, &evaluations_directory)?;
                let result = inventory.write_new(&output)?;
                println!("inventoried exact ENDF photon-production records");
                println!("inventory: {}", result.inventory_path.display());
                println!("inventory SHA-256: {}", result.inventory_sha256);
                println!("evaluations: {}", result.inventory.evaluations.len());
                println!(
                    "MF=6/12/13/14/15 sections: {}",
                    result.inventory.section_count
                );
                println!(
                    "evaluations with a HEATR photon source: {}",
                    result.inventory.evaluations_with_heatr_photon_source_count
                );
                println!("format findings: {}", result.inventory.format_finding_count);
                println!("qualification: source_inventory_unreviewed");
            }
            NjoyCommand::VerifyPhotonInventory {
                selection,
                evaluations_directory,
                inventory,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let document = EndfPhotonProductionInventoryDocument::from_path(&inventory)?;
                document.verify_against_selection(&selection, &evaluations_directory)?;
                println!(
                    "verified ENDF photon-production inventory {}",
                    inventory.display()
                );
                println!("inventory SHA-256: {}", document.sha256);
                println!("evaluations: {}", document.inventory.evaluations.len());
                println!(
                    "format findings: {}",
                    document.inventory.format_finding_count
                );
                println!("qualification: source_inventory_unreviewed");
            }
            NjoyCommand::CalculatePhotonMoments {
                selection,
                evaluations_directory,
                photon_inventory,
                normalization_tolerance,
                output,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report = EndfContinuumPhotonMomentReport::calculate(
                    &selection,
                    &evaluations_directory,
                    &inventory,
                    normalization_tolerance,
                )?;
                let result = report.write_new(&output)?;
                println!("calculated independent ENDF continuum photon moments");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!("reactions: {}", result.report.reaction_count);
                println!("incident-energy samples: {}", result.report.sample_count);
                println!(
                    "maximum absolute normalization error: {:.12e}",
                    result.report.maximum_absolute_normalization_error
                );
                if result.report.failed_sample_count == 0 {
                    println!("qualification: source_moments_checked_unreviewed");
                } else {
                    println!("qualification: spectrum_normalization_rejected");
                    return Err(io::Error::other(format!(
                        "{} continuum spectrum sample(s) failed normalization; report was preserved",
                        result.report.failed_sample_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyPhotonMoments {
                selection,
                evaluations_directory,
                photon_inventory,
                moment_report,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report = EndfContinuumPhotonMomentReportDocument::from_path(&moment_report)?;
                report.verify_against_sources(&selection, &evaluations_directory, &inventory)?;
                println!(
                    "verified continuum photon moments {}",
                    moment_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!("reactions: {}", report.report.reaction_count);
                println!("incident-energy samples: {}", report.report.sample_count);
                println!(
                    "failed normalization samples: {}",
                    report.report.failed_sample_count
                );
                println!(
                    "qualification: {}",
                    if report.report.failed_sample_count == 0 {
                        "source_moments_checked_unreviewed"
                    } else {
                        "spectrum_normalization_rejected"
                    }
                );
            }
            NjoyCommand::ComparePhotonMoments {
                moment_report,
                receipt,
                execution_directory,
                relative_tolerance,
                output,
            } => {
                let moments = EndfContinuumPhotonMomentReportDocument::from_path(&moment_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let comparison = NjoyPhotonMomentComparison::compare(
                    &moments,
                    &execution,
                    &execution_directory,
                    relative_tolerance,
                )?;
                let result = comparison.write_new(&output)?;
                println!("compared independent photon moments with NJOY diagnostics");
                println!("comparison: {}", result.comparison_path.display());
                println!("comparison SHA-256: {}", result.comparison_sha256);
                println!(
                    "compared diagnostic samples: {}",
                    result.comparison.compared_sample_count
                );
                println!(
                    "uncompared independent samples: {}",
                    result.comparison.uncompared_independent_sample_count
                );
                println!(
                    "skipped processor-only samples: {}",
                    result.comparison.skipped_interpolated_sample_count
                );
                println!(
                    "maximum relative difference: {:.12e}",
                    result.comparison.maximum_relative_difference
                );
                if result.comparison.failed_sample_count == 0 {
                    println!("qualification: independent_moments_match_processor_print_unreviewed");
                } else {
                    println!("qualification: processor_print_mismatch_rejected");
                    return Err(io::Error::other(format!(
                        "{} photon-moment sample(s) disagree with NJOY diagnostics; comparison was preserved",
                        result.comparison.failed_sample_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyPhotonMomentComparison {
                moment_report,
                receipt,
                execution_directory,
                comparison_report,
            } => {
                let moments = EndfContinuumPhotonMomentReportDocument::from_path(&moment_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let comparison = NjoyPhotonMomentComparisonDocument::from_path(&comparison_report)?;
                comparison.verify_against_evidence(&moments, &execution, &execution_directory)?;
                println!(
                    "verified NJOY photon-moment comparison {}",
                    comparison_report.display()
                );
                println!("comparison SHA-256: {}", comparison.sha256);
                println!(
                    "compared diagnostic samples: {}",
                    comparison.comparison.compared_sample_count
                );
                println!(
                    "uncompared independent samples: {}",
                    comparison.comparison.uncompared_independent_sample_count
                );
                println!(
                    "skipped processor-only samples: {}",
                    comparison.comparison.skipped_interpolated_sample_count
                );
                println!(
                    "failed samples: {}",
                    comparison.comparison.failed_sample_count
                );
                println!(
                    "qualification: {}",
                    if comparison.comparison.failed_sample_count == 0 {
                        "independent_moments_match_processor_print_unreviewed"
                    } else {
                        "processor_print_mismatch_rejected"
                    }
                );
            }
            NjoyCommand::CalculateCapturePhotonBalance {
                selection,
                evaluations_directory,
                photon_inventory,
                nuclide,
                normalization_tolerance,
                relative_energy_tolerance,
                output,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report = EndfMf6CapturePhotonBalanceReport::calculate(
                    &selection,
                    &evaluations_directory,
                    &inventory,
                    &nuclide,
                    normalization_tolerance,
                    relative_energy_tolerance,
                )?;
                let result = report.write_new(&output)?;
                println!("calculated independent MF=6 capture photon balance");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!("nuclide: {}", result.report.nuclide);
                println!("incident-energy samples: {}", result.report.sample_count);
                println!(
                    "failed normalization samples: {}",
                    result.report.failed_normalization_sample_count
                );
                println!(
                    "failed energy-balance samples: {}",
                    result.report.failed_energy_balance_sample_count
                );
                println!(
                    "maximum absolute relative energy residual: {:.12e}",
                    result.report.maximum_absolute_relative_energy_residual
                );
                println!(
                    "qualification: {}",
                    qualification_name(result.report.qualification)
                );
                if result.report.failed_normalization_sample_count > 0
                    || result.report.failed_energy_balance_sample_count > 0
                    || result.report.sample_count == 0
                {
                    return Err(io::Error::other(format!(
                        "{} MF=6 capture photon source did not pass the independent screening gate; report was preserved",
                        result.report.nuclide
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyCapturePhotonBalance {
                selection,
                evaluations_directory,
                photon_inventory,
                balance_report,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report = EndfMf6CapturePhotonBalanceReportDocument::from_path(&balance_report)?;
                report.verify_against_sources(&selection, &evaluations_directory, &inventory)?;
                println!(
                    "verified MF=6 capture photon balance {}",
                    balance_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!("nuclide: {}", report.report.nuclide);
                println!("incident-energy samples: {}", report.report.sample_count);
                println!(
                    "failed normalization samples: {}",
                    report.report.failed_normalization_sample_count
                );
                println!(
                    "failed energy-balance samples: {}",
                    report.report.failed_energy_balance_sample_count
                );
                println!(
                    "qualification: {}",
                    qualification_name(report.report.qualification)
                );
            }
            NjoyCommand::CalculateLaw7ImplicitResidual {
                selection,
                evaluations_directory,
                photon_inventory,
                nuclide,
                normalization_tolerance,
                relative_energy_tolerance,
                output,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report = EndfMf6Law7ImplicitResidualReport::calculate(
                    &selection,
                    &evaluations_directory,
                    &inventory,
                    &nuclide,
                    normalization_tolerance,
                    relative_energy_tolerance,
                )?;
                let result = report.write_new(&output)?;
                println!("calculated deuterium MF=6/MT=16 LAW=7 implicit residual");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "source incident-energy nodes: {}",
                    result.report.source_incident_node_count
                );
                println!("active samples: {}", result.report.sample_count);
                println!(
                    "failed normalization samples: {}",
                    result.report.failed_normalization_sample_count
                );
                println!(
                    "failed residual-energy samples: {}",
                    result.report.failed_residual_energy_sample_count
                );
                println!(
                    "maximum absolute normalization error: {:.12e}",
                    result.report.maximum_absolute_normalization_error
                );
                println!(
                    "minimum implicit residual energy: {:.12e} eV",
                    result
                        .report
                        .samples
                        .iter()
                        .map(|sample| sample.implicit_residual_energy_ev)
                        .fold(f64::INFINITY, f64::min)
                );
                println!(
                    "qualification: {}",
                    law7_qualification_name(result.report.qualification)
                );
                if result.report.failed_normalization_sample_count > 0
                    || result.report.failed_residual_energy_sample_count > 0
                    || result.report.sample_count == 0
                {
                    return Err(io::Error::other(
                        "deuterium LAW=7 source did not pass the independent screening gate; report was preserved",
                    )
                    .into());
                }
            }
            NjoyCommand::VerifyLaw7ImplicitResidual {
                selection,
                evaluations_directory,
                photon_inventory,
                residual_report,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&residual_report)?;
                report.verify_against_sources(&selection, &evaluations_directory, &inventory)?;
                println!(
                    "verified deuterium LAW=7 implicit-residual report {}",
                    residual_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!("active samples: {}", report.report.sample_count);
                println!(
                    "failed normalization samples: {}",
                    report.report.failed_normalization_sample_count
                );
                println!(
                    "failed residual-energy samples: {}",
                    report.report.failed_residual_energy_sample_count
                );
                println!(
                    "qualification: {}",
                    law7_qualification_name(report.report.qualification)
                );
            }
            NjoyCommand::CompareLaw7ImplicitResidual {
                residual_report,
                receipt,
                execution_directory,
                source_relative_tolerance,
                print_relative_tolerance,
                output,
            } => {
                let residual =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&residual_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let comparison = NjoyLaw7ImplicitResidualComparison::compare(
                    &residual,
                    &execution,
                    &execution_directory,
                    source_relative_tolerance,
                    print_relative_tolerance,
                )?;
                let result = comparison.write_new(&output)?;
                println!("attributed deuterium LAW=7 processor diagnostics");
                println!("comparison: {}", result.comparison_path.display());
                println!("comparison SHA-256: {}", result.comparison_sha256);
                println!(
                    "shared source samples: {}",
                    result.comparison.shared_sample_count
                );
                println!(
                    "receipt violations attributed: {}/{}",
                    result.comparison.attributed_violation_count,
                    result.comparison.receipt_violation_count
                );
                println!("failed samples: {}", result.comparison.failed_sample_count);
                println!(
                    "maximum source/processor neutron-mean difference: {:.12e}",
                    result
                        .comparison
                        .maximum_source_neutron_mean_relative_difference
                );
                println!(
                    "maximum violation remainder/excess difference: {:.12e}",
                    result
                        .comparison
                        .maximum_violation_excess_relative_difference
                );
                println!(
                    "qualification: {}",
                    law7_comparison_qualification_name(result.comparison.qualification)
                );
                if result.comparison.qualification
                    != NjoyLaw7ImplicitResidualComparisonQualification::
                        ProcessorApproximationFullyAttributedUnreviewed
                {
                    return Err(io::Error::other(
                        "H-2 LAW=7 processor attribution did not pass; comparison was preserved",
                    )
                    .into());
                }
            }
            NjoyCommand::VerifyLaw7ImplicitResidualComparison {
                residual_report,
                receipt,
                execution_directory,
                comparison_report,
            } => {
                let residual =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&residual_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let comparison =
                    NjoyLaw7ImplicitResidualComparisonDocument::from_path(&comparison_report)?;
                comparison.verify_against_evidence(&residual, &execution, &execution_directory)?;
                println!(
                    "verified H-2 LAW=7 processor attribution {}",
                    comparison_report.display()
                );
                println!("comparison SHA-256: {}", comparison.sha256);
                println!(
                    "receipt violations attributed: {}/{}",
                    comparison.comparison.attributed_violation_count,
                    comparison.comparison.receipt_violation_count
                );
                println!(
                    "failed samples: {}",
                    comparison.comparison.failed_sample_count
                );
                println!(
                    "qualification: {}",
                    law7_comparison_qualification_name(comparison.comparison.qualification)
                );
            }
            NjoyCommand::AttributeEnergyBalance {
                domain_aware_report,
                receipt,
                execution_directory,
                nuclide,
                print_relative_tolerance,
                output,
            } => {
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let attribution = NjoyEnergyBalanceAttribution::attribute(
                    &domain,
                    &execution,
                    &execution_directory,
                    &nuclide,
                    print_relative_tolerance,
                )?;
                let result = attribution.write_new(&output)?;
                println!("attributed {nuclide} NJOY processor energy-balance accounting");
                println!("attribution: {}", result.attribution_path.display());
                println!("attribution SHA-256: {}", result.attribution_sha256);
                println!(
                    "in-domain findings attributed: {}/{}",
                    result.attribution.attributed_in_domain_violation_count,
                    result.attribution.in_domain_violation_count
                );
                println!(
                    "physical validations still required: {}",
                    result.attribution.physical_validation_required_count
                );
                println!(
                    "waived findings: {}",
                    result.attribution.waived_violation_count
                );
                println!(
                    "maximum printed-remainder/final-excess difference: {:.12e}",
                    result
                        .attribution
                        .maximum_remainder_excess_relative_difference
                );
                println!(
                    "qualification: {}",
                    energy_balance_attribution_qualification_name(result.attribution.qualification)
                );
                if result.attribution.qualification
                    != NjoyEnergyBalanceAttributionQualification::
                        ProcessorAccountingMechanismAttributedPhysicalValidationRequired
                {
                    return Err(io::Error::other(
                        "NJOY energy-balance accounting was not fully attributed; report was preserved",
                    )
                    .into());
                }
            }
            NjoyCommand::VerifyEnergyBalanceAttribution {
                domain_aware_report,
                receipt,
                execution_directory,
                attribution_report,
            } => {
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let attribution =
                    NjoyEnergyBalanceAttributionDocument::from_path(&attribution_report)?;
                attribution.verify_against_evidence(&domain, &execution, &execution_directory)?;
                println!(
                    "verified processor-only energy-balance attribution {}",
                    attribution_report.display()
                );
                println!("attribution SHA-256: {}", attribution.sha256);
                println!(
                    "in-domain findings attributed: {}/{}",
                    attribution.attribution.attributed_in_domain_violation_count,
                    attribution.attribution.in_domain_violation_count
                );
                println!(
                    "physical validations still required: {}",
                    attribution.attribution.physical_validation_required_count
                );
                println!(
                    "qualification: {}",
                    energy_balance_attribution_qualification_name(
                        attribution.attribution.qualification
                    )
                );
            }
            NjoyCommand::CalculateReactionEnergyBalance {
                selection,
                evaluations_directory,
                attribution_report,
                domain_aware_report,
                receipt,
                execution_directory,
                nuclide,
                relative_tolerance,
                output,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let attribution =
                    NjoyEnergyBalanceAttributionDocument::from_path(&attribution_report)?;
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let report = EndfReactionEnergyBalanceReport::calculate(
                    &selection,
                    &evaluations_directory,
                    &attribution,
                    &domain,
                    &execution,
                    &execution_directory,
                    &nuclide,
                    relative_tolerance,
                )?;
                let result = report.write_new(&output)?;
                println!("integrated {nuclide} File 6 reaction energy balances from source");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "samples computed: {}/{} ({} partially computable)",
                    result.report.computed_sample_count,
                    result.report.sample_count,
                    result.report.partially_computed_sample_count
                );
                println!(
                    "independent/printed remainders matched: {}/{}",
                    result.report.remainder_matched_sample_count,
                    result.report.computed_sample_count
                );
                println!(
                    "maximum remainder relative difference: {:.12e}",
                    result.report.maximum_remainder_relative_difference
                );
                println!(
                    "maximum product ebar relative difference: {:.12e} over {} comparisons",
                    result.report.maximum_ebar_relative_difference,
                    result.report.ebar_comparison_count
                );
                println!(
                    "qualification: {}",
                    reaction_balance_qualification_name(result.report.qualification)
                );
            }
            NjoyCommand::VerifyReactionEnergyBalance {
                selection,
                evaluations_directory,
                attribution_report,
                domain_aware_report,
                receipt,
                execution_directory,
                nuclide,
                balance_report,
            } => {
                let selection = EvaluatedNeutronSourceSelectionDocument::from_path(&selection)?;
                let attribution =
                    NjoyEnergyBalanceAttributionDocument::from_path(&attribution_report)?;
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let balance = EndfReactionEnergyBalanceDocument::from_path(&balance_report)?;
                balance.verify_against_sources(
                    &selection,
                    &evaluations_directory,
                    &attribution,
                    &domain,
                    &execution,
                    &execution_directory,
                    &nuclide,
                )?;
                println!(
                    "verified independent reaction energy-balance report {}",
                    balance_report.display()
                );
                println!("report SHA-256: {}", balance.sha256);
                println!(
                    "samples computed: {}/{}",
                    balance.report.computed_sample_count, balance.report.sample_count
                );
                println!(
                    "independent/printed remainders matched: {}/{}",
                    balance.report.remainder_matched_sample_count,
                    balance.report.computed_sample_count
                );
                println!(
                    "qualification: {}",
                    reaction_balance_qualification_name(balance.report.qualification)
                );
            }
            NjoyCommand::GenerateResponseTables {
                material,
                component_profile,
                generation_method,
                nuclear_data_manifest,
                transport_domain,
                selection,
                domain_aware_report,
                receipt,
                execution_directory,
                response_set_output,
                report_output,
            } => {
                let artifacts = ResponseTableArtifacts::load(
                    &material,
                    &component_profile,
                    &generation_method,
                    &nuclear_data_manifest,
                    &transport_domain,
                    &selection,
                    &domain_aware_report,
                    &receipt,
                    execution_directory,
                )?;
                let inputs = artifacts.inputs();
                let case_id = &artifacts.execution.receipt.case_id;
                let generation = NjoyResponseTableGeneration::generate(
                    &inputs,
                    &format!("openbnct.{case_id}.neutron-response-set.v1"),
                    &format!("openbnct.{case_id}.response-table-generation.v1"),
                )?;
                let result = generation.write_new(&response_set_output, &report_output)?;
                println!("generated neutron response set from production-HEATR PENDF tables");
                println!("response set: {}", result.response_set_path.display());
                println!("response set SHA-256: {}", result.response_set_sha256);
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "union grid knots: {}",
                    result.generation.report.union_grid_knot_count
                );
                println!(
                    "carried in-domain kinematic violations: {}",
                    result
                        .generation
                        .report
                        .carried_findings
                        .in_domain_kinematic_violation_count
                );
                println!("qualification: tables_generated_unreviewed");
            }
            NjoyCommand::VerifyResponseTables {
                material,
                component_profile,
                generation_method,
                nuclear_data_manifest,
                transport_domain,
                selection,
                domain_aware_report,
                receipt,
                execution_directory,
                response_set,
                generation_report,
                review_output,
                reviewed_set_output,
            } => {
                let artifacts = ResponseTableArtifacts::load(
                    &material,
                    &component_profile,
                    &generation_method,
                    &nuclear_data_manifest,
                    &transport_domain,
                    &selection,
                    &domain_aware_report,
                    &receipt,
                    execution_directory,
                )?;
                let inputs = artifacts.inputs();
                let (set, set_bytes) = load_response_set(&response_set)?;
                let (report, report_bytes) = load_generation_report(&generation_report)?;
                let case_id = &artifacts.execution.receipt.case_id;
                let (review, reviewed_set) = NjoyResponseSetReviewReport::verify(
                    &inputs,
                    &set,
                    &set_bytes,
                    &report,
                    &report_bytes,
                    &format!("openbnct.{case_id}.response-set-review.v1"),
                )?;
                let result = NjoyResponseSetReviewDocument::write_new(
                    &review,
                    &reviewed_set,
                    &review_output,
                    &reviewed_set_output,
                )?;
                println!("verified response set by deterministic regeneration");
                println!("review: {}", result.review_path.display());
                println!("review SHA-256: {}", result.document.review_sha256);
                println!(
                    "reviewed response set: {}",
                    result.reviewed_set_path.display()
                );
                println!(
                    "reviewed set SHA-256: {}",
                    result.document.reviewed_set_sha256
                );
                println!(
                    "qualification: independently_reviewed (in-house deterministic verification)"
                );
            }
            NjoyCommand::CompareCapturePhotonMoments {
                balance_report,
                receipt,
                execution_directory,
                relative_tolerance,
                output,
            } => {
                let balance =
                    EndfMf6CapturePhotonBalanceReportDocument::from_path(&balance_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let comparison = NjoyCapturePhotonMomentComparison::compare(
                    &balance,
                    &execution,
                    &execution_directory,
                    relative_tolerance,
                )?;
                let result = comparison.write_new(&output)?;
                println!("compared independent capture moments with NJOY diagnostics");
                println!("comparison: {}", result.comparison_path.display());
                println!("comparison SHA-256: {}", result.comparison_sha256);
                println!(
                    "compared diagnostic samples: {}",
                    result.comparison.compared_sample_count
                );
                println!(
                    "uncompared independent samples: {}",
                    result.comparison.uncompared_independent_sample_count
                );
                println!(
                    "skipped processor-only samples: {}",
                    result.comparison.skipped_processor_sample_count
                );
                println!(
                    "maximum relative difference: {:.12e}",
                    result.comparison.maximum_relative_difference
                );
                if result.comparison.failed_sample_count == 0 {
                    println!(
                        "qualification: independent_capture_moments_match_processor_print_unreviewed"
                    );
                } else {
                    println!("qualification: processor_capture_print_mismatch_rejected");
                    return Err(io::Error::other(format!(
                        "{} capture-moment sample(s) disagree with NJOY diagnostics; comparison was preserved",
                        result.comparison.failed_sample_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyCapturePhotonMomentComparison {
                balance_report,
                receipt,
                execution_directory,
                comparison_report,
            } => {
                let balance =
                    EndfMf6CapturePhotonBalanceReportDocument::from_path(&balance_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let comparison =
                    NjoyCapturePhotonMomentComparisonDocument::from_path(&comparison_report)?;
                comparison.verify_against_evidence(&balance, &execution, &execution_directory)?;
                println!(
                    "verified NJOY capture-moment comparison {}",
                    comparison_report.display()
                );
                println!("comparison SHA-256: {}", comparison.sha256);
                println!(
                    "compared diagnostic samples: {}",
                    comparison.comparison.compared_sample_count
                );
                println!(
                    "failed samples: {}",
                    comparison.comparison.failed_sample_count
                );
                println!(
                    "qualification: {}",
                    if comparison.comparison.failed_sample_count == 0 {
                        "independent_capture_moments_match_processor_print_unreviewed"
                    } else {
                        "processor_capture_print_mismatch_rejected"
                    }
                );
            }
            NjoyCommand::Execute {
                selection,
                material,
                generation_method,
                profile,
                receipt,
                evaluations_directory,
                input_bundle,
                njoy_executable,
                processor_support_artifacts,
                timeout_seconds,
                output,
            } => {
                let selection_json = fs::read(selection)?;
                let material_json = fs::read(material)?;
                let generation_method_json = fs::read(generation_method)?;
                let acquisition_bytes = acquisition_pairs(&profile, &receipt)?
                    .iter()
                    .map(|(profile, receipt)| {
                        Ok::<_, io::Error>((fs::read(profile)?, fs::read(receipt)?))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let acquisitions = acquisition_bytes
                    .iter()
                    .map(|(profile, receipt)| NjoyAcquisitionArtifacts {
                        profile_json: profile,
                        receipt_json: receipt,
                    })
                    .collect();
                let bundle = NjoyInputBundle::generate(
                    &evaluations_directory,
                    NjoyInputArtifacts {
                        evaluated_source_selection_json: &selection_json,
                        material_json: &material_json,
                        generation_method_json: &generation_method_json,
                        acquisitions,
                    },
                )?;
                let result = NjoyExecutionReceipt::execute(
                    &bundle,
                    NjoyExecutionOptions {
                        executable: &njoy_executable,
                        processor_support_artifacts: &processor_support_artifacts,
                        input_bundle_root: &input_bundle,
                        evaluations_root: &evaluations_directory,
                        output_root: &output,
                        timeout_seconds,
                    },
                )?;
                println!("executed NJOY2016.78 at {}", output.display());
                println!("nuclide runs: {}", result.receipt.runs.len());
                println!(
                    "processor SHA-256: {}",
                    result.receipt.processor.executable.sha256
                );
                println!("receipt: {}", result.receipt_path.display());
                println!("receipt SHA-256: {}", result.receipt_sha256);
                if result.receipt.rejected_run_count == 0 {
                    println!("qualification: execution_observed_unreviewed");
                } else {
                    println!("qualification: execution_observed_diagnostics_failed");
                    println!(
                        "rejected nuclide runs: {}",
                        result.receipt.rejected_run_count
                    );
                    return Err(io::Error::other(format!(
                        "{} NJOY run(s) exceeded kinematic diagnostic limits; receipt was preserved",
                        result.receipt.rejected_run_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyExecution {
                receipt,
                execution_directory,
            } => {
                let document = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                document.verify_execution_root(&execution_directory)?;
                println!(
                    "verified NJOY execution artifacts at {}",
                    execution_directory.display()
                );
                println!("receipt SHA-256: {}", document.sha256);
                println!("nuclide runs: {}", document.receipt.runs.len());
                println!(
                    "rejected nuclide runs: {}",
                    document.receipt.rejected_run_count
                );
                println!(
                    "qualification: {}",
                    if document.receipt.rejected_run_count == 0 {
                        "execution_observed_unreviewed"
                    } else {
                        "execution_observed_diagnostics_failed"
                    }
                );
            }
            NjoyCommand::AssessExecution {
                receipt,
                execution_directory,
                output,
            } => {
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let report = NjoySuitabilityReport::assess(&execution, &execution_directory)?;
                let result = report.write_new(&output)?;
                println!("assessed transported-photon KERMA suitability");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!("nuclide runs: {}", result.report.runs.len());
                println!(
                    "rejected nuclide runs: {}",
                    result.report.rejected_run_count
                );
                println!(
                    "processor data findings: {} unique / {} occurrences",
                    result.report.processor_finding_count,
                    result.report.processor_finding_occurrence_count
                );
                println!(
                    "kinematic violations: {}",
                    result.report.kinematic_violation_count
                );
                if result.report.rejected_run_count == 0 {
                    println!("qualification: transported_photon_kerma_candidate_unreviewed");
                } else {
                    println!("qualification: transported_photon_kerma_rejected");
                    return Err(io::Error::other(format!(
                        "{} NJOY run(s) are unsuitable for transported-photon KERMA; report was preserved",
                        result.report.rejected_run_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifySuitability {
                receipt,
                execution_directory,
                suitability_report,
            } => {
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let report = NjoySuitabilityReportDocument::from_path(&suitability_report)?;
                report.verify_against_execution(&execution, &execution_directory)?;
                println!(
                    "verified transported-photon suitability report {}",
                    suitability_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!("nuclide runs: {}", report.report.runs.len());
                println!(
                    "rejected nuclide runs: {}",
                    report.report.rejected_run_count
                );
                println!(
                    "qualification: {}",
                    if report.report.rejected_run_count == 0 {
                        "transported_photon_kerma_candidate_unreviewed"
                    } else {
                        "transported_photon_kerma_rejected"
                    }
                );
            }
            NjoyCommand::AssessSourceAware {
                legacy_report,
                receipt,
                execution_directory,
                input_manifest,
                photon_inventory,
                output,
            } => {
                let legacy = NjoySuitabilityReportDocument::from_path(&legacy_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let input_manifest = fs::read(input_manifest)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report = NjoySourceAwareSuitabilityReport::assess(
                    &legacy,
                    &execution,
                    &execution_directory,
                    &input_manifest,
                    &inventory,
                )?;
                let result = report.write_new(&output)?;
                println!("assessed source-aware transported-photon KERMA suitability");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "rejected nuclide runs: {}",
                    result.report.rejected_run_count
                );
                println!(
                    "processor findings: {} rejecting / {} informational",
                    result.report.rejecting_processor_finding_count,
                    result.report.informational_processor_finding_count
                );
                println!(
                    "source format findings: {}",
                    result.report.source_format_finding_count
                );
                if result.report.rejected_run_count == 0 {
                    println!("qualification: transported_photon_kerma_candidate_unreviewed");
                } else {
                    println!("qualification: transported_photon_kerma_rejected");
                    return Err(io::Error::other(format!(
                        "{} NJOY run(s) remain unsuitable after source-aware interpretation; report was preserved",
                        result.report.rejected_run_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifySourceAware {
                legacy_report,
                receipt,
                execution_directory,
                input_manifest,
                photon_inventory,
                source_aware_report,
            } => {
                let legacy = NjoySuitabilityReportDocument::from_path(&legacy_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                let input_manifest = fs::read(input_manifest)?;
                let inventory =
                    EndfPhotonProductionInventoryDocument::from_path(&photon_inventory)?;
                let report =
                    NjoySourceAwareSuitabilityReportDocument::from_path(&source_aware_report)?;
                report.verify_against_evidence(
                    &legacy,
                    &execution,
                    &execution_directory,
                    &input_manifest,
                    &inventory,
                )?;
                println!(
                    "verified source-aware suitability report {}",
                    source_aware_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!(
                    "rejected nuclide runs: {}",
                    report.report.rejected_run_count
                );
                println!(
                    "informational File 13 findings: {}",
                    report.report.informational_processor_finding_count
                );
                println!(
                    "qualification: {}",
                    if report.report.rejected_run_count == 0 {
                        "transported_photon_kerma_candidate_unreviewed"
                    } else {
                        "transported_photon_kerma_rejected"
                    }
                );
            }
            NjoyCommand::AssessDomainAware {
                source_aware_report,
                legacy_report,
                receipt,
                execution_directory,
                input_manifest,
                nuclear_data_manifest,
                material,
                transport_domain,
                output,
            } => {
                let source_aware =
                    NjoySourceAwareSuitabilityReportDocument::from_path(&source_aware_report)?;
                let legacy = NjoySuitabilityReportDocument::from_path(&legacy_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                legacy.verify_against_execution(&execution, &execution_directory)?;
                let input_manifest = fs::read(input_manifest)?;
                let nuclear_data_manifest = fs::read(nuclear_data_manifest)?;
                let material = fs::read(material)?;
                let transport_domain =
                    OpenMcNeutronTransportDomainDocument::from_path(&transport_domain)?;
                let report = NjoyDomainAwareSuitabilityReport::assess(
                    &source_aware,
                    &legacy,
                    &execution,
                    &input_manifest,
                    &nuclear_data_manifest,
                    &material,
                    &transport_domain,
                )?;
                let result = report.write_new(&output)?;
                println!("assessed transport-domain-aware suitability");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "kinematic violations: {} full / {} in-domain / {} out-of-domain",
                    result.report.full_evaluation_kinematic_violation_count,
                    result.report.in_domain_kinematic_violation_count,
                    result.report.out_of_domain_kinematic_violation_count
                );
                println!(
                    "reclassified nuclide runs: {}",
                    result.report.reclassified_run_count
                );
                println!(
                    "rejected nuclide runs: {}",
                    result.report.rejected_run_count
                );
                if result.report.rejected_run_count == 0 {
                    println!("qualification: transported_photon_kerma_candidate_unreviewed");
                } else {
                    println!("qualification: transported_photon_kerma_rejected");
                    return Err(io::Error::other(format!(
                        "{} NJOY run(s) remain unsuitable in the bound transport domain; report was preserved",
                        result.report.rejected_run_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyDomainAware {
                source_aware_report,
                legacy_report,
                receipt,
                execution_directory,
                input_manifest,
                nuclear_data_manifest,
                material,
                transport_domain,
                domain_aware_report,
            } => {
                let source_aware =
                    NjoySourceAwareSuitabilityReportDocument::from_path(&source_aware_report)?;
                let legacy = NjoySuitabilityReportDocument::from_path(&legacy_report)?;
                let execution = NjoyExecutionReceiptDocument::from_path(&receipt)?;
                legacy.verify_against_execution(&execution, &execution_directory)?;
                let input_manifest = fs::read(input_manifest)?;
                let nuclear_data_manifest = fs::read(nuclear_data_manifest)?;
                let material = fs::read(material)?;
                let transport_domain =
                    OpenMcNeutronTransportDomainDocument::from_path(&transport_domain)?;
                let report =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                report.verify_against_evidence(
                    &source_aware,
                    &legacy,
                    &execution,
                    &input_manifest,
                    &nuclear_data_manifest,
                    &material,
                    &transport_domain,
                )?;
                println!(
                    "verified domain-aware suitability report {}",
                    domain_aware_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!(
                    "kinematic violations: {} full / {} in-domain / {} out-of-domain",
                    report.report.full_evaluation_kinematic_violation_count,
                    report.report.in_domain_kinematic_violation_count,
                    report.report.out_of_domain_kinematic_violation_count
                );
                println!(
                    "reclassified nuclide runs: {}",
                    report.report.reclassified_run_count
                );
                println!(
                    "qualification: {}",
                    if report.report.rejected_run_count == 0 {
                        "transported_photon_kerma_candidate_unreviewed"
                    } else {
                        "transported_photon_kerma_rejected"
                    }
                );
            }
            NjoyCommand::AssessEvidenceAware {
                domain_aware_report,
                law7_residual_report,
                law7_comparison_report,
                capture_balance_report,
                capture_comparison_report,
                output,
            } => {
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let law7_residual =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&law7_residual_report)?;
                let law7_comparison =
                    NjoyLaw7ImplicitResidualComparisonDocument::from_path(&law7_comparison_report)?;
                let capture_balance =
                    EndfMf6CapturePhotonBalanceReportDocument::from_path(&capture_balance_report)?;
                let capture_comparison = NjoyCapturePhotonMomentComparisonDocument::from_path(
                    &capture_comparison_report,
                )?;
                let report = NjoyEvidenceAwareSuitabilityReport::assess(
                    &domain,
                    &law7_residual,
                    &law7_comparison,
                    &capture_balance,
                    &capture_comparison,
                )?;
                let result = report.write_new(&output)?;
                println!("assessed reaction-evidence-aware v0.4 suitability");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "kinematic violations: {} in-domain / {} approximation-attributed / {} remaining",
                    result.report.domain_in_scope_kinematic_violation_count,
                    result
                        .report
                        .approximation_attributed_in_domain_violation_count,
                    result.report.remaining_in_domain_kinematic_violation_count
                );
                println!(
                    "domain-status transitions: {} cleared / {} independently rejected",
                    result.report.reclassified_from_domain_run_count,
                    result.report.independently_rejected_from_domain_run_count
                );
                println!(
                    "rejected nuclide runs: {}",
                    result.report.rejected_run_count
                );
                if result.report.rejected_run_count == 0 {
                    println!("qualification: transported_photon_kerma_candidate_unreviewed");
                } else {
                    println!("qualification: transported_photon_kerma_rejected");
                    return Err(io::Error::other(format!(
                        "{} nuclide run(s) remain unsuitable after reaction-level evidence; report was preserved",
                        result.report.rejected_run_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyEvidenceAware {
                domain_aware_report,
                law7_residual_report,
                law7_comparison_report,
                capture_balance_report,
                capture_comparison_report,
                evidence_aware_report,
            } => {
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let law7_residual =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&law7_residual_report)?;
                let law7_comparison =
                    NjoyLaw7ImplicitResidualComparisonDocument::from_path(&law7_comparison_report)?;
                let capture_balance =
                    EndfMf6CapturePhotonBalanceReportDocument::from_path(&capture_balance_report)?;
                let capture_comparison = NjoyCapturePhotonMomentComparisonDocument::from_path(
                    &capture_comparison_report,
                )?;
                let report =
                    NjoyEvidenceAwareSuitabilityReportDocument::from_path(&evidence_aware_report)?;
                report.verify_against_evidence(
                    &domain,
                    &law7_residual,
                    &law7_comparison,
                    &capture_balance,
                    &capture_comparison,
                )?;
                println!(
                    "verified reaction-evidence-aware v0.4 suitability {}",
                    evidence_aware_report.display()
                );
                println!("report SHA-256: {}", report.sha256);
                println!(
                    "kinematic violations: {} in-domain / {} approximation-attributed / {} remaining",
                    report.report.domain_in_scope_kinematic_violation_count,
                    report
                        .report
                        .approximation_attributed_in_domain_violation_count,
                    report.report.remaining_in_domain_kinematic_violation_count
                );
                println!(
                    "rejected nuclide runs: {}",
                    report.report.rejected_run_count
                );
                println!(
                    "qualification: {}",
                    if report.report.rejected_run_count == 0 {
                        "transported_photon_kerma_candidate_unreviewed"
                    } else {
                        "transported_photon_kerma_rejected"
                    }
                );
            }
            NjoyCommand::AssessDiagnosticTriage {
                evidence_aware_report,
                domain_aware_report,
                output,
            } => {
                let evidence =
                    NjoyEvidenceAwareSuitabilityReportDocument::from_path(&evidence_aware_report)?;
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let report = NjoyDiagnosticTriageReport::assess(&evidence, &domain)?;
                let result = report.write_new(&output)?;
                println!("triaged remaining in-domain NJOY diagnostics");
                println!("report: {}", result.report_path.display());
                println!("report SHA-256: {}", result.report_sha256);
                println!(
                    "remaining findings: {} original / {} source-data-blocked / {} requiring independent diagnostics",
                    result
                        .report
                        .original_remaining_in_domain_kinematic_violation_count,
                    result
                        .report
                        .source_data_blocked_in_domain_kinematic_violation_count,
                    result
                        .report
                        .independent_diagnostic_required_in_domain_kinematic_violation_count
                );
                if result
                    .report
                    .independent_diagnostic_required_in_domain_kinematic_violation_count
                    > 0
                {
                    return Err(io::Error::other(format!(
                        "{} in-domain finding(s) still require independent reaction diagnostics; triage report was preserved",
                        result
                            .report
                            .independent_diagnostic_required_in_domain_kinematic_violation_count
                    ))
                    .into());
                }
            }
            NjoyCommand::VerifyDiagnosticTriage {
                evidence_aware_report,
                domain_aware_report,
                triage_report,
            } => {
                let evidence =
                    NjoyEvidenceAwareSuitabilityReportDocument::from_path(&evidence_aware_report)?;
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let triage = NjoyDiagnosticTriageReportDocument::from_path(&triage_report)?;
                triage.verify_against_evidence(&evidence, &domain)?;
                println!(
                    "verified NJOY diagnostic triage {}",
                    triage_report.display()
                );
                println!("report SHA-256: {}", triage.sha256);
                println!(
                    "remaining findings: {} original / {} source-data-blocked / {} requiring independent diagnostics",
                    triage
                        .report
                        .original_remaining_in_domain_kinematic_violation_count,
                    triage
                        .report
                        .source_data_blocked_in_domain_kinematic_violation_count,
                    triage
                        .report
                        .independent_diagnostic_required_in_domain_kinematic_violation_count
                );
            }
            NjoyCommand::CheckDiagnosticTriage {
                domain_aware_report,
                law7_residual_report,
                law7_comparison_report,
                capture_balance_report,
                capture_comparison_report,
                evidence_aware_report,
                triage_report,
                output,
            } => {
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let law7_residual =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&law7_residual_report)?;
                let law7_comparison =
                    NjoyLaw7ImplicitResidualComparisonDocument::from_path(&law7_comparison_report)?;
                let capture_balance =
                    EndfMf6CapturePhotonBalanceReportDocument::from_path(&capture_balance_report)?;
                let capture_comparison = NjoyCapturePhotonMomentComparisonDocument::from_path(
                    &capture_comparison_report,
                )?;
                let evidence =
                    NjoyEvidenceAwareSuitabilityReportDocument::from_path(&evidence_aware_report)?;
                let triage = NjoyDiagnosticTriageReportDocument::from_path(&triage_report)?;
                let result = NjoyDiagnosticTriageCheckResult::verify_and_build(
                    &triage,
                    &evidence,
                    &domain,
                    &law7_residual,
                    &law7_comparison,
                    &capture_balance,
                    &capture_comparison,
                )?;
                result.write_new(&output)?;
                println!("verified diagnostic-triage chain and wrote machine check");
                println!("result: {}", output.display());
                println!(
                    "response qualification: {}",
                    match result.response_qualification {
                        NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed =>
                            "transported_photon_kerma_candidate_unreviewed",
                        NjoySuitabilityQualification::TransportedPhotonKermaRejected =>
                            "transported_photon_kerma_rejected",
                    }
                );
                println!(
                    "remaining findings: {} original / {} source-data-blocked / {} requiring independent diagnostics",
                    result.original_remaining_in_domain_kinematic_violation_count,
                    result.source_data_blocked_in_domain_kinematic_violation_count,
                    result.independent_diagnostic_required_in_domain_kinematic_violation_count
                );
            }
            NjoyCommand::CheckEvidenceAware {
                domain_aware_report,
                law7_residual_report,
                law7_comparison_report,
                capture_balance_report,
                capture_comparison_report,
                evidence_aware_report,
                output,
            } => {
                let domain =
                    NjoyDomainAwareSuitabilityReportDocument::from_path(&domain_aware_report)?;
                let law7_residual =
                    EndfMf6Law7ImplicitResidualReportDocument::from_path(&law7_residual_report)?;
                let law7_comparison =
                    NjoyLaw7ImplicitResidualComparisonDocument::from_path(&law7_comparison_report)?;
                let capture_balance =
                    EndfMf6CapturePhotonBalanceReportDocument::from_path(&capture_balance_report)?;
                let capture_comparison = NjoyCapturePhotonMomentComparisonDocument::from_path(
                    &capture_comparison_report,
                )?;
                let report =
                    NjoyEvidenceAwareSuitabilityReportDocument::from_path(&evidence_aware_report)?;
                let result = NjoyEvidenceAwareCheckResult::verify_and_build(
                    &report,
                    &domain,
                    &law7_residual,
                    &law7_comparison,
                    &capture_balance,
                    &capture_comparison,
                )?;
                result.write_new(&output)?;
                println!("verified evidence-aware suitability and wrote machine check");
                println!("result: {}", output.display());
                println!(
                    "qualification: {}",
                    match result.qualification {
                        NjoySuitabilityQualification::TransportedPhotonKermaCandidateUnreviewed =>
                            "transported_photon_kerma_candidate_unreviewed",
                        NjoySuitabilityQualification::TransportedPhotonKermaRejected =>
                            "transported_photon_kerma_rejected",
                    }
                );
                println!(
                    "remaining in-domain kinematic violations: {}",
                    result.remaining_in_domain_kinematic_violation_count
                );
            }
            NjoyCommand::CompareSuitability {
                baseline_report,
                candidate_report,
                output,
            } => {
                let baseline = NjoySuitabilityReportDocument::from_path(&baseline_report)?;
                let candidate = NjoySuitabilityReportDocument::from_path(&candidate_report)?;
                let comparison = NjoySuitabilityComparison::compare(&baseline, &candidate)?;
                let result = comparison.write_new(&output)?;
                println!("compared response-treatment candidate with rejected baseline");
                println!("comparison: {}", result.comparison_path.display());
                println!("comparison SHA-256: {}", result.comparison_sha256);
                println!(
                    "rejected nuclide runs: baseline={} candidate={}",
                    result.comparison.baseline_rejected_run_count,
                    result.comparison.candidate_rejected_run_count
                );
                println!(
                    "baseline rejections resolved: {}",
                    result.comparison.resolved_baseline_rejection_count
                );
                println!(
                    "new candidate rejections: {}",
                    result.comparison.introduced_rejection_count
                );
                println!(
                    "kinematic violations: baseline={} candidate={}",
                    result.comparison.baseline_kinematic_violation_count,
                    result.comparison.candidate_kinematic_violation_count
                );
                match result.comparison.qualification {
                    NjoySuitabilityComparisonQualification::CandidateRejected => {
                        println!("qualification: candidate_rejected");
                        return Err(io::Error::other(format!(
                            "candidate retains {} rejected nuclide run(s); comparison was preserved",
                            result.comparison.candidate_rejected_run_count
                        ))
                        .into());
                    }
                    NjoySuitabilityComparisonQualification::CandidateMechanicalGateClearUnreviewed => {
                        println!("qualification: candidate_mechanical_gate_clear_unreviewed");
                    }
                }
            }
            NjoyCommand::VerifyComparison {
                baseline_report,
                candidate_report,
                comparison_report,
            } => {
                let baseline = NjoySuitabilityReportDocument::from_path(&baseline_report)?;
                let candidate = NjoySuitabilityReportDocument::from_path(&candidate_report)?;
                let comparison = NjoySuitabilityComparisonDocument::from_path(&comparison_report)?;
                comparison.verify_against_reports(&baseline, &candidate)?;
                println!(
                    "verified response-treatment comparison {}",
                    comparison_report.display()
                );
                println!("comparison SHA-256: {}", comparison.sha256);
                println!(
                    "rejected nuclide runs: baseline={} candidate={}",
                    comparison.comparison.baseline_rejected_run_count,
                    comparison.comparison.candidate_rejected_run_count
                );
                println!(
                    "qualification: {}",
                    match comparison.comparison.qualification {
                        NjoySuitabilityComparisonQualification::CandidateRejected =>
                            "candidate_rejected",
                        NjoySuitabilityComparisonQualification::CandidateMechanicalGateClearUnreviewed =>
                            "candidate_mechanical_gate_clear_unreviewed",
                    }
                );
            }
            NjoyCommand::CheckCandidateComparison {
                baseline_report,
                candidate_report,
                comparison_report,
                output,
            } => {
                let baseline = NjoySuitabilityReportDocument::from_path(&baseline_report)?;
                let candidate = NjoySuitabilityReportDocument::from_path(&candidate_report)?;
                let comparison = NjoySuitabilityComparisonDocument::from_path(&comparison_report)?;
                let result = NjoyCandidateComparisonCheckResult::verify_and_build(
                    &comparison,
                    &baseline,
                    &candidate,
                )?;
                result.write_new(&output)?;
                println!("verified candidate comparison and wrote machine check");
                println!("result: {}", output.display());
                println!(
                    "candidate qualification: {}",
                    match result.candidate_qualification {
                        NjoySuitabilityComparisonQualification::CandidateRejected =>
                            "candidate_rejected",
                        NjoySuitabilityComparisonQualification::CandidateMechanicalGateClearUnreviewed =>
                            "candidate_mechanical_gate_clear_unreviewed",
                    }
                );
                println!(
                    "rejected nuclide runs: baseline={} candidate={} resolved={} introduced={}",
                    result.baseline_rejected_run_count,
                    result.candidate_rejected_run_count,
                    result.resolved_baseline_rejection_count,
                    result.introduced_rejection_count
                );
            }
        },
        Some(Command::Bio(args)) => match args.command {
            BioCommand::Apply {
                model,
                physical_bundle,
                region_masks,
                spectra,
                output,
            } => {
                let model_bytes = fs::read(&model)?;
                let peek: serde_json::Value = serde_json::from_slice(&model_bytes)?;
                let schema = peek
                    .get("schema_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let bundle_bytes = fs::read(&physical_bundle)?;
                let physical: PhysicalDoseBundle = serde_json::from_slice(&bundle_bytes)?;
                let mut masks = Vec::new();
                for pair in &region_masks {
                    let (name, path) = pair.split_once('=').ok_or_else(|| {
                        io::Error::other(format!(
                            "region mask {pair:?} must be written as name=path"
                        ))
                    })?;
                    let mask: RegionMask = serde_json::from_slice(&fs::read(path)?)?;
                    if mask.name != name {
                        return Err(io::Error::other(format!(
                            "region mask {path} is named {:?}, expected {name:?}",
                            mask.name
                        ))
                        .into());
                    }
                    masks.push(mask);
                }
                let bundle = if openbnct_core::schema_matches(
                    &schema,
                    openbnct_bio::MICRODOSIMETRIC_MODEL_SCHEMA,
                ) {
                    let model: MicrodosimetricModel = serde_json::from_slice(&model_bytes)?;
                    let mut spectrum_docs = Vec::with_capacity(spectra.len());
                    for path in &spectra {
                        let bytes = fs::read(path)?;
                        let spectrum: LinealSpectrum =
                            serde_json::from_slice(&bytes).map_err(|e| {
                                io::Error::other(format!("spectrum {}: {e}", path.display()))
                            })?;
                        spectrum_docs.push((spectrum, bytes));
                    }
                    let inputs: Vec<SpectrumInput<'_>> = spectrum_docs
                        .iter()
                        .map(|(spectrum, bytes)| SpectrumInput {
                            spectrum,
                            document_bytes: bytes,
                        })
                        .collect();
                    apply_microdosimetric_model(&model, &model_bytes, &physical, &masks, &inputs)?
                } else {
                    if !spectra.is_empty() {
                        return Err(io::Error::other(
                            "--spectrum applies only to openbnct.microdosimetric-model/0.1.0 models",
                        )
                        .into());
                    }
                    let model: BiologicalModel = serde_json::from_slice(&model_bytes)?;
                    apply_biological_model(&model, &model_bytes, &physical, &masks)?
                };
                let json = serde_json::to_vec_pretty(&bundle)?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                file.write_all(&json)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                println!("biological dose bundle at {}", output.display());
                println!("model: {} sha256:{}", bundle.model.id, bundle.model.sha256);
                println!("semantics: {:?}", bundle.weight_semantics);
                println!("physical provenance: {}", bundle.physical_bundle_provenance);
                println!("regions applied: {}", bundle.regions_applied.join(","));
                if let Some(mkm) = &bundle.microdosimetry {
                    for (component, y) in &mkm.dose_mean_lineal_energy_kev_um {
                        println!("resolved ȳ_D[{component}]: {y:.6} keV/µm");
                    }
                    for applied in &mkm.spectra_applied {
                        println!("spectrum: {} sha256:{}", applied.id, applied.sha256);
                    }
                }
                println!("qualification: {}", bundle.qualification);
            }
            BioCommand::Spectrum {
                record,
                measurement,
                id,
                weighting,
                output,
            } => {
                let record_bytes = fs::read(&record)?;
                let record: openbnct_transport::MeasurementRecord =
                    serde_json::from_slice(&record_bytes)?;
                let measurement = record
                    .measurements
                    .iter()
                    .find(|m| m.id == measurement)
                    .ok_or_else(|| {
                        io::Error::other(format!(
                            "no measurement {measurement:?} in record {:?}",
                            record.id
                        ))
                    })?;
                let openbnct_transport::MeasurementValue::Histogram {
                    bin_edges,
                    bin_values,
                    bin_uncertainties_1sigma,
                } = &measurement.value
                else {
                    return Err(io::Error::other(format!(
                        "measurement {:?} is scalar; only histogram measurements carry spectra",
                        measurement.id
                    ))
                    .into());
                };
                if !measurement.unit.to_lowercase().contains("kev") {
                    return Err(io::Error::other(format!(
                        "measurement {:?} unit {:?} is not a lineal-energy unit (keV/µm)",
                        measurement.id, measurement.unit
                    ))
                    .into());
                }
                let weighting =
                    match weighting.as_str() {
                        "event_frequency" => LinealWeighting::EventFrequency,
                        "dose_weighted" => LinealWeighting::DoseWeighted,
                        other => return Err(io::Error::other(format!(
                            "unknown weighting {other:?}; expected event_frequency or dose_weighted"
                        ))
                        .into()),
                    };
                let spectrum = LinealSpectrum {
                    schema_version: openbnct_bio::LINEAL_SPECTRUM_SCHEMA.into(),
                    id: id.unwrap_or_else(|| measurement.id.clone()),
                    bin_edges_kev_um: bin_edges.clone(),
                    values: bin_values.clone(),
                    absolute_standard_uncertainty: bin_uncertainties_1sigma.clone(),
                    weighting,
                    // The record fixes the edge unit (keV/µm) but not the
                    // contents unit; record the metric interpretation.
                    value_unit: format!("{} bin contents", measurement.metric),
                    derivation: Some(openbnct_core::ContentReference {
                        id: record.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&record_bytes),
                    }),
                    note: Some(format!(
                        "extracted from measurement record {} ({}): {}",
                        record.id,
                        measurement.metric,
                        measurement.note.as_deref().unwrap_or("no note")
                    )),
                };
                spectrum.validate()?;
                let json = serde_json::to_vec_pretty(&spectrum)?;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&output)?;
                file.write_all(&json)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                println!("lineal spectrum at {}", output.display());
                println!("id: {} weighting: {weighting:?}", spectrum.id);
            }
            BioCommand::LinealMean { spectrum } => {
                let bytes = fs::read(&spectrum)?;
                let spectrum: LinealSpectrum = serde_json::from_slice(&bytes)?;
                spectrum.validate()?;
                match spectrum.frequency_mean_kev_um()? {
                    Some(yf) => println!("ȳ_F (frequency mean): {yf:.6} keV/µm"),
                    None => println!("ȳ_F: undefined for a dose-weighted spectrum"),
                }
                println!(
                    "ȳ_D (dose mean): {:.6} keV/µm",
                    spectrum.dose_mean_kev_um()?
                );
            }
            BioCommand::LinealTally {
                case,
                data,
                flux,
                spec,
                assignment,
                id,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let flux_bytes = fs::read(&flux)?;
                let mg_flux: openbnct_transport::MultigroupFlux =
                    serde_json::from_slice(&flux_bytes)?;
                let spec_bytes = fs::read(&spec)?;
                let tally_spec: openbnct_bio::LinealTallySpec =
                    serde_json::from_slice(&spec_bytes)?;
                tally_spec
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let assignment_model = match &assignment {
                    Some(path) => Some(serde_json::from_slice::<MaterialAssignment>(&fs::read(
                        path,
                    )?)?),
                    None => None,
                };
                let spectrum = openbnct_bio::compute_lineal_spectrum(
                    &transport_case,
                    &mg_data,
                    &mg_flux,
                    assignment_model.as_ref(),
                    &tally_spec,
                    &id,
                    openbnct_core::ContentReference {
                        id: tally_spec.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&spec_bytes),
                    },
                )
                .map_err(|error| io::Error::other(format!("lineal tally: {error}")))?;
                write_new_json(&output, &spectrum)?;
                println!("lineal spectrum at {}", output.display());
                println!("id: {} weighting: EventFrequency", spectrum.id);
                println!(
                    "ȳ_D (dose mean): {:.6} keV/µm",
                    spectrum
                        .dose_mean_kev_um()
                        .map_err(|e| io::Error::other(e.to_string()))?
                );
            }
            BioCommand::Bed {
                dose,
                alpha_beta,
                region_alpha_beta,
                region_masks,
                quantity,
                output,
            } => {
                let dose_bundle: openbnct_core::ExternalDoseBundle =
                    serde_json::from_slice(&fs::read(&dose)?)?;
                let masks = load_named_masks(&region_masks)?;
                let mut overrides = BTreeMap::new();
                for pair in &region_alpha_beta {
                    let (name, value) = pair.split_once('=').ok_or_else(|| {
                        io::Error::other(format!(
                            "region alpha/beta {pair:?} must be written as name=Gy"
                        ))
                    })?;
                    overrides.insert(
                        name.to_string(),
                        value.parse::<f64>().map_err(|error| {
                            io::Error::other(format!("region alpha/beta {pair:?}: {error}"))
                        })?,
                    );
                }
                let quantity = match quantity.as_str() {
                    "bed" => BedQuantity::Bed,
                    "eqd2" => BedQuantity::Eqd2,
                    other => {
                        return Err(io::Error::other(format!(
                            "quantity {other:?} must be bed or eqd2"
                        ))
                        .into());
                    }
                };
                let bundle =
                    bed_from_external(&dose_bundle, alpha_beta, &overrides, &masks, quantity)?;
                write_new_json(&output, &bundle)?;
                println!(
                    "{} field at {}",
                    serde_json::to_string(&bundle.quantity)?,
                    output.display()
                );
                println!("basis: {}", serde_json::to_string(&bundle.quantity_basis)?);
                println!(
                    "fractions: {}  alpha/beta: {} Gy",
                    bundle.fractions, bundle.alpha_beta
                );
                println!("external provenance: {}", bundle.external_dose_provenance);
            }
            BioCommand::Combine {
                primary,
                external,
                resample,
                assumption,
                output,
            } => {
                let primary_bytes = fs::read(&primary)?;
                let primary_bundle: openbnct_bio::BiologicalDoseBundle =
                    serde_json::from_slice(&primary_bytes)?;
                let external_bytes = fs::read(&external)?;
                let external_bundle: openbnct_bio::BedBundle =
                    serde_json::from_slice(&external_bytes)?;
                let resample = match resample.as_deref() {
                    None => None,
                    Some("trilinear") => Some(ResampleMethod::Trilinear),
                    Some(other) => {
                        return Err(io::Error::other(format!(
                            "resample method {other:?} is not supported (available: trilinear)"
                        ))
                        .into());
                    }
                };
                let combined = combine_biological_doses(
                    &primary_bundle,
                    &external_bundle,
                    openbnct_core::ContentReference {
                        id: primary.display().to_string(),
                        sha256: openbnct_evidence::sha256_hex(&primary_bytes),
                    },
                    openbnct_core::ContentReference {
                        id: external.display().to_string(),
                        sha256: openbnct_evidence::sha256_hex(&external_bytes),
                    },
                    resample,
                    &assumption,
                )?;
                write_new_json(&output, &combined)?;
                println!("combined dose evaluation at {}", output.display());
                println!("quantity: {}", combined.quantity);
                for input in &combined.inputs {
                    println!("{}: {}", input.role, input.provenance_id);
                }
                if let Some(method) = &combined.external_resampling {
                    println!("external resampling: {}", serde_json::to_string(method)?);
                }
                println!("assumption: {}", combined.additivity_assumption);
                println!("qualification: {}", combined.qualification);
            }
            BioCommand::Compare {
                a,
                b,
                region_masks,
                significant_floor,
                output,
            } => {
                let a_bundle: openbnct_bio::BiologicalDoseBundle =
                    serde_json::from_slice(&fs::read(&a)?)?;
                let b_bundle: openbnct_bio::BiologicalDoseBundle =
                    serde_json::from_slice(&fs::read(&b)?)?;
                let masks = load_named_masks(&region_masks)?;
                let comparison = openbnct_bio::compare_biological_models(
                    &a_bundle,
                    &b_bundle,
                    &masks,
                    format!("openbnct.bio-model-comparison.{}.v1", a_bundle.case_id),
                    significant_floor,
                )?;
                write_new_json(&output, &comparison)?;
                println!("bio-model comparison at {}", output.display());
                println!(
                    "models: {} ({}) vs {} ({})",
                    comparison.models[0].id,
                    comparison.weight_semantics[0],
                    comparison.models[1].id,
                    comparison.weight_semantics[1]
                );
                for row in &comparison.regions {
                    println!(
                        "  region {}: mean {:.4e} vs {:.4e} (ratio {})",
                        row.region,
                        row.mean,
                        row.other_mean,
                        row.mean_ratio
                            .map(|r| format!("{r:.4}"))
                            .unwrap_or_else(|| "n/a".into())
                    );
                }
                println!("qualification: {}", comparison.qualification);
            }
            BioCommand::Sweep {
                model,
                physical_bundle,
                region_masks,
                region,
                parameter,
                values,
                output,
            } => {
                let model_bytes = fs::read(&model)?;
                let model: BiologicalModel = serde_json::from_slice(&model_bytes)?;
                let bundle_bytes = fs::read(&physical_bundle)?;
                let physical: PhysicalDoseBundle = serde_json::from_slice(&bundle_bytes)?;
                let masks = load_named_masks(&region_masks)?;
                let parameter = SweepParameter::parse(&parameter)?;
                let sweep = openbnct_bio::run_sweep(
                    &model,
                    &model_bytes,
                    &physical,
                    &openbnct_evidence::sha256_hex(&bundle_bytes),
                    &masks,
                    &region,
                    &parameter,
                    &values,
                )?;
                write_new_json(&output, &sweep)?;
                println!("sensitivity sweep at {}", output.display());
                println!("parameter: {} region: {}", sweep.parameter, sweep.region);
                println!("quantity: {} ({})", sweep.quantity, sweep.unit);
                for point in &sweep.points {
                    println!(
                        "  {} -> mean {:.6e} [{:.6e}, {:.6e}]",
                        point.value, point.mean, point.minimum, point.maximum
                    );
                }
                println!("qualification: {}", sweep.qualification);
            }
        },
        Some(Command::Dvh {
            dose,
            quantity,
            mask,
            bins,
            output,
        }) => {
            let dose_bytes = fs::read(&dose)?;
            let schema: serde_json::Value = serde_json::from_slice(&dose_bytes)?;
            let mask: RegionMask = serde_json::from_slice(&fs::read(&mask)?)?;
            let source = openbnct_core::ContentReference {
                id: dose.display().to_string(),
                sha256: openbnct_evidence::sha256_file(&dose)?,
            };
            let histogram = match schema
                .get("schema_version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
            {
                openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                    let bundle: PhysicalDoseBundle = serde_json::from_slice(&dose_bytes)?;
                    let (values, unit) = dose_values(&bundle, &quantity)?;
                    let voxel_volume = bundle.geometry.spacing_mm.iter().product();
                    openbnct_evidence::DoseVolumeHistogram::compute(
                        &bundle.case_id,
                        &mask.name,
                        &quantity,
                        source,
                        unit,
                        values,
                        &mask.voxels,
                        voxel_volume,
                        bins,
                    )?
                }
                openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
                    let bundle: openbnct_bio::BiologicalDoseBundle =
                        serde_json::from_slice(&dose_bytes)?;
                    let (values, unit) = biological_dose_values(&bundle, &quantity)?;
                    let voxel_volume = bundle.geometry.spacing_mm.iter().product();
                    openbnct_evidence::DoseVolumeHistogram::compute(
                        &bundle.case_id,
                        &mask.name,
                        &quantity,
                        source,
                        unit,
                        values,
                        &mask.voxels,
                        voxel_volume,
                        bins,
                    )?
                }
                other => {
                    return Err(io::Error::other(format!(
                        "unsupported dose bundle schema {other:?}"
                    ))
                    .into());
                }
            };
            let json = serde_json::to_vec_pretty(&histogram)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            file.write_all(&json)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            println!("dose-volume histogram at {}", output.display());
            println!(
                "region: {} ({} voxels)",
                histogram.region, histogram.region_voxel_count
            );
            println!("quantity: {} [{}]", histogram.quantity, histogram.unit);
        }
        Some(Command::Nifti(args)) => match args.command {
            NiftiCommand::Info { input } => {
                let image = read_nifti_file(&input)
                    .map_err(|error| io::Error::other(format!("nifti: {error}")))?;
                let g = &image.geometry;
                println!("shape: {} x {} x {}", g.shape[0], g.shape[1], g.shape[2]);
                println!(
                    "spacing_mm: [{}, {}, {}]",
                    g.spacing_mm[0], g.spacing_mm[1], g.spacing_mm[2]
                );
                println!(
                    "origin_mm: [{}, {}, {}]",
                    g.origin_mm[0], g.origin_mm[1], g.origin_mm[2]
                );
                println!("direction: {:?}", g.direction);
                println!("transform: {}", image.transform_source);
                println!(
                    "datatype: {} description: {:?}",
                    image.datatype, image.description
                );
            }
            NiftiCommand::ToMask {
                input,
                name,
                output,
            } => {
                let image = read_nifti_file(&input)
                    .map_err(|error| io::Error::other(format!("nifti: {error}")))?;
                let mask = openbnct_nifti::to_mask(&image, name);
                write_new_json(&output, &mask)?;
                println!(
                    "mask {} written ({} voxels included)",
                    mask.name,
                    mask.voxels.iter().filter(|v| **v).count()
                );
            }
            NiftiCommand::ExportDose {
                dose,
                quantity,
                output,
            } => {
                let bundle: PhysicalDoseBundle = serde_json::from_slice(&fs::read(&dose)?)?;
                let (values, _unit) = dose_values(&bundle, &quantity)?;
                let image = openbnct_nifti::NiftiImage {
                    geometry: bundle.geometry.clone(),
                    values: values.to_vec(),
                    datatype: 64,
                    transform_source: "sform",
                    description: format!("openbnct {} {}", bundle.case_id, quantity),
                    intent_name: String::new(),
                    units_declared_mm: true,
                };
                openbnct_nifti::write_nifti(&image, &output)?;
                println!("wrote {}", output.display());
            }
            NiftiCommand::ExportComponents {
                dose,
                output_dir,
                gzip,
            } => {
                let bundle: PhysicalDoseBundle = serde_json::from_slice(&fs::read(&dose)?)?;
                let manifest = openbnct_nifti::export_component_niftis(&bundle, &output_dir, gzip)?;
                let manifest_path = output_dir.join(format!("{}.components.json", bundle.case_id));
                fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
                for entry in &manifest.files {
                    println!(
                        "  {} -> {}{}",
                        entry.component,
                        entry.file,
                        entry
                            .sigma_file
                            .as_ref()
                            .map(|s| format!(" (+{s})"))
                            .unwrap_or_default()
                    );
                }
                println!("manifest at {}", manifest_path.display());
            }
            NiftiCommand::Resample {
                input,
                target,
                interpolation,
                output,
            } => {
                let image = read_nifti_file(&input)
                    .map_err(|error| io::Error::other(format!("nifti: {error}")))?;
                let target_geometry = openbnct_nifti::read_target_geometry(&target)
                    .map_err(|error| io::Error::other(format!("target: {error}")))?;
                let interpolation = match interpolation.as_str() {
                    "nearest" => openbnct_nifti::Interpolation::Nearest,
                    "trilinear" => openbnct_nifti::Interpolation::Trilinear,
                    other => {
                        return Err(io::Error::other(format!(
                            "unknown interpolation {other:?}; use nearest or trilinear"
                        ))
                        .into());
                    }
                };
                let resampled = openbnct_nifti::NiftiImage {
                    geometry: target_geometry.clone(),
                    values: openbnct_nifti::resample_to_grid(
                        &image,
                        &target_geometry,
                        interpolation,
                    ),
                    datatype: 64,
                    transform_source: "sform",
                    description: "openbnct resampled".into(),
                    intent_name: String::new(),
                    units_declared_mm: true,
                };
                openbnct_nifti::write_nifti(&resampled, &output)?;
                println!("wrote {}", output.display());
            }
        },
        Some(Command::Accumulate { plan, output }) => {
            let accumulated = openbnct_plan::accumulate_plan_file(&plan)
                .map_err(|error| io::Error::other(format!("accumulation: {error}")))?;
            let json = serde_json::to_vec_pretty(&accumulated)?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)?;
            file.write_all(&json)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            println!("accumulated dose bundle at {}", output.display());
        }
        Some(Command::Import(args)) => match args.command {
            ImportCommand::Interchange { file, output } => {
                let bytes = fs::read(&file)?;
                let document: openbnct_core::ComponentDoseInterchange =
                    serde_json::from_slice(&bytes)?;
                let sha256 = openbnct_evidence::sha256_hex(&bytes);
                let bundle = openbnct_core::import_component_dose(&document, &sha256)
                    .map_err(|error| io::Error::other(format!("interchange import: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!("imported dose bundle at {}", output.display());
                println!(
                    "producer: {} {} ({})",
                    document.producer.system,
                    document.producer.version,
                    document.producer.normalization
                );
            }
            ImportCommand::Mcnp {
                components,
                case_id,
                unit,
                normalization,
                producer_version,
                frame_of_reference_uid,
                output,
            } => {
                let sources = parse_mcnp_components(&components)?;
                let document = openbnct_mcnp::interchange_from_meshtals(
                    &sources,
                    &case_id,
                    parse_dose_unit(&unit)?,
                    &normalization,
                    frame_of_reference_uid,
                    producer_version,
                )
                .map_err(|error| io::Error::other(format!("mcnp import: {error}")))?;
                let bytes = serde_json::to_vec_pretty(&document)?;
                let sha256 = openbnct_evidence::sha256_hex(&bytes);
                let bundle = openbnct_core::import_component_dose(&document, &sha256)
                    .map_err(|error| io::Error::other(format!("interchange import: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!("imported dose bundle at {}", output.display());
                println!(
                    "producer: {} {} ({})",
                    document.producer.system,
                    document.producer.version,
                    document.producer.normalization
                );
            }
            ImportCommand::Phits {
                components,
                case_id,
                unit,
                normalization,
                producer_version,
                frame_of_reference_uid,
                output,
            } => {
                let sources = parse_phits_components(&components)?;
                let document = openbnct_phits::interchange_from_phits(
                    &sources,
                    &case_id,
                    parse_dose_unit(&unit)?,
                    &normalization,
                    frame_of_reference_uid,
                    &producer_version,
                )
                .map_err(|error| io::Error::other(format!("phits import: {error}")))?;
                let bytes = serde_json::to_vec_pretty(&document)?;
                let sha256 = openbnct_evidence::sha256_hex(&bytes);
                let bundle = openbnct_core::import_component_dose(&document, &sha256)
                    .map_err(|error| io::Error::other(format!("interchange import: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!("imported dose bundle at {}", output.display());
                println!(
                    "producer: {} {} ({})",
                    document.producer.system,
                    document.producer.version,
                    document.producer.normalization
                );
            }
            ImportCommand::Nifti {
                components,
                component_sigmas,
                case_id,
                unit,
                normalization,
                producer_system,
                producer_version,
                frame_of_reference_uid,
                output,
            } => {
                let sources = parse_nifti_components(&components, &component_sigmas)?;
                let document = openbnct_nifti::interchange_from_niftis(
                    &sources,
                    &case_id,
                    parse_dose_unit(&unit)?,
                    &normalization,
                    &producer_system,
                    producer_version,
                    frame_of_reference_uid,
                )
                .map_err(|error| io::Error::other(format!("nifti import: {error}")))?;
                let bytes = serde_json::to_vec_pretty(&document)?;
                let sha256 = openbnct_evidence::sha256_hex(&bytes);
                let bundle = openbnct_core::import_component_dose(&document, &sha256)
                    .map_err(|error| io::Error::other(format!("interchange import: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!("imported dose bundle at {}", output.display());
                println!(
                    "producer: {} {} ({})",
                    document.producer.system,
                    document.producer.version,
                    document.producer.normalization
                );
            }
            ImportCommand::Dose { file, output } => {
                let bytes = fs::read(&file)?;
                let document: openbnct_core::ExternalDoseDocument = serde_json::from_slice(&bytes)?;
                let sha256 = openbnct_evidence::sha256_hex(&bytes);
                let bundle = openbnct_core::import_external_dose(&document, &sha256)
                    .map_err(|error| io::Error::other(format!("external-dose import: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!("external dose bundle at {}", output.display());
                println!(
                    "producer: {} {} ({})",
                    document.producer.system,
                    document.producer.version,
                    document.producer.normalization
                );
                println!("fractions: {}", bundle.fraction_count());
            }
        },
        Some(Command::Export(args)) => match args.command {
            ExportCommand::Mcnp {
                case,
                assignment,
                xs_suffix,
                seed,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let case_doc: TransportCase =
                    serde_json::from_slice(&case_bytes).map_err(|error| {
                        io::Error::other(format!("case {}: {error}", case.display()))
                    })?;
                let assignment_doc = assignment
                    .map(|path| -> io::Result<MaterialAssignment> {
                        serde_json::from_slice(&fs::read(&path)?).map_err(|error| {
                            io::Error::other(format!("assignment {}: {error}", path.display()))
                        })
                    })
                    .transpose()?;
                let deck = openbnct_mcnp::deck::export_mcnp_deck(
                    &case_doc,
                    assignment_doc.as_ref(),
                    &openbnct_mcnp::deck::McnpDeckOptions {
                        xs_suffix,
                        seed,
                        case_sha256: format!(
                            "sha256:{}",
                            openbnct_evidence::sha256_hex(&case_bytes)
                        ),
                    },
                )
                .map_err(|error| io::Error::other(format!("mcnp export: {error}")))?;
                write_new_text(&output, deck.as_bytes())?;
                println!("wrote MCNP deck at {}", output.display());
            }
        },
        Some(Command::Compare {
            reference,
            candidate,
            sigma_level,
            output,
        }) => {
            let reference_bytes = fs::read(&reference)?;
            let reference_bundle: PhysicalDoseBundle = serde_json::from_slice(&reference_bytes)?;
            let candidate_bytes = fs::read(&candidate)?;
            let candidate_bundle: PhysicalDoseBundle = serde_json::from_slice(&candidate_bytes)?;
            let comparison = openbnct_evidence::compare_dose_bundles(
                &reference_bundle,
                &candidate_bundle,
                openbnct_core::ContentReference {
                    id: reference.display().to_string(),
                    sha256: openbnct_evidence::sha256_hex(&reference_bytes),
                },
                openbnct_core::ContentReference {
                    id: candidate.display().to_string(),
                    sha256: openbnct_evidence::sha256_hex(&candidate_bytes),
                },
                sigma_level,
            )
            .map_err(|error| io::Error::other(format!("compare: {error}")))?;
            write_new_json(&output, &comparison)?;
            println!("dose comparison at {}", output.display());
            println!(
                "case {}: {} voxels, sigma level {}",
                comparison.case_id, comparison.voxel_count, comparison.sigma_level
            );
            for quantity in &comparison.quantities {
                let sigma_note = quantity
                    .within_sigma_fraction
                    .map(|f| format!("  within-{:.1}σ: {:.1}%", comparison.sigma_level, f * 100.0))
                    .unwrap_or_default();
                println!(
                    "  {}: max |Δ| = {:.4e}  mean |Δ| = {:.4e}  max normalized = {:.4}{}",
                    quantity.quantity,
                    quantity.max_abs_difference,
                    quantity.mean_abs_difference,
                    quantity.max_normalized_difference,
                    sigma_note
                );
            }
            println!("qualification: {}", comparison.qualification);
        }
        Some(Command::Gamma {
            reference,
            candidate,
            dose_difference_percent,
            distance_to_agreement_mm,
            normalization,
            dose_threshold_percent,
            gamma_volume,
            output,
        }) => {
            let normalization = match normalization.as_str() {
                "global" => openbnct_evidence::GammaNormalization::Global,
                "local" => openbnct_evidence::GammaNormalization::Local,
                other => {
                    return Err(format!(
                        "--normalization must be `global` or `local`, got {other:?}"
                    )
                    .into());
                }
            };
            let reference_bytes = fs::read(&reference)?;
            let reference_bundle: PhysicalDoseBundle = serde_json::from_slice(&reference_bytes)?;
            let candidate_bytes = fs::read(&candidate)?;
            let candidate_bundle: PhysicalDoseBundle = serde_json::from_slice(&candidate_bytes)?;
            let evaluation = openbnct_evidence::evaluate_gamma(
                &reference_bundle,
                &candidate_bundle,
                openbnct_core::ContentReference {
                    id: reference.display().to_string(),
                    sha256: openbnct_evidence::sha256_hex(&reference_bytes),
                },
                openbnct_core::ContentReference {
                    id: candidate.display().to_string(),
                    sha256: openbnct_evidence::sha256_hex(&candidate_bytes),
                },
                openbnct_evidence::GammaCriteria {
                    dose_difference_percent,
                    distance_to_agreement_mm,
                    normalization,
                    dose_threshold_percent,
                },
                gamma_volume,
            )
            .map_err(|error| io::Error::other(format!("gamma: {error}")))?;
            write_new_json(&output, &evaluation)?;
            println!("gamma evaluation at {}", output.display());
            println!(
                "case {}: {:.1}%/{:.1}mm, {} normalization",
                evaluation.case_id,
                evaluation.criteria.dose_difference_percent,
                evaluation.criteria.distance_to_agreement_mm,
                match evaluation.criteria.normalization {
                    openbnct_evidence::GammaNormalization::Global => "global",
                    openbnct_evidence::GammaNormalization::Local => "local",
                }
            );
            for result in &evaluation.results {
                let stats = match (result.mean_gamma, result.max_gamma) {
                    (Some(mean), Some(max)) => format!("  mean γ = {mean:.3}  max γ = {max:.3}"),
                    _ => String::new(),
                };
                println!(
                    "  {}: pass {:.2}%  ({} evaluated, {} excluded){}",
                    result.quantity,
                    result.pass_rate * 100.0,
                    result.voxels_evaluated,
                    result.voxels_excluded,
                    stats
                );
            }
            println!("qualification: {}", evaluation.qualification);
        }
        Some(Command::Metamorphic {
            oracle,
            axis,
            turns,
            declared_symmetry,
            voxel_a,
            voxel_b,
            reference,
            candidate,
            id,
            sigma_level,
            output,
        }) => {
            let spec: openbnct_evidence::MetamorphicOracle = match oracle.as_str() {
                "reflection" => openbnct_evidence::MetamorphicOracle::ReflectionSymmetry {
                    axis: axis.clone().ok_or_else(|| {
                        io::Error::other("--axis is required for the reflection oracle")
                    })?,
                    declared_symmetry: declared_symmetry.ok_or_else(|| {
                        io::Error::other(
                            "--declared-symmetry is required for the reflection oracle",
                        )
                    })?,
                },
                "rotation" => openbnct_evidence::MetamorphicOracle::RotationInvariance {
                    axis: axis.clone().ok_or_else(|| {
                        io::Error::other("--axis is required for the rotation oracle")
                    })?,
                    turns: turns.ok_or_else(|| {
                        io::Error::other("--turns is required for the rotation oracle")
                    })?,
                },
                "superposition" => openbnct_evidence::MetamorphicOracle::Superposition,
                "reciprocity" => openbnct_evidence::MetamorphicOracle::PointReciprocity {
                    voxel_a: voxel_a.ok_or_else(|| {
                        io::Error::other("--voxel-a is required for the reciprocity oracle")
                    })?,
                    voxel_b: voxel_b.ok_or_else(|| {
                        io::Error::other("--voxel-b is required for the reciprocity oracle")
                    })?,
                },
                other => {
                    return Err(format!(
                        "--oracle must be reflection|rotation|superposition|reciprocity, got {other:?}"
                    )
                    .into());
                }
            };
            let reference_bytes = fs::read(&reference)?;
            let reference_bundle: PhysicalDoseBundle = serde_json::from_slice(&reference_bytes)?;
            let mut inputs = vec![openbnct_core::ContentReference {
                id: reference.display().to_string(),
                sha256: openbnct_evidence::sha256_hex(&reference_bytes),
            }];
            let mut candidates = Vec::with_capacity(candidate.len());
            for path in &candidate {
                let bytes = fs::read(path)?;
                inputs.push(openbnct_core::ContentReference {
                    id: path.display().to_string(),
                    sha256: openbnct_evidence::sha256_hex(&bytes),
                });
                candidates.push(serde_json::from_slice::<PhysicalDoseBundle>(&bytes)?);
            }
            let candidate_refs: Vec<&PhysicalDoseBundle> = candidates.iter().collect();
            let evaluation = openbnct_evidence::evaluate_metamorphic(
                &id,
                &spec,
                &reference_bundle,
                &candidate_refs,
                inputs,
                sigma_level,
                "cli:metamorphic",
            )
            .map_err(|error| io::Error::other(format!("metamorphic: {error}")))?;
            write_new_json(&output, &evaluation)?;
            println!("metamorphic evaluation at {}", output.display());
            println!(
                "oracle {:?} on case {} at {:.1}σ",
                match &evaluation.oracle {
                    openbnct_evidence::MetamorphicOracle::ReflectionSymmetry { .. } =>
                        "reflection_symmetry",
                    openbnct_evidence::MetamorphicOracle::RotationInvariance { .. } =>
                        "rotation_invariance",
                    openbnct_evidence::MetamorphicOracle::Superposition => "superposition",
                    openbnct_evidence::MetamorphicOracle::PointReciprocity { .. } =>
                        "point_reciprocity",
                },
                evaluation.case_id,
                evaluation.sigma_level
            );
            for quantity in &evaluation.quantities {
                let fraction = quantity
                    .within_sigma_fraction
                    .map(|f| format!("{:.2}%", f * 100.0))
                    .unwrap_or_else(|| "n/a (no σ)".into());
                let max_z = quantity
                    .max_z
                    .map(|z| format!("  max z = {z:.2}"))
                    .unwrap_or_default();
                println!(
                    "  {}: within σ {}  ({} pairs){}",
                    quantity.quantity, fraction, quantity.evaluated_pairs, max_z
                );
            }
            println!("qualification: {}", evaluation.qualification);
        }
        Some(Command::Analytic {
            oracle,
            dose,
            id,
            output,
        }) => {
            let oracle_bytes = fs::read(&oracle)?;
            let oracle_decl: openbnct_evidence::AnalyticOracle =
                serde_json::from_slice(&oracle_bytes)?;
            let oracle_ref = openbnct_core::ContentReference {
                id: oracle_decl.id.clone(),
                sha256: openbnct_evidence::sha256_hex(&oracle_bytes),
            };
            let dose_bytes = fs::read(&dose)?;
            let bundle: PhysicalDoseBundle = serde_json::from_slice(&dose_bytes)?;
            let dose_ref = openbnct_core::ContentReference {
                id: dose.display().to_string(),
                sha256: openbnct_evidence::sha256_hex(&dose_bytes),
            };
            let evaluation = openbnct_evidence::evaluate_analytic_oracle(
                &id,
                &oracle_decl,
                oracle_ref,
                &bundle,
                dose_ref,
            )
            .map_err(|error| io::Error::other(format!("analytic: {error}")))?;
            write_new_json(&output, &evaluation)?;
            println!("analytic oracle evaluation at {}", output.display());
            println!(
                "{} on case {}: fitted slope {:.5} cm^-1 vs expected {:.5} cm^-1 ({} bins)",
                evaluation.quantity,
                evaluation.case_id,
                evaluation.fitted_slope_per_cm,
                evaluation.expected_slope_per_cm,
                evaluation.bins_evaluated
            );
            println!(
                "relative deviation {:.3}% (tolerance {:.1}%) — {}",
                evaluation.relative_deviation * 100.0,
                evaluation.relative_tolerance * 100.0,
                if evaluation.passed { "PASS" } else { "FAIL" }
            );
            println!("qualification: {}", evaluation.qualification);
        }
        Some(Command::Sn(args)) => match args.command {
            SnCommand::Solve {
                case,
                data,
                assignment,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                no_uncollided_split,
                no_transport_correction,
                dose,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let assignment_model = match &assignment {
                    Some(path) => Some(serde_json::from_slice::<MaterialAssignment>(&fs::read(
                        path,
                    )?)?),
                    None => None,
                };
                let mut periodic_axes = [false; 3];
                for axis in &periodic {
                    let index = match axis.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        other => {
                            return Err(io::Error::other(format!(
                                "periodic axis {other:?} must be x, y, or z"
                            ))
                            .into());
                        }
                    };
                    periodic_axes[index] = true;
                }
                let options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model.clone(),
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: !no_transport_correction,
                };
                let data_ref = openbnct_core::ContentReference {
                    id: mg_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let case_ref = openbnct_core::ContentReference {
                    id: transport_case.case_id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&case_bytes),
                };
                let flux = openbnct_transport::solve_multigroup(
                    &transport_case,
                    &mg_data,
                    &options,
                    data_ref,
                    case_ref,
                )
                .map_err(|error| io::Error::other(format!("multigroup: {error}")))?;
                if !flux.converged {
                    return Err(io::Error::other(format!(
                        "multigroup solve did not converge (residual {:.3e} after {} outer iterations)",
                        flux.residual, flux.outer_iterations
                    ))
                    .into());
                }
                write_new_json(&output, &flux)?;
                println!(
                    "multigroup flux at {} (S{}, {} groups, {} outer iterations, residual {:.2e})",
                    output.display(),
                    flux.quadrature_order,
                    flux.energy_boundaries_ev.len().saturating_sub(1),
                    flux.outer_iterations,
                    flux.residual
                );
                if let Some(dose_path) = dose {
                    let profile = mg_data.component_profile.clone().ok_or_else(|| {
                        io::Error::other(
                            "--dose requires the multigroup data to declare component_profile",
                        )
                    })?;
                    let response_ref = openbnct_core::ContentReference {
                        id: mg_data.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&data_bytes),
                    };
                    let bundle = openbnct_transport::fold_multigroup_dose(
                        &transport_case,
                        &mg_data,
                        &flux,
                        assignment_model.as_ref(),
                        profile,
                        response_ref,
                    )
                    .map_err(|error| io::Error::other(format!("dose fold: {error}")))?;
                    write_new_json(&dose_path, &bundle)?;
                    println!("folded dose bundle at {}", dose_path.display());
                }
            }
            SnCommand::Fold {
                case,
                data,
                flux,
                assignment,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let flux_model: openbnct_transport::MultigroupFlux =
                    serde_json::from_slice(&fs::read(&flux)?)?;
                let assignment_model = match &assignment {
                    Some(path) => Some(serde_json::from_slice::<MaterialAssignment>(&fs::read(
                        path,
                    )?)?),
                    None => None,
                };
                let profile = mg_data.component_profile.clone().ok_or_else(|| {
                    io::Error::other(
                        "fold requires the multigroup data to declare component_profile",
                    )
                })?;
                let response_ref = openbnct_core::ContentReference {
                    id: mg_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let bundle = openbnct_transport::fold_multigroup_dose(
                    &transport_case,
                    &mg_data,
                    &flux_model,
                    assignment_model.as_ref(),
                    profile,
                    response_ref,
                )
                .map_err(|error| io::Error::other(format!("dose fold: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!("folded dose bundle at {}", output.display());
            }
            SnCommand::Collapse {
                library,
                endf,
                material,
                boundaries,
                id,
                component_profile,
                note,
                output,
            } => {
                let mut endf_paths = std::collections::BTreeMap::new();
                for spec in &endf {
                    let (name, path) = spec.split_once('=').ok_or_else(|| {
                        io::Error::other(format!("--endf expects NUCLIDE=PATH, got {spec:?}"))
                    })?;
                    endf_paths.insert(name.to_string(), PathBuf::from(path));
                }
                let mut material_models = Vec::with_capacity(material.len());
                for path in &material {
                    material_models.push(serde_json::from_slice::<
                        openbnct_transport::MaterialDefinition,
                    >(&fs::read(path)?)?);
                }
                let profile_ref = match &component_profile {
                    Some(path) => {
                        let bytes = fs::read(path)?;
                        let profile: serde_json::Value = serde_json::from_slice(&bytes)?;
                        let profile_id = profile
                            .get("id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                io::Error::other("component profile JSON has no string `id` field")
                            })?
                            .to_string();
                        Some(openbnct_core::ContentReference {
                            id: profile_id,
                            sha256: openbnct_evidence::sha256_hex(&bytes),
                        })
                    }
                    None => None,
                };
                let options = openbnct_openmc::CollapseOptions {
                    library_dir: library.clone(),
                    endf_paths,
                    materials: material_models,
                    energy_boundaries_ev: boundaries,
                    weighting:
                        openbnct_openmc::WeightingSpectrum::ThermalMaxwellianEpithermalFlat {
                            cut_ev: 0.5,
                        },
                    id: id.clone(),
                    component_profile: profile_ref,
                    note: note.unwrap_or_default(),
                };
                let data = openbnct_openmc::collapse_multigroup(&options)
                    .map_err(|error| io::Error::other(format!("collapse: {error}")))?;
                let json = serde_json::to_string_pretty(&data)?;
                match &output {
                    Some(path) => {
                        write_new_json(path, &data)?;
                        println!(
                            "multigroup data at {} ({} groups, {} materials)",
                            path.display(),
                            data.energy_boundaries_ev.len() - 1,
                            data.materials.len()
                        );
                    }
                    None => println!("{json}"),
                }
            }
        },
        Some(Command::Plan(args)) => match args.command {
            PlanCommand::Import {
                table,
                id,
                case_id,
                bundles_dir,
                output,
            } => {
                let options = openbnct_plan::TableImportOptions {
                    id,
                    case_id,
                    bundles_dir,
                };
                let plan = openbnct_plan::read_table(&table, &options)
                    .map_err(|error| io::Error::other(format!("plan table: {error}")))?;
                write_new_json(&output, &plan)?;
                println!("exposure plan at {}", output.display());
                println!(
                    "plan {} for case {}: {} exposures",
                    plan.id,
                    plan.case_id,
                    plan.exposures.len()
                );
            }
            PlanCommand::Export { plan, output } => {
                let plan: ExposurePlan = serde_json::from_slice(&fs::read(&plan)?)?;
                plan.validate()
                    .map_err(|error| io::Error::other(format!("exposure plan: {error}")))?;
                openbnct_plan::write_table(&output, &plan)
                    .map_err(|error| io::Error::other(format!("plan table: {error}")))?;
                println!("exposure table at {}", output.display());
            }
            PlanCommand::Validate { plan } => {
                let plan: ExposurePlan = serde_json::from_slice(&fs::read(&plan)?)?;
                let issues = plan.validate_diagnostics();
                if issues.is_empty() {
                    println!(
                        "plan {} is valid: {} exposures",
                        plan.id,
                        plan.exposures.len()
                    );
                } else {
                    eprintln!("plan {} has {} issue(s):", plan.id, issues.len());
                    for issue in &issues {
                        eprintln!("  - {issue}");
                    }
                    return Err(io::Error::other("exposure plan validation failed").into());
                }
            }
        },
        Some(Command::Evidence(args)) => match args.command {
            EvidenceCommand::Export {
                root,
                case_id,
                qualification,
                artifacts,
            } => {
                let qualification = match qualification.as_str() {
                    "synthetic_research_only" => {
                        openbnct_evidence::QualificationBoundary::SyntheticResearchOnly
                    }
                    "cross_code_research_only" => {
                        openbnct_evidence::QualificationBoundary::CrossCodeResearchOnly
                    }
                    "experimentally_validated_research_only" => {
                        openbnct_evidence::QualificationBoundary::ExperimentallyValidatedResearchOnly
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "unknown qualification boundary {other:?}"
                        ))
                        .into());
                    }
                };
                let mut inputs = Vec::new();
                for triple in &artifacts {
                    let (role, rest) = triple.split_once('=').ok_or_else(|| {
                        io::Error::other(format!(
                            "artifact {triple:?} must be written as role=SOURCE:DEST"
                        ))
                    })?;
                    let (source, dest) = rest.rsplit_once(':').ok_or_else(|| {
                        io::Error::other(format!(
                            "artifact {triple:?} must be written as role=SOURCE:DEST"
                        ))
                    })?;
                    inputs.push(openbnct_evidence::BundleInput {
                        role: role.into(),
                        source: PathBuf::from(source),
                        relative_path: dest.into(),
                        media_type: Some("application/json".into()),
                    });
                }
                let manifest = openbnct_evidence::export_evidence_bundle(
                    &root,
                    &case_id,
                    qualification,
                    &inputs,
                )?;
                println!("evidence bundle exported at {}", root.display());
                println!("artifacts: {}", manifest.artifacts.len());
            }
            EvidenceCommand::Verify { root } => {
                let manifest = openbnct_evidence::EvidenceBundleManifest::load_verified(&root)?;
                println!("evidence bundle verified at {}", root.display());
                println!("case: {}", manifest.case_id);
                println!("artifacts verified: {}", manifest.artifacts.len());
            }
        },
        Some(Command::Mask(args)) => match args.command {
            MaskCommand::Subtract {
                input,
                minus,
                name,
                output,
            } => {
                let mut mask = read_region_mask(&input)?;
                for path in &minus {
                    mask = mask
                        .subtract(&read_region_mask(path)?, name.clone())
                        .map_err(|error| io::Error::other(error.to_string()))?;
                }
                write_mask(&mask, &output)?;
            }
            MaskCommand::Union {
                inputs,
                name,
                output,
            } => {
                let mask = fold_region_masks(&inputs, &name, RegionMask::union)?;
                write_mask(&mask, &output)?;
            }
            MaskCommand::Intersect {
                inputs,
                name,
                output,
            } => {
                let mask = fold_region_masks(&inputs, &name, RegionMask::intersection)?;
                write_mask(&mask, &output)?;
            }
            MaskCommand::Threshold {
                ct_dir,
                min,
                max,
                name,
                output,
            } => {
                let mut slices = fs::read_dir(&ct_dir)?
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                slices.sort();
                let ct = openbnct_dicom::import_ct_series(&slices)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let mask = ct
                    .threshold_mask(name, min, max)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_mask(&mask, &output)?;
            }
        },
        Some(Command::IrradiationTime {
            dose,
            quantity,
            source_strength,
            limits,
            masks,
            output,
        }) => {
            let dose_bytes = fs::read(&dose)?;
            let schema: serde_json::Value = serde_json::from_slice(&dose_bytes)?;
            let mut region_masks = Vec::with_capacity(masks.len());
            for binding in &masks {
                let (name, path) = binding
                    .split_once('=')
                    .ok_or_else(|| io::Error::other("--mask entries must be NAME=path"))?;
                let mask = read_region_mask(Path::new(path))?;
                if mask.name != name {
                    return Err(io::Error::other(format!(
                        "--mask {name}: mask file names itself {:?}",
                        mask.name
                    ))
                    .into());
                }
                region_masks.push(mask);
            }
            let mut organ_limits = Vec::with_capacity(limits.len());
            for entry in &limits {
                let (name, rest) = entry.split_once('=').ok_or_else(|| {
                    io::Error::other("--limit entries must be NAME=max|mean:LIMIT")
                })?;
                let (metric, value) = rest.split_once(':').ok_or_else(|| {
                    io::Error::other("--limit entries must be NAME=max|mean:LIMIT")
                })?;
                let metric = match metric {
                    "max" => openbnct_evidence::LimitMetric::Max,
                    "mean" => openbnct_evidence::LimitMetric::Mean,
                    other => {
                        return Err(io::Error::other(format!(
                            "--limit {name}: unknown metric {other:?} (max|mean)"
                        ))
                        .into());
                    }
                };
                let limit: f64 = value.parse().map_err(|_| {
                    io::Error::other(format!("--limit {name}: invalid limit {value:?}"))
                })?;
                organ_limits.push(openbnct_evidence::OrganLimit {
                    region: name.to_owned(),
                    metric,
                    limit,
                });
            }
            let source = openbnct_core::ContentReference {
                id: dose.display().to_string(),
                sha256: openbnct_evidence::sha256_file(&dose)?,
            };
            let report = match schema
                .get("schema_version")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
            {
                openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                    let bundle: PhysicalDoseBundle = serde_json::from_slice(&dose_bytes)?;
                    let (values, unit) = dose_values(&bundle, &quantity)?;
                    openbnct_evidence::IrradiationTimeReport::evaluate(
                        &bundle.case_id,
                        &quantity,
                        source,
                        unit,
                        values,
                        &region_masks,
                        &organ_limits,
                        source_strength,
                    )?
                }
                openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
                    let bundle: openbnct_bio::BiologicalDoseBundle =
                        serde_json::from_slice(&dose_bytes)?;
                    let (values, unit) = biological_dose_values(&bundle, &quantity)?;
                    openbnct_evidence::IrradiationTimeReport::evaluate(
                        &bundle.case_id,
                        &quantity,
                        source,
                        unit,
                        values,
                        &region_masks,
                        &organ_limits,
                        source_strength,
                    )?
                }
                other => {
                    return Err(io::Error::other(format!(
                        "unsupported dose bundle schema {other:?}"
                    ))
                    .into());
                }
            };
            write_new_json(&output, &report)?;
            println!("irradiation-time report at {}", output.display());
            for region in &report.regions {
                match (region.max_time_s, region.max_source_particles) {
                    (Some(time), Some(particles)) => println!(
                        "{} {:?} limit {}: {:.6e} endpoint/s -> max {:.6e} s ({:.6e} particles)",
                        region.region,
                        region.metric,
                        region.limit,
                        region.endpoint_rate_per_s,
                        time,
                        particles,
                    ),
                    _ => println!(
                        "{} {:?} limit {}: zero endpoint rate -> unbounded",
                        region.region, region.metric, region.limit,
                    ),
                }
            }
            match &report.limiting {
                Some(limiting) => println!(
                    "limiting structure: {} ({:?}), max {:.6e} s",
                    limiting.region, limiting.metric, limiting.max_time_s
                ),
                None => println!("no region bounds the irradiation (all endpoint rates zero)"),
            }
        }
        Some(Command::Metrics {
            dose,
            quantity,
            mask,
            dx,
            vx,
            eud,
            output,
        }) => {
            let dose_bytes = fs::read(&dose)?;
            let bundle = load_dose_bundle(&dose_bytes)?;
            let mask: RegionMask = serde_json::from_slice(&fs::read(&mask)?)?;
            let source = openbnct_core::ContentReference {
                id: dose.display().to_string(),
                sha256: openbnct_evidence::sha256_file(&dose)?,
            };
            let selection = bundle.select(&quantity)?;
            let metrics = openbnct_evidence::RegionDoseMetrics::compute(
                selection.case_id,
                &mask.name,
                &quantity,
                source,
                selection.unit,
                selection.values,
                &mask.voxels,
                selection.voxel_volume_mm3,
                &dx,
                &vx,
                &eud,
            )?;
            write_new_json(&output, &metrics)?;
            println!("dose metrics at {}", output.display());
            println!(
                "region: {} ({} voxels, {} [{}])",
                metrics.region, metrics.region_voxel_count, metrics.quantity, metrics.unit
            );
            println!(
                "min/mean/max: {:.6e} / {:.6e} / {:.6e}",
                metrics.minimum_dose, metrics.mean_dose, metrics.maximum_dose
            );
            for metric in &metrics.dx {
                println!("D{}: {:.6e}", metric.percent, metric.dose);
            }
            for metric in &metrics.vx {
                println!("V({:.6e}): {:.4}", metric.level, metric.volume_fraction);
            }
            for metric in &metrics.eud {
                println!("EUD(a={}): {:.6e}", metric.a, metric.dose);
            }
        }
        Some(Command::Endpoint(args)) => match args.command {
            EndpointCommand::Evaluate {
                model,
                dose,
                quantity,
                mask,
                output,
            } => {
                let model_bytes = fs::read(&model)?;
                let endpoint_model: openbnct_bio::EndpointModel =
                    serde_json::from_slice(&model_bytes)?;
                let dose_bytes = fs::read(&dose)?;
                let bundle = load_dose_bundle(&dose_bytes)?;
                let mask = read_region_mask(&mask)?;
                let source = openbnct_core::ContentReference {
                    id: dose.display().to_string(),
                    sha256: openbnct_evidence::sha256_file(&dose)?,
                };
                let selection = bundle.select(&quantity)?;
                let evaluation = openbnct_bio::evaluate_endpoint(
                    &endpoint_model,
                    &model_bytes,
                    selection.case_id,
                    &mask,
                    &quantity,
                    selection.unit,
                    selection.values,
                    selection.voxel_volume_mm3,
                    source,
                )?;
                write_new_json(&output, &evaluation)?;
                println!("endpoint evaluation at {}", output.display());
                println!(
                    "endpoint: {:?} region: {} quantity: {}",
                    evaluation.endpoint, evaluation.region, evaluation.quantity
                );
                if let Some(statistic) = &evaluation.dose_statistic {
                    println!(
                        "dose statistic: {} = {:.6e} [{}]",
                        statistic.kind, statistic.value, statistic.unit
                    );
                }
                println!("probability: {:.6}", evaluation.probability);
                println!("qualification: {}", evaluation.qualification);
            }
            EndpointCommand::Utcp {
                tcp,
                ntcp,
                combination,
                output,
            } => {
                let tcp_bytes = fs::read(&tcp)?;
                let ntcp_bytes = fs::read(&ntcp)?;
                let tcp_eval: openbnct_bio::EndpointEvaluation =
                    serde_json::from_slice(&tcp_bytes)?;
                let ntcp_eval: openbnct_bio::EndpointEvaluation =
                    serde_json::from_slice(&ntcp_bytes)?;
                let combination = match combination.as_str() {
                    "p_plus" => openbnct_bio::UtcpCombination::PPlus,
                    "difference" => openbnct_bio::UtcpCombination::Difference,
                    other => {
                        return Err(io::Error::other(format!(
                            "unknown UTCP combination {other:?}; use p_plus or difference"
                        ))
                        .into());
                    }
                };
                let evaluation = openbnct_bio::combine_utcp(
                    &tcp_eval,
                    &tcp_bytes,
                    &ntcp_eval,
                    &ntcp_bytes,
                    combination,
                )?;
                write_new_json(&output, &evaluation)?;
                println!("UTCP evaluation at {}", output.display());
                println!("probability: {:.6}", evaluation.probability);
                println!("qualification: {}", evaluation.qualification);
            }
        },
        Some(Command::Register(args)) => match args.command {
            RegisterCommand::Landmarks {
                pairs,
                id,
                moving,
                fixed,
                note,
                output,
            } => {
                let pairs: Vec<openbnct_core::LandmarkPair> =
                    serde_json::from_slice(&fs::read(&pairs)?)?;
                let moving = image_reference(&moving)?;
                let fixed = image_reference(&fixed)?;
                let registration =
                    openbnct_core::landmark_registration(id, moving, fixed, pairs, note)?;
                write_new_json(&output, &registration)?;
                println!("registration at {}", output.display());
                println!("method: landmark_least_squares");
                println!(
                    "landmarks: {}",
                    registration.landmarks.as_ref().map_or(0, Vec::len)
                );
                println!(
                    "rms residual: {:.6} mm",
                    registration.rms_residual_mm.unwrap_or(f64::NAN)
                );
            }
            RegisterCommand::Declare {
                id,
                rotation,
                translation_mm,
                moving,
                fixed,
                note,
                output,
            } => {
                if rotation.len() != 9 || translation_mm.len() != 3 {
                    return Err(io::Error::other(
                        "--rotation takes nine comma-separated values and --translation-mm three",
                    )
                    .into());
                }
                let transform = openbnct_core::RigidTransform {
                    rotation: rotation.try_into().unwrap(),
                    translation_mm: translation_mm.try_into().unwrap(),
                };
                let registration = openbnct_core::declared_registration(
                    id,
                    image_reference(&moving)?,
                    image_reference(&fixed)?,
                    transform,
                    note,
                )?;
                write_new_json(&output, &registration)?;
                println!("registration at {}", output.display());
                println!("method: declared");
            }
            RegisterCommand::Info { registration } => {
                let registration: openbnct_core::Registration =
                    serde_json::from_slice(&fs::read(&registration)?)?;
                registration.validate()?;
                println!("id: {}", registration.id);
                println!("method: {:?}", registration.method);
                println!(
                    "rotation (row-major): {:?}",
                    registration.transform.rotation
                );
                println!(
                    "translation mm: {:?}",
                    registration.transform.translation_mm
                );
                if let Some(rms) = registration.rms_residual_mm {
                    println!("rms residual: {rms:.6} mm");
                }
                for (label, reference) in [
                    ("moving", &registration.moving),
                    ("fixed", &registration.fixed),
                ] {
                    if let Some(reference) = reference {
                        println!("{label}: {} sha256:{}", reference.id, reference.sha256);
                    }
                }
                if let Some(note) = &registration.note {
                    println!("note: {note}");
                }
            }
            RegisterCommand::Apply {
                moving,
                registration,
                target_grid,
                interpolation,
                output,
            } => {
                let registration: openbnct_core::Registration =
                    serde_json::from_slice(&fs::read(&registration)?)?;
                registration.validate()?;
                let interpolation = match interpolation.as_str() {
                    "trilinear" => Interpolation::Trilinear,
                    "nearest" => Interpolation::Nearest,
                    other => {
                        return Err(io::Error::other(format!(
                            "unknown interpolation {other:?}; use trilinear or nearest"
                        ))
                        .into());
                    }
                };
                let mut image = read_nifti_file(&moving)?;
                // Registration moves the volume's patient-space frame;
                // the voxel data itself is unchanged.
                image.geometry = registration.transform.apply_to_geometry(&image.geometry);
                let target = read_target_geometry(&target_grid)?;
                let values = resample_to_grid(&image, &target, interpolation);
                let resampled = NiftiImage {
                    geometry: target,
                    values,
                    datatype: 64,
                    transform_source: "sform",
                    description: format!(
                        "resampled under registration {} ({:?})",
                        registration.id, registration.method
                    ),
                    intent_name: image.intent_name.clone(),
                    units_declared_mm: true,
                };
                write_nifti(&resampled, &output)?;
                println!("resampled volume at {}", output.display());
                println!(
                    "registration: {} ({:?})",
                    registration.id, registration.method
                );
            }
        },
        Some(Command::Boron(args)) => match args.command {
            BoronCommand::Info { model } => {
                let model: openbnct_boron::BoronUptakeModel =
                    serde_json::from_slice(&fs::read(&model)?)?;
                model
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("id: {}", model.id);
                println!("mapping: {:?}", model.mapping);
                if let Some(washout) = &model.time_correction {
                    println!(
                        "washout: {} min elapsed, T1/2 {} ± {} min",
                        washout.delta_minutes,
                        washout.half_life_minutes,
                        washout.half_life_1sigma_minutes
                    );
                }
                if let Some(noise) = model.suv_noise_1sigma {
                    println!("suv noise 1σ: {noise}");
                }
                println!("validity domain: {}", model.validity_domain);
                println!("provenance: {}", model.provenance_id);
            }
            BoronCommand::Apply {
                model: model_path,
                case,
                suv,
                registration,
                interpolation,
                id,
                output,
                nifti_output,
            } => {
                let model: openbnct_boron::BoronUptakeModel =
                    serde_json::from_slice(&fs::read(&model_path)?)?;
                model
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let case: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                case.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;

                let registration_doc: Option<openbnct_core::Registration> = registration
                    .as_ref()
                    .map(|path| -> Result<openbnct_core::Registration, io::Error> {
                        let doc: openbnct_core::Registration =
                            serde_json::from_slice(&fs::read(path)?)?;
                        doc.validate()
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        Ok(doc)
                    })
                    .transpose()?;

                // Load, transform, and resample the SUV volume onto the
                // case grid when the model consumes one.
                let suv_values: Option<Vec<f64>> = if let Some(suv_path) = &suv {
                    let mut image = read_nifti_file(suv_path)
                        .map_err(|error| io::Error::other(format!("nifti: {error}")))?;
                    if let Some(doc) = &registration_doc {
                        image.geometry = doc.transform.apply_to_geometry(&image.geometry);
                    }
                    let values = if image.geometry == case.geometry {
                        image.values
                    } else {
                        let interpolation = match interpolation.as_str() {
                            "trilinear" => Interpolation::Trilinear,
                            "nearest" => Interpolation::Nearest,
                            other => {
                                return Err(io::Error::other(format!(
                                    "unknown interpolation {other:?}; use trilinear or nearest"
                                ))
                                .into());
                            }
                        };
                        resample_to_grid(&image, &case.geometry, interpolation)
                    };
                    Some(values)
                } else {
                    if registration_doc.is_some() {
                        return Err(io::Error::other("--registration requires --suv").into());
                    }
                    if model.requires_suv() {
                        return Err(io::Error::other(
                            "model mapping requires --suv (only `uniform` omits it)",
                        )
                        .into());
                    }
                    None
                };

                let field = openbnct_boron::apply_uptake_model(
                    &model,
                    suv_values.as_deref(),
                    &case.geometry,
                    &case.case_id,
                    openbnct_boron::BoronFieldProvenance {
                        id,
                        provenance_id: format!("boron-field:{}", model.id),
                        model: openbnct_core::ContentReference {
                            id: model.id.clone(),
                            sha256: openbnct_evidence::sha256_file(&model_path)?,
                        },
                        suv_image: image_reference(&suv)?,
                        registration: image_reference(&registration)?,
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &field)?;
                println!("boron field at {}", output.display());
                println!("voxels: {}", field.values.len());
                let (min, max) = field
                    .values
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                        (lo.min(v), hi.max(v))
                    });
                println!("B-10 range: {min:.4} – {max:.4} µg/g");
                if field.clamped_negative_voxels > 0 {
                    println!(
                        "note: {} voxels clamped from negative to zero",
                        field.clamped_negative_voxels
                    );
                }
                if let Some(nifti_path) = nifti_output {
                    let image = NiftiImage {
                        geometry: field.geometry.clone(),
                        values: field.values.clone(),
                        datatype: 64,
                        transform_source: "sform",
                        description: format!(
                            "B-10 field {} (µg/g) from model {}",
                            field.id, model.id
                        ),
                        intent_name: String::new(),
                        units_declared_mm: true,
                    };
                    write_nifti(&image, &nifti_path)?;
                    println!("nifti: {}", nifti_path.display());
                }
            }
            BoronCommand::Materialize {
                field,
                case,
                tiers,
                output,
            } => {
                let field: openbnct_boron::BoronField = serde_json::from_slice(&fs::read(&field)?)?;
                field
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let case: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                case.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                if field.geometry != case.geometry {
                    return Err(
                        io::Error::other("field geometry does not match the case grid").into(),
                    );
                }
                let assignment = openbnct_boron::materialize_field(
                    &field,
                    &case.material,
                    &case.case_id,
                    tiers,
                    format!("boron-materialize:{}", field.id),
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &assignment)?;
                println!("material assignment at {}", output.display());
                println!("tiers populated: {}", assignment.regions.len());
            }
            BoronCommand::Microdistribution { model, id, output } => {
                let model_bytes = fs::read(&model)?;
                let model: openbnct_boron::BoronMicrodistribution =
                    serde_json::from_slice(&model_bytes)?;
                model
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let reference = openbnct_core::ContentReference {
                    id: model.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&model_bytes),
                };
                let correction = openbnct_boron::evaluate_microdistribution(
                    &model,
                    &id,
                    &format!("microdistribution:{}", model.id),
                    reference,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &correction)?;
                println!("correction: {}", output.display());
                println!(
                    "nucleus dose factor: {:.3} ± {:.3} (vs uniform)",
                    correction.nucleus_dose_factor, correction.nucleus_dose_factor_1sigma
                );
                let d = &correction.deposition;
                println!(
                    "deposition to nucleus — nucleus {:.3} / cytoplasm {:.3} / membrane {:.3} / extracellular {:.3}",
                    d.nucleus.combined_to_nucleus,
                    d.cytoplasm.combined_to_nucleus,
                    d.membrane.combined_to_nucleus,
                    d.extracellular.combined_to_nucleus
                );
                println!(
                    "uniform reference: {:.3}; intercellular dose CV {:.2}",
                    correction.uniform_reference.combined_to_nucleus,
                    correction.intercellular_dose_cv
                );
            }
        },
        Some(Command::Uq(args)) => match args.command {
            UqCommand::Propagate {
                case,
                data,
                covariance,
                component,
                assignment,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                no_uncollided_split,
                forward_flux,
                statistical_rel_std,
                id,
                output,
                flux,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let cov_bytes = fs::read(&covariance)?;
                let cov: openbnct_transport::MultigroupCovariance =
                    serde_json::from_slice(&cov_bytes)?;
                cov.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let assignment_model = match &assignment {
                    Some(path) => Some(serde_json::from_slice::<MaterialAssignment>(&fs::read(
                        path,
                    )?)?),
                    None => None,
                };
                let mut periodic_axes = [false; 3];
                for axis in &periodic {
                    let index = match axis.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        other => {
                            return Err(io::Error::other(format!(
                                "periodic axis {other:?} must be x, y, or z"
                            ))
                            .into());
                        }
                    };
                    periodic_axes[index] = true;
                }
                let options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model,
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: true,
                };
                let nominal_flux =
                    match &forward_flux {
                        Some(path) => Some(serde_json::from_slice::<
                            openbnct_transport::MultigroupFlux,
                        >(&fs::read(path)?)?),
                        None => None,
                    };
                let derivation = openbnct_transport::propagate_uncertainty(
                    &transport_case,
                    &mg_data,
                    &options,
                    &cov,
                    &component,
                    nominal_flux.as_ref(),
                    statistical_rel_std,
                    &id,
                    openbnct_core::ContentReference {
                        id: cov.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&cov_bytes),
                    },
                    openbnct_core::ContentReference {
                        id: mg_data.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&data_bytes),
                    },
                    openbnct_core::ContentReference {
                        id: transport_case.case_id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&case_bytes),
                    },
                )
                .map_err(|error| io::Error::other(format!("uq: {error}")))?;
                let mut budget = derivation.budget;
                if let Some(prefix) = &flux {
                    let flux_path = prefix.with_extension("json");
                    write_new_json(&flux_path, &derivation.forward_flux)?;
                    let flux_bytes = fs::read(&flux_path)?;
                    budget.fluxes.push(openbnct_core::ContentReference {
                        id: format!("{}.nominal-flux", budget.id),
                        sha256: openbnct_evidence::sha256_hex(&flux_bytes),
                    });
                }
                write_new_json(&output, &budget)?;
                println!(
                    "dose uncertainty budget at {} (R={:.6e}, σ_rel={:.3e}, {} perturbed solves)",
                    output.display(),
                    budget.response_integral,
                    budget.total_relative_std_dev,
                    derivation.perturbed_solves,
                );
                for entry in &budget.entries {
                    println!(
                        "  {} {:>7.2}%  σ_R={:.3e}",
                        entry.parameter,
                        entry.relative_contribution * 100.0,
                        entry.variance_contribution.sqrt(),
                    );
                }
            }
            UqCommand::BudgetInfo { budget } => {
                let budget: openbnct_transport::DoseUncertaintyBudget =
                    serde_json::from_slice(&fs::read(&budget)?)?;
                budget
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("id: {}", budget.id);
                println!("case: {}", budget.case_id);
                println!("component: {}", budget.component);
                println!("response integral: {:.6e}", budget.response_integral);
                println!(
                    "total relative std dev: {:.4e}",
                    budget.total_relative_std_dev
                );
                for entry in &budget.entries {
                    println!(
                        "  [{}] {}: σ_R={:.4e} share={:.2}%",
                        entry.source,
                        entry.parameter,
                        entry.variance_contribution.sqrt(),
                        entry.relative_contribution * 100.0,
                    );
                }
            }
            UqCommand::Screen {
                case,
                data,
                spec,
                assignment,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                no_uncollided_split,
                id,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let spec_bytes = fs::read(&spec)?;
                let screen_spec: openbnct_transport::SensitivitySpec =
                    serde_json::from_slice(&spec_bytes)?;
                screen_spec
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let assignment_model = match &assignment {
                    Some(path) => Some(serde_json::from_slice::<MaterialAssignment>(&fs::read(
                        path,
                    )?)?),
                    None => None,
                };
                let mut periodic_axes = [false; 3];
                for axis in &periodic {
                    let index = match axis.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        other => {
                            return Err(io::Error::other(format!(
                                "periodic axis {other:?} must be x, y, or z"
                            ))
                            .into());
                        }
                    };
                    periodic_axes[index] = true;
                }
                let options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model,
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: true,
                };
                let report = openbnct_transport::run_screening(
                    &transport_case,
                    &mg_data,
                    &options,
                    &screen_spec,
                    &id,
                    openbnct_core::ContentReference {
                        id: screen_spec.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&spec_bytes),
                    },
                    openbnct_core::ContentReference {
                        id: mg_data.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&data_bytes),
                    },
                    openbnct_core::ContentReference {
                        id: transport_case.case_id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&case_bytes),
                    },
                )
                .map_err(|error| io::Error::other(format!("screening: {error}")))?;
                write_new_json(&output, &report)?;
                println!(
                    "sensitivity screening at {} ({} evaluations, R0={:.6e})",
                    output.display(),
                    report.evaluations,
                    report.nominal_response,
                );
                let mut ranked: Vec<_> = report.entries.iter().collect();
                ranked.sort_by_key(|e| e.rank);
                for entry in ranked {
                    let stat = if report.method == "morris" {
                        format!("mu*={:.4e} sigma={:.4e}", entry.mu_star, entry.sigma)
                    } else {
                        format!("S1={:.4e} ST={:.4e}", entry.first_order, entry.total_order)
                    };
                    println!("  #{} {}: {}", entry.rank, entry.name, stat);
                }
            }
            UqCommand::ScreeningInfo { report } => {
                let report: openbnct_transport::SensitivityScreening =
                    serde_json::from_slice(&fs::read(&report)?)?;
                report
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("id: {}", report.id);
                println!("case: {}", report.case_id);
                println!("method: {}", report.method);
                println!("response component: {}", report.response_component);
                println!("nominal response: {:.6e}", report.nominal_response);
                println!("evaluations: {}", report.evaluations);
                let mut ranked: Vec<_> = report.entries.iter().collect();
                ranked.sort_by_key(|e| e.rank);
                for entry in ranked {
                    let stat = if report.method == "morris" {
                        format!(
                            "mu={:.4e} mu*={:.4e} sigma={:.4e}",
                            entry.mu, entry.mu_star, entry.sigma
                        )
                    } else {
                        format!("S1={:.4e} ST={:.4e}", entry.first_order, entry.total_order)
                    };
                    println!("  #{} {}: {}", entry.rank, entry.name, stat);
                }
            }
            UqCommand::Info { report } => {
                let report: openbnct_core::SystematicUncertaintyReport =
                    serde_json::from_slice(&fs::read(&report)?)?;
                report
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("id: {}", report.id);
                println!("quantity: {}", report.quantity);
                println!(
                    "dose bundle: {} sha256:{}",
                    report.dose_bundle.id, report.dose_bundle.sha256
                );
                println!("sources: {}", report.sources.len());
                for (source, summary) in report.sources.iter().zip(report.source_summaries.iter()) {
                    let declaration = match source {
                        openbnct_core::UncertaintySource::BoronConcentration { field } => {
                            format!(
                                "boron_concentration field={} sha256:{}",
                                field.id, field.sha256
                            )
                        }
                        openbnct_core::UncertaintySource::Positioning {
                            sigma_mm,
                            registration,
                        } => format!(
                            "positioning sigma_mm={sigma_mm}{}",
                            registration
                                .as_ref()
                                .map(|r| format!(" registration={}", r.id))
                                .unwrap_or_default()
                        ),
                        openbnct_core::UncertaintySource::RelativeComponent {
                            component,
                            relative_1sigma,
                        } => {
                            format!("relative_component {component} rel={relative_1sigma}")
                        }
                    };
                    println!(
                        "  {declaration}: mean σ {:.4e}, max σ {:.4e}, skipped {}",
                        summary.mean_1sigma, summary.max_1sigma, summary.skipped_voxels
                    );
                }
                for region in &report.regions {
                    println!(
                        "region {}: mean {:.4e}, mc σ {:?}, systematic σ {:.4e}, combined σ {:?}",
                        region.region,
                        region.mean_dose,
                        region.monte_carlo_1sigma,
                        region.systematic_1sigma,
                        region.combined_1sigma
                    );
                }
            }
            UqCommand::Apply {
                dose,
                boron_field,
                relative,
                positioning_sigma_mm,
                positioning_registration,
                masks,
                id,
                output,
            } => {
                let bundle: PhysicalDoseBundle = serde_json::from_slice(&fs::read(&dose)?)?;
                bundle
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let n = bundle
                    .geometry
                    .voxel_count()
                    .map_err(|error| io::Error::other(error.to_string()))?;

                let component_volume = |component: openbnct_core::DoseComponent| {
                    bundle
                        .components
                        .iter()
                        .find(|volume| volume.component == component)
                        .map(|volume| volume.values.as_slice())
                };

                let mut sources: Vec<openbnct_core::UncertaintySource> = Vec::new();
                let mut maps: Vec<Vec<f64>> = Vec::new();
                let mut summaries: Vec<openbnct_core::SourceSummary> = Vec::new();

                if let Some(field_path) = &boron_field {
                    let field: openbnct_boron::BoronField =
                        serde_json::from_slice(&fs::read(field_path)?)?;
                    field
                        .validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    if field.geometry != bundle.geometry {
                        return Err(io::Error::other(
                            "boron field geometry does not match the dose bundle grid",
                        )
                        .into());
                    }
                    if field.case_id != bundle.case_id {
                        return Err(io::Error::other(format!(
                            "boron field case {:?} does not match dose bundle case {:?}",
                            field.case_id, bundle.case_id
                        ))
                        .into());
                    }
                    let boron =
                        component_volume(openbnct_core::DoseComponent::Boron).ok_or_else(|| {
                            io::Error::other("dose bundle carries no boron component")
                        })?;
                    let (map, skipped) = openbnct_core::boron_field_sigma(
                        boron,
                        &field.values,
                        &field.uncertainty_1sigma,
                    );
                    summaries.push(openbnct_core::summarize_source(
                        "boron_concentration",
                        &map,
                        skipped,
                    ));
                    maps.push(map);
                    sources.push(openbnct_core::UncertaintySource::BoronConcentration {
                        field: openbnct_core::ContentReference {
                            id: field.id.clone(),
                            sha256: openbnct_evidence::sha256_file(field_path)?,
                        },
                    });
                }

                for spec in &relative {
                    let (name, sigma_text) = spec.split_once('=').ok_or_else(|| {
                        io::Error::other(format!(
                            "--relative {spec:?} must be written as component=sigma"
                        ))
                    })?;
                    let component = parse_component_name(name)?;
                    let sigma: f64 = sigma_text.parse().map_err(|_| {
                        io::Error::other(format!("--relative {spec:?}: sigma is not a number"))
                    })?;
                    if !sigma.is_finite() || sigma < 0.0 {
                        return Err(io::Error::other(format!(
                            "--relative {spec:?}: sigma must be a non-negative finite value"
                        ))
                        .into());
                    }
                    let dose_values = component_volume(component).ok_or_else(|| {
                        io::Error::other(format!("dose bundle carries no {name:?} component"))
                    })?;
                    let map = openbnct_core::relative_component_sigma(dose_values, sigma);
                    summaries.push(openbnct_core::summarize_source(
                        &format!("relative_component:{name}"),
                        &map,
                        0,
                    ));
                    maps.push(map);
                    sources.push(openbnct_core::UncertaintySource::RelativeComponent {
                        component: name.to_string(),
                        relative_1sigma: sigma,
                    });
                }

                if positioning_sigma_mm.is_some() || positioning_registration.is_some() {
                    let registration_doc: Option<openbnct_core::Registration> =
                        positioning_registration
                            .as_ref()
                            .map(|path| -> Result<openbnct_core::Registration, io::Error> {
                                let doc: openbnct_core::Registration =
                                    serde_json::from_slice(&fs::read(path)?)?;
                                doc.validate()
                                    .map_err(|error| io::Error::other(error.to_string()))?;
                                Ok(doc)
                            })
                            .transpose()?;
                    let sigma_mm = match (positioning_sigma_mm, &registration_doc) {
                        (Some(sigma), _) => sigma,
                        (None, Some(doc)) => doc.rms_residual_mm.ok_or_else(|| {
                            io::Error::other(
                                "registration carries no RMS residual; pass --positioning-sigma-mm",
                            )
                        })?,
                        (None, None) => unreachable!(),
                    };
                    if !sigma_mm.is_finite() || sigma_mm < 0.0 {
                        return Err(io::Error::other(
                            "positioning sigma must be a non-negative finite value",
                        )
                        .into());
                    }
                    let map = openbnct_core::positioning_sigma(
                        &bundle.physical_total.values,
                        &bundle.geometry,
                        sigma_mm,
                    );
                    summaries.push(openbnct_core::summarize_source("positioning", &map, 0));
                    maps.push(map);
                    sources.push(openbnct_core::UncertaintySource::Positioning {
                        sigma_mm,
                        registration: image_reference(&positioning_registration)?,
                    });
                }

                if sources.is_empty() {
                    return Err(io::Error::other(
                        "no systematic sources declared (--boron-field, --relative, --positioning-*)",
                    )
                    .into());
                }

                let systematic = openbnct_core::combine_voxel_sigma(&maps);
                let combined = openbnct_core::combine_total_sigma(
                    bundle
                        .physical_total
                        .absolute_standard_uncertainty
                        .as_deref(),
                    &systematic,
                );

                let mask_list = load_named_masks(&masks)?;
                for mask in &mask_list {
                    if mask.voxels.len() != n {
                        return Err(io::Error::other(format!(
                            "mask {}: {} voxels, grid expects {}",
                            mask.name,
                            mask.voxels.len(),
                            n
                        ))
                        .into());
                    }
                }
                let regions = mask_list
                    .iter()
                    .map(|mask| {
                        openbnct_core::region_uncertainty(
                            &mask.name,
                            &bundle.physical_total.values,
                            bundle
                                .physical_total
                                .absolute_standard_uncertainty
                                .as_deref(),
                            &maps,
                            mask,
                        )
                    })
                    .collect();

                let report = openbnct_core::SystematicUncertaintyReport {
                    schema_version: openbnct_core::SYSTEMATIC_UNCERTAINTY_SCHEMA.into(),
                    id,
                    dose_bundle: openbnct_core::ContentReference {
                        id: bundle.provenance_id.clone(),
                        sha256: openbnct_evidence::sha256_file(&dose)?,
                    },
                    quantity: "physical_total".into(),
                    sources,
                    source_summaries: summaries,
                    systematic_1sigma: systematic,
                    combined_1sigma: combined,
                    regions,
                    qualification: openbnct_core::SYSTEMATIC_UNCERTAINTY_QUALIFICATION.into(),
                    provenance_id: format!("systematic-uncertainty:{}", bundle.case_id),
                };
                report
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &report)?;
                println!("systematic-uncertainty report at {}", output.display());
                println!("sources: {}", report.sources.len());
                let sys_max = report
                    .systematic_1sigma
                    .iter()
                    .copied()
                    .fold(0.0_f64, f64::max);
                println!("max per-voxel systematic σ: {sys_max:.4e}");
            }
        },
        Some(Command::Vr(args)) => match args.command {
            VrCommand::Resolve {
                spec,
                run,
                id,
                output,
            } => {
                let spec_bytes = fs::read(&spec)?;
                let spec_doc: openbnct_transport::VarianceReductionSpec =
                    serde_json::from_slice(&spec_bytes)?;
                spec_doc
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let spec_sha256 = openbnct_evidence::sha256_file(&spec)?;
                let statepoint = run
                    .as_ref()
                    .map(|dir| {
                        let path = openbnct_openmc::latest_statepoint(dir)
                            .map_err(|e| io::Error::other(format!("{}: {e}", dir.display())))?;
                        let sp = openbnct_openmc::OpenMcStatepoint::open(&path)
                            .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
                        let sha = openbnct_evidence::sha256_file(&path)?;
                        Ok::<_, io::Error>((sp, sha, path))
                    })
                    .transpose()?;
                let resolved = openbnct_openmc::resolve_weight_windows(
                    &spec_doc,
                    &spec_sha256,
                    &id,
                    statepoint
                        .as_ref()
                        .map(|(sp, sha, path)| (sp, sha.as_str(), path.as_path())),
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &resolved)?;
                println!("resolved weight windows at {}", output.display());
                println!("method: {}", resolved.derivation.method);
                for (index, window) in resolved.windows.iter().enumerate() {
                    let active = window.lower_bounds.iter().filter(|v| **v >= 0.0).count();
                    println!(
                        "window {index}: {:?} {}x{}x{} mesh, {}/{} cells active",
                        window.particle,
                        window.mesh.dimensions[0],
                        window.mesh.dimensions[1],
                        window.mesh.dimensions[2],
                        active,
                        window.lower_bounds.len()
                    );
                }
            }
            VrCommand::Cadis {
                spec,
                case,
                data,
                forward_flux,
                assignment,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                id,
                output,
                adjoint_flux,
            } => {
                let spec_bytes = fs::read(&spec)?;
                let spec_doc: openbnct_transport::VarianceReductionSpec =
                    serde_json::from_slice(&spec_bytes)?;
                spec_doc
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let spec_sha256 = openbnct_evidence::sha256_file(&spec)?;
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let assignment_model = match &assignment {
                    Some(path) => Some(serde_json::from_slice::<MaterialAssignment>(&fs::read(
                        path,
                    )?)?),
                    None => None,
                };
                let mut periodic_axes = [false; 3];
                for axis in &periodic {
                    let index = match axis.as_str() {
                        "x" => 0,
                        "y" => 1,
                        "z" => 2,
                        other => {
                            return Err(io::Error::other(format!(
                                "periodic axis {other:?} must be x, y, or z"
                            ))
                            .into());
                        }
                    };
                    periodic_axes[index] = true;
                }
                let options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model,
                    periodic: periodic_axes,
                    beam_uncollided_split: true,
                    transport_correction: true,
                };
                let forward = forward_flux
                    .as_ref()
                    .map(|path| {
                        let bytes = fs::read(path)?;
                        let flux: openbnct_transport::MultigroupFlux =
                            serde_json::from_slice(&bytes)?;
                        let reference = openbnct_core::ContentReference {
                            id: flux.provenance_id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&bytes),
                        };
                        Ok::<_, io::Error>((flux, reference))
                    })
                    .transpose()?;
                let data_ref = openbnct_core::ContentReference {
                    id: mg_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let case_ref = openbnct_core::ContentReference {
                    id: transport_case.case_id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&case_bytes),
                };
                let mut derivation = openbnct_transport::resolve_adjoint_windows(
                    &spec_doc,
                    &spec_sha256,
                    &id,
                    &transport_case,
                    &mg_data,
                    &options,
                    forward.as_ref().map(|(f, r)| (f, r.clone())),
                    data_ref,
                    case_ref,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                if let Some(prefix) = &adjoint_flux {
                    for (index, flux) in derivation.adjoint_fluxes.iter().enumerate() {
                        let path = PathBuf::from(format!("{}.{}.json", prefix.display(), index));
                        write_new_json(&path, flux)?;
                        let bytes = fs::read(&path)?;
                        derivation.resolved.derivation.adjoint_flux.push(
                            openbnct_core::ContentReference {
                                id: flux.provenance_id.clone(),
                                sha256: openbnct_evidence::sha256_hex(&bytes),
                            },
                        );
                        println!("adjoint flux {index} at {}", path.display());
                    }
                }
                derivation
                    .resolved
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &derivation.resolved)?;
                let summary = openbnct_transport::summarize_adjoint_derivation(&derivation);
                println!("resolved weight windows at {}", output.display());
                println!("method: {}", derivation.resolved.derivation.method);
                println!(
                    "adjoint solves: {} (all converged: {})",
                    summary.adjoint_solves, summary.all_converged
                );
                for (index, window) in derivation.resolved.windows.iter().enumerate() {
                    let active = window.lower_bounds.iter().filter(|v| **v >= 0.0).count();
                    println!(
                        "window {index}: {:?} {}x{}x{} mesh, {}/{} cells active",
                        window.particle,
                        window.mesh.dimensions[0],
                        window.mesh.dimensions[1],
                        window.mesh.dimensions[2],
                        active,
                        window.lower_bounds.len()
                    );
                }
            }
            VrCommand::Info { document } => {
                let bytes = fs::read(&document)?;
                let schema: serde_json::Value = serde_json::from_slice(&bytes)?;
                match schema["schema_version"].as_str() {
                    Some(openbnct_transport::VARIANCE_REDUCTION_SCHEMA) => {
                        let spec: openbnct_transport::VarianceReductionSpec =
                            serde_json::from_slice(&bytes)?;
                        spec.validate()
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        println!("variance-reduction spec {}", spec.id);
                        println!("windows: {}", spec.windows.len());
                        println!("qualification: {}", spec.qualification);
                    }
                    Some(openbnct_transport::WEIGHT_WINDOWS_SCHEMA) => {
                        let resolved: openbnct_transport::ResolvedWeightWindows =
                            serde_json::from_slice(&bytes)?;
                        resolved
                            .validate()
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        println!("resolved weight windows {}", resolved.id);
                        println!("spec: {}", resolved.spec.id);
                        println!("method: {}", resolved.derivation.method);
                        for (index, window) in resolved.windows.iter().enumerate() {
                            let active = window.lower_bounds.iter().filter(|v| **v >= 0.0).count();
                            println!(
                                "window {index}: {:?}, {}/{} cells active",
                                window.particle,
                                active,
                                window.lower_bounds.len()
                            );
                        }
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "unrecognized schema {other:?}; expected a variance-reduction \
                             or weight-windows document"
                        ))
                        .into());
                    }
                }
            }
            VrCommand::Validate {
                vr_run,
                exit_code,
                reference_report,
                reference_histories,
                z_limit,
                output,
            } => {
                let reference_bytes = fs::read(&reference_report)?;
                let report = openbnct_openmc::validate_variance_reduction(
                    &vr_run,
                    exit_code,
                    &reference_bytes,
                    reference_histories,
                    z_limit,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &report)?;
                println!("vr-validation report at {}", output.display());
                println!(
                    "histories: {} vs reference {} (x{:.1} reduction)",
                    report.vr_run.histories,
                    report.reference.histories,
                    report.history_reduction_factor
                );
                println!(
                    "unbiased: {} ({} comparisons, z <= {})",
                    report.unbiased,
                    report.comparisons.len(),
                    z_limit
                );
                if !report.skipped.is_empty() {
                    println!("skipped unpaired groups: {}", report.skipped.len());
                    for group in &report.skipped {
                        println!("  {} / {} — {}", group.region, group.tally, group.reason);
                    }
                }
                println!("vr acceptance gates passed: {}", report.vr_gates_passed);
                let worst = report
                    .comparisons
                    .iter()
                    .map(|c| c.z_score)
                    .fold(0.0_f64, f64::max);
                println!("max z-score: {worst:.2}");
                println!("gates passed: {}", report.gates_passed);
            }
        },
        Some(Command::Position(args)) => match args.command {
            PositionCommand::Aim {
                case,
                source,
                mask,
                approach,
                direction,
                half_widths_cm,
                margin_cm,
                output_source,
                output_report,
            } => {
                let case: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                let template: openbnct_transport::FixedSourceDefinition =
                    serde_json::from_slice(&fs::read(&source)?)?;
                let mask = read_region_mask(&mask)?;
                if half_widths_cm.len() != 2 {
                    return Err(
                        io::Error::other("--half-widths-cm must be HU,HV (two values)").into(),
                    );
                }
                let direction = if let Some(text) = &direction {
                    let parts: Vec<f64> = text
                        .split(',')
                        .map(|part| {
                            part.trim().parse().map_err(|_| {
                                io::Error::other(format!(
                                    "--direction component {part:?} is not a number"
                                ))
                            })
                        })
                        .collect::<Result<_, _>>()?;
                    if parts.len() != 3 {
                        return Err(io::Error::other(
                            "--direction must be dx,dy,dz (three components)",
                        )
                        .into());
                    }
                    [parts[0], parts[1], parts[2]]
                } else {
                    openbnct_transport::AxisApproach::parse(approach.as_deref().unwrap_or_default())
                        .map_err(|error| io::Error::other(error.to_string()))?
                        .unit_vector()
                };
                let (positioned, mut report) = openbnct_transport::aim_source_at_centroid(
                    &template,
                    &case.geometry,
                    &mask,
                    direction,
                    [half_widths_cm[0], half_widths_cm[1]],
                    margin_cm,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                report.case_id = case.case_id.clone();
                write_new_json(&output_source, &positioned)?;
                write_new_json(&output_report, &report)?;
                println!("positioned source at {}", output_source.display());
                println!(
                    "target {:?} centroid LPS [{:.3}, {:.3}, {:.3}] mm",
                    report.target_region,
                    report.target_centroid_lps_mm[0],
                    report.target_centroid_lps_mm[1],
                    report.target_centroid_lps_mm[2]
                );
                println!(
                    "entry {:?}/{:?} at [{:.3}, {:.3}, {:.3}] mm, source-to-centroid {:.3} mm",
                    report.entry_axis,
                    report.entry_side,
                    report.entry_point_lps_mm[0],
                    report.entry_point_lps_mm[1],
                    report.entry_point_lps_mm[2],
                    report.source_to_centroid_mm
                );
            }
            PositionCommand::Rotate {
                source,
                axis,
                degrees,
                center_mm,
                output_source,
            } => {
                let source: openbnct_transport::FixedSourceDefinition =
                    serde_json::from_slice(&fs::read(&source)?)?;
                let axis = match axis.as_str() {
                    "x" => openbnct_transport::PlaneAxis::X,
                    "y" => openbnct_transport::PlaneAxis::Y,
                    "z" => openbnct_transport::PlaneAxis::Z,
                    other => {
                        return Err(io::Error::other(format!(
                            "--axis must be x|y|z, got {other:?}"
                        ))
                        .into());
                    }
                };
                let rotated = openbnct_transport::rotate_source(
                    &source,
                    [center_mm[0], center_mm[1], center_mm[2]],
                    axis,
                    degrees,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output_source, &rotated)?;
                println!("rotated source at {}", output_source.display());
            }
        },
        None => {
            println!("NCTForge research scaffold");
            println!("Not commissioned or certified for clinical use.");
        }
    }
    Ok(())
}

fn parse_plane_axis(value: &str) -> io::Result<openbnct_transport::PlaneAxis> {
    match value {
        "x" => Ok(openbnct_transport::PlaneAxis::X),
        "y" => Ok(openbnct_transport::PlaneAxis::Y),
        "z" => Ok(openbnct_transport::PlaneAxis::Z),
        other => Err(io::Error::other(format!(
            "--axis must be x|y|z, got {other:?}"
        ))),
    }
}

fn parse_f64_pair(value: &str) -> io::Result<[f64; 2]> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 2 {
        return Err(io::Error::other(format!(
            "expected `u,v` pair, got {value:?}"
        )));
    }
    let parse = |s: &str| {
        s.trim()
            .parse::<f64>()
            .map_err(|_| io::Error::other(format!("non-numeric coordinate {s:?}")))
    };
    Ok([parse(parts[0])?, parse(parts[1])?])
}

fn write_new_text(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// Parse `B=w,H=w,N=w,P=w` component effectiveness weights for beam-QA
/// in-phantom metrics.
fn parse_component_weights(
    raw: &str,
    flag: &str,
) -> Result<openbnct_transport::ComponentWeights, io::Error> {
    let mut weights = openbnct_transport::ComponentWeights {
        boron: 0.0,
        hydrogen: 0.0,
        nitrogen: 0.0,
        photon: 0.0,
    };
    let mut seen = 0_u8;
    for pair in raw.split(',') {
        let (name, value) = pair.trim().split_once('=').ok_or_else(|| {
            io::Error::other(format!("{flag}: expected B=w,H=w,N=w,P=w, got {pair:?}"))
        })?;
        let value: f64 = value
            .trim()
            .parse()
            .map_err(|_| io::Error::other(format!("{flag}: non-numeric weight in {pair:?}")))?;
        if !value.is_finite() || value < 0.0 {
            return Err(io::Error::other(format!(
                "{flag}: weight must be finite >= 0"
            )));
        }
        match name.trim().to_ascii_uppercase().as_str() {
            "B" => weights.boron = value,
            "H" => weights.hydrogen = value,
            "N" => weights.nitrogen = value,
            "P" => weights.photon = value,
            other => {
                return Err(io::Error::other(format!(
                    "{flag}: unknown component {other:?} (B/H/N/P)"
                )));
            }
        }
        seen += 1;
    }
    if seen != 4 {
        return Err(io::Error::other(format!(
            "{flag}: all four components required (B,H,N,P)"
        )));
    }
    Ok(weights)
}

fn print_beam_summary(beam: &openbnct_transport::BeamDescription) {
    println!("beam: {}", beam.id);
    println!("name: {}", beam.name);
    println!("facility: {}", beam.facility);
    println!(
        "port: {:?} axis at {} cm",
        beam.port.axis, beam.port.offset_cm
    );
    match &beam.port.shape {
        openbnct_transport::PortShape::Circle {
            center_uv_cm,
            radius_cm,
        } => println!("  shape: circle r={radius_cm} cm at {center_uv_cm:?}"),
        openbnct_transport::PortShape::Rectangle {
            u_range_cm,
            v_range_cm,
        } => println!("  shape: rectangle {u_range_cm:?} x {v_range_cm:?} cm"),
    }
    match &beam.normalization {
        openbnct_transport::NormalizationBasis::PerSourceParticle => {
            println!("normalization: per source particle")
        }
        openbnct_transport::NormalizationBasis::FluenceRateAtPort { fluence_rate_cm2_s } => {
            println!("normalization: {fluence_rate_cm2_s} cm^-2 s^-1 at port")
        }
    }
    let (kind, citations) = match &beam.provenance {
        openbnct_transport::BeamProvenance::PublishedLiterature { citations, .. } => {
            ("published literature", citations)
        }
        openbnct_transport::BeamProvenance::MeasuredCharacterization { citations, .. } => {
            ("measured characterization", citations)
        }
        openbnct_transport::BeamProvenance::ComputedModel {
            generator,
            derivation_note,
        } => {
            println!("provenance: computed model");
            println!("  generator: {} sha256:{}", generator.id, generator.sha256);
            println!("  derivation: {derivation_note}");
            return;
        }
    };
    println!("provenance: {kind}");
    for citation in citations {
        println!(
            "  {} ({}), {} — {}",
            citation.authors, citation.year, citation.venue, citation.title
        );
        if let Some(doi) = &citation.doi {
            println!("    doi:{doi}");
        }
    }
}

fn write_new_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

/// Content-bind an image file for a registration record: the file's
/// name as id and a SHA-256 of its bytes.
fn image_reference(
    path: &Option<PathBuf>,
) -> Result<Option<openbnct_core::ContentReference>, io::Error> {
    path.as_ref()
        .map(|path| {
            let sha256 = openbnct_evidence::sha256_file(path)?;
            let id = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Ok(openbnct_core::ContentReference { id, sha256 })
        })
        .transpose()
}

fn load_named_masks(pairs: &[String]) -> Result<Vec<RegionMask>, io::Error> {
    pairs
        .iter()
        .map(|pair| {
            let (name, path) = pair.split_once('=').ok_or_else(|| {
                io::Error::other(format!("region mask {pair:?} must be written as name=path"))
            })?;
            let mask: RegionMask = read_region_mask(Path::new(path))?;
            if mask.name != name {
                return Err(io::Error::other(format!(
                    "region mask {path} is named {:?}, expected {name:?}",
                    mask.name
                )));
            }
            Ok(mask)
        })
        .collect()
}

fn read_region_mask(path: &Path) -> Result<RegionMask, io::Error> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))
}

fn write_mask(mask: &RegionMask, output: &Path) -> io::Result<()> {
    write_new_json(output, mask)?;
    println!(
        "mask {} written ({} voxels included)",
        mask.name,
        mask.included_voxel_count()
    );
    Ok(())
}

fn fold_region_masks(
    inputs: &[PathBuf],
    name: &str,
    op: fn(&RegionMask, &RegionMask, String) -> Result<RegionMask, openbnct_core::ValidationError>,
) -> Result<RegionMask, io::Error> {
    if inputs.len() < 2 {
        return Err(io::Error::other("at least two --inputs are required"));
    }
    let mut mask = read_region_mask(&inputs[0])?;
    for path in &inputs[1..] {
        mask = op(&mask, &read_region_mask(path)?, name.to_owned())
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    Ok(mask)
}

/// A loaded physical or biological dose bundle, resolved by schema.
enum DoseBundle {
    Physical(PhysicalDoseBundle),
    Biological(openbnct_bio::BiologicalDoseBundle),
}

/// Load a dose bundle whose `schema_version` is a known dose contract.
fn load_dose_bundle(bytes: &[u8]) -> Result<DoseBundle, Box<dyn Error>> {
    let schema: serde_json::Value = serde_json::from_slice(bytes)?;
    match schema
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
    {
        openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
            Ok(DoseBundle::Physical(serde_json::from_slice(bytes)?))
        }
        openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
            Ok(DoseBundle::Biological(serde_json::from_slice(bytes)?))
        }
        other => Err(io::Error::other(format!("unsupported dose bundle schema {other:?}")).into()),
    }
}

/// A dose quantity resolved out of a loaded bundle.
struct DoseSelection<'a> {
    case_id: &'a str,
    values: &'a [f64],
    unit: &'a str,
    voxel_volume_mm3: f64,
}

impl DoseBundle {
    /// Resolve the requested quantity's values and metadata.
    fn select(&self, quantity: &str) -> Result<DoseSelection<'_>, Box<dyn Error>> {
        match self {
            Self::Physical(bundle) => {
                let (values, unit) = dose_values(bundle, quantity)?;
                Ok(DoseSelection {
                    case_id: &bundle.case_id,
                    values,
                    unit,
                    voxel_volume_mm3: bundle.geometry.spacing_mm.iter().product(),
                })
            }
            Self::Biological(bundle) => {
                let (values, unit) = biological_dose_values(bundle, quantity)?;
                Ok(DoseSelection {
                    case_id: &bundle.case_id,
                    values,
                    unit,
                    voxel_volume_mm3: bundle.geometry.spacing_mm.iter().product(),
                })
            }
        }
    }
}

fn dose_values<'a>(
    bundle: &'a PhysicalDoseBundle,
    quantity: &str,
) -> Result<(&'a [f64], &'a str), Box<dyn Error>> {
    if let Some(name) = quantity.strip_prefix("component:") {
        let component = match name {
            "boron" => openbnct_core::DoseComponent::Boron,
            "nitrogen" => openbnct_core::DoseComponent::Nitrogen,
            "hydrogen" => openbnct_core::DoseComponent::Hydrogen,
            "photon" => openbnct_core::DoseComponent::Photon,
            other => {
                return Err(io::Error::other(format!("unknown dose component {other:?}")).into());
            }
        };
        let volume = bundle
            .components
            .iter()
            .find(|v| v.component == component)
            .ok_or_else(|| io::Error::other(format!("bundle lacks component {name}")))?;
        let unit = match volume.unit {
            openbnct_core::DoseUnit::Gray => "gray",
            openbnct_core::DoseUnit::GrayPerSourceParticle => "gray_per_source_particle",
        };
        return Ok((&volume.values, unit));
    }
    if quantity == "physical_total" {
        let unit = match bundle.physical_total.unit {
            openbnct_core::DoseUnit::Gray => "gray",
            openbnct_core::DoseUnit::GrayPerSourceParticle => "gray_per_source_particle",
        };
        return Ok((&bundle.physical_total.values, unit));
    }
    Err(io::Error::other(format!("unknown physical quantity {quantity:?}")).into())
}

fn biological_dose_values<'a>(
    bundle: &'a openbnct_bio::BiologicalDoseBundle,
    quantity: &str,
) -> Result<(&'a [f64], &'a str), Box<dyn Error>> {
    if let Some(name) = quantity.strip_prefix("component:") {
        let component = match name {
            "boron" => openbnct_core::DoseComponent::Boron,
            "nitrogen" => openbnct_core::DoseComponent::Nitrogen,
            "hydrogen" => openbnct_core::DoseComponent::Hydrogen,
            "photon" => openbnct_core::DoseComponent::Photon,
            other => {
                return Err(io::Error::other(format!("unknown dose component {other:?}")).into());
            }
        };
        let volume = bundle
            .components
            .iter()
            .find(|v| v.component == component)
            .ok_or_else(|| io::Error::other(format!("bundle lacks component {name}")))?;
        return Ok((&volume.values, &volume.unit));
    }
    if quantity == "biological_total" {
        return Ok((&bundle.total.values, &bundle.total.unit));
    }
    Err(io::Error::other(format!("unknown biological quantity {quantity:?}")).into())
}

fn bytes_to_gib(bytes: u64) -> f64 {
    bytes as f64 / 1024.0_f64.powi(3)
}

fn parse_dose_unit(token: &str) -> Result<openbnct_core::DoseUnit, Box<dyn Error>> {
    match token {
        "gray" => Ok(openbnct_core::DoseUnit::Gray),
        "gray_per_source_particle" => Ok(openbnct_core::DoseUnit::GrayPerSourceParticle),
        other => Err(io::Error::other(format!("unknown dose unit {other:?}")).into()),
    }
}

fn parse_component_name(name: &str) -> Result<openbnct_core::DoseComponent, Box<dyn Error>> {
    match name {
        "boron" => Ok(openbnct_core::DoseComponent::Boron),
        "nitrogen" => Ok(openbnct_core::DoseComponent::Nitrogen),
        "hydrogen" => Ok(openbnct_core::DoseComponent::Hydrogen),
        "photon" => Ok(openbnct_core::DoseComponent::Photon),
        other => Err(io::Error::other(format!("unknown dose component {other:?}")).into()),
    }
}

/// `name=meshtal-path:tally[:energy-bin]` — fields split off the right so
/// paths may contain colons; with two trailing numeric fields the last is the
/// energy bin.
fn parse_mcnp_components(
    specs: &[String],
) -> Result<Vec<openbnct_mcnp::ComponentSource>, Box<dyn Error>> {
    let mut sources = Vec::new();
    for spec in specs {
        let (name, rest) = spec
            .split_once('=')
            .ok_or_else(|| io::Error::other(format!("component spec {spec:?} lacks `=`")))?;
        let component = parse_component_name(name)?;
        let (rest, last) = rest
            .rsplit_once(':')
            .ok_or_else(|| io::Error::other(format!("component spec {spec:?} lacks :TALLY")))?;
        let (file, tally, energy_bin) = match rest.rsplit_once(':') {
            Some((path, mid))
                if mid.parse::<u32>().is_ok()
                    && !path.is_empty()
                    && last.parse::<usize>().is_ok() =>
            {
                (
                    PathBuf::from(path),
                    mid.parse::<u32>().unwrap(),
                    last.parse::<usize>().ok(),
                )
            }
            _ => {
                let tally = last
                    .parse::<u32>()
                    .map_err(|_| io::Error::other(format!("{spec:?}: tally is not a number")))?;
                (PathBuf::from(rest), tally, None)
            }
        };
        sources.push(openbnct_mcnp::ComponentSource {
            component,
            file,
            tally,
            energy_bin,
        });
    }
    Ok(sources)
}

/// `name=phits-output-path[:energy-index]`.
fn parse_phits_components(
    specs: &[String],
) -> Result<Vec<openbnct_phits::ComponentSource>, Box<dyn Error>> {
    let mut sources = Vec::new();
    for spec in specs {
        let (name, rest) = spec
            .split_once('=')
            .ok_or_else(|| io::Error::other(format!("component spec {spec:?} lacks `=`")))?;
        let component = parse_component_name(name)?;
        let (file, energy_bin) = match rest.rsplit_once(':') {
            Some((path, tail)) if tail.parse::<usize>().is_ok() && !path.is_empty() => {
                (PathBuf::from(path), tail.parse::<usize>().ok())
            }
            _ => (PathBuf::from(rest), None),
        };
        sources.push(openbnct_phits::ComponentSource {
            component,
            file,
            energy_bin,
        });
    }
    Ok(sources)
}

fn parse_nifti_components(
    specs: &[String],
    sigma_specs: &[String],
) -> Result<Vec<openbnct_nifti::NiftiComponentSource>, Box<dyn Error>> {
    let mut sigma_files = std::collections::BTreeMap::new();
    for spec in sigma_specs {
        let (name, path) = spec
            .split_once('=')
            .ok_or_else(|| io::Error::other(format!("component-sigma {spec:?} lacks `=`")))?;
        sigma_files.insert(parse_component_name(name)?, PathBuf::from(path));
    }
    let mut sources = Vec::new();
    for spec in specs {
        let (name, path) = spec
            .split_once('=')
            .ok_or_else(|| io::Error::other(format!("component spec {spec:?} lacks `=`")))?;
        let component = parse_component_name(name)?;
        sources.push(openbnct_nifti::NiftiComponentSource {
            component,
            file: PathBuf::from(path),
            sigma_file: sigma_files.remove(&component),
        });
    }
    if let Some((component, _)) = sigma_files.into_iter().next() {
        return Err(io::Error::other(format!(
            "component-sigma for {component:?} has no matching --component"
        ))
        .into());
    }
    Ok(sources)
}

fn qualification_name(qualification: EndfMf6CapturePhotonBalanceQualification) -> &'static str {
    match qualification {
        EndfMf6CapturePhotonBalanceQualification::MissingCapturePhotonDataRejected => {
            "missing_capture_photon_data_rejected"
        }
        EndfMf6CapturePhotonBalanceQualification::SpectrumNormalizationRejected => {
            "spectrum_normalization_rejected"
        }
        EndfMf6CapturePhotonBalanceQualification::CapturePhotonEnergyBalanceRejected => {
            "capture_photon_energy_balance_rejected"
        }
        EndfMf6CapturePhotonBalanceQualification::CapturePhotonEnergyBalanceCheckedUnreviewed => {
            "capture_photon_energy_balance_checked_unreviewed"
        }
    }
}

fn law7_qualification_name(
    qualification: EndfMf6Law7ImplicitResidualQualification,
) -> &'static str {
    match qualification {
        EndfMf6Law7ImplicitResidualQualification::SpectrumNormalizationRejected => {
            "spectrum_normalization_rejected"
        }
        EndfMf6Law7ImplicitResidualQualification::NegativeImplicitResidualEnergyRejected => {
            "negative_implicit_residual_energy_rejected"
        }
        EndfMf6Law7ImplicitResidualQualification::SpectrumNormalizationAndResidualEnergyRejected => {
            "spectrum_normalization_and_residual_energy_rejected"
        }
        EndfMf6Law7ImplicitResidualQualification::ImplicitResidualEnergyCheckedUnreviewed => {
            "implicit_residual_energy_checked_unreviewed"
        }
    }
}

fn law7_comparison_qualification_name(
    qualification: NjoyLaw7ImplicitResidualComparisonQualification,
) -> &'static str {
    match qualification {
        NjoyLaw7ImplicitResidualComparisonQualification::
            ProcessorApproximationFullyAttributedUnreviewed => {
                "processor_approximation_fully_attributed_unreviewed"
            }
        NjoyLaw7ImplicitResidualComparisonQualification::ProcessorAttributionRejected => {
            "processor_attribution_rejected"
        }
    }
}

fn energy_balance_attribution_qualification_name(
    qualification: NjoyEnergyBalanceAttributionQualification,
) -> &'static str {
    match qualification {
        NjoyEnergyBalanceAttributionQualification::
            ProcessorAccountingMechanismAttributedPhysicalValidationRequired => {
                "processor_accounting_mechanism_attributed_physical_validation_required"
            }
        NjoyEnergyBalanceAttributionQualification::ProcessorAccountingAttributionMismatch => {
            "processor_accounting_attribution_mismatch"
        }
    }
}

fn reaction_balance_qualification_name(
    qualification: EndfReactionBalanceQualification,
) -> &'static str {
    match qualification {
        EndfReactionBalanceQualification::SourceRemaindersComputedUnreviewed => {
            "source_remainders_computed_unreviewed"
        }
        EndfReactionBalanceQualification::SourceRemaindersPartiallyComputable => {
            "source_remainders_partially_computable"
        }
    }
}

/// Owned artifacts backing `NjoyResponseTableInputs`; the borrowed view is
/// built by `inputs()` once everything is loaded.
struct ResponseTableArtifacts {
    material: MaterialDefinition,
    material_bytes: Vec<u8>,
    component_profile: ComponentDefinitionProfile,
    component_profile_bytes: Vec<u8>,
    method: ResponseGenerationMethod,
    method_bytes: Vec<u8>,
    nuclear_data: NuclearDataManifest,
    nuclear_data_bytes: Vec<u8>,
    transport_domain: OpenMcNeutronTransportDomain,
    transport_domain_bytes: Vec<u8>,
    selection: EvaluatedNeutronSourceSelectionDocument,
    domain_aware: NjoyDomainAwareSuitabilityReportDocument,
    execution: NjoyExecutionReceiptDocument,
    execution_directory: PathBuf,
}

impl ResponseTableArtifacts {
    #[allow(clippy::too_many_arguments)]
    fn load(
        material: &Path,
        component_profile: &Path,
        generation_method: &Path,
        nuclear_data_manifest: &Path,
        transport_domain: &Path,
        selection: &Path,
        domain_aware_report: &Path,
        receipt: &Path,
        execution_directory: PathBuf,
    ) -> Result<Self, Box<dyn Error>> {
        let material_bytes = fs::read(material)?;
        let material: MaterialDefinition = serde_json::from_slice(&material_bytes)?;
        let component_profile_bytes = fs::read(component_profile)?;
        let component_profile: ComponentDefinitionProfile =
            serde_json::from_slice(&component_profile_bytes)?;
        let method_bytes = fs::read(generation_method)?;
        let method: ResponseGenerationMethod = serde_json::from_slice(&method_bytes)?;
        let nuclear_data_bytes = fs::read(nuclear_data_manifest)?;
        let nuclear_data: NuclearDataManifest = serde_json::from_slice(&nuclear_data_bytes)?;
        let transport_domain_bytes = fs::read(transport_domain)?;
        let transport_domain: OpenMcNeutronTransportDomain =
            serde_json::from_slice(&transport_domain_bytes)?;
        Ok(Self {
            material,
            material_bytes,
            component_profile,
            component_profile_bytes,
            method,
            method_bytes,
            nuclear_data,
            nuclear_data_bytes,
            transport_domain,
            transport_domain_bytes,
            selection: EvaluatedNeutronSourceSelectionDocument::from_path(selection)?,
            domain_aware: NjoyDomainAwareSuitabilityReportDocument::from_path(domain_aware_report)?,
            execution: NjoyExecutionReceiptDocument::from_path(receipt)?,
            execution_directory,
        })
    }

    fn inputs(&self) -> NjoyResponseTableInputs<'_> {
        NjoyResponseTableInputs {
            material: &self.material,
            material_bytes: &self.material_bytes,
            component_profile: &self.component_profile,
            component_profile_bytes: &self.component_profile_bytes,
            method: &self.method,
            method_bytes: &self.method_bytes,
            nuclear_data: &self.nuclear_data,
            nuclear_data_bytes: &self.nuclear_data_bytes,
            transport_domain: &self.transport_domain,
            transport_domain_bytes: &self.transport_domain_bytes,
            selection: &self.selection,
            domain_aware: &self.domain_aware,
            execution: &self.execution,
            execution_directory: &self.execution_directory,
        }
    }
}
