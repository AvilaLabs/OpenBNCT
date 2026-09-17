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

## Scope

Research-grade cross-check only. The multigroup data is a declared
three-group fixture — not an evaluation-derived library — and the
measured series is digitized from a figure. Neither constitutes clinical
validation or commissioning evidence.
