// SPDX-License-Identifier: MIT

//! Public conformance suite for `openbnct.endpoint-model/0.1.0` →
//! `openbnct.endpoint-evaluation/0.1.0` scoring and UTCP combination.
//!
//! Walks `conformance/endpoints/0.1.0/manifest.json`: every `expect: "ok"`
//! case must evaluate and equal its reference evaluation under `expected/`;
//! every `expect: "reject"` case must fail with the named error token.
//! Outputs are byte-fixed because evaluations embed the SHA-256 of the model
//! document, and UTCP records embed the SHA-256 of their pretty-printed
//! source evaluations.
//!
//! Regenerate the reference evaluations after an intentional model change
//! with `OPENBNCT_UPDATE_CONFORMANCE=1 cargo test -p openbnct-bio`.

use std::fs;
use std::path::{Path, PathBuf};

use openbnct_bio::{BioError, EndpointEvaluation, EndpointModel, RegionMask, UtcpCombination};
use openbnct_core::ContentReference;
use serde::Deserialize;

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../conformance/endpoints/0.1.0")
        .canonicalize()
        .expect("endpoint conformance suite directory")
}

#[derive(Deserialize)]
struct Manifest {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    kind: Option<String>,
    model: Option<String>,
    tcp_model: Option<String>,
    ntcp_model: Option<String>,
    input: String,
    mask: String,
    ntcp_mask: Option<String>,
    combination: Option<String>,
    expect: String,
    error: Option<String>,
    bundle: Option<String>,
    description: String,
}

/// The self-contained dose selection an evaluation consumes.
#[derive(Deserialize)]
struct DoseInput {
    case_id: String,
    quantity: String,
    unit: String,
    voxel_volume_mm3: f64,
    values: Vec<f64>,
    dose_source: ContentReference,
}

/// Stable rejection tokens published by the manifest.
fn error_token(error: &BioError) -> &'static str {
    match error {
        BioError::UnsupportedSchema(_) => "unsupported_schema",
        BioError::Invalid(message) if message.contains("per-source-particle dose unit") => {
            "per_source_particle_unit"
        }
        BioError::Invalid(message) if message.contains("dose selection") => "dose_selection",
        BioError::Invalid(message) if message.contains("UTCP inputs disagree") => "utcp_mismatch",
        BioError::Invalid(message) if message.contains("UTCP") => "utcp_invalid",
        BioError::Invalid(_) => "invalid_model",
        BioError::UnresolvedSpectrum(_) => "unresolved_spectrum",
    }
}

fn load<T: for<'de> Deserialize<'de>>(root: &Path, file: &str) -> (T, Vec<u8>) {
    let bytes = fs::read(root.join(file)).unwrap_or_else(|e| panic!("{file}: read: {e}"));
    let parsed = serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{file}: parse: {e}"));
    (parsed, bytes)
}

fn evaluate(
    root: &Path,
    model_path: &str,
    input: &DoseInput,
    mask_path: &str,
) -> Result<(EndpointEvaluation, Vec<u8>), BioError> {
    let (model, model_bytes): (EndpointModel, Vec<u8>) = load(root, model_path);
    let (mask, _): (RegionMask, Vec<u8>) = load(root, mask_path);
    let evaluation = openbnct_bio::evaluate_endpoint(
        &model,
        &model_bytes,
        &input.case_id,
        &mask,
        &input.quantity,
        &input.unit,
        &input.values,
        input.voxel_volume_mm3,
        input.dose_source.clone(),
    )?;
    let bytes = serde_json::to_string_pretty(&evaluation)
        .unwrap()
        .into_bytes();
    Ok((evaluation, bytes))
}

#[test]
fn endpoint_model_conformance_suite() {
    let root = suite_dir();
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).expect("manifest.json"))
            .expect("parse manifest");
    assert!(!manifest.cases.is_empty(), "suite carries no cases");
    let update = std::env::var_os("OPENBNCT_UPDATE_CONFORMANCE").is_some();
    let mut ok_count = 0;
    let mut reject_count = 0;
    for case in &manifest.cases {
        let (input, _): (DoseInput, Vec<u8>) = load(&root, &case.input);
        let outcome: Result<EndpointEvaluation, BioError> = match case.kind.as_deref() {
            Some("utcp") => {
                let combination = match case.combination.as_deref() {
                    Some("p_plus") => UtcpCombination::PPlus,
                    Some("difference") => UtcpCombination::Difference,
                    other => panic!("{}: unknown combination {other:?}", case.id),
                };
                evaluate(
                    &root,
                    case.tcp_model
                        .as_deref()
                        .expect("utcp case names tcp_model"),
                    &input,
                    &case.mask,
                )
                .and_then(|(tcp, tcp_bytes)| {
                    evaluate(
                        &root,
                        case.ntcp_model
                            .as_deref()
                            .expect("utcp case names ntcp_model"),
                        &input,
                        case.ntcp_mask.as_deref().unwrap_or(&case.mask),
                    )
                    .and_then(|(ntcp, ntcp_bytes)| {
                        openbnct_bio::combine_utcp(
                            &tcp,
                            &tcp_bytes,
                            &ntcp,
                            &ntcp_bytes,
                            combination,
                        )
                    })
                })
            }
            _ => evaluate(
                &root,
                case.model.as_deref().expect("evaluate case names model"),
                &input,
                &case.mask,
            )
            .map(|(evaluation, _)| evaluation),
        };
        match case.expect.as_str() {
            "ok" => {
                let evaluation = outcome.unwrap_or_else(|e| {
                    panic!(
                        "{}: expected evaluation, got {e} ({})",
                        case.id, case.description
                    )
                });
                let expected_path =
                    root.join(case.bundle.as_deref().expect("ok case names a bundle"));
                if update {
                    fs::write(
                        &expected_path,
                        serde_json::to_string_pretty(&evaluation).unwrap() + "\n",
                    )
                    .unwrap();
                } else {
                    let expected: EndpointEvaluation =
                        serde_json::from_slice(&fs::read(&expected_path).unwrap_or_else(|e| {
                            panic!("{}: read expected evaluation: {e}", case.id)
                        }))
                        .unwrap();
                    assert_eq!(
                        evaluation, expected,
                        "{}: evaluation differs from reference",
                        case.id
                    );
                }
                ok_count += 1;
            }
            "reject" => {
                let error = outcome.expect_err(&format!("{}: expected rejection", case.id));
                assert_eq!(
                    error_token(&error),
                    case.error.as_deref().expect("reject case names an error"),
                    "{}: wrong rejection ({error})",
                    case.id
                );
                reject_count += 1;
            }
            other => panic!("{}: unknown expect {other:?}", case.id),
        }
    }
    assert!(ok_count > 0 && reject_count > 0, "suite must exercise both");
}
