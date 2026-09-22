// SPDX-License-Identifier: MIT

//! OpenMC weight-window resolution and variance-reduction validation.
//!
//! `resolve_weight_windows` turns a transport-neutral
//! `openbnct.variance-reduction/0.1.0` spec into a concrete
//! `openbnct.weight-windows/0.1.0` artifact. `uniform` and `explicit`
//! bounds resolve trivially; `forward_flux` bounds are derived from a
//! completed run's mesh tally using the same rule OpenMC's own MAGIC
//! generator applies: per energy group,
//! `lower(e,m) = tally_mean(e,m) / cell_volume / (2 * group_max(e))`,
//! `upper = lower * survival_ratio`, and cells whose tally relative
//! error exceeds the spec's threshold are left disabled (negative
//! bounds). The resolved artifact records the generating statepoint's
//! content hash so every window traceably descends from an analog run.
//!
//! `validate_variance_reduction` compares a completed
//! variance-reduced run — evaluated through the ordinary acceptance
//! machinery — against a reference acceptance report from analog
//! transport. Two claims are checked honestly:
//!
//! * *Unbiasedness*: every shared region/tally mean must agree within
//!   a sigma-normalized limit (`|Δ| / hypot(σ_vr, σ_ref)`).
//! * *Efficiency*: the VR run must use materially fewer histories than
//!   the reference; the achieved reduction factor is recorded, not
//!   assumed.

use std::collections::BTreeMap;
use std::path::Path;

use openbnct_core::{ContentReference, DoseComponent};
use openbnct_transport::{
    ResolvedWeightWindow, ResolvedWeightWindows, VarianceReductionSpec, WEIGHT_WINDOWS_SCHEMA,
    WeightWindowBounds, WeightWindowDerivation,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::acceptance::{
    OpenMcAcceptanceError, OpenMcAcceptanceReport, OpenMcRegionResult, evaluate_runs,
};
use crate::input::OpenMcInputManifest;
use crate::statepoint::{OpenMcStatepoint, latest_statepoint};

pub const VR_VALIDATION_SCHEMA: &str = "openbnct.vr-validation/0.1.0";
/// Resolved weight-window artifact carried inside a generated deck.
pub const RESOLVED_WW_FILE: &str = "openbnct-weight-windows.json";

/// Resolves a variance-reduction spec into concrete weight windows.
/// `statepoint` is required when any window declares `forward_flux`
/// bounds and ignored otherwise.
pub fn resolve_weight_windows(
    spec: &VarianceReductionSpec,
    spec_sha256: &str,
    resolved_id: &str,
    statepoint: Option<(&OpenMcStatepoint, &str, &Path)>,
) -> Result<ResolvedWeightWindows, VarianceReductionError> {
    let mut windows = Vec::with_capacity(spec.windows.len());
    let mut methods: Vec<&'static str> = Vec::with_capacity(spec.windows.len());
    let mut boost_notes: Vec<String> = Vec::new();
    for (index, window) in spec.windows.iter().enumerate() {
        let (mut lower, mut upper) = match &window.bounds {
            WeightWindowBounds::Uniform { lower_bound } => {
                methods.push("uniform");
                let cells = window.energy_groups() * window.mesh.n_bins();
                let lower = vec![*lower_bound; cells];
                let upper: Vec<f64> = lower
                    .iter()
                    .map(|l| l * window.parameters.survival_ratio)
                    .collect();
                (lower, upper)
            }
            WeightWindowBounds::Explicit {
                lower_bounds,
                upper_bounds,
            } => {
                methods.push("explicit");
                (lower_bounds.clone(), upper_bounds.clone())
            }
            WeightWindowBounds::ForwardFlux {
                tally,
                rel_err_threshold,
            } => {
                methods.push("forward_flux");
                let Some((sp, _, sp_path)) = statepoint else {
                    return Err(VarianceReductionError::MissingStatepoint(tally.clone()));
                };
                derive_forward_flux(window, sp, tally, *rel_err_threshold, sp_path)?
            }
            WeightWindowBounds::Adjoint { .. } => {
                return Err(VarianceReductionError::AdjointResolution);
            }
        };
        let boosted = window.apply_bound_boosts(&mut lower, &mut upper);
        if boosted > 0 {
            let factors: Vec<String> = window
                .bound_boosts
                .iter()
                .map(|b| format!("x{}", b.factor))
                .collect();
            boost_notes.push(format!(
                "window {index}: {boosted} mesh cell(s) scaled ({})",
                factors.join(", ")
            ));
        }
        windows.push(ResolvedWeightWindow {
            particle: window.particle,
            mesh: window.mesh.clone(),
            energy_bounds_ev: window.energy_bounds_ev.clone(),
            lower_bounds: lower,
            upper_bounds: upper,
            parameters: window.parameters,
        });
    }
    let method = if methods.iter().all(|m| *m == methods[0]) {
        methods[0].to_string()
    } else {
        "mixed".to_string()
    };
    let resolved = ResolvedWeightWindows {
        schema_version: WEIGHT_WINDOWS_SCHEMA.into(),
        id: resolved_id.to_string(),
        case_id: spec.case_id.clone(),
        spec: ContentReference {
            id: spec.id.clone(),
            sha256: spec_sha256.to_string(),
        },
        windows,
        derivation: WeightWindowDerivation {
            method,
            adjoint_flux: Vec::new(),
            forward_flux: None,
            multigroup_data: None,
            transport_case: None,
            source_statepoint: statepoint.map(|(_, sha, path)| ContentReference {
                id: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "statepoint".into()),
                sha256: sha.to_string(),
            }),
            note: derivation_note(spec, &boost_notes),
        },
        qualification: spec.qualification.clone(),
    };
    resolved
        .validate()
        .map_err(VarianceReductionError::ResolvedInvalid)?;
    Ok(resolved)
}

fn derivation_note(spec: &VarianceReductionSpec, boost_notes: &[String]) -> String {
    let mut parts = Vec::new();
    for (index, window) in spec.windows.iter().enumerate() {
        let detail = match &window.bounds {
            WeightWindowBounds::Uniform { lower_bound } => {
                format!("window {index} uniform lower={lower_bound}")
            }
            WeightWindowBounds::Explicit { .. } => {
                format!("window {index} explicit bounds")
            }
            WeightWindowBounds::ForwardFlux {
                tally,
                rel_err_threshold,
            } => format!(
                "window {index} MAGIC-equivalent from tally {tally} \
                 (rel_err_threshold={rel_err_threshold}, \
                 survival_ratio={})",
                window.parameters.survival_ratio
            ),
            WeightWindowBounds::Adjoint { .. } => {
                format!("window {index} adjoint-derived (resolve via vr cadis)")
            }
        };
        parts.push(detail);
    }
    parts.extend(boost_notes.iter().cloned());
    parts.join("; ")
}

/// MAGIC-equivalent derivation: per energy group the lower bound is the
/// volume-normalized tally mean scaled by `1/(2 * group_max)`, and the
/// upper bound is `lower * survival_ratio`. Cells whose tally mean is
/// nonpositive or whose relative error exceeds `rel_err_threshold` are
/// disabled (negative bounds), matching OpenMC's own generator.
fn derive_forward_flux(
    window: &openbnct_transport::WeightWindowSpec,
    statepoint: &OpenMcStatepoint,
    tally_name: &str,
    rel_err_threshold: f64,
    sp_path: &Path,
) -> Result<(Vec<f64>, Vec<f64>), VarianceReductionError> {
    let tally =
        statepoint
            .tally(tally_name)
            .ok_or_else(|| VarianceReductionError::MissingTally {
                tally: tally_name.to_string(),
                statepoint: sp_path.display().to_string(),
            })?;
    let mesh_bins = window.mesh.n_bins();
    let energy_groups = window.energy_groups();

    // The tally's energy-filter edges must match the declared window
    // groups exactly, otherwise the derivation would silently re-bin.
    let energy_filter = tally
        .filter_ids
        .iter()
        .find(|id| statepoint.energy_filter_edges.contains_key(id));
    match (window.energy_bounds_ev.as_ref(), energy_filter) {
        (Some(declared), Some(filter_id)) => {
            let edges = &statepoint.energy_filter_edges[filter_id];
            if edges.len() != declared.len()
                || !edges
                    .iter()
                    .zip(declared.iter())
                    .all(|(a, b)| (*a - *b).abs() <= 1e-9 * b.abs().max(1.0))
            {
                return Err(VarianceReductionError::EnergyGridMismatch {
                    tally: tally_name.to_string(),
                });
            }
        }
        (Some(_), None) => {
            return Err(VarianceReductionError::NoEnergyFilter {
                tally: tally_name.to_string(),
            });
        }
        (None, Some(_)) => {
            return Err(VarianceReductionError::UndeclaredEnergyGrid {
                tally: tally_name.to_string(),
            });
        }
        (None, None) => {}
    }

    // Tally bins are row-major over the filter list — for the diagnostic
    // fluence tallies [mesh, particle, energy] the layout is
    // mesh-major / energy-inner with a single particle bin.
    let expected = mesh_bins * energy_groups;
    if tally.mean.len() != expected {
        return Err(VarianceReductionError::TallyShapeMismatch {
            tally: tally_name.to_string(),
            expected,
            actual: tally.mean.len(),
        });
    }

    let cell_volume = window.mesh.cell_volume_cm3();
    let n_e = energy_groups;
    // value(e,m) = volume-normalized tally mean; OpenMC mesh tallies
    // integrate over the cell volume, so divide it back out.
    let value = |e: usize, m: usize| -> f64 { tally.mean[m * n_e + e] / cell_volume };
    let rel_err = |e: usize, m: usize| -> f64 {
        let mean = tally.mean[m * n_e + e];
        if mean > 0.0 {
            tally.standard_error[m * n_e + e] / mean
        } else {
            f64::INFINITY
        }
    };

    let mut lower = vec![-1.0_f64; expected];
    let mut upper = vec![-1.0_f64; expected];
    for e in 0..n_e {
        let group_max = (0..mesh_bins).map(|m| value(e, m)).fold(0.0_f64, f64::max);
        if group_max <= 0.0 {
            continue;
        }
        let norm = 1.0 / (2.0 * group_max);
        for m in 0..mesh_bins {
            if rel_err(e, m) > rel_err_threshold {
                continue;
            }
            let l = value(e, m) * norm;
            if l <= 0.0 {
                continue;
            }
            lower[e * mesh_bins + m] = l;
            upper[e * mesh_bins + m] = l * window.parameters.survival_ratio;
        }
    }
    Ok((lower, upper))
}

/// Result of comparing one shared region/tally between the VR run and
/// the analog reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrComparison {
    pub region: String,
    pub tally: String,
    pub component: Option<DoseComponent>,
    /// Seed of the reference run this comparison evaluated against — a
    /// multi-seed report contributes several results per region and the
    /// VR run must agree with each.
    pub reference_seed: Option<u64>,
    pub vr_reported: f64,
    pub vr_sigma: f64,
    pub reference_reported: f64,
    pub reference_sigma: f64,
    /// `|vr - ref| / hypot(vr_sigma, ref_sigma)`.
    pub z_score: f64,
    pub z_limit: f64,
    pub passed: bool,
}

/// A (region, tally) group that could not be paired between the VR and
/// reference reports — recorded, never silently treated as agreement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrSkippedGroup {
    pub region: String,
    pub tally: String,
    pub reason: String,
}

/// `openbnct.vr-validation/0.1.0` — unbiasedness and efficiency evidence
/// for a variance-reduced run against an analog reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrValidationReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// The variance-reduced run under test.
    pub vr_run: VrRunBinding,
    /// The analog reference (acceptance report + its total histories).
    pub reference: VrReferenceBinding,
    /// `reference_histories / vr_histories` — how much smaller the VR
    /// run's particle budget was.
    pub history_reduction_factor: f64,
    pub comparisons: Vec<VrComparison>,
    /// Shared (region, tally) groups that could not be position-aligned
    /// between the two reports.
    #[serde(default)]
    pub skipped: Vec<VrSkippedGroup>,
    /// Every shared region/tally agreed within `z_limit`.
    pub unbiased: bool,
    /// The VR run's own acceptance report passed all declared gates —
    /// including the photon precision gate — at the reduced budget.
    pub vr_gates_passed: bool,
    pub gates_passed: bool,
    pub qualification: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrRunBinding {
    pub input_manifest_sha256: String,
    pub statepoint_sha256: String,
    pub resolved_weight_windows: Option<ContentReference>,
    pub histories: u64,
    pub acceptance_report_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VrReferenceBinding {
    pub report_sha256: String,
    /// Histories in one reference run (the per-seed particle count; the
    /// acceptance report does not record it, so it is supplied by the
    /// caller who ran the campaign).
    pub histories: u64,
}

/// Evaluate a completed variance-reduced run directory through the
/// ordinary acceptance machinery, then compare its region means against
/// a reference acceptance report (e.g. the frozen analog reference) for
/// unbiasedness. `reference_histories` is the total particle count the
/// reference report covers; `z_limit` is the sigma-normalized agreement
/// limit (3.0 is the conventional choice).
pub fn validate_variance_reduction(
    vr_run_directory: &Path,
    vr_exit_code: i32,
    reference_report_bytes: &[u8],
    reference_histories: u64,
    z_limit: f64,
) -> Result<VrValidationReport, VarianceReductionError> {
    if z_limit.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || reference_histories == 0 {
        return Err(VarianceReductionError::InvalidArguments);
    }
    let reference: OpenMcAcceptanceReport = serde_json::from_slice(reference_report_bytes)
        .map_err(|e| VarianceReductionError::Parse(format!("reference report: {e}")))?;
    if !openbnct_core::schema_matches(
        &reference.schema_version,
        crate::acceptance::ACCEPTANCE_REPORT_SCHEMA,
    ) {
        return Err(VarianceReductionError::Parse(format!(
            "reference report schema {} is not {}",
            reference.schema_version,
            crate::acceptance::ACCEPTANCE_REPORT_SCHEMA
        )));
    }

    // The VR run is evaluated by the same acceptance machinery as any
    // candidate-reference run — its photon precision gate is checked by
    // that evaluation, not by ad-hoc logic here.
    let vr_report = evaluate_runs(&[vr_run_directory], &[vr_exit_code])
        .map_err(VarianceReductionError::VrAcceptance)?;
    let vr_report_sha256 = format!("{:x}", Sha256::digest(&vr_report_bytes(&vr_report)?));

    let manifest_bytes = std::fs::read(crate::statepoint::resolve_run_file(
        vr_run_directory,
        crate::statepoint::OPENMC_INPUT_MANIFEST_FILE,
    ))
    .map_err(|e| VarianceReductionError::Io(e.to_string()))?;
    let manifest: OpenMcInputManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| VarianceReductionError::Parse(e.to_string()))?;
    let manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
    let statepoint_path = latest_statepoint(vr_run_directory)
        .map_err(|e| VarianceReductionError::Io(e.to_string()))?;
    let statepoint_sha256 = {
        let bytes = std::fs::read(&statepoint_path)
            .map_err(|e| VarianceReductionError::Io(e.to_string()))?;
        format!("{:x}", Sha256::digest(&bytes))
    };
    let vr_histories = manifest.execution.batches as u64 * manifest.execution.particles_per_batch;
    let resolved_ww = manifest.bindings.variance_reduction.clone();

    // Pair results position-aligned within each (region, tally) group.
    // Profile-style regions emit one result per bin, and a multi-seed
    // reference report contributes one contiguous block per run — the
    // runs are manifest-identical, hence identically ordered. Each VR
    // bin must agree with the same ordinal bin of every reference seed,
    // never with every bin of every seed. Groups that cannot be aligned
    // are recorded as skipped rather than silently treated as agreement.
    let n_reference_runs = reference.runs.len();
    let n_vr_runs = vr_report.runs.len().max(1);
    // Tally names are contract ids: normalize the legacy `nctforge.` prefix so
    // a report written before the rename still pairs with current artifacts.
    let mut reference_groups: BTreeMap<(String, String), Vec<&OpenMcRegionResult>> =
        BTreeMap::new();
    for result in &reference.region_results {
        reference_groups
            .entry((
                openbnct_core::normalize_contract_id(&result.region),
                openbnct_core::normalize_contract_id(&result.tally),
            ))
            .or_default()
            .push(result);
    }
    let mut vr_groups: BTreeMap<(String, String), Vec<&OpenMcRegionResult>> = BTreeMap::new();
    for result in &vr_report.region_results {
        vr_groups
            .entry((
                openbnct_core::normalize_contract_id(&result.region),
                openbnct_core::normalize_contract_id(&result.tally),
            ))
            .or_default()
            .push(result);
    }
    let mut comparisons = Vec::new();
    let mut skipped = Vec::new();
    let mut unbiased = true;
    for ((region, tally), vr_group) in &vr_groups {
        let Some(reference_group) = reference_groups.get(&(region.clone(), tally.clone())) else {
            skipped.push(VrSkippedGroup {
                region: region.clone(),
                tally: tally.clone(),
                reason: "not present in the reference report".into(),
            });
            continue;
        };
        let Some(vr_bins) = vr_group.len().checked_div(n_vr_runs) else {
            skipped.push(VrSkippedGroup {
                region: region.clone(),
                tally: tally.clone(),
                reason: "vr run count is zero".into(),
            });
            continue;
        };
        let Some(reference_bins) = reference_group.len().checked_div(n_reference_runs) else {
            skipped.push(VrSkippedGroup {
                region: region.clone(),
                tally: tally.clone(),
                reason: "reference run count is zero".into(),
            });
            continue;
        };
        if vr_group.len() % n_vr_runs != 0
            || reference_group.len() % n_reference_runs != 0
            || vr_bins != reference_bins
        {
            skipped.push(VrSkippedGroup {
                region: region.clone(),
                tally: tally.clone(),
                reason: format!(
                    "result blocks do not align: {n_vr_runs} vr run(s) x \
                     {vr_bins} bins vs {n_reference_runs} reference run(s) x \
                     {reference_bins} bins"
                ),
            });
            continue;
        }
        for vr_run_index in 0..n_vr_runs {
            for (bin, vr) in vr_group[vr_run_index * vr_bins..(vr_run_index + 1) * vr_bins]
                .iter()
                .enumerate()
            {
                for reference_run_index in 0..n_reference_runs {
                    let reference_result =
                        reference_group[reference_run_index * reference_bins + bin];
                    let reference_seed =
                        reference.runs.get(reference_run_index).map(|run| run.seed);
                    let sigma = vr.reported_sigma.hypot(reference_result.reported_sigma);
                    let z = if sigma > 0.0 {
                        (vr.reported - reference_result.reported).abs() / sigma
                    } else if vr.reported == reference_result.reported {
                        0.0
                    } else {
                        f64::INFINITY
                    };
                    let passed = z <= z_limit;
                    unbiased &= passed;
                    comparisons.push(VrComparison {
                        region: vr.region.clone(),
                        tally: vr.tally.clone(),
                        component: vr.component,
                        reference_seed,
                        vr_reported: vr.reported,
                        vr_sigma: vr.reported_sigma,
                        reference_reported: reference_result.reported,
                        reference_sigma: reference_result.reported_sigma,
                        z_score: z,
                        z_limit,
                        passed,
                    });
                }
            }
        }
    }
    if comparisons.is_empty() {
        return Err(VarianceReductionError::NoSharedResults);
    }
    let reduction = reference_histories as f64 / vr_histories as f64;
    let gates_passed = unbiased && vr_report.gates_passed && reduction > 1.0;
    Ok(VrValidationReport {
        schema_version: VR_VALIDATION_SCHEMA.into(),
        case_id: manifest.case_id.clone(),
        vr_run: VrRunBinding {
            input_manifest_sha256: manifest_sha256,
            statepoint_sha256,
            resolved_weight_windows: resolved_ww,
            histories: vr_histories,
            acceptance_report_sha256: vr_report_sha256,
        },
        reference: VrReferenceBinding {
            report_sha256: format!("{:x}", Sha256::digest(reference_report_bytes)),
            histories: reference_histories,
        },
        history_reduction_factor: reduction,
        comparisons,
        skipped,
        unbiased,
        vr_gates_passed: vr_report.gates_passed,
        gates_passed,
        qualification: "variance_reduction_validation_research_only_not_clinical".into(),
    })
}

fn vr_report_bytes(report: &OpenMcAcceptanceReport) -> Result<Vec<u8>, VarianceReductionError> {
    serde_json::to_vec(report).map_err(|e| VarianceReductionError::Parse(e.to_string()))
}

#[derive(Debug, Error)]
pub enum VarianceReductionError {
    #[error("{0}")]
    ResolvedInvalid(#[from] openbnct_transport::VarianceReductionError),
    #[error("window tally {0} declares forward_flux bounds but no statepoint was supplied")]
    MissingStatepoint(String),
    #[error("tally {tally} not found in statepoint {statepoint}")]
    MissingTally { tally: String, statepoint: String },
    #[error("tally {tally} energy grid does not match the declared window bounds")]
    EnergyGridMismatch { tally: String },
    #[error("tally {tally} has no energy filter but the window declares energy bounds")]
    NoEnergyFilter { tally: String },
    #[error(
        "tally {tally} has an energy filter but the window declares none; \
         declare the matching energy_bounds_ev"
    )]
    UndeclaredEnergyGrid { tally: String },
    #[error("tally {tally} has {actual} bins; expected n_energy x n_mesh = {expected}")]
    TallyShapeMismatch {
        tally: String,
        expected: usize,
        actual: usize,
    },
    #[error("invalid vr validate arguments")]
    InvalidArguments,
    #[error("no shared region/tally results between the vr run and the reference")]
    NoSharedResults,
    #[error("vr run acceptance evaluation failed: {0}")]
    VrAcceptance(#[source] OpenMcAcceptanceError),
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Io(String),
    #[error(
        "adjoint bounds resolve through the in-house S_N solver          (`openbnct vr cadis`), not the statepoint path"
    )]
    AdjointResolution,
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_transport::{
        ParticleType, WeightWindowMesh, WeightWindowParameters, WeightWindowSpec,
    };

    fn spec(bounds: WeightWindowBounds) -> VarianceReductionSpec {
        VarianceReductionSpec {
            schema_version: openbnct_transport::VARIANCE_REDUCTION_SCHEMA.into(),
            id: "openbnct.test.vr.v1".into(),
            description: "test".into(),
            case_id: Some("nf-bnct-001".into()),
            windows: vec![WeightWindowSpec {
                particle: ParticleType::Photon,
                mesh: WeightWindowMesh {
                    dimensions: [2, 2, 2],
                    lower_left_cm: [-4.0; 3],
                    upper_right_cm: [4.0; 3],
                },
                energy_bounds_ev: None,
                parameters: WeightWindowParameters::default(),
                bounds,
                bound_boosts: vec![],
            }],
            provenance_note: "test".into(),
            qualification: "test_only".into(),
        }
    }

    #[test]
    fn uniform_resolves_without_statepoint() {
        let s = spec(WeightWindowBounds::Uniform { lower_bound: 0.25 });
        let resolved =
            resolve_weight_windows(&s, &"ab".repeat(32), "openbnct.test.ww.v1", None).unwrap();
        assert_eq!(resolved.windows[0].lower_bounds, vec![0.25; 8]);
        assert_eq!(resolved.windows[0].upper_bounds, vec![0.75; 8]);
        assert_eq!(resolved.derivation.method, "uniform");
        assert!(resolved.derivation.source_statepoint.is_none());
    }

    #[test]
    fn forward_flux_requires_statepoint() {
        let s = spec(WeightWindowBounds::ForwardFlux {
            tally: "openbnct.diagnostic.photon_fluence".into(),
            rel_err_threshold: 0.5,
        });
        assert!(matches!(
            resolve_weight_windows(&s, &"ab".repeat(32), "x", None),
            Err(VarianceReductionError::MissingStatepoint(_))
        ));
    }

    /// Synthetic statepoint: a 2-cell mesh, 2 energy groups, photon
    /// fluence tally. Cell m1 has 10x lower flux than m0 in both groups.
    fn synthetic_statepoint() -> OpenMcStatepoint {
        // Layout: tally filters [mesh=1, particle=3, energy=8]; bins are
        // row-major mesh-outer, energy-inner: [m0e0, m0e1, m1e0, m1e1].
        let mean = vec![100.0, 50.0, 10.0, 5.0];
        let standard_error = vec![1.0, 0.5, 0.1, 0.05];
        let tally = crate::statepoint::OpenMcStatepointTally {
            id: 10,
            name: "openbnct.diagnostic.photon_fluence".into(),
            estimator: "tracklength".into(),
            filter_ids: vec![1, 3, 8],
            score_bins: 1,
            realizations: 10,
            mean,
            standard_error,
        };
        OpenMcStatepoint {
            batches: 10,
            particles_per_batch: 100,
            realizations: 10,
            seed: 1,
            stride: 152917,
            energy_mode: "ce".into(),
            run_mode: "fixed source".into(),
            openmc_version: "0.16.0".into(),
            energy_functions: Default::default(),
            energy_filter_edges: [(8u32, vec![0.0, 1.0e6, 2.0e7])].into_iter().collect(),
            tallies: vec![tally],
        }
    }

    #[test]
    fn forward_flux_derives_magic_bounds() {
        let mut s = spec(WeightWindowBounds::ForwardFlux {
            tally: "openbnct.diagnostic.photon_fluence".into(),
            rel_err_threshold: 0.5,
        });
        // 2x1x1 mesh over 20x1x1 cm -> cell volume 10 cm3; two energy
        // groups matching the statepoint's energy filter.
        s.windows[0].mesh = WeightWindowMesh {
            dimensions: [2, 1, 1],
            lower_left_cm: [-10.0, 0.0, 0.0],
            upper_right_cm: [10.0, 1.0, 1.0],
        };
        s.windows[0].energy_bounds_ev = Some(vec![0.0, 1.0e6, 2.0e7]);
        let sp = synthetic_statepoint();
        let resolved = resolve_weight_windows(
            &s,
            &"ab".repeat(32),
            "openbnct.test.ww.v1",
            Some((&sp, "cd".repeat(32).as_str(), Path::new("sp.h5"))),
        )
        .unwrap();
        let window = &resolved.windows[0];
        // e0: means 100,10 -> /vol = 10,1 -> max 10 -> lower .5, .05
        // e1: means 50,5   -> /vol = 5,.5  -> max 5  -> lower .5, .05
        // Bounds are row-major [energy][mesh].
        assert_eq!(window.lower_bounds, vec![0.5, 0.05, 0.5, 0.05]);
        for (got, want) in window.upper_bounds.iter().zip([1.5, 0.15, 1.5, 0.15]) {
            assert!((got - want).abs() < 1e-15, "{got} vs {want}");
        }
        assert_eq!(resolved.derivation.method, "forward_flux");
        assert!(resolved.derivation.source_statepoint.is_some());
    }

    #[test]
    fn forward_flux_disables_noisy_cells() {
        let mut s = spec(WeightWindowBounds::ForwardFlux {
            tally: "openbnct.diagnostic.photon_fluence".into(),
            rel_err_threshold: 0.5,
        });
        s.windows[0].mesh = WeightWindowMesh {
            dimensions: [2, 1, 1],
            lower_left_cm: [-10.0, 0.0, 0.0],
            upper_right_cm: [10.0, 1.0, 1.0],
        };
        s.windows[0].energy_bounds_ev = Some(vec![0.0, 1.0e6, 2.0e7]);
        let mut sp = synthetic_statepoint();
        // Cell m1/e1 has 60% relative error -> disabled (negative pair).
        sp.tallies[0].standard_error[3] = 3.0;
        let resolved = resolve_weight_windows(
            &s,
            &"ab".repeat(32),
            "openbnct.test.ww.v1",
            Some((&sp, "cd".repeat(32).as_str(), Path::new("sp.h5"))),
        )
        .unwrap();
        let window = &resolved.windows[0];
        assert_eq!(window.lower_bounds[3], -1.0);
        assert_eq!(window.upper_bounds[3], -1.0);
        assert!(window.lower_bounds[0] > 0.0);
    }

    #[test]
    fn forward_flux_rejects_mismatched_energy_grid() {
        let mut s = spec(WeightWindowBounds::ForwardFlux {
            tally: "openbnct.diagnostic.photon_fluence".into(),
            rel_err_threshold: 0.5,
        });
        s.windows[0].mesh = WeightWindowMesh {
            dimensions: [2, 1, 1],
            lower_left_cm: [-10.0, 0.0, 0.0],
            upper_right_cm: [10.0, 1.0, 1.0],
        };
        // One declared group, but the tally carries two -> mismatch.
        s.windows[0].energy_bounds_ev = Some(vec![0.0, 2.0e7]);
        let sp = synthetic_statepoint();
        assert!(matches!(
            resolve_weight_windows(
                &s,
                &"ab".repeat(32),
                "x",
                Some((&sp, "cd".repeat(32).as_str(), Path::new("sp.h5"))),
            ),
            Err(VarianceReductionError::EnergyGridMismatch { .. })
                | Err(VarianceReductionError::ResolvedInvalid(_))
                | Err(VarianceReductionError::TallyShapeMismatch { .. })
        ));
    }
}
