# Validation directories

In-phantom evidence for the FiR 1 K63 epithermal beam
(`beams/fir1-k63.json`), all under the research-only qualification —
these are cross-checks against published/digitized measurements, not
clinical validation.

| directory | phantom | transport path | measured anchor |
|---|---|---|---|
| `fir1-k63-water-phantom/` | ~51 cm cubical water phantom (reference geometry) | 20M-history OpenMC Monte Carlo | published advantage depth / thermal-fluence maximum (Seppälä 2002) + digitized TECDOC-1223 depth profile (shape comparison — phantom geometry differs from the measured Ø20 cm cylinder) |
| `fir1-k63-cylindrical-phantom/` | Ø20 × 24 cm water cylinder (like-for-like with the measured phantom) | deterministic S₈, declared 3-group ENDF/B-VIII.1 collapse | digitized TECDOC-1223 FIG. 3 water series — **PASS**, all 12 bins ≤ ~1.3σ, χ² = 6.7 |
| `fir1-k63-pmma-phantom/` | Ø20 × 24 cm PMMA cylinder (like-for-like, second composition) | deterministic S₈, declared 3-group ENDF/B-VIII.1 collapse | digitized TECDOC-1223 FIG. 3 PMMA series — **PASS**, all 15 bins ≤ ~1.15σ, χ² = 6.0 |

Free-beam measured-data comparison lives under `measurements/`
(`fir1-k63-vs-beam-quality.json`). See each directory's README for
reproduction commands and provenance notes.

## Canonical transport cases

Scripted, self-contained benchmarks against published reference
solutions — the kind anyone can rerun (`run.sh` regenerates inputs,
solves, and grades). These verify the deterministic S_N path itself:
source deposition, material heterogeneity, boundary conditions, and
angular/spatial truncation.

| directory | case | reference | verdict |
|---|---|---|---|
| `canonical-reed-problem/` | Reed (1971) heterogeneous 1-D slab — strong absorber/source, void gap, c=0.9 scattering regions | Warsa (2002) eigenfunction expansion, 81 pointwise fluxes | **PASS** — region means ≤1% flat / ~2% scattering peak (angular truncation, halving S4→S8); interior pointwise RMS 0.9% |
| `canonical-azmy-problem/` | Azmy (1988) weighted-DD quadrant problem — central source in absorber, full-domain mirror realization | published quadrant means 1.676 / 4.159e-2 / 1.992e-3 | **PASS** — 0.17% / 0.67% / 3.9%, machine-precision quadrant symmetry |

Each case directory declares its tolerances in `compare.py` — set
against what a converged discrete-ordinates solve can meaningfully
resolve at the stated mesh/order and reference-table resolution, with
the rationale documented in the case README — and freezes its output
in `comparison.json`: frozen evidence, regenerated only by a
deliberate new run.

