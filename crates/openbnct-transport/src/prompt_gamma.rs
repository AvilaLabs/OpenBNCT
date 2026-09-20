// SPDX-License-Identifier: Apache-2.0

//! Prompt-gamma production source derived from a boron dose field.
//!
//! The ¹⁰B(n,α)⁷Li reaction emits a 478 keV de-excitation photon in
//! ~94% of captures (the (n,α₁γ) branch). The BNCT-SPECT and
//! Compton-camera communities consume exactly this spatial production
//! map for detector design and reconstruction research — it is the
//! physics source term for in-vivo dose monitoring, not an image.
//!
//! Derivation is the exact inverse of the charged-kerma convention the
//! dose pipeline already uses (`mgcollapse` — branch-weighted 2.34 MeV
//! charged release per capture):
//!
//! ```text
//! captures/kg = D_boron [Gy] / (E_charged [J] per capture)
//! photons/kg  = captures/kg × branching_ratio
//! ```
//!
//! The artifact is a yield map — emissions per unit mass per unit of
//! parent dose — not a rate; dose bundles carry no time axis. For
//! `GrayPerSourceParticle` parents the unit is photons per kg per
//! source particle.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, DoseComponent, DoseUnit, GridGeometry, PhysicalDoseBundle};

/// Versioned contract id for `PromptGammaSource`.
pub const PROMPT_GAMMA_SOURCE_SCHEMA: &str = "openbnct.prompt-gamma-source/0.1.0";

/// Qualification carried by every prompt-gamma source document.
pub const PROMPT_GAMMA_QUALIFICATION: &str = "prompt_gamma_production_research_only_not_clinical";

/// Charged-particle kerma per ¹⁰B capture — branch-weighted
/// 0.94 × 2.31 MeV + 0.06 × 2.79 MeV ≈ 2.34 MeV, matching the boron
/// dose convention in `openbnct-openmc`'s multigroup collapse.
const B10_CHARGED_KERMA_EV: f64 = 2.34e6;
/// Fraction of captures emitting the 478 keV de-excitation photon.
const B10_GAMMA_BRANCH: f64 = 0.94;
/// De-excitation photon energy.
pub const PROMPT_GAMMA_ENERGY_EV: f64 = 478_000.0;
const EV_TO_J: f64 = 1.602_176_634e-19;

/// Photons per kg per capture-normalizing unit of the parent dose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptGammaUnit {
    /// Parent boron dose was absolute Gray — yields are photons/kg.
    PhotonsPerKg,
    /// Parent boron dose was Gy/source-particle — yields are
    /// photons/kg per source particle.
    PhotonsPerKgPerSourceParticle,
}

/// Spatial 478 keV production map on the parent bundle's grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaSource {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub frame_of_reference_uid: Option<String>,
    /// The grid this map lives on — identical to the parent bundle's.
    pub geometry: GridGeometry,
    /// Content binding to the dose bundle this was derived from.
    pub parent: ContentReference,
    /// Emission photon energy — 478 000 eV.
    pub emission_energy_ev: f64,
    /// Declared branch fraction used in the derivation.
    pub branching_ratio: f64,
    /// Declared charged-kerma-per-capture used to invert dose → captures.
    pub charged_kerma_per_capture_ev: f64,
    pub unit: PromptGammaUnit,
    /// Expected photon emissions per kg of voxel mass per dose unit,
    /// in the bundle's grid order.
    pub values: Vec<f64>,
    /// 1σ in the same unit, propagated from the boron component's
    /// absolute standard uncertainty (the mapping is linear).
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    pub provenance_id: String,
    pub qualification: String,
}

#[derive(Debug)]
pub enum PromptGammaError {
    MissingBoronComponent,
    EmptyIdentifier(&'static str),
    NonFiniteValue,
}

impl std::fmt::Display for PromptGammaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBoronComponent => {
                write!(f, "parent bundle carries no boron dose component")
            }
            Self::EmptyIdentifier(label) => write!(f, "empty identifier: {label}"),
            Self::NonFiniteValue => write!(f, "non-finite value in boron dose field"),
        }
    }
}

impl std::error::Error for PromptGammaError {}

impl PromptGammaSource {
    pub fn validate(&self) -> Result<(), PromptGammaError> {
        for (label, value) in [
            ("schema_version", self.schema_version.as_str()),
            ("id", self.id.as_str()),
            ("case_id", self.case_id.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(PromptGammaError::EmptyIdentifier(label));
            }
        }
        let voxels = self
            .geometry
            .voxel_count()
            .map_err(|_| PromptGammaError::NonFiniteValue)?;
        if self.values.len() != voxels {
            return Err(PromptGammaError::NonFiniteValue);
        }
        if self.values.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(PromptGammaError::NonFiniteValue);
        }
        if let Some(sigma) = &self.absolute_standard_uncertainty
            && (sigma.len() != voxels || sigma.iter().any(|s| !s.is_finite() || *s < 0.0))
        {
            return Err(PromptGammaError::NonFiniteValue);
        }
        Ok(())
    }
}

/// Derive the 478 keV production map from a dose bundle's boron
/// component. `parent` must bind the exact bundle bytes (id + sha256)
/// so the source map's provenance chain stays unforgeable.
pub fn derive_prompt_gamma_source(
    bundle: &PhysicalDoseBundle,
    id: &str,
    parent: ContentReference,
    provenance_id: &str,
) -> Result<PromptGammaSource, PromptGammaError> {
    let boron = bundle
        .components
        .iter()
        .find(|volume| volume.component == DoseComponent::Boron)
        .ok_or(PromptGammaError::MissingBoronComponent)?;

    // D [Gy] = captures/kg × E_charged [J]  →  photons/kg = D/E_J × branch
    let scale = B10_GAMMA_BRANCH / (B10_CHARGED_KERMA_EV * EV_TO_J);
    let values: Vec<f64> = boron.values.iter().map(|d| d * scale).collect();
    if values.iter().any(|v| !v.is_finite()) {
        return Err(PromptGammaError::NonFiniteValue);
    }
    let sigma = boron
        .absolute_standard_uncertainty
        .as_ref()
        .map(|s| s.iter().map(|v| v * scale).collect());
    let unit = match boron.unit {
        DoseUnit::Gray => PromptGammaUnit::PhotonsPerKg,
        DoseUnit::GrayPerSourceParticle => PromptGammaUnit::PhotonsPerKgPerSourceParticle,
    };

    let source = PromptGammaSource {
        schema_version: PROMPT_GAMMA_SOURCE_SCHEMA.into(),
        id: id.into(),
        case_id: bundle.case_id.clone(),
        frame_of_reference_uid: bundle.frame_of_reference_uid.clone(),
        geometry: bundle.geometry.clone(),
        parent,
        emission_energy_ev: PROMPT_GAMMA_ENERGY_EV,
        branching_ratio: B10_GAMMA_BRANCH,
        charged_kerma_per_capture_ev: B10_CHARGED_KERMA_EV,
        unit,
        values,
        absolute_standard_uncertainty: sigma,
        provenance_id: provenance_id.into(),
        qualification: PROMPT_GAMMA_QUALIFICATION.into(),
    };
    source.validate()?;
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{
        DoseVolume, PhysicalDoseBundle, PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

    fn bundle(unit: DoseUnit) -> PhysicalDoseBundle {
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
            component_profile: ContentReference {
                id: "profile".into(),
                sha256: "0".repeat(64),
            },
            response_set: ContentReference {
                id: "resp".into(),
                sha256: "1".repeat(64),
            },
            components: vec![
                DoseVolume {
                    component: DoseComponent::Boron,
                    unit,
                    values: vec![1.0, 0.5],
                    absolute_standard_uncertainty: Some(vec![0.1, 0.05]),
                },
                DoseVolume {
                    component: DoseComponent::Photon,
                    unit,
                    values: vec![2.0, 1.0],
                    absolute_standard_uncertainty: None,
                },
            ],
            physical_total: PhysicalTotalDoseVolume {
                unit,
                values: vec![3.0, 1.5],
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
            },
            provenance_id: "prov".into(),
        }
    }

    #[test]
    fn derives_photon_yield_from_boron_dose() {
        let source = derive_prompt_gamma_source(
            &bundle(DoseUnit::GrayPerSourceParticle),
            "pg-1",
            ContentReference {
                id: "bundle".into(),
                sha256: "a".repeat(64),
            },
            "pg-prov",
        )
        .unwrap();
        assert_eq!(source.unit, PromptGammaUnit::PhotonsPerKgPerSourceParticle);
        // 1 Gy → 1/(2.34e6 eV × 1.602e-19 J/eV) captures/kg × 0.94 photons
        let expected = 0.94 / (2.34e6 * 1.602_176_634e-19);
        assert!((source.values[0] - expected).abs() < expected * 1e-9);
        assert!((source.values[1] - expected * 0.5).abs() < expected * 1e-9);
        assert!(
            (source.absolute_standard_uncertainty.as_ref().unwrap()[0] - expected * 0.1).abs()
                < expected * 1e-9
        );
        assert_eq!(source.emission_energy_ev, 478_000.0);
        source.validate().unwrap();
    }

    #[test]
    fn rejects_bundle_without_boron() {
        let mut bundle = bundle(DoseUnit::Gray);
        bundle
            .components
            .retain(|v| v.component != DoseComponent::Boron);
        assert!(matches!(
            derive_prompt_gamma_source(
                &bundle,
                "pg",
                ContentReference {
                    id: "b".into(),
                    sha256: "a".repeat(64)
                },
                "p"
            ),
            Err(PromptGammaError::MissingBoronComponent)
        ));
    }
}
