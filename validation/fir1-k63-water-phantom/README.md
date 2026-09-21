# FiR 1 K63 — water-phantom validation case (in progress)

A validation case for R7-02 (measured-data validation): the published FiR 1
in-phantom characterization geometry, bound to the versioned FiR 1 K63 beam
description.

## Geometry and fidelity notes

- **Phantom**: cubical light-water phantom, 51 × 51 × 47 cm, matching the
  large cubical phantom used for FiR 1 depth/radial dosimetry (the Seppälä
  2002 / Koivunoro-era measurement campaigns; the other published phantom
  is a Ø 20 × 24 cm PMMA cylinder). Beam enters the z = 0 face; the
  central axis is the measured axis.
- **Grid**: 26 × 26 × 94 voxels at 20 × 20 × 5 mm — 0.5 cm depth
  resolution resolves the published thermal maximum at ~2.0 cm.
- **Source**: `nctforge.beam.fir1-k63.v1` bound via `openbnct beam bind` —
  uniform Ø 14 cm disk just inside the entry face, isotropic cone
  (half-angle 0.1491 rad), measured 3-group ENDF spectrum.
- **Material**: water at ρ = 1.0 g/cm³ with trace B-10 (1e-7 mass
  fraction) and N-14 (1e-6). The trace loading exists only because the
  component-fold capability gate requires B-10 MT=107 and N-14 MT=103
  tables in every transported material; at these loadings the phantom is
  transport-identical to pure water to ≈1e-5 relative effect — far below
  the ±3% activation-foil uncertainty in the source measurements. The
  measured phantom was unboronated water.

## Chain state

Built and verified:

- `case.json`, `case-bound.json` — transport case, beam bound
- `material.json`, `source.json` — exact material / bound source
- `nuclear-data-manifest.json` — ENDF/B-VIII.1 subset matching the
  material nuclide set exactly (B10/N14 retained for component folds)
- `neutron-transport-domain.json` — derived; closed interval
  [1e-5 eV, 20 MeV] identical to the benchmark domain
- `response-generation-method.json` — frozen NJOY2016.78 recipe rebound
  to the water material
- `evaluated-neutron-source-selection.json` — ENDF/B-VIII.1 selections
  rebound to this case + material
- `openmc-validation-profile.json` — 20-batch smoke-purpose profile

Response-set chain (`provenance/`, evidence root under
`.nctforge-data/nctforge/fir1-k63-water-phantom/`):

- `njoy prepare` → 7-nuclide input bundle + manifest (selection is
  material-exact; the ENDF/B-VIII.1 subset lives at
  `.nctforge-data/nctforge/endfb81-sources/fir1-k63-water-phantom/`).
- `njoy execute` → fresh NJOY2016.78 execution + receipt
  (`provenance/njoy2016-78-execution-receipt.json`). The benchmark
  receipt's binary (`8a37cf70…`) is not on disk; this run used a binary
  rebuilt from the exact bound commit `71a76bc` (executable
  `54964d0c…`, banner `njoy 2016.78`), honestly recorded by the new
  receipt rather than rebound to the old one.
- `inventory-photon-data` → source-bound photon inventory.
- `assess-execution` → `assess-source-aware` → `assess-domain-aware`
  (v0.1/v0.2/v0.3 reports). Qualification
  `transported_photon_kerma_rejected` at all three levels with
  62 in-domain kinematic violations on O16/O17/O18 — the same isotope
  findings and posture as the benchmark chain (which rejected N15 too;
  N15 is not in this material).
- `generate-response-tables` → 6,372-knot union grid, violations carried
  through as evidence; `verify-response-tables` regenerated
  deterministically and emitted the independent review +
  `provenance/neutron-response-set.json`
  (`independently_reviewed`, SHA-256 `c01b05107497…`).

Completed — first in-phantom measured-data comparison (results/):

1. `openmc run` executed 20M histories / 20 batches with
   `provenance/neutron-response-set.json`; evidence root exported under
   `openmc-evidence/` (run receipt binds executable SHA-256
   `fee5fb9d…`, input manifest `fc13f4c9…`), dose bundle at
   `dose-bundle.json`.
2. `beam qa --dose` run under two weight conventions
   (`results/beam-quality-cbe-only.json`, `-tn-folded.json`). The
   T/N-folded convention emulates the published TECDOC-1223 weighting —
   tumor B weight = 3.8 CBE x (52.5 ppm / 0.1 ppm phantom loading) =
   1995, normal = 1.0 x (15 ppm / 0.1 ppm) = 150, neutron components
   RBE 3.2 — since the trace-loaded boron dose scales linearly with
   concentration.
3. `measurement compare` against
   `measurements/fir1-k63-water-phantom.json`
   (`results/measurement-comparison.json`):
   - advantage depth: computed 9.75 cm vs published 8.1 cm (20%),
     within the 25% convention tolerance;
   - peak therapeutic ratio: 3.70 vs 5.7 (35%), within 40%;
   - advantage ratio: 2.43 vs 4.9 (50%) — outside tolerance, the honest
     signature of the simplified port geometry and convention gap;
   - thermal-fluence maximum depth: computed 2.75 cm vs published
     ~2.0-2.5 cm (22%) — resolved via the new `boron_dose_profile`
     field, whose argmax marks the thermal peak because boron-capture
     dose is proportional to thermal fluence under uniform dilute
     loading.

4. `measurement compare` against
   `measurements/fir1-k63-cylindrical-phantom-depth.json`
   (`results/measurement-comparison-depth.json`): the measured
   thermal-fluence depth profile digitized from Aschan et al.
   (TECDOC-1223) FIG. 3 — MTS-Ns TL detectors in the Ø20x24 cm
   cylindrical water phantom, 250 kW scale, 13% stated 1σ. Peak-
   normalized shape comparison against this case's computed
   boron-dose (thermal-fluence proxy) profile: agreement within ~2σ
   through the buildup and peak region (1.0-2.6 cm), then systematic
   divergence (up to 2.5x at 8.75 cm) attributable to the phantom
   geometry difference — the measured Ø20 cm cylinder lacks the
   lateral backscatter of this case's ~51 cm cubical phantom, so its
   axial falloff is steeper. The divergence direction and onset are
   physically consistent; the PMMA and Liquid B series from the same
   figure are recorded in the record under phantom-qualified metric
   names and intentionally not compared against this water-phantom
   profile. The like-for-like check landed in
   `../fir1-k63-cylindrical-phantom/`: a deterministic S₈ three-group
   solve on a voxel-set Ø20 × 24 cm water cylinder reproduces the
   measured profile within ~1.3σ at all 12 bins (χ² = 6.7) —
   confirming the divergence above was phantom geometry.

The figure-only limitation is resolved for the depth-profile series by
direct figure digitization (documented interpolation convention in the
record's derivation note); Mn-55/Au-197 foil uncertainty is ~±3% and
calculated-to-measured phantom agreement is reported at 3-5%
(Koivunoro 2014; Seppala 2002; Seren 1999).

## Deterministic transport on the same grid

The deterministic S_N solver now runs this case directly
(`qa-tsl.sh` reproduces the chain; release binary):

- `sn solve` — S8, 28-group TSL+P1 data (the cylindrical phantom's
  `multigroup-data-28g-tsl-v4.json`; identical material and group
  structure), Anderson depth 5: converged 62 outer iterations at
  residual 9.55e-5 (vs ~116 unaccelerated on the cylindrical case).
  `multigroup-flux-28g-tsl-p1-1e.json`, `dose-28g-tsl-p1-1e.json`.
- `sn photon-solve` — 16-group transported photons sourced by the
  converged neutron flux (n→γ production matrix + pair-annihilation
  secondaries): 3 outer iterations, residual 8.57e-7.
  `multigroup-photon-flux-16g.json`, `dose-photon-16g.json`.

Component-dose check against the OpenMC tallies on the identical
26×26×94 grid (same beam, same geometry, `led` electron treatment on
both sides — no bremsstrahlung anywhere in the comparison):

| component | deterministic | OpenMC | ratio |
|-----------|---------------|--------|-------|
| boron     | 2.97e-14      | 7.69e-14 | 0.386 |
| nitrogen  | 2.70e-17      | 7.03e-17 | 0.384 |
| photon (neutron fold, local kerma) | 2.71e-11 | 3.27e-11 | 0.828 |
| photon (transported) | 1.52e-11 | 3.27e-11 | 0.465 |

Reading:

- The neutron-driven components (boron, nitrogen — both ∝ thermal
  fluence under dilute loading) sit at ~0.39× the MC tally. This is
  the same uniform normalization deficit seen in the absolute
  thermal-profile comparisons on both phantoms (~0.5-0.65×), rooted
  in the 3-bin source histogram + multigroup collapse; it is not a
  transport-shape error.
- Local-kerma photon production is within ~17% of the MC-deposited
  photon dose — given MC loses ~15% of photon energy to boundary
  escape, the underlying n→γ production matrix is close to the MC
  production.
- The *transported* photon dose retains ~56% of production; its
  depth ratio to MC declines smoothly 0.86 → ~0.25, consistent with
  the neutron-field deficit compounding the photon escape. The
  hydrogen component's ~2e5× apparent mismatch is definitional —
  the deterministic component folds recoil-proton kerma while the
  MC bundle's hydrogen channel carries a different convention —
  and is excluded from interpretation.
- `beam qa`/`measurement compare` on this dose (artifacts
  `beam-quality-water-28g-tsl-p1-1e.json`,
  `measurement-comparison-water-28g-tsl-p1-1e.json`) are recorded
  for completeness; the advantage-depth/ratio metrics are
  degenerate on an unboronated phantom (no therapeutic window
  exists), so only thermal-fluence-max-depth is physically
  meaningful: computed 1.25 cm vs measured 2.25 cm.

## Voxelwise gamma vs the OpenMC tally (results/gamma-sn-vs-openmc-5pct-20mm.json)

`openbnct gamma` between the 20M-history OpenMC dose bundle and the
S_N dose on the identical 26×26×94 grid. Two artifacts:

- `dose-28g-tsl-p1-1e-shape-normalized-to-openmc.json` — the S_N bundle
  with each component scaled by its measured ref/candidate sum ratio
  (boron 2.59×, nitrogen 2.61×, photon 1.21×, hydrogen 4.3e-6×), so the
  gamma evaluates *distribution shape* while the documented
  normalization deficit is reported separately, not hidden.
- `gamma-sn-vs-openmc-5pct-20mm.json` — Low γ at 5% dose-difference /
  20 mm DTA (= one lateral voxel — 3 mm criteria are meaningless on a
  20×20×5 mm grid), 10% low-dose cutoff, global normalization,
  per-voxel γ embedded.

Result: boron 76.7% pass (mean γ 1.11), nitrogen 76.1%, hydrogen
62.8% (definitional-mismatch channel, not interpreted), photon 19.6%,
physical_total 39.2%. The boron/nitrogen shape agreement is the
physically meaningful number; photon γ is dominated by the known
transported-photon production deficit (~56% of MC production retained)
compounding boundary escape — a documented systematic, not a
transport-shape error.

Research-scope note: this is a verification comparison of a
deterministic S_N solve against an independent Monte Carlo tally —
not a commissioning or equivalence claim.

## INEEL measured-spectrum rerun (2026-09-20)

The phantom was re-solved with the measured FiR 1 free-beam spectrum
digitized from INEEL/EXT-01-00204 Figure 5 (120-bin histogram, VTT
LSL-M2 region anchors — `beams/fir1-k63-ineel-spectrum.json`,
spectrum artifact `beams/fir1-k63-spectrum-ineel-digitized.json`)
at identical solver settings: S8, TSL+P1, Anderson-5, the same
1e-4 convergence target as the committed `*-tsl-p1-1e` pair.

Artifacts: `multigroup-flux-28g-ineel.json` (converged, 94 outers,
residual 9.67e-5), `dose-28g-ineel.json`,
`beam-quality-water-28g-ineel.json`,
`measurement-comparison-water-28g-ineel.json`.

Findings:

- Thermal-fluence peak depth moved 1.25 -> 1.75 cm vs the measured
  2.25 cm — the resolved epithermal structure hardens the incident
  spectrum relative to the coarse histogram and pushes
  thermalization deeper, a real improvement in the one metric that
  is physically meaningful on this unboronated phantom.
- The deep thermal tail nonetheless *fell* further: computed/measured
  shape declines from ~0.65x (3-bin) to ~0.08-0.20x beyond 7.7 cm
  (INEEL) on the normalized profile — the same qualitative pattern
  the PMMA phantom showed.
- Second-phantom confirmation of the PMMA result: the absolute
  residual is **not** a source-spectral-resolution artifact. The
  earlier attribution to "the 3-bin source histogram" is withdrawn;
  the remaining deficit lies in transport/data fidelity
  (angular-order, group structure, scatter-kernel treatment) or in
  the declared port-fluence normalization.

The measured-spectrum source remains the better-provenanced input
and is kept as the canonical refined beam model.

Research-scope note: the spectrum is a literature reconstruction of
a published measured figure, not a commissioned facility model.
