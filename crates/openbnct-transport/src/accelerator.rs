// SPDX-License-Identifier: Apache-2.0

//! Parametric accelerator-target neutron sources
//! (`openbnct.accelerator-source/0.1.0`).
//!
//! A parametric thick-target model of the ⁷Li(p,n)⁷Be reaction — the
//! dominant accelerator-based BNCT source reaction — producing a
//! forward-emitted neutron spectrum and yield from declared proton
//! energy, current, and target thickness. The model integrates the
//! recommended 0° differential production cross sections of Liskien &
//! Paulsen (1975, ADNDT 15, 57) over the proton slowing-down path in
//! lithium metal using Bethe-Bloch stopping with the ICRU mean excitation
//! energy for Li, mapping each contributing proton energy through exact
//! non-relativistic two-body kinematics to its 0° neutron energy.
//!
//! Scope and honesty boundaries (carried in every emitted record):
//! - Ground-state branch (n₀) only. The ⁷Li(p,n₁)⁷Be* group opens at
//!   2.371 MeV and is not modeled — above that proton energy the real
//!   spectrum gains a lower-energy group this record does not contain.
//! - Below 1.95 MeV the recommended table ends; the model applies an
//!   s-wave √(E − E_th) ramp anchored to the 1.95 MeV value. The
//!   threshold-to-1.95 MeV interval contributes little integrated yield
//!   at design energies but is exact to flag.
//! - Emission is assumed uniform in solid angle within the declared
//!   cone — the real distribution is forward-peaked; the declared cone
//!   half-angle is an operator choice, not a model prediction.
//! - Stopping is Bethe-Bloch without shell corrections (I_Li = 40 eV);
//!   near the Bragg region of 2–3 MeV protons this carries ~10% scale
//!   uncertainty on yield, which the record states.
//!
//! This is a parametric screening estimate for beam-design studies. It
//! produces candidate `BeamDescription` documents — which carry a
//! `computed_model` provenance binding back to this artifact — that then
//! flow through the same `beam characterize`/`beam bind` and transport
//! paths as measured beams. It is not a substitute for a measured beam
//! characterization and asserts no clinical or commissioning claim.

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::beam::{BeamDescription, BeamProvenance, Citation, PortGeometry};
use crate::model::{
    AngularDistribution, EnergyDistribution, FixedSourceDefinition, ParticleType, PlaneAxis,
    SourceSpatialDistribution,
};

pub const ACCELERATOR_SOURCE_SCHEMA: &str = "openbnct.accelerator-source/0.1.0";

// ---------------------------------------------------------------------------
// Embedded nuclear data — provenance stated on each table.
// ---------------------------------------------------------------------------

/// ⁷Li(p,n)⁷Be reaction Q value (MeV) from AME mass defects
/// (⁷Li 14.908, ⁷Be 15.769, n 8.071, ¹H 7.289 MeV):
/// Q = m_Li + m_p − m_n − m_Be = −1.6442 MeV.
const LI7_PN_Q_MEV: f64 = -1.6442;

/// Lab threshold proton energy: −Q·(m_Be + m_n)/(m_Be + m_n − m_p)
/// with AME-2020 masses — 1.8806 MeV (literature value 1.8811).
const LI7_PN_THRESHOLD_MEV: f64 = 1.8806;

/// Recommended 0° laboratory differential production cross section of
/// ⁷Li(p,n)⁷Be(g.s.) in barns/sr: `(proton energy eV, dσ/dΩ)` from
/// Liskien & Paulsen, At. Data Nucl. Data Tables 15 (1975) 57 — the
/// evaluation underlying essentially every AB-BNCT Li-target design
/// study. Fetched from EXFOR entry F0004, recommended series.
const LI7_PN_DSIGMA0_B_SR: &[(f64, f64)] = &[
    (1.95e6, 0.0190),
    (2.00e6, 0.0150),
    (2.05e6, 0.0121),
    (2.10e6, 0.0131),
    (2.15e6, 0.0226),
    (2.20e6, 0.0467),
    (2.25e6, 0.0792),
    (2.30e6, 0.0834),
    (2.35e6, 0.0714),
    (2.40e6, 0.0612),
    (2.45e6, 0.0530),
    (2.50e6, 0.0474),
    (2.60e6, 0.0405),
    (2.70e6, 0.0360),
    (2.80e6, 0.0342),
    (2.90e6, 0.0330),
    (3.00e6, 0.0320),
    (3.10e6, 0.0312),
    (3.20e6, 0.0305),
    (3.30e6, 0.0299),
    (3.40e6, 0.0293),
    (3.50e6, 0.0287),
    (3.60e6, 0.0282),
    (3.70e6, 0.0278),
    (3.80e6, 0.0274),
    (3.90e6, 0.0270),
    (4.00e6, 0.0267),
    (4.10e6, 0.0271),
    (4.20e6, 0.0282),
    (4.30e6, 0.0300),
    (4.40e6, 0.0322),
    (4.50e6, 0.0346),
    (4.60e6, 0.0375),
    (4.70e6, 0.0411),
    (4.80e6, 0.0448),
    (4.90e6, 0.0481),
    (5.00e6, 0.0500),
    (5.10e6, 0.0486),
    (5.20e6, 0.0456),
    (5.30e6, 0.0424),
    (5.40e6, 0.0396),
    (5.50e6, 0.0366),
    (5.60e6, 0.0339),
    (5.70e6, 0.0316),
    (5.80e6, 0.0290),
    (5.90e6, 0.0267),
    (6.00e6, 0.0244),
    (6.10e6, 0.0226),
    (6.20e6, 0.0210),
    (6.30e6, 0.0195),
    (6.40e6, 0.0182),
    (6.50e6, 0.0169),
    (6.60e6, 0.0156),
    (6.70e6, 0.0146),
    (6.80e6, 0.0137),
    (6.90e6, 0.0129),
    (7.00e6, 0.0122),
];

/// Number density of lithium metal: ρ = 0.534 g cm⁻³, A = 6.94 g mol⁻¹
/// (natural Li — the model assumes the reaction runs on ⁷Li with the
/// natural-abundance (92.5%) correction applied to the cross-section
/// table's isotopic basis; the table is per ⁷Li atom, so the number
/// density here counts ⁷Li atoms only).
const LI_DENSITY_G_CM3: f64 = 0.534;
const LI7_ISOTOPIC_FRACTION: f64 = 0.9241; // natural Li-7 abundance
const LI_MOLAR_MASS_G_MOL: f64 = 6.94;
const AVOGADRO: f64 = 6.02214076e23;

/// ICRU mean excitation energy for lithium, eV (ICRU Report 37/49).
const LI_MEAN_EXCITATION_EV: f64 = 40.0;
const ELECTRON_MASS_KEV: f64 = 511.0;
const PROTON_MASS_KEV: f64 = 938_272.0;
/// Bethe stopping constant 4πN_A r_e² m_e c² = 0.307075 MeV cm² mol⁻¹.
const BETHE_K: f64 = 0.307075;

fn li7_atoms_per_cm3() -> f64 {
    LI_DENSITY_G_CM3 / LI_MOLAR_MASS_G_MOL * AVOGADRO * LI7_ISOTOPIC_FRACTION
}

/// Mass stopping power of protons in lithium, MeV cm² g⁻¹, via the
/// Bethe formula without shell corrections. Valid to ~10% over the
/// 1.9–3 MeV interval of interest; flagged in the derivation note.
fn bethe_stopping_li_mev_cm2_g(proton_energy_mev: f64) -> f64 {
    let e_kev = proton_energy_mev * 1000.0;
    let beta2 = 2.0 * e_kev / PROTON_MASS_KEV;
    let gamma = 1.0 + e_kev / PROTON_MASS_KEV;
    // T_max for a proton on a free electron.
    let t_max_kev = 2.0 * ELECTRON_MASS_KEV * beta2
        / (1.0
            + 2.0 * gamma * ELECTRON_MASS_KEV / PROTON_MASS_KEV
            + (ELECTRON_MASS_KEV / PROTON_MASS_KEV).powi(2));
    let i_kev = LI_MEAN_EXCITATION_EV / 1000.0;
    let argument = 2.0 * ELECTRON_MASS_KEV * beta2 * t_max_kev / (i_kev * i_kev);
    let bracket = 0.5 * argument.ln() - beta2;
    if bracket <= 0.0 {
        // Below the formal Bethe validity floor — clamp rather than emit
        // a negative stopping power; the threshold ramp region is the
        // only place this can engage and it stays conservative.
        return BETHE_K * (3.0 / LI_MOLAR_MASS_G_MOL) / beta2 * 0.01;
    }
    BETHE_K * (3.0 / LI_MOLAR_MASS_G_MOL) / beta2 * bracket
}

/// Interpolated recommended 0° differential cross section, b/sr.
/// Above 1.95 MeV: log-linear interpolation of the L&P table. From
/// threshold to 1.95 MeV: s-wave √ΔE ramp anchored at the table's first
/// point (documented approximation — the region is yield-light at
/// design energies but must not return zero).
fn dsigma0_li7_pn_b_sr(proton_energy_mev: f64) -> f64 {
    if proton_energy_mev <= LI7_PN_THRESHOLD_MEV {
        return 0.0;
    }
    let first = LI7_PN_DSIGMA0_B_SR[0];
    if proton_energy_mev < first.0 / 1.0e6 {
        // s-wave near-threshold ramp: σ ∝ √(E − E_th).
        return first.1
            * ((proton_energy_mev - LI7_PN_THRESHOLD_MEV)
                / (first.0 / 1.0e6 - LI7_PN_THRESHOLD_MEV))
                .sqrt();
    }
    let e_ev = proton_energy_mev * 1.0e6;
    for pair in LI7_PN_DSIGMA0_B_SR.windows(2) {
        let (e0, s0) = pair[0];
        let (e1, s1) = pair[1];
        if e_ev >= e0 && e_ev <= e1 {
            // Log-linear in σ for the steep near-threshold structure.
            let t = (e_ev - e0) / (e1 - e0);
            return (s0.ln() + t * (s1.ln() - s0.ln())).exp();
        }
    }
    *LI7_PN_DSIGMA0_B_SR.last().map(|(_, s)| s).unwrap_or(&0.0)
}

/// Exact non-relativistic two-body 0° neutron energy for ⁷Li(p,n)⁷Be,
/// in MeV, as a function of lab proton energy (MeV). Returns `None`
/// below threshold.
///
/// In the CM frame the available kinetic energy
/// `E* = E_p·m_Li/(m_Li+m_p) + Q` splits between neutron and recoil in
/// mass proportion, and the neutron's lab energy at 0° adds its CM
/// translation energy in quadrature of velocities:
/// `E_n(0) = (√E_cm + √E*_n)²` with
/// `E_cm = m_n m_p E_p/(m_p+m_Li)²` and `E*_n = E*·m_Be/(m_n+m_Be)`.
/// At threshold E* → 0 and E_n(0) → m_n m_p E_th/(m_p+m_Li)² ≈ 29.7 keV —
/// the known monoenergetic forward-cone floor of the thick-target
/// spectrum.
fn neutron_energy_zero_deg_mev(proton_energy_mev: f64) -> Option<f64> {
    if proton_energy_mev <= LI7_PN_THRESHOLD_MEV {
        return None;
    }
    // Masses in atomic-mass units (ratios only — units cancel).
    let m_p = 1.007825;
    let m_n = 1.008665;
    let m_li = 7.016004;
    let m_be = 7.016929;
    let e_cm = m_n * m_p * proton_energy_mev / (m_p + m_li).powi(2);
    let e_star = (proton_energy_mev * m_li / (m_li + m_p) + LI7_PN_Q_MEV).max(0.0);
    let e_star_n = e_star * m_be / (m_n + m_be);
    Some((e_cm.sqrt() + e_star_n.sqrt()).powi(2))
}

/// The declared accelerator-target source specification — the inputs an
/// operator declares; everything else in the artifact is derived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceleratorSourceSpec {
    /// Proton beam energy incident on the target, MeV.
    pub proton_energy_mev: f64,
    /// Proton beam current on target, mA.
    pub proton_current_ma: f64,
    /// Lithium target thickness in µm. `None` (or any value at least the
    /// slowing-down distance to threshold) means thick-target: protons
    /// stop below threshold inside the Li. A thinner value gives a
    /// partially-thick target — supported deliberately (Lee & Zhou's
    /// partial-thickness case): reactions stop at the back-face proton
    /// energy, not at threshold.
    pub target_thickness_um: Option<f64>,
    /// Beam axis: the world axis the source plane is perpendicular to.
    pub axis: PlaneAxis,
    /// World coordinate of the source plane along `axis`, cm.
    pub plane_offset_cm: f64,
    /// Propagation direction sign along `axis` (+1 or −1).
    pub direction_sign: i8,
    /// Port radius, cm — the neutron-emitting disk and the declared
    /// beam port.
    pub port_radius_cm: f64,
    /// Port center in the plane's in-plane world coordinates, cm.
    pub port_center_uv_cm: [f64; 2],
    /// Emission cone half-angle in degrees about the forward direction.
    /// The model assumes uniform emission in solid angle inside the
    /// cone — an operator choice, flagged in the record.
    pub half_angle_deg: f64,
    /// Energy bin count for the emitted spectrum histogram.
    pub spectrum_bins: u32,
}

/// What the model computed — the derived content of the artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceleratorDerived {
    /// Lithium thickness required to slow the declared proton energy to
    /// reaction threshold, µm — the thick-target depth.
    pub thick_target_depth_um: f64,
    /// Whether the declared thickness fully stops protons below
    /// threshold (thick) or transmits them (partially thick).
    pub thick_target: bool,
    /// Proton energy at the back face, MeV; ~0 for a thick target.
    pub proton_exit_energy_mev: f64,
    /// Forward thick-target differential yield at 0°: neutrons per
    /// proton per steradian.
    pub forward_yield_per_proton_sr: f64,
    /// Neutron yield inside the declared emission cone per second at the
    /// declared current (uniform-in-cone assumption).
    pub cone_yield_per_s: f64,
    /// Neutron energy range of the emitted spectrum, eV
    /// `[E_n(0°, back-face E_p), E_n(0°, incident E_p)]`.
    pub neutron_energy_range_ev: [f64; 2],
    /// The emitted forward spectrum as a histogram (probability mass
    /// per bin — normalized shape, before cone/current scaling).
    pub spectrum: HistogramSpectrum,
}

/// A piecewise-uniform energy histogram: `n+1` edges, `n` masses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistogramSpectrum {
    pub energy_boundaries_ev: Vec<f64>,
    pub bin_weights: Vec<f64>,
}

/// The emitted record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceleratorSource {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    /// Stable document identifier, e.g. `openbnct.accelerator-source.x.v1`.
    pub id: String,
    /// Only `li7_pn` is implemented; the enum is declared so a
    /// `be9_pn` branch can land without a schema change.
    pub reaction: AcceleratorReaction,
    pub spec: AcceleratorSourceSpec,
    pub derived: AcceleratorDerived,
    /// Data provenance: which evaluations and approximations produced
    /// every number.
    pub model_provenance: AcceleratorProvenance,
    /// Research-status qualification; no clinical or commissioning claim.
    pub qualification: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceleratorReaction {
    /// ⁷Li(p,n)⁷Be — the AB-BNCT standard source reaction.
    Li7Pn,
}

/// Where the model's data and approximations come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceleratorProvenance {
    pub citations: Vec<Citation>,
    /// Exact statement of what is measured-evaluated data, what is
    /// computed, and every modeling approximation applied.
    pub derivation_note: String,
}

impl AcceleratorSource {
    pub fn validate(&self) -> Result<(), AcceleratorError> {
        if !openbnct_core::schema_matches(&self.schema_version, ACCELERATOR_SOURCE_SCHEMA) {
            return Err(AcceleratorError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(AcceleratorError::EmptyIdentifier);
        }
        validate_spec(&self.spec)?;
        if self.derived.spectrum.energy_boundaries_ev.len()
            != self.derived.spectrum.bin_weights.len() + 1
        {
            return Err(AcceleratorError::MalformedSpectrum);
        }
        if self.model_provenance.citations.is_empty()
            || self.model_provenance.derivation_note.trim().is_empty()
        {
            return Err(AcceleratorError::InvalidProvenance);
        }
        Ok(())
    }

    /// Emit a complete `BeamDescription` at this source's declared port,
    /// with `computed_model` provenance binding back to this artifact's
    /// content identity. The beam is immediately usable by
    /// `beam characterize` (TECDOC in-air metrics) and `beam bind`.
    pub fn to_beam_description(
        &self,
        beam_id: &str,
        name: &str,
        facility: &str,
        source_content: ContentReference,
    ) -> Result<BeamDescription, AcceleratorError> {
        self.validate()?;
        let spec = &self.spec;
        let normal = match spec.direction_sign {
            1 => 1.0,
            -1 => -1.0,
            _ => return Err(AcceleratorError::InvalidDirectionSign),
        };
        let mut axis_vector = [0.0; 3];
        axis_vector[spec.axis.index()] = normal;
        let half_angle_rad = spec.half_angle_deg.to_radians();
        if !(half_angle_rad > 0.0 && half_angle_rad <= std::f64::consts::FRAC_PI_2) {
            return Err(AcceleratorError::InvalidHalfAngle);
        }
        let port_area_cm2 = std::f64::consts::PI * spec.port_radius_cm * spec.port_radius_cm;
        // Neutron rate through the port: cone yield over port area —
        // the disk emits at the port plane itself.
        let fluence_rate = self.derived.cone_yield_per_s / port_area_cm2;
        let beam = BeamDescription {
            schema_version: crate::beam::BEAM_DESCRIPTION_SCHEMA.into(),
            id: beam_id.into(),
            name: name.into(),
            facility: facility.into(),
            port: PortGeometry {
                axis: spec.axis,
                offset_cm: spec.plane_offset_cm,
                shape: crate::beam::PortShape::Circle {
                    center_uv_cm: spec.port_center_uv_cm,
                    radius_cm: spec.port_radius_cm,
                },
            },
            source: FixedSourceDefinition {
                schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
                id: self.id.clone(),
                particle: ParticleType::Neutron,
                source_sites_per_history: 1,
                statistical_weight_per_site: 1.0,
                space: SourceSpatialDistribution::UniformDisk {
                    axis: spec.axis,
                    offset_cm: spec.plane_offset_cm,
                    center_uv_cm: spec.port_center_uv_cm,
                    radius_cm: spec.port_radius_cm,
                },
                angle: AngularDistribution::IsotropicCone {
                    axis_unit_vector: axis_vector,
                    half_angle_rad,
                },
                energy: EnergyDistribution::TabulatedHistogram {
                    energy_boundaries_ev: self.derived.spectrum.energy_boundaries_ev.clone(),
                    bin_weights: self.derived.spectrum.bin_weights.clone(),
                },
            },
            normalization: crate::beam::NormalizationBasis::FluenceRateAtPort {
                fluence_rate_cm2_s: fluence_rate,
            },
            provenance: BeamProvenance::ComputedModel {
                generator: source_content,
                derivation_note: format!(
                    "parametric 7Li(p,n)7Be thick-target forward source: \
                     Ep={} MeV, I={} mA, {}; uniform-in-cone emission assumed",
                    spec.proton_energy_mev,
                    spec.proton_current_ma,
                    if self.derived.thick_target {
                        format!(
                            "thick target ({} µm required)",
                            self.derived.thick_target_depth_um
                        )
                    } else {
                        format!(
                            "partially thick, Ep exit = {} MeV",
                            self.derived.proton_exit_energy_mev
                        )
                    }
                ),
            },
        };
        beam.validate().map_err(AcceleratorError::Beam)?;
        Ok(beam)
    }
}

fn validate_spec(spec: &AcceleratorSourceSpec) -> Result<(), AcceleratorError> {
    if !(spec.proton_energy_mev.is_finite()
        && spec.proton_energy_mev > LI7_PN_THRESHOLD_MEV
        && spec.proton_energy_mev <= 7.0)
    {
        return Err(AcceleratorError::ProtonEnergyOutOfRange {
            energy_mev: spec.proton_energy_mev,
        });
    }
    if !(spec.proton_current_ma.is_finite() && spec.proton_current_ma > 0.0) {
        return Err(AcceleratorError::InvalidCurrent);
    }
    if let Some(t) = spec.target_thickness_um
        && !(t.is_finite() && t > 0.0)
    {
        return Err(AcceleratorError::InvalidThickness);
    }
    if spec.direction_sign != 1 && spec.direction_sign != -1 {
        return Err(AcceleratorError::InvalidDirectionSign);
    }
    if !(spec.port_radius_cm.is_finite() && spec.port_radius_cm > 0.0) {
        return Err(AcceleratorError::InvalidPort);
    }
    if spec.port_center_uv_cm.iter().any(|v| !v.is_finite()) || !spec.plane_offset_cm.is_finite() {
        return Err(AcceleratorError::InvalidPort);
    }
    if !(spec.half_angle_deg.is_finite()
        && spec.half_angle_deg > 0.0
        && spec.half_angle_deg <= 90.0)
    {
        return Err(AcceleratorError::InvalidHalfAngle);
    }
    if !(8..=4096).contains(&spec.spectrum_bins) {
        return Err(AcceleratorError::InvalidSpectrumBins);
    }
    Ok(())
}

/// Evaluate a declared spec into a full `AcceleratorSource` artifact.
///
/// The forward model: at each proton energy E_p between the back-face
/// energy and the incident energy, the 0° production rate per unit path
/// is `n_Li · dσ/dΩ(0°, E_p) / (ρ·S(E_p))`; the produced neutron lands
/// at the two-body 0° energy E_n(E_p). Integrating E_p down the
/// slowing path gives the thick-target forward spectrum — the classic
/// "step" spectrum of Li(p,n) design studies.
pub fn evaluate_accelerator_source(
    id: &str,
    spec: AcceleratorSourceSpec,
) -> Result<AcceleratorSource, AcceleratorError> {
    validate_spec(&spec)?;
    // Back-face proton energy: walk the proton down the declared (or
    // thick-target) depth in fine energy steps.
    let n_li = li7_atoms_per_cm3();
    let d_e = 0.0005; // MeV per integration step
    let mut thick_depth_um = 0.0;
    // First pass: thickness needed to reach threshold from E_incident.
    {
        let mut e = spec.proton_energy_mev;
        while e > LI7_PN_THRESHOLD_MEV + d_e {
            let s = bethe_stopping_li_mev_cm2_g(e - d_e / 2.0); // MeV cm²/g
            let dx_cm = d_e / (s * LI_DENSITY_G_CM3);
            thick_depth_um += dx_cm * 1.0e4;
            e -= d_e;
        }
    }
    let declared_um = spec.target_thickness_um.unwrap_or(f64::INFINITY);
    let thick = declared_um >= thick_depth_um;
    // Back-face energy under the declared thickness (thick ⇒ ~0, i.e.
    // below threshold; partial ⇒ the energy remaining at t_declared).
    let back_face_mev = if thick {
        LI7_PN_THRESHOLD_MEV
    } else {
        let mut e = spec.proton_energy_mev;
        let mut path_um = 0.0;
        while e > LI7_PN_THRESHOLD_MEV + d_e && path_um < declared_um {
            let s = bethe_stopping_li_mev_cm2_g(e - d_e / 2.0);
            path_um += d_e / (s * LI_DENSITY_G_CM3) * 1.0e4;
            e -= d_e;
        }
        e
    };

    // Second pass: accumulate the forward spectrum. Each proton-energy
    // step contributes weight n_Li·dσ/dΩ·dx into the E_n(E_p) bin.
    let en_max = neutron_energy_zero_deg_mev(spec.proton_energy_mev)
        .ok_or(AcceleratorError::BelowThreshold)?;
    // Thick targets bottom out at the ~29.7 keV threshold cone energy,
    // not zero — evaluate one step above the back-face energy.
    let en_min = neutron_energy_zero_deg_mev(back_face_mev + d_e).unwrap_or(0.0);
    // Guard a degenerate near-threshold span (both ends ~0).
    let span_ev = (en_max - en_min).max(1.0e-6) * 1.0e6;
    let bins = spec.spectrum_bins as usize;
    let mut weights = vec![0.0_f64; bins];
    let mut e = spec.proton_energy_mev;
    let mut forward_yield = 0.0_f64; // n / proton / sr at 0°
    while e > back_face_mev + d_e {
        let mid = e - d_e / 2.0;
        let sigma = dsigma0_li7_pn_b_sr(mid) * 1.0e-24; // b/sr → cm²/sr
        let s = bethe_stopping_li_mev_cm2_g(mid);
        let dx_cm = d_e / (s * LI_DENSITY_G_CM3);
        let dw = n_li * sigma * dx_cm; // n/p/sr in this energy step
        forward_yield += dw;
        if let Some(en_mev) = neutron_energy_zero_deg_mev(mid) {
            let frac = ((en_mev * 1.0e6 - en_min * 1.0e6) / span_ev).clamp(0.0, 1.0);
            let bin = ((frac * bins as f64) as usize).min(bins - 1);
            weights[bin] += dw;
        }
        e -= d_e;
    }

    let solid_angle = 2.0 * std::f64::consts::PI * (1.0 - spec.half_angle_deg.to_radians().cos());
    let protons_per_s = spec.proton_current_ma * 1.0e-3 / 1.602176634e-19;
    let cone_yield_per_s = forward_yield * solid_angle * protons_per_s;

    let total: f64 = weights.iter().sum();
    if !total.is_finite() || total <= 0.0 {
        return Err(AcceleratorError::ZeroYield);
    }
    let normalized: Vec<f64> = weights.iter().map(|w| w / total).collect();
    let boundaries: Vec<f64> = (0..=bins)
        .map(|i| (en_min * 1.0e6 + span_ev * i as f64 / bins as f64).max(1.0e-5))
        .collect();

    Ok(AcceleratorSource {
        schema_version: ACCELERATOR_SOURCE_SCHEMA.into(),
        id: id.into(),
        reaction: AcceleratorReaction::Li7Pn,
        spec,
        derived: AcceleratorDerived {
            thick_target_depth_um: thick_depth_um,
            thick_target: thick,
            proton_exit_energy_mev: if thick { 0.0 } else { back_face_mev },
            forward_yield_per_proton_sr: forward_yield,
            cone_yield_per_s,
            neutron_energy_range_ev: [en_min * 1.0e6, en_max * 1.0e6],
            spectrum: HistogramSpectrum {
                energy_boundaries_ev: boundaries,
                bin_weights: normalized,
            },
        },
        model_provenance: AcceleratorProvenance {
            citations: vec![
                Citation {
                    authors: "Liskien, H.; Paulsen, A.".into(),
                    title: "Neutron production cross sections and energies for the \
                            reactions 7Li(p,n)7Be and 7Li(p,n)7Be*"
                        .into(),
                    venue: "Atomic Data and Nuclear Data Tables 15, 57".into(),
                    year: 1975,
                    doi: Some("10.1016/0092-640X(75)90004-2".into()),
                    url: None,
                },
                Citation {
                    authors: "Lee, C. L.; Zhou, X.-L.".into(),
                    title: "Thick target neutron yields for the 7Li(p,n)7Be reaction \
                            near threshold"
                        .into(),
                    venue: "Nuclear Instruments and Methods B 152, 1".into(),
                    year: 1999,
                    doi: Some("10.1016/S0168-583X(99)00026-9".into()),
                    url: None,
                },
            ],
            derivation_note: "forward spectrum integrated over the proton slowing path \
                in Li metal using Liskien–Paulsen recommended 0° differential cross \
                sections (ground-state branch only — the n1 group above 2.371 MeV is \
                not modeled); below 1.95 MeV an s-wave sqrt(E−Eth) ramp anchors the \
                table; Bethe–Bloch stopping with ICRU I=40 eV for Li (no shell \
                corrections, ~10% yield scale); uniform emission inside the declared \
                cone is an operator assumption — the real distribution is \
                forward-peaked. Parametric screening estimate; not a measured \
                characterization."
                .into(),
        },
        qualification: "parametric accelerator-target source estimate for research \
            beam-design screening — not a measured beam characterization; no \
            clinical, equivalence, or commissioning claim"
            .into(),
    })
}

#[derive(Debug, Error)]
pub enum AcceleratorError {
    #[error(transparent)]
    Beam(#[from] crate::beam::BeamError),
    #[error("accelerator-source id is empty")]
    EmptyIdentifier,
    #[error("unsupported accelerator-source schema {0:?}; expected {ACCELERATOR_SOURCE_SCHEMA:?}")]
    UnsupportedSchema(String),
    #[error(
        "proton energy {energy_mev} MeV is outside the modeled range \
         (threshold {LI7_PN_THRESHOLD_MEV} MeV to the 7 MeV table limit)"
    )]
    ProtonEnergyOutOfRange { energy_mev: f64 },
    #[error("proton current must be finite and positive mA")]
    InvalidCurrent,
    #[error("target thickness must be finite and positive µm when declared")]
    InvalidThickness,
    #[error("direction sign must be +1 or −1")]
    InvalidDirectionSign,
    #[error("port geometry must be finite with positive radius")]
    InvalidPort,
    #[error("emission half-angle must be in (0, 90] degrees")]
    InvalidHalfAngle,
    #[error("spectrum bin count must be in [8, 4096]")]
    InvalidSpectrumBins,
    #[error("incident proton energy is below the reaction threshold")]
    BelowThreshold,
    #[error("computed forward yield is zero — the spec produces no source")]
    ZeroYield,
    #[error("derived spectrum is malformed")]
    MalformedSpectrum,
    #[error("model provenance requires citations and a derivation note")]
    InvalidProvenance,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AcceleratorSourceSpec {
        AcceleratorSourceSpec {
            proton_energy_mev: 2.5,
            proton_current_ma: 1.0,
            target_thickness_um: None,
            axis: PlaneAxis::Z,
            plane_offset_cm: 0.0,
            direction_sign: 1,
            port_radius_cm: 6.0,
            port_center_uv_cm: [0.0, 0.0],
            half_angle_deg: 30.0,
            spectrum_bins: 64,
        }
    }

    #[test]
    fn neutron_energy_zero_at_threshold_and_monotone() {
        assert_eq!(neutron_energy_zero_deg_mev(LI7_PN_THRESHOLD_MEV), None);
        let e1 = neutron_energy_zero_deg_mev(2.0).unwrap();
        let e2 = neutron_energy_zero_deg_mev(2.5).unwrap();
        let e3 = neutron_energy_zero_deg_mev(3.0).unwrap();
        assert!(e1 < e2 && e2 < e3);
        // Known value: Ep=2.5 MeV → E_n(0°) ≈ 0.79 MeV; and the
        // threshold floor is the monoenergetic ~30 keV cone.
        assert!((e2 - 0.79).abs() < 0.05, "En(0°,2.5) = {e2}");
        let floor = neutron_energy_zero_deg_mev(LI7_PN_THRESHOLD_MEV + 1.0e-6).unwrap();
        assert!((floor - 0.0297).abs() < 0.005, "En(0°,th) = {floor}");
    }

    #[test]
    fn dsigma_table_and_threshold_ramp() {
        // At a tabulated point the table value returns.
        let s = dsigma0_li7_pn_b_sr(2.25);
        assert!((s - 0.0792).abs() < 1.0e-6, "σ(2.25) = {s}");
        // Below the table start the ramp is positive and rising.
        let lo = dsigma0_li7_pn_b_sr(1.90);
        assert!(lo > 0.0 && lo < 0.0190);
        // Below threshold: zero.
        assert_eq!(dsigma0_li7_pn_b_sr(1.5), 0.0);
    }

    #[test]
    fn thick_target_evaluates_and_normalizes() {
        let source = evaluate_accelerator_source("acc.test.v1", spec()).unwrap();
        source.validate().unwrap();
        assert!(source.derived.thick_target);
        assert!(source.derived.thick_target_depth_um > 0.0);
        assert!(source.derived.forward_yield_per_proton_sr > 0.0);
        let total: f64 = source.derived.spectrum.bin_weights.iter().sum();
        assert!((total - 1.0).abs() < 1.0e-9);
        // The spectrum ends at the kinematic maximum.
        let last_edge = *source.derived.spectrum.energy_boundaries_ev.last().unwrap();
        assert!((last_edge - 0.79e6).abs() < 0.1e6);
    }

    #[test]
    fn partially_thick_target_cuts_the_spectrum() {
        let mut thin = spec();
        thin.target_thickness_um = Some(1.0); // ~micron — only the surface contributes
        let source = evaluate_accelerator_source("acc.thin.v1", thin).unwrap();
        assert!(!source.derived.thick_target);
        assert!(source.derived.proton_exit_energy_mev > LI7_PN_THRESHOLD_MEV);
        // Exit energy stays near the incident 2.5 MeV for a micron of Li.
        assert!(source.derived.proton_exit_energy_mev > 2.4);
    }

    #[test]
    fn yield_grows_with_proton_energy() {
        let mut harder = spec();
        harder.proton_energy_mev = 3.0;
        let low = evaluate_accelerator_source("a", spec()).unwrap();
        let high = evaluate_accelerator_source("b", harder).unwrap();
        assert!(high.derived.cone_yield_per_s > low.derived.cone_yield_per_s);
    }

    #[test]
    fn emitted_beam_description_validates() {
        let source = evaluate_accelerator_source("acc.beam.v1", spec()).unwrap();
        let beam = source
            .to_beam_description(
                "openbnct.beam.acc-test.v1",
                "accelerator test beam",
                "test facility",
                ContentReference {
                    id: "acc.beam.v1".into(),
                    sha256: "c".repeat(64),
                },
            )
            .unwrap();
        beam.validate().unwrap();
        assert!(matches!(
            beam.provenance,
            BeamProvenance::ComputedModel { .. }
        ));
        // The in-air path works on the generated beam. The raw target
        // spectrum is entirely fast neutrons (~30 keV–0.8 MeV) — the
        // epithermal band appears only after a moderator.
        let metrics = crate::beam_quality::in_air_metrics(&beam).unwrap();
        assert!(metrics.fast_fluence_rate_cm2_s > 0.0);
        assert_eq!(metrics.thermal_fluence_rate_cm2_s, 0.0);
    }

    #[test]
    fn below_threshold_and_bad_specs_reject() {
        let mut bad = spec();
        bad.proton_energy_mev = 1.5;
        assert!(matches!(
            evaluate_accelerator_source("x", bad).unwrap_err(),
            AcceleratorError::ProtonEnergyOutOfRange { .. }
        ));
        let mut bad = spec();
        bad.proton_energy_mev = 8.0; // beyond the table
        assert!(evaluate_accelerator_source("x", bad).is_err());
        let mut bad = spec();
        bad.half_angle_deg = 95.0;
        assert!(evaluate_accelerator_source("x", bad).is_err());
        let mut bad = spec();
        bad.direction_sign = 0;
        assert!(evaluate_accelerator_source("x", bad).is_err());
    }
}
