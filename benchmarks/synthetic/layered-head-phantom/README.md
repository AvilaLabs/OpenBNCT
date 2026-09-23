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

`multigroup-flux-28g-1e.json` / `dose-28g-1e.json`: the S₈ 28-group
solve with the cone uncollided split, consistent transport correction,
and collapse-consistent source-bin weighting converged in 4 outer
iterations over the 15625-cell heterogeneous assignment (skin 3272 /
skull 1250 / brain 3695 voxels, remainder void);
`source_spectrum_weighting: "collapse_consistent"` is recorded on the
flux artifact. `beam-quality-28g-1e.json` carries the absolute
thermal-fluence depth profile and in-phantom tumor-dose profile. The
superseded `multigroup-flux-28g.json` pair predates the
within-bin-weighting fix (deserialization default `"uniform_in_bin"`).
The head phantom's QA metrics are degenerate by construction (no tumor
region is declared — the trace-¹⁰B brain convention makes the
advantage ratio ≈ 1), so the report's value is the resolved absolute
profiles through a realistic tissue stack rather than the summary
indices.

Research scope only — no clinical qualification, commissioning, or
treatment-use claim.

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

Corrected artifacts for this benchmark: `multigroup-data-28g-v2.json`
(`collapse-v2.sh`; σ_t, the P0 scatter matrix, μ̄ and the boron, nitrogen
and photon responses are bit-identical to `multigroup-data-28g.json`, the
hydrogen responses are exactly 10⁻⁶ of the old ones, and the P1/Pₗ moment
arrays the collapse now emits are added) and `dose-28g-v2.json` (a fresh
S₈ solve with the same options as `dose-28g-1e.json` — uncollided split,
transport correction, convergence 1e-6 — converged in 10 outer
iterations). Because the solver has changed since the `-1e` run (θ-weighted
diamond closure, coarse-mesh rebalance), its boron, nitrogen and photon
components differ from `dose-28g-1e.json` by a few percent (median voxel
ratios 0.98–1.04); its hydrogen component is ~10⁻⁶ of the old one. The
"Try it" command in the top-level README and the workbench's bundled
example now use these files. `dose-28g.json`, `dose-28g-1e.json` and the
planning studies under `planning/` predate the fix.
