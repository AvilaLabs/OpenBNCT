// SPDX-License-Identifier: MIT

//! Adjoint-guided beam synthesis: the marginal-utility adjoint source.
//!
//! The `plan directions` sweep ranks candidate beams by *fluence* —
//! a uniform unit source in the aim region measures only how much
//! flux a direction lands there. Dose objectives are richer: a beam
//! that carries the fluence through an organ-at-risk first is worth
//! less than one that avoids it, and the dose-response fold rewards
//! some energies far more than others. Both effects enter through
//! the adjoint *source*, not the scorer:
//!
//! For an objective `o` on mask `M_o` with dose metric `m_o`, the
//! marginal utility of a unit of forward flux at voxel `v`, group `g`
//! is
//!
//! ```text
//! s_o(v, g) = sign_o · w_o · (d m_o / d d_v) · R_o(v, g)
//! ```
//!
//! where `R_o` is the response that folds flux into the objective's
//! `dose_quantity` (the component sum for `physical_total`, the single
//! component response for `component`, `Σ_c w_c·R_c` under
//! `isoeffective` with the mask's `region_weights`), and `sign` is
//! `+1` for coverage (`min_*`, `maximin`) and `−1` for sparing
//! (`max_*`) objectives. Summed over objectives this signed composite
//! source makes the adjoint φ* a marginal-utility field: a candidate
//! direction's `<φ_uncollided, φ*>` score ranks beams by how much
//! they would *improve the plan's objectives* — directions that
//! transit an organ mask score against the negative source there and
//! are demoted without a separate constraint pass.
//!
//! `d m_o / d d_v` is the gradient [`objective_metric`] already
//! returns. When a current dose field is supplied the source is the
//! true marginal-utility gradient at that plan (EUD gradients weight
//! underdosed mask voxels; quantile objectives weight the boundary
//! voxel) — the engine of an iterate-and-resynthesize loop. Without a
//! dose field the gradient falls back to `1/|M_o|` uniform over the
//! mask: equal pull per mask voxel, still response-weighted in group.
//!
//! The composite source is *signed* — solvers must not force it
//! non-negative (the θ-positivity repair is designed for unsigned
//! forward physics; callers should disable it for the adjoint solve).

use std::collections::BTreeMap;

use openbnct_core::{DoseComponent, RegionMask};
use thiserror::Error;

use crate::optimize::{
    DoseObjective, DoseQuantity, InversePlanObjective, component_name, objective_metric,
};

/// Errors from composite-source construction.
#[derive(Debug, Error)]
pub enum SynthesisError {
    #[error("effective response row {0} has the wrong length")]
    ResponseLength(usize),
    #[error("objective response table has {0} rows, expected {1}")]
    ResponseShape(usize, usize),
    #[error("current dose has {0} voxels, expected {1}")]
    DoseLength(usize, usize),
    #[error("objective {0} has no mask voxels")]
    EmptyMask(usize),
    #[error("beam field {0:?} lacks component-resolved doses required by isoeffective")]
    MissingComponents(String),
}

/// The sign an objective applies to its marginal utility: coverage
/// objectives reward dose, sparing objectives penalize it.
fn objective_sign(objective: &DoseObjective) -> f64 {
    match objective {
        DoseObjective::MinEud { .. }
        | DoseObjective::MinDoseAtVolume { .. }
        | DoseObjective::Maximin { .. } => 1.0,
        DoseObjective::MaxMean { .. } | DoseObjective::MaxDoseAtVolume { .. } => -1.0,
    }
}

fn objective_weight(objective: &DoseObjective) -> f64 {
    match objective {
        DoseObjective::MinEud { weight, .. }
        | DoseObjective::MaxMean { weight, .. }
        | DoseObjective::MinDoseAtVolume { weight, .. }
        | DoseObjective::MaxDoseAtVolume { weight, .. }
        | DoseObjective::Maximin { weight, .. } => *weight,
    }
}

/// The per-objective marginal-utility adjoint source `s_o(v, g)` —
/// exported for diagnostics; the composite solver input is the sum
/// over objectives.
///
/// `effective_response[cell][group]` is the response that folds flux
/// into *this* objective's dose quantity at each voxel — the caller
/// (which owns the material map) builds it; see [`effective_response`]
/// for the convention. `mask_voxels` are the objective's selected
/// cells; `dose` is the objective's current dose view when iterating
/// around an existing plan (`None` = uniform-mask fresh mode).
pub fn objective_adjoint_source(
    objective: &DoseObjective,
    mask_voxels: &[usize],
    effective_response: &[Vec<f64>],
    dose: Option<&[f64]>,
) -> Result<Vec<Vec<f64>>, SynthesisError> {
    if mask_voxels.is_empty() {
        return Err(SynthesisError::EmptyMask(0));
    }
    let n_cells = effective_response.len();
    let groups = effective_response.first().map_or(0, Vec::len);
    if effective_response.iter().any(|r| r.len() != groups) {
        let bad = effective_response
            .iter()
            .position(|r| r.len() != groups)
            .unwrap_or(0);
        return Err(SynthesisError::ResponseLength(bad));
    }
    let gradient = match dose {
        Some(d) => {
            if d.len() != n_cells {
                return Err(SynthesisError::DoseLength(d.len(), n_cells));
            }
            let (_metric, g) = objective_metric(objective, d, mask_voxels);
            g
        }
        None => {
            // Fresh mode: uniform pull per mask voxel.
            let n = mask_voxels.len() as f64;
            let mut g = vec![0.0; n_cells];
            for &v in mask_voxels {
                g[v] = 1.0 / n;
            }
            g
        }
    };
    let scale = objective_sign(objective) * objective_weight(objective);
    let mut source = vec![vec![0.0; groups]; n_cells];
    for &v in mask_voxels {
        let pull = scale * gradient[v];
        if pull == 0.0 {
            continue;
        }
        for (s, &r) in source[v].iter_mut().zip(&effective_response[v]) {
            *s += pull * r;
        }
    }
    Ok(source)
}

/// Sum the per-objective adjoint sources into the signed composite
/// the adjoint solver consumes: `Σ_o sign_o·w_o·(dm_o/dd)·R_o`.
///
/// `effective_responses[o][cell][group]` parallels `spec.objectives`
/// and `mask_voxels[o]` parallels them too — both resolved by the
/// caller. `doses` (optional) parallels `spec.objectives` with each
/// objective's current dose view — pass the same dose array for every
/// objective under `physical_total`/`component` quantities and the
/// per-mask effective dose under `isoeffective`.
pub fn composite_adjoint_source(
    spec: &InversePlanObjective,
    mask_voxels: &[Vec<usize>],
    effective_responses: &[Vec<Vec<f64>>],
    doses: Option<&[Vec<f64>]>,
) -> Result<Vec<Vec<f64>>, SynthesisError> {
    if effective_responses.len() != spec.objectives.len() {
        return Err(SynthesisError::ResponseShape(
            effective_responses.len(),
            spec.objectives.len(),
        ));
    }
    let n_cells = effective_responses
        .first()
        .and_then(|r| r.first().map(|_| r.len()))
        .unwrap_or(0);
    let groups = effective_responses
        .first()
        .and_then(|r| r.first())
        .map_or(0, Vec::len);
    let mut composite = vec![vec![0.0; groups]; n_cells];
    for (index, objective) in spec.objectives.iter().enumerate() {
        let dose = doses.map(|d| d[index].as_slice());
        let source = objective_adjoint_source(
            objective,
            &mask_voxels[index],
            &effective_responses[index],
            dose,
        )
        .map_err(|error| match error {
            SynthesisError::EmptyMask(_) => SynthesisError::EmptyMask(index),
            other => other,
        })?;
        for (acc, row) in composite.iter_mut().zip(&source) {
            for (a, &s) in acc.iter_mut().zip(row) {
                *a += s;
            }
        }
    }
    Ok(composite)
}

/// The effective group-resolved response one objective folds flux
/// with at cell `cell`: `d_v = Σ_g R_o(v, g)·φ_vg`.
///
/// `component_response[cell]` yields the material's
/// `dose_response_gy_cm2` map — the caller resolves the cell-material
/// index once and passes a closure. Region `weights` for isoeffective
/// come from the objective's mask via the spec's embedded model.
pub fn effective_response(
    spec: &InversePlanObjective,
    objective: &DoseObjective,
    cell: usize,
    component_response: &dyn Fn(usize) -> BTreeMap<String, Vec<f64>>,
    groups: usize,
) -> Vec<f64> {
    let table = component_response(cell);
    match spec.dose_quantity {
        DoseQuantity::PhysicalTotal => {
            // The bundle's physical total sums the declared
            // components — the response does too.
            let mut r = vec![0.0; groups];
            for values in table.values() {
                for (g, &v) in values.iter().enumerate() {
                    if g < groups {
                        r[g] += v;
                    }
                }
            }
            r
        }
        DoseQuantity::Component(component) => table
            .get(component_name(component))
            .cloned()
            .unwrap_or_else(|| vec![0.0; groups]),
        DoseQuantity::Isoeffective => {
            let model = spec.bio_model.as_ref().expect("validated");
            let mask = crate::optimize::objective_mask(objective);
            let weights = model
                .region_weights
                .get(mask)
                .unwrap_or(&model.component_weights);
            let mut r = vec![0.0; groups];
            for (name, &w) in weights {
                if let Some(values) = table.get(name) {
                    for (g, &v) in values.iter().enumerate() {
                        if g < groups {
                            r[g] += w * v;
                        }
                    }
                }
            }
            r
        }
    }
}

/// Resolve the per-objective mask voxel lists the composite builder
/// consumes — the same mapping `resolve_inputs` performs, without the
/// field plumbing the weight optimizer needs.
pub fn synthesis_masks(
    spec: &InversePlanObjective,
    masks: &[RegionMask],
    n_voxels: usize,
) -> Result<Vec<Vec<usize>>, crate::optimize::OptimizeError> {
    spec.objectives
        .iter()
        .map(|objective| {
            let name = crate::optimize::objective_mask(objective);
            let mask = masks
                .iter()
                .find(|m| m.name == name)
                .ok_or_else(|| crate::optimize::OptimizeError::UnknownMask(name.to_owned()))?;
            let voxels: Vec<usize> = mask
                .voxels
                .iter()
                .enumerate()
                .filter_map(|(i, &on)| if on && i < n_voxels { Some(i) } else { None })
                .collect();
            if voxels.is_empty() {
                return Err(crate::optimize::OptimizeError::EmptyMask(name.to_owned()));
            }
            Ok(voxels)
        })
        .collect()
}

/// Accumulate `Σ_i w_i·D_i` under one objective's dose view — the
/// per-objective dose the marginal-utility gradient needs when
/// iterating around an existing plan. `physical_total`/`component`
/// share one accumulated array across objectives; `isoeffective`
/// folds each beam's components by the objective mask's weights, so
/// the caller asks for one view per objective.
pub fn objective_dose_view(
    spec: &InversePlanObjective,
    objective: &DoseObjective,
    fields: &[crate::optimize::BeamDoseField],
    weights: &[f64],
) -> Result<Vec<f64>, SynthesisError> {
    let n = fields
        .first()
        .map(|f| {
            if f.values.is_empty() {
                f.components
                    .as_ref()
                    .and_then(|c| c.values().next())
                    .map_or(0, Vec::len)
            } else {
                f.values.len()
            }
        })
        .unwrap_or(0);
    let mut dose = vec![0.0; n];
    if spec.dose_quantity != DoseQuantity::Isoeffective {
        for (field, &w) in fields.iter().zip(weights) {
            for (d, &v) in dose.iter_mut().zip(&field.values) {
                *d += w * v;
            }
        }
        return Ok(dose);
    }
    let model = spec.bio_model.as_ref().expect("validated");
    let mask = crate::optimize::objective_mask(objective);
    let weights_map = model
        .region_weights
        .get(mask)
        .unwrap_or(&model.component_weights);
    for (field, &wi) in fields.iter().zip(weights) {
        let comps = field
            .components
            .as_ref()
            .ok_or_else(|| SynthesisError::MissingComponents(field.name.clone()))?;
        for (name, &wc) in weights_map {
            if let Some(values) = comps.get(name) {
                for (d, &v) in dose.iter_mut().zip(values) {
                    *d += wi * wc * v;
                }
            }
        }
    }
    Ok(dose)
}

/// Whether the composite source carries any negative entries — the
/// signal that the adjoint solve must run with signed-source handling
/// (θ-positivity repair off).
pub fn source_is_signed(source: &[Vec<f64>]) -> bool {
    source.iter().flatten().any(|&s| s < 0.0)
}

/// The four required component names in declaration order — used by
/// callers that build `component_response` closures over materials.
pub fn required_component_names() -> Vec<&'static str> {
    DoseComponent::REQUIRED
        .iter()
        .map(|c| component_name(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::INVERSE_PLAN_OBJECTIVE_SCHEMA;
    use openbnct_bio::BiologicalModel;

    fn spec(objectives: Vec<DoseObjective>) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
            id: "synth".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::PhysicalTotal,
            objectives,
            max_iterations: 100,
            gradient_tolerance: 1e-10,
            weight_bound: Some(10.0),
            weight_regularization: 0.0,
            bio_model: None,
            validity_domain: "test".into(),
            provenance_id: "prov".into(),
        }
    }

    fn mask(voxels: &[usize], n: usize) -> (RegionMask, Vec<usize>) {
        let mut on = vec![false; n];
        for &v in voxels {
            on[v] = true;
        }
        (
            RegionMask {
                name: "m".into(),
                voxels: on,
            },
            voxels.to_vec(),
        )
    }

    #[test]
    fn coverage_source_is_positive_and_response_weighted() {
        let (_m, voxels) = mask(&[3, 7], 16);
        // Two groups, response [1.0, 0.5] — the source must carry the
        // response shape exactly: pull·R_g per voxel.
        let response = vec![vec![1.0, 0.5]; 16];
        let s = objective_adjoint_source(
            &DoseObjective::Maximin {
                mask: "m".into(),
                target: 1.0,
                weight: 2.0,
            },
            &voxels,
            &response,
            None,
        )
        .unwrap();
        assert!((s[3][0] - 1.0).abs() < 1e-12); // 2.0 * (1/2) * 1.0
        assert!((s[3][1] - 0.5).abs() < 1e-12); // 2.0 * (1/2) * 0.5
        assert_eq!(s[0], vec![0.0, 0.0]); // off-mask voxel silent
        assert!(!source_is_signed(&s));
    }

    #[test]
    fn sparing_source_is_negative() {
        let (_m, voxels) = mask(&[2], 8);
        let response = vec![vec![0.7, 0.3]; 8];
        let s = objective_adjoint_source(
            &DoseObjective::MaxMean {
                mask: "m".into(),
                limit: 1.0,
                weight: 5.0,
            },
            &voxels,
            &response,
            None,
        )
        .unwrap();
        assert!(s[2][0] < 0.0 && s[2][1] < 0.0);
        assert!(source_is_signed(&s));
    }

    #[test]
    fn marginal_gradient_concentrates_on_the_cold_voxel() {
        // EUD(a=2) gradient ∝ d_v — with dose [hot, cold] the cold
        // voxel carries nearly all of it. Fresh mode would weight them
        // equally; marginal mode must not.
        let voxels = vec![4, 5];
        let response = vec![vec![1.0]; 8];
        let dose = vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.01, 0.0, 0.0];
        let s = objective_adjoint_source(
            &DoseObjective::MinEud {
                mask: "m".into(),
                target: 2.0,
                eud_a: 2.0,
                weight: 1.0,
            },
            &voxels,
            &response,
            Some(&dose),
        )
        .unwrap();
        assert!(
            s[4][0] > 10.0 * s[5][0],
            "EUD gradient should concentrate on the cold voxel: {:?} vs {:?}",
            s[4],
            s[5]
        );
    }

    #[test]
    fn composite_sums_signed_sources() {
        let s1 = spec(vec![
            DoseObjective::Maximin {
                mask: "t".into(),
                target: 1.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "o".into(),
                limit: 0.5,
                weight: 4.0,
            },
        ]);
        let mut t_on = vec![false; 8];
        t_on[1] = true;
        let mut o_on = vec![false; 8];
        o_on[6] = true;
        let masks = vec![
            RegionMask {
                name: "t".into(),
                voxels: t_on,
            },
            RegionMask {
                name: "o".into(),
                voxels: o_on,
            },
        ];
        let mv = synthesis_masks(&s1, &masks, 8).unwrap();
        let responses = vec![vec![vec![1.0]; 8]; 2];
        let c = composite_adjoint_source(&s1, &mv, &responses, None).unwrap();
        assert!(c[1][0] > 0.0, "target voxel pulls positive");
        assert!(c[6][0] < 0.0, "OAR voxel pushes negative");
        assert_eq!(
            c[1][0].abs() * 4.0,
            c[6][0].abs(),
            "weights scale the pulls: |OAR| = 4·|target|"
        );
        assert!(source_is_signed(&c));
    }

    #[test]
    fn isoeffective_response_applies_mask_weights() {
        let model = BiologicalModel {
            schema_version: "openbnct.biological-model/0.1.0".into(),
            id: "iso".into(),
            weight_semantics: openbnct_bio::WeightSemantics::FixedPerComponent,
            input_unit: openbnct_core::DoseUnit::GrayPerSourceParticle,
            component_weights: BTreeMap::from([
                ("boron".to_string(), 5.0),
                ("nitrogen".to_string(), 1.0),
            ]),
            region_weights: BTreeMap::from([(
                "o".to_string(),
                BTreeMap::from([("boron".to_string(), 1.0), ("nitrogen".to_string(), 1.0)]),
            )]),
            region_priority: Vec::new(),
            component_weight_uncertainty: BTreeMap::new(),
            derivation: None,
            validity_domain: None,
            fractionation: None,
        };
        let mut s = spec(vec![DoseObjective::MaxMean {
            mask: "o".into(),
            limit: 1.0,
            weight: 1.0,
        }]);
        s.dose_quantity = DoseQuantity::Isoeffective;
        s.bio_model = Some(model);
        let response_table = BTreeMap::from([
            ("boron".to_string(), vec![2.0, 3.0]),
            ("nitrogen".to_string(), vec![4.0, 5.0]),
        ]);
        let table = &response_table;
        let r = effective_response(&s, &s.objectives[0], 0, &move |_| table.clone(), 2);
        // Region override (boron 1.0, nitrogen 1.0), not the global
        // 5.0/1.0: r = 1·[2,3] + 1·[4,5] = [6,8].
        assert_eq!(r, vec![6.0, 8.0]);
    }
}
