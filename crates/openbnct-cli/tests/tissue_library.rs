// SPDX-License-Identifier: MIT

//! Integrity of the committed standard tissue library (`libraries/tissue`):
//! every material validates, mass fractions sum to one within 1e-6, densities
//! are positive, N14 is present, the HU calibration validates, and every
//! file's sha256 matches the manifest.

use std::path::{Path, PathBuf};

use openbnct_transport::{HuCalibration, MaterialDefinition};

fn lib_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libraries/tissue")
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn library_materials_validate_and_match_manifest() {
    let dir = lib_dir();
    let manifest = read_json(&dir.join("manifest.json"));
    let materials = manifest["materials"].as_array().unwrap();
    assert!(materials.len() >= 9, "expected the standard tissue set");
    for entry in materials {
        let file = dir.join(entry["file"].as_str().unwrap());
        let bytes = std::fs::read(&file).unwrap();
        let material: MaterialDefinition = serde_json::from_slice(&bytes).unwrap();
        material.validate().unwrap();
        assert!(material.density_g_cm3 > 0.0, "{}", material.id);
        let sum: f64 = material.nuclides.iter().map(|n| n.mass_fraction).sum();
        assert!((sum - 1.0).abs() < 1e-6, "{} sums to {sum}", material.id);
        // Every entry except pure water (H2O) carries nitrogen.
        if !material.id.contains("water-liquid") {
            assert!(
                material.nuclides.iter().any(|n| n.name == "N14"),
                "{} lacks N14",
                material.id
            );
        }
        assert_eq!(material.id, entry["id"].as_str().unwrap());
        assert_eq!(
            openbnct_evidence::sha256_file(&file).unwrap(),
            entry["sha256"].as_str().unwrap(),
            "{} hash drifted from manifest",
            entry["file"]
        );
        assert!(entry["source"]["printed_material_name"].is_string());
        assert!(entry["source"]["printed_page_of_357"].is_u64());
    }
}

#[test]
fn library_hu_calibration_validates_and_is_hash_bound() {
    let dir = lib_dir();
    let manifest = read_json(&dir.join("manifest.json"));
    let cal_entry = &manifest["hu_calibration"];
    let file = dir.join(cal_entry["file"].as_str().unwrap());
    let cal: HuCalibration = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    cal.validate().unwrap();
    assert!(
        cal.validity_domain
            .as_deref()
            .unwrap()
            .contains("NOT a scanner calibration")
    );
    assert_eq!(
        openbnct_evidence::sha256_file(&file).unwrap(),
        cal_entry["sha256"].as_str().unwrap()
    );
    // Every anchor is a library material (same nuclides and density).
    let library: Vec<MaterialDefinition> = manifest["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            serde_json::from_slice(&std::fs::read(dir.join(m["file"].as_str().unwrap())).unwrap())
                .unwrap()
        })
        .collect();
    for anchor in &cal.anchors {
        assert!(
            library.contains(&anchor.material),
            "anchor {} is not a library material",
            anchor.material.id
        );
    }
}

#[test]
fn library_multigroup_data_is_hash_bound_and_carries_boron_unit() {
    let dir = lib_dir();
    let manifest = read_json(&dir.join("manifest.json"));
    let entry = &manifest["multigroup_data"];
    let file = dir.join(entry["file"].as_str().unwrap());
    assert_eq!(
        openbnct_evidence::sha256_file(&file).unwrap(),
        entry["sha256"].as_str().unwrap()
    );
    let data = read_json(&file);
    assert!(data["boron_unit_response_gy_cm2_per_ug_g"].is_array());
    let names: Vec<&str> = data["materials"]
        .as_array()
        .map(|m| m.iter().filter_map(|x| x["material_id"].as_str()).collect())
        .unwrap_or_default();
    for m in manifest["materials"].as_array().unwrap() {
        let id = m["id"].as_str().unwrap();
        assert!(names.contains(&id), "multigroup data lacks {id}");
    }
}
