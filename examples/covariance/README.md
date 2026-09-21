# Evaluated nuclear-data covariance demonstration

These two artifacts show the complete evaluated-covariance → dose
uncertainty chain on real ENDF data — not a declared demonstration
covariance:

1. `b10-endfb81-mt1-nf-bnct-003.covariance.json` —
   `openbnct.multigroup-covariance/0.1.0` derived from the ENDF/B-VIII.1
   ¹⁰B evaluation's MF33 MT=1 (total cross section) LB=5 covariance
   matrix, collapsed onto the NF-BNCT-003 one-group mesh: **2.001%**
   evaluated σ on the thermal-group removal cross section.

2. `nf-bnct-003-endf-b10-budget.json` —
   `openbnct.dose-uncertainty-budget/0.1.0` from `uq propagate`: the
   boron dose integral carries σ_rel = **1.911%**, 100% attributable to
   the evaluated ¹⁰B σ_t covariance (finite-difference sensitivities of
   the discrete S_N operator).

Reproduce:

```text
openbnct openmc cov-endf \
  --tape n-005_B_010.endf \            # ENDF/B-VIII.1 10B evaluation
  --data benchmarks/synthetic/nf-bnct-003/transport/multigroup-data.json \
  --material openbnct.nf-bnct-003.material.ideal-b10-absorber.v1 \
  --mt 1 --parameter sigma_total \
  --output b10-endfb81-mt1-nf-bnct-003.covariance.json

openbnct uq propagate \
  --case benchmarks/synthetic/nf-bnct-003/transport/case.json \
  --data benchmarks/synthetic/nf-bnct-003/transport/multigroup-data.json \
  --covariance b10-endfb81-mt1-nf-bnct-003.covariance.json \
  --component boron \
  --id openbnct.nf-bnct-003.endf-budget.v1 \
  --output nf-bnct-003-endf-b10-budget.json
```

`--parameter dose_response --component boron` binds a reaction MT
covariance to that component's dose-response vector instead of σ_t —
demonstrated by the second pair of artifacts, which carry the actual
BNCT channel:

3. `b10-endfb81-mt107-layered-head.covariance.json` — ENDF/B-VIII.1
   ¹⁰B MF33 **MT=107 (n,α)** covariance. ENDF expresses it as NC-type
   LTY=0 references to derived-quantity sections MT=800/801; the reader
   resolves the references and combines their LB=5 covariances, then
   collapses to the layered-head 28-group mesh as a `dose_response`
   block on the boron component (0–1.7% per group; groups outside the
   evaluated covariance mesh carry zero, meaning *no evaluated data*,
   not *zero uncertainty*).

4. `layered-head-endf-b10na-budget.json` — the propagation result on
   the S₈ layered-head case: boron dose integral σ_rel = **0.34%**
   entirely from the evaluated (n,α) covariance (dose-response
   sensitivities are analytic — zero perturbed solves).

Scope note: the MF33 reader handles NI-type sub-subsections with LB ∈
{0,1,5} plus NC-type LTY=0 (coefficient×referenced-section resolution);
LTY ∈ {1,2,3} cross-material covariances and LB ∈ {2,3,4,6} are
skipped with an explicit ledger entry in `provenance_note` rather than
misread.

Research artifacts only — not evaluated-data qualification.
