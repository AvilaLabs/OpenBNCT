# HU-calibration round-trip — layered-head phantom

End-to-end demonstration of `dicom calibrate`: a synthetic HU volume
derived from the phantom's declared materials is calibrated back into a
`material-assignment`, and the solve on that assignment is compared
against the solve on the ground-truth assignment.

## Chain

1. `head-hu.nii` — synthetic float64 HU volume on the case grid: every
   voxel at its true material's anchor HU (void −1000, brain +35,
   skin +60, skull +900). Synthesized from the committed
   `assignment.json`; the stand-in for a resliced CT.
2. `dicom calibrate --calibration materials/hu-calibration.json
   --hu-nifti head-hu.nii --case ../case.json` →
   `assignment-calibrated.json` (every voxel an exact anchor hit —
   `interpolated 0, clamped 0`), `materials/` (the four anchor
   `MaterialDefinition`s for `sn collapse --material`), and
   `calibration-report.json` (per-anchor coverage).
3. `sn solve --assignment assignment-calibrated.json --order 8` →
   `dose-calibrated.json`. Control: the same solve on the committed
   `assignment.json` → `dose-truth-reproduce.json`. (Flux intermediates
   are regenerable and not committed.)

## Result

- The calibrated assignment is **voxel-exact**: 0 dominant-material
   mismatches, identical per-region counts (7408 void / 3695 brain /
   3272 skin / 1250 skull).
- The two solves are **bit-identical** (`dose-calibrated.json` ==
   `dose-truth-reproduce.json`, all components): the HU-calibrated
   phantom is transparent to transport when the HU volume sits on the
   anchors. Off-anchor HU interpolates two-component anchor mixtures
   (`voxel_fractions`), covered by
   `hu_calibration::tests::layered_head_round_trip_recovers_materials`.

## Scope

`materials/hu-calibration.json` is a **declared demonstration
calibration** — ICRU-44 anchor compositions at conventional head-CT HU
positions, not a scanner stoichiometric fit. A real deployment supplies
its own Schneider-method anchor table (Schneider, Bortfeld & Schlegel,
*Phys. Med. Biol.* 45 (2000) 459) for its scanner, kVp, and kernel; the
artifact schema makes that table versioned and content-bound.
CT grids that differ from the case grid must be resliced upstream —
`calibrate` refuses shape mismatches rather than silently resampling.
