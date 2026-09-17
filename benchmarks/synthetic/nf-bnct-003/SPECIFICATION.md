# NF-BNCT-003: Analytic Pure-Absorber Slab

**Specification version:** 0.1.0

**Status:** Geometry, idealized material, and source frozen; analytic
oracle and acceptance contract declared; execution pending

**Qualification ceiling:** Synthetic research only (code-verification
oracle; the material is idealized and is not a tissue surrogate)

## Purpose

`NF-BNCT-003` is the library's analytic ground-truth case: a
monoenergetic thermal neutron beam into a near-pure absorber, where the
boron component dose follows the uncollided attenuation law to a
documented bound. It is designed to reveal:

- absolute normalization errors (the exponential prefactor ties
  normalization to source and material constants, not another code's
  output);
- attenuation-coefficient errors (wrong Σ_t shows up as a log-slope
  deviation);
- voxel-boundary and scoring-mesh indexing errors (the profile is
  checked bin-by-bin); and
- energy-deposition errors in the `¹⁰B(n,α)⁷Li` channel (the boron
  component is the only significant dose term).

## The analytic oracle

At the source energy `0.0253 eV`, ¹⁰B is a near-perfect absorber:
σ_a = 3837 barns against an elastic σ_s of order 2 barns, a
scatter-to-absorption ratio of ~6×10⁻⁴. In a pure-¹⁰B medium a
monodirectional thermal beam therefore produces uncollided-dominated
fluence, and because the per-capture energy deposition is fixed at a
single neutron energy, the boron component dose obeys

```text
D_B(z) = D_B(z_0) · exp( −Σ_t · (z − z_0) )
```

with Σ_t = N_B10·(σ_a + σ_s) ≈ 0.2308 cm⁻¹ at the declared density — an
attenuation length of ≈ 4.33 cm across the 20 cm slab, about two decades
of dynamic range. The oracle is a log-slope regression of the scored
boron component over the declared fit window, compared against Σ_t
computed from the bound nuclear-data manifest at the source energy;
the contract tolerance (`analytic_log_slope_relative_tolerance` = 2%)
covers the residual single-scatter buildup and mesh discretization of
the exponential.

This is a genuine closed-form benchmark, unlike mesh-convergence
references that are themselves Monte Carlo products. The approximation
chain is stated in full: pure absorption dominance (bounded by the
σ_s/σ_a ratio), straight uncollided transport (exact for the beam
component), and constant per-capture deposition (exact at a single
source energy).

## Canonical coordinate system and grid

- Same LPS conventions as NF-BNCT-001/002.
- Grid `[4, 4, 40]` at `5.0 mm` spacing, origin `[-7.5, -7.5, -97.5] mm`:
  voxel boundaries `[-10, 10] mm` on x and y, `[-100, 100] mm` on z — a
  20 cm slab with a 2×2 cm cross-section enclosing the source disk.

## Transport geometry and boundary

- Slab `[-1, 1] cm` on x and y, `[-10, 10] cm` on z.
- Vacuum boundary on all six faces.

## Material

`transport/material.json` — the idealized verification absorber: pure
¹⁰B at `ρ = 1.0e-3 g/cm3`, `293.6 K`, free-gas treatment. The low
density is what moves the attenuation length onto the centimetre scale
resolvable by the 5 mm mesh; at tissue-like densities the same material
would absorb within micrometres. The material is a declared code-
verification absorber, not a physical specimen; every nuclide is named
exactly and fractions sum to one.

## Source

`transport/source.json`:

- Fixed source; one unit-weight source neutron per history.
- Uniform disk of radius `0.9 cm` at z `= -9.999999 cm` — just inside
  the incident face, fitting inside the 2×2 cm cross-section.
- Monodirectional `(0, 0, +1)`.
- Monoenergetic `0.0253 eV` (the 2200 m/s thermal reference energy,
  where the ¹⁰B σ_a = 3837 b convention is exact).

## Required component output and contract

Same component profile as NF-BNCT-001/002. In this material the boron
component dominates absolutely; nitrogen and hydrogen are exactly zero
(no such nuclides present — a backend that reports nonzero N/H dose in
this case has a bookkeeping fault) and the photon component is the
0.478 MeV capture γ, 94% branch, which does *not* follow the exponential
once emitted — the oracle applies to `component:boron` only.

The predeclared acceptance contract is
`transport/openmc-acceptance-contract.json` (the usual precision and
estimator-closure gates, with a single-bin integral over the oracle
window carrying the precision gates). The oracle itself is the separate
transport-neutral artifact `transport/analytic-oracle.json`
(`openbnct.analytic-oracle/0.1.0`), which declares the exponential law,
Σ_t = 0.2308 cm⁻¹, the fit window `z ∈ [-8, 8] cm` (the central 32
bins, away from the incident-face transient and the exit-face escape),
the 2% relative tolerance, and the content-bound material/source it
assumes. `openbnct analytic --oracle ... --dose ...` fits the scored
log-slope and emits an `openbnct.analytic-oracle-evaluation/0.1.0`
record.

## Execution status

No controlled execution yet. Requires a B10-only response set bound to
the absorber material and a NJOY HEATR receipt for B-10 — both available
from the existing pipeline, but the content-bound artifacts are produced
at execution time, not predeclared here.
