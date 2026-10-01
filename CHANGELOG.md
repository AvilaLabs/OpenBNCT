# Changelog

All notable changes to OpenBNCT are documented here. The project follows
[Semantic Versioning](https://semver.org/); schema documents carry their
own versions independent of the crate version.

## Unreleased — user handbook (2026-10-01)

- Added the hosted handbook at `https://openbnct.avilalabs.org/docs/`, with current task guides, source/release distinctions and benchmark interpretation. README and GUI Help link to it; the Pages bundle publishes it beside the workbench.
- The CLI and Python workflows are covered by their handbook chapters. A runtime command or Python function for opening this hosted documentation is deliberately excluded because it adds no calculation or analysis operation.
- Native debug captures can set `OPENBNCT_CAPTURE_HELP=1` with `OPENBNCT_CAPTURE` to review the Help window against a loaded case.




## Unreleased — CT crop connectivity (2026-10-01)

- Body crop uses 6-connectivity (face neighbours): 26-connectivity let thin bridges
  such as head supports join the body and widened the real-head crop box. The
  `validation/real-anatomy/hn-head-ct` project predates automatic cropping; set
  `[imaging] crop = "none"` to reproduce its committed results.

## Unreleased — automatic CT crop (2026-10-01)

- **`dicom import-ct` and `import ct-nifti` crop the CT to the body by default.** A real
  scan includes shoulders, couch and air; the benchmark head's full field of view is
  1.6 M voxels at 4 mm. `--crop body` (default) thresholds HU > -400, keeps the largest
  6-connected component (the couch drops out), fills holes per axial slice and builds
  the covering grid over its bounding box plus `--crop-margin-mm` (default 15, rounded
  out to whole CT voxels). `--crop none` keeps the full field of view;
  `--crop-box-mm x0,x1,y0,y1,z0,z1` gives an explicit LPS box; `--crop-superior-of-mm Z`
  keeps only z >= Z (drops shoulders). ROI masks use the cropped grid; an ROI the crop
  clips prints a warning with the number of dropped CT voxels. The import record has a
  `crop` block (rule, parameters, original and cropped extents, shapes, voxel counts, per-ROI
  dropped counts).
- **Projects:** `project.toml` `[imaging]` gains `crop = "body" | "none"` and
  `crop_margin_mm`; `project init` writes the defaults and `project run` passes them to
  the import. A `project.toml` without the keys now crops (default `body`); set
  `crop = "none"` to reproduce an earlier uncropped import.
- **`beam bind --hu HU.nii`** reports the air gap in front of the first tissue voxel in the
  port footprint and warns when the entry layer already touches tissue (margin too
  small). `project run` passes the HU volume.
- **Surfaces:** CLI only for the flags. The project keys are shared with the GUI project
  workspace (wired separately). The Python package has no project API today (the project
  workflow is CLI/GUI-only), so there is deliberately no Python surface for this change;
  a Python project API is a roadmap item under R18-06.

## Unreleased — GUI Project workspace parity (2026-09-30)

- **The desktop Project workspace now covers what `openbnct project` does.**
  - CT input: a DICOM folder (CT + RTSTRUCT or DICOM SEG) or a CT volume file set
    (`project init --ct-nifti --labels-nifti --label-names`; NIfTI only, and all
    three files are required, as on the CLI). "Scan ROIs" lists the names in the
    label-names JSON.
  - Import: spacing, plus `[imaging] crop` / `crop_margin_mm`.
  - Advanced: quadrature order (S4/S8), `photon_transport`, `source_weighting`.
  - Beam: the `project builtins` list (default `fir1-k63-ineel`) or a
    beam-description JSON path.
  - "Verify with Monte Carlo" runs `openbnct project verify <dir> --particles N`
    through the bounded job runner (stage-line progress, Cancel); the results view
    renders the report's "Independent Monte Carlo check" section as a table with a
    coloured verdict banner and status cells.
  - "Load boron dose" now uses the core sidecar-aware loader, so dose bundles whose
    arrays live in binary sidecars (more than 1M values) load in the Dose workspace.
  - Strings are English and Japanese.
- **Gated by the installed CLI.** The crop controls and the `[verify]
  variance_reduction` choice stay disabled (with a tooltip) until `openbnct project
  init --help` mentions `crop` / `project verify --help` mentions variance
  reduction, because the project runner rejects unknown `project.toml` keys.
- **Deliberately CLI-only for now:** a phase-space (IAEA) beam source. A project's
  `[beam] description` takes a built-in or a beam-description JSON, not a phase-space
  header; bin the header with `openbnct beam phsp-bin` and pick the result under
  "Beam-description file". The GUI shows the entry disabled with that explanation.
  CMFD is not a `project.toml` option, so the GUI has no control for it.
- **Headless capture:** `openbnct-gui --workspace project --project-dir DIR
  --screenshot OUT.png [--wait-seconds N]` (also `--load-dose`, `--prefill-volume`,
  `--scroll-results`, `--advanced`, `--openbnct`) writes one PNG of the window and
  exits. Screenshots under `docs/screenshots/project-*.png` come from it.

## Unreleased — real-anatomy mixture quantization (2026-09-30)

- **HU-calibrated mixtures are quantized before blending.** A real head CT at 4 mm
  (206k voxels) produced 91 695 distinct `voxel_fractions` signatures, and the S_N
  solver built one blended material (scatter matrices, sweep tables) per signature:
  the neutron solve had not finished after 2 h 11 min at 3.9 GB. Fractions are now
  quantized to 20 levels (largest remainder, the same rule the OpenMC realization
  uses, now shared as `openbnct_transport::quantize_fractions`), giving ~150
  materials; the same solve converges in 18 outers (~20 min on 6 threads, 2.3 GB).
  `OPENBNCT_MIXTURE_LEVELS=0` restores exact fractions.
- **`sn photon-solve` converges on real anatomy.** On the same head the coupled photon
  solve stalled (residual 0.52 after 8 outers): Compton-dominated groups 11–12 did
  not finish their inner iterations in a pass. CMFD is now on by default for the
  photon solve (`--no-cmfd`), the exponential closure is off (`--exp-source`), the
  outer budget is 32 and per-outer progress prints (`--quiet`). The head converges
  in 12 outers.
- **Mixed voxels blend dose responses by mass, not volume.** Dose responses are mass
  kerma; `voxel_fractions` blends weighted them by volume fraction, so at air/tissue
  boundaries air's nitrogen (75 % by mass at ~1/1000 the density) dominated the
  per-gram nitrogen kerma — skin nitrogen dose was 1.56× continuous-energy MC on a
  real head while boron was 1.06×. Weights are now f_i·ρ_i / Σ f_j·ρ_j (densities
  from the case and assignment material definitions; volume weights only when a
  density is unknown). Macroscopic cross sections still blend by volume.

## Unreleased — deterministic-transport accuracy fixes (2026-09-29)

- **IAEA phase-space beam sources (R18-11).** `beam phsp-info` reads a
  `.IAEAheader`/`.IAEAphsp` pair (streaming, constant or stored variables, both
  byte orders); `beam phsp-bin` bins its neutrons onto a case face by pixel x
  direction x multigroup energy group into an `openbnct.phase-space-source/0.1.0`
  table with file hashes, particle counts and rejection accounting. A new
  `phase_space` source space feeds the deterministic uncollided beam (the disk/cone
  path is unchanged), and `openmc generate` converts the original particles to an
  OpenMC source bank so `project verify` compares like with like. See
  `docs/USAGE.md` "Phase-space beam sources".
- **Raw binary sidecars for large flux and dose arrays.** Multigroup flux,
  physical/biological dose bundles and boron unit doses may store their large
  arrays as raw little-endian `f64` sidecar files referenced from the JSON by
  `{"external": {"path", "sha256", "len", "dtype"}}` (additive; inline arrays and
  all schema versions are unchanged). Writers externalize above
  `OPENBNCT_SIDECAR_MIN_VALUES` values (default 1,000,000; `0` always, `-1`
  never); all loaders verify length and SHA-256, `evidence export/verify` and
  `bench verify` treat sidecars as bound files. See `docs/ARCHITECTURE.md`.
- **Fine-mesh multigroup CMFD for the S_N solver, on by default** in `sn solve`,
  Python `sn_solve` and `openbnct project` (`--no-cmfd` / `OPENBNCT_CMFD=0` to opt out;
  `--cmfd-inner-sweeps N`, default 2; `--cmfd-damping`). The library `SnOptions`
  default stays off.
  Each outer does a few transport sweeps per group, builds the
  transport-consistent D-hat closure from the sweep's net face currents (od-CMFD
  stabilization after Zhu, Xu & Downar, Ann. Nucl. Energy 2016; the paper's
  theta(tau) polynomial was not available, so a smooth surrogate with the
  published limits is used - see `cmfd.rs`) and solves the low-order multigroup
  system (the upscatter block as one Krylov solve with an exact per-cell energy
  block preconditioner), then replaces the flux by it and rescales the P1
  currents, kernel moments and wrap planes. The sweep's exact balance defect,
  boundary partial currents and the periodic wrap mismatch ride on the
  low-order right-hand side, so the transport fixed point is an exact low-order
  solution. Replaces the coarse-mesh rebalance when on. Layered-head phantom
  (25^3, S4, 28 groups, P1, Anderson 3, 4 threads): 21 -> 14-20 outers,
  2357 -> 560-646 group sweeps (3.6-4.2x), solve loop 37.5 s -> 15.5 s (2.4x),
  converged dose within 3.5e-6 (region means) / 1.4e-5 (voxels >1 % of max) of
  the default path; 50^3 (6 threads): 2506 -> 668 sweeps, solve loop 249 s ->
  111 s. The ~10 s (25^3) / ~70 s (50^3) uncollided-beam ray trace ahead of the
  iteration is unchanged and bounds the end-to-end gain (48 -> 27 s, 319 ->
  181 s). Off by default. `SnStats` (in `SnOptions.stats`) counts group sweeps,
  low-order solves and guard events.

- **Project workflow: histogram-consistent beam and transported photon dose.**
  `project init` now defaults to `builtin:beams/fir1-k63-ineel` (the 120-bin
  log-spaced FiR 1 reconstruction; `builtin:beams/fir1-k63` stays resolvable) and
  the runner passes `sn solve --source-weighting` from the new
  `[transport] source_weighting` key (default `uniform_in_bin`, the OpenMC/MCNP
  within-bin convention; the old 1/E spread put the 10 keV-16.9 MeV bin's mean at
  ~2.3 MeV instead of ~8.5 MeV and made the fast-neutron dose ~0.3x MC). New
  `[transport] photon_transport` (default true): after `sn solve` the runner runs
  `sn photon-solve` on the converged neutron flux and the new `sn
  merge-photon-dose`, which replaces the local capture-gamma kerma in the dose
  bundle by the transported photon dose (total adjusted by the same difference;
  the replacement is recorded in the bundle's `provenance_id`). `false` keeps the
  old local deposition; the report states which was used. New builtin
  `tissue/multigroup-photon-data-16g` (`libraries/tissue/collapse-photon-16g.sh`)
  built from the full ENDF/B-VIII.1 photon library; `sn photon-collapse` now
  accepts any element with a photon-atomic table (was H/B/C/N/O only).
  The builtin beam is `beams/fir1-k63-ineel-20mev.json` (generated by
  `beams/make-ineel-20mev.py`): the 120-bin spectrum clipped at 20 MeV, the
  ENDF/B-VIII.1 upper edge (clipped weight 2e-9), because the OpenMC input check
  rejected the 40 MeV edge; that check now accepts a source edge equal to the top
  of the data range. Measured with `project verify` (synthetic study, S8, 5e6
  histories), S_N/MC structure-mean ratios boron/hydrogen/photon: whole phantom
  0.862/0.983/0.932, target 1.062/0.898/1.033 (before: 0.895/0.332/3.96 and
  0.845/0.274/2.87 at 1e6).

- **`openbnct project verify`.** One command re-computes a finished project with
  continuous-energy OpenMC on the same case, assignment, source and boron
  concentrations (step `08-verify`, `out/08-verify/`), runs `compare` and `gamma`
  against the S_N result, writes `ratio-<component>.nii`, and adds an "Independent
  Monte Carlo check" section (per-structure S_N/MC ratios, gamma pass rates, MC
  statistics, a verdict with explicit thresholds) to `report.md`/`report.json`.
  Optional `[verify]` table in `project.toml`.
- **Declared S(α,β) in OpenMC decks.** `openmc generate|run --thermal-scattering
  NUCLIDE=TABLE` (unit-mass-fraction profile) emits `<sab>` for every material with
  the nuclide and records the table and its library hash in the input manifest, so
  the MC side matches the deterministic tissue data's H-in-H2O kernel.
  `openmc data select-manifest` derives a case-scoped data manifest; `openmc run`
  gains `--boron-unit-dose-output`.

- **Collapse double-counted redundant reactions in σ_t.** `sn collapse` summed every
  MT 3–299 section into removal, including OpenMC-HDF5 reactions flagged `redundant`
  and ENDF summation/production MTs (4 with levels present, 101, 201–207, 251–253).
  H-1's MT 204 (deuteron production) equals its (n,γ), so hydrogen capture — the
  dominant thermal absorber in tissue — was counted twice: collapsed thermal
  absorption ~2× too high and the thermal flux ~2× too low even in an infinite
  medium. Redundant sections are now skipped (the lumped 102–107 channels are
  kept). Dose-response vectors are unchanged; σ_t drops by up to ~20% in thermal
  groups for H-bearing tissue. Multigroup data collapsed before this fix should be
  re-collapsed.
- **Transport correction broke slowing-down.** Since 550c7d3 the P1-matrix path
  subtracted σ_s1(g→g') from every downscatter transfer, treating energy-losing
  forward collisions as uncollided flight in group g. The correction must be an
  identity on the scalar flux in an infinite medium; this one overstated the
  epithermal flux ~1.8× in tissue and starved the thermal field (layered-head
  absorption fell from ~0.6 to ~0.07 per source neutron). The correction now removes
  the forward lobe from the within-group element only, capped at σ_s(g→g), which
  keeps σ_a,eff ≡ σ_a (no fabrication). The infinite-medium oracle test now requires
  every option combination to reproduce the uncorrected fixed point.
- Infinite homogeneous brain (28 groups, H(H2O) S(α,β)) vs continuous-energy OpenMC
  after both fixes: thermal 0.97×, epithermal 1.04×, fast 0.89×.
- **Uncollided beam attenuated void as tissue.** The analytic uncollided beam used the
  destination cell's σ_t over the whole path from the source plane, so off-axis rays
  crossing air/void before the skin were attenuated as tissue. It now traverses the
  voxel grid (Amanatides–Woo) and sums each material's σ_t·ℓ. Layered-head absorbed
  fraction 0.055 → 0.165 (P0, S8).
- **P1 scattering is now the default** (`sn solve`, Python `sn_solve`, and therefore
  `openbnct project`) whenever every scattering material carries P1 moments; `--p0`
  forces the old path. A transverse-periodic brain column vs OpenMC: P0 + transport
  correction absorbs 0.27 per source vs 0.43 (MC) and under-penetrates with depth;
  P1 absorbs 0.45. Under P1 the CLI and Python now leave the exponential
  within-cell closure off by default (its λ refits kept P1 from converging in 3-D;
  `--exp-source` re-enables it, e.g. for optically thick cells where plain diamond
  difference overshoots with depth). `sn solve --anderson` defaults to 3. Open:
  make the closure and P1 converge together.
- **Layered head vs continuous-energy OpenMC** (S4, 28 groups, new
  `multigroup-data-28g-v5-tsl.json`, `--source-weighting uniform_in_bin`): P1 region
  means 1.03–1.08× (boron brain 1.08, nitrogen skin/skull/brain 1.07/1.04/1.03),
  on-axis nitrogen 0.99–1.16× from entrance to exit; P0 with the exponential
  closure 0.76–0.97× and 0.33× at depth. Before these fixes the default path was
  ~0.15× and the committed September v2 result ~1.6× (particle fabrication).
  See `validation/intercomparison-layered-head/2026-09-29-accuracy-fixes.md`.
- Standard tissue library re-collapsed with the fixed collapse and H(H2O) S(α,β)
  (now the `project init` default); project runner defaults `anderson = 3`,
  `max_outer = 128`. Reports carry an accuracy-status line.
- Correction to the first entry above: absorption per source neutron in the
  continuous-energy reference is ~0.19 (the reference tallies are already per cm³).

## [Unreleased]

### Added — `openbnct project`

- `project init` / `project run` / `project status` / `project builtins`:
  CT + RT Structure Set to component dose, boron-scaled dose, per-structure
  metrics, DVH CSVs and `out/report.md` in two commands, driven by a
  `project.toml`. Each step is the same handler as its individual command
  (equivalent command lines are recorded and printed in the report); a
  resumable `out/run-manifest.json` (`openbnct.project-run/0.1.0`) skips
  steps whose inputs, command and outputs still hash-match. Built-in tissue
  library, 28-group data and FiR 1 K63 beam are embedded in the binary and
  copied into the project with sha256. Report schema
  `openbnct.project-report/0.1.0`.
- `beam bind --aim-mask MASK --approach=+x`: aim the bound disk source at a
  mask centroid.

### Added — Python: NumPy arrays and `sn_solve`

- Voxel fields in the Python package return C-order `np.ndarray`s of shape
  `(nz, ny, nx)` through `as_array()` / `uncertainty_array()` (multigroup
  flux: `(groups, nz, ny, nx)`), with geometry attributes; the list-returning
  accessors are unchanged. `numpy` is now a package dependency.
- `openbnct.sn_solve(...)` runs the deterministic S_N solve, dose fold and
  boron unit-dose fold from Python through the same library functions as
  `openbnct sn solve`, releasing the GIL. New wrappers: `TransportCase`,
  `SnSolution`, `BoronUnitDose`, `BoronField`; `boron_dose` accepts loaded
  objects as well as paths.
- `OpenBnctError` is the primary exception; `NctForgeError` remains as an
  alias of the same class.

### Fixed — OpenMC collection

- `openmc collect` no longer multiplies folded neutron components by the
  region/base density ratio. The folded responses are mass KERMA (Gy cm^2),
  which is density-free at fixed composition, so a denser same-composition
  region was over-reported in proportion to its density. Only the
  covered-nuclide mass-fraction ratio remains. Committed OpenMC evidence is
  unaffected (its only assigned region has the base density).

### Added — OpenMC multi-material decks

- Component profile with `unit_mass_fraction_kerma_fold` and
  `native_heating_residual` estimators lets `openmc generate` accept a
  HU-calibrated multi-tissue assignment: B10/N14 unit-mass-fraction folds
  scaled per voxel by the realized mass fraction, hydrogen from native
  neutron heating minus the two folds. New flags on `openmc generate` and
  `openmc run`: `--unit-source-component-profile`, `--unit-source-material`,
  `--unit-source-nuclear-data-manifest`, `--mixture-levels`.
- `voxel_fractions` mixtures are realized by level quantization (recorded in
  the input manifest) instead of being ignored; the base-material profile
  now refuses them.
- `openmc collect --boron-unit-dose-output` writes an
  `openbnct.boron-unit-dose/0.1.0` artifact for such decks.

### Added — post-hoc boron

- `sn collapse` emits an optional `boron_unit_response_gy_cm2_per_ug_g`
  vector (tissue-independent ¹⁰B kerma per µg/g; additive optional field
  of `openbnct.multigroup-data/0.1.0`). `sn solve`/`sn fold
  --boron-unit-output` write the new `openbnct.boron-unit-dose/0.1.0`
  artifact, and `boron dose` (CLI and `openbnct.boron_dose` in Python)
  applies a blood concentration with tissue:blood ratio masks, or a
  `boron-field`, to re-total a physical dose bundle without re-solving.
  Trace-¹⁰B approximation (no flux depression from the applied boron).

### Added — transport

- Exponential within-cell source reconstruction (`SnOptions.exp_source`,
  default on; `OPENBNCT_NO_EXP_SOURCE` A/B kill-switch): in optically
  thick cells (σ_t·Δ > 1) the direction-free source is fit per axis as
  q ∝ e^{λx} toward each outflow edge, and the edge flux takes the
  exact exponential-source value q(edge)/σ_eff with σ_eff = σ + μλ —
  applied as a ratio on the θ-WDD source share so multi-axis coupling
  and the λ → 0 limit are exact, and non-monotone source triplets fall
  back to θ-WDD identically. Fixes DD's asymptotic-preservation
  failure (thick near-conservative cells mix the whole inflow to the
  cell mean and over-transport a declining tail — ~1.47×/cell runaway
  in the controlled probe). On the FiR-1 same-data P0 arm the deep-
  thermal S_N/OpenMC pileup is eliminated (2.5–2.8× → 0.96–1.10 at
  z = 12–21); under P1 it halves (3.3× → ~1.4). Under a directional
  source (P1 dipole or l ≥ 2 kernel) the reconstruction fits each
  direction's source *per component* — the dipole and kernel fields
  carry their own per-direction rates (`OPENBNCT_NO_DIR_LAMBDA`
  A/Bs them): in the P1 probe column the tail drift halves
  (~2.7 %/cell → ~1.4 %/cell). The phantom residual is unchanged —
  it is sourced by the epithermal mid-depth hump below the σ_t·Δ
  gate, not by the deep-cell closure. The reconstruction is rebuilt
  per inner-iteration series (inner map stays affine) and frozen
  after a 16-outer warmup — a λ↔φ lag otherwise sustains a period-2
  oscillation that stalls the slow thermal mode; the frozen arm
  converges at the baseline rate.
  New regressions `thick_cell_conservative_column_probe` and
  `thick_cell_conservative_column_probe_p1` pin both directions
  (corrected tail bounded in [0.5, 2.0], legacy runaway > 50× /
  > 8× under P0 / P1).
- Declared source-spectrum interpolation: `sn solve --source-weighting
  {collapse_consistent|uniform_in_bin}` selects the within-bin spread
  of a `TabulatedHistogram` source — Maxwellian-below-0.5 eV / 1/E
  above (collapse-consistent, the default) or uniform-per-eV matching
  OpenMC `Tabular(interpolation="histogram")` and MCNP histogram
  semantics for CE cross-code comparison. The selected convention is
  recorded on the emitted flux artifact's `source_spectrum_weighting`;
  on the layered-head intercomparison it closed the fast-bin deficit
  from 0.16–0.43× to 0.50–0.86× of the continuous-energy reference.
- `sn collapse --attenuation-depth <cm>`: multiplies collapse weights
  by `exp(−σ_t,material(E)·z)` — survival weighting for penetrating
  problems; verified to shift this phantom's fast-group σ_t by only
  a few percent (intra-group condensation exonerated as the deep-flux
  deficit mechanism).
- `sn spectrum`: extract a per-cell/material `EnergyDistribution`
  histogram from a flux artifact for transport-informed collapse
  weighting (`sn collapse --weighting-spectrum`).
- `sn solve --allow-unconverged`: writes the provisional field with
  `converged: false` and its residual instead of discarding it —
  diagnostics for slow upscatter-coupled (TSL) outer iterations.
  The artifact also carries `residual_site` — the (cell, group) pair
  with the largest last-iterate relative change — so a stalled solve
  is diagnosable from the file alone.
- `sn solve`: per-group periodic wrap planes (below an internal byte
  cap, shared set above) — each group's inner sweeps and successive
  outers now read that group's own wrap-around face values instead of
  whichever group swept last. The shared buffer's cross-group inflow
  contamination was the period-2 limit cycle's source on periodic
  TSL problems: same-group planes removed it — the S4 water column
  now converges one-step at 24 outer iterations where it previously
  sat in a stable orbit caught only by the parity-midpoint path.
- `openbnct.beam-field-set/0.1.0` `screening` block and `plan fields
  --screen-order/--keep-top` (with `--screen-convergence`,
  `--screen-max-inner`, `--screen-max-outer`): two-stage field sweeps
  score every declared beam at a cheap quadrature/convergence, rank
  by mean aim-mask `physical_total`, and run the full-quality solve
  only on the retained beams. The manifest records every score and
  its retention fate; `beams` lists the retained set.
- `plan optimize`: `metrics` on the objective document reports
  post-hoc plan quality — per-region voxel counts, mean/min/max,
  `dose_at_volume` quantiles, `volume_at_dose` at bounds,
  generalized EUD, and endpoint (TCP/NTCP) probabilities — on the
  `InversePlanResult` artifact, the CLI output, and the GUI
  inverse-plan table. Reported, never optimized; isoeffective
  quantities honor `region_weights`.
- `plan optimize --emit-plan --seconds-per-weight S`: writes
  `duration_s = weight·S` on emitted exposures — beam-on seconds
  under the declared source-strength-scaling convention.
- Python `MultigroupFlux` gains `residual_site` and `scattering_order`
  getters.
- `sn boundaries`: adaptive group-boundary placement by equal
  importance mass over lethargy (R17-04). Reads an `EnergyDistribution`
  histogram (`sn spectrum` extraction, measured beam, or fine-group
  flux collapse), folds an optional same-binned `--response`, and
  emits a `openbnct.boundary-proposal/0.1.0` document carrying
  descending edges plus the placement rationale (per-group mass
  shares and lethargy widths). `sn collapse --boundaries-file`
  consumes the proposal directly. `sn boundaries --uniform-floor`
  (default 0.25) reserves a share of the importance mass for
  uniform-lethargy coverage — the anti-starvation guard measured on
  the water-column experiment, where pure flux importance allocated
  41/56 groups below 0.5 eV and zero above 10 keV.
- Multigroup data carries `beam_sigma_nodes_per_cm`: a 4-node
  uniform-in-eV sub-bin σ_t kernel per group. Under
  `--source-weighting uniform_in_bin` the uncollided deposit is
  `Σ_j w_j·e^{−σ_j·s}` — a broad group's penetrating tail survives
  instead of attenuating at the flux-weighted group σ_t. Neutral in
  the bisected-56 structure (sub-bin σ_t spread is already small);
  it matters for coarse or adaptively wide groups.

### Fixed — transport

- P1 `uncollided_split` dropped the beam's first-scatter anisotropy:
  the collided solve's first-collision source carried only
  `Σ_gp σ_s0(gp→g)·φ_unc(gp)` while `current[cell]` accumulated the
  collided iterate's moments alone — the anisotropic term
  `3·Ω·Σ_gp σ_s1(gp→g)·J_unc(gp)` never entered `p1_source`, so a
  forward-directed beam had its first collision isotropized under P1.
  The uncollided ray-trace now also deposits the directional moment
  `J_unc[cell][g][a] = Σ_d Ω_{d,a}·φ_d` (monodirectional ⇒ J = φ·Ω̂
  exactly, pinned by `uncollided_moments_track_beam_direction`), and
  the P1 source fold consumes the total current `J_collided + J_unc`.
  On the 2-cm μ̄ = 0.7 slab regression
  (`p1_uncollided_split_keeps_beam_anisotropy`) the deep-thermal P1/P0
  gain moves 1.02 → 1.82 with the term.
- `uncollided_beam_flux` attenuated to each transverse sample point at
  the cell's AXIAL centre only — depositing `e^{−σ·s_c}` instead of the
  cell mean along the in-cell ray segment. The centre value
  under-counts the mean by ~(σΔ)²/24 — 9% at σΔ = 1.5, 30% at 3, 64%
  at σ_t·Δ = 5.4 — under-sourcing first collisions in optically thick
  groups and under-reporting the uncollided field itself. The deposit
  is now the closed-form segment mean
  `e^{−σ·s_lo}(1−e^{−σ·span})/(σ·span)` over the face-clipped in-cell
  ray segment (per-node for `beam_sigma_nodes` kernels). Pinned by
  `uncollided_deposit_is_axial_cell_mean` (exact mean at σΔ = 3,
  ~30% distinct from the centre value); on the FiR-1 same-data
  comparison the near-face thermal SN/MC improved 0.65–0.73 →
  0.79–0.94 at z ≤ 3.
- Period-2 limit cycle in the outer iteration: the alternating
  group-sweep direction plus lagged periodic-wrap inflow make the
  composed operator a two-step map, and on near-conservative
  periodic problems it settles into a stable period-2 orbit — the
  field is already at its fixed point while the one-step residual
  sits pinned at the oscillation amplitude (~0.29 on the S4
  periodic water column, driven by a single deep high-energy cell).
  Convergence is now measured on the same-parity distance
  `|x_n − x_{n−2}|`; a settled cycle emits the parity midpoint.
  The column then converges at outer 30–32 (S4, 56 and 112 groups).
- `uncollided_beam_flux` deposited the full disk intensity `J/μ̄`
  into every cell whose CENTER back-rayed into the source disk —
  cells the beam footprint only partially covers received the whole
  beam (≈1/coverage overcount at the rim annulus; ~27% systematic
  overshoot when the disk is inscribed in a periodic cell). The
  deposit is now the transverse cell-average over an 8×8 point
  grid — partially covered cells receive the illuminated fraction.
  Verified by `uncollided_deposit_scales_by_transverse_coverage`
  (quadrant-coverage known answer) and the water-column experiment,
  where it removed the coverage component of the near-face
  overshoot by exactly the predicted factor.

### Added — interoperability

- `export meshtal`: emits an MCNP `meshtal`-layout file from a
  physical dose bundle in the OpenPINT convention — one mesh, tallies
  14/24/34/44 for B10/N14/hydrogen/photon — so OpenPINT's
  `sim_result_2_nifti.py` and `get_dose_components` ingest
  deterministic dose volumes unmodified. Rows carry cell-center
  (x,y,z) + `Result Rel Error` (sigma/|value| where the bundle carries
  uncertainties, else 0 — not an accuracy claim); axis-aligned grids
  with signed-permutation directions supported, oblique rejected.
  Verified against the actual OpenPINT `read_mcnp_mesh` parser: all
  four tallies parse and the values match the source bundle to the
  5-decimal print precision (≈1.4e-6).
- `openbnct_mcnp::pint_meshtal` / `meshtal_from_dose_bundle` with
  round-trip coverage through the existing parser.

### Added — transport

- Volumetric fixed sources: `SourceSpatialDistribution::UniformBox`
  declares an axis-aligned emission box in cm. Emission is isotropic
  (the model requires the full-sphere cone) and the field's
  `statistical_weight_per_site` is interpreted as the physical total
  emission rate in n/s — unit-weight normalization now applies only to
  the boundary-source geometries. The deterministic path deposits the
  source into the per-cell fixed source weighted by box/cell overlap;
  the OpenMC writer maps the shape to its native `box` spatial source.
  Plane-only paths (`source_coverage`, uncollided beam split) reject
  volume sources explicitly. Conformance coverage: an infinite-medium
  absorber returns the analytic `φ = S/σ_t` to 1e-6, global
  emission/absorption balance closes, and a source box mirrored about
  the slab midplane produces the exactly-reversed flux column
  (reflection equivariance to machine precision); a
  diamond-difference/periodic-seam parity artifact of ~1% can appear
  locally at a source discontinuity — discretization, not solver bias.

### Added — validation

- `validation/intercomparison-kobayashi-p1/` — first MC↔S_N
  cross-code harness case: Kobayashi P1-ii through OpenMC
  multi-group on the identical 2 cm cell lattice (20M histories),
  compared against both our S8 result and the published GMVP table
  with per-probe z-scores and cross-code deltas.
- `validation/canonical-kobayashi-p1/` — Kobayashi problem 1 (nested
  cubes with a void shell), both published cases. Case i (pure
  absorber) is scored against a cell-averaged analytic ray integral
  reproduced to <0.14% of COG's published table (LLNL-TR-648225);
  case ii (50% scattering) against the NEA report's GMVP MC values.
  Near-field probes pass at 5% in both cases (case ii: 0.4%, 3.3%);
  deep-field probes document the classic discrete-ordinates ray
  effect — confirmed by the S16 discriminator, where narrowing the
  lobes drives off-lobe probes to literal zero (the opposite of
  convergence — the benchmark's intended signature).
- `validation/canonical-azmy-problem/` — Azmy's (1988) weighted-DD
  quadrant problem, solved full-domain (mirror BCs realized by
  reflection): published quadrant means 1.676 / 4.159e-2 / 1.992e-3 vs
  S8 1.6789 / 4.131e-2 / 1.915e-3 — 0.17% / 0.67% / 3.9% — with
  machine-precision quadrant symmetry diagnostics.
- `validation/canonical-reed-problem/` — Reed's (1971) heterogeneous
  slab, scored against the Warsa (2002) eigenfunction reference table.
  The mirror-domain case (periodic transverse boundaries, vacuum at
  ±8 cm) runs as two `UniformBox` solves superposed by linearity;
  `compare.py` reports region-averaged scalar-flux errors against the
  reference's own trapezoid integration plus interior pointwise cells.
  At S8/1 mm the flat regions land within ~1% of the reference and the
  scattering-source peak within ~2% — a demonstrated angular-truncation
  signature (S4→S8 halves it), not spatial truncation (mesh refinement
  to 0.5 mm moves region means <0.3%) nor a normalization defect.
  Ships generator, run script, reference CSV provenance, and a graded
  `comparison.json`.

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
- `openbnct plan optimize --solver qp|lp` runs the certified conic
  solver (Clarabel interior point) instead of projected gradient
  descent. `qp` minimizes the identical bound-normalized quadratic
  penalty — the certified global optimum — and `lp` makes every
  objective bound a hard constraint under `λ·Σw`, so primal
  infeasibility is a definitive answer rather than a stall. Dose-at-
  volume bounds enforce the conservative CVaR tail-mean surrogate
  (recorded on the result), `f = 1` degenerates to exact voxelwise
  minimum rows, `min_eud` is admitted only at `eud_a = 1`, and the
  scenario path enforces per-objective worst-case rows. Results carry
  an optional `certificate` — solver status, primal/dual objective
  bound, and per-objective Lagrange bound multipliers (the marginal
  penalty cost of tightening each bound).
- `benchmarks/synthetic/scenario-robust-planning` is a known-answer
  fixture whose worst-case optimum is closed-form
  (`w_h* = 1.498875`, `w_b* = 0`); a committed conformance test asserts
  the optimizer reproduces it.
- `validation/fir1-k63-scenario-budget` encodes the published FiR 1 /
  BPA-F uncertainty budget (Savolainen 6% decomposition, blood boron
  ±20%, skin 1.5×blood; Kotiluoto 8% computational excluding boron) as a
  12-scenario set evaluated against the committed S_N transported
  field.
- `openbnct plan synthesize` — adjoint marginal-utility beam
  synthesis. One adjoint solve whose source is the signed composite
  `Σ_o sign·w_o·(d metric/d dose)·R_g` over every objective (coverage
  positive, sparing negative, dose-response weighted) ranks a full
  azimuth×elevation fan by marginal objective utility — beams that
  transit a sparing mask score against its negative source and are
  demoted, something fluence-only importance cannot express. With
  `--dose`/`--weights` the source is the true objective gradient at
  the current plan for iterate-and-resynthesize refinement; signed
  sources disable the θ-positivity repair automatically. Emits
  `openbnct.direction-candidates/0.1.0` scored
  `adjoint_marginal_utility`, content-binding the objective document.
- `openbnct plan iterate` — closed-loop adjoint inverse planning.
  Each round re-synthesizes the direction fan against the current
  plan's marginal-utility field, forward-solves the top `--add`
  candidates, and re-optimizes weights over the grown pool. Emits
  per-beam dose bundles, the final `openbnct.inverse-plan-result`,
  and an `openbnct.iteration-report/0.1.0` recording admissions,
  scores, and the penalty trajectory per round.
- `openbnct plan optimize --solver newton` — projected Gauss-Newton
  on the bound-normalized penalty. The dose map is exactly linear in
  beam weights, so the active-violation Hessian is the exact
  outer-product form `Σ 2·w·s²·a aᵀ`; each iteration Cholesky-solves
  `H·δ = −∇` with diagonal damping and a projected Armijo line.
  Converges in single-digit iterations where coordinate descent takes
  hundreds; bounds (`w ≥ 0`, `weight_bound`) are enforced by
  projection. Result records `method: "gauss_newton"`. Incompatible
  with `--scenario-set` (use `pgd`/`qp`/`lp` there).
- `openbnct plan synthesize --spectrum name=path … --radii r1,r2,…` —
  direction × spectrum × aperture-radius sweeps. Each `--spectrum` is
  an `EnergyDistribution` JSON scored through the shared adjoint
  solve (one uncollided ray-trace per combination), each `--radii`
  entry scores the same direction at a different aperture. Candidates
  carry `spectrum`/`aperture_radius_cm` and the emitted
  `openbnct.direction-candidates` document content-binds every
  spectrum file; off-face apertures skip with the positioning error.
  `plan iterate` accepts the same expansion — admitted beams carry
  the winning spectrum and radius into their forward solves, and the
  iteration report binds the spectrum inputs.
- `openbnct plan shape` — adjoint beamlet aperture shaping. The aimed
  disk for `--direction` is tiled `--beamlets`² deep into sub-disks,
  each scored against the objective composite adjoint via the
  uncollided ray-trace (no transport per beamlet); beamlets below
  `--keep-fraction` of the peak utility density close. Supports
  marginal mode (`--dose`/`--weights`) to shape against a current
  plan's residual. `--emit-fields DIR` forward-solves every kept
  beamlet into a `physical-dose-bundle` (256 cap), and running
  `plan optimize --dose DIR/*.json` then produces the beamlet
  intensity map — a delivered intensity-modulated field, not just a
  stencil. Emits `openbnct.aperture-shape/0.1.0`; beamlets
  under-resolving the voxel grid are recorded `resolved: false`.
- `openbnct.fraction-scales/0.1.0` + `plan optimize|select
  --fraction-scales`: multi-fraction planning. Each fraction's dose
  component scales (e.g. boron uptake decay across the washout
  window) expand the beam pool into per-(beam, fraction) delivery
  variables `beam@fraction`; because component scaling commutes
  through the linear isoeffective fold, the certified solvers — and
  beam selection, which then answers "which beams in which
  fractions" — apply unchanged. Requires isoeffective objectives and
  component-resolved bundles.
- `maximin` dose objective (`kind: maximin`): maximizes the minimum
  voxel dose over a mask — the tumor-floor formulation penalty
  composites cannot express. The conic solvers carry a scalar floor
  variable τ with `τ ≤ d_v` rows and cost `−weight·τ`; τ is shared
  across scenario field sets, so under `--scenario-set` the maximized
  floor is the *worst-case* floor. `target` is the aspiration bound
  reported for satisfaction/violation bookkeeping; `weight_bound`
  (delivery cap) is required to keep the floor finite. The PGD solver
  applies the equivalent linear floor pull with an argmin-voxel
  subgradient.
- `plan select` (`openbnct.beam-selection/0.1.0`): joint beam-subset
  × weight optimization over a candidate dose-field pool. Every
  subset of size ≤ `--beams` is re-solved by the certified inner
  solver (`exhaustive` — the global subset optimum, refused past
  50 k solves; `greedy` forward-stepwise for larger pools), ranked by
  certified objective with strict-mode infeasibility marked
  definitively, and the winner re-emitted as a full plan result via
  `--emit-plan`. This is the dosimetric counterpart of the geometric
  `plan directions` pre-filter, on real dose fields.

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

### Added — transport

- `SnOptions::inner_convergence` / `sn solve --inner-convergence`:
  the within-group sweep break tolerance now splits from the outer
  residual target (`None` preserves the historical shared value).
  The outer residual cannot descend far below the inner sweep's own
  break accuracy, so deep convergence targets may need a tighter
  inner tolerance.
- `pg response --aperture x,y,z --aperture-radius-mm r`: pinhole
  collimation for the prompt-gamma response — the adjoint detector
  source is restricted to quadrature ordinates inside the aperture's
  acceptance cone via per-ordinate source weights carried through
  `solve_sn_problem`/`solve_multigroup_adjoint`/`solve_photon_adjoint`.
  The response artifact declares the aperture via a `collimation`
  field (additive optional on `openbnct.pg-response/0.1.0`), and the
  command errors when no ordinate falls inside the cone. The
  collimated rerun of the BeNEdiCTE-style fixture
  (`validation/pg-benedicte-geometry/collimated/`) localizes emission
  to the imaged volume where the uncollimated chain could not.
- Photon anisotropy beyond P1: `PhotonMaterial` gains
  `scatter_legendre_moments_per_cm` (l = 2..=5, same invariant as the
  neutron tables) and the Klein–Nishina collapse now emits per-bin
  ⟨P_l⟩-weighted transfers, so `photon solve --p1 --anisotropy 2..5`
  exercises the shared eigenbasis kernel with real photon data.
  The collapse also corrects the P1 entries — each (g→g′) pair now
  carries *its own* KN mean cosine (E′(μ) ties each bin to a
  distinct angular slice) instead of one row-aggregate value.
- Pixellated detector imaging: `pg response --pixellated` solves each
  `--detector` voxel as its own pixel (one adjoint per voxel —
  superposition does not hold because the sweep's negative-flux
  fixup is nonlinear, so columns are independent solves by design)
  and emits a `openbnct.pg-response-array/0.1.0` bank with a
  sensitivity column per pixel. `pg counts --response` accepts the
  bank and emits `openbnct.pg-counts-array/0.1.0` with
  `per_pixel_tally`; `pg observe` folds it into detector entries
  bound to `{array.id}#pixel{i}` columns; and `pg reconstruct
  --response` accepts the bank for those bindings — the full
  imaging chain now runs on pixellated crystals. With `--aperture`,
  each pixel's acceptance cone axis runs that voxel→aperture.
- Patient-scale sweep memory: `sweep_group` no longer materializes the
  `[direction][cell]` angular field — directions sweep in
  thread-count-sized chunks and fold into per-cell moment
  accumulators (scalar, current, kernel) in fixed order, and periodic
  inflow reads wrap-plane snapshots (`wrap_prev`/`wrap_next`) instead
  of a full previous-iterate field. The dominant sweep storage drops
  from `2·n_dirs·n_cells` to `~n_threads·n_cells` — e.g. ~40 MB vs
  ~1.3 GB at 1M voxels, S8 — while remaining bit-identical for any
  thread count.
- Self-shielding regression coverage: `sn collapse --self-shielding`'s
  Bondarenko heterogeneous-dilution weighting now has a synthetic
  resonance test — a narrow 10⁴ b capture spike on B10 against a flat
  O16 diluent collapses ~50× lower shielded than unshielded, landing
  at the off-resonance value.
- Adjoint anisotropy: `solve_multigroup_adjoint` now transposes the
  P1 and l = 2..=5 moment matrices instead of dropping them, so
  adjoint importance solves carry the same scatter physics as the
  forward run (the antipode conjugation cancels inside the P_l
  kernel — the transpose alone is exact). `pg response --p1
  --anisotropy N` and `vr cadis --p1 --anisotropy N` expose it;
  anisotropic reciprocity ⟨q†,φ⟩ = ⟨q,φ†⟩ is regression-covered.
- Shielded/refined layered-head libraries: `sn collapse` now
  integrates each elastic transfer row on the kernel's own cusp
  abscissae (x = hi, x = lo/α, x = hi/α) — the bare tape-node
  trapezoid smoothed through the cusps and misintegrated the P0/P1
  rows by up to ~4% in resonance-structure groups, and a 56-group
  shielded collapse tripped the emitted-data validator until the fix
  (σ_t keeps the full collapsed elastic — below-floor downscatter is
  real removal — with residual row-sum excess absorbed into removal,
  so `row_sum ≤ σ_t` holds by construction). New benchmark artifacts:
  `multigroup-data-28g-v3-shielded.json` (Bondarenko dilution on the
  28g grid; dose shifts −1.6% median/±4% max vs v2) and
  `multigroup-data-56g-shielded.json` — which exposed a real
  condensation finding: the 28g bottom group smears the whole
  subthermal population into one effective absorber, so at 56g the
  marginally-absorbing sub-meV window resolves and the 1/v dose
  channels rise ~60–80× in the brain (hydrogen ~30%). Twenty-eight
  groups are not converged for BNCT capture dose; the 56g artifact is
  the benchmark's reference going forward.
- Workbench inverse planning: the Plan workspace's new Inverse
  planning section runs the certified conic solver in-process — drop
  or add dose bundles (one per beam), region masks, and an
  `inverse-plan-objective` document, pick QP-penalty or strict-LP
  mode, and the result's weights, per-objective outcomes and
  optimality certificate render in place, exportable as an
  `openbnct.inverse-plan-result`. Inputs enter by painted drop zone
  (web/native) or path add (native); the spec's SHA-256 binds the
  emitted result exactly as the CLI's does.
- First real MF33 covariance through the UQ chain: B10's MT107
  block from ENDF/B-VIII.1 collapsed onto the v3-shielded 28-group
  grid (`openmc cov-endf --parameter dose_response --component
  boron`) and propagated with `uq propagate` — the ¹⁰B(n,α)
  nuclear-data contribution to the folded boron dose is a 0.34%
  relative 1σ, computed analytically (zero perturbed solves).

### Added — imaging

- `dicom synth-pet`: writes a deterministic synthetic PET DICOM
  series (standard PET SOP class, modality PT) on any case's grid —
  layered SUV structure mirroring the phantom's region convention —
  so the `import-pet` → `register apply` → `boron apply` → `boron
  materialize` → tiered-assignment chain is exercisable end-to-end
  without patient data. Verified on the NF-BNCT-001 frame: SUV ratio
  4× core/background maps to 16.2 vs 3.2 µg/g with washout, and the
  materialized assignment feeds `sn solve` tiered materials.

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
