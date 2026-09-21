# Prompt-gamma source artifacts

`layered-head-prompt-gamma-478kev.json` — the voxelwise 478 keV
prompt-gamma production field for the ¹⁰B(n,α)⁷Li* de-excitation,
derived from the layered-head-phantom S₈ physical dose bundle
(`benchmarks/synthetic/layered-head-phantom/dose-28g.json`):

```text
openbnct prompt-gamma \
  --dose benchmarks/synthetic/layered-head-phantom/dose-28g.json \
  --id openbnct.layered-head-phantom.prompt-gamma.v1 \
  --output layered-head-prompt-gamma-478kev.json
```

`openbnct.prompt-gamma-source/0.1.0` carries the 94% branching ratio,
isotropic-emission convention, `photons_per_kg_per_source_particle`
units, and a `parent` content binding to the exact dose bundle — the
detector/SPECT reconstruction consumes this as its emission source term.
3,695 of the 15,625 voxels emit (the boron-bearing region).

Research artifact only — not a clinical imaging or dosimetry claim.
