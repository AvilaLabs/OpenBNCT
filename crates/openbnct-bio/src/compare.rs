// SPDX-License-Identifier: Apache-2.0

//! Cross-model biological-dose comparison.
//!
//! Applying two model families — a declared CBE/RBE weight model and a
//! microdosimetric-kinetic model, for example — to the same
//! `PhysicalDoseBundle` yields two `BiologicalDoseBundle`s whose
//! disagreement is itself a quantity worth recording: the
//! cross-model spread is part of the biological-interpretation
//! uncertainty. `compare_biological_models` binds both model artifacts,
//! checks they consume the same physical bundle on the same geometry,
//! and emits region-resolved statistics plus the pointwise worst-case
//! ratio on the biological totals.

use openbnct_core::{ContentReference, RegionMask};
use serde::{Deserialize, Serialize};

use crate::{BioError, BiologicalDoseBundle};

/// Versioned comparison artifact schema.
pub const BIO_MODEL_COMPARISON_SCHEMA: &str = "openbnct.bio-model-comparison/0.1.0";

/// Region-resolved statistics over one bundle's biological total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionStatistics {
    /// Region name; `"all"` marks the unmasked whole-phantom row.
    pub region: String,
    pub voxel_count: u64,
    pub mean: f64,
    pub maximum: f64,
    /// Region mean of the *other* bundle, for a single-table read.
    pub other_mean: f64,
    /// `mean / other_mean` — the cross-model scale ratio; `NaN` is
    /// never emitted (`null` when the denominator is zero).
    pub mean_ratio: Option<f64>,
}

/// A bounded, checkable record of two models disagreeing on one bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BioModelComparison {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The shared physical-bundle provenance both models consumed.
    pub physical_bundle_provenance: String,
    pub case_id: String,
    /// Content bindings of the two model artifacts, in the order the
    /// bundles were supplied.
    pub models: [ContentReference; 2],
    /// The weight semantics of each bundle (e.g. `photon_isoeffective`
    /// vs `microdosimetric_kinetic`) — the axis along which the
    /// comparison reads.
    pub weight_semantics: [String; 2],
    /// Unit labels of the two totals — may differ across families
    /// (`weighted_eqd2` vs `mkm_weighted_eqd2`); both are recorded so a
    /// ratio is only read within compatible units.
    pub units: [String; 2],
    /// Whole-phantom row plus one row per supplied region mask.
    pub regions: Vec<RegionStatistics>,
    /// Largest voxelwise absolute ratio `max(a,b)/min(a,b)` over the
    /// totals, restricted to voxels where the larger value exceeds
    /// `significant_floor`; `null` when no voxel qualifies.
    pub max_voxelwise_ratio: Option<f64>,
    /// Totals below this floor are excluded from the pointwise ratio —
    /// ratios of near-zero doses are noise, not disagreement.
    pub significant_floor: f64,
    pub qualification: String,
}

impl BioModelComparison {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, BIO_MODEL_COMPARISON_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() {
            return Err(BioError::Invalid("comparison id is empty".into()));
        }
        if self.physical_bundle_provenance.trim().is_empty() || self.case_id.trim().is_empty() {
            return Err(BioError::Invalid(
                "physical_bundle_provenance and case_id are required".into(),
            ));
        }
        for model in &self.models {
            model
                .validate()
                .map_err(|_| BioError::Invalid("model reference is invalid".into()))?;
        }
        for row in &self.regions {
            if row.region.trim().is_empty() {
                return Err(BioError::Invalid("region name is empty".into()));
            }
            for (label, value) in [
                ("mean", row.mean),
                ("maximum", row.maximum),
                ("other_mean", row.other_mean),
            ] {
                if !value.is_finite() || value < 0.0 {
                    return Err(BioError::Invalid(format!(
                        "region {:?} {label} must be finite and non-negative",
                        row.region
                    )));
                }
            }
            if let Some(ratio) = row.mean_ratio {
                if !ratio.is_finite() || ratio < 0.0 {
                    return Err(BioError::Invalid(format!(
                        "region {:?} mean_ratio must be finite and non-negative",
                        row.region
                    )));
                }
            }
        }
        if self.regions.is_empty() {
            return Err(BioError::Invalid(
                "at least the whole-phantom row is required".into(),
            ));
        }
        if !self.significant_floor.is_finite() || self.significant_floor < 0.0 {
            return Err(BioError::Invalid(
                "significant_floor must be finite and non-negative".into(),
            ));
        }
        Ok(())
    }
}

fn region_stats(
    label: &str,
    a: &[f64],
    b: &[f64],
    voxels: Option<&[bool]>,
) -> Result<RegionStatistics, BioError> {
    let gather = |values: &[f64]| -> Vec<f64> {
        match voxels {
            Some(mask) => values
                .iter()
                .zip(mask.iter())
                .filter(|(_, m)| **m)
                .map(|(v, _)| *v)
                .collect(),
            None => values.to_vec(),
        }
    };
    let (a, b) = (gather(a), gather(b));
    if a.is_empty() {
        return Err(BioError::Invalid(format!(
            "region {label:?} selects no voxels"
        )));
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let maximum = |v: &[f64]| v.iter().copied().fold(0.0_f64, f64::max);
    let (ma, mb) = (mean(&a), mean(&b));
    Ok(RegionStatistics {
        region: label.into(),
        voxel_count: a.len() as u64,
        mean: ma,
        maximum: maximum(&a),
        other_mean: mb,
        mean_ratio: (mb > 0.0).then(|| ma / mb),
    })
}

/// Compare two biological bundles that consumed the same physical
/// bundle. `masks` name the regions to resolve; an `"all"` row over the
/// whole phantom is always emitted. `significant_floor` sets the dose
/// level below which voxelwise ratios are ignored.
pub fn compare_biological_models(
    a: &BiologicalDoseBundle,
    b: &BiologicalDoseBundle,
    masks: &[RegionMask],
    id: String,
    significant_floor: f64,
) -> Result<BioModelComparison, BioError> {
    a.validate()?;
    b.validate()?;
    if a.physical_bundle_provenance != b.physical_bundle_provenance {
        return Err(BioError::Invalid(format!(
            "bundles consumed different physical bundles ({:?} vs {:?}) — a model comparison is only meaningful on a shared input",
            a.physical_bundle_provenance, b.physical_bundle_provenance
        )));
    }
    if a.geometry != b.geometry {
        return Err(BioError::Invalid(
            "bundle geometries differ — cannot compare voxelwise".into(),
        ));
    }
    if a.total.values.len() != b.total.values.len() {
        return Err(BioError::Invalid("total volumes differ in length".into()));
    }

    let mut regions = Vec::with_capacity(masks.len() + 1);
    regions.push(region_stats("all", &a.total.values, &b.total.values, None)?);
    let n_voxels = a.total.values.len();
    for mask in masks {
        if mask.voxels.len() != n_voxels {
            return Err(BioError::Invalid(format!(
                "region mask {:?} has {} voxels but the bundle volume has {n_voxels}",
                mask.name,
                mask.voxels.len()
            )));
        }
        regions.push(region_stats(
            &mask.name,
            &a.total.values,
            &b.total.values,
            Some(&mask.voxels),
        )?);
    }

    let mut max_ratio: Option<f64> = None;
    for (va, vb) in a.total.values.iter().zip(&b.total.values) {
        let (hi, lo) = (va.max(*vb), va.min(*vb));
        if hi >= significant_floor && lo > 0.0 {
            let ratio = hi / lo;
            if ratio > 1.0 && max_ratio.is_none_or(|m| ratio > m) {
                max_ratio = Some(ratio);
            }
        }
    }

    let comparison = BioModelComparison {
        schema_version: BIO_MODEL_COMPARISON_SCHEMA.into(),
        id,
        physical_bundle_provenance: a.physical_bundle_provenance.clone(),
        case_id: a.case_id.clone(),
        models: [a.model.clone(), b.model.clone()],
        weight_semantics: [
            serde_json::to_string(&a.weight_semantics)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string(),
            serde_json::to_string(&b.weight_semantics)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string(),
        ],
        units: [a.total.unit.clone(), b.total.unit.clone()],
        regions,
        max_voxelwise_ratio: max_ratio,
        significant_floor,
        qualification:
            "research-only: cross-model biological spread, not a clinical dose statement".into(),
    };
    comparison.validate()?;
    Ok(comparison)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BiologicalTotal, BiologicalUncertaintyMethod, WeightSemantics, WeightedDoseVolume,
    };
    use openbnct_core::{DoseComponent, GridGeometry};

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    fn bundle(model: &str, semantics: WeightSemantics, total: Vec<f64>) -> BiologicalDoseBundle {
        let n = total.len();
        BiologicalDoseBundle {
            schema_version: crate::BIOLOGICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "case".into(),
            geometry: GridGeometry {
                shape: [n as u32, 1, 1],
                spacing_mm: [1.0; 3],
                origin_mm: [0.0; 3],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            physical_bundle_provenance: "phys-1".into(),
            model: cref(model),
            weight_semantics: semantics,
            unit: "weighted_gray_per_source_particle".into(),
            components: DoseComponent::REQUIRED
                .iter()
                .map(|component| WeightedDoseVolume {
                    component: *component,
                    unit: "weighted_gray_per_source_particle".into(),
                    values: vec![0.0; n],
                    absolute_standard_uncertainty: None,
                })
                .collect(),
            fractionation: None,
            total: BiologicalTotal {
                unit: "weighted_gray_per_source_particle".into(),
                values: total,
                absolute_standard_uncertainty: None,
                uncertainty_method: BiologicalUncertaintyMethod::Unavailable,
            },
            regions_applied: vec![],
            qualification: "test".into(),
            microdosimetry: None,
        }
    }

    #[test]
    fn same_input_comparison_reports_ratio() {
        let a = bundle(
            "m-cbe",
            WeightSemantics::PhotonIsoeffective,
            vec![2.0, 4.0, 1.0, 0.0],
        );
        let b = bundle(
            "m-mkm",
            WeightSemantics::MicrodosimetricKinetic,
            vec![1.0, 2.0, 2.0, 0.0],
        );
        let report = compare_biological_models(&a, &b, &[], "cmp".into(), 0.0).unwrap();
        assert_eq!(report.regions.len(), 1);
        let all = &report.regions[0];
        assert_eq!(all.region, "all");
        assert!((all.mean - 7.0 / 4.0).abs() < 1e-12);
        assert!((all.other_mean - 5.0 / 4.0).abs() < 1e-12);
        assert!((all.mean_ratio.unwrap() - 1.4).abs() < 1e-12);
        assert_eq!(report.max_voxelwise_ratio, Some(2.0));
    }

    #[test]
    fn mismatched_provenance_rejected() {
        let mut a = bundle("m-cbe", WeightSemantics::PhotonIsoeffective, vec![1.0; 8]);
        a.physical_bundle_provenance = "phys-other".into();
        let b = bundle(
            "m-mkm",
            WeightSemantics::MicrodosimetricKinetic,
            vec![1.0; 8],
        );
        assert!(compare_biological_models(&a, &b, &[], "cmp".into(), 0.0).is_err());
    }

    #[test]
    fn significant_floor_excludes_noise_ratios() {
        let a = bundle(
            "m-cbe",
            WeightSemantics::PhotonIsoeffective,
            vec![1e-9, 2.0],
        );
        let b = bundle(
            "m-mkm",
            WeightSemantics::MicrodosimetricKinetic,
            vec![1e-12, 1.0],
        );
        // 1e-9/1e-12 is a 1000x ratio but below the floor — excluded.
        let report = compare_biological_models(&a, &b, &[], "cmp".into(), 1e-6).unwrap();
        assert_eq!(report.max_voxelwise_ratio, Some(2.0));
    }

    #[test]
    fn region_mask_resolves_subset_stats() {
        let a = bundle("m-cbe", WeightSemantics::PhotonIsoeffective, vec![2.0; 8]);
        let b = bundle(
            "m-mkm",
            WeightSemantics::MicrodosimetricKinetic,
            vec![4.0; 8],
        );
        let mask = RegionMask {
            name: "deep".into(),
            voxels: vec![false, false, false, false, true, true, true, true],
        };
        let report = compare_biological_models(&a, &b, &[mask], "cmp".into(), 0.0).unwrap();
        assert_eq!(report.regions.len(), 2);
        let deep = &report.regions[1];
        assert_eq!(deep.region, "deep");
        assert_eq!(deep.voxel_count, 4);
        assert_eq!(deep.mean_ratio, Some(0.5));
    }

    #[test]
    fn report_round_trips() {
        let a = bundle("m-cbe", WeightSemantics::PhotonIsoeffective, vec![1.0; 8]);
        let b = bundle(
            "m-mkm",
            WeightSemantics::MicrodosimetricKinetic,
            vec![1.0; 8],
        );
        let report = compare_biological_models(&a, &b, &[], "cmp".into(), 0.0).unwrap();
        let json = serde_json::to_vec(&report).unwrap();
        let back: BioModelComparison = serde_json::from_slice(&json).unwrap();
        back.validate().unwrap();
        assert_eq!(report, back);
    }
}
