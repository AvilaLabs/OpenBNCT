// SPDX-License-Identifier: MIT

//! Beam-subset selection over a candidate dose-field pool — the joint
//! geometry×weights step of inverse planning. The inner weight problem
//! for each candidate subset is the certified conic solve in
//! [`lp`](crate::lp) (or the projected-gradient optimizer), so subset
//! scores are *certified objective values*, not heuristic rankings —
//! this is the geometric counterpart of the direction pre-filter in
//! [`directions`](crate::directions), evaluated on real dose fields.
//!
//! Two searches:
//!
//! * [`SelectionSearch::Exhaustive`] — every subset of size 1..=k is
//!   solved; the ranking is the global optimum over the feasible
//!   subset space. Bounded by [`MAX_EXHAUSTIVE_SOLVES`] — past that,
//!   callers are directed to the greedy path.
//! * [`SelectionSearch::Greedy`] — forward stepwise selection: start
//!   from the best single beam, add the candidate that most improves
//!   the certified score, stop at `k` or when nothing improves.
//!
//! Scoring follows the inner solver: `LpMode::Penalty` scores by the
//! certified penalized-objective value (always feasible — subsets are
//! ranked continuously); `LpMode::Strict` makes every objective bound
//! a hard constraint — infeasible subsets are marked and ranked last,
//! and among feasible ones `λ·Σw` ranks by delivered monitor-unit
//! cost. The selected subset's full certificate rides on the report.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use openbnct_core::{ContentReference, RegionMask};

use crate::lp::{LpMode, PlanCertificate, solve_qp};
use crate::optimize::{
    BeamDoseField, InversePlanObjective, OptimizeError, ResultProvenance, resolve_inputs,
};

/// Current schema token for beam-selection reports.
pub const BEAM_SELECTION_SCHEMA: &str = "openbnct.beam-selection/0.1.0";

/// Qualification string carried by every beam-selection report.
pub const BEAM_SELECTION_QUALIFICATION: &str = "beam_subset_selection_research_only_not_clinical";

/// Refuse exhaustive search past this many subset solves — beyond it
/// the combinatorial sweep stops being interactive; greedy covers it.
pub const MAX_EXHAUSTIVE_SOLVES: usize = 50_000;

/// Which subset-search strategy ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionSearch {
    /// Every subset of size 1..=max_beams solved — the reported best
    /// is the global optimum over the subset space.
    Exhaustive,
    /// Forward stepwise — each round commits the candidate that most
    /// improves the certified score; polynomial in the pool size.
    Greedy,
}

/// One evaluated beam subset with its certified score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamSelectionRow {
    /// Beam names, in the order supplied by `--dose`.
    pub beams: Vec<String>,
    /// Optimized weight per beam, same order.
    pub weights: Vec<f64>,
    /// Certified inner-solve objective — the penalized landscape value
    /// (`qp`) or `weight_regularization·Σw` among feasible subsets
    /// (`lp`). Lower is better.
    pub score: f64,
    /// `false` when the strict-LP inner solve proved the subset cannot
    /// satisfy the declared bounds (definitive — the solver's
    /// primal-infeasible status, not a convergence failure). Always
    /// `true` under the penalty solver.
    pub feasible: bool,
}

/// Versioned report of a beam-subset search: the certified winner, a
/// full ranking of evaluated subsets, and every input content-bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeamSelectionReport {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// `exhaustive` or `greedy`.
    pub search: String,
    /// Inner solver: `qp` (certified penalty optimum) or `lp_strict`
    /// (hard bounds; feasibility + min total weight).
    pub solver: String,
    pub max_beams: u32,
    /// Number of subset solves run.
    pub evaluated: u32,
    /// The winning subset.
    pub selected: BeamSelectionRow,
    /// Optimality certificate of the winning subset's inner solve —
    /// primal/dual bound and per-objective bound multipliers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<PlanCertificate>,
    /// Every evaluated subset, best-first.
    pub ranking: Vec<BeamSelectionRow>,
    /// Content bindings: objective spec + each candidate dose field.
    pub objective: ContentReference,
    pub dose_fields: Vec<ContentReference>,
    pub provenance_id: String,
    pub qualification: String,
}

/// Errors from the selection layer.
#[derive(Debug, Error)]
pub enum SelectionError {
    #[error("selection needs ≥1 candidate field and max_beams ≥ 1")]
    Degenerate,
    #[error(
        "exhaustive search would run {0} subset solves (> {MAX_EXHAUSTIVE_SOLVES}); \
             use a smaller --beams or --search greedy"
    )]
    ExhaustiveTooLarge(usize),
    #[error("inner solver: {0}")]
    Inner(#[from] OptimizeError),
}

/// `C(n, k)` saturating at `cap` — enumeration sizing.
fn binom(n: usize, k: usize, cap: usize) -> usize {
    let mut total = 0_usize;
    for j in 1..=k.min(n) {
        // C(n,j) with early saturate.
        let mut c: u128 = 1;
        for i in 0..j {
            c = c * (n - i) as u128 / (i + 1) as u128;
            if c > cap as u128 {
                return cap;
            }
        }
        total += c as usize;
        if total >= cap {
            return cap;
        }
    }
    total
}

/// Generate all `k`-subsets of `0..n` into `emit` — emit-then-advance
/// so the terminal combination is delivered too.
fn combinations(n: usize, k: usize, mut emit: impl FnMut(&[usize])) {
    if k == 0 || k > n {
        return;
    }
    let mut idx: Vec<usize> = (0..k).collect();
    loop {
        emit(&idx);
        let mut advanced = false;
        for i in (0..k).rev() {
            if idx[i] < n - (k - i) {
                idx[i] += 1;
                for j in i + 1..k {
                    idx[j] = idx[j - 1] + 1;
                }
                advanced = true;
                break;
            }
        }
        if !advanced {
            return;
        }
    }
}

/// Solve `idx` returning weights + certificate; used for the winner
/// and inside greedy rounds where the score is needed.
fn solve_subset_certified(
    fields: &[BeamDoseField],
    idx: &[usize],
    mask_voxels: &[Vec<usize>],
    spec: &InversePlanObjective,
    mode: LpMode,
) -> Result<(Vec<f64>, PlanCertificate), OptimizeError> {
    let subset: Vec<BeamDoseField> = idx.iter().map(|&i| fields[i].clone()).collect();
    solve_qp(spec, mask_voxels, &[subset], mode)
}

/// Evaluate `idx` into a scored row (infeasible → `feasible=false`,
/// score `+inf`).
fn eval_subset(
    fields: &[BeamDoseField],
    idx: &[usize],
    mask_voxels: &[Vec<usize>],
    spec: &InversePlanObjective,
    mode: LpMode,
) -> Result<BeamSelectionRow, OptimizeError> {
    match solve_subset_certified(fields, idx, mask_voxels, spec, mode) {
        Ok((w, cert)) => Ok(BeamSelectionRow {
            beams: idx.iter().map(|&i| fields[i].name.clone()).collect(),
            weights: w,
            score: cert.primal_objective,
            feasible: true,
        }),
        Err(OptimizeError::InvalidObjective(m)) if m.contains("infeasible") => {
            Ok(BeamSelectionRow {
                beams: idx.iter().map(|&i| fields[i].name.clone()).collect(),
                weights: vec![],
                score: f64::INFINITY,
                feasible: false,
            })
        }
        Err(e) => Err(e),
    }
}

/// Ranked ordering: feasible subsets first, then by ascending score.
fn row_cmp(a: &BeamSelectionRow, b: &BeamSelectionRow) -> std::cmp::Ordering {
    b.feasible
        .cmp(&a.feasible)
        .then(a.score.total_cmp(&b.score))
}

/// Run the beam-subset search. `fields` is the full candidate pool
/// (one unit-weight dose field per candidate beam); `max_beams` caps
/// subset size. `provenance.objective` binds the spec; `dose_fields`
/// bindings are assembled by the caller onto the report.
#[allow(clippy::too_many_arguments)]
pub fn select_beams(
    fields: &[BeamDoseField],
    masks: &[RegionMask],
    spec: &InversePlanObjective,
    max_beams: u32,
    mode: LpMode,
    search: SelectionSearch,
    dose_fields: Vec<ContentReference>,
    provenance: ResultProvenance,
) -> Result<BeamSelectionReport, SelectionError> {
    if fields.is_empty() || max_beams == 0 {
        return Err(SelectionError::Degenerate);
    }
    let initial = vec![0.0; fields.len()];
    let (_n_voxels, mask_voxels) = resolve_inputs(fields, masks, spec, &initial)?;
    let n = fields.len();
    let k = (max_beams as usize).min(n);
    let combos = binom(n, k, MAX_EXHAUSTIVE_SOLVES + 1);
    if search == SelectionSearch::Exhaustive && combos > MAX_EXHAUSTIVE_SOLVES {
        return Err(SelectionError::ExhaustiveTooLarge(combos));
    }

    let mut rows: Vec<BeamSelectionRow> = Vec::new();
    let mut evaluated = 0_u32;
    match search {
        SelectionSearch::Exhaustive => {
            let mut pending: Option<OptimizeError> = None;
            for size in 1..=k {
                combinations(n, size, |idx| {
                    if pending.is_some() {
                        return;
                    }
                    evaluated += 1;
                    match eval_subset(fields, idx, &mask_voxels, spec, mode) {
                        Ok(row) => rows.push(row),
                        Err(e) => pending = Some(e),
                    }
                });
                if let Some(e) = pending {
                    return Err(SelectionError::Inner(e));
                }
            }
        }
        SelectionSearch::Greedy => {
            // Forward stepwise: commit the best-improving candidate
            // each round; stop early when nothing beats the incumbent.
            let mut chosen: Vec<usize> = Vec::new();
            let mut best_score = f64::INFINITY;
            for _round in 0..k {
                let mut round_best: Option<(usize, BeamSelectionRow)> = None;
                for cand in 0..n {
                    if chosen.contains(&cand) {
                        continue;
                    }
                    let mut idx = chosen.clone();
                    idx.push(cand);
                    idx.sort_unstable();
                    let row = eval_subset(fields, &idx, &mask_voxels, spec, mode)?;
                    evaluated += 1;
                    // Improvement must clear solver noise — interior
                    // point lands within tolerance of the bound, so an
                    // equal-score candidate is not an improvement.
                    let threshold = if best_score.is_finite() {
                        best_score - 1e-9 * (1.0 + best_score.abs())
                    } else {
                        best_score
                    };
                    if row.feasible
                        && row.score < threshold
                        && round_best.as_ref().is_none_or(|(_, r)| row.score < r.score)
                    {
                        round_best = Some((cand, row));
                    }
                }
                match round_best {
                    Some((cand, row)) => {
                        best_score = row.score;
                        chosen.push(cand);
                        rows.push(row);
                    }
                    None => break, // no candidate improved the score
                }
            }
        }
    }
    rows.sort_by(row_cmp);
    let selected = rows.first().cloned().ok_or_else(|| {
        SelectionError::Inner(OptimizeError::InvalidObjective(if evaluated > 0 {
            "no feasible beam subset found — every evaluated subset \
             violated the declared bounds"
                .into()
        } else {
            "no beam subset solved".into()
        }))
    })?;
    // Re-solve the winner once for its certificate. Match by order so
    // same-named fields (identical file stems) resolve distinctly.
    let mut winner_idx: Vec<usize> = Vec::with_capacity(selected.beams.len());
    for name in &selected.beams {
        if let Some(i) = fields
            .iter()
            .enumerate()
            .find(|(i, f)| !winner_idx.contains(i) && f.name == *name)
            .map(|(i, _)| i)
        {
            winner_idx.push(i);
        }
    }
    winner_idx.sort_unstable();
    let certificate = if selected.feasible {
        solve_subset_certified(fields, &winner_idx, &mask_voxels, spec, mode)
            .ok()
            .map(|(_, c)| c)
    } else {
        None
    };
    Ok(BeamSelectionReport {
        schema_version: BEAM_SELECTION_SCHEMA.into(),
        id: provenance.id,
        case_id: spec.case_id.clone(),
        search: match search {
            SelectionSearch::Exhaustive => "exhaustive",
            SelectionSearch::Greedy => "greedy",
        }
        .into(),
        solver: match mode {
            LpMode::Penalty => "qp",
            LpMode::Strict => "lp_strict",
        }
        .into(),
        max_beams,
        evaluated,
        selected,
        certificate,
        ranking: rows,
        objective: provenance.objective,
        dose_fields,
        provenance_id: provenance.provenance_id,
        qualification: BEAM_SELECTION_QUALIFICATION.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimize::{DoseObjective, DoseQuantity, INVERSE_PLAN_OBJECTIVE_SCHEMA};

    fn spec(objectives: Vec<DoseObjective>) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
            id: "obj".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::PhysicalTotal,
            objectives,
            max_iterations: 500,
            gradient_tolerance: 1e-10,
            weight_bound: None,
            weight_regularization: 1e-3,
            bio_model: None,
            validity_domain: "unit test".into(),
            provenance_id: "test".into(),
        }
    }

    fn provenance() -> ResultProvenance {
        ResultProvenance {
            id: "sel".into(),
            provenance_id: "test".into(),
            objective: ContentReference {
                id: "obj".into(),
                sha256: "0".repeat(64),
            },
        }
    }

    fn mask(name: &str, voxels: &[bool]) -> RegionMask {
        RegionMask {
            name: name.into(),
            voxels: voxels.to_vec(),
        }
    }

    /// Four candidate beams: A covers the tumor strongly (10,5 on
    /// voxels 0,1), B covers the organ (voxels 2,3), C covers both
    /// weakly, D is pure waste (dose only in air). The optimal pair
    /// for a tumor-floor objective is {A} alone or {A,anything}; the
    /// selection must identify A and rank D-containing subsets badly.
    fn pool() -> (Vec<BeamDoseField>, Vec<RegionMask>) {
        let fields = vec![
            BeamDoseField {
                name: "A".into(),
                values: vec![10.0, 5.0, 0.0, 0.0],
                components: None,
            },
            BeamDoseField {
                name: "B".into(),
                values: vec![0.0, 0.0, 10.0, 10.0],
                components: None,
            },
            BeamDoseField {
                name: "C".into(),
                values: vec![3.0, 1.0, 1.0, 1.0],
                components: None,
            },
            BeamDoseField {
                name: "D".into(),
                values: vec![0.0, 0.0, 0.0, 1.0],
                components: None,
            },
        ];
        let masks = vec![
            mask("tumor", &[true, true, false, false]),
            mask("oar", &[false, false, true, true]),
        ];
        (fields, masks)
    }

    #[test]
    fn exhaustive_selection_finds_the_certified_best_subset() {
        let (fields, masks) = pool();
        // Tumor floor ≥ 10 needs 5·w ≥ 10 → beam A alone satisfies at
        // w=2; adding B only costs weight; C would add OAR dose.
        let spec = spec(vec![
            DoseObjective::MinDoseAtVolume {
                mask: "tumor".into(),
                volume_fraction: 1.0,
                target: 10.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "oar".into(),
                limit: 50.0,
                weight: 1.0,
            },
        ]);
        let report = select_beams(
            &fields,
            &masks,
            &spec,
            2,
            LpMode::Strict,
            SelectionSearch::Exhaustive,
            vec![],
            provenance(),
        )
        .unwrap();
        // 4·C(4,1) + C(4,2) = 10 subsets evaluated.
        assert_eq!(report.evaluated, 10);
        // Any optimal subset contains A — only it can satisfy the
        // floor — and ties are legitimate under the λ·Σw score.
        assert!(report.selected.beams.contains(&"A".to_string()));
        let w_a =
            report.selected.weights[report.selected.beams.iter().position(|b| b == "A").unwrap()];
        assert!((w_a - 2.0).abs() < 1e-4);
        assert!(report.selected.feasible);
        assert!(report.certificate.is_some());
        // Strict-mode score (λ·Σw) is blind to dead beams — {A,D}
        // ties {A,B} since the useless beam just takes w = 0. The
        // *selection* is what matters: plain {A} wins outright.
        let ab = report
            .ranking
            .iter()
            .find(|r| r.beams == ["A", "B"])
            .unwrap();
        let ad = report
            .ranking
            .iter()
            .find(|r| r.beams == ["A", "D"])
            .unwrap();
        assert!((ad.score - ab.score).abs() < 1e-6);
    }

    #[test]
    fn strict_mode_marks_infeasible_subsets_definitively() {
        let (fields, masks) = pool();
        // Tumor floor 60: 5·w_A ≥ 60 → w_A = 12 feasible; the whole-
        // phantom mean cap 12 → mean_A = 3.75·w_A = 45 > 12 kills {A}
        // alone and any subset — only B∪A style combos could pass, but
        // mean includes B too. All subsets infeasible.
        let spec = spec(vec![
            DoseObjective::MinDoseAtVolume {
                mask: "tumor".into(),
                volume_fraction: 1.0,
                target: 60.0,
                weight: 1.0,
            },
            DoseObjective::MaxMean {
                mask: "oar".into(),
                limit: 1.0,
                weight: 1.0,
            },
        ]);
        let report = select_beams(
            &fields,
            &masks,
            &spec,
            1,
            LpMode::Strict,
            SelectionSearch::Exhaustive,
            vec![],
            provenance(),
        )
        .unwrap();
        // Single beams: A feasible alone? 5·w_A ≥ 60 → w_A = 12, oar
        // mean 0 ≤ 1 → feasible! C can't reach (3·w ≥ 60 → w=20, adds
        // OAR 1·20 mean = 20 > 1). D,B can't cover tumor.
        let a = report.ranking.iter().find(|r| r.beams == ["A"]).unwrap();
        assert!(a.feasible);
        assert!(
            !report
                .ranking
                .iter()
                .find(|r| r.beams == ["D"])
                .unwrap()
                .feasible
        );
    }

    #[test]
    fn greedy_recovers_the_exhaustive_winner_on_a_separable_pool() {
        let (fields, masks) = pool();
        let spec = spec(vec![DoseObjective::MinDoseAtVolume {
            mask: "tumor".into(),
            volume_fraction: 1.0,
            target: 10.0,
            weight: 1.0,
        }]);
        let report = select_beams(
            &fields,
            &masks,
            &spec,
            2,
            LpMode::Strict,
            SelectionSearch::Greedy,
            vec![],
            provenance(),
        )
        .unwrap();
        // Greedy commits A first (only beam that can satisfy the floor)
        // then stops — nothing improves on w_A = 2, λΣw optimal.
        assert_eq!(report.selected.beams, vec!["A"]);
        assert!((report.selected.weights[0] - 2.0).abs() < 1e-4);
    }

    #[test]
    fn binom_and_combinations_are_exact() {
        assert_eq!(binom(4, 2, usize::MAX), 4 + 6); // sizes 1..=2
        assert_eq!(binom(10, 3, usize::MAX), 10 + 45 + 120);
        let mut seen = Vec::new();
        combinations(4, 2, |idx| seen.push(idx.to_vec()));
        assert_eq!(seen.len(), 6);
        // Strictly increasing, covers every pair once.
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 6);
    }

    #[test]
    fn exhaustive_cap_redirects_to_greedy() {
        let fields: Vec<BeamDoseField> = (0..30)
            .map(|i| BeamDoseField {
                name: format!("b{i}"),
                values: vec![1.0; 4],
                components: None,
            })
            .collect();
        let masks = vec![mask("tumor", &[true; 4])];
        let spec = spec(vec![DoseObjective::MinDoseAtVolume {
            mask: "tumor".into(),
            volume_fraction: 1.0,
            target: 10.0,
            weight: 1.0,
        }]);
        // C(30,1..8) ≈ 5.8M > cap → redirected.
        match select_beams(
            &fields,
            &masks,
            &spec,
            8,
            LpMode::Strict,
            SelectionSearch::Exhaustive,
            vec![],
            provenance(),
        ) {
            Err(SelectionError::ExhaustiveTooLarge(n)) => {
                assert!(n >= MAX_EXHAUSTIVE_SOLVES)
            }
            other => panic!("expected ExhaustiveTooLarge, got {other:?}"),
        }
    }
}
