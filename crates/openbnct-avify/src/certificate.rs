// SPDX-License-Identifier: MIT

//! Typed view of the engine's `certificate.json`. Fields the workbench
//! renders are typed; run tallies and engine internals stay as
//! passthrough JSON (this crate never re-derives them).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::error::AvifyError;

#[derive(Debug, Clone, Deserialize)]
pub struct RoiBracket {
    #[serde(rename = "L")]
    pub l: f64,
    #[serde(rename = "U")]
    pub u: f64,
    #[serde(rename = "sL")]
    pub s_l: f64,
    #[serde(rename = "sU")]
    pub s_u: f64,
    /// Photon-response kernel coefficients (γH, γB) and corner
    /// normalization kernels — passthrough, never re-derived here.
    #[serde(default)]
    pub kernels: BTreeMap<String, f64>,
    #[serde(default)]
    pub photon_valid: Option<bool>,
    #[serde(default)]
    pub fast_applicability_ok: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoiAction {
    /// `[sense, threshold_Gyw]` as declared in the plan.
    pub criterion: (String, f64),
    /// `[L-3sL, U+3sU]` in Gy-w after the plan normalisation.
    #[serde(rename = "certified_Gyw")]
    pub certified_gyw: [f64; 2],
    #[serde(rename = "nominal_Gyw")]
    pub nominal_gyw: Option<f64>,
    /// PASS / FAIL / ADDITIONAL_EVIDENCE — the engine's vocabulary.
    pub action: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunRecord {
    pub ppm: BTreeMap<String, f64>,
    pub seed: u64,
    pub histories: u64,
    pub wall_s: f64,
    /// Engine tally payloads — passthrough, not re-typed.
    #[serde(default)]
    pub tallies: Value,
    #[serde(default)]
    pub b10_atom_density: BTreeMap<String, f64>,
    #[serde(default)]
    pub roi_mass_g: BTreeMap<String, f64>,
}

/// The engine's certificate as returned on disk.
#[derive(Debug, Clone, Deserialize)]
pub struct AvifyCertificate {
    pub plan_sha256: String,
    pub ingest_meta_sha256: String,
    pub ingest_arrays_sha256: String,
    #[serde(default)]
    pub openmc_version: Option<String>,
    #[serde(default)]
    pub timestamp_utc: Option<String>,
    /// The plan the engine ran — passthrough for the record.
    #[serde(default)]
    pub plan: Value,
    pub runs: BTreeMap<String, RunRecord>,
    pub brackets: BTreeMap<String, RoiBracket>,
    #[serde(default)]
    pub direct_doses: BTreeMap<String, BTreeMap<String, Value>>,
    #[serde(rename = "scale_Gyw_per_MeVg")]
    pub scale_gyw_per_mevg: f64,
    pub actions: BTreeMap<String, RoiAction>,
    #[serde(default)]
    pub roi_mass_g: BTreeMap<String, f64>,
}

impl AvifyCertificate {
    pub fn load(path: &std::path::Path) -> Result<Self, AvifyError> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }
}
