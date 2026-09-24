// SPDX-License-Identifier: MIT

#![forbid(unsafe_code)]

use std::{error::Error, path::Path};

use openbnct_core::PhysicalDoseBundle;
use serde::{Deserialize, Serialize};

mod accelerator;
mod beam;
mod beam_quality;
mod bsa;
mod cadis;
mod hu_calibration;
mod measurement;
mod model;
mod multigroup;
mod photon;
mod positioning;
mod prompt_gamma;
mod response;
mod screening;
mod uq;
mod variance_reduction;

pub use accelerator::{
    ACCELERATOR_SOURCE_SCHEMA, AcceleratorDerived, AcceleratorError, AcceleratorProvenance,
    AcceleratorReaction, AcceleratorSource, AcceleratorSourceSpec, HistogramSpectrum,
    evaluate_accelerator_source,
};
pub use beam::{
    BEAM_DESCRIPTION_SCHEMA, BeamDescription, BeamError, BeamProvenance, Citation,
    NormalizationBasis, PortGeometry, PortShape,
};
pub use beam_quality::{
    BEAM_QUALITY_SCHEMA, BeamQualityError, BeamQualityReference, BeamQualityReport,
    ComponentWeights, InAirMetrics, InPhantomMetrics, MetricComparison, ReferenceMetric,
    TransverseFluenceProfile, attach_absolute_fluence_profile, attach_transverse_fluence_profiles,
    evaluate_beam_quality, in_air_metrics, in_phantom_metrics,
};
pub use bsa::{
    BSA_SCHEMA, BSA_SWEEP_SCHEMA, BeamShapingAssembly, BsaError, BsaLayer, BsaLayerKind,
    BsaRadialExtent, BsaSweep, BsaSweepParameter, BsaSweepRecord, BsaSweepVariant,
    enumerate_bsa_sweep, sweep_variant_assignment,
};
pub use cadis::{
    AdjointDerivation, CadisError, CadisSummary, DEFAULT_TARGET_CAP, resolve_adjoint_windows,
    summarize as summarize_adjoint_derivation,
};
pub use hu_calibration::{
    HU_CALIBRATION_SCHEMA, HuAnchor, HuAnchorCoverage, HuCalibration, HuCalibrationReport,
};
pub use measurement::{
    ComparisonSummary, MEASUREMENT_COMPARISON_SCHEMA, MEASUREMENT_RECORD_SCHEMA, Measurement,
    MeasurementComparison, MeasurementComparisonReport, MeasurementError, MeasurementMethod,
    MeasurementPosition, MeasurementProvenance, MeasurementRecord, MeasurementValue,
    beam_quality_metric, compare_measurement_record, compare_with_beam_quality,
};
pub use model::{
    AngularDistribution, BORON_TRACK_RANGE_UM, BoronMicrodistribution, EnergyDistribution,
    FixedSourceDefinition, IntervalConvention, MATERIAL_ASSIGNMENT_SCHEMA, MaterialAssignment,
    MaterialDefinition, MaterialRegion, MaterialRegionShape, NeutronThermalTreatment,
    NuclideMassFraction, ParticleType, PlaneAxis, SourceSpatialDistribution, TransportCase,
    TransportModelError,
};
pub use multigroup::{
    MULTIGROUP_DATA_SCHEMA, MULTIGROUP_FLUX_SCHEMA, MultigroupData, MultigroupError,
    MultigroupFlux, MultigroupMaterial, SnOptions, adjoint_direction_score, cell_compositions,
    cell_materials, fold_multigroup_dose, level_symmetric_quadrature, material_composition_map,
    solve_multigroup, solve_multigroup_adjoint,
};
pub use openbnct_core::ContentReference;
pub use photon::{
    MULTIGROUP_PHOTON_DATA_SCHEMA, MultigroupPhotonData, PHOTON_DOSE_COMPONENT, PhotonMaterial,
    fold_photon_dose, solve_photon, solve_photon_adjoint,
};
pub use positioning::{
    AxisApproach, EntrySide, POSITION_REPORT_SCHEMA, PositionReport, PositioningError,
    aim_disk_source_at_centroid, aim_source_at_centroid, rotate_source,
};
pub use prompt_gamma::{
    PROMPT_GAMMA_COUNTS_SCHEMA, PROMPT_GAMMA_ENERGY_EV, PROMPT_GAMMA_QUALIFICATION,
    PROMPT_GAMMA_RESPONSE_SCHEMA, PROMPT_GAMMA_SOURCE_SCHEMA, PromptGammaCounts, PromptGammaError,
    PromptGammaResponse, PromptGammaSource, PromptGammaUnit, derive_prompt_gamma_source,
    expected_prompt_gamma_counts,
};
pub use response::{
    AtomDensityBasis, ComponentDefinitionProfile, ComponentEstimator, ComponentRule,
    FoldNormalization, GridPolicy, HeatrMethod, MethodQualification, NeutronResponseSemantics,
    NeutronResponseSet, OutsideDomainPolicy, PartialKermaChannel, PhotonEnergyTreatment,
    PhysicalTotalEstimator, ResponseGenerationMethod, ResponseInterpolation, ResponseMethodError,
    ResponseSetError, ResponseSetQualification, ResponseUnit, SourceNormalization,
    SpatialDoseModel, ToolIdentity,
};
pub use screening::{
    SENSITIVITY_SCREENING_SCHEMA, SENSITIVITY_SPEC_SCHEMA, ScreeningEntry, ScreeningError,
    ScreeningParameter, ScreeningTarget, SensitivityScreening, SensitivitySpec, run_screening,
};
pub use uq::{
    BudgetEntry, CovarianceBlock, CovarianceDiagonal, CovarianceParameter,
    DOSE_UNCERTAINTY_BUDGET_SCHEMA, DoseUncertaintyBudget, MULTIGROUP_COVARIANCE_SCHEMA,
    MultigroupCovariance, UqDerivation, UqError, propagate_uncertainty,
};
pub use variance_reduction::{
    AdjointMethod, AdjointResponse, ResolvedWeightWindow, ResolvedWeightWindows,
    VARIANCE_REDUCTION_SCHEMA, VarianceReductionError, VarianceReductionSpec,
    WEIGHT_WINDOWS_SCHEMA, WeightWindowBounds, WeightWindowDerivation, WeightWindowMesh,
    WeightWindowParameters, WeightWindowSpec,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendDescriptor {
    pub id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub can_prepare: bool,
    pub can_execute: bool,
    pub can_import: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedRun {
    pub backend_id: String,
    pub case_id: String,
    pub working_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedRun {
    pub backend_id: String,
    pub case_id: String,
    pub working_directory: String,
    pub exit_code: i32,
}

/// Boundary between NCTForge and a particle-transport implementation.
///
/// GUI, biological models, QA, and evidence code must consume this trait or
/// the normalized dose bundle, never backend-specific output directly.
pub trait TransportBackend {
    type BackendError: Error + Send + Sync + 'static;

    fn descriptor(&self) -> BackendDescriptor;

    fn prepare(
        &self,
        case: &TransportCase,
        working_directory: &Path,
    ) -> Result<PreparedRun, Self::BackendError>;

    fn execute(&self, prepared: &PreparedRun) -> Result<CompletedRun, Self::BackendError>;

    fn collect(&self, completed: &CompletedRun) -> Result<PhysicalDoseBundle, Self::BackendError>;
}
