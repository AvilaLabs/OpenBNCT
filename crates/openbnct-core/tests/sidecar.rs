// SPDX-License-Identifier: MIT

//! Round-trip and failure-mode tests for hash-bound external arrays.

use std::fs;
use std::path::{Path, PathBuf};

use openbnct_core::sidecar::{
    self, DTYPE_F64LE, ExternalArrayRef, F64Array, external_refs, load_json,
    verify_document_sidecars, with_write_context_min,
};
use openbnct_core::{
    ComponentProfileReference, ContentReference, DoseComponent, DoseUnit, DoseVolume, GridGeometry,
    PHYSICAL_DOSE_BUNDLE_SCHEMA, PhysicalDoseBundle, PhysicalTotalDoseVolume,
    TotalUncertaintyMethod, load_physical_dose_bundle,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Probe {
    #[serde(with = "openbnct_core::sidecar::values")]
    values: Vec<f64>,
    #[serde(default, with = "openbnct_core::sidecar::opt_uncertainty")]
    absolute_standard_uncertainty: Option<Vec<f64>>,
    #[serde(with = "openbnct_core::sidecar::flux_rows")]
    flux: Vec<Vec<f64>>,
}

fn probe() -> Probe {
    Probe {
        values: vec![0.1, 1.0e-300, -2.5, 3.0e17],
        absolute_standard_uncertainty: Some(vec![0.5, 0.25]),
        flux: vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]],
    }
}

fn write(dir: &Path, min: i64) -> PathBuf {
    let path = dir.join("probe.json");
    let bytes = with_write_context_min(&path, false, min, || serde_json::to_vec_pretty(&probe()))
        .unwrap()
        .unwrap();
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn inline_output_is_identical_to_plain_serialization() {
    let dir = tempfile::tempdir().unwrap();
    let plain = serde_json::to_vec_pretty(&probe()).unwrap();
    for min in [-1, 1_000_000] {
        let path = write(dir.path(), min);
        assert_eq!(fs::read(&path).unwrap(), plain);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
    }
    let back: Probe = sidecar::from_slice_in(&plain, dir.path()).unwrap();
    assert_eq!(back, probe());
}

#[test]
fn external_round_trip_is_exact() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), 0);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"external\""), "{text}");
    assert!(dir.path().join("probe.values.f64le").is_file());
    assert!(
        dir.path()
            .join("probe.absolute_standard_uncertainty.f64le")
            .is_file()
    );
    assert!(dir.path().join("probe.flux.f64le").is_file());
    let back: Probe = load_json(&path).unwrap();
    assert_eq!(back, probe());
    assert_eq!(external_refs(text.as_bytes()).unwrap().len(), 3);
    assert_eq!(verify_document_sidecars(&path).unwrap().len(), 3);
}

#[test]
fn threshold_is_strictly_greater_than() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.json");
    let inline =
        with_write_context_min(&path, false, 6, || serde_json::to_string(&probe()).unwrap())
            .unwrap();
    assert!(!inline.contains("external"));
    let mixed =
        with_write_context_min(&path, false, 3, || serde_json::to_string(&probe()).unwrap())
            .unwrap();
    // values (4) and flux (6) externalize; the 2-value sigma stays inline.
    assert_eq!(mixed.matches("external").count(), 2);
}

#[test]
fn tampered_sidecar_is_a_sha256_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), 0);
    let side = dir.path().join("probe.values.f64le");
    let mut bytes = fs::read(&side).unwrap();
    bytes[0] ^= 1;
    fs::write(&side, bytes).unwrap();
    let err = load_json::<Probe>(&path).unwrap_err().to_string();
    assert!(err.contains("sha256 mismatch"), "{err}");
    assert!(err.contains("probe.values.f64le"), "{err}");
    assert!(verify_document_sidecars(&path).is_err());
}

#[test]
fn wrong_length_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), 0);
    let side = dir.path().join("probe.values.f64le");
    let mut bytes = fs::read(&side).unwrap();
    bytes.truncate(bytes.len() - 8);
    fs::write(&side, bytes).unwrap();
    let err = load_json::<Probe>(&path).unwrap_err().to_string();
    assert!(err.contains("length mismatch"), "{err}");
}

#[test]
fn missing_sidecar_names_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), 0);
    fs::remove_file(dir.path().join("probe.flux.f64le")).unwrap();
    let err = load_json::<Probe>(&path).unwrap_err().to_string();
    assert!(err.contains("probe.flux.f64le"), "{err}");
}

#[test]
fn external_without_base_directory_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), 0);
    let err = serde_json::from_slice::<Probe>(&fs::read(&path).unwrap())
        .unwrap_err()
        .to_string();
    assert!(err.contains("load_*"), "{err}");
}

#[test]
fn hostile_references_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    for path in ["../x.f64le", "/etc/passwd", ""] {
        let reference = ExternalArrayRef {
            path: path.into(),
            sha256: "0".repeat(64),
            len: 1,
            dtype: DTYPE_F64LE.into(),
            row_len: None,
        };
        assert!(reference.load(dir.path()).is_err(), "{path:?}");
    }
    let bad_dtype = ExternalArrayRef {
        path: "a".into(),
        sha256: "0".repeat(64),
        len: 1,
        dtype: "f32le".into(),
        row_len: None,
    };
    assert!(bad_dtype.load(dir.path()).is_err());
}

#[test]
fn f64_array_two_phase_resolve() {
    let dir = tempfile::tempdir().unwrap();
    let inline: F64Array = serde_json::from_str("[1.0, 2.0]").unwrap();
    assert_eq!(inline.resolve(dir.path()).unwrap(), vec![1.0, 2.0]);
    let data = [1.5_f64, -2.0];
    let raw: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    fs::write(dir.path().join("a.f64le"), &raw).unwrap();
    let json = format!(
        "{{\"external\":{{\"path\":\"a.f64le\",\"sha256\":\"{:x}\",\"len\":2,\"dtype\":\"f64le\"}}}}",
        Sha256::digest(&raw)
    );
    let external: F64Array = serde_json::from_str(&json).unwrap();
    assert!(matches!(external, F64Array::External(_)));
    assert_eq!(external.resolve(dir.path()).unwrap(), data.to_vec());
}

fn bundle() -> PhysicalDoseBundle {
    let n = 8_usize;
    let volume = |component| DoseVolume {
        component,
        unit: DoseUnit::GrayPerSourceParticle,
        values: (0..n).map(|i| i as f64 * 1.0e-12).collect(),
        absolute_standard_uncertainty: Some(vec![1.0e-14; n]),
    };
    PhysicalDoseBundle {
        schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: "c".into(),
        frame_of_reference_uid: None,
        geometry: GridGeometry {
            shape: [2, 2, 2],
            spacing_mm: [1.0; 3],
            origin_mm: [0.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        component_profile: ComponentProfileReference {
            id: "p".into(),
            sha256: "a".repeat(64),
        },
        response_set: ContentReference {
            id: "r".into(),
            sha256: "b".repeat(64),
        },
        components: DoseComponent::REQUIRED.into_iter().map(volume).collect(),
        physical_total: PhysicalTotalDoseVolume {
            unit: DoseUnit::GrayPerSourceParticle,
            values: vec![1.0e-11; n],
            absolute_standard_uncertainty: None,
            uncertainty_method: TotalUncertaintyMethod::Unavailable,
        },
        provenance_id: "prov".into(),
    }
}

#[test]
fn physical_dose_bundle_round_trips_through_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dose.json");
    let original = bundle();
    original.validate().unwrap();
    let bytes = with_write_context_min(&path, false, 0, || {
        serde_json::to_vec_pretty(&original).unwrap()
    })
    .unwrap();
    fs::write(&path, &bytes).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("dose.values.f64le"));
    assert!(dir.path().join("dose.values.2.f64le").is_file());
    let loaded = load_physical_dose_bundle(&path).unwrap();
    assert_eq!(loaded, original);
    // A plain serde read (no base directory) must refuse, not truncate.
    assert!(serde_json::from_str::<PhysicalDoseBundle>(&text).is_err());
    // The inline encoding carries the same value.
    let inline = serde_json::to_vec_pretty(&original).unwrap();
    let from_inline: PhysicalDoseBundle = serde_json::from_slice(&inline).unwrap();
    assert_eq!(from_inline, loaded);
}
