# OpenBNCT

[![CI](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml/badge.svg)](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Status: early research](https://img.shields.io/badge/status-early_research-orange.svg)](ROADMAP.md)
[![Clinical use: not validated](https://img.shields.io/badge/clinical_use-not_validated-red.svg)](DISCLAIMER.md)

**English** | [日本語](README.ja.md)

OpenBNCT is an open, transport-neutral research workbench for boron neutron
capture therapy (BNCT) dosimetry and independent verification. It is built in
Rust around one idea: a dose result should carry its provenance, uncertainty,
and qualification with it — and a claim should only be as strong as the
evidence bound to it.

*(Formerly NCTForge — renamed across crates, the `openbnct` CLI/Python
package, and `openbnct.*` schema identifiers. Pre-rename `nctforge.*`
artifacts, including frozen benchmark evidence, remain readable through a
contract-namespace alias; see [ARCHITECTURE.md](ARCHITECTURE.md).)*

OpenMC is the first transport backend behind a transport-neutral boundary.
MCNP, PHITS, and other external results import through a published
component-dose interchange contract — nothing about the platform requires
bundling those systems.

> **OpenBNCT is research software.** It is not a medical device, not a dose
> calculator for clinical use, and not commissioned for any treatment
> facility. See [DISCLAIMER.md](DISCLAIMER.md).

## What it does today

- **Frozen synthetic benchmark** — `NF-BNCT-001`: synthetic DICOM CT +
  RTSTRUCT, transport-neutral `case.json`, explicit material and source
  contracts, all inputs bound by SHA-256.
- **OpenMC backend** — deterministic deck generation, controlled execution,
  statepoint collection into versioned physical-dose bundles
  (`openbnct openmc generate|run|collect|evaluate`).
- **Verified candidate reference** — three frozen-seed OpenMC runs at 600M
  histories each passed every predeclared acceptance gate: ROI precision,
  per-voxel precision (photon median RSE ≈ 2.7%), estimator comparisons
  (291), and chi-square seed consistency (378). Reports:
  [`openmc-acceptance-report-300M.json`](benchmarks/synthetic/nf-bnct-001/transport/openmc-acceptance-report-300M.json)
  (failed photon gate — kept in the record) and
  [`openmc-acceptance-report-600M.json`](benchmarks/synthetic/nf-bnct-001/transport/openmc-acceptance-report-600M.json)
  (passed).
- **Component dosimetry** — four-component physical dose (boron, nitrogen,
  hydrogen, photon) with absolute voxel uncertainty; DVH, `D_x`/`V_x`/EUD
  region metrics, mask operations, CT-threshold regions, NIfTI I/O, rigid
  image registration with provenance, PET-derived boron-10 fields with
  propagated uncertainty, and systematic-uncertainty reports with an
  explicit correlation model.
- **Biological interpretation** — versioned model families (weighted,
  photon-isoeffective, fractionation), sensitivity sweeps, endpoint models
  (logistic/probit TCP/NTCP, voxel-Poisson, UTCP), BED/EQD2 conversion —
  always a distinct layer from physical dose.
- **Transport neutrality** — `openbnct.component-dose-interchange/0.1.0`
  import contract; MCNP meshtal and PHITS output adapters; MCNP input-deck
  export; external-dose import and combined-treatment evaluation; a
  `openbnct compare` cross-code comparison record.
- **Facility beam descriptions** — versioned `openbnct.beam-description/0.1.0`
  documents (spectrum, divergence, aperture, normalization, cited provenance)
  that bind onto a transport case (`openbnct beam info|list|bind`); the
  `beams/` registry ships the FiR 1 K63 literature beam.
- **Beam quality characterization** — `openbnct beam qa` emits versioned
  `openbnct.beam-quality/0.1.0` reports: TECDOC-1223-style in-air group
  fluences and current-to-fluence ratio computed exactly from the declared
  source, optional in-phantom advantage-depth/ratio and peak therapeutic
  ratio from a dose bundle, and per-metric comparison against published
  reference values.
- **Measurement import** — `openbnct.measurement-record/0.1.0` documents
  (foil, ion chamber, TLD, TEPC spectra) compare against computed
  artifacts via `openbnct measurement compare`, emitting a
  hash-bound `openbnct.measurement-comparison/0.1.0` record with
  sigma-normalized and relative differences. Scalar metrics plus
  histogram-valued depth profiles are compared (peak-normalized shape
  convention, per-bin chi-square when the record states uncertainties).
- **Published-data validation** — the FiR 1 K63 epithermal beam
  (Seppälä 2002, HU-P-D103) is encoded as a declared beam and checked
  against published measurements: free-beam group fluence rates
  reproduce to ~1e-4 (with an honestly recorded 29% J/Φ gap), and the
  20M-history cubical water-phantom run reproduces the published
  advantage depth within 20% (9.75 vs 8.1 cm) and the thermal-fluence
  maximum at 2.75 cm vs ~2.0–2.5 cm. Records under `measurements/`;
  in-phantom evidence under `validation/fir1-k63-water-phantom/`.
- **RT Dose export** — `openbnct dicom export-rtdose` writes any dose
  volume as a multi-frame RTDOSE with full grid geometry and CT
  referencing, verified by independent-toolkit round-trip.
- **Variance reduction** — `openbnct vr resolve` derives OpenMC weight
  windows (uniform, explicit, or MAGIC-equivalent forward-flux bounds
  from an analog statepoint) into a content-bound
  `openbnct.weight-windows/0.1.0` artifact; `openmc generate --vr`
  binds it into the deck and `openbnct vr validate` certifies the
  reduced-history run against an analog acceptance report.
- **Three surfaces, one implementation** — CLI (`openbnct`), a native egui
  workbench (integrity-gated tri-planar viewer, dose wash, DVH/metrics,
  plan workspace, source positioning), and a bounded Python package —
  all calling the same Rust contracts.

## Quick start

All 15 `openbnct-*` crates are published on crates.io, and the `openbnct`
Python wheel is on TestPyPI:

```text
cargo install openbnct-cli               # CLI from crates.io
pip install --index-url https://test.pypi.org/simple/ \
    --extra-index-url https://pypi.org/simple/ openbnct
```

From source:

```text
cargo build --workspace                  # CLI + libraries
cargo run --bin openbnct -- --help       # CLI surface
cargo test --workspace                   # full suite incl. conformance
cargo run --bin openbnct-gui             # desktop workbench
```

The Python package builds one `abi3` wheel per platform (Python ≥ 3.10):

```text
pip install 'maturin>=1.7,<2'
maturin build --manifest-path bindings/python/Cargo.toml
```

## Where the evidence lives

- [`docs/USAGE.md`](docs/USAGE.md) — detailed command and workflow reference
- [`ROADMAP.md`](ROADMAP.md) — evidence-gated status, per-milestone detail
- [`benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md`](benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md)
  — the frozen case and its predeclared acceptance gates
- [`conformance/`](conformance/) — public fixture suites: interchange,
  biological and microdosimetric (MKM) models, endpoints, and
  MCNP/PHITS adapters
- [`docs/adr/`](docs/adr/) — architecture decision records
- [`docs/research/TECHNICAL_BASELINE.md`](docs/research/TECHNICAL_BASELINE.md)
  — scientific rationale
- [`docs/research/CROSS_CODE_REPRODUCTION.md`](docs/research/CROSS_CODE_REPRODUCTION.md)
  — the licensed-user recipe for the reference-promotion gate

## Honest status

- The candidate reference passed all **statistical** acceptance gates. Under
  the case specification it is not promoted to a *reference output* until a
  separately implemented transport path (Geant4, or licensed MCNP/PHITS
  produced by a licensed user) reproduces the frozen case.
- Variance reduction is verified **unbiased** at a 4.3× history reduction
  (1134 comparisons, max z = 2.97) and is pending the frozen photon
  precision gates — the deep photon heating tally is correlation-limited,
  so weight windows help it less than neutron fluence. See
  [`openmc-vr-validation-140M.json`](benchmarks/synthetic/nf-bnct-001/transport/openmc-vr-validation-140M.json).
- The MCNP/PHITS adapters are verified end-to-end at benchmark scale on
  documented-format bundles; real-engine acceptance remains an open gate.
- The FiR 1 in-phantom comparison uses wide tolerances: published
  advantage figures fold a clinical tumor:normal boron uptake (~3.5)
  into dose weighting while the phantom carries trace loading, and the
  simplified cone source under-models penumbra scatter — the advantage
  ratio misses (2.43 vs 4.9) and is kept in the record as evidence of
  the fidelity-tier gap, not suppressed.
- Nothing here claims clinical qualification, clinical equivalence,
  commissioning, or regulatory suitability.

## License and use boundary

Code is licensed under Apache-2.0. Synthetic benchmark data will receive an
explicit data license before its first release.

The repository must not implement Avify Dose patent subject matter without a
documented intellectual-property review. See
[docs/IP_BOUNDARY.md](docs/IP_BOUNDARY.md).
