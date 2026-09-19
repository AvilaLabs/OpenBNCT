//! HU-to-material calibration: map a CT Hounsfield-unit volume onto a
//! `MaterialAssignment` of declared anchor materials.
//!
//! The calibration is a versioned artifact (`openbnct.hu-calibration`)
//! carrying ordered anchors — a material declared at each calibration
//! Hounsfield unit. A voxel's HU locates it inside a segment between two
//! anchors; its composition is the two-component volume mixture
//! `(1 − t)·anchor_i + t·anchor_{i+1}` with `t` the fractional position in
//! the segment — the stoichiometric-calibration structure (Schneider,
//! Bortfeld & Schlegel, *Phys. Med. Biol.* 45 (2000) 459) parameterized
//! by declared anchors rather than a hard-coded fit, since the
//! calibration is scanner- and protocol-dependent in real use.
//!
//! The emitted assignment expresses every voxel as explicit anchor
//! fractions summing to one (`voxel_fractions` regions with a zero
//! remainder), so the base material never contributes — the solver's
//! macroscopic-table blend does the two-component mixing. Anchor
//! compositions are user-declared `MaterialDefinition`s: the artifact is
//! the calibration, and its content hash binds the exact numbers used.

use openbnct_core::{ContentReference, GridGeometry};
use serde::{Deserialize, Serialize};

use crate::model::{
    MATERIAL_ASSIGNMENT_SCHEMA, MaterialAssignment, MaterialDefinition, MaterialRegion,
    MaterialRegionShape, TransportModelError,
};

/// Schema contract for the HU calibration artifact.
pub const HU_CALIBRATION_SCHEMA: &str = "openbnct.hu-calibration/0.1.0";

/// One calibration anchor: a material declared representative of
/// Hounsfield unit `hu`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuAnchor {
    /// The HU value this anchor's composition applies at. Anchors are
    /// ordered ascending and must be strictly increasing.
    pub hu: f64,
    /// The material composition at this HU — a full
    /// `MaterialDefinition` (density, nuclides, thermal treatment).
    pub material: MaterialDefinition,
}

/// A versioned HU→material calibration: an ordered anchor list
/// spanning the HU range the deployment expects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuCalibration {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Ascending anchors — at least two, strictly increasing `hu`.
    pub anchors: Vec<HuAnchor>,
    /// Evidence reference for where the calibration came from.
    pub derivation: Option<ContentReference>,
    /// Free-text description of the calibration's validity domain
    /// (scanner, kVp, reconstruction kernel, tissue coverage).
    pub validity_domain: Option<String>,
    pub provenance_id: String,
}

/// Per-anchor voxel coverage summary for the emitted assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuCalibrationReport {
    /// One entry per anchor, in anchor order: `(anchor_hu, material_id,
    /// voxels carrying a nonzero fraction of that anchor)`.
    pub anchor_coverage: Vec<HuAnchorCoverage>,
    /// Voxels clamped below the lowest anchor's HU.
    pub clamped_low: usize,
    /// Voxels clamped above the highest anchor's HU.
    pub clamped_high: usize,
    /// Voxels inside a segment (partial mixtures) vs. exactly on an
    /// anchor.
    pub interpolated: usize,
    /// HU bounds observed in the input volume.
    pub hu_range: [f64; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HuAnchorCoverage {
    pub hu: f64,
    pub material_id: String,
    /// Voxels with a nonzero fraction of this anchor.
    pub voxels: usize,
    /// Sum of this anchor's volume fractions across the volume — the
    /// effective number of voxels of this material.
    pub volume_fraction_total: f64,
}

impl HuCalibration {
    /// Structural validation — schema, identifiers, anchor ordering,
    /// material definitions.
    pub fn validate(&self) -> Result<(), TransportModelError> {
        if !openbnct_core::schema_matches(&self.schema_version, HU_CALIBRATION_SCHEMA) {
            return Err(TransportModelError::UnsupportedHuCalibrationSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(TransportModelError::MalformedHuCalibration(
                "hu_calibration.id is empty".into(),
            ));
        }
        if self.provenance_id.trim().is_empty() {
            return Err(TransportModelError::MalformedHuCalibration(
                "hu_calibration.provenance_id is empty".into(),
            ));
        }
        if self.anchors.len() < 2 {
            return Err(TransportModelError::MalformedHuCalibration(
                "hu_calibration needs at least two anchors".into(),
            ));
        }
        let mut previous = f64::NEG_INFINITY;
        let mut seen_ids = std::collections::BTreeSet::new();
        for anchor in &self.anchors {
            if !anchor.hu.is_finite() || anchor.hu <= previous {
                return Err(TransportModelError::MalformedHuCalibration(
                    "hu_calibration anchors must be finite and strictly increasing".into(),
                ));
            }
            previous = anchor.hu;
            anchor.material.validate()?;
            if !seen_ids.insert(anchor.material.id.as_str()) {
                return Err(TransportModelError::MalformedHuCalibration(format!(
                    "hu_calibration anchors repeat material id {:?}",
                    anchor.material.id
                )));
            }
        }
        Ok(())
    }

    /// Locate the segment containing `hu`: `Ok((i, t))` with `t` the
    /// fractional position in `[anchors[i].hu, anchors[i+1].hu)`;
    /// `Err(index)` clamps to that anchor strictly outside the range —
    /// a voxel exactly on an anchor is a pure `t = 0`/`t = 1` hit.
    fn segment(&self, hu: f64) -> Result<(usize, f64), usize> {
        let anchors = &self.anchors;
        if hu < anchors[0].hu {
            return Err(0);
        }
        if hu > anchors[anchors.len() - 1].hu {
            return Err(anchors.len() - 1);
        }
        // Linear scan over a handful of anchors.
        for i in 0..anchors.len() - 1 {
            let lo = anchors[i].hu;
            let hi = anchors[i + 1].hu;
            if hu < hi {
                return Ok((i, (hu - lo) / (hi - lo)));
            }
        }
        // hu == top anchor — a pure t = 1 hit on the last segment.
        Ok((anchors.len() - 2, 1.0))
    }

    /// Apply the calibration to an HU volume on `geometry`, producing
    /// the material assignment plus a coverage report. `hu_values` is
    /// voxel-major in the case grid convention (`i + nx·j + nx·ny·k`,
    /// the same ordering `RegionMask.voxels` uses).
    ///
    /// Every voxel carries explicit anchor fractions summing to one, so
    /// the assignment's base material — the lowest anchor — contributes
    /// nothing; it exists because the contract requires a base.
    pub fn apply(
        &self,
        hu_values: &[f64],
        geometry: &GridGeometry,
        case_id: &str,
        provenance_id: &str,
    ) -> Result<(MaterialAssignment, HuCalibrationReport), TransportModelError> {
        self.validate()?;
        let n_cells = geometry.voxel_count()?;
        if hu_values.len() != n_cells {
            return Err(TransportModelError::MalformedHuCalibration(format!(
                "hu volume has {} voxels; the grid has {n_cells}",
                hu_values.len()
            )));
        }
        if hu_values.iter().any(|v| !v.is_finite()) {
            return Err(TransportModelError::MalformedHuCalibration(
                "hu volume contains a non-finite value".into(),
            ));
        }
        let [nx, ny, _nz] = geometry.shape;
        let n_anchors = self.anchors.len();
        let mut indices: Vec<Vec<[u32; 3]>> = vec![Vec::new(); n_anchors];
        let mut fractions: Vec<Vec<f64>> = vec![Vec::new(); n_anchors];
        let mut totals = vec![0.0_f64; n_anchors];
        let mut coverage = vec![0_usize; n_anchors];
        let mut clamped_low = 0;
        let mut clamped_high = 0;
        let mut interpolated = 0;
        let mut hu_range = [f64::INFINITY, f64::NEG_INFINITY];

        for (flat, &hu) in hu_values.iter().enumerate() {
            hu_range[0] = hu_range[0].min(hu);
            hu_range[1] = hu_range[1].max(hu);
            let i = (flat % nx as usize) as u32;
            let j = (flat / nx as usize % ny as usize) as u32;
            let k = (flat / (nx as usize * ny as usize)) as u32;
            let voxel = [i, j, k];
            match self.segment(hu) {
                Ok((a, t)) => {
                    if t > 0.0 && t < 1.0 {
                        interpolated += 1;
                    }
                    if t > 0.0 {
                        indices[a + 1].push(voxel);
                        fractions[a + 1].push(t);
                        totals[a + 1] += t;
                        coverage[a + 1] += 1;
                    }
                    if t < 1.0 {
                        indices[a].push(voxel);
                        fractions[a].push(1.0 - t);
                        totals[a] += 1.0 - t;
                        coverage[a] += 1;
                    }
                }
                Err(a) => {
                    if a == 0 {
                        clamped_low += 1;
                    } else {
                        clamped_high += 1;
                    }
                    indices[a].push(voxel);
                    fractions[a].push(1.0);
                    totals[a] += 1.0;
                    coverage[a] += 1;
                }
            }
        }

        let mut regions = Vec::with_capacity(n_anchors);
        let mut anchor_coverage = Vec::with_capacity(n_anchors);
        for (a, anchor) in self.anchors.iter().enumerate() {
            anchor_coverage.push(HuAnchorCoverage {
                hu: anchor.hu,
                material_id: anchor.material.id.clone(),
                voxels: coverage[a],
                volume_fraction_total: totals[a],
            });
            if indices[a].is_empty() {
                continue;
            }
            regions.push(MaterialRegion {
                name: format!("hu-{}", anchor.material.id),
                material: anchor.material.clone(),
                shape: MaterialRegionShape::VoxelFractions {
                    indices: std::mem::take(&mut indices[a]),
                    fractions: std::mem::take(&mut fractions[a]),
                },
            });
        }
        if regions.is_empty() {
            return Err(TransportModelError::MalformedHuCalibration(
                "hu volume assigned no material to any voxel".into(),
            ));
        }
        let assignment = MaterialAssignment {
            schema_version: MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: case_id.to_string(),
            base_material: self.anchors[0].material.clone(),
            regions,
            provenance_id: provenance_id.to_string(),
        };
        let report = HuCalibrationReport {
            anchor_coverage,
            clamped_low,
            clamped_high,
            interpolated,
            hu_range,
        };
        Ok((assignment, report))
    }

    /// The distinct anchor materials, for the collapse pipeline's
    /// `--material` inputs — every region material must appear in the
    /// collapsed data.
    #[must_use]
    pub fn materials(&self) -> Vec<&MaterialDefinition> {
        self.anchors.iter().map(|a| &a.material).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{NeutronThermalTreatment, NuclideMassFraction};

    fn material(id: &str, density: f64) -> MaterialDefinition {
        MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: id.into(),
            density_g_cm3: density,
            temperature_k: 293.6,
            nuclides: vec![
                NuclideMassFraction {
                    name: "H1".into(),
                    mass_fraction: 0.1,
                },
                NuclideMassFraction {
                    name: "O16".into(),
                    mass_fraction: 0.9,
                },
            ],
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        }
    }

    fn calibration() -> HuCalibration {
        HuCalibration {
            schema_version: HU_CALIBRATION_SCHEMA.into(),
            id: "test.hu-calibration.v1".into(),
            anchors: vec![
                HuAnchor {
                    hu: -1000.0,
                    material: material("air", 0.0012),
                },
                HuAnchor {
                    hu: 0.0,
                    material: material("water", 1.0),
                },
                HuAnchor {
                    hu: 1000.0,
                    material: material("bone", 1.9),
                },
            ],
            derivation: None,
            validity_domain: Some("unit test".into()),
            provenance_id: "test".into(),
        }
    }

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [2, 2, 1],
            spacing_mm: [10.0; 3],
            origin_mm: [-5.0, -5.0, -5.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn midpoint_voxel_splits_between_anchors() {
        // HU -500 is the midpoint of air(-1000)/water(0): half of each.
        let (assignment, report) = calibration()
            .apply(
                &[-500.0, -500.0, -500.0, -500.0],
                &geometry(),
                "case.v1",
                "test",
            )
            .unwrap();
        assert_eq!(report.interpolated, 4);
        let air = assignment
            .regions
            .iter()
            .find(|r| r.material.id == "air")
            .unwrap();
        let water = assignment
            .regions
            .iter()
            .find(|r| r.material.id == "water")
            .unwrap();
        match (&air.shape, &water.shape) {
            (
                MaterialRegionShape::VoxelFractions {
                    indices: ai,
                    fractions: af,
                },
                MaterialRegionShape::VoxelFractions {
                    indices: wi,
                    fractions: wf,
                },
            ) => {
                assert_eq!(ai.len(), 4);
                assert!(af.iter().all(|&f| (f - 0.5).abs() < 1e-12));
                assert_eq!(wi.len(), 4);
                assert!(wf.iter().all(|&f| (f - 0.5).abs() < 1e-12));
            }
            _ => panic!("expected voxel_fractions regions"),
        }
        assignment.validate(&geometry()).unwrap();
    }

    #[test]
    fn clamps_outside_the_anchor_range() {
        let (assignment, report) = calibration()
            .apply(
                &[-2000.0, 0.0, 500.0, 3000.0],
                &geometry(),
                "case.v1",
                "test",
            )
            .unwrap();
        assert_eq!(report.clamped_low, 1);
        assert_eq!(report.clamped_high, 1);
        // -2000 → air, 0 → water exactly (segment boundary, t=0 →
        // pure water), 500 → 50/50 water/bone, 3000 → bone.
        let coverage = |id: &str| {
            report
                .anchor_coverage
                .iter()
                .find(|c| c.material_id == id)
                .unwrap()
                .volume_fraction_total
        };
        assert_eq!(coverage("air"), 1.0);
        assert_eq!(coverage("water"), 1.5);
        assert_eq!(coverage("bone"), 1.5);
        let _ = assignment;
    }

    #[test]
    fn rejects_unsorted_and_duplicate_anchors() {
        let mut bad = calibration();
        bad.anchors.swap(0, 1);
        assert!(bad.validate().is_err());
        let mut dup = calibration();
        dup.anchors[1].material.id = "air".into();
        assert!(dup.validate().is_err());
    }

    #[test]
    fn rejects_length_mismatch_and_nonfinite() {
        let cal = calibration();
        assert!(cal.apply(&[0.0; 3], &geometry(), "c", "p").is_err());
        assert!(
            cal.apply(&[0.0, f64::NAN, 0.0, 0.0], &geometry(), "c", "p")
                .is_err()
        );
    }

    /// Dominant material per voxel from an assignment's region
    /// fractions (remainder → base material).
    fn dominant_materials(assignment: &MaterialAssignment, geometry: &GridGeometry) -> Vec<String> {
        let n = geometry.voxel_count().unwrap();
        let [nx, ny, _] = geometry.shape.map(|d| d as usize);
        let mut fractions: Vec<std::collections::BTreeMap<String, f64>> =
            vec![std::collections::BTreeMap::new(); n];
        for region in &assignment.regions {
            if let MaterialRegionShape::VoxelFractions {
                indices,
                fractions: fs,
            } = &region.shape
            {
                for (vox, &f) in indices.iter().zip(fs.iter()) {
                    let flat = vox[0] as usize + nx * vox[1] as usize + nx * ny * vox[2] as usize;
                    *fractions[flat]
                        .entry(region.material.id.clone())
                        .or_insert(0.0) += f;
                }
            }
        }
        fractions
            .iter()
            .map(|cell| {
                let total: f64 = cell.values().sum();
                let mut best = ((1.0 - total).max(0.0), assignment.base_material.id.clone());
                for (id, f) in cell {
                    if *f > best.0 {
                        best = (*f, id.clone());
                    }
                }
                best.1
            })
            .collect()
    }

    /// Ground-truth dominant material per voxel of a `voxel_set`
    /// assignment.
    fn truth_materials(
        assignment: &MaterialAssignment,
        n: usize,
        nx: usize,
        ny: usize,
    ) -> Vec<String> {
        let mut truth = vec![assignment.base_material.id.clone(); n];
        for region in &assignment.regions {
            if let MaterialRegionShape::VoxelSet { indices } = &region.shape {
                for vox in indices {
                    truth[vox[0] as usize + nx * vox[1] as usize + nx * ny * vox[2] as usize] =
                        region.material.id.clone();
                }
            }
        }
        truth
    }

    /// Round-trip on the layered-head phantom: synthesize an HU volume
    /// from the committed assignment (each layer at its anchor's HU),
    /// calibrate, and require voxel-exact dominant-material recovery.
    /// Off-anchor HU lands on mixtures — interpolation, not failure.
    #[test]
    fn layered_head_round_trip_recovers_materials() {
        let root = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../benchmarks/synthetic/layered-head-phantom"
        );
        let case: crate::model::TransportCase =
            serde_json::from_str(&std::fs::read_to_string(format!("{root}/case.json")).unwrap())
                .unwrap();
        let truth_assignment: MaterialAssignment = serde_json::from_str(
            &std::fs::read_to_string(format!("{root}/assignment.json")).unwrap(),
        )
        .unwrap();
        let cal: HuCalibration = serde_json::from_str(
            &std::fs::read_to_string(format!("{root}/materials/hu-calibration.json")).unwrap(),
        )
        .unwrap();

        let n = case.geometry.voxel_count().unwrap();
        let [nx, ny, _] = case.geometry.shape.map(|d| d as usize);
        let truth = truth_materials(&truth_assignment, n, nx, ny);

        // Synthesize HU: each voxel at its true material's anchor HU.
        let anchor_hu: std::collections::BTreeMap<String, f64> = cal
            .anchors
            .iter()
            .map(|a| (a.material.id.clone(), a.hu))
            .collect();
        let hu: Vec<f64> = truth.iter().map(|id| anchor_hu[id]).collect();

        let (assignment, report) = cal
            .apply(&hu, &case.geometry, &case.case_id, "test")
            .unwrap();
        assignment.validate(&case.geometry).unwrap();
        let recovered = dominant_materials(&assignment, &case.geometry);

        // Every voxel's dominant material matches the ground truth —
        // HU synthesized on anchors recovers exactly.
        let mismatches = truth.iter().zip(&recovered).filter(|(t, r)| t != r).count();
        assert_eq!(
            mismatches, 0,
            "anchor-HU synthesis must recover the declared assignment exactly"
        );
        // Material volumes are recovered to the voxel.
        for coverage in &report.anchor_coverage {
            let expected = truth
                .iter()
                .filter(|id| **id == coverage.material_id)
                .count();
            assert!(
                (coverage.volume_fraction_total - expected as f64).abs() < 1e-9,
                "{}: recovered {} vs truth {}",
                coverage.material_id,
                coverage.volume_fraction_total,
                expected
            );
        }

        // An off-anchor volume interpolates: skull voxels at HU 600 sit
        // inside the skin(60)→skull(900) segment as 64% skull mixtures.
        let hu_mixed: Vec<f64> = truth
            .iter()
            .map(|id| {
                if *id == "openbnct.layered-head.skull.v1" {
                    600.0
                } else {
                    anchor_hu[id]
                }
            })
            .collect();
        let (mixed_assignment, mixed_report) = cal
            .apply(&hu_mixed, &case.geometry, &case.case_id, "test")
            .unwrap();
        let skull_cov = mixed_report
            .anchor_coverage
            .iter()
            .find(|c| c.material_id == "openbnct.layered-head.skull.v1")
            .unwrap();
        let skull_voxels = truth
            .iter()
            .filter(|id| **id == "openbnct.layered-head.skull.v1")
            .count();
        let expected_t = (600.0 - 60.0) / (900.0 - 60.0);
        assert!(
            (skull_cov.volume_fraction_total - expected_t * skull_voxels as f64).abs()
                < 1e-6 * skull_voxels as f64
        );
        // Dominant material still recovers everywhere (64% > 36%).
        let recovered_mixed = dominant_materials(&mixed_assignment, &case.geometry);
        assert_eq!(
            truth
                .iter()
                .zip(&recovered_mixed)
                .filter(|(t, r)| t != r)
                .count(),
            0
        );
    }
}
