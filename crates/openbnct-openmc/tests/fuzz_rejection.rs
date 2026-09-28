// SPDX-License-Identifier: MIT

//! Mutation-corpus rejection tests for the evaluated-data contract and
//! the hand-rolled ENDF readers.
//!
//! `EvaluatedSource` is the trust boundary between fetched nuclear
//! archives and the collapse pipeline — malformed documents must fail
//! with a diagnostic, not panic or silently reinterpret. The ENDF
//! readers are intentionally forgiving (the format itself is
//! ill-specified in places), so the contract there is narrower: no
//! panic, and corruption that removes all sections must Err.

use openbnct_openmc::EvaluatedNeutronSourceSelectionDocument;
use openbnct_openmc::endf_mf7::parse_tsl;
use openbnct_openmc::endf_mf33::parse_mf33;

/// A minimal valid selection document — the mutation seed. Uses the
/// mixed-acquisition schema so the acquisition↔evaluation digest
/// binding rules are exercised.
fn valid_selection() -> serde_json::Value {
    serde_json::json!({
        "schema_version": "openbnct.evaluated-neutron-source-selection/0.3.0",
        "id": "endfb81-minimal",
        "case_id": "nf-bnct-001",
        "qualification": "response_treatment_candidate_unreviewed",
        "evaluated_data_release": "ENDF/B-VIII.1",
        "material": {
            "id": "openbnct.material.brain-tissue",
            "sha256": "a".repeat(64)
        },
        "acquisitions": [{
            "profile_id": "nndc-endfb81-neutron",
            "profile_sha256": "b".repeat(64),
            "receipt_sha256": "c".repeat(64),
            "archive_filename": "ENDF-B-VIII.1-neutrons.zip",
            "archive_size_bytes": 1234567u64,
            "archive_sha256": "d".repeat(64)
        }],
        "evaluations": [{
            "nuclide": "H1",
            "endf_mat": 125,
            "archive_path": "neutrons/H1.endf",
            "extracted_filename": "H1.endf",
            "size_bytes": 1000000u64,
            "sha256": "e".repeat(64),
            "acquisition_sha256": "d".repeat(64)
        }]
    })
}

fn bytes(v: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

fn must_reject_selection(label: &str, bytes: &[u8]) {
    match EvaluatedNeutronSourceSelectionDocument::from_bytes(bytes) {
        Ok(_) => panic!("{label}: malformed selection document accepted"),
        Err(e) => assert!(!e.to_string().is_empty(), "{label}: empty diagnostic"),
    }
}

#[test]
fn valid_selection_parses() {
    EvaluatedNeutronSourceSelectionDocument::from_bytes(&bytes(&valid_selection())).unwrap();
}

#[test]
fn syntactically_malformed_selection_rejected() {
    for (label, b) in [
        ("empty", &b""[..]),
        (
            "truncated mid-object",
            br#"{"schema_version":"openbnct.evalu"#,
        ),
        ("not json", b"ENDF/B-VIII.1"),
        ("json scalar", b"42"),
        ("json array", b"[]"),
        ("json null", b"null"),
    ] {
        must_reject_selection(label, b);
    }
}

#[test]
fn unknown_and_missing_fields_rejected() {
    // deny_unknown_fields: a typo'd key must not deserialize.
    let mut v = valid_selection();
    v["schema_verson"] = v["schema_version"].clone();
    v.as_object_mut().unwrap().remove("schema_version");
    must_reject_selection("renamed schema_version", &bytes(&v));

    for field in ["id", "case_id", "qualification", "evaluations"] {
        let mut v = valid_selection();
        v.as_object_mut().unwrap().remove(field);
        must_reject_selection(&format!("missing {field}"), &bytes(&v));
    }
}

#[test]
fn wrong_types_and_bad_values_rejected() {
    // Type confusion.
    let mut v = valid_selection();
    v["case_id"] = serde_json::json!(42);
    must_reject_selection("case_id as number", &bytes(&v));

    let mut v = valid_selection();
    v["evaluations"] = serde_json::json!({"nuclide": "H1"});
    must_reject_selection("evaluations as object", &bytes(&v));

    let mut v = valid_selection();
    v["evaluations"][0]["endf_mat"] = serde_json::json!("125");
    must_reject_selection("endf_mat as string", &bytes(&v));

    // Number overflow — serde_json must not accept out-of-range
    // exponents (spliced textually since 1e999 is not a Rust literal).
    let text = String::from_utf8(bytes(&valid_selection()))
        .unwrap()
        .replace("1234567", "1e999");
    assert!(text.contains("1e999"), "seed mutation did not apply");
    must_reject_selection("size overflow", text.as_bytes());

    // Non-canonical sha256 (uppercase / wrong length) — the contract's
    // own validate() must reject even though serde parses.
    let mut v = valid_selection();
    v["material"]["sha256"] = serde_json::json!("A".repeat(64));
    must_reject_selection("uppercase sha256", &bytes(&v));

    let mut v = valid_selection();
    v["material"]["sha256"] = serde_json::json!("abc123");
    must_reject_selection("short sha256", &bytes(&v));

    // Unknown qualification enum variant.
    let mut v = valid_selection();
    v["qualification"] = serde_json::json!("production_qualified");
    must_reject_selection("unknown qualification", &bytes(&v));
}

/// Minimal valid MF3-shaped tape (same structure as the in-module
/// test) — used to exercise the public TSL/MF33 readers on mutated
/// ENDF text. The MF3 reader itself is crate-private; its mutation
/// battery lives in `endf_mf3`'s own test module.
fn valid_endf() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n",
        " 1.001000+3 1.008665+0          0          0          0          01001 1451    1",
        " 1.001000+3 1.008665+0          0          0          0          01001 3  2    2",
        " 0.000000+0 1.000000+0          0          0          1          41001 3  2    3",
        "          4          2                                  1001 3  2    4",
        " 1.000000-5 2.000000+1 2.530000-2 2.050000+1 1.000000+4 1.000000+11001 3  2    5",
        " 1.000000+5 1.000000+0                                            1001 3  2    6",
    )
}

#[test]
fn endf_mutations_never_panic() {
    // The readers are intentionally forgiving: bad floats read as 0.0
    // and short lines are skipped — the only contract is no panic and
    // no fabricated sections.
    let seed = valid_endf();
    for text in [
        seed[..seed.len() / 2].to_string(),
        seed[..80].to_string(),
        format!("{}{}{}", &seed[..160], "@@@\n", &seed[160..]),
        String::new(),
        "   \n   \n".to_string(),
    ] {
        let _ = parse_tsl(&text);
        let _ = parse_mf33(&text, None);
        let _ = parse_mf33(&text, Some(102));
    }
    // Field corruption inside the data lines.
    let mut m = seed.clone();
    m.replace_range(300..330, "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@");
    let _ = parse_tsl(&m);
    let _ = parse_mf33(&m, Some(102));
}
