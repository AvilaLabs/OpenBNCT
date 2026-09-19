# Changelog

All notable changes to OpenBNCT are documented here. The project follows
[Semantic Versioning](https://semver.org/); schema documents carry their
own versions independent of the crate version.

## [Unreleased]

### Added

- Rayon-parallel S_N ordinate sweep in `openbnct-transport`: angular
  flux stored `[ordinate][cell]` so each direction owns an exclusive
  row, with per-cell moment reductions computed in parallel and applied
  serially — bit-for-bit solver semantics under `RAYON_NUM_THREADS`.
- `openbnct nifti export-components --pint`: fixed OpenPINT-convention
  filenames (`<case>_B10`, `_N14`, `_n`, `_g`) on the same
  content-hashed component manifest.
- `openbnct dicom import-pet` / `openbnct_dicom::import_pet_series`:
  native single-frame PET series → body-weight SUVbw volume (BQML,
  START decay correction, radiopharmaceutical dose/half-life/start-time
  record, signed or unsigned 16-bit pixels, negative-activity clamp
  counted in the record) on the shared CT geometry path, emitted as
  float64 NIfTI for the boron uptake model.
- `openbnct plan optimize` / `openbnct_plan::optimize`: deterministic
  non-negative beam-weight optimization against
  `openbnct.inverse-plan-objective/0.1.0` dose-volume objectives
  (EUD, mean, dose-at-volume quantiles on named masks over
  `physical_total`, a component, or `isoeffective`) — cyclic coordinate
  descent with analytic gradients and per-coordinate Armijo line search,
  bound-normalized violations making the penalty scale-free at any dose
  magnitude, weight regularization selecting the minimum-weight
  feasible plan; results emitted as
  `openbnct.inverse-plan-result/0.1.0` qualified
  `inverse_planning_research_only_not_clinical`. `--emit-plan` writes
  the weights as an `openbnct.exposure-plan`
  (`source_strength_scaling` basis) consumable by `plan
  validate`/`plan export`.
- Isoeffective objectives: `dose_quantity: "isoeffective"` embeds an
  `openbnct-bio` `BiologicalModel` in the objective document (content-
  bound component weights, per-mask `region_weights` overrides) and
  folds each beam's four component fields into an effective
  `Σ_c w_c·D_c` dose — linear in beam weights so every metric keeps
  its analytic gradient. `microdosimetric_kinetic` semantics and
  `fractionation` are refused as non-linear.
- `openbnct plan fields` / `aim_disk_source_at_centroid`: multi-field
  driver aiming an on-face `UniformDisk` per beam direction through an
  aim mask, solving and folding a unit-weight dose bundle per beam, and
  emitting a `openbnct.beam-field-set/0.1.0` manifest binding every
  aimed case, position report, and bundle to the shared inputs.

## [0.1.1] — 2026-09-16

### Added

- `openbnct` umbrella crate re-exporting the public library crates under
  namespaced modules (`contracts`, `transport`, `openmc`, `bio`, …) —
  the natural `cargo add openbnct` entry point.
- `openbnct import nifti` and Python `import_nifti`: per-component NIfTI
  dose volumes (the layout OpenPINT writes) lift into validated
  component-dose interchange bundles, with caller-declared producer
  provenance and optional paired sigma volumes.
- NIfTI adapter conformance case under `conformance/adapters/0.1.0/nifti/`
  (four dose + four sigma fixtures, byte-pinned expected interchange and
  bundle documents, `OPENBNCT_UPDATE_CONFORMANCE` regeneration).

### Fixed

- `.gitignore` generated-artifact rules (`*.nii`, `*.nii.gz`, `*.h5`,
  `*.dcm`) no longer swallow conformance fixtures, which are deliberate
  byte-fixed references.

## [0.1.0] — 2026-09-16

First public release (formerly NCTForge; the `nctforge.*` schema family
remains readable for compatibility).

### Added

- Transport-neutral case, material, source, and dose contracts with
  SHA-256 content binding (`openbnct.*` schema family).
- OpenMC backend: deterministic deck generation, controlled execution,
  statepoint collection into versioned four-component physical dose
  bundles (boron, nitrogen, hydrogen, photon) with absolute voxel
  uncertainty.
- Frozen synthetic benchmark `NF-BNCT-001` with predeclared acceptance
  gates (ROI and per-voxel precision, estimator comparisons, seed
  consistency) — passed at 600M histories.
- MCNP meshtal and PHITS xyz-mesh import adapters, verified end-to-end
  against the benchmark bundle.
- NJOY response-set provenance chain: evaluated-source selection,
  acquisition receipts, execution receipt, three-level suitability
  reports, deterministic regeneration, independent review.
- Beam-quality characterization (TECDOC-1223 in-air and in-phantom
  metrics) and `openbnct.measurement-record` /
  `measurement-comparison` schemas for published-measurement
  verification, including histogram depth-profile shape comparison.
- FiR 1 K63 published-data validation: free-beam group fluence rates
  reproduce Seppälä 2002 Table 4 to ~1e-4; the cubical water-phantom
  run reproduces published advantage depth (9.75 vs 8.1 cm) and the
  thermal-fluence maximum (2.75 cm vs ~2.0–2.5 cm).
- Biological effectiveness application (BED/EQD2 bundles), DVH and
  `D_x`/`V_x`/EUD region metrics, NIfTI I/O, DICOM RT Dose export,
  PET-derived boron-10 fields, rigid image registration with
  provenance, systematic-uncertainty reports.
- Variance reduction: OpenMC weight-window derivation and unbiasedness
  validation (4.3× history reduction on the benchmark).
- egui desktop workbench for case loading, dose overlay, and evidence
  inspection.
- `openbnct` Python bindings (maturin/PyO3) with wheels for Linux,
  macOS, and Windows.
- OIDC trusted-publishing workflows for crates.io and PyPI.

### Known limitations

- The simplified cone source under-models measured penumbra scatter:
  free-beam J/Φ differs from measurement by ~29%, and the in-phantom
  advantage ratio differs by ~50% under the published weighting
  convention. Both are kept in the record as fidelity-tier evidence.
- Published in-phantom depth profiles are figure-only in accessible
  literature; scalar figures of merit are compared, and the profile
  comparison machinery is ready for a traceable digitization.
- No clinical qualification, equivalence, commissioning, or regulatory
  suitability is claimed. See `DISCLAIMER.md`.
