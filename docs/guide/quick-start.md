# Your first study

## Explore a result in the browser

Open [the workbench](https://openbnct.avilalabs.org) and choose **Load example bundle** in the dose workspace. Inspect the boron, nitrogen, hydrogen and photon components, then the physical total and its normalization.

Use the tri-planar view to locate regions and compare component distributions. A bundled result is a particular recorded calculation; opening it does not solve a new transport problem.

## Run a synthetic CT-to-report study

Use a current source-built CLI for this workflow. From a working directory with fresh output names:

```bash
openbnct benchmark generate study/
openbnct project init --dicom study --output p001 --target CORE --spacing-mm 8
openbnct project run p001
```

This generates synthetic NF-BNCT-001 imaging and initializes a demonstration project. Its tissue calibration, beam and boron assumptions are example inputs. It is not a patient study.

The calculation can take several minutes with a release build. Read `p001/out/report.md` or `report.json`; per-structure DVH curves are under `p001/out/dvh/`.

## Inspect and revise

The project report lists component means and dose-volume statistics, convergence status, normalization and exact step commands. Check all of these before interpreting a total.

Edit `p001/project.toml` to change an explicit study assumption. `openbnct project status p001` shows its steps; `project run` resumes unchanged steps and recalculates affected outputs. Preserve a copy when comparing alternatives.

The current project default uses histogram-bin source weighting and transports capture photons separately. Disabling photon transport changes the photon-dose model to local deposition.

## Compare independently

With OpenMC 0.16.0 and the declared processed library configured:

```bash
openbnct project verify p001 --particles 1e6
```

The report gains component ratios, statistical uncertainty, gamma results and an explicit verification verdict. A completed run can disagree with the deterministic calculation. See [benchmarks](benchmarks.md) before interpreting this result.

Next: [read your results](results.md), then [bring your own inputs](inputs.md).
