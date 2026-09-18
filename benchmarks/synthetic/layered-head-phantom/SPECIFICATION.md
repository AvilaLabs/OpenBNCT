# Layered spherical head phantom — specification

Benchmark id: `openbnct.layered-head-phantom.v1`

## Intent

A heterogeneous-tissue deterministic benchmark that exercises material
assignment across three tissue compositions and the multigroup collapse
pipeline over nuclides outside the processed HDF5 subset. It is a
declared research phantom — not an anatomical model — sized so every
tissue layer is resolved on the 8 mm mesh.

## Geometry

- Cartesian grid: 25×25×25 cells, 8.0 mm spacing, spanning x,y ∈
  [−10,+10.4] cm and z ∈ [0,20] cm.
- Sphere centered at (0, 0, 10) cm, outer radius 10 cm:
  - skin shell: 9.2 < r ≤ 10.0 cm (ICRU-44 skin, ρ=1.09)
  - skull shell: 8.4 < r ≤ 9.2 cm (ICRU-44 cortical bone, ρ=1.92)
  - brain core: r ≤ 8.4 cm (ICRU-44 brain, ρ=1.04, trace ¹⁰B 1e-7)
- Outside the sphere: near-vacuum void (ρ=1e-9).

## Source

The committed FiR 1 K63 fixed source (`nctforge.beam.fir1-k63.v1`):
Ø14 cm uniform disk at the z=0 face, 8.54° isotropic cone, declared
3-bin spectrum; port fluence normalization 1.1769e9 n cm⁻² s⁻¹.

## Materials

Elemental mass fractions follow ICRU Report 44 (skin, cortical bone,
brain). Isotopic splits use natural abundances; isotopes below 1e-4
natural abundance are carried only where their ENDF tape is already
extracted (Ca46). ¹⁰B at 1e-7 mass fraction in brain follows the
water-phantom dose-activation convention.

## Nuclear data provenance

- H, C, N, O, B¹⁰: processed ENDF/B-VIII.1 OpenMC-HDF5 294 K tables.
- Na23, Mg24/25/26, P31, S32/33/34, Cl35/37, K39/40/41, Ca40/42/43/44/46/48:
  ENDF/B-VIII.1 evaluation tapes processed by NJOY 2016.78
  RECONR+BROADR to 293.6 K broadened PENDF, then read directly by the
  `sn collapse` ENDF-6 MF3 reader (`--endf NAME=PATH`). Resolved-
  resonance nuclides require the PENDF path — raw MF3 omits resonance
  contributions.

## Acceptance

- Solve converges (residual < 1e-6) at S8, 28 groups, with the
  uncollided split and transport correction active.
- Depth-dose and thermal-fluence depth profiles are emitted for review;
  no external measured series exists for this synthetic phantom — the
  artifact value is the heterogeneous-coverage solve itself plus the
  collapse-pipeline exercise over the full tissue nuclide set.
- Determinism: `generate.py` emits byte-identical artifacts.

## Scope

Research-only. No clinical, commissioning, or treatment-use claim.
