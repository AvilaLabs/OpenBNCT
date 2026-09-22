// SPDX-License-Identifier: MIT

//! Exact dose-volume metrics over a named voxel mask.
//!
//! Unlike the binned `DoseVolumeHistogram`, metrics are computed directly
//! from the masked voxel values: `D_x` coverages, `V_x` levels, min/max/
//! mean, and generalized EUD at requested organ parameters. Voxels are
//! treated as equal-volume, which holds for the uniform grids NCTForge
//! emits.

use serde::{Deserialize, Serialize};

use openbnct_core::{
    ContentReference, dose_covering_percent, equivalent_uniform_dose, masked_values, mean,
    volume_at_least,
};

use crate::ManifestError;

pub const DOSE_METRICS_SCHEMA: &str = "openbnct.dose-metrics/0.1.0";

/// `D_x`: the dose level covering the hottest `percent` of the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageMetric {
    /// Coverage fraction in percent, in `(0, 100]`.
    pub percent: f64,
    /// Dose value in the source volume's unit.
    pub dose: f64,
}

/// `V_x`: the volume fraction receiving at least `level` dose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeMetric {
    /// Dose level in the source volume's unit.
    pub level: f64,
    /// Volume fraction in `[0, 1]`.
    pub volume_fraction: f64,
}

/// Generalized equivalent uniform dose at one organ parameter `a`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EudMetric {
    /// Niemierko organ parameter: `a = 1` mean, `a > 0` serial-leaning,
    /// `a < 0` parallel-leaning, `a = 0` geometric mean.
    pub a: f64,
    /// EUD value in the source volume's unit.
    pub dose: f64,
}

/// The dose-volume metric set computed for one region and quantity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionDoseMetrics {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// Name of the region the mask selects.
    pub region: String,
    /// Which dose volume was measured, e.g. `component:boron`,
    /// `physical_total`, or `biological_total`.
    pub quantity: String,
    /// Content binding of the dose artifact the metrics derive from.
    pub source: ContentReference,
    /// Dose unit, copied verbatim from the source volume.
    pub unit: String,
    pub region_voxel_count: u64,
    pub voxel_volume_mm3: f64,
    pub region_volume_mm3: f64,
    /// Minimum, mean, and maximum masked dose.
    pub minimum_dose: f64,
    pub mean_dose: f64,
    pub maximum_dose: f64,
    /// Requested `D_x` readings, in request order.
    pub dx: Vec<CoverageMetric>,
    /// Requested `V_x` readings, in request order.
    pub vx: Vec<VolumeMetric>,
    /// Requested EUD readings, in request order.
    pub eud: Vec<EudMetric>,
}

impl RegionDoseMetrics {
    /// Compute the metric set for `values` restricted to `mask`.
    ///
    /// `dx_percents`, `vx_levels`, and `eud_parameters` select which
    /// readings to take; each entry is validated by the shared statistics
    /// functions in `openbnct-core` (percent in `(0, 100]`, non-negative
    /// levels, finite organ parameters).
    #[allow(clippy::too_many_arguments)]
    pub fn compute(
        case_id: &str,
        region: &str,
        quantity: &str,
        source: ContentReference,
        unit: &str,
        values: &[f64],
        mask: &[bool],
        voxel_volume_mm3: f64,
        dx_percents: &[f64],
        vx_levels: &[f64],
        eud_parameters: &[f64],
    ) -> Result<Self, ManifestError> {
        if !voxel_volume_mm3.is_finite() || voxel_volume_mm3 <= 0.0 {
            return Err(ManifestError::Invalid(
                "voxel volume must be positive".into(),
            ));
        }
        source
            .validate()
            .map_err(|_| ManifestError::Invalid("metrics source reference is invalid".into()))?;
        let selected = masked_values(region, values, mask)
            .map_err(|e| ManifestError::Invalid(format!("dose selection: {e}")))?;
        let invalid = |e| ManifestError::Invalid(format!("dose metric: {e}"));

        let metrics = Self {
            schema_version: DOSE_METRICS_SCHEMA.into(),
            case_id: case_id.into(),
            region: region.into(),
            quantity: quantity.into(),
            source,
            unit: unit.into(),
            region_voxel_count: selected.len() as u64,
            voxel_volume_mm3,
            region_volume_mm3: selected.len() as f64 * voxel_volume_mm3,
            minimum_dose: selected.iter().copied().fold(f64::INFINITY, f64::min),
            mean_dose: mean(&selected),
            maximum_dose: selected.iter().copied().fold(0.0, f64::max),
            dx: dx_percents
                .iter()
                .map(|percent| {
                    dose_covering_percent(&selected, *percent)
                        .map(|dose| CoverageMetric {
                            percent: *percent,
                            dose,
                        })
                        .map_err(invalid)
                })
                .collect::<Result<_, _>>()?,
            vx: vx_levels
                .iter()
                .map(|level| {
                    volume_at_least(&selected, *level)
                        .map(|volume_fraction| VolumeMetric {
                            level: *level,
                            volume_fraction,
                        })
                        .map_err(invalid)
                })
                .collect::<Result<_, _>>()?,
            eud: eud_parameters
                .iter()
                .map(|a| {
                    equivalent_uniform_dose(&selected, *a)
                        .map(|dose| EudMetric { a: *a, dose })
                        .map_err(invalid)
                })
                .collect::<Result<_, _>>()?,
        };
        metrics.validate()?;
        Ok(metrics)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, DOSE_METRICS_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported dose-metrics schema {:?}",
                self.schema_version
            )));
        }
        if self.region_voxel_count == 0
            || !self.region_volume_mm3.is_finite()
            || self.region_volume_mm3 <= 0.0
        {
            return Err(ManifestError::Invalid(
                "metrics region volume is malformed".into(),
            ));
        }
        for (label, value) in [
            ("minimum_dose", self.minimum_dose),
            ("mean_dose", self.mean_dose),
            ("maximum_dose", self.maximum_dose),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(ManifestError::Invalid(format!("{label} is malformed")));
            }
        }
        if self.minimum_dose > self.maximum_dose
            || self.mean_dose < self.minimum_dose - 1.0e-12
            || self.mean_dose > self.maximum_dose + 1.0e-12
        {
            return Err(ManifestError::Invalid(
                "metrics min/mean/max ordering is inconsistent".into(),
            ));
        }
        for metric in &self.dx {
            if !metric.dose.is_finite() || metric.dose < 0.0 {
                return Err(ManifestError::Invalid("Dx dose is malformed".into()));
            }
        }
        for metric in &self.vx {
            if !metric.volume_fraction.is_finite() || !(0.0..=1.0).contains(&metric.volume_fraction)
            {
                return Err(ManifestError::Invalid(
                    "Vx volume fraction is malformed".into(),
                ));
            }
        }
        for metric in &self.eud {
            if !metric.dose.is_finite() || metric.dose < 0.0 {
                return Err(ManifestError::Invalid("EUD dose is malformed".into()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ContentReference {
        ContentReference {
            id: "test-bundle".into(),
            sha256: "b".repeat(64),
        }
    }

    #[test]
    fn metrics_match_analytic_values() {
        let metrics = RegionDoseMetrics::compute(
            "case",
            "roi",
            "physical_total",
            source(),
            "gray",
            &[0.0, 1.0, 2.0, 3.0, 4.0],
            &[true; 5],
            125.0,
            &[98.0, 50.0, 2.0],
            &[1.0, 3.5],
            &[1.0, 12.0, -12.0],
        )
        .unwrap();
        assert_eq!(metrics.minimum_dose, 0.0);
        assert_eq!(metrics.mean_dose, 2.0);
        assert_eq!(metrics.maximum_dose, 4.0);
        // D98: position (1-0.98)*4 = 0.08 -> 0.08 between s[0]=0 and s[1]=1.
        assert!((metrics.dx[0].dose - 0.08).abs() < 1e-12);
        assert_eq!(metrics.dx[1].dose, 2.0);
        // D2: position 0.98*4 = 3.92 -> 0.92*s[4] + 0.08*s[3] = 3.92.
        assert!((metrics.dx[2].dose - 3.92).abs() < 1e-12);
        assert_eq!(metrics.vx[0].volume_fraction, 0.8);
        assert_eq!(metrics.vx[1].volume_fraction, 0.2);
        assert_eq!(metrics.eud[0].dose, 2.0); // a=1 is the mean.
        assert!(metrics.eud[1].dose > metrics.mean_dose); // serial > mean
        assert!(metrics.eud[2].dose < metrics.mean_dose); // parallel < mean
        assert_eq!(metrics.region_volume_mm3, 625.0);
    }

    #[test]
    fn rejects_bad_requests() {
        let source = source();
        let values = vec![1.0, 2.0];
        let mask = vec![true, true];
        // Coverage percent out of range.
        assert!(
            RegionDoseMetrics::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &values,
                &mask,
                1.0,
                &[150.0],
                &[],
                &[],
            )
            .is_err()
        );
        // Negative Vx level.
        assert!(
            RegionDoseMetrics::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &values,
                &mask,
                1.0,
                &[],
                &[-1.0],
                &[],
            )
            .is_err()
        );
        // Non-finite EUD parameter.
        assert!(
            RegionDoseMetrics::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &values,
                &mask,
                1.0,
                &[],
                &[],
                &[f64::NAN],
            )
            .is_err()
        );
        // Empty mask.
        assert!(
            RegionDoseMetrics::compute(
                "c",
                "r",
                "q",
                source,
                "u",
                &values,
                &[false, false],
                1.0,
                &[],
                &[],
                &[],
            )
            .is_err()
        );
    }
}
