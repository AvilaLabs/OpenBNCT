// SPDX-License-Identifier: MIT

//! The engine's plan JSON — serialized verbatim; this crate adds no
//! interpretation. Field names match the engine's plan reader.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The declared uptake-uncertainty set: tumour/blood ratio,
/// scalp/blood ratio, and blood boron (ppm), each `[lo, nom, hi]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredSet {
    pub rt: [f64; 3],
    pub rs: [f64; 3],
    #[serde(rename = "B")]
    pub b: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Normalisation {
    Particles {
        source_particles_total: f64,
    },
    Fluence {
        fluence_rate_cm2s: f64,
        area_cm2: f64,
        time_s: f64,
    },
}

/// Engine beam form: the built-in epithermal preset, an OpenMC file
/// source, or a spectrum dict passed through to the engine's
/// `ingest_source` spectrum path.
#[derive(Debug, Clone, Default)]
pub enum BeamSpec {
    #[default]
    Epithermal,
    FileSource(String),
    Spectrum(serde_json::Value),
}

impl Serialize for BeamSpec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            BeamSpec::Epithermal => s.serialize_str("epithermal"),
            BeamSpec::FileSource(p) => {
                #[derive(Serialize)]
                struct F<'a> {
                    file_source: &'a str,
                }
                F { file_source: p }.serialize(s)
            }
            BeamSpec::Spectrum(v) => v.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for BeamSpec {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        if let Some(s) = v.as_str() {
            return if s == "epithermal" {
                Ok(BeamSpec::Epithermal)
            } else {
                Err(serde::de::Error::custom(format!(
                    "unknown beam preset {s:?} (only \"epithermal\" is built in)"
                )))
            };
        }
        let obj = v
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("beam must be a string or object"))?;
        if let Some(p) = obj.get("file_source") {
            let p = p
                .as_str()
                .ok_or_else(|| serde::de::Error::custom("file_source must be a string"))?;
            return Ok(BeamSpec::FileSource(p.to_string()));
        }
        if let Some(sp) = obj.get("spectrum") {
            return Ok(BeamSpec::Spectrum(sp.clone()));
        }
        Err(serde::de::Error::custom(
            "beam object must carry file_source or spectrum",
        ))
    }
}

/// Plan JSON as the engine consumes it — the declared set is carried
/// verbatim; corner maps are the engine's job, never this crate's.
#[derive(Debug, Clone, Serialize)]
pub struct AvifyPlan {
    pub declared_set: DeclaredSet,
    pub brain_ratio: f64,
    pub weights: BTreeMap<String, [f64; 4]>,
    pub criteria: BTreeMap<String, (String, f64)>,
    pub normalisation: Normalisation,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub histories: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub seeds: BTreeMap<String, u64>,
    pub beam: BeamSpec,
}
