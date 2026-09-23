// SPDX-License-Identifier: MIT

//! Human review marker for a finished Avify run.
//!
//! A certificate on disk says the engine ran; it does not say anyone
//! looked at the answer. `review.json` records that a named reviewer
//! examined a specific certificate — content-bound to the certificate's
//! SHA-256 so a re-run under the same directory leaves the old review
//! visibly stale rather than silently attached.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::AvifyError;
use crate::receipt::hex_sha256;

pub const REVIEW_SCHEMA: &str = "openbnct.avify-review/0.1.0";

/// `openbnct.avify-review/0.1.0` — who looked, at what, when.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvifyReview {
    pub schema_version: String,
    /// SHA-256 of the `certificate.json` bytes the reviewer saw.
    pub certificate_sha256: String,
    pub reviewer: String,
    pub note: String,
    /// Unix seconds when the marker was written.
    pub created_unix_seconds: u64,
}

/// The state of `review.json` relative to the current certificate.
#[derive(Debug)]
pub enum ReviewState {
    /// No review file exists.
    Missing,
    /// A review exists and binds the certificate on disk.
    Current(AvifyReview),
    /// A review exists but the certificate has since changed — the
    /// review named different bytes than what's there now.
    Stale {
        review: AvifyReview,
        current_sha256: String,
    },
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn review_path(outdir: &Path) -> PathBuf {
    outdir.join("review.json")
}

/// Write a review marker for the certificate in `outdir`. Overwrites a
/// prior marker — reviews name bytes, so re-reviewing is explicit.
pub fn write_review(outdir: &Path, reviewer: &str, note: &str) -> Result<AvifyReview, AvifyError> {
    if reviewer.trim().is_empty() {
        return Err(AvifyError::Invalid(
            "a review needs a reviewer name — anonymous sign-off is meaningless".into(),
        ));
    }
    let certificate = outdir.join("certificate.json");
    if !certificate.is_file() {
        return Err(AvifyError::Invalid(format!(
            "no certificate.json in {} — nothing to review",
            outdir.display()
        )));
    }
    let review = AvifyReview {
        schema_version: REVIEW_SCHEMA.to_string(),
        certificate_sha256: hex_sha256(&std::fs::read(&certificate)?),
        reviewer: reviewer.to_string(),
        note: note.to_string(),
        created_unix_seconds: unix_seconds(),
    };
    let path = review_path(outdir);
    std::fs::write(&path, serde_json::to_vec_pretty(&review)?)?;
    Ok(review)
}

pub fn load_review(path: &Path) -> Result<AvifyReview, AvifyError> {
    let review: AvifyReview = serde_json::from_slice(&std::fs::read(path)?)?;
    if review.schema_version != REVIEW_SCHEMA {
        return Err(AvifyError::Invalid(format!(
            "avify-review schema {:?} not supported (expected {REVIEW_SCHEMA})",
            review.schema_version
        )));
    }
    Ok(review)
}

/// Where the on-disk review stands relative to the certificate.
pub fn review_state(outdir: &Path) -> Result<ReviewState, AvifyError> {
    let path = review_path(outdir);
    if !path.is_file() {
        return Ok(ReviewState::Missing);
    }
    let review = load_review(&path)?;
    let certificate = outdir.join("certificate.json");
    if !certificate.is_file() {
        // A review naming a certificate that's gone is stale.
        return Ok(ReviewState::Stale {
            review,
            current_sha256: String::new(),
        });
    }
    let current = hex_sha256(&std::fs::read(&certificate)?);
    if current == review.certificate_sha256 {
        Ok(ReviewState::Current(review))
    } else {
        Ok(ReviewState::Stale {
            review,
            current_sha256: current,
        })
    }
}
