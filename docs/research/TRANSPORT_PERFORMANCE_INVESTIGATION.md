# Transport Performance Investigation

**Status:** Open investigation protocol; no bottleneck has been measured and no
engine decision has been accepted.

**Date:** 2026-08-31

## 1. Purpose

This document specifies how OpenBNCT will determine:

1. whether transport materially limits end-to-end time to a scientifically
   acceptable BNCT result;
2. which part of that path dominates;
3. how much of any measured gap can be closed with the accepted OpenMC backend;
   and
4. whether the remaining gap is large enough to justify considering another
   backend.

The question is not whether OpenMC can be made to run faster in isolation. The
question is whether the complete OpenBNCT path meets a predeclared time and
statistical-quality target on declared hardware.

At present, "transport is the bottleneck" is an untested hypothesis. This
document is an investigation protocol, not a decision record. Its final output
is an ADR supported by measurement artifacts.

## 2. Current implementation and readiness boundary

The performance investigation has two distinct scopes that must not be
conflated.

### 2.1 Authoritative OpenBNCT measurement

The current deterministic OpenMC deck generator:

- requires a content-bound, independently reviewed `NeutronResponseSet`;
- validates that response before generating a deck;
- emits three response-weighted neutron component tallies using OpenMC
  energy-function filters; and
- emits the heating, reaction, fluence, and leakage audit tallies in the same
  deck.

The response curves are therefore used while OpenMC scores the component
tallies. They are not solely a post-processing input. Energy-binned diagnostic
fluence can support an independent later fold, but that alternative does not
make the current production deck response-free.

In addition, the `OpenMcBackend` capability flags for controlled preparation,
execution, and statepoint collection remain false. The authoritative
end-to-end benchmark cannot run until all of the following exist:

1. a reviewed response set accepted through the R2 gates;
2. a controlled OpenMC smoke execution; and
3. a statepoint importer that verifies identities, tally definitions, shapes,
   units, histories, and batch structure.

These are implementation prerequisites for measuring the current OpenBNCT
path. They do not prove that raw OpenMC transport is physically dependent on
the response-generation blocker.

### 2.2 Optional preliminary OpenMC microbenchmark

A response-free OpenMC deck containing only non-response tallies can be timed
before the R2 blocker is resolved, but it is a separate microbenchmark. The
current production generator cannot emit that deck.

If such a microbenchmark is useful, it must:

- use a separate, versioned benchmark profile and manifest;
- bind the exact geometry, material, source, nuclear data, OpenMC build,
  settings, and active tallies;
- state prominently that response-weighted tally cost, OpenBNCT execution, and
  statepoint import are absent;
- never manufacture a placeholder "reviewed" response or weaken production
  validation to pass the generator; and
- never be reported as end-to-end OpenBNCT performance.

It can validate the timing harness and establish a rough transport scale. It
cannot justify an engine decision.

## 3. Non-negotiable investigation rules

An implementer working from this document must not:

1. write a particle-transport engine, in whole or in part, before the measured
   decision gate in Phase 3;
2. fork, vendor, or patch OpenMC as an unrecorded benchmark shortcut;
3. mutate an accepted scientific contract, frozen baseline, or artifact and
   then present it under the original identity;
4. manufacture qualified nuclear-data or response evidence;
5. downsample the accepted pointwise response grid merely to improve a timing
   number;
6. disable overflow checks, omit required validation, or relax a fail-closed
   gate;
7. report a speedup without a predeclared statistical target, figure of merit,
   timing scope, and uncertainty on the comparison; or
8. call a stochastic change in an observed sample mean a physics change without
   applying the statistical comparison protocol in section 7.

An experimental setting may differ from the baseline only under a new,
content-bound profile. The baseline remains immutable. A response-compression
method, new execution mode, changed tally ledger, weight-window configuration,
or biased-source law is a versioned scientific or execution change, not an
in-place optimization.

Phases are sequential. Phase N+1 does not begin until Phase N's exit evidence
exists.

## 4. Define performance and precision before measuring

### 4.1 Two performance views

Every authoritative run reports both:

- **OpenMC-process time**: monotonic wall time from controlled process spawn to
  exit; and
- **end-to-end time**: validation, deck generation, OpenMC execution,
  statepoint collection, normalization, hashing, and evidence emission.

These answer different questions. A transport optimization can improve the
first while leaving user-visible latency nearly unchanged.

For each predeclared scalar response `j`, report:

```text
FOM_j(scope) = 1 / (R_j^2 * T_scope)
```

where `R_j` is the estimated relative standard uncertainty and `T_scope` is
either OpenMC-process or end-to-end wall time. A scalar response may be a named
component in a named voxel or a directly tallied region response. A region
uncertainty must not be reconstructed by assuming mesh voxels from shared
histories are independent.

Relative uncertainty is unsuitable when the expected response is zero or near
zero. Such quantities use a predeclared absolute-uncertainty target and are not
forced into the relative-error FOM.

FOM is expected to be approximately stable with history count only after fixed
overheads are small and the estimator has reached its asymptotic sampling
regime. Verify that behavior at multiple history counts; do not declare it by
assumption.

### 4.2 Statistical target

Before the first authoritative run, freeze:

- the component-and-region scalar responses used for FOM;
- a field-quality rule for every clinically or scientifically relevant
  component, such as a declared percentile of voxel relative uncertainty above
  a declared response threshold;
- handling of zero-score and low-response voxels;
- the minimum number of active statistical batches and diagnostics used to
  judge whether reported uncertainties are reliable;
- the target end-to-end time and the hardware class on which it applies; and
- the acceptance method for comparing stochastic results, including treatment
  of multiple voxels and multiple components.

A single convenient ROI and a single component are not enough to claim that a
four-component dose field has converged.

### 4.3 Frozen benchmark identity

Every measurement artifact records:

- case identifier and all content hashes;
- grid shape, spacing, orientation, and scored regions;
- source, material, response-set, nuclear-data, execution-profile, and tally
  identities;
- OpenMC version, commit, compiler, build type, and relevant build flags;
- OpenBNCT commit and Rust toolchain;
- OS, kernel, CPU model and topology, memory, thread count, MPI ranks, and
  affinity;
- whether the host was otherwise idle and any relevant power-management state;
- histories, batches, seeds, and stride;
- active and absent tallies;
- peak resident memory and output sizes; and
- raw stage times, statistical diagnostics, derived FOM values, and the method
  used to summarize repeats.

## 5. Hardware and resource constraints

The first development machine has approximately 30 GB of usable memory. A
previous unrelated process was terminated after allocating approximately
27 GB. This is a benchmark-environment constraint, not a universal OpenBNCT
limit.

- Set and record an explicit memory bound.
- Do not run concurrent heavy jobs during a timing trial.
- Start with bounded smoke sizes and increase only after recording peak memory.
- Prefer several reproducible runs to one machine-threatening run.
- Treat an out-of-memory condition as a measured result.
- Do not silently shrink the grid, tallies, or physics to fit the machine.

## 6. Investigation phases

### Phase 0 — Benchmark contract and measurement harness

No optimization occurs in this phase.

The harness may be developed before R2 is complete. An optional preliminary
microbenchmark may exercise it under section 2.2, but cannot satisfy the
authoritative baseline gate.

Exit evidence:

- a versioned benchmark specification and timing-artifact schema;
- a re-runnable command that binds a named case, build, profile, and host;
- monotonic timing around each externally observable stage;
- raw stdout, stderr, exit status, OpenMC timing output, peak memory, and output
  file sizes retained or content-bound;
- repeated identical trials establishing the timing noise floor; and
- runs at multiple history counts demonstrating whether the chosen FOM has
  entered an approximately stable regime.

A claimed improvement must exceed the measured noise floor and include an
uncertainty interval or other predeclared repeat-run comparison.

### Phase 1 — Authoritative baseline and wall-clock decomposition

This phase begins only after the prerequisites in section 2.1 pass. It uses the
unaltered accepted OpenBNCT case and complete tally ledger.

Measure directly:

- input validation, artifact verification, and deck generation;
- OpenMC process wall time;
- statepoint opening, identity and tally verification, import, and
  normalization;
- output hashing, manifest generation, and final validation; and
- total end-to-end time.

Within OpenMC, particle tracking and tally scoring are interleaved. "Transport"
and "tally accumulation" must not be presented as independently observed
timings unless the pinned OpenMC build exposes suitable internal measurements.
The incremental cost of tallies may instead be estimated with matched,
versioned tally variants. Statepoint writing and other finalization costs may
be estimated from trustworthy internal timers or controlled differential runs.
Such estimates are labeled as estimates, and their components are not required
to sum exactly to process wall time.

Exit evidence:

- repeat-run distributions for every stage and for total wall time;
- component-and-region FOM values at multiple history counts;
- a stated residual for any stage decomposition;
- the dominant stage and its fraction of end-to-end time; and
- a numerical gap between measured performance and the predeclared target.

If OpenMC process time is not dominant, or the target is already met, the
transport investigation terminates and the result is recorded.

### Phase 2 — Measure the accepted-backend performance ceiling

This phase begins only if Phase 1 identifies OpenMC process time as a material
bottleneck.

BNCT's fixed-source geometry and localized responses make variance reduction
worth testing, not presumptively effective. Deep penetration, thermalization,
secondary photons, multiple dose components, and whole-field requirements may
make a single importance strategy ineffective.

Create immutable, content-bound variants and change one material factor at a
time. Candidate studies include:

- OpenMP and MPI scaling, including load balance and memory scaling;
- supported OpenMC execution modes and build options;
- the measured incremental cost of individual diagnostic and component
  tallies;
- mesh- and energy-dependent weight windows; and
- source biasing within the physical source's support, with the exact analog
  and biased distributions plus weight correction recorded.

The frozen `NF-BNCT-001` source already samples only inside its beam aperture.
"Biasing toward the aperture" is therefore not a meaningful experiment for
that case. Any useful source-biasing proposal must identify a different
response-relevant part of source phase space and preserve the analog
expectation. A biased distribution must have compatible support; source
rejection that changes the physical source is not variance reduction.

Weight-window generation cost is part of the accounting. Report both
application-only FOM and total time including generation, with any amortization
assumption stated. Also record long-history behavior, splitting, roulette,
particle populations, and memory. Standard fair-game variance reduction should
preserve expected values, but incorrect source support, weight correction,
cutoffs, population controls, or implementation can introduce bias.

Exit evidence:

- FOM and end-to-end latency for each variant versus the immutable baseline;
- performance effects reported across all predeclared components and regions,
  not only the response favored by the optimization;
- correctness evidence from section 7;
- generation and amortization costs for reusable artifacts such as weight
  windows; and
- the remaining measured gap to the target.

If the target is met without compromising a correctness gate, the
investigation terminates and no new engine is justified by performance.

### Phase 3 — Decide whether another backend deserves study

This phase begins only if Phases 1 and 2 leave a quantified, material gap. It
produces an ADR and literature review, not an implementation.

| Option | Question to answer |
|---|---|
| Upstream OpenMC contribution | Is the measured cost caused by a generally useful OpenMC behavior that can be improved and accepted upstream? |
| Additional specialized CPU Monte Carlo backend | Does a constrained fixed-source voxel scope remove enough measured work to justify a new validation burden? |
| Additional specialized GPU Monte Carlo backend | Does the measured workload map well to available hardware without sacrificing the required continuous-energy neutron, photon, and statistical behavior? |
| Additional deterministic backend | Can a separately versioned multigroup or other deterministic method meet quantified error bounds and provide useful methodological independence? |
| General-purpose Monte Carlo engine from scratch | Out of scope unless a future ADR demonstrates a need not met by the narrower options. |

No speedup from proton, photon, or other transport software is evidence of a
BNCT neutron-photon speedup without a workload-matched benchmark. Claims that
no comparable BNCT code exists require a documented literature search.

ADR 0007 freezes pointwise partial-KERMA response generation for the accepted
OpenMC path. It does not, by itself, prohibit an additional multigroup backend.
Such a backend would require its own cross-section preparation, response
collapse, approximation-error, provenance, and validation contracts.

Specialization does not eliminate:

- continuous-energy or explicitly bounded multigroup nuclear-data treatment;
- resonance and thermal-scattering treatment appropriate to the intended
  material scope;
- evaluated secondary-photon production and photon transport;
- unbiased source sampling and population control;
- reproducible parallel random-number behavior;
- geometry and material verification; or
- independent validation against accepted references.

The number of nuclides alone is not a defensible estimate of engine complexity.

## 7. Correctness and statistical-comparison gates

A performance change may alter the realized stochastic sample. It must not
silently alter the target expectation, normalization, scientific definition, or
accepted uncertainty behavior.

### 7.1 Establish a baseline reference estimate

Run the accepted baseline at sufficient histories and batches to make its
uncertainty small relative to the comparison tolerance. Freeze the inputs and
result by content hash as the **baseline reference estimate**, not as truth.

OpenMC and the comparison configuration are normally run with independent
seeds, in which case their variances are combined. If common random numbers or
overlapping histories are deliberately used, the comparison must estimate and
include covariance; it must not apply an independence formula.

Thousands of voxel comparisons cannot require every unbiased result to land
inside an unadjusted two-sigma interval. The acceptance protocol must be
declared before results are seen and must include:

- multiplicity-aware scalar comparisons;
- a global field test or calibrated distribution of standardized residuals;
- checks for spatially coherent and component-wide offsets; and
- diagnostics establishing that the underlying uncertainty estimates are
  credible.

The baseline is internal regression evidence. It does not replace the
independent estimator evidence required by R2.

### 7.2 Before a performance variant

Record:

- `cargo test --workspace --all-targets` passing;
- `cargo fmt --all -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean;
- the external DICOM validation gate passing on a freshly generated case;
- byte-level hashes of the accepted OpenMC deck and all bound inputs;
- the baseline result, its batch count, per-quantity uncertainties, and
  uncertainty diagnostics;
- the benchmark host and build identity; and
- the Phase 0 timing noise floor.

### 7.3 After a performance variant

All of the following must hold:

1. **Identity is honest.** An implementation-only optimization emits the
   byte-identical accepted deck. A deliberate settings, source, tally, or
   variance-reduction change emits a reviewed new profile and manifest with a
   new identity.
2. **Scalar responses remain statistically compatible.** Predeclared
   component-and-region comparisons pass their multiplicity-aware limits.
3. **The field shows no aggregate bias.** The predeclared global and spatial
   residual checks show no systematic displacement.
4. **Contracts validate.** Physical-dose, response-set, run-manifest, and
   artifact verification pass.
5. **The complete suite remains green.** Every applicable check from section
   7.2 passes.
6. **The improvement is distinguishable.** The FOM or latency improvement
   clears the noise floor and its predeclared repeat-run comparison.
7. **Costs are complete.** Setup, reusable-artifact generation, memory, and
   failure or long-history behavior are reported rather than omitted.

### 7.4 Test coverage required before optimization

Add fail-closed tests for:

- a statepoint whose tally definitions do not match the input manifest;
- a statepoint whose history count or batch structure differs from the
  requested settings;
- an unrecorded weight-window or source-biasing specification;
- a comparison against a baseline with a different case, build, nuclear-data,
  response-set, or tally identity;
- a timing artifact with missing stages, inconsistent totals, or a failed
  process presented as a successful run; and
- statistical-comparison fixtures that exercise expected random excursions,
  multiplicity handling, aggregate bias, and incompatible seed/covariance
  assumptions.

## 8. Strategic constraint on a homegrown engine

OpenBNCT's credibility rests partly on independent verification against a
widely reviewed transport code. An engine developed inside this project cannot
serve as its own independent authority.

If another engine is eventually built, it is an additional backend, not a
silent replacement for OpenMC. Its first obligation is agreement against
frozen cases and independent evidence under declared tolerances. OpenMC remains
part of the validation ladder even if another backend becomes the fast
interactive path.

The `TransportBackend` boundary permits that later choice without making it
today. Cases accumulated before then become useful validation cases only after
their inputs and expected evidence are independently qualified.

## 9. Exit evidence

This investigation closes with an ADR containing:

- the frozen performance and statistical-quality targets;
- the authoritative Phase 1 stage measurements and identified bottleneck;
- Phase 2 performance and correctness results for every evaluated variant;
- the measured remaining gap, including uncertainty;
- the literature and feasibility evidence for any Phase 3 option; and
- a decision tied directly to the measured gap.

Until that ADR exists, "transport is the bottleneck" remains an unqualified
hypothesis.

## 10. Primary references

- [ADR 0005: OpenMC 0.16 estimator boundary](../adr/0005-openmc-016-estimator-boundary.md)
- [ADR 0007: Partial-KERMA response generation](../adr/0007-partial-kerma-response-generation.md)
- [ADR 0009: Deterministic OpenMC input deck](../adr/0009-deterministic-openmc-input-deck.md)
- [OpenBNCT roadmap](../ROADMAP.md)
- [OpenMC tally methods and statistics](https://docs.openmc.org/en/stable/methods/tallies.html)
- [OpenMC variance-reduction guide](https://docs.openmc.org/en/stable/usersguide/variance_reduction.html)
- [OpenMC 0.16.0 release notes](https://docs.openmc.org/en/stable/releasenotes/0.16.0.html)
- [OpenMC energy-function filter](https://docs.openmc.org/en/stable/pythonapi/generated/openmc.EnergyFunctionFilter.html)
