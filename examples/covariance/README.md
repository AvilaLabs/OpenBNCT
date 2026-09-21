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
(e.g. MT=107 n,α) covariance to that component's dose-response vector
instead of σ_t. Scope note: the MF33 reader handles NI-type
sub-subsections with LB ∈ {0,1,5}; NC-type parameter covariances (the
form ENDF/B-VIII.1 uses for ¹⁰B MT=107) are skipped with an explicit
ledger entry in `provenance_note` rather than misread.

Research artifacts only — not evaluated-data qualification.
