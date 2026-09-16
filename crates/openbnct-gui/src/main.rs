// SPDX-License-Identifier: Apache-2.0

#![forbid(unsafe_code)]

mod brand;
mod help;

use std::array;
use std::path::{Path, PathBuf};

use eframe::egui;
use openbnct_bio::{BiologicalDoseBundle, RegionMask};
use openbnct_core::PhysicalDoseBundle;
use openbnct_dicom::{VerifiedBenchmarkCase, load_nf_bnct_001};
use openbnct_evidence::{
    DoseVolumeHistogram, EvidenceBundleManifest, RegionDoseMetrics, sha256_hex,
};
use openbnct_openmc::{OpenMcBackend, TARGET_OPENMC_VERSION};
use openbnct_transport::TransportBackend;
use openbnct_view::{AnatomicalPlane, Crosshair, PatientAlignedGrid, SliceView};

use help::{GuidedHelp, HelpWorkspace, TourTarget, TourTargets};

const OPENMC_MANIFEST_EVIDENCE: &[u8] = include_bytes!(
    "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json"
);
const NJOY_EXECUTION_EVIDENCE: &[u8] = include_bytes!(
    "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-execution-receipt.json"
);
const HEATING_COMPARISON_EVIDENCE: &[u8] = include_bytes!(
    "../../../benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-njoy-mt301-comparison.json"
);

// Debug-only native captures for visual review; no effect in normal launches.
#[cfg(debug_assertions)]
fn capture_preview(context: &egui::Context) {
    let Ok(path) = std::env::var("OPENBNCT_CAPTURE") else {
        return;
    };
    let screenshot = context.input(|input| {
        input.events.iter().find_map(|event| {
            if let egui::Event::Screenshot { image, .. } = event {
                Some(image.clone())
            } else {
                None
            }
        })
    });
    if let Some(image) = screenshot {
        let bytes: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect();
        image::save_buffer(
            &path,
            &bytes,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ColorType::Rgba8,
        )
        .expect("save native preview");
        context.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    if context.cumulative_frame_nr() == 3 {
        context.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
    }
    context.request_repaint();
}

fn main() -> eframe::Result {
    let initial_case = std::env::args_os().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1_440.0, 900.0])
            .with_min_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "OpenBNCT",
        options,
        Box::new(move |creation_context| {
            configure_style(&creation_context.egui_ctx);
            Ok(Box::new(OpenBnctApp::new(
                initial_case,
                &creation_context.egui_ctx,
            )))
        }),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum WorkspaceTab {
    #[default]
    Overview,
    Geometry,
    Transport,
    Plan,
    Dose,
    Evidence,
}

impl WorkspaceTab {
    const ALL: [Self; 6] = [
        Self::Overview,
        Self::Geometry,
        Self::Transport,
        Self::Plan,
        Self::Dose,
        Self::Evidence,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Geometry => "Geometry",
            Self::Transport => "Transport",
            Self::Plan => "Plan",
            Self::Dose => "Dose components",
            Self::Evidence => "Evidence",
        }
    }

    const fn marker(self) -> &'static str {
        match self {
            Self::Overview => "01",
            Self::Geometry => "02",
            Self::Transport => "03",
            Self::Plan => "04",
            Self::Dose => "05",
            Self::Evidence => "06",
        }
    }
}

impl From<WorkspaceTab> for HelpWorkspace {
    fn from(workspace: WorkspaceTab) -> Self {
        match workspace {
            WorkspaceTab::Overview => Self::Overview,
            WorkspaceTab::Geometry => Self::Geometry,
            WorkspaceTab::Transport => Self::Transport,
            WorkspaceTab::Plan => Self::Plan,
            WorkspaceTab::Dose => Self::Dose,
            WorkspaceTab::Evidence => Self::Evidence,
        }
    }
}

impl From<HelpWorkspace> for WorkspaceTab {
    fn from(workspace: HelpWorkspace) -> Self {
        match workspace {
            HelpWorkspace::Overview => Self::Overview,
            HelpWorkspace::Geometry => Self::Geometry,
            HelpWorkspace::Transport => Self::Transport,
            HelpWorkspace::Plan => Self::Plan,
            HelpWorkspace::Dose => Self::Dose,
            HelpWorkspace::Evidence => Self::Evidence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GateState {
    Verified,
    Frozen,
    Blocked,
    Pending,
    InputRequired,
}

impl GateState {
    const fn label(self) -> &'static str {
        match self {
            Self::Verified => "VERIFIED",
            Self::Frozen => "FROZEN",
            Self::Blocked => "BLOCKED",
            Self::Pending => "PENDING",
            Self::InputRequired => "INPUT REQUIRED",
        }
    }

    fn color(self, dark: bool) -> egui::Color32 {
        if !dark {
            return match self {
                Self::Verified => egui::Color32::from_rgb(24, 113, 83),
                Self::Frozen => egui::Color32::from_rgb(44, 93, 156),
                Self::Blocked => egui::Color32::from_rgb(151, 88, 20),
                Self::Pending => egui::Color32::from_rgb(96, 107, 120),
                Self::InputRequired => egui::Color32::from_rgb(166, 58, 58),
            };
        }
        match self {
            Self::Verified => egui::Color32::from_rgb(90, 210, 153),
            Self::Frozen => egui::Color32::from_rgb(91, 166, 255),
            Self::Blocked => egui::Color32::from_rgb(244, 171, 67),
            Self::Pending => egui::Color32::from_rgb(151, 158, 178),
            Self::InputRequired => egui::Color32::from_rgb(214, 117, 117),
        }
    }
}

/// Resolved once per frame from `dark_mode`; every surface and text color
/// that would otherwise be a hardcoded dark-theme literal routes through here.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Theme {
    banner_fill: egui::Color32,
    nav_fill: egui::Color32,
    panel_fill: egui::Color32,
    card_fill: egui::Color32,
    card_alt_fill: egui::Color32,
    brand: egui::Color32,
    text_dim: egui::Color32,
    error: egui::Color32,
    warn_text: egui::Color32,
}

impl Theme {
    fn resolve(dark: bool) -> Self {
        if dark {
            Self {
                banner_fill: egui::Color32::from_rgb(21, 30, 40),
                nav_fill: egui::Color32::from_rgb(15, 19, 27),
                panel_fill: egui::Color32::from_rgb(17, 21, 29),
                card_fill: egui::Color32::from_rgb(22, 30, 41),
                card_alt_fill: egui::Color32::from_rgb(36, 26, 46),
                brand: egui::Color32::from_rgb(172, 166, 255),
                text_dim: egui::Color32::from_rgb(150, 160, 180),
                error: egui::Color32::LIGHT_RED,
                warn_text: egui::Color32::from_rgb(244, 188, 95),
            }
        } else {
            Self {
                banner_fill: egui::Color32::from_rgb(255, 255, 255),
                nav_fill: egui::Color32::from_rgb(248, 250, 249),
                panel_fill: egui::Color32::from_rgb(244, 247, 246),
                card_fill: egui::Color32::WHITE,
                card_alt_fill: egui::Color32::from_rgb(245, 241, 248),
                brand: egui::Color32::from_rgb(24, 0, 173),
                text_dim: egui::Color32::from_rgb(92, 101, 118),
                error: egui::Color32::from_rgb(178, 34, 34),
                warn_text: egui::Color32::from_rgb(146, 84, 6),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadinessGate {
    title: &'static str,
    detail: &'static str,
    state: GateState,
}

fn readiness_gates(case_loaded: bool) -> [ReadinessGate; 5] {
    [
        ReadinessGate {
            title: "DICOM geometry",
            detail: if case_loaded {
                "Case artifacts and patient-space geometry passed the runtime gate."
            } else {
                "Load NF-BNCT-001 to run the DICOM and integrity gate."
            },
            state: if case_loaded {
                GateState::Verified
            } else {
                GateState::InputRequired
            },
        },
        ReadinessGate {
            title: "Material and source",
            detail: "Versioned NF-BNCT-001 benchmark contracts are checked in.",
            state: GateState::Frozen,
        },
        ReadinessGate {
            title: "OpenMC nuclear data",
            detail: "Official case selection and 16 artifact identities are frozen.",
            state: GateState::Frozen,
        },
        ReadinessGate {
            title: "Component responses",
            detail: "O-17/O-18 transported-photon treatment requires independent review.",
            state: GateState::Blocked,
        },
        ReadinessGate {
            title: "Controlled transport run",
            detail: "Disabled until every upstream scientific gate passes.",
            state: GateState::Pending,
        },
    ]
}

/// A loaded dose artifact — physical or biological — with the same contract
/// validation the CLI enforces. Weighted bundles stay visually distinct.
enum DoseArtifact {
    Physical(PhysicalDoseBundle),
    Biological(BiologicalDoseBundle),
}

struct LoadedDose {
    artifact: DoseArtifact,
    sha256: String,
}

/// (component label, values, sigma) rows for display.
type DoseRows<'a> = Vec<(String, &'a [f64], Option<&'a [f64]>)>;

impl DoseArtifact {
    fn load(path: &Path) -> Result<LoadedDose, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let sha256 = sha256_hex(&bytes);
        let schema: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let artifact = match schema
            .get("schema_version")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
        {
            openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                let bundle: PhysicalDoseBundle =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                bundle.validate().map_err(|error| error.to_string())?;
                Self::Physical(bundle)
            }
            openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
                let bundle: BiologicalDoseBundle =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                bundle.validate().map_err(|error| error.to_string())?;
                Self::Biological(bundle)
            }
            other => return Err(format!("unsupported dose bundle schema {other:?}")),
        };
        Ok(LoadedDose { artifact, sha256 })
    }

    fn case_id(&self) -> &str {
        match self {
            Self::Physical(bundle) => &bundle.case_id,
            Self::Biological(bundle) => &bundle.case_id,
        }
    }

    fn geometry(&self) -> &openbnct_core::GridGeometry {
        match self {
            Self::Physical(bundle) => &bundle.geometry,
            Self::Biological(bundle) => &bundle.geometry,
        }
    }

    fn unit(&self) -> &str {
        match self {
            Self::Physical(bundle) => match bundle.components.first() {
                Some(volume) => match volume.unit {
                    openbnct_core::DoseUnit::Gray => "gray",
                    openbnct_core::DoseUnit::GrayPerSourceParticle => "gray_per_source_particle",
                },
                None => "unknown",
            },
            Self::Biological(bundle) => &bundle.unit,
        }
    }

    fn rows(&self) -> DoseRows<'_> {
        match self {
            Self::Physical(bundle) => {
                let mut rows: DoseRows<'_> = bundle
                    .components
                    .iter()
                    .map(|volume| {
                        (
                            serde_json::to_value(volume.component)
                                .and_then(|v| {
                                    v.as_str().map(str::to_owned).ok_or(serde_json::Error::io(
                                        std::io::Error::other("component not a string"),
                                    ))
                                })
                                .unwrap_or_else(|_| "unknown".into()),
                            volume.values.as_slice(),
                            volume.absolute_standard_uncertainty.as_deref(),
                        )
                    })
                    .collect();
                rows.push((
                    "physical_total".into(),
                    bundle.physical_total.values.as_slice(),
                    bundle
                        .physical_total
                        .absolute_standard_uncertainty
                        .as_deref(),
                ));
                rows
            }
            Self::Biological(bundle) => {
                let mut rows: DoseRows<'_> = bundle
                    .components
                    .iter()
                    .map(|volume| {
                        (
                            serde_json::to_value(volume.component)
                                .and_then(|v| {
                                    v.as_str().map(str::to_owned).ok_or(serde_json::Error::io(
                                        std::io::Error::other("component not a string"),
                                    ))
                                })
                                .unwrap_or_else(|_| "unknown".into()),
                            volume.values.as_slice(),
                            volume.absolute_standard_uncertainty.as_deref(),
                        )
                    })
                    .collect();
                rows.push((
                    "biological_total".into(),
                    bundle.total.values.as_slice(),
                    bundle.total.absolute_standard_uncertainty.as_deref(),
                ));
                rows
            }
        }
    }

    fn qualification(&self) -> &'static str {
        match self {
            Self::Physical(_) => "synthetic_research_only",
            Self::Biological(bundle) => match bundle.qualification.as_str() {
                "synthetic_research_only_not_clinical" => "synthetic_research_only_not_clinical",
                _ => "unqualified",
            },
        }
    }
}

/// Resolved dose wash for one render pass: selected quantity values on the
/// case grid, the field maximum used for color normalization, and the
/// absolute display floor derived from the threshold percentage.
#[derive(Clone, Copy)]
struct DoseWash<'a> {
    values: &'a [f64],
    max: f64,
    floor: f64,
    opacity: f32,
    unit: &'a str,
}

/// Optional dose wash over the verified case image. The loaded bundle must
/// declare the same case id and an equivalent grid — the overlay indexes
/// voxels directly, so a mismatched grid or foreign case is rejected.
struct DoseOverlay {
    path: String,
    loaded: Option<LoadedDose>,
    error: Option<String>,
    quantity: String,
    enabled: bool,
    opacity: f32,
    threshold_percent: f32,
}

impl Default for DoseOverlay {
    fn default() -> Self {
        Self {
            path: String::new(),
            loaded: None,
            error: None,
            quantity: String::new(),
            enabled: true,
            opacity: 0.55,
            threshold_percent: 10.0,
        }
    }
}

impl DoseOverlay {
    fn load(&mut self, case: &VerifiedBenchmarkCase) {
        let outcome = DoseArtifact::load(Path::new(self.path.trim())).and_then(|dose| {
            if dose.artifact.case_id() != case.report.case_id {
                return Err(format!(
                    "dose case_id {:?} does not match this case {:?}",
                    dose.artifact.case_id(),
                    case.report.case_id
                ));
            }
            if !openbnct_core::grid_geometry_equivalent(dose.artifact.geometry(), &case.ct.geometry)
            {
                return Err("dose grid geometry does not match this case".into());
            }
            Ok(dose)
        });
        match outcome {
            Ok(dose) => {
                self.error = None;
                self.quantity = match &dose.artifact {
                    DoseArtifact::Physical(_) => "physical_total".into(),
                    DoseArtifact::Biological(_) => "biological_total".into(),
                };
                self.loaded = Some(dose);
            }
            Err(error) => {
                self.loaded = None;
                self.error = Some(error);
            }
        }
    }

    fn wash(&self) -> Option<DoseWash<'_>> {
        if !self.enabled {
            return None;
        }
        let loaded = self.loaded.as_ref()?;
        let values = loaded
            .artifact
            .rows()
            .into_iter()
            .find(|(name, ..)| {
                *name == self.quantity || format!("component:{name}") == self.quantity
            })
            .map(|(_, values, _)| values)?;
        let max = values.iter().copied().fold(0.0_f64, f64::max);
        if !max.is_finite() || max <= 0.0 {
            return None;
        }
        Some(DoseWash {
            values,
            max,
            floor: f64::from(self.threshold_percent) / 100.0 * max,
            opacity: self.opacity,
            unit: loaded.artifact.unit(),
        })
    }
}

/// UI state for the dose workspace: the loaded bundle plus the region mask
/// and quantity chosen for the DVH panel.
struct DosePanel {
    bundle_path: String,
    bundle: Option<LoadedDose>,
    bundle_error: Option<String>,
    mask_path: String,
    quantity: String,
    histogram: Option<DoseVolumeHistogram>,
    histogram_error: Option<String>,
    metrics_dx: String,
    metrics_vx: String,
    metrics_eud: String,
    metrics: Option<RegionDoseMetrics>,
    metrics_error: Option<String>,
    metrics_save_path: String,
    metrics_status: Option<String>,
}

impl Default for DosePanel {
    fn default() -> Self {
        Self {
            bundle_path: String::new(),
            bundle: None,
            bundle_error: None,
            mask_path: String::new(),
            quantity: String::new(),
            histogram: None,
            histogram_error: None,
            metrics_dx: "98,50,2".into(),
            metrics_vx: String::new(),
            metrics_eud: String::new(),
            metrics: None,
            metrics_error: None,
            metrics_save_path: String::new(),
            metrics_status: None,
        }
    }
}

impl DosePanel {
    fn load_bundle(&mut self) {
        match DoseArtifact::load(Path::new(self.bundle_path.trim())) {
            Ok(bundle) => {
                self.bundle_error = None;
                self.histogram = None;
                self.quantity = match &bundle.artifact {
                    DoseArtifact::Physical(_) => "physical_total".into(),
                    DoseArtifact::Biological(_) => "biological_total".into(),
                };
                self.bundle = Some(bundle);
            }
            Err(error) => {
                self.bundle = None;
                self.bundle_error = Some(error);
            }
        }
    }

    /// Resolve the mask + quantity row selection shared by the DVH and
    /// metrics overlays. Returns the mask, quantity label, and the selected
    /// values (cloned so callers can mutate other panel fields freely).
    fn resolve_selection(&self) -> Result<(RegionMask, String, Vec<f64>), String> {
        let bundle = self.bundle.as_ref().ok_or("Load a dose bundle first.")?;
        let mask: RegionMask = std::fs::read(self.mask_path.trim())
            .map_err(|e| e.to_string())
            .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
            .map_err(|e| format!("mask: {e}"))?;
        let quantity = self.quantity.trim().to_owned();
        let values = bundle
            .artifact
            .rows()
            .into_iter()
            .find(|(name, ..)| *name == quantity || format!("component:{name}") == quantity)
            .map(|(_, values, _)| values.to_vec())
            .ok_or_else(|| format!("unknown quantity {quantity:?}"))?;
        Ok((mask, quantity, values))
    }

    fn parse_list(text: &str, field: &str) -> Result<Vec<f64>, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        trimmed
            .split(',')
            .map(|part| {
                part.trim()
                    .parse::<f64>()
                    .map_err(|e| format!("{field}: {e}"))
            })
            .collect()
    }

    fn compute_metrics(&mut self) {
        self.metrics = None;
        self.metrics_error = None;
        self.metrics_status = None;
        let Some(bundle) = &self.bundle else {
            self.metrics_error = Some("Load a dose bundle first.".into());
            return;
        };
        let voxel_volume = match &bundle.artifact {
            DoseArtifact::Physical(b) => b.geometry.spacing_mm.iter().product(),
            DoseArtifact::Biological(b) => b.geometry.spacing_mm.iter().product(),
        };
        let unit = bundle.artifact.unit().to_owned();
        let case_id = bundle.artifact.case_id().to_owned();
        let source = openbnct_core::ContentReference {
            id: case_id.clone(),
            sha256: bundle.sha256.clone(),
        };
        let outcome = self
            .resolve_selection()
            .and_then(|(mask, quantity, values)| {
                let dx = Self::parse_list(&self.metrics_dx, "dx")?;
                let vx = Self::parse_list(&self.metrics_vx, "vx")?;
                let eud = Self::parse_list(&self.metrics_eud, "eud-a")?;
                RegionDoseMetrics::compute(
                    &case_id,
                    &mask.name,
                    &quantity,
                    source,
                    &unit,
                    &values,
                    &mask.voxels,
                    voxel_volume,
                    &dx,
                    &vx,
                    &eud,
                )
                .map_err(|e| e.to_string())
            });
        match outcome {
            Ok(metrics) => self.metrics = Some(metrics),
            Err(error) => self.metrics_error = Some(error),
        }
    }

    fn save_metrics(&mut self) {
        let path = self.metrics_save_path.trim().to_owned();
        let Some(metrics) = &self.metrics else {
            self.metrics_error = Some("Compute metrics first.".into());
            return;
        };
        if path.is_empty() {
            self.metrics_error = Some("choose an output path first".into());
            return;
        }
        match std::fs::write(&path, serde_json::to_string_pretty(metrics).unwrap() + "\n") {
            Ok(()) => self.metrics_status = Some(format!("wrote {path}")),
            Err(e) => self.metrics_error = Some(e.to_string()),
        }
    }

    fn compute_histogram(&mut self) {
        self.histogram = None;
        self.histogram_error = None;
        let Some(bundle) = &self.bundle else {
            self.histogram_error = Some("Load a dose bundle first.".into());
            return;
        };
        let (mask, quantity, values) = match self.resolve_selection() {
            Ok(selection) => selection,
            Err(error) => {
                self.histogram_error = Some(error);
                return;
            }
        };
        match DoseVolumeHistogram::compute(
            bundle.artifact.case_id(),
            &mask.name,
            &quantity,
            openbnct_core::ContentReference {
                id: bundle.artifact.case_id().to_owned(),
                sha256: bundle.sha256.clone(),
            },
            bundle.artifact.unit(),
            &values,
            &mask.voxels,
            match &bundle.artifact {
                DoseArtifact::Physical(b) => b.geometry.spacing_mm.iter().product(),
                DoseArtifact::Biological(b) => b.geometry.spacing_mm.iter().product(),
            },
            100,
        ) {
            Ok(histogram) => self.histogram = Some(histogram),
            Err(error) => self.histogram_error = Some(error.to_string()),
        }
    }
}

/// UI state for the NIfTI section of the dose workspace — the same four
/// operations as `openbnct nifti` over the shared `openbnct-nifti` crate.
#[derive(Default)]
struct NiftiPanel {
    input_path: String,
    image: Option<openbnct_nifti::NiftiImage>,
    mask_name: String,
    mask_output: String,
    bundle_path: String,
    export_quantity: String,
    export_output: String,
    resample_nearest: bool,
    resample_output: String,
    status: Option<String>,
    error: Option<String>,
}

impl NiftiPanel {
    fn inspect(&mut self) {
        self.image = None;
        self.error = None;
        self.status = None;
        match openbnct_nifti::read_nifti_file(Path::new(self.input_path.trim())) {
            Ok(image) => self.image = Some(image),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn write_mask(&mut self) {
        self.error = None;
        self.status = None;
        let Some(image) = &self.image else {
            self.error = Some("Inspect a NIfTI volume first.".into());
            return;
        };
        let name = self.mask_name.trim();
        let output = self.mask_output.trim();
        if name.is_empty() || output.is_empty() {
            self.error = Some("mask name and output path are required".into());
            return;
        }
        let mask = openbnct_nifti::to_mask(image, name);
        match std::fs::write(output, serde_json::to_string_pretty(&mask).unwrap() + "\n") {
            Ok(()) => {
                self.status = Some(format!(
                    "mask {} written ({} voxels included)",
                    mask.name,
                    mask.voxels.iter().filter(|v| **v).count()
                ));
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn export_dose(&mut self) {
        self.error = None;
        self.status = None;
        let quantity = self.export_quantity.trim();
        let output = self.export_output.trim();
        let loaded = match DoseArtifact::load(Path::new(self.bundle_path.trim())) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let Some((name, values, _)) = loaded
            .artifact
            .rows()
            .into_iter()
            .find(|(name, ..)| *name == quantity || format!("component:{name}") == quantity)
        else {
            self.error = Some(format!("unknown quantity {quantity:?}"));
            return;
        };
        let geometry = match &loaded.artifact {
            DoseArtifact::Physical(b) => b.geometry.clone(),
            DoseArtifact::Biological(b) => b.geometry.clone(),
        };
        let image = openbnct_nifti::NiftiImage {
            geometry,
            values: values.to_vec(),
            datatype: 64,
            transform_source: "sform",
            description: format!("openbnct {} {name}", loaded.artifact.case_id()),
            intent_name: String::new(),
            units_declared_mm: true,
        };
        match openbnct_nifti::write_nifti(&image, Path::new(output)) {
            Ok(()) => self.status = Some(format!("wrote {output}")),
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    fn resample(&mut self) {
        self.error = None;
        self.status = None;
        let Some(image) = &self.image else {
            self.error = Some("Inspect a NIfTI volume first.".into());
            return;
        };
        let output = self.resample_output.trim();
        let geometry =
            match openbnct_nifti::read_target_geometry(Path::new(self.bundle_path.trim())) {
                Ok(geometry) => geometry,
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            };
        let interpolation = if self.resample_nearest {
            openbnct_nifti::Interpolation::Nearest
        } else {
            openbnct_nifti::Interpolation::Trilinear
        };
        let resampled = openbnct_nifti::NiftiImage {
            geometry: geometry.clone(),
            values: openbnct_nifti::resample_to_grid(image, &geometry, interpolation),
            datatype: 64,
            transform_source: "sform",
            description: "openbnct resampled".into(),
            intent_name: String::new(),
            units_declared_mm: true,
        };
        match openbnct_nifti::write_nifti(&resampled, Path::new(output)) {
            Ok(()) => self.status = Some(format!("wrote {output}")),
            Err(e) => self.error = Some(e.to_string()),
        }
    }
}

/// UI state for the evidence workspace's bundle-verification panel.
#[derive(Default)]
struct EvidencePanel {
    root: String,
    manifest: Option<EvidenceBundleManifest>,
    error: Option<String>,
}

impl EvidencePanel {
    fn verify(&mut self) {
        match EvidenceBundleManifest::load_verified(Path::new(self.root.trim())) {
            Ok(manifest) => {
                self.manifest = Some(manifest);
                self.error = None;
            }
            Err(error) => {
                self.manifest = None;
                self.error = Some(error.to_string());
            }
        }
    }
}

/// UI state for the plan workspace's exposure-plan panel.
#[derive(Default)]
struct PlanPanel {
    plan_path: String,
    plan: Option<openbnct_core::ExposurePlan>,
    issues: Vec<String>,
    error: Option<String>,
    export_path: String,
    export_status: Option<String>,
}

impl PlanPanel {
    fn load(&mut self) {
        self.plan = None;
        self.issues.clear();
        self.error = None;
        self.export_status = None;
        match std::fs::read(Path::new(self.plan_path.trim()))
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<openbnct_core::ExposurePlan>(&bytes)
                    .map_err(|e| e.to_string())
            }) {
            Ok(plan) => {
                self.issues = plan
                    .validate_diagnostics()
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                self.plan = Some(plan);
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn export_table(&mut self) {
        self.export_status = None;
        let Some(plan) = &self.plan else {
            self.export_status = Some("Load a plan first.".into());
            return;
        };
        match openbnct_plan::write_table(Path::new(self.export_path.trim()), plan) {
            Ok(()) => self.export_status = Some(format!("wrote {}", self.export_path.trim())),
            Err(error) => self.export_status = Some(error.to_string()),
        }
    }
}

/// UI state for the transport workspace's source-positioning panel. Runs the
/// same `openbnct_transport::aim_source_at_centroid`/`rotate_source` path as
/// `openbnct position` and the Python bindings.
struct PositionPanel {
    source_path: String,
    mask_path: String,
    roi: usize,
    approach: usize,
    half_width_u_cm: String,
    half_width_v_cm: String,
    margin_cm: String,
    report: Option<openbnct_transport::PositionReport>,
    positioned: Option<openbnct_transport::FixedSourceDefinition>,
    rotate_axis: usize,
    rotate_degrees: String,
    save_source_path: String,
    save_report_path: String,
    error: Option<String>,
    status: Option<String>,
}

impl Default for PositionPanel {
    fn default() -> Self {
        Self {
            source_path: String::new(),
            mask_path: String::new(),
            roi: 0,
            approach: 0,
            half_width_u_cm: "1.0".into(),
            half_width_v_cm: "1.0".into(),
            margin_cm: "0.1".into(),
            report: None,
            positioned: None,
            rotate_axis: 0,
            rotate_degrees: "90".into(),
            save_source_path: String::new(),
            save_report_path: String::new(),
            error: None,
            status: None,
        }
    }
}

const APPROACHES: [&str; 6] = ["+x", "-x", "+y", "-y", "+z", "-z"];
const ROTATE_AXES: [&str; 3] = ["x", "y", "z"];

impl PositionPanel {
    fn mask(&self, case: &ViewerCase) -> Result<RegionMask, String> {
        let path = self.mask_path.trim();
        if !path.is_empty() {
            return std::fs::read(Path::new(path))
                .map_err(|e| e.to_string())
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()));
        }
        let roi = case
            .verified
            .structures
            .rois
            .get(self.roi)
            .ok_or("select a target ROI")?;
        Ok(RegionMask {
            name: roi.name.clone(),
            voxels: roi.voxels.clone(),
        })
    }

    fn load_source(&self) -> Result<openbnct_transport::FixedSourceDefinition, String> {
        let source: openbnct_transport::FixedSourceDefinition =
            std::fs::read(Path::new(self.source_path.trim()))
                .map_err(|e| e.to_string())
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))?;
        source.validate().map_err(|e| e.to_string())?;
        Ok(source)
    }

    fn parse_f64(text: &str, field: &str) -> Result<f64, String> {
        text.trim()
            .parse::<f64>()
            .map_err(|e| format!("{field}: {e}"))
    }

    fn aim(&mut self, case: &ViewerCase) {
        self.error = None;
        self.status = None;
        let outcome = (|| {
            let source = self.load_source()?;
            let mask = self.mask(case)?;
            let direction = openbnct_transport::AxisApproach::parse(APPROACHES[self.approach])
                .map_err(|e| e.to_string())?
                .unit_vector();
            let half_widths = [
                Self::parse_f64(&self.half_width_u_cm, "half-width u")?,
                Self::parse_f64(&self.half_width_v_cm, "half-width v")?,
            ];
            let margin = Self::parse_f64(&self.margin_cm, "margin")?;
            let (positioned, mut report) = openbnct_transport::aim_source_at_centroid(
                &source,
                &case.verified.ct.geometry,
                &mask,
                direction,
                half_widths,
                margin,
            )
            .map_err(|e| e.to_string())?;
            report.case_id = case.verified.report.case_id.to_string();
            Ok((positioned, report))
        })();
        match outcome {
            Ok((positioned, report)) => {
                self.positioned = Some(positioned);
                self.report = Some(report);
                self.status = Some("source positioned".into());
            }
            Err(error) => {
                self.positioned = None;
                self.report = None;
                self.error = Some(error);
            }
        }
    }

    fn rotate(&mut self) {
        self.error = None;
        self.status = None;
        let outcome = (|| {
            let source = match &self.positioned {
                Some(positioned) => positioned.clone(),
                None => self.load_source()?,
            };
            let axis = match ROTATE_AXES[self.rotate_axis] {
                "x" => openbnct_transport::PlaneAxis::X,
                "y" => openbnct_transport::PlaneAxis::Y,
                _ => openbnct_transport::PlaneAxis::Z,
            };
            let degrees = Self::parse_f64(&self.rotate_degrees, "degrees")?;
            let center = self
                .report
                .as_ref()
                .map(|r| r.target_centroid_lps_mm)
                .unwrap_or([0.0; 3]);
            openbnct_transport::rotate_source(&source, center, axis, degrees)
                .map_err(|e| e.to_string())
        })();
        match outcome {
            Ok(rotated) => {
                self.positioned = Some(rotated);
                self.status = Some("source rotated".into());
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn save(&mut self, source: bool) {
        let (path, payload) = if source {
            (
                self.save_source_path.trim().to_owned(),
                self.positioned
                    .as_ref()
                    .and_then(|s| serde_json::to_string_pretty(s).ok()),
            )
        } else {
            (
                self.save_report_path.trim().to_owned(),
                self.report
                    .as_ref()
                    .and_then(|r| serde_json::to_string_pretty(r).ok()),
            )
        };
        match (path.is_empty(), payload) {
            (true, _) => self.error = Some("choose an output path first".into()),
            (_, Some(json)) => match std::fs::write(&path, json + "\n") {
                Ok(()) => self.status = Some(format!("wrote {path}")),
                Err(e) => self.error = Some(e.to_string()),
            },
            (_, None) => self.error = Some("position a source first".into()),
        }
    }
}

/// Per-workspace panels shared between the app and the workbench render.
#[derive(Default)]
struct WorkbenchPanels {
    dose: DosePanel,
    evidence: EvidencePanel,
    nifti: NiftiPanel,
    plan: PlanPanel,
    position: PositionPanel,
}

/// Where an OS-dragged path lands in the workbench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropTarget {
    Case,
    DoseBundle,
    NiftiVolume,
    Unsupported,
}

/// Directories are case roots; `.json` files are dose bundles; `.nii` /
/// `.nii.gz` files are NIfTI volumes. Anything else is reported rather than
/// guessed.
fn classify_dropped_path(path: &Path) -> DropTarget {
    if path.is_dir() {
        return DropTarget::Case;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".nii") || name.ends_with(".nii.gz") {
        return DropTarget::NiftiVolume;
    }
    if name.ends_with(".json") {
        return DropTarget::DoseBundle;
    }
    DropTarget::Unsupported
}

struct OpenBnctApp {
    case_path: String,
    load_error: Option<String>,
    case: Option<ViewerCase>,
    display: DisplaySettings,
    workspace: WorkspaceTab,
    dark_mode: bool,
    template_status: Option<String>,
    brand_logo: Option<egui::TextureHandle>,
    help: GuidedHelp,
    panels: WorkbenchPanels,
}

impl OpenBnctApp {
    fn new(initial_case: Option<PathBuf>, context: &egui::Context) -> Self {
        let has_initial_case = initial_case.is_some();
        let mut app = Self {
            case_path: initial_case
                .as_deref()
                .map_or_else(String::new, |path| path.display().to_string()),
            load_error: None,
            case: None,
            display: DisplaySettings::default(),
            workspace: if has_initial_case {
                WorkspaceTab::Geometry
            } else {
                WorkspaceTab::Overview
            },
            dark_mode: false,
            template_status: None,
            brand_logo: brand::load_logo_texture(context).ok(),
            help: GuidedHelp::default(),
            panels: WorkbenchPanels::default(),
        };
        if has_initial_case {
            app.load_case();
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("OPENBNCT_CAPTURE").is_some() {
            if let Ok(selected) = std::env::var("OPENBNCT_CAPTURE_WORKSPACE") {
                if let Some(workspace) = WorkspaceTab::ALL
                    .into_iter()
                    .find(|tab| tab.marker() == selected)
                {
                    app.workspace = workspace;
                }
            }
            if std::env::var_os("OPENBNCT_CAPTURE_DARK").is_some() {
                app.dark_mode = true;
                context.set_theme(egui::ThemePreference::Dark);
            }
        }
        app
    }

    fn load_case(&mut self) {
        let path = PathBuf::from(self.case_path.trim());
        if self.case_path.trim().is_empty() {
            self.load_error = Some("Enter an NF-BNCT-001 directory.".into());
            return;
        }
        match ViewerCase::load(&path) {
            Ok(case) => {
                self.case = Some(case);
                self.load_error = None;
            }
            Err(error) => {
                self.case = None;
                self.load_error = Some(error);
            }
        }
    }

    /// Traditional top-left menu bar. File owns case/template actions,
    /// View owns the theme toggle, Help owns the help center.
    fn show_menu_bar(&mut self, ui: &mut egui::Ui, tour_targets: &mut TourTargets) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open case…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.case_path = dir.display().to_string();
                        self.load_case();
                    }
                    ui.close();
                }
                if ui.button("Export case template…").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.template_status = Some(match export_case_template(&dir) {
                            Ok(count) => format!(
                                "template written to {} ({count} files — edit before use)",
                                dir.display()
                            ),
                            Err(error) => format!("template export failed: {error}"),
                        });
                    }
                    ui.close();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                ui.checkbox(&mut self.dark_mode, "Dark mode");
            });
            let help = ui.menu_button("Help", |ui| {
                if ui.button("Help and guided tours").clicked() {
                    self.help.toggle_center();
                    ui.close();
                }
            });
            tour_targets.set(TourTarget::HelpButton, help.response.rect);
        });
    }

    /// Route an OS-dropped path onto the matching workbench surface: a
    /// directory loads as a case, a `.json` as a dose bundle, and a
    /// `.nii`/`.nii.gz` as a NIfTI volume.
    fn handle_dropped(&mut self, paths: &[PathBuf]) {
        let Some(path) = paths.first() else {
            return;
        };
        match classify_dropped_path(path) {
            DropTarget::Case => {
                self.case_path = path.display().to_string();
                self.load_case();
                self.workspace = WorkspaceTab::Geometry;
            }
            DropTarget::DoseBundle => {
                self.panels.dose.bundle_path = path.display().to_string();
                self.panels.dose.load_bundle();
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::NiftiVolume => {
                self.panels.nifti.input_path = path.display().to_string();
                self.panels.nifti.inspect();
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::Unsupported => {
                self.load_error = Some(format!("unsupported drop {}", path.display()));
            }
        }
    }
}

impl eframe::App for OpenBnctApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(debug_assertions)]
        capture_preview(ui.ctx());
        if ui.input(|input| input.key_pressed(egui::Key::F1)) {
            self.help.toggle_center();
        }
        if let Some(workspace) = self.help.requested_workspace() {
            self.workspace = workspace.into();
        }

        let dropped: Vec<PathBuf> = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.handle_dropped(&dropped);
        }

        let mut tour_targets = TourTargets::default();
        let dark_before = self.dark_mode;
        egui::Panel::top("openbnct-menu-bar").show(ui, |ui| {
            self.show_menu_bar(ui, &mut tour_targets);
        });
        if self.dark_mode != dark_before {
            ui.ctx().set_theme(if self.dark_mode {
                egui::ThemePreference::Dark
            } else {
                egui::ThemePreference::Light
            });
        }
        let theme = Theme::resolve(ui.visuals().dark_mode);
        egui::Panel::top("openbnct-header")
            .frame(
                egui::Frame::new()
                    .fill(theme.banner_fill)
                    .inner_margin(egui::Margin::symmetric(20, 10)),
            )
            .show(ui, |ui| {
                show_app_header(
                    ui,
                    self.case.as_ref(),
                    self.brand_logo.as_ref(),
                    theme,
                    &mut tour_targets,
                );
                ui.add_space(8.0);
                let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let load_requested = show_case_loader(
                    ui,
                    &mut self.case_path,
                    self.case.as_ref(),
                    enter_pressed,
                    &mut tour_targets,
                    &mut self.template_status,
                );
                if load_requested {
                    self.load_case();
                }
                if let Some(error) = &self.load_error {
                    ui.colored_label(theme.error, format!("Load rejected: {error}"));
                }
                if let Some(status) = &self.template_status {
                    ui.label(status);
                }
            });
        egui::Panel::bottom("workbench-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Research use only · Not for clinical decision-making")
                        .small()
                        .color(theme.text_dim),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new("Avila Labs  /  OpenBNCT")
                            .small()
                            .color(theme.text_dim),
                    );
                });
            });
        });
        show_workbench(
            ui,
            &mut self.workspace,
            self.case.as_mut(),
            &mut self.display,
            &mut self.panels,
            &mut tour_targets,
            theme,
        );
        self.help
            .show_center(ui.ctx(), self.workspace.into(), self.case.is_some(), theme);
        self.help.show_tour(ui.ctx(), &tour_targets, theme);
    }
}

/// Installs the OpenBNCT palette into egui's own light and dark visuals so
/// every native surface — panels, menus, popups, tooltips, text fields —
/// follows the theme, not just the custom frames. Called once at startup;
/// the View menu only flips `ThemePreference` afterward.
fn configure_style(context: &egui::Context) {
    let mut light = egui::Visuals::light();
    light.panel_fill = egui::Color32::from_rgb(244, 247, 246);
    light.window_fill = egui::Color32::WHITE;
    light.faint_bg_color = egui::Color32::from_rgb(248, 250, 249);
    light.extreme_bg_color = egui::Color32::WHITE;
    light.hyperlink_color = egui::Color32::from_rgb(24, 0, 173);
    light.override_text_color = Some(egui::Color32::from_rgb(36, 49, 48));
    light.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(221, 229, 225));
    light.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(237, 242, 239);
    light.widgets.inactive.bg_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgb(210, 221, 215));
    light.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(232, 230, 250);
    light.selection.bg_fill = egui::Color32::from_rgb(232, 230, 250);
    light.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(24, 0, 173));
    context.set_visuals_of(egui::Theme::Light, light);
    context.style_mut_of(egui::Theme::Light, apply_spacing);

    let mut dark = egui::Visuals::dark();
    dark.panel_fill = egui::Color32::from_rgb(17, 21, 29);
    dark.window_fill = egui::Color32::from_rgb(21, 26, 36);
    dark.faint_bg_color = egui::Color32::from_rgb(27, 33, 44);
    dark.extreme_bg_color = egui::Color32::from_rgb(10, 13, 19);
    dark.hyperlink_color = egui::Color32::from_rgb(172, 166, 255);
    dark.selection.bg_fill = egui::Color32::from_rgb(51, 43, 117);
    dark.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(190, 184, 255));
    context.set_visuals_of(egui::Theme::Dark, dark);
    context.style_mut_of(egui::Theme::Dark, apply_spacing);

    context.set_theme(egui::ThemePreference::Light);
}

fn apply_spacing(style: &mut egui::Style) {
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style.spacing.interact_size.y = 30.0;
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
}

fn show_app_header(
    ui: &mut egui::Ui,
    case: Option<&ViewerCase>,
    brand_logo: Option<&egui::TextureHandle>,
    theme: Theme,
    tour_targets: &mut TourTargets,
) {
    ui.horizontal(|ui| {
        if let Some(logo) = brand_logo {
            let response = ui.add(
                egui::Image::from_texture(logo)
                    .fit_to_exact_size(egui::vec2(30.0, 30.0))
                    .corner_radius(5.0),
            );
            tour_targets.set(TourTarget::Brand, response.rect);
        } else {
            let response = ui.strong("Avila Labs");
            tour_targets.set(TourTarget::Brand, response.rect);
        }
        ui.label(egui::RichText::new("OpenBNCT").size(21.0).strong());
        ui.separator();
        ui.label(egui::RichText::new("Research workbench").color(theme.text_dim));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(case.map_or("No case open", |c| c.verified.report.case_id))
                    .color(theme.text_dim),
            );
        });
    });
}

/// The transport-side authoring contracts, embedded from the benchmark so
/// "export template" writes real validated structure a user can edit into
/// their own case. The response set and acceptance contract are provenance
/// artifacts and stay repo-side.
const TEMPLATE_FILES: [(&str, &str); 4] = [
    (
        "case.json",
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/case.json"),
    ),
    (
        "source.json",
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/source.json"),
    ),
    (
        "material.json",
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json"),
    ),
    (
        "component-profile.json",
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/component-profile.json"),
    ),
];

const TEMPLATE_README: &str = "# OpenBNCT transport-case template\n\
\n\
These are the NF-BNCT-001 benchmark contracts verbatim — a validated\n\
starting point, not your case. Edit `case.json` (geometry, regions),\n\
`source.json` (beam), `material.json` (composition), and\n\
`component-profile.json` (folding) to your geometry and data, then feed\n\
the directory to `openbnct openmc generate`. A `voxel_set` geometry needs\n\
a real volume source (NIfTI/DICOM), which the GUI does not author.\n";

fn export_case_template(destination: &Path) -> Result<usize, String> {
    let mut written = 0;
    for (name, contents) in TEMPLATE_FILES {
        std::fs::write(destination.join(name), contents)
            .map_err(|error| format!("{name}: {error}"))?;
        written += 1;
    }
    std::fs::write(destination.join("README.md"), TEMPLATE_README)
        .map_err(|error| format!("README.md: {error}"))?;
    Ok(written + 1)
}

fn show_case_loader(
    ui: &mut egui::Ui,
    case_path: &mut String,
    case: Option<&ViewerCase>,
    enter_pressed: bool,
    tour_targets: &mut TourTargets,
    template_status: &mut Option<String>,
) -> bool {
    let mut load_requested = false;
    let _ = (case, template_status);
    let response = ui.horizontal(|ui| {
        ui.label("Case folder");
        let path_response = ui.add(
            egui::TextEdit::singleline(case_path)
                .desired_width((ui.available_width() - 220.0).max(180.0))
                .hint_text("Open a verified case folder, or drop it here"),
        );
        load_requested =
            ui.button("Load & verify").clicked() || (path_response.lost_focus() && enter_pressed);
        if ui.button("Browse…").clicked()
            && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            *case_path = dir.display().to_string();
            load_requested = true;
        }
    });
    tour_targets.set(TourTarget::CaseLoader, response.response.rect);
    load_requested
}

fn show_workbench(
    ui: &mut egui::Ui,
    workspace: &mut WorkspaceTab,
    case: Option<&mut ViewerCase>,
    display: &mut DisplaySettings,
    panels: &mut WorkbenchPanels,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    let navigation = egui::Panel::left("openbnct-workspace-navigation")
        .exact_size(210.0)
        .resizable(false)
        .frame(
            egui::Frame::new()
                .fill(theme.nav_fill)
                .inner_margin(egui::Margin::symmetric(16, 24)),
        )
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("WORKBENCH")
                    .size(11.0)
                    .strong()
                    .color(theme.text_dim),
            );
            ui.add_space(12.0);
            for candidate in WorkspaceTab::ALL {
                if candidate == WorkspaceTab::Plan || candidate == WorkspaceTab::Evidence {
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);
                }
                let selected = *workspace == candidate;
                let label = egui::RichText::new(candidate.label()).color(if selected {
                    theme.brand
                } else {
                    ui.visuals().text_color()
                });
                if ui
                    .add_sized(
                        [ui.available_width(), 38.0],
                        egui::Button::new(label)
                            .selected(selected)
                            .frame(true)
                            .frame_when_inactive(selected),
                    )
                    .clicked()
                {
                    *workspace = candidate;
                }
            }
            ui.add_space(30.0);
            ui.label(
                egui::RichText::new("ACTIVE CASE")
                    .size(11.0)
                    .strong()
                    .color(theme.text_dim),
            );
            ui.add_space(6.0);
            if let Some(case) = case.as_deref() {
                ui.strong(case.verified.report.case_id);
                ui.small(format!(
                    "{} verified artifacts",
                    case.verified.report.verified_artifact_count
                ));
            } else {
                ui.label(egui::RichText::new("No case loaded").color(theme.text_dim));
                ui.small("Open a case above to inspect its geometry.");
            }
        });
    tour_targets.set(TourTarget::WorkspaceNavigation, navigation.response.rect);
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(theme.panel_fill)
                .inner_margin(egui::Margin::symmetric(28, 24)),
        )
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("openbnct-workspace", workspace.marker()))
                .auto_shrink([false, false])
                .show(ui, |ui| match *workspace {
                    WorkspaceTab::Overview => {
                        show_overview(ui, case.as_deref(), workspace, tour_targets, theme);
                    }
                    WorkspaceTab::Geometry => {
                        if let Some(case) = case {
                            show_geometry_workspace(ui, case, display, tour_targets, theme);
                        } else {
                            show_workspace_heading(
                                ui,
                                theme,
                                "Geometry",
                                "Patient-space DICOM truth before transport.",
                            );
                            show_empty_state(ui);
                        }
                    }
                    WorkspaceTab::Transport => show_transport_workspace(
                        ui,
                        case.as_deref(),
                        &mut panels.position,
                        tour_targets,
                        theme,
                    ),
                    WorkspaceTab::Plan => show_plan_workspace(ui, &mut panels.plan, theme),
                    WorkspaceTab::Dose => {
                        show_dose_workspace(ui, &mut panels.dose, &mut panels.nifti, theme);
                    }
                    WorkspaceTab::Evidence => show_evidence_workspace(
                        ui,
                        case.as_deref(),
                        &mut panels.evidence,
                        tour_targets,
                        theme,
                    ),
                });
        });
}

fn show_workspace_heading(ui: &mut egui::Ui, theme: Theme, title: &str, subtitle: &str) {
    ui.heading(egui::RichText::new(title).size(23.0));
    ui.label(egui::RichText::new(subtitle).color(theme.text_dim));
    ui.add_space(8.0);
}

fn show_overview(
    ui: &mut egui::Ui,
    case: Option<&ViewerCase>,
    workspace: &mut WorkspaceTab,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        "Your research workspace",
        "Inspect the case. Explore dose. Follow the evidence.",
    );
    ui.add_space(12.0);
    egui::Frame::new().fill(theme.card_fill).corner_radius(10)
        .inner_margin(egui::Margin::same(24)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("CASE STUDY  /  SYNTHETIC BENCHMARK").size(11.0).strong().color(theme.brand));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(case.map_or("Start with a verified case", |c| c.verified.report.case_id)).size(26.0).strong());
            ui.label(egui::RichText::new(if case.is_some() {
                "Patient-space geometry and artifact integrity verified. Ready to inspect."
            } else {
                "Open an NF-BNCT-001 case folder above. Geometry is verified before it is displayed."
            }).color(theme.text_dim));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(case.is_some(), egui::Button::new("Inspect geometry")).clicked() {
                    *workspace = WorkspaceTab::Geometry;
                }
                if ui.button("Review evidence").clicked() { *workspace = WorkspaceTab::Evidence; }
            });
        });
    ui.add_space(20.0);
    ui.heading("Explore the workbench");
    ui.add_space(6.0);
    ui.columns(3, |columns| {
        for (column, (tab, title, detail, action)) in columns.iter_mut().zip([
            (WorkspaceTab::Transport, "01  Prepare", "Inspect material and source contracts, position the beam, and review transport readiness.", "Open transport"),
            (WorkspaceTab::Plan, "02  Evaluate", "Load a plan, inspect fields and weights, and review its calculated result.", "Open planning"),
            (WorkspaceTab::Dose, "03  Understand", "Explore physical and biological dose, region metrics, DVHs, and NIfTI volumes.", "Explore dose"),
        ]) {
            egui::Frame::new().fill(theme.card_fill).corner_radius(8)
                .inner_margin(egui::Margin::same(18)).show(column, |ui| {
                    ui.set_width(ui.available_width());
                    ui.strong(title);
                    ui.add_space(6.0);
                    ui.allocate_ui(egui::vec2(ui.available_width(), 76.0), |ui| {
                        ui.set_min_height(76.0);
                        ui.add(egui::Label::new(egui::RichText::new(detail).color(theme.text_dim)).halign(egui::Align::Min));
                    });
                    if ui.button(action).clicked() { *workspace = tab; }
                });
        }
    });
    ui.add_space(24.0);
    let gates = ui.scope(|ui| {
        ui.heading("Benchmark readiness");
        ui.label(egui::RichText::new("Qualification of the frozen reference workflow; imported artifacts have their own validation.").color(theme.text_dim));
        ui.add_space(8.0);
        egui::Frame::new().fill(theme.card_fill).corner_radius(8)
            .inner_margin(egui::Margin::same(18)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, gate) in readiness_gates(case.is_some()).iter().enumerate() {
                    if index > 0 { ui.separator(); }
                    ui.horizontal(|ui| {
                        ui.allocate_ui(egui::vec2(180.0, 28.0), |ui| { ui.strong(gate.title); });
                        ui.allocate_ui(egui::vec2((ui.available_width() - 130.0).max(150.0), 28.0), |ui| {
                            ui.label(egui::RichText::new(gate.detail).small().color(theme.text_dim));
                        });
                        status_badge(ui, gate.state, gate.state.label());
                    });
                }
            });
    });
    tour_targets.set(TourTarget::OverviewGates, gates.response.rect);
}

fn status_badge(ui: &mut egui::Ui, state: GateState, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(11.0)
            .strong()
            .color(state.color(ui.visuals().dark_mode)),
    );
}

struct DisplaySettings {
    window_center: f64,
    window_width: f64,
    overlay_opacity: f32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            window_center: 0.0,
            window_width: 400.0,
            overlay_opacity: 0.48,
        }
    }
}

struct ViewerCase {
    root: PathBuf,
    verified: VerifiedBenchmarkCase,
    grid: PatientAlignedGrid,
    crosshair: Crosshair,
    roi_visible: Vec<bool>,
    dose: DoseOverlay,
    textures: [Option<egui::TextureHandle>; 3],
    textures_dirty: bool,
}

impl ViewerCase {
    fn load(root: &Path) -> Result<Self, String> {
        let verified = load_nf_bnct_001(root).map_err(|error| error.to_string())?;
        let grid = PatientAlignedGrid::new(&verified.ct.geometry)
            .map_err(|error| format!("anatomical viewer cannot represent this grid: {error}"))?;
        let crosshair = Crosshair::centered(&grid);
        let roi_visible = verified
            .structures
            .rois
            .iter()
            .map(|roi| roi.name != "PHANTOM")
            .collect();
        Ok(Self {
            root: root.to_path_buf(),
            verified,
            grid,
            crosshair,
            roi_visible,
            dose: DoseOverlay::default(),
            textures: array::from_fn(|_| None),
            textures_dirty: true,
        })
    }

    fn refresh_textures(
        &mut self,
        context: &egui::Context,
        display: &DisplaySettings,
    ) -> Result<(), String> {
        if !self.textures_dirty {
            return Ok(());
        }
        let wash = self.dose.wash();
        for (view_index, plane) in AnatomicalPlane::ALL.into_iter().enumerate() {
            let view = self
                .grid
                .slice(plane, self.crosshair)
                .map_err(|error| error.to_string())?;
            let image = render_slice(&self.verified, view, &self.roi_visible, wash, display)?;
            if let Some(texture) = &mut self.textures[view_index] {
                texture.set(image, egui::TextureOptions::NEAREST);
            } else {
                self.textures[view_index] = Some(context.load_texture(
                    format!("openbnct-{}-{}", self.root.display(), plane.name()),
                    image,
                    egui::TextureOptions::NEAREST,
                ));
            }
        }
        self.textures_dirty = false;
        Ok(())
    }
}

fn show_geometry_workspace(
    ui: &mut egui::Ui,
    case: &mut ViewerCase,
    display: &mut DisplaySettings,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        "Geometry",
        "Integrity-gated, linked patient-space views of the frozen synthetic case.",
    );
    if let Err(error) = case.refresh_textures(ui.ctx(), display) {
        ui.colored_label(theme.error, format!("Render rejected: {error}"));
        return;
    }
    let image_height = ((ui.available_height() - 140.0) / 2.0).clamp(140.0, 320.0);
    let mut selected_voxel = None;
    ui.columns(2, |columns| {
        for (index, plane) in AnatomicalPlane::ALL.into_iter().enumerate() {
            let column = &mut columns[index % 2];
            egui::Frame::new()
                .fill(theme.card_fill)
                .corner_radius(8)
                .inner_margin(egui::Margin::same(14))
                .show(column, |ui| {
                    ui.set_width(ui.available_width());
                    if let Some(texture) = &case.textures[index]
                        && let Ok(view) = case.grid.slice(plane, case.crosshair)
                        && let Some(voxel) =
                            show_slice_view(ui, texture, view, case.crosshair, image_height)
                    {
                        selected_voxel = Some(voxel);
                    }
                });
            column.add_space(12.0);
        }
        let controls = egui::Frame::new()
            .fill(theme.card_fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(18))
            .show(&mut columns[1], |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .id_salt("geometry-inspector")
                    .max_height(image_height + 30.0)
                    .show(ui, |ui| {
                        ui.heading("Image inspector");
                        ui.label(
                            egui::RichText::new(
                                "Click or drag an image to move the linked crosshair.",
                            )
                            .color(theme.text_dim),
                        );
                        ui.collapsing("Case & voxel details", |ui| {
                            show_case_summary(ui, case);
                        });
                        if show_display_controls(ui, case, display, theme) {
                            case.textures_dirty = true;
                        }
                    });
            });
        tour_targets.set(TourTarget::GeometryControls, controls.response.rect);
    });
    tour_targets.set(TourTarget::GeometryViews, ui.min_rect());
    if let Some(voxel) = selected_voxel
        && case.crosshair.set_voxel(&case.grid, voxel).is_ok()
    {
        case.textures_dirty = true;
    }
}

fn show_transport_workspace(
    ui: &mut egui::Ui,
    case: Option<&ViewerCase>,
    panel: &mut PositionPanel,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        "Transport",
        "Backend-neutral preparation with explicit scientific and execution gates.",
    );
    let backend = OpenMcBackend::default().descriptor();

    egui::Frame::new()
        .fill(theme.card_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading(format!("{} adapter", backend.display_name));
                    ui.label(format!(
                        "Pinned target {} · executable boundary: {}",
                        TARGET_OPENMC_VERSION, backend.id
                    ));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_badge(ui, GateState::Pending, "NOT EXECUTABLE")
                });
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                capability_label(ui, "Prepare", backend.can_prepare);
                capability_label(ui, "Execute", backend.can_execute);
                capability_label(ui, "Import", backend.can_import);
            });
        });

    ui.add_space(12.0);
    let gate_chain = ui.scope(|ui| {
        ui.heading("Run gate chain");
        for (index, gate) in readiness_gates(case.is_some()).into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(format!("{:02}", index + 1));
                ui.colored_label(gate.state.color(ui.visuals().dark_mode), "●");
                ui.strong(gate.title);
                ui.label("—");
                ui.label(gate.detail);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(gate.state.label())
                            .small()
                            .strong()
                            .color(gate.state.color(ui.visuals().dark_mode)),
                    );
                });
            });
            if index + 1 != readiness_gates(case.is_some()).len() {
                ui.separator();
            }
        }
    });
    tour_targets.set(TourTarget::TransportGates, gate_chain.response.rect);

    ui.add_space(14.0);
    let actions = ui.horizontal(|ui| {
        ui.add_enabled(false, egui::Button::new("Prepare OpenMC run"))
            .on_disabled_hover_text("Blocked until the reviewed component responses pass.");
        ui.add_enabled(false, egui::Button::new("Execute transport"))
            .on_disabled_hover_text("The backend does not advertise controlled execution yet.");
        ui.label("Disabled controls reflect real adapter capabilities.");
    });
    tour_targets.set(TourTarget::TransportActions, actions.response.rect);

    ui.add_space(14.0);
    ui.heading("Source positioning");
    ui.label("Same aim/rotate path as `openbnct position` — reports the entry geometry without running transport.");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SOURCE").small().strong());
            ui.add(
                egui::TextEdit::singleline(&mut panel.source_path)
                    .desired_width(420.0)
                    .hint_text("/path/to/source.json"),
            );
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("TARGET").small().strong());
            if let Some(case) = case {
                let names: Vec<&str> = case
                    .verified
                    .structures
                    .rois
                    .iter()
                    .map(|roi| roi.name.as_str())
                    .collect();
                if !names.is_empty() {
                    panel.roi = panel.roi.min(names.len() - 1);
                    egui::ComboBox::from_id_salt("position-roi")
                        .selected_text(names[panel.roi])
                        .show_ui(ui, |ui| {
                            for (index, name) in names.iter().enumerate() {
                                ui.selectable_value(&mut panel.roi, index, *name);
                            }
                        });
                }
            }
            ui.label("or mask file:");
            ui.add(
                egui::TextEdit::singleline(&mut panel.mask_path)
                    .desired_width(300.0)
                    .hint_text("optional /path/to/mask.json"),
            );
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("APPROACH").small().strong());
            egui::ComboBox::from_id_salt("position-approach")
                .selected_text(APPROACHES[panel.approach])
                .show_ui(ui, |ui| {
                    for (index, name) in APPROACHES.iter().enumerate() {
                        ui.selectable_value(&mut panel.approach, index, *name);
                    }
                });
            ui.label("half-widths u/v cm:");
            ui.add(egui::TextEdit::singleline(&mut panel.half_width_u_cm).desired_width(50.0));
            ui.add(egui::TextEdit::singleline(&mut panel.half_width_v_cm).desired_width(50.0));
            ui.label("margin cm:");
            ui.add(egui::TextEdit::singleline(&mut panel.margin_cm).desired_width(50.0));
            if ui
                .add_enabled(case.is_some(), egui::Button::new("Aim at centroid"))
                .on_disabled_hover_text("Load a case for geometry and ROI masks.")
                .clicked()
                && let Some(case) = case
            {
                panel.aim(case);
            }
        });
    });

    if let Some(report) = &panel.report {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Position report");
                ui.monospace(format!("schema {}", report.schema_version));
            });
            ui.monospace(format!(
                "case {} · target {} · entry {} {} face",
                report.case_id,
                report.target_region,
                side_label(report.entry_side),
                axis_label(report.entry_axis),
            ));
            ui.monospace(format!(
                "centroid LPS mm: [{:.2}, {:.2}, {:.2}]",
                report.target_centroid_lps_mm[0],
                report.target_centroid_lps_mm[1],
                report.target_centroid_lps_mm[2],
            ));
            ui.monospace(format!(
                "entry point LPS mm: [{:.2}, {:.2}, {:.2}] · source→centroid {:.1} mm",
                report.entry_point_lps_mm[0],
                report.entry_point_lps_mm[1],
                report.entry_point_lps_mm[2],
                report.source_to_centroid_mm,
            ));
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ROTATE").small().strong());
            egui::ComboBox::from_id_salt("position-rotate-axis")
                .selected_text(ROTATE_AXES[panel.rotate_axis])
                .show_ui(ui, |ui| {
                    for (index, name) in ROTATE_AXES.iter().enumerate() {
                        ui.selectable_value(&mut panel.rotate_axis, index, *name);
                    }
                });
            ui.label("degrees:");
            ui.add(egui::TextEdit::singleline(&mut panel.rotate_degrees).desired_width(50.0));
            if ui
                .button("Rotate about target centroid")
                .on_hover_text(
                    "Quarter-turn multiples only (90/180/270); the report stays the aim record.",
                )
                .clicked()
            {
                panel.rotate();
            }
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("SAVE").small().strong());
            ui.add(
                egui::TextEdit::singleline(&mut panel.save_source_path)
                    .desired_width(260.0)
                    .hint_text("positioned-source.json"),
            );
            if ui.button("Write source").clicked() {
                panel.save(true);
            }
            ui.add(
                egui::TextEdit::singleline(&mut panel.save_report_path)
                    .desired_width(260.0)
                    .hint_text("position-report.json"),
            );
            if ui.button("Write report").clicked() {
                panel.save(false);
            }
        });
    }
    if let Some(error) = &panel.error {
        ui.colored_label(theme.error, format!("Positioning rejected: {error}"));
    }
    if let Some(status) = &panel.status {
        ui.colored_label(GateState::Verified.color(ui.visuals().dark_mode), status);
    }
}

fn axis_label(axis: openbnct_transport::PlaneAxis) -> &'static str {
    match axis {
        openbnct_transport::PlaneAxis::X => "x",
        openbnct_transport::PlaneAxis::Y => "y",
        openbnct_transport::PlaneAxis::Z => "z",
    }
}

fn side_label(side: openbnct_transport::EntrySide) -> &'static str {
    match side {
        openbnct_transport::EntrySide::Low => "low",
        openbnct_transport::EntrySide::High => "high",
    }
}

fn capability_label(ui: &mut egui::Ui, name: &str, enabled: bool) {
    let (state, value) = if enabled {
        (GateState::Verified, "available")
    } else {
        (GateState::Pending, "unavailable")
    };
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.strong(name);
            ui.colored_label(state.color(ui.visuals().dark_mode), value);
        });
    });
}

fn show_plan_workspace(ui: &mut egui::Ui, panel: &mut PlanPanel, theme: Theme) {
    show_workspace_heading(
        ui,
        theme,
        "Exposure plan",
        "Structured multi-exposure schedules; every detected issue is reported, not just the first.",
    );

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("PLAN").small().strong());
            ui.add(
                egui::TextEdit::singleline(&mut panel.plan_path)
                    .desired_width((ui.available_width() - 275.0).max(160.0))
                    .hint_text("/path/to/exposure-plan.json"),
            );
            if ui.button("Browse…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Plan JSON", &["json"])
                    .pick_file()
            {
                panel.plan_path = path.display().to_string();
                panel.load();
            }
            if ui.button("Load + diagnose").clicked() {
                panel.load();
            }
        });
        if let Some(error) = &panel.error {
            ui.colored_label(theme.error, format!("Plan file rejected: {error}"));
        }
    });

    let Some(plan) = &panel.plan else {
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(theme.card_fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                status_badge(ui, GateState::Pending, "NO PLAN LOADED");
                ui.label(
                    "Load an openbnct.exposure-plan JSON document to inspect its exposures \
                     and validation issues. Tables authored in CSV or XLSX convert through \
                     `openbnct plan import`; saved scenarios rerun through \
                     `openbnct accumulate`.",
                );
            });
        return;
    };

    ui.add_space(10.0);
    egui::Frame::new()
        .fill(theme.card_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading(&plan.id);
                    ui.monospace(format!("case: {}", plan.case_id));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if panel.issues.is_empty() {
                        status_badge(ui, GateState::Verified, "VALID");
                    } else {
                        status_badge(
                            ui,
                            GateState::Blocked,
                            &format!("{} ISSUE(S)", panel.issues.len()),
                        );
                    }
                });
            });
            if !panel.issues.is_empty() {
                ui.add_space(6.0);
                for issue in &panel.issues {
                    ui.colored_label(theme.error, format!("• {issue}"));
                }
            }
            ui.add_space(8.0);
            egui::Grid::new("plan-exposures")
                .striped(true)
                .show(ui, |ui| {
                    for header in [
                        "name",
                        "bundle path",
                        "weight",
                        "basis",
                        "duration s",
                        "boron",
                    ] {
                        ui.strong(header);
                    }
                    ui.end_row();
                    for exposure in &plan.exposures {
                        ui.monospace(&exposure.name);
                        ui.monospace(&exposure.dose_bundle.path);
                        ui.monospace(exposure.weight.to_string());
                        ui.label(format!("{:?}", exposure.weight_basis));
                        ui.monospace(
                            exposure
                                .duration_s
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| "—".into()),
                        );
                        ui.label(exposure.boron_assumption.as_deref().unwrap_or("—"));
                        ui.end_row();
                    }
                });
        });

    ui.add_space(10.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("TABLE EXPORT").small().strong());
            ui.add(
                egui::TextEdit::singleline(&mut panel.export_path)
                    .desired_width(420.0)
                    .hint_text("/path/to/schedule.csv or .xlsx"),
            );
            if ui.button("Export table").clicked() {
                panel.export_table();
            }
        });
        if let Some(status) = &panel.export_status {
            ui.label(status);
        }
    });
}

fn show_dose_workspace(
    ui: &mut egui::Ui,
    panel: &mut DosePanel,
    nifti: &mut NiftiPanel,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        "Dose components",
        "Physical and biological layers stay separate; only validated bundles render.",
    );

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("DOSE BUNDLE").small().strong());
            ui.add(
                egui::TextEdit::singleline(&mut panel.bundle_path)
                    .desired_width(460.0)
                    .hint_text("/path/to/dose-bundle.json — or drop it here"),
            );
            if ui.button("Load + validate").clicked() {
                panel.load_bundle();
            }
            if ui.button("Browse…").clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .add_filter("dose bundle", &["json"])
                    .pick_file()
            {
                panel.bundle_path = file.display().to_string();
                panel.load_bundle();
            }
        });
        if let Some(error) = &panel.bundle_error {
            ui.colored_label(theme.error, format!("Load rejected: {error}"));
        }
    });

    let Some(bundle) = &panel.bundle else {
        ui.add_space(12.0);
        egui::Frame::new()
            .fill(theme.card_fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                status_badge(ui, GateState::Pending, "NO RESULT LOADED");
                ui.label(
                    "OpenBNCT does not render placeholder dose values. Load a validated \
                     physical or biological dose bundle to inspect components, totals, \
                     and DVHs.",
                );
            });
        return;
    };

    ui.add_space(10.0);
    let artifact = &bundle.artifact;
    let is_biological = matches!(artifact, DoseArtifact::Biological(_));
    egui::Frame::new()
        .fill(if is_biological {
            theme.card_alt_fill
        } else {
            theme.card_fill
        })
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading(artifact.case_id());
                    ui.monospace(format!("sha256:{}", bundle.sha256));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    status_badge(ui, GateState::Verified, "VALIDATED");
                    status_badge(
                        ui,
                        if is_biological {
                            GateState::Blocked
                        } else {
                            GateState::Frozen
                        },
                        artifact.qualification().to_uppercase().as_str(),
                    );
                });
            });
            if is_biological {
                ui.colored_label(
                    if ui.visuals().dark_mode {
                        egui::Color32::from_rgb(206, 121, 226)
                    } else {
                        egui::Color32::from_rgb(132, 69, 153)
                    },
                    "Biologically weighted — never aliases physical dose. Not a clinical quantity.",
                );
            }
        });

    ui.add_space(10.0);
    let rows = artifact.rows();
    let columns = rows.len().clamp(1, 5);
    ui.columns(columns, |columns| {
        for (column, (name, values, sigma)) in columns.iter_mut().zip(rows.iter()) {
            let (symbol, color) = match name.as_str() {
                "boron" => ("D_B", GateState::Verified.color(column.visuals().dark_mode)),
                "nitrogen" => ("D_N", GateState::Frozen.color(column.visuals().dark_mode)),
                "hydrogen" => ("D_H", GateState::Blocked.color(column.visuals().dark_mode)),
                "photon" => (
                    "D_gamma",
                    if column.visuals().dark_mode {
                        egui::Color32::from_rgb(206, 121, 226)
                    } else {
                        egui::Color32::from_rgb(132, 69, 153)
                    },
                ),
                _ => ("Σ", egui::Color32::from_rgb(151, 158, 178)),
            };
            let max = values.iter().copied().fold(0.0_f64, f64::max);
            let mean = values.iter().sum::<f64>() / values.len().max(1) as f64;
            egui::Frame::group(column.style()).show(column, |ui| {
                ui.set_min_height(120.0);
                ui.colored_label(color, egui::RichText::new(symbol).size(18.0).strong());
                ui.strong(name.as_str());
                ui.monospace(format!("max  {max:.3e}"));
                ui.monospace(format!("mean {mean:.3e}"));
                ui.small(if sigma.is_some() {
                    "1σ uncertainty present"
                } else {
                    "no uncertainty carried"
                });
            });
        }
    });
    ui.monospace(format!("unit: {}", artifact.unit()));

    ui.add_space(14.0);
    ui.heading("Region dose-volume histogram");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Mask");
            ui.add(
                egui::TextEdit::singleline(&mut panel.mask_path)
                    .desired_width(360.0)
                    .hint_text("/path/to/region-mask.json"),
            );
            ui.label("Quantity");
            ui.add(
                egui::TextEdit::singleline(&mut panel.quantity)
                    .desired_width(170.0)
                    .hint_text("physical_total"),
            );
            if ui.button("Compute DVH").clicked() {
                panel.compute_histogram();
            }
        });
        if let Some(error) = &panel.histogram_error {
            ui.colored_label(theme.error, format!("DVH rejected: {error}"));
        }
        if let Some(histogram) = &panel.histogram {
            ui.label(format!(
                "region {} · {} voxels · {:.1} mm³ · {}",
                histogram.region,
                histogram.region_voxel_count,
                histogram.region_volume_mm3,
                histogram.unit
            ));
            show_dvh_curve(ui, histogram, theme);
        }
    });

    ui.add_space(14.0);
    ui.heading("Region dose-volume metrics");
    ui.label("Same `RegionDoseMetrics::compute` path as `openbnct metrics` and `compute_metrics` in Python.");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("D_x %:");
            ui.add(
                egui::TextEdit::singleline(&mut panel.metrics_dx)
                    .desired_width(90.0)
                    .hint_text("98,50,2"),
            );
            ui.label("V_x levels:");
            ui.add(
                egui::TextEdit::singleline(&mut panel.metrics_vx)
                    .desired_width(110.0)
                    .hint_text("dose units, optional"),
            );
            ui.label("EUD a:");
            ui.add(
                egui::TextEdit::singleline(&mut panel.metrics_eud)
                    .desired_width(80.0)
                    .hint_text("optional"),
            );
            if ui.button("Compute metrics").clicked() {
                panel.compute_metrics();
            }
        });
        if let Some(error) = &panel.metrics_error {
            ui.colored_label(theme.error, format!("Metrics rejected: {error}"));
        }
        if let Some(metrics) = &panel.metrics {
            ui.monospace(format!(
                "region {} · {} voxels · min {:.3e} · mean {:.3e} · max {:.3e} {}",
                metrics.region,
                metrics.region_voxel_count,
                metrics.minimum_dose,
                metrics.mean_dose,
                metrics.maximum_dose,
                metrics.unit,
            ));
            for metric in &metrics.dx {
                ui.monospace(format!(
                    "D{}  {:.3e} {}",
                    metric.percent, metric.dose, metrics.unit
                ));
            }
            for metric in &metrics.vx {
                ui.monospace(format!(
                    "V({:.3e} {})  {:.1}%",
                    metric.level,
                    metrics.unit,
                    metric.volume_fraction * 100.0,
                ));
            }
            for metric in &metrics.eud {
                ui.monospace(format!(
                    "EUD(a={})  {:.3e} {}",
                    metric.a, metric.dose, metrics.unit
                ));
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut panel.metrics_save_path)
                        .desired_width(300.0)
                        .hint_text("dose-metrics.json"),
                );
                if ui.button("Write metrics").clicked() {
                    panel.save_metrics();
                }
            });
            if let Some(status) = &panel.metrics_status {
                ui.colored_label(GateState::Verified.color(ui.visuals().dark_mode), status);
            }
        }
    });

    ui.add_space(14.0);
    ui.heading("NIfTI volumes");
    ui.label("Same `openbnct-nifti` paths as `openbnct nifti` — sform-preferred RAS→LPS handling.");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Volume");
            ui.add(
                egui::TextEdit::singleline(&mut nifti.input_path)
                    .desired_width(300.0)
                    .hint_text("/path/to/volume.nii[.gz]"),
            );
            if ui.button("Inspect").clicked() {
                nifti.inspect();
            }
            if ui.button("Browse…").clicked()
                && let Some(file) = rfd::FileDialog::new()
                    .add_filter("NIfTI", &["nii", "gz"])
                    .pick_file()
            {
                nifti.input_path = file.display().to_string();
                nifti.inspect();
            }
        });
        if let Some(image) = &nifti.image {
            let g = &image.geometry;
            ui.monospace(format!(
                "{}×{}×{} · spacing [{:.2}, {:.2}, {:.2}] mm · origin [{:.1}, {:.1}, {:.1}] mm",
                g.shape[0],
                g.shape[1],
                g.shape[2],
                g.spacing_mm[0],
                g.spacing_mm[1],
                g.spacing_mm[2],
                g.origin_mm[0],
                g.origin_mm[1],
                g.origin_mm[2],
            ));
            ui.monospace(format!(
                "transform: {} · datatype {} · {}",
                image.transform_source,
                image.datatype,
                if image.units_declared_mm {
                    "mm units declared"
                } else {
                    "mm units assumed (unspecified)"
                }
            ));
            ui.horizontal(|ui| {
                ui.label("Mask name");
                ui.add(
                    egui::TextEdit::singleline(&mut nifti.mask_name)
                        .desired_width(110.0)
                        .hint_text("region name"),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut nifti.mask_output)
                        .desired_width(240.0)
                        .hint_text("region-mask.json"),
                );
                if ui.button("Write mask").clicked() {
                    nifti.write_mask();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Resample to bundle grid");
                ui.radio_value(&mut nifti.resample_nearest, true, "nearest");
                ui.radio_value(&mut nifti.resample_nearest, false, "trilinear");
                ui.add(
                    egui::TextEdit::singleline(&mut nifti.resample_output)
                        .desired_width(200.0)
                        .hint_text("resampled.nii"),
                );
                if ui.button("Resample").clicked() {
                    nifti.resample();
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("Bundle / case");
            ui.add(
                egui::TextEdit::singleline(&mut nifti.bundle_path)
                    .desired_width(280.0)
                    .hint_text("dose-bundle.json or case.json (resample target)"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Export dose");
            ui.add(
                egui::TextEdit::singleline(&mut nifti.export_quantity)
                    .desired_width(130.0)
                    .hint_text("physical_total"),
            );
            ui.add(
                egui::TextEdit::singleline(&mut nifti.export_output)
                    .desired_width(200.0)
                    .hint_text("dose.nii"),
            );
            if ui.button("Export").clicked() {
                nifti.export_dose();
            }
        });
        if let Some(error) = &nifti.error {
            ui.colored_label(theme.error, format!("NIfTI rejected: {error}"));
        }
        if let Some(status) = &nifti.status {
            ui.colored_label(GateState::Verified.color(ui.visuals().dark_mode), status);
        }
    });
}

/// Draw the cumulative V(d) curve directly — no plotting dependency.
fn show_dvh_curve(ui: &mut egui::Ui, histogram: &DoseVolumeHistogram, theme: Theme) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width(), 180.0),
        egui::Sense::hover(),
    );
    let rect = response.rect.shrink2(egui::vec2(46.0, 12.0));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(72, 82, 99)),
        egui::StrokeKind::Inside,
    );
    let max_dose = histogram.dose_edges.last().copied().unwrap_or(0.0);
    if max_dose <= 0.0 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "zero dose in region",
            egui::FontId::monospace(12.0),
            theme.text_dim,
        );
        return;
    }
    let points: Vec<egui::Pos2> = histogram
        .dose_edges
        .iter()
        .zip(&histogram.cumulative_volume_fraction)
        .map(|(dose, fraction)| {
            egui::pos2(
                rect.left() + (dose / max_dose) as f32 * rect.width(),
                rect.bottom() - (*fraction as f32) * rect.height(),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        points,
        egui::Stroke::new(2.0, theme.brand),
    ));
    painter.text(
        egui::pos2(rect.left() - 8.0, rect.top()),
        egui::Align2::RIGHT_CENTER,
        "100%",
        egui::FontId::monospace(10.0),
        theme.text_dim,
    );
    painter.text(
        egui::pos2(rect.left() - 8.0, rect.bottom()),
        egui::Align2::RIGHT_CENTER,
        "0%",
        egui::FontId::monospace(10.0),
        theme.text_dim,
    );
    painter.text(
        egui::pos2(rect.right(), rect.bottom() + 4.0),
        egui::Align2::RIGHT_TOP,
        format!("{:.3e} {}", max_dose, histogram.unit),
        egui::FontId::monospace(10.0),
        theme.text_dim,
    );
}

fn show_evidence_workspace(
    ui: &mut egui::Ui,
    case: Option<&ViewerCase>,
    panel: &mut EvidencePanel,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        "Evidence",
        "Qualification is a chain of scoped claims, not one global green check.",
    );

    let geometry_detail = case.map_or_else(
        || "No runtime case has been verified in this session.".to_owned(),
        |case| {
            format!(
                "{} DICOM artifacts verified for {}.",
                case.verified.report.verified_artifact_count, case.verified.report.case_id
            )
        },
    );
    let manifest_hash = short_evidence_hash(OPENMC_MANIFEST_EVIDENCE);
    let execution_hash = short_evidence_hash(NJOY_EXECUTION_EVIDENCE);
    let comparison_hash = short_evidence_hash(HEATING_COMPARISON_EVIDENCE);
    let ledger = ui.scope(|ui| {
        show_evidence_row(
            ui,
            "Runtime geometry gate",
            if case.is_some() {
                GateState::Verified
            } else {
                GateState::InputRequired
            },
            &geometry_detail,
            None,
        );
        show_evidence_row(
            ui,
            "Official OpenMC processed selection",
            GateState::Frozen,
            "Case manifest binds cross_sections.xml, ten neutron tables, and five photon tables.",
            Some(&manifest_hash),
        );
        show_evidence_row(
            ui,
            "Controlled NJOY2016.78 execution",
            GateState::Blocked,
            "Preserved rejected evidence: 72 kinematic findings across four nuclides.",
            Some(&execution_hash),
        );
        show_evidence_row(
            ui,
            "OpenMC / NJOY MT 301 comparison",
            GateState::Frozen,
            "All ten curves agree within 4.9e-7; O-17/O-18 local fallback remains explicit.",
            Some(&comparison_hash),
        );
    });
    tour_targets.set(TourTarget::EvidenceLedger, ledger.response.rect);

    ui.add_space(12.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.heading("Qualification ceiling");
        ui.label(
            egui::RichText::new("synthetic_research_only")
                .monospace()
                .strong(),
        );
        ui.label(
            "Acquisition identity, transport capability, response suitability, execution, "
                .to_owned()
                + "cross-code comparison, and experimental validation remain separate claims.",
        );
    });

    ui.add_space(14.0);
    ui.heading("Exported evidence bundle");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Bundle root");
            ui.add(
                egui::TextEdit::singleline(&mut panel.root)
                    .desired_width(460.0)
                    .hint_text("/path/to/evidence-bundle"),
            );
            if ui.button("Verify manifest + hashes").clicked() {
                panel.verify();
            }
        });
        if let Some(error) = &panel.error {
            ui.colored_label(theme.error, format!("Verification rejected: {error}"));
        }
        if let Some(manifest) = &panel.manifest {
            status_badge(ui, GateState::Verified, "ALL ARTIFACTS VERIFIED");
            let qualification = serde_json::to_value(manifest.qualification.clone())
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "unknown".into());
            ui.monospace(format!("case: {} · {}", manifest.case_id, qualification));
            for artifact in &manifest.artifacts {
                ui.horizontal(|ui| {
                    ui.monospace(format!("{:28}", artifact.role));
                    ui.label(artifact.path.clone());
                    ui.monospace(format!("sha256:{}…", &artifact.sha256[..12]));
                });
            }
        }
    });
}

fn short_evidence_hash(bytes: &[u8]) -> String {
    sha256_hex(bytes)[..12].to_owned()
}

fn show_evidence_row(
    ui: &mut egui::Ui,
    title: &str,
    state: GateState,
    detail: &str,
    hash: Option<&str>,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.colored_label(state.color(ui.visuals().dark_mode), "●");
            ui.vertical(|ui| {
                ui.strong(title);
                ui.label(detail);
                if let Some(hash) = hash {
                    ui.monospace(format!("sha256:{hash}…"));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_badge(ui, state, state.label())
            });
        });
    });
}

fn show_case_summary(ui: &mut egui::Ui, case: &ViewerCase) {
    ui.heading("Case");
    ui.strong(case.verified.report.case_id);
    ui.small(case.root.display().to_string());
    ui.label(format!(
        "Grid: {:?} at {:?} mm",
        case.verified.report.shape, case.verified.report.spacing_mm
    ));
    ui.colored_label(
        GateState::Verified.color(ui.visuals().dark_mode),
        format!(
            "Integrity verified: {} DICOM files",
            case.verified.report.verified_artifact_count
        ),
    );
    ui.label("Qualification: synthetic research only");

    ui.separator();
    ui.heading("Crosshair");
    let voxel = case.crosshair.voxel();
    let world = case
        .crosshair
        .world_lps_mm(&case.grid)
        .unwrap_or([f64::NAN; 3]);
    ui.monospace(format!("voxel [{}, {}, {}]", voxel[0], voxel[1], voxel[2]));
    ui.monospace(format!(
        "LPS [{:.1}, {:.1}, {:.1}] mm",
        world[0], world[1], world[2]
    ));
    if let Ok(index) = case.grid.linear_index(voxel) {
        let stored = case.verified.ct.stored_pixels[index];
        ui.monospace(format!(
            "CT {:.1} HU (stored {stored})",
            case.verified.ct.modality_value(stored)
        ));
        let names: Vec<_> = case
            .verified
            .structures
            .rois
            .iter()
            .filter(|roi| roi.voxels[index])
            .map(|roi| roi.name.as_str())
            .collect();
        ui.label(if names.is_empty() {
            "ROIs: none".into()
        } else {
            format!("ROIs: {}", names.join(", "))
        });
        if let Some(wash) = case.dose.wash() {
            ui.monospace(format!("dose {:.3e} {}", wash.values[index], wash.unit));
        }
    }
}

fn show_display_controls(
    ui: &mut egui::Ui,
    case: &mut ViewerCase,
    display: &mut DisplaySettings,
    theme: Theme,
) -> bool {
    let mut changed = false;
    ui.strong("Window & overlays");
    changed |= ui
        .add(egui::Slider::new(&mut display.window_center, -1_024.0..=3_071.0).text("level HU"))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut display.window_width, 1.0..=4_096.0).text("width HU"))
        .changed();
    changed |= ui
        .add(egui::Slider::new(&mut display.overlay_opacity, 0.0..=1.0).text("ROI opacity"))
        .changed();

    ui.separator();
    ui.strong("Crosshair position");
    let mut voxel = case.crosshair.voxel();
    for (axis, label) in ["column / L", "row / P", "slice / S"]
        .into_iter()
        .enumerate()
    {
        changed |= ui
            .add(
                egui::Slider::new(&mut voxel[axis], 0..=case.grid.geometry().shape[axis] - 1)
                    .text(label),
            )
            .changed();
    }
    if voxel != case.crosshair.voxel() {
        let _ = case.crosshair.set_voxel(&case.grid, voxel);
    }

    ui.separator();
    ui.strong("Structures");
    for ((visible, roi), color) in case
        .roi_visible
        .iter_mut()
        .zip(&case.verified.structures.rois)
        .zip(
            case.verified
                .structures
                .rois
                .iter()
                .map(|roi| roi_color(roi.number)),
        )
    {
        ui.horizontal(|ui| {
            ui.colored_label(color, "■");
            changed |= ui.checkbox(visible, &roi.name).changed();
        });
    }

    ui.separator();
    ui.strong("Dose overlay");
    ui.label("Load a dose bundle for this case to wash it over the image.");
    ui.horizontal(|ui| {
        ui.label("bundle");
        ui.add(
            egui::TextEdit::singleline(&mut case.dose.path)
                .hint_text("dose-bundle.json")
                .desired_width(160.0),
        );
    });
    if ui.button("Load dose bundle").clicked() {
        case.dose.load(&case.verified);
        changed = true;
    }
    if let Some(error) = &case.dose.error {
        ui.colored_label(theme.error, error);
    }
    if let Some(loaded) = &case.dose.loaded {
        let labels: Vec<String> = loaded
            .artifact
            .rows()
            .iter()
            .map(|(name, ..)| {
                if name.ends_with("total") {
                    name.clone()
                } else {
                    format!("component:{name}")
                }
            })
            .collect();
        let meta = format!(
            "{} · {} · {}",
            loaded.artifact.case_id(),
            loaded.artifact.unit(),
            loaded.artifact.qualification()
        );
        ui.monospace(meta);
        let selected = case.dose.quantity.clone();
        egui::ComboBox::from_label("quantity")
            .selected_text(if selected.is_empty() {
                "choose".to_owned()
            } else {
                selected.clone()
            })
            .show_ui(ui, |ui| {
                for label in &labels {
                    if ui.selectable_label(selected == *label, label).clicked() {
                        case.dose.quantity = label.clone();
                        changed = true;
                    }
                }
            });
        changed |= ui
            .checkbox(&mut case.dose.enabled, "show dose wash")
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut case.dose.opacity, 0.0..=1.0).text("dose opacity"))
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut case.dose.threshold_percent, 0.0..=100.0)
                    .text("wash ≥ % of max"),
            )
            .changed();
    }
    changed
}

fn show_slice_view(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    view: SliceView,
    crosshair: Crosshair,
    max_height: f32,
) -> Option<[u32; 3]> {
    ui.strong(format!(
        "{} — index {}",
        view.plane().name(),
        view.fixed_index()
    ));
    let dimensions = view.dimensions();
    let spacing = view.pixel_spacing_mm();
    let physical_aspect = dimensions[0] as f64 * spacing[0] / (dimensions[1] as f64 * spacing[1]);
    let max_width = ui.available_width().max(1.0);
    let width = max_width.min(max_height * physical_aspect as f32);
    let image_size = egui::vec2(width, width / physical_aspect as f32);
    let (slot, _) =
        ui.allocate_exact_size(egui::vec2(max_width, image_size.y), egui::Sense::hover());
    let rect = egui::Rect::from_center_size(slot.center(), image_size);
    let response = ui.interact(
        rect,
        ui.id().with(view.plane().name()),
        egui::Sense::click_and_drag(),
    );
    egui::Image::from_texture(texture).paint_at(ui, rect);
    paint_orientation_and_crosshair(ui, rect, view, crosshair);

    if (response.clicked() || response.dragged())
        && let Some(position) = response.interact_pointer_pos()
        && rect.contains(position)
    {
        let fraction = [
            (position.x - rect.left()) / rect.width(),
            (position.y - rect.top()) / rect.height(),
        ];
        return view.voxel_at_fraction(fraction).ok();
    }
    None
}

fn paint_orientation_and_crosshair(
    ui: &egui::Ui,
    rect: egui::Rect,
    view: SliceView,
    crosshair: Crosshair,
) {
    let painter = ui.painter();
    let labels = view.edge_labels();
    let font = egui::FontId::proportional(15.0);
    let label_color = egui::Color32::WHITE;
    painter.text(
        rect.left_center() + egui::vec2(5.0, 0.0),
        egui::Align2::LEFT_CENTER,
        labels.left,
        font.clone(),
        label_color,
    );
    painter.text(
        rect.right_center() - egui::vec2(5.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        labels.right,
        font.clone(),
        label_color,
    );
    painter.text(
        rect.center_top() + egui::vec2(0.0, 5.0),
        egui::Align2::CENTER_TOP,
        labels.top,
        font.clone(),
        label_color,
    );
    painter.text(
        rect.center_bottom() - egui::vec2(0.0, 5.0),
        egui::Align2::CENTER_BOTTOM,
        labels.bottom,
        font,
        label_color,
    );

    if let Ok(pixel) = view.pixel_for_voxel(crosshair.voxel()) {
        let dimensions = view.dimensions();
        let x = rect.left() + (pixel[0] as f32 + 0.5) / dimensions[0] as f32 * rect.width();
        let y = rect.top() + (pixel[1] as f32 + 0.5) / dimensions[1] as f32 * rect.height();
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(0, 255, 255));
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            stroke,
        );
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            stroke,
        );
    }
}

fn render_slice(
    case: &VerifiedBenchmarkCase,
    view: SliceView,
    roi_visible: &[bool],
    wash: Option<DoseWash<'_>>,
    display: &DisplaySettings,
) -> Result<egui::ColorImage, String> {
    let dimensions = view.dimensions();
    let mut pixels = Vec::with_capacity(dimensions[0] as usize * dimensions[1] as usize);
    for vertical in 0..dimensions[1] {
        for horizontal in 0..dimensions[0] {
            let index = view
                .linear_index_at([horizontal, vertical])
                .map_err(|error| error.to_string())?;
            let stored = case.ct.stored_pixels[index];
            let gray = window_to_gray(
                case.ct.modality_value(stored),
                display.window_center,
                display.window_width,
            );
            let mut color = egui::Color32::from_gray(gray);
            if let Some(wash) = &wash {
                let dose = wash.values[index];
                if dose >= wash.floor {
                    color = blend(color, dose_wash_color(dose / wash.max), wash.opacity);
                }
            }
            for ((visible, roi), overlay) in roi_visible
                .iter()
                .zip(&case.structures.rois)
                .zip(case.structures.rois.iter().map(|roi| roi_color(roi.number)))
            {
                if *visible && roi.voxels[index] {
                    color = blend(color, overlay, display.overlay_opacity);
                }
            }
            pixels.push(color);
        }
    }
    Ok(egui::ColorImage::new(
        [dimensions[0] as usize, dimensions[1] as usize],
        pixels,
    ))
}

fn window_to_gray(value: f64, center: f64, width: f64) -> u8 {
    let width = width.max(1.0);
    if width <= 1.0 {
        return if value <= center - 0.5 { 0 } else { 255 };
    }
    let lower = center - 0.5 - (width - 1.0) / 2.0;
    let upper = center - 0.5 + (width - 1.0) / 2.0;
    if value <= lower {
        0
    } else if value > upper {
        255
    } else {
        (((value - (center - 0.5)) / (width - 1.0) + 0.5) * 255.0).round() as u8
    }
}

fn blend(base: egui::Color32, overlay: egui::Color32, opacity: f32) -> egui::Color32 {
    let opacity = opacity.clamp(0.0, 1.0);
    let channel = |base: u8, overlay: u8| {
        (f32::from(base) * (1.0 - opacity) + f32::from(overlay) * opacity).round() as u8
    };
    egui::Color32::from_rgb(
        channel(base.r(), overlay.r()),
        channel(base.g(), overlay.g()),
        channel(base.b(), overlay.b()),
    )
}

/// Classic "hot" dose-wash ramp: black → red → orange → yellow → white
/// over the normalized fraction `t` of the field maximum.
fn dose_wash_color(t: f64) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgb(
        channel(3.0 * t),
        channel(3.0 * t - 1.0),
        channel(3.0 * t - 2.0),
    )
}

fn roi_color(number: i32) -> egui::Color32 {
    match number {
        1 => egui::Color32::from_rgb(180, 180, 180),
        2 => egui::Color32::from_rgb(255, 80, 80),
        3 => egui::Color32::from_rgb(80, 255, 80),
        4 => egui::Color32::from_rgb(80, 80, 255),
        5 => egui::Color32::from_rgb(255, 255, 0),
        _ => egui::Color32::from_rgb(255, 0, 255),
    }
}

fn show_empty_state(ui: &mut egui::Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(90.0);
        ui.heading("No verified synthetic case loaded");
        ui.label("Generate the frozen case from a terminal:");
        ui.monospace("cargo run --bin openbnct -- benchmark generate /tmp/nf-bnct-001");
        ui.label("Then load that directory above, or pass it as the first GUI argument.");
        ui.add_space(20.0);
        ui.label("DICOM and artifact-integrity gates run before any image is displayed.");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voi_window_has_expected_endpoints_and_midpoint() {
        assert_eq!(window_to_gray(-200.0, 0.0, 400.0), 0);
        assert_eq!(window_to_gray(199.0, 0.0, 400.0), 255);
        assert_eq!(window_to_gray(-0.5, 0.0, 400.0), 128);
    }

    #[test]
    fn overlay_blending_respects_zero_and_full_opacity() {
        let base = egui::Color32::from_rgb(10, 20, 30);
        let overlay = egui::Color32::from_rgb(210, 120, 60);
        assert_eq!(blend(base, overlay, 0.0), base);
        assert_eq!(blend(base, overlay, 1.0), overlay);
    }

    #[test]
    fn dropped_paths_route_to_the_matching_workbench_surface() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(classify_dropped_path(dir.path()), DropTarget::Case);
        assert_eq!(
            classify_dropped_path(Path::new("/tmp/dose-bundle.json")),
            DropTarget::DoseBundle
        );
        assert_eq!(
            classify_dropped_path(Path::new("/tmp/DOSE.JSON")),
            DropTarget::DoseBundle
        );
        assert_eq!(
            classify_dropped_path(Path::new("/tmp/ct.nii")),
            DropTarget::NiftiVolume
        );
        assert_eq!(
            classify_dropped_path(Path::new("/tmp/ct.nii.gz")),
            DropTarget::NiftiVolume
        );
        assert_eq!(
            classify_dropped_path(Path::new("/tmp/notes.txt")),
            DropTarget::Unsupported
        );
    }

    #[test]
    fn workspace_navigation_has_stable_unique_labels() {
        let labels = WorkspaceTab::ALL.map(WorkspaceTab::label);
        assert_eq!(labels.len(), 6);
        assert!(labels.iter().all(|label| !label.is_empty()));
        for (index, left) in labels.iter().enumerate() {
            assert!(!labels[index + 1..].contains(left));
        }
    }

    #[test]
    fn readiness_never_promotes_the_blocked_response_or_transport_run() {
        for case_loaded in [false, true] {
            let gates = readiness_gates(case_loaded);
            assert_eq!(gates[3].state, GateState::Blocked);
            assert_eq!(gates[4].state, GateState::Pending);
        }
        assert_eq!(readiness_gates(false)[0].state, GateState::InputRequired);
        assert_eq!(readiness_gates(true)[0].state, GateState::Verified);
    }

    #[test]
    fn displayed_evidence_hashes_are_derived_from_frozen_bytes() {
        assert_eq!(
            sha256_hex(OPENMC_MANIFEST_EVIDENCE),
            "3eaae09921172199c34f3fb236ae082ea5ace4567e0e04d2afcce357add73fb1"
        );
        assert_eq!(
            sha256_hex(NJOY_EXECUTION_EVIDENCE),
            "65a21b57507e76a68b77349e92390ae03ebb8c38f6ed6cee66197aa5ee4adea7"
        );
        assert_eq!(
            sha256_hex(HEATING_COMPARISON_EVIDENCE),
            "e9b1ffc5e70e3e489f23f9e185d12a5edeb7525161eb3b81470233d33f36f1e7"
        );
    }

    fn physical_bundle_json() -> serde_json::Value {
        let reference = |id: &str| serde_json::json!({"id": id, "sha256": "a".repeat(64)});
        let component = |name: &str, value: f64, sigma: f64| {
            serde_json::json!({
                "component": name,
                "unit": "gray_per_source_particle",
                "values": [value, value],
                "absolute_standard_uncertainty": [sigma, sigma],
            })
        };
        serde_json::json!({
            "schema_version": "openbnct.physical-dose-bundle/0.2.0",
            "case_id": "synthetic-case",
            "frame_of_reference_uid": null,
            "geometry": {
                "shape": [2, 1, 1],
                "spacing_mm": [5.0, 5.0, 5.0],
                "origin_mm": [-2.5, -2.5, -2.5],
                "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            "component_profile": reference("profile"),
            "response_set": reference("response"),
            "components": [
                component("boron", 1.0e-12, 1.0e-14),
                component("nitrogen", 2.0e-13, 2.0e-15),
                component("hydrogen", 5.0e-14, 5.0e-16),
                component("photon", 3.0e-13, 3.0e-15),
            ],
            "physical_total": {
                "unit": "gray_per_source_particle",
                "values": [1.75e-12, 1.75e-12],
                "absolute_standard_uncertainty": [1.1e-14, 1.1e-14],
                "uncertainty_method": "dedicated_estimator",
            },
            "provenance_id": "test-provenance",
        })
    }

    #[test]
    fn position_panel_aims_rotates_and_saves() {
        let scratch = tempfile::tempdir().unwrap();
        let case_root = scratch.path().join("case");
        openbnct_dicom::synthetic::generate_nf_bnct_001(&case_root).unwrap();
        let case = ViewerCase::load(&case_root).unwrap();
        let mut panel = PositionPanel {
            source_path: concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../benchmarks/synthetic/nf-bnct-001/transport/source.json"
            )
            .into(),
            half_width_u_cm: "2.0".into(),
            half_width_v_cm: "2.0".into(),
            margin_cm: "0.1".into(),
            ..Default::default()
        };
        panel.aim(&case);
        assert!(panel.error.is_none(), "aim rejected: {:?}", panel.error);
        let report = panel.report.as_ref().unwrap();
        assert_eq!(report.case_id, case.verified.report.case_id);
        assert_eq!(report.schema_version, "openbnct.position-report/0.1.0");
        assert!(panel.positioned.is_some());

        panel.rotate_axis = 1; // y
        panel.rotate_degrees = "90".into();
        panel.rotate();
        assert!(panel.error.is_none(), "rotate rejected: {:?}", panel.error);

        let source_out = scratch.path().join("positioned-source.json");
        let report_out = scratch.path().join("position-report.json");
        panel.save_source_path = source_out.to_string_lossy().into();
        panel.save_report_path = report_out.to_string_lossy().into();
        panel.save(true);
        panel.save(false);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&source_out).unwrap()).unwrap();
        // The committed source fixture still carries its original `nctforge.`
        // artifact id; save passes the identifier through unchanged.
        assert_eq!(saved["id"], "nctforge.nf-bnct-001.source.v1");
        let report_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report_out).unwrap()).unwrap();
        assert_eq!(report_json["entry_axis"], "x");
        assert_eq!(report_json["entry_side"], "low");
    }

    #[test]
    fn dose_artifact_loads_validates_and_exposes_component_rows() {
        let scratch = tempfile::tempdir().unwrap();
        let path = scratch.path().join("bundle.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&physical_bundle_json()).unwrap(),
        )
        .unwrap();
        let loaded = DoseArtifact::load(&path).unwrap();
        assert!(matches!(loaded.artifact, DoseArtifact::Physical(_)));
        assert_eq!(loaded.artifact.case_id(), "synthetic-case");
        assert_eq!(loaded.artifact.qualification(), "synthetic_research_only");
        let rows = loaded.artifact.rows();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[4].0, "physical_total");
        assert_eq!(rows[4].1, &[1.75e-12, 1.75e-12]);
        assert_eq!(loaded.sha256.len(), 64);
    }

    #[test]
    fn dose_overlay_loads_washes_and_rejects_mismatched_grids() {
        let scratch = tempfile::tempdir().unwrap();
        let case_root = scratch.path().join("case");
        openbnct_dicom::synthetic::generate_nf_bnct_001(&case_root).unwrap();
        let mut case = ViewerCase::load(&case_root).unwrap();

        let voxels: usize = case
            .verified
            .ct
            .geometry
            .shape
            .iter()
            .map(|n| *n as usize)
            .product();
        let values: Vec<f64> = (0..voxels).map(|i| i as f64).collect();
        let components: Vec<serde_json::Value> = ["boron", "nitrogen", "hydrogen", "photon"]
            .iter()
            .map(|name| {
                serde_json::json!({
                    "component": name,
                    "unit": "gray_per_source_particle",
                    "values": values,
                    "absolute_standard_uncertainty": serde_json::Value::Null,
                })
            })
            .collect();
        let bundle = serde_json::json!({
            "schema_version": "openbnct.physical-dose-bundle/0.2.0",
            "case_id": case.verified.report.case_id,
            "frame_of_reference_uid": null,
            "geometry": serde_json::to_value(&case.verified.ct.geometry).unwrap(),
            "component_profile": {"id": "p", "sha256": "a".repeat(64)},
            "response_set": {"id": "r", "sha256": "b".repeat(64)},
            "components": components,
            "physical_total": {
                "unit": "gray_per_source_particle",
                "values": values,
                "absolute_standard_uncertainty": serde_json::Value::Null,
                "uncertainty_method": "unavailable",
            },
            "provenance_id": "test-provenance",
        });
        let path = scratch.path().join("dose.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();

        case.dose.path = path.to_string_lossy().into();
        case.dose.load(&case.verified);
        assert!(
            case.dose.error.is_none(),
            "load rejected: {:?}",
            case.dose.error
        );
        assert_eq!(case.dose.quantity, "physical_total");
        let wash = case.dose.wash().expect("wash must resolve");
        assert_eq!(wash.max, (voxels - 1) as f64);
        assert_eq!(wash.unit, "gray_per_source_particle");
        assert_eq!(wash.values.len(), voxels);

        case.dose.quantity = "component:boron".into();
        assert!(case.dose.wash().is_some());
        case.dose.quantity = "component:missing".into();
        assert!(case.dose.wash().is_none());
        case.dose.quantity = "physical_total".into();
        case.dose.enabled = false;
        assert!(case.dose.wash().is_none());
        case.dose.enabled = true;

        // Hot ramp endpoints.
        assert_eq!(dose_wash_color(1.0), egui::Color32::WHITE);
        assert_eq!(dose_wash_color(0.0), egui::Color32::BLACK);

        // A foreign case_id must not overlay on this patient.
        let mut foreign = bundle.clone();
        foreign["case_id"] = serde_json::json!("other-case");
        let foreign_path = scratch.path().join("foreign.json");
        std::fs::write(&foreign_path, serde_json::to_vec_pretty(&foreign).unwrap()).unwrap();
        case.dose.path = foreign_path.to_string_lossy().into();
        case.dose.load(&case.verified);
        assert!(case.dose.loaded.is_none());
        assert!(
            case.dose.error.as_deref().unwrap().contains("case_id"),
            "unexpected error: {:?}",
            case.dose.error
        );

        // A same-case bundle on a different grid must not overlay either.
        let mut off_grid = bundle.clone();
        off_grid["geometry"]["shape"] = serde_json::json!([2, 1, 1]);
        for component in off_grid["components"].as_array_mut().unwrap() {
            component["values"] = serde_json::json!([1.0, 1.0]);
        }
        off_grid["physical_total"]["values"] = serde_json::json!([1.0, 1.0]);
        let off_path = scratch.path().join("off-grid.json");
        std::fs::write(&off_path, serde_json::to_vec_pretty(&off_grid).unwrap()).unwrap();
        case.dose.path = off_path.to_string_lossy().into();
        case.dose.load(&case.verified);
        assert!(case.dose.loaded.is_none());
        assert!(
            case.dose.error.as_deref().unwrap().contains("grid"),
            "unexpected error: {:?}",
            case.dose.error
        );
    }

    #[test]
    fn dose_artifact_rejects_unknown_schema_and_invalid_bundles() {
        let scratch = tempfile::tempdir().unwrap();
        let bad_schema = scratch.path().join("bad.json");
        std::fs::write(&bad_schema, b"{\"schema_version\": \"other/9.9.9\"}").unwrap();
        assert!(DoseArtifact::load(&bad_schema).is_err());

        let mut invalid = physical_bundle_json();
        invalid["physical_total"]["values"] = serde_json::json!([1.0]);
        let invalid_path = scratch.path().join("invalid.json");
        std::fs::write(&invalid_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
        assert!(DoseArtifact::load(&invalid_path).is_err());
    }

    #[test]
    fn dose_panel_computes_region_metrics_and_reports_rejections() {
        let scratch = tempfile::tempdir().unwrap();
        let bundle_path = scratch.path().join("bundle.json");
        std::fs::write(
            &bundle_path,
            serde_json::to_vec_pretty(&physical_bundle_json()).unwrap(),
        )
        .unwrap();
        let mask_path = scratch.path().join("mask.json");
        std::fs::write(
            &mask_path,
            serde_json::to_vec(&serde_json::json!({
                "name": "all", "voxels": [true, true]
            }))
            .unwrap(),
        )
        .unwrap();

        let mut panel = DosePanel {
            bundle_path: bundle_path.to_string_lossy().into(),
            mask_path: mask_path.to_string_lossy().into(),
            quantity: "physical_total".into(),
            metrics_dx: "50".into(),
            metrics_vx: "1e-12".into(),
            metrics_eud: "1".into(),
            ..Default::default()
        };
        panel.load_bundle();
        assert!(
            panel.bundle.is_some(),
            "load rejected: {:?}",
            panel.bundle_error
        );
        panel.compute_metrics();
        assert!(
            panel.metrics_error.is_none(),
            "metrics rejected: {:?}",
            panel.metrics_error
        );
        let metrics = panel.metrics.as_ref().unwrap();
        assert_eq!(metrics.region, "all");
        assert_eq!(metrics.region_voxel_count, 2);
        assert_eq!(metrics.mean_dose, 1.75e-12);
        assert_eq!(metrics.dx[0].percent, 50.0);
        assert_eq!(metrics.dx[0].dose, 1.75e-12);
        assert_eq!(metrics.vx[0].volume_fraction, 1.0);
        assert_eq!(metrics.eud[0].dose, 1.75e-12);

        let out = scratch.path().join("metrics.json");
        panel.metrics_save_path = out.to_string_lossy().into();
        panel.save_metrics();
        assert!(panel.metrics_status.is_some());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(saved["schema_version"], "openbnct.dose-metrics/0.1.0");

        panel.quantity = "nonsense".into();
        panel.compute_metrics();
        assert!(panel.metrics.is_none());
        assert!(
            panel
                .metrics_error
                .as_deref()
                .unwrap()
                .contains("unknown quantity")
        );

        panel.quantity = "physical_total".into();
        panel.metrics_dx = "0".into();
        panel.compute_metrics();
        assert!(panel.metrics.is_none());
        assert!(panel.metrics_error.is_some());
    }

    #[test]
    fn nifti_panel_exports_inspects_masks_and_resamples() {
        let scratch = tempfile::tempdir().unwrap();
        let bundle_path = scratch.path().join("bundle.json");
        std::fs::write(
            &bundle_path,
            serde_json::to_vec_pretty(&physical_bundle_json()).unwrap(),
        )
        .unwrap();

        let mut panel = NiftiPanel {
            bundle_path: bundle_path.to_string_lossy().into(),
            export_quantity: "physical_total".into(),
            export_output: scratch.path().join("dose.nii").to_string_lossy().into(),
            ..Default::default()
        };
        panel.export_dose();
        assert!(panel.error.is_none(), "export rejected: {:?}", panel.error);
        assert!(panel.status.is_some());

        panel.input_path = panel.export_output.clone();
        panel.inspect();
        assert!(panel.error.is_none(), "inspect rejected: {:?}", panel.error);
        let image = panel.image.as_ref().unwrap();
        assert_eq!(image.geometry.shape, [2, 1, 1]);
        assert_eq!(image.values, vec![1.75e-12, 1.75e-12]);

        panel.mask_name = "total-field".into();
        panel.mask_output = scratch.path().join("mask.json").to_string_lossy().into();
        panel.write_mask();
        assert!(panel.error.is_none(), "mask rejected: {:?}", panel.error);
        let mask: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&panel.mask_output).unwrap()).unwrap();
        assert_eq!(mask["name"], "total-field");
        assert_eq!(mask["voxels"], serde_json::json!([true, true]));

        panel.resample_nearest = false;
        panel.resample_output = scratch
            .path()
            .join("resampled.nii")
            .to_string_lossy()
            .into();
        panel.resample();
        assert!(
            panel.error.is_none(),
            "resample rejected: {:?}",
            panel.error
        );
        let resampled = openbnct_nifti::read_nifti_file(Path::new(&panel.resample_output)).unwrap();
        assert_eq!(resampled.values, vec![1.75e-12, 1.75e-12]);

        // A transport case is an equally valid resample target (the
        // CT-aligned case grid), resolved through the same shared path.
        let case_path = scratch.path().join("case.json");
        std::fs::write(
            &case_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": "openbnct.transport-case/0.1.0",
                "geometry": {
                    "shape": [2, 1, 1],
                    "spacing_mm": [5.0, 5.0, 5.0],
                    "origin_mm": [-2.5, -2.5, -2.5],
                    "direction": [1.0,0.0,0.0,0.0,1.0,0.0,0.0,0.0,1.0],
                },
            }))
            .unwrap(),
        )
        .unwrap();
        panel.bundle_path = case_path.to_string_lossy().into();
        panel.resample();
        assert!(
            panel.error.is_none(),
            "case-target resample rejected: {:?}",
            panel.error
        );

        panel.image = None;
        panel.write_mask();
        assert!(panel.error.as_deref().unwrap().contains("Inspect"));
    }

    #[test]
    fn every_empty_workspace_renders_at_the_minimum_viewport() {
        let context = egui::Context::default();
        configure_style(&context);
        for mut workspace in WorkspaceTab::ALL {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(960.0, 640.0),
                )),
                ..Default::default()
            };
            let mut display = DisplaySettings::default();
            let mut tour_targets = TourTargets::default();
            let mut output = context.run_ui(input, |ui| {
                show_workbench(
                    ui,
                    &mut workspace,
                    None,
                    &mut display,
                    &mut WorkbenchPanels::default(),
                    &mut tour_targets,
                    Theme::resolve(false),
                );
            });
            output.textures_delta.clear();
        }
    }
}
