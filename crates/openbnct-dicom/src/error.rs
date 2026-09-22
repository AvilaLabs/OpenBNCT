// SPDX-License-Identifier: MIT

use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DicomError>;

#[derive(Debug, Error)]
pub enum DicomError {
    #[error("no DICOM CT slices were supplied")]
    EmptySeries,
    #[error("failed to read DICOM object {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("failed to write DICOM object {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{path}: missing or invalid DICOM attribute {attribute}: {detail}")]
    Attribute {
        path: PathBuf,
        attribute: &'static str,
        detail: String,
    },
    #[error("inconsistent CT series: {0}")]
    InconsistentSeries(String),
    #[error("invalid CT geometry: {0}")]
    Geometry(String),
    #[error("invalid RT Structure Set: {0}")]
    StructureSet(String),
    #[error("NF-BNCT-001 verification failed: {0}")]
    Benchmark(String),
    #[error("study import failed: {0}")]
    StudyImport(String),
    #[error(transparent)]
    Manifest(#[from] openbnct_evidence::ManifestError),
    #[error("benchmark output path already exists: {0}")]
    OutputExists(PathBuf),
    #[error("I/O operation failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
