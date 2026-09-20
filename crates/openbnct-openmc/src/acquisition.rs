// SPDX-License-Identifier: Apache-2.0

//! Fail-closed acquisition of externally published OpenMC nuclear data.
//!
//! Acquisition proves which bytes were transferred; it does not qualify the
//! nuclear physics represented by those bytes. In particular, a SHA-256 that
//! NCTForge computes locally is not a substitute for a publisher digest.

use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(not(target_arch = "wasm32"))]
use md5::Md5;
use reqwest::Url;
#[cfg(not(target_arch = "wasm32"))]
use reqwest::blocking::{Client, Response};
#[cfg(not(target_arch = "wasm32"))]
use reqwest::header::{
    ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, ETAG,
    HeaderMap, LAST_MODIFIED, LOCATION, RANGE,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const ACQUISITION_PROFILE_SCHEMA: &str = "openbnct.data-acquisition-profile/0.2.0";
pub const ACQUISITION_RECEIPT_SCHEMA: &str = "openbnct.data-acquisition-receipt/0.1.0";

#[cfg(not(target_arch = "wasm32"))]
const MAX_REDIRECTS: usize = 10;
#[cfg(not(target_arch = "wasm32"))]
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataAcquisitionProfile {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub artifact_role: String,
    pub publication: DataPublication,
    pub artifact: PublishedDataArtifact,
    pub size_evidence: SizeEvidence,
    pub upstream_recipe: Option<UpstreamRecipe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataPublication {
    pub publisher: String,
    pub release_page_uri: String,
    pub source_uri: String,
    pub allowed_https_host_suffixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedDataArtifact {
    pub filename: String,
    pub media_type: String,
    pub expected_size_bytes: u64,
    pub expected_content_disposition_filename: Option<String>,
    pub publisher_digest: Option<PublishedDigest>,
    pub known_prior_digests: Vec<PublishedDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedDigest {
    pub algorithm: DigestAlgorithm,
    pub value: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Md5,
    Sha256,
}

impl DigestAlgorithm {
    fn hexadecimal_length(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha256 => 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SizeEvidence {
    pub method: String,
    pub observed_on: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamRecipe {
    pub repository_uri: String,
    pub commit: String,
    pub path: String,
    pub source_sha256: String,
    pub release_argument: String,
    pub processing_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataAcquisitionProfileDocument {
    pub profile: DataAcquisitionProfile,
    pub sha256: String,
}

impl DataAcquisitionProfileDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AcquisitionError> {
        let profile: DataAcquisitionProfile = serde_json::from_slice(bytes)?;
        profile.validate()?;
        Ok(Self {
            profile,
            sha256: sha256_hex(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, AcquisitionError> {
        let bytes = fs::read(path).map_err(|source| AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }
}

impl DataAcquisitionProfile {
    pub fn validate(&self) -> Result<(), AcquisitionError> {
        if !openbnct_core::schema_matches(&self.schema_version, ACQUISITION_PROFILE_SCHEMA) {
            return Err(AcquisitionError::InvalidProfile(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("artifact_role", self.artifact_role.as_str()),
            ("publisher", self.publication.publisher.as_str()),
            ("media_type", self.artifact.media_type.as_str()),
            ("size_evidence.method", self.size_evidence.method.as_str()),
            (
                "size_evidence.observed_on",
                self.size_evidence.observed_on.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(AcquisitionError::InvalidProfile(format!(
                    "{label} is empty"
                )));
            }
        }

        validate_filename("artifact.filename", &self.artifact.filename)?;
        if let Some(filename) = &self.artifact.expected_content_disposition_filename {
            validate_filename("artifact.expected_content_disposition_filename", filename)?;
        }
        if self.artifact.expected_size_bytes == 0 {
            return Err(AcquisitionError::InvalidProfile(
                "artifact.expected_size_bytes must be positive".into(),
            ));
        }

        let release_page = Url::parse(&self.publication.release_page_uri).map_err(|error| {
            AcquisitionError::InvalidProfile(format!("invalid release_page_uri: {error}"))
        })?;
        validate_publication_url(&release_page, "release_page_uri")?;
        let source = Url::parse(&self.publication.source_uri).map_err(|error| {
            AcquisitionError::InvalidProfile(format!("invalid source_uri: {error}"))
        })?;

        if self.publication.allowed_https_host_suffixes.is_empty() {
            return Err(AcquisitionError::InvalidProfile(
                "allowed_https_host_suffixes is empty".into(),
            ));
        }
        for (index, suffix) in self
            .publication
            .allowed_https_host_suffixes
            .iter()
            .enumerate()
        {
            if suffix.is_empty()
                || suffix.starts_with('.')
                || suffix.ends_with('.')
                || suffix.bytes().any(|byte| {
                    !(byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'-'))
                })
                || self.publication.allowed_https_host_suffixes[..index].contains(suffix)
            {
                return Err(AcquisitionError::InvalidProfile(format!(
                    "invalid HTTPS host suffix {suffix:?}"
                )));
            }
        }
        validate_transfer_url(&source, &self.publication.allowed_https_host_suffixes)?;

        if let Some(digest) = &self.artifact.publisher_digest {
            validate_published_digest("artifact.publisher_digest", digest)?;
        }
        for (index, digest) in self.artifact.known_prior_digests.iter().enumerate() {
            validate_published_digest("artifact.known_prior_digests", digest)?;
            if self
                .artifact
                .known_prior_digests
                .iter()
                .take(index)
                .any(|earlier| {
                    earlier.algorithm == digest.algorithm && earlier.value == digest.value
                })
                || self
                    .artifact
                    .publisher_digest
                    .as_ref()
                    .is_some_and(|current| {
                        current.algorithm == digest.algorithm && current.value == digest.value
                    })
            {
                return Err(AcquisitionError::InvalidProfile(
                    "artifact digest history contains a duplicate".into(),
                ));
            }
        }

        if let Some(recipe) = &self.upstream_recipe {
            for (label, value) in [
                (
                    "upstream_recipe.repository_uri",
                    recipe.repository_uri.as_str(),
                ),
                ("upstream_recipe.path", recipe.path.as_str()),
                (
                    "upstream_recipe.release_argument",
                    recipe.release_argument.as_str(),
                ),
                (
                    "upstream_recipe.processing_code",
                    recipe.processing_code.as_str(),
                ),
            ] {
                if value.trim().is_empty() {
                    return Err(AcquisitionError::InvalidProfile(format!(
                        "{label} is empty"
                    )));
                }
            }
            validate_lower_hex("upstream_recipe.commit", &recipe.commit, 40)?;
            validate_lower_hex("upstream_recipe.source_sha256", &recipe.source_sha256, 64)?;
            let repository = Url::parse(&recipe.repository_uri).map_err(|error| {
                AcquisitionError::InvalidProfile(format!(
                    "invalid upstream_recipe.repository_uri: {error}"
                ))
            })?;
            validate_publication_url(&repository, "upstream_recipe.repository_uri")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataAcquisitionProbe {
    pub profile_id: String,
    pub expected_filename: String,
    pub size_bytes: u64,
    pub accepts_ranges: bool,
    pub final_origin: String,
    pub content_disposition_filename: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataAcquisitionReceipt {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub profile_id: String,
    pub profile_sha256: String,
    pub artifact_role: String,
    pub artifact: AcquiredDataArtifact,
    pub transfer: DataTransferEvidence,
    pub publisher_digest_status: PublisherDigestStatus,
    pub evidence_state: AcquisitionEvidenceState,
    pub completed_at_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataAcquisitionReceiptDocument {
    pub receipt: DataAcquisitionReceipt,
    pub sha256: String,
}

impl DataAcquisitionReceiptDocument {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AcquisitionError> {
        let receipt: DataAcquisitionReceipt = serde_json::from_slice(bytes)?;
        receipt.validate()?;
        Ok(Self {
            receipt,
            sha256: sha256_hex(bytes),
        })
    }

    pub fn from_path(path: &Path) -> Result<Self, AcquisitionError> {
        let bytes = fs::read(path).map_err(|source| AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }

    pub fn validate_for_profile(
        &self,
        profile: &DataAcquisitionProfileDocument,
    ) -> Result<(), AcquisitionError> {
        profile.profile.validate()?;
        self.receipt.validate()?;
        let receipt = &self.receipt;
        let expected = &profile.profile;

        if receipt.profile_id != expected.id
            || receipt.profile_sha256 != profile.sha256
            || receipt.artifact_role != expected.artifact_role
            || receipt.artifact.path != expected.artifact.filename
            || receipt.artifact.media_type != expected.artifact.media_type
            || receipt.artifact.size_bytes != expected.artifact.expected_size_bytes
            || receipt.artifact.publisher_digest != expected.artifact.publisher_digest
            || receipt.transfer.requested_uri != expected.publication.source_uri
        {
            return Err(AcquisitionError::ReceiptProfileMismatch);
        }

        let expected_status = if expected.artifact.publisher_digest.is_some() {
            PublisherDigestStatus::Matched
        } else {
            PublisherDigestStatus::Unavailable
        };
        if receipt.publisher_digest_status != expected_status {
            return Err(AcquisitionError::ReceiptProfileMismatch);
        }
        validate_content_disposition(
            expected,
            receipt.transfer.content_disposition_filename.as_deref(),
        )?;
        let final_origin = Url::parse(&receipt.transfer.final_origin).map_err(|error| {
            AcquisitionError::InvalidReceipt(format!("invalid final origin: {error}"))
        })?;
        validate_transfer_url(
            &final_origin,
            &expected.publication.allowed_https_host_suffixes,
        )?;
        if url_origin(&final_origin)? != receipt.transfer.final_origin {
            return Err(AcquisitionError::InvalidReceipt(
                "transfer.final_origin is not a canonical HTTPS origin".into(),
            ));
        }
        Ok(())
    }
}

impl DataAcquisitionReceipt {
    pub fn validate(&self) -> Result<(), AcquisitionError> {
        if !openbnct_core::schema_matches(&self.schema_version, ACQUISITION_RECEIPT_SCHEMA) {
            return Err(AcquisitionError::InvalidReceipt(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("profile_id", self.profile_id.as_str()),
            ("artifact_role", self.artifact_role.as_str()),
            ("artifact.media_type", self.artifact.media_type.as_str()),
            (
                "transfer.requested_uri",
                self.transfer.requested_uri.as_str(),
            ),
            ("transfer.final_origin", self.transfer.final_origin.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(AcquisitionError::InvalidReceipt(format!(
                    "{label} is empty"
                )));
            }
        }
        validate_lower_hex("profile_sha256", &self.profile_sha256, 64)
            .map_err(profile_error_as_receipt)?;
        validate_filename("artifact.path", &self.artifact.path)
            .map_err(profile_error_as_receipt)?;
        if self.artifact.size_bytes == 0 {
            return Err(AcquisitionError::InvalidReceipt(
                "artifact.size_bytes must be positive".into(),
            ));
        }
        validate_lower_hex("artifact.sha256", &self.artifact.sha256, 64)
            .map_err(profile_error_as_receipt)?;
        if let Some(digest) = &self.artifact.publisher_digest {
            validate_published_digest("artifact.publisher_digest", digest)
                .map_err(profile_error_as_receipt)?;
        }
        if self.transfer.resumed_from_bytes > self.artifact.size_bytes {
            return Err(AcquisitionError::InvalidReceipt(
                "transfer resume offset exceeds artifact size".into(),
            ));
        }
        for (label, value) in [
            (
                "transfer.content_disposition_filename",
                self.transfer.content_disposition_filename.as_deref(),
            ),
            ("transfer.etag", self.transfer.etag.as_deref()),
            (
                "transfer.last_modified",
                self.transfer.last_modified.as_deref(),
            ),
        ] {
            if value.is_some_and(str::is_empty) {
                return Err(AcquisitionError::InvalidReceipt(format!(
                    "{label} is empty"
                )));
            }
        }
        if self.completed_at_unix_seconds == 0 {
            return Err(AcquisitionError::InvalidReceipt(
                "completed_at_unix_seconds must be positive".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquiredDataArtifact {
    pub path: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub publisher_digest: Option<PublishedDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataTransferEvidence {
    pub requested_uri: String,
    pub final_origin: String,
    pub resumed_from_bytes: u64,
    pub content_disposition_filename: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherDigestStatus {
    Unavailable,
    Matched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionEvidenceState {
    AcquisitionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcquisitionProgress {
    pub completed_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquiredData {
    pub artifact_path: PathBuf,
    pub receipt_path: PathBuf,
    pub receipt: DataAcquisitionReceipt,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct DataAcquisitionClient {
    client: Client,
}

#[cfg(not(target_arch = "wasm32"))]
impl DataAcquisitionClient {
    pub fn new() -> Result<Self, AcquisitionError> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("NCTForge/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .timeout(None)
            .build()?;
        Ok(Self { client })
    }

    pub fn probe(
        &self,
        document: &DataAcquisitionProfileDocument,
    ) -> Result<DataAcquisitionProbe, AcquisitionError> {
        let profile = &document.profile;
        profile.validate()?;
        let response = self.get(profile, Some("bytes=0-0"))?;
        let status = response.status();
        let headers = response.headers();
        let (size_bytes, accepts_ranges) = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let range = required_header(headers, CONTENT_RANGE.as_str())?;
            let (start, end, total) = parse_content_range(range)?;
            if start != 0 || end != 0 {
                return Err(AcquisitionError::InvalidRangeResponse(format!(
                    "probe returned bytes {start}-{end}; expected 0-0"
                )));
            }
            (total, true)
        } else if status == reqwest::StatusCode::OK {
            (
                required_header(headers, CONTENT_LENGTH.as_str())?
                    .parse()
                    .map_err(|_| {
                        AcquisitionError::InvalidHeader("invalid Content-Length".into())
                    })?,
                header_text(headers, ACCEPT_RANGES.as_str())?.as_deref() == Some("bytes"),
            )
        } else {
            return Err(AcquisitionError::UnexpectedStatus(status.as_u16()));
        };

        if size_bytes != profile.artifact.expected_size_bytes {
            return Err(AcquisitionError::SizeMismatch {
                expected: profile.artifact.expected_size_bytes,
                observed: size_bytes,
            });
        }
        let disposition = content_disposition_filename(headers)?;
        validate_content_disposition(profile, disposition.as_deref())?;

        Ok(DataAcquisitionProbe {
            profile_id: profile.id.clone(),
            expected_filename: profile.artifact.filename.clone(),
            size_bytes,
            accepts_ranges,
            final_origin: url_origin(response.url())?,
            content_disposition_filename: disposition,
            etag: header_text(headers, ETAG.as_str())?,
            last_modified: header_text(headers, LAST_MODIFIED.as_str())?,
        })
    }

    pub fn acquire(
        &self,
        document: &DataAcquisitionProfileDocument,
        output_directory: &Path,
        confirmed_size_bytes: u64,
    ) -> Result<AcquiredData, AcquisitionError> {
        self.acquire_with_progress(document, output_directory, confirmed_size_bytes, |_| {})
    }

    pub fn acquire_with_progress<F>(
        &self,
        document: &DataAcquisitionProfileDocument,
        output_directory: &Path,
        confirmed_size_bytes: u64,
        mut progress: F,
    ) -> Result<AcquiredData, AcquisitionError>
    where
        F: FnMut(AcquisitionProgress),
    {
        let profile = &document.profile;
        profile.validate()?;
        if confirmed_size_bytes != profile.artifact.expected_size_bytes {
            return Err(AcquisitionError::ConfirmationMismatch {
                expected: profile.artifact.expected_size_bytes,
                confirmed: confirmed_size_bytes,
            });
        }
        let metadata = fs::metadata(output_directory).map_err(|source| AcquisitionError::Io {
            path: output_directory.to_path_buf(),
            source,
        })?;
        if !metadata.is_dir() {
            return Err(AcquisitionError::InvalidOutputDirectory(
                output_directory.to_path_buf(),
            ));
        }

        let artifact_path = output_directory.join(&profile.artifact.filename);
        let partial_path = output_directory.join(format!("{}.part", profile.artifact.filename));
        let receipt_path =
            output_directory.join(format!("{}.acquisition.json", profile.artifact.filename));
        let receipt_partial_path = output_directory.join(format!(
            "{}.acquisition.json.part",
            profile.artifact.filename
        ));
        for path in [&artifact_path, &receipt_path, &receipt_partial_path] {
            if path.try_exists().map_err(|source| AcquisitionError::Io {
                path: path.to_path_buf(),
                source,
            })? {
                return Err(AcquisitionError::OutputExists(path.to_path_buf()));
            }
        }

        let resumed_from_bytes = partial_file_size(&partial_path)?;
        if resumed_from_bytes > profile.artifact.expected_size_bytes {
            return Err(AcquisitionError::PartialTooLarge {
                path: partial_path,
                expected: profile.artifact.expected_size_bytes,
                observed: resumed_from_bytes,
            });
        }

        let probe = self.probe(document)?;
        if resumed_from_bytes > 0
            && resumed_from_bytes < profile.artifact.expected_size_bytes
            && !probe.accepts_ranges
        {
            return Err(AcquisitionError::ResumeUnsupported);
        }

        let (mut sha256, mut md5) = hash_partial_file(
            &partial_path,
            profile
                .artifact
                .publisher_digest
                .as_ref()
                .is_some_and(|digest| digest.algorithm == DigestAlgorithm::Md5),
        )?;
        let mut completed_bytes = resumed_from_bytes;
        progress(AcquisitionProgress {
            completed_bytes,
            total_bytes: profile.artifact.expected_size_bytes,
        });
        let transfer = if completed_bytes < profile.artifact.expected_size_bytes {
            let range = (resumed_from_bytes > 0).then(|| format!("bytes={resumed_from_bytes}-"));
            let mut response = self.get(profile, range.as_deref())?;
            validate_download_response(
                &response,
                resumed_from_bytes,
                profile.artifact.expected_size_bytes,
            )?;
            let response_disposition = content_disposition_filename(response.headers())?;
            validate_content_disposition(profile, response_disposition.as_deref())?;
            let transfer = DataTransferEvidence {
                requested_uri: profile.publication.source_uri.clone(),
                final_origin: url_origin(response.url())?,
                resumed_from_bytes,
                content_disposition_filename: response_disposition,
                etag: header_text(response.headers(), ETAG.as_str())?,
                last_modified: header_text(response.headers(), LAST_MODIFIED.as_str())?,
            };
            let mut output = if resumed_from_bytes == 0 {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&partial_path)
            } else {
                OpenOptions::new().append(true).open(&partial_path)
            }
            .map_err(|source| AcquisitionError::Io {
                path: partial_path.clone(),
                source,
            })?;

            let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
            loop {
                let count = response
                    .read(&mut buffer)
                    .map_err(AcquisitionError::HttpBody)?;
                if count == 0 {
                    break;
                }
                completed_bytes = completed_bytes.checked_add(count as u64).ok_or(
                    AcquisitionError::InvalidRangeResponse("byte count overflow".into()),
                )?;
                if completed_bytes > profile.artifact.expected_size_bytes {
                    return Err(AcquisitionError::SizeMismatch {
                        expected: profile.artifact.expected_size_bytes,
                        observed: completed_bytes,
                    });
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|source| AcquisitionError::Io {
                        path: partial_path.clone(),
                        source,
                    })?;
                sha256.update(&buffer[..count]);
                if let Some(digest) = &mut md5 {
                    digest.update(&buffer[..count]);
                }
                progress(AcquisitionProgress {
                    completed_bytes,
                    total_bytes: profile.artifact.expected_size_bytes,
                });
            }
            output.sync_all().map_err(|source| AcquisitionError::Io {
                path: partial_path.clone(),
                source,
            })?;
            drop(output);
            transfer
        } else {
            DataTransferEvidence {
                requested_uri: profile.publication.source_uri.clone(),
                final_origin: probe.final_origin,
                resumed_from_bytes,
                content_disposition_filename: probe.content_disposition_filename,
                etag: probe.etag,
                last_modified: probe.last_modified,
            }
        };

        if completed_bytes != profile.artifact.expected_size_bytes {
            return Err(AcquisitionError::SizeMismatch {
                expected: profile.artifact.expected_size_bytes,
                observed: completed_bytes,
            });
        }

        let sha256 = format!("{:x}", sha256.finalize());
        let observed_md5 = md5.map(|digest| format!("{:x}", digest.finalize()));
        let publisher_digest_status = if let Some(expected) = &profile.artifact.publisher_digest {
            let observed = match expected.algorithm {
                DigestAlgorithm::Md5 => observed_md5
                    .as_deref()
                    .expect("MD5 initialized when required"),
                DigestAlgorithm::Sha256 => &sha256,
            };
            if observed != expected.value {
                return Err(AcquisitionError::PublisherDigestMismatch {
                    algorithm: expected.algorithm,
                    expected: expected.value.clone(),
                    observed: observed.to_owned(),
                });
            }
            PublisherDigestStatus::Matched
        } else {
            PublisherDigestStatus::Unavailable
        };

        let completed_at_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| AcquisitionError::ClockBeforeUnixEpoch)?
            .as_secs();
        let receipt = DataAcquisitionReceipt {
            schema_version: ACQUISITION_RECEIPT_SCHEMA.into(),
            profile_id: profile.id.clone(),
            profile_sha256: document.sha256.clone(),
            artifact_role: profile.artifact_role.clone(),
            artifact: AcquiredDataArtifact {
                path: profile.artifact.filename.clone(),
                media_type: profile.artifact.media_type.clone(),
                size_bytes: completed_bytes,
                sha256,
                publisher_digest: profile.artifact.publisher_digest.clone(),
            },
            transfer,
            publisher_digest_status,
            evidence_state: AcquisitionEvidenceState::AcquisitionOnly,
            completed_at_unix_seconds,
        };
        let mut receipt_bytes = serde_json::to_vec_pretty(&receipt)?;
        receipt_bytes.push(b'\n');
        let mut receipt_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&receipt_partial_path)
            .map_err(|source| AcquisitionError::Io {
                path: receipt_partial_path.clone(),
                source,
            })?;
        receipt_file
            .write_all(&receipt_bytes)
            .and_then(|()| receipt_file.sync_all())
            .map_err(|source| AcquisitionError::Io {
                path: receipt_partial_path.clone(),
                source,
            })?;
        drop(receipt_file);

        fs::hard_link(&partial_path, &artifact_path).map_err(|source| AcquisitionError::Io {
            path: artifact_path.clone(),
            source,
        })?;
        if let Err(source) = fs::hard_link(&receipt_partial_path, &receipt_path) {
            let _ = fs::remove_file(&artifact_path);
            return Err(AcquisitionError::Io {
                path: receipt_path,
                source,
            });
        }
        fs::remove_file(&partial_path).map_err(|source| AcquisitionError::Io {
            path: partial_path,
            source,
        })?;
        fs::remove_file(&receipt_partial_path).map_err(|source| AcquisitionError::Io {
            path: receipt_partial_path,
            source,
        })?;

        Ok(AcquiredData {
            artifact_path,
            receipt_path,
            receipt,
        })
    }

    fn get(
        &self,
        profile: &DataAcquisitionProfile,
        range: Option<&str>,
    ) -> Result<Response, AcquisitionError> {
        let mut url = Url::parse(&profile.publication.source_uri).map_err(|error| {
            AcquisitionError::InvalidProfile(format!("invalid source_uri: {error}"))
        })?;
        for redirect_count in 0..=MAX_REDIRECTS {
            validate_transfer_url(&url, &profile.publication.allowed_https_host_suffixes)?;
            let mut request = self
                .client
                .get(url.clone())
                .header(ACCEPT_ENCODING, "identity");
            if let Some(range) = range {
                request = request.header(RANGE, range);
            }
            let response = request.send()?;
            if !response.status().is_redirection() {
                return Ok(response);
            }
            if redirect_count == MAX_REDIRECTS {
                return Err(AcquisitionError::TooManyRedirects(MAX_REDIRECTS));
            }
            let location = required_header(response.headers(), LOCATION.as_str())?;
            url = url
                .join(location)
                .map_err(|error| AcquisitionError::InvalidRedirect(error.to_string()))?;
        }
        Err(AcquisitionError::TooManyRedirects(MAX_REDIRECTS))
    }
}

#[derive(Debug, Error)]
pub enum AcquisitionError {
    #[error("invalid acquisition profile: {0}")]
    InvalidProfile(String),
    #[error("invalid acquisition receipt: {0}")]
    InvalidReceipt(String),
    #[error("acquisition receipt does not match its reviewed profile")]
    ReceiptProfileMismatch,
    #[error("failed to parse or serialize acquisition JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("HTTP request failed")]
    Http(#[from] reqwest::Error),
    #[error("HTTP response body failed: {0}")]
    HttpBody(io::Error),
    #[error("unexpected HTTP status {0}")]
    UnexpectedStatus(u16),
    #[error("HTTP response header is invalid: {0}")]
    InvalidHeader(String),
    #[error("redirect target is invalid: {0}")]
    InvalidRedirect(String),
    #[error("HTTP response exceeded the {0}-redirect limit")]
    TooManyRedirects(usize),
    #[error("HTTP range response is invalid: {0}")]
    InvalidRangeResponse(String),
    #[error("artifact size mismatch: expected {expected} bytes, observed {observed}")]
    SizeMismatch { expected: u64, observed: u64 },
    #[error(
        "large-download confirmation mismatch: profile requires {expected} bytes, confirmed {confirmed}"
    )]
    ConfirmationMismatch { expected: u64, confirmed: u64 },
    #[error("cannot resume because the server did not advertise byte-range support")]
    ResumeUnsupported,
    #[error("output directory is not a directory: {0}")]
    InvalidOutputDirectory(PathBuf),
    #[error("refusing to overwrite existing acquisition output: {0}")]
    OutputExists(PathBuf),
    #[error(
        "partial artifact {path} is larger than expected: expected at most {expected}, observed {observed}"
    )]
    PartialTooLarge {
        path: PathBuf,
        expected: u64,
        observed: u64,
    },
    #[error("publisher {algorithm:?} mismatch: expected {expected}, observed {observed}")]
    PublisherDigestMismatch {
        algorithm: DigestAlgorithm,
        expected: String,
        observed: String,
    },
    #[error("system clock predates the Unix epoch")]
    ClockBeforeUnixEpoch,
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

fn profile_error_as_receipt(error: AcquisitionError) -> AcquisitionError {
    match error {
        AcquisitionError::InvalidProfile(message) => AcquisitionError::InvalidReceipt(message),
        other => other,
    }
}

fn validate_filename(label: &str, value: &str) -> Result<(), AcquisitionError> {
    let path = Path::new(value);
    let mut components = path.components();
    if value.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
        || value == "."
        || value == ".."
    {
        return Err(AcquisitionError::InvalidProfile(format!(
            "{label} must be one normalized filename"
        )));
    }
    Ok(())
}

fn validate_lower_hex(label: &str, value: &str, length: usize) -> Result<(), AcquisitionError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(AcquisitionError::InvalidProfile(format!(
            "{label} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_published_digest(
    label: &str,
    digest: &PublishedDigest,
) -> Result<(), AcquisitionError> {
    validate_lower_hex(label, &digest.value, digest.algorithm.hexadecimal_length())?;
    if digest.evidence.trim().is_empty() {
        return Err(AcquisitionError::InvalidProfile(format!(
            "{label} evidence is empty"
        )));
    }
    Ok(())
}

fn validate_publication_url(url: &Url, label: &str) -> Result<(), AcquisitionError> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AcquisitionError::InvalidProfile(format!(
            "{label} must be an HTTPS URL without credentials or a fragment"
        )));
    }
    Ok(())
}

fn validate_transfer_url(url: &Url, suffixes: &[String]) -> Result<(), AcquisitionError> {
    let host = url
        .host_str()
        .ok_or_else(|| AcquisitionError::InvalidRedirect("transfer URL has no hostname".into()))?;
    let secure = url.scheme() == "https";
    #[cfg(test)]
    let secure = secure || (url.scheme() == "http" && matches!(host, "127.0.0.1" | "localhost"));
    if !secure || !url.username().is_empty() || url.password().is_some() || url.fragment().is_some()
    {
        return Err(AcquisitionError::InvalidRedirect(
            "transfer URL must use HTTPS and contain no credentials or fragment".into(),
        ));
    }
    if !suffixes.iter().any(|suffix| host_matches(host, suffix)) {
        return Err(AcquisitionError::InvalidRedirect(format!(
            "host {host:?} is outside the profile allowlist"
        )));
    }
    Ok(())
}

fn host_matches(host: &str, suffix: &str) -> bool {
    host == suffix
        || host
            .strip_suffix(suffix)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(not(target_arch = "wasm32"))]
fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, AcquisitionError> {
    headers
        .get(name)
        .ok_or_else(|| AcquisitionError::InvalidHeader(format!("missing {name}")))?
        .to_str()
        .map_err(|_| AcquisitionError::InvalidHeader(format!("non-text {name}")))
}

#[cfg(not(target_arch = "wasm32"))]
fn header_text(headers: &HeaderMap, name: &str) -> Result<Option<String>, AcquisitionError> {
    headers
        .get(name)
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .map_err(|_| AcquisitionError::InvalidHeader(format!("non-text {name}")))
        })
        .transpose()
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_content_range(value: &str) -> Result<(u64, u64, u64), AcquisitionError> {
    let (unit, range_and_total) = value.split_once(' ').ok_or_else(|| {
        AcquisitionError::InvalidRangeResponse(format!("malformed Content-Range {value:?}"))
    })?;
    let (range, total) = range_and_total.split_once('/').ok_or_else(|| {
        AcquisitionError::InvalidRangeResponse(format!("malformed Content-Range {value:?}"))
    })?;
    let (start, end) = range.split_once('-').ok_or_else(|| {
        AcquisitionError::InvalidRangeResponse(format!("malformed Content-Range {value:?}"))
    })?;
    if unit != "bytes" {
        return Err(AcquisitionError::InvalidRangeResponse(format!(
            "unsupported Content-Range unit {unit:?}"
        )));
    }
    let start = start.parse().map_err(|_| {
        AcquisitionError::InvalidRangeResponse(format!("invalid range start in {value:?}"))
    })?;
    let end = end.parse().map_err(|_| {
        AcquisitionError::InvalidRangeResponse(format!("invalid range end in {value:?}"))
    })?;
    let total = total.parse().map_err(|_| {
        AcquisitionError::InvalidRangeResponse(format!("invalid range total in {value:?}"))
    })?;
    if start > end || end >= total {
        return Err(AcquisitionError::InvalidRangeResponse(format!(
            "inconsistent Content-Range {value:?}"
        )));
    }
    Ok((start, end, total))
}

#[cfg(not(target_arch = "wasm32"))]
fn content_disposition_filename(headers: &HeaderMap) -> Result<Option<String>, AcquisitionError> {
    let Some(value) = header_text(headers, CONTENT_DISPOSITION.as_str())? else {
        return Ok(None);
    };
    for parameter in value.split(';').skip(1) {
        let Some((name, raw_value)) = parameter.trim().split_once('=') else {
            continue;
        };
        if name.eq_ignore_ascii_case("filename") {
            let filename = raw_value.trim().trim_matches('"');
            if filename.is_empty() {
                return Err(AcquisitionError::InvalidHeader(
                    "empty Content-Disposition filename".into(),
                ));
            }
            return Ok(Some(filename.to_owned()));
        }
    }
    Ok(None)
}

fn validate_content_disposition(
    profile: &DataAcquisitionProfile,
    observed: Option<&str>,
) -> Result<(), AcquisitionError> {
    if let Some(expected) = &profile.artifact.expected_content_disposition_filename
        && observed != Some(expected.as_str())
    {
        return Err(AcquisitionError::InvalidHeader(format!(
            "Content-Disposition filename mismatch: expected {expected:?}, observed {observed:?}"
        )));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn validate_download_response(
    response: &Response,
    offset: u64,
    expected_total: u64,
) -> Result<(), AcquisitionError> {
    let expected_remaining = expected_total - offset;
    if offset == 0 {
        if response.status() != reqwest::StatusCode::OK {
            return Err(AcquisitionError::UnexpectedStatus(
                response.status().as_u16(),
            ));
        }
    } else {
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(AcquisitionError::InvalidRangeResponse(format!(
                "resume at byte {offset} returned status {}",
                response.status()
            )));
        }
        let value = required_header(response.headers(), CONTENT_RANGE.as_str())?;
        let (start, end, total) = parse_content_range(value)?;
        if start != offset || total != expected_total || end + 1 != expected_total {
            return Err(AcquisitionError::InvalidRangeResponse(format!(
                "resume returned bytes {start}-{end}/{total}; expected {offset}-{}/{expected_total}",
                expected_total - 1
            )));
        }
    }
    let content_length: u64 = required_header(response.headers(), CONTENT_LENGTH.as_str())?
        .parse()
        .map_err(|_| AcquisitionError::InvalidHeader("invalid Content-Length".into()))?;
    if content_length != expected_remaining {
        return Err(AcquisitionError::SizeMismatch {
            expected: expected_remaining,
            observed: content_length,
        });
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn partial_file_size(path: &Path) -> Result<u64, AcquisitionError> {
    let Some(metadata) = path
        .symlink_metadata()
        .map(Some)
        .or_else(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        })
        .map_err(|source| AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        })?
    else {
        return Ok(0);
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(AcquisitionError::InvalidProfile(format!(
            "partial output is not a regular, non-symlink file: {}",
            path.display()
        )));
    }
    Ok(metadata.len())
}

#[cfg(not(target_arch = "wasm32"))]
fn hash_partial_file(
    path: &Path,
    calculate_md5: bool,
) -> Result<(Sha256, Option<Md5>), AcquisitionError> {
    let mut sha256 = Sha256::new();
    let mut md5 = calculate_md5.then(Md5::new);
    let Some(_) = path
        .try_exists()
        .map_err(|source| AcquisitionError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .then_some(())
    else {
        return Ok((sha256, md5));
    };
    let mut file = File::open(path).map_err(|source| AcquisitionError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|source| AcquisitionError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        sha256.update(&buffer[..count]);
        if let Some(digest) = &mut md5 {
            digest.update(&buffer[..count]);
        }
    }
    Ok((sha256, md5))
}

fn url_origin(url: &Url) -> Result<String, AcquisitionError> {
    let host = url
        .host_str()
        .ok_or_else(|| AcquisitionError::InvalidRedirect("final URL has no host".into()))?;
    let mut origin = format!("{}://{host}", url.scheme());
    if let Some(port) = url.port() {
        origin.push(':');
        origin.push_str(&port.to_string());
    }
    Ok(origin)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::sync::mpsc::{self, Receiver};
    use std::thread::{self, JoinHandle};

    use super::*;

    struct TestServer {
        source_uri: String,
        requests: Receiver<String>,
        thread: JoinHandle<()>,
    }

    impl TestServer {
        fn finish(self) -> Vec<String> {
            self.thread.join().expect("test HTTP server");
            self.requests.try_iter().collect()
        }
    }

    fn serve(responses: Vec<Vec<u8>>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test HTTP server");
        let address = listener.local_addr().expect("test HTTP address");
        let (sender, requests) = mpsc::channel();
        let thread = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept test HTTP request");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).expect("read test HTTP request");
                    assert!(count > 0, "request ended before its headers");
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() < 32 * 1024, "test request headers too large");
                }
                sender
                    .send(String::from_utf8(request).expect("ASCII test HTTP request"))
                    .expect("record test HTTP request");
                stream.write_all(&response).expect("write test response");
            }
        });
        TestServer {
            source_uri: format!("http://{address}/artifact.bin"),
            requests,
            thread,
        }
    }

    fn http_response(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut response = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for (name, value) in headers {
            response.push_str(name);
            response.push_str(": ");
            response.push_str(value);
            response.push_str("\r\n");
        }
        response.push_str("\r\n");
        let mut bytes = response.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    fn probe_response(total: u64) -> Vec<u8> {
        http_response(
            "206 Partial Content",
            &[
                ("Content-Length", "1".into()),
                ("Content-Range", format!("bytes 0-0/{total}")),
                ("Accept-Ranges", "bytes".into()),
                ("ETag", "\"test-etag\"".into()),
            ],
            b"a",
        )
    }

    fn full_response(body: &[u8]) -> Vec<u8> {
        http_response(
            "200 OK",
            &[("Content-Length", body.len().to_string())],
            body,
        )
    }

    fn partial_response(body: &[u8], offset: usize, status: &str) -> Vec<u8> {
        http_response(
            status,
            &[
                ("Content-Length", (body.len() - offset).to_string()),
                (
                    "Content-Range",
                    format!(
                        "bytes {offset}-{}/{total}",
                        body.len() - 1,
                        total = body.len()
                    ),
                ),
            ],
            &body[offset..],
        )
    }

    fn document(
        source_uri: String,
        body: &[u8],
        digest: Option<PublishedDigest>,
    ) -> DataAcquisitionProfileDocument {
        let profile = DataAcquisitionProfile {
            schema_version: ACQUISITION_PROFILE_SCHEMA.into(),
            id: "test-profile".into(),
            artifact_role: "test_artifact".into(),
            publication: DataPublication {
                publisher: "test publisher".into(),
                release_page_uri: "https://example.invalid/release".into(),
                source_uri,
                allowed_https_host_suffixes: vec!["127.0.0.1".into()],
            },
            artifact: PublishedDataArtifact {
                filename: "artifact.bin".into(),
                media_type: "application/octet-stream".into(),
                expected_size_bytes: body.len() as u64,
                expected_content_disposition_filename: None,
                publisher_digest: digest,
                known_prior_digests: Vec::new(),
            },
            size_evidence: SizeEvidence {
                method: "test_range_probe".into(),
                observed_on: "2026-08-31".into(),
            },
            upstream_recipe: None,
        };
        let bytes = serde_json::to_vec(&profile).expect("serialize test profile");
        DataAcquisitionProfileDocument::from_bytes(&bytes).expect("valid test profile")
    }

    fn md5_digest(body: &[u8]) -> PublishedDigest {
        PublishedDigest {
            algorithm: DigestAlgorithm::Md5,
            value: format!("{:x}", Md5::digest(body)),
            evidence: "test publisher digest".into(),
        }
    }

    #[test]
    fn checked_in_profiles_are_valid_and_pin_the_researched_sizes() {
        let library = DataAcquisitionProfileDocument::from_bytes(include_bytes!(
            "../../../profiles/openmc/openmc-endfb81-official-library.json"
        ))
        .expect("official library profile");
        assert_eq!(library.profile.artifact.expected_size_bytes, 9_661_406_540);
        assert_eq!(
            library.sha256,
            crate::data::TARGET_ACQUISITION_PROFILE_SHA256
        );
        assert!(library.profile.artifact.publisher_digest.is_none());

        let evaluations = DataAcquisitionProfileDocument::from_bytes(include_bytes!(
            "../../../profiles/openmc/endfb81-neutron-evaluations.json"
        ))
        .expect("neutron evaluations profile");
        assert_eq!(
            evaluations.profile.artifact.expected_size_bytes,
            343_724_780
        );
        assert_eq!(
            evaluations
                .profile
                .artifact
                .publisher_digest
                .as_ref()
                .expect("published neutron digest")
                .algorithm,
            DigestAlgorithm::Md5
        );
        assert_eq!(
            evaluations
                .profile
                .artifact
                .known_prior_digests
                .first()
                .expect("prior OpenMC recipe digest")
                .value,
            "dc622c0f1c3c4477433e698266e0fc80"
        );
    }

    #[test]
    fn probe_retains_no_body_and_reports_range_capability() {
        let body = b"published data";
        let server = serve(vec![probe_response(body.len() as u64)]);
        let document = document(server.source_uri.clone(), body, None);

        let probe = DataAcquisitionClient::new()
            .expect("client")
            .probe(&document)
            .expect("probe");

        assert_eq!(probe.size_bytes, body.len() as u64);
        assert!(probe.accepts_ranges);
        assert_eq!(probe.etag.as_deref(), Some("\"test-etag\""));
        let requests = server.finish();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("range: bytes=0-0")
        );
    }

    #[test]
    fn acquisition_checks_publisher_digest_and_writes_exclusive_receipt() {
        let body = b"complete evaluated nuclear data";
        let server = serve(vec![probe_response(body.len() as u64), full_response(body)]);
        let document = document(server.source_uri.clone(), body, Some(md5_digest(body)));
        let output = tempfile::tempdir().expect("output directory");

        let acquired = DataAcquisitionClient::new()
            .expect("client")
            .acquire(&document, output.path(), body.len() as u64)
            .expect("acquisition");

        assert_eq!(fs::read(&acquired.artifact_path).expect("artifact"), body);
        assert_eq!(acquired.receipt.artifact.sha256, sha256_hex(body));
        assert_eq!(
            acquired.receipt.publisher_digest_status,
            PublisherDigestStatus::Matched
        );
        assert_eq!(
            acquired.receipt.evidence_state,
            AcquisitionEvidenceState::AcquisitionOnly
        );
        let on_disk: DataAcquisitionReceipt =
            serde_json::from_slice(&fs::read(&acquired.receipt_path).expect("receipt"))
                .expect("receipt JSON");
        assert_eq!(on_disk, acquired.receipt);
        assert!(!output.path().join("artifact.bin.part").exists());
        assert!(matches!(
            DataAcquisitionClient::new().expect("client").acquire(
                &document,
                output.path(),
                body.len() as u64
            ),
            Err(AcquisitionError::OutputExists(path)) if path == acquired.artifact_path
        ));

        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(!requests[1].to_ascii_lowercase().contains("\r\nrange:"));
    }

    #[test]
    fn acquisition_resumes_only_the_exact_remaining_range() {
        let body = b"resume this exact artifact";
        let offset = 7;
        let server = serve(vec![
            probe_response(body.len() as u64),
            partial_response(body, offset, "206 Partial Content"),
        ]);
        let document = document(server.source_uri.clone(), body, None);
        let output = tempfile::tempdir().expect("output directory");
        fs::write(output.path().join("artifact.bin.part"), &body[..offset])
            .expect("partial artifact");

        let acquired = DataAcquisitionClient::new()
            .expect("client")
            .acquire(&document, output.path(), body.len() as u64)
            .expect("resumed acquisition");

        assert_eq!(fs::read(acquired.artifact_path).expect("artifact"), body);
        assert_eq!(acquired.receipt.transfer.resumed_from_bytes, offset as u64);
        assert_eq!(
            acquired.receipt.publisher_digest_status,
            PublisherDigestStatus::Unavailable
        );
        let requests = server.finish();
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains(&format!("range: bytes={offset}-"))
        );
    }

    #[test]
    fn complete_partial_is_rehashed_and_published_without_invalid_range_request() {
        let body = b"transfer finished before receipt publication";
        let server = serve(vec![probe_response(body.len() as u64)]);
        let document = document(server.source_uri.clone(), body, Some(md5_digest(body)));
        let output = tempfile::tempdir().expect("output directory");
        fs::write(output.path().join("artifact.bin.part"), body)
            .expect("complete partial artifact");

        let acquired = DataAcquisitionClient::new()
            .expect("client")
            .acquire(&document, output.path(), body.len() as u64)
            .expect("complete partial finalization");

        assert_eq!(fs::read(acquired.artifact_path).expect("artifact"), body);
        assert_eq!(
            acquired.receipt.transfer.resumed_from_bytes,
            body.len() as u64
        );
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn bad_size_and_bad_digest_leave_no_completed_artifact() {
        let body = b"wrong bytes";
        let size_server = serve(vec![probe_response(body.len() as u64 + 1)]);
        let size_document = document(size_server.source_uri.clone(), body, None);
        assert!(matches!(
            DataAcquisitionClient::new()
                .expect("client")
                .probe(&size_document),
            Err(AcquisitionError::SizeMismatch { .. })
        ));
        size_server.finish();

        let digest_server = serve(vec![probe_response(body.len() as u64), full_response(body)]);
        let mut wrong_digest = md5_digest(body);
        wrong_digest.value = "0".repeat(32);
        let digest_document = document(digest_server.source_uri.clone(), body, Some(wrong_digest));
        let output = tempfile::tempdir().expect("output directory");
        assert!(matches!(
            DataAcquisitionClient::new().expect("client").acquire(
                &digest_document,
                output.path(),
                body.len() as u64
            ),
            Err(AcquisitionError::PublisherDigestMismatch { .. })
        ));
        assert!(!output.path().join("artifact.bin").exists());
        assert!(!output.path().join("artifact.bin.acquisition.json").exists());
        assert_eq!(
            fs::read(output.path().join("artifact.bin.part")).expect("forensic partial"),
            body
        );
        digest_server.finish();
    }

    #[test]
    fn resume_rejects_a_server_that_ignores_the_range() {
        let body = b"range must be honored";
        let offset = 5;
        let server = serve(vec![probe_response(body.len() as u64), full_response(body)]);
        let document = document(server.source_uri.clone(), body, None);
        let output = tempfile::tempdir().expect("output directory");
        fs::write(output.path().join("artifact.bin.part"), &body[..offset])
            .expect("partial artifact");

        assert!(matches!(
            DataAcquisitionClient::new().expect("client").acquire(
                &document,
                output.path(),
                body.len() as u64
            ),
            Err(AcquisitionError::InvalidRangeResponse(_))
        ));
        assert_eq!(
            fs::read(output.path().join("artifact.bin.part")).expect("unchanged partial"),
            &body[..offset]
        );
        server.finish();
    }

    #[test]
    fn redirect_outside_allowlist_is_rejected_before_following() {
        let body = b"redirected";
        let server = serve(vec![http_response(
            "302 Found",
            &[
                ("Content-Length", "0".into()),
                ("Location", "https://example.com/untrusted".into()),
            ],
            b"",
        )]);
        let document = document(server.source_uri.clone(), body, None);

        assert!(matches!(
            DataAcquisitionClient::new()
                .expect("client")
                .probe(&document),
            Err(AcquisitionError::InvalidRedirect(_))
        ));
        server.finish();
    }
}
