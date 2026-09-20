// SPDX-License-Identifier: Apache-2.0

//! Independent oracle for the frozen `NF-BNCT-001` DICOM geometry case.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use openbnct_evidence::{
    CaseManifest, PatientCoordinateSystem, QualificationBoundary, StructureRecord,
};

use crate::{CtVolume, DicomError, Result, import_ct_series, import_rtstruct};

const EXPECTED_FRAME_UID: &str = "2.25.240883953911088373736134884257182446642";
const EXPECTED_STUDY_UID: &str = "2.25.149214599444245138262873740736845471752";
const EXPECTED_SERIES_UID: &str = "2.25.337319594251465962942344971245692083782";
const EXPECTED_RTSTRUCT_SERIES_UID: &str = "2.25.50705181539640583496141175374452175263";
const EXPECTED_RTSTRUCT_INSTANCE_UID: &str = "2.25.277528316852233615277963392913905893031";
const EXPECTED_FIRST_SLICE_UID: &str = "2.25.43546999367060429143037900891741988095";
const EXPECTED_LAST_SLICE_UID: &str = "2.25.224181827055039319855832006853907618875";

#[derive(Debug, Clone, PartialEq)]
pub struct RoiReport {
    pub number: i32,
    pub name: String,
    pub voxel_count: usize,
    pub volume_cm3: f64,
    pub centroid_lps_mm: [f64; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct BenchmarkReport {
    pub case_id: &'static str,
    pub shape: [u32; 3],
    pub spacing_mm: [f64; 3],
    pub origin_mm: [f64; 3],
    pub ct_slice_count: usize,
    pub verified_artifact_count: usize,
    pub rois: Vec<RoiReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedBenchmarkCase {
    pub report: BenchmarkReport,
    pub ct: CtVolume,
    pub structures: crate::StructureSet,
}

struct ExpectedRoi {
    number: i32,
    name: &'static str,
    voxel_count: usize,
    volume_cm3: f64,
    centroid_lps_mm: [f64; 3],
}

const EXPECTED_ROIS: [ExpectedRoi; 5] = [
    ExpectedRoi {
        number: 1,
        name: "PHANTOM",
        voxel_count: 64_000,
        volume_cm3: 8_000.0,
        centroid_lps_mm: [0.0, 0.0, 0.0],
    },
    ExpectedRoi {
        number: 2,
        name: "CORE",
        voxel_count: 512,
        volume_cm3: 64.0,
        centroid_lps_mm: [0.0, 0.0, 0.0],
    },
    ExpectedRoi {
        number: 3,
        name: "LEFT_ANTERIOR_MARKER",
        voxel_count: 64,
        volume_cm3: 8.0,
        centroid_lps_mm: [70.0, -70.0, -70.0],
    },
    ExpectedRoi {
        number: 4,
        name: "RIGHT_POSTERIOR_MARKER",
        voxel_count: 64,
        volume_cm3: 8.0,
        centroid_lps_mm: [-70.0, 70.0, 70.0],
    },
    ExpectedRoi {
        number: 5,
        name: "CENTRAL_AXIS_2CM",
        voxel_count: 8,
        volume_cm3: 1.0,
        centroid_lps_mm: [0.0, 0.0, -80.0],
    },
];

/// Verify generated `NF-BNCT-001` files against an independent frozen oracle.
pub fn verify_nf_bnct_001(root: &Path) -> Result<BenchmarkReport> {
    Ok(load_nf_bnct_001(root)?.report)
}

/// Load `NF-BNCT-001` only after all geometry and artifact gates pass.
pub fn load_nf_bnct_001(root: &Path) -> Result<VerifiedBenchmarkCase> {
    let ct_files = dicom_files(&root.join("ct"))?;
    if ct_files.len() != 40 {
        return mismatch(format!(
            "CT directory contains {} DICOM files; expected 40",
            ct_files.len()
        ));
    }
    let ct = import_ct_series(&ct_files)?;
    check_ct_geometry(&ct)?;

    let rtstruct_path = root.join("rtstruct.dcm");
    let structures = import_rtstruct(&rtstruct_path, &ct)?;
    let reports = check_rois(&structures, &ct)?;

    let verified_artifact_count = verify_manifest(root, &ct, &reports)?;
    Ok(verified_case(
        ct,
        structures,
        reports,
        verified_artifact_count,
    ))
}

/// In-memory variant of [`load_nf_bnct_001`] for hosts without a
/// filesystem (web drops, archives). `files` maps manifest-relative
/// names — `ct/ct-000.dcm`, `rtstruct.dcm`, `case.json` — to complete
/// bytes; every geometry, ROI, and digest gate is identical.
pub fn load_nf_bnct_001_from_files(files: &[(String, Vec<u8>)]) -> Result<VerifiedBenchmarkCase> {
    let map: BTreeMap<String, &[u8]> = files
        .iter()
        .map(|(name, bytes)| (name.clone(), bytes.as_slice()))
        .collect();
    let ct_files: Vec<(String, Vec<u8>)> = map
        .iter()
        .filter(|(name, _)| name.starts_with("ct/") && name.ends_with(".dcm"))
        .map(|(name, bytes)| (name.clone(), bytes.to_vec()))
        .collect();
    if ct_files.len() != 40 {
        return mismatch(format!(
            "file set contains {} CT slices under ct/; expected 40",
            ct_files.len()
        ));
    }
    let ct = crate::import_ct_series_from_bytes(&ct_files)?;
    check_ct_geometry(&ct)?;

    let rtstruct_bytes = map
        .get("rtstruct.dcm")
        .ok_or_else(|| DicomError::Benchmark("file set is missing rtstruct.dcm".into()))?;
    let structures = crate::import_rtstruct_bytes(rtstruct_bytes, &ct)?;
    let reports = check_rois(&structures, &ct)?;

    let verified_artifact_count = verify_manifest_map(&map, &ct, &reports)?;
    Ok(verified_case(
        ct,
        structures,
        reports,
        verified_artifact_count,
    ))
}

fn check_ct_geometry(ct: &CtVolume) -> Result<()> {
    expect_equal("shape", &ct.geometry.shape, &[40, 40, 40])?;
    expect_equal("spacing", &ct.geometry.spacing_mm, &[5.0, 5.0, 5.0])?;
    expect_equal("origin", &ct.geometry.origin_mm, &[-97.5, -97.5, -97.5])?;
    expect_equal(
        "direction",
        &ct.geometry.direction,
        &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    )?;
    expect_equal(
        "Frame of Reference UID",
        &ct.frame_of_reference_uid,
        &EXPECTED_FRAME_UID.to_owned(),
    )?;
    expect_equal(
        "Study Instance UID",
        &ct.study_instance_uid,
        &EXPECTED_STUDY_UID.to_owned(),
    )?;
    expect_equal(
        "CT Series Instance UID",
        &ct.series_instance_uid,
        &EXPECTED_SERIES_UID.to_owned(),
    )?;
    expect_equal(
        "first CT SOP Instance UID",
        &ct.slice_sop_instance_uids[0],
        &EXPECTED_FIRST_SLICE_UID.to_owned(),
    )?;
    expect_equal(
        "last CT SOP Instance UID",
        &ct.slice_sop_instance_uids[39],
        &EXPECTED_LAST_SLICE_UID.to_owned(),
    )?;
    if ct.rescale_slope != 1.0
        || ct.rescale_intercept != 0.0
        || ct.stored_pixels.iter().any(|pixel| *pixel != 0)
    {
        return mismatch("CT pixel or rescale values differ from the frozen all-zero HU volume");
    }
    Ok(())
}

fn check_rois(structures: &crate::StructureSet, ct: &CtVolume) -> Result<Vec<RoiReport>> {
    if structures.rois.len() != EXPECTED_ROIS.len() {
        return mismatch(format!(
            "RT Structure Set contains {} ROIs; expected {}",
            structures.rois.len(),
            EXPECTED_ROIS.len()
        ));
    }

    let mut reports = Vec::with_capacity(EXPECTED_ROIS.len());
    for expected in EXPECTED_ROIS {
        let roi = structures
            .roi(expected.name)
            .ok_or_else(|| DicomError::Benchmark(format!("missing ROI {:?}", expected.name)))?;
        let voxel_count = roi.voxel_count();
        let volume_cm3 = roi.volume_cm3(ct);
        let centroid_lps_mm = roi
            .centroid_lps_mm(ct)
            .ok_or_else(|| DicomError::Benchmark(format!("ROI {:?} is empty", expected.name)))?;
        if roi.number != expected.number
            || voxel_count != expected.voxel_count
            || volume_cm3 != expected.volume_cm3
            || centroid_lps_mm != expected.centroid_lps_mm
        {
            return mismatch(format!(
                "ROI {:?} differs: number={}, voxels={}, volume_cm3={}, centroid={centroid_lps_mm:?}",
                expected.name, roi.number, voxel_count, volume_cm3
            ));
        }
        reports.push(RoiReport {
            number: roi.number,
            name: roi.name.clone(),
            voxel_count,
            volume_cm3,
            centroid_lps_mm,
        });
    }
    Ok(reports)
}

fn verified_case(
    ct: CtVolume,
    structures: crate::StructureSet,
    reports: Vec<RoiReport>,
    verified_artifact_count: usize,
) -> VerifiedBenchmarkCase {
    VerifiedBenchmarkCase {
        report: BenchmarkReport {
            case_id: "NF-BNCT-001",
            shape: ct.geometry.shape,
            spacing_mm: ct.geometry.spacing_mm,
            origin_mm: ct.geometry.origin_mm,
            ct_slice_count: ct.slice_sop_instance_uids.len(),
            verified_artifact_count,
            rois: reports,
        },
        ct,
        structures,
    }
}

fn verify_manifest(root: &Path, ct: &CtVolume, rois: &[RoiReport]) -> Result<usize> {
    let path = root.join("case.json");
    let bytes = fs::read(&path).map_err(|source| DicomError::Io {
        path: path.clone(),
        source,
    })?;
    let manifest: CaseManifest = serde_json::from_slice(&bytes)
        .map_err(|error| DicomError::Benchmark(format!("invalid case.json: {error}")))?;
    check_manifest_fields(&manifest, ct, rois)?;
    manifest.verify_artifacts(root)?;
    Ok(manifest.artifacts.len())
}

/// Bytes-based manifest verify — identical field gates, digests checked
/// against the supplied file map instead of the filesystem.
fn verify_manifest_map(
    files: &BTreeMap<String, &[u8]>,
    ct: &CtVolume,
    rois: &[RoiReport],
) -> Result<usize> {
    let bytes = files
        .get("case.json")
        .ok_or_else(|| DicomError::Benchmark("file set is missing case.json".into()))?;
    let manifest: CaseManifest = serde_json::from_slice(bytes)
        .map_err(|error| DicomError::Benchmark(format!("invalid case.json: {error}")))?;
    check_manifest_fields(&manifest, ct, rois)?;
    manifest.verify_artifacts_map(files)?;
    Ok(manifest.artifacts.len())
}

fn check_manifest_fields(manifest: &CaseManifest, ct: &CtVolume, rois: &[RoiReport]) -> Result<()> {
    manifest.validate()?;

    expect_equal(
        "manifest schema version",
        &manifest.schema_version,
        &"openbnct.case-manifest/0.1.0".to_owned(),
    )?;
    expect_equal(
        "manifest case ID",
        &manifest.case_id,
        &"NF-BNCT-001".to_owned(),
    )?;
    expect_equal(
        "manifest qualification",
        &manifest.qualification,
        &QualificationBoundary::SyntheticResearchOnly,
    )?;
    expect_equal(
        "manifest coordinate system",
        &manifest.coordinate_system,
        &PatientCoordinateSystem::DicomLpsMillimeters,
    )?;
    expect_equal("manifest geometry", &manifest.geometry, &ct.geometry)?;
    expect_equal(
        "manifest Frame of Reference UID",
        &manifest.frame_of_reference_uid,
        &EXPECTED_FRAME_UID.to_owned(),
    )?;
    expect_equal(
        "manifest Study Instance UID",
        &manifest.study_instance_uid,
        &EXPECTED_STUDY_UID.to_owned(),
    )?;
    expect_equal(
        "manifest CT Series Instance UID",
        &manifest.imaging_series_instance_uid,
        &EXPECTED_SERIES_UID.to_owned(),
    )?;
    expect_equal(
        "manifest RT Structure Set Series Instance UID",
        &manifest.structure_set_series_instance_uid,
        &EXPECTED_RTSTRUCT_SERIES_UID.to_owned(),
    )?;
    expect_equal(
        "manifest RT Structure Set SOP Instance UID",
        &manifest.structure_set_instance_uid,
        &EXPECTED_RTSTRUCT_INSTANCE_UID.to_owned(),
    )?;
    expect_equal(
        "manifest material model ID",
        &manifest.material_model_id,
        &"openbnct.nf-bnct-001.material.v1".to_owned(),
    )?;
    expect_equal(
        "manifest source model ID",
        &manifest.source_model_id,
        &"openbnct.nf-bnct-001.source.v1".to_owned(),
    )?;

    let expected_structures: Vec<_> = rois
        .iter()
        .map(|roi| StructureRecord {
            number: roi.number,
            name: roi.name.clone(),
            voxel_count: roi.voxel_count,
            volume_cm3: roi.volume_cm3,
            centroid_lps_mm: roi.centroid_lps_mm,
        })
        .collect();
    expect_equal(
        "manifest structures",
        &manifest.structures,
        &expected_structures,
    )?;

    let artifacts: BTreeMap<_, _> = manifest
        .artifacts
        .iter()
        .map(|artifact| (artifact.path.as_str(), artifact))
        .collect();
    if artifacts.len() != 41 || artifacts.len() != manifest.artifacts.len() {
        return mismatch(format!(
            "manifest contains {} unique artifacts; expected 41",
            artifacts.len()
        ));
    }
    for slice_index in 0..40 {
        let path = format!("ct/ct-{slice_index:03}.dcm");
        let artifact = artifacts
            .get(path.as_str())
            .ok_or_else(|| DicomError::Benchmark(format!("manifest is missing {path}")))?;
        if artifact.role != "dicom_ct_slice"
            || artifact.media_type.as_deref() != Some("application/dicom")
        {
            return mismatch(format!("manifest metadata is invalid for {path}"));
        }
    }
    let rtstruct = artifacts
        .get("rtstruct.dcm")
        .ok_or_else(|| DicomError::Benchmark("manifest is missing rtstruct.dcm".into()))?;
    if rtstruct.role != "dicom_rt_structure_set"
        || rtstruct.media_type.as_deref() != Some("application/dicom")
    {
        return mismatch("manifest metadata is invalid for rtstruct.dcm");
    }
    Ok(())
}

fn dicom_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(directory).map_err(|source| DicomError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| DicomError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let is_file = entry
            .file_type()
            .map_err(|source| DicomError::Io {
                path: path.clone(),
                source,
            })?
            .is_file();
        let is_dicom = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dcm"));
        if is_file && is_dicom {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn expect_equal<T: PartialEq + std::fmt::Debug>(
    label: &str,
    observed: &T,
    expected: &T,
) -> Result<()> {
    if observed != expected {
        return mismatch(format!(
            "{label} differs: observed {observed:?}, expected {expected:?}"
        ));
    }
    Ok(())
}

fn mismatch<T>(message: impl Into<String>) -> Result<T> {
    Err(DicomError::Benchmark(message.into()))
}
