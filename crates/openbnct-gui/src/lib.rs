// SPDX-License-Identifier: Apache-2.0

#![forbid(unsafe_code)]

mod brand;
mod help;
mod i18n;
mod io;
mod run;
#[cfg(target_arch = "wasm32")]
mod web;

use std::array;
use std::path::{Path, PathBuf};

use eframe::egui;
use openbnct_bio::{BiologicalDoseBundle, RegionMask};
use openbnct_core::PhysicalDoseBundle;
use openbnct_dicom::{
    CtVolume, ImportedStudy, StructureSet, VerifiedBenchmarkCase, load_nf_bnct_001,
    load_nf_bnct_001_from_files,
};
use openbnct_evidence::{
    DoseVolumeHistogram, EvidenceBundleManifest, RegionDoseMetrics, sha256_hex,
};
use openbnct_openmc::{OpenMcBackend, TARGET_OPENMC_VERSION};
use openbnct_transport::TransportBackend;
use openbnct_view::{AnatomicalPlane, Crosshair, PatientAlignedGrid, SliceView, ViewError};

use help::{GuidedHelp, HelpWorkspace, TourTarget, TourTargets};
use i18n::Language;

// Vendored copies of the frozen NF-BNCT-001 benchmark artifacts so the crate
// packages standalone; the canonical versions live in benchmarks/synthetic.
const OPENMC_MANIFEST_EVIDENCE: &[u8] =
    include_bytes!("../assets/nf-bnct-001/provenance/openmc-endfb81-processed-data-manifest.json");
const NJOY_EXECUTION_EVIDENCE: &[u8] =
    include_bytes!("../assets/nf-bnct-001/provenance/njoy2016-78-execution-receipt.json");
const HEATING_COMPARISON_EVIDENCE: &[u8] =
    include_bytes!("../assets/nf-bnct-001/provenance/openmc-njoy-mt301-comparison.json");

// Debug-only native captures for visual review; no effect in normal launches.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
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

/// Launch the native desktop shell. WASM builds enter through
/// `start_web` in `web.rs` with no initial case path.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_native(initial_case: Option<PathBuf>) -> eframe::Result {
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

    const fn label_ja(self) -> &'static str {
        match self {
            Self::Overview => "概要",
            Self::Geometry => "ジオメトリ",
            Self::Transport => "輸送",
            Self::Plan => "計画",
            Self::Dose => "線量成分",
            Self::Evidence => "エビデンス",
        }
    }

    fn localized_label(self, language: Language) -> &'static str {
        match language {
            Language::Japanese => self.label_ja(),
            Language::English => self.label(),
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

    const fn label_ja(self) -> &'static str {
        match self {
            Self::Verified => "検証済み",
            Self::Frozen => "凍結",
            Self::Blocked => "ブロック",
            Self::Pending => "保留",
            Self::InputRequired => "入力待ち",
        }
    }

    fn localized_label(self, language: Language) -> &'static str {
        match language {
            Language::Japanese => self.label_ja(),
            Language::English => self.label(),
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

fn readiness_gates(case_loaded: bool, language: Language) -> [ReadinessGate; 5] {
    [
        ReadinessGate {
            title: t!(language, "DICOM geometry", "DICOM ジオメトリ"),
            detail: if case_loaded {
                t!(
                    language,
                    "Case artifacts and patient-space geometry passed the runtime gate.",
                    "症例アーティファクトと患者空間ジオメトリが実行時ゲートを通過。"
                )
            } else {
                t!(
                    language,
                    "Load NF-BNCT-001 to run the DICOM and integrity gate.",
                    "NF-BNCT-001 を読み込むと DICOM・完全性ゲートが実行されます。"
                )
            },
            state: if case_loaded {
                GateState::Verified
            } else {
                GateState::InputRequired
            },
        },
        ReadinessGate {
            title: t!(language, "Material and source", "材料と線源"),
            detail: t!(
                language,
                "Versioned NF-BNCT-001 benchmark contracts are checked in.",
                "バージョン管理された NF-BNCT-001 ベンチマーク契約はコミット済み。"
            ),
            state: GateState::Frozen,
        },
        ReadinessGate {
            title: t!(language, "OpenMC nuclear data", "OpenMC 核データ"),
            detail: t!(
                language,
                "Official case selection and 16 artifact identities are frozen.",
                "公式ケース選択と16件のアーティファクト同一性が凍結済み。"
            ),
            state: GateState::Frozen,
        },
        ReadinessGate {
            title: t!(language, "Component responses", "成分応答"),
            detail: t!(
                language,
                "O-17/O-18 transported-photon treatment requires independent review.",
                "O-17/O-18 輸送済み光子の取り扱いは独立レビューが必要。"
            ),
            state: GateState::Blocked,
        },
        ReadinessGate {
            title: t!(language, "Controlled transport run", "制御付き輸送実行"),
            detail: t!(
                language,
                "Disabled until every upstream scientific gate passes.",
                "上流の科学的ゲートがすべて通過するまで無効。"
            ),
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
        Self::load_bytes(io::read_bytes(path)?)
    }

    /// Web builds reach this through `io::dropped_bytes` — the same
    /// validation path runs on in-memory bytes.
    fn load_bytes(bytes: Vec<u8>) -> Result<LoadedDose, String> {
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

    /// The upstream binding each kind carries — physical bundles name
    /// their run manifest, biological bundles their parent physical dose.
    fn provenance_id(&self) -> &str {
        match self {
            Self::Physical(bundle) => &bundle.provenance_id,
            Self::Biological(bundle) => &bundle.physical_bundle_provenance,
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
    fn load(&mut self, case: &CaseData) {
        let outcome = DoseArtifact::load(Path::new(self.path.trim())).and_then(|dose| {
            if dose.artifact.case_id() != case.case_id {
                return Err(format!(
                    "dose case_id {:?} does not match this case {:?}",
                    dose.artifact.case_id(),
                    case.case_id
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

/// Interactive dose-map state: which component row is sliced, the
/// bundle-grid crosshair voxel, display options, and cached textures.
#[derive(Default)]
struct DoseMapView {
    voxel: Option<[u32; 3]>,
    quantity: String,
    log_scale: bool,
    contours: bool,
    /// Render the component's 1σ field instead of its value field.
    sigma_view: bool,
    textures: Option<[egui::TextureHandle; 3]>,
    /// Render inputs the textures were produced from — regenerated on change.
    cache_key: Option<String>,
}

/// Line-profile state: which grid axis the profile runs along, an
/// optional measurement-record overlay, and display options.
#[derive(Default)]
struct ProfileView {
    axis: usize,
    log_y: bool,
    measurement: Option<openbnct_transport::MeasurementRecord>,
    measurement_error: Option<String>,
    /// Which histogram-valued measurement inside the record is plotted.
    measurement_pick: usize,
}

/// UI state for the dose workspace: the loaded bundle plus the region mask
/// and quantity chosen for the DVH panel.
struct DosePanel {
    bundle_path: String,
    bundle: Option<LoadedDose>,
    bundle_error: Option<String>,
    map: DoseMapView,
    profile: ProfileView,
    /// Second bundle for the A/B diff — loaded via the compare drop zone.
    compare: Option<LoadedDose>,
    compare_error: Option<String>,
    /// Screen rect of the "drop B here" zone, painted last frame — drop
    /// routing needs it before the frame's widget tree runs.
    compare_zone: egui::Rect,
    compare_textures: Option<[egui::TextureHandle; 3]>,
    compare_cache_key: Option<String>,
    /// 478 keV production map derived from this bundle's boron
    /// component — also selectable as a dose-map quantity.
    prompt_gamma: Option<openbnct_transport::PromptGammaSource>,
    prompt_gamma_error: Option<String>,
    uncertainty_budget: Option<openbnct_transport::DoseUncertaintyBudget>,
    uncertainty_budget_error: Option<String>,
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
            map: DoseMapView::default(),
            profile: ProfileView::default(),
            compare: None,
            compare_error: None,
            compare_zone: egui::Rect::NOTHING,
            compare_textures: None,
            compare_cache_key: None,
            prompt_gamma: None,
            prompt_gamma_error: None,
            uncertainty_budget: None,
            uncertainty_budget_error: None,
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
            Ok(bundle) => self.accept_bundle(bundle),
            Err(error) => {
                self.bundle = None;
                self.bundle_error = Some(error);
            }
        }
    }

    /// Web drops land here — bytes validated identically to `load_bundle`.
    fn load_bundle_bytes(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        match DoseArtifact::load_bytes(bytes) {
            Ok(bundle) => {
                self.accept_bundle(bundle);
                Ok(())
            }
            Err(error) => {
                self.bundle = None;
                self.bundle_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Compare-slot loader — parses without touching bundle-A state.
    fn load_compare_bytes(&mut self, bytes: Vec<u8>) {
        match DoseArtifact::load_bytes(bytes) {
            Ok(bundle) => {
                self.compare_error = None;
                self.compare = Some(bundle);
            }
            Err(error) => {
                self.compare = None;
                self.compare_error = Some(error);
            }
        }
    }

    fn accept_bundle(&mut self, bundle: LoadedDose) {
        self.bundle_error = None;
        self.histogram = None;
        self.quantity = match &bundle.artifact {
            DoseArtifact::Physical(_) => "physical_total".into(),
            DoseArtifact::Biological(_) => "biological_total".into(),
        };
        self.map = DoseMapView::default();
        self.map.quantity = self.quantity.clone();
        self.map.voxel = Some(bundle.artifact.geometry().shape.map(|extent| extent / 2));
        // Default profile axis: the longest physical extent — the beam
        // direction for the phantom geometries this workbench handles.
        let geometry = bundle.artifact.geometry();
        self.profile.axis = (0..3)
            .max_by(|&a, &b| {
                (f64::from(geometry.shape[a]) * geometry.spacing_mm[a])
                    .total_cmp(&(f64::from(geometry.shape[b]) * geometry.spacing_mm[b]))
            })
            .unwrap_or(2);
        // A changed under a loaded B — the diff is stale until re-dropped.
        self.compare = None;
        self.compare_textures = None;
        self.compare_cache_key = None;
        self.bundle = Some(bundle);
    }

    /// Load a measurement-record JSON (bytes — drop-compatible on web)
    /// as the profile overlay. Keeps the first histogram-valued
    /// measurement selected.
    fn load_measurement_bytes(&mut self, bytes: &[u8]) {
        let outcome = serde_json::from_slice::<openbnct_transport::MeasurementRecord>(bytes)
            .map_err(|error| error.to_string())
            .and_then(|record| record.validate().map(|_| record).map_err(|e| e.to_string()));
        match outcome {
            Ok(record) => {
                self.profile.measurement_error = None;
                self.profile.measurement_pick = record
                    .measurements
                    .iter()
                    .position(|m| {
                        matches!(
                            m.value,
                            openbnct_transport::MeasurementValue::Histogram { .. }
                        )
                    })
                    .unwrap_or(0);
                self.profile.measurement = Some(record);
            }
            Err(error) => {
                self.profile.measurement = None;
                self.profile.measurement_error = Some(error);
            }
        }
    }

    /// Resolve the mask + quantity row selection shared by the DVH and
    /// metrics overlays. Returns the mask, quantity label, and the selected
    /// values (cloned so callers can mutate other panel fields freely).
    fn resolve_selection(&self) -> Result<(RegionMask, String, Vec<f64>), String> {
        let bundle = self.bundle.as_ref().ok_or("Load a dose bundle first.")?;
        let mask: RegionMask = io::read_bytes(Path::new(self.mask_path.trim()))
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
        let contents = serde_json::to_string_pretty(metrics).unwrap() + "\n";
        match io::write_bytes(Path::new(&path), contents.as_bytes()) {
            Ok(()) => self.metrics_status = Some(format!("wrote {path}")),
            Err(e) => self.metrics_error = Some(e),
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
        match io::read_bytes(Path::new(self.input_path.trim())) {
            Ok(bytes) => self.inspect_bytes(&bytes),
            Err(error) => {
                self.image = None;
                self.status = None;
                self.error = Some(error);
            }
        }
    }

    /// Web drops land here directly — same parse as `read_nifti_file`.
    fn inspect_bytes(&mut self, bytes: &[u8]) {
        self.image = None;
        self.error = None;
        self.status = None;
        match openbnct_nifti::read_nifti(bytes) {
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
        let contents = serde_json::to_string_pretty(&mask).unwrap() + "\n";
        match io::write_bytes(Path::new(output), contents.as_bytes()) {
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
    robustness: Option<openbnct_plan::robustness::PlanRobustnessReport>,
    robustness_error: Option<String>,
}

impl PlanPanel {
    fn load(&mut self) {
        match io::read_bytes(Path::new(self.plan_path.trim())) {
            Ok(bytes) => self.load_bytes(&bytes),
            Err(error) => {
                self.plan = None;
                self.issues.clear();
                self.export_status = None;
                self.error = Some(error);
            }
        }
    }

    /// Web drops land here directly — the same validation path as `load`.
    fn load_bytes(&mut self, bytes: &[u8]) {
        self.plan = None;
        self.issues.clear();
        self.error = None;
        self.export_status = None;
        match serde_json::from_slice::<openbnct_core::ExposurePlan>(bytes)
            .map_err(|e| e.to_string())
        {
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

    /// Bytes-only robustness-report loader — web drop path.
    fn load_robustness_bytes(&mut self, bytes: &[u8]) {
        match serde_json::from_slice::<openbnct_plan::robustness::PlanRobustnessReport>(bytes)
            .map_err(|e| e.to_string())
            .and_then(|report| {
                if openbnct_core::schema_matches(
                    &report.schema_version,
                    openbnct_plan::robustness::PLAN_ROBUSTNESS_SCHEMA,
                ) {
                    Ok(report)
                } else {
                    Err(format!(
                        "unsupported schema {:?} — expected {}",
                        report.schema_version,
                        openbnct_plan::robustness::PLAN_ROBUSTNESS_SCHEMA
                    ))
                }
            }) {
            Ok(report) => {
                self.robustness_error = None;
                self.robustness = Some(report);
            }
            Err(error) => {
                self.robustness = None;
                self.robustness_error = Some(error);
            }
        }
    }
}

/// Run-panel state: the command line, the live job, and the output tail.
/// Execution itself is native-only — `run::Job::spawn` is an honest error
/// on wasm.
struct RunPanel {
    program: String,
    args: String,
    timeout_s: String,
    job: Option<run::Job>,
    output: Vec<String>,
    status: Option<String>,
}

impl Default for RunPanel {
    fn default() -> Self {
        Self {
            program: "openbnct".into(),
            args: "--help".into(),
            timeout_s: "600".into(),
            job: None,
            output: Vec::new(),
            status: None,
        }
    }
}

impl RunPanel {
    fn start(&mut self) {
        self.output.clear();
        self.status = None;
        let args: Vec<String> = self.args.split_whitespace().map(str::to_owned).collect();
        let timeout = self
            .timeout_s
            .trim()
            .parse::<u64>()
            .map(|s| std::time::Duration::from_secs(s.max(1)))
            .unwrap_or(run::DEFAULT_TIMEOUT);
        match run::Job::spawn(self.program.trim(), &args, timeout) {
            Ok(job) => self.job = Some(job),
            Err(error) => self.status = Some(error),
        }
    }

    /// Drain the job and update status; call every frame while a job
    /// exists. Returns true while still running (requests repaints).
    fn poll(&mut self) -> bool {
        let Some(job) = &mut self.job else {
            return false;
        };
        let running = job.poll(&mut self.output, 300);
        if !running {
            let job = self.job.take().unwrap();
            self.status = Some(if job.timed_out {
                "killed — deadline exceeded".into()
            } else {
                match job.exit_code {
                    Some(0) => "exit 0".into(),
                    Some(code) => format!("exit {code}"),
                    None => "terminated".into(),
                }
            });
        }
        running
    }

    fn cancel(&mut self) {
        if let Some(job) = &mut self.job {
            job.cancel();
        }
        self.job = None;
        self.status = Some("cancelled".into());
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
            return io::read_bytes(Path::new(path))
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()));
        }
        let roi = case
            .data
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
            io::read_bytes(Path::new(self.source_path.trim()))
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
                &case.data.ct.geometry,
                &mask,
                direction,
                half_widths,
                margin,
            )
            .map_err(|e| e.to_string())?;
            report.case_id = case.data.case_id.to_string();
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
            (_, Some(json)) => match io::write_bytes(Path::new(&path), (json + "\n").as_bytes()) {
                Ok(()) => self.status = Some(format!("wrote {path}")),
                Err(e) => self.error = Some(e),
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
    spectrum: SpectrumView,
    run: RunPanel,
}

/// Source-spectrum inspector state: the parsed source definition and the
/// artifact label it came from (beam description or standalone source).
#[derive(Default)]
struct SpectrumView {
    source: Option<openbnct_transport::FixedSourceDefinition>,
    source_label: String,
    error: Option<String>,
}

impl SpectrumView {
    /// Accept a `beam-description` (source extracted) or a bare
    /// `fixed-source-definition`, bytes-only so web drops work.
    fn load_bytes(&mut self, bytes: &[u8]) {
        let schema = serde_json::from_slice::<serde_json::Value>(bytes)
            .ok()
            .and_then(|value| {
                value
                    .get("schema_version")
                    .and_then(|s| s.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let outcome = if schema.contains("fixed-source-definition") {
            serde_json::from_slice::<openbnct_transport::FixedSourceDefinition>(bytes)
                .map_err(|error| error.to_string())
                .and_then(|source| {
                    source.validate().map_err(|e| e.to_string())?;
                    Ok((
                        source,
                        serde_json::from_slice::<serde_json::Value>(bytes)
                            .ok()
                            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(str::to_owned))
                            .unwrap_or_else(|| "fixed-source-definition".into()),
                    ))
                })
        } else if schema.contains("beam-description") {
            serde_json::from_slice::<openbnct_transport::BeamDescription>(bytes)
                .map_err(|error| error.to_string())
                .and_then(|beam| {
                    beam.validate().map_err(|e| e.to_string())?;
                    Ok((beam.source, beam.id))
                })
        } else {
            Err("not a beam-description or fixed-source-definition".into())
        };
        match outcome {
            Ok((source, label)) => {
                self.error = None;
                self.source = Some(source);
                self.source_label = label;
            }
            Err(error) => {
                self.source = None;
                self.error = Some(error);
            }
        }
    }
}

/// Where an OS-dragged path lands in the workbench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropTarget {
    Case,
    /// A `.zip` of the NF-BNCT-001 case directory — one file, full case.
    CaseArchive,
    /// One loose member (`case.json`, `rtstruct.dcm`, `ct-*.dcm`) of a
    /// multi-file case drop — accumulated until the set is complete.
    CaseFile,
    /// Any other `.dcm` — a research-study member, accumulated until the
    /// user confirms import.
    StudyFile,
    DoseBundle,
    Plan,
    NiftiVolume,
    Measurement,
    Spectrum,
    Robustness,
    /// A derived `openbnct.prompt-gamma-source` document — loads into the
    /// dose panel's prompt-gamma slot and selects the map quantity.
    PromptGamma,
    /// A `dose-uncertainty-budget` report — budget table in the dose
    /// workspace.
    UncertaintyBudget,
    Unsupported,
}

/// Directories are case roots; `.json` files route by extension first and
/// `schema_version` second; `.nii` / `.nii.gz` files are NIfTI volumes.
/// Anything else is reported rather than guessed.
fn classify_dropped_path(path: &Path) -> DropTarget {
    if path.is_dir() {
        return DropTarget::Case;
    }
    classify_dropped_name(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
    )
}

/// Extension-only classification — the web target has file names and
/// bytes but no paths or directories.
fn classify_dropped_name(name: &str) -> DropTarget {
    let name = name.to_ascii_lowercase();
    if name.ends_with(".nii") || name.ends_with(".nii.gz") {
        return DropTarget::NiftiVolume;
    }
    if name.ends_with(".zip") {
        return DropTarget::CaseArchive;
    }
    if io::looks_like_case_member(&name) {
        return DropTarget::CaseFile;
    }
    if name.ends_with(".dcm") || name.ends_with(".dicom") {
        return DropTarget::StudyFile;
    }
    if name.ends_with(".json") {
        return DropTarget::DoseBundle;
    }
    DropTarget::Unsupported
}

/// A "Browse…" button that works on both targets: native returns the
/// picked path so the caller's path-based loader runs; web spawns an
/// async pick that re-enters the drop channel — schema-routed to the
/// right panel exactly like an OS drop.
fn browse_file_button(
    ui: &mut egui::Ui,
    sender: Option<&DropSender>,
    filter_name: &str,
    extensions: &[&str],
    language: Language,
) -> Option<PathBuf> {
    let clicked = ui.button(t!(language, "Browse…", "参照…")).clicked();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = sender;
        clicked
            .then(|| io::pick_file(filter_name, extensions))
            .flatten()
    }
    #[cfg(target_arch = "wasm32")]
    {
        if !clicked {
            return None;
        }
        let Some(sender) = sender else {
            return None;
        };
        let sender = sender.clone();
        let context = ui.ctx().clone();
        let filter_name = filter_name.to_owned();
        let extensions: Vec<String> = extensions.iter().map(|e| e.to_string()).collect();
        wasm_bindgen_futures::spawn_local(async move {
            let extension_refs: Vec<&str> = extensions.iter().map(|e| e.as_str()).collect();
            if let Some(file) = rfd::AsyncFileDialog::new()
                .add_filter(&filter_name, &extension_refs)
                .pick_file()
                .await
            {
                let name = file.file_name();
                let bytes = file.read().await;
                let _ = sender.send((name, None, Ok(bytes)));
            }
            context.request_repaint();
        });
        None
    }
}

/// Refine a `.json` drop by its declared schema: an exposure plan goes to
/// the Plan workspace; anything else keeps the dose-bundle default.
fn classify_dropped_json(bytes: &[u8]) -> DropTarget {
    let Ok(schema) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return DropTarget::DoseBundle;
    };
    match schema
        .get("schema_version")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
    {
        s if s.starts_with("openbnct.exposure-plan/") => DropTarget::Plan,
        s if s.starts_with("nctforge.measurement-record/")
            || s.starts_with("openbnct.measurement-record/") =>
        {
            DropTarget::Measurement
        }
        s if s.contains("beam-description/") || s.contains("fixed-source-definition/") => {
            DropTarget::Spectrum
        }
        s if s.contains("plan-robustness/") => DropTarget::Robustness,
        s if s.contains("prompt-gamma-source/") => DropTarget::PromptGamma,
        s if s.contains("dose-uncertainty-budget/") => DropTarget::UncertaintyBudget,
        _ => DropTarget::DoseBundle,
    }
}

/// One picked/dropped file in flight: name, drop position (for painted
/// zones like the dose-B box), and its bytes or read error.
type DropMessage = (String, Option<egui::Pos2>, Result<Vec<u8>, String>);
type DropSender = std::sync::mpsc::Sender<DropMessage>;

/// Register the CJK fallback face (subset to the glyphs the Japanese UI
/// actually uses — regen via assets/regen-cjk-font.sh) so 日本語 renders on both
/// targets. Appended to both families: Latin keeps its crisp primary
/// font, CJK falls through.
fn install_cjk_fonts(context: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "noto-cjk-jp".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/NotoSansCJKjp-UI.otf"
        ))),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("noto-cjk-jp".to_owned());
    }
    context.set_fonts(fonts);
}

pub(crate) struct OpenBnctApp {
    case_path: String,
    load_error: Option<String>,
    case: Option<ViewerCase>,
    display: DisplaySettings,
    workspace: WorkspaceTab,
    dark_mode: bool,
    reduce_motion: bool,
    template_status: Option<String>,
    /// Loose case members accumulated across a multi-file drop — a full
    /// NF-BNCT-001 set (case.json + rtstruct.dcm + 40 ct slices) loads.
    pending_case_files: Vec<(String, Vec<u8>)>,
    case_drop_note: Option<String>,
    /// Generic DICOM members accumulated for a research-study import —
    /// bucketed by SOP Class at import time, not by file name.
    pending_study_files: Vec<(String, Vec<u8>)>,
    /// Set by View → Screenshot; consumed when the viewport delivers the
    /// `Event::Screenshot` frame on the next update.
    want_screenshot: bool,
    brand_logo: Option<egui::TextureHandle>,
    help: GuidedHelp,
    panels: WorkbenchPanels,
    /// Web drops resolve asynchronously — bytes land here on a later frame,
    /// carrying the drop position so zones (e.g. the dose-B box) route right.
    #[cfg(target_arch = "wasm32")]
    drop_sender: DropSender,
    #[cfg(target_arch = "wasm32")]
    drop_receiver: std::sync::mpsc::Receiver<DropMessage>,
    /// UI language — English authoring with Japanese localization.
    language: Language,
}

impl OpenBnctApp {
    fn new(initial_case: Option<PathBuf>, context: &egui::Context) -> Self {
        install_cjk_fonts(context);
        let has_initial_case = initial_case.is_some();
        #[cfg(target_arch = "wasm32")]
        let (drop_sender, drop_receiver) = std::sync::mpsc::channel();
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
            reduce_motion: false,
            template_status: None,
            pending_case_files: Vec::new(),
            case_drop_note: None,
            pending_study_files: Vec::new(),
            want_screenshot: false,
            brand_logo: brand::load_logo_texture(context).ok(),
            help: GuidedHelp::default(),
            panels: WorkbenchPanels::default(),
            #[cfg(target_arch = "wasm32")]
            drop_sender,
            #[cfg(target_arch = "wasm32")]
            drop_receiver,
            language: Language::detect(),
        };
        if has_initial_case {
            app.load_case();
        }
        #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
        if std::env::var_os("OPENBNCT_CAPTURE").is_some() {
            if let Ok(selected) = std::env::var("OPENBNCT_CAPTURE_WORKSPACE")
                && let Some(workspace) = WorkspaceTab::ALL
                    .into_iter()
                    .find(|tab| tab.marker() == selected)
            {
                app.workspace = workspace;
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
            let lang = self.language;
            ui.menu_button(t!(lang, "File", "ファイル"), |ui| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if ui
                        .button(t!(lang, "Open case…", "症例を開く…"))
                        .clicked()
                    {
                        if let Some(dir) = io::pick_folder() {
                            self.case_path = dir.display().to_string();
                            self.load_case();
                        }
                        ui.close();
                    }
                    ui.small(t!(
                        lang,
                        "the frozen NF-BNCT-001 case directory — or drop it anywhere.",
                        "凍結済み NF-BNCT-001 症例フォルダ — どこかにドロップしても可。"
                    ));
                    if ui
                        .button(t!(lang, "Import DICOM study…", "DICOM スタディを取り込む…"))
                        .clicked()
                    {
                        if let Some(dir) = io::pick_folder() {
                            let paths = openbnct_dicom::collect_study_paths(&dir);
                            match openbnct_dicom::import_study_from_paths(&paths)
                                .map_err(|e| e.to_string())
                                .and_then(ViewerCase::from_study)
                            {
                                Ok(case) => {
                                    self.case = Some(case);
                                    self.load_error = None;
                                    self.workspace = WorkspaceTab::Geometry;
                                }
                                Err(error) => {
                                    self.load_error = Some(format!(
                                        "{}: {error}",
                                        t!(lang, "study import rejected", "スタディ取り込み拒否")
                                    ));
                                }
                            }
                        }
                        ui.close();
                    }
                    ui.small(t!(
                        lang,
                        "any CT + RTSTRUCT export — bucketed by SOP class, hash-bound.",
                        "任意の CT + RTSTRUCT エクスポート — SOP クラスで仕分け、ハッシュで紐付け。"
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    if ui
                        .button(t!(lang, "Open case or study files…", "症例/スタディを開く…"))
                        .clicked()
                    {
                        // Browsers cannot hand us a folder path — picked
                        // files arrive on the drop channel and route like
                        // drops (zip → archive, .dcm → study accumulator).
                        self.pick_files_into_drops(ui.ctx());
                        ui.close();
                    }
                    ui.small(t!(
                        lang,
                        "…or drop them: a case .zip, the 42 NF-BNCT-001 members, \
                         or a DICOM study's files.",
                        "…またはドロップ: 症例 .zip、NF-BNCT-001 の全42ファイル、または DICOM スタディのファイル群。"
                    ));
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if ui
                        .button(t!(
                            lang,
                            "Export case template…",
                            "症例テンプレートを出力…"
                        ))
                        .clicked()
                    {
                        if let Some(dir) = io::pick_folder() {
                            self.template_status = Some(match export_case_template(&dir) {
                                Ok(count) => format!(
                                    "{} {} ({})",
                                    t!(lang, "template written to", "テンプレート出力先:"),
                                    dir.display(),
                                    t!(
                                        lang,
                                        "{count} files — edit before use",
                                        "{count} ファイル — 使用前に編集してください"
                                    )
                                    .replace("{count}", &count.to_string())
                                ),
                                Err(error) => format!(
                                    "{}: {error}",
                                    t!(lang, "template export failed", "テンプレート出力失敗")
                                ),
                            });
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(t!(lang, "Quit", "終了")).clicked() {
                        ui.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });
            ui.menu_button(t!(self.language, "View", "表示"), |ui| {
                let lang = self.language;
                ui.checkbox(&mut self.dark_mode, t!(lang, "Dark mode", "ダークモード"));
                if ui
                    .checkbox(
                        &mut self.reduce_motion,
                        t!(lang, "Reduce motion", "アニメーションを減らす"),
                    )
                    .changed()
                {
                    self.apply_motion(ui.ctx());
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(t!(lang, "Language / 言語", "言語 / Language")).small(),
                );
                for candidate in Language::ALL {
                    if ui
                        .selectable_label(self.language == candidate, candidate.label())
                        .clicked()
                    {
                        self.language = candidate;
                        candidate.persist();
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new(t!(lang, "Interface scale", "表示倍率")).small());
                ui.horizontal(|ui| {
                    for (label, factor) in [
                        ("75%", 0.75_f32),
                        ("100%", 1.0),
                        ("125%", 1.25),
                        ("150%", 1.5),
                    ] {
                        let current = (ui.ctx().zoom_factor() - factor).abs() < 0.01;
                        if ui.selectable_label(current, label).clicked() {
                            ui.ctx().set_zoom_factor(factor);
                        }
                    }
                });
                ui.separator();
                if ui
                    .button(t!(lang, "Reset views", "ビューをリセット"))
                    .clicked()
                {
                    self.reset_views();
                    ui.close();
                }
                if ui
                    .button(t!(lang, "Screenshot…", "スクリーンショット…"))
                    .clicked()
                {
                    self.want_screenshot = true;
                    ui.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                        egui::UserData::default(),
                    ));
                    ui.close();
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ui.separator();
                    if ui
                        .button(t!(lang, "Toggle fullscreen", "全画面表示の切替"))
                        .clicked()
                    {
                        ui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                            !ui.input(|i| i.viewport().fullscreen.unwrap_or(false)),
                        ));
                        ui.close();
                    }
                }
            });
            let help = ui.menu_button(t!(self.language, "Help", "ヘルプ"), |ui| {
                if ui
                    .button(t!(
                        self.language,
                        "Help and guided tours",
                        "ヘルプとガイドツアー"
                    ))
                    .clicked()
                {
                    self.help.toggle_center();
                    ui.close();
                }
            });
            tour_targets.set(TourTarget::HelpButton, help.response.rect);
        });
    }

    /// Zero egui's animation time for vestibular-sensitive users —
    /// instant transitions instead of the default 0.2 s tweens.
    fn apply_motion(&self, context: &egui::Context) {
        context.global_style_mut(|style| {
            style.animation_time = if self.reduce_motion { 0.0 } else { 0.2 };
        });
    }

    /// Restore display state without touching loaded artifacts: crosshair
    /// centered, dose-map overlays off, profile defaults, textures rebuilt.
    fn reset_views(&mut self) {
        if let Some(case) = &mut self.case {
            case.crosshair = Crosshair::centered(&case.grid);
            case.textures_dirty = true;
        }
        self.panels.dose.map.voxel = None;
        self.panels.dose.map.log_scale = false;
        self.panels.dose.map.contours = false;
        self.panels.dose.map.sigma_view = false;
        self.panels.dose.map.textures = None;
        self.panels.dose.map.cache_key = None;
        self.panels.dose.profile.axis = 0;
        self.panels.dose.profile.log_y = false;
        self.panels.dose.profile.measurement_pick = 0;
        self.template_status =
            Some("views reset — crosshair centered, overlays and profiles restored".into());
    }

    /// Encode the captured frame to PNG, then save (native picker) or
    /// download (browser blob).
    fn save_screenshot(&mut self, image: &egui::ColorImage) {
        use image::ImageEncoder;
        let rgba: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect();
        let mut png = Vec::new();
        let encoded = image::codecs::png::PngEncoder::new(&mut png).write_image(
            &rgba,
            image.size[0] as u32,
            image.size[1] as u32,
            image::ExtendedColorType::Rgba8,
        );
        if let Err(error) = encoded {
            self.load_error = Some(format!("screenshot encode failed: {error}"));
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("PNG image", &["png"])
                .set_file_name("openbnct.png")
                .save_file()
            {
                match io::write_bytes(&path, &png) {
                    Ok(()) => {
                        self.template_status =
                            Some(format!("screenshot saved to {}", path.display()))
                    }
                    Err(error) => {
                        self.load_error = Some(format!("screenshot save failed: {error}"))
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        web::download_bytes("openbnct.png", &png);
    }

    /// Route an OS-dropped file onto the matching workbench surface.
    /// `path` is a real directory-capable path only on native (web drops
    /// carry the file name there); `bytes` is already-read content.
    fn handle_dropped(
        &mut self,
        name: &str,
        path: Option<&Path>,
        bytes: Result<Vec<u8>, String>,
        position: Option<egui::Pos2>,
    ) {
        let target = match path {
            Some(path) => classify_dropped_path(path),
            None => classify_dropped_name(name),
        };
        let target = match target {
            // JSON drops refine by declared schema when bytes arrived.
            DropTarget::DoseBundle => bytes
                .as_ref()
                .ok()
                .map(|b| classify_dropped_json(b))
                .unwrap_or(DropTarget::DoseBundle),
            other => other,
        };
        match target {
            DropTarget::Case => {
                if let Some(path) = path {
                    self.case_path = path.display().to_string();
                    self.load_case();
                    self.workspace = WorkspaceTab::Geometry;
                } else {
                    self.load_error = Some(
                        "case drops need a .zip of the case folder — or drop all 42 \
                         members (case.json, rtstruct.dcm, ct-000…039.dcm) together"
                            .into(),
                    );
                }
            }
            DropTarget::CaseArchive => match bytes
                .and_then(|b| io::unzip_case_archive(&b).and_then(ViewerCase::load_bytes))
            {
                Ok(case) => {
                    self.case = Some(case);
                    self.case_drop_note = None;
                    self.load_error = None;
                    self.workspace = WorkspaceTab::Geometry;
                }
                Err(error) => {
                    self.load_error = Some(format!("case archive rejected: {error}"));
                }
            },
            DropTarget::CaseFile => {
                match bytes {
                    Ok(b) => {
                        if let Some(member) = io::case_member_name(name) {
                            self.pending_case_files.retain(|(n, _)| *n != member);
                            self.pending_case_files.push((member, b));
                        }
                    }
                    Err(error) => {
                        self.load_error = Some(format!("could not read {name}: {error}"));
                    }
                }
                self.try_complete_case_drop();
            }
            DropTarget::StudyFile => match bytes {
                Ok(b) => {
                    self.pending_study_files.retain(|(n, _)| n.as_str() != name);
                    self.pending_study_files.push((name.to_owned(), b));
                }
                Err(error) => {
                    self.load_error = Some(format!("could not read {name}: {error}"));
                }
            },
            DropTarget::DoseBundle => {
                // A drop on the painted B zone while a bundle is loaded
                // feeds the compare slot; anywhere else replaces A.
                let into_compare = self.panels.dose.bundle.is_some()
                    && position.is_some_and(|p| self.panels.dose.compare_zone.contains(p));
                if into_compare {
                    match bytes {
                        Ok(b) => self.panels.dose.load_compare_bytes(b),
                        Err(error) => self.panels.dose.compare_error = Some(error),
                    }
                } else {
                    let result = bytes.and_then(|b| self.panels.dose.load_bundle_bytes(b));
                    if let Err(error) = result {
                        self.panels.dose.bundle_error = Some(error);
                    }
                }
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::Plan => match bytes {
                Ok(b) => self.panels.plan.load_bytes(&b),
                Err(error) => self.panels.plan.error = Some(error),
            },
            DropTarget::NiftiVolume => match bytes {
                Ok(b) => self.panels.nifti.inspect_bytes(&b),
                Err(error) => self.panels.nifti.error = Some(error),
            },
            DropTarget::Measurement => {
                match bytes {
                    Ok(b) => self.panels.dose.load_measurement_bytes(&b),
                    Err(error) => self.panels.dose.profile.measurement_error = Some(error),
                }
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::Spectrum => {
                match bytes {
                    Ok(b) => self.panels.spectrum.load_bytes(&b),
                    Err(error) => self.panels.spectrum.error = Some(error),
                }
                self.workspace = WorkspaceTab::Transport;
            }
            DropTarget::Robustness => {
                match bytes {
                    Ok(b) => self.panels.plan.load_robustness_bytes(&b),
                    Err(error) => self.panels.plan.robustness_error = Some(error),
                }
                self.workspace = WorkspaceTab::Plan;
            }
            DropTarget::UncertaintyBudget => {
                match bytes
                    .map_err(|e| e.clone())
                    .and_then(|b| {
                        serde_json::from_slice::<openbnct_transport::DoseUncertaintyBudget>(&b)
                            .map_err(|e| e.to_string())
                    })
                    .and_then(|budget| budget.validate().map(|_| budget).map_err(|e| e.to_string()))
                {
                    Ok(budget) => {
                        self.panels.dose.uncertainty_budget = Some(budget);
                        self.panels.dose.uncertainty_budget_error = None;
                    }
                    Err(error) => {
                        self.panels.dose.uncertainty_budget_error = Some(error);
                    }
                }
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::PromptGamma => {
                match bytes
                    .map_err(|e| e.clone())
                    .and_then(|b| {
                        serde_json::from_slice::<openbnct_transport::PromptGammaSource>(&b)
                            .map_err(|e| e.to_string())
                    })
                    .and_then(|source| source.validate().map(|_| source).map_err(|e| e.to_string()))
                {
                    Ok(source) => {
                        self.panels.dose.prompt_gamma = Some(source);
                        self.panels.dose.prompt_gamma_error = None;
                        self.panels.dose.map.quantity = "prompt_gamma_478kev".into();
                        self.panels.dose.map.textures = None;
                        self.panels.dose.map.cache_key = None;
                    }
                    Err(error) => {
                        self.panels.dose.prompt_gamma_error = Some(error);
                    }
                }
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::Unsupported => {
                self.load_error = Some(format!("unsupported drop {name}"));
            }
        }
    }

    /// A complete loose-file case set is case.json + rtstruct.dcm +
    /// ct/ct-000…039.dcm (42 files). Until then a progress note tells the
    /// user exactly which members are still missing.
    fn try_complete_case_drop(&mut self) {
        if self.pending_case_files.is_empty() {
            self.case_drop_note = None;
            return;
        }
        let missing = missing_case_members(&self.pending_case_files);
        if missing.is_empty() {
            match ViewerCase::load_bytes(std::mem::take(&mut self.pending_case_files)) {
                Ok(case) => {
                    self.case = Some(case);
                    self.case_drop_note = None;
                    self.load_error = None;
                    self.workspace = WorkspaceTab::Geometry;
                }
                Err(error) => {
                    self.load_error = Some(format!("case files rejected: {error}"));
                }
            }
            return;
        }
        self.case_drop_note = Some(format!(
            "Collecting case files — {} received; still need {}",
            self.pending_case_files.len(),
            missing.join(", ")
        ));
    }

    /// Multi-file picker — picked files re-enter `handle_dropped` so a
    /// .zip becomes a case archive and loose members accumulate exactly
    /// like OS drops. One implementation for both targets.
    #[cfg(not(target_arch = "wasm32"))]
    fn pick_files_into_drops(&mut self, _context: &egui::Context) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter("case or study", &["zip", "dcm", "dicom", "json"])
            .pick_files()
        {
            for path in paths {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("picked-file")
                    .to_owned();
                let bytes = io::read_bytes(&path).map_err(|e| e.to_string());
                self.handle_dropped(&name, None, bytes, None);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn pick_files_into_drops(&mut self, context: &egui::Context) {
        let sender = self.drop_sender.clone();
        let context = context.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let picked = rfd::AsyncFileDialog::new()
                .add_filter("case or study", &["zip", "dcm", "dicom", "json"])
                .pick_files()
                .await;
            if let Some(handles) = picked {
                for handle in handles {
                    let name = handle.file_name();
                    let bytes = handle.read().await;
                    let _ = sender.send((name, None, Ok(bytes)));
                }
            }
            context.request_repaint();
        });
    }

    /// Bucket the accumulated DICOM members by SOP Class and import —
    /// a research case, verified against itself at load, not a fixture.
    /// Members stay buffered on failure so the user can add what's missing.
    fn import_pending_study(&mut self) {
        if self.pending_study_files.is_empty() {
            return;
        }
        match openbnct_dicom::import_study_from_files(&self.pending_study_files) {
            Ok(study) => match ViewerCase::from_study(study) {
                Ok(case) => {
                    self.case = Some(case);
                    self.pending_study_files.clear();
                    self.load_error = None;
                    self.case_drop_note = None;
                    self.workspace = WorkspaceTab::Geometry;
                }
                Err(error) => {
                    self.load_error = Some(format!("study imported but view failed: {error}"));
                }
            },
            Err(error) => {
                self.load_error = Some(format!("study import rejected: {error}"));
            }
        }
    }
}

/// Which NF-BNCT-001 members a loose-file set still lacks. Empty means
/// the set is complete and ready for `load_nf_bnct_001_from_files`.
fn missing_case_members(files: &[(String, Vec<u8>)]) -> Vec<String> {
    let have_manifest = files.iter().any(|(name, _)| name == "case.json");
    let have_rtstruct = files.iter().any(|(name, _)| name == "rtstruct.dcm");
    let ct_count = files
        .iter()
        .filter(|(name, _)| name.starts_with("ct/"))
        .count();
    let mut missing = Vec::new();
    if !have_manifest {
        missing.push("case.json".to_owned());
    }
    if !have_rtstruct {
        missing.push("rtstruct.dcm".to_owned());
    }
    if ct_count < 40 {
        missing.push(format!("{} more CT slice(s)", 40 - ct_count));
    }
    missing
}

impl eframe::App for OpenBnctApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
        capture_preview(ui.ctx());
        if self.want_screenshot {
            let captured = ui.input(|input| {
                input.events.iter().find_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(image) = captured {
                self.save_screenshot(&image);
                self.want_screenshot = false;
            }
        }
        if ui.input(|input| input.key_pressed(egui::Key::F1)) {
            self.help.toggle_center();
        }
        if let Some(workspace) = self.help.requested_workspace(self.language) {
            self.workspace = workspace.into();
        }

        // Web drops read asynchronously — bytes arrive through the channel
        // on a later frame; native drops read synchronously in place.
        let dropped: Vec<egui::DroppedFileHandle> =
            ui.input(|input| input.raw.dropped_files.clone());
        let drop_pos = ui.input(|input| input.pointer.interact_pos().or(input.pointer.hover_pos()));
        for file in dropped {
            let name = file
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("dropped-file")
                .to_owned();
            #[cfg(not(target_arch = "wasm32"))]
            {
                let path = file.path().is_dir().then(|| file.path().to_path_buf());
                self.handle_dropped(&name, path.as_deref(), file.bytes(), drop_pos);
            }
            #[cfg(target_arch = "wasm32")]
            {
                let sender = self.drop_sender.clone();
                let context = ui.ctx().clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let bytes = file.bytes_async().await;
                    let _ = sender.send((name, drop_pos, bytes));
                    context.request_repaint();
                });
            }
        }
        #[cfg(target_arch = "wasm32")]
        while let Ok((name, pos, bytes)) = self.drop_receiver.try_recv() {
            self.handle_dropped(&name, None, bytes, pos);
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
                    self.language,
                    theme,
                    &mut tour_targets,
                );
                ui.add_space(8.0);
                let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                match show_case_loader(
                    ui,
                    &mut self.case_path,
                    self.case.as_ref(),
                    enter_pressed,
                    self.language,
                    &mut tour_targets,
                    &mut self.template_status,
                ) {
                    LoaderRequest::LoadPath => self.load_case(),
                    LoaderRequest::PickFiles => self.pick_files_into_drops(ui.ctx()),
                    LoaderRequest::None => {}
                }
                if let Some(error) = &self.load_error {
                    ui.colored_label(
                        theme.error,
                        format!(
                            "{}: {error}",
                            t!(self.language, "Load rejected", "読込拒否")
                        ),
                    );
                }
                if let Some(note) = self.case_drop_note.clone() {
                    ui.horizontal(|ui| {
                        ui.colored_label(theme.warn_text, &note);
                        if ui
                            .small_button(t!(self.language, "discard", "破棄"))
                            .clicked()
                        {
                            self.pending_case_files.clear();
                            self.case_drop_note = None;
                        }
                    });
                }
                if !self.pending_study_files.is_empty() {
                    let count = self.pending_study_files.len();
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            theme.warn_text,
                            t!(
                                self.language,
                                "{count} DICOM file(s) collected for study import",
                                "スタディ取り込み用に {count} 件の DICOM ファイルを収集済み"
                            )
                            .replace("{count}", &count.to_string()),
                        );
                        if ui
                            .small_button(t!(
                                self.language,
                                "Import as research case",
                                "研究用症例として取り込む"
                            ))
                            .clicked()
                        {
                            self.import_pending_study();
                        }
                        if ui
                            .small_button(t!(self.language, "discard", "破棄"))
                            .clicked()
                        {
                            self.pending_study_files.clear();
                        }
                    });
                }
                if let Some(status) = &self.template_status {
                    ui.label(status);
                }
            });
        egui::Panel::bottom("workbench-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(t!(
                        self.language,
                        "Research use only · Not for clinical decision-making",
                        "研究専用 · 臨床判断には使用不可"
                    ))
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
        #[cfg(target_arch = "wasm32")]
        let drop_sender = Some(&self.drop_sender);
        #[cfg(not(target_arch = "wasm32"))]
        let drop_sender = None;
        show_workbench(
            ui,
            &mut self.workspace,
            self.case.as_mut(),
            &mut self.display,
            &mut self.panels,
            drop_sender,
            self.language,
            &mut tour_targets,
            theme,
        );
        self.help.show_center(
            ui.ctx(),
            self.workspace.into(),
            self.case.is_some(),
            self.language,
            theme,
        );
        self.help
            .show_tour(ui.ctx(), &tour_targets, self.language, theme);
    }
}

/// Installs the OpenBNCT palette into egui's own light and dark visuals so
/// every native surface — panels, menus, popups, tooltips, text fields —
/// follows the theme, not just the custom frames. Called once at startup;
/// the View menu only flips `ThemePreference` afterward.
pub(crate) fn configure_style(context: &egui::Context) {
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
    language: Language,
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
        ui.label(
            egui::RichText::new(t!(language, "Research workbench", "研究ワークベンチ"))
                .color(theme.text_dim),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(
                    case.map_or(t!(language, "No case open", "症例なし"), |c| {
                        c.data.case_id.as_str()
                    }),
                )
                .color(theme.text_dim),
            );
        });
    });
}

/// The transport-side authoring contracts, embedded from the benchmark so
/// "export template" writes real validated structure a user can edit into
/// their own case. The response set and acceptance contract are provenance
/// artifacts and stay repo-side.
#[cfg(not(target_arch = "wasm32"))]
const TEMPLATE_FILES: [(&str, &str); 4] = [
    ("case.json", include_str!("../assets/nf-bnct-001/case.json")),
    (
        "source.json",
        include_str!("../assets/nf-bnct-001/source.json"),
    ),
    (
        "material.json",
        include_str!("../assets/nf-bnct-001/material.json"),
    ),
    (
        "component-profile.json",
        include_str!("../assets/nf-bnct-001/component-profile.json"),
    ),
];

#[cfg(not(target_arch = "wasm32"))]
const TEMPLATE_README: &str = "# OpenBNCT transport-case template\n\
\n\
These are the NF-BNCT-001 benchmark contracts verbatim — a validated\n\
starting point, not your case. Edit `case.json` (geometry, regions),\n\
`source.json` (beam), `material.json` (composition), and\n\
`component-profile.json` (folding) to your geometry and data, then feed\n\
the directory to `openbnct openmc generate`. A `voxel_set` geometry needs\n\
a real volume source (NIfTI/DICOM), which the GUI does not author.\n";

#[cfg(not(target_arch = "wasm32"))]
fn export_case_template(destination: &Path) -> Result<usize, String> {
    let mut written = 0;
    for (name, contents) in TEMPLATE_FILES {
        io::write_bytes(&destination.join(name), contents.as_bytes())
            .map_err(|error| format!("{name}: {error}"))?;
        written += 1;
    }
    io::write_bytes(&destination.join("README.md"), TEMPLATE_README.as_bytes())
        .map_err(|error| format!("README.md: {error}"))?;
    Ok(written + 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // each variant is constructed on exactly one target
enum LoaderRequest {
    None,
    /// Native only: load the benchmark case directory in the path field.
    LoadPath,
    /// Open a multi-file picker — picked files route through the same
    /// drop handler (zip → case archive, .dcm → study accumulator).
    PickFiles,
}

fn show_case_loader(
    ui: &mut egui::Ui,
    case_path: &mut String,
    case: Option<&ViewerCase>,
    enter_pressed: bool,
    language: Language,
    tour_targets: &mut TourTargets,
    template_status: &mut Option<String>,
) -> LoaderRequest {
    let mut request = LoaderRequest::None;
    let _ = (case, template_status);
    let response = ui.horizontal(|ui| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            ui.label(t!(language, "Case folder", "症例フォルダ"));
            let path_response = ui.add(
                egui::TextEdit::singleline(case_path)
                    .desired_width((ui.available_width() - 220.0).max(180.0))
                    .hint_text(t!(
                        language,
                        "Open a verified case folder, or drop it here",
                        "検証済み症例フォルダを開く、またはここにドロップ"
                    )),
            );
            if ui
                .button(t!(language, "Load & verify", "読込・検証"))
                .clicked()
                || (path_response.lost_focus() && enter_pressed)
            {
                request = LoaderRequest::LoadPath;
            }
            if ui.button(t!(language, "Browse…", "参照…")).clicked()
                && let Some(dir) = io::pick_folder()
            {
                *case_path = dir.display().to_string();
                request = LoaderRequest::LoadPath;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (case_path, enter_pressed);
            ui.label(t!(language, "Case", "症例"));
            if ui
                .button(t!(language, "Pick files…", "ファイルを選択…"))
                .clicked()
            {
                request = LoaderRequest::PickFiles;
            }
            ui.small(t!(
                language,
                "a case .zip, the 42 NF-BNCT-001 members, or a DICOM study — or drop them",
                "症例 .zip、NF-BNCT-001 の全42ファイル、または DICOM スタディ — ドロップでも可"
            ));
        }
    });
    tour_targets.set(TourTarget::CaseLoader, response.response.rect);
    request
}

#[allow(clippy::too_many_arguments)] // workspace plumbing — each param is load-bearing
fn show_workbench(
    ui: &mut egui::Ui,
    workspace: &mut WorkspaceTab,
    case: Option<&mut ViewerCase>,
    display: &mut DisplaySettings,
    panels: &mut WorkbenchPanels,
    drop_sender: Option<&DropSender>,
    language: Language,
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
                egui::RichText::new(t!(language, "WORKBENCH", "ワークベンチ"))
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
                let label =
                    egui::RichText::new(candidate.localized_label(language)).color(if selected {
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
                egui::RichText::new(t!(language, "ACTIVE CASE", "開いている症例"))
                    .size(11.0)
                    .strong()
                    .color(theme.text_dim),
            );
            ui.add_space(6.0);
            if let Some(case) = case.as_deref() {
                ui.strong(&case.data.case_id);
                ui.small(&case.data.provenance);
            } else {
                ui.label(
                    egui::RichText::new(t!(language, "No case loaded", "症例なし"))
                        .color(theme.text_dim),
                );
                ui.small(t!(
                    language,
                    "Open a case above to inspect its geometry.",
                    "上で症例を開くとジオメトリを確認できます。"
                ));
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
                        show_overview(
                            ui,
                            case.as_deref(),
                            workspace,
                            language,
                            tour_targets,
                            theme,
                        );
                    }
                    WorkspaceTab::Geometry => {
                        if let Some(case) = case {
                            show_geometry_workspace(
                                ui,
                                case,
                                display,
                                language,
                                tour_targets,
                                theme,
                            );
                        } else {
                            show_workspace_heading(
                                ui,
                                theme,
                                t!(language, "Geometry", "ジオメトリ"),
                                t!(
                                    language,
                                    "Patient-space DICOM truth before transport.",
                                    "輸送計算の前段となる患者空間の DICOM 実データ。"
                                ),
                            );
                            show_empty_state(ui, language);
                        }
                    }
                    WorkspaceTab::Transport => show_transport_workspace(
                        ui,
                        case.as_deref(),
                        &mut panels.position,
                        &mut panels.spectrum,
                        &mut panels.run,
                        language,
                        tour_targets,
                        theme,
                    ),
                    WorkspaceTab::Plan => show_plan_workspace(
                        ui,
                        &mut panels.plan,
                        drop_sender,
                        language,
                        tour_targets,
                        theme,
                    ),
                    WorkspaceTab::Dose => {
                        show_dose_workspace(
                            ui,
                            &mut panels.dose,
                            &mut panels.nifti,
                            drop_sender,
                            language,
                            tour_targets,
                            theme,
                        );
                    }
                    WorkspaceTab::Evidence => show_evidence_workspace(
                        ui,
                        case.as_deref(),
                        &mut panels.evidence,
                        language,
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
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(language, "Your research workspace", "研究ワークベンチ"),
        t!(
            language,
            "Inspect the case. Explore dose. Follow the evidence.",
            "症例を確認し、線量を探索し、エビデンスを追跡する。"
        ),
    );
    ui.add_space(12.0);
    egui::Frame::new().fill(theme.card_fill).corner_radius(10)
        .inner_margin(egui::Margin::same(24)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(t!(language, "CASE STUDY  /  SYNTHETIC BENCHMARK", "症例 / 合成ベンチマーク")).size(11.0).strong().color(theme.brand));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(case.map_or(t!(language, "Start with a verified case", "検証済み症例から開始"), |c| c.data.case_id.as_str())).size(26.0).strong());
            ui.label(egui::RichText::new(if case.is_some() {
                t!(language,
                    "Patient-space geometry and artifact integrity verified. Ready to inspect.",
                    "患者空間ジオメトリとアーティファクト完全性を検証済み。確認の準備ができています。")
            } else {
                t!(language,
                    "Open an NF-BNCT-001 case folder above. Geometry is verified before it is displayed.",
                    "上で NF-BNCT-001 症例フォルダを開いてください。ジオメトリは表示前に検証されます。")
            }).color(theme.text_dim));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(case.is_some(), egui::Button::new(t!(language, "Inspect geometry", "ジオメトリを確認"))).clicked() {
                    *workspace = WorkspaceTab::Geometry;
                }
                if ui.button(t!(language, "Review evidence", "エビデンスを確認")).clicked() { *workspace = WorkspaceTab::Evidence; }
            });
        });
    ui.add_space(20.0);
    ui.heading(t!(language, "Explore the workbench", "ワークベンチを探索"));
    ui.add_space(6.0);
    ui.columns(3, |columns| {
        for (column, (tab, title, detail, action)) in columns.iter_mut().zip([
            (WorkspaceTab::Transport,
                t!(language, "01  Prepare", "01  準備"),
                t!(language,
                    "Inspect material and source contracts, position the beam, and review transport readiness.",
                    "材料・線源の契約を確認し、ビームを位置決めし、輸送レディネスをレビュー。"),
                t!(language, "Open transport", "輸送を開く")),
            (WorkspaceTab::Plan,
                t!(language, "02  Evaluate", "02  評価"),
                t!(language,
                    "Load a plan, inspect fields and weights, and review its calculated result.",
                    "計画を読み込み、フィールドと重みを確認し、計算結果をレビュー。"),
                t!(language, "Open planning", "計画を開く")),
            (WorkspaceTab::Dose,
                t!(language, "03  Understand", "03  理解"),
                t!(language,
                    "Explore physical and biological dose, region metrics, DVHs, and NIfTI volumes.",
                    "物理・生物学的線量、領域指標、DVH、NIfTI ボリュームを探索。"),
                t!(language, "Explore dose", "線量を探索")),
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
        ui.heading(t!(language, "Benchmark readiness", "ベンチマークレディネス"));
        ui.label(egui::RichText::new(t!(language,
            "Qualification of the frozen reference workflow; imported artifacts have their own validation.",
            "凍結参照ワークフローの適格性。取り込み済みアーティファクトは独自の検証を持ちます。")).color(theme.text_dim));
        ui.add_space(8.0);
        egui::Frame::new().fill(theme.card_fill).corner_radius(8)
            .inner_margin(egui::Margin::same(18)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (index, gate) in readiness_gates(case.is_some(), language).iter().enumerate() {
                    if index > 0 { ui.separator(); }
                    ui.horizontal(|ui| {
                        ui.allocate_ui(egui::vec2(180.0, 28.0), |ui| { ui.strong(gate.title); });
                        ui.allocate_ui(egui::vec2((ui.available_width() - 130.0).max(150.0), 28.0), |ui| {
                            ui.label(egui::RichText::new(gate.detail).small().color(theme.text_dim));
                        });
                        status_badge(ui, gate.state, gate.state.localized_label(language));
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
    data: CaseData,
    grid: PatientAlignedGrid,
    crosshair: Crosshair,
    roi_visible: Vec<bool>,
    dose: DoseOverlay,
    textures: [Option<egui::TextureHandle>; 3],
    textures_dirty: bool,
}

/// What every loaded case supplies regardless of origin — the frozen
/// benchmark or an imported research study.
struct CaseData {
    case_id: String,
    /// Provenance line for the UI: "41 artifacts hash-verified" for the
    /// benchmark, "N members hash-bound" for an imported study.
    provenance: String,
    ct: CtVolume,
    structures: StructureSet,
}

impl ViewerCase {
    fn load(root: &Path) -> Result<Self, String> {
        let verified = load_nf_bnct_001(root).map_err(|error| error.to_string())?;
        Self::from_verified(root.to_path_buf(), verified)
    }

    /// In-memory entry point — the same verifier, fed by dropped/picked
    /// bytes instead of the filesystem (web build, zip archives).
    fn load_bytes(files: Vec<(String, Vec<u8>)>) -> Result<Self, String> {
        let verified = load_nf_bnct_001_from_files(&files).map_err(|error| error.to_string())?;
        Self::from_verified(PathBuf::from("<dropped files>"), verified)
    }

    /// Research study import — arbitrary DICOM members, parsed and
    /// hash-bound at import time rather than frozen-verified.
    fn from_study(study: ImportedStudy) -> Result<Self, String> {
        let provenance = if study.ignored.is_empty() {
            format!("{} members hash-bound", study.member_count)
        } else {
            format!(
                "{} members hash-bound, {} ignored",
                study.member_count,
                study.ignored.len()
            )
        };
        let data = CaseData {
            case_id: study.case_id.clone(),
            provenance,
            ct: study.ct,
            structures: study.structures,
        };
        Self::from_data(PathBuf::from("<imported study>"), data)
    }

    fn from_verified(root: PathBuf, verified: VerifiedBenchmarkCase) -> Result<Self, String> {
        let data = CaseData {
            case_id: verified.report.case_id.to_string(),
            provenance: format!(
                "frozen benchmark · {} artifacts hash-verified",
                verified.report.verified_artifact_count
            ),
            ct: verified.ct,
            structures: verified.structures,
        };
        Self::from_data(root, data)
    }

    fn from_data(root: PathBuf, data: CaseData) -> Result<Self, String> {
        let grid = PatientAlignedGrid::new(&data.ct.geometry)
            .map_err(|error| format!("anatomical viewer cannot represent this grid: {error}"))?;
        let crosshair = Crosshair::centered(&grid);
        let roi_visible = data
            .structures
            .rois
            .iter()
            .map(|roi| roi.name != "PHANTOM")
            .collect();
        Ok(Self {
            root,
            data,
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
            let image = render_slice(&self.data, view, &self.roi_visible, wash, display)?;
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
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(language, "Geometry", "ジオメトリ"),
        t!(
            language,
            "Integrity-gated, linked patient-space views of the frozen synthetic case.",
            "完全性ゲート済みの患者空間ビュー — 凍結ベンチマークまたは取り込み済みスタディ。"
        ),
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
                        && let Some(voxel) = show_slice_view(
                            ui,
                            texture,
                            view,
                            case.crosshair,
                            image_height,
                            language,
                        )
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
                        ui.heading(t!(language, "Image inspector", "画像インスペクタ"));
                        ui.label(
                            egui::RichText::new(t!(
                                language,
                                "Click or drag an image to move the linked crosshair.",
                                "画像をクリック/ドラッグすると連動クロスヘアが移動します。"
                            ))
                            .color(theme.text_dim),
                        );
                        ui.collapsing(
                            t!(language, "Case & voxel details", "症例・ボクセル詳細"),
                            |ui| {
                                show_case_summary(ui, case, language);
                            },
                        );
                        if show_display_controls(ui, case, display, language, theme) {
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

/// TECDOC-1223 group boundaries in eV — the same convention the
/// transport crate's beam-quality evaluator uses.
const SPECTRUM_THERMAL_UPPER_EV: f64 = 0.5;
const SPECTRUM_EPITHERMAL_UPPER_EV: f64 = 1.0e4;

/// Log-log energy-spectrum plot: a tabulated histogram renders as a
/// staircase of bin weights with thermal/epithermal/fast region shading
/// and region-integral labels (bins straddling a boundary split by
/// log-interpolated weight density). Monoenergetic sources draw a marker.
fn show_spectrum(ui: &mut egui::Ui, spectrum: &mut SpectrumView, language: Language, theme: Theme) {
    use openbnct_transport::EnergyDistribution;

    egui::Frame::group(ui.style()).show(ui, |ui| {
        if let Some(error) = &spectrum.error {
            ui.colored_label(
                theme.error,
                format!(
                    "{}: {error}",
                    t!(language, "spectrum rejected", "スペクトル拒否")
                ),
            );
        }
        let Some(source) = &spectrum.source else {
            ui.label(t!(
                language,
                "No source loaded — drop a beam-description or fixed-source-definition .json.",
                "線源未読込 — beam-description または fixed-source-definition .json をドロップ。"
            ));
            return;
        };
        ui.monospace(&spectrum.source_label);

        let (response, painter) = ui.allocate_painter(
            egui::vec2(ui.available_width(), 220.0),
            egui::Sense::focusable_noninteractive(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                true,
                t!(
                    language,
                    "Energy spectrum plot — thermal, epithermal, and fast regions",
                    "エネルギースペクトルプロット — 熱・エピサーマル・高速領域"
                ),
            )
        });
        let rect = response.rect.shrink2(egui::vec2(56.0, 30.0));
        let axis_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(72, 82, 99));
        painter.rect_stroke(rect, 4.0, axis_stroke, egui::StrokeKind::Inside);

        match &source.energy {
            EnergyDistribution::Monoenergetic { energy_ev } => {
                let e = energy_ev.max(1e-11);
                let (lo, hi) = (e / 10.0, e * 10.0);
                let x_of = |ev: f64| {
                    rect.left() + ((ev.ln() - lo.ln()) / (hi.ln() - lo.ln())) as f32 * rect.width()
                };
                painter.line_segment(
                    [
                        egui::pos2(x_of(e), rect.top()),
                        egui::pos2(x_of(e), rect.bottom()),
                    ],
                    egui::Stroke::new(2.0, theme.brand),
                );
                painter.text(
                    egui::pos2(rect.left() - 8.0, rect.top()),
                    egui::Align2::RIGHT_CENTER,
                    "1.0",
                    egui::FontId::monospace(10.0),
                    theme.text_dim,
                );
                painter.text(
                    rect.center_bottom() + egui::vec2(0.0, 4.0),
                    egui::Align2::CENTER_TOP,
                    format!("monoenergetic {e:.4e} eV"),
                    egui::FontId::monospace(10.0),
                    theme.text_dim,
                );
            }
            EnergyDistribution::TabulatedHistogram {
                energy_boundaries_ev,
                bin_weights,
            } => {
                let e_lo = energy_boundaries_ev
                    .first()
                    .copied()
                    .unwrap_or(1e-11)
                    .max(1e-11);
                let e_hi = energy_boundaries_ev.last().copied().unwrap_or(1e7);
                let w_max = bin_weights.iter().copied().fold(0.0_f64, f64::max);
                if e_hi <= e_lo || w_max <= 0.0 {
                    ui.label("empty or degenerate histogram");
                    return;
                }
                let w_floor = w_max * 1e-6;
                let log_lo = e_lo.ln();
                let log_span = (e_hi.ln() - log_lo).max(f64::EPSILON);
                let w_log_span = (w_max.ln() - w_floor.ln()).max(f64::EPSILON);
                let x_of = |ev: f64| {
                    rect.left() + ((ev.max(1e-11).ln() - log_lo) / log_span) as f32 * rect.width()
                };
                let y_of = |w: f64| {
                    rect.bottom()
                        - ((w.max(w_floor).ln() - w_floor.ln()) / w_log_span) as f32 * rect.height()
                };

                // Region shading behind the curve.
                let regions = [
                    (
                        e_lo,
                        SPECTRUM_THERMAL_UPPER_EV,
                        egui::Color32::from_rgba_unmultiplied(80, 140, 255, 18),
                        "thermal",
                    ),
                    (
                        SPECTRUM_THERMAL_UPPER_EV,
                        SPECTRUM_EPITHERMAL_UPPER_EV,
                        egui::Color32::from_rgba_unmultiplied(80, 220, 140, 14),
                        "epithermal",
                    ),
                    (
                        SPECTRUM_EPITHERMAL_UPPER_EV,
                        e_hi,
                        egui::Color32::from_rgba_unmultiplied(255, 120, 80, 16),
                        "fast",
                    ),
                ];
                for (lo, hi, tint, _) in regions {
                    let left = x_of(lo.max(e_lo));
                    let right = x_of(hi.min(e_hi));
                    if right > left {
                        painter.rect_filled(
                            egui::Rect::from_x_y_ranges(left..=right, rect.top()..=rect.bottom()),
                            0.0,
                            tint,
                        );
                    }
                }
                for boundary in [SPECTRUM_THERMAL_UPPER_EV, SPECTRUM_EPITHERMAL_UPPER_EV] {
                    if boundary > e_lo && boundary < e_hi {
                        let x = x_of(boundary);
                        painter.line_segment(
                            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 128, 145)),
                        );
                    }
                }

                // Staircase polyline.
                let mut points = Vec::with_capacity(2 * bin_weights.len());
                for (i, &w) in bin_weights.iter().enumerate() {
                    let x0 = x_of(energy_boundaries_ev[i]);
                    let x1 = x_of(energy_boundaries_ev[i + 1]);
                    let y = y_of(w);
                    points.push(egui::pos2(x0, y));
                    points.push(egui::pos2(x1, y));
                }
                painter.add(egui::Shape::line(
                    points,
                    egui::Stroke::new(1.6, theme.brand),
                ));

                // Region integrals — log-linear weight density inside
                // straddling bins.
                let total: f64 = bin_weights.iter().sum();
                let mut region_mass = [0.0_f64; 3];
                for (i, &w) in bin_weights.iter().enumerate() {
                    let lo = energy_boundaries_ev[i];
                    let hi = energy_boundaries_ev[i + 1];
                    let ln_span = (hi.ln() - lo.ln()).max(f64::EPSILON);
                    let split = |bound: f64| {
                        ((bound.clamp(lo, hi).ln() - lo.ln()) / ln_span).clamp(0.0, 1.0)
                    };
                    let below_thermal = split(SPECTRUM_THERMAL_UPPER_EV);
                    let below_epi = split(SPECTRUM_EPITHERMAL_UPPER_EV);
                    region_mass[0] += w * below_thermal;
                    region_mass[1] += w * (below_epi - below_thermal);
                    region_mass[2] += w * (1.0 - below_epi);
                }
                for (mass, (_, _, _, name)) in region_mass.iter().zip(regions.iter()) {
                    if *mass > 0.0 {
                        let frac = mass / total.max(f64::EPSILON);
                        ui.monospace(format!("{name}: {frac:.3} of weight"));
                    }
                }

                // Decade ticks.
                let first_decade = e_lo.log10().ceil() as i32;
                let last_decade = e_hi.log10().floor() as i32;
                for decade in first_decade..=last_decade {
                    let x = x_of(10f64.powi(decade));
                    painter.text(
                        egui::pos2(x, rect.bottom() + 4.0),
                        egui::Align2::CENTER_TOP,
                        format!("1e{decade}"),
                        egui::FontId::monospace(10.0),
                        theme.text_dim,
                    );
                }
                painter.text(
                    egui::pos2(rect.left() - 8.0, rect.top()),
                    egui::Align2::RIGHT_CENTER,
                    format!("{w_max:.2e}"),
                    egui::FontId::monospace(10.0),
                    theme.text_dim,
                );
                painter.text(
                    egui::pos2(rect.left() - 8.0, rect.bottom()),
                    egui::Align2::RIGHT_CENTER,
                    format!("{w_floor:.0e}"),
                    egui::FontId::monospace(10.0),
                    theme.text_dim,
                );
                ui.monospace(format!(
                    "bin weight (probability mass) vs energy [eV] · {} bins · Σw = {:.4}",
                    bin_weights.len(),
                    total
                ));
            }
        }
    });
}

#[allow(clippy::too_many_arguments)] // workspace plumbing
fn show_transport_workspace(
    ui: &mut egui::Ui,
    case: Option<&ViewerCase>,
    panel: &mut PositionPanel,
    spectrum: &mut SpectrumView,
    run: &mut RunPanel,
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(language, "Transport", "輸送"),
        t!(
            language,
            "Backend-neutral preparation with explicit scientific and execution gates.",
            "バックエンド中立の準備 — 科学的ゲートと実行ゲートを明示。"
        ),
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
    tour_targets.set(
        TourTarget::SpectrumPanel,
        ui.heading(t!(language, "Source spectrum", "線源スペクトル"))
            .rect,
    );
    ui.label(
        "Drop a beam-description or fixed-source-definition JSON — the energy histogram \
         renders log-log with TECDOC-1223 region shading.",
    );
    show_spectrum(ui, spectrum, language, theme);

    ui.add_space(12.0);
    let gate_chain = ui.scope(|ui| {
        ui.heading(t!(language, "Run gate chain", "ゲートチェーンを実行"));
        for (index, gate) in readiness_gates(case.is_some(), language)
            .into_iter()
            .enumerate()
        {
            ui.horizontal(|ui| {
                ui.monospace(format!("{:02}", index + 1));
                ui.colored_label(gate.state.color(ui.visuals().dark_mode), "●")
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Label,
                            true,
                            gate.state.localized_label(language),
                        )
                    });
                ui.strong(gate.title);
                ui.label("—");
                ui.label(gate.detail);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(gate.state.localized_label(language))
                            .small()
                            .strong()
                            .color(gate.state.color(ui.visuals().dark_mode)),
                    );
                });
            });
            if index + 1 != readiness_gates(case.is_some(), language).len() {
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
        ui.label(t!(
            language,
            "Disabled controls reflect real adapter capabilities.",
            "無効なコントロールは実際のアダプタ能力を反映しています。"
        ));
    });
    tour_targets.set(TourTarget::TransportActions, actions.response.rect);

    ui.add_space(14.0);
    ui.heading(t!(language, "Source positioning", "線源の位置決め"));
    ui.label(t!(language, "Same aim/rotate path as `openbnct position` — reports the entry geometry without running transport.", "`openbnct position` と同じ照準・回転パス — 輸送を実行せず入射ジオメトリのみ報告。"));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(language, "SOURCE", "線源"))
                    .small()
                    .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.source_path)
                    .desired_width(420.0)
                    .hint_text("/path/to/source.json"),
            );
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(language, "TARGET", "ターゲット"))
                    .small()
                    .strong(),
            );
            if let Some(case) = case {
                let names: Vec<&str> = case
                    .data
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
            ui.label(t!(language, "or mask file:", "またはマスクファイル:"));
            ui.add(
                egui::TextEdit::singleline(&mut panel.mask_path)
                    .desired_width(300.0)
                    .hint_text("optional /path/to/mask.json"),
            );
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(language, "APPROACH", "アプローチ"))
                    .small()
                    .strong(),
            );
            egui::ComboBox::from_id_salt("position-approach")
                .selected_text(APPROACHES[panel.approach])
                .show_ui(ui, |ui| {
                    for (index, name) in APPROACHES.iter().enumerate() {
                        ui.selectable_value(&mut panel.approach, index, *name);
                    }
                });
            ui.label(t!(language, "half-widths u/v cm:", "半値幅 u/v cm:"));
            ui.add(egui::TextEdit::singleline(&mut panel.half_width_u_cm).desired_width(50.0));
            ui.add(egui::TextEdit::singleline(&mut panel.half_width_v_cm).desired_width(50.0));
            ui.label(t!(language, "margin cm:", "マージン cm:"));
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
                ui.strong(t!(language, "Position report", "位置レポート"));
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
            ui.label(
                egui::RichText::new(t!(language, "ROTATE", "回転"))
                    .small()
                    .strong(),
            );
            egui::ComboBox::from_id_salt("position-rotate-axis")
                .selected_text(ROTATE_AXES[panel.rotate_axis])
                .show_ui(ui, |ui| {
                    for (index, name) in ROTATE_AXES.iter().enumerate() {
                        ui.selectable_value(&mut panel.rotate_axis, index, *name);
                    }
                });
            ui.label(t!(language, "degrees:", "角度:"));
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
            ui.label(
                egui::RichText::new(t!(language, "SAVE", "保存"))
                    .small()
                    .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.save_source_path)
                    .desired_width(260.0)
                    .hint_text("positioned-source.json"),
            );
            if ui
                .button(t!(language, "Write source", "線源を出力"))
                .clicked()
            {
                panel.save(true);
            }
            ui.add(
                egui::TextEdit::singleline(&mut panel.save_report_path)
                    .desired_width(260.0)
                    .hint_text("position-report.json"),
            );
            if ui
                .button(t!(language, "Write report", "レポートを出力"))
                .clicked()
            {
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

    ui.add_space(14.0);
    tour_targets.set(
        TourTarget::RunPanel,
        ui.heading(t!(language, "Run a subcommand", "サブコマンドを実行"))
            .rect,
    );
    if cfg!(target_arch = "wasm32") {
        ui.label(t!(
            language,
            "Process execution requires the native build — the web inspector is read-only.",
            "プロセス実行はネイティブ版のみ — Web インスペクタは読み取り専用です。"
        ));
    }
    let running = run.poll();
    if running {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(language, "program", "プログラム"));
            ui.add(
                egui::TextEdit::singleline(&mut run.program)
                    .desired_width(120.0)
                    .hint_text("openbnct"),
            );
            ui.label(t!(language, "args", "引数"));
            ui.add(
                egui::TextEdit::singleline(&mut run.args)
                    .desired_width(ui.available_width() - 220.0)
                    .hint_text("--help"),
            );
            ui.label(t!(language, "timeout s", "タイムアウト秒"));
            ui.add(egui::TextEdit::singleline(&mut run.timeout_s).desired_width(50.0));
        });
        // Command presets — the discoverable surface for controls that
        // would otherwise need `--help` knowledge (like --threads).
        ui.horizontal_wrapped(|ui| {
            ui.label(t!(language, "presets:", "プリセット:"));
            for (label, args) in [
                ("sn solve", "sn solve --case case.json --data data.json --output flux.json"),
                (
                    "openmc run",
                    "openmc run --case case.json --component-profile cp.json --material mat.json --source src.json --response-set rs.json --nuclear-data-manifest nd.json --execution-profile ep.json --nuclear-data-root xs/ --openmc openmc --threads 4 --working-directory runs/case --dose-output dose.json",
                ),
                (
                    "uq propagate",
                    "uq propagate --case case.json --data data.json --covariance cov.json --component boron --output budget.json",
                ),
                (
                    "plan directions",
                    "plan directions --case case.json --aim-mask tumor.json --data data.json --top 6",
                ),
                ("prompt-gamma", "prompt-gamma --dose dose.json --id pg --output pg.json"),
            ] {
                if ui
                    .small_button(label)
                    .on_hover_text(args)
                    .clicked()
                {
                    run.args = args.to_owned();
                    if run.program.trim().is_empty() {
                        run.program = "openbnct".into();
                    }
                }
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !running && cfg!(not(target_arch = "wasm32")),
                    egui::Button::new(t!(language, "Run", "実行")),
                )
                .on_disabled_hover_text(if cfg!(target_arch = "wasm32") {
                    t!(language, "native build only", "ネイティブ版のみ")
                } else {
                    t!(language, "a job is already running", "ジョブ実行中")
                })
                .clicked()
            {
                run.start();
            }
            if ui
                .add_enabled(
                    running,
                    egui::Button::new(t!(language, "Cancel", "キャンセル")),
                )
                .clicked()
            {
                run.cancel();
            }
            if let Some(status) = &run.status {
                ui.monospace(status.clone());
            }
        });
        if !run.output.is_empty() {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &run.output {
                        ui.monospace(line);
                    }
                });
        }
    });
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

fn show_plan_workspace(
    ui: &mut egui::Ui,
    panel: &mut PlanPanel,
    drop_sender: Option<&DropSender>,
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(language, "Exposure plan", "照射計画"),
        t!(
            language,
            "Structured multi-exposure schedules; every detected issue is reported, not just the first.",
            "構造化された複数回照射スケジュール — 検出された問題は最初の一件だけでなく全件報告。"
        ),
    );

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(language, "PLAN", "計画"))
                    .small()
                    .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.plan_path)
                    .desired_width((ui.available_width() - 275.0).max(160.0))
                    .hint_text("/path/to/exposure-plan.json"),
            );
            if let Some(path) =
                browse_file_button(ui, drop_sender, "Plan JSON", &["json"], language)
            {
                panel.plan_path = path.display().to_string();
                panel.load();
            }
            if ui
                .button(t!(language, "Load + diagnose", "読込・診断"))
                .clicked()
            {
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
            ui.label(
                egui::RichText::new(t!(language, "TABLE EXPORT", "テーブル出力"))
                    .small()
                    .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.export_path)
                    .desired_width(420.0)
                    .hint_text("/path/to/schedule.csv or .xlsx"),
            );
            if ui
                .button(t!(language, "Export table", "テーブルを出力"))
                .clicked()
            {
                panel.export_table();
            }
        });
        if let Some(status) = &panel.export_status {
            ui.label(status);
        }
    });

    ui.add_space(14.0);
    tour_targets.set(
        TourTarget::RobustnessCards,
        ui.heading(t!(language, "Robustness report", "ロバスト性レポート"))
            .rect,
    );
    ui.label(
        "Drop a plan-robustness .json — per-objective violation probabilities \
         under the declared systematic uncertainties.",
    );
    egui::Frame::group(ui.style()).show(ui, |ui| {
        if let Some(error) = &panel.robustness_error {
            ui.colored_label(theme.error, format!("robustness rejected: {error}"));
        }
        let Some(report) = &panel.robustness else {
            ui.label(t!(
                language,
                "No robustness report loaded.",
                "ロバスト性レポート未読込。"
            ));
            return;
        };
        ui.monospace(&report.id);
        ui.monospace(format!(
            "method: {} · qualification: {}",
            report.method, report.qualification
        ));
        if !report.sources.is_empty() {
            ui.monospace(format!(
                "sources: {}",
                report
                    .sources
                    .iter()
                    .map(|s| format!("{s:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        ui.add_space(6.0);
        for objective in &report.objectives {
            let p = objective.violation_probability.clamp(0.0, 1.0);
            let color = if p > 0.2 {
                theme.error
            } else if p > 0.05 {
                theme.warn_text
            } else {
                egui::Color32::from_rgb(80, 220, 140)
            };
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong(format!("{} · {}", objective.kind, objective.mask));
                    ui.monospace(format!(
                        "achieved {:.4e} vs bound {:.4e} · σ {:.2e}",
                        objective.achieved, objective.bound, objective.sigma_1sigma
                    ));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(color, format!("P(violate) = {:.3}", p));
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(120.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 3.0, egui::Color32::from_rgb(60, 66, 78));
                    let fill = egui::Rect::from_min_size(
                        rect.min,
                        egui::vec2(rect.width() * p as f32, rect.height()),
                    );
                    ui.painter().rect_filled(fill, 3.0, color);
                });
            });
            ui.separator();
        }
        ui.monospace(format!("provenance: {}", report.provenance_id));
    });
}

/// Tri-planar dose/field map rendered from the loaded bundle's own grid —
/// the standalone-inspection path that the web build also exercises.
/// Click/drag on any pane re-centres the shared crosshair.
fn show_dose_map(ui: &mut egui::Ui, panel: &mut DosePanel, language: Language, theme: Theme) {
    let Some(bundle) = &panel.bundle else {
        return;
    };
    let artifact = &bundle.artifact;
    let mut rows = artifact.rows();
    if let Some(source) = &panel.prompt_gamma {
        rows.push((
            "prompt_gamma_478kev".into(),
            source.values.as_slice(),
            source.absolute_standard_uncertainty.as_deref(),
        ));
    }
    if rows.is_empty() {
        return;
    }
    if !rows.iter().any(|(name, ..)| *name == panel.map.quantity) {
        panel.map.quantity = rows
            .last()
            .map(|(name, ..)| name.clone())
            .unwrap_or_default();
    }
    let Some((_, dose_values, sigma_values)) =
        rows.iter().find(|(name, ..)| *name == panel.map.quantity)
    else {
        return;
    };
    // σ view only when the row actually carries an uncertainty field.
    if panel.map.sigma_view && sigma_values.is_none() {
        panel.map.sigma_view = false;
    }
    let values = if panel.map.sigma_view {
        sigma_values.unwrap_or(dose_values)
    } else {
        dose_values
    };
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    let grid = match PatientAlignedGrid::new(artifact.geometry()) {
        Ok(grid) => grid,
        Err(error) => {
            ui.colored_label(
                theme.warn_text,
                format!("dose map needs a patient-aligned grid: {error}"),
            );
            return;
        }
    };
    if !max.is_finite() || max <= 0.0 {
        ui.label(t!(
            language,
            "selected quantity has no positive values to render",
            "選択した物理量に描画可能な正の値がありません"
        ));
        return;
    }
    let Some(voxel) = panel.map.voxel else {
        return;
    };

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(language, "Quantity", "物理量"));
            egui::ComboBox::from_id_salt("dose-map-quantity")
                .selected_text(panel.map.quantity.clone())
                .show_ui(ui, |ui| {
                    for (name, ..) in &rows {
                        ui.selectable_value(&mut panel.map.quantity, name.clone(), name.as_str());
                    }
                });
            ui.checkbox(
                &mut panel.map.log_scale,
                t!(language, "log scale", "対数スケール"),
            );
            ui.checkbox(
                &mut panel.map.contours,
                t!(language, "isodose 90/50/10%", "等線量 90/50/10%"),
            );
            ui.add_enabled_ui(sigma_values.is_some(), |ui| {
                ui.checkbox(&mut panel.map.sigma_view, t!(language, "σ map", "σ マップ"))
                    .on_disabled_hover_text(t!(
                        language,
                        "this component carries no uncertainty field",
                        "この成分は不確かさフィールドを持ちません"
                    ));
            });
        });

        let Ok(mut crosshair) = Crosshair::new(&grid, voxel) else {
            return;
        };
        let key = format!(
            "{}|{}|{:?}|{}|{}|{}",
            bundle.sha256,
            panel.map.quantity,
            voxel,
            panel.map.log_scale,
            panel.map.contours,
            panel.map.sigma_view
        );
        if panel.map.cache_key.as_deref() != Some(key.as_str()) {
            let mut textures: Option<[egui::TextureHandle; 3]> = None;
            let planes = [
                AnatomicalPlane::Axial,
                AnatomicalPlane::Coronal,
                AnatomicalPlane::Sagittal,
            ];
            let mut rendered = Vec::with_capacity(3);
            let mut ok = true;
            for plane in planes {
                match grid.slice(plane, crosshair) {
                    Ok(view) => {
                        match render_dose_map_slice(
                            values,
                            view,
                            max,
                            panel.map.log_scale,
                            panel.map.contours,
                        ) {
                            Ok(image) => rendered.push(ui.ctx().load_texture(
                                format!("dose-map-{plane:?}"),
                                image,
                                egui::TextureOptions::NEAREST,
                            )),
                            Err(error) => {
                                ui.colored_label(
                                    theme.error,
                                    format!("{plane:?} slice failed: {error}"),
                                );
                                ok = false;
                            }
                        }
                    }
                    Err(error) => {
                        ui.colored_label(theme.error, format!("{plane:?} view failed: {error}"));
                        ok = false;
                    }
                }
            }
            if ok && rendered.len() == 3 {
                textures = Some([
                    rendered[0].clone(),
                    rendered[1].clone(),
                    rendered[2].clone(),
                ]);
            }
            panel.map.textures = textures;
            panel.map.cache_key = Some(key);
        }

        let Some(textures) = &panel.map.textures else {
            return;
        };
        ui.columns(3, |columns| {
            let planes = [
                AnatomicalPlane::Axial,
                AnatomicalPlane::Coronal,
                AnatomicalPlane::Sagittal,
            ];
            for (column, (plane, texture)) in
                columns.iter_mut().zip(planes.iter().zip(textures.iter()))
            {
                if let Ok(view) = grid.slice(*plane, crosshair)
                    && let Some(target) =
                        show_slice_view(column, texture, view, crosshair, 220.0, language)
                {
                    let _ = crosshair.set_voxel(&grid, target);
                }
            }
        });
        panel.map.voxel = Some(crosshair.voxel());

        let crosshair_voxel = crosshair.voxel();
        if let Ok(index) = grid.linear_index(crosshair_voxel) {
            let label = if panel.map.sigma_view { "1σ" } else { "value" };
            ui.monospace(format!(
                "crosshair {:?} → {label} {:.4e} {}",
                crosshair_voxel,
                values[index],
                artifact.unit(),
            ));
        }
    });
}

/// Line profile of the selected quantity along one grid axis, through
/// the dose-map crosshair. An optional measurement-record histogram is
/// overlaid peak-normalized — a shape comparison, never a units mix.
fn show_depth_profile(ui: &mut egui::Ui, panel: &mut DosePanel, language: Language, theme: Theme) {
    use openbnct_transport::MeasurementValue;

    let Some(bundle) = &panel.bundle else {
        return;
    };
    let artifact = &bundle.artifact;
    let geometry = artifact.geometry();
    let rows = artifact.rows();
    let Some((_, values, _)) = rows.iter().find(|(name, ..)| *name == panel.map.quantity) else {
        return;
    };
    let shape = geometry.shape;
    let axis = panel.profile.axis.min(2);
    let voxel = panel
        .map
        .voxel
        .unwrap_or_else(|| shape.map(|extent| extent / 2));
    let index_of = |voxel: [u32; 3]| -> usize {
        (voxel[2] as usize * shape[1] as usize + voxel[1] as usize) * shape[0] as usize
            + voxel[0] as usize
    };
    let count = shape[axis] as usize;
    let profile: Vec<(f64, f64)> = (0..count)
        .map(|i| {
            let mut v = voxel;
            v[axis] = i as u32;
            (
                (geometry.origin_mm[axis] + (i as f64 + 0.5) * geometry.spacing_mm[axis]) / 10.0,
                values[index_of(v)],
            )
        })
        .collect();
    let max = profile.iter().map(|point| point.1).fold(0.0_f64, f64::max);
    if !max.is_finite() || max <= 0.0 {
        ui.label(t!(
            language,
            "no positive values along this profile",
            "このプロファイル上に正の値がありません"
        ));
        return;
    }

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(language, "Axis", "軸"));
            for (candidate, label) in [(0_usize, "X"), (1, "Y"), (2, "Z")] {
                ui.selectable_value(&mut panel.profile.axis, candidate, label);
            }
            ui.checkbox(&mut panel.profile.log_y, "log y");
            ui.separator();
            ui.label(t!(
                language,
                "Measurement overlay — drop a measurement-record .json",
                "実測オーバーレイ — measurement-record .json をドロップ"
            ));
            if let Some(record) = &panel.profile.measurement {
                egui::ComboBox::from_id_salt("profile-measurement")
                    .selected_text(
                        record
                            .measurements
                            .get(panel.profile.measurement_pick)
                            .map(|m| m.id.as_str())
                            .unwrap_or("—"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, m) in record.measurements.iter().enumerate() {
                            if matches!(m.value, MeasurementValue::Histogram { .. }) {
                                ui.selectable_value(&mut panel.profile.measurement_pick, i, &m.id);
                            }
                        }
                    });
                if ui.button(t!(language, "clear", "クリア")).clicked() {
                    panel.profile.measurement = None;
                }
            }
        });
        if let Some(error) = &panel.profile.measurement_error {
            ui.colored_label(theme.error, format!("measurement rejected: {error}"));
        }

        // Overlay: peak-normalize the measured histogram onto the
        // computed profile — both series drawn on the computed scale.
        let overlay = panel.profile.measurement.as_ref().and_then(|record| {
            record
                .measurements
                .get(panel.profile.measurement_pick)
                .and_then(|m| match &m.value {
                    MeasurementValue::Histogram {
                        bin_edges,
                        bin_values,
                        bin_uncertainties_1sigma,
                    } => Some((
                        m.id.as_str(),
                        m.unit.as_str(),
                        bin_edges.as_slice(),
                        bin_values.as_slice(),
                        bin_uncertainties_1sigma.as_deref(),
                    )),
                    _ => None,
                })
        });
        let overlay_scale = overlay.and_then(|(_, _, _, values, _)| {
            values
                .iter()
                .copied()
                .fold(0.0_f64, f64::max)
                .gt(&0.0)
                .then(|| max / values.iter().copied().fold(0.0_f64, f64::max))
        });

        let (response, painter) = ui.allocate_painter(
            egui::vec2(ui.available_width(), 200.0),
            egui::Sense::focusable_noninteractive(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                true,
                t!(
                    language,
                    "Line profile plot along the selected axis",
                    "選択軸に沿ったラインプロファイル"
                ),
            )
        });
        let rect = response.rect.shrink2(egui::vec2(50.0, 26.0));
        painter.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(72, 82, 99)),
            egui::StrokeKind::Inside,
        );
        let x_min = profile.first().map(|point| point.0).unwrap_or(0.0);
        let x_max = profile.last().map(|point| point.0).unwrap_or(0.0);
        let x_span = (x_max - x_min).max(f64::EPSILON);
        let y_of = |value: f64| -> f32 {
            if panel.profile.log_y {
                let floor = max * 1e-4;
                let t = (value.max(floor).ln() - floor.ln()) / (max.ln() - floor.ln());
                rect.bottom() - (t as f32) * rect.height()
            } else {
                rect.bottom() - ((value / max) as f32) * rect.height()
            }
        };
        let x_of = |cm: f64| rect.left() + ((cm - x_min) / x_span) as f32 * rect.width();
        let points: Vec<egui::Pos2> = profile
            .iter()
            .map(|(cm, value)| egui::pos2(x_of(*cm), y_of(*value)))
            .collect();
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(2.0, theme.brand),
        ));
        if let (Some((id, unit, edges, values, sigmas)), Some(scale)) = (overlay, overlay_scale) {
            let point_color = egui::Color32::from_rgb(255, 220, 80);
            for (i, &value) in values.iter().enumerate() {
                let cm = (edges[i] + edges[i + 1]) / 2.0;
                let x = x_of(cm);
                let y = y_of(value * scale);
                painter.circle_filled(egui::pos2(x, y), 3.0, point_color);
                if let Some(sigmas) = sigmas {
                    let sigma = sigmas[i] * scale;
                    painter.line_segment(
                        [
                            egui::pos2(x, y_of(value * scale + sigma)),
                            egui::pos2(x, y_of(value * scale - sigma)),
                        ],
                        egui::Stroke::new(1.0, point_color),
                    );
                }
            }
            painter.text(
                rect.right_top() + egui::vec2(-4.0, 4.0),
                egui::Align2::RIGHT_TOP,
                format!("◦ {id} ({unit}) — peak-normalized overlay"),
                egui::FontId::monospace(10.0),
                point_color,
            );
        }
        painter.text(
            egui::pos2(rect.left() - 8.0, rect.top()),
            egui::Align2::RIGHT_CENTER,
            format!("{max:.2e}"),
            egui::FontId::monospace(10.0),
            theme.text_dim,
        );
        painter.text(
            egui::pos2(rect.right(), rect.bottom() + 4.0),
            egui::Align2::RIGHT_TOP,
            format!(
                "position along {}-axis [cm] · {}",
                "XYZ".chars().nth(axis).unwrap_or('Z'),
                artifact.unit()
            ),
            egui::FontId::monospace(10.0),
            theme.text_dim,
        );
        ui.monospace(format!(
            "profile through crosshair voxel {:?} — move it in the dose map above",
            voxel
        ));
    });
}

/// Diverging ratio color: log2(B/A) mapped blue→white→red over
/// [0.5×, 2×]; undefined voxels (A ≤ 0) render dark.
fn ratio_color(ratio: Option<f64>) -> egui::Color32 {
    let Some(r) = ratio else {
        return egui::Color32::from_rgb(30, 32, 38);
    };
    if !r.is_finite() || r <= 0.0 {
        return egui::Color32::from_rgb(30, 32, 38);
    }
    let t = ((r.log2() + 1.0) / 2.0).clamp(0.0, 1.0) as f32;
    let (low, mid, high) = (
        (80u8, 140u8, 255u8),
        (255u8, 255u8, 255u8),
        (255u8, 90u8, 60u8),
    );
    let lerp = |a: (u8, u8, u8), b: (u8, u8, u8), t: f32| {
        egui::Color32::from_rgb(
            (a.0 as f32 + (b.0 as f32 - a.0 as f32) * t) as u8,
            (a.1 as f32 + (b.1 as f32 - a.1 as f32) * t) as u8,
            (a.2 as f32 + (b.2 as f32 - a.2 as f32) * t) as u8,
        )
    };
    if t < 0.5 {
        lerp(low, mid, t * 2.0)
    } else {
        lerp(mid, high, (t - 0.5) * 2.0)
    }
}

/// Render one ratio slice (B/A) through the shared crosshair voxel.
fn render_ratio_slice(
    a_values: &[f64],
    b_values: &[f64],
    view: SliceView,
) -> Result<egui::ColorImage, ViewError> {
    let a = view.extract(a_values)?;
    let b = view.extract(b_values)?;
    let dimensions = view.dimensions();
    let mut pixels = Vec::with_capacity(a.len());
    for (a_v, b_v) in a.iter().zip(b.iter()) {
        let ratio = (*a_v > 0.0 && b_v.is_finite()).then(|| b_v / a_v);
        pixels.push(ratio_color(ratio));
    }
    Ok(egui::ColorImage::new(
        [dimensions[0] as usize, dimensions[1] as usize],
        pixels,
    ))
}

/// A/B bundle diff: a drop zone loads the second artifact, geometry
/// equivalence is verified, then a B/A ratio map renders on the shared
/// crosshair plus aggregate stats and the two content bindings.
fn show_dose_compare(ui: &mut egui::Ui, panel: &mut DosePanel, language: Language, theme: Theme) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        // The drop zone is painted every frame so handle_dropped can hit-test.
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::hover());
        panel.compare_zone = rect;
        let hovering = ui
            .input(|i| i.pointer.hover_pos())
            .is_some_and(|p| rect.contains(p));
        let fill = if hovering {
            egui::Color32::from_rgb(46, 56, 74)
        } else {
            egui::Color32::from_rgb(34, 38, 48)
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        ui.painter().rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(1.0, theme.text_dim),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if panel.compare.is_some() {
                t!(
                    language,
                    "drop a dose bundle here to replace B · drops elsewhere replace A",
                    "ここに線量バンドルをドロップで B を置換 · ゾーン外は A を置換"
                )
            } else {
                t!(
                    language,
                    "drop a second dose bundle (B) here to diff against the loaded A",
                    "2つ目の線量バンドル(B)をここにドロップして読込済み A と比較"
                )
            },
            egui::FontId::monospace(11.0),
            theme.text_dim,
        );
        if let Some(error) = &panel.compare_error {
            ui.colored_label(
                theme.error,
                format!("{}: {error}", t!(language, "B rejected", "B 拒否")),
            );
        }
        let (Some(a), Some(b)) = (&panel.bundle, &panel.compare) else {
            return;
        };

        // Provenance header — both content bindings, always visible.
        ui.monospace(format!(
            "A: case {} · sha256:{}… · provenance {}",
            a.artifact.case_id(),
            &a.sha256[..12.min(a.sha256.len())],
            a.artifact.provenance_id()
        ));
        ui.monospace(format!(
            "B: case {} · sha256:{}… · provenance {}",
            b.artifact.case_id(),
            &b.sha256[..12.min(b.sha256.len())],
            b.artifact.provenance_id()
        ));
        if ui.button(t!(language, "clear B", "B をクリア")).clicked() {
            panel.compare = None;
            panel.compare_textures = None;
            panel.compare_cache_key = None;
            return;
        }

        // Geometry equivalence — the diff is meaningless otherwise.
        let (ga, gb) = (a.artifact.geometry(), b.artifact.geometry());
        let close = |x: &[f64; 3], y: &[f64; 3]| {
            x.iter()
                .zip(y)
                .all(|(u, v)| (u - v).abs() <= 1e-9 * v.abs().max(1.0))
        };
        if ga.shape != gb.shape
            || !close(&ga.spacing_mm, &gb.spacing_mm)
            || !close(&ga.origin_mm, &gb.origin_mm)
        {
            ui.colored_label(
                theme.error,
                "grid mismatch — A/B diff needs identical shape, spacing, origin",
            );
            return;
        }
        let Some((_, a_values, _)) = a
            .artifact
            .rows()
            .into_iter()
            .find(|(name, ..)| *name == panel.map.quantity)
        else {
            return;
        };
        let Some((_, b_values, _)) = b
            .artifact
            .rows()
            .into_iter()
            .find(|(name, ..)| *name == panel.map.quantity)
        else {
            ui.colored_label(
                theme.warn_text,
                format!(
                    "B carries no row {:?} — pick a quantity present in both",
                    panel.map.quantity
                ),
            );
            return;
        };
        if a_values.len() != b_values.len() {
            ui.colored_label(theme.error, "row length mismatch despite equal grids");
            return;
        }

        // Aggregate ratio stats over voxels where A > 0.
        let mut ratios = Vec::new();
        let mut undefined = 0usize;
        for (a_v, b_v) in a_values.iter().zip(b_values.iter()) {
            if *a_v > 0.0 && b_v.is_finite() {
                ratios.push(b_v / a_v);
            } else {
                undefined += 1;
            }
        }
        if ratios.is_empty() {
            ui.label(t!(
                language,
                "no overlapping nonzero voxels to compare",
                "比較可能な重なり合う非ゼロボクセルがありません"
            ));
            return;
        }
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        let max_dev = ratios.iter().map(|r| (r - 1.0).abs()).fold(0.0, f64::max);
        let out5 = ratios.iter().filter(|r| **r > 1.05 || **r < 0.95).count();
        ui.monospace(format!(
            "B/A over {} voxels ({} undefined): mean {:.4} · max |dev| {:.3} · {:.1}% outside ±5%",
            ratios.len(),
            undefined,
            mean,
            max_dev,
            100.0 * out5 as f64 / ratios.len() as f64,
        ));

        // Tri-planar ratio map on the shared dose-map crosshair.
        let Ok(grid) = PatientAlignedGrid::new(ga) else {
            ui.colored_label(theme.warn_text, "ratio map needs a patient-aligned grid");
            return;
        };
        let voxel = panel.map.voxel.unwrap_or_else(|| ga.shape.map(|e| e / 2));
        let Ok(mut crosshair) = Crosshair::new(&grid, voxel) else {
            return;
        };
        let key = format!(
            "{}|{}|{}|{:?}",
            a.sha256, b.sha256, panel.map.quantity, voxel
        );
        if panel.compare_cache_key.as_deref() != Some(key.as_str()) {
            let planes = [
                AnatomicalPlane::Axial,
                AnatomicalPlane::Coronal,
                AnatomicalPlane::Sagittal,
            ];
            let mut rendered = Vec::with_capacity(3);
            let mut ok = true;
            for plane in planes {
                match grid
                    .slice(plane, crosshair)
                    .and_then(|view| render_ratio_slice(a_values, b_values, view))
                {
                    Ok(image) => rendered.push(ui.ctx().load_texture(
                        format!("ratio-map-{plane:?}"),
                        image,
                        egui::TextureOptions::NEAREST,
                    )),
                    Err(error) => {
                        ui.colored_label(theme.error, format!("{plane:?} ratio slice: {error}"));
                        ok = false;
                    }
                }
            }
            panel.compare_textures = (ok && rendered.len() == 3).then(|| {
                [
                    rendered[0].clone(),
                    rendered[1].clone(),
                    rendered[2].clone(),
                ]
            });
            panel.compare_cache_key = Some(key);
        }
        if let Some(textures) = &panel.compare_textures {
            ui.columns(3, |columns| {
                let planes = [
                    AnatomicalPlane::Axial,
                    AnatomicalPlane::Coronal,
                    AnatomicalPlane::Sagittal,
                ];
                for (column, (plane, texture)) in
                    columns.iter_mut().zip(planes.iter().zip(textures.iter()))
                {
                    if let Ok(view) = grid.slice(*plane, crosshair)
                        && let Some(target) =
                            show_slice_view(column, texture, view, crosshair, 220.0, language)
                    {
                        let _ = crosshair.set_voxel(&grid, target);
                    }
                }
            });
            // Clicks on the ratio panes steer the shared dose-map crosshair.
            panel.map.voxel = Some(crosshair.voxel());
            ui.monospace(
                "B/A ratio · blue < 0.5× · white = 1.0 · red > 2× · dark = undefined (A ≤ 0)",
            );
        }
    });
}

/// Prompt-gamma derivation section: turns the loaded bundle's boron
/// dose into the 478 keV production map that BNCT-SPECT/Compton-camera
/// research consumes — a physics source term, not an image. The derived
/// map is selectable as a dose-map quantity and exportable as the
/// versioned `prompt-gamma-source` artifact.
/// Render a dropped `dose-uncertainty-budget` report: total relative σ
/// up front, then the top variance contributors — the "where does the
/// uncertainty come from" table the artifact exists to answer.
fn show_uncertainty_budget(
    ui: &mut egui::Ui,
    panel: &mut DosePanel,
    language: Language,
    theme: Theme,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        if let Some(error) = &panel.uncertainty_budget_error {
            ui.colored_label(theme.error, error.clone());
        }
        match &panel.uncertainty_budget {
            None => {
                ui.label(t!(
                    language,
                    "Drop a dose-uncertainty-budget JSON (from `uq propagate`) to inspect the variance decomposition.",
                    "dose-uncertainty-budget JSON（`uq propagate` 出力）をドロップすると分散分解を表示します。"
                ));
            }
            Some(budget) => {
                ui.label(format!(
                    "{}: {} · {} {:.4} Gy·cm² · {} ±{:.1}%",
                    t!(language, "component", "成分"),
                    budget.component,
                    t!(language, "response", "応答"),
                    budget.response_integral,
                    t!(language, "total σ", "総 σ"),
                    budget.total_relative_std_dev * 100.0
                ));
                ui.label(
                    egui::RichText::new(&budget.method_note)
                        .small()
                        .weak(),
                );
                let mut entries: Vec<&openbnct_transport::BudgetEntry> =
                    budget.entries.iter().collect();
                entries.sort_by(|a, b| {
                    b.relative_contribution
                        .abs()
                        .partial_cmp(&a.relative_contribution.abs())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                egui::Grid::new("uq-budget")
                    .num_columns(4)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.monospace(t!(language, "parameter", "パラメータ"));
                        ui.monospace(t!(language, "source", "ソース"));
                        ui.monospace(t!(language, "σ param", "パラメータ σ"));
                        ui.monospace(t!(language, "variance share", "分散寄与"));
                        ui.end_row();
                        for entry in entries.iter().take(12) {
                            ui.monospace(&entry.parameter);
                            ui.monospace(&entry.source);
                            ui.monospace(format!("{:.4}", entry.std_dev));
                            ui.monospace(format!("{:.1}%", entry.relative_contribution * 100.0));
                            ui.end_row();
                        }
                    });
                if budget.entries.len() > 12 {
                    ui.small(format!(
                        "… {} {}",
                        budget.entries.len() - 12,
                        t!(language, "more entries", "件の追加項目")
                    ));
                }
            }
        }
    });
}

fn show_prompt_gamma(ui: &mut egui::Ui, panel: &mut DosePanel, language: Language, theme: Theme) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let physical = panel
            .bundle
            .as_ref()
            .and_then(|loaded| match &loaded.artifact {
                DoseArtifact::Physical(bundle) => Some(bundle),
                DoseArtifact::Biological(_) => None,
            });
        ui.label(t!(
            language,
            "Derive the 478 keV ¹⁰B(n,α)⁷Li emission map (94% branch) — the source term for prompt-gamma imaging research.",
            "478 keV ¹⁰B(n,α)⁷Li 放出マップを生成(94% 分枝)— 即発ガンマイメージング研究の線源項。"
        ));
        ui.horizontal(|ui| {
            let enabled = physical.is_some();
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(t!(
                        language,
                        "Derive prompt-gamma source",
                        "即発ガンマ線源を生成"
                    )),
                )
                .on_disabled_hover_text(t!(
                    language,
                    "requires a physical dose bundle with a boron component",
                    "ホウ素成分を含む物理線量バンドルが必要"
                ));
            if response.clicked()
                && let Some(bundle) = physical
            {
                let loaded = panel.bundle.as_ref().expect("bundle present");
                let parent = openbnct_core::ContentReference {
                    id: bundle.provenance_id.clone(),
                    sha256: loaded.sha256.clone(),
                };
                let provenance = format!("prompt-gamma:{}", bundle.provenance_id);
                match openbnct_transport::derive_prompt_gamma_source(
                    bundle,
                    "openbnct.prompt-gamma-source",
                    parent,
                    &provenance,
                ) {
                    Ok(source) => {
                        panel.prompt_gamma = Some(source);
                        panel.prompt_gamma_error = None;
                        panel.map.quantity = "prompt_gamma_478kev".into();
                        panel.map.textures = None;
                        panel.map.cache_key = None;
                    }
                    Err(error) => {
                        panel.prompt_gamma_error = Some(error.to_string());
                    }
                }
            }
            if let Some(source) = &panel.prompt_gamma {
                let total: f64 = source.values.iter().sum();
                ui.monospace(format!(
                    "emission {:.0} keV · branch {:.2} · Σ yield {:.4e} {}",
                    source.emission_energy_ev / 1.0e3,
                    source.branching_ratio,
                    total,
                    match source.unit {
                        openbnct_transport::PromptGammaUnit::PhotonsPerKg => "γ/kg",
                        openbnct_transport::PromptGammaUnit::PhotonsPerKgPerSourceParticle =>
                            "γ/kg/src",
                    }
                ));
                if ui
                    .button(t!(language, "Export JSON", "JSON を保存"))
                    .clicked()
                    && let Ok(json) = serde_json::to_vec_pretty(source)
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("JSON", &["json"])
                        .set_file_name("prompt-gamma-source.json")
                        .save_file()
                    {
                        panel.prompt_gamma_error = io::write_bytes(&path, &json).err();
                    }
                    #[cfg(target_arch = "wasm32")]
                    crate::web::download_bytes("prompt-gamma-source.json", &json);
                }
            }
        });
        if let Some(error) = &panel.prompt_gamma_error {
            ui.colored_label(
                theme.error,
                format!("{}: {error}", t!(language, "prompt-gamma failed", "生成失敗")),
            );
        }
    });
}

#[allow(clippy::too_many_arguments)] // workspace renderers are pure plumbing
fn show_dose_workspace(
    ui: &mut egui::Ui,
    panel: &mut DosePanel,
    nifti: &mut NiftiPanel,
    drop_sender: Option<&DropSender>,
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(language, "Dose components", "線量成分"),
        t!(
            language,
            "Physical and biological layers stay separate; only validated bundles render.",
            "物理線量と生物学的線量は別レイヤー — 検証済みバンドルのみ描画。"
        ),
    );

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(language, "DOSE BUNDLE", "線量バンドル"))
                    .small()
                    .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.bundle_path)
                    .desired_width(460.0)
                    .hint_text("/path/to/dose-bundle.json — or drop it here"),
            );
            if ui
                .button(t!(language, "Load + validate", "読込・検証"))
                .clicked()
            {
                panel.load_bundle();
            }
            if let Some(file) =
                browse_file_button(ui, drop_sender, "dose bundle", &["json"], language)
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
                ui.small(if let Some(sigma) = sigma {
                    let sigma_mean = sigma.iter().sum::<f64>() / sigma.len().max(1) as f64;
                    if mean > 0.0 {
                        format!("1σ mean/mean = {:.1}%", 100.0 * sigma_mean / mean)
                    } else {
                        "1σ uncertainty present".to_string()
                    }
                } else {
                    "no uncertainty carried".to_string()
                });
            });
        }
    });
    ui.monospace(format!("unit: {}", artifact.unit()));

    ui.add_space(14.0);
    tour_targets.set(
        TourTarget::DoseMap,
        ui.heading(t!(language, "Dose map", "線量マップ")).rect,
    );
    show_dose_map(ui, panel, language, theme);

    ui.add_space(14.0);
    ui.heading(t!(language, "Line profile", "ラインプロファイル"));
    show_depth_profile(ui, panel, language, theme);

    ui.add_space(14.0);
    ui.heading(t!(language, "A/B compare", "A/B 比較"));
    show_dose_compare(ui, panel, language, theme);
    ui.add_space(12.0);
    ui.heading(t!(language, "Prompt-gamma source", "即発ガンマ線源"));
    show_prompt_gamma(ui, panel, language, theme);
    ui.add_space(12.0);
    ui.heading(t!(language, "Uncertainty budget", "不確かさバジェット"));
    show_uncertainty_budget(ui, panel, language, theme);
    if panel.compare_zone != egui::Rect::NOTHING {
        tour_targets.set(TourTarget::CompareZone, panel.compare_zone);
    }

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        "Region dose-volume histogram",
        "領域線量-体積ヒストグラム"
    ));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(language, "Mask", "マスク"));
            ui.add(
                egui::TextEdit::singleline(&mut panel.mask_path)
                    .desired_width(360.0)
                    .hint_text("/path/to/region-mask.json"),
            );
            ui.label(t!(language, "Quantity", "物理量"));
            ui.add(
                egui::TextEdit::singleline(&mut panel.quantity)
                    .desired_width(170.0)
                    .hint_text("physical_total"),
            );
            if ui
                .button(t!(language, "Compute DVH", "DVH を計算"))
                .clicked()
            {
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
            show_dvh_curve(ui, histogram, language, theme);
        }
    });

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        "Region dose-volume metrics",
        "領域線量-体積指標"
    ));
    ui.label(t!(language, "Same `RegionDoseMetrics::compute` path as `openbnct metrics` and `compute_metrics` in Python.", "`openbnct metrics`・Python の `compute_metrics` と同じ `RegionDoseMetrics::compute` パス。"));
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
            if ui
                .button(t!(language, "Compute metrics", "指標を計算"))
                .clicked()
            {
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
                if ui
                    .button(t!(language, "Write metrics", "指標を出力"))
                    .clicked()
                {
                    panel.save_metrics();
                }
            });
            if let Some(status) = &panel.metrics_status {
                ui.colored_label(GateState::Verified.color(ui.visuals().dark_mode), status);
            }
        }
    });

    ui.add_space(14.0);
    ui.heading(t!(language, "NIfTI volumes", "NIfTI ボリューム"));
    ui.label("Same `openbnct-nifti` paths as `openbnct nifti` — sform-preferred RAS→LPS handling.");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Volume");
            ui.add(
                egui::TextEdit::singleline(&mut nifti.input_path)
                    .desired_width(300.0)
                    .hint_text("/path/to/volume.nii[.gz]"),
            );
            if ui.button(t!(language, "Inspect", "検査")).clicked() {
                nifti.inspect();
            }
            if let Some(file) =
                browse_file_button(ui, drop_sender, "NIfTI", &["nii", "gz"], language)
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
                if ui
                    .button(t!(language, "Write mask", "マスクを出力"))
                    .clicked()
                {
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
                if ui.button(t!(language, "Resample", "リサンプル")).clicked() {
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
            if ui.button(t!(language, "Export", "エクスポート")).clicked() {
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
fn show_dvh_curve(
    ui: &mut egui::Ui,
    histogram: &DoseVolumeHistogram,
    language: Language,
    theme: Theme,
) {
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width(), 180.0),
        egui::Sense::focusable_noninteractive(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Other,
            true,
            t!(
                language,
                "Dose-volume histogram — cumulative volume fraction vs dose",
                "線量-体積ヒストグラム — 線量対累積体積割合"
            ),
        )
    });
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
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(language, "Evidence", "エビデンス"),
        t!(
            language,
            "Qualification is a chain of scoped claims, not one global green check.",
            "適格性は一つの総合判定ではなく、範囲を限定した主張の連鎖。"
        ),
    );

    let geometry_detail = case.map_or_else(
        || "No runtime case has been verified in this session.".to_owned(),
        |case| format!("{} — {}.", case.data.provenance, case.data.case_id),
    );
    let manifest_hash = short_evidence_hash(OPENMC_MANIFEST_EVIDENCE);
    let execution_hash = short_evidence_hash(NJOY_EXECUTION_EVIDENCE);
    let comparison_hash = short_evidence_hash(HEATING_COMPARISON_EVIDENCE);
    let ledger = ui.scope(|ui| {
        show_evidence_row(
            ui,
            t!(language, "Runtime geometry gate", "実行時ジオメトリゲート"),
            if case.is_some() {
                GateState::Verified
            } else {
                GateState::InputRequired
            },
            &geometry_detail,
            None,
            language,
        );
        show_evidence_row(
            ui,
            t!(
                language,
                "Official OpenMC processed selection",
                "公式 OpenMC 処理済みデータ選択"
            ),
            GateState::Frozen,
            t!(
                language,
                "Case manifest binds cross_sections.xml, ten neutron tables, and five photon tables.",
                "症例マニフェストが cross_sections.xml、中性子テーブル10件、光子テーブル5件を紐付け。"
            ),
            Some(&manifest_hash),
            language,
        );
        show_evidence_row(
            ui,
            t!(
                language,
                "Controlled NJOY2016.78 execution",
                "制御付き NJOY2016.78 実行"
            ),
            GateState::Blocked,
            t!(
                language,
                "Preserved rejected evidence: 72 kinematic findings across four nuclides.",
                "棄却された証拠を保存: 4核種にわたる72件の運動学的所見。"
            ),
            Some(&execution_hash),
            language,
        );
        show_evidence_row(
            ui,
            t!(
                language,
                "OpenMC / NJOY MT 301 comparison",
                "OpenMC / NJOY MT 301 比較"
            ),
            GateState::Frozen,
            t!(
                language,
                "All ten curves agree within 4.9e-7; O-17/O-18 local fallback remains explicit.",
                "全10曲線が 4.9e-7 以内で一致。O-17/O-18 のローカルフォールバックは明示的。"
            ),
            Some(&comparison_hash),
            language,
        );
    });
    tour_targets.set(TourTarget::EvidenceLedger, ledger.response.rect);

    ui.add_space(12.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.heading(t!(language, "Qualification ceiling", "適格範囲の上限"));
        ui.label(
            egui::RichText::new("synthetic_research_only")
                .monospace()
                .strong(),
        );
        ui.label(t!(
            language,
            "Acquisition identity, transport capability, response suitability, execution,              cross-code comparison, and experimental validation remain separate claims.",
            "取得同一性・輸送能力・応答適格性・実行・クロスコード比較・実験的検証は個別の主張です。"
        ));
    });

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        "Exported evidence bundle",
        "エクスポート済みエビデンス"
    ));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Bundle root");
            ui.add(
                egui::TextEdit::singleline(&mut panel.root)
                    .desired_width(460.0)
                    .hint_text("/path/to/evidence-bundle"),
            );
            if ui
                .button(t!(
                    language,
                    "Verify manifest + hashes",
                    "マニフェスト+ハッシュ検証"
                ))
                .clicked()
            {
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
    language: Language,
) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.colored_label(state.color(ui.visuals().dark_mode), "●")
                .widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Label,
                        true,
                        state.localized_label(language),
                    )
                });
            ui.vertical(|ui| {
                ui.strong(title);
                ui.label(detail);
                if let Some(hash) = hash {
                    ui.monospace(format!("sha256:{hash}…"));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                status_badge(ui, state, state.localized_label(language))
            });
        });
    });
}

fn show_case_summary(ui: &mut egui::Ui, case: &ViewerCase, language: Language) {
    ui.heading(t!(language, "Case", "症例"));
    ui.strong(&case.data.case_id);
    ui.small(case.root.display().to_string());
    ui.label(format!(
        "Grid: {:?} at {:?} mm",
        case.data.ct.geometry.shape, case.data.ct.geometry.spacing_mm
    ));
    ui.colored_label(
        GateState::Verified.color(ui.visuals().dark_mode),
        case.data.provenance.clone(),
    );
    ui.label(t!(
        language,
        "Qualification: synthetic research only",
        "適格範囲: 研究用途のみ"
    ));

    ui.separator();
    ui.heading(t!(language, "Crosshair", "クロスヘア"));
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
        let stored = case.data.ct.stored_pixels[index];
        ui.monospace(format!(
            "CT {:.1} HU (stored {stored})",
            case.data.ct.modality_value(stored)
        ));
        let names: Vec<_> = case
            .data
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
    language: Language,
    theme: Theme,
) -> bool {
    let mut changed = false;
    ui.strong(t!(language, "Window & overlays", "ウィンドウ・重ね表示"));
    changed |= ui
        .add(
            egui::Slider::new(&mut display.window_center, -1_024.0..=3_071.0).text(t!(
                language,
                "level HU",
                "レベル HU"
            )),
        )
        .changed();
    changed |= ui
        .add(
            egui::Slider::new(&mut display.window_width, 1.0..=4_096.0)
                .text(t!(language, "width HU", "幅 HU")),
        )
        .changed();
    changed |= ui
        .add(
            egui::Slider::new(&mut display.overlay_opacity, 0.0..=1.0).text(t!(
                language,
                "ROI opacity",
                "ROI 不透明度"
            )),
        )
        .changed();

    ui.separator();
    ui.strong(t!(language, "Crosshair position", "クロスヘア位置"));
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
    ui.strong(t!(language, "Structures", "輪郭構造"));
    for ((visible, roi), color) in case
        .roi_visible
        .iter_mut()
        .zip(&case.data.structures.rois)
        .zip(
            case.data
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
    ui.strong(t!(language, "Dose overlay", "線量オーバーレイ"));
    ui.label(t!(
        language,
        "Load a dose bundle for this case to wash it over the image.",
        "この症例の線量バンドルを読み込むと画像に重ねて表示します。"
    ));
    ui.horizontal(|ui| {
        ui.label(t!(language, "bundle", "バンドル"));
        ui.add(
            egui::TextEdit::singleline(&mut case.dose.path)
                .hint_text("dose-bundle.json")
                .desired_width(160.0),
        );
    });
    if ui
        .button(t!(language, "Load dose bundle", "線量バンドルを読込"))
        .clicked()
    {
        case.dose.load(&case.data);
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
            .checkbox(
                &mut case.dose.enabled,
                t!(language, "show dose wash", "線量ウォッシュを表示"),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut case.dose.opacity, 0.0..=1.0).text(t!(
                    language,
                    "dose opacity",
                    "線量不透明度"
                )),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut case.dose.threshold_percent, 0.0..=100.0).text(t!(
                    language,
                    "wash ≥ % of max",
                    "ウォッシュ ≥ 最大値の%"
                )),
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
    language: Language,
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
    // The image is painted, so the a11y tree gets an explicit summary;
    // focusable for the keyboard crosshair below.
    let plane_name = view.plane().name();
    let index = view.fixed_index();
    response.widget_info(move || {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Image,
            true,
            format!(
                "{} — {} {}",
                match language {
                    Language::Japanese => "スライス",
                    Language::English => "slice",
                },
                plane_name,
                index
            ),
        )
    });
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
    if response.has_focus() {
        // Arrow keys move the in-plane crosshair pixel; PageUp/PageDown
        // step along the fixed axis. Returns the stepped voxel so the
        // shared crosshair updates identically to a pointer click.
        let input = ui.input(|input| {
            (
                input.key_pressed(egui::Key::ArrowLeft),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::PageUp),
                input.key_pressed(egui::Key::PageDown),
            )
        });
        let (left, right, up, down, page_up, page_down) = input;
        if (left || right || up || down)
            && let Ok(pixel) = view.pixel_for_voxel(crosshair.voxel())
        {
            let step = [right as i32 - left as i32, down as i32 - up as i32];
            let target = [
                (pixel[0] as i32 + step[0]).clamp(0, dimensions[0] as i32 - 1) as u32,
                (pixel[1] as i32 + step[1]).clamp(0, dimensions[1] as i32 - 1) as u32,
            ];
            if let Ok(voxel) = view.voxel_at(target) {
                return Some(voxel);
            }
        }
        if page_up || page_down {
            let axis = view.plane().fixed_axis();
            let extent = view.volume_shape()[axis];
            let mut voxel = crosshair.voxel();
            let current = voxel[axis] as i32;
            voxel[axis] =
                (current + page_up as i32 - page_down as i32).clamp(0, extent as i32 - 1) as u32;
            if voxel != crosshair.voxel() {
                return Some(voxel);
            }
        }
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
    case: &CaseData,
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

/// Render one plane of a dose/field map straight from the bundle's own
/// grid — no case image required, so the web build can show it. Optional
/// log normalization spans four decades; isodose contours mark the
/// 90/50/10%-of-max boundaries.
fn render_dose_map_slice(
    values: &[f64],
    view: SliceView,
    max: f64,
    log_scale: bool,
    contours: bool,
) -> Result<egui::ColorImage, String> {
    let slice = view.extract(values).map_err(|error| error.to_string())?;
    let dimensions = view.dimensions();
    let (width, height) = (dimensions[0] as usize, dimensions[1] as usize);
    let floor = (max * 1e-4).max(f64::MIN_POSITIVE);
    let log_range = (max.ln() - floor.ln()).max(f64::EPSILON);
    let mut pixels = Vec::with_capacity(width * height);
    for &value in &slice {
        let t = if log_scale {
            ((value.max(floor).ln() - floor.ln()) / log_range).clamp(0.0, 1.0)
        } else {
            (value / max).clamp(0.0, 1.0)
        };
        pixels.push(dose_wash_color(t));
    }
    if contours {
        for (level, color) in [
            (0.9_f64, egui::Color32::WHITE),
            (0.5, egui::Color32::from_rgb(80, 200, 255)),
            (0.1, egui::Color32::from_rgb(255, 220, 80)),
        ] {
            let threshold = level * max;
            for y in 0..height {
                for x in 0..width {
                    let index = y * width + x;
                    if slice[index] < threshold {
                        continue;
                    }
                    let boundary = [
                        x > 0 && slice[index - 1] < threshold,
                        x + 1 < width && slice[index + 1] < threshold,
                        y > 0 && slice[index - width] < threshold,
                        y + 1 < height && slice[index + width] < threshold,
                    ]
                    .into_iter()
                    .any(|crosses| crosses);
                    if boundary {
                        pixels[index] = color;
                    }
                }
            }
        }
    }
    Ok(egui::ColorImage::new([width, height], pixels))
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

fn show_empty_state(ui: &mut egui::Ui, language: Language) {
    ui.vertical_centered(|ui| {
        ui.add_space(90.0);
        ui.heading(t!(language, "No case loaded", "症例が読み込まれていません"));
        ui.label(t!(
            language,
            "Generate the frozen benchmark from a terminal:",
            "凍結ベンチマークをターミナルで生成:"
        ));
        ui.monospace("cargo run --bin openbnct -- benchmark generate /tmp/nf-bnct-001");
        ui.label(t!(
            language,
            "…or drop a DICOM study's files anywhere — one CT series + one RTSTRUCT.",
            "…または DICOM スタディのファイルをドロップ — CT シリーズ1件 + RTSTRUCT 1件。"
        ));
        ui.add_space(20.0);
        ui.label(t!(
            language,
            "DICOM and artifact-integrity gates run before any image is displayed.",
            "画像表示の前に DICOM・完全性ゲートが実行されます。"
        ));
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
    fn case_member_names_normalize_to_manifest_paths() {
        assert_eq!(
            io::case_member_name("case.json").as_deref(),
            Some("case.json")
        );
        assert_eq!(
            io::case_member_name("RTSTRUCT.DCM").as_deref(),
            Some("rtstruct.dcm")
        );
        assert_eq!(
            io::case_member_name("ct/ct-007.dcm").as_deref(),
            Some("ct/ct-007.dcm")
        );
        assert_eq!(
            io::case_member_name("nf-bnct-001/ct/ct-007.dcm").as_deref(),
            Some("ct/ct-007.dcm")
        );
        assert_eq!(io::case_member_name("dose-bundle.json"), None);
        assert_eq!(io::case_member_name("other.dcm"), None);
        assert_eq!(
            classify_dropped_name("nf-bnct-001.zip"),
            DropTarget::CaseArchive
        );
        assert_eq!(classify_dropped_name("case.json"), DropTarget::CaseFile);
        assert_eq!(classify_dropped_name("ct-012.dcm"), DropTarget::CaseFile);
    }

    #[test]
    fn missing_case_members_reports_the_gap() {
        let files = vec![("case.json".to_string(), b"{}".to_vec())];
        let missing = missing_case_members(&files);
        assert!(missing.iter().any(|entry| entry == "rtstruct.dcm"));
        assert!(missing.iter().any(|entry| entry.contains("CT slice")));

        let mut full = vec![
            ("case.json".to_string(), b"{}".to_vec()),
            ("rtstruct.dcm".to_string(), b"rt".to_vec()),
        ];
        for index in 0..40 {
            full.push((format!("ct/ct-{index:03}.dcm"), b"ct".to_vec()));
        }
        assert!(missing_case_members(&full).is_empty());
    }

    #[test]
    fn case_archive_extracts_manifest_relative_members() {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("nf-bnct-001/case.json", &b"{}"[..]),
                ("nf-bnct-001/rtstruct.dcm", b"rt"),
                ("nf-bnct-001/ct/ct-000.dcm", b"ct0"),
                ("nf-bnct-001/notes.txt", b"ignored"),
            ] {
                writer.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut writer, body).unwrap();
            }
            writer.finish().unwrap();
        }
        let files = io::unzip_case_archive(&cursor.into_inner()).unwrap();
        let names: Vec<_> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["case.json", "rtstruct.dcm", "ct/ct-000.dcm"]);
        assert!(io::unzip_case_archive(b"not a zip").is_err());
        assert!(io::unzip_case_archive(&[]).is_err());
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
            let gates = readiness_gates(case_loaded, Language::English);
            assert_eq!(gates[3].state, GateState::Blocked);
            assert_eq!(gates[4].state, GateState::Pending);
        }
        assert_eq!(
            readiness_gates(false, Language::English)[0].state,
            GateState::InputRequired
        );
        assert_eq!(
            readiness_gates(true, Language::English)[0].state,
            GateState::Verified
        );
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
        assert_eq!(report.case_id, case.data.case_id);
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
            .data
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
            "case_id": case.data.case_id,
            "frame_of_reference_uid": null,
            "geometry": serde_json::to_value(&case.data.ct.geometry).unwrap(),
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
        case.dose.load(&case.data);
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
        case.dose.load(&case.data);
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
        case.dose.load(&case.data);
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
                    None,
                    Language::English,
                    &mut tour_targets,
                    Theme::resolve(false),
                );
            });
            output.textures_delta.clear();
        }
    }
}
