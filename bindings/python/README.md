# Python bindings

`pip install openbnct` installs the published scientific package. For the current
workflow, start with the [Python handbook chapter](https://openbnct.avilalabs.org/docs/python.html).
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
sensitivity sweeps (`sweep_biological_model`, `SensitivitySweep`); and
validated readers for the deterministic-transport and evidence artifact
family — multigroup data/flux/covariance, dose-uncertainty budgets,
sensitivity specs and screening reports, resolved weight windows,
measurement records and comparison reports, beam descriptions and
beam-quality reports, accelerator sources, beam-shaping assemblies and
sweeps, lineal-energy spectra and tally specs, metamorphic and analytic
oracle evaluations, boron microdistribution models and corrections,
RTPLAN summaries (`load_rtplan_summary`, `summarize_rtplan`), and
component-NIfTI export manifests. Every load
runs the same Rust `validate()` as the CLI, every rejection raises
`OpenBnctError` (`NctForgeError` remains as an alias for the very same
class), and adapter provenance binds the generated interchange
document's SHA-256 exactly as the CLI does. Monte Carlo transport actions
stay unavailable until the Rust capability and evidence gates pass;
`backends()` reports those flags honestly.

## NumPy arrays and axis order

`numpy` is a package dependency. Every voxel field (`DoseVolume`,
`ExternalDoseBundle`, `BedBundle`, `CombinedDoseBundle`, `BoronUnitDose`,
`BoronField`, `Structure` masks via `VerifiedCase.structure_mask_array`)
has an `as_array()` accessor (and `uncertainty_array()` where a one-sigma
exists) returning a C-order `np.ndarray` of shape **`(nz, ny, nx)`**, so
`array[k, j, i]` is column `i`, row `j`, slice `k`. This is the repo's
flat voxel order (`i + nx*j + nx*ny*k`, x fastest) reshaped without any
transposition, so `as_array().ravel()` equals the list-returning
`values`, which stay for backward compatibility. Multigroup flux adds a
leading group axis, `(groups, nz, ny, nx)`, with groups in
`energy_boundaries_ev` order (descending energy). Geometry rides along as
`geometry`, `array_shape`, `spacing_mm`, `origin_mm` and `direction`
(`Geometry.shape`, spacing, origin and direction keep x, y, z order;
`Geometry.array_shape` is the NumPy shape). A `MultigroupFlux` loaded from
JSON carries no grid; bind one with `flux.with_geometry(geometry)`.
DVH curves offer `dose_edges_array()` and friends.

## Solving from Python

```python
import numpy as np, openbnct

solution = openbnct.sn_solve("case.json", "multigroup-data.json", order=4)
flux = solution.flux.as_array()                       # (groups, nz, ny, nx)
dose = solution.dose.physical_total.as_array()        # (nz, ny, nx)
print(dose[: dose.shape[0] // 2].mean())
```

`sn_solve(case, data, assignment=None, *, order=4, max_outer=32,
convergence=1e-6, allow_unconverged=False, anderson=3, p1=None,
anisotropy=0, dose=True, boron_unit=False)` (`p1=None` selects P1 whenever
every scattering material carries P1 moments, as the CLI does) calls the same Rust library
functions as `openbnct sn solve` (`solve_multigroup`, `fold_multigroup_dose`,
`fold_boron_unit_dose`) and releases the GIL while solving; it adds no
transport logic in Python (ADR 0015). `case` is a path or a `TransportCase`
(`load_transport_case`), `data` a path or `MultigroupData`. It returns an
`SnSolution` with `.flux`, `.dose` (a `PhysicalDoseBundle`, needs data that
declares a component profile) and `.boron_unit_dose` (a `BoronUnitDose`,
needs a collapsed `boron_unit_response_gy_cm2_per_ug_g`), which
`boron_dose` accepts directly. An unconverged solve raises
`OpenBnctError` unless `allow_unconverged=True`, which returns the
provisional field with `converged == False` and a `RuntimeWarning`.
`examples/python/workflow.py` runs the whole chain on the tiny
`nf-bnct-003` fixture (well under a second); `--layered-head` also solves
the layered head phantom, which takes minutes in a debug-built wheel.

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
Platform wheels are published on PyPI. Packaged releases can lag this source
tree; build current source to use later API additions. [ADR 0015](../../docs/adr/0015-python-and-native-distribution.md)
and [ADR 0027](../../docs/adr/0027-first-bounded-python-api.md) retain the original
distribution and API decisions.
