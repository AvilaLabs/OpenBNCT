// SPDX-License-Identifier: Apache-2.0

//! DICOM PET series import with body-weight SUV (SUVbw) quantification.
//!
//! The importer reuses the CT slice pipeline — same Explicit VR Little
//! Endian single-frame gates and geometry assembly — under the PET SOP
//! class and `PT` modality, then converts rescaled activity
//! concentrations into the QIBA body-weight SUV convention:
//!
//! ```text
//! SUV(v) = C(v)[Bq/ml] · weight[g] / D_scan[Bq]
//! D_scan = D_injected · 2^(−Δt/T½)   Δt = scan time − radiopharmaceutical start
//! ```
//!
//! Scope is honest: only `Units = BQML` series can produce SUVbw —
//! count or SUV-normalized units are rejected rather than guessed —
//! and `Decay Correction` must be `START` (the pixel values are
//! decay-corrected to the scan start, so the injected dose is decayed
//! to the same epoch). Half-life is read from the radiopharmaceutical
//! item, so the import is tracer-agnostic.

use std::path::PathBuf;

use dicom_core::Tag;
use dicom_dictionary_std::{tags, uids};
use dicom_object::mem::InMemDicomObject;
use dicom_object::open_file;
use openbnct_core::GridGeometry;

use crate::ct::{PixelKind, assemble_series, attribute_error, float, read_series, string};
use crate::{DicomError, Result};

const UNITS: Tag = Tag(0x0054, 0x1001);
const DECAY_CORRECTION: Tag = Tag(0x0054, 0x1002);
const RADIOPHARMACEUTICAL_INFORMATION: Tag = Tag(0x0054, 0x0016);
const RADIONUCLIDE_TOTAL_DOSE: Tag = Tag(0x0018, 0x1074);
const RADIOPHARMACEUTICAL_START_TIME: Tag = Tag(0x0018, 0x1072);
const RADIONUCLIDE_HALF_LIFE: Tag = Tag(0x0018, 0x1075);
const PATIENT_WEIGHT: Tag = Tag(0x0010, 0x1030);

/// A validated PET series converted to SUVbw on the shared volume grid.
#[derive(Debug, Clone, PartialEq)]
pub struct PetVolume {
    pub geometry: GridGeometry,
    pub frame_of_reference_uid: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    /// SOP Instance UIDs ordered along the positive slice-normal axis.
    pub slice_sop_instance_uids: Vec<String>,
    /// SUVbw per voxel in grid order `i + nx·j + nx·ny·k`.
    pub suv: Vec<f64>,
    /// Patient's Weight (0010,1030) in kg.
    pub patient_weight_kg: f64,
    /// Radionuclide Total Dose (0018,1074) in Bq at injection.
    pub injected_dose_bq: f64,
    /// Injected dose decayed to the scan start epoch, Bq.
    pub decayed_dose_bq: f64,
    /// Radionuclide Half Life (0018,1075) in seconds.
    pub radionuclide_half_life_s: f64,
    /// Seconds between radiopharmaceutical start and scan start.
    pub delta_t_s: f64,
    /// Voxels whose rescaled activity was negative and clamped to zero —
    /// recorded, not hidden (same honesty convention as the boron field).
    pub clamped_negative_voxels: u64,
}

impl PetVolume {
    /// The resampled-onto-transport-grid SUV values the boron uptake model
    /// consumes — a plain `&[f64]` slice in grid order.
    #[must_use]
    pub fn suv_values(&self) -> &[f64] {
        &self.suv
    }
}

/// Import a PET series and convert to SUVbw.
///
/// The series must carry `Units = BQML`, `Decay Correction = START` (or
/// be absent — tolerated for older exports that pre-date the tag's wide
/// adoption but carry the same convention), a positive patient weight,
/// and a complete radiopharmaceutical record (dose, start time, half
/// life). Scan time is Acquisition Time, falling back to Series Time;
/// a scan before the start time is treated as a midnight crossing.
pub fn import_pet_series(paths: &[PathBuf]) -> Result<PetVolume> {
    // PET slices may be signed or unsigned 16-bit; both decode exactly
    // into i32.
    let slices = read_series(
        paths,
        uids::POSITRON_EMISSION_TOMOGRAPHY_IMAGE_STORAGE,
        "PT",
        PixelKind::Either16,
    )?;
    let reference_path = slices
        .first()
        .ok_or_else(|| DicomError::EmptySeries)?
        .path
        .clone();
    let assembled = assemble_series(slices)?;

    // Series-level SUV tags live on every slice; read them from the
    // reference file (all slices were consistency-checked on the shared
    // identifiers, and SUV tags are series attributes).
    let obj = open_file(&reference_path).map_err(|source| DicomError::Read {
        path: reference_path.clone(),
        source: Box::new(source),
    })?;
    let units = string(&obj, &reference_path, UNITS, "Units")?;
    if units != "BQML" {
        return Err(attribute_error(
            &reference_path,
            "Units",
            format!("SUVbw requires BQML, found {units:?}"),
        ));
    }
    let decay_correction = obj
        .element(DECAY_CORRECTION)
        .ok()
        .and_then(|element| element.to_str().ok())
        .map(|value| value.trim_end_matches([' ', '\0']).to_owned())
        .unwrap_or_else(|| "START".into());
    if decay_correction != "START" {
        return Err(attribute_error(
            &reference_path,
            "Decay Correction",
            format!("expected START, found {decay_correction:?}"),
        ));
    }

    let patient_weight_kg = float(&obj, &reference_path, PATIENT_WEIGHT, "Patient's Weight")?;
    if !patient_weight_kg.is_finite() || patient_weight_kg <= 0.0 {
        return Err(attribute_error(
            &reference_path,
            "Patient's Weight",
            "must be finite and positive for SUVbw",
        ));
    }

    let items = obj
        .element(RADIOPHARMACEUTICAL_INFORMATION)
        .map_err(|error| {
            attribute_error(
                &reference_path,
                "Radiopharmaceutical Information Sequence",
                error.to_string(),
            )
        })?
        .items()
        .ok_or_else(|| {
            attribute_error(
                &reference_path,
                "Radiopharmaceutical Information Sequence",
                "expected a data set sequence",
            )
        })?;
    if items.len() != 1 {
        return Err(attribute_error(
            &reference_path,
            "Radiopharmaceutical Information Sequence",
            format!("expected exactly one item, found {}", items.len()),
        ));
    }
    let radiopharm = &items[0];
    let injected_dose_bq = item_float(
        radiopharm,
        &reference_path,
        RADIONUCLIDE_TOTAL_DOSE,
        "Radionuclide Total Dose",
    )?;
    if !injected_dose_bq.is_finite() || injected_dose_bq <= 0.0 {
        return Err(attribute_error(
            &reference_path,
            "Radionuclide Total Dose",
            "must be finite and positive for SUVbw",
        ));
    }
    let half_life_s = item_float(
        radiopharm,
        &reference_path,
        RADIONUCLIDE_HALF_LIFE,
        "Radionuclide Half Life",
    )?;
    if !half_life_s.is_finite() || half_life_s <= 0.0 {
        return Err(attribute_error(
            &reference_path,
            "Radionuclide Half Life",
            "must be finite and positive for SUVbw",
        ));
    }
    let start_time = item_string(
        radiopharm,
        &reference_path,
        RADIOPHARMACEUTICAL_START_TIME,
        "Radiopharmaceutical Start Time",
    )?;
    let start_s = parse_tm(
        &reference_path,
        "Radiopharmaceutical Start Time",
        &start_time,
    )?;

    let scan_time = obj
        .element(tags::ACQUISITION_TIME)
        .ok()
        .and_then(|element| element.to_str().ok())
        .or_else(|| {
            obj.element(tags::SERIES_TIME)
                .ok()
                .and_then(|element| element.to_str().ok())
        })
        .map(|value| value.trim_end_matches([' ', '\0']).to_owned())
        .ok_or_else(|| {
            attribute_error(
                &reference_path,
                "Acquisition/Series Time",
                "neither is present",
            )
        })?;
    let scan_s = parse_tm(&reference_path, "Acquisition/Series Time", &scan_time)?;

    // Midnight crossing: scan logically after the injection even when the
    // clock wrapped.
    let delta_t_s = if scan_s >= start_s {
        scan_s - start_s
    } else {
        scan_s + 86_400.0 - start_s
    };
    let decayed_dose_bq =
        injected_dose_bq * (-std::f64::consts::LN_2 * delta_t_s / half_life_s).exp();
    // SUVbw factor: g per (Bq/ml) — the standard body-weight convention.
    let suv_factor = patient_weight_kg * 1000.0 / decayed_dose_bq;

    let mut clamped_negative_voxels = 0_u64;
    let suv: Vec<f64> = assembled
        .stored_pixels
        .iter()
        .map(|stored| {
            let bqml =
                f64::from(*stored).mul_add(assembled.rescale_slope, assembled.rescale_intercept);
            let value = bqml * suv_factor;
            if value < 0.0 {
                clamped_negative_voxels += 1;
                0.0
            } else {
                value
            }
        })
        .collect();

    Ok(PetVolume {
        geometry: assembled.geometry,
        frame_of_reference_uid: assembled.frame_of_reference_uid,
        study_instance_uid: assembled.study_instance_uid,
        series_instance_uid: assembled.series_instance_uid,
        slice_sop_instance_uids: assembled.slice_sop_instance_uids,
        suv,
        patient_weight_kg,
        injected_dose_bq,
        decayed_dose_bq,
        radionuclide_half_life_s: half_life_s,
        delta_t_s,
        clamped_negative_voxels,
    })
}

/// Parse a DICOM TM string (`HHMMSS[.FFFFFF]`) into seconds of day.
fn parse_tm(path: &std::path::Path, name: &'static str, value: &str) -> Result<f64> {
    let value = value.trim();
    let (hms, fraction) = value.split_once('.').unwrap_or((value, "0"));
    if hms.len() < 6 || !hms[..6].chars().all(|c| c.is_ascii_digit()) {
        return Err(attribute_error(
            path,
            name,
            format!("expected HHMMSS[.FFFFFF], found {value:?}"),
        ));
    }
    let digits = |range: &str| range.parse::<f64>();
    let hours = digits(&hms[0..2]).map_err(|e| attribute_error(path, name, e.to_string()))?;
    let minutes = digits(&hms[2..4]).map_err(|e| attribute_error(path, name, e.to_string()))?;
    let seconds = digits(&hms[4..6]).map_err(|e| attribute_error(path, name, e.to_string()))?;
    let frac = format!("0.{fraction}")
        .parse::<f64>()
        .map_err(|e: std::num::ParseFloatError| attribute_error(path, name, e.to_string()))?;
    if hours >= 24.0 || minutes >= 60.0 || seconds >= 60.0 {
        return Err(attribute_error(
            path,
            name,
            format!("out-of-range time {value:?}"),
        ));
    }
    Ok(hours * 3600.0 + minutes * 60.0 + seconds + frac)
}

/// Read a float tag on a sequence item (same semantics as `ct::float`
/// on the file object).
fn item_float(
    obj: &InMemDicomObject,
    path: &std::path::Path,
    tag: Tag,
    name: &'static str,
) -> Result<f64> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_float64()
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

/// Read a string tag on a sequence item.
fn item_string(
    obj: &InMemDicomObject,
    path: &std::path::Path,
    tag: Tag,
    name: &'static str,
) -> Result<String> {
    obj.element(tag)
        .map_err(|error| attribute_error(path, name, error.to_string()))?
        .to_str()
        .map(|value| value.trim_end_matches([' ', '\0']).to_owned())
        .map_err(|error| attribute_error(path, name, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::value::{PrimitiveValue, Value};
    use dicom_core::{DataElement, Length, VR};
    use dicom_object::mem::InMemDicomObject;
    use dicom_object::meta::FileMetaTableBuilder;

    fn put_str(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
        obj.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
    }

    fn put_u16(obj: &mut InMemDicomObject, tag: Tag, value: u16) {
        obj.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
    }

    fn write_part10(path: &std::path::Path, obj: InMemDicomObject) {
        let file = obj
            .with_meta(
                FileMetaTableBuilder::new()
                    .media_storage_sop_class_uid(uids::POSITRON_EMISSION_TOMOGRAPHY_IMAGE_STORAGE)
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .implementation_class_uid("9.9.0")
                    .implementation_version_name("OPENBNCT_TEST"),
            )
            .unwrap();
        file.write_to_file(path).unwrap();
    }

    fn radiopharm_item() -> InMemDicomObject {
        let mut item = InMemDicomObject::new_empty();
        put_str(&mut item, Tag(0x0018, 0x1072), VR::TM, "120000");
        put_str(&mut item, Tag(0x0018, 0x1074), VR::DS, "400000000");
        put_str(&mut item, Tag(0x0018, 0x1075), VR::DS, "6586");
        item
    }

    /// Minimal valid PET slice: 2×2 unsigned pixels, F-18
    /// radiopharmaceutical record, acquisition one hour post-injection.
    /// `units` parameterizes the Units tag so rejection paths can build
    /// a non-BQML series.
    fn write_pet_slice(
        dir: &std::path::Path,
        index: usize,
        pixels: &[u16; 4],
        units: &str,
    ) -> PathBuf {
        let mut obj = InMemDicomObject::new_empty();
        put_str(
            &mut obj,
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::POSITRON_EMISSION_TOMOGRAPHY_IMAGE_STORAGE,
        );
        put_str(
            &mut obj,
            tags::SOP_INSTANCE_UID,
            VR::UI,
            &format!("9.9.9.{index}"),
        );
        put_str(&mut obj, tags::STUDY_INSTANCE_UID, VR::UI, "9.9.1");
        put_str(&mut obj, tags::SERIES_INSTANCE_UID, VR::UI, "9.9.2");
        put_str(&mut obj, tags::FRAME_OF_REFERENCE_UID, VR::UI, "9.9.3");
        put_str(&mut obj, tags::MODALITY, VR::CS, "PT");
        put_str(&mut obj, tags::SERIES_TIME, VR::TM, "130000");
        put_str(&mut obj, tags::ACQUISITION_TIME, VR::TM, "130000");
        put_str(&mut obj, PATIENT_WEIGHT, VR::DS, "75.0");
        put_str(&mut obj, UNITS, VR::CS, units);
        put_str(&mut obj, DECAY_CORRECTION, VR::CS, "START");
        obj.put(DataElement::new(
            RADIOPHARMACEUTICAL_INFORMATION,
            VR::SQ,
            Value::new_sequence(vec![radiopharm_item()], Length::UNDEFINED),
        ));
        put_str(&mut obj, tags::PATIENT_POSITION, VR::CS, "HFS");
        put_str(
            &mut obj,
            tags::IMAGE_POSITION_PATIENT,
            VR::DS,
            &format!("0\\0\\{}", index as f64 * 4.0),
        );
        put_str(
            &mut obj,
            tags::IMAGE_ORIENTATION_PATIENT,
            VR::DS,
            "1\\0\\0\\0\\1\\0",
        );
        put_u16(&mut obj, tags::SAMPLES_PER_PIXEL, 1);
        put_str(
            &mut obj,
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            "MONOCHROME2",
        );
        put_u16(&mut obj, tags::ROWS, 2);
        put_u16(&mut obj, tags::COLUMNS, 2);
        put_str(&mut obj, tags::PIXEL_SPACING, VR::DS, "4\\4");
        put_u16(&mut obj, tags::BITS_ALLOCATED, 16);
        put_u16(&mut obj, tags::BITS_STORED, 16);
        put_u16(&mut obj, tags::HIGH_BIT, 15);
        put_u16(&mut obj, tags::PIXEL_REPRESENTATION, 0);
        put_str(&mut obj, tags::RESCALE_INTERCEPT, VR::DS, "0");
        put_str(&mut obj, tags::RESCALE_SLOPE, VR::DS, "100");
        obj.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(pixels.to_vec().into()),
        ));
        let path = dir.join(format!("pet{index}.dcm"));
        write_part10(&path, obj);
        path
    }

    #[test]
    fn pet_suvbw_math() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_pet_slice(dir.path(), 0, &[10, 20, 30, 40], "BQML");
        let b = write_pet_slice(dir.path(), 1, &[50, 60, 70, 80], "BQML");
        let volume = import_pet_series(&[b, a]).unwrap();

        assert_eq!(volume.geometry.shape, [2, 2, 2]);
        assert_eq!(volume.geometry.spacing_mm, [4.0, 4.0, 4.0]);
        // decayed dose: 400 MBq × 2^(−3600/6586)
        let decayed = 400e6 * (-std::f64::consts::LN_2 * 3600.0 / 6586.0).exp();
        let factor = 75_000.0 / decayed;
        assert!((volume.decayed_dose_bq - decayed).abs() < 1.0);
        // voxel 0 stored=10 → 10×100 = 1000 Bq/ml → 1000×factor SUV
        let expected0 = 1000.0 * factor;
        assert!((volume.suv[0] - expected0).abs() < 1e-6 * expected0);
        assert_eq!(volume.clamped_negative_voxels, 0);
    }

    #[test]
    fn pet_rejects_non_bqml() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_pet_slice(dir.path(), 0, &[1, 1, 1, 1], "CNTS");
        let b = write_pet_slice(dir.path(), 1, &[1, 1, 1, 1], "CNTS");
        assert!(import_pet_series(&[a, b]).is_err());
    }
}
