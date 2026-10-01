# Multi-field research plans

An exposure plan combines dose bundles under declared field weights, normalization and covariance assumptions. Use it to compare research alternatives and inspect how objectives respond to beam selection and weighting.

## Bring a plan table

CSV and XLSX exposure tables can be imported and exported:

```bash
openbnct plan import --table schedule.xlsx --output plan.json
openbnct plan validate --plan plan.json
openbnct plan export --plan plan.json --output schedule.csv
```

Read the reported row-level issues. Bind the intended dose bundle for every exposure and keep their geometries and normalization compatible. The desktop Plan workspace displays the table and diagnostics alongside supported optimization controls.

## Search alternatives

Research tools rank beam directions using tissue path or adjoint importance, select fields under objectives, and iteratively optimize weights. Spectrum/aperture sweeps and beamlet shaping support additional declared comparisons.

Use the objective values and violated constraints to interpret the selected weights. Check convergence, initial conditions and the supported candidate set. A better objective is conditional on those inputs and on the chosen dose/biology approximation.

## Scenario and robustness studies

Named uptake, output and positioning scenarios can be evaluated and used for worst-case penalty optimization. The scenario set needs its own basis; it is not automatically a probability distribution.

Current positioning scenarios shift existing dose fields. They do not re-solve transport through moved anatomy. A fixed-weight biological objective also differs from a full nonlinear biological evaluation.

Study the [known-answer scenario optimizer](https://github.com/AvilaLabs/OpenBNCT/tree/main/benchmarks/synthetic/scenario-robust-planning) and [FiR 1 scenario study](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/fir1-k63-scenario-budget). Command contracts are in the [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md).
