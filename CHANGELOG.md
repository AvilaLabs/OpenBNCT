# Changelog

All notable changes to OpenBNCT are documented here. The project follows
[Semantic Versioning](https://semver.org/); schema documents carry their
own versions independent of the crate version.

## [Unreleased]

### Added — pharmacokinetics

- `openbnct pk` is now a subcommand family (`pk fit|tissue-scale|
  schedule|dose`). `pk tissue-scale` declares per-region T/B evolution
  `T/B(t) = T∞ + (T₀−T∞)·e^(−μt)` against a named blood curve
  (`openbnct.pk-tissue-spec/0.1.0`) — the product stays analytically
  inside the exponential family, so tissue curves emit as ordinary
  `pk-model` documents. `pk schedule` searches a declared
  irradiation-window grid: beam-on epoch `w` is an exact amplitude
  rescale `aᵢ→aᵢe^(−λᵢw)`, each window gets a full organ-limit solve,
  the limiting structure, and the deliverable tumor-region dose against
  the constant-concentration reference
  (`openbnct.pk-schedule/0.1.0`). `pk dose` emits the time-integrated
  map as a Gray `openbnct.physical-dose-bundle/0.2.0` — boron scaled by
  the region's `I_r(t)/C_plan`, non-boron by `t` — directly consumable
  by `bio apply`, `dvh`, and `report`.

### Added — robust / scenario planning

- `openbnct.scenario-set/0.1.0` declares named discrete plan
  perturbations: global component scales (uptake), per-region component
  scales (T/N on masked voxels), `dose_scale` (output factor), and
  `shift_mm` (whole-field displacement, trilinear-resampled).
- `openbnct plan scenarios` refolds optimized weights per scenario and
  reports per-objective bands — nominal/min/max/mean, worst-scenario
  attribution, and the violated set
  (`openbnct.scenario-report/0.1.0`).
- `openbnct plan optimize --scenario-set` minimizes the *worst-case*
  penalty across nominal + all scenarios (Danskin subgradient of the
  argmax scenario folded into the coordinate descent); the result
  records `method: "worst_case_scenario"`.
- `benchmarks/synthetic/scenario-robust-planning` is a known-answer
  fixture whose worst-case optimum is closed-form
  (`w_h* = 1.498875`, `w_b* = 0`); a committed conformance test asserts
  the optimizer reproduces it.
- `validation/fir1-k63-scenario-budget` encodes the published FiR 1 /
  BPA-F uncertainty budget (Savolainen 6% decomposition, blood boron
  ±20%, skin 1.5×blood; Kotiluoto 8% computational excluding boron) as a
  12-scenario set evaluated against the committed S_N transported
  field.

### Added — boron microdistribution

- `openbnct.boron-microdistribution-measurement/0.1.0` declares an
  assay (autoradiography, ion microbeam, track imaging, fluorescence,
  or declared-other), compound, cell system, reduction geometry, and
  either compartment fractions or a radial boron-density profile with
  per-bin 1σ. `openbnct boron microdistribution` is now a subcommand
  family: `import` reduces the measurement to the
  `openbnct.boron-microdistribution/0.1.0` model (radial bins integrate
  into compartments by annulus overlap; membrane declared explicitly),
  `evaluate` keeps the existing correction contract.

### Added — registration

- `openbnct.registration` gains the `shared_frame_of_reference` method:
  identity transform by construction, with the shared DICOM
  Frame-of-Reference UID recorded as the basis; `register
  frame-of-reference` creates it and `register info` prints the FoR
  basis.

### Fixed — solver

- The θ-WDD multigroup sweep now records the positivity-clamp excess
  (ideal-vs-clamped cell-average and outgoing-edge flux) in balance
  units and feeds it into the CMR balance right-hand side, making
  `f = 1` an exact coarse-mesh-rebalance solution at the transport
  fixed point. `SnOptions::coarse_rebalance` toggles CMR in code;
  `OPENBNCT_NO_CMR` remains as an A/B override and `CMR_DEBUG` now
  prints the per-region defect.

## [0.2.2] — 2026-09-23

(Supersedes the `v0.2.1` tag, which carried stale lockfiles and was
never published.)

### Fixed — dose physics

- `sn collapse` wrote the `hydrogen` (elastic-recoil) dose response 10⁶
  too large: the group-mean recoil energy came from the eV energy grid
  and was multiplied by a per-MeV kerma conversion. Both the free-gas
  and the bound-atom (S(α,β)) paths now convert to MeV, and a
  regression test pins the pure-hydrogen kerma factor near 1 MeV to a
  hand calculation. Every S_N dose folded from earlier data carries a
  hydrogen component — and any total, weighted dose or in-phantom
  profile built on it — 10⁶ too large; fluence, activation and the
  boron, nitrogen and photon components are unaffected. Frozen
  artifacts are unchanged and carry dated errata in their READMEs; the
  layered-head benchmark ships corrected `multigroup-data-28g-v2.json`
  and `dose-28g-v2.json`, which the README "Try it" command, the
  workbench's bundled example and the notebooks now use.
- Boron microdistribution: an α/⁷Li track from a site outside the
  nucleus now deposits only the part of its range that reaches the
  nucleus — `min(t_far, R) − max(t_near, 0)` — instead of the whole
  chord clamped to the range, which credited cytoplasm, membrane and
  extracellular boron with nuclear dose it cannot deliver.
- In-phantom beam quality (`beam qa`: advantage depth, advantage ratio,
  therapeutic ratio, absolute and transverse thermal-fluence profiles)
  measures depth from the face the beam enters — set by its declared
  direction — instead of always from the grid's low face, so beams
  entering through the high face are no longer read backwards.

### Fixed — solver robustness and reproducibility

- An on-face disk source that extends past a non-periodic grid edge is
  rejected (disk aiming, screening perturbations of the aperture, and
  the forward solve): its injected strength was undefined, and the
  uncollided split and boundary-flux paths disagreed on it. Periodic
  axes still accept a wider disk for slab problems. `plan directions
  --adjoint` skips and reports candidate directions whose aperture does
  not fit its entry face.
- The coarse-mesh rebalance sums face currents over a fixed, thread-
  independent partition of the ordinates, so solves are bit-identical
  for any `RAYON_NUM_THREADS`.
- A non-finite scalar flux or outer residual is now an error instead of
  passing the convergence test (`f64::max` ignores NaN); FW-CADIS gives
  a zero adjoint source to groups the forward flux never reaches instead
  of dividing by 1e-310.
- `vr cadis` refuses to write windows when an adjoint solve did not
  converge (`--allow-nonconverged` writes them anyway), and `uq
  propagate` requires the nominal and every finite-difference probe
  solve to converge.
- OpenMC execution is bounded: `openmc run --timeout-seconds` (default
  48 h) kills and reaps a run that exceeds it, and stdin is closed. The
  workbench's run panel starts each job in its own process group and
  cancels the whole group, so OpenMC launched by the CLI no longer
  outlives a cancel.

### Security

- `import openpint` reads workbook-referenced files only from inside the
  workbook's directory: absolute paths, `..` components, symlinks that
  lead outside, and non-regular files are rejected.
- Dependencies: calamine 0.36 (pulls quick-xml 0.41; RUSTSEC-2026-0194,
  -0195) and rustls 0.23.45 (RUSTSEC-2026-0285) in both lockfiles.
- `SECURITY.md` documents private vulnerability reporting through
  GitHub, the supported versions and the scope.
- Release and CI workflows pin every third-party action to a commit
  SHA, give each job only the token permissions it uses, do not persist
  checkout credentials, and pass workflow values to shell steps through
  environment variables rather than inline `${{ }}` expansion.
  Publishing to crates.io, PyPI and TestPyPI runs in GitHub environments
  that require the owner's approval, and `v*` release tags cannot be
  moved or deleted.

### Changed — release artifacts

- Desktop archives, the web build and the Python wheels and sdist ship
  `LICENSE` and a `THIRD_PARTY_NOTICES.txt` generated from `Cargo.lock`
  by `scripts/third_party_notices.py`: every third-party package on a
  shipped (non-dev, non-build) dependency path with the license texts
  from its published source, plus the OFL of the bundled Noto Sans CJK
  JP subset. Desktop archives also carry `NOTICE`. The Python package
  declares its license as a PEP 639 expression (maturin ≥ 1.9), and the
  release-set check fails if the sdist or any wheel lacks either
  license file.

### Changed — examples

- The notebooks reshape voxel data as `(nz, ny, nx)` — voxels are stored
  x-fastest, and the previous `(nx, ny, nz)` reshape scrambled the
  mid-plane maps — read the corrected layered-head dose, and are
  re-executed against the published wheel.

### Added — pharmacokinetics

- `irradiation-time --pk-samples --pk-bootstrap N` propagates PK fit
  uncertainty into the beam-off answer: each replicate perturbs the
  draws by the fit's residual RMS, refits, and re-solves `t*`, emitting
  a P05/P50/P95 interval per region. Seeded by the samples' SHA-256 —
  identical inputs reproduce the interval byte-for-byte.
- `openbnct pk fit` fits measured concentration draws
  (`openbnct.pk-samples/0.1.0`) into a `pk-model` artifact —
  monoexponential via log-linear least squares, biexponential via a
  deterministic separable-least-squares rate grid with nonnegativity
  rejection. Blood-draw series become a hash-bindable model instead of
  a hand-built curve.
- `openbnct irradiation-time --pk-model` integrates the boron dose
  component under declared per-region concentration curves
  (`openbnct.pk-model/0.1.0`: `C(t) = Σ aᵢ·e^(−λᵢt)` ppm, normalized to
  the concentration the dose map was computed at) and solves the
  *implicit* beam-off time `D(t) = limit` by bisection — replacing the
  fixed-concentration assumption every shipped BNCT TPS makes, which a
  published PK-vs-fixed-T/N comparison shows deviates up to ~11% in
  tumor dose (JRR rraf038). The time-scaled map `O + f(t)·B` is
  evaluated per solver iteration so `D_x`/max statistics stay exact;
  regions without a declared curve fall back to constant-concentration.
  Reports (`openbnct.pk-irradiation-report/0.1.0`) pair each PK answer
  with the static answer, the relative deviation, and the concentration
  ratio at beam-off, all hash-bound to both the dose artifact and the
  PK model. A worked demonstration on the boron-loaded `nf-bnct-003`
  case lands in `benchmarks/synthetic/nf-bnct-003/planning/`.

### Added — interoperability

- `openbnct irradiation-time` accepts `dN` dose-coverage limits
  (`--limit brain=d2:13`, `d50:2.5`) — the `D_x` volume-quantile
  endpoints OpenPINT's limiting-OAR constraint scheme uses
  (`LimitMetric::DoseCoverage`). A committed reproduction of their
  published brain-limited endpoint on the layered-head phantom lands in
  `benchmarks/synthetic/layered-head-phantom/planning/`.
- `openbnct import openpint` ingests an OpenPINT Excel treatment
  workbook (`PlanConfig.from_excel` layout) in one pass: the `bnct`
  sheet's component NIfTIs lift into a physical dose bundle, every
  `GTV`/`CTV`/`PTV`/`HOM`/`OAR` structure mask rasterizes to a
  `RegionMask` on the bundle grid, and a
  `openbnct.openpint-plan-summary/0.1.0` artifact records per-structure
  boron concentrations, OAR dose constraints, optional hadron courses,
  and the workbook's SHA-256.

### Added — examples

- `examples/notebooks/` — a three-notebook Jupyter cookbook against the
  published wheel (quickstart, biological modeling, evaluated-covariance
  uncertainty), executed in place so committed outputs are evidence they
  run.

### Added — Avify Dose connector

- `openbnct-avify` (new crate, debuting at 0.2.1) is the optional
  connector for the separately licensed Avify Dose engine — not
  distributed with OpenBNCT. `openbnct avify export-plan` writes the
  engine's voxel plan (npz class map + ROI masks + metadata, plan JSON)
  from a transport case, material assignment and
  `openbnct.avify-spec/0.1.0`; `avify verify` runs the engine as a
  bounded child (timeout kills and reaps), prints the returned per-ROI
  certified `[L,U]` intervals vs criteria, and writes
  `openbnct.avify-run/0.1.0` — a receipt binding every input and
  artifact by SHA-256 with engine version, measured
  export/engine/bind/total timing, resource records, `cold_start`
  (first-run vs repeat) and non-fatal `warnings` including
  engine-version drift against the spec's optional `engine_version`
  pin. `avify status` re-checks bound inputs (CURRENT/STALE), `avify
  show` renders a certificate, `avify diff` compares two runs, and
  `avify review` writes `openbnct.avify-review/0.1.0` — a hash-bound
  reviewer marker that reports STALE when certificate bytes change.
- The workbench gains an Avify (Experimental) workspace: loaded-case
  carry-over, explanation card, first-visit five-slide concept tour
  (persisted seen-state, replayable), bounded run with process-group
  cancel, certificate + receipt display, staleness recheck, compare-to-
  run, review marker, and a synchronized tri-planar class-map view with
  ROI contours and verdict tinting.
- Python: `avify_export_plan`, `avify_verify`, `avify_status`,
  `avify_load_certificate`, `avify_diff`, `avify_review` — JSON
  passthroughs over the same Rust pipeline, never re-derived.
- `examples/avify/` holds a runnable export example and a documented
  R12-06 walkthrough (ordinary result vs uncertainty envelope with
  measured costs).

### Added — interoperability

- `openbnct export phits` emits a full PHITS input deck from a
  transport case + `--assignment`: `voxel_box` regions become carved
  RPP cells, `voxel_set`/`voxel_fractions` become `LAT=1` lattices with
  per-material universe `FILL` blocks (i-fastest ordering), rectangular
  plane sources (`s-type=2`), tabulated spectra (`e-type=1`), and
  neutron+photon `[t-track]` tallies.

## [0.2.0] — 2026-09-22

### Changed

- Project license changed from Apache-2.0 to MIT (LICENSE, all SPDX
  headers, crate/Python package metadata, badges, NOTICE, and the
  contributing/disclaimer/citation references). The IP-boundary review
  obligation in `docs/IP_BOUNDARY.md` is unchanged; the doc now states
  that MIT carries no express patent grant.

### Fixed — transport solver

- The S_N sweep's spatial closure is now theta-weighted diamond
  difference with the weight chosen per cell/direction/group from the
  axis optical thickness to reproduce exact exponential transmission
  for a pure absorber (`θ(τ) = (τ−(1−e^{−τ}))/(τ(1−e^{−τ}))`; θ→½ on
  thin cells, recovering plain DD). The previous positivity clamp
  zeroed negative extrapolated edges — annihilating particles on cells
  several mean-free-paths thick and systematically under-transporting
  the deep dose tail; a first step-difference fixup was trialled and
  rejected for over-transmission at moderate τ. The θ weight depends
  only on (σ, Δ, μ) so forward and adjoint sweeps stay dual (adjoint
  reciprocity test preserved). Affects all solves through the shared
  sweep — forward, adjoint, photon, and perturbed UQ runs.

### Fixed — biology layer (external review)

- Overlapping region masks no longer resolve alphabetically: every
  voxel matching more than one declared region is a hard error naming
  the regions unless the model declares `region_priority` (new optional
  field on `biological-model/0.2.0`, `microdosimetric-model/0.1.0`, and
  `isoeffective-model/0.1.0`; earliest name wins). Nested-ROI cases —
  tumor inside an OAR mask — previously took the lexicographically
  first region's weights/LQ silently.
- `endpoint utcp` now accepts TCP and NTCP evaluations over *different*
  regions — the standard uncomplicated-control pairing (tumor TCP ×
  OAR NTCP). `--ntcp` is repeatable: `p_plus` computes
  `TCP·Π(1−NTCPᵢ)`; the evaluation records `tcp_region`,
  `ntcp_regions`, `ntcp_terms`, and content bindings for every input.
- MKM photon-equivalent dose is now computed by combining the
  mixed-field effect first and inverting the photon LQ once
  (`X = Σ(α₀+βᵢ·z̄₁D,ᵢ)·dᵢ + (Σ√βᵢ·dᵢ)²` — Zaider–Rossi √β cross
  terms), instead of inverting each component separately and summing.
  The old path overestimated isoeffective dose ~10–20% at clinical
  dose levels (Jensen gap). Per-component volumes now report each
  component's effect-share of the isoeffective dose and still sum to
  the total exactly. `endpoint evaluate`/`dose-metrics`/DVH acceptors
  now normalize legacy `nctforge.*` schema ids consistently.
- `BiologicalModel` gained `component_weight_uncertainty` (σ_w/w per
  component): declared CBE/RBE uncertainty folds into the biological
  total σ in quadrature with the transport uncertainty.

### Added

- `openbnct sn` — the in-house deterministic multigroup discrete-ordinates
  solver and data pipeline, new since 0.1.1: `sn collapse` folds processed
  pointwise HDF5 evaluations into `openbnct.multigroup-data/0.1.0`
  (P0+P1 transfer matrices, l≤5 Legendre moments, S(α,β) bound-atom
  thermal kernel for H in water/Lucite with upscatter, self-shielding,
  boron microdistribution); `sn solve` sweeps level-symmetric quadrature
  on the case grid with an uncollided-beam split, voxel fill-fraction
  material blending, transport correction, thermal-upscatter outer
  iteration with Anderson acceleration, symmetric group sweep, and
  Rayon-parallel ordinate loops; `sn fold` reuses a flux artifact;
  `sn photon-collapse`/`sn photon-solve` run the one-way-coupled n→γ
  problem (photon σt, Klein–Nishina, coherent, pair→annihilation, kerma)
  through the same sweep. Adjoint solves drive CADIS importance maps
  (`vr`) and plan-direction scoring.
- `openbnct accelerator` (parametric accelerator-target neutron source)
  and `openbnct bsa` (beam-shaping-assembly rasterize + parameter sweep).
- Verification surfaces: `openbnct gamma` (Low γ index between dose
  bundles on a frozen case), `openbnct analytic` (declared closed-form
  oracles — the NF-BNCT-003 exponential-attenuation oracle fits at
  2.4e-16 relative deviation under the weighted-diamond closure), and
  `openbnct metamorphic` (symmetry-relation oracles).
- `openbnct prompt-gamma` (`openbnct.prompt-gamma-source/0.1.0`):
  voxelwise 478 keV production map from a dose bundle's boron component
  — the physics source term Compton-camera / BNCT-SPECT verification
  research consumes; committed layered-head artifact under
  `examples/prompt-gamma/`.
- `openbnct export phits`: PHITS input-deck emitter on the same
  transport-neutral case (Z-disk/monoenergetic emit subset with named
  refusals for unrepresentable features).
- `openbnct dicom import-mr` (MR series → rescaled-intensity NIfTI on
  the shared geometry path), RTSTRUCT ROI-contour → `RoiMask`
  rasterization, and `openbnct dicom calibrate` (above).
- `openbnct import labelmap` (NIfTI labelmap + materials table →
  case + assignment — the on-ramp for segmented phantoms from 3D
  Slicer / ITK-SNAP) and `openbnct beam build` (binned CSV →
  `beam-description`); walkthrough in `docs/BYOC.md`.
- `openbnct plan directions`: azimuth×elevation candidate enumeration
  ranked by tissue-path length to the aim mask — the zero-transport
  pre-filter — with optional adjoint-importance scoring when multigroup
  data is supplied; emits `plan fields --beam` spec lines directly.
- `openbnct` Python bindings for the full biology layer:
  `load_isoeffective_model`/`make_isoeffective_model`/`apply_isoeffective`
  and `load_microdosimetric_model`/`make_microdosimetric_model`/
  `apply_mkm_model` (dict-authored constructors share the file path's
  validation), with `MicrodosimetricModel`/`IsoeffectiveModel` classes
  and `.pyi` stubs.
- The dual-target workbench (`openbnct-gui`): one egui codebase compiled
  to native and wasm32 — deployed to GitHub Pages — with WebGL fallback
  where WebGPU is unavailable. Tri-planar dose-map viewer driven by the
  bundle's own grid, line-profile plots with peak-normalized
  measurement overlay, log-log source-spectrum viewer with TECDOC
  region integrals, per-component σ-map toggle and plan-robustness
  violation cards, A/B dose-bundle diff with ratio map, a bounded
  native run panel (cgroup-scoped child processes with cancel +
  deadline and solver presets), five-language UI
  (EN/JA/IT/ES/zh-Hans), an accessibility pass (labelled canvases,
  keyboard crosshair, reduce-motion), and bundled example artifacts so
  a cold visitor lands on a real dose bundle.

### Fixed

- Dose-uncertainty-budget NaN fields round-trip as JSON null (the GUI
  renders them as an em-dash).
- Finite-difference perturbation validation in the UQ path tightened —
  invalid step specifications are rejected instead of silently
  producing degenerate sensitivities.

- `openmc cov-endf` NC-type LTY=0 resolution: a reaction's covariance
  declared as coefficient-weighted references to derived-quantity
  sections (MT ≥ 800) now resolves — contributors are resampled onto a
  union mesh and summed. This unlocks real evaluated (n,α) covariance:
  ENDF/B-VIII.1 ¹⁰B MT=107 propagates to a 0.34% σ_rel boron-dose
  budget on the layered-head phantom (artifacts in
  `examples/covariance/`). `--parameter dose_response --component`
  binds reaction covariances to component kerma responses instead of
  σ_t. LTY ∈ {1,2,3} cross-material covariances remain skipped with a
  ledger entry.
- `openbnct.isoeffective-model/0.1.0`:: the González & Santa Cruz (2012)
  photon-isoeffective dose — per-component dose-independent factors
  (`rbe`, `rbe_beta`), tissue photon LQ (`alpha_0`/`beta`, per-region
  overrides), and an `irradiation` block producing the Lea–Catcheside
  repair factor `G = 2(μT−1+e^(−μT))/(μT)²` for protracted delivery.
  `X = α_γ·Σrbeᵢ·dᵢ + G·β_γ·(Σ√rbeβᵢ·dᵢ)²` is inverted once against
  the photon LQ. Fixed-weight `photon_isoeffective` semantics on
  `biological-model` remains supported but is now documented as the
  dose-independent-factor approximation, not the IsoE formalism.

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
- Fixed a silent-dose bug for Y-axis disk sources: the boundary-flux
  source map and the uncollided ray-trace built their transverse
  `(u, v)` keys as `((axis+1)%3, (axis+2)%3)` — `(z, x)` for Y — while
  the sweep looks them up in canonical order `(x, z)`, so a Y-face beam
  never landed on its cells and delivered ~zero dose. All three sites
  now share `PlaneAxis::in_plane_axes()` ordering; regression test
  `y_axis_disk_source_deposits_in_the_right_column` covers it.
- FiR 1 K63 fine-spectrum source: `beams/fir1-k63-ineel-spectrum.json` —
  a 120-bin energy histogram digitized from the measured INEEL
  iterative-adjustment curve (INEEL/EXT-01-00204 Fig. 5), anchored to the
  VTT LSL-M2 experimental region integrals; method + digitized markers +
  disclosed anchor factors in
  `beams/fir1-k63-spectrum-ineel-digitized.json`. PMMA rerun
  (`*-ineel` artifacts): normalized χ² unchanged (~21), absolute χ²
  25.3 vs 28.3 — the absolute-scale residual is NOT a source-shape
  artifact; the deficit sits in transport/data fidelity or the port-rate
  normalization. Water-phantom rerun at TSL+P1 confirms on a second
  composition (`validation/fir1-k63-water-phantom/*-ineel` artifacts):
  thermal-peak depth moved 1.25→1.75 cm toward the measured 2.25 cm,
  yet the deep tail fell further — spectral resolution does not close
  the gap. Hypothesis closed, better-provenanced source retained.
- Anisotropic-scattering attribution on PMMA: a moments-emitting
  collapse (`multigroup-data-28g-moments.json`, l = 2..5) plus
  `--p1 --anisotropy 5` solves decomposed the measured-profile
  residual — peak-normalized χ² fell 20.95 → 1.92 at S8 (S16 adds
  nothing: 3.34). The isotropic-scatter approximation was the
  dominant *shape* error. A TSL+moments collapse (H(Lucite)
  bound-atom kernel + realizability-clamped l≥2 moments — fixing a
  defect where free-gas stand-in moments violated |Σ_sl| ≤ Σ_s0 on
  upscatter pairs) restores most of the deep thermal tail:
  normalized χ² = 1.06 (vs 20.95 isotropic). Surviving residuals:
  a flat ~0.33 absolute scale through 5 cm (port-rate
  normalization candidate) and a ~0.3–0.45 deep-tail ratio beyond
  ~8.75 cm.
- Coupled-photon verification, second phantom + mesh sensitivity: PMMA
  `sn photon-collapse`/`photon-solve` artifacts at 27- and 12-group
  meshes (`validation/fir1-k63-pmma-phantom/photon-{data,dose}-{12,27}g`).
  Transported dose ≈ 0.21× local-kerma integral (escape-dominated), and
  the 12↔27 group spread is a uniform ~13% — photon discretization is
  not the n→γ deficit driver; the residual is physical redistribution
  plus the upstream neutron-field deficit, consistent with the
  water-phantom comparison. A calibrated-tissue OpenMC cross-check
  remains data-blocked: the local HDF5 library carries no Ca/Cl/K/Mg/Na/
  P/S tables and no `openmc.data` conversion path is installed.
- `openbnct plan robustness` / `openbnct.plan-robustness/0.1.0`: plan-level
  systematic-uncertainty propagation — declared per-component σ sources
  (`--relative`, `--positioning-sigma-mm`, `--boron-field`) fold through
  each objective's isoeffective weights and the optimized beam weights
  into a first-order fully-correlated metric 1σ and a one-sided Gaussian
  violation probability per objective. The supplied objective document is
  hash-verified against the result; worked report on the layered-head
  direction search (`planning/robustness-direction-search.json`).
- `openbnct dicom calibrate` / `openbnct.hu-calibration/0.1.0`:
  HU-to-material calibration — a versioned anchor table (declared
  `MaterialDefinition` per Hounsfield value, Schneider-method
  two-component mixing between anchors) applied to CT slices or an
  HU NIfTI, emitting a `material-assignment` the solver blends
  per-voxel; committed layered-head round-trip recovers the phantom
  assignment voxel-exact and solves bit-identically.
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
  suitability is claimed. See `docs/DISCLAIMER.md`.
