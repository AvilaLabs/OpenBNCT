// SPDX-License-Identifier: MIT

//! `openbnct project verify`: an independent continuous-energy Monte Carlo
//! check of a finished project.
//!
//! The deterministic (S_N) answer comes first; this re-computes the same
//! case, material assignment and source with OpenMC on ENDF/B-VIII.1 (the
//! same boron concentration spec applied post hoc), then measures the
//! agreement per structure and per component and records a verdict with
//! explicit thresholds. The step is `08-verify` in the run manifest, its
//! artifacts live under `out/08-verify/`, and a fresh result is folded into
//! `out/report.md` / `out/report.json`.
//!
//! Research software: agreement here is a measured comparison of two
//! transport methods on a synthetic-or-research case, not a validation of
//! any dose for clinical use.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use openbnct_core::{DoseComponent, PhysicalDoseBundle, RegionMask};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    CwdGuard, DynResult, FileRef, ProjectConfig, RoiEntry, RunManifest, StepRecord, command_line,
    execute_command, fail, find_roi, hash_outputs, hash_path, load_config, load_manifest,
    mask_path, plan_step, read_roi_index, remove_outputs, report_structures, resolve_spec,
    save_manifest, utc_now, write_report,
};

/// Run-manifest id of the verification step.
pub const VERIFY_STEP_ID: &str = "08-verify";

/// Schema token of `out/08-verify/verification.json`.
pub const VERIFY_SCHEMA: &str = "openbnct.project-verify/0.1.0";

const VERIFY_DIR: &str = "out/08-verify";
const SUMMARY_PATH: &str = "out/08-verify/verification.json";

const DEFAULT_PARTICLES: u64 = 1_000_000;
const DEFAULT_BATCHES: u32 = 10;
const DEFAULT_THREADS: u32 = 2;
const DEFAULT_TIMEOUT_SECONDS: u64 = 4 * 3600;
const DEFAULT_RATIO_TOLERANCE: f64 = 0.05;
const DEFAULT_GAMMA_PASS_MIN: f64 = 0.95;
const DEFAULT_GAMMA_DOSE_PERCENT: f64 = 3.0;
const DEFAULT_GAMMA_DTA_MM: f64 = 3.0;
const DEFAULT_GAMMA_CUTOFF_PERCENT: f64 = 10.0;

/// OpenMC thermal table for hydrogen bound in liquid water — the table the
/// deterministic tissue library's `H1` S(alpha,beta) kernel was built from
/// (ENDF/B-VIII.1 `tsl_H(H2O)_0001`).
const H_IN_H2O_TABLE: &str = "c_H_in_H2O";

/// Below this fraction of the MC maximum a voxel's S_N/MC ratio is not
/// reported in the ratio volumes (the ratio of two near-zero, noisy numbers
/// carries no information).
/// Distance the source plane is moved off a grid face, cm.
const SOURCE_INSET_CM: f64 = 0.001;

const RATIO_VOLUME_FLOOR: f64 = 0.01;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The `[verify]` table of `project.toml`; every field is optional and the
/// command-line flags override it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifySection {
    #[serde(default, deserialize_with = "de_count")]
    pub particles: Option<u64>,
    pub batches: Option<u32>,
    pub threads: Option<u32>,
    pub timeout_seconds: Option<u64>,
    pub openmc: Option<String>,
    pub cross_sections: Option<String>,
    /// Structure-mean total-dose ratio tolerance (fraction, default 0.05).
    pub ratio_tolerance: Option<f64>,
    /// Minimum gamma pass rate per structure (fraction, default 0.95).
    pub gamma_pass_min: Option<f64>,
    pub gamma_dose_percent: Option<f64>,
    pub gamma_dta_mm: Option<f64>,
    pub gamma_cutoff_percent: Option<f64>,
    /// Declared S(alpha,beta) tables as `NUCLIDE=TABLE`. By default derived
    /// from the deterministic data's own declaration.
    pub thermal_scattering: Option<Vec<String>>,
    /// Structures the verdict covers (default: the report structures).
    #[serde(default)]
    pub structures: Vec<String>,
}

fn de_count<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    use serde::de::Error;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Number {
        Int(i64),
        Float(f64),
    }
    match Option::<Number>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Number::Int(v)) if v > 0 => Ok(Some(v as u64)),
        Some(Number::Float(v)) if v.is_finite() && v >= 1.0 && v.fract() == 0.0 && v < 9.0e15 => {
            Ok(Some(v as u64))
        }
        _ => Err(D::Error::custom(
            "particles must be a positive whole number (1e6 and 1000000 both work)",
        )),
    }
}

/// Parse a particle count such as `1000000` or `1e6`.
pub fn parse_count(text: &str) -> Result<u64, String> {
    let value: f64 = text
        .trim()
        .replace('_', "")
        .parse()
        .map_err(|_| format!("{text:?} is not a number"))?;
    if value.is_finite() && value >= 1.0 && value.fract() == 0.0 && value < 9.0e15 {
        Ok(value as u64)
    } else {
        Err(format!("{text:?} is not a positive whole number"))
    }
}

impl VerifySection {
    pub(super) fn validate(&self) -> DynResult<()> {
        let bad = |what: &str| fail(format!("project.toml: [verify] {what}"));
        if self.particles == Some(0) {
            return bad("particles must be at least 1");
        }
        if self.batches.is_some_and(|b| b < 2) {
            return bad("batches must be at least 2");
        }
        if self.threads == Some(0) {
            return bad("threads must be at least 1");
        }
        if self.timeout_seconds == Some(0) {
            return bad("timeout_seconds must be at least 1");
        }
        if self
            .ratio_tolerance
            .is_some_and(|t| !(t.is_finite() && t > 0.0 && t < 1.0))
        {
            return bad("ratio_tolerance must be a fraction in (0, 1), e.g. 0.05 for +-5%");
        }
        if self
            .gamma_pass_min
            .is_some_and(|t| !(t.is_finite() && t > 0.0 && t <= 1.0))
        {
            return bad("gamma_pass_min must be a fraction in (0, 1], e.g. 0.95");
        }
        for (name, value) in [
            ("gamma_dose_percent", self.gamma_dose_percent),
            ("gamma_dta_mm", self.gamma_dta_mm),
        ] {
            if value.is_some_and(|v| !(v.is_finite() && v > 0.0)) {
                return bad(&format!("{name} must be a positive number"));
            }
        }
        if self
            .gamma_cutoff_percent
            .is_some_and(|v| !(v.is_finite() && (0.0..100.0).contains(&v)))
        {
            return bad("gamma_cutoff_percent must be in [0, 100)");
        }
        if let Some(list) = &self.thermal_scattering {
            for spec in list {
                openbnct_openmc::ThermalScatteringDeclaration::parse(spec)
                    .map_err(|e| io::Error::other(format!("project.toml: [verify] {e}")))?;
            }
        }
        Ok(())
    }
}

/// Command-line overrides of `[verify]`.
#[derive(Debug, Clone, Default)]
pub struct VerifyOverrides {
    pub particles: Option<u64>,
    pub batches: Option<u32>,
    pub threads: Option<u32>,
    pub openmc: Option<PathBuf>,
    pub cross_sections: Option<PathBuf>,
    pub timeout_seconds: Option<u64>,
    pub force: bool,
}

/// Fully resolved settings of one verification.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub particles: u64,
    pub batches: u32,
    /// `particles` rounded up to a whole number of batches.
    pub histories: u64,
    pub threads: u32,
    pub timeout_seconds: u64,
    pub openmc: PathBuf,
    pub cross_sections: PathBuf,
    pub data_root: PathBuf,
    pub thresholds: Thresholds,
    /// Declared MC S(alpha,beta) tables, `NUCLIDE=TABLE`.
    pub thermal: Vec<String>,
    /// Nuclides carrying bound-atom scattering in the deterministic data.
    pub sn_thermal: Vec<String>,
    pub structures: Vec<String>,
}

/// The agreement thresholds (all reported with the verdict).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Thresholds {
    /// Maximum |S_N/MC - 1| of a structure-mean total dose.
    pub ratio_tolerance: f64,
    /// Minimum fraction of gamma-evaluated voxels passing, per structure.
    pub gamma_pass_min: f64,
    pub gamma_dose_percent: f64,
    pub gamma_dta_mm: f64,
    pub gamma_cutoff_percent: f64,
}

/// Bound-atom (S(alpha,beta)) nuclides named by a multigroup data file's
/// `collapse_declaration` ("... transfer applied for [H1] ...").
pub fn deterministic_thermal_nuclides(data: &Value) -> Vec<String> {
    let Some(text) = data["collapse_declaration"].as_str() else {
        return Vec::new();
    };
    let marker = "transfer applied for [";
    let Some(start) = text.find(marker).map(|i| i + marker.len()) else {
        return Vec::new();
    };
    let Some(end) = text[start..].find(']') else {
        return Vec::new();
    };
    let mut names: Vec<String> = text[start..start + end]
        .split(',')
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names
}

/// Resolve every setting: flag, then `[verify]`, then environment
/// (`OPENBNCT_OPENMC`, `OPENMC_CROSS_SECTIONS`), then a default.
pub fn resolve_settings(
    section: &VerifySection,
    overrides: &VerifyOverrides,
    sn_thermal: Vec<String>,
    structures: Vec<String>,
    env: &dyn Fn(&str) -> Option<String>,
) -> DynResult<Resolved> {
    let particles = overrides
        .particles
        .or(section.particles)
        .unwrap_or(DEFAULT_PARTICLES);
    let batches = overrides
        .batches
        .or(section.batches)
        .unwrap_or(DEFAULT_BATCHES);
    if batches < 2 {
        return fail("--batches must be at least 2");
    }
    let per_batch = particles.div_ceil(u64::from(batches));
    let histories = per_batch * u64::from(batches);
    let threads = overrides
        .threads
        .or(section.threads)
        .unwrap_or(DEFAULT_THREADS);
    if threads == 0 {
        return fail("--threads must be at least 1");
    }
    let timeout_seconds = overrides
        .timeout_seconds
        .or(section.timeout_seconds)
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);

    let openmc = overrides
        .openmc
        .clone()
        .or_else(|| section.openmc.as_ref().map(PathBuf::from))
        .or_else(|| env("OPENBNCT_OPENMC").map(PathBuf::from))
        .or_else(|| find_on_path("openmc", env))
        .ok_or_else(|| {
            io::Error::other(
                "no OpenMC executable: pass --openmc PATH, set [verify] openmc in project.toml, \
                 or set OPENBNCT_OPENMC (OpenMC 0.16.0 is required)",
            )
        })?;
    if !openmc.is_file() {
        return fail(format!(
            "OpenMC executable {} is not a file",
            openmc.display()
        ));
    }
    let cross_sections = overrides
        .cross_sections
        .clone()
        .or_else(|| section.cross_sections.as_ref().map(PathBuf::from))
        .or_else(|| env("OPENMC_CROSS_SECTIONS").map(PathBuf::from))
        .ok_or_else(|| {
            io::Error::other(
                "no nuclear data: pass --cross-sections PATH-TO/cross_sections.xml, set \
                 [verify] cross_sections in project.toml, or set OPENMC_CROSS_SECTIONS \
                 (ENDF/B-VIII.1 HDF5 library including its thermal/ directory)",
            )
        })?;
    if !cross_sections.is_file() {
        return fail(format!(
            "cross-sections file {} is not a file",
            cross_sections.display()
        ));
    }
    if cross_sections.file_name().and_then(|n| n.to_str()) != Some("cross_sections.xml") {
        return fail(format!(
            "{} must be named cross_sections.xml (the nuclear-data manifest binds that name)",
            cross_sections.display()
        ));
    }
    let cross_sections = cross_sections.canonicalize()?;
    let data_root = cross_sections
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("cross_sections.xml has no parent directory"))?;

    // Physics consistency: the MC side declares the same bound-atom
    // scattering the deterministic data carries, unless overridden.
    let thermal = match &section.thermal_scattering {
        Some(list) => list.clone(),
        None => {
            let mut declared = Vec::new();
            for nuclide in &sn_thermal {
                if nuclide == "H1" {
                    declared.push(format!("H1={H_IN_H2O_TABLE}"));
                } else {
                    return fail(format!(
                        "the deterministic data carries S(alpha,beta) for {nuclide}, which has no \
                         default OpenMC table; declare it with [verify] thermal_scattering = \
                         [\"{nuclide}=TABLE\"]"
                    ));
                }
            }
            declared
        }
    };
    let thresholds = Thresholds {
        ratio_tolerance: section.ratio_tolerance.unwrap_or(DEFAULT_RATIO_TOLERANCE),
        gamma_pass_min: section.gamma_pass_min.unwrap_or(DEFAULT_GAMMA_PASS_MIN),
        gamma_dose_percent: section
            .gamma_dose_percent
            .unwrap_or(DEFAULT_GAMMA_DOSE_PERCENT),
        gamma_dta_mm: section.gamma_dta_mm.unwrap_or(DEFAULT_GAMMA_DTA_MM),
        gamma_cutoff_percent: section
            .gamma_cutoff_percent
            .unwrap_or(DEFAULT_GAMMA_CUTOFF_PERCENT),
    };
    Ok(Resolved {
        particles,
        batches,
        histories,
        threads,
        timeout_seconds,
        openmc,
        cross_sections,
        data_root,
        thresholds,
        thermal,
        sn_thermal,
        structures,
    })
}

fn find_on_path(name: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let path = env("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

// ---------------------------------------------------------------------------
// Verdict logic
// ---------------------------------------------------------------------------

/// Per-structure inputs of the agreement test (total dose).
#[derive(Debug, Clone, PartialEq)]
pub struct StructureCheck {
    pub name: String,
    pub sn_mean: f64,
    pub mc_mean: f64,
    /// Conservative (fully correlated) 1-sigma relative uncertainty of the
    /// MC structure mean.
    pub mc_rel_sigma: Option<f64>,
    /// Gamma pass rate of the total dose in the structure, when at least one
    /// voxel of it was gamma-evaluated.
    pub gamma_pass_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StructureStatus {
    Agrees,
    /// Fails a threshold and the MC statistics resolve the tolerance.
    Fails(Vec<String>),
    /// Fails a threshold, but the MC mean is too uncertain to test it.
    Unresolved(Vec<String>),
    NoDose,
}

impl StructureStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Agrees => "agrees",
            Self::Fails(_) => "DISAGREES",
            Self::Unresolved(_) => "unresolved",
            Self::NoDose => "no dose",
        }
    }
}

/// Test one structure against the thresholds.
pub fn evaluate_structure(check: &StructureCheck, th: &Thresholds) -> StructureStatus {
    if check.mc_mean <= 0.0 && check.sn_mean <= 0.0 {
        return StructureStatus::NoDose;
    }
    let mut reasons = Vec::new();
    if check.mc_mean <= 0.0 {
        reasons.push("MC total dose is zero where S_N is not".to_owned());
    } else {
        let ratio = check.sn_mean / check.mc_mean;
        if (ratio - 1.0).abs() > th.ratio_tolerance {
            reasons.push(format!(
                "total-dose ratio {ratio:.3} is outside 1 +- {:.1}%",
                th.ratio_tolerance * 100.0
            ));
        }
    }
    if let Some(rate) = check.gamma_pass_rate
        && rate < th.gamma_pass_min
    {
        reasons.push(format!(
            "gamma pass rate {:.1}% is below {:.1}%",
            rate * 100.0,
            th.gamma_pass_min * 100.0
        ));
    }
    if reasons.is_empty() {
        return StructureStatus::Agrees;
    }
    let resolved = check
        .mc_rel_sigma
        .is_some_and(|s| 2.0 * s <= th.ratio_tolerance);
    if resolved || check.mc_mean <= 0.0 {
        StructureStatus::Fails(reasons)
    } else {
        StructureStatus::Unresolved(reasons)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    /// `AGREES`, `DISAGREES` or `INCONCLUSIVE`.
    pub label: String,
    /// Full verdict sentence with the thresholds.
    pub text: String,
}

/// Combine per-structure statuses into the overall verdict.
pub fn overall_verdict(results: &[(String, StructureStatus)], th: &Thresholds) -> Verdict {
    let criteria = format!(
        "all evaluated structure-mean total-dose ratios (S_N/MC) within +-{:.1}% and gamma \
         ({}%/{} mm, {}% low-dose cutoff) pass rate >= {:.1}% in every structure",
        th.ratio_tolerance * 100.0,
        th.gamma_dose_percent,
        th.gamma_dta_mm,
        th.gamma_cutoff_percent,
        th.gamma_pass_min * 100.0
    );
    let evaluated = results
        .iter()
        .filter(|(_, s)| !matches!(s, StructureStatus::NoDose))
        .count();
    if evaluated == 0 {
        return Verdict {
            label: "INCONCLUSIVE".into(),
            text: "INCONCLUSIVE: no reported structure carries dose, so nothing could be compared"
                .into(),
        };
    }
    let failures: Vec<String> = results
        .iter()
        .filter_map(|(name, status)| match status {
            StructureStatus::Fails(reasons) => Some(format!("{name}: {}", reasons.join("; "))),
            _ => None,
        })
        .collect();
    if !failures.is_empty() {
        return Verdict {
            label: "DISAGREES".into(),
            text: format!(
                "DISAGREES: expected {criteria}; observed {}",
                failures.join(" | ")
            ),
        };
    }
    let unresolved: Vec<&str> = results
        .iter()
        .filter(|(_, s)| matches!(s, StructureStatus::Unresolved(_)))
        .map(|(n, _)| n.as_str())
        .collect();
    if !unresolved.is_empty() {
        return Verdict {
            label: "INCONCLUSIVE".into(),
            text: format!(
                "INCONCLUSIVE: no structure with resolved MC statistics disagrees, but {} could \
                 not be tested at +-{:.1}% (2 sigma of the MC structure mean exceeds the \
                 tolerance); rerun with more particles",
                unresolved.join(", "),
                th.ratio_tolerance * 100.0
            ),
        };
    }
    Verdict {
        label: "AGREES".into(),
        text: format!("AGREES: {criteria} ({evaluated} structures evaluated)"),
    }
}

// ---------------------------------------------------------------------------
// Numerics
// ---------------------------------------------------------------------------

/// Masked means of one quantity on both sides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuantityStat {
    pub quantity: String,
    pub sn_mean: f64,
    pub mc_mean: f64,
    /// S_N / MC of the masked means; absent when the MC mean is not positive.
    pub ratio: Option<f64>,
    /// Conservative 1-sigma relative uncertainty of the MC mean: the sum of
    /// voxel sigmas over the sum of voxel values (fully correlated), which
    /// bounds the true value from above.
    pub mc_rel_sigma: Option<f64>,
}

/// Compute the masked means and the MC relative uncertainty.
pub fn quantity_stat(
    quantity: &str,
    sn: &[f64],
    mc: &[f64],
    mc_sigma: Option<&[f64]>,
    mask: &[bool],
) -> QuantityStat {
    let mut n = 0.0_f64;
    let (mut sum_sn, mut sum_mc, mut sum_sigma) = (0.0, 0.0, 0.0);
    for (i, inside) in mask.iter().enumerate() {
        if *inside {
            n += 1.0;
            sum_sn += sn[i];
            sum_mc += mc[i];
            if let Some(sigma) = mc_sigma {
                sum_sigma += sigma[i];
            }
        }
    }
    let n = n.max(1.0);
    let mc_mean = sum_mc / n;
    QuantityStat {
        quantity: quantity.to_owned(),
        sn_mean: sum_sn / n,
        mc_mean,
        ratio: (mc_mean > 0.0).then(|| (sum_sn / n) / mc_mean),
        mc_rel_sigma: mc_sigma.and_then(|_| (sum_mc > 0.0).then(|| sum_sigma / sum_mc)),
    }
}

/// Pass rate of a gamma volume inside a mask: `(evaluated, passed)`.
pub fn gamma_in_mask(volume: &[Option<f64>], mask: &[bool]) -> (u64, u64) {
    let mut evaluated = 0;
    let mut passed = 0;
    for (gamma, inside) in volume.iter().zip(mask) {
        if *inside && let Some(g) = gamma {
            evaluated += 1;
            if *g <= 1.0 {
                passed += 1;
            }
        }
    }
    (evaluated, passed)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = (p * (sorted.len() - 1) as f64).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

// ---------------------------------------------------------------------------
// Result record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructureResult {
    pub name: String,
    pub voxels: u64,
    /// Boron, nitrogen, hydrogen, photon and physical total, in that order.
    pub quantities: Vec<QuantityStat>,
    pub gamma_evaluated: u64,
    pub gamma_pass_rate: Option<f64>,
    pub status: String,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WholeVolumeResult {
    pub quantity: String,
    pub gamma_pass_rate: f64,
    pub gamma_evaluated: u64,
    pub within_sigma_fraction: Option<f64>,
    pub max_normalized_difference: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McStatistics {
    /// Voxels at or above the cutoff fraction of the MC total maximum.
    pub voxels_evaluated: u64,
    pub cutoff_percent: f64,
    pub median_rel_sigma_total: f64,
    pub p95_rel_sigma_total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineInfo {
    pub openmc_commit: String,
    pub executable: String,
    pub cross_sections: String,
    pub particles_requested: u64,
    pub histories: u64,
    pub batches: u32,
    pub threads: u32,
    pub wall_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThermalInfo {
    /// Nuclides with bound-atom scattering in the deterministic data.
    pub sn: Vec<String>,
    /// `NUCLIDE=TABLE` declared for the MC deck.
    pub mc: Vec<String>,
    pub consistent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifySummary {
    pub schema_version: String,
    pub project_id: String,
    pub generated_utc: String,
    pub engine: EngineInfo,
    pub thermal_scattering: ThermalInfo,
    pub thresholds: Thresholds,
    pub statistics: McStatistics,
    pub structures: Vec<StructureResult>,
    pub whole_volume: Vec<WholeVolumeResult>,
    pub verdict: Verdict,
}

/// State of the verification for report purposes.
#[derive(Debug, Clone)]
pub enum VerifyState {
    NotRun,
    /// A verification exists but the study it verified has changed.
    Stale,
    Fresh(Box<VerifySummary>),
}

/// Quantities compared, with the ratio-volume file stem of each.
const QUANTITIES: [(&str, &str); 5] = [
    ("component:boron", "boron"),
    ("component:nitrogen", "nitrogen"),
    ("component:hydrogen", "hydrogen"),
    ("component:photon", "photon"),
    ("physical_total", "total"),
];

fn column_values<'a>(
    bundle: &'a PhysicalDoseBundle,
    quantity: &str,
) -> DynResult<(&'a [f64], Option<&'a [f64]>)> {
    if quantity == "physical_total" {
        return Ok((
            &bundle.physical_total.values,
            bundle
                .physical_total
                .absolute_standard_uncertainty
                .as_deref(),
        ));
    }
    let component = match quantity.strip_prefix("component:") {
        Some("boron") => DoseComponent::Boron,
        Some("nitrogen") => DoseComponent::Nitrogen,
        Some("hydrogen") => DoseComponent::Hydrogen,
        Some("photon") => DoseComponent::Photon,
        _ => return fail(format!("unknown quantity {quantity:?}")),
    };
    let volume = bundle
        .components
        .iter()
        .find(|v| v.component == component)
        .ok_or_else(|| io::Error::other(format!("dose bundle lacks {quantity}")))?;
    Ok((
        &volume.values,
        volume.absolute_standard_uncertainty.as_deref(),
    ))
}

// ---------------------------------------------------------------------------
// Report section
// ---------------------------------------------------------------------------

fn fmt_ratio(ratio: Option<f64>) -> String {
    ratio.map_or("n/a".into(), |r| format!("{r:.3}"))
}

fn fmt_percent(value: Option<f64>) -> String {
    value.map_or("n/a".into(), |v| format!("{:.1}%", v * 100.0))
}

/// One-line status for the report header block.
pub fn accuracy_suffix(state: &VerifyState) -> String {
    match state {
        VerifyState::NotRun => " No independent Monte Carlo check has been run on this study; \
            `openbnct project verify` runs one."
            .into(),
        VerifyState::Stale => " An independent Monte Carlo check was run earlier but the study \
            has changed since; rerun `openbnct project verify`."
            .into(),
        VerifyState::Fresh(summary) => format!(
            " An independent continuous-energy OpenMC check was run on this study \
             (verdict: {}; see \"Independent Monte Carlo check\").",
            summary.verdict.label
        ),
    }
}

/// The markdown "Independent Monte Carlo check" section.
pub fn render_section(state: &VerifyState) -> String {
    let mut md = String::from("## Independent Monte Carlo check\n\n");
    match state {
        VerifyState::NotRun => {
            md.push_str(
                "Not run. `openbnct project verify <project-dir>` re-computes this study with \
                 continuous-energy OpenMC (ENDF/B-VIII.1) on the same case, material assignment, \
                 source and boron concentrations and reports the agreement with the \
                 deterministic result above.\n\n",
            );
        }
        VerifyState::Stale => {
            md.push_str(
                "A verification exists in `out/08-verify` but the study it checked has changed \
                 since (an input or output no longer matches). It is not reported here; rerun \
                 `openbnct project verify <project-dir>`.\n\n",
            );
        }
        VerifyState::Fresh(s) => {
            let _ = writeln!(md, "**Verdict: {}**\n", s.verdict.text);
            let th = &s.thresholds;
            let _ = writeln!(
                md,
                "Definition: a structure agrees when its S_N/MC mean total-dose ratio is within \
                 +-{:.1}% of 1 and, where at least one of its voxels is gamma-evaluated, the \
                 total-dose gamma ({}% dose difference, {} mm distance to agreement, global, \
                 reference voxels below {}% of the MC maximum excluded) pass rate is at least \
                 {:.1}%. A failing structure whose MC mean is too uncertain to resolve the \
                 tolerance (2 sigma above it) is unresolved, not a disagreement. Overall: AGREES \
                 when every evaluated structure agrees; DISAGREES when any resolved structure \
                 fails; otherwise INCONCLUSIVE. Thresholds live in `[verify]` of `project.toml`.\n",
                th.ratio_tolerance * 100.0,
                th.gamma_dose_percent,
                th.gamma_dta_mm,
                th.gamma_cutoff_percent,
                th.gamma_pass_min * 100.0,
            );
            let _ = writeln!(md, "| | |\n|---|---|");
            let e = &s.engine;
            let _ = writeln!(
                md,
                "| Monte Carlo | OpenMC (commit {}), continuous energy, ENDF/B-VIII.1; {} histories \
                 ({} requested) in {} batches, {} threads, {:.0} s wall |",
                &e.openmc_commit[..e.openmc_commit.len().min(12)],
                e.histories,
                e.particles_requested,
                e.batches,
                e.threads,
                e.wall_seconds
            );
            let t = &s.thermal_scattering;
            let _ = writeln!(
                md,
                "| Thermal scattering | S_N: bound-atom kernel for {}; MC: {}; {} |",
                if t.sn.is_empty() {
                    "none (free gas)".to_owned()
                } else {
                    t.sn.join(", ")
                },
                if t.mc.is_empty() {
                    "none (free gas)".to_owned()
                } else {
                    t.mc.join(", ")
                },
                if t.consistent {
                    "consistent"
                } else {
                    "NOT consistent: the two sides use different scattering physics"
                }
            );
            let st = &s.statistics;
            let _ = writeln!(
                md,
                "| MC statistical uncertainty | total dose, voxels above {}% of the maximum \
                 ({} voxels): median relative 1-sigma {:.1}%, 95th percentile {:.1}% |",
                st.cutoff_percent,
                st.voxels_evaluated,
                st.median_rel_sigma_total * 100.0,
                st.p95_rel_sigma_total * 100.0
            );
            let _ = writeln!(
                md,
                "| Artifacts | `out/08-verify/` (MC dose, comparison, gamma, `ratio-<component>.nii`) |\n"
            );
            let _ = writeln!(
                md,
                "S_N/MC ratio of structure-mean dose per component (S_N is the boron-scaled \
                 deterministic dose; the MC column is the conservative, fully correlated 2-sigma \
                 relative uncertainty of the MC structure-mean total):\n"
            );
            let _ = writeln!(
                md,
                "| structure | voxels | boron | nitrogen | hydrogen | photon | total | MC 2 sigma (total) | gamma pass | status |\n|---|---|---|---|---|---|---|---|---|---|"
            );
            for r in &s.structures {
                let total = r.quantities.last();
                let cell = |q: &str| {
                    fmt_ratio(
                        r.quantities
                            .iter()
                            .find(|x| x.quantity == q)
                            .and_then(|x| x.ratio),
                    )
                };
                let gamma = match r.gamma_pass_rate {
                    Some(rate) => format!("{:.1}% ({} vox)", rate * 100.0, r.gamma_evaluated),
                    None => "n/a".to_owned(),
                };
                let _ = writeln!(
                    md,
                    "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                    r.name,
                    r.voxels,
                    cell("component:boron"),
                    cell("component:nitrogen"),
                    cell("component:hydrogen"),
                    cell("component:photon"),
                    cell("physical_total"),
                    fmt_percent(total.and_then(|q| q.mc_rel_sigma).map(|x| 2.0 * x)),
                    gamma,
                    r.status
                );
            }
            let _ = writeln!(md);
            for r in &s.structures {
                if !r.reasons.is_empty() {
                    let _ = writeln!(md, "- {}: {}", r.name, r.reasons.join("; "));
                }
            }
            if s.structures.iter().any(|r| !r.reasons.is_empty()) {
                let _ = writeln!(md);
            }
            let _ = writeln!(
                md,
                "Whole-volume agreement (S_N against MC as reference):\n\n| quantity | gamma pass | evaluated voxels | within 2 combined sigma | max difference / MC maximum |\n|---|---|---|---|---|"
            );
            for w in &s.whole_volume {
                let _ = writeln!(
                    md,
                    "| {} | {:.1}% | {} | {} | {:.3} |",
                    w.quantity,
                    w.gamma_pass_rate * 100.0,
                    w.gamma_evaluated,
                    fmt_percent(w.within_sigma_fraction),
                    w.max_normalized_difference
                );
            }
            let _ = writeln!(md);
        }
    }
    md
}

/// The `independent_mc_check` member of `report.json`.
pub fn report_json(state: &VerifyState) -> Value {
    match state {
        VerifyState::NotRun => json!({"status": "not_run"}),
        VerifyState::Stale => json!({"status": "stale"}),
        VerifyState::Fresh(summary) => {
            json!({"status": "fresh", "verification": summary})
        }
    }
}

// ---------------------------------------------------------------------------
// State loading (used by the report step)
// ---------------------------------------------------------------------------

/// Whether the recorded verification still matches the study on disk.
pub fn load_state(project: &Path, manifest: &RunManifest) -> DynResult<VerifyState> {
    let Some(record) = manifest.steps.iter().find(|s| s.id == VERIFY_STEP_ID) else {
        return Ok(VerifyState::NotRun);
    };
    if record.status != "complete" {
        return Ok(VerifyState::NotRun);
    }
    for input in &record.inputs {
        match hash_path(project, &input.path) {
            Ok(hashed) if hashed == *input => {}
            _ => return Ok(VerifyState::Stale),
        }
    }
    if hash_outputs(project, &[VERIFY_DIR.to_owned()])? != record.outputs {
        return Ok(VerifyState::Stale);
    }
    let summary: VerifySummary = serde_json::from_slice(&fs::read(project.join(SUMMARY_PATH))?)?;
    Ok(VerifyState::Fresh(Box::new(summary)))
}

// ---------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Action {
    Command(Vec<String>),
    WriteSource,
    WriteExecutionProfile,
    Analyze,
}

impl Action {
    fn text(&self) -> String {
        match self {
            Self::Command(argv) => command_line(argv),
            Self::WriteSource => format!(
                "# internal: write {VERIFY_DIR}/source.json (the \"source\" member of out/03-beam/case.json) and {VERIFY_DIR}/case.json (that case with this source); the port plane is moved {SOURCE_INSET_CM} cm inside the grid if it sits on the boundary"
            ),
            Self::WriteExecutionProfile => format!(
                "# internal: write {VERIFY_DIR}/execution-profile.json = inputs/openmc-execution-profile-smoke.json with the requested histories and batches"
            ),
            Self::Analyze => format!(
                "# internal: compare structure means, gamma per structure and ratio volumes -> {SUMMARY_PATH}, {VERIFY_DIR}/ratio-<component>.nii"
            ),
        }
    }
}

fn args<const N: usize>(parts: [&str; N]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

struct Plan {
    actions: Vec<Action>,
    inputs: Vec<String>,
    options: Value,
}

/// Build the ordered actions of the verification.
fn plan_verify(
    project: &Path,
    config: &ProjectConfig,
    resolved: &Resolved,
    rois: &[RoiEntry],
) -> DynResult<Plan> {
    let base_material = resolve_spec(project, &config.materials.base_material)?;
    let multigroup = resolve_spec(project, &config.materials.multigroup_data)?;
    let response_set = resolve_spec(project, "builtin:openmc/response-set-nf-bnct-001")?;
    let unit_profile = resolve_spec(
        project,
        "builtin:openmc/component-profile-unit-mass-fraction",
    )?;
    let unit_source_profile =
        resolve_spec(project, "builtin:openmc/unit-source-component-profile")?;
    let unit_source_material = resolve_spec(project, "builtin:openmc/unit-source-material")?;
    let base_manifest = resolve_spec(project, "builtin:openmc/endfb81-base-manifest")?;
    let smoke_profile = resolve_spec(project, "builtin:openmc/execution-profile-smoke")?;

    let data_root = resolved.data_root.to_string_lossy().into_owned();
    let cross_sections = resolved.cross_sections.to_string_lossy().into_owned();
    let openmc = resolved.openmc.to_string_lossy().into_owned();
    let dir = |name: &str| format!("{VERIFY_DIR}/{name}");

    // The MC boron step repeats step 05's concentration spec exactly.
    let mut listed: Vec<&RoiEntry> = config
        .boron
        .ratios
        .keys()
        .map(|name| find_roi(rois, name))
        .collect::<DynResult<_>>()?;
    listed.sort_by(|a, b| a.voxels.cmp(&b.voxels).then_with(|| a.name.cmp(&b.name)));
    let mut boron = args([
        "openbnct",
        "boron",
        "dose",
        "--physical-bundle",
        &dir("mc-physical-dose.json"),
        "--unit-dose",
        &dir("mc-boron-unit.json"),
        "--blood-ug-g",
        &config.boron.blood_ug_g.to_string(),
        "--default-ratio",
        &config.boron.default_ratio.to_string(),
    ]);
    let inputs = vec![
        base_material.clone(),
        multigroup.clone(),
        response_set.clone(),
        unit_profile.clone(),
        unit_source_profile.clone(),
        unit_source_material.clone(),
        base_manifest.clone(),
        smoke_profile.clone(),
        "out/02-calibrate/assignment.json".to_owned(),
        "out/03-beam/case.json".to_owned(),
        "out/05-boron/dose.json".to_owned(),
        "out/01-import/masks".to_owned(),
    ];
    for roi in &listed {
        boron.push("--ratio".into());
        boron.push(format!("{}={}", roi.name, config.boron.ratios[&roi.name]));
        boron.push("--mask".into());
        boron.push(format!("{}={}", roi.name, mask_path(roi)));
    }
    boron.extend(["--output".to_owned(), dir("mc-dose.json")]);

    let mut run = args([
        "openbnct",
        "openmc",
        "run",
        "--case",
        &dir("case.json"),
        "--component-profile",
        &unit_profile,
        "--material",
        &base_material,
        "--source",
        &dir("source.json"),
        "--response-set",
        &response_set,
        "--nuclear-data-manifest",
        &dir("nuclear-data-manifest.json"),
        "--execution-profile",
        &dir("execution-profile.json"),
        "--assignment",
        "out/02-calibrate/assignment.json",
        "--unit-source-component-profile",
        &unit_source_profile,
        "--unit-source-material",
        &unit_source_material,
        "--unit-source-nuclear-data-manifest",
        &base_manifest,
        "--mixture-levels",
        "20",
    ]);
    for spec in &resolved.thermal {
        run.push("--thermal-scattering".into());
        run.push(spec.clone());
    }
    run.extend(
        [
            "--nuclear-data-root",
            &data_root,
            "--openmc",
            &openmc,
            "--env",
            &format!("OPENMC_CROSS_SECTIONS={cross_sections}"),
            "--threads",
            &resolved.threads.to_string(),
            "--timeout-seconds",
            &resolved.timeout_seconds.to_string(),
            "--working-directory",
            &dir("openmc-run"),
            "--dose-output",
            &dir("mc-physical-dose.json"),
            "--boron-unit-dose-output",
            &dir("mc-boron-unit.json"),
        ]
        .map(str::to_owned),
    );

    let th = &resolved.thresholds;
    let actions = vec![
        Action::WriteSource,
        Action::WriteExecutionProfile,
        Action::Command(args([
            "openbnct",
            "openmc",
            "data",
            "select-manifest",
            "--base-manifest",
            &base_manifest,
            "--data-root",
            &data_root,
            "--assignment",
            "out/02-calibrate/assignment.json",
            "--manifest-id",
            &format!("{}.openmc-verify.endfb81", config.project.id),
            "--output",
            &dir("nuclear-data-manifest.json"),
        ])),
        Action::Command(run),
        Action::Command(boron),
        Action::Command(args([
            "openbnct",
            "compare",
            "--reference",
            &dir("mc-dose.json"),
            "--candidate",
            "out/05-boron/dose.json",
            "--output",
            &dir("comparison.json"),
        ])),
        Action::Command(args([
            "openbnct",
            "gamma",
            "--reference",
            &dir("mc-dose.json"),
            "--candidate",
            "out/05-boron/dose.json",
            "--dose-difference-percent",
            &th.gamma_dose_percent.to_string(),
            "--distance-to-agreement-mm",
            &th.gamma_dta_mm.to_string(),
            "--dose-threshold-percent",
            &th.gamma_cutoff_percent.to_string(),
            "--gamma-volume",
            "--output",
            &dir("gamma.json"),
        ])),
        Action::Analyze,
    ];

    let options = json!({
        "particles": resolved.particles,
        "histories": resolved.histories,
        "batches": resolved.batches,
        "threads": resolved.threads,
        "timeout_seconds": resolved.timeout_seconds,
        "openmc": resolved.openmc,
        "openmc_sha256": openbnct_evidence::sha256_file(&resolved.openmc)?,
        "cross_sections": resolved.cross_sections,
        "cross_sections_sha256": openbnct_evidence::sha256_file(&resolved.cross_sections)?,
        "thermal_scattering": resolved.thermal,
        "thresholds": resolved.thresholds,
        "structures": resolved.structures,
    });
    Ok(Plan {
        actions,
        inputs,
        options,
    })
}

/// Verify the finished `project run`: every step recorded, complete and
/// its outputs intact.
fn require_completed_run(project: &Path) -> DynResult<RunManifest> {
    let Some(manifest) = load_manifest(project)? else {
        return fail(format!(
            "no run manifest: run `openbnct project run {}` first",
            project.display()
        ));
    };
    for id in super::STEP_IDS.iter().take(super::RUN_STEPS) {
        let Some(step) = manifest.steps.iter().find(|s| s.id == *id) else {
            return fail(format!(
                "step {id} has not run: complete `openbnct project run {}` first",
                project.display()
            ));
        };
        if step.status != "complete" {
            return fail(format!(
                "step {id} is {}: fix it and rerun `openbnct project run {}` first",
                step.status,
                project.display()
            ));
        }
        let roots: Vec<String> = step.outputs.iter().map(|o| o.path.clone()).collect();
        if hash_outputs(project, &roots)? != step.outputs {
            return fail(format!(
                "outputs of step {id} were modified since the run: rerun `openbnct project run {}` first",
                project.display()
            ));
        }
    }
    Ok(manifest)
}

/// OpenMC loses particles born exactly on a vacuum boundary, and `beam bind`
/// places the port plane on the voxel-grid face. Move a plane that sits on the
/// boundary [`SOURCE_INSET_CM`] inward; anything else is left untouched.
fn inset_source(source: &Value, geometry: &Value) -> Value {
    let mut source = source.clone();
    let axis = match source["space"]["axis"].as_str() {
        Some("x") => 0,
        Some("y") => 1,
        Some("z") => 2,
        _ => return source,
    };
    let (Some(offset), Some(origin), Some(spacing), Some(shape)) = (
        source["space"]["offset_cm"].as_f64(),
        geometry["origin_mm"][axis].as_f64(),
        geometry["spacing_mm"][axis].as_f64(),
        geometry["shape"][axis].as_f64(),
    ) else {
        return source;
    };
    let lower = (origin - spacing / 2.0) / 10.0;
    let upper = (origin + (shape - 0.5) * spacing) / 10.0;
    let tolerance = 1e-9;
    let moved = if offset <= lower + tolerance {
        Some(lower + SOURCE_INSET_CM)
    } else if offset >= upper - tolerance {
        Some(upper - SOURCE_INSET_CM)
    } else {
        None
    };
    if let Some(value) = moved {
        source["space"]["offset_cm"] = json!(value);
    }
    source
}

fn write_pretty(path: &Path, value: &Value) -> DynResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes)?;
    Ok(())
}

fn check_openmc_version(openmc: &Path) -> DynResult<()> {
    let output = std::process::Command::new(openmc)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| io::Error::other(format!("cannot run {}: {e}", openmc.display())))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if !text.contains(openbnct_openmc::TARGET_OPENMC_SOURCE_COMMIT) {
        return fail(format!(
            "{} is not the supported OpenMC {} (commit {}); `--version` said: {}",
            openmc.display(),
            openbnct_openmc::TARGET_OPENMC_VERSION,
            openbnct_openmc::TARGET_OPENMC_SOURCE_COMMIT,
            text.lines().take(2).collect::<Vec<_>>().join(" / ")
        ));
    }
    Ok(())
}

pub fn verify_project(project_arg: &Path, overrides: &VerifyOverrides) -> DynResult<()> {
    let project = project_arg
        .canonicalize()
        .map_err(|error| io::Error::other(format!("{}: {error}", project_arg.display())))?;
    let (config, _) = load_config(&project)?;
    let mut manifest = require_completed_run(&project)?;
    let rois = read_roi_index(&project)?;
    let names: Vec<String> = rois.iter().map(|r| r.name.clone()).collect();
    config.validate_rois(&names)?;
    for name in &config.verify.structures {
        if !names.contains(name) {
            return fail(format!(
                "project.toml: [verify] structures names {name:?}, which is not an ROI in the \
                 study; available ROIs: {}",
                names.join(", ")
            ));
        }
    }

    let multigroup = resolve_spec(&project, &config.materials.multigroup_data)?;
    let data: Value = serde_json::from_slice(&fs::read(project.join(&multigroup))?)?;
    let structures: Vec<String> = if config.verify.structures.is_empty() {
        report_structures(&config, &rois)
            .iter()
            .map(|r| r.name.clone())
            .collect()
    } else {
        config.verify.structures.clone()
    };
    let resolved = resolve_settings(
        &config.verify,
        overrides,
        deterministic_thermal_nuclides(&data),
        structures,
        &|key| std::env::var(key).ok(),
    )?;
    check_openmc_version(&resolved.openmc)?;

    let plan = plan_verify(&project, &config, &resolved, &rois)?;
    let commands: Vec<String> = plan.actions.iter().map(Action::text).collect();

    println!("project {} ({})", config.project.id, project.display());
    let position = manifest.steps.iter().position(|s| s.id == VERIFY_STEP_ID);
    let current = !overrides.force
        && super::step_is_current(
            &project,
            position.map(|p| &manifest.steps[p]),
            &super::StepPlan {
                commands: Vec::new(),
                inputs: plan.inputs.clone(),
                outputs: vec![VERIFY_DIR.to_owned()],
                options: plan.options.clone(),
            },
            &commands,
        )?;
    let tag = "[verify]";
    if current {
        println!("{tag} skipped (inputs, options and outputs unchanged; --force reruns)");
        println!("report: {}", project.join("out/report.md").display());
        return Ok(());
    }

    let _cwd = CwdGuard(std::env::current_dir()?);
    std::env::set_current_dir(&project)?;
    println!(
        "{tag} running OpenMC: {} histories in {} batches on {} threads",
        resolved.histories, resolved.batches, resolved.threads
    );
    remove_outputs(&project, &[VERIFY_DIR.to_owned()])?;
    fs::create_dir_all(project.join(VERIFY_DIR))?;
    let started_utc = utc_now();
    let timer = Instant::now();
    let mut inputs = Vec::new();
    for path in &plan.inputs {
        inputs.push(hash_path(&project, path)?);
    }
    let outcome: DynResult<()> = (|| {
        for action in &plan.actions {
            match action {
                Action::Command(argv) => execute_command(argv)?,
                Action::WriteSource => {
                    let case: Value =
                        serde_json::from_slice(&fs::read(project.join("out/03-beam/case.json"))?)?;
                    let source = inset_source(&case["source"], &case["geometry"]);
                    write_pretty(&project.join(VERIFY_DIR).join("source.json"), &source)?;
                    let mut deck_case = case.clone();
                    deck_case["source"] = source;
                    write_pretty(&project.join(VERIFY_DIR).join("case.json"), &deck_case)?;
                }
                Action::WriteExecutionProfile => {
                    let mut profile: Value = serde_json::from_slice(&fs::read(
                        project.join("inputs/openmc-execution-profile-smoke.json"),
                    )?)?;
                    profile["schema_version"] = json!("openbnct.openmc-execution-profile/0.2.0");
                    profile["id"] = json!(format!("{}.verify.smoke", config.project.id));
                    profile["batches"] = json!(resolved.batches);
                    profile["requested_histories"] = json!(resolved.histories);
                    write_pretty(
                        &project.join(VERIFY_DIR).join("execution-profile.json"),
                        &profile,
                    )?;
                }
                Action::Analyze => {
                    analyze(
                        &project,
                        &config,
                        &resolved,
                        &rois,
                        timer.elapsed().as_secs_f64(),
                    )?;
                }
            }
        }
        Ok(())
    })();
    let seconds = timer.elapsed().as_secs_f64();
    let mut record = StepRecord {
        id: VERIFY_STEP_ID.into(),
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
            record.outputs = hash_outputs(&project, &[VERIFY_DIR.to_owned()])?;
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
        save_manifest(&project, &manifest)?;
        let message = manifest
            .steps
            .last()
            .and_then(|s| s.error.clone())
            .unwrap_or_default();
        println!("{tag} FAILED after {seconds:.1} s");
        return fail(format!("step {VERIFY_STEP_ID} failed: {message}"));
    }

    // Fold the result into the report, then re-record the report step so a
    // later `project run` sees it as current.
    write_report(&project, &config, &manifest, &rois)?;
    let plan7 = plan_step(6, &project, &config, &rois)?;
    if let Some(step7) = manifest.steps.iter_mut().find(|s| s.id == "07-report") {
        step7.inputs = plan7
            .inputs
            .iter()
            .map(|p| hash_path(&project, p))
            .collect::<DynResult<Vec<FileRef>>>()?;
        step7.outputs = hash_outputs(&project, &plan7.outputs)?;
    }
    save_manifest(&project, &manifest)?;
    println!("{tag} done in {seconds:.1} s");
    if let VerifyState::Fresh(summary) = load_state(&project, &manifest)? {
        println!("{}", summary.verdict.text);
    }
    println!("report: {}", project.join("out/report.md").display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

fn analyze(
    project: &Path,
    config: &ProjectConfig,
    resolved: &Resolved,
    rois: &[RoiEntry],
    wall_seconds: f64,
) -> DynResult<()> {
    let sn: PhysicalDoseBundle =
        serde_json::from_slice(&fs::read(project.join("out/05-boron/dose.json"))?)?;
    let mc: PhysicalDoseBundle =
        serde_json::from_slice(&fs::read(project.join(VERIFY_DIR).join("mc-dose.json"))?)?;
    let voxel_count = sn.geometry.voxel_count()?;
    if mc.geometry.voxel_count()? != voxel_count {
        return fail("MC and S_N dose bundles are on different grids");
    }
    let gamma: openbnct_evidence::GammaEvaluation =
        serde_json::from_slice(&fs::read(project.join(VERIFY_DIR).join("gamma.json"))?)?;
    let comparison: openbnct_evidence::DoseComparison =
        serde_json::from_slice(&fs::read(project.join(VERIFY_DIR).join("comparison.json"))?)?;
    let total_gamma = gamma
        .results
        .iter()
        .find(|r| r.quantity == "physical_total")
        .and_then(|r| r.gamma_volume.as_ref())
        .ok_or_else(|| io::Error::other("gamma record carries no total-dose gamma volume"))?;

    // Structures covered by the verdict, in ROI order.
    let mut structure_results = Vec::new();
    let mut verdict_inputs: Vec<(String, StructureStatus)> = Vec::new();
    for roi in rois
        .iter()
        .filter(|r| resolved.structures.contains(&r.name))
    {
        let mask: RegionMask = serde_json::from_slice(&fs::read(project.join(mask_path(roi)))?)?;
        if mask.voxels.len() != voxel_count {
            return fail(format!(
                "mask {} has {} voxels but the dose grid has {voxel_count}",
                roi.name,
                mask.voxels.len()
            ));
        }
        let mut quantities = Vec::new();
        for (quantity, _) in QUANTITIES {
            let (sn_values, _) = column_values(&sn, quantity)?;
            let (mc_values, mc_sigma) = column_values(&mc, quantity)?;
            quantities.push(quantity_stat(
                quantity,
                sn_values,
                mc_values,
                mc_sigma,
                &mask.voxels,
            ));
        }
        let (evaluated, passed) = gamma_in_mask(total_gamma, &mask.voxels);
        let gamma_pass_rate = (evaluated > 0).then(|| passed as f64 / evaluated as f64);
        let total = quantities.last().expect("total is the last quantity");
        let check = StructureCheck {
            name: roi.name.clone(),
            sn_mean: total.sn_mean,
            mc_mean: total.mc_mean,
            mc_rel_sigma: total.mc_rel_sigma,
            gamma_pass_rate,
        };
        let status = evaluate_structure(&check, &resolved.thresholds);
        let reasons = match &status {
            StructureStatus::Fails(r) | StructureStatus::Unresolved(r) => r.clone(),
            _ => Vec::new(),
        };
        structure_results.push(StructureResult {
            name: roi.name.clone(),
            voxels: roi.voxels,
            quantities,
            gamma_evaluated: evaluated,
            gamma_pass_rate,
            status: status.label().to_owned(),
            reasons,
        });
        verdict_inputs.push((roi.name.clone(), status));
    }

    // MC statistics of the total dose above the low-dose cutoff.
    let (mc_total, mc_total_sigma) = column_values(&mc, "physical_total")?;
    let max_total = mc_total.iter().copied().fold(0.0_f64, f64::max);
    let cutoff = max_total * resolved.thresholds.gamma_cutoff_percent / 100.0;
    let mut rel: Vec<f64> = match mc_total_sigma {
        Some(sigma) => mc_total
            .iter()
            .zip(sigma)
            .filter(|(v, _)| **v > 0.0 && **v >= cutoff)
            .map(|(v, s)| s / v)
            .collect(),
        None => Vec::new(),
    };
    rel.sort_by(f64::total_cmp);
    let statistics = McStatistics {
        voxels_evaluated: rel.len() as u64,
        cutoff_percent: resolved.thresholds.gamma_cutoff_percent,
        median_rel_sigma_total: percentile(&rel, 0.5),
        p95_rel_sigma_total: percentile(&rel, 0.95),
    };

    // Whole-volume agreement from the comparison and gamma records.
    let mut whole_volume = Vec::new();
    for (quantity, _) in QUANTITIES {
        let g = gamma.results.iter().find(|r| r.quantity == quantity);
        let c = comparison
            .quantities
            .iter()
            .find(|q| q.quantity == quantity);
        if let (Some(g), Some(c)) = (g, c) {
            whole_volume.push(WholeVolumeResult {
                quantity: quantity.to_owned(),
                gamma_pass_rate: g.pass_rate,
                gamma_evaluated: g.voxels_evaluated,
                within_sigma_fraction: c.within_sigma_fraction,
                max_normalized_difference: c.max_normalized_difference,
            });
        }
    }

    // Ratio volumes (S_N / MC).
    for (quantity, stem) in QUANTITIES {
        let (sn_values, _) = column_values(&sn, quantity)?;
        let (mc_values, _) = column_values(&mc, quantity)?;
        let max_mc = mc_values.iter().copied().fold(0.0_f64, f64::max);
        let floor = max_mc * RATIO_VOLUME_FLOOR;
        let values: Vec<f64> = sn_values
            .iter()
            .zip(mc_values)
            .map(|(s, m)| {
                if *m > 0.0 && *m >= floor {
                    s / m
                } else {
                    f64::NAN
                }
            })
            .collect();
        let image = openbnct_nifti::NiftiImage {
            geometry: sn.geometry.clone(),
            values,
            datatype: 64,
            transform_source: "sform",
            description: format!("openbnct S_N/MC ratio {quantity}"),
            intent_name: String::new(),
            units_declared_mm: true,
        };
        openbnct_nifti::write_nifti(
            &image,
            &project.join(VERIFY_DIR).join(format!("ratio-{stem}.nii")),
        )?;
    }

    let thermal_consistent = {
        let sn_set: BTreeSet<&str> = resolved.sn_thermal.iter().map(String::as_str).collect();
        let mc_set: BTreeSet<&str> = resolved
            .thermal
            .iter()
            .filter_map(|spec| spec.split_once('=').map(|(n, _)| n))
            .collect();
        sn_set == mc_set
    };
    let summary = VerifySummary {
        schema_version: VERIFY_SCHEMA.into(),
        project_id: config.project.id.clone(),
        generated_utc: utc_now(),
        engine: EngineInfo {
            openmc_commit: openbnct_openmc::TARGET_OPENMC_SOURCE_COMMIT.into(),
            executable: resolved.openmc.display().to_string(),
            cross_sections: resolved.cross_sections.display().to_string(),
            particles_requested: resolved.particles,
            histories: resolved.histories,
            batches: resolved.batches,
            threads: resolved.threads,
            wall_seconds,
        },
        thermal_scattering: ThermalInfo {
            sn: resolved.sn_thermal.clone(),
            mc: resolved.thermal.clone(),
            consistent: thermal_consistent,
        },
        thresholds: resolved.thresholds,
        statistics,
        structures: structure_results,
        whole_volume,
        verdict: overall_verdict(&verdict_inputs, &resolved.thresholds),
    };
    write_pretty(
        &project.join(SUMMARY_PATH),
        &serde_json::to_value(&summary)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> Thresholds {
        Thresholds {
            ratio_tolerance: 0.05,
            gamma_pass_min: 0.95,
            gamma_dose_percent: 3.0,
            gamma_dta_mm: 3.0,
            gamma_cutoff_percent: 10.0,
        }
    }

    fn check(sn: f64, mc: f64, sigma: f64, gamma: f64) -> StructureCheck {
        StructureCheck {
            name: "S".into(),
            sn_mean: sn,
            mc_mean: mc,
            mc_rel_sigma: Some(sigma),
            gamma_pass_rate: Some(gamma),
        }
    }

    #[test]
    fn agreement_within_tolerance_and_gamma() {
        let status = evaluate_structure(&check(1.04, 1.0, 0.005, 0.97), &th());
        assert_eq!(status, StructureStatus::Agrees);
        // The tolerance is symmetric.
        assert_eq!(
            evaluate_structure(&check(0.96, 1.0, 0.005, 0.97), &th()),
            StructureStatus::Agrees
        );
    }

    #[test]
    fn ratio_outside_tolerance_fails_when_resolved() {
        match evaluate_structure(&check(1.08, 1.0, 0.005, 0.99), &th()) {
            StructureStatus::Fails(reasons) => {
                assert_eq!(reasons.len(), 1);
                assert!(reasons[0].contains("1.080"), "{reasons:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn low_gamma_fails_even_with_good_ratio() {
        match evaluate_structure(&check(1.0, 1.0, 0.005, 0.80), &th()) {
            StructureStatus::Fails(reasons) => assert!(reasons[0].contains("80.0%")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn noisy_mc_makes_a_failure_unresolved_not_a_disagreement() {
        // 2 sigma = 20% > 5% tolerance: the MC cannot resolve the test.
        assert!(matches!(
            evaluate_structure(&check(1.3, 1.0, 0.10, 0.99), &th()),
            StructureStatus::Unresolved(_)
        ));
        // A pass is still a pass however noisy.
        assert_eq!(
            evaluate_structure(&check(1.01, 1.0, 0.10, 0.99), &th()),
            StructureStatus::Agrees
        );
    }

    #[test]
    fn zero_dose_structures_are_skipped_and_zero_mc_against_dose_fails() {
        assert_eq!(
            evaluate_structure(&check(0.0, 0.0, 0.0, 1.0), &th()),
            StructureStatus::NoDose
        );
        assert!(matches!(
            evaluate_structure(&check(1.0, 0.0, 0.0, 0.5), &th()),
            StructureStatus::Fails(_)
        ));
    }

    #[test]
    fn overall_verdict_states_thresholds() {
        let agree = overall_verdict(
            &[
                ("A".into(), StructureStatus::Agrees),
                ("B".into(), StructureStatus::NoDose),
            ],
            &th(),
        );
        assert_eq!(agree.label, "AGREES");
        assert!(
            agree
                .text
                .starts_with("AGREES: all evaluated structure-mean")
        );
        assert!(agree.text.contains("+-5.0%"), "{}", agree.text);
        assert!(agree.text.contains("3%/3 mm"), "{}", agree.text);
        assert!(agree.text.contains(">= 95.0%"), "{}", agree.text);

        let disagree = overall_verdict(
            &[
                ("A".into(), StructureStatus::Agrees),
                (
                    "GTV".into(),
                    StructureStatus::Fails(vec![
                        "total-dose ratio 1.200 is outside 1 +- 5.0%".into(),
                    ]),
                ),
                ("N".into(), StructureStatus::Unresolved(vec!["x".into()])),
            ],
            &th(),
        );
        assert_eq!(disagree.label, "DISAGREES");
        assert!(disagree.text.contains("GTV: total-dose ratio 1.200"));

        let inconclusive = overall_verdict(
            &[("N".into(), StructureStatus::Unresolved(vec!["x".into()]))],
            &th(),
        );
        assert_eq!(inconclusive.label, "INCONCLUSIVE");
        assert!(inconclusive.text.contains("N"));

        let empty = overall_verdict(&[("A".into(), StructureStatus::NoDose)], &th());
        assert_eq!(empty.label, "INCONCLUSIVE");
    }

    #[test]
    fn thresholds_are_configurable() {
        let strict = Thresholds {
            ratio_tolerance: 0.01,
            ..th()
        };
        assert!(matches!(
            evaluate_structure(&check(1.04, 1.0, 0.001, 0.99), &strict),
            StructureStatus::Fails(_)
        ));
        let verdict = overall_verdict(&[("A".into(), StructureStatus::Agrees)], &strict);
        assert!(verdict.text.contains("+-1.0%"), "{}", verdict.text);
    }

    #[test]
    fn masked_means_ratio_and_correlated_sigma() {
        let sn = [2.0, 4.0, 100.0];
        let mc = [1.0, 4.0, 100.0];
        let sigma = [0.1, 0.4, 5.0];
        let mask = [true, true, false];
        let stat = quantity_stat("physical_total", &sn, &mc, Some(&sigma), &mask);
        assert!((stat.sn_mean - 3.0).abs() < 1e-12);
        assert!((stat.mc_mean - 2.5).abs() < 1e-12);
        assert!((stat.ratio.unwrap() - 1.2).abs() < 1e-12);
        // (0.1 + 0.4) / (1 + 4)
        assert!((stat.mc_rel_sigma.unwrap() - 0.1).abs() < 1e-12);
        let zero = quantity_stat("q", &[1.0], &[0.0], None, &[true]);
        assert!(zero.ratio.is_none() && zero.mc_rel_sigma.is_none());
    }

    #[test]
    fn gamma_pass_rate_counts_only_evaluated_voxels_in_the_mask() {
        let volume = [Some(0.5), None, Some(1.5), Some(1.0), Some(9.0)];
        let mask = [true, true, true, true, false];
        assert_eq!(gamma_in_mask(&volume, &mask), (3, 2));
    }

    #[test]
    fn counts_parse_scientific_notation() {
        assert_eq!(parse_count("1e6"), Ok(1_000_000));
        assert_eq!(parse_count("20_000"), Ok(20_000));
        assert!(parse_count("1.5").is_err());
        assert!(parse_count("0").is_err());
        assert!(parse_count("many").is_err());
    }

    #[test]
    fn verify_table_parses_floats_and_rejects_unknown_keys() {
        let section: VerifySection = toml::from_str("particles = 2e4\nbatches = 4\n").unwrap();
        assert_eq!(section.particles, Some(20_000));
        assert!(toml::from_str::<VerifySection>("particles = 2.5\n").is_err());
        assert!(toml::from_str::<VerifySection>("colour = 1\n").is_err());
        let bad: VerifySection = toml::from_str("ratio_tolerance = 5\n").unwrap();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn deterministic_thermal_nuclides_read_the_collapse_declaration() {
        let data = json!({
            "collapse_declaration": "x; bound-atom incoherent-inelastic S(a,b) transfer applied for [H1] at 293.6 K; y"
        });
        assert_eq!(deterministic_thermal_nuclides(&data), vec!["H1".to_owned()]);
        assert!(
            deterministic_thermal_nuclides(&json!({"collapse_declaration": "free gas"})).is_empty()
        );
        assert!(deterministic_thermal_nuclides(&json!({})).is_empty());
    }

    fn fake_files(dir: &Path) -> (PathBuf, PathBuf) {
        let openmc = dir.join("openmc");
        fs::write(&openmc, b"#!/bin/sh\n").unwrap();
        let data = dir.join("data");
        fs::create_dir(&data).unwrap();
        let xs = data.join("cross_sections.xml");
        fs::write(&xs, b"<cross_sections/>").unwrap();
        (openmc, xs)
    }

    #[test]
    fn settings_resolve_flags_then_toml_then_env_and_derive_thermal() {
        let dir = tempfile::tempdir().unwrap();
        let (openmc, xs) = fake_files(dir.path());
        let env_openmc = openmc.to_string_lossy().into_owned();
        let env_xs = xs.to_string_lossy().into_owned();
        let env = move |key: &str| match key {
            "OPENBNCT_OPENMC" => Some(env_openmc.clone()),
            "OPENMC_CROSS_SECTIONS" => Some(env_xs.clone()),
            _ => None,
        };
        let section = VerifySection {
            batches: Some(5),
            ..VerifySection::default()
        };
        let overrides = VerifyOverrides {
            particles: Some(20_003),
            ..VerifyOverrides::default()
        };
        let resolved = resolve_settings(
            &section,
            &overrides,
            vec!["H1".into()],
            vec!["A".into()],
            &env,
        )
        .unwrap();
        assert_eq!(resolved.batches, 5);
        // Rounded up to a whole number of batches.
        assert_eq!(resolved.histories, 20_005);
        assert_eq!(resolved.threads, 2);
        assert_eq!(resolved.thermal, vec!["H1=c_H_in_H2O".to_owned()]);
        assert_eq!(
            resolved.data_root,
            xs.parent().unwrap().canonicalize().unwrap()
        );

        // A deterministic kernel with no default table needs an explicit one.
        let err =
            resolve_settings(&section, &overrides, vec!["Be9".into()], vec![], &env).unwrap_err();
        assert!(err.to_string().contains("Be9"), "{err}");
    }

    #[test]
    fn missing_openmc_or_nuclear_data_is_a_clear_error() {
        let none = |_: &str| None;
        let err = resolve_settings(
            &VerifySection::default(),
            &VerifyOverrides::default(),
            vec![],
            vec![],
            &none,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("OPENBNCT_OPENMC") && err.contains("--openmc"),
            "{err}"
        );

        let dir = tempfile::tempdir().unwrap();
        let (openmc, _) = fake_files(dir.path());
        let env_openmc = openmc.to_string_lossy().into_owned();
        let only_openmc = move |key: &str| (key == "OPENBNCT_OPENMC").then(|| env_openmc.clone());
        let err = resolve_settings(
            &VerifySection::default(),
            &VerifyOverrides::default(),
            vec![],
            vec![],
            &only_openmc,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("OPENMC_CROSS_SECTIONS") && err.contains("--cross-sections"),
            "{err}"
        );
    }

    fn summary(label: &str) -> VerifySummary {
        let stat = |q: &str, ratio: f64| QuantityStat {
            quantity: q.into(),
            sn_mean: ratio,
            mc_mean: 1.0,
            ratio: Some(ratio),
            mc_rel_sigma: Some(0.01),
        };
        VerifySummary {
            schema_version: VERIFY_SCHEMA.into(),
            project_id: "t.p1".into(),
            generated_utc: "2026-09-29T00:00:00Z".into(),
            engine: EngineInfo {
                openmc_commit: "617d35a5063c57796b43428bc401e627d2011046".into(),
                executable: "/usr/bin/openmc".into(),
                cross_sections: "/data/cross_sections.xml".into(),
                particles_requested: 1_000_000,
                histories: 1_000_000,
                batches: 10,
                threads: 2,
                wall_seconds: 12.0,
            },
            thermal_scattering: ThermalInfo {
                sn: vec!["H1".into()],
                mc: vec!["H1=c_H_in_H2O".into()],
                consistent: true,
            },
            thresholds: th(),
            statistics: McStatistics {
                voxels_evaluated: 100,
                cutoff_percent: 10.0,
                median_rel_sigma_total: 0.01,
                p95_rel_sigma_total: 0.03,
            },
            structures: vec![StructureResult {
                name: "CORE".into(),
                voxels: 27,
                quantities: vec![
                    stat("component:boron", 1.02),
                    stat("component:nitrogen", 0.98),
                    stat("component:hydrogen", 1.10),
                    stat("component:photon", 0.90),
                    stat("physical_total", 1.03),
                ],
                gamma_evaluated: 27,
                gamma_pass_rate: Some(0.99),
                status: "agrees".into(),
                reasons: vec![],
            }],
            whole_volume: vec![WholeVolumeResult {
                quantity: "physical_total".into(),
                gamma_pass_rate: 0.97,
                gamma_evaluated: 500,
                within_sigma_fraction: Some(0.8),
                max_normalized_difference: 0.04,
            }],
            verdict: Verdict {
                label: label.into(),
                text: format!("{label}: test verdict"),
            },
        }
    }

    #[test]
    fn report_section_carries_verdict_thresholds_ratios_and_statistics() {
        let md = render_section(&VerifyState::Fresh(Box::new(summary("AGREES"))));
        assert!(md.starts_with("## Independent Monte Carlo check"));
        assert!(md.contains("**Verdict: AGREES: test verdict**"), "{md}");
        assert!(md.contains("within +-5.0% of 1"), "{md}");
        assert!(md.contains("pass rate is at least 95.0%"), "{md}");
        assert!(md.contains("| CORE | 27 | 1.020 | 0.980 | 1.100 | 0.900 | 1.030 | 2.0% | 99.0% (27 vox) | agrees |"), "{md}");
        assert!(
            md.contains("median relative 1-sigma 1.0%, 95th percentile 3.0%"),
            "{md}"
        );
        assert!(
            md.contains("S_N: bound-atom kernel for H1; MC: H1=c_H_in_H2O; consistent"),
            "{md}"
        );
        assert!(
            md.contains("| physical_total | 97.0% | 500 | 80.0% | 0.040 |"),
            "{md}"
        );
    }

    #[test]
    fn report_section_handles_missing_and_stale_verification() {
        assert!(render_section(&VerifyState::NotRun).contains("Not run."));
        assert!(render_section(&VerifyState::Stale).contains("has changed"));
        assert_eq!(report_json(&VerifyState::NotRun)["status"], "not_run");
        assert_eq!(report_json(&VerifyState::Stale)["status"], "stale");
        let fresh = report_json(&VerifyState::Fresh(Box::new(summary("DISAGREES"))));
        assert_eq!(fresh["status"], "fresh");
        assert_eq!(fresh["verification"]["verdict"]["label"], "DISAGREES");
    }

    #[test]
    fn accuracy_line_mentions_a_fresh_verification_only() {
        assert!(accuracy_suffix(&VerifyState::NotRun).contains("No independent Monte Carlo check"));
        let fresh = accuracy_suffix(&VerifyState::Fresh(Box::new(summary("AGREES"))));
        assert!(fresh.contains("independent continuous-energy OpenMC check"));
        assert!(fresh.contains("verdict: AGREES"));
        assert!(accuracy_suffix(&VerifyState::Stale).contains("changed since"));
    }
}
