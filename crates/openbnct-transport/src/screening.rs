//! Global sensitivity screening over *declared* transport-case inputs
//! (`openbnct.sensitivity-spec/0.1.0` →
//! `openbnct.sensitivity-screening/0.1.0`).
//!
//! Where `uq` propagates covariances into a variance budget, screening
//! answers the upstream question: which declared inputs move the folded
//! response at all, and by how much — the ranking that decides what
//! deserves a covariance. Each parameter is a declared perturbation of
//! the transport case or multigroup data over a stated range:
//!
//! * `material_sigma_total_scale` / `material_scatter_scale` /
//!   `material_response_scale` — multiplicative scales on one
//!   material's nuclear data or response vector.
//! * `source_center_shift_mm` / `source_radius_scale` — beam aperture
//!   positioning and size tolerances on `UniformDisk` sources.
//! * `geometry_origin_shift_mm` — shifts the domain relative to the
//!   fixed world-frame beam (phantom-positioning tolerance).
//!
//! Two methods share one deterministic evaluator (an S_N solve folded
//! to the component's integrated response):
//!
//! * `morris` — the elementary-effects method: `r` random trajectories
//!   through a `p`-level unit hypercube, one factor stepped at a time.
//!   Reports `mu` (mean signed effect), `mu_star` (mean absolute
//!   effect — the ranking statistic), and `sigma` (effect spread —
//!   nonzero flags nonlinearity or interactions).
//! * `sobol` — Saltelli sampling (A, B, and per-parameter AB_i
//!   matrices) with Jansen/Saltelli-2010 estimators for first-order
//!   `s1` and total-order `st` indices; `st − s1` measures the
//!   parameter's interaction share.
//!
//! Every evaluation is a full deterministic solve, so screening cost is
//! `r·(k+1)` solves for Morris and `n·(2k+2)` for Sobol — declared
//! parameter sets are expected to be curated. The seed makes runs
//! bit-reproducible; the same seed replays the same design.

use openbnct_core::{ContentReference, ValidationError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::MaterialAssignment;
use crate::model::{SourceSpatialDistribution, TransportCase};
use crate::multigroup::{
    MultigroupData, MultigroupFlux, SnOptions, material_composition_map, solve_multigroup,
};

pub const SENSITIVITY_SPEC_SCHEMA: &str = "openbnct.sensitivity-spec/0.1.0";
pub const SENSITIVITY_SCREENING_SCHEMA: &str = "openbnct.sensitivity-screening/0.1.0";

#[derive(Debug, Error)]
pub enum ScreeningError {
    #[error("screening validation: {0}")]
    Invalid(String),
    #[error("unknown screening parameter target: {0}")]
    UnknownTarget(String),
    #[error("transport case invalid: {0}")]
    Case(#[from] crate::model::TransportModelError),
    #[error("multigroup data invalid: {0}")]
    Data(#[from] crate::multigroup::MultigroupError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
}

fn invalid(message: String) -> ScreeningError {
    ScreeningError::Invalid(message)
}

fn check_identifier(label: &str, value: &str) -> Result<(), ScreeningError> {
    if value.trim().is_empty() {
        Err(invalid(format!("{label} must be non-empty")))
    } else {
        Ok(())
    }
}

/// One declared input perturbation. `low`/`high` are absolute values of
/// the applied quantity: multiplicative scales are dimensionless
/// (nominal 1.0); shifts are in millimetres (nominal 0.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningParameter {
    /// Human-readable name carried into the report (must be unique).
    pub name: String,
    pub target: ScreeningTarget,
    pub nominal: f64,
    pub low: f64,
    pub high: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreeningTarget {
    /// Multiplicative scale on every `sigma_total_per_cm` entry.
    MaterialSigmaTotalScale { material_id: String },
    /// Multiplicative scale on the whole `scatter_matrix_per_cm`.
    MaterialScatterScale { material_id: String },
    /// Multiplicative scale on one `dose_response_gy_cm2` vector.
    MaterialResponseScale {
        material_id: String,
        component: String,
    },
    /// Shift of a `UniformDisk` source's `center_uv_cm[coordinate]` —
    /// `coordinate` is `u` (0) or `v` (1). Millimetres.
    SourceCenterShiftMm { coordinate: u32 },
    /// Multiplicative scale on a `UniformDisk` source's `radius_cm`.
    SourceRadiusScale,
    /// Shift of `geometry.origin_mm[axis]` — moves the domain relative
    /// to the fixed world-frame beam. `axis` is 0, 1, or 2 (x, y, z).
    /// Millimetres.
    GeometryOriginShiftMm { axis: u32 },
    /// Multiplicative scale on one boron-microdistribution compartment
    /// fraction (`nucleus` | `cytoplasm` | `membrane`) of the material
    /// carrying that declared model — the other fractions renormalize
    /// to keep the sum at one. The perturbed compound factor flows into
    /// the boron dose response, so Morris/Sobol ranks compartment-
    /// fraction uncertainty on the effective boron dose.
    MicrodistributionUptakeScale {
        material_id: String,
        compartment: String,
    },
}

/// Declared screening design over a transport case + multigroup data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitivitySpec {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding of the transport case screened.
    pub case: ContentReference,
    /// Content binding of the multigroup data screened.
    pub multigroup_data: ContentReference,
    /// Dose component whose integrated response is the screening output.
    pub response_component: String,
    /// `"morris"` or `"sobol"`.
    pub method: String,
    /// Morris trajectories (≥1) / Sobol base sample count N (≥8).
    pub samples: u32,
    /// Morris grid levels (even, ≥4); ignored for Sobol.
    #[serde(default = "default_levels")]
    pub levels: u32,
    /// Reproducibility seed for the sampling design.
    pub seed: u64,
    pub parameters: Vec<ScreeningParameter>,
    pub provenance_note: String,
    pub qualification: String,
}

fn default_levels() -> u32 {
    4
}

impl SensitivitySpec {
    pub fn validate(&self) -> Result<(), ScreeningError> {
        if !openbnct_core::schema_matches(&self.schema_version, SENSITIVITY_SPEC_SCHEMA) {
            return Err(invalid(format!("schema_version {:?}", self.schema_version)));
        }
        check_identifier("sensitivity_spec.id", &self.id)?;
        check_identifier(
            "sensitivity_spec.multigroup_data.id",
            &self.multigroup_data.id,
        )?;
        if self.parameters.is_empty() {
            return Err(invalid("parameters must not be empty".into()));
        }
        let mut seen = std::collections::HashSet::new();
        for (i, p) in self.parameters.iter().enumerate() {
            if p.name.trim().is_empty() {
                return Err(invalid(format!("parameters[{i}].name must be non-empty")));
            }
            if !seen.insert(p.name.clone()) {
                return Err(invalid(format!("duplicate parameter name {:?}", p.name)));
            }
            if !(p.low.is_finite() && p.high.is_finite() && p.nominal.is_finite()) {
                return Err(invalid(format!(
                    "parameter {:?} has non-finite bounds",
                    p.name
                )));
            }
            if p.low >= p.high {
                return Err(invalid(format!(
                    "parameter {:?} requires low < high",
                    p.name
                )));
            }
            if p.nominal < p.low || p.nominal > p.high {
                return Err(invalid(format!(
                    "parameter {:?} nominal outside [low, high]",
                    p.name
                )));
            }
            match &p.target {
                ScreeningTarget::SourceCenterShiftMm { coordinate } if *coordinate > 1 => {
                    return Err(invalid(format!(
                        "parameter {:?} coordinate must be 0 (u) or 1 (v)",
                        p.name
                    )));
                }
                ScreeningTarget::GeometryOriginShiftMm { axis } if *axis > 2 => {
                    return Err(invalid(format!(
                        "parameter {:?} axis must be 0, 1, or 2",
                        p.name
                    )));
                }
                ScreeningTarget::MicrodistributionUptakeScale { compartment, .. }
                    if !matches!(compartment.as_str(), "nucleus" | "cytoplasm" | "membrane") =>
                {
                    return Err(invalid(format!(
                        "parameter {:?} compartment must be nucleus, cytoplasm, or membrane",
                        p.name
                    )));
                }
                _ => {}
            }
        }
        match self.method.as_str() {
            "morris" => {
                if self.samples == 0 {
                    return Err(invalid("morris requires samples ≥ 1".into()));
                }
                if self.levels < 4 || !self.levels.is_multiple_of(2) {
                    return Err(invalid("morris levels must be even and ≥ 4".into()));
                }
            }
            "sobol" => {
                if self.samples < 8 {
                    return Err(invalid("sobol requires samples ≥ 8".into()));
                }
            }
            other => {
                return Err(invalid(format!(
                    "unknown method {other:?} — supported: morris, sobol"
                )));
            }
        }
        Ok(())
    }
}

/// `f64` ↔ JSON round-trip where NaN maps to `null` — method-specific
/// statistics are NaN when the other method produced the report.
mod f64_nan {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_nan() {
            s.serialize_none()
        } else {
            s.serialize_some(v)
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }
}

/// Per-parameter screening statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreeningEntry {
    pub name: String,
    /// Morris: mean signed elementary effect; Sobol: null.
    #[serde(with = "f64_nan")]
    pub mu: f64,
    /// Morris: mean |elementary effect| (the ranking statistic);
    /// Sobol: null.
    #[serde(with = "f64_nan")]
    pub mu_star: f64,
    /// Morris: elementary-effect standard deviation — nonzero flags
    /// nonlinearity/interactions; Sobol: null.
    #[serde(with = "f64_nan")]
    pub sigma: f64,
    /// Sobol: first-order index S_i; Morris: null.
    #[serde(with = "f64_nan")]
    pub first_order: f64,
    /// Sobol: total-order index ST_i; Morris: null.
    #[serde(with = "f64_nan")]
    pub total_order: f64,
    /// Rank by the method's primary statistic (μ* or ST), 1 = largest.
    pub rank: u32,
}

/// `openbnct.sensitivity-screening/0.1.0` report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensitivityScreening {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub response_component: String,
    pub method: String,
    /// Total deterministic solves consumed.
    pub evaluations: u32,
    pub seed: u64,
    /// Nominal integrated response at the declared nominals.
    pub nominal_response: f64,
    pub entries: Vec<ScreeningEntry>,
    pub spec: ContentReference,
    pub multigroup_data: ContentReference,
    pub case: ContentReference,
    pub method_note: String,
    pub qualification: String,
}

impl SensitivityScreening {
    pub fn validate(&self) -> Result<(), ScreeningError> {
        if !openbnct_core::schema_matches(&self.schema_version, SENSITIVITY_SCREENING_SCHEMA) {
            return Err(invalid(format!("schema_version {:?}", self.schema_version)));
        }
        check_identifier("sensitivity_screening.id", &self.id)?;
        if self.entries.is_empty() {
            return Err(invalid("entries must not be empty".into()));
        }
        Ok(())
    }
}

/// Deterministic PRNG (xorshift64*) — small, reproducible, and private
/// to the screening design.
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform double in [0, 1).
    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Uniform usize in [0, n).
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// Fisher–Yates shuffle.
    fn shuffle(&mut self, items: &mut [usize]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

/// Applies one parameter value to cloned case/data at a unit-hypercube
/// coordinate (mapped to `[low, high]`).
fn apply_parameter(
    case: &mut TransportCase,
    data: &mut MultigroupData,
    assignment: &mut Option<MaterialAssignment>,
    parameter: &ScreeningParameter,
    x: f64,
) -> Result<(), ScreeningError> {
    let theta = parameter.low + x * (parameter.high - parameter.low);
    let material_index = |material_id: &str| -> Result<usize, ScreeningError> {
        data.materials
            .iter()
            .position(|m| m.material_id == material_id)
            .ok_or_else(|| ScreeningError::UnknownTarget(material_id.to_string()))
    };
    match &parameter.target {
        ScreeningTarget::MaterialSigmaTotalScale { material_id } => {
            let m = material_index(material_id)?;
            for v in &mut data.materials[m].sigma_total_per_cm {
                *v *= theta;
            }
        }
        ScreeningTarget::MaterialScatterScale { material_id } => {
            let m = material_index(material_id)?;
            for v in &mut data.materials[m].scatter_matrix_per_cm {
                *v *= theta;
            }
        }
        ScreeningTarget::MaterialResponseScale {
            material_id,
            component,
        } => {
            let m = material_index(material_id)?;
            let response = data.materials[m]
                .dose_response_gy_cm2
                .get_mut(component)
                .ok_or_else(|| {
                    ScreeningError::UnknownTarget(format!(
                        "{material_id}.dose_response[{component}]"
                    ))
                })?;
            for v in response.iter_mut() {
                *v *= theta;
            }
        }
        ScreeningTarget::SourceCenterShiftMm { coordinate } => {
            let SourceSpatialDistribution::UniformDisk { center_uv_cm, .. } =
                &mut case.source.space
            else {
                return Err(ScreeningError::UnknownTarget(
                    "source_center_shift_mm requires a UniformDisk source".into(),
                ));
            };
            center_uv_cm[*coordinate as usize] += theta / 10.0; // mm → cm
        }
        ScreeningTarget::SourceRadiusScale => {
            let SourceSpatialDistribution::UniformDisk { radius_cm, .. } = &mut case.source.space
            else {
                return Err(ScreeningError::UnknownTarget(
                    "source_radius_scale requires a UniformDisk source".into(),
                ));
            };
            *radius_cm *= theta;
        }
        ScreeningTarget::GeometryOriginShiftMm { axis } => {
            case.geometry.origin_mm[*axis as usize] += theta;
        }
        ScreeningTarget::MicrodistributionUptakeScale {
            material_id,
            compartment,
        } => {
            // The declared microdistribution lives on the material
            // definition — the case base material or an assignment
            // region/base material.
            let definition: &mut crate::MaterialDefinition = if case.material.id == *material_id {
                &mut case.material
            } else if let Some(a) = assignment {
                if a.base_material.id == *material_id {
                    &mut a.base_material
                } else {
                    a.regions
                        .iter_mut()
                        .find(|r| r.material.id == *material_id)
                        .map(|r| &mut r.material)
                        .ok_or_else(|| {
                            ScreeningError::UnknownTarget(format!(
                                "microdistribution material {material_id:?}"
                            ))
                        })?
                }
            } else {
                return Err(ScreeningError::UnknownTarget(format!(
                    "microdistribution material {material_id:?}"
                )));
            };
            let micro = definition.boron_microdistribution.as_mut().ok_or_else(|| {
                ScreeningError::UnknownTarget(format!(
                    "material {material_id:?} declares no boron_microdistribution"
                ))
            })?;
            let (target, others): (&mut f64, [&mut f64; 2]) = match compartment.as_str() {
                "nucleus" => (
                    &mut micro.nucleus_fraction,
                    [&mut micro.cytoplasm_fraction, &mut micro.membrane_fraction],
                ),
                "cytoplasm" => (
                    &mut micro.cytoplasm_fraction,
                    [&mut micro.nucleus_fraction, &mut micro.membrane_fraction],
                ),
                _ => (
                    &mut micro.membrane_fraction,
                    [&mut micro.nucleus_fraction, &mut micro.cytoplasm_fraction],
                ),
            };
            *target *= theta;
            // Renormalize: the other fractions scale to preserve the
            // unit sum (proportional-share convention).
            let rest: f64 = others.iter().map(|o| **o).sum();
            let keep = (1.0 - *target).max(0.0);
            if rest > 0.0 {
                for o in others {
                    *o *= keep / rest;
                }
            }
        }
    }
    Ok(())
}

/// The evaluator: perturb → solve → integrate the component response.
#[allow(clippy::too_many_arguments)]
fn evaluate(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    cell_volume_cm3: f64,
    groups: usize,
    component: &str,
    parameters: &[ScreeningParameter],
    x: &[f64],
    data_ref: &ContentReference,
    case_ref: &ContentReference,
) -> Result<f64, ScreeningError> {
    let mut case_p = case.clone();
    let mut data_p = data.clone();
    let mut assignment_p = options.assignment.clone();
    for (parameter, &xi) in parameters.iter().zip(x.iter()) {
        apply_parameter(&mut case_p, &mut data_p, &mut assignment_p, parameter, xi)?;
    }
    // The composition map re-synthesizes fraction blends from the
    // perturbed tables — a precomputed map would carry unperturbed
    // blend rows.
    let (data_eff, case_material) =
        material_composition_map(&case_p, &data_p, assignment_p.as_ref())
            .map_err(|e| ScreeningError::Invalid(format!("compositions: {e}")))?;
    let flux: MultigroupFlux = solve_multigroup(
        &case_p,
        &data_p,
        options,
        data_ref.clone(),
        case_ref.clone(),
    )?;
    // Boron microdistribution: material_id → definition, from the
    // perturbed case + assignment. The boron response scales by the
    // declared compound factor per cell (volume-fraction blended).
    let boron_factor = if component == "component:boron" {
        let mut defs: std::collections::BTreeMap<&str, &crate::MaterialDefinition> =
            std::collections::BTreeMap::new();
        defs.insert(case_p.material.id.as_str(), &case_p.material);
        if let Some(a) = &assignment_p {
            defs.insert(a.base_material.id.as_str(), &a.base_material);
            for r in &a.regions {
                defs.insert(r.material.id.as_str(), &r.material);
            }
        }
        let comps = crate::cell_compositions(&case_p, &data_p, assignment_p.as_ref())
            .map_err(|e| ScreeningError::Invalid(format!("compositions: {e}")))?;
        Some(
            comps
                .iter()
                .map(|sig| {
                    sig.iter()
                        .map(|&(mi, f)| {
                            f * defs
                                .get(data_p.materials[mi].material_id.as_str())
                                .and_then(|d| d.boron_microdistribution.as_ref())
                                .map_or(1.0, |m| m.compound_factor())
                        })
                        .sum::<f64>()
                })
                .collect::<Vec<f64>>(),
        )
    } else {
        None
    };
    // Responses index the effective data — fraction cells carry
    // synthesized blend rows beyond `data_p.materials.len()`.
    let response_at = |m: usize, g: usize| -> f64 {
        data_eff.materials[m]
            .dose_response_gy_cm2
            .get(component)
            .map_or(0.0, |r| r[g])
    };
    let mut r = 0.0;
    for (cell, &m) in case_material.iter().enumerate() {
        let mut cell_r = 0.0;
        for g in 0..groups {
            cell_r += cell_volume_cm3 * response_at(m, g) * flux.flux[cell][g];
        }
        r += boron_factor.as_ref().map_or(cell_r, |f| cell_r * f[cell]);
    }
    Ok(r)
}

/// Runs the screening design and returns the report. `assignment` and
/// solver knobs come through `options`; the spec's `case`/`data` refs
/// must name the artifacts actually screened.
#[allow(clippy::too_many_arguments)]
pub fn run_screening(
    case: &TransportCase,
    data: &MultigroupData,
    options: &SnOptions,
    spec: &SensitivitySpec,
    report_id: &str,
    spec_ref: ContentReference,
    data_ref: ContentReference,
    case_ref: ContentReference,
) -> Result<SensitivityScreening, ScreeningError> {
    case.validate()?;
    data.validate()?;
    spec.validate()?;
    if spec.multigroup_data.id != data.id {
        return Err(invalid(format!(
            "spec {} describes data {:?}, got {:?}",
            spec.id, spec.multigroup_data.id, data.id
        )));
    }
    if spec.case.id != case.case_id {
        return Err(invalid(format!(
            "spec {} describes case {:?}, got {:?}",
            spec.id, spec.case.id, case.case_id
        )));
    }
    let _n_cells = case.geometry.voxel_count()?;
    let groups = data.group_count();
    let (data_eff, case_material) =
        material_composition_map(case, data, options.assignment.as_ref())?;
    let data = &data_eff;
    let cell_volume_cm3 = case.geometry.spacing_mm.iter().product::<f64>() / 1000.0;
    if !case_material.iter().any(|&m| {
        data.materials[m]
            .dose_response_gy_cm2
            .contains_key(&spec.response_component)
    }) {
        return Err(ScreeningError::UnknownTarget(format!(
            "dose_response component {:?}",
            spec.response_component
        )));
    }

    let k = spec.parameters.len();
    let mut rng = XorShift64(spec.seed.max(1));
    // Nominal coordinates: the unit-hypercube point mapping each
    // parameter to its declared nominal.
    let nominal_x: Vec<f64> = spec
        .parameters
        .iter()
        .map(|p| (p.nominal - p.low) / (p.high - p.low))
        .collect();
    let nominal_response = evaluate(
        case,
        data,
        options,
        cell_volume_cm3,
        groups,
        &spec.response_component,
        &spec.parameters,
        &nominal_x,
        &data_ref,
        &case_ref,
    )?;
    let mut evaluations = 1u32;

    let mut entries: Vec<ScreeningEntry> = Vec::with_capacity(k);
    match spec.method.as_str() {
        "morris" => {
            let p = spec.levels as usize;
            // Standard Morris step in unit-cube coordinates.
            let delta = p as f64 / (2.0 * (p - 1) as f64);
            let mut ee: Vec<Vec<f64>> = vec![Vec::with_capacity(spec.samples as usize); k];
            for _ in 0..spec.samples {
                // Random base point on the p-level grid, then step each
                // coordinate once in a random order, choosing the step
                // sign so the point stays in bounds.
                let mut x: Vec<f64> = (0..k)
                    .map(|_| rng.below(p) as f64 / (p - 1) as f64)
                    .collect();
                let mut order: Vec<usize> = (0..k).collect();
                rng.shuffle(&mut order);
                let mut f = evaluate(
                    case,
                    data,
                    options,
                    cell_volume_cm3,
                    groups,
                    &spec.response_component,
                    &spec.parameters,
                    &x,
                    &data_ref,
                    &case_ref,
                )?;
                evaluations += 1;
                for &i in &order {
                    let step = if x[i] + delta <= 1.0 { delta } else { -delta };
                    x[i] += step;
                    let f_new = evaluate(
                        case,
                        data,
                        options,
                        cell_volume_cm3,
                        groups,
                        &spec.response_component,
                        &spec.parameters,
                        &x,
                        &data_ref,
                        &case_ref,
                    )?;
                    evaluations += 1;
                    ee[i].push(
                        (f_new - f) / (step * (spec.parameters[i].high - spec.parameters[i].low)),
                    );
                    f = f_new;
                }
            }
            for (i, parameter) in spec.parameters.iter().enumerate() {
                let effects = &ee[i];
                let n = effects.len() as f64;
                let mu = effects.iter().sum::<f64>() / n;
                let mu_star = effects.iter().map(|e| e.abs()).sum::<f64>() / n;
                let sigma = (effects.iter().map(|e| (e - mu).powi(2)).sum::<f64>()
                    / (n - 1.0).max(1.0))
                .sqrt();
                entries.push(ScreeningEntry {
                    name: parameter.name.clone(),
                    mu,
                    mu_star,
                    sigma,
                    first_order: f64::NAN,
                    total_order: f64::NAN,
                    rank: 0,
                });
            }
            // Rank by μ*, descending.
            let mut order: Vec<usize> = (0..k).collect();
            order.sort_by(|&a, &b| {
                entries[b]
                    .mu_star
                    .partial_cmp(&entries[a].mu_star)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for (rank, &i) in order.iter().enumerate() {
                entries[i].rank = rank as u32 + 1;
            }
        }
        "sobol" => {
            if k == 1 {
                // A single declared parameter owns all response
                // variance: S1 = ST = 1 by definition (the Saltelli
                // design degenerates at k=1 — every AB_i equals B).
                entries.push(ScreeningEntry {
                    name: spec.parameters[0].name.clone(),
                    mu: f64::NAN,
                    mu_star: f64::NAN,
                    sigma: f64::NAN,
                    first_order: 1.0,
                    total_order: 1.0,
                    rank: 1,
                });
            } else {
                let n = spec.samples as usize;
                // Saltelli design: A, B ∈ [0,1]^{n×k} random; AB_i is A
                // with column i swapped to B's.
                let a: Vec<Vec<f64>> = (0..n)
                    .map(|_| (0..k).map(|_| rng.uniform()).collect())
                    .collect();
                let b: Vec<Vec<f64>> = (0..n)
                    .map(|_| (0..k).map(|_| rng.uniform()).collect())
                    .collect();
                let mut fa = Vec::with_capacity(n);
                let mut fb = Vec::with_capacity(n);
                for row in &a {
                    fa.push(evaluate(
                        case,
                        data,
                        options,
                        cell_volume_cm3,
                        groups,
                        &spec.response_component,
                        &spec.parameters,
                        row,
                        &data_ref,
                        &case_ref,
                    )?);
                    evaluations += 1;
                }
                for row in &b {
                    fb.push(evaluate(
                        case,
                        data,
                        options,
                        cell_volume_cm3,
                        groups,
                        &spec.response_component,
                        &spec.parameters,
                        row,
                        &data_ref,
                        &case_ref,
                    )?);
                    evaluations += 1;
                }
                let mut fab: Vec<Vec<f64>> = Vec::with_capacity(k);
                for i in 0..k {
                    let mut col = Vec::with_capacity(n);
                    for j in 0..n {
                        let mut row = a[j].clone();
                        row[i] = b[j][i];
                        col.push(evaluate(
                            case,
                            data,
                            options,
                            cell_volume_cm3,
                            groups,
                            &spec.response_component,
                            &spec.parameters,
                            &row,
                            &data_ref,
                            &case_ref,
                        )?);
                        evaluations += 1;
                    }
                    fab.push(col);
                }
                // Center the output first: Sobol indices are variance
                // decompositions, so f − m̄ changes nothing but removes the
                // m² cancellation that otherwise makes the first-order
                // product estimator ~m·s/√N-noisy — pathological when the
                // response mean dwarfs its variance (the usual case here).
                let mean = (fa.iter().sum::<f64>() + fb.iter().sum::<f64>()) / (2.0 * n as f64);
                let var = (fa.iter().map(|v| (v - mean).powi(2)).sum::<f64>()
                    + fb.iter().map(|v| (v - mean).powi(2)).sum::<f64>())
                    / (2.0 * n as f64 - 1.0);
                let fa_c: Vec<f64> = fa.iter().map(|v| v - mean).collect();
                let fb_c: Vec<f64> = fb.iter().map(|v| v - mean).collect();
                let mean_fa_fb = (0..n).map(|j| fa_c[j] * fb_c[j]).sum::<f64>() / n as f64;
                for (i, parameter) in spec.parameters.iter().enumerate() {
                    // Jansen total-order (difference form, centering-
                    // invariant) and centered Saltelli-2010 first-order
                    // estimators.
                    let st_num: f64 =
                        (0..n).map(|j| (fa[j] - fab[i][j]).powi(2)).sum::<f64>() / (2.0 * n as f64);
                    let s1_num: f64 = (0..n).map(|j| fb_c[j] * (fab[i][j] - mean)).sum::<f64>()
                        / n as f64
                        - mean_fa_fb;
                    entries.push(ScreeningEntry {
                        name: parameter.name.clone(),
                        mu: f64::NAN,
                        mu_star: f64::NAN,
                        sigma: f64::NAN,
                        first_order: s1_num / var,
                        total_order: st_num / var,
                        rank: 0,
                    });
                }
                let mut order: Vec<usize> = (0..k).collect();
                order.sort_by(|&a, &b| {
                    entries[b]
                        .total_order
                        .partial_cmp(&entries[a].total_order)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                for (rank, &i) in order.iter().enumerate() {
                    entries[i].rank = rank as u32 + 1;
                }
            }
        }
        other => return Err(invalid(format!("unknown method {other:?}"))),
    }

    let report = SensitivityScreening {
        schema_version: SENSITIVITY_SCREENING_SCHEMA.into(),
        id: report_id.into(),
        case_id: case.case_id.clone(),
        response_component: spec.response_component.clone(),
        method: spec.method.clone(),
        evaluations,
        seed: spec.seed,
        nominal_response,
        entries,
        spec: spec_ref,
        multigroup_data: data_ref,
        case: case_ref,
        method_note: match spec.method.as_str() {
            "morris" => format!(
                "Morris elementary effects: {} trajectories × ({} params + 1) \
                 + nominal solve = {evaluations} deterministic solves; \
                 p={} levels; effects normalized per unit input range",
                spec.samples, k, spec.levels
            ),
            _ => format!(
                "Saltelli-Sobol: N={} base samples → 2N + N·{} = {} perturbed \
                 solves + nominal; Jansen total-order and Saltelli-2010 \
                 first-order estimators",
                spec.samples,
                k,
                evaluations - 1
            ),
        },
        qualification: "research-verification sensitivity screening — declared \
                        input ranges, deterministic-evaluator estimates; not a \
                        clinical tolerance analysis"
            .into(),
    };
    report.validate()?;
    Ok(report)
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

    fn mg() -> MultigroupData {
        let mut mg = slab_data(&[0.5], vec![0.0]);
        mg.materials[0]
            .dose_response_gy_cm2
            .insert("boron".into(), vec![1e-4]);
        mg
    }

    fn spec(params: Vec<ScreeningParameter>, method: &str, samples: u32) -> SensitivitySpec {
        SensitivitySpec {
            schema_version: SENSITIVITY_SPEC_SCHEMA.into(),
            id: "screen.test".into(),
            case: cref("mg-slab"),
            multigroup_data: cref("mg-data"),
            response_component: "boron".into(),
            method: method.into(),
            samples,
            levels: 4,
            seed: 42,
            parameters: params,
            provenance_note: "test".into(),
            qualification: "test_only".into(),
        }
    }

    fn param(name: &str, target: ScreeningTarget, low: f64, high: f64) -> ScreeningParameter {
        ScreeningParameter {
            name: name.into(),
            target,
            nominal: (low + high) / 2.0,
            low,
            high,
        }
    }

    /// A response-scale parameter is exactly linear in R: its
    /// elementary effect is constant R_nominal/1 everywhere, so
    /// μ* = R_nominal and σ ≈ 0 — the analytic anchor for Morris.
    #[test]
    fn morris_linear_parameter_exact() {
        let case = slab_case();
        let mg = mg();
        let s = spec(
            vec![param(
                "resp_scale",
                ScreeningTarget::MaterialResponseScale {
                    material_id: "absorber".into(),
                    component: "boron".into(),
                },
                0.8,
                1.2,
            )],
            "morris",
            4,
        );
        let report = run_screening(
            &case,
            &mg,
            &options(),
            &s,
            "r1",
            cref("s"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let e = &report.entries[0];
        // R(θ) = θ·R₀ → EE = ΔR/Δθ = R₀ exactly.
        assert!((e.mu_star - report.nominal_response).abs() < 1e-9 * report.nominal_response);
        assert!(e.sigma < 1e-9 * e.mu_star.max(1.0));
        assert_eq!(report.evaluations, 4 * 2 + 1);
    }

    /// A parameter with zero physical leverage (scatter on a
    /// zero-scatter material) must measure zero and rank last.
    #[test]
    fn morris_dead_parameter_ranks_last() {
        let case = slab_case();
        let mg = mg();
        let s = spec(
            vec![
                param(
                    "sigma_t",
                    ScreeningTarget::MaterialSigmaTotalScale {
                        material_id: "absorber".into(),
                    },
                    0.9,
                    1.1,
                ),
                param(
                    "scatter",
                    ScreeningTarget::MaterialScatterScale {
                        material_id: "absorber".into(),
                    },
                    0.5,
                    1.5,
                ),
            ],
            "morris",
            4,
        );
        let report = run_screening(
            &case,
            &mg,
            &options(),
            &s,
            "r2",
            cref("s"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let scatter = report.entries.iter().find(|e| e.name == "scatter").unwrap();
        assert_eq!(scatter.mu_star, 0.0, "zero-scatter scale must be dead");
        assert_eq!(scatter.rank, 2);
        let sigma_t = report.entries.iter().find(|e| e.name == "sigma_t").unwrap();
        assert!(sigma_t.mu_star > 0.0);
        assert_eq!(sigma_t.rank, 1);
    }

    /// Same seed replays the same design — screening output must be
    /// bit-reproducible for provenance.
    #[test]
    fn screening_is_seed_deterministic() {
        let case = slab_case();
        let mg = mg();
        let s = spec(
            vec![
                param(
                    "sigma_t",
                    ScreeningTarget::MaterialSigmaTotalScale {
                        material_id: "absorber".into(),
                    },
                    0.9,
                    1.1,
                ),
                param(
                    "disk_u",
                    ScreeningTarget::SourceCenterShiftMm { coordinate: 0 },
                    -0.5,
                    0.5,
                ),
            ],
            "morris",
            3,
        );
        let a = run_screening(
            &case,
            &mg,
            &options(),
            &s,
            "r",
            cref("s"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let b = run_screening(
            &case,
            &mg,
            &options(),
            &s,
            "r",
            cref("s"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        // NaN fields (first_order/total_order for Morris) break
        // PartialEq — compare the populated statistics field-wise.
        assert_eq!(a.evaluations, b.evaluations);
        assert_eq!(a.nominal_response, b.nominal_response);
        for (ea, eb) in a.entries.iter().zip(b.entries.iter()) {
            assert_eq!(ea.name, eb.name);
            assert_eq!(ea.mu, eb.mu);
            assert_eq!(ea.mu_star, eb.mu_star);
            assert_eq!(ea.sigma, eb.sigma);
            assert_eq!(ea.rank, eb.rank);
        }
    }

    /// One-parameter Sobol must return S1 ≈ ST ≈ 1 — all variance from
    /// the only input.
    #[test]
    fn sobol_single_parameter_dominates() {
        let case = slab_case();
        let mg = mg();
        let s = spec(
            vec![param(
                "sigma_t",
                ScreeningTarget::MaterialSigmaTotalScale {
                    material_id: "absorber".into(),
                },
                0.9,
                1.1,
            )],
            "sobol",
            64,
        );
        let report = run_screening(
            &case,
            &mg,
            &options(),
            &s,
            "r3",
            cref("s"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let e = &report.entries[0];
        assert!(
            (e.first_order - 1.0).abs() < 0.2,
            "single-parameter S1 must approach 1, got {}",
            e.first_order
        );
        assert!(
            (e.total_order - 1.0).abs() < 0.2,
            "single-parameter ST must approach 1, got {}",
            e.total_order
        );
    }

    /// A Sobol dead parameter must produce indices ≈ 0.
    #[test]
    fn sobol_dead_parameter_zero() {
        let case = slab_case();
        let mg = mg();
        let s = spec(
            vec![
                param(
                    "resp_scale",
                    ScreeningTarget::MaterialResponseScale {
                        material_id: "absorber".into(),
                        component: "boron".into(),
                    },
                    0.5,
                    1.5,
                ),
                param(
                    "scatter",
                    ScreeningTarget::MaterialScatterScale {
                        material_id: "absorber".into(),
                    },
                    0.5,
                    1.5,
                ),
            ],
            "sobol",
            64,
        );
        let report = run_screening(
            &case,
            &mg,
            &options(),
            &s,
            "r4",
            cref("s"),
            cref("d"),
            cref("c"),
        )
        .unwrap();
        let dead = report.entries.iter().find(|e| e.name == "scatter").unwrap();
        assert!(
            dead.total_order < 0.05,
            "zero-scatter ST should be ~0, got {}",
            dead.total_order
        );
        assert_eq!(dead.rank, 2);
    }

    /// Spec validation: bad method, empty parameters, inverted ranges,
    /// duplicates, bad axes.
    #[test]
    fn spec_validation_rejects_bad_designs() {
        let s = spec(vec![], "morris", 4);
        assert!(s.validate().is_err());
        let mut s = spec(
            vec![param(
                "p",
                ScreeningTarget::MaterialSigmaTotalScale {
                    material_id: "absorber".into(),
                },
                1.1,
                0.9,
            )],
            "morris",
            4,
        );
        assert!(s.validate().is_err());
        s.method = "latin-hypercube".into();
        assert!(s.validate().is_err());
        s.method = "morris".into();
        s.parameters[0].low = 0.9;
        s.parameters[0].high = 1.1;
        s.parameters[0].nominal = 1.0;
        s.parameters
            .push(param("p", ScreeningTarget::SourceRadiusScale, 0.5, 1.5));
        assert!(s.validate().is_err(), "duplicate names must reject");
    }

    /// Unknown material/component targets surface as UnknownTarget at
    /// evaluation, not a panic.
    #[test]
    fn unknown_targets_rejected() {
        let case = slab_case();
        let mg = mg();
        let s = spec(
            vec![param(
                "ghost",
                ScreeningTarget::MaterialSigmaTotalScale {
                    material_id: "not-here".into(),
                },
                0.9,
                1.1,
            )],
            "morris",
            1,
        );
        assert!(matches!(
            run_screening(
                &case,
                &mg,
                &options(),
                &s,
                "r",
                cref("s"),
                cref("d"),
                cref("c")
            ),
            Err(ScreeningError::UnknownTarget(_))
        ));
    }
}
