// SPDX-License-Identifier: MIT

//! External photon/hadron dose import (`openbnct.external-dose/0.1.0`) and
//! dose-field resampling.
//!
//! This is the single-scalar counterpart of the component-dose interchange:
//! an external pipeline (treatment-planning export, MCNP/PHITS/Geant4 dose
//! tallies, a research code) contributes one absolute absorbed-dose field on
//! a declared grid plus the fractionation the dose was delivered in. The
//! importer validates it into an [`ExternalDoseBundle`] whose provenance
//! binds the document hash (`external-dose:<system>:sha256:<hash>`).
//!
//! Fractionation is declared, never guessed: `uniform` splits the total into
//! `count` equal per-fraction doses; `explicit` carries the full per-fraction
//! dose arrays. The biological layer (`openbnct-bio`) turns either into a
//! BED/EQD2 field; combining that field with a BNCT biological bundle is a
//! separate explicit step with its own compatibility gates.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ExternalProducer, GridGeometry, ValidationError, grid_geometry_equivalent};

/// Current schema token for external dose documents.
pub const EXTERNAL_DOSE_SCHEMA: &str = "openbnct.external-dose/0.1.0";

/// What the imported dose field physically is. `physical` is absorbed dose;
/// `rbe_weighted` declares the producer already applied an RBE model — the
/// basis is carried through BED conversion so a weighted field can never be
/// silently read as physical dose downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalDoseQuantity {
    Physical,
    RbeWeighted,
}

/// How the total dose divides into fractions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalFractionation {
    /// `count` identical fractions; per-fraction dose is `values / count`.
    Uniform { count: u32 },
    /// Explicit per-fraction dose arrays, each voxel-aligned to `geometry`.
    Explicit { doses: Vec<Vec<f64>> },
}

/// A `openbnct.external-dose/0.1.0` document as emitted by the producer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalDoseDocument {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// DICOM frame-of-reference UID tying the grid to a patient space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_of_reference_uid: Option<String>,
    pub geometry: GridGeometry,
    pub producer: ExternalProducer,
    /// What the dose physically is; `rbe_weighted` producers must say so.
    pub quantity: ExternalDoseQuantity,
    /// Absolute absorbed dose in gray over the whole course.
    pub values: Vec<f64>,
    /// Optional per-voxel 1-sigma absolute uncertainty, same unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    /// The fractionation the course was delivered in.
    pub fractionation: ExternalFractionation,
}

/// The validated, provenance-bound form produced by [`import_external_dose`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalDoseBundle {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_of_reference_uid: Option<String>,
    pub geometry: GridGeometry,
    pub producer: ExternalProducer,
    pub quantity: ExternalDoseQuantity,
    pub values: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    pub fractionation: ExternalFractionation,
    /// `external-dose:<system>:sha256:<document hash>`.
    pub provenance_id: String,
}

impl ExternalDoseBundle {
    /// Number of fractions declared by the producer.
    #[must_use]
    pub fn fraction_count(&self) -> usize {
        match &self.fractionation {
            ExternalFractionation::Uniform { count } => *count as usize,
            ExternalFractionation::Explicit { doses } => doses.len(),
        }
    }

    /// Per-fraction dose at `voxel`, expanding a uniform split.
    pub fn fraction_dose(&self, fraction: usize, voxel: usize) -> f64 {
        match &self.fractionation {
            ExternalFractionation::Uniform { count } => self.values[voxel] / f64::from(*count),
            ExternalFractionation::Explicit { doses } => doses[fraction][voxel],
        }
    }
}

#[derive(Debug, Error)]
pub enum ExternalDoseError {
    #[error("unsupported schema_version {0:?}")]
    UnsupportedSchema(String),
    #[error("field {0} must be nonempty")]
    EmptyField(&'static str),
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] ValidationError),
    #[error("dose values are not finite non-negative voxel values")]
    InvalidDoseValues,
    #[error("uncertainty values are not finite non-negative voxel values")]
    InvalidUncertainty,
    #[error("fractionation declares {0} doses; each must cover the grid")]
    InvalidFractionDoses(usize),
    #[error("fraction count must be at least one")]
    ZeroFractions,
}

/// Validate an external-dose document into a provenance-bound bundle.
///
/// `document_sha256` is the SHA-256 of the document's bytes — the caller
/// hashes the exact file so provenance binds content, not a name.
pub fn import_external_dose(
    document: &ExternalDoseDocument,
    document_sha256: &str,
) -> Result<ExternalDoseBundle, ExternalDoseError> {
    if !crate::schema_matches(&document.schema_version, EXTERNAL_DOSE_SCHEMA) {
        return Err(ExternalDoseError::UnsupportedSchema(
            document.schema_version.clone(),
        ));
    }
    if document.case_id.trim().is_empty() {
        return Err(ExternalDoseError::EmptyField("case_id"));
    }
    if document.producer.system.trim().is_empty() {
        return Err(ExternalDoseError::EmptyField("producer.system"));
    }
    if document.producer.version.trim().is_empty() {
        return Err(ExternalDoseError::EmptyField("producer.version"));
    }
    if document.producer.normalization.trim().is_empty() {
        return Err(ExternalDoseError::EmptyField("producer.normalization"));
    }
    if let Some(uid) = &document.frame_of_reference_uid
        && uid.trim().is_empty()
    {
        return Err(ExternalDoseError::EmptyField("frame_of_reference_uid"));
    }
    let voxel_count = document.geometry.voxel_count()?;
    if document.values.len() != voxel_count
        || document.values.iter().any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(ExternalDoseError::InvalidDoseValues);
    }
    if let Some(sigma) = &document.absolute_standard_uncertainty
        && (sigma.len() != voxel_count || sigma.iter().any(|v| !v.is_finite() || *v < 0.0))
    {
        return Err(ExternalDoseError::InvalidUncertainty);
    }
    match &document.fractionation {
        ExternalFractionation::Uniform { count } if *count == 0 => {
            return Err(ExternalDoseError::ZeroFractions);
        }
        ExternalFractionation::Explicit { doses } => {
            if doses.is_empty() {
                return Err(ExternalDoseError::ZeroFractions);
            }
            for dose in doses {
                if dose.len() != voxel_count || dose.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    return Err(ExternalDoseError::InvalidFractionDoses(doses.len()));
                }
            }
            // Explicit fractions must sum to the declared total — otherwise
            // the two dose statements silently disagree.
            for voxel in 0..voxel_count {
                let sum: f64 = doses.iter().map(|d| d[voxel]).sum();
                let total = document.values[voxel];
                if (sum - total).abs() > 1e-6 * total.abs().max(1e-12) {
                    return Err(ExternalDoseError::InvalidFractionDoses(doses.len()));
                }
            }
        }
        ExternalFractionation::Uniform { .. } => {}
    }

    Ok(ExternalDoseBundle {
        schema_version: document.schema_version.clone(),
        case_id: document.case_id.clone(),
        frame_of_reference_uid: document.frame_of_reference_uid.clone(),
        geometry: document.geometry.clone(),
        producer: document.producer.clone(),
        quantity: document.quantity,
        values: document.values.clone(),
        absolute_standard_uncertainty: document.absolute_standard_uncertainty.clone(),
        fractionation: document.fractionation.clone(),
        provenance_id: format!(
            "external-dose:{}:sha256:{document_sha256}",
            document.producer.system
        ),
    })
}

/// Dose-field resampling method. Trilinear interpolation at voxel centers is
/// the only supported scheme; the choice is recorded in whatever record the
/// resampled field lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResampleMethod {
    Trilinear,
}

#[derive(Debug, Error, PartialEq)]
pub enum ResampleError {
    #[error("resampling requires axis-aligned (identity-direction) grids")]
    NonAxisAligned,
    #[error("source field has {0} values; grid holds {1} voxels")]
    LengthMismatch(usize, usize),
    #[error("target grid is not covered by the source field on axis {axis}")]
    UncoveredTarget { axis: usize },
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] ValidationError),
}

/// Resample a voxel field onto a target grid by trilinear interpolation at
/// voxel centers.
///
/// Strict co-registration: both grids must be axis-aligned and every target
/// voxel center must lie inside the source field's outer bounds (source
/// centers at the edge extrapolate flatly to the half-spacing face). A
/// target point outside the source extent is a hard error — never a silent
/// zero.
pub fn resample_trilinear(
    values: &[f64],
    source: &GridGeometry,
    target: &GridGeometry,
) -> Result<Vec<f64>, ResampleError> {
    let identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    if source.direction != identity || target.direction != identity {
        return Err(ResampleError::NonAxisAligned);
    }
    let src_nx = source.shape[0] as usize;
    let src_ny = source.shape[1] as usize;
    let src_nz = source.shape[2] as usize;
    let src_count = src_nx * src_ny * src_nz;
    if values.len() != src_count {
        return Err(ResampleError::LengthMismatch(values.len(), src_count));
    }
    let target_count = target.voxel_count()?;
    // Reuse the shared equivalent-grid check: an identical grid resamples to
    // the same values with no interpolation cost.
    if grid_geometry_equivalent(source, target) {
        return Ok(values.to_vec());
    }

    let mut out = Vec::with_capacity(target_count);
    let [tnx, tny, tnz] = target.shape.map(|d| d as usize);
    for k in 0..tnz {
        for j in 0..tny {
            for i in 0..tnx {
                let mut value = 0.0;
                let mut weights = [(0_usize, 0.0_f64); 8];
                for axis in 0..3 {
                    let p = match axis {
                        0 => target.origin_mm[0] + i as f64 * target.spacing_mm[0],
                        1 => target.origin_mm[1] + j as f64 * target.spacing_mm[1],
                        _ => target.origin_mm[2] + k as f64 * target.spacing_mm[2],
                    };
                    // Fractional source index of the target center; a target
                    // center may sit at most half a spacing outside the outer
                    // centers (flat extrapolation to the face), never beyond.
                    let f = (p - source.origin_mm[axis]) / source.spacing_mm[axis];
                    let n = source.shape[axis] as f64;
                    if !(-0.5..=n - 0.5).contains(&f) {
                        return Err(ResampleError::UncoveredTarget { axis });
                    }
                    let f = f.clamp(0.0, n - 1.0);
                    let lo = f.floor() as usize;
                    let hi = (lo + 1).min(source.shape[axis] as usize - 1);
                    weights[axis * 2] = (lo, 1.0 - (f - lo as f64));
                    weights[axis * 2 + 1] = (hi, f - lo as f64);
                }
                for (bi, wi) in [weights[0], weights[1]] {
                    for (bj, wj) in [weights[2], weights[3]] {
                        for (bk, wk) in [weights[4], weights[5]] {
                            value += values[bi + src_nx * bj + src_nx * src_ny * bk] * wi * wj * wk;
                        }
                    }
                }
                out.push(value);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> GridGeometry {
        GridGeometry {
            shape: [2, 1, 1],
            spacing_mm: [5.0, 5.0, 5.0],
            origin_mm: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn producer() -> ExternalProducer {
        ExternalProducer {
            system: "course-sim".into(),
            version: "1".into(),
            normalization: "absolute gray".into(),
        }
    }

    fn document() -> ExternalDoseDocument {
        ExternalDoseDocument {
            schema_version: EXTERNAL_DOSE_SCHEMA.into(),
            case_id: "case".into(),
            frame_of_reference_uid: None,
            geometry: grid(),
            producer: producer(),
            quantity: ExternalDoseQuantity::Physical,
            values: vec![60.0, 60.0],
            absolute_standard_uncertainty: Some(vec![0.6, 0.6]),
            fractionation: ExternalFractionation::Uniform { count: 30 },
        }
    }

    #[test]
    fn valid_document_binds_provenance() {
        let bundle = import_external_dose(&document(), "ab12").unwrap();
        assert_eq!(bundle.provenance_id, "external-dose:course-sim:sha256:ab12");
        assert_eq!(bundle.fraction_count(), 30);
        assert!((bundle.fraction_dose(0, 0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn rejects_wrong_schema_and_bad_values() {
        let mut bad = document();
        bad.schema_version = "openbnct.external-dose/0.0.0".into();
        assert!(import_external_dose(&bad, "x").is_err());
        let mut bad = document();
        bad.values.push(1.0); // 3 values on a 2-voxel grid
        assert!(import_external_dose(&bad, "x").is_err());
        let mut bad = document();
        bad.values[0] = f64::NAN;
        assert!(import_external_dose(&bad, "x").is_err());
        let mut bad = document();
        bad.fractionation = ExternalFractionation::Uniform { count: 0 };
        assert!(import_external_dose(&bad, "x").is_err());
    }

    #[test]
    fn explicit_fractions_must_sum_to_total() {
        let mut doc = document();
        doc.fractionation = ExternalFractionation::Explicit {
            doses: vec![vec![30.0, 30.0], vec![30.0, 30.0]],
        };
        assert!(import_external_dose(&doc, "x").is_ok());
        doc.fractionation = ExternalFractionation::Explicit {
            doses: vec![vec![30.0, 30.0], vec![29.0, 30.0]],
        };
        assert!(import_external_dose(&doc, "x").is_err());
    }

    #[test]
    fn resample_identity_is_exact() {
        let values = vec![1.0, 2.0];
        assert_eq!(
            resample_trilinear(&values, &grid(), &grid()).unwrap(),
            values
        );
    }

    #[test]
    fn resample_trilinear_interpolates_centers() {
        // Source: 2 voxels at x = 0, 5 mm with values 0 and 10.
        // Target: 3 voxels at x = 0, 2.5, 5 mm → 0, 5, 10.
        let target = GridGeometry {
            shape: [3, 1, 1],
            spacing_mm: [2.5, 5.0, 5.0],
            ..grid()
        };
        let out = resample_trilinear(&[0.0, 10.0], &grid(), &target).unwrap();
        assert!((out[0] - 0.0).abs() < 1e-12);
        assert!((out[1] - 5.0).abs() < 1e-12);
        assert!((out[2] - 10.0).abs() < 1e-12);
    }

    #[test]
    fn resample_rejects_uncovered_target() {
        // Target centered beyond the source extent (x = 10 mm past the edge).
        let target = GridGeometry {
            shape: [1, 1, 1],
            origin_mm: [20.0, 0.0, 0.0],
            ..grid()
        };
        assert_eq!(
            resample_trilinear(&[1.0, 2.0], &grid(), &target),
            Err(ResampleError::UncoveredTarget { axis: 0 })
        );
    }

    #[test]
    fn resample_rejects_rotated_grids() {
        let mut rotated = grid();
        rotated.direction = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        assert_eq!(
            resample_trilinear(&[1.0, 2.0], &rotated, &grid()),
            Err(ResampleError::NonAxisAligned)
        );
    }
}
