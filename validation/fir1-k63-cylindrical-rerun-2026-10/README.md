# FiR 1 K63 cylindrical water phantom — rerun on OpenBNCT 0.3.0 (2026-10-01)

A rerun of the [cylindrical-phantom comparison](../fir1-k63-cylindrical-phantom/README.md) with the current solver and data pipeline. Those committed results date from 2026-09-23, before the deterministic-accuracy fixes of 2026-09-29: the multigroup collapse double-counted redundant reactions, plus transport-correction, void-ray and P1 changes. The earlier record is unchanged; this directory adds the current answer beside it.

## What changed in the inputs

- **Data:** `mg-v5-tsl.json` is re-collapsed from ENDF/B-VIII.1 (294 K HDF5, H-in-H2O S(α,β) on H1) by the current `sn collapse`, which excludes redundant reactions from removal. The materials, group boundaries and component profile are the same as the earlier v4 TSL data.
- **Source:** `case-ineel.json` is the earlier `case.json` with its 3-bin FiR 1 spectrum replaced by the 118-bin INEEL-digitized spectrum (`beams/fir1-k63-ineel-20mev.json`, the project default). Disk, cone and port are the same. The 3-bin spectrum makes the result depend strongly on how weight is spread within its coarse bins (`*-3bin-*` files, below).
- **Solve:** S8, P1 (from the data), CMFD, Anderson depth 3, convergence 1e-5. Each solve took ~25 s on 8 threads (the earlier TSL+P1 solve took ~4.5 h).

## Result: thermal-fluence depth profile against TECDOC-1223 FIG. 3

Computed/measured absolute thermal fluence rate, 118-bin source:

| depth (cm) | 1.0 | 1.4 | 1.8 | 2.2 | 2.6 | 3.1 | 3.8 | 4.85 | 6.5 | 8.75 | 11.5 | 14.5 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| uniform_in_bin | 0.58 | 0.68 | 0.74 | 0.78 | 0.84 | 0.92 | 1.04 | 1.19 | 1.35 | 1.58 | 1.53 | 1.28 |
| collapse_consistent | 0.56 | 0.66 | 0.72 | 0.77 | 0.83 | 0.91 | 1.04 | 1.19 | 1.35 | 1.58 | 1.53 | 1.29 |

| comparison | absolute χ² (12 bins) | peak-normalized χ² |
|---|---:|---:|
| 2026-09-23 record (v4 TSL+P1, 3-bin, collapse-consistent) | 131.7 | 18.8 |
| this rerun, 118-bin, uniform_in_bin | 54.5 | 250.4 |
| this rerun, 118-bin, collapse_consistent | 57.8 | 262.8 |

**Reading.**
- **Scale:** the earlier ~1.6–2.3× uniform underprediction is gone; the absolute scale is right at 3–5 cm.
- **Shape:** the computed profile is flatter than measured. It is ~40 % low at 1 cm and ~1.5× high at 8–12 cm.
- **Why the peak-normalized χ² is worse:** the computed thermal peak (~2 cm) sits deeper than the measured one (≤1 cm), so normalizing to the peak amplifies the shape error.
- **Verdict:** the comparison does **not** pass at either normalization.
- **Open attribution questions:**
  - near-surface thermal flux under an 8.5° cone narrower than any S8 ordinate;
  - the digitized measurement's own uncertainty and normalization;
  - source angular distribution;
  - phantom wall.

3-bin source, for reference (`*-3bin-*`): absolute χ² 118 (uniform_in_bin) and 55 (collapse_consistent), with deep-bin ratios of 1.8 and 1.3. The within-bin weighting of a 3-bin spectrum moves the deep tail by ~40 %, which is why the 118-bin spectrum is the comparison of record here.

## Reproduce

From the repository root, with `openbnct` 0.3.0 and the ENDF/B-VIII.1 HDF5 library plus `tsl_H(H2O)` evaluation:

```bash
V=validation/fir1-k63-cylindrical-phantom; D=validation/fir1-k63-cylindrical-rerun-2026-10
openbnct sn collapse --library <endfb-viii.1-hdf5>/neutron \
  --material validation/fir1-k63-water-phantom/material.json --material $V/material-void.json \
  --tsl "H1=<tsl dir>/tsl_H(H2O)_0001.dat" --tsl-temperature 293.6 \
  --boundaries "1.69e7,1e7,6e6,4e6,2.5e6,1.5e6,1e6,7e5,5e5,3e5,2e5,1e5,6e4,3e4,1e4,6e3,3e3,1e3,5e2,1e2,30,10,3,1,0.5,0.1,0.025,0.01,1e-5" \
  --id openbnct.fir1-k63-cylindrical.multigroup-28g-tsl.v5 \
  --component-profile benchmarks/synthetic/nf-bnct-001/transport/component-profile-local-kerma.json \
  --output mg.json
openbnct sn solve --case $D/case-ineel.json --assignment $V/assignment.json --data mg.json \
  --order 8 --anderson 3 --source-weighting uniform_in_bin --convergence 1e-5 \
  --dose dose.json --output flux.json
openbnct beam qa --beam beams/fir1-k63-ineel-20mev.json --dose dose.json --flux flux.json \
  --transverse-depth-cm 2.0,6.0 --tumor-weights B=1995,N=3.2,H=3.2,P=1.0 \
  --normal-weights B=150,N=3.2,H=3.2,P=1.0 --report-id k63-ineel --output bq.json
openbnct measurement compare --record measurements/fir1-k63-cylindrical-phantom-depth.json \
  --against bq.json --report-id k63-ineel-mc --output mc.json
```

Research software; a digitized-profile comparison, not absolute dosimetry or commissioning.
