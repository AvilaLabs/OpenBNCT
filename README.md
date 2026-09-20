# OpenBNCT

[![CI](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml/badge.svg)](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Status: early research](https://img.shields.io/badge/status-early_research-orange.svg)](ROADMAP.md)
[![Clinical use: not validated](https://img.shields.io/badge/clinical_use-not_validated-red.svg)](DISCLAIMER.md)

**English** | [日本語](README.ja.md)

OpenBNCT is an open, transport-neutral research workbench for boron neutron
capture therapy (BNCT) dosimetry and independent verification. It is built in
Rust around one idea: a dose result should carry its provenance, uncertainty,
and qualification with it — and a claim should only be as strong as the
evidence bound to it.

*(Formerly NCTForge — renamed across crates, the `openbnct` CLI/Python
package, and `openbnct.*` schema identifiers. Pre-rename `nctforge.*`
artifacts, including frozen benchmark evidence, remain readable through a
contract-namespace alias; see [ARCHITECTURE.md](ARCHITECTURE.md).)*

OpenMC is the first transport backend behind a transport-neutral boundary.
MCNP, PHITS, and other external results import through a published
component-dose interchange contract — nothing about the platform requires
bundling those systems.

> **OpenBNCT is research software.** It is not a medical device, not a dose
> calculator for clinical use, and not commissioned for any treatment
> facility. See [DISCLAIMER.md](DISCLAIMER.md).

## What it does today

- **Frozen synthetic benchmark** — `NF-BNCT-001`: synthetic DICOM CT +
  RTSTRUCT, transport-neutral `case.json`, explicit material and source
  contracts, all inputs bound by SHA-256.
- **OpenMC backend** — deterministic deck generation, controlled execution,
  statepoint collection into versioned physical-dose bundles
  (`openbnct openmc generate|run|collect|evaluate`).
- **Verified candidate reference** — three frozen-seed OpenMC runs at 600M
  histories each passed every predeclared acceptance gate: ROI precision,
  per-voxel precision (photon median RSE ≈ 2.7%), estimator comparisons
  (291), and chi-square seed consistency (378). Reports:
  [`openmc-acceptance-report-300M.json`](benchmarks/synthetic/nf-bnct-001/transport/openmc-acceptance-report-300M.json)
  (failed photon gate — kept in the record) and
  [`openmc-acceptance-report-600M.json`](benchmarks/synthetic/nf-bnct-001/transport/openmc-acceptance-report-600M.json)
  (passed).
- **Component dosimetry** — four-component physical dose (boron, nitrogen,
  hydrogen, photon) with absolute voxel uncertainty; DVH, `D_x`/`V_x`/EUD
  region metrics, mask operations, CT-threshold regions, NIfTI I/O, rigid
  image registration with provenance, PET-derived boron-10 fields with
  propagated uncertainty, and systematic-uncertainty reports with an
  explicit correlation model.
- **Biological interpretation** — versioned model families (weighted,
  photon-isoeffective, fractionation), sensitivity sweeps, endpoint models
  (logistic/probit TCP/NTCP, voxel-Poisson, UTCP), BED/EQD2 conversion —
  always a distinct layer from physical dose. `openbnct bio compare`
  records two model families' disagreement on one physical bundle as a
  checkable `openbnct.bio-model-comparison/0.1.0` artifact (region-resolved
  means plus the worst voxelwise ratio above a significance floor).
- **Transport neutrality** — `openbnct.component-dose-interchange/0.1.0`
  import contract; MCNP meshtal and PHITS output adapters; MCNP input-deck
  export; external-dose import and combined-treatment evaluation; a
  `openbnct compare` cross-code comparison record.
- **Facility beam descriptions** — versioned `openbnct.beam-description/0.1.0`
  documents (spectrum, divergence, aperture, normalization, cited provenance)
  that bind onto a transport case (`openbnct beam info|list|bind`); the
  `beams/` registry ships the FiR 1 K63 literature beam.
- **Beam quality characterization** — `openbnct beam qa` emits versioned
  `openbnct.beam-quality/0.1.0` reports: TECDOC-1223-style in-air group
  fluences and current-to-fluence ratio computed exactly from the declared
  source, optional in-phantom advantage-depth/ratio and peak therapeutic
  ratio from a dose bundle, and per-metric comparison against published
  reference values.
- **Measurement import** — `openbnct.measurement-record/0.1.0` documents
  (foil, ion chamber, TLD, TEPC spectra) compare against computed
  artifacts via `openbnct measurement compare`, emitting a
  hash-bound `openbnct.measurement-comparison/0.1.0` record with
  sigma-normalized and relative differences. Scalar metrics plus
  histogram-valued depth profiles are compared (peak-normalized shape
  convention, per-bin chi-square when the record states uncertainties).
- **Published-data validation** — the FiR 1 K63 epithermal beam
  (Seppälä 2002, HU-P-D103) is encoded as a declared beam and checked
  against published measurements: free-beam group fluence rates
  reproduce to ~1e-4 (with an honestly recorded 29% J/Φ gap), and the
  20M-history cubical water-phantom run reproduces the published
  advantage depth within 20% (9.75 vs 8.1 cm) and the thermal-fluence
  maximum at 2.75 cm vs ~2.0–2.5 cm. A deterministic S₈ three-group
  solve on a like-for-like Ø20 × 24 cm cylindrical phantom matches the
  digitized TECDOC-1223 measured depth profile within ~1.3σ at all 12
  bins (χ² = 6.7), and the PMMA-phantom series reproduces likewise
  (χ² = 6.0, 15 bins). Records under `measurements/`; in-phantom
  evidence under `validation/fir1-k63-water-phantom/`,
  `validation/fir1-k63-cylindrical-phantom/`, and
  `validation/fir1-k63-pmma-phantom/`.
- **RT Dose export** — `openbnct dicom export-rtdose` writes any dose
  volume as a multi-frame RTDOSE with full grid geometry and CT
  referencing, verified by independent-toolkit round-trip.
- **Variance reduction** — `openbnct vr resolve` derives OpenMC weight
  windows (uniform, explicit, or MAGIC-equivalent forward-flux bounds
  from an analog statepoint) into a content-bound
  `openbnct.weight-windows/0.1.0` artifact; `openmc generate --vr`
  binds it into the deck and `openbnct vr validate` certifies the
  reduced-history run against an analog acceptance report.
- **Deterministic transport path** — an in-house 3-D Cartesian
  diamond-difference S_N solver behind `openbnct.multigroup-data/0.1.0`
  declared data (level-symmetric quadrature, vacuum/incident/reflective/
  periodic boundaries, analytic uncollided-flux beam split including
  narrow cones, optional extended transport correction σ_t,tr =
  σ_t − μ̄·Σ_s, material assignment overrides); `openbnct sn solve`
  emits a versioned flux artifact foldable to a physical-dose bundle
  (`sn fold` folds an existing flux without re-solving). `sn collapse`
  produces declared multigroup data from pointwise ENDF/B evaluations —
  processed OpenMC-HDF5 nuclides or NJOY-broadened PENDF tapes via a
  built-in ENDF-6 MF3 reader — with P0 isotropic-in-CM elastic transfer
  kernels and per-component mass-kerma dose responses. The physics reach
  is now: **S(α,β) bound-atom scattering** (`--tsl` incoherent-inelastic
  kernels with thermal upscatter + free-gas residual, ENDF MF7/MT4 via a
  parser validated exactly against OpenMC's own, NJOY/THERMR-checked
  bound-atom σ normalization), **P0–P5 anisotropy** (in-group Legendre
  transfer moments `l = 1..5` through an exact discrete addition-theorem
  kernel — eigendecomposed P_l direction kernels, `--anisotropy`),
  **Bondarenko self-shielding** (`--self-shield` heterogeneous-dilution
  weighting in the collapse), **coupled photon transport**
  (`sn photon-collapse`/`sn photon-solve` — a two-pass n→γ solve:
  photoelectric/incoherent/coherent/pair-production atomic tables with a
  Klein–Nishina transfer kernel, ENDF MF12/13/14/15 production collapse
  plus the declared 0.478 MeV ¹⁰B(n,α₁γ) line, photon S_N sweep and
  photon-kerma dose fold), **sub-voxel volume fractions**
  (`voxel_fractions` material regions — sum ≤ 1 per cell, volume-weighted
  composition blending into synthesized transport rows), and **Anderson
  acceleration** on the upscatter-coupled outer iteration. The ordinate
  sweep and per-cell moment reduction are rayon-parallel
  (`RAYON_NUM_THREADS`-bounded; `[ordinate][cell]` flux layout so each
  direction owns an exclusive row, reductions applied serially to keep
  convergence deterministic). On NF-BNCT-003
  the solver reproduces the analytic attenuation oracle exactly (fitted
  slope −0.23080 cm⁻¹, 0.000% deviation over 32 bins); the water-phantom
  TSL+P1 solve lands within ~±20% of the FiR1 measured profile through
  9 cm and the PMMA TSL+P1 solve within ~±5% through 5 cm (normalized
  χ² 18.8 on both compositions — the best deterministic results). The
  k63 water-phantom case now also carries the first coupled n→γ check
  against the OpenMC tallies on the identical 26×26×94 grid
  (photon production within ~17% of the MC-deposited component;
  transported photon at ~0.47× with the deficit tracking the neutron
  normalization — `validation/fir1-k63-water-phantom/README.md`).
- **Adjoint-driven weight windows** — `openbnct vr cadis` runs the
  transposed adjoint solve for a declared response volume and derives
  CADIS bounds; a forward-flux-derived adjoint source gives the
  FW-CADIS global-flattening variant — both emitted as resolved
  weight-window artifacts bound to their derivation.
- **Uncertainty quantification** — `openbnct uq propagate` folds a
  declared `openbnct.multigroup-covariance/0.1.0` (diagonal plus dense
  group-correlation blocks) through central-difference sensitivities of
  the actual discrete operator into a `openbnct.dose-uncertainty-budget`
  per source; `uq screen` runs Morris elementary-effects and
  Saltelli/Sobol screening over declared input dimensions, with a
  fold-only fast path — specs declaring only response-space targets
  (dose-response scales, microdistribution uptake) reuse one converged
  solve instead of re-solving transport per sample.
- **Microdosimetric tallies** — `openbnct bio lineal-tally` evaluates a
  declared site/reaction-product table over a computed multigroup flux
  into the `openbnct.lineal-spectrum/0.1.0` that MKM's computed-spectrum
  source consumes (collision-rate-weighted chord statistics over the
  component's voxels).
- **QA and oracles** — `openbnct gamma` (dose-difference/DTA
  with interpolation and σ-aware modes), `metamorphic`
  (reflection, quarter-turn, superposition, point-reciprocity z-score
  oracles), and `analytic` for closed-form references like the
  NF-BNCT-003 absorber slab.
- **Accelerator beam shaping** — `openbnct accelerator source` evaluates
  a parametric ⁷Li(p,n)⁷Be thick-target source (Liskien–Paulsen 0° cross
  sections over the proton slowing path) and `accelerator beam` binds it
  into a `openbnct.beam-description`; `bsa rasterize|sweep` realizes
  layered beam-shaping assemblies onto a case grid as material
  assignments.
- **Subcellular boron microdistribution** — `openbnct boron
  microdistribution` computes deterministic chord-weighted partition
  corrections (nucleus/cytoplasm/membrane/extracellular) that bound onto
  dose-component interpretation; a declared
  `BoronMicrodistribution` on a material applies a first-order
  compartment-fraction compound factor to the boron dose component, and
  `MicrodistributionUptakeScale` carries compartment-fraction uncertainty
  through Morris/Sobol screening (a declared research approximation, not
  a cell-scale tally).
- **Benchmark library** — NF-BNCT-002 adds a heterogeneous bone/air
  insert acceptance case; NF-BNCT-003 is a pure-absorber slab with a
  closed-form oracle — both with machine inputs, acceptance contracts,
  and conformance tests alongside NF-BNCT-001.
- **Inverse planning** — `openbnct plan optimize` solves non-negative
  exposure-weight assignments against `openbnct.inverse-plan-objective/
  0.1.0` dose-volume objectives (EUD, mean, and dose-at-volume quantile
  targets on named masks over `physical_total`, a component, or the
  BNCT-native `isoeffective` quantity — a `Σ_c w_c·D_c` fold under an
  embedded `BiologicalModel` with per-mask `region_weights` overrides)
  by deterministic cyclic coordinate descent with analytic gradients
  and a weight regularizer selecting the minimum-weight feasible plan —
  a research optimizer, not a commissioned treatment-planning product.
- **Multi-field sweeps** — `openbnct plan fields` aims a disk source per
  beam direction through a target mask, solves and folds a unit-weight
  dose bundle per beam (the optimizer's `--dose` inputs), and binds
  every aimed case, position report, and bundle to the shared inputs in
  a `openbnct.beam-field-set/0.1.0` manifest.
- **Bidirectional interop** — `openbnct nifti export-components` writes
  the four-component NIfTI set with a hash-bound manifest (re-importable
  through `openbnct import nifti`); `--pint` switches filenames to the
  OpenPINT convention (`<case>_B10/_N14/_n/_g`). `openbnct dicom
  rtplan-info`/`export-rtplan` read and write minimal RTPLAN beam
  sequences, and `dicom import-pet` converts a native PET series to a
  body-weight SUV volume (BQML + decay correction, radiopharmaceutical
  record validated) feeding the boron uptake model. `dicom calibrate`
  applies a versioned `openbnct.hu-calibration` anchor table
  (Schneider-method two-component mixing) to CT HU volumes and emits a
  material assignment — the imaging→phantom hop, demonstrated
  voxel-exact on the layered-head round-trip.
- **Three surfaces, one implementation** — CLI (`openbnct`), a native egui
  workbench (integrity-gated tri-planar viewer, dose wash, DVH/metrics,
  plan workspace, source positioning), and a bounded Python package —
  all calling the same Rust contracts.

### Web workbench

The same egui workbench also builds to wasm32 — one codebase, two
products: `cargo run --bin openbnct-gui` for the desktop app, or the
hosted bundle at [openbnct.avilalabs.org](https://openbnct.avilalabs.org)
(also reachable at
[avilalabs.github.io/OpenBNCT](https://avilalabs.github.io/OpenBNCT/)).
The web build is the inspector surface — drag a dose bundle, exposure
plan, or NIfTI volume onto the page and it lands in the matching
workspace through the same validation path as the desktop app.
Filesystem- and process-bound features (case folders, execution) remain
native-only and report that honestly.

**Browser requirements:** the app renders through WebGPU or WebGL2
(with an automatic glow/WebGL fallback). Chrome and Edge work out of
the box, as does Firefox (WebGL2; WebGPU is not required). LibreWolf
disables WebGL by default — allow it per-site from the address-bar
permission prompt, or set `webgl.disabled = false` in `about:config`.
A browser with no WebGL cannot run the web build; use the desktop app
instead.

## Quick start

All 16 `openbnct*` crates are published on crates.io, and the `openbnct`
Python wheel is on PyPI:

```text
cargo install openbnct-cli               # CLI from crates.io
pip install openbnct                     # Python bindings from PyPI
```

From source:

```text
cargo build --workspace                  # CLI + libraries
cargo run --bin openbnct -- --help       # CLI surface
cargo test --workspace                   # full suite incl. conformance
cargo run --bin openbnct-gui             # desktop workbench
```

The Python package builds one `abi3` wheel per platform (Python ≥ 3.10):

```text
pip install 'maturin>=1.7,<2'
maturin build --manifest-path bindings/python/Cargo.toml
```

## Where the evidence lives

- [`docs/USAGE.md`](docs/USAGE.md) — detailed command and workflow reference
- [`ROADMAP.md`](ROADMAP.md) — evidence-gated status, per-milestone detail
- [`benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md`](benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md)
  — the frozen case and its predeclared acceptance gates
- [`conformance/`](conformance/) — public fixture suites: interchange,
  biological and microdosimetric (MKM) models, endpoints, and
  MCNP/PHITS adapters
- [`docs/adr/`](docs/adr/) — architecture decision records
- [`docs/research/TECHNICAL_BASELINE.md`](docs/research/TECHNICAL_BASELINE.md)
  — scientific rationale
- [`docs/research/CROSS_CODE_REPRODUCTION.md`](docs/research/CROSS_CODE_REPRODUCTION.md)
  — the licensed-user recipe for the reference-promotion gate

## Honest status

- The candidate reference passed all **statistical** acceptance gates. Under
  the case specification it is not promoted to a *reference output* until a
  separately implemented transport path (Geant4, or licensed MCNP/PHITS
  produced by a licensed user) reproduces the frozen case.
- Variance reduction is verified **unbiased** against the 600M analog
  reference at up to a 4.3× history reduction (1134 comparisons, max
  z = 2.97), and a 196M-history run cleared both frozen photon-precision
  gates (central-axis photon heating σ 0.853% vs 1.0%; per-voxel median
  2.74% vs 3.0%) at a 3.06× reduction. The one open contract gate is the
  three-seed replication-consistency requirement — two further ~196M
  runs are in progress. See
  [`openmc-vr-validation-196M.json`](benchmarks/synthetic/nf-bnct-001/transport/openmc-vr-validation-196M.json).
- The MCNP/PHITS adapters are verified end-to-end at benchmark scale on
  documented-format bundles; real-engine acceptance remains an open gate.
- The FiR 1 in-phantom comparison uses wide tolerances: published
  advantage figures fold a clinical tumor:normal boron uptake (~3.5)
  into dose weighting while the phantom carries trace loading, and the
  simplified cone source under-models penumbra scatter — the advantage
  ratio misses (2.43 vs 4.9) and is kept in the record as evidence of
  the fidelity-tier gap, not suppressed.
- Nothing here claims clinical qualification, clinical equivalence,
  commissioning, or regulatory suitability.

## License and use boundary

Code is licensed under Apache-2.0. Synthetic benchmark data will receive an
explicit data license before its first release.

The repository must not implement Avify Dose patent subject matter without a
documented intellectual-property review. See
[docs/IP_BOUNDARY.md](docs/IP_BOUNDARY.md).
