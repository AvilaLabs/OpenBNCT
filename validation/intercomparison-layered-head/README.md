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
- **R17-04 status (diagnosis largely closed):** the underproduction
  decomposed into two conventions the artifact never declared:
  1. *Within-bin source spectrum.* OpenMC's
     `Tabular(interpolation="histogram")` samples a declared bin
     **uniformly per eV** — the [10 keV, 16.9 MeV] fast bin is ~99%
     MeV-range. The deterministic source mapping historically spread
     the same bins at the collapse weighting (Maxwellian ≤0.5 eV,
     1/E above), putting ~40% of the fast component below ~200 keV
     where σ_t ~1.5/cm kills it within ~2 cm. Re-solving with
     `--source-weighting uniform_in_bin` (a declared artifact-level
     convention) moves the slice-integrated fast bin from
     **0.16–0.43× → 0.50–0.86× CE** and epithermal from ~0.15× to
     ~0.2–1.6× (rising with depth). The 28g artifact's apparent
     agreement was the same convention — it predates the switch and
     ran the uniform-per-eV spread.
  2. *Missing thermal upscatter.* Neither collapse used a TSL tape,
     so deep-thermal groups were one-way absorbing sinks while the
     CE free-gas kernel keeps ~kT thermal motion. Re-collapsing 56g
     with `--tsl H1=<H(H2O) tape>` puts real upscatter in the matrix
     (g55→g50–54 ≈ 1.2–3.5/cm); the phantom solve's thermal slice
     rises ~3× to a stable ~0.10× CE and the absorbed-fraction audit
     lands at 0.048 against ~0.04 tallied B10+N14 (+ untallied H
     capture) on the CE side.
- **Residual (open):** a ~10× slice-integrated thermal deficit and a
  ~0.5–0.85× fast-bin remainder persist at 56g, plus a ~0.2×
  near-field epithermal deficit common to *every* deterministic
  variant including 28g — suspected remaining sources are the
  epithermal near-field buildup (first-moderation generation) and
  thermal residence-time condensation; `--p1` and S4/S8 A/B runs
  show the transport correction and quadrature are not the residual
  mechanism. `compare-condensation.py` gives the slice-integrated,
  lethargy-proportional remap used for these numbers.
- **Water-column experiment (diagnostic, `target/preflight-tmp/wcol`,
  not frozen):** a transverse-periodic 1×1×20 brain column (8 mm
  cells, the same disk+cone+spectrum source) tallied in 21 fine
  energy bins vs the same 56g solve. It isolates the moderation
  chain without the head-shell stack:
  - *Thermal:* with the TSL matrix the column agrees to
    **0.67–0.95× CE** below ~0.2 eV across all depths — vs
    0.03–0.3× with the free-gas matrix. The phantom's ~10× thermal
    deficit was dominantly the missing upscatter; the remaining
    phantom factor comes through the degraded epithermal feed.
  - *Epi/fast:* after two defects were found and fixed the residual
    is ~1.5–2× over-deposition of the 0.5 eV–100 keV range near the
    entrance transitioning to ~0.2–0.5× under-transport deep —
    the same flattening signature the phantom shows as the
    near-field epi deficit + deep fast underproduction.
  - *Fixed in transport, not data:* the uncollided-split deposit
    applied the full disk intensity `J/μ̄` to every cell whose
    center back-rayed into the disk — cells the footprint only
    partially covers got the whole beam (≈1/coverage overcount;
    ~27% in this column, an annulus error at the phantom's disk
    rim). `uncollided_beam_flux` now deposits the cell-averaged
    fluence over an 8×8 transverse point grid — partially covered
    cells get the illuminated fraction — verified by
    `uncollided_deposit_scales_by_transverse_coverage` and it
    removed the near-face overshoot's coverage component
    (~0.75×) exactly.
  - *Added: `beam_sigma_nodes_per_cm`.* The collapse now emits a
    4-node uniform-in-eV sub-bin σ_t kernel per group; under
    `--source-weighting uniform_in_bin` the uncollided deposit is
    Σ_j w_j·e^{−σ_j·s} — the penetrating tail survives instead of
    attenuating at the group's flux-weighted σ_t (which
    over-removes in broad fast groups). Neutral in the bisected-56
    structure (sub-bin σ spread is already small); it matters for
    coarse or adaptive structures spanning strong σ_t curvature.
  - *Boundary-placement verdict:* two opposite 56g allocations —
    flux-importance (41 thermal groups) and region budgets
    (30 epithermal groups) — leave the epi over-production
    essentially unchanged (identical 2.7–4.4× near-field values
    within provisional-solve tolerance), while pure flux mass
    *starves the source bands* (residual worsens to 0.72, fast
    bins overshoot >400× at the cap). Edge positions are not the
    mechanism; the remaining term is the condensed transfer
    density/deposit amplitude itself. `sn boundaries` +
    `--boundaries-file` stay as documented tooling, with the
    flux-starvation caveat: equal-importance placement needs a
    source-band floor to be usable for transport structures.
  - Caveat: the S4 column solve emits a stable field (24- vs
    60-outer agreement to ~0.1%) while its residual metric plateaus
    at ~0.3 — the artifact records `converged: false`; treat the
    magnitude pattern, not a qualified number.
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
