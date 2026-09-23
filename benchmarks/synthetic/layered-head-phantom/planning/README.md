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

## Beam-direction search (`fields6/` + `result-direction-search.json`)

A six-direction candidate set — `ap`, `pa`, `sup`, `inf`, `lat`, `obl`
— swept by `plan fields` and optimized under the same isoeffective
objective (`objective-direction-search.json`, `max_iterations` 6000).
The phantom's off-center tumor (y = +3 cm) makes the candidates
genuinely different: `sup` enters the +y face 7.4 cm from the tumor,
`inf` crosses 12.6 cm from −y.

Per-beam unit-weight isoeffective dose (tumor mask boron weight 3.8):

| Beam | Tumor EUD | Brain mean | Shell D1% |
|---|---|---|---|
| sup | 3.4765e-8 | 4.50e-9 | 1.72e-7 |
| ap | 2.3641e-8 | 3.92e-9 | 4.62e-7 |
| lat | 2.3641e-8 | 3.92e-9 | 4.62e-7 |
| pa | 2.0785e-8 | 3.69e-9 | 4.10e-7 |
| inf | 1.5224e-8 | 5.34e-9 | 2.02e-7 |
| obl | 1.4015e-8 | 1.87e-9 | 4.81e-7 |

Result (converged, 4788 iterations): **sup alone**, weight 0.6901 —
the analytic vertex `bound / sup_EUD = 2.4e-8 / 3.4765e-8 = 0.6904`.
Every other candidate drains to exactly 0; tumor EUD lands 0.03% inside
the bound at the regularization equilibrium, and the emitted
`exposure-plan-direction-search.json` validates under `plan validate`.

## Plan robustness (`robustness-direction-search.json`)

`plan robustness` propagates declared systematic σ on each beam's
component dose through the optimized weights into per-objective metric
1σ and a Gaussian violation probability. The committed report declares
boron = 10%, photon = 5%, positioning = 1 mm — illustrative sources,
not a facility uncertainty budget.

| Objective | Achieved | Bound | 1σ | P(violate) |
|---|---|---|---|---|
| tumor EUD ≥ | 2.3992e-8 | 2.4e-8 | 7.1e-10 | **0.50** |
| brain mean ≤ | 3.11e-9 | 4.5e-9 | 2.4e-10 | ~0 |
| shell D1% ≤ | 6.99e-8 | 1.35e-6 | 5.1e-8 | ~0 |

The plan lands ~1σ inside the tumor bound by design (the regularization
equilibrium), so under 10% boron σ it is nominal-marginal — P ≈ 0.5 —
while both OAR bounds hold with margin. The propagation is first-order
and fully-correlated across voxels; sources declared on multiple beams
are independent (a conservative documented convention).

## OpenPINT endpoint reproduction (`result-openpint-endpoint.json`)

Cross-implementation check of the limiting-OAR irradiation-time endpoint
from the OpenPINT methods paper (arXiv:2606.21476, eq. 4): on the
biologically weighted dose map, `t = min(13/Ḋ2, 2.5/Ḋ50)` — brain D2 ≤ 13
Gy-w and D50 ≤ 2.5 Gy-w. `irradiation-time` accepts the same endpoints as
`dN` limits (`brain-oar=d2:13`, `brain-oar=d50:2.5`).

Inputs: `biological-dose-fixed-weights.json` (the committed
`fixed-component-weights` model applied to `dose-28g-1e`, tumor bound as
`core`, brain at default weights) at a declared 1e9 source particles/s.

Result: **D50 binds at 0.947 s** (t_D2 = 1.265 s, t_D50 = 0.947 s). In
OpenPINT's published benchmark cases D2 was the tighter constraint; on
this phantom the median binds — a legitimate contrast, since the two
volumes' hot-spot-to-median ratios differ. At t_brain the tumor receives
D50 ≈ 3.7 Gy-w, D98 ≈ 2.8 Gy-w (scaled from the report's per-particle
rates). The report carries the dose bundle's SHA-256 and the endpoint
assumptions verbatim.

## Scope

Declared research demonstration. The phantom boron loading is dilute
(1e-7 mass fraction) so component weights act as a convention, not a
treatment-representative uptake model; no clinical claim.

## Erratum (2026-09-23) — hydrogen dose response

Multigroup data written by `openbnct sn collapse` before this date carries
the `hydrogen` dose response 10⁶ too large: the elastic-recoil energy was
taken from the eV energy grid and multiplied by a per-MeV kerma conversion
(fixed in `openbnct-openmc`'s `mgcollapse`). Every
`dose_response_gy_cm2.hydrogen` vector in this directory's
`multigroup-data-28g*.json` files is therefore 10⁶ too large, and so is the
hydrogen component of every S_N dose bundle folded from them — together
with any quantity that sums it: `physical_total`, weighted (CBE/RBE,
isoeffective, MKM) totals, and the weighted in-phantom dose profiles in the
beam-quality reports. Neutron and photon fluence, activation and
thermal-flux comparisons, and the boron, nitrogen and photon dose
components are unaffected. The committed files stay unchanged as frozen
evidence; re-collapse with the current `sn collapse` before using hydrogen,
total or weighted dose from them.

Every per-field dose under `fields/` and `fields6/` was folded from the
pre-fix `multigroup-data-28g.json`, so the effective (isoeffective) doses,
optimized weights, OpenPINT-endpoint reproduction, robustness and
direction-search results in this directory are dominated by the inflated
hydrogen component and are superseded; they are kept unchanged as the
historical record. Separately, the `obl` field's disk overhangs its entry
face (see "Fields" above); the solver now rejects an on-face disk that
extends past a non-periodic grid edge, because such a disk has no defined
injected strength, so `obl` must be re-aimed or narrowed before it is
re-solved.
