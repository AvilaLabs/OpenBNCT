# openbnct

Facade crate re-exporting the OpenBNCT library crates under stable
namespaced paths:

| Path | Crate |
|---|---|
| `openbnct::contracts` | `openbnct-core` |
| `openbnct::transport` | `openbnct-transport` |
| `openbnct::evidence` | `openbnct-evidence` |
| `openbnct::bio` | `openbnct-bio` |
| `openbnct::boron` | `openbnct-boron` |
| `openbnct::dicom` | `openbnct-dicom` |
| `openbnct::mcnp` | `openbnct-mcnp` |
| `openbnct::nifti` | `openbnct-nifti` |
| `openbnct::njoy` | `openbnct-njoy` |
| `openbnct::openmc` | `openbnct-openmc` |
| `openbnct::phits` | `openbnct-phits` |
| `openbnct::plan` | `openbnct-plan` |
| `openbnct::view` | `openbnct-view` |

This crate adds no API of its own; depend on the individual crates when
you need only part of the workspace. The binaries are `openbnct-cli`
(`cargo install openbnct-cli`) and `openbnct-gui`.

Research software — not for clinical use. See the repository's
`DISCLAIMER.md` and `ROADMAP.md`.
