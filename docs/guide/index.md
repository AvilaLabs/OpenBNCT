# Welcome to OpenBNCT

OpenBNCT is an open-source research workbench for boron neutron capture therapy. Calculate neutron and photon component dose, study boron uptake and irradiation timing, compare biological models, optimize multi-field research plans and investigate uncertainty.

The Rust engine powers the CLI, Python package, desktop application and browser workbench. A deterministic multigroup solver works without an external transport code. OpenMC integration and MCNP/PHITS interchange support independent comparisons.

**OpenBNCT 0.3.0** is the current release. This handbook describes the current source tree; build current source when following commands added after 0.3.0.

## Choose a starting point

| Your task | Start here |
| --- | --- |
| Explore a bundled dose result | [Open the workbench](https://openbnct.avilalabs.org), then [your first study](quick-start.md) |
| Run a CT-to-report demonstration | [Your first study](quick-start.md) |
| Bring an image, labelmap or beam spectrum | [Imaging, materials and beams](inputs.md) |
| Calculate and compare transport | [Transport](transport.md) and [benchmarks](benchmarks.md) |
| Analyze dose arrays in a notebook | [Python and NumPy](python.md) |
| Study uptake, biology or plan alternatives | [Boron](boron.md), [biological models](biology.md) and [planning](planning.md) |

OpenBNCT is experimental research software. Its outputs are not commissioned or clinically qualified for patient care. The [research scope](scope.md) explains this boundary and the evidence needed to interpret a calculation.

Source, releases and issues are on [GitHub](https://github.com/AvilaLabs/OpenBNCT). OpenBNCT is developed by Avila Labs under the [MIT license](https://github.com/AvilaLabs/OpenBNCT/blob/main/LICENSE).
