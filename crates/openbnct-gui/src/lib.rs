// SPDX-License-Identifier: MIT

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
    Avify,
}

impl WorkspaceTab {
    const ALL: [Self; 7] = [
        Self::Overview,
        Self::Geometry,
        Self::Transport,
        Self::Plan,
        Self::Dose,
        Self::Evidence,
        Self::Avify,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Geometry => "Geometry",
            Self::Transport => "Transport",
            Self::Plan => "Plan",
            Self::Dose => "Dose components",
            Self::Evidence => "Evidence",
            Self::Avify => "Avify (Experimental)",
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
            Self::Avify => "Avify（実験的）",
        }
    }

    fn localized_label(self, language: Language) -> &'static str {
        match language {
            Language::Japanese => self.label_ja(),
            Language::Italian => match self {
                Self::Overview => "Panoramica",
                Self::Geometry => "Geometria",
                Self::Transport => "Trasporto",
                Self::Plan => "Piano",
                Self::Dose => "Componenti dose",
                Self::Evidence => "Evidenza",
                Self::Avify => "Avify (Sperimentale)",
            },
            Language::ChineseSimplified => match self {
                Self::Overview => "概览",
                Self::Geometry => "几何",
                Self::Transport => "输运",
                Self::Plan => "计划",
                Self::Dose => "剂量成分",
                Self::Evidence => "证据",
                Self::Avify => "Avify（实验性）",
            },
            Language::Spanish => match self {
                Self::Overview => "Resumen",
                Self::Geometry => "Geometría",
                Self::Transport => "Transporte",
                Self::Plan => "Plan",
                Self::Dose => "Componentes de dosis",
                Self::Evidence => "Evidencia",
                Self::Avify => "Avify (Experimental)",
            },
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
            Self::Avify => "07",
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
            WorkspaceTab::Avify => Self::Avify,
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
            HelpWorkspace::Avify => Self::Avify,
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
            Language::Italian => match self {
                Self::Verified => "VERIFICATO",
                Self::Frozen => "CONGELATO",
                Self::Blocked => "BLOCCATO",
                Self::Pending => "IN ATTESA",
                Self::InputRequired => "INPUT RICHIESTO",
            },
            Language::ChineseSimplified => match self {
                Self::Verified => "已验证",
                Self::Frozen => "已冻结",
                Self::Blocked => "已阻止",
                Self::Pending => "待定",
                Self::InputRequired => "需要输入",
            },
            Language::Spanish => match self {
                Self::Verified => "VERIFICADO",
                Self::Frozen => "CONGELADO",
                Self::Blocked => "BLOQUEADO",
                Self::Pending => "PENDIENTE",
                Self::InputRequired => "REQUIERE ENTRADA",
            },
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
            title: t!(language, en = "DICOM geometry", ja = "DICOM ジオメトリ"),
            detail: if case_loaded {
                t!(
                    language,
                    en = "Case artifacts and patient-space geometry passed the runtime gate.",
                    ja = "症例アーティファクトと患者空間ジオメトリが実行時ゲートを通過。"
                )
            } else {
                t!(
                    language,
                    en = "Load NF-BNCT-001 to run the DICOM and integrity gate.",
                    ja = "NF-BNCT-001 を読み込むと DICOM・完全性ゲートが実行されます。"
                )
            },
            state: if case_loaded {
                GateState::Verified
            } else {
                GateState::InputRequired
            },
        },
        ReadinessGate {
            title: t!(language, en = "Material and source", ja = "材料と線源"),
            detail: t!(
                language,
                en = "Versioned NF-BNCT-001 benchmark contracts are checked in.",
                ja = "バージョン管理された NF-BNCT-001 ベンチマーク契約はコミット済み。"
            ),
            state: GateState::Frozen,
        },
        ReadinessGate {
            title: t!(language, en = "OpenMC nuclear data", ja = "OpenMC 核データ"),
            detail: t!(
                language,
                en = "Official case selection and 16 artifact identities are frozen.",
                ja = "公式ケース選択と16件のアーティファクト同一性が凍結済み。"
            ),
            state: GateState::Frozen,
        },
        ReadinessGate {
            title: t!(language, en = "Component responses", ja = "成分応答"),
            detail: t!(
                language,
                en = "O-17/O-18 transported-photon treatment requires independent review.",
                ja = "O-17/O-18 輸送済み光子の取り扱いは独立レビューが必要。"
            ),
            state: GateState::Blocked,
        },
        ReadinessGate {
            title: t!(
                language,
                en = "Controlled transport run",
                ja = "制御付き輸送実行"
            ),
            detail: t!(
                language,
                en = "Disabled until every upstream scientific gate passes.",
                ja = "上流の科学的ゲートがすべて通過するまで無効。"
            ),
            state: GateState::Pending,
        },
    ]
}

/// A loaded dose artifact — physical or biological — with the same contract
/// validation the CLI enforces. Weighted bundles stay visually distinct.
enum DoseArtifact {
    Physical(Box<PhysicalDoseBundle>),
    Biological(Box<BiologicalDoseBundle>),
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
        let normalized = openbnct_core::normalize_contract_id(
            schema
                .get("schema_version")
                .and_then(|value| value.as_str())
                .unwrap_or_default(),
        );
        let artifact = match normalized.as_str() {
            openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA => {
                let bundle: PhysicalDoseBundle =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                bundle.validate().map_err(|error| error.to_string())?;
                Self::Physical(Box::new(bundle))
            }
            openbnct_bio::BIOLOGICAL_DOSE_BUNDLE_SCHEMA => {
                let bundle: BiologicalDoseBundle =
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                bundle.validate().map_err(|error| error.to_string())?;
                Self::Biological(Box::new(bundle))
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
    /// `openbnct.smk-evaluation` — stochastic-vs-MK survival table.
    smk: Option<openbnct_bio::SmkEvaluation>,
    smk_error: Option<String>,
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
            smk: None,
            smk_error: None,
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

/// The bundled example dose bundle — the layered-head-phantom 28-group
/// physical bundle, zip-deflated so a first-time visitor (web or
/// desktop) can see a populated workspace without having an artifact.
const EXAMPLE_DOSE_ZIP: &[u8] = include_bytes!("../assets/example-dose-bundle.zip");
/// The FiR 1 K63 literature beam-description — the bundled example for
/// the transport spectrum viewer.
const EXAMPLE_BEAM_JSON: &str = include_str!("../assets/fir1-k63.json");

/// Unzip the embedded example dose bundle through the same byte path a
/// dropped file takes.
fn example_dose_bytes() -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(EXAMPLE_DOSE_ZIP))
        .map_err(|error| error.to_string())?;
    let mut entry = archive.by_index(0).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|error| error.to_string())?;
    Ok(bytes)
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

    /// Bytes-only SMK-evaluation loader — web drop path.
    fn load_smk_bytes(&mut self, bytes: &[u8]) {
        let outcome =
            serde_json::from_slice::<openbnct_bio::SmkEvaluation>(bytes)
                .map_err(|e| e.to_string())
                .and_then(|report| {
                    if openbnct_core::schema_matches(
                        &report.schema_version,
                        openbnct_bio::SMK_EVALUATION_SCHEMA,
                    ) {
                        Ok(report)
                    } else {
                        Err(format!(
                            "unsupported schema {:?} — expected {}",
                            report.schema_version,
                            openbnct_bio::SMK_EVALUATION_SCHEMA
                        ))
                    }
                });
        match outcome {
            Ok(report) => {
                self.smk_error = None;
                self.smk = Some(report);
            }
            Err(error) => {
                self.smk = None;
                self.smk_error = Some(error);
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
    /// `openbnct.scenario-report` — per-scenario objective outcomes and
    /// cross-scenario bands from a discrete-scenario evaluation.
    scenario_report: Option<openbnct_plan::scenarios::PlanScenarioReport>,
    scenario_report_error: Option<String>,
    /// `openbnct.pk-schedule` — deliverable-dose landscape over the
    /// beam-on window grid with the optimal window flagged.
    pk_schedule: Option<openbnct_evidence::PkScheduleReport>,
    pk_schedule_error: Option<String>,
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

    /// Bytes-only scenario-report loader — web drop path.
    fn load_scenario_report_bytes(&mut self, bytes: &[u8]) {
        match serde_json::from_slice::<openbnct_plan::scenarios::PlanScenarioReport>(bytes)
            .map_err(|e| e.to_string())
            .and_then(|report| {
                if openbnct_core::schema_matches(
                    &report.schema_version,
                    openbnct_plan::scenarios::PLAN_SCENARIO_REPORT_SCHEMA,
                ) {
                    Ok(report)
                } else {
                    Err(format!(
                        "unsupported schema {:?} — expected {}",
                        report.schema_version,
                        openbnct_plan::scenarios::PLAN_SCENARIO_REPORT_SCHEMA
                    ))
                }
            }) {
            Ok(report) => {
                self.scenario_report_error = None;
                self.scenario_report = Some(report);
            }
            Err(error) => {
                self.scenario_report = None;
                self.scenario_report_error = Some(error);
            }
        }
    }

    /// Bytes-only pk-schedule loader — web drop path.
    fn load_pk_schedule_bytes(&mut self, bytes: &[u8]) {
        match serde_json::from_slice::<openbnct_evidence::PkScheduleReport>(bytes)
            .map_err(|e| e.to_string())
            .and_then(|report| {
                if openbnct_core::schema_matches(
                    &report.schema_version,
                    openbnct_evidence::PK_SCHEDULE_SCHEMA,
                ) {
                    Ok(report)
                } else {
                    Err(format!(
                        "unsupported schema {:?} — expected {}",
                        report.schema_version,
                        openbnct_evidence::PK_SCHEDULE_SCHEMA
                    ))
                }
            }) {
            Ok(report) => {
                self.pk_schedule_error = None;
                self.pk_schedule = Some(report);
            }
            Err(error) => {
                self.pk_schedule = None;
                self.pk_schedule_error = Some(error);
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

/// UI state for the Avify workspace: input paths, the engine job, and the
/// returned certificate + run receipt. Execution shells out to `openbnct
/// avify` — the same code path as the CLI — so this panel never reimplements
/// connector logic. Engine runs are native-only.
struct AvifyPanel {
    case_path: String,
    assignment_path: String,
    spec_path: String,
    outdir: String,
    engine_cmd: String,
    threads: String,
    timeout_s: String,
    #[cfg(not(target_arch = "wasm32"))]
    job: Option<run::Job>,
    output: Vec<String>,
    status: Option<String>,
    /// Parsed certificate after a successful verify.
    #[cfg(not(target_arch = "wasm32"))]
    certificate: Option<openbnct_avify::AvifyCertificate>,
    /// `name → state` from the last staleness check of the run receipt.
    #[cfg(not(target_arch = "wasm32"))]
    staleness: Option<std::collections::BTreeMap<String, openbnct_avify::InputState>>,
    /// The run receipt itself (engine version, timing, warnings).
    #[cfg(not(target_arch = "wasm32"))]
    receipt: Option<openbnct_avify::AvifyRunReceipt>,
    /// `review.json` state for the loaded outdir.
    #[cfg(not(target_arch = "wasm32"))]
    review_state: Option<openbnct_avify::ReviewState>,
    /// Tint ROI regions by their certificate verdict on the class map.
    #[cfg(not(target_arch = "wasm32"))]
    verdict_overlay: bool,
    /// Reviewer-name form state for "Mark reviewed".
    #[cfg(not(target_arch = "wasm32"))]
    review_form: Option<(String, String)>,
    #[cfg(not(target_arch = "wasm32"))]
    receipt_path: String,
    /// The exported voxel arrays (class map + ROI masks) for the
    /// spatial view — loaded from whichever `*_arrays.npz` the outdir
    /// holds.
    #[cfg(not(target_arch = "wasm32"))]
    arrays: Option<openbnct_avify::ArraysNpz>,
    /// Shared tri-planar crosshair, voxel index `[x, y, z]`.
    #[cfg(not(target_arch = "wasm32"))]
    cursor: [usize; 3],
    /// Path of a prior run to compare the current one against
    /// (`avify diff` semantics — pure file IO, no engine).
    compare_path: String,
    /// Active onboarding slide (`None` = tutorial closed).
    tutorial_slide: Option<usize>,
    /// Whether the first-visit tutorial has already been shown
    /// (persisted to a marker file on native).
    tutorial_seen: bool,
}

impl Default for AvifyPanel {
    fn default() -> Self {
        Self {
            case_path: String::new(),
            assignment_path: String::new(),
            spec_path: String::new(),
            outdir: "avify-run".into(),
            engine_cmd: "avify-dose".into(),
            threads: "4".into(),
            timeout_s: "21600".into(),
            #[cfg(not(target_arch = "wasm32"))]
            job: None,
            output: Vec::new(),
            status: None,
            #[cfg(not(target_arch = "wasm32"))]
            certificate: None,
            #[cfg(not(target_arch = "wasm32"))]
            staleness: None,
            #[cfg(not(target_arch = "wasm32"))]
            receipt: None,
            #[cfg(not(target_arch = "wasm32"))]
            review_state: None,
            #[cfg(not(target_arch = "wasm32"))]
            verdict_overlay: true,
            #[cfg(not(target_arch = "wasm32"))]
            review_form: None,
            #[cfg(not(target_arch = "wasm32"))]
            receipt_path: String::new(),
            #[cfg(not(target_arch = "wasm32"))]
            arrays: None,
            #[cfg(not(target_arch = "wasm32"))]
            cursor: [0; 3],
            compare_path: String::new(),
            tutorial_slide: None,
            tutorial_seen: false,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AvifyPanel {
    fn start(&mut self, verify: bool) {
        self.output.clear();
        self.status = None;
        self.certificate = None;
        self.staleness = None;
        let mut args = vec![
            "avify".to_string(),
            if verify { "verify" } else { "export-plan" }.to_string(),
            "--case".into(),
            self.case_path.trim().to_string(),
            "--assignment".into(),
            self.assignment_path.trim().to_string(),
            "--spec".into(),
            self.spec_path.trim().to_string(),
        ];
        if verify {
            args.extend([
                "--outdir".into(),
                self.outdir.trim().to_string(),
                "--engine-cmd".into(),
                self.engine_cmd.trim().to_string(),
                "--timeout-s".into(),
                self.timeout_s.trim().to_string(),
            ]);
            let threads = self.threads.trim();
            if !threads.is_empty() {
                args.extend(["--threads".into(), threads.to_string()]);
            }
        } else {
            args.extend(["--prefix".into(), format!("{}/plan", self.outdir.trim())]);
        }
        // The job's own deadline stays a little looser than the
        // connector's --timeout-s so the connector reports timeouts first.
        let timeout = self
            .timeout_s
            .trim()
            .parse::<u64>()
            .map(|s| std::time::Duration::from_secs(s.max(1) + 60))
            .unwrap_or(run::DEFAULT_TIMEOUT);
        // The CLI writes into `outdir` without creating it.
        if let Err(e) = std::fs::create_dir_all(self.outdir.trim()) {
            self.status = Some(format!("outdir: {e}"));
            return;
        }
        match run::Job::spawn("openbnct", &args, timeout) {
            Ok(job) => self.job = Some(job),
            Err(error) => self.status = Some(error),
        }
    }

    /// Drain the job; on a finished verify, load the certificate and run
    /// receipt from the outdir and check input staleness.
    fn poll(&mut self) -> bool {
        let Some(job) = &mut self.job else {
            return false;
        };
        let running = job.poll(&mut self.output, 300);
        if running {
            return true;
        }
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
        if job.exit_code == Some(0) {
            let outdir = PathBuf::from(self.outdir.trim());
            let cert_path = outdir.join("certificate.json");
            if cert_path.is_file() {
                match openbnct_avify::AvifyCertificate::load(&cert_path) {
                    Ok(cert) => self.certificate = Some(cert),
                    Err(e) => self.status = Some(format!("certificate parse: {e}")),
                }
            }
            let receipt = outdir.join("avify-run.json");
            self.receipt_path = receipt.display().to_string();
            if let Ok(receipt) = openbnct_avify::AvifyRunReceipt::load(&receipt) {
                self.staleness = Some(openbnct_avify::check_staleness(&receipt, &outdir));
                self.receipt = Some(receipt);
            }
            self.review_state = openbnct_avify::review_state(&outdir).ok();
            self.load_arrays(&outdir);
        }
        false
    }

    /// Read whichever `*_arrays.npz` the outdir holds (verify writes
    /// `openbnct-case_*`; export-plan honours `--prefix`).
    fn load_arrays(&mut self, outdir: &std::path::Path) {
        for candidate in ["openbnct-case_arrays.npz", "plan_arrays.npz"] {
            let path = outdir.join(candidate);
            if path.is_file() {
                match openbnct_avify::read_arrays_npz(&path) {
                    Ok(arrays) => {
                        self.cursor = [
                            arrays.shape_zyx[2] / 2,
                            arrays.shape_zyx[1] / 2,
                            arrays.shape_zyx[0] / 2,
                        ];
                        self.arrays = Some(arrays);
                    }
                    Err(e) => self.status = Some(format!("arrays npz: {e}")),
                }
                return;
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(job) = &mut self.job {
            job.cancel();
        }
        self.job = None;
        self.status = Some("cancelled".into());
    }

    /// Diff a prior run against the loaded outdir — in-process file
    /// IO, same `diff_runs` the CLI uses.
    #[cfg(not(target_arch = "wasm32"))]
    fn compare_to(&mut self, other: &str) {
        let base = PathBuf::from(self.outdir.trim());
        match openbnct_avify::diff_runs(std::path::Path::new(other.trim()), &base) {
            Ok(d) => {
                self.output.push(format!(
                    "diff {} -> {} — engine {} -> {}, {:.1}s -> {:.1}s",
                    other.trim(),
                    self.outdir.trim(),
                    d.engine_before,
                    d.engine_after,
                    d.elapsed_before_s,
                    d.elapsed_after_s
                ));
                for c in &d.input_changes {
                    self.output.push(format!(
                        "  input {:12} {} -> {}",
                        c.name, c.sha256_before, c.sha256_after
                    ));
                }
                for (roi, c) in &d.roi_changes {
                    let mark = if c.action_changed { "*" } else { " " };
                    self.output.push(format!(
                        "  {mark}{roi:8} [{:.2}, {:.2}] -> [{:.2}, {:.2}] Gy-w  {} -> {}",
                        c.certified_before[0],
                        c.certified_before[1],
                        c.certified_after[0],
                        c.certified_after[1],
                        c.action_before,
                        c.action_after
                    ));
                }
                for roi in &d.only_before {
                    self.output.push(format!("  -{roi} only in earlier run"));
                }
                for roi in &d.only_after {
                    self.output.push(format!("  +{roi} only in later run"));
                }
                self.status = Some("diff complete".into());
            }
            Err(e) => self.status = Some(format!("diff: {e}")),
        }
    }

    /// Load a previously written result from `outdir` (certificate +
    /// run receipt) — reopens an earlier analysis without rerunning.
    fn load_result(&mut self) {
        let outdir = PathBuf::from(self.outdir.trim());
        let cert_path = outdir.join("certificate.json");
        match openbnct_avify::AvifyCertificate::load(&cert_path) {
            Ok(cert) => {
                self.certificate = Some(cert);
                let receipt = outdir.join("avify-run.json");
                self.receipt_path = receipt.display().to_string();
                self.staleness = openbnct_avify::AvifyRunReceipt::load(&receipt)
                    .map(|r| openbnct_avify::check_staleness(&r, &outdir))
                    .ok();
                self.receipt = openbnct_avify::AvifyRunReceipt::load(&receipt).ok();
                self.review_state = openbnct_avify::review_state(&outdir).ok();
                self.load_arrays(&outdir);
                self.status = Some("loaded".into());
                if let Ok(r) = openbnct_avify::AvifyRunReceipt::load(&receipt) {
                    self.status = Some(format!(
                        "loaded — engine {} · {:.0}s total (engine {:.0}s)",
                        r.engine.version, r.timing.total_s, r.timing.engine_s
                    ));
                }
            }
            Err(e) => self.status = Some(format!("load certificate: {e}")),
        }
    }

    /// Recompute the staleness check — call when the user asks, not per
    /// frame (each pass re-hashes the bound inputs).
    fn recheck_staleness(&mut self) {
        let receipt = PathBuf::from(self.receipt_path.trim());
        if let Ok(receipt) = openbnct_avify::AvifyRunReceipt::load(&receipt) {
            let base = receipt
                .certificate
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            self.staleness = Some(openbnct_avify::check_staleness(&receipt, &base));
            self.review_state = openbnct_avify::review_state(&base).ok();
            self.receipt = Some(receipt);
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
    avify: AvifyPanel,
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
    /// A `openbnct.scenario-report` — bands table in the plan workspace.
    ScenarioReport,
    /// A `openbnct.pk-schedule` — window landscape in the plan workspace.
    PkSchedule,
    /// A `openbnct.smk-evaluation` — survival table in the dose
    /// workspace.
    Smk,
    /// Any other recognized `openbnct.*` / `nctforge.*` schema the
    /// workbench has no dedicated panel for — the generic artifact
    /// inspector shows it instead of a dose-bundle validation error.
    Inspector,
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
    let clicked = ui
        .button(t!(
            language,
            en = "Browse…",
            ja = "参照…",
            it = "Sfoglia…",
            zh = "浏览…",
            es = "Examinar…"
        ))
        .clicked();
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
    let normalized = openbnct_core::normalize_contract_id(
        schema
            .get("schema_version")
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
    );
    match normalized.as_str() {
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
        s if s.contains("scenario-report/") => DropTarget::ScenarioReport,
        s if s.contains("pk-schedule/") => DropTarget::PkSchedule,
        s if s.contains("smk-evaluation/") => DropTarget::Smk,
        s if s.contains("physical-dose-bundle/") => DropTarget::DoseBundle,
        s if s.starts_with("openbnct.") || s.starts_with("nctforge.") => DropTarget::Inspector,
        _ => DropTarget::DoseBundle,
    }
}

/// A recognized `openbnct.*` / `nctforge.*` artifact with no dedicated
/// panel — rendered by the generic inspector window.
struct InspectorArtifact {
    name: String,
    schema: String,
    value: serde_json::Value,
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
    /// Active slide of the first-launch app tour (`None` = closed).
    /// Dormant by owner decision: content and renderer stay compiled —
    /// set to `Some(0)` to re-enable (see `APP_TOUR_SLIDES`).
    app_tour_slide: Option<usize>,
    /// Most recent recognized-but-unpaneled artifact — the floating
    /// inspector window shows it.
    inspector: Option<InspectorArtifact>,
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
            app_tour_slide: None,
            inspector: None,
        };
        app.panels.avify.tutorial_seen = tour_seen_from_disk("avify-tutorial");
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
            ui.menu_button(t!(lang, en = "File", ja = "ファイル",
                it = "File",
                zh = "文件",
                es = "Archivo"), |ui| {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if ui
                        .button(t!(lang, en = "Open case…", ja = "症例を開く…",
                it = "Apri caso…",
                zh = "打开病例…",
                es = "Abrir caso…"))
                        .clicked()
                    {
                        if let Some(dir) = io::pick_folder() {
                            self.case_path = dir.display().to_string();
                            self.load_case();
                        }
                        ui.close();
                    }
                    ui.small(t!(lang, en = "the frozen NF-BNCT-001 case directory — or drop it anywhere.", ja = "凍結済み NF-BNCT-001 症例フォルダ — どこかにドロップしても可。"));
                    if ui
                        .button(t!(lang, en = "Import DICOM study…", ja = "DICOM スタディを取り込む…",
                it = "Importa studio DICOM…",
                zh = "导入 DICOM 检查…",
                es = "Importar estudio DICOM…"))
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
                                        t!(lang, en = "study import rejected", ja = "スタディ取り込み拒否")
                                    ));
                                }
                            }
                        }
                        ui.close();
                    }
                    ui.small(t!(lang, en = "any CT + RTSTRUCT export — bucketed by SOP class, hash-bound.", ja = "任意の CT + RTSTRUCT エクスポート — SOP クラスで仕分け、ハッシュで紐付け。"));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    if ui
                        .button(t!(lang, en = "Open case or study files…", ja = "症例/スタディを開く…"))
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
                        en = "…or drop them: a case .zip, the 42 NF-BNCT-001 members, \
                              or a DICOM study's files.",
                        ja = "…またはドロップ: 症例 .zip、NF-BNCT-001 の全42ファイル、または DICOM スタディのファイル群。",
                        it = "…oppure trascinali: un .zip del caso, i 42 membri di NF-BNCT-001, o i file di uno studio DICOM.",
                        zh = "…或拖放：病例 .zip、NF-BNCT-001 的全部 42 个文件，或一份 DICOM 检查的文件。",
                        es = "…o suéltelos: un .zip del caso, los 42 miembros de NF-BNCT-001, o los archivos de un estudio DICOM."
                    ));
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if ui
                        .button(t!(lang, en = "Export case template…", ja = "症例テンプレートを出力…",
                it = "Esporta modello caso…",
                zh = "导出病例模板…",
                es = "Exportar plantilla de caso…"))
                        .clicked()
                    {
                        if let Some(dir) = io::pick_folder() {
                            self.template_status = Some(match export_case_template(&dir) {
                                Ok(count) => format!(
                                    "{} {} ({})",
                                    t!(lang, en = "template written to", ja = "テンプレート出力先:"),
                                    dir.display(),
                                    t!(lang, en = "{count} files — edit before use", ja = "{count} ファイル — 使用前に編集してください")
                                    .replace("{count}", &count.to_string())
                                ),
                                Err(error) => format!(
                                    "{}: {error}",
                                    t!(lang, en = "template export failed", ja = "テンプレート出力失敗")
                                ),
                            });
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button(t!(lang, en = "Quit", ja = "終了",
                it = "Esci",
                zh = "退出",
                es = "Salir")).clicked() {
                        ui.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });
            ui.menu_button(t!(self.language, en = "View", ja = "表示",
                it = "Vista",
                zh = "视图",
                es = "Ver"), |ui| {
                let lang = self.language;
                ui.checkbox(&mut self.dark_mode, t!(lang, en = "Dark mode", ja = "ダークモード",
                it = "Modalità scura",
                zh = "深色模式",
                es = "Modo oscuro"));
                if ui
                    .checkbox(
                        &mut self.reduce_motion,
                        t!(lang, en = "Reduce motion", ja = "アニメーションを減らす",
                it = "Riduci animazioni",
                zh = "减少动画",
                es = "Reducir movimiento"),
                    )
                    .changed()
                {
                    self.apply_motion(ui.ctx());
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(t!(lang, en = "Language / 言語", ja = "言語 / Language")).small(),
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
                ui.label(egui::RichText::new(t!(lang, en = "Interface scale", ja = "表示倍率",
                it = "Scala interfaccia",
                zh = "界面缩放",
                es = "Escala de interfaz")).small());
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
                    .button(t!(lang, en = "Reset views", ja = "ビューをリセット",
                it = "Ripristina viste",
                zh = "重置视图",
                es = "Restablecer vistas"))
                    .clicked()
                {
                    self.reset_views();
                    ui.close();
                }
                if ui
                    .button(t!(lang, en = "Screenshot…", ja = "スクリーンショット…",
                it = "Screenshot…",
                zh = "截图…",
                es = "Captura…"))
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
                        .button(t!(lang, en = "Toggle fullscreen", ja = "全画面表示の切替",
                it = "Schermo intero",
                zh = "全屏切换",
                es = "Pantalla completa"))
                        .clicked()
                    {
                        ui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                            !ui.input(|i| i.viewport().fullscreen.unwrap_or(false)),
                        ));
                        ui.close();
                    }
                }
            });
            let help = ui.menu_button(t!(self.language, en = "Help", ja = "ヘルプ",
                it = "Aiuto",
                zh = "帮助",
                es = "Ayuda"), |ui| {
                if ui
                    .button(t!(self.language, en = "Help and guided tours", ja = "ヘルプとガイドツアー",
                it = "Guida e tour guidati",
                zh = "帮助与导览",
                es = "Ayuda y visitas guiadas"))
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
            DropTarget::Plan => {
                match bytes {
                    Ok(b) => self.panels.plan.load_bytes(&b),
                    Err(error) => self.panels.plan.error = Some(error),
                }
                self.workspace = WorkspaceTab::Plan;
            }
            DropTarget::NiftiVolume => {
                match bytes {
                    Ok(b) => self.panels.nifti.inspect_bytes(&b),
                    Err(error) => self.panels.nifti.error = Some(error),
                }
                self.workspace = WorkspaceTab::Dose;
            }
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
            DropTarget::ScenarioReport => {
                match bytes {
                    Ok(b) => self.panels.plan.load_scenario_report_bytes(&b),
                    Err(error) => self.panels.plan.scenario_report_error = Some(error),
                }
                self.workspace = WorkspaceTab::Plan;
            }
            DropTarget::PkSchedule => {
                match bytes {
                    Ok(b) => self.panels.plan.load_pk_schedule_bytes(&b),
                    Err(error) => self.panels.plan.pk_schedule_error = Some(error),
                }
                self.workspace = WorkspaceTab::Plan;
            }
            DropTarget::Smk => {
                match bytes {
                    Ok(b) => self.panels.dose.load_smk_bytes(&b),
                    Err(error) => self.panels.dose.smk_error = Some(error),
                }
                self.workspace = WorkspaceTab::Dose;
            }
            DropTarget::Inspector => {
                match bytes.and_then(|b| {
                    serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(value) => {
                        let schema = value
                            .get("schema_version")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_owned();
                        self.inspector = Some(InspectorArtifact {
                            name: name.to_owned(),
                            schema,
                            value,
                        });
                    }
                    Err(error) => {
                        self.load_error = Some(format!("could not parse {name}: {error}"));
                    }
                }
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
        if self.workspace == WorkspaceTab::Avify && !self.panels.avify.tutorial_seen {
            self.panels.avify.tutorial_seen = true;
            self.panels.avify.tutorial_slide = Some(0);
            write_tour_marker("avify-tutorial");
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
                            t!(
                                self.language,
                                en = "Load rejected",
                                ja = "読込拒否",
                                it = "Caricamento rifiutato",
                                zh = "加载被拒绝",
                                es = "Carga rechazada"
                            )
                        ),
                    );
                }
                if let Some(note) = self.case_drop_note.clone() {
                    ui.horizontal(|ui| {
                        ui.colored_label(theme.warn_text, &note);
                        if ui
                            .small_button(t!(
                                self.language,
                                en = "discard",
                                ja = "破棄",
                                it = "scarta",
                                zh = "放弃",
                                es = "descartar"
                            ))
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
                                en = "{count} DICOM file(s) collected for study import",
                                ja = "スタディ取り込み用に {count} 件の DICOM ファイルを収集済み"
                            )
                            .replace("{count}", &count.to_string()),
                        );
                        if ui
                            .small_button(t!(
                                self.language,
                                en = "Import as research case",
                                ja = "研究用症例として取り込む",
                                it = "Importa come caso di ricerca",
                                zh = "作为研究病例导入",
                                es = "Importar como caso de investigación"
                            ))
                            .clicked()
                        {
                            self.import_pending_study();
                        }
                        if ui
                            .small_button(t!(
                                self.language,
                                en = "discard",
                                ja = "破棄",
                                it = "scarta",
                                zh = "放弃",
                                es = "descartar"
                            ))
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
                        en = "Research use only · Not for clinical decision-making",
                        ja = "研究専用 · 臨床判断には使用不可",
                        it = "Solo per ricerca · Non per decisioni cliniche",
                        zh = "仅限研究用途 · 不用于临床决策",
                        es = "Solo para investigación · No para decisiones clínicas"
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
            &self.case_path,
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
        show_slide_tour(
            ui.ctx(),
            "Welcome to OpenBNCT",
            &mut self.app_tour_slide,
            &APP_TOUR_SLIDES,
            theme,
        );
        show_inspector_window(ui.ctx(), &mut self.inspector);
    }
}

/// Floating inspector for any recognized artifact schema without a
/// dedicated panel — header fields plus a collapsible JSON tree so
/// every CLI-emitted document stays viewable.
fn show_inspector_window(context: &egui::Context, inspector: &mut Option<InspectorArtifact>) {
    let Some(artifact) = inspector.as_ref() else {
        return;
    };
    let mut open = true;
    egui::Window::new(format!("artifact — {}", artifact.name))
        .open(&mut open)
        .default_size([560.0, 480.0])
        .show(context, |ui| {
            ui.monospace(&artifact.schema);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let serde_json::Value::Object(map) = &artifact.value {
                    for (key, value) in map {
                        show_json_tree(ui, key, value);
                    }
                }
            });
        });
    if !open {
        *inspector = None;
    }
}

/// Recursive collapsible JSON tree for the inspector: objects and
/// arrays collapse; long numeric arrays summarize to min/max/mean with
/// a peek at the first elements rather than a wall of rows.
fn show_json_tree(ui: &mut egui::Ui, key: &str, value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            egui::CollapsingHeader::new(format!("{key}  {{ {} fields }}", map.len()))
                .default_open(false)
                .show(ui, |ui| {
                    for (k, v) in map {
                        show_json_tree(ui, k, v);
                    }
                });
        }
        serde_json::Value::Array(items) => {
            let header = format!("{key}  [ {} items ]", items.len());
            if items.len() > 16 && items.iter().all(|v| v.is_number()) {
                egui::CollapsingHeader::new(header)
                    .default_open(false)
                    .show(ui, |ui| {
                        let nums: Vec<f64> = items.iter().filter_map(|v| v.as_f64()).collect();
                        let (mut lo, mut hi, mut sum) = (f64::MAX, f64::MIN, 0.0);
                        for n in &nums {
                            lo = lo.min(*n);
                            hi = hi.max(*n);
                            sum += *n;
                        }
                        if nums.is_empty() {
                            ui.label("(empty)");
                        } else {
                            ui.monospace(format!(
                                "min {lo:.6e} · max {hi:.6e} · mean {:.6e}",
                                sum / nums.len() as f64
                            ));
                        }
                        egui::CollapsingHeader::new("first 50 values")
                            .default_open(false)
                            .show(ui, |ui| {
                                for v in items.iter().take(50) {
                                    ui.monospace(v.to_string());
                                }
                            });
                    });
            } else {
                egui::CollapsingHeader::new(header)
                    .default_open(false)
                    .show(ui, |ui| {
                        for (i, v) in items.iter().enumerate().take(200) {
                            show_json_tree(ui, &i.to_string(), v);
                        }
                        if items.len() > 200 {
                            ui.label(format!("… {} more", items.len() - 200));
                        }
                    });
            }
        }
        scalar => {
            ui.horizontal(|ui| {
                ui.label(format!("{key}:"));
                ui.monospace(scalar.to_string());
            });
        }
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
            egui::RichText::new(t!(
                language,
                en = "Research workbench",
                ja = "研究ワークベンチ",
                it = "Workbench di ricerca",
                zh = "研究工作台",
                es = "Entorno de investigación"
            ))
            .color(theme.text_dim),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(case.map_or(
                    t!(
                        language,
                        en = "No case open",
                        ja = "症例なし",
                        it = "Nessun caso aperto",
                        zh = "未打开病例",
                        es = "Ningún caso abierto"
                    ),
                    |c| c.data.case_id.as_str(),
                ))
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
            ui.label(t!(
                language,
                en = "Case folder",
                ja = "症例フォルダ",
                it = "Cartella del caso",
                zh = "病例文件夹",
                es = "Carpeta del caso"
            ));
            let path_response = ui.add(
                egui::TextEdit::singleline(case_path)
                    .desired_width((ui.available_width() - 220.0).max(180.0))
                    .hint_text(t!(
                        language,
                        en = "Open a verified case folder, or drop it here",
                        ja = "検証済み症例フォルダを開く、またはここにドロップ"
                    )),
            );
            if ui
                .button(t!(
                    language,
                    en = "Load & verify",
                    ja = "読込・検証",
                    it = "Carica e verifica",
                    zh = "加载并验证",
                    es = "Cargar y verificar"
                ))
                .clicked()
                || (path_response.lost_focus() && enter_pressed)
            {
                request = LoaderRequest::LoadPath;
            }
            if ui
                .button(t!(
                    language,
                    en = "Browse…",
                    ja = "参照…",
                    it = "Sfoglia…",
                    zh = "浏览…",
                    es = "Examinar…"
                ))
                .clicked()
                && let Some(dir) = io::pick_folder()
            {
                *case_path = dir.display().to_string();
                request = LoaderRequest::LoadPath;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (case_path, enter_pressed);
            ui.label(t!(
                language,
                en = "Case",
                ja = "症例",
                it = "Caso",
                zh = "病例",
                es = "Caso"
            ));
            if ui
                .button(t!(
                    language,
                    en = "Pick files…",
                    ja = "ファイルを選択…",
                    it = "Scegli file…",
                    zh = "选择文件…",
                    es = "Elegir archivos…"
                ))
                .clicked()
            {
                request = LoaderRequest::PickFiles;
            }
            ui.small(t!(
                language,
                en = "a case .zip, the 42 NF-BNCT-001 members, or a DICOM study — or drop them",
                ja =
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
    loaded_case_path: &str,
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
                egui::RichText::new(t!(
                    language,
                    en = "WORKBENCH",
                    ja = "ワークベンチ",
                    it = "WORKBENCH",
                    zh = "工作台",
                    es = "WORKBENCH"
                ))
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
                egui::RichText::new(t!(
                    language,
                    en = "ACTIVE CASE",
                    ja = "開いている症例",
                    it = "CASO ATTIVO",
                    zh = "当前病例",
                    es = "CASO ACTIVO"
                ))
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
                    egui::RichText::new(t!(
                        language,
                        en = "No case loaded",
                        ja = "症例なし",
                        it = "Nessun caso caricato",
                        zh = "未加载病例",
                        es = "Ningún caso cargado"
                    ))
                    .color(theme.text_dim),
                );
                ui.small(t!(
                    language,
                    en = "Open a case above to inspect its geometry.",
                    ja = "上で症例を開くとジオメトリを確認できます。",
                    it = "Apri un caso sopra per ispezionarne la geometria.",
                    zh = "在上方打开病例以检查其几何。",
                    es = "Abra un caso arriba para inspeccionar su geometría."
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
                            &mut panels.dose,
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
                                t!(language, en = "Geometry", ja = "ジオメトリ",
                it = "Geometria",
                zh = "几何",
                es = "Geometría"),
                                t!(language, en = "Patient-space DICOM truth before transport.", ja = "輸送計算の前段となる患者空間の DICOM 実データ。",
                it = "La verità DICOM nello spazio paziente prima del trasporto.",
                zh = "输运前的患者空间 DICOM 真实数据。",
                es = "La verdad DICOM en espacio paciente antes del transporte."),
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
                    WorkspaceTab::Avify => show_avify_workspace(
                        ui,
                        &mut panels.avify,
                        loaded_case_path,
                        language,
                        theme,
                    ),
                });
        });
    show_avify_tutorial(ui.ctx(), &mut panels.avify, theme);
}

/// Engine-class palette — five fixed tissue classes plus a fallback.
/// Chosen to stay separable under common colour-vision deficiencies;
/// the legend also prints each name and index so nothing depends on
/// colour alone.
#[cfg(not(target_arch = "wasm32"))]
fn avify_class_color(class: i8) -> egui::Color32 {
    match class {
        0 => egui::Color32::from_rgb(30, 34, 44),    // air
        1 => egui::Color32::from_rgb(196, 107, 176), // brain
        2 => egui::Color32::from_rgb(214, 214, 214), // cranium
        3 => egui::Color32::from_rgb(224, 164, 88),  // scalp
        4 => egui::Color32::from_rgb(255, 82, 82),   // tumour
        _ => egui::Color32::from_rgb(90, 200, 250),  // unknown
    }
}

/// One orthogonal slice of the engine class map at the shared cursor.
/// `plane` selects the fixed axis: 0 = axial (x-y at z), 1 = coronal
/// (x-z at y), 2 = sagittal (y-z at x). ROI mask members paint as a
/// bright contour — mask voxels whose in-plane 4-neighbour leaves the
/// mask. Clicking moves the shared cursor; returns the new voxel.
#[cfg(not(target_arch = "wasm32"))]
fn avify_class_view(
    ui: &mut egui::Ui,
    arrays: &openbnct_avify::ArraysNpz,
    plane: usize,
    cursor: [usize; 3],
    verdicts: Option<&std::collections::BTreeMap<String, String>>,
    theme: Theme,
) -> Option<[usize; 3]> {
    let [nz, ny, nx] = arrays.shape_zyx;
    let cls = |x: usize, y: usize, z: usize| arrays.cls_zyx[z * ny * nx + y * nx + x];
    // (width, height, pixel→voxel, fixed-index, plane name)
    type ToVoxel = Box<dyn Fn(usize, usize) -> [usize; 3]>;
    let (w, h, to_voxel, fixed, name): (usize, usize, ToVoxel, usize, &str) = match plane {
        0 => (
            nx,
            ny,
            Box::new(move |u, v| [u, v, cursor[2]]) as ToVoxel,
            cursor[2],
            "axial",
        ),
        1 => (
            nx,
            nz,
            Box::new(move |u, v| [u, cursor[1], v]) as ToVoxel,
            cursor[1],
            "coronal",
        ),
        _ => (
            ny,
            nz,
            Box::new(move |u, v| [cursor[0], u, v]) as ToVoxel,
            cursor[0],
            "sagittal",
        ),
    };
    let mut pixels = vec![egui::Color32::BLACK; w * h];
    for v in 0..h {
        for u in 0..w {
            let [x, y, z] = to_voxel(u, v);
            let mut color = avify_class_color(cls(x, y, z));
            // ROI contour: any in-plane 4-neighbour leaving the mask.
            for (i, (roi_name, mask)) in arrays.rois.iter().enumerate() {
                let idx = z * ny * nx + y * nx + x;
                if !mask[idx] {
                    continue;
                }
                // Verdict tint: the ROI's certified interval outcome
                // colours its interior (contour stays bright).
                if let Some(verdicts) = verdicts
                    && let Some(action) = verdicts.get(roi_name)
                {
                    let tint = match action.as_str() {
                        "PASS" => egui::Color32::from_rgba_unmultiplied(60, 160, 80, 96),
                        "FAIL" => egui::Color32::from_rgba_unmultiplied(200, 60, 60, 96),
                        "ADDITIONAL_EVIDENCE" => {
                            egui::Color32::from_rgba_unmultiplied(220, 170, 60, 96)
                        }
                        _ => egui::Color32::from_rgba_unmultiplied(140, 140, 140, 64),
                    };
                    color = color.blend(tint);
                }
                let leaves = |u: isize, v: isize| -> bool {
                    if u < 0 || v < 0 || u >= w as isize || v >= h as isize {
                        return true;
                    }
                    let [x2, y2, z2] = to_voxel(u as usize, v as usize);
                    !mask[z2 * ny * nx + y2 * nx + x2]
                };
                if leaves(u as isize - 1, v as isize)
                    || leaves(u as isize + 1, v as isize)
                    || leaves(u as isize, v as isize - 1)
                    || leaves(u as isize, v as isize + 1)
                {
                    let tint = [
                        egui::Color32::WHITE,
                        egui::Color32::GOLD,
                        egui::Color32::GREEN,
                    ][i % 3];
                    color = tint;
                }
            }
            pixels[v * w + u] = color;
        }
    }
    let image = egui::ColorImage::new([w, h], pixels);
    let texture = ui.ctx().load_texture(
        format!("avify-cls-{plane}"),
        image,
        egui::TextureOptions::NEAREST,
    );
    let size = egui::vec2(150.0, 150.0 * h as f32 / w as f32);
    let (rect, response) = ui
        .vertical(|ui| {
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
            ui.small(format!("{name} {fixed}"));
            (rect, response)
        })
        .inner;
    egui::Image::from_texture(&texture).paint_at(ui, rect);
    // Crosshair lines through the shared cursor's in-plane position.
    let painter = ui.painter_at(rect);
    let (cu, cv) = match plane {
        0 => (cursor[0], cursor[1]),
        1 => (cursor[0], cursor[2]),
        _ => (cursor[1], cursor[2]),
    };
    let fx = rect.left() + rect.width() * (cu as f32 + 0.5) / w as f32;
    let fy = rect.top() + rect.height() * (cv as f32 + 0.5) / h as f32;
    let stroke = egui::Stroke::new(1.0, theme.text_dim);
    painter.line_segment(
        [egui::pos2(fx, rect.top()), egui::pos2(fx, rect.bottom())],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(rect.left(), fy), egui::pos2(rect.right(), fy)],
        stroke,
    );
    if (response.clicked() || response.dragged())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let u =
            ((pos.x - rect.left()) / rect.width() * w as f32).clamp(0.0, w as f32 - 1.0) as usize;
        let v =
            ((pos.y - rect.top()) / rect.height() * h as f32).clamp(0.0, h as f32 - 1.0) as usize;
        return Some(to_voxel(u, v));
    }
    None
}

/// The Avify Dose workspace — connector for the separately licensed
/// engine. The panel explains the research question and assumptions
/// before execution, runs `openbnct avify` as a bounded job, and renders
/// the returned certificate with per-input staleness from the run
/// receipt. All controls stay enabled for reading; execution is
/// native-only.
fn show_avify_workspace(
    ui: &mut egui::Ui,
    panel: &mut AvifyPanel,
    loaded_case_path: &str,
    language: Language,
    theme: Theme,
) {
    #[cfg(target_arch = "wasm32")]
    let _ = loaded_case_path;
    show_workspace_heading(
        ui,
        theme,
        "Avify Dose",
        t!(
            language,
            en = "Independent two-evaluation envelope from the separately licensed engine.",
            ja = "別ライセンスのエンジンによる独立した2評価エンベロープ。",
            it = "Inviluppo indipendente a due valutazioni dal motore con licenza separata.",
            zh = "由单独许可的引擎给出的独立双评估包络。",
            es = "Envolvente independiente de dos evaluaciones del motor con licencia separada."
        ),
    );

    egui::Frame::new()
        .fill(theme.card_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.label(t!(
                language,
                en = "What this asks: for the uptake-uncertainty set declared in the spec, does the plan's dose to each ROI bracket inside its criterion? The engine evaluates the corner maps itself and returns an empirical envelope — research software, not a certified clinical bound.",
                ja = "問い: 仕様で宣言された摂取量不確実性集合について、各 ROI への線量は判定基準内に収まるか。エンジンはコーナーマップを自前で評価し、経験的エンベロープを返します — 研究ソフトウェアであり、認定済みの臨床境界ではありません。",
                it = "Domanda: per l'insieme di incertezza di uptake dichiarato nella spec, la dose del piano a ciascuna ROI rientra nel suo criterio? Il motore valuta le mappe d'angolo da sé e restituisce un inviluppo empirico — software di ricerca, non un limite clinico certificato.",
                zh = "所问问题:对于规范中声明的摄取不确定集合,每个 ROI 的剂量是否落在其判据内?引擎自行评估角点映射并返回经验包络 — 研究软件,非经认证的临床界限。",
                es = "Pregunta: para el conjunto de incertidumbre de captación declarado en la spec, ¿la dosis del plan en cada ROI entra en su criterio? El motor evalúa él mismo los mapas de esquina y devuelve una envolvente empírica — software de investigación, no un límite clínico certificado."
            ));
            ui.add_space(6.0);
            if ui
                .link(t!(
                    language,
                    en = "New here? Replay the intro tour",
                    ja = "はじめてですか?イントロツアーを再生",
                    it = "Nuovo qui? Rivedi il tour introduttivo",
                    zh = "第一次使用?重播入门导览",
                    es = "¿Nuevo aquí? Repite el tour de introducción"
                ))
                .clicked()
            {
                panel.tutorial_slide = Some(0);
            }
        });

    egui::Frame::new()
        .fill(theme.card_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.strong(t!(
                language,
                en = "Inputs",
                ja = "入力",
                it = "Input",
                zh = "输入",
                es = "Entradas"
            ));
            let field = |ui: &mut egui::Ui, label: &str, value: &mut String| {
                ui.horizontal(|ui| {
                    ui.label(label);
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .desired_width((ui.available_width() - 80.0).max(200.0)),
                    );
                });
            };
            field(ui, "case", &mut panel.case_path);
            #[cfg(not(target_arch = "wasm32"))]
            if !loaded_case_path.is_empty()
                && ui
                    .small_button(t!(
                        language,
                        en = "use loaded case",
                        ja = "読込済み症例を使用",
                        it = "usa il caso caricato",
                        zh = "使用已加载病例",
                        es = "usar el caso cargado"
                    ))
                    .clicked()
            {
                panel.case_path = loaded_case_path.to_string();
            }
            field(ui, "assignment", &mut panel.assignment_path);
            field(ui, "spec", &mut panel.spec_path);
            field(ui, "outdir", &mut panel.outdir);
            ui.horizontal(|ui| {
                ui.label("engine");
                ui.add(
                    egui::TextEdit::singleline(&mut panel.engine_cmd)
                        .desired_width(220.0)
                        .hint_text("avify-dose"),
                );
                ui.label(t!(
                    language,
                    en = "threads",
                    ja = "スレッド",
                    it = "thread",
                    zh = "线程",
                    es = "hilos"
                ));
                ui.add(egui::TextEdit::singleline(&mut panel.threads).desired_width(40.0));
                ui.label(t!(
                    language,
                    en = "timeout s",
                    ja = "タイムアウト秒",
                    it = "timeout s",
                    zh = "超时 秒",
                    es = "timeout s"
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut panel.timeout_s)
                        .desired_width(70.0)
                        .hint_text("21600"),
                );
            });
        });

    #[cfg(not(target_arch = "wasm32"))]
    let running = panel.poll();
    #[cfg(target_arch = "wasm32")]
    let running = false;
    if running {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }

    egui::Frame::new()
        .fill(theme.card_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let enabled = !running && cfg!(not(target_arch = "wasm32"));
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(t!(
                            language,
                            en = "Export voxel plan",
                            ja = "voxel plan を書き出す",
                            it = "Esporta voxel plan",
                            zh = "导出体素计划",
                            es = "Exportar voxel plan"
                        )),
                    )
                    .clicked()
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    panel.start(false);
                }
                if ui
                    .add_enabled(
                        enabled,
                        egui::Button::new(t!(
                            language,
                            en = "Verify",
                            ja = "検証",
                            it = "Verifica",
                            zh = "验证",
                            es = "Verificar"
                        )),
                    )
                    .on_disabled_hover_text(if cfg!(target_arch = "wasm32") {
                        t!(
                            language,
                            en = "native build only",
                            ja = "ネイティブ版のみ",
                            it = "solo build nativa",
                            zh = "仅限原生构建",
                            es = "solo build nativa"
                        )
                    } else {
                        t!(
                            language,
                            en = "a job is already running",
                            ja = "ジョブ実行中",
                            it = "un job è già in esecuzione",
                            zh = "已有任务在运行",
                            es = "ya hay un trabajo en ejecución"
                        )
                    })
                    .clicked()
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    panel.start(true);
                }
                if ui
                    .add_enabled(
                        running,
                        egui::Button::new(t!(
                            language,
                            en = "Cancel",
                            ja = "キャンセル",
                            it = "Annulla",
                            zh = "取消",
                            es = "Cancelar"
                        )),
                    )
                    .clicked()
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    panel.cancel();
                }
                if ui
                    .add_enabled(
                        cfg!(not(target_arch = "wasm32")),
                        egui::Button::new(t!(
                            language,
                            en = "Load result",
                            ja = "結果を読込",
                            it = "Carica risultato",
                            zh = "加载结果",
                            es = "Cargar resultado"
                        )),
                    )
                    .on_hover_text("load outdir/certificate.json + avify-run.json")
                    .clicked()
                {
                    #[cfg(not(target_arch = "wasm32"))]
                    panel.load_result();
                }
                if let Some(status) = &panel.status {
                    ui.monospace(status.clone());
                }
            });
            // Compare against a prior run — in-process
            // receipt/certificate diff, no engine invocation.
            ui.horizontal(|ui| {
                ui.label(t!(
                    language,
                    en = "Compare to run",
                    ja = "比較対象ラン",
                    it = "Confronta con run",
                    zh = "对比运行",
                    es = "Comparar con run"
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut panel.compare_path)
                        .desired_width(340.0)
                        .hint_text("prior outdir or avify-run.json"),
                );
                #[cfg(not(target_arch = "wasm32"))]
                if ui
                    .add_enabled(
                        !panel.compare_path.trim().is_empty(),
                        egui::Button::new(t!(
                            language,
                            en = "Diff",
                            ja = "差分",
                            it = "Diff",
                            zh = "对比",
                            es = "Diff"
                        )),
                    )
                    .on_hover_text(
                        "per-ROI interval/action changes + which bound inputs differ",
                    )
                    .clicked()
                {
                    let other = panel.compare_path.clone();
                    panel.compare_to(&other);
                }
            });
            if cfg!(target_arch = "wasm32") {
                ui.label(t!(
                    language,
                    en = "Process execution requires the native build — the web inspector is read-only.",
                    ja = "プロセス実行はネイティブ版のみ — Web インスペクタは読み取り専用です。",
                    it = "L'esecuzione richiede la build nativa — l'inspector web è di sola lettura.",
                    zh = "进程执行需要原生构建 — Web 检查器为只读。",
                    es = "La ejecución de procesos requiere la build nativa — el inspector web es de solo lectura."
                ));
            }
            if !panel.output.is_empty() {
                egui::ScrollArea::vertical()
                    .max_height(160.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &panel.output {
                            ui.monospace(line);
                        }
                    });
            }
        });

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(cert) = &panel.certificate {
        egui::Frame::new()
            .fill(theme.card_fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.strong(t!(
                    language,
                    en = "Certificate — empirical two-evaluation envelope (research only)",
                    ja = "証明書 — 経験的2評価エンベロープ(研究のみ)",
                    it = "Certificato — inviluppo empirico a due valutazioni (solo ricerca)",
                    zh = "证书 — 经验性双评估包络(仅限研究)",
                    es = "Certificado — envolvente empírica de dos evaluaciones (solo investigación)"
                ));
                egui::Grid::new("avify-actions").num_columns(4).show(ui, |ui| {
                    for (roi, action) in &cert.actions {
                        ui.label(roi);
                        ui.monospace(format!(
                            "{} {:.2} Gy-w",
                            action.criterion.0, action.criterion.1
                        ));
                        ui.monospace(format!(
                            "certified [{:.3}, {:.3}]{}",
                            action.certified_gyw[0],
                            action.certified_gyw[1],
                            action
                                .nominal_gyw
                                .map(|n| format!(" nominal {n:.3}"))
                                .unwrap_or_default()
                        ));
                        let color = match action.action.as_str() {
                            "PASS" => theme.brand,
                            "FAIL" => theme.error,
                            _ => theme.warn_text,
                        };
                        ui.colored_label(color, &action.action);
                        ui.end_row();
                    }
                });
                if !cert.runs.is_empty() {
                    ui.add_space(6.0);
                    for (name, run) in &cert.runs {
                        ui.monospace(format!(
                            "run {name:<10} histories={} wall={:.0}s seed={}",
                            run.histories, run.wall_s, run.seed
                        ));
                    }
                }
                // Engine identity: certificate stamp when present, the
                // receipt's --version answer otherwise.
                if let Some(engine) = &cert.engine {
                    ui.label(
                        egui::RichText::new(format!(
                            "engine: {}{}",
                            engine.name.as_deref().unwrap_or("avify-dose"),
                            engine
                                .version
                                .as_deref()
                                .map(|v| format!(" {v}"))
                                .unwrap_or_default()
                        ))
                        .small()
                        .color(theme.text_dim),
                    );
                }
            });
    }

    // Receipt-backed metadata: version-drift warnings, cold-vs-repeat
    // timing, and the reviewed marker — all connector-owned state.
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Warnings (version drift etc.) — verbatim from the receipt.
        if let Some(receipt) = &panel.receipt {
            for warning in &receipt.warnings {
                ui.colored_label(theme.warn_text, format!("\u{26a0} {warning}"));
            }
            if receipt.cold_start {
                ui.label(
                    egui::RichText::new(t!(
                        language,
                        en = "first run in this directory — timings include one-time setup",
                        ja = "このディレクトリでの初回実行 — 時間には初期セットアップが含まれます",
                        it = "prima esecuzione in questa directory — i tempi includono il setup iniziale",
                        zh = "此目录中的首次运行 — 计时包含一次性设置",
                        es = "primera ejecución en este directorio — los tiempos incluyen la configuración inicial"
                    ))
                    .small()
                    .color(theme.text_dim),
                );
            }
        }
        // Reviewed marker — badge or a two-field inline form.
        match &panel.review_state {
            Some(openbnct_avify::ReviewState::Current(review)) => {
                ui.colored_label(
                    theme.brand,
                    t!(
                        language,
                        en = "REVIEWED",
                        ja = "レビュー済み",
                        it = "REVISIONATO",
                        zh = "已评审",
                        es = "REVISADO"
                    ),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "by {}{}",
                        review.reviewer,
                        if review.note.is_empty() {
                            String::new()
                        } else {
                            format!(" \u{2014} {}", review.note)
                        }
                    ))
                    .small()
                    .color(theme.text_dim),
                );
            }
            Some(openbnct_avify::ReviewState::Stale { .. }) => {
                ui.colored_label(
                    theme.warn_text,
                    t!(
                        language,
                        en = "review STALE — certificate changed since review",
                        ja = "レビューは陳腐化 — 証明書が変更されました",
                        it = "revisione SCADUTA — certificato cambiato",
                        zh = "评审已过期 — 证书已更改",
                        es = "revisión OBSOLETA — el certificado cambió"
                    ),
                );
            }
            _ => {}
        }
        if panel.certificate.is_some() && panel.review_state.is_some() {
            let mut write_payload = None;
            let mut cancel_form = false;
            if let Some((reviewer, note)) = &mut panel.review_form {
                ui.horizontal(|ui| {
                    ui.label(t!(
                        language,
                        en = "reviewer:",
                        ja = "レビュアー:",
                        it = "revisore:",
                        zh = "评审人:",
                        es = "revisor:"
                    ));
                    ui.text_edit_singleline(reviewer);
                    ui.label(t!(
                        language,
                        en = "note:",
                        ja = "メモ:",
                        it = "nota:",
                        zh = "备注:",
                        es = "nota:"
                    ));
                    ui.text_edit_singleline(note);
                });
                ui.horizontal(|ui| {
                    if ui
                        .button(t!(
                            language,
                            en = "Write review.json",
                            ja = "review.json を書き込む",
                            it = "Scrivi review.json",
                            zh = "写入 review.json",
                            es = "Escribir review.json"
                        ))
                        .clicked()
                        && !reviewer.trim().is_empty()
                    {
                        write_payload =
                            Some((reviewer.trim().to_string(), note.trim().to_string()));
                    }
                    if ui
                        .button(t!(
                            language,
                            en = "Cancel",
                            ja = "キャンセル",
                            it = "Annulla",
                            zh = "取消",
                            es = "Cancelar"
                        ))
                        .clicked()
                    {
                        cancel_form = true;
                    }
                });
            } else if matches!(
                panel.review_state,
                Some(openbnct_avify::ReviewState::Missing) | None
            ) && ui
                .button(t!(
                    language,
                    en = "Mark reviewed",
                    ja = "レビュー済みにする",
                    it = "Segna revisionato",
                    zh = "标记为已评审",
                    es = "Marcar revisado"
                ))
                .clicked()
            {
                panel.review_form = Some((String::new(), String::new()));
            }
            if let Some((reviewer, note)) = write_payload {
                let outdir = std::path::PathBuf::from(panel.outdir.trim());
                match openbnct_avify::write_review(&outdir, &reviewer, &note) {
                    Ok(_) => {
                        panel.review_state = openbnct_avify::review_state(&outdir).ok();
                        panel.status = Some("reviewed".into());
                        panel.review_form = None;
                    }
                    Err(e) => panel.status = Some(format!("review: {e}")),
                }
            } else if cancel_form {
                panel.review_form = None;
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(arrays) = &panel.arrays {
        let mut clicked_cursor = None;
        egui::Frame::new()
            .fill(theme.card_fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.strong(t!(
                    language,
                    en = "Engine class map",
                    ja = "エンジンクラスマップ",
                    it = "Mappa classi del motore",
                    zh = "引擎类别图",
                    es = "Mapa de clases del motor"
                ));
                ui.label(
                    egui::RichText::new(t!(
                        language,
                        en = "The voxel classes the engine actually saw — click any view to move the shared crosshair.",
                        ja = "エンジンが実際に見た voxel クラス — 任意のビューをクリックすると共通クロスヘアが移動します。",
                        it = "Le classi voxel che il motore ha effettivamente visto — clicca una vista per spostare il crosshair condiviso.",
                        zh = "引擎实际看到的体素类别 — 点击任一视图移动共享十字线。",
                        es = "Las clases de vóxel que el motor realmente vio — clic en cualquier vista para mover el crosshair compartido."
                    ))
                    .color(theme.text_dim)
                    .small(),
                );
                let verdicts: Option<std::collections::BTreeMap<String, String>> =
                    if panel.verdict_overlay {
                        panel.certificate.as_ref().map(|cert| {
                            cert.actions
                                .iter()
                                .map(|(k, a)| (k.clone(), a.action.clone()))
                                .collect()
                        })
                    } else {
                        None
                    };
                if panel.certificate.is_some() {
                    ui.checkbox(
                        &mut panel.verdict_overlay,
                        t!(
                            language,
                            en = "tint ROIs by verdict",
                            ja = "判定で ROI を着色",
                            it = "colora le ROI per verdetto",
                            zh = "按判定为 ROI 着色",
                            es = "colorear ROI por veredicto"
                        ),
                    );
                }
                ui.horizontal(|ui| {
                    for (plane, label) in [(0usize, "axial"), (1, "coronal"), (2, "sagittal")] {
                        if let Some(voxel) =
                            avify_class_view(ui, arrays, plane, panel.cursor, verdicts.as_ref(), theme)
                        {
                            clicked_cursor = Some(voxel);
                        }
                        let _ = label;
                    }
                });
                // Class at the crosshair, named in text — readable
                // without colour.
                let [nx, ny, _nz] = [
                    arrays.shape_zyx[2],
                    arrays.shape_zyx[1],
                    arrays.shape_zyx[0],
                ];
                let [cx, cy, cz] = panel.cursor;
                let cls = arrays.cls_zyx[cz * ny * nx + cy * nx + cx];
                let class_names = ["air", "brain", "cranium", "scalp", "tumour"];
                ui.monospace(format!(
                    "voxel [{cx}, {cy}, {cz}] → class {} ({cls})",
                    class_names.get(cls as usize).unwrap_or(&"?")
                ));
                // Legend: swatch + name + class index.
                ui.horizontal_wrapped(|ui| {
                    for (index, name) in class_names.iter().enumerate() {
                        ui.colored_label(
                            avify_class_color(index as i8),
                            format!("■ {name} ({index})"),
                        );
                    }
                    for (name, _) in &arrays.rois {
                        ui.monospace(format!("▣ roi:{name}"));
                    }
                });
            });
        if let Some(voxel) = clicked_cursor {
            panel.cursor = voxel;
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    if let Some(states) = panel.staleness.clone() {
        let mut recheck = false;
        egui::Frame::new()
            .fill(theme.card_fill)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(t!(
                        language,
                        en = "Input binding",
                        ja = "入力バインディング",
                        it = "Binding degli input",
                        zh = "输入绑定",
                        es = "Vinculación de entradas"
                    ));
                    if ui
                        .small_button(t!(
                            language,
                            en = "recheck",
                            ja = "再確認",
                            it = "ricontrolla",
                            zh = "重新检查",
                            es = "reverificar"
                        ))
                        .clicked()
                    {
                        recheck = true;
                    }
                });
                for (name, state) in &states {
                    let (label, color) = match state {
                        openbnct_avify::InputState::Current => {
                            ("current".to_string(), theme.text_dim)
                        }
                        openbnct_avify::InputState::Changed(d) => {
                            (format!("CHANGED — now {d}"), theme.warn_text)
                        }
                        openbnct_avify::InputState::Missing => {
                            ("MISSING".to_string(), theme.error)
                        }
                    };
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{name:12}"));
                        ui.colored_label(color, label);
                    });
                }
                if openbnct_avify::is_stale(&states) {
                    ui.colored_label(
                        theme.warn_text,
                        t!(
                            language,
                            en = "STALE — inputs changed since this run; re-run Verify for a certificate over the current inputs.",
                            ja = "STALE — この実行以降に入力が変更されています。現在の入力に対する証明書を得るには Verify を再実行してください。",
                            it = "STALE — gli input sono cambiati dopo questa esecuzione; riesegui Verify per un certificato sugli input correnti.",
                            zh = "STALE — 自此次运行以来输入已更改;请重新运行 Verify 以获得当前输入的证书。",
                            es = "STALE — las entradas cambiaron desde esta ejecución; reejecute Verify para un certificado sobre las entradas actuales."
                        ),
                    );
                }
            });
        if recheck {
            panel.recheck_staleness();
        }
    }
}

// ── First-visit slide tours ────────────────────────────────────────────
//
// Two modal slide decks explain concepts the spotlight tour can't:
// `APP_TOUR_SLIDES` fires once on first launch (what OpenBNCT is and how
// its pieces fit); `AVIFY_TOUR_SLIDES` fires on the first visit to the
// experimental workspace. Seen-state persists to marker files under
// `~/.config/openbnct/` on native; on wasm each tour shows once per
// session. Diagrams are schematic painter figures — they teach the
// idea, not real geometry.

/// Marker recording that a tour has been shown; lives in the user's
/// config dir, not the workspace, so it survives updates and checkouts.
#[cfg(not(target_arch = "wasm32"))]
fn tour_marker(name: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|home| {
            home.join(".config")
                .join("openbnct")
                .join(format!("{name}-seen"))
        })
}

fn tour_seen_from_disk(name: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        tour_marker(name).is_some_and(|path| path.exists())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        false
    }
}

fn write_tour_marker(name: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = tour_marker(name) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, b"shown\n");
    }
    #[cfg(target_arch = "wasm32")]
    let _ = name;
}

/// One schematic figure per slide. Variants are shared across tours
/// where the same picture teaches both audiences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TourDiagram {
    NominalVsEnvelope,
    UptakeWhiskers,
    TwoCorner,
    Pipeline,
    Certificate,
    AppIdentity,
    Workspaces,
    Components,
    HashVerify,
    StatusHonesty,
    GettingStarted,
}

struct TourSlide {
    title: &'static str,
    body: &'static str,
    diagram: TourDiagram,
}

const APP_TOUR_SLIDES: [TourSlide; 6] = [
    TourSlide {
        title: "What OpenBNCT is",
        body: "An open research workbench for boron neutron capture therapy \
             dosimetry. You load a patient case, compute dose components, \
             and inspect the evidence behind every number.\n\n\
             This is research software — it shows what is known and what is \
             still uncertain, and never presents unfinished science as a \
             result.",
        diagram: TourDiagram::AppIdentity,
    },
    TourSlide {
        title: "One case, seven views",
        body: "The rail on the left moves ONE case through its whole life:\
             geometry (the patient as imaged), transport (particle \
             preparation), plan (source and boron modelling), dose \
             components, and the evidence ledger. Avify sits last — an \
             experimental envelope check.\n\n\
             Everything stays bound to the same case.",
        diagram: TourDiagram::Workspaces,
    },
    TourSlide {
        title: "Dose is four components, not one number",
        body: "BNCT dose is a sum with different biological weights: boron \
             capture, hydrogen recoil, nitrogen capture, and gamma. OpenBNCT \
             tracks and displays each separately because a plan that wins \
             on total dose can still lose on the component that matters.\n\n\
             Biological models weight them later — the components stay \
             visible underneath.",
        diagram: TourDiagram::Components,
    },
    TourSlide {
        title: "Every artifact is verified",
        body: "Every interchange file carries a content hash, and the app \
             checks integrity before it shows you a voxel. If a file was \
             edited, truncated, or mismatched, you see the boundary — not \
             quietly wrong data.",
        diagram: TourDiagram::HashVerify,
    },
    TourSlide {
        title: "Honest status, not green lights",
        body: "Status here is scoped, never global. Verified, frozen, \
             pending, blocked, and input required mean different things — a \
             green geometry gate does not promote transport, and a computed \
             dose does not claim clinical readiness.\n\n\
             Unfinished work stays labelled as unfinished.",
        diagram: TourDiagram::StatusHonesty,
    },
    TourSlide {
        title: "Start here",
        body: "\u{2022} Load a case directory — the bundled NF-BNCT-001 \
             example works out of the box\n\
             \u{2022} Read the readiness gates on Overview before trusting a \
             view\n\
             \u{2022} Press F1 anytime for the element tour — it points at \
             real buttons instead of explaining concepts",
        diagram: TourDiagram::GettingStarted,
    },
];

const AVIFY_TOUR_SLIDES: [TourSlide; 5] = [
    TourSlide {
        title: "A different question",
        body: "OpenBNCT asks \u{201c}what dose does this plan deliver?\u{201d} \
             Avify asks a second question: \u{201c}how sure are we of that \
             answer, given that boron uptake is never measured \
             perfectly?\u{201d}\n\n\
             Avify Dose is a separate program. This tab only packages the \
             question and reads the answer back \u{2014} the engine does \
             the evaluating.",
        diagram: TourDiagram::NominalVsEnvelope,
    },
    TourSlide {
        title: "Why boron is the hard part",
        body: "BNCT works because boron-10 inside a tumour cell captures a \
             neutron and releases a cell-killing burst. But boron maps come \
             from estimates \u{2014} PET uptake ratios and blood counts, \
             not a direct measurement of every voxel.\n\n\
             So the real dose could sit a little above or below the nominal \
             answer everywhere at once.",
        diagram: TourDiagram::UptakeWhiskers,
    },
    TourSlide {
        title: "The two-corner trick",
        body: "Instead of guessing one boron map, you declare a range of \
             plausible maps. The engine evaluates the two extreme corners \
             \u{2014} the pessimistic map and the optimistic map \u{2014} \
             and reports the interval between them.\n\n\
             If both ends of that interval satisfy your criteria, the plan \
             holds up across the whole declared uncertainty set.",
        diagram: TourDiagram::TwoCorner,
    },
    TourSlide {
        title: "What you provide",
        body: "\u{2022} Your OpenBNCT case and a tissue-class mapping\n\
             \u{2022} Uptake bounds \u{2014} the plausible boron range for \
             tumour and tissue\n\
             \u{2022} A criterion per region, e.g. tumour \u{2265} 20 Gy, \
             brain \u{2264} 10 Gy\n\n\
             The engine runs as its own bounded process; you can watch it \
             work and cancel it here.",
        diagram: TourDiagram::Pipeline,
    },
    TourSlide {
        title: "Reading the certificate",
        body: "Each region gets a certified interval [L, U] checked against \
             its criterion:\n\n\
             \u{2022} PASS \u{2014} the whole interval meets the criterion\n\
             \u{2022} FAIL \u{2014} the evidence doesn\u{2019}t support \
             it; with small runs this often just means \u{201c}not enough \
             histories yet\u{201d}\n\
             \u{2022} ADDITIONAL_EVIDENCE \u{2014} the interval straddles \
             the line\n\n\
             Research software only \u{2014} an empirical envelope, never a \
             clinical bound.",
        diagram: TourDiagram::Certificate,
    },
];

fn show_slide_tour(
    ctx: &egui::Context,
    window_title: &str,
    state: &mut Option<usize>,
    slides: &[TourSlide],
    theme: Theme,
) {
    let Some(index) = *state else {
        return;
    };
    let slide = &slides[index.min(slides.len() - 1)];
    let mut open = true;
    let mut advance: Option<Option<usize>> = None;
    egui::Window::new(window_title)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .fixed_size([580.0, 470.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(slide.title).size(20.0).strong());
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!("Slide {} of {}", index + 1, slides.len()))
                    .size(11.0)
                    .color(theme.text_dim),
            );
            ui.add_space(10.0);
            ui.label(slide.body);
            ui.add_space(12.0);

            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), 190.0),
                egui::Sense::hover(),
            );
            paint_tour_diagram(ui.painter(), rect, slide.diagram, theme);

            ui.add_space(10.0);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Skip tour").clicked() {
                        advance = Some(None);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if index + 1 < slides.len() {
                            if ui.button("Next \u{2192}").clicked() {
                                advance = Some(Some(index + 1));
                            }
                        } else if ui.button("Get started").clicked() {
                            advance = Some(None);
                        }
                        if index > 0 && ui.button("\u{2190} Back").clicked() {
                            advance = Some(Some(index - 1));
                        }
                    });
                    ui.with_layout(
                        egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                        |ui| {
                            ui.horizontal(|ui| {
                                for dot in 0..slides.len() {
                                    let color = if dot == index {
                                        theme.brand
                                    } else {
                                        theme.text_dim.gamma_multiply(0.45)
                                    };
                                    ui.painter().circle_filled(
                                        ui.cursor().min + egui::vec2(6.0, 10.0),
                                        4.0,
                                        color,
                                    );
                                    ui.add_space(11.0);
                                }
                            });
                        },
                    );
                });
            });
        });
    if !open {
        advance = Some(None);
    }
    if let Some(next) = advance {
        *state = next;
    }
}

fn show_avify_tutorial(ctx: &egui::Context, panel: &mut AvifyPanel, theme: Theme) {
    show_slide_tour(
        ctx,
        "Welcome to Avify Dose \u{2014} experimental",
        &mut panel.tutorial_slide,
        &AVIFY_TOUR_SLIDES,
        theme,
    );
}

/// The per-slide figures, keyed by variant so app and Avify tours can
/// share a drawing where the same idea serves both.
fn paint_tour_diagram(p: &egui::Painter, rect: egui::Rect, diagram: TourDiagram, theme: Theme) {
    let bg = theme.card_alt_fill;
    p.rect_filled(rect, 6.0, bg);
    let text = egui::FontId::proportional(12.0);
    let strong = egui::FontId::proportional(13.0);
    let dim = theme.text_dim;
    let brand = theme.brand;
    let ok = egui::Color32::from_rgb(70, 160, 90);
    let c = rect.center();
    let stroke = egui::Stroke::new(1.5, dim);
    match diagram {
        TourDiagram::NominalVsEnvelope => {
            // Nominal single answer vs envelope band.
            let left =
                egui::Rect::from_center_size(c + egui::vec2(-140.0, 0.0), egui::vec2(170.0, 120.0));
            let right =
                egui::Rect::from_center_size(c + egui::vec2(140.0, 0.0), egui::vec2(170.0, 120.0));
            p.rect_filled(left, 8.0, bg.gamma_multiply(1.15));
            p.rect_filled(right, 8.0, bg.gamma_multiply(1.15));
            p.text(
                left.center_top() + egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                "Nominal dose",
                strong.clone(),
                dim,
            );
            p.text(
                right.center_top() + egui::vec2(0.0, 14.0),
                egui::Align2::CENTER_CENTER,
                "Avify envelope",
                strong.clone(),
                brand,
            );
            let ly = left.center().y + 12.0;
            p.line_segment(
                [
                    egui::pos2(left.min.x + 30.0, ly),
                    egui::pos2(left.max.x - 30.0, ly),
                ],
                egui::Stroke::new(2.0, dim),
            );
            p.circle_filled(egui::pos2(left.center().x, ly), 5.0, dim);
            p.text(
                egui::pos2(left.center().x, ly + 18.0),
                egui::Align2::CENTER_CENTER,
                "one number",
                text.clone(),
                dim,
            );
            let ry = right.center().y + 12.0;
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(right.min.x + 30.0, ry - 9.0),
                    egui::pos2(right.max.x - 30.0, ry + 9.0),
                ),
                4.0,
                brand.gamma_multiply(0.35),
            );
            for (x, label) in [(right.min.x + 30.0, "L"), (right.max.x - 30.0, "U")] {
                p.line_segment(
                    [egui::pos2(x, ry - 14.0), egui::pos2(x, ry + 14.0)],
                    egui::Stroke::new(2.0, brand),
                );
                p.text(
                    egui::pos2(x, ry + 24.0),
                    egui::Align2::CENTER_CENTER,
                    label,
                    strong.clone(),
                    brand,
                );
            }
            p.text(
                right.center_bottom() + egui::vec2(0.0, -8.0),
                egui::Align2::CENTER_CENTER,
                "a range",
                text.clone(),
                dim,
            );
        }
        TourDiagram::UptakeWhiskers => {
            // Uptake estimate bars with uncertainty whiskers.
            for (i, (name, h)) in [("tumour", 96.0_f32), ("tissue", 44.0)].iter().enumerate() {
                let x = c.x - 110.0 + i as f32 * 220.0;
                let top = rect.max.y - 40.0 - h;
                p.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x - 34.0, top),
                        egui::pos2(x + 34.0, rect.max.y - 40.0),
                    ),
                    3.0,
                    if i == 0 {
                        brand.gamma_multiply(0.7)
                    } else {
                        dim.gamma_multiply(0.5)
                    },
                );
                let wx = x + 44.0;
                for dy in [-18.0_f32, 18.0] {
                    p.line_segment(
                        [
                            egui::pos2(wx - 6.0, top + dy),
                            egui::pos2(wx + 6.0, top + dy),
                        ],
                        egui::Stroke::new(2.0, theme.warn_text),
                    );
                }
                p.line_segment(
                    [egui::pos2(wx, top - 18.0), egui::pos2(wx, top + 18.0)],
                    egui::Stroke::new(2.0, theme.warn_text),
                );
                p.text(
                    egui::pos2(x, rect.max.y - 30.0),
                    egui::Align2::CENTER_CENTER,
                    *name,
                    text.clone(),
                    dim,
                );
            }
            p.text(
                egui::pos2(rect.center().x, rect.min.y + 18.0),
                egui::Align2::CENTER_CENTER,
                "estimated boron uptake \u{2014} true value inside the whiskers",
                text.clone(),
                dim,
            );
        }
        TourDiagram::TwoCorner => {
            // Declared range -> two corner evaluations -> interval.
            let y = rect.min.y + 52.0;
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + 60.0, y - 4.0),
                    egui::pos2(rect.max.x - 60.0, y + 4.0),
                ),
                4.0,
                dim.gamma_multiply(0.4),
            );
            for (x, label) in [
                (rect.min.x + 60.0, "pessimistic corner"),
                (rect.max.x - 60.0, "optimistic corner"),
            ] {
                p.circle_filled(egui::pos2(x, y), 7.0, brand);
                p.text(
                    egui::pos2(x, y + 20.0),
                    egui::Align2::CENTER_CENTER,
                    label,
                    text.clone(),
                    dim,
                );
            }
            p.text(
                egui::pos2(rect.center().x, y - 24.0),
                egui::Align2::CENTER_CENTER,
                "declared uptake range",
                strong.clone(),
                dim,
            );
            for x in [rect.min.x + 60.0, rect.max.x - 60.0] {
                p.arrow(egui::pos2(x, y + 34.0), egui::vec2(0.0, 34.0), stroke);
            }
            let by = rect.max.y - 48.0;
            for (x, w, label, col) in [
                (
                    rect.min.x + 60.0,
                    70.0,
                    "run 1 \u{2192} low dose",
                    theme.warn_text,
                ),
                (rect.max.x - 60.0, 110.0, "run 2 \u{2192} high dose", ok),
            ] {
                p.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(x - w / 2.0, by - 8.0),
                        egui::pos2(x + w / 2.0, by + 8.0),
                    ),
                    4.0,
                    col.gamma_multiply(0.5),
                );
                p.text(
                    egui::pos2(x, by + 20.0),
                    egui::Align2::CENTER_CENTER,
                    label,
                    text.clone(),
                    dim,
                );
            }
        }
        TourDiagram::Pipeline => {
            // OpenBNCT -> engine -> certificate pipeline.
            let boxes = [
                ("OpenBNCT", "your case + spec", brand),
                ("Avify engine", "separate program", dim),
                ("Certificate", "[L, U] per region", ok),
            ];
            let bw = 150.0;
            for (i, (t1, t2, col)) in boxes.iter().enumerate() {
                let bx = egui::Rect::from_center_size(
                    egui::pos2(rect.min.x + 95.0 + i as f32 * 195.0, c.y - 10.0),
                    egui::vec2(bw, 76.0),
                );
                p.rect_filled(bx, 8.0, bg.gamma_multiply(1.15));
                p.rect_stroke(
                    bx,
                    8.0,
                    egui::Stroke::new(1.5, *col),
                    egui::StrokeKind::Inside,
                );
                p.text(
                    bx.center() - egui::vec2(0.0, 12.0),
                    egui::Align2::CENTER_CENTER,
                    *t1,
                    strong.clone(),
                    *col,
                );
                p.text(
                    bx.center() + egui::vec2(0.0, 12.0),
                    egui::Align2::CENTER_CENTER,
                    *t2,
                    text.clone(),
                    dim,
                );
                if i < 2 {
                    p.arrow(
                        egui::pos2(bx.max.x + 6.0, bx.center().y),
                        egui::vec2(33.0, 0.0),
                        stroke,
                    );
                }
            }
            p.text(
                egui::pos2(rect.center().x, rect.max.y - 22.0),
                egui::Align2::CENTER_CENTER,
                "OpenBNCT packages the question \u{2014} the engine produces the answer",
                text.clone(),
                dim,
            );
        }
        TourDiagram::Certificate => {
            // Interval vs criterion line with PASS chip.
            let y = c.y - 20.0;
            p.line_segment(
                [
                    egui::pos2(rect.min.x + 70.0, y),
                    egui::pos2(rect.max.x - 70.0, y),
                ],
                egui::Stroke::new(2.0, dim.gamma_multiply(0.6)),
            );
            let crit = egui::pos2(rect.min.x + 300.0, y);
            p.line_segment(
                [crit + egui::vec2(0.0, -30.0), crit + egui::vec2(0.0, 30.0)],
                egui::Stroke::new(2.0, dim),
            );
            p.text(
                crit + egui::vec2(0.0, -40.0),
                egui::Align2::CENTER_CENTER,
                "criterion \u{2265} 20 Gy",
                text.clone(),
                dim,
            );
            p.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + 330.0, y - 8.0),
                    egui::pos2(rect.max.x - 100.0, y + 8.0),
                ),
                4.0,
                ok.gamma_multiply(0.5),
            );
            for (x, lab) in [(rect.min.x + 330.0, "L"), (rect.max.x - 100.0, "U")] {
                p.line_segment(
                    [egui::pos2(x, y - 13.0), egui::pos2(x, y + 13.0)],
                    egui::Stroke::new(2.0, ok),
                );
                p.text(
                    egui::pos2(x, y + 24.0),
                    egui::Align2::CENTER_CENTER,
                    lab,
                    strong.clone(),
                    ok,
                );
            }
            let chip = egui::Rect::from_center_size(
                egui::pos2(rect.center().x - 60.0, rect.max.y - 40.0),
                egui::vec2(150.0, 30.0),
            );
            p.rect_filled(chip, 15.0, ok.gamma_multiply(0.35));
            p.text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                "PASS \u{2014} whole interval clears it",
                text.clone(),
                ok,
            );
        }
        TourDiagram::AppIdentity => {
            // Case in, three views out under one roof.
            let case_box = egui::Rect::from_center_size(
                egui::pos2(rect.min.x + 85.0, c.y - 12.0),
                egui::vec2(110.0, 70.0),
            );
            p.rect_filled(case_box, 8.0, bg.gamma_multiply(1.15));
            p.rect_stroke(
                case_box,
                8.0,
                egui::Stroke::new(1.5, brand),
                egui::StrokeKind::Inside,
            );
            p.text(
                case_box.center() - egui::vec2(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "patient",
                strong.clone(),
                dim,
            );
            p.text(
                case_box.center() + egui::vec2(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "case",
                strong.clone(),
                brand,
            );
            let roof = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + 200.0, rect.min.y + 40.0),
                egui::pos2(rect.max.x - 40.0, rect.max.y - 60.0),
            );
            p.rect_stroke(
                roof,
                10.0,
                egui::Stroke::new(2.0, brand),
                egui::StrokeKind::Inside,
            );
            p.text(
                roof.center_top() + egui::vec2(0.0, 16.0),
                egui::Align2::CENTER_CENTER,
                "OpenBNCT workbench",
                strong.clone(),
                brand,
            );
            p.arrow(
                egui::pos2(case_box.max.x + 6.0, case_box.center().y),
                egui::vec2(30.0, 0.0),
                stroke,
            );
            for (i, label) in ["geometry", "transport + dose", "evidence"]
                .iter()
                .enumerate()
            {
                let chip = egui::Rect::from_center_size(
                    egui::pos2(roof.center().x, roof.min.y + 60.0 + i as f32 * 34.0),
                    egui::vec2(170.0, 24.0),
                );
                p.rect_filled(chip, 12.0, dim.gamma_multiply(0.25));
                p.text(
                    chip.center(),
                    egui::Align2::CENTER_CENTER,
                    *label,
                    text.clone(),
                    dim,
                );
            }
        }
        TourDiagram::Workspaces => {
            // Left rail of workspace buttons feeding one case.
            let names = ["Geometry", "Transport", "Plan", "Dose", "Evidence", "Avify"];
            for (i, name) in names.iter().enumerate() {
                let btn = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + 40.0, rect.min.y + 26.0 + i as f32 * 26.0),
                    egui::pos2(rect.min.x + 170.0, rect.min.y + 44.0 + i as f32 * 26.0),
                );
                p.rect_filled(
                    btn,
                    6.0,
                    if i < 5 {
                        dim.gamma_multiply(0.3)
                    } else {
                        brand.gamma_multiply(0.4)
                    },
                );
                p.text(
                    btn.center(),
                    egui::Align2::CENTER_CENTER,
                    *name,
                    text.clone(),
                    if i < 5 { dim } else { brand },
                );
            }
            p.arrow(
                egui::pos2(rect.min.x + 190.0, c.y - 10.0),
                egui::vec2(60.0, 0.0),
                stroke,
            );
            let case = egui::Rect::from_center_size(
                egui::pos2(rect.min.x + 330.0, c.y - 10.0),
                egui::vec2(130.0, 90.0),
            );
            p.rect_filled(case, 10.0, bg.gamma_multiply(1.15));
            p.rect_stroke(
                case,
                10.0,
                egui::Stroke::new(1.5, brand),
                egui::StrokeKind::Inside,
            );
            p.text(
                case.center() - egui::vec2(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "one case",
                strong.clone(),
                brand,
            );
            p.text(
                case.center() + egui::vec2(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "same data throughout",
                text.clone(),
                dim,
            );
            p.text(
                egui::pos2(rect.min.x + 105.0, rect.max.y - 14.0),
                egui::Align2::CENTER_CENTER,
                "left rail",
                text.clone(),
                dim,
            );
        }
        TourDiagram::Components => {
            // Four stacked dose-component bars.
            let components = [
                ("boron", 0.42, brand),
                ("hydrogen", 0.26, dim),
                ("nitrogen", 0.12, dim),
                ("gamma", 0.20, dim),
            ];
            let bar = egui::Rect::from_min_max(
                egui::pos2(rect.min.x + 60.0, c.y - 26.0),
                egui::pos2(rect.max.x - 60.0, c.y + 26.0),
            );
            let mut x = bar.min.x;
            for (i, (name, frac, col)) in components.iter().enumerate() {
                let w = bar.width() * frac;
                let seg = egui::Rect::from_min_max(
                    egui::pos2(x, bar.min.y),
                    egui::pos2(x + w - 2.0, bar.max.y),
                );
                p.rect_filled(
                    seg,
                    if i == 0 { 5.0 } else { 2.0 },
                    col.gamma_multiply(if i == 0 { 0.8 } else { 0.45 }),
                );
                p.text(
                    seg.center() + egui::vec2(0.0, 34.0),
                    egui::Align2::CENTER_CENTER,
                    *name,
                    text.clone(),
                    if i == 0 { brand } else { dim },
                );
                x += w;
            }
            p.text(
                egui::pos2(c.x, bar.min.y - 18.0),
                egui::Align2::CENTER_CENTER,
                "each component tracked separately",
                strong.clone(),
                dim,
            );
        }
        TourDiagram::HashVerify => {
            // File -> sha256 -> check.
            let file = egui::Rect::from_center_size(
                egui::pos2(rect.min.x + 100.0, c.y - 10.0),
                egui::vec2(90.0, 110.0),
            );
            p.rect_filled(file, 4.0, bg.gamma_multiply(1.15));
            p.rect_stroke(file, 4.0, stroke, egui::StrokeKind::Inside);
            for i in 0..4 {
                p.line_segment(
                    [
                        egui::pos2(file.min.x + 14.0, file.min.y + 24.0 + i as f32 * 18.0),
                        egui::pos2(file.max.x - 14.0, file.min.y + 24.0 + i as f32 * 18.0),
                    ],
                    egui::Stroke::new(1.0, dim.gamma_multiply(0.5)),
                );
            }
            p.arrow(
                egui::pos2(file.max.x + 10.0, c.y - 10.0),
                egui::vec2(46.0, 0.0),
                stroke,
            );
            let hash = egui::Rect::from_center_size(
                egui::pos2(c.x + 30.0, c.y - 10.0),
                egui::vec2(150.0, 44.0),
            );
            p.rect_filled(hash, 6.0, dim.gamma_multiply(0.25));
            p.text(
                hash.center(),
                egui::Align2::CENTER_CENTER,
                "sha256:9f2a\u{2026}",
                text.clone(),
                dim,
            );
            p.arrow(
                egui::pos2(hash.max.x + 10.0, c.y - 10.0),
                egui::vec2(46.0, 0.0),
                stroke,
            );
            p.circle_filled(
                egui::pos2(rect.max.x - 80.0, c.y - 10.0),
                22.0,
                ok.gamma_multiply(0.4),
            );
            p.text(
                egui::pos2(rect.max.x - 80.0, c.y - 10.0),
                egui::Align2::CENTER_CENTER,
                "\u{2713}",
                strong.clone(),
                ok,
            );
            p.text(
                egui::pos2(rect.max.x - 80.0, c.y + 24.0),
                egui::Align2::CENTER_CENTER,
                "verified",
                text.clone(),
                ok,
            );
            p.text(
                egui::pos2(rect.center().x, rect.max.y - 18.0),
                egui::Align2::CENTER_CENTER,
                "tampered or mismatched files stop at the boundary",
                text.clone(),
                dim,
            );
        }
        TourDiagram::StatusHonesty => {
            // Scoped status chips: no global green light.
            let chips = [
                ("verified", ok),
                ("frozen", brand),
                ("pending", theme.warn_text),
                ("blocked", theme.error),
                ("input required", dim),
            ];
            for (i, (label, col)) in chips.iter().enumerate() {
                let chip = egui::Rect::from_center_size(
                    egui::pos2(
                        rect.min.x + 100.0 + (i % 3) as f32 * 190.0,
                        c.y - 34.0 + (i / 3) as f32 * 44.0,
                    ),
                    egui::vec2(150.0, 30.0),
                );
                p.rect_filled(chip, 15.0, col.gamma_multiply(0.3));
                p.text(
                    chip.center(),
                    egui::Align2::CENTER_CENTER,
                    *label,
                    text.clone(),
                    *col,
                );
            }
            p.text(
                egui::pos2(rect.center().x, rect.max.y - 22.0),
                egui::Align2::CENTER_CENTER,
                "each claim is scoped \u{2014} there is no global all-clear badge",
                text.clone(),
                dim,
            );
        }
        TourDiagram::GettingStarted => {
            // Pointer to the Load & verify button, F1 hint chip.
            let btn = egui::Rect::from_center_size(
                egui::pos2(c.x - 60.0, c.y - 20.0),
                egui::vec2(200.0, 44.0),
            );
            p.rect_filled(btn, 8.0, brand.gamma_multiply(0.5));
            p.rect_stroke(
                btn,
                8.0,
                egui::Stroke::new(1.5, brand),
                egui::StrokeKind::Inside,
            );
            p.text(
                btn.center(),
                egui::Align2::CENTER_CENTER,
                "Load & verify",
                strong.clone(),
                brand,
            );
            p.circle_filled(btn.center() + egui::vec2(90.0, 34.0), 8.0, theme.warn_text);
            p.line(
                vec![
                    btn.center() + egui::vec2(96.0, 42.0),
                    btn.center() + egui::vec2(112.0, 60.0),
                    btn.center() + egui::vec2(100.0, 62.0),
                    btn.center() + egui::vec2(104.0, 74.0),
                ],
                egui::Stroke::new(2.0, theme.warn_text),
            );
            let f1 = egui::Rect::from_center_size(
                egui::pos2(rect.max.x - 110.0, rect.max.y - 44.0),
                egui::vec2(170.0, 30.0),
            );
            p.rect_filled(f1, 15.0, dim.gamma_multiply(0.3));
            p.text(
                f1.center(),
                egui::Align2::CENTER_CENTER,
                "F1 \u{2192} element tour",
                text.clone(),
                dim,
            );
        }
    }
}

fn show_workspace_heading(ui: &mut egui::Ui, theme: Theme, title: &str, subtitle: &str) {
    ui.heading(egui::RichText::new(title).size(23.0));
    ui.label(egui::RichText::new(subtitle).color(theme.text_dim));
    ui.add_space(8.0);
}

fn show_overview(
    ui: &mut egui::Ui,
    case: Option<&ViewerCase>,
    dose: &mut DosePanel,
    workspace: &mut WorkspaceTab,
    language: Language,
    tour_targets: &mut TourTargets,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(
            language,
            en = "Your research workspace",
            ja = "研究ワークベンチ",
            it = "Il tuo spazio di ricerca",
            zh = "您的研究工作区",
            es = "Su espacio de investigación"
        ),
        t!(
            language,
            en = "Inspect the case. Explore dose. Follow the evidence.",
            ja = "症例を確認し、線量を探索し、エビデンスを追跡する。",
            it = "Ispeziona il caso. Esplora la dose. Segui l'evidenza.",
            zh = "检查病例。探索剂量。追踪证据。",
            es = "Inspeccione el caso. Explore la dosis. Siga la evidencia."
        ),
    );
    ui.add_space(12.0);
    egui::Frame::new().fill(theme.card_fill).corner_radius(10)
        .inner_margin(egui::Margin::same(24)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(t!(language, en = "CASE STUDY  /  SYNTHETIC BENCHMARK", ja = "症例 / 合成ベンチマーク")).size(11.0).strong().color(theme.brand));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(case.map_or(t!(language, en = "Start with a verified case", ja = "検証済み症例から開始",
                it = "Inizia con un caso verificato",
                zh = "从已验证的病例开始",
                es = "Comience con un caso verificado"), |c| c.data.case_id.as_str())).size(26.0).strong());
            ui.label(egui::RichText::new(if case.is_some() {
                t!(language, en = "Patient-space geometry and artifact integrity verified. Ready to inspect.", ja = "患者空間ジオメトリとアーティファクト完全性を検証済み。確認の準備ができています。")
            } else {
                t!(language, en = "Open an NF-BNCT-001 case folder above. Geometry is verified before it is displayed.", ja = "上で NF-BNCT-001 症例フォルダを開いてください。ジオメトリは表示前に検証されます。",
                it = "Apri una cartella caso NF-BNCT-001 sopra. La geometria è verificata prima della visualizzazione.",
                zh = "在上方打开 NF-BNCT-001 病例文件夹。几何在显示前经过验证。",
                es = "Abra una carpeta de caso NF-BNCT-001 arriba. La geometría se verifica antes de mostrarse.")
            }).color(theme.text_dim));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(case.is_some(), egui::Button::new(t!(language, en = "Inspect geometry", ja = "ジオメトリを確認",
                it = "Ispeziona geometria",
                zh = "检查几何",
                es = "Inspeccionar geometría"))).clicked() {
                    *workspace = WorkspaceTab::Geometry;
                }
                if ui.button(t!(language, en = "Review evidence", ja = "エビデンスを確認",
                it = "Rivedi evidenza",
                zh = "查看证据",
                es = "Revisar evidencia")).clicked() { *workspace = WorkspaceTab::Evidence; }
                if dose.bundle.is_none() && ui
                    .button(t!(language, en = "Open the example dose bundle",
                        ja = "サンプル線量バンドルを開く",
                        it = "Apri il bundle di dose di esempio",
                        zh = "打开示例剂量束",
                        es = "Abrir paquete de dosis de ejemplo"))
                    .on_hover_text(t!(language,
                        en = "bundled layered-head benchmark — no files needed",
                        ja = "同梱の layered-head ベンチマーク — ファイル不要",
                        it = "benchmark layered-head incluso — nessun file necessario",
                        zh = "内置 layered-head 基准 — 无需文件",
                        es = "benchmark layered-head incluido — sin archivos"))
                    .clicked()
                {
                    match example_dose_bytes() {
                        Ok(bytes) => {
                            if dose.load_bundle_bytes(bytes).is_ok() {
                                *workspace = WorkspaceTab::Dose;
                            }
                        }
                        Err(error) => dose.bundle_error = Some(error),
                    }
                }
            });
        });
    ui.add_space(20.0);
    ui.heading(t!(
        language,
        en = "Explore the workbench",
        ja = "ワークベンチを探索",
        it = "Esplora il workbench",
        zh = "探索工作台",
        es = "Explorar el entorno"
    ));
    ui.add_space(6.0);
    ui.columns(3, |columns| {
        for (column, (tab, title, detail, action)) in columns.iter_mut().zip([
            (WorkspaceTab::Transport,
                t!(language, en = "01  Prepare", ja = "01  準備",
                it = "01  Prepara",
                zh = "01  准备",
                es = "01  Preparar"),
                t!(language, en = "Inspect material and source contracts, position the beam, and review transport readiness.", ja = "材料・線源の契約を確認し、ビームを位置決めし、輸送レディネスをレビュー。",
                it = "Ispeziona i contratti di materiale e sorgente, posiziona il fascio e rivedi la prontezza del trasporto.",
                zh = "检查材料与源合约、定位射束并查看输运就绪状态。",
                es = "Inspeccione los contratos de material y fuente, posicione el haz y revise la preparación del transporte."),
                t!(language, en = "Open transport", ja = "輸送を開く",
                it = "Apri trasporto",
                zh = "打开输运",
                es = "Abrir transporte")),
            (WorkspaceTab::Plan,
                t!(language, en = "02  Evaluate", ja = "02  評価",
                it = "02  Valuta",
                zh = "02  评估",
                es = "02  Evaluar"),
                t!(language, en = "Load a plan, inspect fields and weights, and review its calculated result.", ja = "計画を読み込み、フィールドと重みを確認し、計算結果をレビュー。",
                it = "Carica un piano, ispeziona campi e pesi e rivedi il risultato calcolato.",
                zh = "加载计划，检查射野与权重，并查看计算结果。",
                es = "Cargue un plan, inspeccione campos y pesos, y revise el resultado calculado."),
                t!(language, en = "Open planning", ja = "計画を開く",
                it = "Apri pianificazione",
                zh = "打开计划",
                es = "Abrir planificación")),
            (WorkspaceTab::Dose,
                t!(language, en = "03  Understand", ja = "03  理解",
                it = "03  Comprendi",
                zh = "03  理解",
                es = "03  Comprender"),
                t!(language, en = "Explore physical and biological dose, region metrics, DVHs, and NIfTI volumes.", ja = "物理・生物学的線量、領域指標、DVH、NIfTI ボリュームを探索。",
                it = "Esplora dose fisica e biologica, metriche di regione, DVH e volumi NIfTI.",
                zh = "探索物理与生物剂量、区域指标、DVH 和 NIfTI 体数据。",
                es = "Explore dosis física y biológica, métricas de región, DVH y volúmenes NIfTI."),
                t!(language, en = "Explore dose", ja = "線量を探索",
                it = "Esplora dose",
                zh = "探索剂量",
                es = "Explorar dosis")),
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
        ui.heading(t!(language, en = "Benchmark readiness", ja = "ベンチマークレディネス",
                it = "Prontezza del benchmark",
                zh = "基准就绪状态",
                es = "Preparación del benchmark"));
        ui.label(egui::RichText::new(t!(language, en = "Qualification of the frozen reference workflow; imported artifacts have their own validation.", ja = "凍結参照ワークフローの適格性。取り込み済みアーティファクトは独自の検証を持ちます。")).color(theme.text_dim));
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
        t!(
            language,
            en = "Geometry",
            ja = "ジオメトリ",
            it = "Geometria",
            zh = "几何",
            es = "Geometría"
        ),
        t!(
            language,
            en = "Integrity-gated, linked patient-space views of the frozen synthetic case.",
            ja = "完全性ゲート済みの患者空間ビュー — 凍結ベンチマークまたは取り込み済みスタディ。"
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
                        ui.heading(t!(language, en = "Image inspector", ja = "画像インスペクタ",
                it = "Ispettore immagini",
                zh = "图像检查器",
                es = "Inspector de imágenes"));
                        ui.label(
                            egui::RichText::new(t!(language, en = "Click or drag an image to move the linked crosshair.", ja = "画像をクリック/ドラッグすると連動クロスヘアが移動します。",
                it = "Clicca o trascina un'immagine per spostare il crosshair collegato.",
                zh = "点击或拖动图像以移动联动十字线。",
                es = "Haga clic o arrastre una imagen para mover la retícula vinculada."))
                            .color(theme.text_dim),
                        );
                        ui.collapsing(
                            t!(language, en = "Case & voxel details", ja = "症例・ボクセル詳細",
                it = "Dettagli caso e voxel",
                zh = "病例与体素详情",
                es = "Detalles de caso y voxel"),
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
                    t!(language, en = "spectrum rejected", ja = "スペクトル拒否",
                it = "spettro rifiutato",
                zh = "能谱被拒绝",
                es = "espectro rechazado")
                ),
            );
        }
        let Some(source) = &spectrum.source else {
            ui.label(t!(language, en = "No source loaded — drop a beam-description or fixed-source-definition .json, or load the bundled FiR 1 example.", ja = "線源未読込 — beam-description または fixed-source-definition .json をドロップ、または同梱の FiR 1 例を開く。",
                it = "Nessuna sorgente caricata — trascina un .json beam-description o fixed-source-definition, oppure l'esempio FiR 1 incluso.",
                zh = "未加载源 — 拖入 beam-description 或 fixed-source-definition 的 .json，或打开内置 FiR 1 示例。",
                es = "Ninguna fuente cargada — suelte un .json beam-description o fixed-source-definition, o cargue el ejemplo FiR 1 incluido."));
            if ui
                .button(t!(language, en = "Load FiR 1 example", ja = "FiR 1 例を読み込む",
                    it = "Carica esempio FiR 1", zh = "加载 FiR 1 示例",
                    es = "Cargar ejemplo FiR 1"))
                .clicked()
            {
                spectrum.load_bytes(EXAMPLE_BEAM_JSON.as_bytes());
            }
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
                t!(language, en = "Energy spectrum plot — thermal, epithermal, and fast regions", ja = "エネルギースペクトルプロット — 熱・エピサーマル・高速領域"),
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
        t!(
            language,
            en = "Transport",
            ja = "輸送",
            it = "Trasporto",
            zh = "输运",
            es = "Transporte"
        ),
        t!(
            language,
            en = "Backend-neutral preparation with explicit scientific and execution gates.",
            ja = "バックエンド中立の準備 — 科学的ゲートと実行ゲートを明示。",
            it = "Preparazione backend-neutrale con gate scientifici ed esecutivi espliciti.",
            zh = "后端中立的准备，带有明确的科学与执行门控。",
            es =
                "Preparación neutral al backend con puertas científicas y de ejecución explícitas."
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
        ui.heading(t!(
            language,
            en = "Source spectrum",
            ja = "線源スペクトル",
            it = "Spettro della sorgente",
            zh = "源能谱",
            es = "Espectro de la fuente"
        ))
        .rect,
    );
    ui.label(
        "Drop a beam-description or fixed-source-definition JSON — the energy histogram \
         renders log-log with TECDOC-1223 region shading.",
    );
    show_spectrum(ui, spectrum, language, theme);

    ui.add_space(12.0);
    let gate_chain = ui.scope(|ui| {
        ui.heading(t!(
            language,
            en = "Run gate chain",
            ja = "ゲートチェーンを実行",
            it = "Catena di gate di run",
            zh = "运行门控链",
            es = "Cadena de puertas de ejecución"
        ));
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
            en = "Disabled controls reflect real adapter capabilities.",
            ja = "無効なコントロールは実際のアダプタ能力を反映しています。",
            it = "I controlli disabilitati riflettono le reali capacità degli adapter.",
            zh = "禁用的控件反映真实的适配器能力。",
            es = "Los controles deshabilitados reflejan las capacidades reales del adaptador."
        ));
    });
    tour_targets.set(TourTarget::TransportActions, actions.response.rect);

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "Source positioning",
        ja = "線源の位置決め",
        it = "Posizionamento sorgente",
        zh = "源定位",
        es = "Posicionamiento de fuente"
    ));
    ui.label(t!(language, en = "Same aim/rotate path as `openbnct position` — reports the entry geometry without running transport.", ja = "`openbnct position` と同じ照準・回転パス — 輸送を実行せず入射ジオメトリのみ報告。"));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "SOURCE",
                    ja = "線源",
                    it = "SORGENTE",
                    zh = "源",
                    es = "FUENTE"
                ))
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
                egui::RichText::new(t!(
                    language,
                    en = "TARGET",
                    ja = "ターゲット",
                    it = "BERSAGLIO",
                    zh = "靶区",
                    es = "OBJETIVO"
                ))
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
            ui.label(t!(
                language,
                en = "or mask file:",
                ja = "またはマスクファイル:",
                it = "o file maschera:",
                zh = "或掩模文件:",
                es = "o archivo de máscara:"
            ));
            ui.add(
                egui::TextEdit::singleline(&mut panel.mask_path)
                    .desired_width(300.0)
                    .hint_text("optional /path/to/mask.json"),
            );
        });
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "APPROACH",
                    ja = "アプローチ",
                    it = "APPROCCIO",
                    zh = "入射方向",
                    es = "APROXIMACIÓN"
                ))
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
            ui.label(t!(
                language,
                en = "half-widths u/v cm:",
                ja = "半値幅 u/v cm:"
            ));
            ui.add(egui::TextEdit::singleline(&mut panel.half_width_u_cm).desired_width(50.0));
            ui.add(egui::TextEdit::singleline(&mut panel.half_width_v_cm).desired_width(50.0));
            ui.label(t!(language, en = "margin cm:", ja = "マージン cm:"));
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
                ui.strong(t!(
                    language,
                    en = "Position report",
                    ja = "位置レポート",
                    it = "Rapporto di posizione",
                    zh = "位置报告",
                    es = "Informe de posición"
                ));
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
                egui::RichText::new(t!(
                    language,
                    en = "ROTATE",
                    ja = "回転",
                    it = "RUOTA",
                    zh = "旋转",
                    es = "ROTAR"
                ))
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
            ui.label(t!(
                language,
                en = "degrees:",
                ja = "角度:",
                it = "gradi:",
                zh = "度:",
                es = "grados:"
            ));
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
                egui::RichText::new(t!(
                    language,
                    en = "SAVE",
                    ja = "保存",
                    it = "SALVA",
                    zh = "保存",
                    es = "GUARDAR"
                ))
                .small()
                .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.save_source_path)
                    .desired_width(260.0)
                    .hint_text("positioned-source.json"),
            );
            if ui
                .button(t!(
                    language,
                    en = "Write source",
                    ja = "線源を出力",
                    it = "Scrivi sorgente",
                    zh = "写出源",
                    es = "Escribir fuente"
                ))
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
                .button(t!(
                    language,
                    en = "Write report",
                    ja = "レポートを出力",
                    it = "Scrivi rapporto",
                    zh = "写出报告",
                    es = "Escribir informe"
                ))
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
        ui.heading(t!(
            language,
            en = "Run a subcommand",
            ja = "サブコマンドを実行",
            it = "Esegui un sottocomando",
            zh = "运行子命令",
            es = "Ejecutar un subcomando"
        ))
        .rect,
    );
    if cfg!(target_arch = "wasm32") {
        ui.label(t!(language, en = "Process execution requires the native build — the web inspector is read-only.", ja = "プロセス実行はネイティブ版のみ — Web インスペクタは読み取り専用です。",
                it = "L'esecuzione richiede la build nativa — l'inspector web è di sola lettura.",
                zh = "进程执行需要原生构建 — Web 检查器为只读。",
                es = "La ejecución de procesos requiere la build nativa — el inspector web es de solo lectura."));
    }
    let running = run.poll();
    if running {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(language, en = "program", ja = "プログラム",
                it = "programma",
                zh = "程序",
                es = "programa"));
            ui.add(
                egui::TextEdit::singleline(&mut run.program)
                    .desired_width(120.0)
                    .hint_text("openbnct"),
            );
            ui.label(t!(language, en = "args", ja = "引数",
                it = "argomenti",
                zh = "参数",
                es = "argumentos"));
            ui.add(
                egui::TextEdit::singleline(&mut run.args)
                    .desired_width(ui.available_width() - 220.0)
                    .hint_text("--help"),
            );
            ui.label(t!(language, en = "timeout s", ja = "タイムアウト秒",
                it = "timeout s",
                zh = "超时 秒",
                es = "timeout s"));
            ui.add(egui::TextEdit::singleline(&mut run.timeout_s).desired_width(50.0));
        });
        // Command presets — the discoverable surface for controls that
        // would otherwise need `--help` knowledge (like --threads).
        ui.horizontal_wrapped(|ui| {
            ui.label(t!(language, en = "presets:", ja = "プリセット:",
                it = "preset:",
                zh = "预设:",
                es = "predefinidos:"));
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
                    egui::Button::new(t!(language, en = "Run", ja = "実行",
                it = "Esegui",
                zh = "运行",
                es = "Ejecutar")),
                )
                .on_disabled_hover_text(if cfg!(target_arch = "wasm32") {
                    t!(language, en = "native build only", ja = "ネイティブ版のみ",
                it = "solo build nativa",
                zh = "仅限原生构建",
                es = "solo build nativa")
                } else {
                    t!(language, en = "a job is already running", ja = "ジョブ実行中",
                it = "un job è già in esecuzione",
                zh = "已有任务在运行",
                es = "ya hay un trabajo en ejecución")
                })
                .clicked()
            {
                run.start();
            }
            if ui
                .add_enabled(
                    running,
                    egui::Button::new(t!(language, en = "Cancel", ja = "キャンセル",
                it = "Annulla",
                zh = "取消",
                es = "Cancelar")),
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
        t!(
            language,
            en = "Exposure plan",
            ja = "照射計画",
            it = "Piano di irradiazione",
            zh = "照射计划",
            es = "Plan de irradiación"
        ),
        t!(
            language,
            en = "Structured multi-exposure schedules; every detected issue is reported, not just the first.",
            ja = "構造化された複数回照射スケジュール — 検出された問題は最初の一件だけでなく全件報告。"
        ),
    );

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "PLAN",
                    ja = "計画",
                    it = "PIANO",
                    zh = "计划",
                    es = "PLAN"
                ))
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
                .button(t!(
                    language,
                    en = "Load + diagnose",
                    ja = "読込・診断",
                    it = "Carica + diagnostica",
                    zh = "加载 + 诊断",
                    es = "Cargar + diagnosticar"
                ))
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
                egui::RichText::new(t!(
                    language,
                    en = "TABLE EXPORT",
                    ja = "テーブル出力",
                    it = "ESPORTA TABELLA",
                    zh = "导出表格",
                    es = "EXPORTAR TABLA"
                ))
                .small()
                .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.export_path)
                    .desired_width(420.0)
                    .hint_text("/path/to/schedule.csv or .xlsx"),
            );
            if ui
                .button(t!(
                    language,
                    en = "Export table",
                    ja = "テーブルを出力",
                    it = "Esporta tabella",
                    zh = "导出表格",
                    es = "Exportar tabla"
                ))
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
        ui.heading(t!(
            language,
            en = "Robustness report",
            ja = "ロバスト性レポート",
            it = "Rapporto di robustezza",
            zh = "稳健性报告",
            es = "Informe de robustez"
        ))
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
                en = "No robustness report loaded.",
                ja = "ロバスト性レポート未読込。",
                it = "Nessun rapporto di robustezza caricato.",
                zh = "未加载稳健性报告。",
                es = "Ningún informe de robustez cargado."
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

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "Scenario evaluation",
        ja = "シナリオ評価",
        it = "Valutazione scenari",
        zh = "场景评估",
        es = "Evaluación de escenarios"
    ));
    ui.label(
        "Drop a scenario-report .json — per-objective achieved bands across the \
         declared perturbation set, worst-scenario attribution, and violations.",
    );
    egui::Frame::group(ui.style()).show(ui, |ui| {
        if let Some(error) = &panel.scenario_report_error {
            ui.colored_label(theme.error, format!("scenario report rejected: {error}"));
        }
        let Some(report) = &panel.scenario_report else {
            ui.label("No scenario report loaded.");
            return;
        };
        ui.monospace(format!("{} · {}", report.id, report.qualification));
        ui.add_space(6.0);
        egui::Grid::new("scenario-bands")
            .striped(true)
            .show(ui, |ui| {
                for header in [
                    "objective",
                    "bound",
                    "nominal",
                    "min",
                    "max",
                    "mean",
                    "worst scenario",
                    "violations",
                ] {
                    ui.strong(header);
                }
                ui.end_row();
                for band in &report.bands {
                    ui.monospace(format!("{} · {}", band.kind, band.mask));
                    ui.monospace(format!("{:.4e}", band.bound));
                    ui.monospace(format!("{:.4e}", band.nominal_achieved));
                    ui.monospace(format!("{:.4e}", band.min_achieved));
                    ui.monospace(format!("{:.4e}", band.max_achieved));
                    ui.monospace(format!("{:.4e}", band.mean_achieved));
                    ui.monospace(&band.worst_scenario);
                    let n = band.violated_scenarios.len();
                    if n == 0 {
                        ui.colored_label(egui::Color32::from_rgb(80, 220, 140), "none");
                    } else {
                        ui.colored_label(
                            theme.error,
                            format!("{n}: {}", band.violated_scenarios.join(", ")),
                        );
                    }
                    ui.end_row();
                }
            });
        ui.add_space(4.0);
        ui.monospace(format!(
            "{} evaluations · provenance: {}",
            report.evaluations.len(),
            report.provenance_id
        ));
    });

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "PK irradiation schedule",
        ja = "PK 照射スケジュール",
        it = "Programma di irradiazione PK",
        zh = "PK 照射计划",
        es = "Programa de irradiación PK"
    ));
    ui.label(
        "Drop a pk-schedule .json — deliverable tumor dose per beam-on window \
         against the constant-concentration reference; the optimal window is flagged.",
    );
    egui::Frame::group(ui.style()).show(ui, |ui| {
        if let Some(error) = &panel.pk_schedule_error {
            ui.colored_label(theme.error, format!("pk schedule rejected: {error}"));
        }
        let Some(report) = &panel.pk_schedule else {
            ui.label("No pk schedule loaded.");
            return;
        };
        ui.monospace(format!(
            "case {} · {} · {} · tumor {} ({:?})",
            report.case_id,
            report.quantity,
            report.endpoint_unit,
            report.tumor_region,
            report.tumor_metric
        ));
        ui.add_space(6.0);
        egui::Grid::new("pk-schedule-windows")
            .striped(true)
            .show(ui, |ui| {
                for header in ["beam-on", "tumor dose", "static ref", "Δ", "limiting", ""] {
                    ui.strong(header);
                }
                ui.end_row();
                for (index, window) in report.windows.iter().enumerate() {
                    let optimal = report.optimal_window_index == Some(index);
                    let epoch_h = window.beam_on_epoch_s / 3600.0;
                    ui.monospace(format!("{epoch_h:.2} h"));
                    ui.monospace(
                        window
                            .tumor_dose
                            .map(|d| format!("{d:.4e}"))
                            .unwrap_or_else(|| "—".into()),
                    );
                    ui.monospace(
                        window
                            .tumor_dose_static
                            .map(|d| format!("{d:.4e}"))
                            .unwrap_or_else(|| "—".into()),
                    );
                    match (window.tumor_dose, window.tumor_dose_static) {
                        (Some(pk), Some(st)) if st > 0.0 => {
                            let gain = (pk - st) / st * 100.0;
                            let color = if gain >= 0.0 {
                                egui::Color32::from_rgb(80, 220, 140)
                            } else {
                                theme.warn_text
                            };
                            ui.colored_label(color, format!("{gain:+.1}%"));
                        }
                        _ => {
                            ui.monospace("—");
                        }
                    }
                    ui.monospace(
                        window
                            .limiting
                            .as_ref()
                            .map(|l| format!("{} {:.2e} s", l.region, l.max_time_s))
                            .unwrap_or_else(|| "unbounded".into()),
                    );
                    if optimal {
                        ui.colored_label(egui::Color32::from_rgb(80, 220, 140), "◀ optimal");
                    } else {
                        ui.label("");
                    }
                    ui.end_row();
                }
            });
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
            en = "selected quantity has no positive values to render",
            ja = "選択した物理量に描画可能な正の値がありません"
        ));
        return;
    }
    let Some(voxel) = panel.map.voxel else {
        return;
    };

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(
                language,
                en = "Quantity",
                ja = "物理量",
                it = "Quantità",
                zh = "物理量",
                es = "Cantidad"
            ));
            egui::ComboBox::from_id_salt("dose-map-quantity")
                .selected_text(panel.map.quantity.clone())
                .show_ui(ui, |ui| {
                    for (name, ..) in &rows {
                        ui.selectable_value(&mut panel.map.quantity, name.clone(), name.as_str());
                    }
                });
            ui.checkbox(
                &mut panel.map.log_scale,
                t!(
                    language,
                    en = "log scale",
                    ja = "対数スケール",
                    it = "scala log",
                    zh = "对数刻度",
                    es = "escala log"
                ),
            );
            ui.checkbox(
                &mut panel.map.contours,
                t!(language, en = "isodose 90/50/10%", ja = "等線量 90/50/10%"),
            );
            ui.add_enabled_ui(sigma_values.is_some(), |ui| {
                ui.checkbox(
                    &mut panel.map.sigma_view,
                    t!(
                        language,
                        en = "σ map",
                        ja = "σ マップ",
                        it = "mappa σ",
                        zh = "σ 图",
                        es = "mapa σ"
                    ),
                )
                .on_disabled_hover_text(t!(
                    language,
                    en = "this component carries no uncertainty field",
                    ja = "この成分は不確かさフィールドを持ちません",
                    it = "questa componente non ha campo di incertezza",
                    zh = "该成分不含不确定度场",
                    es = "este componente no tiene campo de incertidumbre"
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
            en = "no positive values along this profile",
            ja = "このプロファイル上に正の値がありません"
        ));
        return;
    }

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(
                language,
                en = "Axis",
                ja = "軸",
                it = "Asse",
                zh = "轴",
                es = "Eje"
            ));
            for (candidate, label) in [(0_usize, "X"), (1, "Y"), (2, "Z")] {
                ui.selectable_value(&mut panel.profile.axis, candidate, label);
            }
            ui.checkbox(&mut panel.profile.log_y, "log y");
            ui.separator();
            ui.label(t!(
                language,
                en = "Measurement overlay — drop a measurement-record .json",
                ja = "実測オーバーレイ — measurement-record .json をドロップ"
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
                if ui
                    .button(t!(
                        language,
                        en = "clear",
                        ja = "クリア",
                        it = "pulisci",
                        zh = "清除",
                        es = "limpiar"
                    ))
                    .clicked()
                {
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
                    en = "Line profile plot along the selected axis",
                    ja = "選択軸に沿ったラインプロファイル"
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
                    en = "drop a dose bundle here to replace B · drops elsewhere replace A",
                    ja = "ここに線量バンドルをドロップで B を置換 · ゾーン外は A を置換"
                )
            } else {
                t!(
                    language,
                    en = "drop a second dose bundle (B) here to diff against the loaded A",
                    ja = "2つ目の線量バンドル(B)をここにドロップして読込済み A と比較"
                )
            },
            egui::FontId::monospace(11.0),
            theme.text_dim,
        );
        if let Some(error) = &panel.compare_error {
            ui.colored_label(
                theme.error,
                format!(
                    "{}: {error}",
                    t!(language, en = "B rejected", ja = "B 拒否")
                ),
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
        if ui
            .button(t!(
                language,
                en = "clear B",
                ja = "B をクリア",
                it = "pulisci B",
                zh = "清除 B",
                es = "limpiar B"
            ))
            .clicked()
        {
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
                en = "no overlapping nonzero voxels to compare",
                ja = "比較可能な重なり合う非ゼロボクセルがありません"
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
                ui.label(t!(language, en = "Drop a dose-uncertainty-budget JSON (from `uq propagate`) to inspect the variance decomposition.", ja = "dose-uncertainty-budget JSON（`uq propagate` 出力）をドロップすると分散分解を表示します。"));
            }
            Some(budget) => {
                ui.label(format!(
                    "{}: {} · {} {:.4} Gy·cm² · {} ±{:.1}%",
                    t!(language, en = "component", ja = "成分",
                it = "componente",
                zh = "成分",
                es = "componente"),
                    budget.component,
                    t!(language, en = "response", ja = "応答",
                it = "risposta",
                zh = "响应",
                es = "respuesta"),
                    budget.response_integral,
                    t!(language, en = "total σ", ja = "総 σ",
                it = "σ totale",
                zh = "总 σ",
                es = "σ total"),
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
                        ui.monospace(t!(language, en = "parameter", ja = "パラメータ",
                it = "parametro",
                zh = "参数",
                es = "parámetro"));
                        ui.monospace(t!(language, en = "source", ja = "ソース",
                it = "sorgente",
                zh = "来源",
                es = "fuente"));
                        ui.monospace(t!(language, en = "σ param", ja = "パラメータ σ",
                it = "σ param",
                zh = "σ 参数",
                es = "σ param"));
                        ui.monospace(t!(language, en = "variance share", ja = "分散寄与",
                it = "quota di varianza",
                zh = "方差份额",
                es = "fracción de varianza"));
                        ui.end_row();
                        for entry in entries.iter().take(12) {
                            ui.monospace(&entry.parameter);
                            ui.monospace(&entry.source);
                            ui.monospace(if entry.std_dev.is_nan() {
                                "—".to_owned()
                            } else {
                                format!("{:.4}", entry.std_dev)
                            });
                            ui.monospace(format!("{:.1}%", entry.relative_contribution * 100.0));
                            ui.end_row();
                        }
                    });
                if budget.entries.len() > 12 {
                    ui.small(format!(
                        "… {} {}",
                        budget.entries.len() - 12,
                        t!(language, en = "more entries", ja = "件の追加項目",
                it = "altre voci",
                zh = "更多条目",
                es = "más entradas")
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
        ui.label(t!(language, en = "Derive the 478 keV ¹⁰B(n,α)⁷Li emission map (94% branch) — the source term for prompt-gamma imaging research.", ja = "478 keV ¹⁰B(n,α)⁷Li 放出マップを生成(94% 分枝)— 即発ガンマイメージング研究の線源項。"));
        ui.horizontal(|ui| {
            let enabled = physical.is_some();
            let response = ui
                .add_enabled(
                    enabled,
                    egui::Button::new(t!(language, en = "Derive prompt-gamma source", ja = "即発ガンマ線源を生成",
                it = "Deriva sorgente prompt-gamma",
                zh = "推导瞬发伽马源",
                es = "Derivar fuente prompt-gamma")),
                )
                .on_disabled_hover_text(t!(language, en = "requires a physical dose bundle with a boron component", ja = "ホウ素成分を含む物理線量バンドルが必要"));
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
                    .button(t!(language, en = "Export JSON", ja = "JSON を保存",
                it = "Esporta JSON",
                zh = "导出 JSON",
                es = "Exportar JSON"))
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
                format!("{}: {error}", t!(language, en = "prompt-gamma failed", ja = "生成失敗",
                it = "prompt-gamma fallito",
                zh = "瞬发伽马失败",
                es = "prompt-gamma falló")),
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
        t!(
            language,
            en = "Dose components",
            ja = "線量成分",
            it = "Componenti di dose",
            zh = "剂量成分",
            es = "Componentes de dosis"
        ),
        t!(
            language,
            en = "Physical and biological layers stay separate; only validated bundles render.",
            ja = "物理線量と生物学的線量は別レイヤー — 検証済みバンドルのみ描画。",
            it = "Gli strati fisici e biologici restano separati; solo i bundle validati si renderizzano.",
            zh = "物理层与生物层保持分离；只有验证过的束才会渲染。",
            es = "Las capas física y biológica permanecen separadas; solo se renderizan paquetes validados."
        ),
    );

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "DOSE BUNDLE",
                    ja = "線量バンドル",
                    it = "BUNDLE DI DOSE",
                    zh = "剂量束",
                    es = "PAQUETE DE DOSIS"
                ))
                .small()
                .strong(),
            );
            ui.add(
                egui::TextEdit::singleline(&mut panel.bundle_path)
                    .desired_width(460.0)
                    .hint_text("/path/to/dose-bundle.json — or drop it here"),
            );
            if ui
                .button(t!(
                    language,
                    en = "Load + validate",
                    ja = "読込・検証",
                    it = "Carica + valida",
                    zh = "加载 + 验证",
                    es = "Cargar + validar"
                ))
                .clicked()
            {
                panel.load_bundle();
            }
            if ui
                .button(t!(
                    language,
                    en = "example",
                    ja = "サンプル",
                    it = "esempio",
                    zh = "示例",
                    es = "ejemplo"
                ))
                .on_hover_text(t!(
                    language,
                    en = "load the bundled layered-head dose bundle",
                    ja = "同梱の layered-head 線量バンドルを読み込みます",
                    it = "carica il bundle di dose layered-head incluso",
                    zh = "加载内置的 layered-head 剂量束",
                    es = "carga el paquete de dosis layered-head incluido"
                ))
                .clicked()
            {
                match example_dose_bytes() {
                    Ok(bytes) => {
                        let _ = panel.load_bundle_bytes(bytes);
                    }
                    Err(error) => panel.bundle_error = Some(error),
                }
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
                ui.label(t!(
                    language,
                    en = "OpenBNCT does not render placeholder dose values. Load a validated \
                          physical or biological dose bundle — or load the bundled example.",
                    ja = "OpenBNCT はプレースホルダ線量を描画しません。検証済みの物理/生物線量 \
                          バンドルを読み込むか、同梱サンプルを開いてください。",
                    it = "OpenBNCT non mostra valori di dose segnaposto. Carica un bundle di dose \
                          fisico o biologico validato — oppure l'esempio incluso.",
                    zh = "OpenBNCT 不渲染占位剂量值。请加载经过验证的物理/生物剂量束，或打开内置示例。",
                    es = "OpenBNCT no muestra valores de dosis de marcador. Cargue un paquete de \
                          dosis físico o biológico validado — o el ejemplo incluido."
                ));
                if ui
                    .button(t!(
                        language,
                        en = "Load example bundle",
                        ja = "サンプルバンドルを読み込む",
                        it = "Carica bundle di esempio",
                        zh = "加载示例剂量束",
                        es = "Cargar paquete de ejemplo"
                    ))
                    .clicked()
                {
                    match example_dose_bytes() {
                        Ok(bytes) => {
                            let _ = panel.load_bundle_bytes(bytes);
                        }
                        Err(error) => panel.bundle_error = Some(error),
                    }
                }
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
        ui.heading(t!(
            language,
            en = "Dose map",
            ja = "線量マップ",
            it = "Mappa di dose",
            zh = "剂量图",
            es = "Mapa de dosis"
        ))
        .rect,
    );
    show_dose_map(ui, panel, language, theme);

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "Line profile",
        ja = "ラインプロファイル",
        it = "Profilo lineare",
        zh = "线剖面",
        es = "Perfil de línea"
    ));
    show_depth_profile(ui, panel, language, theme);

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "A/B compare",
        ja = "A/B 比較",
        it = "Confronto A/B",
        zh = "A/B 对比",
        es = "Comparación A/B"
    ));
    show_dose_compare(ui, panel, language, theme);
    ui.add_space(12.0);
    ui.heading(t!(
        language,
        en = "Prompt-gamma source",
        ja = "即発ガンマ線源",
        it = "Sorgente prompt-gamma",
        zh = "瞬发伽马源",
        es = "Fuente prompt-gamma"
    ));
    show_prompt_gamma(ui, panel, language, theme);
    ui.add_space(12.0);
    ui.heading(t!(
        language,
        en = "Uncertainty budget",
        ja = "不確かさバジェット",
        it = "Budget di incertezza",
        zh = "不确定度预算",
        es = "Presupuesto de incertidumbre"
    ));
    show_uncertainty_budget(ui, panel, language, theme);
    ui.add_space(12.0);
    ui.heading(t!(
        language,
        en = "SMK survival",
        ja = "SMK 生存率",
        it = "Sopravvivenza SMK",
        zh = "SMK 存活",
        es = "Supervivencia SMK"
    ));
    show_smk(ui, panel, language, theme);
    if panel.compare_zone != egui::Rect::NOTHING {
        tour_targets.set(TourTarget::CompareZone, panel.compare_zone);
    }

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "Region dose-volume histogram",
        ja = "領域線量-体積ヒストグラム",
        it = "Istogramma dose-volume di regione",
        zh = "区域剂量-体积直方图",
        es = "Histograma dosis-volumen de región"
    ));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(
                language,
                en = "Mask",
                ja = "マスク",
                it = "Maschera",
                zh = "掩模",
                es = "Máscara"
            ));
            ui.add(
                egui::TextEdit::singleline(&mut panel.mask_path)
                    .desired_width(360.0)
                    .hint_text("/path/to/region-mask.json"),
            );
            ui.label(t!(
                language,
                en = "Quantity",
                ja = "物理量",
                it = "Quantità",
                zh = "物理量",
                es = "Cantidad"
            ));
            ui.add(
                egui::TextEdit::singleline(&mut panel.quantity)
                    .desired_width(170.0)
                    .hint_text("physical_total"),
            );
            if ui
                .button(t!(
                    language,
                    en = "Compute DVH",
                    ja = "DVH を計算",
                    it = "Calcola DVH",
                    zh = "计算 DVH",
                    es = "Calcular DVH"
                ))
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
        en = "Region dose-volume metrics",
        ja = "領域線量-体積指標",
        it = "Metriche dose-volume di regione",
        zh = "区域剂量-体积指标",
        es = "Métricas dosis-volumen de región"
    ));
    ui.label(t!(language, en = "Same `RegionDoseMetrics::compute` path as `openbnct metrics` and `compute_metrics` in Python.", ja = "`openbnct metrics`・Python の `compute_metrics` と同じ `RegionDoseMetrics::compute` パス。"));
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
                .button(t!(
                    language,
                    en = "Compute metrics",
                    ja = "指標を計算",
                    it = "Calcola metriche",
                    zh = "计算指标",
                    es = "Calcular métricas"
                ))
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
                    .button(t!(
                        language,
                        en = "Write metrics",
                        ja = "指標を出力",
                        it = "Scrivi metriche",
                        zh = "写出指标",
                        es = "Escribir métricas"
                    ))
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
    ui.heading(t!(
        language,
        en = "NIfTI volumes",
        ja = "NIfTI ボリューム",
        it = "Volumi NIfTI",
        zh = "NIfTI 体数据",
        es = "Volúmenes NIfTI"
    ));
    ui.label("Same `openbnct-nifti` paths as `openbnct nifti` — sform-preferred RAS→LPS handling.");
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Volume");
            ui.add(
                egui::TextEdit::singleline(&mut nifti.input_path)
                    .desired_width(300.0)
                    .hint_text("/path/to/volume.nii[.gz]"),
            );
            if ui
                .button(t!(
                    language,
                    en = "Inspect",
                    ja = "検査",
                    it = "Ispeziona",
                    zh = "检查",
                    es = "Inspeccionar"
                ))
                .clicked()
            {
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
                    .button(t!(
                        language,
                        en = "Write mask",
                        ja = "マスクを出力",
                        it = "Scrivi maschera",
                        zh = "写出掩模",
                        es = "Escribir máscara"
                    ))
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
                if ui
                    .button(t!(
                        language,
                        en = "Resample",
                        ja = "リサンプル",
                        it = "Ricampiona",
                        zh = "重采样",
                        es = "Remuestrear"
                    ))
                    .clicked()
                {
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
            if ui
                .button(t!(
                    language,
                    en = "Export",
                    ja = "エクスポート",
                    it = "Esporta",
                    zh = "导出",
                    es = "Exportar"
                ))
                .clicked()
            {
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
                en = "Dose-volume histogram — cumulative volume fraction vs dose",
                ja = "線量-体積ヒストグラム — 線量対累積体積割合"
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

/// SMK-evaluation section — S(dose) curve for the stochastic population
/// vs its MK linearization, plus the isosurvival-RBE table.
fn show_smk(ui: &mut egui::Ui, panel: &mut DosePanel, language: Language, theme: Theme) {
    ui.label(
        "Drop an smk-evaluation .json — stochastic population survival against \
         the MK linearization, with the isosurvival RBE per dose level.",
    );
    egui::Frame::group(ui.style()).show(ui, |ui| {
        if let Some(error) = &panel.smk_error {
            ui.colored_label(theme.error, format!("smk evaluation rejected: {error}"));
        }
        let Some(report) = &panel.smk else {
            ui.label("No SMK evaluation loaded.");
            return;
        };
        ui.monospace(format!(
            "{} · α {:.4e} Gy⁻¹ · β {:.4e} Gy⁻² · anchor {:.4e} Gy",
            report.id,
            report.parameters.alpha_per_gy,
            report.parameters.beta_per_gy2,
            report.parameters.boron_dose_gy_at_mean_captures,
        ));
        ui.add_space(6.0);

        // Survival curve — SMK solid, MK dashed-ish (second color).
        let max_dose = report
            .points
            .iter()
            .map(|p| p.dose_gy)
            .fold(0.0_f64, f64::max);
        if max_dose > 0.0 {
            let (response, painter) = ui.allocate_painter(
                egui::vec2(ui.available_width(), 160.0),
                egui::Sense::focusable_noninteractive(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Other,
                    true,
                    t!(
                        language,
                        en = "SMK and MK survival fraction vs boron dose",
                        ja = "SMK・MK 生存率対ホウ素線量"
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
            let point_at = |dose: f64, survival: f64| {
                egui::pos2(
                    rect.left() + (dose / max_dose) as f32 * rect.width(),
                    rect.bottom() - (survival.clamp(0.0, 1.0) as f32) * rect.height(),
                )
            };
            let smk_points: Vec<egui::Pos2> = report
                .points
                .iter()
                .map(|p| point_at(p.dose_gy, p.smk_survival))
                .collect();
            let mk_points: Vec<egui::Pos2> = report
                .points
                .iter()
                .map(|p| point_at(p.dose_gy, p.mk_survival))
                .collect();
            painter.add(egui::Shape::line(
                smk_points,
                egui::Stroke::new(2.0, theme.brand),
            ));
            painter.add(egui::Shape::line(
                mk_points,
                egui::Stroke::new(1.5, theme.warn_text),
            ));
            painter.text(
                egui::pos2(rect.left() - 8.0, rect.top()),
                egui::Align2::RIGHT_CENTER,
                "S=1",
                egui::FontId::monospace(10.0),
                theme.text_dim,
            );
            painter.text(
                egui::pos2(rect.left() - 8.0, rect.bottom()),
                egui::Align2::RIGHT_CENTER,
                "0",
                egui::FontId::monospace(10.0),
                theme.text_dim,
            );
            painter.text(
                egui::pos2(rect.right(), rect.bottom() + 4.0),
                egui::Align2::RIGHT_TOP,
                format!("{max_dose:.3e} Gy"),
                egui::FontId::monospace(10.0),
                theme.text_dim,
            );
            ui.horizontal(|ui| {
                ui.colored_label(theme.brand, "— SMK ⟨e^(−az−bz²)⟩");
                ui.colored_label(theme.warn_text, "— MK linearization");
            });
        }

        ui.add_space(6.0);
        egui::Grid::new("smk-points").striped(true).show(ui, |ui| {
            for header in ["dose Gy", "S_smk", "S_mk", "isosurvival Gy", "RBE"] {
                ui.strong(header);
            }
            ui.end_row();
            for point in &report.points {
                ui.monospace(format!("{:.4e}", point.dose_gy));
                ui.monospace(format!("{:.4}", point.smk_survival));
                ui.monospace(format!("{:.4}", point.mk_survival));
                ui.monospace(
                    point
                        .isosurvival_reference_gy
                        .map(|d| format!("{d:.4e}"))
                        .unwrap_or_else(|| "—".into()),
                );
                ui.monospace(
                    point
                        .rbe
                        .map(|r| format!("{r:.3}"))
                        .unwrap_or_else(|| "—".into()),
                );
                ui.end_row();
            }
        });
        ui.add_space(4.0);
        ui.monospace(format!("note: {}", report.validity_note));
        ui.monospace(format!("provenance: {}", report.provenance_id));
    });
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
        t!(
            language,
            en = "Evidence",
            ja = "エビデンス",
            it = "Evidenza",
            zh = "证据",
            es = "Evidencia"
        ),
        t!(
            language,
            en = "Qualification is a chain of scoped claims, not one global green check.",
            ja = "適格性は一つの総合判定ではなく、範囲を限定した主張の連鎖。",
            it = "La qualificazione è una catena di affermazioni circoscritte, non un'unica spunta verde globale.",
            zh = "资质是一连串有范围限定的声明，而非一个全局绿勾。",
            es = "La cualificación es una cadena de afirmaciones delimitadas, no un solo visto verde global."
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
            t!(language, en = "Runtime geometry gate", ja = "実行時ジオメトリゲート",
                it = "Gate di geometria a runtime",
                zh = "运行时几何门控",
                es = "Puerta de geometría en ejecución"),
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
            t!(language, en = "Official OpenMC processed selection", ja = "公式 OpenMC 処理済みデータ選択"),
            GateState::Frozen,
            t!(language, en = "Case manifest binds cross_sections.xml, ten neutron tables, and five photon tables.", ja = "症例マニフェストが cross_sections.xml、中性子テーブル10件、光子テーブル5件を紐付け。"),
            Some(&manifest_hash),
            language,
        );
        show_evidence_row(
            ui,
            t!(language, en = "Controlled NJOY2016.78 execution", ja = "制御付き NJOY2016.78 実行"),
            GateState::Blocked,
            t!(language, en = "Preserved rejected evidence: 72 kinematic findings across four nuclides.", ja = "棄却された証拠を保存: 4核種にわたる72件の運動学的所見。"),
            Some(&execution_hash),
            language,
        );
        show_evidence_row(
            ui,
            t!(language, en = "OpenMC / NJOY MT 301 comparison", ja = "OpenMC / NJOY MT 301 比較"),
            GateState::Frozen,
            t!(language, en = "All ten curves agree within 4.9e-7; O-17/O-18 local fallback remains explicit.", ja = "全10曲線が 4.9e-7 以内で一致。O-17/O-18 のローカルフォールバックは明示的。"),
            Some(&comparison_hash),
            language,
        );
    });
    tour_targets.set(TourTarget::EvidenceLedger, ledger.response.rect);

    ui.add_space(12.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.heading(t!(language, en = "Qualification ceiling", ja = "適格範囲の上限",
                it = "Limite di qualificazione",
                zh = "资质上限",
                es = "Techo de cualificación"));
        ui.label(
            egui::RichText::new("synthetic_research_only")
                .monospace()
                .strong(),
        );
        ui.label(t!(language, en = "Acquisition identity, transport capability, response suitability, execution,              cross-code comparison, and experimental validation remain separate claims.", ja = "取得同一性・輸送能力・応答適格性・実行・クロスコード比較・実験的検証は個別の主張です。"));
    });

    ui.add_space(14.0);
    ui.heading(t!(
        language,
        en = "Exported evidence bundle",
        ja = "エクスポート済みエビデンス",
        it = "Bundle di evidenza esportato",
        zh = "导出的证据束",
        es = "Paquete de evidencia exportado"
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
                    en = "Verify manifest + hashes",
                    ja = "マニフェスト+ハッシュ検証",
                    it = "Verifica manifest + hash",
                    zh = "验证清单 + 哈希",
                    es = "Verificar manifiesto + hashes"
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
    ui.heading(t!(
        language,
        en = "Case",
        ja = "症例",
        it = "Caso",
        zh = "病例",
        es = "Caso"
    ));
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
        en = "Qualification: synthetic research only",
        ja = "適格範囲: 研究用途のみ",
        it = "Qualificazione: solo ricerca sintetica",
        zh = "资质：仅限合成研究",
        es = "Cualificación: solo investigación sintética"
    ));

    ui.separator();
    ui.heading(t!(
        language,
        en = "Crosshair",
        ja = "クロスヘア",
        it = "Crosshair",
        zh = "十字线",
        es = "Retícula"
    ));
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
    ui.strong(t!(
        language,
        en = "Window & overlays",
        ja = "ウィンドウ・重ね表示",
        it = "Finestra e overlay",
        zh = "窗宽与叠加",
        es = "Ventana y superposiciones"
    ));
    changed |= ui
        .add(
            egui::Slider::new(&mut display.window_center, -1_024.0..=3_071.0).text(t!(
                language,
                en = "level HU",
                ja = "レベル HU",
                it = "livello HU",
                zh = "窗位 HU",
                es = "nivel HU"
            )),
        )
        .changed();
    changed |= ui
        .add(
            egui::Slider::new(&mut display.window_width, 1.0..=4_096.0).text(t!(
                language,
                en = "width HU",
                ja = "幅 HU",
                it = "ampiezza HU",
                zh = "窗宽 HU",
                es = "ancho HU"
            )),
        )
        .changed();
    changed |= ui
        .add(
            egui::Slider::new(&mut display.overlay_opacity, 0.0..=1.0).text(t!(
                language,
                en = "ROI opacity",
                ja = "ROI 不透明度",
                it = "Opacità ROI",
                zh = "ROI 不透明度",
                es = "Opacidad ROI"
            )),
        )
        .changed();

    ui.separator();
    ui.strong(t!(
        language,
        en = "Crosshair position",
        ja = "クロスヘア位置",
        it = "Posizione crosshair",
        zh = "十字线位置",
        es = "Posición de retícula"
    ));
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
    ui.strong(t!(
        language,
        en = "Structures",
        ja = "輪郭構造",
        it = "Strutture",
        zh = "结构",
        es = "Estructuras"
    ));
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
    ui.strong(t!(
        language,
        en = "Dose overlay",
        ja = "線量オーバーレイ",
        it = "Overlay di dose",
        zh = "剂量叠加",
        es = "Superposición de dosis"
    ));
    ui.label(t!(
        language,
        en = "Load a dose bundle for this case to wash it over the image.",
        ja = "この症例の線量バンドルを読み込むと画像に重ねて表示します。",
        it = "Carica un bundle di dose per questo caso per sovrapporlo all'immagine.",
        zh = "加载此病例的剂量束以叠加在图像上。",
        es = "Cargue un paquete de dosis para este caso para superponerlo a la imagen."
    ));
    ui.horizontal(|ui| {
        ui.label(t!(
            language,
            en = "bundle",
            ja = "バンドル",
            it = "bundle",
            zh = "束",
            es = "paquete"
        ));
        ui.add(
            egui::TextEdit::singleline(&mut case.dose.path)
                .hint_text("dose-bundle.json")
                .desired_width(160.0),
        );
    });
    if ui
        .button(t!(
            language,
            en = "Load dose bundle",
            ja = "線量バンドルを読込",
            it = "Carica bundle di dose",
            zh = "加载剂量束",
            es = "Cargar paquete de dosis"
        ))
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
                t!(
                    language,
                    en = "show dose wash",
                    ja = "線量ウォッシュを表示",
                    it = "mostra wash di dose",
                    zh = "显示剂量叠加",
                    es = "mostrar lavado de dosis"
                ),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut case.dose.opacity, 0.0..=1.0).text(t!(
                    language,
                    en = "dose opacity",
                    ja = "線量不透明度",
                    it = "opacità dose",
                    zh = "剂量不透明度",
                    es = "opacidad de dosis"
                )),
            )
            .changed();
        changed |= ui
            .add(
                egui::Slider::new(&mut case.dose.threshold_percent, 0.0..=100.0).text(t!(
                    language,
                    en = "wash ≥ % of max",
                    ja = "ウォッシュ ≥ 最大値の%"
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
                    _ => "slice",
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
        ui.heading(t!(language, en = "No case loaded", ja = "症例が読み込まれていません",
                it = "Nessun caso caricato",
                zh = "未加载病例",
                es = "Ningún caso cargado"));
        ui.label(t!(language, en = "Generate the frozen benchmark from a terminal:", ja = "凍結ベンチマークをターミナルで生成:",
                it = "Genera il benchmark congelato da un terminale:",
                zh = "从终端生成冻结基准:",
                es = "Genere el benchmark congelado desde una terminal:"));
        ui.monospace("cargo run --bin openbnct -- benchmark generate /tmp/nf-bnct-001");
        ui.label(t!(language, en = "…or drop a DICOM study's files anywhere — one CT series + one RTSTRUCT.", ja = "…または DICOM スタディのファイルをドロップ — CT シリーズ1件 + RTSTRUCT 1件。",
                it = "…oppure trascina i file di uno studio DICOM ovunque — una serie CT + un RTSTRUCT.",
                zh = "…或将 DICOM 检查文件拖到任意位置 — 一个 CT 序列 + 一个 RTSTRUCT。",
                es = "…o suelte los archivos de un estudio DICOM en cualquier lugar — una serie CT + un RTSTRUCT."));
        ui.add_space(20.0);
        ui.label(t!(language, en = "DICOM and artifact-integrity gates run before any image is displayed.", ja = "画像表示の前に DICOM・完全性ゲートが実行されます。",
                it = "I gate DICOM e di integrità degli artefatti girano prima di qualsiasi visualizzazione.",
                zh = "在任何图像显示之前运行 DICOM 与工件完整性门控。",
                es = "Las puertas DICOM y de integridad de artefactos se ejecutan antes de mostrar cualquier imagen."));
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
        assert_eq!(labels.len(), 7);
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
    fn json_drops_route_recognized_schemas_to_the_inspector() {
        let route = |schema: &str| {
            let bytes = serde_json::json!({"schema_version": schema, "id": "t"})
                .to_string()
                .into_bytes();
            classify_dropped_json(&bytes)
        };
        assert_eq!(
            route("openbnct.scenario-report/0.1.0"),
            DropTarget::ScenarioReport
        );
        assert_eq!(route("openbnct.pk-schedule/0.1.0"), DropTarget::PkSchedule);
        assert_eq!(route("openbnct.smk-evaluation/0.1.0"), DropTarget::Smk);
        // Schemas without dedicated panels land in the inspector.
        assert_eq!(
            route("openbnct.cell-microdosimetry/0.1.0"),
            DropTarget::Inspector
        );
        assert_eq!(
            route("openbnct.pg-reconstruction/0.1.0"),
            DropTarget::Inspector
        );
        // Paneled and dose-bundle schemas keep their routes.
        assert_eq!(
            route("openbnct.physical-dose-bundle/0.2.0"),
            DropTarget::DoseBundle
        );
        assert_eq!(
            route("openbnct.plan-robustness/0.1.0"),
            DropTarget::Robustness
        );
        // Unrecognized non-project JSON still tries the dose loader.
        assert_eq!(route("com.example.other/1.0"), DropTarget::DoseBundle);
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
                    "",
                );
            });
            output.textures_delta.clear();
        }
    }
}
