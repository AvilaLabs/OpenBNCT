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
