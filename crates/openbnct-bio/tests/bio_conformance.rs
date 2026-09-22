// SPDX-License-Identifier: MIT

//! Public conformance suite for `openbnct.biological-model/0.2.0` →
//! `openbnct.biological-dose-bundle/0.2.0` application.
//!
//! Walks `conformance/bio/0.2.0/manifest.json`: every `expect: "ok"` case
//! must apply and equal its reference bundle under `expected/`; every
//! `expect: "reject"` case must fail with the named error token. Outputs are
//! byte-fixed because bundle provenance embeds the model document's SHA-256.
//!
//! Regenerate the reference bundles after an intentional model change with
//! `OPENBNCT_UPDATE_CONFORMANCE=1 cargo test -p openbnct-bio`.

use std::fs;
use std::path::{Path, PathBuf};

use openbnct_bio::{BioError, BiologicalDoseBundle, BiologicalModel, RegionMask};
use openbnct_core::PhysicalDoseBundle;
use serde::Deserialize;

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/bio/0.2.0")
        .canonicalize()
        .expect("bio conformance suite directory")
}

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    file: String,
    dose: Option<String>,
    #[serde(default)]
    regions: Vec<String>,
    expect: String,
    error: Option<String>,
    bundle: Option<String>,
    description: String,
}

/// Stable rejection tokens published by the manifest.
fn error_token(error: &BioError) -> &'static str {
    match error {
        BioError::UnsupportedSchema(_) => "unsupported_schema",
        BioError::Invalid(message) if message.contains("no supplied mask") => "missing_region_mask",
        BioError::Invalid(message) if message.contains("does not match") => "unit_mismatch",
        BioError::Invalid(_) => "invalid",
        BioError::UnresolvedSpectrum(_) => "unresolved_spectrum",
    }
}

#[test]
fn biological_model_conformance_suite() {
    let root = suite_dir();
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("manifest.json"))
            .expect("parse manifest");
    assert!(!manifest.cases.is_empty(), "suite carries no cases");
    let update = std::env::var_os("OPENBNCT_UPDATE_CONFORMANCE").is_some();
    let mut ok_count = 0;
    let mut reject_count = 0;
    for case in &manifest.cases {
        let model_bytes = fs::read(root.join(&case.file))
            .unwrap_or_else(|e| panic!("{}: read model: {e}", case.file));
        let model: BiologicalModel = serde_json::from_slice(&model_bytes)
            .unwrap_or_else(|e| panic!("{}: parse model: {e}", case.file));
        let dose_path = root.join(case.dose.as_deref().unwrap_or("cases/physical-bundle.json"));
        let physical: PhysicalDoseBundle = serde_json::from_slice(
            &fs::read(&dose_path).unwrap_or_else(|e| panic!("{}: read dose: {e}", case.file)),
        )
        .unwrap_or_else(|e| panic!("{}: parse dose: {e}", case.file));
        let regions: Vec<RegionMask> = case
            .regions
            .iter()
            .map(|region| {
                serde_json::from_slice(
                    &fs::read(root.join(region))
                        .unwrap_or_else(|e| panic!("{region}: read mask: {e}")),
                )
                .unwrap_or_else(|e| panic!("{region}: parse mask: {e}"))
            })
            .collect();
        match case.expect.as_str() {
            "ok" => {
                let bundle =
                    openbnct_bio::apply_biological_model(&model, &model_bytes, &physical, &regions)
                        .unwrap_or_else(|e| {
                            panic!(
                                "{}: expected apply, got {e} ({})",
                                case.file, case.description
                            )
                        });
                let expected_path =
                    root.join(case.bundle.as_deref().expect("ok case names a bundle"));
                if update {
                    fs::write(
                        &expected_path,
                        serde_json::to_string_pretty(&bundle).unwrap() + "\n",
                    )
                    .unwrap();
                } else {
                    let expected: BiologicalDoseBundle =
                        serde_json::from_slice(&fs::read(&expected_path).unwrap_or_else(|e| {
                            panic!("{}: read expected bundle: {e}", case.file)
                        }))
                        .unwrap();
                    assert_eq!(
                        bundle, expected,
                        "{}: applied bundle differs from reference",
                        case.file
                    );
                }
                ok_count += 1;
            }
            "reject" => {
                let error =
                    openbnct_bio::apply_biological_model(&model, &model_bytes, &physical, &regions)
                        .expect_err(&format!("{}: expected rejection", case.file));
                assert_eq!(
                    error_token(&error),
                    case.error.as_deref().expect("reject case names an error"),
                    "{}: wrong rejection ({error})",
                    case.file
                );
                reject_count += 1;
            }
            other => panic!("{}: unknown expect {other:?}", case.file),
        }
    }
    assert!(ok_count > 0 && reject_count > 0, "suite must exercise both");
}
