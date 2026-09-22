// SPDX-License-Identifier: MIT

//! Public conformance suite for `openbnct.component-dose-interchange/0.1.0`.
//!
//! Walks `conformance/interchange/0.1.0/manifest.json`: every `expect: "ok"`
//! case must import and equal its reference bundle under `expected/`; every
//! `expect: "reject"` case must fail with the named error token. The suite is
//! byte-fixed because bundle provenance embeds each document's SHA-256.
//!
//! Regenerate the reference bundles after an intentional importer change with
//! `OPENBNCT_UPDATE_CONFORMANCE=1 cargo test -p openbnct-core`.

use std::fs;
use std::path::{Path, PathBuf};

use openbnct_core::{
    ComponentDoseInterchange, InterchangeError, PhysicalDoseBundle, import_component_dose,
};
use serde::Deserialize;
use sha2::Digest;

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/interchange/0.1.0")
        .canonicalize()
        .expect("conformance suite directory")
}

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    file: String,
    expect: String,
    error: Option<String>,
    bundle: Option<String>,
    description: String,
}

/// Stable rejection tokens published by the manifest. External implementers
/// can rely on these names to classify failures.
fn error_token(error: &InterchangeError) -> &'static str {
    match error {
        InterchangeError::UnsupportedSchema(_) => "unsupported_schema",
        InterchangeError::EmptyIdentifier(_) => "empty_identifier",
        InterchangeError::Geometry(_) => "geometry",
        InterchangeError::NoComponents => "no_components",
        InterchangeError::DuplicateComponent(_) => "duplicate_component",
        InterchangeError::MissingComponent(_) => "missing_component",
        InterchangeError::InconsistentUnits { .. } => "inconsistent_units",
        InterchangeError::DoseLength { .. } => "dose_length",
        InterchangeError::InvalidDose(_) => "invalid_dose",
        InterchangeError::UncertaintyLength { .. } => "uncertainty_length",
        InterchangeError::InvalidUncertainty(_) => "invalid_uncertainty",
        InterchangeError::TotalLength { .. } => "total_length",
        InterchangeError::InvalidTotal => "invalid_total",
        InterchangeError::TotalUncertaintyLength { .. } => "total_uncertainty_length",
        InterchangeError::InvalidTotalUncertainty => "invalid_total_uncertainty",
        InterchangeError::InvalidReference(_) => "invalid_reference",
        InterchangeError::Bundle(_) => "bundle",
    }
}

#[test]
fn interchange_conformance_suite() {
    let root = suite_dir();
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("manifest.json"))
            .expect("parse manifest");
    assert!(!manifest.cases.is_empty(), "suite carries no cases");
    let update = std::env::var_os("OPENBNCT_UPDATE_CONFORMANCE").is_some();
    let mut ok_count = 0;
    let mut reject_count = 0;
    for case in &manifest.cases {
        let bytes = fs::read(root.join(&case.file))
            .unwrap_or_else(|e| panic!("{}: read case: {e}", case.file));
        let sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
        let document: ComponentDoseInterchange = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{}: parse document: {e}", case.file));
        match case.expect.as_str() {
            "ok" => {
                let bundle = import_component_dose(&document, &sha256).unwrap_or_else(|e| {
                    panic!(
                        "{}: expected import, got {e} ({})",
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
                    let expected: PhysicalDoseBundle =
                        serde_json::from_slice(&fs::read(&expected_path).unwrap_or_else(|e| {
                            panic!("{}: read expected bundle: {e}", case.file)
                        }))
                        .unwrap();
                    assert_eq!(
                        bundle, expected,
                        "{}: imported bundle differs from reference",
                        case.file
                    );
                }
                ok_count += 1;
            }
            "reject" => {
                let error = import_component_dose(&document, &sha256)
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
