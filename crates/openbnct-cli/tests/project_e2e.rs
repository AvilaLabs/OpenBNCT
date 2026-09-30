// SPDX-License-Identifier: MIT

//! End to end: synthetic NF-BNCT-001 study -> `project init` -> `project run`
//! -> report, manifest, resume and invalidation. One provisional (2 outer
//! iterations) S4 solve; a proof that the chain executes, not a converged
//! result.

use std::path::Path;
use std::process::{Command, Output};

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

fn step_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.starts_with("[") && l.contains('/'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn init_run_resume_and_invalidate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(&["benchmark", "generate", "study"], root);
    let init = ok(
        &[
            "project",
            "init",
            "--dicom",
            "study",
            "--output",
            "p001",
            "--target",
            "CORE",
            "--spacing-mm",
            "8",
        ],
        root,
    );
    assert!(init.contains("CORE"), "{init}");
    let project = root.join("p001");
    assert!(project.join("inputs/SHA256SUMS").is_file());
    assert!(
        project
            .join("inputs/multigroup-data-28g-tsl.json")
            .is_file()
    );

    // init refuses to overwrite.
    let again = openbnct(
        &["project", "init", "--dicom", "study", "--output", "p001"],
        root,
    );
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already exists"));

    // Demo settings: two outer iterations, unconverged allowed, S4.
    let toml_path = project.join("project.toml");
    let text = std::fs::read_to_string(&toml_path).unwrap();
    let edited = text
        .replace("order = 8", "order = 4")
        .replace("max_outer = 128", "max_outer = 2")
        .replace("allow_unconverged = false", "allow_unconverged = true");
    assert_ne!(text, edited);
    std::fs::write(&toml_path, edited).unwrap();

    // First run: every step runs.
    let first = ok(&["project", "run", "p001"], root);
    let lines = step_lines(&first);
    assert_eq!(
        lines.iter().filter(|l| l.contains(" done in ")).count(),
        7,
        "{first}"
    );

    let report = std::fs::read_to_string(project.join("out/report.md")).unwrap();
    assert!(report.contains("PROVISIONAL"), "{report}");
    assert!(report.contains("## Commands"));
    assert!(report.contains("openbnct sn solve"));
    assert!(report.contains("not a medical device"));
    let report_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("out/report.json")).unwrap()).unwrap();
    assert_eq!(report_json["provisional"], true);
    assert_eq!(
        report_json["schema_version"],
        "openbnct.project-report/0.1.0"
    );
    assert!(project.join("out/dvh/CORE.csv").is_file());

    let manifest_path = project.join("out/run-manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["schema_version"], "openbnct.project-run/0.1.0");
    let steps = manifest["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 7);
    for step in steps {
        assert_eq!(step["status"], "complete");
        assert!(!step["outputs"].as_array().unwrap().is_empty());
        assert!(
            step["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| { i["sha256"].as_str().is_some_and(|s| s.len() == 64) })
        );
    }
    // Equivalent commands are recorded verbatim.
    let transport = &steps[3]["commands"][0];
    assert!(
        transport
            .as_str()
            .unwrap()
            .starts_with("openbnct sn solve --case out/03-beam/case.json"),
        "{transport}"
    );

    // Rerun: everything is skipped.
    let second = ok(&["project", "run", "p001"], root);
    let lines = step_lines(&second);
    assert_eq!(lines.len(), 7, "{second}");
    assert!(lines.iter().all(|l| l.contains("skipped")), "{second}");

    // Changing the boron concentration reruns only steps 5-7.
    let text = std::fs::read_to_string(&toml_path).unwrap();
    std::fs::write(
        &toml_path,
        text.replace("blood_ug_g = 25.0", "blood_ug_g = 30.0"),
    )
    .unwrap();
    let third = ok(&["project", "run", "p001"], root);
    let lines = step_lines(&third);
    assert_eq!(lines.len(), 7 + 3, "{third}");
    for (index, label) in ["import", "calibrate", "beam", "transport"]
        .iter()
        .enumerate()
    {
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with(&format!("[{}/7]", index + 1))
                    && l.contains(label)
                    && l.contains("skipped")),
            "{label} should be skipped:\n{third}"
        );
    }
    for label in ["boron", "metrics", "report"] {
        assert!(
            lines
                .iter()
                .any(|l| l.contains(label) && l.contains(" done in ")),
            "{label} should rerun:\n{third}"
        );
    }
    let report = std::fs::read_to_string(project.join("out/report.md")).unwrap();
    assert!(report.contains("blood 30 ug/g"), "{report}");

    // Tampering with a recorded output invalidates that step and downstream.
    std::fs::write(project.join("out/05-boron/dose.json"), b"{}").unwrap();
    let fourth = ok(&["project", "run", "p001"], root);
    let lines = step_lines(&fourth);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("transport") && l.contains("skipped")),
        "{fourth}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("boron") && l.contains(" done in ")),
        "{fourth}"
    );

    // --from reruns the named step and everything after it.
    let from = ok(&["project", "run", "p001", "--from", "metrics"], root);
    let lines = step_lines(&from);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("boron") && l.contains("skipped")),
        "{from}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("metrics") && l.contains(" done in ")),
        "{from}"
    );

    let status = ok(&["project", "status", "p001"], root);
    assert!(status.contains("04-transport"), "{status}");
    assert!(status.contains("complete"), "{status}");
}

#[test]
fn run_rejects_unknown_roi_before_transport() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(&["benchmark", "generate", "study"], root);
    ok(
        &[
            "project",
            "init",
            "--dicom",
            "study",
            "--output",
            "p001",
            "--target",
            "CORE",
            "--spacing-mm",
            "8",
        ],
        root,
    );
    let toml_path = root.join("p001/project.toml");
    let text = std::fs::read_to_string(&toml_path).unwrap();
    std::fs::write(&toml_path, text.replace("\"CORE\" = 3.5", "\"SKUN\" = 3.5")).unwrap();
    let output = openbnct(&["project", "run", "p001"], root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("SKUN"), "{stderr}");
    assert!(stderr.contains("available ROIs: PHANTOM, CORE"), "{stderr}");
    assert!(!root.join("p001/out/04-transport").exists());
}

#[test]
fn verify_requires_a_completed_run() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(&["benchmark", "generate", "study"], root);
    ok(
        &[
            "project",
            "init",
            "--dicom",
            "study",
            "--output",
            "p001",
            "--target",
            "CORE",
            "--spacing-mm",
            "8",
        ],
        root,
    );
    let output = openbnct(&["project", "verify", "p001"], root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("openbnct project run"), "{stderr}");
}

/// Real OpenMC check on the synthetic study at 8 mm with a tiny history
/// count. CI has no OpenMC, so this returns early (a clean skip) unless
/// `OPENBNCT_OPENMC` and `OPENMC_CROSS_SECTIONS` name an OpenMC 0.16.0
/// executable and an ENDF/B-VIII.1 HDF5 library with its `thermal/` tables.
/// The physics agreement itself is not asserted (the 2-outer-iteration S4
/// solve is a plumbing demonstration); the artifacts, manifest and report
/// integration are.
#[test]
fn verify_against_openmc_on_the_synthetic_study() {
    let (Some(openmc), Some(xs)) = (
        std::env::var_os("OPENBNCT_OPENMC"),
        std::env::var_os("OPENMC_CROSS_SECTIONS"),
    ) else {
        eprintln!("skipped: set OPENBNCT_OPENMC and OPENMC_CROSS_SECTIONS to run the OpenMC check");
        return;
    };
    if !Path::new(&openmc).is_file() || !Path::new(&xs).is_file() {
        eprintln!("skipped: OPENBNCT_OPENMC / OPENMC_CROSS_SECTIONS do not name files");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    ok(&["benchmark", "generate", "study"], root);
    ok(
        &[
            "project",
            "init",
            "--dicom",
            "study",
            "--output",
            "p001",
            "--target",
            "CORE",
            "--spacing-mm",
            "8",
        ],
        root,
    );
    let toml_path = root.join("p001/project.toml");
    let text = std::fs::read_to_string(&toml_path).unwrap();
    std::fs::write(
        &toml_path,
        text.replace("order = 8", "order = 4")
            .replace("max_outer = 128", "max_outer = 2")
            .replace("allow_unconverged = false", "allow_unconverged = true"),
    )
    .unwrap();
    ok(&["project", "run", "p001"], root);

    let verify_args = [
        "project",
        "verify",
        "p001",
        "--particles",
        "2e4",
        "--batches",
        "4",
        "--timeout-seconds",
        "1800",
    ];
    let out = ok(&verify_args, root);
    assert!(
        out.contains("AGREES") || out.contains("DISAGREES") || out.contains("INCONCLUSIVE"),
        "{out}"
    );
    let project = root.join("p001");
    for name in ["boron", "nitrogen", "hydrogen", "photon", "total"] {
        assert!(
            project
                .join(format!("out/08-verify/ratio-{name}.nii"))
                .is_file(),
            "ratio-{name}.nii"
        );
    }
    for name in [
        "mc-dose.json",
        "comparison.json",
        "gamma.json",
        "verification.json",
    ] {
        assert!(project.join("out/08-verify").join(name).is_file(), "{name}");
    }

    let report = std::fs::read_to_string(project.join("out/report.md")).unwrap();
    assert!(
        report.contains("## Independent Monte Carlo check"),
        "{report}"
    );
    assert!(report.contains("H1=c_H_in_H2O"), "{report}");
    assert!(report.contains("| CORE |"), "{report}");
    assert!(
        report.contains("An independent continuous-energy OpenMC check"),
        "{report}"
    );
    let report_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("out/report.json")).unwrap()).unwrap();
    assert_eq!(report_json["independent_mc_check"]["status"], "fresh");

    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(project.join("out/run-manifest.json")).unwrap())
            .unwrap();
    let steps = manifest["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 8);
    assert_eq!(steps[7]["id"], "08-verify");
    assert_eq!(steps[7]["status"], "complete");
    assert!(
        steps[7]["commands"][3]
            .as_str()
            .unwrap()
            .starts_with("openbnct openmc run --case out/08-verify/case.json")
    );

    // Neither a rerun of the pipeline nor of the verification repeats work,
    // and the report keeps the verification.
    let rerun = ok(&["project", "run", "p001"], root);
    assert!(
        step_lines(&rerun).iter().all(|l| l.contains("skipped")),
        "{rerun}"
    );
    let again = ok(&verify_args, root);
    assert!(again.contains("skipped"), "{again}");
    let report = std::fs::read_to_string(project.join("out/report.md")).unwrap();
    assert!(report.contains("| CORE |"), "{report}");
}
