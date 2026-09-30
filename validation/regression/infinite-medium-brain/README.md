# Case A: infinite brain medium

## Physics

Brain (`materials/brain.json`, 1.04 g/cm3, 293.6 K, S(alpha,beta) on H) in a
4x4x4 grid of 2 cm cells. MC: an 8 cm reflective cube; S_N: `--periodic x,y,z`.
Uniform isotropic volumetric source, density q = 1 /cm3, spectrum as in
`../README.md`. Nothing leaks, so every source neutron is absorbed.

## Normalization (per source neutron)

- S_N `uniform_box` with `statistical_weight_per_site = V = 512 cm3` gives
  q = weight / V = 1 /cm3. Infinite-medium flux is q / Sigma_a per group.
- MC one neutron emitted in V: track length TL = 1 / Sigma_a (cm). The flux
  density is TL / V, so the S_N flux for q = 1 /cm3 equals the raw OpenMC
  track-length tally, NOT divided by V.
- S_N cell flux is the same in every cell (periodic), so the comparison is
  mean S_N cell flux against the raw MC tally, per band.
- Sanity (asserted by the script): analog absorption per source neutron is
  1.000006 +/- 3e-6; S_N `balance_absorbed_fraction / 512` is 0.9999.

## Contents of `reference.json`

`flux_mean_ascending` / `flux_std_ascending` (28 S_N groups, ascending energy),
`band_flux_mean` / `band_flux_std` (thermal, epithermal, fast), absorption.

## Measured at the time of writing

MC bands (cm): thermal 44.27 +/- 0.021, epithermal 6.881 +/- 0.0032, fast
0.8037 +/- 0.0042. S_N/MC (P1 default, 18 outer iterations, debug ~14 s):
thermal 0.972, epithermal 1.042, fast 0.890. The fast deficit is known and
currently unexplained; it is recorded here and inside the +/-20% fast window.
