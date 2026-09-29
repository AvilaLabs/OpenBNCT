# ADR 0027: First bounded Python API

- Status: Accepted
- Date: 2026-09-11
- Decision owners: OpenBNCT maintainers
- Scope: `bindings/python` PyO3/maturin package under the ADR 0015 boundary

## Context

ADR 0015 selected the PyO3/maturin mixed-package architecture and deferred the
first bounded Python surface until the underlying Rust contracts were selected
and versioned. Those contracts now carry frozen schema versions, and the
remaining R2 response-qualification path is paused at its evidence gate, so the
distribution workstream is the open roadmap item that can advance without
weakening any scientific gate.

The risk the boundary exists to control is a second scientific authority: a
Python layer that re-implements validation, geometry transforms, or
qualification state could diverge from the Rust contracts.

## Decision

`bindings/python` builds a mixed package named `openbnct` whose compiled
`_openbnct` extension calls the same crates used by the CLI and GUI. The crate
is excluded from the Cargo workspace so that `cargo test --workspace` and
`cargo clippy --workspace` do not acquire a Python interpreter requirement;
it keeps its own lockfile, targets PyO3 `0.29.2` with the `extension-module`
feature, and builds through maturin.

The first surface is read-oriented and adds no transport capability:

- `generate_case`, `verify_case`, `load_case` — the deterministic `NF-BNCT-001`
  generator, the independent frozen oracle, and the gated loader;
- `Geometry`, `Structure`, `CaseVerification`, `VerifiedCase` — inspected
  geometry, ROI summaries, voxel centres, CT modality values, and masks;
- `read_manifest` — schema-validated `case.json` access plus
  `verify_artifacts`, which re-runs the Rust hash verification;
- `load_material`, `load_fixed_source`, `load_component_profile`,
  `load_response_generation_method`, `load_response_set` — deserialize with
  `deny_unknown_fields`, then run each contract's own `validate()`; no
  validation rule is reimplemented in Python or at the boundary;
- `ResponseSet.folding_ready` reports the Rust `validate_for_folding()` gate
  honestly rather than exposing a folding action;
- `backends` returns the same `BackendDescriptor` the CLI reports, including
  its still-disabled capability flags;
- `file_sha256` and `to_json()` expose the identical hash and canonical
  pretty-printed serialization produced by the Rust evidence path;
- every rejection surfaces as a single `OpenBnctError` (`NctForgeError` is kept as an alias of the
  same class).

The package ships `py.typed` and a checked `_openbnct.pyi` stub. Version
`0.1.0` matches the workspace; no PyPI release is claimed.

## Parity evidence

`bindings/python/tests` runs 12 cross-language checks against the built wheel:
generation, verification, and loading of the frozen case reproduce the Rust
oracle's exact shape, spacing, origin, ROI volumes, and centroids; corrupted
and absent artifacts fail with `OpenBnctError`; manifest artifact hashes equal
independently computed file digests; frozen material, source, profile, and
method contracts load with their declared identifiers; mutated and
unknown-field documents are rejected; an unreviewed response set loads but
reports `folding_ready == False`, a reviewed set without review evidence is
rejected, and a reviewed set with evidence reports ready; the OpenMC backend
advertises no unimplemented capability.

CI builds the wheel, installs it into a clean virtual environment, and runs
the parity suite there, so the tested artifact is the distributed form rather
than a development build.

## Consequences

- Scientific Python users can exercise the verified case and contracts without
  a Rust toolchain once wheels exist.
- The package cannot drift from the Rust contracts: every read path is a
  deserialize-then-`validate()` call into the same code.
- Still pending from ADR 0015: the supported wheel matrix and TestPyPI run,
  the crates.io surface review, and signed native desktop artifacts.
- A transport action in Python remains unavailable until the Rust capability
  and evidence gates pass — the binding inherits, rather than relaxes, the
  paused response-set state.

## Related decisions

- [ADR 0015: Python and native distribution](0015-python-and-native-distribution.md)
- [ADR 0014: Evidence-aware workbench shell](0014-evidence-aware-workbench-shell.md)
- [ADR 0026: O-17 processor energy-balance attribution](0026-o17-processor-energy-balance-attribution.md)
