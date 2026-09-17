# Deterministic S_N Solver Throughput Characterization

**Status:** Measured baseline; informs, but does not close, R7-05's OpenMC
run-path profiling.

**Date:** 2026-10-02

## 1. Purpose

R7-05 asks for transport throughput numbers on the workbench's run paths.
This document records the measured scaling behavior of the in-house
multigroup S_N solver (`openbnct sn solve`, R8-01) — the run path that is
fully under this repository's control. The OpenMC run-path profiling called
for in `TRANSPORT_PERFORMANCE_INVESTIGATION.md` remains gated on free CPU;
the 196M-history baseline (~2,006 histories/s on six i3-N305 threads) stands
as the Monte Carlo reference point and is **not** directly comparable to the
numbers here — a cell-direction sweep is a different unit of work than a
particle history.

## 2. Method

Scaling sweep over Cartesian slab cases generated programmatically:

- **Grid:** `n³` cells at 5 mm spacing, vacuum boundaries, single material.
  n ∈ {8, 16, 24, 32, 40} → 512 … 64,000 cells.
- **Data:** two-group structure `[1e6, 1, 1e-3] eV` with downscatter —
  exercises the source-iteration path rather than a single-group shortcut.
- **Source:** boundary beam entering the −z face, `uncollided_split` beam
  model (analytic uncollided ray-trace + collided sweep).
- **Quadrature:** level-symmetric S4, S8, S16 — 24, 80, 288 ordinates.
- **Convergence:** default `1e-6` relative scalar-flux change; every run in
  the matrix converged in 2 outer iterations (residual ~3e-7). Iteration
  count is case-dependent — this fixture is weakly scattering, so the
  numbers below are effectively the per-sweep cost.
- **Timing:** wall clock around the full CLI invocation (startup + JSON
  I/O + solve + write), debug build. For the smallest cases this includes a
  fixed ~50–100 ms process overhead.
- **Environment:** debug `openbnct` binary on an Intel i3-N305, under
  `systemd-run --user --scope -p MemoryMax=6G -p CPUQuota=200%`; the sweep
  is serial, so the CPU cap did not bind.

## 3. Measured wall times (s)

| cells (n³) | S4 (24 dirs) | S8 (80 dirs) | S16 (288 dirs) |
|-----------:|-------------:|-------------:|---------------:|
| 512        | 0.168        | 0.461        | 1.621          |
| 4,096      | 1.591        | 4.907        | 17.046         |
| 13,824     | 6.091        | 19.769       | 72.903         |
| 32,768     | 15.148       | 54.106       | 190.329        |
| 64,000     | 35.771       | 111.891      | 402.852        |

## 4. Scaling behavior

**Ordinate count.** Work per sweep is `cells × directions` and the
level-symmetric direction count is `N(N+2)`. At n=40:

- S8/S4 time ratio 3.13 vs. direction ratio 80/24 = 3.33
- S16/S8 time ratio 3.60 vs. direction ratio 288/80 = 3.60 (exact)

The solver is direction-count-linear as designed.

**Cell count.** At fixed order, scaling is mildly superlinear in cell count
(e.g., S16: 4096→64,000 cells is 15.6×, wall time is 23.6×) — consistent
with cache pressure on the `[voxel][group]` flux array once the working set
exceeds L2/L3, not with an algorithmic defect.

**Sweep rate.** Averaging `cells × directions × outer_iterations / t`:

- n=40, S16: 36.9M cell-direction sweeps / 402.9 s ≈ **91.5k sweeps/s**
- n=40, S4: 3.07M / 35.8 s ≈ **85.9k sweeps/s**
- n=8, S16: 295k / 1.62 s ≈ **182k sweeps/s** (fits in cache)

Practical reading on this hardware: a ~64k-cell, two-group, S16 solve is a
~7-minute serial operation in a debug build; S8 is ~2 minutes; S4 under a
minute. Release builds and per-case iteration counts will move the constant,
not the scaling law.

## 5. Implications and limits

- The sweep is **serial**. Ray-parallel or distributed-source-iteration
  variants are unexplored; the ordinate loop is embarrassingly parallel in
  principle, and the ~90k sweeps/s serial constant is the honest baseline
  any parallel claim must beat per thread.
- Two-group weak-scatter fixtures converge in 2 iterations; realistic
  multigroup problems with significant upscatter or thermal groups will
  need more outer iterations — multiply the table accordingly.
- Debug build. The release-build constant is deliberately not claimed here.
- These numbers characterize the deterministic path only. They say nothing
  about OpenMC throughput, which R7-05's profiling slice still owns.

## 6. Provenance

- Harness: `/tmp/sn-scaling/` (generated `case{n}.json`, `mg2.json`,
  `sweep.sh`, `results.csv`) — local profiling fixture, deliberately not a
  committed benchmark: the numbers are hardware-bound and would rot.
- Solver: `crates/openbnct-transport/src/multigroup.rs`.
- Command: `openbnct sn solve --case case{n}.json --data mg2.json
  --order {4,8,16} --output out_{n}_{order}.json`.
