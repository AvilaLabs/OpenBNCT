# PK-aware irradiation time — `nf-bnct-003`

Demonstration of `openbnct irradiation-time --pk-model`: the boron dose
component is integrated under a declared concentration curve and the
beam-off time solves `D(t) = limit` implicitly, replacing the fixed
concentration assumption every shipped BNCT TPS makes (a published
PK-vs-fixed-T/N comparison shows deviations up to ~11% in tumor dose;
J. Radiat. Res. rraf038).

This case is boron-loaded (`dose-theta-wdd.json` carries only a boron
component), so it isolates the PK mechanism exactly: the whole endpoint
scales with `C(t)/C_plan`.

- `whole-phantom-mask.json` — all-640-voxel RegionMask.
- `pk-model-bpa-washout.json` — `openbnct.pk-model/0.1.0`: C(t) =
  30·e^(−0.02t) ppm (synthetic monoexponential washout for
  demonstration; not a fitted clinical PK).
- `result-pk-irradiation.json` — `openbnct.pk-irradiation-report/0.1.0`.

## Reproduce

```sh
openbnct irradiation-time \
  --dose benchmarks/synthetic/nf-bnct-003/transport/dose-theta-wdd.json \
  --quantity physical_total --source-strength 1e5 \
  --limit phantom=mean:10 \
  --mask phantom=benchmarks/synthetic/nf-bnct-003/planning/whole-phantom-mask.json \
  --pk-model benchmarks/synthetic/nf-bnct-003/planning/pk-model-bpa-washout.json \
  --output benchmarks/synthetic/nf-bnct-003/planning/result-pk-irradiation.json
```

Result: static answer 18.51 s; PK answer 23.12 s (+24.9%) — washout
carries the region to 26% of its planned concentration by beam-off, so
the beam must stay on ~25% longer to reach the same endpoint. The
closed-form check: `I(t) = (C₀/λ)(1−e^(−λt))/C_plan` with
`S·B·I(t) = 10` gives `t = −ln(1 − 10λ/(S·B))/λ = 23.12 s` — the solver
reproduces the analytic answer to machine precision.

Omit `--pk-model` to recover the constant-concentration answer. Regions
without a declared curve fall back to constant concentration.

Research software only — the PK model is a declared input with a stated
basis, not a fitted or clinically qualified pharmacokinetic estimate.
