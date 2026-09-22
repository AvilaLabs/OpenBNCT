// SPDX-License-Identifier: MIT

use std::collections::BTreeSet;

use openbnct_core::{ContentReference, DoseComponent};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const JOULE_PER_EV: f64 = 1.602_176_634e-19;

/// Stable semantic and contributor profile for the four physical components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDefinitionProfile {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub spatial_model: SpatialDoseModel,
    pub source_normalization: SourceNormalization,
    pub neutron_response: NeutronResponseSemantics,
    pub components: Vec<ComponentRule>,
    pub physical_total: PhysicalTotalEstimator,
}

impl ComponentDefinitionProfile {
    pub fn validate(&self) -> Result<(), ResponseMethodError> {
        validate_identifier("component_profile.schema_version", &self.schema_version)?;
        validate_identifier("component_profile.id", &self.id)?;

        let mut observed = BTreeSet::new();
        for rule in &self.components {
            if !observed.insert(rule.component) {
                return Err(ResponseMethodError::DuplicateComponent(rule.component));
            }
            validate_component_rule(rule)?;
        }
        for component in DoseComponent::REQUIRED {
            if !observed.contains(&component) {
                return Err(ResponseMethodError::MissingComponent(component));
            }
        }
        if self.components.len() != DoseComponent::REQUIRED.len() {
            return Err(ResponseMethodError::UnexpectedComponentCount(
                self.components.len(),
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialDoseModel {
    MacroscopicLocalChargedParticleKerma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceNormalization {
    PerUnitWeightSourceNeutron,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeutronResponseSemantics {
    pub unit: ResponseUnit,
    pub interpolation: ResponseInterpolation,
    pub fold_normalization: FoldNormalization,
    pub outside_domain: OutsideDomainPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseUnit {
    GraySquareCentimeter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseInterpolation {
    LinearLinear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoldNormalization {
    DivideTrackLengthByScoringVolumeCm3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutsideDomainPolicy {
    RejectRun,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentRule {
    pub component: DoseComponent,
    pub estimator: ComponentEstimator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ComponentEstimator {
    NjoyPartialKermaFluenceFold {
        nuclide: String,
        reaction_mt: u16,
        heatr_partial_kerma_mt: u16,
        photon_energy: PhotonEnergyTreatment,
    },
    ResidualNeutronKermaFluenceFold {
        heatr_total_kerma_mt: u16,
        subtract_components: Vec<DoseComponent>,
    },
    CoupledPhotonHeating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhotonEnergyTreatment {
    ExcludedAndTransported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalTotalEstimator {
    CoupledHeatingWithoutParticleFilter,
}

/// Frozen recipe for generating a material-specific neutron response set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseGenerationMethod {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: MethodQualification,
    pub component_profile: ContentReference,
    pub material: ContentReference,
    pub evaluated_data_release: String,
    pub processor: ToolIdentity,
    pub temperature_k: f64,
    pub reconstruction_tolerance_fraction: f64,
    pub heatr: HeatrMethod,
    pub atom_density_basis: AtomDensityBasis,
    pub grid_policy: GridPolicy,
    pub response_unit: ResponseUnit,
    pub interpolation: ResponseInterpolation,
    pub joule_per_ev: f64,
}

impl ResponseGenerationMethod {
    pub fn validate(&self) -> Result<(), ResponseMethodError> {
        validate_identifier("response_method.schema_version", &self.schema_version)?;
        validate_identifier("response_method.id", &self.id)?;
        validate_identifier(
            "response_method.evaluated_data_release",
            &self.evaluated_data_release,
        )?;
        self.component_profile
            .validate()
            .map_err(|_| ResponseMethodError::InvalidContentReference("component_profile"))?;
        self.material
            .validate()
            .map_err(|_| ResponseMethodError::InvalidContentReference("material"))?;
        self.processor.validate()?;
        if !self.temperature_k.is_finite() || self.temperature_k <= 0.0 {
            return Err(ResponseMethodError::InvalidTemperature);
        }
        if !self.reconstruction_tolerance_fraction.is_finite()
            || self.reconstruction_tolerance_fraction <= 0.0
            || self.reconstruction_tolerance_fraction > 1.0e-3
        {
            return Err(ResponseMethodError::InvalidReconstructionTolerance);
        }
        self.heatr.validate()?;
        if !self.joule_per_ev.is_finite()
            || (self.joule_per_ev - JOULE_PER_EV).abs() > JOULE_PER_EV * f64::EPSILON
        {
            return Err(ResponseMethodError::InvalidElectronVoltConversion);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodQualification {
    MethodFrozenTablesPending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolIdentity {
    pub name: String,
    pub version: String,
    pub source_commit: String,
}

impl ToolIdentity {
    fn validate(&self) -> Result<(), ResponseMethodError> {
        validate_identifier("processor.name", &self.name)?;
        validate_identifier("processor.version", &self.version)?;
        if self.source_commit.len() != 40
            || !self
                .source_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ResponseMethodError::InvalidSourceCommit);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeatrMethod {
    pub local_photon_deposition: bool,
    pub kinematic_checks: bool,
    pub allow_q_value_overrides: bool,
    pub total_kerma_mt: u16,
    pub kinematic_total_mt: u16,
    pub partials: Vec<PartialKermaChannel>,
    pub residual_component: DoseComponent,
    pub residual_subtract_components: Vec<DoseComponent>,
}

impl HeatrMethod {
    fn validate(&self) -> Result<(), ResponseMethodError> {
        if self.local_photon_deposition {
            return Err(ResponseMethodError::LocalPhotonDoubleCounting);
        }
        if !self.kinematic_checks {
            return Err(ResponseMethodError::KinematicChecksDisabled);
        }
        if self.allow_q_value_overrides {
            return Err(ResponseMethodError::QValueOverridesEnabled);
        }
        if self.total_kerma_mt != 301 || self.kinematic_total_mt != 443 {
            return Err(ResponseMethodError::InvalidHeatrTotalMt);
        }

        let mut components = BTreeSet::new();
        for partial in &self.partials {
            if !components.insert(partial.component) {
                return Err(ResponseMethodError::DuplicatePartialComponent(
                    partial.component,
                ));
            }
            if u32::from(partial.heatr_partial_kerma_mt) != u32::from(partial.reaction_mt) + 300 {
                return Err(ResponseMethodError::InvalidPartialKermaMt {
                    reaction_mt: partial.reaction_mt,
                    partial_mt: partial.heatr_partial_kerma_mt,
                });
            }
            match partial.component {
                DoseComponent::Boron
                    if partial.nuclide == "B10"
                        && partial.reaction_mt == 107
                        && partial.heatr_partial_kerma_mt == 407 => {}
                DoseComponent::Nitrogen
                    if partial.nuclide == "N14"
                        && partial.reaction_mt == 103
                        && partial.heatr_partial_kerma_mt == 403 => {}
                _ => {
                    return Err(ResponseMethodError::InvalidPartialChannel(
                        partial.component,
                    ));
                }
            }
        }
        if components != BTreeSet::from([DoseComponent::Boron, DoseComponent::Nitrogen])
            || self.residual_component != DoseComponent::Hydrogen
            || self
                .residual_subtract_components
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                != components
            || self.residual_subtract_components.len() != components.len()
        {
            return Err(ResponseMethodError::InvalidResidualClassification);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialKermaChannel {
    pub component: DoseComponent,
    pub nuclide: String,
    pub reaction_mt: u16,
    pub heatr_partial_kerma_mt: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomDensityBasis {
    OpenMcHdf5AtomicWeightRatios,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridPolicy {
    FullPointwiseUnionWithoutDownsampling,
}

/// Material-specific neutron fluence-to-absorbed-dose response functions.
///
/// This object is transport-neutral. A backend may fold the three component
/// curves with neutron fluence only after verifying the content references and
/// ensuring that its transported energy domain is contained in
/// `transport_energy_range_ev`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeutronResponseSet {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub qualification: ResponseSetQualification,
    pub component_profile: ContentReference,
    pub material: ContentReference,
    pub nuclear_data_manifest: ContentReference,
    pub generation_method: ContentReference,
    pub independent_review: Option<ContentReference>,
    pub transport_energy_range_ev: [f64; 2],
    pub energy_ev: Vec<f64>,
    pub unit: ResponseUnit,
    pub interpolation: ResponseInterpolation,
    pub boron_gy_cm2: Vec<f64>,
    pub nitrogen_gy_cm2: Vec<f64>,
    pub hydrogen_gy_cm2: Vec<f64>,
    /// Independently retained material MT=301 response used for closure.
    pub total_neutron_gy_cm2: Vec<f64>,
}

impl NeutronResponseSet {
    pub fn validate(&self) -> Result<(), ResponseSetError> {
        validate_response_set_identifier("response_set.schema_version", &self.schema_version)?;
        validate_response_set_identifier("response_set.id", &self.id)?;

        for (label, reference) in [
            ("component_profile", &self.component_profile),
            ("material", &self.material),
            ("nuclear_data_manifest", &self.nuclear_data_manifest),
            ("generation_method", &self.generation_method),
        ] {
            reference
                .validate()
                .map_err(|_| ResponseSetError::InvalidContentReference(label))?;
        }

        match (&self.qualification, &self.independent_review) {
            (ResponseSetQualification::GeneratedUnreviewed, None) => {}
            (ResponseSetQualification::IndependentlyReviewed, Some(reference)) => reference
                .validate()
                .map_err(|_| ResponseSetError::InvalidContentReference("independent_review"))?,
            _ => return Err(ResponseSetError::InconsistentReviewState),
        }

        let [lower, upper] = self.transport_energy_range_ev;
        if !lower.is_finite() || !upper.is_finite() || lower < 0.0 || lower >= upper {
            return Err(ResponseSetError::InvalidTransportEnergyRange);
        }
        if self.energy_ev.len() < 2 {
            return Err(ResponseSetError::InsufficientEnergyGrid);
        }
        if self
            .energy_ev
            .iter()
            .any(|energy| !energy.is_finite() || *energy < 0.0)
        {
            return Err(ResponseSetError::InvalidEnergyGridValue);
        }
        if self.energy_ev.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ResponseSetError::NonIncreasingEnergyGrid);
        }
        if self.energy_ev[0] > lower || self.energy_ev[self.energy_ev.len() - 1] < upper {
            return Err(ResponseSetError::EnergyDomainNotCovered);
        }

        for (label, values) in [
            ("boron_gy_cm2", &self.boron_gy_cm2),
            ("nitrogen_gy_cm2", &self.nitrogen_gy_cm2),
            ("hydrogen_gy_cm2", &self.hydrogen_gy_cm2),
            ("total_neutron_gy_cm2", &self.total_neutron_gy_cm2),
        ] {
            if values.len() != self.energy_ev.len() {
                return Err(ResponseSetError::ResponseLength {
                    curve: label,
                    expected: self.energy_ev.len(),
                    actual: values.len(),
                });
            }
            if values
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            {
                return Err(ResponseSetError::InvalidResponseValue(label));
            }
        }

        for (index, total) in self.total_neutron_gy_cm2.iter().copied().enumerate() {
            let classified = self.boron_gy_cm2[index]
                + self.nitrogen_gy_cm2[index]
                + self.hydrogen_gy_cm2[index];
            let scale = classified.abs().max(total.abs());
            let tolerance = 64.0 * f64::EPSILON * scale.max(f64::MIN_POSITIVE);
            if (classified - total).abs() > tolerance {
                return Err(ResponseSetError::NeutronKermaClosure { index });
            }
        }

        Ok(())
    }

    /// Validate the table and require recorded independent review before it is
    /// folded into a reported transport result.
    pub fn validate_for_folding(&self) -> Result<(), ResponseSetError> {
        self.validate()?;
        if self.qualification != ResponseSetQualification::IndependentlyReviewed {
            return Err(ResponseSetError::ResponseSetNotIndependentlyReviewed);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseSetQualification {
    GeneratedUnreviewed,
    IndependentlyReviewed,
}

fn validate_response_set_identifier(
    label: &'static str,
    value: &str,
) -> Result<(), ResponseSetError> {
    if value.trim().is_empty() {
        Err(ResponseSetError::EmptyIdentifier(label))
    } else {
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ResponseSetError {
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("{0} must have a nonempty ID and canonical lowercase SHA-256 digest")]
    InvalidContentReference(&'static str),
    #[error("response-set qualification and independent-review evidence are inconsistent")]
    InconsistentReviewState,
    #[error("an independently reviewed response set is required for dose folding")]
    ResponseSetNotIndependentlyReviewed,
    #[error("transport energy range must be finite, non-negative, and increasing")]
    InvalidTransportEnergyRange,
    #[error("response energy grid must contain at least two knots")]
    InsufficientEnergyGrid,
    #[error("response energy-grid values must be finite and non-negative")]
    InvalidEnergyGridValue,
    #[error("response energy grid must be strictly increasing")]
    NonIncreasingEnergyGrid,
    #[error("response energy grid does not cover the declared transport energy range")]
    EnergyDomainNotCovered,
    #[error("{curve} contains {actual} values; expected {expected}")]
    ResponseLength {
        curve: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{0} contains a negative or non-finite response")]
    InvalidResponseValue(&'static str),
    #[error("classified neutron responses do not close to total neutron KERMA at knot {index}")]
    NeutronKermaClosure { index: usize },
}

fn validate_component_rule(rule: &ComponentRule) -> Result<(), ResponseMethodError> {
    match (rule.component, &rule.estimator) {
        (
            DoseComponent::Boron,
            ComponentEstimator::NjoyPartialKermaFluenceFold {
                nuclide,
                reaction_mt: 107,
                heatr_partial_kerma_mt: 407,
                ..
            },
        ) if nuclide == "B10" => Ok(()),
        (
            DoseComponent::Nitrogen,
            ComponentEstimator::NjoyPartialKermaFluenceFold {
                nuclide,
                reaction_mt: 103,
                heatr_partial_kerma_mt: 403,
                ..
            },
        ) if nuclide == "N14" => Ok(()),
        (
            DoseComponent::Hydrogen,
            ComponentEstimator::ResidualNeutronKermaFluenceFold {
                heatr_total_kerma_mt: 301,
                subtract_components,
            },
        ) if subtract_components.len() == 2
            && subtract_components.iter().copied().collect::<BTreeSet<_>>()
                == BTreeSet::from([DoseComponent::Boron, DoseComponent::Nitrogen]) =>
        {
            Ok(())
        }
        (DoseComponent::Photon, ComponentEstimator::CoupledPhotonHeating) => Ok(()),
        (component, _) => Err(ResponseMethodError::InvalidComponentRule(component)),
    }
}

fn validate_identifier(label: &'static str, value: &str) -> Result<(), ResponseMethodError> {
    if value.trim().is_empty() {
        Err(ResponseMethodError::EmptyIdentifier(label))
    } else {
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ResponseMethodError {
    #[error("required identifier {0} is empty")]
    EmptyIdentifier(&'static str),
    #[error("{0} must have a canonical lowercase SHA-256 digest")]
    InvalidContentReference(&'static str),
    #[error("component {0:?} occurs more than once")]
    DuplicateComponent(DoseComponent),
    #[error("component {0:?} is missing")]
    MissingComponent(DoseComponent),
    #[error("component profile contains {0} rules; expected exactly four")]
    UnexpectedComponentCount(usize),
    #[error("component {0:?} has an estimator inconsistent with the canonical profile")]
    InvalidComponentRule(DoseComponent),
    #[error("processor source commit must be 40 lowercase hexadecimal characters")]
    InvalidSourceCommit,
    #[error("response-generation temperature must be finite and greater than zero kelvin")]
    InvalidTemperature,
    #[error("NJOY reconstruction tolerance must be in (0, 1e-3]")]
    InvalidReconstructionTolerance,
    #[error("local photon deposition would double count energy in coupled transport")]
    LocalPhotonDoubleCounting,
    #[error("NJOY HEATR kinematic consistency checks must be enabled")]
    KinematicChecksDisabled,
    #[error("unreviewed Q-value overrides are forbidden")]
    QValueOverridesEnabled,
    #[error("NJOY HEATR total and kinematic-total MT values must be 301 and 443")]
    InvalidHeatrTotalMt,
    #[error("partial KERMA channel for {0:?} occurs more than once")]
    DuplicatePartialComponent(DoseComponent),
    #[error("partial KERMA channel for {0:?} does not match the canonical profile")]
    InvalidPartialChannel(DoseComponent),
    #[error("partial KERMA MT {partial_mt} is not reaction MT {reaction_mt} plus 300")]
    InvalidPartialKermaMt { reaction_mt: u16, partial_mt: u16 },
    #[error("residual neutron KERMA classification is inconsistent")]
    InvalidResidualClassification,
    #[error("electronvolt-to-joule conversion must use the exact SI constant")]
    InvalidElectronVoltConversion,
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    const PROFILE_JSON: &str =
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/component-profile.json");
    const MATERIAL_JSON: &str =
        include_str!("../../../benchmarks/synthetic/nf-bnct-001/transport/material.json");
    const METHOD_JSON: &str = include_str!(
        "../../../benchmarks/synthetic/nf-bnct-001/transport/response-generation-method.json"
    );

    fn profile() -> ComponentDefinitionProfile {
        serde_json::from_str(PROFILE_JSON).unwrap()
    }

    fn method() -> ResponseGenerationMethod {
        serde_json::from_str(METHOD_JSON).unwrap()
    }

    fn sha256(content: &str) -> String {
        format!("{:x}", Sha256::digest(content.as_bytes()))
    }

    fn reference(id: &str, digit: char) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: digit.to_string().repeat(64),
        }
    }

    fn response_set() -> NeutronResponseSet {
        NeutronResponseSet {
            schema_version: "openbnct.neutron-response-set/0.1.0".into(),
            id: "openbnct.synthetic-response-set.v1".into(),
            qualification: ResponseSetQualification::GeneratedUnreviewed,
            component_profile: reference("profile", 'a'),
            material: reference("material", 'b'),
            nuclear_data_manifest: reference("nuclear-data", 'c'),
            generation_method: reference("method", 'd'),
            independent_review: None,
            transport_energy_range_ev: [0.0, 20.0],
            energy_ev: vec![0.0, 1.0, 20.0],
            unit: ResponseUnit::GraySquareCentimeter,
            interpolation: ResponseInterpolation::LinearLinear,
            boron_gy_cm2: vec![1.0, 2.0, 3.0],
            nitrogen_gy_cm2: vec![2.0, 3.0, 4.0],
            hydrogen_gy_cm2: vec![3.0, 4.0, 5.0],
            total_neutron_gy_cm2: vec![6.0, 9.0, 12.0],
        }
    }

    #[test]
    fn frozen_component_profile_is_valid() {
        let profile = profile();
        assert_eq!(profile.id, "nctforge.macroscopic-absorbed-dose.v1");
        assert_eq!(profile.validate(), Ok(()));
    }

    #[test]
    fn frozen_generation_method_is_valid() {
        let method = method();
        assert_eq!(method.id, "nctforge.nf-bnct-001.response-generation.v1");
        assert_eq!(method.component_profile.sha256, sha256(PROFILE_JSON));
        assert_eq!(method.material.sha256, sha256(MATERIAL_JSON));
        assert_eq!(method.validate(), Ok(()));
    }

    #[test]
    fn rejects_partial_kerma_that_is_not_mt_plus_300() {
        let mut method = method();
        method.heatr.partials[0].heatr_partial_kerma_mt = 107;
        assert_eq!(
            method.validate(),
            Err(ResponseMethodError::InvalidPartialKermaMt {
                reaction_mt: 107,
                partial_mt: 107,
            })
        );
    }

    #[test]
    fn rejects_local_photon_double_counting() {
        let mut method = method();
        method.heatr.local_photon_deposition = true;
        assert_eq!(
            method.validate(),
            Err(ResponseMethodError::LocalPhotonDoubleCounting)
        );
    }

    #[test]
    fn validates_closed_material_response_set() {
        let mut response_set = response_set();
        assert_eq!(response_set.validate(), Ok(()));
        assert_eq!(
            response_set.validate_for_folding(),
            Err(ResponseSetError::ResponseSetNotIndependentlyReviewed)
        );

        response_set.qualification = ResponseSetQualification::IndependentlyReviewed;
        response_set.independent_review = Some(reference("review", 'e'));
        assert_eq!(response_set.validate_for_folding(), Ok(()));
    }

    #[test]
    fn rejects_unclosed_material_response_set() {
        let mut response_set = response_set();
        response_set.total_neutron_gy_cm2[1] += 1.0e-6;

        assert_eq!(
            response_set.validate(),
            Err(ResponseSetError::NeutronKermaClosure { index: 1 })
        );
    }

    #[test]
    fn rejects_response_grid_that_does_not_cover_transport_domain() {
        let mut response_set = response_set();
        response_set.transport_energy_range_ev[1] = 21.0;

        assert_eq!(
            response_set.validate(),
            Err(ResponseSetError::EnergyDomainNotCovered)
        );
    }

    #[test]
    fn requires_review_evidence_for_reviewed_response_set() {
        let mut response_set = response_set();
        response_set.qualification = ResponseSetQualification::IndependentlyReviewed;

        assert_eq!(
            response_set.validate(),
            Err(ResponseSetError::InconsistentReviewState)
        );

        response_set.independent_review = Some(reference("review", 'e'));
        assert_eq!(response_set.validate(), Ok(()));
    }

    #[test]
    fn rejects_noncanonical_response_set_reference() {
        let mut response_set = response_set();
        response_set.nuclear_data_manifest.sha256 = "C".repeat(64);

        assert_eq!(
            response_set.validate(),
            Err(ResponseSetError::InvalidContentReference(
                "nuclear_data_manifest"
            ))
        );
    }
}
