# FiR 1 K63 PMMA-phantom deterministic depth-profile check

Second like-for-like deterministic comparison, on the measured PMMA
cylinder — same Ø20 × 24 cm form factor as the water check in
`../fir1-k63-cylindrical-phantom/`, different composition
((C5H8O2)n, ρ = 1.19 g/cm³). Two phantom materials measured on the same
beam gives an independent cross-check of the declared H1/C12/O16
collapse.

## What is here

| file | content |
|---|---|
| `case.json` | `openbnct.transport-case/0.1.0` — same 24×24×26 grid / 10 mm spacing and embedded FiR 1 K63 source as the water check. |
| `assignment.json` | `openbnct.material-assignment/0.2.0` — same voxel-set cylinder geometry, PMMA material (C5H8O2 with natural isotopic splits + dilute trace ¹⁰B/¹⁴N, ρ = 1.19 g/cm³). |
| `multigroup-data.json` | `openbnct.multigroup-data/0.1.0` — three-group PMMA data; H1/C12/O16 σ_s and σ_a from the processed ENDF/B-VIII.1 294 K library at 0.0253 eV / ~70 eV / ~1.3 MeV representative energies; Fermi downscatter removal; boron response at the notional 10 µg/g basis scaled by ρ. Full declaration in the artifact. |
| `multigroup-data-28g.json` | `openbnct.multigroup-data/0.1.0` — real-data 28-group `sn collapse` output for the PMMA material (same group structure and pipeline as the water check), including `transport_mu_bar`. |
| `multigroup-flux-28g.json` / `dose-28g.json` | S₈ 28-group solve under the cone uncollided split + consistent transport correction. **Convention note:** this pair predates the collapse-consistent within-bin source weighting; its flux artifact carries the deserialization default `source_spectrum_weighting: "uniform_in_bin"`. |
| `multigroup-flux-28g-1e.json` / `dose-28g-1e.json` | The current-convention rerun (`collapse_consistent` weighting, cone split + transport correction) — the reference pair for the 28-group result below. |
| `beam-quality-pmma-28g-1e.json` / `measurement-comparison-pmma-28g-1e.json` | QA report (absolute + transverse profiles) and both comparisons for the `-1e` solve. |
| `multigroup-data-28g-tsl-v2.json` | 28-group PMMA data with the ENDF/B-VIII.1 `H(Lucite)` TSL bound-atom kernel — upscatter + bound-atom transfer moments. |
| `multigroup-flux-28g-tsl-p1-1e.json` / `dose-28g-tsl-p1-1e.json` | The TSL+P1 solve pair — S₈, converged at residual 9.9e−5 after 116 outer iterations (TSL upscatter coupling). |
| `beam-quality-pmma-28g-tsl-p1-1e.json` / `measurement-comparison-pmma-28g-tsl-p1-1e.json` | QA report and peak-normalized + absolute comparisons for the TSL+P1 solve. |
| `beam-quality-pmma.json` | `openbnct.beam-quality/0.1.0` — folded `beam qa` output. |
| `measurement-comparison-pmma.json` | `openbnct.measurement-comparison/0.1.0` — against `measurements/fir1-k63-pmma-phantom-depth.json` (the PMMA series of the same TECDOC-1223 FIG. 3 digitization, re-homed under the canonical metric name). |

## Reproduce

Same commands as `../fir1-k63-cylindrical-phantom/README.md` with
`fir1-k63-pmma-phantom` paths and report id
`openbnct.beam-quality.fir1-k63-pmma.v1`; the comparison binds
`measurements/fir1-k63-pmma-phantom-depth.json`.

## Result

Peak-normalized PMMA thermal-fluence depth profile (three-group
fixture data): **PASS** — χ² = 6.04 over 15 bins, every bin within
~1.15σ of the digitized measured values. Agreement is near-exact
through 2.4–6.5 cm (rel. diff ≤ 9%) and stays within σ in the tail
(max 35% at the deepest bin, where measured σ is large). The original
TECDOC-1223 study computed its reference curves with DORT — this
check reproduces the measured shape with the same discrete-ordinates
method family.

28-group ENDF-collapsed result (`*-1e`, cone split + consistent
transport correction): **near-pass on absolute scale** — every bin
within ~32% of the measured absolute thermal fluence and within 1σ at
both ends of the profile; only the mid-depth bins (z ≈ 3–5 cm) exceed
the 2σ tolerance (max 2.4σ, ratio ≈ 1.32). Normalized χ² = 20.8,
absolute χ² = 28.3 over 15 bins. This is markedly tighter than the
water-phantom bracket — consistent with PMMA's smaller hydrogen-driven
forward-scatter error.

28-group TSL+P1 result (`*-tsl-p1-1e`, `H(Lucite)` bound-atom kernel +
P1): **best deterministic PMMA profile** — normalized χ² = 18.8, with
every bin through z ≈ 5 cm within ~±5% of the measured shape (the −1e
solve peaked at 1.32). Absolute scale sits at a uniform ~0.5–0.65×
through mid-depth — the same source-normalization deficit seen on the
water phantom (the 3-bin source histogram stands in for the untabulated
measured FiR 1 spectrum); the deep tail beyond ~9 cm still falls to
0.2–0.4× measured. Cross-composition consistency: the TSL+P1 normalized
χ² is essentially identical on water (18.8) and PMMA (18.8).

## Scope

Research-grade cross-check only; declared three-group fixture data and a
figure-digitized measurement series. Not clinical validation or
commissioning evidence.
