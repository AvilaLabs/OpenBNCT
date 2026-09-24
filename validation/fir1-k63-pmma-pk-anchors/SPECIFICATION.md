# FiR 1 PMMA phantom — published-PK anchor validation (R15-04)

Exercises the pharmacokinetic planning layer against the published
anchors it was built for: the JRR rraf038 PK-vs-fixed-T/N dose
comparison and the reviewed BPA blood kinetics. Research-only —
synthetic structures on the homogeneous PMMA phantom, declared curves,
not a clinical case.

## Published anchors

- **Kashino/Kageji critical review** (EJNMMI Radiopharm. Chem.
  lineage): human ¹⁰B-BPA blood concentration after infusion is
  biphasic — first-component t½ 0.7–3.7 h, second 7.2–12.0 h; resected
  tumor T/B mean 3.40±0.83 (melanoma), range 1.4–4.7 (GBM).
- **Savolainen et al.** (Acta Oncol., FiR 1 glioma series): mean blood
  ¹⁰B 24.4 µg/g ±20%; skin convention 1.5×blood.
- **JRR rraf038**: fixed-T/N vs pharmacokinetic dose calculation
  deviated up to 11.386% in the tumor region; an optimized irradiation
  window gained ~4% GTV dose within the same irradiation period.

## Declared curve family

- `blood-model.json`: `C_blood(t) = 7.32·e^(−ln2·t/7200) +
  17.08·e^(−ln2·t/32400)` µg/g — t½ 2 h + 9 h (midpoints of the
  reviewed ranges), amplitudes summing to the published 24.4 µg/g.
- `tissue-spec.json`: GTV T/B(t) = 3.4 − 2.0·e^(−ln2·t/5400)
  (1.4 → 3.4, 90-min uptake — spans the reviewed GBM bound);
  skin pinned at the FiR 1 convention 1.5×blood.
- `tissue-model.json`: `pk tissue-scale` output — the exact
  exponential-family reduction consumed by the scheduler.

## Committed result

`schedule-report.json` (windows 0–3 h, skin mean-dose limit 0.02 Gy on
`component:boron`, source 1e9/s):

| Beam-on epoch | Beam-off (s) | GTV boron dose (Gy) | vs static |
|---|---|---|---|
| +0 s | 1816 | 0.016275 | +7.55% |
| +1800 | 1964 | 0.020121 | +32.97% |
| +3600 | 2116 | 0.023149 | +52.98% |
| +5400 | 2272 | 0.025531 | +68.72% |
| +7200 | 2431 | 0.027406 | +81.11% |
| +10800 | 2761 | 0.030042 | +98.51% |

## Findings vs the anchors

- **PK-vs-fixed deviation**: at the TPS-convention epoch (beam-on at
  infusion end) the PK answer exceeds the static reference by
  **+7.55%** — same sign and order of magnitude as the paper's
  11.386%. The deviation is curve-dependent, not a constant; ours sits
  inside the published scale.
- **Optimal-window gain**: delaying beam-on gains monotonically
  (+84.6% at +3 h) — the paper's mechanism and sign (rising T/B
  rewards waiting) with a larger magnitude, because the declared T/B
  evolution spans a wider dynamic range (1.4→3.4) than their
  patient-specific curves. The window gain is not a literature
  constant; the fixture pins the mechanism, not the number.
- **Skin constraint tightens with delay** (beam-off 1816→2761 s): the
  blood-boron washout that protects normal tissue is exactly what
  lengthens delivery — the paper's stated tradeoff, reproduced
  qualitatively.

`reproduce.sh` regenerates `tissue-model.json` and
`schedule-report.json` with the repository `openbnct` binary.
