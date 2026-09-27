// SPDX-License-Identifier: MIT

//! Biological-parameter evidence library
//! (`openbnct.bio-evidence-library/0.1.0`).
//!
//! The library stores *records* — one parameter estimate with its
//! experimental context, uncertainty semantics, extraction provenance,
//! and applicability limits — rather than a single averaged default.
//! Conflicting study results coexist; nothing is averaged into one
//! number. `search` returns each record's applicability to a declared
//! context as exact/partial/unsupported, and `to_biological_model`
//! refuses unsupported transfers outright and requires a declared
//! research assumption for every partial one — a mouse endpoint is
//! never silently applied to a human model.
//!
//! Uncertainty semantics are strict: an SD stays an SD (and only an SD
//! maps onto a model's `component_weight_uncertainty`); an SE cannot be
//! realized as a sampling distribution without design information;
//! unavailable uncertainty stays unavailable, visibly.

use std::collections::BTreeSet;

use openbnct_core::{
    Distribution, JointSource, SourceEvidence, SourceSharing, SourceTarget, Support,
    UncertaintyCategory,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{BiologicalModel, DoseUnit, WeightMap, WeightSemantics};

/// Contract token for [`BioEvidenceLibrary`].
pub const BIO_EVIDENCE_LIBRARY_SCHEMA: &str = "openbnct.bio-evidence-library/0.1.0";

/// How the recorded value was obtained. `synthetic_fixture` separates
/// conformance constants from experimental parameter records — a test
/// fixture must never masquerade as measured biology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    Measured,
    Fitted,
    Transferred,
    Illustrative,
    Assumed,
    SyntheticFixture,
}

/// The kind of uncertainty statement attached to an estimate. SD, SE,
/// CI, and range are *different* statistics and stay distinct — no
/// cross-conversion without the design information to justify it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EstimateUncertainty {
    /// Standard deviation of the measured/estimated quantity.
    StandardDeviation { sigma: f64 },
    /// Standard error of an estimator — not a population spread.
    StandardError { se: f64 },
    /// Confidence/coverage interval at the stated level.
    ConfidenceInterval { level: f64, low: f64, high: f64 },
    /// Reported population/observation range.
    PopulationRange { low: f64, high: f64 },
    /// The source reports no uncertainty; kept explicit.
    #[default]
    Unavailable,
}

/// One parameter's point estimate, unit, and uncertainty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterEstimate {
    pub value: f64,
    /// Physical unit; `"1"` for dimensionless weights/ratios.
    pub unit: String,
    #[serde(default)]
    pub uncertainty: EstimateUncertainty,
}

/// The experimental/biological context a record applies to.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentalContext {
    /// Compound/drug (`BPA`, `BSH`, `none` for inherent factors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compound: Option<String>,
    /// Formulation (`BPA-fructose`, `borocaptate sodium`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formulation: Option<String>,
    /// Isotope convention (`10B`, `natural-boron`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isotope_convention: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tissue: Option<String>,
    /// Disease/lesion context (`glioma`, `melanoma`, `healthy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disease_context: Option<String>,
    /// Biological endpoint the value was assessed against
    /// (`cell-survival D10`, `skin-reaction`, `protocol-convention`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Assessment time when time-sensitive (`72 h`, `acute`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment_time: Option<String>,
    /// Reference radiation (`photon`, `x-ray 250 kVp`, `none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_radiation: Option<String>,
    /// Assumed boron microdistribution (`uniform`, `nucleus-loaded`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microdistribution: Option<String>,
}

/// Correlated-parameter covariance reported by the source — preserved,
/// not discarded, when the study gives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointUncertainty {
    /// Parameter names in matrix order.
    pub parameters: Vec<String>,
    /// Row-major `n×n` covariance.
    pub covariance: Vec<f64>,
}

/// Where the value came from and how it entered the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractionRecord {
    /// DOI or URL of the primary source. Required for anything that is
    /// not a synthetic fixture.
    pub source: String,
    /// Location inside the source (`Table 2`, `Eq. (4)`, `p. 169`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// How the number was extracted (`manual transcription`,
    /// `digitized figure`, `protocol encoding`).
    pub method: String,
    /// Sample size when the source reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_size: Option<u32>,
    /// Source population (`FiR 1 glioma series n=…`, `in-vitro HSG`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub population: Option<String>,
    pub value_kind: ValueKind,
    /// Who or what entered the record (`author id`, `tool name`).
    pub extracted_by: String,
    /// Independent reviewer. Absent means not yet reviewed — one AI
    /// extraction does not count as expert review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_by: Option<String>,
}

/// One parameter evidence record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterRecord {
    pub id: String,
    /// What the record supplies (`cbe`, `rbe`, `alpha_beta`,
    /// `tumor_blood_ratio`, `pk_half_life`, `lineal_energy`, ...).
    pub parameter: String,
    /// Dose component the value applies to (`boron`, `nitrogen`,
    /// `hydrogen`, `photon`) when it is a component weight or factor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Model family the value belongs to
    /// (`openbnct.biological-model/0.2.0`, `openbnct.pk-model/0.1.0`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_family: Option<String>,
    #[serde(default)]
    pub context: ExperimentalContext,
    /// Dose-rate / fractionation / repair assumptions as free text —
    /// kept visible rather than flattened into a flag.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<String>,
    pub estimate: ParameterEstimate,
    /// Reported joint covariance with sibling parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub joint: Option<JointUncertainty>,
    /// Applicability limits carried verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applicability_limits: Vec<String>,
    pub provenance: ExtractionRecord,
}

/// The library document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BioEvidenceLibrary {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Qualification boundary string (`synthetic_research_only`, ...) —
    /// a free string like [`crate::BiologicalDoseBundle::qualification`].
    pub qualification: String,
    pub provenance_id: String,
    pub records: Vec<ParameterRecord>,
}

/// How a record applies to a requested context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applicability {
    /// Every queried field matches the record.
    Exact,
    /// No hard mismatch, but the record differs on or omits a queried
    /// field — usable only under a declared research assumption.
    Partial(Vec<String>),
    /// A hard mismatch — compound or species differs. The record cannot
    /// answer this query.
    Unsupported(Vec<String>),
}

/// The context a consumer asks about.
#[derive(Debug, Clone, Default)]
pub struct ContextQuery {
    pub compound: Option<String>,
    pub species: Option<String>,
    pub tissue: Option<String>,
    pub endpoint: Option<String>,
    pub model_family: Option<String>,
}

/// Evaluate one record against a query.
pub fn applicability(record: &ParameterRecord, query: &ContextQuery) -> Applicability {
    let mut partial = Vec::new();
    let mut unsupported = Vec::new();
    // Hard mismatches: compound and species — biology does not transfer.
    for (label, queried, recorded) in [
        ("compound", &query.compound, &record.context.compound),
        ("species", &query.species, &record.context.species),
    ] {
        match (queried, recorded) {
            (Some(q), Some(r)) if q != r => {
                unsupported.push(format!("{label}: requested {q:?}, record declares {r:?}"));
            }
            (Some(q), None) => {
                partial.push(format!(
                    "{label}: record does not declare it (queried {q:?})"
                ));
            }
            _ => {}
        }
    }
    // Soft mismatches: tissue/endpoint/model family — transferable only
    // under a declared assumption.
    for (label, queried, recorded) in [
        ("tissue", &query.tissue, &record.context.tissue),
        ("endpoint", &query.endpoint, &record.context.endpoint),
    ] {
        match (queried, recorded) {
            (Some(q), Some(r)) if q != r => {
                partial.push(format!("{label}: requested {q:?}, record declares {r:?}"));
            }
            (Some(q), None) => {
                partial.push(format!(
                    "{label}: record does not declare it (queried {q:?})"
                ));
            }
            _ => {}
        }
    }
    match (&query.model_family, &record.model_family) {
        (Some(q), Some(r)) if q != r => {
            unsupported.push(format!(
                "model_family: requested {q:?}, record declares {r:?}"
            ));
        }
        (Some(q), None) => partial.push(format!(
            "model_family: record does not declare it (queried {q:?})"
        )),
        _ => {}
    }
    if !unsupported.is_empty() {
        Applicability::Unsupported(unsupported)
    } else if !partial.is_empty() {
        Applicability::Partial(partial)
    } else {
        Applicability::Exact
    }
}

/// Filtered view: `(record, applicability)` for every record in the
/// library, best applicability first.
pub fn search<'a>(
    library: &'a BioEvidenceLibrary,
    query: &ContextQuery,
) -> Vec<(&'a ParameterRecord, Applicability)> {
    let mut hits: Vec<_> = library
        .records
        .iter()
        .map(|record| (record, applicability(record, query)))
        .collect();
    hits.sort_by_key(|(_, a)| match a {
        Applicability::Exact => 0,
        Applicability::Partial(_) => 1,
        Applicability::Unsupported(_) => 2,
    });
    hits
}

/// Convert a set of `(component_name, record)` bindings into a
/// weight-based [`BiologicalModel`]. Every component must be bound to a
/// record whose `parameter` is `cbe`/`rbe`-family and whose unit is
/// `"1"`; partial-applicability bindings require the caller to declare
/// the assumption, and unsupported bindings are rejected.
///
/// Only `StandardDeviation` uncertainties map onto
/// `component_weight_uncertainty`; other uncertainty kinds carry no
/// defensible conversion and are left unset — visible, not invented.
pub fn to_biological_model(
    id: &str,
    semantics: WeightSemantics,
    input_unit: DoseUnit,
    bindings: &[(&str, &ParameterRecord)],
    context: &ContextQuery,
    assumptions: &[String],
    region_bindings: &[(String, String, &ParameterRecord)],
) -> Result<BiologicalModel, EvidenceError> {
    // Shared per-record gate: component agreement, dimensionless unit,
    // and context applicability — partial transfers demand a declared
    // assumption; unsupported ones fail.
    let check = |component: &str, record: &ParameterRecord| -> Result<f64, EvidenceError> {
        if record
            .component
            .as_deref()
            .is_some_and(|declared| declared != component)
        {
            return Err(EvidenceError::Invalid(format!(
                "record {:?} declares component {:?}, bound as {component:?}",
                record.id, record.component
            )));
        }
        if record.estimate.unit != "1" {
            return Err(EvidenceError::Invalid(format!(
                "record {:?} has unit {:?}; component weights must be dimensionless",
                record.id, record.estimate.unit
            )));
        }
        match applicability(record, context) {
            Applicability::Exact => {}
            Applicability::Partial(reasons) => {
                if assumptions.is_empty() {
                    return Err(EvidenceError::AssumptionRequired(format!(
                        "record {:?} only partially applies ({}); declare the transfer assumption",
                        record.id,
                        reasons.join("; ")
                    )));
                }
            }
            Applicability::Unsupported(reasons) => {
                return Err(EvidenceError::Inapplicable {
                    record: record.id.clone(),
                    reasons,
                });
            }
        }
        Ok(record.estimate.value)
    };
    let mut weights = WeightMap::new();
    let mut weight_uncertainty = std::collections::BTreeMap::new();
    for (component, record) in bindings {
        weights.insert((*component).to_string(), check(component, record)?);
        if let EstimateUncertainty::StandardDeviation { sigma } = record.estimate.uncertainty
            && record.estimate.value > 0.0
        {
            weight_uncertainty.insert((*component).to_string(), sigma / record.estimate.value);
        }
    }
    // Region maps must be complete four-component tables; start each
    // from the global map and apply the region-specific overrides.
    let mut region_weights: std::collections::BTreeMap<String, WeightMap> = Default::default();
    for (region, component, record) in region_bindings {
        region_weights
            .entry(region.clone())
            .or_insert_with(|| weights.clone())
            .insert(component.clone(), check(component, record)?);
    }
    let model = BiologicalModel {
        schema_version: crate::BIOLOGICAL_MODEL_SCHEMA.into(),
        id: id.into(),
        weight_semantics: semantics,
        input_unit,
        component_weights: weights,
        region_weights,
        region_priority: Vec::new(),
        component_weight_uncertainty: weight_uncertainty,
        derivation: None,
        validity_domain: if assumptions.is_empty() {
            None
        } else {
            Some(format!(
                "declared transfer assumptions: {}",
                assumptions.join("; ")
            ))
        },
        fractionation: None,
    };
    model
        .validate()
        .map_err(|e| EvidenceError::Invalid(e.to_string()))?;
    Ok(model)
}

/// Realize one record's estimate as a UQ-01 [`JointSource`]. An SD
/// becomes a Normal or LogNormal draw (the latter for positive
/// support); a population range becomes a bounded Uniform. An SE or CI
/// cannot be turned into a sampling distribution without design
/// information — and `Unavailable` produces a point draw only when the
/// caller explicitly opts in via `allow_point`.
pub fn to_joint_source(
    record: &ParameterRecord,
    scope: Vec<SourceTarget>,
    sharing: SourceSharing,
    category: UncertaintyCategory,
    support: Support,
    allow_point: bool,
) -> Result<JointSource, EvidenceError> {
    let value = record.estimate.value;
    let distribution = match record.estimate.uncertainty {
        EstimateUncertainty::StandardDeviation { sigma } => match support {
            Support::Real => Distribution::Normal {
                mean: value,
                std_dev: sigma,
            },
            _ => {
                if value <= 0.0 {
                    return Err(EvidenceError::Invalid(format!(
                        "record {:?}: positive-supported draw needs a positive estimate",
                        record.id
                    )));
                }
                let rel = sigma / value;
                let sigma_log = (1.0_f64 + rel * rel).ln().sqrt();
                let median = value / (1.0_f64 + rel * rel).sqrt();
                Distribution::LogNormal { median, sigma_log }
            }
        },
        EstimateUncertainty::PopulationRange { low, high } => {
            if low.partial_cmp(&high) != Some(std::cmp::Ordering::Less) {
                return Err(EvidenceError::Invalid(format!(
                    "record {:?}: population range must satisfy low < high",
                    record.id
                )));
            }
            Distribution::Uniform { low, high }
        }
        EstimateUncertainty::StandardError { .. }
        | EstimateUncertainty::ConfidenceInterval { .. } => {
            return Err(EvidenceError::UncovertibleUncertainty(format!(
                "record {:?}: {:?} cannot be realized as a sampling distribution without design information",
                record.id, record.estimate.uncertainty
            )));
        }
        EstimateUncertainty::Unavailable => {
            if !allow_point {
                return Err(EvidenceError::UncovertibleUncertainty(format!(
                    "record {:?} declares no uncertainty; refusing an implicit point draw",
                    record.id
                )));
            }
            Distribution::Point { value }
        }
    };
    let source = JointSource {
        id: record.id.clone(),
        category,
        scope,
        sharing,
        unit: record.estimate.unit.clone(),
        support,
        distribution,
        evidence: SourceEvidence {
            basis: format!(
                "bio-evidence record {:?}: {} ({})",
                record.id, record.parameter, record.provenance.source
            ),
            reference: None,
        },
    };
    Ok(source)
}

#[derive(Debug, Error)]
pub enum EvidenceError {
    #[error("unsupported bio-evidence schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid bio-evidence record: {0}")]
    Invalid(String),
    #[error("record {record:?} does not apply: {}", reasons.join("; "))]
    Inapplicable {
        record: String,
        reasons: Vec<String>,
    },
    #[error("partial-context transfer needs a declared assumption: {0}")]
    AssumptionRequired(String),
    #[error("uncertainty cannot be converted: {0}")]
    UncovertibleUncertainty(String),
}

impl BioEvidenceLibrary {
    pub fn validate(&self) -> Result<(), EvidenceError> {
        if !openbnct_core::schema_matches(&self.schema_version, BIO_EVIDENCE_LIBRARY_SCHEMA) {
            return Err(EvidenceError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() || self.provenance_id.trim().is_empty() {
            return Err(EvidenceError::Invalid(
                "id and provenance_id are required".into(),
            ));
        }
        let mut ids = BTreeSet::new();
        for record in &self.records {
            if record.id.trim().is_empty() || record.parameter.trim().is_empty() {
                return Err(EvidenceError::Invalid(
                    "record id and parameter are required".into(),
                ));
            }
            if !ids.insert(record.id.as_str()) {
                return Err(EvidenceError::Invalid(format!(
                    "duplicate record id {:?}",
                    record.id
                )));
            }
            if record.estimate.unit.trim().is_empty() || !record.estimate.value.is_finite() {
                return Err(EvidenceError::Invalid(format!(
                    "record {:?} needs a finite value and a unit",
                    record.id
                )));
            }
            match record.estimate.uncertainty {
                EstimateUncertainty::StandardDeviation { sigma }
                    if !(sigma.is_finite() && sigma >= 0.0) =>
                {
                    return Err(EvidenceError::Invalid(format!(
                        "record {:?}: standard_deviation sigma must be finite, non-negative",
                        record.id
                    )));
                }
                EstimateUncertainty::StandardError { se } if !(se.is_finite() && se >= 0.0) => {
                    return Err(EvidenceError::Invalid(format!(
                        "record {:?}: standard_error must be finite, non-negative",
                        record.id
                    )));
                }
                EstimateUncertainty::ConfidenceInterval { level, low, high }
                    if !(level > 0.0
                        && level < 1.0
                        && low.is_finite()
                        && high.is_finite()
                        && low <= high) =>
                {
                    return Err(EvidenceError::Invalid(format!(
                        "record {:?}: confidence_interval needs 0<level<1 and low<=high",
                        record.id
                    )));
                }
                EstimateUncertainty::PopulationRange { low, high }
                    if !(low.is_finite() && high.is_finite() && low <= high) =>
                {
                    return Err(EvidenceError::Invalid(format!(
                        "record {:?}: population_range needs low <= high",
                        record.id
                    )));
                }
                _ => {}
            }
            let p = &record.provenance;
            if p.extracted_by.trim().is_empty() || p.method.trim().is_empty() {
                return Err(EvidenceError::Invalid(format!(
                    "record {:?}: provenance needs method and extracted_by",
                    record.id
                )));
            }
            if p.value_kind != ValueKind::SyntheticFixture && p.source.trim().is_empty() {
                return Err(EvidenceError::Invalid(format!(
                    "record {:?}: non-synthetic records need a primary source",
                    record.id
                )));
            }
            if let Some(joint) = &record.joint {
                let n = joint.parameters.len();
                if n == 0 || joint.covariance.len() != n * n {
                    return Err(EvidenceError::Invalid(format!(
                        "record {:?}: joint covariance must be {}×{}",
                        record.id, n, n
                    )));
                }
                for i in 0..n {
                    if !joint.covariance[i * n + i].is_finite() || joint.covariance[i * n + i] < 0.0
                    {
                        return Err(EvidenceError::Invalid(format!(
                            "record {:?}: joint covariance diagonal must be finite, non-negative",
                            record.id
                        )));
                    }
                    for j in (i + 1)..n {
                        if joint.covariance[i * n + j] != joint.covariance[j * n + i] {
                            return Err(EvidenceError::Invalid(format!(
                                "record {:?}: joint covariance must be symmetric",
                                record.id
                            )));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn record(&self, id: &str) -> Option<&ParameterRecord> {
        self.records.iter().find(|r| r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_name;

    fn record(
        id: &str,
        parameter: &str,
        component: Option<&str>,
        species: &str,
    ) -> ParameterRecord {
        ParameterRecord {
            id: id.into(),
            parameter: parameter.into(),
            component: component.map(str::to_string),
            model_family: Some(crate::BIOLOGICAL_MODEL_SCHEMA.into()),
            context: ExperimentalContext {
                compound: Some("BPA".into()),
                species: Some(species.into()),
                tissue: Some("tumor".into()),
                endpoint: Some("cell-survival D10".into()),
                reference_radiation: Some("photon".into()),
                ..Default::default()
            },
            conditions: Vec::new(),
            estimate: ParameterEstimate {
                value: 3.8,
                unit: "1".into(),
                uncertainty: EstimateUncertainty::StandardDeviation { sigma: 0.5 },
            },
            joint: None,
            applicability_limits: Vec::new(),
            provenance: ExtractionRecord {
                source: "doi:10.test/fixture".into(),
                location: Some("Table 1".into()),
                method: "manual transcription".into(),
                sample_size: Some(12),
                population: None,
                value_kind: ValueKind::Measured,
                extracted_by: "test".into(),
                reviewed_by: None,
            },
        }
    }

    #[test]
    fn species_mismatch_is_unsupported_not_silent() {
        let mouse = record("r.mouse", "cbe", Some("boron"), "mouse");
        let query = ContextQuery {
            compound: Some("BPA".into()),
            species: Some("human".into()),
            ..Default::default()
        };
        assert!(matches!(
            applicability(&mouse, &query),
            Applicability::Unsupported(_)
        ));
        let human = record("r.human", "cbe", Some("boron"), "human");
        assert_eq!(applicability(&human, &query), Applicability::Exact);
    }

    #[test]
    fn to_model_requires_assumption_for_partial_transfer() {
        let mut skin = record("r.skin", "cbe", Some("boron"), "human");
        skin.context.tissue = Some("skin".into());
        let query = ContextQuery {
            species: Some("human".into()),
            tissue: Some("tumor".into()),
            ..Default::default()
        };
        assert!(
            to_biological_model(
                "m",
                WeightSemantics::PhotonIsoeffective,
                DoseUnit::GrayPerSourceParticle,
                &[("boron", &skin)],
                &query,
                &[],
                &[],
            )
            .is_err()
        );
        // With a declared assumption the partial transfer proceeds —
        // and the assumption lands in the model's validity_domain.
        let mut photon_rec = record("r.p", "rbe", Some("photon"), "human");
        photon_rec.estimate.value = 1.0;
        photon_rec.estimate.uncertainty = EstimateUncertainty::Unavailable;
        let mut nitrogen_rec = record("r.n", "rbe", Some("nitrogen"), "human");
        nitrogen_rec.estimate.value = 3.2;
        nitrogen_rec.estimate.uncertainty = EstimateUncertainty::Unavailable;
        let mut hydrogen_rec = record("r.h", "rbe", Some("hydrogen"), "human");
        hydrogen_rec.estimate.value = 3.2;
        hydrogen_rec.estimate.uncertainty = EstimateUncertainty::Unavailable;
        let query2 = ContextQuery {
            species: Some("human".into()),
            tissue: Some("tumor".into()),
            endpoint: None,
            ..Default::default()
        };
        // A tumor-context record lands in the region override — the
        // global boron weight stays the transferred skin value.
        let boron_tumor = record("r.bt", "cbe", Some("boron"), "human");
        let model = to_biological_model(
            "m",
            WeightSemantics::PhotonIsoeffective,
            DoseUnit::GrayPerSourceParticle,
            &[
                ("boron", &skin),
                ("nitrogen", &nitrogen_rec),
                ("hydrogen", &hydrogen_rec),
                ("photon", &photon_rec),
            ],
            &query2,
            &["skin CBE transferred to tumor as a research assumption".into()],
            &[("tumor".to_string(), "boron".to_string(), &boron_tumor)],
        )
        .unwrap();
        assert_eq!(model.component_weights["boron"], 3.8);
        assert!(model.validity_domain.unwrap().contains("transfer"));
        // Region map inherited the global weights, overriding boron.
        let tumor = &model.region_weights["tumor"];
        assert_eq!(tumor["boron"], 3.8);
        assert_eq!(tumor["nitrogen"], 3.2);
        // SD → relative weight uncertainty; missing stays absent.
        assert!((model.component_weight_uncertainty["boron"] - 0.5 / 3.8).abs() < 1e-12);
    }

    #[test]
    fn joint_source_respects_uncertainty_kinds() {
        // SD → Normal on real support.
        let sd_rec = record("r.sd", "cbe", Some("boron"), "human");
        let src = to_joint_source(
            &sd_rec,
            vec![SourceTarget::Component {
                name: component_name(openbnct_core::DoseComponent::Boron).into(),
            }],
            SourceSharing::Shared,
            UncertaintyCategory::InputParameter,
            Support::Real,
            false,
        )
        .unwrap();
        assert!(matches!(
            src.distribution,
            Distribution::Normal { mean, std_dev } if mean == 3.8 && std_dev == 0.5
        ));

        // SE cannot be realized without design information.
        let mut se_rec = sd_rec.clone();
        se_rec.estimate.uncertainty = EstimateUncertainty::StandardError { se: 0.5 };
        assert!(
            to_joint_source(
                &se_rec,
                vec![SourceTarget::Global],
                SourceSharing::Shared,
                UncertaintyCategory::InputParameter,
                Support::Real,
                false,
            )
            .is_err()
        );

        // Range → bounded Uniform.
        let mut range_rec = sd_rec.clone();
        range_rec.estimate.uncertainty = EstimateUncertainty::PopulationRange {
            low: 3.0,
            high: 4.6,
        };
        let src = to_joint_source(
            &range_rec,
            vec![SourceTarget::Global],
            SourceSharing::Shared,
            UncertaintyCategory::InputParameter,
            Support::Bounded {
                low: 3.0,
                high: 4.6,
            },
            false,
        )
        .unwrap();
        assert!(matches!(
            src.distribution,
            Distribution::Uniform {
                low: 3.0,
                high: 4.6
            }
        ));

        // Unavailable uncertainty refuses an implicit point draw.
        let mut bare = sd_rec.clone();
        bare.estimate.uncertainty = EstimateUncertainty::Unavailable;
        assert!(
            to_joint_source(
                &bare,
                vec![SourceTarget::Global],
                SourceSharing::Shared,
                UncertaintyCategory::InputParameter,
                Support::Real,
                false,
            )
            .is_err()
        );
        let src = to_joint_source(
            &bare,
            vec![SourceTarget::Global],
            SourceSharing::Shared,
            UncertaintyCategory::InputParameter,
            Support::Real,
            true,
        )
        .unwrap();
        assert!(matches!(src.distribution, Distribution::Point { .. }));
    }

    #[test]
    fn converted_model_agrees_with_weighted_sum_fixture() {
        // Records reproduce the committed FiR-1 CBE protocol weights;
        // applying the emitted model must reproduce the fixture's
        // arithmetic: w·d per component.
        let mut boron_rec = record("r.b", "cbe", Some("boron"), "human");
        boron_rec.estimate.value = 3.8;
        let mut n_rec = record("r.n", "rbe", Some("nitrogen"), "human");
        n_rec.estimate.value = 3.2;
        n_rec.estimate.uncertainty = EstimateUncertainty::Unavailable;
        let mut h_rec = record("r.h", "rbe", Some("hydrogen"), "human");
        h_rec.estimate.value = 3.2;
        h_rec.estimate.uncertainty = EstimateUncertainty::Unavailable;
        let mut p_rec = record("r.p", "rbe", Some("photon"), "human");
        p_rec.estimate.value = 1.0;
        p_rec.estimate.uncertainty = EstimateUncertainty::Unavailable;

        let model = to_biological_model(
            "evidence-model",
            WeightSemantics::PhotonIsoeffective,
            DoseUnit::GrayPerSourceParticle,
            &[
                ("boron", &boron_rec),
                ("nitrogen", &n_rec),
                ("hydrogen", &h_rec),
                ("photon", &p_rec),
            ],
            &ContextQuery::default(),
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(model.component_weights["boron"], 3.8);
        assert_eq!(model.component_weights["photon"], 1.0);
        // Only boron carried an SD — the others' uncertainty stays absent.
        assert_eq!(model.component_weight_uncertainty.len(), 1);
    }

    #[test]
    fn library_validates_and_preserves_conflicts() {
        let mut other = record("r.other", "cbe", Some("boron"), "human");
        other.estimate.value = 4.6; // conflicting study result — both kept.
        let lib = BioEvidenceLibrary {
            schema_version: BIO_EVIDENCE_LIBRARY_SCHEMA.into(),
            id: "test-lib".into(),
            qualification: "synthetic_research_only".into(),
            provenance_id: "test".into(),
            records: vec![
                record("r.a", "cbe", Some("boron"), "human"),
                record("r.b", "cbe", Some("boron"), "human"),
                other,
            ],
        };
        lib.validate().unwrap();
        // Duplicate ids rejected.
        let mut dup = lib.clone();
        dup.records[2].id = "r.a".into();
        assert!(dup.validate().is_err());
        // Non-synthetic records need a source.
        let mut bare = lib.clone();
        bare.records[0].provenance.source = String::new();
        assert!(bare.validate().is_err());
        // Conflicting values coexist — search returns both.
        let hits = search(&lib, &ContextQuery::default());
        assert_eq!(hits.len(), 3);
    }
}
