// SPDX-License-Identifier: MIT

//! Boron-10 subcellular microdistribution artifacts
//! (`openbnct.boron-microdistribution/0.1.0`,
//! `openbnct.microdistribution-correction/0.1.0`).
//!
//! A microdistribution model declares how ¹⁰B partitions across
//! subcellular compartments — nucleus, cytoplasm, cell membrane, and the
//! extracellular space — together with the concentric-sphere cell
//! geometry, the adopted α/⁷Li track ranges, and the cell-to-cell
//! uptake heterogeneity (coefficient of variation).
//!
//! [`evaluate_microdistribution`] computes, by deterministic quadrature,
//! the fraction of ¹⁰B(n,α)⁷Li charged-particle energy that reaches the
//! cell nucleus for each compartment, and folds them into a
//! *nucleus-dose factor*: the multiplier on the boron dose component
//! when the distribution is nonuniform, relative to the conventional
//! uniform-concentration assumption. This is the quantity the
//! photon-isoeffective literature (e.g. compound biological
//! effectiveness factors) conditions on, and published work shows
//! intercellular heterogeneity and nuclear localization materially
//! change it.
//!
//! Model and honesty conventions, mirroring the other artifact families:
//!
//! - Track physics is a documented first-order model: straight-line
//!   CSDA tracks, constant LET along the track, exact ray–sphere chord
//!   lengths, isotropic emission. Straggling, Bragg-curve shape, and
//!   non-concentric cell geometry are out of scope and stated so.
//! - The correction record reports absolute per-compartment deposition
//!   fractions *and* the relative factor, so downstream consumers can
//!   apply it under their own normalization convention.
//! - Particle energies and ranges are declared parameters of the model
//!   document (with the dominant 94%-branch ¹⁰B(n,α)⁷Li values as the
//!   conventional choice), not hidden constants — every number in the
//!   record traces to the input artifact by content hash.
//! - Intercellular heterogeneity is reported as distribution moments;
//!   its nonlinear effect materializes in survival-space evaluation
//!   downstream, which the record states explicitly.

use openbnct_core::{ContentReference, ValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current schema token for microdistribution model documents.
pub const BORON_MICRODISTRIBUTION_SCHEMA: &str = "openbnct.boron-microdistribution/0.1.0";

/// Current schema token for evaluated correction records.
pub const MICRODISTRIBUTION_CORRECTION_SCHEMA: &str = "openbnct.microdistribution-correction/0.1.0";

/// Qualification asserted on every emitted correction record.
pub const MICRODISTRIBUTION_CORRECTION_QUALIFICATION: &str =
    "first_order_geometric_microdosimetry_research_only_not_clinical";

/// Number of deterministic isotropic emission directions per decay site
/// (Fibonacci-sphere point set — reproducible, no RNG).
const DIRECTION_SAMPLES: usize = 512;

/// Number of radial quadrature points per compartment shell.
const RADIAL_SAMPLES: usize = 64;

/// A ¹⁰B subcellular microdistribution model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronMicrodistribution {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Cell radius, µm.
    pub cell_radius_um: f64,
    /// Nucleus radius, µm (must be smaller than `cell_radius_um`).
    pub nucleus_radius_um: f64,
    /// Outer boundary of the distribution region for extracellular
    /// sites, µm — the half-spacing to the next cell in a packed
    /// tissue approximation. Must exceed `cell_radius_um`.
    pub extracellular_extent_um: f64,
    /// Compartment mass fractions of ¹⁰B; must sum to 1.
    pub compartments: CompartmentFractions,
    /// Coefficient of variation (σ/μ) of cell-level ¹⁰B uptake across
    /// the cell population — 0 declares homogeneous uptake.
    pub intercellular_cv: f64,
    /// 1σ on `intercellular_cv`.
    pub intercellular_cv_1sigma: f64,
    /// Emitted α particle: kinetic energy, MeV.
    pub alpha_energy_mev: f64,
    /// Emitted α particle: CSDA range in unit-density tissue, µm.
    pub alpha_range_um: f64,
    /// Emitted ⁷Li recoil: kinetic energy, MeV.
    pub li_energy_mev: f64,
    /// Emitted ⁷Li recoil: CSDA range in unit-density tissue, µm.
    pub li_range_um: f64,
    /// Free-text validity domain: compound, cell system, assay method,
    /// and conditions the fractions were measured or assumed under.
    /// Required and non-empty.
    pub validity_domain: String,
    pub provenance_id: String,
}

/// Subcellular ¹⁰B mass fractions and their 1σ uncertainties.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompartmentFractions {
    /// Fraction of cellular-region ¹⁰B residing in the nucleus.
    pub nucleus: f64,
    pub nucleus_1sigma: f64,
    /// Fraction in the cytoplasm.
    pub cytoplasm: f64,
    pub cytoplasm_1sigma: f64,
    /// Fraction on/in the cell membrane (decays at r = cell radius).
    pub membrane: f64,
    pub membrane_1sigma: f64,
    /// Fraction in the extracellular space.
    pub extracellular: f64,
    pub extracellular_1sigma: f64,
}

/// Errors from microdistribution modeling.
#[derive(Debug, Error)]
pub enum MicrodistributionError {
    #[error("unsupported microdistribution schema {0:?}")]
    UnsupportedModelSchema(String),
    #[error("unsupported correction schema {0:?}")]
    UnsupportedCorrectionSchema(String),
    #[error("invalid microdistribution model: {0}")]
    InvalidModel(String),
    #[error("invalid correction record: {0}")]
    InvalidCorrection(String),
    #[error("invalid content reference: {0}")]
    InvalidContentReference(#[from] openbnct_core::ContentReferenceError),
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] ValidationError),
}

/// Per-particle geometric deposition results for one compartment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompartmentDeposition {
    /// Mean fraction of emitted α energy deposited in the nucleus.
    pub alpha_to_nucleus: f64,
    /// Mean fraction of emitted ⁷Li energy deposited in the nucleus.
    pub li_to_nucleus: f64,
    /// Energy-weighted mean across both fragments.
    pub combined_to_nucleus: f64,
}

/// Per-compartment deposition fractions in a correction record.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectionDeposition {
    pub nucleus: CompartmentDeposition,
    pub cytoplasm: CompartmentDeposition,
    pub membrane: CompartmentDeposition,
    pub extracellular: CompartmentDeposition,
}

/// The evaluated microdistribution correction record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MicrodistributionCorrection {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Geometric deposition fractions per compartment.
    pub deposition: CorrectionDeposition,
    /// The same quantity under the conventional uniform-concentration
    /// assumption (¹⁰B uniform over the whole distribution region).
    pub uniform_reference: CompartmentDeposition,
    /// `Σ_c fraction_c · g_c` — the model's absolute nucleus dose
    /// fraction.
    pub nucleus_dose_fraction: f64,
    /// `nucleus_dose_fraction / uniform_reference.combined_to_nucleus`:
    /// the multiplier on the boron dose component under this
    /// microdistribution relative to uniform concentration — the
    /// compound-factor-style correction a downstream
    /// photon-isoeffective evaluation may apply.
    pub nucleus_dose_factor: f64,
    /// First-order σ on `nucleus_dose_factor` from the declared
    /// compartment-fraction sigmas (linear propagation; the geometric
    /// factors and uniform reference are exact in-model).
    pub nucleus_dose_factor_1sigma: f64,
    /// Cell-to-cell nucleus-dose coefficient of variation implied by
    /// `intercellular_cv` under linear dose scaling — equals the
    /// declared CV; recorded so downstream survival evaluation sees
    /// the heterogeneity it must integrate over.
    pub intercellular_dose_cv: f64,
    /// Explicit statement of where the heterogeneity nonlinearity
    /// materializes.
    pub heterogeneity_note: String,
    /// The model evaluated.
    pub model: ContentReference,
    pub qualification: String,
    pub provenance_id: String,
}

impl BoronMicrodistribution {
    /// Structural validation; called by every consumer.
    pub fn validate(&self) -> Result<(), MicrodistributionError> {
        if !openbnct_core::schema_matches(&self.schema_version, BORON_MICRODISTRIBUTION_SCHEMA) {
            return Err(MicrodistributionError::UnsupportedModelSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(MicrodistributionError::InvalidModel("id is empty".into()));
        }
        for (name, v) in [
            ("cell_radius_um", self.cell_radius_um),
            ("nucleus_radius_um", self.nucleus_radius_um),
            ("extracellular_extent_um", self.extracellular_extent_um),
            ("alpha_energy_mev", self.alpha_energy_mev),
            ("alpha_range_um", self.alpha_range_um),
            ("li_energy_mev", self.li_energy_mev),
            ("li_range_um", self.li_range_um),
        ] {
            if !v.is_finite() || v <= 0.0 {
                return Err(MicrodistributionError::InvalidModel(format!(
                    "{name} must be positive and finite"
                )));
            }
        }
        if self.nucleus_radius_um >= self.cell_radius_um {
            return Err(MicrodistributionError::InvalidModel(
                "nucleus_radius_um must be smaller than cell_radius_um".into(),
            ));
        }
        if self.extracellular_extent_um <= self.cell_radius_um {
            return Err(MicrodistributionError::InvalidModel(
                "extracellular_extent_um must exceed cell_radius_um".into(),
            ));
        }
        let c = &self.compartments;
        let fractions = [
            ("nucleus", c.nucleus, c.nucleus_1sigma),
            ("cytoplasm", c.cytoplasm, c.cytoplasm_1sigma),
            ("membrane", c.membrane, c.membrane_1sigma),
            ("extracellular", c.extracellular, c.extracellular_1sigma),
        ];
        let mut total = 0.0;
        for (name, fraction, sigma) in fractions {
            if !fraction.is_finite() || fraction < 0.0 {
                return Err(MicrodistributionError::InvalidModel(format!(
                    "compartments.{name} must be a non-negative finite fraction"
                )));
            }
            if !sigma.is_finite() || sigma < 0.0 {
                return Err(MicrodistributionError::InvalidModel(format!(
                    "compartments.{name}_1sigma must be non-negative"
                )));
            }
            total += fraction;
        }
        if (total - 1.0).abs() > 1.0e-6 {
            return Err(MicrodistributionError::InvalidModel(format!(
                "compartment fractions must sum to 1, got {total}"
            )));
        }
        if !self.intercellular_cv.is_finite() || self.intercellular_cv < 0.0 {
            return Err(MicrodistributionError::InvalidModel(
                "intercellular_cv must be a non-negative finite value".into(),
            ));
        }
        if !self.intercellular_cv_1sigma.is_finite() || self.intercellular_cv_1sigma < 0.0 {
            return Err(MicrodistributionError::InvalidModel(
                "intercellular_cv_1sigma must be non-negative".into(),
            ));
        }
        if self.validity_domain.trim().is_empty() {
            return Err(MicrodistributionError::InvalidModel(
                "validity_domain is required and must not be empty".into(),
            ));
        }
        Ok(())
    }
}

impl MicrodistributionCorrection {
    /// Structural validation; called by consumers.
    pub fn validate(&self) -> Result<(), MicrodistributionError> {
        if !openbnct_core::schema_matches(&self.schema_version, MICRODISTRIBUTION_CORRECTION_SCHEMA)
        {
            return Err(MicrodistributionError::UnsupportedCorrectionSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(MicrodistributionError::InvalidCorrection(
                "id is empty".into(),
            ));
        }
        for (name, v) in [
            ("nucleus_dose_fraction", self.nucleus_dose_fraction),
            ("nucleus_dose_factor", self.nucleus_dose_factor),
            (
                "nucleus_dose_factor_1sigma",
                self.nucleus_dose_factor_1sigma,
            ),
            ("intercellular_dose_cv", self.intercellular_dose_cv),
        ] {
            if !v.is_finite() || v < 0.0 {
                return Err(MicrodistributionError::InvalidCorrection(format!(
                    "{name} must be a non-negative finite value"
                )));
            }
        }
        for (name, deposition) in [
            ("nucleus", self.deposition.nucleus),
            ("cytoplasm", self.deposition.cytoplasm),
            ("membrane", self.deposition.membrane),
            ("extracellular", self.deposition.extracellular),
            ("uniform_reference", self.uniform_reference),
        ] {
            for (quantity, v) in [
                ("alpha_to_nucleus", deposition.alpha_to_nucleus),
                ("li_to_nucleus", deposition.li_to_nucleus),
                ("combined_to_nucleus", deposition.combined_to_nucleus),
            ] {
                if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                    return Err(MicrodistributionError::InvalidCorrection(format!(
                        "{name}.{quantity} must lie in [0, 1]"
                    )));
                }
            }
        }
        self.model.validate()?;
        if self.qualification != MICRODISTRIBUTION_CORRECTION_QUALIFICATION {
            return Err(MicrodistributionError::InvalidCorrection(
                "qualification mismatch".into(),
            ));
        }
        if self.provenance_id.trim().is_empty() {
            return Err(MicrodistributionError::InvalidCorrection(
                "provenance_id is empty".into(),
            ));
        }
        Ok(())
    }
}

/// Deterministic Fibonacci-sphere point set: `n` nearly-uniform unit
/// direction vectors, fully reproducible with no RNG.
fn fibonacci_directions(n: usize) -> Vec<[f64; 3]> {
    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    (0..n)
        .map(|i| {
            let z = 1.0 - (2.0 * i as f64 + 1.0) / n as f64;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = golden * i as f64;
            [r * phi.cos(), r * phi.sin(), z]
        })
        .collect()
}

/// Chord length (µm) of a ray through the nucleus. The site sits at
/// `(r, 0, 0)` by spherical symmetry; the nucleus is the sphere of
/// radius `rn` centered at the origin. Returns 0 when the ray misses
/// or points away.
fn nucleus_chord_um(r: f64, direction: [f64; 3], rn: f64) -> f64 {
    // |p + t·u|² = rn² ⇒ t² + 2(p·u)t + r² − rn² = 0.
    let b = r * direction[0];
    let discriminant = b * b - (r * r - rn * rn);
    if discriminant <= 0.0 {
        return 0.0;
    }
    let d = discriminant.sqrt();
    let t_near = -b - d;
    let t_far = -b + d;
    if t_far <= 0.0 {
        return 0.0;
    }
    t_far - t_near.max(0.0)
}

/// Mean fraction of a track's energy deposited in the nucleus for
/// decay sites distributed uniformly by volume over the radial shell
/// `[r_in, r_out]` (a point shell when `r_in == r_out`), under the
/// straight-line constant-LET track model with CSDA range `range_um`.
/// `rn` is the nucleus radius.
fn shell_deposition_fraction(r_in: f64, r_out: f64, rn: f64, range_um: f64) -> f64 {
    let directions = fibonacci_directions(DIRECTION_SAMPLES);
    let mut weight_sum = 0.0;
    let mut acc = 0.0;
    for i in 0..RADIAL_SAMPLES {
        // Midpoint rule on r² dr — exact in the point-shell limit and
        // converges as 1/N² for shells.
        let r = if (r_out - r_in).abs() < f64::EPSILON {
            r_in
        } else {
            r_in + (r_out - r_in) * (i as f64 + 0.5) / RADIAL_SAMPLES as f64
        };
        let w = r * r;
        let dir_mean: f64 = directions
            .iter()
            .map(|u| nucleus_chord_um(r, *u, rn).min(range_um) / range_um)
            .sum::<f64>()
            / directions.len() as f64;
        acc += w * dir_mean;
        weight_sum += w;
    }
    if weight_sum <= 0.0 {
        0.0
    } else {
        acc / weight_sum
    }
}

/// Compute the deposition fraction for one compartment.
fn compartment_deposition(
    model: &BoronMicrodistribution,
    r_in: f64,
    r_out: f64,
) -> CompartmentDeposition {
    let rn = model.nucleus_radius_um;
    let alpha = shell_deposition_fraction(r_in, r_out, rn, model.alpha_range_um);
    let li = shell_deposition_fraction(r_in, r_out, rn, model.li_range_um);
    let total_energy = model.alpha_energy_mev + model.li_energy_mev;
    CompartmentDeposition {
        alpha_to_nucleus: alpha,
        li_to_nucleus: li,
        combined_to_nucleus: (model.alpha_energy_mev * alpha + model.li_energy_mev * li)
            / total_energy,
    }
}

/// Evaluate a microdistribution model into its nucleus-dose correction
/// record.
pub fn evaluate_microdistribution(
    model: &BoronMicrodistribution,
    id: &str,
    provenance_id: &str,
    model_reference: ContentReference,
) -> Result<MicrodistributionCorrection, MicrodistributionError> {
    model.validate()?;
    model_reference.validate()?;
    if id.trim().is_empty() {
        return Err(MicrodistributionError::InvalidCorrection(
            "correction id is empty".into(),
        ));
    }

    let rc = model.cell_radius_um;
    let rn = model.nucleus_radius_um;
    let re = model.extracellular_extent_um;

    let deposition = CorrectionDeposition {
        nucleus: compartment_deposition(model, 0.0, rn),
        cytoplasm: compartment_deposition(model, rn, rc),
        membrane: compartment_deposition(model, rc, rc),
        extracellular: compartment_deposition(model, rc, re),
    };
    // Uniform reference: sites uniform over the whole distribution
    // region, the conventional homogeneous-¹⁰B assumption.
    let uniform_reference = compartment_deposition(model, 0.0, re);

    let c = &model.compartments;
    let g = [
        (
            c.nucleus,
            c.nucleus_1sigma,
            deposition.nucleus.combined_to_nucleus,
        ),
        (
            c.cytoplasm,
            c.cytoplasm_1sigma,
            deposition.cytoplasm.combined_to_nucleus,
        ),
        (
            c.membrane,
            c.membrane_1sigma,
            deposition.membrane.combined_to_nucleus,
        ),
        (
            c.extracellular,
            c.extracellular_1sigma,
            deposition.extracellular.combined_to_nucleus,
        ),
    ];
    let nucleus_dose_fraction: f64 = g.iter().map(|(f, _, dep)| f * dep).sum();
    let reference = uniform_reference.combined_to_nucleus;
    let nucleus_dose_factor = if reference > 0.0 {
        nucleus_dose_fraction / reference
    } else {
        f64::INFINITY
    };
    // Linear propagation over the fraction sigmas; the geometric
    // deposition factors are exact inside the model.
    let variance: f64 = g.iter().map(|(_, sigma, dep)| (sigma * dep).powi(2)).sum();
    let factor_sigma = if reference > 0.0 {
        variance.sqrt() / reference
    } else {
        0.0
    };

    let correction = MicrodistributionCorrection {
        schema_version: MICRODISTRIBUTION_CORRECTION_SCHEMA.into(),
        id: id.into(),
        deposition,
        uniform_reference,
        nucleus_dose_fraction,
        nucleus_dose_factor,
        nucleus_dose_factor_1sigma: factor_sigma,
        intercellular_dose_cv: model.intercellular_cv,
        heterogeneity_note: "nucleus dose scales linearly with per-cell ¹⁰B uptake, so the \
            declared intercellular_cv transfers unchanged to the cell-to-cell dose distribution; \
            its effect on population survival is nonlinear and materializes in downstream \
            isoeffective/survival evaluation, not in this record"
            .into(),
        model: model_reference,
        qualification: MICRODISTRIBUTION_CORRECTION_QUALIFICATION.into(),
        provenance_id: provenance_id.into(),
    };
    correction.validate()?;
    Ok(correction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> BoronMicrodistribution {
        BoronMicrodistribution {
            schema_version: BORON_MICRODISTRIBUTION_SCHEMA.into(),
            id: "openbnct.boron-microdistribution.test.v1".into(),
            cell_radius_um: 10.0,
            nucleus_radius_um: 5.0,
            extracellular_extent_um: 12.0,
            compartments: CompartmentFractions {
                nucleus: 0.6,
                nucleus_1sigma: 0.1,
                cytoplasm: 0.2,
                cytoplasm_1sigma: 0.05,
                membrane: 0.1,
                membrane_1sigma: 0.02,
                extracellular: 0.1,
                extracellular_1sigma: 0.02,
            },
            intercellular_cv: 0.3,
            intercellular_cv_1sigma: 0.05,
            alpha_energy_mev: 1.47,
            alpha_range_um: 9.0,
            li_energy_mev: 0.84,
            li_range_um: 4.0,
            validity_domain: "BPA-fr in glioma cells, adopted literature geometry".into(),
            provenance_id: "test".into(),
        }
    }

    fn reference() -> ContentReference {
        ContentReference {
            id: "model".into(),
            sha256: "0".repeat(64),
        }
    }

    #[test]
    fn valid_model_passes() {
        model().validate().unwrap();
    }

    #[test]
    fn rejects_unsummed_fractions_and_bad_geometry() {
        let mut bad = model();
        bad.compartments.nucleus = 0.9;
        assert!(bad.validate().is_err());
        let mut bad = model();
        bad.nucleus_radius_um = 12.0;
        assert!(bad.validate().is_err());
        let mut bad = model();
        bad.validity_domain.clear();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn nuclear_boron_beats_uniform_and_extracellular_loses() {
        let m = model();
        // All in nucleus → factor well above 1.
        let mut nuclear = m.clone();
        nuclear.compartments.nucleus = 1.0;
        nuclear.compartments.cytoplasm = 0.0;
        nuclear.compartments.membrane = 0.0;
        nuclear.compartments.extracellular = 0.0;
        let c_nuclear = evaluate_microdistribution(&nuclear, "n", "p", reference()).unwrap();
        assert!(c_nuclear.nucleus_dose_factor > 1.0);

        // All extracellular → below 1.
        let mut extra = m.clone();
        extra.compartments.nucleus = 0.0;
        extra.compartments.cytoplasm = 0.0;
        extra.compartments.membrane = 0.0;
        extra.compartments.extracellular = 1.0;
        let c_extra = evaluate_microdistribution(&extra, "e", "p", reference()).unwrap();
        assert!(c_extra.nucleus_dose_factor < 1.0);

        // Uniform fractions matching the geometry volumes reproduce the
        // reference → factor ≈ 1.
        let rn3 = 125.0_f64;
        let rc3 = 1000.0_f64;
        let re3 = 1728.0_f64;
        let mut uniform = m.clone();
        uniform.compartments.nucleus = rn3 / re3;
        uniform.compartments.cytoplasm = (rc3 - rn3) / re3;
        uniform.compartments.membrane = 0.0;
        uniform.compartments.extracellular = (re3 - rc3) / re3;
        let c_uniform = evaluate_microdistribution(&uniform, "u", "p", reference()).unwrap();
        assert!((c_uniform.nucleus_dose_factor - 1.0).abs() < 0.02);
    }

    #[test]
    fn nucleus_sites_always_hit() {
        let m = model();
        let dep = compartment_deposition(&m, 0.0, m.nucleus_radius_um);
        // From inside the nucleus the α always intersects it; the only
        // loss is track range exceeding the chord.
        assert!(dep.alpha_to_nucleus > 0.4);
        assert!(dep.li_to_nucleus > 0.6);
        assert!(dep.combined_to_nucleus > dep.li_to_nucleus - 0.2);
    }

    #[test]
    fn chord_math_is_exact() {
        // Site at r = 10, nucleus rn = 5: a ray straight inward crosses
        // a full diameter.
        assert_eq!(nucleus_chord_um(10.0, [-1.0, 0.0, 0.0], 5.0), 10.0);
        // Grazing ray at exactly the tangent → zero chord.
        assert_eq!(nucleus_chord_um(10.0, [-0.6, 0.8, 0.0], 5.0), 0.0);
        // Outward ray from inside → exits through the far surface.
        let c = nucleus_chord_um(2.0, [1.0, 0.0, 0.0], 5.0);
        assert!((c - 3.0).abs() < 1e-9);
    }

    #[test]
    fn heterogeneity_cv_transfers_and_note_is_present() {
        let c = evaluate_microdistribution(&model(), "c", "p", reference()).unwrap();
        assert_eq!(c.intercellular_dose_cv, 0.3);
        assert!(c.heterogeneity_note.contains("nonlinear"));
        c.validate().unwrap();
    }

    #[test]
    fn factor_sigma_propagates_fraction_sigmas() {
        let mut m = model();
        m.compartments.nucleus_1sigma = 0.0;
        m.compartments.cytoplasm_1sigma = 0.0;
        m.compartments.membrane_1sigma = 0.0;
        m.compartments.extracellular_1sigma = 0.0;
        let c = evaluate_microdistribution(&m, "c", "p", reference()).unwrap();
        assert_eq!(c.nucleus_dose_factor_1sigma, 0.0);
    }
}
