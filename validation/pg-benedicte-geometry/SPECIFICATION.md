# Prompt-gamma literature-geometry anchor — BeNEdiCTE-style (R13-04)

Runs the full prompt-gamma verification chain (emission → adjoint
response → expected counts → observation → NNLS reconstruction) on
the committed FiR 1 phantom transport in the published
Polimi BeNEdiCTE / Nagoya BNCT-SPECT geometry class — and records
the honest result: **the forward model closes; source localization
on uncollimated detectors does not.**

## Published anchors

- **BeNEdiCTE module** (Polimi/FBK): LaBr₃(Ce+Sr) 5×5×2 cm³
  monolithic crystal, 8×8 SiPM matrix, **pinhole collimator** —
  ~60% detection efficiency at 478 keV, <3% energy resolution at
  662 keV.
- **LENA measurements** (IEEE TRPMS 2025; Sci Rep 2025): two ¹⁰B
  vials at 1.4 cm separation reconstructed in 3D at better than 1 cm
  spatial resolution from collimated projections; 250 ppm detection
  sensitivity at 3.14×10⁶ n/cm²/s.
- **PG-SPECT concept** (Kobayashi et al.): 478 keV prompt line as
  the dose-monitoring signal.

## Fixture encoding

- `emission-two-vials.json` — the committed transported 478 keV
  emission map masked to two 16 mm-diameter vial regions, centers
  20 mm apart at the phantom mid-plane (x = ±10 mm, z = 125 mm).
  Magnitudes are the real transported emissions inside the vial
  voxels — only the spatial extent is synthesized (insert
  self-shielding is a declared second-order omission).
- Detector voxels on ~10.5 cm rings in the phantom's void margin —
  the stand-in for detector positions; the published ~60% crystal
  efficiency enters as the `pg counts` scalar.
- `run-chain.sh` — the 8-detector chain; `r2-*` artifacts extend it
  to 32 detectors across two planes (λ 1e-4 → 1e-6).

## Result — forward model closes, localization needs collimation

- Forward model (emission → counts) executes end-to-end on real
  transported response maps: tallies span 4.6e-17 → 8.3e-13 across
  the ring (uncollimated detectors see sharply different geometric
  acceptances — a real adjoint result).
- **Reconstruction does not localize the vials** — NNLS converges
  in 1 iteration, placing emission at detector-adjacent ring voxels
  at ~1e-9 vs the true ~2e-6. With 8 or 32 uncollimated detectors
  the response columns are near-identical smooth attenuated
  kernels: the inverse is rank-deficient and the residual-matching
  solution parks emission next to the detectors.
- The published programmes resolve vials because the **pinhole
  collimator** makes each projection spatially selective — each
  detector views a narrow ray bundle through the aperture. Our
  response model carries no aperture/collimation structure.

## Follow-on scope this measurement identifies

Collimation-aware `pg response`: model the detector aperture
(pinhole/slit) in the adjoint — either as declared shielding
geometry in the transport case (collimator channel voxels) or as an
angular restriction on the detector tally. That is the missing
capability for a true PG-SPECT reconstruction claim; the forward
chain and the solver itself need no further work for it.
