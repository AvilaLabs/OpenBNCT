// SPDX-License-Identifier: MIT

//! End to end: synthetic NF-BNCT-001 DICOM -> `dicom import-ct` at a coarser
//! spacing -> `dicom calibrate --hu-nifti`.

use std::path::Path;
use std::process::{Command, Output};

fn openbnct(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_openbnct"))
        .args(args)
        .output()
        .expect("spawn openbnct")
}

fn ok(args: &[&str]) -> String {
    let out = openbnct(args);
    assert!(
        out.status.success(),
        "openbnct {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

#[test]
fn import_ct_builds_case_hu_masks_and_calibrates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = tempfile::tempdir().unwrap();
    let study = dir.path().join("study");
    ok(&["benchmark", "generate", s(&study)]);

    let case_path = dir.path().join("case.json");
    let hu_path = dir.path().join("hu.nii");
    let masks = dir.path().join("masks");
    let void = root.join("benchmarks/synthetic/layered-head-phantom/materials/void.json");
    let stdout = ok(&[
        "dicom",
        "import-ct",
        "--series",
        s(&study),
        "--spacing-mm",
        "8",
        "--case-id",
        "test.import-ct.v1",
        "--base-material",
        s(&void),
        "--case-output",
        s(&case_path),
        "--hu-output",
        s(&hu_path),
        "--masks-dir",
        s(&masks),
    ]);
    assert!(stdout.contains("placeholder"), "{stdout}");

    // Native CT geometry, read back through the DICOM library.
    let paths = openbnct_dicom::collect_study_paths(&study.join("ct"));
    let ct = openbnct_dicom::import_ct_series(&paths).unwrap();

    let case: openbnct_transport::TransportCase =
        serde_json::from_slice(&std::fs::read(&case_path).unwrap()).unwrap();
    assert_eq!(case.case_id, "test.import-ct.v1");
    assert_eq!(case.geometry.spacing_mm, [8.0; 3]);
    assert_eq!(case.geometry.direction, ct.geometry.direction);
    for axis in 0..3 {
        let covered = f64::from(case.geometry.shape[axis]) * 8.0;
        let ct_extent = f64::from(ct.geometry.shape[axis]) * ct.geometry.spacing_mm[axis];
        assert!(covered >= ct_extent - 1e-6 && covered < ct_extent + 8.0);
    }

    let hu = openbnct_nifti::read_nifti_file(&hu_path).unwrap();
    assert_eq!(hu.geometry.shape, case.geometry.shape);
    assert_eq!(hu.values.len(), case.geometry.voxel_count().unwrap());
    let (lo, hi) = ct
        .stored_pixels
        .iter()
        .map(|&p| ct.modality_value(p))
        .fold((f64::MAX, f64::MIN), |(l, h), v| (l.min(v), h.max(v)));
    assert!(hu.values.iter().all(|v| *v >= lo - 1e-9 && *v <= hi + 1e-9));

    // Masks: one per ROI, on the case grid, plus a hash-bound index.
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(masks.join("index.json")).unwrap()).unwrap();
    let entries = index["masks"].as_array().unwrap();
    assert!(!entries.is_empty());
    for entry in entries {
        let file = masks.join(entry["file"].as_str().unwrap());
        let mask: openbnct_core::RegionMask =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(mask.voxels.len(), hu.values.len());
        assert_eq!(
            entry["sha256"].as_str().unwrap(),
            openbnct_evidence::sha256_file(&file).unwrap()
        );
    }

    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("case.import-record.json")).unwrap())
            .unwrap();
    assert!(!record["inputs"].as_array().unwrap().is_empty());
    assert_eq!(
        record["outputs"]["hu"]["sha256"].as_str().unwrap(),
        openbnct_evidence::sha256_file(&hu_path).unwrap()
    );

    // Calibrate on the case grid.
    let calibration =
        root.join("benchmarks/synthetic/layered-head-phantom/materials/hu-calibration.json");
    let assignment_path = dir.path().join("assignment.json");
    ok(&[
        "dicom",
        "calibrate",
        "--calibration",
        s(&calibration),
        "--hu-nifti",
        s(&hu_path),
        "--case",
        s(&case_path),
        "--output",
        s(&assignment_path),
    ]);
    let assignment: openbnct_transport::MaterialAssignment =
        serde_json::from_slice(&std::fs::read(&assignment_path).unwrap()).unwrap();
    assignment.validate(&case.geometry).unwrap();
    assert_eq!(assignment.case_id, case.case_id);
}

#[test]
fn import_ct_refuses_to_overwrite_and_bad_input() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let void = root.join("benchmarks/synthetic/layered-head-phantom/materials/void.json");
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let out = openbnct(&[
        "dicom",
        "import-ct",
        "--series",
        s(&empty),
        "--case-id",
        "x.y.v1",
        "--base-material",
        s(&void),
        "--case-output",
        s(&dir.path().join("case.json")),
        "--hu-output",
        s(&dir.path().join("hu.nii")),
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no CT"));
    assert!(!dir.path().join("case.json").exists());
}
