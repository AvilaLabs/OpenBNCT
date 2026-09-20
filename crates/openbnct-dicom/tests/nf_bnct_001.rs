// SPDX-License-Identifier: Apache-2.0

use std::fs;
use std::path::Path;

use dicom_core::value::PrimitiveValue;
use dicom_core::{DataElement, VR};
use dicom_dictionary_std::tags;
use dicom_object::open_file;
use openbnct_dicom::synthetic::{
    COLUMNS, FIRST_CENTER_MM, FRAME_OF_REFERENCE_UID, ROWS, SLICES, SPACING_MM, ct_slice_uid,
    generate_nf_bnct_001, validate_part10_files,
};
use openbnct_dicom::{
    DicomError, import_ct_series, import_rtstruct, import_study_from_files,
    load_nf_bnct_001_from_files, verify_nf_bnct_001,
};
use tempfile::tempdir;

#[test]
fn generated_case_round_trips_with_exact_geometry_and_masks() {
    let temp = tempdir().expect("temporary directory");
    let output = temp.path().join("nf-bnct-001");
    let generated = generate_nf_bnct_001(&output).expect("generate benchmark");
    validate_part10_files(&generated).expect("all generated files are readable Part 10 files");

    let mut deliberately_reversed = generated.ct_files.clone();
    deliberately_reversed.reverse();
    let ct = import_ct_series(&deliberately_reversed).expect("import CT in arbitrary file order");
    assert_eq!(
        ct.geometry.shape,
        [COLUMNS as u32, ROWS as u32, SLICES as u32]
    );
    assert_eq!(ct.geometry.spacing_mm, [SPACING_MM; 3]);
    assert_eq!(ct.geometry.origin_mm, [FIRST_CENTER_MM; 3]);
    assert_eq!(
        ct.geometry.direction,
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(ct.frame_of_reference_uid, FRAME_OF_REFERENCE_UID);
    assert_eq!(ct.slice_sop_instance_uids[0], ct_slice_uid(0));
    assert_eq!(
        ct.slice_sop_instance_uids[SLICES - 1],
        ct_slice_uid(SLICES - 1)
    );
    assert_eq!(
        ct.slice_sop_instance_uids[0],
        "2.25.43546999367060429143037900891741988095"
    );
    assert_eq!(
        ct.slice_sop_instance_uids[SLICES - 1],
        "2.25.224181827055039319855832006853907618875"
    );
    assert_eq!(ct.stored_pixels.len(), COLUMNS * ROWS * SLICES);
    assert!(ct.stored_pixels.iter().all(|pixel| *pixel == 0));
    assert_eq!(ct.rescale_slope, 1.0);
    assert_eq!(ct.rescale_intercept, 0.0);

    let structures = import_rtstruct(&generated.rtstruct_file, &ct).expect("import RTSTRUCT");
    let expected = [
        ("PHANTOM", 64_000, 8_000.0, [0.0, 0.0, 0.0]),
        ("CORE", 512, 64.0, [0.0, 0.0, 0.0]),
        ("LEFT_ANTERIOR_MARKER", 64, 8.0, [70.0, -70.0, -70.0]),
        ("RIGHT_POSTERIOR_MARKER", 64, 8.0, [-70.0, 70.0, 70.0]),
        ("CENTRAL_AXIS_2CM", 8, 1.0, [0.0, 0.0, -80.0]),
    ];
    assert_eq!(structures.rois.len(), expected.len());
    for (name, voxel_count, volume_cm3, centroid) in expected {
        let roi = structures
            .roi(name)
            .unwrap_or_else(|| panic!("missing ROI {name}"));
        assert_eq!(roi.voxel_count(), voxel_count, "ROI {name}");
        assert_eq!(roi.volume_cm3(&ct), volume_cm3, "ROI {name}");
        assert_eq!(roi.centroid_lps_mm(&ct), Some(centroid), "ROI {name}");
    }

    let report = verify_nf_bnct_001(&output).expect("independent frozen oracle");
    assert_eq!(report.case_id, "NF-BNCT-001");
    assert_eq!(report.shape, [40, 40, 40]);
    assert_eq!(report.verified_artifact_count, 41);
    assert_eq!(report.rois.len(), 5);
}

#[test]
fn generated_case_verifies_from_in_memory_files() {
    let temp = tempdir().expect("temporary directory");
    let output = temp.path().join("nf-bnct-001");
    let generated = generate_nf_bnct_001(&output).expect("generate benchmark");

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in &generated.ct_files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("slice file name");
        files.push((
            format!("ct/{name}"),
            std::fs::read(path).expect("read slice"),
        ));
    }
    files.push((
        "rtstruct.dcm".to_owned(),
        std::fs::read(&generated.rtstruct_file).expect("read rtstruct"),
    ));
    files.push((
        "case.json".to_owned(),
        std::fs::read(&generated.manifest_file).expect("read manifest"),
    ));

    let verified = load_nf_bnct_001_from_files(&files).expect("in-memory verify");
    assert_eq!(
        verified.ct.geometry.shape,
        [COLUMNS as u32, ROWS as u32, SLICES as u32]
    );
    assert_eq!(verified.structures.rois.len(), 5);

    // A single corrupted byte must fail the manifest digest.
    let mut corrupted = files.clone();
    corrupted[0].1[512] ^= 0xFF;
    assert!(load_nf_bnct_001_from_files(&corrupted).is_err());

    // Missing members are named, not guessed at.
    let incomplete = files[..files.len() - 1].to_vec();
    assert!(load_nf_bnct_001_from_files(&incomplete).is_err());
}

#[test]
fn generated_study_imports_through_the_research_path() {
    let temp = tempdir().expect("temporary directory");
    let output = temp.path().join("nf-bnct-001");
    let generated = generate_nf_bnct_001(&output).expect("generate benchmark");

    // A research drop has no manifest — the importer must bucket members
    // by SOP class and bind its own content.
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for path in &generated.ct_files {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .expect("slice file name");
        files.push((name.to_owned(), std::fs::read(path).expect("read slice")));
    }
    files.push((
        "rtstruct.dcm".to_owned(),
        std::fs::read(&generated.rtstruct_file).expect("read rtstruct"),
    ));
    // A stray non-DICOM member is ignored, not fatal.
    files.push(("notes.txt".to_owned(), b"operator notes".to_vec()));

    let study = import_study_from_files(&files).expect("study import");
    assert_eq!(
        study.ct.geometry.shape,
        [COLUMNS as u32, ROWS as u32, SLICES as u32]
    );
    assert_eq!(study.structures.rois.len(), 5);
    assert_eq!(study.rois.len(), 5);
    assert!(study.pet.is_none());
    assert!(study.case_id.starts_with("study-"));
    assert_eq!(study.ignored, vec!["notes.txt".to_owned()]);
    assert_eq!(study.member_count, files.len() - 1);

    // Without the RTSTRUCT there is no case — refuse, not guess.
    let ct_only: Vec<_> = files
        .iter()
        .filter(|(name, _)| name.starts_with("ct-"))
        .cloned()
        .collect();
    assert!(import_study_from_files(&ct_only).is_err());
}

#[test]
fn generated_dicom_is_byte_deterministic() {
    let temp = tempdir().expect("temporary directory");
    let first = generate_nf_bnct_001(&temp.path().join("first")).expect("first generation");
    let second = generate_nf_bnct_001(&temp.path().join("second")).expect("second generation");
    for slice in 0..SLICES {
        assert_eq!(
            fs::read(&first.ct_files[slice]).expect("read first CT"),
            fs::read(&second.ct_files[slice]).expect("read second CT"),
            "CT slice {slice} differs"
        );
    }
    assert_eq!(
        fs::read(first.rtstruct_file).expect("read first RTSTRUCT"),
        fs::read(second.rtstruct_file).expect("read second RTSTRUCT")
    );
    assert_eq!(
        fs::read(first.manifest_file).expect("read first manifest"),
        fs::read(second.manifest_file).expect("read second manifest")
    );
}

#[test]
fn generated_instances_retain_validator_required_metadata() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");

    let ct = open_file(&generated.ct_files[0]).expect("open CT");
    assert_eq!(
        ct.element(tags::IMAGE_LATERALITY)
            .expect("CT Image Laterality")
            .to_str()
            .expect("CT Image Laterality string"),
        "U"
    );

    let rtstruct = open_file(&generated.rtstruct_file).expect("open RTSTRUCT");
    assert!(rtstruct.element(tags::OPERATORS_NAME).is_ok());
    assert_eq!(
        rtstruct
            .element(tags::FRAME_OF_REFERENCE_UID)
            .expect("RTSTRUCT Frame of Reference UID")
            .to_str()
            .expect("RTSTRUCT Frame of Reference UID string"),
        FRAME_OF_REFERENCE_UID
    );
    assert!(rtstruct.element(tags::POSITION_REFERENCE_INDICATOR).is_ok());
    assert!(rtstruct.element(tags::CONTENT_DATE).is_err());
    assert!(rtstruct.element(tags::CONTENT_TIME).is_err());
}

#[test]
fn manifest_detects_semantically_accepted_file_tampering() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");
    rewrite_string(
        &generated.ct_files[5],
        tags::STUDY_DESCRIPTION,
        VR::LO,
        "TAMPERED",
    );
    let error = verify_nf_bnct_001(&generated.root).expect_err("hash mismatch must fail");
    assert!(matches!(
        error,
        DicomError::Manifest(openbnct_evidence::ManifestError::HashMismatch { .. })
    ));
}

#[test]
fn generator_never_overwrites_an_existing_destination() {
    let temp = tempdir().expect("temporary directory");
    let output = temp.path().join("case");
    generate_nf_bnct_001(&output).expect("first generation");
    assert!(matches!(
        generate_nf_bnct_001(&output),
        Err(DicomError::OutputExists(path)) if path == output
    ));
}

#[test]
fn rejects_duplicate_slice_positions() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");
    rewrite_string(
        &generated.ct_files[1],
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        "-97.5\\-97.5\\-97.5",
    );
    let error = import_ct_series(&generated.ct_files).expect_err("duplicate plane must fail");
    assert!(matches!(error, DicomError::Geometry(message) if message.contains("duplicate")));
}

#[test]
fn rejects_non_orthonormal_orientation() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");
    rewrite_string(
        &generated.ct_files[1],
        tags::IMAGE_ORIENTATION_PATIENT,
        VR::DS,
        "1\\0\\0\\1\\0\\0",
    );
    let error = import_ct_series(&generated.ct_files).expect_err("bad orientation must fail");
    assert!(matches!(error, DicomError::Geometry(message) if message.contains("non-orthonormal")));
}

#[test]
fn rejects_nonuniform_projected_spacing() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");
    rewrite_string(
        &generated.ct_files[20],
        tags::IMAGE_POSITION_PATIENT,
        VR::DS,
        "-97.5\\-97.5\\3.5",
    );
    let error = import_ct_series(&generated.ct_files).expect_err("irregular spacing must fail");
    assert!(matches!(error, DicomError::Geometry(message) if message.contains("non-uniform")));
}

#[test]
fn rejects_inconsistent_ct_frame_of_reference() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");
    rewrite_string(
        &generated.ct_files[10],
        tags::FRAME_OF_REFERENCE_UID,
        VR::UI,
        "2.25.1",
    );
    let error = import_ct_series(&generated.ct_files).expect_err("mixed frame must fail");
    assert!(
        matches!(error, DicomError::InconsistentSeries(message) if message.contains("Frame of Reference"))
    );
}

#[test]
fn rejects_rtstruct_frame_mismatch() {
    let temp = tempdir().expect("temporary directory");
    let generated = generate_nf_bnct_001(&temp.path().join("case")).expect("generate");
    let mut ct = import_ct_series(&generated.ct_files).expect("import CT");
    ct.frame_of_reference_uid = "2.25.1".into();
    let error =
        import_rtstruct(&generated.rtstruct_file, &ct).expect_err("frame mismatch must fail");
    assert!(
        matches!(error, DicomError::StructureSet(message) if message.contains("does not match CT"))
    );
}

fn rewrite_string(path: &Path, tag: dicom_core::Tag, vr: VR, value: &str) {
    let mut object = open_file(path).expect("open DICOM for mutation");
    object.put(DataElement::new(tag, vr, PrimitiveValue::from(value)));
    object.write_to_file(path).expect("rewrite mutated DICOM");
}
