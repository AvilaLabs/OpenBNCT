# FiR 1 published uncertainty budget — scenario evaluation

Encodes the published FiR 1 / BPA-F uncertainty budget as a discrete
`openbnct.scenario-set` and evaluates it against the committed
transported beam model — the validation half of R16-03. Research-only:
the masks are declared virtual structures on the homogeneous water
phantom; no clinical plan is implied.

## Published anchors

- **Savolainen et al.** (Acta Oncol., FiR 1 glioma series): total
  reference-dose uncertainty 6.0% (1σ) — MCNP model 2.3%, patient
  geometry 3.9%, source distance 3.3%, reference calibration 1.5%;
  blood ¹⁰B 24.4 µg/g ±20% (1σ); skin = 1.5×blood concentration.
- **Kotiluoto et al.** (FiR 1 beam model): 8% (1σ) total computational
  uncertainty at the brain dose maximum, *excluding* boron
  concentration — boron is deliberately the dominant excluded term.
- Published BPA-F tumor/blood uptake ratios span roughly 2.5–4.5;
  encoded as ±30% region scales on the tumor mask.

## Encoding

| Term | Scenario(s) | Mapping |
|---|---|---|
| blood-boron ±20% | `boron-uptake-low/high` | global boron ×0.8/×1.2 |
| beam model ±2.3% | `beam-model-low/high` | `dose_scale` 0.977/1.023 |
| calibration ±1.5% | `calibration-low/high` | `dose_scale` 0.985/1.015 |
| distance 3.3% | `distance-∓2mm` | whole-field ±2 mm shift on the beam axis |
| tumor T/B range | `tumor-uptake-low/high` | tumor-mask boron ×0.7/×1.3 |
| skin factor spread | `skin-uptake-low/high` | entrance-mask boron ×0.67/×1.33 |

Values are ±1σ worst cases, not distributions — the set is a declared
budget, not a probability model.

## Inputs

- `dose-28g-tsl-p1-1e.json` (from `../fir1-k63-cylindrical-phantom/`) —
  the flagship S_N TSL+P1 transported field, per-source-particle Gray.
- `mask-tumor.json` — declared 4 cm target cylinder on axis,
  z ∈ [50, 90] mm (65 voxels).
- `mask-entrance.json` — the beam-facing two-layer shell (632 voxels).
- `objective.json` — `component:boron` quantity: `min_eud` tumor
  1.9e-17, `max_mean` entrance 3.2e-17, zero regularization so the
  baseline sits exactly on the coverage boundary.

## Committed result

`result.json` + `scenario-report.json` (regenerate with
`reproduce.sh`). The bands show the literature's ordering on a real
transported model: tumor coverage is violated only by
`boron-uptake-low` and `tumor-uptake-low`; the entrance limit only by
`boron-uptake-high` and `skin-uptake-high`. The ~2–3% model,
calibration, and distance terms do not move either bound at ±1σ —
boron-uptake uncertainty dominates, consistent with Kotiluoto's
framing of boron as the excluded dominant term.
