// SPDX-License-Identifier: MIT

//! Case → Avify voxel-plan export: per-voxel engine class map, ROI
//! masks, and the meta JSON the engine's `plan_model.py` consumes.
//!
//! Conventions: the engine's npz is z-major `(nz, ny, nx)`; its
//! `lower_left_cm_xyz` is the grid corner, while OpenBNCT's
//! `origin_mm` is the centre of voxel 0 — converted here. Engine voxels
//! are isotropic (`voxel_cm` scalar), so anisotropic cases reject.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openbnct_transport::{MaterialAssignment, MaterialRegionShape, TransportCase};
use serde::Serialize;

use crate::error::AvifyError;
use crate::npz::write_arrays_npz;
use crate::receipt::hex_sha256;
use crate::spec::{AvifySpec, EngineClass};

/// Result of a voxel-plan export.
#[derive(Debug)]
pub struct VoxelPlanExport {
    pub arrays_path: PathBuf,
    pub meta_path: PathBuf,
    /// SHA-256 of the written npz — content binding for the record.
    pub arrays_sha256: String,
    pub meta_sha256: String,
    /// Per-class voxel counts (export audit).
    pub class_voxels: BTreeMap<String, usize>,
}

/// Export the case + assignment + spec as an engine voxel plan under
/// `prefix` (writes `<prefix>_arrays.npz` and `<prefix>_meta.json`).
pub fn export_voxel_plan(
    case: &TransportCase,
    assignment: &MaterialAssignment,
    spec: &AvifySpec,
    prefix: &std::path::Path,
) -> Result<VoxelPlanExport, AvifyError> {
    spec.validate()?;
    let geo = &case.geometry;
    let [nx, ny, nz] = geo.shape.map(|d| d as usize);
    let n_cells = nx * ny * nz;

    // Engine voxels are isotropic.
    let sp = geo.spacing_mm;
    if (sp[0] - sp[1]).abs() > 1e-9 || (sp[1] - sp[2]).abs() > 1e-9 {
        return Err(AvifyError::Invalid(format!(
            "avify voxel plans require isotropic voxels; case spacing is {:?} mm",
            sp
        )));
    }
    let voxel_cm = sp[0] / 10.0;

    // Per-voxel dominant class. Candidates: each covering region with
    // weight 1.0 (box/set) or its declared fraction, plus the base
    // material with weight 1 − Σfractions. Equal maxima between distinct
    // claimants reject — ambiguous voxels must not be silently classed.
    let mut best_w: Vec<f64> = vec![0.0; n_cells];
    let mut frac_sum: Vec<f64> = vec![0.0; n_cells];
    let mut full_cover: Vec<bool> = vec![false; n_cells];
    let mut owner: Vec<Option<usize>> = vec![None; n_cells];
    let mut ambiguous: Vec<bool> = vec![false; n_cells];
    for (ri, region) in assignment.regions.iter().enumerate() {
        let mut claim = |v: [u32; 3], w: f64| {
            let cell = (v[0] + v[1] * nx as u32 + v[2] * nx as u32 * ny as u32) as usize;
            if cell >= n_cells {
                return;
            }
            if matches!(region.shape, MaterialRegionShape::VoxelFractions { .. }) {
                frac_sum[cell] += w;
            } else {
                full_cover[cell] = true;
            }
            if w > best_w[cell] {
                best_w[cell] = w;
                owner[cell] = Some(ri);
                ambiguous[cell] = false;
            } else if w == best_w[cell] && owner[cell] != Some(ri) {
                ambiguous[cell] = true;
            }
        };
        match &region.shape {
            MaterialRegionShape::VoxelFractions { indices, fractions } => {
                for (v, f) in indices.iter().zip(fractions.iter()) {
                    claim(*v, *f);
                }
            }
            _ => region.for_each_voxel(|v| claim(v, 1.0)),
        }
    }
    // Base material claims the remainder share of every voxel (zero
    // where a box/set region fully covers it).
    for cell in 0..n_cells {
        let base_w = if full_cover[cell] {
            0.0
        } else {
            (1.0 - frac_sum[cell]).max(0.0)
        };
        if best_w[cell] == 0.0 || base_w > best_w[cell] {
            owner[cell] = None;
            ambiguous[cell] = false;
        } else if base_w == best_w[cell] && owner[cell].is_some() {
            ambiguous[cell] = true;
        }
    }

    let mut cls_zyx = vec![0i8; n_cells];
    let mut class_voxels: BTreeMap<String, usize> = BTreeMap::new();
    for class in EngineClass::ALL {
        class_voxels.insert(class.name().to_string(), 0);
    }
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let cell = i + j * nx + k * nx * ny;
                let class = if ambiguous[cell] {
                    return Err(AvifyError::Invalid(format!(
                        "voxel [{i},{j},{k}] is claimed equally by two claimants — \
                         ambiguous class mapping"
                    )));
                } else {
                    match owner[cell] {
                        Some(ri) => {
                            let name = &assignment.regions[ri].name;
                            *spec.region_classes.get(name).ok_or_else(|| {
                                AvifyError::Invalid(format!(
                                    "region {name:?} has no engine class mapping in the spec"
                                ))
                            })?
                        }
                        None => spec.default_class,
                    }
                };
                let zi = k * ny * nx + j * nx + i; // zyx order: x fastest within (z,y,x)
                cls_zyx[zi] = class.index() as i8;
                *class_voxels.get_mut(class.name()).unwrap() += 1;
            }
        }
    }

    // ROI masks: v1 ROI = class membership (engine enforces
    // roi ⊆ cls==class anyway).
    let rois: Vec<(String, Vec<bool>)> = ["tumour", "brain", "scalp"]
        .iter()
        .map(|name| {
            let idx = EngineClass::ALL
                .iter()
                .find(|c| c.name() == *name)
                .unwrap()
                .index() as i8;
            (
                name.to_string(),
                cls_zyx.iter().map(|&c| c == idx).collect(),
            )
        })
        .collect();

    // The engine tallies all three ROIs unconditionally — a class with
    // zero voxels never enters the lattice, and its CellFilter aborts
    // OpenMC init. Reject before writing rather than crash the engine.
    for name in ["tumour", "brain", "scalp"] {
        if class_voxels[name] == 0 {
            return Err(AvifyError::Invalid(format!(
                "ROI class {name:?} has zero voxels — the engine requires all \
                 three ROI classes (tumour/brain/scalp) to be present"
            )));
        }
    }

    let arrays_path = write_arrays_npz(prefix, &cls_zyx, &rois, [nz, ny, nx])?;
    let arrays_sha256 = hex_sha256(&std::fs::read(&arrays_path)?);

    let lower_left_cm_xyz: Vec<f64> = (0..3)
        .map(|a| (geo.origin_mm[a] - 0.5 * geo.spacing_mm[a]) / 10.0)
        .collect();

    let roi_voxels: BTreeMap<String, usize> = rois
        .iter()
        .map(|(n, m)| (n.clone(), m.iter().filter(|&&b| b).count()))
        .collect();
    let roi_volume_cm3: BTreeMap<String, f64> = roi_voxels
        .iter()
        .map(|(n, c)| (n.clone(), *c as f64 * voxel_cm.powi(3)))
        .collect();

    #[derive(Serialize)]
    struct Meta<'a> {
        classes: [&'a str; 5],
        density_g_cm3: &'a BTreeMap<String, f64>,
        voxel_cm: f64,
        shape_zyx: [usize; 3],
        lower_left_cm_xyz: Vec<f64>,
        roi_voxels: BTreeMap<String, usize>,
        roi_volume_cm3: BTreeMap<String, f64>,
        class_voxels: BTreeMap<String, usize>,
        // OpenBNCT provenance — ignored by the engine, used for the record.
        openbnct_case_id: &'a str,
        openbnct_arrays_sha256: &'a str,
        openbnct_class_map: &'a BTreeMap<String, EngineClass>,
        openbnct_default_class: &'a str,
    }
    let meta = Meta {
        classes: ["air", "brain", "cranium", "scalp", "tumour"],
        density_g_cm3: &spec.density_g_cm3,
        voxel_cm,
        shape_zyx: [nz, ny, nx],
        lower_left_cm_xyz,
        roi_voxels,
        roi_volume_cm3,
        class_voxels: class_voxels.clone(),
        openbnct_case_id: &case.case_id,
        openbnct_arrays_sha256: &arrays_sha256,
        openbnct_class_map: &spec.region_classes,
        openbnct_default_class: spec.default_class.name(),
    };
    let meta_path = PathBuf::from(format!("{}_meta.json", prefix.display()));
    let meta_json = serde_json::to_vec_pretty(&meta)?;
    std::fs::write(&meta_path, &meta_json)?;
    let meta_sha256 = hex_sha256(&meta_json);

    Ok(VoxelPlanExport {
        arrays_path,
        meta_path,
        arrays_sha256,
        meta_sha256,
        class_voxels,
    })
}
