// SPDX-License-Identifier: MIT

//! Response-weighted adaptive group-boundary placement (R17-04).
//!
//! A fixed lethargy grid is wasteful wherever the flux response is
//! concentrated in a narrow band — the thermal peak and the epi-to-fast
//! moderation ramp need fine structure while smooth regions tolerate
//! wide groups. `adapt_boundaries` takes an importance spectrum (a
//! `sn spectrum` extraction, a measured beam histogram, or a solved
//! flux collapse), optionally folded with a per-bin response, and
//! places `groups + 1` edges so that every group carries equal
//! importance mass over lethargy — the equal-mass boundary rule used
//! across the deterministic-transport literature.
//!
//! The emitted document is a `openbnct.boundary-proposal/0.1.0`
//! artifact carrying the descending edge list plus the rationale
//! (which lethargy intervals concentrate importance) so the choice is
//! declared, not hidden in a grid file.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::EnergyDistribution;

/// Schema of the emitted boundary-proposal document.
pub const BOUNDARY_PROPOSAL_SCHEMA: &str = "openbnct.boundary-proposal/0.1.0";

/// Adaptive boundary proposal: the edge list `sn collapse
/// --boundaries-file` consumes plus the placement rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryProposal {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    /// Group edges in eV, strictly descending (group 0 = highest).
    pub energy_boundaries_ev: Vec<f64>,
    /// What the edges were placed against.
    pub placement: String,
    /// Per-group share of importance mass — should cluster at
    /// `1/groups`; reported so the document shows its own balance.
    pub group_mass_shares: Vec<f64>,
    /// Lethargy interval (ln ratio) each group spans, same order as
    /// `group_mass_shares`.
    pub group_lethargy_widths: Vec<f64>,
    /// SHA-256 of the importance-spectrum input, when emitted by the CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spectrum_sha256: Option<String>,
    /// SHA-256 of the optional response-curve input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_sha256: Option<String>,
}

#[derive(Debug, Error)]
pub enum BoundaryError {
    #[error("importance spectrum must be a tabulated histogram")]
    NotHistogram,
    #[error("need at least 2 bins of positive importance to place {groups} groups")]
    DegenerateImportance { groups: usize },
    #[error("response vector length {actual} must equal spectrum bins {expected}")]
    ResponseLength { expected: usize, actual: usize },
    #[error("groups must be ≥ 1, got {0}")]
    NoGroups(usize),
    #[error("spectrum edges must be strictly increasing positive energies")]
    BadEdges,
}

/// Importance density per unit lethargy for each histogram bin.
fn importance_density(
    energy_boundaries_ev: &[f64],
    bin_weights: &[f64],
    response: Option<&[f64]>,
) -> Result<Vec<f64>, BoundaryError> {
    let n = bin_weights.len();
    if energy_boundaries_ev.len() != n + 1
        || n == 0
        || energy_boundaries_ev
            .windows(2)
            .any(|w| !(w[0] > 0.0 && w[1] > w[0]))
    {
        return Err(BoundaryError::BadEdges);
    }
    let mut density = Vec::with_capacity(n);
    for i in 0..n {
        let du = (energy_boundaries_ev[i + 1] / energy_boundaries_ev[i]).ln();
        let r = response.map(|w| w[i]).unwrap_or(1.0);
        density.push((bin_weights[i] / du) * r.max(0.0));
    }
    Ok(density)
}

/// Place `groups + 1` edges so each group spans equal importance mass
/// over lethargy. `response` optionally supplies one non-negative weight
/// per spectrum bin (a folded response curve on the same binning).
/// `uniform_floor` (0..1) blends a flat-per-lethargy share of the total
/// importance mass into every bin — the anti-starvation guard: an
/// equilibrium flux spectrum concentrates its mass where flux lives
/// (thermal) and gives the source/moderation bands too few groups to
/// transport; the floor reserves `uniform_floor × groups` bins for
/// uniform-lethargy coverage. Returns descending edges suitable for
/// `sn collapse --boundaries`.
pub fn adapt_boundaries(
    spectrum: &EnergyDistribution,
    groups: usize,
    response: Option<&[f64]>,
    uniform_floor: f64,
) -> Result<BoundaryProposal, BoundaryError> {
    if groups == 0 {
        return Err(BoundaryError::NoGroups(groups));
    }
    let (mut edges, mut weights) = match spectrum {
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev,
            bin_weights,
        } => (energy_boundaries_ev.clone(), bin_weights.clone()),
        EnergyDistribution::Monoenergetic { .. } => return Err(BoundaryError::NotHistogram),
    };
    // `sn spectrum` emits edges descending (group order); the placement
    // works on an ascending orientation.
    if edges.len() >= 2 && edges[0] > edges[edges.len() - 1] {
        edges.reverse();
        weights.reverse();
    }
    let (edges, weights) = (&edges, &weights);
    if let Some(r) = response.filter(|r| r.len() != weights.len()) {
        return Err(BoundaryError::ResponseLength {
            expected: weights.len(),
            actual: r.len(),
        });
    }
    let mut density = importance_density(edges, weights, response)?;
    let n = density.len();
    // Importance MASS per bin = per-lethargy density × lethargy width
    // (equivalently w_i·r_i). Accumulating density alone would treat
    // every bin as equal and degenerate onto the input edges.
    let du: Vec<f64> = edges.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
    let mut mass: Vec<f64> = density
        .iter()
        .zip(&du)
        .map(|(d, u)| d.max(0.0) * u)
        .collect();
    let mut total: f64 = mass.iter().sum();
    if n < 2 || total <= 0.0 {
        return Err(BoundaryError::DegenerateImportance { groups });
    }
    // Anti-starvation floor: blend a fraction of the total importance
    // mass with flat-per-lethargy coverage so the source/moderation
    // bands keep enough groups to transport the flux that eventually
    // collects in the importance peak.
    if uniform_floor > 0.0 {
        let f = uniform_floor.clamp(0.0, 1.0);
        let u_total: f64 = du.iter().sum();
        for (m, u) in mass.iter_mut().zip(&du) {
            *m = (1.0 - f) * *m / total + f * u / u_total;
        }
        total = 1.0;
        for (d, (m, u)) in density.iter_mut().zip(mass.iter().zip(&du)) {
            *d = *m / *u;
        }
    }
    let cum: Vec<f64> = mass
        .iter()
        .scan(0.0, |acc, m| {
            *acc += *m;
            Some(*acc)
        })
        .collect();
    // Quantile targets C_k = total·k/groups → interpolated energies.
    // Interpolation is linear in lethargy within the holding bin.
    let mut out = vec![edges[0]];
    for k in 1..groups {
        let target = total * k as f64 / groups as f64;
        // First bin whose cumulative mass reaches the target.
        let i = cum.iter().position(|&c| c >= target).unwrap_or(n - 1);
        let prev = if i == 0 { 0.0 } else { cum[i - 1] };
        let bin_mass = cum[i] - prev;
        let frac = if bin_mass > 0.0 {
            ((target - prev) / bin_mass).clamp(0.0, 1.0)
        } else {
            1.0
        };
        // Within a bin the importance density is per-lethargy uniform
        // (the only shape the histogram can express) — invert the CDF
        // exponentially in energy.
        out.push(edges[i] * (edges[i + 1] / edges[i]).powf(frac));
    }
    out.push(*edges.last().expect("histogram edges"));
    // Edges currently ascend; emit descending with the shares/widths.
    let mut proposal_edges: Vec<f64> = out.into_iter().rev().collect();
    // Guard: deduplicate nearly-coincident edges would corrupt the
    // group count; coincident edges arise only at zero-mass boundaries
    // — nudge them into distinct positions inside the holding bin.
    for i in 1..proposal_edges.len() {
        if proposal_edges[i] >= proposal_edges[i - 1] {
            proposal_edges[i] = proposal_edges[i - 1] * (1.0 - 1e-9);
        }
    }
    // Per-group statistics for the rationale.
    let mut shares = Vec::with_capacity(groups);
    let mut widths = Vec::with_capacity(groups);
    for g in 0..groups {
        let hi = proposal_edges[g];
        let lo = proposal_edges[g + 1];
        let u_lo = (edges.last().unwrap() / hi).ln();
        let u_hi = (edges.last().unwrap() / lo).ln();
        // Importance mass strictly inside the group.
        let mut mass = 0.0;
        for i in 0..n {
            let b_lo = (edges.last().unwrap() / edges[i + 1]).ln();
            let b_hi = (edges.last().unwrap() / edges[i]).ln();
            let ov = (u_hi.min(b_hi) - u_lo.max(b_lo)).max(0.0);
            mass += density[i] * ov;
        }
        shares.push(mass / total);
        widths.push(u_hi - u_lo);
    }
    let floor_pct = uniform_floor.clamp(0.0, 1.0) * 100.0;
    let placement = match (response, uniform_floor > 0.0) {
        (Some(_), true) => format!(
            "equal response-weighted importance mass per group (lethargy), \
             {floor_pct:.0}% uniform-lethargy floor"
        ),
        (Some(_), false) => {
            "equal response-weighted importance mass per group (lethargy)".to_string()
        }
        (None, true) => format!(
            "equal importance mass per group (lethargy), \
             {floor_pct:.0}% uniform-lethargy floor"
        ),
        (None, false) => "equal importance mass per group (lethargy)".to_string(),
    };
    Ok(BoundaryProposal {
        schema_version: BOUNDARY_PROPOSAL_SCHEMA.into(),
        energy_boundaries_ev: proposal_edges,
        placement,
        group_mass_shares: shares,
        group_lethargy_widths: widths,
        spectrum_sha256: None,
        response_sha256: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(edges: &[f64], weights: &[f64]) -> EnergyDistribution {
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: edges.to_vec(),
            bin_weights: weights.to_vec(),
        }
    }

    #[test]
    fn uniform_importance_recovers_uniform_lethargy_grid() {
        // Per-lethargy-flat spectrum (each decade carries equal mass
        // per lethargy) → edges must be lethargy-uniform.
        let edges: [f64; 7] = [1e-5, 1e-3, 1e-1, 1e1, 1e3, 1e5, 1.7e7];
        // Per-lethargy-flat: each bin's mass is its lethargy span.
        let weights: Vec<f64> = (0..6).map(|i| (edges[i + 1] / edges[i]).ln()).collect();
        let proposal = adapt_boundaries(&spec(&edges, &weights), 6, None, 0.0).unwrap();
        let b = &proposal.energy_boundaries_ev;
        assert_eq!(b.len(), 7);
        assert!((b[0] - 1.7e7).abs() < 1e-3);
        assert!((b[6] - 1e-5).abs() < 1e-7);
        let widths: Vec<f64> = (0..6).map(|g| (b[g] / b[g + 1]).ln()).collect();
        let mean = widths.iter().sum::<f64>() / 6.0;
        for w in widths {
            assert!((w - mean).abs() < 1e-6, "non-uniform lethargy width {w}");
        }
        assert!(
            proposal
                .group_mass_shares
                .iter()
                .all(|s| (*s - 1.0 / 6.0).abs() < 1e-3)
        );
    }

    #[test]
    fn concentrated_importance_refines_the_hot_region() {
        // Importance concentrated at low energy → edges densify there.
        let edges = [1e-5, 0.1, 1.0, 1e4, 1.7e7];
        let weights = [9.0, 0.5, 0.4, 0.1];
        let proposal = adapt_boundaries(&spec(&edges, &weights), 4, None, 0.0).unwrap();
        let b = &proposal.energy_boundaries_ev;
        // 3 of 4 groups should end below ~1 eV given 9/10 of the mass
        // sits in the first bin's lethargy.
        assert!(b[3] <= 0.2, "expected dense low-energy edges: {b:?}");
        assert!((b[0] - 1.7e7).abs() < 1e-3);
        assert!(b.windows(2).all(|w| w[0] > w[1]));
    }

    #[test]
    fn response_weights_shift_the_refinement() {
        let edges = [1e-5, 0.5, 1e4, 1.7e7];
        let weights = [1.0, 1.0, 1.0];
        let thermalized =
            adapt_boundaries(&spec(&edges, &weights), 4, Some(&[9.0, 1.0, 1.0]), 0.0).unwrap();
        let raw = adapt_boundaries(&spec(&edges, &weights), 4, None, 0.0).unwrap();
        // Response weighting on the thermal bin must pull more edges low.
        assert!(thermalized.group_lethargy_widths[3] < raw.group_lethargy_widths[3]);
    }

    #[test]
    fn uniform_floor_keeps_source_bands_alive() {
        // A thermal-concentrated importance (the water-column flux
        // failure mode) starves the fast band entirely without a
        // floor; with a 50% uniform floor the fast bin must keep
        // structure.
        let edges = [1e-5, 0.1, 1.0, 1e4, 1.7e7];
        let weights = [9.0, 0.5, 0.4, 0.1];
        let starved = adapt_boundaries(&spec(&edges, &weights), 4, None, 0.0).unwrap();
        let floored = adapt_boundaries(&spec(&edges, &weights), 4, None, 0.5).unwrap();
        // Edges strictly inside the epi/fast region (> 1 eV, below the
        // top boundary): the floored proposal must place some there.
        let interior = |p: &BoundaryProposal| {
            p.energy_boundaries_ev
                .iter()
                .filter(|&&e| e > 1.0 && e < 1.7e7)
                .count()
        };
        assert_eq!(interior(&starved), 0);
        assert!(interior(&floored) > 0);
        assert!(floored.placement.contains("uniform-lethargy floor"));
    }
}
