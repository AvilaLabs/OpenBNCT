// SPDX-License-Identifier: MIT

//! IAEA phase-space beam source, end to end on a tiny slab: `beam
//! phsp-info` / `beam phsp-bin` write a table and a bound case, and `sn
//! solve` on that case reproduces the analytic disk beam's dose up to the
//! known normalization (the lattice fills the 4 mm x 4 mm face, the disk
//! spreads one particle over pi r^2).

use openbnct_core::{GridGeometry, PhysicalDoseBundle};
use openbnct_transport::{
    AngularDistribution, EnergyDistribution, FixedSourceDefinition, MaterialDefinition,
    MultigroupData, MultigroupMaterial, NeutronThermalTreatment, NuclideMassFraction, PHSP_NEUTRON,
    PHSP_PHOTON, ParticleType, PhspRecord, PlaneAxis, SourceSpatialDistribution, TransportCase,
    write_iaea_phsp,
};
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

const RADIUS_CM: f64 = 0.3;

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
                radius_cm: RADIUS_CM,
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
        boron_unit_response_gy_cm2_per_ug_g: None,
        materials: vec![MultigroupMaterial {
            material_id: "absorber".into(),
            sigma_total_per_cm: vec![0.3],
            scatter_matrix_per_cm: vec![0.1],
            scatter_p1_matrix_per_cm: None,
            scatter_legendre_moments_per_cm: None,
            dose_response_gy_cm2: [
                ("boron".to_string(), vec![3.0e-12]),
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

fn total(path: &Path) -> Vec<f64> {
    let bundle: PhysicalDoseBundle = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    bundle.physical_total.values
}

#[test]
fn phase_space_lattice_reproduces_the_disk_beam_dose() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // 20 x 20 neutrons over the 4 mm x 4 mm face, plus a photon and a
    // backward neutron that must be reported, not used.
    let mut records = Vec::new();
    for i in 0..20 {
        for j in 0..20 {
            records.push(PhspRecord {
                particle_type: PHSP_NEUTRON,
                new_history: true,
                energy_mev: 2.53e-8,
                position_cm: [
                    -0.2 + 0.01 + 0.02 * f64::from(i),
                    -0.2 + 0.01 + 0.02 * f64::from(j),
                    0.0,
                ],
                direction: [0.0, 0.0, 1.0],
                weight: 1.0,
            });
        }
    }
    records.push(PhspRecord {
        particle_type: PHSP_PHOTON,
        new_history: true,
        energy_mev: 1.0,
        position_cm: [0.0, 0.0, 0.0],
        direction: [0.0, 0.0, 1.0],
        weight: 1.0,
    });
    records.push(PhspRecord {
        particle_type: PHSP_NEUTRON,
        new_history: true,
        energy_mev: 2.53e-8,
        position_cm: [0.0, 0.0, 0.0],
        direction: [0.0, 0.0, -1.0],
        weight: 1.0,
    });
    let header = d.join("lattice.IAEAheader");
    write_iaea_phsp(&header, &records, 402.0, "test lattice").unwrap();

    let case_path = d.join("case.json");
    let data_path = d.join("data.json");
    std::fs::write(&case_path, serde_json::to_vec(&case()).unwrap()).unwrap();
    std::fs::write(&data_path, serde_json::to_vec(&data()).unwrap()).unwrap();

    let info = ok(&["beam", "phsp-info", "--header", s(&header)]);
    assert!(info.contains("402 records"), "{info}");
    assert!(info.contains("photon"), "{info}");

    let table = d.join("table.json");
    let ps_case = d.join("ps-case.json");
    let bin = ok(&[
        "beam",
        "phsp-bin",
        "--header",
        s(&header),
        "--case",
        s(&case_path),
        "--data",
        s(&data_path),
        "--plane",
        "+z",
        "--pixel-mm",
        "1",
        "--dir-bins",
        "1x1",
        "--output",
        s(&table),
        "--case-output",
        s(&ps_case),
    ]);
    assert!(bin.contains("accepted 400"), "{bin}");
    assert!(bin.contains("photons: 1 records"), "{bin}");
    assert!(bin.contains("backward 1"), "{bin}");

    // Solve both beams; the phase-space one carries 1/pi r^2 -> 1/face-area.
    let (disk_flux, disk_dose) = (d.join("disk-flux.json"), d.join("disk-dose.json"));
    let (ps_flux, ps_dose) = (d.join("ps-flux.json"), d.join("ps-dose.json"));
    for (case, flux, dose) in [
        (&case_path, &disk_flux, &disk_dose),
        (&ps_case, &ps_flux, &ps_dose),
    ] {
        ok(&[
            "sn",
            "solve",
            "--case",
            s(case),
            "--data",
            s(&data_path),
            "--periodic",
            "x,y",
            "--dose",
            s(dose),
            "--output",
            s(flux),
        ]);
    }
    let a = total(&disk_dose);
    let b = total(&ps_dose);
    assert_eq!(a.len(), b.len());
    let scale = std::f64::consts::PI * RADIUS_CM * RADIUS_CM / 0.16;
    for (x, y) in a.iter().zip(&b) {
        assert!(*x > 0.0);
        assert!(
            (y / x - scale).abs() < 1e-6 * scale,
            "phase-space {y} vs disk {x} x {scale}"
        );
    }

    // Refusals: a malformed bin spec, and never overwriting an output.
    let err = fails(&[
        "beam",
        "phsp-bin",
        "--header",
        s(&header),
        "--case",
        s(&case_path),
        "--data",
        s(&data_path),
        "--dir-bins",
        "eight",
        "--output",
        s(&d.join("x.json")),
    ]);
    assert!(err.contains("dir-bins"), "{err}");
    let err = fails(&[
        "beam",
        "phsp-bin",
        "--header",
        s(&header),
        "--case",
        s(&case_path),
        "--data",
        s(&data_path),
        "--pixel-mm",
        "1",
        "--dir-bins",
        "1x1",
        "--output",
        s(&table),
    ]);
    assert!(err.contains("exist"), "{err}");
}
