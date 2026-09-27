// SPDX-License-Identifier: MIT
//
//! Per-realization evaluation of `openbnct.joint-uncertainty-input/0.1.0`
//! documents over physical dose bundles — the adapter layer between the
//! [`openbnct_core::joint`] contract and this crate's dose statistics and
//! PK integration.
//!
//! An adapter declares *how* one joint source perturbs the delivered
//! map: [`DoseAdapter`] scales components or the whole output,
//! [`PkBinding`] perturbs a regional concentration curve before
//! [`pk_integrated_dose_bundle`] runs. In every realization the source
//! is drawn **once** and the same draw is applied wherever the source
//! scopes — a shared calibration is never re-drawn per field, voxel, or
//! interval.
//!
//! Evaluation order per realization:
//!
//! 1. dose adapters rescale the declared subject bundle (in declared
//!    order),
//! 2. optional PK bindings realize a perturbed [`PkModel`] and
//!    `pk_integrated_dose_bundle` integrates the boron component against
//!    it (fixed neutron field — the declared approximation),
//! 3. every requested metric is evaluated on the realized map.
//!
//! Intervals are the metric's distribution across realizations —
//! [`openbnct_core::dose_covering_percent`] per realized map, never an
//! interval over per-voxel summaries. Failed realizations are counted
//! in the report, not dropped silently.

use std::collections::{BTreeMap, BTreeSet};

use openbnct_core::{
    ContentReference, DoseComponent, DoseUnit, EnsembleMethod, EnsembleSpec, JointError,
    JointSource, JointUncertaintyInput, JointUncertaintyReport, MetricValue, PhysicalDoseBundle,
    Realization, RegionMask, SourceSharing, SourceTarget, TotalUncertaintyMethod,
    category_coverage, dose_covering_percent, equivalent_uniform_dose, masked_values, mean,
    run_ensemble, volume_at_least,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ManifestError, PkModel, PkRegion, pk_integrated_dose_bundle};

/// How one joint source scales the dose bundle. All scale draws must
/// be positive; a realized non-positive scale fails the realization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DoseAdapter {
    /// `s` multiplies `physical_total` and every component — output-
    /// factor / source-strength uncertainty.
    OutputScale { source_id: String },
    /// `s` multiplies one component everywhere; the total is adjusted
    /// additively `T += (s−1)·D_c` — exact when the total is the
    /// component sum, a declared first-order-consistent approximation
    /// when it is a dedicated estimator.
    ComponentScale {
        source_id: String,
        component: DoseComponent,
    },
    /// `s` multiplies one component inside one region mask only —
    /// regional uptake uncertainty (e.g. T/N). Total adjusted as
    /// `ComponentScale` at masked voxels.
    RegionComponentScale {
        source_id: String,
        region: String,
        component: DoseComponent,
    },
}

/// One slot inside a regional PK curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PkSlot {
    /// `amplitudes_ppm[i]`.
    Amplitude(u32),
    /// `rates_per_s[i]`.
    Rate(u32),
    /// `planned_concentration_ppm`.
    PlannedConcentration,
}

/// Expected physical unit of a slot — enforced against the source's
/// declared unit for `AbsoluteVector` bindings.
fn slot_unit(slot: PkSlot) -> &'static str {
    match slot {
        PkSlot::Amplitude(_) | PkSlot::PlannedConcentration => "ppm",
        PkSlot::Rate(_) => "1/s",
    }
}

/// How one joint source perturbs the PK model. A binding names a
/// region; the perturbed curve applies uniformly to every voxel the
/// region mask covers and to the whole integration window — the common
/// blood curve and tissue-specific terms are preserved across space
/// and time by construction of [`pk_integrated_dose_bundle`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PkBinding {
    /// Scalar draw `s` scales every amplitude of the region's curve —
    /// concentration-amplitude uncertainty with the curve shape held.
    /// Source must be a scalar draw with unit `"1"`.
    AmplitudeScale { source_id: String, region: String },
    /// Vector draw of *log-scales* `x_i`: `slot_i *= exp(x_i)`. Covers
    /// correlated amplitude/rate perturbations via a declared
    /// multivariate normal over log-scales (unit `"1"`, real support —
    /// the applied scale is positive by construction).
    LogScaleVector {
        source_id: String,
        region: String,
        slots: Vec<PkSlot>,
    },
    /// Vector draw of absolute parameter values replacing the listed
    /// slots — posterior samples of fitted PK parameters. All slots in
    /// one vector must share the source's declared unit (`"ppm"` for
    /// amplitudes/planned concentration, `"1/s"` for rates); mixed-unit
    /// vectors are rejected — declare per-unit sources instead.
    AbsoluteVector {
        source_id: String,
        region: String,
        slots: Vec<PkSlot>,
    },
}

/// PK integration inputs for the ensemble path.
#[derive(Debug, Clone)]
pub struct PkIntegration {
    /// Nominal PK model each realization perturbs.
    pub model: PkModel,
    /// Content binding of that model — must appear in the joint
    /// input's `subjects`.
    pub model_ref: ContentReference,
    pub bindings: Vec<PkBinding>,
    /// Beam-on epoch and window length forwarded to
    /// [`pk_integrated_dose_bundle`].
    pub beam_on_epoch_s: f64,
    pub beam_time_s: f64,
    /// Delivered source-strength normalization (particles/s).
    pub source_strength_per_s: f64,
}

/// Which dose volume a metric reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricQuantity {
    /// The bundle's dedicated physical total.
    PhysicalTotal,
    /// One named component's map.
    Component(DoseComponent),
}

/// Scalar metric evaluated per realization.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetricKind {
    Mean,
    Minimum,
    Maximum,
    /// `D_x` — the level covering the hottest `percent` of the region.
    DoseCoveringPercent {
        percent: f64,
    },
    /// `V_x` — the volume fraction at or above `level`.
    VolumeAtLeast {
        level: f64,
    },
    /// Generalized EUD at Niemierko parameter `a`.
    EquivalentUniformDose {
        a: f64,
    },
}

/// One requested metric over one region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricRequest {
    /// Caller-chosen identifier (carried into the report).
    pub id: String,
    /// Region mask name.
    pub region: String,
    pub quantity: MetricQuantity,
    pub kind: MetricKind,
}

/// Evaluation-spec schema token.
pub const JOINT_DOSE_ENSEMBLE_SPEC_SCHEMA: &str = "openbnct.joint-dose-ensemble-spec/0.1.0";

/// PK integration parameters inside an evaluation spec — everything
/// except the model document itself, which the caller binds separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PkIntegrationSpec {
    pub beam_on_epoch_s: f64,
    pub beam_time_s: f64,
    /// Delivered source-strength normalization (particles/s).
    pub source_strength_per_s: f64,
    #[serde(default)]
    pub bindings: Vec<PkBinding>,
}

/// `openbnct.joint-dose-ensemble-spec/0.1.0` — a versioned evaluation
/// request: which metrics to take, how each joint source perturbs the
/// delivered map, optional PK integration, and the ensemble method.
/// The dose bundle, joint input, and PK model are bound by content
/// hash at call time — the spec is reusable across subjects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointDoseEnsembleSpec {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    /// Requested scalar metrics.
    pub metrics: Vec<MetricRequest>,
    /// Dose-map adapters, applied in declared order each realization.
    #[serde(default)]
    pub dose_adapters: Vec<DoseAdapter>,
    /// PK integration over the scaled rate map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pk: Option<PkIntegrationSpec>,
    /// Grouped first-order attribution: each inner list is one group of
    /// source ids evaluated with the rest pinned at their distribution
    /// means; applies to every requested metric. Correlation-group
    /// members must share an entry — splitting them double-counts
    /// shared variance. Expensive: each group re-samples the ensemble.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribution_groups: Vec<Vec<String>>,
    /// Ensemble method and reporting spec.
    pub ensemble: EnsembleSpec,
}

impl JointDoseEnsembleSpec {
    pub fn validate(&self) -> Result<(), UqEnsembleError> {
        if !openbnct_core::schema_matches(&self.schema_version, JOINT_DOSE_ENSEMBLE_SPEC_SCHEMA) {
            return Err(invalid(format!(
                "unsupported ensemble-spec schema {:?}",
                self.schema_version
            )));
        }
        if self.metrics.is_empty() {
            return Err(invalid("spec declares no metrics"));
        }
        self.ensemble.validate()?;
        if let Some(pk) = &self.pk
            && !(pk.beam_on_epoch_s.is_finite()
                && pk.beam_on_epoch_s >= 0.0
                && pk.beam_time_s.is_finite()
                && pk.beam_time_s >= 0.0
                && pk.source_strength_per_s.is_finite()
                && pk.source_strength_per_s > 0.0)
        {
            return Err(invalid(
                "pk integration needs finite non-negative epoch/time and positive strength",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum UqEnsembleError {
    #[error("joint uncertainty: {0}")]
    Joint(#[from] JointError),
    #[error("manifest: {0}")]
    Manifest(#[from] ManifestError),
    #[error("invalid dose: {0}")]
    Dose(#[from] openbnct_core::ValidationError),
    #[error(
        "joint input {input:?} does not describe {artifact} ({expected}) — stale or missing content binding"
    )]
    StaleReference {
        input: String,
        artifact: String,
        expected: String,
    },
    #[error("invalid uq ensemble request: {0}")]
    Invalid(String),
}

fn invalid(reason: impl Into<String>) -> UqEnsembleError {
    UqEnsembleError::Invalid(reason.into())
}

/// `gray` / `gray_per_source_particle` — serde names without pulling
/// serde_json in just for the tag.
fn unit_label(unit: DoseUnit) -> &'static str {
    match unit {
        DoseUnit::Gray => "gray",
        DoseUnit::GrayPerSourceParticle => "gray_per_source_particle",
    }
}

/// The serde name of a dose component (`"boron"`, …) — also the scope
/// target name for per-target draws.
fn component_name(component: DoseComponent) -> &'static str {
    match component {
        DoseComponent::Boron => "boron",
        DoseComponent::Nitrogen => "nitrogen",
        DoseComponent::Hydrogen => "hydrogen",
        DoseComponent::Photon => "photon",
    }
}

/// True when `source`'s scope covers `target` — either an exact target
/// entry or `Global`.
fn scope_covers(source: &JointSource, target: &SourceTarget) -> bool {
    source
        .scope
        .iter()
        .any(|t| t == &SourceTarget::Global || t == target)
}

/// Resolve a scalar draw for `source` in `realization`: the shared draw,
/// or the draw for `target` under `independent_per_target`.
fn resolve_scale(
    input: &JointUncertaintyInput,
    realization: &Realization,
    source_id: &str,
    target: Option<&str>,
) -> Result<f64, UqEnsembleError> {
    let source = input
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| invalid(format!("adapter references unknown source {source_id:?}")))?;
    match source.sharing {
        SourceSharing::Shared => realization.scalar(source_id).ok_or_else(|| {
            invalid(format!(
                "realization carries no shared draw for {source_id:?}"
            ))
        }),
        SourceSharing::IndependentPerTarget => {
            let target = target.ok_or_else(|| {
                invalid(format!(
                    "per-target source {source_id:?} needs a target name"
                ))
            })?;
            realization
                .for_target(source_id, target)
                .and_then(|v| v.first().copied())
                .ok_or_else(|| {
                    invalid(format!(
                        "realization carries no draw for {source_id:?} target {target:?}"
                    ))
                })
        }
    }
}

/// Vector draws for bindings.
fn resolve_vector(
    input: &JointUncertaintyInput,
    realization: &Realization,
    source_id: &str,
    expected_len: usize,
) -> Result<Vec<f64>, UqEnsembleError> {
    let source = input
        .sources
        .iter()
        .find(|s| s.id == source_id)
        .ok_or_else(|| invalid(format!("adapter references unknown source {source_id:?}")))?;
    if source.sharing != SourceSharing::Shared {
        return Err(invalid(format!(
            "PK binding source {source_id:?} must be a shared draw"
        )));
    }
    let values = realization.vector(source_id).ok_or_else(|| {
        invalid(format!(
            "realization carries no shared draw for {source_id:?}"
        ))
    })?;
    if values.len() != expected_len {
        return Err(invalid(format!(
            "source {source_id:?} drew {} values, binding needs {expected_len}",
            values.len()
        )));
    }
    Ok(values.to_vec())
}

/// Scale a dose volume and its sigma map by `s` / `|s|`.
fn scale_volume(values: &mut [f64], sigmas: Option<&mut Vec<f64>>, s: f64) {
    for v in values.iter_mut() {
        *v *= s;
    }
    if let Some(sigmas) = sigmas {
        for sigma in sigmas.iter_mut() {
            *sigma *= s.abs();
        }
    }
}

/// Apply the dose adapters to `bundle` under one realization.
///
/// Component/region scales adjust the total additively
/// (`T += (s−1)·D_c` at touched voxels) and drop the total's own sigma
/// — under component rebalancing the dedicated estimator's covariance
/// structure is not reproduced by scaling, so the realized map reports
/// `Unavailable` rather than a wrong value. Per-component sigmas are
/// scaled by `|s|`, matching the applied scale.
pub fn realize_dose_bundle(
    bundle: &PhysicalDoseBundle,
    adapters: &[DoseAdapter],
    masks: &[RegionMask],
    input: &JointUncertaintyInput,
    realization: &Realization,
) -> Result<PhysicalDoseBundle, UqEnsembleError> {
    let mut realized = bundle.clone();
    let mut component_scaled = false;
    for adapter in adapters {
        match adapter {
            DoseAdapter::OutputScale { source_id } => {
                let s = resolve_scale(input, realization, source_id, None)?;
                if !s.is_finite() || s <= 0.0 {
                    return Err(invalid(format!(
                        "source {source_id:?}: output scale {s} is not positive"
                    )));
                }
                for volume in &mut realized.components {
                    scale_volume(
                        &mut volume.values,
                        volume.absolute_standard_uncertainty.as_mut(),
                        s,
                    );
                }
                scale_volume(
                    &mut realized.physical_total.values,
                    realized
                        .physical_total
                        .absolute_standard_uncertainty
                        .as_mut(),
                    s,
                );
            }
            DoseAdapter::ComponentScale {
                source_id,
                component,
            } => {
                let s = resolve_scale(
                    input,
                    realization,
                    source_id,
                    Some(component_name(*component)),
                )?;
                scale_component(&mut realized, *component, s, |_| true, source_id)?;
                component_scaled = true;
            }
            DoseAdapter::RegionComponentScale {
                source_id,
                region,
                component,
            } => {
                let mask = masks
                    .iter()
                    .find(|m| m.name == *region)
                    .ok_or_else(|| invalid(format!("unknown region mask {region:?}")))?;
                if mask.voxels.len() != realized.physical_total.values.len() {
                    return Err(invalid(format!(
                        "region mask {region:?} has {} voxels, bundle has {}",
                        mask.voxels.len(),
                        realized.physical_total.values.len()
                    )));
                }
                let s = resolve_scale(input, realization, source_id, Some(region))?;
                scale_component(&mut realized, *component, s, |v| mask.voxels[v], source_id)?;
                component_scaled = true;
            }
        }
    }
    if component_scaled {
        realized.physical_total.absolute_standard_uncertainty = None;
        realized.physical_total.uncertainty_method = TotalUncertaintyMethod::Unavailable;
    }
    realized
        .validate()
        .map_err(|e| invalid(format!("realized bundle: {e}")))?;
    Ok(realized)
}

fn scale_component(
    bundle: &mut PhysicalDoseBundle,
    component: DoseComponent,
    s: f64,
    inside: impl Fn(usize) -> bool,
    source_id: &str,
) -> Result<(), UqEnsembleError> {
    if !s.is_finite() || s <= 0.0 {
        return Err(invalid(format!(
            "source {source_id:?}: component scale {s} is not positive"
        )));
    }
    let index = bundle
        .components
        .iter()
        .position(|v| v.component == component)
        .ok_or_else(|| {
            invalid(format!(
                "bundle carries no {:?} component",
                component_name(component)
            ))
        })?;
    for (voxel, value) in bundle.components[index].values.iter_mut().enumerate() {
        if inside(voxel) {
            let old = *value;
            *value = old * s;
            bundle.physical_total.values[voxel] += (s - 1.0) * old;
        }
    }
    if let Some(sigmas) = &mut bundle.components[index].absolute_standard_uncertainty {
        for sigma in sigmas.iter_mut() {
            *sigma *= s;
        }
    }
    Ok(())
}

/// Mutable handle to one PK parameter slot.
fn pk_slot_mut(region: &mut PkRegion, slot: PkSlot) -> Result<&mut f64, UqEnsembleError> {
    match slot {
        PkSlot::Amplitude(i) => region
            .amplitudes_ppm
            .get_mut(i as usize)
            .ok_or_else(|| invalid(format!("amplitude slot {i} out of range"))),
        PkSlot::Rate(i) => region
            .rates_per_s
            .get_mut(i as usize)
            .ok_or_else(|| invalid(format!("rate slot {i} out of range"))),
        PkSlot::PlannedConcentration => Ok(&mut region.planned_concentration_ppm),
    }
}

/// Apply the PK bindings to `model` under one realization. The
/// perturbed curve is returned wholesale — it applies to every voxel
/// the region covers and the full integration window.
pub fn realize_pk_model(
    model: &PkModel,
    bindings: &[PkBinding],
    input: &JointUncertaintyInput,
    realization: &Realization,
) -> Result<PkModel, UqEnsembleError> {
    let mut realized = model.clone();
    for binding in bindings {
        match binding {
            PkBinding::AmplitudeScale { source_id, region } => {
                let s = resolve_scale(input, realization, source_id, None)?;
                if !s.is_finite() || s <= 0.0 {
                    return Err(invalid(format!(
                        "source {source_id:?}: PK amplitude scale {s} is not positive"
                    )));
                }
                let curve = realized
                    .regions
                    .iter_mut()
                    .find(|r| r.region == *region)
                    .ok_or_else(|| invalid(format!("PK model has no region {region:?}")))?;
                for amplitude in &mut curve.amplitudes_ppm {
                    *amplitude *= s;
                }
            }
            PkBinding::LogScaleVector {
                source_id,
                region,
                slots,
            } => {
                let draws = resolve_vector(input, realization, source_id, slots.len())?;
                let curve = realized
                    .regions
                    .iter_mut()
                    .find(|r| r.region == *region)
                    .ok_or_else(|| invalid(format!("PK model has no region {region:?}")))?;
                for (slot, draw) in slots.iter().zip(draws.iter()) {
                    if !draw.is_finite() {
                        return Err(invalid(format!(
                            "source {source_id:?}: log-scale draw is not finite"
                        )));
                    }
                    *pk_slot_mut(curve, *slot)? *= draw.exp();
                }
            }
            PkBinding::AbsoluteVector {
                source_id,
                region,
                slots,
            } => {
                let draws = resolve_vector(input, realization, source_id, slots.len())?;
                let curve = realized
                    .regions
                    .iter_mut()
                    .find(|r| r.region == *region)
                    .ok_or_else(|| invalid(format!("PK model has no region {region:?}")))?;
                for (slot, draw) in slots.iter().zip(draws.iter()) {
                    *pk_slot_mut(curve, *slot)? = *draw;
                }
            }
        }
    }
    realized
        .validate()
        .map_err(|e| invalid(format!("realized PK model: {e}")))?;
    Ok(realized)
}

/// Evaluate one metric on a bundle's dose volume.
fn evaluate_metric(
    bundle: &PhysicalDoseBundle,
    masks: &[RegionMask],
    request: &MetricRequest,
) -> Result<MetricValue, UqEnsembleError> {
    let values: &[f64] = match request.quantity {
        MetricQuantity::PhysicalTotal => &bundle.physical_total.values,
        MetricQuantity::Component(component) => {
            &bundle
                .components
                .iter()
                .find(|v| v.component == component)
                .ok_or_else(|| {
                    invalid(format!(
                        "bundle has no {:?} component for metric {:?}",
                        component_name(component),
                        request.id
                    ))
                })?
                .values
        }
    };
    let mask = masks
        .iter()
        .find(|m| m.name == request.region)
        .ok_or_else(|| {
            invalid(format!(
                "metric {:?}: unknown region mask {:?}",
                request.id, request.region
            ))
        })?;
    let selected = masked_values(&request.region, values, &mask.voxels)?;
    let value = match request.kind {
        MetricKind::Mean => mean(&selected),
        MetricKind::Minimum => selected.iter().copied().fold(f64::INFINITY, f64::min),
        MetricKind::Maximum => selected.iter().copied().fold(0.0, f64::max),
        MetricKind::DoseCoveringPercent { percent } => dose_covering_percent(&selected, percent)?,
        MetricKind::VolumeAtLeast { level } => volume_at_least(&selected, level)?,
        MetricKind::EquivalentUniformDose { a } => equivalent_uniform_dose(&selected, a)?,
    };
    Ok(MetricValue {
        metric: request.id.clone(),
        value,
    })
}

/// Preflight validation of adapters/bindings/metrics against the
/// declared input — every failure is a clear error, not a skipped term.
fn preflight(
    input: &JointUncertaintyInput,
    bundle: &PhysicalDoseBundle,
    masks: &[RegionMask],
    metrics: &[MetricRequest],
    dose_adapters: &[DoseAdapter],
    attribution_groups: &[Vec<String>],
    pk: Option<&PkIntegration>,
) -> Result<(), UqEnsembleError> {
    openbnct_core::validate_attribution_groups(input, attribution_groups)?;
    let source = |id: &str| -> Result<&JointSource, UqEnsembleError> {
        input
            .sources
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| invalid(format!("adapter references unknown source {id:?}")))
    };
    let require_unit = |s: &JointSource, unit: &str, what: &str| -> Result<(), UqEnsembleError> {
        if s.unit != unit {
            return Err(invalid(format!(
                "source {:?}: {what} expects unit {unit:?}, declared {:?}",
                s.id, s.unit
            )));
        }
        Ok(())
    };
    let require_scalar = |s: &JointSource, what: &str| -> Result<(), UqEnsembleError> {
        if s.dimension() != 1 {
            return Err(invalid(format!(
                "source {:?}: {what} needs a scalar draw, declared dimension {}",
                s.id,
                s.dimension()
            )));
        }
        Ok(())
    };
    for adapter in dose_adapters {
        match adapter {
            DoseAdapter::OutputScale { source_id } => {
                let s = source(source_id)?;
                require_scalar(s, "output scale")?;
                require_unit(s, "1", "output scale")?;
            }
            DoseAdapter::ComponentScale {
                source_id,
                component,
            } => {
                let s = source(source_id)?;
                require_scalar(s, "component scale")?;
                require_unit(s, "1", "component scale")?;
                if !bundle.components.iter().any(|v| v.component == *component) {
                    return Err(invalid(format!(
                        "bundle has no {:?} component",
                        component_name(*component)
                    )));
                }
                let covered = scope_covers(
                    s,
                    &SourceTarget::Component {
                        name: component_name(*component).into(),
                    },
                );
                if !covered {
                    return Err(invalid(format!(
                        "source {source_id:?} scope does not cover component {:?}",
                        component_name(*component)
                    )));
                }
            }
            DoseAdapter::RegionComponentScale {
                source_id,
                region,
                component,
            } => {
                let s = source(source_id)?;
                require_scalar(s, "region component scale")?;
                require_unit(s, "1", "region component scale")?;
                if !masks.iter().any(|m| m.name == *region) {
                    return Err(invalid(format!("unknown region mask {region:?}")));
                }
                if !bundle.components.iter().any(|v| v.component == *component) {
                    return Err(invalid(format!(
                        "bundle has no {:?} component",
                        component_name(*component)
                    )));
                }
                let covered = scope_covers(
                    s,
                    &SourceTarget::Region {
                        name: region.clone(),
                    },
                ) || scope_covers(
                    s,
                    &SourceTarget::Component {
                        name: component_name(*component).into(),
                    },
                );
                if !covered {
                    return Err(invalid(format!(
                        "source {source_id:?} scope covers neither region {region:?} \
                         nor component {:?}",
                        component_name(*component)
                    )));
                }
            }
        }
    }
    if let Some(pk) = pk {
        if bundle.physical_total.unit != DoseUnit::GrayPerSourceParticle
            || bundle
                .components
                .iter()
                .any(|v| v.unit != DoseUnit::GrayPerSourceParticle)
        {
            return Err(invalid(
                "PK integration needs a gray_per_source_particle rate bundle",
            ));
        }
        for binding in &pk.bindings {
            match binding {
                PkBinding::AmplitudeScale { source_id, region } => {
                    let s = source(source_id)?;
                    require_scalar(s, "PK amplitude scale")?;
                    require_unit(s, "1", "PK amplitude scale")?;
                    if !pk.model.regions.iter().any(|r| r.region == *region) {
                        return Err(invalid(format!("PK model has no region {region:?}")));
                    }
                }
                PkBinding::LogScaleVector {
                    source_id,
                    region,
                    slots,
                }
                | PkBinding::AbsoluteVector {
                    source_id,
                    region,
                    slots,
                } => {
                    let s = source(source_id)?;
                    if slots.is_empty() {
                        return Err(invalid(format!(
                            "PK binding on {source_id:?} lists no slots"
                        )));
                    }
                    if s.dimension() != slots.len() {
                        return Err(invalid(format!(
                            "source {source_id:?} dimension {} != {} slots",
                            s.dimension(),
                            slots.len()
                        )));
                    }
                    if matches!(binding, PkBinding::AbsoluteVector { .. }) {
                        let expected = slot_unit(slots[0]);
                        if !slots.iter().all(|slot| slot_unit(*slot) == expected) {
                            return Err(invalid(format!(
                                "source {source_id:?}: absolute-vector slots must share \
                                 one unit (mixed ppm and 1/s is not allowed)"
                            )));
                        }
                        require_unit(s, expected, "PK absolute vector")?;
                    } else {
                        require_unit(s, "1", "PK log-scale vector")?;
                    }
                    if !pk.model.regions.iter().any(|r| r.region == *region) {
                        return Err(invalid(format!("PK model has no region {region:?}")));
                    }
                }
            }
        }
        if !(pk.beam_on_epoch_s.is_finite()
            && pk.beam_on_epoch_s >= 0.0
            && pk.beam_time_s.is_finite()
            && pk.beam_time_s >= 0.0
            && pk.source_strength_per_s.is_finite()
            && pk.source_strength_per_s > 0.0)
        {
            return Err(invalid(
                "PK integration needs finite non-negative epoch/time and positive strength",
            ));
        }
    }
    let mut ids = BTreeSet::new();
    for metric in metrics {
        if metric.id.trim().is_empty() || !ids.insert(metric.id.clone()) {
            return Err(invalid("metric ids must be non-empty and unique"));
        }
        if !masks.iter().any(|m| m.name == metric.region) {
            return Err(invalid(format!(
                "metric {:?}: unknown region mask {:?}",
                metric.id, metric.region
            )));
        }
        if let MetricQuantity::Component(component) = metric.quantity
            && !bundle.components.iter().any(|v| v.component == component)
        {
            return Err(invalid(format!(
                "metric {:?}: bundle has no {:?} component",
                metric.id,
                component_name(component)
            )));
        }
    }
    Ok(())
}

/// One realization's metric values: realize the bundle (and PK model
/// when integration is requested), then evaluate every request.
#[allow(clippy::too_many_arguments)]
fn evaluate_realization(
    bundle: &PhysicalDoseBundle,
    dose_adapters: &[DoseAdapter],
    masks: &[RegionMask],
    input: &JointUncertaintyInput,
    pk: Option<&PkIntegration>,
    metrics: &[MetricRequest],
    report_id: &str,
    realization: &openbnct_core::Realization,
) -> Result<Vec<MetricValue>, UqEnsembleError> {
    let realized = realize_dose_bundle(bundle, dose_adapters, masks, input, realization)?;
    let integrated;
    let evaluated: &PhysicalDoseBundle = if let Some(pk) = pk {
        let realized_pk = realize_pk_model(&pk.model, &pk.bindings, input, realization)?;
        integrated = pk_integrated_dose_bundle(
            &realized,
            &realized_pk,
            masks,
            pk.beam_on_epoch_s,
            pk.beam_time_s,
            pk.source_strength_per_s,
            format!("joint-ensemble:{report_id}"),
        )?;
        &integrated
    } else {
        &realized
    };
    metrics
        .iter()
        .map(|m| evaluate_metric(evaluated, masks, m))
        .collect()
}

/// Evaluate `metrics` over the joint ensemble on `bundle` and assemble
/// the `openbnct.joint-uncertainty-report/0.1.0`.
///
/// `input_ref`/`bundle_ref` are content references of the two parsed
/// artifacts; the input must list the bundle — and the PK model, when
/// `pk` is supplied — in `subjects`, binding the declaration to the
/// evaluated artifacts by hash. A subject mismatch is a hard
/// [`UqEnsembleError::StaleReference`].
///
/// `attribution_groups` requests a grouped first-order attribution for
/// every metric: each group is re-sampled with the remaining sources
/// pinned at their distribution means. Correlation-group members must
/// share a group — the decomposition refuses to split them.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_joint_dose_ensemble(
    input: &JointUncertaintyInput,
    spec: &EnsembleSpec,
    bundle: &PhysicalDoseBundle,
    bundle_ref: &ContentReference,
    masks: &[RegionMask],
    metrics: &[MetricRequest],
    dose_adapters: &[DoseAdapter],
    attribution_groups: &[Vec<String>],
    pk: Option<&PkIntegration>,
    input_ref: ContentReference,
    report_id: &str,
    provenance_id: &str,
) -> Result<JointUncertaintyReport, UqEnsembleError> {
    input.validate()?;
    spec.validate()?;
    bundle.validate()?;
    if metrics.is_empty() {
        return Err(invalid("at least one metric request is required"));
    }
    if !input.subjects.iter().any(|r| r == bundle_ref) {
        return Err(UqEnsembleError::StaleReference {
            input: input.id.clone(),
            artifact: "dose bundle".into(),
            expected: format!("{}:{}", bundle_ref.id, bundle_ref.sha256),
        });
    }
    if let Some(pk) = pk
        && !input.subjects.iter().any(|r| r == &pk.model_ref)
    {
        return Err(UqEnsembleError::StaleReference {
            input: input.id.clone(),
            artifact: "pk model".into(),
            expected: format!("{}:{}", pk.model_ref.id, pk.model_ref.sha256),
        });
    }
    preflight(
        input,
        bundle,
        masks,
        metrics,
        dose_adapters,
        attribution_groups,
        pk,
    )?;

    // The reported unit: gray once PK integrates, else the bundle's own.
    let unit = if pk.is_some() {
        "gray"
    } else {
        unit_label(bundle.physical_total.unit)
    };
    let metric_units: BTreeMap<String, String> = metrics
        .iter()
        .map(|m| (m.id.clone(), unit.to_string()))
        .collect();

    let mut evaluate = |realization: &openbnct_core::Realization| {
        evaluate_realization(
            bundle,
            dose_adapters,
            masks,
            input,
            pk,
            metrics,
            report_id,
            realization,
        )
    };
    let distributions = run_ensemble(input, spec, &metric_units, &mut evaluate)?;

    // Grouped attribution: re-sample each group with the other sources
    // pinned; the pinned-point evaluation shares the same realization
    // machinery, so draws stay deterministic under the declared seed.
    let mut attribution = Vec::new();
    for distribution in &distributions {
        if attribution_groups.is_empty() {
            break;
        }
        let metric_id = distribution.metric.clone();
        let total = distribution.std_dev.max(0.0).powi(2);
        let (effects, residual) = openbnct_core::attribute_first_order(
            input,
            spec,
            attribution_groups,
            total,
            &mut |realization: &openbnct_core::Realization| {
                evaluate_realization(
                    bundle,
                    dose_adapters,
                    masks,
                    input,
                    pk,
                    metrics,
                    report_id,
                    realization,
                )
                .and_then(|values| {
                    values
                        .iter()
                        .find(|v| v.metric == metric_id)
                        .map(|v| v.value)
                        .ok_or_else(|| {
                            invalid(format!("metric {metric_id:?} missing in a realization"))
                        })
                })
            },
        )?;
        attribution.push(openbnct_core::MetricAttribution {
            metric: metric_id,
            effects,
            residual,
        });
    }

    let (method, seed) = match &spec.method {
        EnsembleMethod::MonteCarlo { seed, .. } => ("monte_carlo", Some(*seed)),
        EnsembleMethod::WeightedSamples { .. } => ("weighted_samples", None),
    };
    let mut assumptions = vec![
        "each metric is evaluated on the fully realized map before summarizing; \
         intervals are ensemble distributions, not scenario ranges"
            .into(),
        "each declared source is drawn once per realization and reused wherever \
         its identity scopes — no per-field or per-interval re-draws"
            .into(),
        "the report propagates the declared sources only; unassessed and excluded \
         uncertainty categories are listed in `categories` — this is not a claim \
         of complete patient uncertainty"
            .into(),
    ];
    if dose_adapters
        .iter()
        .any(|a| !matches!(a, DoseAdapter::OutputScale { .. }))
    {
        assumptions.push(
            "physical_total is adjusted additively (T += (s−1)·D_c) under \
             component and region scales — exact when the total is the component \
             sum, a declared first-order-consistent approximation for a \
             dedicated total estimator; realized totals carry no derived sigma"
                .into(),
        );
    }
    if pk.is_some() {
        assumptions.push(
            "boron dose scales with the declared regional PK curves against a \
             fixed neutron field — concentration-dependent spectrum changes \
             require fresh transport"
                .into(),
        );
        assumptions.push(
            "voxels outside every PK region mask use the constant-concentration \
             scale, matching the solver convention"
                .into(),
        );
    }
    assumptions.push(
        "per-voxel Monte Carlo sigmas on the source bundle are not re-drawn here; \
         the statistical_sampling category's disposition records how it is \
         assessed"
            .into(),
    );
    if !attribution_groups.is_empty() {
        assumptions.push(
            "attribution is a grouped first-order decomposition: pinned groups \
             contribute their main-effect variance and the residual retains \
             interactions and sampling noise"
                .into(),
        );
    }

    let report = JointUncertaintyReport {
        schema_version: openbnct_core::JOINT_UNCERTAINTY_REPORT_SCHEMA.into(),
        id: report_id.into(),
        case_id: bundle.case_id.clone(),
        input: input_ref,
        subject: Some(bundle_ref.clone()),
        joint_input: input.clone(),
        categories: category_coverage(input),
        metrics: distributions,
        attribution,
        method: method.into(),
        quantile_estimator: "weighted_hazen".into(),
        seed,
        assumptions,
        qualification: openbnct_core::JOINT_UNCERTAINTY_QUALIFICATION.into(),
        provenance_id: provenance_id.into(),
    };
    report.validate()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{
        CategoryDisposition, CategoryStatus, Distribution, EnsembleMethod, GridGeometry,
        JointSource, PhysicalTotalDoseVolume, SourceEvidence, SourceSharing, Support,
        TotalUncertaintyMethod, UncertaintyCategory,
    };

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [2, 1, 1],
            spacing_mm: [1.0; 3],
            origin_mm: [0.0; 3],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn volume(
        component: DoseComponent,
        values: Vec<f64>,
        unit: DoseUnit,
    ) -> openbnct_core::DoseVolume {
        openbnct_core::DoseVolume {
            component,
            unit,
            values,
            absolute_standard_uncertainty: None,
        }
    }

    /// Two-voxel bundle in Gy/source-particle, boron-heavy.
    fn rate_bundle() -> PhysicalDoseBundle {
        let components = vec![
            volume(
                DoseComponent::Boron,
                vec![4.0, 4.0],
                DoseUnit::GrayPerSourceParticle,
            ),
            volume(
                DoseComponent::Nitrogen,
                vec![1.0, 1.0],
                DoseUnit::GrayPerSourceParticle,
            ),
            volume(
                DoseComponent::Hydrogen,
                vec![0.5, 0.5],
                DoseUnit::GrayPerSourceParticle,
            ),
            volume(
                DoseComponent::Photon,
                vec![0.5, 0.5],
                DoseUnit::GrayPerSourceParticle,
            ),
        ];
        PhysicalDoseBundle {
            schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "uq.test".into(),
            frame_of_reference_uid: None,
            geometry: geometry(),
            component_profile: cref("profile"),
            response_set: cref("responses"),
            components,
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values: vec![6.0, 6.0],
                absolute_standard_uncertainty: None,
                uncertainty_method: TotalUncertaintyMethod::Unavailable,
            },
            provenance_id: "test".into(),
        }
    }

    fn mask(name: &str, voxels: &[bool]) -> RegionMask {
        RegionMask {
            name: name.into(),
            voxels: voxels.to_vec(),
        }
    }

    fn scale_source(id: &str, scope: Vec<SourceTarget>) -> JointSource {
        JointSource {
            id: id.into(),
            category: UncertaintyCategory::InputParameter,
            scope,
            sharing: SourceSharing::Shared,
            unit: "1".into(),
            support: Support::Positive,
            distribution: Distribution::LogNormal {
                median: 1.0,
                sigma_log: 0.1,
            },
            evidence: SourceEvidence {
                basis: "synthetic".into(),
                reference: None,
            },
        }
    }

    fn input(sources: Vec<JointSource>, subjects: Vec<ContentReference>) -> JointUncertaintyInput {
        JointUncertaintyInput {
            schema_version: openbnct_core::JOINT_UNCERTAINTY_INPUT_SCHEMA.into(),
            id: "joint.test".into(),
            subjects,
            sources,
            correlations: Vec::new(),
            category_disposition: vec![
                CategoryDisposition {
                    category: UncertaintyCategory::StatisticalSampling,
                    status: CategoryStatus::Approximated,
                    note: "per-voxel MC sigmas carried on the bundle".into(),
                },
                CategoryDisposition {
                    category: UncertaintyCategory::StructuralModel,
                    status: CategoryStatus::Unassessed,
                    note: "single model".into(),
                },
                CategoryDisposition {
                    category: UncertaintyCategory::ModelDiscrepancy,
                    status: CategoryStatus::Unassessed,
                    note: "not assessed".into(),
                },
            ],
            qualification: openbnct_core::JOINT_UNCERTAINTY_QUALIFICATION.into(),
            provenance_id: "test".into(),
        }
    }

    fn d95_metric() -> MetricRequest {
        MetricRequest {
            id: "d95".into(),
            region: "both".into(),
            quantity: MetricQuantity::Component(DoseComponent::Boron),
            kind: MetricKind::DoseCoveringPercent { percent: 95.0 },
        }
    }

    #[test]
    fn attribution_splits_variance_by_source_and_respects_groups() {
        let bundle = rate_bundle();
        let masks = vec![mask("both", &[true, true])];
        // mean(total) = 4·s_u + 1.5 + 0.5·s_p — additive in the draws,
        // so group effects are the analytic shares exactly.
        let joint = input(
            vec![
                scale_source(
                    "uptake",
                    vec![SourceTarget::Component {
                        name: "boron".into(),
                    }],
                ),
                scale_source(
                    "photon-cal",
                    vec![SourceTarget::Component {
                        name: "photon".into(),
                    }],
                ),
            ],
            vec![cref("bundle")],
        );
        let adapters = vec![
            DoseAdapter::ComponentScale {
                source_id: "uptake".into(),
                component: DoseComponent::Boron,
            },
            DoseAdapter::ComponentScale {
                source_id: "photon-cal".into(),
                component: DoseComponent::Photon,
            },
        ];
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 512,
                seed: 11,
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let groups: Vec<Vec<String>> = vec![vec!["uptake".into()], vec!["photon-cal".into()]];
        let report = evaluate_joint_dose_ensemble(
            &joint,
            &spec,
            &bundle,
            &cref("bundle"),
            &masks,
            &[MetricRequest {
                id: "mean_total".into(),
                region: "both".into(),
                quantity: MetricQuantity::PhysicalTotal,
                kind: MetricKind::Mean,
            }],
            &adapters,
            &groups,
            None,
            cref("joint"),
            "report.test",
            "test",
        )
        .unwrap();
        let dist = &report.metrics[0];
        let attr = report
            .attribution
            .iter()
            .find(|a| a.metric == "mean_total")
            .expect("attribution reported");
        // Var(s) for LogNormal(1, 0.1) = e^0.01 − 1 ≈ 0.010050; the
        // uptake share is 16/(16 + 0.25) = 0.9846 of the total.
        let uptake = attr.effects.iter().find(|e| e.group == "uptake").unwrap();
        let expected_share = 16.0 / 16.25;
        assert!(
            (uptake.fraction.unwrap() - expected_share).abs() < 0.02,
            "uptake fraction {} vs {expected_share}",
            uptake.fraction.unwrap()
        );
        // Additive metric → interactions vanish; residual is noise only.
        assert!(
            attr.residual.abs() < 0.05 * dist.std_dev.powi(2),
            "residual {} vs total {}",
            attr.residual,
            dist.std_dev.powi(2)
        );

        // Correlated members in different groups are refused, not split.
        let mut correlated = joint.clone();
        correlated.correlations = vec![openbnct_core::CorrelationGroup {
            sources: vec!["uptake".into(), "photon-cal".into()],
            correlation: vec![1.0, 0.5, 0.5, 1.0],
        }];
        assert!(
            evaluate_joint_dose_ensemble(
                &correlated,
                &spec,
                &bundle,
                &cref("bundle"),
                &masks,
                &[MetricRequest {
                    id: "m".into(),
                    region: "both".into(),
                    quantity: MetricQuantity::PhysicalTotal,
                    kind: MetricKind::Mean,
                }],
                &adapters,
                &groups,
                None,
                cref("joint"),
                "report.test",
                "test",
            )
            .is_err()
        );
        // Grouped together, the correlated pair is attributed jointly.
        let grouped: Vec<Vec<String>> = vec![vec!["uptake".into(), "photon-cal".into()]];
        let report = evaluate_joint_dose_ensemble(
            &correlated,
            &spec,
            &bundle,
            &cref("bundle"),
            &masks,
            &[MetricRequest {
                id: "m".into(),
                region: "both".into(),
                quantity: MetricQuantity::PhysicalTotal,
                kind: MetricKind::Mean,
            }],
            &adapters,
            &grouped,
            None,
            cref("joint"),
            "report.test",
            "test",
        )
        .unwrap();
        let attr = &report.attribution[0];
        assert_eq!(attr.effects.len(), 1);
        assert_eq!(attr.effects[0].group, "uptake+photon-cal");
    }

    #[test]
    fn component_scales_realize_and_adjust_total() {
        let bundle = rate_bundle();
        let masks = vec![mask("both", &[true, true])];
        let joint = input(
            vec![scale_source(
                "uptake",
                vec![SourceTarget::Component {
                    name: "boron".into(),
                }],
            )],
            vec![cref("bundle")],
        );
        let adapters = vec![DoseAdapter::ComponentScale {
            source_id: "uptake".into(),
            component: DoseComponent::Boron,
        }];
        // Weighted realization at s=0.5: boron halves everywhere.
        let spec = EnsembleSpec {
            method: EnsembleMethod::WeightedSamples {
                realizations: vec![openbnct_core::Realization {
                    weight: 1.0,
                    draws: vec![openbnct_core::SourceDraw {
                        source_id: "uptake".into(),
                        target: None,
                        values: vec![0.5],
                    }],
                }],
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let report = evaluate_joint_dose_ensemble(
            &joint,
            &spec,
            &bundle,
            &cref("bundle"),
            &masks,
            &[MetricRequest {
                id: "mean_boron".into(),
                region: "both".into(),
                quantity: MetricQuantity::Component(DoseComponent::Boron),
                kind: MetricKind::Mean,
            }],
            &adapters,
            &[],
            None,
            cref("joint"),
            "report.test",
            "test",
        )
        .unwrap();
        let m = &report.metrics[0];
        assert_eq!(m.mean, 2.0);
        assert_eq!(m.std_dev, 0.0);
        assert_eq!(m.unit, "gray_per_source_particle");
    }

    #[test]
    fn d95_follows_per_realization_order_statistics() {
        // Two voxels whose hot dose swaps across realizations: the
        // per-realization D95 reads the order statistic of each realized
        // map (1.15), not any per-voxel summary (voxel means give 2.5).
        let bundle = rate_bundle();
        let masks = vec![
            mask("v0", &[true, false]),
            mask("v1", &[false, true]),
            mask("both", &[true, true]),
        ];
        let joint = input(
            vec![
                scale_source("s0", vec![SourceTarget::Region { name: "v0".into() }]),
                scale_source("s1", vec![SourceTarget::Region { name: "v1".into() }]),
            ],
            vec![cref("bundle")],
        );
        let adapters = vec![
            DoseAdapter::RegionComponentScale {
                source_id: "s0".into(),
                region: "v0".into(),
                component: DoseComponent::Boron,
            },
            DoseAdapter::RegionComponentScale {
                source_id: "s1".into(),
                region: "v1".into(),
                component: DoseComponent::Boron,
            },
        ];
        let draw = |s0: f64, s1: f64| openbnct_core::Realization {
            weight: 0.5,
            draws: vec![
                openbnct_core::SourceDraw {
                    source_id: "s0".into(),
                    target: None,
                    values: vec![s0],
                },
                openbnct_core::SourceDraw {
                    source_id: "s1".into(),
                    target: None,
                    values: vec![s1],
                },
            ],
        };
        let spec = EnsembleSpec {
            method: EnsembleMethod::WeightedSamples {
                realizations: vec![draw(1.0, 0.25), draw(0.25, 1.0)],
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: true,
        };
        let report = evaluate_joint_dose_ensemble(
            &joint,
            &spec,
            &bundle,
            &cref("bundle"),
            &masks,
            &[
                d95_metric(),
                MetricRequest {
                    id: "mean_boron".into(),
                    region: "both".into(),
                    quantity: MetricQuantity::Component(DoseComponent::Boron),
                    kind: MetricKind::Mean,
                },
            ],
            &adapters,
            &[],
            None,
            cref("joint"),
            "report.test",
            "test",
        )
        .unwrap();
        let d95 = report.metrics.iter().find(|m| m.metric == "d95").unwrap();
        // Realized maps [4,1] and [1,4]: D95 interpolates between sorted
        // order stats → 0.95·1 + 0.05·4 = 1.15 each realization.
        assert!((d95.mean - 1.15).abs() < 1e-12);
        assert_eq!(d95.std_dev, 0.0);
        assert!(
            d95.samples
                .as_ref()
                .unwrap()
                .iter()
                .all(|v| (*v - 1.15).abs() < 1e-12)
        );
        // The voxel-wise mean map is [2.5, 2.5] — a "D95 of means" would
        // report 2.5, which the ensemble correctly never produces.
        let mean = report
            .metrics
            .iter()
            .find(|m| m.metric == "mean_boron")
            .unwrap();
        assert_eq!(mean.mean, 2.5);
    }

    #[test]
    fn pk_amplitude_scale_is_hand_computable() {
        // Constant concentration (rate 0): I(t) = a·t, boron dose =
        // S·b·(a·t)/C_plan — linear in the amplitude draw, so the
        // metric variance is exact by hand.
        let bundle = rate_bundle();
        let masks = vec![mask("tumor", &[true, true])];
        let pk_model = PkModel {
            schema_version: crate::PK_MODEL_SCHEMA.into(),
            id: "pk.test".into(),
            regions: vec![crate::PkRegion {
                region: "tumor".into(),
                planned_concentration_ppm: 20.0,
                amplitudes_ppm: vec![20.0],
                rates_per_s: vec![0.0],
            }],
            basis: "synthetic constant".into(),
        };
        let joint = input(
            vec![scale_source("pk-amp", vec![SourceTarget::Global])],
            vec![cref("bundle"), cref("pk")],
        );
        let pk = PkIntegration {
            model: pk_model,
            model_ref: cref("pk"),
            bindings: vec![PkBinding::AmplitudeScale {
                source_id: "pk-amp".into(),
                region: "tumor".into(),
            }],
            beam_on_epoch_s: 0.0,
            beam_time_s: 10.0,
            source_strength_per_s: 3.0,
        };
        let draw = |s: f64| openbnct_core::Realization {
            weight: 0.5,
            draws: vec![openbnct_core::SourceDraw {
                source_id: "pk-amp".into(),
                target: None,
                values: vec![s],
            }],
        };
        let spec = EnsembleSpec {
            method: EnsembleMethod::WeightedSamples {
                realizations: vec![draw(0.8), draw(1.2)],
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let report = evaluate_joint_dose_ensemble(
            &joint,
            &spec,
            &bundle,
            &cref("bundle"),
            &masks,
            &[MetricRequest {
                id: "mean_boron".into(),
                region: "tumor".into(),
                quantity: MetricQuantity::Component(DoseComponent::Boron),
                kind: MetricKind::Mean,
            }],
            &[],
            &[],
            Some(&pk),
            cref("joint"),
            "report.test",
            "test",
        )
        .unwrap();
        let m = &report.metrics[0];
        // I = s·20·10/20 = 10s s; dose = 3·b·10s = 120s e-12·... with
        // b=4: 120s Gy·sp-scaled = 120s (units gray). mean_b = 4 →
        // mean dose = s·120. s∈{0.8,1.2}: mean 120, std 24.
        assert!((m.mean - 120.0).abs() < 1e-9);
        assert!((m.std_dev - 24.0).abs() < 1e-9);
        assert_eq!(m.unit, "gray");
    }

    #[test]
    fn pk_point_draw_recovers_the_static_bundle() {
        // Zero declared variance must equal the existing deterministic
        // PK path exactly.
        let bundle = rate_bundle();
        let masks = vec![mask("tumor", &[true, true])];
        let pk_model = PkModel {
            schema_version: crate::PK_MODEL_SCHEMA.into(),
            id: "pk.test".into(),
            regions: vec![crate::PkRegion {
                region: "tumor".into(),
                planned_concentration_ppm: 20.0,
                amplitudes_ppm: vec![20.0],
                rates_per_s: vec![0.0],
            }],
            basis: "synthetic".into(),
        };
        let direct =
            pk_integrated_dose_bundle(&bundle, &pk_model, &masks, 0.0, 10.0, 3.0, "x".into())
                .unwrap();
        let mut source = scale_source("pk-amp", vec![SourceTarget::Global]);
        source.distribution = Distribution::Point { value: 1.0 };
        let joint = input(vec![source], vec![cref("bundle"), cref("pk")]);
        let pk = PkIntegration {
            model: pk_model,
            model_ref: cref("pk"),
            bindings: vec![PkBinding::AmplitudeScale {
                source_id: "pk-amp".into(),
                region: "tumor".into(),
            }],
            beam_on_epoch_s: 0.0,
            beam_time_s: 10.0,
            source_strength_per_s: 3.0,
        };
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 4,
                seed: 1,
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let report = evaluate_joint_dose_ensemble(
            &joint,
            &spec,
            &bundle,
            &cref("bundle"),
            &masks,
            &[MetricRequest {
                id: "mean_boron".into(),
                region: "tumor".into(),
                quantity: MetricQuantity::Component(DoseComponent::Boron),
                kind: MetricKind::Mean,
            }],
            &[],
            &[],
            Some(&pk),
            cref("joint"),
            "report.test",
            "test",
        )
        .unwrap();
        let expected = mean(
            &direct
                .components
                .iter()
                .find(|c| c.component == DoseComponent::Boron)
                .unwrap()
                .values,
        );
        assert_eq!(report.metrics[0].mean, expected);
        assert_eq!(report.metrics[0].std_dev, 0.0);
    }

    #[test]
    fn correlated_pk_log_scales_draw_together() {
        // ρ = ±1 correlation between log-scales must draw perfectly
        // correlated/anticorrelated pairs — the copula is exercised, not
        // approximated.
        let mut source = scale_source("pk-mv", vec![SourceTarget::Global]);
        source.distribution = Distribution::MultivariateNormal {
            mean: vec![0.0, 0.0],
            covariance: vec![0.01, 0.01, 0.01, 0.01],
        };
        source.support = Support::Real;
        source.unit = "1".into();
        let joint = input(vec![source], vec![cref("bundle")]);
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 8,
                seed: 5,
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let realizations = openbnct_core::realize_ensemble(&joint, &spec).unwrap();
        for r in &realizations {
            let v = r.vector("pk-mv").unwrap();
            // ρ=1 with equal σ: both coordinates are the same draw —
            // up to the Cholesky factor's last-ulp rounding.
            assert!((v[0] - v[1]).abs() < 1e-15, "{:?}", v);
        }
        // Anticorrelated: v[0] == −v[1].
        let mut joint = joint.clone();
        joint.correlations = vec![openbnct_core::CorrelationGroup {
            sources: vec!["pk-mv".into(), "pk-mv2".into()],
            correlation: vec![1.0, -1.0, -1.0, 1.0],
        }];
        joint.sources.push({
            let mut s = scale_source("pk-mv2", vec![SourceTarget::Global]);
            s.distribution = Distribution::Normal {
                mean: 0.0,
                std_dev: 0.1,
            };
            s.support = Support::Real;
            s
        });
        // Replace mv source with a scalar normal of the same σ so the
        // group is scalar+scalar.
        joint.sources[0] = {
            let mut s = scale_source("pk-mv", vec![SourceTarget::Global]);
            s.distribution = Distribution::Normal {
                mean: 0.0,
                std_dev: 0.1,
            };
            s.support = Support::Real;
            s
        };
        let realizations = openbnct_core::realize_ensemble(&joint, &spec).unwrap();
        for r in &realizations {
            let a = r.scalar("pk-mv").unwrap();
            let b = r.scalar("pk-mv2").unwrap();
            assert!((a + b).abs() < 1e-12, "anticorrelated draws: {a} {b}");
        }
    }

    #[test]
    fn unresolved_source_and_stale_binding_fail() {
        let bundle = rate_bundle();
        let masks = vec![mask("both", &[true, true])];
        let joint = input(
            vec![scale_source("declared", vec![SourceTarget::Global])],
            vec![cref("bundle")],
        );
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 4,
                seed: 1,
            },
            quantiles: vec![0.5],
            quantile_estimator: openbnct_core::QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        // Adapter references an undeclared source.
        assert!(
            evaluate_joint_dose_ensemble(
                &joint,
                &spec,
                &bundle,
                &cref("bundle"),
                &masks,
                &[MetricRequest {
                    id: "m".into(),
                    region: "both".into(),
                    quantity: MetricQuantity::PhysicalTotal,
                    kind: MetricKind::Mean,
                }],
                &[DoseAdapter::OutputScale {
                    source_id: "ghost".into()
                }],
                &[],
                None,
                cref("joint"),
                "r",
                "t",
            )
            .is_err()
        );
        // Input does not describe this bundle (stale binding).
        assert!(matches!(
            evaluate_joint_dose_ensemble(
                &joint,
                &spec,
                &bundle,
                &cref("other-bundle"),
                &masks,
                &[MetricRequest {
                    id: "m".into(),
                    region: "both".into(),
                    quantity: MetricQuantity::PhysicalTotal,
                    kind: MetricKind::Mean,
                }],
                &[],
                &[],
                None,
                cref("joint"),
                "r",
                "t",
            ),
            Err(UqEnsembleError::StaleReference { .. })
        ));
        // Unknown metric region.
        assert!(
            evaluate_joint_dose_ensemble(
                &joint,
                &spec,
                &bundle,
                &cref("bundle"),
                &masks,
                &[MetricRequest {
                    id: "m".into(),
                    region: "nowhere".into(),
                    quantity: MetricQuantity::PhysicalTotal,
                    kind: MetricKind::Mean,
                }],
                &[],
                &[],
                None,
                cref("joint"),
                "r",
                "t",
            )
            .is_err()
        );
    }
}
