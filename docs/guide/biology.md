# Biological models

Biological interpretation starts with physical component dose and an explicit model. OpenBNCT retains separate weighted units and the model identity so those results remain distinguishable from absorbed dose.

## Choose the question and model

Supported research families include fixed component CBE/RBE weighting, photon-isoeffective models, microdosimetric-kinetic and stochastic-MK models. Fractionation, BED/EQD2, combined-treatment analysis and declared TCP/NTCP/UTCP models support further comparisons.

A model's parameters, reference radiation, compound, tissue system, endpoint and validity domain determine what its output means. A named model family alone does not supply those details.

## Regional weights and overlap

For fixed weighting, each component gets a declared factor, with optional regional overrides. Photon-isoeffect weighting requires photon factors of 1.0. Overlapping region masks require explicit priority where the contract demands it; inspect which model applies to a nested target.

The weighted uncertainty follows the declared correlation assumptions. Components derived from shared transport histories should not be interpreted as independent merely because they occupy separate arrays.

## Compare and report

Hold the physical bundle fixed when testing biological model changes. Use model comparisons and sensitivity sweeps to show the effect of parameters. Keep weighted dose and endpoint predictions accompanied by their exact model and assumptions.

Plan optimization's `isoeffective` objectives use fixed component weights. Full nonlinear biological evaluation is a separate path; applying it afterward can change the interpretation of an optimized plan.

Parameter evidence can be inspected through the biological evidence library. A literature-derived research model and a passing software fixture do not establish clinical applicability to a particular tissue or patient.

Commands, models and evidence: [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md) and [biological conformance cases](https://github.com/AvilaLabs/OpenBNCT/tree/main/conformance/bio).
