// SPDX-License-Identifier: MIT

//! Acceptance evaluation for candidate-reference runs.
//!
//! Reads each run directory's input manifest, bound acceptance contract, and
//! statepoint; evaluates the predeclared ROI precision, voxel-level
//! precision, and estimator-comparison gates; and computes reduced
//! chi-square consistency across independent seeds. Emits a content-hashed
//! `openbnct.openmc-acceptance-report/0.1.0` document.
//!
//! These are conformance thresholds for the synthetic benchmark, not clinical
//! commissioning tolerances.

use std::path::Path;

use openbnct_core::DoseComponent;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::OPENMC_RUN_RECEIPT_FILE;
use crate::input::{
    OpenMcAcceptanceContract, OpenMcInputManifest, OpenMcTallyQuantity, OpenMcTallyScope,
};
use crate::statepoint::{OpenMcCollectError, OpenMcStatepoint, latest_statepoint};

pub const ACCEPTANCE_CONTRACT_FILE: &str = "openbnct-acceptance-contract.json";
pub const ACCEPTANCE_REPORT_SCHEMA: &str = "openbnct.openmc-acceptance-report/0.1.0";
const EV_TO_JOULE: f64 = 1.602176634e-19;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcAcceptanceReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub acceptance_id: String,
    pub acceptance_sha256: String,
    pub runs: Vec<OpenMcRunBinding>,
    pub region_results: Vec<OpenMcRegionResult>,
    pub voxel_precision: Vec<OpenMcVoxelPrecision>,
    pub estimator_comparisons: Vec<OpenMcEstimatorComparison>,
    pub chi_square: Vec<OpenMcChiSquare>,
    pub gates_passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRunBinding {
    pub seed: u64,
    pub input_manifest_sha256: String,
    pub statepoint_sha256: String,
    pub run_receipt_sha256: Option<String>,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcRegionResult {
    pub region: String,
    /// Batch mean of the region-integrated tally in its manifest raw unit.
    pub mean: f64,
    /// One-sigma standard error of `mean`.
    pub standard_error: f64,
    /// Region-mean dose in gray per source neutron, when the tally is a dose
    /// quantity; otherwise the normalized fluence or raw rate.
    pub reported: f64,
    pub reported_sigma: f64,
    pub reported_unit: String,
    pub relative_standard_error: Option<f64>,
    pub tally: String,
    pub quantity: OpenMcTallyQuantity,
    pub component: Option<DoseComponent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcVoxelPrecision {
    pub component: DoseComponent,
    pub voxels_evaluated: u32,
    pub median_relative_standard_error: f64,
    pub p95_relative_standard_error: f64,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcEstimatorComparison {
    pub region: String,
    /// Bin index within the region mesh (always 0 for single-bin regions).
    pub bin: u32,
    pub comparison: String,
    pub observed_relative_difference: f64,
    pub limit: f64,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcChiSquare {
    pub tally: String,
    pub region: String,
    pub bin: u32,
    pub chi_square: f64,
    pub degrees_of_freedom: u32,
    /// Chi-square survival probability: p < chi_square_p_min fails.
    pub p_value: f64,
    pub passed: bool,
}

struct RunData {
    manifest: OpenMcInputManifest,
    manifest_sha256: String,
    statepoint: OpenMcStatepoint,
    statepoint_sha256: String,
    receipt_sha256: Option<String>,
    acceptance_sha256: String,
    exit_code: i32,
}

/// Evaluate one or more completed candidate-reference run directories against
/// their bound acceptance contract. Each directory must contain the deck
/// manifest, the emitted acceptance contract, and a statepoint; a run receipt
/// contributes its digest when present.
pub fn evaluate_runs(
    run_directories: &[&Path],
    exit_codes: &[i32],
) -> Result<OpenMcAcceptanceReport, OpenMcAcceptanceError> {
    if run_directories.is_empty() || run_directories.len() != exit_codes.len() {
        return Err(OpenMcAcceptanceError::InvalidArguments);
    }
    let mut runs = Vec::with_capacity(run_directories.len());
    for (directory, exit_code) in run_directories.iter().zip(exit_codes.iter()) {
        runs.push(load_run(directory, *exit_code)?);
    }

    let acceptance_sha256 = runs[0].acceptance_sha256.clone();
    for run in &runs[1..] {
        if run.acceptance_sha256 != acceptance_sha256 {
            return Err(OpenMcAcceptanceError::AcceptanceMismatch);
        }
        if run.manifest.case_id != runs[0].manifest.case_id {
            return Err(OpenMcAcceptanceError::CaseMismatch);
        }
    }
    let manifest = &runs[0].manifest;
    let acceptance: OpenMcAcceptanceContract = serde_json::from_slice(
        &std::fs::read(crate::statepoint::resolve_run_file(
            run_directories[0],
            ACCEPTANCE_CONTRACT_FILE,
        ))
        .map_err(|e| OpenMcAcceptanceError::Io(e.to_string()))?,
    )
    .map_err(|e| OpenMcAcceptanceError::Parse(e.to_string()))?;
    acceptance
        .validate()
        .map_err(|e| OpenMcAcceptanceError::Contract(e.to_string()))?;
    if acceptance.case_id != manifest.case_id {
        return Err(OpenMcAcceptanceError::CaseMismatch);
    }
    if let Some(bound) = &manifest.bindings.acceptance {
        if bound.sha256 != acceptance_sha256 {
            return Err(OpenMcAcceptanceError::AcceptanceMismatch);
        }
    } else {
        return Err(OpenMcAcceptanceError::NoAcceptanceBinding);
    }
    if manifest.execution.purpose != crate::input::OpenMcExecutionPurpose::CandidateReference {
        return Err(OpenMcAcceptanceError::NotCandidateReference);
    }
    if manifest.execution.batches < acceptance.min_batches {
        return Err(OpenMcAcceptanceError::Contract(format!(
            "manifest batches {} below acceptance minimum {}",
            manifest.execution.batches, acceptance.min_batches
        )));
    }

    // Seed registration and uniqueness across the evaluated runs.
    let mut seeds = std::collections::BTreeSet::new();
    for run in &runs {
        if !acceptance.seeds.contains(&run.manifest.execution.seed) {
            return Err(OpenMcAcceptanceError::UnregisteredSeed(
                run.manifest.execution.seed,
            ));
        }
        if !seeds.insert(run.manifest.execution.seed) {
            return Err(OpenMcAcceptanceError::DuplicateSeed(
                run.manifest.execution.seed,
            ));
        }
    }
    let enough_seeds = seeds.len() >= acceptance.seeds.len().min(3);

    let gates = &acceptance.gates;
    let mut region_results = Vec::new();
    let mut voxel_precision = Vec::new();
    let mut estimator_comparisons = Vec::new();
    let mut chi_square = Vec::new();
    let mut gates_passed = true;

    // ---- Per-run region results and single-seed gates ----
    for run in &runs {
        for (roi_index, roi) in manifest.rois.iter().enumerate() {
            let contract = acceptance.regions.get(roi_index).ok_or_else(|| {
                OpenMcAcceptanceError::Contract(format!(
                    "manifest roi {} has no matching acceptance region",
                    roi.name
                ))
            })?;
            if contract.name != roi.name {
                return Err(OpenMcAcceptanceError::Contract(format!(
                    "manifest roi {} mismatches acceptance region {}",
                    roi.name, contract.name
                )));
            }
            let bins: usize = roi.dimensions.iter().product::<u32>() as usize;
            let bin_volume_cm3 = roi.volume_cm3 / bins as f64;
            let bin_mass_kg = roi.mass_g * 1.0e-3 / bins as f64;
            let precision_gated = contract.precision_gated;
            for tally_contract in &manifest.tallies {
                if tally_contract.scope != Some(OpenMcTallyScope::Acceptance)
                    || tally_contract.roi.as_deref() != Some(roi.name.as_str())
                {
                    continue;
                }
                let tally = run.statepoint.tally(&tally_contract.name).ok_or_else(|| {
                    OpenMcAcceptanceError::MissingTally(tally_contract.name.clone())
                })?;
                if tally.mean.len() != bins {
                    return Err(OpenMcAcceptanceError::BinMismatch {
                        tally: tally_contract.name.clone(),
                        expected: bins,
                        actual: tally.mean.len(),
                    });
                }
                for bin in 0..bins {
                    let (reported, reported_sigma, unit) = report_value(
                        tally_contract,
                        tally.mean[bin],
                        tally.standard_error[bin],
                        bin_volume_cm3,
                        bin_mass_kg,
                    );
                    let rel = (reported_sigma.abs() > 0.0 && reported != 0.0)
                        .then(|| reported_sigma / reported.abs());
                    region_results.push(OpenMcRegionResult {
                        region: roi.name.clone(),
                        mean: tally.mean[bin],
                        standard_error: tally.standard_error[bin],
                        reported,
                        reported_sigma,
                        reported_unit: unit,
                        relative_standard_error: rel,
                        tally: tally_contract.name.clone(),
                        quantity: tally_contract.quantity,
                        component: tally_contract.component,
                    });
                }

                // Single-bin precision gate on every nonzero dose component.
                if precision_gated
                    && bins == 1
                    && matches!(
                        tally_contract.quantity,
                        OpenMcTallyQuantity::ResponseWeightedTrackLength
                            | OpenMcTallyQuantity::Heating
                    )
                {
                    let mean = tally.mean[0];
                    let sigma = tally.standard_error[0];
                    let passed = mean != 0.0
                        && (sigma / mean.abs()) <= gates.roi_relative_standard_uncertainty_max;
                    gates_passed &= passed;
                    estimator_comparisons.push(OpenMcEstimatorComparison {
                        region: roi.name.clone(),
                        bin: 0,
                        comparison: format!("precision:{}", tally_contract.name),
                        observed_relative_difference: if mean != 0.0 {
                            sigma / mean.abs()
                        } else {
                            f64::INFINITY
                        },
                        limit: gates.roi_relative_standard_uncertainty_max,
                        passed,
                    });
                }
            }

            // Estimator comparisons per bin. For multi-bin regions only bins
            // at or above 20% of the compared quantity's maximum are gated.
            let dose_of = |tally_name: &str, bin: usize| -> Option<(f64, f64)> {
                // Match by normalized contract id: a run directory written
                // before the rename carries `nctforge.roi.*` tally names in
                // both manifest and statepoint, while the expected names
                // below are built with the current `openbnct.roi.*` prefix.
                let contract = manifest.tallies.iter().find(|c| {
                    openbnct_core::normalize_contract_id(&c.name)
                        == openbnct_core::normalize_contract_id(tally_name)
                })?;
                let tally = run.statepoint.tally(&contract.name)?;
                Some(report_value(
                    contract,
                    tally.mean[bin],
                    tally.standard_error[bin],
                    bin_volume_cm3,
                    bin_mass_kg,
                ))
                .map(|(m, s, _)| (m, s))
            };
            let name = |suffix: &str| format!("openbnct.roi.{}.{suffix}", roi.name);
            let series = |suffix: &str| -> Result<Vec<(f64, f64)>, OpenMcAcceptanceError> {
                (0..bins)
                    .map(|bin| {
                        dose_of(&name(suffix), bin)
                            .ok_or_else(|| OpenMcAcceptanceError::MissingTally(name(suffix)))
                    })
                    .collect()
            };
            let boron = series("component.boron.response")?;
            let nitrogen = series("component.nitrogen.response")?;
            let hydrogen = series("component.hydrogen.response")?;
            let photon = series("component.photon.heating")?;
            let neutron_heat = series("audit.neutron_heating")?;
            let coupled = series("physical_total.coupled_heating")?;
            let b10 = series("audit.b10_mt107")?;
            let n14 = series("audit.n14_mt103")?;
            let max_of = |v: &[(f64, f64)]| v.iter().map(|(m, _)| *m).fold(0.0_f64, f64::max);
            let boron_threshold = gates.voxel_max_fraction * max_of(&boron);
            let nitrogen_threshold = gates.voxel_max_fraction * max_of(&nitrogen);
            let neutron_threshold = gates.voxel_max_fraction * max_of(&neutron_heat);
            let coupled_threshold = gates.voxel_max_fraction * max_of(&coupled);
            for bin in 0..bins {
                // Reaction-rate audits: rate x evaluated mean energy -> dose.
                for (label, rate, response, threshold, e_dep) in [
                    (
                        "b10_mt107",
                        b10[bin],
                        boron[bin],
                        boron_threshold,
                        acceptance.evaluated_mean_deposited_energy_ev.b10_mt107_ev,
                    ),
                    (
                        "n14_mt103",
                        n14[bin],
                        nitrogen[bin],
                        nitrogen_threshold,
                        acceptance.evaluated_mean_deposited_energy_ev.n14_mt103_ev,
                    ),
                ] {
                    let audit_dose = rate.0 * e_dep * EV_TO_JOULE / bin_mass_kg;
                    if response.0 >= threshold && response.0 > 0.0 && audit_dose > 0.0 {
                        let diff = (audit_dose - response.0).abs() / response.0;
                        let passed = diff <= gates.reaction_rate_agreement;
                        gates_passed &= passed;
                        estimator_comparisons.push(OpenMcEstimatorComparison {
                            region: roi.name.clone(),
                            bin: bin as u32,
                            comparison: format!("reaction_rate:{label}"),
                            observed_relative_difference: diff,
                            limit: gates.reaction_rate_agreement,
                            passed,
                        });
                    }
                }
                // Dedicated-estimator closure.
                for (label, dedicated, sum, threshold, limit) in [
                    (
                        "neutron_heating_vs_component_sum",
                        neutron_heat[bin].0,
                        boron[bin].0 + nitrogen[bin].0 + hydrogen[bin].0,
                        neutron_threshold,
                        gates.neutron_heating_agreement,
                    ),
                    (
                        "coupled_heating_vs_component_sum",
                        coupled[bin].0,
                        boron[bin].0 + nitrogen[bin].0 + hydrogen[bin].0 + photon[bin].0,
                        coupled_threshold,
                        gates.coupled_heating_agreement,
                    ),
                ] {
                    if dedicated >= threshold && dedicated > 0.0 {
                        let diff = (dedicated - sum).abs() / dedicated;
                        let passed = diff <= limit;
                        gates_passed &= passed;
                        estimator_comparisons.push(OpenMcEstimatorComparison {
                            region: roi.name.clone(),
                            bin: bin as u32,
                            comparison: label.to_string(),
                            observed_relative_difference: diff,
                            limit,
                            passed,
                        });
                    }
                }
            }
        }

        // Per-voxel precision gates on the scoring-mesh dose tallies.
        let voxel_gates = [
            "openbnct.component.boron.response",
            "openbnct.component.nitrogen.response",
            "openbnct.component.hydrogen.response",
            "openbnct.component.photon.heating",
        ];
        for name in voxel_gates {
            let contract = manifest
                .tallies
                .iter()
                .find(|c| {
                    openbnct_core::normalize_contract_id(&c.name)
                        == openbnct_core::normalize_contract_id(name)
                })
                .ok_or_else(|| OpenMcAcceptanceError::MissingTally(name.to_string()))?;
            let tally = run
                .statepoint
                .tally(&contract.name)
                .ok_or_else(|| OpenMcAcceptanceError::MissingTally(name.to_string()))?;
            let max = tally.mean.iter().copied().fold(f64::MIN, f64::max);
            if max <= 0.0 {
                continue;
            }
            let threshold = gates.voxel_max_fraction * max;
            let mut rel: Vec<f64> = tally
                .mean
                .iter()
                .zip(tally.standard_error.iter())
                .filter(|(m, _)| **m >= threshold)
                .map(|(m, s)| s / m)
                .collect();
            if rel.is_empty() {
                continue;
            }
            rel.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let median = rel[rel.len() / 2];
            let p95 = rel[((rel.len() as f64) * 0.95).ceil() as usize - 1];
            let passed = median <= gates.voxel_median_relative_uncertainty_max
                && p95 <= gates.voxel_p95_relative_uncertainty_max;
            gates_passed &= passed;
            voxel_precision.push(OpenMcVoxelPrecision {
                component: contract.component.unwrap_or(DoseComponent::Photon),
                voxels_evaluated: rel.len() as u32,
                median_relative_standard_error: median,
                p95_relative_standard_error: p95,
                passed,
            });
        }
    }

    // ---- Cross-seed chi-square on acceptance tally bins ----
    for tally_contract in &manifest.tallies {
        if tally_contract.scope != Some(OpenMcTallyScope::Acceptance) {
            continue;
        }
        let roi_name = tally_contract.roi.clone().unwrap_or_default();
        let bins = tally_contract.bins.unwrap_or(1) as usize;
        let roi = manifest
            .rois
            .iter()
            .find(|r| r.name == roi_name)
            .ok_or_else(|| OpenMcAcceptanceError::MissingTally(roi_name.clone()))?;
        let bin_volume_cm3 = roi.volume_cm3 / bins as f64;
        let bin_mass_kg = roi.mass_g * 1.0e-3 / bins as f64;
        for bin in 0..bins {
            let mut xs = Vec::new();
            let mut ss = Vec::new();
            for run in &runs {
                let tally = run.statepoint.tally(&tally_contract.name).ok_or_else(|| {
                    OpenMcAcceptanceError::MissingTally(tally_contract.name.clone())
                })?;
                let (m, s, _) = report_value(
                    tally_contract,
                    tally.mean[bin],
                    tally.standard_error[bin],
                    bin_volume_cm3,
                    bin_mass_kg,
                );
                xs.push(m);
                ss.push(s);
            }
            if !enough_seeds || xs.iter().all(|x| *x == 0.0) {
                continue;
            }
            let dof = xs.len() - 1;
            let w: f64 = ss.iter().map(|s| 1.0 / (s * s)).sum();
            if w <= 0.0 || !w.is_finite() {
                continue;
            }
            let mean_w: f64 = xs
                .iter()
                .zip(ss.iter())
                .map(|(x, s)| x / (s * s))
                .sum::<f64>()
                / w;
            let chi2: f64 = xs
                .iter()
                .zip(ss.iter())
                .map(|(x, s)| (x - mean_w) * (x - mean_w) / (s * s))
                .sum();
            let p = chi_square_sf(chi2, dof as f64);
            let passed = p >= gates.chi_square_p_min;
            gates_passed &= passed;
            chi_square.push(OpenMcChiSquare {
                tally: tally_contract.name.clone(),
                region: roi_name.clone(),
                bin: bin as u32,
                chi_square: chi2,
                degrees_of_freedom: dof as u32,
                p_value: p,
                passed,
            });
        }
    }

    let run_bindings = runs
        .iter()
        .map(|run| OpenMcRunBinding {
            seed: run.manifest.execution.seed,
            input_manifest_sha256: run.manifest_sha256.clone(),
            statepoint_sha256: run.statepoint_sha256.clone(),
            run_receipt_sha256: run.receipt_sha256.clone(),
            exit_code: run.exit_code,
        })
        .collect();
    Ok(OpenMcAcceptanceReport {
        schema_version: ACCEPTANCE_REPORT_SCHEMA.into(),
        case_id: manifest.case_id.clone(),
        acceptance_id: acceptance.id.clone(),
        acceptance_sha256,
        runs: run_bindings,
        region_results,
        voxel_precision,
        estimator_comparisons,
        chi_square,
        gates_passed: gates_passed && enough_seeds,
    })
}

/// Convert a raw tally mean/sigma into the reported quantity for the
/// acceptance report: region-mean dose (Gy per source neutron) for dose
/// tallies, mean fluence (cm^-2) for fluence, and raw rates for reaction
/// audits.
fn report_value(
    contract: &crate::input::OpenMcTallyContract,
    mean: f64,
    sigma: f64,
    bin_volume_cm3: f64,
    bin_mass_kg: f64,
) -> (f64, f64, String) {
    match contract.quantity {
        OpenMcTallyQuantity::ResponseWeightedTrackLength => (
            mean / bin_volume_cm3,
            sigma / bin_volume_cm3,
            "gray_per_source_neutron".to_string(),
        ),
        OpenMcTallyQuantity::Heating => (
            mean * EV_TO_JOULE / bin_mass_kg,
            sigma * EV_TO_JOULE / bin_mass_kg,
            "gray_per_source_neutron".to_string(),
        ),
        OpenMcTallyQuantity::EnergyBinnedTrackLength => (
            mean / bin_volume_cm3,
            sigma / bin_volume_cm3,
            "fluence_per_source_neutron_cm^-2".to_string(),
        ),
        _ => (
            mean,
            sigma,
            serde_json::to_value(contract.raw_unit)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "raw".to_string()),
        ),
    }
}

fn load_run(directory: &Path, exit_code: i32) -> Result<RunData, OpenMcAcceptanceError> {
    let read = |name: &str| -> Result<(Vec<u8>, String), OpenMcAcceptanceError> {
        let path = crate::statepoint::resolve_run_file(directory, name);
        let bytes = std::fs::read(&path)
            .map_err(|e| OpenMcAcceptanceError::Io(format!("{}: {e}", path.display())))?;
        let sha256 = format!("{:x}", Sha256::digest(&bytes));
        Ok((bytes, sha256))
    };
    let (manifest_bytes, manifest_sha256) = read("openbnct-input-manifest.json")?;
    let manifest: OpenMcInputManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| OpenMcAcceptanceError::Parse(e.to_string()))?;
    let (_, acceptance_sha256) = read(ACCEPTANCE_CONTRACT_FILE)?;
    let receipt_sha256 = read(OPENMC_RUN_RECEIPT_FILE).ok().map(|(_, s)| s);

    let statepoint_path = latest_statepoint(directory).map_err(OpenMcAcceptanceError::Collect)?;
    let statepoint_bytes =
        std::fs::read(&statepoint_path).map_err(|e| OpenMcAcceptanceError::Io(e.to_string()))?;
    let statepoint_sha256 = format!("{:x}", Sha256::digest(&statepoint_bytes));
    let statepoint =
        OpenMcStatepoint::open(&statepoint_path).map_err(OpenMcAcceptanceError::Collect)?;

    // Run-header consistency with the manifest.
    if statepoint.batches != manifest.execution.batches
        || statepoint.particles_per_batch != manifest.execution.particles_per_batch
        || statepoint.seed != manifest.execution.seed
        || statepoint.stride != manifest.execution.stride
        || statepoint.openmc_version != manifest.openmc_version
    {
        return Err(OpenMcAcceptanceError::RunHeaderMismatch);
    }
    // Every manifest tally contract must appear in the statepoint with its
    // declared id and bin count before any gate is evaluated.
    for contract in &manifest.tallies {
        let tally = statepoint
            .tally(&contract.name)
            .ok_or_else(|| OpenMcAcceptanceError::MissingTally(contract.name.clone()))?;
        if tally.id != contract.id {
            return Err(OpenMcAcceptanceError::TallyIdMismatch(
                contract.name.clone(),
            ));
        }
        if let Some(bins) = contract.bins
            && tally.mean.len() != bins as usize
        {
            return Err(OpenMcAcceptanceError::BinMismatch {
                tally: contract.name.clone(),
                expected: bins as usize,
                actual: tally.mean.len(),
            });
        }
    }
    Ok(RunData {
        manifest,
        manifest_sha256,
        statepoint,
        statepoint_sha256,
        receipt_sha256,
        acceptance_sha256,
        exit_code,
    })
}

/// Chi-square survival function Q(dof/2, chi2/2) via the regularized upper
/// incomplete gamma (series/continued-fraction, standard implementation).
fn chi_square_sf(chi2: f64, dof: f64) -> f64 {
    if chi2 <= 0.0 {
        return 1.0;
    }
    let a = dof / 2.0;
    let x = chi2 / 2.0;
    1.0 - gamma_p(a, x)
}

fn gamma_p(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x < a + 1.0 {
        // Series: P(a,x) = e^-x x^a / Γ(a) * Σ x^n/(a(a+1)...(a+n)).
        let mut ap = a;
        let mut term = 1.0 / a;
        let mut sum = term;
        for _ in 0..10_000 {
            ap += 1.0;
            term *= x / ap;
            sum += term;
            if term.abs() < sum.abs() * 1.0e-15 {
                break;
            }
        }
        sum * (-x + a * x.ln() - gamma_ln(a)).exp()
    } else {
        1.0 - gamma_q(a, x)
    }
}

fn gamma_q(a: f64, x: f64) -> f64 {
    // Continued-fraction representation of Q(a,x).
    const TINY: f64 = 1.0e-300;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / TINY;
    let mut d = 1.0 / b.max(TINY);
    let mut h = d;
    for i in 1..10_000u64 {
        let i_f = i as f64;
        let an = -i_f * (i_f - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < TINY {
            d = TINY;
        }
        c = b + an / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < 1.0e-15 {
            break;
        }
    }
    (-x + a * x.ln() - gamma_ln(a)).exp() * h
}

/// Lanczos log-gamma (9-term approximation, standard coefficients).
fn gamma_ln(z: f64) -> f64 {
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_1,
        9.984_369_578_019_57e-6,
        1.505_632_735_149_31e-7,
    ];
    if z < 0.5 {
        return (std::f64::consts::PI / (std::f64::consts::PI * z).sin()).ln() - gamma_ln(1.0 - z);
    }
    let z = z - 1.0;
    let mut x = C[0];
    for (i, c) in C.iter().enumerate().skip(1) {
        x += c / (z + i as f64);
    }
    let t = z + 7.5;
    0.5 * (2.0 * std::f64::consts::PI).ln() + (z + 0.5) * t.ln() - t + x.ln()
}

#[derive(Debug, Error)]
pub enum OpenMcAcceptanceError {
    #[error("each --run directory needs a matching --exit-code")]
    InvalidArguments,
    #[error("io error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("contract error: {0}")]
    Contract(String),
    #[error("run directories disagree on the bound acceptance contract")]
    AcceptanceMismatch,
    #[error("run directories disagree on case id")]
    CaseMismatch,
    #[error("manifest does not bind an acceptance contract")]
    NoAcceptanceBinding,
    #[error("manifest purpose is not candidate_reference")]
    NotCandidateReference,
    #[error("seed {0} is not in the acceptance seed set")]
    UnregisteredSeed(u64),
    #[error("seed {0} is evaluated more than once")]
    DuplicateSeed(u64),
    #[error("acceptance tally {0} absent from a statepoint")]
    MissingTally(String),
    #[error("contract tally {0} has a different id in the statepoint")]
    TallyIdMismatch(String),
    #[error("tally {tally} has {actual} bins, expected {expected}")]
    BinMismatch {
        tally: String,
        expected: usize,
        actual: usize,
    },
    #[error("statepoint run header does not match the input manifest")]
    RunHeaderMismatch,
    #[error(transparent)]
    Collect(#[from] OpenMcCollectError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OPENMC_INPUT_MANIFEST_FILE;
    use crate::input::CANDIDATE_REFERENCE_SEEDS;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;

    const REALIZATIONS: i64 = 50;
    const EV: f64 = 1.602176634e-19;
    const E_B10: f64 = 2_341_900.4411541675;
    const E_N14: f64 = 625_976.8493398946;
    const REL_SIGMA: f64 = 0.005;

    struct TallySpec {
        id: u32,
        name: String,
        bins: usize,
        means: Vec<f64>,
        rel_sigma: f64,
    }

    /// Physically consistent per-bin quantities: dose components in gray per
    /// source neutron, rates in reactions, heating in eV. `doses` are
    /// (boron, nitrogen, hydrogen, photon) per bin.
    fn region_tallies(
        roi: &str,
        id_base: u32,
        bins: usize,
        bin_volume_cm3: f64,
        bin_mass_kg: f64,
        doses: &[[f64; 4]],
    ) -> Vec<TallySpec> {
        let mut out = Vec::new();
        let mut push = |slot: u32, suffix: &str, values: Vec<f64>| {
            out.push(TallySpec {
                id: id_base + slot,
                name: format!("openbnct.roi.{roi}.{suffix}"),
                bins,
                rel_sigma: REL_SIGMA,
                means: values,
            });
        };
        let sum_n: Vec<f64> = doses.iter().map(|d| d[0] + d[1] + d[2]).collect();
        let sum_all: Vec<f64> = doses.iter().map(|d| d.iter().sum()).collect();
        push(
            0,
            "component.boron.response",
            doses.iter().map(|d| d[0] * bin_volume_cm3).collect(),
        );
        push(
            1,
            "component.nitrogen.response",
            doses.iter().map(|d| d[1] * bin_volume_cm3).collect(),
        );
        push(
            2,
            "component.hydrogen.response",
            doses.iter().map(|d| d[2] * bin_volume_cm3).collect(),
        );
        push(
            3,
            "component.photon.heating",
            doses.iter().map(|d| d[3] * bin_mass_kg / EV).collect(),
        );
        push(
            4,
            "audit.neutron_heating",
            sum_n.iter().map(|d| d * bin_mass_kg / EV).collect(),
        );
        push(
            5,
            "physical_total.coupled_heating",
            sum_all.iter().map(|d| d * bin_mass_kg / EV).collect(),
        );
        push(
            6,
            "audit.b10_mt107",
            doses
                .iter()
                .map(|d| d[0] * bin_mass_kg / (E_B10 * EV))
                .collect(),
        );
        push(
            7,
            "audit.n14_mt103",
            doses
                .iter()
                .map(|d| d[1] * bin_mass_kg / (E_N14 * EV))
                .collect(),
        );
        push(
            8,
            "diagnostic.neutron_fluence",
            doses
                .iter()
                .map(|d| (d[0] + d[2]) * 1.0e20 * bin_volume_cm3)
                .collect(),
        );
        out
    }

    fn scoring_tallies() -> Vec<TallySpec> {
        // Two scoring voxels, each at 1e-12 Gy.cm^3 (response) or the eV
        // equivalent (heating), all well inside the voxel precision gates.
        let mk = |id, name: &str, mean: f64| TallySpec {
            id,
            name: name.to_string(),
            bins: 2,
            means: vec![mean, mean],
            rel_sigma: REL_SIGMA,
        };
        vec![
            mk(1, "openbnct.component.boron.response", 1.0e-12),
            mk(2, "openbnct.component.nitrogen.response", 2.0e-13),
            mk(3, "openbnct.component.hydrogen.response", 5.0e-14),
            mk(4, "openbnct.component.photon.heating", 2.0e3),
            mk(5, "openbnct.physical_total.coupled_heating", 9.8e3),
        ]
    }

    fn contract_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version": "openbnct.acceptance-contract/0.1.0",
            "id": "test.acceptance.v1",
            "case_id": "synthetic-case",
            "regions": [
                {"name": "core", "bounds_cm": {"x_cm": [-1.0, 1.0], "y_cm": [-1.0, 1.0], "z_cm": [-1.0, 1.0]}, "dimensions": [1, 1, 1], "precision_gated": true},
                {"name": "axis", "bounds_cm": {"x_cm": [-0.5, 0.5], "y_cm": [-0.5, 0.5], "z_cm": [-1.0, 1.0]}, "dimensions": [1, 1, 2], "precision_gated": false}
            ],
            "evaluated_mean_deposited_energy_ev": {
                "b10_mt107_ev": E_B10,
                "n14_mt103_ev": E_N14,
                "evidence_sha256": "a".repeat(64)
            },
            "gates": {
                "roi_relative_standard_uncertainty_max": 0.01,
                "voxel_max_fraction": 0.2,
                "voxel_median_relative_uncertainty_max": 0.03,
                "voxel_p95_relative_uncertainty_max": 0.05,
                "reaction_rate_agreement": 0.02,
                "neutron_heating_agreement": 0.03,
                "coupled_heating_agreement": 0.03,
                "chi_square_p_min": 0.001
            },
            "seeds": CANDIDATE_REFERENCE_SEEDS,
            "min_batches": 50
        })
    }

    fn tally_contract_json(spec: &TallySpec, roi: Option<&str>) -> serde_json::Value {
        let (component, particle, quantity, raw_unit, normalization) =
            if spec.name.contains("response") {
                let component = if spec.name.contains("boron") {
                    "boron"
                } else if spec.name.contains("nitrogen") {
                    "nitrogen"
                } else {
                    "hydrogen"
                };
                (
                    serde_json::json!(component),
                    serde_json::json!("neutron"),
                    "response_weighted_track_length",
                    "gray_cubic_centimeter_per_source_neutron",
                    "divide_by_voxel_volume_cm3",
                )
            } else if spec.name.contains("photon.heating") {
                (
                    serde_json::json!("photon"),
                    serde_json::json!("photon"),
                    "heating",
                    "electron_volt_per_source_neutron",
                    "electron_volt_to_joule_divide_by_voxel_mass_kg",
                )
            } else if spec.name.contains("neutron_heating") {
                (
                    serde_json::Value::Null,
                    serde_json::json!("neutron"),
                    "heating",
                    "electron_volt_per_source_neutron",
                    "electron_volt_to_joule_divide_by_voxel_mass_kg",
                )
            } else if spec.name.contains("coupled_heating") {
                (
                    serde_json::Value::Null,
                    serde_json::Value::Null,
                    "heating",
                    "electron_volt_per_source_neutron",
                    "electron_volt_to_joule_divide_by_voxel_mass_kg",
                )
            } else if spec.name.contains("mt10") {
                (
                    serde_json::Value::Null,
                    serde_json::json!("neutron"),
                    "reaction_rate",
                    "reactions_per_source_neutron",
                    "none",
                )
            } else {
                (
                    serde_json::Value::Null,
                    serde_json::json!("neutron"),
                    "energy_binned_track_length",
                    "centimeter_per_source_neutron",
                    "none",
                )
            };
        let mut entry = serde_json::json!({
            "id": spec.id,
            "name": spec.name,
            "component": component,
            "particle": particle,
            "quantity": quantity,
            "raw_unit": raw_unit,
            "collection_normalization": normalization,
            "bins": spec.bins,
        });
        if let Some(roi) = roi {
            entry["scope"] = serde_json::json!("acceptance");
            entry["roi"] = serde_json::json!(roi);
        } else {
            entry["scope"] = serde_json::json!("dose");
        }
        entry
    }

    fn write_statepoint(path: &Path, tallies: &[TallySpec], seed: u64) {
        let file = hdf5_pure::File::create(path).expect("create statepoint fixture");
        let root = file.root();
        root.set_attr(
            "filetype",
            hdf5_pure::AttrValue::AsciiString("statepoint".into()),
        )
        .unwrap();
        root.set_attr(
            "openmc_version",
            hdf5_pure::AttrValue::I32Array(vec![0, 16, 0]),
        )
        .unwrap();
        for (name, value) in [
            ("n_batches", REALIZATIONS),
            ("n_particles", 20_000),
            ("n_realizations", REALIZATIONS),
            ("seed", seed as i64),
            ("stride", 152_917),
        ] {
            root.create_dataset(name, |d| {
                d.with_i64_data(&[value]);
            })
            .unwrap();
        }
        root.create_dataset("energy_mode", |d| {
            d.with_vlen_strings(&["continuous-energy"]);
        })
        .unwrap();
        root.create_dataset("run_mode", |d| {
            d.with_vlen_strings(&["fixed source"]);
        })
        .unwrap();
        let tallies_group = root.create_group("tallies").unwrap();
        let filters_group = tallies_group.create_group("filters").unwrap();
        let mesh_filter = filters_group.create_group("filter 1").unwrap();
        mesh_filter
            .create_dataset("type", |d| {
                d.with_vlen_strings(&["mesh"]);
            })
            .unwrap();
        mesh_filter
            .create_dataset("bins", |d| {
                d.with_i64_data(&[0, 1, 2, 3]);
            })
            .unwrap();
        let n = REALIZATIONS as f64;
        for spec in tallies {
            let group = tallies_group
                .create_group(&format!("tally {}", spec.id))
                .unwrap();
            group
                .create_dataset("name", |d| {
                    d.with_vlen_strings(&[spec.name.as_str()]);
                })
                .unwrap();
            group
                .create_dataset("estimator", |d| {
                    d.with_vlen_strings(&["tracklength"]);
                })
                .unwrap();
            group
                .create_dataset("filters", |d| {
                    d.with_i64_data(&[1]);
                })
                .unwrap();
            group
                .create_dataset("n_filters", |d| {
                    d.with_i64_data(&[1]);
                })
                .unwrap();
            group
                .create_dataset("n_score_bins", |d| {
                    d.with_i64_data(&[1]);
                })
                .unwrap();
            group
                .create_dataset("n_realizations", |d| {
                    d.with_i64_data(&[REALIZATIONS]);
                })
                .unwrap();
            let mut results = Vec::with_capacity(spec.bins * 2);
            for &mean in &spec.means {
                let std = mean.abs() * spec.rel_sigma;
                results.push(mean * n);
                results.push(n * (mean * mean + (n - 1.0) * std * std));
            }
            group
                .create_dataset("results", |d| {
                    d.with_f64_data(&results)
                        .with_shape(&[spec.bins as u64, 1, 2]);
                })
                .unwrap();
        }
        file.commit().unwrap();
    }

    fn write_run(directory: &Path, seed: u64, perturb_boron: f64) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        let contract = contract_json();
        let contract_bytes = serde_json::to_vec_pretty(&contract).unwrap();
        std::fs::write(directory.join(ACCEPTANCE_CONTRACT_FILE), &contract_bytes).unwrap();
        let contract_sha256 = format!("{:x}", Sha256::digest(&contract_bytes));

        // Region bins: core [1,1,1] (8 cm^3, 8 g), axis [1,1,2] (2 bins of
        // 0.5 cm^3 / 0.5 g each).
        let mut tallies = scoring_tallies();
        let mut core = region_tallies(
            "core",
            21,
            1,
            8.0,
            8.0e-3,
            &[[1.0e-12 * perturb_boron, 2.0e-13, 5.0e-14, 3.2e-13]],
        );
        let mut axis = region_tallies(
            "axis",
            30,
            2,
            0.5,
            5.0e-4,
            &[
                [2.0e-12, 4.0e-13, 1.0e-13, 6.4e-13],
                [1.0e-12, 2.0e-13, 5.0e-14, 3.2e-13],
            ],
        );
        tallies.append(&mut core);
        tallies.append(&mut axis);

        let tally_entries: Vec<serde_json::Value> = tallies
            .iter()
            .map(|spec| {
                let name = openbnct_core::normalize_contract_id(&spec.name);
                let roi = if name.contains("openbnct.roi.core") {
                    Some("core")
                } else if name.contains("openbnct.roi.axis") {
                    Some("axis")
                } else {
                    None
                };
                tally_contract_json(spec, roi)
            })
            .collect();
        let manifest = serde_json::json!({
            "schema_version": "openbnct.openmc-input-manifest/0.2.0",
            "case_id": "synthetic-case",
            "backend_id": "openmc",
            "openmc_version": "0.16.0",
            "openmc_source_commit": "617d35a5063c57796b43428bc401e627d2011046",
            "bindings": {
                "component_profile": {"id": "profile", "sha256": "a".repeat(64)},
                "material": {"id": "material", "sha256": "b".repeat(64)},
                "source": {"id": "source", "sha256": "c".repeat(64)},
                "response_set": {"id": "response-set", "sha256": "d".repeat(64)},
                "nuclear_data_manifest": {"id": "ndm", "sha256": "e".repeat(64)},
                "response_generation_method": {"id": "method", "sha256": "f".repeat(64)},
                "independent_response_review": {"id": "review", "sha256": "0".repeat(64)},
                "execution_profile": {"id": "profile", "sha256": "1".repeat(64)},
                "acceptance": {"id": "test.acceptance.v1", "sha256": contract_sha256},
            },
            "execution": {
                "purpose": "candidate_reference",
                "requested_histories": 1_000_000,
                "batches": REALIZATIONS,
                "particles_per_batch": 20_000,
                "seed": seed,
                "stride": 152917,
            },
            "scoring_mesh": {
                "mesh_id": 1,
                "dimensions": [2, 1, 1],
                "lower_left_cm": [-1.0, -0.5, -0.5],
                "upper_right_cm": [1.0, 0.5, 0.5],
                "voxel_volume_cm3": 1.0,
                "voxel_mass_g": 1.0,
                "cell_volume_cm3": 2.0,
                "cell_mass_g": 2.0,
            },
            "rois": [
                {"name": "core", "mesh_id": 2, "mesh_filter_id": 10, "dimensions": [1, 1, 1],
                 "lower_left_cm": [-1.0, -1.0, -1.0], "upper_right_cm": [1.0, 1.0, 1.0],
                 "volume_cm3": 8.0, "mass_g": 8.0},
                {"name": "axis", "mesh_id": 3, "mesh_filter_id": 11, "dimensions": [1, 1, 2],
                 "lower_left_cm": [-0.5, -0.5, -1.0], "upper_right_cm": [0.5, 0.5, 1.0],
                 "volume_cm3": 1.0, "mass_g": 1.0}
            ],
            "tallies": tally_entries,
            "xml_artifacts": [],
        });
        std::fs::write(
            directory.join(OPENMC_INPUT_MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        write_statepoint(
            &directory.join(format!("statepoint.{REALIZATIONS}.h5")),
            &tallies,
            seed,
        );
        directory.to_path_buf()
    }

    /// A run directory written before the rename keeps `nctforge-*`
    /// artifact filenames, `nctforge.*` schema ids, and `nctforge.*`
    /// tally names in both manifest and statepoint. Evaluation must
    /// still pass: file resolution falls back to the legacy names,
    /// schema ids normalize on load, and tally lookups match by
    /// normalized contract id.
    #[test]
    fn evaluates_pre_rename_run_directory() {
        fn legacy_ids(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::String(s) => {
                    *s = s
                        .replace("openbnct.", "nctforge.")
                        .replace("openbnct-", "nctforge-");
                }
                serde_json::Value::Array(items) => items.iter_mut().for_each(legacy_ids),
                serde_json::Value::Object(map) => map.values_mut().for_each(legacy_ids),
                _ => {}
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let dirs: Vec<PathBuf> = CANDIDATE_REFERENCE_SEEDS
            .iter()
            .map(|seed| {
                let dir = write_run(&temp.path().join(format!("run-{seed}")), *seed, 1.0);

                // Legacy filenames + legacy identifiers in the JSON artifacts.
                // The contract is rewritten first so the manifest's bound
                // sha256 can be updated to the legacy bytes — a real
                // pre-rename run bound the hash of its own era's contract.
                let mut contract: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(dir.join(ACCEPTANCE_CONTRACT_FILE)).unwrap(),
                )
                .unwrap();
                legacy_ids(&mut contract);
                let contract_bytes = serde_json::to_vec_pretty(&contract).unwrap();
                std::fs::write(
                    dir.join("nctforge-acceptance-contract.json"),
                    &contract_bytes,
                )
                .unwrap();
                std::fs::remove_file(dir.join(ACCEPTANCE_CONTRACT_FILE)).unwrap();
                let contract_sha256 = format!("{:x}", Sha256::digest(&contract_bytes));

                let mut manifest: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(dir.join(OPENMC_INPUT_MANIFEST_FILE)).unwrap(),
                )
                .unwrap();
                legacy_ids(&mut manifest);
                manifest["bindings"]["acceptance"]["sha256"] =
                    serde_json::Value::String(contract_sha256);
                std::fs::write(
                    dir.join("nctforge-input-manifest.json"),
                    serde_json::to_vec_pretty(&manifest).unwrap(),
                )
                .unwrap();
                std::fs::remove_file(dir.join(OPENMC_INPUT_MANIFEST_FILE)).unwrap();

                // The statepoint tally names must carry the legacy namespace
                // too — a real pre-rename run wrote them that way.
                let mut tallies = scoring_tallies();
                let mut core = region_tallies(
                    "core",
                    21,
                    1,
                    8.0,
                    8.0e-3,
                    &[[1.0e-12, 2.0e-13, 5.0e-14, 3.2e-13]],
                );
                let mut axis = region_tallies(
                    "axis",
                    30,
                    2,
                    0.5,
                    5.0e-4,
                    &[
                        [2.0e-12, 4.0e-13, 1.0e-13, 6.4e-13],
                        [1.0e-12, 2.0e-13, 5.0e-14, 3.2e-13],
                    ],
                );
                tallies.append(&mut core);
                tallies.append(&mut axis);
                for spec in &mut tallies {
                    spec.name = spec.name.replace("openbnct.", "nctforge.");
                }
                write_statepoint(
                    &dir.join(format!("statepoint.{REALIZATIONS}.h5")),
                    &tallies,
                    *seed,
                );
                dir
            })
            .collect();

        let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        let report = evaluate_runs(&refs, &[0, 0, 0]).unwrap();
        assert!(report.gates_passed);
        assert!(report.estimator_comparisons.iter().all(|c| c.passed));
        assert!(report.voxel_precision.iter().all(|v| v.passed));
        assert!(report.chi_square.iter().all(|c| c.passed));
    }

    #[test]
    fn evaluates_and_passes_consistent_runs() {
        let temp = tempfile::tempdir().unwrap();
        let dirs: Vec<PathBuf> = CANDIDATE_REFERENCE_SEEDS
            .iter()
            .map(|seed| write_run(&temp.path().join(format!("run-{seed}")), *seed, 1.0))
            .collect();
        let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        let report = evaluate_runs(&refs, &[0, 0, 0]).unwrap();
        assert_eq!(report.case_id, "synthetic-case");
        assert_eq!(report.runs.len(), 3);
        assert!(
            report
                .estimator_comparisons
                .iter()
                .all(|comparison| comparison.passed)
        );
        assert!(report.voxel_precision.iter().all(|v| v.passed));
        assert!(report.chi_square.iter().all(|c| c.passed));
        assert!(report.gates_passed);
        // Nine acceptance tallies x (1 + 2) bins = 27 chi-square entries.
        assert_eq!(report.chi_square.len(), 27);
    }

    #[test]
    fn rejects_unregistered_and_reused_seeds() {
        let temp = tempfile::tempdir().unwrap();
        let bad = write_run(&temp.path().join("bad"), 999_999, 1.0);
        assert!(matches!(
            evaluate_runs(&[bad.as_path()], &[0]),
            Err(OpenMcAcceptanceError::UnregisteredSeed(999_999))
        ));
        let a = write_run(&temp.path().join("a"), CANDIDATE_REFERENCE_SEEDS[0], 1.0);
        let b = write_run(&temp.path().join("b"), CANDIDATE_REFERENCE_SEEDS[0], 1.0);
        assert!(matches!(
            evaluate_runs(&[a.as_path(), b.as_path()], &[0, 0]),
            Err(OpenMcAcceptanceError::DuplicateSeed(_))
        ));
    }

    #[test]
    fn fails_when_seed_results_disagree() {
        let temp = tempfile::tempdir().unwrap();
        let mut dirs: Vec<PathBuf> = CANDIDATE_REFERENCE_SEEDS[..2]
            .iter()
            .map(|seed| write_run(&temp.path().join(format!("run-{seed}")), *seed, 1.0))
            .collect();
        // Third seed's core boron ROI mean is 20% high — a 40-sigma excursion.
        dirs.push(write_run(
            &temp.path().join("run-outlier"),
            CANDIDATE_REFERENCE_SEEDS[2],
            1.2,
        ));
        let refs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        let report = evaluate_runs(&refs, &[0, 0, 0]).unwrap();
        let boron = report
            .chi_square
            .iter()
            .find(|c| c.tally == "openbnct.roi.core.component.boron.response")
            .unwrap();
        assert!(!boron.passed);
        assert!(!report.gates_passed);
    }

    #[test]
    fn requires_candidate_reference_manifest_binding() {
        let temp = tempfile::tempdir().unwrap();
        let dir = write_run(&temp.path().join("run"), CANDIDATE_REFERENCE_SEEDS[0], 1.0);
        let manifest_path = dir.join(OPENMC_INPUT_MANIFEST_FILE);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        manifest["bindings"]
            .as_object_mut()
            .unwrap()
            .remove("acceptance");
        std::fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            evaluate_runs(&[dir.as_path()], &[0]),
            Err(OpenMcAcceptanceError::NoAcceptanceBinding)
        ));
    }
}
