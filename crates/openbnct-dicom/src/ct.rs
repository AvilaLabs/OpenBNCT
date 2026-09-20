// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dicom_core::Tag;
use dicom_core::value::{PrimitiveValue, Value};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{DefaultDicomObject, open_file};
use openbnct_core::{GridGeometry, RegionMask, ValidationError};

use crate::{DicomError, Result};

const DIRECTION_TOLERANCE: f64 = 1.0e-6;
const POSITION_TOLERANCE_MM: f64 = 1.0e-4;
const SPACING_TOLERANCE_MM: f64 = 1.0e-5;

/// A validated, regularly spaced CT series in DICOM LPS patient coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct CtVolume {
    pub geometry: GridGeometry,
    pub frame_of_reference_uid: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    /// SOP Instance UIDs ordered along the positive slice-normal axis.
    pub slice_sop_instance_uids: Vec<String>,
    /// Stored signed pixel samples, indexed with columns varying fastest, then
    /// rows, then slices. Apply `rescale_slope` and `rescale_intercept` to
    /// obtain the modality value.
    pub stored_pixels: Vec<i16>,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
}

impl CtVolume {
    /// Convert a stored pixel sample to its rescaled CT modality value.
    #[must_use]
    pub fn modality_value(&self, stored_pixel: i16) -> f64 {
        f64::from(stored_pixel).mul_add(self.rescale_slope, self.rescale_intercept)
    }

    /// Construct a `RegionMask` selecting voxels whose rescaled modality
    /// value falls inside `[minimum, maximum]` (inclusive). For CT this is
    /// an HU window — e.g. a configurable threshold range for building a
    /// limiting-organ region. Bounds must be finite with
    /// `minimum <= maximum`, and the window must select at least one voxel.
    pub fn threshold_mask(
        &self,
        name: impl Into<String>,
        minimum: f64,
        maximum: f64,
    ) -> std::result::Result<RegionMask, ValidationError> {
        if !minimum.is_finite() || !maximum.is_finite() || minimum > maximum {
            return Err(ValidationError::InvalidThresholdWindow { minimum, maximum });
        }
        let mask = RegionMask {
            name: name.into(),
            voxels: self
                .stored_pixels
                .iter()
                .map(|sample| {
                    let value = self.modality_value(*sample);
                    value >= minimum && value <= maximum
                })
                .collect(),
        };
        if mask.included_voxel_count() == 0 {
            return Err(ValidationError::EmptyMask(mask.name));
        }
        Ok(mask)
    }

    /// Return the world-coordinate center of one voxel in DICOM LPS mm.
    #[must_use]
    pub fn voxel_center_lps_mm(&self, column: u32, row: u32, slice: u32) -> [f64; 3] {
        self.geometry
            .voxel_center_lps_mm([column, row, slice])
            .expect("CT voxel indices originate from validated volume bounds")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RawSlice {
    pub(crate) path: PathBuf,
    pub(crate) study_instance_uid: String,
    pub(crate) series_instance_uid: String,
    pub(crate) frame_of_reference_uid: String,
    pub(crate) sop_instance_uid: String,
    pub(crate) rows: u16,
    pub(crate) columns: u16,
    pub(crate) pixel_spacing: [f64; 2],
    pub(crate) image_position: [f64; 3],
    pub(crate) image_orientation: [f64; 6],
    pub(crate) projection: f64,
    /// i32 so unsigned 16-bit PET samples (0..=65535) stay exact.
    pub(crate) stored_pixels: Vec<i32>,
    pub(crate) rescale_slope: f64,
    pub(crate) rescale_intercept: f64,
}

/// Import a single-frame CT series from any input file order.
///
/// The importer deliberately accepts only native, signed 16-bit Explicit VR
/// Little Endian CT in this first boundary. Unsupported encodings fail closed
/// instead of being guessed. Slices are ordered from their patient-space image
/// positions, never from filenames or Instance Number.
/// The shared volume-assembly result: geometry plus flattened pixels.
pub(crate) struct AssembledSeries {
    pub(crate) geometry: GridGeometry,
    pub(crate) frame_of_reference_uid: String,
    pub(crate) study_instance_uid: String,
    pub(crate) series_instance_uid: String,
    pub(crate) slice_sop_instance_uids: Vec<String>,
    pub(crate) stored_pixels: Vec<i32>,
    pub(crate) rescale_slope: f64,
    pub(crate) rescale_intercept: f64,
}

/// Validate a parsed slice set for geometric consistency and assemble the
/// shared grid — modality-agnostic; the caller's slice reader supplies
/// `RawSlice` values parsed under its own SOP-class/modality gates.
pub(crate) fn assemble_series(mut slices: Vec<RawSlice>) -> Result<AssembledSeries> {
    let reference = slices[0].clone();
    let x_axis = array3(&reference.image_orientation[0..3]);
    let y_axis = array3(&reference.image_orientation[3..6]);
    validate_axes(x_axis, y_axis, &reference.path)?;
    let normal = normalize(cross(x_axis, y_axis));

    for slice in &mut slices {
        validate_consistency(&reference, slice)?;
        let slice_x = array3(&slice.image_orientation[0..3]);
        let slice_y = array3(&slice.image_orientation[3..6]);
        validate_axes(slice_x, slice_y, &slice.path)?;
        if !array_close(slice_x, x_axis, DIRECTION_TOLERANCE)
            || !array_close(slice_y, y_axis, DIRECTION_TOLERANCE)
        {
            return Err(DicomError::InconsistentSeries(format!(
                "{} has a different Image Orientation (Patient)",
                slice.path.display()
            )));
        }
        slice.projection = dot(slice.image_position, normal);
    }

    slices.sort_by(|left, right| left.projection.total_cmp(&right.projection));

    let mut seen_uids = BTreeSet::new();
    for slice in &slices {
        if !seen_uids.insert(slice.sop_instance_uid.as_str()) {
            return Err(DicomError::InconsistentSeries(format!(
                "duplicate SOP Instance UID {}",
                slice.sop_instance_uid
            )));
        }
    }

    let origin = slices[0].image_position;
    let first_projection = slices[0].projection;
    for slice in &slices {
        let along_normal = slice.projection - first_projection;
        let expected = add(origin, scale(normal, along_normal));
        if norm(sub(slice.image_position, expected)) > POSITION_TOLERANCE_MM {
            return Err(DicomError::Geometry(format!(
                "{} is shifted within the image plane; tilted or sheared CT stacks are not yet supported",
                slice.path.display()
            )));
        }
    }

    let slice_spacing = slices[1].projection - slices[0].projection;
    if !slice_spacing.is_finite() || slice_spacing <= SPACING_TOLERANCE_MM {
        return Err(DicomError::Geometry(
            "duplicate or non-increasing slice positions".into(),
        ));
    }
    for pair in slices.windows(2) {
        let observed = pair[1].projection - pair[0].projection;
        if !close(observed, slice_spacing, SPACING_TOLERANCE_MM) {
            return Err(DicomError::Geometry(format!(
                "non-uniform slice spacing: expected {slice_spacing:.9} mm, observed {observed:.9} mm between {} and {}",
                pair[0].path.display(),
                pair[1].path.display()
            )));
        }
    }

    let columns = u32::from(reference.columns);
    let rows = u32::from(reference.rows);
    let slice_count = u32::try_from(slices.len())
        .map_err(|_| DicomError::Geometry("slice count exceeds u32".into()))?;
    let geometry = GridGeometry {
        shape: [columns, rows, slice_count],
        spacing_mm: [
            reference.pixel_spacing[1],
            reference.pixel_spacing[0],
            slice_spacing,
        ],
        origin_mm: origin,
        // Columns are voxel-axis direction vectors; storage is row-major.
        direction: [
            x_axis[0], y_axis[0], normal[0], x_axis[1], y_axis[1], normal[1], x_axis[2], y_axis[2],
            normal[2],
        ],
    };
    geometry
        .voxel_count()
        .map_err(|error| DicomError::Geometry(error.to_string()))?;

    let mut stored_pixels = Vec::with_capacity(
        usize::from(reference.rows) * usize::from(reference.columns) * slices.len(),
    );
    let mut slice_sop_instance_uids = Vec::with_capacity(slices.len());
    for slice in slices {
        stored_pixels.extend(slice.stored_pixels);
        slice_sop_instance_uids.push(slice.sop_instance_uid);
    }

    Ok(AssembledSeries {
        geometry,
        frame_of_reference_uid: reference.frame_of_reference_uid.clone(),
        study_instance_uid: reference.study_instance_uid.clone(),
        series_instance_uid: reference.series_instance_uid.clone(),
        slice_sop_instance_uids,
        stored_pixels,
        rescale_slope: reference.rescale_slope,
        rescale_intercept: reference.rescale_intercept,
    })
}

/// Import a single-frame CT series from any input file order.
///
/// The importer deliberately accepts only native, signed 16-bit Explicit VR
/// Little Endian CT in this first boundary. Unsupported encodings fail closed
/// instead of being guessed. Slices are ordered from their patient-space image
/// positions, never from filenames or Instance Number.
pub fn import_ct_series(paths: &[PathBuf]) -> Result<CtVolume> {
    let slices = read_series(paths, uids::CT_IMAGE_STORAGE, "CT", PixelKind::Signed16)?;
    ct_from_slices(slices)
}

/// Bytes-based CT import for hosts without a filesystem. `files` pairs a
/// stable label (used in errors) with complete Part-10 bytes per slice.
/// The same geometry and encoding gates run as [`import_ct_series`].
pub fn import_ct_series_from_bytes(files: &[(String, Vec<u8>)]) -> Result<CtVolume> {
    let slices = read_series_bytes(files, uids::CT_IMAGE_STORAGE, "CT", PixelKind::Signed16)?;
    ct_from_slices(slices)
}

fn ct_from_slices(slices: Vec<RawSlice>) -> Result<CtVolume> {
    let assembled = assemble_series(slices)?;
    Ok(CtVolume {
        geometry: assembled.geometry,
        frame_of_reference_uid: assembled.frame_of_reference_uid,
        study_instance_uid: assembled.study_instance_uid,
        series_instance_uid: assembled.series_instance_uid,
        slice_sop_instance_uids: assembled.slice_sop_instance_uids,
        // CT pixels are signed 16-bit by the PixelKind gate — exact cast.
        stored_pixels: assembled.stored_pixels.iter().map(|v| *v as i16).collect(),
        rescale_slope: assembled.rescale_slope,
        rescale_intercept: assembled.rescale_intercept,
    })
}

/// Accepted pixel encoding for a slice reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PixelKind {
    /// `Pixel Representation = 1` — signed 16-bit.
    Signed16,
    /// Either signed or unsigned 16-bit samples.
    Either16,
}

/// Read and gate every slice file under one SOP class + modality pair.
pub(crate) fn read_series(
    paths: &[PathBuf],
    sop_class_uid: &str,
    modality: &str,
    pixel_kind: PixelKind,
) -> Result<Vec<RawSlice>> {
    if paths.is_empty() {
        return Err(DicomError::EmptySeries);
    }
    if paths.len() < 2 {
        return Err(DicomError::Geometry(
            "at least two slices are required to establish slice spacing".into(),
        ));
    }

    let mut slices = Vec::with_capacity(paths.len());
    for path in paths {
        slices.push(read_slice(path, sop_class_uid, modality, pixel_kind)?);
    }
    Ok(slices)
}

/// Byte-slices variant of [`read_series`] — same gates, in-memory input.
pub(crate) fn read_series_bytes(
    files: &[(String, Vec<u8>)],
    sop_class_uid: &str,
    modality: &str,
    pixel_kind: PixelKind,
) -> Result<Vec<RawSlice>> {
    if files.is_empty() {
        return Err(DicomError::EmptySeries);
    }
    if files.len() < 2 {
        return Err(DicomError::Geometry(
            "at least two slices are required to establish slice spacing".into(),
        ));
    }

    let mut slices = Vec::with_capacity(files.len());
    for (name, bytes) in files {
        slices.push(read_slice_bytes(
            bytes,
            Path::new(name),
            sop_class_uid,
            modality,
            pixel_kind,
        )?);
    }
    Ok(slices)
}

pub(crate) fn read_slice(
    path: &Path,
    sop_class_uid: &str,
    modality: &str,
    pixel_kind: PixelKind,
) -> Result<RawSlice> {
    let obj = open_file(path).map_err(|source| DicomError::Read {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    slice_from_object(&obj, path, sop_class_uid, modality, pixel_kind)
}

/// In-memory variant of [`read_slice`] for hosts without a filesystem
/// (web drops, archives). `label` stands in for the path in errors and
/// the slice's recorded origin.
pub(crate) fn read_slice_bytes(
    bytes: &[u8],
    label: &Path,
    sop_class_uid: &str,
    modality: &str,
    pixel_kind: PixelKind,
) -> Result<RawSlice> {
    let obj = DefaultDicomObject::from_reader(std::io::Cursor::new(bytes)).map_err(|source| {
        DicomError::Read {
            path: label.to_path_buf(),
            source: Box::new(source),
        }
    })?;
    slice_from_object(&obj, label, sop_class_uid, modality, pixel_kind)
}

fn slice_from_object(
    obj: &DefaultDicomObject,
    path: &Path,
    sop_class_uid: &str,
    modality: &str,
    pixel_kind: PixelKind,
) -> Result<RawSlice> {
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
        sop_class_uid,
    )?;
    require_string(obj, path, tags::MODALITY, "Modality", modality)?;

    let rows = integer(obj, path, tags::ROWS, "Rows")?;
    let columns = integer(obj, path, tags::COLUMNS, "Columns")?;
    if rows == 0 || columns == 0 {
        return Err(attribute_error(path, "Rows/Columns", "must be non-zero"));
    }

    let pixel_spacing = fixed_floats::<2>(obj, path, tags::PIXEL_SPACING, "Pixel Spacing")?;
    if pixel_spacing
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(attribute_error(
            path,
            "Pixel Spacing",
            "both values must be finite and positive",
        ));
    }
    let image_position = fixed_floats::<3>(
        obj,
        path,
        tags::IMAGE_POSITION_PATIENT,
        "Image Position (Patient)",
    )?;
    let image_orientation = fixed_floats::<6>(
        obj,
        path,
        tags::IMAGE_ORIENTATION_PATIENT,
        "Image Orientation (Patient)",
    )?;

    require_integer(
        obj,
        path,
        tags::SAMPLES_PER_PIXEL,
        "Samples per Pixel",
        1_u16,
    )?;
    require_string(
        obj,
        path,
        tags::PHOTOMETRIC_INTERPRETATION,
        "Photometric Interpretation",
        "MONOCHROME2",
    )?;
    require_integer(obj, path, tags::BITS_ALLOCATED, "Bits Allocated", 16_u16)?;
    require_integer(obj, path, tags::BITS_STORED, "Bits Stored", 16_u16)?;
    require_integer(obj, path, tags::HIGH_BIT, "High Bit", 15_u16)?;
    let pixel_representation = integer(
        obj,
        path,
        tags::PIXEL_REPRESENTATION,
        "Pixel Representation",
    )?;
    let pixel_signed = match (pixel_kind, pixel_representation) {
        (PixelKind::Signed16, 1) | (PixelKind::Either16, 1) => true,
        (PixelKind::Either16, 0) => false,
        _ => {
            return Err(attribute_error(
                path,
                "Pixel Representation",
                format!("unsupported value {pixel_representation}"),
            ));
        }
    };

    let rescale_slope = float(obj, path, tags::RESCALE_SLOPE, "Rescale Slope")?;
    let rescale_intercept = float(obj, path, tags::RESCALE_INTERCEPT, "Rescale Intercept")?;
    if !rescale_slope.is_finite() || rescale_slope == 0.0 || !rescale_intercept.is_finite() {
        return Err(attribute_error(
            path,
            "Rescale Slope/Intercept",
            "slope must be finite and non-zero and intercept must be finite",
        ));
    }

    let expected_pixels = usize::from(rows) * usize::from(columns);
    let stored_pixels = pixels(obj, path, expected_pixels, pixel_signed)?;

    Ok(RawSlice {
        path: path.to_path_buf(),
        study_instance_uid: string(obj, path, tags::STUDY_INSTANCE_UID, "Study Instance UID")?,
        series_instance_uid: string(obj, path, tags::SERIES_INSTANCE_UID, "Series Instance UID")?,
        frame_of_reference_uid: string(
            obj,
            path,
            tags::FRAME_OF_REFERENCE_UID,
            "Frame of Reference UID",
        )?,
        sop_instance_uid: string(obj, path, tags::SOP_INSTANCE_UID, "SOP Instance UID")?,
        rows,
        columns,
        pixel_spacing,
        image_position,
        image_orientation,
        projection: 0.0,
        stored_pixels,
        rescale_slope,
        rescale_intercept,
    })
}

fn pixels(
    obj: &DefaultDicomObject,
    path: &Path,
    expected: usize,
    signed: bool,
) -> Result<Vec<i32>> {
    let element = obj
        .element(tags::PIXEL_DATA)
        .map_err(|error| attribute_error(path, "Pixel Data", error.to_string()))?;
    let output: Vec<i32> = match element.value() {
        Value::Primitive(PrimitiveValue::U16(values)) => values
            .iter()
            .map(|value| {
                if signed {
                    i32::from(*value as i16)
                } else {
                    i32::from(*value)
                }
            })
            .collect(),
        Value::Primitive(PrimitiveValue::I16(values)) => {
            if signed {
                values.iter().map(|value| i32::from(*value)).collect()
            } else {
                return Err(attribute_error(
                    path,
                    "Pixel Data",
                    "unsigned series carries signed pixel words",
                ));
            }
        }
        Value::Primitive(PrimitiveValue::U8(bytes)) if bytes.len() % 2 == 0 => bytes
            .chunks_exact(2)
            .map(|pair| {
                if signed {
                    i32::from(i16::from_le_bytes([pair[0], pair[1]]))
                } else {
                    i32::from(u16::from_le_bytes([pair[0], pair[1]]))
                }
            })
            .collect(),
        _ => {
            return Err(attribute_error(
                path,
                "Pixel Data",
                "expected native 16-bit pixel words",
            ));
        }
    };
    if output.len() != expected {
        return Err(attribute_error(
            path,
            "Pixel Data",
            format!("contains {} samples; expected {expected}", output.len()),
        ));
    }
    Ok(output)
}

fn validate_consistency(reference: &RawSlice, candidate: &RawSlice) -> Result<()> {
    macro_rules! same {
        ($field:ident, $label:literal) => {
            if candidate.$field != reference.$field {
                return Err(DicomError::InconsistentSeries(format!(
                    "{} has a different {}",
                    candidate.path.display(),
                    $label
                )));
            }
        };
    }
    same!(study_instance_uid, "Study Instance UID");
    same!(series_instance_uid, "Series Instance UID");
    same!(frame_of_reference_uid, "Frame of Reference UID");
    same!(rows, "row count");
    same!(columns, "column count");
    if !array_close(
        candidate.pixel_spacing,
        reference.pixel_spacing,
        SPACING_TOLERANCE_MM,
    ) {
        return Err(DicomError::InconsistentSeries(format!(
            "{} has different Pixel Spacing",
            candidate.path.display()
        )));
    }
    if !close(candidate.rescale_slope, reference.rescale_slope, 1.0e-12)
        || !close(
            candidate.rescale_intercept,
            reference.rescale_intercept,
            1.0e-12,
        )
    {
        return Err(DicomError::InconsistentSeries(format!(
            "{} has different rescale parameters",
            candidate.path.display()
        )));
    }
    Ok(())
}

fn validate_axes(x_axis: [f64; 3], y_axis: [f64; 3], path: &Path) -> Result<()> {
    if x_axis.iter().chain(&y_axis).any(|value| !value.is_finite())
        || !close(norm(x_axis), 1.0, DIRECTION_TOLERANCE)
        || !close(norm(y_axis), 1.0, DIRECTION_TOLERANCE)
        || dot(x_axis, y_axis).abs() > DIRECTION_TOLERANCE
    {
        return Err(DicomError::Geometry(format!(
            "{} has non-orthonormal Image Orientation (Patient)",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn string(
    obj: &DefaultDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
) -> Result<String> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_str()
        .map(|value| value.trim_end_matches([' ', '\0']).to_owned())
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

fn require_string(
    obj: &DefaultDicomObject,
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

pub(crate) fn integer(
    obj: &DefaultDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
) -> Result<u16> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_int::<u16>()
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

fn require_integer(
    obj: &DefaultDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
    expected: u16,
) -> Result<()> {
    let observed = integer(obj, path, tag, name)?;
    if observed != expected {
        return Err(attribute_error(
            path,
            name,
            format!("expected {expected:?}, found {observed:?}"),
        ));
    }
    Ok(())
}

pub(crate) fn float(
    obj: &DefaultDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
) -> Result<f64> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_float64()
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

fn fixed_floats<const N: usize>(
    obj: &DefaultDicomObject,
    path: &Path,
    tag: Tag,
    name: &'static str,
) -> Result<[f64; N]> {
    let values = obj
        .element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_multi_float64()
        .map_err(|error| attribute_error(path, name, error.to_string()))?;
    values.try_into().map_err(|values: Vec<f64>| {
        attribute_error(
            path,
            name,
            format!("expected {N} values, found {}", values.len()),
        )
    })
}

pub(crate) fn attribute_error(
    path: &Path,
    attribute: &'static str,
    detail: impl Into<String>,
) -> DicomError {
    DicomError::Attribute {
        path: path.to_path_buf(),
        attribute,
        detail: detail.into(),
    }
}

fn array3(slice: &[f64]) -> [f64; 3] {
    [slice[0], slice[1], slice[2]]
}

fn close(left: f64, right: f64, tolerance: f64) -> bool {
    (left - right).abs() <= tolerance
}

fn array_close<const N: usize>(left: [f64; N], right: [f64; N], tolerance: f64) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(left, right)| close(left, right, tolerance))
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0].mul_add(right[0], left[1].mul_add(right[1], left[2] * right[2]))
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1].mul_add(right[2], -(left[2] * right[1])),
        left[2].mul_add(right[0], -(left[0] * right[2])),
        left[0].mul_add(right[1], -(left[1] * right[0])),
    ]
}

fn norm(value: [f64; 3]) -> f64 {
    dot(value, value).sqrt()
}

fn normalize(value: [f64; 3]) -> [f64; 3] {
    scale(value, 1.0 / norm(value))
}

fn scale(value: [f64; 3], factor: f64) -> [f64; 3] {
    [value[0] * factor, value[1] * factor, value[2] * factor]
}

fn add(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn sub(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(pixels: &[i16]) -> CtVolume {
        CtVolume {
            geometry: GridGeometry {
                shape: [pixels.len() as u32, 1, 1],
                spacing_mm: [1.0; 3],
                origin_mm: [0.0; 3],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            frame_of_reference_uid: "1.2.3".into(),
            study_instance_uid: "1.2.4".into(),
            series_instance_uid: "1.2.5".into(),
            slice_sop_instance_uids: vec!["1.2.6".into()],
            stored_pixels: pixels.to_vec(),
            rescale_slope: 1.0,
            rescale_intercept: -1024.0,
        }
    }

    #[test]
    fn threshold_mask_selects_inclusive_window() {
        let ct = volume(&[923, 1100, 1200, 2000]);
        let mask = ct.threshold_mask("SOFT", -100.0, 200.0).unwrap();

        assert_eq!(mask.voxels, vec![false, true, true, false]);
        assert_eq!(mask.included_voxel_count(), 2);
    }

    #[test]
    fn threshold_mask_rejects_inverted_or_empty_windows() {
        let ct = volume(&[1024, 1100]);

        assert!(matches!(
            ct.threshold_mask("X", 50.0, -50.0),
            Err(ValidationError::InvalidThresholdWindow { .. })
        ));
        assert!(matches!(
            ct.threshold_mask("X", f64::NAN, 50.0),
            Err(ValidationError::InvalidThresholdWindow { .. })
        ));
        assert!(matches!(
            ct.threshold_mask("X", 5000.0, 6000.0),
            Err(ValidationError::EmptyMask(_))
        ));
    }
}
