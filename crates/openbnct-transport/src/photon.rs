// SPDX-License-Identifier: Apache-2.0

//! Coupled photon transport: multigroup photon data carrying the
//! neutron→photon production map, and the two-pass solve that couples a
//! converged neutron flux to a photon S_N sweep.
//!
//! BNCT photon coupling is one-way: neutron captures and inelastic
//! reactions emit prompt gammas, but at these energies photons never
//! produce neutrons. The honest decomposition is therefore sequential —
//! the converged neutron flux drives a volumetric photon source
//! `Q_γ(cell,gγ) = Σ_g φ_n(cell,g)·Σ_p(g→gγ)`, and the photon sweep
//! carries it. Incoherent (Klein–Nishina) scattering is strictly
//! downscatter, so the photon problem needs no upscatter outer
//! iteration; coherent scattering is elastic and stays on the
//! in-group diagonal.
//!
//! This is the deterministic analogue of what coupled neutron–photon
//! Monte Carlo tallies; the response model's
//! `PhotonEnergyTreatment::ExcludedAndTransported` case anticipates
//! exactly this split (photon energy excluded from neutron heating
//! because it is transported separately).

use serde::{Deserialize, Serialize};

use openbnct_core::ContentReference;

use crate::model::TransportCase;
use crate::multigroup::{
    BoundarySource, MultigroupData, MultigroupError, MultigroupFlux, MultigroupMaterial, SnOptions,
    cell_compositions, level_symmetric_quadrature, material_composition_map, solve_sn_problem,
};

/// Versioned contract id for `MultigroupPhotonData`.
pub const MULTIGROUP_PHOTON_DATA_SCHEMA: &str = "openbnct.multigroup-photon-data/0.1.0";

/// The dose-component key the photon kerma response registers under —
/// matches `DoseComponent::Photon` in the component-profile convention.
pub const PHOTON_DOSE_COMPONENT: &str = "photon";

/// Collapsed coupled photon transport data: photon interaction tables
/// plus the neutron→photon production matrix that turns a converged
/// neutron flux into a volumetric photon source.
///
/// `neutron_energy_boundaries_ev` reproduces the group structure the
/// production matrix maps from — it must match the `MultigroupData`
/// the neutron flux was solved against (validated at solve time).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultigroupPhotonData {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Photon group edges, eV, strictly descending (group 0 = highest).
    pub energy_boundaries_ev: Vec<f64>,
    /// The neutron group structure the production matrix maps from,
    /// eV, strictly descending — pinned to the neutron solve's data.
    pub neutron_energy_boundaries_ev: Vec<f64>,
    /// How the data was collapsed — recorded for review, not verified.
    pub collapse_declaration: String,
    /// Component profile the `dose_response` vectors realize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component_profile: Option<ContentReference>,
    pub materials: Vec<PhotonMaterial>,
}

/// Per-material coupled photon tables.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhotonMaterial {
    pub material_id: String,
    /// cm⁻¹ — photoelectric + incoherent + coherent + pair production,
    /// atom-density summed, per photon group `[Gγ]`.
    pub sigma_total_per_cm: Vec<f64>,
    /// cm⁻¹ — row-major `[Gγ × Gγ]` photon scatter transfer matrix.
    /// Incoherent (Klein–Nishina) transfer is strictly downscatter
    /// (entries land in lower-energy groups); coherent scattering is
    /// elastic and contributes only to the in-group diagonal.
    pub scatter_matrix_per_cm: Vec<f64>,
    /// Optional P1 (l = 1 Legendre) transfer moments `[Gγ × Gγ]`,
    /// cm⁻¹ — per-pair transfer weighted by its mean scatter cosine
    /// (Klein–Nishina per outgoing group; coherent maximally forward;
    /// annihilation isotropic). Feeds the anisotropic scattering
    /// source when the solve enables `p1_anisotropic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scatter_p1_matrix_per_cm: Option<Vec<f64>>,
    /// Scatter-weighted mean lab-frame cosine per photon group `[Gγ]`
    /// — Klein–Nishina weighted; coherent contributes its forward bias.
    /// In [−1, 1]; used by the transport correction.
    pub transport_mu_bar: Vec<f64>,
    /// cm⁻¹ — row-major `[G_n × Gγ]` neutron→photon production matrix:
    /// photon production rate in group gγ per unit neutron scalar flux
    /// in group g_n (σ_reaction·yield·spectrum-fraction, atom-density
    /// summed over the material's nuclides and photon-producing
    /// reactions).
    pub production_matrix_per_cm: Vec<f64>,
    /// cm⁻¹ — row-major `[Gγ × Gγ]` photon→photon secondary-production
    /// matrix: photons created per unit photon flux (pair-production
    /// annihilation secondaries, `2·σ_pair` into the 511 keV group).
    /// Kept separate from the scatter matrix — photon number is not
    /// conserved, so the neutron row-sum ≤ σ_t invariant does not
    /// apply to it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pair_production_matrix_per_cm: Vec<f64>,
    /// Gy·cm² — photon energy-deposition (kerma) response per unit
    /// photon fluence, per group `[Gγ]`: photoelectric and pair
    /// production deposit the incident energy locally; incoherent
    /// scatters deposit `E − E′` (the recoil energy); coherent
    /// deposits ~0. Folded over the photon flux by the dose path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dose_response_gy_cm2: Vec<f64>,
}

impl MultigroupPhotonData {
    pub fn validate(&self) -> Result<(), MultigroupError> {
        let invalid = |m: String| MultigroupError::InvalidData(m);
        if !openbnct_core::schema_matches(&self.schema_version, MULTIGROUP_PHOTON_DATA_SCHEMA) {
            return Err(invalid(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() {
            return Err(invalid("id must be nonempty".into()));
        }
        let g_gamma = self.photon_group_count();
        let g_n = self.neutron_group_count();
        if g_gamma == 0
            || !self
                .energy_boundaries_ev
                .iter()
                .all(|e| e.is_finite() && *e > 0.0)
            || !self.energy_boundaries_ev.windows(2).all(|w| w[0] > w[1])
        {
            return Err(invalid(
                "energy_boundaries_ev must be >1 strictly descending positive edges".into(),
            ));
        }
        if g_n == 0
            || !self
                .neutron_energy_boundaries_ev
                .iter()
                .all(|e| e.is_finite() && *e > 0.0)
            || !self
                .neutron_energy_boundaries_ev
                .windows(2)
                .all(|w| w[0] > w[1])
        {
            return Err(invalid(
                "neutron_energy_boundaries_ev must be >1 strictly descending positive edges".into(),
            ));
        }
        if self.materials.is_empty() {
            return Err(invalid("at least one material is required".into()));
        }
        for material in &self.materials {
            let m = material.material_id.as_str();
            if material.sigma_total_per_cm.len() != g_gamma {
                return Err(invalid(format!("{m} sigma_total len != {g_gamma}")));
            }
            if material.scatter_matrix_per_cm.len() != g_gamma * g_gamma {
                return Err(invalid(format!("{m} scatter_matrix len != {g_gamma}²")));
            }
            if material.transport_mu_bar.len() != g_gamma {
                return Err(invalid(format!("{m} transport_mu_bar len != {g_gamma}")));
            }
            if material.production_matrix_per_cm.len() != g_n * g_gamma {
                return Err(invalid(format!(
                    "{m} production_matrix len != {g_n}×{g_gamma}"
                )));
            }
            if let Some(p1) = &material.scatter_p1_matrix_per_cm
                && p1.len() != g_gamma * g_gamma
            {
                return Err(invalid(format!("{m} scatter_p1 len != {g_gamma}²")));
            }
            if !material.pair_production_matrix_per_cm.is_empty()
                && (material.pair_production_matrix_per_cm.len() != g_gamma * g_gamma
                    || material
                        .pair_production_matrix_per_cm
                        .iter()
                        .any(|v| !v.is_finite() || *v < 0.0))
            {
                return Err(invalid(format!(
                    "{m} pair_production len != {g_gamma}² or negative"
                )));
            }
            if material
                .sigma_total_per_cm
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            {
                return Err(invalid(format!("{m} sigma_total must be finite ≥0")));
            }
            if material
                .transport_mu_bar
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 1.0)
            {
                return Err(invalid(format!("{m} transport_mu_bar must be in [−1,1]")));
            }
            // Conservation: each scatter row sum may not exceed σ_t.
            for g in 0..g_gamma {
                let row_sum: f64 = (0..g_gamma)
                    .map(|gp| material.scatter_matrix_per_cm[g * g_gamma + gp])
                    .sum();
                if row_sum > material.sigma_total_per_cm[g] * (1.0 + 1e-9) + 1e-12 {
                    return Err(invalid(format!(
                        "{m} scatter row {g} sum {row_sum} exceeds sigma_t {}",
                        material.sigma_total_per_cm[g]
                    )));
                }
            }
            if !material.dose_response_gy_cm2.is_empty()
                && material.dose_response_gy_cm2.len() != g_gamma
            {
                return Err(invalid(format!("{m} dose_response len != {g_gamma}")));
            }
        }
        Ok(())
    }

    pub fn photon_group_count(&self) -> usize {
        self.energy_boundaries_ev.len().saturating_sub(1)
    }

    pub fn neutron_group_count(&self) -> usize {
        self.neutron_energy_boundaries_ev.len().saturating_sub(1)
    }

    /// Adapt the photon tables into a `MultigroupData` view for the
    /// shared S_N sweep — photon σ_t/scatter/μ̄ map field-for-field;
    /// the kerma response registers under the `photon` dose component;
    /// the production matrix is consumed separately as the volumetric
    /// source.
    fn as_transport_data(&self) -> MultigroupData {
        MultigroupData {
            schema_version: crate::multigroup::MULTIGROUP_DATA_SCHEMA.into(),
            id: self.id.clone(),
            energy_boundaries_ev: self.energy_boundaries_ev.clone(),
            collapse_declaration: self.collapse_declaration.clone(),
            component_profile: self.component_profile.clone(),
            materials: self
                .materials
                .iter()
                .map(|m| {
                    let mut dose = std::collections::BTreeMap::new();
                    if !m.dose_response_gy_cm2.is_empty() {
                        dose.insert(
                            PHOTON_DOSE_COMPONENT.to_string(),
                            m.dose_response_gy_cm2.clone(),
                        );
                    }
                    MultigroupMaterial {
                        material_id: m.material_id.clone(),
                        sigma_total_per_cm: m.sigma_total_per_cm.clone(),
                        scatter_matrix_per_cm: m.scatter_matrix_per_cm.clone(),
                        scatter_p1_matrix_per_cm: m.scatter_p1_matrix_per_cm.clone(),
                        scatter_legendre_moments_per_cm: None,
                        transport_mu_bar: Some(m.transport_mu_bar.clone()),
                        dose_response_gy_cm2: dose,
                    }
                })
                .collect(),
        }
    }
}

/// Solve the photon problem driven by a converged neutron flux.
///
/// Builds the volumetric photon source
/// `Q(cell,gγ) = Σ_gn φ_n(cell,gn)·Σ_p(gn→gγ)` per cell, then runs the
/// shared S_N sweep on the photon tables with no boundary source.
/// Incoherent downscatter means the group iteration converges in a
/// single outer pass; the option's `max_outer_iterations` is honoured
/// for safety but should never be reached.
pub fn solve_photon(
    case: &TransportCase,
    data: &MultigroupPhotonData,
    neutron_flux: &MultigroupFlux,
    options: &SnOptions,
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<MultigroupFlux, MultigroupError> {
    data.validate()?;
    let transport = data.as_transport_data();
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let g_gamma = data.photon_group_count();
    let g_n = data.neutron_group_count();

    // The neutron flux must carry the group structure the production
    // matrix maps from — same cell count too.
    if neutron_flux.flux.len() != n_cells {
        return Err(MultigroupError::Solve(format!(
            "neutron flux has {} cells, geometry has {n_cells}",
            neutron_flux.flux.len()
        )));
    }
    if neutron_flux.flux.iter().any(|row| row.len() != g_n) {
        return Err(MultigroupError::Solve(format!(
            "neutron flux groups != {g_n} the production matrix maps from"
        )));
    }

    // The transport adapter preserves the photon material ordering, so
    // composition indices map back onto `data.materials` — the
    // production matrix blends by the same volume fractions.
    let compositions = cell_compositions(case, &transport, options.assignment.as_ref())?;
    let (transport, case_material) =
        material_composition_map(case, &transport, options.assignment.as_ref())?;
    let quadrature = level_symmetric_quadrature(options.quadrature_order)?;

    // Volumetric photon source from the converged neutron flux.
    let base_source: Vec<Vec<f64>> = (0..n_cells)
        .map(|cell| {
            (0..g_gamma)
                .map(|gg| {
                    (0..g_n)
                        .map(|gn| {
                            compositions[cell]
                                .iter()
                                .map(|&(mi, f)| {
                                    f * data.materials[mi].production_matrix_per_cm
                                        [gn * g_gamma + gg]
                                })
                                .sum::<f64>()
                                * neutron_flux.flux[cell][gn]
                        })
                        .sum()
                })
                .collect()
        })
        .collect();

    // Pair-production secondaries: annihilation photons (511 keV) are a
    // photon-created volumetric source — a fixed point on the photon
    // field itself. The daughters sit below the 1.022 MeV pair
    // threshold, so they cannot regenerate pairs — the iteration is
    // mathematically exact within a few passes, typically two. An outer
    // source iteration keeps the sweep's conservative-scatter
    // assumption intact.
    let has_pair = data
        .materials
        .iter()
        .any(|m| !m.pair_production_matrix_per_cm.is_empty());
    let mut pair_source = vec![vec![0.0_f64; g_gamma]; n_cells];
    let mut flux = None;
    for _pass in 0..8 {
        let mut fixed_source = base_source.clone();
        if has_pair {
            for cell in 0..n_cells {
                for gg in 0..g_gamma {
                    fixed_source[cell][gg] += pair_source[cell][gg];
                }
            }
        }
        let solved = solve_sn_problem(
            case,
            &transport,
            options,
            &case_material,
            &quadrature,
            &boundary_empty(),
            &fixed_source,
            data_ref.clone(),
            case_ref.clone(),
        )?;
        if !has_pair {
            return Ok(solved);
        }
        let mut next_pair = vec![vec![0.0_f64; g_gamma]; n_cells];
        let mut change = 0.0_f64;
        for cell in 0..n_cells {
            for &(mi, f) in &compositions[cell] {
                let pair = &data.materials[mi].pair_production_matrix_per_cm;
                if pair.is_empty() {
                    continue;
                }
                for gg in 0..g_gamma {
                    for gp in 0..g_gamma {
                        next_pair[cell][gp] += f * pair[gg * g_gamma + gp] * solved.flux[cell][gg];
                    }
                }
            }
            for gg in 0..g_gamma {
                let d = (next_pair[cell][gg] - pair_source[cell][gg]).abs();
                change = change.max(d / next_pair[cell][gg].abs().max(1e-30));
            }
        }
        pair_source = next_pair;
        flux = Some(solved);
        if change < options.convergence {
            break;
        }
    }
    flux.ok_or_else(|| MultigroupError::Solve("photon solve produced no flux".into()))
}

fn boundary_empty() -> BoundarySource {
    BoundarySource::new()
}

/// Fold a photon flux through the data's kerma response into a
/// `PhysicalDoseBundle` — the same fold `sn solve --dose` performs for
/// neutrons, applied to the photon tables via the shared adapter. The
/// response registers under the `photon` dose component; the bundle's
/// component profile must include it.
pub fn fold_photon_dose(
    case: &TransportCase,
    data: &MultigroupPhotonData,
    flux: &MultigroupFlux,
    assignment: Option<&crate::model::MaterialAssignment>,
    component_profile: ContentReference,
    response_ref: ContentReference,
) -> Result<openbnct_core::PhysicalDoseBundle, MultigroupError> {
    let transport = data.as_transport_data();
    crate::multigroup::fold_multigroup_dose(
        case,
        &transport,
        flux,
        assignment,
        component_profile,
        response_ref,
    )
}
