// SPDX-License-Identifier: Apache-2.0

//! Versioned manifest for a multi-field beam sweep
//! (`openbnct.beam-field-set/0.1.0`).
//!
//! `openbnct plan fields` aims one source per beam direction through a
//! target mask, solves each field, and folds a unit-weight dose bundle
//! per beam — the inputs `plan optimize` consumes. The manifest binds
//! every emitted artifact to the shared case, data, assignment, and
//! aim-mask inputs by content hash so a field set is replayable and
//! auditable end to end.

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current schema token for beam-field-set manifests.
pub const BEAM_FIELD_SET_SCHEMA: &str = "openbnct.beam-field-set/0.1.0";
/// Qualification asserted on every emitted field set.
pub const BEAM_FIELD_SET_QUALIFICATION: &str = "beam_field_sweep_research_only_not_clinical";

/// The deterministic-solver options a sweep ran with — echoed so the
/// manifest alone can replay every solve.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSweepOptions {
    pub quadrature_order: u32,
    pub convergence: f64,
    pub max_inner_iterations: u32,
    pub max_outer_iterations: u32,
    pub periodic: [bool; 3],
    pub beam_uncollided_split: bool,
    pub transport_correction: bool,
    pub p1_anisotropic: bool,
    /// Anisotropy order beyond P1 (0 = none).
    pub anisotropy_order: u32,
    /// Anderson acceleration depth (0 = plain sweeps).
    pub anderson_depth: usize,
}

/// The artifacts one aimed beam produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamFieldArtifacts {
    /// Beam name from the sweep spec.
    pub name: String,
    /// Requested propagation direction, a finite vector in LPS.
    pub direction_lps: [f64; 3],
    /// The aimed transport case this beam solved.
    pub case: ContentReference,
    /// The `openbnct.position-report` for the aim.
    pub position_report: ContentReference,
    /// The folded unit-weight `openbnct.physical-dose-bundle` — a
    /// `plan optimize --dose` input.
    pub dose: ContentReference,
    /// Whether the underlying solve converged, and its record.
    pub converged: bool,
    pub outer_iterations: u32,
    pub residual: f64,
}

/// A versioned beam-field-set manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamFieldSet {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The case every beam was aimed into.
    pub case_id: String,
    /// Shared inputs, content-bound.
    pub case: ContentReference,
    pub data: ContentReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<ContentReference>,
    /// The region mask every beam axis passes through.
    pub aim_mask: ContentReference,
    /// Circular aperture radius applied to every aimed source, in cm.
    pub aperture_radius_cm: f64,
    pub solver: FieldSweepOptions,
    pub beams: Vec<BeamFieldArtifacts>,
    pub provenance_id: String,
    /// Research-status qualification; no clinical claim is made.
    pub qualification: String,
}

/// Errors from field-set validation.
#[derive(Debug, Error)]
pub enum FieldSetError {
    #[error("unsupported beam-field-set schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid beam-field-set manifest: {0}")]
    Invalid(String),
}

impl BeamFieldSet {
    /// Structural validation; called by consumers.
    pub fn validate(&self) -> Result<(), FieldSetError> {
        if !openbnct_core::schema_matches(&self.schema_version, BEAM_FIELD_SET_SCHEMA) {
            return Err(FieldSetError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("case_id", self.case_id.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
            ("qualification", self.qualification.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(FieldSetError::Invalid(format!(
                    "{label} is required and must not be empty"
                )));
            }
        }
        if self.beams.is_empty() {
            return Err(FieldSetError::Invalid(
                "at least one beam is required".into(),
            ));
        }
        for (i, beam) in self.beams.iter().enumerate() {
            if beam.name.trim().is_empty() {
                return Err(FieldSetError::Invalid(format!(
                    "beam {i} name must not be empty"
                )));
            }
            for reference in [&beam.case, &beam.position_report, &beam.dose] {
                reference.validate().map_err(|error| {
                    FieldSetError::Invalid(format!("beam {:?}: {error}", beam.name))
                })?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "0".repeat(64),
        }
    }

    fn set() -> BeamFieldSet {
        BeamFieldSet {
            schema_version: BEAM_FIELD_SET_SCHEMA.into(),
            id: "set".into(),
            case_id: "case".into(),
            case: reference("case"),
            data: reference("data"),
            assignment: None,
            aim_mask: reference("mask"),
            aperture_radius_cm: 5.0,
            solver: FieldSweepOptions {
                quadrature_order: 8,
                convergence: 1e-4,
                max_inner_iterations: 40,
                max_outer_iterations: 400,
                periodic: [false; 3],
                beam_uncollided_split: true,
                transport_correction: true,
                p1_anisotropic: false,
                anisotropy_order: 0,
                anderson_depth: 5,
            },
            beams: vec![BeamFieldArtifacts {
                name: "ap".into(),
                direction_lps: [0.0, 0.0, 1.0],
                case: reference("case-ap"),
                position_report: reference("report-ap"),
                dose: reference("dose-ap"),
                converged: true,
                outer_iterations: 12,
                residual: 9e-5,
            }],
            provenance_id: "test".into(),
            qualification: BEAM_FIELD_SET_QUALIFICATION.into(),
        }
    }

    #[test]
    fn valid_set_round_trips() {
        let set = set();
        set.validate().unwrap();
        let json = serde_json::to_string(&set).unwrap();
        let back: BeamFieldSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn empty_beams_rejected() {
        let mut set = set();
        set.beams.clear();
        assert!(matches!(set.validate(), Err(FieldSetError::Invalid(_))));
    }
}
