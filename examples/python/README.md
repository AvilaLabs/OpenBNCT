# Python worked example

`workflow.py` exercises the Python parity surface end to end — the same
Rust engines the CLI calls, driven through `import openbnct`:

- **Case pipeline** — `generate_case` → `verify_case` → `load_case`,
  then geometry/structure inspection, contract-driven source aiming, and
  `export_mcnp_deck` for the reproduction path.
- **Dose analysis** — `load_physical_dose_bundle` → `compute_dvh` →
  `compute_metrics` → `apply_model` → `compare_dose_bundles`, all on the
  committed 2-voxel conformance bundle and its matching mask/model.

- **Solve and NumPy** — `sn_solve` on the tiny `nf-bnct-003` slab fixture,
  the flux and dose as `(nz, ny, nx)` NumPy arrays, a NumPy region mean,
  and `boron_dose`. The fixture has no collapsed 10B unit response, so the
  example attaches an illustrative one: a mechanics demo, not a physical
  prediction. `python examples/python/workflow.py --layered-head` also
  solves the layered head phantom (minutes in a debug wheel).

Monte Carlo transport (OpenMC or a licensed engine) is external and not
part of the example; everything else runs against committed
artifacts. All outputs live under a temporary directory.

```text
pip install bindings/python/dist/openbnct-*.whl   # or: maturin develop
python examples/python/workflow.py
```

CI runs the same script against the built wheel. Research use only —
the biological weights and all outputs carry no clinical claim.
