// SPDX-License-Identifier: MIT

//! Multi-exposure physical-dose accumulation.
//!
//! A research scenario may deliver several weighted exposures — fields,
//! fractions, or repeated runs — onto one voxel grid. The
//! `openbnct.exposure-plan/0.1.0` contract declares each exposure's bound
//! dose bundle, multiplicative delivery weight, basis, and optional
//! duration and boron assumption. Accumulation sums `weight * dose` and
//! propagates 1-sigma uncertainties under the declared covariance
//! assumption; the only supported assumption is statistical independence
//! between exposures, so sigmas add in quadrature. Within one exposure the
//! component covariance is already accounted for by that bundle's
//! dedicated physical-total estimator — the accumulated total sums the
//! exposure totals, not the accumulated components.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ContentReference, DoseVolume, PHYSICAL_DOSE_BUNDLE_SCHEMA, PhysicalDoseBundle,
    PhysicalTotalDoseVolume, TotalUncertaintyMethod, ValidationError,
};

/// Schema identifier carried by every `ExposurePlan`.
pub const EXPOSURE_PLAN_SCHEMA: &str = "openbnct.exposure-plan/0.1.0";

/// A file on disk bound by content hash. `path` is resolved relative to the
/// plan file's directory by the caller; the referenced bytes must hash to
/// `sha256`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundFileReference {
    pub id: String,
    pub sha256: String,
    pub path: String,
}

/// What an exposure's delivery weight physically represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightBasis {
    /// Fraction of a prescribed delivery (for example one fraction of a
    /// multi-fraction course).
    DeliveredFraction,
    /// Ratio of delivered source strength or monitor units to the bundle's
    /// simulated normalization.
    SourceStrengthScaling,
    /// Ratio of delivered particle histories to the simulated histories.
    DeliveredHistories,
    /// Weight derived outside the above bases; the plan's notes must say how.
    Manual,
}

/// Covariance model between exposures. Only independence is supported:
/// statistically coupled exposures (for example two fractions sharing one
/// transport run) must be merged into a single exposure first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExposureCovariance {
    IndependentExposures,
}

/// One weighted exposure: a bound dose bundle plus its delivery semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    pub name: String,
    pub dose_bundle: BoundFileReference,
    /// Multiplicative delivery scale applied to the bundle's per-source
    /// dose. Must be finite and non-negative.
    pub weight: f64,
    pub weight_basis: WeightBasis,
    /// Irradiation duration in seconds, when the plan tracks dose rate.
    pub duration_s: Option<f64>,
    /// Free-text record of the boron concentration or compound assumed for
    /// this exposure — for example the ppm and biodistribution the bound
    /// run's material encoded.
    pub boron_assumption: Option<String>,
}

/// A set of weighted exposures accumulated onto one voxel grid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposurePlan {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Case identity carried by the accumulated bundle. Exposures may come
    /// from runs with different case ids (for example different field
    /// directions); the plan's case id is the accumulated scenario's.
    pub case_id: String,
    pub covariance: ExposureCovariance,
    pub exposures: Vec<Exposure>,
}

impl ExposurePlan {
    /// Collect every detectable problem with the plan rather than stopping
    /// at the first, for user-facing diagnostics on malformed plans.
    /// Semantic errors that need cross-field context (duplicate exposure
    /// names) are reported alongside field-level errors.
    pub fn validate_diagnostics(&self) -> Vec<ExposurePlanError> {
        let mut issues = Vec::new();
        if !crate::schema_matches(&self.schema_version, EXPOSURE_PLAN_SCHEMA) {
            issues.push(ExposurePlanError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
            return issues;
        }
        for (label, value) in [("id", self.id.as_str()), ("case_id", self.case_id.as_str())] {
            if value.trim().is_empty() {
                issues.push(ExposurePlanError::EmptyIdentifier(label));
            }
        }
        if self.exposures.is_empty() {
            issues.push(ExposurePlanError::NoExposures);
            return issues;
        }
        let mut names = BTreeSet::new();
        for exposure in &self.exposures {
            if exposure.name.trim().is_empty() {
                issues.push(ExposurePlanError::EmptyIdentifier("exposure.name"));
            } else if !names.insert(exposure.name.as_str()) {
                issues.push(ExposurePlanError::DuplicateExposure(exposure.name.clone()));
            }
            if !exposure.weight.is_finite() || exposure.weight < 0.0 {
                issues.push(ExposurePlanError::InvalidWeight(exposure.name.clone()));
            }
            if let Some(duration) = exposure.duration_s
                && (!duration.is_finite() || duration <= 0.0)
            {
                issues.push(ExposurePlanError::InvalidDuration(exposure.name.clone()));
            }
            let reference = &exposure.dose_bundle;
            if reference.id.trim().is_empty() || reference.path.trim().is_empty() {
                issues.push(ExposurePlanError::EmptyIdentifier("exposure.dose_bundle"));
            }
            if (ContentReference {
                id: reference.id.clone(),
                sha256: reference.sha256.clone(),
            })
            .validate()
            .is_err()
            {
                issues.push(ExposurePlanError::InvalidBundleReference(
                    exposure.name.clone(),
                ));
            }
        }
        issues
    }

    pub fn validate(&self) -> Result<(), ExposurePlanError> {
        if !crate::schema_matches(&self.schema_version, EXPOSURE_PLAN_SCHEMA) {
            return Err(ExposurePlanError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        for (label, value) in [("id", self.id.as_str()), ("case_id", self.case_id.as_str())] {
            if value.trim().is_empty() {
                return Err(ExposurePlanError::EmptyIdentifier(label));
            }
        }
        if self.exposures.is_empty() {
            return Err(ExposurePlanError::NoExposures);
        }
        let mut names = BTreeSet::new();
        for exposure in &self.exposures {
            if exposure.name.trim().is_empty() {
                return Err(ExposurePlanError::EmptyIdentifier("exposure.name"));
            }
            if !names.insert(exposure.name.as_str()) {
                return Err(ExposurePlanError::DuplicateExposure(exposure.name.clone()));
            }
            if !exposure.weight.is_finite() || exposure.weight < 0.0 {
                return Err(ExposurePlanError::InvalidWeight(exposure.name.clone()));
            }
            if let Some(duration) = exposure.duration_s
                && (!duration.is_finite() || duration <= 0.0)
            {
                return Err(ExposurePlanError::InvalidDuration(exposure.name.clone()));
            }
            let reference = &exposure.dose_bundle;
            if reference.id.trim().is_empty() || reference.path.trim().is_empty() {
                return Err(ExposurePlanError::EmptyIdentifier("exposure.dose_bundle"));
            }
            ContentReference {
                id: reference.id.clone(),
                sha256: reference.sha256.clone(),
            }
            .validate()
            .map_err(|_| ExposurePlanError::InvalidBundleReference(exposure.name.clone()))?;
        }
        Ok(())
    }
}

/// Accumulate the plan's exposure bundles into one physical dose bundle.
///
/// `bundles` must align one-to-one with `plan.exposures` — the caller
/// resolves each bound path, verifies the recorded SHA-256, and parses the
/// bundle before calling. Every bundle must share the accumulated grid's
/// geometry, component profile, component set, and dose unit. The result
/// reuses `openbnct.physical-dose-bundle/0.2.0`; when the exposures bind
/// different response sets (for example differing boron loading), the
/// output's `response_set` reference points at the plan artifact, which
/// enumerates the actual bound sets.
pub fn accumulate_exposures(
    plan: &ExposurePlan,
    plan_sha256: &str,
    bundles: &[PhysicalDoseBundle],
) -> Result<PhysicalDoseBundle, ExposurePlanError> {
    plan.validate()?;
    if bundles.len() != plan.exposures.len() {
        return Err(ExposurePlanError::BundleCountMismatch {
            expected: plan.exposures.len(),
            actual: bundles.len(),
        });
    }
    for bundle in bundles {
        bundle.validate()?;
    }
    let first = &bundles[0];
    for (exposure, bundle) in plan.exposures.iter().zip(bundles.iter()) {
        if bundle.geometry != first.geometry {
            return Err(ExposurePlanError::GeometryMismatch(exposure.name.clone()));
        }
        if bundle.component_profile != first.component_profile {
            return Err(ExposurePlanError::ComponentProfileMismatch(
                exposure.name.clone(),
            ));
        }
        if bundle.physical_total.unit != first.physical_total.unit {
            return Err(ExposurePlanError::DoseUnitMismatch(exposure.name.clone()));
        }
        if bundle.frame_of_reference_uid != first.frame_of_reference_uid {
            return Err(ExposurePlanError::GeometryMismatch(exposure.name.clone()));
        }
        let first_components: BTreeSet<_> = first.components.iter().map(|v| v.component).collect();
        let components: BTreeSet<_> = bundle.components.iter().map(|v| v.component).collect();
        if components != first_components {
            return Err(ExposurePlanError::ComponentSetMismatch(
                exposure.name.clone(),
            ));
        }
    }
    let voxel_count = first.physical_total.values.len();
    let shared_response_set = if bundles
        .iter()
        .all(|bundle| bundle.response_set == first.response_set)
    {
        first.response_set.clone()
    } else {
        ContentReference {
            id: format!("multiple:{}", plan.id),
            sha256: plan_sha256.to_owned(),
        }
    };

    let mut components = Vec::with_capacity(first.components.len());
    for component in &first.components {
        let mut values = vec![0.0; voxel_count];
        let mut sigmas = vec![0.0_f64; voxel_count];
        let mut have_sigmas = true;
        for (exposure, bundle) in plan.exposures.iter().zip(bundles.iter()) {
            let volume = bundle
                .components
                .iter()
                .find(|v| v.component == component.component)
                .expect("component set checked above");
            for (index, value) in values.iter_mut().enumerate() {
                *value += exposure.weight * volume.values[index];
            }
            match &volume.absolute_standard_uncertainty {
                Some(sigma) => {
                    for (index, accumulated) in sigmas.iter_mut().enumerate() {
                        *accumulated += (exposure.weight * sigma[index]).powi(2);
                    }
                }
                None => have_sigmas = false,
            }
        }
        components.push(DoseVolume {
            component: component.component,
            unit: component.unit,
            values,
            absolute_standard_uncertainty: have_sigmas
                .then(|| sigmas.iter().map(|sigma| sigma.sqrt()).collect()),
        });
    }

    let mut total_values = vec![0.0; voxel_count];
    let mut total_sigmas = vec![0.0_f64; voxel_count];
    let mut total_method = TotalUncertaintyMethod::DedicatedEstimator;
    let mut have_total_sigmas = true;
    for (exposure, bundle) in plan.exposures.iter().zip(bundles.iter()) {
        for (index, value) in total_values.iter_mut().enumerate() {
            *value += exposure.weight * bundle.physical_total.values[index];
        }
        match (
            &bundle.physical_total.absolute_standard_uncertainty,
            bundle.physical_total.uncertainty_method,
        ) {
            (Some(sigma), TotalUncertaintyMethod::DedicatedEstimator) => {
                for (index, accumulated) in total_sigmas.iter_mut().enumerate() {
                    *accumulated += (exposure.weight * sigma[index]).powi(2);
                }
            }
            (Some(sigma), TotalUncertaintyMethod::BatchCovariance) => {
                total_method = TotalUncertaintyMethod::BatchCovariance;
                for (index, accumulated) in total_sigmas.iter_mut().enumerate() {
                    *accumulated += (exposure.weight * sigma[index]).powi(2);
                }
            }
            _ => {
                total_method = TotalUncertaintyMethod::Unavailable;
                have_total_sigmas = false;
            }
        }
    }
    if !have_total_sigmas {
        total_method = TotalUncertaintyMethod::Unavailable;
    }

    let bundle = PhysicalDoseBundle {
        schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: plan.case_id.clone(),
        frame_of_reference_uid: first.frame_of_reference_uid.clone(),
        geometry: first.geometry.clone(),
        component_profile: first.component_profile.clone(),
        response_set: shared_response_set,
        components,
        physical_total: PhysicalTotalDoseVolume {
            unit: first.physical_total.unit,
            values: total_values,
            absolute_standard_uncertainty: have_total_sigmas
                .then(|| total_sigmas.iter().map(|sigma| sigma.sqrt()).collect()),
            uncertainty_method: total_method,
        },
        provenance_id: format!(
            "exposure-plan:sha256:{plan_sha256};covariance:independent_exposures"
        ),
    };
    bundle.validate()?;
    Ok(bundle)
}

#[derive(Debug, Error)]
pub enum ExposurePlanError {
    #[error("unsupported exposure-plan schema {0:?}")]
    UnsupportedSchema(String),
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("exposure plan contains no exposures")]
    NoExposures,
    #[error("exposure {0} occurs more than once")]
    DuplicateExposure(String),
    #[error("exposure {0} has a non-finite or negative weight")]
    InvalidWeight(String),
    #[error("exposure {0} has a non-finite or non-positive duration")]
    InvalidDuration(String),
    #[error("exposure {0} carries an invalid dose-bundle content reference")]
    InvalidBundleReference(String),
    #[error("plan declares {expected} exposures but {actual} bundles were supplied")]
    BundleCountMismatch { expected: usize, actual: usize },
    #[error("exposure {0} bundle geometry or frame of reference differs")]
    GeometryMismatch(String),
    #[error("exposure {0} binds a different component profile")]
    ComponentProfileMismatch(String),
    #[error("exposure {0} binds a different component set")]
    ComponentSetMismatch(String),
    #[error("exposure {0} uses a different dose unit")]
    DoseUnitMismatch(String),
    #[error(transparent)]
    Bundle(#[from] ValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DoseComponent, DoseUnit, GridGeometry};

    fn sha() -> String {
        "a".repeat(64)
    }

    fn bundle(scale: f64, sigma_scale: f64) -> PhysicalDoseBundle {
        let component = |component: DoseComponent, value: f64| DoseVolume {
            component,
            unit: DoseUnit::GrayPerSourceParticle,
            values: vec![value * scale, 2.0 * value * scale],
            absolute_standard_uncertainty: Some(vec![
                value * sigma_scale,
                2.0 * value * sigma_scale,
            ]),
        };
        PhysicalDoseBundle {
            schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "case-a".into(),
            frame_of_reference_uid: Some("1.2.3".into()),
            geometry: GridGeometry {
                shape: [2, 1, 1],
                spacing_mm: [10.0, 10.0, 10.0],
                origin_mm: [-5.0, 0.0, 0.0],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            component_profile: ContentReference {
                id: "profile".into(),
                sha256: sha(),
            },
            response_set: ContentReference {
                id: "response-set".into(),
                sha256: sha(),
            },
            components: vec![
                component(DoseComponent::Boron, 1.0e-12),
                component(DoseComponent::Nitrogen, 2.0e-13),
                component(DoseComponent::Hydrogen, 5.0e-13),
                component(DoseComponent::Photon, 3.0e-12),
            ],
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values: vec![4.8e-12 * scale, 9.6e-12 * scale],
                absolute_standard_uncertainty: Some(vec![
                    1.0e-14 * sigma_scale,
                    2.0e-14 * sigma_scale,
                ]),
                uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
            },
            provenance_id: "test".into(),
        }
    }

    fn plan(weights: &[f64]) -> ExposurePlan {
        ExposurePlan {
            schema_version: EXPOSURE_PLAN_SCHEMA.into(),
            id: "openbnct.test.exposure-plan.v1".into(),
            case_id: "accumulated-case".into(),
            covariance: ExposureCovariance::IndependentExposures,
            exposures: weights
                .iter()
                .enumerate()
                .map(|(index, weight)| Exposure {
                    name: format!("field-{index}"),
                    dose_bundle: BoundFileReference {
                        id: format!("bundle-{index}"),
                        sha256: sha(),
                        path: format!("bundle-{index}.json"),
                    },
                    weight: *weight,
                    weight_basis: WeightBasis::DeliveredFraction,
                    duration_s: Some(600.0),
                    boron_assumption: Some("10 ppm B-10 in CORE".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn accumulates_unequal_weights_with_quadrature_uncertainty() {
        let plan = plan(&[1.0, 0.5]);
        let bundles = [bundle(1.0, 1.0e-14), bundle(1.0, 1.0e-14)];
        let accumulated = accumulate_exposures(&plan, &sha(), &bundles).unwrap();
        assert_eq!(accumulated.case_id, "accumulated-case");
        let boron = &accumulated.components[0];
        // 1.0 * 1e-12 + 0.5 * 1e-12 per unit value.
        assert_eq!(boron.values, vec![1.5e-12, 3.0e-12]);
        // sqrt(1^2 + 0.25) * sigma -> 1.118x the single-exposure sigma.
        let sigma = &boron.absolute_standard_uncertainty.as_ref().unwrap()[0];
        let expected = 1.0e-14 * 1.0e-12 * (1.0_f64 + 0.25).sqrt();
        assert!((sigma - expected).abs() / expected < 1.0e-12);
        assert_eq!(
            accumulated.physical_total.uncertainty_method,
            TotalUncertaintyMethod::DedicatedEstimator
        );
        assert!(accumulated.provenance_id.contains("independent_exposures"));
    }

    #[test]
    fn zero_weight_exposure_contributes_nothing() {
        let plan = plan(&[1.0, 0.0]);
        let bundles = [bundle(1.0, 1.0e-14), bundle(9.0, 9.0e-14)];
        let accumulated = accumulate_exposures(&plan, &sha(), &bundles).unwrap();
        assert_eq!(accumulated.components[0].values, vec![1.0e-12, 2.0e-12]);
        assert_eq!(accumulated.physical_total.values, vec![4.8e-12, 9.6e-12]);
    }

    #[test]
    fn rejects_mismatched_grids_and_components() {
        let plan = plan(&[1.0, 1.0]);
        let mut other = bundle(1.0, 1.0e-14);
        other.geometry.spacing_mm = [5.0, 10.0, 10.0];
        assert!(matches!(
            accumulate_exposures(&plan, &sha(), &[bundle(1.0, 1.0e-14), other.clone()]),
            Err(ExposurePlanError::GeometryMismatch(_))
        ));
        let mut fewer = bundle(1.0, 1.0e-14);
        fewer.components.pop();
        // A bundle missing a required component fails its own validation
        // before the cross-exposure component-set check.
        assert!(matches!(
            accumulate_exposures(&plan, &sha(), &[bundle(1.0, 1.0e-14), fewer]),
            Err(ExposurePlanError::Bundle(
                ValidationError::MissingComponent(DoseComponent::Photon)
            ))
        ));
        let mut wrong_profile = bundle(1.0, 1.0e-14);
        wrong_profile.component_profile.sha256 = "b".repeat(64);
        assert!(matches!(
            accumulate_exposures(&plan, &sha(), &[bundle(1.0, 1.0e-14), wrong_profile]),
            Err(ExposurePlanError::ComponentProfileMismatch(_))
        ));
    }

    #[test]
    fn plan_validation_rejects_bad_inputs() {
        assert!(matches!(
            plan(&[]).validate(),
            Err(ExposurePlanError::NoExposures)
        ));
        assert!(matches!(
            plan(&[-1.0]).validate(),
            Err(ExposurePlanError::InvalidWeight(_))
        ));
        let mut duplicate = plan(&[1.0, 1.0]);
        duplicate.exposures[1].name = "field-0".into();
        assert!(matches!(
            duplicate.validate(),
            Err(ExposurePlanError::DuplicateExposure(_))
        ));
        let mut bad_schema = plan(&[1.0]);
        bad_schema.schema_version = "other/0.0.0".into();
        assert!(matches!(
            bad_schema.validate(),
            Err(ExposurePlanError::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn diagnostics_collect_every_issue() {
        let mut broken = plan(&[1.0, -2.0]);
        broken.id = "  ".into();
        broken.exposures[0].name = String::new();
        broken.exposures[0].dose_bundle.sha256 = "short".into();
        let issues = broken.validate_diagnostics();
        // empty id, empty exposure name, bad weight on field-1, bad ref on field-0
        assert_eq!(issues.len(), 4);
        assert!(issues.iter().any(|i| matches!(
            i,
            ExposurePlanError::EmptyIdentifier(label) if *label == "id"
        )));
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ExposurePlanError::InvalidWeight(name) if name == "field-1"))
        );
        assert!(
            issues
                .iter()
                .any(|i| matches!(i, ExposurePlanError::InvalidBundleReference(_)))
        );
        // validate() still reports the first problem only.
        assert!(broken.validate().is_err());
        assert!(plan(&[1.0]).validate_diagnostics().is_empty());
    }
}
