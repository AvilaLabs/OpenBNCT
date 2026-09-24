# Scenario-robust planning — known-answer fixture

A deliberately degenerate four-voxel fixture where the worst-case
optimizer's optimum is computable in closed form. Synthetic data only;
no patient or measured content. Research-grade — asserts optimizer
correctness, not physics.

## Construction

- Grid: 2×2×1 voxels of 10 mm (`shape [2,2,1]`), shared across both
  beam fields; one mask `all` covering every voxel.
- `field-h.json`: hydrogen component 1.0 Gy everywhere, all other
  components 0; physical total 1.0. The scenario-immune beam.
- `field-b.json`: boron component 1.0 Gy everywhere; physical total 1.0.
  The nominally-equivalent beam that the scenario kills.
- `objective.json`: `min_eud` on `all`, target 1.5, a = 1 (EUD = mean),
  weight 1.0, `weight_regularization` 1e-3, `physical_total` quantity.
- `scenario-set.json`: one scenario `boron-zero` scaling the boron
  component by 0.0 — the carrier-failure case.

## Analytic optimum

For any weights `w_h, w_b ≥ 0` the boron-zero scenario delivers
`w_h·1.0` while the nominal delivers `w_h + w_b` — the scenario is the
worst case everywhere, so the robust problem reduces to minimizing
`(max(0, 1.5 − w_h)/1.5)² + 1e-3·(w_h + w_b)` over `w_h, w_b ≥ 0`.

`w_b*` is exactly 0: it costs regularization and buys nothing under
the worst case. For `w_h`, stationarity at the kink gives
`w_h* = 1.5 − reg·bound²/(2·weight) = 1.5 − 1e-3·2.25/2 = 1.498875`
with penalty `1e-3·w_h* + (1.125e-3/1.5)² ≈ 1.4994e-3`.

The nominal-only solver's optimum differs qualitatively — any
`w_h + w_b = 1.498875`-ish split is feasible, and the regularization
is indifferent between the beams — so the fixture discriminates the
robust method rather than repeating the nominal one.

## Acceptance gates

- `plan optimize --scenario-set` returns `w_h ∈ [1.498, 1.500]`,
  `w_b < 1e-2`, `method: "worst_case_scenario"`, `converged: true`.
- `plan scenarios` on the result reports the identical achieved EUD
  under `nominal` and `boron-zero` — the robust plan is
  scenario-invariant by construction here.

## Reproduction

`reproduce.sh` regenerates `result.json` and `scenario-report.json`
with the repository's `openbnct` binary. Committed outputs are frozen
evidence — regenerate only to verify, not to update.

## License and scope

Synthetic, generated for this repository; same license as the
workbench. Asserts optimizer mechanics only — no physics, no clinical
meaning.
