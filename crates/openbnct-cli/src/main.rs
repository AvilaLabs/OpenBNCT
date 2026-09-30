// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod project;

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
    DT_FLOAT64, Interpolation, NiftiImage, box_average_to_grid, covering_grid, read_nifti_file,
    read_target_geometry, resample_to_grid, write_nifti,
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
    /// One-command golden path: initialize and run a self-contained
    /// project from a CT + RT Structure Set to component dose, boron-scaled
    /// dose, DVHs and a report. Orchestrates the same steps as the
    /// individual commands and records them in a resumable manifest.
    Project(project::ProjectArgs),
    /// Evaluate parametric accelerator-target neutron sources
    /// (`openbnct.accelerator-source/0.1.0`).
    Accelerator(AcceleratorArgs),
    /// Optional Avify Dose engine integration (R12) — export the voxel
    /// plan, run the separately licensed engine, ingest its certificate.
    Avify(AvifyArgs),
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
    /// Derive a 478 keV prompt-gamma production map from a dose bundle's
    /// boron component (`openbnct.prompt-gamma-source/0.1.0`) — the
    /// physics source term BNCT-SPECT/Compton-camera research consumes.
    PromptGamma {
        /// `openbnct.physical-dose-bundle` JSON.
        #[arg(long)]
        dose: PathBuf,
        /// Source document identifier.
        #[arg(long)]
        id: String,
        /// Provenance identifier; defaults to `prompt-gamma:` + the
        /// parent bundle's provenance.
        #[arg(long)]
        provenance_id: Option<String>,
        /// New output path for the prompt-gamma-source JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Prompt-gamma delivery-verification chain: adjoint detector
    /// response maps and the expected-counts forward model. Research
    /// instruments for PG-imaging work — not imaging devices.
    Pg {
        #[command(subcommand)]
        command: PgCommand,
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
    /// Inspect and verify the benchmark/validation evidence catalogue
    /// (`openbnct.benchmark-catalogue/0.1.0`). Read-only — never
    /// modifies the referenced evidence files.
    Bench(BenchArgs),
    /// Import and inspect normalized delivery histories
    /// (`openbnct.delivery-history/0.1.0`). Read-only import — no beam
    /// control, record modification, or upload.
    Delivery(DeliveryArgs),
    /// Reconstruct accumulated physical dose from a recorded delivery
    /// history (`openbnct.dose-replay/0.1.0` spec → reconstructed bundle
    /// + `openbnct.dose-replay-report/0.1.0`).
    Replay(ReplayArgs),
    /// Validate and whitelist-filter dose-to-outcome study exports
    /// (`openbnct.outcomes-export/0.1.0`).
    Outcomes(OutcomesArgs),
    /// Inspect and verify a qualification-readiness record
    /// (`openbnct.qualification-record/0.1.0`) against the benchmark
    /// catalogue.
    Qual(QualArgs),
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
        /// Region limit `NAME=max|mean|dN:LIMIT` in endpoint dose
        /// units (`dN` = `D_x` volume-coverage percent, e.g. `d2`).
        /// repeatable.
        #[arg(long = "limit", required = true)]
        limits: Vec<String>,
        /// RegionMask binding `NAME=path`; repeatable.
        #[arg(long = "mask", required = true)]
        masks: Vec<String>,
        /// PK model JSON (`openbnct.pk-model/0.1.0`): integrates the
        /// boron component under declared regional concentration curves
        /// and solves the implicit beam-off time. Requires
        /// `physical_total`, `biological_total`, or `component:boron`.
        #[arg(long)]
        pk_model: Option<PathBuf>,
        /// PkSamples JSON (`openbnct.pk-samples/0.1.0`) the PK model was
        /// fit from — enables a parametric-bootstrap `t*` interval per
        /// region. Requires `--pk-model`.
        #[arg(long, requires = "pk_model")]
        pk_samples: Option<PathBuf>,
        /// Bootstrap replicates for `--pk-samples` (default 256).
        #[arg(long, requires = "pk_samples", default_value = "256")]
        pk_bootstrap: u32,
        /// New output path for the irradiation-time report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Pharmacokinetic boron-concentration tooling.
    Pk {
        #[command(subcommand)]
        command: PkCommand,
    },
}

/// Pharmacokinetic boron-concentration tooling.
#[derive(Debug, Subcommand)]
enum PkCommand {
    /// Fit per-region exponential concentration curves from measured
    /// draws (`openbnct.pk-model/0.1.0`).
    Fit {
        /// PkSamples JSON (`openbnct.pk-samples/0.1.0`) — measured
        /// concentration draws per region.
        #[arg(long)]
        samples: PathBuf,
        /// Identifier for the emitted PK model artifact.
        #[arg(long)]
        id: String,
        /// Exponential terms to fit per region (1 or 2).
        #[arg(long, default_value = "2")]
        exponentials: usize,
        /// New output path for the fitted PK model JSON
        /// (`openbnct.pk-model/0.1.0`), consumable by
        /// `irradiation-time --pk-model` and `pk schedule`.
        #[arg(long)]
        output: PathBuf,
    },
    /// Fold a declared tumor-to-blood evolution into a blood PK
    /// model — `C_tissue = C_blood·T/B(t)` stays in the exponential
    /// family, so the emitted `openbnct.pk-model` is exact.
    TissueScale {
        /// Blood PK model JSON (`openbnct.pk-model/0.1.0`).
        #[arg(long)]
        blood_model: PathBuf,
        /// T/B evolution spec JSON (`openbnct.pk-tissue-spec/0.1.0`).
        #[arg(long)]
        spec: PathBuf,
        /// Identifier for the emitted tissue PK model artifact.
        #[arg(long)]
        id: String,
        /// New output path for the tissue-scaled PK model JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Irradiation-window search: solve the organ limits at each
    /// declared beam-on epoch and report the deliverable tumor dose —
    /// the PK-vs-fixed gap is explicit per window
    /// (`openbnct.pk-schedule/0.1.0`).
    Schedule {
        /// Physical or biological dose bundle JSON.
        #[arg(long)]
        dose: PathBuf,
        /// `component:NAME`, `physical_total`, or `biological_total`.
        #[arg(long)]
        quantity: String,
        /// Source strength in source particles per second.
        #[arg(long)]
        source_strength: f64,
        /// Region limit `NAME=max|mean|dN:LIMIT` in endpoint dose
        /// units. Repeatable.
        #[arg(long = "limit", required = true)]
        limits: Vec<String>,
        /// RegionMask binding `NAME=path`; repeatable.
        #[arg(long = "mask", required = true)]
        masks: Vec<String>,
        /// PK model JSON (`openbnct.pk-model/0.1.0`) — blood or
        /// tissue-scaled curves over the post-infusion epoch.
        #[arg(long)]
        pk_model: PathBuf,
        /// Beam-on epochs after the curves' epoch zero, seconds —
        /// comma-separated or repeatable.
        #[arg(long = "window-s", required = true, value_delimiter = ',')]
        windows_s: Vec<f64>,
        /// Tumor region the deliverable dose reports.
        #[arg(long)]
        tumor_region: String,
        /// Tumor statistic: `max`, `mean`, or `dN` (default `mean`).
        #[arg(long, default_value = "mean")]
        tumor_metric: String,
        /// New output path for the schedule report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Emit the PK-integrated dose map for one beam-on window as a
    /// real `openbnct.physical-dose-bundle` in Gray — directly
    /// consumable by `bio apply` and `dvh`.
    Dose {
        /// Physical dose bundle JSON (per-source-particle rates).
        #[arg(long)]
        dose: PathBuf,
        /// PK model JSON (`openbnct.pk-model/0.1.0`).
        #[arg(long)]
        pk_model: PathBuf,
        /// RegionMask binding `NAME=path`; repeatable — first match
        /// wins per voxel, unmasked voxels keep constant
        /// concentration.
        #[arg(long = "mask")]
        masks: Vec<String>,
        /// Source strength in source particles per second.
        #[arg(long)]
        source_strength: f64,
        /// Schedule report JSON to take the window from — combines
        /// with `--window-index` (mutually exclusive with
        /// `--window-s`/`--time-s`).
        #[arg(long, requires = "window_index")]
        schedule: Option<PathBuf>,
        /// Window index inside `--schedule`.
        #[arg(long, requires = "schedule")]
        window_index: Option<usize>,
        /// Beam-on epoch after infusion end, seconds — combine with
        /// `--time-s` (mutually exclusive with `--schedule`).
        #[arg(long, requires = "time_s", conflicts_with = "schedule")]
        window_s: Option<f64>,
        /// Beam-on duration, seconds.
        #[arg(long, requires = "window_s", conflicts_with = "schedule")]
        time_s: Option<f64>,
        /// Provenance identifier for the emitted bundle; defaults to a
        /// deterministic `pk-dose-*` id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// New output path for the dose bundle JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum PgCommand {
    /// Solve the photon adjoint for a declared detector voxel region:
    /// the per-voxel sensitivity of a fluence-weighted tally on that
    /// region to emissions in the 478 keV group
    /// (`openbnct.pg-response/0.1.0`). One solve = one detector
    /// position's response-matrix column.
    Response {
        /// `openbnct.transport-case/0.1.0` case JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-photon-data/0.1.0` photon data JSON.
        #[arg(long)]
        photon_data: PathBuf,
        /// Detector voxel as `i,j,k` on the case grid; repeatable —
        /// all listed voxels form one detector region.
        #[arg(long)]
        detector: Vec<String>,
        /// Pinhole aperture center in patient mm as `x,y,z`. With
        /// `--aperture-radius-mm`, the adjoint detector source emits
        /// only along ordinates inside the acceptance cone — the
        /// response then carries a real pinhole collimator's spatial
        /// selectivity. Requires `--order` fine enough that ordinates
        /// land inside the cone (the command errors otherwise).
        #[arg(long)]
        aperture: Option<String>,
        /// Pinhole aperture radius in mm — required with `--aperture`.
        #[arg(long, requires = "aperture")]
        aperture_radius_mm: Option<f64>,
        /// Emission photon energy in eV — selects the photon group it
        /// falls in (default: the 478 keV boron line).
        #[arg(long, default_value_t = 478_000.0)]
        emission_energy_ev: f64,
        /// S_N quadrature order (even, 2–16).
        #[arg(long, default_value_t = 8)]
        order: u32,
        /// Relative adjoint-flux convergence target.
        #[arg(long, default_value_t = 1e-6)]
        convergence: f64,
        /// Within-group iterations per group pass.
        #[arg(long, default_value_t = 64)]
        max_inner: u32,
        /// Outer sweeps.
        #[arg(long, default_value_t = 16)]
        max_outer: u32,
        /// Axis treated as periodic; repeatable or comma-separated.
        #[arg(long, value_delimiter = ',')]
        periodic: Vec<String>,
        /// Disable the extended transport correction on the sweep.
        #[arg(long)]
        no_transport_correction: bool,
        /// Solve each `--detector` voxel as its own pixel — one adjoint
        /// per voxel, one `openbnct.pg-response-array/0.1.0` bank with a
        /// response column per pixel. With `--aperture`, each pixel's
        /// acceptance cone axis runs that voxel→aperture.
        #[arg(long)]
        pixellated: bool,
        /// Enable P1 anisotropic scatter in the adjoint solve — requires
        /// `scatter_p1_matrix_per_cm` on every scattering material.
        #[arg(long)]
        p1: bool,
        /// Highest Legendre order beyond P1 (2..=5) in the adjoint
        /// scatter — requires `--p1` and `scatter_legendre_moments_per_cm`.
        #[arg(long, requires = "p1", default_value_t = 0)]
        anisotropy: u32,
        /// Response document identifier.
        #[arg(long)]
        id: String,
        /// Provenance identifier; defaults to `pg-response:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// Output path for the pg-response JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Fold a prompt-gamma emission map against a response map into
    /// the expected detector tally (`openbnct.pg-counts/0.1.0`) — the
    /// forward-model half of the reconstruction chain.
    Counts {
        /// `openbnct.prompt-gamma-source/0.1.0` emission map JSON.
        #[arg(long)]
        emission: PathBuf,
        /// `openbnct.pg-response/0.1.0` response map JSON.
        #[arg(long)]
        response: PathBuf,
        /// Uniform voxel density in kg/m³ converting the per-kg
        /// emission map to volumetric sources.
        #[arg(long)]
        density_kg_per_m3: f64,
        /// Declared detector efficiency calibration applied to the
        /// raw tally (crystal volume, collimation — a scalar, not
        /// modeled transport).
        #[arg(long, default_value_t = 1.0)]
        efficiency: f64,
        /// Counts document identifier.
        #[arg(long)]
        id: String,
        /// Provenance identifier; defaults to `pg-counts:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// Output path for the pg-counts JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Collect counts artifacts into a synthetic observation
    /// (`openbnct.pg-observation/0.1.0`) — the measured half of the
    /// reconstruction problem. Real measurements are authored in the
    /// same schema directly.
    Observe {
        /// `openbnct.pg-counts/0.1.0` JSON; repeatable, at least one.
        #[arg(long)]
        counts: Vec<PathBuf>,
        /// Observation document identifier.
        #[arg(long)]
        id: String,
        /// Provenance identifier; defaults to `pg-observation:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// Output path for the pg-observation JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Reconstruct the per-kg emission map from an observation and the
    /// response artifacts it binds (`openbnct.pg-reconstruction/0.1.0`)
    /// — non-negative least squares with a Tikhonov term.
    Reconstruct {
        /// `openbnct.pg-observation/0.1.0` JSON.
        #[arg(long)]
        observation: PathBuf,
        /// `openbnct.pg-response/0.1.0` JSON, once per observation
        /// detector in the same order; each file's sha256 is verified
        /// against the observation's content binding.
        #[arg(long)]
        response: Vec<PathBuf>,
        /// Uniform voxel density in kg/m³ folding per-kg emissions
        /// into the operator.
        #[arg(long)]
        density_kg_per_m3: f64,
        /// Tikhonov λ applied to the reconstructed map.
        #[arg(long, default_value_t = 1e-4)]
        lambda: f64,
        /// Projected-gradient iteration budget.
        #[arg(long, default_value_t = 2000)]
        max_iterations: u32,
        /// Emission unit the tallies were taken in:
        /// `photons-per-kg-per-source-particle` (default) or
        /// `photons-per-kg`.
        #[arg(long, default_value = "photons-per-kg-per-source-particle")]
        unit: String,
        /// Reconstruction document identifier.
        #[arg(long)]
        id: String,
        /// Provenance identifier; defaults to `pg-reconstruction:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// Output path for the pg-reconstruction JSON.
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
        /// NTCP endpoint-evaluation JSON — repeatable for multi-OAR
        /// P₊ = TCP·Π(1−NTCPᵢ); each may name a different region.
        #[arg(long = "ntcp", required = true)]
        ntcp: Vec<PathBuf>,
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
    /// Record a shared-DICOM-frame-of-reference registration — the
    /// transform is identity by construction; the shared Frame of
    /// Reference UID both series declare is the evidence. The common
    /// case for a PET or MR series co-acquired with the planning CT.
    FrameOfReference {
        /// Registration id.
        #[arg(long)]
        id: String,
        /// Frame of Reference UID both series declare
        /// (DICOM tag 0020,0052).
        #[arg(long)]
        uid: String,
        /// Moving image file whose content hash is bound into the
        /// record.
        #[arg(long)]
        moving: Option<PathBuf>,
        /// Fixed (target) image file whose content hash is bound.
        #[arg(long)]
        fixed: Option<PathBuf>,
        /// Free-text provenance note (acquisition, protocol).
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
    /// Apply a ¹⁰B concentration to a unit-concentration boron dose and
    /// re-total a physical dose bundle (post-hoc boron, trace-¹⁰B
    /// approximation: the applied boron does not perturb the flux).
    ///
    /// The boron component becomes C(v)·u(v); the physical total is
    /// old total − old boron + new boron. Give either a blood
    /// concentration with optional tissue:blood ratio masks, or a
    /// per-voxel `openbnct.boron-field/0.1.0` from `boron apply`.
    Dose {
        /// Physical dose bundle JSON (`openbnct.physical-dose-bundle/0.2.0`)
        /// from the same transport run as the unit dose.
        #[arg(long)]
        physical_bundle: PathBuf,
        /// `openbnct.boron-unit-dose/0.1.0` from `sn solve/fold
        /// --boron-unit-output`.
        #[arg(long)]
        unit_dose: PathBuf,
        /// Blood ¹⁰B concentration in µg/g (with `--ratio`/`--mask`;
        /// mutually exclusive with `--boron-field`).
        #[arg(long)]
        blood_ug_g: Option<f64>,
        /// Tissue:blood ratio `NAME=value`; repeatable. Every ratio
        /// needs a `--mask` of the same name and vice versa.
        #[arg(long = "ratio")]
        ratios: Vec<String>,
        /// RegionMask binding `NAME=path`; repeatable — the first
        /// matching mask wins per voxel.
        #[arg(long = "mask")]
        masks: Vec<String>,
        /// Tissue:blood ratio for voxels covered by no mask.
        #[arg(long, default_value_t = 1.0)]
        default_ratio: f64,
        /// `openbnct.boron-field/0.1.0` (per-voxel µg/g with 1σ); its
        /// σ propagates into the boron and total uncertainty.
        #[arg(long)]
        boron_field: Option<PathBuf>,
        /// New output path for the re-totalled physical dose bundle.
        #[arg(long)]
        output: PathBuf,
    },
    /// Microdistribution model evaluation and measured-data import.
    Microdistribution {
        #[command(subcommand)]
        command: BoronMicrodistributionCommand,
    },
    /// Sequential measurement-informed boron estimation over a declared
    /// low-dimensional state (`openbnct.boron-inference/0.1.0` spec →
    /// `openbnct.boron-inference-report/0.1.0`). Reports unresolved
    /// state directions explicitly rather than a falsely precise map.
    Infer {
        /// `openbnct.boron-inference/0.1.0` spec JSON.
        #[arg(long)]
        spec: PathBuf,
        /// New output path for the report JSON.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum BoronMicrodistributionCommand {
    /// Evaluate a ¹⁰B subcellular microdistribution model: per-compartment
    /// α/⁷Li energy-deposition fractions to the nucleus and the
    /// nucleus-dose factor relative to uniform concentration.
    Evaluate {
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
    /// Reduce a declared
    /// `openbnct.boron-microdistribution-measurement/0.1.0` (assay
    /// compartment fractions or a radial boron-density profile) to the
    /// `openbnct.boron-microdistribution/0.1.0` model document.
    Import {
        /// `openbnct.boron-microdistribution-measurement/0.1.0` JSON.
        #[arg(long)]
        measurement: PathBuf,
        /// Emitted model id — defaults to `"{measurement.id}.model"`.
        #[arg(long)]
        id: Option<String>,
        /// New output path for the
        /// `openbnct.boron-microdistribution/0.1.0` JSON.
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
    /// Evaluate a `openbnct.joint-dose-ensemble-spec/0.1.0` request over a
    /// `openbnct.joint-uncertainty-input/0.1.0` declaration, emitting a
    /// `openbnct.joint-uncertainty-report/0.1.0`. Correlated/shared
    /// sources are drawn once per realization; per-target sources draw
    /// independently.
    Joint {
        /// Physical dose bundle JSON — `gray_per_source_particle` rate
        /// units when the spec requests PK integration.
        #[arg(long)]
        dose: PathBuf,
        /// `openbnct.joint-uncertainty-input/0.1.0` JSON.
        #[arg(long)]
        joint: PathBuf,
        /// `openbnct.joint-dose-ensemble-spec/0.1.0` JSON.
        #[arg(long)]
        spec: PathBuf,
        /// `openbnct.pk-model/0.1.0` JSON — required when the spec's
        /// `pk` block is set.
        #[arg(long)]
        pk_model: Option<PathBuf>,
        /// Region masks as `NAME=path`; repeatable.
        #[arg(long = "mask")]
        masks: Vec<String>,
        /// Report id.
        #[arg(long)]
        id: String,
        /// New output path for the report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate and print a joint-uncertainty report.
    JointInfo {
        /// `openbnct.joint-uncertainty-report/0.1.0` JSON document.
        #[arg(long)]
        report: PathBuf,
    },
    /// Expected value of information: rank proposed measurements by
    /// their exact expected reduction in metric variance under the
    /// declared linear-Gaussian model (`openbnct.voi-evaluation/0.1.0`
    /// spec → `openbnct.voi-report/0.1.0`). Compares research designs;
    /// it does not schedule care or control equipment.
    Voi {
        /// `openbnct.voi-evaluation/0.1.0` spec JSON.
        #[arg(long)]
        spec: PathBuf,
        /// New output path for the report JSON.
        #[arg(long)]
        output: PathBuf,
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
struct BenchArgs {
    #[command(subcommand)]
    command: BenchCommand,
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    /// Validate a `openbnct.benchmark-catalogue/0.1.0` document and
    /// print its entries with status and evidence counts.
    Info {
        /// Catalogue JSON document.
        #[arg(long)]
        catalogue: PathBuf,
    },
    /// Print one catalogue entry's full record — evidence items,
    /// tolerances, limitations.
    Report {
        /// Catalogue JSON document.
        #[arg(long)]
        catalogue: PathBuf,
        /// Catalogue entry id.
        #[arg(long)]
        entry: String,
    },
    /// Read-only verification: resolve every referenced file under
    /// `--root`, re-hash declared artifacts, and report broken
    /// references plus absent license/uncertainty metadata. Exits
    /// nonzero when error-severity findings exist. Optionally writes a
    /// `openbnct.benchmark-report/0.1.0` JSON.
    Verify {
        /// Catalogue JSON document.
        #[arg(long)]
        catalogue: PathBuf,
        /// Repository/catalogue root the entry paths resolve under.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Report id (required with `--output`).
        #[arg(long, requires = "output")]
        id: Option<String>,
        /// New output path for the verification report JSON.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct DeliveryArgs {
    #[command(subcommand)]
    command: DeliveryCommand,
}

#[derive(Debug, Subcommand)]
enum DeliveryCommand {
    /// Import one or more CSV streams into a normalized
    /// `openbnct.delivery-history/0.1.0` document under a declared
    /// `openbnct.delivery-csv-import/0.1.0` mapping spec. Diagnostics
    /// (rejected rows, reordered samples, dropped duplicates, resolved
    /// rollovers) are printed per stream and stay inspectable.
    Import {
        /// Import-mapping spec JSON.
        #[arg(long)]
        spec: PathBuf,
        /// New output path for the delivery-history JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate a `openbnct.delivery-history/0.1.0` document and print
    /// its streams, coverage, gaps, and calibration-window status.
    Info {
        /// Delivery-history JSON document.
        #[arg(long)]
        history: PathBuf,
        /// Optional `[t_start, t_end]` session window for gap analysis
        /// (comma-separated seconds).
        #[arg(long, value_delimiter = ',')]
        window: Option<Vec<f64>>,
    },
}

#[derive(Debug, Args)]
struct ReplayArgs {
    #[command(subcommand)]
    command: ReplayCommand,
}

#[derive(Debug, Subcommand)]
enum ReplayCommand {
    /// Run a dose replay: integrate recorded output (× concentration for
    /// boron) over delivered beam-on intervals, emit the reconstructed
    /// physical dose bundle and a reconstruction report. Physical dose
    /// only — biological equivalence is marked unavailable on
    /// interrupted histories.
    Run {
        /// `openbnct.dose-replay/0.1.0` spec JSON.
        #[arg(long)]
        spec: PathBuf,
        /// `openbnct.delivery-history/0.1.0` document.
        #[arg(long)]
        history: PathBuf,
        /// Per-beam rate bundle, `BEAM_ID=path`, once per spec beam.
        /// Bundles carry `gray_per_source_particle` rate maps.
        #[arg(long, required = true)]
        bundle: Vec<String>,
        /// Optional planned dose bundle for reconstructed-vs-planned deltas.
        #[arg(long)]
        planned: Option<PathBuf>,
        /// New output path for the reconstructed bundle JSON.
        #[arg(long)]
        bundle_output: PathBuf,
        /// New output path for the replay report JSON.
        #[arg(long)]
        report_output: PathBuf,
    },
}

#[derive(Debug, Args)]
struct OutcomesArgs {
    #[command(subcommand)]
    command: OutcomesCommand,
}

#[derive(Debug, Subcommand)]
enum OutcomesCommand {
    /// Validate an `openbnct.outcomes-export/0.1.0` document: linkage,
    /// chronology, endpoint-system consistency, missingness semantics.
    Validate {
        /// Export JSON document.
        #[arg(long)]
        export: PathBuf,
    },
    /// Apply a field whitelist and emit the filtered export plus the
    /// list of excluded paths — a software check, not a certification
    /// of anonymization.
    Export {
        /// Export JSON document.
        #[arg(long)]
        export: PathBuf,
        /// Whitelist JSON: `{ "section": ["field", ...] }` — a section
        /// maps to an empty array to keep it wholesale; an absent
        /// section is excluded and reported.
        #[arg(long)]
        whitelist: PathBuf,
        /// New output path for the filtered export JSON.
        #[arg(long)]
        output: PathBuf,
        /// New output path for the exclusion list JSON.
        #[arg(long)]
        excluded_output: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct QualArgs {
    #[command(subcommand)]
    command: QualCommand,
}

#[derive(Debug, Subcommand)]
enum QualCommand {
    /// Validate and print a qualification-readiness record.
    Info {
        /// `openbnct.qualification-record/0.1.0` JSON.
        #[arg(long)]
        record: PathBuf,
    },
    /// Verify every claim's evidence against the benchmark catalogue's
    /// entry ids and content bindings.
    Verify {
        /// `openbnct.qualification-record/0.1.0` JSON.
        #[arg(long)]
        record: PathBuf,
        /// `openbnct.benchmark-catalogue/0.1.0` JSON.
        #[arg(long)]
        catalogue: PathBuf,
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
        /// Enable P1 anisotropic scatter in the adjoint solve — requires
        /// `scatter_p1_matrix_per_cm` on every scattering material.
        #[arg(long)]
        p1: bool,
        /// Highest Legendre order beyond P1 (2..=5) in the adjoint
        /// scatter — requires `--p1` and `scatter_legendre_moments_per_cm`.
        #[arg(long, requires = "p1", default_value_t = 0)]
        anisotropy: u32,
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
        /// Write the windows even when an adjoint solve did not
        /// converge. Without it a non-converged solve is an error and
        /// nothing is written.
        #[arg(long)]
        allow_nonconverged: bool,
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
struct AvifyArgs {
    #[command(subcommand)]
    command: AvifyCommand,
}

#[derive(Debug, Subcommand)]
enum AvifyCommand {
    /// Export the voxel plan + engine plan JSON without running the
    /// engine — review the exact inputs the engine will consume.
    ExportPlan {
        /// `openbnct.transport-case` JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.material-assignment` JSON for the case.
        #[arg(long)]
        assignment: PathBuf,
        /// `openbnct.avify-spec` JSON — class map + engine plan fields.
        #[arg(long)]
        spec: PathBuf,
        /// Output prefix for `<prefix>_arrays.npz` / `<prefix>_meta.json`;
        /// the plan JSON lands at `<prefix>.plan.json`.
        #[arg(long)]
        prefix: PathBuf,
    },
    /// Export the voxel plan, run `avify-dose verify` as a bounded child
    /// process, and print the returned certificate summary.
    Verify {
        /// `openbnct.transport-case` JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.material-assignment` JSON for the case.
        #[arg(long)]
        assignment: PathBuf,
        /// `openbnct.avify-spec` JSON — class map + engine plan fields.
        #[arg(long)]
        spec: PathBuf,
        /// Run directory: the voxel plan, plan JSON, engine logs, and
        /// `certificate.json` land here.
        #[arg(long)]
        outdir: PathBuf,
        /// Engine invocation; e.g. `avify-dose` (default) or
        /// `/path/to/python -m avify`. Split on whitespace.
        #[arg(long, default_value = "avify-dose")]
        engine_cmd: String,
        /// OpenMC threads passed through to the engine.
        #[arg(long)]
        threads: Option<u32>,
        /// Bound on total engine wall time, seconds (default 21600).
        /// Exceeding it kills and reaps the child.
        #[arg(long, default_value_t = 21600)]
        timeout_s: u64,
    },
    /// Render a returned `certificate.json` — per-ROI certified
    /// intervals vs criteria and the recorded run records.
    Show {
        /// `certificate.json` from an `avify verify` run.
        #[arg(long)]
        certificate: PathBuf,
    },
    /// Check a run receipt (`avify-run.json`) against the filesystem:
    /// reports whether the certificate still corresponds to the inputs
    /// it was bound to, or which inputs changed/went missing.
    Status {
        /// `avify-run.json` written by `avify verify`.
        #[arg(long)]
        receipt: PathBuf,
    },
    /// Compare two runs — per-ROI interval/action changes plus which
    /// bound inputs differ. Each side is a run directory or its
    /// `avify-run.json`.
    Diff {
        #[arg(long)]
        before: PathBuf,
        #[arg(long)]
        after: PathBuf,
    },
    /// Mark a run's certificate as reviewed — writes a hash-bound
    /// `review.json` naming the certificate bytes the reviewer saw.
    Review {
        /// Run directory holding `certificate.json`.
        #[arg(long)]
        outdir: PathBuf,
        /// Reviewer name — anonymous sign-off is meaningless.
        #[arg(long)]
        reviewer: String,
        /// Free-text scope of the review (what was checked).
        #[arg(long, default_value = "")]
        note: String,
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
        /// RegionMask JSON whose centroid the beam axis is aimed at
        /// (requires `--approach`). The circular port becomes a disk on
        /// the entry face centered where the axis meets it.
        #[arg(long, requires = "approach")]
        aim_mask: Option<PathBuf>,
        /// Axis approach `+x|-x|+y|-y|+z|-z` used with `--aim-mask`.
        #[arg(long, requires = "aim_mask")]
        approach: Option<String>,
    },
    /// Emit a `openbnct.beam-description/0.1.0` from a binned spectrum
    /// CSV plus declared port geometry — the on-ramp for a group that
    /// has measured or digitized its own facility spectrum.
    Build {
        /// Beam document identifier, e.g. `mylab.beam.thermal-column.v1`.
        #[arg(long)]
        id: String,
        /// Human-readable beam name.
        #[arg(long)]
        name: String,
        /// Facility and host institution.
        #[arg(long)]
        facility: String,
        /// CSV of `e_low_ev,e_high_ev,weight` rows; a single header line
        /// is tolerated. Edges must tile without gaps and weights are
        /// normalized internally.
        #[arg(long)]
        spectrum_csv: PathBuf,
        /// World axis the port plane is perpendicular to (x, y, or z).
        #[arg(long, default_value = "z")]
        port_axis: String,
        /// Port plane offset along that axis, cm.
        #[arg(long, default_value_t = 0.0)]
        port_offset_cm: f64,
        /// Circular port radius, cm.
        #[arg(long)]
        radius_cm: f64,
        /// In-plane port center `u,v` in cm.
        #[arg(long, value_delimiter = ',', default_values_t = [0.0, 0.0])]
        center_uv_cm: Vec<f64>,
        /// Divergence half-angle in degrees (0 = parallel beam).
        #[arg(long, default_value_t = 0.0)]
        divergence_deg: f64,
        /// Optional declared fluence rate through the port, cm^-2 s^-1.
        #[arg(long)]
        fluence_rate_cm2_s: Option<f64>,
        /// Provenance note recorded verbatim as the derivation — say
        /// where the spectrum numbers came from.
        #[arg(long)]
        note: Option<String>,
        /// `authors;title;venue;year` citation for the spectrum source —
        /// repeatable, at least one required (an internal characterization
        /// memo is a valid citation).
        #[arg(long = "cite", required = true)]
        citations: Vec<String>,
        /// New output path for the beam-description JSON.
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
    /// Import a DICOM PET series as a body-weight SUV (SUVbw) volume —
    /// NIfTI float64, directly resampleable onto a transport grid for
    /// the boron uptake model. Requires BQML units, patient weight, and
    /// a complete radiopharmaceutical record.
    ImportPet {
        /// PET slice `.dcm` files; repeatable or directory-expanded
        /// upstream.
        #[arg(long, required = true)]
        slices: Vec<PathBuf>,
        /// Output `.nii` or `.nii.gz` path for the SUVbw volume.
        #[arg(long)]
        output: PathBuf,
    },
    /// Emit a deterministic synthetic BQML PET series on the
    /// NF-BNCT-001 grid and frame of reference — a companion volume
    /// (not part of the frozen case) for exercising the
    /// PET→SUV→boron-field chain end to end: CORE box at `core_suv`,
    /// background elsewhere.
    SynthPet {
        /// New output directory for the `pet-NNN.dcm` series; must not
        /// already exist.
        #[arg(long)]
        output: PathBuf,
        /// SUV inside the CORE ROI box.
        #[arg(long, default_value_t = 4.0)]
        core_suv: f64,
        /// SUV outside the CORE box.
        #[arg(long, default_value_t = 1.0)]
        background_suv: f64,
        /// Patient weight in kg for the SUVbw tags.
        #[arg(long, default_value_t = 70.0)]
        weight_kg: f64,
        /// Administered activity in MBq.
        #[arg(long, default_value_t = 500.0)]
        dose_mbq: f64,
    },
    /// Import a DICOM MR series as a rescaled-intensity volume — NIfTI
    /// float64 on the MR grid, resampleable onto a case via
    /// `register apply`. Intensities are unitless signal, never HU.
    ImportMr {
        /// MR slice `.dcm` files; repeatable or directory-expanded
        /// upstream.
        #[arg(long, required = true)]
        slices: Vec<PathBuf>,
        /// Output `.nii` or `.nii.gz` path for the intensity volume.
        #[arg(long)]
        output: PathBuf,
    },
    /// Build a transport-case scaffold, an HU volume and ROI masks from a
    /// CT series (+ optional RT Structure Set). The CT is volume-averaged
    /// (box / partial-volume mean) onto a coarser transport grid that
    /// covers it in the CT's own patient frame; nothing is reoriented.
    /// The scaffold source is a placeholder — bind a real beam with
    /// `beam bind`. Research import, not a clinical workflow.
    ImportCt {
        /// Directory searched recursively for CT (+ RTSTRUCT) `.dcm`
        /// files. Mutually exclusive with `--slices`.
        #[arg(long)]
        series: Option<PathBuf>,
        /// Explicit CT slice files. Mutually exclusive with `--series`.
        #[arg(long, num_args = 1..)]
        slices: Vec<PathBuf>,
        /// RT Structure Set file (also picked up from `--series`).
        #[arg(long)]
        rtstruct: Option<PathBuf>,
        /// Transport voxel size in mm: one value (isotropic) or `x,y,z`.
        /// Default keeps the native CT spacing.
        #[arg(long, value_delimiter = ',')]
        spacing_mm: Vec<f64>,
        /// Case id recorded in the scaffold case.
        #[arg(long)]
        case_id: String,
        /// `openbnct.material-definition` JSON used as the case's base
        /// material (e.g. void/air).
        #[arg(long)]
        base_material: PathBuf,
        /// New output path for the scaffold `openbnct.transport-case`.
        #[arg(long)]
        case_output: PathBuf,
        /// New output path for the HU volume (`.nii`) on the case grid,
        /// for `dicom calibrate --hu-nifti`.
        #[arg(long)]
        hu_output: PathBuf,
        /// New directory receiving one RegionMask JSON per ROI on the
        /// case grid plus `index.json`.
        #[arg(long)]
        masks_dir: Option<PathBuf>,
    },
    /// Apply an `openbnct.hu-calibration` anchor table to a CT HU
    /// volume, emitting an `openbnct.material-assignment` whose
    /// per-voxel two-component anchor mixtures the solver blends.
    /// Research calibration — the anchor artifact is a declared
    /// convention, not a site-validated stoichiometric fit.
    Calibrate {
        /// `openbnct.hu-calibration` JSON document.
        #[arg(long)]
        calibration: PathBuf,
        /// CT slice `.dcm` files (HU via RescaleSlope/Intercept).
        /// Mutually exclusive with `--hu-nifti`.
        #[arg(long)]
        slices: Vec<PathBuf>,
        /// HU-valued NIfTI (`.nii`/`.nii.gz`) already resliced onto the
        /// case grid — mutually exclusive with `--slices`.
        #[arg(long)]
        hu_nifti: Option<PathBuf>,
        /// `openbnct.transport-case` the assignment binds to; its grid
        /// shape must match the HU volume's.
        #[arg(long)]
        case: PathBuf,
        /// New output path for the `openbnct.material-assignment` JSON.
        #[arg(long)]
        output: PathBuf,
        /// Directory receiving one `MaterialDefinition` JSON per
        /// calibration anchor — the `sn collapse --material` inputs.
        #[arg(long)]
        materials_out_dir: Option<PathBuf>,
        /// Optional output path for the calibration coverage report.
        #[arg(long)]
        report: Option<PathBuf>,
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
        /// Name dose files with the OpenPINT convention
        /// (`<case>_<B10|N14|n|g>.nii`) instead of `component:<name>`.
        #[arg(long)]
        pint: bool,
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
    /// Emit a `meshtal` file from a physical dose bundle, in the
    /// OpenPINT convention — tallies 14/24/34/44 for B10/N14/n/g on one
    /// mesh — so OpenPINT's `sim_result_2_nifti.py` and
    /// `get_dose_components` consume the deterministic dose unchanged.
    ///
    /// The grid must be axis-aligned (any signed permutation). The
    /// `Rel Error` column reports `sigma/|value|` where the bundle
    /// carries per-voxel uncertainties, else 0 — a deterministic field
    /// has no Monte-Carlo statistical error, which is not an accuracy
    /// claim; units and normalization follow the bundle's own manifest.
    Meshtal {
        /// `openbnct.physical-dose-bundle` document.
        #[arg(long)]
        dose: PathBuf,
        /// New output path for the meshtal text file.
        #[arg(long)]
        output: PathBuf,
    },
    /// Emit a PHITS input deck for a transport case.
    ///
    /// Supports disk (Z face) and rectangular-plane sources, monoenergetic
    /// and tabulated-histogram energies, and optional material
    /// assignments: `voxel_box` regions carve `RPP` cells, voxel sets
    /// emit a `LAT=1` lattice fill. `[t-track]` xyz-mesh tallies (neutron
    /// + photon) read back through `openbnct import phits`. Anything
    ///   outside the subset is refused with a named reason.
    Phits {
        /// `openbnct.transport-case/0.1.0` document.
        #[arg(long)]
        case: PathBuf,
        /// Optional `openbnct.material-assignment/0.2.0` region assignment.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Batch count (`maxcas` is derived from `requested_histories`).
        #[arg(long, default_value = "10")]
        maxbch: u32,
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
    /// Import an OpenPINT Excel treatment workbook.
    ///
    /// Reads the `bnct` sheet (component dose NIfTI paths), the structure
    /// sheets (`GTV`/`CTV`/`PTV`/`HOM`/`OAR` — mask NIfTIs plus per-structure
    /// boron concentration and OAR dose constraints), and the optional `ct`
    /// sheet. Workbook paths resolve relative to the workbook's directory.
    /// The four component volumes lift into a physical dose bundle; each
    /// structure mask rasterizes onto the bundle's grid (nearest-neighbour
    /// when geometries differ) as a `RegionMask` JSON. A plan-summary JSON
    /// records boron concentrations, OAR constraints, and the workbook's
    /// SHA-256 as provenance.
    #[command(name = "openpint")]
    OpenPint {
        /// OpenPINT `.xlsx` workbook in `PlanConfig.from_excel` layout.
        #[arg(long)]
        workbook: PathBuf,
        /// Accumulated case identifier.
        #[arg(long)]
        case_id: String,
        /// `gray_per_source_particle` or `gray` — the dose semantics of the
        /// component NIfTIs (OpenPINT MCNP6/PHITS tallies are
        /// per-source-particle).
        #[arg(long)]
        unit: String,
        /// Declared dose semantics: normalization basis and any folding or
        /// kerma-response treatment applied by the producing pipeline.
        #[arg(long)]
        normalization: String,
        /// OpenPINT version label; workbooks do not record one.
        #[arg(long)]
        producer_version: Option<String>,
        /// Optional DICOM frame-of-reference UID carried into the bundle.
        #[arg(long)]
        frame_of_reference_uid: Option<String>,
        /// Output directory for `physical-dose-bundle.json`, one
        /// `<name>.mask.json` per structure, and `openpint-plan-summary.json`.
        #[arg(long)]
        out: PathBuf,
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
    /// Build a `openbnct.material-assignment` from a NIfTI labelmap plus
    /// a materials table — the on-ramp for segmented phantoms exported
    /// from 3D Slicer, ITK-SNAP, or a Python pipeline.
    ///
    /// `--materials` is a JSON object mapping integer labels to inline
    /// `openbnct.material-definition` objects; label `0` is the
    /// background/base material. Every nonzero label in the labelmap
    /// must have an entry.
    Labelmap {
        /// Integer-labeled NIfTI-1 image (`.nii` or `.nii.gz`); voxels
        /// must be exact integer labels within 1e-6.
        #[arg(long)]
        nifti: PathBuf,
        /// JSON `{"label": material-definition}` map.
        #[arg(long)]
        materials: PathBuf,
        /// Existing transport-case JSON to bind (grid must match the
        /// labelmap exactly; base material comes from the case).
        #[arg(long)]
        case: Option<PathBuf>,
        /// Emit a scaffold transport-case JSON here — grid from the
        /// labelmap, label-0 material as base, and a mono-thermal disk
        /// source on the -z face meant to be replaced via `beam bind`.
        /// Required when `--case` is absent.
        #[arg(long)]
        case_output: Option<PathBuf>,
        /// Case identifier; required with `--case-output`, otherwise
        /// taken from `--case`.
        #[arg(long)]
        case_id: Option<String>,
        /// New output path for the material-assignment JSON.
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
    /// Optimize non-negative exposure weights against dose-volume
    /// objectives (`openbnct.inverse-plan-objective/0.1.0` →
    /// `openbnct.inverse-plan-result/0.1.0`). Research optimizer — not
    /// a commissioned treatment-planning product.
    Optimize {
        /// `openbnct.inverse-plan-objective` JSON document.
        #[arg(long)]
        objective: PathBuf,
        /// Per-beam `openbnct.physical-dose-bundle` JSON — one file
        /// per beam; repeatable. The beam name is the file stem.
        #[arg(long, required = true)]
        dose: Vec<PathBuf>,
        /// `RegionMask` JSON (`{"name": ..., "voxels": [...]}`);
        /// repeatable. Every mask the objectives name must be supplied.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// Initial weight per beam in `--dose` order; repeatable.
        /// Defaults to 1.0 for every beam.
        #[arg(long)]
        initial: Vec<f64>,
        /// Result document identifier.
        #[arg(long, default_value = "openbnct.inverse-plan-result")]
        id: String,
        /// Provenance identifier recorded on the result.
        #[arg(long)]
        provenance_id: Option<String>,
        /// Output path for the result JSON.
        #[arg(long)]
        output: PathBuf,
        /// Optionally also write an `openbnct.exposure-plan` binding the
        /// optimized weights to their dose bundles (consumable by
        /// `plan validate`/`plan export` and the GUI Plan workspace).
        #[arg(long)]
        emit_plan: Option<PathBuf>,
        /// Monitor-unit convention for the emitted exposure plan: each
        /// exposure's `duration_s` is its optimized weight × this many
        /// seconds — the declared beam-on time at the dose bundle's
        /// simulated normalization (`source_strength_scaling` basis).
        /// Requires `--emit-plan`.
        #[arg(long, requires = "emit_plan")]
        seconds_per_weight: Option<f64>,
        /// `openbnct.scenario-set/0.1.0` JSON — when supplied, optimize
        /// against the worst-case penalty across the declared
        /// perturbations (plus the nominal), not the nominal alone.
        /// The result records `method: "worst_case_scenario"`.
        #[arg(long)]
        scenario_set: Option<PathBuf>,
        /// `openbnct.fraction-scales/0.1.0` JSON — per-fraction dose
        /// component scales (e.g. boron washout). Expands the pool to
        /// `(beam, fraction)` variables named `beam@fraction` and
        /// optimizes the delivery-time allocation across the schedule.
        /// Requires isoeffective objectives + component-resolved bundles.
        #[arg(long)]
        fraction_scales: Option<PathBuf>,
        /// Solver: `pgd` (projected gradient descent, the default),
        /// `newton` (projected Gauss-Newton — exact quadratic curvature
        /// of active violations, converges in a handful of iterations),
        /// `qp` (Clarabel interior point on the identical quadratic
        /// penalty — certified optimum + dual bound multipliers), or
        /// `lp` (strict: objective bounds become hard constraints and
        /// the cost is `weight_regularization·Σw`; primal infeasibility
        /// is a definitive answer). `qp`/`lp` require every objective
        /// be linear or CVaR-representable — `min_eud` only at
        /// `eud_a = 1`.
        #[arg(long, default_value = "pgd", value_parser = ["pgd", "newton", "qp", "lp"])]
        solver: String,
    },
    /// Select the best beam subset of size ≤ `--beams` from a
    /// candidate dose-field pool. Each subset's weights come from a
    /// certified inner solve (`qp` penalty, or `lp` strict bounds that
    /// mark infeasible subsets definitively), so the ranking is exact
    /// — this is the dosimetric counterpart of the geometric direction
    /// pre-filter in `plan directions`. Emits
    /// `openbnct.beam-selection/0.1.0`.
    Select {
        /// `openbnct.inverse-plan-objective/0.1.0` JSON — the
        /// objective every subset is scored against.
        #[arg(long)]
        objective: PathBuf,
        /// `openbnct.physical-dose-bundle/0.2.0` JSON per *candidate*
        /// beam — supply the whole pool, not just expected winners.
        #[arg(long)]
        dose: Vec<PathBuf>,
        /// `openbnct.region-mask/0.1.0` JSON per named mask.
        #[arg(long)]
        mask: Vec<PathBuf>,
        /// Maximum beams per subset; subsets of size 1..=N are ranked.
        #[arg(long, default_value_t = 2)]
        beams: u32,
        /// `exhaustive` (every subset — the global optimum, refused
        /// past 50 000 solves) or `greedy` (forward stepwise for
        /// large pools).
        #[arg(long, default_value = "exhaustive", value_parser = ["exhaustive", "greedy"])]
        search: String,
        /// `openbnct.fraction-scales/0.1.0` JSON — expands each
        /// candidate to per-fraction variables (`beam@fraction`), so
        /// selection answers "which beams in which fractions".
        /// Requires isoeffective objectives + component-resolved bundles.
        #[arg(long)]
        fraction_scales: Option<PathBuf>,
        /// Inner solver: `qp` (certified penalty — continuous ranking)
        /// or `lp` (hard bounds — feasibility + min Σw).
        #[arg(long, default_value = "qp", value_parser = ["qp", "lp"])]
        solver: String,
        /// Output path for the selection report.
        #[arg(long)]
        output: PathBuf,
        /// Optionally emit the winning subset's
        /// `openbnct.inverse-plan-result/0.1.0` to this path.
        #[arg(long)]
        emit_plan: Option<PathBuf>,
        /// Report id embedded in the artifact.
        #[arg(long)]
        id: String,
        #[arg(long)]
        provenance_id: Option<String>,
    },
    /// Propagate declared systematic σ on each beam's component dose
    /// through the optimized weights: per-objective metric 1σ and the
    /// Gaussian violation probability against its bound. Emits
    /// `openbnct.plan-robustness/0.1.0`.
    Robustness {
        /// `openbnct.inverse-plan-result` JSON from `plan optimize`.
        #[arg(long)]
        result: PathBuf,
        /// The `openbnct.inverse-plan-objective` JSON the result was
        /// optimized under (its hash is verified against the result).
        #[arg(long)]
        objective: PathBuf,
        /// Per-beam `openbnct.physical-dose-bundle` JSON in the same
        /// order `plan optimize` consumed them; repeatable.
        #[arg(long, required = true)]
        dose: Vec<PathBuf>,
        /// `RegionMask` JSON; repeatable — every mask the objectives
        /// name must be supplied.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// Declared relative 1σ on a component as `name=sigma`;
        /// repeatable.
        #[arg(long)]
        relative: Vec<String>,
        /// Positioning 1σ in millimetres — contributes `|∇D_c|·σ_mm`
        /// per component per beam.
        #[arg(long)]
        positioning_sigma_mm: Option<f64>,
        /// `openbnct.boron-field` JSON whose per-voxel concentration σ
        /// scales the boron component of every beam.
        #[arg(long)]
        boron_field: Option<PathBuf>,
        /// Report identifier.
        #[arg(long, default_value = "openbnct.plan-robustness")]
        id: String,
        /// Output path for the robustness JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Evaluate the optimized weights under a declared set of discrete
    /// scenarios — named component/uptake/T-N scalings and whole-field
    /// mm shifts — reporting every objective's achieved metric per
    /// scenario plus cross-scenario bands, as an
    /// `openbnct.scenario-report/0.1.0`. Complements `plan robustness`'s
    /// first-order Gaussian propagation for structured non-Gaussian
    /// uncertainties.
    Scenarios {
        /// `openbnct.inverse-plan-result` JSON from `plan optimize`.
        #[arg(long)]
        result: PathBuf,
        /// The `openbnct.inverse-plan-objective` JSON the result was
        /// optimized under (its hash is verified against the result).
        #[arg(long)]
        objective: PathBuf,
        /// `openbnct.scenario-set/0.1.0` JSON document.
        #[arg(long)]
        scenario_set: PathBuf,
        /// Per-beam `openbnct.physical-dose-bundle` JSON in the same
        /// order `plan optimize` consumed them; repeatable.
        #[arg(long, required = true)]
        dose: Vec<PathBuf>,
        /// `RegionMask` JSON; repeatable — every mask the objectives
        /// and scenario region scales name must be supplied.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// Report identifier.
        #[arg(long, default_value = "openbnct.scenario-report")]
        id: String,
        /// Output path for the scenario report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Compare plan results (nominal vs robust weights) on a held-out
    /// scenario set — the set must be disjoint from the scenarios the
    /// weights were optimized against, declared via `--trained-on`.
    /// Emits `openbnct.heldout-comparison/0.1.0`.
    Compare {
        /// `openbnct.inverse-plan-result` JSON per compared plan;
        /// repeatable.
        #[arg(long, required = true)]
        result: Vec<PathBuf>,
        /// The `openbnct.inverse-plan-objective` JSON all results were
        /// optimized under (hash-verified per result).
        #[arg(long)]
        objective: PathBuf,
        /// Held-out `openbnct.scenario-set/0.1.0` JSON.
        #[arg(long)]
        scenario_set: PathBuf,
        /// `openbnct.scenario-set/0.1.0` JSON of the scenarios the
        /// robust weights were optimized against. Overlap with the
        /// held-out set fails — same-scenario performance is not
        /// independent robustness validation.
        #[arg(long)]
        trained_on: PathBuf,
        /// Per-beam `openbnct.physical-dose-bundle` JSON in the same
        /// order `plan optimize` consumed them; repeatable.
        #[arg(long, required = true)]
        dose: Vec<PathBuf>,
        /// `RegionMask` JSON; repeatable.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// Report identifier.
        #[arg(long, default_value = "openbnct.heldout-comparison")]
        id: String,
        /// Output path for the comparison report JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Enumerate beam-direction candidates over an azimuth×elevation
    /// grid and rank them by tissue path length to the aim-mask
    /// centroid — the zero-transport pre-filter that selects which
    /// directions `plan fields` should solve. Emits `name,dx,dy,dz`
    /// spec lines directly consumable as `plan fields --beam`.
    Directions {
        /// `openbnct.transport-case` JSON (geometry source).
        #[arg(long)]
        case: PathBuf,
        /// `RegionMask` JSON every beam axis converges on.
        #[arg(long)]
        aim_mask: PathBuf,
        /// Optional body/skin `RegionMask` — enables tissue-path-length
        /// scoring (epithermal depth heuristic).
        #[arg(long)]
        body_mask: Option<PathBuf>,
        /// Azimuth divisions over 0–360°.
        #[arg(long, default_value = "12")]
        azimuth_steps: u32,
        /// Elevation divisions over −60..+60°.
        #[arg(long, default_value = "3")]
        elevation_steps: u32,
        /// Emit only the top N ranked candidates (0 = all).
        #[arg(long, default_value = "0")]
        top: usize,
        /// Optional `openbnct.multigroup-data` JSON — enables adjoint
        /// importance scoring: one adjoint solve with the aim region as
        /// the source, then every candidate scored by the inner product
        /// of its uncollided beam with φ* (transport-informed, no
        /// per-direction solve). Re-ranks the list by importance.
        #[arg(long)]
        data: Option<PathBuf>,
        /// Aperture radius in cm for the aimed-disk scoring (with
        /// `--data`).
        #[arg(long, default_value = "4.0")]
        radius_cm: f64,
        /// Also emit a `name,direction,tissue_path_mm` CSV for the
        /// record; without it only `plan fields --beam` lines print.
        #[arg(long)]
        csv: Option<PathBuf>,
        /// Emit an `openbnct.direction-candidates/0.1.0` document —
        /// the ranked sweep with both scores, content-bound to the
        /// case, masks, and (when present) multigroup data.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Sweep document identifier; defaults to
        /// `{case_id}.direction-candidates`.
        #[arg(long)]
        id: Option<String>,
        /// Provenance identifier; defaults to `directions:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
    },
    /// Synthesize beam directions from a signed composite adjoint
    /// solve: every objective contributes its marginal-utility field —
    /// coverage masks positive, sparing masks negative, all
    /// response-weighted — so a candidate's score is the marginal
    /// dose-utility of delivering that beam, not just aim fluence.
    /// One adjoint solve ranks the whole direction fan; with `--dose`/
    /// `--weights` the source is the true objective gradient at the
    /// current plan (the iterate-and-resynthesize loop). Emits
    /// `plan fields --beam` lines and an optional candidates document
    /// scored `adjoint_marginal_utility`.
    Synthesize {
        /// `openbnct.transport-case` JSON (geometry + source template).
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data` JSON — drives the adjoint solve.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.material-assignment` JSON.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// `openbnct.inverse-plan-objective` JSON — the objective set
        /// whose masks, weights, and dose quantity build the source.
        #[arg(long)]
        objective: PathBuf,
        /// `RegionMask` JSON; repeatable — every mask the objectives
        /// name must be supplied.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// `RegionMask` JSON every candidate beam converges on.
        #[arg(long)]
        aim_mask: PathBuf,
        /// Per-beam `openbnct.physical-dose-bundle` JSON of the current
        /// plan; repeatable — enables marginal-utility gradients.
        #[arg(long)]
        dose: Vec<PathBuf>,
        /// Comma-separated current weights parallel to `--dose`.
        #[arg(long)]
        weights: Option<String>,
        /// Azimuth divisions over 0–360°.
        #[arg(long, default_value = "12")]
        azimuth_steps: u32,
        /// Elevation divisions over −60..+60°.
        #[arg(long, default_value = "3")]
        elevation_steps: u32,
        /// Emit only the top N ranked candidates (0 = all).
        #[arg(long, default_value = "0")]
        top: usize,
        /// Aperture radius in cm for the aimed-disk scoring.
        #[arg(long, default_value = "4.0")]
        radius_cm: f64,
        /// Quadrature order for the adjoint solve.
        #[arg(long, default_value = "8")]
        order: u32,
        /// Emit an `openbnct.direction-candidates/0.1.0` document.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Spectrum variant `name=path` — each path is an
        /// `EnergyDistribution` JSON; repeatable. Scores every
        /// direction against every spectrum (the adjoint solve is
        /// shared — variants cost only an uncollided ray-trace each).
        #[arg(long, value_name = "NAME=PATH")]
        spectrum: Vec<String>,
        /// Comma-separated aperture radii in cm — scores
        /// direction×radius combinations when supplied; overrides
        /// `--radius-cm`.
        #[arg(long)]
        radii: Option<String>,
        /// Sweep document identifier; defaults to
        /// `{case_id}.direction-synthesis`.
        #[arg(long)]
        id: Option<String>,
        /// Provenance identifier; defaults to `synthesize:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
    },
    /// Closed-loop adjoint inverse planning: per round, synthesize
    /// ranks the direction fan by marginal utility at the *current*
    /// plan, the top `--add` unscored beams get real forward solves,
    /// and `plan optimize`'s solver re-assigns weights — the iterate
    /// loop the adjoint gradient was built for. Emits per-beam dose
    /// bundles, the final `openbnct.inverse-plan-result`, and an
    /// `openbnct.iteration-report` documenting the round trajectory.
    Iterate {
        /// `openbnct.transport-case` JSON (geometry + source template).
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data` JSON; must declare
        /// `component_profile` for dose folding.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.material-assignment` JSON.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// `openbnct.inverse-plan-objective` JSON.
        #[arg(long)]
        objective: PathBuf,
        /// `RegionMask` JSON; repeatable — every mask the objectives
        /// name must be supplied.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// `RegionMask` JSON every candidate beam converges on.
        #[arg(long)]
        aim_mask: PathBuf,
        /// Azimuth divisions over 0–360°.
        #[arg(long, default_value = "12")]
        azimuth_steps: u32,
        /// Elevation divisions over −60..+60°.
        #[arg(long, default_value = "3")]
        elevation_steps: u32,
        /// Aperture radius in cm for the aimed-disk solves — the
        /// fallback when `--radii` is unset.
        #[arg(long, default_value = "4.0")]
        radius_cm: f64,
        /// Spectrum variant `name=path` (`EnergyDistribution` JSON);
        /// repeatable. The candidate fan expands over direction ×
        /// spectrum — admitted beams carry the winning variant's
        /// energy block into their forward solves.
        #[arg(long, value_name = "NAME=PATH")]
        spectrum: Vec<String>,
        /// Comma-separated aperture radii in cm — the fan expands
        /// over direction × radius as well; overrides `--radius-cm`.
        #[arg(long)]
        radii: Option<String>,
        /// Synthesis–solve rounds; each adds up to `--add` beams.
        #[arg(long, default_value = "3")]
        rounds: u32,
        /// Beams added per round (top ranked, not already in the pool).
        #[arg(long, default_value = "2")]
        add: usize,
        /// Quadrature order for forward and adjoint solves.
        #[arg(long, default_value = "8")]
        order: u32,
        /// Relative convergence target for forward solves.
        #[arg(long, default_value = "1e-6")]
        convergence: f64,
        /// Inner iterations per group solve.
        #[arg(long, default_value = "40")]
        max_inner: u32,
        /// Outer sweeps per forward solve.
        #[arg(long, default_value = "30")]
        max_outer: u32,
        /// Empty output directory — receives per-beam dose bundles,
        /// the iteration report, and the final result.
        #[arg(long)]
        output_dir: PathBuf,
        /// Report identifier; defaults to `{case_id}.iterate`.
        #[arg(long)]
        id: Option<String>,
        /// Provenance identifier; defaults to `iterate:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
    },
    /// Shape a beam aperture by adjoint beamlet importance: the aimed
    /// disk for `--direction` is subdivided into `--beamlets`² sub-disk
    /// beamlets, each scored against the objective composite adjoint
    /// (uncollided flux × φ*, no transport per beamlet). Beamlets whose
    /// utility density falls below `--keep-fraction` of the maximum are
    /// marked closed — the kept set is the shaped aperture. Emits
    /// `openbnct.aperture-shape/0.1.0`.
    Shape {
        /// `openbnct.transport-case` JSON (geometry + source template).
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data` JSON.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.material-assignment` JSON.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// `openbnct.inverse-plan-objective` JSON.
        #[arg(long)]
        objective: PathBuf,
        /// `RegionMask` JSON; repeatable — every objective mask.
        #[arg(long, required = true)]
        mask: Vec<PathBuf>,
        /// `RegionMask` JSON the aperture disk converges on.
        #[arg(long)]
        aim_mask: PathBuf,
        /// `openbnct.physical-dose-bundle` JSON per beam of a current
        /// plan — shapes against the marginal-utility field at that
        /// plan. Repeatable; pair with `--weights`.
        #[arg(long)]
        dose: Vec<PathBuf>,
        /// Comma-separated weights parallel to `--dose`.
        #[arg(long)]
        weights: Option<String>,
        /// Beam direction `dx,dy,dz` (LPS unit vector).
        #[arg(long, allow_hyphen_values = true)]
        direction: String,
        /// Full aperture radius in cm the beamlets subdivide.
        #[arg(long, default_value = "4.0")]
        radius_cm: f64,
        /// Beamlets per axis across the disk bounding square.
        #[arg(long, default_value = "8")]
        beamlets: u32,
        /// Keep beamlets with utility density ≥ this fraction of max.
        #[arg(long, default_value = "0.5")]
        keep_fraction: f64,
        /// Quadrature order for the adjoint solve (and beamlet field
        /// solves when `--emit-fields` is set).
        #[arg(long, default_value = "8")]
        order: u32,
        /// Optionally write a `openbnct.physical-dose-bundle` per
        /// *kept* beamlet into this (empty-or-new) directory — a
        /// forward transport solve per beamlet — so
        /// `plan optimize --dose DIR/*.json` produces the beamlet
        /// intensity map. Capped at 256 kept beamlets.
        #[arg(long)]
        emit_fields: Option<PathBuf>,
        /// Output path for the `openbnct.aperture-shape` document.
        #[arg(long)]
        output: PathBuf,
        /// Document identifier; defaults to `{case_id}.aperture-shape`.
        #[arg(long)]
        id: Option<String>,
        /// Provenance identifier; defaults to `shape:` + the id.
        #[arg(long)]
        provenance_id: Option<String>,
    },
    /// Aim and solve a beam per direction through a target mask —
    /// emits a unit-weight dose bundle per beam plus a
    /// `openbnct.beam-field-set/0.1.0` manifest. The deterministic
    /// multi-field front end to `plan optimize`.
    Fields {
        /// `openbnct.transport-case` JSON whose `source` is the aim
        /// template (its space/angle are repositioned per beam).
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-data` JSON; must declare
        /// `component_profile` for dose folding.
        #[arg(long)]
        data: PathBuf,
        /// `openbnct.material-assignment` JSON.
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// `RegionMask` JSON every beam axis passes through.
        #[arg(long)]
        aim_mask: PathBuf,
        /// Beam spec `name,dx,dy,dz`; repeatable. Direction is the
        /// propagation vector in LPS (normalized internally).
        #[arg(long, required = true)]
        beam: Vec<String>,
        /// Circular aperture radius in cm applied to every beam — the
        /// deterministic solver's on-face disk source.
        #[arg(long, required = true)]
        radius_cm: f64,
        /// S_N quadrature order (per `sn solve`).
        #[arg(long, default_value = "8")]
        order: u32,
        /// Outer-iteration convergence target.
        #[arg(long, default_value = "1e-4")]
        convergence: f64,
        #[arg(long, default_value = "40")]
        max_inner: u32,
        #[arg(long, default_value = "400")]
        max_outer: u32,
        /// Periodic boundary axes; repeatable (x, y, z).
        #[arg(long)]
        periodic: Vec<String>,
        /// Disable the analytic uncollided-beam split.
        #[arg(long)]
        no_uncollided_split: bool,
        /// Disable the extended transport correction.
        #[arg(long)]
        no_transport_correction: bool,
        /// P1 anisotropy fast path (as `sn solve --p1`).
        #[arg(long)]
        p1: bool,
        /// Explicit anisotropy order 2–5 (requires --p1 and
        /// scatter Legendre moments on the data; as `sn solve
        /// --anisotropy`).
        #[arg(long, default_value_t = 0)]
        anisotropy: u32,
        /// Anderson acceleration depth (0 = plain sweeps; as
        /// `sn solve --anderson`).
        #[arg(long, default_value_t = 0)]
        anderson: usize,
        /// Coarse screening stage: run every declared beam at this
        /// quadrature order, score its mean aim-mask `physical_total`,
        /// then run the full-quality solve only on the `--keep-top`
        /// best. The manifest records all scores and retention.
        #[arg(long)]
        screen_order: Option<u32>,
        /// Screening-stage outer convergence target.
        #[arg(long, default_value = "1e-2")]
        screen_convergence: f64,
        #[arg(long, default_value = "20")]
        screen_max_inner: u32,
        #[arg(long, default_value = "80")]
        screen_max_outer: u32,
        /// Beams retained for the full-quality solve; required with
        /// `--screen-order`, must not exceed the declared beam count.
        #[arg(long)]
        keep_top: Option<usize>,
        /// Output directory for per-beam artifacts and the manifest;
        /// created if absent, must not already contain files.
        #[arg(long)]
        output_dir: PathBuf,
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
        /// Within-group sweep break tolerance (defaults to
        /// `--convergence`). The outer residual cannot descend far
        /// below the inner accuracy floor — tighten this for deep
        /// outer targets.
        #[arg(long)]
        inner_convergence: Option<f64>,
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
        /// Force P1 anisotropic scattering (requires
        /// `scatter_p1_matrix_per_cm` on every scattering material in
        /// the data; supersedes the transport correction — physical σ_t
        /// applies). P1 is already the default whenever the data carries
        /// P1 moments on every scattering material: P0 plus a transport
        /// correction cannot represent hydrogen's forward-peaked
        /// downscatter and under-penetrates tissue with depth.
        #[arg(long, conflicts_with = "p0")]
        p1: bool,
        /// Force P0 scattering (isotropic in-scatter plus the in-group
        /// transport correction) even when the data carries P1 moments.
        #[arg(long)]
        p0: bool,
        /// Keep the exponential within-cell source closure on under P1.
        /// It is on by default only for P0: under P1 its λ refits can keep
        /// a 3-D solve from converging (layered head, S4/S8), at the cost
        /// of diamond-difference overshoot in optically thick cells
        /// (σ_t·Δx ≳ 1) — refine the grid or pass this flag there.
        #[arg(long)]
        exp_source: bool,
        /// Highest Legendre order l carried by the in-scatter kernel
        /// beyond P1 (2–5; requires `--p1` and
        /// `scatter_legendre_moments_per_cm` on every scattering
        /// material — emitted by `sn collapse` v2+ data). The discrete
        /// P_l kernel is eigendecomposed once per quadrature, so the
        /// source carries the exact addition theorem per ordinate set.
        #[arg(long, default_value_t = 0)]
        anisotropy: u32,
        /// Anderson acceleration depth for the outer iteration
        /// (0 = plain sweeps). ≥1 mixes the last N outer iterates
        /// per symmetric cycle — accelerates the slow energy-coupling
        /// mode bound-atom S(α,β) upscatter introduces.
        #[arg(long, default_value_t = 3)]
        anderson: usize,
        /// Disable the coarse-mesh rebalance of the upscatter block. It
        /// is on by default: without it the layered-head solve needs
        /// ~3x the outer iterations (68 vs 21 at 25^3) and does not
        /// reach the default target inside `--max-outer 32`, although
        /// each outer is ~25 % cheaper and ~30 % less memory is held
        /// (the per-cell face-current buffers are never allocated). The
        /// converged dose is unchanged to <= 4e-7 relative.
        /// `OPENBNCT_NO_CMR` also disables it.
        #[arg(long)]
        no_cmr: bool,
        /// Within-bin spread of a tabulated-histogram source spectrum:
        /// `collapse_consistent` (Maxwellian below 0.5 eV, 1/E above —
        /// the default) or `uniform_in_bin` (uniform per eV — the
        /// histogram convention OpenMC/MCNP apply; the honest match
        /// for a coarse-bin spectrum compared against CE transport).
        #[arg(long, default_value = "collapse_consistent")]
        source_weighting: String,
        /// Write the flux field even when the outer iteration exhausts
        /// `max_outer` unconverged — the artifact records
        /// `converged: false` and the residual. For diagnostics only;
        /// downstream consumers should treat unconverged fields as
        /// provisional.
        #[arg(long)]
        allow_unconverged: bool,
        /// Also write a folded `openbnct.physical-dose-bundle/0.2.0` to
        /// this path (the data must declare `dose_response_gy_cm2` and a
        /// `component_profile` binding).
        #[arg(long)]
        dose: Option<PathBuf>,
        /// Also write the unit-concentration boron dose
        /// (`openbnct.boron-unit-dose/0.1.0`, Gy per source particle per
        /// µg/g of ¹⁰B) to this path. Requires the data to carry
        /// `boron_unit_response_gy_cm2_per_ug_g` (re-run `sn collapse`
        /// for older data). Apply a concentration later with
        /// `boron dose`.
        #[arg(long)]
        boron_unit_output: Option<PathBuf>,
        /// Suppress the per-outer-iteration progress lines
        /// (`[sn] outer k/max: residual r (t s)`) printed to stderr.
        #[arg(long)]
        quiet: bool,
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
        /// Also write the unit-concentration boron dose
        /// (`openbnct.boron-unit-dose/0.1.0`) to this path; the data must
        /// carry `boron_unit_response_gy_cm2_per_ug_g`.
        #[arg(long)]
        boron_unit_output: Option<PathBuf>,
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
        /// MF7/MT4 thermal-scattering-law tape for a nuclide —
        /// `--tsl H1=/path/tsl_H(H2O)_0001.dat`, repeatable. Its
        /// incoherent-inelastic S(α,β) kernel replaces the free-gas
        /// elastic kernel below the tape's E_max (thermal upscatter
        /// included); free-gas covers the |ΔE|>β_max·kT residual and
        /// higher energies.
        #[arg(long, value_name = "NUCLIDE=PATH")]
        tsl: Vec<String>,
        /// Material temperature (K) for the S(α,β) evaluation — the
        /// nearest tabulated temperature on each tape is used.
        #[arg(long, default_value_t = 293.6)]
        tsl_temperature: f64,
        /// Apply Bondarenko heterogeneous-dilution self-shielding:
        /// each nuclide's collapse weight ×σ₀/(σ_t+σ₀), with σ₀
        /// supplied by the other nuclides in the material (no free
        /// parameter). Suppresses resonance-dip weighting — the
        /// correct first-order treatment for absorber-rich spectra.
        #[arg(long)]
        self_shielding: bool,
        /// Material definition artifact (openbnct.material/0.1.0) —
        /// repeat for every material the solve requires.
        #[arg(long, required = true)]
        material: Vec<PathBuf>,
        /// Energy boundaries in eV, strictly descending — e.g.
        /// `--boundaries 1.7e7,1e4,0.5,1e-5` (group 0 = highest).
        /// Mutually exclusive with `--boundaries-file`.
        #[arg(
            long,
            value_delimiter = ',',
            conflicts_with = "boundaries_file",
            required_unless_present = "boundaries_file"
        )]
        boundaries: Vec<f64>,
        /// `openbnct.boundary-proposal/0.1.0` document (or a bare
        /// `{"energy_boundaries_ev": [...]}` object) supplying the
        /// descending edge list — from `sn boundaries`.
        #[arg(long)]
        boundaries_file: Option<PathBuf>,
        /// Artifact id for the emitted multigroup-data artifact.
        #[arg(long)]
        id: String,
        /// Component-definition-profile JSON to content-bind; required
        /// for `--dose` folding at solve time.
        #[arg(long)]
        component_profile: Option<PathBuf>,
        /// Optional `EnergyDistribution` JSON overriding the default
        /// Maxwellian+1/E collapse weighting — a tabulated histogram
        /// from a fine-group or MC solve carries intra-group spectral
        /// hardening that analytic weighting cannot represent. Bin
        /// weights are normalized to a per-eV flux density.
        #[arg(long)]
        weighting_spectrum: Option<PathBuf>,
        /// Penetration depth (cm) for survival-weighted condensation:
        /// each collapse weight is multiplied by the host material's
        /// uncollided-survival factor exp(−σ_t(E)·z), carrying
        /// intra-group spectral hardening into the group constants.
        /// The right condensation for a beam-driven deep-penetration
        /// problem; `z` is the declared dose-relevant reference depth.
        #[arg(long)]
        attenuation_depth: Option<f64>,
        /// Free-text appended to the collapse declaration.
        #[arg(long)]
        note: Option<String>,
        /// Artifact path; printed to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Extract a volume-weighted group-flux spectrum from a
    /// `multigroup-flux` artifact as an `EnergyDistribution`
    /// tabulated histogram — feed it to `sn collapse
    /// --weighting-spectrum` to condense cross sections against a
    /// problem-informed spectrum (intra-group hardening), or to
    /// `plan synthesize --spectrum` as a source variant.
    Spectrum {
        /// `openbnct.multigroup-flux` artifact.
        #[arg(long)]
        flux: PathBuf,
        /// Restrict to cells inside this `openbnct.region-mask`.
        #[arg(long, conflicts_with = "material")]
        mask: Option<PathBuf>,
        /// Restrict to cells whose assignment resolves to this
        /// material id — requires `--case`, `--data`, `--assignment`.
        #[arg(long)]
        material: Option<String>,
        /// `openbnct.transport-case` (required with `--material`).
        #[arg(long)]
        case: Option<PathBuf>,
        /// `openbnct.multigroup-data` (required with `--material`).
        #[arg(long)]
        data: Option<PathBuf>,
        /// `openbnct.material-assignment` (required with `--material`).
        #[arg(long)]
        assignment: Option<PathBuf>,
        /// Output path for the `EnergyDistribution` JSON; stdout when
        /// omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Collapse photon-atomic HDF5 tables plus neutron-evaluation
    /// photon products into a coupled `openbnct.multigroup-photon-data/0.1.0`
    /// artifact — photon σt, Klein–Nishina transfer, coherent diagonal,
    /// pair→annihilation source, kerma response, and the n→γ
    /// production matrix bound to the neutron group structure.
    PhotonCollapse {
        /// Directory of `photo_<Elem>.h5` photon-atomic tables.
        #[arg(long)]
        photon_library: PathBuf,
        /// Directory of `<Nuclide>.h5` neutron tables (for n→γ production).
        #[arg(long)]
        neutron_library: PathBuf,
        /// Material definition artifact (openbnct.material/0.1.0) —
        /// repeat for every material the solve requires.
        #[arg(long, required = true)]
        material: Vec<PathBuf>,
        /// Photon energy boundaries in eV, strictly descending —
        /// must contain a bin covering 511 keV for pair annihilation.
        #[arg(long, value_delimiter = ',', required = true)]
        photon_boundaries: Vec<f64>,
        /// The `openbnct.multigroup-data/0.1.0` the neutron flux is
        /// solved against — its group structure pins the production
        /// matrix's neutron axis.
        #[arg(long)]
        neutron_data: PathBuf,
        /// Artifact id for the emitted photon-data artifact.
        #[arg(long)]
        id: String,
        /// Component-definition-profile JSON to content-bind; required
        /// for `--dose` folding at photon-solve time.
        #[arg(long)]
        component_profile: Option<PathBuf>,
        /// Free-text appended to the collapse declaration.
        #[arg(long)]
        note: Option<String>,
        /// Artifact path; printed to stdout when omitted.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Solve the coupled photon problem: builds the volumetric n→γ
    /// source from a converged neutron flux and the photon data's
    /// production matrix, sweeps the photon tables through the same
    /// S_N solver, and optionally folds photon dose. One-way coupled —
    /// the neutron flux is an input, not iterated.
    PhotonSolve {
        /// `openbnct.transport-case/0.1.0` case JSON.
        #[arg(long)]
        case: PathBuf,
        /// `openbnct.multigroup-photon-data/0.1.0` photon data JSON.
        #[arg(long)]
        photon_data: PathBuf,
        /// `openbnct.multigroup-flux/0.1.0` converged neutron flux —
        /// must carry the neutron group structure the production
        /// matrix maps from.
        #[arg(long)]
        neutron_flux: PathBuf,
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
        /// Outer sweeps (downscatter-only: converges in one pass).
        #[arg(long, default_value_t = 8)]
        max_outer: u32,
        /// Axis treated as periodic; repeatable or comma-separated.
        #[arg(long, value_delimiter = ',')]
        periodic: Vec<String>,
        /// Disable the extended transport correction on the sweep —
        /// applies when the data declares `transport_mu_bar`.
        #[arg(long)]
        no_transport_correction: bool,
        /// Enable P1 anisotropic scattering — requires the photon
        /// data's P1 moments (the Klein–Nishina collapse supplies
        /// them via `transport_mu_bar`; in-group P1 needs a P1 matrix).
        #[arg(long)]
        p1: bool,
        /// Higher-order Legendre scattering l = 2..=5 — requires
        /// `--p1` and l-moment tables in the photon data (emitted by
        /// the Klein–Nishina collapse).
        #[arg(long, default_value_t = 0)]
        anisotropy: u32,
        /// Also write a folded `openbnct.physical-dose-bundle/0.2.0`
        /// photon dose to this path (the data must declare
        /// `dose_response_gy_cm2` and a `component_profile` binding).
        #[arg(long)]
        dose: Option<PathBuf>,
        /// Output path for the photon multigroup-flux JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Replace the photon component of a neutron dose bundle with a
    /// transported photon dose. `sn solve --dose` deposits capture-gamma
    /// energy where it is born (local kerma); `sn photon-solve --dose`
    /// transports those photons. This writes a copy of the neutron bundle
    /// whose `photon` component is the transported one and whose physical
    /// total is adjusted by the same difference, so total = sum of
    /// components still holds. Boron, nitrogen and hydrogen are untouched.
    MergePhotonDose {
        /// `openbnct.physical-dose-bundle/0.2.0` from `sn solve --dose`.
        #[arg(long)]
        neutron_dose: PathBuf,
        /// Photon bundle from `sn photon-solve --dose` (same case and grid).
        #[arg(long)]
        photon_dose: PathBuf,
        /// Output bundle path.
        #[arg(long)]
        output: PathBuf,
    },
    /// Place adaptive group boundaries by equal importance mass over
    /// lethargy (R17-04).
    ///
    /// `--spectrum` is an `EnergyDistribution` tabulated histogram — a
    /// `sn spectrum` extraction, a measured beam histogram, or a
    /// fine-group flux collapse — optionally folded with `--response`
    /// (a second histogram on the same binning supplying per-bin
    /// weights). Edges land so every group carries equal importance
    /// mass; emitted as a `openbnct.boundary-proposal/0.1.0` document
    /// whose `energy_boundaries_ev` drops into `sn collapse
    /// --boundaries-file` (or paste the list into `--boundaries`).
    Boundaries {
        /// Importance spectrum: `EnergyDistribution` JSON.
        #[arg(long)]
        spectrum: PathBuf,
        /// Optional response histogram (same binning as `--spectrum`).
        #[arg(long)]
        response: Option<PathBuf>,
        /// Number of groups to place.
        #[arg(long, default_value_t = 56)]
        groups: usize,
        /// Fraction of importance mass reserved for uniform-lethargy
        /// coverage — an anti-starvation guard so the source and
        /// moderation bands keep enough groups to transport the flux
        /// that pools in the importance peak. 0 disables.
        #[arg(long, default_value_t = 0.25)]
        uniform_floor: f64,
        /// New output path for the boundary-proposal document.
        #[arg(long)]
        output: PathBuf,
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

/// Inputs of the unit-mass-fraction (multi-material) component profile.
/// Required together under that profile, rejected under the base-material
/// profile.
#[derive(Debug, Args)]
struct MultiMaterialArgs {
    /// Component profile the response set was generated under (the
    /// base-material profile it is bound to).
    #[arg(long)]
    unit_source_component_profile: Option<PathBuf>,
    /// Material the response set was generated for; its B10 and N14 mass
    /// fractions normalize the response curves to per-unit-mass-fraction.
    #[arg(long)]
    unit_source_material: Option<PathBuf>,
    /// Nuclear-data manifest the response set is bound to; the deck's
    /// manifest must select the identical B10 and N14 evaluations.
    #[arg(long)]
    unit_source_nuclear_data_manifest: Option<PathBuf>,
    /// Quantization levels for `voxel_fractions` mixtures: each voxel's
    /// volume fractions are rounded to multiples of 1/LEVELS and each
    /// distinct result becomes one OpenMC material.
    #[arg(long)]
    mixture_levels: Option<u32>,
    /// Declared S(alpha,beta) table as `NUCLIDE=TABLE` (repeatable), e.g.
    /// `H1=c_H_in_H2O`: emitted as `<sab>` in every material containing the
    /// nuclide, with the table's library hash recorded in the input
    /// manifest. Omitted means free-gas scattering. Requires the
    /// `--unit-source-*` artifacts and a `cross_sections.xml` that lists the
    /// table under `thermal`.
    #[arg(long = "thermal-scattering")]
    thermal_scattering: Vec<String>,
}

impl MultiMaterialArgs {
    fn into_config(
        self,
    ) -> Result<Option<openbnct_openmc::OpenMcMultiMaterialConfig>, Box<dyn std::error::Error>>
    {
        match (
            self.unit_source_component_profile,
            self.unit_source_material,
            self.unit_source_nuclear_data_manifest,
        ) {
            (None, None, None) => {
                if self.mixture_levels.is_some() || !self.thermal_scattering.is_empty() {
                    return Err(io::Error::other(
                        "--mixture-levels and --thermal-scattering require the --unit-source-* artifacts",
                    )
                    .into());
                }
                Ok(None)
            }
            (Some(profile), Some(material), Some(manifest)) => {
                Ok(Some(openbnct_openmc::OpenMcMultiMaterialConfig {
                    unit_source_component_profile: profile,
                    unit_source_material: material,
                    unit_source_nuclear_data_manifest: manifest,
                    mixture_levels: self
                        .mixture_levels
                        .unwrap_or(openbnct_openmc::DEFAULT_MIXTURE_LEVELS),
                    thermal_scattering: self
                        .thermal_scattering
                        .iter()
                        .map(|spec| openbnct_openmc::ThermalScatteringDeclaration::parse(spec))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(io::Error::other)?,
                }))
            }
            _ => Err(io::Error::other(
                "--unit-source-component-profile, --unit-source-material and \
                 --unit-source-nuclear-data-manifest must be given together",
            )
            .into()),
        }
    }
}

#[derive(Debug, Subcommand)]
enum OpenMcCommand {
    /// Probe or acquire externally published nuclear data.
    Data(OpenMcDataArgs),
    /// Collapse an ENDF MF33 covariance block onto multigroup
    /// boundaries into an `openbnct.multigroup-covariance` artifact —
    /// the nuclear-data uncertainty source `uq propagate` consumes.
    /// NI LB ∈ {0,1,5} sub-subsections are read; anything else is
    /// reported as skipped, never silently dropped.
    CovEndf {
        /// ENDF-6 tape (`.endf`) carrying MF33.
        #[arg(long)]
        tape: PathBuf,
        /// `openbnct.multigroup-data` JSON supplying the group
        /// boundaries and material names.
        #[arg(long)]
        data: PathBuf,
        /// Material id in the multigroup data to bind the covariance to.
        #[arg(long)]
        material: String,
        /// Reaction MT whose section to read; default 1 (total).
        #[arg(long, default_value = "1")]
        mt: i32,
        /// Which multigroup parameter the covariance describes:
        /// `sigma_total` (removal XS, e.g. MT=1) or `dose_response`
        /// (reaction-kerma response, e.g. MT=107 binds the ¹⁰B(n,α)
        /// covariance to the boron component's dose response).
        /// `dose_response` requires `--component`.
        #[arg(long, default_value = "sigma_total")]
        parameter: String,
        /// Dose component for `--parameter dose_response`
        /// (`boron`, `nitrogen`, `hydrogen`, `photon`).
        #[arg(long)]
        component: Option<String>,
        /// Free-text provenance note (evaluation, source, reviewer).
        #[arg(long)]
        note: Option<String>,
        /// Output path for the covariance JSON.
        #[arg(long)]
        output: PathBuf,
    },
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
        #[command(flatten)]
        multi: MultiMaterialArgs,
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
        #[command(flatten)]
        multi: MultiMaterialArgs,
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
        /// OpenMP thread count — a first-class alias for
        /// `--env OMP_NUM_THREADS=N` so parallelism is discoverable and
        /// recorded in the run receipt's environment overlay.
        #[arg(long)]
        threads: Option<u32>,
        /// Wall-clock limit for the OpenMC process, seconds; on expiry it
        /// is killed and reaped and the command fails (logs are kept).
        #[arg(long, default_value_t = openbnct_openmc::DEFAULT_OPENMC_TIMEOUT.as_secs())]
        timeout_seconds: u64,
        /// New working directory for the run; it must not already exist.
        #[arg(long)]
        working_directory: PathBuf,
        /// New output path for the collected physical dose bundle JSON.
        #[arg(long)]
        dose_output: PathBuf,
        /// Also write the unit-concentration boron dose
        /// (`openbnct.boron-unit-dose/0.1.0`); decks generated under the
        /// unit-mass-fraction profile only.
        #[arg(long)]
        boron_unit_dose_output: Option<PathBuf>,
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
        /// Also write the unit-concentration boron dose
        /// (`openbnct.boron-unit-dose/0.1.0`, Gy per source particle per
        /// ug/g of B-10). Only decks generated under the unit-mass-fraction
        /// profile carry it.
        #[arg(long)]
        boron_unit_dose_output: Option<PathBuf>,
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
        /// `openbnct.cell-microdosimetry` JSON — required when the model
        /// is `openbnct.smk-model/0.1.0`.
        #[arg(long)]
        cell_microdosimetry: Option<PathBuf>,
        /// `openbnct.boron-microdistribution` JSON — required when the
        /// model is `openbnct.smk-model/0.1.0`; its hash is verified
        /// against the population artifact's binding.
        #[arg(long)]
        microdistribution: Option<PathBuf>,
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
    /// Stochastically sample a cell population under a
    /// `openbnct.boron-microdistribution` model: gamma-distributed
    /// uptake, Poisson captures, α/⁷Li tracks through the
    /// compartmented cell. Emits `openbnct.cell-microdosimetry/0.1.0`
    /// — the specific-energy distribution P(z), untouched fraction,
    /// and the nucleus lineal spectrum.
    CellMicrodosimetry {
        /// `openbnct.boron-microdistribution` JSON.
        #[arg(long)]
        model: PathBuf,
        /// Expected ¹⁰B captures per cell at the scenario's boron dose.
        #[arg(long)]
        mean_captures: f64,
        /// Cell population size.
        #[arg(long, default_value = "10000")]
        cells: u32,
        /// Deterministic stream seed.
        #[arg(long, default_value = "1")]
        seed: u64,
        /// Specific-energy bin edges in Gy — comma-separated, n+1.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "0,0.25,0.5,1,2,4,8,16,32"
        )]
        z_edges_gy: Vec<f64>,
        /// Lineal bin edges in keV/µm — comma-separated, n+1.
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "0,10,50,100,200,400,1000"
        )]
        y_edges_kev_um: Vec<f64>,
        /// Artifact id (default `{model_id}.cell-microdosimetry`).
        #[arg(long)]
        id: Option<String>,
        /// Provenance identifier; defaults to `cell-microdosimetry:` + id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// New output path for the cell-microdosimetry JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Apply the stochastic-microdosimetric-kinetic survival integral
    /// over a sampled cell population — SMK vs MK comparison and
    /// isosurvival RBE against a declared photon LQ reference.
    /// Emits `openbnct.smk-evaluation/0.1.0`.
    Smk {
        /// `openbnct.cell-microdosimetry` JSON.
        #[arg(long)]
        cell_microdosimetry: PathBuf,
        /// The `openbnct.boron-microdistribution` JSON the artifact
        /// was sampled under — its hash is verified against the
        /// artifact's binding.
        #[arg(long)]
        model: PathBuf,
        /// SMK domain α (Gy⁻¹).
        #[arg(long)]
        alpha: f64,
        /// SMK domain β (Gy⁻²).
        #[arg(long)]
        beta: f64,
        /// Photon-reference LQ α (Gy⁻¹).
        #[arg(long, default_value = "0.2")]
        reference_alpha: f64,
        /// Photon-reference LQ β (Gy⁻²).
        #[arg(long, default_value = "0.02")]
        reference_beta: f64,
        /// Macroscopic boron dose (Gy) the artifact's mean captures
        /// correspond to — the z-rescaling anchor.
        #[arg(long)]
        boron_dose_gy: f64,
        /// Dose levels to evaluate, Gy — comma-separated.
        #[arg(long, value_delimiter = ',', required = true)]
        dose_levels_gy: Vec<f64>,
        /// Artifact id (default `{artifact_id}.smk`).
        #[arg(long)]
        id: Option<String>,
        /// Provenance identifier; defaults to `smk:` + id.
        #[arg(long)]
        provenance_id: Option<String>,
        /// New output path for the SMK evaluation JSON.
        #[arg(long)]
        output: PathBuf,
    },
    /// Biological-parameter evidence library: search records against a
    /// declared context, inspect one record, or convert selected
    /// records into a biological-model document.
    Evidence(BioEvidenceArgs),
}

#[derive(Debug, Args)]
struct BioEvidenceArgs {
    #[command(subcommand)]
    command: BioEvidenceCommand,
}

#[derive(Debug, Subcommand)]
enum BioEvidenceCommand {
    /// Search a `openbnct.bio-evidence-library/0.1.0` library against an
    /// optional context; every record prints with its applicability
    /// (exact / partial / unsupported) and reasons.
    Search {
        /// Library JSON document.
        #[arg(long)]
        library: PathBuf,
        #[arg(long)]
        compound: Option<String>,
        #[arg(long)]
        species: Option<String>,
        #[arg(long)]
        tissue: Option<String>,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        model_family: Option<String>,
    },
    /// Print one record's full record.
    Info {
        /// Library JSON document.
        #[arg(long)]
        library: PathBuf,
        /// Record id.
        #[arg(long)]
        record: String,
    },
    /// Convert `component=record-id` bindings into a
    /// `openbnct.biological-model/0.2.0` document. A binding whose
    /// record only partially matches the declared context requires a
    /// `--assumption`; unsupported bindings fail outright.
    ToModel {
        /// Library JSON document.
        #[arg(long)]
        library: PathBuf,
        /// `COMPONENT=RECORD_ID`; all four components must be bound.
        /// repeatable.
        #[arg(long = "weight", required = true)]
        weights: Vec<String>,
        /// `REGION:COMPONENT=RECORD_ID` region override; repeatable.
        /// Each named region starts from the global weights, so the
        /// emitted region map stays a complete four-component table.
        #[arg(long = "region-weight")]
        region_weights: Vec<String>,
        /// `fixed_per_component` or `photon_isoeffective`.
        #[arg(long, default_value = "photon_isoeffective")]
        semantics: String,
        #[arg(long)]
        compound: Option<String>,
        #[arg(long)]
        species: Option<String>,
        #[arg(long)]
        tissue: Option<String>,
        #[arg(long)]
        endpoint: Option<String>,
        /// Declared transfer assumption for partial-context bindings;
        /// repeatable.
        #[arg(long = "assumption")]
        assumptions: Vec<String>,
        /// Emitted model id.
        #[arg(long)]
        id: String,
        /// New output path for the biological-model JSON.
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
    /// Derive a case-scoped nuclear-data manifest for every nuclide of a
    /// material assignment from a reviewed base manifest: tables the base
    /// already selects are reused, others are inspected from the data root.
    SelectManifest {
        /// Reviewed base manifest whose `cross_sections.xml` and
        /// distribution identity the result inherits.
        #[arg(long)]
        base_manifest: PathBuf,
        /// Root containing cross_sections.xml and every selected HDF5 file.
        #[arg(long)]
        data_root: PathBuf,
        /// Material assignment whose base material and region materials
        /// name the required nuclides.
        #[arg(long)]
        assignment: PathBuf,
        /// Identifier of the derived manifest.
        #[arg(long)]
        manifest_id: String,
        /// New output path for the manifest JSON.
        #[arg(long)]
        output: PathBuf,
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

/// Whether every scattering material in the data carries a P1 scatter
/// matrix — the condition under which `sn solve` defaults to P1.
fn data_supports_p1(data: &openbnct_transport::MultigroupData) -> bool {
    data.materials.iter().all(|m| {
        m.scatter_p1_matrix_per_cm.is_some() || m.scatter_matrix_per_cm.iter().all(|&v| v == 0.0)
    })
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

/// Render an Avify certificate: per-ROI certified interval vs criterion,
/// action, applicability flags, and the recorded run cost. The envelope
/// is labelled what the engine says it is — empirical, not certified.
fn print_avify_certificate(cert: &openbnct_avify::AvifyCertificate) {
    println!(
        "avify certificate — empirical two-evaluation envelope (research only, not a certified bound)"
    );
    for (roi, action) in &cert.actions {
        let flags = cert.brackets.get(roi).map(|b| {
            let mut f = String::new();
            if b.photon_valid == Some(false) {
                f.push_str(" photon-decomposition-invalid");
            }
            if b.fast_applicability_ok == Some(false) {
                f.push_str(" fast-allowance-failed");
            }
            f
        });
        println!(
            "  {roi:8} {} {:>5.1} Gy-w: certified [{:.2}, {:.2}]{} -> {}",
            action.criterion.0,
            action.criterion.1,
            action.certified_gyw[0],
            action.certified_gyw[1],
            match action.nominal_gyw {
                Some(n) => format!(", nominal {n:.2}"),
                None => String::new(),
            },
            action.action
        );
        if let Some(f) = flags.filter(|f| !f.is_empty()) {
            println!("           flags:{f}");
        }
    }
    for (name, run) in &cert.runs {
        println!(
            "  run {name:12} histories={} wall={:.0}s seed={}",
            run.histories, run.wall_s, run.seed
        );
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
        Some(Command::Project(args)) => project::run_project(args)?,
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
                            openbnct_transport::MaterialRegionShape::VoxelFractions { .. } =>
                                "voxel fractions".to_owned(),
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
            BeamCommand::Bind {
                beam,
                case,
                output,
                aim_mask,
                approach,
            } => {
                let beam: openbnct_transport::BeamDescription =
                    serde_json::from_slice(&fs::read(&beam)?)?;
                let mut bound: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                bound.source = beam
                    .bound_source(&bound.geometry)
                    .map_err(|error| io::Error::other(format!("beam bind: {error}")))?;
                if let (Some(mask), Some(approach)) = (&aim_mask, &approach) {
                    bound.source = aim_bound_source(&bound, &read_region_mask(mask)?, approach)?;
                }
                bound
                    .validate()
                    .map_err(|error| io::Error::other(format!("bound case is invalid: {error}")))?;
                write_new_json(&output, &bound)?;
                println!("bound {} onto {}", beam.id, bound.case_id);
                println!("case: {}", output.display());
            }
            BeamCommand::Build {
                id,
                name,
                facility,
                spectrum_csv,
                port_axis,
                port_offset_cm,
                radius_cm,
                center_uv_cm,
                divergence_deg,
                fluence_rate_cm2_s,
                note,
                citations,
                output,
            } => {
                use openbnct_transport::{
                    AngularDistribution, BeamDescription, BeamProvenance, EnergyDistribution,
                    FixedSourceDefinition, NormalizationBasis, ParticleType, PortGeometry,
                    PortShape, SourceSpatialDistribution,
                };
                let axis = match port_axis.as_str() {
                    "x" => openbnct_transport::PlaneAxis::X,
                    "y" => openbnct_transport::PlaneAxis::Y,
                    "z" => openbnct_transport::PlaneAxis::Z,
                    other => {
                        return Err(
                            format!("--port-axis must be x, y, or z (got {other:?})").into()
                        );
                    }
                };
                if center_uv_cm.len() != 2 {
                    return Err("--center-uv-cm takes exactly two values u,v".into());
                }
                let (edges, weights) = read_spectrum_csv(&spectrum_csv)?;
                let beam = BeamDescription {
                    schema_version: openbnct_transport::BEAM_DESCRIPTION_SCHEMA.into(),
                    id: id.clone(),
                    name,
                    facility,
                    port: PortGeometry {
                        axis,
                        offset_cm: port_offset_cm,
                        shape: PortShape::Circle {
                            center_uv_cm: [center_uv_cm[0], center_uv_cm[1]],
                            radius_cm,
                        },
                    },
                    source: FixedSourceDefinition {
                        schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
                        id: id.clone(),
                        particle: ParticleType::Neutron,
                        source_sites_per_history: 1,
                        statistical_weight_per_site: 1.0,
                        space: SourceSpatialDistribution::UniformDisk {
                            axis,
                            offset_cm: port_offset_cm,
                            center_uv_cm: [center_uv_cm[0], center_uv_cm[1]],
                            radius_cm,
                        },
                        angle: AngularDistribution::IsotropicCone {
                            axis_unit_vector: match axis {
                                openbnct_transport::PlaneAxis::X => [1.0, 0.0, 0.0],
                                openbnct_transport::PlaneAxis::Y => [0.0, 1.0, 0.0],
                                openbnct_transport::PlaneAxis::Z => [0.0, 0.0, 1.0],
                            },
                            half_angle_rad: divergence_deg.to_radians(),
                        },
                        energy: EnergyDistribution::TabulatedHistogram {
                            energy_boundaries_ev: edges,
                            bin_weights: weights,
                        },
                    },
                    normalization: match fluence_rate_cm2_s {
                        Some(rate) => NormalizationBasis::FluenceRateAtPort {
                            fluence_rate_cm2_s: rate,
                        },
                        None => NormalizationBasis::PerSourceParticle,
                    },
                    provenance: BeamProvenance::MeasuredCharacterization {
                        citations: citations
                            .iter()
                            .map(|raw| {
                                let f: Vec<&str> = raw.splitn(5, ';').collect();
                                openbnct_transport::Citation {
                                    authors: f.first().unwrap_or(&"").trim().into(),
                                    title: f.get(1).unwrap_or(&"").trim().into(),
                                    venue: f.get(2).unwrap_or(&"").trim().into(),
                                    year: f.get(3).and_then(|y| y.trim().parse().ok()).unwrap_or(0),
                                    doi: None,
                                    url: None,
                                }
                            })
                            .collect(),
                        derivation_note: note.unwrap_or_else(|| {
                            format!(
                                "user-declared beam built by `openbnct beam build` from {}",
                                spectrum_csv.display()
                            )
                        }),
                    },
                };
                beam.validate()
                    .map_err(|error| format!("beam description invalid: {error}"))?;
                write_new_json(&output, &beam)?;
                println!("beam: {}", output.display());
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
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose_path)?;
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
                        openbnct_core::sidecar::load_json(flux_path)?;
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
        Some(Command::Avify(args)) => match args.command {
            AvifyCommand::ExportPlan {
                case,
                assignment,
                spec,
                prefix,
            } => {
                let case: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                let assignment: MaterialAssignment =
                    serde_json::from_slice(&fs::read(&assignment)?)?;
                let spec: openbnct_avify::AvifySpec = serde_json::from_slice(&fs::read(&spec)?)?;
                let export = openbnct_avify::export_voxel_plan(&case, &assignment, &spec, &prefix)
                    .map_err(|e| io::Error::other(format!("avify export: {e}")))?;
                let plan = openbnct_avify::AvifyPlan {
                    declared_set: spec.plan.declared_set.clone(),
                    brain_ratio: spec.plan.brain_ratio,
                    weights: spec.plan.weights.clone(),
                    criteria: spec.plan.criteria.clone(),
                    normalisation: spec.plan.normalisation.clone(),
                    histories: spec.plan.histories.clone(),
                    seeds: spec.plan.seeds.clone(),
                    beam: spec.plan.beam.clone(),
                };
                let plan_path = PathBuf::from(format!("{}.plan.json", prefix.display()));
                write_new_json(&plan_path, &plan)?;
                println!("voxel plan: {}", export.arrays_path.display());
                println!("  meta:     {}", export.meta_path.display());
                println!("  plan:     {}", plan_path.display());
                println!("  arrays sha256: {}", export.arrays_sha256);
                println!("  meta   sha256: {}", export.meta_sha256);
                for (class, count) in &export.class_voxels {
                    println!("  class {class:<8} {count} voxels");
                }
            }
            AvifyCommand::Verify {
                case,
                assignment,
                spec,
                outdir,
                engine_cmd,
                threads,
                timeout_s,
            } => {
                let output = openbnct_avify::verify_pipeline(
                    &case,
                    &assignment,
                    &spec,
                    &outdir,
                    &engine_cmd,
                    timeout_s,
                    threads,
                )
                .map_err(|e| io::Error::other(format!("avify: {e}")))?;
                println!(
                    "voxel plan exported to {}",
                    openbnct_avify::pipeline::verify_prefix(&outdir).display()
                );
                println!("  arrays sha256: {}", output.export.arrays_sha256);
                println!("  meta   sha256: {}", output.export.meta_sha256);
                println!(
                    "engine finished in {:.0}s — certificate: {}",
                    output.outcome.elapsed.as_secs_f64(),
                    output.outcome.certificate_path.display()
                );
                println!("run receipt: {}", output.receipt_path.display());
                print_avify_certificate(&output.outcome.certificate);
            }
            AvifyCommand::Show { certificate } => {
                let cert = openbnct_avify::AvifyCertificate::load(&certificate)
                    .map_err(|e| io::Error::other(format!("avify certificate: {e}")))?;
                print_avify_certificate(&cert);
                if let Some(engine) = &cert.engine {
                    println!(
                        "engine: {}{}",
                        engine.name.as_deref().unwrap_or("avify-dose"),
                        engine
                            .version
                            .as_deref()
                            .map(|v| format!(" {v}"))
                            .unwrap_or_default()
                    );
                }
                if let Some(dir) = certificate.parent() {
                    match openbnct_avify::review_state(dir)
                        .map_err(|e| io::Error::other(format!("avify review: {e}")))?
                    {
                        openbnct_avify::ReviewState::Current(review) => {
                            println!("review: REVIEWED by {}", review.reviewer)
                        }
                        openbnct_avify::ReviewState::Stale { .. } => {
                            println!("review: STALE — certificate changed since review")
                        }
                        openbnct_avify::ReviewState::Missing => {}
                    }
                }
            }
            AvifyCommand::Status { receipt } => {
                let receipt = openbnct_avify::AvifyRunReceipt::load(&receipt)
                    .map_err(|e| io::Error::other(format!("avify receipt: {e}")))?;
                let base = PathBuf::from(&receipt.certificate.path)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("."));
                let states = openbnct_avify::check_staleness(&receipt, &base);
                println!(
                    "avify run receipt — engine {} ({}), certificate {}",
                    receipt.engine.version,
                    receipt.engine.argv0.join(" "),
                    &receipt.certificate.sha256[..16]
                );
                // Measured timing breakdown — absent on pre-0.2.1
                // receipts stays absent rather than printing zeros.
                if receipt.timing.total_s > 0.0 {
                    println!(
                        "  timing (measured): export {:.1}s, engine {:.1}s, bind {:.1}s — total {:.1}s",
                        receipt.timing.export_s,
                        receipt.timing.engine_s,
                        receipt.timing.bind_s,
                        receipt.timing.total_s
                    );
                }
                if let Some(n) = receipt.resources.available_parallelism {
                    let threads = receipt
                        .engine
                        .threads
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "engine default".into());
                    println!(
                        "  resources: {n} logical cores; engine threads {threads}, bound {}s",
                        receipt.engine.timeout_s
                    );
                }
                if receipt.cold_start {
                    println!(
                        "  run kind: first run in this directory (cold — includes one-time setup)"
                    );
                }
                for warning in &receipt.warnings {
                    println!("  warning: {warning}");
                }
                match openbnct_avify::review_state(&base)
                    .map_err(|e| io::Error::other(format!("avify review: {e}")))?
                {
                    openbnct_avify::ReviewState::Current(review) => println!(
                        "  review: REVIEWED by {}{}",
                        review.reviewer,
                        if review.note.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", review.note)
                        }
                    ),
                    openbnct_avify::ReviewState::Stale { .. } => {
                        println!("  review: STALE — the certificate changed since it was reviewed")
                    }
                    openbnct_avify::ReviewState::Missing => {}
                }
                let mut stale = false;
                for (name, state) in &states {
                    let label = match state {
                        openbnct_avify::InputState::Current => "current".to_string(),
                        openbnct_avify::InputState::Changed(digest) => {
                            stale = true;
                            format!("CHANGED — now {digest}")
                        }
                        openbnct_avify::InputState::Missing => {
                            stale = true;
                            "MISSING".to_string()
                        }
                    };
                    println!("  {name:12} {label}");
                }
                println!(
                    "verdict: {}",
                    if stale {
                        "STALE — inputs changed since this run; the certificate no longer \
                         describes the current inputs (re-run `openbnct avify verify`)"
                    } else {
                        "CURRENT — all bound inputs match the receipt"
                    }
                );
            }
            AvifyCommand::Review {
                outdir,
                reviewer,
                note,
            } => {
                let review = openbnct_avify::write_review(&outdir, &reviewer, &note)
                    .map_err(|e| io::Error::other(format!("avify review: {e}")))?;
                println!(
                    "review.json written — {} reviewed certificate {}…",
                    review.reviewer,
                    &review.certificate_sha256[..16]
                );
            }
            AvifyCommand::Diff { before, after } => {
                let d = openbnct_avify::diff_runs(&before, &after)
                    .map_err(|e| io::Error::other(format!("avify diff: {e}")))?;
                println!(
                    "avify diff — engine {} -> {}, total {:.1}s -> {:.1}s",
                    d.engine_before, d.engine_after, d.elapsed_before_s, d.elapsed_after_s
                );
                if !d.input_changes.is_empty() {
                    println!("  inputs changed:");
                    for c in &d.input_changes {
                        println!(
                            "    {:12} {} -> {}",
                            c.name, c.sha256_before, c.sha256_after
                        );
                    }
                }
                for (roi, c) in &d.roi_changes {
                    let arrow = if c.action_changed { "*" } else { " " };
                    println!(
                        "  {arrow}{roi:8} [{:.2}, {:.2}] -> [{:.2}, {:.2}] Gy-w  {} -> {}",
                        c.certified_before[0],
                        c.certified_before[1],
                        c.certified_after[0],
                        c.certified_after[1],
                        c.action_before,
                        c.action_after
                    );
                }
                for roi in &d.only_before {
                    println!("  -{roi:8} present only in the earlier run");
                }
                for roi in &d.only_after {
                    println!("  +{roi:8} present only in the later run");
                }
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
                    openbnct_core::sidecar::load_json(&bundle)?;
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
            DicomCommand::ImportMr { slices, output } => {
                let volume = openbnct_dicom::import_mr_series(&slices)
                    .map_err(|error| io::Error::other(format!("mr import: {error}")))?;
                let image = NiftiImage {
                    geometry: volume.geometry.clone(),
                    values: volume.intensities.clone(),
                    datatype: DT_FLOAT64,
                    transform_source: "sform",
                    description: format!("openbnct MR {}", volume.series_instance_uid),
                    intent_name: String::new(),
                    units_declared_mm: true,
                };
                write_nifti(&image, &output)?;
                println!(
                    "mr: {} voxels -> {}",
                    volume.intensities.len(),
                    output.display()
                );
                println!(
                    "  TR {} ms | TE {} ms | FoR {}",
                    volume
                        .repetition_time_ms
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "n/a".into()),
                    volume
                        .echo_time_ms
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "n/a".into()),
                    volume.frame_of_reference_uid
                );
            }
            DicomCommand::ImportPet { slices, output } => {
                let volume = openbnct_dicom::import_pet_series(&slices)
                    .map_err(|error| io::Error::other(format!("pet import: {error}")))?;
                let image = NiftiImage {
                    geometry: volume.geometry.clone(),
                    values: volume.suv.clone(),
                    datatype: DT_FLOAT64,
                    transform_source: "sform",
                    description: format!("openbnct SUVbw {}", volume.series_instance_uid),
                    intent_name: String::new(),
                    units_declared_mm: true,
                };
                write_nifti(&image, &output)?;
                println!(
                    "pet: {} SUVbw voxels -> {}",
                    volume.suv.len(),
                    output.display()
                );
                println!(
                    "  weight {:.1} kg | dose {:.0} -> {:.0} MBq (Δt {:.0} s, T½ {:.0} s) | clamped {}",
                    volume.patient_weight_kg,
                    volume.injected_dose_bq / 1e6,
                    volume.decayed_dose_bq / 1e6,
                    volume.delta_t_s,
                    volume.radionuclide_half_life_s,
                    volume.clamped_negative_voxels
                );
            }
            DicomCommand::SynthPet {
                output,
                core_suv,
                background_suv,
                weight_kg,
                dose_mbq,
            } => {
                let spec = openbnct_dicom::synthetic::SyntheticPetSpec {
                    patient_weight_kg: weight_kg,
                    injected_dose_bq: dose_mbq * 1e6,
                    core_suv,
                    background_suv,
                    ..Default::default()
                };
                let files = openbnct_dicom::synthetic::write_pet_series(&output, &spec)
                    .map_err(|error| io::Error::other(format!("synth-pet: {error}")))?;
                println!(
                    "synth-pet: {} slices (BQML, core SUV {core_suv}, background {background_suv}) -> {}",
                    files.len(),
                    output.display()
                );
            }
            DicomCommand::ImportCt {
                series,
                slices,
                rtstruct,
                spacing_mm,
                case_id,
                base_material,
                case_output,
                hu_output,
                masks_dir,
            } => {
                cmd_dicom_import_ct(
                    series,
                    slices,
                    rtstruct,
                    &spacing_mm,
                    case_id,
                    &base_material,
                    &case_output,
                    &hu_output,
                    masks_dir.as_deref(),
                )?;
            }
            DicomCommand::Calibrate {
                calibration,
                slices,
                hu_nifti,
                case,
                output,
                materials_out_dir,
                report,
            } => {
                if slices.is_empty() == hu_nifti.is_none() {
                    return Err(io::Error::other(
                        "exactly one of --slices or --hu-nifti is required",
                    )
                    .into());
                }
                let cal: openbnct_transport::HuCalibration =
                    serde_json::from_slice(&fs::read(&calibration)?)?;
                let case_doc: TransportCase = serde_json::from_slice(&fs::read(&case)?)?;
                let (hu_values, hu_geometry) = if !slices.is_empty() {
                    let ct = openbnct_dicom::import_ct_series(&slices)
                        .map_err(|error| io::Error::other(format!("ct import: {error}")))?;
                    let values = ct
                        .stored_pixels
                        .iter()
                        .map(|&px| ct.modality_value(px))
                        .collect();
                    (values, ct.geometry.clone())
                } else {
                    let image = read_nifti_file(hu_nifti.as_ref().unwrap())
                        .map_err(|error| io::Error::other(format!("hu nifti: {error}")))?;
                    (image.values, image.geometry)
                };
                if hu_geometry.shape != case_doc.geometry.shape {
                    return Err(io::Error::other(format!(
                        "hu volume shape {:?} does not match the case grid {:?} — \
                         reslice the CT onto the case grid first",
                        hu_geometry.shape, case_doc.geometry.shape
                    ))
                    .into());
                }
                let provenance = format!("hu-calibration:{}", cal.id);
                let (assignment, cal_report) = cal
                    .apply(
                        &hu_values,
                        &case_doc.geometry,
                        &case_doc.case_id,
                        &provenance,
                    )
                    .map_err(|error| io::Error::other(format!("calibration: {error}")))?;
                assignment
                    .validate(&case_doc.geometry)
                    .map_err(|error| io::Error::other(format!("assignment: {error}")))?;
                write_new_json(&output, &assignment)?;
                if let Some(dir) = materials_out_dir {
                    fs::create_dir_all(&dir)?;
                    for material in cal.materials() {
                        write_new_json(&dir.join(format!("{}.json", material.id)), material)?;
                    }
                }
                if let Some(path) = report {
                    write_new_json(&path, &cal_report)?;
                }
                println!(
                    "calibrate: {} voxels -> {} | anchors {} | interpolated {} | clamped +{}/-{}",
                    hu_values.len(),
                    output.display(),
                    cal.anchors.len(),
                    cal_report.interpolated,
                    cal_report.clamped_high,
                    cal_report.clamped_low
                );
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
                OpenMcDataCommand::SelectManifest {
                    base_manifest,
                    data_root,
                    assignment,
                    manifest_id,
                    output,
                } => {
                    let base: NuclearDataManifest =
                        serde_json::from_slice(&fs::read(&base_manifest)?)?;
                    let assignment: openbnct_transport::MaterialAssignment =
                        serde_json::from_slice(&fs::read(&assignment)?)?;
                    let mut nuclides = std::collections::BTreeSet::new();
                    for material in std::iter::once(&assignment.base_material)
                        .chain(assignment.regions.iter().map(|region| &region.material))
                    {
                        nuclides.extend(material.nuclides.iter().map(|n| n.name.clone()));
                    }
                    // The unit-mass-fraction response is folded for B-10
                    // whether or not a material carries it.
                    nuclides.insert("B10".to_owned());
                    let derived = openbnct_openmc::select_manifest(
                        &base,
                        &data_root,
                        &nuclides,
                        &manifest_id,
                    )?;
                    write_new_json(&output, &derived)?;
                    println!(
                        "manifest {}: {} neutron tables, {} photon tables ({} inspected from the data root)",
                        derived.id,
                        derived.neutron_tables.len(),
                        derived.photon_tables.len(),
                        derived
                            .neutron_tables
                            .iter()
                            .filter(|t| base.neutron_table(&t.nuclide).is_none())
                            .count()
                            + derived
                                .photon_tables
                                .iter()
                                .filter(|t| !base
                                    .photon_tables
                                    .iter()
                                    .any(|b| b.element == t.element))
                                .count()
                    );
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
                multi,
                output,
            } => {
                let multi = multi.into_config()?;
                let multi_bytes = multi
                    .as_ref()
                    .map(|config| -> Result<_, io::Error> {
                        Ok((
                            fs::read(&config.unit_source_component_profile)?,
                            fs::read(&config.unit_source_material)?,
                            fs::read(&config.unit_source_nuclear_data_manifest)?,
                            config.mixture_levels,
                            config.thermal_scattering.clone(),
                        ))
                    })
                    .transpose()?;
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
                        multimaterial: multi_bytes.as_ref().map(
                            |(profile, material, manifest, levels, thermal)| {
                                openbnct_openmc::MultiMaterialInputs {
                                    unit_response_source:
                                        openbnct_openmc::UnitResponseSourceArtifacts {
                                            component_profile_json: profile,
                                            material_json: material,
                                            nuclear_data_manifest_json: manifest,
                                        },
                                    mixture_levels: *levels,
                                    thermal_scattering: thermal,
                                }
                            },
                        ),
                    },
                )?;
                deck.write_new(&output)?;
                if let Some(realization) = &deck.manifest.material_realization {
                    println!(
                        "unit-mass-fraction profile: {} realized materials, {} mixture voxels (levels {}, max fraction error {:.4})",
                        realization.materials.len(),
                        realization.mixture_voxel_count,
                        realization.levels,
                        realization.max_fraction_error
                    );
                }
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
                multi,
                nuclear_data_root,
                openmc,
                environment,
                threads,
                timeout_seconds,
                working_directory,
                dose_output,
                boron_unit_dose_output,
                evidence_root,
            } => {
                if timeout_seconds == 0 {
                    return Err(io::Error::other("--timeout-seconds must be ≥ 1").into());
                }
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
                    multimaterial: multi.into_config()?,
                    nuclear_data_root,
                };
                let mut backend = OpenMcBackend::new(&openmc)
                    .configured(config)
                    .with_timeout(std::time::Duration::from_secs(timeout_seconds));
                if let Some(threads) = threads {
                    if threads == 0 {
                        return Err(io::Error::other("--threads must be ≥ 1").into());
                    }
                    backend = backend.with_env("OMP_NUM_THREADS", threads.to_string());
                }
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
                let json = openbnct_core::sidecar::to_vec_pretty_for(&bundle, &dose_output, false)?;
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
                if let Some(unit_output) = boron_unit_dose_output {
                    let run = openbnct_openmc::collect_statepoint_full(&working_directory)?;
                    let unit = run.boron_unit_dose.ok_or_else(|| {
                        io::Error::other(
                            "this deck was not generated under the unit-mass-fraction profile; no boron unit dose exists",
                        )
                    })?;
                    write_new_json(&unit_output, &unit)?;
                    println!("wrote boron unit dose at {}", unit_output.display());
                }

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
                boron_unit_dose_output,
            } => {
                let completed = CompletedRun {
                    backend_id: "openmc".into(),
                    case_id: String::new(),
                    working_directory: working_directory.display().to_string(),
                    exit_code,
                };
                let bundle = OpenMcBackend::default().collect(&completed)?;
                let json = openbnct_core::sidecar::to_vec_pretty_for(&bundle, &output, false)?;
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
                if let Some(unit_output) = boron_unit_dose_output {
                    let run = openbnct_openmc::collect_statepoint_full(&working_directory)?;
                    let unit = run.boron_unit_dose.ok_or_else(|| {
                        io::Error::other(
                            "this deck was not generated under the unit-mass-fraction profile; no boron unit dose exists",
                        )
                    })?;
                    let json =
                        openbnct_core::sidecar::to_vec_pretty_for(&unit, &unit_output, false)?;
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&unit_output)?;
                    file.write_all(&json)?;
                    file.write_all(b"\n")?;
                    file.sync_all()?;
                    println!("wrote boron unit dose at {}", unit_output.display());
                }
            }
            OpenMcCommand::CovEndf {
                tape,
                data,
                material,
                mt,
                parameter,
                component,
                note,
                output,
            } => {
                let cov_parameter = match parameter.as_str() {
                    "sigma_total" => openbnct_transport::CovarianceParameter::SigmaTotal,
                    "dose_response" => openbnct_transport::CovarianceParameter::DoseResponse,
                    other => {
                        return Err(io::Error::other(format!(
                            "--parameter must be sigma_total or dose_response (got {other:?})"
                        ))
                        .into());
                    }
                };
                if cov_parameter == openbnct_transport::CovarianceParameter::DoseResponse
                    && component.is_none()
                {
                    return Err(
                        io::Error::other("--parameter dose_response requires --component").into(),
                    );
                }
                let tape_bytes = fs::read(&tape)?;
                let tape_text = String::from_utf8_lossy(&tape_bytes);
                let parsed = openbnct_openmc::endf_mf33::parse_mf33(&tape_text, Some(mt))
                    .map_err(|e| io::Error::other(format!("mf33: {e}")))?;
                let data_bytes = fs::read(&data)?;
                let mg: openbnct_transport::MultigroupData = serde_json::from_slice(&data_bytes)
                    .map_err(|error| {
                        io::Error::other(format!("data {}: {error}", data.display()))
                    })?;
                let boundaries = &mg.energy_boundaries_ev;
                let groups = mg.group_count();
                let diagonal = Vec::new();
                let mut blocks = Vec::new();
                for cov in &parsed.covariances {
                    let Some(collapsed) =
                        openbnct_openmc::endf_mf33::collapse_covariance_to_groups(cov, boundaries)
                    else {
                        continue;
                    };
                    let rel: Vec<f64> = (0..groups)
                        .map(|g| collapsed[g * groups + g].max(0.0).sqrt())
                        .collect();
                    let mut corr = vec![0.0; groups * groups];
                    for g in 0..groups {
                        for h in 0..groups {
                            let denom = rel[g] * rel[h];
                            corr[g * groups + h] = if denom > 0.0 {
                                (collapsed[g * groups + h] / denom).clamp(-1.0, 1.0)
                            } else {
                                if g == h { 1.0 } else { 0.0 }
                            };
                        }
                    }
                    if rel.iter().all(|r| *r == 0.0) {
                        continue;
                    }
                    blocks.push(openbnct_transport::CovarianceBlock {
                        material_id: material.clone(),
                        parameter: cov_parameter,
                        component: component.clone(),
                        relative_std_dev: rel,
                        correlation: corr,
                    });
                }
                let mut note_text = note.unwrap_or_else(|| {
                    format!(
                        "ENDF MF33 collapse of {} (MT{}) — NI LB∈{{0,1,5}} only; {} sub-blocks parsed, {} skipped",
                        tape.display(),
                        mt,
                        parsed.covariances.len(),
                        parsed.skipped.len()
                    )
                });
                if !parsed.skipped.is_empty() {
                    note_text.push_str(&format!(" | skipped: {}", parsed.skipped.join("; ")));
                }
                let covariance = openbnct_transport::MultigroupCovariance {
                    schema_version: openbnct_transport::MULTIGROUP_COVARIANCE_SCHEMA.into(),
                    id: format!("openbnct.covariance.{}.mt{}", material, mt),
                    multigroup_data: openbnct_core::ContentReference {
                        id: mg.id.clone(),
                        sha256: format!("sha256:{}", openbnct_evidence::sha256_hex(&data_bytes)),
                    },
                    diagonal,
                    blocks,
                    provenance_note: note_text,
                    qualification: "nuclear_data_covariance_research_only_not_clinical".into(),
                };
                write_new_json(&output, &covariance)?;
                println!("covariance artifact at {}", output.display());
                println!(
                    "blocks: {} ({} parsed, {} skipped)",
                    covariance.blocks.len(),
                    parsed.covariances.len(),
                    parsed.skipped.len()
                );
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
                cell_microdosimetry,
                microdistribution,
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
                let physical: PhysicalDoseBundle =
                    openbnct_core::sidecar::from_slice_at(&bundle_bytes, &physical_bundle)?;
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
                } else if openbnct_core::schema_matches(&schema, openbnct_bio::SMK_MODEL_SCHEMA) {
                    if !spectra.is_empty() {
                        return Err(io::Error::other(
                            "--spectrum applies only to openbnct.microdosimetric-model/0.1.0 models",
                        )
                        .into());
                    }
                    let smk_model: openbnct_bio::SmkModel = serde_json::from_slice(&model_bytes)?;
                    let artifact_path = cell_microdosimetry.ok_or_else(|| {
                        io::Error::other(
                            "--cell-microdosimetry is required for openbnct.smk-model models",
                        )
                    })?;
                    let microdist_path = microdistribution.ok_or_else(|| {
                        io::Error::other(
                            "--microdistribution is required for openbnct.smk-model models",
                        )
                    })?;
                    let artifact_bytes = fs::read(&artifact_path)?;
                    let artifact: openbnct_bio::CellMicrodosimetry =
                        serde_json::from_slice(&artifact_bytes).map_err(|e| {
                            io::Error::other(format!(
                                "cell-microdosimetry {}: {e}",
                                artifact_path.display()
                            ))
                        })?;
                    let microdist_bytes = fs::read(&microdist_path)?;
                    let microdist: openbnct_boron::BoronMicrodistribution =
                        serde_json::from_slice(&microdist_bytes).map_err(|e| {
                            io::Error::other(format!(
                                "microdistribution {}: {e}",
                                microdist_path.display()
                            ))
                        })?;
                    openbnct_bio::apply_smk_model(
                        &smk_model,
                        &model_bytes,
                        &artifact,
                        &artifact_bytes,
                        &microdist,
                        &microdist_bytes,
                        &physical,
                    )?
                } else if openbnct_core::schema_matches(
                    &schema,
                    openbnct_bio::ISOEFFECTIVE_MODEL_SCHEMA,
                ) {
                    if !spectra.is_empty() {
                        return Err(io::Error::other(
                            "--spectrum applies only to openbnct.microdosimetric-model/0.1.0 models",
                        )
                        .into());
                    }
                    let model: openbnct_bio::IsoeffectiveModel =
                        serde_json::from_slice(&model_bytes)?;
                    openbnct_bio::apply_isoeffective_model(&model, &model_bytes, &physical, &masks)?
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
                let json = openbnct_core::sidecar::to_vec_pretty_for(&bundle, &output, false)?;
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
                    openbnct_core::sidecar::from_slice_at(&flux_bytes, &flux)?;
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
                    bed_from_external(&dose_bundle, alpha_beta, &overrides, &masks, quantity, &[])?;
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
                    openbnct_core::sidecar::from_slice_at(&primary_bytes, &primary)?;
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
                    openbnct_core::sidecar::load_json(&a)?;
                let b_bundle: openbnct_bio::BiologicalDoseBundle =
                    openbnct_core::sidecar::load_json(&b)?;
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
                let physical: PhysicalDoseBundle =
                    openbnct_core::sidecar::from_slice_at(&bundle_bytes, &physical_bundle)?;
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
            BioCommand::CellMicrodosimetry {
                model,
                mean_captures,
                cells,
                seed,
                z_edges_gy,
                y_edges_kev_um,
                id,
                provenance_id,
                output,
            } => {
                let model_bytes = fs::read(&model)?;
                let microdistribution: openbnct_boron::BoronMicrodistribution =
                    serde_json::from_slice(&model_bytes).map_err(|error| {
                        io::Error::other(format!("model {}: {error}", model.display()))
                    })?;
                let id =
                    id.unwrap_or_else(|| format!("{}.cell-microdosimetry", microdistribution.id));
                let artifact = openbnct_bio::sample_cell_microdosimetry(
                    &microdistribution,
                    openbnct_core::ContentReference {
                        id: microdistribution.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&model_bytes),
                    },
                    mean_captures,
                    cells,
                    seed,
                    &z_edges_gy,
                    &y_edges_kev_um,
                    &id,
                    provenance_id
                        .as_deref()
                        .unwrap_or(&format!("cell-microdosimetry:{id}")),
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &artifact)?;
                println!("cell microdosimetry at {}", output.display());
                println!(
                    "{} cells · {} captures · {} nucleus hits · untouched {:.1}% · z̄ {:.3e} Gy",
                    artifact.statistics.cells_simulated,
                    artifact.statistics.captures_simulated,
                    artifact.statistics.nucleus_hits,
                    artifact.untouched_fraction * 100.0,
                    artifact.mean_specific_energy_gy
                );
            }
            BioCommand::Smk {
                cell_microdosimetry,
                model,
                alpha,
                beta,
                reference_alpha,
                reference_beta,
                boron_dose_gy,
                dose_levels_gy,
                id,
                provenance_id,
                output,
            } => {
                let artifact_bytes = fs::read(&cell_microdosimetry)?;
                let artifact: openbnct_bio::CellMicrodosimetry =
                    serde_json::from_slice(&artifact_bytes).map_err(|error| {
                        io::Error::other(format!(
                            "artifact {}: {error}",
                            cell_microdosimetry.display()
                        ))
                    })?;
                let model_bytes = fs::read(&model)?;
                let microdistribution: openbnct_boron::BoronMicrodistribution =
                    serde_json::from_slice(&model_bytes).map_err(|error| {
                        io::Error::other(format!("model {}: {error}", model.display()))
                    })?;
                let expected = openbnct_evidence::sha256_hex(&model_bytes);
                if artifact.microdistribution.sha256 != expected {
                    return Err(io::Error::other(format!(
                        "model {} hash does not match the artifact's microdistribution binding",
                        model.display()
                    ))
                    .into());
                }
                let id = id.unwrap_or_else(|| format!("{}.smk", artifact.id));
                let evaluation = openbnct_bio::evaluate_smk(
                    &artifact,
                    openbnct_core::ContentReference {
                        id: artifact.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&artifact_bytes),
                    },
                    &microdistribution,
                    openbnct_bio::SmkParameters {
                        alpha_per_gy: alpha,
                        beta_per_gy2: beta,
                        reference_alpha_per_gy: reference_alpha,
                        reference_beta_per_gy2: reference_beta,
                        boron_dose_gy_at_mean_captures: boron_dose_gy,
                        dose_levels_gy,
                    },
                    &id,
                    provenance_id.as_deref().unwrap_or(&format!("smk:{id}")),
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &evaluation)?;
                println!("smk evaluation at {}", output.display());
                for point in &evaluation.points {
                    println!(
                        "  D={:.3} Gy: SMK S={:.4} · MK S={:.4} · RBE {}",
                        point.dose_gy,
                        point.smk_survival,
                        point.mk_survival,
                        point
                            .rbe
                            .map(|r| format!("{r:.3}"))
                            .unwrap_or_else(|| "n/a".into())
                    );
                }
            }
            BioCommand::Evidence(args) => match args.command {
                BioEvidenceCommand::Search {
                    library,
                    compound,
                    species,
                    tissue,
                    endpoint,
                    model_family,
                } => {
                    let lib: openbnct_bio::BioEvidenceLibrary =
                        serde_json::from_slice(&fs::read(&library)?)?;
                    lib.validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    let query = openbnct_bio::ContextQuery {
                        compound,
                        species,
                        tissue,
                        endpoint,
                        model_family,
                    };
                    for (record, applicability) in openbnct_bio::search(&lib, &query) {
                        let label = match &applicability {
                            openbnct_bio::Applicability::Exact => "exact".to_string(),
                            openbnct_bio::Applicability::Partial(r) => {
                                format!("partial ({})", r.join("; "))
                            }
                            openbnct_bio::Applicability::Unsupported(r) => {
                                format!("unsupported ({})", r.join("; "))
                            }
                        };
                        println!(
                            "  {:36} {:14} {} {} — {}",
                            record.id,
                            record.parameter,
                            record.estimate.value,
                            record.estimate.unit,
                            label
                        );
                    }
                }
                BioEvidenceCommand::Info { library, record } => {
                    let lib: openbnct_bio::BioEvidenceLibrary =
                        serde_json::from_slice(&fs::read(&library)?)?;
                    lib.validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    let found = lib
                        .record(&record)
                        .ok_or_else(|| io::Error::other(format!("unknown record {record:?}")))?;
                    println!(
                        "{}: {} = {} {}",
                        found.id, found.parameter, found.estimate.value, found.estimate.unit
                    );
                    println!("  uncertainty: {:?}", found.estimate.uncertainty);
                    println!("  value_kind: {:?}", found.provenance.value_kind);
                    println!("  source: {}", found.provenance.source);
                    if let Some(location) = &found.provenance.location {
                        println!("  location: {location}");
                    }
                    if let Some(sample) = found.provenance.sample_size {
                        println!("  sample_size: {sample}");
                    }
                    println!(
                        "  reviewed_by: {}",
                        found
                            .provenance
                            .reviewed_by
                            .as_deref()
                            .unwrap_or("(not reviewed)")
                    );
                    if let Some(compound) = &found.context.compound {
                        println!("  compound: {compound}");
                    }
                    if let Some(species) = &found.context.species {
                        println!("  species: {species}");
                    }
                    if let Some(tissue) = &found.context.tissue {
                        println!("  tissue: {tissue}");
                    }
                    if let Some(endpoint) = &found.context.endpoint {
                        println!("  endpoint: {endpoint}");
                    }
                    for limit in &found.applicability_limits {
                        println!("  limit: {limit}");
                    }
                }
                BioEvidenceCommand::ToModel {
                    library,
                    weights,
                    region_weights,
                    semantics,
                    compound,
                    species,
                    tissue,
                    endpoint,
                    assumptions,
                    id,
                    output,
                } => {
                    let lib: openbnct_bio::BioEvidenceLibrary =
                        serde_json::from_slice(&fs::read(&library)?)?;
                    lib.validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    let semantics = match semantics.as_str() {
                        "fixed_per_component" => openbnct_bio::WeightSemantics::FixedPerComponent,
                        "photon_isoeffective" => openbnct_bio::WeightSemantics::PhotonIsoeffective,
                        other => {
                            return Err(io::Error::other(format!(
                                "unsupported weight semantics {other:?}"
                            ))
                            .into());
                        }
                    };
                    let mut bindings = Vec::new();
                    for pair in &weights {
                        let (component, record_id) = pair.split_once('=').ok_or_else(|| {
                            io::Error::other(format!(
                                "--weight expects COMPONENT=RECORD_ID, got {pair:?}"
                            ))
                        })?;
                        let record = lib.record(record_id).ok_or_else(|| {
                            io::Error::other(format!("unknown record {record_id:?}"))
                        })?;
                        bindings.push((component, record));
                    }
                    let mut region_bindings = Vec::new();
                    for triple in &region_weights {
                        let (region_component, record_id) =
                            triple.split_once('=').ok_or_else(|| {
                                io::Error::other(format!(
                                    "--region-weight expects REGION:COMPONENT=RECORD_ID, got {triple:?}"
                                ))
                            })?;
                        let (region, component) =
                            region_component.split_once(':').ok_or_else(|| {
                                io::Error::other(format!(
                                    "--region-weight expects REGION:COMPONENT=RECORD_ID, got {triple:?}"
                                ))
                            })?;
                        let record = lib.record(record_id).ok_or_else(|| {
                            io::Error::other(format!("unknown record {record_id:?}"))
                        })?;
                        region_bindings.push((region.to_string(), component.to_string(), record));
                    }
                    let context = openbnct_bio::ContextQuery {
                        compound,
                        species,
                        tissue,
                        endpoint,
                        model_family: Some(openbnct_bio::BIOLOGICAL_MODEL_SCHEMA.to_string()),
                    };
                    let model = openbnct_bio::to_biological_model(
                        &id,
                        semantics,
                        openbnct_core::DoseUnit::GrayPerSourceParticle,
                        &bindings,
                        &context,
                        &assumptions,
                        &region_bindings,
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                    write_new_json(&output, &model)?;
                    println!("wrote biological model at {}", output.display());
                    for (component, record) in &bindings {
                        println!("  {component}: {}", record.id);
                    }
                }
            },
        },
        Some(Command::Dvh {
            dose,
            quantity,
            mask,
            bins,
            output,
        }) => {
            let histogram = compute_dvh_file(&dose, &quantity, &mask, bins, &output)?;
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
                let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(&dose)?;
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
                pint,
            } => {
                let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(&dose)?;
                let manifest =
                    openbnct_nifti::export_component_niftis(&bundle, &output_dir, gzip, pint)?;
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
        Some(Command::PromptGamma {
            dose,
            id,
            provenance_id,
            output,
        }) => {
            let bytes = fs::read(&dose)?;
            let bundle: PhysicalDoseBundle = openbnct_core::sidecar::from_slice_at(&bytes, &dose)
                .map_err(|error| {
                io::Error::other(format!("dose {}: {error}", dose.display()))
            })?;
            bundle
                .validate()
                .map_err(|error| io::Error::other(format!("dose {}: {error}", dose.display())))?;
            let parent = openbnct_core::ContentReference {
                id: bundle.provenance_id.clone(),
                sha256: format!("sha256:{}", openbnct_evidence::sha256_hex(&bytes)),
            };
            let source = openbnct_transport::derive_prompt_gamma_source(
                &bundle,
                &id,
                parent,
                provenance_id
                    .as_deref()
                    .unwrap_or(&format!("prompt-gamma:{}", bundle.provenance_id)),
            )
            .map_err(|error| io::Error::other(format!("prompt-gamma: {error}")))?;
            write_new_json(&output, &source)?;
            println!("prompt-gamma source at {}", output.display());
            println!(
                "emission {:.0} keV · branch {:.2} · {} voxels",
                source.emission_energy_ev / 1.0e3,
                source.branching_ratio,
                source.values.len()
            );
        }
        Some(Command::Pg { command }) => match command {
            PgCommand::Response {
                case,
                photon_data,
                detector,
                aperture,
                aperture_radius_mm,
                emission_energy_ev,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                no_transport_correction,
                pixellated,
                p1,
                anisotropy,
                id,
                provenance_id,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)
                    .map_err(|e| io::Error::other(format!("case {}: {e}", case.display())))?;
                let data_bytes = fs::read(&photon_data)?;
                let ph_data: openbnct_transport::MultigroupPhotonData =
                    serde_json::from_slice(&data_bytes).map_err(|e| {
                        io::Error::other(format!("photon data {}: {e}", photon_data.display()))
                    })?;
                ph_data
                    .validate()
                    .map_err(|e| io::Error::other(format!("photon data: {e}")))?;
                if detector.is_empty() {
                    return Err(io::Error::other("pg response: --detector required").into());
                }
                let edges = &ph_data.energy_boundaries_ev;
                let groups = edges.len() - 1;
                let emission_group = (0..groups)
                    .find(|&g| emission_energy_ev <= edges[g] && emission_energy_ev > edges[g + 1])
                    .ok_or_else(|| {
                        io::Error::other(format!(
                            "emission energy {emission_energy_ev} eV outside the photon group structure"
                        ))
                    })?;
                let [nx, ny, nz] = transport_case.geometry.shape;
                let n_cells = transport_case
                    .geometry
                    .voxel_count()
                    .map_err(|e| io::Error::other(format!("geometry: {e}")))?;
                let mut detector_voxels = Vec::with_capacity(detector.len());
                for spec in &detector {
                    let parts: Vec<&str> = spec.split(',').collect();
                    if parts.len() != 3 {
                        return Err(io::Error::other(format!(
                            "detector voxel {spec:?} must be i,j,k"
                        ))
                        .into());
                    }
                    let parse = |s: &str| {
                        s.trim().parse::<u32>().map_err(|_| {
                            io::Error::other(format!("detector voxel {spec:?} must be i,j,k"))
                        })
                    };
                    let v = [parse(parts[0])?, parse(parts[1])?, parse(parts[2])?];
                    let (i, j, k) = (v[0] as usize, v[1] as usize, v[2] as usize);
                    if i >= nx as usize || j >= ny as usize || k >= nz as usize {
                        return Err(io::Error::other(format!(
                            "detector voxel {i},{j},{k} outside the {nx}x{ny}x{nz} grid"
                        ))
                        .into());
                    }
                    detector_voxels.push(v);
                }
                // The regions to solve: pixellated → one region per
                // declared voxel; aggregate → the whole list is one
                // detector. Per-region solves share case, quadrature,
                // and aperture geometry.
                let regions: Vec<Vec<[u32; 3]>> = if pixellated {
                    detector_voxels.iter().map(|&v| vec![v]).collect()
                } else {
                    vec![detector_voxels.clone()]
                };
                // Parse the aperture once; the acceptance cone axis is
                // recomputed per region from that region's centroid.
                let aperture_spec: Option<([f64; 3], f64)> = match (&aperture, aperture_radius_mm) {
                    (None, None) => None,
                    (Some(_), None) => {
                        return Err(io::Error::other(
                            "pg response: --aperture requires --aperture-radius-mm",
                        )
                        .into());
                    }
                    (None, Some(_)) => unreachable!("clap requires aperture with the radius"),
                    (Some(spec), Some(radius)) => {
                        let parts: Vec<&str> = spec.split(',').collect();
                        let parse = |s: &str| {
                            s.trim().parse::<f64>().map_err(|_| {
                                io::Error::other(format!("aperture {spec:?} must be x,y,z mm"))
                            })
                        };
                        if parts.len() != 3 {
                            return Err(io::Error::other(format!(
                                "aperture {spec:?} must be x,y,z mm"
                            ))
                            .into());
                        }
                        let aperture_mm = [parse(parts[0])?, parse(parts[1])?, parse(parts[2])?];
                        if !(radius > 0.0 && radius.is_finite())
                            || aperture_mm.iter().any(|v| !v.is_finite())
                        {
                            return Err(io::Error::other(
                                "pg response: aperture position must be finite and radius > 0",
                            )
                            .into());
                        }
                        Some((aperture_mm, radius))
                    }
                };
                let quadrature = aperture_spec
                    .map(|_| {
                        openbnct_transport::level_symmetric_quadrature(order)
                            .map_err(|e| io::Error::other(format!("quadrature: {e}")))
                    })
                    .transpose()?;
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
                    progress: false,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: None,
                    periodic: periodic_axes,
                    beam_uncollided_split: false,
                    transport_correction: !no_transport_correction,
                    p1_anisotropic: p1,
                    anisotropy_order: anisotropy,
                    anderson_depth: 0,
                    coarse_rebalance: true,
                    inner_convergence: None,
                    theta_repair: true,
                    exp_source: true,
                    source_weighting: openbnct_transport::SourceWeighting::CollapseConsistent,
                };
                let data_ref = openbnct_core::ContentReference {
                    id: ph_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let case_ref = openbnct_core::ContentReference {
                    id: transport_case.case_id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&case_bytes),
                };
                let geometry = &transport_case.geometry;
                // One adjoint solve per region: `(sensitivity,
                // residual, outer_iterations, collimation)` per pixel.
                let mut columns = Vec::with_capacity(regions.len());
                for region in &regions {
                    let mut adjoint_source = vec![vec![0.0; groups]; n_cells];
                    for v in region {
                        adjoint_source[v[0] as usize
                            + nx as usize * v[1] as usize
                            + nx as usize * ny as usize * v[2] as usize][emission_group] = 1.0;
                    }
                    // Collimation: restrict the adjoint source to
                    // ordinates inside the cone the aperture subtends
                    // at this region's centroid — adjoint directions
                    // run detector→aperture, tracing back the photons
                    // a real pinhole accepts.
                    let (source_weights, collimation) = match (aperture_spec, &quadrature) {
                        (None, _) => (None, None),
                        (Some((aperture_mm, radius)), Some(quadrature)) => {
                            let mut centroid = [0.0_f64; 3];
                            for v in region {
                                for a in 0..3 {
                                    centroid[a] += geometry.origin_mm[a]
                                        + (v[a] as f64 + 0.5) * geometry.spacing_mm[a];
                                }
                            }
                            for c in &mut centroid {
                                *c /= region.len() as f64;
                            }
                            let axis: Vec<f64> =
                                (0..3).map(|a| aperture_mm[a] - centroid[a]).collect();
                            let dist = axis.iter().map(|x| x * x).sum::<f64>().sqrt();
                            if dist <= 0.0 {
                                return Err(io::Error::other(
                                    "pg response: aperture must not coincide with the detector",
                                )
                                .into());
                            }
                            let axis: Vec<f64> = axis.iter().map(|x| x / dist).collect();
                            let cos_min = dist / (dist * dist + radius * radius).sqrt();
                            let weights: Vec<f64> = quadrature
                                .iter()
                                .map(|(u, _)| {
                                    let dot: f64 = (0..3).map(|a| u[a] * axis[a]).sum();
                                    if dot >= cos_min { 1.0 } else { 0.0 }
                                })
                                .collect();
                            let accepted = weights.iter().filter(|w| **w > 0.0).count();
                            if accepted == 0 {
                                return Err(io::Error::other(format!(
                                    "pg response: no S{order} ordinate falls inside the \
                                     {radius} mm aperture cone at {dist:.1} mm — raise --order \
                                     or the radius"
                                ))
                                .into());
                            }
                            (
                                Some(weights),
                                Some(openbnct_transport::PgCollimation {
                                    aperture_mm,
                                    aperture_radius_mm: radius,
                                    accepted_ordinate_fraction: accepted as f64
                                        / quadrature.len() as f64,
                                }),
                            )
                        }
                        (Some(_), None) => unreachable!("quadrature built when aperture set"),
                    };
                    let adjoint = openbnct_transport::solve_photon_adjoint(
                        &transport_case,
                        &ph_data,
                        &options,
                        &adjoint_source,
                        source_weights.as_deref(),
                        data_ref.clone(),
                        case_ref.clone(),
                    )
                    .map_err(|e| io::Error::other(format!("photon adjoint solve: {e}")))?;
                    if !adjoint.converged {
                        return Err(io::Error::other(format!(
                            "photon adjoint did not converge (residual {:.3e} after {} outer iterations)",
                            adjoint.residual, adjoint.outer_iterations
                        ))
                        .into());
                    }
                    let sensitivity: Vec<f64> =
                        adjoint.flux.iter().map(|row| row[emission_group]).collect();
                    columns.push((
                        sensitivity,
                        adjoint.residual,
                        adjoint.outer_iterations,
                        collimation,
                    ));
                }
                let provenance = provenance_id.unwrap_or_else(|| format!("pg-response:{id}"));
                if pixellated {
                    let array = openbnct_transport::PgResponseArray {
                        schema_version: openbnct_transport::PG_RESPONSE_ARRAY_SCHEMA.into(),
                        id: id.clone(),
                        case_id: transport_case.case_id.clone(),
                        geometry: geometry.clone(),
                        emission_group: emission_group as u32,
                        emission_energy_ev,
                        quadrature_order: order,
                        pixels: detector_voxels
                            .iter()
                            .zip(columns)
                            .map(
                                |(
                                    &voxel,
                                    (sensitivity, residual, outer_iterations, collimation),
                                )| {
                                    openbnct_transport::PgPixelResponse {
                                        detector_voxel: voxel,
                                        sensitivity,
                                        converged: true,
                                        residual,
                                        outer_iterations,
                                        collimation,
                                    }
                                },
                            )
                            .collect(),
                        case: case_ref,
                        photon_data: data_ref,
                        provenance_id: provenance,
                        qualification: openbnct_transport::PROMPT_GAMMA_QUALIFICATION.into(),
                    };
                    array
                        .validate()
                        .map_err(|e| io::Error::other(format!("pg response: {e}")))?;
                    let worst = array
                        .pixels
                        .iter()
                        .map(|p| p.residual)
                        .fold(0.0_f64, f64::max);
                    write_new_json(&output, &array)?;
                    println!("pg response array at {}", output.display());
                    println!(
                        "emission group {} ({:.0} keV) · {} pixels · S{} · worst residual {:.2e}",
                        array.emission_group,
                        array.emission_energy_ev / 1.0e3,
                        array.pixels.len(),
                        array.quadrature_order,
                        worst
                    );
                } else {
                    let (sensitivity, residual, outer_iterations, collimation) =
                        columns.into_iter().next().expect("one region");
                    let response = openbnct_transport::PromptGammaResponse {
                        schema_version: openbnct_transport::PROMPT_GAMMA_RESPONSE_SCHEMA.into(),
                        id: id.clone(),
                        case_id: transport_case.case_id.clone(),
                        geometry: geometry.clone(),
                        detector_voxels,
                        emission_group: emission_group as u32,
                        emission_energy_ev,
                        sensitivity,
                        quadrature_order: order,
                        converged: true,
                        residual,
                        outer_iterations,
                        collimation,
                        case: case_ref,
                        photon_data: data_ref,
                        provenance_id: provenance,
                        qualification: openbnct_transport::PROMPT_GAMMA_QUALIFICATION.into(),
                    };
                    response
                        .validate()
                        .map_err(|e| io::Error::other(format!("pg response: {e}")))?;
                    write_new_json(&output, &response)?;
                    println!("pg response at {}", output.display());
                    println!(
                        "emission group {} ({:.0} keV) · {} detector voxels · S{} · residual {:.2e}",
                        response.emission_group,
                        response.emission_energy_ev / 1.0e3,
                        response.detector_voxels.len(),
                        response.quadrature_order,
                        response.residual
                    );
                }
            }
            PgCommand::Counts {
                emission,
                response,
                density_kg_per_m3,
                efficiency,
                id,
                provenance_id,
                output,
            } => {
                let emission_bytes = fs::read(&emission)?;
                let emission_source: openbnct_transport::PromptGammaSource =
                    serde_json::from_slice(&emission_bytes).map_err(|e| {
                        io::Error::other(format!("emission {}: {e}", emission.display()))
                    })?;
                let response_bytes = fs::read(&response)?;
                let emission_ref = openbnct_core::ContentReference {
                    id: emission_source.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&emission_bytes),
                };
                let provenance = provenance_id.unwrap_or_else(|| format!("pg-counts:{id}"));
                // The response may be a single-column artifact or a
                // pixellated bank — dispatch on the declared schema.
                let schema_probe: serde_json::Value = serde_json::from_slice(&response_bytes)
                    .map_err(|e| {
                        io::Error::other(format!("response {}: {e}", response.display()))
                    })?;
                if schema_probe["schema_version"].as_str()
                    == Some(openbnct_transport::PG_RESPONSE_ARRAY_SCHEMA)
                {
                    let array: openbnct_transport::PgResponseArray =
                        serde_json::from_slice(&response_bytes).map_err(|e| {
                            io::Error::other(format!("response {}: {e}", response.display()))
                        })?;
                    let response_ref = openbnct_core::ContentReference {
                        id: array.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&response_bytes),
                    };
                    let counts = openbnct_transport::expected_prompt_gamma_counts_pixels(
                        &emission_source,
                        &array,
                        density_kg_per_m3,
                        efficiency,
                        &id,
                        emission_ref,
                        response_ref,
                        &provenance,
                    )
                    .map_err(|e| io::Error::other(format!("pg counts: {e}")))?;
                    write_new_json(&output, &counts)?;
                    println!("pg counts at {}", output.display());
                    println!(
                        "{} pixels · tally range [{:.4e}, {:.4e}] (efficiency {:.3} · density {:.0} kg/m³)",
                        counts.per_pixel_tally.len(),
                        counts
                            .per_pixel_tally
                            .iter()
                            .cloned()
                            .fold(f64::INFINITY, f64::min),
                        counts
                            .per_pixel_tally
                            .iter()
                            .cloned()
                            .fold(f64::NEG_INFINITY, f64::max),
                        counts.detector_efficiency,
                        counts.voxel_density_kg_per_m3
                    );
                } else {
                    let response_map: openbnct_transport::PromptGammaResponse =
                        serde_json::from_slice(&response_bytes).map_err(|e| {
                            io::Error::other(format!("response {}: {e}", response.display()))
                        })?;
                    let response_ref = openbnct_core::ContentReference {
                        id: response_map.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&response_bytes),
                    };
                    let counts = openbnct_transport::expected_prompt_gamma_counts(
                        &emission_source,
                        &response_map,
                        density_kg_per_m3,
                        efficiency,
                        &id,
                        emission_ref,
                        response_ref,
                        &provenance,
                    )
                    .map_err(|e| io::Error::other(format!("pg counts: {e}")))?;
                    write_new_json(&output, &counts)?;
                    println!("pg counts at {}", output.display());
                    println!(
                        "expected tally {:.4e} (efficiency {:.3} · density {:.0} kg/m³)",
                        counts.expected_tally,
                        counts.detector_efficiency,
                        counts.voxel_density_kg_per_m3
                    );
                }
            }
            PgCommand::Observe {
                counts,
                id,
                provenance_id,
                output,
            } => {
                if counts.is_empty() {
                    return Err(io::Error::other("pg observe: --counts required").into());
                }
                let mut collected = Vec::with_capacity(counts.len());
                let mut observation: Option<openbnct_transport::PromptGammaObservation> = None;
                for path in &counts {
                    let bytes = fs::read(path)?;
                    let probe: serde_json::Value = serde_json::from_slice(&bytes)
                        .map_err(|e| io::Error::other(format!("counts {}: {e}", path.display())))?;
                    if probe["schema_version"].as_str()
                        == Some(openbnct_transport::PG_COUNTS_ARRAY_SCHEMA)
                    {
                        // A pixellated counts file becomes one detector
                        // entry per pixel, bound to the array's
                        // `{id}#pixel{i}` columns.
                        let model: openbnct_transport::PgPixelCounts =
                            serde_json::from_slice(&bytes).map_err(|e| {
                                io::Error::other(format!("counts {}: {e}", path.display()))
                            })?;
                        let pixel_obs =
                            openbnct_transport::collect_prompt_gamma_observation_pixels(
                                &model,
                                &id,
                                provenance_id
                                    .as_deref()
                                    .unwrap_or(&format!("pg-observation:{id}")),
                            )
                            .map_err(|e| io::Error::other(format!("pg observe: {e}")))?;
                        observation = match observation.take() {
                            None => Some(pixel_obs),
                            Some(mut prior) => {
                                prior.detectors.extend(pixel_obs.detectors);
                                Some(prior)
                            }
                        };
                    } else {
                        let model: openbnct_transport::PromptGammaCounts =
                            serde_json::from_slice(&bytes).map_err(|e| {
                                io::Error::other(format!("counts {}: {e}", path.display()))
                            })?;
                        collected.push(model);
                    }
                }
                if !collected.is_empty() {
                    let aggregate = openbnct_transport::collect_prompt_gamma_observation(
                        &collected,
                        &id,
                        provenance_id
                            .as_deref()
                            .unwrap_or(&format!("pg-observation:{id}")),
                    )
                    .map_err(|e| io::Error::other(format!("pg observe: {e}")))?;
                    observation = match observation.take() {
                        None => Some(aggregate),
                        Some(mut prior) => {
                            prior.detectors.extend(aggregate.detectors);
                            Some(prior)
                        }
                    };
                }
                let observation = observation
                    .ok_or_else(|| io::Error::other("pg observe: no counts files parsed"))?;
                observation
                    .validate()
                    .map_err(|e| io::Error::other(format!("pg observe: {e}")))?;
                write_new_json(&output, &observation)?;
                println!("pg observation at {}", output.display());
                println!("{} detector readings", observation.detectors.len());
            }
            PgCommand::Reconstruct {
                observation,
                response,
                density_kg_per_m3,
                lambda,
                max_iterations,
                unit,
                id,
                provenance_id,
                output,
            } => {
                let observation_bytes = fs::read(&observation)?;
                let observation_model: openbnct_transport::PromptGammaObservation =
                    serde_json::from_slice(&observation_bytes).map_err(|e| {
                        io::Error::other(format!("observation {}: {e}", observation.display()))
                    })?;
                observation_model
                    .validate()
                    .map_err(|e| io::Error::other(format!("pg reconstruct: {e}")))?;
                if response.len() != observation_model.detectors.len() {
                    return Err(io::Error::other(format!(
                        "pg reconstruct: {} --response files for {} observation detectors",
                        response.len(),
                        observation_model.detectors.len()
                    ))
                    .into());
                }
                let mut responses = Vec::with_capacity(response.len());
                for (path, detector) in response.iter().zip(observation_model.detectors.iter()) {
                    let bytes = fs::read(path)?;
                    let sha = openbnct_evidence::sha256_hex(&bytes);
                    let bound = &detector.response;
                    let bound_sha_ok =
                        bound.sha256 == sha || bound.sha256 == format!("sha256:{sha}");
                    let probe: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
                        io::Error::other(format!("response {}: {e}", path.display()))
                    })?;
                    let model = if probe["schema_version"].as_str()
                        == Some(openbnct_transport::PG_RESPONSE_ARRAY_SCHEMA)
                    {
                        // A response bank: the binding `id` names one
                        // column, `{array.id}#pixel{i}`.
                        let array: openbnct_transport::PgResponseArray =
                            serde_json::from_slice(&bytes).map_err(|e| {
                                io::Error::other(format!("response {}: {e}", path.display()))
                            })?;
                        let pixel_index = bound
                            .id
                            .strip_prefix(&format!("{}#pixel", array.id))
                            .and_then(|s| s.parse::<usize>().ok())
                            .ok_or_else(|| {
                                io::Error::other(format!(
                                    "response {} is a pixel array but the binding {:?} \
                                     does not name a {{id}}#pixel{{i}} column",
                                    path.display(),
                                    bound.id
                                ))
                            })?;
                        let Some(model) = array.pixel_as_response(pixel_index) else {
                            return Err(io::Error::other(format!(
                                "response {} has no pixel {}",
                                path.display(),
                                pixel_index
                            ))
                            .into());
                        };
                        model
                    } else {
                        serde_json::from_slice(&bytes).map_err(|e| {
                            io::Error::other(format!("response {}: {e}", path.display()))
                        })?
                    };
                    if model.id != bound.id || !bound_sha_ok {
                        return Err(io::Error::other(format!(
                            "response {} does not match the observation binding {}",
                            path.display(),
                            bound.id
                        ))
                        .into());
                    }
                    responses.push(model);
                }
                let observation_ref = openbnct_core::ContentReference {
                    id: observation_model.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&observation_bytes),
                };
                let emission_unit = match unit.as_str() {
                    "photons-per-kg-per-source-particle" => {
                        openbnct_transport::PromptGammaUnit::PhotonsPerKgPerSourceParticle
                    }
                    "photons-per-kg" => openbnct_transport::PromptGammaUnit::PhotonsPerKg,
                    other => {
                        return Err(io::Error::other(format!(
                            "unit {other:?} must be photons-per-kg-per-source-particle or photons-per-kg"
                        ))
                        .into())
                    }
                };
                let reconstruction = openbnct_transport::reconstruct_prompt_gamma_emission(
                    &observation_model,
                    &responses,
                    density_kg_per_m3,
                    lambda,
                    max_iterations,
                    emission_unit,
                    &id,
                    observation_ref,
                    provenance_id
                        .as_deref()
                        .unwrap_or(&format!("pg-reconstruction:{id}")),
                )
                .map_err(|e| io::Error::other(format!("pg reconstruct: {e}")))?;
                reconstruction
                    .validate()
                    .map_err(|e| io::Error::other(format!("pg reconstruct: {e}")))?;
                write_new_json(&output, &reconstruction)?;
                println!("pg reconstruction at {}", output.display());
                println!(
                    "residual {:.3e} after {} iterations{}",
                    reconstruction.regularization.residual_norm,
                    reconstruction.regularization.iterations,
                    if reconstruction.regularization.converged {
                        " (converged)"
                    } else {
                        " (budget exhausted)"
                    }
                );
            }
        },
        Some(Command::Accumulate { plan, output }) => {
            let accumulated = openbnct_plan::accumulate_plan_file(&plan)
                .map_err(|error| io::Error::other(format!("accumulation: {error}")))?;
            let json = openbnct_core::sidecar::to_vec_pretty_for(&accumulated, &output, false)?;
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
            ImportCommand::OpenPint {
                workbook,
                case_id,
                unit,
                normalization,
                producer_version,
                frame_of_reference_uid,
                out,
            } => {
                let bytes = fs::read(&workbook)?;
                let workbook_sha256 = openbnct_evidence::sha256_hex(&bytes);
                let plan = openbnct_plan::openpint::parse_openpint_workbook(&bytes)
                    .map_err(|error| io::Error::other(format!("openpint workbook: {error}")))?;
                let base = match workbook.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent,
                    _ => Path::new("."),
                };
                // Files the import reads must stay inside the workbook's
                // directory; paths it only records keep their declared form.
                let resolve = |path: &std::path::Path| confined_workbook_path(base, path);
                let declared = |path: &std::path::Path| {
                    if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        base.join(path)
                    }
                };

                let mut sources = Vec::new();
                for (key, rel) in &plan.bnct_components {
                    let component = match key.as_str() {
                        "B10" => openbnct_core::DoseComponent::Boron,
                        "N14" => openbnct_core::DoseComponent::Nitrogen,
                        "n" => openbnct_core::DoseComponent::Hydrogen,
                        "g" => openbnct_core::DoseComponent::Photon,
                        other => {
                            return Err(io::Error::other(format!(
                                "openpint bnct sheet: unknown component key {other:?} \
                                 (expected B10, N14, n, g)"
                            ))
                            .into());
                        }
                    };
                    sources.push(openbnct_nifti::NiftiComponentSource {
                        component,
                        file: resolve(rel)?,
                        sigma_file: None,
                    });
                }
                let document = openbnct_nifti::interchange_from_niftis(
                    &sources,
                    &case_id,
                    parse_dose_unit(&unit)?,
                    &normalization,
                    "openpint",
                    producer_version.clone(),
                    frame_of_reference_uid,
                )
                .map_err(|error| io::Error::other(format!("nifti import: {error}")))?;
                let document_bytes = serde_json::to_vec_pretty(&document)?;
                let document_sha256 = openbnct_evidence::sha256_hex(&document_bytes);
                let bundle = openbnct_core::import_component_dose(&document, &document_sha256)
                    .map_err(|error| io::Error::other(format!("interchange import: {error}")))?;

                fs::create_dir_all(&out)?;
                let bundle_path = out.join("physical-dose-bundle.json");
                write_new_json(&bundle_path, &bundle)?;
                println!("imported dose bundle at {}", bundle_path.display());

                let mut structure_rows = Vec::new();
                for structure in &plan.structures {
                    let image = openbnct_nifti::read_nifti_file(&resolve(&structure.mask_path)?)
                        .map_err(|error| {
                            io::Error::other(format!(
                                "mask {}: {error}",
                                structure.mask_path.display()
                            ))
                        })?;
                    let mask = if openbnct_core::grid_geometry_equivalent(
                        &image.geometry,
                        &bundle.geometry,
                    ) {
                        openbnct_nifti::to_mask(&image, &structure.name)
                    } else {
                        let values = openbnct_nifti::resample_to_grid(
                            &image,
                            &bundle.geometry,
                            openbnct_nifti::Interpolation::Nearest,
                        );
                        RegionMask {
                            name: structure.name.clone(),
                            voxels: values.iter().map(|v| *v != 0.0).collect(),
                        }
                    };
                    let included = mask.included_voxel_count();
                    if included == 0 {
                        return Err(io::Error::other(format!(
                            "structure {} rasterizes to an empty mask",
                            structure.name
                        ))
                        .into());
                    }
                    let mask_path = out.join(format!("{}.mask.json", structure.name));
                    write_new_json(&mask_path, &mask)?;
                    println!(
                        "  mask {} → {} ({} voxels, boron {})",
                        structure.name,
                        mask_path.display(),
                        included,
                        structure.boron_conc
                    );
                    structure_rows.push(serde_json::json!({
                        "name": structure.name,
                        "roi_type": structure.roi_type.sheet_name(),
                        "mask_artifact": mask_path.display().to_string(),
                        "mask_sha256": openbnct_evidence::sha256_file(&mask_path)
                            .unwrap_or_default(),
                        "source_mask_path": structure.mask_path.display().to_string(),
                        "boron_conc": structure.boron_conc,
                        "max_dose": structure.max_dose,
                        "mean_dose": structure.mean_dose,
                    }));
                }

                let summary = serde_json::json!({
                    "schema_version": "openbnct.openpint-plan-summary/0.1.0",
                    "id": format!("openpint-import-{}", &workbook_sha256[..16]),
                    "case_id": case_id,
                    "workbook_sha256": workbook_sha256,
                    "ct_path": plan.ct_path.as_ref().map(|p| declared(p).display().to_string()),
                    "dose_bundle": bundle_path.display().to_string(),
                    "dose_bundle_sha256": bundle.provenance_id,
                    "structures": structure_rows,
                    "hadron_courses": plan.hadron_courses.iter().map(|c| serde_json::json!({
                        "name": c.name,
                        "dose_path": declared(&c.dose_path).display().to_string(),
                        "fractions": c.fractions,
                    })).collect::<Vec<_>>(),
                    "qualification": "research_only_not_clinical",
                });
                let summary_path = out.join("openpint-plan-summary.json");
                write_new_json(&summary_path, &summary)?;
                println!("plan summary at {}", summary_path.display());
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
            ImportCommand::Labelmap {
                nifti,
                materials,
                case,
                case_output,
                case_id,
                output,
            } => {
                cmd_import_labelmap(nifti, materials, case, case_output, case_id, output)?;
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
            ExportCommand::Meshtal { dose, output } => {
                let bundle: openbnct_core::PhysicalDoseBundle =
                    openbnct_core::sidecar::load_json(&dose).map_err(|error| {
                        io::Error::other(format!("dose {}: {error}", dose.display()))
                    })?;
                let text = openbnct_mcnp::pint_meshtal(&bundle)
                    .map_err(|error| io::Error::other(format!("meshtal export: {error}")))?;
                write_new_text(&output, text.as_bytes())?;
                println!(
                    "wrote OpenPINT-convention meshtal (tallies 14/24/34/44) at {}",
                    output.display()
                );
            }
            ExportCommand::Phits {
                case,
                assignment,
                maxbch,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let case_doc: TransportCase =
                    serde_json::from_slice(&case_bytes).map_err(|error| {
                        io::Error::other(format!("case {}: {error}", case.display()))
                    })?;
                let assignment_doc = assignment
                    .map(|path| {
                        let bytes = fs::read(&path)?;
                        serde_json::from_slice::<MaterialAssignment>(&bytes).map_err(|error| {
                            io::Error::other(format!("assignment {}: {error}", path.display()))
                        })
                    })
                    .transpose()?;
                let deck = openbnct_phits::deck::export_phits_deck(
                    &case_doc,
                    assignment_doc.as_ref(),
                    &openbnct_phits::deck::PhitsDeckOptions {
                        case_sha256: format!(
                            "sha256:{}",
                            openbnct_evidence::sha256_hex(&case_bytes)
                        ),
                        maxbch,
                    },
                )
                .map_err(|error| io::Error::other(format!("phits export: {error}")))?;
                write_new_text(&output, deck.as_bytes())?;
                println!("wrote PHITS deck at {}", output.display());
            }
        },
        Some(Command::Compare {
            reference,
            candidate,
            sigma_level,
            output,
        }) => {
            let reference_bytes = fs::read(&reference)?;
            let reference_bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&reference_bytes, &reference)?;
            let candidate_bytes = fs::read(&candidate)?;
            let candidate_bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&candidate_bytes, &candidate)?;
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
            let reference_bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&reference_bytes, &reference)?;
            let candidate_bytes = fs::read(&candidate)?;
            let candidate_bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&candidate_bytes, &candidate)?;
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
            let reference_bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&reference_bytes, &reference)?;
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
                candidates.push(openbnct_core::sidecar::from_slice_at::<PhysicalDoseBundle>(
                    &bytes, path,
                )?);
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
            let bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
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
                inner_convergence,
                max_inner,
                max_outer,
                periodic,
                no_uncollided_split,
                no_transport_correction,
                p1,
                p0,
                exp_source,
                anisotropy,
                anderson,
                no_cmr,
                source_weighting,
                allow_unconverged,
                dose,
                boron_unit_output,
                quiet,
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
                let use_p1 = p1 || (!p0 && data_supports_p1(&mg_data));
                let options = openbnct_transport::SnOptions {
                    progress: !quiet,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model.clone(),
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: !no_transport_correction,
                    p1_anisotropic: use_p1,
                    anisotropy_order: anisotropy,
                    anderson_depth: anderson,
                    coarse_rebalance: !no_cmr,
                    inner_convergence,
                    theta_repair: true,
                    exp_source: exp_source || !use_p1,
                    source_weighting: match source_weighting.as_str() {
                        "collapse_consistent" => {
                            openbnct_transport::SourceWeighting::CollapseConsistent
                        }
                        "uniform_in_bin" => openbnct_transport::SourceWeighting::UniformInBin,
                        other => {
                            return Err(io::Error::other(format!(
                                "--source-weighting {other:?} must be \
                                 collapse_consistent or uniform_in_bin"
                            ))
                            .into());
                        }
                    },
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
                if !flux.converged && !allow_unconverged {
                    return Err(io::Error::other(format!(
                        "multigroup solve did not converge (residual {:.3e} after {} outer iterations);                          --allow-unconverged writes the provisional field",
                        flux.residual, flux.outer_iterations
                    ))
                    .into());
                }
                if !flux.converged {
                    eprintln!(
                        "warning: solve unconverged (residual {:.3e});                          emitting provisional field with converged=false",
                        flux.residual
                    );
                    if let Some(site) = flux.residual_site {
                        eprintln!(
                            "  limiting site: cell {} (group {}) changed {:.3e}",
                            site.cell, site.group, site.relative_change
                        );
                    }
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
                if let Some(unit_path) = boron_unit_output {
                    write_boron_unit_dose(
                        &unit_path,
                        &transport_case,
                        &mg_data,
                        &flux,
                        assignment_model.as_ref(),
                        &data_bytes,
                        &fs::read(&output)?,
                    )?;
                }
            }
            SnCommand::Fold {
                case,
                data,
                flux,
                assignment,
                boron_unit_output,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes)?;
                let flux_bytes = fs::read(&flux)?;
                let flux_model: openbnct_transport::MultigroupFlux =
                    openbnct_core::sidecar::from_slice_at(&flux_bytes, &flux)?;
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
                if let Some(unit_path) = boron_unit_output {
                    write_boron_unit_dose(
                        &unit_path,
                        &transport_case,
                        &mg_data,
                        &flux_model,
                        assignment_model.as_ref(),
                        &data_bytes,
                        &flux_bytes,
                    )?;
                }
            }
            SnCommand::Collapse {
                library,
                endf,
                tsl,
                tsl_temperature,
                self_shielding,
                material,
                boundaries,
                boundaries_file,
                id,
                component_profile,
                weighting_spectrum,
                attenuation_depth,
                note,
                output,
            } => {
                let boundaries = match boundaries_file {
                    Some(path) => {
                        let doc: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)
                            .map_err(|error| {
                                io::Error::other(format!("boundaries {}: {error}", path.display()))
                            })?;
                        doc.get("energy_boundaries_ev")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().filter_map(|v| v.as_f64()).collect::<Vec<f64>>())
                            .filter(|b| b.len() >= 2)
                            .ok_or_else(|| {
                                io::Error::other(format!(
                                    "boundaries {}: no `energy_boundaries_ev` list",
                                    path.display()
                                ))
                            })?
                    }
                    None => boundaries,
                };
                let mut endf_paths = std::collections::BTreeMap::new();
                for spec in &endf {
                    let (name, path) = spec.split_once('=').ok_or_else(|| {
                        io::Error::other(format!("--endf expects NUCLIDE=PATH, got {spec:?}"))
                    })?;
                    endf_paths.insert(name.to_string(), PathBuf::from(path));
                }
                let mut tsl_paths = std::collections::BTreeMap::new();
                for spec in &tsl {
                    let (name, path) = spec.split_once('=').ok_or_else(|| {
                        io::Error::other(format!("--tsl expects NUCLIDE=PATH, got {spec:?}"))
                    })?;
                    tsl_paths.insert(name.to_string(), PathBuf::from(path));
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
                let weighting = match &weighting_spectrum {
                    Some(path) => {
                        let spec: openbnct_transport::EnergyDistribution =
                            serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                                io::Error::other(format!(
                                    "weighting spectrum {}: {error}",
                                    path.display()
                                ))
                            })?;
                        match spec {
                            openbnct_transport::EnergyDistribution::TabulatedHistogram {
                                energy_boundaries_ev,
                                bin_weights,
                            } => openbnct_openmc::WeightingSpectrum::Tabulated {
                                energy_boundaries_ev,
                                bin_weights,
                            },
                            openbnct_transport::EnergyDistribution::Monoenergetic { .. } => {
                                return Err(io::Error::other(
                                    "--weighting-spectrum requires a tabulated histogram",
                                )
                                .into());
                            }
                        }
                    }
                    None => openbnct_openmc::WeightingSpectrum::ThermalMaxwellianEpithermalFlat {
                        cut_ev: 0.5,
                    },
                };
                let options = openbnct_openmc::CollapseOptions {
                    library_dir: library.clone(),
                    endf_paths,
                    tsl_paths,
                    tsl_temperature_k: tsl_temperature,
                    self_shielding,
                    materials: material_models,
                    energy_boundaries_ev: boundaries,
                    weighting,
                    attenuation_depth_cm: attenuation_depth,
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
            SnCommand::Spectrum {
                flux,
                mask,
                material,
                case,
                data,
                assignment,
                output,
            } => {
                let flux_doc: openbnct_transport::MultigroupFlux =
                    openbnct_core::sidecar::load_json(&flux).map_err(|error| {
                        io::Error::other(format!("flux {}: {error}", flux.display()))
                    })?;
                let groups = flux_doc.energy_boundaries_ev.len().saturating_sub(1);
                if groups == 0 || flux_doc.flux.iter().any(|row| row.len() != groups) {
                    return Err(
                        io::Error::other("flux artifact group structure is inconsistent").into(),
                    );
                }
                let n_cells = flux_doc.flux.len();
                // Cell selection: mask, material id via assignment, or
                // all cells.
                let selector: Vec<bool> = if let Some(path) = &mask {
                    let m: RegionMask =
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("mask {}: {error}", path.display()))
                        })?;
                    if m.voxels.len() != n_cells {
                        return Err(io::Error::other(format!(
                            "mask has {} voxels, flux has {n_cells} cells",
                            m.voxels.len()
                        ))
                        .into());
                    }
                    m.voxels
                } else if let Some(name) = &material {
                    let (case, data, assignment) = match (&case, &data, &assignment) {
                        (Some(c), Some(d), Some(a)) => (c, d, a),
                        _ => {
                            return Err(io::Error::other(
                                "--material requires --case, --data and --assignment",
                            )
                            .into());
                        }
                    };
                    let case_document: TransportCase = serde_json::from_slice(&fs::read(case)?)
                        .map_err(|error| {
                            io::Error::other(format!("case {}: {error}", case.display()))
                        })?;
                    let mg_data: openbnct_transport::MultigroupData =
                        serde_json::from_slice(&fs::read(data)?).map_err(|error| {
                            io::Error::other(format!("data {}: {error}", data.display()))
                        })?;
                    let assignment_doc: MaterialAssignment =
                        serde_json::from_slice(&fs::read(assignment)?).map_err(|error| {
                            io::Error::other(format!(
                                "assignment {}: {error}",
                                assignment.display()
                            ))
                        })?;
                    let cell_mat = openbnct_transport::cell_materials(
                        &case_document,
                        &mg_data,
                        Some(&assignment_doc),
                    )
                    .map_err(|error| io::Error::other(format!("materials: {error}")))?;
                    let wanted: Vec<usize> = mg_data
                        .materials
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| m.material_id == *name)
                        .map(|(i, _)| i)
                        .collect();
                    if wanted.is_empty() {
                        return Err(io::Error::other(format!(
                            "no material {name:?} in the multigroup data"
                        ))
                        .into());
                    }
                    cell_mat.iter().map(|&m| wanted.contains(&m)).collect()
                } else {
                    vec![true; n_cells]
                };
                let mut acc = vec![0.0; groups];
                let mut selected = 0usize;
                for (cell, row) in flux_doc.flux.iter().enumerate() {
                    if selector[cell] {
                        selected += 1;
                        for (g, v) in row.iter().enumerate() {
                            acc[g] += v;
                        }
                    }
                }
                if selected == 0 {
                    return Err(io::Error::other("selection contains no cells").into());
                }
                // Per-group integrated fluence (uniform cell volume cancels
                // in the weighting ratios); `w(E)` then returns the
                // per-eV density φ_g.
                let bin_weights: Vec<f64> = acc
                    .iter()
                    .enumerate()
                    .map(|(g, v)| {
                        let w = (flux_doc.energy_boundaries_ev[g]
                            - flux_doc.energy_boundaries_ev[g + 1])
                            .abs();
                        v * w / selected as f64
                    })
                    .collect();
                let spec = openbnct_transport::EnergyDistribution::TabulatedHistogram {
                    energy_boundaries_ev: flux_doc.energy_boundaries_ev.clone(),
                    bin_weights,
                };
                let json = serde_json::to_string_pretty(&spec)?;
                match &output {
                    Some(path) => {
                        fs::write(path, &json)?;
                        println!("spectrum ({} cells) at {}", selected, path.display());
                    }
                    None => println!("{json}"),
                }
            }
            SnCommand::Boundaries {
                spectrum,
                response,
                groups,
                uniform_floor,
                output,
            } => {
                let spectrum_bytes = fs::read(&spectrum)?;
                let spec: openbnct_transport::EnergyDistribution =
                    serde_json::from_slice(&spectrum_bytes).map_err(|error| {
                        io::Error::other(format!("spectrum {}: {error}", spectrum.display()))
                    })?;
                let mut proposal = match &response {
                    Some(path) => {
                        let bytes = fs::read(path)?;
                        let resp: openbnct_transport::EnergyDistribution =
                            serde_json::from_slice(&bytes).map_err(|error| {
                                io::Error::other(format!("response {}: {error}", path.display()))
                            })?;
                        let (edges, mut weights) = match &resp {
                            openbnct_transport::EnergyDistribution::TabulatedHistogram {
                                energy_boundaries_ev,
                                bin_weights,
                            } => (energy_boundaries_ev.clone(), bin_weights.clone()),
                            _ => {
                                return Err(io::Error::other(
                                    "--response must be a tabulated histogram",
                                )
                                .into());
                            }
                        };
                        let spec_edges = match &spec {
                            openbnct_transport::EnergyDistribution::TabulatedHistogram {
                                energy_boundaries_ev,
                                ..
                            } => energy_boundaries_ev.clone(),
                            _ => {
                                return Err(io::Error::other(
                                    "boundaries: spectrum must be a tabulated histogram",
                                )
                                .into());
                            }
                        };
                        // Compare on a common ascending orientation;
                        // reverse the response weights when its edges
                        // run descending.
                        let mut resp_asc = edges.clone();
                        let mut resp_w = std::mem::take(&mut weights);
                        if resp_asc.len() >= 2 && resp_asc[0] > *resp_asc.last().unwrap() {
                            resp_asc.reverse();
                            resp_w.reverse();
                        }
                        let mut spec_asc = spec_edges;
                        if spec_asc.len() >= 2 && spec_asc[0] > *spec_asc.last().unwrap() {
                            spec_asc.reverse();
                        }
                        if resp_asc != spec_asc {
                            return Err(io::Error::other(
                                "--response binning must match --spectrum",
                            )
                            .into());
                        }
                        let mut p = openbnct_transport::adapt_boundaries(
                            &spec,
                            groups,
                            Some(&resp_w),
                            uniform_floor,
                        )
                        .map_err(|error| io::Error::other(format!("boundaries: {error}")))?;
                        p.response_sha256 = Some(openbnct_evidence::sha256_hex(&bytes));
                        p
                    }
                    None => {
                        openbnct_transport::adapt_boundaries(&spec, groups, None, uniform_floor)
                            .map_err(|error| io::Error::other(format!("boundaries: {error}")))?
                    }
                };
                proposal.spectrum_sha256 = Some(openbnct_evidence::sha256_hex(&spectrum_bytes));
                write_new_json(&output, &proposal)?;
                println!(
                    "{}-group boundary proposal at {}",
                    proposal.energy_boundaries_ev.len() - 1,
                    output.display()
                );
            }
            SnCommand::PhotonCollapse {
                photon_library,
                neutron_library,
                material,
                photon_boundaries,
                neutron_data,
                id,
                component_profile,
                note,
                output,
            } => {
                let mut material_models = Vec::with_capacity(material.len());
                for path in &material {
                    material_models.push(serde_json::from_slice::<
                        openbnct_transport::MaterialDefinition,
                    >(&fs::read(path)?)?);
                }
                let neutron_mg: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&fs::read(&neutron_data)?)?;
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
                let options = openbnct_openmc::PhotonCollapseOptions {
                    photon_library_dir: photon_library.clone(),
                    neutron_library_dir: neutron_library.clone(),
                    materials: material_models,
                    photon_boundaries_ev: photon_boundaries,
                    neutron_boundaries_ev: neutron_mg.energy_boundaries_ev.clone(),
                    weighting:
                        openbnct_openmc::WeightingSpectrum::ThermalMaxwellianEpithermalFlat {
                            cut_ev: 0.5,
                        },
                    id: id.clone(),
                    component_profile: profile_ref,
                    note: note.unwrap_or_default(),
                };
                let data = openbnct_openmc::collapse_photon(&options)
                    .map_err(|error| io::Error::other(format!("photon collapse: {error}")))?;
                let json = serde_json::to_string_pretty(&data)?;
                match &output {
                    Some(path) => {
                        write_new_json(path, &data)?;
                        println!(
                            "multigroup photon data at {} ({} photon groups, {} neutron groups, {} materials)",
                            path.display(),
                            data.energy_boundaries_ev.len() - 1,
                            data.neutron_energy_boundaries_ev.len() - 1,
                            data.materials.len()
                        );
                    }
                    None => println!("{json}"),
                }
            }
            SnCommand::PhotonSolve {
                case,
                photon_data,
                neutron_flux,
                assignment,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                no_transport_correction,
                p1,
                anisotropy,
                dose,
                output,
            } => {
                let case_bytes = fs::read(&case)?;
                let transport_case: TransportCase = serde_json::from_slice(&case_bytes)?;
                let data_bytes = fs::read(&photon_data)?;
                let ph_data: openbnct_transport::MultigroupPhotonData =
                    serde_json::from_slice(&data_bytes)?;
                let neutron_flux_model: openbnct_transport::MultigroupFlux =
                    openbnct_core::sidecar::load_json(&neutron_flux)?;
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
                    progress: false,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model.clone(),
                    periodic: periodic_axes,
                    beam_uncollided_split: false,
                    transport_correction: !no_transport_correction,
                    p1_anisotropic: p1,
                    anisotropy_order: anisotropy,
                    anderson_depth: 0,
                    coarse_rebalance: true,
                    inner_convergence: None,
                    theta_repair: true,
                    exp_source: true,
                    source_weighting: openbnct_transport::SourceWeighting::CollapseConsistent,
                };
                let data_ref = openbnct_core::ContentReference {
                    id: ph_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let case_ref = openbnct_core::ContentReference {
                    id: transport_case.case_id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&case_bytes),
                };
                let flux = openbnct_transport::solve_photon(
                    &transport_case,
                    &ph_data,
                    &neutron_flux_model,
                    &options,
                    data_ref,
                    case_ref,
                )
                .map_err(|error| io::Error::other(format!("photon solve: {error}")))?;
                if !flux.converged {
                    return Err(io::Error::other(format!(
                        "photon solve did not converge (residual {:.3e} after {} outer iterations)",
                        flux.residual, flux.outer_iterations
                    ))
                    .into());
                }
                write_new_json(&output, &flux)?;
                println!(
                    "photon flux at {} (S{}, {} groups, {} outer iterations, residual {:.2e})",
                    output.display(),
                    flux.quadrature_order,
                    flux.energy_boundaries_ev.len().saturating_sub(1),
                    flux.outer_iterations,
                    flux.residual
                );
                if let Some(dose_path) = dose {
                    let profile = ph_data.component_profile.clone().ok_or_else(|| {
                        io::Error::other(
                            "--dose requires the photon data to declare component_profile",
                        )
                    })?;
                    let response_ref = openbnct_core::ContentReference {
                        id: ph_data.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&data_bytes),
                    };
                    let bundle = openbnct_transport::fold_photon_dose(
                        &transport_case,
                        &ph_data,
                        &flux,
                        assignment_model.as_ref(),
                        profile,
                        response_ref,
                    )
                    .map_err(|error| io::Error::other(format!("photon dose fold: {error}")))?;
                    write_new_json(&dose_path, &bundle)?;
                    println!("folded photon dose bundle at {}", dose_path.display());
                }
            }
            SnCommand::MergePhotonDose {
                neutron_dose,
                photon_dose,
                output,
            } => {
                let neutron_bytes = fs::read(&neutron_dose)?;
                let photon_bytes = fs::read(&photon_dose)?;
                let mut bundle: PhysicalDoseBundle =
                    openbnct_core::sidecar::from_slice_at(&neutron_bytes, &neutron_dose)?;
                let photon: PhysicalDoseBundle =
                    openbnct_core::sidecar::from_slice_at(&photon_bytes, &photon_dose)?;
                bundle
                    .validate()
                    .map_err(|error| io::Error::other(format!("neutron dose bundle: {error}")))?;
                if bundle.case_id != photon.case_id || bundle.geometry != photon.geometry {
                    return Err(io::Error::other(
                        "neutron and photon dose bundles differ in case_id or grid geometry",
                    )
                    .into());
                }
                let transported = photon
                    .components
                    .iter()
                    .find(|c| c.component == openbnct_core::DoseComponent::Photon)
                    .ok_or_else(|| {
                        io::Error::other("photon dose bundle has no photon component")
                    })?;
                let slot = bundle
                    .components
                    .iter_mut()
                    .find(|c| c.component == openbnct_core::DoseComponent::Photon)
                    .ok_or_else(|| {
                        io::Error::other("neutron dose bundle has no photon component")
                    })?;
                if slot.unit != transported.unit {
                    return Err(io::Error::other("photon dose unit mismatch").into());
                }
                for ((total, old), new) in bundle
                    .physical_total
                    .values
                    .iter_mut()
                    .zip(&slot.values)
                    .zip(&transported.values)
                {
                    *total = (*total - old + new).max(0.0);
                }
                slot.values = transported.values.clone();
                slot.absolute_standard_uncertainty =
                    transported.absolute_standard_uncertainty.clone();
                bundle.physical_total.absolute_standard_uncertainty = None;
                bundle.physical_total.uncertainty_method =
                    openbnct_core::TotalUncertaintyMethod::Unavailable;
                bundle.provenance_id = format!(
                    "{}+transported-photon[{};local capture-gamma kerma of {} replaced]",
                    bundle.provenance_id,
                    photon.provenance_id,
                    neutron_dose.display()
                );
                write_new_json(&output, &bundle)?;
                println!(
                    "physical dose bundle with transported photon component at {} \
                     (neutron bundle sha256 {}, photon bundle sha256 {})",
                    output.display(),
                    openbnct_evidence::sha256_hex(&neutron_bytes),
                    openbnct_evidence::sha256_hex(&photon_bytes)
                );
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
            PlanCommand::Optimize {
                objective,
                dose,
                mask,
                initial,
                id,
                provenance_id,
                output,
                emit_plan,
                seconds_per_weight,
                scenario_set,
                fraction_scales,
                solver,
            } => {
                use openbnct_plan::lp::{
                    LpMode, optimize_weights_lp, optimize_weights_scenarios_lp,
                };
                use openbnct_plan::optimize::{
                    BeamDoseField, DoseQuantity, InversePlanObjective, ResultProvenance,
                    optimize_weights,
                };
                let (lp_mode, newton) = match solver.as_str() {
                    "qp" => (Some(LpMode::Penalty), false),
                    "lp" => (Some(LpMode::Strict), false),
                    "newton" => (None, true),
                    _ => (None, false),
                };
                let spec_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&spec_bytes).map_err(|error| {
                        io::Error::other(format!("inverse-plan objective: {error}"))
                    })?;
                let scenario_bytes = scenario_set.as_ref().map(fs::read).transpose()?;
                let scenarios_doc: Option<openbnct_plan::scenarios::PlanScenarioSet> =
                    scenario_bytes
                        .as_ref()
                        .map(|bytes| {
                            serde_json::from_slice(bytes)
                                .map_err(|error| io::Error::other(format!("scenario set: {error}")))
                        })
                        .transpose()?;
                let fractions_doc: Option<openbnct_plan::fractions::FractionScales> =
                    fraction_scales
                        .as_ref()
                        .map(|p| {
                            serde_json::from_slice(&fs::read(p)?).map_err(|error| {
                                io::Error::other(format!("fraction scales: {error}"))
                            })
                        })
                        .transpose()?;
                // Scenario perturbation and per-fraction component
                // scaling both need the full component map whatever
                // the dose quantity.
                let need_components = scenarios_doc.is_some() || fractions_doc.is_some();
                let mut fields = Vec::with_capacity(dose.len());
                let mut geometry: Option<openbnct_core::GridGeometry> = None;
                for path in &dose {
                    let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(path)?;
                    if scenarios_doc.is_some() {
                        match &geometry {
                            Some(g) if *g != bundle.geometry => {
                                return Err(io::Error::other(format!(
                                    "{}: scenario evaluation requires a shared beam grid",
                                    path.display()
                                ))
                                .into());
                            }
                            None => geometry = Some(bundle.geometry.clone()),
                            _ => {}
                        }
                    }
                    // Scenario mode needs every component map for
                    // perturbation, whatever the dose quantity.
                    let all_components = || {
                        let name = |c: openbnct_core::DoseComponent| match c {
                            openbnct_core::DoseComponent::Boron => "boron",
                            openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                            openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                            openbnct_core::DoseComponent::Photon => "photon",
                        };
                        let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                            .components
                            .iter()
                            .map(|volume| {
                                (name(volume.component).to_string(), volume.values.clone())
                            })
                            .collect();
                        components
                    };
                    let (values, components) = match spec.dose_quantity {
                        DoseQuantity::PhysicalTotal => (
                            bundle.physical_total.values.clone(),
                            need_components.then(&all_components),
                        ),
                        DoseQuantity::Component(component) => (
                            bundle
                                .components
                                .iter()
                                .find(|volume| volume.component == component)
                                .ok_or_else(|| {
                                    io::Error::other(format!(
                                        "{}: no {:?} component",
                                        path.display(),
                                        component
                                    ))
                                })?
                                .values
                                .clone(),
                            need_components.then(&all_components),
                        ),
                        DoseQuantity::Isoeffective => {
                            (bundle.physical_total.values.clone(), Some(all_components()))
                        }
                    };
                    fields.push(BeamDoseField {
                        name: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        values,
                        components,
                    });
                }
                let masks: Vec<RegionMask> = mask
                    .iter()
                    .map(|path| {
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("{}: {error}", path.display())).into()
                        })
                    })
                    .collect::<Result<_, Box<dyn Error>>>()?;
                let n_beams = fields.len();
                if let Some(doc) = &fractions_doc {
                    fields = openbnct_plan::fractions::expand_fractions(&fields, &spec, doc)
                        .map_err(|e| io::Error::other(format!("fraction scales: {e}")))?;
                }
                let weights0 = if initial.is_empty() {
                    vec![1.0; fields.len()]
                } else if fractions_doc.is_some() && initial.len() == n_beams {
                    // Per-beam initials replicate across fractions.
                    let f = fields.len() / n_beams.max(1);
                    initial
                        .iter()
                        .flat_map(|&w| std::iter::repeat_n(w, f))
                        .collect()
                } else {
                    initial
                };
                let spec_sha = openbnct_evidence::sha256_hex(&spec_bytes);
                let provenance = ResultProvenance {
                    id,
                    provenance_id: provenance_id
                        .unwrap_or_else(|| format!("inverse-plan-run:{}", &spec_sha[..12])),
                    objective: openbnct_core::ContentReference {
                        id: spec.id.clone(),
                        sha256: format!("sha256:{spec_sha}"),
                    },
                };
                if newton && scenarios_doc.is_some() {
                    return Err(io::Error::other(
                        "--solver newton is incompatible with --scenario-set (use pgd/lp/qp)",
                    )
                    .into());
                }
                let result = if newton {
                    openbnct_plan::optimize::optimize_weights_newton(
                        &fields, &masks, &spec, &weights0, provenance,
                    )
                    .map_err(|error| io::Error::other(format!("optimize: {error}")))?
                } else {
                    match (&scenarios_doc, lp_mode) {
                        (Some(set), None) => openbnct_plan::scenarios::optimize_weights_scenarios(
                            &fields,
                            &masks,
                            &spec,
                            set,
                            geometry
                                .as_ref()
                                .expect("scenario sets require at least one dose bundle"),
                            &weights0,
                            provenance,
                        )
                        .map_err(|error| io::Error::other(format!("optimize: {error}")))?,
                        (Some(set), Some(mode)) => optimize_weights_scenarios_lp(
                            &fields,
                            &masks,
                            &spec,
                            set,
                            geometry
                                .as_ref()
                                .expect("scenario sets require at least one dose bundle"),
                            mode,
                            provenance,
                        )
                        .map_err(|error| io::Error::other(format!("optimize: {error}")))?,
                        (None, None) => {
                            optimize_weights(&fields, &masks, &spec, &weights0, provenance)
                                .map_err(|error| io::Error::other(format!("optimize: {error}")))?
                        }
                        (None, Some(mode)) => {
                            optimize_weights_lp(&fields, &masks, &spec, mode, provenance)
                                .map_err(|error| io::Error::other(format!("optimize: {error}")))?
                        }
                    }
                };
                fs::write(&output, serde_json::to_vec_pretty(&result)?)?;
                println!(
                    "optimize: {} beams, penalty {:.6e}, {} iterations, converged={}",
                    result.weights.len(),
                    result.penalty,
                    result.iterations,
                    result.converged
                );
                for w in &result.weights {
                    println!("  {}: weight {:.6e}", w.name, w.weight);
                }
                for o in &result.outcomes {
                    println!(
                        "  {} {}: achieved {:.6e} vs bound {:.6e} ({})",
                        o.kind,
                        o.mask,
                        o.achieved,
                        o.bound,
                        if o.satisfied { "satisfied" } else { "VIOLATED" }
                    );
                }
                if let Some(metrics) = &result.metrics {
                    for region in metrics {
                        println!(
                            "  metrics {}: n={}, mean {:.6e}, range {:.3e}..{:.3e}",
                            region.mask, region.voxel_count, region.mean, region.min, region.max
                        );
                        for q in &region.dose_at_volume {
                            println!("    D({:.3}) = {:.6e}", q.volume_fraction, q.dose);
                        }
                        for v in &region.volume_at_dose {
                            println!("    V({:.6e}) = {:.2}%", v.dose, 100.0 * v.volume_fraction);
                        }
                        for e in &region.eud {
                            println!("    EUD(a={:.3}) = {:.6e}", e.a, e.value);
                        }
                        for ep in &region.endpoints {
                            println!(
                                "    {:?} ({}): {:.4}",
                                ep.endpoint, ep.model_id, ep.probability
                            );
                        }
                    }
                }
                println!("result: {}", output.display());
                if let Some(plan_path) = emit_plan {
                    let exposures: Vec<openbnct_core::Exposure> = result
                        .weights
                        .iter()
                        .zip(&dose)
                        .map(|(w, path)| {
                            let bytes = fs::read(path).map_err(|error| {
                                io::Error::other(format!(
                                    "{}: re-read for plan hash: {error}",
                                    path.display()
                                ))
                            })?;
                            Ok(openbnct_core::Exposure {
                                name: w.name.clone(),
                                dose_bundle: openbnct_core::BoundFileReference {
                                    id: format!("{}.dose-bundle", w.name),
                                    sha256: openbnct_evidence::sha256_hex(&bytes),
                                    path: path.display().to_string(),
                                },
                                weight: w.weight,
                                weight_basis: openbnct_core::WeightBasis::SourceStrengthScaling,
                                duration_s: seconds_per_weight.map(|s| s * w.weight),
                                boron_assumption: None,
                            })
                        })
                        .collect::<Result<_, io::Error>>()?;
                    let plan = openbnct_core::ExposurePlan {
                        schema_version: openbnct_core::EXPOSURE_PLAN_SCHEMA.into(),
                        id: format!("{}.exposure-plan", result.id),
                        case_id: spec.case_id.clone(),
                        covariance: openbnct_core::ExposureCovariance::IndependentExposures,
                        exposures,
                    };
                    let issues = plan.validate_diagnostics();
                    if !issues.is_empty() {
                        return Err(io::Error::other(format!(
                            "emitted exposure plan fails validation: {issues:?}"
                        ))
                        .into());
                    }
                    write_new_json(&plan_path, &plan)?;
                    println!("exposure plan: {}", plan_path.display());
                }
            }
            PlanCommand::Select {
                objective,
                dose,
                mask,
                beams,
                search,
                fraction_scales,
                solver,
                output,
                emit_plan,
                id,
                provenance_id,
            } => {
                use openbnct_plan::optimize::{
                    BeamDoseField, DoseQuantity, InversePlanObjective, ResultProvenance,
                };
                let spec_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&spec_bytes).map_err(|error| {
                        io::Error::other(format!("inverse-plan objective: {error}"))
                    })?;
                // Same per-beam bundle loading as `plan optimize`
                // (no scenario perturbation — selection runs on the
                // nominal fields; `--scenario-set` stays on optimize).
                let mut fields = Vec::with_capacity(dose.len());
                let mut dose_refs = Vec::with_capacity(dose.len());
                for path in &dose {
                    let bytes = fs::read(path)?;
                    let bundle: PhysicalDoseBundle =
                        openbnct_core::sidecar::from_slice_at(&bytes, path)?;
                    let all_components = || {
                        let name = |c: openbnct_core::DoseComponent| match c {
                            openbnct_core::DoseComponent::Boron => "boron",
                            openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                            openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                            openbnct_core::DoseComponent::Photon => "photon",
                        };
                        bundle
                            .components
                            .iter()
                            .map(|volume| {
                                (name(volume.component).to_string(), volume.values.clone())
                            })
                            .collect()
                    };
                    let (values, components) = match spec.dose_quantity {
                        DoseQuantity::PhysicalTotal => (bundle.physical_total.values.clone(), None),
                        DoseQuantity::Component(component) => (
                            bundle
                                .components
                                .iter()
                                .find(|volume| volume.component == component)
                                .ok_or_else(|| {
                                    io::Error::other(format!(
                                        "{}: no {:?} component",
                                        path.display(),
                                        component
                                    ))
                                })?
                                .values
                                .clone(),
                            None,
                        ),
                        DoseQuantity::Isoeffective => {
                            (bundle.physical_total.values.clone(), Some(all_components()))
                        }
                    };
                    fields.push(BeamDoseField {
                        name: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        values,
                        components,
                    });
                    dose_refs.push(openbnct_core::ContentReference {
                        id: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        sha256: format!("sha256:{}", openbnct_evidence::sha256_hex(&bytes)),
                    });
                }
                let masks: Vec<RegionMask> = mask
                    .iter()
                    .map(|path| {
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("{}: {error}", path.display())).into()
                        })
                    })
                    .collect::<Result<_, Box<dyn Error>>>()?;
                if let Some(path) = &fraction_scales {
                    let doc: openbnct_plan::fractions::FractionScales =
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("{}: {error}", path.display()))
                        })?;
                    fields = openbnct_plan::fractions::expand_fractions(&fields, &spec, &doc)
                        .map_err(|e| io::Error::other(format!("fraction scales: {e}")))?;
                }
                let mode = match solver.as_str() {
                    "qp" => openbnct_plan::lp::LpMode::Penalty,
                    "lp" => openbnct_plan::lp::LpMode::Strict,
                    other => {
                        return Err(io::Error::other(format!(
                            "--solver must be qp|lp, got {other}"
                        ))
                        .into());
                    }
                };
                let search_mode = match search.as_str() {
                    "exhaustive" => openbnct_plan::selection::SelectionSearch::Exhaustive,
                    "greedy" => openbnct_plan::selection::SelectionSearch::Greedy,
                    other => {
                        return Err(io::Error::other(format!(
                            "--search must be exhaustive|greedy, got {other}"
                        ))
                        .into());
                    }
                };
                let spec_sha = openbnct_evidence::sha256_hex(&spec_bytes);
                let provenance = ResultProvenance {
                    id,
                    provenance_id: provenance_id
                        .unwrap_or_else(|| format!("beam-selection:{}", &spec_sha[..12])),
                    objective: openbnct_core::ContentReference {
                        id: spec.id.clone(),
                        sha256: format!("sha256:{spec_sha}"),
                    },
                };
                let report = openbnct_plan::selection::select_beams(
                    &fields,
                    &masks,
                    &spec,
                    beams,
                    mode,
                    search_mode,
                    dose_refs,
                    provenance,
                )
                .map_err(|e| io::Error::other(format!("beam selection: {e}")))?;
                write_new_json(&output, &report)?;
                println!("beam selection: {}", output.display());
                println!(
                    "selected {:?} ({} evaluated, score {:.4e})",
                    report.selected.beams, report.evaluated, report.selected.score
                );
                if let Some(plan_path) = emit_plan {
                    // Re-emit the winning subset as a full plan result —
                    // the report row carries weights but not the
                    // outcome/iteration bookkeeping.
                    let idx: Vec<usize> = fields
                        .iter()
                        .enumerate()
                        .filter(|(_, f)| report.selected.beams.contains(&f.name))
                        .map(|(i, _)| i)
                        .collect();
                    let subset: Vec<BeamDoseField> =
                        idx.iter().map(|&i| fields[i].clone()).collect();
                    let result = openbnct_plan::lp::optimize_weights_lp(
                        &subset,
                        &masks,
                        &spec,
                        mode,
                        ResultProvenance {
                            id: format!("{}.selected", report.id),
                            provenance_id: report.provenance_id.clone(),
                            objective: report.objective.clone(),
                        },
                    )
                    .map_err(|e| io::Error::other(format!("selected plan: {e}")))?;
                    write_new_json(&plan_path, &result)?;
                    println!("selected plan: {}", plan_path.display());
                }
            }
            PlanCommand::Robustness {
                result,
                objective,
                dose,
                mask,
                relative,
                positioning_sigma_mm,
                boron_field,
                id,
                output,
            } => {
                use openbnct_plan::optimize::{InversePlanObjective, InversePlanResult};
                use openbnct_plan::robustness::{
                    BeamSigmaField, PLAN_ROBUSTNESS_SCHEMA, PlanRobustnessReport, plan_robustness,
                };
                let result_bytes = fs::read(&result)?;
                let plan_result: InversePlanResult = serde_json::from_slice(&result_bytes)
                    .map_err(|error| io::Error::other(format!("inverse-plan result: {error}")))?;
                let spec_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&spec_bytes).map_err(|error| {
                        io::Error::other(format!("inverse-plan objective: {error}"))
                    })?;
                // The objective the caller supplies must be the one the
                // result recorded — otherwise the σ reports describe a
                // different optimization.
                let spec_sha = openbnct_evidence::sha256_hex(&spec_bytes);
                if plan_result.objective.sha256 != format!("sha256:{spec_sha}") {
                    return Err(io::Error::other(format!(
                        "objective hash mismatch: result binds {}, supplied file hashes {}",
                        plan_result.objective.sha256, spec_sha
                    ))
                    .into());
                }

                // Component name helper — the same tokens the dose
                // bundles use.
                let comp_name = |c: openbnct_core::DoseComponent| match c {
                    openbnct_core::DoseComponent::Boron => "boron",
                    openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                    openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                    openbnct_core::DoseComponent::Photon => "photon",
                };
                let parse_component = |name: &str| match name {
                    "boron" => Some(openbnct_core::DoseComponent::Boron),
                    "nitrogen" => Some(openbnct_core::DoseComponent::Nitrogen),
                    "hydrogen" => Some(openbnct_core::DoseComponent::Hydrogen),
                    "photon" => Some(openbnct_core::DoseComponent::Photon),
                    _ => None,
                };

                // Declared sources → (component, σ-spec) pairs evaluated
                // per beam below.
                let mut rel_specs: Vec<(openbnct_core::DoseComponent, f64)> = Vec::new();
                let mut sources: Vec<openbnct_core::UncertaintySource> = Vec::new();
                for spec_text in &relative {
                    let (name, sigma_text) = spec_text.split_once('=').ok_or_else(|| {
                        io::Error::other(format!(
                            "--relative {spec_text:?} must be written as component=sigma"
                        ))
                    })?;
                    let component = parse_component(name).ok_or_else(|| {
                        io::Error::other(format!("--relative: unknown component {name:?}"))
                    })?;
                    let sigma: f64 = sigma_text.parse().map_err(|_| {
                        io::Error::other(format!("--relative {spec_text:?}: sigma is not a number"))
                    })?;
                    if !sigma.is_finite() || sigma < 0.0 {
                        return Err(io::Error::other(format!(
                            "--relative {spec_text:?}: sigma must be a non-negative finite value"
                        ))
                        .into());
                    }
                    rel_specs.push((component, sigma));
                    sources.push(openbnct_core::UncertaintySource::RelativeComponent {
                        component: name.to_string(),
                        relative_1sigma: sigma,
                    });
                }
                if let Some(sigma_mm) = positioning_sigma_mm {
                    if !sigma_mm.is_finite() || sigma_mm < 0.0 {
                        return Err(io::Error::other(
                            "--positioning-sigma-mm must be a non-negative finite value",
                        )
                        .into());
                    }
                    sources.push(openbnct_core::UncertaintySource::Positioning {
                        sigma_mm,
                        registration: None,
                    });
                }
                let field = boron_field
                    .map(|path| -> Result<_, Box<dyn Error>> {
                        let field: openbnct_boron::BoronField =
                            serde_json::from_slice(&fs::read(&path)?)?;
                        field
                            .validate()
                            .map_err(|e| io::Error::other(e.to_string()))?;
                        Ok((path, field))
                    })
                    .transpose()?;
                if let Some((path, _)) = &field {
                    sources.push(openbnct_core::UncertaintySource::BoronConcentration {
                        field: openbnct_core::ContentReference {
                            id: "boron-field".into(),
                            sha256: openbnct_evidence::sha256_file(path)?,
                        },
                    });
                }
                if sources.is_empty() {
                    return Err(io::Error::other(
                        "no systematic sources declared (--relative, --positioning-sigma-mm, --boron-field)",
                    )
                    .into());
                }

                // Per-beam fields + σ maps, in --dose order — the same
                // order the optimizer consumed and the result records.
                let mut fields = Vec::with_capacity(dose.len());
                let mut sigmas = Vec::with_capacity(dose.len());
                let mut dose_references = Vec::with_capacity(dose.len());
                for path in &dose {
                    let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(path)?;
                    bundle
                        .validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    let component_values = |c: openbnct_core::DoseComponent| {
                        bundle
                            .components
                            .iter()
                            .find(|v| v.component == c)
                            .map(|v| v.values.as_slice())
                    };
                    let mut sigma_components: std::collections::BTreeMap<String, Vec<f64>> =
                        std::collections::BTreeMap::new();
                    for component in [
                        openbnct_core::DoseComponent::Boron,
                        openbnct_core::DoseComponent::Nitrogen,
                        openbnct_core::DoseComponent::Hydrogen,
                        openbnct_core::DoseComponent::Photon,
                    ] {
                        let Some(dose_values) = component_values(component) else {
                            continue;
                        };
                        let mut maps: Vec<Vec<f64>> = Vec::new();
                        for (rel_component, sigma) in &rel_specs {
                            if *rel_component == component {
                                maps.push(openbnct_core::relative_component_sigma(
                                    dose_values,
                                    *sigma,
                                ));
                            }
                        }
                        if let Some(sigma_mm) = positioning_sigma_mm {
                            maps.push(openbnct_core::positioning_sigma(
                                dose_values,
                                &bundle.geometry,
                                sigma_mm,
                            ));
                        }
                        if component == openbnct_core::DoseComponent::Boron
                            && let Some((_, boron)) = &field
                        {
                            if boron.geometry != bundle.geometry {
                                return Err(io::Error::other(
                                    "boron field geometry does not match the dose bundle grid",
                                )
                                .into());
                            }
                            let (map, _) = openbnct_core::boron_field_sigma(
                                dose_values,
                                &boron.values,
                                &boron.uncertainty_1sigma,
                            );
                            maps.push(map);
                        }
                        if !maps.is_empty() {
                            sigma_components.insert(
                                comp_name(component).to_string(),
                                openbnct_core::combine_voxel_sigma(&maps),
                            );
                        }
                    }
                    let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                        .components
                        .iter()
                        .map(|volume| {
                            (
                                comp_name(volume.component).to_string(),
                                volume.values.clone(),
                            )
                        })
                        .collect();
                    fields.push(openbnct_plan::optimize::BeamDoseField {
                        name: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        values: bundle.physical_total.values.clone(),
                        components: Some(components),
                    });
                    sigmas.push(BeamSigmaField {
                        beam: path.display().to_string(),
                        components: sigma_components,
                    });
                    dose_references.push(openbnct_core::ContentReference {
                        id: format!("{}.dose-bundle", fields.last().expect("just pushed").name),
                        sha256: openbnct_evidence::sha256_file(path)?,
                    });
                }
                // Beam order must match the recorded weight order —
                // names come from file stems, exactly as `plan optimize`
                // assigned them.
                for (weight, field) in plan_result.weights.iter().zip(&fields) {
                    if weight.name != field.name {
                        return Err(io::Error::other(format!(
                            "beam order mismatch: result beam {:?} vs supplied field {:?} — pass --dose in optimize order",
                            weight.name, field.name
                        ))
                        .into());
                    }
                }

                let masks: Vec<RegionMask> = mask
                    .iter()
                    .map(|path| {
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("{}: {error}", path.display())).into()
                        })
                    })
                    .collect::<Result<_, Box<dyn Error>>>()?;
                let mut mask_voxels: std::collections::BTreeMap<String, Vec<usize>> =
                    std::collections::BTreeMap::new();
                for region in &masks {
                    let voxels: Vec<usize> = region
                        .voxels
                        .iter()
                        .enumerate()
                        .filter_map(|(v, &on)| on.then_some(v))
                        .collect();
                    mask_voxels.insert(region.name.clone(), voxels);
                }

                let outcomes = plan_robustness(&plan_result, &spec, &fields, &sigmas, &mask_voxels)
                    .map_err(|error| io::Error::other(format!("robustness: {error}")))?;
                let result_sha = openbnct_evidence::sha256_hex(&result_bytes);
                let report = PlanRobustnessReport {
                    schema_version: PLAN_ROBUSTNESS_SCHEMA.into(),
                    id,
                    result: openbnct_core::ContentReference {
                        id: plan_result.id.clone(),
                        sha256: format!("sha256:{result_sha}"),
                    },
                    objective: plan_result.objective.clone(),
                    dose_references,
                    sources,
                    objectives: outcomes,
                    method: "first_order_gaussian_fully_correlated".into(),
                    qualification: "inverse_plan_robustness_research_only_not_clinical".into(),
                    provenance_id: format!("plan-robustness:{}", &result_sha[..12]),
                };
                write_new_json(&output, &report)?;
                for o in &report.objectives {
                    println!(
                        "  {} {}: achieved {:.6e} ± {:.3e} vs bound {:.6e} — P(violate) = {:.4}",
                        o.kind,
                        o.mask,
                        o.achieved,
                        o.sigma_1sigma,
                        o.bound,
                        o.violation_probability
                    );
                }
                println!("robustness: {}", output.display());
            }
            PlanCommand::Scenarios {
                result,
                objective,
                scenario_set,
                dose,
                mask,
                id,
                output,
            } => {
                use openbnct_plan::optimize::{InversePlanObjective, InversePlanResult};
                use openbnct_plan::scenarios::{
                    PLAN_SCENARIO_REPORT_SCHEMA, PlanScenarioReport, PlanScenarioSet,
                    evaluate_scenarios,
                };
                let result_bytes = fs::read(&result)?;
                let plan_result: InversePlanResult = serde_json::from_slice(&result_bytes)
                    .map_err(|error| io::Error::other(format!("inverse-plan result: {error}")))?;
                let spec_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&spec_bytes).map_err(|error| {
                        io::Error::other(format!("inverse-plan objective: {error}"))
                    })?;
                let spec_sha = openbnct_evidence::sha256_hex(&spec_bytes);
                if plan_result.objective.sha256 != format!("sha256:{spec_sha}") {
                    return Err(io::Error::other(format!(
                        "objective hash mismatch: result binds {}, supplied file hashes {}",
                        plan_result.objective.sha256, spec_sha
                    ))
                    .into());
                }
                let scenario_bytes = fs::read(&scenario_set)?;
                let set: PlanScenarioSet = serde_json::from_slice(&scenario_bytes)
                    .map_err(|error| io::Error::other(format!("scenario set: {error}")))?;
                set.validate()
                    .map_err(|error| io::Error::other(format!("scenario set: {error}")))?;

                let comp_name = |c: openbnct_core::DoseComponent| match c {
                    openbnct_core::DoseComponent::Boron => "boron",
                    openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                    openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                    openbnct_core::DoseComponent::Photon => "photon",
                };
                let mut fields = Vec::with_capacity(dose.len());
                let mut dose_references = Vec::with_capacity(dose.len());
                let mut geometry: Option<openbnct_core::GridGeometry> = None;
                for path in &dose {
                    let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(path)?;
                    bundle
                        .validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    match &geometry {
                        Some(g) if *g != bundle.geometry => {
                            return Err(io::Error::other(format!(
                                "{}: beam fields must share one grid",
                                path.display()
                            ))
                            .into());
                        }
                        None => geometry = Some(bundle.geometry.clone()),
                        _ => {}
                    }
                    let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                        .components
                        .iter()
                        .map(|volume| {
                            (
                                comp_name(volume.component).to_string(),
                                volume.values.clone(),
                            )
                        })
                        .collect();
                    fields.push(openbnct_plan::optimize::BeamDoseField {
                        name: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        values: bundle.physical_total.values.clone(),
                        components: Some(components),
                    });
                    dose_references.push(openbnct_core::ContentReference {
                        id: format!("{}.dose-bundle", fields.last().expect("just pushed").name),
                        sha256: openbnct_evidence::sha256_file(path)?,
                    });
                }
                for (weight, field) in plan_result.weights.iter().zip(&fields) {
                    if weight.name != field.name {
                        return Err(io::Error::other(format!(
                            "beam order mismatch: result beam {:?} vs supplied field {:?} — pass --dose in optimize order",
                            weight.name, field.name
                        ))
                        .into());
                    }
                }
                let geometry = geometry
                    .ok_or_else(|| io::Error::other("--dose requires at least one beam field"))?;

                let mut mask_voxels: std::collections::BTreeMap<String, Vec<usize>> =
                    std::collections::BTreeMap::new();
                for path in &mask {
                    let region: RegionMask =
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("{}: {error}", path.display()))
                        })?;
                    let voxels: Vec<usize> = region
                        .voxels
                        .iter()
                        .enumerate()
                        .filter_map(|(v, &on)| on.then_some(v))
                        .collect();
                    mask_voxels.insert(region.name.clone(), voxels);
                }

                let (evaluations, bands) =
                    evaluate_scenarios(&plan_result, &spec, &fields, &geometry, &set, &mask_voxels)
                        .map_err(|error| io::Error::other(format!("scenarios: {error}")))?;
                let result_sha = openbnct_evidence::sha256_hex(&result_bytes);
                let report = PlanScenarioReport {
                    schema_version: PLAN_SCENARIO_REPORT_SCHEMA.into(),
                    id,
                    result: openbnct_core::ContentReference {
                        id: plan_result.id.clone(),
                        sha256: format!("sha256:{result_sha}"),
                    },
                    objective: plan_result.objective.clone(),
                    scenario_set: openbnct_core::ContentReference {
                        id: set.id.clone(),
                        sha256: format!(
                            "sha256:{}",
                            openbnct_evidence::sha256_hex(&scenario_bytes)
                        ),
                    },
                    dose_references,
                    evaluations,
                    bands,
                    qualification: "inverse_plan_scenarios_research_only_not_clinical".into(),
                    provenance_id: format!("plan-scenarios:{}", &result_sha[..12]),
                };
                write_new_json(&output, &report)?;
                for band in &report.bands {
                    println!(
                        "  {} {}: nominal {:.6e} band [{:.6e}, {:.6e}] worst {:?} bound {:.6e}{}",
                        band.kind,
                        band.mask,
                        band.nominal_achieved,
                        band.min_achieved,
                        band.max_achieved,
                        band.worst_scenario,
                        band.bound,
                        if band.violated_scenarios.is_empty() {
                            String::new()
                        } else {
                            format!(" — violated by {}", band.violated_scenarios.join(", "))
                        }
                    );
                }
                println!("scenarios: {}", output.display());
            }
            PlanCommand::Compare {
                result,
                objective,
                scenario_set,
                trained_on,
                dose,
                mask,
                id,
                output,
            } => {
                use openbnct_plan::optimize::{InversePlanObjective, InversePlanResult};
                use openbnct_plan::scenarios::{
                    HELDOUT_COMPARISON_SCHEMA, HeldOutComparisonReport, PlanScenarioSet,
                    compare_plans_heldout,
                };
                let spec_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&spec_bytes).map_err(|error| {
                        io::Error::other(format!("inverse-plan objective: {error}"))
                    })?;
                let spec_sha = openbnct_evidence::sha256_hex(&spec_bytes);
                let mut plan_results = Vec::with_capacity(result.len());
                for path in &result {
                    let plan_result: InversePlanResult = serde_json::from_slice(&fs::read(path)?)
                        .map_err(|error| {
                        io::Error::other(format!("{}: {error}", path.display()))
                    })?;
                    if plan_result.objective.sha256 != format!("sha256:{spec_sha}") {
                        return Err(io::Error::other(format!(
                            "{}: objective hash mismatch — result binds {}",
                            path.display(),
                            plan_result.objective.sha256
                        ))
                        .into());
                    }
                    plan_results.push(plan_result);
                }
                let scenario_bytes = fs::read(&scenario_set)?;
                let heldout: PlanScenarioSet = serde_json::from_slice(&scenario_bytes)
                    .map_err(|error| io::Error::other(format!("scenario set: {error}")))?;
                let trained: PlanScenarioSet = serde_json::from_slice(&fs::read(&trained_on)?)
                    .map_err(|error| {
                        io::Error::other(format!("trained-on scenario set: {error}"))
                    })?;
                let trained_names: Vec<String> =
                    trained.scenarios.iter().map(|s| s.name.clone()).collect();

                let comp_name = |c: openbnct_core::DoseComponent| match c {
                    openbnct_core::DoseComponent::Boron => "boron",
                    openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                    openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                    openbnct_core::DoseComponent::Photon => "photon",
                };
                let mut fields = Vec::with_capacity(dose.len());
                let mut geometry: Option<openbnct_core::GridGeometry> = None;
                for path in &dose {
                    let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(path)?;
                    bundle
                        .validate()
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    match &geometry {
                        Some(g) if *g != bundle.geometry => {
                            return Err(io::Error::other(format!(
                                "{}: beam fields must share one grid",
                                path.display()
                            ))
                            .into());
                        }
                        None => geometry = Some(bundle.geometry.clone()),
                        _ => {}
                    }
                    let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                        .components
                        .iter()
                        .map(|volume| {
                            (
                                comp_name(volume.component).to_string(),
                                volume.values.clone(),
                            )
                        })
                        .collect();
                    fields.push(openbnct_plan::optimize::BeamDoseField {
                        name: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string()),
                        values: bundle.physical_total.values.clone(),
                        components: Some(components),
                    });
                }
                let geometry = geometry
                    .ok_or_else(|| io::Error::other("--dose requires at least one beam field"))?;
                let mut mask_voxels: std::collections::BTreeMap<String, Vec<usize>> =
                    std::collections::BTreeMap::new();
                for path in &mask {
                    let region: RegionMask =
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("{}: {error}", path.display()))
                        })?;
                    let voxels: Vec<usize> = region
                        .voxels
                        .iter()
                        .enumerate()
                        .filter_map(|(v, &on)| on.then_some(v))
                        .collect();
                    mask_voxels.insert(region.name.clone(), voxels);
                }

                let plans = compare_plans_heldout(
                    &plan_results,
                    &spec,
                    &fields,
                    &geometry,
                    &heldout,
                    &trained_names,
                    &mask_voxels,
                )
                .map_err(|error| io::Error::other(format!("held-out comparison: {error}")))?;
                let report = HeldOutComparisonReport {
                    schema_version: HELDOUT_COMPARISON_SCHEMA.into(),
                    id,
                    scenario_set: openbnct_core::ContentReference {
                        id: heldout.id.clone(),
                        sha256: format!(
                            "sha256:{}",
                            openbnct_evidence::sha256_hex(&scenario_bytes)
                        ),
                    },
                    optimization_scenarios: trained_names,
                    plans,
                    qualification: "inverse_plan_heldout_research_only_not_clinical".into(),
                    provenance_id: format!(
                        "plan-compare:{}",
                        &openbnct_evidence::sha256_hex(&scenario_bytes)[..12]
                    ),
                };
                write_new_json(&output, &report)?;
                for plan in &report.plans {
                    println!(
                        "plan {} ({}){}:",
                        plan.plan,
                        plan.method.as_deref().unwrap_or("nominal"),
                        if plan.converged {
                            ""
                        } else {
                            " [NOT CONVERGED]"
                        }
                    );
                    for band in &plan.bands {
                        println!(
                            "  {} {}: nominal {:.6e} held-out band [{:.6e}, {:.6e}] worst {:?}{}",
                            band.kind,
                            band.mask,
                            band.nominal_achieved,
                            band.min_achieved,
                            band.max_achieved,
                            band.worst_scenario,
                            if band.violated_scenarios.is_empty() {
                                String::new()
                            } else {
                                format!(" — violated by {}", band.violated_scenarios.join(", "))
                            }
                        );
                    }
                }
                println!("held-out comparison: {}", output.display());
            }
            PlanCommand::Directions {
                case,
                aim_mask,
                body_mask,
                azimuth_steps,
                elevation_steps,
                top,
                data,
                radius_cm,
                csv,
                output,
                id,
                provenance_id,
            } => {
                let case_bytes = fs::read(&case)?;
                let case_document: TransportCase =
                    serde_json::from_slice(&case_bytes).map_err(|error| {
                        io::Error::other(format!("case {}: {error}", case.display()))
                    })?;
                let geometry = case_document.geometry.clone();
                let aim_bytes = fs::read(&aim_mask)?;
                let aim: RegionMask = serde_json::from_slice(&aim_bytes).map_err(|error| {
                    io::Error::other(format!("mask {}: {error}", aim_mask.display()))
                })?;
                let (body, body_bytes) = match body_mask {
                    Some(path) => {
                        let bytes = fs::read(&path)?;
                        let mask: RegionMask = serde_json::from_slice(&bytes).map_err(|error| {
                            io::Error::other(format!("mask {}: {error}", path.display()))
                        })?;
                        (Some(mask), Some(bytes))
                    }
                    None => (None, None),
                };
                let mut candidates = openbnct_plan::directions::enumerate_directions(
                    &geometry,
                    &aim,
                    body.as_ref(),
                    azimuth_steps,
                    elevation_steps,
                )
                .map_err(|error| io::Error::other(format!("directions: {error}")))?;
                // Adjoint scoring: one solve of the transposed problem
                // with the aim region as adjoint source, then rank every
                // candidate by its uncollided beam × φ* inner product.
                let mut data_bytes: Option<Vec<u8>> = None;
                let mut scoring = "tissue_path_length";
                if let Some(data_path) = data {
                    let bytes = fs::read(&data_path)?;
                    let data: openbnct_transport::MultigroupData = serde_json::from_slice(&bytes)
                        .map_err(|error| {
                        io::Error::other(format!("data {}: {error}", data_path.display()))
                    })?;
                    data_bytes = Some(bytes);
                    let options = openbnct_transport::SnOptions::default();
                    let n_cells = geometry
                        .voxel_count()
                        .map_err(|error| io::Error::other(format!("geometry: {error}")))?;
                    let groups = data.group_count();
                    let mut adjoint_source = vec![vec![0.0; groups]; n_cells];
                    for (cell, present) in aim.voxels.iter().enumerate() {
                        if *present && cell < n_cells {
                            for row in adjoint_source[cell].iter_mut() {
                                *row = 1.0;
                            }
                        }
                    }
                    let adjoint = openbnct_transport::solve_multigroup_adjoint(
                        &case_document,
                        &data,
                        &options,
                        &adjoint_source,
                        None,
                        openbnct_core::ContentReference {
                            id: "multigroup-data".into(),
                            sha256: format!(
                                "sha256:{}",
                                openbnct_evidence::sha256_hex(&fs::read(&data_path)?)
                            ),
                        },
                        openbnct_core::ContentReference {
                            id: "case".into(),
                            sha256: format!(
                                "sha256:{}",
                                openbnct_evidence::sha256_hex(&fs::read(&case)?)
                            ),
                        },
                    )
                    .map_err(|error| io::Error::other(format!("adjoint solve: {error}")))?;
                    let mut scored: Vec<(f64, usize)> = Vec::with_capacity(candidates.len());
                    for (index, candidate) in candidates.iter().enumerate() {
                        let score = match openbnct_transport::adjoint_direction_score(
                            &case_document,
                            &data,
                            &options,
                            &adjoint,
                            &aim,
                            candidate.direction_lps,
                            radius_cm,
                        ) {
                            Ok(score) => score,
                            // A direction whose aperture cannot sit wholly on
                            // its entry face is not realizable on this grid.
                            Err(openbnct_transport::MultigroupError::ApertureOutsideFace(
                                reason,
                            )) => {
                                eprintln!("skip {}: {reason}", candidate.name);
                                continue;
                            }
                            Err(error) => {
                                return Err(io::Error::other(format!(
                                    "score {}: {error}",
                                    candidate.name
                                ))
                                .into());
                            }
                        };
                        scored.push((score, index));
                    }
                    for (score, index) in &scored {
                        candidates[*index].adjoint_score = Some(*score);
                    }
                    if scored.is_empty() {
                        return Err(io::Error::other(
                            "no candidate direction admits an aperture that fits its entry face",
                        )
                        .into());
                    }
                    scored
                        .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    candidates = scored
                        .iter()
                        .map(|(_, index)| candidates[*index].clone())
                        .collect();
                    scoring = "adjoint_importance";
                    eprintln!("adjoint-ranked by uncollided-beam importance");
                }
                let take = if top == 0 { candidates.len() } else { top };
                for line in openbnct_plan::directions::beam_spec_lines(&candidates, take) {
                    println!("{line}");
                }
                if let Some(path) = csv {
                    let mut text = String::from(
                        "name,dx,dy,dz,azimuth_deg,elevation_deg,tissue_path_mm
",
                    );
                    for c in &candidates {
                        text.push_str(&format!(
                            "{},{:.6},{:.6},{:.6},{:.1},{:.1},{}
",
                            c.name,
                            c.direction_lps[0],
                            c.direction_lps[1],
                            c.direction_lps[2],
                            c.azimuth_deg,
                            c.elevation_deg,
                            c.tissue_path_mm
                                .map(|v| format!("{v:.2}"))
                                .unwrap_or_default()
                        ));
                    }
                    fs::write(&path, text)?;
                }
                if let Some(path) = output {
                    let id = id.clone().unwrap_or_else(|| {
                        format!("{}.direction-candidates", case_document.case_id)
                    });
                    let document = openbnct_plan::directions::build_direction_candidates_document(
                        &id,
                        &case_document.case_id,
                        candidates.clone(),
                        scoring,
                        azimuth_steps,
                        elevation_steps,
                        data_bytes.as_ref().map(|_| radius_cm),
                        openbnct_core::ContentReference {
                            id: case_document.case_id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&case_bytes),
                        },
                        openbnct_core::ContentReference {
                            id: aim.name.clone(),
                            sha256: openbnct_evidence::sha256_hex(&aim_bytes),
                        },
                        body_bytes.as_ref().zip(body.as_ref()).map(|(bytes, mask)| {
                            openbnct_core::ContentReference {
                                id: mask.name.clone(),
                                sha256: openbnct_evidence::sha256_hex(bytes),
                            }
                        }),
                        data_bytes
                            .as_ref()
                            .map(|bytes| openbnct_core::ContentReference {
                                id: "multigroup-data".into(),
                                sha256: openbnct_evidence::sha256_hex(bytes),
                            }),
                        provenance_id
                            .as_deref()
                            .unwrap_or(&format!("directions:{id}")),
                    );
                    write_new_json(&path, &document)?;
                    println!("direction candidates at {}", path.display());
                }
                eprintln!(
                    "{} candidates enumerated, top {take} emitted",
                    candidates.len()
                );
            }
            PlanCommand::Synthesize {
                case,
                data,
                assignment,
                objective,
                mask,
                aim_mask,
                dose,
                weights,
                azimuth_steps,
                elevation_steps,
                top,
                radius_cm,
                order,
                output,
                spectrum,
                radii,
                id,
                provenance_id,
            } => {
                use openbnct_plan::optimize::{BeamDoseField, DoseQuantity, InversePlanObjective};
                use openbnct_plan::synthesis;
                let case_bytes = fs::read(&case)?;
                let case_document: TransportCase =
                    serde_json::from_slice(&case_bytes).map_err(|error| {
                        io::Error::other(format!("case {}: {error}", case.display()))
                    })?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes).map_err(|error| {
                        io::Error::other(format!("data {}: {error}", data.display()))
                    })?;
                let objective_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&objective_bytes).map_err(|error| {
                        io::Error::other(format!("objective {}: {error}", objective.display()))
                    })?;
                spec.validate()
                    .map_err(|error| io::Error::other(format!("objective: {error}")))?;
                let mut masks = Vec::new();
                for path in &mask {
                    let m: RegionMask =
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("mask {}: {error}", path.display()))
                        })?;
                    masks.push(m);
                }
                let aim_bytes = fs::read(&aim_mask)?;
                let aim: RegionMask = serde_json::from_slice(&aim_bytes).map_err(|error| {
                    io::Error::other(format!("mask {}: {error}", aim_mask.display()))
                })?;
                let assignment_doc: Option<MaterialAssignment> = assignment
                    .as_ref()
                    .map(|path| {
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("assignment {}: {error}", path.display()))
                        })
                    })
                    .transpose()?;
                let geometry = case_document.geometry.clone();
                let n_cells = geometry
                    .voxel_count()
                    .map_err(|error| io::Error::other(format!("geometry: {error}")))?;
                let groups = mg_data.group_count();
                let cell_mat = openbnct_transport::cell_materials(
                    &case_document,
                    &mg_data,
                    assignment_doc.as_ref(),
                )
                .map_err(|error| io::Error::other(format!("materials: {error}")))?;
                // Per-objective effective responses: the fold each
                // objective's dose quantity applies at every cell.
                let response_at = |cell: usize| {
                    mg_data.materials[cell_mat[cell]]
                        .dose_response_gy_cm2
                        .clone()
                };
                let effective_responses: Vec<Vec<Vec<f64>>> = spec
                    .objectives
                    .iter()
                    .map(|o| {
                        (0..n_cells)
                            .map(|cell| {
                                synthesis::effective_response(&spec, o, cell, &response_at, groups)
                            })
                            .collect()
                    })
                    .collect();
                let mask_voxels = synthesis::synthesis_masks(&spec, &masks, n_cells)
                    .map_err(|error| io::Error::other(format!("masks: {error}")))?;
                // Marginal mode: current dose per objective view.
                let doses: Option<Vec<Vec<f64>>> = if dose.is_empty() {
                    None
                } else {
                    let weights: Vec<f64> = weights
                        .as_deref()
                        .ok_or_else(|| io::Error::other("--dose requires --weights w1,w2,…"))?
                        .split(',')
                        .map(|t| {
                            t.trim()
                                .parse::<f64>()
                                .map_err(|error| io::Error::other(format!("--weights: {error}")))
                        })
                        .collect::<Result<_, _>>()?;
                    if weights.len() != dose.len() {
                        return Err(io::Error::other(format!(
                            "--weights has {} entries for {} dose fields",
                            weights.len(),
                            dose.len()
                        ))
                        .into());
                    }
                    let mut fields = Vec::with_capacity(dose.len());
                    for path in &dose {
                        let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(path)
                            .map_err(|error| {
                                io::Error::other(format!("dose {}: {error}", path.display()))
                            })?;
                        let name = |c: openbnct_core::DoseComponent| match c {
                            openbnct_core::DoseComponent::Boron => "boron",
                            openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                            openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                            openbnct_core::DoseComponent::Photon => "photon",
                        };
                        let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                            .components
                            .iter()
                            .map(|v| (name(v.component).to_string(), v.values.clone()))
                            .collect();
                        let values = match spec.dose_quantity {
                            DoseQuantity::PhysicalTotal => bundle.physical_total.values.clone(),
                            DoseQuantity::Component(component) => {
                                components.get(name(component)).cloned().ok_or_else(|| {
                                    io::Error::other(format!(
                                        "{}: no {:?} component",
                                        path.display(),
                                        component
                                    ))
                                })?
                            }
                            DoseQuantity::Isoeffective => Vec::new(),
                        };
                        fields.push(BeamDoseField {
                            name: path.display().to_string(),
                            values,
                            components: Some(components),
                        });
                    }
                    Some(
                        spec.objectives
                            .iter()
                            .map(|o| synthesis::objective_dose_view(&spec, o, &fields, &weights))
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|error| io::Error::other(format!("dose views: {error}")))?,
                    )
                };
                let source = synthesis::composite_adjoint_source(
                    &spec,
                    &mask_voxels,
                    &effective_responses,
                    doses.as_deref(),
                )
                .map_err(|error| io::Error::other(format!("adjoint source: {error}")))?;
                let signed = synthesis::source_is_signed(&source);
                // A signed source (sparing objectives) makes negative
                // adjoint flux legitimate — θ repair must not clamp it.
                let options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    assignment: assignment_doc.clone(),
                    theta_repair: !signed,
                    exp_source: true,
                    ..Default::default()
                };
                let adjoint = openbnct_transport::solve_multigroup_adjoint(
                    &case_document,
                    &mg_data,
                    &options,
                    &source,
                    None,
                    openbnct_core::ContentReference {
                        id: mg_data.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&data_bytes),
                    },
                    openbnct_core::ContentReference {
                        id: case_document.case_id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&case_bytes),
                    },
                )
                .map_err(|error| io::Error::other(format!("adjoint solve: {error}")))?;
                if !adjoint.converged {
                    eprintln!("warning: adjoint solve did not converge — scores are approximate");
                }
                let mut candidates = openbnct_plan::directions::enumerate_directions(
                    &geometry,
                    &aim,
                    None,
                    azimuth_steps,
                    elevation_steps,
                )
                .map_err(|error| io::Error::other(format!("directions: {error}")))?;
                // Spectrum variants swap the template's energy block;
                // the adjoint solve is shared across all of them.
                // Each spectrum keeps its bytes for the content binding.
                let spectra: Vec<(String, openbnct_transport::EnergyDistribution, String)> =
                    spectrum
                        .iter()
                        .map(|entry| {
                            let (name, path) = entry.split_once('=').ok_or_else(|| {
                                io::Error::other(format!("--spectrum {entry:?} must be NAME=PATH"))
                            })?;
                            let bytes = fs::read(path)?;
                            let energy: openbnct_transport::EnergyDistribution =
                                serde_json::from_slice(&bytes).map_err(|error| {
                                    io::Error::other(format!("spectrum {}: {error}", path))
                                })?;
                            Ok((
                                name.to_string(),
                                energy,
                                openbnct_evidence::sha256_hex(&bytes),
                            ))
                        })
                        .collect::<Result<_, Box<dyn Error>>>()?;
                let radius_list: Vec<f64> = match &radii {
                    Some(text) => text
                        .split(',')
                        .map(|part| {
                            part.trim().parse::<f64>().map_err(|_| {
                                io::Error::other(format!("--radii entry {part:?} is not a number"))
                            })
                        })
                        .collect::<Result<_, io::Error>>()?,
                    None => vec![radius_cm],
                };
                for r in &radius_list {
                    if !r.is_finite() || *r <= 0.0 {
                        return Err(io::Error::other("--radii entries must be positive").into());
                    }
                }
                let scored_variants: Vec<(Option<String>, openbnct_transport::EnergyDistribution)> =
                    if spectra.is_empty() {
                        vec![(None, case_document.source.energy.clone())]
                    } else {
                        spectra
                            .iter()
                            .map(|(n, e, _)| (Some(n.clone()), e.clone()))
                            .collect()
                    };
                // Expand the fan over (spectrum × radius): each combo
                // scores independently through the same adjoint field.
                let mut expanded: Vec<openbnct_plan::directions::DirectionCandidate> = Vec::new();
                let mut scored: Vec<(f64, usize)> = Vec::new();
                for (spec_name, energy) in &scored_variants {
                    let mut case_variant = case_document.clone();
                    case_variant.source.energy = energy.clone();
                    for r in &radius_list {
                        for candidate in &candidates {
                            let mut row = candidate.clone();
                            row.spectrum = spec_name.clone();
                            row.aperture_radius_cm = Some(*r);
                            row.name = match spec_name {
                                Some(s) => format!("{}/{s}/r{r}", candidate.name),
                                None if radius_list.len() > 1 => {
                                    format!("{}/r{r}", candidate.name)
                                }
                                _ => candidate.name.clone(),
                            };
                            let index = expanded.len();
                            let score = match openbnct_transport::adjoint_direction_score(
                                &case_variant,
                                &mg_data,
                                &options,
                                &adjoint,
                                &aim,
                                candidate.direction_lps,
                                *r,
                            ) {
                                Ok(score) => score,
                                Err(openbnct_transport::MultigroupError::ApertureOutsideFace(
                                    reason,
                                )) => {
                                    eprintln!("skip {}: {reason}", row.name);
                                    continue;
                                }
                                Err(error) => {
                                    return Err(io::Error::other(format!(
                                        "score {}: {error}",
                                        row.name
                                    ))
                                    .into());
                                }
                            };
                            row.adjoint_score = Some(score);
                            expanded.push(row);
                            scored.push((score, index));
                        }
                    }
                }
                if scored.is_empty() {
                    return Err(io::Error::other(
                        "no candidate admits an aperture that fits its entry face",
                    )
                    .into());
                }
                scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                candidates = scored
                    .iter()
                    .map(|(_, index)| expanded[*index].clone())
                    .collect();
                let take = if top == 0 { candidates.len() } else { top };
                for line in openbnct_plan::directions::beam_spec_lines(&candidates, take) {
                    println!("{line}");
                }
                if let Some(path) = output {
                    let id = id.clone().unwrap_or_else(|| {
                        format!("{}.direction-synthesis", case_document.case_id)
                    });
                    let mut document =
                        openbnct_plan::directions::build_direction_candidates_document(
                            &id,
                            &case_document.case_id,
                            candidates.clone(),
                            "adjoint_marginal_utility",
                            azimuth_steps,
                            elevation_steps,
                            Some(radius_cm),
                            openbnct_core::ContentReference {
                                id: case_document.case_id.clone(),
                                sha256: openbnct_evidence::sha256_hex(&case_bytes),
                            },
                            openbnct_core::ContentReference {
                                id: aim.name.clone(),
                                sha256: openbnct_evidence::sha256_hex(&aim_bytes),
                            },
                            None,
                            Some(openbnct_core::ContentReference {
                                id: mg_data.id.clone(),
                                sha256: openbnct_evidence::sha256_hex(&data_bytes),
                            }),
                            provenance_id
                                .as_deref()
                                .unwrap_or(&format!("synthesize:{id}")),
                        );
                    document.objective = Some(openbnct_core::ContentReference {
                        id: spec.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&objective_bytes),
                    });
                    if !spectra.is_empty() {
                        document.spectra = Some(
                            spectra
                                .iter()
                                .map(|(name, _, sha)| openbnct_core::ContentReference {
                                    id: name.clone(),
                                    sha256: sha.clone(),
                                })
                                .collect(),
                        );
                    }
                    write_new_json(&path, &document)?;
                    println!("synthesis candidates at {}", path.display());
                }
                eprintln!(
                    "{} candidates scored by marginal utility{}",
                    candidates.len(),
                    if doses.is_some() {
                        " at the current plan"
                    } else {
                        ""
                    }
                );
            }
            PlanCommand::Iterate {
                case,
                data,
                assignment,
                objective,
                mask,
                aim_mask,
                azimuth_steps,
                elevation_steps,
                radius_cm,
                spectrum,
                radii,
                rounds,
                add,
                order,
                convergence,
                max_inner,
                max_outer,
                output_dir,
                id,
                provenance_id,
            } => {
                use openbnct_plan::optimize::{
                    BeamDoseField, InversePlanObjective, ResultProvenance, optimize_weights,
                };
                use openbnct_plan::synthesis;
                let case_bytes = fs::read(&case)?;
                let case_document: TransportCase =
                    serde_json::from_slice(&case_bytes).map_err(|error| {
                        io::Error::other(format!("case {}: {error}", case.display()))
                    })?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes).map_err(|error| {
                        io::Error::other(format!("data {}: {error}", data.display()))
                    })?;
                let objective_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&objective_bytes).map_err(|error| {
                        io::Error::other(format!("objective {}: {error}", objective.display()))
                    })?;
                spec.validate()
                    .map_err(|error| io::Error::other(format!("objective: {error}")))?;
                let profile = mg_data.component_profile.clone().ok_or_else(|| {
                    io::Error::other(
                        "plan iterate requires the multigroup data to declare component_profile",
                    )
                })?;
                let mut masks = Vec::new();
                for path in &mask {
                    let m: RegionMask =
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("mask {}: {error}", path.display()))
                        })?;
                    masks.push(m);
                }
                let aim_bytes = fs::read(&aim_mask)?;
                let aim: RegionMask = serde_json::from_slice(&aim_bytes).map_err(|error| {
                    io::Error::other(format!("mask {}: {error}", aim_mask.display()))
                })?;
                let assignment_doc: Option<MaterialAssignment> = assignment
                    .as_ref()
                    .map(|path| {
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("assignment {}: {error}", path.display()))
                        })
                    })
                    .transpose()?;
                let geometry = case_document.geometry.clone();
                let n_cells = geometry
                    .voxel_count()
                    .map_err(|error| io::Error::other(format!("geometry: {error}")))?;
                let groups = mg_data.group_count();
                let cell_mat = openbnct_transport::cell_materials(
                    &case_document,
                    &mg_data,
                    assignment_doc.as_ref(),
                )
                .map_err(|error| io::Error::other(format!("materials: {error}")))?;
                let response_at = |cell: usize| {
                    mg_data.materials[cell_mat[cell]]
                        .dose_response_gy_cm2
                        .clone()
                };
                let effective_responses: Vec<Vec<Vec<f64>>> = spec
                    .objectives
                    .iter()
                    .map(|o| {
                        (0..n_cells)
                            .map(|cell| {
                                synthesis::effective_response(&spec, o, cell, &response_at, groups)
                            })
                            .collect()
                    })
                    .collect();
                let mask_voxels = synthesis::synthesis_masks(&spec, &masks, n_cells)
                    .map_err(|error| io::Error::other(format!("masks: {error}")))?;
                let mut candidates = openbnct_plan::directions::enumerate_directions(
                    &geometry,
                    &aim,
                    None,
                    azimuth_steps,
                    elevation_steps,
                )
                .map_err(|error| io::Error::other(format!("directions: {error}")))?;
                // Spectrum variants: name → (energy, sha) for the
                // admission path and the report bindings.
                let spectra: Vec<(String, openbnct_transport::EnergyDistribution, String)> =
                    spectrum
                        .iter()
                        .map(|entry| {
                            let (name, path) = entry.split_once('=').ok_or_else(|| {
                                io::Error::other(format!("--spectrum {entry:?} must be NAME=PATH"))
                            })?;
                            let bytes = fs::read(path)?;
                            let energy: openbnct_transport::EnergyDistribution =
                                serde_json::from_slice(&bytes).map_err(|error| {
                                    io::Error::other(format!("spectrum {}: {error}", path))
                                })?;
                            Ok((
                                name.to_string(),
                                energy,
                                openbnct_evidence::sha256_hex(&bytes),
                            ))
                        })
                        .collect::<Result<_, Box<dyn Error>>>()?;
                let radius_list: Vec<f64> = match &radii {
                    Some(text) => text
                        .split(',')
                        .map(|part| {
                            part.trim().parse::<f64>().map_err(|_| {
                                io::Error::other(format!("--radii entry {part:?} is not a number"))
                            })
                        })
                        .collect::<Result<_, io::Error>>()?,
                    None => vec![radius_cm],
                };
                if !radius_cm.is_finite() || radius_cm <= 0.0 {
                    return Err(io::Error::other("--radius-cm must be positive").into());
                }
                for r in &radius_list {
                    if !r.is_finite() || *r <= 0.0 {
                        return Err(io::Error::other("--radii entries must be positive").into());
                    }
                }
                // Expand the fan over spectrum × radius — the same
                // composite adjoint field scores every combination.
                if !spectra.is_empty() || radius_list.len() > 1 {
                    let mut expanded = Vec::with_capacity(
                        candidates.len() * spectra.len().max(1) * radius_list.len(),
                    );
                    for candidate in &candidates {
                        for r in &radius_list {
                            if spectra.is_empty() {
                                let mut row = candidate.clone();
                                row.aperture_radius_cm = Some(*r);
                                row.name = format!("{}/r{r}", candidate.name);
                                expanded.push(row);
                            } else {
                                for (name, _, _) in &spectra {
                                    let mut row = candidate.clone();
                                    row.spectrum = Some(name.clone());
                                    row.aperture_radius_cm = Some(*r);
                                    row.name = format!("{}/{name}/r{r}", candidate.name);
                                    expanded.push(row);
                                }
                            }
                        }
                    }
                    candidates = expanded;
                }
                if add == 0 || rounds == 0 {
                    return Err(io::Error::other("--add and --rounds must be positive").into());
                }
                if output_dir.exists() && fs::read_dir(&output_dir)?.next().is_some() {
                    return Err(io::Error::other(format!(
                        "{}: output directory is not empty",
                        output_dir.display()
                    ))
                    .into());
                }
                fs::create_dir_all(&output_dir)?;
                let data_ref = openbnct_core::ContentReference {
                    id: mg_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let case_ref = openbnct_core::ContentReference {
                    id: case_document.case_id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&case_bytes),
                };
                let adjoint_options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    assignment: assignment_doc.clone(),
                    ..Default::default()
                };
                let forward_options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_doc.clone(),
                    ..Default::default()
                };
                let mut pool: Vec<openbnct_plan::directions::DirectionCandidate> = Vec::new();
                let mut fields: Vec<BeamDoseField> = Vec::new();
                let mut weights: Vec<f64> = Vec::new();
                let mut round_records: Vec<synthesis::IterationRound> = Vec::new();
                for round in 0..rounds {
                    // Marginal-utility source: at the current plan's
                    // dose when a pool exists, uniform-mask otherwise.
                    let doses: Option<Vec<Vec<f64>>> = if fields.is_empty() {
                        None
                    } else {
                        Some(
                            spec.objectives
                                .iter()
                                .map(|o| {
                                    synthesis::objective_dose_view(&spec, o, &fields, &weights)
                                })
                                .collect::<Result<Vec<_>, _>>()
                                .map_err(|error| {
                                    io::Error::other(format!("dose views: {error}"))
                                })?,
                        )
                    };
                    let source = synthesis::composite_adjoint_source(
                        &spec,
                        &mask_voxels,
                        &effective_responses,
                        doses.as_deref(),
                    )
                    .map_err(|error| io::Error::other(format!("adjoint source: {error}")))?;
                    let mut options = adjoint_options.clone();
                    options.theta_repair = !synthesis::source_is_signed(&source);
                    let adjoint = openbnct_transport::solve_multigroup_adjoint(
                        &case_document,
                        &mg_data,
                        &options,
                        &source,
                        None,
                        data_ref.clone(),
                        case_ref.clone(),
                    )
                    .map_err(|error| io::Error::other(format!("adjoint solve: {error}")))?;
                    if !adjoint.converged {
                        eprintln!("round {round}: warning — adjoint solve did not converge");
                    }
                    let taken: std::collections::BTreeSet<String> =
                        pool.iter().map(|c| c.name.clone()).collect();
                    let mut scored: Vec<(f64, usize)> = Vec::new();
                    for (index, candidate) in candidates.iter().enumerate() {
                        if taken.contains(&candidate.name) {
                            continue;
                        }
                        // Score the candidate's own spectrum/radius —
                        // expanded rows carry both.
                        let mut score_case = case_document.clone();
                        if let Some(spec_name) = &candidate.spectrum {
                            score_case.source.energy = spectra
                                .iter()
                                .find(|(n, _, _)| n == spec_name)
                                .map(|(_, e, _)| e.clone())
                                .ok_or_else(|| {
                                    io::Error::other(format!(
                                        "candidate references unknown spectrum {spec_name}"
                                    ))
                                })?;
                        }
                        let score_radius = candidate.aperture_radius_cm.unwrap_or(radius_cm);
                        match openbnct_transport::adjoint_direction_score(
                            &score_case,
                            &mg_data,
                            &options,
                            &adjoint,
                            &aim,
                            candidate.direction_lps,
                            score_radius,
                        ) {
                            Ok(score) => scored.push((score, index)),
                            Err(openbnct_transport::MultigroupError::ApertureOutsideFace(_)) => {
                                continue;
                            }
                            Err(error) => {
                                return Err(io::Error::other(format!(
                                    "score {}: {error}",
                                    candidate.name
                                ))
                                .into());
                            }
                        }
                    }
                    scored
                        .sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                    if scored.is_empty() {
                        eprintln!("round {round}: candidate pool exhausted — stopping early");
                        break;
                    }
                    let chosen: Vec<(f64, usize)> = scored.iter().take(add).copied().collect();
                    eprintln!(
                        "round {round}: adding {}",
                        chosen
                            .iter()
                            .map(|(_, i)| candidates[*i].name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    // Forward-solve each admitted beam — with the
                    // candidate's own spectrum and aperture radius.
                    for (score, index) in &chosen {
                        let candidate = &candidates[*index];
                        let admitted_radius = candidate.aperture_radius_cm.unwrap_or(radius_cm);
                        let (positioned, _report) =
                            openbnct_transport::aim_disk_source_at_centroid(
                                &case_document.source,
                                &case_document.geometry,
                                &aim,
                                candidate.direction_lps,
                                admitted_radius,
                            )
                            .map_err(|error| {
                                io::Error::other(format!("aim {}: {error}", candidate.name))
                            })?;
                        let mut aimed = case_document.clone();
                        aimed.case_id = format!("{}-{}", case_document.case_id, candidate.name);
                        aimed.source = positioned;
                        if let Some(spec_name) = &candidate.spectrum {
                            aimed.source.energy = spectra
                                .iter()
                                .find(|(n, _, _)| n == spec_name)
                                .map(|(_, e, _)| e.clone())
                                .ok_or_else(|| {
                                    io::Error::other(format!(
                                        "candidate references unknown spectrum {spec_name}"
                                    ))
                                })?;
                        }
                        let aimed_ref = openbnct_core::ContentReference {
                            id: aimed.case_id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&serde_json::to_vec(&aimed)?),
                        };
                        let flux = openbnct_transport::solve_multigroup(
                            &aimed,
                            &mg_data,
                            &forward_options,
                            data_ref.clone(),
                            aimed_ref,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{}: {error}", candidate.name))
                        })?;
                        if !flux.converged {
                            return Err(io::Error::other(format!(
                                "{}: solve did not converge (residual {:.3e})",
                                candidate.name, flux.residual
                            ))
                            .into());
                        }
                        let bundle = openbnct_transport::fold_multigroup_dose(
                            &aimed,
                            &mg_data,
                            &flux,
                            assignment_doc.as_ref(),
                            profile.clone(),
                            data_ref.clone(),
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{}: dose fold: {error}", candidate.name))
                        })?;
                        let dose_path = output_dir
                            .join(format!("{}.dose.json", candidate.name.replace('/', "_")));
                        write_new_json(&dose_path, &bundle)?;
                        let name = |c: openbnct_core::DoseComponent| match c {
                            openbnct_core::DoseComponent::Boron => "boron",
                            openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                            openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                            openbnct_core::DoseComponent::Photon => "photon",
                        };
                        let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                            .components
                            .iter()
                            .map(|v| (name(v.component).to_string(), v.values.clone()))
                            .collect();
                        let values = match spec.dose_quantity {
                            openbnct_plan::optimize::DoseQuantity::PhysicalTotal => {
                                bundle.physical_total.values.clone()
                            }
                            openbnct_plan::optimize::DoseQuantity::Component(c) => {
                                components.get(name(c)).cloned().unwrap_or_default()
                            }
                            openbnct_plan::optimize::DoseQuantity::Isoeffective => Vec::new(),
                        };
                        fields.push(BeamDoseField {
                            name: candidate.name.clone(),
                            values,
                            components: Some(components),
                        });
                        let mut recorded = candidate.clone();
                        recorded.adjoint_score = Some(*score);
                        pool.push(recorded);
                    }
                    // Re-optimize over the grown pool.
                    let result = optimize_weights(
                        &fields,
                        &masks,
                        &spec,
                        &vec![0.0; fields.len()],
                        ResultProvenance {
                            id: format!("iterate-r{round}"),
                            provenance_id: provenance_id.clone().unwrap_or_default(),
                            objective: openbnct_core::ContentReference {
                                id: spec.id.clone(),
                                sha256: openbnct_evidence::sha256_hex(&objective_bytes),
                            },
                        },
                    )
                    .map_err(|error| io::Error::other(format!("optimize: {error}")))?;
                    weights = result.weights.iter().map(|w| w.weight).collect();
                    round_records.push(synthesis::IterationRound {
                        round,
                        added: chosen
                            .iter()
                            .map(|(_, i)| candidates[*i].name.clone())
                            .collect(),
                        scores: chosen.iter().map(|(s, _)| *s).collect(),
                        penalty: result.penalty,
                        converged: result.converged,
                    });
                    eprintln!(
                        "round {round}: penalty {:.4e} over {} beams",
                        result.penalty,
                        fields.len()
                    );
                }
                if fields.is_empty() {
                    return Err(io::Error::other(
                        "iteration produced no beams — check aperture/fan geometry",
                    )
                    .into());
                }
                // Final result document + iteration report.
                let id = id
                    .clone()
                    .unwrap_or_else(|| format!("{}.iterate", case_document.case_id));
                let result = optimize_weights(
                    &fields,
                    &masks,
                    &spec,
                    &weights,
                    ResultProvenance {
                        id: format!("{id}.result"),
                        provenance_id: provenance_id
                            .clone()
                            .unwrap_or_else(|| format!("iterate:{id}")),
                        objective: openbnct_core::ContentReference {
                            id: spec.id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&objective_bytes),
                        },
                    },
                )
                .map_err(|error| io::Error::other(format!("optimize: {error}")))?;
                let result_path = output_dir.join("result.json");
                write_new_json(&result_path, &result)?;
                let report = synthesis::IterationReport {
                    schema_version: synthesis::ITERATION_REPORT_SCHEMA.into(),
                    id: id.clone(),
                    case_id: case_document.case_id.clone(),
                    case: case_ref,
                    multigroup_data: data_ref,
                    objective: openbnct_core::ContentReference {
                        id: spec.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&objective_bytes),
                    },
                    beams: pool,
                    rounds: round_records,
                    result: openbnct_core::ContentReference {
                        id: format!("{id}.result"),
                        sha256: openbnct_evidence::sha256_hex(&fs::read(&result_path)?),
                    },
                    spectra: if spectra.is_empty() {
                        None
                    } else {
                        Some(
                            spectra
                                .iter()
                                .map(|(name, _, sha)| openbnct_core::ContentReference {
                                    id: name.clone(),
                                    sha256: sha.clone(),
                                })
                                .collect(),
                        )
                    },
                    provenance_id: provenance_id.unwrap_or_else(|| format!("iterate:{id}")),
                    qualification: synthesis::ITERATION_REPORT_QUALIFICATION.into(),
                };
                let report_path = output_dir.join("iteration-report.json");
                write_new_json(&report_path, &report)?;
                println!("result: {}", result_path.display());
                println!("report: {}", report_path.display());
                eprintln!(
                    "{} beams, penalty {:.4e}, converged={}",
                    result.weights.len(),
                    result.penalty,
                    result.converged
                );
            }
            PlanCommand::Shape {
                case,
                data,
                assignment,
                objective,
                mask,
                aim_mask,
                dose,
                weights,
                direction,
                radius_cm,
                beamlets,
                keep_fraction,
                order,
                emit_fields,
                output,
                id,
                provenance_id,
            } => {
                use openbnct_plan::optimize::{BeamDoseField, DoseQuantity, InversePlanObjective};
                use openbnct_plan::synthesis;
                let case_bytes = fs::read(&case)?;
                let case_document: TransportCase =
                    serde_json::from_slice(&case_bytes).map_err(|error| {
                        io::Error::other(format!("case {}: {error}", case.display()))
                    })?;
                let data_bytes = fs::read(&data)?;
                let mg_data: openbnct_transport::MultigroupData =
                    serde_json::from_slice(&data_bytes).map_err(|error| {
                        io::Error::other(format!("data {}: {error}", data.display()))
                    })?;
                let objective_bytes = fs::read(&objective)?;
                let spec: InversePlanObjective =
                    serde_json::from_slice(&objective_bytes).map_err(|error| {
                        io::Error::other(format!("objective {}: {error}", objective.display()))
                    })?;
                spec.validate()
                    .map_err(|error| io::Error::other(format!("objective: {error}")))?;
                let mut masks = Vec::new();
                for path in &mask {
                    masks.push(
                        serde_json::from_slice::<RegionMask>(&fs::read(path)?).map_err(
                            |error| io::Error::other(format!("mask {}: {error}", path.display())),
                        )?,
                    );
                }
                let aim_bytes = fs::read(&aim_mask)?;
                let aim: RegionMask = serde_json::from_slice(&aim_bytes).map_err(|error| {
                    io::Error::other(format!("mask {}: {error}", aim_mask.display()))
                })?;
                let assignment_doc: Option<MaterialAssignment> = assignment
                    .as_ref()
                    .map(|path| {
                        serde_json::from_slice(&fs::read(path)?).map_err(|error| {
                            io::Error::other(format!("assignment {}: {error}", path.display()))
                        })
                    })
                    .transpose()?;
                let direction_lps: Vec<f64> = direction
                    .split(',')
                    .map(|part| {
                        part.trim().parse::<f64>().map_err(|_| {
                            io::Error::other(format!(
                                "--direction component {part:?} is not a number"
                            ))
                        })
                    })
                    .collect::<Result<_, io::Error>>()?;
                if direction_lps.len() != 3 {
                    return Err(io::Error::other("--direction must be dx,dy,dz").into());
                }
                let direction_lps = [direction_lps[0], direction_lps[1], direction_lps[2]];
                if !(0.0..=1.0).contains(&keep_fraction) {
                    return Err(io::Error::other("--keep-fraction must be in [0,1]").into());
                }
                let geometry = case_document.geometry.clone();
                let n_cells = geometry
                    .voxel_count()
                    .map_err(|error| io::Error::other(format!("geometry: {error}")))?;
                let groups = mg_data.group_count();
                let cell_mat = openbnct_transport::cell_materials(
                    &case_document,
                    &mg_data,
                    assignment_doc.as_ref(),
                )
                .map_err(|error| io::Error::other(format!("materials: {error}")))?;
                let response_at = |cell: usize| {
                    mg_data.materials[cell_mat[cell]]
                        .dose_response_gy_cm2
                        .clone()
                };
                let effective_responses: Vec<Vec<Vec<f64>>> = spec
                    .objectives
                    .iter()
                    .map(|o| {
                        (0..n_cells)
                            .map(|cell| {
                                synthesis::effective_response(&spec, o, cell, &response_at, groups)
                            })
                            .collect()
                    })
                    .collect();
                let mask_voxels = synthesis::synthesis_masks(&spec, &masks, n_cells)
                    .map_err(|error| io::Error::other(format!("masks: {error}")))?;
                // Optional marginal mode — current plan's dose fields.
                let doses: Option<Vec<Vec<f64>>> = if dose.is_empty() {
                    None
                } else {
                    let weight_list: Vec<f64> = weights
                        .as_deref()
                        .unwrap_or("")
                        .split(',')
                        .filter(|part| !part.trim().is_empty())
                        .map(|part| {
                            part.trim().parse().map_err(|_| {
                                io::Error::other(format!(
                                    "--weights entry {part:?} is not a number"
                                ))
                            })
                        })
                        .collect::<Result<_, io::Error>>()?;
                    if weight_list.len() != dose.len() {
                        return Err(io::Error::other(format!(
                            "--weights has {} entries, expected {}",
                            weight_list.len(),
                            dose.len()
                        ))
                        .into());
                    }
                    let mut fields = Vec::new();
                    for path in &dose {
                        let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(path)
                            .map_err(|error| {
                                io::Error::other(format!("{}: {error}", path.display()))
                            })?;
                        let name = |c: openbnct_core::DoseComponent| match c {
                            openbnct_core::DoseComponent::Boron => "boron",
                            openbnct_core::DoseComponent::Nitrogen => "nitrogen",
                            openbnct_core::DoseComponent::Hydrogen => "hydrogen",
                            openbnct_core::DoseComponent::Photon => "photon",
                        };
                        let components: std::collections::BTreeMap<String, Vec<f64>> = bundle
                            .components
                            .iter()
                            .map(|v| (name(v.component).to_string(), v.values.clone()))
                            .collect();
                        let values = match spec.dose_quantity {
                            DoseQuantity::PhysicalTotal => bundle.physical_total.values.clone(),
                            DoseQuantity::Component(c) => {
                                components.get(name(c)).cloned().unwrap_or_default()
                            }
                            DoseQuantity::Isoeffective => Vec::new(),
                        };
                        fields.push(BeamDoseField {
                            name: path.display().to_string(),
                            values,
                            components: Some(components),
                        });
                    }
                    Some(
                        spec.objectives
                            .iter()
                            .map(|o| {
                                synthesis::objective_dose_view(&spec, o, &fields, &weight_list)
                            })
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(|error| io::Error::other(format!("dose views: {error}")))?,
                    )
                };
                let source = synthesis::composite_adjoint_source(
                    &spec,
                    &mask_voxels,
                    &effective_responses,
                    doses.as_deref(),
                )
                .map_err(|error| io::Error::other(format!("adjoint source: {error}")))?;
                let signed = synthesis::source_is_signed(&source);
                let options = openbnct_transport::SnOptions {
                    quadrature_order: order,
                    assignment: assignment_doc.clone(),
                    theta_repair: !signed,
                    exp_source: true,
                    ..Default::default()
                };
                let data_ref = openbnct_core::ContentReference {
                    id: mg_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                let case_ref = openbnct_core::ContentReference {
                    id: case_document.case_id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&case_bytes),
                };
                let adjoint = openbnct_transport::solve_multigroup_adjoint(
                    &case_document,
                    &mg_data,
                    &options,
                    &source,
                    None,
                    data_ref.clone(),
                    case_ref.clone(),
                )
                .map_err(|error| io::Error::other(format!("adjoint solve: {error}")))?;
                if !adjoint.converged {
                    eprintln!("warning: adjoint solve did not converge — scores approximate");
                }
                // Aim the full disk, then subdivide into beamlets.
                let (aimed_source, _report) = openbnct_transport::aim_disk_source_at_centroid(
                    &case_document.source,
                    &case_document.geometry,
                    &aim,
                    direction_lps,
                    radius_cm,
                )
                .map_err(|error| io::Error::other(format!("aim: {error}")))?;
                let openbnct_transport::SourceSpatialDistribution::UniformDisk {
                    axis: disk_axis,
                    offset_cm: disk_offset,
                    center_uv_cm: disk_center,
                    ..
                } = aimed_source.space.clone()
                else {
                    return Err(io::Error::other("aimed source is not a disk").into());
                };
                let tiles = synthesis::beamlet_tiling(radius_cm, beamlets);
                if tiles.is_empty() {
                    return Err(
                        io::Error::other("beamlet grid cannot tile the aperture disk").into(),
                    );
                }
                let mut beamlet_scores = Vec::with_capacity(tiles.len());
                for (offset, sub_radius) in &tiles {
                    let mut beamlet_case = case_document.clone();
                    beamlet_case.source = aimed_source.clone();
                    beamlet_case.source.space =
                        openbnct_transport::SourceSpatialDistribution::UniformDisk {
                            axis: disk_axis,
                            offset_cm: disk_offset,
                            center_uv_cm: [disk_center[0] + offset[0], disk_center[1] + offset[1]],
                            radius_cm: *sub_radius,
                        };
                    let uncollided = match openbnct_transport::uncollided_beam_flux(
                        &beamlet_case,
                        &mg_data,
                        &cell_mat,
                        openbnct_transport::SourceWeighting::CollapseConsistent,
                    ) {
                        Ok(Some(unc)) => Some(unc),
                        Ok(None) => {
                            return Err(
                                io::Error::other("beamlet uncollided path unavailable").into()
                            );
                        }
                        // A sub-voxel beamlet illuminates no cell
                        // centers — under-resolved, not zero-utility.
                        Err(openbnct_transport::MultigroupError::Source(m))
                            if m.contains("illuminates no cell centers") =>
                        {
                            None
                        }
                        Err(error) => return Err(io::Error::other(error.to_string()).into()),
                    };
                    let mut utility = 0.0;
                    if let Some(unc) = &uncollided {
                        for (b, a) in unc.iter().zip(adjoint.flux.iter()).take(n_cells) {
                            for (bv, av) in b.iter().zip(a.iter()) {
                                utility += bv * av;
                            }
                        }
                    }
                    let area = std::f64::consts::PI * sub_radius * sub_radius;
                    beamlet_scores.push(synthesis::BeamletScore {
                        center_uv_cm: *offset,
                        radius_cm: *sub_radius,
                        utility,
                        utility_density: if uncollided.is_some() {
                            utility / area
                        } else {
                            0.0
                        },
                        kept: false,
                        resolved: uncollided.is_some(),
                    });
                }
                let max_density = beamlet_scores
                    .iter()
                    .map(|b| b.utility_density)
                    .fold(f64::NEG_INFINITY, f64::max);
                for b in &mut beamlet_scores {
                    b.kept = b.resolved && b.utility_density >= keep_fraction * max_density;
                }
                let unresolved = beamlet_scores.iter().filter(|b| !b.resolved).count();
                if unresolved > 0 {
                    eprintln!(
                        "warning: {unresolved} beamlets under-resolve the voxel grid — refine the mesh or coarsen --beamlets"
                    );
                }
                let kept_count = beamlet_scores.iter().filter(|b| b.kept).count();
                let id = id
                    .clone()
                    .unwrap_or_else(|| format!("{}.aperture-shape", case_document.case_id));
                let document = synthesis::ApertureShapeDocument {
                    schema_version: synthesis::APERTURE_SHAPE_SCHEMA.into(),
                    id: id.clone(),
                    case_id: case_document.case_id.clone(),
                    direction_lps,
                    disk_radius_cm: radius_cm,
                    beamlet_grid: beamlets,
                    keep_fraction,
                    beamlets: beamlet_scores,
                    case: case_ref,
                    aim_mask: openbnct_core::ContentReference {
                        id: aim.name.clone(),
                        sha256: openbnct_evidence::sha256_hex(&aim_bytes),
                    },
                    multigroup_data: data_ref.clone(),
                    objective: Some(openbnct_core::ContentReference {
                        id: spec.id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&objective_bytes),
                    }),
                    provenance_id: provenance_id.unwrap_or_else(|| format!("shape:{id}")),
                    qualification: synthesis::APERTURE_SHAPE_QUALIFICATION.into(),
                };
                write_new_json(&output, &document)?;
                println!(
                    "aperture: {}/{} beamlets kept → {}",
                    kept_count,
                    document.beamlets.len(),
                    output.display()
                );
                // Optional: forward-solve each kept beamlet into a dose
                // bundle so `plan optimize` produces the intensity map.
                if let Some(dir) = emit_fields {
                    if dir.exists() && fs::read_dir(&dir)?.next().is_some() {
                        return Err(io::Error::other(format!(
                            "{}: field output directory is not empty",
                            dir.display()
                        ))
                        .into());
                    }
                    let kept: Vec<&synthesis::BeamletScore> =
                        document.beamlets.iter().filter(|b| b.kept).collect();
                    const MAX_FIELD_BEAMLETS: usize = 256;
                    if kept.len() > MAX_FIELD_BEAMLETS {
                        return Err(io::Error::other(format!(
                            "{} kept beamlets exceed the {} field-solve cap — coarsen --beamlets or raise --keep-fraction",
                            kept.len(),
                            MAX_FIELD_BEAMLETS
                        ))
                        .into());
                    }
                    let profile = mg_data.component_profile.clone().ok_or_else(|| {
                        io::Error::other(
                            "--emit-fields requires component_profile in the multigroup data",
                        )
                    })?;
                    let forward_options = openbnct_transport::SnOptions {
                        quadrature_order: order,
                        assignment: assignment_doc.clone(),
                        ..Default::default()
                    };
                    fs::create_dir_all(&dir)?;
                    for beamlet in kept {
                        let [du, dv] = beamlet.center_uv_cm;
                        let mut beamlet_case = case_document.clone();
                        beamlet_case.case_id = format!(
                            "{}-beamlet{:+.0}{:+.0}",
                            case_document.case_id,
                            du * 100.0,
                            dv * 100.0
                        );
                        beamlet_case.source = aimed_source.clone();
                        beamlet_case.source.space =
                            openbnct_transport::SourceSpatialDistribution::UniformDisk {
                                axis: disk_axis,
                                offset_cm: disk_offset,
                                center_uv_cm: [disk_center[0] + du, disk_center[1] + dv],
                                radius_cm: beamlet.radius_cm,
                            };
                        let beamlet_ref = openbnct_core::ContentReference {
                            id: beamlet_case.case_id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&serde_json::to_vec(
                                &beamlet_case,
                            )?),
                        };
                        let flux = openbnct_transport::solve_multigroup(
                            &beamlet_case,
                            &mg_data,
                            &forward_options,
                            data_ref.clone(),
                            beamlet_ref,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("beamlet ({du:+.2},{dv:+.2}): {error}"))
                        })?;
                        if !flux.converged {
                            return Err(io::Error::other(format!(
                                "beamlet ({du:+.2},{dv:+.2}): solve did not converge",
                            ))
                            .into());
                        }
                        let bundle = openbnct_transport::fold_multigroup_dose(
                            &beamlet_case,
                            &mg_data,
                            &flux,
                            assignment_doc.as_ref(),
                            profile.clone(),
                            data_ref.clone(),
                        )
                        .map_err(|error| {
                            io::Error::other(format!(
                                "beamlet ({du:+.2},{dv:+.2}) dose fold: {error}"
                            ))
                        })?;
                        let path = dir.join(format!(
                            "beamlet-{:+.0}{:+.0}.dose.json",
                            du * 100.0,
                            dv * 100.0
                        ));
                        write_new_json(&path, &bundle)?;
                    }
                    println!("beamlet fields: {} bundles → {}", kept_count, dir.display());
                }
            }
            PlanCommand::Fields {
                case,
                data,
                assignment,
                aim_mask,
                beam,
                radius_cm,
                order,
                convergence,
                max_inner,
                max_outer,
                periodic,
                no_uncollided_split,
                no_transport_correction,
                p1,
                anisotropy,
                anderson,
                screen_order,
                screen_convergence,
                screen_max_inner,
                screen_max_outer,
                keep_top,
                output_dir,
            } => {
                use openbnct_plan::fields::{
                    BEAM_FIELD_SET_QUALIFICATION, BEAM_FIELD_SET_SCHEMA, BeamFieldArtifacts,
                    BeamFieldSet, FieldScreening, FieldScreeningScore, FieldSweepOptions,
                };
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
                let mask = read_region_mask(&aim_mask)?;
                if !radius_cm.is_finite() || radius_cm <= 0.0 {
                    return Err(io::Error::other("--radius-cm must be positive").into());
                }
                let beams: Vec<(String, [f64; 3])> = beam
                    .iter()
                    .map(|text| {
                        let mut parts = text.splitn(2, ',');
                        let name = parts.next().unwrap_or_default().trim().to_string();
                        let dir: Vec<f64> = parts
                            .next()
                            .unwrap_or_default()
                            .split(',')
                            .map(|part| {
                                part.trim().parse().map_err(|_| {
                                    io::Error::other(format!(
                                        "--beam direction component {part:?} is not a number"
                                    ))
                                })
                            })
                            .collect::<Result<_, _>>()?;
                        if name.is_empty() || dir.len() != 3 {
                            return Err(io::Error::other(format!(
                                "--beam {text:?} must be name,dx,dy,dz"
                            ))
                            .into());
                        }
                        Ok((name, [dir[0], dir[1], dir[2]]))
                    })
                    .collect::<Result<_, Box<dyn Error>>>()?;
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
                    progress: false,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model.clone(),
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: !no_transport_correction,
                    p1_anisotropic: p1,
                    anisotropy_order: anisotropy,
                    anderson_depth: anderson,
                    coarse_rebalance: true,
                    inner_convergence: None,
                    theta_repair: true,
                    exp_source: true,
                    source_weighting: openbnct_transport::SourceWeighting::CollapseConsistent,
                };
                match (screen_order, keep_top) {
                    (Some(_), None) => {
                        return Err(io::Error::other("--screen-order requires --keep-top").into());
                    }
                    (None, Some(_)) => {
                        return Err(io::Error::other("--keep-top requires --screen-order").into());
                    }
                    (Some(_), Some(k)) if k == 0 || k > beams.len() => {
                        return Err(io::Error::other(format!(
                            "--keep-top {k} out of range for {} declared beams",
                            beams.len()
                        ))
                        .into());
                    }
                    _ => {}
                }
                let profile = mg_data.component_profile.clone().ok_or_else(|| {
                    io::Error::other(
                        "plan fields requires the multigroup data to declare component_profile",
                    )
                })?;
                let data_ref = openbnct_core::ContentReference {
                    id: mg_data.id.clone(),
                    sha256: openbnct_evidence::sha256_hex(&data_bytes),
                };
                if output_dir.exists() && fs::read_dir(&output_dir)?.next().is_some() {
                    return Err(io::Error::other(format!(
                        "{}: output directory is not empty",
                        output_dir.display()
                    ))
                    .into());
                }
                fs::create_dir_all(&output_dir)?;

                // Aim every declared beam once — the screening solves
                // and the retained fine solves share the aimed cases.
                let mut aimed_beams = Vec::with_capacity(beams.len());
                for (name, direction) in &beams {
                    let (positioned, mut report) = openbnct_transport::aim_disk_source_at_centroid(
                        &transport_case.source,
                        &transport_case.geometry,
                        &mask,
                        *direction,
                        radius_cm,
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                    report.case_id = format!("{}-{}", transport_case.case_id, name);
                    let mut aimed = transport_case.clone();
                    aimed.case_id = report.case_id.clone();
                    aimed.source = positioned;
                    aimed_beams.push((name.clone(), *direction, aimed, report));
                }

                // Coarse screening stage: every declared beam gets a
                // cheap solve; the mean aim-mask physical total under
                // unit weight ranks them for retention.
                let screening = if let Some(screen_order) = screen_order {
                    let screen_opts = openbnct_transport::SnOptions {
                        quadrature_order: screen_order,
                        convergence: screen_convergence,
                        max_inner_iterations: screen_max_inner,
                        max_outer_iterations: screen_max_outer,
                        assignment: options.assignment.clone(),
                        ..options
                    };
                    let keep = keep_top.unwrap_or(beams.len());
                    let mut scores = Vec::with_capacity(aimed_beams.len());
                    for (name, _, aimed, _) in &aimed_beams {
                        let screen_case_bytes = serde_json::to_vec(aimed)?;
                        let screen_case_ref = openbnct_core::ContentReference {
                            id: aimed.case_id.clone(),
                            sha256: openbnct_evidence::sha256_hex(&screen_case_bytes),
                        };
                        let flux = openbnct_transport::solve_multigroup(
                            aimed,
                            &mg_data,
                            &screen_opts,
                            data_ref.clone(),
                            screen_case_ref,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{name}: screening solve: {error}"))
                        })?;
                        let bundle = openbnct_transport::fold_multigroup_dose(
                            aimed,
                            &mg_data,
                            &flux,
                            assignment_model.as_ref(),
                            profile.clone(),
                            data_ref.clone(),
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{name}: screening dose fold: {error}"))
                        })?;
                        let included = mask.included_voxel_count();
                        let score = bundle
                            .physical_total
                            .values
                            .iter()
                            .zip(mask.voxels.iter())
                            .filter(|(_, on)| **on)
                            .map(|(v, _)| *v)
                            .sum::<f64>()
                            / included.max(1) as f64;
                        println!(
                            "{name}: screening score {:.4e} ({}, residual {:.2e})",
                            score,
                            if flux.converged {
                                "converged"
                            } else {
                                "unconverged"
                            },
                            flux.residual
                        );
                        scores.push(FieldScreeningScore {
                            name: name.clone(),
                            score,
                            converged: flux.converged,
                            retained: false,
                        });
                    }
                    // Rank by score descending; stable order keeps the
                    // declaration order among equal scores.
                    let mut ranked: Vec<usize> = (0..scores.len()).collect();
                    ranked.sort_by(|&a, &b| {
                        scores[b]
                            .score
                            .partial_cmp(&scores[a].score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for &i in ranked.iter().take(keep) {
                        scores[i].retained = true;
                    }
                    Some(FieldScreening {
                        solver: FieldSweepOptions {
                            quadrature_order: screen_opts.quadrature_order,
                            convergence: screen_opts.convergence,
                            max_inner_iterations: screen_opts.max_inner_iterations,
                            max_outer_iterations: screen_opts.max_outer_iterations,
                            periodic: screen_opts.periodic,
                            beam_uncollided_split: screen_opts.beam_uncollided_split,
                            transport_correction: screen_opts.transport_correction,
                            p1_anisotropic: screen_opts.p1_anisotropic,
                            anisotropy_order: screen_opts.anisotropy_order,
                            anderson_depth: screen_opts.anderson_depth,
                        },
                        metric: "aim_mask_mean_physical_total".into(),
                        keep_top: keep,
                        scores,
                    })
                } else {
                    None
                };

                let mut artifacts = Vec::with_capacity(beams.len());
                for (name, direction, aimed, report) in &aimed_beams {
                    if let Some(screening) = &screening
                        && let Some(score) = screening.scores.iter().find(|s| s.name == *name)
                        && !score.retained
                    {
                        println!("{name}: screened out (score {:.4e})", score.score);
                        continue;
                    }
                    let case_path = output_dir.join(format!("{name}.case.json"));
                    write_new_json(&case_path, aimed)?;
                    let case_bytes = fs::read(&case_path)?;
                    let report_path = output_dir.join(format!("{name}.position-report.json"));
                    write_new_json(&report_path, report)?;
                    let case_ref = openbnct_core::ContentReference {
                        id: aimed.case_id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&case_bytes),
                    };
                    let flux = openbnct_transport::solve_multigroup(
                        aimed,
                        &mg_data,
                        &options,
                        data_ref.clone(),
                        case_ref.clone(),
                    )
                    .map_err(|error| io::Error::other(format!("{name}: multigroup: {error}")))?;
                    if !flux.converged {
                        return Err(io::Error::other(format!(
                            "{name}: solve did not converge (residual {:.3e} after {} outer iterations)",
                            flux.residual, flux.outer_iterations
                        ))
                        .into());
                    }
                    let bundle = openbnct_transport::fold_multigroup_dose(
                        aimed,
                        &mg_data,
                        &flux,
                        assignment_model.as_ref(),
                        profile.clone(),
                        data_ref.clone(),
                    )
                    .map_err(|error| io::Error::other(format!("{name}: dose fold: {error}")))?;
                    let dose_path = output_dir.join(format!("{name}.dose.json"));
                    write_new_json(&dose_path, &bundle)?;
                    artifacts.push(BeamFieldArtifacts {
                        name: name.clone(),
                        direction_lps: *direction,
                        case: case_ref,
                        position_report: openbnct_core::ContentReference {
                            id: format!("{name}.position-report"),
                            sha256: openbnct_evidence::sha256_hex(&fs::read(&report_path)?),
                        },
                        dose: openbnct_core::ContentReference {
                            id: format!("{name}.dose"),
                            sha256: openbnct_evidence::sha256_hex(&fs::read(&dose_path)?),
                        },
                        converged: flux.converged,
                        outer_iterations: flux.outer_iterations,
                        residual: flux.residual,
                    });
                    println!(
                        "{name}: converged ({} outers, residual {:.2e}) → {}",
                        flux.outer_iterations,
                        flux.residual,
                        dose_path.display()
                    );
                }
                let manifest = BeamFieldSet {
                    schema_version: BEAM_FIELD_SET_SCHEMA.into(),
                    id: format!("{}.beam-field-set", transport_case.case_id),
                    case_id: transport_case.case_id.clone(),
                    case: openbnct_core::ContentReference {
                        id: transport_case.case_id.clone(),
                        sha256: openbnct_evidence::sha256_hex(&case_bytes),
                    },
                    data: data_ref,
                    assignment: match &assignment {
                        Some(path) => Some(openbnct_core::ContentReference {
                            id: path
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            sha256: openbnct_evidence::sha256_hex(&fs::read(path)?),
                        }),
                        None => None,
                    },
                    aim_mask: openbnct_core::ContentReference {
                        id: mask.name.clone(),
                        sha256: openbnct_evidence::sha256_hex(&fs::read(&aim_mask)?),
                    },
                    aperture_radius_cm: radius_cm,
                    solver: FieldSweepOptions {
                        quadrature_order: order,
                        convergence,
                        max_inner_iterations: max_inner,
                        max_outer_iterations: max_outer,
                        periodic: periodic_axes,
                        beam_uncollided_split: !no_uncollided_split,
                        transport_correction: !no_transport_correction,
                        p1_anisotropic: p1,
                        anisotropy_order: anisotropy,
                        anderson_depth: anderson,
                    },
                    screening,
                    beams: artifacts,
                    provenance_id: format!(
                        "plan-fields:{}",
                        &openbnct_evidence::sha256_hex(&case_bytes)[..12]
                    ),
                    qualification: BEAM_FIELD_SET_QUALIFICATION.into(),
                };
                manifest
                    .validate()
                    .map_err(|error| io::Error::other(format!("field-set: {error}")))?;
                let manifest_path = output_dir.join("fields.json");
                write_new_json(&manifest_path, &manifest)?;
                println!("field-set manifest: {}", manifest_path.display());
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
            pk_model,
            pk_samples,
            pk_bootstrap,
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
                    io::Error::other("--limit entries must be NAME=max|mean|dN:LIMIT")
                })?;
                let (metric, value) = rest.split_once(':').ok_or_else(|| {
                    io::Error::other("--limit entries must be NAME=max|mean|dN:LIMIT")
                })?;
                let metric = match metric {
                    "max" => openbnct_evidence::LimitMetric::Max,
                    "mean" => openbnct_evidence::LimitMetric::Mean,
                    other if other.starts_with('d') && other.len() > 1 => {
                        let percent: u16 = other[1..].parse().map_err(|_| {
                            io::Error::other(format!(
                                "--limit {name}: invalid dose-coverage metric {other:?} (dN, N in 1..=100)"
                            ))
                        })?;
                        if !(1..=100).contains(&percent) {
                            return Err(io::Error::other(format!(
                                "--limit {name}: dose-coverage percent {percent} out of 1..=100"
                            ))
                            .into());
                        }
                        openbnct_evidence::LimitMetric::DoseCoverage { percent }
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "--limit {name}: unknown metric {other:?} (max|mean|dN)"
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
            let dose_schema = openbnct_core::normalize_contract_id(
                schema
                    .get("schema_version")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default(),
            );
            if let Some(pk_path) = &pk_model {
                let pk: openbnct_evidence::PkModel = serde_json::from_slice(&fs::read(pk_path)?)?;
                let pk_ref = openbnct_core::ContentReference {
                    id: pk_path.display().to_string(),
                    sha256: openbnct_evidence::sha256_file(pk_path)?,
                };
                let (case_id, total_values, boron_values, unit) = match dose_schema.as_str() {
                    openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                        let bundle: PhysicalDoseBundle =
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
                        let (total, total_unit) = dose_values(&bundle, &quantity)?;
                        let (total, boron, unit) = pk_boron_split(
                            &quantity,
                            total,
                            total_unit,
                            bundle.components.iter().map(|v| {
                                (
                                    v.component,
                                    v.values.as_slice(),
                                    match v.unit {
                                        openbnct_core::DoseUnit::Gray => "gray",
                                        openbnct_core::DoseUnit::GrayPerSourceParticle => {
                                            "gray_per_source_particle"
                                        }
                                    },
                                )
                            }),
                        )?;
                        (bundle.case_id.clone(), total, boron, unit)
                    }
                    openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
                        let bundle: openbnct_bio::BiologicalDoseBundle =
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
                        let (total, total_unit) = biological_dose_values(&bundle, &quantity)?;
                        let (total, boron, unit) = pk_boron_split(
                            &quantity,
                            total,
                            total_unit,
                            bundle
                                .components
                                .iter()
                                .map(|v| (v.component, v.values.as_slice(), v.unit.as_str())),
                        )?;
                        (bundle.case_id.clone(), total, boron, unit)
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "unsupported dose bundle schema {other:?}"
                        ))
                        .into());
                    }
                };

                let samples_doc = match &pk_samples {
                    Some(path) => Some((
                        serde_json::from_slice::<openbnct_evidence::PkSamples>(&fs::read(path)?)?,
                        pk_bootstrap,
                    )),
                    None => None,
                };
                let report = openbnct_evidence::PkIrradiationReport::evaluate(
                    &case_id,
                    &quantity,
                    source,
                    pk_ref,
                    &pk,
                    &unit,
                    &total_values,
                    &boron_values,
                    &region_masks,
                    &organ_limits,
                    source_strength,
                    samples_doc.as_ref().map(|(s, n)| (s, *n)),
                )?;
                write_new_json(&output, &report)?;
                println!("pk irradiation-time report at {}", output.display());
                for region in &report.regions {
                    if let Some(u) = &region.time_uncertainty {
                        println!(
                            "{} {:?}: t* {:.6e} s [P05 {:.6e}, P95 {:.6e}] over {}/{} replicates",
                            region.region,
                            region.metric,
                            u.p50_s,
                            u.p05_s,
                            u.p95_s,
                            u.converged,
                            u.replicates
                        );
                    }
                    match (region.max_time_s, region.static_max_time_s) {
                        (Some(pk_t), Some(static_t)) => println!(
                            "{} {:?} limit {}: pk {:.6e} s vs static {:.6e} s ({:+.1}%)",
                            region.region,
                            region.metric,
                            region.limit,
                            pk_t,
                            static_t,
                            region.relative_deviation.unwrap_or(0.0) * 100.0,
                        ),
                        (None, _) => println!(
                            "{} {:?} limit {}: asymptote below limit -> unbounded",
                            region.region, region.metric, region.limit,
                        ),
                        _ => println!(
                            "{} {:?} limit {}: zero endpoint rate -> unbounded",
                            region.region, region.metric, region.limit,
                        ),
                    }
                }
                if let Some(limiting) = &report.limiting {
                    println!(
                        "limiting structure: {} ({:?}), max {:.6e} s",
                        limiting.region, limiting.metric, limiting.max_time_s
                    );
                }
            } else {
                let report = match dose_schema.as_str() {
                    openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                        let bundle: PhysicalDoseBundle =
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
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
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
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
        }
        Some(Command::Pk { command }) => match command {
            PkCommand::Fit {
                samples,
                id,
                exponentials,
                output,
            } => {
                let samples_doc: openbnct_evidence::PkSamples =
                    serde_json::from_slice(&fs::read(&samples)?)?;
                let model = openbnct_evidence::fit_pk_model(&samples_doc, &id, exponentials)?;
                write_new_json(&output, &model)?;
                println!("pk model at {}", output.display());
                for region in &model.regions {
                    println!(
                        "{}: {} exponential terms, planned {} ppm",
                        region.region,
                        region.amplitudes_ppm.len(),
                        region.planned_concentration_ppm
                    );
                }
            }
            PkCommand::TissueScale {
                blood_model,
                spec,
                id,
                output,
            } => {
                let blood: openbnct_evidence::PkModel =
                    serde_json::from_slice(&fs::read(&blood_model)?)?;
                let tissue_spec: openbnct_evidence::PkTissueSpec =
                    serde_json::from_slice(&fs::read(&spec)?)?;
                let model = openbnct_evidence::apply_tissue_spec(&tissue_spec, &blood, &id)?;
                write_new_json(&output, &model)?;
                println!("tissue-scaled pk model at {}", output.display());
                for region in &model.regions {
                    println!(
                        "{}: {} exponential terms, planned {} ppm",
                        region.region,
                        region.amplitudes_ppm.len(),
                        region.planned_concentration_ppm
                    );
                }
            }
            PkCommand::Schedule {
                dose,
                quantity,
                source_strength,
                limits,
                masks,
                pk_model,
                windows_s,
                tumor_region,
                tumor_metric,
                output,
            } => {
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
                let metric_of = |name: &str, token: &str| {
                    Ok(match token {
                        "max" => openbnct_evidence::LimitMetric::Max,
                        "mean" => openbnct_evidence::LimitMetric::Mean,
                        other if other.starts_with('d') && other.len() > 1 => {
                            let percent: u16 = other[1..].parse().map_err(|_| {
                                io::Error::other(format!(
                                    "{name}: invalid dose-coverage metric {other:?}"
                                ))
                            })?;
                            if !(1..=100).contains(&percent) {
                                return Err(io::Error::other(format!(
                                    "{name}: dose-coverage percent {percent} out of 1..=100"
                                )));
                            }
                            openbnct_evidence::LimitMetric::DoseCoverage { percent }
                        }
                        other => {
                            return Err(io::Error::other(format!(
                                "{name}: unknown metric {other:?} (max|mean|dN)"
                            )));
                        }
                    })
                };
                let mut organ_limits = Vec::with_capacity(limits.len());
                for entry in &limits {
                    let (name, rest) = entry.split_once('=').ok_or_else(|| {
                        io::Error::other("--limit entries must be NAME=max|mean|dN:LIMIT")
                    })?;
                    let (metric, value) = rest.split_once(':').ok_or_else(|| {
                        io::Error::other("--limit entries must be NAME=max|mean|dN:LIMIT")
                    })?;
                    let limit: f64 = value.parse().map_err(|_| {
                        io::Error::other(format!("--limit {name}: invalid limit {value:?}"))
                    })?;
                    organ_limits.push(openbnct_evidence::OrganLimit {
                        region: name.to_owned(),
                        metric: metric_of(&format!("--limit {name}"), metric)?,
                        limit,
                    });
                }
                let tumor_metric = metric_of("--tumor-metric", &tumor_metric)?;
                let dose_source = openbnct_core::ContentReference {
                    id: dose.display().to_string(),
                    sha256: openbnct_evidence::sha256_file(&dose)?,
                };
                let dose_schema = openbnct_core::normalize_contract_id(
                    schema
                        .get("schema_version")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default(),
                );
                let pk: openbnct_evidence::PkModel = serde_json::from_slice(&fs::read(&pk_model)?)?;
                let (case_id, total, boron, unit) = match dose_schema.as_str() {
                    openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                        let bundle: PhysicalDoseBundle =
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
                        let (total, total_unit) = dose_values(&bundle, &quantity)?;
                        let (total, boron, unit) = pk_boron_split(
                            &quantity,
                            total,
                            total_unit,
                            bundle.components.iter().map(|v| {
                                (
                                    v.component,
                                    v.values.as_slice(),
                                    match v.unit {
                                        openbnct_core::DoseUnit::Gray => "gray",
                                        openbnct_core::DoseUnit::GrayPerSourceParticle => {
                                            "gray_per_source_particle"
                                        }
                                    },
                                )
                            }),
                        )?;
                        (bundle.case_id.clone(), total, boron, unit)
                    }
                    openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
                        let bundle: openbnct_bio::BiologicalDoseBundle =
                            openbnct_core::sidecar::from_slice_at(&dose_bytes, &dose)?;
                        let (total, total_unit) = biological_dose_values(&bundle, &quantity)?;
                        let (total, boron, unit) = pk_boron_split(
                            &quantity,
                            total,
                            total_unit,
                            bundle
                                .components
                                .iter()
                                .map(|v| (v.component, v.values.as_slice(), v.unit.as_str())),
                        )?;
                        (bundle.case_id.clone(), total, boron, unit)
                    }
                    other => {
                        return Err(io::Error::other(format!(
                            "unsupported dose bundle schema {other:?}"
                        ))
                        .into());
                    }
                };
                let report = openbnct_evidence::evaluate_pk_schedule(
                    &case_id,
                    &quantity,
                    dose_source,
                    openbnct_core::ContentReference {
                        id: pk_model.display().to_string(),
                        sha256: openbnct_evidence::sha256_file(&pk_model)?,
                    },
                    &pk,
                    &unit,
                    &total,
                    &boron,
                    &region_masks,
                    &organ_limits,
                    &tumor_region,
                    tumor_metric,
                    source_strength,
                    &windows_s,
                )?;
                write_new_json(&output, &report)?;
                println!("pk schedule at {}", output.display());
                for window in &report.windows {
                    let limiting = window
                        .limiting
                        .as_ref()
                        .map(|l| format!("{:.0} s ({})", l.max_time_s, l.region))
                        .unwrap_or_else(|| "unbounded".into());
                    println!(
                        "  +{:.0} s: beam-on {} | tumor dose {} | static {}",
                        window.beam_on_epoch_s,
                        limiting,
                        window
                            .tumor_dose
                            .map(|d| format!("{d:.6}"))
                            .unwrap_or_else(|| "—".into()),
                        window
                            .tumor_dose_static
                            .map(|d| format!("{d:.6}"))
                            .unwrap_or_else(|| "—".into()),
                    );
                }
                if let Some(index) = report.optimal_window_index {
                    let w = &report.windows[index];
                    println!(
                        "optimal window: beam-on at +{:.0} s, tumor dose {:.6}",
                        w.beam_on_epoch_s,
                        w.tumor_dose.unwrap_or(0.0)
                    );
                }
            }
            PkCommand::Dose {
                dose,
                pk_model,
                masks,
                source_strength,
                schedule,
                window_index,
                window_s,
                time_s,
                provenance_id,
                output,
            } => {
                let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(&dose)?;
                let pk: openbnct_evidence::PkModel = serde_json::from_slice(&fs::read(&pk_model)?)?;
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
                let (epoch, duration) = match (schedule, window_index, window_s, time_s) {
                    (Some(schedule_path), Some(index), None, None) => {
                        let report: openbnct_evidence::PkScheduleReport =
                            serde_json::from_slice(&fs::read(&schedule_path)?)?;
                        let window = report.windows.get(index).ok_or_else(|| {
                            io::Error::other(format!(
                                "--window-index {index} out of range ({} windows)",
                                report.windows.len()
                            ))
                        })?;
                        let limiting = window.limiting.as_ref().ok_or_else(|| {
                            io::Error::other(format!(
                                "schedule window {index} is unbounded — no finite beam-on duration"
                            ))
                        })?;
                        (window.beam_on_epoch_s, limiting.max_time_s)
                    }
                    (None, None, Some(w), Some(t)) => (w, t),
                    _ => {
                        return Err(io::Error::other(
                            "declare either --schedule PATH --window-index N, or --window-s W --time-s T",
                        )
                        .into())
                    }
                };
                let prov = provenance_id.unwrap_or_else(|| {
                    format!(
                        "pk-dose-{}-{}-{:.0}-{:.0}",
                        bundle.case_id, pk.id, epoch, duration
                    )
                });
                let integrated = openbnct_evidence::pk_integrated_dose_bundle(
                    &bundle,
                    &pk,
                    &region_masks,
                    epoch,
                    duration,
                    source_strength,
                    prov,
                )?;
                write_new_json(&output, &integrated)?;
                let boron = integrated
                    .components
                    .iter()
                    .find(|c| c.component == openbnct_core::DoseComponent::Boron);
                println!(
                    "pk-integrated dose at {} (beam-on +{:.0} s, {:.0} s)",
                    output.display(),
                    epoch,
                    duration
                );
                if let Some(b) = boron {
                    let max = b.values.iter().copied().fold(0.0, f64::max);
                    println!("  boron component: max {:.6} Gy", max);
                }
                println!(
                    "  physical total: max {:.6} Gy",
                    integrated
                        .physical_total
                        .values
                        .iter()
                        .copied()
                        .fold(0.0, f64::max)
                );
            }
        },
        Some(Command::Metrics {
            dose,
            quantity,
            mask,
            dx,
            vx,
            eud,
            output,
        }) => {
            let metrics = compute_metrics_file(&dose, &quantity, &mask, &dx, &vx, &eud, &output)?;
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
                let bundle = load_dose_bundle(&dose_bytes, &dose)?;
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
                let tcp_eval: openbnct_bio::EndpointEvaluation =
                    serde_json::from_slice(&tcp_bytes)?;
                let mut ntcp_inputs = Vec::with_capacity(ntcp.len());
                for path in &ntcp {
                    let bytes = fs::read(path)?;
                    let eval: openbnct_bio::EndpointEvaluation = serde_json::from_slice(&bytes)?;
                    ntcp_inputs.push((eval, bytes));
                }
                let ntcp_refs: Vec<(&openbnct_bio::EndpointEvaluation, &[u8])> = ntcp_inputs
                    .iter()
                    .map(|(eval, bytes)| (eval, bytes.as_slice()))
                    .collect();
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
                let evaluation = openbnct_bio::combine_utcp_multi(
                    &tcp_eval,
                    &tcp_bytes,
                    &ntcp_refs,
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
            RegisterCommand::FrameOfReference {
                id,
                uid,
                moving,
                fixed,
                note,
                output,
            } => {
                let registration = openbnct_core::shared_for_registration(
                    id,
                    image_reference(&moving)?,
                    image_reference(&fixed)?,
                    uid,
                    note,
                )?;
                write_new_json(&output, &registration)?;
                println!("registration at {}", output.display());
                println!("method: shared_frame_of_reference (identity transform)");
                println!(
                    "frame of reference: {}",
                    registration.frame_of_reference_uid.as_deref().unwrap_or("")
                );
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
                if let Some(for_uid) = &registration.frame_of_reference_uid {
                    println!("frame of reference: {for_uid}");
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
            BoronCommand::Dose {
                physical_bundle,
                unit_dose,
                blood_ug_g,
                ratios,
                masks,
                default_ratio,
                boron_field,
                output,
            } => {
                let physical: PhysicalDoseBundle =
                    openbnct_core::sidecar::load_json(&physical_bundle)?;
                let unit: openbnct_transport::BoronUnitDose =
                    openbnct_core::sidecar::load_json(&unit_dose)?;
                let n = unit
                    .geometry
                    .voxel_count()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let (conc, sigma, spec): (Vec<f64>, Option<Vec<f64>>, String) =
                    match (blood_ug_g, &boron_field) {
                        (Some(blood), None) => {
                            let mut regions = Vec::new();
                            let loaded = load_named_masks(&masks)?;
                            let mut ratio_of = std::collections::BTreeMap::new();
                            for pair in &ratios {
                                let (name, value) = pair.split_once('=').ok_or_else(|| {
                                    io::Error::other(format!(
                                        "--ratio {pair:?} must be written as NAME=value"
                                    ))
                                })?;
                                let value: f64 = value.parse().map_err(|_| {
                                    io::Error::other(format!("--ratio {pair:?}: bad number"))
                                })?;
                                if ratio_of.insert(name.to_string(), value).is_some() {
                                    return Err(io::Error::other(format!(
                                        "--ratio {name:?} given twice"
                                    ))
                                    .into());
                                }
                            }
                            for mask in &loaded {
                                let ratio = ratio_of.get(&mask.name).ok_or_else(|| {
                                    io::Error::other(format!(
                                        "--mask {:?} has no matching --ratio",
                                        mask.name
                                    ))
                                })?;
                                regions.push(openbnct_transport::RatioRegion {
                                    name: mask.name.clone(),
                                    ratio: *ratio,
                                    mask: mask.voxels.clone(),
                                });
                            }
                            if let Some(name) = ratio_of
                                .keys()
                                .find(|k| !loaded.iter().any(|m| &m.name == *k))
                            {
                                return Err(io::Error::other(format!(
                                    "--ratio {name:?} has no matching --mask"
                                ))
                                .into());
                            }
                            let conc = openbnct_transport::concentration_from_ratios(
                                blood,
                                &regions,
                                default_ratio,
                                n,
                            )
                            .map_err(|error| io::Error::other(error.to_string()))?;
                            let spec = format!(
                                "blood {blood} ug/g; ratios [{}]; default ratio {default_ratio}",
                                regions
                                    .iter()
                                    .map(|r| format!("{}={}", r.name, r.ratio))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            (conc, None, spec)
                        }
                        (None, Some(path)) => {
                            if !ratios.is_empty() || !masks.is_empty() {
                                return Err(io::Error::other(
                                    "--ratio/--mask apply to --blood-ug-g, not --boron-field",
                                )
                                .into());
                            }
                            let field_bytes = fs::read(path)?;
                            let field: openbnct_boron::BoronField =
                                serde_json::from_slice(&field_bytes)?;
                            field
                                .validate()
                                .map_err(|error| io::Error::other(error.to_string()))?;
                            if field.case_id != unit.case_id || field.geometry != unit.geometry {
                                return Err(io::Error::other(
                                    "boron field case_id/grid does not match the unit dose",
                                )
                                .into());
                            }
                            let spec = format!(
                                "boron field {} sha256 {}",
                                field.id,
                                openbnct_evidence::sha256_hex(&field_bytes)
                            );
                            (field.values, Some(field.uncertainty_1sigma), spec)
                        }
                        _ => {
                            return Err(io::Error::other(
                                "give exactly one of --blood-ug-g (with optional \
                                 --ratio/--mask) or --boron-field",
                            )
                            .into());
                        }
                    };
                let bundle = openbnct_transport::apply_boron_concentration(
                    &physical,
                    &unit,
                    &conc,
                    sigma.as_deref(),
                    &spec,
                )
                .map_err(|error| io::Error::other(format!("boron dose: {error}")))?;
                write_new_json(&output, &bundle)?;
                println!(
                    "physical dose bundle with applied boron at {} ({spec})",
                    output.display()
                );
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
            BoronCommand::Microdistribution {
                command:
                    BoronMicrodistributionCommand::Import {
                        measurement,
                        id,
                        output,
                    },
            } => {
                let measurement: openbnct_boron::BoronMicrodistributionMeasurement =
                    serde_json::from_slice(&fs::read(&measurement)?).map_err(|error| {
                        io::Error::other(format!("microdistribution measurement: {error}"))
                    })?;
                let model = openbnct_boron::import_measurement(&measurement, id)
                    .map_err(|error| io::Error::other(format!("measurement import: {error}")))?;
                model
                    .validate()
                    .map_err(|error| io::Error::other(format!("imported model: {error}")))?;
                fs::write(&output, serde_json::to_vec_pretty(&model)?)?;
                println!(
                    "microdistribution model: {} -> {}",
                    measurement.id,
                    output.display()
                );
            }
            BoronCommand::Microdistribution {
                command: BoronMicrodistributionCommand::Evaluate { model, id, output },
            } => {
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
            BoronCommand::Infer { spec, output } => {
                let spec_doc: openbnct_evidence::BoronInferenceSpec =
                    serde_json::from_slice(&fs::read(&spec)?)?;
                let report = openbnct_evidence::run_boron_inference(&spec_doc)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &report)?;
                println!("boron-inference report at {}", output.display());
                if let Some(last) = report.estimates.last() {
                    for (i, s) in spec_doc.states.iter().enumerate() {
                        println!(
                            "  {} = {:.4} ± {:.4} {} (post/prior σ {:.2})",
                            s.id,
                            last.mean[i],
                            last.sd[i],
                            s.unit,
                            report
                                .posterior_to_prior_sd
                                .iter()
                                .find(|(id, _)| id == &s.id)
                                .map(|(_, r)| *r)
                                .unwrap_or(f64::NAN)
                        );
                    }
                }
                for u in &report.unresolved {
                    println!(
                        "  unresolved direction {:?} (post/prior {:.2})",
                        u.direction, u.posterior_to_prior
                    );
                }
                for (a, b) in &report.unobserved_intervals {
                    println!("  unobserved span [{a:.0}, {b:.0}) s");
                }
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
                    progress: false,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model,
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: true,
                    p1_anisotropic: false,
                    anisotropy_order: 0,
                    anderson_depth: 0,
                    coarse_rebalance: true,
                    inner_convergence: None,
                    theta_repair: true,
                    exp_source: true,
                    source_weighting: openbnct_transport::SourceWeighting::CollapseConsistent,
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
                    progress: false,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model,
                    periodic: periodic_axes,
                    beam_uncollided_split: !no_uncollided_split,
                    transport_correction: true,
                    p1_anisotropic: false,
                    anisotropy_order: 0,
                    anderson_depth: 0,
                    coarse_rebalance: true,
                    inner_convergence: None,
                    theta_repair: true,
                    exp_source: true,
                    source_weighting: openbnct_transport::SourceWeighting::CollapseConsistent,
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
                let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(&dose)?;
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
            UqCommand::Joint {
                dose,
                joint,
                spec,
                pk_model,
                masks,
                id,
                output,
            } => {
                let bundle: PhysicalDoseBundle = openbnct_core::sidecar::load_json(&dose)?;
                bundle
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let input: openbnct_core::JointUncertaintyInput =
                    serde_json::from_slice(&fs::read(&joint)?)?;
                input
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let spec_doc: openbnct_evidence::JointDoseEnsembleSpec =
                    serde_json::from_slice(&fs::read(&spec)?)?;
                spec_doc
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let mask_list = load_named_masks(&masks)?;
                let pk = match (&spec_doc.pk, pk_model.as_ref()) {
                    (Some(pk_spec), Some(path)) => {
                        let model: openbnct_evidence::PkModel =
                            serde_json::from_slice(&fs::read(path)?)?;
                        model
                            .validate()
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        Some(openbnct_evidence::PkIntegration {
                            model_ref: openbnct_core::ContentReference {
                                id: model.id.clone(),
                                sha256: openbnct_evidence::sha256_file(path)?,
                            },
                            model,
                            bindings: pk_spec.bindings.clone(),
                            beam_on_epoch_s: pk_spec.beam_on_epoch_s,
                            beam_time_s: pk_spec.beam_time_s,
                            source_strength_per_s: pk_spec.source_strength_per_s,
                        })
                    }
                    (Some(_), None) => {
                        return Err(io::Error::other(
                            "spec declares pk integration; pass --pk-model",
                        )
                        .into());
                    }
                    (None, Some(_)) => {
                        return Err(io::Error::other(
                            "--pk-model given but the spec declares no pk integration",
                        )
                        .into());
                    }
                    (None, None) => None,
                };
                let bundle_ref = openbnct_core::ContentReference {
                    id: bundle.provenance_id.clone(),
                    sha256: openbnct_evidence::sha256_file(&dose)?,
                };
                let input_ref = openbnct_core::ContentReference {
                    id: input.id.clone(),
                    sha256: openbnct_evidence::sha256_file(&joint)?,
                };
                let report = openbnct_evidence::evaluate_joint_dose_ensemble(
                    &input,
                    &spec_doc.ensemble,
                    &bundle,
                    &bundle_ref,
                    &mask_list,
                    &spec_doc.metrics,
                    &spec_doc.dose_adapters,
                    &spec_doc.attribution_groups,
                    pk.as_ref(),
                    input_ref,
                    &id,
                    &format!("uq-joint:{}", bundle.case_id),
                )
                .map_err(|error| io::Error::other(format!("uq joint: {error}")))?;
                write_new_json(&output, &report)?;
                println!("joint-uncertainty report at {}", output.display());
                println!("method: {}", report.method);
                for metric in &report.metrics {
                    println!(
                        "  {} = {:.6e} ± {:.6e} {} (n={} used, {} failed)",
                        metric.metric,
                        metric.mean,
                        metric.std_dev,
                        metric.unit,
                        metric.successful_realizations,
                        metric.failed_realizations
                    );
                }
            }
            UqCommand::JointInfo { report } => {
                let doc: openbnct_core::JointUncertaintyReport =
                    serde_json::from_slice(&fs::read(&report)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("id: {}", doc.id);
                println!("case_id: {}", doc.case_id);
                println!("method: {}", doc.method);
                println!("seed: {:?}", doc.seed);
                println!("qualification: {}", doc.qualification);
                println!("categories:");
                for c in &doc.categories {
                    println!("  {:?}: {:?}", c.category, c.status);
                }
                println!("metrics:");
                for metric in &doc.metrics {
                    println!(
                        "  {} = {:.6e} ± {:.6e} {} (n={} used, {} failed)",
                        metric.metric,
                        metric.mean,
                        metric.std_dev,
                        metric.unit,
                        metric.successful_realizations,
                        metric.failed_realizations
                    );
                    for quantile in &metric.quantiles {
                        println!("    p{:.2}: {:.6e}", quantile.probability, quantile.value);
                    }
                }
            }
            UqCommand::Voi { spec, output } => {
                let spec_doc: openbnct_core::VoiEvaluationSpec =
                    serde_json::from_slice(&fs::read(&spec)?)?;
                let report = openbnct_core::evaluate_voi(&spec_doc)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &report)?;
                println!("voi report at {}", output.display());
                let mut metric = String::new();
                for u in &report.rankings {
                    if u.metric != metric {
                        metric = u.metric.clone();
                        println!("metric {metric}:");
                    }
                    if u.evaluated {
                        println!(
                            "  {}: ΔVar {:.6e} → post Var {:.6e} (prior {:.6e}){}",
                            u.proposal,
                            u.expected_variance_reduction,
                            u.expected_posterior_variance,
                            u.prior_variance,
                            u.cost_adjusted_utility
                                .map(|c| format!(" — cost-adjusted {c:.6e}"))
                                .unwrap_or_default()
                        );
                    } else {
                        println!(
                            "  {}: not evaluated — {}",
                            u.proposal,
                            u.reason.as_deref().unwrap_or("unspecified")
                        );
                    }
                }
            }
        },
        Some(Command::Bench(args)) => match args.command {
            BenchCommand::Info { catalogue } => {
                let doc: openbnct_evidence::BenchmarkCatalogue =
                    serde_json::from_slice(&fs::read(&catalogue)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("catalogue: {} ({})", doc.id, doc.title);
                println!("qualification: {:?}", doc.qualification);
                println!("entries: {}", doc.entries.len());
                for entry in &doc.entries {
                    println!(
                        "  {:30} {:12} {:?} — {} evidence item(s)",
                        entry.id,
                        entry.modality,
                        entry.status,
                        entry.evidence.len()
                    );
                }
            }
            BenchCommand::Report { catalogue, entry } => {
                let doc: openbnct_evidence::BenchmarkCatalogue =
                    serde_json::from_slice(&fs::read(&catalogue)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let found = doc.entries.iter().find(|e| e.id == entry).ok_or_else(|| {
                    io::Error::other(format!("unknown catalogue entry {entry:?}"))
                })?;
                println!("{}: {}", found.id, found.title);
                println!("  path: {}", found.path);
                println!(
                    "  class: {:?}  modality: {}  status: {:?}",
                    found.problem_class, found.modality, found.status
                );
                if let Some(solver) = &found.solver {
                    println!("  solver: {solver}");
                }
                println!(
                    "  license: {}",
                    found.license.as_deref().unwrap_or("(absent)")
                );
                println!(
                    "  uncertainty: {}",
                    found.uncertainty.as_deref().unwrap_or("(absent)")
                );
                if let Some(normalization) = &found.normalization {
                    println!("  normalization: {normalization}");
                }
                for tolerance in &found.tolerances {
                    println!(
                        "  tolerance: {} — {} ({:?})",
                        tolerance.scope, tolerance.criterion, tolerance.declared
                    );
                }
                for item in &found.evidence {
                    println!(
                        "  evidence: {:?} {:?} — {}",
                        item.kind, item.verdict, item.label
                    );
                    if let Some(region) = &item.region {
                        println!("      region: {region}");
                    }
                    if let Some(origin) = &item.origin {
                        println!("      origin: {origin}");
                    }
                    if let Some(normalization) = &item.normalization {
                        println!("      normalization: {normalization}");
                    }
                    if let Some(file) = &item.file {
                        println!("      file: {file}");
                    }
                    if let Some(note) = &item.note {
                        println!("      note: {note}");
                    }
                }
                for limitation in &found.limitations {
                    println!("  limitation: {limitation}");
                }
            }
            BenchCommand::Verify {
                catalogue,
                root,
                id,
                output,
            } => {
                let doc: openbnct_evidence::BenchmarkCatalogue =
                    serde_json::from_slice(&fs::read(&catalogue)?)?;
                let findings = openbnct_evidence::verify_catalogue(&doc, &root)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let errors = findings
                    .iter()
                    .filter(|f| f.severity == openbnct_evidence::FindingSeverity::Error)
                    .count();
                let warnings = findings.len() - errors;
                println!(
                    "verified {} entries under {}: {} error(s), {} warning(s)",
                    doc.entries.len(),
                    root.display(),
                    errors,
                    warnings
                );
                for finding in &findings {
                    println!(
                        "  {:?} [{}] {}",
                        finding.severity, finding.entry, finding.message
                    );
                }
                if let Some(output) = output {
                    let report = openbnct_evidence::catalogue_report(
                        &doc,
                        openbnct_core::ContentReference {
                            id: doc.id.clone(),
                            sha256: openbnct_evidence::sha256_file(&catalogue)?,
                        },
                        findings,
                        &id.expect("--id is required with --output"),
                        "bench-verify",
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                    write_new_json(&output, &report)?;
                    println!("wrote verification report at {}", output.display());
                }
                if errors > 0 {
                    return Err(io::Error::other(format!(
                        "catalogue verification failed with {errors} error(s)"
                    ))
                    .into());
                }
            }
        },
        Some(Command::Delivery(args)) => match args.command {
            DeliveryCommand::Import { spec, output } => {
                let spec_doc: openbnct_evidence::CsvImportSpec =
                    serde_json::from_slice(&fs::read(&spec)?)?;
                spec_doc
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let base = spec.parent().unwrap_or(Path::new(".")).to_path_buf();
                let delimiter = spec_doc.delimiter.chars().next().unwrap_or(',');
                let comment = spec_doc.comment.chars().next().unwrap_or('#');
                let mut streams = Vec::new();
                for s in &spec_doc.streams {
                    let path = base.join(&s.csv);
                    let text = fs::read_to_string(&path)
                        .map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?;
                    let (stream, diag) =
                        openbnct_evidence::import_stream_csv(s, delimiter, comment, &text)
                            .map_err(|error| io::Error::other(error.to_string()))?;
                    println!(
                        "  {}: {} rows parsed, {} rejected, {} reordered, {} duplicate(s) dropped, {} rollover(s) resolved",
                        diag.stream_id,
                        diag.rows_parsed,
                        diag.rows_rejected.len(),
                        diag.reordered,
                        diag.duplicates_dropped,
                        diag.rollovers_resolved
                    );
                    for (row, reason) in &diag.rows_rejected {
                        println!("    row {row}: {reason}");
                    }
                    for extra in &diag.extra_columns {
                        println!("    extra column ignored: {extra}");
                    }
                    streams.push(stream);
                }
                let history = openbnct_evidence::DeliveryHistory {
                    schema_version: openbnct_evidence::DELIVERY_HISTORY_SCHEMA.into(),
                    id: spec_doc.history_id.clone(),
                    qualification: spec_doc.qualification.clone(),
                    provenance_id: spec_doc.provenance_id.clone(),
                    session_id: spec_doc.session_id.clone(),
                    coordinate_frame: spec_doc.coordinate_frame.clone(),
                    beams: spec_doc.beams.clone(),
                    streams,
                };
                history
                    .validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &history)?;
                println!("wrote delivery history at {}", output.display());
            }
            DeliveryCommand::Info { history, window } => {
                let doc: openbnct_evidence::DeliveryHistory =
                    serde_json::from_slice(&fs::read(&history)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("history: {} (session {})", doc.id, doc.session_id);
                println!("  coordinate frame: {}", doc.coordinate_frame);
                println!("  beams: {}", doc.beams.len());
                for beam in &doc.beams {
                    println!(
                        "    {} cal {} valid [{:.1}, {:.1}] s",
                        beam.id,
                        beam.calibration.id,
                        beam.calibration_valid.0,
                        beam.calibration_valid.1
                    );
                }
                for stream in &doc.streams {
                    println!(
                        "  stream {} ({:?}, {}, clock {}): {} sample(s){}",
                        stream.id,
                        stream.kind,
                        stream.unit,
                        stream.clock.name,
                        stream.samples.len(),
                        stream
                            .available_at_seconds
                            .map(|t| format!(", results at t={t}"))
                            .unwrap_or_default()
                    );
                    if let Ok(rates) = doc.interval_rates(&stream.id) {
                        for r in &rates {
                            println!(
                                "    [{:.1}, {:.1}) s: {:.4} {} ({:?})",
                                r.t_start_s, r.t_end_s, r.rate, stream.unit, r.quality
                            );
                        }
                    }
                    if let Ok(on) = doc.beam_on_intervals(&stream.id) {
                        for (a, b) in &on {
                            match b {
                                Some(e) => println!("    beam on [{:.1}, {:.1}) s", a, e),
                                None => println!("    beam on from {:.1} s (open)", a),
                            }
                        }
                    }
                    if let Some(w) = &window
                        && w.len() == 2
                        && let Ok(gaps) = doc.stream_gaps(&stream.id, w[0], w[1])
                    {
                        for (a, b) in gaps {
                            println!("    GAP [{:.1}, {:.1}) s — no observation", a, b);
                        }
                    }
                }
                if let Some(w) = &window
                    && w.len() == 2
                {
                    for b in doc.expired_calibrations(w[1]) {
                        println!(
                            "  WARNING: beam {} calibration expired before t={:.1} s",
                            b.id, w[1]
                        );
                    }
                }
            }
        },
        Some(Command::Replay(args)) => match args.command {
            ReplayCommand::Run {
                spec,
                history,
                bundle,
                planned,
                bundle_output,
                report_output,
            } => {
                let spec_doc: openbnct_evidence::ReplaySpec =
                    serde_json::from_slice(&fs::read(&spec)?)?;
                let history_doc: openbnct_evidence::DeliveryHistory =
                    serde_json::from_slice(&fs::read(&history)?)?;
                let named: std::collections::BTreeMap<String, PathBuf> = bundle
                    .iter()
                    .map(|entry| {
                        entry
                            .split_once('=')
                            .map(|(k, v)| (k.to_string(), PathBuf::from(v)))
                            .ok_or_else(|| {
                                io::Error::other(format!(
                                    "--bundle wants BEAM_ID=path, got {entry:?}"
                                ))
                            })
                    })
                    .collect::<Result<_, _>>()?;
                let mut bundles: Vec<PhysicalDoseBundle> = Vec::new();
                for beam in &spec_doc.beams {
                    let path = named.get(&beam.beam).ok_or_else(|| {
                        io::Error::other(format!("no --bundle supplied for beam {:?}", beam.beam))
                    })?;
                    bundles.push(openbnct_core::sidecar::load_json::<PhysicalDoseBundle>(
                        path,
                    )?);
                }
                let planned_doc: Option<PhysicalDoseBundle> = planned
                    .as_ref()
                    .map(|p| {
                        let bytes = fs::read(p)?;
                        openbnct_core::sidecar::from_slice_at::<PhysicalDoseBundle>(&bytes, p)
                            .map_err(io::Error::other)
                    })
                    .transpose()?;
                let planned_ref = planned_doc.as_ref().map(|p| (p, 0.0f64));
                let (recon, report) = openbnct_evidence::run_replay(
                    &spec_doc,
                    &history_doc,
                    &bundles.iter().collect::<Vec<_>>(),
                    openbnct_core::ContentReference {
                        id: spec_doc.id.clone(),
                        sha256: String::new(),
                    },
                    planned_ref,
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                // Write the reconstructed bundle, hash it, then rebind the
                // report to its real digest.
                write_new_json(&bundle_output, &recon)?;
                let digest = openbnct_evidence::sha256_file(&bundle_output)?;
                let mut report = report;
                report.reconstructed = openbnct_core::ContentReference {
                    id: format!("{}", bundle_output.display()),
                    sha256: digest,
                };
                write_new_json(&report_output, &report)?;
                println!("reconstructed bundle at {}", bundle_output.display());
                println!("replay report at {}", report_output.display());
                println!(
                    "total reconstructed dose: {:.6} Gy over [{:.1}, {:.1}] s",
                    report.total_dose_gray, report.coverage.0, report.coverage.1
                );
                for beam in &report.beams {
                    println!(
                        "  beam {}: {:.1} s delivered, {} gap(s), {} suspect interval(s), {} expired-calibration span(s)",
                        beam.beam,
                        beam.delivered_seconds,
                        beam.gaps.len(),
                        beam.suspect_intervals.len(),
                        beam.expired_calibration_intervals.len()
                    );
                }
                if !report.coverage_complete {
                    println!("coverage: PARTIAL — delivered intervals have unobserved gaps");
                }
                if !report.biological_equivalence_available {
                    println!(
                        "biological equivalence: not reconstructed (interrupted recorded histories are unsupported)"
                    );
                }
            }
        },
        Some(Command::Outcomes(args)) => match args.command {
            OutcomesCommand::Validate { export } => {
                let doc: openbnct_evidence::OutcomesExport =
                    serde_json::from_slice(&fs::read(&export)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!(
                    "{}: {} participant(s), {} course(s), {} session(s), {} dose record(s), {} observation(s) — valid",
                    doc.id,
                    doc.participants.len(),
                    doc.courses.len(),
                    doc.sessions.len(),
                    doc.dose_records.len(),
                    doc.observations.len()
                );
            }
            OutcomesCommand::Export {
                export,
                whitelist,
                output,
                excluded_output,
            } => {
                let doc: openbnct_evidence::OutcomesExport =
                    serde_json::from_slice(&fs::read(&export)?)?;
                let whitelist: std::collections::BTreeMap<
                    String,
                    std::collections::BTreeSet<String>,
                > = serde_json::from_slice(&fs::read(&whitelist)?)
                    .map_err(|error| io::Error::other(format!("whitelist: {error}")))?;
                let (filtered, excluded) = openbnct_evidence::export_fields(&doc, &whitelist)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                write_new_json(&output, &filtered)?;
                println!("filtered export at {}", output.display());
                println!("excluded: {}", excluded.join(", "));
                if let Some(path) = excluded_output {
                    write_new_json(&path, &excluded)?;
                }
            }
        },
        Some(Command::Qual(args)) => match args.command {
            QualCommand::Info { record } => {
                let doc: openbnct_evidence::QualificationRecord =
                    serde_json::from_slice(&fs::read(&record)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                println!("{} — {} claim(s)", doc.id, doc.claims.len());
                for c in &doc.claims {
                    println!("  [{:?} / {:?}] {}: {}", c.level, c.status, c.id, c.claim);
                }
                for a in &doc.absent_records {
                    println!("  absent: {a}");
                }
            }
            QualCommand::Verify { record, catalogue } => {
                let doc: openbnct_evidence::QualificationRecord =
                    serde_json::from_slice(&fs::read(&record)?)?;
                doc.validate()
                    .map_err(|error| io::Error::other(error.to_string()))?;
                let cat: openbnct_evidence::BenchmarkCatalogue =
                    serde_json::from_slice(&fs::read(&catalogue)?)?;
                let missing = doc
                    .verify_against(&cat)
                    .map_err(|error| io::Error::other(error.to_string()))?;
                if missing.is_empty() {
                    println!(
                        "{}: all {} claim(s) verified against catalogue — no dangling evidence",
                        doc.id,
                        doc.claims.len()
                    );
                } else {
                    for m in &missing {
                        println!("missing: {m}");
                    }
                    return Err(io::Error::other(format!(
                        "{} claim evidence reference(s) unresolved",
                        missing.len()
                    ))
                    .into());
                }
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
                p1,
                anisotropy,
                id,
                output,
                adjoint_flux,
                allow_nonconverged,
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
                    progress: false,
                    quadrature_order: order,
                    convergence,
                    max_inner_iterations: max_inner,
                    max_outer_iterations: max_outer,
                    assignment: assignment_model,
                    periodic: periodic_axes,
                    beam_uncollided_split: true,
                    transport_correction: true,
                    p1_anisotropic: p1,
                    anisotropy_order: anisotropy,
                    anderson_depth: 0,
                    coarse_rebalance: true,
                    inner_convergence: None,
                    theta_repair: true,
                    exp_source: true,
                    source_weighting: openbnct_transport::SourceWeighting::CollapseConsistent,
                };
                let forward = forward_flux
                    .as_ref()
                    .map(|path| {
                        let bytes = fs::read(path)?;
                        let flux: openbnct_transport::MultigroupFlux =
                            openbnct_core::sidecar::from_slice_at(&bytes, path)
                                .map_err(io::Error::other)?;
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
                let unconverged: Vec<usize> = derivation
                    .adjoint_fluxes
                    .iter()
                    .enumerate()
                    .filter(|(_, flux)| !flux.converged)
                    .map(|(index, _)| index)
                    .collect();
                if !unconverged.is_empty() && !allow_nonconverged {
                    return Err(io::Error::other(format!(
                        "adjoint solve for window(s) {unconverged:?} did not converge; raise \
                         --max-outer/--max-inner, or pass --allow-nonconverged to write the \
                         windows anyway"
                    ))
                    .into());
                }
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

/// Resolve a file path declared inside an imported workbook: it must be
/// relative, free of `..`/root components, resolve (after symlinks) to a
/// location inside the workbook's directory, and name a regular file —
/// a received workbook cannot make the import read arbitrary files, or
/// hang on a FIFO or device.
fn confined_workbook_path(base: &Path, declared: &Path) -> io::Result<PathBuf> {
    use std::path::Component;
    let reject =
        |why: &str| io::Error::other(format!("workbook path {}: {why}", declared.display()));
    if declared.is_absolute()
        || declared
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(reject(
            "must be relative to the workbook's directory, without `..` or root components",
        ));
    }
    let root = fs::canonicalize(base)?;
    let target = fs::canonicalize(root.join(declared)).map_err(|e| reject(&e.to_string()))?;
    if !target.starts_with(&root) {
        return Err(reject("resolves outside the workbook's directory"));
    }
    if !fs::metadata(&target)?.is_file() {
        return Err(reject("is not a regular file"));
    }
    Ok(target)
}

/// Compute a dose-volume histogram from a dose bundle file and a mask file
/// and write it to `output` (never overwriting). Shared by `dvh` and the
/// project runner.
fn compute_dvh_file(
    dose: &Path,
    quantity: &str,
    mask: &Path,
    bins: usize,
    output: &Path,
) -> Result<openbnct_evidence::DoseVolumeHistogram, Box<dyn Error>> {
    let dose_bytes = fs::read(dose)?;
    let schema: serde_json::Value = serde_json::from_slice(&dose_bytes)?;
    let mask: RegionMask = serde_json::from_slice(&fs::read(mask)?)?;
    let source = openbnct_core::ContentReference {
        id: dose.display().to_string(),
        sha256: openbnct_evidence::sha256_file(dose)?,
    };
    let dose_schema = openbnct_core::normalize_contract_id(
        schema
            .get("schema_version")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
    );
    let histogram = match dose_schema.as_str() {
        openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
            let bundle: PhysicalDoseBundle =
                openbnct_core::sidecar::from_slice_at(&dose_bytes, dose)?;
            let (values, unit) = dose_values(&bundle, quantity)?;
            let voxel_volume = bundle.geometry.spacing_mm.iter().product();
            openbnct_evidence::DoseVolumeHistogram::compute(
                &bundle.case_id,
                &mask.name,
                quantity,
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
                openbnct_core::sidecar::from_slice_at(&dose_bytes, dose)?;
            let (values, unit) = biological_dose_values(&bundle, quantity)?;
            let voxel_volume = bundle.geometry.spacing_mm.iter().product();
            openbnct_evidence::DoseVolumeHistogram::compute(
                &bundle.case_id,
                &mask.name,
                quantity,
                source,
                unit,
                values,
                &mask.voxels,
                voxel_volume,
                bins,
            )?
        }
        other => {
            return Err(
                io::Error::other(format!("unsupported dose bundle schema {other:?}")).into(),
            );
        }
    };
    let json = serde_json::to_vec_pretty(&histogram)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    file.write_all(&json)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(histogram)
}

/// Compute exact dose-volume metrics from a dose bundle file and a mask
/// file and write them to `output` (never overwriting). Shared by
/// `metrics` and the project runner.
fn compute_metrics_file(
    dose: &Path,
    quantity: &str,
    mask: &Path,
    dx: &[f64],
    vx: &[f64],
    eud: &[f64],
    output: &Path,
) -> Result<openbnct_evidence::RegionDoseMetrics, Box<dyn Error>> {
    let dose_bytes = fs::read(dose)?;
    let bundle = load_dose_bundle(&dose_bytes, dose)?;
    let mask: RegionMask = serde_json::from_slice(&fs::read(mask)?)?;
    let source = openbnct_core::ContentReference {
        id: dose.display().to_string(),
        sha256: openbnct_evidence::sha256_file(dose)?,
    };
    let selection = bundle.select(quantity)?;
    let metrics = openbnct_evidence::RegionDoseMetrics::compute(
        selection.case_id,
        &mask.name,
        quantity,
        source,
        selection.unit,
        selection.values,
        &mask.voxels,
        selection.voxel_volume_mm3,
        dx,
        vx,
        eud,
    )?;
    write_new_json(output, &metrics)?;
    Ok(metrics)
}

/// `beam bind --aim-mask`: re-aim a face-centered bound disk source so the
/// beam axis (`approach`) passes through the mask centroid. The aperture
/// radius is the bound port's; the aimed disk sits on the entry face.
fn aim_bound_source(
    case: &TransportCase,
    mask: &RegionMask,
    approach: &str,
) -> Result<openbnct_transport::FixedSourceDefinition, io::Error> {
    let radius_cm = match &case.source.space {
        openbnct_transport::SourceSpatialDistribution::UniformDisk { radius_cm, .. } => *radius_cm,
        _ => {
            return Err(io::Error::other(
                "--aim-mask needs a beam with a circular port (uniform disk source)",
            ));
        }
    };
    let direction = openbnct_transport::AxisApproach::parse(approach)
        .map_err(|error| io::Error::other(error.to_string()))?
        .unit_vector();
    let (aimed, report) = openbnct_transport::aim_disk_source_at_centroid(
        &case.source,
        &case.geometry,
        mask,
        direction,
        radius_cm,
    )
    .map_err(|error| io::Error::other(format!("beam aim: {error}")))?;
    // Keep the beam's angular distribution: only re-point a cone's axis.
    let mut aimed = aimed;
    if let openbnct_transport::AngularDistribution::IsotropicCone { half_angle_rad, .. } =
        &case.source.angle
    {
        aimed.angle = openbnct_transport::AngularDistribution::IsotropicCone {
            axis_unit_vector: direction,
            half_angle_rad: *half_angle_rad,
        };
    }
    println!(
        "aimed at {:?} centroid LPS [{:.3}, {:.3}, {:.3}] mm from {approach}",
        report.target_region,
        report.target_centroid_lps_mm[0],
        report.target_centroid_lps_mm[1],
        report.target_centroid_lps_mm[2]
    );
    Ok(aimed)
}

fn write_new_json<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let mut out = io::BufWriter::with_capacity(1 << 20, &mut file);
    openbnct_core::sidecar::with_write_context(path, false, || {
        serde_json::to_writer_pretty(&mut out, value)
    })
    .map_err(io::Error::other)??;
    out.write_all(b"\n")?;
    out.flush()?;
    drop(out);
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

/// Fold the flux through the data's tissue-independent ¹⁰B unit response
/// and write the `openbnct.boron-unit-dose/0.1.0` artifact.
fn write_boron_unit_dose(
    path: &Path,
    case: &TransportCase,
    data: &openbnct_transport::MultigroupData,
    flux: &openbnct_transport::MultigroupFlux,
    assignment: Option<&MaterialAssignment>,
    data_bytes: &[u8],
    flux_bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let data_ref = openbnct_core::ContentReference {
        id: data.id.clone(),
        sha256: openbnct_evidence::sha256_hex(data_bytes),
    };
    let flux_ref = openbnct_core::ContentReference {
        id: flux.provenance_id.clone(),
        sha256: openbnct_evidence::sha256_hex(flux_bytes),
    };
    let unit =
        openbnct_transport::fold_boron_unit_dose(case, data, flux, assignment, data_ref, flux_ref)
            .map_err(|error| io::Error::other(format!("boron unit dose: {error}")))?;
    write_new_json(path, &unit)?;
    println!("boron unit dose at {}", path.display());
    Ok(())
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
    Physical(Box<PhysicalDoseBundle>),
    Biological(Box<openbnct_bio::BiologicalDoseBundle>),
}

/// Load a dose bundle whose `schema_version` is a known dose contract.
fn load_dose_bundle(bytes: &[u8], document: &Path) -> Result<DoseBundle, Box<dyn Error>> {
    let schema: serde_json::Value = openbnct_core::sidecar::from_slice_at(bytes, document)?;
    match openbnct_core::normalize_contract_id(
        schema
            .get("schema_version")
            .and_then(|v| v.as_str())
            .unwrap_or_default(),
    )
    .as_str()
    {
        openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => Ok(DoseBundle::Physical(Box::new(
            openbnct_core::sidecar::from_slice_at(bytes, document)?,
        ))),
        openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => Ok(DoseBundle::Biological(Box::new(
            openbnct_core::sidecar::from_slice_at(bytes, document)?,
        ))),
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

/// Split a resolved endpoint map into `(total, boron, unit)` for the
/// PK-aware irradiation-time path: `boron` is the boron component map
/// that the PK curve scales, and `total` is the quantity the limits
/// apply to. `component:boron` scales itself; other `component:*`
/// quantities carry no boron (zero map — the PK solve then reduces to
/// the constant-rate answer).
type ComponentSlice<'a> = (openbnct_core::DoseComponent, &'a [f64], &'a str);

#[allow(clippy::type_complexity)]
fn pk_boron_split<'a>(
    quantity: &str,
    total_values: &'a [f64],
    total_unit: &str,
    components: impl Iterator<Item = ComponentSlice<'a>> + Clone,
) -> Result<(Vec<f64>, Vec<f64>, String), Box<dyn Error>> {
    let find = |component| {
        components
            .clone()
            .find(|(c, _, _)| *c == component)
            .map(|(_, v, u)| (v, u))
            .ok_or_else(|| io::Error::other(format!("bundle lacks {component:?} component")))
    };
    let (boron_values, boron_unit) = find(openbnct_core::DoseComponent::Boron)?;
    Ok(match quantity {
        "component:boron" => (
            boron_values.to_vec(),
            boron_values.to_vec(),
            boron_unit.to_string(),
        ),
        q if q.starts_with("component:") => {
            let component = match &q["component:".len()..] {
                "nitrogen" => openbnct_core::DoseComponent::Nitrogen,
                "hydrogen" => openbnct_core::DoseComponent::Hydrogen,
                "photon" => openbnct_core::DoseComponent::Photon,
                other => {
                    return Err(
                        io::Error::other(format!("unknown dose component {other:?}")).into(),
                    );
                }
            };
            let (values, unit) = find(component)?;
            (values.to_vec(), vec![0.0; values.len()], unit.to_string())
        }
        "physical_total" | "biological_total" => (
            total_values.to_vec(),
            boron_values.to_vec(),
            total_unit.to_string(),
        ),
        other => {
            return Err(io::Error::other(format!(
                "--pk-model needs a total or component:* quantity, not {other:?}"
            ))
            .into());
        }
    })
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

/// Parse a binned spectrum CSV (`e_low_ev,e_high_ev,weight` per row; one
/// optional header line). Edges must be positive and tile contiguously.
fn read_spectrum_csv(path: &Path) -> Result<(Vec<f64>, Vec<f64>), io::Error> {
    let text = fs::read_to_string(path)?;
    let mut edges = Vec::new();
    let mut weights = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let parsed: Option<(f64, f64, f64)> = (fields.len() == 3)
            .then(|| {
                match (
                    fields[0].parse::<f64>(),
                    fields[1].parse::<f64>(),
                    fields[2].parse::<f64>(),
                ) {
                    (Ok(lo), Ok(hi), Ok(w)) => Some((lo, hi, w)),
                    _ => None,
                }
            })
            .flatten();
        let Some((lo, hi, weight)) = parsed else {
            if lineno == 0 {
                continue; // tolerate one header line
            }
            return Err(io::Error::other(format!(
                "{}:{lineno}: expected `e_low_ev,e_high_ev,weight`",
                path.display()
            )));
        };
        if !(lo > 0.0 && hi > lo && weight.is_finite() && weight >= 0.0) {
            return Err(io::Error::other(format!(
                "{}:{lineno}: require 0 < e_low < e_high and weight >= 0",
                path.display()
            )));
        }
        if let Some(last) = edges.last() {
            if (lo - last).abs() > 1e-9 * lo.max(1.0) {
                return Err(io::Error::other(format!(
                    "{}:{lineno}: bin edge {lo} does not continue from {last} \
                     (bins must tile without gaps)",
                    path.display()
                )));
            }
            edges.push(hi);
        } else {
            edges.push(lo);
            edges.push(hi);
        }
        weights.push(weight);
    }
    if weights.is_empty() || weights.iter().all(|w| *w == 0.0) {
        return Err(io::Error::other(format!(
            "{}: no positive-weight bins",
            path.display()
        )));
    }
    Ok((edges, weights))
}

/// `import labelmap`: NIfTI labelmap + materials table → material
/// assignment, with an optional scaffold transport case.
fn cmd_import_labelmap(
    nifti: PathBuf,
    materials: PathBuf,
    case: Option<PathBuf>,
    case_output: Option<PathBuf>,
    case_id: Option<String>,
    output: PathBuf,
) -> Result<(), io::Error> {
    use openbnct_transport::{
        MaterialAssignment, MaterialDefinition, MaterialRegion, MaterialRegionShape,
    };
    use std::collections::BTreeMap;

    if case.is_none() && (case_output.is_none() || case_id.is_none()) {
        return Err(io::Error::other(
            "--case-output and --case-id are required when --case is absent",
        ));
    }
    let image = read_nifti_file(&nifti).map_err(|e| io::Error::other(e.to_string()))?;
    let geometry = &image.geometry;
    let [nx, ny, nz] = geometry.shape;
    let material_table: BTreeMap<String, MaterialDefinition> =
        serde_json::from_slice(&fs::read(&materials)?)
            .map_err(|error| io::Error::other(format!("materials JSON: {error}")))?;

    // Every voxel must be an integer label; group indices by label.
    let mut label_voxels: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for (linear, value) in image.values.iter().enumerate() {
        let rounded = value.round();
        if (value - rounded).abs() > 1e-6 || !(0.0..=(u32::MAX as f64)).contains(&rounded) {
            return Err(io::Error::other(format!(
                "labelmap voxel {linear} is {value} — integer labels required"
            )));
        }
        let label = rounded as u32;
        if label == 0 {
            continue; // background → base material
        }
        let k = linear / (nx as usize * ny as usize);
        let j = (linear / nx as usize) % ny as usize;
        let i = linear % nx as usize;
        label_voxels
            .entry(label)
            .or_default()
            .push([i as u32, j as u32, k as u32]);
    }
    let missing: Vec<u32> = label_voxels
        .keys()
        .filter(|label| !material_table.contains_key(&label.to_string()))
        .copied()
        .collect();
    if !missing.is_empty() {
        return Err(io::Error::other(format!(
            "materials table has no entry for labels {missing:?}"
        )));
    }
    let nz = nz as usize;
    let _ = nz;

    let (case_id, base_material) = match &case {
        Some(path) => {
            let existing: openbnct_transport::TransportCase =
                serde_json::from_slice(&fs::read(path)?)?;
            let case_grid = &existing.geometry;
            let spacing_ok = (0..3).all(|axis| {
                (case_grid.spacing_mm[axis] - geometry.spacing_mm[axis]).abs() < 1e-6
                    && (case_grid.origin_mm[axis] - geometry.origin_mm[axis]).abs() < 1e-3
            });
            if case_grid.shape != geometry.shape || !spacing_ok {
                return Err(io::Error::other(
                    "labelmap grid does not match the bound case geometry",
                ));
            }
            (existing.case_id.clone(), existing.material.clone())
        }
        None => {
            let base = material_table.get("0").cloned().ok_or_else(|| {
                io::Error::other("materials table needs a \"0\" entry for the base material")
            })?;
            (case_id.unwrap(), base)
        }
    };

    let regions: Vec<MaterialRegion> = label_voxels
        .into_iter()
        .map(|(label, indices)| {
            let material = material_table[&label.to_string()].clone();
            MaterialRegion {
                name: material.id.clone(),
                material,
                shape: MaterialRegionShape::VoxelSet { indices },
            }
        })
        .collect();
    let assignment = MaterialAssignment {
        schema_version: openbnct_transport::MATERIAL_ASSIGNMENT_SCHEMA.into(),
        case_id: case_id.clone(),
        base_material,
        regions,
        provenance_id: format!(
            "nifti:sha256:{}",
            openbnct_evidence::sha256_hex(&fs::read(&nifti)?)
        ),
    };
    write_new_json(&output, &assignment)?;
    println!(
        "assignment: {} ({} regions, bound to {case_id})",
        output.display(),
        assignment.regions.len()
    );

    if let Some(case_path) = case_output {
        let scaffold = scaffold_case_from_labelmap(&image, &assignment, &case_id)?;
        write_new_json(&case_path, &scaffold)?;
        println!(
            "scaffold case: {} (edit the source — it is a placeholder)",
            case_path.display()
        );
    }
    Ok(())
}

/// `dicom import-ct`: CT series → covering transport grid, box-averaged HU
/// volume, scaffold case, optional per-ROI masks, and an import record
/// binding every input and output by SHA-256.
#[allow(clippy::too_many_arguments)]
fn cmd_dicom_import_ct(
    series: Option<PathBuf>,
    slices: Vec<PathBuf>,
    rtstruct: Option<PathBuf>,
    spacing_mm: &[f64],
    case_id: String,
    base_material: &Path,
    case_output: &Path,
    hu_output: &Path,
    masks_dir: Option<&Path>,
) -> Result<(), io::Error> {
    if series.is_some() != slices.is_empty() {
        return Err(io::Error::other(
            "exactly one of --series or --slices is required",
        ));
    }
    let mut paths = match &series {
        Some(dir) => openbnct_dicom::collect_study_paths(dir),
        None => slices,
    };
    if let Some(file) = &rtstruct
        && !paths.contains(file)
    {
        paths.push(file.clone());
    }
    let record_path = case_output.with_extension("import-record.json");
    for output in [case_output, hu_output, record_path.as_path()] {
        if output.exists() {
            return Err(io::Error::other(format!(
                "{} already exists; outputs are never overwritten",
                output.display()
            )));
        }
    }
    if let Some(dir) = masks_dir
        && dir.exists()
    {
        return Err(io::Error::other(format!(
            "{} already exists; outputs are never overwritten",
            dir.display()
        )));
    }
    let base: MaterialDefinition = serde_json::from_slice(&fs::read(base_material)?)
        .map_err(|error| io::Error::other(format!("base material JSON: {error}")))?;

    let import = openbnct_dicom::import_ct_contours_from_paths(&paths)
        .map_err(|error| io::Error::other(format!("ct import: {error}")))?;
    let ct = &import.ct;
    if masks_dir.is_some() && import.structures.is_none() {
        return Err(io::Error::other(
            "--masks-dir requested but no RT Structure Set was found",
        ));
    }
    let native = ct.geometry.spacing_mm;
    let target_spacing = match spacing_mm {
        [] => native,
        [s] => [*s; 3],
        [x, y, z] => [*x, *y, *z],
        _ => {
            return Err(io::Error::other(
                "--spacing-mm takes one value or three (x,y,z)",
            ));
        }
    };
    if target_spacing.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(io::Error::other("--spacing-mm must be positive and finite"));
    }
    let grid = covering_grid(&ct.geometry, target_spacing);
    grid.voxel_count().map_err(io::Error::other)?;
    let ct_hu: Vec<f64> = ct
        .stored_pixels
        .iter()
        .map(|&px| ct.modality_value(px))
        .collect();
    let hu = box_average_to_grid(&ct_hu, &ct.geometry, &grid)
        .ok_or_else(|| io::Error::other("internal: grid does not align with the CT lattice"))?;

    write_nifti(
        &NiftiImage {
            geometry: grid.clone(),
            values: hu,
            datatype: DT_FLOAT64,
            transform_source: "sform",
            description: format!("openbnct HU box-mean {}", ct.series_instance_uid),
            intent_name: String::new(),
            units_declared_mm: true,
        },
        hu_output,
    )?;

    let scaffold = scaffold_case_from_geometry(&grid, &base, &case_id);
    write_new_json(case_output, &scaffold)?;

    let mut mask_records = Vec::new();
    if let (Some(dir), Some(structures)) = (masks_dir, &import.structures) {
        fs::create_dir_all(dir)?;
        for roi in &structures.rois {
            let fractions = box_average_to_grid(
                &roi.voxels
                    .iter()
                    .map(|&v| if v { 1.0 } else { 0.0 })
                    .collect::<Vec<_>>(),
                &ct.geometry,
                &grid,
            )
            .ok_or_else(|| io::Error::other("internal: mask grid misaligned"))?;
            let mask = openbnct_core::RegionMask {
                name: roi.name.clone(),
                voxels: fractions.iter().map(|f| *f >= 0.5).collect(),
            };
            let safe: String = roi
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let file = format!("{:03}-{safe}.json", roi.number);
            let path = dir.join(&file);
            write_new_json(&path, &mask)?;
            mask_records.push(serde_json::json!({
                "roi_number": roi.number,
                "name": roi.name,
                "file": file,
                "sha256": openbnct_evidence::sha256_file(&path)?,
                "ct_voxels": roi.voxel_count(),
                "grid_voxels": mask.included_voxel_count(),
            }));
        }
        write_new_json(
            &dir.join("index.json"),
            &serde_json::json!({
                "schema_version": "openbnct.roi-mask-index/0.1.0",
                "case_id": case_id,
                "membership": "voxel is in the ROI when >= 50% of its volume is covered by CT voxels whose centers lie inside the contour polygon",
                "masks": mask_records,
            }),
        )?;
    }

    let record = serde_json::json!({
        "schema_version": "openbnct.ct-import-record/0.1.0",
        "case_id": case_id,
        "ct_series_instance_uid": ct.series_instance_uid,
        "frame_of_reference_uid": ct.frame_of_reference_uid,
        "ct_geometry": ct.geometry,
        "case_geometry": grid,
        "resampling": "hu: volume-weighted box mean of overlapping CT voxels (outside-CT parts excluded); masks: >= 50% volume fraction of CT-grid ROI voxels; no reorientation",
        "inputs": import.members.iter()
            .map(|(name, sha)| serde_json::json!({"file": name, "sha256": sha}))
            .collect::<Vec<_>>(),
        "ignored_inputs": import.ignored,
        "base_material_sha256": openbnct_evidence::sha256_file(base_material)?,
        "outputs": {
            "case": {"file": case_output, "sha256": openbnct_evidence::sha256_file(case_output)?},
            "hu": {"file": hu_output, "sha256": openbnct_evidence::sha256_file(hu_output)?},
        },
        "masks": mask_records,
    });
    write_new_json(&record_path, &record)?;

    println!(
        "ct: {:?} @ {:?} mm -> case grid {:?} @ {:?} mm",
        ct.geometry.shape, native, grid.shape, grid.spacing_mm
    );
    println!("case: {}", case_output.display());
    println!("hu: {}", hu_output.display());
    if let Some(dir) = masks_dir {
        println!("masks: {} ({} ROIs)", dir.display(), mask_records.len());
    }
    println!("record: {}", record_path.display());
    println!(
        "note: the scaffold source is a placeholder — bind a real beam with `openbnct beam bind`"
    );
    Ok(())
}

/// Scaffold transport case for `import labelmap`: labelmap grid, the
/// label-0 material as base, and a mono-thermal disk source on the -z
/// face. The source is an explicit placeholder — users replace it with a
/// real beam via `beam build` + `beam bind`.
fn scaffold_case_from_labelmap(
    image: &openbnct_nifti::NiftiImage,
    assignment: &openbnct_transport::MaterialAssignment,
    case_id: &str,
) -> Result<openbnct_transport::TransportCase, io::Error> {
    Ok(scaffold_case_from_geometry(
        &image.geometry,
        &assignment.base_material,
        case_id,
    ))
}

/// Shared scaffold builder: grid, base material, placeholder source.
fn scaffold_case_from_geometry(
    geometry: &openbnct_core::GridGeometry,
    base_material: &MaterialDefinition,
    case_id: &str,
) -> openbnct_transport::TransportCase {
    use openbnct_transport::{
        AngularDistribution, EnergyDistribution, FixedSourceDefinition, SourceSpatialDistribution,
        TransportCase,
    };
    let radius_cm = 0.05
        * (geometry.shape[0] as f64 * geometry.spacing_mm[0])
            .min(geometry.shape[1] as f64 * geometry.spacing_mm[1]);
    TransportCase {
        schema_version: "openbnct.transport-case/0.1.0".into(),
        case_id: case_id.into(),
        geometry: geometry.clone(),
        material: base_material.clone(),
        source: FixedSourceDefinition {
            schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
            id: format!("{case_id}.scaffold-source"),
            particle: openbnct_transport::ParticleType::Neutron,
            source_sites_per_history: 1,
            statistical_weight_per_site: 1.0,
            space: SourceSpatialDistribution::UniformDisk {
                axis: openbnct_transport::PlaneAxis::Z,
                offset_cm: geometry.origin_mm[2] / 10.0,
                center_uv_cm: [0.0, 0.0],
                radius_cm,
            },
            angle: AngularDistribution::IsotropicCone {
                axis_unit_vector: [0.0, 0.0, 1.0],
                half_angle_rad: 0.15,
            },
            energy: EnergyDistribution::Monoenergetic { energy_ev: 0.0253 },
        },
        requested_histories: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_aim_preserves_the_cone_half_angle() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let beam: openbnct_transport::BeamDescription =
            serde_json::from_slice(&fs::read(root.join("beams/fir1-k63.json")).unwrap()).unwrap();
        let base: MaterialDefinition = serde_json::from_slice(
            &fs::read(root.join("libraries/tissue/materials/air-dry.json")).unwrap(),
        )
        .unwrap();
        let geometry = openbnct_core::GridGeometry {
            shape: [25, 25, 25],
            spacing_mm: [8.0; 3],
            origin_mm: [-96.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        };
        let mut case = scaffold_case_from_geometry(&geometry, &base, "t.aim");
        case.source = beam.bound_source(&geometry).unwrap();
        let mut voxels = vec![false; 25 * 25 * 25];
        voxels[12 + 25 * (12 + 25 * 12)] = true;
        let mask = RegionMask {
            name: "T".into(),
            voxels,
        };
        for (approach, axis) in [("+x", [1.0, 0.0, 0.0]), ("-y", [0.0, -1.0, 0.0])] {
            let aimed = aim_bound_source(&case, &mask, approach).unwrap();
            match aimed.angle {
                openbnct_transport::AngularDistribution::IsotropicCone {
                    axis_unit_vector,
                    half_angle_rad,
                } => {
                    assert_eq!(half_angle_rad, 0.1491);
                    assert_eq!(axis_unit_vector, axis);
                }
                other => panic!("cone became {other:?}"),
            }
        }
    }

    #[test]
    fn workbook_paths_stay_inside_the_workbook_directory() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("plan");
        fs::create_dir_all(base.join("masks")).unwrap();
        fs::write(base.join("masks").join("gtv.nii"), b"x").unwrap();
        fs::write(dir.path().join("outside.nii"), b"x").unwrap();

        let ok = confined_workbook_path(&base, Path::new("masks/gtv.nii")).unwrap();
        assert!(ok.ends_with("masks/gtv.nii"));
        assert!(confined_workbook_path(&base, Path::new("./masks/gtv.nii")).is_ok());

        for bad in ["../outside.nii", "masks/../../outside.nii"] {
            assert!(
                confined_workbook_path(&base, Path::new(bad)).is_err(),
                "{bad}"
            );
        }
        let absolute = dir.path().join("outside.nii");
        assert!(confined_workbook_path(&base, &absolute).is_err());
        // Directories and missing files are not regular files.
        assert!(confined_workbook_path(&base, Path::new("masks")).is_err());
        assert!(confined_workbook_path(&base, Path::new("missing.nii")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("outside.nii"), base.join("link.nii"))
                .unwrap();
            assert!(confined_workbook_path(&base, Path::new("link.nii")).is_err());
        }
    }
}
