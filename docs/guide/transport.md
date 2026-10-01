# Neutron and photon transport

OpenBNCT's deterministic solver uses multigroup discrete ordinates, with declared spatial/angular resolution and scattering moments. OpenMC supplies an independent Monte Carlo path. Compare both methods with matched inputs and adequate resolution.

## Start with the project workflow

`project run` orchestrates imaging import, material calibration, beam binding, neutron transport, optional photon transport, boron scaling, metrics and reporting. Current defaults spread histogram bins uniformly per eV and transport capture photons with a separate photon solve.

Inspect `[transport]` in `project.toml`: angular order, iteration budget, source weighting, photon policy and the allowed-unconverged setting. A solve that fails to converge requires diagnosis before its dose field is interpreted.

## A lower-level neutron example

From the current source checkout:

```bash
openbnct sn solve \
  --case benchmarks/synthetic/layered-head-phantom/case.json \
  --data benchmarks/synthetic/layered-head-phantom/multigroup-data-28g-v5-tsl.json \
  --assignment benchmarks/synthetic/layered-head-phantom/assignment.json \
  --source-weighting uniform_in_bin --dose dose.json --output flux.json
```

This produces a neutron flux and its folded dose. The project workflow additionally uses `sn photon-solve` and `sn merge-photon-dose` with compatible photon data. A neutron-only folded photon channel can use local capture-energy deposition, which changes the physical comparison.

## Compare the same physics

Match geometry, density/composition, source distributions, thermal scattering and dose-response definitions. `project verify` uses the project's boron assumptions and reports whether thermal-scattering physics matches.

Group collapse, mesh/angular resolution, void ray effects and Monte Carlo statistics can each cause differences. Increasing iteration count alone does not resolve those errors. Inspect regional and spatial discrepancies alongside a global mean.

Forward/adjoint and variance-reduction tools support additional research workflows. Their exact data requirements and options are in the [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md). Read [benchmarks](benchmarks.md) for the tested regimes.
