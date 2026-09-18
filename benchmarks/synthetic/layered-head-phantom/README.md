# Layered spherical head phantom

Heterogeneous-geometry benchmark: a concentric-sphere "Snyder-style"
head — 8 mm skin shell, 8 mm cortical-skull shell, brain core —
voxelized on an 8 mm Cartesian grid inside a 20 cm cube and irradiated
by the FiR 1 K63 disk + 8.5° isotropic-cone source (the same fixed
source as the cylindrical-phantom validation).

- `generate.py` — deterministic generator for `case.json`,
  `assignment.json`, and `materials/{skin,skull,brain,void}.json`.
- Tissue elemental compositions are ICRU-44 mass fractions with natural
  isotopic splits; shell thicknesses are sized to the mesh so every
  layer is voxel-resolved. This is a declared research benchmark, not
  an anatomical replica.
- Brain carries a trace ¹⁰B fraction (1e-7) for dose-response
  activation — the same convention as the water-phantom material.

## Nuclear data

H/C/N/O/B¹⁰ collapse from the processed ENDF/B-VIII.1 OpenMC-HDF5
library. The tissue nuclides outside that library (Na, Mg, P, S, Cl, K,
Ca isotopes) are processed from the ENDF/B-VIII.1 evaluation tapes by
NJOY RECONR+BROADR to 293.6 K broadened PENDF, then collapsed by
`openbnct sn collapse --endf` through the built-in ENDF-6 MF3 reader.
Resolved-resonance nuclides require the PENDF path — raw evaluation
tapes omit resonance contributions in MF3.

## Solving

```sh
# after generating the PENDF tapes (process-tissue-pendf.sh) and
# multigroup data:
openbnct sn solve --case case.json --data multigroup-data-28g.json \
  --assignment assignment.json --order 8 \
  --dose dose-28g.json --output multigroup-flux-28g.json
```

## Landed result

`multigroup-flux-28g.json` / `dose-28g.json`: the S₈ 28-group solve
with the cone uncollided split and consistent transport correction
converged in 4 outer iterations (residual 9.1e−7) over the 15625-cell
heterogeneous assignment (skin 3272 / skull 1250 / brain 3695 voxels,
remainder void). `beam-quality-28g.json` carries the absolute
thermal-fluence depth and transverse profiles. The head phantom's QA
metrics are degenerate by construction (no tumor region is declared —
the trace-¹⁰B brain convention makes the advantage ratio ≈ 1), so the
report's value is the resolved absolute profiles through a realistic
tissue stack rather than the summary indices.

Research scope only — no clinical qualification, commissioning, or
treatment-use claim.
