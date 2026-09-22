// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use dicom_core::Tag;
use dicom_dictionary_std::{tags, uids};
use dicom_object::{DefaultDicomObject, InMemDicomObject, open_file};

use crate::{CtVolume, DicomError, Result};

const PLANE_TOLERANCE_MM: f64 = 1.0e-3;

#[derive(Debug, Clone, PartialEq)]
pub struct RoiMask {
    pub number: i32,
    pub name: String,
    /// Boolean mask in the same columns-fastest order as `CtVolume`.
    pub voxels: Vec<bool>,
}

impl RoiMask {
    #[must_use]
    pub fn voxel_count(&self) -> usize {
        self.voxels.iter().filter(|value| **value).count()
    }

    #[must_use]
    pub fn volume_cm3(&self, ct: &CtVolume) -> f64 {
        let voxel_volume_mm3: f64 = ct.geometry.spacing_mm.iter().product();
        self.voxel_count() as f64 * voxel_volume_mm3 / 1_000.0
    }

    #[must_use]
    pub fn centroid_lps_mm(&self, ct: &CtVolume) -> Option<[f64; 3]> {
        let columns = ct.geometry.shape[0] as usize;
        let rows = ct.geometry.shape[1] as usize;
        let mut count = 0_u64;
        let mut sum = [0.0; 3];
        for (index, included) in self.voxels.iter().copied().enumerate() {
            if !included {
                continue;
            }
            let slice = index / (columns * rows);
            let within_slice = index % (columns * rows);
            let row = within_slice / columns;
            let column = within_slice % columns;
            let center = ct.voxel_center_lps_mm(column as u32, row as u32, slice as u32);
            for axis in 0..3 {
                sum[axis] += center[axis];
            }
            count += 1;
        }
        (count > 0).then(|| {
            let denominator = count as f64;
            [
                sum[0] / denominator,
                sum[1] / denominator,
                sum[2] / denominator,
            ]
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructureSet {
    pub frame_of_reference_uid: String,
    /// ROIs sorted by ROI Number.
    pub rois: Vec<RoiMask>,
}

impl StructureSet {
    #[must_use]
    pub fn roi(&self, name: &str) -> Option<&RoiMask> {
        self.rois.iter().find(|roi| roi.name == name)
    }
}

#[derive(Debug)]
struct RoiDefinition {
    name: String,
    frame_of_reference_uid: String,
}

/// Import an RT Structure Set onto an already validated CT geometry.
///
/// R1 accepts `CLOSED_PLANAR` contours with one polygon per ROI per image
/// plane. Multi-polygon topology is rejected until hole and XOR semantics are
/// implemented explicitly; silently filling those contours would be unsafe.
pub fn import_rtstruct(path: &Path, ct: &CtVolume) -> Result<StructureSet> {
    let obj = open_file(path).map_err(|source| DicomError::Read {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    rtstruct_from_object(&obj, path, ct)
}

/// In-memory variant of [`import_rtstruct`] for hosts without a
/// filesystem; the same topology gates run on the parsed object.
pub fn import_rtstruct_bytes(bytes: &[u8], ct: &CtVolume) -> Result<StructureSet> {
    let label = Path::new("rtstruct.dcm");
    let obj = DefaultDicomObject::from_reader(std::io::Cursor::new(bytes)).map_err(|source| {
        DicomError::Read {
            path: label.to_path_buf(),
            source: Box::new(source),
        }
    })?;
    rtstruct_from_object(&obj, label, ct)
}

fn rtstruct_from_object(
    obj: &DefaultDicomObject,
    path: &Path,
    ct: &CtVolume,
) -> Result<StructureSet> {
    if obj.meta().transfer_syntax() != uids::EXPLICIT_VR_LITTLE_ENDIAN {
        return Err(attribute_error(
            path,
            "Transfer Syntax UID",
            format!(
                "expected {}, found {}",
                uids::EXPLICIT_VR_LITTLE_ENDIAN,
                obj.meta().transfer_syntax()
            ),
        ));
    }
    require_string(
        obj,
        path,
        tags::SOP_CLASS_UID,
        "SOP Class UID",
        uids::RT_STRUCTURE_SET_STORAGE,
    )?;
    require_string(obj, path, tags::MODALITY, "Modality", "RTSTRUCT")?;
    require_string(
        obj,
        path,
        tags::STUDY_INSTANCE_UID,
        "Study Instance UID",
        &ct.study_instance_uid,
    )?;

    let frame_uid = validate_references(obj, path, ct)?;
    let definitions = read_definitions(obj, path, &frame_uid)?;
    let voxel_count = ct
        .geometry
        .voxel_count()
        .map_err(|error| DicomError::Geometry(error.to_string()))?;
    let mut masks: BTreeMap<i32, Vec<bool>> = definitions
        .keys()
        .map(|number| (*number, vec![false; voxel_count]))
        .collect();
    let mut observed_contour_rois = BTreeSet::new();

    let roi_contours = sequence(
        obj,
        path,
        tags::ROI_CONTOUR_SEQUENCE,
        "ROI Contour Sequence",
    )?;
    for roi_contour in roi_contours {
        let roi_number = integer(
            roi_contour,
            path,
            tags::REFERENCED_ROI_NUMBER,
            "Referenced ROI Number",
        )?;
        if !definitions.contains_key(&roi_number) {
            return Err(DicomError::StructureSet(format!(
                "ROI Contour Sequence references undefined ROI Number {roi_number}"
            )));
        }
        if !observed_contour_rois.insert(roi_number) {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {roi_number} occurs more than once in ROI Contour Sequence"
            )));
        }
        let contours = sequence(
            roi_contour,
            path,
            tags::CONTOUR_SEQUENCE,
            "Contour Sequence",
        )?;
        if contours.is_empty() {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {roi_number} has no contours"
            )));
        }

        let mut observed_planes = BTreeSet::new();
        for contour in contours {
            require_string(
                contour,
                path,
                tags::CONTOUR_GEOMETRIC_TYPE,
                "Contour Geometric Type",
                "CLOSED_PLANAR",
            )?;
            let point_count = integer(
                contour,
                path,
                tags::NUMBER_OF_CONTOUR_POINTS,
                "Number of Contour Points",
            )?;
            if point_count < 3 {
                return Err(DicomError::StructureSet(format!(
                    "ROI Number {roi_number} has a contour with fewer than three points"
                )));
            }
            let world_coordinates = floats(contour, path, tags::CONTOUR_DATA, "Contour Data")?;
            if world_coordinates.len() != point_count as usize * 3 {
                return Err(DicomError::StructureSet(format!(
                    "ROI Number {roi_number} declares {point_count} contour points but contains {} coordinates",
                    world_coordinates.len()
                )));
            }

            let (slice_index, polygon) = contour_to_grid(&world_coordinates, ct, roi_number)?;
            if !observed_planes.insert(slice_index) {
                return Err(DicomError::StructureSet(format!(
                    "ROI Number {roi_number} contains multiple CLOSED_PLANAR contours on slice {slice_index}; R1 rejects ambiguous hole topology"
                )));
            }
            validate_contour_image_reference(contour, path, ct, slice_index)?;
            rasterize_polygon(
                masks
                    .get_mut(&roi_number)
                    .expect("mask exists for validated ROI number"),
                ct,
                slice_index,
                &polygon,
            );
        }
    }

    for roi_number in definitions.keys() {
        if !observed_contour_rois.contains(roi_number) {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {roi_number} has no ROI Contour Sequence item"
            )));
        }
    }

    let rois = definitions
        .into_iter()
        .map(|(number, definition)| RoiMask {
            number,
            name: definition.name,
            voxels: masks
                .remove(&number)
                .expect("mask exists for every ROI definition"),
        })
        .collect();

    Ok(StructureSet {
        frame_of_reference_uid: frame_uid,
        rois,
    })
}

fn validate_references(obj: &DefaultDicomObject, path: &Path, ct: &CtVolume) -> Result<String> {
    let frames = sequence(
        obj,
        path,
        tags::REFERENCED_FRAME_OF_REFERENCE_SEQUENCE,
        "Referenced Frame of Reference Sequence",
    )?;
    let frame = exactly_one(frames, "Referenced Frame of Reference Sequence")?;
    let frame_uid = string(
        frame,
        path,
        tags::FRAME_OF_REFERENCE_UID,
        "Frame of Reference UID",
    )?;
    if frame_uid != ct.frame_of_reference_uid {
        return Err(DicomError::StructureSet(format!(
            "Frame of Reference UID {frame_uid} does not match CT {}",
            ct.frame_of_reference_uid
        )));
    }

    let studies = sequence(
        frame,
        path,
        tags::RT_REFERENCED_STUDY_SEQUENCE,
        "RT Referenced Study Sequence",
    )?;
    let study = exactly_one(studies, "RT Referenced Study Sequence")?;
    require_string(
        study,
        path,
        tags::REFERENCED_SOP_INSTANCE_UID,
        "Referenced SOP Instance UID",
        &ct.study_instance_uid,
    )?;

    let series_items = sequence(
        study,
        path,
        tags::RT_REFERENCED_SERIES_SEQUENCE,
        "RT Referenced Series Sequence",
    )?;
    let series = exactly_one(series_items, "RT Referenced Series Sequence")?;
    require_string(
        series,
        path,
        tags::SERIES_INSTANCE_UID,
        "Series Instance UID",
        &ct.series_instance_uid,
    )?;

    let images = sequence(
        series,
        path,
        tags::CONTOUR_IMAGE_SEQUENCE,
        "Contour Image Sequence",
    )?;
    let referenced_uids: BTreeSet<_> = images
        .iter()
        .map(|image| {
            require_string(
                image,
                path,
                tags::REFERENCED_SOP_CLASS_UID,
                "Referenced SOP Class UID",
                uids::CT_IMAGE_STORAGE,
            )?;
            string(
                image,
                path,
                tags::REFERENCED_SOP_INSTANCE_UID,
                "Referenced SOP Instance UID",
            )
        })
        .collect::<Result<_>>()?;
    let ct_uids: BTreeSet<_> = ct.slice_sop_instance_uids.iter().cloned().collect();
    if referenced_uids != ct_uids || images.len() != ct.slice_sop_instance_uids.len() {
        return Err(DicomError::StructureSet(
            "top-level Contour Image Sequence does not reference exactly the imported CT slices"
                .into(),
        ));
    }
    Ok(frame_uid)
}

fn read_definitions(
    obj: &DefaultDicomObject,
    path: &Path,
    frame_uid: &str,
) -> Result<BTreeMap<i32, RoiDefinition>> {
    let items = sequence(
        obj,
        path,
        tags::STRUCTURE_SET_ROI_SEQUENCE,
        "Structure Set ROI Sequence",
    )?;
    if items.is_empty() {
        return Err(DicomError::StructureSet(
            "Structure Set ROI Sequence is empty".into(),
        ));
    }
    let mut definitions = BTreeMap::new();
    let mut names = BTreeSet::new();
    for item in items {
        let number = integer(item, path, tags::ROI_NUMBER, "ROI Number")?;
        let name = string(item, path, tags::ROI_NAME, "ROI Name")?;
        if name.is_empty() {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {number} has an empty ROI Name"
            )));
        }
        if !names.insert(name.clone()) {
            return Err(DicomError::StructureSet(format!(
                "duplicate ROI Name {name:?}"
            )));
        }
        let referenced_frame = string(
            item,
            path,
            tags::REFERENCED_FRAME_OF_REFERENCE_UID,
            "Referenced Frame of Reference UID",
        )?;
        if referenced_frame != frame_uid {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {number} references Frame of Reference UID {referenced_frame}, expected {frame_uid}"
            )));
        }
        if definitions
            .insert(
                number,
                RoiDefinition {
                    name,
                    frame_of_reference_uid: referenced_frame,
                },
            )
            .is_some()
        {
            return Err(DicomError::StructureSet(format!(
                "duplicate ROI Number {number}"
            )));
        }
    }
    // Read the stored field here so an accidental future mismatch cannot hide
    // behind an unused definition member.
    debug_assert!(
        definitions
            .values()
            .all(|definition| definition.frame_of_reference_uid == frame_uid)
    );
    Ok(definitions)
}

fn contour_to_grid(
    world_coordinates: &[f64],
    ct: &CtVolume,
    roi_number: i32,
) -> Result<(usize, Vec<[f64; 2]>)> {
    let g = &ct.geometry;
    let axes = [
        [g.direction[0], g.direction[3], g.direction[6]],
        [g.direction[1], g.direction[4], g.direction[7]],
        [g.direction[2], g.direction[5], g.direction[8]],
    ];
    let mut polygon = Vec::with_capacity(world_coordinates.len() / 3);
    let mut slice_index = None;
    for point in world_coordinates.chunks_exact(3) {
        if point.iter().any(|value| !value.is_finite()) {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {roi_number} contains non-finite Contour Data"
            )));
        }
        let delta = [
            point[0] - g.origin_mm[0],
            point[1] - g.origin_mm[1],
            point[2] - g.origin_mm[2],
        ];
        let local = [
            dot(delta, axes[0]) / g.spacing_mm[0],
            dot(delta, axes[1]) / g.spacing_mm[1],
            dot(delta, axes[2]) / g.spacing_mm[2],
        ];
        let rounded_slice = local[2].round();
        if (local[2] - rounded_slice).abs() * g.spacing_mm[2] > PLANE_TOLERANCE_MM {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {roi_number} contains a contour point off a CT slice plane"
            )));
        }
        if rounded_slice < 0.0 || rounded_slice >= f64::from(g.shape[2]) {
            return Err(DicomError::StructureSet(format!(
                "ROI Number {roi_number} contains a contour outside the CT slice range"
            )));
        }
        let current_slice = rounded_slice as usize;
        if let Some(expected) = slice_index {
            if current_slice != expected {
                return Err(DicomError::StructureSet(format!(
                    "ROI Number {roi_number} contains a non-planar contour"
                )));
            }
        } else {
            slice_index = Some(current_slice);
        }
        polygon.push([local[0], local[1]]);
    }
    Ok((
        slice_index.expect("point count is validated as at least three"),
        polygon,
    ))
}

fn validate_contour_image_reference(
    contour: &InMemDicomObject,
    path: &Path,
    ct: &CtVolume,
    slice_index: usize,
) -> Result<()> {
    let images = sequence(
        contour,
        path,
        tags::CONTOUR_IMAGE_SEQUENCE,
        "Contour Image Sequence",
    )?;
    let image = exactly_one(images, "per-contour Contour Image Sequence")?;
    require_string(
        image,
        path,
        tags::REFERENCED_SOP_CLASS_UID,
        "Referenced SOP Class UID",
        uids::CT_IMAGE_STORAGE,
    )?;
    require_string(
        image,
        path,
        tags::REFERENCED_SOP_INSTANCE_UID,
        "Referenced SOP Instance UID",
        &ct.slice_sop_instance_uids[slice_index],
    )
}

fn rasterize_polygon(mask: &mut [bool], ct: &CtVolume, slice: usize, polygon: &[[f64; 2]]) {
    let columns = ct.geometry.shape[0] as usize;
    let rows = ct.geometry.shape[1] as usize;
    let slice_offset = slice * rows * columns;
    for row in 0..rows {
        for column in 0..columns {
            if point_in_polygon([column as f64, row as f64], polygon) {
                mask[slice_offset + row * columns + column] = true;
            }
        }
    }
}

fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let left = polygon[previous];
        let right = polygon[current];
        if point_on_segment(point, left, right) {
            return true;
        }
        if (left[1] > point[1]) != (right[1] > point[1]) {
            let crossing_x =
                (right[0] - left[0]) * (point[1] - left[1]) / (right[1] - left[1]) + left[0];
            if point[0] < crossing_x {
                inside = !inside;
            }
        }
        previous = current;
    }
    inside
}

fn point_on_segment(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> bool {
    let cross =
        (point[0] - start[0]) * (end[1] - start[1]) - (point[1] - start[1]) * (end[0] - start[0]);
    if cross.abs() > 1.0e-10 {
        return false;
    }
    let dot =
        (point[0] - start[0]) * (point[0] - end[0]) + (point[1] - start[1]) * (point[1] - end[1]);
    dot <= 1.0e-10
}

fn sequence<'a>(
    obj: &'a InMemDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
) -> Result<&'a [InMemDicomObject]> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .items()
        .ok_or_else(|| attribute_error(path, name, "expected a data set sequence"))
}

fn exactly_one<'a>(items: &'a [InMemDicomObject], name: &str) -> Result<&'a InMemDicomObject> {
    if items.len() != 1 {
        return Err(DicomError::StructureSet(format!(
            "{name} contains {} items; expected exactly one",
            items.len()
        )));
    }
    Ok(&items[0])
}

fn string(obj: &InMemDicomObject, path: &Path, tag: Tag, name: &'static str) -> Result<String> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_str()
        .map(|value| value.trim_end_matches([' ', '\0']).to_owned())
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

fn require_string(
    obj: &InMemDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
    expected: &str,
) -> Result<()> {
    let observed = string(obj, path, tag, name)?;
    if observed != expected {
        return Err(attribute_error(
            path,
            name,
            format!("expected {expected:?}, found {observed:?}"),
        ));
    }
    Ok(())
}

fn integer(obj: &InMemDicomObject, path: &Path, tag: Tag, name: &'static str) -> Result<i32> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_int::<i32>()
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

fn floats(obj: &InMemDicomObject, path: &Path, tag: Tag, name: &'static str) -> Result<Vec<f64>> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_multi_float64()
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

fn attribute_error(path: &Path, attribute: &'static str, detail: impl Into<String>) -> DicomError {
    DicomError::Attribute {
        path: PathBuf::from(path),
        attribute,
        detail: detail.into(),
    }
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0].mul_add(right[0], left[1].mul_add(right[1], left[2] * right[2]))
}

#[cfg(test)]
mod tests {
    use super::point_in_polygon;

    #[test]
    fn rasterizer_uses_voxel_centers() {
        let square = [[-0.5, -0.5], [1.5, -0.5], [1.5, 1.5], [-0.5, 1.5]];
        assert!(point_in_polygon([0.0, 0.0], &square));
        assert!(point_in_polygon([1.0, 1.0], &square));
        assert!(!point_in_polygon([2.0, 1.0], &square));
    }
}
