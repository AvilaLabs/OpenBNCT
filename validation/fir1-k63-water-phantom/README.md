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

Remaining (in dependency order):

1. `njoy prepare` with this selection → water NJOY input manifest, then
   NJOY2016.78 execution for that manifest (the existing execution
   receipt binds the benchmark input manifest and cannot be rebound).
2. Suitability chain for the water execution (v0.1 → source-aware →
   domain-aware) via `njoy assess-*`/`verify-*`.
3. `njoy generate-response-tables` + `verify-response-tables` →
   independently reviewed water response set (required — deck generation
   refuses unreviewed sets).
4. `openmc run` on the bound case (~20M histories, validation scale).
5. `openmc collect` → dose bundle → `beam qa --dose` in-phantom metrics.
6. `measurement-record` for the published in-phantom values (Au-197
   normalization at the thermal maximum, gamma depth profile, thermal
   peak position ~2.0 cm) + `measurement compare` evidence record.

The open literature question: several published in-phantom quantities
are figure-only in accessible sources; the record will encode only
values with citable provenance, with `absolute_uncertainty_1sigma: null`
where the source's stated uncertainty was not transcribed.
