// SPDX-License-Identifier: MIT

//! Gamma-index evaluation (`openbnct.gamma-evaluation/0.1.0`).
//!
//! Implements the Low et al. (1998) gamma metric used throughout
//! radiotherapy dose-comparison practice: for each evaluated reference
//! voxel `r`, gamma is the minimum over candidate positions `r'` of
//!
//! ```text
//! Γ(r, r') = sqrt( |r − r'|² / dta² + (D_c(r') − D_r(r))² / Δd² )
//! ```
//!
//! where `dta` is the distance-to-agreement and `Δd` the dose-difference
//! criterion — a global percent of the reference maximum or a local percent
//! of the reference voxel's own value. A voxel passes when γ ≤ 1.
//!
//! Pass/fail is computed exactly: only candidate voxels within `dta` can
//! produce Γ ≤ 1 (the distance term alone exceeds the criterion beyond it),
//! so the search neighborhood is the dta-radius ball. Reported γ values
//! above 1.0 are minima over that ball — an upper bound on the
//! unconstrained minimum, which can only make failing voxels look worse,
//! never better.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, PhysicalDoseBundle, grid_geometry_equivalent};

use crate::ManifestError;

pub const GAMMA_EVALUATION_SCHEMA: &str = "openbnct.gamma-evaluation/0.1.0";

use crate::compare::ComparisonInput;

/// How the dose-difference criterion is scaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GammaNormalization {
    /// `Δd` is a percent of the reference distribution's maximum value —
    /// the usual convention for dose maps with heterogeneous dynamic range.
    Global,
    /// `Δd` is a percent of the reference value at the evaluated voxel —
    /// stricter in low-dose regions.
    Local,
}

/// The gamma criteria and exclusions applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GammaCriteria {
    /// Dose-difference criterion in percent.
    pub dose_difference_percent: f64,
    /// Distance-to-agreement in millimetres.
    pub distance_to_agreement_mm: f64,
    pub normalization: GammaNormalization,
    /// Reference voxels below this percent of the reference maximum are
    /// excluded from evaluation (the standard low-dose cutoff; e.g. 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dose_threshold_percent: Option<f64>,
}

/// Gamma statistics for one dose quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GammaQuantityResult {
    /// `component:boron`, … or `physical_total`.
    pub quantity: String,
    pub unit: String,
    /// Voxels evaluated (met the dose threshold).
    pub voxels_evaluated: u64,
    /// Voxels excluded by the dose threshold.
    pub voxels_excluded: u64,
    /// Fraction of evaluated voxels with γ ≤ 1.
    pub pass_rate: f64,
    /// Mean γ over evaluated voxels with a finite γ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean_gamma: Option<f64>,
    /// 95th-percentile γ over evaluated voxels with a finite γ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95_gamma: Option<f64>,
    /// Largest finite γ over evaluated voxels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_gamma: Option<f64>,
    /// Per-voxel γ aligned to the grid's linear index — present only when
    /// the caller requests the volume. `null` marks voxels excluded by the
    /// dose threshold. Values above 1.0 are the pass-radius-restricted
    /// minimum (upper bound; see module docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma_volume: Option<Vec<Option<f64>>>,
}

/// The evaluation record itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GammaEvaluation {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub inputs: Vec<ComparisonInput>,
    pub criteria: GammaCriteria,
    /// Radius actually searched — equals `distance_to_agreement_mm`.
    pub search_radius_mm: f64,
    pub results: Vec<GammaQuantityResult>,
    /// Research-status qualification; no equivalence or clinical claim.
    pub qualification: String,
}

/// Evaluate the gamma index between two physical dose bundles.
///
/// Both must share `case_id`, an equivalent grid, the same component set
/// and unit — the same frozen-case guards as `compare_dose_bundles`. When
/// `emit_gamma_volume` is set, each result carries the per-voxel γ field
/// for overlay and inspection.
pub fn evaluate_gamma(
    reference: &PhysicalDoseBundle,
    candidate: &PhysicalDoseBundle,
    reference_ref: ContentReference,
    candidate_ref: ContentReference,
    criteria: GammaCriteria,
    emit_gamma_volume: bool,
) -> Result<GammaEvaluation, ManifestError> {
    let invalid = |msg: String| ManifestError::Invalid(format!("gamma evaluation: {msg}"));
    criteria.validate()?;
    if reference.case_id != candidate.case_id {
        return Err(invalid(format!(
            "case_id mismatch: {:?} vs {:?} — only the same frozen case can be compared",
            reference.case_id, candidate.case_id
        )));
    }
    if !grid_geometry_equivalent(&reference.geometry, &candidate.geometry) {
        return Err(invalid(
            "grids differ — resample externally before comparing".into(),
        ));
    }
    reference
        .geometry
        .voxel_count()
        .map_err(|e| invalid(format!("geometry: {e}")))?;
    if reference.components.len() != candidate.components.len() {
        return Err(invalid(format!(
            "component sets differ: {} vs {} components",
            reference.components.len(),
            candidate.components.len()
        )));
    }

    // World positions are quantity-independent: compute once.
    let centers = voxel_centers(&reference.geometry)?;

    let mut results = Vec::new();
    for reference_component in &reference.components {
        let candidate_component = candidate
            .components
            .iter()
            .find(|c| c.component == reference_component.component)
            .ok_or_else(|| {
                invalid(format!(
                    "candidate is missing component {:?}",
                    reference_component.component
                ))
            })?;
        if reference_component.unit != candidate_component.unit {
            return Err(invalid(format!(
                "component {:?} unit mismatch: {:?} vs {:?}",
                reference_component.component, reference_component.unit, candidate_component.unit
            )));
        }
        let name = serde_json::to_value(reference_component.component)
            .and_then(serde_json::from_value::<String>)
            .map_err(|e| invalid(format!("component name: {e}")))?;
        let unit = serde_json::to_value(reference_component.unit)
            .and_then(serde_json::from_value::<String>)
            .map_err(|e| invalid(format!("unit: {e}")))?;
        results.push(gamma_quantity(
            &format!("component:{name}"),
            &unit,
            reference,
            &reference_component.values,
            &candidate_component.values,
            &centers,
            &criteria,
            emit_gamma_volume,
        )?);
    }
    if reference.physical_total.unit != candidate.physical_total.unit {
        return Err(invalid(format!(
            "total unit mismatch: {:?} vs {:?}",
            reference.physical_total.unit, candidate.physical_total.unit
        )));
    }
    let total_unit = serde_json::to_value(reference.physical_total.unit)
        .and_then(serde_json::from_value::<String>)
        .map_err(|e| invalid(format!("unit: {e}")))?;
    results.push(gamma_quantity(
        "physical_total",
        &total_unit,
        reference,
        &reference.physical_total.values,
        &candidate.physical_total.values,
        &centers,
        &criteria,
        emit_gamma_volume,
    )?);

    let evaluation = GammaEvaluation {
        schema_version: GAMMA_EVALUATION_SCHEMA.into(),
        case_id: reference.case_id.clone(),
        inputs: vec![
            ComparisonInput {
                role: "reference".into(),
                content: reference_ref,
                provenance_id: reference.provenance_id.clone(),
            },
            ComparisonInput {
                role: "candidate".into(),
                content: candidate_ref,
                provenance_id: candidate.provenance_id.clone(),
            },
        ],
        search_radius_mm: criteria.distance_to_agreement_mm,
        criteria,
        results,
        qualification: "research gamma-index comparison; reports measured agreement — \
                        no equivalence, clinical, or commissioning claim"
            .into(),
    };
    evaluation.validate()?;
    Ok(evaluation)
}

impl GammaCriteria {
    pub fn validate(&self) -> Result<(), ManifestError> {
        let invalid = |msg: String| ManifestError::Invalid(format!("gamma criteria: {msg}"));
        if !(self.dose_difference_percent.is_finite() && self.dose_difference_percent > 0.0) {
            return Err(invalid(format!(
                "dose difference {} must be positive",
                self.dose_difference_percent
            )));
        }
        if !(self.distance_to_agreement_mm.is_finite() && self.distance_to_agreement_mm > 0.0) {
            return Err(invalid(format!(
                "distance to agreement {} mm must be positive",
                self.distance_to_agreement_mm
            )));
        }
        if let Some(threshold) = self.dose_threshold_percent
            && !(threshold.is_finite() && (0.0..100.0).contains(&threshold))
        {
            return Err(invalid(format!(
                "dose threshold {threshold} must be in [0, 100)"
            )));
        }
        Ok(())
    }
}

/// World-space centre of every voxel in grid order (x-fastest), in mm.
fn voxel_centers(geometry: &openbnct_core::GridGeometry) -> Result<Vec<[f64; 3]>, ManifestError> {
    let invalid = |msg: String| ManifestError::Invalid(format!("gamma evaluation: {msg}"));
    let [nx, ny, nz] = geometry.shape.map(|d| d as usize);
    let mut centers = Vec::with_capacity(nx * ny * nz);
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                centers.push(
                    geometry
                        .voxel_center_lps_mm([i as u32, j as u32, k as u32])
                        .map_err(|e| invalid(format!("voxel center: {e}")))?,
                );
            }
        }
    }
    Ok(centers)
}

#[allow(clippy::too_many_arguments)]
fn gamma_quantity(
    quantity: &str,
    unit: &str,
    reference: &PhysicalDoseBundle,
    reference_values: &[f64],
    candidate_values: &[f64],
    centers: &[[f64; 3]],
    criteria: &GammaCriteria,
    emit_gamma_volume: bool,
) -> Result<GammaQuantityResult, ManifestError> {
    let invalid = |msg: String| ManifestError::Invalid(format!("gamma evaluation: {msg}"));
    if reference_values.len() != candidate_values.len() || reference_values.len() != centers.len() {
        return Err(invalid(format!(
            "{quantity} length mismatch: {} reference / {} candidate / {} centers",
            reference_values.len(),
            candidate_values.len(),
            centers.len()
        )));
    }
    if reference_values
        .iter()
        .chain(candidate_values.iter())
        .any(|v| !v.is_finite())
    {
        return Err(invalid(format!("{quantity} contains non-finite values")));
    }

    let geometry = &reference.geometry;
    let [nx, ny, nz] = geometry.shape.map(|d| d as usize);
    let reference_max = reference_values.iter().copied().fold(0.0, f64::max);
    let dta = criteria.distance_to_agreement_mm;
    let threshold = criteria
        .dose_threshold_percent
        .map(|p| p / 100.0 * reference_max);

    // Integer ball radius per axis: only voxels within dta can pass.
    let ball: [i64; 3] = [
        (dta / geometry.spacing_mm[0]).ceil() as i64,
        (dta / geometry.spacing_mm[1]).ceil() as i64,
        (dta / geometry.spacing_mm[2]).ceil() as i64,
    ];

    let mut evaluated = 0_u64;
    let mut excluded = 0_u64;
    let mut passing = 0_u64;
    let mut sum = 0.0_f64;
    let mut max_gamma = 0.0_f64;
    let mut gammas = Vec::new();
    let mut volume = emit_gamma_volume.then(|| vec![None; reference_values.len()]);

    for linear in 0..reference_values.len() {
        let reference_dose = reference_values[linear];
        if let Some(cutoff) = threshold
            && reference_dose < cutoff
        {
            excluded += 1;
            continue;
        }
        let i = (linear % nx) as i64;
        let j = ((linear / nx) % ny) as i64;
        let k = (linear / (nx * ny)) as i64;
        let dd_limit = match criteria.normalization {
            GammaNormalization::Global => criteria.dose_difference_percent / 100.0 * reference_max,
            GammaNormalization::Local => criteria.dose_difference_percent / 100.0 * reference_dose,
        };
        let mut best = f64::INFINITY;
        if dd_limit > 0.0 {
            for dk in -ball[2]..=ball[2] {
                let ck = k + dk;
                if ck < 0 || ck >= nz as i64 {
                    continue;
                }
                for dj in -ball[1]..=ball[1] {
                    let cj = j + dj;
                    if cj < 0 || cj >= ny as i64 {
                        continue;
                    }
                    for di in -ball[0]..=ball[0] {
                        let ci = i + di;
                        if ci < 0 || ci >= nx as i64 {
                            continue;
                        }
                        let c_linear = (ci + nx as i64 * cj + nx as i64 * ny as i64 * ck) as usize;
                        let center_r = &centers[linear];
                        let center_c = &centers[c_linear];
                        let dr = ((center_r[0] - center_c[0]).powi(2)
                            + (center_r[1] - center_c[1]).powi(2)
                            + (center_r[2] - center_c[2]).powi(2))
                        .sqrt();
                        if dr > dta {
                            continue;
                        }
                        let dd = candidate_values[c_linear] - reference_dose;
                        let gamma = ((dr / dta).powi(2) + (dd / dd_limit).powi(2)).sqrt();
                        if gamma < best {
                            best = gamma;
                        }
                    }
                }
            }
        }
        // No candidate within dta → distance term alone exceeds 1 → fails.
        // Infinite γ is excluded from the summary statistics (which are
        // therefore over finite values only) and serializes as null in
        // the optional volume.
        evaluated += 1;
        if best <= 1.0 + 1e-12 {
            passing += 1;
        }
        if best.is_finite() {
            sum += best;
            if best > max_gamma {
                max_gamma = best;
            }
            gammas.push(best);
        }
        if let Some(vol) = &mut volume {
            vol[linear] = if best.is_finite() { Some(best) } else { None };
        }
    }

    let mut sorted = gammas;
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Statistics cover finite γ only; absent any, the fields stay absent.
    let finite = |v: f64| v.is_finite().then_some(v);
    let p95_gamma = if sorted.is_empty() {
        None
    } else {
        let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
        sorted
            .get(idx.saturating_sub(1).min(sorted.len() - 1))
            .copied()
    };
    Ok(GammaQuantityResult {
        quantity: quantity.into(),
        unit: unit.into(),
        voxels_evaluated: evaluated,
        voxels_excluded: excluded,
        pass_rate: if evaluated > 0 {
            passing as f64 / evaluated as f64
        } else {
            0.0
        },
        mean_gamma: if sorted.is_empty() {
            None
        } else {
            finite(sum / sorted.len() as f64)
        },
        p95_gamma,
        max_gamma: finite(max_gamma),
        gamma_volume: volume,
    })
}

impl GammaEvaluation {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, GAMMA_EVALUATION_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported gamma-evaluation schema {:?}",
                self.schema_version
            )));
        }
        for result in &self.results {
            if !(0.0..=1.0).contains(&result.pass_rate) {
                return Err(ManifestError::Invalid(format!(
                    "{:?} pass rate is malformed",
                    result.quantity
                )));
            }
            if result.voxels_evaluated + result.voxels_excluded == 0 {
                return Err(ManifestError::Invalid(format!(
                    "{:?} evaluated over zero voxels",
                    result.quantity
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{
        DoseComponent, DoseUnit, DoseVolume, GridGeometry, PHYSICAL_DOSE_BUNDLE_SCHEMA,
        PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "b".repeat(64),
        }
    }

    fn geometry(shape: [u32; 3], spacing: [f64; 3]) -> GridGeometry {
        GridGeometry {
            shape,
            spacing_mm: spacing,
            origin_mm: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn bundle(case_id: &str, geometry: GridGeometry, values: Vec<f64>) -> PhysicalDoseBundle {
        let components = DoseComponent::REQUIRED
            .iter()
            .map(|component| DoseVolume {
                component: *component,
                unit: DoseUnit::GrayPerSourceParticle,
                values: values.clone(),
                absolute_standard_uncertainty: None,
            })
            .collect();
        PhysicalDoseBundle {
            schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: case_id.into(),
            frame_of_reference_uid: None,
            geometry,
            component_profile: cref("profile"),
            response_set: cref("responses"),
            components,
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values,
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::Unavailable,
            },
            provenance_id: "prov".into(),
        }
    }

    fn criteria(dd_percent: f64, dta_mm: f64) -> GammaCriteria {
        GammaCriteria {
            dose_difference_percent: dd_percent,
            distance_to_agreement_mm: dta_mm,
            normalization: GammaNormalization::Global,
            dose_threshold_percent: None,
        }
    }

    fn evaluate(
        reference: &PhysicalDoseBundle,
        candidate: &PhysicalDoseBundle,
        criteria: GammaCriteria,
    ) -> GammaEvaluation {
        evaluate_gamma(
            reference,
            candidate,
            cref("ref"),
            cref("cand"),
            criteria,
            false,
        )
        .expect("gamma evaluation")
    }

    fn total_result(evaluation: &GammaEvaluation) -> &GammaQuantityResult {
        evaluation
            .results
            .iter()
            .find(|r| r.quantity == "physical_total")
            .expect("physical_total result")
    }

    #[test]
    fn identical_fields_pass_completely() {
        let grid = geometry([8, 8, 8], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![7.5; 512]);
        let candidate = bundle("case", grid, vec![7.5; 512]);
        let evaluation = evaluate(&reference, &candidate, criteria(3.0, 3.0));
        let total = total_result(&evaluation);
        assert_eq!(total.voxels_evaluated, 512);
        assert_eq!(total.voxels_excluded, 0);
        assert_eq!(total.pass_rate, 1.0);
        assert_eq!(total.max_gamma, Some(0.0));
        // Every required component plus the physical total.
        assert_eq!(evaluation.results.len(), 5);
    }

    #[test]
    fn uniform_offset_below_criterion_passes() {
        let grid = geometry([4, 4, 4], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![100.0; 64]);
        let candidate = bundle("case", grid, vec![102.9; 64]); // 2.9% < 3%
        let evaluation = evaluate(&reference, &candidate, criteria(3.0, 1.0));
        assert_eq!(total_result(&evaluation).pass_rate, 1.0);
    }

    #[test]
    fn uniform_offset_above_criterion_fails() {
        let grid = geometry([4, 4, 4], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![100.0; 64]);
        let candidate = bundle("case", grid, vec![103.1; 64]); // 3.1% > 3%, uniform → dta can't help
        let evaluation = evaluate(&reference, &candidate, criteria(3.0, 3.0));
        assert_eq!(total_result(&evaluation).pass_rate, 0.0);
    }

    #[test]
    fn one_voxel_spatial_shift_passes_with_dta() {
        // A step at x=4 shifted one voxel right in the candidate.
        let grid = geometry([8, 1, 1], [1.0, 1.0, 1.0]);
        let reference_values: Vec<f64> = (0..8).map(|i| if i >= 4 { 100.0 } else { 0.0 }).collect();
        let candidate_values: Vec<f64> = (0..8).map(|i| if i >= 5 { 100.0 } else { 0.0 }).collect();
        let reference = bundle("case", grid.clone(), reference_values);
        let candidate = bundle("case", grid, candidate_values);
        let evaluation = evaluate(&reference, &candidate, criteria(3.0, 1.5));
        // With dta=1.5 mm the shift's γ = 1/1.5 ≈ 0.67 at the two edge voxels.
        assert_eq!(total_result(&evaluation).pass_rate, 1.0);
    }

    #[test]
    fn one_voxel_shift_fails_with_tight_dta() {
        let grid = geometry([8, 1, 1], [1.0, 1.0, 1.0]);
        let reference_values: Vec<f64> = (0..8).map(|i| if i >= 4 { 100.0 } else { 0.0 }).collect();
        let candidate_values: Vec<f64> = (0..8).map(|i| if i >= 5 { 100.0 } else { 0.0 }).collect();
        let reference = bundle("case", grid.clone(), reference_values);
        let candidate = bundle("case", grid, candidate_values);
        // dta=0.5 mm < 1 mm spacing → only the coincident voxel counts.
        let evaluation = evaluate(&reference, &candidate, criteria(3.0, 0.5));
        let total = total_result(&evaluation);
        // Only voxel 4 mismatches (ref=100 vs cand=0); voxel 5 agrees (100=100).
        assert_eq!(total.pass_rate, 7.0 / 8.0);
    }

    #[test]
    fn dose_threshold_excludes_low_dose_voxels() {
        let grid = geometry([4, 1, 1], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![100.0, 5.0, 100.0, 0.0]);
        let candidate = bundle("case", grid, vec![99.0, 500.0, 100.0, 900.0]);
        let mut c = criteria(3.0, 1.0);
        c.dose_threshold_percent = Some(10.0); // cutoff 10 → only the two 100-voxels evaluate
        let evaluation = evaluate(&reference, &candidate, c);
        let total = total_result(&evaluation);
        assert_eq!(total.voxels_evaluated, 2);
        assert_eq!(total.voxels_excluded, 2);
        assert_eq!(total.pass_rate, 1.0); // excluded voxels' huge errors don't count
    }

    #[test]
    fn local_normalization_is_stricter_at_low_dose() {
        let grid = geometry([4, 1, 1], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![100.0, 10.0, 10.0, 10.0]);
        // +0.5 absolute everywhere: 0.5% of global max but 5% of the 10-voxels.
        let candidate = bundle("case", grid, vec![100.5, 10.5, 10.5, 10.5]);
        let mut c = criteria(3.0, 1.0);
        c.normalization = GammaNormalization::Local;
        let evaluation = evaluate(&reference, &candidate, c);
        // Global would pass all; local passes only the 100-voxel.
        assert_eq!(total_result(&evaluation).pass_rate, 0.25);
    }

    #[test]
    fn mismatched_case_or_grid_rejects() {
        let grid = geometry([4, 4, 4], [1.0, 1.0, 1.0]);
        let reference = bundle("case-a", grid.clone(), vec![1.0; 64]);
        let other_case = bundle("case-b", grid.clone(), vec![1.0; 64]);
        assert!(
            evaluate_gamma(
                &reference,
                &other_case,
                cref("r"),
                cref("c"),
                criteria(3.0, 3.0),
                false
            )
            .is_err()
        );

        let other_grid = bundle(
            "case-a",
            geometry([4, 4, 4], [2.0, 1.0, 1.0]),
            vec![1.0; 64],
        );
        assert!(
            evaluate_gamma(
                &reference,
                &other_grid,
                cref("r"),
                cref("c"),
                criteria(3.0, 3.0),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn criteria_reject_nonpositive_and_bad_threshold() {
        assert!(criteria(0.0, 3.0).validate().is_err());
        assert!(criteria(3.0, 0.0).validate().is_err());
        assert!(criteria(f64::NAN, 3.0).validate().is_err());
        let mut c = criteria(3.0, 3.0);
        c.dose_threshold_percent = Some(100.0);
        assert!(c.validate().is_err());
    }

    #[test]
    fn evaluation_serializes_and_validates() {
        let grid = geometry([4, 4, 4], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![9.0; 64]);
        let candidate = bundle("case", grid, vec![9.1; 64]);
        let evaluation = evaluate_gamma(
            &reference,
            &candidate,
            cref("ref"),
            cref("cand"),
            criteria(3.0, 3.0),
            true,
        )
        .expect("evaluation");
        assert_eq!(evaluation.schema_version, "openbnct.gamma-evaluation/0.1.0");
        let json = serde_json::to_string_pretty(&evaluation).expect("serialize");
        let parsed: GammaEvaluation = serde_json::from_str(&json).expect("deserialize");
        parsed.validate().expect("validate");
        assert_eq!(parsed, evaluation);
        // Volume was emitted and aligns with the grid.
        let volume = total_result(&parsed)
            .gamma_volume
            .as_ref()
            .expect("gamma volume");
        assert_eq!(volume.len(), 64);
        assert!(volume.iter().all(|v| v.is_some()));
    }

    #[test]
    fn missing_component_rejects() {
        let grid = geometry([4, 4, 4], [1.0, 1.0, 1.0]);
        let reference = bundle("case", grid.clone(), vec![1.0; 64]);
        let mut candidate = bundle("case", grid, vec![1.0; 64]);
        candidate
            .components
            .retain(|c| c.component != DoseComponent::Photon);
        assert!(
            evaluate_gamma(
                &reference,
                &candidate,
                cref("r"),
                cref("c"),
                criteria(3.0, 3.0),
                false
            )
            .is_err()
        );
    }
}
