// SPDX-License-Identifier: MIT

//! Nuclear-data uncertainty propagation through the deterministic
//! multigroup path (`openbnct.multigroup-covariance/0.1.0` in,
//! `openbnct.dose-uncertainty-budget/0.1.0` out).
//!
//! The sensitivity of a folded scalar response
//! `R = Σ_c V_c Σ_g σ_d[c][g]·φ[c][g]` to each declared multigroup
//! parameter is computed by **central finite difference** on the
//! forward solve — `S = (R(θ+δθ) − R(θ−δθ)) / 2δθ` — the exact
//! derivative of the discrete model actually shipped, positivity
//! clamps and uncollided split included. (Adjoint-weighted GPT inner
//! products were considered and measured against finite differences;
//! the diamond-difference sweep's positivity clamping makes the
//! discrete operator nonlinear, so the continuous-adjoint estimate
//! carries a systematic discretization bias — it remains the right
//! tool for CADIS importance *ratios*, not for committed sensitivity
//! magnitudes.) Response-vector entries are linear in `R` and use the
//! exact analytic derivative `S = Σ_{c∈m} V_c·φ[c][g]` instead of a
//! redundant re-solve.
//!
//! Propagation is the quadratic form `σ²(R) = Sᵀ·C·S` over the
//! declared covariance — diagonal entries are independent relative
//! standard deviations; `blocks` carry dense group-correlation
//! matrices for one material's `sigma_total` or dose-response vector
//! (the dominant correlations in collapsed data). Each perturbed
//! parameter costs two forward solves — declared covariance sets are
//! expected to be curated (the dominant groups), not exhaustive.
//!
//! Honest bounds: first-order sensitivities, declared covariances (not
//! yet ENDF-derived — the NJOY covariance chain is a separate step),
//! and the scalar *integrated* response, not per-voxel dose maps.

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::TransportCase;
use crate::multigroup::{
    MultigroupData, MultigroupError, MultigroupFlux, SnOptions, cell_materials, solve_multigroup,
    solve_multigroup_unchecked,
};

/// Central-difference step as a fraction of the parameter value.
pub const FD_REL_STEP: f64 = 1.0e-3;
/// Absolute step floor so zero-valued declared parameters still get a
/// meaningful derivative when an uncertainty is declared on them.
pub const FD_ABS_FLOOR: f64 = 1.0e-8;

pub const MULTIGROUP_COVARIANCE_SCHEMA: &str = "openbnct.multigroup-covariance/0.1.0";
pub const DOSE_UNCERTAINTY_BUDGET_SCHEMA: &str = "openbnct.dose-uncertainty-budget/0.1.0";

/// Which declared quantity an uncertainty entry perturbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CovarianceParameter {
    /// `sigma_total_per_cm[g]` — removal cross section.
    SigmaTotal,
    /// `scatter_matrix_per_cm[g_from][g_to]` — one transfer entry.
    Scatter,
    /// `dose_response_gy_cm2[component][g]` — folded response.
    DoseResponse,
}

/// Independent relative standard deviations over a set of scalar
/// parameters of one material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CovarianceDiagonal {
    pub material_id: String,
    pub parameter: CovarianceParameter,
    /// Required for `dose_response`; ignored otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Groups the entry covers. For `sigma_total`/`dose_response` these
    /// are group indices; for `scatter` they are `[g_from, g_to]` pairs
    /// — same field, two shapes, disambiguated by `parameter`.
    pub groups: Vec<[u32; 2]>,
    /// Relative standard deviation applied to the parameter's declared
    /// value (σ = rel·|θ|).
    pub relative_std_dev: f64,
}

/// Dense group-correlation block for one material's `sigma_total` or
/// `dose_response` vector — `cov[g][h] = rel[g]·rel[h]·corr[g][h]`
/// times parameter magnitudes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CovarianceBlock {
    pub material_id: String,
    pub parameter: CovarianceParameter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// Relative standard deviation per group, length = group count.
    pub relative_std_dev: Vec<f64>,
    /// Row-major G×G correlation matrix — symmetric with unit diagonal.
    pub correlation: Vec<f64>,
}

/// Declared covariance over an `openbnct.multigroup-data` artifact.
/// The data itself stays nominal; every uncertainty lives here so the
/// nominal artifact remains a clean reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MultigroupCovariance {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content-bound reference to the `openbnct.multigroup-data`
    /// artifact these uncertainties describe.
    pub multigroup_data: ContentReference,
    /// Independent (diagonal) entries.
    #[serde(default)]
    pub diagonal: Vec<CovarianceDiagonal>,
    /// Dense group-correlation blocks.
    #[serde(default)]
    pub blocks: Vec<CovarianceBlock>,
    /// Where the declared covariances come from — required; "guessed"
    /// is acceptable to declare, silence is not.
    pub provenance_note: String,
    pub qualification: String,
}

/// One scalar parameter's contribution to the budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetEntry {
    /// `nuclear_data` for σ_t/σ_s, `response_data` for dose-response,
    /// `statistical` for a declared MC statistical σ.
    pub source: String,
    /// Human-readable parameter address, e.g. `absorber.sigma_total[0]`.
    pub parameter: String,
    /// Absolute sensitivity `∂R/∂θ` (Gy·cm² per parameter unit).
    /// `NaN` for whole-block and statistical entries — serialized as
    /// `null` and read back as `NaN` so the contract round-trips.
    #[serde(
        serialize_with = "serialize_nan_as_null",
        deserialize_with = "deserialize_null_as_nan"
    )]
    pub sensitivity: f64,
    /// Absolute standard deviation of the parameter; `NaN` like
    /// `sensitivity` when a per-parameter σ is not applicable.
    #[serde(
        serialize_with = "serialize_nan_as_null",
        deserialize_with = "deserialize_null_as_nan"
    )]
    pub std_dev: f64,
    /// `Sᵀ·C·S` contribution of this entry (or block) — can be negative
    /// only for correlated blocks reported as a whole.
    pub variance_contribution: f64,
    /// Share of the total propagated variance.
    pub relative_contribution: f64,
}

/// Decomposed uncertainty budget for one integrated folded response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoseUncertaintyBudget {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// The folded component this budget describes.
    pub component: String,
    /// Integrated response `R = Σ_c V_c·Σ_g σ_d·φ` — the scalar the
    /// budget refers to.
    pub response_integral: f64,
    pub entries: Vec<BudgetEntry>,
    /// Sqrt of summed variance contributions, relative to
    /// `response_integral`.
    pub total_relative_std_dev: f64,
    /// Content-bound inputs: covariance artifact, multigroup data,
    /// transport case, and the forward/adjoint solves when they were
    /// supplied or persisted.
    pub covariance: ContentReference,
    pub multigroup_data: ContentReference,
    pub case: ContentReference,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fluxes: Vec<ContentReference>,
    pub method_note: String,
    pub qualification: String,
}

/// `NaN` ↔ `null`: block/statistical budget entries carry no
/// per-parameter sensitivity, and serde_json would otherwise write
/// `null` that `f64` deserialization refuses — breaking round-trip.
fn serialize_nan_as_null<S: serde::Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
    if value.is_nan() {
        s.serialize_none()
    } else {
        s.serialize_f64(*value)
    }
}

fn deserialize_null_as_nan<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
}

impl MultigroupCovariance {
    pub fn validate(&self) -> Result<(), UqError> {
        if !openbnct_core::schema_matches(&self.schema_version, MULTIGROUP_COVARIANCE_SCHEMA) {
            return Err(UqError::Invalid(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        if self.id.is_empty() || self.provenance_note.is_empty() || self.qualification.is_empty() {
            return Err(UqError::Invalid(
                "id, provenance_note, and qualification are required".into(),
            ));
        }
        if self.diagonal.is_empty() && self.blocks.is_empty() {
            return Err(UqError::Invalid(
                "at least one diagonal entry or block is required".into(),
            ));
        }
        for entry in &self.diagonal {
            if entry.material_id.is_empty()
                || entry.groups.is_empty()
                || !(entry.relative_std_dev.is_finite() && entry.relative_std_dev >= 0.0)
            {
                return Err(UqError::Invalid(
                    "diagonal entries need a material, ≥1 group, and a finite \
                     non-negative relative_std_dev"
                        .into(),
                ));
            }
            if entry.parameter == CovarianceParameter::DoseResponse
                && entry.component.as_deref().unwrap_or("").is_empty()
            {
                return Err(UqError::Invalid(
                    "dose_response diagonal entries need a component".into(),
                ));
            }
        }
        for block in &self.blocks {
            if block.parameter == CovarianceParameter::Scatter {
                return Err(UqError::Invalid(
                    "scatter blocks are not supported — list scatter \
                     uncertainties as diagonal [g_from,g_to] entries"
                        .into(),
                ));
            }
            let n = block.relative_std_dev.len();
            if n == 0
                || block.correlation.len() != n * n
                || block
                    .relative_std_dev
                    .iter()
                    .any(|s| !s.is_finite() || *s < 0.0)
            {
                return Err(UqError::Invalid(
                    "blocks need per-group relative_std_dev and a G×G \
                     correlation matrix"
                        .into(),
                ));
            }
            for g in 0..n {
                if (block.correlation[g * n + g] - 1.0).abs() > 1e-9 {
                    return Err(UqError::Invalid(
                        "correlation matrix must have unit diagonal".into(),
                    ));
                }
                for h in 0..n {
                    if (block.correlation[g * n + h] - block.correlation[h * n + g]).abs() > 1e-9 {
                        return Err(UqError::Invalid(
                            "correlation matrix must be symmetric".into(),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

impl DoseUncertaintyBudget {
    pub fn validate(&self) -> Result<(), UqError> {
        if !openbnct_core::schema_matches(&self.schema_version, DOSE_UNCERTAINTY_BUDGET_SCHEMA) {
            return Err(UqError::Invalid(format!(
                "unsupported schema {:?}",
                self.schema_version
            )));
        }
        if self.id.is_empty()
            || self.component.is_empty()
            || self.method_note.is_empty()
            || self.qualification.is_empty()
        {
            return Err(UqError::Invalid(
                "id, component, method_note, and qualification are required".into(),
            ));
        }
        if !self.response_integral.is_finite()
            || !self.total_relative_std_dev.is_finite()
            || self.total_relative_std_dev < 0.0
        {
            return Err(UqError::Invalid(
                "response_integral must be finite and total_relative_std_dev ≥ 0".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum UqError {
    #[error("uncertainty propagation: {0}")]
    Invalid(String),
    #[error("{0}")]
    Solve(#[from] MultigroupError),
    #[error("{0}")]
    Geometry(#[from] openbnct_core::ValidationError),
    #[error("{0}")]
    Model(#[from] crate::model::TransportModelError),
    #[error("covariance entry references {0}, which the multigroup data does not declare")]
    UnknownParameter(String),
}

/// Result of a propagation: the budget artifact plus the nominal
/// forward solve it anchored to (for optional serialization and content
/// binding).
#[derive(Debug)]
pub struct UqDerivation {
    pub budget: DoseUncertaintyBudget,
    pub forward_flux: MultigroupFlux,
    /// Number of perturbed forward solves the finite differences ran —
    /// recorded so the cost of the declared covariance set is auditable.
    pub perturbed_solves: usize,
}

/// Propagate a declared multigroup covariance to the integrated dose
/// response `R` for `component`.
///
/// `forward_flux` may supply a precomputed forward solve; `None`
/// computes one in-process. Each perturbed nuclear-data parameter costs
/// two further forward solves (central difference); dose-response and
/// statistical entries are exact analytic terms.
#[allow(clippy::too_many_arguments)]
pub fn propagate_uncertainty(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    covariance: &MultigroupCovariance,
    component: &str,
    forward_flux: Option<&MultigroupFlux>,
    statistical_rel_std: Option<f64>,
    budget_id: &str,
    covariance_ref: ContentReference,
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<UqDerivation, UqError> {
    case.validate()?;
    data.validate()?;
    covariance.validate()?;
    if covariance.multigroup_data.id != data.id {
        return Err(UqError::Invalid(format!(
            "covariance {} describes data {:?}, got {:?}",
            covariance.id, covariance.multigroup_data.id, data.id
        )));
    }
    let invalid = |m: String| UqError::Invalid(m);
    let geometry = &case.geometry;
    let n_cells = geometry.voxel_count()?;
    let groups = data.group_count();
    let case_material = cell_materials(case, data, options.assignment.as_ref())?;
    let cell_volume_cm3 = geometry.spacing_mm.iter().product::<f64>() / 1000.0;

    // The forward solve (or the supplied flux) anchors R(θ).
    let forward = match forward_flux {
        Some(flux) => {
            if flux.flux.len() != n_cells
                || flux.flux.iter().any(|row| row.len() != groups)
                || flux.energy_boundaries_ev != data.energy_boundaries_ev
            {
                return Err(invalid(
                    "supplied forward flux does not match the case/data structure".into(),
                ));
            }
            flux.clone()
        }
        None => solve_multigroup(case, data, options, data_ref.clone(), case_ref.clone())?,
    };
    // The component's response must be declared somewhere.
    if !case_material.iter().any(|&m| {
        data.materials[m]
            .dose_response_gy_cm2
            .contains_key(component)
    }) {
        return Err(UqError::UnknownParameter(format!(
            "dose_response component {component:?}"
        )));
    }

    // Response integral and the material-indexed helper views.
    let response_at = |material: usize, group: usize| -> f64 {
        data.materials[material]
            .dose_response_gy_cm2
            .get(component)
            .map_or(0.0, |r| r[group])
    };
    let mut response_integral = 0.0;
    for (cell, &m) in case_material.iter().enumerate().take(n_cells) {
        for g in 0..groups {
            response_integral += cell_volume_cm3 * response_at(m, g) * forward.flux[cell][g];
        }
    }
    if response_integral <= 0.0 || !response_integral.is_finite() {
        return Err(invalid(
            "integrated response is not positive — check the component \
             response and forward solve"
                .into(),
        ));
    }

    // Sensitivities. Nuclear-data entries go through a central finite
    // difference of the actual solver — two solves per parameter —
    // because the SC-fixup DD operator is nonlinear and the
    // continuous-adjoint inner product is only first-order-faithful to
    // it. Response entries are linear in R and exact analytically.
    let integrate = |flux: &MultigroupFlux| -> f64 {
        let mut r = 0.0;
        for (cell, &m) in case_material.iter().enumerate().take(n_cells) {
            for g in 0..groups {
                r += cell_volume_cm3 * response_at(m, g) * flux.flux[cell][g];
            }
        }
        r
    };
    let mut solves = 0usize;
    let mut sensitivity_fd =
        |set: &mut dyn FnMut(&mut MultigroupData, f64), theta: f64| -> Result<f64, UqError> {
            let delta = FD_REL_STEP * theta.abs().max(FD_ABS_FLOOR);
            let mut data_hi = data.clone();
            set(&mut data_hi, theta + delta);
            let flux_hi = solve_multigroup_unchecked(
                case,
                &data_hi,
                options,
                data_ref.clone(),
                case_ref.clone(),
            )?;
            let mut data_lo = data.clone();
            set(&mut data_lo, theta - delta);
            let flux_lo = solve_multigroup_unchecked(
                case,
                &data_lo,
                options,
                data_ref.clone(),
                case_ref.clone(),
            )?;
            solves += 2;
            Ok((integrate(&flux_hi) - integrate(&flux_lo)) / (2.0 * delta))
        };
    let s_response = |material: usize, g: usize| -> f64 {
        (0..n_cells)
            .filter(|c| case_material[*c] == material)
            .map(|c| cell_volume_cm3 * forward.flux[c][g])
            .sum()
    };
    let material_index = |material_id: &str| -> Result<usize, UqError> {
        data.materials
            .iter()
            .position(|m| m.material_id == material_id)
            .ok_or_else(|| UqError::UnknownParameter(material_id.to_string()))
    };

    let mut entries: Vec<BudgetEntry> = Vec::new();
    let mut total_variance = 0.0;

    // Diagonal (independent) entries.
    for entry in &covariance.diagonal {
        let m = material_index(&entry.material_id)?;
        for pair in &entry.groups {
            let (sens, theta, address) = match entry.parameter {
                CovarianceParameter::SigmaTotal => {
                    let g = pair[0] as usize;
                    if g >= groups {
                        return Err(UqError::UnknownParameter(format!(
                            "{}.sigma_total[{g}]",
                            entry.material_id
                        )));
                    }
                    let theta = data.materials[m].sigma_total_per_cm[g];
                    let sens = if entry.relative_std_dev * theta.abs() > 0.0 {
                        sensitivity_fd(
                            &mut |d: &mut MultigroupData, v: f64| {
                                d.materials[m].sigma_total_per_cm[g] = v
                            },
                            theta,
                        )?
                    } else {
                        0.0
                    };
                    (
                        sens,
                        theta,
                        format!("{}.sigma_total[{g}]", entry.material_id),
                    )
                }
                CovarianceParameter::Scatter => {
                    let (gf, gt) = (pair[0] as usize, pair[1] as usize);
                    if gf >= groups || gt >= groups {
                        return Err(UqError::UnknownParameter(format!(
                            "{}.scatter[{gf}][{gt}]",
                            entry.material_id
                        )));
                    }
                    let theta = data.materials[m].scatter_matrix_per_cm[gf * groups + gt];
                    let sens = if entry.relative_std_dev * theta.abs() > 0.0 {
                        sensitivity_fd(
                            &mut |d: &mut MultigroupData, v: f64| {
                                d.materials[m].scatter_matrix_per_cm[gf * groups + gt] = v
                            },
                            theta,
                        )?
                    } else {
                        0.0
                    };
                    (
                        sens,
                        theta,
                        format!("{}.scatter[{gf}→{gt}]", entry.material_id),
                    )
                }
                CovarianceParameter::DoseResponse => {
                    let g = pair[0] as usize;
                    let comp = entry.component.as_deref().unwrap_or("");
                    let theta = data.materials[m]
                        .dose_response_gy_cm2
                        .get(comp)
                        .and_then(|r| r.get(g))
                        .copied()
                        .ok_or_else(|| {
                            UqError::UnknownParameter(format!(
                                "{}.dose_response[{comp}][{g}]",
                                entry.material_id
                            ))
                        })?;
                    (
                        s_response(m, g),
                        theta,
                        format!("{}.dose_response[{comp}][{g}]", entry.material_id),
                    )
                }
            };
            let std_dev = entry.relative_std_dev * theta.abs();
            let variance = (sens * std_dev).powi(2);
            total_variance += variance;
            entries.push(BudgetEntry {
                source: match entry.parameter {
                    CovarianceParameter::DoseResponse => "response_data",
                    _ => "nuclear_data",
                }
                .into(),
                parameter: address,
                sensitivity: sens,
                std_dev,
                variance_contribution: variance,
                relative_contribution: 0.0,
            });
        }
    }

    // Dense blocks: Sᵀ·C·S over each (material, parameter) group vector.
    for block in &covariance.blocks {
        let m = material_index(&block.material_id)?;
        if block.relative_std_dev.len() != groups {
            return Err(invalid(format!(
                "block {}.{:?} has {} groups, expected {groups}",
                block.material_id,
                block.parameter,
                block.relative_std_dev.len()
            )));
        }
        let mut sens = Vec::with_capacity(groups);
        let mut theta = Vec::with_capacity(groups);
        for g in 0..groups {
            let (s, t) = match block.parameter {
                CovarianceParameter::SigmaTotal => {
                    let theta = data.materials[m].sigma_total_per_cm[g];
                    let sens = if block.relative_std_dev[g] * theta.abs() > 0.0 {
                        sensitivity_fd(
                            &mut |d: &mut MultigroupData, v: f64| {
                                d.materials[m].sigma_total_per_cm[g] = v
                            },
                            theta,
                        )?
                    } else {
                        0.0
                    };
                    (sens, theta)
                }
                CovarianceParameter::DoseResponse => {
                    let comp = block.component.as_deref().unwrap_or("");
                    let t = data.materials[m]
                        .dose_response_gy_cm2
                        .get(comp)
                        .and_then(|r| r.get(g))
                        .copied()
                        .ok_or_else(|| {
                            UqError::UnknownParameter(format!(
                                "{}.dose_response[{comp}][{g}]",
                                block.material_id
                            ))
                        })?;
                    (s_response(m, g), t)
                }
                CovarianceParameter::Scatter => {
                    unreachable!("scatter blocks rejected at validate")
                }
            };
            sens.push(s);
            theta.push(t);
        }
        let mut variance = 0.0;
        for g in 0..groups {
            for h in 0..groups {
                variance += sens[g]
                    * sens[h]
                    * block.relative_std_dev[g]
                    * theta[g].abs()
                    * block.relative_std_dev[h]
                    * theta[h].abs()
                    * block.correlation[g * groups + h];
            }
        }
        total_variance += variance;
        entries.push(BudgetEntry {
            source: match block.parameter {
                CovarianceParameter::DoseResponse => "response_data",
                _ => "nuclear_data",
            }
            .into(),
            parameter: format!(
                "{}.{:?}{}",
                block.material_id,
                block.parameter,
                block
                    .component
                    .as_ref()
                    .map(|c| format!("[{c}]"))
                    .unwrap_or_default()
            ),
            sensitivity: f64::NAN,
            std_dev: f64::NAN,
            variance_contribution: variance,
            relative_contribution: 0.0,
        });
    }

    // Declared statistical contribution (e.g. the MC run's σ on the same
    // integral) enters as an independent variance.
    if let Some(rel) = statistical_rel_std {
        if !(rel.is_finite() && rel >= 0.0) {
            return Err(invalid("statistical_rel_std must be finite and ≥ 0".into()));
        }
        let variance = (rel * response_integral).powi(2);
        total_variance += variance;
        entries.push(BudgetEntry {
            source: "statistical".into(),
            parameter: "monte_carlo_statistics".into(),
            sensitivity: f64::NAN,
            std_dev: rel * response_integral,
            variance_contribution: variance,
            relative_contribution: 0.0,
        });
    }

    for entry in &mut entries {
        entry.relative_contribution = if total_variance > 0.0 {
            entry.variance_contribution / total_variance
        } else {
            0.0
        };
    }

    let budget = DoseUncertaintyBudget {
        schema_version: DOSE_UNCERTAINTY_BUDGET_SCHEMA.into(),
        id: budget_id.into(),
        case_id: case.case_id.clone(),
        component: component.into(),
        response_integral,
        entries,
        total_relative_std_dev: total_variance.sqrt() / response_integral,
        covariance: covariance_ref,
        multigroup_data: data_ref,
        case: case_ref,
        fluxes: Vec::new(),
        method_note: format!(
            "central-difference sensitivities (δ_rel={FD_REL_STEP}) on the \
             deterministic solve times the declared covariance; diagonal \
             entries independent, blocks dense group-correlated; scatter \
             entries uncorrelated (declared-data limitation); {solves} perturbed solves"
        ),
        qualification: "research-verification uncertainty accounting — declared \
                        covariances, first-order sensitivities, integrated \
                        response; not a clinical tolerance evaluation"
            .into(),
    };
    budget.validate()?;
    Ok(UqDerivation {
        budget,
        forward_flux: forward,
        perturbed_solves: solves,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multigroup::tests::{data as slab_data, options, slab_case};

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    fn covariance(diagonal: Vec<CovarianceDiagonal>) -> MultigroupCovariance {
        MultigroupCovariance {
            schema_version: MULTIGROUP_COVARIANCE_SCHEMA.into(),
            id: "openbnct.test.cov".into(),
            multigroup_data: cref("mg-data"),
            diagonal,
            blocks: vec![],
            provenance_note: "test".into(),
            qualification: "test_only".into(),
        }
    }

    fn data_with_response(sigma: f64) -> MultigroupData {
        let mut mg = slab_data(&[sigma], vec![0.0]);
        mg.materials[0]
            .dose_response_gy_cm2
            .insert("boron".into(), vec![1e-4]);
        mg
    }

    /// The committed σ_t sensitivity is a central difference at
    /// FD_REL_STEP; an independent one-sided difference at a different
    /// step must agree within discretization of the derivative —
    /// the step-stability check that the derivative is real.
    #[test]
    fn sigma_t_sensitivity_step_stable() {
        let case = slab_case();
        let mg = data_with_response(0.5);
        let cov = covariance(vec![CovarianceDiagonal {
            material_id: "absorber".into(),
            parameter: CovarianceParameter::SigmaTotal,
            component: None,
            groups: vec![[0, 0]],
            relative_std_dev: 0.02,
        }]);
        let opts = options();
        let derivation = propagate_uncertainty(
            &case,
            &mg,
            &opts,
            &cov,
            "boron",
            None,
            None,
            "budget",
            cref("cov"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let s_committed = derivation.budget.entries[0].sensitivity;
        assert!(s_committed < 0.0, "removal sensitivity must be negative");
        assert_eq!(derivation.perturbed_solves, 2);
        // Independent one-sided difference at a 0.5% step.
        let eps = 0.005_f64;
        let mut perturbed = mg.clone();
        perturbed.materials[0].sigma_total_per_cm[0] = 0.5 * (1.0 + eps);
        let flux_p =
            crate::solve_multigroup(&case, &perturbed, &opts, cref("d"), cref("c")).unwrap();
        let cell_volume = 0.001_f64; // 1 mm³
        let r_p: f64 = flux_p
            .flux
            .iter()
            .map(|row| cell_volume * 1e-4 * row[0])
            .sum();
        let s_fd = (r_p - derivation.budget.response_integral) / (0.5 * eps);
        let rel = (s_committed - s_fd).abs() / s_fd.abs();
        assert!(
            rel < 0.01,
            "central-difference {s_committed} vs one-sided {s_fd} differ {:.1}%",
            rel * 100.0
        );
    }

    /// Scatter-transfer sensitivity is positive (more transfer into the
    /// response group raises the response) and step-stable.
    #[test]
    fn scatter_sensitivity_step_stable() {
        let case = slab_case();
        // 2 groups, strong downscatter, response only in group 1.
        let mut mg = slab_data(&[1.0, 0.6], vec![0.2, 0.3, 0.0, 0.1]);
        mg.materials[0]
            .dose_response_gy_cm2
            .insert("boron".into(), vec![0.0, 1e-4]);
        let opts = options();
        let cov = covariance(vec![CovarianceDiagonal {
            material_id: "absorber".into(),
            parameter: CovarianceParameter::Scatter,
            component: None,
            groups: vec![[0, 1]],
            relative_std_dev: 0.05,
        }]);
        let derivation = propagate_uncertainty(
            &case,
            &mg,
            &opts,
            &cov,
            "boron",
            None,
            None,
            "budget",
            cref("cov"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let s_committed = derivation.budget.entries[0].sensitivity;
        assert!(
            s_committed > 0.0,
            "transfer into the response group is positive"
        );
        let eps = 0.005_f64;
        let mut perturbed = mg.clone();
        perturbed.materials[0].scatter_matrix_per_cm[1] = 0.3 * (1.0 + eps);
        let flux_p =
            crate::solve_multigroup(&case, &perturbed, &opts, cref("d"), cref("c")).unwrap();
        let cell_volume = 0.001_f64; // 1 mm³
        let r_p: f64 = flux_p
            .flux
            .iter()
            .map(|row| cell_volume * 1e-4 * row[1])
            .sum();
        let s_fd = (r_p - derivation.budget.response_integral) / (0.3 * eps);
        let rel = (s_committed - s_fd).abs() / s_fd.abs();
        assert!(
            rel < 0.02,
            "central-difference {s_committed} vs one-sided {s_fd} differ {:.1}%",
            rel * 100.0
        );
    }

    /// Response-parameter sensitivity is exact: R is linear in σ_d.
    #[test]
    fn response_sensitivity_is_exact() {
        let case = slab_case();
        let mg = data_with_response(0.5);
        let cov = covariance(vec![CovarianceDiagonal {
            material_id: "absorber".into(),
            parameter: CovarianceParameter::DoseResponse,
            component: Some("boron".into()),
            groups: vec![[0, 0]],
            relative_std_dev: 0.03,
        }]);
        let derivation = propagate_uncertainty(
            &case,
            &mg,
            &options(),
            &cov,
            "boron",
            None,
            None,
            "budget",
            cref("cov"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let entry = &derivation.budget.entries[0];
        // S·θ = R exactly (linear response), so the relative budget is 3%.
        let implied = entry.sensitivity * 1e-4;
        assert!(
            (implied - derivation.budget.response_integral).abs()
                < 1e-12 * derivation.budget.response_integral
        );
        assert!((derivation.budget.total_relative_std_dev - 0.03).abs() < 1e-9);
        assert_eq!(entry.source, "response_data");
    }

    /// A fully correlated block produces (S0·σ0 + S1·σ1)², larger than
    /// the diagonal-only quadrature sum — the correlation term shows up.
    #[test]
    fn correlated_block_beats_diagonal_quadrature() {
        let case = slab_case();
        // Two groups so the block has structure; response in both.
        let mut mg = slab_data(&[0.8, 0.5], vec![0.1, 0.2, 0.0, 0.1]);
        mg.materials[0]
            .dose_response_gy_cm2
            .insert("boron".into(), vec![1e-4, 1e-4]);
        let opts = options();
        let mut cov = covariance(vec![]);
        cov.blocks = vec![CovarianceBlock {
            material_id: "absorber".into(),
            parameter: CovarianceParameter::SigmaTotal,
            component: None,
            relative_std_dev: vec![0.02, 0.02],
            correlation: vec![1.0, 1.0, 1.0, 1.0],
        }];
        let derivation = propagate_uncertainty(
            &case,
            &mg,
            &opts,
            &cov,
            "boron",
            None,
            None,
            "budget",
            cref("cov"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        // The emitted JSON must round-trip: block entries serialize
        // their not-applicable sensitivity/std_dev as null.
        let json = serde_json::to_string(&derivation.budget).unwrap();
        assert!(json.contains("\"sensitivity\":null"));
        let back: DoseUncertaintyBudget = serde_json::from_str(&json).unwrap();
        assert!(back.entries[0].sensitivity.is_nan());
        back.validate().unwrap();

        // Recompute the block variance independently: (Σ_g S_g·σ_g)²
        // with S_g from independent central differences.
        let cell_volume = 0.001_f64; // 1 mm³
        let mut s_times_sigma = [0.0_f64; 2];
        for g in 0..2 {
            let theta = [0.8, 0.5][g];
            let d = 1e-3 * theta;
            let mut s_g = [0.0_f64; 2];
            for (i, dv) in [theta + d, theta - d].iter().enumerate() {
                let mut perturbed = mg.clone();
                perturbed.materials[0].sigma_total_per_cm[g] = *dv;
                let flux = crate::solve_multigroup(&case, &perturbed, &opts, cref("d"), cref("c"))
                    .unwrap();
                s_g[i] = flux
                    .flux
                    .iter()
                    .map(|row| cell_volume * (1e-4 * row[0] + 1e-4 * row[1]))
                    .sum();
            }
            let s = (s_g[0] - s_g[1]) / (2.0 * d);
            s_times_sigma[g] = s * 0.02 * theta;
        }
        let variance_check = (s_times_sigma[0] + s_times_sigma[1]).powi(2);
        let reported = derivation.budget.entries[0].variance_contribution;
        assert!(
            (reported - variance_check).abs() < 1e-9 * variance_check,
            "block variance {reported} vs {variance_check}"
        );
        // Diagonal quadrature for the same sigmas is smaller (ρ=+1 adds
        // the 2·S0σ0·S1σ1 cross term — both sensitivities are negative
        // for removal, so the cross term is positive).
        let diag = s_times_sigma[0].powi(2) + s_times_sigma[1].powi(2);
        assert!(variance_check > diag);
    }

    /// Statistical σ enters the total as an independent variance.
    #[test]
    fn statistical_contribution_combines() {
        let case = slab_case();
        let mg = data_with_response(0.5);
        let cov = covariance(vec![CovarianceDiagonal {
            material_id: "absorber".into(),
            parameter: CovarianceParameter::DoseResponse,
            component: Some("boron".into()),
            groups: vec![[0, 0]],
            relative_std_dev: 0.03,
        }]);
        let derivation = propagate_uncertainty(
            &case,
            &mg,
            &options(),
            &cov,
            "boron",
            None,
            Some(0.04),
            "budget",
            cref("cov"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        // sqrt(0.03² + 0.04²) = 0.05.
        assert!((derivation.budget.total_relative_std_dev - 0.05).abs() < 1e-9);
        let stats = derivation
            .budget
            .entries
            .iter()
            .find(|e| e.source == "statistical")
            .unwrap();
        assert!((stats.relative_contribution - 0.64).abs() < 1e-9);
    }

    /// Covariance referencing a different data artifact or an unknown
    /// material fails at validation, not silently.
    #[test]
    fn mismatched_covariance_rejected() {
        let case = slab_case();
        let mg = data_with_response(0.5);
        let mut cov = covariance(vec![CovarianceDiagonal {
            material_id: "absorber".into(),
            parameter: CovarianceParameter::SigmaTotal,
            component: None,
            groups: vec![[0, 0]],
            relative_std_dev: 0.02,
        }]);
        cov.multigroup_data = cref("some.other.data");
        assert!(
            propagate_uncertainty(
                &case,
                &mg,
                &options(),
                &cov,
                "boron",
                None,
                None,
                "b",
                cref("cov"),
                cref("d"),
                cref("c")
            )
            .is_err()
        );
        cov.multigroup_data = cref(&mg.id);
        cov.diagonal[0].material_id = "nonexistent".into();
        assert!(matches!(
            propagate_uncertainty(
                &case,
                &mg,
                &options(),
                &cov,
                "boron",
                None,
                None,
                "b",
                cref("cov"),
                cref("d"),
                cref("c")
            ),
            Err(UqError::UnknownParameter(_))
        ));
    }
}

#[cfg(test)]
mod solver_invariants {
    use super::*;
    use crate::multigroup::tests::{data as slab_data, options, slab_case};
    use crate::multigroup::{BoundarySource, level_symmetric_quadrature, solve_sn_problem};

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    /// Conservation audit: for a unit-density volumetric source in a
    /// pure absorber, total absorptions must not exceed emissions
    /// (faces take the rest). The FD sensitivities inherit whatever the
    /// sweep does, so this guards the layer the UQ rests on.
    #[test]
    fn volumetric_source_conserves() {
        let case = slab_case();
        let mg = slab_data(&[0.5], vec![0.0]);
        let mut opts = options();
        opts.beam_uncollided_split = false;
        let n_cells = 4 * 4 * 20;
        let q = vec![vec![1.0; 1]; n_cells];
        let quadrature = level_symmetric_quadrature(opts.quadrature_order).unwrap();
        let cm = cell_materials(&case, &mg, None).unwrap();
        let forward = solve_sn_problem(
            &case,
            &mg,
            &opts,
            &cm,
            &quadrature,
            &BoundarySource::new(),
            &q,
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let v = 0.001_f64;
        let absorbed: f64 = forward.flux.iter().map(|r| 0.5 * r[0] * v).sum();
        let emitted = (n_cells as f64) * v;
        assert!(absorbed > 0.0);
        assert!(
            absorbed <= emitted * (1.0 + 1e-9),
            "absorbed {absorbed} exceeds emitted {emitted} — sweep gains particles"
        );
    }
}
