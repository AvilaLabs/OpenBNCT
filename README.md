# OpenBNCT

[![CI](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml/badge.svg)](https://github.com/AvilaLabs/OpenBNCT/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![crates.io](https://img.shields.io/crates/v/openbnct-core.svg)](https://crates.io/crates/openbnct-core)
[![PyPI](https://img.shields.io/pypi/v/openbnct.svg)](https://pypi.org/project/openbnct/)
[![Status: early research](https://img.shields.io/badge/status-early_research-orange.svg)](ROADMAP.md)
[![Clinical use: not validated](https://img.shields.io/badge/clinical_use-not_validated-red.svg)](DISCLAIMER.md)

**English** | [日本語](README.ja.md)

### An open, transport-neutral workbench for BNCT research and independent verification.

Every dose result carries its provenance, uncertainty, and qualification with
it — every claim is only as strong as the evidence bound to it.

<p align="left">
  <a href="https://openbnct.avilalabs.org"><img src="https://img.shields.io/badge/open%20in%20browser-openbnct.avilalabs.org-0d9488?style=for-the-badge" alt="Open in browser"></a>
  <a href="https://github.com/AvilaLabs/OpenBNCT/releases/latest"><img src="https://img.shields.io/badge/download-desktop%20app-1f2937?style=for-the-badge" alt="Download desktop app"></a>
</p>

> **Research software** — not a medical device, not commissioned for any
> treatment facility, not for clinical decisions. See
> [DISCLAIMER.md](DISCLAIMER.md).

## What you can do with it

- **Run the frozen benchmark** — `NF-BNCT-001`, a synthetic head-and-neck
  case whose inputs, acceptance gates, and 600M-history OpenMC reference are
  all checked in and hash-bound.
- **Compute and inspect dose** — four-component physical dose (boron,
  nitrogen, hydrogen, photon) with per-voxel uncertainty, DVHs, region
  metrics, NIfTI/PET/DICOM round-trips, and biological interpretation as a
  strictly separate layer.
- **Compare codes, not claims** — import MCNP meshtal or PHITS tallies,
  emit MCNP and PHITS decks, run OpenMC, or solve the in-house
  deterministic S_N path — then diff results with gamma tests,
  metamorphic oracles, and measurement records.
- **Propagate uncertainty** — ENDF MF33 nuclear covariances and Monte
  Carlo statistics fold into a dose-uncertainty budget you can audit
  entry by entry.
- **Iterate on plans** — beam-direction enumeration, adjoint-scored
  ranking, multi-field sweeps, and weight optimization against
  dose-volume objectives (including isoeffective dose).
- **Verify the physics** — a deterministic solver checked against
  closed-form oracles, FiR 1 K63 published measurements, and
  metamorphic invariants — with the failures kept in the record.

## The web workbench

The same Rust code compiles to WebAssembly: drop a dose bundle, exposure
plan, NIfTI volume, or uncertainty budget onto
[openbnct.avilalabs.org](https://openbnct.avilalabs.org) and it lands in
the matching workspace — in English, 日本語, Italiano, 中文, or Español.
No install, no upload — everything runs in your browser.
(Filesystem and process-bound features stay desktop-only and say so.)

Browser needs WebGL2 or WebGPU (Chrome/Edge/Firefox fine; LibreWolf needs
WebGL allowed per-site).

## Install

```text
cargo install openbnct-cli               # CLI from crates.io
pip install openbnct                     # Python bindings from PyPI
```

Desktop builds for Linux, macOS, and Windows are on the
[releases page](https://github.com/AvilaLabs/OpenBNCT/releases/latest).
From source: `cargo build --workspace`, `cargo run --bin openbnct-gui`.

## Honest status

- The 600M-history OpenMC candidate passed every statistical acceptance
  gate — but under the case spec it isn't *the reference* until a second,
  independently implemented transport path reproduces it.
- MCNP/PHITS adapters are verified at benchmark scale on documented-format
  bundles; real-engine acceptance is an open gate.
- The FiR 1 in-phantom comparison deliberately keeps its misses in the
  record — evidence of the fidelity-tier gap, not a hidden failure.
- Nothing here claims clinical qualification, equivalence, commissioning,
  or regulatory suitability.

## Documentation

| | |
|---|---|
| [`docs/USAGE.md`](docs/USAGE.md) | Full command and workflow reference |
| [`ROADMAP.md`](ROADMAP.md) | Evidence-gated status, milestone detail |
| [`benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md`](benchmarks/synthetic/nf-bnct-001/SPECIFICATION.md) | The frozen case and its predeclared gates |
| [`conformance/`](conformance/) | Public fixture suites for every adapter |
| [`docs/adr/`](docs/adr/) | Architecture decision records |
| [`docs/research/`](docs/research/) | Technical baseline and cross-code recipe |

## License

Apache-2.0 for code. The repository must not implement Avify Dose patent
subject matter without an IP review — see
[docs/IP_BOUNDARY.md](docs/IP_BOUNDARY.md).
