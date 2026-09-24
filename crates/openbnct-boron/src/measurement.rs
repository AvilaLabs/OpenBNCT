//! Measured ¹⁰B subcellular microdistribution import.
//!
//! `openbnct.boron-microdistribution-measurement/0.1.0` declares what an
//! assay actually reported — compartment mass fractions directly, or a
//! radial boron-density profile binned in radius — under the assay's own
//! declared geometry, method, compound, and cell system. `import` reduces
//! the measurement to the `openbnct.boron-microdistribution/0.1.0` model
//! document the correction and stochastic layers consume, so measured
//! subcellular data enters the workbench content-bound rather than
//! hand-translated.
//!
//! Radial profiles cannot resolve the membrane compartment — the
//! measurement declares a membrane fraction explicitly and the import
//! scales the profile-derived masses to fit it. Per-bin σ propagates
//! through the linear mass integration and the fraction normalization
//! by the first-order Jacobian; cross-bin correlations are not declared
//! by this schema (diagonal propagation, stated in the artifact).

use serde::{Deserialize, Serialize};

use crate::microdistribution::{
    BORON_MICRODISTRIBUTION_SCHEMA, BoronMicrodistribution, CompartmentFractions,
    MicrodistributionError,
};

/// Current schema token for measured microdistribution artifacts.
pub const BORON_MICRODISTRIBUTION_MEASUREMENT_SCHEMA: &str =
    "openbnct.boron-microdistribution-measurement/0.1.0";

/// Conventional emitted-particle parameters — the dominant 94%-branch
/// ¹⁰B(n,α)⁷Li values, matching the model documents' declared defaults.
const DEFAULT_ALPHA_ENERGY_MEV: f64 = 1.47;
const DEFAULT_ALPHA_RANGE_UM: f64 = 9.0;
const DEFAULT_LI_ENERGY_MEV: f64 = 0.84;
const DEFAULT_LI_RANGE_UM: f64 = 5.0;

/// How the subcellular boron was assayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrodistributionAssay {
    /// Grain/track autoradiography over sectioned cells.
    Autoradiography,
    /// Scanned ion microbeam with spatially-resolved capture products.
    IonMicrobeam,
    /// Track-imaging detectors (FNTD, Timepix-class) reconstructing
    /// individual α/⁷Li tracks back to capture sites.
    TrackImaging,
    /// Fluorescence/spectroscopic localization of a labelled carrier.
    Fluorescence,
    /// Another declared method — `assay_detail` carries the specifics.
    Other,
}

/// One annulus of a measured radial boron-density profile.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadialDensityBin {
    /// Annulus inner radius, µm.
    pub radius_lo_um: f64,
    /// Annulus outer radius, µm (exclusive; must exceed `radius_lo_um`).
    pub radius_hi_um: f64,
    /// Measured ¹⁰B areal density in the annulus — atoms/µm² or any
    /// consistent relative units (fractions are unit-free).
    pub density: f64,
    /// 1σ on `density`.
    pub density_1sigma: f64,
}

/// What the assay reported.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MicrodistributionEvidence {
    /// The assay reported compartment mass fractions directly (with 1σ).
    CompartmentFractions { fractions: CompartmentFractions },
    /// A radial boron-density profile. Membrane is unresolvable at
    /// radial-bin scale — `membrane_fraction` declares its share of the
    /// total boron mass and the profile supplies the rest.
    RadialProfile {
        bins: Vec<RadialDensityBin>,
        /// Fraction of total ¹⁰B attributed to the membrane compartment.
        membrane_fraction: f64,
        /// 1σ on `membrane_fraction`.
        membrane_fraction_1sigma: f64,
    },
}

/// A declared microdistribution measurement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronMicrodistributionMeasurement {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub assay: MicrodistributionAssay,
    /// Free-text assay detail — instrument, protocol, sectioning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assay_detail: Option<String>,
    /// Boron carrier assayed (BPA, BSH, …).
    pub compound: String,
    /// Cell system measured (line, conditions).
    pub cell_system: String,
    /// Cell radius, µm — the geometry the measurement is reduced against.
    pub cell_radius_um: f64,
    /// Nucleus radius, µm (must be smaller than `cell_radius_um`).
    pub nucleus_radius_um: f64,
    /// Extracellular boundary, µm (must exceed `cell_radius_um`).
    pub extracellular_extent_um: f64,
    /// The measured content.
    pub evidence: MicrodistributionEvidence,
    /// Emitted α kinetic energy, MeV — defaults to the 94%-branch value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_energy_mev: Option<f64>,
    /// Emitted α CSDA range in unit-density tissue, µm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpha_range_um: Option<f64>,
    /// Emitted ⁷Li kinetic energy, MeV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub li_energy_mev: Option<f64>,
    /// Emitted ⁷Li CSDA range in unit-density tissue, µm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub li_range_um: Option<f64>,
    /// Intercellular uptake CV if the assay resolves per-cell uptake —
    /// required by the stochastic layer; 0 declares unresolved
    /// heterogeneity.
    #[serde(default)]
    pub intercellular_cv: Option<f64>,
    /// 1σ on `intercellular_cv`.
    #[serde(default)]
    pub intercellular_cv_1sigma: Option<f64>,
    /// Free-text validity domain carried to the model document.
    pub validity_domain: String,
    pub provenance_id: String,
}

impl BoronMicrodistributionMeasurement {
    /// Structural validation; called before import.
    pub fn validate(&self) -> Result<(), MicrodistributionError> {
        if !openbnct_core::schema_matches(
            &self.schema_version,
            BORON_MICRODISTRIBUTION_MEASUREMENT_SCHEMA,
        ) {
            return Err(MicrodistributionError::InvalidModel(format!(
                "unsupported measurement schema {:?}",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("compound", self.compound.as_str()),
            ("cell_system", self.cell_system.as_str()),
            ("validity_domain", self.validity_domain.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(MicrodistributionError::InvalidModel(format!(
                    "{label} is empty"
                )));
            }
        }
        if self.assay == MicrodistributionAssay::Other
            && self
                .assay_detail
                .as_deref()
                .is_none_or(|d| d.trim().is_empty())
        {
            return Err(MicrodistributionError::InvalidModel(
                "assay `other` requires a non-empty assay_detail".into(),
            ));
        }
        if !(self.nucleus_radius_um > 0.0
            && self.nucleus_radius_um < self.cell_radius_um
            && self.cell_radius_um < self.extracellular_extent_um)
        {
            return Err(MicrodistributionError::InvalidModel(
                "require 0 < nucleus_radius < cell_radius < extracellular_extent".into(),
            ));
        }
        for (label, v) in [
            ("alpha_energy_mev", self.alpha_energy_mev),
            ("alpha_range_um", self.alpha_range_um),
            ("li_energy_mev", self.li_energy_mev),
            ("li_range_um", self.li_range_um),
            ("intercellular_cv", self.intercellular_cv),
            ("intercellular_cv_1sigma", self.intercellular_cv_1sigma),
        ] {
            if v.is_some_and(|v| !v.is_finite() || v < 0.0) {
                return Err(MicrodistributionError::InvalidModel(format!(
                    "{label} must be finite and nonnegative"
                )));
            }
        }
        match &self.evidence {
            MicrodistributionEvidence::CompartmentFractions { fractions } => {
                for (label, v) in [
                    ("nucleus", fractions.nucleus),
                    ("cytoplasm", fractions.cytoplasm),
                    ("membrane", fractions.membrane),
                    ("extracellular", fractions.extracellular),
                ] {
                    if !v.is_finite() || v < 0.0 {
                        return Err(MicrodistributionError::InvalidModel(format!(
                            "fraction {label} must be finite and nonnegative"
                        )));
                    }
                }
                let sum = fractions.nucleus
                    + fractions.cytoplasm
                    + fractions.membrane
                    + fractions.extracellular;
                if sum <= 0.0 {
                    return Err(MicrodistributionError::InvalidModel(
                        "compartment fractions sum to zero".into(),
                    ));
                }
            }
            MicrodistributionEvidence::RadialProfile {
                bins,
                membrane_fraction,
                ..
            } => {
                if bins.is_empty() {
                    return Err(MicrodistributionError::InvalidModel(
                        "radial profile has no bins".into(),
                    ));
                }
                if !membrane_fraction.is_finite()
                    || *membrane_fraction < 0.0
                    || *membrane_fraction >= 1.0
                {
                    return Err(MicrodistributionError::InvalidModel(
                        "membrane_fraction must be in [0, 1)".into(),
                    ));
                }
                let mut prev_hi = f64::NEG_INFINITY;
                for bin in bins {
                    if !(bin.radius_lo_um.is_finite()
                        && bin.radius_hi_um > bin.radius_lo_um
                        && bin.density.is_finite()
                        && bin.density >= 0.0
                        && bin.density_1sigma.is_finite()
                        && bin.density_1sigma >= 0.0)
                    {
                        return Err(MicrodistributionError::InvalidModel(
                            "radial bin requires finite radii hi>lo and nonnegative density, σ"
                                .into(),
                        ));
                    }
                    if bin.radius_lo_um < prev_hi {
                        return Err(MicrodistributionError::InvalidModel(
                            "radial bins must not overlap".into(),
                        ));
                    }
                    prev_hi = bin.radius_hi_um;
                }
            }
        }
        Ok(())
    }
}

/// Interval overlap length of [lo,hi) with [a,b).
fn overlap(lo: f64, hi: f64, a: f64, b: f64) -> f64 {
    (hi.min(b) - lo.max(a)).max(0.0)
}

/// Compartment index for the linear mass map.
const NUCLEUS: usize = 0;
const CYTOPLASM: usize = 1;
const EXTRACELLULAR: usize = 2;

/// Reduce a radial profile to compartment masses.
///
/// Each bin's measured mass `density_i·A_i` distributes over the three
/// profile-resolvable compartments (nucleus r<R_n, cytoplasm
/// R_n≤r<R_c, extracellular R_c≤r<R_ext) in proportion to the annulus's
/// radial overlap with each region. The returned 3×bins Jacobian maps
/// density perturbations to compartment-mass perturbations — the linear
/// propagation handle for per-bin σ.
fn profile_compartment_masses(
    bins: &[RadialDensityBin],
    nucleus_r: f64,
    cell_r: f64,
    ext_r: f64,
) -> ([f64; 3], Vec<[f64; 3]>) {
    let regions = [(0.0_f64, nucleus_r), (nucleus_r, cell_r), (cell_r, ext_r)];
    let mut masses = [0.0_f64; 3];
    let mut jacobian = Vec::with_capacity(bins.len());
    for bin in bins {
        let area = std::f64::consts::PI
            * (bin.radius_hi_um * bin.radius_hi_um - bin.radius_lo_um * bin.radius_lo_um);
        let width = bin.radius_hi_um - bin.radius_lo_um;
        let mut jac = [0.0_f64; 3];
        for (c, &(a, b)) in regions.iter().enumerate() {
            let share = overlap(bin.radius_lo_um, bin.radius_hi_um, a, b) / width;
            jac[c] = area * share;
            masses[c] += bin.density * jac[c];
        }
        jacobian.push(jac);
    }
    (masses, jacobian)
}

/// Reduce the measurement to a `BoronMicrodistribution` model document.
/// The emitted model inherits the measurement's declared geometry,
/// validity domain, and (defaulted) emitted-particle parameters; the
/// model id is `"{measurement.id}.model"` unless overridden.
pub fn import_measurement(
    measurement: &BoronMicrodistributionMeasurement,
    model_id: Option<String>,
) -> Result<BoronMicrodistribution, MicrodistributionError> {
    measurement.validate()?;
    let fractions = match &measurement.evidence {
        MicrodistributionEvidence::CompartmentFractions { fractions } => {
            // Normalize the reported fractions defensively — the model
            // contract requires an exact unit sum; a measured report may
            // carry rounding drift.
            let sum = fractions.nucleus
                + fractions.cytoplasm
                + fractions.membrane
                + fractions.extracellular;
            CompartmentFractions {
                nucleus: fractions.nucleus / sum,
                nucleus_1sigma: fractions.nucleus_1sigma / sum,
                cytoplasm: fractions.cytoplasm / sum,
                cytoplasm_1sigma: fractions.cytoplasm_1sigma / sum,
                membrane: fractions.membrane / sum,
                membrane_1sigma: fractions.membrane_1sigma / sum,
                extracellular: fractions.extracellular / sum,
                extracellular_1sigma: fractions.extracellular_1sigma / sum,
            }
        }
        MicrodistributionEvidence::RadialProfile {
            bins,
            membrane_fraction,
            membrane_fraction_1sigma,
        } => {
            let (masses, jacobian) = profile_compartment_masses(
                bins,
                measurement.nucleus_radius_um,
                measurement.cell_radius_um,
                measurement.extracellular_extent_um,
            );
            // Per-compartment mass σ under diagonal per-bin σ.
            let mut mass_var = [0.0_f64; 3];
            for (bin, jac) in bins.iter().zip(&jacobian) {
                for c in 0..3 {
                    mass_var[c] += (jac[c] * bin.density_1sigma).powi(2);
                }
            }
            // The profile measures (1 − f_m) of the total boron mass:
            // F_c = m_c·(1−f_m)/profile_total, membrane takes f_m.
            let profile_total: f64 = masses.iter().sum();
            if profile_total <= 0.0 {
                return Err(MicrodistributionError::InvalidModel(
                    "radial profile integrates to zero mass".into(),
                ));
            }
            let scale = 1.0 - membrane_fraction;
            let profile_total_sigma = (mass_var[0] + mass_var[1] + mass_var[2]).sqrt();
            let comp_fraction = |c: usize| masses[c] * scale / profile_total;
            // First-order σ on F_c = m_c·s/T under diagonal inputs.
            let frac_sigma = |c: usize| -> f64 {
                let f = comp_fraction(c);
                let rel2 = if masses[c] > 0.0 {
                    (mass_var[c].sqrt() / masses[c]).powi(2)
                } else {
                    0.0
                } + if scale > 0.0 {
                    (membrane_fraction_1sigma / scale).powi(2)
                } else {
                    0.0
                } + (profile_total_sigma / profile_total).powi(2);
                f * rel2.sqrt()
            };
            CompartmentFractions {
                nucleus: comp_fraction(NUCLEUS),
                nucleus_1sigma: frac_sigma(NUCLEUS),
                cytoplasm: comp_fraction(CYTOPLASM),
                cytoplasm_1sigma: frac_sigma(CYTOPLASM),
                membrane: *membrane_fraction,
                membrane_1sigma: *membrane_fraction_1sigma,
                extracellular: comp_fraction(EXTRACELLULAR),
                extracellular_1sigma: frac_sigma(EXTRACELLULAR),
            }
        }
    };
    Ok(BoronMicrodistribution {
        schema_version: BORON_MICRODISTRIBUTION_SCHEMA.into(),
        id: model_id.unwrap_or_else(|| format!("{}.model", measurement.id)),
        cell_radius_um: measurement.cell_radius_um,
        nucleus_radius_um: measurement.nucleus_radius_um,
        extracellular_extent_um: measurement.extracellular_extent_um,
        compartments: fractions,
        intercellular_cv: measurement.intercellular_cv.unwrap_or(0.0),
        intercellular_cv_1sigma: measurement.intercellular_cv_1sigma.unwrap_or(0.0),
        alpha_energy_mev: measurement
            .alpha_energy_mev
            .unwrap_or(DEFAULT_ALPHA_ENERGY_MEV),
        alpha_range_um: measurement.alpha_range_um.unwrap_or(DEFAULT_ALPHA_RANGE_UM),
        li_energy_mev: measurement.li_energy_mev.unwrap_or(DEFAULT_LI_ENERGY_MEV),
        li_range_um: measurement.li_range_um.unwrap_or(DEFAULT_LI_RANGE_UM),
        validity_domain: format!(
            "{} (measured: {} on {}, {} cells)",
            measurement.validity_domain,
            measurement.compound,
            measurement.cell_system,
            match measurement.assay {
                MicrodistributionAssay::Autoradiography => "autoradiography",
                MicrodistributionAssay::IonMicrobeam => "ion microbeam",
                MicrodistributionAssay::TrackImaging => "track imaging",
                MicrodistributionAssay::Fluorescence => "fluorescence",
                MicrodistributionAssay::Other => "declared method",
            }
        ),
        provenance_id: measurement.provenance_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_measurement() -> BoronMicrodistributionMeasurement {
        BoronMicrodistributionMeasurement {
            schema_version: BORON_MICRODISTRIBUTION_MEASUREMENT_SCHEMA.into(),
            id: "meas".into(),
            assay: MicrodistributionAssay::TrackImaging,
            assay_detail: None,
            compound: "BPA".into(),
            cell_system: "test cells".into(),
            cell_radius_um: 10.0,
            nucleus_radius_um: 5.0,
            extracellular_extent_um: 15.0,
            evidence: MicrodistributionEvidence::CompartmentFractions {
                fractions: CompartmentFractions {
                    nucleus: 0.2,
                    nucleus_1sigma: 0.02,
                    cytoplasm: 0.3,
                    cytoplasm_1sigma: 0.03,
                    membrane: 0.1,
                    membrane_1sigma: 0.01,
                    extracellular: 0.4,
                    extracellular_1sigma: 0.04,
                },
            },
            alpha_energy_mev: None,
            alpha_range_um: None,
            li_energy_mev: None,
            li_range_um: None,
            intercellular_cv: Some(0.35),
            intercellular_cv_1sigma: Some(0.05),
            validity_domain: "test domain".into(),
            provenance_id: "test".into(),
        }
    }

    #[test]
    fn compartment_fraction_import_normalizes_and_passes_geometry() {
        let mut m = base_measurement();
        // Measured reports may not sum exactly to 1.
        if let MicrodistributionEvidence::CompartmentFractions { ref mut fractions } = m.evidence {
            fractions.nucleus = 0.21; // sum now 1.01
        }
        let model = import_measurement(&m, None).unwrap();
        model.validate().unwrap();
        let c = &model.compartments;
        let sum = c.nucleus + c.cytoplasm + c.membrane + c.extracellular;
        assert!((sum - 1.0).abs() < 1e-12);
        assert!((c.nucleus - 0.21 / 1.01).abs() < 1e-12);
        assert_eq!(model.id, "meas.model");
        assert_eq!(model.intercellular_cv, 0.35);
        assert_eq!(model.alpha_energy_mev, DEFAULT_ALPHA_ENERGY_MEV);
    }

    #[test]
    fn radial_profile_integrates_annuli_into_compartments() {
        // Uniform density 1.0 everywhere → compartment masses ∝ areas:
        // nucleus π·25, cytoplasm π·(100−25), extracellular π·(225−100).
        let mut m = base_measurement();
        m.evidence = MicrodistributionEvidence::RadialProfile {
            bins: vec![
                RadialDensityBin {
                    radius_lo_um: 0.0,
                    radius_hi_um: 5.0,
                    density: 1.0,
                    density_1sigma: 0.0,
                },
                RadialDensityBin {
                    radius_lo_um: 5.0,
                    radius_hi_um: 10.0,
                    density: 1.0,
                    density_1sigma: 0.0,
                },
                RadialDensityBin {
                    radius_lo_um: 10.0,
                    radius_hi_um: 15.0,
                    density: 1.0,
                    density_1sigma: 0.0,
                },
            ],
            membrane_fraction: 0.1,
            membrane_fraction_1sigma: 0.0,
        };
        let model = import_measurement(&m, None).unwrap();
        model.validate().unwrap();
        let c = &model.compartments;
        let sum = c.nucleus + c.cytoplasm + c.membrane + c.extracellular;
        assert!((sum - 1.0).abs() < 1e-12, "fractions must sum to 1");
        assert!((c.membrane - 0.1).abs() < 1e-12);
        // 0.9 of the mass splits 25:75:125 = 1:3:5.
        assert!((c.nucleus - 0.9 * 25.0 / 225.0).abs() < 1e-12);
        assert!((c.cytoplasm - 0.9 * 75.0 / 225.0).abs() < 1e-12);
        assert!((c.extracellular - 0.9 * 125.0 / 225.0).abs() < 1e-12);
    }

    #[test]
    fn radial_profile_splits_straddling_bins() {
        // One bin straddling the nucleus/cytoplasm boundary at r=5:
        // [4,8) — 1µm in nucleus, 3µm in cytoplasm.
        let mut m = base_measurement();
        m.evidence = MicrodistributionEvidence::RadialProfile {
            bins: vec![RadialDensityBin {
                radius_lo_um: 4.0,
                radius_hi_um: 8.0,
                density: 2.0,
                density_1sigma: 0.0,
            }],
            membrane_fraction: 0.0,
            membrane_fraction_1sigma: 0.0,
        };
        let model = import_measurement(&m, None).unwrap();
        let c = &model.compartments;
        let area = std::f64::consts::PI * (64.0 - 16.0);
        // nucleus share 1/4 of the mass, cytoplasm 3/4.
        assert!((c.nucleus - 2.0 * area * 0.25 / (2.0 * area)).abs() < 1e-12);
        assert!((c.cytoplasm - 0.75).abs() < 1e-12);
        assert_eq!(c.membrane, 0.0);
        assert_eq!(c.extracellular, 0.0);
    }

    #[test]
    fn import_rejects_overlapping_bins_and_bad_fractions() {
        let mut m = base_measurement();
        m.evidence = MicrodistributionEvidence::RadialProfile {
            bins: vec![
                RadialDensityBin {
                    radius_lo_um: 0.0,
                    radius_hi_um: 6.0,
                    density: 1.0,
                    density_1sigma: 0.0,
                },
                RadialDensityBin {
                    radius_lo_um: 5.0,
                    radius_hi_um: 9.0,
                    density: 1.0,
                    density_1sigma: 0.0,
                },
            ],
            membrane_fraction: 0.0,
            membrane_fraction_1sigma: 0.0,
        };
        assert!(import_measurement(&m, None).is_err());
        m.evidence = MicrodistributionEvidence::RadialProfile {
            bins: vec![RadialDensityBin {
                radius_lo_um: 0.0,
                radius_hi_um: 5.0,
                density: 1.0,
                density_1sigma: 0.0,
            }],
            membrane_fraction: 1.2,
            membrane_fraction_1sigma: 0.0,
        };
        assert!(import_measurement(&m, None).is_err());
    }

    #[test]
    fn assay_other_requires_detail() {
        let mut m = base_measurement();
        m.assay = MicrodistributionAssay::Other;
        assert!(m.validate().is_err());
        m.assay_detail = Some("polarized muon spin resonance".into());
        m.validate().unwrap();
    }
}
