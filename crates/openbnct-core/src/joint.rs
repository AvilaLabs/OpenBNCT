// SPDX-License-Identifier: MIT
//
//! Joint uncertainty declaration and propagation
//! (`openbnct.joint-uncertainty-input/0.1.0`,
//! `openbnct.joint-uncertainty-report/0.1.0`).
//!
//! [`crate::systematic`] maps declared per-voxel σ contributions; this
//! module is the layer above it: *one* declared source identity that many
//! voxels, beams, regions, or time intervals share. A joint input
//! declares, per source, what kind of uncertainty it is
//! ([`UncertaintyCategory`]), what it physically perturbs
//! ([`SourceTarget`]), whether its draw is shared across those targets
//! ([`SourceSharing`]), its physical unit, its support, its distribution,
//! and the evidence the declaration rests on. Sources may additionally
//! be tied by explicit [`CorrelationGroup`]s; absent a group, distinct
//! sources are independent *by declaration* — independence is never
//! inferred from separate field or artifact ids.
//!
//! Two evaluation paths consume the same contract:
//!
//! - [`propagate_first_order`]: analytic linear folding
//!   `σ_m² = aᵀ·C·a` over per-source scalar metric sensitivities —
//!   shared draws sum *inside* the covariance form, which is what makes
//!   a common calibration error add linearly rather than in quadrature.
//! - [`run_ensemble`]: per-realization evaluation. Each realization is a
//!   complete joint draw (every source resolved before the metric runs),
//!   so nonlinear metrics such as `D95` are computed on each realized
//!   dose map rather than reconstructed from per-voxel intervals.
//!
//! Ensemble summaries record the quantile estimator, sample weights,
//! seed, and sampling diagnostics. Uncertainty categories with no
//! declared source must still carry an explicit
//! [`CategoryDisposition`]: a category that was never assessed is
//! reported as `unassessed`, not silently dropped, and no report may
//! claim the propagated interval is the complete uncertainty.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ContentReference;

/// Contract token for joint-uncertainty input documents.
pub const JOINT_UNCERTAINTY_INPUT_SCHEMA: &str = "openbnct.joint-uncertainty-input/0.1.0";
/// Contract token for joint-uncertainty reports.
pub const JOINT_UNCERTAINTY_REPORT_SCHEMA: &str = "openbnct.joint-uncertainty-report/0.1.0";
/// Qualification asserted on every joint-uncertainty report.
pub const JOINT_UNCERTAINTY_QUALIFICATION: &str = "joint_uncertainty_research_only_not_clinical";

/// Absolute pivot floor for covariance factorization: a pivot whose
/// magnitude is within `EIGENVALUE_TOLERANCE × trace/n` of zero is
/// treated as a *supported* singular direction (perfectly correlated
/// input); a pivot beyond it in the negative direction is an invalid
/// covariance and fails validation rather than being repaired.
pub const EIGENVALUE_TOLERANCE: f64 = 1.0e-10;

/// Hard cap on stored ensemble work: `realizations × metrics` retained
/// values. Larger studies must stream summaries instead.
pub const MAX_RETAINED_VALUES: usize = 4_000_000;
/// Hard cap on declared sources in one input.
pub const MAX_SOURCES: usize = 512;
/// Hard cap on realizations per ensemble.
pub const MAX_REALIZATIONS: u32 = 1_000_000;

/// Which uncertainty category a source belongs to. The categories are
/// reported independently — combining a sampling error with a model
/// discrepancy in one number is a category error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyCategory {
    /// Finite-statistics error of a Monte Carlo or measurement estimate.
    StatisticalSampling,
    /// Uncertain input parameter (concentration, output factor, PK
    /// coefficient, biological weight).
    InputParameter,
    /// Structural model choice (which compartment model, which
    /// biological model family).
    StructuralModel,
    /// Explicitly assessed transport/model discrepancy against a
    /// reference calculation or measurement.
    ModelDiscrepancy,
}

/// What a propagated source acts on. The *sharing* of the draw across
/// targets is [`SourceSharing`]; the target list is which concrete
/// fields, regions, components, or intervals the source perturbs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceTarget {
    /// Everywhere the joint input applies (e.g. a source-output
    /// calibration shared by all beams).
    Global,
    /// One named beam/field.
    Field { name: String },
    /// One named region mask.
    Region { name: String },
    /// One named dose component (`boron`, `nitrogen`, `hydrogen`,
    /// `photon`).
    Component { name: String },
    /// One named delivery/observation interval.
    Interval { name: String },
}

/// How one source's draw is shared across its scope targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceSharing {
    /// One draw applies to every target — the shared-calibration case.
    Shared,
    /// An independent draw is taken per target. This must be *declared*;
    /// it is never inferred from the target set.
    IndependentPerTarget,
}

/// Mathematical support the declared distribution must respect. A
/// distribution whose support exceeds the declared support is rejected
/// at validation — there is no silent clipping of out-of-domain draws.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Support {
    /// All reals.
    Real,
    /// Strictly positive (concentrations, rates, scale factors).
    Positive,
    /// Non-negative.
    NonNegative,
    /// Explicitly bounded interval.
    Bounded { low: f64, high: f64 },
}

/// The declared distribution of one source's draw (or of its vector of
/// coupled parameters). Scalar variants produce one value per draw;
/// the multivariate variants produce `mean.len()` values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Distribution {
    /// Degenerate draw — zero declared variance. The realization is the
    /// point estimate.
    Point { value: f64 },
    /// Normal(μ, σ). Supported only on [`Support::Real`]; restricted
    /// supports need a bounded/positive distribution instead.
    Normal { mean: f64, std_dev: f64 },
    /// `X = median·exp(sigma_log·Z)`, `Z ~ N(0,1)`. Positive support.
    /// For small `sigma_log` the relative standard deviation ≈
    /// `sigma_log`.
    LogNormal { median: f64, sigma_log: f64 },
    /// Uniform on `[low, high]`.
    Uniform { low: f64, high: f64 },
    /// Correlated real vector: row-major covariance matrix, dimension
    /// `mean.len()`. Singular (perfectly correlated) covariances are a
    /// supported path; indefinite ones are rejected.
    MultivariateNormal {
        mean: Vec<f64>,
        covariance: Vec<f64>,
    },
    /// Correlated positive vector: `X_i = exp(μ_i + (Lz)_i)` with `L`
    /// the Cholesky factor of `covariance_log`.
    MultivariateLogNormal {
        mean_log: Vec<f64>,
        covariance_log: Vec<f64>,
    },
}

/// Where a source's declared uncertainty comes from. `basis` is
/// required; "assumed" or "estimated" is admissible, silence is not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEvidence {
    /// Free-text basis: the assay method, calibration record,
    /// literature range, or assumption the distribution encodes.
    pub basis: String,
    /// Optional content-bound artifact the distribution derives from
    /// (fit report, calibration document, measurement series).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<ContentReference>,
}

/// One declared uncertainty source. `id` is the identity: every
/// consumer that references this id shares the same draw in one
/// realization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointSource {
    pub id: String,
    pub category: UncertaintyCategory,
    /// What the draw perturbs. `IndependentPerTarget` sources must
    /// name every target they draw for.
    pub scope: Vec<SourceTarget>,
    pub sharing: SourceSharing,
    /// Physical unit of the drawn quantity — `"1"` for a dimensionless
    /// multiplicative scale. Adapters refuse mismatched units.
    pub unit: String,
    pub support: Support,
    pub distribution: Distribution,
    pub evidence: SourceEvidence,
}

/// An explicit cross-source correlation set: the listed scalar sources
/// share the given row-major correlation matrix (unit diagonal,
/// symmetric). Members must be scalar, `Shared`, and must not overlap
/// another group. Grouped sources propagate through `aᵀCa`; they may
/// only be split for attribution as one combined group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrelationGroup {
    /// Member source ids, in matrix order.
    pub sources: Vec<String>,
    /// Row-major `n×n` correlation matrix.
    pub correlation: Vec<f64>,
}

/// How one uncertainty category was handled. Every category without a
/// declared source requires an explicit disposition entry — an
/// unassessed category stays visible in every report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryDisposition {
    pub category: UncertaintyCategory,
    pub status: CategoryStatus,
    /// Required for `approximated`, `unassessed`, and `excluded`;
    /// records what was done or why nothing was.
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CategoryStatus {
    /// At least one declared source of this category was propagated.
    Propagated,
    /// The category was included through a stated approximation.
    Approximated,
    /// Assessed but no quantitative statement is available.
    Unassessed,
    /// Deliberately excluded, with the reason in `note`.
    Excluded,
}

/// Versioned joint-uncertainty input document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointUncertaintyInput {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Artifacts this input describes (dose bundles, PK models, plan
    /// results), content-bound. Consumers verify the artifact they
    /// actually received is listed.
    pub subjects: Vec<ContentReference>,
    pub sources: Vec<JointSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub correlations: Vec<CorrelationGroup>,
    /// Explicit disposition for every category no source covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub category_disposition: Vec<CategoryDisposition>,
    pub qualification: String,
    pub provenance_id: String,
}

/// Errors from joint-input validation and ensemble evaluation.
#[derive(Debug, Error)]
pub enum JointError {
    #[error("unsupported joint-uncertainty schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid joint uncertainty input: {0}")]
    Invalid(String),
    #[error("invalid content reference: {0}")]
    InvalidContentReference(#[from] crate::ContentReferenceError),
}

pub(crate) fn invalid(reason: impl Into<String>) -> JointError {
    JointError::Invalid(reason.into())
}

/// True when `distribution`'s mathematical support is compatible with
/// `support`. Incompatible combinations are a validation error — never
/// a clipped draw.
fn support_compatible(support: &Support, distribution: &Distribution) -> bool {
    // (lower bound, lower bound inclusive, upper bound) of the draw's
    // mathematical support.
    let (low, low_inclusive, high) = match distribution {
        Distribution::Point { value } => (*value, true, *value),
        Distribution::Normal { .. } | Distribution::MultivariateNormal { .. } => {
            (f64::NEG_INFINITY, false, f64::INFINITY)
        }
        // Log-normal draws are strictly positive: 0 is an open bound.
        Distribution::LogNormal { .. } | Distribution::MultivariateLogNormal { .. } => {
            (0.0, false, f64::INFINITY)
        }
        Distribution::Uniform { low, high } => (*low, true, *high),
    };
    match support {
        Support::Real => true,
        // MVN/Normal reach negatives; positive support rejects them.
        Support::Positive => low > 0.0 || (low == 0.0 && !low_inclusive),
        Support::NonNegative => low >= 0.0,
        Support::Bounded { low: lo, high: hi } => low >= *lo && high <= *hi,
    }
}

/// Dimension of one draw.
pub fn distribution_dimension(distribution: &Distribution) -> usize {
    match distribution {
        Distribution::MultivariateNormal { mean, .. } => mean.len(),
        Distribution::MultivariateLogNormal { mean_log, .. } => mean_log.len(),
        _ => 1,
    }
}

/// Tolerant Cholesky factorization of a symmetric `n×n` matrix stored
/// row-major. Returns the lower-triangular factor `L` (row-major).
///
/// Semi-definite inputs are supported: a pivot inside
/// `±tolerance × mean(diagonal)` of zero is set to exactly zero, which
/// yields draws confined to the range space — the declared handling
/// for perfectly correlated inputs. A pivot negative beyond the
/// tolerance means the matrix is indefinite and is an error.
pub fn cholesky_psd(matrix: &[f64], n: usize) -> Result<Vec<f64>, JointError> {
    if matrix.len() != n * n {
        return Err(invalid(format!(
            "covariance has {} entries, expected {n}×{n}",
            matrix.len()
        )));
    }
    let scale = if n == 0 {
        1.0
    } else {
        let mean_diag = (0..n).map(|i| matrix[i * n + i]).sum::<f64>() / n as f64;
        mean_diag.abs().max(1.0)
    };
    let tol = EIGENVALUE_TOLERANCE * scale;
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = matrix[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if s < -tol {
                    return Err(invalid(format!(
                        "covariance is not positive-semidefinite (pivot {i} = {s:.3e}, tolerance {tol:.3e})"
                    )));
                }
                // Within ±tol of zero the pivot is treated as rank-
                // deficient, not as a tiny independent component — exact
                // (anti-)correlated draws must draw identically.
                l[i * n + i] = if s > tol { s.sqrt() } else { 0.0 };
            } else {
                l[i * n + j] = if l[j * n + j] > 0.0 {
                    s / l[j * n + j]
                } else {
                    0.0
                };
            }
        }
    }
    Ok(l)
}

/// Row-major `n×n` symmetry check within `tolerance`.
fn check_symmetric(matrix: &[f64], n: usize, tolerance: f64, what: &str) -> Result<(), JointError> {
    for i in 0..n {
        for j in (i + 1)..n {
            if (matrix[i * n + j] - matrix[j * n + i]).abs() > tolerance {
                return Err(invalid(format!(
                    "{what} matrix is not symmetric at ({i},{j})"
                )));
            }
        }
    }
    Ok(())
}

/// The marginal standard deviation of a distribution's draw, in draw
/// units. Multivariate variants return per-coordinate σ of the drawn
/// vector (lognormal σ is exact on the drawn value).
pub fn marginal_std_dev(distribution: &Distribution) -> Vec<f64> {
    match distribution {
        Distribution::Point { .. } => vec![0.0],
        Distribution::Normal { std_dev, .. } => vec![*std_dev],
        Distribution::LogNormal { median, sigma_log } => {
            // Var = m²·e^{σ²}·(e^{σ²}−1)
            let s2 = sigma_log * sigma_log;
            vec![median * (s2 / 2.0).exp() * (s2.exp() - 1.0).sqrt()]
        }
        Distribution::Uniform { low, high } => {
            vec![(high - low) / (12.0_f64).sqrt()]
        }
        Distribution::MultivariateNormal { covariance, .. } => {
            let n = covariance.len().isqrt();
            if n * n != covariance.len() {
                return Vec::new();
            }
            (0..n)
                .map(|i| covariance[i * n + i].max(0.0).sqrt())
                .collect()
        }
        Distribution::MultivariateLogNormal {
            mean_log,
            covariance_log,
        } => {
            let n = mean_log.len();
            if covariance_log.len() != n * n {
                return Vec::new();
            }
            (0..n)
                .map(|i| {
                    let s2 = covariance_log[i * n + i].max(0.0);
                    (mean_log[i] + s2 / 2.0).exp() * (s2.exp() - 1.0).max(0.0).sqrt()
                })
                .collect()
        }
    }
}

impl JointSource {
    /// Draw dimension of this source's distribution.
    pub fn dimension(&self) -> usize {
        distribution_dimension(&self.distribution)
    }

    /// Marginal σ of the draw in draw units.
    pub fn std_dev(&self) -> Vec<f64> {
        marginal_std_dev(&self.distribution)
    }
}

impl JointUncertaintyInput {
    /// Structural + scientific validation; every consumer calls this.
    pub fn validate(&self) -> Result<(), JointError> {
        if !crate::schema_matches(&self.schema_version, JOINT_UNCERTAINTY_INPUT_SCHEMA) {
            return Err(JointError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty()
            || self.qualification.trim().is_empty()
            || self.provenance_id.trim().is_empty()
        {
            return Err(invalid("id, qualification, and provenance_id are required"));
        }
        for subject in &self.subjects {
            subject.validate()?;
        }
        if self.sources.is_empty() {
            return Err(invalid("at least one source is required"));
        }
        if self.sources.len() > MAX_SOURCES {
            return Err(invalid(format!(
                "{} sources exceed the {MAX_SOURCES} budget",
                self.sources.len()
            )));
        }
        let mut ids = BTreeSet::new();
        for source in &self.sources {
            if source.id.trim().is_empty() {
                return Err(invalid("source id is empty"));
            }
            if !ids.insert(source.id.as_str()) {
                return Err(invalid(format!("duplicate source id {:?}", source.id)));
            }
            if source.unit.trim().is_empty() {
                return Err(invalid(format!("source {:?}: unit is empty", source.id)));
            }
            if source.scope.is_empty() {
                return Err(invalid(format!(
                    "source {:?}: declares no scope target",
                    source.id
                )));
            }
            if source.evidence.basis.trim().is_empty() {
                return Err(invalid(format!(
                    "source {:?}: evidence basis is required",
                    source.id
                )));
            }
            if let Some(reference) = &source.evidence.reference {
                reference.validate()?;
            }
            for target in &source.scope {
                let name = match target {
                    SourceTarget::Global => continue,
                    SourceTarget::Field { name }
                    | SourceTarget::Region { name }
                    | SourceTarget::Component { name }
                    | SourceTarget::Interval { name } => name,
                };
                if name.trim().is_empty() {
                    return Err(invalid(format!(
                        "source {:?}: scope target name is empty",
                        source.id
                    )));
                }
            }
            if source.sharing == SourceSharing::IndependentPerTarget
                && source
                    .scope
                    .iter()
                    .any(|t| matches!(t, SourceTarget::Global))
            {
                return Err(invalid(format!(
                    "source {:?}: independent_per_target over a global scope is incoherent",
                    source.id
                )));
            }
            validate_distribution(&source.id, &source.distribution)?;
            if let Support::Bounded { low, high } = &source.support
                && (!low.is_finite() || !high.is_finite() || low >= high)
            {
                return Err(invalid(format!(
                    "source {:?}: bounded support requires finite low < high",
                    source.id
                )));
            }
            if !support_compatible(&source.support, &source.distribution) {
                return Err(invalid(format!(
                    "source {:?}: distribution support is incompatible with declared \
                     support — choose a distribution whose support fits (no clipping is applied)",
                    source.id
                )));
            }
        }

        // Correlation groups: scalar shared sources only, matrix shape,
        // unit diagonal, symmetry, semidefinite, no duplicate membership.
        let mut grouped = BTreeSet::new();
        for group in &self.correlations {
            let n = group.sources.len();
            if n < 2 {
                return Err(invalid("correlation groups need at least two sources"));
            }
            if group.correlation.len() != n * n {
                return Err(invalid(format!(
                    "correlation group {:?}: matrix has {} entries, expected {n}×{n}",
                    group.sources,
                    group.correlation.len()
                )));
            }
            for member in &group.sources {
                let source = self
                    .sources
                    .iter()
                    .find(|s| &s.id == member)
                    .ok_or_else(|| {
                        invalid(format!(
                            "correlation group references unknown source {member:?}"
                        ))
                    })?;
                if source.dimension() != 1 || source.sharing != SourceSharing::Shared {
                    return Err(invalid(format!(
                        "correlation group member {member:?} must be a scalar shared source"
                    )));
                }
                if !grouped.insert(member.as_str()) {
                    return Err(invalid(format!(
                        "source {member:?} appears in two correlation groups"
                    )));
                }
            }
            for i in 0..n {
                if (group.correlation[i * n + i] - 1.0).abs() > 1e-9 {
                    return Err(invalid(format!(
                        "correlation group {:?}: diagonal must be 1",
                        group.sources
                    )));
                }
            }
            check_symmetric(&group.correlation, n, 1e-9, "correlation")?;
            // A correlation matrix is PSD — reuse the covariance check.
            let scaled: Vec<f64> = group.correlation.clone();
            cholesky_psd(&scaled, n)
                .map_err(|e| invalid(format!("correlation group {:?}: {e}", group.sources)))?;
        }

        // Every category must be either sourced or explicitly disposed.
        let mut sourced: BTreeSet<UncertaintyCategory> = BTreeSet::new();
        for source in &self.sources {
            sourced.insert(source.category);
        }
        let mut disposed: BTreeSet<UncertaintyCategory> = BTreeSet::new();
        for entry in &self.category_disposition {
            if entry.note.trim().is_empty() {
                return Err(invalid(format!(
                    "category {:?} disposition requires a note",
                    entry.category
                )));
            }
            if !disposed.insert(entry.category) {
                return Err(invalid(format!(
                    "duplicate disposition for category {:?}",
                    entry.category
                )));
            }
        }
        for category in [
            UncertaintyCategory::StatisticalSampling,
            UncertaintyCategory::InputParameter,
            UncertaintyCategory::StructuralModel,
            UncertaintyCategory::ModelDiscrepancy,
        ] {
            if !sourced.contains(&category) && !disposed.contains(&category) {
                return Err(invalid(format!(
                    "category {category:?} has no source and no explicit disposition"
                )));
            }
        }
        Ok(())
    }
}

fn validate_distribution(id: &str, distribution: &Distribution) -> Result<(), JointError> {
    match distribution {
        Distribution::Point { value } => {
            if !value.is_finite() {
                return Err(invalid(format!(
                    "source {id:?}: point value must be finite"
                )));
            }
        }
        Distribution::Normal { mean, std_dev } => {
            if !mean.is_finite() || !std_dev.is_finite() || *std_dev < 0.0 {
                return Err(invalid(format!(
                    "source {id:?}: normal requires finite mean and non-negative std_dev"
                )));
            }
        }
        Distribution::LogNormal { median, sigma_log } => {
            if !median.is_finite() || *median <= 0.0 || !sigma_log.is_finite() || *sigma_log < 0.0 {
                return Err(invalid(format!(
                    "source {id:?}: lognormal requires finite positive median and \
                     non-negative sigma_log"
                )));
            }
        }
        Distribution::Uniform { low, high } => {
            if !low.is_finite() || !high.is_finite() || low >= high {
                return Err(invalid(format!(
                    "source {id:?}: uniform requires finite low < high"
                )));
            }
        }
        Distribution::MultivariateNormal { mean, covariance } => {
            let n = mean.len();
            if n == 0 || covariance.len() != n * n || mean.iter().any(|m| !m.is_finite()) {
                return Err(invalid(format!(
                    "source {id:?}: multivariate normal needs a non-empty finite mean \
                     and an n×n covariance"
                )));
            }
            check_symmetric(covariance, n, 1e-9, "covariance")?;
            cholesky_psd(covariance, n).map_err(|e| invalid(format!("source {id:?}: {e}")))?;
        }
        Distribution::MultivariateLogNormal {
            mean_log,
            covariance_log,
        } => {
            let n = mean_log.len();
            if n == 0 || covariance_log.len() != n * n || mean_log.iter().any(|m| !m.is_finite()) {
                return Err(invalid(format!(
                    "source {id:?}: multivariate lognormal needs a non-empty finite \
                     mean_log and an n×n covariance_log"
                )));
            }
            check_symmetric(covariance_log, n, 1e-9, "covariance_log")?;
            cholesky_psd(covariance_log, n).map_err(|e| invalid(format!("source {id:?}: {e}")))?;
        }
    }
    Ok(())
}

/// One source's draw inside a realization. `target` is `None` for
/// `Shared` sources and the scope target's name for
/// `IndependentPerTarget` draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDraw {
    pub source_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub values: Vec<f64>,
}

/// One complete joint draw: every declared source resolved before any
/// metric evaluates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Realization {
    /// Sample weight — `1.0` for Monte Carlo draws, arbitrary positive
    /// for explicit weighted realizations.
    pub weight: f64,
    pub draws: Vec<SourceDraw>,
}

impl Realization {
    /// The shared draw for `source_id` (`None` if absent or per-target).
    pub fn scalar(&self, source_id: &str) -> Option<f64> {
        self.draws
            .iter()
            .find(|d| d.source_id == source_id && d.target.is_none())
            .and_then(|d| d.values.first().copied())
    }

    /// The shared vector draw for `source_id`.
    pub fn vector(&self, source_id: &str) -> Option<&[f64]> {
        self.draws
            .iter()
            .find(|d| d.source_id == source_id && d.target.is_none())
            .map(|d| d.values.as_slice())
    }

    /// The per-target draw for `source_id` at `target`.
    pub fn for_target(&self, source_id: &str, target: &str) -> Option<&[f64]> {
        self.draws
            .iter()
            .find(|d| d.source_id == source_id && d.target.as_deref() == Some(target))
            .map(|d| d.values.as_slice())
    }
}

/// Deterministic PRNG (xorshift64*) — the same algorithm the transport
/// screening design uses, kept crate-private so ensemble draws
/// are bit-reproducible under a declared seed.
pub(crate) struct XorShift64(u64);

impl XorShift64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    /// Uniform double in [0, 1).
    pub(crate) fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Standard normal via Box–Muller.
    pub(crate) fn standard_normal(&mut self) -> f64 {
        let u1 = (1.0 - self.uniform()).max(f64::MIN_POSITIVE);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// Gaussian Φ(z) — Abramowitz–Stegun 7.1.26, |ε| < 1.5e-7. Used to map
/// a copula draw onto a uniform marginal.
fn normal_cdf(z: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.2316419 * z.abs());
    let poly = t
        * (0.319381530
            + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    let pdf = (-0.5 * z * z).exp() / (2.0_f64 * std::f64::consts::PI).sqrt();
    let cdf = 1.0 - pdf * poly;
    if z >= 0.0 { cdf } else { 1.0 - cdf }
}

/// Sample a scalar marginal driven by a correlated standard normal
/// `z` — the Gaussian-copula transform for correlation-group members.
/// `z` is consumed in place of fresh randomness.
fn marginal_from_standard_normal(distribution: &Distribution, z: f64) -> Vec<f64> {
    match distribution {
        Distribution::Point { value } => vec![*value],
        Distribution::Normal { mean, std_dev } => vec![std_dev.mul_add(z, *mean)],
        Distribution::LogNormal { median, sigma_log } => {
            vec![median * (sigma_log * z).exp()]
        }
        Distribution::Uniform { low, high } => {
            vec![low + (high - low) * normal_cdf(z)]
        }
        // Multivariate sources cannot be correlation-group members
        // (validation requires scalar members); unreachable.
        _ => Vec::new(),
    }
}

/// Draw one realization of every source under `rng`. Correlation-group
/// members draw a joint Gaussian vector via the group's Cholesky factor
/// and map coordinates through their marginals (Gaussian copula);
/// `independent_per_target` sources draw once per scope target; shared
/// sources draw once.
fn draw_realization(input: &JointUncertaintyInput, rng: &mut XorShift64) -> Vec<SourceDraw> {
    let mut group_of: BTreeMap<usize, usize> = BTreeMap::new();
    for (group_index, group) in input.correlations.iter().enumerate() {
        for member in &group.sources {
            let index = input
                .sources
                .iter()
                .position(|s| &s.id == member)
                .expect("validated");
            group_of.insert(index, group_index);
        }
    }
    let mut group_z: BTreeMap<usize, Vec<f64>> = BTreeMap::new();
    let mut out = Vec::new();
    for (index, source) in input.sources.iter().enumerate() {
        if let Some(&group_index) = group_of.get(&index) {
            let zs = group_z.entry(group_index).or_insert_with(|| {
                let group = &input.correlations[group_index];
                let n = group.sources.len();
                let l = cholesky_psd(&group.correlation, n).expect("validated");
                let z: Vec<f64> = (0..n).map(|_| rng.standard_normal()).collect();
                (0..n)
                    .map(|i| (0..=i).map(|j| l[i * n + j] * z[j]).sum())
                    .collect()
            });
            let member_index = input.correlations[group_index]
                .sources
                .iter()
                .position(|id| id == &source.id)
                .expect("validated");
            out.push(SourceDraw {
                source_id: source.id.clone(),
                target: None,
                values: marginal_from_standard_normal(&source.distribution, zs[member_index]),
            });
        } else {
            draw_source(source, rng, &mut out);
        }
    }
    out
}

/// Draw one source's values under `rng` (ungrouped sources only) and
/// push one `SourceDraw` per resolved target into `out`.
fn draw_source(source: &JointSource, rng: &mut XorShift64, out: &mut Vec<SourceDraw>) {
    let sample = |rng: &mut XorShift64| -> Vec<f64> {
        match &source.distribution {
            Distribution::Point { value } => vec![*value],
            Distribution::Normal { mean, std_dev } => {
                vec![std_dev.mul_add(rng.standard_normal(), *mean)]
            }
            Distribution::LogNormal { median, sigma_log } => {
                vec![median * (sigma_log * rng.standard_normal()).exp()]
            }
            Distribution::Uniform { low, high } => {
                vec![low + (high - low) * rng.uniform()]
            }
            Distribution::MultivariateNormal { mean, covariance } => {
                let n = mean.len();
                let l = cholesky_psd(covariance, n).expect("validated covariance");
                let z: Vec<f64> = (0..n).map(|_| rng.standard_normal()).collect();
                (0..n)
                    .map(|i| {
                        let mut s = mean[i];
                        for (k, &l_ik) in l[i * n..=i * n + i].iter().enumerate() {
                            s += l_ik * z[k];
                        }
                        s
                    })
                    .collect()
            }
            Distribution::MultivariateLogNormal {
                mean_log,
                covariance_log,
            } => {
                let n = mean_log.len();
                let l = cholesky_psd(covariance_log, n).expect("validated covariance");
                let z: Vec<f64> = (0..n).map(|_| rng.standard_normal()).collect();
                (0..n)
                    .map(|i| {
                        let mut s = mean_log[i];
                        for (k, &l_ik) in l[i * n..=i * n + i].iter().enumerate() {
                            s += l_ik * z[k];
                        }
                        s.exp()
                    })
                    .collect()
            }
        }
    };
    match source.sharing {
        SourceSharing::Shared => {
            let values = sample(rng);
            out.push(SourceDraw {
                source_id: source.id.clone(),
                target: None,
                values,
            });
        }
        SourceSharing::IndependentPerTarget => {
            for target in &source.scope {
                let name = match target {
                    SourceTarget::Field { name }
                    | SourceTarget::Region { name }
                    | SourceTarget::Component { name }
                    | SourceTarget::Interval { name } => name.clone(),
                    SourceTarget::Global => unreachable!("global scope rejected at validation"),
                };
                let values = sample(rng);
                out.push(SourceDraw {
                    source_id: source.id.clone(),
                    target: Some(name),
                    values,
                });
            }
        }
    }
}

/// How the ensemble of realizations is produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnsembleMethod {
    /// Independent seeded draws from the declared distributions.
    MonteCarlo {
        /// Number of realizations. Bounded by [`MAX_REALIZATIONS`].
        realizations: u32,
        /// Reproducibility seed; the same seed replays the same design.
        seed: u64,
    },
    /// Explicitly supplied weighted joint realizations — the
    /// "distribution or explicit weighted samples" alternative. Each
    /// realization must resolve every declared source.
    WeightedSamples { realizations: Vec<Realization> },
}

/// Ensemble evaluation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnsembleSpec {
    pub method: EnsembleMethod,
    /// Quantile probabilities to report, each in `(0, 1)`.
    #[serde(default = "default_quantiles")]
    pub quantiles: Vec<f64>,
    /// Estimator tag recorded on the report. The implementation is the
    /// weighted Hazen plotting position `p_i = (W_{i-1} + w_i/2)/W` —
    /// declared so consumers know exactly which convention produced
    /// the interval.
    #[serde(default)]
    pub quantile_estimator: QuantileEstimator,
    /// Retain per-realization metric samples in the report (bounded by
    /// [`MAX_RETAINED_VALUES`]). Off for large studies.
    #[serde(default)]
    pub retain_samples: bool,
}

fn default_quantiles() -> Vec<f64> {
    vec![0.05, 0.5, 0.95]
}

/// Quantile estimator identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuantileEstimator {
    /// Weighted Hazen plotting positions with linear interpolation.
    #[default]
    WeightedHazen,
}

impl EnsembleSpec {
    pub fn validate(&self) -> Result<(), JointError> {
        match &self.method {
            EnsembleMethod::MonteCarlo { realizations, .. } => {
                if *realizations == 0 || *realizations > MAX_REALIZATIONS {
                    return Err(invalid(format!(
                        "realizations must be in 1..={MAX_REALIZATIONS}"
                    )));
                }
            }
            EnsembleMethod::WeightedSamples { realizations } => {
                if realizations.is_empty() {
                    return Err(invalid("weighted_samples requires ≥1 realization"));
                }
                if realizations.len() > MAX_REALIZATIONS as usize {
                    return Err(invalid("realization count exceeds budget"));
                }
                if realizations
                    .iter()
                    .any(|r| !r.weight.is_finite() || r.weight < 0.0)
                {
                    return Err(invalid("sample weights must be finite non-negative"));
                }
                if realizations.iter().map(|r| r.weight).sum::<f64>() <= 0.0 {
                    return Err(invalid("sample weights sum to zero"));
                }
            }
        }
        if self.quantiles.is_empty()
            || self
                .quantiles
                .iter()
                .any(|q| !q.is_finite() || *q <= 0.0 || *q >= 1.0)
        {
            return Err(invalid("quantile probabilities must lie in (0, 1)"));
        }
        Ok(())
    }
}

/// Materialize the ensemble of realizations under `input`'s declared
/// sources. Draws are deterministic for `MonteCarlo` under the seed.
pub fn realize_ensemble(
    input: &JointUncertaintyInput,
    spec: &EnsembleSpec,
) -> Result<Vec<Realization>, JointError> {
    input.validate()?;
    spec.validate()?;
    match &spec.method {
        EnsembleMethod::MonteCarlo { realizations, seed } => {
            let mut rng = XorShift64((*seed).max(1));
            let mut out = Vec::with_capacity(*realizations as usize);
            for _ in 0..*realizations {
                out.push(Realization {
                    weight: 1.0,
                    draws: draw_realization(input, &mut rng),
                });
            }
            Ok(out)
        }
        EnsembleMethod::WeightedSamples { realizations } => {
            for (index, realization) in realizations.iter().enumerate() {
                check_realization_covers(input, realization)
                    .map_err(|e| invalid(format!("weighted realization {index}: {e}")))?;
            }
            Ok(realizations.clone())
        }
    }
}

/// Verify a supplied realization resolves every source exactly as its
/// sharing declares — no missing or extra draws.
fn check_realization_covers(
    input: &JointUncertaintyInput,
    realization: &Realization,
) -> Result<(), JointError> {
    for source in &input.sources {
        let dimension = source.dimension();
        match source.sharing {
            SourceSharing::Shared => {
                let draw = realization
                    .draws
                    .iter()
                    .find(|d| d.source_id == source.id && d.target.is_none())
                    .ok_or_else(|| invalid(format!("missing draw for source {:?}", source.id)))?;
                if draw.values.len() != dimension || draw.values.iter().any(|v| !v.is_finite()) {
                    return Err(invalid(format!(
                        "source {:?}: draw must be {dimension} finite values",
                        source.id
                    )));
                }
            }
            SourceSharing::IndependentPerTarget => {
                for target in &source.scope {
                    let name = match target {
                        SourceTarget::Field { name }
                        | SourceTarget::Region { name }
                        | SourceTarget::Component { name }
                        | SourceTarget::Interval { name } => name.as_str(),
                        SourceTarget::Global => {
                            return Err(invalid(format!(
                                "source {:?}: global scope with independent sharing",
                                source.id
                            )));
                        }
                    };
                    let draw = realization
                        .draws
                        .iter()
                        .find(|d| d.source_id == source.id && d.target.as_deref() == Some(name))
                        .ok_or_else(|| {
                            invalid(format!(
                                "missing draw for source {:?} target {name:?}",
                                source.id
                            ))
                        })?;
                    if draw.values.len() != dimension || draw.values.iter().any(|v| !v.is_finite())
                    {
                        return Err(invalid(format!(
                            "source {:?} target {name:?}: draw must be {dimension} \
                             finite values",
                            source.id
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Weighted sample statistics. `weights` must be finite, non-negative,
/// and sum positive.
fn weighted_mean(values: &[f64], weights: &[f64]) -> f64 {
    let total: f64 = weights.iter().sum();
    values
        .iter()
        .zip(weights.iter())
        .map(|(v, w)| v * w)
        .sum::<f64>()
        / total
}

/// Weighted variance with the frequency-weight convention
/// `Σw(v−μ)²/Σw` — the population variance of the weighted empirical
/// distribution (the right summary of a *distribution* of metric
/// values, as opposed to a reliability estimate of a mean).
fn weighted_variance(values: &[f64], weights: &[f64], mean: f64) -> f64 {
    let total: f64 = weights.iter().sum();
    values
        .iter()
        .zip(weights.iter())
        .map(|(v, w)| w * (v - mean) * (v - mean))
        .sum::<f64>()
        / total
}

/// Weighted Hazen quantile: sort by value, cumulative weight positions
/// `p_i = (W_{i−1} + w_i/2)/W_total`, linear interpolation between
/// adjacent positions, clamped to the sample extremes.
fn weighted_quantile(sorted: &[(f64, f64)], probability: f64) -> f64 {
    debug_assert!(!sorted.is_empty());
    let total: f64 = sorted.iter().map(|(_, w)| w).sum();
    if total <= 0.0 {
        return sorted[0].0;
    }
    let mut positions = Vec::with_capacity(sorted.len());
    let mut cumulative = 0.0;
    for (value, w) in sorted {
        cumulative += w;
        positions.push((cumulative - w / 2.0) / total);
        let _ = value;
    }
    if probability <= positions[0] {
        return sorted[0].0;
    }
    for i in 1..sorted.len() {
        if probability <= positions[i] {
            let (p0, p1) = (positions[i - 1], positions[i]);
            let t = if p1 > p0 {
                (probability - p0) / (p1 - p0)
            } else {
                0.0
            };
            return sorted[i - 1].0 + t * (sorted[i].0 - sorted[i - 1].0);
        }
    }
    sorted[sorted.len() - 1].0
}

/// Per-realization metric value, identified by the caller's request.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricValue {
    /// Metric identifier supplied by the caller (e.g. `"d95:tumor"`).
    pub metric: String,
    pub value: f64,
}

/// Distribution summary for one requested metric across the ensemble.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricDistribution {
    /// Caller-supplied metric identifier.
    pub metric: String,
    /// Physical unit of the metric values (copied verbatim from the
    /// evaluated quantity).
    pub unit: String,
    /// Realizations that produced a finite value for this metric.
    pub successful_realizations: u64,
    /// Realizations whose evaluation failed or produced a non-finite
    /// value — recorded, never silently dropped.
    pub failed_realizations: u64,
    /// Weighted mean of the metric distribution.
    pub mean: f64,
    /// Weighted population standard deviation.
    pub std_dev: f64,
    /// `std_dev/√N_eff` — the Monte Carlo standard error of the mean,
    /// the sampling-convergence diagnostic.
    pub mc_standard_error: f64,
    /// `(Σw)²/Σw²` — effective sample size under unequal weights.
    pub effective_sample_size: f64,
    /// Requested quantiles, paired with their probability.
    pub quantiles: Vec<ReportedQuantile>,
    /// Retained per-realization values when `retain_samples` was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportedQuantile {
    pub probability: f64,
    pub value: f64,
}

/// Evaluate `evaluate` over the ensemble and summarize each requested
/// metric's distribution. `evaluate` returns one [`MetricValue`] per
/// requested metric for a single realization; a failed realization
/// increments `failed_realizations` and contributes nothing.
///
/// The metric list is discovered from the first successful realization,
/// so callers must return the same metric ids every time. Retained
/// samples are bounded by [`MAX_RETAINED_VALUES`] — the run fails
/// rather than materializing an unbounded tensor.
pub fn run_ensemble<E: std::fmt::Display>(
    input: &JointUncertaintyInput,
    spec: &EnsembleSpec,
    metric_units: &BTreeMap<String, String>,
    evaluate: &mut dyn FnMut(&Realization) -> Result<Vec<MetricValue>, E>,
) -> Result<Vec<MetricDistribution>, JointError> {
    let realizations = realize_ensemble(input, spec)?;
    let n_metrics = metric_units.len();
    if n_metrics == 0 {
        return Err(invalid("at least one requested metric is required"));
    }
    let retained = realizations.len().saturating_mul(n_metrics);
    if spec.retain_samples && retained > MAX_RETAINED_VALUES {
        return Err(invalid(format!(
            "retaining {}×{} = {} metric values exceeds the {MAX_RETAINED_VALUES} budget",
            realizations.len(),
            n_metrics,
            retained
        )));
    }
    let mut series: BTreeMap<String, Vec<(f64, f64)>> = metric_units
        .keys()
        .map(|metric| (metric.clone(), Vec::new()))
        .collect();
    let mut failed: BTreeMap<String, u64> = metric_units
        .keys()
        .map(|metric| (metric.clone(), 0))
        .collect();

    for realization in &realizations {
        match evaluate(realization) {
            Ok(values) => {
                let mut seen = BTreeSet::new();
                for value in values {
                    let Some(series_entry) = series.get_mut(&value.metric) else {
                        return Err(invalid(format!(
                            "evaluation produced undeclared metric {:?}",
                            value.metric
                        )));
                    };
                    if !seen.insert(value.metric.clone()) {
                        return Err(invalid(format!(
                            "metric {:?} reported twice in one realization",
                            value.metric
                        )));
                    }
                    if value.value.is_finite() {
                        series_entry.push((value.value, realization.weight));
                    } else {
                        *failed.get_mut(&value.metric).expect("declared") += 1;
                    }
                }
                // Metrics not reported at all count as failed for this
                // realization.
                for metric in series.keys() {
                    if !seen.contains(metric) {
                        *failed.get_mut(metric).expect("declared") += 1;
                    }
                }
            }
            Err(_) => {
                for count in failed.values_mut() {
                    *count += 1;
                }
            }
        }
    }

    let mut out = Vec::with_capacity(n_metrics);
    for (metric, mut samples) in series {
        samples.sort_by(|a, b| a.0.total_cmp(&b.0));
        let values: Vec<f64> = samples.iter().map(|(v, _)| *v).collect();
        let weights: Vec<f64> = samples.iter().map(|(_, w)| *w).collect();
        let n = samples.len() as u64;
        let (mean, std_dev, mce, ess, quantiles) = if n == 0 {
            (f64::NAN, f64::NAN, f64::NAN, 0.0, Vec::new())
        } else {
            let mean = weighted_mean(&values, &weights);
            let variance = weighted_variance(&values, &weights, mean);
            let w_sum: f64 = weights.iter().sum();
            let w_sq: f64 = weights.iter().map(|w| w * w).sum();
            let ess = if w_sq > 0.0 {
                w_sum * w_sum / w_sq
            } else {
                0.0
            };
            let std_dev = variance.max(0.0).sqrt();
            let quantiles = spec
                .quantiles
                .iter()
                .map(|&probability| ReportedQuantile {
                    probability,
                    value: weighted_quantile(&samples, probability),
                })
                .collect();
            (
                mean,
                std_dev,
                if ess > 0.0 {
                    std_dev / ess.sqrt()
                } else {
                    f64::NAN
                },
                ess,
                quantiles,
            )
        };
        out.push(MetricDistribution {
            metric: metric.clone(),
            unit: metric_units
                .get(&metric)
                .cloned()
                .unwrap_or_else(|| "1".into()),
            successful_realizations: n,
            failed_realizations: failed[&metric],
            mean,
            std_dev,
            mc_standard_error: mce,
            effective_sample_size: ess,
            quantiles,
            samples: spec.retain_samples.then_some(values),
        });
    }
    Ok(out)
}

/// One scalar metric's first-order sensitivity to one source's draw:
/// `a = ∂metric/∂(draw)` in draw units. Signed — cancellation between
/// beams or components is preserved by summing *before* the covariance
/// fold. For `IndependentPerTarget` sources, `target` names the scope
/// target the coefficient applies to (each target is a separate draw
/// and folds in quadrature); for `Shared` sources it must be `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSensitivity {
    /// Index into `JointUncertaintyInput::sources`.
    pub source: usize,
    /// Scope-target name for `IndependentPerTarget` draws; `None` for
    /// shared draws.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `∂metric/∂draw` for this target (or the shared draw).
    pub coefficient: f64,
}

/// The name a `SourceTarget` carries for per-target sensitivity
/// addressing (`Global` has no name and cannot be a per-target key).
fn target_name(target: &SourceTarget) -> Option<&str> {
    match target {
        SourceTarget::Global => None,
        SourceTarget::Field { name }
        | SourceTarget::Region { name }
        | SourceTarget::Component { name }
        | SourceTarget::Interval { name } => Some(name.as_str()),
    }
}

/// Elementary draw coordinates and their joint covariance, factored
/// out of `propagate_first_order` for reuse by the linear-Gaussian
/// value-of-information evaluator: one coordinate per `Shared` source,
/// one per (source, target) pair for `IndependentPerTarget` sources.
/// Correlation-group members form a joint `ρ·σᵢσⱼ` block.
///
/// Elementary draw coordinate: `(source_index, Option<target>)`.
pub(crate) type CoordinateKey = (usize, Option<String>);

/// Returns `(coordinate_keys, covariance)` where each key is
/// `(source_index, Option<target>)`.
pub(crate) fn elementary_covariance(
    input: &JointUncertaintyInput,
) -> Result<(Vec<CoordinateKey>, Vec<f64>), JointError> {
    let mut coordinate_source: Vec<usize> = Vec::new();
    let mut coordinate_index: BTreeMap<(usize, Option<String>), usize> = BTreeMap::new();
    for (s, source) in input.sources.iter().enumerate() {
        match source.sharing {
            SourceSharing::Shared => {
                coordinate_index.insert((s, None), coordinate_source.len());
                coordinate_source.push(s);
            }
            SourceSharing::IndependentPerTarget => {
                for target in &source.scope {
                    let name = target_name(target).expect("independent scope has names");
                    coordinate_index.insert((s, Some(name.to_string())), coordinate_source.len());
                    coordinate_source.push(s);
                }
            }
        }
    }
    let mut sigma = vec![0.0; coordinate_source.len()];
    for (c, &source_index) in coordinate_source.iter().enumerate() {
        let sd = input.sources[source_index].std_dev();
        if sd.len() != 1 {
            return Err(invalid(format!(
                "source {:?}: first-order scalar propagation needs a scalar \
                 distribution; decompose vector draws into declared coordinates",
                input.sources[source_index].id
            )));
        }
        sigma[c] = sd[0];
    }
    let m = coordinate_source.len();
    let mut covariance = vec![0.0; m * m];
    for (c, s) in sigma.iter().enumerate() {
        covariance[c * m + c] = s * s;
    }
    for group in &input.correlations {
        let member_coords: Vec<usize> = group
            .sources
            .iter()
            .map(|id| {
                let source_index = input
                    .sources
                    .iter()
                    .position(|s| &s.id == id)
                    .expect("validated");
                coordinate_index[&(source_index, None)]
            })
            .collect();
        let k = member_coords.len();
        for (gi, &i) in member_coords.iter().enumerate() {
            for (gj, &j) in member_coords.iter().enumerate() {
                covariance[i * m + j] = group.correlation[gi * k + gj] * sigma[i] * sigma[j];
            }
        }
    }
    let keys = coordinate_source
        .iter()
        .enumerate()
        .map(|(c, &s)| {
            (
                s,
                coordinate_index
                    .iter()
                    .find(|(_, v)| **v == c)
                    .and_then(|((_, t), _)| t.clone()),
            )
        })
        .collect();
    Ok((keys, covariance))
}

/// Resolve a `SourceSensitivity` onto the elementary coordinate index.
pub(crate) fn sensitivity_coordinate(
    input: &JointUncertaintyInput,
    sensitivity: &SourceSensitivity,
) -> Result<(usize, Option<String>), JointError> {
    let source = &input.sources[sensitivity.source];
    match source.sharing {
        SourceSharing::Shared => {
            if sensitivity.target.is_some() {
                return Err(invalid(format!(
                    "shared source {:?}: sensitivities must not carry a target",
                    source.id
                )));
            }
            Ok((sensitivity.source, None))
        }
        SourceSharing::IndependentPerTarget => {
            let target = sensitivity.target.as_deref().ok_or_else(|| {
                invalid(format!(
                    "independent-per-target source {:?}: sensitivity needs a target",
                    source.id
                ))
            })?;
            if !source.scope.iter().any(|t| target_name(t) == Some(target)) {
                return Err(invalid(format!(
                    "source {:?}: sensitivity target {target:?} is not in its scope",
                    source.id
                )));
            }
            Ok((sensitivity.source, Some(target.to_string())))
        }
    }
}

/// First-order joint propagation `σ² = aᵀ·C·a` over elementary draws:
/// one coordinate per `Shared` source, one per (source, target) pair
/// for `IndependentPerTarget` sources. Sensitivities to the same draw
/// add linearly — a shared calibration does not shrink when the field
/// it scales is subdivided; independent draws fold in quadrature.
/// Correlation-group members form a joint `ρ·σᵢσⱼ` block.
///
/// Returns `(contributions, total_variance)`: one entry per ungrouped
/// source (summing its coordinate contributions) plus one per
/// correlation group under the joined member ids — reported alongside
/// the total, never forced into a 100% partition.
pub fn propagate_first_order(
    input: &JointUncertaintyInput,
    sensitivities: &[SourceSensitivity],
) -> Result<(Vec<(String, f64)>, f64), JointError> {
    input.validate()?;
    let n = input.sources.len();
    let (keys, covariance) = elementary_covariance(input)?;
    let m = keys.len();
    let key_index: BTreeMap<(usize, Option<String>), usize> = keys
        .iter()
        .cloned()
        .enumerate()
        .map(|(c, k)| (k, c))
        .collect();
    let mut coefficients = vec![0.0; m];
    for sensitivity in sensitivities {
        if sensitivity.source >= n || !sensitivity.coefficient.is_finite() {
            return Err(invalid("sensitivity index out of range or non-finite"));
        }
        let key = sensitivity_coordinate(input, sensitivity)?;
        coefficients[key_index[&key]] += sensitivity.coefficient;
    }
    // σ² = cᵀCc.
    let mut total = 0.0;
    for i in 0..m {
        for j in 0..m {
            total += coefficients[i] * covariance[i * m + j] * coefficients[j];
        }
    }
    // Contributions: per ungrouped source (all its coordinates folded
    // in quadrature), per group the joint block.
    let mut grouped_sources: BTreeSet<usize> = BTreeSet::new();
    for group in &input.correlations {
        for member in &group.sources {
            let index = input
                .sources
                .iter()
                .position(|s| &s.id == member)
                .expect("validated");
            grouped_sources.insert(index);
        }
    }
    let mut contributions: Vec<(String, f64)> = Vec::new();
    for (s, source) in input.sources.iter().enumerate() {
        if grouped_sources.contains(&s) {
            continue;
        }
        let mut contribution = 0.0;
        for c in 0..m {
            if keys[c].0 == s {
                contribution += coefficients[c] * coefficients[c] * covariance[c * m + c];
            }
        }
        contributions.push((source.id.clone(), contribution));
    }
    for group in &input.correlations {
        let member_coords: Vec<usize> = group
            .sources
            .iter()
            .map(|id| {
                let source_index = input
                    .sources
                    .iter()
                    .position(|s| &s.id == id)
                    .expect("validated");
                key_index[&(source_index, None)]
            })
            .collect();
        let mut block = 0.0;
        for &i in &member_coords {
            for &j in &member_coords {
                block += coefficients[i] * covariance[i * m + j] * coefficients[j];
            }
        }
        contributions.push((group.sources.join("+"), block));
    }
    if total < 0.0 && total.abs() < 1e-12 * coefficients.iter().map(|c| c * c).sum::<f64>().max(1.0)
    {
        total = 0.0;
    }
    if total < 0.0 {
        return Err(invalid(
            "declared covariance produced a negative propagated variance — check \
             correlation groups for an inconsistent matrix",
        ));
    }
    Ok((contributions, total))
}

/// One held-out group's first-order (main-effect) variance attribution.
/// `unresolved` retains interactions and higher-order terms — the
/// shares are *not* forced to sum to one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttributionResult {
    /// Group label (single source id or a joined correlation group).
    pub group: String,
    /// `Var(E[metric | group])` — the first-order effect variance.
    pub effect_variance: f64,
    /// `effect_variance / total_variance`; `None` when the total is 0.
    pub fraction: Option<f64>,
}

/// Validate attribution group declarations: disjoint, known source
/// ids, and correlated sources never split across groups. Called by
/// [`attribute_first_order`] and by adapters' preflight checks so
/// errors surface before any sampling.
pub fn validate_attribution_groups(
    input: &JointUncertaintyInput,
    groups: &[Vec<String>],
) -> Result<(), JointError> {
    let mut assigned = BTreeSet::new();
    for group in groups {
        if group.is_empty() {
            return Err(invalid("attribution groups must be non-empty"));
        }
        for member in group {
            if !input.sources.iter().any(|s| &s.id == member) {
                return Err(invalid(format!("unknown attribution source {member:?}")));
            }
            if !assigned.insert(member.as_str()) {
                return Err(invalid(format!(
                    "source {member:?} assigned to two attribution groups"
                )));
            }
        }
    }
    for correlation in &input.correlations {
        let inside = |member: &str| groups.iter().find(|g| g.iter().any(|m| m == member));
        let first = inside(&correlation.sources[0]);
        for member in &correlation.sources[1..] {
            if inside(member) != first {
                return Err(invalid(
                    "correlated sources must share one attribution group",
                ));
            }
        }
    }
    Ok(())
}

/// Grouped first-order attribution: for each declared group of source
/// ids, re-evaluate the metric with those sources sampled and every
/// other source pinned at its point value (mean), under `spec`'s
/// ensemble settings per group. Correlated sources must be grouped
/// together — splitting them would attribute shared variance twice.
///
/// `pin_at_mean` maps each source to its pinned value; the caller owns
/// the metric evaluation. Returns per-group results plus the residual
/// (total − Σ effects), which retains interactions.
pub fn attribute_first_order<E: std::fmt::Display>(
    input: &JointUncertaintyInput,
    spec: &EnsembleSpec,
    groups: &[Vec<String>],
    total_variance: f64,
    evaluate: &mut dyn FnMut(&Realization) -> Result<f64, E>,
) -> Result<(Vec<AttributionResult>, f64), JointError> {
    input.validate()?;
    validate_attribution_groups(input, groups)?;

    // Pinned value per source: distribution mean/point (draw units).
    let pin_values: BTreeMap<String, Vec<f64>> = input
        .sources
        .iter()
        .map(|source| {
            let pinned = match &source.distribution {
                Distribution::Point { value } => vec![*value],
                Distribution::Normal { mean, .. } => vec![*mean],
                Distribution::LogNormal { median, .. } => vec![*median],
                Distribution::Uniform { low, high } => vec![(low + high) / 2.0],
                Distribution::MultivariateNormal { mean, .. } => mean.clone(),
                Distribution::MultivariateLogNormal { mean_log, .. } => {
                    // Median of the drawn vector — the pinned point.
                    mean_log.iter().map(|m| m.exp()).collect()
                }
            };
            (source.id.clone(), pinned)
        })
        .collect();

    let mut results = Vec::with_capacity(groups.len());
    for group in groups {
        // Build an input variant where only this group's sources vary;
        // everything else is pinned at Point(pinned).
        let mut narrowed = input.clone();
        for source in &mut narrowed.sources {
            if group.iter().any(|m| m == &source.id) {
                continue;
            }
            let pinned = pin_values[&source.id].clone();
            source.distribution = if pinned.len() == 1 {
                Distribution::Point { value: pinned[0] }
            } else {
                let n = pinned.len();
                Distribution::MultivariateNormal {
                    mean: pinned,
                    covariance: vec![0.0; n * n],
                }
            };
            // Pinned scalars keep sharing semantics; per-target draws
            // become identical point draws, which is correct.
        }
        // Correlation groups survive narrowing unchanged: a pinned
        // member contributes zero variance to its block.
        let realizations = realize_ensemble(&narrowed, spec)?;
        let mut samples = Vec::with_capacity(realizations.len());
        for realization in &realizations {
            let value = evaluate(realization)
                .map_err(|_| invalid("attribution evaluation failed for a realization"))?;
            if !value.is_finite() {
                return Err(invalid("attribution metric produced a non-finite value"));
            }
            samples.push((value, realization.weight));
        }
        let values: Vec<f64> = samples.iter().map(|(v, _)| *v).collect();
        let weights: Vec<f64> = samples.iter().map(|(_, w)| *w).collect();
        let mean = weighted_mean(&values, &weights);
        let variance = weighted_variance(&values, &weights, mean);
        results.push(AttributionResult {
            group: group.join("+"),
            effect_variance: variance,
            fraction: (total_variance > 0.0).then_some(variance / total_variance),
        });
    }
    let residual = total_variance - results.iter().map(|r| r.effect_variance).sum::<f64>();
    Ok((results, residual))
}

/// Category coverage table for the report: every category gets exactly
/// one status — propagated if a source carries it, else the declared
/// disposition.
pub fn category_coverage(input: &JointUncertaintyInput) -> Vec<CategoryDisposition> {
    [
        UncertaintyCategory::StatisticalSampling,
        UncertaintyCategory::InputParameter,
        UncertaintyCategory::StructuralModel,
        UncertaintyCategory::ModelDiscrepancy,
    ]
    .iter()
    .map(|category| {
        if input.sources.iter().any(|s| s.category == *category) {
            CategoryDisposition {
                category: *category,
                status: CategoryStatus::Propagated,
                note: String::new(),
            }
        } else {
            input
                .category_disposition
                .iter()
                .find(|d| d.category == *category)
                .cloned()
                .unwrap_or(CategoryDisposition {
                    category: *category,
                    status: CategoryStatus::Unassessed,
                    note: "no source declared and no disposition recorded".into(),
                })
        }
    })
    .collect()
}

/// One metric's grouped first-order attribution: per-group effect
/// variances plus the residual (total − Σ effects) that retains
/// interactions. Reported unnormalized — `fraction` inside each effect
/// is the honest share, never a forced 100% partition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricAttribution {
    /// Metric identifier matching [`MetricDistribution::metric`].
    pub metric: String,
    pub effects: Vec<AttributionResult>,
    /// Unexplained variance — interactions between groups, and Monte
    /// Carlo sampling noise; reported unclamped (it may go negative).
    pub residual: f64,
}

/// `openbnct.joint-uncertainty-report/0.1.0` — the distribution of
/// requested metrics under a joint input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JointUncertaintyReport {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// Content binding of the evaluated joint input.
    pub input: ContentReference,
    /// Content binding of the dose/artifact the ensemble perturbed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<ContentReference>,
    /// The joint input, embedded so the report is self-contained.
    pub joint_input: JointUncertaintyInput,
    /// Per-category coverage: propagated / approximated / unassessed /
    /// excluded, with notes.
    pub categories: Vec<CategoryDisposition>,
    pub metrics: Vec<MetricDistribution>,
    /// Grouped first-order attribution per metric that requested it —
    /// correlated sources are constrained to shared groups by
    /// [`attribute_first_order`], so no effect double-counts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribution: Vec<MetricAttribution>,
    /// `monte_carlo` or `weighted_samples`.
    pub method: String,
    pub quantile_estimator: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Recorded assumptions and approximation labels — including what
    /// was *not* propagated.
    pub assumptions: Vec<String>,
    pub qualification: String,
    pub provenance_id: String,
}

impl JointUncertaintyReport {
    pub fn validate(&self) -> Result<(), JointError> {
        if !crate::schema_matches(&self.schema_version, JOINT_UNCERTAINTY_REPORT_SCHEMA) {
            return Err(JointError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() || self.case_id.trim().is_empty() {
            return Err(invalid("id and case_id are required"));
        }
        self.input.validate()?;
        if let Some(subject) = &self.subject {
            subject.validate()?;
        }
        self.joint_input.validate()?;
        if self.metrics.is_empty() {
            return Err(invalid("report carries no metric distributions"));
        }
        for metric in &self.metrics {
            if metric.metric.trim().is_empty() || metric.unit.trim().is_empty() {
                return Err(invalid("metric id and unit are required"));
            }
            if metric.successful_realizations > 0
                && (!metric.mean.is_finite() || !metric.std_dev.is_finite() || metric.std_dev < 0.0)
            {
                return Err(invalid(format!(
                    "metric {:?}: malformed distribution summary",
                    metric.metric
                )));
            }
        }
        if self.qualification.trim().is_empty() || self.provenance_id.trim().is_empty() {
            return Err(invalid("qualification and provenance_id are required"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence() -> SourceEvidence {
        SourceEvidence {
            basis: "test".into(),
            reference: None,
        }
    }

    fn scale_source(id: &str, sigma: f64, scope: Vec<SourceTarget>) -> JointSource {
        JointSource {
            id: id.into(),
            category: UncertaintyCategory::InputParameter,
            scope,
            sharing: SourceSharing::Shared,
            unit: "1".into(),
            support: Support::Real,
            distribution: Distribution::Normal {
                mean: 0.0,
                std_dev: sigma,
            },
            evidence: evidence(),
        }
    }

    fn input(sources: Vec<JointSource>) -> JointUncertaintyInput {
        JointUncertaintyInput {
            schema_version: JOINT_UNCERTAINTY_INPUT_SCHEMA.into(),
            id: "joint.test".into(),
            subjects: Vec::new(),
            sources,
            correlations: Vec::new(),
            category_disposition: vec![
                CategoryDisposition {
                    category: UncertaintyCategory::StatisticalSampling,
                    status: CategoryStatus::Approximated,
                    note: "mc sigmas carried separately".into(),
                },
                CategoryDisposition {
                    category: UncertaintyCategory::StructuralModel,
                    status: CategoryStatus::Unassessed,
                    note: "single model family".into(),
                },
                CategoryDisposition {
                    category: UncertaintyCategory::ModelDiscrepancy,
                    status: CategoryStatus::Unassessed,
                    note: "no comparison assessed".into(),
                },
            ],
            qualification: JOINT_UNCERTAINTY_QUALIFICATION.into(),
            provenance_id: "test".into(),
        }
    }

    #[test]
    fn shared_source_folds_linearly_independent_in_quadrature() {
        // Acceptance fixture: doses 2 and 3, one shared 10%
        // multiplicative error. Sensitivities of the total to the
        // *unit-σ draw* are 0.1·2 = 0.2 and 0.1·3 = 0.3 under one
        // shared source -> a = 0.5, σ = 0.5. Two independent sources
        // give sqrt(0.2² + 0.3²).
        let shared = input(vec![scale_source(
            "calibration",
            1.0,
            vec![
                SourceTarget::Field { name: "a".into() },
                SourceTarget::Field { name: "b".into() },
            ],
        )]);
        let (contrib, total) = propagate_first_order(
            &shared,
            &[
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.2,
                },
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.3,
                },
            ],
        )
        .unwrap();
        assert!((total.sqrt() - 0.5).abs() < 1e-12);
        assert_eq!(contrib.len(), 1);

        let independent = input(vec![
            scale_source("a-cal", 1.0, vec![SourceTarget::Field { name: "a".into() }]),
            scale_source("b-cal", 1.0, vec![SourceTarget::Field { name: "b".into() }]),
        ]);
        let (_, total) = propagate_first_order(
            &independent,
            &[
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.2,
                },
                SourceSensitivity {
                    source: 1,
                    target: None,
                    coefficient: 0.3,
                },
            ],
        )
        .unwrap();
        let expected = (0.2_f64.powi(2) + 0.3_f64.powi(2)).sqrt();
        assert!((total.sqrt() - expected).abs() < 1e-12);
    }

    #[test]
    fn permuting_sources_preserves_the_propagated_result() {
        // Ordering of declared sources is an input artifact, not part
        // of the uncertainty model — the propagated variance and each
        // named source's contribution must be order-invariant.
        let mut a = input(vec![
            scale_source(
                "s-alpha",
                1.0,
                vec![SourceTarget::Field { name: "f".into() }],
            ),
            scale_source(
                "s-beta",
                2.0,
                vec![SourceTarget::Field { name: "f".into() }],
            ),
        ]);
        a.correlations.push(CorrelationGroup {
            sources: vec!["s-alpha".into(), "s-beta".into()],
            correlation: vec![1.0, 0.6, 0.6, 1.0],
        });
        let forward = propagate_first_order(
            &a,
            &[
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.5,
                },
                SourceSensitivity {
                    source: 1,
                    target: None,
                    coefficient: 0.5,
                },
            ],
        )
        .unwrap();
        // Permuted input: sources swapped, sensitivities remapped.
        let mut b = input(vec![
            scale_source(
                "s-beta",
                2.0,
                vec![SourceTarget::Field { name: "f".into() }],
            ),
            scale_source(
                "s-alpha",
                1.0,
                vec![SourceTarget::Field { name: "f".into() }],
            ),
        ]);
        b.correlations.push(CorrelationGroup {
            sources: vec!["s-alpha".into(), "s-beta".into()],
            correlation: vec![1.0, 0.6, 0.6, 1.0],
        });
        let permuted = propagate_first_order(
            &b,
            &[
                SourceSensitivity {
                    source: 1,
                    target: None,
                    coefficient: 0.5,
                },
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.5,
                },
            ],
        )
        .unwrap();
        assert!((forward.1 - permuted.1).abs() < 1e-12);
        // Contribution keyed by source id must match across orders.
        let key = |c: &[(String, f64)]| -> Vec<(String, f64)> { c.to_vec() };
        let mut ka = key(&forward.0);
        let mut kb = key(&permuted.0);
        ka.sort_by(|x, y| x.0.cmp(&y.0));
        kb.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(ka.len(), kb.len());
        for (x, y) in ka.iter().zip(kb.iter()) {
            assert_eq!(x.0, y.0);
            assert!((x.1 - y.1).abs() < 1e-12, "{}: {} vs {}", x.0, x.1, y.1);
        }
    }

    #[test]
    fn ensemble_coverage_matches_the_declared_gaussian_model() {
        // Synthetic coverage under the declared data-generating model:
        // draws from a declared N(0, 1) source must populate the
        // theoretical interval — ±1σ at ~68%, ±1.645σ at ~90%.
        // Over- or under-coverage would mean the sampler inflates or
        // deflates the declared uncertainty.
        let joint = input(vec![scale_source("x", 1.0, vec![SourceTarget::Global])]);
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 4000,
                seed: 7,
            },
            quantiles: vec![0.05, 0.5, 0.95],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let draws: Vec<f64> = realize_ensemble(&joint, &spec)
            .unwrap()
            .iter()
            .map(|r| r.scalar("x").unwrap())
            .collect();
        assert_eq!(draws.len(), 4000);
        let inside = |sigma: f64| -> f64 {
            draws.iter().filter(|&&d| d.abs() <= sigma).count() as f64 / draws.len() as f64
        };
        // ~N(0.683, 0.0074) at n=4000 — a 5σ band.
        assert!((inside(1.0) - 0.6827).abs() < 0.04, "±1σ {}", inside(1.0));
        assert!(
            (inside(1.645) - 0.9).abs() < 0.03,
            "±1.645σ {}",
            inside(1.645)
        );
    }

    #[test]
    fn splitting_a_field_preserves_shared_sigma() {
        // The same shared source applied to a split field must not
        // reduce its contribution: field A dose 2 split into two
        // identical-history halves still carries 0.2 total sensitivity.
        let shared = input(vec![scale_source(
            "calibration",
            1.0,
            vec![SourceTarget::Global],
        )]);
        let (_, total) = propagate_first_order(
            &shared,
            &[
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.1,
                },
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.1,
                },
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.3,
                },
            ],
        )
        .unwrap();
        assert!((total.sqrt() - 0.5).abs() < 1e-12);
    }

    #[test]
    fn correlated_sources_fold_through_declared_matrix() {
        let mut joint = input(vec![
            scale_source("a", 1.0, vec![SourceTarget::Global]),
            scale_source("b", 1.0, vec![SourceTarget::Global]),
        ]);
        joint.correlations = vec![CorrelationGroup {
            sources: vec!["a".into(), "b".into()],
            correlation: vec![1.0, 1.0, 1.0, 1.0],
        }];
        // Perfect positive correlation: σ = a + b.
        let (_, total) = propagate_first_order(
            &joint,
            &[
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.2,
                },
                SourceSensitivity {
                    source: 1,
                    target: None,
                    coefficient: 0.3,
                },
            ],
        )
        .unwrap();
        assert!((total.sqrt() - 0.5).abs() < 1e-9);
        // Anticorrelated: |a − b|.
        joint.correlations[0].correlation = vec![1.0, -1.0, -1.0, 1.0];
        let (_, total) = propagate_first_order(
            &joint,
            &[
                SourceSensitivity {
                    source: 0,
                    target: None,
                    coefficient: 0.2,
                },
                SourceSensitivity {
                    source: 1,
                    target: None,
                    coefficient: 0.3,
                },
            ],
        )
        .unwrap();
        assert!((total.sqrt() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn invalid_covariance_is_rejected_not_repaired() {
        let mut joint = input(vec![scale_source("x", 1.0, vec![SourceTarget::Global])]);
        joint.sources[0].distribution = Distribution::MultivariateNormal {
            mean: vec![0.0, 0.0],
            covariance: vec![1.0, 2.0, 2.0, 1.0],
        };
        joint.sources[0].support = Support::Real;
        assert!(joint.validate().is_err());
        // Singular-but-PSD (perfect correlation) is supported.
        joint.sources[0].distribution = Distribution::MultivariateNormal {
            mean: vec![0.0, 0.0],
            covariance: vec![1.0, 1.0, 1.0, 1.0],
        };
        assert!(joint.validate().is_ok());
    }

    #[test]
    fn support_rules_block_silent_clipping() {
        // A normal on a positive quantity must fail validation.
        let mut joint = input(vec![scale_source("c", 0.1, vec![SourceTarget::Global])]);
        joint.sources[0].support = Support::Positive;
        assert!(joint.validate().is_err());
        joint.sources[0].distribution = Distribution::LogNormal {
            median: 1.0,
            sigma_log: 0.1,
        };
        assert!(joint.validate().is_ok());
    }

    #[test]
    fn unassessed_category_must_be_disposed() {
        let mut joint = input(vec![scale_source("c", 0.1, vec![SourceTarget::Global])]);
        joint.category_disposition.clear();
        assert!(joint.validate().is_err());
    }

    #[test]
    fn ensemble_is_seed_deterministic_and_weighted() {
        let joint = input(vec![scale_source("x", 1.0, vec![SourceTarget::Global])]);
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 64,
                seed: 7,
            },
            quantiles: vec![0.5],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: true,
        };
        let a = realize_ensemble(&joint, &spec).unwrap();
        let b = realize_ensemble(&joint, &spec).unwrap();
        assert_eq!(a, b);
        // Weighted-explicit path rejects empty/zero weights and
        // uncovers sources.
        let bad = EnsembleSpec {
            method: EnsembleMethod::WeightedSamples {
                realizations: vec![Realization {
                    weight: 0.0,
                    draws: vec![SourceDraw {
                        source_id: "x".into(),
                        target: None,
                        values: vec![0.0],
                    }],
                }],
            },
            quantiles: vec![0.5],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        assert!(realize_ensemble(&joint, &bad).is_err());
    }

    #[test]
    fn ensemble_recovers_gaussian_metric_distribution() {
        // Linear metric m = 2·x under x ~ N(0, 1): the ensemble mean →
        // 0 and std_dev → 2 within MC error.
        let joint = input(vec![scale_source("x", 1.0, vec![SourceTarget::Global])]);
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 4000,
                seed: 3,
            },
            quantiles: vec![0.05, 0.5, 0.95],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let units = [("m".to_string(), "gy".to_string())].into_iter().collect();
        let mut eval = |r: &Realization| -> Result<Vec<MetricValue>, String> {
            Ok(vec![MetricValue {
                metric: "m".into(),
                value: 2.0 * r.scalar("x").unwrap(),
            }])
        };
        let out = run_ensemble(&joint, &spec, &units, &mut eval).unwrap();
        let m = &out[0];
        assert!(m.mean.abs() < 0.1);
        assert!((m.std_dev - 2.0).abs() < 0.1);
        assert!((m.quantiles[1].value).abs() < 0.1);
        // p05 ≈ −1.645·2·... wait: N(0,1)·2 → p05 ≈ −3.29.
        assert!((m.quantiles[0].value + 3.29).abs() < 0.2);
        assert!(m.mc_standard_error > 0.0);
    }

    #[test]
    fn zero_declared_variance_is_the_point_estimate() {
        let mut joint = input(vec![scale_source("x", 1.0, vec![SourceTarget::Global])]);
        joint.sources[0].distribution = Distribution::Point { value: 3.0 };
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 8,
                seed: 1,
            },
            quantiles: vec![0.5],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let units = [("m".to_string(), "1".to_string())].into_iter().collect();
        let mut eval = |r: &Realization| -> Result<Vec<MetricValue>, String> {
            Ok(vec![MetricValue {
                metric: "m".into(),
                value: 10.0 * r.scalar("x").unwrap(),
            }])
        };
        let out = run_ensemble(&joint, &spec, &units, &mut eval).unwrap();
        assert_eq!(out[0].mean, 30.0);
        assert_eq!(out[0].std_dev, 0.0);
    }

    #[test]
    fn independent_per_target_draws_each_target() {
        let mut source = scale_source(
            "regional",
            1.0,
            vec![
                SourceTarget::Region { name: "a".into() },
                SourceTarget::Region { name: "b".into() },
            ],
        );
        source.sharing = SourceSharing::IndependentPerTarget;
        let joint = input(vec![source]);
        joint.validate().unwrap();
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 4,
                seed: 1,
            },
            quantiles: vec![0.5],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let draws = realize_ensemble(&joint, &spec).unwrap();
        assert!(draws[0].for_target("regional", "a").is_some());
        assert!(draws[0].for_target("regional", "b").is_some());
        assert_ne!(
            draws[0].for_target("regional", "a"),
            draws[0].for_target("regional", "b")
        );
    }

    #[test]
    fn attribution_keeps_interactions_visible() {
        // m = x·y: each first-order effect is zero, all variance is
        // interaction — the residual must show it.
        let joint = input(vec![
            scale_source("x", 1.0, vec![SourceTarget::Global]),
            scale_source("y", 1.0, vec![SourceTarget::Global]),
        ]);
        let spec = EnsembleSpec {
            method: EnsembleMethod::MonteCarlo {
                realizations: 2000,
                seed: 11,
            },
            quantiles: vec![0.5],
            quantile_estimator: QuantileEstimator::WeightedHazen,
            retain_samples: false,
        };
        let mut eval = |r: &Realization| -> Result<f64, String> {
            Ok(r.scalar("x").unwrap() * r.scalar("y").unwrap())
        };
        // Total variance of x·y for independent standard normals = 1.
        let (results, residual) = attribute_first_order(
            &joint,
            &spec,
            &[vec!["x".into()], vec!["y".into()]],
            1.0,
            &mut eval,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        for r in &results {
            assert!(
                r.effect_variance < 0.1,
                "{} should carry no main effect",
                r.group
            );
        }
        // Nearly all variance unresolved (interaction).
        assert!(residual > 0.8, "residual {residual}");
    }
}
