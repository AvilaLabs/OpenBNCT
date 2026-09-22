# Contributing

OpenBNCT is currently an Avila Labs research scaffold. Contributions should be
small, reviewable, independently testable, and explicit about scientific and
licensing provenance.

## License and certification

All contributions are submitted under the MIT license. Commits must include a
Developer Certificate of Origin sign-off:

```text
Signed-off-by: Your Name <your.email@example.com>
```

By signing off, contributors certify that they have the right to submit the
work under the project license. Do not submit confidential information,
patient data, employer-owned code without authorization, OpenPINT source, or
code copied from another implementation.

## Scientific changes

Changes affecting geometry, transport, physical dose, biology, uncertainty, or
qualification must include:

- a source or technical rationale;
- units and coordinate conventions;
- valid and invalid test cases;
- predeclared acceptance tolerances;
- known limitations;
- a statement of whether reference results are independent.

A successful build is not evidence of physical correctness.

Changes to generated DICOM metadata must also pass the external validator gate:

```text
scripts/validate-dicom-iod.sh PATH-TO-GENERATED-NF-BNCT-001
```

CI supplies the pinned `dciodvfy` and `dcentvfy` binaries.

## Patent-sensitive contributions

Read `docs/IP_BOUNDARY.md` before opening an issue or contribution related to
boron-distribution optimization, robust bounds, extremal maps, recomposition,
or certificates. Those areas require an Avila Labs patent review first.

## Patient and facility data

Do not submit patient data, re-identifiable metadata, proprietary beam models,
or facility measurements without documented authorization and a release plan.
Public test data must be synthetic or explicitly licensed for redistribution.
