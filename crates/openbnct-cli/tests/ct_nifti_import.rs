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
            "--crop",
            "none",
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
    assert_eq!(record["crop"]["mode"], "none");
    assert_eq!(record["crop"]["original_ct_voxels"], 32 * 32 * 32);

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
    assert!(text.contains("\ncrop = \"body\""), "{text}");
    assert!(text.contains("\ncrop_margin_mm = 15.0"), "{text}");
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
    assert!(
        import
            .as_str()
            .unwrap()
            .contains("--crop body --crop-margin-mm 15"),
        "{import}"
    );
    // The default crop shrank the CT: sphere r = 60 mm + 15 mm margin.
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.join("out/01-import/case.import-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["crop"]["mode"], "body");
    assert!(record["crop"]["cropped_ct_voxels"].as_u64().unwrap() < 32 * 32 * 32);
    assert_eq!(record["crop"]["parameters"]["margin_mm"], 15.0);
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

    // crop = "none" is honored and validated.
    let none = std::fs::read_to_string(&toml_path)
        .unwrap()
        .replace("crop = \"body\"", "crop = \"none\"");
    std::fs::write(&toml_path, &none).unwrap();
    let third = ok(&["project", "run", "p"], root);
    assert!(third.contains(" done in "), "{third}");
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(project.join("out/01-import/case.import-record.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["crop"]["mode"], "none");
    let bad = none.replace("crop = \"none\"", "crop = \"head\"");
    std::fs::write(&toml_path, &bad).unwrap();
    let refused = openbnct(&["project", "run", "p"], root);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("crop must be"));
    std::fs::write(&toml_path, &none).unwrap();

    // A project.toml naming both a DICOM study and a NIfTI is refused.
    let mixed = std::fs::read_to_string(&toml_path)
        .unwrap()
        .replace("[imaging]\n", "[imaging]\ndicom = \"../study\"\n");
    std::fs::write(&toml_path, mixed).unwrap();
    let refused = openbnct(&["project", "run", "p"], root);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("[imaging]"));
}

/// 40 x 40 x 30 CT at 5 mm: a 16 x 16 x 16-voxel tissue block (with an air
/// pocket, to exercise hole filling) plus a disconnected "couch" slab at the
/// bottom, and a labelmap whose one ROI spans the block and the couch.
fn write_body_and_couch(dir: &Path) {
    let shape = [40_usize, 40, 30];
    let grid = GridGeometry {
        shape: [40, 40, 30],
        spacing_mm: [5.0; 3],
        origin_mm: [-100.0, -100.0, 0.0],
        direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };
    let mut hu = vec![-1000.0; shape.iter().product()];
    let mut labels = vec![0.0; hu.len()];
    for k in 0..shape[2] {
        for j in 0..shape[1] {
            for i in 0..shape[0] {
                let at = i + shape[0] * (j + shape[1] * k);
                if (12..28).contains(&i) && (12..28).contains(&j) && (8..24).contains(&k) {
                    hu[at] = 40.0;
                    labels[at] = 1.0;
                    if (18..22).contains(&i) && (18..22).contains(&j) {
                        hu[at] = -900.0;
                    }
                }
                if k < 2 && (2..38).contains(&j) {
                    hu[at] = 300.0;
                    labels[at] = 1.0;
                }
            }
        }
    }
    write_nifti(&image(grid.clone(), hu), &dir.join("bc-hu.nii")).unwrap();
    write_nifti(&image(grid, labels), &dir.join("bc-labels.nii")).unwrap();
    std::fs::write(dir.join("bc-names.json"), r#"{"1": "TISSUE"}"#).unwrap();
}

fn import_bc(root: &Path, tag: &str, extra: &[&str]) -> (Output, serde_json::Value) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let void = repo.join("benchmarks/synthetic/layered-head-phantom/materials/void.json");
    let name = |suffix: &str| {
        root.join(format!("{tag}{suffix}"))
            .to_str()
            .unwrap()
            .to_owned()
    };
    let hu = root.join("bc-hu.nii");
    let labels = root.join("bc-labels.nii");
    let names = root.join("bc-names.json");
    let mut args: Vec<String> = [
        "import",
        "ct-nifti",
        "--hu",
        hu.to_str().unwrap(),
        "--labels",
        labels.to_str().unwrap(),
        "--label-names",
        names.to_str().unwrap(),
        "--spacing-mm",
        "5",
        "--case-id",
        "test.crop.v1",
        "--base-material",
        void.to_str().unwrap(),
        "--case-output",
        &name("-case.json"),
        "--hu-output",
        &name("-hu.nii"),
        "--masks-dir",
        &name("-masks"),
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend(extra.iter().map(|a| (*a).to_owned()));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = openbnct(&refs, root);
    let record = std::fs::read(root.join(format!("{tag}-case.import-record.json")))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(serde_json::Value::Null);
    (output, record)
}

#[test]
fn body_crop_keeps_body_drops_couch_and_reports_roi_clipping() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_body_and_couch(root);

    // Default: body + 15 mm. The block is voxels 12..28 -> 9..31 (22 voxels
    // per in-plane axis) and 5..27 in z (22); the couch (k < 2) is dropped.
    let (out, record) = import_bc(root, "a", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let crop = &record["crop"];
    assert_eq!(crop["mode"], "body");
    assert_eq!(crop["original_ct_shape"], serde_json::json!([40, 40, 30]));
    assert_eq!(crop["cropped_ct_shape"], serde_json::json!([22, 22, 22]));
    assert_eq!(crop["original_ct_voxels"], 48_000);
    assert_eq!(crop["cropped_ct_voxels"], 22 * 22 * 22);
    assert_eq!(crop["parameters"]["margin_mm"], 15.0);
    assert!(
        crop["rule"]
            .as_str()
            .unwrap()
            .contains("largest 26-connected")
    );
    assert_eq!(crop["cropped_extent"]["min_lps_mm"][0], -57.5);
    // The ROI included the couch: the crop dropped exactly those voxels.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ROI \"TISSUE\" is clipped by the crop"),
        "{stderr}"
    );
    let couch = 36 * 2 * 40;
    assert!(
        stderr.contains(&format!("{couch} of {}", 16 * 16 * 16 + couch)),
        "{stderr}"
    );
    assert_eq!(crop["roi_clipping"][0]["dropped_by_crop"], couch);
    let index: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("a-masks/index.json")).unwrap()).unwrap();
    assert_eq!(index["masks"][0]["ct_voxels"], 16 * 16 * 16);
    assert_eq!(index["masks"][0]["ct_voxels_dropped_by_crop"], couch);
    // The mask lives on the cropped case grid (22^3 at 5 mm).
    let case: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("a-case.json")).unwrap()).unwrap();
    assert_eq!(case["geometry"]["shape"], serde_json::json!([22, 22, 22]));

    // Margin: 0 mm is the tight box; 16 mm rounds out to 4 voxels.
    let (_, tight) = import_bc(root, "b", &["--crop-margin-mm", "0"]);
    assert_eq!(
        tight["crop"]["cropped_ct_shape"],
        serde_json::json!([16, 16, 16])
    );
    let (_, wide) = import_bc(root, "c", &["--crop-margin-mm", "16"]);
    assert_eq!(
        wide["crop"]["cropped_ct_shape"],
        serde_json::json!([24, 24, 24])
    );

    // No crop keeps everything (and no warning).
    let (out, none) = import_bc(root, "d", &["--crop", "none"]);
    assert_eq!(none["crop"]["mode"], "none");
    assert_eq!(none["crop"]["cropped_ct_voxels"], 48_000);
    assert!(!String::from_utf8_lossy(&out.stderr).contains("clipped"));

    // Explicit box: x centers in [-40, 0] -> 9 voxels; the rest as asked.
    let (_, boxed) = import_bc(root, "e", &["--crop-box-mm", "-40,0,-40,0,20,60"]);
    assert_eq!(boxed["crop"]["mode"], "box");
    assert_eq!(
        boxed["crop"]["cropped_ct_shape"],
        serde_json::json!([9, 9, 9])
    );
    assert_eq!(
        boxed["crop"]["cropped_extent"]["min_lps_mm"],
        serde_json::json!([-42.5, -42.5, 17.5])
    );

    // Superior cut: z >= 60 mm is k >= 12; the body box starts there.
    let (_, cut) = import_bc(root, "f", &["--crop-superior-of-mm", "60"]);
    assert_eq!(
        cut["crop"]["cropped_ct_shape"],
        serde_json::json!([22, 22, 15])
    );
    assert_eq!(cut["crop"]["parameters"]["superior_of_mm"], 60.0);
    assert_eq!(cut["crop"]["cropped_extent"]["min_lps_mm"][2], 57.5);

    // Conflicts and bad boxes fail closed.
    let (bad, _) = import_bc(root, "g", &["--crop", "none", "--crop-superior-of-mm", "0"]);
    assert!(!bad.status.success());
    let (bad, _) = import_bc(root, "h", &["--crop-box-mm", "1000,2000,0,1,0,1"]);
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("no CT voxel center"));
}

#[test]
fn beam_bind_warns_when_the_entry_face_has_no_air_gap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_body_and_couch(root);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let beam = repo.join("beams/fir1-k63-ineel-20mev.json");
    let bind = |tag: &str, extra: &[&str]| {
        let (out, _) = import_bc(root, tag, extra);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let name = |suffix: &str| {
            root.join(format!("{tag}{suffix}"))
                .to_str()
                .unwrap()
                .to_owned()
        };
        openbnct(
            &[
                "beam",
                "bind",
                "--beam",
                beam.to_str().unwrap(),
                "--case",
                &name("-case.json"),
                "--output",
                &name("-bound.json"),
                "--hu",
                &name("-hu.nii"),
            ],
            root,
        )
    };
    // A 30 mm margin (face 140 mm wide for the 70 mm port radius) leaves air
    // in front of the low-z entry face.
    let with_air = bind(
        "p",
        &["--crop-margin-mm", "30", "--crop-superior-of-mm", "30"],
    );
    let stdout = String::from_utf8_lossy(&with_air.stdout);
    let stderr = String::from_utf8_lossy(&with_air.stderr);
    assert!(with_air.status.success(), "{stderr}");
    assert!(
        stdout.contains("air gap at the beam entry"),
        "{stdout}{stderr}"
    );
    assert!(!stderr.contains("no air gap"), "{stderr}");
    // A box cut tight on the body in z puts the entry layer in tissue.
    let tight = bind("q", &["--crop-box-mm", "-100,100,-100,100,40,115"]);
    let stderr = String::from_utf8_lossy(&tight.stderr);
    assert!(tight.status.success(), "{stderr}");
    assert!(
        stderr.contains("no air gap in front of the skin"),
        "{stderr}"
    );
}
