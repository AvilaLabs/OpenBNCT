// SPDX-License-Identifier: MIT

//! Hash-bound external-array ("sidecar") artifacts end to end: with
//! `OPENBNCT_SIDECAR_MIN_VALUES=0` the solve, fold and boron commands write
//! raw `f64le` sidecars, and every consumer (`sn fold`, `boron dose`, `dvh`,
//! `nifti export-dose`, evidence export/verify) reads them back with numbers
//! identical to the all-inline run.

use openbnct_core::{GridGeometry, PhysicalDoseBundle, load_physical_dose_bundle};
use openbnct_transport::{
    AngularDistribution, EnergyDistribution, FixedSourceDefinition, MaterialDefinition,
    MultigroupData, MultigroupMaterial, NeutronThermalTreatment, NuclideMassFraction, ParticleType,
    PlaneAxis, SourceSpatialDistribution, TransportCase,
};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const UNIT: f64 = 3.0e-12;
const CONC: f64 = 25.0;

fn run(args: &[&str], min_values: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_openbnct"));
    command.args(args).env_remove("OPENBNCT_SIDECAR_MIN_VALUES");
    if let Some(min) = min_values {
        command.env("OPENBNCT_SIDECAR_MIN_VALUES", min);
    }
    command.output().expect("spawn openbnct")
}

fn ok_env(args: &[&str], min_values: Option<&str>) -> String {
    let out = run(args, min_values);
    assert!(
        out.status.success(),
        "openbnct {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn ok(args: &[&str]) -> String {
    ok_env(args, None)
}

fn fails(args: &[&str]) -> String {
    let out = run(args, None);
    assert!(
        !out.status.success(),
        "openbnct {args:?} should have failed"
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn case() -> TransportCase {
    TransportCase {
        schema_version: "openbnct.transport-case/0.1.0".into(),
        case_id: "mg-slab".into(),
        geometry: GridGeometry {
            shape: [4, 4, 20],
            spacing_mm: [1.0; 3],
            origin_mm: [-1.5, -1.5, -9.5],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        material: MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: "absorber".into(),
            density_g_cm3: 1.0,
            temperature_k: 294.0,
            nuclides: vec![NuclideMassFraction {
                name: "B10".into(),
                mass_fraction: 1.0,
            }],
            neutron_thermal_treatment: NeutronThermalTreatment::FreeGas,
            boron_microdistribution: None,
        },
        source: FixedSourceDefinition {
            schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
            id: "beam".into(),
            particle: ParticleType::Neutron,
            source_sites_per_history: 1,
            statistical_weight_per_site: 1.0,
            space: SourceSpatialDistribution::UniformDisk {
                axis: PlaneAxis::Z,
                offset_cm: -1.0,
                center_uv_cm: [0.0, 0.0],
                radius_cm: 0.25,
            },
            angle: AngularDistribution::Monodirectional {
                unit_vector: [0.0, 0.0, 1.0],
            },
            energy: EnergyDistribution::Monoenergetic { energy_ev: 0.0253 },
        },
        requested_histories: 1,
    }
}

fn data() -> MultigroupData {
    MultigroupData {
        schema_version: "openbnct.multigroup-data/0.1.0".into(),
        id: "mg-data".into(),
        energy_boundaries_ev: vec![1.0, 1.0e-3],
        collapse_declaration: "test fixture".into(),
        component_profile: Some(openbnct_core::ContentReference {
            id: "profile".into(),
            sha256: "a".repeat(64),
        }),
        boron_unit_response_gy_cm2_per_ug_g: Some(vec![UNIT]),
        materials: vec![MultigroupMaterial {
            material_id: "absorber".into(),
            sigma_total_per_cm: vec![0.1],
            scatter_matrix_per_cm: vec![0.0],
            scatter_p1_matrix_per_cm: None,
            scatter_legendre_moments_per_cm: None,
            dose_response_gy_cm2: [
                ("boron".to_string(), vec![CONC * UNIT]),
                ("nitrogen".to_string(), vec![2.0e-13]),
                ("hydrogen".to_string(), vec![5.0e-13]),
                ("photon".to_string(), vec![7.0e-13]),
            ]
            .into_iter()
            .collect(),
            transport_mu_bar: None,
            beam_sigma_nodes_per_cm: None,
        }],
    }
}

fn read_bundle(path: &Path) -> PhysicalDoseBundle {
    load_physical_dose_bundle(path).unwrap()
}

fn raw(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn has_external(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .unwrap()
        .contains("\"external\"")
}

/// `sn solve` with dose and boron unit dose outputs; returns
/// `(flux, bundle, unit)` paths tagged `tag`.
fn solve(d: &Path, tag: &str, min_values: &str) -> (PathBuf, PathBuf, PathBuf) {
    let case_path = d.join("case.json");
    let data_path = d.join("data.json");
    if !case_path.exists() {
        std::fs::write(&case_path, serde_json::to_vec(&case()).unwrap()).unwrap();
        std::fs::write(&data_path, serde_json::to_vec(&data()).unwrap()).unwrap();
    }
    let flux = d.join(format!("flux-{tag}.json"));
    let bundle = d.join(format!("bundle-{tag}.json"));
    let unit = d.join(format!("unit-{tag}.json"));
    ok_env(
        &[
            "sn",
            "solve",
            "--case",
            s(&case_path),
            "--data",
            s(&data_path),
            "--periodic",
            "x,y",
            "--dose",
            s(&bundle),
            "--boron-unit-output",
            s(&unit),
            "--output",
            s(&flux),
        ],
        Some(min_values),
    );
    (flux, bundle, unit)
}

fn assert_same_dose(a: &Path, b: &Path) {
    let (a, b) = (read_bundle(a), read_bundle(b));
    assert_eq!(a.geometry, b.geometry);
    assert_eq!(a.components.len(), b.components.len());
    for (x, y) in a.components.iter().zip(&b.components) {
        assert_eq!(x.component, y.component);
        assert_eq!(x.values, y.values, "{:?}", x.component);
        assert_eq!(
            x.absolute_standard_uncertainty,
            y.absolute_standard_uncertainty
        );
    }
    assert_eq!(a.physical_total.values, b.physical_total.values);
}

#[test]
fn sidecar_artifacts_are_consumed_with_identical_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (flux_i, bundle_i, unit_i) = solve(d, "inline", "-1");
    let (flux_s, bundle_s, unit_s) = solve(d, "side", "0");

    // The inline run is pure JSON; the sidecar run externalizes the volumes.
    for p in [&flux_i, &bundle_i, &unit_i] {
        assert!(!has_external(p), "{p:?}");
    }
    for p in [&flux_s, &bundle_s, &unit_s] {
        assert!(has_external(p), "{p:?}");
    }
    assert!(d.join("flux-side.flux.f64le").is_file());
    assert!(d.join("bundle-side.values.f64le").is_file());
    assert!(d.join("unit-side.values.f64le").is_file());
    assert_same_dose(&bundle_i, &bundle_s);

    // The default threshold keeps this small artifact inline, byte-identical
    // to the never-sidecar run.
    let (flux_d, bundle_d, _) = solve(d, "default", "1000000");
    assert_eq!(
        std::fs::read(&flux_d).unwrap().len(),
        std::fs::read(&flux_i).unwrap().len()
    );
    assert_eq!(
        std::fs::read(&bundle_d).unwrap().len(),
        std::fs::read(&bundle_i).unwrap().len()
    );
    assert!(!has_external(&bundle_d));

    // `sn fold` consumes the sidecar flux (and can write sidecars itself).
    let case_path = d.join("case.json");
    let data_path = d.join("data.json");
    let fold_i = d.join("fold-inline.json");
    let fold_s = d.join("fold-side.json");
    ok(&[
        "sn",
        "fold",
        "--case",
        s(&case_path),
        "--data",
        s(&data_path),
        "--flux",
        s(&flux_i),
        "--output",
        s(&fold_i),
    ]);
    ok_env(
        &[
            "sn",
            "fold",
            "--case",
            s(&case_path),
            "--data",
            s(&data_path),
            "--flux",
            s(&flux_s),
            "--output",
            s(&fold_s),
        ],
        Some("0"),
    );
    assert!(has_external(&fold_s));
    assert_same_dose(&fold_i, &fold_s);
    assert_same_dose(&fold_i, &bundle_i);

    // `boron dose` consumes the sidecar bundle and unit dose.
    let boron_i = d.join("boron-inline.json");
    let boron_s = d.join("boron-side.json");
    for (bundle, unit, out, env) in [
        (&bundle_i, &unit_i, &boron_i, "-1"),
        (&bundle_s, &unit_s, &boron_s, "0"),
    ] {
        ok_env(
            &[
                "boron",
                "dose",
                "--physical-bundle",
                s(bundle),
                "--unit-dose",
                s(unit),
                "--blood-ug-g",
                "10",
                "--output",
                s(out),
            ],
            Some(env),
        );
    }
    assert!(has_external(&boron_s));
    assert_same_dose(&boron_i, &boron_s);

    // `dvh` and `nifti export-dose` read the sidecar bundle.
    let n = 4 * 4 * 20;
    let mask: Vec<bool> = (0..n).map(|v| v / 16 >= 10).collect();
    let mask_path = d.join("tumor.json");
    std::fs::write(
        &mask_path,
        serde_json::to_vec(&serde_json::json!({"name": "tumor", "voxels": mask})).unwrap(),
    )
    .unwrap();
    let dvh_i = d.join("dvh-inline.json");
    let dvh_s = d.join("dvh-side.json");
    for (bundle, out) in [(&bundle_i, &dvh_i), (&bundle_s, &dvh_s)] {
        ok(&[
            "dvh",
            "--dose",
            s(bundle),
            "--quantity",
            "physical_total",
            "--mask",
            s(&mask_path),
            "--output",
            s(out),
        ]);
    }
    let (mut a, mut b) = (raw(&dvh_i), raw(&dvh_s));
    // The source references name different (but equal-valued) input files.
    for v in [&mut a, &mut b] {
        let object = v.as_object_mut().unwrap();
        object.remove("source");
        object.remove("provenance_id");
        object.remove("dose");
    }
    assert_eq!(a, b);

    // `metrics-batch` writes byte-identical files to the single commands,
    // for the inline and the sidecar-backed bundle alike.
    for (tag, bundle) in [("inline", &bundle_i), ("side", &bundle_s)] {
        let mut jobs = Vec::new();
        let mut pairs = Vec::new();
        for quantity in ["physical_total", "component:boron"] {
            let stem = quantity.replace(':', "-");
            let single = d.join(format!("single-{tag}-{stem}.metrics.json"));
            let batch = d.join(format!("batch-{tag}-{stem}.metrics.json"));
            ok(&[
                "metrics",
                "--dose",
                s(bundle),
                "--quantity",
                quantity,
                "--mask",
                s(&mask_path),
                "--dx",
                "95,50,2",
                "--output",
                s(&single),
            ]);
            jobs.push(serde_json::json!({
                "kind": "metrics", "quantity": quantity, "mask": s(&mask_path),
                "dx": [95.0, 50.0, 2.0], "output": s(&batch),
            }));
            pairs.push((single, batch));
        }
        let single = d.join(format!("single-{tag}.dvh.json"));
        let batch = d.join(format!("batch-{tag}.dvh.json"));
        ok(&[
            "dvh",
            "--dose",
            s(bundle),
            "--quantity",
            "physical_total",
            "--mask",
            s(&mask_path),
            "--bins",
            "100",
            "--output",
            s(&single),
        ]);
        jobs.push(serde_json::json!({
            "kind": "dvh", "quantity": "physical_total", "mask": s(&mask_path),
            "bins": 100, "output": s(&batch),
        }));
        pairs.push((single, batch));
        let plan = d.join(format!("plan-{tag}.json"));
        std::fs::write(
            &plan,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": "openbnct.metrics-batch-plan/0.1.0",
                "jobs": jobs,
            }))
            .unwrap(),
        )
        .unwrap();
        ok(&["metrics-batch", "--dose", s(bundle), "--plan", s(&plan)]);
        for (single, batch) in &pairs {
            assert_eq!(
                std::fs::read(single).unwrap(),
                std::fs::read(batch).unwrap(),
                "{} differs from {}",
                batch.display(),
                single.display()
            );
        }
    }
    let nii_i = d.join("dose-inline.nii");
    let nii_s = d.join("dose-side.nii");
    for (bundle, out) in [(&bundle_i, &nii_i), (&bundle_s, &nii_s)] {
        ok(&[
            "nifti",
            "export-dose",
            "--dose",
            s(bundle),
            "--quantity",
            "component:boron",
            "--output",
            s(out),
        ]);
    }
    assert_eq!(
        std::fs::read(&nii_i).unwrap(),
        std::fs::read(&nii_s).unwrap()
    );
}

#[test]
fn sidecar_failures_are_clear() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (_flux, bundle, unit) = solve(d, "side", "0");
    let mask_path = d.join("m.json");
    std::fs::write(
        &mask_path,
        serde_json::to_vec(&serde_json::json!({"name": "m", "voxels": vec![true; 320]})).unwrap(),
    )
    .unwrap();
    let dvh = |out: &str| {
        fails(&[
            "dvh",
            "--dose",
            s(&bundle),
            "--quantity",
            "physical_total",
            "--mask",
            s(&mask_path),
            "--output",
            s(&d.join(out)),
        ])
    };

    // Tampered sidecar: a sha256 error naming the file.
    let side = d.join("bundle-side.values.f64le");
    let original = std::fs::read(&side).unwrap();
    let mut tampered = original.clone();
    tampered[3] ^= 0x40;
    std::fs::write(&side, &tampered).unwrap();
    let err = dvh("t1.json");
    assert!(
        err.contains("sha256 mismatch") && err.contains("bundle-side.values.f64le"),
        "{err}"
    );

    // Wrong length.
    std::fs::write(&side, &original[..original.len() - 8]).unwrap();
    let err = dvh("t2.json");
    assert!(err.contains("length mismatch"), "{err}");

    // Missing.
    std::fs::remove_file(&side).unwrap();
    let err = dvh("t3.json");
    assert!(err.contains("bundle-side.values.f64le"), "{err}");

    // The unit dose is checked on its own path too.
    std::fs::write(d.join("unit-side.values.f64le"), b"short").unwrap();
    let err = fails(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&unit),
        "--blood-ug-g",
        "10",
        "--output",
        s(&d.join("t4.json")),
    ]);
    assert!(err.contains("side.values.f64le"), "{err}");
}

#[test]
fn evidence_bundle_carries_and_verifies_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let (_flux, bundle, _unit) = solve(d, "side", "0");
    let root = d.join("evidence");
    let artifact = format!("dose={}:artifacts/bundle.json", s(&bundle));
    let export = |root: &Path| {
        run(
            &[
                "evidence",
                "export",
                "--root",
                s(root),
                "--case-id",
                "sidecar",
                "--qualification",
                "synthetic_research_only",
                "--artifact",
                &artifact,
            ],
            None,
        )
    };
    let out = export(&root);
    assert!(out.status.success(), "{out:?}");

    // The document's sidecars were copied next to it and recorded.
    let manifest = raw(&root.join("artifact-manifest.json"));
    let paths: Vec<&str> = manifest["artifacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"artifacts/bundle.json"));
    assert!(
        paths.contains(&"artifacts/bundle-side.values.f64le"),
        "{paths:?}"
    );
    assert!(paths.len() > 2, "{paths:?}");
    ok(&["evidence", "verify", "--root", s(&root)]);

    // A tampered bundled sidecar breaks verification.
    let copied = root.join("artifacts/bundle-side.values.f64le");
    let mut bytes = std::fs::read(&copied).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&copied, bytes).unwrap();
    let err = fails(&["evidence", "verify", "--root", s(&root)]);
    assert!(err.contains("hash mismatch"), "{err}");

    // Export refuses a tampered source sidecar.
    let side = d.join("bundle-side.values.f64le");
    let mut bytes = std::fs::read(&side).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&side, bytes).unwrap();
    let out = export(&d.join("evidence2"));
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("sha256 mismatch"),
        "{out:?}"
    );
}
