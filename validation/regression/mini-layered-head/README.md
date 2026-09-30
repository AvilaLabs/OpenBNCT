# Case C: mini layered head

## Geometry

Derived from the 25^3 x 8 mm `layered-head-phantom` assignment (skin 2 cells,
skull 1 cell, brain core) by 3x3x3 majority vote into 9x9x9 voxels of 24 mm:

- Void padding to 27 fine cells per axis: x and y one cell each side (so the
  beam axis x = y = 0 is the centre of block 4); z two cells on the high side
  (the beam-entrance face z = 0 stays on the grid face).
- Winner = most frequent material in the 27-cell block; ties go to
  skull > skin > brain > void (keeps the thin skull; 21 skull voxels survive:
  137 skin, 21 skull, 142 brain).
- `sn-case.json` / `sn-assignment.json` are written by the script and the
  OpenMC model is built from the same voxel grid (RectLattice, void universe
  for empty voxels, vacuum box).

Beam: uniform disk r = 7 cm on z = 0, isotropic cone half-angle 0.1491 rad about
+z, spectrum of `../README.md`. Materials: skin, skull, brain (S(alpha,beta) on H).

## Why 24 mm

The full 25^3 phantom needs ~3 min in a release build. A 13x13x13 x 16 mm grid
was measured at ~300 CPU-seconds in a debug build (uncollided-beam ray trace
scales with cell count and dominates), too slow for CI. 9^3 x 24 mm costs ~100
CPU-seconds. The price is an optically thick grid (sigma_t dx ~ 8 for thermal
neutrons in tissue), which biases S_N low against MC by a deterministic amount
(see below); the test windows are centred on that measured bias.

## Normalization

- Flux: OpenMC track length in the voxel per source neutron divided by the voxel
  volume (24 mm cube = 13.824 cm3), in 1/cm2 per source neutron: the same
  quantity as the S_N `flux` artifact (R = 1). Three bands: 1e-5..0.5 eV,
  0.5 eV..10 keV, 10 keV..16.9 MeV.
- B10 and N14: OpenMC `absorption` per nuclide, events per voxel per source
  neutron (not per cm3). S_N: dose bundle component `boron` / `nitrogen`
  (Gy per source particle) converted with D rho V / E, rho = 1.09 / 1.92 /
  1.04 g/cm3 (skin / skull / brain), E = 2.34 MeV (B10(n,alpha)) and
  0.626 MeV (N14(n,p)), the values the S_N data declares.
- Flat voxel index i + 9 j + 81 k (x fastest) in both codes; the script checks
  that N14 absorption is zero in void voxels (lattice orientation) and that
  almost every tissue voxel scores.
- Only the brain carries B10 (skin and skull have none).

## Measured at the time of writing

MC (2e6 histories): N14 absorption skin 0.01024 (0.1% error), skull 0.00372
(0.2%), brain 0.01062 (0.08%); B10 brain 1.363e-4 (0.08%).

S_N/MC (P1 default, `--max-outer 64`; 36 outer iterations):

| Quantity | S_N/MC |
|---|---|
| B10 brain | 0.667 |
| N14 brain / skin / skull | 0.639 / 0.757 / 0.681 |
| band flux (all voxels) thermal / epithermal / fast | 0.774 / 1.082 / 0.943 |

Other configurations on the same grid (not in the suite): forced `--p0` gives
B10 0.857, N14 brain 0.821; `--exp-source` overshoots (B10 1.20, N14 skin 1.51) and
needs all 64 outers; `--no-uncollided-split` gives ~0.05 (used by the negative
test).
