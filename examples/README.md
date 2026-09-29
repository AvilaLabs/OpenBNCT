# Examples

End-to-end examples are added only when they are executable in CI and use
the reviewed synthetic benchmark corpus.

## `biological/`

- `fixed-component-weights-model-v1.json` — a demonstration
  `openbnct.biological-model/0.2.0` with fixed per-component effectiveness
  weights and a tumor-like boron override inside the `core` region. The
  weights are illustrative research values only; they carry no clinical CBE
  or RBE claim and are not part of the frozen NF-BNCT-001 benchmark outputs.
- `photon-isoeffective-lq-model-v1.json` — a `photon_isoeffective` variant
  whose weights are RBE/CBE-style photon-isoeffect factors (photon weight
  pinned to 1.0) with a linear-quadratic fractionation block: 30 fractions,
  1.0e9 source particles per fraction, α/β = 10 inside `core` and 3
  elsewhere. `bio apply` then reports the total in `weighted_eqd2` and
  records the applied schedule. Illustrative research values only.
- `core-region-mask.json` — the 512-voxel mask selecting the
  acceptance-contract `CORE` box (voxel centers within ±20 mm on every axis)
  on the benchmark's 40×40×40, 5 mm grid.

Usage is documented under "Biological interpretation and dose-volume
histograms" in the repository README.

## `endpoint/`

- `logistic-tcp-model-v1.json` — a logistic TCP model (`d50`, `gamma50`)
  reading serial-leaning `eud` (a=8) as its dose statistic.
- `probit-ntcp-model-v1.json` — a Lyman probit NTCP model (`td50`, `m`)
  reading the mean dose.
- `voxel-poisson-tcp-model-v1.json` — a voxel-level LQ Poisson TCP model
  (clonogen density, α, α/β, fraction count, source particles per
  fraction) reading a `*_per_source_particle` dose volume directly.

All are `openbnct.endpoint-model/0.1.0` artifacts with illustrative
synthetic parameters; they carry no clinical claim and are not part of the
frozen NF-BNCT-001 benchmark outputs.

## `openmc-multimaterial/`

- `component-profile-unit-mass-fraction.json` — the component profile that
  lets `openbnct openmc generate` take a multi-tissue material assignment
  (unit-mass-fraction B10/N14 folds, native-heating hydrogen residual). See
  "Multi-tissue decks" in `docs/USAGE.md`.

## `python/`

- `workflow.py` — the Python parity surface end to end: case generate /
  verify / load, contract-driven source aiming, MCNP deck export, and a
  DVH / metrics / biological-weighting / bundle-comparison pass on the
  committed conformance bundle. Runs in CI against the built wheel.
