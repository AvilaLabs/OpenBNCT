# Python and NumPy

The `openbnct` package exposes the authoritative Rust contracts and calculations. It supports verified cases, dose analysis, model evaluation, interchange and a bounded deterministic-solver interface.

Use `python -m pip install openbnct` for the published package. Build the current source wheel to use later additions; see [installation](install.md).

## Read and analyze a bundle

```python
import openbnct

bundle = openbnct.load_physical_dose_bundle("dose.json")
dose = bundle.physical_total.as_array()
print(dose.shape)
print(dose.mean())
```

Inspect the bundle's units and normalization before interpreting the mean. A whole-array mean is not a target statistic; use the appropriate structure mask for regional analysis.

## Solve with current source

From a source checkout with current bindings installed:

```python
import openbnct

# Substitute your declared transport-case and compatible multigroup data.
solution = openbnct.sn_solve("case.json", "multigroup-data.json", order=4)
flux = solution.flux.as_array()
physical = solution.dose.physical_total.as_array()
```

The call releases the GIL and uses the Rust solver. Unconverged solutions raise by default; explicitly allowing them returns provisional output with a warning. The Python interface exposes a subset of CLI solver controls, so use the CLI/project path for the full current photon and source-weighting workflow.

## Array conventions

Voxel arrays have C-order shape `(nz, ny, nx)`: `array[k, j, i]` maps to x-index `i`, y-index `j`, z-index `k`. Flattening preserves x-fastest voxel order. Geometry shape, origin and spacing retain x/y/z order.

Flux adds a leading group axis: `(groups, nz, ny, nx)`, in descending energy-boundary order. A flux JSON carries no grid by itself; bind geometry with `with_geometry` where needed.

Run [the worked example](https://github.com/AvilaLabs/OpenBNCT/blob/main/examples/python/workflow.py) for actual fixture paths, case generation, dose analysis and a tiny solver demonstration. [Binding reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/bindings/python/README.md).
