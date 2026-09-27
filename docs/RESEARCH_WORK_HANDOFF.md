# Research work handoff: uncertainty, delivery replay, and boron estimation

Prepared 2026-09-27 at the project owner's request for a separate engineering
instance. This is an implementation brief, not a record of completed work.
Package IDs below are local to this brief; they do not renumber the roadmap.

The objective is to let researchers reconstruct dose over an irradiation,
quantify uncertainty in dose-volume and biological results, and determine
which measurements would improve those estimates. Extend the authoritative
Rust implementation and expose it through the existing product surfaces.

## Start here

Copy this instruction into the new instance, together with access to this
checkout or an isolated worktree:

> Read AGENTS.md and docs/RESEARCH_WORK_HANDOFF.md. Implement the available
> software slices in the order given below, starting with UQ-01. Another
> instance owns R17-04. Preserve its files and processes; do not change the
> transport/collapse implementation or its evidence. Inventory the current
> implementation before adding features. Work in small, reviewable slices
> with the specified scientific acceptance checks. Use the enforced local
> resource limits and coordinate the shared build/test/transport slot before
> executing jobs. Keep external-data milestones explicitly pending; complete
> independent software work while those inputs are unavailable. Do not
> contact institutions, publish, push, or operate treatment equipment as
> part of this handoff. Update this brief with exact changes, checks, and
> remaining dependencies as each slice finishes.

### Ownership and concurrency

R17-04, spectrum-adaptive group boundaries, is actively owned by another
instance. At handoff preparation the shared working tree had modifications
in the files below. This is a snapshot, not a complete future ownership list;
run `git status --short` again when starting.

- `crates/openbnct-cli/src/main.rs`
- `crates/openbnct-openmc/src/mgcollapse.rs`
- `crates/openbnct-openmc/src/photoncollapse.rs`
- `crates/openbnct-transport/src/cadis.rs`
- `crates/openbnct-transport/src/lib.rs`
- `crates/openbnct-transport/src/multigroup.rs`
- `crates/openbnct-transport/src/photon.rs`
- `validation/intercomparison-layered-head/compare-condensation.py`

Keep hands off those changes and the R17-04 study. Do not reset, stash,
reformat, or commit somebody else's work. Prefer an isolated worktree from
an agreed committed baseline; an independent worktree will not contain the
other instance's uncommitted fixes. Record that baseline. Do not copy a
moving working tree into your branch or assume it has those fixes.

Implement library code, focused tests, new examples, and documentation
first. Defer shared CLI dispatch edits until the R17-04 owner releases
`main.rs`, or prepare a small separate integration commit in an isolated
worktree. Treat `docs/ROADMAP.md`, crate export modules, Python bindings,
and GUI dispatch as shared integration points. Do not refactor them simply
to make the merge easier. This handoff's author owns the accompanying root
README edit; coordinate before editing that file again.

Do not run parallel agents or parallel local jobs by default. Independent
worktrees do not remove the workstation's shared resource constraint.

### Local execution rules

Read [AGENTS.md](../AGENTS.md) before any job. Only the coordinating instance
runs builds, executable tests, benchmarks, or transport, one job at a time.
With another session active, establish that the shared job slot is available
with the owner before execution; read/edit/document work can proceed while
waiting. Do not stop or reconfigure other sessions' processes.

All such jobs on this workstation must use:

```sh
systemd-run --user --scope \
  -p MemoryMax=6G -p MemorySwapMax=0 -p TasksMax=128 -p CPUQuota=200% \
  -- env CARGO_BUILD_JOBS=1 RUST_TEST_THREADS=1 RAYON_NUM_THREADS=2 \
  TMPDIR="/absolute/path/to/this-worktree/target/preflight-tmp" COMMAND ...
```

Create that disk-backed directory first. Keep each worktree's target
directory separate, and pass TMPDIR through every child process, including
NJOY and artifact-download tools. Never use RAM-backed `/tmp` for build or
transport scratch files.

Before the actual command starts, inspect the scope's cgroup from inside it.
For cgroup v2, `/proc/self/cgroup` identifies the directory under
`/sys/fs/cgroup`; read `memory.max`, `memory.swap.max`, `pids.max`, and
`cpu.max`. Require an effective memory cap of at most 6442450944 bytes,
zero swap, at most 128 tasks, and a finite CPU quota/period ratio at most 2.
Make the command conditional on that successful inspection. If limits cannot
be verified, stop execution and report the problem; never fall back to an
unlimited run. Do not probe limits by allocating memory.

Use bounded wall-clock budgets for new commands and test children. Review
process-spawning code before execution: timeout, cancellation, termination,
and reaping must be covered, including cancellation races. Never invoke a
Rust test's `current_exe()` without a reviewed, isolated child entry point.

### Project boundaries

- Rust under `crates/` owns models, validation, numerical methods, and
  reports. Python and GUI code call it; do not create a second engine.
- Version new interchange artifacts and bind their inputs with SHA-256.
  Reject incompatible geometry, normalization, units, model meaning, and
  stale input references. Unknown uncertainty is not zero uncertainty.
- Frozen evidence in `benchmarks/` and `validation/` is immutable. Read it
  as input. New runs write to a new named directory under the worktree's
  `target/`; selected new evidence may later be added at a new versioned
  path. Some old reproduction scripts overwrite committed files: inspect
  them and arrange new output locations before running them.
- Follow [IP_BOUNDARY.md](IP_BOUNDARY.md). General statistical reporting is
  in the documented public scope. Continuous admissible boron-map
  optimization, protected extremal-map/run-recomposition methods, and
  robust dose-bound certification require the recorded review described
  there before design or implementation.
- [R12_SCOPE_REVIEW.md](R12_SCOPE_REVIEW.md) has an owner-signed section 6
  approving its specific Avify connector scope, despite stale draft wording
  at its top. Do not request that existing approval again. It does not grant
  blanket clearance for new protected methods. This brief does not expand
  that scope; PLAN-01 below stops at evaluation where further review is
  unresolved.
- These packages deliver research software. They do not authorize patient
  decisions, treatment control, commissioning claims, or clinical deployment.
- Commits use the owner's author/committer identity and plain messages,
  without AI/tool attribution. Stage only your files. This handoff does not
  itself request a push or publication.

## Existing implementation: inspect and extend

Paths are relative to the repository root. Comments and roadmap status can
lag code; inspect implementations and their tests before making a claim.

| Area | Starting files | Existing behavior / extension point |
|---|---|---|
| Geometry, dose, metrics | `crates/openbnct-core/src/lib.rs`, `stats.rs`, `exposure.rs` | Grid and dose contracts, D95/EUD primitives, exposure accumulation and explicit normalization |
| Systematic uncertainty | `crates/openbnct-core/src/systematic.rs` | Boron, positioning, and relative-component sources; specified correlation assumptions |
| Plan uncertainty | `crates/openbnct-plan/src/robustness.rs` | First-order metric propagation; current cross-beam systematic independence is a documented limitation |
| Scenario evaluation | `crates/openbnct-plan/src/scenarios.rs` | Component/region scales, output scale, dose-field translation, worst-case weight optimization |
| Optimization | `crates/openbnct-plan/src/optimize.rs`, `lp.rs`, `selection.rs`, `synthesis.rs`, `fractions.rs` | Existing solvers, field selection, adjoint synthesis, and fraction handling; do not replace with a new optimizer by default |
| Nuclear-data UQ | `crates/openbnct-transport/src/uq.rs`, `screening.rs`; `crates/openbnct-openmc/src/` | Covariance propagation to scalar integrated responses; Morris/Sobol transport screening; consume existing contracts without changing R17-04 |
| Boron uptake | `crates/openbnct-boron/src/lib.rs`, `microdistribution.rs`, `measurement.rs` | PET/SUV mapping, uncertainty, tissue materialization, cellular microdistribution and measurements |
| Pharmacokinetics | `crates/openbnct-evidence/src/pk.rs` | Sample fitting, tissue:blood evolution, scheduling, integrated dose maps; map uncertainty currently unavailable |
| Biological models | `crates/openbnct-bio/src/lib.rs`, `isoeffective.rs`, `mkm.rs`, `cell_microdosimetry.rs`, `endpoint.rs`, `sweep.rs`, `compare.rs` | Fixed weights, nonlinear IsoE/MKM/SMK, parameter sweeps, endpoint evaluation, model comparison |
| Prompt gamma | `crates/openbnct-transport/src/prompt_gamma.rs` | Emission, detector response, expected counts, observations, regularized reconstruction |
| Reports and comparisons | `crates/openbnct-evidence/src/dvh.rs`, `metrics.rs`, `compare.rs`, `gamma.rs`, `analytic_oracle.rs`, `metamorphic.rs` | Reuse existing metric definitions and report conventions |
| Beam and measurement contracts | `crates/openbnct-transport/src/beam.rs`, `measurement.rs`; `crates/openbnct-core/src/registration.rs` | Beam identity/calibration context, measurement records, coordinate transforms |
| Public surfaces | `crates/openbnct-cli/src/main.rs`, `bindings/python/src/lib.rs`, `crates/openbnct-gui/src/lib.rs`, `io.rs`, `help.rs`, `i18n.rs` | CLI first after ownership coordination; thin Python API, then GUI report inspection |

Respect the crate dependency graph. `openbnct-core` must not depend on
plan/bio/evidence/transport. At this baseline `openbnct-evidence` depends on
core, while plan depends on core and bio. Keep shared input contracts in core,
low-level PK work in evidence, and orchestration in an appropriate upper
layer. Do not introduce a dependency cycle. A new orchestration crate is an
option only after documenting why the existing graph cannot host the work;
do not reorganize the workspace as the first task.

## Delivery order and completion states

| Order | Package | Can start without facility data? | Dependencies / completion boundary |
|---|---|---|---|
| 1 | UQ-01: joint uncertainty | Yes | Core statistical and correlation fixtures first; transport adapters wait for stable R17 interfaces |
| 2 | BENCH-01: benchmark catalogue and evaluation | Yes | Read frozen records immediately; new measured cases need data access and permission |
| 3 | BIO-01: biological-parameter evidence library | Yes | Literature curation and synthetic selection tests; expert review remains explicit |
| 4 | INTEROP-01: delivery-history import | Yes, with synthetic input | Define with REPLAY-01; real vendor support requires real documented exports |
| 5 | REPLAY-01: retrospective dose reconstruction | Yes | UQ core + normalized history; actual facility demonstration is a separate gate |
| 6 | VOI-01: measurement value analysis | Yes | UQ + explicit observation model; start with an analytic research example |
| 7 | BORON-01: measurement-informed 4-D estimation | Yes, on constrained synthetic cases | UQ, replay, detector calibration/observability; measured validation needs a partner |
| 8 | PLAN-01: stronger robust-planning evaluation | Yes | UQ + stable transport evidence; optimization extensions depend on applicable IP review |
| 9 | OUTCOMES-01: research outcome linkage | Yes, synthetic export only | Stable model/dose identifiers; institutional protocols and data agreements for real records |
| 10 | QUAL-01: qualification-readiness record | Yes, documentation only | Catalogue evidence and external requirements; no clinical qualification claim |

Advance one software slice at a time. Start with UQ source identity and a
two-field shared-error fixture, then propagate that identity through the PK
map. This does not require transport runs, a new optimizer, or GUI changes.

For each slice report four separate states: implementation, executed software
checks, independent numerical/measurement evidence, and external adoption.
An implemented schema is not an experimentally validated workflow. Record
missing partner data, licenses, or scientific review as dependencies and
continue independent work; do not invent data to mark them complete.

## UQ-01 — Joint uncertainty in dose and research endpoints

**Research result:** calculate distributions of D95, EUD, organ-dose metrics,
and compatible biological endpoints under declared joint input assumptions.
Retain which sources are shared across voxels, fields, and time.

### Software slices

1. Define a source-identity and joint-input contract, with physical units,
   scope (shared calibration, field, region, time), distribution or explicit
   weighted samples, support, and source evidence. Separate statistical
   sampling error, uncertain input parameters, structural model choice, and
   explicitly assessed transport/model discrepancy. An unassessed category
   must appear as unassessed rather than disappear from the report.
2. Add explicit correlated-systematic propagation alongside the legacy
   convention in `robustness.rs`. Preserve signed sensitivities before
   covariance folding. One assay calibration affecting two beams has one
   source identity. Do not infer independence simply because fields or
   artifacts have different IDs. Do not silently change old report meaning.
3. Propagate a declared PK-parameter/sample distribution through
   `pk_integrated_dose_bundle`. Preserve the common blood curve and any
   tissue-specific terms across all affected voxels and time intervals.
   Keep the point-estimate path available and label omitted terms.
4. Evaluate an ensemble using the existing dose, PK, biological, and metric
   functions. Calculate the requested metric on each realization before
   summarizing its distribution. Keep scenario ranges and probabilistic
   intervals distinct; do not assign probabilities to an unweighted list
   of named scenarios. Record the quantile estimator, sample weights, seed,
   model, units, and numerical/sampling convergence diagnostics.
5. Add adapters in stages: component/output scales and PK first; fixed-weight
   biology next; imaging/segmentation/material and nuclear-data perturbations
   only with explicit support and the corresponding transport treatment.
   A scalar integrated nuclear-data budget is not a voxel covariance field;
   do not attach it to every voxel as though it were one.
6. Add parameter sensitivity/attribution. Independent-input Sobol indices may
   reuse the existing methods within their assumptions. Correlated inputs
   require a documented conditional/grouped attribution method. Interactions
   and unresolved terms must remain visible; do not force percentages to
   sum to 100 by dropping them. Model-family comparisons are separate unless
   a justified common endpoint and model-probability construction are given.

Use a proposed new versioned joint-input/report contract; finalize names
after searching the schema catalogue. The CLI can eventually expose
`uq joint` and `uq joint-info` (proposed names, not existing commands).
Existing `uq apply`, `uq propagate`, and `plan robustness` remain useful.

### Scientific and numerical rules

- Support bounded/positive parameters with appropriate distributions; do not
  use unrestricted normals for concentrations and silently clip negatives.
  Specify covariance in physical or transformed parameter space, and verify
  that the requested dependence is valid. Fail on invalid covariance; do
  not silently repair it into a different scientific input.
- Enforce valid dimensions, finite values, symmetric positive-semidefinite
  covariance within a declared numerical tolerance, and bounded input sizes.
  Perfectly correlated/singular valid inputs need a defined supported path
  or a clear limitation, not arbitrary regularization.
- Existing MC voxel sigmas do not establish independent voxel errors. Use
  batch/covariance information where available or declare the approximation.
  Preserve reuse of the same MC estimate across fields/intervals; do not
  redraw it independently and make its error disappear.
- Biological model uncertainty is conditional on endpoint, tissue, drug,
  fractionation, and reference radiation. Do not pool unlike dose quantities
  into one interval. Keep an ensemble's missing model uncertainty explicit.
- Scaling boron dose assumes a fixed neutron field. Changes in boron can
  change absorption/self-shielding; bound the approximation's demonstrated
  domain or require fresh transport for that realization. This is an
  approximation policy, not a guarantee over all possible boron maps.
- Stream realizations and retain metric samples plus requested summaries;
  avoid storing an unbounded samples-by-voxels-by-components tensor.
  Preflight memory/work estimates, maximum samples, and transport-run budgets.

### Acceptance

- Analytic two-field fixture: doses 2 and 3 with a shared 10% multiplicative
  error yield total sigma 0.5; independent errors yield sqrt(0.2² + 0.3²).
  Splitting one field into two identical-history contributions must not
  reduce a shared source's uncertainty. Permuting inputs preserves results.
- Constant concentration reduces to the existing exposure result. An
  uncertain concentration amplitude has a hand-computable integrated-dose
  variance. Add a case with correlated amplitude/decay parameters.
- A small dose-map ensemble that changes voxel ordering demonstrates that
  computing D95 per realization differs from summarizing voxel intervals
  first. Compare against explicitly enumerated metric results.
- Known linear-Gaussian cases recover analytic intervals within predeclared
  sampling error; repeated synthetic trials check interval coverage with
  statistical tolerance. Coverage under a synthetic model is labeled as such.
- Invalid units/covariance, unresolved source IDs, inconsistent masks, stale
  hashes, empty samples, all-zero weights, and out-of-budget inputs fail
  clearly. Zero declared variance yields the point-estimate result.
- A report states which uncertainty categories were propagated, approximated,
  unavailable, or excluded. It never calls the interval complete patient
  uncertainty merely because all supplied inputs were processed.

## BENCH-01 — Discoverable, independently usable benchmark evidence

**Research result:** let a researcher select an appropriate case, inspect
exactly what it tests, and compare component predictions with its reference
without relying on a README's blanket PASS label.

### Software slices

1. Build a versioned catalogue that references existing immutable cases and
   reports without rewriting them. Include problem class, modality,
   geometry/material/source definitions, normalization, solver/data versions,
   reference type, measurement origin, uncertainty, license, tolerances,
   graded/excluded regions, and limitations. Record when tolerances were set;
   do not retroactively describe an unverified historical tolerance as
   preregistered.
2. Separate evidence types: analytic equation; published numerical reference;
   independent-engine comparison; measured experiment; internal closure;
   declared-input model study; parser/conformance fixture. Store observed
   failure and missing checks as first-class outcomes.
3. Add a read-only catalogue/info/report command before any execution runner.
   Reuse compare/gamma/analytic evaluators where appropriate. A common
   result envelope must retain each evaluator's original quantity, spatial
   domain, threshold, uncertainty assumptions, and detailed report.
4. Add a new synthetic delivery/uncertainty case once UQ and replay exist.
   Predeclare inputs and scoring before examining candidate results; include
   component doses, timing, known perturbations, and negative controls.
5. Prepare a measured-phantom contribution template and an external execution
   recipe. Ask for raw/reference measurements and permission before adding
   third-party data; do not digitize restricted content or invent licensing.
   A separate contributor/center owns the independent reproduction record.

Start by cataloguing NF-BNCT-001/002/003, the scenario optimizer oracle,
Reed/Azmy/Kobayashi, FiR 1 comparisons, the PK/scenario/cell-microdosimetry
studies, and prompt-gamma geometry. The root README's evidence table is a
discovery surface, not the authoritative scoring implementation.

### Acceptance

- Read-only catalogue generation leaves every existing evidence file byte
  unchanged. Broken references, absent licenses, and absent uncertainties
  are reported; they are not filled with defaults.
- Kobayashi's near-field result and severe deep-field discrepancies remain
  separately visible. A normalized FiR 1 shape comparison cannot appear as
  an absolute-dose pass. NF-BNCT-001 remains a candidate reference, and
  NF-BNCT-002 remains unexecuted until new execution evidence exists.
- Altered component units, source normalization, or grid alignment prevent
  comparison. Acceptance cannot be rescued by fitting an unexplained
  normalization to the reference. If fitting is part of the protocol, record
  it and distinguish fitted from held-out observations.
- A deliberately biased result fails the predeclared metric. A missing
  component or out-of-domain case produces incomplete/not-applicable, not
  PASS. Solver convergence alone never counts as physical agreement.
- An outside implementation can consume the catalogue and submit a report
  without using OpenBNCT as its dose engine. Claim community acceptance only
  after actual outside use and review; software completion alone is internal.

The 2025 [heterogeneous head-phantom study](https://doi.org/10.1016/j.apradiso.2025.111681)
is a candidate collaboration/reference source, not a dataset already licensed
to this repository. Prepare the data-request checklist; do not send outreach.

## BIO-01 — Biological-parameter evidence library

**Research result:** select and compare parameter sets appropriate to a
declared experimental context, and see when no matching evidence exists.

### Software slices

1. Define parameter evidence records in or beside `openbnct-bio`. Include
   drug/formulation and isotope convention; species/cell line; tissue and
   disease context; endpoint and assessment time; reference radiation;
   dose/rate/fractionation/repair assumptions; boron microdistribution;
   model/equation version; units; estimates; uncertainty type; joint
   parameter covariance when reported; and applicability limits.
2. Record exact source DOI/URL and table/equation location, extraction method,
   sample size where available, source population, author/reviewer status,
   and whether a value is measured, fitted, transferred, illustrative, or
   assumed. Distinguish SD, SE, confidence interval, population range, and
   unavailable uncertainty. Use explicit evidence categories with criteria,
   not an unexplained numeric quality score.
3. Seed a small, carefully checked BPA/BSH collection from sources already
   used by the repository. Separate the synthetic conformance constants from
   experimental parameter records. Preserve conflicting records and study
   dependencies rather than averaging everything into one default CBE.
4. Add search/info and explicit conversion to supported model documents.
   Context matching reports exact, partial, or unsupported applicability.
   Require a declared research assumption for any mismatch; do not silently
   transfer a mouse endpoint into a human parameter set.
5. Feed compatible uncertainty and parameter covariance to UQ-01 and expose
   comparisons through the existing biological evaluation/sweep functions.
   Expert-curated defaults are a separate future decision, not the default
   outcome of importing papers.

### Acceptance

- Each seeded non-synthetic value is checked against a primary source with
  units and location. Mark extraction review honestly; one AI extraction is
  not independent expert review. Commit metadata/facts permitted by the
  source license, not unauthorized full papers or tables.
- Tests distinguish two different tissues/endpoints for the same drug, reject
  incompatible model units, and preserve conflicting study results.
- Missing uncertainty stays missing. SD is not converted to SE without the
  required sample-size/design information; reported parameter correlations
  are not discarded when constructing joint samples.
- Existing model evaluation agrees with its independent equation fixture
  after a compatible evidence record is converted to a model document.
- CLI/Python results identify the selected record and all assumed transfers.
  The catalogue does not claim to determine the correct model for a patient.

## INTEROP-01 — Normalized retrospective delivery histories

**Research result:** consume recorded output, timing, position, and assay data
through a documented internal contract that preserves what was measured.
Define this package together with REPLAY-01 so it serves a working analysis.

### Software slices

1. Specify a versioned session/history contract with beam and calibration
   references, coordinate frame, observation times, units, acquisition
   intervals, quality flags, and source-record references. Distinguish
   instantaneous samples, interval-average rates, integrated counts/charge,
   cumulative counters, and explicit beam-state events.
2. Give each stream its clock basis, epoch, offset/drift mapping, timing
   uncertainty, and supported alignment rule. Keep acquisition/sample time
   separate from result-availability time, especially for blood assays.
   Specify interval edge conventions. Do not turn a missing segment into a
   zero-output interval or carry a stale position forward indefinitely.
3. Add a strict CSV/JSON reference importer using synthetic fixtures. Specify
   column names, delimiters, units, schema version, and mapping configuration.
   Keep raw records and conversion diagnostics available for inspection.
4. Define adapters around the normalized contract. A real adapter needs an
   actual format specification, a legally shareable sample or approved private
   test, calibration semantics, and independent review of the mapping.
   A made-up vendor fixture is labeled synthetic, not supported hardware.
5. Map supported standard information to existing DICOM/RT objects where it
   fits. Verify current standards before asserting a mapping. Keep additional
   BNCT research streams in an explicit companion contract; a new JSON file
   is not a DICOM standard or proof of vendor neutrality.

### Acceptance

- A synthetic export/import round trip preserves time, units, flags, IDs,
  counter resets, and coordinate transformations to declared precision.
- Duplicate timestamps, out-of-order samples, drift, gaps, counter rollover,
  ambiguous time zones, expired calibration, and mismatched frames have
  explicit tested policies. Unrecoverable ambiguity stops dependent replay.
- An independent hand calculation verifies rate/interval/counter conversion;
  no double integration of counts and no multiplication by duration twice.
- One real format is a supported adapter only after its data mapping passes
  the real-data gate. Cross-vendor capability requires at least two independently
  documented systems. Continue synthetic/core work while samples are missing.
- This is a read-only import workflow. No beam-control, treatment-record
  modification, automatic upload, or institutional messaging is included.

## REPLAY-01 — Retrospective accumulated-dose reconstruction

**Research result:** compare planned dose with estimated accumulated physical
and biological dose under a recorded irradiation history, with uncertainty
and the observation gaps visible.

### Software slices

1. Inventory `exposure.rs` and `pk.rs`, then accept a validated INTEROP history,
   per-beam physical inputs, concentration assumptions, and selected metrics.
   Establish geometry, source normalization, beam-model/calibration identity,
   and usable time coverage before calculation.
2. Begin with a single static field and a known concentration history. Handle
   explicit beam-on/off intervals, interruptions, and output variation.
   Normalize proton current/charge or monitor readings through a declared
   calibrated source-output mapping; current is not inherently neutron
   fluence, and target/beam changes can invalidate a calibration.
3. Integrate the boron contribution using both output and concentration over
   time; integrate non-boron components using the applicable output history.
   Use the current PK convention where valid. A product of average output
   and average concentration is generally not their time-integrated product.
   Retain the actual history needed for downstream biological calculations.
4. Extend to multiple supported fields through the existing exposure semantics.
   Reuse of transport results must stay within the recorded project scope;
   do not introduce protected run-recomposition methods. Propagate shared
   calibration, PK, and transport-estimate uncertainty through UQ-01.
5. Support position histories in explicit stages: stationary first, declared
   rigid approximations next, and independently calculated moved-geometry
   segments after transport interfaces stabilize. Dose resampling is not a
   transport re-solve. Deformable dose accumulation is out of the initial
   package and must not appear implicitly as nearest-grid remapping.
6. Evaluate biological effects only for supported schedules. The current
   IsoE repair factor assumes constant-rate protraction; do not apply it
   unchanged to arbitrary interrupted histories. Initially report physical
   dose and supported fixed-weight interpretations, and mark unsupported
   time-dependent biology unavailable. Add other models only with an
   independently checked effect/repair formulation. Do not simply sum
   separately inverted nonlinear biological dose maps.
7. Emit a reconstruction report: coverage interval, observed versus inferred
   inputs, component maps, planned/reconstructed metric deltas, uncertainty,
   model-specific biological results, calibration usage, and limitations.
   Proposed CLI names: `delivery inspect` and `delivery replay`.

### Acceptance

- Constant output and concentration reproduce the existing static result.
  A beam-off interval contributes exactly zero irradiation dose.
- A two-interval hand calculation with correlated output/concentration
  changes catches the product-of-averages error. Splitting an identical
  interval leaves accumulated dose and shared uncertainty unchanged.
- Exponential concentration at constant output matches the analytic PK
  integral. Time-unit conversion and a shifted common clock epoch preserve
  results; changing only relative stream timing changes them as expected.
- Gaps and unusable calibration/position segments are rejected or explicitly
  reported as incomplete; no report presents partial coverage as a complete
  delivered-dose estimate. Relative/per-source dose cannot be labeled Gy
  without the required normalization.
- Shared-batch transport uncertainty survives reuse across intervals without
  artificial averaging. The input histories and frozen dose bundles remain
  unchanged; outputs go to new paths.
- A new synthetic replay case checks component totals and metric differences
  against an independent analytic calculation. Later, a partner phantom run
  with recorded interruptions/output and independently measured dose checks
  the physical reconstruction. Code agreement alone does not close that gate.

The first real-data dependency is one usable facility export plus its output
calibration and a corresponding phantom experiment. Record exact requested
fields and measurement uncertainties. Prepare a request document; do not
send it or promise patient reconstruction while data are unavailable.

## VOI-01 — Which additional measurement would help most?

**Research result:** compare candidate research measurements by their expected
effect on uncertainty in a specified dose metric. This can guide experiment
design even before a facility supplies live observations.

### Software slices

1. Define a measurement proposal: quantity observed, sampling time/region,
   measurement model, random error, shared calibration bias, cost or duration,
   and the compatible UQ input/source IDs it constrains. Start with an extra
   blood assay versus an output-calibration measurement.
2. Define the utility explicitly, initially expected reduction in the variance
   of one compatible physical-dose metric. Compare candidate measurements
   using their possible outcomes under the declared model, not by replacing
   uncertain inputs with their true values. Separate expected information
   from the posterior update after a measurement is actually supplied.
3. Implement an analytic linear-Gaussian example before a general ensemble
   method. Record predictive assumptions, computational/sampling error, and
   any approximation in the expected utility. Present cost separately or
   state the exact cost-adjusted objective; do not invent a clinical utility.
4. Generalize to supported PK observations and later BORON-01 detector
   observations. Candidate assay times are research designs; this command
   does not schedule care, change a plan, or control equipment.

### Acceptance

- A scalar prior/measurement fixture matches posterior variance
  `(1 / prior_variance + 1 / measurement_variance)^(-1)`.
- A measurement independent of the target inputs gives zero expected
  information. As measurement noise grows, its expected benefit tends to
  zero. Exact duplicates of the same observation do not count twice.
- Repeated assays reduce independent measurement noise while retaining their
  shared calibration uncertainty. A perfectly known nuisance quantity cannot
  produce further uncertainty reduction.
- Candidate rankings on a small explicitly enumerated problem match exact
  expected utilities. Ensemble estimates carry uncertainty adequate to show
  when two candidates cannot be distinguished.
- Unknown observation precision produces an unavailable/assumption-required
  result, not an arbitrary default making that measurement look valuable.

## BORON-01 — Measurement-informed spatial and temporal boron estimation

**Research result:** update an explicitly constrained boron/capture estimate
from available PET, blood, kinetic, and prompt-gamma observations, while
showing which features remain unresolved by the measurements.

### Software slices

1. Write the inference assumptions before implementing updates. Define state,
   time origin, spatial basis/regions, observation equations, nonnegativity,
   prior information, parameter uncertainty, and units. Begin with a small
   number of tissue/region concentration parameters and existing PK curves.
   Rendering their values on a voxel grid does not create voxel-resolution
   observational evidence.
2. Distinguish therapeutic B-10 concentration from PET tracer uptake, total
   boron assays, reconstructed capture-emission rate, and accumulated boron
   dose. Record enrichment, density, assay units, and the explicit transfer
   assumptions needed to map between them. PET is a surrogate observation.
3. Extend the observation representation for time-binned detector data:
   detector/response/calibration identity, live time, efficiency, background,
   count units, and uncertainty. Use an appropriate likelihood for raw counts
   (normally Poisson with a stated background model); corrected or integrated
   measurements need their own statistical treatment. Existing normalized
   expected tallies are not automatically absolute detector counts.
4. Add observability diagnostics before allowing free parameter expansion.
   Prompt-gamma source strength depends on boron density and the neutron
   spectrum/fluence, followed by detector response. Do not jointly infer
   arbitrary fluence and boron maps from their product without independent
   constraints. Report unresolved directions and prior dependence.
5. Add sequential inference for the supported low-dimensional model using
   existing Rust physical/PK functions. Carry shared errors from UQ-01,
   including response calibration and neutron-field uncertainty. Do not use
   the same blood/PET observation twice through both a derived prior and an
   independent likelihood without accounting for that dependence.
6. Emit time-indexed state estimates, uncertainty, observation residuals,
   prior-versus-data influence, unsupported intervals, and the associated
   capture/dose predictions. Connect supported estimates to REPLAY-01 through
   the same concentration interface. Expand spatial freedom only when
   synthetic recovery and measured information justify it.

No continuous extremal boron-map search or dose-bound certification belongs
in this package. Statistical inference and observability are its scope;
check the existing IP boundary before proposing any additional methods.

### Acceptance

- Recover a one-region known concentration with a known neutron field from
  synthetic observations, with repeated-trial uncertainty coverage assessed
  under the same declared data-generating model.
- Two states giving identical observations remain indistinguishable; the
  tool reports that degeneracy instead of displaying a falsely precise map.
- Preserve the existing uncollimated/two-vial negative control. The example
  in `validation/pg-benedicte-geometry/` does not establish sub-centimeter
  imaging. New detector sampling/geometry must demonstrate any improvement
  on a new case and retain quadrature-resolution limitations.
- Avoid the inverse crime: include synthetic data generated with a different
  grid or independently implemented forward model, detector calibration
  perturbations, and unmodeled-background controls. Report degradation.
- Check low/zero counts, backgrounds, missing time bins, changing calibration,
  late-arriving assays, and concentration/fluence confounding. A retrospective
  smoother that uses future data must not be described as an online filter.
- Compare against PET-only and blood/PK-only estimates on held-out synthetic
  or partner measurements. Demonstrate useful predictive improvement and
  calibrated uncertainty, not only reduced residuals on fitted observations.

External completion needs a detector/BNCT partner's geometry, calibration,
raw or properly characterized processed observations, and independent
phantom truth. [i-TED pilot measurements](https://arxiv.org/abs/2409.05687)
are an existing research example; do not claim the field has no measurement
work or that this repository already reproduces that experiment.

## PLAN-01 — Stronger evaluation of robust research plans

**Research result:** determine whether a nominal or existing robust plan
remains acceptable under explicitly declared physical/biological assumptions,
including scenarios beyond those used to choose its weights.

### Software slices available now

1. Inventory `scenarios.rs`, `robustness.rs`, `optimize.rs`, `lp.rs`,
   `selection.rs`, and `fractions.rs`. Existing direction/weight/scenario
   machinery is the baseline. Coordinate metric/report and monitor-unit
   work with R17-05/06; do not create competing definitions.
2. Add UQ-01 evaluation of existing plan results. Preserve physical dose,
   fixed-weight dose, nonlinear IsoE/MKM/SMK evaluation, endpoint meaning,
   and fraction schedule as distinct quantities. In particular, the existing
   optimizer's `isoeffective` objective is a linear component-weight fold;
   it does not establish nonlinear biological inverse planning.
3. Compare nominal and robust weights on held-out scenarios/distributions.
   Report per-objective distributions, declared scenario extrema, failures,
   and nominal-quality tradeoffs. Do not describe performance on the same
   scenarios used for optimization as independent robustness validation.
4. Evaluate approximation error separately: dose shifts versus fresh rigid
   setup transport; fixed-flux boron scaling versus fresh material-bound
   transport; deterministic candidate results versus supported independent MC
   checks. Queue these runs after R17-04 and the shared workload slot permit.
5. State feasible machine/pose/aperture/timing inputs explicitly. An arbitrary
   mathematical direction or beamlet intensity map is not evidence that a
   facility can deliver it. Unsupported machine constraints remain unresolved.

### Scope-dependent follow-on

The requested longer-term capability is optimization with supported nonlinear
biology, setup, output, kinetics, and delivery constraints. Before detailed
design of any new optimizer/recomposition/bound method, record whether the
existing IP review covers it. If not, complete the evaluation slices above
and prepare the concrete research requirements and scope questions for the
owner. Do not implement protected methods in the MIT tree or presume that
the R12 connector approval covers them. No algorithm for those protected
extensions is authorized by this brief.

### Acceptance

- The existing analytic scenario fixture continues to satisfy its declared
  mathematical objective after any permitted evaluation changes.
- New held-out cases include one where robustness improves worst-case
  behavior and one where an omitted uncertainty invalidates that apparent
  improvement. Report both, including the nominal-dose tradeoff.
- A resampled shifted field and a fresh shifted-geometry solve are labeled
  differently; disagreement is retained and attributed, not hidden by a
  renamed "setup uncertainty" input.
- Each displayed result retains metric definition, biological model, unit,
  schedule, and normalization. Unsupported nonlinear objectives fail clearly
  rather than falling back to component weights.
- Nonconvergence/infeasibility and local-solver limitations remain visible.
  A research scenario optimum is not a globally certified dose bound.

## OUTCOMES-01 — Research exports for dose-to-outcome studies

**Research result:** make compatible dose, model, drug, and outcome records
available for institution-led retrospective analysis. The initial deliverable
is a local data dictionary and synthetic export/validation path.

### Software slices

1. Define the study's questions and linkage units: participant, lesion/ROI,
   treatment course, fraction/session, dose reconstruction, and follow-up.
   Retain calculated physical component maps or references to them, beam and
   drug protocol, blood/uptake observations, biological model/parameter
   identity, prior/concurrent treatments, and the relevant time basis.
2. Define outcomes with endpoint definitions and versions, assessment method,
   observation date/relative time, follow-up, censoring/competing events, and
   missingness reason. Toxicity grades and tumor-response categories from
   different systems cannot be silently combined. Keep planned and estimated
   reconstructed dose separately accessible.
3. Add local Rust export/validation using synthetic cases. Make identifiers
   stable within an approved study, and preserve component-dose/model meaning
   so a future model can be evaluated without replacing the historical result.
   Export only the configured fields; do not assume removing names makes
   medical images or exact dates anonymous.
4. Prepare an institutional integration brief covering study ownership,
   authorized data use, linkage/de-identification, schema mapping, retention,
   access, endpoint adjudication, and independent review. A central hosted
   registry versus institution-local/federated analysis is a partner decision;
   do not build either production service before that is resolved.
5. With authorized data and a prespecified scientific protocol, later studies
   can compare biological predictions using held-out patients/centers and
   temporal validation. Preserve study/center effects, selection bias,
   repeated-session dependence, and missing follow-up. Do not claim model
   calibration from training-set fit or fit synthetic outcomes as evidence
   about clinical biology.

### Acceptance

- A synthetic multi-lesion, multi-session case retains all linkage and
  chronology across export/import. A repeated session is not counted as an
  independent patient. Missing follow-up is not encoded as no toxicity.
- Model/endpoint incompatibility, stale dose references, impossible time
  sequences, and mismatched ROI definitions are detected.
- The export whitelist has negative fixtures containing unwanted identity
  fields, free text, and unsupported references. Report what was excluded;
  this is a software check, not a certification of anonymization.
- A study can re-evaluate a compatible new model while preserving the old
  model and result. The software gate uses synthetic records only; real
  outcomes and independent endpoint review remain external milestones.

Existing [nationwide BNCT post-marketing surveillance](https://pmc.ncbi.nlm.nih.gov/articles/PMC10931064/)
demonstrates that clinical outcome collection already occurs. The proposed
contribution is interoperable dose/model linkage, not inventing the first
BNCT outcomes registry.

## QUAL-01 — Research qualification-readiness record

**Research result:** give collaborators an accurate account of which claims
the software evidence supports and which external activities would be needed
for a different intended use. This package is documentation and evidence
organization, not a clinical TPS implementation.

### Deliverables

1. Create a claim/evidence matrix linked to BENCH-01. Separate software
   conformance, numerical verification, measurement comparison, independent
   external reproduction, and any future clinical/facility qualification.
   Include supported input domains, known failure modes, unresolved model
   assumptions, and the exact software/data versions evaluated.
2. Describe the current research workflow and user responsibilities using
   [DISCLAIMER.md](DISCLAIMER.md). Inventory release/regression records,
   dependency and data versions, reproducible build inputs, issue handling,
   and review records that already exist; list absent records honestly.
3. Prepare a separate future-program brief for the owner and qualified
   institutional collaborators: intended use, participating facility,
   measurement/calibration plan, software quality responsibilities, human
   review, resources, and the applicable jurisdiction/standards review.
   Verify current official sources before naming legal requirements; do not
   infer a device classification or certification from this roadmap.
4. Document how a new algorithm/data revision affects prior evidence without
   altering its frozen record. Passing old fixtures does not make a new
   beam model, facility, or biological endpoint qualified.

### Acceptance

- Every positive claim has an identifiable supporting record and a defined
  applicability domain. No planned/unexecuted activity appears as passed.
- A reviewer can distinguish software readiness from facility commissioning
  and clinical evidence without consulting internal implementation details.
- Existing public research qualification remains unchanged. This package
  does not alter treatment-device software, establish a clinical release,
  or prepare/send a regulatory submission.

## Surface integration and documentation

For each completed computational slice, implement a stable Rust API and a
small CLI workflow first. The proposed command names in this document are
placeholders until checked against current clap dispatch and ownership.
Document the exact command only once it is wired and exercised.

Python exports call the same Rust functions and return the same contract
semantics. The GUI should initially inspect a report and explain its metric,
units, assumptions, missing inputs, and result; avoid building a second
calculation path. Expose a feature on the web only when its dependencies and
resource budget support that environment. Background jobs must use the
reviewed cancellation/process infrastructure.

Update `docs/USAGE.md` with one working example per new workflow, and the
relevant crate README with its actual responsibilities. Once an integration
slice lands, update the root capability/evidence table and roadmap status
with the exact scope. Do not advertise a CLI as GUI-supported, synthetic
replay as a real facility demonstration, or a planned package as implemented.
Coordinate `README.ja.md` parity when capability descriptions are translated.

## Checks and review packet for each slice

These are instructions for the future implementation instance. No checks
listed here were executed merely by drafting this brief.

1. Inspect all process-spawning paths the new tests might reach. Unit fixtures
   should use small pure-Rust arrays and analytic answers wherever possible.
   Do not launch transport to test serialization or source-identity logic.
2. After confirming the shared workload slot and cgroup limits, run the
   affected library/integration test target with an explicit filter or target
   and a bounded wall time. Add scientific regression cases from the package's
   acceptance list; avoid tests that only reproduce the implementation.
3. In an isolated worktree, run `cargo fmt --all` and
   `cargo fmt --all -- --check`. Do not format another instance's dirty files
   in a shared tree. Run `cargo clippy --workspace --all-targets -- -D warnings`
   inside the enforced scope before any push. Respect the same resource
   limits for all builds, tests, benchmark jobs, and transport subprocesses.
4. Exercise an end-to-end CLI example after dispatch integration. Check Python
   or GUI integration only when changed; determine their required commands
   from the existing project tooling and keep execution bounded. Run one job
   at a time, and record exact commands, exit status, and skipped checks.
5. Do not regenerate committed benchmark/conformance answers to make failures
   disappear. New contract versions get explicit new fixtures; numerical
   regressions need explanation and new evidence, not edited old results.
6. Review the diff against the agreed baseline, including input normalization,
   scientific assumptions, dependency graph, memory/work limits, evidence
   immutability, and preservation of the other instance's changes.

If a longer numerical comparison is needed, first write a run plan: inputs,
engine/data version, grid/history count, estimated memory/runtime, enforced
limits, output directory, independent reference, and predeclared scoring.
Queue it with the owner. R17-04 results must be integrated from their own
recorded revision; this handoff does not rerun or take over that investigation.

For each completed slice, provide:

- changed files and public behavior;
- source revision and new artifact versions;
- assumptions, approximations, excluded uncertainties, and supported domain;
- executed checks/results and their commands, plus unexecuted checks;
- an example input/output a reviewer can inspect;
- external-data/review dependencies and the next independent slice.

Use this progress table rather than marking an entire scientific problem
solved after its first software slice:

| Package | Software slice completed | Executed checks | Independent evidence | External dependency / next step |
|---|---|---|---|---|
| UQ-01 | Pending | Not run | Pending | Start source identity + shared-error fixture |
| BENCH-01 | Pending | Not run | Existing records to catalogue | External contributors for community adoption |
| BIO-01 | Pending | Not run | Primary-source extraction required | Expert review of parameter applicability |
| INTEROP-01 | Pending | Not run | Synthetic reference format first | Real export specifications and calibrations |
| REPLAY-01 | Pending | Not run | Analytic replay first | Facility phantom logs and dose measurements |
| VOI-01 | Pending | Not run | Analytic observation model first | Evidence for real measurement error models |
| BORON-01 | Pending | Not run | Synthetic observability/recovery first | Detector partner and independent phantom truth |
| PLAN-01 | Pending | Not run | Existing optimizer fixture | R17 transport findings; scope review for extensions |
| OUTCOMES-01 | Pending | Not run | Synthetic linkage cases first | Institutional protocol, data, endpoint review |
| QUAL-01 | Pending | Not run | Evidence inventory first | Qualified partners for any later clinical program |

## Reference cautions for future literature work

- The matching DICOM discussion of log recalculation, stream synchronization,
  and machine characterization is in the
  [June 11–12, 2023 WG-07 Ion minutes](https://dicom.nema.org/Dicom/minutes/WG-07/WG-07-ION/Ion_2023/WG-07-Ion-2023-06-12-mtg-Mins.pdf),
  not a 2026 BNCT-specific standard. Check current official specifications
  when implementing interoperability; meeting notes are not a standard.
- [PET-derived heterogeneous boron calculation](https://www.nature.com/articles/s41598-023-42284-x)
  and [BNCT patient-position optimization](https://pubmed.ncbi.nlm.nih.gov/42623144/)
  are existing research. Novelty or superiority claims require a current,
  matched comparison rather than absence from this repository.
- Use primary studies for model equations, parameters, detector performance,
  and benchmark data. A review can identify literature but does not turn a
  proposed harmonization framework into consensus or supply missing biology.
- Public sources, data access, and publication licenses are different things.
  Keep factual metadata and attributed methods separate from reproducing
  protected full texts, images, or datasets.
