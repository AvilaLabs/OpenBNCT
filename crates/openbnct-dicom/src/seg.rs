// SPDX-License-Identifier: MIT

//! DICOM Segmentation Storage (SEG) import.
//!
//! A SEG object is a multi-frame image whose frames are per-segment,
//! per-slice bitmaps. Import produces the same [`StructureSet`] of
//! [`RoiMask`]s the RT Structure Set path produces, so everything that
//! consumes RTSTRUCT masks (`dicom import-ct --masks-dir`, project init,
//! study import) consumes a SEG unchanged.
//!
//! Scope, deliberately strict:
//! * `SegmentationType` BINARY (1-bit packed) and FRACTIONAL (8- or 16-bit)
//!   in uncompressed little-endian transfer syntaxes. LABELMAP and
//!   compressed pixel data are refused.
//! * Fractional frames become masks with an explicit, recorded threshold
//!   (`value / MaximumFractionalValue >= threshold`); nothing is silently
//!   rounded.
//! * Mapped onto a CT grid only when every frame lies exactly on that CT's
//!   voxel lattice (same in-plane axes and spacing, positions on integer
//!   voxel indices). Anything else is refused rather than resampled.
//! * Without a CT, the SEG's own frame-of-reference grid is rebuilt from
//!   its Image Position/Orientation and Pixel Spacing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dicom_core::Tag;
use dicom_dictionary_std::{tags, uids};
use dicom_object::{DefaultDicomObject, InMemDicomObject, open_file};
use openbnct_core::GridGeometry;

use crate::rtstruct::{attribute_error, dot, floats, integer, require_string, sequence, string};
use crate::{CtVolume, DicomError, Result, RoiMask, StructureSet};

/// In-plane / through-plane alignment tolerance in voxel units.
const LATTICE_TOLERANCE: f64 = 0.02;
const MAX_VOXELS: usize = 1 << 30;

/// `SegmentationType` of the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentationType {
    Binary,
    Fractional,
}

/// Options for [`import_seg`] / [`import_seg_bytes`].
#[derive(Debug, Clone, Copy)]
pub struct SegImportOptions {
    /// Fractional segmentations: a voxel is in the mask when its fraction of
    /// `MaximumFractionalValue` is at least this. Ignored for BINARY.
    pub fractional_threshold: f64,
}

impl Default for SegImportOptions {
    fn default() -> Self {
        Self {
            fractional_threshold: 0.5,
        }
    }
}

/// One segment's descriptive metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentInfo {
    pub number: i32,
    pub label: String,
    pub description: Option<String>,
    pub algorithm_type: Option<String>,
    /// Number of frames that carried this segment.
    pub frame_count: usize,
}

/// A parsed SEG mapped onto a grid.
#[derive(Debug, Clone)]
pub struct SegmentationImport {
    /// The grid the masks are expressed on: the CT's when one was supplied,
    /// otherwise the grid rebuilt from the SEG's own frames.
    pub geometry: GridGeometry,
    pub frame_of_reference_uid: String,
    pub sop_instance_uid: String,
    pub segmentation_type: SegmentationType,
    /// The threshold actually applied (fractional segmentations only).
    pub fractional_threshold: Option<f64>,
    pub segments: Vec<SegmentInfo>,
    /// One mask per segment (`number` = Segment Number, `name` = label),
    /// sorted by segment number, columns-fastest on `geometry`.
    pub structures: StructureSet,
}

fn seg_error(message: impl Into<String>) -> DicomError {
    DicomError::Segmentation(message.into())
}

/// Import a SEG file. `ct` is the referenced CT grid, when available.
pub fn import_seg(
    path: &Path,
    ct: Option<&CtVolume>,
    options: &SegImportOptions,
) -> Result<SegmentationImport> {
    let obj = open_file(path).map_err(|source| DicomError::Read {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    seg_from_object(&obj, path, ct, options)
}

/// In-memory variant of [`import_seg`].
pub fn import_seg_bytes(
    bytes: &[u8],
    ct: Option<&CtVolume>,
    options: &SegImportOptions,
) -> Result<SegmentationImport> {
    let label = Path::new("seg.dcm");
    let obj = DefaultDicomObject::from_reader(std::io::Cursor::new(bytes)).map_err(|source| {
        DicomError::Read {
            path: label.to_path_buf(),
            source: Box::new(source),
        }
    })?;
    seg_from_object(&obj, label, ct, options)
}

struct Frame {
    segment: i32,
    position: [f64; 3],
    orientation: [f64; 6],
    /// Per-pixel membership fraction in [0, 1], rows*columns, row-major.
    fraction: Vec<f32>,
}

fn optional_items(obj: &InMemDicomObject, tag: Tag) -> Option<&[InMemDicomObject]> {
    obj.get(tag).and_then(|e| e.items())
}

fn opt_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.get(tag)
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim_end_matches([' ', '\0']).to_owned())
        .filter(|s| !s.is_empty())
}

fn seg_from_object(
    obj: &DefaultDicomObject,
    path: &Path,
    ct: Option<&CtVolume>,
    options: &SegImportOptions,
) -> Result<SegmentationImport> {
    let ts = obj.meta().transfer_syntax();
    if ts != uids::EXPLICIT_VR_LITTLE_ENDIAN && ts != uids::IMPLICIT_VR_LITTLE_ENDIAN {
        return Err(attribute_error(
            path,
            "Transfer Syntax UID",
            format!("only uncompressed little-endian SEG is supported, found {ts}"),
        ));
    }
    require_string(
        obj,
        path,
        tags::SOP_CLASS_UID,
        "SOP Class UID",
        uids::SEGMENTATION_STORAGE,
    )?;
    require_string(obj, path, tags::MODALITY, "Modality", "SEG")?;
    let sop_instance_uid = string(obj, path, tags::SOP_INSTANCE_UID, "SOP Instance UID")?;
    let frame_of_reference_uid = string(
        obj,
        path,
        tags::FRAME_OF_REFERENCE_UID,
        "Frame of Reference UID",
    )?;
    if !(0.0..=1.0).contains(&options.fractional_threshold) || options.fractional_threshold.is_nan()
    {
        return Err(seg_error("fractional threshold must lie in [0, 1]"));
    }
    if let Some(ct) = ct {
        if frame_of_reference_uid != ct.frame_of_reference_uid {
            return Err(seg_error(format!(
                "Frame of Reference UID {frame_of_reference_uid} does not match CT {}",
                ct.frame_of_reference_uid
            )));
        }
        // ReferencedSeriesSequence, when present, must name the CT series.
        if let Some(items) = optional_items(obj, tags::REFERENCED_SERIES_SEQUENCE) {
            let uids_seen: BTreeSet<String> = items
                .iter()
                .filter_map(|i| opt_string(i, tags::SERIES_INSTANCE_UID))
                .collect();
            if !uids_seen.is_empty() && !uids_seen.contains(&ct.series_instance_uid) {
                return Err(seg_error(format!(
                    "referenced series {uids_seen:?} does not include CT series {}",
                    ct.series_instance_uid
                )));
            }
        }
    }

    let segmentation_type =
        match string(obj, path, tags::SEGMENTATION_TYPE, "Segmentation Type")?.as_str() {
            "BINARY" => SegmentationType::Binary,
            "FRACTIONAL" => SegmentationType::Fractional,
            other => {
                return Err(seg_error(format!(
                    "Segmentation Type {other:?} is not supported; supported: BINARY, FRACTIONAL"
                )));
            }
        };
    let max_fractional = match segmentation_type {
        SegmentationType::Fractional => {
            let m = integer(
                obj,
                path,
                tags::MAXIMUM_FRACTIONAL_VALUE,
                "Maximum Fractional Value",
            )?;
            if m <= 0 {
                return Err(seg_error("Maximum Fractional Value must be positive"));
            }
            f64::from(m)
        }
        SegmentationType::Binary => 1.0,
    };

    // Segments.
    let mut segments: BTreeMap<i32, SegmentInfo> = BTreeMap::new();
    let mut labels = BTreeSet::new();
    for item in sequence(obj, path, tags::SEGMENT_SEQUENCE, "Segment Sequence")? {
        let number = integer(item, path, tags::SEGMENT_NUMBER, "Segment Number")?;
        let label = string(item, path, tags::SEGMENT_LABEL, "Segment Label")?;
        if label.is_empty() {
            return Err(seg_error(format!("segment {number} has an empty label")));
        }
        if !labels.insert(label.clone()) {
            return Err(seg_error(format!("duplicate Segment Label {label:?}")));
        }
        let info = SegmentInfo {
            number,
            label,
            description: opt_string(item, tags::SEGMENT_DESCRIPTION),
            algorithm_type: opt_string(item, tags::SEGMENT_ALGORITHM_TYPE),
            frame_count: 0,
        };
        if segments.insert(number, info).is_some() {
            return Err(seg_error(format!("duplicate Segment Number {number}")));
        }
    }
    if segments.is_empty() {
        return Err(seg_error("Segment Sequence is empty"));
    }

    // Pixel geometry.
    let rows = integer(obj, path, tags::ROWS, "Rows")?;
    let columns = integer(obj, path, tags::COLUMNS, "Columns")?;
    let frame_total = integer(obj, path, tags::NUMBER_OF_FRAMES, "Number of Frames")?;
    let bits = integer(obj, path, tags::BITS_ALLOCATED, "Bits Allocated")?;
    if rows <= 0 || columns <= 0 || frame_total <= 0 {
        return Err(seg_error(
            "Rows, Columns and Number of Frames must be positive",
        ));
    }
    let (rows, columns, frame_total) = (rows as usize, columns as usize, frame_total as usize);
    let plane = rows
        .checked_mul(columns)
        .ok_or_else(|| seg_error("frame size overflows"))?;
    let pixel_count = plane
        .checked_mul(frame_total)
        .ok_or_else(|| seg_error("pixel count overflows"))?;
    match (segmentation_type, bits) {
        (SegmentationType::Binary, 1) | (SegmentationType::Fractional, 8 | 16) => {}
        (kind, other) => {
            return Err(seg_error(format!(
                "Bits Allocated {other} is not valid for {kind:?} segmentation"
            )));
        }
    }
    if let Some(Ok(lossy)) = obj
        .get(tags::LOSSY_IMAGE_COMPRESSION)
        .map(|e| e.to_str().map(|s| s.trim().to_owned()))
        && lossy == "01"
    {
        return Err(seg_error(
            "lossy-compressed SEG pixel data is not supported",
        ));
    }
    let pixel_bytes = obj
        .element(tags::PIXEL_DATA)
        .map_err(|e| attribute_error(path, "Pixel Data", e.to_string()))?
        .to_bytes()
        .map_err(|e| attribute_error(path, "Pixel Data", e.to_string()))?;
    let needed = match bits {
        1 => pixel_count.div_ceil(8),
        8 => pixel_count,
        _ => pixel_count * 2,
    };
    if pixel_bytes.len() < needed {
        return Err(seg_error(format!(
            "Pixel Data holds {} bytes, {needed} required for {frame_total} frames",
            pixel_bytes.len()
        )));
    }

    // Functional groups.
    let shared = optional_items(obj, tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE)
        .and_then(|items| items.first());
    let shared_orientation = shared.and_then(plane_orientation);
    let (shared_pixel_spacing, shared_thickness, shared_spacing_between) = match shared
        .and_then(|s| optional_items(s, tags::PIXEL_MEASURES_SEQUENCE))
        .and_then(|i| i.first())
    {
        Some(pm) => (
            pm.get(tags::PIXEL_SPACING)
                .and_then(|e| e.to_multi_float64().ok()),
            pm.get(tags::SLICE_THICKNESS)
                .and_then(|e| e.to_float64().ok()),
            pm.get(tags::SPACING_BETWEEN_SLICES)
                .and_then(|e| e.to_float64().ok()),
        ),
        None => (None, None, None),
    };
    let per_frame = sequence(
        obj,
        path,
        tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE,
        "Per-frame Functional Groups Sequence",
    )?;
    if per_frame.len() != frame_total {
        return Err(seg_error(format!(
            "Per-frame Functional Groups Sequence has {} items for {frame_total} frames",
            per_frame.len()
        )));
    }
    let pixel_spacing = match shared_pixel_spacing {
        Some(v) if v.len() == 2 => [v[0], v[1]],
        _ => {
            // Fall back to a per-frame pixel measure on the first frame.
            per_frame
                .first()
                .and_then(|f| optional_items(f, tags::PIXEL_MEASURES_SEQUENCE))
                .and_then(|i| i.first())
                .and_then(|pm| pm.get(tags::PIXEL_SPACING))
                .and_then(|e| e.to_multi_float64().ok())
                .filter(|v| v.len() == 2)
                .map(|v| [v[0], v[1]])
                .ok_or_else(|| attribute_error(path, "Pixel Spacing", "not found"))?
        }
    };
    if pixel_spacing.iter().any(|v| !v.is_finite() || *v <= 0.0) {
        return Err(seg_error("Pixel Spacing must be positive"));
    }

    let mut frames = Vec::with_capacity(frame_total);
    for (index, item) in per_frame.iter().enumerate() {
        let segment_item = optional_items(item, tags::SEGMENT_IDENTIFICATION_SEQUENCE)
            .and_then(|i| i.first())
            .ok_or_else(|| {
                seg_error(format!(
                    "frame {index} has no Segment Identification Sequence"
                ))
            })?;
        let segment = integer(
            segment_item,
            path,
            tags::REFERENCED_SEGMENT_NUMBER,
            "Referenced Segment Number",
        )?;
        let info = segments.get_mut(&segment).ok_or_else(|| {
            seg_error(format!(
                "frame {index} references undefined Segment Number {segment}"
            ))
        })?;
        info.frame_count += 1;
        let position_item = optional_items(item, tags::PLANE_POSITION_SEQUENCE)
            .and_then(|i| i.first())
            .ok_or_else(|| seg_error(format!("frame {index} has no Plane Position Sequence")))?;
        let position = floats(
            position_item,
            path,
            tags::IMAGE_POSITION_PATIENT,
            "Image Position (Patient)",
        )?;
        if position.len() != 3 || position.iter().any(|v| !v.is_finite()) {
            return Err(seg_error(format!("frame {index} has a bad Image Position")));
        }
        let orientation = plane_orientation(item)
            .or(shared_orientation)
            .ok_or_else(|| {
                seg_error(format!(
                    "frame {index} has no Plane Orientation (per-frame or shared)"
                ))
            })?;
        // Fractions.
        let base = index * plane;
        let fraction: Vec<f32> = match bits {
            1 => (0..plane)
                .map(|p| {
                    let bit = base + p;
                    ((pixel_bytes[bit / 8] >> (bit % 8)) & 1) as f32
                })
                .collect(),
            8 => (0..plane)
                .map(|p| (f64::from(pixel_bytes[base + p]) / max_fractional) as f32)
                .collect(),
            _ => (0..plane)
                .map(|p| {
                    let o = 2 * (base + p);
                    let v = u16::from_le_bytes([pixel_bytes[o], pixel_bytes[o + 1]]);
                    (f64::from(v) / max_fractional) as f32
                })
                .collect(),
        };
        frames.push(Frame {
            segment,
            position: [position[0], position[1], position[2]],
            orientation,
            fraction,
        });
    }
    if let Some((number, _)) = segments.iter().find(|(_, s)| s.frame_count == 0) {
        return Err(seg_error(format!("segment {number} has no frames")));
    }
    // All frames must share one in-plane orientation.
    let orientation = frames[0].orientation;
    if frames.iter().any(|f| {
        f.orientation
            .iter()
            .zip(orientation)
            .any(|(a, b)| (a - b).abs() > 1.0e-5)
    }) {
        return Err(seg_error("frames have differing Image Orientation"));
    }

    // Grid.
    let geometry = match ct {
        Some(ct) => ct.geometry.clone(),
        None => derive_grid(
            &frames,
            orientation,
            pixel_spacing,
            [columns, rows],
            shared_thickness,
            shared_spacing_between,
        )?,
    };
    let voxel_count = geometry
        .voxel_count()
        .map_err(|e| DicomError::Geometry(e.to_string()))?;
    if voxel_count > MAX_VOXELS {
        return Err(seg_error("segmentation grid is too large"));
    }

    let mut masks: BTreeMap<i32, Vec<bool>> = segments
        .keys()
        .map(|n| (*n, vec![false; voxel_count]))
        .collect();
    let mut seen: BTreeSet<(i32, i64)> = BTreeSet::new();
    let [nx, ny, nz] = geometry.shape.map(|v| v as usize);
    let axes = [
        [
            geometry.direction[0],
            geometry.direction[3],
            geometry.direction[6],
        ],
        [
            geometry.direction[1],
            geometry.direction[4],
            geometry.direction[7],
        ],
        [
            geometry.direction[2],
            geometry.direction[5],
            geometry.direction[8],
        ],
    ];
    let row_dir = [orientation[0], orientation[1], orientation[2]];
    let col_dir = [orientation[3], orientation[4], orientation[5]];
    // Index-space steps of one SEG column / row step, and the pixel (0,0)
    // index of each frame; both must sit on the grid lattice.
    let local_of = |world: [f64; 3]| -> [f64; 3] {
        let delta = [
            world[0] - geometry.origin_mm[0],
            world[1] - geometry.origin_mm[1],
            world[2] - geometry.origin_mm[2],
        ];
        [
            dot(delta, axes[0]) / geometry.spacing_mm[0],
            dot(delta, axes[1]) / geometry.spacing_mm[1],
            dot(delta, axes[2]) / geometry.spacing_mm[2],
        ]
    };
    let step_c = [
        dot(row_dir, axes[0]) * pixel_spacing[1] / geometry.spacing_mm[0],
        dot(row_dir, axes[1]) * pixel_spacing[1] / geometry.spacing_mm[1],
        dot(row_dir, axes[2]) * pixel_spacing[1] / geometry.spacing_mm[2],
    ];
    let step_r = [
        dot(col_dir, axes[0]) * pixel_spacing[0] / geometry.spacing_mm[0],
        dot(col_dir, axes[1]) * pixel_spacing[0] / geometry.spacing_mm[1],
        dot(col_dir, axes[2]) * pixel_spacing[0] / geometry.spacing_mm[2],
    ];
    let unit = |v: [f64; 3], axis: usize| {
        (0..3).all(|a| (v[a] - if a == axis { 1.0 } else { 0.0 }).abs() <= LATTICE_TOLERANCE)
    };
    if !unit(step_c, 0) || !unit(step_r, 1) {
        return Err(seg_error(
            "SEG in-plane axes or pixel spacing do not coincide with the target grid's \
             voxel lattice; resample the segmentation to the CT grid first",
        ));
    }
    let threshold = match segmentation_type {
        SegmentationType::Binary => None,
        SegmentationType::Fractional => Some(options.fractional_threshold),
    };
    for (index, frame) in frames.iter().enumerate() {
        let local = local_of(frame.position);
        let rounded = local.map(f64::round);
        if (0..3).any(|a| (local[a] - rounded[a]).abs() > LATTICE_TOLERANCE) {
            return Err(seg_error(format!(
                "frame {index} (segment {}) does not lie on the grid's voxel lattice \
                 (index {:.3}, {:.3}, {:.3})",
                frame.segment, local[0], local[1], local[2]
            )));
        }
        let (i0, j0, k) = (rounded[0] as i64, rounded[1] as i64, rounded[2] as i64);
        if k < 0 || k >= nz as i64 {
            return Err(seg_error(format!(
                "frame {index} (segment {}) lies outside the grid's slice range",
                frame.segment
            )));
        }
        if !seen.insert((frame.segment, k)) {
            return Err(seg_error(format!(
                "segment {} has more than one frame on slice {k}",
                frame.segment
            )));
        }
        let mask = masks.get_mut(&frame.segment).expect("segment validated");
        for r in 0..rows {
            for c in 0..columns {
                let fraction = f64::from(frame.fraction[r * columns + c]);
                let on = match threshold {
                    Some(t) => fraction >= t && fraction > 0.0,
                    None => fraction >= 0.5,
                };
                if !on {
                    continue;
                }
                let (i, j) = (i0 + c as i64, j0 + r as i64);
                if i < 0 || j < 0 || i >= nx as i64 || j >= ny as i64 {
                    return Err(seg_error(format!(
                        "frame {index} (segment {}) has set pixels outside the grid",
                        frame.segment
                    )));
                }
                mask[k as usize * nx * ny + j as usize * nx + i as usize] = true;
            }
        }
    }

    let rois = segments
        .iter()
        .map(|(number, info)| RoiMask {
            number: *number,
            name: info.label.clone(),
            voxels: masks.remove(number).expect("mask exists per segment"),
        })
        .collect();
    Ok(SegmentationImport {
        geometry,
        frame_of_reference_uid: frame_of_reference_uid.clone(),
        sop_instance_uid,
        segmentation_type,
        fractional_threshold: threshold,
        segments: segments.into_values().collect(),
        structures: StructureSet {
            frame_of_reference_uid,
            rois,
        },
    })
}

/// Image Orientation (Patient) from a Plane Orientation Sequence, if any.
fn plane_orientation(group: &InMemDicomObject) -> Option<[f64; 6]> {
    let item = optional_items(group, tags::PLANE_ORIENTATION_SEQUENCE)?.first()?;
    let v = item
        .get(tags::IMAGE_ORIENTATION_PATIENT)?
        .to_multi_float64()
        .ok()?;
    (v.len() == 6 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
}

fn derive_grid(
    frames: &[Frame],
    orientation: [f64; 6],
    pixel_spacing: [f64; 2],
    [columns, rows]: [usize; 2],
    slice_thickness: Option<f64>,
    spacing_between: Option<f64>,
) -> Result<GridGeometry> {
    let u = [orientation[0], orientation[1], orientation[2]];
    let v = [orientation[3], orientation[4], orientation[5]];
    let norm = |a: [f64; 3]| dot(a, a).sqrt();
    if (norm(u) - 1.0).abs() > 1.0e-3 || (norm(v) - 1.0).abs() > 1.0e-3 || dot(u, v).abs() > 1.0e-3
    {
        return Err(seg_error("Image Orientation is not an orthonormal pair"));
    }
    let n = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let reference = frames[0].position;
    let mut stations: Vec<(f64, [f64; 3])> = Vec::new();
    for frame in frames {
        let delta = [
            frame.position[0] - reference[0],
            frame.position[1] - reference[1],
            frame.position[2] - reference[2],
        ];
        let t = dot(delta, n);
        let in_plane = [
            delta[0] - t * n[0],
            delta[1] - t * n[1],
            delta[2] - t * n[2],
        ];
        if norm(in_plane) > 0.01 {
            return Err(seg_error(
                "frames are not stacked on one in-plane lattice (Image Position drifts in-plane)",
            ));
        }
        stations.push((t, frame.position));
    }
    stations.sort_by(|a, b| a.0.total_cmp(&b.0));
    stations.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0e-3);
    let spacing = if stations.len() >= 2 {
        let smallest = stations
            .windows(2)
            .map(|w| w[1].0 - w[0].0)
            .fold(f64::INFINITY, f64::min);
        for w in stations.windows(2) {
            let steps = (w[1].0 - w[0].0) / smallest;
            if (steps - steps.round()).abs() > 1.0e-2 {
                return Err(seg_error(
                    "frame slice positions are not on a uniform lattice",
                ));
            }
        }
        smallest
    } else {
        spacing_between
            .or(slice_thickness)
            .filter(|s| s.is_finite() && *s > 0.0)
            .ok_or_else(|| {
                seg_error("a single-slice SEG needs Slice Thickness or Spacing Between Slices")
            })?
    };
    let first = stations[0];
    let last = stations[stations.len() - 1];
    let nz = ((last.0 - first.0) / spacing).round() as usize + 1;
    if nz > 65_536 || columns.saturating_mul(rows).saturating_mul(nz) > MAX_VOXELS {
        return Err(seg_error("segmentation grid is too large"));
    }
    Ok(GridGeometry {
        shape: [columns as u32, rows as u32, nz as u32],
        spacing_mm: [pixel_spacing[1], pixel_spacing[0], spacing],
        origin_mm: first.1,
        direction: [u[0], v[0], n[0], u[1], v[1], n[1], u[2], v[2], n[2]],
    })
}
