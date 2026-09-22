// SPDX-License-Identifier: MIT

//! Systematic uncertainty propagation (`openbnct.systematic-uncertainty/0.1.0`).
//!
//! Monte Carlo uncertainties in a dose bundle are per-voxel *statistical*
//! standard uncertainties — independent from voxel to voxel. Systematic
//! uncertainties behave differently: one blood boron assay, one
//! registration residual, one response calibration shifts *every* voxel
//! together. This module records declared systematic sources, propagates
//! them to per-voxel contributions, and combines them with the bundle's
//! statistical uncertainty under the honest correlation model:
//!
//! - per voxel, sources are independent of one another → quadrature;
//! - for a region mean, statistical σ averages down as
//!   `sqrt(Σσ²)/N` while each systematic source contributes its *mean*
//!   per-voxel σ (fully correlated across voxels), sources then combined
//!   in quadrature.
//!
//! The report is a separate layer: the dose bundle's
//! `absolute_standard_uncertainty` remains pure Monte Carlo, and nothing
//! here relabels a systematic-augmented σ as statistical.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ContentReference, GridGeometry, RegionMask, ValidationError};

/// Current schema token for systematic-uncertainty reports.
pub const SYSTEMATIC_UNCERTAINTY_SCHEMA: &str = "openbnct.systematic-uncertainty/0.1.0";

/// Qualification asserted on every report.
pub const SYSTEMATIC_UNCERTAINTY_QUALIFICATION: &str =
    "systematic_uncertainty_research_only_not_clinical";

/// A declared systematic uncertainty source. The variant is the audit
/// trail — *what* was declared; the per-voxel contribution map is computed
/// at apply time and summarized in [`SourceSummary`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UncertaintySource {
    /// Fractional uncertainty of an estimated B-10 concentration field
    /// applied to the boron dose component:
    /// `σ(v) = D_boron(v) · σ_B(v)/B(v)`. Voxels where the field
    /// concentration is zero contribute zero (recorded in the summary).
    BoronConcentration {
        /// Content-bound `openbnct.boron-field` document.
        field: ContentReference,
    },
    /// Translational positioning uncertainty: `σ(v) = |∇D_phys(v)|·σ_pos`,
    /// the first-order dose shift under a rigid displacement. `∇D` is the
    /// physical-total dose gradient by central differences.
    Positioning {
        /// Declared 1σ displacement magnitude in millimetres.
        sigma_mm: f64,
        /// Optional registration document the σ derives from (its RMS
        /// landmark residual is the conventional estimate).
        registration: Option<ContentReference>,
    },
    /// Declared relative 1σ on a named dose component — response
    /// calibration, model-parameter, or loading uncertainty expressed
    /// directly on the component: `σ(v) = rel · D_component(v)`.
    RelativeComponent {
        /// Component name matching `DoseComponent::name` (`boron`,
        /// `nitrogen`, `hydrogen`, `photon`).
        component: String,
        /// Relative standard uncertainty, dimensionless.
        relative_1sigma: f64,
    },
}

/// Summary statistics for one source's realized per-voxel contribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSummary {
    /// Source kind tag (matches the corresponding `UncertaintySource`).
    pub kind: String,
    /// Mean of the per-voxel contribution over all grid voxels.
    pub mean_1sigma: f64,
    /// Maximum per-voxel contribution.
    pub max_1sigma: f64,
    /// Voxels where the source could contribute but was degenerate —
    /// e.g. a zero-concentration boron voxel — recorded, not hidden.
    pub skipped_voxels: u64,
}

/// Region-mean uncertainty under the correlation model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionUncertainty {
    pub region: String,
    pub voxel_count: u64,
    /// Region mean of the reported quantity.
    pub mean_dose: f64,
    /// σ of the region mean from voxelwise-independent Monte Carlo
    /// statistics: `sqrt(Σσ_mc²)/N`.
    pub monte_carlo_1sigma: Option<f64>,
    /// σ of the region mean with every systematic source fully
    /// correlated across voxels (quadrature across sources):
    /// `sqrt(Σ_src mean_v(σ_src)²)`.
    pub systematic_1sigma: f64,
    /// `sqrt(mc² + systematic²)`.
    pub combined_1sigma: Option<f64>,
}

/// A versioned systematic-uncertainty report over one dose quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystematicUncertaintyReport {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// The physical dose bundle the report propagates over.
    pub dose_bundle: ContentReference,
    /// The quantity the voxel results describe (e.g. `physical_total`).
    pub quantity: String,
    /// Declared sources, in evaluation order.
    pub sources: Vec<UncertaintySource>,
    /// Per-source contribution summaries, parallel to `sources`.
    pub source_summaries: Vec<SourceSummary>,
    /// Per-voxel quadrature-combined systematic σ, grid order, same unit
    /// as the quantity.
    pub systematic_1sigma: Vec<f64>,
    /// Per-voxel `sqrt(σ_mc² + σ_sys²)`; absent when the bundle carries
    /// no statistical uncertainty for the quantity.
    pub combined_1sigma: Option<Vec<f64>>,
    /// Region-mean results for declared masks.
    pub regions: Vec<RegionUncertainty>,
    pub qualification: String,
    pub provenance_id: String,
}

/// Errors from systematic-uncertainty evaluation and validation.
#[derive(Debug, Error)]
pub enum SystematicError {
    #[error("unsupported systematic-uncertainty schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid systematic-uncertainty report: {0}")]
    Invalid(String),
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] ValidationError),
    #[error("invalid content reference: {0}")]
    InvalidContentReference(#[from] crate::ContentReferenceError),
}

impl SystematicUncertaintyReport {
    /// Structural validation; called by consumers.
    pub fn validate(&self) -> Result<(), SystematicError> {
        if !crate::schema_matches(&self.schema_version, SYSTEMATIC_UNCERTAINTY_SCHEMA) {
            return Err(SystematicError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(SystematicError::Invalid("report id is empty".into()));
        }
        self.dose_bundle.validate()?;
        if self.quantity.trim().is_empty() {
            return Err(SystematicError::Invalid("quantity is empty".into()));
        }
        if self.sources.is_empty() {
            return Err(SystematicError::Invalid(
                "at least one uncertainty source is required".into(),
            ));
        }
        if self.source_summaries.len() != self.sources.len() {
            return Err(SystematicError::Invalid(format!(
                "source_summaries length {} does not match sources length {}",
                self.source_summaries.len(),
                self.sources.len()
            )));
        }
        for source in &self.sources {
            match source {
                UncertaintySource::BoronConcentration { field } => field.validate()?,
                UncertaintySource::Positioning {
                    sigma_mm,
                    registration,
                } => {
                    if !sigma_mm.is_finite() || *sigma_mm < 0.0 {
                        return Err(SystematicError::Invalid(
                            "positioning.sigma_mm must be a non-negative finite value".into(),
                        ));
                    }
                    if let Some(reference) = registration {
                        reference.validate()?;
                    }
                }
                UncertaintySource::RelativeComponent {
                    component,
                    relative_1sigma,
                } => {
                    if component.trim().is_empty() {
                        return Err(SystematicError::Invalid(
                            "relative_component.component is empty".into(),
                        ));
                    }
                    if !relative_1sigma.is_finite() || *relative_1sigma < 0.0 {
                        return Err(SystematicError::Invalid(
                            "relative_component.relative_1sigma must be a non-negative finite value"
                                .into(),
                        ));
                    }
                }
            }
        }
        let n = self.systematic_1sigma.len();
        for (index, value) in self.systematic_1sigma.iter().enumerate() {
            if !value.is_finite() || *value < 0.0 {
                return Err(SystematicError::Invalid(format!(
                    "systematic_1sigma[{index}] must be non-negative and finite"
                )));
            }
        }
        if let Some(combined) = &self.combined_1sigma {
            if combined.len() != n {
                return Err(SystematicError::Invalid(
                    "combined_1sigma length does not match systematic_1sigma".into(),
                ));
            }
            for (index, value) in combined.iter().enumerate() {
                if !value.is_finite() || *value < 0.0 {
                    return Err(SystematicError::Invalid(format!(
                        "combined_1sigma[{index}] must be non-negative and finite"
                    )));
                }
            }
        }
        if self.qualification.trim().is_empty() {
            return Err(SystematicError::Invalid("qualification is empty".into()));
        }
        Ok(())
    }
}

/// Per-voxel σ for a `relative_component` source: `σ(v) = rel·D(v)`.
/// Non-finite or negative dose values are treated as zero contribution.
#[must_use]
pub fn relative_component_sigma(dose: &[f64], relative_1sigma: f64) -> Vec<f64> {
    dose.iter()
        .map(|d| {
            if d.is_finite() && *d > 0.0 {
                relative_1sigma * d
            } else {
                0.0
            }
        })
        .collect()
}

/// Per-voxel σ for the boron-field source: `σ(v) = D_b(v)·σ_B(v)/B(v)`
/// for `B(v) > 0`, else 0. Returns the map plus the count of voxels
/// skipped because the field concentration was zero.
#[must_use]
pub fn boron_field_sigma(
    boron_dose: &[f64],
    field_values: &[f64],
    field_sigma: &[f64],
) -> (Vec<f64>, u64) {
    let mut skipped = 0u64;
    let map = boron_dose
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let b = field_values.get(i).copied().unwrap_or(0.0);
            let sigma_b = field_sigma.get(i).copied().unwrap_or(0.0);
            if b > 0.0 && d.is_finite() && *d > 0.0 {
                d * sigma_b / b
            } else {
                if b <= 0.0 && sigma_b > 0.0 {
                    skipped += 1;
                }
                0.0
            }
        })
        .collect();
    (map, skipped)
}

/// Per-voxel positioning σ: `|∇D(v)|·σ_mm` by central differences on the
/// grid (one-sided at faces). The gradient is taken along voxel axes —
/// valid because `GridGeometry` direction is validated orthonormal, so
/// the axis-gradient L2 norm equals the world-space magnitude under any
/// rotation. `dose` is the physical-total volume in grid order.
#[must_use]
pub fn positioning_sigma(dose: &[f64], geometry: &GridGeometry, sigma_mm: f64) -> Vec<f64> {
    let (nx, ny, nz) = (
        geometry.shape[0] as usize,
        geometry.shape[1] as usize,
        geometry.shape[2] as usize,
    );
    let mut out = vec![0.0; dose.len()];
    let gradient_term = |axis: usize, i: usize, j: usize, k: usize| -> f64 {
        let (di, dj, dk) = match axis {
            0 => (1i64, 0i64, 0i64),
            1 => (0i64, 1i64, 0i64),
            _ => (0i64, 0i64, 1i64),
        };
        let extent = [nx, ny, nz][axis];
        let at = |i: i64, j: i64, k: i64| -> f64 {
            dose[(i as usize) + nx * (j as usize) + nx * ny * (k as usize)]
        };
        let (i, j, k) = (i as i64, j as i64, k as i64);
        let coord = [i, j, k][axis];
        let deriv = if coord > 0 && coord + 1 < extent as i64 {
            (at(i + di, j + dj, k + dk) - at(i - di, j - dj, k - dk))
                / (2.0 * geometry.spacing_mm[axis])
        } else if coord + 1 < extent as i64 {
            (at(i + di, j + dj, k + dk) - at(i, j, k)) / geometry.spacing_mm[axis]
        } else if coord > 0 {
            (at(i, j, k) - at(i - di, j - dj, k - dk)) / geometry.spacing_mm[axis]
        } else {
            0.0
        };
        deriv.abs()
    };
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let g2 = gradient_term(0, i, j, k).powi(2)
                    + gradient_term(1, i, j, k).powi(2)
                    + gradient_term(2, i, j, k).powi(2);
                out[i + nx * j + nx * ny * k] = g2.sqrt() * sigma_mm;
            }
        }
    }
    out
}

/// Quadrature-combine per-voxel contribution maps of independent sources.
#[must_use]
pub fn combine_voxel_sigma(maps: &[Vec<f64>]) -> Vec<f64> {
    let n = maps.first().map_or(0, Vec::len);
    let mut out = vec![0.0; n];
    for map in maps {
        debug_assert_eq!(map.len(), n);
        for (o, v) in out.iter_mut().zip(map.iter()) {
            *o += v * v;
        }
    }
    for o in out.iter_mut() {
        *o = o.sqrt();
    }
    out
}

/// `sqrt(σ_mc² + σ_sys²)` per voxel; `None` entries in `mc` are treated
/// as zero statistical contribution.
#[must_use]
pub fn combine_total_sigma(mc: Option<&[f64]>, systematic: &[f64]) -> Option<Vec<f64>> {
    mc.map(|mc| {
        mc.iter()
            .zip(systematic.iter())
            .map(|(m, s)| m.mul_add(*m, s * s).sqrt())
            .collect()
    })
}

/// Region-mean uncertainty honoring the correlation model.
///
/// - Statistical σ averages down: `σ_mc(D̄) = sqrt(Σσ_mc(v)²)/N`.
/// - Each systematic source is fully correlated across voxels:
///   `σ_src(D̄) = mean_v σ_src(v)`; sources combine in quadrature.
/// - Combined σ is the quadrature sum of the two.
#[must_use]
pub fn region_uncertainty(
    region: &str,
    dose: &[f64],
    mc_sigma: Option<&[f64]>,
    source_maps: &[Vec<f64>],
    mask: &RegionMask,
) -> RegionUncertainty {
    let indices: Vec<usize> = mask
        .voxels
        .iter()
        .enumerate()
        .filter_map(|(i, included)| included.then_some(i))
        .collect();
    let n = indices.len().max(1) as f64;
    let mean_dose = indices
        .iter()
        .map(|&i| dose.get(i).copied().unwrap_or(0.0))
        .sum::<f64>()
        / n;
    let monte_carlo_1sigma = mc_sigma.map(|mc| {
        (indices
            .iter()
            .map(|&i| mc.get(i).copied().unwrap_or(0.0).powi(2))
            .sum::<f64>())
        .sqrt()
            / n
    });
    let systematic_1sigma = source_maps
        .iter()
        .map(|map| {
            indices
                .iter()
                .map(|&i| map.get(i).copied().unwrap_or(0.0))
                .sum::<f64>()
                / n
        })
        .map(|mean_source| mean_source * mean_source)
        .sum::<f64>()
        .sqrt();
    let combined_1sigma =
        monte_carlo_1sigma.map(|mc| mc.mul_add(mc, systematic_1sigma * systematic_1sigma).sqrt());
    RegionUncertainty {
        region: region.into(),
        voxel_count: indices.len() as u64,
        mean_dose,
        monte_carlo_1sigma,
        systematic_1sigma,
        combined_1sigma,
    }
}

/// Mean/max summary of a per-voxel contribution map.
#[must_use]
pub fn summarize_source(kind: &str, map: &[f64], skipped_voxels: u64) -> SourceSummary {
    let (mean, max) = if map.is_empty() {
        (0.0, 0.0)
    } else {
        (
            map.iter().sum::<f64>() / map.len() as f64,
            map.iter().copied().fold(0.0_f64, f64::max),
        )
    };
    SourceSummary {
        kind: kind.into(),
        mean_1sigma: mean,
        max_1sigma: max,
        skipped_voxels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [4, 4, 4],
            spacing_mm: [2.0, 2.0, 2.0],
            origin_mm: [-4.0, -4.0, -4.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[test]
    fn relative_sigma_scales_dose() {
        let dose = vec![10.0, 0.0, -1.0, f64::NAN];
        let map = relative_component_sigma(&dose, 0.1);
        assert_eq!(map[0], 1.0);
        assert_eq!(map[1], 0.0);
        assert_eq!(map[2], 0.0);
        assert_eq!(map[3], 0.0);
    }

    #[test]
    fn boron_sigma_uses_fractional_field_uncertainty() {
        let dose = vec![8.0, 8.0, 8.0];
        let b = vec![40.0, 0.0, 20.0];
        let sb = vec![8.0, 2.0, 4.0];
        let (map, skipped) = boron_field_sigma(&dose, &b, &sb);
        assert!((map[0] - 8.0 * 8.0 / 40.0).abs() < 1e-12); // 20% -> 1.6
        assert_eq!(map[1], 0.0);
        assert_eq!(skipped, 1);
        assert!((map[2] - 8.0 * 4.0 / 20.0).abs() < 1e-12);
    }

    #[test]
    fn positioning_sigma_tracks_dose_gradient() {
        // Linear ramp along x: D = i (per index), spacing 2 mm
        // -> |dD/dx| = 0.5/mm interior, one-sided same at faces.
        let (nx, ny, nz) = (4usize, 4usize, 4usize);
        let mut dose = vec![0.0; nx * ny * nz];
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    dose[i + nx * j + nx * ny * k] = i as f64;
                }
            }
        }
        let map = positioning_sigma(&dose, &geometry(), 3.0);
        for &v in &map {
            assert!((v - 1.5).abs() < 1e-12, "{v}");
        }
    }

    #[test]
    fn region_sigma_honors_correlation() {
        // Two voxels: dose 10 each, mc σ 0.1 each (independent),
        // one systematic source contributing 2.0 each (correlated).
        let dose = vec![10.0, 10.0];
        let mc = vec![0.1, 0.1];
        let sys = vec![vec![2.0, 2.0]];
        let mask = RegionMask {
            name: "all".into(),
            voxels: vec![true, true],
        };
        let r = region_uncertainty("all", &dose, Some(&mc), &sys, &mask);
        assert!((r.mean_dose - 10.0).abs() < 1e-12);
        // independent: sqrt(0.01+0.01)/2 ≈ 0.0707
        assert!((r.monte_carlo_1sigma.unwrap() - (0.02f64.sqrt() / 2.0)).abs() < 1e-12);
        // correlated: mean of contributions = 2.0
        assert!((r.systematic_1sigma - 2.0).abs() < 1e-12);
        let want = (0.02f64 / 4.0 + 4.0).sqrt();
        assert!((r.combined_1sigma.unwrap() - want).abs() < 1e-12);
    }

    #[test]
    fn combine_helpers_behave() {
        let maps = vec![vec![3.0, 0.0], vec![4.0, 1.0]];
        assert_eq!(combine_voxel_sigma(&maps), vec![5.0, 1.0]);
        let total = combine_total_sigma(Some(&[0.0, 2.0]), &[5.0, 1.0]).unwrap();
        assert_eq!(total, vec![5.0, 5.0f64.sqrt()]);
        assert!(combine_total_sigma(None, &[1.0]).is_none());
    }
}
