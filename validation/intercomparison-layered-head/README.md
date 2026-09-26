# Intercomparison: layered-head phantom — S_N vs continuous-energy MC

The dosimetric leg of R17-03: the same `layered-head-phantom`
benchmark case run through OpenMC in continuous-energy mode on
ENDF/B-VIII.1, arbitrating the open group-condensation question —
whether the 28→56 g thermal-tail growth seen in the S_N solves is
real physics or residual multigroup condensation error.

## Setup

- Materials/geometry read verbatim from
  `benchmarks/synthetic/layered-head-phantom/`: 25³ × 8 mm voxels,
  skin/skull/brain shells as isotope mass fractions → CE materials
  (all 29 nuclides, ENDF/B-VIII.1), void base realized as vacuum
  (ρ = 1e-9 is below OpenMC's practical floor and physically
  indistinguishable).
- Beam matches `case.json`: uniform disk r = 7 cm on the z = 0 face,
  isotropic cone half-angle 0.1491 rad about +z, three-bin
  tabulated spectrum (0.0611 / 0.9093 / 0.0296 below 0.5 eV /
  epithermal / fast).
- Tallies on the native voxel lattice (cell-mean vs cell-mean):
  flux in five energy bins [1e-5, 0.1, 0.5, 4e3, 2e5, 2e7] eV;
  B10 and N14 absorption rate densities.

## Pilot result (400k histories, 32 s at 4 threads)

- Per-voxel statistics: mid-head B10/N14 capture rel-err 2–9%,
  deepest axial voxel ~18%. ~15–20M histories gives sub-1–2% —
  a ~25 min bounded production run.
- **Finding:** the S8/28-group solve tracks the CE reference within
  ~±30% total flux at every axial depth; the S8/56-group solve
  overproduces deep flux by ~40–100×. The 28g→56g flux growth is a
  condensation artifact (overproduction), not convergent
  refinement — the mechanism now lives in the collapse weighting,
  not the solver.
- Caveat noted in code: `openmc.stats.Tabular(interpolation=
  "histogram")` takes pdf *heights* — passing declared bin
  probabilities emits a ~98%-fast spectrum.

`openmc-tallies.json` freezes the pilot per-voxel bins and capture
rates with σ. `openmc_run/` statepoints are gitignored.

## Reproduce

```sh
OPENMC_CROSS_SECTIONS=<endfb-viii.1 cross_sections.xml> \
  /path/to/openmc-env/bin/python3 openmc_run.py
```

Research-only cross-check — not a clinical or commissioning claim.
