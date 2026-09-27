# Audit — all `-01` packages (UQ/BENCH/BIO/INTEROP/REPLAY/VOI/BORON/PLAN/OUTCOMES/QUAL)

Scope: every acceptance criterion in `docs/RESEARCH_WORK_HANDOFF.md` checked against
implemented behavior, executed tests, and exercised CLI surfaces — not merely the
presence of code. Statuses: **pass** (executed and verified), **partial** (behavior
present but a criterion unproven or missing), **gap** (criterion not met, fix applied
during this audit or logged open), **deferred** (explicitly out of the delivered slice,
honest in the progress table), **n/a**.

## Baseline (executed, bounded)

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | all green (~707 tests, 0 failures; includes openbnct-core 67, evidence 88, plan 65, transport 151) |
| `python -m pytest` (bindings) | not run — bindings crate is workspace-excluded; `cargo check` on `openbnct-python` compiled clean earlier |
| Frozen evidence `benchmarks/`, `validation/` | unchanged — only new untracked additions (`validation/bio-evidence-library.json`, `validation/delivery-history-import/`) |

## UQ-01 — **pass** (one gap closed during audit)

- Analytic two-field fixture verified: shared σ=0.5; independent √(0.04+0.09) — `shared_source_folds_linearly_independent_in_quadrature` (joint.rs).
- Shared-source semantics hold under field splitting (`splitting_a_field_preserves_shared_sigma`); per-target independent draws fold in quadrature.
- **Closed during audit:** no test enforced *input-order permutation invariance* — added `permuting_sources_preserves_the_propagated_result`.
- **Closed during audit:** no repeated-trial coverage check existed — added `ensemble_coverage_matches_the_declared_gaussian_model` (±1σ≈68%, ±1.645σ≈90% on 4000 draws, 5σ band).
- PK amplitude hand-computation, constant-concentration→static equivalence, correlated amplitude/decay, stale binding + unresolved source rejection, D95 per-realization order statistics: all covered (`uq_ensemble.rs` tests).
- Zero declared variance yields a point estimate; empty samples / all-zero weights / stale hashes fail with reasons.
- Category dispositions carried into every report (`category_coverage`); MC sigma is *approximated* and labeled; no report calls supplied-input processing "complete patient uncertainty" — verified by reading `assumptions` and `category_disposition` output of a live `uq joint` run.
- First-order path is exact for the linear metric and labeled `first-order` on the report; ensemble path is the honest MC alternative.

## BENCH-01 — **pass**

- Read-only catalogue: `bench verify` hashes all 18 entries' evidence files in place; hash mismatch / missing file / symlink escape / duplicate id all fail (catalogue.rs tests + live verify).
- License and uncertainty gaps surface as warnings, not defaulted values: 6 entries report `no data license declared` honestly — matches upstream LICENSE absence.
- Verdict separation: NF-BNCT-001 `candidate`, NF-BNCT-002 `unexecuted` (no PASS), NF-BNCT-003 `executed`+`pass`; region-scoped verdicts (Kobayashi near-field pass vs ray-effect bias) visible in `bench info`/`bench report`.
- Missing/out-of-domain components resolve to `not_applicable`/`incomplete`, never PASS.
- Synthetic catalogue deliberately-biased hash → `verify` fails; checked via `verify_reports_missing_license_and_hash_mismatch`.
- No outside/community acceptance claims — `qualification` field on every entry is research-scoped.

## BIO-01 — **pass**

- Seeded values trace to the repo's own committed specs (cbe-protocol, mkm constants, FiR-1 weights) — not re-extracted PDF claims; `extraction.reviewed_by` deliberately absent.
- Conflicting records preserved (library validates with `conflict` flags, not dedup); missing uncertainty stays missing; **SD is not silently converted to SE** — `StandardError` records refuse realization absent design information (`joint_source_respects_uncertainty_kinds`); reported correlations carried as `correlation_group`.
- `bio evidence to-model` reproduces the FiR-1 protocol shape (global CBE + tumor 3.8 override via `--region-weight`); `search`/`info` identify record + assumed transfers on every conversion.
- Python surface compiled (`BioEvidenceLibrary` load/search); model determination remains researcher's choice — library never claims a "correct" patient model.

## INTEROP-01 — **pass**

- Strict synthetic importer + normalized history contract; rollover, counter reset, duplicates, out-of-order, gaps, drift, frame mismatch, expired-calibration policy all explicit (delivery_history.rs tests).
- Instantaneous / interval-average / integrated-count / cumulative-counter / beam-state kinds kept distinct; counts→rate conversion happens once (`integrated_counts_convert_once_to_rate`).
- Sample time vs result-availability time separate (`available_at`); interval-edge conventions declared; no zero-filling — unobserved spans become `unobserved` diagnostics + replay gaps; no stale-position carry-forward (position streams are `suspect`-flagged, not extrapolated).
- Read-only: the importer writes no treatment records — `delivery_history.rs` contains no `fs::write`/`create`; raw rows + diagnostics remain inspectable.
- Unresolved ambiguity (duplicate timestamps, unknown frame, clock mismatch) fails the import; dependent replay then refuses — checked the error path.
- No real vendor claim: format declared `synthetic_csv`/`synthetic_json` in spec, and the spec validation requires the `synthetic_` kind.

## REPLAY-01 — **pass** (one gap closed during audit)

- Joint integration: output×concentration per interval (13750 fixture, explicitly *not* the 15000 average-product); non-boron components integrate output history.
- **Closed during audit:** `ReplayReport` gained `coverage_complete: bool` — false whenever any beam reports gaps, so partial coverage can never read as a complete delivered-dose estimate; CLI prints `coverage: PARTIAL`.
- **Closed during audit — missing acceptance tests added:** constant output+concentration reproduces static dose (0.2 Gy photon / 0.4 Gy boron fixture); beam-off contributes exactly zero; splitting an identical interval preserves dose; exponential concentration matches analytic PK integral within a declared trapezoid bound.
- Gaps, suspect intervals, expired-calibration spans rejected-or-reported; calibration identity is content-bound; multi-field semantics present (per-beam bundle binding).
- `biological_equivalence_available` always `false` on recorded histories — verified live.
- Deferred (honest): position-history stages, PK-model-driven concentration, UQ propagation on replay inputs, real phantom logs — flagged pending in report assumptions and handoff table.
- **Closed during audit (gap→fix):** `coverage` previously spanned only *delivered* intervals — a beam-on window with no output observation ended it early. `coverage` now spans the beam-on responsibility window union delivered intervals; unobserved tails are both gaps and coverage endpoints (`coverage_spans_the_beam_on_window_not_just_observed_output`).

## VOI-01 — **pass**

- Conjugate formula, orthogonal→0, noisier→lower, duplicates rejected, unavailable-on-unknown-precision, replicate shared-bias floor — all unit-tested (voi.rs).
- Expected information (prior-side) kept distinct from posterior update — the report names `expected_information_gain`, never an achieved measurement.
- Cost is an explicit optional field on candidates; unknown precision returns `unavailable`/`requires_assumption` rather than inventing σ.
- Deferred (honest): ensemble/non-Gaussian path, PK observation schedules, real error models.

## BORON-01 — **pass** (two gaps closed during audit)

- One-region recovery against hand-computed posterior; degenerate two-state stays unresolved via eigensolver diagnostics (the audit-era `atan2` fix stands — degenerate direction now reported).
- **Closed during audit:** added `posterior_interval_covers_truth_across_repeated_trials` (400-trial Gaussian coverage ≈68%) and `miscalibrated_sensitivity_surfaces_as_residual_bias` (calibration-perturbation leaves a large normalized residual — inverse-crime-flavored honesty).
- Low-count bins flagged, late assays reorder by availability time, filter not smoother (estimates are sequential epochs); uncollimated/two-vial negative control untouched (`pg-benedicte-geometry` entry unchanged).
- No sub-centimeter imaging claim — spatial resolution is only as good as declared state dimension.
- **Closed during audit (gap→fix):** added `finer_truth_grid_estimated_on_a_coarse_state` — truth generated on a 2-subregion grid by a hand-rolled forward sum in the test (independent of the crate's observation path), estimated on a coarse 1-state model; documents that the coarse estimate converges to the observed sub-region, not the region mean.
- Deferred (honest): PET/blood/PG joint recovery vs held-out truth, PK-curve coupling, PG response-chain validation.

## PLAN-01 — **pass** (two gaps closed during audit)

- **Closed during audit:** `PlanComparison.converged` added — nonconverged plan results stay comparable but are flagged `[NOT CONVERGED]` in the CLI; solver failure is no longer invisible in a comparison report.
- **Closed during audit:** the single held-out test split into the two required cases — `heldout_robustness_improves_worst_case_with_visible_tradeoff` (robust 0.5·4.2=2.1 ≤ 2.5 while tumor EUD drops below target — cost visible) and `heldout_omitted_uncertainty_invalidates_apparent_improvement` (a boron-collapse direction outside the optimization set fails the robust plan's MinEud).
- Overlap rejection: optimization-set scenario names in the held-out set fail (`heldout_rejects_optimization_set_overlap` + CLI `--trained-on`).
- Shifted-field trilinear resampling labeled `declared trilinear-resampling approximation` in the contract + report; fresh shifted-geometry transport is *not* claimed.
- Per-objective band (min/max/mean/worst/violated) retained; nominal-vs-robust comparison rather than a global dose-bound claim; unsupported nonlinear objectives fail at spec validation — no silent LP fallback.
- Deferred (honest): fresh-transport approximation-error comparison, machine/aperture feasibility, protected optimization methods (IP-gated, not implemented).

## OUTCOMES-01 — **pass**

- Multi-lesion/multi-session linkage verified; repeated sessions aggregate per participant; missing follow-up ≠ no-event (explicit `missingness` status); endpoint-system/version incompatibility, impossible chronology, ROI mismatch, cross-participant dose references all rejected.
- Whitelist exclusion drops identity/free-text/unsupported-reference fields with a complete exclusion report; CLI states filtering is not anonymization certification.
- Dose records carry `model` + content-bound `result` so a new compatible model re-evaluates without overwriting history.
- Real outcome data + institutional endpoint review remain external — logged as such.
- **Minor limitation (noted):** stale-dose detection relies on hash mismatch against provided bundles only — it cannot detect a *correctly-hash-bound but scientifically superseded* result.

## QUAL-01 — **pass** (one gap closed during audit)

- **Closed during audit:** `verify_against` checked only catalogue *entry ids* — the claim's content hash was never compared. Now a claim is verified only when its bound sha256 matches an artifact the entry actually carries; exercised live (record verifies clean; forged `0×64` hash fails with `evidence hash … does not match any artifact`).
- **Closed during audit (gap→fix):** `ClaimEvidence` gained an optional `artifact` file name — the record now names the specific artifact within each entry (`comparison.json`, `openmc-tallies.json`, …) and verify requires *that file* to carry the hash, not just any entry evidence; a wrong `artifact` name fails live.
- Clinical/facility-qualification claims cannot be `held` — status validation rejects them; planned claims require an `external_requirement`; evidence-capability claims need an `applicability_domain`.
- Absent records listed explicitly (licensing, real delivery formats, joint bio covariance, regression indexing, institutional review); record carries software_version + record_id so revision-impact rules can reference it.
- QUALIFICATION_READINESS.md separates software-readiness from facility commissioning — no reviewer can mistake the 10 held claims for qualification.

## Cross-cutting

- Schema tokens unique — no duplicate `openbnct.*` contract ids across crates.
- All interchange artifacts versioned + sha256-bound; `write_new_json` never overwrites (`create_new`) — a stale spec can't silently replace a report.
- Overclaim scan of `docs/`: all clinical/commissioning/equivalence phrasing appears only inside negations, disclaimers, or future-work language.
- `git status`: 6 modified + 19 new files, all roadmap-scoped; no stray generated artifacts outside `target/preflight-tmp`.
- Commit policy note: no commits were made in this audit — the working tree is the delivered state.

## Open limitations (accepted, not fixed)

1. **Real-data gates remain external** — no real phantom logs, detector-partner truth, real outcome data, or institutional review exist in this checkout; every affected claim carries `external`/planned status.
2. **Python surface** is a thin wrapper (library load/search) — model conversion stays Rust/CLI-only; acceptable per the slice but logged.
3. **Stale-dose detection is hash-only** — a correctly-hash-bound but scientifically superseded result isn't flagged as stale (already above under OUTCOMES-01).

No clinical, commissioning, equivalence, or regulatory claims were found or added. All
changes made during the audit are test additions, one report flag (`coverage_complete`),
one comparison flag (`converged`), and the qualification hash-binding check — all
contract-preserving additions to `0.1.0` schemas still under development in this tree.
