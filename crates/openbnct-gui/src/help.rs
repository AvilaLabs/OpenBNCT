// SPDX-License-Identifier: Apache-2.0

use eframe::egui;

const TARGET_COUNT: usize = 10;

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
}

impl GuideKind {
    const ALL: [Self; 3] = [Self::QuickStart, Self::Geometry, Self::Readiness];

    const fn title(self) -> &'static str {
        match self {
            Self::QuickStart => "Quick start",
            Self::Geometry => "Inspect geometry",
            Self::Readiness => "Readiness & evidence",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::QuickStart => "Learn the shell, load gate, workspaces, and status language.",
            Self::Geometry => "Review linked DICOM views, display controls, and the crosshair.",
            Self::Readiness => "Trace why a capability is frozen, blocked, pending, or verified.",
        }
    }

    const fn steps(self) -> &'static [TourStep] {
        match self {
            Self::QuickStart => &QUICK_START_STEPS,
            Self::Geometry => &GEOMETRY_STEPS,
            Self::Readiness => &READINESS_STEPS,
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

    pub(crate) fn requested_workspace(&self) -> Option<HelpWorkspace> {
        self.active_step().and_then(|step| step.workspace)
    }

    pub(crate) fn show_center(
        &mut self,
        context: &egui::Context,
        workspace: HelpWorkspace,
        case_loaded: bool,
        theme: crate::Theme,
    ) {
        if !self.center_open || self.active_tour.is_some() {
            return;
        }

        let mut open = true;
        let mut guide_to_start = None;
        egui::Window::new("Help & guided tours")
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
                    egui::RichText::new("CONTEXTUAL HELP")
                        .small()
                        .strong()
                        .color(theme.brand),
                );
                let (title, body) = workspace_help(workspace);
                ui.heading(title);
                ui.label(body);

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Walk me through it");
                for guide in GuideKind::ALL {
                    let enabled = guide != GuideKind::Geometry || case_loaded;
                    let response = ui.add_enabled(
                        enabled,
                        egui::Button::new(egui::RichText::new(guide.title()).strong())
                            .min_size(egui::vec2(ui.available_width(), 34.0)),
                    );
                    if response.clicked() {
                        guide_to_start = Some(guide);
                    }
                    ui.small(guide.description());
                    if guide == GuideKind::Geometry && !case_loaded {
                        ui.colored_label(
                            theme.warn_text,
                            "Load a verified case to enable this tour.",
                        );
                    }
                    ui.add_space(5.0);
                }

                ui.add_space(8.0);
                ui.separator();
                ui.heading("Ask bundled help");
                ui.small(
                    "Answers stay on this device and come from reviewed, bundled guidance. "
                        .to_owned()
                        + "No external model or service is contacted.",
                );
                let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.question)
                        .hint_text("Why is transport disabled?")
                        .desired_width(f32::INFINITY),
                );
                if response.changed() || (response.has_focus() && enter_pressed) {
                    self.answer_index = best_answer(&self.question);
                }

                if self.question.trim().is_empty() {
                    ui.small("Try one of these common questions:");
                    for (index, entry) in FAQ.iter().enumerate().take(4) {
                        if ui.link(entry.question).clicked() {
                            self.question = entry.question.to_owned();
                            self.answer_index = Some(index);
                        }
                    }
                } else if let Some(index) = self.answer_index {
                    show_answer(ui, FAQ[index]);
                } else {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.strong("No bundled answer matched that question yet.");
                        ui.label(
                            "Try asking about loading a case, geometry, status gates, OpenMC, "
                                .to_owned()
                                + "dose, clinical use, or Python installation.",
                        );
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
        theme: crate::Theme,
    ) {
        let Some(active) = self.active_tour else {
            return;
        };
        let steps = active.guide.steps();
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
                                egui::RichText::new(active.guide.title().to_uppercase())
                                    .small()
                                    .strong()
                                    .color(theme.brand),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    close = ui
                                        .button(egui::RichText::new("×").size(18.0))
                                        .on_hover_text("End tour")
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
                        ui.small(
                            "The highlighted controls remain live. Use them now if useful, then continue.",
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            go_back = ui
                                .add_enabled(active.step_index > 0, egui::Button::new("Back"))
                                .clicked();
                            ui.label(format!(
                                "Step {} of {}",
                                active.step_index + 1,
                                steps.len()
                            ));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    go_next = ui
                                        .button(if active.step_index + 1 == steps.len() {
                                            "Finish"
                                        } else {
                                            "Next"
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

    fn active_step(&self) -> Option<TourStep> {
        let active = self.active_tour?;
        active.guide.steps().get(active.step_index).copied()
    }
}

fn workspace_help(workspace: HelpWorkspace) -> (&'static str, &'static str) {
    match workspace {
        HelpWorkspace::Overview => (
            "Research overview",
            "Start here to understand the current scientific ceiling. Each readiness card is a scoped claim, not a project-wide pass or fail.",
        ),
        HelpWorkspace::Geometry => (
            "Geometry",
            "Inspect the accepted DICOM geometry in linked patient-space views. Display controls do not alter the underlying case.",
        ),
        HelpWorkspace::Transport => (
            "Transport",
            "Follow the ordered gate chain and backend capability flags. Unavailable actions are intentionally disabled.",
        ),
        HelpWorkspace::Plan => (
            "Exposure plan",
            "Load a structured exposure plan, inspect every detected issue, and round-trip the schedule through CSV/XLSX tables. Accumulation runs through the CLI or Python.",
        ),
        HelpWorkspace::Dose => (
            "Dose components",
            "Load a validated physical or biological dose bundle to inspect component statistics and region DVHs. The two layers never merge.",
        ),
        HelpWorkspace::Evidence => (
            "Evidence",
            "Inspect evidence one bounded claim at a time. Frozen project artifacts and a verified local run are intentionally different states.",
        ),
    }
}

#[derive(Debug, Clone, Copy)]
struct FaqEntry {
    question: &'static str,
    keywords: &'static [&'static str],
    answer: &'static str,
}

const FAQ: [FaqEntry; 8] = [
    FaqEntry {
        question: "How do I load a case?",
        keywords: &["load", "case", "directory", "dicom", "generate"],
        answer: "Generate NF-BNCT-001 with the CLI, enter its directory in the CASE field, and press Load & verify. OpenBNCT rejects modified artifacts or ambiguous DICOM geometry before rendering.",
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
];

fn best_answer(question: &str) -> Option<usize> {
    let normalized = question.to_ascii_lowercase();
    let tokens: Vec<_> = normalized
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .collect();
    FAQ.iter()
        .enumerate()
        .map(|(index, entry)| {
            let title = entry.question.to_ascii_lowercase();
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
            assert!(!guide.steps().is_empty());
            for step in guide.steps() {
                assert!(step.target.index() < TARGET_COUNT);
                assert!(!step.title.is_empty());
                assert!(!step.instruction.is_empty());
            }
        }
    }

    #[test]
    fn bundled_questions_match_relevant_answers() {
        assert_eq!(best_answer("Why can't I execute OpenMC?"), Some(1));
        assert_eq!(best_answer("Is there a pip install?"), Some(5));
        assert_eq!(best_answer("Can I treat a patient with this?"), Some(6));
        assert_eq!(best_answer("completely unrelated words"), None);
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
        assert_eq!(help.requested_workspace(), Some(HelpWorkspace::Transport));
        help.active_tour = None;
        assert_eq!(help.requested_workspace(), None);
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
            help.show_tour(&context, &targets, crate::Theme::resolve(false));
        });
        output.textures_delta.clear();
    }
}
