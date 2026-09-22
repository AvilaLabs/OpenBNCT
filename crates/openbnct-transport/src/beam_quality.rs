// SPDX-License-Identifier: MIT

//! IAEA TECDOC-1223-style beam quality characterization
//! (`openbnct.beam-quality`).
//!
//! The report has two layers:
//!
//! - **In-air metrics** are exact properties of the declared beam
//!   description — group fluence rates on the thermal / epithermal / fast
//!   convention, current-to-fluence ratio, and port area. They are
//!   computed from the beam document itself, not transported.
//! - **In-phantom metrics** consume a physical dose bundle produced by a
//!   transport run on a phantom case: a depth profile along the port
//!   axis inside the aperture footprint, weighted into tumor and
//!   normal-tissue dose by declared component effectiveness factors, from
//!   which advantage depth, advantage ratio, and peak therapeutic ratio
//!   follow the TECDOC definitions.
//!
//! A `reference` block optionally carries published or measured values
//! with declared relative tolerances; comparisons are reported pass/fail
//! without promotion semantics — this is characterization, not
//! commissioning.

use openbnct_core::{ContentReference, DoseComponent, PhysicalDoseBundle, ValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::beam::BeamDescription;
use crate::model::{AngularDistribution, EnergyDistribution};

pub const BEAM_QUALITY_SCHEMA: &str = "openbnct.beam-quality/0.1.0";

/// TECDOC-1223 group boundaries in eV.
pub const THERMAL_UPPER_EV: f64 = 0.5;
pub const EPITHERMAL_UPPER_EV: f64 = 1.0e4;

/// A beam quality report: exact in-air metrics plus optional in-phantom
/// metrics and a reference comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamQualityReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding (`id` + `sha256`) of the beam description that was
    /// characterized.
    pub beam: ContentReference,
    pub in_air: InAirMetrics,
    /// Present only when a phantom dose bundle was supplied.
    pub in_phantom: Option<InPhantomMetrics>,
    /// Present only when a reference-values document was supplied.
    pub reference_comparison: Option<Vec<MetricComparison>>,
}

/// In-air beam metrics on the TECDOC-1223 group convention, evaluated at
/// the port reference plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InAirMetrics {
    /// Fluence rate below 0.5 eV, cm⁻² s⁻¹.
    pub thermal_fluence_rate_cm2_s: f64,
    /// Fluence rate in 0.5 eV–10 keV, cm⁻² s⁻¹.
    pub epithermal_fluence_rate_cm2_s: f64,
    /// Fluence rate above 10 keV, cm⁻² s⁻¹.
    pub fast_fluence_rate_cm2_s: f64,
    pub total_fluence_rate_cm2_s: f64,
    /// Thermal share of the total fluence rate.
    pub thermal_fraction: f64,
    /// Fast share of the total fluence rate.
    pub fast_fraction: f64,
    /// Forward-current-to-fluence ratio J/Φ of the declared angular
    /// distribution (1.0 monodirectional, (1+cos θ)/2 for a cone).
    pub current_to_fluence_ratio: f64,
    /// Spectrum mean energy in eV.
    pub mean_energy_ev: f64,
    pub port_area_cm2: f64,
    /// What the declared source does not carry and this report therefore
    /// cannot characterize — e.g. photon contamination of a neutron-only
    /// source term. A measured value belongs in the reference block.
    pub unmodeled_channels: Vec<String>,
}

/// Per-component effectiveness weights applied to physical dose to form
/// tumor and normal-tissue weighted dose profiles. These are operator
/// declarations (compound biological effectiveness factors), not fitted
/// quantities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentWeights {
    pub boron: f64,
    pub nitrogen: f64,
    pub hydrogen: f64,
    pub photon: f64,
}

/// In-phantom beam quality metrics following the TECDOC-1223 definitions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InPhantomMetrics {
    /// Depth profile of weighted tumor dose along the port axis, averaged
    /// over the aperture footprint, in the bundle's dose unit. The first
    /// entry is the entry-face layer.
    pub tumor_dose_profile: Vec<f64>,
    pub normal_tissue_dose_profile: Vec<f64>,
    /// Depth-layer centers in cm along the port axis.
    pub depth_cm: Vec<f64>,
    /// Deepest layer where tumor weighted dose still exceeds the maximum
    /// normal-tissue weighted dose, cm from the entry face.
    pub advantage_depth_cm: f64,
    /// Mean tumor dose over 0..advantage_depth divided by maximum normal
    /// tissue dose.
    pub advantage_ratio: f64,
    /// Maximum over depth of tumor dose / normal tissue dose.
    pub peak_therapeutic_ratio: f64,
    /// Unweighted boron-capture dose per depth layer on the same
    /// footprint average as the weighted profiles. Under uniform dilute
    /// boron loading this profile is proportional to the thermal-neutron
    /// fluence depth profile (constant capture cross-section), so its
    /// maximum marks the measured thermal-fluence maximum depth.
    /// Empty for reports produced before this field existed.
    #[serde(default)]
    pub boron_dose_profile: Vec<f64>,
    /// Absolute thermal fluence rate (n cm⁻² s⁻¹) per depth layer on the
    /// same footprint average — present only when the report was
    /// produced with `--flux` and a declared source rate. Unlike the
    /// dose profiles this is an absolute-scale quantity.
    #[serde(default)]
    pub thermal_fluence_depth_profile_absolute_cm2_s: Option<Vec<f64>>,
    /// Absolute thermal fluence transverse profiles through the port
    /// axis at declared depths — the lateral scan a foil string
    /// measurement resolves (e.g. TECDOC-1223 FIG. 4).
    #[serde(default)]
    pub thermal_fluence_transverse_profiles_cm2_s: Vec<TransverseFluenceProfile>,
    /// How the absolute scale was derived (declared port fluence ×
    /// current-to-fluence × port area → source rate).
    #[serde(default)]
    pub absolute_fluence_note: Option<String>,
    pub tumor_weights: ComponentWeights,
    pub normal_weights: ComponentWeights,
    /// Content binding of the dose bundle the profile was computed from.
    pub dose: ContentReference,
}

/// One transverse fluence profile: the absolute thermal fluence along
/// the port's first in-plane axis through the beam axis, at a declared
/// depth layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransverseFluenceProfile {
    /// Depth-layer center of the scanned layer, cm from the bounding-box
    /// minimum along the port axis.
    pub depth_cm: f64,
    /// Signed lateral offsets from the port axis along the first
    /// in-plane axis, cm (bin centers).
    pub lateral_cm: Vec<f64>,
    /// Absolute thermal fluence rate at each lateral offset,
    /// n cm⁻² s⁻¹.
    pub values_cm2_s: Vec<f64>,
}

/// One metric compared against a declared reference value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricComparison {
    pub metric: String,
    pub computed: f64,
    pub reference: f64,
    pub relative_difference: f64,
    pub relative_tolerance: f64,
    pub passed: bool,
}

/// Operator-supplied reference values with relative tolerances, e.g.
/// published beam-QA numbers for the encoded facility beam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamQualityReference {
    /// `metric` → `{value, relative_tolerance}`. Metric names match the
    /// `InAirMetrics`/`InPhantomMetrics` field names; unknown names are
    /// reported as uncomputed.
    pub values: Vec<ReferenceMetric>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceMetric {
    pub metric: String,
    pub value: f64,
    pub relative_tolerance: f64,
}

/// Fraction of the declared energy spectrum inside `[lo, hi)` eV.
fn spectrum_fraction(energy: &EnergyDistribution, lo: f64, hi: f64) -> f64 {
    match energy {
        EnergyDistribution::Monoenergetic { energy_ev } => {
            f64::from(*energy_ev >= lo && *energy_ev < hi)
        }
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev,
            bin_weights,
        } => {
            let total: f64 = bin_weights.iter().sum();
            bin_weights
                .iter()
                .enumerate()
                .map(|(index, weight)| {
                    let bin_lo = energy_boundaries_ev[index];
                    let bin_hi = energy_boundaries_ev[index + 1];
                    let overlap = bin_hi.min(hi) - bin_lo.max(lo);
                    if overlap <= 0.0 {
                        0.0
                    } else {
                        weight * overlap / (bin_hi - bin_lo)
                    }
                })
                .sum::<f64>()
                / total
        }
    }
}

fn spectrum_mean_ev(energy: &EnergyDistribution) -> f64 {
    match energy {
        EnergyDistribution::Monoenergetic { energy_ev } => *energy_ev,
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev,
            bin_weights,
        } => {
            let total: f64 = bin_weights.iter().sum();
            bin_weights
                .iter()
                .enumerate()
                .map(|(index, weight)| {
                    weight * (energy_boundaries_ev[index] + energy_boundaries_ev[index + 1]) / 2.0
                })
                .sum::<f64>()
                / total
        }
    }
}

/// Forward-current-to-fluence ratio of the angular distribution. For a
/// uniform-in-solid-angle cone of half-angle θ about the port normal,
/// J/Φ = (1 + cos θ)/2; a coaxial monodirectional beam gives exactly 1.
fn current_to_fluence_ratio(angle: &AngularDistribution) -> f64 {
    match angle {
        AngularDistribution::Monodirectional { .. } => 1.0,
        AngularDistribution::IsotropicCone { half_angle_rad, .. } => {
            (1.0 + half_angle_rad.cos()) / 2.0
        }
    }
}

fn port_area_cm2(beam: &BeamDescription) -> f64 {
    match &beam.port.shape {
        crate::beam::PortShape::Circle { radius_cm, .. } => {
            std::f64::consts::PI * radius_cm * radius_cm
        }
        crate::beam::PortShape::Rectangle {
            u_range_cm,
            v_range_cm,
        } => (u_range_cm[1] - u_range_cm[0]) * (v_range_cm[1] - v_range_cm[0]),
    }
}

/// Exact in-air metrics of the declared beam. Returns `None` for the
/// fluence rates when the beam declares only per-particle normalization —
/// fractions and ratios remain meaningful regardless.
pub fn in_air_metrics(beam: &BeamDescription) -> Result<InAirMetrics, BeamQualityError> {
    beam.validate()?;
    let total = match &beam.normalization {
        crate::beam::NormalizationBasis::FluenceRateAtPort { fluence_rate_cm2_s } => {
            Some(*fluence_rate_cm2_s)
        }
        crate::beam::NormalizationBasis::PerSourceParticle => None,
    };
    let thermal = spectrum_fraction(&beam.source.energy, 0.0, THERMAL_UPPER_EV);
    let epithermal = spectrum_fraction(&beam.source.energy, THERMAL_UPPER_EV, EPITHERMAL_UPPER_EV);
    let fast = spectrum_fraction(&beam.source.energy, EPITHERMAL_UPPER_EV, f64::INFINITY);
    let rate = |fraction: f64| total.map(|t| t * fraction).unwrap_or(fraction);
    let mut unmodeled = Vec::new();
    if beam.source.particle == crate::model::ParticleType::Neutron {
        unmodeled.push("photon".to_owned());
    }
    Ok(InAirMetrics {
        thermal_fluence_rate_cm2_s: rate(thermal),
        epithermal_fluence_rate_cm2_s: rate(epithermal),
        fast_fluence_rate_cm2_s: rate(fast),
        total_fluence_rate_cm2_s: total.unwrap_or(1.0),
        thermal_fraction: thermal,
        fast_fraction: fast,
        current_to_fluence_ratio: current_to_fluence_ratio(&beam.source.angle),
        mean_energy_ev: spectrum_mean_ev(&beam.source.energy),
        port_area_cm2: port_area_cm2(beam),
        unmodeled_channels: unmodeled,
    })
}

/// (depth_cm, weighted_tumor, weighted_normal, unweighted_boron) per
/// layer along the port axis.
type DepthProfiles = (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>);

/// Voxel columns inside the port aperture footprint, in the bundle's
/// voxel-index space.
fn footprint_voxels(
    geometry: &openbnct_core::GridGeometry,
    beam: &BeamDescription,
) -> Result<Vec<[usize; 3]>, BeamQualityError> {
    if geometry.direction
        != [
            1.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, //
            0.0, 0.0, 1.0,
        ]
    {
        return Err(BeamQualityError::RotatedGeometryUnsupported);
    }
    let shape = geometry.shape.map(|v| v as usize);
    let (u_axis, v_axis) = beam.port.axis.in_plane_axes();
    // Aperture footprint in voxel-index space (mm → index).
    let in_footprint = |voxel: [usize; 3]| -> bool {
        let u_mm = geometry.origin_mm[u_axis] + voxel[u_axis] as f64 * geometry.spacing_mm[u_axis];
        let v_mm = geometry.origin_mm[v_axis] + voxel[v_axis] as f64 * geometry.spacing_mm[v_axis];
        match &beam.port.shape {
            crate::beam::PortShape::Circle {
                center_uv_cm,
                radius_cm,
            } => {
                let du = u_mm / 10.0 - center_uv_cm[0];
                let dv = v_mm / 10.0 - center_uv_cm[1];
                du * du + dv * dv <= radius_cm * radius_cm
            }
            crate::beam::PortShape::Rectangle {
                u_range_cm,
                v_range_cm,
            } => {
                let u_cm = u_mm / 10.0;
                let v_cm = v_mm / 10.0;
                u_cm >= u_range_cm[0]
                    && u_cm <= u_range_cm[1]
                    && v_cm >= v_range_cm[0]
                    && v_cm <= v_range_cm[1]
            }
        }
    };
    let mut footprint: Vec<[usize; 3]> = Vec::new();
    for u in 0..shape[u_axis] {
        for v in 0..shape[v_axis] {
            let mut voxel = [0_usize; 3];
            voxel[u_axis] = u;
            voxel[v_axis] = v;
            if in_footprint(voxel) {
                footprint.push(voxel);
            }
        }
    }
    if footprint.is_empty() {
        return Err(BeamQualityError::NoFootprintVoxels);
    }
    Ok(footprint)
}

/// Depth profile of a dose bundle along the port axis, averaged over the
/// aperture footprint.
fn depth_profiles(
    beam: &BeamDescription,
    dose: &PhysicalDoseBundle,
    tumor_weights: &ComponentWeights,
    normal_weights: &ComponentWeights,
) -> Result<DepthProfiles, BeamQualityError> {
    let shape = dose.geometry.shape.map(|v| v as usize);
    let axis = beam.port.axis.index();
    let (minimum, _) = dose.geometry.bounding_box_lps_mm()?;
    let footprint = footprint_voxels(&dose.geometry, beam)?;
    let component = |kind: DoseComponent| -> Result<&[f64], BeamQualityError> {
        dose.components
            .iter()
            .find(|volume| volume.component == kind)
            .map(|volume| volume.values.as_slice())
            .ok_or(BeamQualityError::MissingComponent(kind))
    };
    let boron = component(DoseComponent::Boron)?;
    let nitrogen = component(DoseComponent::Nitrogen)?;
    let hydrogen = component(DoseComponent::Hydrogen)?;
    let photon = component(DoseComponent::Photon)?;
    let weight = |w: &ComponentWeights, index: usize| {
        w.boron * boron[index]
            + w.nitrogen * nitrogen[index]
            + w.hydrogen * hydrogen[index]
            + w.photon * photon[index]
    };
    let layers = shape[axis];
    let mut depth_cm = Vec::with_capacity(layers);
    let mut tumor = Vec::with_capacity(layers);
    let mut normal = Vec::with_capacity(layers);
    let mut boron_profile = Vec::with_capacity(layers);
    for layer in 0..layers {
        let center_mm =
            dose.geometry.origin_mm[axis] + layer as f64 * dose.geometry.spacing_mm[axis];
        depth_cm.push((center_mm - minimum[axis]) / 10.0);
        let (mut tumor_sum, mut normal_sum, mut boron_sum) = (0.0, 0.0, 0.0);
        for voxel in &footprint {
            let mut index_voxel = *voxel;
            index_voxel[axis] = layer;
            let index = index_voxel[0] + shape[0] * (index_voxel[1] + shape[1] * index_voxel[2]);
            tumor_sum += weight(tumor_weights, index);
            normal_sum += weight(normal_weights, index);
            boron_sum += boron[index];
        }
        let n = footprint.len() as f64;
        boron_profile.push(boron_sum / n);
        tumor.push(tumor_sum / n);
        normal.push(normal_sum / n);
    }
    Ok((depth_cm, tumor, normal, boron_profile))
}

/// Attach an absolute-scale thermal fluence depth profile to a report
/// that already carries in-phantom metrics: the footprint-averaged sum
/// of group fluxes below `thermal_edge_ev`, scaled by the declared
/// source rate (source neutrons per second).
///
/// The scale derivation belongs in `scale_note` — e.g. `fluence rate
/// 1.1769e9 cm⁻² s⁻¹ × current-to-fluence 0.9945 × port area
/// 153.94 cm² → 1.80e11 source n/s`.
pub fn attach_absolute_fluence_profile(
    report: &mut BeamQualityReport,
    beam: &BeamDescription,
    geometry: &openbnct_core::GridGeometry,
    flux: &crate::multigroup::MultigroupFlux,
    thermal_edge_ev: f64,
    source_rate_per_s: f64,
    scale_note: &str,
) -> Result<(), BeamQualityError> {
    let Some(in_phantom) = report.in_phantom.as_mut() else {
        return Err(BeamQualityError::DegenerateProfile(
            "absolute fluence attach requires in-phantom metrics",
        ));
    };
    let shape = geometry.shape.map(|v| v as usize);
    let n_cells: usize = shape.iter().product();
    if flux.flux.len() != n_cells {
        return Err(BeamQualityError::DegenerateProfile(
            "flux artifact cell count does not match the dose geometry",
        ));
    }
    let axis = beam.port.axis.index();
    let footprint = footprint_voxels(geometry, beam)?;
    // Thermal groups: those whose upper edge sits at or below the
    // declared thermal/epithermal boundary.
    let b = &flux.energy_boundaries_ev;
    let thermal_groups: Vec<usize> = (0..b.len().saturating_sub(1))
        .filter(|&g| b[g] <= thermal_edge_ev)
        .collect();
    if thermal_groups.is_empty() {
        return Err(BeamQualityError::DegenerateProfile(
            "no group lies entirely below the thermal edge",
        ));
    }
    let layers = shape[axis];
    let mut profile = Vec::with_capacity(layers);
    for layer in 0..layers {
        let mut sum = 0.0;
        for voxel in &footprint {
            let mut index_voxel = *voxel;
            index_voxel[axis] = layer;
            let index = index_voxel[0] + shape[0] * (index_voxel[1] + shape[1] * index_voxel[2]);
            for &g in &thermal_groups {
                sum += flux.flux[index][g];
            }
        }
        profile.push(sum / footprint.len() as f64 * source_rate_per_s);
    }
    in_phantom.thermal_fluence_depth_profile_absolute_cm2_s = Some(profile);
    in_phantom.absolute_fluence_note = Some(scale_note.into());
    Ok(())
}

/// Attach absolute thermal-fluence transverse profiles at the declared
/// depths: the lateral scan through the port axis along the port's
/// first in-plane axis, at the nearest depth layer to each requested
/// depth. The detector-string geometry a transverse foil measurement
/// resolves (lateral offset vs fluence at fixed depth).
///
/// `depths_cm` are measured from the bounding-box minimum along the
/// port axis — the same coordinate convention as `depth_cm` on the
/// depth profiles.
pub fn attach_transverse_fluence_profiles(
    report: &mut BeamQualityReport,
    beam: &BeamDescription,
    geometry: &openbnct_core::GridGeometry,
    flux: &crate::multigroup::MultigroupFlux,
    thermal_edge_ev: f64,
    source_rate_per_s: f64,
    depths_cm: &[f64],
) -> Result<(), BeamQualityError> {
    let Some(in_phantom) = report.in_phantom.as_mut() else {
        return Err(BeamQualityError::DegenerateProfile(
            "transverse fluence attach requires in-phantom metrics",
        ));
    };
    let shape = geometry.shape.map(|v| v as usize);
    let n_cells: usize = shape.iter().product();
    if flux.flux.len() != n_cells {
        return Err(BeamQualityError::DegenerateProfile(
            "flux artifact cell count does not match the dose geometry",
        ));
    }
    let axis = beam.port.axis.index();
    let (u_axis, v_axis) = beam.port.axis.in_plane_axes();
    let (minimum, _) = geometry.bounding_box_lps_mm()?;
    // Thermal groups: those whose upper edge sits at or below the
    // declared thermal/epithermal boundary.
    let b = &flux.energy_boundaries_ev;
    let thermal_groups: Vec<usize> = (0..b.len().saturating_sub(1))
        .filter(|&g| b[g] <= thermal_edge_ev)
        .collect();
    if thermal_groups.is_empty() {
        return Err(BeamQualityError::DegenerateProfile(
            "no group lies entirely below the thermal edge",
        ));
    }
    // The scanned row: v-index nearest the port center along v.
    let v_center_cm = match &beam.port.shape {
        crate::beam::PortShape::Circle { center_uv_cm, .. } => center_uv_cm[1],
        crate::beam::PortShape::Rectangle { v_range_cm, .. } => {
            (v_range_cm[0] + v_range_cm[1]) / 2.0
        }
    };
    let v_index = (0..shape[v_axis])
        .min_by(|&i, &j| {
            let ci = geometry.origin_mm[v_axis] + i as f64 * geometry.spacing_mm[v_axis];
            let cj = geometry.origin_mm[v_axis] + j as f64 * geometry.spacing_mm[v_axis];
            (ci / 10.0 - v_center_cm)
                .abs()
                .partial_cmp(&(cj / 10.0 - v_center_cm).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .ok_or(BeamQualityError::NoFootprintVoxels)?;
    let u_center_cm = match &beam.port.shape {
        crate::beam::PortShape::Circle { center_uv_cm, .. } => center_uv_cm[0],
        crate::beam::PortShape::Rectangle { u_range_cm, .. } => {
            (u_range_cm[0] + u_range_cm[1]) / 2.0
        }
    };

    let mut profiles = Vec::with_capacity(depths_cm.len());
    for &depth_cm in depths_cm {
        // Nearest depth layer to the requested depth.
        let layer = (0..shape[axis])
            .min_by(|&i, &j| {
                let di = geometry.origin_mm[axis] + i as f64 * geometry.spacing_mm[axis];
                let dj = geometry.origin_mm[axis] + j as f64 * geometry.spacing_mm[axis];
                let target = minimum[axis] + depth_cm * 10.0;
                (di - target)
                    .abs()
                    .partial_cmp(&(dj - target).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or(BeamQualityError::DegenerateProfile("empty depth axis"))?;
        let depth_actual_cm = (geometry.origin_mm[axis] + layer as f64 * geometry.spacing_mm[axis]
            - minimum[axis])
            / 10.0;
        let mut lateral_cm = Vec::with_capacity(shape[u_axis]);
        let mut values = Vec::with_capacity(shape[u_axis]);
        for i in 0..shape[u_axis] {
            let mut voxel = [0_usize; 3];
            voxel[u_axis] = i;
            voxel[v_axis] = v_index;
            voxel[axis] = layer;
            let index = voxel[0] + shape[0] * (voxel[1] + shape[1] * voxel[2]);
            let u_cm = (geometry.origin_mm[u_axis] + i as f64 * geometry.spacing_mm[u_axis]) / 10.0;
            lateral_cm.push(u_cm - u_center_cm);
            let fluence: f64 = thermal_groups.iter().map(|&g| flux.flux[index][g]).sum();
            values.push(fluence * source_rate_per_s);
        }
        profiles.push(TransverseFluenceProfile {
            depth_cm: depth_actual_cm,
            lateral_cm,
            values_cm2_s: values,
        });
    }
    in_phantom
        .thermal_fluence_transverse_profiles_cm2_s
        .extend(profiles);
    Ok(())
}

/// In-phantom metrics from a transported dose bundle and declared
/// component effectiveness weights.
pub fn in_phantom_metrics(
    beam: &BeamDescription,
    dose: &PhysicalDoseBundle,
    dose_reference: ContentReference,
    tumor_weights: &ComponentWeights,
    normal_weights: &ComponentWeights,
) -> Result<InPhantomMetrics, BeamQualityError> {
    beam.validate()?;
    let (depth_cm, tumor, normal, boron_profile) =
        depth_profiles(beam, dose, tumor_weights, normal_weights)?;
    let normal_max = normal.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if normal_max.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Err(BeamQualityError::DegenerateProfile(
            "normal-tissue dose is zero everywhere",
        ));
    }
    // Advantage depth: deepest layer where the tumor profile still
    // exceeds the normal-tissue maximum.
    let advantage_depth_cm = depth_cm
        .iter()
        .zip(&tumor)
        .filter(|(_, t)| **t >= normal_max)
        .map(|(d, _)| *d)
        .next_back()
        .unwrap_or(0.0);
    let advantage_depth_layers = depth_cm
        .iter()
        .take_while(|d| **d <= advantage_depth_cm)
        .count();
    let tumor_mean = if advantage_depth_layers == 0 {
        0.0
    } else {
        tumor.iter().take(advantage_depth_layers).sum::<f64>() / advantage_depth_layers as f64
    };
    let advantage_ratio = tumor_mean / normal_max;
    let peak_therapeutic_ratio = tumor
        .iter()
        .zip(&normal)
        .filter(|(_, n)| **n > 0.0)
        .map(|(t, n)| t / n)
        .fold(f64::NEG_INFINITY, f64::max);
    if !peak_therapeutic_ratio.is_finite() {
        return Err(BeamQualityError::DegenerateProfile(
            "normal-tissue dose is zero wherever tumor dose is defined",
        ));
    }
    Ok(InPhantomMetrics {
        tumor_dose_profile: tumor,
        normal_tissue_dose_profile: normal,
        depth_cm,
        advantage_depth_cm,
        advantage_ratio,
        peak_therapeutic_ratio,
        boron_dose_profile: boron_profile,
        thermal_fluence_depth_profile_absolute_cm2_s: None,
        thermal_fluence_transverse_profiles_cm2_s: Vec::new(),
        absolute_fluence_note: None,
        tumor_weights: tumor_weights.clone(),
        normal_weights: normal_weights.clone(),
        dose: dose_reference,
    })
}

/// Build the full report; `reference` optionally compares each supplied
/// metric against the computed value within its declared relative
/// tolerance.
pub fn evaluate_beam_quality(
    report_id: &str,
    beam: &BeamDescription,
    beam_reference: ContentReference,
    dose: Option<(
        &PhysicalDoseBundle,
        ContentReference,
        ComponentWeights,
        ComponentWeights,
    )>,
    reference: Option<&BeamQualityReference>,
) -> Result<BeamQualityReport, BeamQualityError> {
    let in_air = in_air_metrics(beam)?;
    let in_phantom = dose
        .map(|(bundle, reference, tumor, normal)| {
            in_phantom_metrics(beam, bundle, reference, &tumor, &normal)
        })
        .transpose()?;
    let mut report = BeamQualityReport {
        schema_version: BEAM_QUALITY_SCHEMA.into(),
        id: report_id.into(),
        beam: beam_reference,
        in_air,
        in_phantom,
        reference_comparison: None,
    };
    report.reference_comparison = reference.map(|reference| {
        reference
            .values
            .iter()
            .filter_map(|metric| {
                crate::measurement::beam_quality_metric(&report, &metric.metric).map(|computed| {
                    let relative_difference = if metric.value == 0.0 {
                        if computed == 0.0 { 0.0 } else { f64::INFINITY }
                    } else {
                        (computed - metric.value).abs() / metric.value.abs()
                    };
                    MetricComparison {
                        metric: metric.metric.clone(),
                        computed,
                        reference: metric.value,
                        relative_difference,
                        relative_tolerance: metric.relative_tolerance,
                        passed: relative_difference <= metric.relative_tolerance,
                    }
                })
            })
            .collect()
    });
    Ok(report)
}

impl BeamQualityReport {
    pub fn validate(&self) -> Result<(), BeamQualityError> {
        if !openbnct_core::schema_matches(&self.schema_version, BEAM_QUALITY_SCHEMA) {
            return Err(BeamQualityError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(BeamQualityError::Validation(
                ValidationError::EmptyIdentifier("report.id"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum BeamQualityError {
    #[error(transparent)]
    Beam(#[from] crate::beam::BeamError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("dose bundle lacks component {0:?}")]
    MissingComponent(DoseComponent),
    #[error("no voxels lie inside the port aperture footprint")]
    NoFootprintVoxels,
    #[error("in-phantom profiles require an identity (axis-aligned) grid direction")]
    RotatedGeometryUnsupported,
    #[error("degenerate dose profile: {0}")]
    DegenerateProfile(&'static str),
    #[error("unsupported beam-quality schema {0:?}; expected {BEAM_QUALITY_SCHEMA:?}")]
    UnsupportedSchema(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beam::{BeamProvenance, PortGeometry, PortShape};
    use crate::model::{FixedSourceDefinition, ParticleType, PlaneAxis, SourceSpatialDistribution};
    use openbnct_core::{
        ComponentProfileReference, DoseVolume, PhysicalDoseBundle, PhysicalTotalDoseVolume,
        TotalUncertaintyMethod,
    };

    fn cone_beam() -> BeamDescription {
        BeamDescription {
            schema_version: crate::beam::BEAM_DESCRIPTION_SCHEMA.into(),
            id: "openbnct.beam.test.v1".into(),
            name: "test beam".into(),
            facility: "test".into(),
            port: PortGeometry {
                axis: PlaneAxis::Z,
                offset_cm: 0.0,
                shape: PortShape::Circle {
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 7.0,
                },
            },
            source: FixedSourceDefinition {
                schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
                id: "openbnct.beam.test.v1".into(),
                particle: ParticleType::Neutron,
                source_sites_per_history: 1,
                statistical_weight_per_site: 1.0,
                space: SourceSpatialDistribution::UniformDisk {
                    axis: PlaneAxis::Z,
                    offset_cm: 0.0,
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 7.0,
                },
                angle: AngularDistribution::IsotropicCone {
                    axis_unit_vector: [0.0, 0.0, 1.0],
                    half_angle_rad: 0.1491,
                },
                energy: EnergyDistribution::TabulatedHistogram {
                    energy_boundaries_ev: vec![1.0e-5, 0.5, 1.0e4, 1.69e7],
                    bin_weights: vec![0.0611, 0.9093, 0.0296],
                },
            },
            normalization: crate::beam::NormalizationBasis::FluenceRateAtPort {
                fluence_rate_cm2_s: 1.1769e9,
            },
            provenance: BeamProvenance::PublishedLiterature {
                citations: vec![crate::beam::Citation {
                    authors: "A".into(),
                    title: "t".into(),
                    venue: "v".into(),
                    year: 2000,
                    doi: None,
                    url: None,
                }],
                derivation_note: "note".into(),
            },
        }
    }

    fn reference() -> ContentReference {
        ContentReference {
            id: "openbnct.beam.test.v1".into(),
            sha256: "sha256:ab".repeat(32),
        }
    }

    #[test]
    fn in_air_group_fractions_and_jphi() {
        let metrics = in_air_metrics(&cone_beam()).unwrap();
        assert!((metrics.thermal_fraction - 0.0611).abs() < 1e-12);
        // Epithermal absorbs the boundary-straddling bins exactly.
        let epithermal_expected = 0.9093;
        assert!(
            (metrics.epithermal_fluence_rate_cm2_s - 1.1769e9 * epithermal_expected).abs() < 1.0
        );
        assert!((metrics.fast_fraction - 0.0296).abs() < 1e-12);
        // (1 + cos 0.1491) / 2
        let expected_jphi = (1.0 + 0.1491_f64.cos()) / 2.0;
        assert!((metrics.current_to_fluence_ratio - expected_jphi).abs() < 1e-12);
        assert!((metrics.port_area_cm2 - std::f64::consts::PI * 49.0).abs() < 1e-9);
        assert_eq!(metrics.unmodeled_channels, vec!["photon"]);
    }

    #[test]
    fn histogram_fraction_splits_straddling_bins_by_width() {
        // A bin [0.25, 0.75] straddles the 0.5 eV thermal edge: half its
        // weight lands in each group.
        let energy = EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: vec![0.25, 0.75, 2.0e4],
            bin_weights: vec![2.0, 1.0],
        };
        let thermal = spectrum_fraction(&energy, 0.0, THERMAL_UPPER_EV);
        // Overlap [0.25,0.5) is half of the 0.5-wide bin → w*0.5.
        assert!((thermal - (2.0 * 0.5) / 3.0).abs() < 1e-12);
    }

    #[test]
    fn per_source_particle_normalization_reports_fractions() {
        let mut beam = cone_beam();
        beam.normalization = crate::beam::NormalizationBasis::PerSourceParticle;
        let metrics = in_air_metrics(&beam).unwrap();
        assert!((metrics.total_fluence_rate_cm2_s - 1.0).abs() < 1e-12);
        // Rates fall back to fractions so the report stays meaningful.
        assert!((metrics.epithermal_fluence_rate_cm2_s - 0.9093).abs() < 1e-12);
    }

    fn dose_bundle() -> PhysicalDoseBundle {
        // 2x2x8 grid, 5 mm spacing: tumor dose = boron, normal uses lower
        // boron weight; boron decays with depth.
        let shape = [2usize, 2, 8];
        let n = shape[0] * shape[1] * shape[2];
        let index = |i: usize, j: usize, k: usize| i + shape[0] * (j + shape[1] * k);
        let mut boron = vec![0.0; n];
        let mut photon = vec![0.0; n];
        for k in 0..shape[2] {
            for j in 0..shape[0] {
                for i in 0..shape[1] {
                    boron[index(i, j, k)] = 10.0 * (-(k as f64) / 3.0).exp();
                    photon[index(i, j, k)] = 1.0;
                }
            }
        }
        let volume = |component, values: Vec<f64>| DoseVolume {
            component,
            unit: openbnct_core::DoseUnit::Gray,
            values,
            absolute_standard_uncertainty: None,
        };
        PhysicalDoseBundle {
            schema_version: "openbnct.physical-dose-bundle/0.2.0".into(),
            case_id: "qa-test".into(),
            frame_of_reference_uid: None,
            geometry: openbnct_core::GridGeometry {
                shape: [2, 2, 8],
                spacing_mm: [5.0; 3],
                origin_mm: [-2.5, -2.5, 0.0],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            component_profile: ComponentProfileReference {
                id: "p".into(),
                sha256: "sha256:aa".repeat(32),
            },
            response_set: reference(),
            components: vec![
                volume(DoseComponent::Boron, boron),
                volume(DoseComponent::Nitrogen, vec![0.5; n]),
                volume(DoseComponent::Hydrogen, vec![0.5; n]),
                volume(DoseComponent::Photon, photon),
            ],
            physical_total: PhysicalTotalDoseVolume {
                unit: openbnct_core::DoseUnit::Gray,
                values: vec![0.0; n],
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::Unavailable,
            },
            provenance_id: "qa-test".into(),
        }
    }

    #[test]
    fn in_phantom_profiles_and_metrics() {
        let beam = cone_beam(); // r=7 cm port covers the whole 1x1 cm face
        let bundle = dose_bundle();
        let tumor = ComponentWeights {
            boron: 3.8,
            nitrogen: 1.0,
            hydrogen: 1.0,
            photon: 1.0,
        };
        let normal = ComponentWeights {
            boron: 1.35,
            nitrogen: 1.0,
            hydrogen: 1.0,
            photon: 1.0,
        };
        let metrics = in_phantom_metrics(&beam, &bundle, reference(), &tumor, &normal).unwrap();
        assert_eq!(metrics.depth_cm.len(), 8);
        assert_eq!(metrics.tumor_dose_profile.len(), 8);
        // Tumor dose decays with depth; AD is where it drops below the
        // normal-tissue maximum (at depth 0).
        assert!(metrics.advantage_depth_cm > 0.0);
        assert!(metrics.advantage_ratio > 1.0);
        assert!(metrics.peak_therapeutic_ratio > 1.0);
        // Layer 0: tumor = 3.8*10 + 0.5 + 0.5 + 1 = 40; normal = 13.5+1+1+... wait check:
        // normal = 1.35*10 + 0.5 + 0.5 + 1.0 = 15.5; tumor = 3.8*10+0.5+0.5+1=40.
        assert!((metrics.tumor_dose_profile[0] - 40.0).abs() < 1e-9);
        assert!((metrics.normal_tissue_dose_profile[0] - 15.5).abs() < 1e-9);
    }

    #[test]
    fn reference_comparison_marks_pass_and_fail() {
        let reference_values = BeamQualityReference {
            values: vec![
                ReferenceMetric {
                    metric: "epithermal_fluence_rate_cm2_s".into(),
                    value: 1.07e9,
                    relative_tolerance: 0.02,
                },
                ReferenceMetric {
                    metric: "current_to_fluence_ratio".into(),
                    value: 0.77,
                    relative_tolerance: 0.05,
                },
            ],
            note: None,
        };
        let report = evaluate_beam_quality(
            "openbnct.beam-quality.test.v1",
            &cone_beam(),
            reference(),
            None,
            Some(&reference_values),
        )
        .unwrap();
        report.validate().unwrap();
        let comparisons = report.reference_comparison.unwrap();
        assert_eq!(comparisons.len(), 2);
        assert!(comparisons[0].passed);
        assert!(!comparisons[1].passed);
    }
}
