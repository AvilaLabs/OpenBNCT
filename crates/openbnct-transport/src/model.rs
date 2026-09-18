// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;

use openbnct_core::{GridGeometry, ValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MASS_FRACTION_TOLERANCE: f64 = 1.0e-12;
const UNIT_VECTOR_TOLERANCE: f64 = 1.0e-12;

/// A transport-ready material with no backend-dependent element expansion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialDefinition {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub density_g_cm3: f64,
    pub temperature_k: f64,
    pub nuclides: Vec<NuclideMassFraction>,
    pub neutron_thermal_treatment: NeutronThermalTreatment,
    /// Optional declared boron microdistribution — the sub-cellular
    /// ¹⁰B compartment model that rescales the boron dose component to
    /// the effective (compound-factor) dose. Absent means the material
    /// boron dose carries no microdistribution correction (uniform
    /// uptake, factor 1.0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boron_microdistribution: Option<BoronMicrodistribution>,
}

/// Declared sub-cellular ¹⁰B compartment model: the fraction of boron
/// resident in each compartment (nucleus / cytoplasm / membrane, summing
/// to one — the convention microdistribution measurements report) plus
/// the cell geometry the α/⁷Li tracks resolve. The ¹⁰B(n,α) products
/// have ~4.8/9.6 µm ranges — sub-cellular placement changes the
/// fraction of emitted energy deposited inside the cell, the standard
/// "compound factor" correction applied to the boron dose component in
/// BNCT treatment-planning practice.
///
/// `compound_factor()` returns `Σ_c f_c·F_c` with
/// `F_c = 1 − exp(−⟨ℓ_c⟩/L)` the first-order escape correction —
/// `⟨ℓ⟩` the mean chord: `4R/3` for volume-distributed compartments
/// (Cauchy mean chord of the cell), `2R/3` for membrane-bound boron's
/// inward surface emission (declared approximation), and `L = 9.5 µm`
/// the effective combined α+⁷Li track range. A first-order geometric
/// model — not a cell-scale Monte Carlo tally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronMicrodistribution {
    /// Fraction of the cell's ¹⁰B resident in the nucleus compartment.
    pub nucleus_fraction: f64,
    /// Fraction resident in the cytoplasm compartment.
    pub cytoplasm_fraction: f64,
    /// Fraction bound to the cell membrane.
    pub membrane_fraction: f64,
    /// Cell radius in micrometres.
    pub cell_radius_um: f64,
    /// Nucleus radius in micrometres (≤ cell radius; carried for the
    /// record — the first-order factor uses the cell chord only).
    pub nucleus_radius_um: f64,
}

/// Effective combined α+⁷Li track range for the ¹⁰B(n,α) products —
/// the mean chord-equivalent length in tissue (declared constant;
/// the α is ~4.8 µm and ⁷Li ~9.6 µm at the emitted energies).
pub const BORON_TRACK_RANGE_UM: f64 = 9.5;

impl BoronMicrodistribution {
    pub fn validate(&self) -> Result<(), TransportModelError> {
        let fractions = [
            self.nucleus_fraction,
            self.cytoplasm_fraction,
            self.membrane_fraction,
        ];
        if fractions.iter().any(|u| !u.is_finite() || *u < 0.0)
            || (fractions.iter().sum::<f64>() - 1.0).abs() > 1e-6
        {
            return Err(TransportModelError::MalformedMaterialRegion(
                "boron microdistribution fractions must be non-negative \
                 and sum to one"
                    .into(),
            ));
        }
        if !self.cell_radius_um.is_finite()
            || self.cell_radius_um <= 0.0
            || !self.nucleus_radius_um.is_finite()
            || self.nucleus_radius_um <= 0.0
            || self.nucleus_radius_um > self.cell_radius_um
        {
            return Err(TransportModelError::MalformedMaterialRegion(
                "boron microdistribution radii must be positive with nucleus ≤ cell".into(),
            ));
        }
        Ok(())
    }

    /// `Σ_c f_c·F_c` — the compartment-fraction-weighted deposited
    /// fraction (the compound factor multiplying the boron dose
    /// component).
    pub fn compound_factor(&self) -> f64 {
        let l = BORON_TRACK_RANGE_UM;
        let r = self.cell_radius_um;
        let f_vol = 1.0 - (-4.0 * r / (3.0 * l)).exp();
        let f_mem = 1.0 - (-2.0 * r / (3.0 * l)).exp();
        self.nucleus_fraction * f_vol
            + self.cytoplasm_fraction * f_vol
            + self.membrane_fraction * f_mem
    }
}

impl MaterialDefinition {
    pub fn validate(&self) -> Result<(), TransportModelError> {
        validate_identifier("material.schema_version", &self.schema_version)?;
        validate_identifier("material.id", &self.id)?;
        if !self.density_g_cm3.is_finite() || self.density_g_cm3 <= 0.0 {
            return Err(TransportModelError::InvalidDensity);
        }
        if !self.temperature_k.is_finite() || self.temperature_k <= 0.0 {
            return Err(TransportModelError::InvalidTemperature);
        }
        if self.nuclides.is_empty() {
            return Err(TransportModelError::EmptyComposition);
        }
        if let Some(micro) = &self.boron_microdistribution {
            micro.validate()?;
        }

        let mut names = BTreeSet::new();
        let mut sum = 0.0;
        for nuclide in &self.nuclides {
            if !is_nuclide_name(&nuclide.name) {
                return Err(TransportModelError::InvalidNuclideName(
                    nuclide.name.clone(),
                ));
            }
            if !names.insert(nuclide.name.as_str()) {
                return Err(TransportModelError::DuplicateNuclide(nuclide.name.clone()));
            }
            if !nuclide.mass_fraction.is_finite() || nuclide.mass_fraction <= 0.0 {
                return Err(TransportModelError::InvalidMassFraction(
                    nuclide.name.clone(),
                ));
            }
            sum += nuclide.mass_fraction;
        }
        if (sum - 1.0).abs() > MASS_FRACTION_TOLERANCE {
            return Err(TransportModelError::MassFractionsDoNotSumToOne { sum });
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NuclideMassFraction {
    /// GNDS-style nuclide name such as `H1`, `B10`, or `Am242_m1`.
    pub name: String,
    pub mass_fraction: f64,
}

/// Treatment below the resolved-resonance range for this material.
///
/// R2 intentionally supports only free-gas scattering. Bound-atom tables will
/// require a new, content-bound contract rather than an untracked string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NeutronThermalTreatment {
    FreeGas,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedSourceDefinition {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub particle: ParticleType,
    /// Number of source sites sampled for each source history.
    pub source_sites_per_history: u32,
    /// Statistical weight assigned to each sampled source site.
    pub statistical_weight_per_site: f64,
    pub space: SourceSpatialDistribution,
    pub angle: AngularDistribution,
    pub energy: EnergyDistribution,
}

impl FixedSourceDefinition {
    pub fn validate(&self) -> Result<(), TransportModelError> {
        validate_identifier("source.schema_version", &self.schema_version)?;
        validate_identifier("source.id", &self.id)?;
        if self.source_sites_per_history != 1 {
            return Err(TransportModelError::UnsupportedSourceSitesPerHistory(
                self.source_sites_per_history,
            ));
        }
        if self.statistical_weight_per_site != 1.0 {
            return Err(TransportModelError::UnsupportedSourceWeight(
                self.statistical_weight_per_site,
            ));
        }

        match &self.space {
            SourceSpatialDistribution::UniformCartesianPlane { .. }
            | SourceSpatialDistribution::UniformAxisPlane { .. } => {
                let Some((_, offset_cm, u_range_cm, v_range_cm)) = self.space.plane_parts() else {
                    return Err(TransportModelError::InvalidSourceSpace);
                };
                if !valid_interval(u_range_cm)
                    || !valid_interval(v_range_cm)
                    || !offset_cm.is_finite()
                {
                    return Err(TransportModelError::InvalidSourceSpace);
                }
            }
            SourceSpatialDistribution::UniformDisk {
                offset_cm,
                center_uv_cm,
                radius_cm,
                ..
            } => {
                if !offset_cm.is_finite()
                    || center_uv_cm.iter().any(|v| !v.is_finite())
                    || !radius_cm.is_finite()
                    || *radius_cm <= 0.0
                {
                    return Err(TransportModelError::InvalidSourceSpace);
                }
            }
        }
        match &self.angle {
            AngularDistribution::Monodirectional { unit_vector } => {
                if !is_unit_vector(unit_vector) {
                    return Err(TransportModelError::InvalidSourceDirection);
                }
            }
            AngularDistribution::IsotropicCone {
                axis_unit_vector,
                half_angle_rad,
            } => {
                if !is_unit_vector(axis_unit_vector)
                    || !half_angle_rad.is_finite()
                    || *half_angle_rad <= 0.0
                    || *half_angle_rad > std::f64::consts::PI
                {
                    return Err(TransportModelError::InvalidSourceDirection);
                }
            }
        }
        match &self.energy {
            EnergyDistribution::Monoenergetic { energy_ev }
                if !energy_ev.is_finite() || *energy_ev <= 0.0 =>
            {
                return Err(TransportModelError::InvalidSourceEnergy);
            }
            EnergyDistribution::Monoenergetic { .. } => {}
            EnergyDistribution::TabulatedHistogram {
                energy_boundaries_ev,
                bin_weights,
            } => {
                if energy_boundaries_ev.len() < 2
                    || energy_boundaries_ev.len() != bin_weights.len() + 1
                    || energy_boundaries_ev
                        .iter()
                        .any(|e| !e.is_finite() || *e <= 0.0)
                    || energy_boundaries_ev
                        .windows(2)
                        .any(|pair| pair[0] >= pair[1])
                    || bin_weights.iter().any(|w| !w.is_finite() || *w < 0.0)
                    || bin_weights.iter().all(|w| *w == 0.0)
                {
                    return Err(TransportModelError::InvalidSourceEnergy);
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticleType {
    Neutron,
    Photon,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSpatialDistribution {
    /// Uniform sampling in x and y at a fixed z in Cartesian centimetres.
    UniformCartesianPlane {
        x_range_cm: [f64; 2],
        y_range_cm: [f64; 2],
        z_cm: f64,
        interval_convention: IntervalConvention,
    },
    /// Uniform sampling over a bounded plane perpendicular to a world axis.
    /// `u_range_cm`/`v_range_cm` are world-coordinate intervals along the
    /// plane's in-plane axes in canonical order: `X` planes span (y, z),
    /// `Y` planes span (x, z), `Z` planes span (x, y). `offset_cm` is the
    /// world coordinate of the plane along `axis`.
    UniformAxisPlane {
        axis: PlaneAxis,
        u_range_cm: [f64; 2],
        v_range_cm: [f64; 2],
        offset_cm: f64,
        interval_convention: IntervalConvention,
    },
    /// Uniform sampling over a bounded disk perpendicular to a world axis —
    /// the natural form of a circular beam aperture. `center_uv_cm` is the
    /// disk center in the plane's in-plane world coordinates (canonical
    /// `(u, v)` order for `axis`); `offset_cm` is the world coordinate of
    /// the disk plane along `axis`.
    UniformDisk {
        axis: PlaneAxis,
        offset_cm: f64,
        center_uv_cm: [f64; 2],
        radius_cm: f64,
    },
}

/// World axis a `UniformAxisPlane` is perpendicular to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaneAxis {
    X,
    Y,
    Z,
}

impl PlaneAxis {
    /// Index (0, 1, 2) of this axis in a world-coordinate triple.
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            PlaneAxis::X => 0,
            PlaneAxis::Y => 1,
            PlaneAxis::Z => 2,
        }
    }

    /// In-plane world-axis indices `(u, v)` in canonical order.
    #[must_use]
    pub fn in_plane_axes(self) -> (usize, usize) {
        match self {
            PlaneAxis::X => (1, 2),
            PlaneAxis::Y => (0, 2),
            PlaneAxis::Z => (0, 1),
        }
    }
}

impl SourceSpatialDistribution {
    /// Normalize a rectangular planar variant to `(axis, offset_cm,
    /// u_range_cm, v_range_cm)` in world coordinates. `None` for
    /// non-rectangular variants (`UniformDisk`) — callers must not fall
    /// back to a bounding box, which would silently widen the aperture.
    #[must_use]
    pub fn plane_parts(&self) -> Option<(PlaneAxis, f64, [f64; 2], [f64; 2])> {
        match *self {
            SourceSpatialDistribution::UniformCartesianPlane {
                x_range_cm,
                y_range_cm,
                z_cm,
                ..
            } => Some((PlaneAxis::Z, z_cm, x_range_cm, y_range_cm)),
            SourceSpatialDistribution::UniformAxisPlane {
                axis,
                u_range_cm,
                v_range_cm,
                offset_cm,
                ..
            } => Some((axis, offset_cm, u_range_cm, v_range_cm)),
            SourceSpatialDistribution::UniformDisk { .. } => None,
        }
    }

    /// The world axis the distribution's plane/disk is perpendicular to.
    #[must_use]
    pub fn axis(&self) -> PlaneAxis {
        match self {
            SourceSpatialDistribution::UniformCartesianPlane { .. } => PlaneAxis::Z,
            SourceSpatialDistribution::UniformAxisPlane { axis, .. }
            | SourceSpatialDistribution::UniformDisk { axis, .. } => *axis,
        }
    }

    /// World coordinate of the plane/disk along its perpendicular axis.
    #[must_use]
    pub fn offset_cm(&self) -> f64 {
        match self {
            SourceSpatialDistribution::UniformCartesianPlane { z_cm, .. } => *z_cm,
            SourceSpatialDistribution::UniformAxisPlane { offset_cm, .. }
            | SourceSpatialDistribution::UniformDisk { offset_cm, .. } => *offset_cm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntervalConvention {
    HalfOpen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AngularDistribution {
    Monodirectional {
        unit_vector: [f64; 3],
    },
    /// Uniform in solid angle inside a cone of half-angle
    /// `half_angle_rad` about `axis_unit_vector` — the divergence model
    /// of a collimated beam. `half_angle_rad = π` is full-sphere
    /// isotropic.
    IsotropicCone {
        axis_unit_vector: [f64; 3],
        half_angle_rad: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnergyDistribution {
    Monoenergetic {
        energy_ev: f64,
    },
    /// Piecewise-uniform energy histogram: `energy_boundaries_ev` holds
    /// `n+1` strictly increasing positive edges and `bin_weights` the `n`
    /// bin probability masses (normalized by the sampler, not required to
    /// sum to one). The admitted encoding for a tabulated facility-beam
    /// spectrum.
    TabulatedHistogram {
        energy_boundaries_ev: Vec<f64>,
        bin_weights: Vec<f64>,
    },
}

/// Complete backend-neutral input to one transport preparation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportCase {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub geometry: GridGeometry,
    pub material: MaterialDefinition,
    pub source: FixedSourceDefinition,
    /// Requested independent source histories, not batches or source weight.
    pub requested_histories: u64,
}

impl TransportCase {
    pub fn validate(&self) -> Result<(), TransportModelError> {
        validate_identifier("transport_case.schema_version", &self.schema_version)?;
        validate_identifier("transport_case.case_id", &self.case_id)?;
        self.geometry.voxel_count()?;
        self.material.validate()?;
        self.source.validate()?;
        if self.requested_histories == 0 {
            return Err(TransportModelError::ZeroRequestedHistories);
        }
        Ok(())
    }
}

pub const MATERIAL_ASSIGNMENT_SCHEMA: &str = "openbnct.material-assignment/0.2.0";

/// A transport-neutral DICOM-derived material assignment: named voxel regions
/// that override the case's base material. Regions are either exact
/// axis-aligned voxel boxes (realized as CSG cells) or explicit voxel sets
/// (realized as per-voxel lattice elements); arbitrary masks are represented
/// exactly rather than approximated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialAssignment {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// The base material filling all voxels outside every region. Carried
    /// explicitly so per-voxel density-ratio correction can be computed from
    /// the assignment alone; generation requires it to equal the bound
    /// material artifact.
    pub base_material: MaterialDefinition,
    pub regions: Vec<MaterialRegion>,
    /// Content-bound provenance of the source case the masks were rasterized
    /// from — for example `case:sha256:<hex>`.
    pub provenance_id: String,
}

/// One named voxel region carrying a material that overrides the base
/// material on the region's voxels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialRegion {
    pub name: String,
    pub material: MaterialDefinition,
    /// The voxels the material applies to, as `{"kind": "voxel_box" |
    /// "voxel_set", ...}`.
    pub shape: MaterialRegionShape,
}

/// How a material region's member voxels are declared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MaterialRegionShape {
    /// Inclusive axis-aligned voxel-index bounds; exact CSG box.
    VoxelBox { lower: [u32; 3], upper: [u32; 3] },
    /// Explicit `[i, j, k]` voxel indices (grid convention
    /// `i + nx*j + nx*ny*k`); realized as per-voxel lattice elements.
    VoxelSet { indices: Vec<[u32; 3]> },
    /// Partial-cell volumes: each listed voxel carries `fraction` of
    /// this region's material and the remainder of the base material —
    /// the sub-voxel mixture rule for boundary voxels. `fractions[i]`
    /// pairs with `indices[i]`, each in (0, 1]; the sum across
    /// overlapping regions must stay ≤ 1. The solver blends the
    /// macroscopic tables (σ_t, scatter moments, dose responses) by
    /// volume fraction — the correct first-order treatment for
    /// optically-thin material mixtures.
    VoxelFractions {
        indices: Vec<[u32; 3]>,
        fractions: Vec<f64>,
    },
}

impl MaterialRegion {
    /// Whether this region's voxels can be represented as a single CSG box.
    #[must_use]
    pub fn is_axis_aligned_box(&self) -> bool {
        matches!(self.shape, MaterialRegionShape::VoxelBox { .. })
    }

    /// Number of member voxels.
    #[must_use]
    pub fn voxel_count(&self) -> usize {
        match &self.shape {
            MaterialRegionShape::VoxelBox { lower, upper } => {
                if (0..3).any(|axis| lower[axis] > upper[axis]) {
                    0
                } else {
                    (0..3)
                        .map(|axis| (upper[axis] - lower[axis] + 1) as usize)
                        .product()
                }
            }
            MaterialRegionShape::VoxelSet { indices } => indices.len(),
            MaterialRegionShape::VoxelFractions { indices, .. } => indices.len(),
        }
    }

    /// Visit every member voxel's `[i, j, k]` index.
    pub fn for_each_voxel(&self, mut visit: impl FnMut([u32; 3])) {
        match &self.shape {
            MaterialRegionShape::VoxelBox { lower, upper } => {
                for k in lower[2]..=upper[2] {
                    for j in lower[1]..=upper[1] {
                        for i in lower[0]..=upper[0] {
                            visit([i, j, k]);
                        }
                    }
                }
            }
            MaterialRegionShape::VoxelSet { indices } => {
                for &index in indices {
                    visit(index);
                }
            }
            MaterialRegionShape::VoxelFractions { indices, .. } => {
                for &index in indices {
                    visit(index);
                }
            }
        }
    }

    /// World-space (mm) edges of a `VoxelBox` region under an axis-aligned
    /// grid. Voxel indices span half-open cells, so the upper edge adds one
    /// spacing. `None` for voxel-set regions, which have no single CSG box.
    #[must_use]
    pub fn world_bounds_mm(&self, geometry: &GridGeometry) -> Option<([f64; 3], [f64; 3])> {
        let MaterialRegionShape::VoxelBox { lower, upper } = &self.shape else {
            return None;
        };
        let mut lower_mm = [0.0; 3];
        let mut upper_mm = [0.0; 3];
        for axis in 0..3 {
            let edge0 = geometry.origin_mm[axis] - 0.5 * geometry.spacing_mm[axis];
            lower_mm[axis] = edge0 + f64::from(lower[axis]) * geometry.spacing_mm[axis];
            upper_mm[axis] = edge0 + f64::from(upper[axis] + 1) * geometry.spacing_mm[axis];
        }
        Some((lower_mm, upper_mm))
    }
}

impl MaterialAssignment {
    pub fn validate(&self, geometry: &GridGeometry) -> Result<(), TransportModelError> {
        validate_identifier("material_assignment.schema_version", &self.schema_version)?;
        if !openbnct_core::schema_matches(&self.schema_version, MATERIAL_ASSIGNMENT_SCHEMA) {
            return Err(TransportModelError::UnsupportedMaterialAssignmentSchema(
                self.schema_version.clone(),
            ));
        }
        validate_identifier("material_assignment.case_id", &self.case_id)?;
        self.base_material.validate()?;
        validate_identifier("material_assignment.provenance_id", &self.provenance_id)?;
        if self.regions.is_empty() {
            return Err(TransportModelError::EmptyMaterialAssignment);
        }
        // Region world bounds are emitted in grid-axis coordinates: the
        // voxel-axis direction matrix must be the identity so that a voxel
        // index maps to a world-axis-aligned box (or lattice element).
        if geometry.direction != [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            return Err(TransportModelError::NonAxisAlignedMaterialAssignment);
        }
        let mut names = BTreeSet::new();
        for region in &self.regions {
            validate_identifier("material_region.name", &region.name)?;
            if !names.insert(region.name.as_str()) {
                return Err(TransportModelError::DuplicateMaterialRegion(
                    region.name.clone(),
                ));
            }
            region.material.validate()?;
            if region.voxel_count() == 0 {
                return Err(TransportModelError::EmptyMaterialRegion(
                    region.name.clone(),
                ));
            }
            if let MaterialRegionShape::VoxelBox { lower, upper } = &region.shape {
                for axis in 0..3 {
                    if lower[axis] > upper[axis] || upper[axis] >= geometry.shape[axis] {
                        return Err(TransportModelError::MaterialRegionOutsideGrid(
                            region.name.clone(),
                        ));
                    }
                }
            }
            // Voxel-set indices are bounds-checked before occupancy; an
            // out-of-grid index would silently map to a wrong flat voxel.
            if let MaterialRegionShape::VoxelSet { indices } = &region.shape {
                if indices
                    .iter()
                    .any(|voxel| (0..3).any(|axis| voxel[axis] >= geometry.shape[axis]))
                {
                    return Err(TransportModelError::MaterialRegionOutsideGrid(
                        region.name.clone(),
                    ));
                }
                let mut unique = BTreeSet::new();
                if indices.iter().any(|voxel| !unique.insert(voxel)) {
                    return Err(TransportModelError::DuplicateVoxelInRegion(
                        region.name.clone(),
                    ));
                }
            }
            // Partial-cell fractions: same length as indices, each
            // fraction finite and in (0, 1], indices in-grid and
            // unique. A voxel may appear in several fraction regions
            // (ternary blends) but never alongside a full region —
            // checked in the occupancy pass below.
            if let MaterialRegionShape::VoxelFractions { indices, fractions } = &region.shape {
                if indices.len() != fractions.len() {
                    return Err(TransportModelError::MalformedMaterialRegion(format!(
                        "region {:?} indices/fractions length mismatch",
                        region.name
                    )));
                }
                if fractions
                    .iter()
                    .any(|f| !f.is_finite() || *f <= 0.0 || *f > 1.0)
                {
                    return Err(TransportModelError::MalformedMaterialRegion(format!(
                        "region {:?} fractions must be in (0,1]",
                        region.name
                    )));
                }
                if indices
                    .iter()
                    .any(|voxel| (0..3).any(|axis| voxel[axis] >= geometry.shape[axis]))
                {
                    return Err(TransportModelError::MaterialRegionOutsideGrid(
                        region.name.clone(),
                    ));
                }
                let mut unique = BTreeSet::new();
                if indices.iter().any(|voxel| !unique.insert(voxel)) {
                    return Err(TransportModelError::DuplicateVoxelInRegion(
                        region.name.clone(),
                    ));
                }
            }
        }
        // No voxel may carry two materials: occupancy is checked per voxel so
        // boxes and voxel sets share one exact overlap rule.
        let voxel_total = geometry
            .shape
            .iter()
            .map(|dim| *dim as usize)
            .product::<usize>();
        let nx = geometry.shape[0] as usize;
        let ny = geometry.shape[1] as usize;
        let mut occupancy = vec![usize::MAX; voxel_total];
        // Fraction regions carry a fractional occupancy per voxel —
        // they may share a voxel with other fraction regions (sum ≤ 1)
        // but never with a full box/set region (a voxel is either fully
        // one material or a declared mixture).
        let mut fraction_sum = vec![0.0_f64; voxel_total];
        let mut fraction_owner = vec![usize::MAX; voxel_total];
        for (index, region) in self.regions.iter().enumerate() {
            if let MaterialRegionShape::VoxelFractions { indices, fractions } = &region.shape {
                for (voxel, f) in indices.iter().zip(fractions.iter()) {
                    let flat =
                        voxel[0] as usize + nx * voxel[1] as usize + nx * ny * voxel[2] as usize;
                    if occupancy[flat] != usize::MAX {
                        return Err(TransportModelError::OverlappingMaterialRegions {
                            first: self.regions[occupancy[flat]].name.clone(),
                            second: region.name.clone(),
                        });
                    }
                    fraction_sum[flat] += f;
                    if fraction_sum[flat] > 1.0 + 1e-9 {
                        let first = fraction_owner[flat];
                        return Err(TransportModelError::VoxelFractionOverflow {
                            first: if first == usize::MAX {
                                region.name.clone()
                            } else {
                                self.regions[first].name.clone()
                            },
                            second: region.name.clone(),
                        });
                    }
                    if fraction_owner[flat] == usize::MAX {
                        fraction_owner[flat] = index;
                    }
                }
                continue;
            }
            let mut overlap = None;
            region.for_each_voxel(|voxel| {
                let flat = voxel[0] as usize + nx * voxel[1] as usize + nx * ny * voxel[2] as usize;
                if occupancy[flat] == usize::MAX && fraction_owner[flat] == usize::MAX {
                    occupancy[flat] = index;
                } else {
                    overlap = Some(if occupancy[flat] == usize::MAX {
                        fraction_owner[flat]
                    } else {
                        occupancy[flat]
                    });
                }
            });
            if let Some(first) = overlap {
                return Err(TransportModelError::OverlappingMaterialRegions {
                    first: self.regions[first].name.clone(),
                    second: region.name.clone(),
                });
            }
        }
        Ok(())
    }
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), TransportModelError> {
    if value.trim().is_empty() {
        Err(TransportModelError::EmptyIdentifier(label))
    } else {
        Ok(())
    }
}

fn valid_interval(interval: [f64; 2]) -> bool {
    interval.iter().all(|value| value.is_finite()) && interval[0] < interval[1]
}

fn is_unit_vector(vector: &[f64; 3]) -> bool {
    if vector.iter().any(|value| !value.is_finite()) {
        return false;
    }
    let norm_squared = vector[0].mul_add(
        vector[0],
        vector[1].mul_add(vector[1], vector[2] * vector[2]),
    );
    (norm_squared - 1.0).abs() <= UNIT_VECTOR_TOLERANCE
}

fn is_nuclide_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_uppercase() {
        return false;
    }
    let mut index = 1;
    if index < bytes.len() && bytes[index].is_ascii_lowercase() {
        index += 1;
    }
    let mass_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if mass_start == index || bytes[mass_start] == b'0' {
        return false;
    }
    if index == bytes.len() {
        return true;
    }
    if !bytes[index..].starts_with(b"_m") {
        return false;
    }
    index += 2;
    let state_start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    state_start < index && bytes[state_start] != b'0' && index == bytes.len()
}

#[derive(Debug, Error, PartialEq)]
pub enum TransportModelError {
    #[error(transparent)]
    Geometry(#[from] ValidationError),
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("material density must be finite and greater than zero g/cm3")]
    InvalidDensity,
    #[error("material temperature must be finite and greater than zero kelvin")]
    InvalidTemperature,
    #[error("material nuclide composition is empty")]
    EmptyComposition,
    #[error("invalid GNDS-style nuclide name {0:?}")]
    InvalidNuclideName(String),
    #[error("nuclide {0} has a non-positive or non-finite mass fraction")]
    InvalidMassFraction(String),
    #[error("nuclide {0} occurs more than once")]
    DuplicateNuclide(String),
    #[error("material mass fractions sum to {sum}; expected one within 1e-12")]
    MassFractionsDoNotSumToOne { sum: f64 },
    #[error("this normalization profile requires one source site per history, observed {0}")]
    UnsupportedSourceSitesPerHistory(u32),
    #[error("this normalization profile requires unit source weight, observed {0}")]
    UnsupportedSourceWeight(f64),
    #[error("source spatial distribution has an invalid interval or coordinate")]
    InvalidSourceSpace,
    #[error("source direction must be a finite unit vector")]
    InvalidSourceDirection,
    #[error("source energy must be finite and greater than zero eV")]
    InvalidSourceEnergy,
    #[error("transport case requests zero source histories")]
    ZeroRequestedHistories,
    #[error("unsupported material-assignment schema {0:?}")]
    UnsupportedMaterialAssignmentSchema(String),
    #[error("material assignment contains no regions")]
    EmptyMaterialAssignment,
    #[error("material region {0} occurs more than once")]
    DuplicateMaterialRegion(String),
    #[error("material region {0} has voxel bounds outside the case grid")]
    MaterialRegionOutsideGrid(String),
    #[error("material regions {first:?} and {second:?} overlap")]
    OverlappingMaterialRegions { first: String, second: String },
    #[error("material region {0} contains no voxels")]
    EmptyMaterialRegion(String),
    #[error("material region {0} lists the same voxel more than once")]
    DuplicateVoxelInRegion(String),
    #[error("material region malformed: {0}")]
    MalformedMaterialRegion(String),
    #[error("voxel-fraction regions {first:?} and {second:?} exceed a full voxel (sum > 1)")]
    VoxelFractionOverflow { first: String, second: String },
    #[error(
        "material assignment requires an axis-aligned grid (identity direction); \
         rotated or permuted voxel axes are not representable by box surfaces or rectilinear lattices"
    )]
    NonAxisAlignedMaterialAssignment,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATERIAL_JSON: &str =
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const SOURCE_JSON: &str =
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/source.json");

    fn material() -> MaterialDefinition {
        serde_json::from_str(MATERIAL_JSON).unwrap()
    }

    fn source() -> FixedSourceDefinition {
        serde_json::from_str(SOURCE_JSON).unwrap()
    }

    #[test]
    fn frozen_material_is_exact_and_valid() {
        let material = material();
        assert_eq!(material.id, "nctforge.nf-bnct-001.material.v1");
        assert_eq!(material.nuclides.len(), 10);
        assert_eq!(material.validate(), Ok(()));
    }

    #[test]
    fn frozen_source_is_exact_and_valid() {
        let source = source();
        assert_eq!(source.id, "nctforge.nf-bnct-001.source.v1");
        assert_eq!(source.particle, ParticleType::Neutron);
        assert_eq!(source.validate(), Ok(()));
    }

    #[test]
    fn rejects_duplicate_nuclide() {
        let mut material = material();
        material.nuclides.push(material.nuclides[0].clone());
        assert_eq!(
            material.validate(),
            Err(TransportModelError::DuplicateNuclide("H1".into()))
        );
    }

    #[test]
    fn rejects_nonunit_source_direction() {
        let mut source = source();
        source.angle = AngularDistribution::Monodirectional {
            unit_vector: [0.0, 0.0, 0.5],
        };
        assert_eq!(
            source.validate(),
            Err(TransportModelError::InvalidSourceDirection)
        );
    }

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 4, 4],
            spacing_mm: [5.0, 5.0, 5.0],
            origin_mm: [-10.0, -10.0, -10.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn assignment() -> MaterialAssignment {
        MaterialAssignment {
            schema_version: MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: "nf-bnct-001".into(),
            base_material: material(),
            regions: vec![MaterialRegion {
                name: "core".into(),
                material: material(),
                shape: MaterialRegionShape::VoxelBox {
                    lower: [1, 1, 1],
                    upper: [2, 2, 2],
                },
            }],
            provenance_id: "case:sha256:test".into(),
        }
    }

    #[test]
    fn valid_assignment_passes_and_maps_to_world_edges() {
        let geometry = geometry();
        let assignment = assignment();
        assignment.validate(&geometry).unwrap();
        // Voxel center -10 mm, spacing 5 mm -> edge0 = -12.5 mm.
        // Voxel [1,1,1]..[2,2,2] maps to [-7.5, 2.5) mm on every axis.
        let (lower, upper) = assignment.regions[0].world_bounds_mm(&geometry).unwrap();
        assert_eq!(lower, [-7.5, -7.5, -7.5]);
        assert_eq!(upper, [2.5, 2.5, 2.5]);
    }

    #[test]
    fn rejects_bad_assignment_schema_regions_and_overlap() {
        let geometry = geometry();

        let mut wrong_schema = assignment();
        wrong_schema.schema_version = "other/9.9.9".into();
        assert!(matches!(
            wrong_schema.validate(&geometry),
            Err(TransportModelError::UnsupportedMaterialAssignmentSchema(_))
        ));

        let mut empty = assignment();
        empty.regions.clear();
        assert_eq!(
            empty.validate(&geometry),
            Err(TransportModelError::EmptyMaterialAssignment)
        );

        let mut outside = assignment();
        outside.regions[0].shape = MaterialRegionShape::VoxelBox {
            lower: [1, 1, 1],
            upper: [3, 3, 4],
        };
        assert_eq!(
            outside.validate(&geometry),
            Err(TransportModelError::MaterialRegionOutsideGrid(
                "core".into()
            ))
        );

        let mut inverted = assignment();
        inverted.regions[0].shape = MaterialRegionShape::VoxelBox {
            lower: [2, 2, 2],
            upper: [1, 1, 1],
        };
        // An inverted box contains no voxels — the empty-region gate fires.
        assert_eq!(
            inverted.validate(&geometry),
            Err(TransportModelError::EmptyMaterialRegion("core".into()))
        );

        let mut duplicate = assignment();
        let copy = duplicate.regions[0].clone();
        duplicate.regions.push(MaterialRegion {
            name: "second".into(),
            ..copy
        });
        duplicate.regions[1].name = "core".into();
        assert_eq!(
            duplicate.validate(&geometry),
            Err(TransportModelError::DuplicateMaterialRegion("core".into()))
        );

        let mut overlapping = assignment();
        overlapping.regions[0].name = "first".into();
        overlapping.regions.push(MaterialRegion {
            name: "second".into(),
            material: material(),
            shape: MaterialRegionShape::VoxelBox {
                lower: [2, 2, 2],
                upper: [3, 3, 3],
            },
        });
        assert!(matches!(
            overlapping.validate(&geometry),
            Err(TransportModelError::OverlappingMaterialRegions { .. })
        ));

        // Touching boxes do not overlap — halves are half-open.
        let mut adjacent = assignment();
        adjacent.regions[0].shape = MaterialRegionShape::VoxelBox {
            lower: [1, 1, 1],
            upper: [1, 1, 1],
        };
        adjacent.regions.push(MaterialRegion {
            name: "second".into(),
            material: material(),
            shape: MaterialRegionShape::VoxelBox {
                lower: [2, 2, 2],
                upper: [3, 3, 3],
            },
        });
        adjacent.validate(&geometry).unwrap();
    }

    #[test]
    fn voxel_set_regions_validate_per_voxel() {
        let geo = geometry();

        // A non-box L-shape: three voxels that do not fill a bounding box.
        let mut set_assignment = assignment();
        set_assignment.regions[0].shape = MaterialRegionShape::VoxelSet {
            indices: vec![[0, 0, 0], [1, 0, 0], [0, 1, 0]],
        };
        set_assignment.validate(&geo).unwrap();
        assert_eq!(set_assignment.regions[0].voxel_count(), 3);
        assert!(!set_assignment.regions[0].is_axis_aligned_box());
        assert_eq!(set_assignment.regions[0].world_bounds_mm(&geo), None);

        // Box and set share the per-voxel overlap rule.
        let mut mixed = assignment();
        mixed.regions[0].name = "box".into();
        mixed.regions.push(MaterialRegion {
            name: "set".into(),
            material: material(),
            shape: MaterialRegionShape::VoxelSet {
                indices: vec![[0, 0, 0], [2, 2, 2]],
            },
        });
        assert_eq!(
            mixed.validate(&geo),
            Err(TransportModelError::OverlappingMaterialRegions {
                first: "box".into(),
                second: "set".into(),
            })
        );

        // Out-of-grid, duplicate, and empty voxel sets are rejected.
        let mut outside = assignment();
        outside.regions[0].shape = MaterialRegionShape::VoxelSet {
            indices: vec![[0, 0, 0], [4, 0, 0]],
        };
        assert_eq!(
            outside.validate(&geo),
            Err(TransportModelError::MaterialRegionOutsideGrid(
                "core".into()
            ))
        );

        let mut duplicated = assignment();
        duplicated.regions[0].shape = MaterialRegionShape::VoxelSet {
            indices: vec![[0, 0, 0], [0, 0, 0]],
        };
        assert_eq!(
            duplicated.validate(&geo),
            Err(TransportModelError::DuplicateVoxelInRegion("core".into()))
        );

        let mut empty_set = assignment();
        empty_set.regions[0].shape = MaterialRegionShape::VoxelSet { indices: vec![] };
        assert_eq!(
            empty_set.validate(&geo),
            Err(TransportModelError::EmptyMaterialRegion("core".into()))
        );

        // Rotated grids cannot express voxel-index regions in world space.
        let mut rotated = geometry();
        rotated.direction = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(
            assignment().validate(&rotated),
            Err(TransportModelError::NonAxisAlignedMaterialAssignment)
        );
    }

    fn fraction_region(name: &str, voxels: &[[u32; 3]], fractions: &[f64]) -> MaterialRegion {
        MaterialRegion {
            name: name.into(),
            material: material(),
            shape: MaterialRegionShape::VoxelFractions {
                indices: voxels.to_vec(),
                fractions: fractions.to_vec(),
            },
        }
    }

    #[test]
    fn voxel_fractions_validate_local_shape() {
        let geo = geometry();

        // Valid single-region fractions.
        let mut ok = assignment();
        ok.regions[0] = fraction_region("frac", &[[0, 0, 0], [1, 1, 1]], &[0.5, 1.0]);
        ok.validate(&geo).unwrap();
        assert_eq!(ok.regions[0].voxel_count(), 2);
        assert!(!ok.regions[0].is_axis_aligned_box());

        // Length mismatch is malformed.
        let mut mismatch = assignment();
        mismatch.regions[0] = fraction_region("frac", &[[0, 0, 0], [1, 0, 0]], &[0.5]);
        assert!(matches!(
            mismatch.validate(&geo),
            Err(TransportModelError::MalformedMaterialRegion(_))
        ));

        // Empty is still the empty-region error.
        let mut empty = assignment();
        empty.regions[0] = fraction_region("frac", &[], &[]);
        assert_eq!(
            empty.validate(&geo),
            Err(TransportModelError::EmptyMaterialRegion("frac".into()))
        );

        // Out-of-grid and duplicate voxels are rejected.
        let mut outside = assignment();
        outside.regions[0] = fraction_region("frac", &[[4, 0, 0]], &[0.5]);
        assert_eq!(
            outside.validate(&geo),
            Err(TransportModelError::MaterialRegionOutsideGrid(
                "frac".into()
            ))
        );
        let mut duplicated = assignment();
        duplicated.regions[0] = fraction_region("frac", &[[0, 0, 0], [0, 0, 0]], &[0.3, 0.4]);
        assert_eq!(
            duplicated.validate(&geo),
            Err(TransportModelError::DuplicateVoxelInRegion("frac".into()))
        );

        // Fractions must be finite and in (0, 1].
        for bad in [0.0, -0.2, 1.5, f64::NAN, f64::INFINITY] {
            let mut a = assignment();
            a.regions[0] = fraction_region("frac", &[[0, 0, 0]], &[bad]);
            assert!(
                matches!(
                    a.validate(&geo),
                    Err(TransportModelError::MalformedMaterialRegion(_))
                ),
                "fraction {bad} must be rejected"
            );
        }
    }

    #[test]
    fn voxel_fractions_share_voxels_but_never_exceed_one() {
        let geo = geometry();

        // Two fraction regions summing to exactly 1 in one voxel —
        // a valid two-material mixture.
        let mut mixture = assignment();
        mixture.regions[0] = fraction_region("skin", &[[0, 0, 0], [1, 0, 0]], &[0.3, 0.25]);
        mixture
            .regions
            .push(fraction_region("tumor", &[[0, 0, 0]], &[0.7]));
        mixture.validate(&geo).unwrap();

        // Three-way mixtures are equally valid — a 0.2/0.5/0.3 split.
        let mut tri = assignment();
        tri.regions[0] = fraction_region("skin", &[[0, 0, 0]], &[0.2]);
        tri.regions
            .push(fraction_region("tumor", &[[0, 0, 0]], &[0.5]));
        tri.regions.push(fraction_region(
            "bone",
            &[[0, 0, 0], [2, 2, 2]],
            &[0.3, 0.9],
        ));
        tri.validate(&geo).unwrap();

        // Sum exceeding 1 is rejected and names both regions.
        let mut over = assignment();
        over.regions[0] = fraction_region("skin", &[[0, 0, 0]], &[0.6]);
        over.regions
            .push(fraction_region("tumor", &[[0, 0, 0]], &[0.6]));
        assert_eq!(
            over.validate(&geo),
            Err(TransportModelError::VoxelFractionOverflow {
                first: "skin".into(),
                second: "tumor".into(),
            })
        );

        // A full box/set region may not share a voxel with any
        // fraction occupancy — in either declaration order.
        let mut frac_first = assignment();
        frac_first.regions[0] = fraction_region("frac", &[[0, 0, 0]], &[0.5]);
        frac_first.regions.push(MaterialRegion {
            name: "full".into(),
            material: material(),
            shape: MaterialRegionShape::VoxelSet {
                indices: vec![[0, 0, 0]],
            },
        });
        assert_eq!(
            frac_first.validate(&geo),
            Err(TransportModelError::OverlappingMaterialRegions {
                first: "frac".into(),
                second: "full".into(),
            })
        );

        let mut full_first = assignment();
        full_first.regions[0].name = "full".into();
        full_first.regions[0].shape = MaterialRegionShape::VoxelSet {
            indices: vec![[0, 0, 0]],
        };
        full_first
            .regions
            .push(fraction_region("frac", &[[0, 0, 0]], &[0.5]));
        assert_eq!(
            full_first.validate(&geo),
            Err(TransportModelError::OverlappingMaterialRegions {
                first: "full".into(),
                second: "frac".into(),
            })
        );
    }

    #[test]
    fn boron_microdistribution_validates_and_factors() {
        let mut micro = BoronMicrodistribution {
            nucleus_fraction: 0.4,
            cytoplasm_fraction: 0.5,
            membrane_fraction: 0.1,
            cell_radius_um: 6.0,
            nucleus_radius_um: 4.0,
        };
        micro.validate().unwrap();
        // Uniform-in-cell boron (all cytoplasm+nucleus) → the volume
        // chord factor; membrane-bound boron escapes more (smaller
        // chord → smaller factor).
        let uniform = BoronMicrodistribution {
            membrane_fraction: 0.0,
            cytoplasm_fraction: 1.0 - 0.4,
            ..micro.clone()
        };
        let membrane_heavy = BoronMicrodistribution {
            nucleus_fraction: 0.1,
            cytoplasm_fraction: 0.2,
            membrane_fraction: 0.7,
            ..micro.clone()
        };
        assert!(uniform.compound_factor() > membrane_heavy.compound_factor());
        assert!(micro.compound_factor() > 0.0 && micro.compound_factor() <= 1.0);

        // Fractions not summing to one / non-finite are malformed.
        micro.membrane_fraction = 0.5;
        assert!(matches!(
            micro.validate(),
            Err(TransportModelError::MalformedMaterialRegion(_))
        ));
        micro.membrane_fraction = f64::NAN;
        assert!(matches!(
            micro.validate(),
            Err(TransportModelError::MalformedMaterialRegion(_))
        ));
        micro.membrane_fraction = 0.1;
        micro.nucleus_radius_um = 9.0; // nucleus larger than cell
        assert!(matches!(
            micro.validate(),
            Err(TransportModelError::MalformedMaterialRegion(_))
        ));

        // A material carrying a malformed microdistribution is rejected.
        let mut mat = material();
        mat.boron_microdistribution = Some(micro.clone());
        assert!(matches!(
            mat.validate(),
            Err(TransportModelError::MalformedMaterialRegion(_))
        ));
        mat.boron_microdistribution = None;
        assert_eq!(mat.validate(), Ok(()));
    }

    #[test]
    fn validates_representative_nuclide_names() {
        assert!(is_nuclide_name("B10"));
        assert!(is_nuclide_name("Am242_m1"));
        assert!(!is_nuclide_name("B-10"));
        assert!(!is_nuclide_name("H01"));
        assert!(!is_nuclide_name("h1"));
    }

    #[test]
    fn expanded_benchmark_cases_validate() {
        // NF-BNCT-002: heterogeneous deep-penetration case — frozen case,
        // three materials, and the declared assignment must all load and
        // validate against the case grid.
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../benchmarks/synthetic");
        let case2: TransportCase = serde_json::from_str(
            &std::fs::read_to_string(format!("{root}/nf-bnct-002/transport/case.json")).unwrap(),
        )
        .unwrap();
        case2.validate().unwrap();
        assert_eq!(case2.case_id, "nf-bnct-002");
        assert_eq!(case2.geometry.voxel_count().unwrap(), 216_000);
        let assignment2: MaterialAssignment = serde_json::from_str(
            &std::fs::read_to_string(format!("{root}/nf-bnct-002/transport/assignment.json"))
                .unwrap(),
        )
        .unwrap();
        assignment2.validate(&case2.geometry).unwrap();
        for name in [
            "material-tissue-b10-10ugg",
            "material-tissue-b10-40ugg",
            "material-skull-equivalent",
        ] {
            let material: MaterialDefinition = serde_json::from_str(
                &std::fs::read_to_string(format!("{root}/nf-bnct-002/transport/{name}.json"))
                    .unwrap(),
            )
            .unwrap();
            material.validate().unwrap();
        }

        // NF-BNCT-003: idealized pure-absorber slab.
        let case3: TransportCase = serde_json::from_str(
            &std::fs::read_to_string(format!("{root}/nf-bnct-003/transport/case.json")).unwrap(),
        )
        .unwrap();
        case3.validate().unwrap();
        assert_eq!(case3.case_id, "nf-bnct-003");
        assert_eq!(case3.geometry.voxel_count().unwrap(), 640);
        assert_eq!(case3.material.nuclides.len(), 1);
        assert_eq!(case3.material.nuclides[0].name, "B10");
    }
}
