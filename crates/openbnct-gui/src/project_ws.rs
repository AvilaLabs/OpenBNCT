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
const FALLBACK_BEAM: &str = "builtin:beams/fir1-k63-ineel";

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

/// Everything the form writes into the `project.toml` that `project init`
/// created.
struct TomlEdits<'a> {
    beam: &'a str,
    approach: &'a str,
    blood_ug_g: f64,
    ratios: &'a [(String, f64)],
    default_ratio: f64,
    /// `[transport] order` (4 or 8).
    order: u32,
    photon_transport: bool,
    source_weighting: &'a str,
    /// `[imaging] crop` / `crop_margin_mm`, written only when the runner
    /// knows them and the user left the default.
    crop: Option<&'a str>,
    crop_margin_mm: Option<f64>,
}

/// True when any line of a `--help` text mentions `word` (the option exists).
fn help_mentions(output: &[String], word: &str) -> bool {
    output.iter().any(|l| l.to_ascii_lowercase().contains(word))
}

/// Edit values in the `project.toml` that `project init` wrote. Line-based
/// so the header comment (detected ROIs) survives; every key must be found
/// exactly once or the patch is refused. The optional `[imaging]` crop keys
/// are replaced when present and inserted after `spacing_mm` otherwise.
fn patch_project_toml(text: &str, edits: &TomlEdits<'_>) -> Result<String, String> {
    let ratio_table = if edits.ratios.is_empty() {
        "{}".to_owned()
    } else {
        let body: Vec<String> = edits
            .ratios
            .iter()
            .map(|(name, value)| format!("{name:?} = {value:?}"))
            .collect();
        format!("{{ {} }}", body.join(", "))
    };
    let required: [(&str, String); 8] = [
        ("description", format!("description = {:?}", edits.beam)),
        ("approach", format!("approach = {:?}", edits.approach)),
        ("blood_ug_g", format!("blood_ug_g = {:?}", edits.blood_ug_g)),
        ("ratios", format!("ratios = {ratio_table}")),
        (
            "default_ratio",
            format!("default_ratio = {:?}", edits.default_ratio),
        ),
        ("order", format!("order = {}", edits.order)),
        (
            "photon_transport",
            format!("photon_transport = {}", edits.photon_transport),
        ),
        (
            "source_weighting",
            format!("source_weighting = {:?}", edits.source_weighting),
        ),
    ];
    let mut optional: Vec<(&str, String)> = Vec::new();
    if let Some(crop) = edits.crop {
        optional.push(("crop", format!("crop = {crop:?}")));
    }
    if let Some(margin) = edits.crop_margin_mm {
        optional.push(("crop_margin_mm", format!("crop_margin_mm = {margin:?}")));
    }
    let mut seen = [0_usize; 8];
    // Keys already in the file are replaced in place, never inserted again.
    let mut optional_seen: Vec<bool> = optional
        .iter()
        .map(|(name, _)| {
            text.lines().any(|l| {
                !l.trim_start().starts_with('#')
                    && l.split('=').next().unwrap_or("").trim() == *name
            })
        })
        .collect();
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        let key = line.split('=').next().unwrap_or("").trim();
        let mut replaced = false;
        if !line.trim_start().starts_with('#') && line.contains('=') {
            for (i, (name, replacement)) in required.iter().enumerate() {
                if key == *name {
                    out.push_str(replacement);
                    seen[i] += 1;
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                for (i, (name, replacement)) in optional.iter().enumerate() {
                    if key == *name {
                        out.push_str(replacement);
                        optional_seen[i] = true;
                        replaced = true;
                        break;
                    }
                }
            }
        }
        if !replaced {
            out.push_str(line);
        }
        out.push('\n');
        if key == "spacing_mm" && !line.trim_start().starts_with('#') {
            for (i, (_, replacement)) in optional.iter().enumerate() {
                if !optional_seen[i] {
                    out.push_str(replacement);
                    out.push('\n');
                    optional_seen[i] = true;
                }
            }
        }
    }
    for (i, (name, _)) in required.iter().enumerate() {
        if seen[i] != 1 {
            return Err(format!(
                "project.toml: expected exactly one `{name}` line, found {}",
                seen[i]
            ));
        }
    }
    if let Some(i) = optional_seen.iter().position(|done| !done) {
        return Err(format!(
            "project.toml: no `spacing_mm` line to place `{}` after",
            optional[i].0
        ));
    }
    Ok(out)
}

/// Set `key = value_literal` inside `[section]`, replacing an existing
/// line or inserting one (creating the section at the end if needed).
fn set_toml_key(text: &str, section: &str, key: &str, value_literal: &str) -> String {
    let header = format!("[{section}]");
    let new_line = format!("{key} = {value_literal}");
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let Some(start) = lines.iter().position(|l| l.trim() == header) else {
        let mut out = text.trim_end().to_owned();
        out.push_str(&format!("\n\n{header}\n{new_line}\n"));
        return out;
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map_or(lines.len(), |p| start + 1 + p);
    let existing = (start + 1..end).find(|&i| {
        let line = lines[i].trim_start();
        !line.starts_with('#') && line.split('=').next().is_some_and(|k| k.trim() == key)
    });
    match existing {
        Some(i) => lines[i] = new_line,
        None => lines.insert(start + 1, new_line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Value of a `key = "…"` line, if present.
fn toml_string_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let value = line.strip_prefix(key)?.trim_start().strip_prefix('=')?;
        let value = value.trim().strip_prefix('"')?;
        Some(value[..value.find('"')?].to_owned())
    })
}

/// Value of the `dicom = "…"` line, if present.
fn toml_dicom_dir(text: &str) -> Option<String> {
    toml_string_value(text, "dicom")
}

/// ROI names from a label-names JSON (flat `{"1": "Brain"}` or
/// `{"encoding": …, "labels": {…}}`), ordered by label number.
fn parse_label_name_list(bytes: &[u8]) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let object = value
        .as_object()
        .ok_or("label names JSON must be an object")?;
    let table = match object.get("labels") {
        Some(serde_json::Value::Object(table)) => table,
        _ => object,
    };
    let mut named: Vec<(u64, String)> = Vec::new();
    for (key, name) in table {
        let number: u64 = key
            .parse()
            .map_err(|_| format!("label key {key:?} is not a positive integer"))?;
        let name = name
            .as_str()
            .ok_or_else(|| format!("label {key} is not named by a string"))?;
        named.push((number, name.to_owned()));
    }
    if named.is_empty() {
        return Err("label names JSON names no labels".into());
    }
    named.sort();
    Ok(named.into_iter().map(|(_, name)| name).collect())
}

/// One recognised line of `openbnct project verify` output.
#[derive(Debug, PartialEq)]
enum VerifyEvent {
    /// `[verify] running OpenMC: N histories in B batches on T threads`
    Running {
        histories: u64,
        batches: u32,
    },
    /// OpenMC's own `Simulating batch N` line, when it is passed through.
    Batch(u32),
    Skipped,
    Done(f64),
    Failed,
}

fn parse_verify_line(line: &str) -> Option<VerifyEvent> {
    let trimmed = line.trim();
    if let Some(n) = trimmed.strip_prefix("Simulating batch ") {
        return n.trim().parse().ok().map(VerifyEvent::Batch);
    }
    let rest = trimmed.strip_prefix("[verify]")?.trim();
    if let Some(rest) = rest.strip_prefix("running OpenMC:") {
        let mut words = rest.split_whitespace();
        let histories = words.next()?.parse().ok()?;
        let _histories_word = words.next()?;
        let _in = words.next()?;
        let batches = words.next()?.parse().ok()?;
        return Some(VerifyEvent::Running { histories, batches });
    }
    if rest.starts_with("skipped") {
        Some(VerifyEvent::Skipped)
    } else if let Some(seconds) = rest.strip_prefix("done in ") {
        Some(VerifyEvent::Done(
            seconds.trim_end_matches(" s").trim().parse().ok()?,
        ))
    } else if rest.starts_with("FAILED") {
        Some(VerifyEvent::Failed)
    } else {
        None
    }
}

const VERIFY_HEADING: &str = "## Independent Monte Carlo check";

/// Split report.md around its "Independent Monte Carlo check" section:
/// (text before, the section, text after).
fn split_verify_section(report: &str) -> (String, Option<String>, String) {
    let lines: Vec<&str> = report.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == VERIFY_HEADING) else {
        return (report.to_owned(), None, String::new());
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("## "))
        .map_or(lines.len(), |p| start + 1 + p);
    (
        lines[..start].join("\n"),
        Some(lines[start..end].join("\n")),
        lines[end..].join("\n"),
    )
}

/// Cells of a markdown table row, or `None` for the `|---|` separator.
fn table_cells(line: &str) -> Option<Vec<String>> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    let cells: Vec<String> = inner.split('|').map(|c| c.trim().to_owned()).collect();
    cells
        .iter()
        .all(|c| !c.is_empty() && c.chars().all(|ch| matches!(ch, '-' | ':')))
        .then_some(())
        .map_or(Some(cells), |()| None)
}

/// Strip the inline emphasis markers of the report's plain markdown.
fn plain_inline(text: &str) -> String {
    text.replace("**", "").replace('`', "")
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
    /// `project init --help` / `project verify --help`, read to learn
    /// which options the installed CLI knows.
    HelpInit,
    HelpVerify,
    Init,
    Run,
    Verify,
}

const QUADRATURE: [(&str, u32); 2] = [("S4", 4), ("S8", 8)];
const WEIGHTINGS: [&str; 2] = ["uniform_in_bin", "collapse_consistent"];
const CROPS: [Option<&str>; 3] = [None, Some("body"), Some("none")];
/// `[verify] variance_reduction` choices; the first leaves the key alone.
const VARIANCE: [Option<&str>; 4] = [None, Some("none"), Some("cadis"), Some("fw-cadis")];
/// Sentinel entry of the beam list for a user-supplied beam-description file.
const CUSTOM_BEAM: &str = "(custom beam-description file)";

/// Where the new project's CT comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CtSource {
    /// DICOM folder: CT series + RT Structure Set or DICOM SEG.
    Dicom,
    /// HU NIfTI volume + labelmap + label-names JSON.
    Volume,
}

/// Options the installed `openbnct` advertises in its `--help`; the
/// controls for newer `project.toml` keys stay disabled until then, since
/// the runner rejects keys it does not know.
#[derive(Default, Clone, Copy)]
struct Capabilities {
    crop: bool,
    variance_reduction: bool,
}

/// Progress of a `project verify` job, from its stage lines.
#[derive(Default)]
struct VerifyView {
    histories: Option<u64>,
    batches: Option<u32>,
    batch_done: u32,
    started: Option<std::time::Instant>,
    finished_s: Option<f64>,
    skipped: bool,
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
    /// Startup probes done so far: builtins, init --help, verify --help.
    probe_stage: u8,
    caps: Capabilities,
    source: CtSource,
    ct_file: String,
    labels_file: String,
    label_names_file: String,
    crop: usize,
    crop_margin_mm: String,
    advanced_open: bool,
    quadrature: usize,
    photon_transport: bool,
    source_weighting: usize,
    custom_beam: String,
    verify_particles: String,
    variance: usize,
    verify: VerifyView,
    /// Frames left in which the workspace scrolls to the Results card
    /// (command-line capture only).
    scroll_results_frames: u16,
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
            probe_stage: 0,
            caps: Capabilities::default(),
            source: CtSource::Dicom,
            ct_file: String::new(),
            labels_file: String::new(),
            label_names_file: String::new(),
            crop: 0,
            crop_margin_mm: String::new(),
            advanced_open: false,
            quadrature: 1,
            photon_transport: true,
            source_weighting: 0,
            custom_beam: String::new(),
            verify_particles: "1e6".into(),
            variance: 0,
            verify: VerifyView::default(),
            scroll_results_frames: 0,
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

    // --- command-line launch hooks (see `LaunchOptions`) -----------------

    pub(crate) fn set_program(&mut self, program: &str) {
        self.program = program.to_owned();
    }

    pub(crate) fn set_launch_view(&mut self, advanced_open: bool, scroll_results: bool) {
        self.advanced_open = advanced_open;
        self.scroll_results_frames = if scroll_results { 40 } else { 0 };
    }

    /// Fill the new-project form from a folder with `hu.nii.gz`,
    /// `labels.nii.gz` and `label-names.json`.
    pub(crate) fn prefill_volume(&mut self, dir: &Path) {
        self.source = CtSource::Volume;
        self.ct_file = dir.join("hu.nii.gz").display().to_string();
        self.labels_file = dir.join("labels.nii.gz").display().to_string();
        self.label_names_file = dir.join("label-names.json").display().to_string();
        self.output_dir = dir.join("project").display().to_string();
        self.scan_label_names();
        if let Some(target) = self.rois.iter().find(|r| r.contains("TARGET")).cloned() {
            self.target = target;
            self.set_target_row();
        }
    }

    /// Open a project folder; with `load_dose`, also hand its finished
    /// boron dose to the Dose workspace.
    pub(crate) fn open_dir(&mut self, dir: &Path, load_dose: bool) {
        self.project_dir = dir.display().to_string();
        self.open_project();
        if load_dose {
            self.dose_request = self.dose_bundle_path();
        }
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
        let absolute = |value: String| {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                dir.join(path)
            }
            .display()
            .to_string()
        };
        if let Some(text) = self.toml_text.clone() {
            if let Some(dicom) = toml_dicom_dir(&text) {
                self.source = CtSource::Dicom;
                self.dicom_dir = absolute(dicom);
            } else if let Some(ct) = toml_string_value(&text, "ct_nifti") {
                self.source = CtSource::Volume;
                self.ct_file = absolute(ct);
                if let Some(labels) = toml_string_value(&text, "labels_nifti") {
                    self.labels_file = absolute(labels);
                }
                if let Some(names) = toml_string_value(&text, "label_names") {
                    self.label_names_file = absolute(names);
                }
            }
        }
    }

    fn dose_bundle_path(&self) -> Option<PathBuf> {
        self.boron_outputs
            .iter()
            .any(|p| p.trim_end_matches('/') == "out/05-boron" || p.starts_with("out/05-boron/"))
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
        match self.source {
            CtSource::Dicom => {
                if self.dicom_dir.trim().is_empty() || !Path::new(self.dicom_dir.trim()).is_dir() {
                    return Err(
                        "choose a DICOM folder (CT series + RT Structure Set or SEG)".into(),
                    );
                }
            }
            CtSource::Volume => {
                for (path, what) in [
                    (&self.ct_file, "CT volume (NIfTI)"),
                    (&self.labels_file, "labels volume (NIfTI)"),
                    (&self.label_names_file, "label-names JSON"),
                ] {
                    if !Path::new(path.trim()).is_file() {
                        return Err(format!("choose the {what} file"));
                    }
                }
            }
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
        if self.beam == CUSTOM_BEAM && !Path::new(self.custom_beam.trim()).is_file() {
            return Err("choose the beam-description JSON file".into());
        }
        if self.caps.crop && CROPS[self.crop].is_some() && !self.crop_margin_mm.trim().is_empty() {
            Self::parse_number(&self.crop_margin_mm, "crop margin").and_then(|m| {
                if m < 0.0 {
                    Err("crop margin must be non-negative".to_owned())
                } else {
                    Ok(())
                }
            })?;
        }
        self.numeric_form().map(|_| ())
    }

    /// Fill the target list from a label-names JSON (no CT is read).
    fn scan_label_names(&mut self) {
        self.error = None;
        let result = std::fs::read(self.label_names_file.trim())
            .map_err(|e| format!("{}: {e}", self.label_names_file.trim()))
            .and_then(|bytes| parse_label_name_list(&bytes));
        match result {
            Ok(names) => {
                self.rois = names;
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
                self.study = None;
                self.scan_status = Some(format!("{} label(s) named", self.rois.len()));
            }
            Err(error) => {
                self.scan_status = None;
                self.error = Some(format!("label names: {error}"));
            }
        }
    }

    fn start_init(&mut self) {
        self.error = None;
        if let Err(error) = self.validate_new() {
            self.error = Some(error);
            return;
        }
        let mut args: Vec<String> = vec!["project".into(), "init".into()];
        match self.source {
            CtSource::Dicom => args.extend(["--dicom".into(), self.dicom_dir.trim().into()]),
            CtSource::Volume => args.extend([
                "--ct-nifti".into(),
                self.ct_file.trim().into(),
                "--labels-nifti".into(),
                self.labels_file.trim().into(),
                "--label-names".into(),
                self.label_names_file.trim().into(),
            ]),
        }
        args.extend([
            "--output".into(),
            self.output_dir.trim().into(),
            "--target".into(),
            self.target.clone(),
            "--spacing-mm".into(),
            self.spacing_mm.trim().into(),
        ]);
        self.reset_run_view();
        self.toml_text = None;
        self.spawn(JobKind::Init, args, HELPER_TIMEOUT);
    }

    fn beam_spec(&self) -> String {
        if self.beam == CUSTOM_BEAM {
            self.custom_beam.trim().to_owned()
        } else {
            self.beam.clone()
        }
    }

    fn finish_init(&mut self) {
        let dir = PathBuf::from(self.output_dir.trim());
        let patched = self
            .numeric_form()
            .and_then(|(_, blood, default_ratio, ratios)| {
                let path = dir.join("project.toml");
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                let crop = if self.caps.crop {
                    CROPS[self.crop]
                } else {
                    None
                };
                let margin = if crop.is_some() && self.caps.crop {
                    self.crop_margin_mm.trim().parse::<f64>().ok()
                } else {
                    None
                };
                let beam = self.beam_spec();
                let text = patch_project_toml(
                    &text,
                    &TomlEdits {
                        beam: &beam,
                        approach: APPROACHES[self.approach],
                        blood_ug_g: blood,
                        ratios: &ratios,
                        default_ratio,
                        order: QUADRATURE[self.quadrature].1,
                        photon_transport: self.photon_transport,
                        source_weighting: WEIGHTINGS[self.source_weighting],
                        crop,
                        crop_margin_mm: margin,
                    },
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

    /// `project verify` needs a finished run (all seven steps recorded).
    fn can_verify(&self) -> bool {
        matches!(self.steps[6], StepState::Done(_) | StepState::Skipped) && !self.running()
    }

    fn start_verify(&mut self) {
        self.error = None;
        let dir = self.project_path();
        if !dir.join("project.toml").is_file() || !self.can_verify() {
            self.error = Some("run the project to completion before verifying".into());
            return;
        }
        let particles = self.verify_particles.trim().to_owned();
        match Self::parse_number(&particles, "particles") {
            Ok(n) if n >= 1.0 => {}
            Ok(_) => {
                self.error = Some("particles must be at least 1".into());
                return;
            }
            Err(error) => {
                self.error = Some(error);
                return;
            }
        }
        if self.caps.variance_reduction
            && let Some(mode) = VARIANCE[self.variance]
        {
            let path = dir.join("project.toml");
            let written = std::fs::read_to_string(&path)
                .map_err(|e| format!("{}: {e}", path.display()))
                .and_then(|text| {
                    let text =
                        set_toml_key(&text, "verify", "variance_reduction", &format!("{mode:?}"));
                    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
                });
            if let Err(error) = written {
                self.error = Some(error);
                return;
            }
            self.toml_text = std::fs::read_to_string(&path).ok();
        }
        self.verify = VerifyView {
            started: Some(std::time::Instant::now()),
            ..VerifyView::default()
        };
        self.log.clear();
        let mut args = vec![
            "project".into(),
            "verify".into(),
            dir.display().to_string(),
            "--particles".into(),
            particles,
        ];
        if self.force {
            args.push("--force".into());
        }
        self.spawn(JobKind::Verify, args, RUN_TIMEOUT);
        if self.job.is_some() {
            self.status = Some("verifying with OpenMC…".into());
        }
    }

    fn ingest_verify_lines(&mut self, lines: &[String]) {
        for line in lines {
            match parse_verify_line(line) {
                Some(VerifyEvent::Running { histories, batches }) => {
                    self.verify.histories = Some(histories);
                    self.verify.batches = Some(batches);
                }
                Some(VerifyEvent::Batch(n)) => self.verify.batch_done = n,
                Some(VerifyEvent::Skipped) => self.verify.skipped = true,
                Some(VerifyEvent::Done(seconds)) => self.verify.finished_s = Some(seconds),
                Some(VerifyEvent::Failed) | None => {}
            }
        }
        self.log.extend(lines.iter().cloned());
        if self.log.len() > LOG_CAP {
            let drop = self.log.len() - LOG_CAP;
            self.log.drain(..drop);
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
            JobKind::Verify => self.ingest_verify_lines(&fresh),
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
            JobKind::HelpInit => {
                self.caps.crop = code == Some(0) && help_mentions(&self.job_output, "crop");
            }
            JobKind::HelpVerify => {
                self.caps.variance_reduction =
                    code == Some(0) && help_mentions(&self.job_output, "variance");
            }
            JobKind::Verify => {
                self.verify.started = None;
                self.refresh_from_disk();
                if code != Some(0) || timed_out {
                    let detail = self
                        .log
                        .iter()
                        .rev()
                        .find(|l| l.trim_start().starts_with("error"))
                        .cloned()
                        .unwrap_or_default();
                    self.error = Some(format!(
                        "project verify did not finish ({summary}) {detail}"
                    ));
                } else {
                    self.status = Some(format!("verification complete ({summary})"));
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

/// Colour of a verification status cell.
fn status_colour(theme: Theme, text: &str) -> Option<egui::Color32> {
    match text.trim() {
        "agrees" => Some(egui::Color32::from_rgb(86, 190, 120)),
        "unresolved" => Some(theme.warn_text),
        "disagrees" => Some(theme.error),
        _ => None,
    }
}

/// Verdict line (`**Verdict: AGREES …**`) as a coloured banner.
fn show_verdict(ui: &mut egui::Ui, theme: Theme, line: &str) {
    let text = plain_inline(line);
    let colour = if text.contains("DISAGREES") {
        theme.error
    } else if text.contains("INCONCLUSIVE") {
        theme.warn_text
    } else {
        egui::Color32::from_rgb(86, 190, 120)
    };
    egui::Frame::new()
        .fill(colour.gamma_multiply(0.18))
        .stroke(egui::Stroke::new(1.0, colour))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(text).strong().color(colour));
        });
}

fn show_table(ui: &mut egui::Ui, theme: Theme, id: usize, rows: &[Vec<String>]) {
    let Some(header) = rows.first() else { return };
    let status_col = header.iter().position(|h| h.eq_ignore_ascii_case("status"));
    let header_blank = header.iter().all(|h| h.is_empty());
    egui::Grid::new(("project-md-table", id))
        .striped(true)
        .spacing([14.0, 3.0])
        .show(ui, |ui| {
            if !header_blank {
                for cell in header {
                    ui.label(egui::RichText::new(plain_inline(cell)).strong());
                }
                ui.end_row();
            }
            for row in &rows[1..] {
                for (col, cell) in row.iter().enumerate() {
                    let text = plain_inline(cell);
                    let colour = (Some(col) == status_col)
                        .then(|| status_colour(theme, &text))
                        .flatten();
                    match colour {
                        Some(colour) => ui.label(egui::RichText::new(text).strong().color(colour)),
                        None => ui.label(text),
                    };
                }
                ui.end_row();
            }
        });
}

/// Rendering of report.md: headings emphasised, pipe tables as real
/// grids (status cells coloured), the verdict line as a banner, code
/// kept monospace, everything else wrapped.
fn show_markdown(ui: &mut egui::Ui, theme: Theme, text: &str) {
    let mut in_code = false;
    let mut table: Vec<Vec<String>> = Vec::new();
    let mut tables = 0_usize;
    let mut flush = |ui: &mut egui::Ui, table: &mut Vec<Vec<String>>| {
        if !table.is_empty() {
            tables += 1;
            show_table(ui, theme, tables, table);
            table.clear();
            ui.add_space(4.0);
        }
    };
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            flush(ui, &mut table);
            in_code = !in_code;
            continue;
        }
        if !in_code && trimmed.starts_with('|') {
            if let Some(cells) = table_cells(trimmed) {
                table.push(cells);
            }
            continue;
        }
        flush(ui, &mut table);
        if in_code {
            ui.monospace(line);
        } else if trimmed.starts_with("**Verdict") {
            show_verdict(ui, theme, trimmed);
        } else if let Some(title) = trimmed.strip_prefix("### ") {
            ui.label(egui::RichText::new(plain_inline(title)).strong());
        } else if let Some(title) = trimmed.strip_prefix("## ") {
            ui.label(egui::RichText::new(plain_inline(title)).strong().size(16.0));
        } else if let Some(title) = trimmed.strip_prefix("# ") {
            ui.label(egui::RichText::new(plain_inline(title)).strong().size(19.0));
        } else if trimmed.is_empty() {
            ui.add_space(4.0);
        } else {
            ui.label(plain_inline(line));
        }
    }
    flush(ui, &mut table);
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

#[cfg(not(target_arch = "wasm32"))]
fn pick_file(current: &str, extensions: &[&str]) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().add_filter("file", extensions);
    if let Some(parent) = Path::new(current.trim()).parent()
        && parent.is_dir()
    {
        dialog = dialog.set_directory(parent);
    }
    dialog.pick_file().map(|p| p.display().to_string())
}

#[cfg(target_arch = "wasm32")]
fn pick_file(_current: &str, _extensions: &[&str]) -> Option<String> {
    None
}

/// One `label | path field | Browse…` grid row.
fn file_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    browse: &str,
    extensions: &[&str],
    enabled: bool,
) {
    ui.label(label);
    ui.add(egui::TextEdit::singleline(value).desired_width(380.0));
    if ui.add_enabled(enabled, egui::Button::new(browse)).clicked()
        && let Some(file) = pick_file(value, extensions)
    {
        *value = file;
    }
    ui.end_row();
}

/// Transport options: quadrature, photon transport, source weighting.
fn show_advanced(ui: &mut egui::Ui, panel: &mut ProjectPanel, language: Language, theme: Theme) {
    egui::Grid::new("project-advanced-grid")
        .num_columns(3)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label(t!(language, en = "Quadrature order", ja = "求積次数"));
            egui::ComboBox::from_id_salt("project-quadrature")
                .width(80.0)
                .selected_text(QUADRATURE[panel.quadrature].0)
                .show_ui(ui, |ui| {
                    for (index, (name, _)) in QUADRATURE.iter().enumerate() {
                        ui.selectable_value(&mut panel.quadrature, index, *name);
                    }
                });
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "S4 is faster; S8 resolves angular detail better ([transport] order)",
                    ja = "S4 は高速、S8 は角度分解能が高い ([transport] order)"
                ))
                .color(theme.text_dim),
            );
            ui.end_row();

            ui.label(t!(language, en = "Photon transport", ja = "光子輸送"));
            ui.checkbox(
                &mut panel.photon_transport,
                t!(
                    language,
                    en = "transport capture photons",
                    ja = "捕獲光子を輸送する"
                ),
            );
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "off: photon energy is deposited where it is born",
                    ja = "オフ: 光子のエネルギーを発生地点に付与"
                ))
                .color(theme.text_dim),
            );
            ui.end_row();

            ui.label(t!(language, en = "Source weighting", ja = "線源の重み付け"));
            egui::ComboBox::from_id_salt("project-weighting")
                .width(170.0)
                .selected_text(WEIGHTINGS[panel.source_weighting])
                .show_ui(ui, |ui| {
                    for (index, name) in WEIGHTINGS.iter().enumerate() {
                        ui.selectable_value(&mut panel.source_weighting, index, *name);
                    }
                });
            ui.label(
                egui::RichText::new(t!(
                    language,
                    en = "within-bin spectrum shape of a histogram beam",
                    ja = "ヒストグラムビームのビン内スペクトル形状"
                ))
                .color(theme.text_dim),
            );
            ui.end_row();
        });
    ui.label(
        egui::RichText::new(t!(
            language,
            en = "CMFD acceleration is not a project.toml option, so it has no control here.",
            ja =
                "CMFD 加速は project.toml のオプションではないため、ここには設定項目がありません。"
        ))
        .color(theme.text_dim),
    );
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
    if !busy_now && panel.probe_stage < 3 {
        let (kind, args): (JobKind, &[&str]) = match panel.probe_stage {
            0 => (JobKind::Builtins, &["project", "builtins"]),
            1 => (JobKind::HelpInit, &["project", "init", "--help"]),
            _ => (JobKind::HelpVerify, &["project", "verify", "--help"]),
        };
        panel.probe_stage += 1;
        panel.spawn(
            kind,
            args.iter().map(|a| (*a).to_owned()).collect(),
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
                ui.label(t!(language, en = "CT input", ja = "CT 入力"));
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut panel.source,
                        CtSource::Dicom,
                        t!(language, en = "DICOM folder", ja = "DICOM フォルダ"),
                    );
                    ui.selectable_value(
                        &mut panel.source,
                        CtSource::Volume,
                        t!(
                            language,
                            en = "CT volume file (NIfTI)",
                            ja = "CT ボリュームファイル (NIfTI)"
                        ),
                    );
                });
                ui.label(
                    egui::RichText::new(match panel.source {
                        CtSource::Dicom => t!(
                            language,
                            en = "CT series + RT Structure Set or DICOM SEG",
                            ja = "CT シリーズ + RT Structure Set または DICOM SEG"
                        ),
                        CtSource::Volume => t!(
                            language,
                            en = "HU CT + integer labelmap + label-names JSON",
                            ja = "HU 値の CT + 整数ラベルマップ + ラベル名 JSON"
                        ),
                    })
                    .color(theme.text_dim),
                );
                ui.end_row();

                match panel.source {
                    CtSource::Dicom => {
                        ui.label(t!(language, en = "DICOM folder", ja = "DICOM フォルダ"));
                        ui.add(
                            egui::TextEdit::singleline(&mut panel.dicom_dir).desired_width(380.0),
                        );
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
                                    egui::Button::new(t!(
                                        language,
                                        en = "Scan ROIs",
                                        ja = "ROI を検出"
                                    )),
                                )
                                .clicked()
                            {
                                panel.error = None;
                                let dir = PathBuf::from(panel.dicom_dir.trim());
                                panel.start_scan(dir, false);
                            }
                        });
                        ui.end_row();
                    }
                    CtSource::Volume => {
                        file_row(
                            ui,
                            t!(language, en = "CT volume", ja = "CT ボリューム"),
                            &mut panel.ct_file,
                            t!(language, en = "Browse…", ja = "参照…"),
                            &["nii", "gz"],
                            !busy,
                        );
                        file_row(
                            ui,
                            t!(language, en = "Labels volume", ja = "ラベルボリューム"),
                            &mut panel.labels_file,
                            t!(language, en = "Browse…", ja = "参照…"),
                            &["nii", "gz"],
                            !busy,
                        );
                        ui.label(t!(language, en = "Label names", ja = "ラベル名"));
                        ui.add(
                            egui::TextEdit::singleline(&mut panel.label_names_file)
                                .desired_width(380.0),
                        );
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !busy,
                                    egui::Button::new(t!(language, en = "Browse…", ja = "参照…")),
                                )
                                .clicked()
                                && let Some(file) = pick_file(&panel.label_names_file, &["json"])
                            {
                                panel.label_names_file = file;
                            }
                            if ui
                                .add_enabled(
                                    !busy && !panel.label_names_file.trim().is_empty(),
                                    egui::Button::new(t!(
                                        language,
                                        en = "Scan ROIs",
                                        ja = "ROI を検出"
                                    )),
                                )
                                .on_hover_text(t!(
                                    language,
                                    en = "Lists the names in the label-names JSON.",
                                    ja = "ラベル名 JSON に書かれた名前を一覧します。"
                                ))
                                .clicked()
                            {
                                panel.scan_label_names();
                            }
                        });
                        ui.end_row();
                    }
                }

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

                ui.label(t!(language, en = "Crop", ja = "クロップ"));
                let crop_enabled = panel.caps.crop;
                let crop_response = ui
                    .add_enabled_ui(crop_enabled, |ui| {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("project-crop")
                                .width(110.0)
                                .selected_text(CROPS[panel.crop].unwrap_or("default"))
                                .show_ui(ui, |ui| {
                                    for (index, crop) in CROPS.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut panel.crop,
                                            index,
                                            crop.unwrap_or("default"),
                                        );
                                    }
                                });
                            ui.label(t!(language, en = "margin (mm)", ja = "マージン (mm)"));
                            ui.add_enabled(
                                CROPS[panel.crop] == Some("body"),
                                egui::TextEdit::singleline(&mut panel.crop_margin_mm)
                                    .desired_width(60.0)
                                    .hint_text("default"),
                            );
                        });
                    })
                    .response;
                crop_response.on_hover_text(if crop_enabled {
                    t!(
                        language,
                        en = "Crop the imported grid to the body outline (plus margin) or not at all.",
                        ja = "取り込んだ格子を体表(+マージン)に切り詰めるか、切り詰めません。"
                    )
                } else {
                    t!(
                        language,
                        en = "Disabled: the installed openbnct does not list `crop` in `project init --help`.",
                        ja = "無効: インストール済みの openbnct の `project init --help` に `crop` がありません。"
                    )
                });
                ui.end_row();

                ui.label(t!(language, en = "Beam", ja = "ビーム"));
                egui::ComboBox::from_id_salt("project-beam")
                    .width(300.0)
                    .selected_text(panel.beam.clone())
                    .show_ui(ui, |ui| {
                        for beam in panel.beams.clone() {
                            ui.selectable_value(&mut panel.beam, beam.clone(), beam);
                        }
                        ui.selectable_value(
                            &mut panel.beam,
                            CUSTOM_BEAM.to_owned(),
                            t!(
                                language,
                                en = "Beam-description file…",
                                ja = "ビーム記述ファイル…"
                            ),
                        );
                        // A project cannot reference a phase-space source:
                        // `[beam] description` takes a builtin or a binned
                        // beam-description JSON, so the IAEA header is
                        // reduced with `openbnct beam phsp-bin` first.
                        ui.add_enabled(
                            false,
                            egui::Button::new(t!(
                                language,
                                en = "Phase-space file (IAEA)…",
                                ja = "位相空間ファイル (IAEA)…"
                            )),
                        )
                        .on_disabled_hover_text(t!(
                            language,
                            en = "CLI-only for now: bin the IAEA header with `openbnct beam phsp-bin`, then choose the resulting beam-description file here.",
                            ja = "現時点では CLI のみ: `openbnct beam phsp-bin` で IAEA ヘッダをビン化し、得られたビーム記述ファイルをここで選択してください。"
                        ));
                    });
                ui.end_row();

                if panel.beam == CUSTOM_BEAM {
                    file_row(
                        ui,
                        t!(language, en = "Beam file", ja = "ビームファイル"),
                        &mut panel.custom_beam,
                        t!(language, en = "Browse…", ja = "参照…"),
                        &["json"],
                        !busy,
                    );
                }

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

        ui.add_space(4.0);
        egui::CollapsingHeader::new(t!(language, en = "Advanced", ja = "詳細設定"))
            .id_salt("project-advanced")
            .default_open(panel.advanced_open)
            .show(ui, |ui| show_advanced(ui, panel, language, theme));

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
    let scroll_now = panel.scroll_results_frames > 0;
    if scroll_now {
        panel.scroll_results_frames -= 1;
        ui.ctx().request_repaint();
        // Without a verify section, anchor on the Results card itself.
        if !panel
            .report
            .as_deref()
            .is_some_and(|r| r.contains(VERIFY_HEADING))
        {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
    }
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
                    panel.source == CtSource::Dicom
                        && panel.scan_rx.is_none()
                        && Path::new(&dicom).is_dir(),
                    egui::Button::new(t!(
                        language,
                        en = "Load CT and ROIs into Geometry",
                        ja = "CT と ROI をジオメトリに読み込む"
                    )),
                )
                .on_disabled_hover_text(t!(
                    language,
                    en = "available for DICOM-folder projects",
                    ja = "DICOM フォルダのプロジェクトで利用できます"
                ))
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
        show_verify_controls(ui, panel, language, theme);
        ui.add_space(6.0);
        match panel.report.clone() {
            Some(report) => {
                let (before, verify, after) = split_verify_section(&report);
                if let Some(section) = verify {
                    ui.separator();
                    if scroll_now {
                        ui.scroll_to_cursor(Some(egui::Align::TOP));
                    }
                    ui.push_id("verify-section", |ui| show_markdown(ui, theme, &section));
                    ui.separator();
                }
                ui.push_id("report-before", |ui| show_markdown(ui, theme, &before));
                ui.push_id("report-after", |ui| show_markdown(ui, theme, &after));
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

/// "Verify with Monte Carlo": particles, variance reduction, progress.
fn show_verify_controls(
    ui: &mut egui::Ui,
    panel: &mut ProjectPanel,
    language: Language,
    theme: Theme,
) {
    ui.label(
        egui::RichText::new(t!(
            language,
            en = "Independent Monte Carlo check (OpenMC)",
            ja = "独立モンテカルロ検証 (OpenMC)"
        ))
        .strong(),
    );
    let verifying = panel
        .job
        .as_ref()
        .is_some_and(|(k, _)| *k == JobKind::Verify);
    ui.horizontal(|ui| {
        ui.label(t!(language, en = "Particles", ja = "粒子数"));
        ui.add_enabled(
            !panel.running(),
            egui::TextEdit::singleline(&mut panel.verify_particles).desired_width(70.0),
        )
        .on_hover_text(t!(
            language,
            en = "Source histories, e.g. 2e5 or 1000000 (more particles resolve smaller structures).",
            ja = "線源ヒストリー数。例: 2e5 または 1000000(多いほど小さな構造を解像できます)。"
        ));
        ui.label(t!(language, en = "Variance reduction", ja = "分散低減"));
        let enabled = panel.caps.variance_reduction && !panel.running();
        let response = ui
            .add_enabled_ui(enabled, |ui| {
                egui::ComboBox::from_id_salt("project-variance")
                    .width(120.0)
                    .selected_text(VARIANCE[panel.variance].unwrap_or("project.toml"))
                    .show_ui(ui, |ui| {
                        for (index, mode) in VARIANCE.iter().enumerate() {
                            ui.selectable_value(
                                &mut panel.variance,
                                index,
                                mode.unwrap_or("project.toml"),
                            );
                        }
                    });
            })
            .response;
        response.on_hover_text(if panel.caps.variance_reduction {
            t!(
                language,
                en = "Written to [verify] variance_reduction in project.toml before the run.",
                ja = "実行前に project.toml の [verify] variance_reduction に書き込みます。"
            )
        } else {
            t!(
                language,
                en = "Disabled: the installed openbnct does not list variance reduction in `project verify --help`.",
                ja = "無効: インストール済みの openbnct の `project verify --help` に分散低減がありません。"
            )
        });
        if ui
            .add_enabled(
                panel.can_verify(),
                egui::Button::new(t!(
                    language,
                    en = "Verify with Monte Carlo",
                    ja = "モンテカルロで検証"
                )),
            )
            .on_disabled_hover_text(t!(
                language,
                en = "needs a completed run (all seven steps) and OpenMC 0.16 with ENDF/B-VIII.1",
                ja = "実行の完了(7 ステップすべて)と、OpenMC 0.16 および ENDF/B-VIII.1 が必要です"
            ))
            .clicked()
        {
            panel.start_verify();
        }
        if ui
            .add_enabled(
                verifying,
                egui::Button::new(t!(language, en = "Cancel", ja = "キャンセル")),
            )
            .clicked()
        {
            panel.cancel();
        }
    });
    if verifying {
        let elapsed = panel
            .verify
            .started
            .map_or(0.0, |t0| t0.elapsed().as_secs_f32());
        let (text, fraction) = match (panel.verify.batches, panel.verify.histories) {
            (Some(batches), _) if panel.verify.batch_done > 0 => (
                format!(
                    "OpenMC batch {}/{batches} · {elapsed:.0} s",
                    panel.verify.batch_done
                ),
                (panel.verify.batch_done as f32 / batches.max(1) as f32).min(1.0),
            ),
            (Some(batches), Some(histories)) => (
                format!("OpenMC: {histories} histories, {batches} batches · {elapsed:.0} s"),
                0.0,
            ),
            _ => (format!("preparing · {elapsed:.0} s"), 0.0),
        };
        ui.add(
            egui::ProgressBar::new(fraction)
                .animate(panel.verify.batch_done == 0)
                .text(text),
        );
    } else if let Some(seconds) = panel.verify.finished_s {
        ui.label(
            egui::RichText::new(format!("verify done in {seconds:.1} s")).color(theme.text_dim),
        );
    } else if panel.verify.skipped {
        ui.label(
            egui::RichText::new(t!(
                language,
                en = "verification up to date (tick force to rerun)",
                ja = "検証は最新です(再実行するには強制にチェック)"
            ))
            .color(theme.text_dim),
        );
    }
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

    fn edits<'a>(ratios: &'a [(String, f64)]) -> TomlEdits<'a> {
        TomlEdits {
            beam: "builtin:beams/other",
            approach: "-z",
            blood_ug_g: 30.0,
            ratios,
            default_ratio: 0.5,
            order: 4,
            photon_transport: false,
            source_weighting: "collapse_consistent",
            crop: None,
            crop_margin_mm: None,
        }
    }

    const TEMPLATE: &str = "# ROIs: GTV\n[project]\nid = \"x\"\n[imaging]\ndicom = \"d\"\nspacing_mm = 5.0\n[beam]\ndescription = \"builtin:beams/fir1-k63\"    # c\ntarget = \"GTV\"\napproach = \"+x\"    # c\n[transport]\nengine = \"sn\"\norder = 8\nsource_weighting = \"uniform_in_bin\"    # c\nphoton_transport = true    # c\n[boron]\nblood_ug_g = 25.0\nratios = { \"GTV\" = 3.5 }\ndefault_ratio = 1.0    # c\n";

    #[test]
    fn toml_patch_replaces_each_key_once() {
        let ratios = [("GTV".to_owned(), 3.5), ("SKIN".to_owned(), 1.0)];
        let out = patch_project_toml(TEMPLATE, &edits(&ratios)).unwrap();
        assert!(out.contains("description = \"builtin:beams/other\"\n"));
        assert!(out.contains("approach = \"-z\"\n"));
        assert!(out.contains("blood_ug_g = 30.0\n"));
        assert!(out.contains("ratios = { \"GTV\" = 3.5, \"SKIN\" = 1.0 }\n"));
        assert!(out.contains("default_ratio = 0.5\n"));
        assert!(out.contains("order = 4\n"));
        assert!(out.contains("photon_transport = false\n"));
        assert!(out.contains("source_weighting = \"collapse_consistent\"\n"));
        assert!(out.contains("target = \"GTV\""));
        assert!(!out.contains("crop"));
        assert!(patch_project_toml("[beam]\n", &edits(&[])).is_err());
    }

    #[test]
    fn toml_patch_inserts_or_replaces_crop_keys() {
        let mut e = edits(&[]);
        e.crop = Some("body");
        e.crop_margin_mm = Some(10.0);
        let out = patch_project_toml(TEMPLATE, &e).unwrap();
        assert!(out.contains("spacing_mm = 5.0\ncrop = \"body\"\ncrop_margin_mm = 10.0\n"));
        // Re-patching replaces rather than duplicates.
        e.crop = Some("none");
        e.crop_margin_mm = None;
        let again = patch_project_toml(&out, &e).unwrap();
        assert_eq!(again.matches("crop = ").count(), 1);
        assert!(again.contains("crop = \"none\"\n"));
    }

    #[test]
    fn set_toml_key_edits_or_creates_section() {
        let text = "[a]\nx = 1\n[verify]\nparticles = 5\nvariance_reduction = \"none\"\n[z]\n";
        let out = set_toml_key(text, "verify", "variance_reduction", "\"cadis\"");
        assert!(out.contains("variance_reduction = \"cadis\"\n"));
        assert_eq!(out.matches("variance_reduction").count(), 1);
        let inserted = set_toml_key(
            "[verify]\nparticles = 5\n",
            "verify",
            "variance_reduction",
            "\"fw-cadis\"",
        );
        assert!(inserted.starts_with("[verify]\nvariance_reduction = \"fw-cadis\"\n"));
        let created = set_toml_key("[a]\nx = 1\n", "verify", "variance_reduction", "\"cadis\"");
        assert!(created.ends_with("[verify]\nvariance_reduction = \"cadis\"\n"));
    }

    #[test]
    fn label_names_and_verify_lines_parse() {
        let flat = br#"{"2": "Brainstem", "1": "Brain"}"#;
        assert_eq!(parse_label_name_list(flat).unwrap(), ["Brain", "Brainstem"]);
        let wrapped = br#"{"encoding": "bitmask", "labels": {"4": "B", "1": "A"}}"#;
        assert_eq!(parse_label_name_list(wrapped).unwrap(), ["A", "B"]);
        assert!(parse_label_name_list(b"[]").is_err());
        assert_eq!(
            parse_verify_line(
                "[verify] running OpenMC: 200000 histories in 10 batches on 2 threads"
            ),
            Some(VerifyEvent::Running {
                histories: 200_000,
                batches: 10
            })
        );
        assert_eq!(
            parse_verify_line("[verify] done in 61.2 s"),
            Some(VerifyEvent::Done(61.2))
        );
        assert_eq!(
            parse_verify_line("  Simulating batch 3"),
            Some(VerifyEvent::Batch(3))
        );
        assert_eq!(
            parse_verify_line("[verify] FAILED after 1.0 s"),
            Some(VerifyEvent::Failed)
        );
        assert_eq!(parse_verify_line("report: x"), None);
    }

    #[test]
    fn report_verify_section_splits_and_tables_parse() {
        let report = "# R\n\n## Metrics\ntext\n\n## Independent Monte Carlo check\n\n**Verdict: AGREES**\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n## Notes\nend\n";
        let (before, section, after) = split_verify_section(report);
        let section = section.unwrap();
        assert!(before.contains("## Metrics") && !before.contains("Independent"));
        assert!(section.starts_with("## Independent Monte Carlo check"));
        assert!(section.contains("| 1 | 2 |") && !section.contains("## Notes"));
        assert!(after.starts_with("## Notes"));
        assert_eq!(table_cells("| a | b |"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(table_cells("|---|:--:|"), None);
        assert!(split_verify_section("# no section").1.is_none());
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
