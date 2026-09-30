// SPDX-License-Identifier: MIT

//! R18-10 Tier-1 input formats through the real binary: `import volume`,
//! NRRD through `nifti info`, `dicom import-ct` with a DICOM SEG, and
//! `import rtdose`.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use openbnct_core::{
    ComponentProfileReference, ContentReference, DoseComponent, DoseUnit, DoseVolume, GridGeometry,
    PhysicalDoseBundle, PhysicalTotalDoseVolume, TotalUncertaintyMethod,
};
use openbnct_dicom::synthetic::{
    CT_SERIES_INSTANCE_UID, FRAME_OF_REFERENCE_UID, STUDY_INSTANCE_UID, SyntheticSegSpec,
    SyntheticSegment, write_seg,
};
use openbnct_dicom::{DoseSelection, RtDoseExportOptions, export_rt_dose};

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
fn import_volume_converts_nrrd_and_other_commands_read_it() {
    let dir = tempfile::tempdir().unwrap();
    // 3x2x2 float NRRD, RAS space with negated x/y, gzip-free raw.
    let values: Vec<f32> = (0..12).map(|i| i as f32 * 2.0).collect();
    let mut bytes = b"NRRD0004\ntype: float\ndimension: 3\nspace: right-anterior-superior\n\
        sizes: 3 2 2\nspace directions: (-2,0,0) (0,-1.5,0) (0,0,3)\nencoding: raw\n\
        endian: little\nspace origin: (10,-20,30.5)\n\n"
        .to_vec();
    for v in &values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let nrrd = dir.path().join("vol.nrrd");
    fs::write(&nrrd, bytes).unwrap();
    let out = dir.path().join("vol.nii");
    let stdout = ok(&["import", "volume", "--input", s(&nrrd), "--output", s(&out)]);
    assert!(stdout.contains("as nrrd"), "{stdout}");
    let image = openbnct_nifti::read_nifti_file(&out).unwrap();
    assert_eq!(image.geometry.shape, [3, 2, 2]);
    assert_eq!(image.geometry.spacing_mm, [2.0, 1.5, 3.0]);
    assert_eq!(image.geometry.origin_mm, [-10.0, 20.0, 30.5]);
    assert_eq!(
        image.values,
        values.iter().map(|v| f64::from(*v)).collect::<Vec<_>>()
    );
    // Refuses to overwrite.
    let again = openbnct(&["import", "volume", "--input", s(&nrrd), "--output", s(&out)]);
    assert!(!again.status.success());
    // Existing NIfTI-reading commands accept the NRRD directly.
    let info = ok(&["nifti", "info", "--input", s(&nrrd)]);
    assert!(info.contains("3"), "{info}");
    // Oblique input is refused with a clear message.
    let oblique = dir.path().join("oblique.nrrd");
    let mut bytes = b"NRRD0004\ntype: float\ndimension: 3\nspace: LPS\nsizes: 3 2 2\n\
        space directions: (1,0.4,0) (0,1,0) (0,0,1)\nencoding: raw\nendian: little\n\n"
        .to_vec();
    bytes.extend_from_slice(&[0_u8; 48]);
    fs::write(&oblique, bytes).unwrap();
    let bad = openbnct(&[
        "import",
        "volume",
        "--input",
        s(&oblique),
        "--output",
        s(&dir.path().join("o.nii")),
    ]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("oblique"));
}

#[test]
fn import_ct_accepts_a_dicom_seg_in_place_of_rtstruct() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = tempfile::tempdir().unwrap();
    let study = dir.path().join("study");
    ok(&["benchmark", "generate", s(&study)]);
    // CT-only directory plus a SEG.
    let ct_dir = dir.path().join("ct-and-seg");
    fs::create_dir(&ct_dir).unwrap();
    let ct_files = openbnct_dicom::collect_study_paths(&study.join("ct"));
    for (i, f) in ct_files.iter().enumerate() {
        fs::copy(f, ct_dir.join(format!("ct{i:03}.dcm"))).unwrap();
    }
    let ct = openbnct_dicom::import_ct_series(&ct_files).unwrap();
    let [nx, ny, nz] = ct.geometry.shape.map(|v| v as usize);
    let mut a = vec![0.0; nx * ny * nz];
    let mut b = vec![0.0; nx * ny * nz];
    for k in 10..20 {
        for j in 10..20 {
            for i in 10..20 {
                a[k * nx * ny + j * nx + i] = 1.0;
            }
        }
    }
    for k in 25..30 {
        for j in 5..10 {
            for i in 5..10 {
                b[k * nx * ny + j * nx + i] = 1.0;
            }
        }
    }
    write_seg(
        &ct_dir.join("seg.dcm"),
        &SyntheticSegSpec {
            geometry: ct.geometry.clone(),
            frame_of_reference_uid: FRAME_OF_REFERENCE_UID.into(),
            study_instance_uid: STUDY_INSTANCE_UID.into(),
            referenced_series_uid: Some(CT_SERIES_INSTANCE_UID.into()),
            max_fractional: None,
            segments: vec![
                SyntheticSegment {
                    number: 1,
                    label: "TUMOR".into(),
                    values: a,
                },
                SyntheticSegment {
                    number: 2,
                    label: "OAR".into(),
                    values: b,
                },
            ],
            shared_orientation: true,
        },
    )
    .unwrap();

    let case_path = dir.path().join("case.json");
    let hu_path = dir.path().join("hu.nii");
    let masks = dir.path().join("masks");
    let void = root.join("benchmarks/synthetic/layered-head-phantom/materials/void.json");
    ok(&[
        "dicom",
        "import-ct",
        "--series",
        s(&ct_dir),
        "--spacing-mm",
        "5",
        "--case-id",
        "test.import-seg.v1",
        "--base-material",
        s(&void),
        "--case-output",
        s(&case_path),
        "--hu-output",
        s(&hu_path),
        "--masks-dir",
        s(&masks),
    ]);
    let index: serde_json::Value =
        serde_json::from_slice(&fs::read(masks.join("index.json")).unwrap()).unwrap();
    let names: Vec<&str> = index["masks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["TUMOR", "OAR"]);
    // Grid spacing equals the CT's, so the mask voxel counts are exact.
    assert_eq!(index["masks"][0]["grid_voxels"], 1000);
    assert_eq!(index["masks"][1]["grid_voxels"], 125);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(case_path.with_extension("import-record.json")).unwrap())
            .unwrap();
    assert_eq!(record["structures_source"], "seg");
}

fn reference(id: &str) -> ContentReference {
    ContentReference {
        id: id.into(),
        sha256: "sha256:".to_string() + &"ab".repeat(32),
    }
}

fn dose_bundle() -> PhysicalDoseBundle {
    PhysicalDoseBundle {
        schema_version: "openbnct.physical-dose-bundle/0.2.0".into(),
        case_id: "rtdose-test".into(),
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
        response_set: reference("r"),
        components: vec![DoseVolume {
            component: DoseComponent::Photon,
            unit: DoseUnit::Gray,
            values: vec![0.0; 24],
            absolute_standard_uncertainty: None,
        }],
        physical_total: PhysicalTotalDoseVolume {
            unit: DoseUnit::Gray,
            // Linear in index: x + 4y + 12z, times 0.25 Gy.
            values: (0..24).map(|i| f64::from(i) * 0.25).collect(),
            absolute_standard_uncertainty: None,
            uncertainty_method: TotalUncertaintyMethod::Unavailable,
        },
        provenance_id: "prov-rtdose".into(),
    }
}

fn case_json(dir: &Path, name: &str, geometry: &GridGeometry) -> std::path::PathBuf {
    let path = dir.join(name);
    let doc = serde_json::json!({
        "schema_version": "openbnct.transport-case/0.1.0",
        "case_id": "rtdose-case",
        "frame_of_reference_uid": "1.2.3.4.5",
        "geometry": geometry,
    });
    fs::write(&path, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    path
}

#[test]
fn import_rtdose_native_and_resampled() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dose_bundle();
    let dcm = dir.path().join("dose.dcm");
    let exported = export_rt_dose(
        &bundle,
        DoseSelection::PhysicalTotal,
        &RtDoseExportOptions::default(),
        &dcm,
    )
    .unwrap();

    // Native grid.
    let case = case_json(dir.path(), "case.json", &bundle.geometry);
    let out = dir.path().join("native.json");
    ok(&[
        "import",
        "rtdose",
        "--file",
        s(&dcm),
        "--case",
        s(&case),
        "--fractions",
        "2",
        "--output",
        s(&out),
    ]);
    let native: openbnct_core::ExternalDoseBundle =
        serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(native.case_id, "rtdose-case");
    assert_eq!(native.fraction_count(), 2);
    assert_eq!(native.geometry, bundle.geometry);
    assert!(
        native
            .provenance_id
            .starts_with("external-dose:dicom-rtdose:sha256:")
    );
    for (got, want) in native.values.iter().zip(&bundle.physical_total.values) {
        assert!((got - want).abs() <= exported.dose_grid_scaling);
    }
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(out.with_extension("import-record.json")).unwrap())
            .unwrap();
    assert_eq!(record["resampled"], false);
    assert_eq!(record["rtdose"]["dose_units"], "GY");
    assert_eq!(
        record["inputs"]["rtdose"]["sha256"],
        openbnct_evidence::sha256_file(&dcm).unwrap()
    );

    // Case grid shifted half a voxel in x: trilinear resample.
    let mut shifted = bundle.geometry.clone();
    shifted.origin_mm[0] += 1.0;
    let case2 = case_json(dir.path(), "case2.json", &shifted);
    let out2 = dir.path().join("shifted.json");
    let stdout = ok(&[
        "import",
        "rtdose",
        "--file",
        s(&dcm),
        "--case",
        s(&case2),
        "--output",
        s(&out2),
    ]);
    assert!(stdout.contains("resampled"), "{stdout}");
    let resampled: openbnct_core::ExternalDoseBundle =
        serde_json::from_slice(&fs::read(&out2).unwrap()).unwrap();
    // Voxel (0,0,0) of the shifted grid sits at dose index x = 0.5: 0.125 Gy.
    assert!(
        (resampled.values[0] - 0.125).abs() < 1e-6,
        "{}",
        resampled.values[0]
    );
    let record2: serde_json::Value =
        serde_json::from_slice(&fs::read(out2.with_extension("import-record.json")).unwrap())
            .unwrap();
    assert_eq!(record2["resampled"], true);

    // Mismatched frame of reference is refused.
    let mut doc: serde_json::Value = serde_json::from_slice(&fs::read(&case).unwrap()).unwrap();
    doc["frame_of_reference_uid"] = "9.9.9".into();
    let case3 = dir.path().join("case3.json");
    fs::write(&case3, serde_json::to_vec(&doc).unwrap()).unwrap();
    let bad = openbnct(&[
        "import",
        "rtdose",
        "--file",
        s(&dcm),
        "--case",
        s(&case3),
        "--output",
        s(&dir.path().join("bad.json")),
    ]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("Frame of Reference"));
}
