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
