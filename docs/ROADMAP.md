# OpenBNCT Development Roadmap

*(Formerly OpenBNCT — internal crate names, the `openbnct` CLI/Python package,
and `openbnct.*` schema identifiers retain the original namespace.)*

**Adopted:** 2026-08-31

**Status:** R1 complete; R2 reference-statistics execution passed all
predeclared gates at 600M histories per seed; reference-output promotion
awaits the cross-code reproduction gate. R3 capability parity complete; R4
complete except real-engine acceptance gates.

**Style:** Evidence-gated, not feature-count or calendar driven

## Goal

Establish an open, transport-neutral BNCT research and independent-verification
platform that makes geometry, component dosimetry, biological interpretation,
uncertainty, and provenance comparable between codes and institutions.


## OpenPINT capability parity and demonstrated superiority

**Adopted:** 2026-09-12, at the project owner's direction.

OpenBNCT's completion target is a full BNCT research workbench that matches
every verified OpenPINT research capability and demonstrates advantages in
accuracy, usability, performance, interoperability, and reproducibility.
Independent verification is an integrated capability, not a reason to omit
research planning or analysis functions. A remaining OpenPINT advantage is a
tracked gap to close, not an accepted permanent product boundary.

This is a development objective, not a claim of present superiority.
OpenPINT's age or adoption does not establish its technical quality. Completion
requires evidence from implemented workflows and comparable benchmarks.

### Audited baseline and source evidence

The initial comparison is against OpenPINT commit
[`7d035fbfc1b764cab0b112e45acee39e6b909efb`](https://github.com/ipostuma/OpenPINT/tree/7d035fbfc1b764cab0b112e45acee39e6b909efb),
reviewed on 2026-09-12. Its README and relevant implementation surfaces were
inspected; OpenPINT was not executed during this roadmap review. Thus
"present" below means visible implementation, not independently validated
scientific correctness or usability.

Source paths at that revision:

- [README and workflow inventory](https://github.com/ipostuma/OpenPINT/blob/7d035fbfc1b764cab0b112e45acee39e6b909efb/README.md)
- [Treatment evaluation and combined treatments](https://github.com/ipostuma/OpenPINT/blob/7d035fbfc1b764cab0b112e45acee39e6b909efb/OpenPINT/treatment.py)
- [Biological model functions](https://github.com/ipostuma/OpenPINT/blob/7d035fbfc1b764cab0b112e45acee39e6b909efb/OpenPINT/dose/Model_db/models.py)
- [Component import, uncertainty, and fraction aggregation](https://github.com/ipostuma/OpenPINT/blob/7d035fbfc1b764cab0b112e45acee39e6b909efb/OpenPINT/dose/bnct.py)
- [Patient positioning](https://github.com/ipostuma/OpenPINT/blob/7d035fbfc1b764cab0b112e45acee39e6b909efb/OpenPINT/mcgenerator/patient_positioning.py)
- [MCNP deck generation](https://github.com/ipostuma/OpenPINT/blob/7d035fbfc1b764cab0b112e45acee39e6b909efb/OpenPINT/mcgenerator/mcnp.py)

The prior R3/R4 wording did not guarantee this breadth. In particular,
OpenBNCT's fixed-component biological weights do not establish parity with
photon-isoeffective models, fractionation, TCP/NTCP, or combined-treatment
analysis. An abstract backend interface does not establish working MCNP/PHITS
support. Each row below is required and remains open until its evidence passes.

### Required capability matrix

| ID / milestone | OpenPINT baseline | Required OpenBNCT deliverable | Acceptance evidence |
| --- | --- | --- | --- |
| OP-01 / R3 | NIfTI dose and mask workflows; CT-aligned mesh conversion | NIfTI import/export alongside DICOM; explicit LPS/RAS, affine, units, masks, interpolation and resampling semantics | Synthetic oblique, translated and anisotropic cases; landmark and volume checks; component round-trips; grid-resolution convergence with tolerances declared before execution |
| OP-02 / R3 | Synthetic patient masks, bounding boxes, material lattice generation | General heterogeneous synthetic anatomy and segmented-volume material/density mapping, including explicit tissue boron inputs | Multi-material head and cylindrical cases; analytic region volumes and densities; ambiguous mappings rejected; reproducible decks |
| OP-03 / R3 | Patient rotation, tumor-centroid and skin-entry positioning helpers | Interactive and scriptable research positioning, beam-entry geometry and source-to-anatomy transforms | Independent transform/landmark calculations, round-trips and out-of-volume rejection; matched GUI/CLI/Python outputs |
| OP-04 / R4 | MCNP and PHITS mesh readers; component uncertainty handling | Working MCNP and PHITS import adapters with explicit version/format support, component meaning, normalization and available statistical uncertainty | Independently created parser fixtures plus authorized real-engine examples; preservation of dose and uncertainty; no silent substitution of missing information |
| OP-05 / R4 | MCNP material/lattice and transform deck generation | MCNP input export for supported research geometry and sources, alongside native OpenMC preparation/execution | Generated decks executed by an authorized MCNP user; matched-case geometry and dose comparisons; transport programs remain external dependencies |
| OP-06 / R3 | Weighted irradiation-fraction aggregation | Multiple fields/exposures and fractions with explicit duration, source strength, boron assumptions and physical accumulation semantics | Hand-computable unequal-weight cases; dose-rate versus dose checks; alignment checks; covariance assumptions recorded; biological evaluation respects schedules rather than blindly summing weighted maps |
| OP-07 / R3 | Organ-limited irradiation time using physical, weighted and isoeffective endpoints | Research scenario evaluation under explicit maximum/mean organ limits, reporting the limiting structure/voxel and assumptions | Analytic scaling cases, multiple competing limits, zero-rate and infeasible cases, and independent checks for nonlinear biological endpoints |
| OP-08 / R3 | Weighted dose, photon-isoeffective tumor/healthy-tissue models and fractionation | Source-attributed model registry extending fixed weights to these model families, with explicit parameters, units, validity domains and fraction schedules | Independent equation-based fixtures and published reference cases for each model; parameter sensitivity; physical and biological results kept distinct |
| OP-09 / R3 | TCP, NTCP and UTCP functions; treatment metrics and DVHs | Tumor-control and normal-tissue-complication research estimates, combined endpoint where justified, standard dose-volume metrics and overlays | Probability bounds and limiting cases, analytic DVHs, explicit tissue/model provenance; distinguish low-level function availability from a working end-to-end workflow |
| OP-10 / R4 | Hadron-dose resampling, BED conversion and BNCT/hadron combination | Imported external photon/hadron dose, fraction-aware BED analysis and scientifically specified combined-treatment research evaluation | Analytic BED fixtures; co-registration checks; explicit compatible endpoint/model assumptions; incompatible biological quantities cannot be silently added |
| OP-11 / R3 | Structured plan configuration, Excel input and CLI/Python workflows | Validated research plan schema, CSV/XLSX interchange and complete GUI/CLI/Python paths for the covered tasks | Clean-install end-to-end examples; identical numerical results across surfaces; useful diagnostics for malformed plans; persistence and rerun of saved scenarios |
| OP-12 / R3 | Mask subtraction and limiting-organ construction | Region subtraction, exclusion masks and configurable CT-threshold region construction | Exact synthetic mask tests, frame checks, empty/overlapping region handling and recorded threshold choices; full segmentation remains a separate feature |

### Delivery sequence and completion gates

1. Finish the R2 physical benchmark and restore passing CI. Retain the
   predeclared scientific acceptance gates.
2. Complete OP-01/02/06 first so heterogeneous cases, interchange and exposure
   semantics are sound. Build OP-03/07/08/09/11/12 into the R3 research alpha.
   Simple fixed weights and DVH plumbing alone do not close these tasks.
3. Complete OP-04/05/10 in R4 and publish a reproducible cross-code case suite.
   One external importer alone no longer satisfies the full R4 target.
4. Carry all parity evidence into R5 external validation. An earlier alpha may
   ship with explicit gaps, but the full research platform is not complete
   while a verified baseline capability remains uncovered.
5. Refresh the pinned OpenPINT capability inventory at each release candidate.
   New implemented capabilities or measured advantages become named roadmap
   tasks. Track upstream plans separately from demonstrated functions.

For each OP task, maintain an evidence record containing implementation status,
OpenBNCT and comparator revisions, test inputs, expected outputs or independent
reference, acceptance tolerances, actual results, limitations and artifact
locations. Record unsupported and untested workflows as open.

### Demonstrating that the complete workbench is better

Freeze a comparison protocol before collecting results. Include homogeneous
and heterogeneous phantoms, oblique geometry, boundary-sensitive DVHs, multiple
exposures, biological model evaluation, and imported MCNP/PHITS outputs.
Use independently generated synthetic data and published methods.

- **Accuracy:** compare against analytic, independent numerical and available
  measured references. Agreement with OpenPINT alone is not truth. Report
  component/region error, statistical uncertainty, grid convergence and model
  assumptions; any material disadvantage gets a corrective roadmap task.
- **Performance:** compare identical analysis workloads on declared hardware;
  report wall time and peak memory. Separate OpenBNCT processing overhead from
  transport-engine time. Compare transport at equivalent precision, not merely
  equal particle histories. Rust alone is not evidence of greater speed.
- **Usability:** measure clean installation, time to first result, completion
  time, manual steps and errors for the same research tasks with external users.
  A GUI screenshot does not establish better usability.
- **Interoperability:** demonstrate actual DICOM and NIfTI round-trips, MCNP and
  PHITS imports, OpenMC execution and MCNP deck export on frozen cases.
- **Reproducibility:** recreate results from saved plans/evidence on a clean
  environment, retain model/data versions, and reject altered or incompatible
  artifacts.

The release comparison must show coverage of every OP row and report each
dimension honestly. The objective is parity or better across these dimensions
and measurable advantages beyond parity. Every measured regression remains a
tracked improvement task; no aggregate score may hide a material deficit.

Implement independently from scientific publications and public format
specifications with attribution. Do not copy OpenPINT implementation code.
The existing research-use and Avify Dose IP boundaries continue to apply.
Conventional scenario evaluation and the named parity tasks do not authorize
patent-sensitive optimization or clinical deployment.

## R0 — Architecture and risk register

Exit evidence:

- backend-neutral case and physical-dose contracts;
- OpenMC isolated behind a transport interface;
- MIT license and contribution policy;
- research and Avify Dose IP boundaries;
- synthetic-data-only repository policy;
- documented feasibility risks and acceptance gates.

## R1 — Geometry truth case

Deliver one synthetic DICOM CT and RTSTRUCT case with linked egui views.

Exit evidence:

- DICOM frame of reference and voxel affine preserved;
- axial, sagittal, and coronal orientation independently checked;
- structures round-trip within predeclared geometric tolerances;
- patient identifiers absent by construction;
- malformed or ambiguous geometry is rejected.

The frozen starting case is
[`NF-BNCT-001`](benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md). R1 implements
its synthetic CT, RTSTRUCT, expected masks, and backend-neutral case manifest;
it does not wait for a transport engine.

Implementation status:

- complete: deterministic CT/RTSTRUCT generation;
- complete: patient-space CT ordering and affine validation;
- complete: exact frozen ROI masks, volumes, and LPS centroids;
- complete: malformed frame, plane, spacing, and orientation rejection tests;
- complete: one-command CLI generation and independent verification;
- complete: backend-neutral `case.json` with traversal-safe SHA-256 artifact
  verification;
- complete: UI-independent axial, coronal, and sagittal mappings with linked
  crosshair and independent edge-orientation tests;
- complete: integrity-gated egui viewer with linked axial, sagittal, and coronal
  views, LPS cursor, window/level controls, and RT structure overlays;
- complete: warning-free external CT/RT Structure Set IOD validation with
  `dciodvfy`, plus cross-instance consistency validation with `dcentvfy`, in CI.

## R2 — Physical component truth case

Calculate the four physical BNCT dose components for a simple analytic phantom.

Exit evidence:

- OpenMC version, commit, nuclear data, settings, seed, and inputs recorded;
- B-10, N-14, hydrogen/recoil, and photon definitions cited and tested;
- estimator limitations documented;
- analytic or independently calculated reference tolerances passed;
- statistical uncertainty retained at voxel level.

R2 also requires the response-generation and classification ledger specified by
`NF-BNCT-001`, plus an independent estimator comparison. OpenMC output alone is
not promoted to a reference result.

Implementation status:

- complete: OpenMC 0.16.0 estimator boundary and reaction-filter limitation
  recorded in ADR 0005;
- complete: canonical four-component physical-dose bundle with content-hashed
  profile identity, absolute voxel uncertainty, and independently derived
  physical-total uncertainty;
- complete: versioned explicit-nuclide material and unit-weight fixed-source
  contracts, including frozen machine inputs for `NF-BNCT-001`;
- complete: machine-validated contributor ledger and NJOY partial-KERMA
  generation method, explicitly held at `method_frozen_tables_pending`;
- complete: fail-closed response-set envelope and physical-dose-bundle binding,
  including pointwise neutron-KERMA closure and review-state enforcement;
- complete: case-scoped OpenMC nuclear-data manifest, HDF5 capability
  inspection, artifact verification, and cross-sections mapping preflight;
- complete: acquire the official 9.66 GB OpenMC ENDF/B-VIII.1 distribution,
  freeze its case-scoped acquisition receipt and 16 selected artifacts, and
  pass the material-specific transport capability preflight;
- complete: resumable, no-overwrite nuclear-data acquisition with frozen
  publisher profiles, redirect confinement, size and publisher-digest checks,
  and receipts bound into manifest schema `0.3.0`;
- complete: acquire the current publisher-matched NNDC neutron archive and
  freeze the ten `NF-BNCT-001` evaluations by path, size, and SHA-256 as an
  unqualified candidate, preserving the different OpenMC-recipe archive digest
  and unresolved equivalence state;
- complete: validate every selected ENDF material identity and generate the ten
  byte-stable NJOY2016.78 production/diagnostic decks with a content-bound,
  no-overwrite `input_preparation_only` manifest;
- complete: byte-stable OpenMC 0.16 input generation with complete scoring and
  audit tally ledgers;
- complete: controlled, no-overwrite NJOY2016.78 execution with exact
  input/output file sets, processor/runtime hashes, structured kinematic
  diagnostics, preserved rejected receipts, and independent artifact
  verification;
- complete: record the first ten-nuclide execution as rejected evidence after
  72 MT 301 violations across N-15, O-16, O-17, and O-18, without clipping or
  silently dropping an isotope;
- complete: derive and independently regenerate a transported-photon KERMA
  suitability report that rejects the same four nuclides on explicit NJOY
  missing/incomplete photon-data findings;
- complete: compare all ten production MT 301 tables pointwise against the
  official processed OpenMC tables with corresponding grids and no
  interpolation; the maximum relative difference is `4.892060e-7`;
- complete: establish that the official processed tables retain effective
  local-photon fallback for O-17 and O-18, so official-library acquisition does
  not by itself resolve the transported-photon KERMA blocker;
- complete: expose the intended workflow in an evidence-aware egui shell with
  overview, geometry, transport, four-component dose, and evidence workspaces;
  unfinished physics and backend actions remain explicitly disabled;
- complete: add official Avila Labs branding, contextual offline help, and
  guided dim-and-spotlight tours over live workflow controls;
- complete: add a separately versioned alternate-library selection and
  deterministic comparison contract, then run the first complete candidate
  gate with publisher-matched JEFF-4.0; it is rejected with six affected
  nuclides, 120 kinematic violations, no baseline rejection resolved, and two
  new rejections;
- complete: inventory exact MF=6/12/13/14/15 source records for both
  ten-nuclide selections and add source-aware suitability schema `0.2.0`; this
  corrects NJOY's File 12 message to informational when File 13 is valid,
  clearing JEFF N-15 while leaving five full-evaluation candidate rejections;
- complete: independently integrate all eight supported N-15 File 13/File 15
  continuum photon-energy moments for both selections and reproduce 58 shared-
  node NJOY diagnostics within their five-digit print precision;
- complete: independently reconstruct JEFF N-15 MF=6/MT=102 photon first and
  second moments, photon-momentum recoil, and Q-value balance; 33 of 37 source
  nodes fail a conservative 1% screen even though all spectra normalize and
  NJOY's self-bounded kinematic check reports zero violations;
- complete: derive and content-bind the common OpenMC neutron transport domain
  from the exact processed-data manifest and material, then apply it
  symmetrically through domain-aware suitability schema `0.3.0`; all 72
  baseline violations remain in domain, while six of 120 JEFF findings are
  above 20 MeV and O-16 alone clears with zero in-domain violations;
- complete: add evidence-aware suitability schema `0.4.0`, bind the exact H-2
  source and processor-attribution reports to the immutable domain assessment,
  clear all 15 attributed H-2 findings without deleting them, and integrate
  N-15's independent capture-balance rejection;
- complete: expose that evidence-aware gate through Avila Core's hash-bound
  external-checker protocol, preserving the categorical qualification and
  exact remaining-finding count as separate claims while OpenBNCT retains all
  domain logic;
- complete: partition the 102 remaining in-domain JEFF-4.0 findings into 59
  source-data-blocked C-13/O-18 findings and a 43-finding O-17 reaction queue;
- complete: reproduce all 43 O-17 MT 301 excesses from NJOY's printed
  per-reaction File 6 energy-balance remainders, while retaining all 43 for
  independent physical validation and waiving none;
- complete: add mixed-source selection schema `0.3.0`, NJOY input-manifest
  `0.2.0`, and repeatable profile/receipt CLI binding, then execute the
  ENDF/B-VIII.1 + TENDL-2025 candidate under the frozen method; the six shared
  nuclides reproduce the baseline exactly while all four TENDL substitutions
  remain rejected with 77 kinematic violations (70 in domain), no baseline
  rejection resolved, and none introduced;
- complete: attribute all 43 in-domain TENDL O-17 findings to NJOY's printed
  energy-balance remainders and expose the verified candidate comparison to
  Avila Core through `check-candidate-comparison`; TENDL's duplicated File 3
  grid points make the deeper independent gates uncomputable, which is
  preserved as a source-format finding;
- complete: compute independent File 3/File 6 reaction-level energy-balance
  remainders for JEFF-4.0 O-17 without NJOY; all 43 in-domain samples compute,
  and the independent remainder sum reproduces NJOY's printed `ebal` sums and
  the MT=301 excess within 1.7% at every finding energy, with the MT=91
  processor-internal convention and residual per-product `ebar` differences
  preserved as unreviewed evidence (ADR 0030);
- resolved: the O-17 queue is dispositioned as explained — the non-conservation
  is a documented property of the evaluated File 6 accounting, independently
  reproduced; the baseline photon-coverage findings are data-coverage findings
  (ADR 0031); all findings remain carried in provenance, none waived;
- complete: generate the first response tables under the frozen method —
  `generate-response-tables` folds receipt-bound production HEATR PENDF
  sections (MT=301 total, MT=443 kinematic, MT=407 B-10, MT=403 N-14 partial
  KERMA) onto a 7,526-knot union grid with exact B+N+H closure at every knot,
  and `verify-response-tables` reproduces both artifacts byte-exactly and
  seals the set as independently reviewed under the in-house deterministic
  verification path (ADR 0031); all 72 in-domain kinematic findings are
  carried into the generation report, none waived;
- complete: smoke execution under OpenMC 0.16.0 at the frozen commit —
  `openmc generate` materializes the deterministic deck bound to the reviewed
  response set, the run produces a five-batch statepoint honoring all twelve
  tally contracts, and `compare-openmc-smoke-estimators.py` freezes the ADR
  0007 correlated-diagnostic evidence: executed energy-function tables bitwise
  identical to the sealed set, coupled-heating closure within combined
  uncertainty, component response sum versus the dedicated neutron-heating
  estimator at the 1e-9 relative level, and B-10/N-14 reaction-rate audits
  reproducing the evaluated mean deposited energies within 2e-4;
- complete: statepoint import into the platform result model — `openmc
  collect` reads the newest statepoint through a pure-Rust HDF5 path, binds
  the run header, recorded OpenMC version, and tally contracts to the input
  manifest, normalizes the component tallies into gray per source neutron
  with per-voxel 1-sigma uncertainties, takes the coupled-heating tally as
  the dedicated physical total, and emits a validated
  `openbnct.physical-dose-bundle/0.2.0` whose provenance id binds the
  input-manifest and statepoint SHA-256 digests. The smoke bundle is
  execution evidence only; reference results still require a
  reference-statistics execution under the predeclared acceptance gates.

## R3 — End-to-end research alpha

Run a synthetic head case from DICOM through physical dose, a separately
versioned biological model, visualization, DVH, and evidence export.

Exit evidence:

- one-command reproducible run;
- GUI and CLI consume identical engine results;
- an installable Python alpha wraps the same Rust validation and model contracts
  without duplicating scientific logic;
- physical and biological layers can be inspected separately;
- deterministic manifest binds inputs and outputs;
- independent verifier rejects modified artifacts;
- all R3 tasks in the OpenPINT capability matrix pass their acceptance evidence.

Implementation status:

- complete: biological interpretation layer — `openbnct-bio` applies a
  separately versioned `openbnct.biological-model/0.2.0` artifact to a
  physical dose bundle, producing `openbnct.biological-dose-bundle/0.2.0`
  whose `weighted_gray*`/`weighted_eqd2` units and `synthetic_research_only`
  qualification can never alias physical dose;
- complete (OP-08, first slice): biological model families — models now
  declare `fixed_per_component` or `photon_isoeffective` weight semantics
  (the latter requiring every photon weight to be exactly 1.0), an optional
  free-text `validity_domain`, and an optional linear-quadratic
  `fractionation` block (fraction count, source particles per fraction,
  default + per-region α/β). A fractionated model transforms the weighted
  per-particle total to `weighted_eqd2` — `n·d·(1+d/(α/β))/(1+2/(α/β))` —
  with first-order sigma propagation, while components stay linear
  weighted values; both schema bumps are 0.2.0 because
  `deny_unknown_fields` makes the new fields forward-incompatible.
  Published reference cases now live in `conformance/bio/0.2.0/` — one
  byte-fixed golden bundle per model family plus stable rejection tokens
  (`cargo test -p openbnct-bio --test bio_conformance`). Parameter
  sensitivity is covered by `openbnct bio sweep` / Python
  `sweep_biological_model` — a `openbnct.bio-sensitivity-sweep/0.1.0`
  record sweeping component weights, region overrides, or fractionation
  parameters with per-point re-validation and region-masked
  min/mean/max;
- complete: deterministic `openbnct.dose-volume-histogram/0.1.0` over named
  voxel masks for any bundle component or total;
- complete: evidence-bundle export — `evidence export` collects inputs,
  deck files, logs, statepoints, and dose bundles into a
  `openbnct.evidence-bundle-manifest/0.1.0` directory; `evidence verify`
  re-checks every artifact hash and rejects tampering;
- complete: one-command `openmc run` chaining prepare, execute, and
  collect, with optional `--evidence-root` export;
- complete: GUI parity — the dose workspace loads validated physical and
  biological bundles (component statistics, totals, region-mask DVH plot)
  and the evidence workspace verifies bundle manifests in place; no
  placeholder values and no GUI-side scientific logic;
- complete: Python parity — the bounded API now exposes dose-bundle
  loading, biological-model application, and DVH computation over the same
  Rust contracts, with an extended wheel-level parity suite;
- complete (OP-02, first slice): DICOM-derived material assignment —
  `benchmark derive-materials` emits a transport-neutral
  `openbnct.material-assignment/0.2.0` from verified ROI masks. Box-exact
  masks become CSG cell regions; arbitrary masks become `voxel_set` regions
  realized as a rectilinear material lattice (one universe per distinct
  material, one element per voxel). Region densities may differ from the
  base — collection normalizes heating by per-voxel mass and rescales
  folded components by the atom-density ratio (density × mass fraction),
  residual folds by the density ratio alone. Rotated grids, overlaps,
  out-of-grid indices, and duplicated voxels are all rejected;
- complete (negative result): the first candidate-reference evaluation at
  300M histories missed only the photon voxel-precision gates — report
  committed at `transport/openmc-acceptance-report-300M.json` (sha256
  e68ba744…a820), estimator comparisons (291) and chi-square (378) all
  passed, photon median RSE ≈ 3.72% vs the 3.0% gate;
- complete (passed at 600M): the specification's stated protocol —
  "particle count is increased until the precision gates are met" — was
  executed via OpenMC restart: `*-b100` profiles declared 100 batches /
  600M histories per frozen seed upfront and each run continued from its
  50-batch statepoint (identical RNG stream and tally accumulators). All
  gates passed on all three seeds: photon median RSE ≈ 2.67% vs 3.0%,
  p95 ≈ 3.5% vs 5.0%, every estimator comparison (291) and chi-square
  seed-consistency check (378) green — report committed at
  `transport/openmc-acceptance-report-600M.json` (sha256 5f57a0fc…b220).
  Per-seed immutable bundles are retained under
  `.openbnct-data/openbnct/nf-bnct-001-reference-600M/`;
- pending: reference promotion — the specification's cross-code
  reproduction gate still requires a separately implemented transport path
  (Geant4, or licensed MCNP/PHITS produced by a licensed user) reproducing
  the frozen case before the candidate becomes a reference output;
- complete (OP-02, continued): `benchmark derive-materials --mask NAME=path`
  binds map keys to external `RegionMask` JSONs (e.g. `nifti to-mask`
  output) instead of RT Structure Set ROIs, with name/voxel-count checks —
  segmented NIfTI volumes now drive material assignment end-to-end;
- complete (OP-12, first slice): mask operations — `openbnct mask`
  subtract/union/intersect `RegionMask` volumes with shared-frame and
  non-empty-result checks, and `mask threshold` builds regions from a DICOM
  CT modality (HU) window for limiting-organ construction;
- complete (OP-07, first slice): organ-limited irradiation time —
  `openbnct irradiation-time` evaluates per-source-particle endpoints
  (physical or biological weighted) under per-region max/mean limits,
  emitting `openbnct.irradiation-time-report/0.1.0` with the limiting
  structure, per-region time and particle budgets, and declared
  assumptions; zero-statistic regions are unbounded and absolute-unit
  endpoints are rejected;
- complete (OP-03, first slice): positioning helpers — `openbnct position
  aim` derives a `UniformAxisPlane` source whose beam axis passes through a
  mask centroid (axis approaches and oblique directions), reporting the
  entry face/point and source-to-centroid distance; `position rotate`
  applies quarter-turns about a patient axis and rejects
  non-axis-aligned results. Python parity (`aim_source`,
  `rotate_source`, `PositionReport`) and a GUI positioning panel in the
  transport workspace (ROI or mask-file target, approach axis, quarter-turn
  rotate, report/source export) now match the CLI — all three surfaces run
  the same `openbnct_transport` functions;
- complete (OP-01): NIfTI imaging I/O — `openbnct-nifti` reads and writes
  NIfTI-1 `.nii`/`.nii.gz` 3-D scalar volumes (`u8`–`f64`), prefers sform
  over qform, converts RAS+ to patient LPS with transform provenance,
  accepts explicit-mm or unspecified units, and rejects unsupported
  dimensions/datatypes/transforms/non-mm units. `openbnct nifti` provides
  `info`, `to-mask`, `export-dose`, and `resample` (nearest/trilinear);
  affine handling is regression-tested against independent `nibabel`
  output including an oblique sform. The GUI dose workspace surfaces the
  same four operations — volume inspection with transform provenance,
  mask writing, dose export, and nearest/trilinear resampling onto a
  bundle grid. Grid-resolution convergence evidence is pinned by an
  analytic test: a smooth field sampled at 2.0/1.0/0.5 mm and resampled
  onto a fixed incommensurate target must show interior L∞ error ratios
  below the predeclared 0.35 second-order bound per halving. CT-aligned
  mesh conversion is covered: `nifti resample --target case.json` (and
  the GUI panel) map external volumes onto the transport case's
  CT-aligned `GridGeometry` through the shared `read_target_geometry`
  path, which also accepts dose bundles and refuses unknown schemas;
- complete (OP-06): weighted exposure/fraction aggregation — the
  `openbnct.exposure-plan/0.1.0` contract binds each exposure's dose bundle
  by SHA-256 with an explicit weight, weight basis, duration, and boron
  assumption; `openbnct accumulate` sums weighted doses with quadrature
  1-sigma propagation under declared inter-exposure independence, emitting
  an ordinary physical dose bundle that flows through DVH, biological
  models, and the GUI;
- complete (OP-09, first slice): dose-volume metrics and endpoint models —
  `openbnct metrics` computes exact `D_x`/`V_x`/min/mean/max/EUD readings
  over a region mask (`openbnct.dose-metrics/0.1.0`), and `openbnct
  endpoint` scores `openbnct.endpoint-model/0.1.0` artifacts —
  `voxel_poisson_tcp` (voxel-level LQ Poisson TCP over per-particle dose),
  `logistic`, and `probit` (Lyman) functions over mean/min/max/EUD
  statistics — emitting `openbnct.endpoint-evaluation/0.1.0`, with `endpoint
  utcp` combining TCP+NTCP under `p_plus`/`difference` after matching
  case/region/quantity/source checks. Python exposes the same functions,
  and the GUI dose workspace overlays region metrics through the shared
  `RegionDoseMetrics` path with metrics-artifact export. Public
  reference fixtures live in `conformance/endpoints/0.1.0/` covering all
  three function families and both UTCP combinations;
- complete (OP-11, first slice): plan tables and diagnostics — the
  `openbnct-plan` crate round-trips `openbnct.exposure-plan/0.1.0` through
  CSV (`# key:` metadata) and XLSX (`plan`+`exposures` sheets), reporting
  every malformed row with its row number and hashing bundles on import
  when `sha256` cells are blank; `openbnct plan import|export|validate`,
  `ExposurePlan::validate_diagnostics`, and
  `openbnct_plan::accumulate_plan_file` serve CLI, Python
  (`load_exposure_plan`, `exposure_plan_diagnostics`,
  `accumulate_exposures`, `plan_table_read`, `plan_table_write`), and the
  GUI Plan workspace identically;
- complete: optional GUI slice-overlay of loaded dose on the patient grid —
  the geometry workspace loads a physical or biological dose bundle gated on
  matching `case_id` and equivalent grid geometry, then blends a hot-ramp
  dose wash (component or total, opacity and %of-max threshold controls)
  into the linked axial/sagittal/coronal renders, with the dose value
  surfaced at the linked voxel;

## R4 — Transport-neutral reference platform

Add generic component-dose import and cross-code comparison cases.

Exit evidence:

- published interchange schema;
- at least one result produced outside OpenMC imported without loss of meaning;
- OpenMC and one independent transport path compared on frozen cases;
- public conformance suite and versioned reference outputs;
- Python API supports external biological-model experiments without duplicating
  production evaluation logic;
- all R4 tasks in the OpenPINT capability matrix pass, including both MCNP and
  PHITS import, MCNP deck export, and external-dose research analysis.

Implementation status:

- complete (first slice): published interchange schema + generic importer —
  `openbnct.component-dose-interchange/0.1.0` records the producing system,
  version, and normalization, the transport-neutral grid, and the four
  physical component volumes with optional per-voxel sigmas. `openbnct
  import interchange` and Python `import_component_dose` share one Rust
  path validating it into `openbnct.physical-dose-bundle/0.2.0`.
  `dedicated` totals keep their estimator sigmas; `component_sum` totals
  record `unavailable` uncertainty rather than claiming a dedicated
  estimator. Bundle provenance binds the document SHA-256
  (`interchange:<system>:sha256:<hash>`) and threads through
  `dvh`/`metrics`/`bio apply` — verified end-to-end on a synthetic
  PHITS-labeled fixture in `examples/interchange/` (analytic stand-in
  values, not PHITS output);
- complete (first slice): public conformance suite —
  `conformance/interchange/0.1.0/` ships a manifest-driven fixture set (4
  valid documents with byte-fixed golden bundles, 15 rejection cases with
  stable error tokens) exercised by
  `cargo test -p openbnct-core --test interchange_conformance`;
  `conformance/adapters/0.1.0/` extends coverage end-to-end — documented
  MCNP meshtal and PHITS `xyz`-mesh inputs replay through each adapter into
  byte-fixed interchange documents and bundles
  (`cargo test -p openbnct-mcnp|openbnct-phits --test adapter_conformance`,
  same `OPENBNCT_UPDATE_CONFORMANCE` regeneration convention);
- complete (documented-format slice): MCNP/PHITS import adapters — the
  `openbnct-mcnp` crate parses ASCII `meshtal` files (tally + optional
  energy-bin selection, `Rel Error` → absolute sigmas) and the
  `openbnct-phits` crate parses `xyz`-mesh `.out` files (echo-parsed grid,
  `#newpage` axis pages, inline `r.err` preferred over `_err` siblings,
  incomplete error coverage rejected). Both emit the interchange document
  and validate through the shared importer; `openbnct import mcnp|phits`
  and Python `import_mcnp_meshtal`/`import_phits` are parity surfaces with
  document-hash provenance. Parser fixtures are authored to documented
  formats — real-engine acceptance remains the open gate;
- complete (documented-format slice): MCNP deck export — `openbnct export
  mcnp` and Python `export_mcnp_deck` emit a deterministic deck from a
  transport case: grid `RPP` box, `voxel_box` regions as carved CSG cells,
  `voxel_set` regions via a `LAT=1` lattice `FILL` array (one universe per
  distinct material), `M` cards from declared mass fractions (operator's
  `--xs-suffix` or xsdir defaults — never an invented library), `SDEF`
  plane source, `NPS`, and `FMESH` flux tallies on the case mesh. The deck
  deliberately carries no component folding — that stays the external
  pipeline's declared step before `import mcnp` re-ingests the meshtal.
  Source planes outside the grid are rejected. Execution against real MCNP
  remains the open acceptance gate;
- complete (first slice): OP-10 external-dose/BED combined analysis —
  `openbnct.external-dose/0.1.0` imports one absolute-dose course with
  declared fractionation (uniform or explicit, hash-bound provenance);
  `openbnct bio bed` converts it to a BED/EQD2 field under declared
  (optionally per-region) α/β with first-order sigma propagation; `openbnct
  bio combine` adds an external `eqd2` field to a photon-isoeffective
  `weighted_eqd2` bundle — the only admitted combination — under a required
  operator-stated additivity assumption, with strict trilinear
  co-registration (`--resample trilinear`) when grids differ and rejection
  of every incompatible-quantity or uncovered-target path. The
  `openbnct.combined-dose/0.1.0` record binds both input content hashes,
  both provenance chains, and the declared assumption. CLI/Python parity
  (`import_external_dose`, `bed_from_external_dose`,
  `combine_biological_doses`) with analytic BED fixtures in unit and parity
  tests; `examples/interchange/photon-course-60gy.json` ships a synthetic
  60 Gy/30-fx demonstration course;
- pending: committed real-engine parser fixtures per producing system for
  OP-04;
- complete (infrastructure slice): cross-code frozen-case comparison —
  `openbnct compare` and Python `compare_dose_bundles` measure voxelwise
  agreement between two physical dose bundles on the same case (equal
  `case_id`, equivalent grid, same components/unit — all enforced) and emit
  `openbnct.dose-comparison/0.1.0`: per-component and total max/mean/RMS
  absolute difference, normalized difference anchored to the reference
  maximum, and a combined-sigma coverage fraction when both sides state
  uncertainties, with both content hashes and provenance chains bound into
  the record;
- pending: a real OpenMC-vs-independent-code comparison on a frozen case —
  the comparison record exists; it still needs an actual MCNP/PHITS-produced
  candidate bundle (the real-engine acceptance gate).

## Cross-cutting distribution

The accepted distribution boundary is recorded in [ADR
0015](docs/adr/0015-python-and-native-distribution.md): PyPI is the planned
primary scientific-user entry point, Cargo remains the native developer path,
and desktop builds ship as native release artifacts. All three surfaces call the
same Rust contracts.

Implementation status:

- complete: choose the PyO3/maturin mixed-package architecture and prohibit a
  parallel Python dose, geometry, evidence, or qualification engine;
- complete: implement the first bounded Python API over the selected,
  versioned Rust contracts — case generation, verification, and gated loading;
  geometry and ROI inspection; schema-validated manifest and transport-contract
  readers with canonical `to_json` serialization; the honest response-set
  folding gate and backend capability flags — with a 39-test cross-language
  parity suite run against a wheel installed into a clean environment in CI;
- complete: build and smoke-test the supported wheel matrix — the abi3
  (`py310`) wheel set builds for linux-x86_64/aarch64, macos-x86_64/arm64
  and windows-x86_64 plus sdist via `.github/workflows/publish-pypi.yml`;
  TestPyPI verified by clean-venv install, then PyPI `openbnct` 0.1.0
  published under the `v0.1.0` tag through OIDC trusted publishing;
- complete: crates.io publication review and execution — `publish = true`
  lifted, all 16 workspace crates (including the `openbnct` facade) published at v0.1.0 in dependency order
  (`openbnct-gui` vendors its frozen benchmark assets to package
  standalone); `publish-crates.yml` wires tag-driven releases once
  per-crate trusted publishers exist;
- complete: native desktop artifacts — `release-desktop.yml` builds
  `openbnct` + `openbnct-gui` release binaries for five platforms and
  attaches them to GitHub releases with Sigstore build-provenance
  attestation; v0.1.0 carries all five archives (built at 912224c,
  repacked for naming — attested subjects are the run's artifacts;
  future `v*` tags attest release assets directly).

## R5 — External validation and adoption

Exit evidence:

- independent reproduction by a researcher outside Avila Labs;
- review by a BNCT physicist;
- comparison with measured phantom or commissioned beam data under a written
  collaboration agreement;
- at least two institutions execute the conformance suite;
- methods manuscript and archival software/data release.

## R6 — Domain completeness

Candidate milestones for the "go-to BNCT workbench" scope, adopted at the
project owner's direction. Each remains evidence-gated like the earlier
milestones; R6 capabilities strengthen what R5 validates but are not
prerequisites for starting external review.

- **R6-01 — facility beam descriptions.** A versioned beam document:
  source term (energy spectrum, angular and spatial distributions),
  aperture/collimator geometry, normalization basis, and provenance
  (published reference or measured characterization). Acceptance: schema,
  a published-beam registry encoding at least one literature epithermal
  beam with citation, and a transport case binding the beam that executes.
  **Status: implemented.** `openbnct.beam-description/0.1.0` is live
  (`beam.rs`); the transport model gained `uniform_disk` space,
  `isotropic_cone` angle, and `tabulated_histogram` energy variants; the
  OpenMC emitter maps them to `cylindrical`/`mu-phi`/`tabular`
  distributions and the MCNP deck emitter to `POS/AXS/RAD`, `DIR` cosine
  histograms, and `ERG` histograms; `openbnct beam info|list|bind` is on
  the CLI; `beams/fir1-k63.json` encodes the FiR 1 K63 beam from
  Seppälä (2002, HU-P-D103) group-integrated fluences and is labeled a
  literature reconstruction. A bound FiR 1 case generated a deck and
  executed a 5-batch OpenMC smoke run end-to-end. Also fixed a latent
  emit bug: `reference_uvw` was written as an XML attribute that OpenMC
  ignores (silent +z default — coincidentally correct for the benchmark's
  beam); it is now emitted as the schema's child element.
- **R6-02 — beam quality characterization.** *(implemented)* IAEA
  TECDOC-1223-style metrics via `openbnct.beam-quality/0.1.0` reports
  (`beam_quality.rs`) and `openbnct beam qa`: in-air thermal/epithermal/
  fast/total fluence rates, spectral fractions, mean energy, and
  current-to-fluence ratio are exact properties of the declared source
  distribution (histogram bins are split by partial overlap at group
  boundaries); optional `--dose` adds in-phantom advantage depth,
  advantage ratio, and peak therapeutic ratio from a physical dose bundle
  under declared compound effectiveness weights; `--reference` compares
  each metric against published values inside per-metric relative
  tolerances. `beams/references/fir1-k63.json` carries the Seppälä (2002)
  measured values; `beams/qa/fir1-k63-phantom.json` is a generated report
  from a 10M-history FiR 1 run through the NF-BNCT-001 phantom (AD 7.75
  cm, AR 1.99, PTR 2.59). Group fluences reproduce within tolerance;
  `current_to_fluence_ratio` honestly *fails* (modeled 0.994 vs measured
  0.77) — the fixture's collimator-bound cone cannot represent measured
  penumbra divergence, and the report records that fidelity gap rather
  than hiding it. Gamma contamination remains an unmodeled channel listed
  explicitly in the report.
- **R6-03 — measurement import.** *(implemented)* `openbnct.measurement-
  record/0.1.0` covers activation foil, ion chamber, TLD, TEPC, and
  fission-chamber measurements with explicit units, optional one-sigma
  uncertainty, position provenance, and scalar or histogram (lineal-
  energy spectrum) values; `openbnct.measurement-comparison/0.1.0` binds
  a record to a computed artifact (currently a beam-quality report) by
  content hash and reports per-point relative difference,
  sigma-normalized difference, and chi-square over σ-bearing points.
  `openbnct measurement info|compare` drives both. The committed fixture
  `measurements/fir1-k63-free-beam.json` digitizes Seppälä (2002) Table 4
  free-beam values; its uncertainties were not transcribed, so points
  compare by relative difference only — the schema reports
  `without_uncertainty` rather than inventing sigmas. Comparing a
  measurement record against raw tally outputs (not just beam-quality
  reports) is the remaining extension.
- **R6-04 — RT Dose export.** *(implemented)* `openbnct dicom
  export-rtdose` writes any dose-bundle volume (physical total or named
  component) as a multi-frame RT Dose object: 32-bit pixels with
  DoseGridScaling, full IPP/IOP/GFOV grid addressing, deterministic
  2.25.* UIDs derived from bundle provenance, optional CT
  ReferencedSOPSequence, and research-only marking. DoseUnits is honest:
  GY for absolute dose, RELATIVE for per-source-particle bundles.
  Verified by independent-toolkit (pydicom) round-trip reproducing a 40³
  grid within 8e-7 relative — the 32-bit quantization floor. RTSTRUCT
  contour export and RTPLAN linkage remain future scope; the export is
  not a commissioned planning artifact.
- **R6-05 — microdosimetric model family.** *(implemented)*
  `openbnct.microdosimetric-model/0.1.0` artifacts carry per-component
  linearized-MKM parameters (α₀, β, lineal-energy source), the spherical
  domain geometry, and a mandatory free-text validity domain; each
  component's dose-mean lineal energy is either a published constant or
  resolved at apply time from a `openbnct.lineal-spectrum/0.1.0` document
  (TEPC-derived or published; event-frequency or dose-weighted bins with
  exact piecewise-constant moments). Applying converts each component to
  photon-equivalent dose through `α* = α₀ + β·z̄₁D`,
  `z̄₁D = ȳ_D/(ρ·π·r_d²)`, LQ-inverted against the cell photon response;
  the emitted bundle marks `microdosimetric_kinetic` semantics, an
  `mkm_weighted_*` unit, and an `mkm_research_only_not_clinical`
  qualification, and records the resolved ȳ_D plus content-bound spectra
  in an `microdosimetry` provenance block. `openbnct bio apply` routes on
  the model schema and takes repeatable `--spectrum`; `bio spectrum`
  extracts a TEPC histogram from a measurement record; `bio lineal-mean`
  reports ȳ_F/ȳ_D. Conformance fixtures live in
  `conformance/bio/mkm-0.1.0/` (HSG-like LQ parameters traced to Kase
  2006/2008; component lineal energies are representative stand-ins) with
  derivation hashes verified by the suite. The family is kept distinct
  from photon-isoeffective weighting: a weight-model declaring
  `microdosimetric_kinetic` semantics is rejected, and MKM outputs assert
  no clinical RBE/CBE/Gy-Eq claim. Cross-model comparison landed with
  `openbnct bio compare`: two biological bundles over the same physical
  dose are checked for shared provenance and geometry, then summarized
  per region with a maximum voxelwise ratio above a significance floor —
  the emitted `openbnct.bio-model-comparison/0.1.0` record keeps the
  research-only qualification explicit. On the FiR 1 cylindrical-phantom
  dose the TECDOC-convention CBE weights and the literature-constant MKM
  differ by a uniform ≈2.5× — a quantified cross-model spread, not a
  clinical statement.
- **R6-06 — PET-derived boron.** *(implemented)*
  `openbnct.boron-uptake-model/0.1.0` maps a co-registered SUV volume to a
  per-voxel B-10 concentration field: `suv_ratio` (measured blood-pool
  reference scaling — the tumor:blood-ratio method, with a B-10
  isotopic-fraction parameter for total-boron assays), `linear_suv`
  (calibrated regression), and `uniform` (assumed-uptake baseline). An
  optional uniform exponential `time_correction` applies
  `2^(−Δt/T½)` washout between imaging and irradiation with half-life
  uncertainty. Model parameters carry 1σ uncertainties propagated to
  per-voxel field σ; the mandatory `validity_domain` states the protocol
  assumptions, and negative mapped values clamp to zero with the count
  recorded. Output is `openbnct.boron-field/0.1.0`, content-binding the
  model, SUV image, and registration. `openbnct boron info|apply|
  materialize` covers inspection, field generation (optionally chaining
  `--registration` to co-register the PET volume first), and realization
  into a tiered `MaterialAssignment` for `--assignment` deck generation.
  Per-voxel SUV noise is an explicit optional term. The field is a
  research estimate (`pet_derived_boron_research_only_not_clinical`), not
  an assayed patient measurement.
- **R6-07 — rigid registration.** *(implemented)*
  `openbnct.registration/0.1.0` records a moving→fixed rigid transform
  (LPS mm, row-major 3×3 rotation + translation) with optional
  content-bound moving/fixed image references. Two construction paths:
  `landmark_least_squares` (Horn's closed-form quaternion fit over ≥3
  non-degenerate landmark pairs, storing the pairs and the RMS residual
  as evidence) and `declared` (operator-transcribed transform requiring
  an explicit provenance note — no landmark evidence is fabricated).
  `openbnct register landmarks|declare|info|apply` covers creation,
  inspection, and NIfTI resampling onto a transport-case or dose-bundle
  grid (trilinear or nearest). Synthetic landmark cases recover exact
  transforms to ~1e-15 rad / machine-precision residuals; validation
  rejects non-orthonormal rotations, reflections, degenerate point sets,
  and declared transforms without provenance.
- **R6-08 — systematic uncertainty propagation.** *(implemented)*
  `openbnct.systematic-uncertainty/0.1.0` reports propagate declared
  systematic sources over a physical dose bundle: `boron_concentration`
  (fractional per-voxel σ of a content-bound `boron-field` scaling the
  boron dose component), `positioning` (`|∇D|·σ_mm` first-order
  displacement shift, optionally bound to a registration whose RMS
  residual supplies σ), and `relative_component` (declared relative σ on
  a named component). The correlation model is explicit: per voxel,
  sources combine in quadrature; for region means, Monte Carlo σ is
  voxelwise-independent (`sqrt(Σσ²)/N`) while each systematic source
  contributes its mean per-voxel σ fully correlated across voxels —
  the structure that makes boron loading the dominant BNCT uncertainty.
  `openbnct uq apply|info` emits and inspects reports; the dose bundle's
  own σ stays pure Monte Carlo — the systematic layer is additive, never
  relabeled.
- **R6-09 — variance reduction.** *(implemented; benchmark evidence
  in progress)* Weight-window variance reduction for the OpenMC path. A
  `openbnct.variance-reduction/0.1.0` spec declares per-particle regular
  meshes with uniform, explicit, or forward-flux-derived bounds;
  `vr resolve` turns it into a `openbnct.weight-windows/0.1.0` artifact
  (forward-flux resolution implements OpenMC's MAGIC-equivalent
  volume-normalized, group-maximum-normalized derivation from a
  completed analog statepoint, disabling cells above a relative-error
  threshold). `openmc generate --vr` emits the mesh and
  `<weight_windows>` blocks into `settings.xml`; `vr validate`
  re-runs the ordinary acceptance evaluation on the reduced-history
  result and compares shared region/tally means against a reference
  acceptance report under combined uncertainty, recording the achieved
  history-reduction factor in a `openbnct.vr-validation/0.1.0` report.

  Evidence so far: a 140M-history run under region-targeted weight
  windows (`variance-reduction/nf-bnct-001-ww-v3.json`) validated
  **unbiased** against the 600M analog reference — all 1134 shared
  region/tally comparisons within combined uncertainty, max z = 2.97 —
  at a 4.3× history reduction
  (`transport/openmc-vr-validation-140M.json`). It narrowly missed two
  photon-precision gates (`central_axis_2cm` photon heating 1.041% vs
  the 1.0% limit; per-voxel photon median 3.24% vs 3.0%): the deep
  photon heating tally is correlation-limited — weight windows give
  ~10× per-history variance reduction on deep neutron fluence but only
  ~1.3× on the gated photon tally.

  A 196M-history run (140 batches x 1.4M particles, seed 271828182)
  cleared both photon-precision gates: `central_axis_2cm` photon
  heating relative standard uncertainty 0.853% (limit 1.0%) and
  per-voxel photon median 2.74% with p95 3.42% (limits 3.0%/5.0%),
  remaining unbiased against the 600M analog reference (all 1134
  comparisons within combined uncertainty, max z = 2.80) at a 3.06x
  history reduction (`transport/openmc-vr-validation-196M.json`,
  `transport/openmc-acceptance-report-196M-vr.json`; 97,688 s wall
  clock). One contract gate remains open: the acceptance contract
  requires runs at at least three registered seeds for the chi-square
  replication-consistency test, and the campaign so far is single-seed.
  Two further runs at seeds 20260831 and 314159265 (each needing
  ~196M histories to pass their own per-run precision gates) are
  required for full contract acceptance.

Out of scope for R6 remains the deferred list below — in particular plan
optimization stays behind the IP boundary and nothing in R6 is a clinical
claim.

## R7 — Distribution and external validation (draft)

Draft — not yet scheduled. Candidate items for the phase after R6, pending
review:

- **R7-01 — public distribution.** Publish the `openbnct-*` crates to
  crates.io in dependency order (`openbnct-core` first, `openbnct-cli` last),
  publish the `openbnct` wheel to TestPyPI then PyPI, and add a tagged release
  workflow. *(COMPLETE — all 16 workspace crates (including the `openbnct` facade) on crates.io v0.1.0
  including `openbnct-gui`; `openbnct` wheel on PyPI via OIDC trusted
  publishing (`.github/workflows/publish-pypi.yml`, five platform wheels
  + sdist verified by release-set check and smoke test); `v0.1.0` tag
  and GitHub release created. crates.io tag workflow
  `publish-crates.yml` is armed for future releases — per-crate
  trusted-publisher entries configured for all 16 crates.)*
- **R7-02 — measured-data validation.** Import published BNCT beam
  measurements through `openbnct.measurement-record` /
  `openbnct.measurement-comparison`, extend the candidate-data comparison
  machinery to facility measurements with explicit provenance, and record
  agreement results as evidence bundles — distinct from the synthetic
  self-consistency benchmark, which remains the correctness gate.
  *(COMPLETE — free-beam FiR 1 K63 record + comparison committed under
  `measurements/`; in-phantom water-phantom chain under
  `validation/fir1-k63-water-phantom/`: NJOY response-set provenance chain
  (fresh 2016.78 execution receipt, three-level suitability reports,
  independently reviewed water response set), 20M-history OpenMC phantom
  run, and the first in-phantom measured-data comparison — computed
  advantage depth 9.75 cm vs published 8.1 cm, thermal-fluence maximum at
  2.75 cm vs published ~2.0–2.5 cm, with the advantage-ratio convention
  gap honestly recorded. Depth profiles landed:
  `measurements/fir1-k63-cylindrical-phantom-depth.json` digitizes the
  Aschan et al. (TECDOC-1223) TL-detector series in water, PMMA, and
  Liquid B cylindrical phantoms with stated 13% 1σ and a documented
  interpolation convention; the peak-normalized comparison
  (`results/measurement-comparison-depth.json`) agrees through the
  buildup region and records the expected tail divergence of the Ø20 cm
  cylinder vs the cubical reference phantom. The like-for-like check
  then landed under `validation/fir1-k63-cylindrical-phantom/`: a
  deterministic S₈ three-group solve on a voxel-set Ø20 × 24 cm water
  cylinder with declared ENDF/B-VIII.1-collapsed data reproduces the
  digitized measured profile within ~1.3σ at all 12 bins (χ² = 6.7),
  confirming the earlier divergence was phantom geometry; the PMMA
  phantom series additionally reproduces under
  `validation/fir1-k63-pmma-phantom/` (χ² = 6.0, all 15 bins within
  ~1.15σ) — two measured compositions on the same beam.)*
- **R7-03 — cross-code comparison depth.** Extend the MCNP meshtal and PHITS
  xyz-mesh import adapters into documented cross-code comparison workflows
  against the OpenMC path on the synthetic benchmark, and evaluate import
  adapters for other BNCT research codes (e.g., OpenPINT outputs) where their
  formats are documented. *(technical scope COMPLETE — both adapters
  verified import→compare at benchmark scale on a real 64k-voxel run;
  recipe in `docs/research/CROSS_CODE_REPRODUCTION.md`. OpenPINT output
  interop evaluated and landed: its pipeline writes per-component NIfTI
  volumes, now importable via `openbnct import nifti` with
  caller-declared producer provenance and optional paired sigma
  volumes. Licensed-engine execution is an external gate outside the
  technical scope.)*
- **R7-04 — workbench usability.** Bring the egui desktop shell to a
  documented, reproducible workflow (case load → run → dose overlay →
  evidence inspection) with packaged artifacts once distribution exists.
  *(COMPLETE — end-to-end research workflow documented in
  `docs/USAGE.md` (case → response data → transport → characterize →
  verify → inspect); packaged five-platform binaries ship with v0.1.0
  via `release-desktop.yml`; crates.io/PyPI install paths live.)*
- **R7-05 — transport throughput.** Profile the OpenMC run path (the
  196M-history R6-09 run is the current baseline: 97,688 s wall clock,
  ~2,006 histories/s active on six i3-N305 threads) and evaluate
  parallel-seed orchestration across machines without changing the contract
  surface. *(orchestration slice demonstrated — deterministic per-seed decks
  verified byte-identical by regeneration, a ~130 MB self-contained worker
  bundle (OpenMC binary + bundled glibc + case-scoped cross sections + deck)
  executed under WSL2 on a second machine, and ping-based completion
  notification across a NAT boundary; runbook in
  `docs/research/PARALLEL_SEED_ORCHESTRATION.md`. The deterministic S_N run
  path is now characterized — 5×3 cells×order matrix, 91k cell-direction
  sweeps/s serial at the 64k-cell end, direction-count-linear scaling —
  `docs/research/SN_SOLVER_THROUGHPUT.md`. Remaining: actual OpenMC
  run-path profiling once the workstation has free CPU. **Parallel sweep
  landed:** the S_N ordinate sweep and per-cell moment reduction are
  rayon-parallel — angular flux stored `[ordinate][cell]` so each
  direction owns an exclusive row under `par_iter_mut`, per-cell
  reductions computed in parallel then applied serially to preserve the
  deterministic convergence order. Bit-for-bit solver semantics; thread
  count bounded by `RAYON_NUM_THREADS`.)*

Nothing in R7 changes the deferred list below or adds any clinical claim.

## R8 — Frontier transport and uncertainty (draft)

Draft scope, accepted for scheduling. Items are ordered by dependency;
R8-01 gates R8-02 and enables parts of R8-03 and R9-02. No item in R8/R9
requires licensed software, external approval, or any clinical claim.

- **R8-01 — in-house deterministic transport path.** *Landed.* A
  multigroup discrete-ordinates (S_N) solver over the regular scoring
  mesh inside `openbnct-transport` (`multigroup.rs`): 3-D Cartesian
  diamond difference, level-symmetric quadrature (exact S2–S8 tables,
  equal-moment extension to S16), vacuum/incident-flux/periodic faces,
  Jacobi source iteration with an outer re-sweep for upscatter, and a
  monodirectional-beam uncollided-flux split (analytic ray-trace for the
  uncollided component — the standard fix for ordinate-obliquity bias).
  It consumes the versioned `openbnct.multigroup-data/0.1.0` contract —
  declared group structure plus per-material totals, scatter matrices,
  and optional dose-response vectors — and emits content-bound
  `openbnct.multigroup-flux/0.1.0` plus, with `--dose`, a folded
  `PhysicalDoseBundle` the analytic/gamma/metamorphic evaluators consume
  directly. Verification: unit tests pin the DD sweep to the Padé
  per-cell ratio at 1e-9, the uncollided split to the exact exponential,
  and heterogeneous inserts to per-region slopes; on NF-BNCT-003 the
  solver's folded boron dose reproduces the analytic oracle's declared
  0.2308 cm⁻¹ slope at 0.000% deviation — an independent transport
  implementation matching a closed-form ground truth. Scope stays
  honest: isotropic scattering, no fission, declared multigroup data —
  a verification solver, not a production engine.
  
  The data pipeline landed with the FiR 1 28-group work:
  `openbnct sn collapse` produces `multigroup-data` from pointwise
  evaluations — processed OpenMC-HDF5 nuclides or NJOY-broadened PENDF
  tapes through a built-in ENDF-6 MF3 reader — with analytic P0
  isotropic-in-CM elastic transfer, mass-kerma dose responses
  (¹⁰B(n,α), ¹⁴N(n,p), recoil, capture-γ local kerma), and the
  scatter-weighted `transport_mu_bar`. Beam handling grew the cone
  uncollided split (the analytic ray-trace now integrates narrow
  isotropic cones over an equal-area direction grid) and the consistent
  extended transport correction (σ_t,tr = σ_t − μ̄·Σ_s with the same
  forward fraction removed from the in-group diagonal — required for
  contractive iteration in near-conservative media). Opt-out flags
  (`--no-uncollided-split`, `--no-transport-correction`) keep the raw
  path available for comparison.

  P1 in-group anisotropy landed as a third scattering treatment:
  the collapse emits exact lab-cosine transfer moments
  (`scatter_p1_matrix_per_cm`, validated against the 2/(3A) mean-cosine
  identity), and `sn solve --p1` iterates angular currents alongside
  the scalar flux and adds `3·Ω·Σ_s1·J` to the directional source —
  superseding the extended transport correction (same diffusion limit,
  exact kinematics rather than the row-mean approximation). Flux
  artifacts record `scattering_order`; the adjoint path stays honest
  P0. An MF7/MT4 S(α,β) tape reader (`endf_mf7`) is landed and
  validated on the real ENDF/B-VIII.1 H(H₂O) and H(Lucite) evaluations
  (29,798 / 7,752 sections) — the collapse-side bound-atom transfer
  integration is also landed: `sn collapse --tsl` applies the ENDF-102
  incoherent-inelastic kernel below the tape's E_max (thermal
  upscatter, σ_b/natom·((A_r+1)/A_r)² bound-atom normalization,
  free-gas residual beyond the β domain), and its kernel-integrated
  σ_s(E) reproduces NJOY 2016.79 THERMR's MF3/MT222 bound-atom σ_s
  within ~10% across 0.025–5 eV.

  The physics layer grew six capabilities on top of that base.
  **Coupled photon transport**: `sn photon-collapse` builds
  `openbnct.multigroup-photon-data/0.1.0` — photoelectric/incoherent/
  coherent/pair-production atomic tables with a Klein–Nishina transfer
  kernel and per-subshell photoelectric response — and
  `sn photon-solve` runs a two-pass n→γ solve (ENDF MF12/13/14/15
  production collapse plus the declared 0.478 MeV ¹⁰B(n,α₁γ) line,
  photon S_N sweep, photon-kerma dose fold). **P2–P5 anisotropy**:
  `scatter_legendre_moments_per_cm` carries l = 2..5 transfer moments
  through an exact discrete addition-theorem kernel — Jacobi
  eigendecomposition of the P_l direction kernel yields the discrete
  moment basis the quadrature resolves — behind `--anisotropy`.
  **Bondarenko self-shielding**: `--self-shield` weights each nuclide's
  collapse by 1/(σ_t + σ₀) with σ₀ the heterogeneous dilution of the
  material's other nuclides. **Sub-voxel volume fractions**:
  `voxel_fractions` material regions declare per-cell material
  fractions (sum ≤ 1, no overlap with full regions); the shared
  `cell_compositions`/`material_composition_map` path blends them into
  synthesized transport rows every solver, fold, and screening pass
  consumes. **Boron microdistribution**: a declared compartment-fraction
  model (nucleus/cytoplasm/membrane) applies a first-order chord
  compound factor to the boron dose component, with
  `MicrodistributionUptakeScale` carrying the compartment fractions
  through Morris/Sobol screening. **Anderson acceleration**: a
  mixing-history accelerator on the symmetric down+up cycle's outer
  fixed point cuts the upscatter-coupled iteration count — the
  TSL solves' dominant cost.

  Honest validation status (FiR 1 cylindrical phantom, 28-group
  ENDF/B-VIII.1 data): the measured TECDOC-1223 water thermal-fluence
  depth profile is **bracketed**, not reproduced — but the
  best-physics solve now sits essentially on it. Under the
  collapse-consistent source weighting (`*-1e` artifacts) the raw P0
  solve underpredicts the deep tail ~23–50×, the consistent transport
  correction overpredicts it ~1.7–4.8×, and the **TSL+P1 solve**
  (`multigroup-flux-28g-tsl-p1-1e` — ENDF/B-VIII.1 `lwtr` bound-atom
  kernel on H1 below 10 eV + P1 anisotropy) lands within ~±20% of
  measured through 9 cm (normalized χ² = 18.8 vs 31 P1-only / 128 raw
  / 261 corrected). Residual disagreement: a ~1.6–2.3× uniform
  absolute-scale offset plus the >9 cm tail (0.46/0.26 at 11.5/14.5
  cm) — genuine model residual (incident rate verified to 1%), driven
  by the declared three-bin source histogram (the measured FiR1
  adjusted spectrum is not publicly tabulated) and the coarse
  very-low-energy transfer detail. See the validation README for the
  full accounting.

- **R8-02 — adjoint-driven variance reduction.** *Landed.* An open
  CADIS/FW-CADIS implementation: `adjoint` bounds in a
  `openbnct.variance-reduction` spec are resolved by
  `openbnct vr cadis` — the R8-01 solver runs the transposed problem
  (reflection-symmetric quadrature makes the adjoint solve a forward
  sweep of transposed data), converts the importance field into
  source-normalized window targets `w₀ = w_ref/φ†`, and emits a
  content-bound `openbnct.weight-windows` artifact consumable by the
  existing `openmc generate --vr` path. FW-CADIS takes a forward
  `multigroup-flux` artifact and builds `q† = response/φ_fwd`.
  Verified by discrete reciprocity (⟨q†,φ⟩ = ⟨q,φ†⟩ to truncation
  error), monotone-importance, and bound-shape invariants. A
  deterministic-space biased-source artifact remains a follow-on; the
  unit-weight normalization keeps the current unbiased-source decks
  consistent.

- **R8-03 — nuclear-data uncertainty propagation.** Complete:
  `openbnct.multigroup-covariance/0.1.0` declares per-material relative
  standard deviations over `sigma_total`, per-transfer `scatter`
  entries, and `dose_response` vectors — independent diagonal entries
  plus dense group-correlation blocks. `openbnct uq propagate` folds a
  component response through the R8-01 deterministic solve and emits
  `openbnct.dose-uncertainty-budget/0.1.0`: σ²(R) = Sᵀ·C·S decomposed
  by source (nuclear data / response data / declared statistical),
  with the covariance, data, case, and nominal flux content-bound.
  Sensitivities are central finite differences of the shipped discrete
  operator — the positivity-clamped sweep is nonlinear, so continuous-
  adjoint GPT inner products measured ~25–40% pointwise off and were
  rejected for committed magnitudes (they remain correct for CADIS
  importance ratios). Verified by step-stability checks, exact linear
  response terms, correlated-vs-diagonal quadrature, and solver
  conservation invariants. ENDF-covariance *processing* (NJOY → the
  declared format) remains open follow-on scope; no BNCT tool ships
  any of this today.

- **R8-04 — declared-input sensitivity screening.** Complete:
  `openbnct.sensitivity-spec/0.1.0` declares parameter ranges over six
  target kinds (material σ_t / scatter / response scales, beam disk
  center/radius, geometry origin shifts) and `openbnct uq screen`
  evaluates each design point as a full deterministic solve folded to
  the component response. `morris` gives elementary-effects μ/μ*/σ in
  r·(k+1) solves; `sobol` gives Jansen ST and centered Saltelli-2010
  S1 indices in N·(2k+2) solves (output mean-centered to kill the m²
  cancellation pathology). Reports are seed-reproducible
  `openbnct.sensitivity-screening/0.1.0` artifacts with content-bound
  spec/case/data; verified by exact linear-parameter recovery,
  dead-parameter detection, seed determinism, and a committed
  NF-BNCT-003 spec.

- **R8-05 — transport-derived lineal-energy spectra.** Complete:
  `openbnct.lineal-tally-spec/0.1.0` declares a spherical site diameter
  plus a per-component charged-secondary table (emission energy,
  effective range, collision share); `openbnct bio lineal-tally`
  evaluates it over a computed `multigroup-flux` artifact —
  `ε(l) = E·min(1, l/R)` over the sphere's isotropic chord distribution,
  rate-weighted and domain-integrated — and emits the
  `openbnct.lineal-spectrum/0.1.0` the MKM `computed_spectrum` source
  already consumes, closing the transport→biology loop on declared
  data. Verified against the analytic chord statistics (long-range
  ramp, short-range saturation at E/l̄, rate weighting, scale
  invariance) and a committed NF-BNCT-003 spec+flux pair producing
  ȳ_D ≈ 198 keV/µm for a 1 µm site — inside the published boron-capture
  TEPC range. In-deck MC lineal tallies remain open follow-on scope;
  the deterministic path supplies the spectral shape today.

## R9 — Breadth and credibility (draft)

Draft scope, accepted for scheduling. Items are independent of each
other and of R8 ordering unless noted.

- **R9-01 — gamma-index and BNCT QA metrics.** A γ(dose-difference,
  distance-to-agreement) evaluator with configurable criteria and pass
  rates, plus the field's standard comparison quantities, emitted as an
  `openbnct.gamma-evaluation` record bound into the comparison chain.
  Complete: `openbnct gamma`, Rust `evaluate_gamma`, and Python
  `evaluate_gamma` implement Low et al. (1998) gamma over the frozen-case
  guards — configurable dose-difference percent, distance-to-agreement,
  global/local normalization, and a low-dose threshold cutoff; per
  component plus physical total they report evaluated/excluded counts,
  pass rate, and mean/p95/max γ, with an optional embedded per-voxel γ
  field. Pass/fail is exact (the search neighborhood is the dta-radius
  ball); γ values above 1.0 are ball-restricted upper bounds, stated
  honestly in the record. Emitted as `openbnct.gamma-evaluation/0.1.0`
  with both input content hashes and provenance chains bound in.

- **R9-02 — accelerator-source and beam-shaping layer.** Parametric
  ⁷Li(p,n)/⁹Be(p,n) thick-target source terms and moderator/filter
  assembly sweeps feeding the existing TECDOC-1223 beam-quality figures
  of merit — lets users model and compare candidate beam designs, the
  field's modern direction. Uses the R8-01 fast solver for sweeps where
  available. In progress, ⁷Li(p,n) path landed: `openbnct accelerator
  source` integrates the Liskien–Paulsen recommended 0° differential
  cross sections over Bethe proton slowing in lithium with exact
  nonrelativistic two-body kinematics (29.7 keV threshold floor, 0.79
  MeV at 2.5 MeV), emits `openbnct.accelerator-source/0.1.0` with a
  normalized histogram spectrum, and can write a ready-to-bind
  `BeamDescription` whose new `computed_model` provenance variant
  hash-binds the generator artifact. `openbnct bsa` adds
  `openbnct.beam-shaping-assembly/0.1.0` — ordered moderator/filter/
  reflector/collimator/aperture layer stacks with full/disk/annulus
  footprints — plus `rasterize` onto a case grid as a
  `MaterialAssignment` and `sweep` enumerating thickness combinations
  into a content-bound `openbnct.bsa-sweep/0.1.0` record. Remaining:
  ⁹Be(p,n) data path, sweep results wired through beam-quality
  evaluation, and the R8-01 solver hookup when it lands.

- **R9-03 — boron microdistribution artifacts.** Subcellular
  localization fractions and intercellular heterogeneity variance as a
  versioned research artifact feeding photon-isoeffective evaluation;
  published work shows intercellular ¹⁰B heterogeneity materially
  changes IsoE dose. Complete: `openbnct.boron-microdistribution/0.1.0`
  declares compartment fractions (nucleus/cytoplasm/membrane/
  extracellular, with 1σ), concentric-sphere cell geometry, adopted
  α/⁷Li energies and CSDA ranges, and intercellular uptake CV;
  `openbnct boron microdistribution` evaluates per-compartment
  energy-deposition fractions to the nucleus by deterministic
  quadrature (straight-line constant-LET tracks, exact ray–sphere
  chords, no RNG) and emits `openbnct.microdistribution-correction/
  0.1.0` — the nucleus-dose factor vs uniform concentration with
  propagated 1σ, the uniform reference, and the cell-to-cell dose CV,
  with the heterogeneity nonlinearity explicitly deferred to downstream
  survival evaluation.

- **R9-04 — metamorphic transport oracles.** Property-based invariants
  for Monte Carlo decks — rotation invariance of isotropic problems,
  source/detector reciprocity, energy-conservation closure — as an
  additional automated correctness layer beyond fixed-answer
  conformance. Complete: `openbnct metamorphic` evaluates four oracles
  over dose bundles — `reflection` (a bundle vs its own grid-axis
  mirror under an operator-declared symmetry premise), `rotation`
  (reference vs a source-rotated run, the candidate volume permuted
  back by the `rotate_quarter`-consistent index map), `superposition`
  (combined-source run vs the sum of two component runs), and
  `reciprocity` (source↔detector voxel-pair interchange) — emitting
  `openbnct.metamorphic-evaluation/0.1.0` with per-quantity z-score
  statistics under combined MC σ, all inputs content-bound, and no
  equivalence claim. Energy conservation is deliberately absent: BNCT
  capture reactions are exoenergetic, so a naive budget is not a valid
  invariant.

- **R9-05 — benchmark library expansion.** Complete (specification +
  contract layer): `NF-BNCT-002` adds the deeper-penetration
  heterogeneous case — a 30 cm phantom with a skull-equivalent slab on
  the incident face and a high-boron tumor insert on axis under a
  declared 1/E epithermal disk source, with frozen case/materials/
  assignment/contract committed. `NF-BNCT-003` adds the analytically
  solvable case — a 0.0253 eV monodirectional beam into a near-pure ¹⁰B
  slab where the boron component follows `exp(−Σ_t·z)` — backed by the
  new transport-neutral `openbnct.analytic-oracle/0.1.0` declaration and
  `openbnct analytic` evaluator (weighted log-slope regression over the
  declared fit window, `openbnct.analytic-oracle-evaluation/0.1.0`
  output). The published OpenPINT "analytic" reference was investigated
  and declined: its reference is itself a 1 mm-mesh MCNP CSG run, not a
  closed-form solution, so NF-BNCT-003 supplies the genuinely analytic
  oracle instead; a cross-code OpenPINT-format fixture remains under
  R9-06's export path. Execution of both new cases is pending their
  material-bound response sets, same phased pattern as NF-BNCT-001.
  Additionally `benchmarks/synthetic/layered-head-phantom` adds a
  concentric-layer head surrogate (skin/skull/brain over a declared
  void, ICRU-44-style isotopic compositions with trace ¹⁰B as a
  dose-activation convention) collapsed to 28 groups through the
  R8-01 ENDF/PENDF pipeline — explicitly not an anatomical or clinical
  model.

- **R9-06 — bidirectional interop.** Complete: `openbnct nifti
  export-components` writes all four components as float64 NIfTI volumes
  with sigma companions — the per-component convention `import nifti`
  consumes — plus a content-hashed
  `openbnct.component-nifti-manifest/0.1.0` (export→import round-trip is
  value-, sigma-, and grid-exact in tests). `--pint` switches the
  filenames to the fixed OpenPINT convention
  (`<case>_B10/_N14/_n/_g`); the manifest stays authoritative either
  way. DICOM RT Plan is now bidirectional: `dicom rtplan-info` parses
  a plan into `openbnct.rtplan-summary/0.1.0` (fraction groups,
  referenced-beam metersets, per-beam static delivery geometry), and
  `dicom export-rtplan` writes a minimal static-beam RTPLAN — one
  fraction group, one control point per beam — covering BNCT's
  fixed-field regime. `dicom import-pet` converts a native single-frame
  PET series to a body-weight SUV volume (`PetVolume` — BQML units,
  START decay correction, radiopharmaceutical dose/half-life/start-time
  record, unsigned 16-bit pixels, scan-time decay to injection) on the
  shared CT geometry path, emitting float64 NIfTI for the boron uptake
  model. Both directions are research interop, not commissioned
  planning.

- **R9-07 — inverse planning (provisional).** *Landed, pending the
  IP-boundary note below.* `openbnct.inverse-plan-objective/0.1.0`
  declares dose-volume objectives — `min_eud`, `max_mean`,
  `min_dose_at_volume`, `max_dose_at_volume` on named masks over
  `physical_total`, a named component, or `isoeffective` — and
  `openbnct.inverse-plan-result/0.1.0` records the outcome.
  `openbnct plan optimize` solves the non-negative weight assignment by
  deterministic cyclic coordinate descent with analytic metric
  gradients and per-coordinate Armijo line search; a
  `weight_regularization` term selects the minimum-total-weight
  feasible plan. Deterministic (no sampling), content-bound to the
  objective document, qualified
  `inverse_planning_research_only_not_clinical`. **Boundary note:**
  dose superposition over per-beam dose fields is standard
  radiotherapy-planning mathematics, but weighted recombination of
  transport outputs may intersect the "designated transport-run
  recomposition" item in `docs/IP_BOUNDARY.md` — the feature ships
  provisionally under the same recorded-review requirement until the
  boundary review confirms scope.

- **R9-08 — isoeffective objectives and multi-field sweeps
  (provisional, same boundary note).** `dose_quantity: "isoeffective"`
  embeds an `openbnct.bio` `BiologicalModel` in the objective document
  — its component weights are content-bound with the spec — and folds
  each beam's four component fields into an effective
  `Σ_c w_c·D_c` dose per objective, applying the model's
  `region_weights` override for that objective's mask. The effective
  quantity stays linear in beam weights, so every metric keeps its
  analytic gradient; `microdosimetric_kinetic` semantics and
  fractionation schedules are refused as non-linear. `openbnct plan
  fields` is the multi-field front end: it aims a disk source per beam
  direction through an aim mask (`aim_disk_source_at_centroid` — the
  on-face `UniformDisk` the solver's boundary/uncollided paths accept),
  solves and folds a unit-weight dose bundle per beam, and emits a
  `openbnct.beam-field-set/0.1.0` manifest binding every aimed case,
  position report, and bundle to the shared inputs by hash.
- **R9-09 — HU-to-material calibration.** `openbnct.hu-calibration/0.1.0`
  carries a versioned anchor table — a declared `MaterialDefinition` per
  Hounsfield value — and `openbnct dicom calibrate` maps a CT series (or
  an HU NIfTI already resliced onto the case grid) to a
  `material-assignment` of per-voxel two-component anchor mixtures, the
  Schneider-method structure parameterized by the artifact rather than a
  hard-coded fit. The committed layered-head round-trip recovers the
  phantom assignment voxel-exact at the anchors and solves
  bit-identically; anchor tables are declared conventions, and a real
  deployment substitutes its scanner's stoichiometric fit.

Nothing in R8/R9 changes the deferred list below or adds any clinical
claim; plan optimization involving Avify Dose patent subject matter
remains behind the IP boundary.

## R10 — Field-parity gap closure (draft)

Draft scope, accepted for scheduling. Gap analysis 2026-09-20 against the
current external field (NeuMANTA/NeuPex, JCDS-II/Tsukuba Plan, SERA,
OpenPINT, and the BNCT-SPECT/Compton-camera prompt-gamma literature).
Each item is evidence-gated; none implies a clinical claim.

- **R10-01 — prompt-gamma production source.** The workbench scores the
  ¹⁰B(n,α)⁷Li channel everywhere a boron dose exists; the BNCT-SPECT /
  Compton-camera community consumes spatial 478 keV production maps for
  detector design and image-reconstruction research. Deliverable: a
  versioned `openbnct.prompt-gamma-source` artifact derived from the
  boron dose component (per-capture yield with the declared 93.9%
  branching ratio, monoenergetic 478 keV emission, isotropic
  assumption), hash-bound to its parent dose bundle, with CLI
  derivation and GUI display. Detector modeling, collimation, and
  reconstruction are out of scope — the artifact is the physics source
  term, not an imaging system. *Landed:* `prompt_gamma.rs` in
  openbnct-transport (contract + validation + linear σ propagation),
  `openbnct prompt-gamma` CLI, a derive/export section in the GUI dose
  workspace, the 478 keV map selectable as a dose-map quantity, and
  drop routing for the schema.
- **R10-02 — OpenMC parallelism controls.** *Landed:* the run receipt
  already bound `environment_overlay`; `--env` was already
  general-purpose. Added `--threads N` as a first-class validated alias
  for `OMP_NUM_THREADS` so parallelism is discoverable and recorded. The runner currently
  inherits OpenMC defaults; expose declared thread/particle parallelism
  (OMP thread count, MPI launch where present) on run manifests and the
  GUI run surface so runs are reproducible under resource limits.
- **R10-03 — DICOM MR import and PET/MR-to-CT resampling.** Research
  targeting is MRI-defined; accept MR Image Storage series for display
  and overlay, resample PET/MR volumes onto the case CT grid by declared
  rigid transform (frame-of-reference or explicit matrix), and record
  the registration basis in the artifact. Deformable registration stays
  deferred.
- **R10-04 — beam-direction search.** Extend `plan fields`/`optimize`
  with a discrete direction sweep — enumerate candidate beam directions
  over a declared angular grid, score each by the fast transport path,
  and feed survivors to the weight optimizer. Acceptance: the sweep
  reproduces a known optimum on a synthetic case and is content-bound.
- **R10-05 — S_N plan-iteration quality.** Grow the deterministic solver
  from verification scope toward iteration scope: more groups, wider
  anisotropy support, performance pass. Each increment lands with its
  own accuracy evidence; the solver never silently claims MC-grade
  fidelity.
- **R10-06 — nuclear-data uncertainty propagation.** Read ENDF
  covariance data where published and propagate perturbations through
  the multigroup path to dose-level uncertainty surfaces — a
  differentiator no current BNCT tool offers, matching the project's
  provenance/uncertainty brand.
- **R10-07 — PHITS deck emitter.** Symmetric interoperability: the
  workbench reads PHITS outputs; emit PHITS input for supported
  geometry/source subsets so PHITS-side users can cross-check.
  Acceptance: emitted deck executes under PHITS and matches the
  OpenMC/MCNP reference on a frozen case within declared tolerances.

## R11 — Physics-fidelity frontier and release pipeline (draft)

Draft scope, 2026-09-21. The theta-weighted diamond closure (landed
post-R10) removed a compensating error in the S_N sweep and exposed the
real residual discrepancies below — these are measurable gaps against
independent engines, ordered by leverage. None implies a clinical claim.

- **R11-01 — deep-tail reservoir mechanism: documented data-side
  cause.** Decomposition of the committed TSL+P1 flux and the
  collapsed 28-group data (see `validation/fir1-k63-cylindrical-
  phantom/README.md`, "Deep-tail reservoir decomposition") shows the
  deep thermal tail is sustained by the **fast halo**, not the
  epithermal reservoir: every group below 30 keV decays at κ ≳ 0.9/cm
  while the >1 MeV groups carry κ ≈ 0.15–0.44 and supply 57% of the
  flux at 22.5 cm. The measured tail (κ = 0.32) is bracketed by the
  anisotropy treatment (P0+trcorr κ ≈ 0.24–0.27 vs P1+TSL κ = 0.43)
  and modulated ~45% by the undeclared within-bin shape of the 3-bin
  source's fast bin — committed A/B solves under 1/E vs fission
  weighting (`*-tsl-trcorr-1e` vs `*-tsl-trcorr-fission`). Closure
  requires a published fine-group K63 spectrum (Seppälä 2002 tabulates
  only the 3-group integrals) or a declared intermediate weighting,
  plus a converged P_l solve.
- **R11-02 — water-phantom photon-channel deficit.** Transported
  photon dose at FiR-1 water phantom sits at 0.465× the 20M-history
  OpenMC tally; the voxelwise γ record (5%/20mm) shows photon the
  weakest channel (19.6% pass; boron 76.7%). Requires the θ-WDD solve
  on the 63k-cell phantom and an n→γ production/deposition accounting
  pass. Blocked mainly by solve cost on a laptop.
- **R11-03 — θ-WDD + P1 iteration stiffness.** The weighted closure
  under P1 in-scatter converges far slower than the clamp-era map
  (committed solve: 39 outers; WDD+P1 did not reach 1e-4 within ~5h).
  Measured and rejected (2026-09-22): a thermal-block sub-iteration —
  re-sweeping the upscatter-coupled suffix (g24–27, the only
  significant S(α,β) upscatters) to self-consistency inside each
  outer — implemented and unit-tested green but removed: the stiff
  mode's ~0.85/sweep contraction needs ~60 block passes per outer at
  real sweep cost, so per-outer time swamps the saved outer count
  (first outer alone exceeded the 24-outer baseline's total runtime).
  Anderson depth tuning also saturates: depth 12 converged in the
  same 24 outers as depth 5 at 1e-3 on the cylindrical-phantom case.
  Remaining candidates: a true diffusion-synthetic preconditioner on
  the thermal block (the slow mode is the near-conservative
  σ_a/σ_s ≈ 0.02–0.1 sub-eV block — DSA is the standard answer) or a
  coarser-mesh multigroup preconditioner. Needed before the P1
  validation suite re-runs under the new closure.
- **R11-04 — VR chi-square gate seeds (R6-09 remainder).** Two
  ~196M-history OpenMC runs outstanding for the
  variance-reduction acceptance gate (~27h each on this workstation).
  Pure compute; no code work.
- **R11-05 — licensed-engine execution gates.** The MCNP and PHITS
  emitters produce decks, but end-to-end execution requires licensed
  users. External dependency — track, don't block.
- **R11-06 — ENDF NC-type LTY 1–3 covariances.** Cross-material and
  weighting-function NC subsections are currently skipped with a ledger
  entry; only LTY=0 (derived-quantity reference) resolves. Extend if a
  real evaluation needs them.

## Release pipeline

- **0.2.0 cut** — first release containing the in-house S_N solver,
  photon transport, the verification surfaces (gamma/analytic/
  metamorphic oracles), the planning stack, the GUI workbench, and the
  MIT license. Release notes must state plainly that the closure fix
  changes solver answers on optically thick cells — same-version
  fluxes will not reproduce 0.1.x outputs.
- **Software-artifact paper** — `papers/03-software-artifact.md` is a
  checklist, not a writeup; the publication-grade artifact (validated
  γ figures, covariance chain, oracle results) is the credibility
  deliverable.

## Deferred beyond the research platform

- patient-specific clinical decisions;
- treatment delivery instructions;
- automated segmentation or contour editing;
- facility commissioning claims;
- regulatory submission;
- optimization involving Avify Dose patent subject matter;
- any claim of clinical equivalence to a certified TPS.
