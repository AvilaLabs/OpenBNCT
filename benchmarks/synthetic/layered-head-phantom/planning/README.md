# Isoeffective multi-field planning study — layered-head phantom

The first committed end-to-end exercise of the inverse-planning chain:
beam aiming → per-beam deterministic transport → component dose fold →
isoeffective weight optimization → exposure-plan emission. Research
demonstration on a declared synthetic phantom — not a clinical plan.

## Setup

- Phantom: `openbnct.layered-head-phantom.v1` (25³ voxels, 8 mm, ICRU-44
  skin/skull/brain shells, trace ¹⁰B in brain).
- Transport: 28-group S8 multigroup solve per beam
  (`multigroup-data-28g.json`, `--anderson 5`), uncollided split and
  transport correction active; every beam converged in 4 outers
  (residual ≤ 8.9e-5).
- Aiming: `plan fields` centers an on-face `UniformDisk`
  (`--radius-cm 2.4`) per beam on the line from the grid face through
  the tumor-mask centroid.
- Masks (generated, in this directory): `tumor` — r ≤ 2.4 cm sphere at
  (0, 3, 10) cm inside the brain (112 voxels); `brain-oar` — brain
  minus tumor (4664); `shell-oar` — skin+skull shell 8.4 < r ≤ 10 cm
  (3368).

## Fields

| Beam | Direction | Entry face | Tumor eff. mean | Brain eff. mean | Shell eff. max |
|---|---|---|---|---|---|
| `ap` | (0,0,+1) | z-low | 2.364e-8 | 3.915e-9 | 1.283e-6 |
| `lat` | (+1,0,0) | x-low | 2.364e-8 | 3.915e-9 | 1.283e-6 |
| `obl` | (1,0,+1) | x-low (corner-edge aim) | 1.402e-8 | 1.866e-9 | 1.627e-6 |

Effective dose = Σ_c w_c·D_c under the embedded `BiologicalModel`
(TECDOC-convention `photon_isoeffective`: N/H RBE 3.2, photon 1.0,
B 1.0 default / 3.8 in `tumor`).

Findings: `ap`/`lat` are statistically interchangeable — the
concentric-shell phantom is symmetric enough that perpendicular beams
give identical mask statistics. `obl` is strictly worse: its disk
aims at a face corner-edge so part of the aperture emits outside the
grid (reduced coupled strength, longer chord) — 59% of the tumor dose,
48% of the brain dose, but a 27% hotter shell spot.

## Optimization

`objective-isoeffective.json` — `dose_quantity: "isoeffective"`,
objectives:

- `min_eud` tumor (a=1) ≥ 2.4e-8 Gy·src⁻¹
- `max_mean` brain-oar ≤ 4.5e-9
- `max_dose_at_volume` shell-oar (hottest 1%) ≤ 1.35e-6

`weight_bound` 4, `weight_regularization` 1e-3 (bound-normalized
penalty — scale-free), `gradient_tolerance` 1e-8.

**Result** (`result-isoeffective.json`, converged, 1200 iterations):

| Beam | Weight |
|---|---|
| ap | 0.8747 |
| lat | 0.1400 |
| obl | **0** |

- tumor EUD 2.39878e-8 vs bound 2.4e-8 — reported `VIOLATED` by 0.05%
- brain mean 3.97e-9 ✓, shell D1% 9.9e-8 ✓
- Σw = 1.015 ≈ the analytic minimum-weight vertex
  (2.4e-8 / 2.364e-8 = 1.0154) with the least-efficient beam off.

The hair-thin tumor shortfall is the regularization equilibrium, not a
convergence failure: with a linear pull λΣw the soft-penalty optimum
sits at `violation ≈ λ·bound²/(2·slope)` inside the bound — here
≈ 5e-4 relative. Raising the objective's `weight` shrinks the
equilibrium; λ = 0 would land exactly on the bound but loses the
minimum-weight selection that turns the useless beam off.

`exposure-plan.json` — the emitted `openbnct.exposure-plan` binding the
weights to their hashed dose bundles (`source_strength_scaling` basis);
validates under `plan validate` and is consumable by `plan export`
and the GUI Plan workspace.

## Reproduce

```text
openbnct plan fields \
  --case case.json --data multigroup-data-28g.json \
  --assignment assignment.json --aim-mask planning/tumor-mask.json \
  --beam ap,0,0,1 --beam lat,1,0,0 --beam obl,1,0,1 \
  --radius-cm 2.4 --order 8 --anderson 5 \
  --output-dir planning/fields
openbnct plan optimize \
  --objective planning/objective-isoeffective.json \
  --dose planning/fields/ap.dose.json --dose planning/fields/lat.dose.json \
  --dose planning/fields/obl.dose.json \
  --mask planning/tumor-mask.json --mask planning/brain-oar-mask.json \
  --mask planning/shell-oar-mask.json \
  --emit-plan planning/exposure-plan.json \
  --output planning/result-isoeffective.json
```

## Scope

Declared research demonstration. The phantom boron loading is dilute
(1e-7 mass fraction) so component weights act as a convention, not a
treatment-representative uptake model; no clinical claim.
