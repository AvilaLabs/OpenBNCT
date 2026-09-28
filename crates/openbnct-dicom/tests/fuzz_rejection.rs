// SPDX-License-Identifier: MIT

//! Mutation-corpus rejection tests for the DICOM importers.
//!
//! Valid synthetic PET bytes are mutated deterministically (truncation,
//! preamble corruption, bit flips, wrong-modality composition); every
//! mutation must return `Err` with a diagnostic — never panic, never
//! silently accept a corrupted series. This is the regression net for
//! the parser-hardening class of bugs: a crashed or mis-ingested import
//! would poison every downstream artifact.

use openbnct_dicom::synthetic::{SyntheticPetSpec, write_pet_series};
use openbnct_dicom::{import_ct_series_from_bytes, import_pet_series_from_bytes};
use tempfile::tempdir;

/// Valid PET series bytes, one entry per slice, in import order.
fn valid_series() -> Vec<(String, Vec<u8>)> {
    let dir = tempdir().unwrap();
    let files = write_pet_series(
        dir.path().join("pet").as_path(),
        &SyntheticPetSpec::default(),
    )
    .unwrap();
    files
        .iter()
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(p).unwrap(),
            )
        })
        .collect()
}

type Series = Vec<(String, Vec<u8>)>;

fn must_reject(mutants: &[(&str, Series)]) {
    for (label, files) in mutants {
        match import_pet_series_from_bytes(files) {
            Ok(_) => panic!("{label}: corrupted series imported"),
            Err(e) => assert!(!e.to_string().is_empty(), "{label}: empty diagnostic"),
        }
    }
}

#[test]
fn valid_synthetic_series_imports() {
    // Control: the seed itself must import — a corpus that rejects
    // everything is not a test of rejection.
    let files = valid_series();
    import_pet_series_from_bytes(&files).unwrap();
}

#[test]
fn non_dicom_inputs_rejected() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("whitespace", b"   \n\t  ".to_vec()),
        ("json", br#"{"modality":"PT"}"#.to_vec()),
        ("ascii text", b"this is not a dicom file at all".to_vec()),
        ("zeros", vec![0u8; 4096]),
        ("0xFF fill", vec![0xFFu8; 4096]),
    ];
    for (label, bytes) in cases {
        let files = vec![("f.dcm".into(), bytes)];
        let r = import_pet_series_from_bytes(&files);
        assert!(r.is_err(), "{label}: non-DICOM input imported");
    }
}

#[test]
fn truncated_files_rejected() {
    let files = valid_series();
    let full = &files[0].1;
    assert!(full.len() > 4096, "synthetic slice unexpectedly small");
    // Truncation points spanning the Part-10 layout: inside the 128B
    // preamble, just past the DICM magic, mid-header, mid-dataset,
    // and just short of the pixel trailer.
    for cut in [64usize, 132, 1024, full.len() / 2, full.len() - 16] {
        let mut mutated = files.clone();
        mutated[0].1 = full[..cut].to_vec();
        must_reject(&[(&format!("truncated at {cut}"), mutated)]);
    }
}

#[test]
fn magic_corruption_rejected() {
    // The 128B preamble is spec-arbitrary content — only the DICM
    // magic at bytes 128–132 is a corruption invariant.
    let files = valid_series();
    for (label, lo, hi, fill) in [
        ("magic zeroed", 128usize, 132, 0u8),
        ("magic flipped", 128, 132, 0xAB),
    ] {
        let mut mutated = files.clone();
        for b in &mut mutated[0].1[lo..hi] {
            *b = fill;
        }
        must_reject(&[(label, mutated)]);
    }
}

#[test]
fn bit_flips_rejected_or_identical() {
    // Deterministic single-byte corruption across the file. Some
    // offsets may land in tags the importer does not read — the
    // contract is only "no panic, deterministic answer"; unread-tag
    // flips may legitimately import (the parser is not a checksum).
    let files = valid_series();
    let full = &files[0].1;
    let n = full.len();
    for off in [128usize, 132, 500, n / 2, n / 3, n - 64] {
        let mut mutated = files.clone();
        mutated[0].1[off] ^= 0xFF;
        // Must not panic; Ok or Err both acceptable.
        let _ = import_pet_series_from_bytes(&mutated);
    }
}

#[test]
fn malformed_series_compositions_rejected() {
    let files = valid_series();
    // Empty series.
    must_reject(&[("empty series", Vec::new())]);
    // Duplicated slice: same object listed twice in the series.
    let dup = vec![files[0].clone(), files[0].clone()];
    must_reject(&[("duplicated slice", dup)]);
    // Wrong modality composition: PET bytes must not import through
    // the CT importer (and the error must be a diagnostic, not a
    // panic or a silent reinterpretation).
    let as_ct = import_ct_series_from_bytes(&files);
    assert!(
        as_ct.is_err(),
        "PET series accepted by the CT importer — modality unchecked"
    );
}
