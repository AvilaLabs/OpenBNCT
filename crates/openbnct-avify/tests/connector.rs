// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;
use std::io::Read;

use openbnct_avify::{AvifyCertificate, AvifySpec, EngineClass, EngineInvocation};

fn spec_json() -> serde_json::Value {
    serde_json::json!({
        "schema_version": "openbnct.avify-spec/0.1.0",
        "default_class": "air",
        "region_classes": {
            "brain-region": "brain",
            "scalp-region": "scalp",
            "tumour-region": "tumour",
            "bone-region": "cranium"
        },
        "density_g_cm3": {"air": 0.001293, "brain": 1.04, "cranium": 1.61, "scalp": 1.09, "tumour": 1.04},
        "plan": {
            "declared_set": {"rt": [2.5, 3.5, 4.5], "rs": [0.8, 1.0, 1.2], "B": [15.0, 20.0, 25.0]},
            "brain_ratio": 1.0,
            "weights": {"tumour": [3.8, 3.2, 3.2, 1.0], "brain": [1.35, 3.2, 3.2, 1.0], "scalp": [2.5, 3.2, 3.2, 1.0]},
            "criteria": {"tumour": [">=", 20.0], "brain": ["<=", 11.0], "scalp": ["<=", 10.0]},
            "normalisation": {"source_particles_total": 4.0e7},
            "histories": {"corner": 1000, "nominal": 1000},
            "seeds": {"lo": 1, "hi": 2, "nom": 3},
            "beam": "epithermal"
        }
    })
}

fn toy_case() -> openbnct_transport::TransportCase {
    serde_json::from_value(serde_json::json!({
        "schema_version": "openbnct.transport-case/0.1.0",
        "case_id": "toy",
        "geometry": {"shape": [4, 4, 4], "spacing_mm": [5.0, 5.0, 5.0], "origin_mm": [-7.5, -7.5, -7.5], "direction": [1.0,0.0,0.0, 0.0,1.0,0.0, 0.0,0.0,1.0]},
        "material": {"schema_version": "openbnct.material-definition/0.1.0", "id": "void", "density_g_cm3": 1e-9, "temperature_k": 293.6, "nuclides": [{"name": "H1", "mass_fraction": 1.0}], "neutron_thermal_treatment": "free_gas"},
        "source": {"schema_version": "openbnct.fixed-source-definition/0.1.0", "id": "s", "particle": "neutron", "source_sites_per_history": 1, "statistical_weight_per_site": 1.0, "space": {"kind": "uniform_disk", "axis": "z", "offset_cm": 0.0, "center_uv_cm": [0.0, 0.0], "radius_cm": 1.0}, "angle": {"kind": "isotropic_cone", "axis_unit_vector": [0.0, 0.0, 1.0], "half_angle_rad": 0.1}, "energy": {"kind": "monoenergetic", "energy_ev": 1.0e4}},
        "requested_histories": 1
    }))
    .unwrap()
}

fn toy_assignment() -> openbnct_transport::MaterialAssignment {
    let mat: openbnct_transport::MaterialDefinition = serde_json::from_value(serde_json::json!({
        "schema_version": "openbnct.material-definition/0.1.0", "id": "tissue",
        "density_g_cm3": 1.04, "temperature_k": 293.6,
        "nuclides": [{"name": "H1", "mass_fraction": 1.0}], "neutron_thermal_treatment": "free_gas"
    }))
    .unwrap();
    serde_json::from_value(serde_json::json!({
        "schema_version": "openbnct.material-assignment/0.2.0",
        "case_id": "toy",
        "base_material": {"schema_version": "openbnct.material-definition/0.1.0", "id": "void", "density_g_cm3": 1e-9, "temperature_k": 293.6, "nuclides": [{"name": "H1", "mass_fraction": 1.0}], "neutron_thermal_treatment": "free_gas"},
        "regions": [
            {"name": "brain-region", "material": mat, "shape": {"kind": "voxel_box", "lower": [0,0,0], "upper": [1,3,3]}},
            {"name": "scalp-region", "material": mat, "shape": {"kind": "voxel_box", "lower": [2,0,0], "upper": [2,3,3]}},
            {"name": "tumour-region", "material": mat, "shape": {"kind": "voxel_set", "indices": [[3,0,0],[3,1,1]]}},
            {"name": "bone-region", "material": mat, "shape": {"kind": "voxel_box", "lower": [3,2,0], "upper": [3,3,3]}}
        ],
        "provenance_id": "case:sha256:test"
    }))
    .unwrap()
}

#[test]
fn spec_roundtrip_and_validate() {
    let spec: AvifySpec = serde_json::from_value(spec_json()).unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.default_class, EngineClass::Air);
    assert_eq!(spec.region_classes["tumour-region"], EngineClass::Tumour);
}

#[test]
fn spec_rejects_missing_roi_weights() {
    let mut v = spec_json();
    v["plan"]["weights"]
        .as_object_mut()
        .unwrap()
        .remove("scalp");
    let spec: AvifySpec = serde_json::from_value(v).unwrap();
    assert!(spec.validate().is_err());
}

#[test]
fn export_produces_loadable_npz_and_meta() {
    let dir = std::env::temp_dir().join(format!("avify-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let prefix = dir.join("plan");
    let spec: AvifySpec = serde_json::from_value(spec_json()).unwrap();
    let export =
        openbnct_avify::export_voxel_plan(&toy_case(), &toy_assignment(), &spec, &prefix).unwrap();

    // npz is a zip of .npy members — verify magic + headers directly.
    let f = std::fs::File::open(&export.arrays_path).unwrap();
    let mut z = zip::ZipArchive::new(f).unwrap();
    let names: Vec<String> = (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().to_string())
        .collect();
    for expected in [
        "cls.npy",
        "roi_brain.npy",
        "roi_scalp.npy",
        "roi_tumour.npy",
    ] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
    let mut cls = Vec::new();
    z.by_name("cls.npy").unwrap().read_to_end(&mut cls).unwrap();
    assert_eq!(&cls[..6], b"\x93NUMPY");
    let header_end = 8 + u16::from_le_bytes([cls[8], cls[9]]) as usize + 2;
    let header = String::from_utf8_lossy(&cls[10..header_end]).to_string();
    assert!(header.contains("'descr': '|i1'"));
    assert!(header.contains("(4, 4, 4)"));
    // zyx order: voxel [i,j,k] sits at k*16+j*4+i. Region layout:
    // brain box i∈{0,1}, scalp i=2, tumour [3,0,0]+[3,1,1], bone i=3 j≥2.
    let data = &cls[header_end..];
    assert_eq!(data[0], EngineClass::Brain.index() as u8); // [0,0,0]
    assert_eq!(data[2], EngineClass::Scalp.index() as u8); // [2,0,0] → zi=0+0+2
    assert_eq!(data[3], EngineClass::Tumour.index() as u8); // [3,0,0] → zi=3
    assert_eq!(data[23], EngineClass::Tumour.index() as u8); // [3,1,1] → zi=16+4+3=23
    assert_eq!(data[7], EngineClass::Air.index() as u8); // [3,1,0] unclaimed → air
    // bone region i=3, j∈{2,3}, k all → cell [3,2,0] → zi=0+8+3=11
    assert_eq!(data[11], EngineClass::Cranium.index() as u8);
    // air cell [3,0,3] → zi=3*16+0+3=51
    assert_eq!(data[51], EngineClass::Air.index() as u8);

    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&export.meta_path).unwrap()).unwrap();
    assert_eq!(meta["voxel_cm"], 0.5);
    assert_eq!(meta["classes"].as_array().unwrap().len(), 5);
    // origin center -7.5mm, spacing 5 → corner -10mm = -1.0cm
    assert_eq!(meta["lower_left_cm_xyz"].as_array().unwrap()[0], -1.0);
    assert_eq!(export.class_voxels["tumour"], 2);
    assert_eq!(export.class_voxels["brain"], 32);

    // Round-trip through the reader: same zyx order and masks.
    let back = openbnct_avify::read_arrays_npz(&export.arrays_path).unwrap();
    assert_eq!(back.shape_zyx, [4, 4, 4]);
    assert_eq!(back.cls_zyx.len(), 64);
    assert_eq!(back.cls_zyx[0], EngineClass::Brain.index() as i8);
    assert_eq!(back.cls_zyx[23], EngineClass::Tumour.index() as i8);
    assert_eq!(back.cls_zyx[11], EngineClass::Cranium.index() as i8);
    let tumour = &back.rois.iter().find(|(n, _)| n == "tumour").unwrap().1;
    assert!(tumour[3] && tumour[23] && !tumour[0]);
    let brain = &back.rois.iter().find(|(n, _)| n == "brain").unwrap().1;
    assert_eq!(brain.iter().filter(|&&b| b).count(), 32);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_rejects_zero_voxel_roi() {
    let dir = std::env::temp_dir().join(format!("avify-test-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut spec: AvifySpec = serde_json::from_value(spec_json()).unwrap();
    // Map the tumour region to scalp instead → zero tumour voxels.
    spec.region_classes
        .insert("tumour-region".into(), EngineClass::Scalp);
    let err =
        openbnct_avify::export_voxel_plan(&toy_case(), &toy_assignment(), &spec, &dir.join("plan"))
            .unwrap_err();
    assert!(err.to_string().contains("zero voxels"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn certificate_parses_engine_output() {
    let cert = AvifyCertificate::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/certificate.json"),
    )
    .unwrap();
    assert_eq!(cert.actions.len(), 3);
    let tumour = &cert.actions["tumour"];
    assert_eq!(tumour.criterion.0, ">=");
    assert_eq!(tumour.action, "FAIL");
    assert_eq!(cert.actions["brain"].action, "PASS");
    assert!(cert.brackets["brain"].photon_valid == Some(true));
    assert_eq!(cert.runs.len(), 3);
    assert!(cert.runs["corner_lo"].wall_s > 0.0);
    assert_eq!(cert.runs["corner_lo"].seed, 1);
}

#[test]
fn receipt_roundtrip_and_staleness() {
    use openbnct_avify::receipt::*;
    let dir = std::env::temp_dir().join(format!("avify-test-receipt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("case.json");
    std::fs::write(&input, b"{}").unwrap();
    let inputs = bind_inputs(&[("case", &input)]).unwrap();
    let receipt = receipt_for_run(
        inputs,
        BTreeMap::new(),
        EngineRecord {
            argv0: vec!["avify-dose".into()],
            version: "avify-dose 0.1.0".into(),
            threads: Some(2),
            timeout_s: 600,
        },
        BoundInput {
            path: dir.join("certificate.json"),
            sha256: "ab".repeat(32),
        },
        12.5,
    );
    let path = dir.join("avify-run.json");
    receipt.write(&path).unwrap();
    let loaded = AvifyRunReceipt::load(&path).unwrap();
    let states = check_staleness(&loaded, &dir);
    assert_eq!(states["case"], InputState::Current);
    assert!(!is_stale(&states));
    // Mutate the input → Changed; remove it → Missing.
    std::fs::write(&input, b"{\"changed\":true}").unwrap();
    let states = check_staleness(&loaded, &dir);
    assert!(matches!(states["case"], InputState::Changed(_)));
    assert!(is_stale(&states));
    std::fs::remove_file(&input).unwrap();
    assert_eq!(check_staleness(&loaded, &dir)["case"], InputState::Missing);
    // Wrong schema version is rejected.
    let mut bad: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    bad["schema_version"] = serde_json::json!("openbnct.avify-run/9.9.9");
    std::fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
    assert!(AvifyRunReceipt::load(&path).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn engine_version_is_bounded_and_optional() {
    // A real --version responder.
    let inv = EngineInvocation {
        argv0: vec!["sh".into(), "-c".into(), "echo 'fake-engine 9.9'".into()],
        timeout_s: 5,
    };
    assert_eq!(inv.engine_version(), "fake-engine 9.9");
    // No such binary → "unknown", not an error.
    let inv = EngineInvocation {
        argv0: vec!["/nonexistent/engine".into()],
        timeout_s: 5,
    };
    assert_eq!(inv.engine_version(), "unknown");
}

#[test]
fn engine_timeout_kills_and_reaps() {
    // A fake engine that sleeps forever; timeout must kill+reap it.
    let dir = std::env::temp_dir().join(format!("avify-test-timeout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("p_arrays.npz"), b"").unwrap();
    std::fs::write(dir.join("p.plan.json"), b"{}").unwrap();
    let inv = EngineInvocation {
        // sh -c ignores the appended verify args; the child sleeps until killed.
        argv0: vec!["sh".into(), "-c".into(), "sleep 300".into()],
        timeout_s: 1,
    };
    let err = inv
        .verify(&dir.join("p"), &dir.join("p.plan.json"), &dir, None)
        .unwrap_err();
    assert!(err.to_string().contains("timed out"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn diff_runs_reports_interval_and_input_changes() {
    use openbnct_avify::diff_runs;
    let dir = std::env::temp_dir().join(format!("avify-test-diff-{}", std::process::id()));
    let make_run = |name: &str, spec_sha: &str, tumour_action: &str| {
        let d = dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        let cert = serde_json::json!({
            "plan_sha256": spec_sha,
            "ingest_meta_sha256": spec_sha,
            "ingest_arrays_sha256": spec_sha,
            "scale_Gyw_per_MeVg": 1.0,
            "runs": {},
            "brackets": {},
            "actions": {
                "tumour": {
                    "criterion": [">=", 20.0],
                    "certified_Gyw": [if tumour_action == "PASS" { 21.0 } else { 0.0 },
                                    if tumour_action == "PASS" { 23.0 } else { 0.0 }],
                    "nominal_Gyw": 22.0,
                    "action": tumour_action
                },
                "brain": {
                    "criterion": ["<=", 11.0],
                    "certified_Gyw": [5.0, 8.0],
                    "nominal_Gyw": 6.5,
                    "action": "PASS"
                }
            }
        });
        let cert_path = d.join("certificate.json");
        std::fs::write(&cert_path, serde_json::to_vec(&cert).unwrap()).unwrap();
        let receipt = serde_json::json!({
            "schema_version": "openbnct.avify-run/0.1.0",
            "created_unix_seconds": 1,
            "inputs": {"spec": {"path": d.join("spec.json"), "sha256": spec_sha}},
            "exported": {},
            "engine": {"argv0": ["avify-dose"], "version": "avify-dose 0.1.0"},
            "certificate": {"path": cert_path, "sha256": spec_sha},
            "engine_elapsed_s": 1.0
        });
        std::fs::write(
            d.join("avify-run.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        d
    };
    let a = make_run("a", &"aa".repeat(32), "FAIL");
    let b = make_run("b", &"bb".repeat(32), "PASS");
    let d = diff_runs(&a, &b).unwrap();
    assert!(d.roi_changes["tumour"].action_changed);
    assert_eq!(d.roi_changes["tumour"].certified_after, [21.0, 23.0]);
    assert!(!d.roi_changes["brain"].action_changed);
    let mut names: Vec<_> = d.input_changes.iter().map(|c| c.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["certificate", "spec"]);
    // Same run twice: nothing differs.
    let same = diff_runs(&a, &a).unwrap();
    assert!(same.input_changes.is_empty());
    assert!(!same.roi_changes["tumour"].action_changed);
    std::fs::remove_dir_all(&dir).ok();
}
