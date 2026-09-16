# OpenBNCT conformance suites

Public, implementation-neutral test vectors for OpenBNCT's published
interchange contracts. External transport pipelines (MCNP, PHITS, Geant4,
custom tools) can run these fixtures against their own exporters; the
authoritative Rust importer is continuously tested against the same files.

## `interchange/0.1.0/` — component-dose interchange

Covers `openbnct.component-dose-interchange/0.1.0` →
`openbnct.physical-dose-bundle/0.2.0` import. `manifest.json` lists every
case with its expected outcome:

- `expect: "ok"` — the document must import; `expected/` holds the
  reference bundle the authoritative importer emits.
- `expect: "reject"` — the document must be refused; `error` names the
  stable rejection token (see below).

Documents are byte-fixed: bundle `provenance_id` and fallback content
references embed each document's SHA-256, so reformatting a fixture changes
the expected output.

### Rejection tokens

| token | condition |
| --- | --- |
| `unsupported_schema` | `schema_version` is not the covered schema |
| `empty_identifier` | `case_id`, `producer.system`, `producer.version`, or `producer.normalization` is blank |
| `geometry` | grid fails `GridGeometry` validation (e.g. non-orthonormal direction) |
| `no_components` | `components` is empty |
| `duplicate_component` | a `DoseComponent` kind appears more than once |
| `missing_component` | a required kind (boron/nitrogen/hydrogen/photon) is absent |
| `inconsistent_units` | components do not share one `DoseUnit` |
| `dose_length` | component `values` length differs from the voxel count |
| `invalid_dose` | component carries a non-finite or negative value |
| `uncertainty_length` | component sigma length differs from the voxel count |
| `invalid_uncertainty` | component carries a non-finite or negative sigma |
| `total_length` | dedicated total length differs from the voxel count |
| `invalid_total` | dedicated total carries a non-finite or negative value |
| `total_uncertainty_length` | dedicated total sigma length differs from the voxel count |
| `invalid_total_uncertainty` | dedicated total carries a non-finite or negative sigma |
| `invalid_reference` | a declared `component_profile`/`response_set` fails content-reference validation |
| `bundle` | the assembled bundle fails `PhysicalDoseBundle` contract validation |

### Running the suite

```text
cargo test -p openbnct-core --test interchange_conformance
```

After an intentional importer change, regenerate the reference bundles with
`OPENBNCT_UPDATE_CONFORMANCE=1` on the same command, review the diff, and
commit the updated fixtures.

## `adapters/0.1.0/` — producing-system parser outputs

Covers the whole adapter path: producing-system file →
`openbnct.component-dose-interchange/0.1.0` document →
`openbnct.physical-dose-bundle/0.2.0` bundle. `manifest.json` names, per
case, the input files, the component selections (`file`, MCNP `tally`,
optional `energy_bin`), the declared `unit`/`normalization`/`case_id`, and
byte-fixed `document` and `bundle` references.

- `mcnp/` — one ASCII meshtal carrying four component tallies (energy
  column, `Rel Error` → absolute sigmas) on a shared mesh.
- `phits/` — per-component `xyz`-mesh `t-deposit` `.out` files with
  `*_err.out` siblings; `axis=xy` z-slice pages; `unit = 0`.
- `nifti/` — per-component scalar `.nii` volumes plus paired `sigma_file`
  absolute-uncertainty volumes on a shared grid, the shape OpenPINT's
  per-component NIfTI export produces. NIfTI carries no producer
  identity, so the case declares `producer_system` explicitly.

Input fixtures are authored to the documented file formats — they are
parser fixtures, not real MCNP/PHITS executions. `sources[].file` is
relative to the suite directory and is embedded verbatim in the generated
document's normalization trail; run each crate's suite from anywhere, the
test enters the suite directory itself.

```text
cargo test -p openbnct-mcnp --test adapter_conformance
cargo test -p openbnct-phits --test adapter_conformance
cargo test -p openbnct-nifti --test adapter_conformance
```

Regeneration uses the same `OPENBNCT_UPDATE_CONFORMANCE=1` convention.

## `bio/0.2.0/` — biological model application

Covers `openbnct.biological-model/0.2.0` →
`openbnct.biological-dose-bundle/0.2.0` application. `manifest.json` names a
model, a physical dose bundle, and region masks per case; `expected/` holds
the reference biological bundles. One case per model family:

- `fixed-weights` — `fixed_per_component` with a core-region override
- `isoeffective` — `photon_isoeffective` per-region weights, unfractionated
- `isoeffective-fractionated` — LQ fractionation with a region α/β override
  (the `weighted_eqd2` total)

Reject cases pin the stable tokens `missing_region_mask`,
`unsupported_schema`, and `unit_mismatch`. Fixture weights and α/β ratios
are analytic stand-ins for conformance checking — not clinical model
parameters.

```text
cargo test -p openbnct-bio --test bio_conformance
```

## `bio/mkm-0.1.0/` — microdosimetric model application

Covers `openbnct.microdosimetric-model/0.1.0` →
`openbnct.biological-dose-bundle/0.2.0` application, including
`openbnct.lineal-spectrum/0.1.0` spectrum inputs. The shared physical
bundle and region masks are reused from `bio/0.2.0/` via relative paths,
and a case model's `derivation` reference is verified against
`cases/<id>.json` so provenance is hash-bound, not merely recorded:

- `published` — HSG-like LQ parameters (Kase 2006/2008) with constant
  component lineal energies
- `spectrum` — boron ȳ_D resolved from a supplied lineal spectrum
- `fractionated` — MKM photon-equivalence per fraction then the EQD2
  rescale (the `mkm_weighted_eqd2` total)

Reject cases pin `unresolved_spectrum`, `unsupported_schema`, `invalid`
(empty validity domain), `unit_mismatch`, and `missing_region_mask`.
Component lineal energies are representative stand-ins — not clinical
model parameters.

```text
cargo test -p openbnct-bio --test mkm_conformance
```

## `endpoints/0.1.0/` — endpoint model scoring

Covers `openbnct.endpoint-model/0.1.0` →
`openbnct.endpoint-evaluation/0.1.0` scoring plus `combine_utcp`.
`manifest.json` names a model (or TCP/NTCP model pair for `utcp` cases),
a self-contained dose input, and a region mask per case; `expected/`
holds the reference evaluations. Cases cover `logistic`, `probit`
(Lyman), `voxel_poisson_tcp`, EUD statistics, and both `p_plus` and
`difference` UTCP combinations.

Reject cases pin the stable tokens `invalid_model`,
`per_source_particle_unit`, `dose_selection`, and `utcp_mismatch`.
All model parameters are analytic stand-ins for conformance checking —
not clinical response models.

```text
cargo test -p openbnct-bio --test endpoint_conformance
```

Regeneration uses the same `OPENBNCT_UPDATE_CONFORMANCE=1` convention.

Research only: conformance here means contract fidelity — it does not
qualify any producer's physics or imply clinical suitability.
