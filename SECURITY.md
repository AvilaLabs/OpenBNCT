# Security Policy

## Supported versions

OpenBNCT is research software and is not approved for clinical use (see
[`docs/DISCLAIMER.md`](docs/DISCLAIMER.md)). Security fixes land on `main` and
ship in the next release; only the latest release published on crates.io,
PyPI, and the GitHub releases page is supported.

## Reporting a vulnerability

Report suspected vulnerabilities privately through GitHub's private
vulnerability reporting: open this repository's **Security** tab and choose
**Report a vulnerability**
(<https://github.com/AvilaLabs/OpenBNCT/security/advisories/new>). Do not open
a public issue for a suspected vulnerability.

Do not include patient information, credentials, facility secrets, or
weapon-relevant operational data in a report — describe the problem and how
to reproduce it with synthetic inputs.

## Scope

In scope: the Rust crates, the `openbnct` CLI, the desktop and web builds of
the workbench, the `openbnct` Python package, and this repository's release
and CI workflows. External engines and data (OpenMC, NJOY, MCNP, PHITS,
nuclear-data libraries) are out of scope — report issues in those upstream.
