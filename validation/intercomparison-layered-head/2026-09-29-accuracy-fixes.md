# Layered head vs continuous-energy OpenMC after the 2026-09-29 fixes

Reference: `openmc-tallies.json` in this directory (OpenMC 0.16.0, ENDF/B-VIII.1,
400k histories; B10/N14 absorption rate densities per cm³ per source neutron — the
tally JSON is already divided by the 0.512 cm³ voxel volume). Deterministic dose is
compared as `D·ρ / (R·E)` with `E` the charged-particle release (2.34 MeV for
¹⁰B(n,α), 0.626 MeV for ¹⁴N(n,p)); 1.0 means agreement.

Fixes (see CHANGELOG): redundant reactions excluded from collapsed σ_t (H capture was
double-counted); transport correction restricted to the in-group element; uncollided
beam attenuated along the actual voxel ray; P1 default; exponential closure P0-only.

Data: `benchmarks/synthetic/layered-head-phantom/multigroup-data-28g-v5-tsl.json`
(`collapse-v5-tsl.sh`). Command (S4, 28 groups):

```text
openbnct sn solve --case benchmarks/synthetic/layered-head-phantom/case.json \
  --data benchmarks/synthetic/layered-head-phantom/multigroup-data-28g-v5-tsl.json \
  --assignment benchmarks/synthetic/layered-head-phantom/assignment.json \
  --source-weighting uniform_in_bin --dose dose.json --output flux.json
```

| Configuration | B brain | N skin | N skull | N brain | on-axis N, entrance → exit |
|---|---|---|---|---|---|
| P1 (default), no exp closure | 1.08 | 1.07 | 1.04 | 1.03 | 1.06 0.99 1.06 1.08 1.06 1.16 1.16 |
| P0 + exp closure (previous default) | 0.80 | 0.97 | 0.81 | 0.76 | 1.05 0.91 0.76 0.58 0.44 0.39 0.33 |
| P0, no exp closure | 0.69 | 0.78 | 0.74 | 0.66 | 0.93 0.73 0.66 0.61 0.58 0.63 0.62 |

With the defaults the command above converges in 21 outer iterations (185 s on 2
cores, release build) and reproduces the P1 row exactly.

Absorbed per source neutron: S_N (P1) 0.212; the reference's ¹⁴N + estimated H capture
is ~0.19. Supporting single-physics checks: infinite homogeneous brain (thermal 0.97×,
epithermal 1.04×, fast 0.89×) and a transverse-periodic brain column (P1 absorbs 0.45
per source vs 0.43).

Scope: one synthetic geometry, one beam, 28 groups at S4, a 400k-history pilot
reference (region sums ±0.1–0.7 %). Not a clinical or facility validation.
