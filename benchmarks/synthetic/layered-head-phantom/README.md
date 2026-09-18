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
  --assignment assignment.json --order 8 --output multigroup-flux-28g.json
```

Research scope only — no clinical qualification, commissioning, or
treatment-use claim.
