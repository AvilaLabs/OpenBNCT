# Avify Dose connector example

`openbnct avify` is the optional connector surface for the separately
licensed **Avify Dose** engine (R12, see `docs/R12_SCOPE_REVIEW.md`). The
engine is not distributed with OpenBNCT; the connector exports inputs,
runs the engine as a bounded child process, and ingests its certificate.

This example uses the layered-head benchmark case
(`benchmarks/synthetic/layered-head-phantom/case.json`) with:

- `assignment-tumour.json` — the benchmark assignment plus a 27-voxel
  `tumour-core` region carved from the brain centre (the benchmark has no
  tumour, and the engine requires all three ROI classes to be present).
- `spec.json` — an `openbnct.avify-spec/0.1.0` mapping the four regions to
  engine classes (skin→scalp, skull→cranium, brain→brain, tumour→tumour,
  default air) and carrying the engine plan fields: declared uptake set,
  ROI weights and criteria, normalization, and run budget. The declared
  set is passed to the engine **verbatim** — the connector never computes
  extremal maps.

## Export only (no engine needed)

```text
openbnct avify export-plan \
    --case benchmarks/synthetic/layered-head-phantom/case.json \
    --assignment examples/avify/assignment-tumour.json \
    --spec examples/avify/spec.json \
    --prefix out/plan
```

Writes `plan_arrays.npz` (int8 class map + ROI masks, z-y-x order),
`plan_meta.json` (classes, densities, scalar `voxel_cm`, corner origin),
and `plan.plan.json` (the engine plan). This step is pure Rust and
CI-executable.

## Verify (requires the engine)

With `avify-dose` installed (see the engine repository; needs OpenMC):

```text
openbnct avify verify \
    --case benchmarks/synthetic/layered-head-phantom/case.json \
    --assignment examples/avify/assignment-tumour.json \
    --spec examples/avify/spec.json \
    --outdir out/verify \
    --engine-cmd avify-dose --threads 4
```

Exports the voxel plan into `out/verify`, runs `avify-dose verify` with a
bounded wait (kill+reap on `--timeout-s`), then prints the certificate:
per-ROI certified `[L,U]` intervals vs their criteria with PASS / FAIL /
ADDITIONAL_EVIDENCE actions, nominal doses, and applicability flags. The
output is an **empirical two-evaluation envelope — research software, not
a certified clinical bound**.

The run also writes `out/verify/avify-run.json` — an
`openbnct.avify-run/0.1.0` receipt binding every input and exported
artifact by SHA-256 plus the engine's self-reported version.

## Inspect a result

```text
openbnct avify show    --certificate out/verify/certificate.json
openbnct avify status  --receipt    out/verify/avify-run.json
```

`status` recomputes the receipt's bound hashes and reports each input
`current` / `CHANGED` / `MISSING` with an overall CURRENT/STALE verdict —
a certificate whose inputs have since changed no longer describes them.
