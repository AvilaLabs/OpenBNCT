// SPDX-License-Identifier: Apache-2.0

//! PET-derived boron-10 field modeling (`openbnct.boron-uptake-model/0.1.0`,
//! `openbnct.boron-field/0.1.0`).
//!
//! A boron uptake model maps a voxelwise ¹⁸F-BPA PET SUV volume — already
//! co-registered and resampled onto the transport grid — into a B-10
//! concentration field (µg/g) with a propagated per-voxel 1σ. The field is
//! the artifact a downstream step can realize into a
//! [`openbnct_transport::MaterialAssignment`] (via [`materialize_field`]) or
//! use for boron-dose sensitivity analysis.
//!
//! Honesty conventions, mirroring the other model families:
//!
//! - The model document carries a mandatory free-text `validity_domain`
//!   describing the imaging protocol, population, and time assumptions the
//!   mapping was fitted under; empty domains are rejected.
//! - Every emitted field records which model, SUV image, and registration
//!   produced it by content hash, plus how many voxels required clamping —
//!   the model does not silently hide nonphysical output.
//! - Mapped values are a research estimate of an unmeasured tissue
//!   concentration; the emitted `qualification` string states this.

use openbnct_core::{ContentReference, GridGeometry, ValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current schema token for boron uptake model documents.
pub const BORON_UPTAKE_MODEL_SCHEMA: &str = "openbnct.boron-uptake-model/0.1.0";

/// Current schema token for boron field documents.
pub const BORON_FIELD_SCHEMA: &str = "openbnct.boron-field/0.1.0";

/// Qualification asserted on every emitted field.
pub const BORON_FIELD_QUALIFICATION: &str = "pet_derived_boron_research_only_not_clinical";

/// ln 2 — the exponential washout constant.
const LN2: f64 = std::f64::consts::LN_2;

/// A versioned SUV→B-10 uptake model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronUptakeModel {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The SUV→concentration mapping and its parameter uncertainties.
    pub mapping: UptakeMapping,
    /// Optional exponential washout between the PET measurement time and
    /// irradiation: `B(t_irr) = B(t_pet)·2^(−Δt/T½)`. This is a uniform,
    /// single-rate correction — it assumes every voxel shares the same
    /// washout half-life, which is the standard first-order approximation
    /// and *not* a compartment pharmacokinetic model.
    pub time_correction: Option<WashoutCorrection>,
    /// Optional per-voxel SUV measurement noise (1σ, SUV units) folded
    /// into the propagated uncertainty.
    pub suv_noise_1sigma: Option<f64>,
    /// Free-text validity domain: imaging protocol, population, tracer
    /// dose, timing, and tissue assumptions. Required and non-empty.
    pub validity_domain: String,
    pub provenance_id: String,
}

/// How SUV is mapped to B-10 concentration.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UptakeMapping {
    /// `B10(v) = η · R_ref · SUV(v) / SUV_ref`: a measured reference
    /// concentration (classically a blood sample at irradiation time)
    /// scales the SUV ratio — the tumor:blood-ratio method.
    SuvRatio {
        /// Blood-pool (or other reference-region) mean SUV.
        reference_suv: f64,
        /// 1σ uncertainty on `reference_suv`.
        reference_suv_1sigma: f64,
        /// Measured reference B-10 (or total-boron) concentration, µg/g.
        reference_boron_ug_g: f64,
        /// 1σ uncertainty on `reference_boron_ug_g`.
        reference_boron_1sigma: f64,
        /// Fraction of the reference assay that is B-10: 1.0 when the
        /// assay already reports B-10, ~0.99 for enriched-BPA total-boron
        /// assays, 0.199 for natural-abundance boron.
        b10_fraction_of_reference: f64,
    },
    /// `B10(v) = a·SUV(v) + b`: a calibrated linear mapping, e.g. a
    /// population regression between image SUV and assayed concentration.
    LinearSuv {
        slope_ug_g_per_suv: f64,
        slope_1sigma: f64,
        intercept_ug_g: f64,
        intercept_1sigma: f64,
    },
    /// A declared spatially uniform concentration — the conventional
    /// "assumed uptake" baseline; consumes no SUV volume.
    Uniform {
        concentration_ug_g: f64,
        concentration_1sigma: f64,
    },
}

/// Uniform exponential washout between measurement and irradiation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WashoutCorrection {
    /// Minutes between the SUV measurement and irradiation start (≥ 0).
    pub delta_minutes: f64,
    /// Boron biological washout half-life in minutes (> 0).
    pub half_life_minutes: f64,
    /// 1σ uncertainty on `half_life_minutes`.
    pub half_life_1sigma_minutes: f64,
}

/// A per-voxel B-10 concentration field on a transport grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronField {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The transport case this field is defined for.
    pub case_id: String,
    pub geometry: GridGeometry,
    /// B-10 concentration per voxel in µg/g, in grid order
    /// `i + nx·j + nx·ny·k`.
    pub values: Vec<f64>,
    /// Per-voxel propagated 1σ (µg/g), same order as `values`.
    pub uncertainty_1sigma: Vec<f64>,
    /// Voxels whose raw mapped value was negative and clamped to zero —
    /// recorded, not hidden.
    pub clamped_negative_voxels: u64,
    /// The uptake model applied.
    pub model: ContentReference,
    /// The SUV volume consumed; `zero-filled` when the model is
    /// `uniform` and no image was required.
    pub suv_image: Option<ContentReference>,
    /// The registration applied to the SUV volume, when used.
    pub registration: Option<ContentReference>,
    pub qualification: String,
    pub provenance_id: String,
}

/// Errors from boron model/field construction and validation.
#[derive(Debug, Error)]
pub enum BoronError {
    #[error("unsupported boron uptake model schema {0:?}")]
    UnsupportedModelSchema(String),
    #[error("unsupported boron field schema {0:?}")]
    UnsupportedFieldSchema(String),
    #[error("invalid boron uptake model: {0}")]
    InvalidModel(String),
    #[error("invalid boron field: {0}")]
    InvalidField(String),
    #[error("SUV voxel count {suv} does not match grid voxel count {grid}")]
    SuvCountMismatch { suv: usize, grid: usize },
    #[error("SUV value at voxel {index} is non-finite or negative")]
    InvalidSuvValue { index: usize },
    #[error("model mapping requires an SUV volume but none was supplied")]
    SuvRequired,
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] ValidationError),
    #[error("invalid content reference: {0}")]
    InvalidContentReference(#[from] openbnct_core::ContentReferenceError),
}

impl BoronUptakeModel {
    /// Structural validation; called by every constructor/consumer.
    pub fn validate(&self) -> Result<(), BoronError> {
        if !openbnct_core::schema_matches(&self.schema_version, BORON_UPTAKE_MODEL_SCHEMA) {
            return Err(BoronError::UnsupportedModelSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(BoronError::InvalidModel("model id is empty".into()));
        }
        if self.validity_domain.trim().is_empty() {
            return Err(BoronError::InvalidModel(
                "validity_domain is required and must not be empty".into(),
            ));
        }
        match &self.mapping {
            UptakeMapping::SuvRatio {
                reference_suv,
                reference_suv_1sigma,
                reference_boron_ug_g,
                reference_boron_1sigma,
                b10_fraction_of_reference,
            } => {
                if !reference_suv.is_finite() || *reference_suv <= 0.0 {
                    return Err(BoronError::InvalidModel(
                        "suv_ratio.reference_suv must be positive".into(),
                    ));
                }
                for (name, v) in [
                    ("reference_suv_1sigma", *reference_suv_1sigma),
                    ("reference_boron_ug_g", *reference_boron_ug_g),
                    ("reference_boron_1sigma", *reference_boron_1sigma),
                ] {
                    if !v.is_finite() || v < 0.0 {
                        return Err(BoronError::InvalidModel(format!(
                            "suv_ratio.{name} must be a non-negative finite value"
                        )));
                    }
                }
                if !b10_fraction_of_reference.is_finite()
                    || *b10_fraction_of_reference <= 0.0
                    || *b10_fraction_of_reference > 1.0
                {
                    return Err(BoronError::InvalidModel(
                        "suv_ratio.b10_fraction_of_reference must lie in (0, 1]".into(),
                    ));
                }
            }
            UptakeMapping::LinearSuv {
                slope_ug_g_per_suv,
                slope_1sigma,
                intercept_ug_g,
                intercept_1sigma,
            } => {
                for (name, v) in [
                    ("slope_ug_g_per_suv", *slope_ug_g_per_suv),
                    ("slope_1sigma", *slope_1sigma),
                    ("intercept_ug_g", *intercept_ug_g),
                    ("intercept_1sigma", *intercept_1sigma),
                ] {
                    if !v.is_finite() {
                        return Err(BoronError::InvalidModel(format!(
                            "linear_suv.{name} must be finite"
                        )));
                    }
                }
                if *slope_1sigma < 0.0 || *intercept_1sigma < 0.0 {
                    return Err(BoronError::InvalidModel(
                        "linear_suv sigmas must be non-negative".into(),
                    ));
                }
                if *slope_ug_g_per_suv == 0.0 && *intercept_ug_g == 0.0 {
                    return Err(BoronError::InvalidModel(
                        "linear_suv mapping is identically zero".into(),
                    ));
                }
            }
            UptakeMapping::Uniform {
                concentration_ug_g,
                concentration_1sigma,
            } => {
                if !concentration_ug_g.is_finite() || *concentration_ug_g < 0.0 {
                    return Err(BoronError::InvalidModel(
                        "uniform.concentration_ug_g must be a non-negative finite value".into(),
                    ));
                }
                if !concentration_1sigma.is_finite() || *concentration_1sigma < 0.0 {
                    return Err(BoronError::InvalidModel(
                        "uniform.concentration_1sigma must be a non-negative finite value".into(),
                    ));
                }
            }
        }
        if let Some(washout) = &self.time_correction {
            if !washout.delta_minutes.is_finite() || washout.delta_minutes < 0.0 {
                return Err(BoronError::InvalidModel(
                    "time_correction.delta_minutes must be a non-negative finite value".into(),
                ));
            }
            if !washout.half_life_minutes.is_finite() || washout.half_life_minutes <= 0.0 {
                return Err(BoronError::InvalidModel(
                    "time_correction.half_life_minutes must be positive".into(),
                ));
            }
            if !washout.half_life_1sigma_minutes.is_finite()
                || washout.half_life_1sigma_minutes < 0.0
            {
                return Err(BoronError::InvalidModel(
                    "time_correction.half_life_1sigma_minutes must be non-negative".into(),
                ));
            }
        }
        if let Some(noise) = self.suv_noise_1sigma
            && (!noise.is_finite() || noise < 0.0)
        {
            return Err(BoronError::InvalidModel(
                "suv_noise_1sigma must be a non-negative finite value".into(),
            ));
        }
        Ok(())
    }

    /// Whether this mapping consumes an SUV volume.
    #[must_use]
    pub fn requires_suv(&self) -> bool {
        !matches!(self.mapping, UptakeMapping::Uniform { .. })
    }
}

/// Identity and content bindings stamped on an emitted `BoronField`.
#[derive(Debug, Clone)]
pub struct BoronFieldProvenance {
    /// Field id.
    pub id: String,
    /// Free-text provenance token (e.g. `boron-field:<model-id>`).
    pub provenance_id: String,
    /// The uptake model applied.
    pub model: ContentReference,
    /// The SUV volume consumed; `None` for a `uniform` mapping.
    pub suv_image: Option<ContentReference>,
    /// The registration applied to the SUV volume, when used.
    pub registration: Option<ContentReference>,
}

/// Apply an uptake model over an SUV volume, producing a B-10 field with
/// propagated per-voxel 1σ. `suv` is in grid order `i + nx·j + nx·ny·k`
/// and must be non-negative and finite; pass `None` for a `uniform`
/// mapping. Negative mapped values are clamped to zero and counted.
pub fn apply_uptake_model(
    model: &BoronUptakeModel,
    suv: Option<&[f64]>,
    geometry: &GridGeometry,
    case_id: &str,
    provenance: BoronFieldProvenance,
) -> Result<BoronField, BoronError> {
    model.validate()?;
    geometry.voxel_count()?;
    let n = geometry.voxel_count()?;
    if case_id.trim().is_empty() {
        return Err(BoronError::InvalidField("case_id is empty".into()));
    }

    let suv_noise2 = model.suv_noise_1sigma.unwrap_or(0.0).powi(2);
    let (mut values, mut sigmas) = match &model.mapping {
        UptakeMapping::Uniform {
            concentration_ug_g,
            concentration_1sigma,
        } => (vec![*concentration_ug_g; n], vec![*concentration_1sigma; n]),
        mapping => {
            let suv = suv.ok_or(BoronError::SuvRequired)?;
            if suv.len() != n {
                return Err(BoronError::SuvCountMismatch {
                    suv: suv.len(),
                    grid: n,
                });
            }
            let mut values = Vec::with_capacity(n);
            let mut sigmas = Vec::with_capacity(n);
            for (index, &s) in suv.iter().enumerate() {
                if !s.is_finite() || s < 0.0 {
                    return Err(BoronError::InvalidSuvValue { index });
                }
                let (value, sigma) = match mapping {
                    UptakeMapping::SuvRatio {
                        reference_suv,
                        reference_suv_1sigma,
                        reference_boron_ug_g,
                        reference_boron_1sigma,
                        b10_fraction_of_reference,
                    } => {
                        // B = η·R·S/S_ref; first-order σ over R, S_ref, S.
                        let eta = *b10_fraction_of_reference;
                        let value = eta * reference_boron_ug_g * s / reference_suv;
                        let var = (eta * s / reference_suv * reference_boron_1sigma).powi(2)
                            + (eta * reference_boron_ug_g * s / (reference_suv * reference_suv)
                                * reference_suv_1sigma)
                                .powi(2)
                            + (eta * reference_boron_ug_g / reference_suv).powi(2) * suv_noise2;
                        (value, var.sqrt())
                    }
                    UptakeMapping::LinearSuv {
                        slope_ug_g_per_suv,
                        slope_1sigma,
                        intercept_ug_g,
                        intercept_1sigma,
                    } => {
                        // B = a·S + b; σ over a, b, S.
                        let value = slope_ug_g_per_suv * s + intercept_ug_g;
                        let var = (s * slope_1sigma).powi(2)
                            + intercept_1sigma.powi(2)
                            + (slope_ug_g_per_suv).powi(2) * suv_noise2;
                        (value, var.sqrt())
                    }
                    UptakeMapping::Uniform { .. } => unreachable!(),
                };
                values.push(value);
                sigmas.push(sigma);
            }
            (values, sigmas)
        }
    };

    // Uniform exponential washout: B' = B·f with f = exp(−ln2·Δt/T½);
    // σ gains the half-life term |B·f·λ·Δt/T½|·σ_T½.
    if let Some(washout) = &model.time_correction {
        let lambda = LN2 / washout.half_life_minutes;
        let factor = (-lambda * washout.delta_minutes).exp();
        let dt_half_over_t_half = lambda * washout.delta_minutes / washout.half_life_minutes;
        for (value, sigma) in values.iter_mut().zip(sigmas.iter_mut()) {
            let scaled = *value * factor;
            *sigma = ((*sigma * factor).powi(2)
                + (*value * factor * dt_half_over_t_half * washout.half_life_1sigma_minutes)
                    .powi(2))
            .sqrt();
            *value = scaled;
        }
    }

    let mut clamped = 0u64;
    for (value, sigma) in values.iter_mut().zip(sigmas.iter_mut()) {
        if *value < 0.0 {
            *value = 0.0;
            *sigma = 0.0;
            clamped += 1;
        }
        debug_assert!(sigma.is_finite() && *sigma >= 0.0);
    }

    let field = BoronField {
        schema_version: BORON_FIELD_SCHEMA.into(),
        id: provenance.id,
        case_id: case_id.into(),
        geometry: geometry.clone(),
        values,
        uncertainty_1sigma: sigmas,
        clamped_negative_voxels: clamped,
        model: provenance.model,
        suv_image: provenance.suv_image,
        registration: provenance.registration,
        qualification: BORON_FIELD_QUALIFICATION.into(),
        provenance_id: provenance.provenance_id,
    };
    field.validate()?;
    Ok(field)
}

impl BoronField {
    /// Structural validation; called by consumers.
    pub fn validate(&self) -> Result<(), BoronError> {
        if !openbnct_core::schema_matches(&self.schema_version, BORON_FIELD_SCHEMA) {
            return Err(BoronError::UnsupportedFieldSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(BoronError::InvalidField("field id is empty".into()));
        }
        if self.case_id.trim().is_empty() {
            return Err(BoronError::InvalidField("case_id is empty".into()));
        }
        let n = self.geometry.voxel_count()?;
        if self.values.len() != n {
            return Err(BoronError::InvalidField(format!(
                "values length {} does not match grid voxel count {n}",
                self.values.len()
            )));
        }
        if self.uncertainty_1sigma.len() != n {
            return Err(BoronError::InvalidField(format!(
                "uncertainty_1sigma length {} does not match grid voxel count {n}",
                self.uncertainty_1sigma.len()
            )));
        }
        for (index, value) in self.values.iter().enumerate() {
            if !value.is_finite() || *value < 0.0 {
                return Err(BoronError::InvalidField(format!(
                    "values[{index}] must be a non-negative finite concentration"
                )));
            }
        }
        for (index, sigma) in self.uncertainty_1sigma.iter().enumerate() {
            if !sigma.is_finite() || *sigma < 0.0 {
                return Err(BoronError::InvalidField(format!(
                    "uncertainty_1sigma[{index}] must be non-negative and finite"
                )));
            }
        }
        self.model.validate()?;
        if let Some(reference) = &self.suv_image {
            reference.validate()?;
        }
        if let Some(reference) = &self.registration {
            reference.validate()?;
        }
        if self.qualification.trim().is_empty() {
            return Err(BoronError::InvalidField("qualification is empty".into()));
        }
        Ok(())
    }
}

/// Realize a boron field as a [`openbnct_transport::MaterialAssignment`]:
/// voxels are grouped into `n_tiers` linearly-spaced concentration bins
/// and each tier becomes a voxel-set region whose material is the base
/// material with its B10 mass fraction set to the tier's concentration.
/// Remaining nuclide fractions are renormalized proportionally. The
/// resulting assignment covers every grid voxel, so tiering accuracy —
/// not coverage — is what `n_tiers` controls.
///
/// Per-voxel lattice realization of many tiers is expensive for CSG
/// backends; choose `n_tiers` accordingly.
pub fn materialize_field(
    field: &BoronField,
    base_material: &openbnct_transport::MaterialDefinition,
    case_id: &str,
    n_tiers: u32,
    provenance_id: String,
) -> Result<openbnct_transport::MaterialAssignment, BoronError> {
    use openbnct_transport::{MaterialAssignment, MaterialRegion, MaterialRegionShape};

    field.validate()?;
    base_material
        .validate()
        .map_err(|e| BoronError::InvalidModel(format!("base material: {e}")))?;
    if case_id.trim().is_empty() || case_id != field.case_id {
        return Err(BoronError::InvalidField(format!(
            "case_id {case_id:?} does not match field case {:?}",
            field.case_id
        )));
    }
    if n_tiers == 0 {
        return Err(BoronError::InvalidField(
            "n_tiers must be at least 1".into(),
        ));
    }
    if !base_material
        .nuclides
        .iter()
        .any(|nuclide| nuclide.name == "B10")
    {
        return Err(BoronError::InvalidModel(
            "base material carries no B10 nuclide to scale".into(),
        ));
    }

    let (nx, ny, _nz) = (
        field.geometry.shape[0],
        field.geometry.shape[1],
        field.geometry.shape[2],
    );

    // Linearly spaced edges over [0, max]; zero-concentration voxels are
    // still binned (they land in tier 0, mass fraction ~0).
    let max = field.values.iter().copied().fold(0.0_f64, f64::max);
    let edge = |tier: u32| max * f64::from(tier) / f64::from(n_tiers);
    let mut tier_voxels: Vec<Vec<[u32; 3]>> = (0..n_tiers).map(|_| Vec::new()).collect();
    for (index, &value) in field.values.iter().enumerate() {
        let mut tier = if max > 0.0 {
            ((value / max) * f64::from(n_tiers)) as u32
        } else {
            0
        };
        if tier >= n_tiers {
            tier = n_tiers - 1;
        }
        let i = (index % nx as usize) as u32;
        let j = ((index / nx as usize) % ny as usize) as u32;
        let k = (index / (nx as usize * ny as usize)) as u32;
        tier_voxels[tier as usize].push([i, j, k]);
    }

    // Tier concentration = bin center; a tier's material replaces the B10
    // mass fraction and renormalizes the rest proportionally.
    let base_b10 = base_material
        .nuclides
        .iter()
        .find(|nuclide| nuclide.name == "B10")
        .map(|nuclide| nuclide.mass_fraction)
        .unwrap_or(0.0);
    let other_sum = 1.0 - base_b10;

    let mut regions = Vec::new();
    for (tier, indices) in tier_voxels.into_iter().enumerate() {
        if indices.is_empty() {
            continue;
        }
        let tier = tier as u32;
        let concentration = (edge(tier) + edge(tier + 1)) / 2.0;
        let b10_fraction = concentration * 1.0e-6;
        if b10_fraction >= 1.0 {
            return Err(BoronError::InvalidField(
                "boron concentration exceeds 1e6 µg/g — nonphysical".into(),
            ));
        }
        let scale = (1.0 - b10_fraction) / other_sum;
        let nuclides = base_material
            .nuclides
            .iter()
            .map(|nuclide| openbnct_transport::NuclideMassFraction {
                name: nuclide.name.clone(),
                mass_fraction: if nuclide.name == "B10" {
                    b10_fraction
                } else {
                    nuclide.mass_fraction * scale
                },
            })
            .collect();
        let mut material = base_material.clone();
        material.id = format!("{}-b10-tier-{tier}", base_material.id);
        material.nuclides = nuclides;
        regions.push(MaterialRegion {
            name: format!("boron-tier-{tier}"),
            material,
            shape: MaterialRegionShape::VoxelSet { indices },
        });
    }
    if regions.is_empty() {
        return Err(BoronError::InvalidField(
            "boron field produced no populated tiers".into(),
        ));
    }

    Ok(MaterialAssignment {
        schema_version: openbnct_transport::MATERIAL_ASSIGNMENT_SCHEMA.into(),
        case_id: case_id.into(),
        base_material: base_material.clone(),
        regions,
        provenance_id,
    })
}

pub mod microdistribution;

pub use microdistribution::{
    BORON_MICRODISTRIBUTION_SCHEMA, BoronMicrodistribution, CompartmentDeposition,
    CompartmentFractions, CorrectionDeposition, MICRODISTRIBUTION_CORRECTION_QUALIFICATION,
    MICRODISTRIBUTION_CORRECTION_SCHEMA, MicrodistributionCorrection, MicrodistributionError,
    evaluate_microdistribution,
};

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_transport::NeutronThermalTreatment;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 4, 2],
            spacing_mm: [5.0, 5.0, 5.0],
            origin_mm: [-10.0, -10.0, -5.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn reference(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    fn ratio_model() -> BoronUptakeModel {
        BoronUptakeModel {
            schema_version: BORON_UPTAKE_MODEL_SCHEMA.into(),
            id: "m-ratio".into(),
            mapping: UptakeMapping::SuvRatio {
                reference_suv: 2.0,
                reference_suv_1sigma: 0.1,
                reference_boron_ug_g: 12.0,
                reference_boron_1sigma: 0.6,
                b10_fraction_of_reference: 1.0,
            },
            time_correction: None,
            suv_noise_1sigma: None,
            validity_domain: "F-BPA PET, SUV at 2h post-infusion".into(),
            provenance_id: "test".into(),
        }
    }

    fn provenance(suv: Option<&[f64]>) -> BoronFieldProvenance {
        BoronFieldProvenance {
            id: "field-1".into(),
            provenance_id: "test".into(),
            model: reference("model"),
            suv_image: suv.map(|_| reference("suv")),
            registration: None,
        }
    }

    fn apply(model: &BoronUptakeModel, suv: Option<&[f64]>) -> BoronField {
        apply_uptake_model(model, suv, &geometry(), "case-1", provenance(suv)).unwrap()
    }

    #[test]
    fn suv_ratio_scales_linearly_and_propagates_sigma() {
        let n = geometry().voxel_count().unwrap();
        let suv = vec![4.0; n]; // SUV/S_ref = 2 -> B10 = 24 µg/g
        let field = apply(&ratio_model(), Some(&suv));
        assert_eq!(field.values.len(), n);
        for &v in &field.values {
            assert!((v - 24.0).abs() < 1e-12);
        }
        // σ² = (S/S_ref)²σ_R² + (R·S/S_ref²)²σ_Sref² = (2·0.6)² + (12·4/4)²·0.01
        let want = ((2.0 * 0.6f64).powi(2) + (12.0f64).powi(2) * 0.01).sqrt();
        for &s in &field.uncertainty_1sigma {
            assert!((s - want).abs() < 1e-9, "{s} vs {want}");
        }
        assert_eq!(field.clamped_negative_voxels, 0);
        assert_eq!(field.qualification, BORON_FIELD_QUALIFICATION);
    }

    #[test]
    fn b10_fraction_scales_reference() {
        let mut model = ratio_model();
        let UptakeMapping::SuvRatio {
            b10_fraction_of_reference,
            ..
        } = &mut model.mapping
        else {
            panic!()
        };
        *b10_fraction_of_reference = 0.99;
        let suv = vec![2.0; geometry().voxel_count().unwrap()];
        let field = apply(&model, Some(&suv));
        assert!((field.values[0] - 0.99 * 12.0).abs() < 1e-12);
    }

    #[test]
    fn linear_mapping_applies_and_clamps_negatives() {
        let model = BoronUptakeModel {
            schema_version: BORON_UPTAKE_MODEL_SCHEMA.into(),
            id: "m-lin".into(),
            mapping: UptakeMapping::LinearSuv {
                slope_ug_g_per_suv: 5.0,
                slope_1sigma: 0.5,
                intercept_ug_g: -1.0,
                intercept_1sigma: 0.2,
            },
            time_correction: None,
            suv_noise_1sigma: Some(0.05),
            validity_domain: "calibrated regression".into(),
            provenance_id: "test".into(),
        };
        let mut suv = vec![3.0; geometry().voxel_count().unwrap()];
        suv[0] = 0.1; // 5*0.1 - 1 = -0.5 -> clamps
        let field = apply(&model, Some(&suv));
        assert_eq!(field.clamped_negative_voxels, 1);
        assert_eq!(field.values[0], 0.0);
        assert!((field.values[1] - 14.0).abs() < 1e-12);
        // σ² = S²σ_a² + σ_b² + a²σ_S² = 9·0.25 + 0.04 + 25·0.0025
        let want = (9.0 * 0.25 + 0.04 + 25.0 * 0.0025f64).sqrt();
        assert!((field.uncertainty_1sigma[1] - want).abs() < 1e-9);
    }

    #[test]
    fn washout_decays_and_adds_half_life_sigma() {
        let mut model = ratio_model();
        model.time_correction = Some(WashoutCorrection {
            delta_minutes: 60.0,
            half_life_minutes: 120.0,
            half_life_1sigma_minutes: 6.0,
        });
        let suv = vec![4.0; geometry().voxel_count().unwrap()];
        let field = apply(&model, Some(&suv));
        let f = 2f64.powf(-0.5); // 60 min at T½=120
        for &v in &field.values {
            assert!((v - 24.0 * f).abs() < 1e-9);
        }
        // σ grows by the half-life term; base σ was ~1.6125
        let base = ((2.0 * 0.6f64).powi(2) + 1.44).sqrt() * f;
        let hl = 24.0 * f * (LN2 * 60.0 / 120.0 / 120.0) * 6.0;
        let want = (base * base + hl * hl).sqrt();
        assert!((field.uncertainty_1sigma[0] - want).abs() < 1e-9);
    }

    #[test]
    fn uniform_needs_no_suv() {
        let model = BoronUptakeModel {
            schema_version: BORON_UPTAKE_MODEL_SCHEMA.into(),
            id: "m-uni".into(),
            mapping: UptakeMapping::Uniform {
                concentration_ug_g: 40.0,
                concentration_1sigma: 4.0,
            },
            time_correction: None,
            suv_noise_1sigma: None,
            validity_domain: "assumed uptake baseline".into(),
            provenance_id: "test".into(),
        };
        assert!(!model.requires_suv());
        let field = apply(&model, None);
        assert!(field.values.iter().all(|&v| v == 40.0));
        assert!(field.suv_image.is_none());
    }

    #[test]
    fn rejects_invalid_inputs() {
        // Empty validity domain
        let mut model = ratio_model();
        model.validity_domain = "  ".into();
        assert!(model.validate().is_err());
        // Negative SUV
        let mut suv = vec![1.0; geometry().voxel_count().unwrap()];
        suv[3] = -0.5;
        assert!(matches!(
            apply_uptake_model(
                &ratio_model(),
                Some(&suv),
                &geometry(),
                "case-1",
                provenance(Some(&suv))
            ),
            Err(BoronError::InvalidSuvValue { index: 3 })
        ));
        // SUV count mismatch
        let short = [1.0, 2.0];
        assert!(matches!(
            apply_uptake_model(
                &ratio_model(),
                Some(&short),
                &geometry(),
                "case-1",
                provenance(Some(&short))
            ),
            Err(BoronError::SuvCountMismatch { .. })
        ));
        // Missing SUV for a mapping that needs it
        assert!(matches!(
            apply_uptake_model(
                &ratio_model(),
                None,
                &geometry(),
                "case-1",
                provenance(None)
            ),
            Err(BoronError::SuvRequired)
        ));
    }

    #[test]
    fn materialize_bins_tiers_and_scales_b10() {
        let suv: Vec<f64> = (0..32).map(|i| i as f64 / 8.0).collect(); // 0..3.875
        let field = apply(&ratio_model(), Some(&suv));
        let base = openbnct_transport::MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: "tissue".into(),
            density_g_cm3: 1.0,
            temperature_k: 293.6,
            nuclides: vec![
                openbnct_transport::NuclideMassFraction {
                    name: "H1".into(),
                    mass_fraction: 0.1,
                },
                openbnct_transport::NuclideMassFraction {
                    name: "O16".into(),
                    mass_fraction: 0.9 - 4e-5,
                },
                openbnct_transport::NuclideMassFraction {
                    name: "B10".into(),
                    mass_fraction: 4e-5,
                },
            ],
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        };
        let assignment = materialize_field(&field, &base, "case-1", 4, "test".into()).unwrap();
        assignment.validate(&field.geometry).unwrap();
        assert_eq!(assignment.regions.len(), 4);
        // Every voxel assigned exactly once
        let covered: usize = assignment.regions.iter().map(|r| r.voxel_count()).sum();
        assert_eq!(covered, 32);
        // Highest tier has the largest B10 mass fraction
        let top = &assignment.regions[3].material;
        let b10 = top
            .nuclides
            .iter()
            .find(|n| n.name == "B10")
            .unwrap()
            .mass_fraction;
        let bottom = &assignment.regions[0].material;
        let b10_low = bottom
            .nuclides
            .iter()
            .find(|n| n.name == "B10")
            .unwrap()
            .mass_fraction;
        assert!(b10 > b10_low);
        // Mass fractions still sum to 1
        for region in &assignment.regions {
            let sum: f64 = region
                .material
                .nuclides
                .iter()
                .map(|n| n.mass_fraction)
                .sum();
            assert!((sum - 1.0).abs() < 1e-12);
        }
    }
}
