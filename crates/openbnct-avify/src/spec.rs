// SPDX-License-Identifier: MIT

//! The OpenBNCT-side Avify request: case reference + class mapping +
//! the engine's plan fields. The declared set is carried verbatim — the
//! engine constructs corner maps; this crate never does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::plan::{BeamSpec, DeclaredSet, Normalisation};

/// Engine tissue classes (the Avify voxel-plan domain). Order matches
/// the engine's `DEFAULT_CLASSES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineClass {
    Air,
    Brain,
    Cranium,
    Scalp,
    Tumour,
}

impl EngineClass {
    pub const ALL: [EngineClass; 5] = [
        EngineClass::Air,
        EngineClass::Brain,
        EngineClass::Cranium,
        EngineClass::Scalp,
        EngineClass::Tumour,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            EngineClass::Air => "air",
            EngineClass::Brain => "brain",
            EngineClass::Cranium => "cranium",
            EngineClass::Scalp => "scalp",
            EngineClass::Tumour => "tumour",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }
}

/// `openbnct.avify-spec/0.1.0` — everything `openbnct avify` needs that
/// the case itself does not carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvifySpec {
    pub schema_version: String,
    /// Engine tissue class for voxels carrying the base material.
    pub default_class: EngineClass,
    /// Per-region-name override: which engine class a case region's
    /// voxels map to. Unmapped regions reject the export — silent
    /// misclassification is worse than a hard stop.
    pub region_classes: BTreeMap<String, EngineClass>,
    /// ICRU-46 densities per class for the engine's ROI masses.
    pub density_g_cm3: BTreeMap<String, f64>,
    /// The engine plan fields, verbatim.
    pub plan: AvifyPlanFields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AvifyPlanFields {
    pub declared_set: DeclaredSet,
    #[serde(default = "one")]
    pub brain_ratio: f64,
    /// Per-ROI `[wB, wN, wf, wg]` biological weights.
    pub weights: BTreeMap<String, [f64; 4]>,
    /// Per-ROI `[sense, threshold_Gyw]` decision criteria.
    pub criteria: BTreeMap<String, (String, f64)>,
    pub normalisation: Normalisation,
    #[serde(default)]
    pub histories: BTreeMap<String, u64>,
    #[serde(default)]
    pub seeds: BTreeMap<String, u64>,
    #[serde(default)]
    pub beam: BeamSpec,
}

fn one() -> f64 {
    1.0
}

impl AvifySpec {
    pub const SCHEMA: &'static str = "openbnct.avify-spec/0.1.0";

    pub fn validate(&self) -> Result<(), crate::AvifyError> {
        if self.schema_version != Self::SCHEMA {
            return Err(crate::AvifyError::Invalid(format!(
                "spec schema_version {:?} != {:?}",
                self.schema_version,
                Self::SCHEMA
            )));
        }
        for name in ["tumour", "brain", "scalp"] {
            if !self.plan.weights.contains_key(name) {
                return Err(crate::AvifyError::Invalid(format!(
                    "plan.weights missing required ROI {name:?} (engine schema)"
                )));
            }
            if !self.plan.criteria.contains_key(name) {
                return Err(crate::AvifyError::Invalid(format!(
                    "plan.criteria missing required ROI {name:?}"
                )));
            }
        }
        for (roi, (sense, _)) in &self.plan.criteria {
            if sense != ">=" && sense != "<=" {
                return Err(crate::AvifyError::Invalid(format!(
                    "criterion for {roi:?} must be \">=\" or \"<=\", got {sense:?}"
                )));
            }
        }
        Ok(())
    }
}
