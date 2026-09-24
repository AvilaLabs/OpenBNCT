// SPDX-License-Identifier: MIT

//! Cell-level stochastic microdosimetry — the compartmented-cell layer
//! the homogeneous-site [`crate::lineal_tally`] explicitly leaves out.
//!
//! `sample_cell_microdosimetry` Monte-Carlo-samples ¹⁰B(n,α)⁷Li
//! captures across a cell population under a declared
//! `BoronMicrodistribution`: each cell's ¹⁰B amount is drawn from a
//! gamma uptake heterogeneity (shape 1/CV²), its capture count is
//! Poisson, and each capture places a back-to-back α/⁷Li pair in the
//! declared compartment (nucleus / cytoplasm / membrane /
//! extracellular shell) with an isotropic axis. Energy imparted to the
//! **nucleus** follows the same rectilinear CSDA convention as the
//! rest of the stack: ε = E·min(1, l/R) over the ray–sphere chord.
//!
//! What the sample produces that the deterministic layers cannot:
//!
//! - the full **specific-energy distribution** P(z) across cells —
//!   the stochastic axis the linearized MK model discards — including
//!   the **untouched fraction** P(z = 0), which sets the high-dose
//!   survival plateau MK has no way to express;
//! - the nucleus-domain lineal spectrum f(y) in a *compartmented*
//!   cell with nonuniform capture density (vs. the bare-sphere,
//!   uniform-source `lineal_tally` model);
//! - per-compartment capture tallies for audit.
//!
//! `evaluate_smk` then applies the stochastic-microdosimetric-kinetic
//! survival integral directly over the sampled z population —
//! S = ⟨exp(−a·z − b·z²)⟩ — plus the MK linearization of the same
//! data for an internal comparison, and an isosurvival RBE against a
//! declared photon LQ reference. Dose scaling is declared: sampled z
//! values rescale linearly with the macroscopic boron dose, the
//! standard SMK small-λ approximation — exact in the capture-rich
//! limit and stated in the artifact.
//!
//! All randomness flows from a single declared seed through a
//! splitmix64 stream: identical inputs and seed give bit-identical
//! artifacts. Research-only — no clinical RBE, CBE, or Gy-Eq claim.

use std::f64::consts::PI;

use openbnct_boron::BoronMicrodistribution;
use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};

use crate::{BioError, LINEAL_SPECTRUM_SCHEMA, LinealSpectrum, LinealWeighting};

/// Versioned cell-microdosimetry artifact schema.
pub const CELL_MICRODOSIMETRY_SCHEMA: &str = "openbnct.cell-microdosimetry/0.1.0";
/// Versioned SMK-evaluation artifact schema.
pub const SMK_EVALUATION_SCHEMA: &str = "openbnct.smk-evaluation/0.1.0";
/// Qualification string carried by both artifacts.
pub const CELL_MICRODOSIMETRY_QUALIFICATION: &str =
    "cell_microdosimetry_research_only_not_clinical";

/// Joules per keV (exact SI conversion constant).
const JOULE_PER_KEV: f64 = 1.602_176_634e-16;

/// Deterministic splitmix64 stream — the declared-entropy source.
struct Splitmix64(u64);

impl Splitmix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Poisson draw by Knuth's product method — declared adequate for
    /// capture means up to ~100 (each draw costs ⌈λ⌉ uniform steps).
    fn poisson(&mut self, lambda: f64) -> u64 {
        let l = (-lambda).exp();
        let mut k = 0u64;
        let mut p = 1.0_f64;
        loop {
            p *= self.f64();
            if p <= l {
                return k;
            }
            k += 1;
        }
    }

    /// Gamma(shape k, scale 1) by Marsaglia–Tsang; k ≥ 1 is handled by
    /// the direct method, k < 1 by the standard boost trick.
    fn gamma_unit(&mut self, k: f64) -> f64 {
        if k < 1.0 {
            return self.gamma_unit(k + 1.0) * self.f64().powf(1.0 / k);
        }
        let d = k - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            // Box–Muller normal.
            let u1 = self.f64().max(f64::MIN_POSITIVE);
            let u2 = self.f64();
            let normal = (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos();
            let v = (1.0 + c * normal).powi(3);
            if v <= 0.0 {
                continue;
            }
            let u = self.f64();
            if u < 1.0 - 0.0331 * normal.powi(4)
                || u.ln() < 0.5 * normal * normal + d * (1.0 - v + v.ln())
            {
                return d * v;
            }
        }
    }
}

/// Declared histogram binning shared by the z and y spectra.
fn bin_index(edges: &[f64], value: f64) -> Option<usize> {
    if value < edges[0] || value >= *edges.last()? {
        return None;
    }
    edges.windows(2).position(|w| value >= w[0] && value < w[1])
}

/// Per-compartment capture shares actually drawn (audit counters).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompartmentTallies {
    pub nucleus: u64,
    pub cytoplasm: u64,
    pub membrane: u64,
    pub extracellular: u64,
}

/// Statistics of the run, recorded so the artifact stands alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingStatistics {
    pub cells_simulated: u32,
    pub captures_simulated: u64,
    /// Captures depositing nonzero energy in the nucleus.
    pub nucleus_hits: u64,
    /// Per-compartment capture counts drawn.
    pub compartment_captures: CompartmentTallies,
}

/// The declared sampling declaration, verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SamplingDeclaration {
    /// Expected ¹⁰B(n,α) captures per cell at the scenario's boron dose.
    pub mean_captures_per_cell: f64,
    /// Cell population size.
    pub cell_count: u32,
    /// splitmix64 stream seed.
    pub seed: u64,
    /// Nucleus mass density in kg/m³ — z is per unit nucleus mass.
    pub nucleus_density_kg_m3: f64,
}

/// A sampled cell-population microdosimetry record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellMicrodosimetry {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding to the `openbnct.boron-microdistribution` model.
    pub microdistribution: ContentReference,
    /// Echoed geometry so the artifact reads standalone.
    pub cell_radius_um: f64,
    pub nucleus_radius_um: f64,
    pub extracellular_extent_um: f64,
    pub sampling: SamplingDeclaration,
    /// Specific-energy histogram in Gy: `n+1` edges, `n` cell counts.
    pub z_bin_edges_gy: Vec<f64>,
    pub z_bin_counts: Vec<u64>,
    /// Cells receiving zero nucleus energy — the survival plateau
    /// the MK model cannot express.
    pub untouched_fraction: f64,
    /// Population-mean nucleus specific energy, Gy.
    pub mean_specific_energy_gy: f64,
    /// Nucleus-domain event-frequency lineal spectrum — a complete
    /// `openbnct.lineal-spectrum` document, directly consumable by
    /// the MKM family as `LinealEnergySource::Spectrum`.
    pub nucleus_lineal_spectrum: LinealSpectrum,
    pub statistics: SamplingStatistics,
    /// Stated approximation scope.
    pub validity_note: String,
    pub provenance_id: String,
    pub qualification: String,
}

/// Monte-Carlo-sample a cell population under `model`. Errors on
/// malformed binning or nonpositive counts/density.
#[allow(clippy::too_many_arguments)]
pub fn sample_cell_microdosimetry(
    model: &BoronMicrodistribution,
    microdistribution: ContentReference,
    mean_captures_per_cell: f64,
    cell_count: u32,
    seed: u64,
    z_bin_edges_gy: &[f64],
    y_bin_edges_kev_um: &[f64],
    id: &str,
    provenance_id: &str,
) -> Result<CellMicrodosimetry, BioError> {
    for (label, value) in [
        ("mean_captures_per_cell", mean_captures_per_cell),
        ("nucleus_radius_um", model.nucleus_radius_um),
        ("cell_radius_um", model.cell_radius_um),
        ("extracellular_extent_um", model.extracellular_extent_um),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(BioError::Invalid(format!(
                "{label} must be a non-negative finite"
            )));
        }
    }
    if cell_count == 0 {
        return Err(BioError::Invalid("cell_count must be ≥ 1".into()));
    }
    for (label, edges) in [
        ("z_bin_edges_gy", z_bin_edges_gy),
        ("y_bin_edges_kev_um", y_bin_edges_kev_um),
    ] {
        if edges.len() < 2
            || !edges.iter().all(|e| e.is_finite() && *e >= 0.0)
            || !edges.windows(2).all(|w| w[1] > w[0])
        {
            return Err(BioError::Invalid(format!(
                "{label} must be n+1 ≥ 2 finite strictly-increasing edges"
            )));
        }
    }
    let density_kg_m3 = 1000.0_f64; // unit-density tissue, declared below.
    let rn = model.nucleus_radius_um;
    let rc = model.cell_radius_um;
    let re = model.extracellular_extent_um;
    let nucleus_mass_kg = density_kg_m3 * 4.0 / 3.0 * PI * (rn * 1e-6).powi(3);
    // Mean chord of the nucleus domain (Cauchy: 4V/S = 4R/3 = 2d/3) —
    // the same convention `lineal_tally` declares.
    let mean_chord_um = 4.0 * rn / 3.0;
    let comp = &model.compartments;
    let mut rng = Splitmix64(seed);
    let cv2 = model.intercellular_cv * model.intercellular_cv;
    let mut z_counts = vec![0u64; z_bin_edges_gy.len() - 1];
    let mut y_counts = vec![0u64; y_bin_edges_kev_um.len() - 1];
    let mut stats = SamplingStatistics {
        cells_simulated: cell_count,
        captures_simulated: 0,
        nucleus_hits: 0,
        compartment_captures: CompartmentTallies::default(),
    };
    let mut untouched = 0u64;
    let mut z_sum = 0.0_f64;
    for _cell in 0..cell_count {
        // Uptake heterogeneity: gamma with mean 1 and variance CV² —
        // the declared cell-to-cell boron spread.
        let uptake = if cv2 > 0.0 {
            // Gamma(shape 1/CV², scale CV²): mean 1, variance CV².
            rng.gamma_unit(1.0 / cv2) * cv2
        } else {
            1.0
        };
        let captures = rng.poisson(uptake * mean_captures_per_cell);
        stats.captures_simulated += captures;
        let mut cell_z_kev = 0.0_f64;
        for _ in 0..captures {
            // Compartment draw on declared mass fractions.
            let draw = rng.f64();
            let (compartment, tally) = if draw < comp.nucleus {
                (0, &mut stats.compartment_captures.nucleus)
            } else if draw < comp.nucleus + comp.cytoplasm {
                (1, &mut stats.compartment_captures.cytoplasm)
            } else if draw < comp.nucleus + comp.cytoplasm + comp.membrane {
                (2, &mut stats.compartment_captures.membrane)
            } else {
                (3, &mut stats.compartment_captures.extracellular)
            };
            *tally += 1;
            // Position: uniform in the compartment's shell volume
            // (membrane fixes r = rc).
            let (r_lo, r_hi) = match compartment {
                0 => (0.0, rn),
                1 => (rn, rc),
                2 => (rc, rc),
                _ => (rc, re),
            };
            let r = if r_hi > r_lo {
                (r_lo.powi(3) + rng.f64() * (r_hi.powi(3) - r_lo.powi(3))).cbrt()
            } else {
                r_lo
            };
            // Isotropic emission point: random direction for the
            // radius vector, independent isotropic track axis.
            let position = random_direction(&mut rng).map(|v| v * r);
            let axis = random_direction(&mut rng);
            // α along +axis, ⁷Li along −axis — back-to-back.
            let eps_alpha = track_deposit_kev(&position, &axis, rn, model.alpha_range_um)
                * model.alpha_energy_mev
                * 1e3;
            let li_axis = [-axis[0], -axis[1], -axis[2]];
            let eps_li = track_deposit_kev(&position, &li_axis, rn, model.li_range_um)
                * model.li_energy_mev
                * 1e3;
            let eps = eps_alpha + eps_li;
            if eps > 0.0 {
                stats.nucleus_hits += 1;
                // One capture = one compound event in the nucleus.
                let y = eps / mean_chord_um;
                if let Some(bin) = bin_index(y_bin_edges_kev_um, y) {
                    y_counts[bin] += 1;
                }
            }
            cell_z_kev += eps;
        }
        let z_gy = cell_z_kev * JOULE_PER_KEV / nucleus_mass_kg;
        z_sum += z_gy;
        if z_gy == 0.0 {
            untouched += 1;
        } else if let Some(bin) = bin_index(z_bin_edges_gy, z_gy) {
            z_counts[bin] += 1;
        }
    }
    let spectrum = LinealSpectrum {
        schema_version: LINEAL_SPECTRUM_SCHEMA.into(),
        id: format!("{id}.nucleus-spectrum"),
        bin_edges_kev_um: y_bin_edges_kev_um.to_vec(),
        values: y_counts.iter().map(|c| *c as f64).collect(),
        absolute_standard_uncertainty: None,
        weighting: LinealWeighting::EventFrequency,
        value_unit: "compound_capture_events".into(),
        derivation: None,
        note: Some(
            "nucleus-domain lineal spectrum sampled in the compartmented \
             cell under the bound microdistribution; per-capture compound \
             events (α+⁷Li), rectilinear CSDA tracks, unit-density tissue"
                .into(),
        ),
    };
    let artifact_model_id = microdistribution.id.clone();
    let artifact = CellMicrodosimetry {
        schema_version: CELL_MICRODOSIMETRY_SCHEMA.into(),
        id: id.into(),
        microdistribution,
        cell_radius_um: rc,
        nucleus_radius_um: rn,
        extracellular_extent_um: re,
        sampling: SamplingDeclaration {
            mean_captures_per_cell,
            cell_count,
            seed,
            nucleus_density_kg_m3: density_kg_m3,
        },
        z_bin_edges_gy: z_bin_edges_gy.to_vec(),
        z_bin_counts: z_counts,
        untouched_fraction: untouched as f64 / cell_count as f64,
        mean_specific_energy_gy: z_sum / cell_count as f64,
        nucleus_lineal_spectrum: spectrum,
        statistics: stats,
        validity_note: format!(
            "Stochastic sample under model {:?} ({}): straight CSDA tracks, \
             constant LET, isotropic α/⁷Li back-to-back emission, Poisson \
             captures, gamma uptake heterogeneity (CV = {}), unit-density \
             nucleus. No straggling, no non-concentric geometry, no \
             cell-to-cell track sharing beyond the extracellular shell.",
            artifact_model_id, model.validity_domain, model.intercellular_cv
        ),
        provenance_id: provenance_id.into(),
        qualification: CELL_MICRODOSIMETRY_QUALIFICATION.into(),
    };
    Ok(artifact)
}

/// A declared SMK evaluation point: the survival integral at one
/// macroscopic boron-dose level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmkDosePoint {
    /// Macroscopic boron dose, Gy.
    pub dose_gy: f64,
    /// SMK population survival `⟨exp(−a·z − b·z²)⟩`.
    pub smk_survival: f64,
    /// MK linearization of the same sampled population —
    /// `exp(−(a + b·z̄)·z̄·scale…)` evaluated at the population mean.
    pub mk_survival: f64,
    /// Photon-reference dose giving equal survival, Gy.
    pub isosurvival_reference_gy: Option<f64>,
    /// `reference / boron` — the isosurvival RBE at this level.
    pub rbe: Option<f64>,
}

/// SMK model parameters, all declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmkParameters {
    /// Linear coefficient of lethal-lesion yield per Gy of nucleus
    /// specific energy.
    pub alpha_per_gy: f64,
    /// Quadratic coefficient, per Gy².
    pub beta_per_gy2: f64,
    /// Photon-reference LQ α (Gy⁻¹).
    pub reference_alpha_per_gy: f64,
    /// Photon-reference LQ β (Gy⁻²).
    pub reference_beta_per_gy2: f64,
    /// Macroscopic boron dose corresponding to the artifact's declared
    /// `mean_captures_per_cell` — the z-rescaling anchor.
    pub boron_dose_gy_at_mean_captures: f64,
    /// Dose levels to evaluate, Gy.
    pub dose_levels_gy: Vec<f64>,
}

/// The SMK evaluation record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmkEvaluation {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding to the `openbnct.cell-microdosimetry` artifact.
    pub cell_microdosimetry: ContentReference,
    pub parameters: SmkParameters,
    pub points: Vec<SmkDosePoint>,
    /// Declared z-rescaling approximation note.
    pub validity_note: String,
    pub provenance_id: String,
    pub qualification: String,
}

/// Evaluate SMK survival over a sampled z-population. The per-cell
/// nucleus energies in `artifact` are re-derived as a z-distribution
/// by replaying the same declared sampling — a second draw is *not*
/// taken: the artifact's own mean is the anchor and each sampled
/// cell's z rescales linearly with macroscopic dose (the declared
/// SMK small-λ approximation, stated in the record).
pub fn evaluate_smk(
    artifact: &CellMicrodosimetry,
    cell_microdosimetry: ContentReference,
    model: &BoronMicrodistribution,
    parameters: SmkParameters,
    id: &str,
    provenance_id: &str,
) -> Result<SmkEvaluation, BioError> {
    if parameters.alpha_per_gy < 0.0
        || parameters.beta_per_gy2 < 0.0
        || parameters.reference_alpha_per_gy < 0.0
        || parameters.reference_beta_per_gy2 < 0.0
        || parameters.boron_dose_gy_at_mean_captures <= 0.0
        || parameters.dose_levels_gy.is_empty()
        || parameters
            .dose_levels_gy
            .iter()
            .any(|d| !d.is_finite() || *d <= 0.0)
    {
        return Err(BioError::Invalid(
            "smk parameters must be finite, coefficients ≥ 0, anchor dose > 0, \
             and at least one positive dose level"
                .into(),
        ));
    }
    // Replay the declared sampling to recover the per-cell z list —
    // deterministic under the recorded seed.
    let z_list = replay_z_distribution(artifact, model)?;
    let anchor = parameters.boron_dose_gy_at_mean_captures;
    let mut points = Vec::with_capacity(parameters.dose_levels_gy.len());
    for &dose in &parameters.dose_levels_gy {
        let scale = dose / anchor;
        let mut smk_sum = 0.0_f64;
        let mut z_mean = 0.0_f64;
        for &z in &z_list {
            let zd = z * scale;
            smk_sum += (-parameters.alpha_per_gy * zd - parameters.beta_per_gy2 * zd * zd).exp();
            z_mean += zd;
        }
        z_mean /= z_list.len() as f64;
        let smk = smk_sum / z_list.len() as f64;
        // MK form at the population mean — the mean-field the SMK
        // integral relaxes.
        let mk =
            (-parameters.alpha_per_gy * z_mean - parameters.beta_per_gy2 * z_mean * z_mean).exp();
        let x = -smk.ln();
        // Isosurvival photon dose: α_r·D + β_r·D² = X.
        let (reference, rbe) = photon_isodose(
            x,
            parameters.reference_alpha_per_gy,
            parameters.reference_beta_per_gy2,
        )
        .map(|d| (Some(d), Some(d / dose)))
        .unwrap_or((None, None));
        points.push(SmkDosePoint {
            dose_gy: dose,
            smk_survival: smk,
            mk_survival: mk,
            isosurvival_reference_gy: reference,
            rbe,
        });
    }
    Ok(SmkEvaluation {
        schema_version: SMK_EVALUATION_SCHEMA.into(),
        id: id.into(),
        cell_microdosimetry,
        parameters,
        points,
        validity_note: "Sampled per-cell z rescales linearly with macroscopic \
             boron dose about the declared anchor — the SMK small-λ \
             approximation; exact in the capture-rich limit. Survival is the \
             population integral over the declared LQ-domain coefficients; \
             RBE is the isosurvival ratio against the declared photon LQ \
             reference. Research scope only — not a clinical RBE claim."
            .into(),
        provenance_id: provenance_id.into(),
        qualification: CELL_MICRODOSIMETRY_QUALIFICATION.into(),
    })
}

/// Solve α·D + β·D² = X for D ≥ 0 (X ≥ 0); `None` when X ≤ 0.
fn photon_isodose(x: f64, alpha: f64, beta: f64) -> Option<f64> {
    if x <= 0.0 || (alpha <= 0.0 && beta <= 0.0) {
        return None;
    }
    Some(if beta > 0.0 {
        ((alpha * alpha + 4.0 * beta * x).sqrt() - alpha) / (2.0 * beta)
    } else {
        x / alpha
    })
}

/// Replay the artifact's declared sampling to recover the per-cell
/// nucleus-z vector — identical to the run that produced the
/// artifact by seed determinism.
fn replay_z_distribution(
    artifact: &CellMicrodosimetry,
    model: &BoronMicrodistribution,
) -> Result<Vec<f64>, BioError> {
    let rn = artifact.nucleus_radius_um;
    let rc = artifact.cell_radius_um;
    let re = artifact.extracellular_extent_um;
    let nucleus_mass_kg =
        artifact.sampling.nucleus_density_kg_m3 * 4.0 / 3.0 * PI * (rn * 1e-6).powi(3);
    let comp = &model.compartments;
    let mut rng = Splitmix64(artifact.sampling.seed);
    let cv2 = model.intercellular_cv * model.intercellular_cv;
    let mut out = Vec::with_capacity(artifact.sampling.cell_count as usize);
    for _cell in 0..artifact.sampling.cell_count {
        let uptake = if cv2 > 0.0 {
            // Gamma(shape 1/CV², scale CV²): mean 1, variance CV².
            rng.gamma_unit(1.0 / cv2) * cv2
        } else {
            1.0
        };
        let captures = rng.poisson(uptake * artifact.sampling.mean_captures_per_cell);
        let mut cell_z_kev = 0.0_f64;
        for _ in 0..captures {
            let draw = rng.f64();
            let compartment = if draw < comp.nucleus {
                0
            } else if draw < comp.nucleus + comp.cytoplasm {
                1
            } else if draw < comp.nucleus + comp.cytoplasm + comp.membrane {
                2
            } else {
                3
            };
            let (r_lo, r_hi) = match compartment {
                0 => (0.0, rn),
                1 => (rn, rc),
                2 => (rc, rc),
                _ => (rc, re),
            };
            let r = if r_hi > r_lo {
                (r_lo.powi(3) + rng.f64() * (r_hi.powi(3) - r_lo.powi(3))).cbrt()
            } else {
                r_lo
            };
            let position = random_direction(&mut rng).map(|v| v * r);
            let axis = random_direction(&mut rng);
            let li_axis = [-axis[0], -axis[1], -axis[2]];
            cell_z_kev += track_deposit_kev(&position, &axis, rn, model.alpha_range_um)
                * model.alpha_energy_mev
                * 1e3
                + track_deposit_kev(&position, &li_axis, rn, model.li_range_um)
                    * model.li_energy_mev
                    * 1e3;
        }
        out.push(cell_z_kev * JOULE_PER_KEV / nucleus_mass_kg);
    }
    Ok(out)
}

/// Uniform random unit vector (isotropic).
fn random_direction(rng: &mut Splitmix64) -> [f64; 3] {
    let cos_theta = 2.0 * rng.f64() - 1.0;
    let phi = 2.0 * PI * rng.f64();
    let s = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    [s * phi.cos(), s * phi.sin(), cos_theta]
}

/// Chord length of the forward ray from `position` along `axis`
/// through a sphere of radius `radius_um` centered at the origin —
/// the same rectilinear convention `lineal_tally` declares. Returns
/// the deposited fraction of the track's full CSDA range (0..1).
fn track_deposit_kev(position: &[f64; 3], axis: &[f64; 3], radius_um: f64, range_um: f64) -> f64 {
    // Solve |p + t·d|² = R²; chord = forward segment inside.
    let b = position[0] * axis[0] + position[1] * axis[1] + position[2] * axis[2];
    let c = position[0] * position[0] + position[1] * position[1] + position[2] * position[2]
        - radius_um * radius_um;
    let disc = b * b - c;
    if disc <= 0.0 {
        return 0.0;
    }
    let sq = disc.sqrt();
    let (t_in, t_out) = (-b - sq, -b + sq);
    let chord = if c <= 0.0 {
        // Inside the sphere: forward exit only.
        t_out.max(0.0)
    } else {
        // Outside: forward segment ∩ [0, ∞).
        (t_out.max(0.0) - t_in.max(0.0)).max(0.0)
    };
    (chord / range_um).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_boron::{BoronMicrodistribution, CompartmentFractions};

    fn model(
        nucleus: f64,
        cytoplasm: f64,
        membrane: f64,
        extracellular: f64,
    ) -> BoronMicrodistribution {
        BoronMicrodistribution {
            schema_version: "openbnct.boron-microdistribution/0.1.0".into(),
            id: "m".into(),
            cell_radius_um: 9.0,
            nucleus_radius_um: 5.0,
            extracellular_extent_um: 15.0,
            compartments: CompartmentFractions {
                nucleus,
                nucleus_1sigma: 0.0,
                cytoplasm,
                cytoplasm_1sigma: 0.0,
                membrane,
                membrane_1sigma: 0.0,
                extracellular,
                extracellular_1sigma: 0.0,
            },
            intercellular_cv: 0.3,
            intercellular_cv_1sigma: 0.0,
            alpha_energy_mev: 1.47,
            alpha_range_um: 8.8,
            li_energy_mev: 0.84,
            li_range_um: 4.9,
            validity_domain: "test".into(),
            provenance_id: "test".into(),
        }
    }

    fn cref() -> ContentReference {
        ContentReference {
            id: "m".into(),
            sha256: "a".repeat(64),
        }
    }

    fn z_edges() -> Vec<f64> {
        (0..=40).map(|i| i as f64 * 0.25).collect()
    }

    fn y_edges() -> Vec<f64> {
        vec![0.0, 10.0, 50.0, 100.0, 200.0, 400.0, 1000.0]
    }

    #[test]
    fn sampling_is_seed_deterministic() {
        let a = sample_cell_microdosimetry(
            &model(0.2, 0.5, 0.1, 0.2),
            cref(),
            2.0,
            400,
            42,
            &z_edges(),
            &y_edges(),
            "s",
            "p",
        )
        .unwrap();
        let b = sample_cell_microdosimetry(
            &model(0.2, 0.5, 0.1, 0.2),
            cref(),
            2.0,
            400,
            42,
            &z_edges(),
            &y_edges(),
            "s",
            "p",
        )
        .unwrap();
        assert_eq!(a, b);
        let c = sample_cell_microdosimetry(
            &model(0.2, 0.5, 0.1, 0.2),
            cref(),
            2.0,
            400,
            43,
            &z_edges(),
            &y_edges(),
            "s",
            "p",
        )
        .unwrap();
        assert_ne!(
            a.statistics.captures_simulated,
            c.statistics.captures_simulated
        );
    }

    #[test]
    fn nuclear_boron_beats_extracellular_boron() {
        // Nucleus-localized ¹⁰B must impart far more nucleus energy
        // than extracellular ¹⁰B at equal capture rate — the
        // microdistribution physics the scalar dose ignores.
        let nuclear = sample_cell_microdosimetry(
            &model(1.0, 0.0, 0.0, 0.0),
            cref(),
            3.0,
            400,
            7,
            &z_edges(),
            &y_edges(),
            "n",
            "p",
        )
        .unwrap();
        let external = sample_cell_microdosimetry(
            &model(0.0, 0.0, 0.0, 1.0),
            cref(),
            3.0,
            400,
            7,
            &z_edges(),
            &y_edges(),
            "e",
            "p",
        )
        .unwrap();
        assert!(nuclear.mean_specific_energy_gy > 5.0 * external.mean_specific_energy_gy);
        // Uptake heterogeneity still leaves some zero-capture cells —
        // but far fewer than when ¹⁰B sits outside the cell entirely.
        assert!(external.untouched_fraction > 2.0 * nuclear.untouched_fraction);
    }

    #[test]
    fn zero_captures_leaves_every_cell_untouched() {
        let a = sample_cell_microdosimetry(
            &model(0.5, 0.5, 0.0, 0.0),
            cref(),
            0.0,
            200,
            1,
            &z_edges(),
            &y_edges(),
            "z",
            "p",
        )
        .unwrap();
        assert_eq!(a.untouched_fraction, 1.0);
        assert_eq!(a.mean_specific_energy_gy, 0.0);
        assert!(a.nucleus_lineal_spectrum.values.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn smk_population_lies_above_mk_mean_field() {
        // Jensen: ⟨e^{−f(z)}⟩ ≥ e^{−f(z̄)} for convex f — the
        // untouched/heterogeneous population must survive better than
        // the MK mean-field predicts at matched mean z.
        let model = model(0.3, 0.5, 0.1, 0.1);
        let artifact = sample_cell_microdosimetry(
            &model,
            cref(),
            3.0,
            800,
            11,
            &z_edges(),
            &y_edges(),
            "s",
            "p",
        )
        .unwrap();
        let evaluation = evaluate_smk(
            &artifact,
            ContentReference {
                id: "s".into(),
                sha256: "b".repeat(64),
            },
            &model,
            SmkParameters {
                alpha_per_gy: 0.5,
                beta_per_gy2: 0.05,
                reference_alpha_per_gy: 0.2,
                reference_beta_per_gy2: 0.02,
                boron_dose_gy_at_mean_captures: artifact.mean_specific_energy_gy,
                dose_levels_gy: vec![artifact.mean_specific_energy_gy],
            },
            "e",
            "p",
        )
        .unwrap();
        let point = &evaluation.points[0];
        assert!(point.smk_survival >= point.mk_survival - 1e-12);
        assert!(point.rbe.unwrap() > 0.0);
    }
}
