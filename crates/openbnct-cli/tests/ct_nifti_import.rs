// SPDX-License-Identifier: MIT

//! Synthetic NIfTI CT (HU + bitmask labelmap) -> `import ct-nifti`, then
//! `project init --ct-nifti` -> `project run` at tiny settings. A proof that
//! the NIfTI entry point reaches the same chain as DICOM, not a result.

use std::path::Path;
use std::process::{Command, Output};

use openbnct_core::GridGeometry;
use openbnct_nifti::{DT_FLOAT64, NiftiImage, write_nifti};

fn openbnct(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_openbnct"))
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn openbnct")
}

fn ok(args: &[&str], cwd: &Path) -> String {
    let output = openbnct(args, cwd);
    assert!(
        output.status.success(),
        "openbnct {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn geometry(origin_mm: [f64; 3]) -> GridGeometry {
    GridGeometry {
        shape: [32, 32, 32],
        spacing_mm: [5.0; 3],
        origin_mm,
        direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    }
}

fn image(geometry: GridGeometry, values: Vec<f64>) -> NiftiImage {
    NiftiImage {
        geometry,
        values,
        datatype: DT_FLOAT64,
        transform_source: "sform",
        description: "synthetic".into(),
        intent_name: String::new(),
        units_declared_mm: true,
    }
}

/// A 160 mm cube: air outside a 60 mm radius "head" of 40 HU, with a 20 mm
/// radius core. Labels are a bitmask (HEAD = 1, CORE = 2) so CORE lies
/// inside HEAD.
fn write_phantom(dir: &Path, labels_origin: [f64; 3]) {
    let grid = geometry([-77.5, -77.5, -77.5]);
    let n = 32_usize;
    let mut hu = vec![-1000.0; n * n * n];
    let mut labels = vec![0.0; n * n * n];
    for k in 0..n {
        for j in 0..n {
            for i in 0..n {
                let at = |a: usize| -77.5 + 5.0 * a as f64;
                let r = (at(i).powi(2) + at(j).powi(2) + at(k).powi(2)).sqrt();
                let index = i + n * (j + n * k);
                if r <= 60.0 {
                    hu[index] = 40.0;
                    labels[index] = 1.0;
                }
                if r <= 20.0 {
                    labels[index] = 3.0;
                }
            }
        }
    }
    write_nifti(&image(grid, hu), &dir.join("hu.nii")).unwrap();
    write_nifti(
        &image(geometry(labels_origin), labels),
        &dir.join("labels.nii.gz"),
    )
    .unwrap();
    std::fs::write(
        dir.join("names.json"),
        r#"{"encoding": "bitmask", "labels": {"1": "HEAD", "2": "CORE"}}"#,
    )
    .unwrap();
}

#[test]
fn ct_nifti_import_masks_and_error_paths() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let void = repo.join("benchmarks/synthetic/layered-head-phantom/materials/void.json");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_phantom(root, [-77.5; 3]);

    let s = |name: &str| root.join(name).to_str().unwrap().to_owned();
    let base_args = |extra: &[&str]| -> Vec<String> {
        let mut args: Vec<String> = [
            "import",
            "ct-nifti",
            "--hu",
            &s("hu.nii"),
            "--spacing-mm",
            "10",
            "--case-id",
            "test.ct-nifti.v1",
            "--base-material",
            void.to_str().unwrap(),
        ]
        .map(str::to_owned)
        .to_vec();
        args.extend(extra.iter().map(|a| (*a).to_owned()));
        args
    };
    let run = |args: Vec<String>| {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        openbnct(&refs, root)
    };

    let good = run(base_args(&[
        "--labels",
        &s("labels.nii.gz"),
        "--label-names",
        &s("names.json"),
        "--case-output",
        &s("case.json"),
        "--hu-output",
        &s("hu-out.nii"),
        "--masks-dir",
        &s("masks"),
    ]));
    assert!(
        good.status.success(),
        "{}",
        String::from_utf8_lossy(&good.stderr)
    );
    let stdout = String::from_utf8_lossy(&good.stdout);
    assert!(stdout.contains("[32, 32, 32] @ [5.0, 5.0, 5.0] mm -> case grid [16, 16, 16]"));
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("masks/index.json")).unwrap()).unwrap();
    let masks = index["masks"].as_array().unwrap();
    assert_eq!(masks.len(), 2);
    let count = |name: &str| {
        masks
            .iter()
            .find(|m| m["name"] == name)
            .unwrap_or_else(|| panic!("{name}"))["grid_voxels"]
            .as_u64()
            .unwrap()
    };
    // CORE (overlapping HEAD) is a strict subset; both are non-empty.
    assert!(count("CORE") > 0 && count("CORE") < count("HEAD"));
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("case.import-record.json")).unwrap())
            .unwrap();
    assert_eq!(record["schema_version"], "openbnct.ct-import-record/0.1.0");
    assert_eq!(record["source_format"], "nifti");
    assert_eq!(record["inputs"].as_array().unwrap().len(), 3);

    // Outputs are never overwritten.
    let again = run(base_args(&[
        "--case-output",
        &s("case.json"),
        "--hu-output",
        &s("hu-out2.nii"),
    ]));
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));

    // --labels needs --label-names; a labelmap on another grid is refused.
    let half = run(base_args(&[
        "--labels",
        &s("labels.nii.gz"),
        "--case-output",
        &s("c2.json"),
        "--hu-output",
        &s("h2.nii"),
    ]));
    assert!(!half.status.success());
    assert!(String::from_utf8_lossy(&half.stderr).contains("together"));

    write_phantom(root, [-72.5, -77.5, -77.5]);
    let shifted = run(base_args(&[
        "--labels",
        &s("labels.nii.gz"),
        "--label-names",
        &s("names.json"),
        "--case-output",
        &s("c3.json"),
        "--hu-output",
        &s("h3.nii"),
    ]));
    assert!(!shifted.status.success());
    assert!(
        String::from_utf8_lossy(&shifted.stderr).contains("does not match the HU volume"),
        "{}",
        String::from_utf8_lossy(&shifted.stderr)
    );
}

#[test]
fn ct_nifti_project_init_and_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_phantom(root, [-77.5; 3]);

    let init = ok(
        &[
            "project",
            "init",
            "--ct-nifti",
            "hu.nii",
            "--labels-nifti",
            "labels.nii.gz",
            "--label-names",
            "names.json",
            "--output",
            "p",
            "--target",
            "CORE",
            "--spacing-mm",
            "10",
        ],
        root,
    );
    assert!(init.contains("CORE") && init.contains("HEAD"), "{init}");
    let project = root.join("p");
    let toml_path = project.join("project.toml");
    let text = std::fs::read_to_string(&toml_path).unwrap();
    assert!(text.contains("ct_nifti = \"../hu.nii\""), "{text}");
    assert!(!text.contains("\ndicom ="), "{text}");
    // Tiny provisional settings, as in the DICOM end-to-end test.
    let edited = text
        .replace("order = 8", "order = 4")
        .replace("max_outer = 128", "max_outer = 2")
        .replace("allow_unconverged = false", "allow_unconverged = true");
    assert_ne!(text, edited);
    std::fs::write(&toml_path, edited).unwrap();

    let first = ok(&["project", "run", "p"], root);
    assert_eq!(
        first.lines().filter(|l| l.contains(" done in ")).count(),
        7,
        "{first}"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("out/run-manifest.json")).unwrap())
            .unwrap();
    let import = &manifest["steps"][0]["commands"][0];
    assert!(
        import
            .as_str()
            .unwrap()
            .starts_with("openbnct import ct-nifti --hu ../hu.nii"),
        "{import}"
    );
    let report = std::fs::read_to_string(project.join("out/report.md")).unwrap();
    assert!(
        report.contains("CORE") && report.contains("HEAD"),
        "{report}"
    );

    // Rerun skips every step, including the NIfTI import.
    let second = ok(&["project", "run", "p"], root);
    assert_eq!(
        second.lines().filter(|l| l.contains("skipped")).count(),
        7,
        "{second}"
    );

    // A project.toml naming both a DICOM study and a NIfTI is refused.
    let mixed = std::fs::read_to_string(&toml_path)
        .unwrap()
        .replace("[imaging]\n", "[imaging]\ndicom = \"../study\"\n");
    std::fs::write(&toml_path, mixed).unwrap();
    let refused = openbnct(&["project", "run", "p"], root);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("[imaging]"));
}
