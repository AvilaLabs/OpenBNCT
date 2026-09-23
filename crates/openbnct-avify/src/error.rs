// SPDX-License-Identifier: MIT

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AvifyError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("transport model: {0}")]
    Model(#[from] openbnct_transport::TransportModelError),
    #[error("{0}")]
    Invalid(String),
    #[error("avify engine failed (exit {code}): {stderr}")]
    EngineFailed { code: i32, stderr: String },
    #[error("avify engine did not finish within {seconds} s (timed out)")]
    Timeout { seconds: u64 },
}
