# Usage reference

Detailed command and workflow reference for the OpenBNCT CLI, GUI, and Python
surfaces. For project status see [ROADMAP.md](../ROADMAP.md); for the research
boundary see [DISCLAIMER.md](../DISCLAIMER.md).

## Workspace

```text
crates/openbnct-core/       geometry and component-dose contracts
crates/openbnct-dicom/      strict DICOM import and synthetic geometry benchmark
crates/openbnct-view/       patient-aligned tri-planar view geometry
crates/openbnct-transport/  backend interface and normalized run lifecycle
crates/openbnct-evidence/   hashes, manifests, and qualification boundary
crates/openbnct-openmc/     OpenMC preflight and deterministic input generator
crates/openbnct-njoy/       deterministic NJOY preparation, execution, and evidence
crates/openbnct-cli/        headless entry point
crates/openbnct-gui/        native egui application shell
bindings/python/            bounded PyO3/maturin scientific package
benchmarks/synthetic/       public, non-patient validation corpus
profiles/                   reviewed external-data acquisition profiles
schemas/                    versioned interchange schemas
docs/                       architecture, decisions, and qualification records
integrations/               bounded external orchestration specimens
```

## Build

The workspace pins Rust 1.95, the minimum required by eframe 0.36.1. Once the
toolchain is installed:

```text
cargo test --workspace --all-targets
cargo run --bin openbnct
cargo run --bin openbnct-gui
```

Generate and independently verify the first synthetic DICOM case:

```text
cargo run --bin openbnct -- benchmark generate /tmp/nf-bnct-001
cargo run --bin openbnct -- benchmark verify /tmp/nf-bnct-001
cargo run --bin openbnct-gui -- /tmp/nf-bnct-001
```

## End-to-end research workflow

The canonical reproducible path — the same stages regardless of surface
(CLI, workbench, Python):

1. **Case** — generate the synthetic benchmark (`benchmark generate`) or
   assemble a real case directory: `case.json` (grid geometry), frozen
   `material.json` with declared nuclides, `source.json`, execution
   profile, and a `beam bind` step that writes the declared beam onto the
   case's entry face (`beams/fir1-k63.json` is the worked example).
2. **Response data** — the NJOY chain (`njoy prepare` → execution →
   suitability reports → `generate-response-tables` →
   `verify-response-tables`) produces an independently reviewed,
   material-bound response set. Deck generation refuses unreviewed sets.
3. **Transport** — `openmc run` prepares the deterministic deck,
   executes, and collects a `physical-dose-bundle` with per-component
   absolute uncertainty; `--evidence-root` exports every input and the
   run receipt, all content-bound by SHA-256.
4. **Characterize** — `beam qa` emits in-air fluence metrics and, given
   `--dose`, in-phantom advantage depth/ratio/peak therapeutic ratio plus
   the boron-capture depth profile. `--reference` compares against
   published scalars with declared tolerances.
5. **Verify against measurement** — `measurement compare` checks a
   `measurement-record` against the QA report: scalars by sigma and
   relative difference, histogram depth profiles by peak-normalized
   shape with per-bin chi-square.
6. **Inspect** — the workbench loads the case, washes a matching dose
   bundle over the geometry, and verifies an exported
   `artifact-manifest.json` in place.

Every artifact between steps is versioned and hash-bound, so any report
can be traced back through the chain — that provenance is the point.

Generation refuses to overwrite an existing destination. Generated DICOM files
are ignored by default and contain visibly synthetic identity values only.

Without a case argument, the GUI opens on a research-readiness overview. Passing
a verified case opens its geometry workspace directly. Use the left navigation
to see the current OpenMC capability gates, the dose workspace, and the evidence
ledger. The transport workspace also exposes the same source-positioning helpers
as `openbnct position` — aim a source at an ROI centroid across an approach
axis, inspect the position report, and rotate by quarter-turns — sharing the
authoritative `openbnct_transport` path with the CLI and Python. The dose
workspace loads any validated physical or biological bundle
file — component statistics, totals, a region-mask DVH plot, and region
dose-volume metrics (`D_x`, `V_x`, EUD via the same `RegionDoseMetrics` path
as the CLI and Python, exportable as `openbnct.dose-metrics/0.1.0`) — while
keeping the two layers visually distinct. The geometry workspace can
additionally wash a loaded dose over the patient image: the bundle must
declare the same `case_id` and an equivalent grid, after which any component
or total renders as a hot-ramp overlay with opacity and %of-max threshold
controls and a live dose readout at the linked voxel. A NIfTI section inspects
`.nii`/`.nii.gz` volumes, writes region masks, resamples onto a bundle's
grid, and exports dose volumes — the same `openbnct-nifti` paths as
`openbnct nifti`. The evidence workspace can verify an exported
`artifact-manifest.json` in place. Select the `?` button or press `F1` for
contextual guidance, bundled offline answers, and guided tours that dim the
application and spotlight live workflow controls. Interactive transport actions
stay disabled until the upstream response gates are qualified, and the interface
never shows placeholder dose values. See [ADR
0014](../docs/adr/0014-evidence-aware-workbench-shell.md).

`pip install openbnct` is the primary distribution path for scientific
users, backed by the same Rust implementation through PyO3 and maturin. The
first bounded API is implemented under `bindings/python` and exercised by a
cross-language parity suite. Release wheels (Linux x86_64/aarch64, macOS
x86_64/arm64, Windows x86_64, plus sdist) publish through OIDC trusted
publishing — TestPyPI on manual dispatch, PyPI on `v*` tags; all 15
`openbnct*` crates are on crates.io (`cargo install openbnct-cli`). Cargo
remains the native source/developer path, while desktop releases will ship as
native artifacts. See [ADR
0015](../docs/adr/0015-python-and-native-distribution.md) and [ADR
0027](../docs/adr/0027-first-bounded-python-api.md).

### Independent DICOM validation

CI pins Ubuntu 24.04's `dicom3tools` snapshot `20240118131615` and runs both
`dciodvfy` and `dcentvfy` against a newly generated case. With those tools on
your path, the same strict gate is:

```text
scripts/validate-dicom-iod.sh /tmp/nf-bnct-001
```

The gate rejects validator warnings as well as errors. Passing these tools is
useful interoperability evidence, not a DICOM certification or a guarantee of
clinical fitness.

### Nuclear-data acquisition

OpenBNCT will not download multi-gigabyte nuclear data as a hidden build step.
First make a one-byte probe of the frozen official OpenMC profile:

```text
cargo run --bin openbnct -- openmc data probe \
  --profile profiles/openmc/openmc-endfb81-official-library.json
```

Acquisition requires the exact reported byte count and an existing output
directory. It writes a resumable `.part` file and a JSON receipt without
overwriting completed output. The official processed archive currently has no
published digest, so its receipt deliberately remains `acquisition_only`; a
locally calculated SHA-256 is byte identity, not scientific qualification. See
[ADR 0010](../docs/adr/0010-verifiable-nuclear-data-acquisition.md).

After selective extraction, independently verify the checked manifest and the
material-specific capabilities with:

```text
cargo run --bin openbnct -- openmc data verify-manifest \
  --manifest benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json \
  --data-root PATH-TO-SELECTED-OPENMC-DATA \
  --material benchmarks/synthetic/nf-bnct-001/transport/material.json
```

### NJOY input preparation

After acquiring and extracting the exact evaluated-neutron selection, generate
a new reviewable bundle with:

```text
cargo run --bin openbnct -- njoy prepare \
  --selection benchmarks/synthetic/nf-bnct-001/transport/evaluated-neutron-source-selection.json \
  --material benchmarks/synthetic/nf-bnct-001/transport/material.json \
  --generation-method benchmarks/synthetic/nf-bnct-001/transport/response-generation-method.json \
  --profile profiles/openmc/endfb81-neutron-evaluations.json \
  --receipt benchmarks/synthetic/nf-bnct-001/transport/provenance/endfb81-neutron-acquisition-receipt.json \
  --evaluations-directory PATH-TO-EXACT-SELECTION \
  --output NEW-OUTPUT-DIRECTORY
```

The command executes no external processor and refuses an existing output
directory. The frozen benchmark copy is under
`benchmarks/synthetic/nf-bnct-001/transport/njoy/`; see [ADR
0011](../docs/adr/0011-deterministic-njoy-input-preparation.md).

### Controlled NJOY execution evidence

`openbnct njoy execute` requires the same five content-bound source documents,
the exact prepared bundle, a real NJOY executable, explicitly declared runtime
support artifacts, and a new output directory. Run `openbnct njoy execute
--help` for the complete argument contract. It preserves a receipt before
returning a failure when NJOY reports a kinematic violation.

An execution directory can be checked later against an external receipt:

```text
cargo run --bin openbnct -- njoy verify-execution \
  --receipt benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-execution-receipt.json \
  --execution-directory PATH-TO-COMPLETE-EXECUTION-DIRECTORY
```

The first canonical receipt is intentionally
`execution_observed_diagnostics_failed`, not a response table or reference
result. See [ADR 0012](../docs/adr/0012-controlled-njoy-execution-evidence.md) and
the [structured finding summary](../docs/research/NJOY2016_78_KINEMATIC_FINDINGS.md).

Derive the separately versioned data-suitability gate from a verified root:

```text
cargo run --bin openbnct -- njoy assess-execution \
  --receipt benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-execution-receipt.json \
  --execution-directory PATH-TO-COMPLETE-EXECUTION-DIRECTORY \
  --output NEW-SUITABILITY-REPORT.json
```

The canonical assessment is `transported_photon_kerma_rejected`: O-17 and O-18
have no photon-production files, N-15 lacks File 12, and O-16 has a potentially
incomplete discrete photon sequence. See [ADR
0013](../docs/adr/0013-transported-photon-kerma-suitability.md).

That immutable v0.1 report records the processor messages conservatively.
Source-aware v0.2 evidence subsequently recognizes valid File 13 alternatives,
and domain-aware v0.3 evidence scopes kinematic findings to a material- and
manifest-bound OpenMC interval without deleting full-range diagnostics. For
the JEFF-4.0 investigation this clears O-16's sole 30 MeV finding from the
bounded 20 MeV decision. An independent H-2 LAW=7 calculation confirms
normalized source distributions and positive mean energy for the implicit
proton, and a receipt-bound comparison attributes all 15 H-2 findings to
NJOY's excluded File 6 energy-balance remainder. Evidence-aware v0.4 consumes
that exact evidence and reclassifies only H-2 while independently restoring
N-15's capture-balance rejection. C-13, O-17, and O-18 retain 102 in-domain
findings. Diagnostic triage preserves all 102 while separating 59 findings on
the source-data-blocked C-13/O-18 runs from 43 O-17 findings. All 43 O-17
excesses now reproduce NJOY's printed per-reaction energy-balance accounting,
but that same-processor attribution waives none of them and cannot replace an
independent physical validation; the candidate remains rejected. See [ADR
0017](../docs/adr/0017-source-aware-photon-production-suitability.md), [ADR
0019](../docs/adr/0019-independent-mf6-capture-photon-balance.md), and [ADR
0020](../docs/adr/0020-content-bound-transport-domain-suitability.md), followed by
[ADR 0021](../docs/adr/0021-independent-law7-implicit-residual-balance.md).
The processor attribution is frozen in [ADR
0022](../docs/adr/0022-law7-processor-attribution.md), and the integrated decision
is specified by [ADR
0023](../docs/adr/0023-reaction-evidence-aware-suitability.md), and the bounded
work queue by [ADR
0025](../docs/adr/0025-diagnostic-triage-of-remaining-njoy-findings.md), and the
O-17 attribution and response-path pause by [ADR
0026](../docs/adr/0026-o17-processor-energy-balance-attribution.md); that pause is
superseded by [ADR
0031](../docs/adr/0031-o17-diagnostic-queue-dispositioned.md), under which the
first component response tables are generated from the receipt-bound
production HEATR output and sealed `independently_reviewed` by deterministic
in-house regeneration.

### Facility beam descriptions

A `openbnct.beam-description/0.1.0` document records a facility beam as
delivered at its port reference plane: the source term (energy spectrum,
angular distribution, spatial distribution matching the aperture), the
port geometry, a declared normalization basis, and provenance citing the
published reference or measured characterization every number came from.
The repository's `beams/` registry currently ships one encoded literature
beam — `fir1-k63.json`, the FiR 1 K63 epithermal column from
Seppälä (2002, HU-P-D103), explicitly labeled a literature reconstruction
(group-integrated fluences are measured; within-group shape and the
collimator-derived divergence cone are stated assumptions).

```text
openbnct beam list                          # scan ./beams
openbnct beam info --beam beams/fir1-k63.json
openbnct beam bind \
  --beam beams/fir1-k63.json \
  --case benchmarks/synthetic/nf-bnct-001/transport/case.json \
  --output bound-case.json
```

`bind` repositions the beam's source onto the case: the port plane lands
just inside the bounding-box face the beam enters (sign of propagation
picks the side), centered on that face, keeping the declared aperture. An
aperture that does not fit the face is rejected rather than clipped. The
bound case feeds `openmc generate` directly — extract its `source` member
as the `--source` artifact so content binding stays honest.

The underlying transport model supports `uniform_disk` spatial,
`isotropic_cone` angular, and `tabulated_histogram` energy distributions;
the OpenMC emitter maps them to `cylindrical`/`mu-phi`/`tabular` XML and
the MCNP deck emitter to `POS/AXS/RAD`, `DIR` cosine histograms, and
`ERG` histograms. Cone half-angles are bounded below π/2 (no upstream
emission) and must be coaxial with the port normal.

`beam qa` emits a `openbnct.beam-quality/0.1.0` report: in-air metrics
(TECDOC-1223 group fluence rates, fractions, current-to-fluence ratio,
port area, mean energy) are exact properties of the declared source;
`--reference` compares against published/measured values inside declared
relative tolerances; `--dose` plus `--tumor-weights`/`--normal-weights`
(`B=w,H=w,N=w,P=w` compound effectiveness factors) adds in-phantom
metrics — depth profiles inside the aperture footprint, advantage depth,
advantage ratio, and peak therapeutic ratio.

```text
openbnct beam qa --beam beams/fir1-k63.json \
  --report-id openbnct.beam-quality.fir1-k63.v1 \
  --reference beams/references/fir1-k63.json \
  --output beams/qa/fir1-k63.json
```

The committed report reproduces the published group fluences within
tolerance and honestly *fails* `current_to_fluence_ratio` (modeled 0.994
vs measured 0.77): the fixture's conservative collimator-bound cone
cannot reproduce measured penumbra divergence — the report records
exactly that fidelity gap.

### Accelerator sources and beam-shaping assemblies

`openbnct accelerator` evaluates a parametric `⁷Li(p,n)⁷Be` thick-target
neutron source and writes an `openbnct.accelerator-source/0.1.0` record:

```text
openbnct accelerator source \
  --id openbnct.accelerator-source.example.v1 \
  --proton-energy-mev 2.5 --proton-current-ma 1.0 \
  --target-thickness-um 100 --port-radius-cm 5 \
  --output source.json \
  --beam-output beam.json --beam-id beam.example
```

The model integrates the Liskien–Paulsen recommended 0° differential
cross sections over the proton slowing path through the lithium target
(Bethe stopping, ICRU mean excitation for Li) and applies exact
nonrelativistic two-body kinematics — the forward neutron energy is
≈29.7 keV at reaction threshold and ≈0.79 MeV at a 2.5 MeV proton.
Omit `--target-thickness-um` for a fully thick target (protons stop
below threshold inside the Li); the record reports the required
thickness either way. The emitted spectrum is a normalized
`tabulated_histogram` and the optional `--beam-output` writes a
ready-to-bind `openbnct.beam-description` whose `computed_model`
provenance binds the source artifact by SHA-256 — it feeds
`beam bind`, `beam qa`, and `openmc generate` directly. `accelerator
beam` regenerates the beam description from an existing source record.
The raw target spectrum is fast-neutron dominated (≈30–800 keV);
epithermal content requires downstream moderation — which is what the
BSA layer is for.

`openbnct bsa` declares a `openbnct.beam-shaping-assembly/0.1.0`
document: an ordered stack of moderator/filter/reflector/collimator/
delimiting-aperture layers along a beam axis, each with a thickness, a
material definition, and a radial footprint (`full`, `disk`, or
`annulus` — annuli leave their bores empty, which is how collimator
walls and aperture rings are expressed).

```text
openbnct bsa info --assembly bsa.json
openbnct bsa rasterize --assembly bsa.json --case case.json \
  --output assignment.json
openbnct bsa sweep --spec sweep.json --base bsa.json \
  --output-dir variants/ --record sweep-record.json
```

`rasterize` overlays the stack onto a transport case's scoring grid and
emits a `MaterialAssignment` (the same artifact the DICOM pipeline
emits), so a swept assembly slots into deck generation without a CSG
modeler. `sweep` takes a `openbnct.bsa-sweep/0.1.0` spec listing
candidate thicknesses per named layer, writes every Cartesian variant
as its own assembly document, and emits a sweep record binding the
spec, base, and variants by content hash.

### Measurement import and comparison

Verification against measurements uses two more versioned documents.
`openbnct.measurement-record/0.1.0` records measured points with method
(activation foil, ion chamber, TLD, TEPC, fission chamber), explicit
unit, position provenance, and one-sigma uncertainty — scalar or
histogram (lineal-energy spectrum) values. `openbnct measurement
compare` resolves each measurement's canonical metric name against a
beam-quality report and emits a `openbnct.measurement-comparison/0.1.0`
record binding both inputs by content hash: per-point relative
difference, sigma-normalized difference, and a chi-square summary.

```text
openbnct measurement info --record measurements/fir1-k63-free-beam.json
openbnct measurement compare --record measurements/fir1-k63-free-beam.json \
  --against beams/qa/fir1-k63-phantom.json \
  --report-id openbnct.measurement-comparison.fir1-k63.v1 \
  --output measurements/fir1-k63-vs-beam-quality.json
```

Points whose source states no uncertainty are compared by relative
difference only and counted as `without_uncertainty` — the record never
invents a sigma to produce a pass. The committed FiR 1 record is such a
case: Seppälä's Table 4 values are digitized but their reported
uncertainties were not transcribed, so all five points compare honestly
without pass/fail — including the expected 29% J/Φ gap.

Histogram-valued measurements additionally resolve against depth
profiles in a beam-quality report when the metric names one:
`thermal_fluence_depth_profile` or `boron_dose_depth_profile` resolve to
the boron-capture profile (proportional to thermal fluence under uniform
dilute loading), and `tumor_dose_depth_profile` /
`normal_tissue_dose_depth_profile` resolve to the weighted profiles.
Edges carry the measurement's `unit` (cm for depth profiles); each bin
is compared at its center against the linearly interpolated computed
profile, and both sides are peak-normalized — published activation
profiles are relative, so this is a shape comparison, the standard
foil-scan convention. Per-bin results (normalized values, relative
differences, sigma-normalized differences when bin uncertainties are
stated, chi-square) land in the comparison record's
`profile_comparisons`; histograms that resolve to no profile are still
reported unmatched, never silently dropped.

### RT Dose export

`openbnct dicom export-rtdose` writes one volume of a physical dose
bundle as a multi-frame DICOM RT Dose object: unsigned 32-bit pixels
scaled by DoseGridScaling, with ImagePositionPatient /
ImageOrientationPatient / GridFrameOffsetVector addressing the voxel
grid. `--component total|B|N|H|P` selects the physical total (default)
or a named component; `--ct-series <dir>` attaches the source CT via
ReferencedSOPSequence and adopts its study/frame-of-reference identity.
Absolute-gray volumes declare `DoseUnits=GY`; per-source-particle
quantities honestly declare `RELATIVE` with the true unit in
DoseComment. Exported objects are marked research-only. The export
round-trips in pydicom with grid fidelity at the 32-bit quantization
floor (verified max relative deviation <1e-6 on a 40³ bundle).

```text
openbnct dicom export-rtdose --bundle dose.json --component total \
  --ct-series /path/to/ct --output dose.dcm
```

### RT Plan read and export

`openbnct dicom rtplan-info` summarizes an RT Plan file — label, plan
intent, patient identity, fraction groups with their referenced-beam
metersets, and each beam's static delivery geometry (gantry, collimator,
and couch angles, isocenter, SSD, machine) — as an
`openbnct.rtplan-summary/0.1.0` record:

```text
openbnct dicom rtplan-info --input plan.dcm --output plan-summary.json
```

`openbnct dicom export-rtplan` writes a minimal static-beam RT Plan:
one fraction group referencing each `--beam` declaration
(`name,gantry_deg,collimator_deg,couch_deg,iso_x,iso_y,iso_z,sad_mm,ssd_mm,radiation_type,meterset[,energy_mev]`).
The writer covers BNCT's fixed-field regime only — single control point
per beam, no MLC or dynamic delivery:

```text
openbnct dicom export-rtplan --plan-label RESEARCH-1 --fractions 2 \
  --frame-of-reference-uid 2.25.123 \
  --beam "AP,90,0,0,0,0,25,1800,1775,NEUTRON,120" \
  --beam "PA,270,0,0,0,0,25,1800,1775,NEUTRON,100" \
  --output plan.dcm
```

Both directions are research interop, not commissioned treatment
planning.

### OpenMC input generation

With the sealed response set in place, generate the deterministic OpenMC deck
for the frozen smoke profile:

```text
cargo run --bin openbnct -- openmc generate \
  --case benchmarks/synthetic/nf-bnct-001/transport/case.json \
  --component-profile benchmarks/synthetic/nf-bnct-001/transport/component-profile.json \
  --material benchmarks/synthetic/nf-bnct-001/transport/material.json \
  --source benchmarks/synthetic/nf-bnct-001/transport/source.json \
  --response-set benchmarks/synthetic/nf-bnct-001/transport/provenance/neutron-response-set.json \
  --nuclear-data-manifest benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json \
  --execution-profile benchmarks/synthetic/nf-bnct-001/transport/openmc-smoke-profile.json \
  --nuclear-data-root PATH-TO-SELECTED-ENDFB81-HDF5-ROOT \
  --output NEW-DECK-DIRECTORY
```

The generator verifies every content binding, requires the response set to
pass `validate_for_folding` (`independently_reviewed` under the ADR 0031
in-house deterministic-verification path), confirms the response energy range
covers the selected data, and refuses an existing output directory. The deck
executes under OpenMC 0.16.0 at commit
`617d35a5063c57796b43428bc401e627d2011046` with `OPENMC_CROSS_SECTIONS`
pointed at the manifest's `cross_sections.xml`.

After execution, `scripts/compare-openmc-smoke-estimators.py` binds the
statepoint to its input manifest and sealed response set, checks the tally
contract and executed energy-function tables, and freezes the ADR 0007
correlated-diagnostic estimator comparisons (coupled-heating closure,
component sum versus dedicated neutron heating, and reaction-rate times
evaluated mean deposited energy for B-10 and N-14) as a content-hashed report:

```text
python3 scripts/compare-openmc-smoke-estimators.py \
  --statepoint DECK-DIRECTORY/statepoint.5.h5 \
  --input-manifest DECK-DIRECTORY/openbnct-input-manifest.json \
  --response-set benchmarks/synthetic/nf-bnct-001/transport/provenance/neutron-response-set.json \
  --material benchmarks/synthetic/nf-bnct-001/transport/material.json \
  --execution-profile benchmarks/synthetic/nf-bnct-001/transport/openmc-smoke-profile.json \
  --execution-root PATH-TO-NJOY-EXECUTION-ROOT \
  --execution-receipt benchmarks/synthetic/nf-bnct-001/transport/provenance/njoy2016-78-execution-receipt.json \
  --report-id openbnct.nf-bnct-001.openmc-smoke-estimator-comparison.v1 \
  --output NEW-COMPARISON-REPORT.json
```

`openmc collect` then imports the completed run into the platform result
model. The collector reads the newest `statepoint.N.h5` with a pure-Rust HDF5
path, refuses a nonzero exit code or an existing output, binds the run header
(batches, particles per batch, seed, stride, and the statepoint's recorded
OpenMC version) and every tally contract to the deck's input manifest, and
normalizes each component tally under its manifest-declared semantics into
gray per source neutron. The coupled-heating tally — no component, no
particle filter — supplies the dedicated physical total; particle-filtered
audit heating stays out of the bundle. The emitted
`openbnct.physical-dose-bundle/0.2.0` carries per-voxel 1-sigma uncertainties
and a provenance id binding both the input-manifest and statepoint SHA-256:

```text
openbnct openmc collect \
  --working-directory DECK-DIRECTORY \
  --exit-code 0 \
  --output NEW-DOSE-BUNDLE.json
```

The collected smoke bundle is execution evidence only — a five-batch,
thousand-history run cannot produce reference values, and the bundle makes no
clinical or qualification claim.

### DICOM-derived material assignment

`benchmark derive-materials` turns verified RT Structure Set masks into a
transport-neutral `openbnct.material-assignment/0.2.0` artifact: named,
non-overlapping voxel regions that each carry a `MaterialDefinition`. An ROI
that fills its bounding box exactly becomes a `voxel_box` region (realized as
an exact CSG cell); any other mask becomes a `voxel_set` region listing its
member voxels explicitly — an exact representation, never an approximation —
and the artifact binds the source `case.json` by SHA-256 provenance:

```text
openbnct benchmark derive-materials \
  --case-root CASE-ROOT \
  --case transport/case.json \
  --base-material transport/material.json \
  --map examples/derived/material-map.json \
  --output-assignment NEW-ASSIGNMENT.json \
  --output-case NEW-DERIVED-CASE.json
```

`--map` is a JSON object `{"regions": {"ROI_NAME": "material-file.json"}}`
whose paths resolve relative to the map file. By default region names are
looked up in the case's RT Structure Set; `--mask NAME=path` (repeatable)
instead binds names to external `RegionMask` JSON files — e.g. produced by
`openbnct nifti to-mask` — whose voxel array must match the case grid
exactly. When any `--mask` is supplied every mapped key must resolve to one
of them. The derived transport case reuses the verified DICOM geometry and
is written alongside the assignment. `examples/derived/` ships a runnable
demonstration that unloads boron from the `CORE` box.

`openmc generate --assignment` then builds a multi-cell deck: one OpenMC
material per distinct region material plus, for box-only assignments, one
CSG cell per region box with the base cell carved by the region complements.
Assignments containing any `voxel_set` region instead emit a rectilinear
material lattice spanning the whole grid — one universe per distinct
material, one lattice element per voxel — so arbitrary masks assign
materials exactly. Both paths require an axis-aligned (identity-direction)
grid. Generation gates keep the
result scientifically meaningful — the assignment's base material must equal
the bound material artifact byte-for-byte, region density and temperature
must match (collection still assumes one voxel mass), regions may not
introduce nuclides absent from the base material, and only nuclides covered
by `njoy_partial_kerma_fluence_fold` component estimators may change
fraction; uncovered nuclides must match the base exactly so the residual
response tables stay valid.

At collection the folded-response tallies still encode the base material's
atom densities, so `openmc collect` applies a per-voxel region/base
mass-fraction ratio to each covered component's values and 1-sigma
uncertainties, leaving residual, photon, and native-heating components
untouched. The emitted assignment and component profile are copied into the
deck directory and hash-verified against the manifest before any correction
is applied.

### Candidate-reference runs and acceptance evaluation

Candidate-reference execution profiles (`purpose: candidate_reference`,
profile schema `0.2.0`) must bind a predeclared acceptance contract via
`openmc generate --acceptance`; smoke profiles may not bind one. The contract
(`openbnct.acceptance-contract/0.1.0`) declares the acceptance regions — each
realized as its own OpenMC mesh so region sums carry proper batch statistics —
the evaluated mean deposited energies for the reaction-rate audits, the
precision and estimator-comparison gate tolerances, the frozen seed set, and
the minimum batch count. The generator emits one mesh plus nine ROI-scoped
tallies per region, binds the contract hash into the input manifest, and
writes the contract JSON into the deck directory.

`OpenMcBackend` can now drive a run itself: `prepare` generates the deck from
the configured artifact set, and `execute` launches the configured binary in
the run directory, captures stdout/stderr, and freezes an
`openbnct.openmc-run-receipt/0.1.0` recording the executable hash, environment
overlay, timestamps, exit code, and content hashes of every log and
statepoint artifact.

`openmc evaluate` reads each run directory's manifest, contract, and
statepoint; verifies seed registration and uniqueness, run-header and
tally-contract bindings, and the manifest's acceptance binding; then applies
the predeclared gates — ROI precision, per-voxel precision at or above 20% of
each component's maximum, and the estimator comparisons — plus reduced
chi-square consistency across independent seeds. It emits a content-hashed
`openbnct.openmc-acceptance-report/0.1.0`:

```text
openbnct openmc evaluate \
  --run RUN-DIRECTORY-SEED-A --run RUN-DIRECTORY-SEED-B --run RUN-DIRECTORY-SEED-C \
  --output NEW-ACCEPTANCE-REPORT.json
```

These are conformance thresholds for the synthetic benchmark, not clinical
commissioning tolerances; a passing report earns reference-result status for
the run set only within the case's declared qualification ceiling.

### Biological interpretation and dose-volume histograms

`openbnct-bio` is a separately versioned interpretation layer. A
`openbnct.biological-model/0.2.0` artifact assigns dimensionless
effectiveness weights to the four physical dose components, with optional
per-region overrides; `bio apply` produces a
`openbnct.biological-dose-bundle/0.2.0` whose weighted values never alias
physical dose (`weighted_gray*`/`weighted_eqd2` unit labels, a
`synthetic_research_only` qualification, and content hashes binding the
model and physical bundle). The biological total's uncertainty is the
fully-correlated sum of the weighted component sigmas, since all components
share transport histories.

Two weight-semantics families are supported. `fixed_per_component` treats
the weights as free research factors; `photon_isoeffective` declares them
photon-isoeffect factors (RBE/CBE relative to photon dose) and requires
every photon weight — defaults and all region overrides — to be exactly
1.0. A model may also declare a linear-quadratic `fractionation` block
(fraction count, source particles per fraction, a default α/β, and
optional per-region α/β overrides): the weighted per-source-particle
total is then transformed to a photon-isoeffective EQD2,
`n·d·(1 + d/(α/β)) / (1 + 2/(α/β))` with `d` the per-fraction dose, while
component volumes keep their linear weighted values and the applied
schedule is recorded in the bundle. Weight and α/β region selections are
independent — each uses the first matching mask in its own map's order.
Models carry a free-text `validity_domain` for provenance.

A second, separately versioned model family covers stochastic
microdosimetry: `openbnct.microdosimetric-model/0.1.0` artifacts carry
linearized-MKM parameters — per-component α₀/β (the cell system's photon
LQ response) and each component's dose-mean lineal energy, either a
constant or resolved from a `openbnct.lineal-spectrum/0.1.0` document —
plus the spherical domain geometry and a mandatory `validity_domain`.
`bio apply` routes on the model's `schema_version`, and components naming
a spectrum source require a matching `--spectrum` document:

```text
openbnct bio spectrum \
  --record TEPC-MEASUREMENT-RECORD.json \
  --measurement boron-lineal \
  --weighting event_frequency \
  --output LINEAL-SPECTRUM.json
openbnct bio lineal-mean --spectrum LINEAL-SPECTRUM.json
openbnct bio apply \
  --model MKM-MODEL.json \
  --physical-bundle DOSE-BUNDLE.json \
  --spectrum LINEAL-SPECTRUM.json \
  --output NEW-BIO-BUNDLE.json
```

Spectra can also come from the deterministic transport solve instead
of a measurement record: `openbnct bio lineal-tally` evaluates a
`openbnct.lineal-tally-spec/0.1.0` artifact — a spherical site diameter
and a declared per-component secondary table (emission energy, effective
range, collision share) — over an `openbnct.multigroup-flux/0.1.0` solve.
Each collision emits a rectilinear secondary depositing
`ε(l) = E·min(1, l/R)` over the sphere's isotropic chord distribution
(mean chord `2d/3`), and the rate-weighted domain-integrated event
spectrum `f(y)` lands as a unit-normalized `event_frequency`
`openbnct.lineal-spectrum/0.1.0` — directly consumable by MKM
`computed_spectrum` sources. The model is declared-data microdosimetry:
no straggling or sub-site structure, honest about its approximation in
the artifact's note. A committed NF-BNCT-003 spec + flux pair exercises
the path in CI.

```text
openbnct bio lineal-tally \
  --case transport/case.json \
  --data transport/multigroup-data.json \
  --flux transport/multigroup-flux.json \
  --spec transport/lineal-tally-spec.json \
  --id openbnct.case.lineal-spectrum.v1 --output NEW-SPECTRUM.json
```

Each physical component is converted to a photon-equivalent dose via the
MKM effective `α* = α₀ + β·z̄₁D` with `z̄₁D = ȳ_D/(ρ·π·r_d²)`, and the
total sums them; the emitted bundle marks `microdosimetric_kinetic`
semantics, an `mkm_weighted_*` unit, and a `microdosimetry` block binding
the resolved lineal energies and spectrum hashes. MKM outputs are
research artifacts — they assert no clinical RBE/CBE/Gy-Eq claim, and a
weight model cannot claim `microdosimetric_kinetic` semantics.

The NF-BNCT-001 specification's exclusion of CBE/RBE/Gy-Eq claims is
preserved: biological bundles exist only when a model artifact is supplied,
and demonstration models plus a core-region mask live under
`examples/biological/`:

```text
openbnct bio apply \
  --model examples/biological/fixed-component-weights-model-v1.json \
  --physical-bundle DOSE-BUNDLE.json \
  --region-mask core=examples/biological/core-region-mask.json \
  --output NEW-BIO-BUNDLE.json
```

`openbnct bio sweep` runs a one-parameter sensitivity sweep: it varies a
declared parameter (`component:<name>`, `region_weight:<region>:<component>`,
`alpha_beta:default`, `alpha_beta:<region>`, `fraction_count`, or
`source_particles_per_fraction`) over an explicit value list, re-validating
and re-applying the model at each point, and records the region-masked
min/mean/max of the biological total as
`openbnct.bio-sensitivity-sweep/0.1.0` bound to the model and bundle
hashes:

```text
openbnct bio sweep \
  --model examples/biological/photon-isoeffective-lq-model-v1.json \
  --physical-bundle DOSE-BUNDLE.json \
  --region-mask core=examples/biological/core-region-mask.json \
  --region core --parameter alpha_beta:core --value 3,10,20 \
  --output NEW-SWEEP.json
```

Python exposes the same path as `sweep_biological_model`.

### Multi-exposure accumulation

`openbnct accumulate` implements weighted irradiation-fraction and
multi-field aggregation under an `openbnct.exposure-plan/0.1.0` contract.
Each exposure binds a physical dose bundle by SHA-256 plus an explicit
delivery weight, weight basis, optional duration, and a boron-assumption
record. Accumulation sums `weight * dose` and propagates 1-sigma
uncertainties in quadrature — the only supported covariance model is
statistical independence between exposures; within each exposure the
dedicated physical-total estimator already accounts for component
covariance, so the accumulated total sums exposure totals rather than
recombining components. Every bundle must share the grid, component
profile, component set, and dose unit; the output is an ordinary
`openbnct.physical-dose-bundle/0.2.0` usable by `dvh`, `bio apply`, and the
GUI:

```text
openbnct accumulate \
  --plan examples/exposure/two-field-plan.json \
  --output accumulated-dose.json
```

`examples/exposure/` ships a two-field demonstration plan.

### External component-dose import

`openbnct import interchange` ingests a
`openbnct.component-dose-interchange/0.1.0` document — a transport-neutral
record an external pipeline (MCNP, PHITS, Geant4, or a custom tool) emits —
and validates it into an ordinary `openbnct.physical-dose-bundle/0.2.0`:

```text
openbnct import interchange \
  --file examples/interchange/phits-synthetic-dose.json \
  --output imported-dose.json
```

The document declares the producing system, its version, and its
normalization; carries the transport-neutral grid and all four physical
component volumes (boron/nitrogen/hydrogen/photon) with optional per-voxel
sigmas; and chooses a total mode. `dedicated` preserves a producer-tallied
total and its estimator sigmas; `component_sum` sums the components and
records `unavailable` total uncertainty — imported component sums never
claim a dedicated-estimator uncertainty they do not have. The bundle's
`provenance_id` binds the document's SHA-256
(`interchange:<system>:sha256:<hash>`), so imported results keep their
external identity downstream through `dvh`, `metrics`, `bio apply`,
evidence bundles, and the Python `import_component_dose` parity surface.
`examples/interchange/` ships a synthetic PHITS-labeled fixture (analytic
stand-in values, not PHITS output) demonstrating the format.

Two native adapters generate that document directly. `openbnct import
mcnp` reads ASCII `meshtal` files — each component mapping names a file,
tally number, and optional energy bin (`file:tally[:energy-bin]`), and MCNP
relative errors import as absolute per-voxel sigmas:

```text
openbnct import mcnp \
  --case-id my-case \
  --unit gray_per_source_particle \
  --normalization "per source particle; F4 flux-to-dose fold" \
  --component boron=meshtal:14 \
  --component nitrogen=meshtal:24 \
  --component hydrogen=meshtal:34 \
  --component photon=meshtal:44 \
  --output mcnp-dose.json
```

`openbnct import phits` reads PHITS `xyz`-mesh output (e.g. `t-deposit`
`.out` files) — each mapping names a file with an optional energy index
(`file[:energy-index]`); inline `r.err` columns are preferred, otherwise a
sibling `*_err` file supplies relative errors (partial or mismatched error
coverage is rejected). PHITS tally files do not reliably record the code
version, so `--producer-version` is required:

```text
openbnct import phits \
  --case-id my-case \
  --unit gray_per_source_particle \
  --normalization "unit=0 deposit dose per source" \
  --producer-version 3.34 \
  --component boron=d_boron.out \
  --component nitrogen=d_nitrogen.out \
  --component hydrogen=d_hydrogen.out \
  --component photon=d_photon.out \
  --output phits-dose.json
```

Both adapters emit the same interchange document and pass it through the
shared validator, so the resulting bundle is identical in kind to the
interchange path above — the Python `import_mcnp_meshtal` and
`import_phits` functions are parity surfaces (as is `import_nifti` for
the NIfTI path below). The parsers are built
against documented formats; acceptance against real MCNP/PHITS-produced
files is an open R4 gate.

`openbnct export mcnp` goes the other direction — it emits an MCNP input
deck for a transport case:

```text
openbnct export mcnp \
  --case benchmarks/synthetic/nf-bnct-001/transport/case.json \
  --xs-suffix 80c \
  --seed 42 \
  --output deck.i
```

The deck carries the scoring grid as one `RPP` box — `voxel_box` material
regions become carved `RPP` cells and any `voxel_set` region puts the whole
grid into a `LAT=1` lattice with a per-material-universe `FILL` array (same
semantics as the OpenMC emitter) — plus `M` cards from the declared nuclide
mass fractions, the plane source as an `SDEF` card, `NPS` from the
requested histories, and `FMESH` neutron/photon flux tallies on the case
mesh. Cross-section tables come from `--xs-suffix` (recorded in the deck
header) or bare ZAIDs resolved by `xsdir` defaults — OpenBNCT never invents
a data library. Component-dose folding is deliberately absent from the
deck: folding flux into the four components applies the published response
set, which is the external pipeline's declared step before `import mcnp`
re-ingests the meshtal. A source plane outside the grid is rejected rather
than silently scoring zeros. Python parity is `export_mcnp_deck`. Deck
execution against real MCNP remains an open acceptance gate.

`openbnct import nifti` lifts per-component NIfTI dose volumes — the
shape OpenPINT writes (`{prefix}_{B10,N14,n,g}.nii.gz`), and the shape
any pipeline that emits one scalar volume per component produces:

```text
openbnct import nifti \
  --case-id my-case \
  --unit gray_per_source_particle \
  --normalization "OpenPINT B10/N14/n/g tallies resampled to CT grid" \
  --producer-system openpint \
  --producer-version 7d035fb \
  --component boron=BNCT_B10.nii.gz \
  --component nitrogen=BNCT_N14.nii.gz \
  --component hydrogen=BNCT_n.nii.gz \
  --component photon=BNCT_g.nii.gz \
  --output openpint-dose.json
```

All four components are required and must share one grid geometry
(world-frame disagreement is rejected, so a resample-to-CT OpenPINT set
is internally consistent by construction). `--component-sigma
NAME=FILE` optionally pairs each value volume with an absolute one-sigma
NIfTI on the same grid — matching OpenPINT's `get_dose_component_sigmas`
convention. NIfTI headers carry no producer identity, so
`--producer-system` is mandatory and what the files did state (datatype,
sform/qform choice) is recorded in the normalization string. Uncertainty
is `null` when no sigma volume is supplied — never invented.

### External-dose and combined-treatment evaluation

`openbnct import dose` ingests a `openbnct.external-dose/0.1.0` document —
one absolute absorbed-dose field (gray) plus the fractionation the course
was delivered in — the shape a photon or hadron course contributes to a
combined-treatment research evaluation:

```text
openbnct import dose \
  --file examples/interchange/photon-course-60gy.json \
  --output external-course.json
```

Fractionation is declared, never guessed: `{"kind": "uniform", "count": N}`
splits the total into equal per-fraction doses; `{"kind": "explicit",
"doses": [...]}` carries per-fraction dose arrays that must sum to the
declared total. The imported bundle's provenance binds the document hash
(`external-dose:<system>:sha256:<hash>`).

`openbnct bio bed` converts the course to a BED or EQD2 field under a
declared α/β (`BED = Σ_f d_f·(1 + d_f/r)`, `EQD2 = BED/(1 + 2/r)`), with
optional per-region α/β overrides driven by the same named-mask mechanism
as `bio apply`. `openbnct bio combine` then adds an external `eqd2` field
to a photon-isoeffective BNCT `weighted_eqd2` bundle — the only compatible
combination; a `bed` field, a non-fractionated primary, a mismatched case,
a differing grid without a declared `--resample trilinear`, or a missing
additivity assumption all reject rather than silently adding incompatible
quantities. The combined record (`openbnct.combined-dose/0.1.0`) binds both
input content hashes and provenance chains, records any resampling applied
and the operator's additivity assumption verbatim, and combines
independent-course sigmas in quadrature:

```text
openbnct bio bed --dose external-course.json --alpha-beta 3.0 \
  --output external-eqd2.json
openbnct bio combine \
  --primary biological-eqd2.json --external external-eqd2.json \
  --assumption "full-repair additive EQD2; independent courses" \
  --output combined-eqd2.json
```

Python parity is `import_external_dose`, `bed_from_external_dose`, and
`combine_biological_doses`. This is a research evaluation aid only — the
record states no clinical, equivalence, or commissioning claim.

### Cross-code dose comparison

`openbnct compare` measures voxelwise agreement between two physical dose
bundles on the same frozen case — for example an OpenMC-collected result
against an MCNP- or PHITS-imported one — and writes a
`openbnct.dose-comparison/0.1.0` record:

```text
openbnct compare \
  --reference openmc-dose.json --candidate mcnp-dose.json \
  --sigma-level 2 --output comparison.json
```

Both inputs must share `case_id`, an equivalent grid, the same component
set, and the same unit; anything else rejects — a frozen case is the only
valid comparison basis. Per component and `physical_total` the record
reports max/mean/RMS absolute difference, a normalized difference anchored
to the reference maximum (never a per-voxel ratio that explodes near zero),
and — when both sides state uncertainties — the fraction of voxels within
`sigma_level` combined sigmas. Both input content hashes and provenance
chains are bound into the record. It reports measured agreement only: no
equivalence, clinical, or commissioning verdict is implied. Python parity
is `compare_dose_bundles`.

### Gamma-index evaluation

`openbnct gamma` evaluates the Low et al. (1998) gamma index — the field's
standard dose-comparison metric — between two physical dose bundles on the
same frozen case, and writes an `openbnct.gamma-evaluation/0.1.0` record:

```text
openbnct gamma \
  --reference openmc-dose.json --candidate mcnp-dose.json \
  --dose-difference-percent 3 --distance-to-agreement-mm 3 \
  --dose-threshold-percent 10 --gamma-volume \
  --output gamma.json
```

For each evaluated reference voxel, gamma is the minimum over candidate
positions of `sqrt(Δr²/dta² + ΔD²/Δd²)`; a voxel passes when γ ≤ 1.
Pass/fail is computed exactly — only candidate voxels within `dta` can
pass, so the search is the dta-radius ball — while reported γ values above
1.0 are minima over that ball (an upper bound that can only make failing
voxels look worse, never better). `--normalization global` (default)
scales Δd by the reference maximum; `local` scales it by each reference
voxel's own value. `--dose-threshold-percent` applies the standard
low-dose cutoff, excluding reference voxels below that percent of the
reference maximum. `--gamma-volume` embeds the per-voxel γ field (grid
order, `null` for excluded voxels) for overlay and inspection. Per
component and `physical_total` the record reports evaluated/excluded
counts, pass rate, and mean/p95/max γ over finite values, with the same
frozen-case guards and hash-bound inputs as `openbnct compare`. It
reports measured agreement only: no equivalence, clinical, or
commissioning claim. Python parity is `evaluate_gamma`.

### Metamorphic transport oracles

`openbnct metamorphic` verifies a run against a symmetry *relation*
rather than a fixed reference — the complement to conformance fixtures
when no independent ground truth exists — and emits
`openbnct.metamorphic-evaluation/0.1.0` with all inputs content-bound:

```text
openbnct metamorphic --oracle reflection \
  --axis y --declared-symmetry "homogeneous slab, isotropic plane" \
  --reference dose.json --id EVAL-001 --output metamorphic.json

openbnct metamorphic --oracle rotation --axis z --turns 1 \
  --reference dose.json --candidate dose-rotated.json \
  --id EVAL-002 --output rotation.json

openbnct metamorphic --oracle superposition \
  --reference dose-ab.json --candidate dose-a.json \
  --candidate dose-b.json --id EVAL-003 --output superposition.json

openbnct metamorphic --oracle reciprocity \
  --voxel-a 1234 --voxel-b 5678 \
  --reference dose-detector-a.json --candidate dose-detector-b.json \
  --id EVAL-004 --output reciprocity.json
```

Each oracle reports per-quantity z-score statistics (fraction of voxel
pairs within `--sigma-level` combined σ, mean/max z, excluded counts):
`reflection` mirrors a bundle about a grid axis under an
operator-declared symmetry premise (recorded for review, not proven);
`rotation` permutes a source-rotated run back into the reference frame
by the same quarter-turn convention `rotate_source` applies, requiring
equal in-plane extents and spacings; `superposition` checks voxelwise
additivity of component-source runs with three-input combined σ; and
`reciprocity` pairs the dose at a declared voxel in each of two
source↔detector-interchanged runs. A naive energy-budget oracle is
deliberately absent: BNCT capture reactions are exoenergetic, so total
deposited energy is not a valid invariant. Passing oracles are
consistency evidence, not a correctness proof.

### Analytic oracles

`openbnct analytic` checks a dose bundle against a *closed-form*
expectation declared in an `openbnct.analytic-oracle/0.1.0` artifact —
the complement to metamorphic oracles when a genuine analytic ground
truth exists. The only law defined today is `exponential_attenuation`,
used by the `NF-BNCT-003` pure-absorber slab benchmark:

```text
openbnct analytic \
  --oracle benchmarks/synthetic/nf-bnct-003/transport/analytic-oracle.json \
  --dose dose.json --id EVAL-005 --output analytic.json
```

The oracle declares the quantity (e.g. `component:boron`), the profile
axis, the expected attenuation coefficient Σ_t in cm⁻¹, a relative
tolerance that must cover the law's stated approximation bound, a
world-coordinate fit window, and the content-bound material/source the
expectation assumes. The evaluation pools each perpendicular plane into
an axis profile, fits `ln D(z)` by weighted least squares over the
window (voxel uncertainties, when present, weight the fit and yield a
slope σ), and reports the fitted slope, its deviation from the declared
Σ_t, a log-residual RMS measuring how exponential the profile actually
is, and a pass/fail against the tolerance — emitted as
`openbnct.analytic-oracle-evaluation/0.1.0`. Axis-aligned grids only;
the oracle is consistency evidence, not a correctness proof.

### Deterministic multigroup transport (S_N)

`openbnct sn solve` is the in-house deterministic transport path — a
3-D Cartesian diamond-difference discrete-ordinates solver over the
transport case's regular grid, consuming a declared
`openbnct.multigroup-data/0.1.0` artifact (group structure plus
per-material total and scatter cross sections, optionally flux→dose
response vectors):

```text
openbnct sn solve \
  --case benchmarks/synthetic/nf-bnct-003/transport/case.json \
  --data benchmarks/synthetic/nf-bnct-003/transport/multigroup-data.json \
  --order 4 --convergence 1e-6 \
  --dose mg-dose.json \
  --output mg-flux.json
```

The emitted `openbnct.multigroup-flux/0.1.0` record carries the
content-bound case and data references, the quadrature order, iteration
counts, the final residual, and the beam model. `--dose` additionally
folds the flux through the data's declared `dose_response_gy_cm2`
vectors into a `PhysicalDoseBundle` — which the analytic oracle,
gamma-index, and metamorphic evaluators consume directly. Monodirectional
on-face disk sources use an uncollided-flux split by default: the
uncollided beam is ray-traced analytically (exact exponential, no
ordinate-obliquity bias) while the sweep solves only the collided
remainder — `--no-uncollided-split` exercises the pure boundary-flux
path. `--periodic x,y` marks faces periodic for infinite-slab problems;
other faces are vacuum. `--assignment` applies a material-assignment
heterogeneity. Nonconvergence within the iteration budget is a hard
error. Scope is honestly bounded: isotropic (P0) scattering, no fission,
multigroup data as declared input — a verification solver, not a
production engine, and its output is research-only.

### Exposure-plan tables and diagnostics

The `openbnct plan` family bridges spreadsheet workflows and the JSON
contract. `plan import` converts a `.csv` or `.xlsx` exposure table into a
validated plan (reporting every malformed row with its row number; blank
`dose_bundle_sha256` cells are filled by hashing files under
`--bundles-dir`), `plan export` writes the table back, and `plan validate`
lists every detectable issue in a plan document:

```text
openbnct plan import --table schedule.xlsx --output plan.json
openbnct plan export --plan plan.json --output schedule.csv
openbnct plan validate --plan plan.json
```

CSV tables carry `# format:`/`# id:`/`# case_id:`/`# covariance:` metadata
lines above the header row; XLSX workbooks carry a `plan` key/value sheet
plus an `exposures` sheet. The same Rust code serves the Python surface
(`load_exposure_plan`, `exposure_plan_diagnostics`, `accumulate_exposures`,
`plan_table_read`, `plan_table_write`) and the GUI's Plan workspace, which
displays the exposure table alongside every detected issue.

`plan optimize` closes the loop the exposure plan leaves open: given a
`openbnct.inverse-plan-objective/0.1.0` document — dose-volume
objectives (`min_eud`, `max_mean`, `min_dose_at_volume`,
`max_dose_at_volume`) on named region masks over `physical_total` or a
named component — it solves the non-negative weight assignment across
per-beam dose bundles by projected-gradient descent with analytic metric
gradients and an Armijo line search. A `weight_regularization` term
selects the minimum-total-weight feasible plan (feasibility alone is a
plateau). Each `--dose` bundle is one beam's unit-weight dose field,
named by file stem; each `--mask` is a `RegionMask` JSON
(`{"name", "voxels"}`) that every objective mask must resolve against.
The result is `openbnct.inverse-plan-result/0.1.0` — per-beam weights,
per-objective achieved/violation, iteration count, and convergence,
content-bound to the hashed objective document and qualified
`inverse_planning_research_only_not_clinical`:

```text
openbnct plan optimize \
  --objective OBJECTIVE.json \
  --dose BEAM-AP-DOSE.json --dose BEAM-PA-DOSE.json \
  --mask TUMOR-MASK.json --mask OAR-MASK.json \
  [--initial 1.0 --initial 1.0] \
  --output NEW-RESULT.json
```

The solver is deterministic — same inputs, byte-identical weights. It is
a research optimizer over linear dose superposition, not a commissioned
treatment-planning product; see `docs/IP_BOUNDARY.md` for the
optimization-adjacent scope boundary.

### Dose-volume metrics and endpoint response models

`openbnct metrics` computes exact dose-volume readings over a region mask
— `D_x` coverages, `V_x` levels, min/mean/max, and Niemierko generalized
EUD at requested organ parameters (`a = 1` mean, `a > 0` serial-leaning,
`a < 0` parallel-leaning, `a = 0` geometric mean; any zero-dose voxel
collapses a parallel EUD to zero) — emitting
`openbnct.dose-metrics/0.1.0`:

```text
openbnct metrics \
  --dose DOSE-BUNDLE.json \
  --quantity biological_total \
  --mask examples/biological/core-region-mask.json \
  --dx 98,50,2 --vx 50,60 --eud-a -10,1,10 \
  --output NEW-METRICS.json
```

`openbnct endpoint` scores separately versioned
`openbnct.endpoint-model/0.1.0` response models over a dose selection,
emitting `openbnct.endpoint-evaluation/0.1.0`. Three functions exist:
`voxel_poisson_tcp` (voxel-level LQ Poisson TCP — per-particle dose to
per-fraction `d`, BED, surviving clonogens; requires a
`*_per_source_particle` unit), `logistic` (`1/(1+(D50/D)^(4γ50))`), and
`probit` (Lyman `Φ((D−TD50)/(m·TD50))`) — the last two over a declared
scalar statistic (mean/min/max/EUD). `endpoint utcp` combines a TCP and an
NTCP evaluation over the same case/region/quantity/dose source under
`p_plus` (`TCP·(1−NTCP)`) or `difference`, rejecting mismatched
ingredients. Demonstration models live in `examples/endpoint/`; all
probabilities are synthetic research values:

```text
openbnct endpoint evaluate \
  --model examples/endpoint/logistic-tcp-model-v1.json \
  --dose DOSE-BUNDLE.json \
  --quantity biological_total \
  --mask examples/biological/core-region-mask.json \
  --output TCP-EVAL.json

openbnct endpoint utcp \
  --tcp TCP-EVAL.json --ntcp NTCP-EVAL.json \
  --combination p_plus --output UTCP-EVAL.json
```

### NIfTI imaging I/O

`openbnct nifti` provides a strict NIfTI-1 boundary alongside DICOM for
imaging-driven research workflows. The reader accepts single-file `.nii` and
gzip-compressed `.nii.gz` volumes: 3-D scalar data (`u8`, `i16`, `i32`, `f32`,
`f64`), sform preferred over qform, explicit millimeter units or the common
`xyzt_units == 0` "unspecified" convention (recorded as an assumed-mm
provenance note). NIfTI's RAS+ world frame is converted to OpenBNCT's
patient-LPS `GridGeometry` on import and back on export; the conversion and
transform source are recorded in provenance. Unsupported dimensions,
datatypes, transforms, endianness, and declared non-millimeter units are
rejected rather than approximated:

```text
openbnct nifti info --input image.nii.gz
openbnct nifti to-mask --input seg.nii.gz --name ROI --output NEW-MASK.json
openbnct nifti export-dose \
  --dose DOSE-BUNDLE.json --quantity component:boron \
  --output NEW-BORON-DOSE.nii.gz
openbnct nifti export-components \
  --dose DOSE-BUNDLE.json --output-dir NEW-DIR [--gzip] [--pint]
openbnct nifti resample \
  --input map.nii.gz --target DOSE-BUNDLE.json \
  --interpolation nearest --output NEW-RESAMPLED.nii.gz
openbnct nifti resample \
  --input map.nii.gz --target CASE.json \
  --interpolation trilinear --output NEW-CASE-ALIGNED.nii.gz
```

`export-dose` writes any component or the physical total as a NIfTI image on
the bundle's grid; `export-components` writes all four components at once —
`<case>.<component>.nii` plus `.<component>.sigma.nii` companions when the
bundle carries per-voxel uncertainties — alongside an
`openbnct.component-nifti-manifest/0.1.0` record hash-binding every written
file, so the set re-imports through `import nifti` with component, value,
sigma, and grid fidelity. `--pint` switches the component filenames to the
fixed OpenPINT convention (`<case>_B10`, `_N14`, `_n`, `_g`; sigma companions
keep the `.<component>.sigma` form) for direct hand-off to PINT-workflow
consumers — the manifest and contents are unchanged. `resample` interpolates an external image onto a
dose bundle's grid or a transport case's CT-aligned grid (nearest-neighbor
or trilinear). The affine handling is
regression-tested against independent `nibabel` output including oblique
sforms, and `.nii.gz` round-trips are verified in both directions.

### Rigid registration

`openbnct register` records rigid co-registrations between image volumes
(e.g. CT↔PET for boron mapping) as versioned
`openbnct.registration/0.1.0` documents. The transform maps moving-image
patient coordinates (LPS mm) onto the fixed image's frame; the document
optionally content-binds both source images by SHA-256. Two construction
paths exist:

- `landmarks` fits the closed-form least-squares rigid transform (Horn's
  quaternion method) over three or more non-degenerate landmark pairs —
  a JSON array of `moving_lps_mm`/`fixed_lps_mm` points — and stores the
  pairs plus the RMS residual as evidence.
- `declare` records an operator-transcribed transform (e.g. a matrix
  exported from an external registration tool). It requires a provenance
  note stating where the numbers came from; no landmark evidence is
  fabricated for declared transforms.

```text
openbnct register landmarks \
  --pairs LANDMARKS.json --id REG-001 \
  --moving PET.nii.gz --fixed CT-STACK.nii.gz \
  --note "fiducial + anatomy picks, OPERATOR, DATE" \
  --output NEW-REGISTRATION.json
openbnct register declare \
  --id REG-002 --rotation "1,0,0,0,1,0,0,0,1" \
  --translation-mm "0,0,0" \
  --note "identity: PET and CT acquired in one session" \
  --output NEW-REGISTRATION.json
openbnct register info --registration REGISTRATION.json
openbnct register apply \
  --moving PET.nii.gz --registration REGISTRATION.json \
  --target-grid DOSE-BUNDLE.json \
  --interpolation trilinear --output NEW-PET-ON-GRID.nii.gz
```

`apply` moves the moving volume's frame through the transform and
resamples onto a transport-case or dose-bundle grid; the output records
the registration id and method in its NIfTI description. Validation
rejects non-orthonormal rotations, reflections, fewer than three or
degenerate (coincident/collinear) landmark sets, and declared
registrations without a provenance note. Registration is a research
interoperability feature; it makes no clinical assertion about alignment
quality beyond the recorded landmark residual.

### PET-derived boron fields

The SUV input can come straight off the scanner: `openbnct dicom
import-pet` converts a native single-frame PET series into a body-weight
SUV volume on the same validated geometry path as the CT importer. The
series must carry `Units = BQML`, `Decay Correction = START` (the tag
may be absent on older exports; any other value is rejected), a positive
Patient's Weight, and a complete Radiopharmaceutical Information
Sequence — total dose (Bq), radionuclide half-life (s), and start
time — plus an Acquisition or Series Time to decay the injected dose
to. Pixels may be signed or unsigned 16-bit; rescaled activity
concentrations below zero are clamped and counted in the record rather
than hidden. Output is a float64 NIfTI (`.nii.gz` compresses
automatically):

```text
openbnct dicom import-pet \
  --slices PET-001.dcm PET-002.dcm PET-003.dcm ... \
  --output NEW-PET-SUV.nii.gz
```

`openbnct boron` maps a co-registered PET SUV volume to a per-voxel B-10
concentration field (`openbnct.boron-field/0.1.0`, µg/g) under a versioned
`openbnct.boron-uptake-model/0.1.0`. Three mappings are supported:

- `suv_ratio` — the tumor:blood-ratio method:
  `B10(v) = η · R_ref · SUV(v)/SUV_ref`, where `R_ref` is a measured
  reference concentration (classically a blood sample at irradiation
  time) and `η` is the assay's B-10 fraction (1.0 for a B-10 assay,
  ~0.99 for enriched-BPA total-boron assays).
- `linear_suv` — a calibrated regression `B10(v) = a·SUV(v) + b`.
- `uniform` — a declared constant loading (the assumed-uptake baseline);
  consumes no SUV volume.

All mapping parameters carry 1σ uncertainties propagated to per-voxel
field σ (first order, parameters treated as independent); an optional
per-voxel `suv_noise_1sigma` and an optional single-rate exponential
`time_correction` (washout `2^(−Δt/T½)` with half-life σ) are included.
The model's `validity_domain` is mandatory free text stating the imaging
protocol and population assumptions. Negative mapped values clamp to
zero and the clamped count is recorded in the field.

```text
openbnct boron info --model examples/boron/suv-ratio-model-v1.json
openbnct boron apply \
  --model MODEL.json --case CASE.json \
  --suv PET-SUV.nii.gz [--registration REGISTRATION.json] \
  --id FIELD-001 --output NEW-FIELD.json \
  [--nifti-output CONC.nii.gz]
openbnct boron materialize \
  --field FIELD.json --case CASE.json --tiers 8 \
  --output NEW-ASSIGNMENT.json
```

`apply` requires the case the field binds (grid + `case_id`); when
`--registration` is given the SUV volume is first moved through that
transform, then resampled onto the case grid — SUV voxels outside the
imaged field of view contribute zero concentration. `materialize` bins
the field into linearly-spaced concentration tiers realized as voxel-set
`MaterialRegion`s, each carrying the tier-center B10 mass fraction
(other nuclides renormalized); the resulting
`openbnct.material-assignment` feeds `openmc generate --assignment`.
Tier count trades geometric fidelity for lattice cost — every tier is a
distinct material in the deck. The emitted field asserts
`pet_derived_boron_research_only_not_clinical`: it is a modeled estimate
with propagated parameter uncertainty, not an assayed measurement.

`openbnct boron microdistribution` evaluates a ¹⁰B subcellular
microdistribution model (`openbnct.boron-microdistribution/0.1.0`) into
a correction record (`openbnct.microdistribution-correction/0.1.0`):

```text
openbnct boron microdistribution \
  --model microdistribution.json \
  --id openbnct.microcorr.bpa.v1 --output correction.json
```

The model declares how ¹⁰B partitions across nucleus, cytoplasm, cell
membrane, and extracellular space (fractions with 1σ, summing to 1), the
concentric-sphere cell geometry (cell/nucleus radii, extracellular
extent), the adopted α/⁷Li energies and CSDA ranges, and the
cell-to-cell uptake coefficient of variation. Evaluation integrates —
by deterministic quadrature, no RNG — the fraction of capture-reaction
charged-particle energy reaching the nucleus from each compartment
under straight-line constant-LET tracks, and reports the *nucleus-dose
factor*: the multiplier on the boron dose component under the declared
microdistribution relative to the conventional uniform-concentration
assumption. That factor is the quantity photon-isoeffective
evaluations (compound-effectiveness weights) may condition on; the
record also carries the absolute per-compartment fractions, the uniform
reference, a linearly-propagated factor 1σ, and the cell-to-cell dose
CV. It states explicitly that the heterogeneity nonlinearity
materializes in downstream survival evaluation, and asserts
`first_order_geometric_microdosimetry_research_only_not_clinical`.

### Systematic uncertainty

`openbnct uq` propagates *declared* systematic uncertainties over a
physical dose bundle into a `openbnct.systematic-uncertainty/0.1.0`
report. Sources:

- `--boron-field FIELD.json` — a PET-derived boron field's fractional
  per-voxel σ scales the boron dose component
  (`σ(v) = D_b(v)·σ_B(v)/B(v)`); this is the dominant BNCT systematic.
- `--relative component=sigma` — a declared relative 1σ on a named
  component (response calibration, model parameter).
- `--positioning-sigma-mm X` — translational positioning σ contributing
  `|∇D(v)|·X` per voxel (first-order dose-shift under displacement).
  `--positioning-registration REG.json` binds a registration document
  as provenance and defaults σ to its RMS landmark residual.

Per voxel, independent sources combine in quadrature and then with the
bundle's Monte Carlo σ into `combined_1sigma`. For region means
(`--mask NAME=path`), the correlation model is explicit: MC σ is
voxelwise-independent (`sqrt(Σσ²)/N`) while each systematic source
contributes its mean per-voxel σ fully correlated across voxels —
systematics do not average down. The dose bundle's own σ remains pure
Monte Carlo; the report is a separate, additive layer.

```text
openbnct uq apply \
  --dose BUNDLE.json \
  --boron-field FIELD.json \
  --relative photon=0.05 \
  --positioning-registration REGISTRATION.json \
  --mask TARGET=mask.json \
  --id UQ-001 --output NEW-UQ-REPORT.json
openbnct uq info --report UQ-REPORT.json
```

`openbnct uq propagate` propagates a *declared nuclear-data covariance*
through the deterministic S_N solve into a
`openbnct.dose-uncertainty-budget/0.1.0` report on one dose component's
integrated response. The `openbnct.multigroup-covariance/0.1.0`
artifact declares per-material relative standard deviations over
`sigma_total`, `scatter` (group-to-group transfer entries), and
`dose_response` vectors — independent `diagonal` entries plus dense
group-correlation `blocks` for `sigma_total`/`dose_response`. The
nominal multigroup data stays untouched; every uncertainty lives in
the covariance artifact.

Sensitivities are central finite differences of the actual discrete
solve — two perturbed solves per declared parameter — because the
positivity-clamped diamond-difference operator is nonlinear and a
continuous-adjoint inner product is only first-order-faithful to it
(measured ~25–40% pointwise on coarse meshes; adjoint importance
remains the right machinery for CADIS ratios). Response entries are
linear in the integral and exact analytically. Propagation is the
quadratic form σ²(R) = Sᵀ·C·S; `--statistical-rel-std` folds a Monte
Carlo σ on the same integral in as an independent variance. Entries
report each source's response-space σ and variance share; the nominal
forward flux can be emitted (`--flux`) and is content-bound into the
budget.

```text
openbnct uq propagate \
  --case transport/case.json \
  --data transport/multigroup-data.json \
  --covariance transport/multigroup-covariance.json \
  --component boron --periodic x,y \
  --statistical-rel-std 0.02 \
  --id openbnct.case.budget.v1 --output NEW-BUDGET.json \
  --flux NEW-FLUX
openbnct uq budget-info --budget BUDGET.json
```

`openbnct uq screen` runs a `openbnct.sensitivity-spec/0.1.0` design —
global sensitivity screening over *declared* input ranges, answering
which inputs deserve a covariance at all. Each parameter is an absolute
range on a named target: `material_sigma_total_scale`,
`material_scatter_scale`, `material_response_scale` (multiplicative,
nominal 1.0), `source_center_shift_mm`, `source_radius_scale`, and
`geometry_origin_shift_mm` (beam-positioning and phantom-placement
tolerances, millimetres). Every design point is a full deterministic
S_N solve folded to one component's integrated response.

`method: "morris"` runs `samples` elementary-effects trajectories over
a `levels`-point grid — `r·(k+1)` solves — reporting μ (mean signed
effect), μ* (ranking statistic), and σ (nonlinearity/interaction flag).
`method: "sobol"` runs the Saltelli design — `N·(2k+2)` solves — with
Jansen total-order (ST) and centered Saltelli-2010 first-order (S1)
estimators; output is mean-centered before estimation so the product
term doesn't suffer the usual m²-cancellation noise. Results are
bit-reproducible under `seed`. Reports bind the spec, case, and data
hashes; a committed example spec ships with NF-BNCT-003.

```text
openbnct uq screen \
  --case transport/case.json \
  --data transport/multigroup-data.json \
  --spec transport/sensitivity-spec.json \
  --periodic x,y \
  --id openbnct.case.screening.v1 --output NEW-SCREENING.json
openbnct uq screening-info --report SCREENING.json
```

A committed demonstration covariance ships with NF-BNCT-003
(`transport/multigroup-covariance.json`, content-bound to the case's
multigroup data). Declared covariances are inputs, not evaluations —
the artifact's `provenance_note` must say where the numbers came from.

### Variance reduction

`openbnct vr` manages weight-window variance reduction for the OpenMC
path through two versioned documents. A `openbnct.variance-reduction/0.1.0`
spec declares, per particle, a regular mesh (cm, world frame), optional
energy groups, splitting/roulette knobs (`survival_ratio`, `max_split`,
`weight_cutoff`), and how the bounds are supplied: `uniform` (one lower
bound everywhere), `explicit` (concrete row-major `(energy, mesh)`
arrays), `forward_flux` (derived from a completed run's mesh flux
tally by the MAGIC-equivalent rule `lower = flux/(2 × group_max)` with
noisy cells disabled), or `adjoint` (derived by the in-house S_N
adjoint solve — CADIS/FW-CADIS).

`vr resolve` turns the spec into a `openbnct.weight-windows/0.1.0`
artifact carrying the concrete bounds and a content-bound provenance
chain — the spec hash and, for flux-derived windows, the generating
statepoint hash:

```text
openbnct vr resolve \
  --spec SPEC.json --run ANALOG-RUN-DIR \
  --id WW-001 --output NEW-WW.json
openbnct vr info --document WW.json
```

The resolved artifact is consumed at deck generation or run time —
`openmc generate|run --vr WW.json` declares each window mesh inside
`settings.xml` (OpenMC parses it before the `<weight_windows>` entries
that reference it) and binds the artifact in the input manifest. The
benchmark spec
`variance-reduction/nf-bnct-001-ww-v1.json` derives neutron and photon
windows from the case's diagnostic fluence tallies on the scoring mesh.

`vr cadis` resolves `adjoint` bounds without any Monte Carlo run: the
in-house S_N solver computes the adjoint (importance) field for the
declared response — `dose_component` folds a named response vector
through the material at each voxel, `voxel_box`/`global` declare a
region response — and sets window targets `w₀ = w_ref/φ†` normalized so
declared-source particles are born at unit weight on their local
target. `fw_cadis` additionally takes `--forward-flux` (a
`openbnct.multigroup-flux` artifact, e.g. from `sn solve`) and builds
`q† = response/φ_fwd`, flattening the population for mesh-global
tallies. Cells with zero importance become kill windows at
`target_cap`× the source weight. The resolved artifact content-binds
the spec, case, multigroup data, forward flux, and each adjoint solve:

```text
openbnct vr cadis \
  --spec VR-CADIS.json \
  --case benchmarks/synthetic/nf-bnct-003/transport/case.json \
  --data benchmarks/synthetic/nf-bnct-003/transport/multigroup-data.json \
  --order 4 --periodic x,y \
  --id openbnct.nf-bnct-003.ww.cadis.v1 \
  --output NEW-WW.json --adjoint-flux NEW-ADJ
```

The window mesh must coincide with the case grid and its energy bounds
must be the multigroup structure in ascending order — the derivation
never re-bins across the solve.

`vr validate` evaluates a completed variance-reduced run through the
ordinary acceptance machinery and then checks it for *unbiasedness*
against an analog acceptance report: every shared region/tally mean must
agree within `z_limit` combined sigma of *each* reference seed's result,
and the run must have used fewer histories. The emitted
`openbnct.vr-validation/0.1.0` report records the achieved reduction
factor rather than assuming it:

```text
openbnct vr validate \
  --vr-run VR-RUN-DIR --exit-code 0 \
  --reference-report openmc-acceptance-report-600M.json \
  --reference-histories 600000000 \
  --z-limit 3.0 --output NEW-VR-VALIDATION.json
```

Weight windows change statistical efficiency only — estimator semantics
are untouched — and these artifacts are research-verification machinery,
not clinical commissioning evidence.

`openbnct mask` combines and constructs `RegionMask` volumes for
limiting-organ construction: subtraction (e.g. organ minus tumor), union,
and intersection across mask JSONs, plus CT-threshold regions built from a
DICOM series' rescaled modality (HU) window. All masks must share one voxel
count, and operations that would select nothing are rejected:

```text
openbnct mask subtract --input ORGAN.json --minus TUMOR.json \
  --name ORGAN-MINUS-T --output NEW-MASK.json
openbnct mask union --inputs A.json --inputs B.json --name U --output M.json
openbnct mask intersect --inputs A.json --inputs B.json --name I --output M.json
openbnct mask threshold --ct-dir CT-DIR --min -10 --max 40 \
  --name SOFT-TISSUE --output NEW-MASK.json
```

Resulting masks feed `benchmark derive-materials --mask`, `openbnct dvh`,
`openbnct bio apply --region-mask`, and `openbnct irradiation-time`.

`openbnct irradiation-time` evaluates organ-limited irradiation time over a
per-source-particle endpoint — a physical component/total or a biological
weighted total — under declared `max`/`mean` region limits, emitting a
`openbnct.irradiation-time-report/0.1.0` that names the limiting structure,
each region's admissible time and particle budget, and the assumptions
(linear accumulation at constant source strength, static anatomy, no
inter-fraction recovery):

```text
openbnct irradiation-time \
  --dose DOSE-BUNDLE.json --quantity physical_total \
  --source-strength 1e9 \
  --limit CORD=max:12.5 --limit SKIN=mean:5.0 \
  --mask CORD=cord.json --mask SKIN=skin.json \
  --output NEW-REPORT.json
```

Endpoints in absolute units (not per-source-particle), non-positive source
strengths, limits without a same-named mask, and zero-statistic regions
are reported explicitly — the last as unbounded rather than an error.

`openbnct position` provides research positioning helpers. `position aim`
derives a fixed source whose beam axis passes through a region mask's
centroid: the source plane is placed just inside the bounding-box face the
beam enters, its aperture centered on the beam axis, for any axis approach
(`+x`…`-z`) or an oblique `--direction dx,dy,dz`. `position rotate` applies
a right-hand-rule quarter-turn about a patient axis, remapping the source
plane, aperture, and direction; rotations that would require a
non-axis-aligned aperture are rejected. `aim` emits the source JSON plus a
`openbnct.position-report/0.1.0` recording the centroid, entry face and
point, and source-to-centroid distance:

```text
openbnct position aim \
  --case transport/case.json --source transport/source.json \
  --mask CORE.json --approach +z --half-widths-cm 2.0,2.0 \
  --output-source NEW-SOURCE.json --output-report NEW-REPORT.json
openbnct position rotate --source NEW-SOURCE.json --axis y --degrees 90 \
  --output-source NEW-ROTATED.json
```

`openbnct dvh` computes a deterministic `openbnct.dose-volume-histogram/0.1.0`
over a named voxel mask for any component or total in a physical or
biological bundle — equal-width dose bins, differential volume fractions that
sum to one, and a cumulative `V(d)` curve:

```text
openbnct dvh \
  --dose DOSE-BUNDLE.json --quantity component:boron \
  --mask examples/biological/core-region-mask.json \
  --bins 100 --output NEW-DVH.json
```

### One-command runs and evidence bundles

`openmc run` chains `prepare` (deterministic deck), `execute` (run receipt),
and `collect` (normalized dose bundle) in one invocation; `--evidence-root`
additionally exports a `openbnct.evidence-bundle-manifest/0.1.0` directory
that binds every input, deck file, log, statepoint, and the dose bundle by
SHA-256 under a declared qualification boundary:

```text
openbnct openmc run \
  --case transport/case.json --component-profile transport/component-profile.json \
  --material transport/material.json --source transport/source.json \
  --response-set transport/provenance/neutron-response-set.json \
  --nuclear-data-manifest transport/provenance/openmc-endfb81-processed-data-manifest.json \
  --execution-profile transport/openmc-smoke-profile.json \
  --nuclear-data-root DATA-ROOT --openmc /path/to/openmc \
  --env LD_LIBRARY_PATH=/path/to/libopenmc \
  --env OPENMC_CROSS_SECTIONS=DATA-ROOT/cross_sections.xml \
  --working-directory NEW-RUN-DIR --dose-output NEW-DOSE.json \
  --evidence-root NEW-BUNDLE-DIR

openbnct evidence verify --root BUNDLE-DIR
```

`evidence export`/`verify` are also available standalone for assembling
arbitrary hash-bound artifact sets. The exported layout follows the
evidence-bundle list predeclared in the NF-BNCT-001 specification.

A controlled ENDF/B-VIII.1 + TENDL-2025 mixed-source candidate was then
executed under selection schema `0.3.0`. The six shared nuclides reproduce the
baseline exactly, but all four TENDL-2025 substitutions remain rejected with
77 kinematic violations, and TENDL's duplicated File 3 grid points leave the
deeper independent gates uncomputable — a preserved source-format finding. All
three published evaluated libraries are now rejected under the controlled
chain, so the response path resumes only with a reviewed independent O-17
calculation. See
[ADR 0028](../docs/adr/0028-mixed-evaluated-neutron-source-selections.md) and
[ADR 0029](../docs/adr/0029-tendl2025-mixed-source-candidate.md).

The derived diagnostic-triage gate is also a live dogfood case for Avila Core.
OpenBNCT keeps the domain verification and emits a deterministic machine
result; Core binds the exact executable and inputs, records the run, evaluates
the 43-finding queue, and independently enforces the closed response category.
A second integration binds the candidate-comparison check so a rejected
candidate is a verified result rather than a process failure. See the
[integration case](../integrations/avila-core/njoy-evidence-aware/README.md), the
[candidate-comparison case](../integrations/avila-core/njoy-candidate-comparison/README.md),
and [ADR 0024](../docs/adr/0024-avila-core-evidence-loop.md).

