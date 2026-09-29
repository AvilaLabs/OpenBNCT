// SPDX-License-Identifier: MIT

//! Project workspace: drives `openbnct project init|run|status` from the
//! desktop shell. The GUI never re-implements the pipeline — it builds the
//! command line, launches it through the bounded [`crate::run::Job`]
//! runner (process group, timeout, Cancel), parses the runner's per-step
//! and `[sn] outer k/max: residual …` progress lines, and reads
//! `out/run-manifest.json` for the step table and the result paths.
//!
//! Research software: nothing here validates a dose for clinical use.
//! Process launching is native-only; the web build shows an explanatory
//! note (see [`show_project_workspace`]).

#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eframe::egui;
use openbnct_dicom::ImportedStudy;

use crate::i18n::Language;
use crate::run::Job;
use crate::{Theme, show_workspace_heading, t};

/// Wall-clock bound for `project run` (transport dominates): 2 hours.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// Bound for the short `init` / `builtins` helper commands.
const HELPER_TIMEOUT: Duration = Duration::from_secs(300);
const LOG_CAP: usize = 400;
const STEP_LABELS: [&str; 7] = [
    "import",
    "calibrate",
    "beam",
    "transport",
    "boron",
    "metrics",
    "report",
];
const APPROACHES: [&str; 6] = ["+x", "-x", "+y", "-y", "+z", "-z"];
const FALLBACK_BEAM: &str = "builtin:beams/fir1-k63";

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum StepState {
    Pending,
    Running,
    Done(f64),
    Skipped,
    Failed(Option<String>),
}

#[derive(Debug, PartialEq)]
enum StepEvent {
    Running,
    Skipped,
    Done(f64),
    Failed,
}

/// Parse a runner line such as `[4/7] transport  done in 12.3 s`.
fn parse_step_line(line: &str) -> Option<(usize, StepEvent)> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (fraction, rest) = rest.split_once(']')?;
    let (index, total) = fraction.split_once('/')?;
    let index: usize = index.trim().parse().ok()?;
    let _total: usize = total.trim().parse().ok()?;
    if index == 0 || index > STEP_LABELS.len() {
        return None;
    }
    let rest = rest.trim();
    let after_label = rest
        .split_once(char::is_whitespace)
        .map_or("", |(_, r)| r.trim());
    let event = if after_label.starts_with("running") {
        StepEvent::Running
    } else if after_label.starts_with("skipped") {
        StepEvent::Skipped
    } else if let Some(seconds) = after_label.strip_prefix("done in ") {
        StepEvent::Done(
            seconds
                .trim_end_matches(" s")
                .trim()
                .parse()
                .unwrap_or_default(),
        )
    } else if after_label.starts_with("FAILED") {
        StepEvent::Failed
    } else {
        return None;
    };
    Some((index - 1, event))
}

/// Parse `[sn] outer 3/64: residual 1.234e-3 (5.0s)` into (k, max, residual).
fn parse_sn_line(line: &str) -> Option<(u32, u32, f64)> {
    let rest = line.trim_start().strip_prefix("[sn] outer ")?;
    let (fraction, rest) = rest.split_once(':')?;
    let (k, max) = fraction.split_once('/')?;
    let residual = rest.trim().strip_prefix("residual ")?;
    let residual = residual.split_whitespace().next()?;
    Some((
        k.trim().parse().ok()?,
        max.trim().parse().ok()?,
        residual.parse().ok()?,
    ))
}

/// Beam descriptions listed by `openbnct project builtins`
/// (`builtin:beams/NAME  (file, N bytes)` lines).
fn parse_builtin_beams(output: &[String]) -> Vec<String> {
    output
        .iter()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|spec| spec.starts_with("builtin:beams/"))
        .map(str::to_owned)
        .collect()
}

/// Edit values in the `project.toml` that `project init` wrote. Line-based
/// so the header comment (detected ROIs) survives; every key must be found
/// exactly once or the patch is refused.
fn patch_project_toml(
    text: &str,
    beam: &str,
    approach: &str,
    blood_ug_g: f64,
    ratios: &[(String, f64)],
    default_ratio: f64,
) -> Result<String, String> {
    let ratio_table = if ratios.is_empty() {
        "{}".to_owned()
    } else {
        let body: Vec<String> = ratios
            .iter()
            .map(|(name, value)| format!("{name:?} = {value:?}"))
            .collect();
        format!("{{ {} }}", body.join(", "))
    };
    let edits: [(&str, String); 5] = [
        ("description", format!("description = {beam:?}")),
        ("approach", format!("approach = {approach:?}")),
        ("blood_ug_g", format!("blood_ug_g = {blood_ug_g:?}")),
        ("ratios", format!("ratios = {ratio_table}")),
        (
            "default_ratio",
            format!("default_ratio = {default_ratio:?}"),
        ),
    ];
    let mut seen = [0_usize; 5];
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let key = line.split('=').next().unwrap_or("").trim();
        let mut replaced = false;
        if !line.trim_start().starts_with('#') && line.contains('=') {
            for (i, (name, replacement)) in edits.iter().enumerate() {
                if key == *name {
                    out.push_str(replacement);
                    seen[i] += 1;
                    replaced = true;
                    break;
                }
            }
        }
        if !replaced {
            out.push_str(line);
        }
        out.push('\n');
    }
    for (i, (name, _)) in edits.iter().enumerate() {
        if seen[i] != 1 {
            return Err(format!(
                "project.toml: expected exactly one `{name}` line, found {}",
                seen[i]
            ));
        }
    }
    Ok(out)
}

/// Value of the `dicom = "…"` line, if present.
fn toml_dicom_dir(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix("dicom")?.trim_start().strip_prefix('=')?;
        let value = value.trim().strip_prefix('"')?;
        Some(value[..value.find('"')?].to_owned())
    })
}

#[derive(Debug, Default, PartialEq)]
struct ManifestView {
    project_id: String,
    steps: [Option<StepState>; 7],
    /// Recorded outputs of `05-boron` (paths relative to the project).
    boron_outputs: Vec<String>,
}

fn parse_manifest(text: &str) -> Result<ManifestView, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let schema = value
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if !schema.contains("project-run") {
        return Err(format!("unexpected manifest schema {schema:?}"));
    }
    let mut view = ManifestView {
        project_id: value
            .get("project_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        ..ManifestView::default()
    };
    for step in value
        .get("steps")
        .and_then(|v| v.as_array())
        .map_or(&[][..], Vec::as_slice)
    {
        let id = step.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        let Some(index) = STEP_LABELS
            .iter()
            .position(|label| id.split_once('-').is_some_and(|(_, name)| name == *label))
        else {
            continue;
        };
        let status = step.get("status").and_then(|v| v.as_str()).unwrap_or("");
        let seconds = step.get("seconds").and_then(|v| v.as_f64()).unwrap_or(0.0);
        view.steps[index] = Some(match status {
            "complete" => StepState::Done(seconds),
            "failed" => StepState::Failed(
                step.get("error")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            ),
            _ => StepState::Pending,
        });
        if index == 4 && status == "complete" {
            view.boron_outputs =
                step.get("outputs")
                    .and_then(|v| v.as_array())
                    .map_or_else(Vec::new, |outs| {
                        outs.iter()
                            .filter_map(|o| o.get("path").and_then(|p| p.as_str()))
                            .map(str::to_owned)
                            .collect()
                    });
        }
    }
    Ok(view)
}

/// Recursively read every regular file under `dir` as (relative name, bytes).
fn read_dicom_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.is_file() {
                let name = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                out.push((
                    name,
                    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
                ));
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    if files.is_empty() {
        return Err(format!("{}: no files found", dir.display()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Default `openbnct` binary: a sibling of this executable when present
/// (cargo target dir / install dir), else `openbnct` on `PATH` — the same
/// default the "Run a subcommand" panel uses.
fn default_program() -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(if cfg!(windows) {
            "openbnct.exe"
        } else {
            "openbnct"
        });
        if sibling.is_file() {
            return sibling.display().to_string();
        }
    }
    "openbnct".into()
}

// ---------------------------------------------------------------------------
// Panel state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Builtins,
    Init,
    Run,
}

type NumericForm = (f64, f64, f64, Vec<(String, f64)>);
type ScanResult = Result<ImportedStudy, String>;

struct RatioRow {
    roi: String,
    listed: bool,
    ratio: String,
}

pub(crate) struct ProjectPanel {
    program: String,
    dicom_dir: String,
    output_dir: String,
    spacing_mm: String,
    blood_ug_g: String,
    default_ratio: String,
    approach: usize,
    beam: String,
    beams: Vec<String>,
    builtins_fetched: bool,
    target: String,
    rois: Vec<String>,
    rows: Vec<RatioRow>,
    scan_rx: Option<Receiver<ScanResult>>,
    /// Hand the next scan result to the app as the active case.
    scan_delivers_case: bool,
    study: Option<ImportedStudy>,
    scan_status: Option<String>,
    // Project / run
    project_dir: String,
    force: bool,
    job: Option<(JobKind, Job)>,
    job_output: Vec<String>,
    log: Vec<String>,
    steps: [StepState; 7],
    sn: Option<(u32, u32, f64)>,
    status: Option<String>,
    error: Option<String>,
    toml_text: Option<String>,
    report: Option<String>,
    boron_outputs: Vec<String>,
    // Requests for the app (which owns the case and the dose panel).
    dose_request: Option<PathBuf>,
    case_request: Option<ImportedStudy>,
}

impl Default for ProjectPanel {
    fn default() -> Self {
        Self {
            program: default_program(),
            dicom_dir: String::new(),
            output_dir: String::new(),
            spacing_mm: "5".into(),
            blood_ug_g: "25".into(),
            default_ratio: "1.0".into(),
            approach: 0,
            beam: FALLBACK_BEAM.into(),
            beams: vec![FALLBACK_BEAM.into()],
            builtins_fetched: false,
            target: String::new(),
            rois: Vec::new(),
            rows: Vec::new(),
            scan_rx: None,
            scan_delivers_case: false,
            study: None,
            scan_status: None,
            project_dir: String::new(),
            force: false,
            job: None,
            job_output: Vec::new(),
            log: Vec::new(),
            steps: std::array::from_fn(|_| StepState::Pending),
            sn: None,
            status: None,
            error: None,
            toml_text: None,
            report: None,
            boron_outputs: Vec::new(),
            dose_request: None,
            case_request: None,
        }
    }
}

impl ProjectPanel {
    /// Dose bundle the app should load into the Dose workspace.
    pub(crate) fn take_dose_request(&mut self) -> Option<PathBuf> {
        self.dose_request.take()
    }

    /// Imported study the app should make the active case.
    pub(crate) fn take_case_request(&mut self) -> Option<ImportedStudy> {
        self.case_request.take()
    }

    fn running(&self) -> bool {
        self.job.is_some()
    }

    fn project_path(&self) -> PathBuf {
        PathBuf::from(self.project_dir.trim())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn spawn(&mut self, kind: JobKind, args: Vec<String>, timeout: Duration) {
        self.job_output.clear();
        match Job::spawn(self.program.trim(), &args, timeout) {
            Ok(job) => self.job = Some((kind, job)),
            Err(error) => self.error = Some(error),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn spawn(&mut self, _kind: JobKind, _args: Vec<String>, _timeout: Duration) {
        self.error = Some("process execution requires the native build".into());
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_scan(&mut self, dir: PathBuf, deliver_case: bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = read_dicom_files(&dir).and_then(|files| {
                openbnct_dicom::import_study_from_files(&files).map_err(|e| e.to_string())
            });
            let _ = tx.send(result);
        });
        self.scan_rx = Some(rx);
        self.scan_delivers_case = deliver_case;
        self.scan_status = Some("reading study…".into());
    }

    #[cfg(target_arch = "wasm32")]
    fn start_scan(&mut self, _dir: PathBuf, _deliver_case: bool) {}

    fn poll_scan(&mut self) {
        let Some(rx) = &self.scan_rx else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.scan_rx = None;
        match result {
            Ok(study) => {
                if self.scan_delivers_case {
                    self.scan_status = Some("CT and ROIs sent to the Geometry workspace".into());
                    self.case_request = Some(study);
                } else {
                    self.rois = study.rois.iter().map(|r| r.name.clone()).collect();
                    self.rows = self
                        .rois
                        .iter()
                        .map(|roi| RatioRow {
                            roi: roi.clone(),
                            listed: false,
                            ratio: "1.0".into(),
                        })
                        .collect();
                    if !self.rois.contains(&self.target) {
                        self.target = self.rois.first().cloned().unwrap_or_default();
                    }
                    self.set_target_row();
                    self.scan_status = Some(format!(
                        "{} ROI(s) detected, {} DICOM members",
                        self.rois.len(),
                        study.member_count
                    ));
                    self.study = Some(study);
                }
            }
            Err(error) => {
                self.scan_status = None;
                self.error = Some(format!("study scan failed: {error}"));
            }
        }
        self.scan_delivers_case = false;
    }

    /// The beam target is listed by default with the same placeholder
    /// ratio `project init` uses.
    fn set_target_row(&mut self) {
        for row in &mut self.rows {
            if row.roi == self.target && !row.listed {
                row.listed = true;
                row.ratio = "3.5".into();
            }
        }
    }

    fn reset_run_view(&mut self) {
        self.steps = std::array::from_fn(|_| StepState::Pending);
        self.sn = None;
        self.log.clear();
        self.status = None;
        self.error = None;
        self.report = None;
    }

    /// Reload toml, manifest and report from the project folder.
    fn refresh_from_disk(&mut self) {
        let dir = self.project_path();
        self.toml_text = std::fs::read_to_string(dir.join("project.toml")).ok();
        self.report = std::fs::read_to_string(dir.join("out/report.md")).ok();
        self.boron_outputs.clear();
        match std::fs::read_to_string(dir.join("out/run-manifest.json")) {
            Ok(text) => match parse_manifest(&text) {
                Ok(view) => {
                    for (slot, recorded) in self.steps.iter_mut().zip(view.steps) {
                        *slot = recorded.unwrap_or(StepState::Pending);
                    }
                    self.boron_outputs = view.boron_outputs;
                    self.status = Some(format!("manifest: project {}", view.project_id));
                }
                Err(error) => self.error = Some(format!("run-manifest.json: {error}")),
            },
            Err(_) => {
                self.steps = std::array::from_fn(|_| StepState::Pending);
                self.status = Some("no run manifest yet — press Run".into());
            }
        }
    }

    fn open_project(&mut self) {
        self.error = None;
        let dir = self.project_path();
        if !dir.join("project.toml").is_file() {
            self.error = Some(format!("{}: no project.toml in this folder", dir.display()));
            return;
        }
        self.reset_run_view();
        self.refresh_from_disk();
        if let Some(text) = &self.toml_text
            && let Some(dicom) = toml_dicom_dir(text)
        {
            let dicom = PathBuf::from(dicom);
            self.dicom_dir = if dicom.is_absolute() {
                dicom
            } else {
                dir.join(dicom)
            }
            .display()
            .to_string();
        }
    }

    fn dose_bundle_path(&self) -> Option<PathBuf> {
        self.boron_outputs
            .iter()
            .any(|p| p.trim_end_matches('/') == "out/05-boron")
            .then(|| self.project_path().join("out/05-boron/dose.json"))
            .filter(|p| p.is_file())
    }

    fn parse_number(text: &str, what: &str) -> Result<f64, String> {
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{what} must be a number"))
    }

    /// (spacing, blood, default ratio, listed ratios) from the form.
    fn numeric_form(&self) -> Result<NumericForm, String> {
        let spacing = Self::parse_number(&self.spacing_mm, "spacing")?;
        if spacing <= 0.0 {
            return Err("spacing must be positive".into());
        }
        let blood = Self::parse_number(&self.blood_ug_g, "blood boron")?;
        let default_ratio = Self::parse_number(&self.default_ratio, "default ratio")?;
        if blood < 0.0 || default_ratio < 0.0 {
            return Err("boron concentration and ratios must be non-negative".into());
        }
        let mut ratios = Vec::new();
        for row in self.rows.iter().filter(|r| r.listed) {
            let value = Self::parse_number(&row.ratio, &format!("ratio for {}", row.roi))?;
            if value < 0.0 {
                return Err(format!("ratio for {} must be non-negative", row.roi));
            }
            ratios.push((row.roi.clone(), value));
        }
        Ok((spacing, blood, default_ratio, ratios))
    }

    fn validate_new(&self) -> Result<(), String> {
        if self.dicom_dir.trim().is_empty() || !Path::new(self.dicom_dir.trim()).is_dir() {
            return Err("choose a DICOM folder (CT series + RT Structure Set)".into());
        }
        if self.output_dir.trim().is_empty() {
            return Err("choose a new project folder".into());
        }
        if Path::new(self.output_dir.trim()).exists() {
            return Err("the project folder already exists — choose a new one".into());
        }
        if self.target.is_empty() {
            return Err("scan the study and pick the target ROI".into());
        }
        self.numeric_form().map(|_| ())
    }

    fn start_init(&mut self) {
        self.error = None;
        if let Err(error) = self.validate_new() {
            self.error = Some(error);
            return;
        }
        let args = vec![
            "project".into(),
            "init".into(),
            "--dicom".into(),
            self.dicom_dir.trim().into(),
            "--output".into(),
            self.output_dir.trim().into(),
            "--target".into(),
            self.target.clone(),
            "--spacing-mm".into(),
            self.spacing_mm.trim().into(),
        ];
        self.reset_run_view();
        self.toml_text = None;
        self.spawn(JobKind::Init, args, HELPER_TIMEOUT);
    }

    fn finish_init(&mut self) {
        let dir = PathBuf::from(self.output_dir.trim());
        let patched = self
            .numeric_form()
            .and_then(|(_, blood, default_ratio, ratios)| {
                let path = dir.join("project.toml");
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let text = patch_project_toml(
                    &text,
                    &self.beam,
                    APPROACHES[self.approach],
                    blood,
                    &ratios,
                    default_ratio,
                )?;
                std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))
            });
        match patched {
            Ok(()) => {
                self.project_dir = self.output_dir.trim().to_owned();
                self.refresh_from_disk();
                self.status = Some("project created — review project.toml, then Run".into());
            }
            Err(error) => self.error = Some(format!("project created but patch failed: {error}")),
        }
    }

    fn start_run(&mut self) {
        self.error = None;
        let dir = self.project_path();
        if !dir.join("project.toml").is_file() {
            self.error = Some("open or create a project first".into());
            return;
        }
        self.reset_run_view();
        let mut args = vec!["project".into(), "run".into(), dir.display().to_string()];
        if self.force {
            args.push("--force".into());
        }
        self.spawn(JobKind::Run, args, RUN_TIMEOUT);
        if self.job.is_some() {
            self.status = Some("running…".into());
        }
    }

    fn cancel(&mut self) {
        if let Some((_, job)) = &mut self.job {
            job.cancel();
        }
    }

    fn ingest_run_lines(&mut self, lines: &[String]) {
        for line in lines {
            if let Some((index, event)) = parse_step_line(line) {
                self.steps[index] = match event {
                    StepEvent::Running => StepState::Running,
                    StepEvent::Skipped => StepState::Skipped,
                    StepEvent::Done(s) => StepState::Done(s),
                    StepEvent::Failed => StepState::Failed(None),
                };
                if index == 3 && matches!(self.steps[3], StepState::Running) {
                    self.sn = None;
                }
            } else if let Some(progress) = parse_sn_line(line) {
                self.sn = Some(progress);
            }
        }
        self.log.extend(lines.iter().cloned());
        if self.log.len() > LOG_CAP {
            let drop = self.log.len() - LOG_CAP;
            self.log.drain(..drop);
        }
    }

    /// Drain the active job once per frame; returns true while busy.
    fn poll(&mut self) -> bool {
        self.poll_scan();
        let Some((kind, job)) = &mut self.job else {
            return self.scan_rx.is_some();
        };
        let kind = *kind;
        let mut fresh = Vec::new();
        let running = job.poll(&mut fresh, 100_000);
        let finished = (!running).then_some((job.exit_code, job.timed_out));
        match kind {
            JobKind::Run => self.ingest_run_lines(&fresh),
            _ => {
                self.job_output.extend(fresh.iter().cloned());
                if kind == JobKind::Init {
                    self.log.extend(fresh);
                }
            }
        }
        let Some((code, timed_out)) = finished else {
            return true;
        };
        self.job = None;
        let summary = if timed_out {
            "killed — deadline exceeded".to_owned()
        } else {
            match code {
                Some(0) => "exit 0".to_owned(),
                Some(code) => format!("exit {code}"),
                None => "terminated".to_owned(),
            }
        };
        match kind {
            JobKind::Builtins => {
                let beams = parse_builtin_beams(&self.job_output);
                if code == Some(0) && !beams.is_empty() {
                    self.beams = beams;
                    if !self.beams.contains(&self.beam) {
                        self.beam = self.beams[0].clone();
                    }
                }
            }
            JobKind::Init => {
                if code == Some(0) && !timed_out {
                    self.finish_init();
                } else {
                    self.error = Some(format!("project init failed ({summary})"));
                }
            }
            JobKind::Run => {
                self.refresh_from_disk();
                if code != Some(0) || timed_out {
                    self.error = Some(format!(
                        "project run did not finish ({summary}); see the log and the step table"
                    ));
                } else {
                    self.status = Some(format!("run complete ({summary})"));
                }
            }
        }
        false
    }

    fn overall_fraction(&self) -> f32 {
        let mut total = 0.0_f32;
        for (index, step) in self.steps.iter().enumerate() {
            total += match step {
                StepState::Done(_) | StepState::Skipped => 1.0,
                StepState::Running if index == 3 => self
                    .sn
                    .map_or(0.0, |(k, max, _)| (k as f32 / max.max(1) as f32).min(1.0)),
                _ => 0.0,
            };
        }
        total / STEP_LABELS.len() as f32
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

fn step_state_text(state: &StepState) -> String {
    match state {
        StepState::Pending => "not run".into(),
        StepState::Running => "running".into(),
        StepState::Done(seconds) => format!("done · {seconds:.1} s"),
        StepState::Skipped => "skipped (up to date)".into(),
        StepState::Failed(Some(error)) => format!("FAILED · {error}"),
        StepState::Failed(None) => "FAILED".into(),
    }
}

/// Plain-text rendering of report.md: headings emphasised, tables and
/// code kept monospace, everything else wrapped.
fn show_markdown(ui: &mut egui::Ui, text: &str) {
    let mut in_code = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let trimmed = line.trim_start();
        if in_code || trimmed.starts_with('|') {
            ui.monospace(line);
        } else if let Some(title) = trimmed.strip_prefix("### ") {
            ui.label(egui::RichText::new(title).strong());
        } else if let Some(title) = trimmed.strip_prefix("## ") {
            ui.label(egui::RichText::new(title).strong().size(16.0));
        } else if let Some(title) = trimmed.strip_prefix("# ") {
            ui.label(egui::RichText::new(title).strong().size(19.0));
        } else if trimmed.is_empty() {
            ui.add_space(4.0);
        } else {
            ui.label(line);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pick_folder(current: &str) -> Option<String> {
    let mut dialog = rfd::FileDialog::new();
    if Path::new(current.trim()).is_dir() {
        dialog = dialog.set_directory(current.trim());
    }
    dialog.pick_folder().map(|p| p.display().to_string())
}

#[cfg(target_arch = "wasm32")]
fn pick_folder(_current: &str) -> Option<String> {
    None
}

fn card(ui: &mut egui::Ui, theme: Theme, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme.card_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
    ui.add_space(10.0);
}

pub(crate) fn show_project_workspace(
    ui: &mut egui::Ui,
    panel: &mut ProjectPanel,
    language: Language,
    theme: Theme,
) {
    show_workspace_heading(
        ui,
        theme,
        t!(
            language,
            en = "Project",
            ja = "プロジェクト",
            it = "Progetto",
            zh = "项目",
            es = "Proyecto"
        ),
        t!(
            language,
            en = "DICOM study to boron-scaled dose through the project runner. Research software; not for clinical use.",
            ja = "DICOM スタディからホウ素換算線量まで、プロジェクトランナーで実行します。研究用ソフトウェアであり、臨床使用はできません。"
        ),
    );
    if cfg!(target_arch = "wasm32") {
        ui.label(t!(
            language,
            en = "Running a project launches the openbnct command-line tool and reads local folders, which browsers cannot do. Use the desktop build for this workspace; the web build stays view-only.",
            ja = "プロジェクトの実行は openbnct コマンドラインツールの起動とローカルフォルダの読み取りを伴うため、ブラウザでは行えません。このワークスペースにはデスクトップ版をご利用ください。Web 版は閲覧専用です。"
        ));
        return;
    }

    let busy_now = panel.poll();
    if busy_now {
        ui.ctx().request_repaint_after(Duration::from_millis(150));
    }
    if !panel.builtins_fetched && !busy_now {
        panel.builtins_fetched = true;
        panel.spawn(
            JobKind::Builtins,
            vec!["project".into(), "builtins".into()],
            HELPER_TIMEOUT,
        );
    }
    let busy = panel.running() || panel.scan_rx.is_some();

    show_program_and_open(ui, panel, language, theme, busy);
    show_new_project(ui, panel, language, theme, busy);
    show_run(ui, panel, language, theme);
    show_results(ui, panel, language, theme);
}

fn show_program_and_open(
    ui: &mut egui::Ui,
    panel: &mut ProjectPanel,
    language: Language,
    theme: Theme,
    busy: bool,
) {
    card(ui, theme, |ui| {
        ui.horizontal(|ui| {
            ui.label(t!(
                language,
                en = "openbnct binary",
                ja = "openbnct バイナリ"
            ));
            ui.add(egui::TextEdit::singleline(&mut panel.program).desired_width(320.0));
        });
        ui.horizontal(|ui| {
            ui.label(t!(
                language,
                en = "Existing project",
                ja = "既存のプロジェクト"
            ));
            ui.add(
                egui::TextEdit::singleline(&mut panel.project_dir)
                    .desired_width(320.0)
                    .hint_text("…/my-project"),
            );
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(t!(language, en = "Browse…", ja = "参照…")),
                )
                .clicked()
                && let Some(dir) = pick_folder(&panel.project_dir)
            {
                panel.project_dir = dir;
                panel.open_project();
            }
            if ui
                .add_enabled(
                    !busy,
                    egui::Button::new(t!(
                        language,
                        en = "Open project…",
                        ja = "プロジェクトを開く"
                    )),
                )
                .clicked()
            {
                panel.open_project();
            }
        });
    });
}

fn show_new_project(
    ui: &mut egui::Ui,
    panel: &mut ProjectPanel,
    language: Language,
    theme: Theme,
    busy: bool,
) {
    card(ui, theme, |ui| {
        ui.heading(t!(
            language,
            en = "1 · New project",
            ja = "1 · 新規プロジェクト"
        ));
        egui::Grid::new("project-new-grid")
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label(t!(language, en = "DICOM folder", ja = "DICOM フォルダ"));
                ui.add(egui::TextEdit::singleline(&mut panel.dicom_dir).desired_width(380.0));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(t!(language, en = "Browse…", ja = "参照…")),
                        )
                        .clicked()
                        && let Some(dir) = pick_folder(&panel.dicom_dir)
                    {
                        panel.dicom_dir = dir;
                    }
                    if ui
                        .add_enabled(
                            !busy && !panel.dicom_dir.trim().is_empty(),
                            egui::Button::new(t!(language, en = "Scan ROIs", ja = "ROI を検出")),
                        )
                        .clicked()
                    {
                        panel.error = None;
                        let dir = PathBuf::from(panel.dicom_dir.trim());
                        panel.start_scan(dir, false);
                    }
                });
                ui.end_row();

                ui.label(t!(
                    language,
                    en = "Project folder (new)",
                    ja = "プロジェクトフォルダ(新規)"
                ));
                ui.add(egui::TextEdit::singleline(&mut panel.output_dir).desired_width(380.0));
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(t!(language, en = "Browse…", ja = "参照…")),
                    )
                    .clicked()
                    && let Some(dir) = pick_folder(&panel.output_dir)
                {
                    // The picked folder is a parent; the project itself
                    // must not exist yet.
                    panel.output_dir = Path::new(&dir).join("bnct-project").display().to_string();
                }
                ui.end_row();

                ui.label(t!(language, en = "Target ROI", ja = "標的 ROI"));
                let previous = panel.target.clone();
                egui::ComboBox::from_id_salt("project-target")
                    .width(300.0)
                    .selected_text(if panel.target.is_empty() {
                        t!(
                            language,
                            en = "scan a study first",
                            ja = "先にスタディを検出"
                        )
                    } else {
                        panel.target.as_str()
                    })
                    .show_ui(ui, |ui| {
                        for roi in panel.rois.clone() {
                            ui.selectable_value(&mut panel.target, roi.clone(), roi);
                        }
                    });
                if panel.target != previous {
                    panel.set_target_row();
                }
                if let Some(status) = &panel.scan_status {
                    ui.label(status);
                }
                ui.end_row();

                ui.label(t!(language, en = "Spacing (mm)", ja = "間隔 (mm)"));
                ui.add(egui::TextEdit::singleline(&mut panel.spacing_mm).desired_width(80.0));
                ui.end_row();

                ui.label(t!(language, en = "Beam", ja = "ビーム"));
                egui::ComboBox::from_id_salt("project-beam")
                    .width(300.0)
                    .selected_text(panel.beam.clone())
                    .show_ui(ui, |ui| {
                        for beam in panel.beams.clone() {
                            ui.selectable_value(&mut panel.beam, beam.clone(), beam);
                        }
                    });
                ui.end_row();

                ui.label(t!(language, en = "Approach axis", ja = "照射方向"));
                egui::ComboBox::from_id_salt("project-approach")
                    .width(80.0)
                    .selected_text(APPROACHES[panel.approach])
                    .show_ui(ui, |ui| {
                        for (index, axis) in APPROACHES.iter().enumerate() {
                            ui.selectable_value(&mut panel.approach, index, *axis);
                        }
                    });
                ui.label(t!(
                    language,
                    en = "beam travel direction (LPS)",
                    ja = "ビーム進行方向 (LPS)"
                ));
                ui.end_row();

                ui.label(t!(
                    language,
                    en = "Blood boron (µg/g)",
                    ja = "血中ホウ素 (µg/g)"
                ));
                ui.add(egui::TextEdit::singleline(&mut panel.blood_ug_g).desired_width(80.0));
                ui.end_row();

                ui.label(t!(language, en = "Default ratio", ja = "既定の比"));
                ui.add(egui::TextEdit::singleline(&mut panel.default_ratio).desired_width(80.0));
                ui.label(t!(
                    language,
                    en = "tissue:blood, for voxels in no listed ROI",
                    ja = "組織:血液。リスト外のボクセル用"
                ));
                ui.end_row();
            });

        if !panel.rows.is_empty() {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "Tissue:blood boron ratios per ROI (illustrative placeholders — enter values that belong to your study; overlapping ROIs: the smaller one wins)",
                    ja = "ROI ごとの組織:血液ホウ素比(値は例示用プレースホルダです。ご自身の研究の値を入力してください。ROI が重なる場合は小さい方が優先)"
                ))
                .strong(),
            );
            egui::ScrollArea::vertical()
                .id_salt("project-ratio-scroll")
                .max_height(160.0)
                .show(ui, |ui| {
                    egui::Grid::new("project-ratio-grid")
                        .num_columns(3)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for row in &mut panel.rows {
                                ui.checkbox(&mut row.listed, "");
                                ui.label(&row.roi);
                                ui.add_enabled(
                                    row.listed,
                                    egui::TextEdit::singleline(&mut row.ratio).desired_width(60.0),
                                );
                                ui.end_row();
                            }
                        });
                });
        }

        ui.add_space(6.0);
        if ui
            .add_enabled(
                !busy,
                egui::Button::new(t!(
                    language,
                    en = "Create project",
                    ja = "プロジェクトを作成"
                )),
            )
            .on_hover_text(t!(
                language,
                en = "Runs `openbnct project init`, then sets beam, approach and boron values in project.toml.",
                ja = "`openbnct project init` を実行し、project.toml のビーム・照射方向・ホウ素値を設定します。"
            ))
            .clicked()
        {
            panel.start_init();
        }
    });
}

fn show_run(ui: &mut egui::Ui, panel: &mut ProjectPanel, language: Language, theme: Theme) {
    card(ui, theme, |ui| {
        ui.heading(t!(language, en = "2 · Run", ja = "2 · 実行"));
        if let Some(text) = panel.toml_text.clone() {
            ui.label(
                egui::RichText::new(format!("{}/project.toml", panel.project_dir.trim()))
                    .color(theme.text_dim),
            );
            egui::ScrollArea::vertical()
                .id_salt("project-toml-scroll")
                .max_height(150.0)
                .show(ui, |ui| {
                    // Read-only view: one non-editable label per line.
                    for line in text.lines() {
                        ui.monospace(line);
                    }
                });
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(t!(
                        language,
                        en = "Read-only here. Open project.toml in your editor to change it, then press Run.",
                        ja = "ここでは読み取り専用です。エディタで project.toml を開いて編集してから実行してください。"
                    ))
                    .color(theme.text_dim),
                );
                if ui
                    .small_button(t!(language, en = "Copy path", ja = "パスをコピー"))
                    .clicked()
                {
                    let path = panel.project_path().join("project.toml");
                    ui.ctx().copy_text(path.display().to_string());
                }
            });
        }
        ui.add_space(6.0);
        let running = panel.job.as_ref().is_some_and(|(k, _)| *k == JobKind::Run);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !panel.running(),
                    egui::Button::new(t!(language, en = "Run project", ja = "プロジェクトを実行")),
                )
                .clicked()
            {
                panel.start_run();
            }
            if ui
                .add_enabled(
                    running,
                    egui::Button::new(t!(language, en = "Cancel", ja = "キャンセル")),
                )
                .clicked()
            {
                panel.cancel();
            }
            ui.checkbox(
                &mut panel.force,
                t!(
                    language,
                    en = "force (rerun all steps)",
                    ja = "強制(全ステップ再実行)"
                ),
            );
            ui.label(
                egui::RichText::new(t!(language, en = "timeout 2 h", ja = "タイムアウト 2 時間"))
                    .color(theme.text_dim),
            );
        });
        if let Some(status) = &panel.status {
            ui.label(status);
        }
        if let Some(error) = &panel.error {
            ui.colored_label(theme.error, error);
        }
        ui.add_space(6.0);
        ui.add(
            egui::ProgressBar::new(panel.overall_fraction())
                .show_percentage()
                .text(t!(language, en = "overall", ja = "全体")),
        );
        if let Some((k, max, residual)) = panel.sn {
            ui.add(
                egui::ProgressBar::new((k as f32 / max.max(1) as f32).min(1.0))
                    .text(format!("transport outer {k}/{max}")),
            );
            ui.monospace(format!("residual {residual:.3e}"));
        }
        egui::Grid::new("project-step-grid")
            .num_columns(3)
            .spacing([14.0, 3.0])
            .show(ui, |ui| {
                for (index, label) in STEP_LABELS.iter().enumerate() {
                    ui.monospace(format!("{}/7", index + 1));
                    ui.monospace(*label);
                    let state = &panel.steps[index];
                    let text = step_state_text(state);
                    match state {
                        StepState::Failed(_) => {
                            ui.colored_label(theme.error, text);
                        }
                        StepState::Running => {
                            ui.colored_label(theme.brand, text);
                        }
                        _ => {
                            ui.label(text);
                        }
                    }
                    ui.end_row();
                }
            });
        if !panel.log.is_empty() {
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .id_salt("project-log-scroll")
                .max_height(160.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &panel.log {
                        ui.monospace(line);
                    }
                });
        }
    });
}

fn show_results(ui: &mut egui::Ui, panel: &mut ProjectPanel, language: Language, theme: Theme) {
    card(ui, theme, |ui| {
        ui.heading(t!(language, en = "3 · Results", ja = "3 · 結果"));
        let dose = panel.dose_bundle_path();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    dose.is_some() && !panel.running(),
                    egui::Button::new(t!(
                        language,
                        en = "Load boron dose into Dose workspace",
                        ja = "ホウ素線量を線量ワークスペースに読み込む"
                    )),
                )
                .on_disabled_hover_text(t!(
                    language,
                    en = "needs a completed boron step in out/run-manifest.json",
                    ja = "out/run-manifest.json に完了済みの boron ステップが必要です"
                ))
                .clicked()
                && let Some(path) = dose.clone()
            {
                panel.dose_request = Some(path);
            }
            let dicom = panel.dicom_dir.trim().to_owned();
            if ui
                .add_enabled(
                    panel.scan_rx.is_none() && Path::new(&dicom).is_dir(),
                    egui::Button::new(t!(
                        language,
                        en = "Load CT and ROIs into Geometry",
                        ja = "CT と ROI をジオメトリに読み込む"
                    )),
                )
                .clicked()
            {
                if let Some(study) = panel.study.take() {
                    panel.case_request = Some(study);
                } else {
                    panel.start_scan(PathBuf::from(dicom), true);
                }
            }
        });
        ui.add_space(6.0);
        match panel.report.clone() {
            Some(report) => {
                egui::ScrollArea::vertical()
                    .id_salt("project-report-scroll")
                    .max_height(380.0)
                    .show(ui, |ui| show_markdown(ui, &report));
            }
            None => {
                ui.label(
                    egui::RichText::new(t!(
                        language,
                        en = "report.md appears here once the report step has run.",
                        ja = "report ステップの実行後に report.md がここに表示されます。"
                    ))
                    .color(theme.text_dim),
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_lines_parse() {
        assert_eq!(
            parse_step_line("[4/7] transport  done in 12.3 s"),
            Some((3, StepEvent::Done(12.3)))
        );
        assert_eq!(
            parse_step_line("[1/7] import     running"),
            Some((0, StepEvent::Running))
        );
        assert_eq!(
            parse_step_line("[2/7] calibrate  skipped (inputs, options and outputs unchanged)"),
            Some((1, StepEvent::Skipped))
        );
        assert_eq!(
            parse_step_line("[5/7] boron      FAILED after 1.0 s"),
            Some((4, StepEvent::Failed))
        );
        assert_eq!(parse_step_line("[sn] outer 3/64: residual 1e-3"), None);
        assert_eq!(parse_step_line("[9/7] x running"), None);
    }

    #[test]
    fn sn_lines_parse() {
        assert_eq!(
            parse_sn_line("[sn] outer 3/64: residual 1.234e-3 (5.0s)"),
            Some((3, 64, 1.234e-3))
        );
        assert_eq!(parse_sn_line("[4/7] transport running"), None);
    }

    #[test]
    fn toml_patch_replaces_each_key_once() {
        let text = "# ROIs: GTV\n[project]\nid = \"x\"\n[beam]\ndescription = \"builtin:beams/fir1-k63\"    # c\ntarget = \"GTV\"\napproach = \"+x\"    # c\n[boron]\nblood_ug_g = 25.0\nratios = { \"GTV\" = 3.5 }\ndefault_ratio = 1.0    # c\n";
        let out = patch_project_toml(
            text,
            "builtin:beams/other",
            "-z",
            30.0,
            &[("GTV".into(), 3.5), ("SKIN".into(), 1.0)],
            0.5,
        )
        .unwrap();
        assert!(out.contains("description = \"builtin:beams/other\"\n"));
        assert!(out.contains("approach = \"-z\"\n"));
        assert!(out.contains("blood_ug_g = 30.0\n"));
        assert!(out.contains("ratios = { \"GTV\" = 3.5, \"SKIN\" = 1.0 }\n"));
        assert!(out.contains("default_ratio = 0.5\n"));
        assert!(out.contains("target = \"GTV\""));
        assert!(patch_project_toml("[beam]\n", "b", "+x", 1.0, &[], 1.0).is_err());
    }

    #[test]
    fn builtins_and_dicom_key_parse() {
        let out = vec![
            "builtin:tissue/material-air-dry  (a.json, 10 bytes)".to_owned(),
            "builtin:beams/fir1-k63  (beam.json, 99 bytes)".to_owned(),
        ];
        assert_eq!(parse_builtin_beams(&out), vec!["builtin:beams/fir1-k63"]);
        assert_eq!(
            toml_dicom_dir("[imaging]\ndicom = \"../study\"    # c\nspacing_mm = 5.0\n"),
            Some("../study".into())
        );
    }

    #[test]
    fn manifest_parses_steps_and_boron_outputs() {
        let text = r#"{"schema_version":"openbnct.project-run/0.1.0","project_id":"p",
            "steps":[{"id":"01-import","status":"complete","seconds":1.5,"outputs":[]},
                     {"id":"05-boron","status":"complete","seconds":2.0,"outputs":[{"path":"out/05-boron"}]},
                     {"id":"06-metrics","status":"failed","seconds":0.1,"error":"boom","outputs":[]}]}"#;
        let view = parse_manifest(text).unwrap();
        assert_eq!(view.steps[0], Some(StepState::Done(1.5)));
        assert_eq!(view.steps[5], Some(StepState::Failed(Some("boom".into()))));
        assert_eq!(view.boron_outputs, vec!["out/05-boron"]);
        assert!(parse_manifest("{\"schema_version\":\"x\"}").is_err());
    }
}
