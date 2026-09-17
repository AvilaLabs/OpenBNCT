// SPDX-License-Identifier: Apache-2.0

//! Metamorphic transport oracles (`openbnct.metamorphic-evaluation/0.1.0`).
//!
//! Metamorphic testing verifies a Monte Carlo transport run against
//! *relations* rather than a fixed reference answer — the standard
//! complement to conformance fixtures when no independent ground truth
//! exists. Each oracle pairs dose bundles (or a bundle with itself)
//! under a declared symmetry and reports voxelwise z-score agreement
//! against combined Monte Carlo uncertainty:
//!
//! - `reflection_symmetry` — a bundle compared against its own
//!   reflection about a grid axis. Valid only for problems the operator
//!   declares symmetric about that axis (homogeneous geometry plus a
//!   symmetric source); the declaration is recorded, not proven.
//! - `rotation_invariance` — a reference bundle against a second bundle
//!   produced from the same case with the source rotated a quarter-turn
//!   about a grid axis; the candidate volume is permuted back into the
//!   reference frame and compared. Requires equal in-plane grid extents.
//! - `superposition` — a combined-source run (reference) against the
//!   voxelwise sum of two component-source runs (a + b). Combined σ is
//!   `sqrt(σ_ref² + σ_a² + σ_b²)` under independence.
//! - `point_reciprocity` — dose at declared voxel `a` in run B against
//!   dose at voxel `b` in run A (source↔detector interchange). Reported
//!   per quantity as a paired z-score, not a field statistic.
//!
//! The record binds every input artifact by content hash, states the
//! declared symmetry so a reviewer can reject an invalid premise, and
//! asserts no equivalence claim: passing oracles are *consistent-with*
//! evidence, not proof of correctness.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, PhysicalDoseBundle, grid_geometry_equivalent};

use crate::ManifestError;

/// Versioned metamorphic evaluation schema.
pub const METAMORPHIC_SCHEMA: &str = "openbnct.metamorphic-evaluation/0.1.0";

/// The oracle evaluated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetamorphicOracle {
    /// Bundle vs its own reflection about axis `axis`.
    ReflectionSymmetry {
        /// `x`, `y`, or `z`.
        axis: String,
        /// Why this problem is symmetric about the axis — recorded so a
        /// reviewer can reject the premise.
        declared_symmetry: String,
    },
    /// Reference bundle vs a run with the source rotated `turns`
    /// quarter-turns (1–3) about `axis`; the candidate volume is
    /// rotated back into the reference frame for comparison.
    RotationInvariance { axis: String, turns: u8 },
    /// Reference (combined-source run) vs the sum of runs `a` and `b`.
    Superposition,
    /// Dose at `voxel_a` in the candidate run vs dose at `voxel_b` in
    /// the reference run (source↔detector interchange).
    PointReciprocity { voxel_a: u64, voxel_b: u64 },
}

/// Agreement statistics for one quantity under an oracle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleQuantity {
    /// `component:boron`, …, or `physical_total`.
    pub quantity: String,
    pub unit: String,
    /// Voxel pairs evaluated (both values finite; for superposition all
    /// three finite).
    pub evaluated_pairs: u64,
    /// Pairs excluded for non-finite values.
    pub excluded_pairs: u64,
    /// Fraction of evaluated pairs within `sigma_level` combined σ —
    /// present only when every input states an uncertainty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_sigma_fraction: Option<f64>,
    /// Mean absolute z-score |a − b| / σ_combined over evaluated pairs
    /// (abs difference where σ is absent).
    pub mean_abs_z: Option<f64>,
    /// Largest z-score over evaluated pairs.
    pub max_z: Option<f64>,
    /// Largest absolute voxel difference.
    pub max_abs_difference: f64,
    /// Root-mean-square voxel difference.
    pub rms_difference: f64,
}

/// The evaluation record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetamorphicEvaluation {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub oracle: MetamorphicOracle,
    /// Every input bundle, content-bound; order is reference first,
    /// then candidate(s) in argument order.
    pub inputs: Vec<ContentReference>,
    /// Combined-uncertainty multiplier for the within-sigma fraction.
    pub sigma_level: f64,
    /// Per-quantity oracle statistics.
    pub quantities: Vec<OracleQuantity>,
    /// Research-status qualification; no equivalence or clinical claim.
    pub qualification: String,
    pub provenance_id: String,
}

fn invalid(msg: String) -> ManifestError {
    ManifestError::Invalid(format!("metamorphic evaluation: {msg}"))
}

fn axis_index(axis: &str) -> Result<usize, ManifestError> {
    match axis {
        "x" => Ok(0),
        "y" => Ok(1),
        "z" => Ok(2),
        other => Err(invalid(format!("axis must be x|y|z, got {other:?}"))),
    }
}

/// Voxel index under reflection about `axis`: the coordinate along the
/// axis is mirrored about the grid's index range.
fn reflect_index(shape: [u32; 3], axis: usize, index: usize) -> usize {
    let [nx, ny, nz] = shape.map(|d| d as usize);
    let i = index % nx;
    let j = (index / nx) % ny;
    let k = index / (nx * ny);
    let (i, j, k) = match axis {
        0 => (nx - 1 - i, j, k),
        1 => (i, ny - 1 - j, k),
        _ => (i, j, nz - 1 - k),
    };
    i + nx * j + nx * ny * k
}

/// Voxel index under `turns` quarter-turns about `axis` — the forward
/// rotation matching `openbnct_transport::rotate_source`, so a run
/// produced with that source rotation lands its dose field on this
/// permutation. Each quarter-turn about axis `a` maps the other two
/// axes `(u, v) → (n − 1 − v, u)` in index space (the `rotate_quarter`
/// `(−v, u)` continuous convention).
fn rotate_index(shape: [u32; 3], axis: usize, turns: u8, index: usize) -> usize {
    let shape = shape.map(|d| d as usize);
    let mut c = [
        index % shape[0],
        (index / shape[0]) % shape[1],
        index / (shape[0] * shape[1]),
    ];
    for _ in 0..turns {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let nv = shape[v];
        (c[u], c[v]) = (nv - 1 - c[v], c[u]);
    }
    c[0] + shape[0] * c[1] + shape[0] * shape[1] * c[2]
}

struct VolumeSlice<'a> {
    name: String,
    unit: String,
    values: &'a [f64],
    sigma: Option<&'a [f64]>,
}

fn quantity_slices(bundle: &PhysicalDoseBundle) -> Result<Vec<VolumeSlice<'_>>, ManifestError> {
    let mut slices = Vec::with_capacity(bundle.components.len() + 1);
    for component in &bundle.components {
        let name = serde_json::to_value(component.component)
            .and_then(serde_json::from_value::<String>)
            .map_err(|e| invalid(format!("component name: {e}")))?;
        let unit = serde_json::to_value(component.unit)
            .and_then(serde_json::from_value::<String>)
            .map_err(|e| invalid(format!("unit: {e}")))?;
        slices.push(VolumeSlice {
            name: format!("component:{name}"),
            unit,
            values: &component.values,
            sigma: component.absolute_standard_uncertainty.as_deref(),
        });
    }
    let unit = serde_json::to_value(bundle.physical_total.unit)
        .and_then(serde_json::from_value::<String>)
        .map_err(|e| invalid(format!("unit: {e}")))?;
    slices.push(VolumeSlice {
        name: "physical_total".into(),
        unit,
        values: &bundle.physical_total.values,
        sigma: bundle
            .physical_total
            .absolute_standard_uncertainty
            .as_deref(),
    });
    Ok(slices)
}

fn same_components(
    reference: &PhysicalDoseBundle,
    candidate: &PhysicalDoseBundle,
) -> Result<(), ManifestError> {
    if reference.case_id != candidate.case_id {
        return Err(invalid(format!(
            "case_id mismatch: {:?} vs {:?}",
            reference.case_id, candidate.case_id
        )));
    }
    if !grid_geometry_equivalent(&reference.geometry, &candidate.geometry) {
        return Err(invalid("grids differ".into()));
    }
    if reference.components.len() != candidate.components.len() {
        return Err(invalid("component sets differ".into()));
    }
    Ok(())
}

/// z-statistics over paired samples. `pairs` yields
/// `(reference, candidate, combined_1sigma)` — `None` σ when any side
/// lacks it.
fn pair_statistics(
    quantity: &str,
    unit: &str,
    pairs: impl Iterator<Item = (f64, f64, Option<f64>)>,
) -> OracleQuantity {
    let mut evaluated = 0u64;
    let mut excluded = 0u64;
    let mut have_sigma = true;
    let mut sum_z = 0.0;
    let mut max_z = 0.0_f64;
    let mut max_abs = 0.0_f64;
    let mut sum_sq = 0.0;
    for (a, b, sigma) in pairs {
        if !a.is_finite() || !b.is_finite() || sigma.is_some_and(|s| !s.is_finite()) {
            excluded += 1;
            continue;
        }
        evaluated += 1;
        let diff = (a - b).abs();
        max_abs = max_abs.max(diff);
        sum_sq += diff * diff;
        match sigma {
            Some(s) if s > 0.0 => {
                let z = diff / s;
                sum_z += z;
                max_z = max_z.max(z);
            }
            _ => {
                have_sigma = false;
            }
        }
    }
    OracleQuantity {
        quantity: quantity.into(),
        unit: unit.into(),
        evaluated_pairs: evaluated,
        excluded_pairs: excluded,
        within_sigma_fraction: None,
        mean_abs_z: if have_sigma && evaluated > 0 {
            Some(sum_z / evaluated as f64)
        } else {
            None
        },
        max_z: if have_sigma && evaluated > 0 {
            Some(max_z)
        } else {
            None
        },
        max_abs_difference: if evaluated > 0 { max_abs } else { 0.0 },
        rms_difference: if evaluated > 0 {
            (sum_sq / evaluated as f64).sqrt()
        } else {
            0.0
        },
    }
}

/// Evaluate a metamorphic oracle. Bundle roles by oracle:
///
/// - `reflection_symmetry`: `reference` only (compared to itself).
/// - `rotation_invariance`, `point_reciprocity`: `reference` and
///   `candidates[0]` (the rotated / detector-site run).
/// - `superposition`: `reference` (combined run) and `candidates[0]`,
///   `candidates[1]` (the two component runs).
#[allow(clippy::too_many_arguments)]
pub fn evaluate_metamorphic(
    id: &str,
    oracle: &MetamorphicOracle,
    reference: &PhysicalDoseBundle,
    candidates: &[&PhysicalDoseBundle],
    inputs: Vec<ContentReference>,
    sigma_level: f64,
    provenance_id: &str,
) -> Result<MetamorphicEvaluation, ManifestError> {
    if id.trim().is_empty() {
        return Err(invalid("id is empty".into()));
    }
    if !(sigma_level.is_finite() && sigma_level > 0.0) {
        return Err(invalid(format!(
            "sigma level {sigma_level} must be positive"
        )));
    }
    if inputs.len() != 1 + candidates.len() {
        return Err(invalid(format!(
            "inputs length {} does not match 1 + {} candidates",
            inputs.len(),
            candidates.len()
        )));
    }
    for reference_ in &inputs {
        reference_
            .validate()
            .map_err(|e| invalid(format!("input content reference: {e}")))?;
    }
    let n = reference
        .geometry
        .voxel_count()
        .map_err(|e| invalid(format!("geometry: {e}")))?;
    for candidate in candidates {
        same_components(reference, candidate)?;
    }

    // Oracle-specific checks and the index map candidate→reference frame.
    let index_map: Box<dyn Fn(usize) -> usize> = match oracle {
        MetamorphicOracle::ReflectionSymmetry {
            axis,
            declared_symmetry,
        } => {
            if declared_symmetry.trim().is_empty() {
                return Err(invalid(
                    "reflection_symmetry requires a non-empty declared_symmetry".into(),
                ));
            }
            let a = axis_index(axis)?;
            Box::new(move |i| reflect_index(reference.geometry.shape, a, i))
        }
        MetamorphicOracle::RotationInvariance { axis, turns } => {
            if candidates.is_empty() {
                return Err(invalid("rotation_invariance requires a candidate".into()));
            }
            if !(1..=3).contains(turns) {
                return Err(invalid(format!("turns must be 1..=3, got {turns}")));
            }
            let a = axis_index(axis)?;
            let (u, v) = ((a + 1) % 3, (a + 2) % 3);
            let shape = reference.geometry.shape;
            if shape[u] != shape[v] {
                return Err(invalid(format!(
                    "rotation about {axis} needs equal in-plane extents, got {} vs {}",
                    shape[u], shape[v]
                )));
            }
            let sp = reference.geometry.spacing_mm;
            if (sp[u] - sp[v]).abs() > f64::EPSILON {
                return Err(invalid(format!(
                    "rotation about {axis} needs equal in-plane spacings, got {} vs {}",
                    sp[u], sp[v]
                )));
            }
            Box::new(move |i| rotate_index(shape, a, *turns, i))
        }
        MetamorphicOracle::Superposition => {
            if candidates.len() != 2 {
                return Err(invalid(
                    "superposition requires exactly two candidates".into(),
                ));
            }
            Box::new(|i| i)
        }
        MetamorphicOracle::PointReciprocity { voxel_a, voxel_b } => {
            if candidates.is_empty() {
                return Err(invalid("point_reciprocity requires a candidate".into()));
            }
            if *voxel_a as usize >= n || *voxel_b as usize >= n {
                return Err(invalid(format!(
                    "reciprocity voxels {voxel_a},{voxel_b} exceed grid voxel count {n}"
                )));
            }
            Box::new(|i| i)
        }
    };

    let reference_slices = quantity_slices(reference)?;
    let mut quantities = Vec::new();
    for reference_slice in &reference_slices {
        // Locate the matching quantity in each candidate.
        let mut candidate_slices = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let slices = quantity_slices(candidate)?;
            let found = slices
                .into_iter()
                .find(|s| s.name == reference_slice.name)
                .ok_or_else(|| {
                    invalid(format!(
                        "candidate is missing quantity {:?}",
                        reference_slice.name
                    ))
                })?;
            if found.unit != reference_slice.unit {
                return Err(invalid(format!(
                    "quantity {:?} unit mismatch: {:?} vs {:?}",
                    reference_slice.name, found.unit, reference_slice.unit
                )));
            }
            candidate_slices.push(found);
        }

        let quantity = match oracle {
            MetamorphicOracle::PointReciprocity { voxel_a, voxel_b } => {
                let a = *voxel_a as usize;
                let b = *voxel_b as usize;
                let cand = &candidate_slices[0];
                let sigma = match (reference_slice.sigma, cand.sigma) {
                    (Some(sr), Some(sc)) => Some((sr[b].powi(2) + sc[a].powi(2)).sqrt()),
                    _ => None,
                };
                pair_statistics(
                    &reference_slice.name,
                    &reference_slice.unit,
                    std::iter::once((reference_slice.values[b], cand.values[a], sigma)),
                )
            }
            MetamorphicOracle::Superposition => {
                let sa = &candidate_slices[0];
                let sb = &candidate_slices[1];
                let map = &index_map;
                let iter = (0..n).map(|i| {
                    let ia = map(i);
                    let ib = map(i);
                    let sigma = match (reference_slice.sigma, sa.sigma, sb.sigma) {
                        (Some(sr), Some(sa), Some(sb)) => {
                            Some((sr[i].powi(2) + sa[ia].powi(2) + sb[ib].powi(2)).sqrt())
                        }
                        _ => None,
                    };
                    (
                        reference_slice.values[i],
                        sa.values[ia] + sb.values[ib],
                        sigma,
                    )
                });
                pair_statistics(&reference_slice.name, &reference_slice.unit, iter)
            }
            _ => {
                // reflection_symmetry uses the reference as its own
                // candidate; rotation_invariance uses candidates[0].
                let (values, sigma) = match oracle {
                    MetamorphicOracle::ReflectionSymmetry { .. } => {
                        (reference_slice.values, reference_slice.sigma)
                    }
                    _ => (candidate_slices[0].values, candidate_slices[0].sigma),
                };
                let map = &index_map;
                let iter = (0..n).map(|i| {
                    let j = map(i);
                    let sigma = match (reference_slice.sigma, sigma) {
                        (Some(sr), Some(sc)) => Some((sr[i].powi(2) + sc[j].powi(2)).sqrt()),
                        _ => None,
                    };
                    (reference_slice.values[i], values[j], sigma)
                });
                pair_statistics(&reference_slice.name, &reference_slice.unit, iter)
            }
        };

        // Fill in within_sigma_fraction where combined σ exists — a
        // second pass over the same pairing.
        let mut quantity = quantity;
        let fraction = match oracle {
            MetamorphicOracle::PointReciprocity { voxel_a, voxel_b } => {
                let a = *voxel_a as usize;
                let b = *voxel_b as usize;
                let cand = &candidate_slices[0];
                match (reference_slice.sigma, cand.sigma) {
                    (Some(sr), Some(sc)) => {
                        let s = (sr[b].powi(2) + sc[a].powi(2)).sqrt();
                        if s > 0.0 {
                            let z = (reference_slice.values[b] - cand.values[a]).abs() / s;
                            Some((z <= sigma_level) as u8 as f64)
                        } else {
                            Some(1.0)
                        }
                    }
                    _ => None,
                }
            }
            _ => {
                if quantity.mean_abs_z.is_some() {
                    let mut within = 0u64;
                    let map = &index_map;
                    for i in 0..n {
                        let (a_val, b_val, s) = match oracle {
                            MetamorphicOracle::Superposition => {
                                let sa = &candidate_slices[0];
                                let sb = &candidate_slices[1];
                                let (ia, ib) = (map(i), map(i));
                                let s = match (reference_slice.sigma, sa.sigma, sb.sigma) {
                                    (Some(sr), Some(sa_s), Some(sb_s)) => {
                                        (sr[i].powi(2) + sa_s[ia].powi(2) + sb_s[ib].powi(2)).sqrt()
                                    }
                                    _ => continue,
                                };
                                (reference_slice.values[i], sa.values[ia] + sb.values[ib], s)
                            }
                            MetamorphicOracle::ReflectionSymmetry { .. } => {
                                let j = map(i);
                                let s = match (reference_slice.sigma, reference_slice.sigma) {
                                    (Some(sr), Some(_)) => (sr[i].powi(2) + sr[j].powi(2)).sqrt(),
                                    _ => continue,
                                };
                                (reference_slice.values[i], reference_slice.values[j], s)
                            }
                            MetamorphicOracle::RotationInvariance { .. } => {
                                let cand = &candidate_slices[0];
                                let j = map(i);
                                let s = match (reference_slice.sigma, cand.sigma) {
                                    (Some(sr), Some(sc)) => (sr[i].powi(2) + sc[j].powi(2)).sqrt(),
                                    _ => continue,
                                };
                                (reference_slice.values[i], cand.values[j], s)
                            }
                            MetamorphicOracle::PointReciprocity { .. } => unreachable!(),
                        };
                        if a_val.is_finite()
                            && b_val.is_finite()
                            && s.is_finite()
                            && s > 0.0
                            && (a_val - b_val).abs() <= sigma_level * s
                        {
                            within += 1;
                        }
                    }
                    if quantity.evaluated_pairs > 0 {
                        Some(within as f64 / quantity.evaluated_pairs as f64)
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };
        quantity.within_sigma_fraction = fraction;
        quantities.push(quantity);
    }

    Ok(MetamorphicEvaluation {
        schema_version: METAMORPHIC_SCHEMA.into(),
        id: id.into(),
        case_id: reference.case_id.clone(),
        oracle: oracle.clone(),
        inputs,
        sigma_level,
        quantities,
        qualification: "metamorphic_consistency_evidence_only_no_equivalence_or_clinical_claim"
            .into(),
        provenance_id: provenance_id.into(),
    })
}

/// Structural validation of a loaded record.
impl MetamorphicEvaluation {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, METAMORPHIC_SCHEMA) {
            return Err(invalid(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() {
            return Err(invalid("id is empty".into()));
        }
        if self.provenance_id.trim().is_empty() {
            return Err(invalid("provenance_id is empty".into()));
        }
        if self.inputs.is_empty() {
            return Err(invalid("inputs is empty".into()));
        }
        for input in &self.inputs {
            input
                .validate()
                .map_err(|e| invalid(format!("input content reference: {e}")))?;
        }
        if !(self.sigma_level.is_finite() && self.sigma_level > 0.0) {
            return Err(invalid("sigma_level must be positive".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{
        DoseComponent, DoseUnit, GridGeometry, PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 4, 4],
            spacing_mm: [5.0, 5.0, 5.0],
            origin_mm: [-10.0, -10.0, -10.0],
            direction: [
                1.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, //
                0.0, 0.0, 1.0,
            ],
        }
    }

    fn bundle_with(values: Vec<f64>, sigma: Option<Vec<f64>>) -> PhysicalDoseBundle {
        PhysicalDoseBundle {
            schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "case".into(),
            frame_of_reference_uid: None,
            geometry: geometry(),
            component_profile: ContentReference {
                id: "profile".into(),
                sha256: "0".repeat(64),
            },
            response_set: ContentReference {
                id: "responses".into(),
                sha256: "0".repeat(64),
            },
            provenance_id: "test".into(),
            components: vec![openbnct_core::DoseVolume {
                component: DoseComponent::Boron,
                unit: DoseUnit::GrayPerSourceParticle,
                values: values.clone(),
                absolute_standard_uncertainty: sigma.clone(),
            }],
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values,
                absolute_standard_uncertainty: sigma,
                uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
            },
        }
    }

    fn cref(name: &str) -> ContentReference {
        ContentReference {
            id: name.into(),
            sha256: "0".repeat(64),
        }
    }

    #[test]
    fn reflection_of_symmetric_field_passes() {
        // Field symmetric about x: f(i) depends only on j,k.
        let n = 64;
        let mut values = vec![0.0; n];
        let mut sigma = vec![0.0; n];
        for k in 0..4 {
            for j in 0..4 {
                for i in 0..4 {
                    let idx = i + 4 * j + 16 * k;
                    values[idx] = (j + k) as f64 * 10.0;
                    sigma[idx] = 0.5;
                }
            }
        }
        let bundle = bundle_with(values, Some(sigma));
        let evaluation = evaluate_metamorphic(
            "e1",
            &MetamorphicOracle::ReflectionSymmetry {
                axis: "x".into(),
                declared_symmetry: "homogeneous slab, isotropic plane source".into(),
            },
            &bundle,
            &[],
            vec![cref("a")],
            2.0,
            "p",
        )
        .unwrap();
        for q in &evaluation.quantities {
            assert_eq!(q.within_sigma_fraction, Some(1.0));
            assert_eq!(q.max_abs_difference, 0.0);
        }
    }

    #[test]
    fn reflection_detects_asymmetry() {
        let mut values = vec![1.0; 64];
        values[0] = 100.0; // corner voxel, far from its mirror partner
        let sigma = vec![0.01; 64];
        let bundle = bundle_with(values, Some(sigma));
        let evaluation = evaluate_metamorphic(
            "e2",
            &MetamorphicOracle::ReflectionSymmetry {
                axis: "x".into(),
                declared_symmetry: "test".into(),
            },
            &bundle,
            &[],
            vec![cref("a")],
            2.0,
            "p",
        )
        .unwrap();
        let total = evaluation
            .quantities
            .iter()
            .find(|q| q.quantity == "physical_total")
            .unwrap();
        assert!(total.within_sigma_fraction.unwrap() < 1.0);
        assert!(total.max_z.unwrap() > 10.0);
    }

    #[test]
    fn rotation_invariance_unpermutes_correctly() {
        // 4x4x4 grid rotated about z. Build a field with a distinctive
        // asymmetric feature, then build the "rotated run" by applying
        // the forward permutation.
        let n = 64;
        let mut values = vec![0.0; n];
        for k in 0..4 {
            for j in 0..4 {
                for i in 0..4 {
                    values[i + 4 * j + 16 * k] = (i + 2 * j + 3 * k) as f64;
                }
            }
        }
        let sigma = vec![0.1; n];
        let reference = bundle_with(values.clone(), Some(sigma.clone()));

        // Forward rotation about z: candidate[map(i)] = reference[i].
        // Simulate the rotated run with the same forward permutation the
        // evaluator applies.
        let shape = [4u32, 4, 4];
        let mut rotated = vec![0.0; n];
        for i in 0..n {
            rotated[rotate_index(shape, 2, 1, i)] = values[i];
        }
        let candidate = bundle_with(rotated, Some(sigma));

        let evaluation = evaluate_metamorphic(
            "e3",
            &MetamorphicOracle::RotationInvariance {
                axis: "z".into(),
                turns: 1,
            },
            &reference,
            &[&candidate],
            vec![cref("ref"), cref("cand")],
            2.0,
            "p",
        )
        .unwrap();
        for q in &evaluation.quantities {
            assert_eq!(q.within_sigma_fraction, Some(1.0));
            assert_eq!(q.max_abs_difference, 0.0);
        }
    }

    #[test]
    fn superposition_checks_additivity() {
        let a = bundle_with(vec![2.0; 64], Some(vec![0.1; 64]));
        let b = bundle_with(vec![3.0; 64], Some(vec![0.1; 64]));
        let total = bundle_with(vec![5.0; 64], Some(vec![0.15; 64]));
        let evaluation = evaluate_metamorphic(
            "e4",
            &MetamorphicOracle::Superposition,
            &total,
            &[&a, &b],
            vec![cref("ref"), cref("a"), cref("b")],
            2.0,
            "p",
        )
        .unwrap();
        for q in &evaluation.quantities {
            assert_eq!(q.within_sigma_fraction, Some(1.0));
        }

        // A non-additive combined run fails loudly.
        let bad_total = bundle_with(vec![9.0; 64], Some(vec![0.15; 64]));
        let evaluation = evaluate_metamorphic(
            "e5",
            &MetamorphicOracle::Superposition,
            &bad_total,
            &[&a, &b],
            vec![cref("ref"), cref("a"), cref("b")],
            2.0,
            "p",
        )
        .unwrap();
        let q = evaluation
            .quantities
            .iter()
            .find(|q| q.quantity == "physical_total")
            .unwrap();
        assert_eq!(q.within_sigma_fraction, Some(0.0));
    }

    #[test]
    fn point_reciprocity_compares_sites() {
        let mut ref_values = vec![0.0; 64];
        ref_values[7] = 4.0;
        let mut cand_values = vec![0.0; 64];
        cand_values[9] = 4.0;
        let reference = bundle_with(ref_values, Some(vec![0.1; 64]));
        let candidate = bundle_with(cand_values, Some(vec![0.1; 64]));
        let evaluation = evaluate_metamorphic(
            "e6",
            &MetamorphicOracle::PointReciprocity {
                voxel_a: 9,
                voxel_b: 7,
            },
            &reference,
            &[&candidate],
            vec![cref("ref"), cref("cand")],
            2.0,
            "p",
        )
        .unwrap();
        for q in &evaluation.quantities {
            assert_eq!(q.evaluated_pairs, 1);
            assert_eq!(q.max_abs_difference, 0.0);
        }
    }

    #[test]
    fn rejects_invalid_specs() {
        let bundle = bundle_with(vec![1.0; 64], Some(vec![0.1; 64]));
        // Reflection without a declared symmetry.
        assert!(
            evaluate_metamorphic(
                "e",
                &MetamorphicOracle::ReflectionSymmetry {
                    axis: "x".into(),
                    declared_symmetry: " ".into()
                },
                &bundle,
                &[],
                vec![cref("a")],
                2.0,
                "p"
            )
            .is_err()
        );
        // Superposition with wrong candidate count.
        assert!(
            evaluate_metamorphic(
                "e",
                &MetamorphicOracle::Superposition,
                &bundle,
                &[&bundle],
                vec![cref("a"), cref("b")],
                2.0,
                "p"
            )
            .is_err()
        );
        // Rotation on an asymmetric grid is rejected.
        let mut asymmetric = bundle_with(vec![1.0; 64], Some(vec![0.1; 64]));
        asymmetric.geometry.shape = [4, 4, 8];
        asymmetric.components[0].values = vec![1.0; 128];
        asymmetric.components[0].absolute_standard_uncertainty = Some(vec![0.1; 128]);
        asymmetric.physical_total.values = vec![1.0; 128];
        asymmetric.physical_total.absolute_standard_uncertainty = Some(vec![0.1; 128]);
        let candidate = bundle_with(vec![1.0; 64], Some(vec![0.1; 64]));
        assert!(
            evaluate_metamorphic(
                "e",
                &MetamorphicOracle::RotationInvariance {
                    axis: "z".into(),
                    turns: 1
                },
                &asymmetric,
                &[&candidate],
                vec![cref("a"), cref("b")],
                2.0,
                "p"
            )
            .is_err()
        );
    }
}
