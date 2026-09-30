# Case B: 1-D brain column

## Physics

Brain, a 1x1x25 column of 0.8 cm cells (20 cm long). Transverse faces
reflective (MC) / `--periodic x,y` (S_N), vacuum at z=0 and z=20 cm. A
monodirectional +z beam enters at z = 0 with the spectrum of `../README.md`.

- S_N: `uniform_disk` r = 0.6 cm centred on the cell, `monodirectional +z`. The
  disk (1.131 cm2) circumscribes the 0.8 x 0.8 cm cell (corner distance
  0.566 cm), so the cell is fully illuminated; the solver deposits an areal
  density of 1 / pi r2 per source neutron.
- MC: the same neutrons distributed uniformly over the one cell face.

## Normalization

- Let s be the areal source density on the face (n/cm2). The flux per unit s in
  a voxel of thickness dz is TL / dz, where TL is the OpenMC track length in
  that voxel per emitted neutron over an area A: s = 1 / A and flux density is
  TL / (A dz), so flux per unit s is TL / dz. `reference.json` stores TL / dz
  (`flux_mean_voxel_x_group_ascending`, `band_flux_mean_voxel_x_band`).
- S_N emits R = 1 over the disk, so its flux density is (1 / pi r2) times the
  flux per unit s. Comparable quantity: S_N flux x `disk_area_cm2` (pi r2).
- Absorption: the solver's `balance_absorbed_fraction` counts the neutrons that
  fall on the modelled cell, a fraction cell_area / disk_area = 0.566 of R, so
  absorbed per neutron entering the cell = balance x disk_area / cell_area.
  MC: analog absorption per emitted neutron.
- Sanity (asserted by the script): absorbed + leaked = 1 within 0.5%
  (absorption 0.4338, front leakage 0.5556, back 0.0106); the first-voxel fast
  flux per fast source neutron is 1.30, above the analytic uncollided value
  0.936 (from the library total cross sections) and within 1.6x of it, i.e.
  the column entrance is uncollided-dominated and the normalization is right
  (scattered neutrons add track length as ~1/mu). The S_N first-voxel fast
  flux agrees to 2%.

## Measured at the time of writing

MC absorption 0.4338 +/- 0.0002. S_N/MC (P1 default, `--max-outer 128`; the
default 32 outer sweeps stop at residual 1.7e-4; 48 outers needed):

| Band | column total | depth zones (voxels 0-7 / 8-16 / 17-24) |
|---|---|---|
| thermal | 0.912 | 0.829 / 1.125 / 1.636 |
| epithermal | 1.052 | 1.047 / 1.278 / 0.908 |
| fast | 0.964 | 1.025 / 0.951 / 0.846 |

Absorption per source neutron entering the cell: S_N 0.411, ratio 0.947. Forced
`--p0` gives ratio 0.71 (absorption 0.307), thermal total 0.68.
