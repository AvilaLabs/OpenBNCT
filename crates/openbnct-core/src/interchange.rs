// SPDX-License-Identifier: MIT

//! Transport-neutral component-dose interchange.
//!
//! `openbnct.component-dose-interchange/0.1.0` is the published document any
//! external transport pipeline (MCNP, PHITS, Geant4, custom tools) emits to
//! hand NCTForge a component-resolved voxel dose. It carries everything the
//! authoritative `openbnct.physical-dose-bundle/0.2.0` contract needs while
//! keeping the producer's normalization and estimator semantics explicit:
//!
//! - geometry reuses the transport-neutral [`GridGeometry`];
//! - components reuse [`DoseVolume`] (all four [`DoseComponent`] kinds are
//!   required, in the bundle's `i + nx*j + nx*ny*k` grid order);
//! - `producer` names the external system, its version, and how tallies
//!   were normalized and folded;
//! - `total` is either a producer-tallied dedicated total or an explicit
//!   request to derive the total as the component sum — in which case no
//!   total uncertainty is claimed, because the importer cannot know the
//!   producer's component covariance;
//! - `component_profile`/`response_set` are producer-declared content
//!   references; when absent the importer binds the interchange document's
//!   own SHA-256 so provenance never silently dangles.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ComponentProfileReference, ContentReference, DoseComponent, DoseUnit, DoseVolume, GridGeometry,
    PHYSICAL_DOSE_BUNDLE_SCHEMA, PhysicalDoseBundle, PhysicalTotalDoseVolume,
    TotalUncertaintyMethod, ValidationError,
};

/// Schema identifier carried by every interchange document.
pub const COMPONENT_DOSE_INTERCHANGE_SCHEMA: &str = "openbnct.component-dose-interchange/0.1.0";

/// The external system that produced the dose and how it normalized it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalProducer {
    /// Transport system name — for example `mcnp`, `phits`, `geant4`, or a
    /// custom pipeline identifier.
    pub system: String,
    /// Producer version string (free text; not parsed).
    pub version: String,
    /// How the tallies were normalized and scored — for example the source
    /// particle count, folding tables, or estimator chain used.
    pub normalization: String,
}

/// How the bundle's `physical_total` is determined on import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalTotal {
    /// The producer tallied the physical total directly. `values` is
    /// required; `absolute_standard_uncertainty` is optional and, when
    /// present, is recorded with `dedicated_estimator` method provenance.
    Dedicated {
        values: Vec<f64>,
        absolute_standard_uncertainty: Option<Vec<f64>>,
    },
    /// Derive the total as the sum of the four components. The importer
    /// never claims a total uncertainty here: component covariance is the
    /// producer's domain knowledge, so `unavailable` is the honest record.
    ComponentSum,
}

/// The interchange document: everything needed to construct a
/// `PhysicalDoseBundle` without loss of meaning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDoseInterchange {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    pub frame_of_reference_uid: Option<String>,
    pub geometry: GridGeometry,
    pub producer: ExternalProducer,
    /// Component dose volumes in grid order. Exactly the four
    /// `DoseComponent::REQUIRED` kinds must appear, once each.
    pub components: Vec<DoseVolume>,
    /// Producer-declared component-definition reference; absent binds the
    /// document's own hash on import.
    pub component_profile: Option<ComponentProfileReference>,
    /// Producer-declared response/folding reference; absent binds the
    /// document's own hash on import.
    pub response_set: Option<ContentReference>,
    pub total: ExternalTotal,
}

impl ComponentDoseInterchange {
    pub fn validate(&self) -> Result<(), InterchangeError> {
        if !crate::schema_matches(&self.schema_version, COMPONENT_DOSE_INTERCHANGE_SCHEMA) {
            return Err(InterchangeError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        for (label, value) in [
            ("case_id", self.case_id.as_str()),
            ("producer.system", self.producer.system.as_str()),
            ("producer.version", self.producer.version.as_str()),
            (
                "producer.normalization",
                self.producer.normalization.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(InterchangeError::EmptyIdentifier(label));
            }
        }
        let voxel_count = self
            .geometry
            .voxel_count()
            .map_err(InterchangeError::Geometry)?;
        if self.components.is_empty() {
            return Err(InterchangeError::NoComponents);
        }
        let unit = self.components[0].unit;
        let mut observed = BTreeSet::new();
        for volume in &self.components {
            if !observed.insert(volume.component) {
                return Err(InterchangeError::DuplicateComponent(volume.component));
            }
            if volume.unit != unit {
                return Err(InterchangeError::InconsistentUnits {
                    component: volume.component,
                    component_unit: volume.unit,
                    expected: unit,
                });
            }
            if volume.values.len() != voxel_count {
                return Err(InterchangeError::DoseLength {
                    component: volume.component,
                    expected: voxel_count,
                    actual: volume.values.len(),
                });
            }
            if volume.values.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err(InterchangeError::InvalidDose(volume.component));
            }
            if let Some(sigma) = &volume.absolute_standard_uncertainty {
                if sigma.len() != voxel_count {
                    return Err(InterchangeError::UncertaintyLength {
                        component: volume.component,
                        expected: voxel_count,
                        actual: sigma.len(),
                    });
                }
                if sigma.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    return Err(InterchangeError::InvalidUncertainty(volume.component));
                }
            }
        }
        for required in DoseComponent::REQUIRED {
            if !observed.contains(&required) {
                return Err(InterchangeError::MissingComponent(required));
            }
        }
        if let ExternalTotal::Dedicated {
            values,
            absolute_standard_uncertainty,
        } = &self.total
        {
            if values.len() != voxel_count {
                return Err(InterchangeError::TotalLength {
                    expected: voxel_count,
                    actual: values.len(),
                });
            }
            if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err(InterchangeError::InvalidTotal);
            }
            if let Some(sigma) = absolute_standard_uncertainty {
                if sigma.len() != voxel_count {
                    return Err(InterchangeError::TotalUncertaintyLength {
                        expected: voxel_count,
                        actual: sigma.len(),
                    });
                }
                if sigma.iter().any(|v| !v.is_finite() || *v < 0.0) {
                    return Err(InterchangeError::InvalidTotalUncertainty);
                }
            }
        }
        for (label, reference) in [
            ("component_profile", &self.component_profile),
            ("response_set", &self.response_set),
        ] {
            if let Some(reference) = reference {
                reference
                    .validate()
                    .map_err(|_| InterchangeError::InvalidReference(label))?;
            }
        }
        Ok(())
    }

    /// The single unit shared by every component (checked by `validate`).
    fn unit(&self) -> DoseUnit {
        self.components[0].unit
    }
}

/// Loose grid comparison for producers that print identical meshes at finite
/// precision: exact shape equality plus a tight relative tolerance on spacing,
/// origin, and direction. Importers must use this rather than `==` on f64
/// fields, and must never resample disagreeing meshes into agreement.
pub fn grid_geometry_equivalent(a: &GridGeometry, b: &GridGeometry) -> bool {
    if a.shape != b.shape {
        return false;
    }
    let close = |x: f64, y: f64| (x - y).abs() <= 1e-6 * x.abs().max(y.abs()).max(1e-12);
    a.spacing_mm
        .iter()
        .zip(&b.spacing_mm)
        .all(|(x, y)| close(*x, *y))
        && a.origin_mm
            .iter()
            .zip(&b.origin_mm)
            .all(|(x, y)| close(*x, *y))
        && a.direction
            .iter()
            .zip(&b.direction)
            .all(|(x, y)| close(*x, *y))
}

/// Import an interchange document into a validated `PhysicalDoseBundle`.
///
/// `document_sha256` is the SHA-256 of the interchange document's bytes; it
/// anchors the output's `provenance_id` and stands in for any
/// producer-declared references that were left absent.
pub fn import_component_dose(
    document: &ComponentDoseInterchange,
    document_sha256: &str,
) -> Result<PhysicalDoseBundle, InterchangeError> {
    document.validate()?;
    let voxel_count = document
        .geometry
        .voxel_count()
        .map_err(InterchangeError::Geometry)?;
    let unit = document.unit();
    let physical_total = match &document.total {
        ExternalTotal::Dedicated {
            values,
            absolute_standard_uncertainty,
        } => PhysicalTotalDoseVolume {
            unit,
            values: values.clone(),
            absolute_standard_uncertainty: absolute_standard_uncertainty.clone(),
            uncertainty_method: if absolute_standard_uncertainty.is_some() {
                TotalUncertaintyMethod::DedicatedEstimator
            } else {
                TotalUncertaintyMethod::Unavailable
            },
        },
        ExternalTotal::ComponentSum => {
            let mut values = vec![0.0_f64; voxel_count];
            for volume in &document.components {
                for (index, value) in values.iter_mut().enumerate() {
                    *value += volume.values[index];
                }
            }
            if values.iter().any(|v| !v.is_finite()) {
                return Err(InterchangeError::InvalidTotal);
            }
            PhysicalTotalDoseVolume {
                unit,
                values,
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::Unavailable,
            }
        }
    };
    let fallback = || ContentReference {
        id: format!("external:{}", document.producer.system),
        sha256: document_sha256.to_owned(),
    };
    let bundle = PhysicalDoseBundle {
        schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
        case_id: document.case_id.clone(),
        frame_of_reference_uid: document.frame_of_reference_uid.clone(),
        geometry: document.geometry.clone(),
        component_profile: document.component_profile.clone().unwrap_or_else(fallback),
        response_set: document.response_set.clone().unwrap_or_else(fallback),
        components: document.components.clone(),
        physical_total,
        provenance_id: format!(
            "interchange:{}:sha256:{}",
            document.producer.system, document_sha256
        ),
    };
    bundle.validate().map_err(InterchangeError::Bundle)?;
    Ok(bundle)
}

#[derive(Debug, Error)]
pub enum InterchangeError {
    #[error("unsupported interchange schema {0:?}; expected {COMPONENT_DOSE_INTERCHANGE_SCHEMA:?}")]
    UnsupportedSchema(String),
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("interchange geometry: {0}")]
    Geometry(ValidationError),
    #[error("interchange document carries no components")]
    NoComponents,
    #[error("component {0:?} occurs more than once")]
    DuplicateComponent(DoseComponent),
    #[error("component {0:?} is required but absent")]
    MissingComponent(DoseComponent),
    #[error(
        "component {component:?} uses {component_unit:?} while earlier components use {expected:?}"
    )]
    InconsistentUnits {
        component: DoseComponent,
        component_unit: DoseUnit,
        expected: DoseUnit,
    },
    #[error("component {component:?} dose length {actual} != voxel count {expected}")]
    DoseLength {
        component: DoseComponent,
        expected: usize,
        actual: usize,
    },
    #[error("component {0:?} carries non-finite or negative dose")]
    InvalidDose(DoseComponent),
    #[error("component {component:?} uncertainty length {actual} != voxel count {expected}")]
    UncertaintyLength {
        component: DoseComponent,
        expected: usize,
        actual: usize,
    },
    #[error("component {0:?} carries non-finite or negative uncertainty")]
    InvalidUncertainty(DoseComponent),
    #[error("dedicated total length {actual} != voxel count {expected}")]
    TotalLength { expected: usize, actual: usize },
    #[error("dedicated total carries non-finite or negative dose")]
    InvalidTotal,
    #[error("dedicated total uncertainty length {actual} != voxel count {expected}")]
    TotalUncertaintyLength { expected: usize, actual: usize },
    #[error("dedicated total carries non-finite or negative uncertainty")]
    InvalidTotalUncertainty,
    #[error("producer-declared {0} is not a valid content reference")]
    InvalidReference(&'static str),
    #[error("imported bundle failed contract validation: {0}")]
    Bundle(ValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DoseComponent;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [2, 1, 1],
            spacing_mm: [10.0, 10.0, 10.0],
            origin_mm: [-5.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn component(component: DoseComponent, value: f64) -> DoseVolume {
        DoseVolume {
            component,
            unit: DoseUnit::GrayPerSourceParticle,
            values: vec![value, 2.0 * value],
            absolute_standard_uncertainty: Some(vec![0.01 * value, 0.02 * value]),
        }
    }

    fn document(total: ExternalTotal) -> ComponentDoseInterchange {
        ComponentDoseInterchange {
            schema_version: COMPONENT_DOSE_INTERCHANGE_SCHEMA.into(),
            case_id: "external-case".into(),
            frame_of_reference_uid: Some("9.8.7".into()),
            geometry: geometry(),
            producer: ExternalProducer {
                system: "phits".into(),
                version: "3.34".into(),
                normalization: "per source particle; F6 kerma tallies".into(),
            },
            components: vec![
                component(DoseComponent::Boron, 1.0e-12),
                component(DoseComponent::Nitrogen, 2.0e-13),
                component(DoseComponent::Hydrogen, 5.0e-13),
                component(DoseComponent::Photon, 3.0e-12),
            ],
            component_profile: None,
            response_set: None,
            total,
        }
    }

    #[test]
    fn dedicated_total_imports_with_estimator_provenance() {
        let doc = document(ExternalTotal::Dedicated {
            values: vec![4.0e-12, 8.0e-12],
            absolute_standard_uncertainty: Some(vec![4.0e-14, 8.0e-14]),
        });
        let bundle = import_component_dose(&doc, &"f".repeat(64)).unwrap();
        assert_eq!(bundle.physical_total.values, vec![4.0e-12, 8.0e-12]);
        assert_eq!(
            bundle.physical_total.uncertainty_method,
            TotalUncertaintyMethod::DedicatedEstimator
        );
        // Absent producer references bind the document's own hash.
        assert_eq!(bundle.component_profile.sha256, "f".repeat(64));
        assert_eq!(bundle.response_set.id, "external:phits");
        assert!(bundle.provenance_id.contains("interchange:phits:"));
        assert_eq!(bundle.frame_of_reference_uid.as_deref(), Some("9.8.7"));
    }

    #[test]
    fn component_sum_derives_total_without_claimed_uncertainty() {
        let doc = document(ExternalTotal::ComponentSum);
        let bundle = import_component_dose(&doc, &"f".repeat(64)).unwrap();
        // 1e-12 + 2e-13 + 5e-13 + 3e-12 = 4.7e-12 per unit value.
        for (actual, expected) in bundle.physical_total.values.iter().zip([4.7e-12, 9.4e-12]) {
            assert!((actual - expected).abs() / expected < 1.0e-12);
        }
        assert_eq!(bundle.physical_total.absolute_standard_uncertainty, None);
        assert_eq!(
            bundle.physical_total.uncertainty_method,
            TotalUncertaintyMethod::Unavailable
        );
    }

    #[test]
    fn dedicated_total_without_sigma_marks_unavailable() {
        let doc = document(ExternalTotal::Dedicated {
            values: vec![4.0e-12, 8.0e-12],
            absolute_standard_uncertainty: None,
        });
        let bundle = import_component_dose(&doc, &"f".repeat(64)).unwrap();
        assert_eq!(
            bundle.physical_total.uncertainty_method,
            TotalUncertaintyMethod::Unavailable
        );
    }

    #[test]
    fn rejects_missing_component_bad_units_and_short_totals() {
        let mut doc = document(ExternalTotal::ComponentSum);
        doc.components.pop();
        assert!(matches!(
            import_component_dose(&doc, &"f".repeat(64)),
            Err(InterchangeError::MissingComponent(DoseComponent::Photon))
        ));

        let mut doc = document(ExternalTotal::ComponentSum);
        doc.components[1].unit = DoseUnit::Gray;
        assert!(matches!(
            import_component_dose(&doc, &"f".repeat(64)),
            Err(InterchangeError::InconsistentUnits { .. })
        ));

        let mut doc = document(ExternalTotal::Dedicated {
            values: vec![1.0],
            absolute_standard_uncertainty: None,
        });
        doc.components[0].values = vec![-1.0, 1.0];
        assert!(matches!(
            import_component_dose(&doc, &"f".repeat(64)),
            Err(InterchangeError::InvalidDose(DoseComponent::Boron))
        ));

        let mut doc = document(ExternalTotal::Dedicated {
            values: vec![1.0],
            absolute_standard_uncertainty: None,
        });
        doc.components[0].values = vec![1.0, 1.0];
        assert!(matches!(
            import_component_dose(&doc, &"f".repeat(64)),
            Err(InterchangeError::TotalLength {
                expected: 2,
                actual: 1
            })
        ));
    }
}
