// SPDX-License-Identifier: MIT

//! Cross-code dose comparison (`openbnct.dose-comparison/0.1.0`).
//!
//! Compares two physical dose bundles on the same frozen case — for example
//! an OpenMC-collected result against an MCNP/PHITS-imported one — and
//! records agreement statistics under both inputs' content hashes and
//! provenance chains. This is a research comparison record: it reports
//! measured agreement and never asserts equivalence between transport codes.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, PhysicalDoseBundle, grid_geometry_equivalent};

use crate::ManifestError;

pub const DOSE_COMPARISON_SCHEMA: &str = "openbnct.dose-comparison/0.1.0";

/// One bundle feeding a comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonInput {
    /// `reference` or `candidate`.
    pub role: String,
    /// Content hash of the exact artifact compared.
    pub content: ContentReference,
    /// The bundle's own provenance chain.
    pub provenance_id: String,
}

/// Voxelwise agreement statistics for one quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuantityAgreement {
    /// `component:boron`, … or `physical_total`.
    pub quantity: String,
    pub unit: String,
    /// Largest absolute voxel difference.
    pub max_abs_difference: f64,
    /// Mean absolute voxel difference.
    pub mean_abs_difference: f64,
    /// Root-mean-square voxel difference.
    pub rms_difference: f64,
    /// Largest difference normalized to the reference's maximum value —
    /// relative agreement anchored to a physically meaningful scale rather
    /// than per-voxel ratios that explode near zero.
    pub max_normalized_difference: f64,
    /// Fraction of voxels within `sigma_level` combined sigmas —
    /// `|a − b| ≤ k·sqrt(σa² + σb²)` — present only when both inputs state
    /// an uncertainty for this quantity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within_sigma_fraction: Option<f64>,
}

/// The comparison record itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseComparison {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub inputs: Vec<ComparisonInput>,
    /// Combined-uncertainty multiplier used for `within_sigma_fraction`.
    pub sigma_level: f64,
    pub voxel_count: u64,
    /// Per-quantity agreement: each component plus `physical_total`.
    pub quantities: Vec<QuantityAgreement>,
    /// Research-status qualification; no equivalence or clinical claim.
    pub qualification: String,
}

/// Compare two physical dose bundles voxel-by-voxel.
///
/// Both must share `case_id`, an equivalent grid, the same component set and
/// unit — a frozen case compared across codes; anything less is a different
/// comparison and is rejected. `sigma_level` sets the combined-uncertainty
/// multiplier for the within-sigma fraction.
pub fn compare_dose_bundles(
    reference: &PhysicalDoseBundle,
    candidate: &PhysicalDoseBundle,
    reference_ref: ContentReference,
    candidate_ref: ContentReference,
    sigma_level: f64,
) -> Result<DoseComparison, ManifestError> {
    let invalid = |msg: String| ManifestError::Invalid(format!("dose comparison: {msg}"));
    if !(sigma_level.is_finite() && sigma_level > 0.0) {
        return Err(invalid(format!(
            "sigma level {sigma_level} must be positive"
        )));
    }
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
    let voxel_count = reference
        .geometry
        .voxel_count()
        .map_err(|e| invalid(format!("geometry: {e}")))?;

    // Each candidate component must match a reference component exactly —
    // same set, same unit.
    if reference.components.len() != candidate.components.len() {
        return Err(invalid(format!(
            "component sets differ: {} vs {} components",
            reference.components.len(),
            candidate.components.len()
        )));
    }

    let mut quantities = Vec::new();
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
        quantities.push(agreement(
            &format!("component:{name}"),
            &unit,
            &reference_component.values,
            &candidate_component.values,
            reference_component.absolute_standard_uncertainty.as_deref(),
            candidate_component.absolute_standard_uncertainty.as_deref(),
            sigma_level,
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
    quantities.push(agreement(
        "physical_total",
        &total_unit,
        &reference.physical_total.values,
        &candidate.physical_total.values,
        reference
            .physical_total
            .absolute_standard_uncertainty
            .as_deref(),
        candidate
            .physical_total
            .absolute_standard_uncertainty
            .as_deref(),
        sigma_level,
    )?);

    let comparison = DoseComparison {
        schema_version: DOSE_COMPARISON_SCHEMA.into(),
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
        sigma_level,
        voxel_count: voxel_count as u64,
        quantities,
        qualification: "research cross-code comparison; reports measured voxelwise agreement — \
                        no equivalence, clinical, or commissioning claim"
            .into(),
    };
    comparison.validate()?;
    Ok(comparison)
}

fn agreement(
    quantity: &str,
    unit: &str,
    a: &[f64],
    b: &[f64],
    sigma_a: Option<&[f64]>,
    sigma_b: Option<&[f64]>,
    sigma_level: f64,
) -> Result<QuantityAgreement, ManifestError> {
    let invalid = |msg: String| ManifestError::Invalid(format!("dose comparison: {msg}"));
    if a.len() != b.len() {
        return Err(invalid(format!(
            "{quantity} length mismatch: {} vs {} voxels",
            a.len(),
            b.len()
        )));
    }
    if a.iter().chain(b.iter()).any(|v| !v.is_finite()) {
        return Err(invalid(format!("{quantity} contains non-finite values")));
    }
    let reference_max = a.iter().copied().fold(0.0, f64::max);
    let mut max_abs = 0.0_f64;
    let mut sum_abs = 0.0_f64;
    let mut sum_sq = 0.0_f64;
    for (x, y) in a.iter().zip(b) {
        let d = (x - y).abs();
        max_abs = max_abs.max(d);
        sum_abs += d;
        sum_sq += d * d;
    }
    let n = a.len() as f64;
    let within_sigma = match (sigma_a, sigma_b) {
        (Some(sa), Some(sb)) if sa.len() == a.len() && sb.len() == b.len() => {
            let k = sigma_level;
            let inside = a
                .iter()
                .zip(b)
                .zip(sa.iter().zip(sb))
                .filter(|&((&x, &y), (&sx, &sy))| (x - y).abs() <= k * (sx * sx + sy * sy).sqrt())
                .count();
            Some(inside as f64 / n)
        }
        _ => None,
    };
    Ok(QuantityAgreement {
        quantity: quantity.into(),
        unit: unit.into(),
        max_abs_difference: max_abs,
        mean_abs_difference: sum_abs / n,
        rms_difference: (sum_sq / n).sqrt(),
        max_normalized_difference: if reference_max > 0.0 {
            max_abs / reference_max
        } else {
            0.0
        },
        within_sigma_fraction: within_sigma,
    })
}

impl DoseComparison {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, DOSE_COMPARISON_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported dose-comparison schema {:?}",
                self.schema_version
            )));
        }
        if self.voxel_count == 0 {
            return Err(ManifestError::Invalid("comparison over zero voxels".into()));
        }
        for quantity in &self.quantities {
            for (label, value) in [
                ("max_abs_difference", quantity.max_abs_difference),
                ("mean_abs_difference", quantity.mean_abs_difference),
                ("rms_difference", quantity.rms_difference),
                (
                    "max_normalized_difference",
                    quantity.max_normalized_difference,
                ),
            ] {
                if !value.is_finite() || value < 0.0 {
                    return Err(ManifestError::Invalid(format!(
                        "{quantity:?} {label} is malformed"
                    )));
                }
            }
            if let Some(fraction) = quantity.within_sigma_fraction
                && !(0.0..=1.0).contains(&fraction)
            {
                return Err(ManifestError::Invalid(format!(
                    "{quantity:?} within-sigma fraction is malformed"
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
        DoseComponent, DoseUnit, DoseVolume, PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

    fn grid() -> openbnct_core::GridGeometry {
        openbnct_core::GridGeometry {
            shape: [2, 1, 1],
            spacing_mm: [5.0; 3],
            origin_mm: [0.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn bundle(values: [f64; 2], sigma: Option<[f64; 2]>, provenance: &str) -> PhysicalDoseBundle {
        let volume = |component| DoseVolume {
            component,
            unit: DoseUnit::GrayPerSourceParticle,
            values: values.to_vec(),
            absolute_standard_uncertainty: sigma.map(|s| s.to_vec()),
        };
        PhysicalDoseBundle {
            schema_version: "openbnct.physical-dose-bundle/0.2.0".into(),
            case_id: "case".into(),
            geometry: grid(),
            frame_of_reference_uid: None,
            components: vec![
                volume(DoseComponent::Boron),
                volume(DoseComponent::Nitrogen),
                volume(DoseComponent::Hydrogen),
                volume(DoseComponent::Photon),
            ],
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values: values.to_vec(),
                absolute_standard_uncertainty: sigma.map(|s| s.to_vec()),
                uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
            },
            component_profile: ContentReference {
                id: "p".into(),
                sha256: "0".repeat(64),
            },
            response_set: ContentReference {
                id: "r".into(),
                sha256: "0".repeat(64),
            },
            provenance_id: provenance.into(),
        }
    }

    fn reference(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "1".repeat(64),
        }
    }

    #[test]
    fn identical_bundles_report_zero_difference() {
        let a = bundle([1.0, 2.0], Some([0.1, 0.1]), "openmc:run-1");
        let cmp =
            compare_dose_bundles(&a, &a.clone(), reference("ref"), reference("cand"), 2.0).unwrap();
        assert_eq!(cmp.quantities.len(), 5);
        for quantity in &cmp.quantities {
            assert_eq!(quantity.max_abs_difference, 0.0);
            assert_eq!(quantity.within_sigma_fraction, Some(1.0));
        }
    }

    #[test]
    fn differences_and_sigma_fraction_are_reported() {
        let a = bundle([1.0, 2.0], Some([0.05, 0.05]), "openmc:run-1");
        let b = bundle([1.1, 1.9], Some([0.05, 0.05]), "phits:run-1");
        let cmp = compare_dose_bundles(&a, &b, reference("ref"), reference("cand"), 2.0).unwrap();
        let total = cmp
            .quantities
            .iter()
            .find(|q| q.quantity == "physical_total")
            .unwrap();
        assert!((total.max_abs_difference - 0.1).abs() < 1e-12);
        assert!((total.max_normalized_difference - 0.05).abs() < 1e-12);
        // |Δ|=0.1 ≤ 2·sqrt(0.05²+0.05²)=0.141 → all inside.
        assert_eq!(total.within_sigma_fraction, Some(1.0));
        // At sigma level 0.5 the same difference is outside.
        let cmp = compare_dose_bundles(&a, &b, reference("ref"), reference("cand"), 0.5).unwrap();
        assert_eq!(
            cmp.quantities
                .iter()
                .find(|q| q.quantity == "physical_total")
                .unwrap()
                .within_sigma_fraction,
            Some(0.0)
        );
    }

    #[test]
    fn mismatched_cases_and_grids_reject() {
        let a = bundle([1.0, 2.0], None, "a");
        let mut b = bundle([1.0, 2.0], None, "b");
        b.case_id = "other".into();
        assert!(compare_dose_bundles(&a, &b, reference("r"), reference("c"), 2.0).is_err());
        let mut b = bundle([1.0, 2.0], None, "b");
        b.geometry.shape = [4, 1, 1];
        assert!(compare_dose_bundles(&a, &b, reference("r"), reference("c"), 2.0).is_err());
    }
}
