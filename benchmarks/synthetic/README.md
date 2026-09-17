# Synthetic benchmark corpus

Only deliberately generated, non-patient data may be committed here.

Every benchmark must eventually include:

- generator source and version;
- coordinate-frame and unit definitions;
- expected physical components and uncertainty;
- reference method and tolerances;
- provenance hashes;
- explicit data license;
- known limitations and qualification boundary.

Binary benchmark data are generated on demand and are not committed by default.
The generator, frozen specification, independent oracle, and rejection tests
are source-controlled.

## Specified cases

- [`NF-BNCT-001`](nf-bnct-001/SPECIFICATION.md) freezes the first synthetic
  DICOM geometry, macroscopic material, source, dose semantics, uncertainty
  rules, and preregistered acceptance gates. Geometry, material, source,
  nuclear-data preflight, and deterministic OpenMC deck generation are
  implemented. Its first controlled NJOY execution is preserved as rejected
  evidence, and its transported-photon suitability report rejects four source
  evaluations; KERMA response tables and reference outputs remain intentionally
  unqualified.
- [`NF-BNCT-002`](nf-bnct-002/SPECIFICATION.md) extends the library into
  deep-penetration heterogeneous transport: a 30 cm cube with a
  skull-equivalent slab on the incident face and a high-boron tumor insert
  on axis, driven by a declared 1/E epithermal disk source. Machine inputs
  (frozen case, three materials, voxel assignment, epithermal source,
  acceptance contract) are committed; execution is pending its
  material-bound response set.
- [`NF-BNCT-003`](nf-bnct-003/SPECIFICATION.md) is the library's analytic
  oracle: a 0.0253 eV monodirectional beam into a near-pure ¹⁰B absorber,
  where the boron component follows `exp(−Σ_t·z)` to a documented bound.
  The declared expectation lives in `transport/analytic-oracle.json` and is
  checked by `openbnct analytic` — a closed-form ground truth rather than
  another code's output. `transport/multigroup-data.json` is the declared
  one-group data set for the deterministic S_N solver; `openbnct sn solve`
  on this case folds a boron dose that reproduces the oracle's 0.2308 cm⁻¹
  slope to 0.000% relative deviation — the cross-method check between the
  deterministic and Monte Carlo transport paths.

Evaluate an analytic oracle against a dose bundle with:

```text
cargo run --bin openbnct -- analytic \
  --oracle benchmarks/synthetic/nf-bnct-003/transport/analytic-oracle.json \
  --dose DOSE.json --id EVAL-ID --output evaluation.json
```

Generate and verify NF-BNCT-001's DICOM geometry inputs with:

```text
cargo run --bin openbnct -- benchmark generate /tmp/nf-bnct-001
cargo run --bin openbnct -- benchmark verify /tmp/nf-bnct-001
```
