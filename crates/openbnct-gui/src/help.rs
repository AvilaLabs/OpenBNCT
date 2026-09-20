// SPDX-License-Identifier: Apache-2.0

use eframe::egui;

use crate::i18n::Language;
use crate::t;

const TARGET_COUNT: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HelpWorkspace {
    Overview,
    Geometry,
    Transport,
    Plan,
    Dose,
    Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TourTarget {
    Brand,
    HelpButton,
    CaseLoader,
    WorkspaceNavigation,
    OverviewGates,
    GeometryControls,
    GeometryViews,
    TransportGates,
    TransportActions,
    EvidenceLedger,
    DoseMap,
    CompareZone,
    SpectrumPanel,
    RunPanel,
    RobustnessCards,
}

impl TourTarget {
    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Default, Clone)]
pub(crate) struct TourTargets {
    rects: [Option<egui::Rect>; TARGET_COUNT],
}

impl TourTargets {
    pub(crate) fn set(&mut self, target: TourTarget, rect: egui::Rect) {
        self.rects[target.index()] = Some(rect);
    }

    fn get(&self, target: TourTarget) -> Option<egui::Rect> {
        self.rects[target.index()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuideKind {
    QuickStart,
    Geometry,
    Readiness,
    DoseMaps,
    Compare,
    SourceAndRun,
}

impl GuideKind {
    const ALL: [Self; 6] = [
        Self::QuickStart,
        Self::Geometry,
        Self::DoseMaps,
        Self::Compare,
        Self::SourceAndRun,
        Self::Readiness,
    ];

    fn title(self, language: Language) -> &'static str {
        match (language, self) {
            (Language::English, Self::QuickStart) => "Quick start",
            (Language::English, Self::Geometry) => "Inspect geometry",
            (Language::English, Self::Readiness) => "Readiness & evidence",
            (Language::English, Self::DoseMaps) => "Read dose & σ maps",
            (Language::English, Self::Compare) => "Compare two bundles",
            (Language::English, Self::SourceAndRun) => "Spectra & running jobs",
            (Language::Japanese, Self::QuickStart) => "クイックスタート",
            (Language::Japanese, Self::Geometry) => "ジオメトリの確認",
            (Language::Japanese, Self::Readiness) => "レディネスとエビデンス",
            (Language::Japanese, Self::DoseMaps) => "線量・σマップの読み方",
            (Language::Japanese, Self::Compare) => "2つのバンドルを比較",
            (Language::Japanese, Self::SourceAndRun) => "スペクトルとジョブ実行",
        }
    }

    fn description(self, language: Language) -> &'static str {
        match (language, self) {
            (Language::English, Self::QuickStart) => {
                "Learn the shell, load gate, workspaces, and status language."
            }
            (Language::English, Self::Geometry) => {
                "Review linked DICOM views, display controls, and the crosshair."
            }
            (Language::English, Self::Readiness) => {
                "Trace why a capability is frozen, blocked, pending, or verified."
            }
            (Language::English, Self::DoseMaps) => {
                "Drop a dose bundle, read component maps, contours, σ maps, and profiles."
            }
            (Language::English, Self::Compare) => {
                "A/B two dose bundles — ratio slices, statistics, and provenance."
            }
            (Language::English, Self::SourceAndRun) => {
                "Inspect a beam spectrum on web or native; run bounded commands on desktop."
            }
            (Language::Japanese, Self::QuickStart) => {
                "画面構成、読込ゲート、ワークスペース、ステータス表記を学びます。"
            }
            (Language::Japanese, Self::Geometry) => {
                "連動 DICOM ビュー、表示コントロール、クロスヘアを確認します。"
            }
            (Language::Japanese, Self::Readiness) => {
                "機能が「凍結」「ブロック」「保留」「検証済み」になる理由を追跡します。"
            }
            (Language::Japanese, Self::DoseMaps) => {
                "線量バンドルをドロップし、成分マップ・等高線・σマップ・プロファイルを読みます。"
            }
            (Language::Japanese, Self::Compare) => {
                "2つの線量バンドルを A/B 比較 — 比スライス・統計・プロベナンス。"
            }
            (Language::Japanese, Self::SourceAndRun) => {
                "ビームスペクトルを確認(Web/ネイティブ共通)。デスクトップでは範囲制限付きコマンド実行。"
            }
        }
    }

    fn steps(self, language: Language) -> &'static [TourStep] {
        match (language, self) {
            (Language::English, Self::QuickStart) => &QUICK_START_STEPS,
            (Language::English, Self::Geometry) => &GEOMETRY_STEPS,
            (Language::English, Self::Readiness) => &READINESS_STEPS,
            (Language::English, Self::DoseMaps) => &DOSE_MAP_STEPS,
            (Language::English, Self::Compare) => &COMPARE_STEPS,
            (Language::English, Self::SourceAndRun) => &SOURCE_RUN_STEPS,
            (Language::Japanese, Self::QuickStart) => &QUICK_START_STEPS_JA,
            (Language::Japanese, Self::Geometry) => &GEOMETRY_STEPS_JA,
            (Language::Japanese, Self::Readiness) => &READINESS_STEPS_JA,
            (Language::Japanese, Self::DoseMaps) => &DOSE_MAP_STEPS_JA,
            (Language::Japanese, Self::Compare) => &COMPARE_STEPS_JA,
            (Language::Japanese, Self::SourceAndRun) => &SOURCE_RUN_STEPS_JA,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TourStep {
    target: TourTarget,
    workspace: Option<HelpWorkspace>,
    title: &'static str,
    instruction: &'static str,
}

const QUICK_START_STEPS: [TourStep; 5] = [
    TourStep {
        target: TourTarget::Brand,
        workspace: None,
        title: "Welcome to OpenBNCT",
        instruction: "This Avila Labs workbench keeps geometry, transport, dose, and evidence in one research interface without presenting unfinished science as a result.",
    },
    TourStep {
        target: TourTarget::CaseLoader,
        workspace: None,
        title: "Load and verify a case",
        instruction: "Enter an NF-BNCT-001 directory and press Load & verify. Artifact integrity and DICOM geometry must pass before images are shown.",
    },
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: None,
        title: "Move between workspaces",
        instruction: "Use these six workspaces to inspect the same case from geometry through evidence. A workspace can exist even when its scientific result is not ready.",
    },
    TourStep {
        target: TourTarget::OverviewGates,
        workspace: Some(HelpWorkspace::Overview),
        title: "Read the gates first",
        instruction: "Every status is scoped. Verified, frozen, blocked, pending, and input required mean different things; a green geometry gate never promotes transport or dose.",
    },
    TourStep {
        target: TourTarget::HelpButton,
        workspace: None,
        title: "Help is always here",
        instruction: "Open the Help menu for contextual guidance, bundled answers, or another guided tour. Press Escape at any point to leave a tour.",
    },
];

const GEOMETRY_STEPS: [TourStep; 4] = [
    TourStep {
        target: TourTarget::CaseLoader,
        workspace: None,
        title: "Start with verified input",
        instruction: "Geometry tools only receive a case after the DICOM and artifact-integrity boundary accepts it.",
    },
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: Some(HelpWorkspace::Geometry),
        title: "Open Geometry",
        instruction: "The Geometry workspace is the patient-space truth view used before any transport preparation.",
    },
    TourStep {
        target: TourTarget::GeometryControls,
        workspace: Some(HelpWorkspace::Geometry),
        title: "Control the display",
        instruction: "Adjust CT level and width, ROI opacity, the linked voxel, and individual structure visibility here. These controls change display only, never source data.",
    },
    TourStep {
        target: TourTarget::GeometryViews,
        workspace: Some(HelpWorkspace::Geometry),
        title: "Use the linked views",
        instruction: "Click or drag in axial, coronal, or sagittal view. All three views share one voxel crosshair and retain explicit patient-side orientation labels.",
    },
];

const READINESS_STEPS: [TourStep; 4] = [
    TourStep {
        target: TourTarget::OverviewGates,
        workspace: Some(HelpWorkspace::Overview),
        title: "Begin at the readiness summary",
        instruction: "These rows separate runtime verification from frozen project evidence and unresolved scientific work.",
    },
    TourStep {
        target: TourTarget::TransportGates,
        workspace: Some(HelpWorkspace::Transport),
        title: "Trace the transport gate chain",
        instruction: "Transport can advance only in order. The current O-17/O-18 response blocker prevents the controlled-run gate from being promoted.",
    },
    TourStep {
        target: TourTarget::TransportActions,
        workspace: Some(HelpWorkspace::Transport),
        title: "Capabilities control actions",
        instruction: "Disabled buttons are deliberate. They become available only when the adapter advertises a tested capability and its upstream evidence gates pass.",
    },
    TourStep {
        target: TourTarget::EvidenceLedger,
        workspace: Some(HelpWorkspace::Evidence),
        title: "Inspect the evidence ledger",
        instruction: "Each row names one bounded claim and, where available, derives a short identifier from the frozen evidence bytes. There is no global all-clear badge.",
    },
];

const DOSE_MAP_STEPS: [TourStep; 5] = [
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: Some(HelpWorkspace::Dose),
        title: "Open Dose components",
        instruction: "Drop a physical or biological dose-bundle JSON anywhere in the window — the bundle's own grid geometry drives the viewer, so no case directory is needed on web.",
    },
    TourStep {
        target: TourTarget::DoseMap,
        workspace: Some(HelpWorkspace::Dose),
        title: "Read the component map",
        instruction: "Pick a component (boron, nitrogen, hydrogen, gamma, total) and quantity. Click any of the three planes to move the shared crosshair — all panes stay linked.",
    },
    TourStep {
        target: TourTarget::DoseMap,
        workspace: Some(HelpWorkspace::Dose),
        title: "Shape the display",
        instruction: "Log scale reveals wide dynamic ranges; isodose contours mark 90/50/10% of peak. The σ-map toggle swaps the wash to absolute standard uncertainty where the component carries one.",
    },
    TourStep {
        target: TourTarget::DoseMap,
        workspace: Some(HelpWorkspace::Dose),
        title: "Quantify along a line",
        instruction: "Below the map, the line-profile section draws the selected component along an axis through the crosshair — drop a measurement-record JSON to overlay measured points on the same axis.",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "Cards carry the summary",
        instruction: "Above the map, each component card reports min/max/mean and mean relative 1σ. Below, the compare drop zone accepts a second bundle for A/B diffing — the next tour walks through it.",
    },
];

const COMPARE_STEPS: [TourStep; 4] = [
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: Some(HelpWorkspace::Dose),
        title: "Load artifact A",
        instruction: "Drop the first dose bundle anywhere — it becomes the reference (A). Its case id, SHA-256, and provenance binding appear in the diff header.",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "Drop B on the compare zone",
        instruction: "Drop a second bundle inside the marked zone. Dropping anywhere else replaces A instead — the zone is what makes it a comparison.",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "Read the ratio map",
        instruction: "Tri-planar B/A ratio renders on the shared crosshair: blue under 0.5×, white at unity, red over 2×, dark where the ratio is undefined. Statistics report mean ratio, max deviation, and the ±5% agreement fraction.",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "Check provenance",
        instruction: "The diff header prints both artifacts' content bindings — if geometry or provenance don't line up, the comparison refuses rather than guessing. That refusal is the verification story.",
    },
];

const SOURCE_RUN_STEPS: [TourStep; 4] = [
    TourStep {
        target: TourTarget::SpectrumPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "Drop a source artifact",
        instruction: "A beam-description or fixed-source-definition JSON renders its energy spectrum log-log with thermal/epithermal/fast region shading and per-region fractions.",
    },
    TourStep {
        target: TourTarget::SpectrumPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "Read the regions",
        instruction: "The shaded bands mark the BNCT-relevant ranges; fractions under the plot quantify how much source strength sits in each — the number that matters for epithermal quality.",
    },
    TourStep {
        target: TourTarget::RunPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "Run a bounded command (native)",
        instruction: "On the desktop build, type a program (e.g. openbnct) and arguments (e.g. dicom verify <case-dir>), set a timeout, and Run. Output streams live; Cancel kills and reaps the child.",
    },
    TourStep {
        target: TourTarget::RunPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "Web runs inspection only",
        instruction: "In the browser this panel is intentionally inert — no processes exist there. The web build inspects and verifies artifacts; execution stays on the desktop binary.",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ActiveTour {
    guide: GuideKind,
    step_index: usize,
}

#[derive(Debug, Default)]
pub(crate) struct GuidedHelp {
    center_open: bool,
    active_tour: Option<ActiveTour>,
    question: String,
    answer_index: Option<usize>,
}

impl GuidedHelp {
    pub(crate) fn toggle_center(&mut self) {
        self.center_open = !self.center_open;
        if self.center_open {
            self.active_tour = None;
        }
    }

    pub(crate) fn requested_workspace(&self, language: Language) -> Option<HelpWorkspace> {
        self.active_step(language).and_then(|step| step.workspace)
    }

    pub(crate) fn show_center(
        &mut self,
        context: &egui::Context,
        workspace: HelpWorkspace,
        case_loaded: bool,
        language: Language,
        theme: crate::Theme,
    ) {
        if !self.center_open || self.active_tour.is_some() {
            return;
        }

        let mut open = true;
        let mut guide_to_start = None;
        egui::Window::new(t!(language, "Help & guided tours", "ヘルプとガイドツアー"))
            .id(egui::Id::new("openbnct-help-center"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(430.0)
            .min_width(360.0)
            .max_width(560.0)
            .constrain_to(context.content_rect())
            .show(context, |ui| {
                ui.label(
                    egui::RichText::new(t!(
                        language,
                        "CONTEXTUAL HELP",
                        "コンテキストヘルプ"
                    ))
                    .small()
                    .strong()
                    .color(theme.brand),
                );
                let (title, body) = workspace_help(workspace, language);
                ui.heading(title);
                ui.label(body);

                ui.add_space(12.0);
                ui.separator();
                ui.heading(t!(language, "Walk me through it", "ガイドツアー"));
                for guide in GuideKind::ALL {
                    let enabled = guide != GuideKind::Geometry || case_loaded;
                    let response = ui.add_enabled(
                        enabled,
                        egui::Button::new(
                            egui::RichText::new(guide.title(language)).strong(),
                        )
                        .min_size(egui::vec2(ui.available_width(), 34.0)),
                    );
                    if response.clicked() {
                        guide_to_start = Some(guide);
                    }
                    ui.small(guide.description(language));
                    if guide == GuideKind::Geometry && !case_loaded {
                        ui.colored_label(
                            theme.warn_text,
                            t!(
                                language,
                                "Load a verified case to enable this tour.",
                                "症例を読み込むとこのツアーが有効になります。"
                            ),
                        );
                    }
                    ui.add_space(5.0);
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading(t!(language, "Use cases", "ユースケース"));
                ui.small(t!(
                    language,
                    "Concrete recipes — each names its inputs and what to check.",
                    "具体的な手順レシピ — 入力と確認ポイントを明記。"
                ));
                for recipe in use_cases(language) {
                    ui.collapsing(egui::RichText::new(recipe.title).strong(), |ui| {
                        ui.small(
                            egui::RichText::new(format!(
                                "{}: {}",
                                t!(language, "Workspace", "ワークスペース"),
                                recipe.workspace
                            ))
                            .color(theme.text_dim),
                        );
                        ui.label(recipe.goal);
                        for (index, step) in recipe.steps.iter().enumerate() {
                            ui.label(format!("{}. {step}", index + 1));
                        }
                        if let Some(watch) = recipe.watch_for {
                            ui.colored_label(
                                theme.warn_text,
                                format!(
                                    "{}: {watch}",
                                    t!(language, "Watch", "注意")
                                ),
                            );
                        }
                    });
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading(t!(language, "Ask bundled help", "ヘルプ検索"));
                ui.small(t!(
                    language,
                    "Answers stay on this device and come from reviewed, bundled guidance.                      No external model or service is contacted.",
                    "回答はすべて同梱のレビュー済みガイドから生成 — 外部サービスは不使用。"
                ));
                let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.question)
                        .hint_text(t!(
                            language,
                            "Why is transport disabled?",
                            "なぜ輸送実行は無効ですか?"
                        ))
                        .desired_width(f32::INFINITY),
                );
                if response.changed() || (response.has_focus() && enter_pressed) {
                    self.answer_index = best_answer(&self.question, language);
                }

                if self.question.trim().is_empty() {
                    ui.small(t!(
                        language,
                        "Try one of these common questions:",
                        "よくある質問から試してください:"
                    ));
                    for (index, entry) in faq(language).iter().enumerate().take(4) {
                        if ui.link(entry.question).clicked() {
                            self.question = entry.question.to_owned();
                            self.answer_index = Some(index);
                        }
                    }
                } else if let Some(index) = self.answer_index {
                    show_answer(ui, faq(language)[index]);
                } else {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.strong(t!(
                            language,
                            "No bundled answer matched that question yet.",
                            "その質問に一致する同梱回答はまだありません。"
                        ));
                        ui.label(t!(
                            language,
                            "Try asking about loading a case, geometry, status gates, OpenMC,                              dose, clinical use, or Python installation.",
                            "症例の読み込み、ジオメトリ、ステータスゲート、OpenMC、線量、                             臨床利用、Python インストールなどで試してください。"
                        ));
                    });
                }
            });

        self.center_open = open;
        if let Some(guide) = guide_to_start {
            self.active_tour = Some(ActiveTour {
                guide,
                step_index: 0,
            });
            self.center_open = false;
        }
    }

    pub(crate) fn show_tour(
        &mut self,
        context: &egui::Context,
        targets: &TourTargets,
        language: Language,
        theme: crate::Theme,
    ) {
        let Some(active) = self.active_tour else {
            return;
        };
        let steps = active.guide.steps(language);
        let step = steps[active.step_index];

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.active_tour = None;
            return;
        }

        let screen = context.content_rect();
        let fallback = egui::Rect::from_center_size(
            screen.center(),
            egui::vec2(screen.width().min(520.0), screen.height().min(260.0)),
        );
        let spotlight = targets
            .get(step.target)
            .unwrap_or(fallback)
            .expand(8.0)
            .intersect(screen);

        let painter = context.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("openbnct-tour-dimmer"),
        ));
        let dim_color = egui::Color32::from_black_alpha(205);
        for rect in dim_rectangles(screen, spotlight) {
            if rect.is_positive() {
                painter.rect_filled(rect, 0.0, dim_color);
            }
        }
        painter.rect_stroke(
            spotlight,
            9.0,
            egui::Stroke::new(3.0, theme.brand),
            egui::StrokeKind::Outside,
        );

        let callout_position = callout_position(screen, spotlight);
        let mut go_back = false;
        let mut go_next = false;
        let mut close = false;
        egui::Area::new(egui::Id::new("openbnct-tour-callout"))
            .order(egui::Order::Tooltip)
            .fixed_pos(callout_position)
            .constrain_to(screen)
            .show(context, |ui| {
                egui::Frame::new()
                    .fill(theme.card_fill)
                    .stroke(egui::Stroke::new(
                        1.0,
                        theme.brand,
                    ))
                    .corner_radius(10)
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 2,
                        color: egui::Color32::from_black_alpha(150),
                    })
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(350.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(active.guide.title(language).to_uppercase())
                                    .small()
                                    .strong()
                                    .color(theme.brand),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    close = ui
                                        .button(egui::RichText::new("×").size(18.0))
                                        .on_hover_text(t!(
                                            language,
                                            "End tour",
                                            "ツアー終了"
                                        ))
                                        .clicked();
                                },
                            );
                        });
                        ui.add(
                            egui::ProgressBar::new(
                                (active.step_index + 1) as f32 / steps.len() as f32,
                            )
                            .desired_width(ui.available_width())
                            .show_percentage(),
                        );
                        ui.heading(step.title);
                        ui.label(step.instruction);
                        ui.add_space(6.0);
                        ui.small(t!(
                            language,
                            "The highlighted controls remain live. Use them now if useful, then continue.",
                            "ハイライト中の操作は有効です。試してから次へ進めます。"
                        ));
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            go_back = ui
                                .add_enabled(
                                    active.step_index > 0,
                                    egui::Button::new(t!(language, "Back", "戻る")),
                                )
                                .clicked();
                            ui.label(format!(
                                "{} {} / {}",
                                t!(language, "Step", "ステップ"),
                                active.step_index + 1,
                                steps.len()
                            ));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    go_next = ui
                                        .button(if active.step_index + 1 == steps.len() {
                                            t!(language, "Finish", "完了")
                                        } else {
                                            t!(language, "Next", "次へ")
                                        })
                                        .clicked();
                                },
                            );
                        });
                    });
            });

        go_back |= context.input(|input| input.key_pressed(egui::Key::ArrowLeft));
        go_next |= context.input(|input| input.key_pressed(egui::Key::ArrowRight));
        if close {
            self.active_tour = None;
        } else if go_back && active.step_index > 0 {
            self.active_tour = Some(ActiveTour {
                step_index: active.step_index - 1,
                ..active
            });
        } else if go_next {
            if active.step_index + 1 == steps.len() {
                self.active_tour = None;
            } else {
                self.active_tour = Some(ActiveTour {
                    step_index: active.step_index + 1,
                    ..active
                });
            }
        }
    }

    fn active_step(&self, language: Language) -> Option<TourStep> {
        let active = self.active_tour?;
        active.guide.steps(language).get(active.step_index).copied()
    }
}

fn workspace_help(workspace: HelpWorkspace, language: Language) -> (&'static str, &'static str) {
    match (language, workspace) {
        (Language::English, HelpWorkspace::Overview) => (
            "Research overview",
            "Start here to understand the current scientific ceiling. Each readiness card is a scoped claim, not a project-wide pass or fail. Drop a case archive or bundle to begin.",
        ),
        (Language::English, HelpWorkspace::Geometry) => (
            "Geometry",
            "Inspect the accepted DICOM geometry in linked patient-space views. Click any plane to move the shared crosshair; ROI checkboxes and CT level/width change display only, never source data.",
        ),
        (Language::English, HelpWorkspace::Transport) => (
            "Transport",
            "Follow the ordered gate chain and backend capability flags, drop a beam-description JSON for the spectrum viewer, or run a bounded CLI command (desktop only).",
        ),
        (Language::English, HelpWorkspace::Plan) => (
            "Exposure plan",
            "Drop an exposure plan to inspect every detected issue, round-trip the schedule through CSV/XLSX, or read a plan-robustness report's violation probabilities. Accumulation runs through the CLI or Python.",
        ),
        (Language::English, HelpWorkspace::Dose) => (
            "Dose components",
            "Drop a dose bundle for component cards, the tri-planar map (log scale, contours, σ toggle), line profiles with measurement overlays, and the A/B compare zone. Physical and biological layers never merge.",
        ),
        (Language::English, HelpWorkspace::Evidence) => (
            "Evidence",
            "Inspect evidence one bounded claim at a time. Frozen project artifacts and a verified local run are intentionally different states; content bindings print alongside.",
        ),
        (Language::Japanese, HelpWorkspace::Overview) => (
            "研究概要",
            "まずここで現在の科学的上限を把握してください。各レディネスカードは範囲を限定した主張であり、プロジェクト全体の合否ではありません。症例アーカイブやバンドルをドロップして開始します。",
        ),
        (Language::Japanese, HelpWorkspace::Geometry) => (
            "ジオメトリ",
            "受理された DICOM ジオメトリを連動する患者空間ビューで確認します。任意の面をクリックすると共有クロスヘアが移動します。ROI チェックボックスと CT レベル/幅は表示のみを変更し、元データは変更しません。",
        ),
        (Language::Japanese, HelpWorkspace::Transport) => (
            "輸送",
            "順序付きゲートチェーンとバックエンド能力フラグを確認し、beam-description JSON をドロップしてスペクトルを表示するか、範囲制限付き CLI コマンドを実行します(デスクトップのみ)。",
        ),
        (Language::Japanese, HelpWorkspace::Plan) => (
            "照射計画",
            "照射計画をドロップして検出された全問題を確認したり、CSV/XLSX でスケジュールを往復変換したり、plan-robustness レポートの違反確率を読み取れます。線量積算は CLI または Python から実行します。",
        ),
        (Language::Japanese, HelpWorkspace::Dose) => (
            "線量成分",
            "線量バンドルをドロップすると、成分カード・3面マップ(対数スケール・等高線・σ切替)・測定値オーバーレイ付きラインプロファイル・A/B 比較ゾーンが使えます。物理層と生物学的層は絶対に混ざりません。",
        ),
        (Language::Japanese, HelpWorkspace::Evidence) => (
            "エビデンス",
            "範囲を限定した主張を一件ずつ確認します。凍結済みプロジェクト成果物と検証済みローカル実行は意図的に異なる状態であり、コンテンツのハッシュ紐付けも併記されます。",
        ),
    }
}

fn faq(language: Language) -> &'static [FaqEntry] {
    match language {
        Language::English => &FAQ,
        Language::Japanese => &FAQ_JA,
    }
}

fn use_cases(language: Language) -> &'static [UseCase] {
    match language {
        Language::English => &USE_CASES,
        Language::Japanese => &USE_CASES_JA,
    }
}

#[derive(Debug, Clone, Copy)]
struct FaqEntry {
    question: &'static str,
    keywords: &'static [&'static str],
    answer: &'static str,
}

const FAQ: [FaqEntry; 19] = [
    FaqEntry {
        question: "What is a case, and how do I get one?",
        keywords: &["case", "what", "get", "obtain", "have", "start", "first"],
        answer: "A case is the spatial substrate everything anchors to: a CT series for patient geometry plus an RTSTRUCT for contours, hash-bound so downstream artifacts can prove they belong to it. Two kinds load: the frozen NF-BNCT-001 benchmark (generate it with `openbnct dicom generate` — nobody needs external files) and any research study you already have (a CT series + RTSTRUCT export from a TPS or archive). Research imports are verified against themselves at load — parsed strictly, hash-bound — rather than against a fixture.",
    },
    FaqEntry {
        question: "How do I load a case?",
        keywords: &["load", "case", "directory", "dicom", "generate"],
        answer: "NF-BNCT-001: generate it with the CLI, enter its directory in the CASE field, and press Load & verify. A research study: File → \"Import DICOM study…\" and pick the export folder, or just drop the DICOM files — members are bucketed by SOP class, then Import as research case finishes the load.",
    },
    FaqEntry {
        question: "Why are the transport buttons disabled?",
        keywords: &[
            "transport",
            "disabled",
            "button",
            "openmc",
            "execute",
            "prepare",
        ],
        answer: "The controlled transport path is not qualified yet. The O-17/O-18 transported-photon response treatment still requires review, so interactive prepare and execute stay disabled even though the OpenMC adapter advertises those capabilities to the CLI.",
    },
    FaqEntry {
        question: "Does OpenBNCT depend completely on OpenMC?",
        keywords: &["depend", "openmc", "backend", "neutral", "mcnp", "phits"],
        answer: "No. OpenMC is the first backend, while case, physical-dose, uncertainty, and evidence contracts remain transport-neutral. Future adapters or imported results can use those contracts without reimplementing the GUI.",
    },
    FaqEntry {
        question: "Where are the dose values and heat maps?",
        keywords: &["dose", "heat", "map", "dvh", "value", "result"],
        answer: "They appear only after loading a validated physical-dose or biological bundle in the Dose workspace. OpenBNCT never renders placeholder dose values; biological weights stay a separate layer and are never clinical quantities.",
    },
    FaqEntry {
        question: "What do the status labels mean?",
        keywords: &[
            "status", "verified", "frozen", "blocked", "pending", "input",
        ],
        answer: "Verified means a runtime gate passed; frozen means a checked project artifact exists; blocked names a known unresolved requirement; pending is not yet executed; and input required means the local gate cannot run without a case.",
    },
    FaqEntry {
        question: "Can I install OpenBNCT with pip?",
        keywords: &["pip", "python", "pypi", "install", "maturin", "pyo3"],
        answer: "That is the committed distribution direction, but no PyPI release exists yet. The Python package will use PyO3 and maturin to call the same Rust core; it will not contain a second Python dose engine.",
    },
    FaqEntry {
        question: "Can this be used for clinical decisions?",
        keywords: &[
            "clinical",
            "patient",
            "treatment",
            "decision",
            "prescription",
        ],
        answer: "No. This build is synthetic-research-only and must not be used for clinical decisions, prescriptions, treatment planning, delivery, or commissioning claims.",
    },
    FaqEntry {
        question: "How do the linked geometry views work?",
        keywords: &[
            "geometry",
            "view",
            "crosshair",
            "axial",
            "coronal",
            "sagittal",
            "roi",
        ],
        answer: "Click or drag in any anatomical view to update one shared voxel crosshair. The other two views follow it, while LPS coordinates and explicit patient-side labels preserve orientation meaning.",
    },
    FaqEntry {
        question: "How do I open a case in the web app?",
        keywords: &["web", "browser", "open", "case", "zip", "upload"],
        answer: "Browsers cannot hand the app a folder path — everything is bytes. Use File → \"Open case or study files…\" (or the header's Pick files) to multi-select: a case .zip, the 42 NF-BNCT-001 members, or a DICOM study's files all route correctly. Drops work the same way.",
    },
    FaqEntry {
        question: "How do I import my own DICOM study?",
        keywords: &[
            "import", "study", "research", "own", "dicom", "patient", "export",
        ],
        answer: "Drop the study's DICOM files anywhere (or pick them). Each file is bucketed by SOP class — exactly one CT series and one RTSTRUCT are required, a PET series is optional, and everything else is listed as ignored. Click \"Import as research case\" when the set is complete. The import generates its own hash binding — research cases are not checked against the frozen benchmark, and vice versa.",
    },
    FaqEntry {
        question: "What is the σ map?",
        keywords: &[
            "sigma",
            "σ",
            "uncertainty",
            "standard",
            "deviation",
            "error",
        ],
        answer: "The σ-map toggle on the dose map swaps the dose wash for the component's absolute standard-uncertainty field. It is disabled when that component carries no uncertainty data. Component cards report mean relative 1σ alongside dose statistics.",
    },
    FaqEntry {
        question: "How do I compare two dose bundles?",
        keywords: &["compare", "diff", "ratio", "two", "ab", "a/b", "second"],
        answer: "Drop the reference bundle anywhere (it becomes A), then drop the second bundle inside the marked compare zone (it becomes B). A tri-planar B/A ratio map renders on the shared crosshair with agreement statistics; mismatched grids refuse rather than interpolate.",
    },
    FaqEntry {
        question: "How do I overlay measured data on a profile?",
        keywords: &[
            "measurement",
            "measured",
            "overlay",
            "profile",
            "depth",
            "chamber",
        ],
        answer: "Drop a measurement-record JSON while a dose bundle is loaded. The line-profile section plots the record's series against the extracted line; use the series picker and normalization control to align conventions.",
    },
    FaqEntry {
        question: "What does the spectrum viewer show?",
        keywords: &["spectrum", "spectra", "energy", "beam", "source", "log"],
        answer: "Drop a beam-description or fixed-source-definition JSON in the Transport workspace. The histogram renders log-log with thermal, epithermal, and fast regions shaded and their fraction of total source strength listed below.",
    },
    FaqEntry {
        question: "What is a plan-robustness report?",
        keywords: &[
            "robustness",
            "violation",
            "probability",
            "worst",
            "scenario",
        ],
        answer: "Drop a plan-robustness JSON in the Plan workspace. Each objective card shows achieved value, bound, σ, and violation probability with a color-coded bar — the quantities that say whether a plan survives setup uncertainty.",
    },
    FaqEntry {
        question: "How do I run a command from the app?",
        keywords: &[
            "run", "execute", "command", "cli", "process", "job", "timeout",
        ],
        answer: "Native build only: the run panel in the Transport workspace takes a program, arguments, and a wall-clock timeout. Output streams live; Cancel kills and reaps the child. The web build disables it — browsers have no processes.",
    },
    FaqEntry {
        question: "What can the web build do?",
        keywords: &["web", "browser", "wasm", "online", "hosted", "difference"],
        answer: "Everything inspection-side: case archives, dose maps, σ maps, profiles, spectra, A/B diffs, robustness cards, NIfTI volumes, evidence rows. What it cannot do is touch your filesystem or spawn processes — the desktop build owns those.",
    },
    FaqEntry {
        question: "How do I save a picture of the screen?",
        keywords: &["screenshot", "capture", "png", "export", "image", "picture"],
        answer: "View → Screenshot captures the current frame. On the desktop it asks for a save path; in the browser it downloads a PNG. The View menu also carries zoom presets and a reset that recenters crosshairs without unloading artifacts.",
    },
];

struct UseCase {
    title: &'static str,
    workspace: &'static str,
    goal: &'static str,
    steps: &'static [&'static str],
    watch_for: Option<&'static str>,
}

const USE_CASES: [UseCase; 15] = [
    UseCase {
        title: "Inspect the frozen NF-BNCT-001 benchmark",
        workspace: "Overview → Geometry",
        goal: "Open the synthetic benchmark case and confirm it verifies.",
        steps: &[
            "Desktop: File → Open case… and pick the generated NF-BNCT-001 directory.",
            "Web: zip the case directory, then File → Open case archive (.zip)… — or drop the zip anywhere.",
            "The verifier hashes every member against case.json and checks DICOM geometry before rendering.",
            "Geometry workspace shows the phantom in tri-planar views; ROI checkboxes toggle each structure.",
        ],
        watch_for: Some(
            "any modified or missing file is rejected with the exact manifest entry that failed",
        ),
    },
    UseCase {
        title: "Load a case by dropping loose files",
        workspace: "Header → Geometry",
        goal: "Assemble the case without zipping — useful on native builds.",
        steps: &[
            "In a file manager, select case.json, rtstruct.dcm, and all ct/ct-*.dcm files together.",
            "Drop the selection anywhere in the window.",
            "The status line counts received members and names what is still missing.",
            "When all 42 members arrive, verification runs and Geometry opens automatically.",
        ],
        watch_for: Some(
            "drop is additive — a stray file mid-drop goes to the artifact panels, not the case set",
        ),
    },
    UseCase {
        title: "Import your own DICOM study",
        workspace: "Geometry",
        goal: "Turn a real CT + RTSTRUCT export (from a TPS, PACS pull, or archive) into a working case — the answer to 'how do I get a case' when you don't want the synthetic phantom.",
        steps: &[
            "Collect the study's DICOM files — CT slices plus the RTSTRUCT file (a PET series is optional).",
            "Drop them anywhere in the window, or use File → \"Import DICOM study…\" (native) / \"Open case or study files…\" (web).",
            "The header counts collected members; click \"Import as research case\".",
            "Files are bucketed by SOP class: exactly one CT series and one RTSTRUCT are required — a second series is rejected by name rather than merged.",
            "Geometry opens on the imported study; the header shows its hash-bound provenance.",
        ],
        watch_for: Some(
            "research imports bind their own content — they are NOT checked against the frozen benchmark, and an empty-ROI contour reports a NaN centroid instead of failing",
        ),
    },
    UseCase {
        title: "Inspect a dose bundle",
        workspace: "Dose components",
        goal: "Read component statistics for a physical or biological bundle.",
        steps: &[
            "Drop a dose-bundle JSON anywhere — the schema gate validates and binds SHA-256.",
            "Component cards list min/max/mean and mean relative 1σ per component.",
            "Region DVHs and the region metrics table sit below the map.",
        ],
        watch_for: Some(
            "physical and biological bundles are separate layers — they never merge into one display",
        ),
    },
    UseCase {
        title: "Read a dose map",
        workspace: "Dose components",
        goal: "See a component's spatial distribution on the bundle grid.",
        steps: &[
            "Load a dose bundle, then pick the component and quantity in the map controls.",
            "Click any plane to move the crosshair; all three panes stay linked.",
            "Toggle log scale for wide dynamic ranges; contours mark 90/50/10% of peak.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Check uncertainty with the σ map",
        workspace: "Dose components",
        goal: "See where a component's standard uncertainty is large.",
        steps: &[
            "Load a bundle whose components carry absolute_standard_uncertainty.",
            "Toggle σ map — the wash switches from dose to σ per voxel.",
            "Component cards quantify the mean relative 1σ for context.",
        ],
        watch_for: Some("the toggle disables itself when the selected component has no σ field"),
    },
    UseCase {
        title: "Overlay a measurement on a line profile",
        workspace: "Dose components",
        goal: "Compare a measured depth series against the computed profile.",
        steps: &[
            "Load a dose bundle, scroll to the line-profile section, pick an axis.",
            "Drop a measurement-record JSON — its series appear as overlay points.",
            "Use the series picker and normalization control to match conventions.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Compare two dose bundles",
        workspace: "Dose components",
        goal: "A/B two runs — e.g. a repeat transport or a different tally.",
        steps: &[
            "Drop bundle A anywhere; its sha256 and provenance appear in the diff header.",
            "Drop bundle B inside the marked compare zone.",
            "Read the B/A ratio map: blue <0.5×, white 1.0, red >2×, dark = undefined.",
            "Check the statistics row: mean ratio, max deviation, ±5% agreement fraction.",
        ],
        watch_for: Some(
            "mismatched grids or provenance refuse to diff — that refusal is deliberate",
        ),
    },
    UseCase {
        title: "Inspect a beam spectrum",
        workspace: "Transport",
        goal: "See how a source's strength distributes across energy.",
        steps: &[
            "Drop a beam-description or fixed-source-definition JSON.",
            "The log-log histogram shades thermal, epithermal, and fast regions.",
            "Region fractions below quantify each band's share of total strength.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Read a plan-robustness report",
        workspace: "Plan",
        goal: "Check whether a plan's objectives survive setup uncertainty.",
        steps: &[
            "Drop a plan-robustness JSON.",
            "Each objective card shows achieved value, bound, σ, and violation probability.",
            "The probability bar is color-coded — red marks objectives likely violated.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Load and check an exposure plan",
        workspace: "Plan",
        goal: "Validate a structured exposure plan and inspect its issues.",
        steps: &[
            "Drop an exposure-plan JSON (schema openbnct.exposure-plan/…).",
            "The panel lists every detected issue; round-trip edits via CSV/XLSX exports.",
            "Accumulation itself runs through the CLI or Python — the panel is inspection.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Inspect a NIfTI volume",
        workspace: "Dose components",
        goal: "Look at a .nii / .nii.gz volume without a case directory.",
        steps: &[
            "Drop the NIfTI file anywhere.",
            "The inspector shows geometry, dtype, and slice previews.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Run a bounded command (desktop)",
        workspace: "Transport",
        goal: "Drive the CLI from inside the workbench.",
        steps: &[
            "Native build only — the run panel is inert in the browser.",
            "Type a program (e.g. openbnct) and arguments (e.g. dicom verify <dir>).",
            "Set a timeout, Run, watch output live; Cancel kills and reaps the child.",
        ],
        watch_for: Some("the panel wraps the CLI — it never reimplements a scientific path"),
    },
    UseCase {
        title: "Export a case template",
        workspace: "File menu",
        goal: "Get an editable skeleton of the NF-BNCT-001 layout.",
        steps: &[
            "File → Export case template… and pick a destination directory.",
            "The written files are a template — edit before use; they are not the frozen benchmark.",
        ],
        watch_for: None,
    },
    UseCase {
        title: "Capture and share a view",
        workspace: "View menu",
        goal: "Save the current frame for a report or issue.",
        steps: &[
            "View → Screenshot captures the whole window.",
            "Desktop prompts for a save path; the browser downloads a PNG.",
            "Zoom presets under View rescale the UI before capturing.",
        ],
        watch_for: None,
    },
];

// ── Japanese content ──────────────────────────────────────────────
// Mirrors the English arrays above, same order and structure.

const USE_CASES_JA: [UseCase; 15] = [
    UseCase {
        title: "凍結 NF-BNCT-001 ベンチマークの確認",
        workspace: "概要 → ジオメトリ",
        goal: "合成ベンチマーク症例を開き、検証が通ることを確認する。",
        steps: &[
            "デスクトップ: ファイル → 症例を開く… で生成した NF-BNCT-001 ディレクトリを選択。",
            "Web: 症例ディレクトリを zip 化してファイル → 症例/スタディを開く…、または zip をドロップ。",
            "検証器は全メンバーを case.json に対してハッシュ照合し、DICOM ジオメトリを確認してから描画。",
            "ジオメトリワークスペースにファントムの3面ビュー。ROI チェックボックスで各構造を切替。",
        ],
        watch_for: Some("改変・欠落したファイルは、失敗したマニフェスト項目を明示して拒否されます"),
    },
    UseCase {
        title: "バラのファイルをドロップして症例を読み込む",
        workspace: "ヘッダ → ジオメトリ",
        goal: "zip 化せずに症例を組み立てる — ネイティブ版で便利。",
        steps: &[
            "ファイルマネージャで case.json・rtstruct.dcm・全 ct/ct-*.dcm をまとめて選択。",
            "ウィンドウのどこかにドロップ。",
            "ステータス行が受信済みメンバー数と不足分を表示。",
            "全42ファイルが揃うと検証が走り、ジオメトリが自動で開きます。",
        ],
        watch_for: Some(
            "ドロップは加算式 — 途中の stray ファイルは症例セットではなくアーティファクトパネルへ",
        ),
    },
    UseCase {
        title: "自分の DICOM スタディを取り込む",
        workspace: "ジオメトリ",
        goal: "実際の CT + RTSTRUCT エクスポート(TPS・PACS・アーカイブ由来)を作業用症例に変換 — 合成ファントムを使わない場合の「症例の入手方法」の答え。",
        steps: &[
            "スタディの DICOM ファイルを集める — CT スライス + RTSTRUCT ファイル(PET シリーズは任意)。",
            "ウィンドウ内にドロップ、または ファイル → 「DICOM スタディを取り込む…」(ネイティブ)/「症例/スタディを開く…」(Web)。",
            "ヘッダが収集数を表示。「研究用症例として取り込む」をクリック。",
            "ファイルは SOP クラスで仕分け: CT シリーズ1件と RTSTRUCT 1件が必須 — 2件目のシリーズは名前を挙げて拒否(マージしません)。",
            "取り込んだスタディでジオメトリが開き、ヘッダにハッシュ紐付け済みプロベナンスが表示されます。",
        ],
        watch_for: Some(
            "研究用取り込みは自身のコンテンツを紐付け — 凍結ベンチマークとの照合は行いません。空の ROI 輪郭は失敗ではなく NaN 重心として報告されます",
        ),
    },
    UseCase {
        title: "線量バンドルの検査",
        workspace: "線量成分",
        goal: "物理・生物学的バンドルの成分統計を読む。",
        steps: &[
            "線量バンドル JSON をドロップ — スキーマゲートが検証し SHA-256 を紐付け。",
            "成分カードが成分ごとの min/max/mean と平均相対1σを表示。",
            "領域 DVH と領域指標テーブルはマップの下方にあります。",
        ],
        watch_for: Some(
            "物理と生物学的バンドルは別レイヤー — 一つの表示にマージされることはありません",
        ),
    },
    UseCase {
        title: "線量マップを読む",
        workspace: "線量成分",
        goal: "成分の空間分布をバンドルグリッド上で確認する。",
        steps: &[
            "線量バンドルを読み込み、マップコントロールで成分と物理量を選択。",
            "任意の面をクリックしてクロスヘアを移動 — 3面すべて連動。",
            "広いダイナミックレンジには対数スケール、等高線はピークの90/50/10%を表示。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "σマップで不確かさを確認",
        workspace: "線量成分",
        goal: "成分の標準不確かさが大きい場所を把握する。",
        steps: &[
            "absolute_standard_uncertainty を持つ成分を含むバンドルを読み込む。",
            "σマップを切替 — ウォッシュがボクセルごとの線量から σ に切り替わります。",
            "成分カードが平均相対1σを定量表示。",
        ],
        watch_for: Some("選択成分に σ フィールドがない場合、切替は自動で無効になります"),
    },
    UseCase {
        title: "ラインプロファイルに実測を重ねる",
        workspace: "線量成分",
        goal: "計算プロファイルと実測深さ系列を比較する。",
        steps: &[
            "線量バンドルを読み込み、ラインプロファイル区画で軸を選択。",
            "measurement-record JSON をドロップ — その系列がオーバーレイ点として表示。",
            "系列ピッカーと正規化コントロールで規約を合わせます。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "2つの線量バンドルを比較",
        workspace: "線量成分",
        goal: "2つの実行結果を A/B 比較 — 例: 輸送の再実行や別タリー。",
        steps: &[
            "バンドル A をドロップ — sha256 とプロベナンスが差分ヘッダに表示。",
            "バンドル B を印のある比較ゾーン内にドロップ。",
            "B/A 比マップを読む: 0.5倍未満は青、1.0は白、2倍超は赤、暗色は未定義。",
            "統計行を確認: 平均比・最大偏差・±5%一致率。",
        ],
        watch_for: Some("グリッドやプロベナンスの不一致は差分を拒否 — その拒否は意図的です"),
    },
    UseCase {
        title: "ビームスペクトルの検査",
        workspace: "輸送",
        goal: "線源強度がエネルギーにどう分布するかを見る。",
        steps: &[
            "beam-description または fixed-source-definition JSON をドロップ。",
            "両対数ヒストグラムが熱・エピサーマル・高速領域をシェード表示。",
            "下方の領域割合が各帯域の全強度に占める割合を定量。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "plan-robustness レポートを読む",
        workspace: "計画",
        goal: "計画の目的関数がセットアップ不確かさに耐えるか確認。",
        steps: &[
            "plan-robustness JSON をドロップ。",
            "各目的カードに達成値・境界・σ・違反確率を表示。",
            "確率バーは色分け — 赤は違反の可能性が高い目的を示します。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "照射計画の読み込みと確認",
        workspace: "計画",
        goal: "構造化された照射計画を検証し、問題点を確認する。",
        steps: &[
            "exposure-plan JSON をドロップ(スキーマ openbnct.exposure-plan/…)。",
            "パネルが検出した全問題を一覧。CSV/XLSX エクスポートで編集を往復。",
            "線量積算は CLI または Python から実行 — パネルは検査専用です。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "NIfTI ボリュームの検査",
        workspace: "線量成分",
        goal: "症例ディレクトリなしで .nii / .nii.gz ボリュームを見る。",
        steps: &[
            "NIfTI ファイルをドロップ。",
            "インスペクタがジオメトリ・dtype・スライスプレビューを表示。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "範囲制限付きコマンド実行(デスクトップ)",
        workspace: "輸送",
        goal: "ワークベンチ内から CLI を駆動する。",
        steps: &[
            "ネイティブ版のみ — ブラウザではラン パネルは無効です。",
            "プログラム(例: openbnct)と引数(例: dicom verify <dir>)を入力。",
            "タイムアウトを設定して Run。出力はライブ表示、Cancel は子プロセスを終了・回収。",
        ],
        watch_for: Some("パネルは CLI のラッパー — 科学計算パスを再実装しません"),
    },
    UseCase {
        title: "症例テンプレートの出力",
        workspace: "ファイルメニュー",
        goal: "NF-BNCT-001 レイアウトの編集可能な骨格を得る。",
        steps: &[
            "ファイル → 症例テンプレートを出力… で出力先ディレクトリを選択。",
            "出力されるファイルはテンプレート — 使用前に編集が必要であり、凍結ベンチマークではありません。",
        ],
        watch_for: None,
    },
    UseCase {
        title: "ビューのキャプチャと共有",
        workspace: "表示メニュー",
        goal: "レポートや課題報告用に現在のフレームを保存する。",
        steps: &[
            "表示 → スクリーンショット でウィンドウ全体をキャプチャ。",
            "デスクトップは保存先を選択、ブラウザは PNG をダウンロード。",
            "表示メニューのズームプリセットでキャプチャ前に UI を拡縮できます。",
        ],
        watch_for: None,
    },
];

const QUICK_START_STEPS_JA: [TourStep; 5] = [
    TourStep {
        target: TourTarget::Brand,
        workspace: None,
        title: "OpenBNCT へようこそ",
        instruction: "この Avila Labs ワークベンチは、ジオメトリ・輸送・線量・エビデンスを一つの研究用インターフェースにまとめ、未完成の科学を結果として提示しません。",
    },
    TourStep {
        target: TourTarget::CaseLoader,
        workspace: None,
        title: "症例の読み込みと検証",
        instruction: "NF-BNCT-001 ディレクトリを入力して「読込・検証」を押します。アーティファクト完全性と DICOM ジオメトリの両方が通らなければ画像は表示されません。",
    },
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: None,
        title: "ワークスペース間の移動",
        instruction: "6つのワークスペースで同じ症例をジオメトリからエビデンスまで確認します。科学的結果が未準備でもワークスペース自体は存在します。",
    },
    TourStep {
        target: TourTarget::OverviewGates,
        workspace: Some(HelpWorkspace::Overview),
        title: "まずゲートを読む",
        instruction: "すべてのステータスは範囲限定です。検証済み・凍結・ブロック・保留・入力待ちはそれぞれ別の意味を持ち、緑のジオメトリゲートが輸送や線量を昇格させることはありません。",
    },
    TourStep {
        target: TourTarget::HelpButton,
        workspace: None,
        title: "ヘルプは常にここに",
        instruction: "ヘルプメニューからコンテキストガイド・同梱回答・別のツアーを開けます。ツアー中はいつでも Escape で終了できます。",
    },
];

const GEOMETRY_STEPS_JA: [TourStep; 4] = [
    TourStep {
        target: TourTarget::CaseLoader,
        workspace: None,
        title: "検証済み入力から始める",
        instruction: "ジオメトリツールは、DICOM とアーティファクト完全性の境界を通過した症例だけを受け取ります。",
    },
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: Some(HelpWorkspace::Geometry),
        title: "ジオメトリを開く",
        instruction: "ジオメトリワークスペースは、輸送準備の前に使う患者空間の実データビューです。",
    },
    TourStep {
        target: TourTarget::GeometryControls,
        workspace: Some(HelpWorkspace::Geometry),
        title: "表示のコントロール",
        instruction: "CT レベル/幅、ROI 不透明度、連動ボクセル、各構造の表示切替はここで行います。これらは表示のみを変更し、元データは変更しません。",
    },
    TourStep {
        target: TourTarget::GeometryViews,
        workspace: Some(HelpWorkspace::Geometry),
        title: "連動ビューの利用",
        instruction: "軸位・冠状・矢状断のいずれかをクリックまたはドラッグします。3面は1つのボクセルクロスヘアを共有し、明示的な患者方向ラベルを保持します。",
    },
];

const READINESS_STEPS_JA: [TourStep; 4] = [
    TourStep {
        target: TourTarget::OverviewGates,
        workspace: Some(HelpWorkspace::Overview),
        title: "レディネス概要から開始",
        instruction: "各行は実行時検証・凍結済みプロジェクト証拠・未解決の科学的課題を分離します。",
    },
    TourStep {
        target: TourTarget::TransportGates,
        workspace: Some(HelpWorkspace::Transport),
        title: "輸送ゲートチェーンを追跡",
        instruction: "輸送は順序どおりにしか進みません。現在の O-17/O-18 応答ブロッカーが制御実行ゲートの昇格を妨げています。",
    },
    TourStep {
        target: TourTarget::TransportActions,
        workspace: Some(HelpWorkspace::Transport),
        title: "能力が操作を制御",
        instruction: "無効なボタンは意図的です。アダプタがテスト済み能力を宣言し、上流のエビデンスゲートが通過した時のみ有効になります。",
    },
    TourStep {
        target: TourTarget::EvidenceLedger,
        workspace: Some(HelpWorkspace::Evidence),
        title: "エビデンス台帳の確認",
        instruction: "各行は範囲限定の主張を一件示し、可能な場合は凍結証拠バイトから短い識別子を導出します。全体クリアのバッジは存在しません。",
    },
];

const DOSE_MAP_STEPS_JA: [TourStep; 5] = [
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: Some(HelpWorkspace::Dose),
        title: "線量成分を開く",
        instruction: "物理または生物学的線量バンドル JSON をウィンドウのどこかにドロップします。バンドル自身のグリッドがビューアを駆動するため、Web でも症例ディレクトリは不要です。",
    },
    TourStep {
        target: TourTarget::DoseMap,
        workspace: Some(HelpWorkspace::Dose),
        title: "成分マップを読む",
        instruction: "成分(ホウ素・窒素・水素・ガンマ・総量)と物理量を選択します。3面のいずれかをクリックすると共有クロスヘアが移動し、全面が連動します。",
    },
    TourStep {
        target: TourTarget::DoseMap,
        workspace: Some(HelpWorkspace::Dose),
        title: "表示を整形",
        instruction: "対数スケールは広いダイナミックレンジを可視化し、等線量線はピークの90/50/10%を示します。成分が不確かさを持つ場合、σマップ切替でウォッシュを絶対標準不確かさに切り替えます。",
    },
    TourStep {
        target: TourTarget::DoseMap,
        workspace: Some(HelpWorkspace::Dose),
        title: "線プロファイルで定量",
        instruction: "マップ下のラインプロファイルは、選択成分をクロスヘアを通る軸に沿って描画します。measurement-record JSON をドロップすると同軸に実測点を重ねます。",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "カードが要約を持つ",
        instruction: "マップ上の成分カードは min/max/mean と平均相対1σを報告します。下方の比較ドロップゾーンは2つ目のバンドルを受け取り A/B 差分を表示 — 次のツアーで詳しく。",
    },
];

const COMPARE_STEPS_JA: [TourStep; 4] = [
    TourStep {
        target: TourTarget::WorkspaceNavigation,
        workspace: Some(HelpWorkspace::Dose),
        title: "アーティファクト A を読み込む",
        instruction: "最初の線量バンドルをどこかにドロップ — それが参照(A)になります。症例ID・SHA-256・プロベナンス紐付けが差分ヘッダに表示されます。",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "比較ゾーンに B をドロップ",
        instruction: "2つ目のバンドルを印のあるゾーン内にドロップします。ゾーン外にドロップすると A が置き換わります — ゾーンが比較を成立させます。",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "比マップを読む",
        instruction: "共有クロスヘア上に3面 B/A 比を描画: 0.5倍未満は青、1.0で白、2倍超は赤、比が未定義の箇所は暗色。統計は平均比・最大偏差・±5%一致率を報告します。",
    },
    TourStep {
        target: TourTarget::CompareZone,
        workspace: Some(HelpWorkspace::Dose),
        title: "プロベナンスの確認",
        instruction: "差分ヘッダは両アーティファクトのコンテンツ紐付けを表示 — ジオメトリやプロベナンスが一致しなければ、推測せず拒否します。その拒否が検証の本質です。",
    },
];

const SOURCE_RUN_STEPS_JA: [TourStep; 4] = [
    TourStep {
        target: TourTarget::SpectrumPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "線源アーティファクトをドロップ",
        instruction: "beam-description または fixed-source-definition JSON をドロップすると、エネルギースペクトルを両対数で表示 — 熱/エピサーマル/高速の領域シェードと領域別割合付き。",
    },
    TourStep {
        target: TourTarget::SpectrumPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "領域を読む",
        instruction: "シェード帯は BNCT に関係するエネルギー範囲を示し、プロット下の割合は各領域の線源強度を定量します — エピサーマル品質に重要な数値です。",
    },
    TourStep {
        target: TourTarget::RunPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "範囲制限付きコマンド実行(ネイティブ)",
        instruction: "デスクトップ版ではプログラム(例: openbnct)と引数(例: dicom verify <症例ディレクトリ>)、タイムアウトを設定して Run。出力はライブ表示され、Cancel は子プロセスを終了・回収します。",
    },
    TourStep {
        target: TourTarget::RunPanel,
        workspace: Some(HelpWorkspace::Transport),
        title: "Web 版は検査専用",
        instruction: "ブラウザ版ではこのパネルは意図的に無効 — プロセスが存在しないためです。Web 版はアーティファクトの検査と検証を行い、実行はデスクトップバイナリが担います。",
    },
];

const FAQ_JA: [FaqEntry; 19] = [
    FaqEntry {
        question: "症例とは何ですか?どうやって入手しますか?",
        keywords: &["症例", "ケース", "入手", "最初", "始め方", "case"],
        answer: "症例はすべての計算の基盤となる空間情報です: 患者ジオメトリ用の CT シリーズ + 輪郭用の RTSTRUCT をハッシュで紐付け、下流のアーティファクトが所属を証明できるようにします。2種類読み込めます: 凍結 NF-BNCT-001 ベンチマーク(`openbnct dicom generate` で生成 — 外部ファイル不要)と、手持ちの研究スタディ(TPS やアーカイブからの CT + RTSTRUCT エクスポート)。研究用取り込みは読込時に厳密パース・ハッシュ紐付けされ、フィクスチャとの一致ではなく自身との整合性で検証されます。",
    },
    FaqEntry {
        question: "症例はどう読み込みますか?",
        keywords: &["読み込み", "読込", "症例", "dicom", "生成", "load"],
        answer: "NF-BNCT-001: CLI で生成し、CASE 欄にディレクトリを入力して「読込・検証」。研究スタディ: ファイル → 「DICOM スタディを取り込む…」でエクスポートフォルダを選択、または DICOM ファイルをドロップ — SOP クラスで仕分けされ、「研究用症例として取り込む」で完了します。",
    },
    FaqEntry {
        question: "なぜ輸送ボタンが無効ですか?",
        keywords: &[
            "輸送",
            "無効",
            "ボタン",
            "openmc",
            "実行",
            "準備",
            "disabled",
        ],
        answer: "制御付き輸送パスはまだ適格化されていません。O-17/O-18 輸送済み光子応答の取り扱いにレビューが必要なため、OpenMC アダプタが CLI へ能力を広告していても対話的な準備・実行は無効のままです。",
    },
    FaqEntry {
        question: "OpenBNCT は完全に OpenMC 依存ですか?",
        keywords: &["依存", "openmc", "バックエンド", "中立", "mcnp", "phits"],
        answer: "いいえ。OpenMC は最初のバックエンドであり、症例・物理線量・不確かさ・エビデンスの契約は輸送中立です。将来のアダプタや外部取り込み結果は GUI を再実装せず同じ契約を利用できます。",
    },
    FaqEntry {
        question: "線量値とヒートマップはどこですか?",
        keywords: &["線量", "ヒートマップ", "dvh", "値", "結果", "dose"],
        answer: "線量ワークスペースで検証済みの物理・生物学的バンドルを読み込んだ後にのみ表示されます。OpenBNCT はプレースホルダ線量を絶対に描画しません。生物学的重み付けは別レイヤーであり、臨床量ではありません。",
    },
    FaqEntry {
        question: "ステータスラベルの意味は?",
        keywords: &[
            "ステータス",
            "検証済み",
            "凍結",
            "ブロック",
            "保留",
            "status",
        ],
        answer: "検証済みは実行時ゲートの通過、凍結はコミット済みプロジェクト成果物の存在、ブロックは既知の未解決要件、保留は未実行、入力待ちは症例なしにローカルゲートが実行不可、を意味します。",
    },
    FaqEntry {
        question: "OpenBNCT は pip でインストールできますか?",
        keywords: &["pip", "python", "pypi", "インストール", "install"],
        answer: "それが確定した配布方針ですが、PyPI リリースはまだありません。Python パッケージは PyO3 と maturin で同じ Rust コアを呼び出し、第二の Python 線量エンジンを含みません。",
    },
    FaqEntry {
        question: "臨床判断に使えますか?",
        keywords: &["臨床", "患者", "治療", "判断", "処方", "clinical"],
        answer: "いいえ。このビルドは合成データ研究専用であり、臨床判断・処方・治療計画・照射・コミッショニングの主張には使用できません。",
    },
    FaqEntry {
        question: "連動ジオメトリビューの仕組みは?",
        keywords: &[
            "ジオメトリ",
            "ビュー",
            "クロスヘア",
            "軸位",
            "冠状",
            "矢状",
            "geometry",
        ],
        answer: "任意の解剖断面をクリックまたはドラッグすると1つの共有ボクセルクロスヘアが更新されます。他の2面が追従し、LPS 座標と明示的な患者方向ラベルで方向の意味を保持します。",
    },
    FaqEntry {
        question: "Web アプリで症例を開くには?",
        keywords: &["web", "ブラウザ", "開く", "症例", "zip", "アップロード"],
        answer: "ブラウザはフォルダパスを渡せません — すべてバイト列です。ファイル → 「症例/スタディを開く…」(またはヘッダの「ファイルを選択」)で複数選択: 症例 .zip、NF-BNCT-001 全42ファイル、DICOM スタディのファイル群、すべて正しく振り分けられます。ドロップも同様に動作します。",
    },
    FaqEntry {
        question: "自分の DICOM スタディを取り込むには?",
        keywords: &[
            "取り込み",
            "スタディ",
            "研究",
            "自分の",
            "dicom",
            "インポート",
            "import",
        ],
        answer: "スタディの DICOM ファイルをどこかにドロップ(または選択)します。各ファイルは SOP クラスで仕分け — CT シリーズ1件と RTSTRUCT 1件が必須、PET シリーズは任意、その他は無視として一覧表示。「研究用症例として取り込む」で完了。独自のハッシュ紐付けを生成し、凍結ベンチマークとは相互に照合されません。",
    },
    FaqEntry {
        question: "σマップとは何ですか?",
        keywords: &["σ", "シグマ", "不確かさ", "標準偏差", "誤差", "sigma"],
        answer: "線量マップの σ 切替はウォッシュを成分の絶対標準不確かさフィールドに置き換えます。その成分が不確かさデータを持たない場合は無効です。成分カードは線量統計と並んで平均相対1σを報告します。",
    },
    FaqEntry {
        question: "2つの線量バンドルを比較するには?",
        keywords: &["比較", "差分", "a/b", "比", "二つ", "compare", "diff"],
        answer: "最初のバンドルをどこかにドロップ(=参照A)、2つ目を Dose ワークスペースの「B と比較」ゾーンにドロップ。グリッドが一致すれば共有クロスヘア上に3面 B/A 比マップと一致統計が表示されます。",
    },
    FaqEntry {
        question: "実測値をプロファイルに重ねるには?",
        keywords: &[
            "実測",
            "測定",
            "オーバーレイ",
            "プロファイル",
            "measurement",
        ],
        answer: "線量マップのラインプロファイルは選択成分をクロスヘア軸に沿って描画します。measurement-record JSON をドロップすると、同じ軸と深さ座標に実測点を重ね、正規化差が一目で分かります。",
    },
    FaqEntry {
        question: "スペクトルビューアは何を表示しますか?",
        keywords: &["スペクトル", "ビーム", "エネルギー", "束", "spectrum"],
        answer: "Transport ワークスペースで beam-description または fixed-source-definition JSON をドロップ。ヒストグラムビンを両対数階段状に描画し、熱(<0.5 eV)/エピサーマル/高速領域をシェード、領域別割合を表示します。単色線源はマーカー表示になります。",
    },
    FaqEntry {
        question: "plan-robustness レポートとは?",
        keywords: &["ロバスト", "違反", "確率", "シナリオ", "robustness"],
        answer: "Plan ワークスペースに plan-robustness JSON をドロップ。各目的カードに達成値・境界・σ・違反確率とカラーバーを表示 — セットアップ不確かさ下で計画が生き残るかを示す量です。",
    },
    FaqEntry {
        question: "アプリからコマンドを実行するには?",
        keywords: &["実行", "コマンド", "cli", "プロセス", "ジョブ", "run"],
        answer: "ネイティブ版のみ: Transport ワークスペースのラン パネルでプログラム・引数・タイムアウトを指定。出力はライブ表示、Cancel は子プロセスを終了・回収。Web 版は無効 — ブラウザにプロセスは存在しません。",
    },
    FaqEntry {
        question: "Web 版でできることは?",
        keywords: &["web", "ブラウザ", "wasm", "オンライン", "違い", "browser"],
        answer: "検査系はすべて: 症例アーカイブ、線量マップ、σマップ、プロファイル、スペクトラ、A/B 差分、ロバスト性カード、NIfTI、エビデンス行。できないのはファイルシステムアクセスとプロセス生成 — それらはデスクトップ版の役割です。",
    },
    FaqEntry {
        question: "画面の画像を保存するには?",
        keywords: &["スクリーンショット", "キャプチャ", "png", "画像", "保存"],
        answer: "表示 → スクリーンショット で現在のフレームを保存。デスクトップは保存先を選択、ブラウザは PNG ダウンロード。表示メニューにはズームプリセットと、読み込み済みアーティファクトを保持したままクロスヘアを中央に戻すリセットもあります。",
    },
];

fn best_answer(question: &str, language: Language) -> Option<usize> {
    let normalized = question.to_lowercase();
    let tokens: Vec<_> = normalized
        .split(|character: char| !(character.is_ascii_alphanumeric() || character >= '\u{3040}'))
        .filter(|token| {
            token.chars().count() >= 3
                || (token.chars().count() >= 2 && token.chars().any(|c| c >= '\u{3040}'))
        })
        .collect();
    faq(language)
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let title = entry.question.to_lowercase();
            let keyword_score = entry
                .keywords
                .iter()
                .filter(|keyword| normalized.contains(**keyword))
                .count()
                * 3;
            let title_score = tokens
                .iter()
                .filter(|token| title.contains(**token))
                .count();
            (index, keyword_score + title_score)
        })
        .filter(|(_, score)| *score >= 3)
        .max_by_key(|(_, score)| *score)
        .map(|(index, _)| index)
}

fn show_answer(ui: &mut egui::Ui, entry: FaqEntry) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong(entry.question);
        ui.label(entry.answer);
    });
}

fn dim_rectangles(screen: egui::Rect, hole: egui::Rect) -> [egui::Rect; 4] {
    let hole = hole.intersect(screen);
    [
        egui::Rect::from_min_max(screen.min, egui::pos2(screen.max.x, hole.top())),
        egui::Rect::from_min_max(egui::pos2(screen.min.x, hole.bottom()), screen.max),
        egui::Rect::from_min_max(
            egui::pos2(screen.min.x, hole.top()),
            egui::pos2(hole.left(), hole.bottom()),
        ),
        egui::Rect::from_min_max(
            egui::pos2(hole.right(), hole.top()),
            egui::pos2(screen.max.x, hole.bottom()),
        ),
    ]
}

fn callout_position(screen: egui::Rect, spotlight: egui::Rect) -> egui::Pos2 {
    const CALLOUT_WIDTH: f32 = 382.0;
    const CALLOUT_HEIGHT: f32 = 245.0;
    const GAP: f32 = 16.0;
    const MARGIN: f32 = 12.0;

    let desired = if spotlight.right() + GAP + CALLOUT_WIDTH <= screen.right() {
        egui::pos2(spotlight.right() + GAP, spotlight.top())
    } else if spotlight.left() - GAP - CALLOUT_WIDTH >= screen.left() {
        egui::pos2(spotlight.left() - GAP - CALLOUT_WIDTH, spotlight.top())
    } else if spotlight.bottom() + GAP + CALLOUT_HEIGHT <= screen.bottom() {
        egui::pos2(spotlight.left(), spotlight.bottom() + GAP)
    } else {
        egui::pos2(spotlight.left(), spotlight.top() - GAP - CALLOUT_HEIGHT)
    };
    egui::pos2(
        desired.x.clamp(
            screen.left() + MARGIN,
            screen.right() - CALLOUT_WIDTH - MARGIN,
        ),
        desired.y.clamp(
            screen.top() + MARGIN,
            screen.bottom() - CALLOUT_HEIGHT - MARGIN,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tour_step_has_a_valid_target_and_copy() {
        for guide in GuideKind::ALL {
            assert!(!guide.steps(Language::English).is_empty());
            for step in guide.steps(Language::English) {
                assert!(step.target.index() < TARGET_COUNT);
                assert!(!step.title.is_empty());
                assert!(!step.instruction.is_empty());
            }
        }
    }

    #[test]
    fn bundled_questions_match_relevant_answers() {
        let transport = best_answer("Why can't I execute OpenMC?", Language::English)
            .expect("transport answer");
        assert!(FAQ[transport].answer.contains("transport"));
        let pip = best_answer("Is there a pip install?", Language::English).expect("pip answer");
        assert!(FAQ[pip].answer.contains("PyPI") || FAQ[pip].answer.contains("pip"));
        let clinical = best_answer("Can I treat a patient with this?", Language::English)
            .expect("clinical answer");
        assert!(FAQ[clinical].answer.contains("must not"));
        assert_eq!(
            best_answer("completely unrelated words", Language::English),
            None
        );
    }

    #[test]
    fn dimmer_preserves_the_spotlight_cutout() {
        let screen = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1_000.0, 700.0));
        let hole = egui::Rect::from_min_max(egui::pos2(200.0, 150.0), egui::pos2(600.0, 400.0));
        for rect in dim_rectangles(screen, hole) {
            assert!(screen.contains_rect(rect));
            assert!(rect.intersect(hole).area() <= f32::EPSILON);
        }
    }

    #[test]
    fn requested_workspace_tracks_the_active_step() {
        let mut help = GuidedHelp {
            active_tour: Some(ActiveTour {
                guide: GuideKind::Readiness,
                step_index: 1,
            }),
            ..Default::default()
        };
        assert_eq!(
            help.requested_workspace(Language::English),
            Some(HelpWorkspace::Transport)
        );
        help.active_tour = None;
        assert_eq!(help.requested_workspace(Language::English), None);
    }

    #[test]
    fn help_center_and_tour_render_at_the_minimum_viewport() {
        let context = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(960.0, 640.0),
            )),
            ..Default::default()
        };
        let mut help = GuidedHelp::default();
        help.toggle_center();
        let mut output = context.run_ui(input(), |_ui| {
            help.show_center(
                &context,
                HelpWorkspace::Overview,
                false,
                Language::English,
                crate::Theme::resolve(false),
            );
        });
        output.textures_delta.clear();

        help.active_tour = Some(ActiveTour {
            guide: GuideKind::QuickStart,
            step_index: 0,
        });
        let mut targets = TourTargets::default();
        targets.set(
            TourTarget::Brand,
            egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(48.0, 48.0)),
        );
        let mut output = context.run_ui(input(), |_ui| {
            help.show_tour(
                &context,
                &targets,
                Language::English,
                crate::Theme::resolve(false),
            );
        });
        output.textures_delta.clear();
    }
}
