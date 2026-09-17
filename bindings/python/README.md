# Python bindings

`pip install openbnct` is the planned primary entry point for scientific users.
The package uses PyO3 and maturin to wrap the authoritative Rust crates; it
does not implement a second dose, geometry, evidence, or QA engine (ADR 0015).

The mixed-package shape is:

```text
bindings/python/
  Cargo.toml                 PyO3 extension crate (outside the workspace)
  pyproject.toml             maturin build and package metadata
  src/lib.rs                 narrow Rust-to-Python boundary
  python/openbnct/
    __init__.py              ergonomic public API
    _openbnct.pyi            checked extension types
    py.typed                 typing marker
  tests/                     cross-language parity suite
```

The API covers case generation, verification, and gated loading for
`NF-BNCT-001`; geometry, ROI, and CT inspection; `case.json` manifest
reading and artifact re-verification; validated material, source, component
profile, response-generation method, and response-set contract readers with
canonical `to_json` serialization; the response-set `folding_ready` review
gate; statepoint collection; DVH and dose-volume metrics; biological-model
application and TCP/NTCP/UTCP endpoint evaluation; exposure-plan table
import/export and weighted accumulation; and external component-dose import
(`import_component_dose`, `import_mcnp_meshtal`, `import_phits`) plus MCNP
deck export (`export_mcnp_deck`); and external-dose/BED combined analysis
(`import_external_dose`, `bed_from_external_dose`,
`combine_biological_doses`); source positioning (`aim_source`,
`rotate_source`, `PositionReport`); and cross-code dose comparison
(`compare_dose_bundles`, `DoseComparison`); and gamma-index evaluation
(`evaluate_gamma`, `GammaEvaluation`); and biological-model
sensitivity sweeps (`sweep_biological_model`, `SensitivitySweep`). Every load
runs the same Rust `validate()` as the CLI, every rejection raises
`NctForgeError`, and adapter provenance binds the generated interchange
document's SHA-256 exactly as the CLI does. Transport actions stay
unavailable until the Rust capability and evidence gates pass;
`backends()` reports those flags honestly.

Local development:

```text
python3 -m venv .venv
.venv/bin/pip install 'maturin>=1.7,<2'
.venv/bin/maturin develop
.venv/bin/python -m unittest discover -s tests -p 'test_*.py'
```

`maturin develop` builds the extension in place; `maturin build` produces a
wheel under `target/wheels`. The extension targets the CPython stable ABI
(`abi3-py310`), so one wheel per platform covers every supported interpreter
(`cp310-abi3-*`). CI builds the wheel, installs it into a clean
virtual environment, and runs the parity suite there; the same build +
clean-venv parity run has been verified locally on CPython 3.14.
Publication of the platform matrix through TestPyPI remains a pending
release gate — see [ADR 0015](../../docs/adr/0015-python-and-native-distribution.md)
and [ADR 0027](../../docs/adr/0027-first-bounded-python-api.md).
