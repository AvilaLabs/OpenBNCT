// SPDX-License-Identifier: Apache-2.0

//! Microdosimetric-kinetic model (MKM) family — a stochastic
//! microdosimetry model kept deliberately distinct from the
//! component-weight and photon-isoeffective families in
//! `openbnct.biological-model/0.2.0`.
//!
//! Two versioned artifacts live here:
//!
//! - `openbnct.lineal-spectrum/0.1.0` — a binned lineal-energy spectrum
//!   (TEPC-derived or published) with an explicit event-frequency or
//!   dose-weighted interpretation, from which the dose-mean lineal
//!   energy ȳ_D is computed exactly under the piecewise-constant bin
//!   assumption.
//! - `openbnct.microdosimetric-model/0.1.0` — per-component MKM
//!   parameters (α₀, β, and each component's lineal-energy source),
//!   the spherical-domain geometry, a mandatory free-text validity
//!   domain, and optional LQ fractionation.
//!
//! Applying an MKM model converts each physical dose component to a
//! photon-equivalent dose through the cell system the parameters
//! describe: with the standard linearized MKM relation
//! `α* = α₀ + β·z̄₁D` and `z̄₁D = ȳ_D/(ρ·π·r_d²)` for a spherical
//! domain, component effect `X = α*·d + β·d²` is inverted through the
//! photon LQ `α₀·D + β·D² = X` to yield the component's
//! photon-equivalent dose. The biological total is their sum.
//!
//! This remains a research-only layer: outputs are labeled
//! `mkm_weighted_*` (never `gray` or `eqd2` alone), carry the
//! `microdosimetric_kinetic` weight semantics, and assert no clinical
//! RBE or Gy-Eq claim.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::PI;

use openbnct_core::{ContentReference, DoseComponent, DoseUnit, PhysicalDoseBundle, RegionMask};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    AppliedFractionation, BIOLOGICAL_DOSE_BUNDLE_SCHEMA, BioError, BiologicalDoseBundle,
    BiologicalTotal, BiologicalUncertaintyMethod, Fractionation, WeightSemantics,
    WeightedDoseVolume, component_name,
};

/// Versioned lineal-energy spectrum artifact schema.
pub const LINEAL_SPECTRUM_SCHEMA: &str = "openbnct.lineal-spectrum/0.1.0";
/// Versioned microdosimetric-kinetic model artifact schema.
pub const MICRODOSIMETRIC_MODEL_SCHEMA: &str = "openbnct.microdosimetric-model/0.1.0";

/// Joules per keV (exact SI conversion constant).
const JOULE_PER_KEV: f64 = 1.602_176_634e-16;

/// How a binned lineal spectrum's bin contents are weighted.
///
/// Microdosimetry distinguishes the event-frequency density `f(y)` —
/// each lineal-energy event weighted equally — from the dose density
/// `d(y) ∝ y·f(y)`. The dose-mean lineal energy ȳ_D is
/// `∫y·d(y)dy/∫d(y)dy` in dose weighting and
/// `∫y²f(y)dy/∫y f(y)dy` in event-frequency weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinealWeighting {
    /// Bin contents are event counts or an event-frequency density.
    EventFrequency,
    /// Bin contents are dose-weighted (d(y)); ȳ_D is the direct mean.
    DoseWeighted,
}

/// A binned lineal-energy spectrum — TEPC-derived or published.
///
/// `bin_edges_kev_um` holds `n+1` strictly increasing edges in keV/µm
/// and `values` the `n` piecewise-constant bin contents. `value_unit`
/// records what the contents mean physically (event counts, relative
/// frequency, dose fraction); moments do not depend on it, so it is
/// provenance, not an enforced contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinealSpectrum {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// `n+1` strictly increasing bin edges in keV/µm.
    pub bin_edges_kev_um: Vec<f64>,
    /// `n` bin contents, piecewise-constant over each bin.
    pub values: Vec<f64>,
    /// Optional per-bin one-sigma uncertainties in `value_unit`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    pub weighting: LinealWeighting,
    /// Free-text unit of `values` (e.g. `"events"`,
    /// `"relative_frequency"`, `"dose_fraction"`).
    pub value_unit: String,
    /// Where the spectrum came from — a measurement record, a
    /// published digitization, or a transport estimate.
    pub derivation: Option<ContentReference>,
    /// Free-text provenance note (instrument, site size, publication).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl LinealSpectrum {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, LINEAL_SPECTRUM_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() {
            return Err(BioError::Invalid("lineal spectrum id is empty".into()));
        }
        if self.bin_edges_kev_um.len() < 2
            || self.bin_edges_kev_um.len() != self.values.len() + 1
            || !self
                .bin_edges_kev_um
                .iter()
                .all(|e| e.is_finite() && *e >= 0.0)
            || !self.bin_edges_kev_um.windows(2).all(|w| w[1] > w[0])
            || self.values.iter().any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(BioError::Invalid(format!(
                "lineal spectrum {:?} is malformed: need n+1 finite increasing non-negative edges and n non-negative contents",
                self.id
            )));
        }
        if let Some(sigma) = &self.absolute_standard_uncertainty
            && (sigma.len() != self.values.len()
                || sigma.iter().any(|v| !v.is_finite() || *v < 0.0))
        {
            return Err(BioError::Invalid(format!(
                "lineal spectrum {:?} uncertainty is malformed",
                self.id
            )));
        }
        if self.value_unit.trim().is_empty() {
            return Err(BioError::Invalid(
                "lineal spectrum value_unit is empty".into(),
            ));
        }
        if let Some(derivation) = &self.derivation {
            derivation.validate().map_err(|_| {
                BioError::Invalid("spectrum derivation reference is invalid".into())
            })?;
        }
        Ok(())
    }

    /// `∫values(y)·y^k dy` under the piecewise-constant bin assumption:
    /// each bin contributes `v_i·(e_{i+1}^{k+1} - e_i^{k+1})/(k+1)`.
    fn moment(&self, k: u32) -> f64 {
        let p = f64::from(k + 1);
        self.values
            .iter()
            .zip(self.bin_edges_kev_um.windows(2))
            .map(|(v, e)| v * (e[1].powf(p) - e[0].powf(p)) / p)
            .sum()
    }

    /// Dose-mean lineal energy ȳ_D in keV/µm. For a dose-weighted
    /// spectrum this is `∫y·d(y)/∫d(y)`; for an event-frequency
    /// spectrum it is `∫y²f(y)/∫y f(y)` — the quantity MKM's z̄₁D
    /// conversion consumes.
    pub fn dose_mean_kev_um(&self) -> Result<f64, BioError> {
        self.validate()?;
        let (numerator, denominator) = match self.weighting {
            LinealWeighting::DoseWeighted => (self.moment(1), self.moment(0)),
            LinealWeighting::EventFrequency => (self.moment(2), self.moment(1)),
        };
        if denominator.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err(BioError::Invalid(format!(
                "lineal spectrum {:?} has zero weight; dose-mean lineal energy is undefined",
                self.id
            )));
        }
        Ok(numerator / denominator)
    }

    /// Frequency-mean lineal energy ȳ_F = `∫y·f(y)/∫f(y)` — defined
    /// only for event-frequency spectra; a dose-weighted spectrum
    /// cannot recover the event distribution.
    pub fn frequency_mean_kev_um(&self) -> Result<Option<f64>, BioError> {
        self.validate()?;
        if self.weighting == LinealWeighting::DoseWeighted {
            return Ok(None);
        }
        let denominator = self.moment(0);
        if denominator.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err(BioError::Invalid(format!(
                "lineal spectrum {:?} has zero weight; frequency-mean lineal energy is undefined",
                self.id
            )));
        }
        Ok(Some(self.moment(1) / denominator))
    }
}

/// A spectrum document supplied to `apply_microdosimetric_model`,
/// paired with its raw bytes so the emitted bundle can bind it by
/// content hash.
pub struct SpectrumInput<'a> {
    pub spectrum: &'a LinealSpectrum,
    pub document_bytes: &'a [u8],
}

/// Where one component's dose-mean lineal energy comes from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinealEnergySource {
    /// A published or derived constant ȳ_D in keV/µm.
    Constant {
        dose_mean_lineal_energy_kev_um: f64,
        /// Free-text origin note (spectrum, publication, instrument).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// ȳ_D computed at apply time from a `openbnct.lineal-spectrum`
    /// document supplied via `--spectrum`; `spectrum_id` is the
    /// spectrum document's `id`.
    Spectrum { spectrum_id: String },
}

/// MKM parameters for one physical dose component.
///
/// `alpha_0` and `beta` are the cell system's photon (low-LET)
/// linear-quadratic parameters — in the linearized MKM the
/// domain-invariant β equals the photon β, and α₀ is the photon α —
/// so they double as the reference-radiation response when the
/// component dose is inverted to photon-equivalent dose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MkmComponent {
    /// Intrinsic α at zero lineal energy (Gy⁻¹); the cell's photon α.
    pub alpha_0: f64,
    /// Domain-invariant β (Gy⁻²); the cell's photon β.
    pub beta: f64,
    /// This component's radiation quality as a lineal-energy source.
    pub lineal_energy: LinealEnergySource,
}

/// Per-region override of the cell system's LQ parameters.
///
/// Lineal energies are beam/component physics and stay global; what
/// legitimately varies between tissues is radiosensitivity, so a
/// region override replaces α₀ and β for every component inside the
/// mask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MkmLq {
    pub alpha_0: f64,
    pub beta: f64,
}

/// A separately versioned microdosimetric-kinetic model artifact.
///
/// Unlike `BiologicalModel`, the validity domain is mandatory: MKM
/// parameters are meaningless without stating the cell system, dose
/// range, and radiation qualities they were derived for. The domain
/// text is recorded for provenance; out-of-domain *parameters* are
/// rejected by validation, but the model cannot detect an
/// out-of-domain application — that responsibility stays with the
/// operator and is why the qualification boundary exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicrodosimetricModel {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The physical-bundle unit this model consumes.
    pub input_unit: DoseUnit,
    /// MKM spherical domain radius in micrometers.
    pub domain_radius_um: f64,
    /// Domain material density in g/cm³ (tissue-equivalent ≈ 1.0).
    pub domain_density_g_cm3: f64,
    /// MKM parameters per dose component (`boron`, `nitrogen`,
    /// `hydrogen`, `photon`) — exactly the four keys.
    pub components: BTreeMap<String, MkmComponent>,
    /// Optional per-region LQ overrides keyed by region name. Region
    /// masks are supplied at application time; a region listed here
    /// without a matching mask is an error.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub region_lq: BTreeMap<String, MkmLq>,
    /// Explicit precedence for overlapping region masks, earliest wins —
    /// required when voxels could match more than one declared region
    /// (see [`crate::BiologicalModel::region_priority`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub region_priority: Vec<String>,
    /// Evidence reference for where the parameter set came from.
    pub derivation: Option<ContentReference>,
    /// Mandatory free-text validity domain: cell system, dose range,
    /// radiation qualities, endpoint.
    pub validity_domain: String,
    /// Optional linear-quadratic fractionation. The MKM
    /// photon-equivalence is applied to each component's per-fraction
    /// dose (`d_c·p`); the summed per-fraction photon-equivalent total
    /// `T` then transforms to `n·T·(1 + T/(α/β))/(1 + 2/(α/β))`.
    /// Requires `input_unit` `gray_per_source_particle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fractionation: Option<Fractionation>,
}

impl MicrodosimetricModel {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, MICRODOSIMETRIC_MODEL_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() {
            return Err(BioError::Invalid("model id is empty".into()));
        }
        if self.validity_domain.trim().is_empty() {
            return Err(BioError::Invalid(
                "microdosimetric models must declare a non-empty validity domain".into(),
            ));
        }
        if !self.domain_radius_um.is_finite()
            || self.domain_radius_um <= 0.0
            || !self.domain_density_g_cm3.is_finite()
            || self.domain_density_g_cm3 <= 0.0
        {
            return Err(BioError::Invalid(
                "domain_radius_um and domain_density_g_cm3 must be finite and positive".into(),
            ));
        }
        let required: BTreeSet<&str> = DoseComponent::REQUIRED
            .iter()
            .map(|component| component_name(*component))
            .collect();
        let present: BTreeSet<&str> = self.components.keys().map(String::as_str).collect();
        if present != required {
            return Err(BioError::Invalid(format!(
                "components must define exactly the four dose components {required:?}; observed {present:?}"
            )));
        }
        for (name, component) in &self.components {
            if !component.alpha_0.is_finite()
                || component.alpha_0 < 0.0
                || !component.beta.is_finite()
                || component.beta < 0.0
            {
                return Err(BioError::Invalid(format!(
                    "component {name}: alpha_0 and beta must be finite and non-negative"
                )));
            }
            if component.alpha_0 == 0.0 && component.beta == 0.0 {
                return Err(BioError::Invalid(format!(
                    "component {name}: alpha_0 and beta cannot both be zero"
                )));
            }
            match &component.lineal_energy {
                LinealEnergySource::Constant {
                    dose_mean_lineal_energy_kev_um,
                    ..
                } => {
                    if !dose_mean_lineal_energy_kev_um.is_finite()
                        || *dose_mean_lineal_energy_kev_um < 0.0
                    {
                        return Err(BioError::Invalid(format!(
                            "component {name}: dose_mean_lineal_energy_kev_um must be finite and non-negative"
                        )));
                    }
                }
                LinealEnergySource::Spectrum { spectrum_id } => {
                    if spectrum_id.trim().is_empty() {
                        return Err(BioError::Invalid(format!(
                            "component {name}: spectrum_id is empty"
                        )));
                    }
                }
            }
        }
        {
            let declared: std::collections::BTreeSet<&str> = self
                .region_lq
                .keys()
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
}

/// Dose-mean single-event specific energy z̄₁D (Gy) deposited in one
/// spherical MKM domain by a single event of dose-mean lineal energy
/// `y_kev_um` — `z̄₁D = ȳ_D/(ρ·π·r_d²)` (mean chord `4r/3` over mass
/// `4πr³ρ/3` cancels to `πρr²`). SI conversion: keV/µm → J/m,
/// g/cm³ → kg/m³, µm → m. For reference, ȳ = 1 keV/µm in a 1 µm water
/// domain gives z̄₁D ≈ 0.051 Gy.
pub fn domain_mean_specific_energy_gy(y_kev_um: f64, radius_um: f64, density_g_cm3: f64) -> f64 {
    let y_j_per_m = y_kev_um * JOULE_PER_KEV / 1.0e-6;
    let rho = density_g_cm3 * 1.0e3;
    let r_m = radius_um * 1.0e-6;
    y_j_per_m / (rho * PI * r_m * r_m)
}

/// Invert the photon LQ `X = α₀·D + β·D²` once for a combined
/// mixed-field effect X — the stable root `2X/(α₀ + √(α₀²+4βX))` with
/// `dD/dX = 1/(α₀ + 2βD)`.
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

/// Per-component photon-equivalent dose and its dose derivative.
///
/// `X = α*·d + β·d²` is the component's survival exponent under the
/// MKM effective α `α* = α₀ + β·z̄₁D`; the photon-equivalent dose D
/// solves `α₀·D + β·D² = X`. The stable root form
/// `2X/(α₀ + √(α₀² + 4βX))` avoids cancellation at small X; the β→0
/// limit is `X/α₀`. The derivative `dD/dd = (α* + 2βd)/√(α₀² + 4βX)`
/// propagates first-order dose uncertainty.
///
/// Retained for tests — the production path combines components in
/// effect space and inverts once via [`invert_lq`]; summing these
/// per-component values is *not* the mixed-field answer.
#[cfg(test)]
fn photon_equivalent_dose(d: f64, alpha_0: f64, beta: f64, z1d: f64) -> (f64, f64) {
    let alpha_star = alpha_0 + beta * z1d;
    let effect = alpha_star * d + beta * d * d;
    if effect <= 0.0 {
        return (
            0.0,
            if alpha_0 > 0.0 {
                alpha_star / alpha_0
            } else {
                0.0
            },
        );
    }
    let root = (alpha_0 * alpha_0 + 4.0 * beta * effect).sqrt();
    let dose = if beta > 0.0 {
        2.0 * effect / (alpha_0 + root)
    } else {
        effect / alpha_0
    };
    (dose, (alpha_star + 2.0 * beta * d) / root)
}

/// The MKM provenance block recorded on the emitted bundle: the domain
/// geometry and the lineal energies actually resolved per component,
/// plus content bindings of every spectrum document consumed. Without
/// this an MKM result is not independently checkable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MkmApplied {
    pub domain_radius_um: f64,
    pub domain_density_g_cm3: f64,
    /// Resolved dose-mean lineal energy per component, keV/µm.
    pub dose_mean_lineal_energy_kev_um: BTreeMap<String, f64>,
    /// Content bindings of the spectrum documents consumed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spectra_applied: Vec<ContentReference>,
}

/// Apply a microdosimetric-kinetic model to a physical dose bundle.
///
/// Region plumbing mirrors `apply_biological_model`: masks supply
/// `region_lq` names (LQ overrides) and `fractionation.region_alpha_beta`
/// names; a declared region without a mask is rejected, and a voxel in
/// several named regions takes the first in map order. `spectra`
/// supplies every `LinealEnergySource::Spectrum` the components name —
/// an unresolved id is a hard error, never a silent constant fallback.
///
/// The emitted bundle keeps the four-component shape, but each
/// component volume now holds that component's *photon-equivalent*
/// dose (not a linearly weighted one) and `weight_semantics` is
/// `microdosimetric_kinetic`. With fractionation the total is the
/// MKM-derived `mkm_weighted_eqd2`; otherwise it mirrors the input
/// unit as `mkm_weighted_gray[_per_source_particle]`.
pub fn apply_microdosimetric_model(
    model: &MicrodosimetricModel,
    model_bytes: &[u8],
    physical: &PhysicalDoseBundle,
    regions: &[RegionMask],
    spectra: &[SpectrumInput<'_>],
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

    // Region-name -> voxel lookup; every declared region must have a
    // covering mask.
    let masks: BTreeMap<&str, &Vec<bool>> = regions
        .iter()
        .map(|mask| (mask.name.as_str(), &mask.voxels))
        .collect();
    for name in model.region_lq.keys() {
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
    let lq_order = crate::ordered_region_names(model.region_lq.keys(), &model.region_priority);
    let lq_assignment = crate::resolve_regions(
        &lq_order,
        &masks,
        &model.region_priority,
        voxel_count,
        "region_lq",
    )?;
    let lq_of = |voxel: usize| -> Option<(f64, f64)> {
        lq_assignment[voxel].map(|i| {
            let lq = &model.region_lq[&lq_order[i]];
            (lq.alpha_0, lq.beta)
        })
    };

    // Resolve each component's dose-mean lineal energy once per apply:
    // constants directly, spectra through the supplied documents.
    let spectrum_of = |id: &str| -> Result<&SpectrumInput<'_>, BioError> {
        spectra
            .iter()
            .find(|input| input.spectrum.id == id)
            .ok_or_else(|| BioError::UnresolvedSpectrum(id.into()))
    };
    let mut resolved_y = BTreeMap::new();
    let mut spectra_applied: Vec<ContentReference> = Vec::new();
    for (name, component) in &model.components {
        let y = match &component.lineal_energy {
            LinealEnergySource::Constant {
                dose_mean_lineal_energy_kev_um,
                ..
            } => *dose_mean_lineal_energy_kev_um,
            LinealEnergySource::Spectrum { spectrum_id } => {
                let input = spectrum_of(spectrum_id)?;
                let reference = ContentReference {
                    id: input.spectrum.id.clone(),
                    sha256: format!("{:x}", Sha256::digest(input.document_bytes)),
                };
                if !spectra_applied.contains(&reference) {
                    spectra_applied.push(reference);
                }
                input.spectrum.dose_mean_kev_um()?
            }
        };
        resolved_y.insert(name.clone(), y);
    }

    let unit = match model.input_unit {
        DoseUnit::GrayPerSourceParticle => "mkm_weighted_gray_per_source_particle".to_string(),
        DoseUnit::Gray => "mkm_weighted_gray".to_string(),
    };
    let p = model
        .fractionation
        .as_ref()
        .map_or(1.0, |f| f.source_particles_per_fraction);
    // Mixed-field MKM: components combine in *effect* space, then the
    // photon-equivalent dose is inverted once. Inverting each component
    // separately and summing (as this code originally did) double-counts
    // the quadratic cross terms and overstates the total by ~10-20% at
    // single-fraction BNCT doses. The combined exponent is
    //   X = Σᵢ (α₀ᵢ + βᵢ·z̄₁Dᵢ)·dᵢ + (Σᵢ √βᵢ·dᵢ)²
    // — the Zaider–Rossi √β synergistic form, which reduces to
    // β·(Σdᵢ)² under MKM's domain-invariant common β. Each component's
    // reported value is its effect-share of the isoeffective dose, so
    // the components still sum to the total exactly.
    let component_z1d: Vec<f64> = physical
        .components
        .iter()
        .map(|volume| {
            domain_mean_specific_energy_gy(
                resolved_y[component_name(volume.component)],
                model.domain_radius_um,
                model.domain_density_g_cm3,
            )
        })
        .collect();
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
    let photon_params = &model.components[component_name(DoseComponent::Photon)];
    for voxel in 0..voxel_count {
        // Region LQ overrides replace α₀/β for every component inside the
        // mask; the override is also the photon reference for inversion.
        let region_lq = lq_of(voxel);
        let (ref_alpha, ref_beta) =
            region_lq.unwrap_or((photon_params.alpha_0, photon_params.beta));
        let mut effect = 0.0;
        let mut sqrt_beta_d = 0.0;
        let mut alpha_star = vec![0.0; component_count];
        let mut betas = vec![0.0; component_count];
        let mut doses = vec![0.0; component_count];
        for (index, volume) in physical.components.iter().enumerate() {
            let d = volume.values[voxel] * p;
            let (a0, b) = region_lq.unwrap_or_else(|| {
                let params = &model.components[component_name(volume.component)];
                (params.alpha_0, params.beta)
            });
            alpha_star[index] = a0 + b * component_z1d[index];
            betas[index] = b;
            doses[index] = d;
            effect += alpha_star[index] * d;
            sqrt_beta_d += b.sqrt() * d;
        }
        effect += sqrt_beta_d * sqrt_beta_d;
        let (d_iso, dd_dx) = invert_lq(effect, ref_alpha, ref_beta);
        total_values[voxel] = d_iso;
        for (index, volume) in physical.components.iter().enumerate() {
            // Share sᵢ = α*ᵢ·dᵢ + √βᵢ·dᵢ·(Σ√βⱼ·dⱼ): the linear term plus
            // this component's portion of every cross term touching it.
            let share =
                alpha_star[index] * doses[index] + betas[index].sqrt() * doses[index] * sqrt_beta_d;
            components[index].values[voxel] = if effect > 0.0 {
                d_iso * share / effect
            } else {
                0.0
            };
            if let (Some(sigmas), Some(source)) = (
                components[index].absolute_standard_uncertainty.as_mut(),
                volume.absolute_standard_uncertainty.as_ref(),
            ) {
                // ∂D/∂dᵢ(raw) = (∂X/∂dᵢ)·(∂d/∂raw)·(dD/dX); per-component
                // sigmas are marginal contributions summing to total σ.
                let dx_ddi = alpha_star[index] + 2.0 * betas[index].sqrt() * sqrt_beta_d;
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
        let ab_order = crate::ordered_region_names(
            fractionation.region_alpha_beta.keys(),
            &model.region_priority,
        );
        let ab_assignment = crate::resolve_regions(
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
            // `total_values` currently holds the per-fraction
            // photon-equivalent total T (components were transformed at
            // d·p); rescale to EQD2: n·T·(1 + T/r)/(1 + 2/r).
            let ratio = alpha_beta_of(voxel);
            let t = total_values[voxel];
            total_values[voxel] = n * t * (1.0 + t / ratio) / (1.0 + 2.0 / ratio);
            if have_sigma {
                let derivative = n * (1.0 + 2.0 * t / ratio) / (1.0 + 2.0 / ratio);
                total_sigma[voxel] *= derivative;
            }
        }
        total_unit = "mkm_weighted_eqd2".into();
        applied_fractionation = Some(AppliedFractionation {
            fraction_count: fractionation.fraction_count,
            source_particles_per_fraction: p,
            regions_applied: fractionation.region_alpha_beta.keys().cloned().collect(),
        });
    }

    let mut regions_applied: Vec<String> = model.region_lq.keys().cloned().collect();
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
        weight_semantics: WeightSemantics::MicrodosimetricKinetic,
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
        qualification: "mkm_research_only_not_clinical".into(),
        microdosimetry: Some(MkmApplied {
            domain_radius_um: model.domain_radius_um,
            domain_density_g_cm3: model.domain_density_g_cm3,
            dose_mean_lineal_energy_kev_um: resolved_y,
            spectra_applied,
        }),
        isoeffective: None,
    };
    bundle.validate()?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BiologicalModel, WeightSemantics};
    use openbnct_core::{
        DoseVolume, GridGeometry, PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

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

    fn constant(y: f64) -> LinealEnergySource {
        LinealEnergySource::Constant {
            dose_mean_lineal_energy_kev_um: y,
            note: None,
        }
    }

    /// Literature-shaped parameter set (Kase 2008-style HSG cell LQ,
    /// composite BNCT component lineal energies) — test stand-in, not a
    /// clinical model.
    fn mkm_model() -> MicrodosimetricModel {
        let component = |y: f64| MkmComponent {
            alpha_0: 0.2,
            beta: 0.05,
            lineal_energy: constant(y),
        };
        MicrodosimetricModel {
            schema_version: MICRODOSIMETRIC_MODEL_SCHEMA.into(),
            region_priority: Vec::new(),
            id: "test.mkm.v1".into(),
            input_unit: DoseUnit::GrayPerSourceParticle,
            domain_radius_um: 1.0,
            domain_density_g_cm3: 1.0,
            components: BTreeMap::from([
                ("boron".to_string(), component(200.0)),
                ("nitrogen".to_string(), component(60.0)),
                ("hydrogen".to_string(), component(30.0)),
                ("photon".to_string(), component(1.5)),
            ]),
            region_lq: BTreeMap::new(),
            derivation: None,
            validity_domain: "synthetic test model; not clinical".into(),
            fractionation: None,
        }
    }

    fn spectrum() -> LinealSpectrum {
        LinealSpectrum {
            schema_version: LINEAL_SPECTRUM_SCHEMA.into(),
            id: "spec.test".into(),
            bin_edges_kev_um: vec![1.0, 2.0, 4.0],
            values: vec![3.0, 1.0],
            absolute_standard_uncertainty: None,
            weighting: LinealWeighting::EventFrequency,
            value_unit: "events".into(),
            derivation: None,
            note: None,
        }
    }

    #[test]
    fn domain_energy_matches_published_scale() {
        // 1 keV/µm in a 1 µm water domain ≈ 0.0510 Gy.
        let z = domain_mean_specific_energy_gy(1.0, 1.0, 1.0);
        assert!((z - 0.0510).abs() / 0.0510 < 0.01, "z1d = {z}");
        // Scales linearly with y, inversely with r² and rho.
        let z2 = domain_mean_specific_energy_gy(200.0, 2.0, 2.0);
        assert!((z2 - 200.0 * 0.0510 / 8.0).abs() / z2 < 0.01);
    }

    #[test]
    fn spectrum_moments_are_exact_under_piecewise_constant_bins() {
        let s = spectrum();
        // Event-frequency f: ∫f = 3·1 + 1·2 = 5
        // ∫y f = 3·(4-1)/2 + 1·(16-4)/2 = 4.5 + 6 = 10.5
        // ∫y²f = 3·(8-1)/3 + 1·(64-8)/3 = 7 + 56/3 = 77/3
        assert_eq!(s.frequency_mean_kev_um().unwrap(), Some(10.5 / 5.0));
        let yd = s.dose_mean_kev_um().unwrap();
        assert!((yd - (77.0 / 3.0) / 10.5).abs() / yd < 1e-12, "yd = {yd}");
    }

    #[test]
    fn dose_weighted_spectrum_reports_dose_mean_only() {
        let mut s = spectrum();
        s.weighting = LinealWeighting::DoseWeighted;
        assert_eq!(s.frequency_mean_kev_um().unwrap(), None);
        // ∫y·d = 10.5, ∫d = 5 → ȳ_D = 2.1
        let yd = s.dose_mean_kev_um().unwrap();
        assert!((yd - 2.1).abs() < 1e-12);
    }

    #[test]
    fn zero_lineal_energy_is_the_photon_fixed_point() {
        // ȳ → 0 collapses α* → α₀ and D_pe → d exactly.
        let (d, derivative) = photon_equivalent_dose(1.3, 0.2, 0.05, 0.0);
        assert!((d - 1.3).abs() < 1e-12);
        assert!((derivative - 1.0).abs() < 1e-12);
    }

    #[test]
    fn applies_photon_equivalent_components_and_mkm_provenance() {
        let model = mkm_model();
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let bundle =
            apply_microdosimetric_model(&model, &bytes, &physical_bundle(), &[], &[]).unwrap();
        assert_eq!(
            bundle.weight_semantics,
            WeightSemantics::MicrodosimetricKinetic
        );
        assert_eq!(bundle.unit, "mkm_weighted_gray_per_source_particle");
        assert_eq!(bundle.qualification, "mkm_research_only_not_clinical");
        let mkm = bundle.microdosimetry.as_ref().unwrap();
        assert_eq!(mkm.dose_mean_lineal_energy_kev_um["boron"], 200.0);
        assert!(mkm.spectra_applied.is_empty());

        // At per-particle doses the quadratic term vanishes: D_pe ≈
        // (α*/α₀)·d — boron ȳ=200 → z̄≈10.2 Gy → α*≈0.71 → RBE≈3.55.
        let boron = bundle
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Boron)
            .unwrap();
        let expected_ratio = (0.2 + 0.05 * 200.0 * 0.0510) / 0.2;
        let observed = boron.values[0] / 1.0e-12;
        assert!((observed - expected_ratio).abs() / expected_ratio < 0.01);
        // Photon (ȳ=1.5) stays near unity, and boron exceeds it.
        let photon = bundle
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Photon)
            .unwrap();
        assert!(photon.values[0] > 3.0e-13 && photon.values[0] < 3.5e-13);
        assert!(boron.values[0] > photon.values[0]);
        // Total is the combined-effect isoeffective dose; components are
        // effect-shares summing to it (within f64 accumulation order).
        let sum: f64 = bundle.components.iter().map(|c| c.values[0]).sum();
        assert!((bundle.total.values[0] - sum).abs() / bundle.total.values[0] < 1e-12);
    }

    #[test]
    fn fractionation_reveals_the_quadratic_term() {
        let mut model = mkm_model();
        model.fractionation = Some(Fractionation {
            fraction_count: 30,
            source_particles_per_fraction: 1.0e12,
            default_alpha_beta: 10.0,
            region_alpha_beta: BTreeMap::new(),
        });
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let bundle =
            apply_microdosimetric_model(&model, &bytes, &physical_bundle(), &[], &[]).unwrap();
        assert_eq!(bundle.total.unit, "mkm_weighted_eqd2");
        // Boron per-fraction dose = 1.0 Gy → X = α*·1 + 0.05·1² with
        // α* = 0.2 + 0.05·z̄(200 keV/µm); D_pe = 2X/(0.2 + √(0.04 + 0.2X)).
        // Combined-field check: X = Σ(α*ᵢ·dᵢ) + β(Σdᵢ)² on the
        // per-fraction doses (per-particle values × 1e12 particles).
        let z1d = |name: &str| {
            let y = &model.components[name].lineal_energy;
            let LinealEnergySource::Constant {
                dose_mean_lineal_energy_kev_um,
                ..
            } = y
            else {
                unreachable!("test model uses constant lineal energies")
            };
            domain_mean_specific_energy_gy(*dose_mean_lineal_energy_kev_um, 1.0, 1.0)
        };
        let d: BTreeMap<&str, f64> = [
            ("boron", 1.0),
            ("nitrogen", 0.2),
            ("hydrogen", 0.05),
            ("photon", 0.3),
        ]
        .into_iter()
        .collect();
        let mut x = 0.0;
        for name in model.components.keys() {
            x += (0.2 + 0.05 * z1d(name)) * d[name.as_str()];
        }
        x += 0.05 * d.values().sum::<f64>().powi(2);
        let expected_pf = 2.0 * x / (0.2 + (0.04_f64 + 4.0 * 0.05 * x).sqrt());
        let per_fraction_sum: f64 = bundle.components.iter().map(|c| c.values[0]).sum();
        assert!(
            (per_fraction_sum - expected_pf).abs() / expected_pf < 1e-9,
            "per-fraction D_pe {per_fraction_sum} vs {expected_pf}"
        );
        // Component volumes carry the per-fraction isoeffective shares;
        // the total carries the EQD2 rescale.
        let eqd2 = 30.0 * per_fraction_sum * (1.0 + per_fraction_sum / 10.0) / 1.2;
        assert!(
            (bundle.total.values[0] - eqd2).abs() / eqd2 < 1e-9,
            "total {} vs {eqd2}",
            bundle.total.values[0]
        );
    }

    #[test]
    fn region_lq_overrides_alpha_beta_inside_the_mask() {
        let mut model = mkm_model();
        model.region_lq.insert(
            "tumor".into(),
            MkmLq {
                alpha_0: 0.1,
                beta: 0.05,
            },
        );
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let mask = RegionMask {
            name: "tumor".into(),
            voxels: vec![true, false],
        };
        let bundle = apply_microdosimetric_model(
            &model,
            &bytes,
            &physical_bundle(),
            std::slice::from_ref(&mask),
            &[],
        )
        .unwrap();
        let boron = bundle
            .components
            .iter()
            .find(|c| c.component == DoseComponent::Boron)
            .unwrap();
        // Tumor voxel: α₀=0.1 → ratio (0.1 + 0.05·10.2)/0.1 = 6.1.
        let tumor_ratio = (0.1 + 0.05 * 200.0 * 0.0510) / 0.1;
        let default_ratio = (0.2 + 0.05 * 200.0 * 0.0510) / 0.2;
        assert!((boron.values[0] / 1.0e-12 - tumor_ratio).abs() / tumor_ratio < 0.01);
        assert!((boron.values[1] / 1.0e-12 - default_ratio).abs() / default_ratio < 0.01);
        assert_eq!(bundle.regions_applied, vec!["tumor"]);
    }

    #[test]
    fn spectrum_sourced_lineal_energy_resolves_and_binds() {
        let mut model = mkm_model();
        model.components.get_mut("boron").unwrap().lineal_energy = LinealEnergySource::Spectrum {
            spectrum_id: "spec.test".into(),
        };
        let spec = spectrum();
        let spec_bytes = serde_json::to_vec_pretty(&spec).unwrap();
        let inputs = [SpectrumInput {
            spectrum: &spec,
            document_bytes: &spec_bytes,
        }];
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let bundle =
            apply_microdosimetric_model(&model, &bytes, &physical_bundle(), &[], &inputs).unwrap();
        let mkm = bundle.microdosimetry.as_ref().unwrap();
        // ȳ_D of the test spectrum is (77/3)/10.5 ≈ 2.4444 keV/µm.
        assert!(
            (mkm.dose_mean_lineal_energy_kev_um["boron"] - (77.0 / 3.0) / 10.5).abs() < 1e-9,
            "resolved boron ȳ = {}",
            mkm.dose_mean_lineal_energy_kev_um["boron"]
        );
        assert_eq!(mkm.spectra_applied.len(), 1);
        assert_eq!(mkm.spectra_applied[0].id, "spec.test");
        assert_eq!(
            mkm.spectra_applied[0].sha256,
            format!("{:x}", Sha256::digest(&spec_bytes))
        );
    }

    #[test]
    fn rejects_unresolved_spectrum_and_bad_parameters() {
        let mut model = mkm_model();
        model.components.get_mut("boron").unwrap().lineal_energy = LinealEnergySource::Spectrum {
            spectrum_id: "missing".into(),
        };
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let error =
            apply_microdosimetric_model(&model, &bytes, &physical_bundle(), &[], &[]).unwrap_err();
        assert!(matches!(error, BioError::UnresolvedSpectrum(_)));

        let mut empty_domain = mkm_model();
        empty_domain.validity_domain = "   ".into();
        assert!(empty_domain.validate().is_err());

        let mut bad = mkm_model();
        bad.domain_radius_um = 0.0;
        assert!(bad.validate().is_err());

        let mut incomplete = mkm_model();
        incomplete.components.remove("photon");
        assert!(incomplete.validate().is_err());

        let mut negative = mkm_model();
        negative.components.get_mut("nitrogen").unwrap().beta = -1.0;
        assert!(negative.validate().is_err());
    }

    #[test]
    fn weight_models_cannot_claim_microdosimetric_semantics() {
        let mut weights = crate::WeightMap::new();
        for name in ["boron", "nitrogen", "hydrogen", "photon"] {
            weights.insert(name.to_string(), 1.0);
        }
        let model = BiologicalModel {
            region_priority: Vec::new(),
            component_weight_uncertainty: BTreeMap::new(),
            schema_version: crate::BIOLOGICAL_MODEL_SCHEMA.into(),
            id: "mismarked".into(),
            weight_semantics: WeightSemantics::MicrodosimetricKinetic,
            input_unit: DoseUnit::GrayPerSourceParticle,
            component_weights: weights,
            region_weights: BTreeMap::new(),
            derivation: None,
            validity_domain: None,
            fractionation: None,
        };
        assert!(model.validate().is_err());
    }
}
