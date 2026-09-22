// SPDX-License-Identifier: MIT

//! Dose-volume histograms over a named voxel mask.
//!
//! The histogram is a deterministic derived artifact: equal-width dose bins
//! over `[0, max]`, a differential volume fraction per bin, and a cumulative
//! `V(d)` curve giving the fraction of the region receiving at least `d`.
//! Voxels are treated as equal-volume, which holds for the uniform grids
//! NCTForge emits.

use serde::{Deserialize, Serialize};

use openbnct_core::ContentReference;

use crate::ManifestError;

pub const DVH_SCHEMA: &str = "openbnct.dose-volume-histogram/0.1.0";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseVolumeHistogram {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// Name of the region the mask selects.
    pub region: String,
    /// Which dose volume was histogrammed, e.g. `component:boron`,
    /// `physical_total`, or `biological_total`.
    pub quantity: String,
    /// Content binding of the dose artifact the histogram derives from.
    pub source: ContentReference,
    /// Dose-axis unit, copied verbatim from the source volume.
    pub unit: String,
    /// `bin_count + 1` ascending dose edges covering `[0, max_observed]`.
    pub dose_edges: Vec<f64>,
    /// Fraction of region volume whose dose falls in each bin; sums to 1.
    pub differential_volume_fraction: Vec<f64>,
    /// `V(d)` at every edge: fraction of the region with dose >= edge.
    /// Length equals `dose_edges.len()`.
    pub cumulative_volume_fraction: Vec<f64>,
    pub region_voxel_count: u64,
    pub voxel_volume_mm3: f64,
    pub region_volume_mm3: f64,
}

impl DoseVolumeHistogram {
    /// Compute the histogram of `values` restricted to `mask`.
    ///
    /// `values` and `mask` must cover the same grid; the mask must select at
    /// least one voxel. `bin_count` equal-width bins span `[0, max]` where
    /// `max` is the largest masked value. A zero-dose region concentrates all
    /// volume in the first bin.
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
        bin_count: usize,
    ) -> Result<Self, ManifestError> {
        if values.len() != mask.len() {
            return Err(ManifestError::Invalid(format!(
                "dose/mask length mismatch: {} values vs {} mask voxels",
                values.len(),
                mask.len()
            )));
        }
        if bin_count == 0 {
            return Err(ManifestError::Invalid(
                "DVH needs at least one dose bin".into(),
            ));
        }
        if !voxel_volume_mm3.is_finite() || voxel_volume_mm3 <= 0.0 {
            return Err(ManifestError::Invalid(
                "voxel volume must be positive".into(),
            ));
        }
        source
            .validate()
            .map_err(|_| ManifestError::Invalid("DVH source reference is invalid".into()))?;

        let mut selected = Vec::new();
        for (index, inside) in mask.iter().enumerate() {
            if !inside {
                continue;
            }
            let value = values[index];
            if !value.is_finite() || value < 0.0 {
                return Err(ManifestError::Invalid(format!(
                    "dose value at voxel {index} is not finite non-negative"
                )));
            }
            selected.push(value);
        }
        if selected.is_empty() {
            return Err(ManifestError::Invalid(format!(
                "region mask {region} selects no voxels"
            )));
        }

        let max = selected.iter().copied().fold(0.0_f64, f64::max);
        let width = if max > 0.0 {
            max / bin_count as f64
        } else {
            0.0
        };
        let dose_edges: Vec<f64> = (0..=bin_count).map(|index| index as f64 * width).collect();

        let n = selected.len() as f64;
        let mut differential = vec![0.0_f64; bin_count];
        for value in &selected {
            let bin = if width > 0.0 {
                ((value / width) as usize).min(bin_count - 1)
            } else {
                0
            };
            differential[bin] += 1.0 / n;
        }
        // V(edge[i]) = fraction with dose >= edge[i]; exact edge-zero mass
        // belongs to V(0)=1 but not to V(d>0).
        let cumulative_volume_fraction: Vec<f64> = dose_edges
            .iter()
            .map(|edge| selected.iter().filter(|v| **v >= *edge).count() as f64 / n)
            .collect();

        let histogram = Self {
            schema_version: DVH_SCHEMA.into(),
            case_id: case_id.into(),
            region: region.into(),
            quantity: quantity.into(),
            source,
            unit: unit.into(),
            dose_edges,
            differential_volume_fraction: differential,
            cumulative_volume_fraction,
            region_voxel_count: selected.len() as u64,
            voxel_volume_mm3,
            region_volume_mm3: selected.len() as f64 * voxel_volume_mm3,
        };
        histogram.validate()?;
        Ok(histogram)
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, DVH_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported DVH schema {:?}",
                self.schema_version
            )));
        }
        if self.dose_edges.len() < 2
            || self.dose_edges.windows(2).any(|w| w[0] > w[1])
            || self.dose_edges.iter().any(|v| !v.is_finite())
        {
            return Err(ManifestError::Invalid(
                "DVH dose edges must be ascending and finite".into(),
            ));
        }
        if self.differential_volume_fraction.len() != self.dose_edges.len() - 1
            || self.cumulative_volume_fraction.len() != self.dose_edges.len()
        {
            return Err(ManifestError::Invalid("DVH vector lengths disagree".into()));
        }
        let sum: f64 = self.differential_volume_fraction.iter().sum();
        if (sum - 1.0).abs() > 1.0e-9 {
            return Err(ManifestError::Invalid(format!(
                "differential volume fractions sum to {sum}, not 1"
            )));
        }
        if self
            .cumulative_volume_fraction
            .windows(2)
            .any(|w| w[0] < w[1] - 1.0e-12)
        {
            return Err(ManifestError::Invalid(
                "cumulative DVH must be non-increasing".into(),
            ));
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
    fn histogram_is_deterministic_and_normalized() {
        let values = vec![0.0, 1.0, 2.0, 3.0, 4.0];
        let mask = vec![true, true, true, true, true];
        let dvh = DoseVolumeHistogram::compute(
            "case",
            "roi",
            "physical_total",
            source(),
            "gray_per_source_particle",
            &values,
            &mask,
            125.0,
            4,
        )
        .unwrap();
        assert_eq!(dvh.dose_edges, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        // Bins: [0,1)={0}, [1,2)={1}, [2,3)={2}, [3,4]={3,4}
        assert_eq!(dvh.differential_volume_fraction, vec![0.2, 0.2, 0.2, 0.4]);
        // V(0)=1.0, V(1)=0.8, V(2)=0.6, V(3)=0.4, V(4)=0.2
        assert_eq!(
            dvh.cumulative_volume_fraction,
            vec![1.0, 0.8, 0.6, 0.4, 0.2]
        );
        assert_eq!(dvh.region_volume_mm3, 625.0);
    }

    #[test]
    fn zero_dose_region_concentrates_in_first_bin() {
        let dvh = DoseVolumeHistogram::compute(
            "case",
            "roi",
            "physical_total",
            source(),
            "gray_per_source_particle",
            &[0.0, 0.0],
            &[true, true],
            1.0,
            8,
        )
        .unwrap();
        assert_eq!(dvh.differential_volume_fraction[0], 1.0);
        // All edges collapse to 0.0, so every cumulative ordinate is V(0)=1.
        assert!(dvh.cumulative_volume_fraction.iter().all(|v| *v == 1.0));
    }

    #[test]
    fn rejects_bad_inputs() {
        let source = source();
        // Length mismatch.
        assert!(
            DoseVolumeHistogram::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &[1.0],
                &[true, true],
                1.0,
                4
            )
            .is_err()
        );
        // Empty mask.
        assert!(
            DoseVolumeHistogram::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &[1.0],
                &[false],
                1.0,
                4
            )
            .is_err()
        );
        // Negative dose inside the mask.
        assert!(
            DoseVolumeHistogram::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &[-1.0],
                &[true],
                1.0,
                4
            )
            .is_err()
        );
        // Zero bins.
        assert!(
            DoseVolumeHistogram::compute(
                "c",
                "r",
                "q",
                source.clone(),
                "u",
                &[1.0],
                &[true],
                1.0,
                0
            )
            .is_err()
        );
        // Negative dose outside the mask is ignored.
        assert!(
            DoseVolumeHistogram::compute(
                "c",
                "r",
                "q",
                source,
                "u",
                &[1.0, -5.0],
                &[true, false],
                1.0,
                4
            )
            .is_ok()
        );
    }
}
