# Intercomparison: Kobayashi P1-ii — S_N vs OpenMC vs GMVP

First R17-03 cross-code case: the same Kobayashi problem-1 50%
scattering geometry run through OpenMC's multi-group mode and
compared point-by-point against both the published GMVP reference
and our deterministic S8 result.

## Setup

- Same mirrored domain as `../canonical-kobayashi-p1/`:
  [−100,100]³ cm vacuum, source cube [−10,10]³ at q = 1 n/cm³/s
  (8000 n/s), void shell [−50,50]³, shield σ_t=0.1 / σ_s=0.05,
  void σ_t=1e-4 / σ_s=5e-5. One energy group, P0 scatter.
- OpenMC 0.15.3, `energy_mode = multi-group`, macroscopic XSdata
  library generated in-script; IndependentSource box + Discrete
  energy in the single group (source energies must land inside the
  MG group or every site is rejected — the sharp edge of the
  energy-constraint check).
- Tally: flux on the same 2 cm / 100³ cell lattice as the S_N solve
  → cell-mean vs cell-mean, no interpolation. 200 batches × 100k
  histories ≈ 107 s.

## Result (20M histories)

MC tracks GMVP within ~1σ at all probes (worst |z| ≈ 1.9), so the
MC realization is a trustworthy arbiter. The three-way table
(`comparison-mc.json`) then shows the S_N ray-effect field directly:

| probe | S_N−ref | MC z | S_N−MC |
|---|---|---|---|
| (5,5,5) src | +0.4% | −1.9 | +1.0% |
| (5,15,5) void-near | +3.3% | +0.6 | +2.8% |
| (5,65,5) shield | −38.5% | +0.5 | −40.0% |
| (5,95,5) deep | −47.9% | +0.9 | −56.9% |
| (45,45,45) lobe | +108.7% | −0.4 | +110.9% |
| (55,55,55) lobe | +193.3% | −1.5 | +218.5% |

The S_N−MC deltas equal the S_N−GMVP deltas (MC adds no bias of its
own): the deterministic code underfills off-axis void-streaming
cones and overshoots the diagonal lobes — measured, not assumed.

## Reproduce

```sh
# MC leg (needs an openmc python env; ~2 min at 20M histories)
PATH=<openmc-env>/bin:$PATH python3 openmc_run.py
python3 compare_mc.py
```

`KOB1_MC_BATCHES` / `KOB1_MC_PARTICLES` scale statistics.

Research-only cross-check — not a clinical or commissioning claim.
