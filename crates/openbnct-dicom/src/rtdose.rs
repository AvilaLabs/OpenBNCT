// SPDX-License-Identifier: MIT

//! DICOM RT Dose export (`openbnct-dicom::rtdose`).
//!
//! Writes a dose volume as a multi-frame RT Dose object: pixel data are
//! unsigned 32-bit integers scaled by DoseGridScaling, the frame stack is
//! addressed by ImagePositionPatient / ImageOrientationPatient /
//! GridFrameOffsetVector, and DoseUnits honestly reports GY only for
//! absolute-gray volumes — per-source-particle quantities export as
//! RELATIVE with the unit named in DoseComment.
//!
//! This is a research export for independent review and interop
//! comparison. It is not a commissioned treatment-planning product and
//! carries no patient identity unless the caller supplies one.

use std::path::{Path, PathBuf};

use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::{DataElement, Length, Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::InMemDicomObject;
use dicom_object::meta::FileMetaTableBuilder;
use openbnct_core::{DoseComponent, DoseUnit, GridGeometry, PhysicalDoseBundle};
use uuid::Uuid;

use crate::error::{DicomError, Result};

const RT_DOSE_SOP_CLASS: &str = "1.2.840.10008.5.1.4.1.1.481.2";
/// Deterministic fixed date/time: exported objects are generated
/// artifacts, not patient records.
const EXPORT_DATE: &str = "19700101";
const EXPORT_TIME: &str = "000000";

/// Which volume of a dose bundle to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoseSelection {
    /// The bundle's dedicated physical total.
    PhysicalTotal,
    /// One named dose component (B, N, H, photon).
    Component(DoseComponent),
}

/// Caller-supplied identity and referencing context for an RTDOSE
/// object. UIDs may be left empty to derive deterministic `2.25.*` UIDs
/// from the bundle's provenance id.
#[derive(Debug, Clone)]
pub struct RtDoseExportOptions {
    pub patient_name: String,
    pub patient_id: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
    /// Defaults to the bundle's `frame_of_reference_uid` when empty.
    pub frame_of_reference_uid: String,
    /// DICOM DoseSummationType (e.g. PLAN, FRACTION).
    pub dose_summation_type: String,
    /// Free-text series description; the component label is appended.
    pub series_description: String,
    /// Provenance/unit note written to DoseComment.
    pub dose_comment: Option<String>,
    /// SOP Instance UIDs of the source CT slices, used to build the
    /// ReferencedFrameOfReferenceSequence linkage. Empty omits the
    /// sequence.
    pub referenced_ct_instance_uids: Vec<String>,
}

impl Default for RtDoseExportOptions {
    fn default() -> Self {
        Self {
            patient_name: "OPENBNCT^RESEARCH".into(),
            patient_id: "OPENBNCT".into(),
            study_instance_uid: String::new(),
            series_instance_uid: String::new(),
            sop_instance_uid: String::new(),
            frame_of_reference_uid: String::new(),
            dose_summation_type: "PLAN".into(),
            series_description: "OpenBNCT research dose export".into(),
            dose_comment: None,
            referenced_ct_instance_uids: Vec::new(),
        }
    }
}

/// Result of an export: the written path plus the DoseGridScaling used,
/// so callers can report the quantization step.
#[derive(Debug)]
pub struct RtDoseExportResult {
    pub path: PathBuf,
    pub dose_grid_scaling: f64,
    pub dose_units: &'static str,
}

/// Export one volume of `bundle` as a Part-10 RT Dose file.
pub fn export_rt_dose(
    bundle: &PhysicalDoseBundle,
    selection: DoseSelection,
    options: &RtDoseExportOptions,
    path: &Path,
) -> Result<RtDoseExportResult> {
    let (label, unit, values): (String, DoseUnit, &[f64]) = match selection {
        DoseSelection::PhysicalTotal => (
            "physical-total".to_string(),
            bundle.physical_total.unit,
            &bundle.physical_total.values,
        ),
        DoseSelection::Component(component) => {
            let volume = bundle
                .components
                .iter()
                .find(|volume| volume.component == component)
                .ok_or(DicomError::StructureSet(format!(
                    "dose bundle has no component {component:?}"
                )))?;
            (
                format!("{component:?}").to_lowercase(),
                volume.unit,
                &volume.values,
            )
        }
    };
    let geometry = &bundle.geometry;
    let voxels = geometry
        .voxel_count()
        .map_err(|error| DicomError::Geometry(format!("dose grid: {error}")))?;
    if values.len() != voxels {
        return Err(DicomError::Geometry(format!(
            "dose values {} do not match grid {} voxels",
            values.len(),
            voxels
        )));
    }
    if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(DicomError::Geometry(
            "dose values must be finite and non-negative".into(),
        ));
    }
    // RTDOSE only declares GY or RELATIVE; a per-source-particle quantity
    // is not gray and must not be labeled as such.
    let dose_units: &'static str = match unit {
        DoseUnit::Gray => "GY",
        DoseUnit::GrayPerSourceParticle => "RELATIVE",
    };
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    let scaling = if max > 0.0 {
        max / u32::MAX as f64
    } else {
        1.0
    };
    let pixels: Vec<u32> = values
        .iter()
        .map(|v| (v / scaling).round().min(u32::MAX as f64) as u32)
        .collect();

    let name_seed = format!(
        "openbnct.rtdose.{}.{}.{}",
        bundle.provenance_id, label, bundle.case_id
    );
    let uid = |suffix: &str| -> String {
        format!(
            "2.25.{}",
            Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("{name_seed}.{suffix}").as_bytes()
            )
            .as_u128()
        )
    };
    let sop_instance_uid = non_empty_or(options.sop_instance_uid.clone(), || uid("sop"));
    let series_instance_uid = non_empty_or(options.series_instance_uid.clone(), || uid("series"));
    let study_instance_uid = non_empty_or(options.study_instance_uid.clone(), || uid("study"));
    let frame_of_reference_uid = non_empty_or(options.frame_of_reference_uid.clone(), || {
        bundle
            .frame_of_reference_uid
            .clone()
            .unwrap_or_else(|| uid("frame"))
    });

    let mut obj = InMemDicomObject::new_empty();
    put_str(&mut obj, tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192");
    put_str(
        &mut obj,
        tags::IMAGE_TYPE,
        VR::CS,
        "DERIVED\\SECONDARY\\MONTE_CARLO",
    );
    put_str(&mut obj, tags::SOP_CLASS_UID, VR::UI, RT_DOSE_SOP_CLASS);
    put_str(&mut obj, tags::SOP_INSTANCE_UID, VR::UI, &sop_instance_uid);
    for (tag, vr) in [
        (tags::STUDY_DATE, VR::DA),
        (tags::SERIES_DATE, VR::DA),
        (tags::CONTENT_DATE, VR::DA),
    ] {
        put_str(&mut obj, tag, vr, EXPORT_DATE);
    }
    for (tag, vr) in [
        (tags::STUDY_TIME, VR::TM),
        (tags::SERIES_TIME, VR::TM),
        (tags::CONTENT_TIME, VR::TM),
    ] {
        put_str(&mut obj, tag, vr, EXPORT_TIME);
    }
    put_str(&mut obj, tags::ACCESSION_NUMBER, VR::SH, "");
    put_str(&mut obj, tags::MODALITY, VR::CS, "RTDOSE");
    put_str(&mut obj, tags::MANUFACTURER, VR::LO, "Avila Labs");
    put_str(&mut obj, tags::REFERRING_PHYSICIAN_NAME, VR::PN, "");
    put_str(
        &mut obj,
        tags::STUDY_DESCRIPTION,
        VR::LO,
        &format!("OpenBNCT dose export — {}", bundle.case_id),
    );
    put_str(
        &mut obj,
        tags::SERIES_DESCRIPTION,
        VR::LO,
        &format!("{} — {label}", options.series_description),
    );
    put_str(&mut obj, tags::PATIENT_NAME, VR::PN, &options.patient_name);
    put_str(&mut obj, tags::PATIENT_ID, VR::LO, &options.patient_id);
    put_str(&mut obj, tags::PATIENT_BIRTH_DATE, VR::DA, "");
    put_str(&mut obj, tags::PATIENT_SEX, VR::CS, "");
    put_str(
        &mut obj,
        tags::STUDY_INSTANCE_UID,
        VR::UI,
        &study_instance_uid,
    );
    put_str(
        &mut obj,
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        &series_instance_uid,
    );
    put_str(&mut obj, tags::STUDY_ID, VR::SH, &bundle.case_id);
    put_str(&mut obj, tags::SERIES_NUMBER, VR::IS, "1");
    put_str(&mut obj, tags::INSTANCE_NUMBER, VR::IS, "1");
    put_str(
        &mut obj,
        tags::FRAME_OF_REFERENCE_UID,
        VR::UI,
        &frame_of_reference_uid,
    );
    put_str(&mut obj, tags::POSITION_REFERENCE_INDICATOR, VR::LO, "");

    // Plane orientation: columns vary along grid axis 0, rows along
    // axis 1; IOP wants the column and row direction cosines in LPS.
    let d = geometry.direction;
    put_str(
        &mut obj,
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        &ds_values(&geometry.origin_mm),
    );
    put_str(
        &mut obj,
        tags::IMAGE_ORIENTATION_PATIENT,
        VR::DS,
        &ds_values(&[d[0], d[3], d[6], d[1], d[4], d[7]]),
    );
    put_str(
        &mut obj,
        tags::PIXEL_SPACING,
        VR::DS,
        &ds_values(&[geometry.spacing_mm[1], geometry.spacing_mm[0]]),
    );
    let [nx, ny, nz] = geometry.shape;
    put_str(&mut obj, tags::NUMBER_OF_FRAMES, VR::IS, &nz.to_string());
    // Pixel data are addressed by the grid frame offset vector.
    obj.put(DataElement::new(
        tags::FRAME_INCREMENT_POINTER,
        VR::AT,
        PrimitiveValue::Tags(vec![tags::GRID_FRAME_OFFSET_VECTOR].into()),
    ));
    put_u16(&mut obj, tags::SAMPLES_PER_PIXEL, VR::US, 1);
    put_str(
        &mut obj,
        tags::PHOTOMETRIC_INTERPRETATION,
        VR::CS,
        "MONOCHROME2",
    );
    put_u16(&mut obj, tags::ROWS, VR::US, ny as u16);
    put_u16(&mut obj, tags::COLUMNS, VR::US, nx as u16);
    put_u16(&mut obj, tags::BITS_ALLOCATED, VR::US, 32);
    put_u16(&mut obj, tags::BITS_STORED, VR::US, 32);
    put_u16(&mut obj, tags::HIGH_BIT, VR::US, 31);
    put_u16(&mut obj, tags::PIXEL_REPRESENTATION, VR::US, 0);

    let offsets: Vec<f64> = (0..nz)
        .map(|k| f64::from(k) * geometry.spacing_mm[2])
        .collect();
    put_str(
        &mut obj,
        tags::GRID_FRAME_OFFSET_VECTOR,
        VR::DS,
        &ds_values(&offsets),
    );
    put_str(
        &mut obj,
        tags::DOSE_GRID_SCALING,
        VR::DS,
        &format!("{scaling:.17e}"),
    );
    put_str(&mut obj, tags::DOSE_UNITS, VR::CS, dose_units);
    put_str(&mut obj, tags::DOSE_TYPE, VR::CS, "PHYSICAL");
    put_str(
        &mut obj,
        tags::DOSE_SUMMATION_TYPE,
        VR::CS,
        &options.dose_summation_type,
    );
    let comment = match &options.dose_comment {
        Some(comment) => comment.clone(),
        None => format!(
            "Research use only — not for clinical treatment. provenance:{} unit:{:?}",
            bundle.provenance_id, unit
        ),
    };
    put_str(&mut obj, tags::DOSE_COMMENT, VR::LT, &comment);

    if !options.referenced_ct_instance_uids.is_empty() {
        // General Reference module: plain per-slice SOP references to the
        // source CT. The geometric link is the shared FrameOfReferenceUID.
        let image_items: Vec<InMemDicomObject> = options
            .referenced_ct_instance_uids
            .iter()
            .map(|uid| referenced_sop_item(uids::CT_IMAGE_STORAGE, uid))
            .collect();
        put_sequence(&mut obj, tags::REFERENCED_SOP_SEQUENCE, image_items);
    }

    obj.put(DataElement::new(
        tags::PIXEL_DATA,
        VR::OW,
        PrimitiveValue::U32(pixels.into()),
    ));

    let file = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .implementation_class_uid(uid("implementation"))
                .implementation_version_name("OPENBNCT_0_1")
                .source_application_entity_title("OPENBNCT"),
        )
        .map_err(|source| DicomError::Write {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
    file.write_to_file(path)
        .map_err(|source| DicomError::Write {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
    Ok(RtDoseExportResult {
        path: path.to_path_buf(),
        dose_grid_scaling: scaling,
        dose_units,
    })
}

fn referenced_sop_item(sop_class_uid: &str, sop_instance_uid: &str) -> InMemDicomObject {
    let mut item = InMemDicomObject::new_empty();
    put_str(
        &mut item,
        tags::REFERENCED_SOP_CLASS_UID,
        VR::UI,
        sop_class_uid,
    );
    put_str(
        &mut item,
        tags::REFERENCED_SOP_INSTANCE_UID,
        VR::UI,
        sop_instance_uid,
    );
    item
}

fn non_empty_or(value: String, fallback: impl FnOnce() -> String) -> String {
    if value.trim().is_empty() {
        fallback()
    } else {
        value
    }
}

fn put_str(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
    obj.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
}

fn put_u16(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: u16) {
    obj.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
}

fn put_sequence(obj: &mut InMemDicomObject, tag: Tag, items: Vec<InMemDicomObject>) {
    obj.put(DataElement::new(
        tag,
        VR::SQ,
        Value::new_sequence(items, Length::UNDEFINED),
    ));
}

fn ds_values(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| format!("{value:.10}"))
        .collect::<Vec<_>>()
        .join("\\")
}

/// A decoded RT Dose grid on a patient-LPS `GridGeometry`.
#[derive(Debug, Clone, PartialEq)]
pub struct RtDoseVolume {
    /// Grid in patient LPS millimeters. Axis 0 follows the columns, axis 1
    /// the rows, axis 2 the frame stack (signed by the Grid Frame Offset
    /// Vector direction).
    pub geometry: GridGeometry,
    /// `pixel * DoseGridScaling`, columns fastest, then rows, then frames.
    /// Gray when `dose_units == "GY"`.
    pub values: Vec<f64>,
    /// DICOM Dose Units: `GY` or `RELATIVE`.
    pub dose_units: String,
    pub dose_grid_scaling: f64,
    pub dose_type: Option<String>,
    pub dose_summation_type: Option<String>,
    pub dose_comment: Option<String>,
    pub manufacturer: Option<String>,
    pub frame_of_reference_uid: Option<String>,
    pub sop_instance_uid: String,
    pub study_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
}

const MAX_DOSE_VOXELS: usize = 1 << 30;

fn dose_error(message: impl Into<String>) -> DicomError {
    DicomError::RtDose(message.into())
}

/// Import a Part-10 RT Dose file.
///
/// Reads Dose Grid Scaling, 16/32-bit unsigned pixel data, Image
/// Position/Orientation (Patient), Pixel Spacing, the Grid Frame Offset
/// Vector, and Dose Units. Only `GY` and `RELATIVE` are accepted, and the
/// frame offsets must be uniform. Compressed transfer syntaxes are refused.
pub fn import_rt_dose(path: &Path) -> Result<RtDoseVolume> {
    let obj = dicom_object::open_file(path).map_err(|source| DicomError::Read {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    rt_dose_from_object(&obj, path)
}

/// In-memory variant of [`import_rt_dose`].
pub fn import_rt_dose_bytes(bytes: &[u8]) -> Result<RtDoseVolume> {
    let label = Path::new("rtdose.dcm");
    let obj = dicom_object::DefaultDicomObject::from_reader(std::io::Cursor::new(bytes)).map_err(
        |source| DicomError::Read {
            path: label.to_path_buf(),
            source: Box::new(source),
        },
    )?;
    rt_dose_from_object(&obj, label)
}

fn opt_text(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.get(tag)
        .and_then(|e| e.to_str().ok())
        .map(|s| s.trim_end_matches([' ', '\0']).trim().to_owned())
        .filter(|s| !s.is_empty())
}

fn rt_dose_from_object(
    obj: &dicom_object::DefaultDicomObject,
    path: &Path,
) -> Result<RtDoseVolume> {
    use crate::rtstruct::{attribute_error, dot, floats, integer, require_string, string};
    let ts = obj.meta().transfer_syntax();
    if ts != uids::EXPLICIT_VR_LITTLE_ENDIAN && ts != uids::IMPLICIT_VR_LITTLE_ENDIAN {
        return Err(attribute_error(
            path,
            "Transfer Syntax UID",
            format!("only uncompressed little-endian RT Dose is supported, found {ts}"),
        ));
    }
    require_string(
        obj,
        path,
        tags::SOP_CLASS_UID,
        "SOP Class UID",
        RT_DOSE_SOP_CLASS,
    )?;
    let dose_units = string(obj, path, tags::DOSE_UNITS, "Dose Units")?;
    if dose_units != "GY" && dose_units != "RELATIVE" {
        return Err(dose_error(format!(
            "Dose Units {dose_units:?} is not GY or RELATIVE"
        )));
    }
    let scaling: f64 = floats(obj, path, tags::DOSE_GRID_SCALING, "Dose Grid Scaling")?
        .first()
        .copied()
        .ok_or_else(|| dose_error("Dose Grid Scaling is empty"))?;
    if !scaling.is_finite() || scaling <= 0.0 {
        return Err(dose_error("Dose Grid Scaling must be positive and finite"));
    }
    let rows = integer(obj, path, tags::ROWS, "Rows")?;
    let columns = integer(obj, path, tags::COLUMNS, "Columns")?;
    let frames = match obj.get(tags::NUMBER_OF_FRAMES) {
        Some(_) => integer(obj, path, tags::NUMBER_OF_FRAMES, "Number of Frames")?,
        None => 1,
    };
    if rows <= 0 || columns <= 0 || frames <= 0 {
        return Err(dose_error(
            "Rows, Columns and Number of Frames must be positive",
        ));
    }
    let (rows, columns, frames) = (rows as usize, columns as usize, frames as usize);
    let voxels = rows
        .checked_mul(columns)
        .and_then(|v| v.checked_mul(frames))
        .filter(|v| *v <= MAX_DOSE_VOXELS)
        .ok_or_else(|| dose_error("dose grid is too large"))?;
    let samples = integer(obj, path, tags::SAMPLES_PER_PIXEL, "Samples per Pixel")?;
    let bits = integer(obj, path, tags::BITS_ALLOCATED, "Bits Allocated")?;
    let representation = integer(
        obj,
        path,
        tags::PIXEL_REPRESENTATION,
        "Pixel Representation",
    )?;
    if samples != 1 || representation != 0 || (bits != 16 && bits != 32) {
        return Err(dose_error(format!(
            "unsupported pixel format: {samples} samples, {bits} bits, representation \
             {representation}; need 1 sample of unsigned 16 or 32 bits"
        )));
    }
    let width = (bits / 8) as usize;
    let pixel_bytes = obj
        .element(tags::PIXEL_DATA)
        .map_err(|e| attribute_error(path, "Pixel Data", e.to_string()))?
        .to_bytes()
        .map_err(|e| attribute_error(path, "Pixel Data", e.to_string()))?;
    if pixel_bytes.len() < voxels * width {
        return Err(dose_error(format!(
            "Pixel Data holds {} bytes, {} required",
            pixel_bytes.len(),
            voxels * width
        )));
    }
    let values: Vec<f64> = if width == 2 {
        pixel_bytes[..voxels * 2]
            .chunks_exact(2)
            .map(|c| f64::from(u16::from_le_bytes([c[0], c[1]])) * scaling)
            .collect()
    } else {
        pixel_bytes[..voxels * 4]
            .chunks_exact(4)
            .map(|c| f64::from(u32::from_le_bytes([c[0], c[1], c[2], c[3]])) * scaling)
            .collect()
    };

    let position = floats(
        obj,
        path,
        tags::IMAGE_POSITION_PATIENT,
        "Image Position (Patient)",
    )?;
    let orientation = floats(
        obj,
        path,
        tags::IMAGE_ORIENTATION_PATIENT,
        "Image Orientation (Patient)",
    )?;
    let pixel_spacing = floats(obj, path, tags::PIXEL_SPACING, "Pixel Spacing")?;
    if position.len() != 3 || orientation.len() != 6 || pixel_spacing.len() != 2 {
        return Err(dose_error(
            "Image Position, Image Orientation and Pixel Spacing have the wrong length",
        ));
    }
    if position
        .iter()
        .chain(&orientation)
        .chain(&pixel_spacing)
        .any(|v| !v.is_finite())
        || pixel_spacing.iter().any(|v| *v <= 0.0)
    {
        return Err(dose_error("non-finite or non-positive geometry attribute"));
    }
    let u = [orientation[0], orientation[1], orientation[2]];
    let v = [orientation[3], orientation[4], orientation[5]];
    let norm = |a: [f64; 3]| dot(a, a).sqrt();
    if (norm(u) - 1.0).abs() > 1.0e-3 || (norm(v) - 1.0).abs() > 1.0e-3 || dot(u, v).abs() > 1.0e-3
    {
        return Err(dose_error("Image Orientation is not an orthonormal pair"));
    }
    let normal = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    // Frame stack: offsets along the plane normal relative to the position.
    let offsets = match obj.get(tags::GRID_FRAME_OFFSET_VECTOR) {
        Some(element) => element
            .to_multi_float64()
            .map_err(|e| attribute_error(path, "Grid Frame Offset Vector", e.to_string()))?,
        None => Vec::new(),
    };
    let (first_offset, step) = if offsets.is_empty() {
        if frames != 1 {
            return Err(dose_error(
                "multi-frame RT Dose needs a Grid Frame Offset Vector",
            ));
        }
        let thickness = obj
            .get(tags::SLICE_THICKNESS)
            .and_then(|e| e.to_float64().ok())
            .filter(|t| t.is_finite() && *t > 0.0)
            .ok_or_else(|| {
                dose_error("single-frame RT Dose without offsets needs a Slice Thickness")
            })?;
        (0.0, thickness)
    } else {
        if offsets.len() != frames {
            return Err(dose_error(format!(
                "Grid Frame Offset Vector has {} entries for {frames} frames",
                offsets.len()
            )));
        }
        if offsets.iter().any(|o| !o.is_finite()) {
            return Err(dose_error("Grid Frame Offset Vector is not finite"));
        }
        if frames == 1 {
            let thickness = obj
                .get(tags::SLICE_THICKNESS)
                .and_then(|e| e.to_float64().ok())
                .filter(|t| t.is_finite() && *t > 0.0)
                .ok_or_else(|| {
                    dose_error("single-frame RT Dose needs a Slice Thickness for its spacing")
                })?;
            (offsets[0], thickness)
        } else {
            let step = offsets[1] - offsets[0];
            if step == 0.0 {
                return Err(dose_error("Grid Frame Offset Vector repeats a position"));
            }
            for w in offsets.windows(2) {
                if ((w[1] - w[0]) - step).abs() > 1.0e-3 * step.abs().max(1.0) {
                    return Err(dose_error(
                        "Grid Frame Offset Vector spacing is not uniform",
                    ));
                }
            }
            (offsets[0], step)
        }
    };
    // GridGeometry is right-handed, so axis 2 is always +normal. A stack
    // stored in descending offset order is reversed frame-wise and its
    // origin is the last stored frame.
    let mut values = values;
    let start_offset = if step < 0.0 {
        let plane = rows * columns;
        let mut reversed = Vec::with_capacity(values.len());
        for frame in (0..frames).rev() {
            reversed.extend_from_slice(&values[frame * plane..(frame + 1) * plane]);
        }
        values = reversed;
        first_offset + step * (frames - 1) as f64
    } else {
        first_offset
    };
    let origin = [
        position[0] + start_offset * normal[0],
        position[1] + start_offset * normal[1],
        position[2] + start_offset * normal[2],
    ];
    let axis2 = normal;
    let geometry = GridGeometry {
        shape: [columns as u32, rows as u32, frames as u32],
        spacing_mm: [pixel_spacing[1], pixel_spacing[0], step.abs()],
        origin_mm: origin,
        direction: [
            u[0], v[0], axis2[0], u[1], v[1], axis2[1], u[2], v[2], axis2[2],
        ],
    };
    geometry
        .voxel_count()
        .map_err(|e| DicomError::Geometry(format!("dose grid: {e}")))?;
    Ok(RtDoseVolume {
        geometry,
        values,
        dose_units,
        dose_grid_scaling: scaling,
        dose_type: opt_text(obj, tags::DOSE_TYPE),
        dose_summation_type: opt_text(obj, tags::DOSE_SUMMATION_TYPE),
        dose_comment: opt_text(obj, tags::DOSE_COMMENT),
        manufacturer: opt_text(obj, tags::MANUFACTURER),
        frame_of_reference_uid: opt_text(obj, tags::FRAME_OF_REFERENCE_UID),
        sop_instance_uid: string(obj, path, tags::SOP_INSTANCE_UID, "SOP Instance UID")?,
        study_instance_uid: opt_text(obj, tags::STUDY_INSTANCE_UID),
        series_instance_uid: opt_text(obj, tags::SERIES_INSTANCE_UID),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_object::open_file;
    use openbnct_core::{
        ComponentProfileReference, ContentReference, DoseVolume, GridGeometry,
        PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

    fn bundle() -> PhysicalDoseBundle {
        let reference = || ContentReference {
            id: "r".into(),
            sha256: "sha256:".to_string() + &"ab".repeat(32),
        };
        PhysicalDoseBundle {
            schema_version: "openbnct.physical-dose-bundle/0.2.0".into(),
            case_id: "case-1".into(),
            frame_of_reference_uid: Some("1.2.3.4.5".into()),
            geometry: GridGeometry {
                shape: [4, 3, 2],
                spacing_mm: [2.0, 3.0, 4.0],
                origin_mm: [10.0, 20.0, 30.0],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            component_profile: ComponentProfileReference {
                id: "p".into(),
                sha256: "sha256:".to_string() + &"cd".repeat(32),
            },
            response_set: reference(),
            components: vec![DoseVolume {
                component: DoseComponent::Photon,
                unit: DoseUnit::GrayPerSourceParticle,
                values: (0..24).map(|i| i as f64 * 1e-15).collect(),
                absolute_standard_uncertainty: None,
            }],
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::Gray,
                values: (0..24).map(|i| i as f64 * 0.25).collect(),
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::Unavailable,
            },
            provenance_id: "prov-1".into(),
        }
    }

    fn text(obj: &dicom_object::DefaultDicomObject, tag: Tag) -> String {
        obj.element(tag).unwrap().to_str().unwrap().into_owned()
    }

    #[test]
    fn exports_valid_rtdose_with_grid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dose.dcm");
        let result = export_rt_dose(
            &bundle(),
            DoseSelection::PhysicalTotal,
            &RtDoseExportOptions::default(),
            &path,
        )
        .unwrap();
        assert_eq!(result.dose_units, "GY");
        let obj = open_file(&path).unwrap();
        assert_eq!(text(&obj, tags::SOP_CLASS_UID), RT_DOSE_SOP_CLASS);
        assert_eq!(text(&obj, tags::MODALITY), "RTDOSE");
        // Grid: 4x3x2 → 2 frames, offsets 0 and 4 mm.
        assert_eq!(text(&obj, tags::NUMBER_OF_FRAMES), "2");
        assert_eq!(
            text(&obj, tags::GRID_FRAME_OFFSET_VECTOR),
            "0.0000000000\\4.0000000000"
        );
        assert_eq!(
            text(&obj, tags::PIXEL_SPACING),
            "3.0000000000\\2.0000000000"
        );
        // Physical total max = 5.75 → scaling = 5.75/u32::MAX; first voxel 0,
        // last voxel round-trips to ~5.75.
        let bytes = obj.element(tags::PIXEL_DATA).unwrap().to_bytes().unwrap();
        let pixels: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(pixels.len(), 24);
        assert_eq!(pixels[0], 0);
        let scaling: f64 = text(&obj, tags::DOSE_GRID_SCALING).parse().unwrap();
        let recovered = pixels[23] as f64 * scaling;
        assert!((recovered - 5.75).abs() / 5.75 < 1e-9);
    }

    #[test]
    fn per_source_particle_unit_exports_as_relative() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dose.dcm");
        let result = export_rt_dose(
            &bundle(),
            DoseSelection::Component(DoseComponent::Photon),
            &RtDoseExportOptions::default(),
            &path,
        )
        .unwrap();
        assert_eq!(result.dose_units, "RELATIVE");
        let obj = open_file(&path).unwrap();
        assert_eq!(text(&obj, tags::DOSE_UNITS), "RELATIVE");
    }

    #[test]
    fn deterministic_uids_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.dcm");
        let second = dir.path().join("b.dcm");
        export_rt_dose(
            &bundle(),
            DoseSelection::PhysicalTotal,
            &RtDoseExportOptions::default(),
            &first,
        )
        .unwrap();
        export_rt_dose(
            &bundle(),
            DoseSelection::PhysicalTotal,
            &RtDoseExportOptions::default(),
            &second,
        )
        .unwrap();
        let a = open_file(&first).unwrap();
        let b = open_file(&second).unwrap();
        assert_eq!(
            text(&a, tags::SOP_INSTANCE_UID),
            text(&b, tags::SOP_INSTANCE_UID)
        );
    }

    #[test]
    fn missing_component_and_bad_grid_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.dcm");
        assert!(
            export_rt_dose(
                &bundle(),
                DoseSelection::Component(DoseComponent::Boron),
                &RtDoseExportOptions::default(),
                &path,
            )
            .is_err()
        );
        let mut broken = bundle();
        broken.geometry.spacing_mm[0] = 0.0;
        assert!(
            export_rt_dose(
                &broken,
                DoseSelection::PhysicalTotal,
                &RtDoseExportOptions::default(),
                &path,
            )
            .is_err()
        );
    }

    #[test]
    fn export_then_import_round_trips_values_and_grid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dose.dcm");
        let source = bundle();
        let result = export_rt_dose(
            &source,
            DoseSelection::PhysicalTotal,
            &RtDoseExportOptions::default(),
            &path,
        )
        .unwrap();
        let imported = import_rt_dose(&path).unwrap();
        assert_eq!(imported.dose_units, "GY");
        assert_eq!(imported.geometry.shape, source.geometry.shape);
        assert_eq!(imported.geometry.spacing_mm, source.geometry.spacing_mm);
        assert_eq!(imported.geometry.origin_mm, source.geometry.origin_mm);
        assert_eq!(imported.geometry.direction, source.geometry.direction);
        assert_eq!(
            imported.frame_of_reference_uid.as_deref(),
            Some("1.2.3.4.5")
        );
        assert!((imported.dose_grid_scaling - result.dose_grid_scaling).abs() < 1e-24);
        assert_eq!(imported.values.len(), 24);
        for (got, want) in imported.values.iter().zip(&source.physical_total.values) {
            assert!(
                (got - want).abs() <= 0.5 * result.dose_grid_scaling * 1.0001,
                "{got} vs {want}"
            );
        }
        // Bytes path agrees.
        let again = import_rt_dose_bytes(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(again, imported);
    }

    #[test]
    fn import_handles_descending_frames_and_refuses_bad_units() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dose.dcm");
        export_rt_dose(
            &bundle(),
            DoseSelection::PhysicalTotal,
            &RtDoseExportOptions::default(),
            &path,
        )
        .unwrap();
        // Rewrite with descending frame offsets: the stack runs -z.
        let mut obj = dicom_object::open_file(&path).unwrap();
        obj.put(DataElement::new(
            tags::GRID_FRAME_OFFSET_VECTOR,
            VR::DS,
            PrimitiveValue::from("0.0\\-4.0"),
        ));
        let flipped = dir.path().join("flipped.dcm");
        obj.write_to_file(&flipped).unwrap();
        let imported = import_rt_dose(&flipped).unwrap();
        assert_eq!(imported.geometry.spacing_mm[2], 4.0);
        assert_eq!(imported.geometry.direction[8], 1.0);
        assert_eq!(imported.geometry.origin_mm, [10.0, 20.0, 26.0]);
        // Stored frame 0 (z = 30) is now the last frame.
        let original = import_rt_dose(&path).unwrap();
        assert_eq!(imported.values[..12], original.values[12..]);
        assert_eq!(imported.values[12..], original.values[..12]);

        obj.put(DataElement::new(
            tags::DOSE_UNITS,
            VR::CS,
            PrimitiveValue::from("PERCENT"),
        ));
        let bad = dir.path().join("bad.dcm");
        obj.write_to_file(&bad).unwrap();
        assert!(import_rt_dose(&bad).is_err());
    }
}
