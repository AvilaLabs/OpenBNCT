// SPDX-License-Identifier: MIT

//! Automatic CT cropping for `dicom import-ct` and `import ct-nifti`.
//!
//! A real CT includes shoulders, couch and air; building the transport grid
//! over the whole field of view can exhaust memory. The crop picks a box of
//! CT voxels (default: the body plus a margin), and the import runs on that
//! box only. ROI masks are cropped with the same box, and what the crop drops
//! from each ROI is reported. Research import, not a clinical workflow.

use std::io;

use openbnct_core::GridGeometry;
use serde_json::{Value, json};

use crate::ct_import::{CtImportRoi, CtImportSource};

/// HU above which a voxel counts as body (soft tissue and denser; air and
/// lung are below).
pub(crate) const BODY_HU_THRESHOLD: f64 = -400.0;

/// What `--crop` selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum CropMode {
    /// Bounding box of the largest connected body component plus a margin.
    Body,
    /// Keep the full CT field of view.
    None,
}

/// Crop options shared by `dicom import-ct` and `import ct-nifti`.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct CropArgs {
    /// Crop the CT before the transport grid is built. `body` (default):
    /// threshold HU > -400, keep the largest 6-connected component (drops
    /// the couch), fill holes per axial slice, and use its bounding box plus
    /// `--crop-margin-mm`, clipped to the CT. `none`: keep the full field of
    /// view (a whole-body CT can exhaust memory). ROI masks use the cropped
    /// grid; ROIs the crop clips are reported.
    #[arg(long, value_enum, default_value_t = CropMode::Body)]
    pub crop: CropMode,
    /// Air margin around the body box, mm (rounded out to whole CT voxels).
    #[arg(long, default_value_t = 15.0)]
    pub crop_margin_mm: f64,
    /// Explicit crop box in patient LPS mm, `x0,x1,y0,y1,z0,z1`; replaces the
    /// body rule (no margin is added). Keeps CT voxels whose centers lie in
    /// the box. Needs an axis-aligned (identity direction) CT.
    #[arg(long, value_delimiter = ',', allow_hyphen_values = true)]
    pub crop_box_mm: Option<Vec<f64>>,
    /// Keep only CT voxels with center z >= Z (patient LPS mm), applied on
    /// top of the body rule or the explicit box (the body box is then
    /// computed from the kept part, so wide shoulders below the cut do not
    /// widen it). Drops the shoulders of a head scan.
    #[arg(long, allow_hyphen_values = true)]
    pub crop_superior_of_mm: Option<f64>,
}

impl Default for CropArgs {
    fn default() -> Self {
        Self {
            crop: CropMode::Body,
            crop_margin_mm: 15.0,
            crop_box_mm: None,
            crop_superior_of_mm: None,
        }
    }
}

/// The CT voxel range kept, `lo..hi` per axis (hi exclusive), with the
/// record of how it was chosen.
#[derive(Debug, Clone)]
pub(crate) struct CropPlan {
    pub lo: [usize; 3],
    pub hi: [usize; 3],
    pub rule: String,
    pub parameters: Value,
}

fn lattice_index(shape: [usize; 3], i: usize, j: usize, k: usize) -> usize {
    i + shape[0] * (j + shape[1] * k)
}

/// Body mask: HU > threshold, largest 6-connected component, holes filled
/// per axial slice (4-connected background reachable from the slice border
/// stays outside).
pub(crate) fn body_mask(hu: &[f64], shape: [usize; 3]) -> io::Result<Vec<bool>> {
    let n = hu.len();
    let foreground: Vec<bool> = hu.iter().map(|&v| v > BODY_HU_THRESHOLD).collect();
    let mut label = vec![0_u32; n];
    let mut sizes: Vec<usize> = vec![0];
    let mut stack: Vec<usize> = Vec::new();
    for seed in 0..n {
        if !foreground[seed] || label[seed] != 0 {
            continue;
        }
        let id = sizes.len() as u32;
        label[seed] = id;
        stack.push(seed);
        let mut size = 0_usize;
        while let Some(p) = stack.pop() {
            size += 1;
            let i = p % shape[0];
            let j = (p / shape[0]) % shape[1];
            let k = p / (shape[0] * shape[1]);
            for kk in k.saturating_sub(1)..=(k + 1).min(shape[2] - 1) {
                for jj in j.saturating_sub(1)..=(j + 1).min(shape[1] - 1) {
                    for ii in i.saturating_sub(1)..=(i + 1).min(shape[0] - 1) {
                        // 6-connectivity: face neighbours only. 26-connectivity
                        // let thin bridges (head supports) join the body and
                        // widened the real-head box ~1.3x per transverse axis.
                        if usize::from(ii != i) + usize::from(jj != j) + usize::from(kk != k) != 1 {
                            continue;
                        }
                        let q = lattice_index(shape, ii, jj, kk);
                        if foreground[q] && label[q] == 0 {
                            label[q] = id;
                            stack.push(q);
                        }
                    }
                }
            }
        }
        sizes.push(size);
    }
    drop(foreground);
    let Some((best, _)) = sizes
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(&a.0)))
    else {
        return Err(io::Error::other(format!(
            "--crop body: no voxel exceeds {BODY_HU_THRESHOLD} HU, so there is no body to \
             crop to (check the CT is in HU; use --crop none to keep the full field of view)"
        )));
    };
    let best = best as u32;
    let mut mask: Vec<bool> = label.iter().map(|&l| l == best).collect();
    drop(label);

    // Fill holes per axial slice.
    let (nx, ny) = (shape[0], shape[1]);
    let mut outside = vec![false; nx * ny];
    let mut queue: Vec<usize> = Vec::new();
    for k in 0..shape[2] {
        let slice = &mut mask[k * nx * ny..(k + 1) * nx * ny];
        outside.fill(false);
        queue.clear();
        // Mark a background pixel as reachable from the border.
        let visit = |p: usize, outside: &mut [bool], queue: &mut Vec<usize>, slice: &[bool]| {
            if !slice[p] && !outside[p] {
                outside[p] = true;
                queue.push(p);
            }
        };
        for i in 0..nx {
            visit(i, &mut outside, &mut queue, slice);
            visit(i + nx * (ny - 1), &mut outside, &mut queue, slice);
        }
        for j in 0..ny {
            visit(nx * j, &mut outside, &mut queue, slice);
            visit(nx - 1 + nx * j, &mut outside, &mut queue, slice);
        }
        while let Some(p) = queue.pop() {
            let (i, j) = (p % nx, p / nx);
            if i > 0 {
                visit(p - 1, &mut outside, &mut queue, slice);
            }
            if i + 1 < nx {
                visit(p + 1, &mut outside, &mut queue, slice);
            }
            if j > 0 {
                visit(p - nx, &mut outside, &mut queue, slice);
            }
            if j + 1 < ny {
                visit(p + nx, &mut outside, &mut queue, slice);
            }
        }
        for (cell, out) in slice.iter_mut().zip(&outside) {
            *cell = !*out;
        }
    }
    Ok(mask)
}

fn is_identity(direction: &[f64; 9]) -> bool {
    let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    direction
        .iter()
        .zip(identity)
        .all(|(a, b)| (a - b).abs() < 1.0e-6)
}

/// Decide the CT voxel range to keep. `None` when no cropping applies
/// (`--crop none`).
pub(crate) fn plan_crop(
    geometry: &GridGeometry,
    hu: &[f64],
    crop: &CropArgs,
) -> io::Result<Option<CropPlan>> {
    if !(crop.crop_margin_mm.is_finite() && crop.crop_margin_mm >= 0.0) {
        return Err(io::Error::other(
            "--crop-margin-mm must be finite and non-negative",
        ));
    }
    if crop.crop == CropMode::None {
        if crop.crop_box_mm.is_some() || crop.crop_superior_of_mm.is_some() {
            return Err(io::Error::other(
                "--crop none cannot be combined with --crop-box-mm or --crop-superior-of-mm",
            ));
        }
        return Ok(None);
    }
    let shape = geometry.shape.map(|n| n as usize);
    let spacing = geometry.spacing_mm;
    if (crop.crop_box_mm.is_some() || crop.crop_superior_of_mm.is_some())
        && !is_identity(&geometry.direction)
    {
        return Err(io::Error::other(
            "--crop-box-mm and --crop-superior-of-mm need an axis-aligned CT in patient LPS \
             (identity direction cosines); use --crop body or --crop none",
        ));
    }
    // Voxel-center coordinate of index `a` along `axis` (identity direction).
    let center = |axis: usize, a: usize| geometry.origin_mm[axis] + a as f64 * spacing[axis];

    // First slice at or above the superior cut.
    let mut z_floor = 0_usize;
    if let Some(z) = crop.crop_superior_of_mm {
        if !z.is_finite() {
            return Err(io::Error::other("--crop-superior-of-mm must be finite"));
        }
        match (0..shape[2]).find(|&k| center(2, k) >= z) {
            Some(k) => z_floor = k,
            None => {
                return Err(io::Error::other(format!(
                    "--crop-superior-of-mm {z} lies above the whole CT (top voxel center z = {} mm)",
                    center(2, shape[2] - 1)
                )));
            }
        }
    }

    let mut lo = [0_usize; 3];
    let mut hi = shape;
    let mut params = serde_json::Map::new();
    let mut rule;
    if let Some(values) = &crop.crop_box_mm {
        let [x0, x1, y0, y1, z0, z1] = values[..] else {
            return Err(io::Error::other("--crop-box-mm takes six values"));
        };
        for (axis, b) in [[x0, x1], [y0, y1], [z0, z1]].iter().enumerate() {
            if !(b[0].is_finite() && b[1].is_finite() && b[0] <= b[1]) {
                return Err(io::Error::other(
                    "--crop-box-mm must be x0,x1,y0,y1,z0,z1 with each lower bound <= its upper",
                ));
            }
            let kept = |a: &usize| center(axis, *a) >= b[0] && center(axis, *a) <= b[1];
            let (Some(first), Some(last)) = (
                (0..shape[axis]).find(kept),
                (0..shape[axis]).rev().find(kept),
            ) else {
                return Err(io::Error::other(format!(
                    "--crop-box-mm contains no CT voxel center along {} (box {b:?} mm)",
                    ["x", "y", "z"][axis],
                )));
            };
            lo[axis] = first;
            hi[axis] = last + 1;
        }
        params.insert("box_mm_lps".into(), json!(values));
        rule = "explicit box: CT voxels whose centers lie in the given LPS box".to_owned();
    } else {
        let mask = body_mask(hu, shape)?;
        let mut min = [usize::MAX; 3];
        let mut max = [0_usize; 3];
        let mut any = false;
        for k in z_floor..shape[2] {
            for j in 0..shape[1] {
                let row = lattice_index(shape, 0, j, k);
                for i in 0..shape[0] {
                    if mask[row + i] {
                        any = true;
                        for (axis, a) in [i, j, k].into_iter().enumerate() {
                            min[axis] = min[axis].min(a);
                            max[axis] = max[axis].max(a);
                        }
                    }
                }
            }
        }
        if !any {
            return Err(io::Error::other(
                "--crop body: the body has no voxel at or above --crop-superior-of-mm",
            ));
        }
        for axis in 0..3 {
            let pad = (crop.crop_margin_mm / spacing[axis]).ceil() as usize;
            lo[axis] = min[axis].saturating_sub(pad);
            hi[axis] = (max[axis] + 1 + pad).min(shape[axis]);
        }
        params.insert("hu_threshold".into(), json!(BODY_HU_THRESHOLD));
        params.insert("margin_mm".into(), json!(crop.crop_margin_mm));
        rule = format!(
            "body: HU > {BODY_HU_THRESHOLD}, largest 6-connected component, holes filled per \
             axial slice, axis-aligned bounding box plus {} mm margin (rounded out to whole CT \
             voxels), clipped to the CT extent",
            crop.crop_margin_mm,
        );
    }
    if let Some(z) = crop.crop_superior_of_mm {
        lo[2] = lo[2].max(z_floor);
        if lo[2] >= hi[2] {
            return Err(io::Error::other(
                "--crop-superior-of-mm leaves no CT slices inside the crop box",
            ));
        }
        params.insert("superior_of_mm".into(), json!(z));
        rule.push_str(&format!(
            "; then only CT voxels with center z >= {z} mm are kept (a body box is taken over \
             that part only)"
        ));
    }
    Ok(Some(CropPlan {
        lo,
        hi,
        rule,
        parameters: Value::Object(params),
    }))
}

fn crop_lattice<T: Copy>(src: &[T], shape: [usize; 3], lo: [usize; 3], hi: [usize; 3]) -> Vec<T> {
    let mut out = Vec::with_capacity((hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]));
    for k in lo[2]..hi[2] {
        for j in lo[1]..hi[1] {
            let row = lattice_index(shape, 0, j, k);
            out.extend_from_slice(&src[row + lo[0]..row + hi[0]]);
        }
    }
    out
}

/// What the crop dropped from one ROI.
pub(crate) struct RoiClipping {
    pub name: String,
    pub ct_voxels: usize,
    pub dropped: usize,
}

/// A source restricted to a crop box: geometry on the cropped lattice, HU and
/// per-ROI masks cropped, plus what the crop dropped from each ROI.
pub(crate) struct CroppedSource {
    pub geometry: GridGeometry,
    pub hu: Vec<f64>,
    pub rois: Option<Vec<CtImportRoi>>,
    pub clipping: Vec<RoiClipping>,
}

pub(crate) fn apply_crop(source: &CtImportSource, plan: &CropPlan) -> CroppedSource {
    let shape = source.geometry.shape.map(|n| n as usize);
    let (lo, hi) = (plan.lo, plan.hi);
    let sp = source.geometry.spacing_mm;
    let d = &source.geometry.direction;
    let local = [
        lo[0] as f64 * sp[0],
        lo[1] as f64 * sp[1],
        lo[2] as f64 * sp[2],
    ];
    let o = source.geometry.origin_mm;
    let geometry = GridGeometry {
        shape: [
            (hi[0] - lo[0]) as u32,
            (hi[1] - lo[1]) as u32,
            (hi[2] - lo[2]) as u32,
        ],
        spacing_mm: sp,
        origin_mm: [
            o[0] + d[0] * local[0] + d[1] * local[1] + d[2] * local[2],
            o[1] + d[3] * local[0] + d[4] * local[1] + d[5] * local[2],
            o[2] + d[6] * local[0] + d[7] * local[1] + d[8] * local[2],
        ],
        direction: source.geometry.direction,
    };
    let mut clipping = Vec::new();
    let rois = source.rois.as_ref().map(|rois| {
        rois.iter()
            .map(|roi| {
                let before = roi.voxels.iter().filter(|v| **v).count();
                let voxels = crop_lattice(&roi.voxels, shape, lo, hi);
                let after = voxels.iter().filter(|v| **v).count();
                clipping.push(RoiClipping {
                    name: roi.name.clone(),
                    ct_voxels: before,
                    dropped: before - after,
                });
                CtImportRoi {
                    number: roi.number,
                    name: roi.name.clone(),
                    voxels,
                }
            })
            .collect()
    });
    CroppedSource {
        geometry,
        hu: crop_lattice(&source.hu, shape, lo, hi),
        rois,
        clipping,
    }
}

/// The `crop` block of the import record.
pub(crate) fn crop_record(
    original: &GridGeometry,
    cropped: &GridGeometry,
    plan: Option<&CropPlan>,
    clipping: &[RoiClipping],
) -> Value {
    let extent = |g: &GridGeometry| match g.bounding_box_lps_mm() {
        Ok((min, max)) => json!({"min_lps_mm": min, "max_lps_mm": max}),
        Err(_) => Value::Null,
    };
    let count = |g: &GridGeometry| g.voxel_count().ok();
    let roi_records: Vec<Value> = clipping
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "ct_voxels": c.ct_voxels,
                "dropped_by_crop": c.dropped,
            })
        })
        .collect();
    match plan {
        None => json!({
            "mode": "none",
            "rule": "no crop: the full CT field of view",
            "original_extent": extent(original),
            "original_ct_voxels": count(original),
            "original_ct_shape": original.shape,
            "cropped_extent": extent(cropped),
            "cropped_ct_voxels": count(cropped),
            "cropped_ct_shape": cropped.shape,
            "roi_clipping": roi_records,
        }),
        Some(plan) => json!({
            "mode": if plan.parameters.get("box_mm_lps").is_some() { "box" } else { "body" },
            "rule": plan.rule,
            "parameters": plan.parameters,
            "ct_voxel_range": {"lo": plan.lo, "hi_exclusive": plan.hi},
            "original_extent": extent(original),
            "original_ct_voxels": count(original),
            "original_ct_shape": original.shape,
            "cropped_extent": extent(cropped),
            "cropped_ct_voxels": count(cropped),
            "cropped_ct_shape": cropped.shape,
            "roi_clipping": roi_records,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: [usize; 3] = [40, 40, 30];

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [40, 40, 30],
            spacing_mm: [5.0; 3],
            origin_mm: [-100.0, -100.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// Air everywhere; a hollow (air-core) body ellipsoid-ish blob spanning
    /// voxels i,j in 12..28, k in 8..24 with an air hole inside, plus a
    /// disconnected "couch" slab at k = 0..2 spanning the full x extent.
    fn phantom() -> Vec<f64> {
        let mut hu = vec![-1000.0; N[0] * N[1] * N[2]];
        for k in 0..N[2] {
            for j in 0..N[1] {
                for i in 0..N[0] {
                    let at = lattice_index(N, i, j, k);
                    if (12..28).contains(&i) && (12..28).contains(&j) && (8..24).contains(&k) {
                        hu[at] = 40.0;
                        // interior air pocket (not connected to the outside in-plane)
                        if (18..22).contains(&i) && (18..22).contains(&j) {
                            hu[at] = -900.0;
                        }
                    }
                    if k < 2 && (2..38).contains(&j) {
                        hu[at] = 300.0;
                    }
                }
            }
        }
        hu
    }

    #[test]
    fn body_crop_keeps_body_drops_couch() {
        let hu = phantom();
        let mask = body_mask(&hu, N).unwrap();
        // Hole filled.
        assert!(mask[lattice_index(N, 20, 20, 15)]);
        // Couch excluded.
        assert!(!mask[lattice_index(N, 20, 20, 0)]);
        let plan = plan_crop(
            &geometry(),
            &hu,
            &CropArgs {
                crop_margin_mm: 0.0,
                ..CropArgs::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(plan.lo, [12, 12, 8]);
        assert_eq!(plan.hi, [28, 28, 24]);
    }

    #[test]
    fn margin_rounds_out_and_clips_to_ct() {
        let hu = phantom();
        // 15 mm = 3 voxels; 16 mm rounds out to 4.
        let plan = plan_crop(&geometry(), &hu, &CropArgs::default())
            .unwrap()
            .unwrap();
        assert_eq!(plan.lo, [9, 9, 5]);
        assert_eq!(plan.hi, [31, 31, 27]);
        let plan = plan_crop(
            &geometry(),
            &hu,
            &CropArgs {
                crop_margin_mm: 16.0,
                ..CropArgs::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(plan.lo, [8, 8, 4]);
        // Huge margin clips to the CT; the couch is then inside the box.
        let plan = plan_crop(
            &geometry(),
            &hu,
            &CropArgs {
                crop_margin_mm: 1000.0,
                ..CropArgs::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!((plan.lo, plan.hi), ([0, 0, 0], [40, 40, 30]));
    }

    #[test]
    fn explicit_box_selects_voxel_centers() {
        // x centers are -100 + 5i: [-50, 0] is i = 10..=20.
        let plan = plan_crop(
            &geometry(),
            &phantom(),
            &CropArgs {
                crop_box_mm: Some(vec![-50.0, 0.0, -25.0, 25.0, 10.0, 100.0]),
                ..CropArgs::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(plan.lo, [10, 15, 2]);
        assert_eq!(plan.hi, [21, 26, 21]);
        assert!(
            plan_crop(
                &geometry(),
                &phantom(),
                &CropArgs {
                    crop_box_mm: Some(vec![500.0, 600.0, 0.0, 1.0, 0.0, 1.0]),
                    ..CropArgs::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn superior_cut_applies_to_body_and_box_and_none_conflicts() {
        let hu = phantom();
        // z centers are 5k; keep z >= 60 -> k >= 12.
        let plan = plan_crop(
            &geometry(),
            &hu,
            &CropArgs {
                crop_margin_mm: 0.0,
                crop_superior_of_mm: Some(60.0),
                ..CropArgs::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!((plan.lo[2], plan.hi[2]), (12, 24));
        // Margin below the cut is clamped to the cut.
        let plan = plan_crop(
            &geometry(),
            &hu,
            &CropArgs {
                crop_superior_of_mm: Some(60.0),
                ..CropArgs::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(plan.lo[2], 12);
        // Cut above the whole CT is an error.
        assert!(
            plan_crop(
                &geometry(),
                &hu,
                &CropArgs {
                    crop_superior_of_mm: Some(1.0e4),
                    ..CropArgs::default()
                },
            )
            .is_err()
        );
        let mut none = CropArgs {
            crop: CropMode::None,
            ..CropArgs::default()
        };
        assert!(plan_crop(&geometry(), &hu, &none).unwrap().is_none());
        none.crop_superior_of_mm = Some(0.0);
        assert!(plan_crop(&geometry(), &hu, &none).is_err());
    }

    #[test]
    fn roi_clipping_counts_dropped_voxels() {
        let hu = phantom();
        let mut voxels = vec![false; hu.len()];
        // ROI straddles the crop: one voxel inside, one in the couch.
        voxels[lattice_index(N, 20, 20, 15)] = true;
        voxels[lattice_index(N, 20, 20, 0)] = true;
        let source = CtImportSource {
            geometry: geometry(),
            hu,
            description: String::new(),
            rois: Some(vec![CtImportRoi {
                number: 1,
                name: "R".into(),
                voxels,
            }]),
            provenance: serde_json::Map::new(),
        };
        let plan = plan_crop(&source.geometry, &source.hu, &CropArgs::default())
            .unwrap()
            .unwrap();
        let cropped = apply_crop(&source, &plan);
        assert_eq!(cropped.geometry.shape, [22, 22, 22]);
        assert_eq!(cropped.geometry.origin_mm, [-55.0, -55.0, 25.0]);
        assert_eq!(cropped.clipping[0].ct_voxels, 2);
        assert_eq!(cropped.clipping[0].dropped, 1);
        let roi = &cropped.rois.unwrap()[0];
        assert_eq!(roi.voxels.iter().filter(|v| **v).count(), 1);
        assert!(roi.voxels[lattice_index([22, 22, 22], 11, 11, 10)]);
    }
}
