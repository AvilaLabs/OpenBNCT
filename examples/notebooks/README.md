# Python notebooks

Worked Jupyter notebooks against the published `openbnct` wheel — the
same Rust engines the CLI calls, driven through `import openbnct`. Each
notebook reads committed repository artifacts, so they run anywhere a
checkout exists, and the committed `.ipynb` files carry executed outputs
as evidence they work.

```text
pip install openbnct jupyter matplotlib
jupyter lab examples/notebooks/
```

| Notebook | What it covers |
|---|---|
| `01-quickstart.ipynb` | Load a committed S_N dose bundle, inspect the four physical components, mid-plane dose map, DVH, exact D_x/V_x/EUD metrics |
| `02-biological-modeling.ipynb` | Fixed-weights biological model with region binding, González & Santa Cruz isoeffective model authored inline, side-by-side comparison |
| `03-uncertainty.ipynb` | Evaluated ENDF ¹⁰B(n,α) covariance artifact, group-correlation inspection, the propagated dose-uncertainty budget |

`build.py` reauthors and re-executes all notebooks in place — run it
after changing the artifact set or the Python surface:

```text
python examples/notebooks/build.py
```

Research use only — nothing here is a clinical, equivalence,
commissioning, or regulatory claim.
