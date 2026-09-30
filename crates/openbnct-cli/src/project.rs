// SPDX-License-Identifier: MIT

//! `openbnct project`: the golden path in one command.
//!
//! `project init` writes a self-contained project (a `project.toml` plus
//! copies of the built-in library artifacts it uses, with sha256). `project
//! run` executes the same steps as the individual commands
//! (`dicom import-ct` or `import ct-nifti` -> `dicom calibrate` -> `beam bind` -> `sn solve` ->
//! `boron dose` -> `metrics`/`dvh`) by building each step's equivalent
//! command line, parsing it with the real clap definition and dispatching
//! it to the real handler, so the printed "equivalent command" is exactly
//! what ran. Every step is recorded in `out/run-manifest.json`
//! (`openbnct.project-run/0.1.0`), which makes reruns resumable: a step is
//! skipped when its recorded inputs, command and outputs still hash-match.
//!
//! Research software: nothing here validates a dose for any clinical use.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Schema token of `out/run-manifest.json`.
pub const PROJECT_RUN_SCHEMA: &str = "openbnct.project-run/0.1.0";

/// Schema token of `out/report.json`.
pub const PROJECT_REPORT_SCHEMA: &str = "openbnct.project-report/0.1.0";

mod verify;

type DynResult<T> = Result<T, Box<dyn Error>>;

fn fail<T>(message: impl Into<String>) -> DynResult<T> {
    Err(io::Error::other(message.into()).into())
}

// ---------------------------------------------------------------------------
// CLI surface
// ---------------------------------------------------------------------------

#[derive(Debug, Args)]
pub struct ProjectArgs {
    #[command(subcommand)]
    command: ProjectCommand,
}

#[derive(Debug, Subcommand)]
enum ProjectCommand {
    /// Create a new project directory from a DICOM study (CT + RT Structure
    /// Set) or from a NIfTI CT with a labelmap (`--ct-nifti`): writes
    /// `project.toml` (detected ROI names listed in a comment) and copies
    /// every built-in artifact it uses into `inputs/` with sha256. Refuses
    /// to overwrite an existing directory.
    Init {
        /// Directory holding one CT series and an RT Structure Set.
        #[arg(
            long,
            required_unless_present = "ct_nifti",
            conflicts_with = "ct_nifti"
        )]
        dicom: Option<PathBuf>,
        /// HU-valued NIfTI CT (alternative to `--dicom`); needs
        /// `--labels-nifti` and `--label-names`.
        #[arg(long, requires_all = ["labels_nifti", "label_names"])]
        ct_nifti: Option<PathBuf>,
        /// Integer labelmap NIfTI on the CT grid.
        #[arg(long, requires = "ct_nifti")]
        labels_nifti: Option<PathBuf>,
        /// JSON naming the labelmap's labels (see `import ct-nifti`).
        #[arg(long, requires = "ct_nifti")]
        label_names: Option<PathBuf>,
        /// New project directory (must not exist).
        #[arg(long)]
        output: PathBuf,
        /// RT Structure Set ROI the beam is aimed at (its centroid).
        /// Without it `project.toml` carries a required placeholder.
        #[arg(long)]
        target: Option<String>,
        /// Transport voxel size in mm (isotropic).
        #[arg(long, default_value_t = 5.0)]
        spacing_mm: f64,
        /// Project id used for the case id (default: derived from the
        /// output directory name).
        #[arg(long)]
        id: Option<String>,
    },
    /// Run every step of a project (import, calibrate, beam, transport,
    /// boron, metrics, report), skipping steps whose inputs, options and
    /// outputs are unchanged since the last run.
    Run {
        /// Project directory containing `project.toml`.
        project: PathBuf,
        /// Rerun every step even when up to date.
        #[arg(long)]
        force: bool,
        /// Rerun this step and every later one (`import`, `calibrate`,
        /// `beam`, `transport`, `boron`, `metrics`, `report`, or 1-7).
        #[arg(long, conflicts_with = "force")]
        from: Option<String>,
    },
    /// Re-compute a finished project's dose with continuous-energy OpenMC
    /// on the same case, material assignment, source and boron
    /// concentrations, and report the agreement with the deterministic
    /// (S_N) result per structure and component (step 08, artifacts in
    /// `out/08-verify/`, folded into `out/report.md`). Needs OpenMC 0.16.0
    /// and the ENDF/B-VIII.1 HDF5 library including its `thermal/` tables.
    /// Settings come from flags, then `[verify]` in `project.toml`, then the
    /// environment (`OPENBNCT_OPENMC`, `OPENMC_CROSS_SECTIONS`).
    Verify {
        /// Project directory containing `project.toml` and a completed
        /// `openbnct project run`.
        project: PathBuf,
        /// Source histories, whole number or scientific notation
        /// (default 1e6); rounded up to a multiple of the batch count.
        #[arg(long, value_parser = verify::parse_count)]
        particles: Option<u64>,
        /// OpenMC batches (default 10).
        #[arg(long)]
        batches: Option<u32>,
        /// OpenMP threads for OpenMC (default 2).
        #[arg(long)]
        threads: Option<u32>,
        /// OpenMC executable (else `[verify] openmc`, `OPENBNCT_OPENMC`, or
        /// `openmc` on PATH).
        #[arg(long)]
        openmc: Option<PathBuf>,
        /// Path to the library's `cross_sections.xml` (else `[verify]
        /// cross_sections` or `OPENMC_CROSS_SECTIONS`).
        #[arg(long)]
        cross_sections: Option<PathBuf>,
        /// Wall-clock limit for the OpenMC process in seconds (default
        /// 14400); on expiry it is killed and the step fails.
        #[arg(long)]
        timeout_seconds: Option<u64>,
        /// Rerun even when inputs, options and outputs are unchanged.
        #[arg(long)]
        force: bool,
    },
    /// Print the step table recorded in a project's run manifest.
    Status {
        /// Project directory containing `project.toml`.
        project: PathBuf,
    },
    /// List the built-in artifacts a project can reference as
    /// `builtin:NAME`.
    Builtins,
}

pub fn run_project(args: ProjectArgs) -> DynResult<()> {
    match args.command {
        ProjectCommand::Init {
            dicom,
            ct_nifti,
            labels_nifti,
            label_names,
            output,
            target,
            spacing_mm,
            id,
        } => {
            let source = match (dicom, ct_nifti, labels_nifti, label_names) {
                (Some(dicom), None, None, None) => ImagingSource::Dicom(dicom),
                (None, Some(hu), Some(labels), Some(names)) => {
                    ImagingSource::Nifti { hu, labels, names }
                }
                _ => {
                    return fail(
                        "give --dicom, or --ct-nifti with --labels-nifti and --label-names",
                    );
                }
            };
            init_project(&source, &output, target.as_deref(), spacing_mm, id)
        }
        ProjectCommand::Run {
            project,
            force,
            from,
        } => run_steps(&project, force, from.as_deref()),
        ProjectCommand::Verify {
            project,
            particles,
            batches,
            threads,
            openmc,
            cross_sections,
            timeout_seconds,
            force,
        } => verify::verify_project(
            &project,
            &verify::VerifyOverrides {
                particles,
                batches,
                threads,
                openmc,
                cross_sections,
                timeout_seconds,
                force,
            },
        ),
        ProjectCommand::Status { project } => print_status(&project),
        ProjectCommand::Builtins => {
            for builtin in BUILTINS {
                println!(
                    "builtin:{}  ({}, {} bytes)",
                    builtin.name,
                    builtin.file,
                    builtin.bytes.len()
                );
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in artifacts (embedded so `cargo install` users are self-contained)
// ---------------------------------------------------------------------------

struct Builtin {
    name: &'static str,
    file: &'static str,
    bytes: &'static [u8],
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "tissue/hu-calibration-generic-head-ct",
        file: "hu-calibration-generic-head-ct.json",
        bytes: include_bytes!("../builtins/hu-calibration-generic-head-ct.json"),
    },
    Builtin {
        name: "tissue/multigroup-data-28g-tsl",
        file: "multigroup-data-28g-tsl.json",
        bytes: include_bytes!("../builtins/multigroup-data-28g-tsl.json"),
    },
    Builtin {
        name: "tissue/multigroup-data-28g",
        file: "multigroup-data-28g.json",
        bytes: include_bytes!("../builtins/multigroup-data-28g.json"),
    },
    Builtin {
        name: "tissue/material-air-dry",
        file: "material-air-dry.json",
        bytes: include_bytes!("../builtins/material-air-dry.json"),
    },
    Builtin {
        name: "openmc/response-set-nf-bnct-001",
        file: "openmc-response-set-nf-bnct-001.json",
        bytes: include_bytes!("../builtins/openmc-response-set-nf-bnct-001.json"),
    },
    Builtin {
        name: "openmc/component-profile-unit-mass-fraction",
        file: "openmc-component-profile-unit-mass-fraction.json",
        bytes: include_bytes!("../builtins/openmc-component-profile-unit-mass-fraction.json"),
    },
    Builtin {
        name: "openmc/unit-source-component-profile",
        file: "openmc-unit-source-component-profile.json",
        bytes: include_bytes!("../builtins/openmc-unit-source-component-profile.json"),
    },
    Builtin {
        name: "openmc/unit-source-material",
        file: "openmc-unit-source-material.json",
        bytes: include_bytes!("../builtins/openmc-unit-source-material.json"),
    },
    Builtin {
        name: "openmc/endfb81-base-manifest",
        file: "openmc-endfb81-base-manifest.json",
        bytes: include_bytes!("../builtins/openmc-endfb81-base-manifest.json"),
    },
    Builtin {
        name: "openmc/execution-profile-smoke",
        file: "openmc-execution-profile-smoke.json",
        bytes: include_bytes!("../builtins/openmc-execution-profile-smoke.json"),
    },
    Builtin {
        name: "tissue/multigroup-photon-data-16g",
        file: "multigroup-photon-data-16g.json",
        bytes: include_bytes!("../builtins/multigroup-photon-data-16g.json"),
    },
    Builtin {
        name: "beams/fir1-k63-ineel",
        file: "beam-fir1-k63-ineel.json",
        bytes: include_bytes!("../builtins/beam-fir1-k63-ineel.json"),
    },
    Builtin {
        name: "beams/fir1-k63",
        file: "beam-fir1-k63.json",
        bytes: include_bytes!("../builtins/beam-fir1-k63.json"),
    },
];

fn lookup_builtin(name: &str) -> DynResult<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name).ok_or_else(|| {
        let available: Vec<String> = BUILTINS
            .iter()
            .map(|b| format!("builtin:{}", b.name))
            .collect();
        io::Error::other(format!(
            "unknown builtin \"builtin:{name}\"; available builtins: {}",
            available.join(", ")
        ))
        .into()
    })
}

/// Resolve a `builtin:NAME` or path spec to a path relative to the project
/// directory (or absolute, as written). Builtins are materialized into
/// `inputs/` when the copy is missing; an existing copy is used as-is.
fn resolve_spec(project: &Path, spec: &str) -> DynResult<String> {
    if let Some(name) = spec.strip_prefix("builtin:") {
        let builtin = lookup_builtin(name)?;
        let relative = format!("inputs/{}", builtin.file);
        let target = project.join(&relative);
        if !target.exists() {
            fs::create_dir_all(project.join("inputs"))?;
            fs::write(&target, builtin.bytes)?;
        }
        return Ok(relative);
    }
    let path = project.join(spec);
    if !path.is_file() {
        return fail(format!(
            "{spec}: not a file (paths are relative to the project directory; \
             built-ins are written builtin:NAME, see `openbnct project builtins`)"
        ));
    }
    Ok(spec.to_owned())
}

// ---------------------------------------------------------------------------
// project.toml
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectConfig {
    project: ProjectSection,
    imaging: ImagingSection,
    #[serde(default)]
    materials: MaterialsSection,
    beam: BeamSection,
    #[serde(default)]
    transport: TransportSection,
    boron: BoronSection,
    #[serde(default)]
    report: ReportSection,
    #[serde(default)]
    verify: verify::VerifySection,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectSection {
    id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImagingSection {
    /// DICOM directory (CT series + RT Structure Set), or ...
    #[serde(default)]
    dicom: Option<String>,
    /// ... an HU NIfTI with its labelmap and label names.
    #[serde(default)]
    ct_nifti: Option<String>,
    #[serde(default)]
    labels_nifti: Option<String>,
    #[serde(default)]
    label_names: Option<String>,
    #[serde(default = "default_spacing")]
    spacing_mm: f64,
}

/// Where a project's CT and structures come from.
enum ImagingSource {
    Dicom(PathBuf),
    Nifti {
        hu: PathBuf,
        labels: PathBuf,
        names: PathBuf,
    },
}

fn default_spacing() -> f64 {
    5.0
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialsSection {
    #[serde(default = "default_calibration")]
    calibration: String,
    #[serde(default = "default_multigroup")]
    multigroup_data: String,
    #[serde(default = "default_photon_data")]
    photon_data: String,
    #[serde(default = "default_base_material")]
    base_material: String,
}

impl Default for MaterialsSection {
    fn default() -> Self {
        Self {
            calibration: default_calibration(),
            multigroup_data: default_multigroup(),
            photon_data: default_photon_data(),
            base_material: default_base_material(),
        }
    }
}

fn default_calibration() -> String {
    "builtin:tissue/hu-calibration-generic-head-ct".into()
}
fn default_multigroup() -> String {
    "builtin:tissue/multigroup-data-28g-tsl".into()
}
fn default_photon_data() -> String {
    "builtin:tissue/multigroup-photon-data-16g".into()
}
fn default_base_material() -> String {
    "builtin:tissue/material-air-dry".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BeamSection {
    #[serde(default = "default_beam")]
    description: String,
    target: Option<String>,
    #[serde(default = "default_approach")]
    approach: String,
}

fn default_beam() -> String {
    "builtin:beams/fir1-k63-ineel".into()
}
fn default_approach() -> String {
    "+x".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransportSection {
    #[serde(default = "default_engine")]
    engine: String,
    #[serde(default = "default_order")]
    order: u32,
    #[serde(default = "default_max_outer")]
    max_outer: u32,
    #[serde(default = "default_anderson")]
    anderson: u32,
    #[serde(default)]
    allow_unconverged: bool,
    /// Within-bin spread of a histogram beam spectrum: `uniform_in_bin`
    /// (uniform per eV, the OpenMC/MCNP convention; default) or
    /// `collapse_consistent` (Maxwellian below 0.5 eV, 1/E above).
    #[serde(default = "default_source_weighting")]
    source_weighting: String,
    /// Transport the capture and inelastic photons (`sn photon-solve`) and
    /// use that photon dose (default). `false` keeps the local deposition
    /// of photon energy where it is born.
    #[serde(default = "default_photon_transport")]
    photon_transport: bool,
}

impl Default for TransportSection {
    fn default() -> Self {
        Self {
            engine: default_engine(),
            order: default_order(),
            max_outer: default_max_outer(),
            anderson: default_anderson(),
            allow_unconverged: false,
            source_weighting: default_source_weighting(),
            photon_transport: default_photon_transport(),
        }
    }
}

fn default_source_weighting() -> String {
    "uniform_in_bin".into()
}
fn default_photon_transport() -> bool {
    true
}
fn default_engine() -> String {
    "sn".into()
}
fn default_order() -> u32 {
    8
}
fn default_max_outer() -> u32 {
    128
}
fn default_anderson() -> u32 {
    3
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoronSection {
    blood_ug_g: f64,
    #[serde(default)]
    ratios: BTreeMap<String, f64>,
    #[serde(default = "default_ratio")]
    default_ratio: f64,
}

fn default_ratio() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportSection {
    #[serde(default)]
    structures: Vec<String>,
    source_strength_per_s: Option<f64>,
    irradiation_time_s: Option<f64>,
}

impl ProjectConfig {
    fn parse(text: &str) -> DynResult<Self> {
        let config: Self = toml::from_str(text)
            .map_err(|error| io::Error::other(format!("project.toml: {error}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Checks that need no study data. ROI-name checks run later in
    /// [`Self::validate_rois`].
    fn validate(&self) -> DynResult<()> {
        if self.project.id.trim().is_empty() {
            return fail("project.toml: [project] id must not be empty");
        }
        if !(self.imaging.spacing_mm.is_finite() && self.imaging.spacing_mm > 0.0) {
            return fail("project.toml: [imaging] spacing_mm must be a positive number");
        }
        let imaging = &self.imaging;
        match (
            &imaging.dicom,
            &imaging.ct_nifti,
            &imaging.labels_nifti,
            &imaging.label_names,
        ) {
            (Some(_), None, None, None) | (None, Some(_), Some(_), Some(_)) => {}
            _ => {
                return fail(
                    "project.toml: [imaging] needs either `dicom`, or all of `ct_nifti`, \
                     `labels_nifti` and `label_names` (not both)",
                );
            }
        }
        if self.transport.engine != "sn" {
            return fail(format!(
                "project.toml: [transport] engine {:?} is not supported; only \"sn\" is",
                self.transport.engine
            ));
        }
        if self.transport.order < 2
            || self.transport.order > 16
            || !self.transport.order.is_multiple_of(2)
        {
            return fail("project.toml: [transport] order must be even, 2-16");
        }
        if !matches!(
            self.transport.source_weighting.as_str(),
            "uniform_in_bin" | "collapse_consistent"
        ) {
            return fail(format!(
                "project.toml: [transport] source_weighting {:?} must be \"uniform_in_bin\" or \"collapse_consistent\"",
                self.transport.source_weighting
            ));
        }
        if self.transport.max_outer == 0 {
            return fail("project.toml: [transport] max_outer must be at least 1");
        }
        if self
            .beam
            .target
            .as_deref()
            .is_none_or(|t| t.trim().is_empty())
        {
            return fail(
                "project.toml: [beam] target is required; set it to one of the RT structure \
                 ROI names listed in the comment at the top of project.toml",
            );
        }
        if openbnct_transport::AxisApproach::parse(&self.beam.approach).is_err() {
            return fail(format!(
                "project.toml: [beam] approach {:?} must be one of +x, -x, +y, -y, +z, -z",
                self.beam.approach
            ));
        }
        if !(self.boron.blood_ug_g.is_finite() && self.boron.blood_ug_g >= 0.0) {
            return fail("project.toml: [boron] blood_ug_g must be a non-negative number");
        }
        if !(self.boron.default_ratio.is_finite() && self.boron.default_ratio >= 0.0) {
            return fail("project.toml: [boron] default_ratio must be a non-negative number");
        }
        for (name, ratio) in &self.boron.ratios {
            if !(ratio.is_finite() && *ratio >= 0.0) {
                return fail(format!(
                    "project.toml: [boron] ratios.{name} must be a non-negative number"
                ));
            }
        }
        match (
            self.report.source_strength_per_s,
            self.report.irradiation_time_s,
        ) {
            (None, None) => {}
            (Some(s), Some(t)) if s.is_finite() && s > 0.0 && t.is_finite() && t > 0.0 => {}
            (Some(_), Some(_)) => {
                return fail(
                    "project.toml: [report] source_strength_per_s and irradiation_time_s \
                     must both be positive numbers",
                );
            }
            _ => {
                return fail(
                    "project.toml: [report] give both source_strength_per_s and \
                     irradiation_time_s, or neither",
                );
            }
        }
        self.verify.validate()?;
        // Built-in names are checked up front so a typo fails before any work.
        for spec in [
            &self.materials.calibration,
            &self.materials.multigroup_data,
            &self.materials.photon_data,
            &self.materials.base_material,
            &self.beam.description,
        ] {
            if let Some(name) = spec.strip_prefix("builtin:") {
                lookup_builtin(name)?;
            }
        }
        Ok(())
    }

    /// Cross-check ROI names against the ROIs actually in the study.
    fn validate_rois(&self, available: &[String]) -> DynResult<()> {
        let list = available.join(", ");
        let known: BTreeSet<&str> = available.iter().map(String::as_str).collect();
        let target = self.target();
        if !known.contains(target) {
            return fail(format!(
                "project.toml: [beam] target {target:?} is not an ROI in the study; \
                 available ROIs: {list}"
            ));
        }
        for name in self.boron.ratios.keys() {
            if !known.contains(name.as_str()) {
                return fail(format!(
                    "project.toml: [boron] ratios names {name:?}, which is not an ROI in the \
                     study; available ROIs: {list}"
                ));
            }
        }
        for name in &self.report.structures {
            if !known.contains(name.as_str()) {
                return fail(format!(
                    "project.toml: [report] structures names {name:?}, which is not an ROI in \
                     the study; available ROIs: {list}"
                ));
            }
        }
        Ok(())
    }

    fn target(&self) -> &str {
        self.beam.target.as_deref().unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// init
// ---------------------------------------------------------------------------

fn init_project(
    source: &ImagingSource,
    output: &Path,
    target: Option<&str>,
    spacing_mm: f64,
    id: Option<String>,
) -> DynResult<()> {
    if output.exists() {
        return fail(format!(
            "{} already exists; project init never overwrites an existing directory",
            output.display()
        ));
    }
    if !(spacing_mm.is_finite() && spacing_mm > 0.0) {
        return fail("--spacing-mm must be a positive number");
    }
    let (rois, origin) = match source {
        ImagingSource::Dicom(dicom) => {
            if !dicom.is_dir() {
                return fail(format!("{}: not a directory", dicom.display()));
            }
            let paths = openbnct_dicom::collect_study_paths(dicom);
            let import =
                openbnct_dicom::import_ct_contours_from_paths(&paths).map_err(|error| {
                    io::Error::other(format!("reading {}: {error}", dicom.display()))
                })?;
            let rois: Vec<String> = import
                .structures
                .as_ref()
                .map(|s| s.rois.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default();
            if rois.is_empty() {
                return fail(format!(
                    "{}: no RT Structure Set ROIs found; the project workflow needs one to aim \
                     the beam and report per-structure dose",
                    dicom.display()
                ));
            }
            (rois, dicom.clone())
        }
        ImagingSource::Nifti { hu, labels, names } => {
            for file in [hu, labels, names] {
                if !file.is_file() {
                    return fail(format!("{}: not a file", file.display()));
                }
            }
            let parsed = crate::ct_import::parse_label_names(&fs::read(names)?)
                .map_err(|error| io::Error::other(format!("{}: {error}", names.display())))?;
            (parsed.names(), hu.clone())
        }
    };
    let dicom = &origin;
    if let Some(target) = target
        && !rois.iter().any(|r| r == target)
    {
        return fail(format!(
            "--target {target:?} is not an ROI in the study; available ROIs: {}",
            rois.join(", ")
        ));
    }

    fs::create_dir_all(output)?;
    let project_dir = output.canonicalize()?;
    let imaging_lines = match source {
        ImagingSource::Dicom(_) => {
            let rel = relative_path(&project_dir, &dicom.canonicalize()?);
            format!(
                "dicom = {rel:?}    # directory with one CT series + RTSTRUCT (relative to this directory)\n"
            )
        }
        ImagingSource::Nifti { hu, labels, names } => {
            let rel = |path: &Path| -> DynResult<String> {
                Ok(relative_path(&project_dir, &path.canonicalize()?))
            };
            format!(
                "ct_nifti = {:?}    # HU-valued NIfTI CT (relative to this directory)\n\
                 labels_nifti = {:?}    # integer labelmap on the same grid\n\
                 label_names = {:?}    # JSON naming the labels\n",
                rel(hu)?,
                rel(labels)?,
                rel(names)?
            )
        }
    };
    let id = id.unwrap_or_else(|| {
        let name = project_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "project".into());
        format!("local.{name}")
    });

    // Copy built-ins into inputs/ with sha256 so the project is self-contained.
    let defaults = ProjectConfig::parse(&format!(
        "[project]\nid = {id:?}\n[imaging]\ndicom = \"x\"\n[beam]\ntarget = \"x\"\n[boron]\nblood_ug_g = 0.0\n"
    ))?;
    let mut sums = String::new();
    for spec in [
        &defaults.materials.calibration,
        &defaults.materials.multigroup_data,
        &defaults.materials.photon_data,
        &defaults.materials.base_material,
        &defaults.beam.description,
    ] {
        let relative = resolve_spec(&project_dir, spec)?;
        let sha = openbnct_evidence::sha256_file(&project_dir.join(&relative))?;
        let _ = writeln!(sums, "{sha}  {}", relative.trim_start_matches("inputs/"));
    }
    fs::write(project_dir.join("inputs/SHA256SUMS"), sums)?;

    let ratios = match target {
        Some(t) => format!("{{ {t:?} = 3.5 }}"),
        None => "{}".into(),
    };
    let target_line = match target {
        Some(t) => format!("target = {t:?}"),
        None => "# target = \"<ROI>\"   # REQUIRED: pick one of the ROIs listed at the top".into(),
    };
    let text = format!(
        "# OpenBNCT project (research software; not for clinical use, see docs/DISCLAIMER.md).\n\
         # Run:  openbnct project run <this directory>\n\
         #\n\
         # ROIs detected in the study (RT Structure Set or label names):\n\
         #   {rois}\n\
         \n\
         [project]\n\
         id = {id:?}\n\
         \n\
         [imaging]\n\
         {imaging_lines}\
         spacing_mm = {spacing_mm:?}\n\
         \n\
         [materials]\n\
         calibration = {calibration:?}    # or a path to an openbnct.hu-calibration JSON\n\
         multigroup_data = {mg:?}    # or a path\n\
         photon_data = {ph:?}    # coupled photon data for photon_transport (bound to the neutron group structure above)\n\
         base_material = {air:?}    # material outside the CT (air)\n\
         \n\
         [beam]\n\
         description = {beam:?}    # or a path to an openbnct.beam-description JSON\n\
         {target_line}    # ROI whose centroid the beam axis is aimed at\n\
         approach = \"+x\"    # beam travel direction: +x -x +y -y +z -z (LPS axes)\n\
         \n\
         [transport]\n\
         engine = \"sn\"\n\
         order = 8\n\
         max_outer = 128\n\
         anderson = 3    # Anderson acceleration depth; thermal-scattering data need it to converge\n\
         source_weighting = \"uniform_in_bin\"    # histogram beam bins spread uniformly per eV (OpenMC/MCNP); or \"collapse_consistent\" (1/E above 0.5 eV)\n\
         photon_transport = true    # transport capture photons (sn photon-solve); false = deposit their energy where it is born\n\
         allow_unconverged = false    # true only for demos/tests; the report then says PROVISIONAL if it did not converge\n\
         \n\
         [boron]\n\
         # Illustrative placeholders: replace with concentrations and tissue:blood\n\
         # ratios that belong to your study. Overlapping ROIs: the smaller one wins.\n\
         blood_ug_g = 25.0\n\
         ratios = {ratios}\n\
         default_ratio = 1.0    # tissue:blood ratio for voxels in no listed ROI\n\
         \n\
         [report]\n\
         structures = []    # empty = every ROI\n\
         # Optional delivery normalization (else results stay per source particle):\n\
         # source_strength_per_s = 1.0e12\n\
         # irradiation_time_s = 1800\n",
        rois = rois.join(", "),
        calibration = defaults.materials.calibration,
        mg = defaults.materials.multigroup_data,
        ph = defaults.materials.photon_data,
        air = defaults.materials.base_material,
        beam = defaults.beam.description,
    );
    ProjectConfig::parse(
        &text
            .replace("# target", "target")
            .replace("target = \"<ROI>\"", "target = \"x\""),
    )?;
    fs::write(project_dir.join("project.toml"), text)?;
    println!("project: {}", project_dir.display());
    println!("ROIs: {}", rois.join(", "));
    println!(
        "inputs copied to {}/inputs (sha256 in SHA256SUMS)",
        project_dir.display()
    );
    if target.is_none() {
        println!("next: set [beam] target in project.toml to one of the ROIs above");
    }
    println!("next: openbnct project run {}", output.display());
    Ok(())
}

/// Relative path from directory `from` to `to` (both absolute).
fn relative_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".into());
    }
    for component in &to[common..] {
        parts.push(component.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

// ---------------------------------------------------------------------------
// Run manifest
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileRef {
    path: String,
    sha256: String,
    /// Present when `path` is a directory hashed as a tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepRecord {
    id: String,
    status: String,
    commands: Vec<String>,
    options: Value,
    inputs: Vec<FileRef>,
    outputs: Vec<FileRef>,
    started_utc: String,
    seconds: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunManifest {
    schema_version: String,
    project_id: String,
    openbnct_version: String,
    project_toml_sha256: String,
    steps: Vec<StepRecord>,
}

impl RunManifest {
    fn validate(&self) -> DynResult<()> {
        if !openbnct_core::schema_matches(&self.schema_version, PROJECT_RUN_SCHEMA) {
            return fail(format!(
                "run manifest schema {:?} is not {PROJECT_RUN_SCHEMA:?}",
                self.schema_version
            ));
        }
        let mut last = None;
        for step in &self.steps {
            let Some(index) = STEP_IDS.iter().position(|id| *id == step.id) else {
                return fail(format!("run manifest has unknown step {:?}", step.id));
            };
            if last.is_some_and(|l| l >= index) {
                return fail("run manifest steps are out of order");
            }
            last = Some(index);
            match step.status.as_str() {
                "complete" if step.outputs.is_empty() => {
                    return fail(format!("step {} is complete but lists no outputs", step.id));
                }
                "complete" | "failed" => {}
                other => return fail(format!("step {} has unknown status {other:?}", step.id)),
            }
        }
        Ok(())
    }
}

/// Steps executed by `project run`.
const RUN_STEPS: usize = 7;

/// Every step id a run manifest may record; `08-verify` is written by
/// `project verify`, not by `project run`.
const STEP_IDS: [&str; 8] = [
    "01-import",
    "02-calibrate",
    "03-beam",
    "04-transport",
    "05-boron",
    "06-metrics",
    "07-report",
    verify::VERIFY_STEP_ID,
];

fn step_label(id: &str) -> &str {
    id.split_once('-').map_or(id, |(_, name)| name)
}

fn resolve_step_index(text: &str) -> DynResult<usize> {
    let lowered = text.trim().to_ascii_lowercase();
    for (index, id) in STEP_IDS.iter().enumerate().take(RUN_STEPS) {
        if lowered == *id || lowered == step_label(id) || lowered == (index + 1).to_string() {
            return Ok(index);
        }
    }
    fail(format!(
        "unknown step {text:?}; steps are: {}",
        STEP_IDS
            .iter()
            .take(RUN_STEPS)
            .map(|id| step_label(id))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn manifest_path(project: &Path) -> PathBuf {
    project.join("out/run-manifest.json")
}

fn load_manifest(project: &Path) -> DynResult<Option<RunManifest>> {
    let path = manifest_path(project);
    if !path.exists() {
        return Ok(None);
    }
    let manifest: RunManifest = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
    manifest.validate()?;
    Ok(Some(manifest))
}

fn save_manifest(project: &Path, manifest: &RunManifest) -> DynResult<()> {
    let path = manifest_path(project);
    let temp = path.with_extension("json.tmp");
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    fs::write(&temp, bytes)?;
    fs::rename(&temp, &path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Hashing and file helpers
// ---------------------------------------------------------------------------

/// Hash a file, or a directory as a tree (sha256 over sorted
/// `relative-path<TAB>file-sha256` lines).
fn hash_path(project: &Path, relative: &str) -> DynResult<FileRef> {
    let path = project.join(relative);
    if path.is_dir() {
        let mut files = Vec::new();
        collect_files(&path, &mut files)?;
        files.sort();
        let mut listing = String::new();
        for file in &files {
            let name = file.strip_prefix(&path).unwrap_or(file);
            let _ = writeln!(
                listing,
                "{}\t{}",
                name.to_string_lossy(),
                openbnct_evidence::sha256_file(file)?
            );
        }
        Ok(FileRef {
            path: relative.to_owned(),
            sha256: openbnct_evidence::sha256_hex(listing.as_bytes()),
            files: Some(files.len() as u64),
        })
    } else if path.is_file() {
        Ok(FileRef {
            path: relative.to_owned(),
            sha256: openbnct_evidence::sha256_file(&path)?,
            files: None,
        })
    } else {
        fail(format!("{relative}: input not found"))
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Every file under the given output paths (files or directories), hashed.
fn hash_outputs(project: &Path, roots: &[String]) -> DynResult<Vec<FileRef>> {
    let mut files = Vec::new();
    for root in roots {
        let path = project.join(root);
        if path.is_dir() {
            collect_files(&path, &mut files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    files
        .iter()
        .map(|file| {
            let relative = file
                .strip_prefix(project)
                .unwrap_or(file)
                .to_string_lossy()
                .into_owned();
            Ok(FileRef {
                path: relative,
                sha256: openbnct_evidence::sha256_file(file)?,
                files: None,
            })
        })
        .collect()
}

fn remove_outputs(project: &Path, roots: &[String]) -> io::Result<()> {
    for root in roots {
        let path = project.join(root);
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else if path.is_file() {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_utc(seconds)
}

/// RFC 3339 UTC timestamp from unix seconds (proleptic Gregorian).
fn format_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Shell-quote one command-line token when needed.
fn quote(token: &str) -> String {
    let plain = !token.is_empty()
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./=:+,@%-".contains(c));
    if plain {
        token.to_owned()
    } else {
        format!("'{}'", token.replace('\'', "'\\''"))
    }
}

fn command_line(argv: &[String]) -> String {
    argv.iter().map(|t| quote(t)).collect::<Vec<_>>().join(" ")
}

fn safe_name(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Step planning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RoiEntry {
    name: String,
    file: String,
    voxels: u64,
}

const QUANTITIES: [&str; 5] = [
    "component:boron",
    "component:nitrogen",
    "component:hydrogen",
    "component:photon",
    "physical_total",
];

struct StepPlan {
    commands: Vec<Vec<String>>,
    /// Paths (relative to the project directory) whose content the step consumes.
    inputs: Vec<String>,
    /// Paths the step writes; removed before a rerun, hashed after.
    outputs: Vec<String>,
    options: Value,
}

fn argv(parts: &[&str]) -> Vec<String> {
    std::iter::once("openbnct")
        .chain(parts.iter().copied())
        .map(str::to_owned)
        .collect()
}

fn read_roi_index(project: &Path) -> DynResult<Vec<RoiEntry>> {
    let path = project.join("out/01-import/masks/index.json");
    let value: Value = serde_json::from_slice(
        &fs::read(&path).map_err(|e| io::Error::other(format!("{}: {e}", path.display())))?,
    )?;
    let masks = value["masks"]
        .as_array()
        .ok_or_else(|| io::Error::other("ROI mask index has no `masks` list"))?;
    let mut entries = Vec::new();
    for mask in masks {
        entries.push(RoiEntry {
            name: mask["name"].as_str().unwrap_or_default().to_owned(),
            file: mask["file"].as_str().unwrap_or_default().to_owned(),
            voxels: mask["grid_voxels"].as_u64().unwrap_or(0),
        });
    }
    let mut seen = BTreeSet::new();
    for entry in &entries {
        if !seen.insert(safe_name(&entry.name)) {
            return fail(format!(
                "ROI name {:?} collides with another ROI once made file-safe; rename one in the RT structure set",
                entry.name
            ));
        }
    }
    Ok(entries)
}

fn mask_path(roi: &RoiEntry) -> String {
    format!("out/01-import/masks/{}", roi.file)
}

fn find_roi<'a>(rois: &'a [RoiEntry], name: &str) -> DynResult<&'a RoiEntry> {
    rois.iter().find(|r| r.name == name).ok_or_else(|| {
        io::Error::other(format!(
            "ROI {name:?} not found; available ROIs: {}",
            rois.iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .into()
    })
}

/// Structures reported: `[report] structures`, else every ROI.
fn report_structures<'a>(config: &ProjectConfig, rois: &'a [RoiEntry]) -> Vec<&'a RoiEntry> {
    if config.report.structures.is_empty() {
        rois.iter().collect()
    } else {
        rois.iter()
            .filter(|r| config.report.structures.contains(&r.name))
            .collect()
    }
}

fn plan_step(
    index: usize,
    project: &Path,
    config: &ProjectConfig,
    rois: &[RoiEntry],
) -> DynResult<StepPlan> {
    let calibration = resolve_spec(project, &config.materials.calibration)?;
    let multigroup = resolve_spec(project, &config.materials.multigroup_data)?;
    let photon_data = resolve_spec(project, &config.materials.photon_data)?;
    let base_material = resolve_spec(project, &config.materials.base_material)?;
    let beam = resolve_spec(project, &config.beam.description)?;
    let spacing = config.imaging.spacing_mm.to_string();
    Ok(match index {
        0 => {
            let tail = [
                "--spacing-mm",
                &spacing,
                "--case-id",
                &config.project.id,
                "--base-material",
                &base_material,
                "--case-output",
                "out/01-import/case.json",
                "--hu-output",
                "out/01-import/hu.nii",
                "--masks-dir",
                "out/01-import/masks",
            ];
            let imaging = &config.imaging;
            let (mut command, mut inputs) = match (
                &imaging.dicom,
                &imaging.ct_nifti,
                &imaging.labels_nifti,
                &imaging.label_names,
            ) {
                (Some(dicom), ..) => (
                    argv(&["dicom", "import-ct", "--series", dicom]),
                    vec![dicom.clone()],
                ),
                (None, Some(hu), Some(labels), Some(names)) => (
                    argv(&[
                        "import",
                        "ct-nifti",
                        "--hu",
                        hu,
                        "--labels",
                        labels,
                        "--label-names",
                        names,
                    ]),
                    vec![hu.clone(), labels.clone(), names.clone()],
                ),
                _ => return fail("project.toml: [imaging] names no CT source"),
            };
            command.extend(tail.iter().map(|part| (*part).to_owned()));
            inputs.push(base_material);
            StepPlan {
                commands: vec![command],
                inputs,
                outputs: vec!["out/01-import".into()],
                options: json!({}),
            }
        }
        1 => StepPlan {
            commands: vec![argv(&[
                "dicom",
                "calibrate",
                "--calibration",
                &calibration,
                "--hu-nifti",
                "out/01-import/hu.nii",
                "--case",
                "out/01-import/case.json",
                "--output",
                "out/02-calibrate/assignment.json",
                "--report",
                "out/02-calibrate/report.json",
            ])],
            inputs: vec![
                calibration,
                "out/01-import/hu.nii".into(),
                "out/01-import/case.json".into(),
            ],
            outputs: vec!["out/02-calibrate".into()],
            options: json!({}),
        },
        2 => {
            let target = mask_path(find_roi(rois, config.target())?);
            let approach = format!("--approach={}", config.beam.approach);
            StepPlan {
                commands: vec![argv(&[
                    "beam",
                    "bind",
                    "--beam",
                    &beam,
                    "--case",
                    "out/01-import/case.json",
                    "--output",
                    "out/03-beam/case.json",
                    "--aim-mask",
                    &target,
                    &approach,
                ])],
                inputs: vec![beam, "out/01-import/case.json".into(), target],
                outputs: vec!["out/03-beam".into()],
                options: json!({}),
            }
        }
        3 => {
            let order = config.transport.order.to_string();
            let max_outer = config.transport.max_outer.to_string();
            let anderson = config.transport.anderson.to_string();
            let mut command = argv(&[
                "sn",
                "solve",
                "--case",
                "out/03-beam/case.json",
                "--data",
                &multigroup,
                "--assignment",
                "out/02-calibrate/assignment.json",
                "--order",
                &order,
                "--max-outer",
                &max_outer,
                "--anderson",
                &anderson,
                "--source-weighting",
                &config.transport.source_weighting,
            ]);
            if config.transport.allow_unconverged {
                command.push("--allow-unconverged".into());
            }
            // With photon transport the solve's own dose keeps the local
            // capture-gamma kerma; the merged bundle replaces that photon
            // component and is the one later steps read (`dose.json`).
            let neutron_dose = if config.transport.photon_transport {
                "out/04-transport/dose-local-photon.json"
            } else {
                "out/04-transport/dose.json"
            };
            command.extend(
                [
                    "--dose",
                    neutron_dose,
                    "--boron-unit-output",
                    "out/04-transport/boron-unit.json",
                    "--output",
                    "out/04-transport/flux.json",
                ]
                .map(str::to_owned),
            );
            let mut commands = vec![command];
            let mut inputs = vec![
                "out/03-beam/case.json".to_owned(),
                multigroup,
                "out/02-calibrate/assignment.json".to_owned(),
            ];
            if config.transport.photon_transport {
                commands.push(argv(&[
                    "sn",
                    "photon-solve",
                    "--case",
                    "out/03-beam/case.json",
                    "--photon-data",
                    &photon_data,
                    "--neutron-flux",
                    "out/04-transport/flux.json",
                    "--assignment",
                    "out/02-calibrate/assignment.json",
                    "--order",
                    &order,
                    "--dose",
                    "out/04-transport/dose-photon.json",
                    "--output",
                    "out/04-transport/photon-flux.json",
                ]));
                commands.push(argv(&[
                    "sn",
                    "merge-photon-dose",
                    "--neutron-dose",
                    "out/04-transport/dose-local-photon.json",
                    "--photon-dose",
                    "out/04-transport/dose-photon.json",
                    "--output",
                    "out/04-transport/dose.json",
                ]));
                inputs.push(photon_data);
            }
            StepPlan {
                commands,
                inputs,
                outputs: vec!["out/04-transport".into()],
                options: json!({
                    "source_weighting": config.transport.source_weighting,
                    "photon_transport": config.transport.photon_transport,
                }),
            }
        }
        4 => {
            // Smaller ROIs first: `boron dose` lets the first matching mask
            // win per voxel, so a specific structure beats an enclosing one.
            let mut listed: Vec<&RoiEntry> = config
                .boron
                .ratios
                .keys()
                .map(|name| find_roi(rois, name))
                .collect::<DynResult<_>>()?;
            listed.sort_by(|a, b| a.voxels.cmp(&b.voxels).then_with(|| a.name.cmp(&b.name)));
            let blood = config.boron.blood_ug_g.to_string();
            let default_ratio = config.boron.default_ratio.to_string();
            let mut command = argv(&[
                "boron",
                "dose",
                "--physical-bundle",
                "out/04-transport/dose.json",
                "--unit-dose",
                "out/04-transport/boron-unit.json",
                "--blood-ug-g",
                &blood,
                "--default-ratio",
                &default_ratio,
            ]);
            let mut inputs = vec![
                "out/04-transport/dose.json".to_owned(),
                "out/04-transport/boron-unit.json".to_owned(),
            ];
            for roi in &listed {
                command.push("--ratio".into());
                command.push(format!("{}={}", roi.name, config.boron.ratios[&roi.name]));
                command.push("--mask".into());
                command.push(format!("{}={}", roi.name, mask_path(roi)));
                inputs.push(mask_path(roi));
            }
            command.extend(["--output".to_owned(), "out/05-boron/dose.json".to_owned()]);
            StepPlan {
                commands: vec![command],
                inputs,
                outputs: vec!["out/05-boron".into()],
                options: json!({}),
            }
        }
        5 => {
            let mut commands = Vec::new();
            let mut inputs = vec!["out/05-boron/dose.json".to_owned()];
            for roi in report_structures(config, rois) {
                let mask = mask_path(roi);
                inputs.push(mask.clone());
                let stem = safe_name(&roi.name);
                for quantity in QUANTITIES {
                    let output = format!(
                        "out/06-metrics/{stem}.{}.metrics.json",
                        quantity.replace(':', "-")
                    );
                    commands.push(argv(&[
                        "metrics",
                        "--dose",
                        "out/05-boron/dose.json",
                        "--quantity",
                        quantity,
                        "--mask",
                        &mask,
                        "--dx",
                        "95,50,2",
                        "--output",
                        &output,
                    ]));
                }
                commands.push(argv(&[
                    "dvh",
                    "--dose",
                    "out/05-boron/dose.json",
                    "--quantity",
                    "physical_total",
                    "--mask",
                    &mask,
                    "--bins",
                    "100",
                    "--output",
                    &format!("out/06-metrics/{stem}.physical_total.dvh.json"),
                ]));
            }
            StepPlan {
                commands,
                inputs,
                outputs: vec!["out/06-metrics".into()],
                options: json!({}),
            }
        }
        6 => StepPlan {
            commands: Vec::new(),
            inputs: {
                let mut inputs = vec!["out/06-metrics".into(), "out/04-transport/flux.json".into()];
                // The report folds a verification in when one exists.
                if project.join("out/08-verify/verification.json").is_file() {
                    inputs.push("out/08-verify/verification.json".into());
                }
                inputs
            },
            outputs: vec![
                "out/report.md".into(),
                "out/report.json".into(),
                "out/dvh".into(),
            ],
            options: json!({
                "project_id": config.project.id,
                "structures": report_structures(config, rois).iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
                "source_strength_per_s": config.report.source_strength_per_s,
                "irradiation_time_s": config.report.irradiation_time_s,
                "allow_unconverged": config.transport.allow_unconverged,
            }),
        },
        _ => unreachable!("step index out of range"),
    })
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Restores the working directory when dropped.
struct CwdGuard(PathBuf);

impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

/// Parse one equivalent command line with the real clap definition and
/// dispatch it to the real handler.
fn execute_command(command: &[String]) -> DynResult<()> {
    let cli = super::Cli::try_parse_from(command).map_err(|error| {
        io::Error::other(format!("internal: step command did not parse: {error}"))
    })?;
    match cli.command {
        Some(super::Command::Metrics {
            dose,
            quantity,
            mask,
            dx,
            vx,
            eud,
            output,
        }) => {
            super::compute_metrics_file(&dose, &quantity, &mask, &dx, &vx, &eud, &output)?;
            Ok(())
        }
        Some(super::Command::Dvh {
            dose,
            quantity,
            mask,
            bins,
            output,
        }) => {
            super::compute_dvh_file(&dose, &quantity, &mask, bins, &output)?;
            Ok(())
        }
        _ => super::run(cli),
    }
}

fn load_config(project: &Path) -> DynResult<(ProjectConfig, String)> {
    let path = project.join("project.toml");
    let text = fs::read_to_string(&path)
        .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
    let config = ProjectConfig::parse(&text)?;
    Ok((config, openbnct_evidence::sha256_hex(text.as_bytes())))
}

fn step_is_current(
    project: &Path,
    record: Option<&StepRecord>,
    plan: &StepPlan,
    commands: &[String],
) -> DynResult<bool> {
    let Some(record) = record else {
        return Ok(false);
    };
    if record.status != "complete" || record.commands != commands || record.options != plan.options
    {
        return Ok(false);
    }
    let mut inputs = Vec::new();
    for path in &plan.inputs {
        match hash_path(project, path) {
            Ok(hashed) => inputs.push(hashed),
            Err(_) => return Ok(false),
        }
    }
    if inputs != record.inputs {
        return Ok(false);
    }
    Ok(hash_outputs(project, &plan.outputs)? == record.outputs)
}

fn run_steps(project_arg: &Path, force: bool, from: Option<&str>) -> DynResult<()> {
    let project = project_arg
        .canonicalize()
        .map_err(|error| io::Error::other(format!("{}: {error}", project_arg.display())))?;
    let (config, config_sha) = load_config(&project)?;
    let from_index = match from {
        Some(text) => resolve_step_index(text)?,
        None if force => 0,
        None => RUN_STEPS,
    };
    // Fail on unresolvable inputs before any work.
    for spec in [
        &config.materials.calibration,
        &config.materials.multigroup_data,
        &config.materials.photon_data,
        &config.materials.base_material,
        &config.beam.description,
    ] {
        resolve_spec(&project, spec)?;
    }
    if let Some(dicom) = &config.imaging.dicom {
        if !project.join(dicom).is_dir() {
            return fail(format!(
                "project.toml: [imaging] dicom {dicom:?} is not a directory (relative to {})",
                project.display()
            ));
        }
    } else {
        for (key, value) in [
            ("ct_nifti", &config.imaging.ct_nifti),
            ("labels_nifti", &config.imaging.labels_nifti),
            ("label_names", &config.imaging.label_names),
        ] {
            let value = value.as_deref().unwrap_or_default();
            if !project.join(value).is_file() {
                return fail(format!(
                    "project.toml: [imaging] {key} {value:?} is not a file (relative to {})",
                    project.display()
                ));
            }
        }
    }

    let _cwd = CwdGuard(std::env::current_dir()?);
    std::env::set_current_dir(&project)?;
    fs::create_dir_all("out")?;

    let mut manifest = load_manifest(&project)?.unwrap_or_else(|| RunManifest {
        schema_version: PROJECT_RUN_SCHEMA.into(),
        project_id: config.project.id.clone(),
        openbnct_version: env!("CARGO_PKG_VERSION").into(),
        project_toml_sha256: config_sha.clone(),
        steps: Vec::new(),
    });
    manifest.project_id = config.project.id.clone();
    manifest.openbnct_version = env!("CARGO_PKG_VERSION").into();
    manifest.project_toml_sha256 = config_sha;

    println!("project {} ({})", config.project.id, project.display());
    let mut rois: Vec<RoiEntry> = Vec::new();
    for (index, id) in STEP_IDS.iter().copied().enumerate().take(RUN_STEPS) {
        if index == 1 {
            rois = read_roi_index(&project)?;
            let names: Vec<String> = rois.iter().map(|r| r.name.clone()).collect();
            config.validate_rois(&names)?;
        }
        let plan = plan_step(index, &project, &config, &rois)?;
        let commands: Vec<String> = plan.commands.iter().map(|c| command_line(c)).collect();
        let position = manifest.steps.iter().position(|s| s.id == id);
        let current = index < from_index
            && step_is_current(
                &project,
                position.map(|p| &manifest.steps[p]),
                &plan,
                &commands,
            )?;
        let tag = format!("[{}/{}] {:<10}", index + 1, RUN_STEPS, step_label(id));
        if current {
            println!("{tag} skipped (inputs, options and outputs unchanged)");
            continue;
        }

        println!("{tag} running");
        remove_outputs(&project, &plan.outputs)?;
        if index < 6 {
            fs::create_dir_all(project.join("out").join(id))?;
        }
        let started_utc = utc_now();
        let timer = Instant::now();
        let mut inputs = Vec::new();
        for path in &plan.inputs {
            inputs.push(hash_path(&project, path)?);
        }
        let outcome: DynResult<()> = (|| {
            if index == 6 {
                return write_report(&project, &config, &manifest, &rois);
            }
            // `sn solve` prints per-outer-iteration progress by default.
            for command in &plan.commands {
                execute_command(command)?;
            }
            Ok(())
        })();
        let seconds = timer.elapsed().as_secs_f64();
        let mut record = StepRecord {
            id: id.into(),
            status: "complete".into(),
            commands,
            options: plan.options.clone(),
            inputs,
            outputs: Vec::new(),
            started_utc,
            seconds,
            error: None,
        };
        match outcome {
            Ok(()) => {
                record.outputs = hash_outputs(&project, &plan.outputs)?;
                println!("{tag} done in {seconds:.1} s");
            }
            Err(error) => {
                record.status = "failed".into();
                record.error = Some(error.to_string());
            }
        }
        let failed = record.status == "failed";
        match position {
            Some(p) => manifest.steps[p] = record,
            None => manifest.steps.push(record),
        }
        if failed {
            manifest
                .steps
                .truncate(position.map_or(manifest.steps.len(), |p| p + 1));
            save_manifest(&project, &manifest)?;
            let message = manifest
                .steps
                .last()
                .and_then(|s| s.error.clone())
                .unwrap_or_default();
            println!("{tag} FAILED after {seconds:.1} s");
            return fail(format!("step {id} failed: {message}"));
        }
        save_manifest(&project, &manifest)?;
    }
    println!("report: {}", project.join("out/report.md").display());
    Ok(())
}

fn print_status(project_arg: &Path) -> DynResult<()> {
    let project = project_arg
        .canonicalize()
        .map_err(|error| io::Error::other(format!("{}: {error}", project_arg.display())))?;
    let Some(manifest) = load_manifest(&project)? else {
        println!(
            "no run manifest yet; run `openbnct project run {}`",
            project_arg.display()
        );
        return Ok(());
    };
    println!(
        "project {} (openbnct {}, manifest {})",
        manifest.project_id, manifest.openbnct_version, manifest.schema_version
    );
    println!(
        "{:<14} {:<18} {:>8}  {:>6} {:>7}  started",
        "step", "status", "seconds", "inputs", "outputs"
    );
    for id in STEP_IDS {
        match manifest.steps.iter().find(|s| s.id == id) {
            Some(step) => {
                let roots: Vec<String> = step.outputs.iter().map(|o| o.path.clone()).collect();
                let intact = hash_outputs(&project, &roots)? == step.outputs;
                let status = if step.status == "complete" && !intact {
                    "outputs modified"
                } else {
                    step.status.as_str()
                };
                println!(
                    "{:<14} {:<18} {:>8.1}  {:>6} {:>7}  {}",
                    id,
                    status,
                    step.seconds,
                    step.inputs.len(),
                    step.outputs.len(),
                    step.started_utc
                );
                if let Some(error) = &step.error {
                    println!("               error: {error}");
                }
            }
            None => println!("{id:<14} {:<18}", "not run"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Current, measured accuracy status of the default deterministic path —
/// printed in every report so absolute numbers are never read in isolation.
const ACCURACY_STATUS: &str = "Accuracy status: on the synthetic layered-head benchmark, with \
the default settings (histogram beam bins uniform per eV, transported photons), deterministic \
(S_N) structure-mean boron, hydrogen (fast-neutron) and photon doses are within about 15% of \
continuous-energy OpenMC with matching S(alpha,beta) (whole phantom 0.86 / 0.98 / 0.93, target \
1.06 / 0.90 / 1.03 at 5e6 histories). Projects that set photon_transport = false deposit \
capture-gamma energy where it is born, which over-predicts the photon dose ~3-4x. One synthetic \
geometry is not a validation of your study: treat absolute and biologically weighted totals as \
research estimates and run `openbnct project verify` for an independent Monte Carlo check.";

const DISCLAIMER: &str = "Research software output. OpenBNCT is not a medical device and has \
not been clinically validated or commissioned for any treatment facility; these results are \
not for diagnosis, treatment planning, or any clinical decision. See docs/DISCLAIMER.md.";

fn fmt_dose(value: f64) -> String {
    format!("{value:.4e}")
}

fn write_report(
    project: &Path,
    config: &ProjectConfig,
    manifest: &RunManifest,
    rois: &[RoiEntry],
) -> DynResult<()> {
    let flux: Value =
        serde_json::from_slice(&fs::read(project.join("out/04-transport/flux.json"))?)?;
    let converged = flux["converged"].as_bool().unwrap_or(false);
    let residual = flux["residual"].as_f64().unwrap_or(f64::NAN);
    let outer = flux["outer_iterations"].as_u64().unwrap_or(0);
    let status = if converged {
        "converged".to_owned()
    } else {
        format!(
            "PROVISIONAL (transport field not converged: residual {residual:.3e} after {outer} outer iterations)"
        )
    };

    let scale = match (
        config.report.source_strength_per_s,
        config.report.irradiation_time_s,
    ) {
        (Some(s), Some(t)) => Some((s, t, s * t)),
        _ => None,
    };
    let (factor, unit) = match scale {
        Some((_, _, f)) => (f, "Gy"),
        None => (1.0, "Gy per source particle"),
    };

    struct Row {
        quantity: String,
        mean: f64,
        max: f64,
        d95: f64,
        d50: f64,
        d2: f64,
    }
    let mut structures_json = Vec::new();
    let mut structure_tables = Vec::new();
    fs::create_dir_all(project.join("out/dvh"))?;
    for roi in report_structures(config, rois) {
        let stem = safe_name(&roi.name);
        let mut rows = Vec::new();
        let mut voxels = 0;
        for quantity in QUANTITIES {
            let path = project.join(format!(
                "out/06-metrics/{stem}.{}.metrics.json",
                quantity.replace(':', "-")
            ));
            let metrics: openbnct_evidence::RegionDoseMetrics =
                serde_json::from_slice(&fs::read(&path)?)?;
            voxels = metrics.region_voxel_count;
            let dx = |percent: f64| {
                metrics
                    .dx
                    .iter()
                    .find(|m| (m.percent - percent).abs() < 1e-9)
                    .map_or(f64::NAN, |m| m.dose)
            };
            rows.push(Row {
                quantity: quantity.to_owned(),
                mean: metrics.mean_dose * factor,
                max: metrics.maximum_dose * factor,
                d95: dx(95.0) * factor,
                d50: dx(50.0) * factor,
                d2: dx(2.0) * factor,
            });
        }
        // DVH CSV (physical total).
        let dvh: openbnct_evidence::DoseVolumeHistogram = serde_json::from_slice(&fs::read(
            project.join(format!("out/06-metrics/{stem}.physical_total.dvh.json")),
        )?)?;
        // Header carries the unit; cumulative_volume_fraction is the
        // fraction of the structure receiving at least that dose.
        let unit_slug = unit.to_ascii_lowercase().replace(' ', "_");
        let mut csv = format!("dose_{unit_slug},cumulative_volume_fraction\n");
        for (edge, volume) in dvh.dose_edges.iter().zip(&dvh.cumulative_volume_fraction) {
            let _ = writeln!(csv, "{:e},{volume}", edge * factor);
        }
        let csv_path = format!("out/dvh/{stem}.csv");
        fs::write(project.join(&csv_path), csv)?;

        let mut table = format!(
            "### {} ({voxels} voxels)\n\n| quantity | mean | max | D95 | D50 | D2 |\n|---|---|---|---|---|---|\n",
            roi.name
        );
        let mut quantities_json = Vec::new();
        for row in &rows {
            let _ = writeln!(
                table,
                "| {} | {} | {} | {} | {} | {} |",
                row.quantity,
                fmt_dose(row.mean),
                fmt_dose(row.max),
                fmt_dose(row.d95),
                fmt_dose(row.d50),
                fmt_dose(row.d2)
            );
            quantities_json.push(json!({
                "quantity": row.quantity,
                "mean": row.mean, "max": row.max,
                "d95": row.d95, "d50": row.d50, "d2": row.d2,
            }));
        }
        let _ = write!(table, "\nDVH (physical total): `{csv_path}`\n");
        structure_tables.push(table);
        structures_json.push(json!({
            "name": roi.name,
            "voxels": voxels,
            "dvh_csv": csv_path,
            "quantities": quantities_json,
        }));
    }

    // Input hashes: every step input that is not a produced artifact.
    let mut input_hashes: BTreeMap<String, String> = BTreeMap::new();
    for step in &manifest.steps {
        for input in &step.inputs {
            if !input.path.starts_with("out/") {
                input_hashes.insert(input.path.clone(), input.sha256.clone());
            }
        }
    }
    let verify_state = verify::load_state(project, manifest)?;
    let commands: Vec<(String, Vec<String>)> = manifest
        .steps
        .iter()
        .filter(|s| !s.commands.is_empty())
        .filter(|s| {
            s.id != verify::VERIFY_STEP_ID || matches!(verify_state, verify::VerifyState::Fresh(_))
        })
        .map(|s| (s.id.clone(), s.commands.clone()))
        .collect();

    let photon_treatment = if config.transport.photon_transport {
        "transported: capture photons solved with `sn photon-solve` (16-group coupled photon data); \
         the neutron solve's local capture-gamma kerma is replaced by the transported photon dose"
    } else {
        "local deposition: capture-gamma energy is deposited where it is born (no photon transport; \
         over-predicts photon dose in finite heads)"
    };
    let generated = utc_now();
    let mut md = String::new();
    let _ = writeln!(
        md,
        "# OpenBNCT project report: {}{}\n",
        config.project.id,
        if converged { "" } else { " (PROVISIONAL)" }
    );
    let _ = writeln!(md, "> {DISCLAIMER}\n");
    let _ = writeln!(
        md,
        "> {ACCURACY_STATUS}{}\n",
        verify::accuracy_suffix(&verify_state)
    );
    let _ = writeln!(md, "| | |\n|---|---|");
    let _ = writeln!(md, "| Project id | {} |", config.project.id);
    let _ = writeln!(md, "| OpenBNCT version | {} |", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(md, "| Generated (UTC) | {generated} |");
    let _ = writeln!(md, "| Transport status | {status} |");
    let _ = writeln!(
        md,
        "| Transport | S{} discrete ordinates, {} outer iterations, residual {residual:.3e} |",
        config.transport.order, outer
    );
    let _ = writeln!(
        md,
        "| Beam spectrum bins | spread {} within each histogram bin |",
        if config.transport.source_weighting == "uniform_in_bin" {
            "uniformly per eV (OpenMC/MCNP convention)"
        } else {
            "as Maxwellian below 0.5 eV and 1/E above (collapse-consistent)"
        }
    );
    let _ = writeln!(md, "| Photon treatment | {photon_treatment} |");
    let _ = writeln!(
        md,
        "| Target / approach | {} / {} |",
        config.target(),
        config.beam.approach
    );
    let _ = writeln!(
        md,
        "| Boron | blood {} ug/g; tissue:blood ratios {}; default ratio {} |",
        config.boron.blood_ug_g,
        if config.boron.ratios.is_empty() {
            "none".to_owned()
        } else {
            config
                .boron
                .ratios
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        },
        config.boron.default_ratio
    );
    let _ = match scale {
        Some((s, t, _)) => writeln!(
            md,
            "| Dose unit | Gy (per-source-particle dose x {s:e} particles/s x {t} s) |"
        ),
        None => writeln!(
            md,
            "| Dose unit | Gy per source particle (no delivery normalization given) |"
        ),
    };
    let _ = writeln!(md, "\n## Input hashes (sha256)\n");
    for (path, sha) in &input_hashes {
        let _ = writeln!(md, "- `{path}`: `{sha}`");
    }
    let _ = writeln!(md, "\n## Dose by structure ({unit})\n");
    let _ = writeln!(
        md,
        "D95, D50, D2 are the doses received by at least 95 %, 50 % and 2 % of the structure's volume. Physical total includes the boron dose scaled by the concentrations above.\n"
    );
    for table in &structure_tables {
        let _ = writeln!(md, "{table}");
    }
    let _ = writeln!(md, "{}", verify::render_section(&verify_state));
    let _ = writeln!(md, "## Commands\n");
    let _ = writeln!(
        md,
        "Run from the project directory to reproduce or modify a single step by hand. The report step is internal (it reads `out/06-metrics`); lines starting with `#` in the verify step are internal sub-steps.\n"
    );
    for (id, lines) in &commands {
        let _ = writeln!(md, "{id}:\n\n```text");
        for line in lines {
            let _ = writeln!(md, "{line}");
        }
        let _ = writeln!(md, "```\n");
    }
    let _ = writeln!(md, "{DISCLAIMER}");
    fs::write(project.join("out/report.md"), md)?;

    let report = json!({
        "schema_version": PROJECT_REPORT_SCHEMA,
        "project_id": config.project.id,
        "openbnct_version": env!("CARGO_PKG_VERSION"),
        "generated_utc": generated,
        "converged": converged,
        "provisional": !converged,
        "transport_status": status,
        "source_weighting": config.transport.source_weighting,
        "photon_transport": config.transport.photon_transport,
        "photon_treatment": photon_treatment,
        "residual": residual,
        "outer_iterations": outer,
        "dose_unit": unit,
        "delivery_normalization": scale.map(|(s, t, f)| json!({
            "source_strength_per_s": s, "irradiation_time_s": t, "factor": f
        })),
        "input_hashes": input_hashes,
        "structures": structures_json,
        "independent_mc_check": verify::report_json(&verify_state),
        "commands": commands.iter().map(|(id, lines)| json!({"step": id, "commands": lines})).collect::<Vec<_>>(),
        "disclaimer": DISCLAIMER,
    });
    let mut bytes = serde_json::to_vec_pretty(&report)?;
    bytes.push(b'\n');
    fs::write(project.join("out/report.json"), bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "[project]\nid = \"t.p1\"\n[imaging]\ndicom = \"../study\"\n\
        [beam]\ntarget = \"GTV\"\n[boron]\nblood_ug_g = 25.0\nratios = { GTV = 3.5 }\n";

    fn rois() -> Vec<String> {
        vec!["GTV".into(), "SKIN".into(), "BRAIN".into()]
    }

    #[test]
    fn minimal_config_parses_with_defaults() {
        let config = ProjectConfig::parse(MINIMAL).unwrap();
        assert_eq!(config.transport.order, 8);
        assert_eq!(config.transport.max_outer, 128);
        assert_eq!(config.transport.anderson, 3);
        assert!(!config.transport.allow_unconverged);
        assert_eq!(config.beam.approach, "+x");
        config.validate_rois(&rois()).unwrap();
    }

    #[test]
    fn missing_target_is_a_clear_error() {
        let text = MINIMAL.replace("target = \"GTV\"\n", "");
        let error = ProjectConfig::parse(&text).unwrap_err().to_string();
        assert!(error.contains("[beam] target is required"), "{error}");
    }

    #[test]
    fn unknown_roi_in_ratios_lists_available_rois() {
        let text = MINIMAL.replace("{ GTV = 3.5 }", "{ GTV = 3.5, SKUN = 1.5 }");
        let config = ProjectConfig::parse(&text).unwrap();
        let error = config.validate_rois(&rois()).unwrap_err().to_string();
        assert!(error.contains("\"SKUN\""), "{error}");
        assert!(
            error.contains("available ROIs: GTV, SKIN, BRAIN"),
            "{error}"
        );
    }

    #[test]
    fn unknown_target_lists_available_rois() {
        let config =
            ProjectConfig::parse(&MINIMAL.replace("\"GTV\"\n[boron]", "\"CTV\"\n[boron]")).unwrap();
        let error = config.validate_rois(&rois()).unwrap_err().to_string();
        assert!(
            error.contains("available ROIs: GTV, SKIN, BRAIN"),
            "{error}"
        );
    }

    #[test]
    fn builtin_typo_lists_available_builtins() {
        let text = format!(
            "{MINIMAL}[materials]\ncalibration = \"builtin:tissue/hu-calibration-generic-head\"\n"
        );
        let error = ProjectConfig::parse(&text).unwrap_err().to_string();
        assert!(error.contains("unknown builtin"), "{error}");
        assert!(
            error.contains("builtin:tissue/hu-calibration-generic-head-ct"),
            "{error}"
        );
        assert!(error.contains("builtin:beams/fir1-k63"), "{error}");
    }

    #[test]
    fn unknown_keys_and_bad_values_are_rejected() {
        let error = ProjectConfig::parse(&format!("{MINIMAL}[report]\nstructurez = []\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("structurez"), "{error}");
        let error = ProjectConfig::parse(&format!("{MINIMAL}[transport]\norder = 5\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("order must be even"), "{error}");
        let error = ProjectConfig::parse(&format!(
            "{MINIMAL}[report]\nsource_strength_per_s = 1e12\n"
        ))
        .unwrap_err()
        .to_string();
        assert!(error.contains("both"), "{error}");
        let error =
            ProjectConfig::parse(&MINIMAL.replace("blood_ug_g = 25.0", "blood_ug_g = -1.0"))
                .unwrap_err()
                .to_string();
        assert!(error.contains("blood_ug_g"), "{error}");
        let error = ProjectConfig::parse(&format!("{MINIMAL}[transport]\nengine = \"openmc\"\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("only \"sn\""), "{error}");
    }

    #[test]
    fn embedded_builtins_match_the_committed_libraries() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let sources = [
            (
                "tissue/hu-calibration-generic-head-ct",
                "libraries/tissue/hu-calibration-generic-head-ct.json",
            ),
            (
                "tissue/multigroup-data-28g-tsl",
                "libraries/tissue/multigroup-data-28g-tsl.json",
            ),
            (
                "tissue/multigroup-data-28g",
                "libraries/tissue/multigroup-data-28g.json",
            ),
            (
                "tissue/material-air-dry",
                "libraries/tissue/materials/air-dry.json",
            ),
            ("beams/fir1-k63", "beams/fir1-k63.json"),
            ("beams/fir1-k63-ineel", "beams/fir1-k63-ineel-20mev.json"),
            (
                "tissue/multigroup-photon-data-16g",
                "libraries/tissue/multigroup-photon-data-16g.json",
            ),
            (
                "openmc/response-set-nf-bnct-001",
                "benchmarks/synthetic/nf-bnct-001/transport/provenance/neutron-response-set.json",
            ),
            (
                "openmc/component-profile-unit-mass-fraction",
                "examples/openmc-multimaterial/component-profile-unit-mass-fraction.json",
            ),
            (
                "openmc/unit-source-component-profile",
                "benchmarks/synthetic/nf-bnct-001/transport/component-profile.json",
            ),
            (
                "openmc/unit-source-material",
                "benchmarks/synthetic/nf-bnct-001/transport/material.json",
            ),
            (
                "openmc/endfb81-base-manifest",
                "benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json",
            ),
            (
                "openmc/execution-profile-smoke",
                "benchmarks/synthetic/nf-bnct-001/transport/openmc-smoke-profile.json",
            ),
        ];
        assert_eq!(sources.len(), BUILTINS.len());
        for (name, file) in sources {
            let builtin = lookup_builtin(name).unwrap();
            assert_eq!(
                builtin.bytes,
                fs::read(root.join(file)).unwrap().as_slice(),
                "builtin:{name} drifted from {file}; recopy it into crates/openbnct-cli/builtins/"
            );
        }
    }

    #[test]
    fn helpers_behave() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_709_210_096), "2024-02-29T12:34:56Z");
        assert_eq!(quote("out/a.json"), "out/a.json");
        assert_eq!(quote("--approach=+x"), "--approach=+x");
        assert_eq!(quote("GTV=out/a b.json"), "'GTV=out/a b.json'");
        assert_eq!(
            relative_path(Path::new("/a/b/p001"), Path::new("/a/b/study")),
            "../study"
        );
        assert_eq!(
            relative_path(Path::new("/a/b"), Path::new("/a/b/c/d")),
            "c/d"
        );
        assert_eq!(resolve_step_index("transport").unwrap(), 3);
        assert_eq!(resolve_step_index("5").unwrap(), 4);
        assert!(
            resolve_step_index("nope")
                .unwrap_err()
                .to_string()
                .contains("steps are")
        );
    }

    #[test]
    fn manifest_validation_rejects_bad_schema_and_order() {
        let mut manifest = RunManifest {
            schema_version: PROJECT_RUN_SCHEMA.into(),
            project_id: "p".into(),
            openbnct_version: "0".into(),
            project_toml_sha256: String::new(),
            steps: Vec::new(),
        };
        manifest.validate().unwrap();
        manifest.schema_version = "openbnct.project-run/9.9.9".into();
        assert!(manifest.validate().is_err());
    }
}
