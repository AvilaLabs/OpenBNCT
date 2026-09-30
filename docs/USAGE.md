# Usage reference

Detailed command and workflow reference for the OpenBNCT CLI, GUI, and Python
surfaces. For project status see [ROADMAP.md](ROADMAP.md); for the research
boundary see [DISCLAIMER.md](DISCLAIMER.md).

## Quick start: project

From a head CT with an RT Structure Set to component dose, boron-scaled dose,
DVHs and a report in two commands, with no hand-written JSON. The example uses
the synthetic NF-BNCT-001 study (research demonstration only):

```text
openbnct benchmark generate ./study
openbnct project init --dicom ./study --output ./p001 --target CORE --spacing-mm 8
openbnct project run ./p001
```

`project init` reads the study, lists the detected ROI names in a comment at
the top of `p001/project.toml`, and copies every built-in artifact the project
uses into `p001/inputs/` (with `SHA256SUMS`), so the project is self-contained
and reproducible from the binary alone. It refuses to overwrite an existing
directory; without `--target` the `[beam] target` line is a commented
placeholder you must fill in.

`project run` executes seven steps (an eighth, `verify`, is separate: see
[Independent Monte Carlo check](#independent-monte-carlo-check-project-verify)), each the same code as the individual
command it names: `import` (`dicom import-ct`), `calibrate` (`dicom
calibrate`), `beam` (`beam bind --aim-mask`), `transport` (`sn solve --dose
--boron-unit-output --source-weighting uniform_in_bin`, then, with
`photon_transport = true`, `sn photon-solve --dose` and `sn merge-photon-dose`),
`boron` (`boron dose`), `metrics` (`metrics` and `dvh`
per structure) and `report`. Artifacts land in `p001/out/01-import` ...
`06-metrics`; the report is `out/report.md` and `out/report.json`
(`openbnct.project-report/0.1.0`) with DVH curves in `out/dvh/*.csv`.
Everything underneath stays the existing hash-bound artifacts; the runner only
orchestrates.

```toml
[project]
id = "mylab.p001"                          # case id

[imaging]
dicom = "../study"                         # one CT series (+ RTSTRUCT), relative to the project dir
spacing_mm = 5.0

[materials]                                # builtin:NAME or a path; `openbnct project builtins` lists names
calibration = "builtin:tissue/hu-calibration-generic-head-ct"
multigroup_data = "builtin:tissue/multigroup-data-28g-tsl"
photon_data = "builtin:tissue/multigroup-photon-data-16g"   # coupled photon data, bound to the 28-group neutron structure
base_material = "builtin:tissue/material-air-dry"

[beam]
description = "builtin:beams/fir1-k63-ineel"   # 120-bin FiR 1 spectrum; builtin:beams/fir1-k63 is the 3-bin version; or a path to an openbnct.beam-description
target = "GTV"                             # RTSTRUCT ROI whose centroid the beam axis passes through
approach = "+x"                            # +x -x +y -y +z -z

[transport]
engine = "sn"
order = 8
max_outer = 128
anderson = 3                               # Anderson depth (passed to sn solve --anderson)
source_weighting = "uniform_in_bin"        # histogram beam bins uniform per eV (OpenMC/MCNP convention); or "collapse_consistent" (1/E above 0.5 eV)
photon_transport = true                    # transport capture photons; false = deposit their energy where it is born
allow_unconverged = false                  # demos/tests only; the report is then marked PROVISIONAL if it did not converge

[boron]
blood_ug_g = 25.0
ratios = { GTV = 3.5, SKIN = 1.5 }         # tissue:blood per ROI; smaller ROI wins where they overlap
default_ratio = 1.0                        # voxels in no listed ROI

[report]
structures = []                            # empty = every ROI
# source_strength_per_s = 1.0e12           # both, or neither: scales per-source-particle
# irradiation_time_s = 1800                # dose to Gy
```

The generic HU table and 28-group data are demonstration inputs (see
[Standard tissue library](#standard-tissue-library)); the boron
concentrations above are illustrative placeholders. The report header carries
the project id, OpenBNCT version, timestamp, `converged` or `PROVISIONAL`,
and the sha256 of every input; each structure gets mean, max, D95, D50 and D2
for the boron, nitrogen, hydrogen and photon components and the physical total
(per source particle, or Gy with delivery normalization); a "Commands"
section lists the exact command line of every step, run from the
project directory, so any step can be reproduced or modified by hand.

Runs are resumable. `out/run-manifest.json` (`openbnct.project-run/0.1.0`)
records each step's input sha256s, command, options, outputs and their
sha256s. A rerun skips a step whose inputs, command and outputs still match,
so editing `[boron] blood_ug_g` reruns only `boron`, `metrics` and `report`.
`openbnct project run ./p001 --force` reruns everything and `--from
transport` reruns that step and all later ones. `openbnct project status
./p001` prints the step table. While `transport` runs, one line per outer
iteration (`[sn] outer k/max: residual r (t s)`) goes to stderr; `sn solve`
prints these by default and `--quiet` turns them off. Research software;
nothing here is a clinical calculation.

### Independent Monte Carlo check: `project verify`

The deterministic answer is fast; `project verify` is the one-command
independent check of it. After a completed `project run`:

```text
export OPENBNCT_OPENMC=$HOME/micromamba/envs/openmc016/bin/openmc
export OPENMC_CROSS_SECTIONS=/path/to/endfb-viii.1-hdf5/cross_sections.xml
openbnct project verify p001 --particles 1e6
```

It re-computes the same case, material assignment, source and boron
concentrations with continuous-energy OpenMC 0.16.0 on ENDF/B-VIII.1 (the
unit-mass-fraction multi-material path below, including the H-in-H2O
S(α,β) table that the default 28-group data carries on hydrogen — the MC
side declares `H1=c_H_in_H2O` whenever the deterministic data does, and the
report states whether the two sides use the same scattering physics). The
library at `OPENMC_CROSS_SECTIONS` must be the full HDF5 distribution
including its `thermal/` directory. Flags: `--particles N` (default 1e6, `1e6`
notation accepted), `--batches B` (10), `--threads T` (2), `--openmc PATH`,
`--cross-sections PATH`, `--timeout-seconds S` (14400; the OpenMC process is
killed and reaped on expiry), `--force`. Flags override the optional
`[verify]` table of `project.toml`, which overrides the environment:

```toml
[verify]                          # every key optional
particles = 1e6
batches = 10
threads = 2
ratio_tolerance = 0.05            # structure-mean total-dose ratio S_N/MC within 1 +- 5 %
gamma_pass_min = 0.95             # gamma pass rate per structure
gamma_dose_percent = 3.0          # gamma criteria, global normalization
gamma_dta_mm = 3.0
gamma_cutoff_percent = 10.0       # reference voxels below this % of the maximum are excluded
# thermal_scattering = ["H1=c_H_in_H2O"]   # default: taken from the deterministic data
# structures = []                 # verdict structures (default: the report structures)
```

Step `08-verify` writes `out/08-verify/`: the OpenMC deck, run receipt and
statepoint (`openmc-run/`), the MC dose bundle `mc-dose.json` (post-hoc
boron applied with step 05's exact concentration spec), `comparison.json`
(`openbnct compare`), `gamma.json` (`openbnct gamma`, MC as reference, per-voxel
volume included), `verification.json`, and `ratio-boron.nii`,
`ratio-nitrogen.nii`, `ratio-hydrogen.nii`, `ratio-photon.nii`,
`ratio-total.nii` (S_N/MC per voxel; undefined where the MC value is below 1 %
of its maximum). It is recorded in the run manifest like the other steps
(commands, options, input and output hashes), skipped when unchanged, and
`report.md` / `report.json` gain an "Independent Monte Carlo check" section:
MC statistical uncertainty, the per-structure S_N/MC mean-dose ratio for
each component and the total, the gamma pass rate per structure, and a verdict
line that states its thresholds — `AGREES` (every evaluated structure-mean
total-dose ratio within the tolerance and every structure's gamma pass rate at
or above the minimum), `DISAGREES`, or `INCONCLUSIVE` when the MC mean of a
failing structure is too uncertain (2σ above the tolerance) to test it. A
verification whose inputs later change is shown as stale and not reported. The
built-in OpenMC inputs it uses (`project builtins` lists them) are the NF-BNCT-001
response set and unit-mass-fraction profile; the comparison is between two
transport methods on one research case and states no clinical claim.

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
0014](adr/0014-evidence-aware-workbench-shell.md).

`pip install openbnct` is the primary distribution path for scientific
users, backed by the same Rust implementation through PyO3 and maturin. The
first bounded API is implemented under `bindings/python` and exercised by a
cross-language parity suite. Release wheels (Linux x86_64/aarch64, macOS
x86_64/arm64, Windows x86_64, plus sdist) publish through OIDC trusted
publishing — TestPyPI on manual dispatch, PyPI on `v*` tags; all 15
`openbnct*` crates are on crates.io (`cargo install openbnct-cli`). Cargo
remains the native source/developer path, while desktop releases will ship as
native artifacts. See [ADR
0015](adr/0015-python-and-native-distribution.md) and [ADR
0027](adr/0027-first-bounded-python-api.md).

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
[ADR 0010](adr/0010-verifiable-nuclear-data-acquisition.md).

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
0011](adr/0011-deterministic-njoy-input-preparation.md).

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
result. See [ADR 0012](adr/0012-controlled-njoy-execution-evidence.md) and
the [structured finding summary](research/NJOY2016_78_KINEMATIC_FINDINGS.md).

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
0013](adr/0013-transported-photon-kerma-suitability.md).

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
0017](adr/0017-source-aware-photon-production-suitability.md), [ADR
0019](adr/0019-independent-mf6-capture-photon-balance.md), and [ADR
0020](adr/0020-content-bound-transport-domain-suitability.md), followed by
[ADR 0021](adr/0021-independent-law7-implicit-residual-balance.md).
The processor attribution is frozen in [ADR
0022](adr/0022-law7-processor-attribution.md), and the integrated decision
is specified by [ADR
0023](adr/0023-reaction-evidence-aware-suitability.md), and the bounded
work queue by [ADR
0025](adr/0025-diagnostic-triage-of-remaining-njoy-findings.md), and the
O-17 attribution and response-path pause by [ADR
0026](adr/0026-o17-processor-energy-balance-attribution.md); that pause is
superseded by [ADR
0031](adr/0031-o17-diagnostic-queue-dispositioned.md), under which the
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

`bind` can also aim the beam: `--aim-mask MASK.json --approach=+x` (a
RegionMask, and one of `+x -x +y -y +z -z`) re-centers the circular port as
a disk on the entry face where the beam axis through the mask centroid meets
it (position from `aim_disk_source_at_centroid`, as `plan fields` uses),
keeping the declared radius. The angular distribution is preserved: an
`isotropic_cone` source keeps its half-angle and only its axis is re-pointed,
and a monodirectional source stays monodirectional. The disk must fit inside the face or the bind is
rejected. `openbnct project run` uses this to aim at the target structure.

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

### CT series to transport case

`openbnct dicom import-ct` builds the transport-grid inputs from a CT series
(and optional RT Structure Set): a scaffold `openbnct.transport-case` with a
**placeholder** source, the HU volume on the case grid, per-ROI masks, and a
`*.import-record.json` hash-binding all inputs and outputs. HU is
volume-averaged (overlap-weighted box mean) onto the coarser grid; ROI
membership is at least 50 % volume coverage by contour-interior CT voxels.
The grid is built in the CT's own patient frame; nothing is reoriented.

```text
openbnct dicom import-ct --series DICOM-DIR \
  --spacing-mm 5 --case-id mylab.patient.v1 \
  --base-material void.json \
  --case-output NEW-CASE.json --hu-output NEW-HU.nii \
  [--rtstruct RTSTRUCT.dcm] [--masks-dir NEW-MASKS-DIR]
```

Feed the output to the calibration below with `--hu-nifti NEW-HU.nii --case
NEW-CASE.json`, then bind a real beam (`beam bind`).

### HU-to-material calibration

`openbnct dicom calibrate` closes the imaging→phantom hop: it applies an
`openbnct.hu-calibration/0.1.0` anchor table to a CT HU volume and emits
an `openbnct.material-assignment`. Each anchor declares a
`MaterialDefinition` at a Hounsfield value; a voxel between anchors
becomes the two-component volume mixture `(1−t)·A_i + t·A_{i+1}` — the
stoichiometric-calibration structure (Schneider, Bortfeld & Schlegel,
*Phys. Med. Biol.* 45 (2000) 459) parameterized by the declared anchors
rather than a hard-coded fit, because the calibration is scanner- and
protocol-dependent in real use. The artifact's content hash binds the
exact table used.

```text
openbnct dicom calibrate \
  --calibration hu-calibration.json \
  --slices CT-001.dcm CT-002.dcm ... \
  --case case.json \
  --output NEW-ASSIGNMENT.json \
  [--materials-out-dir NEW-MATERIALS-DIR] \
  [--report NEW-REPORT.json]
```

`--hu-nifti VOLUME.nii[.gz]` replaces `--slices` for an HU volume
already resliced onto the case grid (3D Slicer, plastimatch); the grid
shape must match the case's — `calibrate` refuses a mismatch rather
than silently resampling. `--materials-out-dir` writes one
`MaterialDefinition` per anchor for `sn collapse --material`, and
`--report` records per-anchor coverage, interpolated/clamped voxel
counts, and the observed HU range.

The committed example
`benchmarks/synthetic/layered-head-phantom/materials/hu-calibration.json`
is a **declared demonstration table** (ICRU-44 void/brain/skin/skull at
conventional head-CT HU positions), not a site calibration — see the
round-trip artifact under `planning/hu-demo/`, where a synthetic
HU volume at the anchors recovers the phantom's assignment voxel-exact
and its solve is bit-identical to the ground-truth solve.

### Standard tissue library

`libraries/tissue/` ships versioned, hash-listed inputs so a head CT reaches
a transport solve without authoring materials or collapsing cross sections
(`manifest.json`, `openbnct.library-manifest/0.1.0`, lists every file's
sha256 and its source):

- `materials/*.json` — `openbnct.material-definition/0.1.0` for dry air,
  liquid water, adipose, soft tissue, skeletal muscle, brain, skin, blood,
  lung, cortical bone (all N-bearing tissues carry N14). Element weight
  fractions are transcribed from **PNNL-15870 Rev. 1** (the printed
  "(ICRP)" entries, which reproduce NIST 1998 tabulations); the manifest
  records each entry's printed name, entry number and page. Elements
  and isotopes with no local ENDF/B-VIII.1 evaluation (Ar, Si, Zn,
  Fe54/57/58, S36) are dropped, never substituted, and the rest
  renormalized; `audit.json` records every dropped mass fraction (largest:
  argon in air, 1.28 %; otherwise at most 0.01 %). `lung-inflated-declared`
  is the same lung composition at a *declared* density of 0.26 g/cm3, which
  is not a PNNL value. Spongiosa and marrow are omitted (no source read).
- `hu-calibration-generic-head-ct.json` — `openbnct.hu-calibration/0.1.0`,
  a **generic demonstration table, not a scanner calibration**: anchors at
  air −1000, inflated lung −740, adipose −100, water 0, brain 30, skeletal
  muscle 50, skin 60, cortical bone 1200 HU. The HU positions are
  declared conventions of the library, not values read from Schneider,
  Bortfeld & Schlegel, *Phys. Med. Biol.* 45 (2000) 459, whose
  stoichiometric-calibration structure (linear volume mixtures between
  anchors) `dicom calibrate` uses. Substitute a site table for real work.
- `multigroup-data-28g-tsl.json` (default; `builtin:tissue/multigroup-data-28g-tsl`)
  — 28-group collapse of all library materials with the ENDF/B-VIII.1
  H-in-H2O thermal-scattering law on hydrogen (thermal upscatter), free-gas
  kernel elsewhere, no self-shielding, re-collapsed after the
  redundant-reaction fix (`4d477ca`), bound to the local-kerma component
  profile, with `boron_unit_response_gy_cm2_per_ug_g`. Regenerate with
  `libraries/tissue/collapse-28g.sh` (about 1.5 minutes, under 100 MB RAM;
  needs the external nuclear-data store).
- `multigroup-data-28g.json` (`builtin:tissue/multigroup-data-28g`) — the
  earlier free-gas-only collapse (also re-collapsed after the fix; `FREE_GAS=1 collapse-28g.sh`), kept for provenance.

End to end, on the synthetic NF-BNCT-001 study (research demonstration only):

```text
openbnct benchmark generate STUDY-DIR
openbnct dicom import-ct --series STUDY-DIR --spacing-mm 8 \
  --case-id mylab.head.v1 --base-material libraries/tissue/materials/air-dry.json \
  --case-output CASE.json --hu-output HU.nii --masks-dir MASKS-DIR
openbnct dicom calibrate \
  --calibration libraries/tissue/hu-calibration-generic-head-ct.json \
  --hu-nifti HU.nii --case CASE.json \
  --output ASSIGNMENT.json --materials-out-dir NEW-MATERIALS-DIR
openbnct beam bind --beam beams/fir1-k63.json --case CASE.json --output CASE-BEAM.json
openbnct sn solve --case CASE-BEAM.json \
  --data libraries/tissue/multigroup-data-28g-tsl.json --assignment ASSIGNMENT.json \
  --order 4 --dose DOSE.json --boron-unit-output UNIT-DOSE.json --output FLUX.json
```

`import-ct` leaves a placeholder source, so a beam must be bound before
`sn solve`. The synthetic study is uniform 0 HU, so this exercises only the
water anchor; feeding an HU volume that spans the anchors produces
`voxel_fractions` mixtures, which `sn solve` accepts directly with the
library data (it blends the per-material macroscopic tables). The
checked chain ran with `--max-outer 2 --allow-unconverged` on the 25³
grid, S4, and emitted a provisional (unconverged) field — proof the chain
executes, not a converged result. `--materials-out-dir` files are the
anchors again and are not needed when solving against the library data,
whose material ids are the library's.

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
grid. Under the base-material profile, generation gates keep the result
scientifically meaningful — the assignment's base material must equal the
bound material artifact byte-for-byte, region temperature must match the
base (the cross sections are bound to it), regions may not introduce
nuclides absent from the base material, and only nuclides covered by
`njoy_partial_kerma_fluence_fold` component estimators may change fraction;
uncovered nuclides must match the base exactly so the residual response
tables stay valid. Region *density* may differ: collection normalizes native
heating by the per-voxel mass. `voxel_fractions` mixture regions are refused
under this profile (use the unit-mass-fraction profile below).

The folded responses are mass KERMA (Gy cm^2 per unit fluence), which does
not depend on density at fixed composition, so `openmc collect` applies only
a per-voxel region/base mass-fraction ratio to each covered component's
values and 1-sigma uncertainties (no density factor), leaving residual,
photon, and native-heating components untouched. The emitted assignment and
component profile are copied into the deck directory and hash-verified
against the manifest before any correction is applied.

#### Multi-tissue decks: the unit-mass-fraction profile

A HU-calibrated head (skin, brain, bone, air) differs from any one base
material in every nuclide, which the profile above cannot represent. The
component profile
`examples/openmc-multimaterial/component-profile-unit-mass-fraction.json`
(`unit_mass_fraction_kerma_fold` for B10 and N14,
`native_heating_residual` for hydrogen, `coupled_photon_heating` for photon)
removes the single-base-material gate:

- boron(v) = `w_B10(v)` × fold(v), nitrogen(v) = `w_N14(v)` × fold(v), where
  fold(v) is the flux ⊗ HEATR partial-KERMA curve **per unit mass fraction**
  divided by the voxel volume and `w` is the mass fraction of the material
  actually realized in that voxel;
- hydrogen(v) = native neutron `heating` per voxel mass − boron(v) −
  nitrogen(v), the same residual definition the base-material profile uses
  (total KERMA minus the two partials). Both terms come from the same
  tracks, so the reported sigma adds them in quadrature, which overstates
  it (conservative). Collection refuses a voxel whose residual is negative
  beyond round-off: that would mean the library's total heating is below
  the partial KERMA it contains;
- photon(v) and the physical total are the same native tallies as before.

The unit curves are not a new NJOY product: they are the reviewed response
set's B10 and N14 curves divided by the set's own source-material mass
fractions. Generation therefore takes the three artifacts the response set
is bound to (`--unit-source-component-profile`, `--unit-source-material`,
`--unit-source-nuclear-data-manifest`), re-verifies those bindings, and
requires the deck's manifest to select the identical B10 and N14
evaluations. The deck's own manifest must list every nuclide of the base and
all region materials (and their photon elements); a nuclide missing from it
is refused. A material assignment is required, region temperatures must
match the base, and acceptance contracts are not yet supported under this
profile.

`voxel_fractions` mixtures (for example the two-anchor mixtures
`dicom calibrate` emits between HU anchors) are realized by quantizing each
voxel's volume fractions to `--mixture-levels` levels (default 20;
largest-remainder rounding, so quantized fractions always sum to one). Every
distinct quantized mixture becomes one OpenMC material (volume-weighted
density; mass fractions from the volume-weighted partial densities) and one
lattice universe, and collection uses the *realized* composition and
density, not the declared one. The manifest records the rule, the level
count, every realized material with its component level counts and voxel
count, the number of mixture voxels, and the largest quantization error in
any volume fraction. Mixtures may not combine materials with different
temperatures or boron microdistributions.

```text
openbnct openmc generate \
  --case CASE.json \
  --component-profile examples/openmc-multimaterial/component-profile-unit-mass-fraction.json \
  --material BASE-MATERIAL.json --source SOURCE.json \
  --response-set benchmarks/synthetic/nf-bnct-001/transport/provenance/neutron-response-set.json \
  --nuclear-data-manifest CASE-SCOPED-MANIFEST-COVERING-ALL-TISSUES.json \
  --execution-profile benchmarks/synthetic/nf-bnct-001/transport/openmc-smoke-profile.json \
  --nuclear-data-root PATH-TO-SELECTED-ENDFB81-HDF5-ROOT \
  --assignment HU-CALIBRATED-ASSIGNMENT.json \
  --unit-source-component-profile benchmarks/synthetic/nf-bnct-001/transport/component-profile.json \
  --unit-source-material benchmarks/synthetic/nf-bnct-001/transport/material.json \
  --unit-source-nuclear-data-manifest benchmarks/synthetic/nf-bnct-001/transport/provenance/openmc-endfb81-processed-data-manifest.json \
  --mixture-levels 20 \
  --output NEW-DECK-DIRECTORY
```

Thermal scattering: `--thermal-scattering NUCLIDE=TABLE` (repeatable, for
example `H1=c_H_in_H2O`; `openmc generate` and `openmc run`, unit profile
only) emits `<sab name="TABLE"/>` in every deck material containing the
nuclide. The table must be listed as `thermal` in the data root's
`cross_sections.xml`; the input manifest records the nuclide, table name,
library file, its SHA-256 and the number of materials carrying it. Without the
flag every nuclide uses free-gas scattering. `openbnct openmc data
select-manifest --base-manifest B --data-root ROOT --assignment A.json
--manifest-id ID --output M.json` derives the case-scoped nuclear-data manifest
for an assignment's nuclides from a reviewed base manifest, inspecting the
HDF5 tables it does not already carry. `openmc run --boron-unit-dose-output`
writes the unit dose in the same invocation.

`openbnct openmc collect --boron-unit-dose-output NEW-UNIT-DOSE.json`
additionally writes the tissue-independent `openbnct.boron-unit-dose/0.1.0`
artifact (Gy per source particle per µg/g of B-10 — the boron fold per unit
mass fraction times 1e-6, with its 1-sigma) for decks generated under this
profile. `openmc run` accepts the same `--unit-source-*` and
`--mixture-levels` flags.

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
independent, resolved in `region_priority` order (then alphabetically).
If any voxel could match more than one declared region, the model must
declare `region_priority` — overlapping masks without it are an error,
since silent alphabetical resolution would shadow nested ROIs (tumor
inside `brain` would take `brain` weights). `component_weight_uncertainty`
(σ_w/w per component) propagates declared CBE/RBE uncertainty into the
biological σ in quadrature with the transport uncertainty. Models carry
a free-text `validity_domain` for provenance.

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

The components combine in *effect* space under the mixed-field MKM —
`X = Σᵢ (α₀ + β·z̄₁D,ᵢ)·dᵢ + (Σᵢ √βᵢ·dᵢ)²` (Zaider–Rossi √β synergy) —
and the photon-equivalent dose inverts `α₀·D + β·D² = X` once; summing
per-component inversions would double-count the quadratic cross terms.
Component volumes report each component's effect-share of the
isoeffective dose (they still sum to the total exactly); the emitted
bundle marks `microdosimetric_kinetic` semantics, an `mkm_weighted_*`
unit, and a `microdosimetry` block binding the resolved lineal energies
and spectrum hashes. MKM outputs are
research artifacts — they assert no clinical RBE/CBE/Gy-Eq claim, and a
weight model cannot claim `microdosimetric_kinetic` semantics.

A third model family implements the González & Santa Cruz (2012)
photon-isoeffective dose: `openbnct.isoeffective-model/0.1.0` declares
per-component dose-independent factors (`rbe` for the linear term — the
boron CBE — and `rbe_beta` for the quadratic term, photon pinned to
1.0/1.0), the tissue's photon-reference `alpha_0`/`beta`, optional
per-region factor and reference-LQ tables, and an optional `irradiation`
block (`duration_s`, `repair_rate_per_s`) producing the Lea–Catcheside
repair factor `G = 2(μT−1+e^(−μT))/(μT)²`. The model equates the
mixed-field exponent `X = α_γ·Σ rbeᵢ·dᵢ + G·β_γ·(Σ √rbeβᵢ·dᵢ)²` to the
photon LQ and inverts once — unlike fixed-weight `photon_isoeffective`
semantics, this is the full IsoE formalism including component synergy
and protracted-delivery repair. Bundles carry `photon_isoeffective`
semantics, `isoeffective_*` units, and an `isoeffective` provenance
block recording the applied G.

The MKM path is a mean-field theory — it consumes only the dose-mean
lineal energy, so the survival plateau set by untouched and
under-dosed cells in a heterogeneous-uptake population is invisible to
it. `bio cell-microdosimetry` samples the population directly under a
`openbnct.boron-microdistribution` model: each cell's ¹⁰B amount draws
from a gamma heterogeneity (shape 1/CV²), its captures are Poisson,
and each capture places an isotropic back-to-back α/⁷Li pair in the
declared compartment, depositing `ε = E·min(1, l/R)` into the nucleus
sphere. The `openbnct.cell-microdosimetry/0.1.0` artifact records the
declared sampling (cell count, mean captures, splitmix64 seed —
bit-identical replay), the specific-energy histogram P(z), the
untouched fraction, per-compartment capture tallies, and the
nucleus-domain `openbnct.lineal-spectrum` (MKM-consumable).
`bio smk` then replays the declared seed and evaluates the SMK
population integral `S = ⟨exp(−a·z − b·z²)⟩` at declared dose levels
alongside the MK mean-field of the same population and the
isosurvival RBE against a declared photon LQ reference — the model
document's sha256 is verified against the artifact's binding first.
Both artifacts are research-only; the z-rescaling anchor and small-λ
approximation are stated in the record.

An `openbnct.smk-model/0.1.0` folds that population into the voxel
dose pipeline: it declares the nucleus-domain SMK coefficients, the
photon reference, the boron-dose ↔ mean-captures anchor in absolute
Gy, and — required on `gray_per_source_particle` input — the
`source_particles_per_fraction` scale. `bio apply` routes on the
model's `schema_version` and needs two extra inputs verified against
the artifact's bindings; per voxel the boron dose rescales the
stored P(z), non-boron components add declared photon-LQ exponents,
and the total inverts once through the reference LQ. The bundle
carries `smk_stochastic` semantics, effect-share component volumes
that sum to the total, an `smk` provenance block, and `unavailable`
uncertainty.

```text
openbnct bio apply \
  --model SMK-MODEL.json \
  --physical-bundle DOSE-BUNDLE.json \
  --cell-microdosimetry CELL-MICRODOSIMETRY.json \
  --microdistribution BORON-MICRODISTRIBUTION.json \
  --output NEW-SMK-BUNDLE.json
```

```text
openbnct bio cell-microdosimetry \
  --model BORON-MICRODISTRIBUTION.json \
  --mean-captures 5 [--cells 10000 --seed 1] \
  [--z-edges-gy 0,0.25,... --y-edges-kev-um 0,10,...] \
  --output NEW-CELL-MICRODOSIMETRY.json
openbnct bio smk \
  --cell-microdosimetry CELL-MICRODOSIMETRY.json \
  --model BORON-MICRODISTRIBUTION.json \
  --alpha 0.6 --beta 0.05 [--reference-alpha 0.2 --reference-beta 0.02] \
  --boron-dose-gy 5.0 --dose-levels-gy 1,2.5,5,10 \
  --output NEW-SMK-EVALUATION.json
```

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

### Biological-parameter evidence library

`openbnct.bio-evidence-library/0.1.0` documents hold typed parameter
records — estimate + unit, uncertainty kind (SD, SE, CI, population
range, or explicitly unavailable), experimental context (compound,
species, tissue, endpoint, microdistribution), extraction provenance
(primary source, location, method, sample size, extractor, reviewer),
and applicability limits. A seeded BPA/BSH collection lives at
`validation/bio-evidence-library.json`; conflicting study values stay
as separate records rather than being averaged.

`bio evidence search` ranks every record against a declared context —
compound and species mismatches are *unsupported* (a mouse endpoint is
never silently applied to a human model), tissue/endpoint/undeclared
fields are *partial*:

```text
openbnct bio evidence search --library validation/bio-evidence-library.json \
  --compound BPA --species human --endpoint protocol-convention
```

`bio evidence info` prints one record in full:

```text
openbnct bio evidence info --library validation/bio-evidence-library.json \
  --record tb-melanoma-kashino
```

`bio evidence to-model` converts selected records into a real
`openbnct.biological-model/0.2.0` document — one `--weight
component=record-id` per dose component, optional `--region-weight
region:component=record-id` overrides (each region starts from the
global weights, so the region map stays a complete table). Any binding
whose record only partially matches the declared context requires a
`--assumption` line that lands in the model's `validity_domain`:

```text
openbnct bio evidence to-model \
  --library validation/bio-evidence-library.json \
  --weight boron=cbe-bpa-normal-tecdoc1223 \
  --weight nitrogen=rbe-nitrogen-tecdoc1223 \
  --weight hydrogen=rbe-hydrogen-tecdoc1223 \
  --weight photon=rbe-photon-tecdoc1223 \
  --region-weight tumor:boron=cbe-bpa-tumor-tecdoc1223 \
  --species human --compound BPA \
  --assumption "nitrogen/hydrogen protocol RBE records do not declare compound" \
  --id evidence-model-v1 --output NEW-MODEL.json
```

Only `standard_deviation` records map onto the model's
`component_weight_uncertainty`; an SE or CI cannot be realized as a
sampling distribution without design information, and `unavailable`
uncertainty stays absent. The library's `to_joint_source` adapter
realizes compatible records as UQ-01 joint sources (SD → Normal or
LogNormal, population range → bounded Uniform; SE/CI refused), so
record-level covariance and uncertainty feed `uq joint` ensembles.

Python exposes the library as `load_bio_evidence_library` and
`search_bio_evidence`.

### Retrospective delivery histories

`openbnct.delivery-history/0.1.0` documents normalize what was measured
during an irradiation session: content-bound beam and calibration
references with calibration validity windows, a shared coordinate
frame, and typed streams — instantaneous readings, interval-average
rates, integrated counts, cumulative counters, and beam-state events.
Each stream declares its clock basis (offset, optional drift ppm,
timing uncertainty, epoch), and assay streams keep result-availability
time separate from draw time. Missing segments are reported as gaps —
never turned into zero-output intervals or stale carried values.

`delivery import` builds a history from CSV streams under a strict
`openbnct.delivery-csv-import/0.1.0` mapping spec (declared column
names, delimiter, comment char, clock, units; quoted fields and unknown
quality/state values are rejected; extra columns are ignored but
reported). Reordering, dropped duplicates, resolved counter rollovers,
and declared resets are reported per stream:

```text
openbnct delivery import \
  --spec validation/delivery-history-import/import-spec.json \
  --output NEW-HISTORY.json
```

`delivery info` prints streams, converted interval rates, beam-on
intervals, coverage gaps over an optional window, and
expired-calibration warnings:

```text
openbnct delivery info --history HISTORY.json --window 0,1200
```

Policies are explicit: duplicate timestamps and out-of-order samples
are resolved at import (counted in diagnostics) and rejected outright
in a stored history; cumulative decreases need a declared reset or a
`counter_bits` rollover; overlapping rate intervals, unzoned clock
epochs, and stream frames mismatched with the history frame all fail
validation. This is a read-only import workflow — no beam control,
record modification, or upload.

### Expected value of information

`uq voi` ranks proposed measurements by the exact expected reduction in
a declared linear dose metric's variance under a `joint/0.1.0`
uncertainty state — the linear-Gaussian conjugate update, so no
ensemble is sampled. Each candidate declares which sources it observes,
its measurement noise, `repeats` (independent replicates), and an
optional shared calibration-bias source whose variance is a floor that
replication cannot reduce. A candidate with undeclared measurement
precision reports *unavailable*, not a flattering default; cost is
printed alongside unless the spec declares a cost-adjusted objective.
This compares research measurement designs — it does not schedule care
or control equipment:

```text
openbnct uq voi --spec VOI-SPEC.json --output NEW-REPORT.json
```

### Measurement-informed boron estimation

`boron infer` runs a sequential linear-Gaussian estimator over a small
declared state (region concentration scales, clearance corrections,
shared calibration terms) against typed observations: blood/total
assays, PET surrogate readings (which require an explicit transfer
declaration — PET is a surrogate, never the concentration itself), and
raw prompt-gamma count bins with background and live-time metadata.
The report carries the state trajectory, posterior/prior σ ratios,
normalized residuals, low-count flags, unobserved coverage spans, and
state-space directions the data did not resolve — two states that
produce identical predicted observations stay indistinguishable rather
than returning a falsely precise estimate. It is a forward filter
ordered by observation availability, not a retrospective smoother:

```text
openbnct boron infer --spec BORON-SPEC.json --output NEW-REPORT.json
```

### Held-out plan comparison

`plan compare` evaluates plan results — nominal and robust weights —
on a scenario set disjoint from the set the robust weights were
optimized against. The optimization set is declared with
`--trained-on`; any name overlap with the held-out set fails, because
performance on the training scenarios is not independent robustness
validation. The report keeps the nominal-scenario achieved value next
to the held-out worst case for every objective, so the nominal-quality
cost of robust weights stays visible. Emits
`openbnct.heldout-comparison/0.1.0`:

```text
openbnct plan compare \
  --result NOMINAL-RESULT.json --result ROBUST-RESULT.json \
  --objective OBJECTIVE.json \
  --scenario-set HELDOUT-SET.json \
  --trained-on OPTIMIZATION-SET.json \
  --dose BEAM-BUNDLE.json ... --mask MASK.json ... \
  --output NEW-COMPARISON.json
```

### Qualification-readiness records

`qual info` prints a `openbnct.qualification-record/0.1.0` claim matrix
— level (conformance / numerical / measured / external / clinical),
status, evidence bindings, and the honest `absent_records` list;
`qual verify` cross-checks every claim's evidence against the benchmark
catalogue's entry ids. Clinical and facility claims can only ever be
recorded as `external_dependency` — the record cannot hold them:

```text
openbnct qual info   --record qualification-record.json
openbnct qual verify --record qualification-record.json \
  --catalogue benchmark-catalogue.json
```

### Dose-to-outcome research exports

`outcomes validate` checks an `openbnct.outcomes-export/0.1.0` study
export's linkage and honesty rules: every session belongs to a course
and participant (a repeated session is never an independent record),
chronology is monotone, dose records bind session×ROI of the same
participant with `planned`/`reconstructed` kept distinct, endpoint
scoring stays within one declared system+version per endpoint, and a
missing follow-up requires a `missingness_reason` — absent data is
never read as no event. `outcomes export` applies a declared field
whitelist and reports every excluded path — a software check, not a
certification of anonymization:

```text
openbnct outcomes validate --export EXPORT.json
openbnct outcomes export --export EXPORT.json --whitelist WHITELIST.json \
  --output NEW-FILTERED.json --excluded-output NEW-EXCLUSIONS.json
```

### Retrospective dose reconstruction

`replay run` reconstructs the accumulated physical dose a recorded
irradiation delivered: it integrates each beam's output stream (× the
declared concentration history for the boron component — the product
is integrated jointly, not as a product of averages) over delivered
beam-on intervals, scales the bound rate-bundle maps into absolute
Gray, and emits a reconstructed `physical-dose-bundle` plus an
`openbnct.dose-replay-report/0.1.0`. The report carries delivered
seconds, integrated output, unobserved gaps, suspect intervals,
expired-calibration spans, and planned-vs-reconstructed deltas when a
planned bundle is bound. Repair-capable biological models assume
constant-rate protraction, so on recorded (possibly interrupted)
histories `biological_equivalence_available` is `false` — physical
dose only:

```text
openbnct replay run \
  --spec REPLAY-SPEC.json --history HISTORY.json \
  --bundle epithermal-1=RATE-BUNDLE.json \
  --bundle-output NEW-RECONSTRUCTED.json --report-output NEW-REPORT.json
```

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

### OpenPINT treatment workbooks

`openbnct import openpint` ingests a whole OpenPINT Excel workbook — the
`.xlsx` layout `PlanConfig.from_excel` consumes — in one pass: the `bnct`
sheet's component NIfTIs lift into a physical dose bundle, every
`GTV`/`CTV`/`PTV`/`HOM`/`OAR` mask rasterizes to a `RegionMask` on the
bundle grid (nearest-neighbour when the mask's geometry differs), and a
`openpint-plan-summary/0.1.0` artifact records per-structure boron
concentrations, OAR `max_dose`/`mean_dose` constraints, optional hadron
courses, and the workbook's SHA-256 as provenance:

```text
openbnct import openpint \
  --workbook plan.xlsx \
  --case-id my-case \
  --unit gray_per_source_particle \
  --normalization "OpenPINT MCNP6 tallies resampled to CT grid" \
  --producer-version 7d035fb \
  --out imported/
```

Paths in the workbook resolve relative to the workbook's directory and must
stay inside it: absolute paths, `..` components, symlinks that lead outside
the directory, and anything that is not a regular file are rejected before
any file is read.
Component keys map `B10`→boron, `N14`→nitrogen, `n`→hydrogen, `g`→photon;
unknown keys are rejected. The emitted bundle is a standard
`physical-dose-bundle` — every downstream surface (DVH, biological
models, gamma comparison, planning) applies unchanged.

### Avify Dose engine connector

`openbnct avify` is the optional connector for the separately licensed
Avify Dose engine (R12; see `docs/R12_SCOPE_REVIEW.md` for the boundary).
The engine is not distributed with OpenBNCT. Three subcommands:

- `export-plan` — write the engine's voxel plan (`<prefix>_arrays.npz`
  int8 class map + ROI masks in z-y-x order, `<prefix>_meta.json` with a
  scalar `voxel_cm` and corner-origin `lower_left_cm_xyz`) and the engine
  plan JSON. The `openbnct.avify-spec/0.1.0` file declares the
  material→engine-class map (air/brain/cranium/scalp/tumour), ROI
  classes, and the engine plan fields; the declared uptake-uncertainty
  set passes through verbatim — the connector never computes extremal
  maps. Exports require isotropic voxels and a nonzero voxel count in
  every ROI class, and reject ambiguous overlapping region claims.
- `verify` — export, then run the engine as a bounded child process
  (`--engine-cmd`, `--timeout-s`, `--threads`; timeout kills and reaps
  the child). Prints the returned certificate — per-ROI certified `[L,U]`
  intervals vs criteria with PASS/FAIL/ADDITIONAL_EVIDENCE actions — and
  writes `avify-run.json`, a versioned receipt binding every input and
  artifact by SHA-256 plus the engine's self-reported version.
- `show`/`status` — render a `certificate.json`, and check an
  `avify-run.json` receipt against the filesystem (CURRENT/STALE per
  bound input).
- `diff` — compare two run directories: per-ROI interval/action changes
  plus which bound inputs differ.
- `review` — mark a certificate as reviewed:
  `openbnct avify review --outdir run --reviewer "name" --note "what was
  checked"` writes `review.json` (`openbnct.avify-review/0.1.0`), a
  marker hash-bound to the certificate bytes. A re-run under the same
  directory leaves the review STALE rather than silently attached;
  `status` and `show` report the state.

Receipt fields beyond the binding: `cold_start` marks a run whose
outdir had no prior certificate (its timings include one-time setup a
repeat run wouldn't pay); `warnings` carries non-fatal observations —
notably engine-version drift when the spec's optional `engine_version`
pin disagrees with `avify-dose --version`, or when the engine version
changed between runs in the same directory. The pin warns, never
refuses. Certificates from `avify-dose` 0.1.0+ also carry an `engine`
block (name + version) so the certificate is self-describing without
its receipt.

A runnable example (export is pure Rust; verify needs the engine +
OpenMC) is in `examples/avify/`. Certificate output is an empirical
two-evaluation envelope — research software, not a certified bound.

The Python package exposes the same pipeline —
`openbnct.avify_export_plan(case, assignment, spec, prefix)`,
`avify_verify(case, assignment, spec, outdir, engine_cmd, threads,
timeout_s)`, `avify_status(receipt)` (now including `cold_start`,
`warnings`, and `review` state), `avify_review(outdir, reviewer,
note)`, `avify_diff(before, after)`, and `avify_load_certificate(path)`
— each returning a JSON string of the artifact paths, run receipt
states, or the certificate verbatim. The workbench's Avify tab shells
out to `openbnct avify` for the same bounded run, renders the class map
with ROI verdict tinting, and exposes the review marker inline.

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

### Benchmark catalogue

`benchmark-catalogue.json` at the repository root is the versioned
`openbnct.benchmark-catalogue/0.1.0` index of every committed benchmark
and validation case. Each entry declares its problem class, evidence
kinds (analytic, published numerical reference, independent-engine
comparison, measured experiment, internal closure, declared-input
study, parser fixture), tolerances with *when they were set*, graded
versus reported-only regions, and per-item verdicts — including
recorded failures such as the uncollimated prompt-gamma localization
negative result. Artifact references are SHA-256 bound; `bench verify`
re-checks them read-only and reports broken references plus absent
license/uncertainty metadata rather than filling defaults.

```text
openbnct bench info --catalogue benchmark-catalogue.json
openbnct bench report --catalogue benchmark-catalogue.json --entry canonical-kobayashi-p1
openbnct bench verify --catalogue benchmark-catalogue.json --root . \
  --id VERIFY-001 --output NEW-VERIFY-REPORT.json
```

An outside implementation can consume the catalogue and submit against
the declared references without running OpenBNCT as its dose engine.

### Deterministic multigroup transport (S_N)

`openbnct sn solve` is the in-house deterministic transport path — a
3-D Cartesian discrete-ordinates solver (diamond difference with a
step-characteristic fixup on cells whose optical thickness exceeds the
diamond positivity bound — unconditionally positive AND conservative,
unlike a plain clamp which annihilates particles) over the
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
heterogeneity. The outer iteration alternates the group-sweep
direction, which makes the composed map two-step: on
near-conservative periodic problems it can settle into a stable
period-2 orbit — the field is correct while the one-step residual
sits at the oscillation amplitude. Convergence is therefore decided
on the same-parity (two-step) distance, and a converged cycle emits
the parity midpoint; `residual_site` on the artifact names the
(cell, group) limiting the last iterate, and `--allow-unconverged`
emits a provisional field with `converged: false` for inspection.
Nonconvergence within the iteration budget is otherwise a hard
error. Periodic transverse boundaries are a solver-side choice —
`--periodic x,y` — not part of the case document; comparisons against
an infinite-column MC tally need both sides periodic.

`sn collapse` emits the multigroup-data artifact itself from the
processed ENDF HDF5 library (`--library`), a material definition
(`--material`), and a group structure (`--boundaries` or
`--boundaries-file`), with optional TSL tapes (`--tsl H1=...`),
heterogeneous self-shielding (`--self-shielding`), a measured or
fine-solve spectrum (`--weighting-spectrum`), and survival weighting
(`--attenuation-depth Z` — multiplies every collapse weight by
exp(−σ_t(E)·z) so the group constants bias toward the in-group
penetrating tail). Alongside the flux-weighted σ_t and P0 scatter
matrix it emits `transport_mu_bar`, the P1 transfer matrix, higher
Legendre moments (for `--p1`/`--anisotropy` solves), and
`beam_sigma_nodes_per_cm` — a 4-node sub-bin σ_t kernel so a
`uniform_in_bin` beam source attenuates as an exponential mixture
rather than a single group mean. `sn boundaries` proposes group-edge
placements for a spectrum or importance document and
`sn spectrum` extracts a flux histogram from a solved field —
both consumable by `sn collapse`'s `--boundaries-file` and
`--weighting-spectrum`.

Scope is honestly bounded: isotropic (P0) scattering by default
(P1+ available on data carrying the moments), no fission,
multigroup data as declared input — a verification solver, not a
production engine, and its output is research-only.

#### Large flux and dose artifacts

Above `OPENBNCT_SIDECAR_MIN_VALUES` values (default 1,000,000; `0` = always,
`-1` = never) `sn solve`, `sn fold`, `boron dose` and the other dose writers
store the flux, dose values and uncertainties as raw little-endian `f64` files
next to the JSON (`<output-stem>.<field>.f64le`), referenced by SHA-256 from
the document. Every command that reads these artifacts resolves and verifies
the files transparently; copy the JSON and its `.f64le` files together.
`openbnct evidence export` bundles them automatically. See
`docs/ARCHITECTURE.md` (Large numeric arrays).

### Prompt-gamma verification chain

The `openbnct prompt-gamma` and `openbnct pg` commands build the
478 keV delivery-verification chain — the physics source term plus the
forward model every PG-imaging programme (BNCT-SPECT, Compton camera,
PG-SPECT) consumes for detector design and reconstruction research.
Research instruments only — not imaging devices or clinical monitors.

`prompt-gamma` derives the emission map from a dose bundle's boron
component (`openbnct.prompt-gamma-source/0.1.0`): captures/kg =
D_boron/E_charged with the declared 2.34 MeV branch-weighted kerma,
photons = captures × 0.94, monoenergetic 478 keV isotropic emission.

```text
openbnct prompt-gamma \
  --dose physical-dose-bundle.json \
  --id case.pg-source.v1 --output pg-source.json
```

`pg response` solves the photon *adjoint* problem for a declared
detector voxel region — one S_N solve returns that position's whole
response-matrix column: the sensitivity of a fluence-weighted detector
tally to a photon born in any voxel of the case
(`openbnct.pg-response/0.1.0`). Detectors outside the phantom live at
boundary-adjacent or void-assigned voxels; the detector region is one
or more `i,j,k` voxels on the case grid:

```text
openbnct pg response \
  --case validation/fir1-k63-water-phantom/case.json \
  --photon-data validation/fir1-k63-water-phantom/multigroup-photon-data-16g.json \
  --detector 13,13,0 --detector 12,13,0 \
  --order 8 --id case.pg-response.z0.v1 --output pg-response.json
```

`pg counts` folds an emission map against a response map into the
expected detector tally (`openbnct.pg-counts/0.1.0`) — the linear
inner product `Σ_v emission·mass·sensitivity` under a declared uniform
voxel density and an optional scalar efficiency calibration (crystal
volume, collimation — declared, not modeled):

```text
openbnct pg counts \
  --emission pg-source.json --response pg-response.json \
  --density-kg-per-m3 1000 --efficiency 1.0 \
  --id case.pg-counts.v1 --output pg-counts.json
```

`pg observe` collects counts artifacts into a synthetic observation
(`openbnct.pg-observation/0.1.0`) — each detector's expected tally
becomes a measured reading bound to its response artifact. Real
measurements are authored in the same schema directly.

```text
openbnct pg observe \
  --counts pg-counts-z0.json --counts pg-counts-z1.json \
  --id case.pg-obs.v1 --output pg-observation.json
```

`pg reconstruct` inverts an observation back onto the emission grid
(`openbnct.pg-reconstruction/0.1.0`): non-negative least squares with
a declared Tikhonov λ, FISTA with a power-iterated step bound. Every response file's sha256 is verified against the
observation's content bindings before the solve — a swapped or stale
response artifact is a hard error, never a silent column.

```text
openbnct pg reconstruct \
  --observation pg-observation.json \
  --response pg-response-z0.json --response pg-response-z1.json \
  --density-kg-per-m3 1000 --lambda 1e-4 --max-iterations 2000 \
  --id case.pg-recon.v1 --output pg-reconstruction.json
```

Each detector position takes one adjoint solve; the observation pairs
the resulting response columns with measured tallies, and the
reconstruction carries residual and convergence diagnostics in its
regularization declaration.

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
`max_dose_at_volume`) on named region masks over `physical_total`, a
named component, or `isoeffective` — it solves the non-negative weight
assignment across per-beam dose bundles by deterministic cyclic
coordinate descent with analytic metric gradients and per-coordinate
Armijo line search. A `weight_regularization` term selects the
minimum-total-weight feasible plan (feasibility alone is a plateau).
Each `--dose` bundle is one beam's unit-weight dose field, named by
file stem; each `--mask` is a `RegionMask` JSON (`{"name", "voxels"}`)
that every objective mask must resolve against. The result is
`openbnct.inverse-plan-result/0.1.0` — per-beam weights, per-objective
achieved/violation, iteration count, and convergence, content-bound to
the hashed objective document and qualified
`inverse_planning_research_only_not_clinical`:

```text
openbnct plan optimize \
  --objective OBJECTIVE.json \
  --dose BEAM-AP-DOSE.json --dose BEAM-PA-DOSE.json \
  --mask TUMOR-MASK.json --mask OAR-MASK.json \
  [--initial 1.0 --initial 1.0] \
  [--emit-plan NEW-EXPOSURE-PLAN.json --seconds-per-weight 60] \
  --output NEW-RESULT.json
```

The solver is deterministic — same inputs, byte-identical weights.
Violations are normalized by their objective bounds inside the penalty,
so `weight_regularization` and `gradient_tolerance` are O(1) knobs at
any dose magnitude; with `weight_regularization` on, the converged plan
sits a fraction `≈ λ·bound/(2·slope)` inside a binding minimum-dose
bound (the minimum-weight trade-off — raise the objective's `weight`
or drop λ to tighten). `--emit-plan` writes the optimized weights as an
`openbnct.exposure-plan` (`source_strength_scaling` basis, dose bundles
hash-bound) for `plan validate`/`plan export`. It is a research
optimizer over linear dose superposition, not a commissioned
treatment-planning product; see `docs/IP_BOUNDARY.md` for the
optimization-adjacent scope boundary.

`dose_quantity: "isoeffective"` makes the objectives BNCT-native: the
objective document embeds a `bio_model` (`openbnct-bio`'s
`BiologicalModel`), each `--dose` bundle supplies its four component
fields, and the optimizer evaluates every objective against the
effective dose `Σ_c w_c·D_c` — the model's default component weights,
or its `region_weights` override for that objective's mask. The fold is
linear in the beam weights so every metric gradient stays analytic.
`microdosimetric_kinetic` semantics and `fractionation` schedules are
refused (non-linear — not expressible as component weights), a
`bio_model` on a physical quantity is rejected, and every
`region_weights` key must resolve to a supplied mask.

The objective document's optional `metrics` block evaluates the plan
the solver produced — per region it reports the voxel count and
mean/min/max under the optimized weighted dose, `dose_at_volume`
quantiles (D95-style), `volume_at_dose` at declared bounds,
generalized-EUD values for declared exponents, and `endpoint`
probabilities scored by `openbnct-bio` endpoint models (logistic,
probit, Poisson TCP). Metrics are *reported*, never optimized, and
isoeffective quantities honor `region_weights` so each mask is scored
with its declared component weighting. The CLI prints the table; the
result document carries `metrics` as `RegionPlanMetrics`; the GUI's
inverse-plan view renders it alongside the outcomes.

`--seconds-per-weight` on `--emit-plan` converts each optimized weight
to `duration_s` on the emitted exposures — beam-on seconds per unit
optimizer weight under the source-strength-scaling convention declared
in the plan contract (the S_N solves are normalized per unit source
rate, so weight × seconds-per-weight is the declared irradiation
time).

`plan directions` enumerates which beams are worth solving at all:
an azimuth×elevation grid of propagation vectors converging on the
aim-mask centroid, ranked by the millimetres of tissue each ray
crosses before reaching it (epithermal incidence favors shallow
entry). With `--data` a single adjoint solve with the aim region as
adjoint source re-ranks every candidate by its uncollided-beam × φ*
importance — transport-informed ranking at one solve's cost.
`--output` emits `openbnct.direction-candidates/0.1.0` content-binding
the ranked sweep (both scores, grid declaration) to the case, masks,
and data; the printed `name,dx,dy,dz` lines feed `--beam` verbatim:

```text
openbnct plan directions \
  --case CASE.json --aim-mask TARGET-MASK.json \
  [--body-mask BODY-MASK.json] [--azimuth-steps 12 --elevation-steps 3] \
  [--data MULTIGROUP-DATA.json --radius-cm 4.0] [--top 4] \
  [--csv candidates.csv] [--output direction-candidates.json]
# → stdout: name,dx,dy,dz spec lines for plan fields --beam
```

`plan fields` produces those per-beam dose bundles end to end: it aims
an on-face disk source per beam direction through an aim mask, solves
each field with the deterministic multigroup solver, folds a unit-weight
`openbnct.physical-dose-bundle` per beam, and emits a
`openbnct.beam-field-set/0.1.0` manifest content-binding every aimed
case, position report, and bundle to the shared inputs:

```text
openbnct plan fields \
  --case CASE.json --data MULTIGROUP-DATA.json \
  [--assignment ASSIGNMENT.json] \
  --aim-mask TARGET-MASK.json \
  --beam ap,0,0,1 --beam pa,0,0,-1 \
  --radius-cm 1.0 [--order 8 --anderson 5 …sn-solve options] \
  [--screen-order 2 --screen-convergence 1e-2 --keep-top K \
   --screen-max-inner 20 --screen-max-outer 80] \
  --output-dir NEW-DIR/
# → NEW-DIR/{beam}.{case,position-report,dose}.json + fields.json
openbnct plan optimize \
  --objective OBJECTIVE.json \
  --dose NEW-DIR/ap.dose.json --dose NEW-DIR/pa.dose.json \
  --mask TARGET-MASK.json --output NEW-RESULT.json
```

The aim emits `UniformDisk` on the grid face — the only source shape the
solver's boundary-flux and uncollided-split paths currently accept (the
rectangular `position aim` plane is not consumable by `sn solve`). A
beam that fails to converge aborts the sweep rather than emitting a
non-converged field.

`--screen-order` turns the sweep two-stage: every declared beam first
gets a cheap scoring solve at the screening options, the mean
`physical_total` dose inside the aim mask ranks the candidates, and
only the `--keep-top` best receive the full-quality solve and dose
bundle. The manifest's `screening` block records the screening solver
options, every declared beam's score, convergence flag, and retention —
`beams` then lists the retained set only. This is the intended path
when a large candidate list (from `plan directions`) is expensive at
fine fidelity: the coarse ranking is separable work, the fine solves
spend effort where the objective will actually draw weight.

`plan robustness` answers the follow-up question a weight vector leaves
open: under declared systematic σ on each beam's component dose, how
uncertain is each objective's achieved metric, and what is the
one-sided Gaussian probability of violating its bound? Declared sources
mirror `uq apply` (`--relative component=σ`, `--positioning-sigma-mm`,
`--boron-field`); component σ maps fold with each objective's
isoeffective weights, beam weights fold as independent contributions,
and the metric σ propagates first-order through the analytic gradient:

```text
openbnct plan robustness \
  --result RESULT.json --objective OBJECTIVE.json \
  --dose DIR/ap.dose.json --dose DIR/pa.dose.json … `# optimize order` \
  --mask TARGET-MASK.json --mask OAR-MASK.json \
  --relative boron=0.10 --positioning-sigma-mm 1.0 \
  --output NEW-ROBUSTNESS.json
# → openbnct.plan-robustness/0.1.0: per objective
#   {achieved, bound, sigma_1sigma, violation_probability}
```

The supplied objective is hash-verified against the one the result
recorded, and beam order must match the result's weight order — both
guards refuse a silently-mismatched propagation. Sources declared on
multiple beams are treated as independent (documented convention;
correlated cross-beam systematics are a known limitation). A worked
report lives in
`benchmarks/synthetic/layered-head-phantom/planning/`.

`plan scenarios` answers a different question with different machinery:
not *"how uncertain is the metric"* but *"what does the plan actually
deliver under these named, discrete perturbations?"* Gaussian
propagation is the wrong shape for bounded, asymmetric, structured
uncertainties — boron uptake inferred from a single pre-treatment
scan, per-structure T/N ratios, whole-field positioning offsets. An
`openbnct.scenario-set/0.1.0` declares them: `component_scales`
(global uptake/yield), `region_scales` (scale a component only at a
mask's voxels — how T/N uncertainty is expressed), `dose_scale`
(output factor), and `shift_mm` (whole-field displacement,
trilinear-resampled on the beam grid — a declared approximation; no
transport is re-solved). Each scenario refolds the optimized weights
and re-evaluates every objective; the report records the nominal
evaluation, per-scenario outcomes, and per-objective bands (min/max/
mean, worst scenario, violated set):

```text
openbnct plan scenarios \
  --result RESULT.json --objective OBJECTIVE.json \
  --scenario-set SCENARIO-SET.json \
  --dose DIR/ap.dose.json --dose DIR/pa.dose.json … `# optimize order` \
  --mask TARGET-MASK.json --mask OAR-MASK.json \
  --output NEW-SCENARIO-REPORT.json
# → openbnct.scenario-report/0.1.0: evaluations + bands
```

Component and region scales are exact — dose is linear in
concentration and component yield — and require the beam bundles'
component maps; `shift_mm` resamples each component so `values` stays
consistent with the shifted component sum. The same objective-hash
and beam-order guards as `plan robustness` apply.

The same set feeds `plan optimize --scenario-set SET.json`: the
optimizer then minimizes the *worst-case* composite penalty across
nominal plus all declared scenarios rather than the nominal alone —
each scenario's perturbed fields are computed once, and the argmax
scenario's own gradient drives the coordinate descent (a valid
subgradient by Danskin's theorem). The result records
`method: "worst_case_scenario"` alongside the nominal-plan outcomes —
robust plans typically carry deliberate nominal slack so the worst
scenario still clears its bounds, which `plan scenarios` on the
result makes explicit.

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
scalar statistic (mean/min/max/EUD). `endpoint utcp` combines a TCP evaluation with
one or more NTCP evaluations over the same case/quantity/dose source
under `p_plus` (`TCP·Π(1−NTCPᵢ)`) or `difference` — the standard
uncomplicated-control pairing puts TCP on the target and each NTCP on a
*different* organ at risk, so `--ntcp` is repeatable and regions are
recorded, not required to match. Demonstration models live in `examples/endpoint/`; all
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
- `frame-of-reference` records the common case where the moving and
  fixed series share one DICOM Frame of Reference UID — the transform
  is identity *by construction* and the declared UID (tag 0020,0052)
  is the evidence. A non-identity transform fails validation under
  this method: a real offset needs a fit or a declared matrix.

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
openbnct register frame-of-reference \
  --id REG-003 --uid 1.2.826.0.1.3680043.9.7 \
  --moving PET.nii.gz --fixed CT-STACK.nii.gz \
  --note "co-acquired PET/CT session" \
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

For exercising the chain without patient data, `openbnct dicom
synth-pet` writes a deterministic BQML series on the NF-BNCT-001 grid
and frame of reference (a companion volume — not part of the frozen
case): the benchmark CORE box carries `--core-suv` (default 4.0), the
background `--background-suv` (default 1.0), with a complete
radiopharmaceutical record so the import exercises the full SUVbw decay
correction:

```text
openbnct dicom synth-pet --output NEW-PET-DIR [--core-suv 4.0 \
  --background-suv 1.0 --weight-kg 70 --dose-mbq 500]
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

### Post-hoc boron: unit-concentration dose

BNCT practice treats ¹⁰B as a trace: transport once, record the boron
dose per unit ¹⁰B concentration, then apply blood concentration ×
tissue:blood ratios (or a per-voxel PET-derived field) at evaluation
time. Changing the concentration therefore does not require rebuilding
materials or re-solving. The mass kerma per µg/g of ¹⁰B is
tissue-independent, so the per-voxel boron dose is `D(v) = C(v) · u(v)`
with `u(v) = Σ_g φ_g(v) · unit_g`.

`sn collapse` emits, whenever a `B10` table is available, an optional
top-level `boron_unit_response_gy_cm2_per_ug_g` vector in the
multigroup data (Gy·cm² per µg/g; same (n,α) collapse, declared
2.34 MeV charged kerma and unit conversion as the material `boron`
response, so `material_boron[g] = w_B10 · 1e6 · unit[g]` for a material
with ¹⁰B mass fraction `w_B10`). The vector uses the bare declared
spectrum weighting — no per-material Bondarenko shielding or
penetration weighting — so that identity is exact for data collapsed
without `--self-shielding`/attenuation weighting. The field is an
additive optional member of `openbnct.multigroup-data/0.1.0`; data
collapsed earlier (all committed benchmark data) lacks it, and the
unit-dose fold refuses with an instruction to re-run `sn collapse`.

```text
openbnct sn solve --case CASE.json --data MG-DATA.json \
  --dose PHYSICAL-BUNDLE.json --boron-unit-output NEW-UNIT-DOSE.json \
  --output NEW-FLUX.json
openbnct sn fold --case CASE.json --data MG-DATA.json --flux FLUX.json \
  --boron-unit-output NEW-UNIT-DOSE.json --output NEW-PHYSICAL-BUNDLE.json
openbnct boron dose --physical-bundle PHYSICAL-BUNDLE.json \
  --unit-dose UNIT-DOSE.json --blood-ug-g 25 \
  [--ratio tumor=3.5 --mask tumor=tumor-mask.json \
   --ratio brain=1.0 --mask brain=brain-mask.json --default-ratio 1.0] \
  --output NEW-PHYSICAL-BUNDLE-25UG.json
openbnct boron dose --physical-bundle PHYSICAL-BUNDLE.json \
  --unit-dose UNIT-DOSE.json --boron-field FIELD.json \
  --output NEW-PHYSICAL-BUNDLE-PET.json
```

`--boron-unit-output` writes `openbnct.boron-unit-dose/0.1.0`: the grid,
`case_id`, per-voxel dose in Gy per source particle per µg/g, an
optional statistical σ (the deterministic fold carries none), content
references to the flux and multigroup data, and the declared trace-¹⁰B
assumption. A boron-microdistribution compound factor declared on a
material multiplies the unit dose exactly as in the ordinary fold.

`boron dose` writes a new physical dose bundle whose boron component is
`C(v) · u(v)` and whose physical total is
`old_total − old_boron + new_boron` (floored at zero against rounding).
Concentration comes from either a blood concentration with optional
tissue:blood ratio masks (`--ratio NAME=value` needs a `--mask
NAME=path` of the same name and vice versa; the first matching mask
wins per voxel; uncovered voxels use `--default-ratio`, 1.0 by
default), or a per-voxel `openbnct.boron-field/0.1.0` from `boron
apply`. The physical bundle, unit dose and field must share the exact
grid and `case_id`, or the command refuses. Other components are
carried over unchanged and units stay Gy per source particle.

Uncertainty handling: the new boron σ combines, in quadrature, the
unit-dose σ (when present) scaled by `C` and the field's 1σ scaled by
`u`; a scalar blood concentration and ratios are treated as exact. The
input bundle's total σ cannot be split into boron and non-boron parts
without covariance, so the output total σ is only reported when the
input carries both a total σ and a boron-component σ, and is then the
conservative triangle bound `σ_T,old + σ_B,old + σ_B,new` (keeping the
input's total-uncertainty method label; it is an upper bound, not a
dedicated estimate). Otherwise the total's uncertainty is
`unavailable`. Provenance records the concentration specification, the
unit-dose provenance and the assumption.

Declared approximation: the applied ¹⁰B is a trace — it does not perturb
the neutron flux. The flux is the one transported with whatever ¹⁰B the
transport materials carried; for concentrations far from that, the
flux-depression error is not modeled. Research software, not a clinical
quantity.

From Python: `openbnct.boron_dose(bundle, unit_dose, blood_ug_g=25.0,
ratios={"tumor": 3.5}, masks=[("tumor", "tumor-mask.json")])`.

`openbnct boron microdistribution evaluate` evaluates a ¹⁰B subcellular
microdistribution model (`openbnct.boron-microdistribution/0.1.0`) into
a correction record (`openbnct.microdistribution-correction/0.1.0`):

```text
openbnct boron microdistribution evaluate \
  --model microdistribution.json \
  --id openbnct.microcorr.bpa.v1 --output correction.json
```

Measured subcellular data enters the same model document through
`openbnct boron microdistribution import`, which reduces a declared
`openbnct.boron-microdistribution-measurement/0.1.0` artifact — an assay
method (autoradiography, ion microbeam, track imaging, fluorescence, or
a declared other), the compound and cell system, the reduction geometry,
and either directly-reported compartment fractions or a radial
boron-density profile — to the model JSON:

```text
openbnct boron microdistribution import \
  --measurement bpa-trackimaging-measurement.json \
  --output bpa-microdistribution-model.json
```

A radial profile integrates each measured bin's annulus mass into the
nucleus/cytoplasm/extracellular regions by radial overlap (the membrane
is unresolvable at bin scale — the measurement declares its fraction
explicitly); per-bin 1σ propagates through the linear mass map to the
fraction σ's. The emitted model carries the measurement's compound,
cell system, and assay in its validity domain, and is consumable by
`microdistribution evaluate`, `bio cell-microdosimetry`, and the
`smk-model` path unchanged.

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

### Joint uncertainty ensembles

`openbnct uq joint` evaluates a
`openbnct.joint-dose-ensemble-spec/0.1.0` request over a
`openbnct.joint-uncertainty-input/0.1.0` declaration, emitting a
`openbnct.joint-uncertainty-report/0.1.0`. Where `uq apply` propagates
per-voxel σ maps, `uq joint` declares named *sources* with identity,
scope, and distributions — a shared calibration error is drawn once per
realization and applied everywhere it scopes, `independent_per_target`
sources draw separately per scope target, and declared correlation
groups are sampled jointly through a Gaussian copula. Metrics (mean,
D/V statistics, EUD over named masks) are computed per realization on
the fully realized map and then summarized — D95 follows per-map order
statistics, not per-voxel interval summaries.

The input document carries per-source evidence and an explicit
disposition for every uncertainty category no source covers, so an
unassessed category is visible rather than silently absent. The spec
chooses `monte_carlo` (seeded, deterministic) or `weighted_samples`
(explicit realization sets), the quantiles to report, optional grouped
`attribution` (each group re-sampled with the other sources pinned —
correlated members may only be attributed jointly), dose adapters
(output/component/region scales) and, when the bundle is in rate units,
PK integration over realized `pk-model` parameter draws.

```text
openbnct uq joint \
  --dose BUNDLE.json \
  --joint JOINT-INPUT.json \
  --spec ENSEMBLE-SPEC.json \
  --pk-model PK-MODEL.json \
  --mask TARGET=mask.json \
  --id UQJ-001 --output NEW-JOINT-REPORT.json
openbnct uq joint-info --report JOINT-REPORT.json
```

A first-order counterpart without sampling is
`openbnct_plan::robustness::plan_robustness_joint`
(`openbnct.joint-robustness/0.1.0`): per-beam signed sensitivity maps
fold through the declared covariance so a shared source's σ does not
shrink when a field is subdivided, while `plan robustness` keeps its
original independent-per-beam convention unchanged.

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
diamond-difference-with-characteristic-fixup operator is nonlinear and a
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

The covariance artifact itself comes from an evaluated data tape:
`openbnct openmc cov-endf` reads an ENDF-6 file's MF33 section (NI
LB∈{0,1,5} blocks plus NC LTY=0 derived-quantity references; anything
outside that subset lands in an explicit skip ledger), collapses the
energy-grid covariance onto a `openbnct.multigroup-data` boundary set,
and binds it to a parameter — `sigma_total` for removal cross sections
(e.g. MT=1) or `dose_response` for a component's kerma response (e.g.
¹⁰B(n,α) MT=107 with `--component boron`).

```text
openbnct openmc cov-endf \
  --tape n-005_B_010.endf \
  --data transport/multigroup-data.json \
  --material openbnct.layered-head.brain.v1 \
  --mt 107 --parameter dose_response --component boron \
  --note "ENDF/B-VIII.1 n-005_B_010 MT=107" \
  --output NEW-COVARIANCE.json
```

Verified end-to-end on real data (2026-09-23): ENDF/B-VIII.1
`n-005_B_010.endf` MT=107 collapsed onto the layered-head 28-group
mesh → propagated through `uq propagate` → the ¹⁰B(n,α) covariance
alone contributes σ_rel ≈ 0.34% to the boron dose integral on that
benchmark.

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

`openbnct openmc cov-endf` derives the covariance artifact from a real
evaluated nuclear-data file instead — an ENDF-6 tape's MF33
cross-section covariance sections (NI-type sub-subsections, LB ∈
{0,1,5}: diagonal absolute/relative and symmetric energy-grid matrices)
collapsed onto the multigroup mesh by overlap-weighted averaging.
`--parameter sigma_total` binds the collapsed covariance to the
material's removal cross section (the default; MT=1 total XS);
`--parameter dose_response --component boron` binds a reaction MT's
covariance (e.g. MT=107, the ¹⁰B(n,α) channel) to that component's
dose-response vector instead. NC-type LTY=0 references — the form
ENDF/B-VIII.1 uses for ¹⁰B MT=107, where a reaction's covariance is
declared as a coefficient-weighted sum of derived-quantity sections
(MT ≥ 800) — are resolved by resampling contributors onto a union mesh;
LTY ∈ {1,2,3} (cross-material covariances) remain skipped with an
explicit ledger entry in `provenance_note`. A worked end-to-end
example on the real ENDF/B-VIII.1 ¹⁰B evaluation ships under
`examples/covariance/` — including the MT=107 (n,α) channel propagated
to a 0.34% σ_rel boron-dose budget on the layered-head phantom:

```text
openbnct openmc cov-endf   --tape n-005_B_010.endf --data transport/multigroup-data.json   --material MATERIAL-ID --mt 1 --parameter sigma_total   --output NEW-COVARIANCE.json
```

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

`openbnct pk` carries the pharmacokinetic layer the `--pk-model` flag
consumes. `pk fit` builds an `openbnct.pk-model/0.1.0` — per-region
exponential concentration curves — from measured draws
(`openbnct.pk-samples/0.1.0`); `irradiation-time --pk-model` then solves
beam-off under decaying concentration instead of the fixed-concentration
approximation. `pk tissue-scale` folds a declared tumor-to-blood
evolution (`openbnct.pk-tissue-spec/0.1.0`,
`T/B(t) = T∞ + (T₀ − T∞)·e^(−μt)`) into a blood model — the product
stays in the exponential family so the tissue curves are exact.
`pk schedule` searches the irradiation window: for each declared beam-on
epoch it shifts the curves, solves the organ limits, and reports the
deliverable tumor-region dose, emitting `openbnct.pk-schedule/0.1.0`
with the full landscape, the dose-maximizing window, and the
constant-concentration reference for comparison. `pk dose` then
emits the integrated map for a chosen window — either a solved
schedule window (`--schedule REPORT --window-index N`, which reuses
the report's limiting beam-off time) or an explicit
`--window-s W --time-s T` — as a real
`openbnct.physical-dose-bundle/0.2.0` in Gray. The boron component
scales per voxel by the region's integrated concentration; every
other component scales by the beam-on duration; voxels no mask
covers keep the constant-concentration scale. The bundle feeds
`bio apply`, `dvh`, and `report` unchanged:

```text
openbnct pk fit --samples PK-SAMPLES.json --id blood.v1 \
  --output NEW-PK-MODEL.json
openbnct pk tissue-scale --blood-model PK-MODEL.json \
  --spec PK-TISSUE-SPEC.json --id tissue.v1 --output NEW-TISSUE-PK.json
openbnct pk schedule --dose DOSE-BUNDLE.json --quantity physical_total \
  --source-strength 1e9 \
  --limit SKIN=mean:5.0 --mask SKIN=skin.json --mask GTV=gtv.json \
  --pk-model NEW-TISSUE-PK.json \
  --window-s 0,900,1800,3600 --tumor-region GTV --tumor-metric mean \
  --output NEW-SCHEDULE.json
openbnct pk dose --dose DOSE-BUNDLE.json --pk-model NEW-TISSUE-PK.json \
  --mask SKIN=skin.json --mask GTV=gtv.json --source-strength 1e9 \
  --schedule NEW-SCHEDULE.json --window-index 3 \
  --output NEW-INTEGRATED-DOSE.json
```

All three artifacts are research-only: the curves are declared inputs
with a stated basis, not a fitted clinical model.

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
[ADR 0028](adr/0028-mixed-evaluated-neutron-source-selections.md) and
[ADR 0029](adr/0029-tendl2025-mixed-source-candidate.md).

The derived diagnostic-triage gate is also a live dogfood case for Avila Core.
OpenBNCT keeps the domain verification and emits a deterministic machine
result; Core binds the exact executable and inputs, records the run, evaluates
the 43-finding queue, and independently enforces the closed response category.
A second integration binds the candidate-comparison check so a rejected
candidate is a verified result rather than a process failure. See the
[integration case](../integrations/avila-core/njoy-evidence-aware/README.md), the
[candidate-comparison case](../integrations/avila-core/njoy-candidate-comparison/README.md),
and [ADR 0024](adr/0024-avila-core-evidence-loop.md).

