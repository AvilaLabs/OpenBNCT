// SPDX-License-Identifier: MIT

//! DICOM RT Plan read/write (`openbnct-dicom::rtplan`).
//!
//! `summarize_rt_plan` parses an RTPLAN object into the
//! `openbnct.rtplan-summary/0.1.0` record — plan identity, fraction
//! groups with their referenced-beam metersets, and per-beam geometry
//! (gantry, collimator, couch angles, isocenter, SSD) from the first
//! control point of each static field. Reading is the
//! interoperability-critical direction: it lets a TPS- or trial-produced
//! plan be inspected against OpenBNCT artifacts.
//!
//! `export_rt_plan` writes a minimal, valid RTPLAN — one fraction group
//! referencing the caller-declared static beams — for research
//! interop. BNCT deliveries are fixed-field, so the writer deliberately
//! supports only static single-control-point beams; MLC motion,
//! dynamic delivery, and branching control points are out of scope.
//! The export carries the same research qualification as RTDOSE export:
//! it is not a commissioned treatment-planning product.

use std::path::{Path, PathBuf};

use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::{DataElement, Length, Tag, VR};
use dicom_dictionary_std::{tags, uids};
use dicom_object::{InMemDicomObject, open_file};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DicomError, Result};

const RT_PLAN_SOP_CLASS: &str = "1.2.840.10008.5.1.4.1.1.481.5";
/// Deterministic fixed date/time: exported objects are generated
/// artifacts, not patient records.
const EXPORT_DATE: &str = "19700101";
const EXPORT_TIME: &str = "000000";

/// Versioned summary schema for the read direction.
pub const RTPLAN_SUMMARY_SCHEMA: &str = "openbnct.rtplan-summary/0.1.0";

/// One control point of a static beam: the angles and isocenter a plan
/// delivers the field from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtPlanControlPoint {
    pub control_point_index: i32,
    pub gantry_angle_deg: Option<f64>,
    pub gantry_rotation_direction: Option<String>,
    pub beam_limiting_device_angle_deg: Option<f64>,
    pub patient_support_angle_deg: Option<f64>,
    /// Isocenter position `[x, y, z]` mm in the patient coordinate frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isocenter_position_mm: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_to_surface_distance_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nominal_beam_energy_mev: Option<f64>,
}

/// One beam in the plan's beam sequence, plus the meterset each
/// fraction group assigns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtPlanBeamSummary {
    pub beam_number: i32,
    pub beam_name: Option<String>,
    pub beam_type: Option<String>,
    pub radiation_type: Option<String>,
    pub treatment_delivery_type: Option<String>,
    pub treatment_machine_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_axis_distance_mm: Option<f64>,
    /// First control point — the static delivery geometry.
    pub control_point: Option<RtPlanControlPoint>,
    /// Metersets assigned by each fraction group, in fraction-group order.
    pub metersets: Vec<f64>,
}

/// One fraction group and its referenced-beam meterset table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtPlanFractionGroup {
    pub fraction_group_number: i32,
    pub number_of_fractions_planned: Option<i32>,
    /// `(referenced_beam_number, meterset)` pairs.
    pub referenced_beams: Vec<(i32, Option<f64>)>,
}

/// `openbnct.rtplan-summary/0.1.0` — what an RT Plan declares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RtPlanSummary {
    pub schema_version: String,
    /// Source file's SOP Instance UID.
    pub sop_instance_uid: String,
    pub rt_plan_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_plan_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rt_plan_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_intent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patient_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patient_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_of_reference_uid: Option<String>,
    pub fraction_groups: Vec<RtPlanFractionGroup>,
    pub beams: Vec<RtPlanBeamSummary>,
}

/// Caller-declared beam for `export_rt_plan`.
#[derive(Debug, Clone)]
pub struct RtPlanBeamSpec {
    pub name: String,
    /// IEC gantry angle in degrees; a fixed horizontal BNCT port is
    /// conventionally 90° or 270°.
    pub gantry_angle_deg: f64,
    /// Beam-limiting-device (collimator) angle in degrees.
    pub collimator_angle_deg: f64,
    /// Patient-support (couch) rotation in degrees.
    pub patient_support_angle_deg: f64,
    /// Isocenter `[x, y, z]` mm in the patient frame.
    pub isocenter_position_mm: [f64; 3],
    /// Source-axis distance in mm (machine constant; e.g. 1000).
    pub source_axis_distance_mm: f64,
    /// Source-to-surface distance in mm.
    pub source_to_surface_distance_mm: f64,
    /// DICOM RadiationType CS term (`NEUTRON`, `PHOTON`, …).
    pub radiation_type: String,
    /// Meterset delivered per fraction.
    pub meterset: f64,
    /// Optional beam energy label (MeV).
    pub nominal_beam_energy_mev: Option<f64>,
}

/// Identity and plan-level context for `export_rt_plan`.
#[derive(Debug, Clone)]
pub struct RtPlanExportOptions {
    pub patient_name: String,
    pub patient_id: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub sop_instance_uid: String,
    /// Frame of Reference the isocenters live in (the source CT's).
    pub frame_of_reference_uid: String,
    pub plan_label: String,
    pub plan_name: String,
    /// DICOM PlanIntent; `RESEARCH` is not a defined term — callers
    /// typically declare `CURATIVE` or leave `VERIFICATION`.
    pub plan_intent: String,
    pub number_of_fractions: i32,
    pub treatment_machine_name: String,
    pub beams: Vec<RtPlanBeamSpec>,
}

fn attribute_error(path: &Path, attribute: &'static str, detail: impl Into<String>) -> DicomError {
    DicomError::Attribute {
        path: PathBuf::from(path),
        attribute,
        detail: detail.into(),
    }
}

fn opt_string(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|v| v.trim_end_matches([' ', '\0']).to_owned())
        .filter(|v| !v.is_empty())
}

fn opt_float(obj: &InMemDicomObject, tag: Tag) -> Option<f64> {
    obj.element(tag).ok().and_then(|e| e.to_float64().ok())
}

fn opt_int(obj: &InMemDicomObject, tag: Tag) -> Option<i32> {
    obj.element(tag).ok().and_then(|e| e.to_int::<i32>().ok())
}

fn opt_floats(obj: &InMemDicomObject, tag: Tag) -> Option<Vec<f64>> {
    obj.element(tag)
        .ok()
        .and_then(|e| e.to_multi_float64().ok())
}

fn opt_sequence(obj: &InMemDicomObject, tag: Tag) -> Option<&[InMemDicomObject]> {
    obj.element(tag).ok().and_then(|e| e.items())
}

/// Parse an RTPLAN file into an `openbnct.rtplan-summary/0.1.0` record.
///
/// Only Explicit VR Little Endian transfer syntax is accepted, matching
/// the crate's other importers. Every field the schema marks optional is
/// genuinely optional in DICOM; absent attributes report `None` rather
/// than erroring, so partial research plans still summarize.
pub fn summarize_rt_plan(path: &Path) -> Result<RtPlanSummary> {
    let obj = open_file(path).map_err(|source| DicomError::Read {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
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
    let sop_class = opt_string(&obj, tags::SOP_CLASS_UID)
        .unwrap_or_else(|| obj.meta().media_storage_sop_class_uid().to_owned());
    if sop_class != RT_PLAN_SOP_CLASS {
        return Err(attribute_error(
            path,
            "SOP Class UID",
            format!("expected {RT_PLAN_SOP_CLASS}, found {sop_class:?}"),
        ));
    }

    let sop_instance_uid = opt_string(&obj, tags::SOP_INSTANCE_UID)
        .unwrap_or_else(|| obj.meta().media_storage_sop_instance_uid().to_owned());
    let rt_plan_label = opt_string(&obj, tags::RT_PLAN_LABEL)
        .ok_or_else(|| attribute_error(path, "RT Plan Label", "required attribute absent"))?;

    let mut fraction_groups = Vec::new();
    let mut group_metersets: Vec<Vec<(i32, Option<f64>)>> = Vec::new();
    if let Some(groups) = opt_sequence(&obj, tags::FRACTION_GROUP_SEQUENCE) {
        for group in groups {
            let mut referenced = Vec::new();
            if let Some(refs) = opt_sequence(group, tags::REFERENCED_BEAM_SEQUENCE) {
                for r in refs {
                    referenced.push((
                        opt_int(r, tags::REFERENCED_BEAM_NUMBER).unwrap_or(0),
                        opt_float(r, tags::BEAM_METERSET),
                    ));
                }
            }
            group_metersets.push(referenced.clone());
            fraction_groups.push(RtPlanFractionGroup {
                fraction_group_number: opt_int(group, tags::FRACTION_GROUP_NUMBER).unwrap_or(0),
                number_of_fractions_planned: opt_int(group, tags::NUMBER_OF_FRACTIONS_PLANNED),
                referenced_beams: referenced,
            });
        }
    }

    let mut beams = Vec::new();
    if let Some(beam_items) = opt_sequence(&obj, tags::BEAM_SEQUENCE) {
        for beam in beam_items {
            let number = opt_int(beam, tags::BEAM_NUMBER).unwrap_or(0);
            let control_point = opt_sequence(beam, tags::CONTROL_POINT_SEQUENCE)
                .and_then(|cps| {
                    cps.iter()
                        .find(|cp| opt_int(cp, tags::CONTROL_POINT_INDEX) == Some(0))
                })
                .map(|cp| RtPlanControlPoint {
                    control_point_index: 0,
                    gantry_angle_deg: opt_float(cp, tags::GANTRY_ANGLE),
                    gantry_rotation_direction: opt_string(cp, tags::GANTRY_ROTATION_DIRECTION),
                    beam_limiting_device_angle_deg: opt_float(cp, tags::BEAM_LIMITING_DEVICE_ANGLE),
                    patient_support_angle_deg: opt_float(cp, tags::PATIENT_SUPPORT_ANGLE),
                    isocenter_position_mm: opt_floats(cp, tags::ISOCENTER_POSITION)
                        .and_then(|v| (v.len() == 3).then(|| [v[0], v[1], v[2]])),
                    source_to_surface_distance_mm: opt_float(cp, tags::SOURCE_TO_SURFACE_DISTANCE),
                    nominal_beam_energy_mev: opt_float(cp, tags::NOMINAL_BEAM_ENERGY),
                });
            let metersets = group_metersets
                .iter()
                .filter_map(|refs| {
                    refs.iter()
                        .find(|(n, _)| *n == number)
                        .and_then(|(_, m)| *m)
                })
                .collect();
            beams.push(RtPlanBeamSummary {
                beam_number: number,
                beam_name: opt_string(beam, tags::BEAM_NAME),
                beam_type: opt_string(beam, tags::BEAM_TYPE),
                radiation_type: opt_string(beam, tags::RADIATION_TYPE),
                treatment_delivery_type: opt_string(beam, tags::TREATMENT_DELIVERY_TYPE),
                treatment_machine_name: opt_string(beam, tags::TREATMENT_MACHINE_NAME),
                source_axis_distance_mm: opt_float(beam, tags::SOURCE_AXIS_DISTANCE),
                control_point,
                metersets,
            });
        }
    }

    Ok(RtPlanSummary {
        schema_version: RTPLAN_SUMMARY_SCHEMA.into(),
        sop_instance_uid,
        rt_plan_label,
        rt_plan_name: opt_string(&obj, tags::RT_PLAN_NAME),
        rt_plan_date: opt_string(&obj, tags::RT_PLAN_DATE),
        plan_intent: opt_string(&obj, tags::PLAN_INTENT),
        patient_name: opt_string(&obj, tags::PATIENT_NAME),
        patient_id: opt_string(&obj, tags::PATIENT_ID),
        frame_of_reference_uid: opt_string(&obj, tags::FRAME_OF_REFERENCE_UID),
        fraction_groups,
        beams,
    })
}

fn put_str(obj: &mut InMemDicomObject, tag: Tag, vr: VR, value: &str) {
    obj.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
}

fn put_sequence(obj: &mut InMemDicomObject, tag: Tag, items: Vec<InMemDicomObject>) {
    obj.put(DataElement::new(
        tag,
        VR::SQ,
        Value::new_sequence(items, Length::UNDEFINED),
    ));
}

fn ds(value: f64) -> String {
    format!("{value:.10}")
}

fn ds_values(values: &[f64]) -> String {
    values.iter().map(|v| ds(*v)).collect::<Vec<_>>().join("\\")
}

/// Write a minimal static-beam RTPLAN.
///
/// One fraction group plans `number_of_fractions` deliveries of every
/// declared beam at its per-fraction meterset; each beam is `STATIC`
/// with a single control point carrying its angles and isocenter. UIDs
/// may be left empty to derive deterministic `2.25.*` UIDs from
/// `plan_label`.
pub fn export_rt_plan(path: &Path, options: &RtPlanExportOptions) -> Result<()> {
    if path.exists() {
        return Err(DicomError::OutputExists(path.to_path_buf()));
    }
    if options.beams.is_empty() {
        return Err(DicomError::Attribute {
            path: path.to_path_buf(),
            attribute: "beams",
            detail: "at least one beam is required".into(),
        });
    }
    for beam in &options.beams {
        for value in [
            beam.gantry_angle_deg,
            beam.collimator_angle_deg,
            beam.patient_support_angle_deg,
            beam.meterset,
            beam.source_axis_distance_mm,
            beam.source_to_surface_distance_mm,
        ]
        .into_iter()
        .chain(beam.isocenter_position_mm)
        {
            if !value.is_finite() {
                return Err(DicomError::Attribute {
                    path: path.to_path_buf(),
                    attribute: "beams",
                    detail: format!("beam {:?} carries a non-finite value", beam.name),
                });
            }
        }
        if beam.meterset <= 0.0 {
            return Err(DicomError::Attribute {
                path: path.to_path_buf(),
                attribute: "beams",
                detail: format!("beam {:?} meterset must be positive", beam.name),
            });
        }
    }
    if options.number_of_fractions <= 0 {
        return Err(DicomError::Attribute {
            path: path.to_path_buf(),
            attribute: "number_of_fractions",
            detail: "must be positive".into(),
        });
    }

    let seed = options.plan_label.as_str();
    let sop_instance_uid = if options.sop_instance_uid.is_empty() {
        format!(
            "2.25.{}",
            Uuid::new_v5(&Uuid::NAMESPACE_OID, seed.as_bytes())
        )
    } else {
        options.sop_instance_uid.clone()
    };
    let series_instance_uid = if options.series_instance_uid.is_empty() {
        format!(
            "2.25.{}",
            Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}:series").as_bytes())
        )
    } else {
        options.series_instance_uid.clone()
    };
    let study_instance_uid = if options.study_instance_uid.is_empty() {
        format!(
            "2.25.{}",
            Uuid::new_v5(&Uuid::NAMESPACE_OID, format!("{seed}:study").as_bytes())
        )
    } else {
        options.study_instance_uid.clone()
    };

    let mut obj = InMemDicomObject::new_empty();
    put_str(&mut obj, tags::SPECIFIC_CHARACTER_SET, VR::CS, "ISO_IR 192");
    put_str(
        &mut obj,
        tags::IMAGE_TYPE,
        VR::CS,
        "DERIVED\\SECONDARY\\RESEARCH",
    );
    put_str(&mut obj, tags::SOP_CLASS_UID, VR::UI, RT_PLAN_SOP_CLASS);
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
        (tags::RT_PLAN_TIME, VR::TM),
    ] {
        put_str(&mut obj, tag, vr, EXPORT_TIME);
    }
    put_str(&mut obj, tags::ACCESSION_NUMBER, VR::SH, "");
    put_str(&mut obj, tags::MODALITY, VR::CS, "RTPLAN");
    put_str(&mut obj, tags::MANUFACTURER, VR::LO, "Avila Labs");
    put_str(&mut obj, tags::REFERRING_PHYSICIAN_NAME, VR::PN, "");
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
    put_str(&mut obj, tags::STUDY_ID, VR::SH, "");
    put_str(&mut obj, tags::SERIES_NUMBER, VR::IS, "1");
    put_str(
        &mut obj,
        tags::FRAME_OF_REFERENCE_UID,
        VR::UI,
        &options.frame_of_reference_uid,
    );
    put_str(&mut obj, tags::POSITION_REFERENCE_INDICATOR, VR::LO, "");
    put_str(&mut obj, tags::RT_PLAN_LABEL, VR::SH, &options.plan_label);
    put_str(&mut obj, tags::RT_PLAN_NAME, VR::LO, &options.plan_name);
    put_str(&mut obj, tags::RT_PLAN_DATE, VR::DA, EXPORT_DATE);
    put_str(&mut obj, tags::PLAN_INTENT, VR::CS, &options.plan_intent);
    put_str(&mut obj, tags::RT_PLAN_GEOMETRY, VR::CS, "PATIENT");

    // Beam sequence: one static beam per spec, single control point 0.
    let mut beam_items = Vec::with_capacity(options.beams.len());
    for (index, beam) in options.beams.iter().enumerate() {
        let mut cp = InMemDicomObject::new_empty();
        put_str(&mut cp, tags::CONTROL_POINT_INDEX, VR::IS, "0");
        put_str(
            &mut cp,
            tags::NOMINAL_BEAM_ENERGY,
            VR::DS,
            &beam
                .nominal_beam_energy_mev
                .map(ds)
                .unwrap_or_else(|| "0".to_owned()),
        );
        put_str(
            &mut cp,
            tags::GANTRY_ANGLE,
            VR::DS,
            &ds(beam.gantry_angle_deg),
        );
        put_str(&mut cp, tags::GANTRY_ROTATION_DIRECTION, VR::CS, "NONE");
        put_str(
            &mut cp,
            tags::BEAM_LIMITING_DEVICE_ANGLE,
            VR::DS,
            &ds(beam.collimator_angle_deg),
        );
        put_str(
            &mut cp,
            tags::PATIENT_SUPPORT_ANGLE,
            VR::DS,
            &ds(beam.patient_support_angle_deg),
        );
        put_str(
            &mut cp,
            tags::ISOCENTER_POSITION,
            VR::DS,
            &ds_values(&beam.isocenter_position_mm),
        );
        put_str(
            &mut cp,
            tags::SOURCE_TO_SURFACE_DISTANCE,
            VR::DS,
            &ds(beam.source_to_surface_distance_mm),
        );
        put_str(&mut cp, tags::CUMULATIVE_METERSET_WEIGHT, VR::DS, "1");

        let mut item = InMemDicomObject::new_empty();
        put_str(&mut item, tags::MANUFACTURER, VR::LO, "Avila Labs");
        put_str(&mut item, tags::MANUFACTURER_MODEL_NAME, VR::LO, "OpenBNCT");
        put_str(
            &mut item,
            tags::BEAM_NUMBER,
            VR::IS,
            &(index + 1).to_string(),
        );
        put_str(&mut item, tags::BEAM_NAME, VR::LO, &beam.name);
        put_str(&mut item, tags::BEAM_TYPE, VR::CS, "STATIC");
        put_str(
            &mut item,
            tags::RADIATION_TYPE,
            VR::CS,
            &beam.radiation_type,
        );
        put_str(
            &mut item,
            tags::TREATMENT_MACHINE_NAME,
            VR::SH,
            &options.treatment_machine_name,
        );
        put_str(&mut item, tags::PRIMARY_DOSIMETER_UNIT, VR::CS, "MU");
        put_str(
            &mut item,
            tags::SOURCE_AXIS_DISTANCE,
            VR::DS,
            &ds(beam.source_axis_distance_mm),
        );
        put_str(
            &mut item,
            tags::TREATMENT_DELIVERY_TYPE,
            VR::CS,
            "TREATMENT",
        );
        put_str(&mut item, tags::NUMBER_OF_WEDGES, VR::IS, "0");
        put_str(&mut item, tags::NUMBER_OF_COMPENSATORS, VR::IS, "0");
        put_str(&mut item, tags::NUMBER_OF_BOLI, VR::IS, "0");
        put_str(&mut item, tags::NUMBER_OF_BLOCKS, VR::IS, "0");
        put_str(&mut item, tags::NUMBER_OF_CONTROL_POINTS, VR::IS, "1");
        put_sequence(&mut item, tags::CONTROL_POINT_SEQUENCE, vec![cp]);
        beam_items.push(item);
    }
    put_sequence(&mut obj, tags::BEAM_SEQUENCE, beam_items);

    // One fraction group referencing every beam at its meterset.
    let mut referenced = Vec::with_capacity(options.beams.len());
    for (index, beam) in options.beams.iter().enumerate() {
        let mut r = InMemDicomObject::new_empty();
        put_str(
            &mut r,
            tags::REFERENCED_BEAM_NUMBER,
            VR::IS,
            &(index + 1).to_string(),
        );
        put_str(
            &mut r,
            tags::SPECIFIED_PRIMARY_METERSET,
            VR::DS,
            &ds(beam.meterset),
        );
        put_str(&mut r, tags::BEAM_METERSET, VR::DS, &ds(beam.meterset));
        referenced.push(r);
    }
    let mut group = InMemDicomObject::new_empty();
    put_str(&mut group, tags::FRACTION_GROUP_NUMBER, VR::IS, "1");
    put_str(
        &mut group,
        tags::NUMBER_OF_FRACTIONS_PLANNED,
        VR::IS,
        &options.number_of_fractions.to_string(),
    );
    put_str(
        &mut group,
        tags::NUMBER_OF_BEAMS,
        VR::IS,
        &options.beams.len().to_string(),
    );
    put_str(
        &mut group,
        tags::NUMBER_OF_BRACHY_APPLICATION_SETUPS,
        VR::IS,
        "0",
    );
    put_sequence(&mut group, tags::REFERENCED_BEAM_SEQUENCE, referenced);
    put_sequence(&mut obj, tags::FRACTION_GROUP_SEQUENCE, vec![group]);

    let file = obj
        .with_meta(
            dicom_object::meta::FileMetaTableBuilder::new()
                .media_storage_sop_class_uid(RT_PLAN_SOP_CLASS)
                .media_storage_sop_instance_uid(&sop_instance_uid)
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .implementation_class_uid(format!(
                    "2.25.{}",
                    Uuid::new_v5(&Uuid::NAMESPACE_OID, b"openbnct-rtplan")
                ))
                .implementation_version_name("OPENBNCT_0_1"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> RtPlanExportOptions {
        RtPlanExportOptions {
            patient_name: "TEST^RTPLAN".into(),
            patient_id: "TEST-001".into(),
            study_instance_uid: String::new(),
            series_instance_uid: String::new(),
            sop_instance_uid: String::new(),
            frame_of_reference_uid: "2.25.999999".into(),
            plan_label: "NF-BNCT-TEST".into(),
            plan_name: "Two-field epithermal".into(),
            plan_intent: "VERIFICATION".into(),
            number_of_fractions: 2,
            treatment_machine_name: "OPENBNCT-EPITHERMAL".into(),
            beams: vec![
                RtPlanBeamSpec {
                    name: "AP-epithermal".into(),
                    gantry_angle_deg: 90.0,
                    collimator_angle_deg: 0.0,
                    patient_support_angle_deg: 0.0,
                    isocenter_position_mm: [0.0, 0.0, 25.0],
                    source_axis_distance_mm: 1800.0,
                    source_to_surface_distance_mm: 1775.0,
                    radiation_type: "NEUTRON".into(),
                    meterset: 120.0,
                    nominal_beam_energy_mev: None,
                },
                RtPlanBeamSpec {
                    name: "PA-epithermal".into(),
                    gantry_angle_deg: 270.0,
                    collimator_angle_deg: 0.0,
                    patient_support_angle_deg: 0.0,
                    isocenter_position_mm: [0.0, 0.0, 25.0],
                    source_axis_distance_mm: 1800.0,
                    source_to_surface_distance_mm: 1775.0,
                    radiation_type: "NEUTRON".into(),
                    meterset: 100.0,
                    nominal_beam_energy_mev: Some(0.02),
                },
            ],
        }
    }

    #[test]
    fn export_then_summarize_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.dcm");
        export_rt_plan(&path, &options()).unwrap();
        let summary = summarize_rt_plan(&path).unwrap();
        assert_eq!(summary.rt_plan_label, "NF-BNCT-TEST");
        assert_eq!(summary.plan_intent.as_deref(), Some("VERIFICATION"));
        assert_eq!(summary.patient_id.as_deref(), Some("TEST-001"));
        assert_eq!(
            summary.frame_of_reference_uid.as_deref(),
            Some("2.25.999999")
        );
        assert_eq!(summary.beams.len(), 2);
        assert_eq!(summary.fraction_groups.len(), 1);
        assert_eq!(
            summary.fraction_groups[0].number_of_fractions_planned,
            Some(2)
        );
        let ap = &summary.beams[0];
        assert_eq!(ap.beam_number, 1);
        assert_eq!(ap.beam_name.as_deref(), Some("AP-epithermal"));
        assert_eq!(ap.radiation_type.as_deref(), Some("NEUTRON"));
        assert_eq!(ap.metersets, vec![120.0]);
        let cp = ap.control_point.as_ref().unwrap();
        assert_eq!(cp.gantry_angle_deg, Some(90.0));
        assert_eq!(cp.isocenter_position_mm, Some([0.0, 0.0, 25.0]));
        assert_eq!(cp.source_to_surface_distance_mm, Some(1775.0));
        let pa = &summary.beams[1];
        let pa_cp = pa.control_point.as_ref().unwrap();
        assert_eq!(pa_cp.gantry_angle_deg, Some(270.0));
        assert_eq!(pa_cp.nominal_beam_energy_mev, Some(0.02));
    }

    #[test]
    fn export_rejects_empty_beams_and_bad_values() {
        let dir = tempfile::tempdir().unwrap();
        let mut no_beams = options();
        no_beams.beams.clear();
        assert!(export_rt_plan(&dir.path().join("a.dcm"), &no_beams).is_err());

        let mut bad = options();
        bad.beams[0].meterset = 0.0;
        assert!(export_rt_plan(&dir.path().join("b.dcm"), &bad).is_err());

        let mut nan = options();
        nan.beams[0].gantry_angle_deg = f64::NAN;
        assert!(export_rt_plan(&dir.path().join("c.dcm"), &nan).is_err());
    }

    #[test]
    fn summarize_rejects_non_rtplan() {
        // A non-RTPLAN object is rejected on the SOP class check rather
        // than silently summarized.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("empty.dcm");
        let mut obj = InMemDicomObject::new_empty();
        put_str(
            &mut obj,
            tags::SOP_CLASS_UID,
            VR::UI,
            uids::CT_IMAGE_STORAGE,
        );
        let file = obj
            .with_meta(
                dicom_object::meta::FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("1.2.3.4"),
            )
            .unwrap();
        file.write_to_file(&dir).unwrap();
        assert!(summarize_rt_plan(&dir).is_err());
    }
}
