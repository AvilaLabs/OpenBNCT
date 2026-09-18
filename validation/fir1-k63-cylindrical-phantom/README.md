# FiR 1 K63 cylindrical-phantom deterministic depth-profile check

A like-for-like deterministic comparison of the FiR 1 K63 epithermal beam
against the *measured* small cylindrical phantom used in the TECDOC-1223
thermoluminescent-detector study — complementing
`../fir1-k63-water-phantom/`, which models the ~51 cm cubical reference
phantom and therefore cannot match the measured small-cylinder tail.

## What is here

| file | content |
|---|---|
| `case.json` | `openbnct.transport-case/0.1.0` — 24×24×26 grid at 10 mm spacing (xy in [−12,12] cm, z in [0,26] cm), embedding the declared FiR 1 K63 source (`nctforge.beam.fir1-k63.v1`: Ø14 cm disk, 8.5° cone, 3-bin spectrum). `requested_histories` is a placeholder; the deterministic solver does not consume it. |
| `assignment.json` | `openbnct.material-assignment/0.2.0` — Ø20 × 24 cm water cylinder realized as a `voxel_set` region (7584 voxels, 316 per layer) inside a near-vacuum void base. Water reuses the `openbnct.fir1-k63-water-phantom.material.v1` definition (H2O + dilute trace ¹⁰B/¹⁴N). |
| `multigroup-data.json` | `openbnct.multigroup-data/0.1.0` — declared three-group data with boundaries matching the beam histogram bins (16.9 MeV / 10 keV / 0.5 eV / 1e−5 eV). H1/O16 pointwise σ evaluated from the processed ENDF/B-VIII.1 294 K library used by the OpenMC validation run; downscatter removal follows the Fermi slowing-down estimate; upscatter neglected by declaration. The boron dose response is the ¹⁰B(n,α) kerma coefficient at a notional dilute 10 µg/g basis — a folding convention, not phantom composition. Full collapse declaration is in the artifact. |
| `beam-quality-cylindrical.json` | `openbnct.beam-quality/0.1.0` — `beam qa` output folded from the S₈ solve. |
| `measurement-comparison-cylindrical.json` | `openbnct.measurement-comparison/0.1.0` — σ-weighted, peak-normalized profile comparison against the digitized TECDOC-1223 FIG. 3 water-phantom series (`measurements/fir1-k63-cylindrical-phantom-depth.json`). |
| `multigroup-data-28g.json` | `openbnct.multigroup-data/0.1.0` — real-data 28-group collapse from the processed ENDF/B-VIII.1 library (`sn collapse`): pointwise σ collapsed over a Maxwellian/1-E weighting, P0 isotropic-in-CM elastic transfer matrix, per-component mass-kerma dose responses, and the scatter-weighted `transport_mu_bar` used by the extended transport correction. |
| `multigroup-flux-28g-split.json` / `dose-28g-split.json` | S₈ 28-group solve and folded dose — cone analytic uncollided split, no transport correction. |
| `multigroup-flux-28g-trcorr.json` / `dose-28g-trcorr.json` | Same solve with the extended transport correction active (σ_t,tr = σ_t − μ̄·Σ_s plus the consistent diagonal self-scatter reduction). |
| `beam-quality-cylindrical-28g*.json` | `beam qa` reports including the absolute thermal-fluence depth profile (declared port fluence × J/Φ × port area → source rate) and transverse profiles. |
| `measurement-comparison-cylindrical-28g*.json` | Both peak-normalized and absolute comparisons for each solve. |
| `models/` | `cbe-protocol.json` (TECDOC-convention CBE/RBE weights) and `mkm-literature-constants.json` (MKM with literature-convention lineal energies) — the two biological interpretations compared by `bio compare`. |
| `bio-cbe-28g*.json`, `bio-mkm-28g*.json`, `bio-model-comparison-28g*.json` | Biological bundles over the split- and corrected-solve doses plus the `openbnct.bio-model-comparison/0.1.0` records (cross-model spread ≈2.5×, uniform). |
| `region-tumor.json` | Declared "tumor" mask (centered r≤4 cm disk, z ∈ [2,5] cm) used to exercise the CBE model's region override — a research convention on a water phantom, not an anatomy. |

## Reproduce

```bash
openbnct sn solve \
  --case validation/fir1-k63-cylindrical-phantom/case.json \
  --data validation/fir1-k63-cylindrical-phantom/multigroup-data.json \
  --assignment validation/fir1-k63-cylindrical-phantom/assignment.json \
  --order 8 --dose /tmp/cyl-dose.json --output /tmp/cyl-flux.json

openbnct beam qa --beam beams/fir1-k63.json \
  --report-id openbnct.beam-quality.fir1-k63-cylindrical.v1 \
  --dose /tmp/cyl-dose.json \
  --tumor-weights B=1995,N=3.2,H=3.2,P=1.0 \
  --normal-weights B=150,N=3.2,H=3.2,P=1.0 \
  --output beam-quality-cylindrical.json

openbnct measurement compare \
  --record measurements/fir1-k63-cylindrical-phantom-depth.json \
  --against beam-quality-cylindrical.json \
  --report-id openbnct.measurement-comparison.fir1-k63-cylindrical.v1 \
  --output measurement-comparison-cylindrical.json
```

The 28-group pipeline (ENDF-collapsed data, absolute-scale QA):

```bash
# collapse (once) — HDF5 library nuclides, or --endf N=PATH for PENDF
openbnct sn collapse --library <endfb81-hdf5-neutron-dir> \
  --material ../fir1-k63-water-phantom/material.json \
  --material <void-material.json> \
  --boundaries <28-group descending eV list> --id <artifact-id> \
  --component-profile <local-kerma profile> \
  --output multigroup-data-28g.json   # see artifact declaration for provenance

# solve (uncollided split + transport correction on by default;
# --no-transport-correction / --no-uncollided-split give the raw path)
openbnct sn solve --case case.json --assignment assignment.json \
  --data multigroup-data-28g.json --order 8 \
  --dose dose-28g-trcorr.json --output multigroup-flux-28g-trcorr.json

# QA with absolute depth profile + transverse profiles
openbnct beam qa --beam ../../beams/fir1-k63.json \
  --dose dose-28g-trcorr.json --flux multigroup-flux-28g-trcorr.json \
  --transverse-depth-cm 2.0,6.0 \
  --tumor-weights B=1995,N=3.2,H=3.2,P=1.0 \
  --normal-weights B=150,N=3.2,H=3.2,P=1.0 \
  --report-id openbnct.beam-quality.fir1-k63-cylindrical.28g-trcorr.v1 \
  --output beam-quality-cylindrical-28g-trcorr.json

# measurement comparison emits peak-normalized AND absolute rows
openbnct measurement compare \
  --record ../../measurements/fir1-k63-cylindrical-phantom-depth.json \
  --against beam-quality-cylindrical-28g-trcorr.json \
  --report-id openbnct.measurement-comparison.fir1-k63-cylindrical.28g-trcorr.v1 \
  --output measurement-comparison-cylindrical-28g-trcorr.json

# cross-model biological spread on the same physical bundle
openbnct bio apply --model models/cbe-protocol.json \
  --physical-bundle dose-28g-trcorr.json \
  --region-mask tumor=region-tumor.json --output bio-cbe-28g-trcorr.json
openbnct bio apply --model models/mkm-literature-constants.json \
  --physical-bundle dose-28g-trcorr.json --output bio-mkm-28g-trcorr.json
openbnct bio compare --a bio-cbe-28g-trcorr.json --b bio-mkm-28g-trcorr.json \
  --region-mask tumor=region-tumor.json \
  --output bio-model-comparison-28g-trcorr.json
```

The S₈ solve converges in 15 outer iterations (residual 9.8e−7). The
8.54° source cone is narrower than any discrete ordinate direction, so
the boundary flux collapses to the nearest inward ordinate while the
analytic uncollided split still ray-traces the true cone axis.

## Result

Peak-normalized water thermal-fluence depth profile: **PASS** —
χ² = 6.73 over 12 bins, all bins within ~1.3σ of the digitized measured
values. Relative differences are 3–11% through the buildup and
mid-phantom; the deepest bins (11.5, 14.5 cm) run 31–52% low as the
three-group diffusion tail undershoots — consistent with the declared
two-downscatter-group collapse. The PMMA and Liquid B series in the
measurement record are unmatched by design (water phantom only).

This resolves the tail divergence recorded by the cubical-phantom
comparison (`../fir1-k63-water-phantom/results/measurement-comparison-depth.json`):
the disagreement there was phantom geometry (lateral backscatter in a
51 cm cube vs a Ø20 cm cylinder), not source or data.

## 28-group real-data result — honest status

The ENDF-collapsed 28-group solves do **not** pass the measured
profile; they bracket it:

- **Split-only solve** (`multigroup-flux-28g-split`): absolute thermal
  fluence underpredicts the measured depth profile by ~10× at 14 cm
  (peak-normalized χ² = 83; absolute χ² = 309). The P0-isotropic
  transfer matrix over-removes forward-peaked hydrogen scattering.
- **Extended transport correction** (`multigroup-flux-28g-trcorr`):
  σ_t,tr = σ_t − μ̄·Σ_s applied consistently to removal *and* the
  diagonal self-scatter converges cleanly (4 outer iterations,
  residual 9.4e−7) and deepens the thermal tail — but now
  **overpredicts** the measured profile ~2.3–9.5× (peak χ² = 531;
  absolute χ² = 13605).

The measured series therefore sits between the raw-P0 and
corrected-P0 deterministic results. Remaining declared model gaps:
P0 isotropic transfer even after the correction (no P1 anisotropic
in-group source), no S(α,β) thermal-scattering treatment below ~4 eV,
and the histogram source-bin mapping. The absolute scale itself is
verified: the solver's incident thermal fluence (≈7.2e7 cm⁻² s⁻¹)
reproduces the declared port thermal rate (7.19e7) within 1%, so the
comparison is genuinely absolute — the disagreement is transport
physics, not normalization.

Note on convention history: the committed `*-28g*` flux artifacts were
produced while histogram bins spread uniform-per-eV across sub-groups
(`source_spectrum_weighting: "uniform_in_bin"` — the deserialization
default). The solver now maps within-bin weight
collapse-consistently (Maxwellian below 0.5 eV, 1/E above); artifacts
emitted after the change record `collapse_consistent` explicitly.

## Scope

Research-grade cross-check only. The multigroup data is a declared
three-group fixture — not an evaluation-derived library — and the
measured series is digitized from a figure. Neither constitutes clinical
validation or commissioning evidence.
