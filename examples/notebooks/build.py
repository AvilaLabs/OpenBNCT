#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Author and execute the OpenBNCT example notebooks.

Each notebook is written under this directory and executed in place with
nbclient, so the committed .ipynb files carry real outputs — they are
evidence the notebooks run against the published wheel, not just prose.

    pip install openbnct nbformat nbclient ipykernel matplotlib
    python examples/notebooks/build.py

Run from the repository root (the notebooks reference committed
artifacts by repository-relative paths).
"""

from pathlib import Path

import nbformat as nbf
from nbclient import NotebookClient

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]

KERNELSPEC = {"display_name": "Python 3", "language": "python", "name": "python3"}
LANGINFO = {"name": "python", "pygments_lexer": "ipython3"}


def nb(cells):
    book = nbf.v4.new_notebook()
    book.metadata["kernelspec"] = KERNELSPEC
    book.metadata["language_info"] = LANGINFO
    for kind, src in cells:
        book.cells.append(
            nbf.v4.new_markdown_cell(src) if kind == "md" else nbf.v4.new_code_cell(src)
        )
    return book


HEADER = (
    "> **OpenBNCT is research software.** Nothing computed here is a "
    "clinical, equivalence, commissioning, or regulatory claim. "
    "See `docs/DISCLAIMER.md`.\n>\n"
    "> Run from the repository root after `pip install openbnct` "
    "(the notebooks read committed artifacts by repository-relative "
    "paths)."
)

# ---------------------------------------------------------------- nb 1

nb1 = nb([
    ("md", "# 01 — Quickstart: a dose bundle in five minutes\n"
           "\n"
           + HEADER + "\n\n"
           "This notebook loads the committed layered-head phantom dose "
           "bundle — a 28-group deterministic S_N solve — and exercises "
           "the read-only analysis surface: component inspection, a "
           "mid-plane dose map, a DVH, and exact dose-volume metrics."),
    ("md", "## Load the bundle\n\n"
           "`load_physical_dose_bundle` validates the artifact against "
           "its schema before returning it — a malformed bundle fails "
           "here, never downstream."),
    ("code",
     "from pathlib import Path\n"
     "import openbnct\n"
     "\n"
     "# locate the repository root regardless of launch directory\n"
     "REPO = Path.cwd()\n"
     "while not (REPO / 'benchmarks').is_dir() and REPO != REPO.parent:\n"
     "    REPO = REPO.parent\n"
     "assert (REPO / 'benchmarks').is_dir(), 'run inside an OpenBNCT checkout'\n"
     "\n"
     "BENCH = REPO / 'benchmarks/synthetic/layered-head-phantom'\n"
     "bundle = openbnct.load_physical_dose_bundle(BENCH / 'dose-28g-v2.json')\n"
     "print(bundle.schema_version, bundle.case_id)\n"
     "print('grid:', bundle.geometry.shape, 'voxels @',\n"
     "      bundle.geometry.spacing_mm, 'mm')\n"
     "print('provenance:', bundle.provenance_id)"),
    ("md", "## Components\n\n"
           "Every bundle carries the four physical BNCT dose components "
           "separately — boron (¹⁰B(n,α)⁷Li), nitrogen (¹⁴N(n,p)¹⁴C), "
           "hydrogen (recoil protons), and photon — each with its own "
           "unit and an optional per-voxel standard uncertainty."),
    ("code",
     "import math\n"
     "\n"
     "rows = []\n"
     "for c in bundle.components + [bundle.physical_total]:\n"
     "    nz = sum(1 for v in c.values if v > 0)\n"
     "    rows.append((c.component, c.unit, max(c.values), nz))\n"
     "print(f'{\"component\":<14}{\"unit\":<30}{\"max\":>12}{\"nonzero voxels\":>16}')\n"
     "for name, unit, mx, nz in rows:\n"
     "    print(f'{name:<14}{unit:<30}{mx:>12.3e}{nz:>16}')"),
    ("md", "## Mid-plane dose map\n\n"
           "The bundle stores voxel data x-fastest (`[column, row, slice]`, "
           "columns varying fastest), so NumPy's default C-order reshape "
           "takes the axes reversed — `(nz, ny, nx)` — and the central "
           "z-plane is `vol[nz // 2]`, a `(ny, nx)` image."),
    ("code",
     "import matplotlib.pyplot as plt\n"
     "import numpy as np\n"
     "\n"
     "nx, ny, nz = bundle.geometry.shape\n"
     "total = np.array(bundle.physical_total.values).reshape(nz, ny, nx)\n"
     "mid = total[nz // 2]\n"
     "\n"
     "fig, ax = plt.subplots(figsize=(5, 4))\n"
     "im = ax.imshow(np.log10(mid + 1e-30), origin='lower', cmap='inferno')\n"
     "ax.set_title('physical_total — mid z-plane (log10)')\n"
     "fig.colorbar(im, ax=ax, label='log10(Gy/source-particle)')\n"
     "plt.show()"),
    ("md", "## DVH and dose-volume metrics over the tumor mask\n\n"
           "Masks are versioned `RegionMask` artifacts — name plus a "
           "boolean per grid voxel. `compute_metrics` is exact: D_x, "
           "V_x, and EUD are evaluated on the voxel values, not a "
           "histogram approximation."),
    ("code",
     "import json\n"
     "\n"
     "mask = json.loads((BENCH / 'planning/tumor-mask.json').read_text())\n"
     "dvh = openbnct.compute_dvh(bundle, 'physical_total',\n"
     "                           mask['name'], mask['voxels'], bins=60)\n"
     "\n"
     "fig, ax = plt.subplots(figsize=(6, 3.5))\n"
     "ax.plot(dvh.dose_edges[1:], dvh.cumulative_volume_fraction[1:])\n"
     "ax.set_xlabel(f'dose ({dvh.unit})')\n"
     "ax.set_ylabel('fraction of tumor volume ≥ dose')\n"
     "ax.set_title(f'DVH — {dvh.region} '\n"
     "             f'({dvh.region_voxel_count} voxels, '\n"
     "             f'{dvh.region_volume_mm3:.0f} mm³)')\n"
     "ax.grid(alpha=0.3)\n"
     "plt.show()"),
    ("code",
     "met = openbnct.compute_metrics(bundle, 'physical_total',\n"
     "                               mask['name'], mask['voxels'],\n"
     "                               dx=[2.0, 50.0, 98.0],\n"
     "                               # V_x thresholds are absolute doses —\n"
     "                               # this bundle is Gy/source-particle,\n"
     "                               # so pick in-range values\n"
     "                               vx=[4.0e-14, 6.0e-14], eud=[-10.0])\n"
     "print(f\"mean  {met.mean_dose:.3e}  min {met.minimum_dose:.3e}  \"\n"
     "      f\"max {met.maximum_dose:.3e} {met.unit}\")\n"
     "print('D_x:', {x: f'{v:.3e}' for x, v in met.dx})\n"
     "print('V_x:', {x: f'{v:.3f}' for x, v in met.vx})\n"
     "print('EUD:', {a: f'{v:.3e}' for a, v in met.eud})"),
    ("md", "**Next:** `02-biological-modeling.ipynb` applies the "
           "biological layer; `03-uncertainty.ipynb` walks the "
           "evaluated-covariance chain."),
])

# ---------------------------------------------------------------- nb 2

nb2 = nb([
    ("md", "# 02 — Biological modeling: weighted, MKM, and isoeffective dose\n"
           "\n" + HEADER + "\n\n"
           "Physical dose is not the quantity clinicians weight — the "
           "mixed-field components carry different biological potency. "
           "This notebook applies the two biological surfaces: a "
           "fixed-weights biological model and the González & Santa Cruz "
           "isoeffective model (per-component dose-independent factors, "
           "√β cross-terms, Lea–Catcheside repair)."),
    ("md", "## Fixed-weights model\n\n"
           "The model is a versioned artifact declaring per-component "
           "weights plus optional per-region overrides. Regions bind to "
           "`RegionMask` artifacts **by name** — the validator rejects a "
           "mask whose declared name doesn't match the model's region."),
    ("code",
     "from pathlib import Path\n"
     "import json, tempfile\n"
     "import openbnct\n"
     "\n"
     "# locate the repository root regardless of launch directory\n"
     "REPO = Path.cwd()\n"
     "while not (REPO / 'benchmarks').is_dir() and REPO != REPO.parent:\n"
     "    REPO = REPO.parent\n"
     "assert (REPO / 'benchmarks').is_dir(), 'run inside an OpenBNCT checkout'\n"
     "\n"
     "BENCH = REPO / 'benchmarks/synthetic/layered-head-phantom'\n"
     "bundle = openbnct.load_physical_dose_bundle(BENCH / 'dose-28g-v2.json')\n"
     "\n"
     "model = openbnct.load_biological_model(\n"
     "    REPO / 'examples/biological/fixed-component-weights-model-v1.json')\n"
     "print(model.id, model.schema_version)"),
    ("code",
     "# The model declares a 'core' region override; the phantom's tumor\n"
     "# mask is that region here. Masks bind by name, so write a copy\n"
     "# carrying the region name the model expects.\n"
     "mask = json.loads((BENCH / 'planning/tumor-mask.json').read_text())\n"
     "mask['name'] = 'core'\n"
     "tmp = Path(tempfile.mkdtemp())\n"
     "mask_path = tmp / 'core-mask.json'\n"
     "mask_path.write_text(json.dumps(mask))\n"
     "\n"
     "bio = openbnct.apply_model(model, bundle, [('core', str(mask_path))])\n"
     "print('regions applied:', bio.regions_applied)\n"
     "print('weight semantics:', bio.weight_semantics)\n"
     "print('qualification:', bio.qualification)"),
    ("md", "Each component keeps its weighted volume; the biological "
           "total is their sum. Compare maxima against the physical "
           "bundle:"),
    ("code",
     "for c in bio.components:\n"
     "    print(f'{c.component:<10} max weighted {max(c.values):.3e} '\n"
     "          f'{c.unit}')\n"
     "print(f'{\"biological_total\":<10} max {max(bio.biological_total.values):.3e} '\n"
     "      f'{bio.unit}')"),
    ("md", "## González & Santa Cruz isoeffective dose\n\n"
           "The fixed-weights model is the dose-independent-factor "
           "approximation. The IsoE model keeps per-component `rbe` "
           "(linear term) and `rbe_beta` (√β synergy) separate and "
           "inverts the mixed-field exponent once against the photon LQ. "
           "Models can be authored inline — `make_isoeffective_model` "
           "runs the same validation the file path does."),
    ("code",
     "iso = openbnct.make_isoeffective_model({\n"
     "    'schema_version': 'openbnct.isoeffective-model/0.1.0',\n"
     "    'id': 'notebooks.gs-isoeffective.demo',\n"
     "    'input_unit': 'gray_per_source_particle',\n"
     "    'alpha_0': 0.02,\n"
     "    'beta': 0.01,\n"
     "    'components': {\n"
     "        'boron':    {'rbe': 3.8, 'rbe_beta': 4.9},\n"
     "        'nitrogen': {'rbe': 2.5, 'rbe_beta': 2.5},\n"
     "        'hydrogen': {'rbe': 3.2, 'rbe_beta': 3.2},\n"
     "        'photon':   {'rbe': 1.0, 'rbe_beta': 1.0},\n"
     "    },\n"
     "    'region_factors': {\n"
     "        'tumor': {\n"
     "            'boron':    {'rbe': 4.9, 'rbe_beta': 4.9},\n"
     "            'nitrogen': {'rbe': 2.5, 'rbe_beta': 2.5},\n"
     "            'hydrogen': {'rbe': 3.2, 'rbe_beta': 3.2},\n"
     "            'photon':   {'rbe': 1.0, 'rbe_beta': 1.0},\n"
     "        },\n"
     "    },\n"
     "    'validity_domain': 'notebook demonstration factors; '\n"
     "                       'not a clinical model',\n"
     "})\n"
     "isoe = openbnct.apply_isoeffective(\n"
     "    iso, bundle, [('tumor', str(\n"
     "        BENCH / 'planning/tumor-mask.json'))])\n"
     "print('weight semantics:', isoe.weight_semantics)\n"
     "for c in isoe.components:\n"
     "    print(f'{c.component:<10} max {max(c.values):.3e} {c.unit}')"),
    ("md", "## Fixed-weights vs IsoE, head to head\n\n"
           "Same factors where comparable — the difference is the "
           "combined-then-inverted exponent (Zaider–Rossi cross terms) "
           "versus per-component inversion summed after."),
    ("code",
     "import matplotlib.pyplot as plt\n"
     "import numpy as np\n"
     "\n"
     "nx, ny, nz = bundle.geometry.shape\n"
     "# x-fastest voxel order → C-order reshape (nz, ny, nx); mid z-plane.\n"
     "a = np.array(bio.biological_total.values).reshape(nz, ny, nx)[nz // 2]\n"
     "c = np.array(isoe.biological_total.values).reshape(nz, ny, nx)[nz // 2]\n"
     "\n"
     "fig, axes = plt.subplots(1, 3, figsize=(13, 4))\n"
     "for ax, vol, title in zip(axes, [a, c, c / np.maximum(a, 1e-30)],\n"
     "                          ['fixed-weights', 'G&S IsoE', 'IsoE / fixed ratio']):\n"
     "    im = ax.imshow(np.log10(vol + 1e-30) if 'ratio' not in title\n"
     "                   else vol, origin='lower', cmap='inferno')\n"
     "    ax.set_title(title)\n"
     "    fig.colorbar(im, ax=ax)\n"
     "plt.tight_layout()\n"
     "plt.show()"),
    ("md", "**Next:** `03-uncertainty.ipynb` — the evaluated-covariance "
           "chain that no other BNCT tool ships."),
])

# ---------------------------------------------------------------- nb 3

nb3 = nb([
    ("md", "# 03 — Uncertainty: evaluated nuclear data to dose σ\n"
           "\n" + HEADER + "\n\n"
           "OpenBNCT propagates *evaluated* covariance — the uncertainty "
           "carried in ENDF evaluations — through the dose-response "
           "chain. This notebook inspects the committed artifacts: the "
           "¹⁰B(n,α) covariance collapsed onto the 28-group mesh and the "
           "dose-uncertainty budget it produced."),
    ("md", "## The covariance artifact\n\n"
           "`openbnct.multigroup-covariance/0.1.0` binds a reaction's "
           "evaluated covariance to a multigroup group structure — here "
           "ENDF/B-VIII.1 ¹⁰B MT=107 (n,α), including NC-type "
           "derived-quantity references resolved by the reader."),
    ("code",
     "from pathlib import Path\n"
     "import json\n"
     "import openbnct\n"
     "\n"
     "# locate the repository root regardless of launch directory\n"
     "REPO = Path.cwd()\n"
     "while not (REPO / 'benchmarks').is_dir() and REPO != REPO.parent:\n"
     "    REPO = REPO.parent\n"
     "assert (REPO / 'benchmarks').is_dir(), 'run inside an OpenBNCT checkout'\n"
     "\n"
     "cov = openbnct.load_multigroup_covariance(\n"
     "    REPO / 'examples/covariance/b10-endfb81-mt107-layered-head.covariance.json')\n"
     "doc = json.loads(cov.to_json())\n"
     "print(doc['id'])\n"
     "print('blocks:', len(doc['blocks']))\n"
     "blk = doc['blocks'][0]\n"
     "print('material:', blk['material_id'])\n"
     "print('parameter:', blk['parameter'], '| component:', blk['component'])\n"
     "print('provenance:', doc['provenance_note'])"),
    ("code",
     "import matplotlib.pyplot as plt\n"
     "import numpy as np\n"
     "\n"
     "# per-group relative standard deviation and the 28×28 correlation\n"
     "# matrix (flattened row-major in the artifact)\n"
     "rsd = np.array(blk['relative_std_dev'])\n"
     "corr = np.array(blk['correlation']).reshape(28, 28)\n"
     "\n"
     "fig, axes = plt.subplots(1, 2, figsize=(10, 4))\n"
     "axes[0].bar(range(28), rsd)\n"
     "axes[0].set_xlabel('group index (high → low energy)')\n"
     "axes[0].set_ylabel('relative std dev')\n"
     "axes[0].set_title('¹⁰B(n,α) σ_rel per group')\n"
     "im = axes[1].imshow(corr, cmap='viridis', vmin=-1, vmax=1)\n"
     "axes[1].set_title('group correlation')\n"
     "fig.colorbar(im, ax=axes[1])\n"
     "plt.tight_layout()\n"
     "plt.show()"),
    ("md", "## The dose-uncertainty budget\n\n"
           "`uq propagate` folds the covariance through the component's "
           "dose-response vector — analytic sensitivities, no perturbed "
           "solves. The committed budget records every contribution and "
           "the total."),
    ("code",
     "budget = openbnct.load_dose_uncertainty_budget(\n"
     "    REPO / 'examples/covariance/layered-head-endf-b10na-budget.json')\n"
     "bd = json.loads(budget.to_json())\n"
     "print('component:', bd['component'])\n"
     "print('response integral:', f\"{bd['response_integral']:.3e}\")\n"
     "print('total σ_rel:', f\"{bd['total_relative_std_dev']:.4%}\")\n"
     "print()\n"
     "for e in bd['entries']:\n"
     "    print(f\"  {e['source']:<40} share {e['relative_contribution']:.2%} \"\n"
     "          f\"(variance contribution {e['variance_contribution']:.3e})\")\n"
     "print()\n"
     "print('method:', bd['method_note'])"),
    ("md", "The entire evaluated-¹⁰B(n,α) uncertainty contributes 0.34% "
           "σ_rel on the boron dose integral — the evaluation is tight "
           "thermally where the response concentrates. The same chain "
           "accepts systematic terms (boron concentration, positioning, "
           "component scale) via `openbnct uq propagate`.\n\n"
           "**Where to go deeper:** `docs/USAGE.md` covers `uq`, "
           "`cov-endf`, and the sensitivity-screening surfaces; "
           "`examples/covariance/README.md` reproduces this artifact "
           "end to end."),
])

# ---------------------------------------------------------------- write

for name, book in [("01-quickstart", nb1),
                   ("02-biological-modeling", nb2),
                   ("03-uncertainty", nb3)]:
    path = HERE / f"{name}.ipynb"
    nbf.write(book, path)
    client = NotebookClient(book, timeout=300, kernel_name="python3",
                            resources={"metadata": {"path": str(REPO)}})
    client.execute()
    nbf.write(book, path)
    print(f"{name}: executed, {sum(1 for c in book.cells if c.cell_type == 'code')} code cells")
