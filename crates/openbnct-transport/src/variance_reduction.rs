// SPDX-License-Identifier: Apache-2.0

//! Versioned variance-reduction contracts (`openbnct.variance-reduction`,
//! `openbnct.weight-windows`).
//!
//! A [`VarianceReductionSpec`] is the transport-neutral, human-authored
//! declaration of how a transport run's particle population should be
//! biased toward the regions that matter: which particle, over which
//! mesh, with which bounding policy. Its bounds may be declared
//! concretely (`uniform`, `explicit`) or derived from a completed run's
//! tally results (`forward_flux`), in which case a backend resolves them
//! into a [`ResolvedWeightWindows`] artifact
//! (`openbnct.weight-windows/0.1.0`) carrying the concrete arrays and the
//! content-bound evidence of where they came from.
//!
//! Weight-window style variance reduction exists across the supported
//! backends (OpenMC `weight_windows`, MCNP `wwp/wwe`, PHITS importance);
//! this contract captures the shared semantic core. Backends that cannot
//! honor a spec must refuse it explicitly rather than silently degrading
//! to analog transport.
//!
//! Variance reduction changes statistical efficiency only — it must never
//! change the reported physical estimator semantics, and its artifacts are
//! research-verification machinery, not clinical commissioning evidence.

use openbnct_core::ContentReference;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::ParticleType;

pub const VARIANCE_REDUCTION_SCHEMA: &str = "openbnct.variance-reduction/0.1.0";
pub const WEIGHT_WINDOWS_SCHEMA: &str = "openbnct.weight-windows/0.1.0";

/// Transport-neutral variance-reduction declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarianceReductionSpec {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    /// Stable document identifier, e.g. `openbnct.nf-bnct-001.vr.ww.v1`.
    pub id: String,
    pub description: String,
    /// Case the windows are shaped for, when the declaration is
    /// case-specific. A backend must refuse to apply the spec to a
    /// different case.
    pub case_id: Option<String>,
    pub windows: Vec<WeightWindowSpec>,
    /// Where the declared bounds policy comes from — the analytic
    /// argument, the generating run, or the literature it follows.
    /// Required; a window set without stated provenance is not
    /// acceptable verification machinery.
    pub provenance_note: String,
    pub qualification: String,
}

/// One particle's weight-window declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeightWindowSpec {
    pub particle: ParticleType,
    /// Axis-aligned regular mesh the windows are defined over, in cm.
    pub mesh: WeightWindowMesh,
    /// Energy-group edges in eV; `None` declares a single group spanning
    /// the transportable range of the particle.
    pub energy_bounds_ev: Option<Vec<f64>>,
    #[serde(default)]
    pub parameters: WeightWindowParameters,
    pub bounds: WeightWindowBounds,
    /// Post-resolution bound scaling confined to axis-aligned boxes —
    /// the declarative form of region-targeted window strengthening
    /// (e.g. forcing extra photon splitting over a gated voxel) without
    /// authoring ~10^5 explicit bounds by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bound_boosts: Vec<BoundBoost>,
}

/// Axis-aligned box in cm (world frame) whose intersecting mesh cells
/// have both window bounds scaled by `factor`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundBoost {
    pub lower_left_cm: [f64; 3],
    pub upper_right_cm: [f64; 3],
    /// `factor < 1` lowers the window — particles arrive above the upper
    /// bound and split harder; `factor > 1` raises it — particles arrive
    /// below the lower bound and are rouletted harder. Disabled
    /// (negative-bound) cells stay disabled.
    pub factor: f64,
}

/// Regular mesh for weight windows. Coordinates are cm in the case
/// world frame — the same frame the scoring mesh is declared in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeightWindowMesh {
    pub dimensions: [u32; 3],
    pub lower_left_cm: [f64; 3],
    pub upper_right_cm: [f64; 3],
}

impl WeightWindowMesh {
    pub fn n_bins(&self) -> usize {
        self.dimensions.iter().map(|d| *d as usize).product()
    }

    /// Uniform cell volume in cm3.
    pub fn cell_volume_cm3(&self) -> f64 {
        let mut volume = 1.0;
        for axis in 0..3 {
            volume *= (self.upper_right_cm[axis] - self.lower_left_cm[axis])
                / self.dimensions[axis] as f64;
        }
        volume
    }
}

/// Splitting/roulette controls carried through to the backend. Defaults
/// match the OpenMC `WeightWindows` defaults so an unsophisticated spec
/// reproduces the backend's own baseline behavior.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeightWindowParameters {
    /// Ratio of survival weight to lower bound; particles below the lower
    /// bound play Russian roulette to survive at `lower × survival_ratio`.
    #[serde(default = "default_survival_ratio")]
    pub survival_ratio: f64,
    /// Maximum split factor applied when a particle enters a cell above
    /// the upper bound.
    #[serde(default = "default_max_split")]
    pub max_split: u32,
    /// Absolute weight below which particles are rouletted regardless of
    /// window position. Must lie in (0, 1]; values near zero effectively
    /// disable the global cutoff.
    #[serde(default = "default_weight_cutoff")]
    pub weight_cutoff: f64,
    /// Optional cap on `lower_bound / current_weight` before splitting
    /// is limited; `None` leaves the backend default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_lower_bound_ratio: Option<f64>,
}

impl Default for WeightWindowParameters {
    fn default() -> Self {
        Self {
            survival_ratio: default_survival_ratio(),
            max_split: default_max_split(),
            weight_cutoff: default_weight_cutoff(),
            max_lower_bound_ratio: None,
        }
    }
}

const fn default_survival_ratio() -> f64 {
    3.0
}
const fn default_max_split() -> u32 {
    10
}
const fn default_weight_cutoff() -> f64 {
    1.0e-38
}

/// How a window's lower/upper bounds are supplied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WeightWindowBounds {
    /// One constant lower bound in every cell; the upper bound is
    /// `lower_bound × survival_ratio`. Useful for smoke tests and for
    /// globally boosting a penetrating population.
    Uniform { lower_bound: f64 },
    /// Concrete arrays, row-major `(energy, mesh)` — energy group outer,
    /// mesh bin inner — each of length `n_energy × n_mesh`. A negative
    /// lower/upper pair disables the window in that cell.
    Explicit {
        lower_bounds: Vec<f64>,
        upper_bounds: Vec<f64>,
    },
    /// Derived by a backend from a completed run's forward-flux tally
    /// over this mesh — the MAGIC-style rule
    /// `lower(e,m) = flux(e,m) / (2 × group_max(e))` with cells whose
    /// relative tally error exceeds `rel_err_threshold` left disabled.
    /// Resolution requires the generating statepoint and produces a
    /// content-bound `openbnct.weight-windows` artifact.
    ForwardFlux {
        /// Name of the mesh tally to derive from, e.g.
        /// `openbnct.diagnostic.photon_fluence`.
        tally: String,
        /// Cells with tally relative error above this are disabled
        /// (bound pair set negative) rather than trusted.
        rel_err_threshold: f64,
    },
    /// Derived by the in-house deterministic adjoint S_N solve
    /// (CADIS/FW-CADIS): the adjoint scalar flux is the per-cell,
    /// per-group importance toward the declared response, and window
    /// targets follow `w₀(e,m) = w_ref / φ†(e,m)` normalized so
    /// declared-source particles are born at their local target weight.
    /// Resolution requires the transport case and `openbnct.multigroup-data`
    /// artifact; `fw_cadis` additionally requires a forward
    /// `openbnct.multigroup-flux` artifact.
    Adjoint {
        method: AdjointMethod,
        response: AdjointResponse,
        /// Clamp on the window target weight as a multiple of the unit
        /// source weight; also the kill-window value assigned to cells
        /// with zero adjoint flux. `None` uses the resolver default
        /// (currently 1e6).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target_cap: Option<f64>,
    },
}

impl WeightWindowBounds {
    /// The declared `adjoint` target cap, if this is an adjoint window.
    pub fn target_cap(&self) -> Option<f64> {
        match self {
            WeightWindowBounds::Adjoint { target_cap, .. } => *target_cap,
            _ => None,
        }
    }
}

/// CADIS variant selected by `WeightWindowBounds::Adjoint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdjointMethod {
    /// Classical CADIS: adjoint source is the declared detector
    /// response; windows pull the population toward that response.
    Cadis,
    /// Forward-weighted CADIS: adjoint source is `1/φ_fwd`, flattening
    /// the population distribution for global (mesh-wide) tallies.
    FwCadis,
}

/// What the adjoint source `q†` represents — the objective the
/// importance map is built toward.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdjointResponse {
    /// A named dose-response vector from the `openbnct.multigroup-data`
    /// artifact, folded through the material at each voxel — the adjoint
    /// source for "importance toward that dose component".
    DoseComponent { component: String },
    /// Unit response inside an inclusive voxel box (detector region).
    VoxelBox { lower: [u32; 3], upper: [u32; 3] },
    /// Unit response in every voxel — global importance.
    Global,
}

/// Resolved, backend-ready weight windows — concrete bounds plus the
/// provenance of how they were produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedWeightWindows {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: Option<String>,
    /// Content-bound reference to the `openbnct.variance-reduction`
    /// spec this resolves.
    pub spec: ContentReference,
    pub windows: Vec<ResolvedWeightWindow>,
    pub derivation: WeightWindowDerivation,
    pub qualification: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedWeightWindow {
    pub particle: ParticleType,
    pub mesh: WeightWindowMesh,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_bounds_ev: Option<Vec<f64>>,
    /// Row-major `(energy, mesh)` lower bounds; negative = window off.
    pub lower_bounds: Vec<f64>,
    pub upper_bounds: Vec<f64>,
    pub parameters: WeightWindowParameters,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeightWindowDerivation {
    /// `uniform`, `explicit`, `forward_flux`, `cadis`, or `fw_cadis` —
    /// per-window resolution is only homogeneous for the trivial
    /// policies; a mixed spec records `mixed`.
    pub method: String,
    /// Generating statepoint when bounds were flux-derived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_statepoint: Option<ContentReference>,
    /// Content-bound adjoint flux artifact(s) when bounds were derived
    /// by the in-house adjoint S_N solve — one per adjoint window, in
    /// window order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjoint_flux: Vec<ContentReference>,
    /// Content-bound forward flux artifact feeding `fw_cadis`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_flux: Option<ContentReference>,
    /// Content-bound multigroup data the adjoint solve consumed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multigroup_data: Option<ContentReference>,
    /// Content-bound transport case the adjoint solve ran on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport_case: Option<ContentReference>,
    /// Free-text note recording derivation parameters (threshold,
    /// normalization) that are not recoverable from the artifact itself.
    pub note: String,
}

impl WeightWindowSpec {
    pub fn energy_groups(&self) -> usize {
        self.energy_bounds_ev
            .as_ref()
            .map_or(1, |bounds| bounds.len() - 1)
    }

    /// Applies `bound_boosts` to a resolved bound pair in place, scaling
    /// both bounds of every mesh cell that overlaps each boost box.
    /// Disabled (negative-lower) cells are left alone. Returns the
    /// number of cell entries scaled per energy group — the same cells
    /// are scaled in every group.
    pub fn apply_bound_boosts(&self, lower: &mut [f64], upper: &mut [f64]) -> usize {
        if self.bound_boosts.is_empty() {
            return 0;
        }
        let [nx, ny, nz] = self.mesh.dimensions;
        let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
        let mesh_bins = nx * ny * nz;
        let cell_width = |axis: usize| {
            (self.mesh.upper_right_cm[axis] - self.mesh.lower_left_cm[axis])
                / self.mesh.dimensions[axis] as f64
        };
        let mut scaled = 0usize;
        for boost in &self.bound_boosts {
            for iz in 0..nz {
                for iy in 0..ny {
                    for ix in 0..nx {
                        let index = [ix, iy, iz];
                        let overlaps = (0..3).all(|axis| {
                            let lo = self.mesh.lower_left_cm[axis]
                                + index[axis] as f64 * cell_width(axis);
                            let hi = lo + cell_width(axis);
                            hi > boost.lower_left_cm[axis] && lo < boost.upper_right_cm[axis]
                        });
                        if !overlaps {
                            continue;
                        }
                        // Row-major mesh bin: x fastest, then y, then z —
                        // the ordering OpenMC expects inside a bounds row.
                        let bin = (iz * ny + iy) * nx + ix;
                        let mut touched = false;
                        for e in 0..self.energy_groups() {
                            let i = e * mesh_bins + bin;
                            if lower[i] < 0.0 {
                                continue;
                            }
                            lower[i] *= boost.factor;
                            upper[i] *= boost.factor;
                            touched = true;
                        }
                        if touched {
                            scaled += 1;
                        }
                    }
                }
            }
        }
        scaled
    }
}

impl ResolvedWeightWindow {
    pub fn energy_groups(&self) -> usize {
        self.energy_bounds_ev
            .as_ref()
            .map_or(1, |bounds| bounds.len() - 1)
    }
}

#[derive(Debug, Error)]
pub enum VarianceReductionError {
    #[error("schema_version must be {expected}, got {actual}")]
    Schema { expected: String, actual: String },
    #[error("variance-reduction document requires {0}")]
    Missing(&'static str),
    #[error("weight window {index} mesh invalid: {reason}")]
    Mesh { index: usize, reason: String },
    #[error("weight window {index} energy bounds invalid: {reason}")]
    EnergyBounds { index: usize, reason: String },
    #[error("weight window {index} parameters invalid: {reason}")]
    Parameters { index: usize, reason: String },
    #[error("weight window {index} bounds invalid: {reason}")]
    Bounds { index: usize, reason: String },
    #[error("spec {spec} declares case {spec_case} but target case is {case}")]
    CaseMismatch {
        spec: String,
        spec_case: String,
        case: String,
    },
}

impl VarianceReductionSpec {
    pub fn validate(&self) -> Result<(), VarianceReductionError> {
        if !openbnct_core::schema_matches(&self.schema_version, VARIANCE_REDUCTION_SCHEMA) {
            return Err(VarianceReductionError::Schema {
                expected: VARIANCE_REDUCTION_SCHEMA.into(),
                actual: self.schema_version.clone(),
            });
        }
        for field in [
            (self.id.is_empty(), "id"),
            (self.description.is_empty(), "description"),
            (self.provenance_note.is_empty(), "provenance_note"),
            (self.qualification.is_empty(), "qualification"),
        ] {
            if field.0 {
                return Err(VarianceReductionError::Missing(field.1));
            }
        }
        if self.windows.is_empty() {
            return Err(VarianceReductionError::Missing("windows"));
        }
        for (index, window) in self.windows.iter().enumerate() {
            validate_mesh(&window.mesh, index)?;
            validate_energy_bounds(window.energy_bounds_ev.as_deref(), index)?;
            window.parameters.validate(index)?;
            validate_bounds(&window.bounds, window, index)?;
            for boost in &window.bound_boosts {
                for axis in 0..3 {
                    if !strictly_greater(boost.upper_right_cm[axis], boost.lower_left_cm[axis]) {
                        return Err(VarianceReductionError::Bounds {
                            index,
                            reason: format!("boost box axis {axis} is degenerate"),
                        });
                    }
                }
                if !strictly_greater(boost.factor, 0.0) {
                    return Err(VarianceReductionError::Bounds {
                        index,
                        reason: "boost factor must be finite and > 0".into(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The spec resolves without external artifacts only when every
    /// window's bounds are already concrete — `forward_flux` needs a
    /// statepoint and `adjoint` needs the transport case plus multigroup
    /// data.
    pub fn is_self_contained(&self) -> bool {
        self.windows.iter().all(|w| {
            matches!(
                w.bounds,
                WeightWindowBounds::Uniform { .. } | WeightWindowBounds::Explicit { .. }
            )
        })
    }
}

impl ResolvedWeightWindows {
    pub fn validate(&self) -> Result<(), VarianceReductionError> {
        if !openbnct_core::schema_matches(&self.schema_version, WEIGHT_WINDOWS_SCHEMA) {
            return Err(VarianceReductionError::Schema {
                expected: WEIGHT_WINDOWS_SCHEMA.into(),
                actual: self.schema_version.clone(),
            });
        }
        for field in [
            (self.id.is_empty(), "id"),
            (self.qualification.is_empty(), "qualification"),
            (self.derivation.note.is_empty(), "derivation.note"),
            (self.derivation.method.is_empty(), "derivation.method"),
        ] {
            if field.0 {
                return Err(VarianceReductionError::Missing(field.1));
            }
        }
        if self.windows.is_empty() {
            return Err(VarianceReductionError::Missing("windows"));
        }
        for (index, window) in self.windows.iter().enumerate() {
            validate_mesh(&window.mesh, index)?;
            validate_energy_bounds(window.energy_bounds_ev.as_deref(), index)?;
            window.parameters.validate(index)?;
            let expected = window.energy_groups() * window.mesh.n_bins();
            if window.lower_bounds.len() != expected || window.upper_bounds.len() != expected {
                return Err(VarianceReductionError::Bounds {
                    index,
                    reason: format!(
                        "bounds length {}/{} != n_energy x n_mesh = {}",
                        window.lower_bounds.len(),
                        window.upper_bounds.len(),
                        expected
                    ),
                });
            }
            for (cell, (&lower, &upper)) in window
                .lower_bounds
                .iter()
                .zip(window.upper_bounds.iter())
                .enumerate()
            {
                // A negative lower bound marks the cell inactive; the
                // upper entry is ignored there but must still be finite.
                if at_least(lower, 0.0) && !at_least(upper, lower) {
                    return Err(VarianceReductionError::Bounds {
                        index,
                        reason: format!(
                            "cell {cell}: upper bound {upper} below lower bound {lower}"
                        ),
                    });
                }
            }
        }
        Ok(())
    }
}

impl WeightWindowParameters {
    fn validate(&self, index: usize) -> Result<(), VarianceReductionError> {
        let invalid = |reason: &str| VarianceReductionError::Parameters {
            index,
            reason: reason.to_string(),
        };
        if !strictly_greater(self.survival_ratio, 1.0) {
            return Err(invalid("survival_ratio must be > 1"));
        }
        if self.max_split <= 1 {
            return Err(invalid("max_split must be > 1"));
        }
        if !(strictly_greater(self.weight_cutoff, 0.0) && at_least(1.0, self.weight_cutoff)) {
            return Err(invalid("weight_cutoff must be in (0, 1]"));
        }
        if let Some(ratio) = self.max_lower_bound_ratio
            && !strictly_greater(ratio, 1.0)
        {
            return Err(invalid("max_lower_bound_ratio must be > 1"));
        }
        Ok(())
    }
}

/// Strict `x > y` under partial ordering: NaN never satisfies a strict
/// comparison, so a `None` result fails the check.
fn strictly_greater(x: f64, y: f64) -> bool {
    matches!(x.partial_cmp(&y), Some(std::cmp::Ordering::Greater))
}

/// `x >= y` under partial ordering; NaN fails the check.
fn at_least(x: f64, y: f64) -> bool {
    matches!(
        x.partial_cmp(&y),
        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
    )
}

fn validate_mesh(mesh: &WeightWindowMesh, index: usize) -> Result<(), VarianceReductionError> {
    for axis in 0..3 {
        if mesh.dimensions[axis] == 0 {
            return Err(VarianceReductionError::Mesh {
                index,
                reason: format!("dimension {axis} is zero"),
            });
        }
        if !strictly_greater(mesh.upper_right_cm[axis], mesh.lower_left_cm[axis]) {
            return Err(VarianceReductionError::Mesh {
                index,
                reason: format!(
                    "axis {axis}: upper_right {} not above lower_left {}",
                    mesh.upper_right_cm[axis], mesh.lower_left_cm[axis]
                ),
            });
        }
    }
    Ok(())
}

fn validate_energy_bounds(
    bounds: Option<&[f64]>,
    index: usize,
) -> Result<(), VarianceReductionError> {
    let Some(bounds) = bounds else { return Ok(()) };
    if bounds.len() < 2 {
        return Err(VarianceReductionError::EnergyBounds {
            index,
            reason: "fewer than two edges".into(),
        });
    }
    for pair in bounds.windows(2) {
        if !strictly_greater(pair[1], pair[0]) || !at_least(pair[0], 0.0) {
            return Err(VarianceReductionError::EnergyBounds {
                index,
                reason: "edges must be non-negative and strictly increasing".into(),
            });
        }
    }
    Ok(())
}

fn validate_bounds(
    bounds: &WeightWindowBounds,
    window: &WeightWindowSpec,
    index: usize,
) -> Result<(), VarianceReductionError> {
    let invalid = |reason: String| VarianceReductionError::Bounds { index, reason };
    match bounds {
        WeightWindowBounds::Uniform { lower_bound } => {
            if !strictly_greater(*lower_bound, 0.0) {
                return Err(invalid("uniform lower_bound must be > 0".into()));
            }
        }
        WeightWindowBounds::Explicit {
            lower_bounds,
            upper_bounds,
        } => {
            let groups = window.energy_bounds_ev.as_ref().map_or(1, |b| b.len() - 1);
            let expected = groups * window.mesh.n_bins();
            if lower_bounds.len() != expected || upper_bounds.len() != expected {
                return Err(invalid(format!(
                    "explicit bounds length {}/{} != n_energy x n_mesh = {expected}",
                    lower_bounds.len(),
                    upper_bounds.len()
                )));
            }
        }
        WeightWindowBounds::Adjoint {
            response,
            target_cap,
            ..
        } => {
            if let Some(cap) = target_cap
                && !strictly_greater(*cap, 1.0)
            {
                return Err(invalid("adjoint target_cap must be > 1".into()));
            }
            match response {
                AdjointResponse::DoseComponent { component } => {
                    if component.is_empty() {
                        return Err(invalid("adjoint dose component name is empty".into()));
                    }
                }
                AdjointResponse::VoxelBox { lower, upper } => {
                    for axis in 0..3 {
                        if lower[axis] > upper[axis] {
                            return Err(invalid(format!(
                                "adjoint voxel-box axis {axis}: lower above upper"
                            )));
                        }
                    }
                }
                AdjointResponse::Global => {}
            }
        }
        WeightWindowBounds::ForwardFlux {
            tally,
            rel_err_threshold,
        } => {
            if tally.is_empty() {
                return Err(invalid("forward_flux tally name is empty".into()));
            }
            if !(strictly_greater(*rel_err_threshold, 0.0)
                && strictly_greater(1.0, *rel_err_threshold))
            {
                return Err(invalid("rel_err_threshold must be in (0, 1)".into()));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mesh() -> WeightWindowMesh {
        WeightWindowMesh {
            dimensions: [4, 4, 4],
            lower_left_cm: [-10.0; 3],
            upper_right_cm: [10.0; 3],
        }
    }

    fn spec(bounds: WeightWindowBounds) -> VarianceReductionSpec {
        VarianceReductionSpec {
            schema_version: VARIANCE_REDUCTION_SCHEMA.into(),
            id: "openbnct.test.vr.v1".into(),
            description: "test spec".into(),
            case_id: Some("nf-bnct-001".into()),
            windows: vec![WeightWindowSpec {
                particle: ParticleType::Photon,
                mesh: mesh(),
                energy_bounds_ev: Some(vec![0.0, 1.0e6, 2.0e7]),
                parameters: WeightWindowParameters::default(),
                bounds,
                bound_boosts: vec![],
            }],
            provenance_note: "unit test".into(),
            qualification: "variance_reduction_research_only".into(),
        }
    }

    #[test]
    fn uniform_spec_validates() {
        spec(WeightWindowBounds::Uniform { lower_bound: 0.25 })
            .validate()
            .unwrap();
    }

    #[test]
    fn explicit_bounds_must_match_energy_mesh_shape() {
        let bad = spec(WeightWindowBounds::Explicit {
            lower_bounds: vec![0.1; 64],
            upper_bounds: vec![0.5; 64],
        });
        // 2 energy groups x 64 mesh bins = 128 expected; 64 fails.
        bad.validate().unwrap_err();
        let good = spec(WeightWindowBounds::Explicit {
            lower_bounds: vec![0.1; 128],
            upper_bounds: vec![0.5; 128],
        });
        good.validate().unwrap();
    }

    #[test]
    fn forward_flux_requires_statepoint() {
        let s = spec(WeightWindowBounds::ForwardFlux {
            tally: "openbnct.diagnostic.photon_fluence".into(),
            rel_err_threshold: 0.5,
        });
        s.validate().unwrap();
        assert!(!s.is_self_contained());
        let u = spec(WeightWindowBounds::Uniform { lower_bound: 0.25 });
        assert!(u.is_self_contained());
    }

    #[test]
    fn parameters_reject_backwards_knobs() {
        let mut s = spec(WeightWindowBounds::Uniform { lower_bound: 0.25 });
        s.windows[0].parameters.survival_ratio = 0.5;
        assert!(matches!(
            s.validate(),
            Err(VarianceReductionError::Parameters { .. })
        ));
        s.windows[0].parameters.survival_ratio = 3.0;
        s.windows[0].parameters.max_split = 1;
        s.validate().unwrap_err();
        s.windows[0].parameters.max_split = 10;
        s.windows[0].parameters.weight_cutoff = 0.0;
        s.validate().unwrap_err();
    }

    #[test]
    fn resolved_windows_validate_shape_and_pairs() {
        let resolved = ResolvedWeightWindows {
            schema_version: WEIGHT_WINDOWS_SCHEMA.into(),
            id: "openbnct.test.ww.v1".into(),
            case_id: Some("nf-bnct-001".into()),
            spec: ContentReference {
                id: "openbnct.test.vr.v1".into(),
                sha256: "00".repeat(32),
            },
            windows: vec![ResolvedWeightWindow {
                particle: ParticleType::Photon,
                mesh: mesh(),
                energy_bounds_ev: Some(vec![0.0, 1.0e6, 2.0e7]),
                lower_bounds: vec![0.1; 128],
                upper_bounds: vec![0.5; 128],
                parameters: WeightWindowParameters::default(),
            }],
            derivation: WeightWindowDerivation {
                method: "uniform".into(),
                source_statepoint: None,
                adjoint_flux: Vec::new(),
                forward_flux: None,
                multigroup_data: None,
                transport_case: None,
                note: "test".into(),
            },
            qualification: "variance_reduction_research_only".into(),
        };
        resolved.validate().unwrap();

        let mut inverted = resolved.clone();
        inverted.windows[0].upper_bounds[3] = 0.05;
        inverted.validate().unwrap_err();

        // Negative pair = disabled cell; accepted.
        let mut disabled = resolved.clone();
        disabled.windows[0].lower_bounds[0] = -1.0;
        disabled.windows[0].upper_bounds[0] = -1.0;
        disabled.validate().unwrap();
    }

    #[test]
    fn missing_provenance_rejected() {
        let mut s = spec(WeightWindowBounds::Uniform { lower_bound: 0.25 });
        s.provenance_note.clear();
        assert!(matches!(
            s.validate(),
            Err(VarianceReductionError::Missing("provenance_note"))
        ));
    }

    #[test]
    fn bound_boost_scales_overlapping_cells_x_fastest() {
        // 4x4x4 mesh over [-10,10] cm: 5 cm cells. The box covers cells
        // ix in {1,2}, iy=2, iz=3 — row-major bins 57 and 58.
        let mut s = spec(WeightWindowBounds::Uniform { lower_bound: 0.25 });
        s.windows[0].bound_boosts = vec![BoundBoost {
            lower_left_cm: [-5.0, 0.01, 5.01],
            upper_right_cm: [5.0, 5.0, 10.0],
            factor: 0.1,
        }];
        s.validate().unwrap();
        let cells = 2 * 64;
        let mut lower = vec![0.25; cells];
        let mut upper = vec![0.75; cells];
        // Disable bin 57 inside the boost box in every group; it must
        // stay disabled.
        for e in 0..2 {
            lower[e * 64 + 57] = -1.0;
            upper[e * 64 + 57] = -1.0;
        }
        let scaled = s.windows[0].apply_bound_boosts(&mut lower, &mut upper);
        assert_eq!(scaled, 1);
        for e in 0..2 {
            assert_eq!(lower[e * 64 + 57], -1.0);
            assert!((lower[e * 64 + 58] - 0.025).abs() < 1e-15);
            assert!((upper[e * 64 + 58] - 0.075).abs() < 1e-15);
        }
        // A cell the box does not touch is untouched.
        assert_eq!(lower[0], 0.25);
        assert_eq!(upper[0], 0.75);
    }

    #[test]
    fn degenerate_boost_rejected() {
        let mut s = spec(WeightWindowBounds::Uniform { lower_bound: 0.25 });
        s.windows[0].bound_boosts = vec![BoundBoost {
            lower_left_cm: [0.0; 3],
            upper_right_cm: [0.0, 1.0, 1.0],
            factor: 0.5,
        }];
        s.validate().unwrap_err();
        s.windows[0].bound_boosts[0].upper_right_cm[0] = 1.0;
        s.windows[0].bound_boosts[0].factor = 0.0;
        s.validate().unwrap_err();
        s.windows[0].bound_boosts[0].factor = f64::NAN;
        s.validate().unwrap_err();
    }
}
