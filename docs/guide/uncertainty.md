# Uncertainty and sensitivity

OpenBNCT provides several uncertainty paths. Choose one that matches the quantity and sources you need to assess, and report which categories remain unassessed.

| Path | What it evaluates |
| --- | --- |
| Statistical sigma in a dose bundle | Available sampling uncertainty from the producing calculation |
| Systematic dose/plan tools | Declared uptake, positioning, component and metric uncertainty |
| Joint ensembles | Named correlated/shared sources applied to realized dose maps, with per-realization metrics |
| Nuclear-data propagation | Declared multigroup covariance propagated to an integrated component response |
| Morris/Sobol screening | Sensitivity of specified outputs to declared input variations |

## Joint sources

Current source provides `uq joint`. Its input declares source identity, scope, distribution, evidence and dispositions for uncovered categories. Shared calibration draws apply across their scope; declared correlation groups are sampled jointly.

Metrics such as D95 are computed on each realized map before summarizing the ensemble. Quantiles of per-voxel fields do not substitute for quantiles of a dose-volume statistic. Optional PK integration applies where the bundle units and declared model support it.

The ensemble is conditional on its supplied distributions, correlations and dose adapters. It does not infer a complete patient uncertainty model from an input file.

## Nuclear data and solver effects

`uq propagate` uses the declared covariance and finite-difference response sensitivities of the discrete solve. It reports response-space variance contributions. Supported ENDF MF33 inputs and multigroup covariance contracts have explicit coverage limits.

Iteration residual, discretization error, material/source mismatch, measurement error and statistical sigma are different quantities. A converged solve with small statistical uncertainty can still have a substantial systematic discrepancy.

Compare [recorded validation](benchmarks.md), then use the [uncertainty reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md) to declare the sources relevant to your research question. Retain unsupported categories in the report.
