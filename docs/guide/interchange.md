# Import, export and compare

OpenBNCT exchanges explicit component dose and geometry so independent calculations can be compared meaningfully. Preserve normalization, spatial frame, component definitions and available uncertainty when importing another engine's output.

## Supported exchange paths

| Data | Research workflow |
| --- | --- |
| Component NIfTI | Import/export separate physical components and sigma volumes |
| MCNP | Export a declared deck; import supported meshtal output |
| PHITS | Export a declared deck; import supported component output |
| DICOM RT Dose | Import/resample supported Gy fields or export declared results |
| Static-beam RT Plan | Read/export supported plan summaries and fields |
| CSV/XLSX | Exchange validated exposure plans |

Parser conformance establishes behavior on the committed fixtures. It does not establish real-engine agreement across arbitrary MCNP/PHITS runs.

## Compare bundles

`compare` produces component-wise dose comparisons; `gamma` evaluates declared dose/distance criteria. Use `openbnct compare --help` and `openbnct gamma --help` for your installed version, then consult the [worked command reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md).

Set the reference explicitly. Match geometry or apply a declared resampling operation; check units, normalization and masks. Monte Carlo sigma can explain a difference's statistical precision, but cannot remove a systematic modeling difference.

Gamma depends on dose normalization, dose-difference percentage, distance-to-agreement, low-dose cutoff and voxel inclusion. Report those settings with the pass rate. A single gamma percentage can conceal component errors, so retain the component ratios and spatial distributions too.

## Keep a research record

Project reports list their inputs and commands. Preserve the project configuration, exact data and result artifacts alongside exported views. Input changes invalidate the old comparison until recalculated.

Contracts and fixtures: [schemas](https://github.com/AvilaLabs/OpenBNCT/tree/main/schemas) and [conformance](https://github.com/AvilaLabs/OpenBNCT/tree/main/conformance).
