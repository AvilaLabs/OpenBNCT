// SPDX-License-Identifier: MIT

//! Organ-limited irradiation-time evaluation over a per-source-particle
//! dose endpoint.
//!
//! Given an endpoint map in `*_per_source_particle` units (physical
//! components/total or a biological weighted total), a source strength in
//! particles per second, and per-region limits on the maximum or mean
//! endpoint value, the report gives each region's admissible irradiation
//! time and delivered-particle budget and names the limiting structure.
//! Linear accumulation at constant source strength and static anatomy are
//! declared assumptions, not approximations the evaluator can check.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, RegionMask};

use crate::ManifestError;

pub const IRRADIATION_TIME_SCHEMA: &str = "openbnct.irradiation-time-report/0.1.0";

/// Which summary statistic of the endpoint a region limit constrains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LimitMetric {
    /// Largest single-voxel endpoint value inside the region.
    Max,
    /// Mean endpoint value across the region's voxels.
    Mean,
    /// `D_x` dose coverage: the endpoint level exceeded by at most
    /// `(100 - percent)` of region voxels — `dose_covering_percent`
    /// semantics. `dose_coverage: 2` is the hottest-2% dose (`D2`), the
    /// volume-quantile endpoint OpenPINT's limiting-OAR constraint uses.
    DoseCoverage { percent: u16 },
}

/// One region's endpoint limit, in the endpoint's own dose unit.
#[derive(Debug, Clone, PartialEq)]
pub struct OrganLimit {
    pub region: String,
    pub metric: LimitMetric,
    /// Maximum admissible value of the endpoint in its dose unit.
    pub limit: f64,
}

/// One region's evaluated admissible irradiation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionLimitResult {
    pub region: String,
    pub metric: LimitMetric,
    /// The declared limit, in `endpoint_unit` of the parent report.
    pub limit: f64,
    pub region_voxel_count: u64,
    /// Region statistic of the endpoint, per source particle.
    pub endpoint_per_source_particle: f64,
    /// Region statistic rate: `endpoint_per_source_particle * source_strength`.
    pub endpoint_rate_per_s: f64,
    /// `limit / endpoint_per_source_particle`; `None` when the region's
    /// statistic is zero and the limit is unbounded.
    pub max_source_particles: Option<f64>,
    /// `max_source_particles / source_strength_per_s`; `None` when unbounded.
    pub max_time_s: Option<f64>,
}

/// The region/metric pair that bounds the irradiation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitingStructure {
    pub region: String,
    pub metric: LimitMetric,
    pub max_time_s: f64,
    pub max_source_particles: f64,
}

/// Deterministic organ-limited irradiation-time evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IrradiationTimeReport {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// Which dose volume was evaluated, e.g. `component:boron`,
    /// `physical_total`, or `biological_total`.
    pub quantity: String,
    /// Content binding of the dose artifact evaluated.
    pub source: ContentReference,
    /// Endpoint unit, copied verbatim; always `*_per_source_particle`.
    pub endpoint_unit: String,
    /// Source strength in source particles per second.
    pub source_strength_per_s: f64,
    pub regions: Vec<RegionLimitResult>,
    /// `None` when every region's endpoint statistic is zero (unbounded).
    pub limiting: Option<LimitingStructure>,
    pub assumptions: Vec<String>,
}

impl IrradiationTimeReport {
    /// Evaluate `limits` against `masks` over an endpoint map.
    ///
    /// `unit` must end in `per_source_particle` — irradiation time is only
    /// defined for rate-capable endpoints; absolute-dose bundles are
    /// rejected. `source_strength` is source particles per second and must
    /// be finite and positive. Every limit needs a same-named mask, and
    /// every mask must match `values` in length.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        case_id: &str,
        quantity: &str,
        source: ContentReference,
        unit: &str,
        values: &[f64],
        masks: &[RegionMask],
        limits: &[OrganLimit],
        source_strength: f64,
    ) -> Result<Self, ManifestError> {
        if !unit.ends_with("per_source_particle") {
            return Err(ManifestError::Invalid(format!(
                "endpoint unit {unit:?} is not per-source-particle; irradiation time needs a rate-capable endpoint"
            )));
        }
        if !source_strength.is_finite() || source_strength <= 0.0 {
            return Err(ManifestError::Invalid(
                "source strength must be finite and positive".into(),
            ));
        }
        if limits.is_empty() {
            return Err(ManifestError::Invalid(
                "at least one organ limit is required".into(),
            ));
        }
        source
            .validate()
            .map_err(|_| ManifestError::Invalid("dose source reference is invalid".into()))?;

        let mut regions = Vec::with_capacity(limits.len());
        for limit in limits {
            if !limit.limit.is_finite() || limit.limit < 0.0 {
                return Err(ManifestError::Invalid(format!(
                    "limit for region {:?} must be finite and non-negative",
                    limit.region
                )));
            }
            let mask = masks
                .iter()
                .find(|mask| mask.name == limit.region)
                .ok_or_else(|| {
                    ManifestError::Invalid(format!("no mask for limit region {:?}", limit.region))
                })?;
            let selected = openbnct_core::masked_values(&mask.name, values, &mask.voxels).map_err(
                |error| ManifestError::Invalid(format!("region {:?}: {error}", limit.region)),
            )?;
            let statistic = match limit.metric {
                LimitMetric::Max => selected.iter().copied().fold(0.0, f64::max),
                LimitMetric::Mean => openbnct_core::mean(&selected),
                LimitMetric::DoseCoverage { percent } => {
                    openbnct_core::dose_covering_percent(&selected, f64::from(percent)).map_err(
                        |error| {
                            ManifestError::Invalid(format!("region {:?}: {error}", limit.region))
                        },
                    )?
                }
            };
            if statistic < 0.0 {
                return Err(ManifestError::Invalid(format!(
                    "region {:?} has a negative endpoint statistic",
                    limit.region
                )));
            }
            let (max_source_particles, max_time_s) = if statistic == 0.0 {
                (None, None)
            } else {
                let particles = limit.limit / statistic;
                (Some(particles), Some(particles / source_strength))
            };
            regions.push(RegionLimitResult {
                region: limit.region.clone(),
                metric: limit.metric,
                limit: limit.limit,
                region_voxel_count: selected.len() as u64,
                endpoint_per_source_particle: statistic,
                endpoint_rate_per_s: statistic * source_strength,
                max_source_particles,
                max_time_s,
            });
        }

        let limiting = regions
            .iter()
            .filter_map(|region| region.max_time_s.map(|time| (region, time)))
            .min_by(|(a, time_a), (b, time_b)| {
                time_a
                    .total_cmp(time_b)
                    .then_with(|| a.region.cmp(&b.region))
            })
            .map(|(region, time)| LimitingStructure {
                region: region.region.clone(),
                metric: region.metric,
                max_time_s: time,
                max_source_particles: region
                    .max_source_particles
                    .expect("bounded time implies bounded particles"),
            });

        Ok(Self {
            schema_version: IRRADIATION_TIME_SCHEMA.into(),
            case_id: case_id.into(),
            quantity: quantity.into(),
            source,
            endpoint_unit: unit.into(),
            source_strength_per_s: source_strength,
            regions,
            limiting,
            assumptions: vec![
                "endpoint accumulates linearly with delivered source particles at constant source strength".into(),
                "anatomy, material assignment, and the endpoint map are static over the irradiation".into(),
                "region limits apply to the stated max/mean/D_x statistic only; no inter-fraction recovery or repopulation is modeled".into(),
                "biological endpoints use the bundle's weighted unit and model validity domain unchanged".into(),
            ],
        })
    }

    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, IRRADIATION_TIME_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "unsupported irradiation-time schema {:?}",
                self.schema_version
            )));
        }
        if self.regions.is_empty() {
            return Err(ManifestError::Invalid(
                "irradiation-time report has no regions".into(),
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
            id: "test.dose.v1".into(),
            sha256: "ab".repeat(32),
        }
    }

    fn mask(name: &str, voxels: &[bool]) -> RegionMask {
        RegionMask {
            name: name.into(),
            voxels: voxels.to_vec(),
        }
    }

    #[test]
    fn competing_limits_report_the_limiting_structure() {
        // values: organ A voxels at 2.0/particle, organ B at 5.0/particle
        let values = vec![2.0, 2.0, 5.0, 5.0];
        let masks = [
            mask("A", &[true, true, false, false]),
            mask("B", &[false, false, true, true]),
        ];
        let limits = [
            OrganLimit {
                region: "A".into(),
                metric: LimitMetric::Mean,
                limit: 20.0,
            },
            OrganLimit {
                region: "B".into(),
                metric: LimitMetric::Max,
                limit: 50.0,
            },
        ];

        let report = IrradiationTimeReport::evaluate(
            "case",
            "physical_total",
            source(),
            "gray_per_source_particle",
            &values,
            &masks,
            &limits,
            1.0e9,
        )
        .unwrap();

        // A: 20 Gy / 2 Gy-per-particle = 10 particles; B: 50/5 = 10 -> tie,
        // resolved by region name (A first).
        assert_eq!(report.regions[0].max_source_particles, Some(10.0));
        assert_eq!(report.regions[0].max_time_s, Some(1.0e-8));
        let limiting = report.limiting.clone().unwrap();
        assert_eq!(limiting.region, "A");
        report.validate().unwrap();
    }

    #[test]
    fn zero_rate_region_is_unbounded_not_limiting() {
        let values = vec![0.0, 0.0, 1.0];
        let masks = [
            mask("ZERO", &[true, true, false]),
            mask("HOT", &[false, false, true]),
        ];
        let limits = [
            OrganLimit {
                region: "ZERO".into(),
                metric: LimitMetric::Max,
                limit: 1.0,
            },
            OrganLimit {
                region: "HOT".into(),
                metric: LimitMetric::Max,
                limit: 10.0,
            },
        ];

        let report = IrradiationTimeReport::evaluate(
            "case",
            "component:boron",
            source(),
            "gray_per_source_particle",
            &values,
            &masks,
            &limits,
            1.0e6,
        )
        .unwrap();

        assert_eq!(report.regions[0].max_time_s, None);
        assert_eq!(report.limiting.unwrap().region, "HOT");
    }

    #[test]
    fn rejects_absolute_dose_units_and_bad_strength() {
        let values = vec![1.0];
        let masks = [mask("A", &[true])];
        let limits = [OrganLimit {
            region: "A".into(),
            metric: LimitMetric::Mean,
            limit: 1.0,
        }];

        assert!(
            IrradiationTimeReport::evaluate(
                "case",
                "q",
                source(),
                "gray",
                &values,
                &masks,
                &limits,
                1.0
            )
            .is_err()
        );
        assert!(
            IrradiationTimeReport::evaluate(
                "case",
                "q",
                source(),
                "gray_per_source_particle",
                &values,
                &masks,
                &limits,
                0.0
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_limit_without_mask_and_negative_statistic() {
        let values = vec![1.0, 1.0];
        let masks = [mask("A", &[true, true])];
        let missing = [OrganLimit {
            region: "B".into(),
            metric: LimitMetric::Mean,
            limit: 1.0,
        }];
        assert!(
            IrradiationTimeReport::evaluate(
                "case",
                "q",
                source(),
                "gray_per_source_particle",
                &values,
                &masks,
                &missing,
                1.0
            )
            .is_err()
        );
    }

    #[test]
    fn dose_coverage_limit_uses_volume_quantile() {
        // 100 voxels: 98 at 1.0/particle, 2 at 10.0/particle inside "A".
        // D50 reads the median 1.0, D1 the hottest-1% 10.0 — the quantile
        // endpoint decouples from both voxel-max and voxel-mean.
        let mut values = vec![1.0; 98];
        values.extend([10.0, 10.0]);
        let masks = [mask("A", &[true; 100])];
        let limits = [
            OrganLimit {
                region: "A".into(),
                metric: LimitMetric::DoseCoverage { percent: 50 },
                limit: 5.0,
            },
            OrganLimit {
                region: "A".into(),
                metric: LimitMetric::DoseCoverage { percent: 1 },
                limit: 5.0,
            },
        ];
        let report = IrradiationTimeReport::evaluate(
            "case",
            "physical_total",
            source(),
            "gray_per_source_particle",
            &values,
            &masks,
            &limits,
            1.0e9,
        )
        .unwrap();
        let d50 = &report.regions[0];
        let d1 = &report.regions[1];
        assert_eq!(d50.endpoint_per_source_particle, 1.0);
        assert_eq!(d1.endpoint_per_source_particle, 10.0);
        assert!(d50.max_time_s.unwrap() > d1.max_time_s.unwrap());
    }
}
