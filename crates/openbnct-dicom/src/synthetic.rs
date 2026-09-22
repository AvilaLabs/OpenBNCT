// SPDX-License-Identifier: MIT

//! Deterministic writer for the public `NF-BNCT-001` geometry benchmark.
//!
//! This writer is intentionally independent of the importer and rasterizer.
//! It describes contours from the frozen benchmark boxes rather than using
//! any production mask-to-contour conversion path.

use std::fs;
use std::path::{Path, PathBuf};

use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::{DataElement, Length, Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::meta::FileMetaTableBuilder;
use dicom_object::{InMemDicomObject, open_file};
use openbnct_core::GridGeometry;
use openbnct_evidence::{
    ArtifactRecord, CaseManifest, PatientCoordinateSystem, QualificationBoundary, StructureRecord,
    sha256_file,
};
use uuid::Uuid;

use crate::{DicomError, Result};

pub const CASE_ID: &str = "NF-BNCT-001";
pub const FRAME_OF_REFERENCE_UID: &str = "2.25.240883953911088373736134884257182446642";
pub const STUDY_INSTANCE_UID: &str = "2.25.149214599444245138262873740736845471752";
pub const CT_SERIES_INSTANCE_UID: &str = "2.25.337319594251465962942344971245692083782";
pub const RTSTRUCT_SERIES_INSTANCE_UID: &str = "2.25.50705181539640583496141175374452175263";
pub const RTSTRUCT_INSTANCE_UID: &str = "2.25.277528316852233615277963392913905893031";

pub const COLUMNS: usize = 40;
pub const ROWS: usize = 40;
pub const SLICES: usize = 40;
pub const SPACING_MM: f64 = 5.0;
pub const FIRST_CENTER_MM: f64 = -97.5;

// These UUIDv5 name strings are frozen: every `2.25.*` UID in the benchmark is
// derived from them and published in SPECIFICATION.md, so the original
// `nctforge.org` names persist even though the project is now OpenBNCT.
const IMPLEMENTATION_UID_NAME: &str = "https://nctforge.org/dicom/implementation-class";
const SLICE_UID_NAME_PREFIX: &str = "https://nctforge.org/benchmarks/nf-bnct-001/ct-slice-";
// Required by the legacy RT Referenced Study Sequence in the RT Structure Set IOD.
const DETACHED_STUDY_MANAGEMENT_UID: &str = "1.2.840.10008.3.1.2.3.1";
const FROZEN_DATE: &str = "20260101";
const FROZEN_TIME: &str = "000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCase {
    pub root: PathBuf,
    pub ct_files: Vec<PathBuf>,
    pub rtstruct_file: PathBuf,
    pub manifest_file: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct RoiBox {
    number: i32,
    name: &'static str,
    x: [f64; 2],
    y: [f64; 2],
    z: [f64; 2],
    color: [u16; 3],
}

const ROIS: [RoiBox; 5] = [
    RoiBox {
        number: 1,
        name: "PHANTOM",
        x: [-100.0, 100.0],
        y: [-100.0, 100.0],
        z: [-100.0, 100.0],
        color: [180, 180, 180],
    },
    RoiBox {
        number: 2,
        name: "CORE",
        x: [-20.0, 20.0],
        y: [-20.0, 20.0],
        z: [-20.0, 20.0],
        color: [255, 80, 80],
    },
    RoiBox {
        number: 3,
        name: "LEFT_ANTERIOR_MARKER",
        x: [60.0, 80.0],
        y: [-80.0, -60.0],
        z: [-80.0, -60.0],
        color: [80, 255, 80],
    },
    RoiBox {
        number: 4,
        name: "RIGHT_POSTERIOR_MARKER",
        x: [-80.0, -60.0],
        y: [60.0, 80.0],
        z: [60.0, 80.0],
        color: [80, 80, 255],
    },
    RoiBox {
        number: 5,
        name: "CENTRAL_AXIS_2CM",
        x: [-5.0, 5.0],
        y: [-5.0, 5.0],
        z: [-85.0, -75.0],
        color: [255, 255, 0],
    },
];

/// Create the complete deterministic CT and RT Structure Set benchmark.
///
/// The destination must not already exist. This protects previous benchmark
/// artifacts from accidental replacement.
pub fn generate_nf_bnct_001(output: &Path) -> Result<GeneratedCase> {
    if output.exists() {
        return Err(DicomError::OutputExists(output.to_path_buf()));
    }
    create_dir(output)?;
    let ct_dir = output.join("ct");
    create_dir(&ct_dir)?;

    let mut ct_files = Vec::with_capacity(SLICES);
    let mut slice_uids = Vec::with_capacity(SLICES);
    for slice_index in 0..SLICES {
        let uid = ct_slice_uid(slice_index);
        let path = ct_dir.join(format!("ct-{slice_index:03}.dcm"));
        write_ct_slice(&path, slice_index, &uid)?;
        ct_files.push(path);
        slice_uids.push(uid);
    }

    let rtstruct_file = output.join("rtstruct.dcm");
    write_rtstruct(&rtstruct_file, &slice_uids)?;

    let manifest_file = output.join("case.json");
    write_case_manifest(&manifest_file, &ct_files, &rtstruct_file)?;

    let generated = GeneratedCase {
        root: output.to_path_buf(),
        ct_files,
        rtstruct_file,
        manifest_file,
    };
    crate::verify_nf_bnct_001(output)?;

    Ok(generated)
}

/// Deterministically derive one valid DICOM `2.25` UID from a UUIDv5 name.
#[must_use]
pub fn ct_slice_uid(slice_index: usize) -> String {
    let name = format!("{SLICE_UID_NAME_PREFIX}{slice_index:03}");
    format!(
        "2.25.{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, name.as_bytes()).as_u128()
    )
}

fn implementation_class_uid() -> String {
    format!(
        "2.25.{}",
        Uuid::new_v5(&Uuid::NAMESPACE_URL, IMPLEMENTATION_UID_NAME.as_bytes()).as_u128()
    )
}

fn write_ct_slice(path: &Path, slice_index: usize, sop_instance_uid: &str) -> Result<()> {
    let mut obj = InMemDicomObject::new_empty();
    put_str(&mut obj, tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192");
    put_str(
        &mut obj,
        tags::IMAGE_TYPE,
        VR::CS,
        "ORIGINAL\\PRIMARY\\AXIAL",
    );
    // The synthetic cube is unpaired. An explicit image-level value prevents
    // a series-level right/left assertion from being invented for the phantom.
    put_str(&mut obj, tags::IMAGE_LATERALITY, VR::CS, "U");
    put_str(
        &mut obj,
        tags::SOP_CLASS_UID,
        VR::UI,
        uids::CT_IMAGE_STORAGE,
    );
    put_str(&mut obj, tags::SOP_INSTANCE_UID, VR::UI, sop_instance_uid);
    put_str(&mut obj, tags::STUDY_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::SERIES_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::ACQUISITION_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::CONTENT_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::STUDY_TIME, VR::TM, FROZEN_TIME);
    put_str(&mut obj, tags::SERIES_TIME, VR::TM, FROZEN_TIME);
    put_str(&mut obj, tags::ACQUISITION_TIME, VR::TM, FROZEN_TIME);
    put_str(&mut obj, tags::CONTENT_TIME, VR::TM, FROZEN_TIME);
    put_str(&mut obj, tags::ACCESSION_NUMBER, VR::SH, "");
    put_str(&mut obj, tags::MODALITY, VR::CS, "CT");
    put_str(&mut obj, tags::MANUFACTURER, VR::LO, "Avila Labs");
    put_str(
        &mut obj,
        tags::INSTITUTION_NAME,
        VR::LO,
        "NCTForge public benchmark",
    );
    put_str(&mut obj, tags::REFERRING_PHYSICIAN_NAME, VR::PN, "");
    put_str(&mut obj, tags::STUDY_DESCRIPTION, VR::LO, CASE_ID);
    put_str(
        &mut obj,
        tags::SERIES_DESCRIPTION,
        VR::LO,
        "Synthetic water phantom CT",
    );
    put_str(&mut obj, tags::PATIENT_NAME, VR::PN, "NCTFORGE^SYNTHETIC");
    put_str(&mut obj, tags::PATIENT_ID, VR::LO, CASE_ID);
    put_str(&mut obj, tags::PATIENT_BIRTH_DATE, VR::DA, "");
    put_str(&mut obj, tags::PATIENT_SEX, VR::CS, "");
    put_str(&mut obj, tags::PATIENT_IDENTITY_REMOVED, VR::CS, "YES");
    put_str(
        &mut obj,
        tags::DEIDENTIFICATION_METHOD,
        VR::LO,
        "Synthetic; no patient source",
    );
    put_str(&mut obj, tags::SLICE_THICKNESS, VR::DS, "5");
    put_str(&mut obj, tags::KVP, VR::DS, "120");
    put_str(
        &mut obj,
        tags::STUDY_INSTANCE_UID,
        VR::UI,
        STUDY_INSTANCE_UID,
    );
    put_str(
        &mut obj,
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        CT_SERIES_INSTANCE_UID,
    );
    put_str(&mut obj, tags::STUDY_ID, VR::SH, "NFBNCT001");
    put_str(&mut obj, tags::SERIES_NUMBER, VR::IS, "1");
    put_str(&mut obj, tags::ACQUISITION_NUMBER, VR::IS, "1");
    put_str(
        &mut obj,
        tags::INSTANCE_NUMBER,
        VR::IS,
        &(slice_index + 1).to_string(),
    );
    put_str(&mut obj, tags::PATIENT_POSITION, VR::CS, "HFS");
    put_str(
        &mut obj,
        tags::FRAME_OF_REFERENCE_UID,
        VR::UI,
        FRAME_OF_REFERENCE_UID,
    );
    put_str(
        &mut obj,
        tags::POSITION_REFERENCE_INDICATOR,
        VR::LO,
        "SYNTHETIC_ORIGIN",
    );
    put_str(
        &mut obj,
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        &format!("-97.5\\-97.5\\{}", slice_center(slice_index)),
    );
    put_str(
        &mut obj,
        tags::IMAGE_ORIENTATION_PATIENT,
        VR::DS,
        "1\\0\\0\\0\\1\\0",
    );
    put_u16(&mut obj, tags::SAMPLES_PER_PIXEL, VR::US, 1);
    put_str(
        &mut obj,
        tags::PHOTOMETRIC_INTERPRETATION,
        VR::CS,
        "MONOCHROME2",
    );
    put_u16(&mut obj, tags::ROWS, VR::US, ROWS as u16);
    put_u16(&mut obj, tags::COLUMNS, VR::US, COLUMNS as u16);
    put_str(&mut obj, tags::PIXEL_SPACING, VR::DS, "5\\5");
    put_u16(&mut obj, tags::BITS_ALLOCATED, VR::US, 16);
    put_u16(&mut obj, tags::BITS_STORED, VR::US, 16);
    put_u16(&mut obj, tags::HIGH_BIT, VR::US, 15);
    put_u16(&mut obj, tags::PIXEL_REPRESENTATION, VR::US, 1);
    put_str(&mut obj, tags::WINDOW_CENTER, VR::DS, "0");
    put_str(&mut obj, tags::WINDOW_WIDTH, VR::DS, "1");
    put_str(&mut obj, tags::RESCALE_INTERCEPT, VR::DS, "0");
    put_str(&mut obj, tags::RESCALE_SLOPE, VR::DS, "1");
    put_str(&mut obj, tags::RESCALE_TYPE, VR::LO, "HU");
    obj.put(DataElement::new(
        tags::PIXEL_DATA,
        VR::OW,
        PrimitiveValue::U16(vec![0_u16; ROWS * COLUMNS].into()),
    ));

    write_file(path, obj)
}

fn write_rtstruct(path: &Path, slice_uids: &[String]) -> Result<()> {
    let mut obj = InMemDicomObject::new_empty();
    put_str(&mut obj, tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192");
    put_str(
        &mut obj,
        tags::SOP_CLASS_UID,
        VR::UI,
        uids::RT_STRUCTURE_SET_STORAGE,
    );
    put_str(
        &mut obj,
        tags::SOP_INSTANCE_UID,
        VR::UI,
        RTSTRUCT_INSTANCE_UID,
    );
    put_str(&mut obj, tags::STUDY_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::SERIES_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::STUDY_TIME, VR::TM, FROZEN_TIME);
    put_str(&mut obj, tags::SERIES_TIME, VR::TM, FROZEN_TIME);
    put_str(&mut obj, tags::ACCESSION_NUMBER, VR::SH, "");
    put_str(&mut obj, tags::MODALITY, VR::CS, "RTSTRUCT");
    put_str(&mut obj, tags::MANUFACTURER, VR::LO, "Avila Labs");
    put_str(
        &mut obj,
        tags::INSTITUTION_NAME,
        VR::LO,
        "NCTForge public benchmark",
    );
    put_str(&mut obj, tags::REFERRING_PHYSICIAN_NAME, VR::PN, "");
    put_str(&mut obj, tags::STUDY_DESCRIPTION, VR::LO, CASE_ID);
    put_str(
        &mut obj,
        tags::SERIES_DESCRIPTION,
        VR::LO,
        "Synthetic reference structures",
    );
    put_str(&mut obj, tags::PATIENT_NAME, VR::PN, "NCTFORGE^SYNTHETIC");
    put_str(&mut obj, tags::PATIENT_ID, VR::LO, CASE_ID);
    put_str(&mut obj, tags::PATIENT_BIRTH_DATE, VR::DA, "");
    put_str(&mut obj, tags::PATIENT_SEX, VR::CS, "");
    put_str(&mut obj, tags::PATIENT_IDENTITY_REMOVED, VR::CS, "YES");
    put_str(
        &mut obj,
        tags::DEIDENTIFICATION_METHOD,
        VR::LO,
        "Synthetic; no patient source",
    );
    put_str(
        &mut obj,
        tags::STUDY_INSTANCE_UID,
        VR::UI,
        STUDY_INSTANCE_UID,
    );
    put_str(
        &mut obj,
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        RTSTRUCT_SERIES_INSTANCE_UID,
    );
    put_str(&mut obj, tags::STUDY_ID, VR::SH, "NFBNCT001");
    put_str(&mut obj, tags::SERIES_NUMBER, VR::IS, "2");
    put_str(&mut obj, tags::INSTANCE_NUMBER, VR::IS, "1");
    put_str(&mut obj, tags::OPERATORS_NAME, VR::PN, "");
    put_str(
        &mut obj,
        tags::FRAME_OF_REFERENCE_UID,
        VR::UI,
        FRAME_OF_REFERENCE_UID,
    );
    put_str(
        &mut obj,
        tags::POSITION_REFERENCE_INDICATOR,
        VR::LO,
        "SYNTHETIC_ORIGIN",
    );
    put_str(&mut obj, tags::STRUCTURE_SET_LABEL, VR::SH, "NFBNCT001");
    put_str(&mut obj, tags::STRUCTURE_SET_NAME, VR::LO, CASE_ID);
    put_str(
        &mut obj,
        tags::STRUCTURE_SET_DESCRIPTION,
        VR::ST,
        "NCTForge deterministic geometry benchmark",
    );
    put_str(&mut obj, tags::STRUCTURE_SET_DATE, VR::DA, FROZEN_DATE);
    put_str(&mut obj, tags::STRUCTURE_SET_TIME, VR::TM, FROZEN_TIME);

    put_sequence(
        &mut obj,
        tags::REFERENCED_FRAME_OF_REFERENCE_SEQUENCE,
        vec![referenced_frame_item(slice_uids)],
    );
    put_sequence(
        &mut obj,
        tags::STRUCTURE_SET_ROI_SEQUENCE,
        ROIS.iter().map(structure_set_roi_item).collect(),
    );
    put_sequence(
        &mut obj,
        tags::ROI_CONTOUR_SEQUENCE,
        ROIS.iter()
            .map(|roi| roi_contour_item(*roi, slice_uids))
            .collect(),
    );
    put_sequence(
        &mut obj,
        tags::RTROI_OBSERVATIONS_SEQUENCE,
        ROIS.iter().map(roi_observation_item).collect(),
    );

    write_file(path, obj)
}

fn write_case_manifest(path: &Path, ct_files: &[PathBuf], rtstruct_file: &Path) -> Result<()> {
    let mut artifacts = Vec::with_capacity(ct_files.len() + 1);
    for (slice_index, ct_file) in ct_files.iter().enumerate() {
        artifacts.push(ArtifactRecord {
            role: "dicom_ct_slice".into(),
            path: format!("ct/ct-{slice_index:03}.dcm"),
            sha256: hash_file(ct_file)?,
            media_type: Some("application/dicom".into()),
        });
    }
    artifacts.push(ArtifactRecord {
        role: "dicom_rt_structure_set".into(),
        path: "rtstruct.dcm".into(),
        sha256: hash_file(rtstruct_file)?,
        media_type: Some("application/dicom".into()),
    });

    let structures = ROIS
        .iter()
        .map(|roi| {
            let voxel_count = [roi.x, roi.y, roi.z]
                .into_iter()
                .map(|bounds| ((bounds[1] - bounds[0]) / SPACING_MM).round() as usize)
                .product();
            StructureRecord {
                number: roi.number,
                name: roi.name.into(),
                voxel_count,
                volume_cm3: voxel_count as f64 * SPACING_MM.powi(3) / 1_000.0,
                centroid_lps_mm: [
                    (roi.x[0] + roi.x[1]) / 2.0,
                    (roi.y[0] + roi.y[1]) / 2.0,
                    (roi.z[0] + roi.z[1]) / 2.0,
                ],
            }
        })
        .collect();
    let manifest = CaseManifest {
        schema_version: "openbnct.case-manifest/0.1.0".into(),
        case_id: CASE_ID.into(),
        qualification: QualificationBoundary::SyntheticResearchOnly,
        coordinate_system: PatientCoordinateSystem::DicomLpsMillimeters,
        geometry: GridGeometry {
            shape: [COLUMNS as u32, ROWS as u32, SLICES as u32],
            spacing_mm: [SPACING_MM; 3],
            origin_mm: [FIRST_CENTER_MM; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        frame_of_reference_uid: FRAME_OF_REFERENCE_UID.into(),
        study_instance_uid: STUDY_INSTANCE_UID.into(),
        imaging_series_instance_uid: CT_SERIES_INSTANCE_UID.into(),
        structure_set_series_instance_uid: RTSTRUCT_SERIES_INSTANCE_UID.into(),
        structure_set_instance_uid: RTSTRUCT_INSTANCE_UID.into(),
        material_model_id: "openbnct.nf-bnct-001.material.v1".into(),
        source_model_id: "openbnct.nf-bnct-001.source.v1".into(),
        structures,
        artifacts,
    };
    manifest.validate()?;
    let mut json = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        DicomError::Benchmark(format!("cannot serialize case manifest: {error}"))
    })?;
    json.push(b'\n');
    fs::write(path, json).map_err(|source| DicomError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn hash_file(path: &Path) -> Result<String> {
    sha256_file(path).map_err(|source| DicomError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn referenced_frame_item(slice_uids: &[String]) -> InMemDicomObject {
    let contour_images: Vec<_> = slice_uids
        .iter()
        .map(|uid| referenced_image_item(uids::CT_IMAGE_STORAGE, uid))
        .collect();
    let mut series = InMemDicomObject::new_empty();
    put_str(
        &mut series,
        tags::SERIES_INSTANCE_UID,
        VR::UI,
        CT_SERIES_INSTANCE_UID,
    );
    put_sequence(&mut series, tags::CONTOUR_IMAGE_SEQUENCE, contour_images);

    let mut study = InMemDicomObject::new_empty();
    put_str(
        &mut study,
        tags::REFERENCED_SOP_CLASS_UID,
        VR::UI,
        DETACHED_STUDY_MANAGEMENT_UID,
    );
    put_str(
        &mut study,
        tags::REFERENCED_SOP_INSTANCE_UID,
        VR::UI,
        STUDY_INSTANCE_UID,
    );
    put_sequence(
        &mut study,
        tags::RT_REFERENCED_SERIES_SEQUENCE,
        vec![series],
    );

    let mut frame = InMemDicomObject::new_empty();
    put_str(
        &mut frame,
        tags::FRAME_OF_REFERENCE_UID,
        VR::UI,
        FRAME_OF_REFERENCE_UID,
    );
    put_sequence(&mut frame, tags::RT_REFERENCED_STUDY_SEQUENCE, vec![study]);
    frame
}

fn structure_set_roi_item(roi: &RoiBox) -> InMemDicomObject {
    let mut item = InMemDicomObject::new_empty();
    put_str(&mut item, tags::ROI_NUMBER, VR::IS, &roi.number.to_string());
    put_str(
        &mut item,
        tags::REFERENCED_FRAME_OF_REFERENCE_UID,
        VR::UI,
        FRAME_OF_REFERENCE_UID,
    );
    put_str(&mut item, tags::ROI_NAME, VR::LO, roi.name);
    put_str(
        &mut item,
        tags::ROI_GENERATION_ALGORITHM,
        VR::CS,
        "AUTOMATIC",
    );
    item
}

fn roi_contour_item(roi: RoiBox, slice_uids: &[String]) -> InMemDicomObject {
    let contours = (0..SLICES)
        .filter(|slice_index| {
            let z = slice_center(*slice_index);
            z >= roi.z[0] && z < roi.z[1]
        })
        .enumerate()
        .map(|(contour_index, slice_index)| {
            contour_item(
                roi,
                slice_index,
                contour_index + 1,
                &slice_uids[slice_index],
            )
        })
        .collect();

    let mut item = InMemDicomObject::new_empty();
    put_str(
        &mut item,
        tags::ROI_DISPLAY_COLOR,
        VR::IS,
        &format!("{}\\{}\\{}", roi.color[0], roi.color[1], roi.color[2]),
    );
    put_sequence(&mut item, tags::CONTOUR_SEQUENCE, contours);
    put_str(
        &mut item,
        tags::REFERENCED_ROI_NUMBER,
        VR::IS,
        &roi.number.to_string(),
    );
    item
}

fn contour_item(
    roi: RoiBox,
    slice_index: usize,
    contour_number: usize,
    slice_uid: &str,
) -> InMemDicomObject {
    let z = slice_center(slice_index);
    let coordinates = [
        roi.x[0], roi.y[0], z, roi.x[1], roi.y[0], z, roi.x[1], roi.y[1], z, roi.x[0], roi.y[1], z,
    ];
    let mut item = InMemDicomObject::new_empty();
    put_sequence(
        &mut item,
        tags::CONTOUR_IMAGE_SEQUENCE,
        vec![referenced_image_item(uids::CT_IMAGE_STORAGE, slice_uid)],
    );
    put_str(
        &mut item,
        tags::CONTOUR_GEOMETRIC_TYPE,
        VR::CS,
        "CLOSED_PLANAR",
    );
    put_str(&mut item, tags::NUMBER_OF_CONTOUR_POINTS, VR::IS, "4");
    put_str(
        &mut item,
        tags::CONTOUR_NUMBER,
        VR::IS,
        &contour_number.to_string(),
    );
    put_str(
        &mut item,
        tags::CONTOUR_DATA,
        VR::DS,
        &ds_values(&coordinates),
    );
    item
}

fn referenced_image_item(sop_class_uid: &str, sop_instance_uid: &str) -> InMemDicomObject {
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

fn roi_observation_item(roi: &RoiBox) -> InMemDicomObject {
    let mut item = InMemDicomObject::new_empty();
    put_str(
        &mut item,
        tags::OBSERVATION_NUMBER,
        VR::IS,
        &roi.number.to_string(),
    );
    put_str(
        &mut item,
        tags::REFERENCED_ROI_NUMBER,
        VR::IS,
        &roi.number.to_string(),
    );
    put_str(&mut item, tags::RTROI_INTERPRETED_TYPE, VR::CS, "ORGAN");
    put_str(&mut item, tags::ROI_INTERPRETER, VR::PN, "");
    item
}

fn write_file(path: &Path, obj: InMemDicomObject) -> Result<()> {
    let file = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .implementation_class_uid(implementation_class_uid())
                .implementation_version_name("OPENBNCT_0_1")
                .source_application_entity_title("NCTFORGE"),
        )
        .map_err(|source| DicomError::Write {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;
    file.write_to_file(path)
        .map_err(|source| DicomError::Write {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
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

fn slice_center(slice_index: usize) -> f64 {
    FIRST_CENTER_MM + slice_index as f64 * SPACING_MM
}

fn ds_values(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("\\")
}

fn create_dir(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::AlreadyExists {
            DicomError::OutputExists(path.to_path_buf())
        } else {
            DicomError::Io {
                path: path.to_path_buf(),
                source,
            }
        }
    })
}

/// Open all generated files to ensure the Part 10 encoding round-trips.
/// This is used by tests and is public for lightweight downstream smoke checks.
pub fn validate_part10_files(case: &GeneratedCase) -> Result<()> {
    for path in case
        .ct_files
        .iter()
        .chain(std::iter::once(&case.rtstruct_file))
    {
        open_file(path).map_err(|source| DicomError::Read {
            path: path.clone(),
            source: Box::new(source),
        })?;
    }
    Ok(())
}
