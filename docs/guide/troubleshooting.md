# Troubleshooting

| Symptom | Check next |
| --- | --- |
| A documented command is missing | Installed version; current project/solver additions postdate the packaged release. |
| Project import rejects DICOM | Multiple CT series, unsupported structure objects, geometry or missing calibration. |
| A scaffold case gives unexpected transport | Replace its placeholder source with the intended beam. |
| Solver stops without convergence | Residual history, iteration budget, mesh, source/data consistency and supported solver settings. |
| Report says provisional | An unconverged calculation was explicitly allowed; retain that status. |
| Photon dose disagrees strongly | Local deposition versus transported capture photons and compatible photon data. |
| OpenMC comparison differs | Source-bin weighting, S(α,β), material fractions, response definitions and Monte Carlo sigma. |
| Python image appears transposed | Arrays use `(nz, ny, nx)`; geometry fields use x/y/z order. |
| An apparent dose is tiny or enormous | Per-particle versus rate versus Gy normalization, source strength and irradiation time. |
| Plan import flags a row | Referenced bundle, normalization, geometry and table contract. |
| Verification is stale | Inputs changed; rerun the affected calculation and comparison. |
| Browser cannot execute an external job | External processes and case folders require desktop/CLI. |

The browser needs enabled WebGL2 or WebGPU. Start with the bundled example to distinguish file/import issues from graphics support.

Report bugs with the version/commit, platform, exact command, convergence output and a minimal synthetic input through [GitHub Issues](https://github.com/AvilaLabs/OpenBNCT/issues). Preserve private imaging and study data locally; use shareable synthetic fixtures in public reports.
