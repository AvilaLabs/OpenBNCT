// SPDX-License-Identifier: Apache-2.0

//! Beam-shaping assemblies (`openbnct.beam-shaping-assembly/0.1.0`) and
//! assembly sweeps (`openbnct.bsa-sweep/0.1.0`).
//!
//! A beam-shaping assembly is the ordered layer stack between an
//! accelerator target and the beam port: moderators, filters,
//! reflectors, collimators, and the delimiting aperture. Each layer is a
//! slab of a declared material perpendicular to the beam axis with a
//! declared radial footprint (full cross-section, disk, or annulus).
//! The assembly rasterizes onto a transport case's scoring grid as a
//! `MaterialAssignment` — voxel boxes for full-width slabs, exact voxel
//! sets for disk and annular footprints — so a candidate assembly goes
//! straight through the existing deck-generation and transport path.
//!
//! The artifact describes geometry and materials only. It asserts
//! nothing about the beam that emerges: beam quality is measured by
//! `beam characterize` on transported output, not predicted here.

use std::collections::BTreeSet;

use openbnct_core::{ContentReference, GridGeometry};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::{
    MaterialAssignment, MaterialDefinition, MaterialRegion, MaterialRegionShape, PlaneAxis,
    TransportModelError,
};

pub const BSA_SCHEMA: &str = "openbnct.beam-shaping-assembly/0.1.0";
pub const BSA_SWEEP_SCHEMA: &str = "openbnct.bsa-sweep/0.1.0";

/// Function role of a layer in the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BsaLayerKind {
    /// Bulk moderating medium (e.g. MgF₂, Al+AlF₃, D₂O).
    Moderator,
    /// Fast-neutron or photon suppression filter (e.g. ⁶LiF, Pb, Cd).
    Filter,
    /// Back-scattering reflector around the stack (e.g. Pb, graphite).
    Reflector,
    /// Collimating cone or aperture plate (e.g. Pb, Ni, polyethylene).
    Collimator,
    /// The delimiting aperture at the port face (e.g. ⁶Li-polyethylene).
    DelimitingAperture,
    /// Generic shielding member that fits no narrower role.
    Shield,
}

/// Radial footprint of a layer perpendicular to the beam axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BsaRadialExtent {
    /// The layer spans the assembly's full declared `max_radius_cm`.
    Full,
    /// Solid disk of the given radius, cm.
    Disk { radius_cm: f64 },
    /// Annulus between the given radii, cm — a collimator wall or
    /// aperture ring; the bore inside `inner_radius_cm` stays empty.
    Annulus {
        inner_radius_cm: f64,
        outer_radius_cm: f64,
    },
}

/// One slab of the stack, ordered upstream → downstream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BsaLayer {
    pub name: String,
    pub kind: BsaLayerKind,
    pub material: MaterialDefinition,
    /// Slab thickness along the beam axis, cm.
    pub thickness_cm: f64,
    pub radial: BsaRadialExtent,
}

/// A complete beam-shaping assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamShapingAssembly {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    /// Stable document identifier, e.g. `openbnct.bsa.moderated-li.v1`.
    pub id: String,
    /// Beam propagation axis — the stack builds along it.
    pub axis: PlaneAxis,
    /// World coordinate of the stack's upstream face along `axis`, cm.
    pub upstream_offset_cm: f64,
    /// Propagation direction sign along `axis` (+1 or −1).
    pub direction_sign: i8,
    /// Beam-axis position in the in-plane world coordinates, cm.
    pub center_uv_cm: [f64; 2],
    /// Assembly outer radius, cm — the footprint of `Full` layers.
    pub max_radius_cm: f64,
    /// Ordered upstream → downstream.
    pub layers: Vec<BsaLayer>,
    /// Content binding of the beam this assembly shapes, when declared.
    pub shaped_beam: Option<ContentReference>,
    /// Provenance of the assembly declaration itself.
    pub provenance_id: String,
}

impl BeamShapingAssembly {
    pub fn validate(&self) -> Result<(), BsaError> {
        if !openbnct_core::schema_matches(&self.schema_version, BSA_SCHEMA) {
            return Err(BsaError::UnsupportedSchema(self.schema_version.clone()));
        }
        non_empty("bsa.id", &self.id)?;
        non_empty("bsa.provenance_id", &self.provenance_id)?;
        if self.direction_sign != 1 && self.direction_sign != -1 {
            return Err(BsaError::InvalidDirectionSign);
        }
        if !(self.upstream_offset_cm.is_finite() && self.center_uv_cm.iter().all(|v| v.is_finite()))
        {
            return Err(BsaError::InvalidGeometry);
        }
        if !(self.max_radius_cm.is_finite() && self.max_radius_cm > 0.0) {
            return Err(BsaError::InvalidGeometry);
        }
        if self.layers.is_empty() {
            return Err(BsaError::EmptyStack);
        }
        let mut names = BTreeSet::new();
        for layer in &self.layers {
            non_empty("layer.name", &layer.name)?;
            if !names.insert(layer.name.clone()) {
                return Err(BsaError::DuplicateLayer(layer.name.clone()));
            }
            layer.material.validate()?;
            if !(layer.thickness_cm.is_finite() && layer.thickness_cm > 0.0) {
                return Err(BsaError::InvalidThickness(layer.name.clone()));
            }
            match layer.radial {
                BsaRadialExtent::Full => {}
                BsaRadialExtent::Disk { radius_cm } => {
                    if !(radius_cm.is_finite()
                        && radius_cm > 0.0
                        && radius_cm <= self.max_radius_cm)
                    {
                        return Err(BsaError::InvalidRadialExtent(layer.name.clone()));
                    }
                }
                BsaRadialExtent::Annulus {
                    inner_radius_cm,
                    outer_radius_cm,
                } => {
                    if !(inner_radius_cm.is_finite()
                        && outer_radius_cm.is_finite()
                        && inner_radius_cm > 0.0
                        && outer_radius_cm > inner_radius_cm
                        && outer_radius_cm <= self.max_radius_cm)
                    {
                        return Err(BsaError::InvalidRadialExtent(layer.name.clone()));
                    }
                }
            }
        }
        if let Some(beam) = &self.shaped_beam {
            beam.validate()
                .map_err(|_| BsaError::InvalidBeamReference)?;
        }
        Ok(())
    }

    /// Total stack depth along the beam axis, cm.
    #[must_use]
    pub fn total_depth_cm(&self) -> f64 {
        self.layers.iter().map(|l| l.thickness_cm).sum()
    }

    /// Rasterize the assembly onto a scoring grid as a
    /// `MaterialAssignment`. Full layers become exact `VoxelBox` slabs;
    /// disks and annuli become `VoxelSet`s of the voxels whose centers
    /// fall inside the declared radial footprint (an exact membership
    /// test — the footprint is never silently widened to a box).
    /// `base_material` fills every voxel the stack does not claim.
    pub fn to_material_assignment(
        &self,
        geometry: &GridGeometry,
        base_material: MaterialDefinition,
        provenance_id: &str,
    ) -> Result<MaterialAssignment, BsaError> {
        self.validate()?;
        base_material.validate()?;
        let axis = self.axis.index();
        let (u_axis, v_axis) = self.axis.in_plane_axes();
        let sign = f64::from(self.direction_sign);
        let mut cursor = self.upstream_offset_cm;
        let mut regions = Vec::new();
        for layer in &self.layers {
            // World-coordinate span of this slab along the beam axis.
            let (lo, hi) = if sign > 0.0 {
                (cursor, cursor + layer.thickness_cm)
            } else {
                (cursor - layer.thickness_cm, cursor)
            };
            cursor += sign * layer.thickness_cm;
            // Voxel index range whose cell centers fall inside [lo, hi).
            let extent = geometry.shape[axis];
            let spacing = geometry.spacing_mm[axis] / 10.0; // cm
            let origin = geometry.origin_mm[axis] / 10.0;
            let mut slab: Vec<u32> = Vec::new();
            for index in 0..extent {
                let center = origin + (index as f64) * spacing;
                if center >= lo && center < hi {
                    slab.push(index);
                }
            }
            if slab.is_empty() {
                return Err(BsaError::LayerOutsideGrid(layer.name.clone()));
            }
            let shape = match layer.radial {
                BsaRadialExtent::Full => {
                    let mut lower = [0_u32; 3];
                    let mut upper = [
                        geometry.shape[0] - 1,
                        geometry.shape[1] - 1,
                        geometry.shape[2] - 1,
                    ];
                    lower[axis] = slab[0];
                    upper[axis] = *slab.last().expect("nonempty slab");
                    MaterialRegionShape::VoxelBox { lower, upper }
                }
                BsaRadialExtent::Disk { radius_cm } => radial_voxels(
                    geometry,
                    &slab,
                    axis,
                    u_axis,
                    v_axis,
                    self.center_uv_cm,
                    0.0,
                    radius_cm,
                )?,
                BsaRadialExtent::Annulus {
                    inner_radius_cm,
                    outer_radius_cm,
                } => radial_voxels(
                    geometry,
                    &slab,
                    axis,
                    u_axis,
                    v_axis,
                    self.center_uv_cm,
                    inner_radius_cm,
                    outer_radius_cm,
                )?,
            };
            regions.push(MaterialRegion {
                name: layer.name.clone(),
                material: layer.material.clone(),
                shape,
            });
        }
        Ok(MaterialAssignment {
            schema_version: crate::model::MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: String::new(), // bound by the caller
            base_material,
            regions,
            provenance_id: provenance_id.into(),
        })
    }
}

/// Exact voxel set for a radial footprint over a slab of axis indices.
#[allow(clippy::too_many_arguments)]
fn radial_voxels(
    geometry: &GridGeometry,
    slab: &[u32],
    axis: usize,
    u_axis: usize,
    v_axis: usize,
    center_uv_cm: [f64; 2],
    inner_radius_cm: f64,
    outer_radius_cm: f64,
) -> Result<MaterialRegionShape, BsaError> {
    let mut indices = Vec::new();
    let u_spacing = geometry.spacing_mm[u_axis] / 10.0;
    let v_spacing = geometry.spacing_mm[v_axis] / 10.0;
    let u_origin = geometry.origin_mm[u_axis] / 10.0;
    let v_origin = geometry.origin_mm[v_axis] / 10.0;
    for &along in slab {
        for j in 0..geometry.shape[u_axis] {
            let u = u_origin + j as f64 * u_spacing;
            for k in 0..geometry.shape[v_axis] {
                let v = v_origin + k as f64 * v_spacing;
                let r = ((u - center_uv_cm[0]).powi(2) + (v - center_uv_cm[1]).powi(2)).sqrt();
                if r >= inner_radius_cm && r < outer_radius_cm {
                    let mut index = [0_u32; 3];
                    index[axis] = along;
                    index[u_axis] = j;
                    index[v_axis] = k;
                    indices.push(index);
                }
            }
        }
    }
    if indices.is_empty() {
        return Err(BsaError::EmptyFootprint);
    }
    Ok(MaterialRegionShape::VoxelSet { indices })
}

fn non_empty(label: &'static str, value: &str) -> Result<(), BsaError> {
    if value.trim().is_empty() {
        Err(BsaError::EmptyIdentifier(label))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sweeps
// ---------------------------------------------------------------------------

/// One swept parameter: a layer's thickness over an explicit value list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BsaSweepParameter {
    /// Name of the layer (must match a layer in the base assembly).
    pub layer: String,
    /// Explicit candidate thicknesses, cm.
    pub thickness_cm: Vec<f64>,
}

/// A declared sweep: the base assembly plus the parameters to vary —
/// every combination produces one variant assembly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BsaSweep {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding of the base `BeamShapingAssembly` document.
    pub base: ContentReference,
    pub parameters: Vec<BsaSweepParameter>,
}

/// One emitted variant of a sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BsaSweepVariant {
    /// `{sweep_id}.variant-N`.
    pub id: String,
    /// `(layer name, thickness_cm)` assignments for this variant.
    pub parameters: Vec<(String, f64)>,
    /// Content binding of the emitted variant document — filled by the
    /// caller after serialization, since the hash covers the bytes.
    pub content: ContentReference,
}

/// The record enumerating every variant a sweep produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BsaSweepRecord {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding of the sweep spec that was executed.
    pub sweep: ContentReference,
    /// Content binding of the base assembly.
    pub base: ContentReference,
    pub variants: Vec<BsaSweepVariant>,
    pub qualification: String,
}

/// Enumerate the variant assemblies a sweep spec produces — the
/// cartesian product of every declared parameter's value list, each
/// carrying the base's layers with the swept thicknesses replaced.
/// Variant ids are `{base.id}.s{N}` in product order.
pub fn enumerate_bsa_sweep(
    sweep: &BsaSweep,
    base: &BeamShapingAssembly,
) -> Result<Vec<BeamShapingAssembly>, BsaError> {
    if !openbnct_core::schema_matches(&sweep.schema_version, BSA_SWEEP_SCHEMA) {
        return Err(BsaError::UnsupportedSchema(sweep.schema_version.clone()));
    }
    base.validate()?;
    if sweep.parameters.is_empty() {
        return Err(BsaError::EmptySweep);
    }
    let mut names = BTreeSet::new();
    for parameter in &sweep.parameters {
        if !base.layers.iter().any(|l| l.name == parameter.layer) {
            return Err(BsaError::UnknownLayer(parameter.layer.clone()));
        }
        if !names.insert(parameter.layer.clone()) {
            return Err(BsaError::DuplicateParameter(parameter.layer.clone()));
        }
        if parameter.thickness_cm.is_empty()
            || parameter
                .thickness_cm
                .iter()
                .any(|t| !t.is_finite() || *t <= 0.0)
        {
            return Err(BsaError::InvalidSweepValues(parameter.layer.clone()));
        }
    }
    // Cartesian product over parameter value lists.
    let mut combos: Vec<Vec<(String, f64)>> = vec![Vec::new()];
    for parameter in &sweep.parameters {
        let mut next = Vec::new();
        for combo in &combos {
            for &thickness in &parameter.thickness_cm {
                let mut c = combo.clone();
                c.push((parameter.layer.clone(), thickness));
                next.push(c);
            }
        }
        combos = next;
    }
    let mut variants = Vec::with_capacity(combos.len());
    for (index, combo) in combos.iter().enumerate() {
        let mut variant = base.clone();
        variant.id = format!("{}.s{}", base.id, index);
        for (layer_name, thickness) in combo {
            for layer in &mut variant.layers {
                if layer.name == *layer_name {
                    layer.thickness_cm = *thickness;
                }
            }
        }
        variant.provenance_id = format!("sweep:{}", sweep.id);
        variant.validate()?;
        variants.push(variant);
    }
    Ok(variants)
}

/// Record the parameter assignment of one enumerated variant (the
/// caller supplies the content binding after writing the document).
pub fn sweep_variant_assignment(
    sweep: &BsaSweep,
    variant: &BeamShapingAssembly,
) -> Vec<(String, f64)> {
    sweep
        .parameters
        .iter()
        .filter_map(|p| {
            variant
                .layers
                .iter()
                .find(|l| l.name == p.layer)
                .map(|l| (p.layer.clone(), l.thickness_cm))
        })
        .collect()
}

impl BsaSweepRecord {
    pub fn validate(&self) -> Result<(), BsaError> {
        if !openbnct_core::schema_matches(&self.schema_version, BSA_SWEEP_SCHEMA) {
            return Err(BsaError::UnsupportedSchema(self.schema_version.clone()));
        }
        non_empty("sweep.id", &self.id)?;
        self.sweep
            .validate()
            .map_err(|_| BsaError::InvalidBeamReference)?;
        self.base
            .validate()
            .map_err(|_| BsaError::InvalidBeamReference)?;
        if self.variants.is_empty() {
            return Err(BsaError::EmptySweep);
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum BsaError {
    #[error(transparent)]
    Model(#[from] TransportModelError),
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("unsupported schema {0:?}")]
    UnsupportedSchema(String),
    #[error("direction sign must be +1 or −1")]
    InvalidDirectionSign,
    #[error("assembly geometry must be finite with positive max radius")]
    InvalidGeometry,
    #[error("an assembly needs at least one layer")]
    EmptyStack,
    #[error("duplicate layer name {0:?}")]
    DuplicateLayer(String),
    #[error("layer {0:?} thickness must be finite and positive cm")]
    InvalidThickness(String),
    #[error("layer {0:?} radial extent is malformed or exceeds the assembly radius")]
    InvalidRadialExtent(String),
    #[error("shaped-beam content reference is invalid")]
    InvalidBeamReference,
    #[error("layer {0:?} produces no voxels — the slab falls outside the grid")]
    LayerOutsideGrid(String),
    #[error("radial footprint selects no voxels on this grid")]
    EmptyFootprint,
    #[error("a sweep needs at least one parameter")]
    EmptySweep,
    #[error("sweep parameter {0:?} matches no layer in the base assembly")]
    UnknownLayer(String),
    #[error("sweep parameter {0:?} is declared twice")]
    DuplicateParameter(String),
    #[error("sweep parameter {0:?} needs at least one positive finite thickness")]
    InvalidSweepValues(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NeutronThermalTreatment, NuclideMassFraction};

    fn material(name: &str) -> MaterialDefinition {
        MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: name.into(),
            density_g_cm3: 1.0,
            temperature_k: 300.0,
            nuclides: vec![NuclideMassFraction {
                name: "H1".into(),
                mass_fraction: 1.0,
            }],
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
        }
    }

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [20, 20, 20],
            spacing_mm: [10.0; 3],
            origin_mm: [-100.0, -100.0, -100.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn assembly() -> BeamShapingAssembly {
        BeamShapingAssembly {
            schema_version: BSA_SCHEMA.into(),
            id: "openbnct.bsa.test.v1".into(),
            axis: PlaneAxis::Z,
            upstream_offset_cm: -10.0,
            direction_sign: 1,
            center_uv_cm: [0.0, 0.0],
            max_radius_cm: 10.0,
            layers: vec![
                BsaLayer {
                    name: "moderator".into(),
                    kind: BsaLayerKind::Moderator,
                    material: material("mgf2"),
                    thickness_cm: 5.0,
                    radial: BsaRadialExtent::Full,
                },
                BsaLayer {
                    name: "filter".into(),
                    kind: BsaLayerKind::Filter,
                    material: material("lif6"),
                    thickness_cm: 2.0,
                    radial: BsaRadialExtent::Full,
                },
                BsaLayer {
                    name: "aperture".into(),
                    kind: BsaLayerKind::DelimitingAperture,
                    material: material("lipoly"),
                    thickness_cm: 3.0,
                    radial: BsaRadialExtent::Annulus {
                        inner_radius_cm: 5.0,
                        outer_radius_cm: 10.0,
                    },
                },
            ],
            shaped_beam: None,
            provenance_id: "test".into(),
        }
    }

    #[test]
    fn valid_assembly_passes_and_stacks() {
        let bsa = assembly();
        bsa.validate().unwrap();
        assert!((bsa.total_depth_cm() - 10.0).abs() < 1e-12);
    }

    #[test]
    fn rasterizes_layers_in_order() {
        let assignment = assembly()
            .to_material_assignment(&geometry(), material("air"), "test")
            .unwrap();
        assert_eq!(assignment.regions.len(), 3);
        // Moderator: z slab −10..−5 cm → voxel centers −9.5..−5.5 →
        // indices 0..4 inclusive = 5 voxels × full 20×20 face.
        let MaterialRegionShape::VoxelBox { lower, upper } = &assignment.regions[0].shape else {
            panic!("expected voxel box");
        };
        assert_eq!(lower[2], 0);
        assert_eq!(upper[2], 4);
        // Filter: −5..−3 cm → centers −4.5,−3.5 → indices 5,6.
        let MaterialRegionShape::VoxelBox { lower, upper } = &assignment.regions[1].shape else {
            panic!("expected voxel box");
        };
        assert_eq!((*lower, *upper), ([0, 0, 5], [19, 19, 6]));
        // Aperture: −3..0 cm → an annulus voxel set, never a box.
        assert!(matches!(
            assignment.regions[2].shape,
            MaterialRegionShape::VoxelSet { .. }
        ));
    }

    #[test]
    fn sweep_enumerates_cartesian_product() {
        let base = assembly();
        let sweep = BsaSweep {
            schema_version: BSA_SWEEP_SCHEMA.into(),
            id: "sweep.test".into(),
            base: ContentReference {
                id: "base".into(),
                sha256: "d".repeat(64),
            },
            parameters: vec![
                BsaSweepParameter {
                    layer: "moderator".into(),
                    thickness_cm: vec![4.0, 5.0, 6.0],
                },
                BsaSweepParameter {
                    layer: "filter".into(),
                    thickness_cm: vec![1.0, 2.0],
                },
            ],
        };
        let variants = enumerate_bsa_sweep(&sweep, &base).unwrap();
        assert_eq!(variants.len(), 6);
        assert_eq!(variants[0].id, "openbnct.bsa.test.v1.s0");
        assert!((variants[0].layers[0].thickness_cm - 4.0).abs() < 1e-12);
        assert!((variants[5].layers[0].thickness_cm - 6.0).abs() < 1e-12);
        assert!((variants[5].layers[1].thickness_cm - 2.0).abs() < 1e-12);
        for variant in &variants {
            variant.validate().unwrap();
        }
    }

    #[test]
    fn sweep_rejects_unknown_layer_and_empty_values() {
        let base = assembly();
        let mut sweep = BsaSweep {
            schema_version: BSA_SWEEP_SCHEMA.into(),
            id: "sweep.bad".into(),
            base: ContentReference {
                id: "base".into(),
                sha256: "d".repeat(64),
            },
            parameters: vec![BsaSweepParameter {
                layer: "nonexistent".into(),
                thickness_cm: vec![1.0],
            }],
        };
        assert!(matches!(
            enumerate_bsa_sweep(&sweep, &base).unwrap_err(),
            BsaError::UnknownLayer(_)
        ));
        sweep.parameters[0].layer = "moderator".into();
        sweep.parameters[0].thickness_cm = vec![];
        assert!(matches!(
            enumerate_bsa_sweep(&sweep, &base).unwrap_err(),
            BsaError::InvalidSweepValues(_)
        ));
    }

    #[test]
    fn malformed_layers_reject() {
        let mut bsa = assembly();
        bsa.layers[0].thickness_cm = 0.0;
        assert!(bsa.validate().is_err());
        let mut bsa = assembly();
        bsa.layers[0].radial = BsaRadialExtent::Disk { radius_cm: 15.0 };
        assert!(bsa.validate().is_err());
        let mut bsa = assembly();
        bsa.direction_sign = 0;
        assert!(bsa.validate().is_err());
    }
}
