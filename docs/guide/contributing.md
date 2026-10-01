# Contribute and cite

Start with [CONTRIBUTING.md](https://github.com/AvilaLabs/OpenBNCT/blob/main/CONTRIBUTING.md). The Rust crates are authoritative; GUI and Python operations reuse those implementations. Preserve versioned contracts, explicit units and qualification boundaries.

Frozen benchmark and validation artifacts remain immutable. New solver results need separately identified inputs, execution and evaluation evidence. Routine CI verifies the committed catalogue and qualification bindings without regenerating reference data.

Development gates include formatting, Clippy, workspace tests, WebAssembly checks, Python parity and independent DICOM checks. On the repository owner's workstation, follow its resource limits and process-lifecycle rules in [AGENTS.md](https://github.com/AvilaLabs/OpenBNCT/blob/main/AGENTS.md).

Handbook changes build with mdBook 0.5.4. CI checks chapter membership, source references, local links, navigation, search, themes and mobile layout. See the [documentation maintainer guide](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/maintainers/DOCUMENTATION.md) for publishing.

## Cite a study

Use [CITATION.cff](https://github.com/AvilaLabs/OpenBNCT/blob/main/CITATION.cff), also available through GitHub's **Cite this repository** control. Include the source commit or release, case/data versions, solver configuration, normalization, biological/boron assumptions and comparison criteria alongside reported results.

OpenBNCT is [MIT-licensed](https://github.com/AvilaLabs/OpenBNCT/blob/main/LICENSE). Third-party data and dependencies retain their terms; preserve the associated notices when redistributing artifacts.
