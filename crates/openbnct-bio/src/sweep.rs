// SPDX-License-Identifier: Apache-2.0

//! One-parameter sensitivity sweeps over a biological model.
//!
//! `run_sweep` varies one declared parameter — a component weight, a
//! per-region weight override, or a fractionation quantity — across an
//! explicit value list, re-validating and re-applying the model at each
//! point. Each point records the masked min/mean/max of the resulting
//! biological total over a named region, so the sweep answers "how does
//! the region-integrated result respond to this parameter" without
//! materializing a bundle per point. The record binds the exact model
//! document and dose bundle by SHA-256.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use openbnct_core::{ContentReference, PhysicalDoseBundle, RegionMask, masked_values, mean};

use crate::{BioError, BiologicalModel, apply_biological_model};

/// Schema token for the sensitivity-sweep record.
pub const BIO_SWEEP_SCHEMA: &str = "openbnct.bio-sensitivity-sweep/0.1.0";

/// One sweepable model parameter, addressed by a compact spec string:
/// `component:<name>`, `region_weight:<region>:<component>`,
/// `alpha_beta:default`, `alpha_beta:<region>`, `fraction_count`, or
/// `source_particles_per_fraction`.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepParameter {
    /// A default component weight (`component:boron`).
    ComponentWeight(String),
    /// A per-region override weight (`region_weight:margin:boron`).
    RegionWeight {
        /// Region whose override map carries the weight.
        region: String,
        /// Dose component name.
        component: String,
    },
    /// The LQ α/β for a named region or the `default` fallback.
    AlphaBeta(String),
    /// The number of identical fractions; sweep values must be positive
    /// integers.
    FractionCount,
    /// Source particles per fraction.
    SourceParticlesPerFraction,
}

impl SweepParameter {
    /// Parse a sweep spec like `component:boron` or `alpha_beta:tumor`.
    pub fn parse(spec: &str) -> Result<Self, BioError> {
        let invalid = || BioError::Invalid(format!("unknown sweep parameter {spec:?}"));
        let parts: Vec<&str> = spec.split(':').collect();
        match parts.as_slice() {
            ["component", name] => Ok(Self::ComponentWeight((*name).to_owned())),
            ["region_weight", region, component] => Ok(Self::RegionWeight {
                region: (*region).to_owned(),
                component: (*component).to_owned(),
            }),
            ["alpha_beta", region] => Ok(Self::AlphaBeta((*region).to_owned())),
            ["fraction_count"] => Ok(Self::FractionCount),
            ["source_particles_per_fraction"] => Ok(Self::SourceParticlesPerFraction),
            _ => Err(invalid()),
        }
    }

    /// The canonical label recorded on the sweep.
    pub fn label(&self) -> String {
        match self {
            Self::ComponentWeight(name) => format!("component:{name}"),
            Self::RegionWeight { region, component } => {
                format!("region_weight:{region}:{component}")
            }
            Self::AlphaBeta(region) => format!("alpha_beta:{region}"),
            Self::FractionCount => "fraction_count".into(),
            Self::SourceParticlesPerFraction => "source_particles_per_fraction".into(),
        }
    }

    /// Apply `value` to `model`, or reject when the model does not carry
    /// the parameter (e.g. `alpha_beta:*` without a fractionation block).
    fn set(&self, model: &mut BiologicalModel, value: f64) -> Result<(), BioError> {
        if !value.is_finite() {
            return Err(BioError::Invalid(format!(
                "sweep value {value} is not finite"
            )));
        }
        match self {
            Self::ComponentWeight(name) => {
                let weight = model
                    .component_weights
                    .get_mut(name.as_str())
                    .ok_or_else(|| BioError::Invalid(format!("model has no component {name:?}")))?;
                *weight = value;
            }
            Self::RegionWeight { region, component } => {
                let weights = model.region_weights.get_mut(region).ok_or_else(|| {
                    BioError::Invalid(format!("model has no region weight map {region:?}"))
                })?;
                let weight = weights.get_mut(component.as_str()).ok_or_else(|| {
                    BioError::Invalid(format!("region {region:?} has no component {component:?}"))
                })?;
                *weight = value;
            }
            Self::AlphaBeta(region) => {
                let fractionation = model.fractionation.as_mut().ok_or_else(|| {
                    BioError::Invalid("model declares no fractionation block".into())
                })?;
                if region == "default" {
                    fractionation.default_alpha_beta = value;
                } else {
                    let target =
                        fractionation
                            .region_alpha_beta
                            .get_mut(region)
                            .ok_or_else(|| {
                                BioError::Invalid(format!(
                                    "model declares no alpha_beta override for {region:?}"
                                ))
                            })?;
                    *target = value;
                }
            }
            Self::FractionCount => {
                if value < 1.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
                    return Err(BioError::Invalid(format!(
                        "fraction_count sweep values must be positive integers, observed {value}"
                    )));
                }
                let fractionation = model.fractionation.as_mut().ok_or_else(|| {
                    BioError::Invalid("model declares no fractionation block".into())
                })?;
                fractionation.fraction_count = value as u32;
            }
            Self::SourceParticlesPerFraction => {
                let fractionation = model.fractionation.as_mut().ok_or_else(|| {
                    BioError::Invalid("model declares no fractionation block".into())
                })?;
                fractionation.source_particles_per_fraction = value;
            }
        }
        Ok(())
    }
}

/// The region-masked summary of the biological total at one parameter
/// value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepPoint {
    /// The parameter value applied at this point.
    pub value: f64,
    pub region_voxel_count: u64,
    pub minimum: f64,
    pub mean: f64,
    pub maximum: f64,
}

/// A one-parameter sensitivity record over a biological model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitivitySweep {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// Region the totals were masked to.
    pub region: String,
    /// Canonical parameter label (`component:boron`, ...).
    pub parameter: String,
    /// The total quantity scored (`biological_total` or `weighted_eqd2`).
    pub quantity: String,
    /// Unit label of the scored total.
    pub unit: String,
    /// Content binding of the exact model JSON swept.
    pub model: ContentReference,
    /// Content binding of the physical dose bundle scored.
    pub dose_bundle: ContentReference,
    pub points: Vec<SweepPoint>,
    pub qualification: String,
}

impl SensitivitySweep {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, BIO_SWEEP_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.case_id.trim().is_empty() || self.region.trim().is_empty() {
            return Err(BioError::Invalid(
                "sweep case_id and region must be non-empty".into(),
            ));
        }
        if self.points.is_empty() {
            return Err(BioError::Invalid("sweep carries no points".into()));
        }
        for point in &self.points {
            let finite = [point.value, point.minimum, point.mean, point.maximum]
                .iter()
                .all(|v| v.is_finite());
            if !finite {
                return Err(BioError::Invalid(
                    "sweep point values must be finite".into(),
                ));
            }
        }
        self.model
            .validate()
            .map_err(|_| BioError::Invalid("model reference is invalid".into()))?;
        self.dose_bundle
            .validate()
            .map_err(|_| BioError::Invalid("dose bundle reference is invalid".into()))?;
        Ok(())
    }
}

/// Run a one-parameter sensitivity sweep.
///
/// `region` names the mask in `regions` that the biological total is
/// summarized over at each point; it must exist. `values` is the explicit
/// parameter list — at least one finite value. The model is re-validated
/// after each parameter set, so a swept value that breaks the model (a
/// photon weight ≠ 1 under `photon_isoeffective`, a negative weight) is a
/// point-level failure reported honestly.
#[allow(clippy::too_many_arguments)]
pub fn run_sweep(
    model: &BiologicalModel,
    model_bytes: &[u8],
    physical: &PhysicalDoseBundle,
    physical_sha256: &str,
    regions: &[RegionMask],
    region: &str,
    parameter: &SweepParameter,
    values: &[f64],
) -> Result<SensitivitySweep, BioError> {
    if values.is_empty() {
        return Err(BioError::Invalid(
            "sweep requires at least one value".into(),
        ));
    }
    let mask = regions
        .iter()
        .find(|mask| mask.name == region)
        .ok_or_else(|| BioError::Invalid(format!("no supplied mask named {region:?}")))?;
    model.validate()?;
    physical
        .validate()
        .map_err(|e| BioError::Invalid(format!("dose bundle: {e}")))?;

    let mut points = Vec::with_capacity(values.len());
    let mut quantity = String::new();
    let mut unit = String::new();
    for &value in values {
        let mut varied = model.clone();
        parameter.set(&mut varied, value)?;
        varied.validate()?;
        // Bind each intermediate bundle to its own varied document bytes —
        // the base model hash would misstate the parameterization applied.
        let varied_bytes = serde_json::to_vec(&varied)
            .map_err(|e| BioError::Invalid(format!("model serialization: {e}")))?;
        let bundle = apply_biological_model(&varied, &varied_bytes, physical, regions)?;
        let selected = masked_values(region, &bundle.total.values, &mask.voxels)
            .map_err(|e| BioError::Invalid(format!("dose selection: {e}")))?;
        quantity = if bundle.fractionation.is_some() {
            "weighted_eqd2".into()
        } else {
            "biological_total".into()
        };
        unit = bundle.total.unit.clone();
        points.push(SweepPoint {
            value,
            region_voxel_count: selected.len() as u64,
            minimum: selected.iter().copied().fold(f64::INFINITY, f64::min),
            mean: mean(&selected),
            maximum: selected.iter().copied().fold(0.0, f64::max),
        });
    }

    let sweep = SensitivitySweep {
        schema_version: BIO_SWEEP_SCHEMA.into(),
        case_id: physical.case_id.clone(),
        region: region.into(),
        parameter: parameter.label(),
        quantity,
        unit,
        model: ContentReference {
            id: model.id.clone(),
            sha256: format!("{:x}", Sha256::digest(model_bytes)),
        },
        dose_bundle: ContentReference {
            id: format!("{}.physical-dose-bundle", physical.case_id),
            sha256: physical_sha256.into(),
        },
        points,
        qualification: "synthetic_research_only_not_clinical".into(),
    };
    sweep.validate()?;
    Ok(sweep)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openbnct_core::{
        DoseComponent, DoseUnit, DoseVolume, GridGeometry, PhysicalTotalDoseVolume,
        TotalUncertaintyMethod,
    };

    use super::*;
    use crate::{Fractionation, WeightMap, WeightSemantics};

    fn physical_bundle() -> PhysicalDoseBundle {
        let reference = |id: &str| ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        };
        let components = [
            (DoseComponent::Boron, 1.0e-12, 1.0e-14),
            (DoseComponent::Nitrogen, 2.0e-13, 2.0e-15),
            (DoseComponent::Hydrogen, 5.0e-14, 5.0e-16),
            (DoseComponent::Photon, 3.0e-13, 3.0e-15),
        ]
        .into_iter()
        .map(|(component, mean, sigma)| DoseVolume {
            component,
            unit: DoseUnit::GrayPerSourceParticle,
            values: vec![mean, mean],
            absolute_standard_uncertainty: Some(vec![sigma, sigma]),
        })
        .collect();
        PhysicalDoseBundle {
            schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "synthetic-case".into(),
            frame_of_reference_uid: None,
            geometry: GridGeometry {
                shape: [2, 1, 1],
                spacing_mm: [5.0; 3],
                origin_mm: [-2.5; 3],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            component_profile: reference("profile"),
            response_set: reference("response"),
            components,
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values: vec![1.75e-12, 1.75e-12],
                absolute_standard_uncertainty: Some(vec![1.1e-14, 1.1e-14]),
                uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
            },
            provenance_id: "test-provenance".into(),
        }
    }

    fn model() -> BiologicalModel {
        let weights = WeightMap::from([
            ("boron".into(), 3.8),
            ("nitrogen".into(), 2.5),
            ("hydrogen".into(), 1.0),
            ("photon".into(), 1.0),
        ]);
        BiologicalModel {
            region_priority: Vec::new(),
            component_weight_uncertainty: BTreeMap::new(),
            schema_version: crate::BIOLOGICAL_MODEL_SCHEMA.into(),
            id: "test.model.v1".into(),
            weight_semantics: WeightSemantics::FixedPerComponent,
            input_unit: DoseUnit::GrayPerSourceParticle,
            component_weights: weights,
            region_weights: BTreeMap::new(),
            derivation: None,
            validity_domain: None,
            fractionation: None,
        }
    }

    fn mask() -> RegionMask {
        RegionMask {
            name: "all".into(),
            voxels: vec![true, true],
        }
    }

    #[test]
    fn component_weight_sweep_scales_the_total_linearly() {
        let model = model();
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        // boron weight 0 -> total = N 2e-13*2.5 + H 5e-14 + P 3e-13
        // = 8.5e-13; weight 3.8 -> 3.8e-12 + 8.5e-13 = 4.65e-12.
        let sweep = run_sweep(
            &model,
            &bytes,
            &physical_bundle(),
            &"b".repeat(64),
            &[mask()],
            "all",
            &SweepParameter::parse("component:boron").unwrap(),
            &[0.0, 3.8],
        )
        .unwrap();
        assert_eq!(sweep.parameter, "component:boron");
        assert_eq!(sweep.quantity, "biological_total");
        assert_eq!(sweep.points.len(), 2);
        assert_eq!(sweep.points[0].mean, 8.5e-13);
        assert_eq!(sweep.points[1].mean, 4.65e-12);
        sweep.validate().unwrap();
    }

    #[test]
    fn fraction_count_sweep_marks_eqd2_and_rejects_noninteger() {
        let mut model = model();
        model.fractionation = Some(Fractionation {
            fraction_count: 10,
            source_particles_per_fraction: 1.0e10,
            default_alpha_beta: 3.0,
            region_alpha_beta: BTreeMap::new(),
        });
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let sweep = run_sweep(
            &model,
            &bytes,
            &physical_bundle(),
            &"b".repeat(64),
            &[mask()],
            "all",
            &SweepParameter::FractionCount,
            &[1.0, 10.0],
        )
        .unwrap();
        assert_eq!(sweep.quantity, "weighted_eqd2");
        assert!(sweep.points[0].mean > sweep.points[1].mean * 0.0);
        assert!(sweep.points.iter().all(|p| p.mean.is_finite()));

        let error = run_sweep(
            &model,
            &bytes,
            &physical_bundle(),
            &"b".repeat(64),
            &[mask()],
            "all",
            &SweepParameter::FractionCount,
            &[2.5],
        )
        .expect_err("non-integer fraction count must reject");
        assert!(matches!(error, BioError::Invalid(_)));
    }

    #[test]
    fn sweep_rejects_unknown_parameters_and_missing_masks() {
        let model = model();
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        assert!(SweepParameter::parse("bogus").is_err());
        let error = run_sweep(
            &model,
            &bytes,
            &physical_bundle(),
            &"b".repeat(64),
            &[mask()],
            "all",
            &SweepParameter::FractionCount,
            &[2.0],
        )
        .expect_err("fraction_count without fractionation must reject");
        assert!(matches!(error, BioError::Invalid(_)));

        let error = run_sweep(
            &model,
            &bytes,
            &physical_bundle(),
            &"b".repeat(64),
            &[mask()],
            "nonexistent-region",
            &SweepParameter::ComponentWeight("boron".into()),
            &[1.0],
        )
        .expect_err("missing region must reject");
        assert!(matches!(error, BioError::Invalid(_)));
    }
}
