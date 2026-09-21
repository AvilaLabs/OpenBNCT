# Architecture

**Status:** Initial direction, 2026-08-31

## Objective

Build a neutral BNCT research and verification platform whose calculation
inputs, physical-dose components, biological assumptions, uncertainty, and
provenance remain independently inspectable.

## One authoritative Rust model

The Rust domain model is authoritative for production interchange, validation,
QA, and evidence generation. GUI, CLI, Python, and future hosted interfaces must
invoke the same implementation rather than reproduce scientific logic.

## Backend neutrality

The application must not pass OpenMC-specific objects beyond
`openbnct-openmc`. Transport adapters consume a `TransportCase` and produce a
`PhysicalDoseBundle`. Backends may prepare and execute a calculation, import an
external result, or both. Each normalized bundle binds both its component
semantics and its material- and nuclear-data-specific neutron response set by
content hash.

Initial adapters are expected to be:

1. analytic synthetic benchmarks;
2. OpenMC preparation, controlled execution, and statepoint collection;
3. generic component-dose import;
4. MCNP and PHITS import when licensing and validation permit.

No adapter may advertise a capability before a committed acceptance case
demonstrates it.

## Scientific layers

1. **Geometry:** patient coordinate frame, voxel affine, structures, materials.
2. **Transport:** neutron and photon histories and reaction estimators.
3. **Physical dose:** macroscopic boron (`D_B`), nitrogen (`D_N`), conventional
   hydrogen/neutron (`D_H`), and photon (`D_gamma`) components under a named
   semantic profile. Contributor reactions remain inspectable.
4. **Boron model:** measured or assumed concentration and spatial distribution.
5. **Biology:** separately versioned CBE, RBE, or isoeffective interpretation.
6. **QA:** DVH, gamma, sensitivities, cross-code and measurement comparisons.
7. **Evidence:** immutable manifest binding every consequential artifact.

A derived biological dose must retain references to the exact physical bundle,
boron model, biological model, and parameter set used to produce it.

## DICOM geometry boundary

`openbnct-dicom` is the sole DICOM-to-domain boundary. It uses a pinned
`dicom-rs` release for Part 10 parsing, then applies OpenBNCT's own fail-closed
semantic checks. CT order comes from projected Image Position (Patient), not
file order or Instance Number. The core grid uses `[column, row, slice]`, stores
the first voxel centre as its origin, and retains a right-handed orthonormal LPS
direction matrix.

R1 deliberately supports only native signed 16-bit Explicit VR Little Endian CT
with a uniform, unsheared stack and one `CLOSED_PLANAR` polygon per ROI per image
plane. Compressed pixels, enhanced multiframe CT, tilted stacks, and ambiguous
multi-polygon topology fail explicitly until separately specified and tested.
See `docs/adr/0003-dicom-geometry-boundary.md`.

The initial component semantics are fixed by
`docs/adr/0002-macroscopic-dose-semantics.md`. In particular, `D_H` is not
limited to H-1 elastic scattering when that would leave neutron energy
unclassified. Physical-total uncertainty must not assume that component tallies
from shared particle histories are independent.

OpenMC-specific estimator behavior remains inside `openbnct-openmc`. In the
0.16.0 adapter, reaction-filtered neutron heating is diagnostic only because it
does not expose reaction-wise KERMA. Reported component definitions and their
hashed response ledger stay backend-neutral. See
`docs/adr/0005-openmc-016-estimator-boundary.md`. A response set cannot be
folded into reported dose until its table is internally valid and its
independent review artifact is content-bound.

## GUI process model

egui/eframe is the initial native interface. The GUI thread must never execute
transport. It submits immutable jobs to a worker process or remote executor and
observes structured progress events. A calculation crash must not corrupt the
open case or terminate the viewer.

The R1 GUI opens only a manifest- and artifact-verified `NF-BNCT-001` directory.
It contains linked axial, sagittal, and coronal views, window/level controls, RT
structure overlays, an LPS cursor readout, and visible transport-capability
state. Component-dose selection arrives with qualified component data in R2;
contour editing is explicitly deferred.

Anatomical mapping lives in `openbnct-view`, not egui callbacks. R1 labels views
as axial, coronal, or sagittal only when grid direction is aligned to canonical
DICOM LPS axes. The viewer rejects oblique or permuted grids until their
resampling and labeling conventions have dedicated tests. Screen-edge mappings
and click-to-voxel round trips are tested independently for all three planes.
See `docs/adr/0004-anatomical-view-conventions.md`.

## Data handling

Patient data is denied by default during early development. The repository
ignores common medical-image and transport-output formats. Only deliberately
reviewed synthetic artifacts may enter the public benchmark corpus.

Large external nuclear-data artifacts are never implicit build dependencies.
Reviewed acquisition profiles constrain their publisher URI, HTTPS redirects,
size, and digest evidence. The Rust acquisition path emits a receipt, and the
case-scoped OpenMC manifest binds both profile and receipt by SHA-256 before any
table is accepted. This establishes transfer provenance without implying
scientific or clinical qualification; see ADR 0010.

## Qualification

Every result declares one of the bounded qualification states defined by
`openbnct-evidence`. The software must never infer clinical fitness from a
successful calculation or benchmark.

The R1 case manifest binds its coordinate system, grid, DICOM identifiers,
structure truth values, material/source model identifiers, and every CT or
RTSTRUCT input by relative path and SHA-256. Artifact verification rejects path
traversal, symlink escape from the case root, missing files, and hash changes.
The manifest intentionally does not hash itself; archival releases bind the
manifest from a higher-level release or run manifest.

R1 also generates the case afresh in CI and runs `dciodvfy` on every instance
and `dcentvfy` across the complete CT/RT Structure Set collection. The validator
snapshot and runner image are pinned, and warnings fail the gate. This is an
independent mechanical check of DICOM PS3 IOD, encoding, dictionary, and entity
consistency rules; it is not a certification claim.
## Contract namespaces and the NCTForge rename

The project was renamed from NCTForge to OpenBNCT after the R6 roadmap
completed; crates, the CLI binary, and the Python package are now `openbnct`.
Contract identifiers are versioned protocol strings, not branding: artifacts
written before the rename carry `nctforge.*` schema ids (and `nctforge-*`
tool/method ids), and they remain valid.

Readers must normalize the legacy namespace on load —
`openbnct_core::normalize_contract_id` maps `nctforge.` → `openbnct.` and
`nctforge-` → `openbnct-` — rather than rejecting old ids or rewriting frozen
files. Every `schema_version` field applies the normalization through serde, so
in-memory values are always canonical `openbnct.*`; byte-level evidence such as
frozen benchmark reports and hash-bound manifests keeps its original ids
intact, and content hashes of those files therefore remain valid.

Two legacy names are frozen forever, not aliased: the DICOM `2.25.*` UIDs of
the `nf-bnct-001` synthetic case derive from `nctforge.org` UUIDv5 name strings
published in the benchmark specification, and changing the seed would invalidate
every published UID.
