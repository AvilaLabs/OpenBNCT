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
  overproduces deep flux by ~40–100× and violates global balance:
  42.6 n/s absorbed against a 1.0 n/s source (28g: 0.96 n/s),
  total dose inflated 28.7×, photon (capture) channel +32×.
- **Mechanism (resolved):** the violation was solver-side, in the
  transport correction, not in the collapse and not in the sweep
  closure. The corrected operator subtracted `μ̄·Σ_s` from σ_t but
  removed at most the *diagonal* from the scatter row — so
  σ_a,eff = σ_t,tr − Σ_s,eff drifted below the declared σ_a and went
  negative in 54/56 groups (e.g. −0.117/cm where σ_a = +0.0002/cm).
  A negative effective absorption is a distributed particle source
  proportional to flux; the cascade amplifies it group over group
  into the thermal tail (96% of the 42.6 sat in the two deepest
  groups). The 28g condensation hid the bug because broad-group
  diagonals can absorb `μ̄·Σ_s` (that artifact still audits at 0.96,
  mostly in its deepest group).
- **Fix:** the forward lobe now leaves the row elementwise through
  the collapsed P1 moment (`σ_s1(g→g')` — its row sum is exactly
  `μ̄·Σ_s` by construction, elementwise ≤ σ_s0 in this data), with a
  diagonal-capped fallback for data lacking P1 moments. σ_a,eff
  ≡ σ_a identically. A `balance_absorbed_fraction` audit rides on
  every flux artifact — values > ~1 flag fabrication regardless of
  what `converged`/`residual` claim.
- **Verified:** the same 6³ mini-case that fabricated 2.33× at S8
  conserves at 0.086 after the fix; the full phantom re-solve
  (`multigroup-flux-56g-tcfix3.json`) audits at 0.080 — against
  ~0.04 absorbed in B10+N14 channels alone on the CE reference
  (H capture untallied). The θ-WDD positivity clamps were *not* the
  mechanism — the θ repair landed independently (negligible change
  to this field) and stays as hardening.
- **Open:** the conserving 56g solve now *under*produces deep flux
  (~0.03–0.4× CE on axis, ~0.15× whole-domain) — an honest
  discretization/condensation question (fine-group σ_t
  condensation, group-boundary placement), not a balance violation.
  That is the R17-04 study. The uncollided-split and boundary-flux
  models show the same deficit sign (source-path exonerated:
  boundary-mode audit was 12.0 pre-fix).
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
