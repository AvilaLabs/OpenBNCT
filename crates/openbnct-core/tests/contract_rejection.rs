// SPDX-License-Identifier: MIT

//! Contract-boundary rejection corpus: every interchange artifact is
//! SHA-256 content-bound and schema-versioned, so malformed documents
//! — bad digests, unknown fields, type confusion, non-finite numbers —
//! must fail deterministically with a diagnostic, never panic and
//! never silently reinterpret. This is the cheap end of the
//! provenance-infrastructure audit: an artifact that deserializes
//! wrongly would contaminate every downstream comparison.

use openbnct_core::{ContentReference, schema_matches};

#[test]
fn content_reference_digest_canonicality() {
    let ok = ContentReference {
        id: "openbnct.test.artifact".into(),
        sha256: "ab".repeat(32),
    };
    ok.validate().unwrap();

    for (label, sha) in [
        ("uppercase", "AB".repeat(32)),
        ("short", "ab".repeat(16)),
        ("long", "ab".repeat(40)),
        ("non-hex", "zz".repeat(32)),
        ("empty", String::new()),
        ("prefixed", format!("0x{}", "ab".repeat(31))),
    ] {
        let r = ContentReference {
            id: "x".into(),
            sha256: sha.clone(),
        };
        assert!(
            r.validate().is_err(),
            "{label}: non-canonical sha256 accepted: {sha:?}"
        );
    }

    for (label, id) in [("empty", ""), ("whitespace", "   ")] {
        let r = ContentReference {
            id: id.into(),
            sha256: "ab".repeat(32),
        };
        assert!(r.validate().is_err(), "{label}: empty id accepted");
    }
}

#[test]
fn schema_namespace_normalization() {
    // The legacy `nctforge.` namespace still reads; an unknown
    // namespace must not silently match.
    assert!(schema_matches(
        "nctforge.multigroup-flux",
        "openbnct.multigroup-flux"
    ));
    assert!(schema_matches(
        "openbnct.multigroup-flux",
        "nctforge.multigroup-flux"
    ));
    assert!(!schema_matches(
        "othercode.multigroup-flux",
        "openbnct.multigroup-flux"
    ));
    assert!(!schema_matches("", "openbnct.multigroup-flux"));
    assert!(!schema_matches(
        "openbnct.multigroup-flux2",
        "openbnct.multigroup-flux"
    ));
}

#[test]
fn contract_structs_reject_unknown_fields() {
    // deny_unknown_fields on the shared contracts: a renamed key must
    // fail loudly rather than deserialize into a wrong-shaped record.
    let json = r#"{"id": "a", "sha256": "ab", "typo_field": 1}"#;
    assert!(serde_json::from_str::<ContentReference>(json).is_err());
}

#[test]
fn json_number_extremes_rejected_or_finite() {
    // JSON cannot carry NaN/Inf literally; exponent overflow must
    // error out, not silently become infinity inside a contract.
    assert!(serde_json::from_str::<f64>("1e999").is_err());
    assert!(serde_json::from_str::<f64>("-1e999").is_err());
    // Bare words are not numbers.
    assert!(serde_json::from_str::<f64>("NaN").is_err());
    assert!(serde_json::from_str::<f64>("Infinity").is_err());
}
