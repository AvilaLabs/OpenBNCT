// SPDX-License-Identifier: MIT

//! Certificate comparison — what changed between two engine runs.
//! Reports per-ROI interval shifts and action transitions verbatim
//! from the two certificates; nothing is recomputed or rescaled.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use crate::certificate::AvifyCertificate;
use crate::error::AvifyError;
use crate::receipt::AvifyRunReceipt;

/// One ROI's change between runs.
#[derive(Debug, Clone, Serialize)]
pub struct RoiChange {
    /// `[L-3sL, U+3sU]` certified interval, then and now.
    pub certified_before: [f64; 2],
    pub certified_after: [f64; 2],
    /// Nominal dose, then and now (absent fields stay absent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nominal_before: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nominal_after: Option<f64>,
    pub action_before: String,
    pub action_after: String,
    /// True when the engine's verdict vocabulary changed.
    pub action_changed: bool,
}

/// Which named inputs differ between two receipts.
#[derive(Debug, Clone, Serialize)]
pub struct InputChange {
    pub name: String,
    /// `current sha256 → other sha256` (truncated to 16 chars).
    pub sha256_before: String,
    pub sha256_after: String,
}

/// The comparison record.
#[derive(Debug, Clone, Serialize)]
pub struct AvifyRunDiff {
    pub roi_changes: BTreeMap<String, RoiChange>,
    /// ROIs present in only one certificate.
    pub only_before: Vec<String>,
    pub only_after: Vec<String>,
    /// Inputs/artifacts whose bound hashes differ between the runs.
    pub input_changes: Vec<InputChange>,
    /// Engine versions, then and now.
    pub engine_before: String,
    pub engine_after: String,
    /// Wall time, then and now (receipt totals when present).
    pub elapsed_before_s: f64,
    pub elapsed_after_s: f64,
}

fn short(hash: &str) -> String {
    hash.chars().take(16).collect()
}

/// Compare two run directories or receipts. Each `run` may point at an
/// `avify-run.json` or at a directory containing `avify-run.json` +
/// `certificate.json`.
pub fn diff_runs(before: &Path, after: &Path) -> Result<AvifyRunDiff, AvifyError> {
    let resolve = |p: &Path| -> Result<(AvifyRunReceipt, AvifyCertificate), AvifyError> {
        let receipt_path = if p.is_dir() {
            p.join("avify-run.json")
        } else {
            p.to_path_buf()
        };
        let receipt = AvifyRunReceipt::load(&receipt_path)?;
        let cert = AvifyCertificate::load(&receipt.certificate.path)?;
        Ok((receipt, cert))
    };
    let (r0, c0) = resolve(before)?;
    let (r1, c1) = resolve(after)?;

    let mut roi_changes = BTreeMap::new();
    let mut only_before = Vec::new();
    let mut only_after = Vec::new();
    for (roi, a0) in &c0.actions {
        match c1.actions.get(roi) {
            Some(a1) => {
                roi_changes.insert(
                    roi.clone(),
                    RoiChange {
                        certified_before: a0.certified_gyw,
                        certified_after: a1.certified_gyw,
                        nominal_before: a0.nominal_gyw,
                        nominal_after: a1.nominal_gyw,
                        action_before: a0.action.clone(),
                        action_after: a1.action.clone(),
                        action_changed: a0.action != a1.action,
                    },
                );
            }
            None => only_before.push(roi.clone()),
        }
    }
    for roi in c1.actions.keys() {
        if !c0.actions.contains_key(roi) {
            only_after.push(roi.clone());
        }
    }

    let mut input_changes = Vec::new();
    let names: Vec<_> = r0
        .inputs
        .keys()
        .chain(r0.exported.keys())
        .chain(r1.inputs.keys())
        .chain(r1.exported.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for name in names {
        let h0 = r0
            .inputs
            .get(&name)
            .or_else(|| r0.exported.get(&name))
            .map(|b| b.sha256.as_str());
        let h1 = r1
            .inputs
            .get(&name)
            .or_else(|| r1.exported.get(&name))
            .map(|b| b.sha256.as_str());
        if h0 != h1 {
            input_changes.push(InputChange {
                name,
                sha256_before: h0.map(short).unwrap_or_else(|| "absent".into()),
                sha256_after: h1.map(short).unwrap_or_else(|| "absent".into()),
            });
        }
    }
    if r0.certificate.sha256 != r1.certificate.sha256 {
        input_changes.push(InputChange {
            name: "certificate".into(),
            sha256_before: short(&r0.certificate.sha256),
            sha256_after: short(&r1.certificate.sha256),
        });
    }

    Ok(AvifyRunDiff {
        roi_changes,
        only_before,
        only_after,
        input_changes,
        engine_before: r0.engine.version.clone(),
        engine_after: r1.engine.version.clone(),
        elapsed_before_s: r0.timing.total_s.max(r0.engine_elapsed_s),
        elapsed_after_s: r1.timing.total_s.max(r1.engine_elapsed_s),
    })
}
