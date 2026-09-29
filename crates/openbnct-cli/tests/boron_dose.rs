// SPDX-License-Identifier: MIT

//! End to end post-hoc boron: `sn solve --dose --boron-unit-output` ->
//! `boron dose` on a small synthetic slab. A uniform concentration equal to
//! the one baked into the transport material reproduces the original
//! bundle; ratio masks apply per region; mismatches are refused.

use openbnct_core::{DoseComponent, GridGeometry, PhysicalDoseBundle};
use openbnct_transport::{
    AngularDistribution, EnergyDistribution, FixedSourceDefinition, MaterialDefinition,
    MultigroupData, MultigroupMaterial, NeutronThermalTreatment, NuclideMassFraction, ParticleType,
    PlaneAxis, SourceSpatialDistribution, TransportCase,
};
use std::path::Path;
use std::process::{Command, Output};

const UNIT: f64 = 3.0e-12;
const CONC: f64 = 25.0;

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

fn fails(args: &[&str]) -> String {
    let out = openbnct(args);
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

/// One-group data whose material already carries `CONC` µg/g of boron.
fn data(with_unit: bool) -> MultigroupData {
    MultigroupData {
        schema_version: "openbnct.multigroup-data/0.1.0".into(),
        id: "mg-data".into(),
        energy_boundaries_ev: vec![1.0, 1.0e-3],
        collapse_declaration: "test fixture".into(),
        component_profile: Some(openbnct_core::ContentReference {
            id: "profile".into(),
            sha256: "a".repeat(64),
        }),
        boron_unit_response_gy_cm2_per_ug_g: with_unit.then(|| vec![UNIT]),
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
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn component(b: &PhysicalDoseBundle, c: DoseComponent) -> Vec<f64> {
    b.components
        .iter()
        .find(|x| x.component == c)
        .unwrap()
        .values
        .clone()
}

#[test]
fn posthoc_boron_reproduces_baked_in_dose_and_applies_ratios() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let case_path = d.join("case.json");
    let data_path = d.join("data.json");
    std::fs::write(&case_path, serde_json::to_vec(&case()).unwrap()).unwrap();
    std::fs::write(&data_path, serde_json::to_vec(&data(true)).unwrap()).unwrap();

    let flux = d.join("flux.json");
    let bundle = d.join("bundle.json");
    let unit = d.join("unit.json");
    ok(&[
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
    ]);
    let orig = read_bundle(&bundle);
    let orig_boron = component(&orig, DoseComponent::Boron);
    assert!(orig_boron.iter().any(|v| *v > 0.0));

    // Uniform CONC reproduces the original bundle (boron, others, total).
    let same = d.join("same.json");
    ok(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&unit),
        "--blood-ug-g",
        "25",
        "--output",
        s(&same),
    ]);
    let same = read_bundle(&same);
    for c in [
        DoseComponent::Boron,
        DoseComponent::Nitrogen,
        DoseComponent::Hydrogen,
        DoseComponent::Photon,
    ] {
        for (a, b) in component(&same, c).iter().zip(component(&orig, c)) {
            assert!((a - b).abs() <= 1e-12 * b.abs(), "{c:?}: {a} vs {b}");
        }
    }
    for (a, b) in same
        .physical_total
        .values
        .iter()
        .zip(&orig.physical_total.values)
    {
        assert!((a - b).abs() <= 1e-12 * b.abs());
    }
    assert!(same.provenance_id.contains("posthoc-boron"));

    // Ratio masks: tumor (k >= 10) at 3.5x blood, everything else default 1.0.
    let n = 4 * 4 * 20;
    let mask: Vec<bool> = (0..n).map(|v| v / 16 >= 10).collect();
    let mask_path = d.join("tumor.json");
    std::fs::write(
        &mask_path,
        serde_json::to_vec(&serde_json::json!({"name": "tumor", "voxels": mask})).unwrap(),
    )
    .unwrap();
    let mask_arg = format!("tumor={}", s(&mask_path));
    let regional = d.join("regional.json");
    ok(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&unit),
        "--blood-ug-g",
        "10",
        "--ratio",
        "tumor=3.5",
        "--mask",
        &mask_arg,
        "--output",
        s(&regional),
    ]);
    let regional = read_bundle(&regional);
    let boron = component(&regional, DoseComponent::Boron);
    for v in 0..n {
        let conc = if mask[v] { 35.0 } else { 10.0 };
        let want = orig_boron[v] * conc / CONC;
        assert!(
            (boron[v] - want).abs() <= 1e-12 * want.abs().max(1e-300),
            "voxel {v}: {} vs {want}",
            boron[v]
        );
    }

    // A ratio without a mask (and vice versa) is refused.
    let err = fails(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&unit),
        "--blood-ug-g",
        "10",
        "--ratio",
        "tumor=3.5",
        "--output",
        s(&d.join("bad1.json")),
    ]);
    assert!(err.contains("no matching --mask"), "{err}");
    let err = fails(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&unit),
        "--blood-ug-g",
        "10",
        "--mask",
        &mask_arg,
        "--output",
        s(&d.join("bad2.json")),
    ]);
    assert!(err.contains("no matching --ratio"), "{err}");

    // Grid mismatch is refused.
    let mut moved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&unit).unwrap()).unwrap();
    moved["geometry"]["origin_mm"][0] = serde_json::json!(0.5);
    let moved_path = d.join("unit-moved.json");
    std::fs::write(&moved_path, serde_json::to_vec(&moved).unwrap()).unwrap();
    let err = fails(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&moved_path),
        "--blood-ug-g",
        "25",
        "--output",
        s(&d.join("bad3.json")),
    ]);
    assert!(err.contains("geometry"), "{err}");

    // Neither --blood-ug-g nor --boron-field is refused.
    fails(&[
        "boron",
        "dose",
        "--physical-bundle",
        s(&bundle),
        "--unit-dose",
        s(&unit),
        "--output",
        s(&d.join("bad4.json")),
    ]);

    // `sn fold --boron-unit-output` gives the same artifact content.
    let fold_bundle = d.join("fold-bundle.json");
    let fold_unit = d.join("fold-unit.json");
    ok(&[
        "sn",
        "fold",
        "--case",
        s(&case_path),
        "--data",
        s(&data_path),
        "--flux",
        s(&flux),
        "--boron-unit-output",
        s(&fold_unit),
        "--output",
        s(&fold_bundle),
    ]);
    let a: serde_json::Value = serde_json::from_slice(&std::fs::read(&unit).unwrap()).unwrap();
    let b: serde_json::Value = serde_json::from_slice(&std::fs::read(&fold_unit).unwrap()).unwrap();
    assert_eq!(a["values"], b["values"]);
    assert_eq!(a["schema_version"], "openbnct.boron-unit-dose/0.1.0");
}

#[test]
fn data_without_unit_vector_is_refused_with_recollapse_hint() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let case_path = d.join("case.json");
    let data_path = d.join("data.json");
    std::fs::write(&case_path, serde_json::to_vec(&case()).unwrap()).unwrap();
    std::fs::write(&data_path, serde_json::to_vec(&data(false)).unwrap()).unwrap();
    let err = fails(&[
        "sn",
        "solve",
        "--case",
        s(&case_path),
        "--data",
        s(&data_path),
        "--periodic",
        "x,y",
        "--boron-unit-output",
        s(&d.join("unit.json")),
        "--output",
        s(&d.join("flux.json")),
    ]);
    assert!(err.contains("sn collapse"), "{err}");
}
