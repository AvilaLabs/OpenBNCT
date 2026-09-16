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

Remaining (in dependency order):

1. `openmc run` on the bound case (~20M histories, validation scale)
   with `provenance/neutron-response-set.json`.
2. `openmc collect` → dose bundle → `beam qa --dose` in-phantom metrics.
3. `measurement-record` for the published in-phantom values (Au-197
   normalization at the thermal maximum, gamma depth profile, thermal
   peak position ~2.0 cm) + `measurement compare` evidence record.

The open literature question: several published in-phantom quantities
are figure-only in accessible sources; the record will encode only
values with citable provenance, with `absolute_uncertainty_1sigma: null`
where the source's stated uncertainty was not transcribed.
