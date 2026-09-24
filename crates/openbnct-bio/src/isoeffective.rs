//! González & Santa Cruz photon-isoeffective dose (Radiation Research
//! 178(6), 2012 — DOI 10.1667/RR2944.1).
//!
//! Fixed RBE/CBE weighting (`WeightSemantics::PhotonIsoeffective` on a
//! `BiologicalModel`) sums dose-independent weighted doses; the IsoE
//! formalism instead equates the mixed-field survival exponent to the
//! photon linear-quadratic response and inverts:
//!
//! ```text
//! X = α_γ·Σᵢ RBEᵢ·dᵢ  +  G·β_γ·(Σᵢ √RBEβᵢ·dᵢ)²
//! α_γ·D_IsoE + β_γ·D_IsoE² = X
//! ```
//!
//! `RBEᵢ` is component i's linear-term effectiveness relative to the
//! photon reference (the CBE for boron), `RBEβᵢ` the quadratic-term
//! factor — the √β cross terms are the Zaider–Rossi synergy the fixed
//! weighting drops. `G` is the generalized Lea–Catcheside factor for
//! sublethal repair during protracted delivery; for constant-rate
//! irradiation of duration T with first-order repair rate μ,
//! `G = 2(μT − 1 + e^(−μT)) / (μT)²` (G → 1 for acute delivery).
//!
//! Per-component bundle values allocate the isoeffective dose by
//! effect share, so components still sum to the total exactly.

use std::collections::{BTreeMap, BTreeSet};

use openbnct_core::{ContentReference, DoseComponent, DoseUnit, PhysicalDoseBundle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mkm::MkmLq;
use crate::{
    AppliedFractionation, BioError, BiologicalDoseBundle, BiologicalTotal, Fractionation,
    RegionMask, WeightSemantics, WeightedDoseVolume, component_name, ordered_region_names,
    resolve_regions,
};

/// Schema identifier carried by every IsoE model artifact.
pub const ISOEFFECTIVE_MODEL_SCHEMA: &str = "openbnct.isoeffective-model/0.1.0";

/// Dose-independent IsoE weighting factors for one dose component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsoeComponent {
    /// Linear-term effectiveness relative to the photon reference
    /// (`RBEᵢ = αᵢ/α_γ`; the boron CBE lives here).
    pub rbe: f64,
    /// Quadratic-term factor (`RBEβᵢ = βᵢ/β_γ`); 1.0 for a component
    /// whose damage is purely linear relative to the reference.
    pub rbe_beta: f64,
}

/// Delivery-time structure for the Lea–Catcheside repair factor.
///
/// Constant dose rate over `duration_s` with first-order sublethal
/// repair at `repair_rate_per_s`; absent → G = 1 (acute delivery, no
/// repair credit — conservative for BNCT's tens-of-minute irradiations).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Irradiation {
    /// Irradiation duration in seconds.
    pub duration_s: f64,
    /// First-order repair rate μ in s⁻¹ (μ = ln2 / repair half-time).
    pub repair_rate_per_s: f64,
}

/// A separately versioned photon-isoeffective model artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsoeffectiveModel {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The physical-bundle unit this model consumes.
    pub input_unit: DoseUnit,
    /// Photon-reference α (Gy⁻¹) of the tissue.
    pub alpha_0: f64,
    /// Photon-reference β (Gy⁻²); domain-invariant.
    pub beta: f64,
    /// Dose-independent factors per dose component — exactly the four
    /// keys (`boron`, `nitrogen`, `hydrogen`, `photon`); the photon
    /// entry must be `rbe = rbe_beta = 1.0` by definition.
    pub components: BTreeMap<String, IsoeComponent>,
    /// Optional per-region factor tables, all four components each.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_factors: BTreeMap<String, BTreeMap<String, IsoeComponent>>,
    /// Optional per-region photon-reference LQ overrides — tumor and
    /// normal-tissue radiosensitivity legitimately differ.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_lq: BTreeMap<String, MkmLq>,
    /// Explicit precedence for overlapping region masks, earliest wins —
    /// required when voxels could match more than one declared region.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region_priority: Vec<String>,
    /// Delivery-time structure driving the repair factor G.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irradiation: Option<Irradiation>,
    /// Evidence reference for where the factors came from.
    pub derivation: Option<ContentReference>,
    /// Mandatory free-text validity domain: tissue, dose range,
    /// irradiation conditions the factors were derived under.
    pub validity_domain: String,
    /// Optional inter-fraction EQD2 on the IsoE total (intra-fraction
    /// repair is G's job; this is the between-fractions transform).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fractionation: Option<Fractionation>,
}

impl IsoeffectiveModel {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, ISOEFFECTIVE_MODEL_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() {
            return Err(BioError::Invalid("model id is empty".into()));
        }
        if self.validity_domain.trim().is_empty() {
            return Err(BioError::Invalid(
                "validity_domain is required for isoeffective models".into(),
            ));
        }
        if !self.alpha_0.is_finite()
            || self.alpha_0 < 0.0
            || !self.beta.is_finite()
            || self.beta < 0.0
            || (self.alpha_0 == 0.0 && self.beta == 0.0)
        {
            return Err(BioError::Invalid(
                "alpha_0 and beta must be finite, non-negative, and not both zero".into(),
            ));
        }
        let required: BTreeSet<&str> = [
            DoseComponent::Boron,
            DoseComponent::Nitrogen,
            DoseComponent::Hydrogen,
            DoseComponent::Photon,
        ]
        .iter()
        .map(|c| component_name(*c))
        .collect();
        let check_factors = |factors: &BTreeMap<String, IsoeComponent>,
                             label: &str|
         -> Result<(), BioError> {
            let present: BTreeSet<&str> = factors.keys().map(String::as_str).collect();
            if present != required {
                return Err(BioError::Invalid(format!(
                    "{label} must define exactly the four dose components {required:?}; observed {present:?}"
                )));
            }
            for (name, factor) in factors {
                if !factor.rbe.is_finite()
                    || factor.rbe < 0.0
                    || !factor.rbe_beta.is_finite()
                    || factor.rbe_beta < 0.0
                {
                    return Err(BioError::Invalid(format!(
                        "{label} factors for {name} must be finite and non-negative"
                    )));
                }
            }
            Ok(())
        };
        check_factors(&self.components, "components")?;
        let photon = component_name(DoseComponent::Photon);
        if self.components[photon].rbe != 1.0 || self.components[photon].rbe_beta != 1.0 {
            return Err(BioError::Invalid(
                "the photon component's factors must be exactly rbe = rbe_beta = 1.0 — it is the reference".into(),
            ));
        }
        for (region, factors) in &self.region_factors {
            if region.trim().is_empty() {
                return Err(BioError::Invalid("region name is empty".into()));
            }
            check_factors(factors, "region_factors")?;
        }
        for (region, lq) in &self.region_lq {
            if region.trim().is_empty() {
                return Err(BioError::Invalid("region name is empty".into()));
            }
            if !lq.alpha_0.is_finite()
                || lq.alpha_0 < 0.0
                || !lq.beta.is_finite()
                || lq.beta < 0.0
                || (lq.alpha_0 == 0.0 && lq.beta == 0.0)
            {
                return Err(BioError::Invalid(format!(
                    "region {region}: alpha_0 and beta must be finite, non-negative, and not both zero"
                )));
            }
        }
        {
            let declared: BTreeSet<&str> = self
                .region_factors
                .keys()
                .chain(self.region_lq.keys())
                .chain(
                    self.fractionation
                        .iter()
                        .flat_map(|f| f.region_alpha_beta.keys()),
                )
                .map(String::as_str)
                .collect();
            for name in &self.region_priority {
                if !declared.contains(name.as_str()) {
                    return Err(BioError::Invalid(format!(
                        "region_priority names {name:?}, which is not a declared region"
                    )));
                }
            }
        }
        if let Some(irradiation) = &self.irradiation
            && (!irradiation.duration_s.is_finite()
                || irradiation.duration_s <= 0.0
                || !irradiation.repair_rate_per_s.is_finite()
                || irradiation.repair_rate_per_s < 0.0)
        {
            return Err(BioError::Invalid(
                "irradiation requires positive duration_s and non-negative repair_rate_per_s"
                    .into(),
            ));
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
                        "fractionation region {region}: alpha_beta must be positive"
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

    /// The Lea–Catcheside repair factor for the declared delivery —
    /// 1.0 when no `irradiation` is declared (acute-delivery limit).
    pub fn repair_factor(&self) -> f64 {
        let Some(irr) = &self.irradiation else {
            return 1.0;
        };
        let mu_t = irr.repair_rate_per_s * irr.duration_s;
        if mu_t <= 0.0 {
            return 1.0;
        }
        2.0 * (mu_t - 1.0 + (-mu_t).exp()) / (mu_t * mu_t)
    }
}

/// Invert the photon LQ once for a combined effect X — shared shape
/// with `mkm::invert_lq`, reimplemented locally so this module reads
/// standalone (the reference α₀/β here are tissue radiosensitivity,
/// not MKM domain parameters).
fn invert_lq(x: f64, alpha_0: f64, beta: f64) -> (f64, f64) {
    if x <= 0.0 {
        return (0.0, if alpha_0 > 0.0 { 1.0 / alpha_0 } else { 0.0 });
    }
    if beta <= 0.0 {
        return (
            if alpha_0 > 0.0 { x / alpha_0 } else { 0.0 },
            if alpha_0 > 0.0 { 1.0 / alpha_0 } else { 0.0 },
        );
    }
    let root = (alpha_0 * alpha_0 + 4.0 * beta * x).sqrt();
    let dose = if alpha_0 > 0.0 {
        2.0 * x / (alpha_0 + root)
    } else {
        (x / beta).sqrt()
    };
    (dose, 1.0 / (alpha_0 + 2.0 * beta * dose))
}

/// Provenance recorded on bundles produced by an IsoE model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IsoeApplied {
    /// The applied Lea–Catcheside repair factor.
    pub g_factor: f64,
    /// The declared delivery structure, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irradiation: Option<Irradiation>,
}

/// Apply an IsoE model to a physical dose bundle.
///
/// `regions` supplies the masks named by `region_factors`,
/// `region_lq`, and `fractionation.region_alpha_beta`; overlapping
/// masks require `region_priority` (shared resolution rule with the
/// weight and MKM models). Uncertainty propagation is first-order
/// through the inversion: σ_D = σ_X/(α₀ + 2β·D_IsoE).
pub fn apply_isoeffective_model(
    model: &IsoeffectiveModel,
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

    let masks: BTreeMap<&str, &Vec<bool>> = regions
        .iter()
        .map(|mask| (mask.name.as_str(), &mask.voxels))
        .collect();
    for name in model.region_factors.keys().chain(model.region_lq.keys()) {
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

    // Region resolution: factors and reference-LQ resolve independently,
    // each under the declared priority.
    let factor_order = ordered_region_names(model.region_factors.keys(), &model.region_priority);
    let factor_assignment = resolve_regions(
        &factor_order,
        &masks,
        &model.region_priority,
        voxel_count,
        "region_factors",
    )?;
    let lq_order = ordered_region_names(model.region_lq.keys(), &model.region_priority);
    let lq_assignment = resolve_regions(
        &lq_order,
        &masks,
        &model.region_priority,
        voxel_count,
        "region_lq",
    )?;

    let g = model.repair_factor();
    let p = model
        .fractionation
        .as_ref()
        .map_or(1.0, |f| f.source_particles_per_fraction);
    let unit = match model.input_unit {
        DoseUnit::GrayPerSourceParticle => "isoeffective_gray_per_source_particle".to_string(),
        DoseUnit::Gray => "isoeffective_gray".to_string(),
    };

    let component_count = physical.components.len();
    let mut components: Vec<WeightedDoseVolume> = physical
        .components
        .iter()
        .map(|volume| WeightedDoseVolume {
            component: volume.component,
            unit: unit.clone(),
            values: vec![0.0; voxel_count],
            absolute_standard_uncertainty: volume
                .absolute_standard_uncertainty
                .as_ref()
                .map(|_| vec![0.0; voxel_count]),
        })
        .collect();
    let mut total_values = vec![0.0; voxel_count];
    let have_sigma = physical
        .components
        .iter()
        .all(|v| v.absolute_standard_uncertainty.is_some());
    let mut total_sigma = vec![0.0; voxel_count];

    for voxel in 0..voxel_count {
        let factor_table = factor_assignment[voxel]
            .map(|i| &model.region_factors[&factor_order[i]])
            .unwrap_or(&model.components);
        let (ref_alpha, ref_beta) = lq_assignment[voxel]
            .map(|i| {
                let lq = &model.region_lq[&lq_order[i]];
                (lq.alpha_0, lq.beta)
            })
            .unwrap_or((model.alpha_0, model.beta));
        // X = α_γ·Σ rbeᵢ·dᵢ + G·β_γ·(Σ √rbeβᵢ·dᵢ)²
        let mut linear = 0.0;
        let mut sqrt_sum = 0.0;
        let mut rbe = vec![0.0; component_count];
        let mut rbe_beta = vec![0.0; component_count];
        let mut doses = vec![0.0; component_count];
        for (index, volume) in physical.components.iter().enumerate() {
            let d = volume.values[voxel] * p;
            let factor = &factor_table[component_name(volume.component)];
            rbe[index] = factor.rbe;
            rbe_beta[index] = factor.rbe_beta;
            doses[index] = d;
            linear += factor.rbe * d;
            sqrt_sum += factor.rbe_beta.sqrt() * d;
        }
        let effect = ref_alpha * linear + g * ref_beta * sqrt_sum * sqrt_sum;
        let (d_iso, dd_dx) = invert_lq(effect, ref_alpha, ref_beta);
        total_values[voxel] = d_iso;
        for (index, volume) in physical.components.iter().enumerate() {
            // Effect share: α_γ·rbeᵢ·dᵢ + G·β_γ·(√rbeβᵢ·dᵢ)·(Σ√rbeβⱼ·dⱼ)
            // — linear term plus this component's portion of every
            // cross term involving it.
            let share = ref_alpha * rbe[index] * doses[index]
                + g * ref_beta * rbe_beta[index].sqrt() * doses[index] * sqrt_sum;
            components[index].values[voxel] = if effect > 0.0 {
                d_iso * share / effect
            } else {
                0.0
            };
            if let (Some(sigmas), Some(source)) = (
                components[index].absolute_standard_uncertainty.as_mut(),
                volume.absolute_standard_uncertainty.as_ref(),
            ) {
                // ∂X/∂dᵢ = α_γ·rbeᵢ + 2·G·β_γ·√rbeβᵢ·(Σ√rbeβⱼ·dⱼ)
                let dx_ddi =
                    ref_alpha * rbe[index] + 2.0 * g * ref_beta * rbe_beta[index].sqrt() * sqrt_sum;
                let sigma = source[voxel] * p * dd_dx * dx_ddi;
                sigmas[voxel] = sigma;
                total_sigma[voxel] += sigma;
            }
        }
    }

    let mut applied_fractionation = None;
    let mut total_unit = unit.clone();
    if let Some(fractionation) = &model.fractionation {
        let n = f64::from(fractionation.fraction_count);
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
        let ab_order = ordered_region_names(
            fractionation.region_alpha_beta.keys(),
            &model.region_priority,
        );
        let ab_assignment = resolve_regions(
            &ab_order,
            &masks,
            &model.region_priority,
            voxel_count,
            "region_alpha_beta",
        )?;
        let alpha_beta_of = |voxel: usize| -> f64 {
            ab_assignment[voxel].map_or(fractionation.default_alpha_beta, |i| {
                fractionation.region_alpha_beta[&ab_order[i]]
            })
        };
        for voxel in 0..voxel_count {
            let ratio = alpha_beta_of(voxel);
            let t = total_values[voxel];
            total_values[voxel] = n * t * (1.0 + t / ratio) / (1.0 + 2.0 / ratio);
            if have_sigma {
                let derivative = n * (1.0 + 2.0 * t / ratio) / (1.0 + 2.0 / ratio);
                total_sigma[voxel] *= derivative;
            }
        }
        total_unit = "isoeffective_eqd2".into();
        applied_fractionation = Some(AppliedFractionation {
            fraction_count: fractionation.fraction_count,
            source_particles_per_fraction: p,
            regions_applied: fractionation.region_alpha_beta.keys().cloned().collect(),
        });
    }

    let mut regions_applied: Vec<String> = model
        .region_factors
        .keys()
        .chain(model.region_lq.keys())
        .cloned()
        .collect();
    regions_applied.retain(|name| masks.contains_key(name.as_str()));
    regions_applied.sort();
    regions_applied.dedup();

    let bundle = BiologicalDoseBundle {
        schema_version: crate::BIOLOGICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: physical.case_id.clone(),
        geometry: physical.geometry.clone(),
        physical_bundle_provenance: physical.provenance_id.clone(),
        model: ContentReference {
            id: model.id.clone(),
            sha256: format!("{:x}", Sha256::digest(model_bytes)),
        },
        weight_semantics: WeightSemantics::PhotonIsoeffective,
        unit: total_unit.clone(),
        components,
        fractionation: applied_fractionation,
        total: BiologicalTotal {
            unit: total_unit.clone(),
            values: total_values,
            absolute_standard_uncertainty: if have_sigma { Some(total_sigma) } else { None },
            uncertainty_method: if have_sigma {
                crate::BiologicalUncertaintyMethod::CorrelatedComponentSum
            } else {
                crate::BiologicalUncertaintyMethod::Unavailable
            },
        },
        regions_applied,
        qualification: "isoeffective_research_only_not_clinical".into(),
        microdosimetry: None,
        isoeffective: Some(IsoeApplied {
            g_factor: g,
            irradiation: model.irradiation,
        }),
        smk: None,
    };
    bundle.validate()?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{DoseComponent, DoseVolume, GridGeometry, PhysicalTotalDoseVolume};

    fn physical() -> PhysicalDoseBundle {
        let component = |c: DoseComponent, values: Vec<f64>| DoseVolume {
            component: c,
            unit: DoseUnit::Gray,
            values,
            absolute_standard_uncertainty: None,
        };
        PhysicalDoseBundle {
            schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "case".into(),
            frame_of_reference_uid: None,
            geometry: GridGeometry {
                shape: [2, 1, 1],
                spacing_mm: [1.0, 1.0, 1.0],
                origin_mm: [0.0, 0.0, 0.0],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            component_profile: openbnct_core::ComponentProfileReference {
                id: "profile".into(),
                sha256: "a".repeat(64),
            },
            response_set: ContentReference {
                id: "responses".into(),
                sha256: "b".repeat(64),
            },
            components: vec![
                component(DoseComponent::Boron, vec![4.0, 1.0]),
                component(DoseComponent::Nitrogen, vec![0.5, 0.4]),
                component(DoseComponent::Hydrogen, vec![1.0, 1.5]),
                component(DoseComponent::Photon, vec![2.0, 2.0]),
            ],
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::Gray,
                values: vec![7.5, 4.9],
                absolute_standard_uncertainty: None,
                uncertainty_method: openbnct_core::TotalUncertaintyMethod::Unavailable,
            },
            provenance_id: "prov".into(),
        }
    }

    fn model() -> IsoeffectiveModel {
        let factor = |rbe: f64, rbe_beta: f64| IsoeComponent { rbe, rbe_beta };
        let mut components = BTreeMap::new();
        components.insert("boron".to_string(), factor(3.8, 1.0));
        components.insert("nitrogen".to_string(), factor(3.2, 1.0));
        components.insert("hydrogen".to_string(), factor(3.2, 1.0));
        components.insert("photon".to_string(), factor(1.0, 1.0));
        IsoeffectiveModel {
            schema_version: ISOEFFECTIVE_MODEL_SCHEMA.into(),
            id: "test.isoe.v1".into(),
            input_unit: DoseUnit::Gray,
            alpha_0: 0.172,
            beta: 0.0615,
            components,
            region_factors: BTreeMap::new(),
            region_lq: BTreeMap::new(),
            region_priority: Vec::new(),
            irradiation: None,
            derivation: None,
            validity_domain: "test".into(),
            fractionation: None,
        }
    }

    #[test]
    fn combined_effect_inverts_once_and_components_sum() {
        let bundle = apply_isoeffective_model(&model(), b"{}", &physical(), &[]).unwrap();
        // X = α·Σrbeᵢdᵢ + β(Σ√rbeβᵢ dᵢ)², D solves αD + βD² = X.
        let (a, b) = (0.172, 0.0615);
        let splits = [(4.0, 3.8), (0.5, 3.2), (1.0, 3.2), (2.0, 1.0)];
        let x = a * splits.iter().map(|(d, r)| r * d).sum::<f64>() + b * 7.5_f64.powi(2);
        let expected = 2.0 * x / (a + (a * a + 4.0 * b * x).sqrt());
        assert!((bundle.total.values[0] - expected).abs() < 1e-9);
        let sum: f64 = bundle.components.iter().map(|c| c.values[0]).sum();
        assert!((sum - bundle.total.values[0]).abs() < 1e-12);
        assert_eq!(bundle.weight_semantics, WeightSemantics::PhotonIsoeffective);
    }

    #[test]
    fn repair_factor_reduces_quadratic_term() {
        let mut m = model();
        m.irradiation = Some(Irradiation {
            duration_s: 1800.0,
            repair_rate_per_s: 0.01,
        });
        let g = m.repair_factor();
        assert!(g > 0.0 && g < 1.0);
        let acute = apply_isoeffective_model(&model(), b"{}", &physical(), &[]).unwrap();
        let protracted = apply_isoeffective_model(&m, b"{}", &physical(), &[]).unwrap();
        assert!(protracted.total.values[0] < acute.total.values[0]);
        assert_eq!(protracted.isoeffective.as_ref().unwrap().g_factor, g);
    }

    #[test]
    fn overlapping_regions_require_priority() {
        let mut m = model();
        m.region_factors
            .insert("tumor".into(), m.components.clone());
        m.region_factors
            .insert("brain".into(), m.components.clone());
        let masks = vec![
            RegionMask {
                name: "tumor".into(),
                voxels: vec![true, false],
            },
            RegionMask {
                name: "brain".into(),
                voxels: vec![true, true],
            },
        ];
        let err = apply_isoeffective_model(&m, b"{}", &physical(), &masks).unwrap_err();
        assert!(format!("{err}").contains("multiple regions"));
        m.region_priority = vec!["tumor".into(), "brain".into()];
        assert!(apply_isoeffective_model(&m, b"{}", &physical(), &masks).is_ok());
    }

    #[test]
    fn non_unity_photon_factors_rejected() {
        let mut m = model();
        m.components.get_mut("photon").unwrap().rbe = 1.1;
        assert!(m.validate().is_err());
    }
}
