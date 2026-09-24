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

/// Versioned contract id for `PromptGammaObservation`.
pub const PROMPT_GAMMA_OBSERVATION_SCHEMA: &str = "openbnct.pg-observation/0.1.0";

/// Versioned contract id for `PromptGammaReconstruction`.
pub const PROMPT_GAMMA_RECONSTRUCTION_SCHEMA: &str = "openbnct.pg-reconstruction/0.1.0";

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
    /// Pinhole-collimation declaration when the adjoint source was
    /// direction-restricted — `None` for an uncollimated (all-ordinate)
    /// response. Omitted from the wire format when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collimation: Option<PgCollimation>,
    /// Content bindings to the transport case and photon data the
    /// adjoint was solved against.
    pub case: ContentReference,
    pub photon_data: ContentReference,
    pub provenance_id: String,
    pub qualification: String,
}

/// Pinhole-collimator geometry a collimated `pg response` was solved
/// under. The adjoint detector source emits only along ordinates inside
/// the cone subtended by the aperture as seen from the detector
/// centroid — the response then carries the spatial selectivity a real
/// pinhole measurement has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PgCollimation {
    /// Aperture center in the case's mm coordinates.
    pub aperture_mm: [f64; 3],
    /// Aperture radius in mm — the acceptance cone subtends
    /// `atan(radius / distance)` about the detector→aperture axis.
    pub aperture_radius_mm: f64,
    /// Fraction of quadrature ordinates falling inside the acceptance
    /// cone — recorded so consumers can check angular resolution.
    pub accepted_ordinate_fraction: f64,
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
        if let Some(c) = &self.collimation {
            let bad = c.aperture_mm.iter().any(|v| !v.is_finite())
                || !(c.aperture_radius_mm > 0.0 && c.aperture_radius_mm.is_finite())
                || !(c.accepted_ordinate_fraction > 0.0 && c.accepted_ordinate_fraction <= 1.0);
            if bad {
                return Err(PromptGammaError::NonFiniteValue);
            }
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

/// One detector's measured tally paired with the response artifact it
/// was taken against — either synthesized from a `PromptGammaCounts`
/// (forward-model output, R13-04 closure) or authored by hand for a
/// real measurement. The response binding is content-hashed; the
/// reconstruct path verifies every response file against it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaObservationEntry {
    pub response: ContentReference,
    pub measured_tally: f64,
}

/// The measured half of the reconstruction problem: one tally per
/// detector position, each bound to the response artifact that
/// position's sensitivity map lives in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaObservation {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub detectors: Vec<PromptGammaObservationEntry>,
    pub provenance_id: String,
    pub qualification: String,
}

/// Reconstructed emission map: the bounded inverse of the response
/// matrix under a declared regularization. Values share the emission
/// map's per-kg unit — the inverse estimate of the 478 keV source
/// distribution, not an image and not a dose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaReconstruction {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub geometry: GridGeometry,
    pub unit: PromptGammaUnit,
    pub emission_energy_ev: f64,
    /// Reconstructed per-kg emission per voxel, grid order —
    /// non-negative by construction.
    pub values: Vec<f64>,
    /// Declared regularization and its outcome.
    pub regularization: PromptGammaRegularization,
    /// Content bindings to the observation and to every response
    /// artifact that formed the matrix columns.
    pub observation: ContentReference,
    pub responses: Vec<ContentReference>,
    pub provenance_id: String,
    pub qualification: String,
}

/// The declared inversion: non-negative least squares with a
/// Tikhonov term — `min_x ‖Ax − b‖² + λ‖x‖², x ≥ 0` — solved by
/// projected gradient with a power-iterated step bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptGammaRegularization {
    pub method: String,
    pub lambda: f64,
    pub iterations: u32,
    pub converged: bool,
    /// ‖Ax − b‖ at the returned x, in tally units.
    pub residual_norm: f64,
    /// Uniform voxel density used to fold per-kg emissions into the
    /// operator — kept with the declaration, not hidden.
    pub voxel_density_kg_per_m3: f64,
}

impl PromptGammaObservation {
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
        if self.detectors.is_empty()
            || self.detectors.iter().any(|d| {
                !d.measured_tally.is_finite()
                    || d.measured_tally < 0.0
                    || d.response.id.trim().is_empty()
            })
        {
            return Err(PromptGammaError::NonFiniteValue);
        }
        Ok(())
    }
}

impl PromptGammaReconstruction {
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
        if self.values.len() != voxels
            || self.values.iter().any(|v| !v.is_finite() || *v < 0.0)
            || !self.regularization.residual_norm.is_finite()
            || self.regularization.residual_norm < 0.0
        {
            return Err(PromptGammaError::NonFiniteValue);
        }
        Ok(())
    }
}

/// Collect synthetic detector readings into an observation: every
/// `PromptGammaCounts` contributes its bound response reference and
/// its expected tally as the "measured" value. This is the R13-04
/// closure path — real measurements are authored as the same schema
/// directly.
pub fn collect_prompt_gamma_observation(
    counts: &[PromptGammaCounts],
    id: &str,
    provenance_id: &str,
) -> Result<PromptGammaObservation, PromptGammaError> {
    if counts.is_empty() {
        return Err(PromptGammaError::EmptyIdentifier("counts"));
    }
    let case_id = counts[0].case_id.clone();
    if counts.iter().any(|c| c.case_id != case_id) {
        return Err(PromptGammaError::ShapeMismatch(
            "counts artifacts span multiple cases",
        ));
    }
    let observation = PromptGammaObservation {
        schema_version: PROMPT_GAMMA_OBSERVATION_SCHEMA.into(),
        id: id.into(),
        case_id,
        detectors: counts
            .iter()
            .map(|c| PromptGammaObservationEntry {
                response: c.response.clone(),
                measured_tally: c.expected_tally,
            })
            .collect(),
        provenance_id: provenance_id.into(),
        qualification: PROMPT_GAMMA_QUALIFICATION.into(),
    };
    observation.validate()?;
    Ok(observation)
}

/// Reconstruct the per-kg emission map from an observation and the
/// response artifacts it binds. `responses[i]` must be the artifact
/// `observation.detectors[i].response` points at — the caller verifies
/// content hashes before calling. The matrix column for detector d is
/// `sensitivity_d(v)·voxel_mass`, so the unknown x is in the emission
/// map's per-kg unit.
///
/// Solver: FISTA (accelerated projected gradient) on
/// `min_x ‖Ãx − b̃‖² + λ‖x‖², x ≥ 0` over row-normalized rows, with
/// the step set by a power-iterated Lipschitz bound. Stops on
/// relative iterate change below 1e-8 or at `max_iterations`.
#[allow(clippy::too_many_arguments)]
pub fn reconstruct_prompt_gamma_emission(
    observation: &PromptGammaObservation,
    responses: &[PromptGammaResponse],
    voxel_density_kg_per_m3: f64,
    lambda: f64,
    max_iterations: u32,
    unit: PromptGammaUnit,
    id: &str,
    observation_ref: ContentReference,
    provenance_id: &str,
) -> Result<PromptGammaReconstruction, PromptGammaError> {
    observation.validate()?;
    if responses.len() != observation.detectors.len() {
        return Err(PromptGammaError::ShapeMismatch(
            "response count != detector count",
        ));
    }
    for (response, detector) in responses.iter().zip(observation.detectors.iter()) {
        if response.id != detector.response.id {
            return Err(PromptGammaError::ShapeMismatch(
                "response artifact does not match the observation binding",
            ));
        }
    }
    let geometry = responses[0].geometry.clone();
    if responses
        .iter()
        .any(|r| r.geometry != geometry || !r.converged)
    {
        return Err(PromptGammaError::ShapeMismatch(
            "response grids differ or a response solve did not converge",
        ));
    }
    if !voxel_density_kg_per_m3.is_finite()
        || voxel_density_kg_per_m3 <= 0.0
        || !lambda.is_finite()
        || lambda < 0.0
        || max_iterations == 0
    {
        return Err(PromptGammaError::NonFiniteValue);
    }
    let voxels = geometry
        .voxel_count()
        .map_err(|_| PromptGammaError::NonFiniteValue)?;
    let spacing = &geometry.spacing_mm;
    let voxel_mass_kg = voxel_density_kg_per_m3 * spacing[0] * spacing[1] * spacing[2] * 1e-9;

    // A[d][v] = sensitivity_d(v)·voxel_mass — operator on per-kg x.
    let n_det = responses.len();
    let mut a = vec![vec![0.0; voxels]; n_det];
    for (d, response) in responses.iter().enumerate() {
        for (v, s) in response.sensitivity.iter().enumerate() {
            a[d][v] = s * voxel_mass_kg;
        }
    }
    let b: Vec<f64> = observation
        .detectors
        .iter()
        .map(|d| d.measured_tally)
        .collect();

    // Detector tallies differ by orders of magnitude across
    // geometries, so the solve runs on row-normalized rows —
    // Ã_d = A_d/‖A_d‖, b̃_d = b_d/‖A_d‖ — keeping λ meaningful
    // relative to a unit-scale operator. The reported residual is
    // computed on the raw operator, in tally units.
    let a_solve = {
        let mut rows = a.clone();
        let mut rhs = b.clone();
        for (d, row) in rows.iter_mut().enumerate() {
            let norm = row.iter().map(|v| v * v).sum::<f64>().sqrt();
            if norm > 0.0 {
                for v in row.iter_mut() {
                    *v /= norm;
                }
                rhs[d] /= norm;
            }
        }
        (rows, rhs)
    };
    let (a_solve, b_solve) = a_solve;

    // FISTA on the normalized system — the passive-set methods churn
    // combinatorially over tens of thousands of columns, while the
    // accelerated first-order method's cost is predictable.
    let (x, converged, iterations) = fista_nnls(&a_solve, &b_solve, lambda, max_iterations);

    let residual_norm = a
        .iter()
        .zip(&b)
        .map(|(row, b)| row.iter().zip(&x).map(|(a, x)| a * x).sum::<f64>() - b)
        .map(|r| r * r)
        .sum::<f64>()
        .sqrt();

    Ok(PromptGammaReconstruction {
        schema_version: PROMPT_GAMMA_RECONSTRUCTION_SCHEMA.into(),
        id: id.into(),
        case_id: observation.case_id.clone(),
        geometry,
        unit,
        emission_energy_ev: PROMPT_GAMMA_ENERGY_EV,
        values: x,
        regularization: PromptGammaRegularization {
            method: "nnls_tikhonov_fista".into(),
            lambda,
            iterations,
            converged,
            residual_norm,
            voxel_density_kg_per_m3,
        },
        observation: observation_ref,
        responses: observation
            .detectors
            .iter()
            .map(|d| d.response.clone())
            .collect(),
        provenance_id: provenance_id.into(),
        qualification: PROMPT_GAMMA_QUALIFICATION.into(),
    })
}

/// FISTA (Beck–Teboulle accelerated projected gradient) on
/// `min_x ‖Ax − b‖² + λ‖x‖², x ≥ 0`. Returns
/// `(x, converged, iterations)` — `converged` when the relative
/// iterate change drops below 1e-8.
fn fista_nnls(
    a: &[Vec<f64>],
    b: &[f64],
    lambda: f64,
    max_iterations: u32,
) -> (Vec<f64>, bool, u32) {
    let voxels = a[0].len();
    let matvec = |x: &[f64]| -> Vec<f64> {
        a.iter()
            .map(|row| row.iter().zip(x).map(|(a, x)| a * x).sum())
            .collect()
    };
    let rmatvec = |r: &[f64]| -> Vec<f64> {
        let mut z = vec![0.0; voxels];
        for (d, row) in a.iter().enumerate() {
            for v in 0..voxels {
                z[v] += row[v] * r[d];
            }
        }
        z
    };
    // Lipschitz bound on ‖Ã^T Ã + λI‖ by power iteration.
    let mut w = vec![1.0_f64; voxels];
    let mut lipschitz = 1.0;
    for _ in 0..60 {
        let z = rmatvec(&matvec(&w));
        let norm = z.iter().map(|z| z * z).sum::<f64>().sqrt();
        if norm == 0.0 {
            break;
        }
        lipschitz = norm
            / w.iter()
                .map(|w| w * w)
                .sum::<f64>()
                .sqrt()
                .max(f64::MIN_POSITIVE);
        w = z.iter().map(|z| z / norm).collect();
    }
    let step = 1.0 / (lipschitz + lambda).max(f64::MIN_POSITIVE);

    let mut x = vec![0.0_f64; voxels];
    let mut y = x.clone();
    let mut t = 1.0_f64;
    let mut iterations = 0_u32;
    let mut converged = false;
    for iter in 0..max_iterations {
        iterations = iter + 1;
        // grad = Ã^T(Ãy − b̃) + λy at the accelerated point.
        let residual = matvec(&y);
        let g = rmatvec(
            &residual
                .iter()
                .zip(b)
                .map(|(r, b)| r - b)
                .collect::<Vec<_>>(),
        );
        let mut next = x.clone();
        let mut change = 0.0_f64;
        for v in 0..voxels {
            let nv = (y[v] - step * (g[v] + lambda * y[v])).max(0.0);
            change = change.max((nv - x[v]).abs());
            next[v] = nv;
        }
        let t_next = 0.5 * (1.0 + (1.0 + 4.0 * t * t).sqrt());
        let momentum = (t - 1.0) / t_next;
        for v in 0..voxels {
            y[v] = next[v] + momentum * (next[v] - x[v]);
        }
        let scale = next.iter().map(|v| v.abs()).fold(1.0_f64, f64::max);
        x = next;
        t = t_next;
        if change < 1e-8 * scale {
            converged = true;
            break;
        }
    }
    (x, converged, iterations)
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
            collimation: None,
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

    /// R13-04 forward-inverse closure on a synthetic operator: eight
    /// voxels in a row, eight detectors each dominated by a different
    /// voxel's neighborhood. A single-voxel emission must come back
    /// peaked at the true voxel with most of its mass.
    #[test]
    fn reconstructs_synthetic_emission() {
        let geometry = GridGeometry {
            shape: [8, 1, 1],
            spacing_mm: [10.0, 10.0, 10.0],
            origin_mm: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        };
        let cref = |id: &str, byte: u8| ContentReference {
            id: id.into(),
            sha256: format!("{byte:02x}").repeat(64),
        };
        // A[d][v] ∝ exp(−2(v−d)²) — each detector's column dominated by
        // its own voxel with weak neighbor bleed, so the 8×8 system is
        // well-conditioned and the solve converges quickly.
        let responses: Vec<PromptGammaResponse> = (0..8u32)
            .map(|d| PromptGammaResponse {
                schema_version: PROMPT_GAMMA_RESPONSE_SCHEMA.into(),
                id: format!("resp-{d}"),
                case_id: "case".into(),
                geometry: geometry.clone(),
                detector_voxels: vec![[d, 0, 0]],
                emission_group: 1,
                emission_energy_ev: 478_000.0,
                sensitivity: (0..8)
                    .map(|v| (-2.0 * (v as f64 - d as f64).powi(2)).exp())
                    .collect(),
                quadrature_order: 8,
                converged: true,
                residual: 1e-8,
                outer_iterations: 2,
                collimation: None,
                case: cref("case", 1),
                photon_data: cref("data", 2),
                provenance_id: format!("resp-{d}-prov"),
                qualification: PROMPT_GAMMA_QUALIFICATION.into(),
            })
            .collect();

        // Truth: a unit-per-kg blob in voxel 3. measured = A·x exactly.
        // 10 mm-sided voxels are 1e-6 m³ — 1e-3 kg at 1000 kg/m³.
        let voxel_mass = 1e-3;
        let observation = PromptGammaObservation {
            schema_version: PROMPT_GAMMA_OBSERVATION_SCHEMA.into(),
            id: "obs".into(),
            case_id: "case".into(),
            detectors: responses
                .iter()
                .enumerate()
                .map(|(d, r)| PromptGammaObservationEntry {
                    response: cref(&r.id, 0x10 + d as u8),
                    measured_tally: r.sensitivity[3] * voxel_mass,
                })
                .collect(),
            provenance_id: "obs-prov".into(),
            qualification: PROMPT_GAMMA_QUALIFICATION.into(),
        };
        // λ → 0: the synthetic system is exactly consistent, so the
        // NNLS should recover the blob almost exactly. (A realistic
        // λ against this ill-conditioned kernel honestly shrinks the
        // peak — regularization is the point, not a defect.)
        let reconstruction = reconstruct_prompt_gamma_emission(
            &observation,
            &responses,
            1000.0,
            1e-10,
            2000,
            PromptGammaUnit::PhotonsPerKgPerSourceParticle,
            "recon",
            cref("obs", 3),
            "recon-prov",
        )
        .unwrap();
        reconstruction.validate().unwrap();
        let peak = reconstruction
            .values
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(peak, 3, "reconstruction {:#?}", reconstruction.values);
        assert!(
            reconstruction.values[3] > 0.5,
            "peak mass {} too low",
            reconstruction.values[3]
        );
        let b_norm = observation
            .detectors
            .iter()
            .map(|d| d.measured_tally * d.measured_tally)
            .sum::<f64>()
            .sqrt();
        assert!(
            reconstruction.regularization.residual_norm < 1e-3 * b_norm,
            "residual {} too high vs ‖b‖ {b_norm}",
            reconstruction.regularization.residual_norm
        );
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
