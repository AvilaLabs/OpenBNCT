// SPDX-License-Identifier: MIT

//! Voxel material realization for the unit-mass-fraction OpenMC profile.
//!
//! A [`MaterialAssignment`] describes each voxel as a base material, a full
//! region material, or a volume-fraction mixture of several. OpenMC needs one
//! concrete material per lattice element, and the per-voxel dose fold needs
//! that material's real composition. This module realizes every voxel by
//! quantizing its volume fractions to a declared number of levels
//! (largest-remainder rounding, ties to the lower component index, so the
//! quantized fractions always sum to exactly one) and deduplicating the
//! results: each distinct quantized mixture becomes one material.
//!
//! The realization is a pure function of the assignment and the level count,
//! so deck generation and collection recompute the identical mapping.

use std::collections::BTreeSet;

use openbnct_transport::{
    MaterialAssignment, MaterialDefinition, MaterialRegionShape, NuclideMassFraction,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default quantization levels per anchor pair.
pub const DEFAULT_MIXTURE_LEVELS: u32 = 20;
/// Largest accepted level count (keeps material counts bounded).
pub const MAX_MIXTURE_LEVELS: u32 = 1000;

#[derive(Debug, Error, PartialEq)]
pub enum RealizationError {
    #[error("mixture levels must be between 1 and {MAX_MIXTURE_LEVELS}, got {0}")]
    InvalidLevels(u32),
    #[error("component materials of a mixture must share one temperature ({0} K vs {1} K)")]
    MixedTemperatures(f64, f64),
    #[error("component materials of a mixture must share one thermal treatment")]
    MixedThermalTreatment,
    #[error("material {0} declares a boron microdistribution, which mixtures cannot carry")]
    MicrodistributionInMixture(String),
    #[error("voxel {0} carries fractions summing above one")]
    FractionOverflow(usize),
}

/// One distinct realized material and the voxels that use it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealizedMaterialRecord {
    /// OpenMC material id in the emitted deck (`materials.xml`).
    pub openmc_material_id: u32,
    pub material_id: String,
    pub density_g_cm3: f64,
    /// `[component name, level count]` pairs; a pure material has one pair
    /// at the full level count.
    pub components: Vec<(String, u32)>,
    pub voxel_count: u64,
}

/// The quantization record written into the input manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenMcMaterialRealization {
    pub rule: String,
    pub levels: u32,
    pub materials: Vec<RealizedMaterialRecord>,
    pub mixture_voxel_count: u64,
    /// Largest |quantized - declared| volume fraction over all voxels.
    pub max_fraction_error: f64,
}

/// Rule token recorded in manifests.
pub const REALIZATION_RULE: &str = "largest_remainder_volume_fraction_quantization";

/// Concrete materials and the per-voxel index into them.
#[derive(Debug, Clone)]
pub struct MaterialRealization {
    /// Index 0 is always the base material. Ids in the deck are index + 1.
    pub materials: Vec<MaterialDefinition>,
    /// Flat voxel order `i + nx*j + nx*ny*k`.
    pub voxel_material: Vec<usize>,
    pub record: OpenMcMaterialRealization,
}

impl MaterialRealization {
    /// Whether any voxel is a genuine mixture.
    #[must_use]
    pub fn has_mixtures(&self) -> bool {
        self.record.mixture_voxel_count > 0
    }

    /// Mass fraction of `nuclide` in the material realized at flat `voxel`.
    #[must_use]
    pub fn voxel_mass_fraction(&self, voxel: usize, nuclide: &str) -> f64 {
        self.materials[self.voxel_material[voxel]]
            .nuclides
            .iter()
            .find(|n| n.name == nuclide)
            .map_or(0.0, |n| n.mass_fraction)
    }

    /// Density (g/cm3) of the material realized at flat `voxel`.
    #[must_use]
    pub fn voxel_density_g_cm3(&self, voxel: usize) -> f64 {
        self.materials[self.voxel_material[voxel]].density_g_cm3
    }
}

/// Whether an assignment carries any partial-cell mixture region.
#[must_use]
pub fn assignment_has_fraction_regions(assignment: &MaterialAssignment) -> bool {
    assignment
        .regions
        .iter()
        .any(|region| matches!(region.shape, MaterialRegionShape::VoxelFractions { .. }))
}

type Key = Vec<(usize, u32)>;

/// Largest-remainder quantization shared with the deterministic blend.
fn quantize(fractions: &[f64], levels: u32) -> Vec<u32> {
    openbnct_transport::quantize_fractions(fractions, levels)
}

fn blend(
    components: &[&MaterialDefinition],
    key: &Key,
    levels: u32,
) -> Result<MaterialDefinition, RealizationError> {
    let first = components[key[0].0];
    let mut density = 0.0;
    let mut mass: Vec<(String, f64)> = Vec::new();
    for &(component, count) in key {
        let material = components[component];
        if material.temperature_k != first.temperature_k {
            return Err(RealizationError::MixedTemperatures(
                first.temperature_k,
                material.temperature_k,
            ));
        }
        if material.neutron_thermal_treatment != first.neutron_thermal_treatment {
            return Err(RealizationError::MixedThermalTreatment);
        }
        if material.boron_microdistribution.is_some() {
            return Err(RealizationError::MicrodistributionInMixture(
                material.id.clone(),
            ));
        }
        let volume = f64::from(count) / f64::from(levels);
        density += volume * material.density_g_cm3;
        for nuclide in &material.nuclides {
            let partial = volume * material.density_g_cm3 * nuclide.mass_fraction;
            match mass.iter_mut().find(|(name, _)| *name == nuclide.name) {
                Some((_, total)) => *total += partial,
                None => mass.push((nuclide.name.clone(), partial)),
            }
        }
    }
    let id = format!(
        "openbnct.mixture.q{levels}.{}",
        key.iter()
            .map(|&(component, count)| format!("{}x{count}", components[component].id))
            .collect::<Vec<_>>()
            .join("+")
    );
    Ok(MaterialDefinition {
        schema_version: first.schema_version.clone(),
        id,
        density_g_cm3: density,
        temperature_k: first.temperature_k,
        nuclides: mass
            .into_iter()
            .map(|(name, total)| NuclideMassFraction {
                name,
                mass_fraction: total / density,
            })
            .collect(),
        neutron_thermal_treatment: first.neutron_thermal_treatment,
        boron_microdistribution: None,
    })
}

/// Realize every voxel of `assignment` on a grid of `shape`.
///
/// Component 0 is the base material; component `r + 1` is `regions[r]`.
/// Full regions (`VoxelBox`, `VoxelSet`) own their voxels outright;
/// `VoxelFractions` regions contribute their fraction of volume and the
/// base material takes the remainder.
pub fn realize(
    assignment: &MaterialAssignment,
    shape: [u32; 3],
    levels: u32,
) -> Result<MaterialRealization, RealizationError> {
    if levels == 0 || levels > MAX_MIXTURE_LEVELS {
        return Err(RealizationError::InvalidLevels(levels));
    }
    let [nx, ny, nz] = shape.map(|d| d as usize);
    let voxel_count = nx * ny * nz;
    let mut components: Vec<&MaterialDefinition> = vec![&assignment.base_material];
    components.extend(assignment.regions.iter().map(|region| &region.material));

    // Sparse per-voxel contributions: (component, fraction).
    let mut contributions: Vec<Vec<(usize, f64)>> = vec![Vec::new(); voxel_count];
    let flat = |v: [u32; 3]| v[0] as usize + nx * v[1] as usize + nx * ny * v[2] as usize;
    for (index, region) in assignment.regions.iter().enumerate() {
        match &region.shape {
            MaterialRegionShape::VoxelFractions { indices, fractions } => {
                for (voxel, fraction) in indices.iter().zip(fractions) {
                    contributions[flat(*voxel)].push((index + 1, *fraction));
                }
            }
            _ => region.for_each_voxel(|voxel| {
                contributions[flat(voxel)] = vec![(index + 1, 1.0)];
            }),
        }
    }

    let mut voxel_keys: Vec<Key> = Vec::with_capacity(voxel_count);
    let mut max_error = 0.0_f64;
    let mut mixture_voxels = 0_u64;
    for (voxel, entries) in contributions.iter().enumerate() {
        let total: f64 = entries.iter().map(|(_, f)| f).sum();
        if total > 1.0 + 1.0e-9 {
            return Err(RealizationError::FractionOverflow(voxel));
        }
        let mut fractions = vec![0.0_f64; components.len()];
        for &(component, fraction) in entries {
            fractions[component] += fraction;
        }
        fractions[0] = (1.0 - total).max(0.0);
        let counts = quantize(&fractions, levels);
        for (fraction, count) in fractions.iter().zip(&counts) {
            max_error = max_error.max((fraction - f64::from(*count) / f64::from(levels)).abs());
        }
        let key: Key = counts
            .iter()
            .enumerate()
            .filter(|&(_, &count)| count > 0)
            .map(|(component, &count)| (component, count))
            .collect();
        if key.len() > 1 {
            mixture_voxels += 1;
        }
        voxel_keys.push(key);
    }

    // Deterministic material order: base, other pure materials by component
    // index, then mixtures by key.
    let used: BTreeSet<&Key> = voxel_keys.iter().collect();
    let pure = |key: &Key| key.len() == 1 && key[0].1 == levels;
    let mut ordered: Vec<Key> = vec![vec![(0, levels)]];
    ordered.extend(
        used.iter()
            .filter(|key| pure(key) && key[0].0 != 0)
            .map(|key| (*key).clone()),
    );
    ordered.extend(
        used.iter()
            .filter(|key| !pure(key))
            .map(|key| (*key).clone()),
    );

    let mut materials = Vec::with_capacity(ordered.len());
    for key in &ordered {
        if pure(key) {
            materials.push(components[key[0].0].clone());
        } else {
            materials.push(blend(&components, key, levels)?);
        }
    }
    let index_of: std::collections::BTreeMap<&Key, usize> = ordered
        .iter()
        .enumerate()
        .map(|(index, key)| (key, index))
        .collect();
    let mut voxel_material = Vec::with_capacity(voxel_count);
    let mut voxel_counts = vec![0_u64; ordered.len()];
    for key in &voxel_keys {
        let index = index_of[key];
        voxel_material.push(index);
        voxel_counts[index] += 1;
    }

    let records = ordered
        .iter()
        .zip(&materials)
        .enumerate()
        .map(|(index, (key, material))| RealizedMaterialRecord {
            openmc_material_id: index as u32 + 1,
            material_id: material.id.clone(),
            density_g_cm3: material.density_g_cm3,
            components: key
                .iter()
                .map(|&(component, count)| (components[component].id.clone(), count))
                .collect(),
            voxel_count: voxel_counts[index],
        })
        .collect();
    Ok(MaterialRealization {
        materials,
        voxel_material,
        record: OpenMcMaterialRealization {
            rule: REALIZATION_RULE.into(),
            levels,
            materials: records,
            mixture_voxel_count: mixture_voxels,
            max_fraction_error: max_error,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_transport::{MaterialRegion, NeutronThermalTreatment};

    fn material(id: &str, density: f64, nuclides: &[(&str, f64)]) -> MaterialDefinition {
        MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: id.into(),
            density_g_cm3: density,
            temperature_k: 293.6,
            nuclides: nuclides
                .iter()
                .map(|(name, mass_fraction)| NuclideMassFraction {
                    name: (*name).into(),
                    mass_fraction: *mass_fraction,
                })
                .collect(),
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        }
    }

    fn assignment(regions: Vec<MaterialRegion>) -> MaterialAssignment {
        MaterialAssignment {
            schema_version: "openbnct.material-assignment/0.2.0".into(),
            case_id: "case".into(),
            base_material: material("air", 0.001, &[("H1", 1.0)]),
            regions,
            provenance_id: "test".into(),
        }
    }

    #[test]
    fn quantization_sums_to_levels_and_prefers_low_index_on_ties() {
        assert_eq!(quantize(&[0.5, 0.5], 1), vec![1, 0]);
        assert_eq!(quantize(&[0.25, 0.75], 20), vec![5, 15]);
        assert_eq!(quantize(&[1.0 / 3.0; 3], 20).iter().sum::<u32>(), 20);
        assert_eq!(quantize(&[0.0, 1.0], 20), vec![0, 20]);
        // 0.33 * 20 = 6.6 -> 7 by largest remainder, base takes 13.
        assert_eq!(quantize(&[0.67, 0.33], 20), vec![13, 7]);
    }

    #[test]
    fn pure_and_mixed_voxels_map_to_deduplicated_materials() {
        let tissue = material("tissue", 1.0, &[("H1", 0.1), ("O16", 0.9)]);
        let bone = material("bone", 2.0, &[("O16", 0.5), ("Ca40", 0.5)]);
        let regions = vec![
            MaterialRegion {
                name: "tissue".into(),
                material: tissue,
                shape: MaterialRegionShape::VoxelFractions {
                    indices: vec![[0, 0, 0], [1, 0, 0], [2, 0, 0]],
                    fractions: vec![0.50, 0.51, 1.0],
                },
            },
            MaterialRegion {
                name: "bone".into(),
                material: bone,
                shape: MaterialRegionShape::VoxelFractions {
                    indices: vec![[0, 0, 0], [1, 0, 0]],
                    fractions: vec![0.5, 0.49],
                },
            },
        ];
        let realization = realize(&assignment(regions), [4, 1, 1], 20).unwrap();
        // Voxels 0 and 1 quantize to the same 10/10 tissue/bone mixture.
        assert_eq!(realization.voxel_material[0], realization.voxel_material[1]);
        assert!(realization.has_mixtures());
        assert_eq!(realization.record.mixture_voxel_count, 2);
        // Voxel 2 is pure tissue, voxel 3 pure base (air).
        assert_eq!(realization.materials[0].id, "air");
        assert_eq!(realization.voxel_material[3], 0);
        assert_eq!(
            realization.materials[realization.voxel_material[2]].id,
            "tissue"
        );
        // Mixture: 10/20 tissue (1.0) + 10/20 bone (2.0) -> density 1.5,
        // O16 mass = 0.5*1.0*0.9 + 0.5*2.0*0.5 = 0.95 -> fraction 0.95/1.5.
        let mixed = &realization.materials[realization.voxel_material[0]];
        assert!((mixed.density_g_cm3 - 1.5).abs() < 1e-12);
        let sum: f64 = mixed.nuclides.iter().map(|n| n.mass_fraction).sum();
        assert!((sum - 1.0).abs() < 1e-12);
        assert!((realization.voxel_mass_fraction(0, "O16") - 0.95 / 1.5).abs() < 1e-12);
        assert_eq!(realization.voxel_mass_fraction(0, "B10"), 0.0);
        // Quantization error stays within half a level.
        assert!(realization.record.max_fraction_error <= 0.5 / 20.0 + 1e-12);
        // Materials are counted and ids are dense.
        let total: u64 = realization
            .record
            .materials
            .iter()
            .map(|m| m.voxel_count)
            .sum();
        assert_eq!(total, 4);
        assert_eq!(realization.record.materials[0].openmc_material_id, 1);
    }

    #[test]
    fn realization_is_deterministic_and_rejects_bad_inputs() {
        let tissue = material("tissue", 1.0, &[("H1", 1.0)]);
        let regions = vec![MaterialRegion {
            name: "t".into(),
            material: tissue,
            shape: MaterialRegionShape::VoxelFractions {
                indices: vec![[0, 0, 0]],
                fractions: vec![0.37],
            },
        }];
        let a = realize(&assignment(regions.clone()), [2, 1, 1], 20).unwrap();
        let b = realize(&assignment(regions.clone()), [2, 1, 1], 20).unwrap();
        assert_eq!(a.record, b.record);
        assert_eq!(
            realize(&assignment(regions.clone()), [2, 1, 1], 0).unwrap_err(),
            RealizationError::InvalidLevels(0)
        );
        let mut hot = regions;
        hot[0].material.temperature_k = 600.0;
        assert!(matches!(
            realize(&assignment(hot), [2, 1, 1], 20),
            Err(RealizationError::MixedTemperatures(..))
        ));
    }

    #[test]
    fn hu_calibrated_layered_head_realizes_to_the_ground_truth_materials() {
        // The committed HU round trip (every voxel an exact anchor hit,
        // expressed as unit-fraction mixtures) must realize voxel-for-voxel
        // to the same materials as the phantom's own assignment.
        const TRUTH: &str =
            include_str!("../../../benchmarks/synthetic/layered-head-phantom/assignment.json");
        const CALIBRATED: &str = include_str!(
            "../../../benchmarks/synthetic/layered-head-phantom/planning/hu-demo/assignment-calibrated.json"
        );
        let truth: MaterialAssignment = serde_json::from_str(TRUTH).unwrap();
        let calibrated: MaterialAssignment = serde_json::from_str(CALIBRATED).unwrap();
        let shape = [25, 25, 25];
        let a = realize(&truth, shape, 20).unwrap();
        let b = realize(&calibrated, shape, 20).unwrap();
        assert!(!a.has_mixtures() && !b.has_mixtures());
        for voxel in 0..a.voxel_material.len() {
            assert_eq!(
                a.materials[a.voxel_material[voxel]], b.materials[b.voxel_material[voxel]],
                "voxel {voxel}"
            );
        }
        assert_eq!(a.record.materials.len(), 4);
    }
}
