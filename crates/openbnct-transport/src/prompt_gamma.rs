// SPDX-License-Identifier: MIT

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
    ShapeMismatch(&'static str),
    DetectorOutsideGeometry,
}

impl std::fmt::Display for PromptGammaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBoronComponent => {
                write!(f, "parent bundle carries no boron dose component")
            }
            Self::EmptyIdentifier(label) => write!(f, "empty identifier: {label}"),
            Self::NonFiniteValue => write!(f, "non-finite value in boron dose field"),
            Self::ShapeMismatch(label) => write!(f, "shape mismatch: {label}"),
            Self::DetectorOutsideGeometry => {
                write!(f, "detector voxel outside the case geometry")
            }
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

/// Versioned contract id for `PromptGammaResponse`.
pub const PROMPT_GAMMA_RESPONSE_SCHEMA: &str = "openbnct.pg-response/0.1.0";

/// Versioned contract id for `PromptGammaCounts`.
pub const PROMPT_GAMMA_COUNTS_SCHEMA: &str = "openbnct.pg-counts/0.1.0";

/// Detector-response map for a declared voxel region: the adjoint
/// solution's value at the 478 keV emission group in every cell —
/// the importance of a photon born there to a fluence-weighted
/// tally on the detector region. This is the response-matrix column
/// for the declared position: expected tally under an emission map
/// `q` [photons/cm³] is Σ_v q(v)·sensitivity(v)·V_v.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaResponse {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub geometry: GridGeometry,
    /// Detector region as voxel indices on the case grid.
    pub detector_voxels: Vec<[u32; 3]>,
    /// Photon group carrying the emission line the response was
    /// solved for — the group containing `emission_energy_ev`.
    pub emission_group: u32,
    pub emission_energy_ev: f64,
    /// Adjoint sensitivity at `emission_group` per voxel, grid order —
    /// detector tally per unit volumetric emission density.
    pub sensitivity: Vec<f64>,
    /// Quadrature and convergence of the adjoint solve.
    pub quadrature_order: u32,
    pub converged: bool,
    pub residual: f64,
    pub outer_iterations: u32,
    /// Content bindings to the transport case and photon data the
    /// adjoint was solved against.
    pub case: ContentReference,
    pub photon_data: ContentReference,
    pub provenance_id: String,
    pub qualification: String,
}

/// Expected detector tally under a prompt-gamma emission map —
/// `Σ_v emission(v)·voxel_mass(v)·sensitivity(v)`, the forward-model
/// half of the PG reconstruction chain. Absolute detector efficiency
/// (crystal volume, collimation) enters only as a declared scalar
/// calibration — transport through the phantom is already carried by
/// the response map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaCounts {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// Expected fluence-weighted tally in the emission map's dose
    /// unit — per source particle for `PhotonsPerKgPerSourceParticle`
    /// parents.
    pub expected_tally: f64,
    /// Declared efficiency calibration applied to the raw tally.
    pub detector_efficiency: f64,
    /// Declared uniform voxel density used to convert the per-kg
    /// emission map into volumetric sources.
    pub voxel_density_kg_per_m3: f64,
    pub emission: ContentReference,
    pub response: ContentReference,
    pub provenance_id: String,
    pub qualification: String,
}

impl PromptGammaResponse {
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
        let [nx, ny, nz] = self.geometry.shape;
        for v in &self.detector_voxels {
            if v[0] >= nx || v[1] >= ny || v[2] >= nz {
                return Err(PromptGammaError::DetectorOutsideGeometry);
            }
        }
        let voxels = self
            .geometry
            .voxel_count()
            .map_err(|_| PromptGammaError::NonFiniteValue)?;
        if self.sensitivity.len() != voxels
            || self.sensitivity.iter().any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(PromptGammaError::NonFiniteValue);
        }
        Ok(())
    }
}

/// Expected detector tally under an emission map — the linear inner
/// product `Σ_v emission(v)·mass(v)·sensitivity(v)` with the declared
/// uniform density and an optional efficiency calibration. Units
/// follow the emission map (per source particle for
/// `PhotonsPerKgPerSourceParticle`).
#[allow(clippy::too_many_arguments)]
pub fn expected_prompt_gamma_counts(
    emission: &PromptGammaSource,
    response: &PromptGammaResponse,
    voxel_density_kg_per_m3: f64,
    detector_efficiency: f64,
    id: &str,
    emission_ref: ContentReference,
    response_ref: ContentReference,
    provenance_id: &str,
) -> Result<PromptGammaCounts, PromptGammaError> {
    emission.validate()?;
    response.validate()?;
    if emission.geometry != response.geometry {
        return Err(PromptGammaError::ShapeMismatch(
            "emission and response grids differ",
        ));
    }
    if (emission.emission_energy_ev - response.emission_energy_ev).abs() > 1.0 {
        return Err(PromptGammaError::ShapeMismatch(
            "emission and response energies differ",
        ));
    }
    if !voxel_density_kg_per_m3.is_finite()
        || voxel_density_kg_per_m3 <= 0.0
        || !detector_efficiency.is_finite()
        || detector_efficiency <= 0.0
    {
        return Err(PromptGammaError::NonFiniteValue);
    }
    let spacing = &emission.geometry.spacing_mm;
    let voxel_m3 = spacing[0] * spacing[1] * spacing[2] * 1e-9;
    let voxel_mass_kg = voxel_density_kg_per_m3 * voxel_m3;
    let mut tally = 0.0;
    for (e, s) in emission.values.iter().zip(response.sensitivity.iter()) {
        tally += e * voxel_mass_kg * s;
    }
    if !tally.is_finite() {
        return Err(PromptGammaError::NonFiniteValue);
    }
    Ok(PromptGammaCounts {
        schema_version: PROMPT_GAMMA_COUNTS_SCHEMA.into(),
        id: id.into(),
        case_id: emission.case_id.clone(),
        expected_tally: tally * detector_efficiency,
        detector_efficiency,
        voxel_density_kg_per_m3,
        emission: emission_ref,
        response: response_ref,
        provenance_id: provenance_id.into(),
        qualification: PROMPT_GAMMA_QUALIFICATION.into(),
    })
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
    fn folds_emission_into_expected_counts() {
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
        let response = PromptGammaResponse {
            schema_version: PROMPT_GAMMA_RESPONSE_SCHEMA.into(),
            id: "resp".into(),
            case_id: "case".into(),
            geometry: source.geometry.clone(),
            detector_voxels: vec![[0, 0, 0]],
            emission_group: 7,
            emission_energy_ev: 478_000.0,
            sensitivity: vec![0.25, 0.5],
            quadrature_order: 8,
            converged: true,
            residual: 1e-7,
            outer_iterations: 3,
            case: ContentReference {
                id: "case".into(),
                sha256: "b".repeat(64),
            },
            photon_data: ContentReference {
                id: "data".into(),
                sha256: "c".repeat(64),
            },
            provenance_id: "resp-prov".into(),
            qualification: PROMPT_GAMMA_QUALIFICATION.into(),
        };
        // 1mm³ voxels at 1000 kg/m³ → 1e-6 kg per voxel.
        let counts = expected_prompt_gamma_counts(
            &source,
            &response,
            1000.0,
            0.5,
            "counts",
            ContentReference {
                id: "em".into(),
                sha256: "d".repeat(64),
            },
            ContentReference {
                id: "rs".into(),
                sha256: "e".repeat(64),
            },
            "counts-prov",
        )
        .unwrap();
        let expected = (source.values[0] * 0.25 + source.values[1] * 0.5) * 1e-6 * 0.5;
        assert!((counts.expected_tally - expected).abs() < expected * 1e-12);
        assert_eq!(counts.detector_efficiency, 0.5);
        assert_eq!(counts.voxel_density_kg_per_m3, 1000.0);
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
