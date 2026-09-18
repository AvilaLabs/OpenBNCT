# FiR 1 K63 — Liquid B (brain-substitute) cylindrical phantom, 28-group transverse

Third-composition evidence in the FiR 1 K63 series: the brain-tissue-substitute
**Liquid B** phantom from the TECDOC-1223 TL-dosimetry study (Aschan et al.,
pp. 169–175). The phantom is the same Ø 20 cm × 24 cm cylinder used for the
water and PMMA series. This is the only TECDOC-1223 phantom carrying a
**transverse** measurement (FIG. 4), so it exercises the transverse-profile
machinery the water/PMMA depth series cannot.

## Phantom material declaration

`material-liquid-b.json` declares the **brain-tissue-equivalent elemental
composition** the Liquid B formulation was designed to reproduce — the
reference states both main elements (H/O/C/N) and minor elements
(P, Na, Cl, K, S) at ICRU adult-brain atomic densities — at ρ = 1.04 g/cm³.
The solvent recipe itself is not published in the TECDOC; for neutron
transport the element content is the physics-relevant description. Honest
limitation: the aqueous solvent's molecular binding (H(H2O) S(α,β)) is not
yet applied — the collapse is free-gas.

## Evidence set

| File | Contents |
|---|---|
| `material-liquid-b.json`, `material-void.json` | Declared materials |
| `case.json` | FiR 1 K63 beam on the Ø20×24 cylinder, 10 mm voxels |
| `assignment.json` | Material assignment (region + void base) |
| `collapse.sh` | Reproduces `multigroup-data-28g.json` (HDF5 H/C/N/O/B10 + NJOY PENDF minors) |
| `multigroup-data-28g.json` | 28-group collapse including `transport_mu_bar` |
| `multigroup-flux-28g.json` | S₈ solve, transport-corrected P0, cone-split, collapse-consistent source weighting — converged in 4 outer iterations |
| `dose-28g.json` | Folded component dose |
| `beam-quality-28g.json` | QA with absolute thermal-fluence depth + transverse profiles at 2.5/6.0 cm |
| `measurement-comparison-28g.json` | Comparison against `measurements/fir1-k63-liquid-b-transverse.json` (digitized FIG. 4) |

## Result (transport-corrected P0)

| metric | normalized max rel diff | χ² | absolute max rel diff | χ² |
|---|---|---|---|---|
| transverse 25 mm | 0.30 | 24.4 | 2.28 | 3018 |
| transverse 60 mm | 0.26 | 14.0 | 4.97 | 11638 |

The **transverse shape** is reproduced within ~26–30% at both depths —
the profile falloff, asymmetry, and edge structure track the TL-detector
scan. The **absolute level** overpredicts ~2.3× at 25 mm and ~5× at 60 mm,
the same direction and roughly the same magnitude as the corrected water
deep-tail bracket (1.7–4.8×) — a third-composition confirmation that the
residual is transport physics (P0 anisotropy + no S(α,β)), not
material-composition error or normalization.

## Reproduce

```bash
bash collapse.sh                       # writes multigroup-data-28g.json
openbnct sn solve --case case.json --data multigroup-data-28g.json \
  --assignment assignment.json --order 8 \
  --dose dose-28g.json --output multigroup-flux-28g.json
openbnct beam qa --beam ../../beams/fir1-k63.json \
  --dose dose-28g.json --flux multigroup-flux-28g.json \
  --transverse-depth-cm 2.5,6.0 \
  --tumor-weights B=1995,N=3.2,H=3.2,P=1.0 \
  --normal-weights B=150,N=3.2,H=3.2,P=1.0 \
  --report-id openbnct.beam-quality.fir1-k63-liquid-b.v1 \
  --output beam-quality-28g.json
openbnct measurement compare \
  --record ../../measurements/fir1-k63-liquid-b-transverse.json \
  --against beam-quality-28g.json \
  --report-id openbnct.measurement-comparison.fir1-k63-liquid-b-transverse.v1 \
  --output measurement-comparison-28g.json
```

Research-only: these are deterministic multigroup comparisons against a
digitized literature figure (declared 10% 1σ covering ±0.5 cm positioning
and digitization); not a clinical or commissioning statement.
