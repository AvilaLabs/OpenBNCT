# Cell-microdosimetry literature anchor — Sato et al. 2018 (R14-03)

Anchors the compartmented-cell sampler and SMK evaluation against the
published PHITS microdosimetry scenario and fitted parameters of Sato,
Masunaga, Kumada & Hamada, *Sci Rep* **8**, 988 (2018) — the same
stochastic-microdosimetric-kinetic model family this crate implements.
Research-only; declared distributions, not measured data.

## Published anchors used

- **Geometry**: cell radius 5 µm, nucleus radius 3 µm, concentric —
  the paper's concentric-sphere case (their eccentric case with 1.5 µm
  offset is not representable in the declared model).
- **BPA fractions** (`bpa-microdistribution.json`): N_n = N_s = 0,
  N_c = 0.78, N_e = 0.22 — amino-acid-transporter uptake distributes
  in cytoplasm, not nucleus; intra/extra ≈ 3.2 measured.
- **BSH fractions** (`bsh-microdistribution.json`): N_n = N_c = 0,
  N_s = 0.48, N_e = 0.52 — BSH does not cross the cell membrane;
  intra/extra ≈ 0.86. The paper's "cell surface" maps to this model's
  membrane compartment.
- **Published conversion factors** (nucleus absorbed dose, declared
  distribution vs homogeneous ¹⁰B — their κ_B, concentric case):
  **0.82 for BPA, 0.45 for BSH**.
- **Published fitted SMK domain parameters** (SCC VII cells):
  α₀ = 0.0422 Gy⁻¹, β₀ = 0.00822 Gy⁻² (γ₀ = 4.33 h⁻¹, r_d = 0.24 µm
  unused by the instantaneous-irradiation survival integral).
- **Particle ranges**: α 1.47 MeV / 9 µm, ⁷Li 0.84 MeV / 5 µm — the
  dominant (93.7%) capture branch, conventional unit-density-tissue
  CSDA values.
- `extracellular_extent_um` = 6 µm — close-packed lattice half-gap
  (declared; the paper's lattice spacing is not explicitly tabulated).

## Committed result

`{bpa,bsh}-correction.json` — `microdistribution evaluate`:
- BPA nucleus dose factor **0.796** vs published κ_B **0.82** (−3%).
- BSH nucleus dose factor **0.420** vs published κ_B **0.45** (−7%).

Both inside the declared 1σ of the model's own fraction uncertainty
and consistent with the expected CSDA-vs-full-transport difference.

`{bpa,bsh,bpa-hetero}-cell-microdosimetry.json` + `*-smk-evaluation.json`
— 20 000 cells, 20 mean captures ↔ 5 Gy boron kerma, published α₀/β₀,
SMK-vs-MK comparison and isosurvival RBE per dose level:

- **S_BPA < S_BSH at equal dose** (0.420 vs 0.642 at 8 Gy) — the
  paper's headline result, driven by the larger BPA conversion factor.
- **Intercellular heterogeneity raises high-dose SF**: a declared
  CV 0.4 (inside the paper's studied σ 0–0.6) lifts BPA survival
  0.420 → 0.448 at 8 Gy with negligible change at 1 Gy — their
  Fig. 7/8 finding.
- **Untouched fraction** BSH 2.7% > BPA 0.2% — membrane/extracellular
  binding leaves cells the short-range products cannot reach.
- **SMK diverges from MK at high dose** (0.420 vs 0.382 at 8 Gy) —
  the stochastic survival plateau the paper motivates.

`reproduce.sh` regenerates all committed outputs.
