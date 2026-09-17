// SPDX-License-Identifier: Apache-2.0

//! Biological dose interpretation layer.
//!
//! A `BiologicalModel` is a separately versioned research artifact that
//! assigns dimensionless effectiveness weights to the four physical dose
//! components, optionally overridden per named region. Applying it to a
//! `PhysicalDoseBundle` produces a `BiologicalDoseBundle` whose weighted
//! values never alias physical dose: the bundle carries its own schema, unit
//! label, and qualification boundary, and the physical bundle remains
//! separately inspectable.
//!
//! This is a research-only modeling layer. It does not assert clinical CBE,
//! RBE, or Gy-Eq values for any real treatment, and the benchmark
//! specification's exclusion of weighted dose from NF-BNCT-001 is preserved:
//! biological bundles are produced only when a model artifact is supplied.

#![forbid(unsafe_code)]

mod bed;
mod endpoint;
mod lineal_tally;
mod mkm;
mod sweep;

use std::collections::{BTreeMap, BTreeSet};

use openbnct_core::{ContentReference, DoseComponent, DoseUnit, GridGeometry, PhysicalDoseBundle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub use bed::{
    BED_BUNDLE_SCHEMA, BedBundle, BedQuantity, COMBINED_DOSE_SCHEMA, CombinedDoseBundle,
    CombinedDoseInput, bed_from_external, combine_biological_doses,
};
pub use endpoint::{
    AppliedDoseStatistic, DoseStatistic, ENDPOINT_EVALUATION_SCHEMA, ENDPOINT_MODEL_SCHEMA,
    EndpointEvaluation, EndpointFunction, EndpointKind, EndpointModel, EvaluatedEndpoint,
    UtcpCombination, UtcpComponents, combine_utcp, evaluate_endpoint,
};
pub use lineal_tally::{
    LINEAL_TALLY_SPEC_SCHEMA, LinealComponent, LinealTallyError, LinealTallySpec,
    compute_lineal_spectrum,
};
pub use mkm::{
    LINEAL_SPECTRUM_SCHEMA, LinealEnergySource, LinealSpectrum, LinealWeighting,
    MICRODOSIMETRIC_MODEL_SCHEMA, MicrodosimetricModel, MkmApplied, MkmComponent, MkmLq,
    SpectrumInput, apply_microdosimetric_model, domain_mean_specific_energy_gy,
};
pub use sweep::{BIO_SWEEP_SCHEMA, SensitivitySweep, SweepParameter, SweepPoint, run_sweep};

pub const BIOLOGICAL_MODEL_SCHEMA: &str = "openbnct.biological-model/0.2.0";
pub const BIOLOGICAL_DOSE_BUNDLE_SCHEMA: &str = "openbnct.biological-dose-bundle/0.2.0";

/// Dimensionless effectiveness weight applied to one physical component.
pub type WeightMap = BTreeMap<String, f64>;

/// Weight semantics currently supported. `fixed_per_component` applies one
/// constant weight per dose component; per-region overrides replace the
/// default weight inside a named region mask. `photon_isoeffective`
/// declares the weights are photon-isoeffect factors (RBE/CBE relative to
/// the photon component), which requires every photon weight to be exactly
/// 1.0 so the weighted sum is expressed in photon-equivalent dose.
/// `microdosimetric_kinetic` marks bundles produced by a
/// `openbnct.microdosimetric-model/0.1.0` MKM artifact — a separate model
/// family whose component volumes are nonlinear photon-equivalent doses,
/// not weighted ones; it cannot be declared on a weight-based
/// `BiologicalModel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightSemantics {
    FixedPerComponent,
    PhotonIsoeffective,
    MicrodosimetricKinetic,
}

/// Linear-quadratic fractionation parameters for the biological total.
///
/// The model's weighted total is a per-source-particle endpoint;
/// `source_particles_per_fraction` converts it to a per-fraction dose `d`,
/// and the reported total becomes the photon-isoeffective EQD2
/// `n·d·(1 + d/(α/β)) / (1 + 2/(α/β))`. Component volumes keep their
/// linear weighted values — only the total carries the EQD2 transform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fractionation {
    /// Number of identical fractions.
    pub fraction_count: u32,
    /// Source particles delivered per fraction; converts the
    /// per-particle weighted total to a per-fraction dose.
    pub source_particles_per_fraction: f64,
    /// α/β ratio in the endpoint's dose unit, applied outside named regions.
    pub default_alpha_beta: f64,
    /// Per-region α/β overrides; each name needs a mask at apply time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_alpha_beta: BTreeMap<String, f64>,
}

/// The fractionation schedule actually applied to a bundle's total.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedFractionation {
    pub fraction_count: u32,
    pub source_particles_per_fraction: f64,
    /// Region names whose α/β overrides were applied.
    pub regions_applied: Vec<String>,
}

/// A separately versioned biological model artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiologicalModel {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub weight_semantics: WeightSemantics,
    /// The physical-bundle unit this model consumes.
    pub input_unit: DoseUnit,
    /// Dimensionless weight per dose component (`boron`, `nitrogen`,
    /// `hydrogen`, `photon`).
    pub component_weights: WeightMap,
    /// Optional per-region weight overrides keyed by region name. Region
    /// masks are supplied at application time; a region listed here without a
    /// matching mask is an error.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_weights: BTreeMap<String, WeightMap>,
    /// Evidence reference for where the weight values came from.
    pub derivation: Option<ContentReference>,
    /// Free-text description of the model's validity domain (dose range,
    /// tissue types, endpoint); recorded for provenance, not enforced.
    pub validity_domain: Option<String>,
    /// Optional linear-quadratic fractionation applied to the biological
    /// total. Requires `input_unit` `gray_per_source_particle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fractionation: Option<Fractionation>,
}

impl BiologicalModel {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, BIOLOGICAL_MODEL_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() {
            return Err(BioError::Invalid("model id is empty".into()));
        }
        let check = |weights: &WeightMap, label: &str| -> Result<(), BioError> {
            let required: BTreeSet<&str> = DoseComponent::REQUIRED
                .iter()
                .map(|component| component_name(*component))
                .collect();
            let present: BTreeSet<&str> = weights.keys().map(String::as_str).collect();
            if present != required {
                return Err(BioError::Invalid(format!(
                    "{label} must define exactly the four dose components {required:?}; observed {present:?}"
                )));
            }
            for (name, weight) in weights {
                if !weight.is_finite() || *weight < 0.0 {
                    return Err(BioError::Invalid(format!(
                        "{label} weight for {name} must be finite and non-negative"
                    )));
                }
            }
            Ok(())
        };
        check(&self.component_weights, "component_weights")?;
        for (region, weights) in &self.region_weights {
            if region.trim().is_empty() {
                return Err(BioError::Invalid("region name is empty".into()));
            }
            check(weights, "region_weights")?;
        }
        if self.weight_semantics == WeightSemantics::MicrodosimetricKinetic {
            return Err(BioError::Invalid(
                "microdosimetric_kinetic semantics are not expressible as component weights; use an openbnct.microdosimetric-model/0.1.0 artifact".into(),
            ));
        }
        if self.weight_semantics == WeightSemantics::PhotonIsoeffective {
            let photon = component_name(DoseComponent::Photon);
            if self.component_weights[photon] != 1.0
                || self
                    .region_weights
                    .values()
                    .any(|weights| weights[photon] != 1.0)
            {
                return Err(BioError::Invalid(
                    "photon_isoeffective semantics require every photon weight to be exactly 1.0"
                        .into(),
                ));
            }
        }
        if let Some(fractionation) = &self.fractionation {
            if self.input_unit != DoseUnit::GrayPerSourceParticle {
                return Err(BioError::Invalid(
                    "fractionated models require gray_per_source_particle input".into(),
                ));
            }
            if fractionation.fraction_count == 0
                || !fractionation.source_particles_per_fraction.is_finite()
                || fractionation.source_particles_per_fraction <= 0.0
                || !fractionation.default_alpha_beta.is_finite()
                || fractionation.default_alpha_beta <= 0.0
            {
                return Err(BioError::Invalid(
                    "fractionation requires fraction_count >= 1, positive particles per fraction, and positive alpha/beta".into(),
                ));
            }
            for (region, ratio) in &fractionation.region_alpha_beta {
                if region.trim().is_empty() || !ratio.is_finite() || *ratio <= 0.0 {
                    return Err(BioError::Invalid(format!(
                        "fractionation alpha/beta for region {region:?} must be finite and positive"
                    )));
                }
            }
        }
        if let Some(derivation) = &self.derivation {
            derivation
                .validate()
                .map_err(|_| BioError::Invalid("derivation reference is invalid".into()))?;
        }
        Ok(())
    }

    fn weight_for(&self, component: DoseComponent, region: Option<&str>) -> f64 {
        let table = region
            .and_then(|name| self.region_weights.get(name))
            .unwrap_or(&self.component_weights);
        *table
            .get(component_name(component))
            .expect("validated weights cover every component")
    }
}

pub(crate) fn component_name(component: DoseComponent) -> &'static str {
    match component {
        DoseComponent::Boron => "boron",
        DoseComponent::Nitrogen => "nitrogen",
        DoseComponent::Hydrogen => "hydrogen",
        DoseComponent::Photon => "photon",
    }
}

/// Named voxel mask in the bundle's grid order — re-exported from
/// `openbnct-core`, where the type now lives so imaging imports can produce
/// masks without depending on the biological layer.
pub use openbnct_core::RegionMask;

/// One component's biologically weighted dose volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeightedDoseVolume {
    pub component: DoseComponent,
    /// `weighted_gray_per_source_particle` or `weighted_gray`, mirroring the
    /// input bundle's unit.
    pub unit: String,
    pub values: Vec<f64>,
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
}

/// How the biological total's uncertainty was formed. Component
/// uncertainties share transport histories, so the reported total sigma is
/// the fully-correlated linear sum — a conservative upper bound that does not
/// pretend the components are independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiologicalUncertaintyMethod {
    CorrelatedComponentSum,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiologicalTotal {
    pub unit: String,
    pub values: Vec<f64>,
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    pub uncertainty_method: BiologicalUncertaintyMethod,
}

/// The biological interpretation of one physical dose bundle under one model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BiologicalDoseBundle {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub geometry: GridGeometry,
    /// `provenance_id` of the physical bundle this was derived from.
    pub physical_bundle_provenance: String,
    /// Content binding of the exact model JSON that was applied.
    pub model: ContentReference,
    pub weight_semantics: WeightSemantics,
    /// Unit label of the weighted values — deliberately never `gray`.
    pub unit: String,
    pub components: Vec<WeightedDoseVolume>,
    /// The fractionation schedule applied to the total, when the model
    /// declared one; the total then carries `weighted_eqd2` values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fractionation: Option<AppliedFractionation>,
    pub total: BiologicalTotal,
    /// Region names whose override weights were actually applied.
    pub regions_applied: Vec<String>,
    pub qualification: String,
    /// MKM provenance block — present exactly when `weight_semantics`
    /// is `microdosimetric_kinetic`; records the resolved lineal
    /// energies and consumed spectra so the result is checkable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microdosimetry: Option<mkm::MkmApplied>,
}

impl BiologicalDoseBundle {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, BIOLOGICAL_DOSE_BUNDLE_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.case_id.trim().is_empty() {
            return Err(BioError::Invalid("case_id is empty".into()));
        }
        self.model
            .validate()
            .map_err(|_| BioError::Invalid("model reference is invalid".into()))?;
        let voxel_count = self
            .geometry
            .voxel_count()
            .map_err(|e| BioError::Invalid(format!("geometry: {e}")))?;
        let mut observed = BTreeSet::new();
        for volume in &self.components {
            if !observed.insert(volume.component) {
                return Err(BioError::Invalid(format!(
                    "duplicate component {:?}",
                    volume.component
                )));
            }
            if volume.values.len() != voxel_count
                || volume.values.iter().any(|v| !v.is_finite() || *v < 0.0)
            {
                return Err(BioError::Invalid(format!(
                    "component {:?} values are not finite non-negative voxel values",
                    volume.component
                )));
            }
            if let Some(sigma) = &volume.absolute_standard_uncertainty
                && (sigma.len() != voxel_count || sigma.iter().any(|v| !v.is_finite() || *v < 0.0))
            {
                return Err(BioError::Invalid(format!(
                    "component {:?} uncertainty is malformed",
                    volume.component
                )));
            }
        }
        for required in DoseComponent::REQUIRED {
            if !observed.contains(&required) {
                return Err(BioError::Invalid(format!(
                    "component {required:?} is missing"
                )));
            }
        }
        if self.total.values.len() != voxel_count {
            return Err(BioError::Invalid("total dose length mismatch".into()));
        }
        Ok(())
    }
}

fn weighted_unit(unit: DoseUnit) -> String {
    match unit {
        DoseUnit::GrayPerSourceParticle => "weighted_gray_per_source_particle".to_string(),
        DoseUnit::Gray => "weighted_gray".to_string(),
    }
}

/// Apply a biological model to a physical dose bundle.
///
/// `regions` supplies the masks named by `model.region_weights` and by
/// `fractionation.region_alpha_beta`; a model region without a matching
/// mask is rejected, as is a mask that does not cover the bundle grid. When
/// a voxel belongs to several named regions the first matching region in
/// the respective map's order applies — call this out explicitly because
/// overlapping ROI masks are a real possibility. Weight and alpha/beta
/// region selections are independent: a voxel picks its weight table from
/// `region_weights` and its α/β from `region_alpha_beta`, each defaulting
/// when no listed region contains it.
pub fn apply_biological_model(
    model: &BiologicalModel,
    model_bytes: &[u8],
    physical: &PhysicalDoseBundle,
    regions: &[RegionMask],
) -> Result<BiologicalDoseBundle, BioError> {
    model.validate()?;
    physical
        .validate()
        .map_err(|e| BioError::Invalid(format!("physical bundle: {e}")))?;
    let voxel_count = physical
        .geometry
        .voxel_count()
        .map_err(|e| BioError::Invalid(format!("geometry: {e}")))?;
    if physical
        .components
        .first()
        .is_some_and(|c| c.unit != model.input_unit)
    {
        return Err(BioError::Invalid(format!(
            "model input_unit {:?} does not match the physical bundle",
            model.input_unit
        )));
    }

    // Region-name -> voxel lookup; reject model regions without masks and
    // masks that do not cover this grid.
    let masks: BTreeMap<&str, &Vec<bool>> = regions
        .iter()
        .map(|mask| (mask.name.as_str(), &mask.voxels))
        .collect();
    for name in model.region_weights.keys() {
        let mask = masks.get(name.as_str()).ok_or_else(|| {
            BioError::Invalid(format!("model region {name} has no supplied mask"))
        })?;
        if mask.len() != voxel_count {
            return Err(BioError::Invalid(format!(
                "region mask {name} covers {} voxels, grid needs {voxel_count}",
                mask.len()
            )));
        }
    }

    // Per-voxel effective region: first matching mask in region_weights order.
    let region_of = |voxel: usize| -> Option<&str> {
        model
            .region_weights
            .keys()
            .find(|name| masks[name.as_str()][voxel])
            .map(String::as_str)
    };

    let unit = weighted_unit(model.input_unit);
    let mut components = Vec::new();
    for volume in &physical.components {
        let mut values = Vec::with_capacity(voxel_count);
        let mut sigmas = volume
            .absolute_standard_uncertainty
            .as_ref()
            .map(|_| Vec::with_capacity(voxel_count));
        for voxel in 0..voxel_count {
            let weight = model.weight_for(volume.component, region_of(voxel));
            values.push(volume.values[voxel] * weight);
            if let (Some(sigmas), Some(source)) = (
                sigmas.as_mut(),
                volume.absolute_standard_uncertainty.as_ref(),
            ) {
                sigmas.push(source[voxel] * weight);
            }
        }
        components.push(WeightedDoseVolume {
            component: volume.component,
            unit: unit.clone(),
            values,
            absolute_standard_uncertainty: sigmas,
        });
    }

    // Biological total: sum of weighted components; uncertainty is the
    // fully-correlated linear sum of component sigmas (conservative, since
    // components share transport histories).
    let mut total_values = vec![0.0; voxel_count];
    let mut have_sigma = true;
    let mut total_sigma = vec![0.0; voxel_count];
    for component in &components {
        for voxel in 0..voxel_count {
            total_values[voxel] += component.values[voxel];
            if let Some(sigma) = &component.absolute_standard_uncertainty {
                total_sigma[voxel] += sigma[voxel];
            } else {
                have_sigma = false;
            }
        }
    }

    // Fractionated models turn the linear weighted total into a
    // photon-isoeffective EQD2: per-fraction dose d = w·particles, BED =
    // n·d·(1 + d/(α/β)), EQD2 = BED/(1 + 2/(α/β)), with per-region α/β.
    // First-order sigma propagation: σ_EQD2 = σ_w·p·n(1+2d/r)/(1+2/r).
    let mut applied_fractionation = None;
    let mut total_unit = weighted_unit(model.input_unit);
    if let Some(fractionation) = &model.fractionation {
        let n = f64::from(fractionation.fraction_count);
        let p = fractionation.source_particles_per_fraction;
        for name in fractionation.region_alpha_beta.keys() {
            let mask = masks.get(name.as_str()).ok_or_else(|| {
                BioError::Invalid(format!("fractionation region {name} has no supplied mask"))
            })?;
            if mask.len() != voxel_count {
                return Err(BioError::Invalid(format!(
                    "fractionation mask {name} covers {} voxels, grid needs {voxel_count}",
                    mask.len()
                )));
            }
        }
        let alpha_beta_of = |voxel: usize| -> f64 {
            fractionation
                .region_alpha_beta
                .keys()
                .find(|name| masks[name.as_str()][voxel])
                .map_or(fractionation.default_alpha_beta, |name| {
                    fractionation.region_alpha_beta[name]
                })
        };
        for voxel in 0..voxel_count {
            let ratio = alpha_beta_of(voxel);
            let d = total_values[voxel] * p;
            total_values[voxel] = n * d * (1.0 + d / ratio) / (1.0 + 2.0 / ratio);
            if have_sigma {
                let derivative = p * n * (1.0 + 2.0 * d / ratio) / (1.0 + 2.0 / ratio);
                total_sigma[voxel] *= derivative;
            }
        }
        total_unit = "weighted_eqd2".into();
        applied_fractionation = Some(AppliedFractionation {
            fraction_count: fractionation.fraction_count,
            source_particles_per_fraction: p,
            regions_applied: fractionation.region_alpha_beta.keys().cloned().collect(),
        });
    }

    let mut regions_applied: Vec<String> = model.region_weights.keys().cloned().collect();
    regions_applied.retain(|name| masks.contains_key(name.as_str()));
    let bundle = BiologicalDoseBundle {
        schema_version: BIOLOGICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: physical.case_id.clone(),
        geometry: physical.geometry.clone(),
        physical_bundle_provenance: physical.provenance_id.clone(),
        model: ContentReference {
            id: model.id.clone(),
            sha256: format!("{:x}", Sha256::digest(model_bytes)),
        },
        weight_semantics: model.weight_semantics,
        unit,
        components,
        fractionation: applied_fractionation,
        total: BiologicalTotal {
            unit: total_unit,
            values: total_values,
            absolute_standard_uncertainty: have_sigma.then_some(total_sigma),
            uncertainty_method: if have_sigma {
                BiologicalUncertaintyMethod::CorrelatedComponentSum
            } else {
                BiologicalUncertaintyMethod::Unavailable
            },
        },
        regions_applied,
        qualification: "synthetic_research_only_not_clinical".into(),
        microdosimetry: None,
    };
    bundle.validate()?;
    Ok(bundle)
}

#[derive(Debug, Error)]
pub enum BioError {
    #[error("unsupported biological-model schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid biological artifact: {0}")]
    Invalid(String),
    /// An MKM component named a lineal spectrum that was not supplied
    /// at apply time — a hard error, never a silent fallback.
    #[error("lineal spectrum {0:?} was not supplied")]
    UnresolvedSpectrum(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{DoseVolume, PhysicalTotalDoseVolume, TotalUncertaintyMethod};

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [2, 1, 1],
            spacing_mm: [5.0; 3],
            origin_mm: [-2.5; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

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
            geometry: geometry(),
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
        let mut weights = WeightMap::new();
        weights.insert("boron".into(), 3.8);
        weights.insert("nitrogen".into(), 2.5);
        weights.insert("hydrogen".into(), 1.0);
        weights.insert("photon".into(), 1.0);
        BiologicalModel {
            schema_version: BIOLOGICAL_MODEL_SCHEMA.into(),
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

    #[test]
    fn applies_component_weights_and_correlated_total() {
        let model = model();
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let bundle = apply_biological_model(&model, &bytes, &physical_bundle(), &[]).unwrap();
        assert_eq!(bundle.schema_version, BIOLOGICAL_DOSE_BUNDLE_SCHEMA);
        assert_eq!(bundle.unit, "weighted_gray_per_source_particle");
        let boron = bundle
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Boron)
            .unwrap();
        assert_eq!(boron.values[0], 3.8e-12);
        // Total = sum of weighted components; sigma = correlated linear sum.
        let expected = 3.8e-12 + 2.5 * 2.0e-13 + 1.0 * 5.0e-14 + 1.0 * 3.0e-13;
        assert!((bundle.total.values[0] - expected).abs() / expected < 1.0e-12);
        let sigma = 3.8e-14 + 2.5 * 2.0e-15 + 5.0e-16 + 3.0e-15;
        let total_sigma = bundle.total.absolute_standard_uncertainty.as_ref().unwrap()[0];
        assert!((total_sigma - sigma).abs() / sigma < 1.0e-12);
        assert_eq!(
            bundle.total.uncertainty_method,
            BiologicalUncertaintyMethod::CorrelatedComponentSum
        );
        assert_eq!(bundle.model.sha256, format!("{:x}", Sha256::digest(&bytes)));
    }

    #[test]
    fn region_weights_override_defaults_inside_the_mask() {
        let mut model = model();
        let mut tumor = WeightMap::new();
        tumor.insert("boron".into(), 5.0);
        tumor.insert("nitrogen".into(), 2.5);
        tumor.insert("hydrogen".into(), 1.0);
        tumor.insert("photon".into(), 1.0);
        model.region_weights.insert("tumor".into(), tumor);
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let mask = RegionMask {
            name: "tumor".into(),
            voxels: vec![true, false],
        };
        let bundle = apply_biological_model(&model, &bytes, &physical_bundle(), &[mask]).unwrap();
        let boron = bundle
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Boron)
            .unwrap();
        assert_eq!(boron.values, vec![5.0e-12, 3.8e-12]);
        assert_eq!(bundle.regions_applied, vec!["tumor"]);
    }

    #[test]
    fn rejects_missing_region_mask_and_bad_weights() {
        let mut regioned = model();
        regioned
            .region_weights
            .insert("tumor".into(), regioned.component_weights.clone());
        let bytes = serde_json::to_vec_pretty(&regioned).unwrap();
        assert!(apply_biological_model(&regioned, &bytes, &physical_bundle(), &[]).is_err());

        let mut bad = model();
        bad.component_weights.insert("boron".into(), f64::NAN);
        assert!(bad.validate().is_err());
        let mut incomplete = model();
        incomplete.component_weights.remove("photon");
        assert!(incomplete.validate().is_err());
    }

    #[test]
    fn weighted_bundle_never_aliases_physical_unit() {
        let model = model();
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let bundle = apply_biological_model(&model, &bytes, &physical_bundle(), &[]).unwrap();
        assert_ne!(bundle.unit, "gray");
        assert_ne!(bundle.unit, "gray_per_source_particle");
        assert_eq!(bundle.qualification, "synthetic_research_only_not_clinical");
        assert_eq!(bundle.physical_bundle_provenance, "test-provenance");
    }

    #[test]
    fn photon_isoeffective_requires_unit_photon_weight() {
        let mut model = model();
        model.weight_semantics = WeightSemantics::PhotonIsoeffective;
        assert!(model.validate().is_ok()); // fixture photon weight is 1.0

        model.component_weights.insert("photon".into(), 1.2);
        assert!(model.validate().is_err());

        model.component_weights.insert("photon".into(), 1.0);
        let mut region = WeightMap::from([
            ("boron".to_string(), 4.0),
            ("nitrogen".to_string(), 2.0),
            ("hydrogen".to_string(), 1.0),
            ("photon".to_string(), 1.1),
        ]);
        model.region_weights.insert("tumor".into(), region.clone());
        assert!(model.validate().is_err());
        region.insert("photon".into(), 1.0);
        model.region_weights.insert("tumor".into(), region);
        assert!(model.validate().is_ok());
    }

    #[test]
    fn fractionation_transforms_total_to_eqd2_with_region_alpha_beta() {
        let mut model = model();
        let mut fractionation = Fractionation {
            fraction_count: 30,
            source_particles_per_fraction: 1.0e12,
            default_alpha_beta: 3.0,
            region_alpha_beta: BTreeMap::from([("tumor".to_string(), 10.0)]),
        };
        model.fractionation = Some(fractionation.clone());
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let mask = RegionMask {
            name: "tumor".into(),
            voxels: vec![true, false],
        };
        let bundle = apply_biological_model(
            &model,
            &bytes,
            &physical_bundle(),
            std::slice::from_ref(&mask),
        )
        .unwrap();

        // Weighted per-particle total w = 3.8e-12 + 2.5*2e-13 + 1*5e-14 +
        // 1*3e-13 = 4.65e-12 (tumor boron weight is default: model has no
        // region_weights here, only region alpha/beta).
        let w = 3.8e-12 + 2.5 * 2.0e-13 + 5.0e-14 + 3.0e-13;
        let d = w * 1.0e12; // per-fraction dose
        let eqd2_tumor = 30.0 * d * (1.0 + d / 10.0) / (1.0 + 2.0 / 10.0);
        let eqd2_default = 30.0 * d * (1.0 + d / 3.0) / (1.0 + 2.0 / 3.0);
        assert!((bundle.total.values[0] - eqd2_tumor).abs() / eqd2_tumor < 1e-9);
        assert!((bundle.total.values[1] - eqd2_default).abs() / eqd2_default < 1e-9);
        assert_eq!(bundle.total.unit, "weighted_eqd2");
        assert_eq!(bundle.fractionation.unwrap().regions_applied, vec!["tumor"]);

        fractionation
            .region_alpha_beta
            .insert("missing".into(), 5.0);
        model.fractionation = Some(fractionation);
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        assert!(
            apply_biological_model(
                &model,
                &bytes,
                &physical_bundle(),
                std::slice::from_ref(&mask)
            )
            .is_err()
        );
    }

    #[test]
    fn fractionation_requires_per_particle_input_and_valid_parameters() {
        let mut model = model();
        model.input_unit = DoseUnit::Gray;
        model.fractionation = Some(Fractionation {
            fraction_count: 10,
            source_particles_per_fraction: 1.0e9,
            default_alpha_beta: 3.0,
            region_alpha_beta: BTreeMap::new(),
        });
        assert!(model.validate().is_err());

        model.input_unit = DoseUnit::GrayPerSourceParticle;
        model.fractionation.as_mut().unwrap().default_alpha_beta = 0.0;
        assert!(model.validate().is_err());
        model.fractionation.as_mut().unwrap().default_alpha_beta = 3.0;
        model.fractionation.as_mut().unwrap().fraction_count = 0;
        assert!(model.validate().is_err());
    }
}
