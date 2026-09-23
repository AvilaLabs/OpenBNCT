// SPDX-License-Identifier: MIT

//! `openbnct.avify-run/0.1.0` — the connector-side run receipt.
//!
//! The engine's certificate binds *its own* inputs by hash
//! (plan/arrays/meta sha256). It does not know which OpenBNCT artifacts
//! the connector exported them from, which engine build ran, or whether
//! the inputs have since changed. This receipt closes that gap: every
//! `verify` writes one next to the certificate, and `status` recomputes
//! the recorded hashes to report whether a returned certificate still
//! corresponds to the current inputs.
//!
//! Staleness is a connector-side advisory — it says the bound inputs
//! changed, never that the certificate's numbers are wrong.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::AvifyError;

pub const RUN_SCHEMA: &str = "openbnct.avify-run/0.1.0";

/// One hash-bound input to a verify run. `path` is recorded as passed
/// (may be relative to the caller's cwd at run time).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundInput {
    pub path: PathBuf,
    pub sha256: String,
}

/// Per-input staleness verdict from [`check_staleness`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputState {
    /// File exists and its sha256 matches the receipt.
    Current,
    /// File exists but content changed since the run.
    Changed(String),
    /// The recorded path no longer resolves to a readable file.
    Missing,
}

/// The receipt written next to `certificate.json` on a successful run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvifyRunReceipt {
    pub schema_version: String,
    /// Unix seconds at run start.
    pub created_unix_seconds: u64,
    /// OpenBNCT-side inputs the export was bound to.
    pub inputs: BTreeMap<String, BoundInput>,
    /// Exported artifacts (the engine hashes these into the
    /// certificate itself; recorded here for the binding chain).
    pub exported: BTreeMap<String, BoundInput>,
    /// How the engine was invoked, and its self-reported version.
    pub engine: EngineRecord,
    /// The returned certificate.
    pub certificate: BoundInput,
    /// Total connector wall time for the engine call, seconds.
    pub engine_elapsed_s: f64,
    /// Measured wall-time breakdown (added post-0.1.0 — absent on
    /// older receipts).
    #[serde(default)]
    pub timing: TimingRecord,
    /// Host resources at run start.
    #[serde(default)]
    pub resources: ResourceRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineRecord {
    pub argv0: Vec<String>,
    /// First line of `<argv0> --version`, or "unknown" if the engine
    /// did not answer (pre-versioning builds still run).
    pub version: String,
    /// OpenMC threads passed through to the engine, if any.
    #[serde(default)]
    pub threads: Option<u32>,
    /// The connector's wall bound on the engine call.
    #[serde(default)]
    pub timeout_s: u64,
}

/// Wall-time breakdown — preparation, the engine call, and the
/// binding/reporting tail. Every entry is measured by the connector;
/// nothing is estimated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimingRecord {
    /// Case/assignment/spec load + voxel-plan export + plan JSON write.
    #[serde(default)]
    pub export_s: f64,
    /// The bounded engine subprocess.
    #[serde(default)]
    pub engine_s: f64,
    /// Hash binding, receipt write, certificate parse.
    #[serde(default)]
    pub bind_s: f64,
    /// Connector-side total (≈ export + engine + bind).
    #[serde(default)]
    pub total_s: f64,
}

/// The host's parallelism at run start — the resource-allocation
/// record R12-05 asks for.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceRecord {
    #[serde(default)]
    pub available_parallelism: Option<usize>,
}

impl AvifyRunReceipt {
    pub fn write(&self, path: &Path) -> Result<(), AvifyError> {
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, AvifyError> {
        let receipt: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if receipt.schema_version != RUN_SCHEMA {
            return Err(AvifyError::Invalid(format!(
                "avify-run schema {:?} not supported (expected {RUN_SCHEMA})",
                receipt.schema_version
            )));
        }
        Ok(receipt)
    }
}

/// Recompute each recorded input hash against the filesystem. Paths are
/// resolved relative to `base` (the receipt's directory at call time)
/// when not absolute. Returns `path → state` for every bound input —
/// originals and exported artifacts alike.
pub fn check_staleness(receipt: &AvifyRunReceipt, base: &Path) -> BTreeMap<String, InputState> {
    let mut out = BTreeMap::new();
    for (label, bound) in receipt.inputs.iter().chain(&receipt.exported) {
        let path = if bound.path.is_absolute() {
            bound.path.clone()
        } else {
            base.join(&bound.path)
        };
        let state = match std::fs::read(&path) {
            Ok(bytes) => {
                let digest = hex_sha256(&bytes);
                if digest == bound.sha256 {
                    InputState::Current
                } else {
                    InputState::Changed(digest)
                }
            }
            Err(_) => InputState::Missing,
        };
        out.insert(label.clone(), state);
    }
    out
}

/// A receipt is stale if any bound input changed or went missing.
pub fn is_stale(states: &BTreeMap<String, InputState>) -> bool {
    states.values().any(|s| *s != InputState::Current)
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Bind a set of named files: `name → path`, hashing each now.
pub fn bind_inputs(entries: &[(&str, &Path)]) -> Result<BTreeMap<String, BoundInput>, AvifyError> {
    entries
        .iter()
        .map(|(name, path)| {
            Ok((
                (*name).to_string(),
                BoundInput {
                    path: (*path).to_path_buf(),
                    sha256: hex_sha256(&std::fs::read(path)?),
                },
            ))
        })
        .collect()
}

/// Build a receipt for a completed verify run.
pub fn receipt_for_run(
    inputs: BTreeMap<String, BoundInput>,
    exported: BTreeMap<String, BoundInput>,
    engine: EngineRecord,
    certificate: BoundInput,
    engine_elapsed_s: f64,
) -> AvifyRunReceipt {
    AvifyRunReceipt {
        schema_version: RUN_SCHEMA.to_string(),
        created_unix_seconds: unix_seconds(),
        inputs,
        exported,
        engine,
        certificate,
        engine_elapsed_s,
        timing: TimingRecord {
            engine_s: engine_elapsed_s,
            ..TimingRecord::default()
        },
        resources: ResourceRecord {
            available_parallelism: std::thread::available_parallelism().map(|n| n.get()).ok(),
        },
    }
}
