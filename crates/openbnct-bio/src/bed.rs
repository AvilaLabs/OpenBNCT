// SPDX-License-Identifier: MIT

//! BED/EQD2 conversion of external photon/hadron dose (`openbnct.bed-bundle/0.1.0`)
//! and combined-treatment evaluation (`openbnct.combined-dose/0.1.0`).
//!
//! An [`ExternalDoseBundle`] states an absolute dose field and the
//! fractionation it was delivered in. [`bed_from_external`] turns it into a
//! BED or EQD2 field under a declared α/β (with optional per-region
//! overrides). [`combine_biological_doses`] then adds that EQD2 field to a
//! BNCT biological bundle — only when the quantities are provably
//! compatible (photon-isoeffective `weighted_eqd2` + `eqd2`), the cases
//! match, the grids co-register (possibly through an explicitly declared
//! resampling), and the operator states the additivity assumption.
//! Incompatible biological quantities are rejected, never silently added.

use std::collections::BTreeMap;

use openbnct_core::{
    ContentReference, ExternalDoseBundle, ExternalDoseQuantity, ExternalFractionation,
    GridGeometry, RegionMask, ResampleError, ResampleMethod, grid_geometry_equivalent,
    resample_trilinear,
};
use serde::{Deserialize, Serialize};

use crate::{BioError, BiologicalDoseBundle, WeightSemantics};

/// Current schema token for BED/EQD2 bundles.
pub const BED_BUNDLE_SCHEMA: &str = "openbnct.bed-bundle/0.1.0";
/// Current schema token for combined-dose evaluations.
pub const COMBINED_DOSE_SCHEMA: &str = "openbnct.combined-dose/0.1.0";

/// The biological quantity a [`BedBundle`] expresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BedQuantity {
    /// Biologically effective dose, `BED = Σ_f d_f·(1 + d_f/r)`.
    Bed,
    /// Equivalent dose in 2 Gy fractions, `EQD2 = BED / (1 + 2/r)`.
    Eqd2,
}

/// An external dose course converted to a biological quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BedBundle {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_of_reference_uid: Option<String>,
    pub geometry: GridGeometry,
    /// Whether this field is `bed` or `eqd2` — the label is part of the
    /// contract, so a downstream consumer can never misread the quantity.
    pub quantity: BedQuantity,
    /// The physical basis of the source dose (`physical` or `rbe_weighted`),
    /// carried through so a weighted field stays labelled.
    pub quantity_basis: ExternalDoseQuantity,
    /// α/β ratio (Gy) applied to unmasked voxels.
    pub alpha_beta: f64,
    /// Region α/β overrides that were applied, in application order.
    pub region_alpha_beta: BTreeMap<String, f64>,
    /// Fraction count used in the conversion.
    pub fractions: usize,
    pub values: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    /// Provenance of the external-dose bundle this field derives from.
    pub external_dose_provenance: String,
}

/// One input feeding a combined-dose evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombinedDoseInput {
    /// `bnct_biological` or `external_course`.
    pub role: String,
    /// Content hash of the exact artifact consumed.
    pub content: ContentReference,
    /// The input's own provenance chain (physical-bundle provenance for the
    /// BNCT side, external-dose provenance for the imported course).
    pub provenance_id: String,
}

/// A combined BNCT-plus-external-course biological evaluation.
///
/// The only combination currently admitted is EQD2 + EQD2: a
/// photon-isoeffective fractionated BNCT biological bundle plus an external
/// course converted to `eqd2`. The record states the additivity assumption
/// the operator declared — the library performs no equivalence judgement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombinedDoseBundle {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub geometry: GridGeometry,
    /// Combined quantity label; currently always `eqd2`.
    pub quantity: String,
    pub values: Vec<f64>,
    /// Independent-course uncertainty: `sqrt(σ_bnct² + σ_ext²)`; absent when
    /// either input lacks an uncertainty statement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    pub inputs: Vec<CombinedDoseInput>,
    /// Resampling applied to the external field, if any (`trilinear`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_resampling: Option<ResampleMethod>,
    /// Physical basis of the external course before BED conversion.
    pub external_quantity_basis: ExternalDoseQuantity,
    /// The operator-declared assumption under which the courses are added
    /// (for example full-repair additivity of EQD2). Required, non-empty.
    pub additivity_assumption: String,
    /// Research-status qualification; no clinical claim is made.
    pub qualification: String,
}

/// Convert an external dose course into a BED or EQD2 field.
///
/// `alpha_beta` (Gy) applies everywhere `region_alpha_beta` does not override
/// it; each named region requires a same-length [`RegionMask`] in `regions`,
/// mirroring [`apply_biological_model`](crate::apply_biological_model).
/// First-order uncertainty propagation treats the declared total sigma as
/// scaling with dose: for explicit fractions `d(BED)/dD = 1 + 2·Σd_f²/(D·r)`
/// (uniform: `1 + 2D/(n·r)`), divided by `1 + 2/r` for EQD2.
pub fn bed_from_external(
    dose: &ExternalDoseBundle,
    alpha_beta: f64,
    region_alpha_beta: &BTreeMap<String, f64>,
    regions: &[RegionMask],
    quantity: BedQuantity,
    region_priority: &[String],
) -> Result<BedBundle, BioError> {
    if !(alpha_beta.is_finite() && alpha_beta > 0.0) {
        return Err(BioError::Invalid(format!(
            "alpha_beta {alpha_beta} must be finite and positive"
        )));
    }
    let voxel_count = dose
        .geometry
        .voxel_count()
        .map_err(|e| BioError::Invalid(format!("external-dose geometry: {e}")))?;
    if dose.values.len() != voxel_count {
        return Err(BioError::Invalid(format!(
            "external dose has {} values for a {voxel_count}-voxel grid",
            dose.values.len()
        )));
    }
    let masks: BTreeMap<&str, &Vec<bool>> = regions
        .iter()
        .map(|mask| (mask.name.as_str(), &mask.voxels))
        .collect();
    for (name, ratio) in region_alpha_beta {
        if !(ratio.is_finite() && *ratio > 0.0) {
            return Err(BioError::Invalid(format!(
                "region {name} alpha_beta {ratio} must be finite and positive"
            )));
        }
        let mask = masks
            .get(name.as_str())
            .ok_or_else(|| BioError::Invalid(format!("bed region {name} has no supplied mask")))?;
        if mask.len() != voxel_count {
            return Err(BioError::Invalid(format!(
                "bed mask {name} covers {} voxels, grid needs {voxel_count}",
                mask.len()
            )));
        }
    }
    let ab_order = crate::ordered_region_names(region_alpha_beta.keys(), region_priority);
    let ab_assignment = crate::resolve_regions(
        &ab_order,
        &masks,
        region_priority,
        voxel_count,
        "region_alpha_beta",
    )?;
    let ratio_of = |voxel: usize| -> f64 {
        ab_assignment[voxel].map_or(alpha_beta, |i| region_alpha_beta[&ab_order[i]])
    };

    let fractions = dose.fraction_count();
    // Per-voxel Σ d_f² — precomputed for the explicit sigma derivative.
    let mut bed = vec![0.0; voxel_count];
    for voxel in 0..voxel_count {
        let r = ratio_of(voxel);
        match &dose.fractionation {
            ExternalFractionation::Uniform { count } => {
                let d = dose.values[voxel] / f64::from(*count);
                bed[voxel] = f64::from(*count) * d * (1.0 + d / r);
            }
            ExternalFractionation::Explicit { doses } => {
                bed[voxel] = doses.iter().map(|d| d[voxel] * (1.0 + d[voxel] / r)).sum();
            }
        }
        if let BedQuantity::Eqd2 = quantity {
            bed[voxel] /= 1.0 + 2.0 / r;
        }
    }

    let sigma = dose.absolute_standard_uncertainty.as_ref().map(|s| {
        s.iter()
            .enumerate()
            .map(|(voxel, &sigma)| {
                let r = ratio_of(voxel);
                let total = dose.values[voxel];
                // d(BED)/dD under proportional per-fraction scaling.
                let derivative = match &dose.fractionation {
                    ExternalFractionation::Uniform { count } => {
                        1.0 + 2.0 * total / (f64::from(*count) * r)
                    }
                    ExternalFractionation::Explicit { doses } => {
                        if total > 0.0 {
                            let s2: f64 = doses.iter().map(|d| d[voxel] * d[voxel]).sum();
                            1.0 + 2.0 * s2 / (total * r)
                        } else {
                            1.0
                        }
                    }
                };
                let derivative = match quantity {
                    BedQuantity::Bed => derivative,
                    BedQuantity::Eqd2 => derivative / (1.0 + 2.0 / r),
                };
                sigma * derivative
            })
            .collect()
    });

    Ok(BedBundle {
        schema_version: BED_BUNDLE_SCHEMA.into(),
        case_id: dose.case_id.clone(),
        frame_of_reference_uid: dose.frame_of_reference_uid.clone(),
        geometry: dose.geometry.clone(),
        quantity,
        quantity_basis: dose.quantity,
        alpha_beta,
        region_alpha_beta: region_alpha_beta.clone(),
        fractions,
        values: bed,
        absolute_standard_uncertainty: sigma,
        external_dose_provenance: dose.provenance_id.clone(),
    })
}

/// Add an external EQD2 course to a photon-isoeffective BNCT EQD2 bundle.
///
/// Compatibility gates, all enforced before any arithmetic:
///
/// - `primary` must be `photon_isoeffective` with a `weighted_eqd2` total and
///   `external.quantity` must be `eqd2` — anything else is a different
///   biological quantity and cannot be added.
/// - `case_id` must match.
/// - Geometries must be equivalent, or `resample = Some(trilinear)`
///   co-registers the external field onto the primary grid (strict
///   coverage — uncovered targets reject).
/// - `assumption` must be a non-empty operator statement of the additivity
///   basis; it is recorded verbatim in the output.
///
/// `primary_ref`/`external_ref` bind the exact input artifacts by content
/// hash so the combined record is auditable.
pub fn combine_biological_doses(
    primary: &BiologicalDoseBundle,
    external: &BedBundle,
    primary_ref: ContentReference,
    external_ref: ContentReference,
    resample: Option<ResampleMethod>,
    assumption: &str,
) -> Result<CombinedDoseBundle, BioError> {
    if primary.weight_semantics != WeightSemantics::PhotonIsoeffective
        || primary.total.unit != "weighted_eqd2"
        || external.quantity != BedQuantity::Eqd2
    {
        return Err(BioError::Invalid(
            "only photon-isoeffective weighted_eqd2 (BNCT) + eqd2 (external) can be combined; \
             convert both courses to the same biological quantity first"
                .into(),
        ));
    }
    if primary.case_id != external.case_id {
        return Err(BioError::Invalid(format!(
            "case_id mismatch: {:?} vs {:?}",
            primary.case_id, external.case_id
        )));
    }
    if assumption.trim().is_empty() {
        return Err(BioError::Invalid(
            "an additivity assumption must be declared for a combined evaluation".into(),
        ));
    }
    let voxel_count = primary
        .geometry
        .voxel_count()
        .map_err(|e| BioError::Invalid(format!("primary geometry: {e}")))?;

    // Co-register the external field onto the primary grid.
    let (external_values, external_sigma) =
        if grid_geometry_equivalent(&primary.geometry, &external.geometry) {
            (
                external.values.clone(),
                external.absolute_standard_uncertainty.clone(),
            )
        } else {
            match resample {
                Some(ResampleMethod::Trilinear) => (
                    resample_trilinear(&external.values, &external.geometry, &primary.geometry)
                        .map_err(resample_err)?,
                    external
                        .absolute_standard_uncertainty
                        .as_ref()
                        .map(|s| resample_trilinear(s, &external.geometry, &primary.geometry))
                        .transpose()
                        .map_err(resample_err)?,
                ),
                None => {
                    return Err(BioError::Invalid(
                        "grids differ; declare --resample trilinear to co-register the external \
                     field onto the primary grid"
                            .into(),
                    ));
                }
            }
        };
    if external_values.len() != voxel_count {
        return Err(BioError::Invalid(format!(
            "external field has {} values for a {voxel_count}-voxel grid",
            external_values.len()
        )));
    }

    let mut values = primary.total.values.clone();
    for (voxel, value) in values.iter_mut().enumerate() {
        *value += external_values[voxel];
    }
    let sigma = match (
        &primary.total.absolute_standard_uncertainty,
        &external_sigma,
    ) {
        (Some(a), Some(b)) => Some(
            a.iter()
                .zip(b)
                .map(|(x, y)| (x * x + y * y).sqrt())
                .collect(),
        ),
        _ => None,
    };

    Ok(CombinedDoseBundle {
        schema_version: COMBINED_DOSE_SCHEMA.into(),
        case_id: primary.case_id.clone(),
        geometry: primary.geometry.clone(),
        quantity: "eqd2".into(),
        values,
        absolute_standard_uncertainty: sigma,
        inputs: vec![
            CombinedDoseInput {
                role: "bnct_biological".into(),
                content: primary_ref,
                provenance_id: format!(
                    "{} (model sha256:{})",
                    primary.physical_bundle_provenance, primary.model.sha256
                ),
            },
            CombinedDoseInput {
                role: "external_course".into(),
                content: external_ref,
                provenance_id: external.external_dose_provenance.clone(),
            },
        ],
        external_resampling: if grid_geometry_equivalent(&primary.geometry, &external.geometry) {
            None
        } else {
            resample
        },
        external_quantity_basis: external.quantity_basis,
        additivity_assumption: assumption.to_string(),
        qualification: "research combined-treatment evaluation; additive EQD2 under the declared \
                        assumption — no clinical, equivalence, or commissioning claim"
            .into(),
    })
}

fn resample_err(error: ResampleError) -> BioError {
    BioError::Invalid(format!("external-field resampling: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::ExternalProducer;

    fn grid() -> GridGeometry {
        GridGeometry {
            shape: [2, 1, 1],
            spacing_mm: [5.0, 5.0, 5.0],
            origin_mm: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn external(values: Vec<f64>, fractionation: ExternalFractionation) -> ExternalDoseBundle {
        ExternalDoseBundle {
            schema_version: openbnct_core::EXTERNAL_DOSE_SCHEMA.into(),
            case_id: "case-1".into(),
            frame_of_reference_uid: Some("1.2.3".into()),
            geometry: grid(),
            producer: ExternalProducer {
                system: "phits".into(),
                version: "3.34".into(),
                normalization: "prescribed".into(),
            },
            quantity: ExternalDoseQuantity::Physical,
            values,
            absolute_standard_uncertainty: Some(vec![0.1, 0.1]),
            fractionation,
            provenance_id: "external-dose:phits:sha256:deadbeef".into(),
        }
    }

    #[test]
    fn uniform_fraction_bed_and_eqd2_match_hand_values() {
        // D=60 Gy in 30 fx, r=10: d=2, BED=30·2·1.2=72, EQD2=72/1.2=60.
        let dose = external(
            vec![60.0, 60.0],
            ExternalFractionation::Uniform { count: 30 },
        );
        let bed =
            bed_from_external(&dose, 10.0, &BTreeMap::new(), &[], BedQuantity::Bed, &[]).unwrap();
        assert!((bed.values[0] - 72.0).abs() < 1e-9);
        // σ_BED = σ_D · (1 + 2D/(nr)) = 0.1 · (1 + 0.4) = 0.14.
        assert!((bed.absolute_standard_uncertainty.as_ref().unwrap()[0] - 0.14).abs() < 1e-9);
        let eqd2 =
            bed_from_external(&dose, 10.0, &BTreeMap::new(), &[], BedQuantity::Eqd2, &[]).unwrap();
        assert!((eqd2.values[0] - 60.0).abs() < 1e-9);
        assert_eq!(eqd2.fractions, 30);
        assert_eq!(
            eqd2.external_dose_provenance,
            "external-dose:phits:sha256:deadbeef"
        );
    }

    #[test]
    fn explicit_fractions_sum_each_term() {
        // d = [1, 2, 3] Gy, r=3: BED = 1·(1+1/3) + 2·(1+2/3) + 3·(1+1) = 32/3.
        let dose = external(
            vec![6.0, 6.0],
            ExternalFractionation::Explicit {
                doses: vec![vec![1.0, 1.0], vec![2.0, 2.0], vec![3.0, 3.0]],
            },
        );
        let bed =
            bed_from_external(&dose, 3.0, &BTreeMap::new(), &[], BedQuantity::Bed, &[]).unwrap();
        assert!((bed.values[0] - 32.0 / 3.0).abs() < 1e-9);
        let eqd2 =
            bed_from_external(&dose, 3.0, &BTreeMap::new(), &[], BedQuantity::Eqd2, &[]).unwrap();
        assert!((eqd2.values[0] - 6.4).abs() < 1e-9);
        // σ: d(BED)/dD = 1 + 2·(1+4+9)/(6·3) = 1 + 14/9 ≈ 2.5556; EQD2 /(5/3).
        let sigma = eqd2.absolute_standard_uncertainty.unwrap()[0];
        assert!((sigma - 0.1 * (1.0 + 14.0 / 9.0) / (5.0 / 3.0)).abs() < 1e-9);
    }

    #[test]
    fn region_alpha_beta_overrides_inside_mask() {
        let dose = external(
            vec![30.0, 30.0],
            ExternalFractionation::Uniform { count: 10 },
        );
        let mut overrides = BTreeMap::new();
        overrides.insert("tumor".to_string(), 4.0);
        let mask = RegionMask {
            name: "tumor".into(),
            voxels: vec![true, false],
        };
        let eqd2 =
            bed_from_external(&dose, 10.0, &overrides, &[mask], BedQuantity::Eqd2, &[]).unwrap();
        // Voxel 0: r=4 → BED=10·3·(1+0.75)=52.5, EQD2=52.5/1.5=35.
        assert!((eqd2.values[0] - 35.0).abs() < 1e-9);
        // Voxel 1: r=10 → BED=10·3·1.3=39, EQD2=39/1.2=32.5.
        assert!((eqd2.values[1] - 32.5).abs() < 1e-9);
    }

    #[test]
    fn missing_region_mask_and_bad_alpha_beta_reject() {
        let dose = external(vec![1.0], ExternalFractionation::Uniform { count: 1 });
        let mut overrides = BTreeMap::new();
        overrides.insert("tumor".to_string(), 4.0);
        assert!(bed_from_external(&dose, 10.0, &overrides, &[], BedQuantity::Bed, &[]).is_err());
        assert!(
            bed_from_external(&dose, 0.0, &BTreeMap::new(), &[], BedQuantity::Bed, &[]).is_err()
        );
    }

    fn bnct_bundle() -> BiologicalDoseBundle {
        let mut weights = BTreeMap::new();
        weights.insert("photon".to_string(), 1.0);
        BiologicalDoseBundle {
            schema_version: crate::BIOLOGICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "case-1".into(),
            geometry: grid(),
            physical_bundle_provenance: "openmc-run".into(),
            model: ContentReference {
                id: "m".into(),
                sha256: "00".repeat(32),
            },
            weight_semantics: WeightSemantics::PhotonIsoeffective,
            unit: "weighted_gray_per_source_particle".into(),
            components: vec![],
            fractionation: None,
            total: crate::BiologicalTotal {
                unit: "weighted_eqd2".into(),
                values: vec![35.0, 32.5],
                absolute_standard_uncertainty: Some(vec![0.3, 0.3]),
                uncertainty_method: crate::BiologicalUncertaintyMethod::CorrelatedComponentSum,
            },
            regions_applied: vec![],
            qualification: "research".into(),
            microdosimetry: None,
            isoeffective: None,
        }
    }

    fn reference(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "11".repeat(32),
        }
    }

    #[test]
    fn combine_adds_eqd2_fields_with_quadrature_sigma() {
        let primary = bnct_bundle();
        let ext = bed_from_external(
            &external(
                vec![60.0, 60.0],
                ExternalFractionation::Uniform { count: 30 },
            ),
            10.0,
            &BTreeMap::new(),
            &[],
            BedQuantity::Eqd2,
            &[],
        )
        .unwrap();
        let combined = combine_biological_doses(
            &primary,
            &ext,
            reference("bio.json"),
            reference("bed.json"),
            None,
            "full-repair additive EQD2",
        )
        .unwrap();
        assert_eq!(combined.quantity, "eqd2");
        assert!((combined.values[0] - 95.0).abs() < 1e-9);
        // sqrt(0.3² + 0.1167²) — EQD2 sigma = 0.1·1.4/1.2 = 0.11667.
        let expected = (0.3_f64.powi(2) + (0.1_f64 * 1.4 / 1.2).powi(2)).sqrt();
        assert!(
            (combined.absolute_standard_uncertainty.as_ref().unwrap()[0] - expected).abs() < 1e-9
        );
        assert_eq!(combined.inputs.len(), 2);
        assert_eq!(combined.external_resampling, None);
    }

    #[test]
    fn combine_rejects_incompatible_quantities() {
        let mut primary = bnct_bundle();
        let ext = bed_from_external(
            &external(
                vec![60.0, 60.0],
                ExternalFractionation::Uniform { count: 30 },
            ),
            10.0,
            &BTreeMap::new(),
            &[],
            BedQuantity::Eqd2,
            &[],
        )
        .unwrap();
        // Fixed-per-component weighting is not photon-isoeffective.
        primary.weight_semantics = WeightSemantics::FixedPerComponent;
        assert!(
            combine_biological_doses(&primary, &ext, reference("a"), reference("b"), None, "x")
                .is_err()
        );
        primary.weight_semantics = WeightSemantics::PhotonIsoeffective;
        // A BED (not EQD2) external field cannot be added.
        let bed_only = bed_from_external(
            &external(
                vec![60.0, 60.0],
                ExternalFractionation::Uniform { count: 30 },
            ),
            10.0,
            &BTreeMap::new(),
            &[],
            BedQuantity::Bed,
            &[],
        )
        .unwrap();
        assert!(
            combine_biological_doses(
                &primary,
                &bed_only,
                reference("a"),
                reference("b"),
                None,
                "x"
            )
            .is_err()
        );
        // Missing assumption rejects.
        assert!(
            combine_biological_doses(&primary, &ext, reference("a"), reference("b"), None, "  ")
                .is_err()
        );
        // Case mismatch rejects.
        let mut wrong_case = ext.clone();
        wrong_case.case_id = "other".into();
        assert!(
            combine_biological_doses(
                &primary,
                &wrong_case,
                reference("a"),
                reference("b"),
                None,
                "x"
            )
            .is_err()
        );
    }

    #[test]
    fn combine_resamples_external_field_when_declared() {
        let primary = bnct_bundle();
        // External grid at half spacing covering the same extent.
        let mut dose4 = external(vec![60.0; 4], ExternalFractionation::Uniform { count: 30 });
        dose4.geometry = GridGeometry {
            shape: [4, 1, 1],
            spacing_mm: [2.5, 5.0, 5.0],
            ..grid()
        };
        dose4.absolute_standard_uncertainty = Some(vec![0.1; 4]);
        let ext =
            bed_from_external(&dose4, 10.0, &BTreeMap::new(), &[], BedQuantity::Eqd2, &[]).unwrap();
        // Without --resample the mismatched grid rejects.
        assert!(
            combine_biological_doses(&primary, &ext, reference("a"), reference("b"), None, "x")
                .is_err()
        );
        let combined = combine_biological_doses(
            &primary,
            &ext,
            reference("a"),
            reference("b"),
            Some(ResampleMethod::Trilinear),
            "full-repair additive EQD2",
        )
        .unwrap();
        // A constant field resamples to itself.
        assert!((combined.values[0] - 95.0).abs() < 1e-9);
        assert_eq!(
            combined.external_resampling,
            Some(ResampleMethod::Trilinear)
        );
    }
}
