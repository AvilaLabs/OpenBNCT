// SPDX-License-Identifier: MIT

//! CT import shared by `dicom import-ct` and `import ct-nifti`.
//!
//! Both commands reduce their input to a [`CtImportSource`] (an HU volume on
//! the source lattice plus optional per-ROI boolean masks on the same
//! lattice) and hand it to [`finish_ct_import`], which does the covering
//! grid, the overlap-weighted box-mean HU downsampling, the 50 %-coverage
//! ROI rule, the scaffold case and the import record. Research import, not a
//! clinical workflow.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use openbnct_core::GridGeometry;
use openbnct_nifti::{DT_FLOAT64, NiftiImage, box_average_to_grid, covering_grid, write_nifti};
use openbnct_transport::MaterialDefinition;
use serde_json::{Map, Value, json};

use super::{scaffold_case_from_geometry, write_new_json};
use crate::ct_crop::{CropArgs, apply_crop, crop_record, plan_crop};

pub(crate) use crate::ct_crop::BODY_HU_THRESHOLD;

/// One ROI rasterized on the source lattice.
pub(crate) struct CtImportRoi {
    pub number: i32,
    pub name: String,
    pub voxels: Vec<bool>,
}

/// A CT ready for transport-grid import.
pub(crate) struct CtImportSource {
    pub geometry: GridGeometry,
    /// HU in lattice order `i + nx*j + nx*ny*k`.
    pub hu: Vec<f64>,
    /// NIfTI `descrip` of the written HU volume.
    pub description: String,
    /// `None` when the source carries no structures at all.
    pub rois: Option<Vec<CtImportRoi>>,
    /// Source-specific keys merged into the import record.
    pub provenance: Map<String, Value>,
}

/// Outputs are never overwritten.
pub(crate) fn check_new_outputs(
    case_output: &Path,
    hu_output: &Path,
    masks_dir: Option<&Path>,
) -> io::Result<()> {
    let record_path = case_output.with_extension("import-record.json");
    for output in [case_output, hu_output, record_path.as_path()] {
        if output.exists() {
            return Err(io::Error::other(format!(
                "{} already exists; outputs are never overwritten",
                output.display()
            )));
        }
    }
    if let Some(dir) = masks_dir
        && dir.exists()
    {
        return Err(io::Error::other(format!(
            "{} already exists; outputs are never overwritten",
            dir.display()
        )));
    }
    Ok(())
}

pub(crate) fn read_base_material(path: &Path) -> io::Result<MaterialDefinition> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::other(format!("base material JSON: {error}")))
}

/// Covering transport grid, box-averaged HU, scaffold case, optional masks
/// and the import record.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_ct_import(
    source: &CtImportSource,
    spacing_mm: &[f64],
    case_id: &str,
    base: &MaterialDefinition,
    base_material_path: &Path,
    case_output: &Path,
    hu_output: &Path,
    masks_dir: Option<&Path>,
    crop: &CropArgs,
) -> io::Result<()> {
    let record_path = case_output.with_extension("import-record.json");
    if masks_dir.is_some() && source.rois.is_none() {
        return Err(io::Error::other(
            "--masks-dir requested but no RT Structure Set was found",
        ));
    }
    let native = source.geometry.spacing_mm;
    let target_spacing = match spacing_mm {
        [] => native,
        [s] => [*s; 3],
        [x, y, z] => [*x, *y, *z],
        _ => {
            return Err(io::Error::other(
                "--spacing-mm takes one value or three (x,y,z)",
            ));
        }
    };
    if target_spacing.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(io::Error::other("--spacing-mm must be positive and finite"));
    }
    // Crop first: the covering grid, the HU downsampling and every ROI mask
    // use the cropped lattice only.
    let plan = plan_crop(&source.geometry, &source.hu, crop)?;
    let cropped = plan.as_ref().map(|plan| apply_crop(source, plan));
    let (ct_geometry, ct_hu, ct_rois) = match &cropped {
        Some(c) => (&c.geometry, &c.hu, &c.rois),
        None => (&source.geometry, &source.hu, &source.rois),
    };
    let clipping: &[crate::ct_crop::RoiClipping] =
        cropped.as_ref().map_or(&[], |c| c.clipping.as_slice());
    for c in clipping.iter().filter(|c| c.dropped > 0) {
        eprintln!(
            "warning: ROI {:?} is clipped by the crop: {} of {} CT voxels dropped ({:.1} %); \
             widen --crop-margin-mm or use --crop none / --crop-box-mm",
            c.name,
            c.dropped,
            c.ct_voxels,
            100.0 * c.dropped as f64 / c.ct_voxels.max(1) as f64
        );
    }
    let grid = covering_grid(ct_geometry, target_spacing);
    grid.voxel_count().map_err(io::Error::other)?;
    let hu = box_average_to_grid(ct_hu, ct_geometry, &grid)
        .ok_or_else(|| io::Error::other("internal: grid does not align with the CT lattice"))?;

    write_nifti(
        &NiftiImage {
            geometry: grid.clone(),
            values: hu,
            datatype: DT_FLOAT64,
            transform_source: "sform",
            description: source.description.clone(),
            intent_name: String::new(),
            units_declared_mm: true,
        },
        hu_output,
    )?;

    let scaffold = scaffold_case_from_geometry(&grid, base, case_id);
    write_new_json(case_output, &scaffold)?;

    let mut mask_records = Vec::new();
    if let (Some(dir), Some(rois)) = (masks_dir, ct_rois) {
        fs::create_dir_all(dir)?;
        for roi in rois {
            let fractions = box_average_to_grid(
                &roi.voxels
                    .iter()
                    .map(|&v| if v { 1.0 } else { 0.0 })
                    .collect::<Vec<_>>(),
                ct_geometry,
                &grid,
            )
            .ok_or_else(|| io::Error::other("internal: mask grid misaligned"))?;
            let mask = openbnct_core::RegionMask {
                name: roi.name.clone(),
                voxels: fractions.iter().map(|f| *f >= 0.5).collect(),
            };
            let safe: String = roi
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let file = format!("{:03}-{safe}.json", roi.number);
            let path = dir.join(&file);
            write_new_json(&path, &mask)?;
            mask_records.push(json!({
                "roi_number": roi.number,
                "name": roi.name,
                "file": file,
                "sha256": openbnct_evidence::sha256_file(&path)?,
                "ct_voxels": roi.voxels.iter().filter(|v| **v).count(),
                "ct_voxels_dropped_by_crop": clipping
                    .iter()
                    .find(|c| c.name == roi.name)
                    .map_or(0, |c| c.dropped),
                "grid_voxels": mask.included_voxel_count(),
            }));
        }
        write_new_json(
            &dir.join("index.json"),
            &json!({
                "schema_version": "openbnct.roi-mask-index/0.1.0",
                "case_id": case_id,
                "membership": "voxel is in the ROI when >= 50% of its volume is covered by CT voxels whose centers lie inside the contour polygon",
                "masks": mask_records,
            }),
        )?;
    }

    let mut record = json!({
        "schema_version": "openbnct.ct-import-record/0.1.0",
        "case_id": case_id,
        "ct_geometry": source.geometry,
        "crop": crop_record(&source.geometry, ct_geometry, plan.as_ref(), clipping),
        "case_geometry": grid,
        "resampling": "hu: volume-weighted box mean of overlapping CT voxels (outside-CT parts excluded); masks: >= 50% volume fraction of CT-grid ROI voxels; no reorientation",
        "base_material_sha256": openbnct_evidence::sha256_file(base_material_path)?,
        "outputs": {
            "case": {"file": case_output, "sha256": openbnct_evidence::sha256_file(case_output)?},
            "hu": {"file": hu_output, "sha256": openbnct_evidence::sha256_file(hu_output)?},
        },
        "masks": mask_records,
    });
    if let Value::Object(map) = &mut record {
        for (key, value) in &source.provenance {
            map.insert(key.clone(), value.clone());
        }
    }
    write_new_json(&record_path, &record)?;

    println!(
        "ct: {:?} @ {:?} mm -> case grid {:?} @ {:?} mm",
        ct_geometry.shape, native, grid.shape, grid.spacing_mm
    );
    if plan.is_some() {
        println!(
            "crop: CT {:?} ({} voxels) -> {:?} ({} voxels); rule and extents in the record",
            source.geometry.shape,
            source.geometry.voxel_count().unwrap_or(0),
            ct_geometry.shape,
            ct_geometry.voxel_count().unwrap_or(0)
        );
    } else {
        println!("crop: none (full CT field of view)");
    }
    println!("case: {}", case_output.display());
    println!("hu: {}", hu_output.display());
    if let Some(dir) = masks_dir {
        println!("masks: {} ({} ROIs)", dir.display(), mask_records.len());
    }
    println!("record: {}", record_path.display());
    println!(
        "note: the scaffold source is a placeholder — bind a real beam with `openbnct beam bind`"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// NIfTI source
// ---------------------------------------------------------------------------

/// How label values map to ROIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelEncoding {
    /// A voxel belongs to the ROI whose label equals its value (ROIs are
    /// disjoint).
    Exclusive,
    /// Each label is a distinct power of two and a voxel belongs to every
    /// ROI whose bit is set (ROIs may overlap).
    Bitmask,
}

/// Parsed `--label-names` document.
#[derive(Debug)]
pub(crate) struct LabelNames {
    pub encoding: LabelEncoding,
    /// Label value → ROI name, ascending by value.
    pub labels: BTreeMap<u64, String>,
}

impl LabelNames {
    /// ROI names in ascending label order.
    pub fn names(&self) -> Vec<String> {
        self.labels.values().cloned().collect()
    }
}

/// Parse a label-names document: either a flat `{"1": "Brain", ...}` object
/// (exclusive labels) or `{"encoding": "exclusive"|"bitmask", "labels":
/// {"1": "Brain", ...}}`.
pub(crate) fn parse_label_names(bytes: &[u8]) -> io::Result<LabelNames> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| io::Error::other(format!("label names JSON: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| io::Error::other("label names JSON must be an object"))?;
    let (encoding, table) = match object.get("labels") {
        Some(Value::Object(table)) => {
            let encoding = match object.get("encoding").and_then(Value::as_str) {
                None | Some("exclusive") => LabelEncoding::Exclusive,
                Some("bitmask") => LabelEncoding::Bitmask,
                Some(other) => {
                    return Err(io::Error::other(format!(
                        "label names: encoding {other:?} is not `exclusive` or `bitmask`"
                    )));
                }
            };
            for key in object.keys() {
                if key != "labels" && key != "encoding" && key != "description" {
                    return Err(io::Error::other(format!(
                        "label names: unknown key {key:?}"
                    )));
                }
            }
            (encoding, table)
        }
        _ => (LabelEncoding::Exclusive, object),
    };
    let mut labels = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for (key, name) in table {
        let label: u64 = key.parse().map_err(|_| {
            io::Error::other(format!(
                "label names: key {key:?} is not a positive integer"
            ))
        })?;
        if label == 0 {
            return Err(io::Error::other(
                "label names: label 0 is the background and cannot be named",
            ));
        }
        if encoding == LabelEncoding::Bitmask && !label.is_power_of_two() {
            return Err(io::Error::other(format!(
                "label names: bitmask label {label} is not a power of two"
            )));
        }
        if label > (1_u64 << 52) {
            return Err(io::Error::other(format!(
                "label names: label {label} exceeds 2^52 (not exactly representable)"
            )));
        }
        let name = name.as_str().map(str::trim).filter(|n| !n.is_empty());
        let Some(name) = name else {
            return Err(io::Error::other(format!(
                "label names: label {label} needs a non-empty string name"
            )));
        };
        if !seen.insert(name.to_owned()) {
            return Err(io::Error::other(format!(
                "label names: duplicate ROI name {name:?}"
            )));
        }
        labels.insert(label, name.to_owned());
    }
    if labels.is_empty() {
        return Err(io::Error::other("label names: no labels defined"));
    }
    Ok(LabelNames { encoding, labels })
}

fn geometry_mismatch(a: &GridGeometry, b: &GridGeometry) -> Option<String> {
    if a.shape != b.shape {
        return Some(format!("shape {:?} vs {:?}", a.shape, b.shape));
    }
    let close = |x: &[f64], y: &[f64]| x.iter().zip(y).all(|(p, q)| (p - q).abs() < 1.0e-4);
    if !close(&a.spacing_mm, &b.spacing_mm) {
        return Some(format!("spacing {:?} vs {:?}", a.spacing_mm, b.spacing_mm));
    }
    if !close(&a.origin_mm, &b.origin_mm) {
        return Some(format!("origin {:?} vs {:?}", a.origin_mm, b.origin_mm));
    }
    if !close(&a.direction, &b.direction) {
        return Some("direction cosines differ".into());
    }
    None
}

/// Load an HU NIfTI (+ optional labelmap and names) as an import source.
pub(crate) fn load_ct_nifti_source(
    hu_path: &Path,
    labels: Option<(&Path, &Path)>,
) -> io::Result<CtImportSource> {
    let hu_image = openbnct_nifti::read_volume(hu_path)
        .map_err(|error| io::Error::other(format!("{}: {error}", hu_path.display())))?;
    let geometry = hu_image.geometry.clone();
    let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    if geometry
        .direction
        .iter()
        .zip(identity)
        .any(|(a, b)| (a - b).abs() > 1.0e-6)
    {
        return Err(io::Error::other(format!(
            "{}: the HU volume is not axis-aligned in LPS (direction cosines {:?}); the transport \
             stack needs an identity direction, so reorient/resample it first (nothing is \
             reoriented here)",
            hu_path.display(),
            geometry.direction
        )));
    }
    if hu_image.values.iter().any(|v| !v.is_finite()) {
        return Err(io::Error::other(format!(
            "{}: HU volume contains non-finite values",
            hu_path.display()
        )));
    }
    let mut provenance = Map::new();
    provenance.insert("source_format".into(), json!("nifti"));
    let mut inputs = vec![json!({
        "role": "hu",
        "file": hu_path.file_name().map(|n| n.to_string_lossy().into_owned()),
        "sha256": openbnct_evidence::sha256_file(hu_path)?,
    })];

    let rois = match labels {
        None => None,
        Some((labels_path, names_path)) => {
            let names_bytes = fs::read(names_path)?;
            let names = parse_label_names(&names_bytes)?;
            let label_image = openbnct_nifti::read_volume(labels_path)
                .map_err(|error| io::Error::other(format!("{}: {error}", labels_path.display())))?;
            if let Some(why) = geometry_mismatch(&geometry, &label_image.geometry) {
                return Err(io::Error::other(format!(
                    "{}: labelmap grid does not match the HU volume ({why}); resample the labels \
                     onto the HU grid with nearest-neighbour first",
                    labels_path.display()
                )));
            }
            let mut integers = Vec::with_capacity(label_image.values.len());
            for &value in &label_image.values {
                let rounded = value.round();
                if !value.is_finite() || (value - rounded).abs() > 1.0e-6 || rounded < 0.0 {
                    return Err(io::Error::other(format!(
                        "{}: labelmap values must be non-negative integers (found {value})",
                        labels_path.display()
                    )));
                }
                integers.push(rounded as u64);
            }
            let known: u64 = names.labels.keys().fold(0, |acc, k| acc | k);
            match names.encoding {
                LabelEncoding::Exclusive => {
                    if let Some(unknown) = integers
                        .iter()
                        .find(|&&v| v != 0 && !names.labels.contains_key(&v))
                    {
                        return Err(io::Error::other(format!(
                            "{}: labelmap value {unknown} has no entry in the label names",
                            labels_path.display()
                        )));
                    }
                }
                LabelEncoding::Bitmask => {
                    if let Some(unknown) = integers.iter().find(|&&v| v & !known != 0) {
                        return Err(io::Error::other(format!(
                            "{}: labelmap value {unknown} sets bits outside the named labels",
                            labels_path.display()
                        )));
                    }
                }
            }
            let mut rois = Vec::new();
            for (&label, name) in &names.labels {
                let voxels: Vec<bool> = match names.encoding {
                    LabelEncoding::Exclusive => integers.iter().map(|&v| v == label).collect(),
                    LabelEncoding::Bitmask => integers.iter().map(|&v| v & label != 0).collect(),
                };
                let number = match names.encoding {
                    LabelEncoding::Exclusive => label as i32,
                    LabelEncoding::Bitmask => label.trailing_zeros() as i32 + 1,
                };
                rois.push(CtImportRoi {
                    number,
                    name: name.clone(),
                    voxels,
                });
            }
            inputs.push(json!({
                "role": "labels",
                "file": labels_path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "sha256": openbnct_evidence::sha256_file(labels_path)?,
            }));
            inputs.push(json!({
                "role": "label_names",
                "file": names_path.file_name().map(|n| n.to_string_lossy().into_owned()),
                "sha256": openbnct_evidence::sha256_file(names_path)?,
                "encoding": match names.encoding {
                    LabelEncoding::Exclusive => "exclusive",
                    LabelEncoding::Bitmask => "bitmask",
                },
            }));
            Some(rois)
        }
    };
    provenance.insert("inputs".into(), Value::Array(inputs));
    Ok(CtImportSource {
        geometry,
        hu: hu_image.values,
        description: "openbnct HU box-mean (nifti)".into(),
        rois,
        provenance,
    })
}

/// `import ct-nifti`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_import_ct_nifti(
    hu: &Path,
    labels: Option<&Path>,
    label_names: Option<&Path>,
    spacing_mm: &[f64],
    case_id: &str,
    base_material: &Path,
    case_output: &Path,
    hu_output: &Path,
    masks_dir: Option<&Path>,
    crop: &CropArgs,
) -> io::Result<()> {
    if labels.is_some() != label_names.is_some() {
        return Err(io::Error::other(
            "--labels and --label-names are given together or not at all",
        ));
    }
    if masks_dir.is_some() && labels.is_none() {
        return Err(io::Error::other(
            "--masks-dir requires --labels and --label-names",
        ));
    }
    check_new_outputs(case_output, hu_output, masks_dir)?;
    let base = read_base_material(base_material)?;
    let source = load_ct_nifti_source(hu, labels.zip(label_names))?;
    finish_ct_import(
        &source,
        spacing_mm,
        case_id,
        &base,
        base_material,
        case_output,
        hu_output,
        masks_dir,
        crop,
    )
}
