# OpenBNCT project report: openbnct.hn-head-ct.v1

> Research software output. OpenBNCT is not a medical device and has not been clinically validated or commissioned for any treatment facility; these results are not for diagnosis, treatment planning, or any clinical decision. See docs/DISCLAIMER.md.

> Accuracy status: on the synthetic layered-head benchmark, with the default settings (histogram beam bins uniform per eV, transported photons), deterministic (S_N) structure-mean boron, hydrogen (fast-neutron) and photon doses are within about 15% of continuous-energy OpenMC with matching S(alpha,beta) (whole phantom 0.86 / 0.98 / 0.93, target 1.06 / 0.90 / 1.03 at 5e6 histories). Projects that set photon_transport = false deposit capture-gamma energy where it is born, which over-predicts the photon dose ~3-4x. One synthetic geometry is not a validation of your study: treat absolute and biologically weighted totals as research estimates and run `openbnct project verify` for an independent Monte Carlo check. An independent continuous-energy OpenMC check was run on this study (verdict: INCONCLUSIVE; see "Independent Monte Carlo check").

| | |
|---|---|
| Project id | openbnct.hn-head-ct.v1 |
| OpenBNCT version | 0.2.2 |
| Generated (UTC) | 2026-09-30T20:09:29Z |
| Transport status | converged |
| Transport | S4 discrete ordinates, 18 outer iterations, residual 5.561e-7 |
| Beam spectrum bins | spread uniformly per eV (OpenMC/MCNP convention) within each histogram bin |
| Photon treatment | transported: capture photons solved with `sn photon-solve` (16-group coupled photon data); the neutron solve's local capture-gamma kerma is replaced by the transported photon dose |
| Target / approach | RESEARCH_TARGET / +x |
| Boron | blood 25 ug/g; tissue:blood ratios RESEARCH_TARGET=3.5, SKIN=1.5; default ratio 1 |
| Dose unit | Gy per source particle (no delivery normalization given) |

## Input hashes (sha256)

- `../data/hu.nii.gz`: `29d191c9ad04fd61468a4fa889c7b9c70f256eff2de44d946ee7b6435252924e`
- `../data/label-names.json`: `752f276db7d1575004081574c52efbaebc3f7d316f234330270b49f8bf72b1a0`
- `../data/labels.nii.gz`: `a29a6a99a52918b60cb9ffb98a38321b75a57972fa9b2faa74e4d5ae9958ae06`
- `inputs/beam-fir1-k63-ineel.json`: `20be38efa8275e06c2e20bb0c75ba587f7d2b74bb760702ea97d6858e3f277e3`
- `inputs/hu-calibration-generic-head-ct.json`: `00949f2280409e7387670bfe474265d1481d160075284e31ba3e302c69505a1d`
- `inputs/material-air-dry.json`: `d32ecf9ab0ce18161843be4c57393c1e9f2bb208514a8aaffa3738acb49567d2`
- `inputs/multigroup-data-28g-tsl.json`: `c664332337cdacfe28f4141e916fcc400c61155b027adccea54053c8b709e490`
- `inputs/multigroup-photon-data-16g.json`: `df59fb829c43b2b1e2306716c3ee0be6440a16107a6930dec7e4cc318e8ff23d`
- `inputs/openmc-component-profile-unit-mass-fraction.json`: `7659e2f50a0999167ffb1017e1285ee20ce769f61d67fea00b68f6a4b4fcde50`
- `inputs/openmc-endfb81-base-manifest.json`: `3eaae09921172199c34f3fb236ae082ea5ace4567e0e04d2afcce357add73fb1`
- `inputs/openmc-execution-profile-smoke.json`: `73c644e483e9b9008a88be93d0f47ede174e5180f4c137c208fd7cc62be23e07`
- `inputs/openmc-response-set-nf-bnct-001.json`: `bfc48efe75f470cd8f1f78c35e4589cba8853655cc9cf9f709f5cece4a8a9afd`
- `inputs/openmc-unit-source-component-profile.json`: `a35b26c0134ae02d3b1b0ede5b8c6f38e86966e86c65e1727b4c7f38677ab41a`
- `inputs/openmc-unit-source-material.json`: `096e236d234acabc18f3027ae53be3c94f5608a86a1dec866cefc8bb330db813`

## Dose by structure (Gy per source particle)

D95, D50, D2 are the doses received by at least 95 %, 50 % and 2 % of the structure's volume. Physical total includes the boron dose scaled by the concentrations above.

### Brain (19291 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.2669e-14 | 1.1035e-13 | 1.1016e-15 | 6.3407e-15 | 9.9836e-14 |
| component:nitrogen | 4.3196e-16 | 5.2367e-15 | 2.9659e-17 | 1.7210e-16 | 3.2776e-15 |
| component:hydrogen | 3.4074e-16 | 1.1278e-15 | 9.8071e-17 | 2.6421e-16 | 9.4848e-16 |
| component:photon | 6.7547e-15 | 1.5293e-14 | 2.4737e-15 | 5.8732e-15 | 1.4348e-14 |
| physical_total | 2.0197e-14 | 1.2982e-13 | 3.7793e-15 | 1.2760e-14 | 1.1584e-13 |

DVH (physical total): `out/dvh/Brain.csv`

### Brainstem (357 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.6019e-15 | 1.4316e-14 | 3.3698e-15 | 6.3239e-15 | 1.1472e-14 |
| component:nitrogen | 1.0806e-16 | 7.0132e-16 | 0.0000e0 | 9.3669e-17 | 3.0302e-16 |
| component:hydrogen | 2.2915e-16 | 3.6778e-16 | 9.6229e-17 | 2.3544e-16 | 3.3285e-16 |
| component:photon | 6.5962e-15 | 1.0565e-14 | 4.4386e-15 | 6.5274e-15 | 9.2628e-15 |
| physical_total | 1.3535e-14 | 2.5269e-14 | 8.0860e-15 | 1.3227e-14 | 2.1159e-14 |

DVH (physical total): `out/dvh/Brainstem.csv`

### Lacrimal-Lt (7 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.8422e-16 | 7.0166e-16 | 4.7983e-16 | 5.7312e-16 | 6.9780e-16 |
| component:nitrogen | 5.3506e-17 | 1.0573e-16 | 1.9611e-18 | 7.7803e-17 | 1.0384e-16 |
| component:hydrogen | 5.0646e-17 | 6.3428e-17 | 4.0023e-17 | 5.2079e-17 | 6.3291e-17 |
| component:photon | 1.7959e-15 | 1.9307e-15 | 1.6764e-15 | 1.8064e-15 | 1.9273e-15 |
| physical_total | 2.4843e-15 | 2.6978e-15 | 2.2757e-15 | 2.4467e-15 | 2.6904e-15 |

DVH (physical total): `out/dvh/Lacrimal-Lt.csv`

### Lacrimal-Rt (6 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 8.5561e-15 | 9.5255e-15 | 7.0370e-15 | 9.0612e-15 | 9.5175e-15 |
| component:nitrogen | 5.1381e-16 | 1.4805e-15 | 5.5536e-18 | 2.1106e-16 | 1.4482e-15 |
| component:hydrogen | 2.0632e-16 | 2.6041e-16 | 1.1811e-16 | 2.3566e-16 | 2.6030e-16 |
| component:photon | 5.7304e-15 | 6.1333e-15 | 5.1245e-15 | 5.9082e-15 | 6.1288e-15 |
| physical_total | 1.5007e-14 | 1.6391e-14 | 1.3041e-14 | 1.5720e-14 | 1.6347e-14 |

DVH (physical total): `out/dvh/Lacrimal-Rt.csv`

### Lens-Lt (1 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 |
| component:nitrogen | 3.7819e-17 | 3.7819e-17 | 3.7819e-17 | 3.7819e-17 | 3.7819e-17 |
| component:hydrogen | 3.0341e-17 | 3.0341e-17 | 3.0341e-17 | 3.0341e-17 | 3.0341e-17 |
| component:photon | 1.7684e-15 | 1.7684e-15 | 1.7684e-15 | 1.7684e-15 | 1.7684e-15 |
| physical_total | 2.4826e-15 | 2.4826e-15 | 2.4826e-15 | 2.4826e-15 | 2.4826e-15 |

DVH (physical total): `out/dvh/Lens-Lt.csv`

### Lens-Rt (2 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 3.7289e-15 | 4.3037e-15 | 3.2115e-15 | 3.7289e-15 | 4.2807e-15 |
| component:nitrogen | 1.6802e-16 | 2.0706e-16 | 1.3289e-16 | 1.6802e-16 | 2.0549e-16 |
| component:hydrogen | 5.5091e-17 | 6.0742e-17 | 5.0005e-17 | 5.5091e-17 | 6.0516e-17 |
| component:photon | 3.9551e-15 | 4.2289e-15 | 3.7088e-15 | 3.9551e-15 | 4.2179e-15 |
| physical_total | 7.9071e-15 | 8.8004e-15 | 7.1032e-15 | 7.9071e-15 | 8.7647e-15 |

DVH (physical total): `out/dvh/Lens-Rt.csv`

### Mandible (1222 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.7312e-15 | 2.2551e-14 | 1.0413e-16 | 4.2657e-16 | 1.7777e-14 |
| component:nitrogen | 2.7874e-16 | 3.7203e-15 | 1.6429e-17 | 6.8518e-17 | 2.9053e-15 |
| component:hydrogen | 3.0613e-17 | 7.5672e-16 | 2.8847e-18 | 7.7117e-18 | 4.8781e-16 |
| component:photon | 1.9305e-15 | 1.0896e-14 | 7.7115e-16 | 1.2945e-15 | 9.3130e-15 |
| physical_total | 3.9711e-15 | 3.7770e-14 | 9.0429e-16 | 1.8028e-15 | 3.0762e-14 |

DVH (physical total): `out/dvh/Mandible.csv`

### Optic-Nerve-Lt (5 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 2.3500e-15 | 2.6168e-15 | 1.9680e-15 | 2.4149e-15 | 2.6140e-15 |
| component:nitrogen | 1.8430e-17 | 2.6446e-17 | 6.4694e-18 | 2.1963e-17 | 2.6137e-17 |
| component:hydrogen | 1.4351e-16 | 1.7052e-16 | 1.2302e-16 | 1.4373e-16 | 1.6901e-16 |
| component:photon | 3.2965e-15 | 3.4875e-15 | 3.0190e-15 | 3.3375e-15 | 3.4865e-15 |
| physical_total | 5.8085e-15 | 6.2891e-15 | 5.1321e-15 | 5.9135e-15 | 6.2840e-15 |

DVH (physical total): `out/dvh/Optic-Nerve-Lt.csv`

### Optic-Nerve-Rt (5 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 9.6101e-15 | 1.0271e-14 | 9.0452e-15 | 9.6801e-15 | 1.0232e-14 |
| component:nitrogen | 2.7012e-16 | 1.0137e-15 | 3.4530e-17 | 9.7829e-17 | 9.4506e-16 |
| component:hydrogen | 3.5662e-16 | 3.9378e-16 | 3.4091e-16 | 3.5025e-16 | 3.9054e-16 |
| component:photon | 7.1857e-15 | 7.5669e-15 | 6.9126e-15 | 7.0477e-15 | 7.5586e-15 |
| physical_total | 1.7422e-14 | 1.8594e-14 | 1.6497e-14 | 1.7028e-14 | 1.8568e-14 |

DVH (physical total): `out/dvh/Optic-Nerve-Rt.csv`

### Orbit-Lt (116 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.0121e-15 | 1.7901e-15 | 5.7236e-16 | 9.9977e-16 | 1.6154e-15 |
| component:nitrogen | 7.6296e-18 | 4.6554e-17 | 0.0000e0 | 2.8352e-18 | 3.6583e-17 |
| component:hydrogen | 5.1494e-17 | 8.8236e-17 | 3.0318e-17 | 4.9737e-17 | 8.3409e-17 |
| component:photon | 2.1456e-15 | 2.8311e-15 | 1.6890e-15 | 2.1265e-15 | 2.6812e-15 |
| physical_total | 3.2169e-15 | 4.7095e-15 | 2.3079e-15 | 3.2064e-15 | 4.3431e-15 |

DVH (physical total): `out/dvh/Orbit-Lt.csv`

### Orbit-Rt (116 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.5749e-15 | 9.4573e-15 | 3.1303e-15 | 5.4715e-15 | 9.1809e-15 |
| component:nitrogen | 5.9561e-17 | 3.4675e-16 | 0.0000e0 | 3.5284e-17 | 2.1096e-16 |
| component:hydrogen | 1.0430e-16 | 2.9939e-16 | 4.8522e-17 | 8.0291e-17 | 2.3874e-16 |
| component:photon | 4.7414e-15 | 6.5520e-15 | 3.6569e-15 | 4.6685e-15 | 6.0983e-15 |
| physical_total | 1.0480e-14 | 1.6336e-14 | 6.9805e-15 | 1.0261e-14 | 1.5531e-14 |

DVH (physical total): `out/dvh/Orbit-Rt.csv`

### Parotid-Lt (520 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 2.7279e-16 | 9.0136e-16 | 1.1663e-16 | 2.3156e-16 | 7.4767e-16 |
| component:nitrogen | 8.1257e-18 | 1.4775e-16 | 2.6035e-19 | 1.7477e-18 | 9.1319e-17 |
| component:hydrogen | 2.4627e-17 | 5.4119e-17 | 9.6061e-18 | 2.3449e-17 | 4.7545e-17 |
| component:photon | 1.2071e-15 | 2.1016e-15 | 8.9148e-16 | 1.1678e-15 | 1.9081e-15 |
| physical_total | 1.5126e-15 | 3.1621e-15 | 1.0294e-15 | 1.4217e-15 | 2.7383e-15 |

DVH (physical total): `out/dvh/Parotid-Lt.csv`

### Parotid-Rt (579 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 8.0785e-15 | 2.2413e-14 | 1.8744e-15 | 7.1703e-15 | 1.8809e-14 |
| component:nitrogen | 1.6195e-16 | 3.6463e-15 | 3.0773e-18 | 4.8965e-17 | 1.8461e-15 |
| component:hydrogen | 2.4806e-16 | 1.6518e-15 | 1.9439e-17 | 9.1911e-17 | 1.2756e-15 |
| component:photon | 5.1698e-15 | 1.0169e-14 | 2.3479e-15 | 5.0545e-15 | 8.9175e-15 |
| physical_total | 1.3658e-14 | 3.6200e-14 | 4.2957e-15 | 1.2309e-14 | 2.9540e-14 |

DVH (physical total): `out/dvh/Parotid-Rt.csv`

### Spinal-Canal (277 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.4990e-15 | 4.1452e-15 | 2.2565e-16 | 1.3769e-15 | 3.5800e-15 |
| component:nitrogen | 9.3397e-17 | 6.9710e-16 | 3.4257e-18 | 4.8279e-17 | 5.1062e-16 |
| component:hydrogen | 2.5935e-17 | 6.9466e-17 | 8.0844e-18 | 2.1430e-17 | 6.4667e-17 |
| component:photon | 2.6807e-15 | 4.7100e-15 | 1.4542e-15 | 2.6194e-15 | 4.4445e-15 |
| physical_total | 4.2990e-15 | 9.6162e-15 | 1.6858e-15 | 4.0878e-15 | 8.2593e-15 |

DVH (physical total): `out/dvh/Spinal-Canal.csv`

### Spinal-Cord (73 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.3928e-15 | 3.2493e-15 | 1.8026e-16 | 1.2929e-15 | 2.9257e-15 |
| component:nitrogen | 3.0453e-17 | 1.2514e-16 | 4.0901e-18 | 2.4680e-17 | 1.1448e-16 |
| component:hydrogen | 2.4768e-17 | 6.3007e-17 | 8.5041e-18 | 2.0407e-17 | 5.9590e-17 |
| component:photon | 2.6200e-15 | 4.2362e-15 | 1.4615e-15 | 2.5604e-15 | 4.0108e-15 |
| physical_total | 4.0680e-15 | 7.5509e-15 | 1.6578e-15 | 3.9048e-15 | 7.0922e-15 |

DVH (physical total): `out/dvh/Spinal-Cord.csv`

### Submandibular-Lt (115 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.8069e-16 | 4.3327e-16 | 6.4563e-17 | 1.7031e-16 | 3.7246e-16 |
| component:nitrogen | 3.9937e-18 | 1.9723e-17 | 1.5541e-19 | 2.4701e-18 | 1.5896e-17 |
| component:hydrogen | 6.5274e-18 | 1.2579e-17 | 4.2351e-18 | 5.9725e-18 | 1.0980e-17 |
| component:photon | 1.0136e-15 | 1.3729e-15 | 8.3351e-16 | 1.0069e-15 | 1.2945e-15 |
| physical_total | 1.2048e-15 | 1.8261e-15 | 9.3837e-16 | 1.1797e-15 | 1.6924e-15 |

DVH (physical total): `out/dvh/Submandibular-Lt.csv`

### Submandibular-Rt (95 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.4985e-16 | 1.5896e-15 | 1.4550e-16 | 4.7165e-16 | 1.3406e-15 |
| component:nitrogen | 1.7080e-17 | 8.7446e-17 | 1.2321e-18 | 1.0998e-17 | 7.4029e-17 |
| component:hydrogen | 9.0138e-18 | 1.6596e-17 | 5.9957e-18 | 8.4260e-18 | 1.4677e-17 |
| component:photon | 1.3169e-15 | 2.2376e-15 | 9.7190e-16 | 1.2476e-15 | 2.0054e-15 |
| physical_total | 1.8928e-15 | 3.9165e-15 | 1.1522e-15 | 1.7222e-15 | 3.4206e-15 |

DVH (physical total): `out/dvh/Submandibular-Rt.csv`

### BODY (66852 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.5500e-15 | 1.1035e-13 | 1.6158e-16 | 2.2319e-15 | 2.8617e-14 |
| component:nitrogen | 4.5307e-16 | 3.0656e-14 | 1.6363e-18 | 1.0276e-16 | 3.8267e-15 |
| component:hydrogen | 2.2432e-16 | 2.0797e-15 | 6.2839e-18 | 8.0005e-17 | 1.5765e-15 |
| component:photon | 4.2562e-15 | 1.5293e-14 | 9.8141e-16 | 3.0852e-15 | 1.3070e-14 |
| physical_total | 1.1484e-14 | 1.2982e-13 | 1.1784e-15 | 5.6441e-15 | 4.5240e-14 |

DVH (physical total): `out/dvh/BODY.csv`

### SKIN (10415 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 4.7625e-15 | 3.9380e-14 | 8.3186e-17 | 9.9465e-16 | 2.8190e-14 |
| component:nitrogen | 2.3441e-16 | 3.0656e-14 | 8.9065e-19 | 1.8517e-17 | 1.4913e-15 |
| component:hydrogen | 3.1581e-16 | 2.0797e-15 | 4.5710e-18 | 5.1640e-17 | 1.9226e-15 |
| component:photon | 2.8371e-15 | 1.1823e-14 | 7.3716e-16 | 1.8403e-15 | 9.4383e-15 |
| physical_total | 8.1498e-15 | 5.9626e-14 | 8.3560e-16 | 2.9080e-15 | 3.9762e-14 |

DVH (physical total): `out/dvh/SKIN.csv`

### RESEARCH_TARGET (959 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 9.2858e-14 | 1.1035e-13 | 6.8632e-14 | 9.6394e-14 | 1.0964e-13 |
| component:nitrogen | 9.3670e-16 | 5.2367e-15 | 1.8294e-16 | 6.0389e-16 | 5.0441e-15 |
| component:hydrogen | 7.0678e-16 | 1.0948e-15 | 4.5690e-16 | 6.8638e-16 | 1.0349e-15 |
| component:photon | 1.3932e-14 | 1.5293e-14 | 1.2319e-14 | 1.4075e-14 | 1.5173e-14 |
| physical_total | 1.0843e-13 | 1.2982e-13 | 8.1700e-14 | 1.1192e-13 | 1.2789e-13 |

DVH (physical total): `out/dvh/RESEARCH_TARGET.csv`

## Independent Monte Carlo check

**Verdict: INCONCLUSIVE: no structure with resolved MC statistics disagrees, but Lacrimal-Rt, Lens-Lt, Lens-Rt, Optic-Nerve-Lt, Optic-Nerve-Rt, Orbit-Lt, Spinal-Canal, Spinal-Cord, Submandibular-Rt, BODY, SKIN could not be tested at +-5.0% (2 sigma of the MC structure mean exceeds the tolerance); rerun with more particles**

Definition: a structure agrees when its S_N/MC mean total-dose ratio is within +-5.0% of 1 and, where at least one of its voxels is gamma-evaluated, the total-dose gamma (3% dose difference, 3 mm distance to agreement, global, reference voxels below 10% of the MC maximum excluded) pass rate is at least 95.0%. A failing structure whose MC mean is too uncertain to resolve the tolerance (2 sigma above it) is unresolved, not a disagreement. Overall: AGREES when every evaluated structure agrees; DISAGREES when any resolved structure fails; otherwise INCONCLUSIVE. Thresholds live in `[verify]` of `project.toml`.

| | |
|---|---|
| Monte Carlo | OpenMC (commit 617d35a5063c), continuous energy, ENDF/B-VIII.1; 6000000 histories (6000000 requested) in 10 batches, 3 threads, 1270 s wall |
| Thermal scattering | S_N: bound-atom kernel for H1; MC: H1=c_H_in_H2O; consistent |
| MC statistical uncertainty | total dose, voxels above 10% of the maximum (172 voxels): median relative 1-sigma 99.8%, 95th percentile 100.0% |
| Artifacts | `out/08-verify/` (MC dose, comparison, gamma, `ratio-<component>.nii`) |

S_N/MC ratio of structure-mean dose per component (S_N is the boron-scaled deterministic dose; the MC column is the conservative, fully correlated 2-sigma relative uncertainty of the MC structure-mean total):

| structure | voxels | boron | nitrogen | hydrogen | photon | total | MC 2 sigma (total) | gamma pass | status |
|---|---|---|---|---|---|---|---|---|---|
| Brain | 19291 | 0.951 | 0.955 | 0.933 | 1.050 | 0.982 | 20.5% | n/a | agrees |
| Brainstem | 357 | 0.981 | 0.972 | 0.961 | 1.099 | 1.035 | 32.4% | n/a | agrees |
| Lacrimal-Lt | 7 | 1.146 | 1.157 | 1.245 | 0.972 | 0.985 | 81.2% | n/a | agrees |
| Lacrimal-Rt | 6 | 0.953 | 0.960 | 1.081 | 0.902 | 0.920 | 23.7% | n/a | unresolved |
| Lens-Lt | 1 | 1.047 | 1.046 | 1.229 | 0.836 | 0.895 | 62.5% | n/a | unresolved |
| Lens-Rt | 2 | 1.019 | 1.018 | 1.186 | 1.352 | 1.169 | 41.4% | n/a | unresolved |
| Mandible | 1222 | 0.996 | 1.008 | 1.049 | 1.026 | 1.011 | 42.6% | n/a | agrees |
| Optic-Nerve-Lt | 5 | 1.022 | 1.079 | 1.014 | 0.789 | 0.868 | 42.8% | n/a | unresolved |
| Optic-Nerve-Rt | 5 | 0.957 | 0.981 | 0.904 | 0.973 | 0.946 | 25.2% | n/a | unresolved |
| Orbit-Lt | 116 | 1.038 | 1.041 | 1.005 | 1.140 | 1.096 | 71.7% | n/a | unresolved |
| Orbit-Rt | 116 | 0.973 | 0.978 | 1.049 | 1.045 | 1.006 | 34.0% | n/a | agrees |
| Parotid-Lt | 520 | 1.150 | 1.132 | 1.233 | 0.993 | 1.020 | 105.4% | n/a | agrees |
| Parotid-Rt | 579 | 0.984 | 0.979 | 1.065 | 1.034 | 1.003 | 27.9% | n/a | agrees |
| Spinal-Canal | 277 | 1.059 | 1.059 | 1.230 | 1.186 | 1.135 | 59.3% | n/a | unresolved |
| Spinal-Cord | 73 | 1.077 | 1.075 | 1.247 | 1.105 | 1.097 | 63.8% | n/a | unresolved |
| Submandibular-Lt | 115 | 1.077 | 1.055 | 0.837 | 1.023 | 1.029 | 116.4% | n/a | agrees |
| Submandibular-Rt | 95 | 1.113 | 1.103 | 0.857 | 0.865 | 0.930 | 90.8% | n/a | unresolved |
| BODY | 66852 | 0.976 | 1.136 | 0.988 | 1.053 | 1.009 | 27.4% | 0.0% (1 vox) | unresolved |
| SKIN | 10415 | 1.064 | 1.558 | 1.057 | 1.041 | 1.064 | 36.5% | n/a | unresolved |
| RESEARCH_TARGET | 959 | 0.941 | 0.943 | 0.932 | 1.032 | 0.952 | 6.9% | n/a | agrees |

- Lacrimal-Rt: total-dose ratio 0.920 is outside 1 +- 5.0%
- Lens-Lt: total-dose ratio 0.895 is outside 1 +- 5.0%
- Lens-Rt: total-dose ratio 1.169 is outside 1 +- 5.0%
- Optic-Nerve-Lt: total-dose ratio 0.868 is outside 1 +- 5.0%
- Optic-Nerve-Rt: total-dose ratio 0.946 is outside 1 +- 5.0%
- Orbit-Lt: total-dose ratio 1.096 is outside 1 +- 5.0%
- Spinal-Canal: total-dose ratio 1.135 is outside 1 +- 5.0%
- Spinal-Cord: total-dose ratio 1.097 is outside 1 +- 5.0%
- Submandibular-Rt: total-dose ratio 0.930 is outside 1 +- 5.0%
- BODY: gamma pass rate 0.0% is below 95.0%
- SKIN: total-dose ratio 1.064 is outside 1 +- 5.0%

Whole-volume agreement (S_N against MC as reference):

| quantity | gamma pass | evaluated voxels | within 2 combined sigma | max difference / MC maximum |
|---|---|---|---|---|
| component:boron | 92.5% | 11674 | n/a | 0.091 |
| component:nitrogen | 66.8% | 16114 | n/a | 0.861 |
| component:hydrogen | 41.7% | 21189 | n/a | 0.663 |
| component:photon | 0.0% | 168 | n/a | 1.000 |
| physical_total | 0.0% | 172 | n/a | 0.998 |


## Commands

Run from the project directory to reproduce or modify a single step by hand. The report step is internal (it reads `out/06-metrics`); lines starting with `#` in the verify step are internal sub-steps.

01-import:

```text
openbnct import ct-nifti --hu ../data/hu.nii.gz --labels ../data/labels.nii.gz --label-names ../data/label-names.json --spacing-mm 4 --case-id openbnct.hn-head-ct.v1 --base-material inputs/material-air-dry.json --case-output out/01-import/case.json --hu-output out/01-import/hu.nii --masks-dir out/01-import/masks
```

02-calibrate:

```text
openbnct dicom calibrate --calibration inputs/hu-calibration-generic-head-ct.json --hu-nifti out/01-import/hu.nii --case out/01-import/case.json --output out/02-calibrate/assignment.json --report out/02-calibrate/report.json
```

03-beam:

```text
openbnct beam bind --beam inputs/beam-fir1-k63-ineel.json --case out/01-import/case.json --output out/03-beam/case.json --aim-mask out/01-import/masks/022-RESEARCH_TARGET.json --approach=+x
```

04-transport:

```text
openbnct sn solve --case out/03-beam/case.json --data inputs/multigroup-data-28g-tsl.json --assignment out/02-calibrate/assignment.json --order 4 --max-outer 128 --anderson 3 --source-weighting uniform_in_bin --dose out/04-transport/dose-local-photon.json --boron-unit-output out/04-transport/boron-unit.json --output out/04-transport/flux.json
openbnct sn photon-solve --case out/03-beam/case.json --photon-data inputs/multigroup-photon-data-16g.json --neutron-flux out/04-transport/flux.json --assignment out/02-calibrate/assignment.json --order 4 --dose out/04-transport/dose-photon.json --output out/04-transport/photon-flux.json
openbnct sn merge-photon-dose --neutron-dose out/04-transport/dose-local-photon.json --photon-dose out/04-transport/dose-photon.json --output out/04-transport/dose.json
```

05-boron:

```text
openbnct boron dose --physical-bundle out/04-transport/dose.json --unit-dose out/04-transport/boron-unit.json --blood-ug-g 25 --default-ratio 1 --ratio RESEARCH_TARGET=3.5 --mask RESEARCH_TARGET=out/01-import/masks/022-RESEARCH_TARGET.json --ratio SKIN=1.5 --mask SKIN=out/01-import/masks/021-SKIN.json --output out/05-boron/dose.json
```

06-metrics:

```text
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/001-Brain.json --dx 95,50,2 --output out/06-metrics/Brain.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/001-Brain.json --dx 95,50,2 --output out/06-metrics/Brain.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/001-Brain.json --dx 95,50,2 --output out/06-metrics/Brain.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/001-Brain.json --dx 95,50,2 --output out/06-metrics/Brain.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/001-Brain.json --dx 95,50,2 --output out/06-metrics/Brain.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/001-Brain.json --bins 100 --output out/06-metrics/Brain.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/002-Brainstem.json --dx 95,50,2 --output out/06-metrics/Brainstem.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/002-Brainstem.json --dx 95,50,2 --output out/06-metrics/Brainstem.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/002-Brainstem.json --dx 95,50,2 --output out/06-metrics/Brainstem.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/002-Brainstem.json --dx 95,50,2 --output out/06-metrics/Brainstem.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/002-Brainstem.json --dx 95,50,2 --output out/06-metrics/Brainstem.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/002-Brainstem.json --bins 100 --output out/06-metrics/Brainstem.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/005-Lacrimal-Lt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Lt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/005-Lacrimal-Lt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Lt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/005-Lacrimal-Lt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Lt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/005-Lacrimal-Lt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Lt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/005-Lacrimal-Lt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Lt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/005-Lacrimal-Lt.json --bins 100 --output out/06-metrics/Lacrimal-Lt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/006-Lacrimal-Rt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Rt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/006-Lacrimal-Rt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Rt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/006-Lacrimal-Rt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Rt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/006-Lacrimal-Rt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Rt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/006-Lacrimal-Rt.json --dx 95,50,2 --output out/06-metrics/Lacrimal-Rt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/006-Lacrimal-Rt.json --bins 100 --output out/06-metrics/Lacrimal-Rt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/007-Lens-Lt.json --dx 95,50,2 --output out/06-metrics/Lens-Lt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/007-Lens-Lt.json --dx 95,50,2 --output out/06-metrics/Lens-Lt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/007-Lens-Lt.json --dx 95,50,2 --output out/06-metrics/Lens-Lt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/007-Lens-Lt.json --dx 95,50,2 --output out/06-metrics/Lens-Lt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/007-Lens-Lt.json --dx 95,50,2 --output out/06-metrics/Lens-Lt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/007-Lens-Lt.json --bins 100 --output out/06-metrics/Lens-Lt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/008-Lens-Rt.json --dx 95,50,2 --output out/06-metrics/Lens-Rt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/008-Lens-Rt.json --dx 95,50,2 --output out/06-metrics/Lens-Rt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/008-Lens-Rt.json --dx 95,50,2 --output out/06-metrics/Lens-Rt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/008-Lens-Rt.json --dx 95,50,2 --output out/06-metrics/Lens-Rt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/008-Lens-Rt.json --dx 95,50,2 --output out/06-metrics/Lens-Rt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/008-Lens-Rt.json --bins 100 --output out/06-metrics/Lens-Rt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/009-Mandible.json --dx 95,50,2 --output out/06-metrics/Mandible.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/009-Mandible.json --dx 95,50,2 --output out/06-metrics/Mandible.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/009-Mandible.json --dx 95,50,2 --output out/06-metrics/Mandible.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/009-Mandible.json --dx 95,50,2 --output out/06-metrics/Mandible.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/009-Mandible.json --dx 95,50,2 --output out/06-metrics/Mandible.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/009-Mandible.json --bins 100 --output out/06-metrics/Mandible.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/010-Optic-Nerve-Lt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Lt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/010-Optic-Nerve-Lt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Lt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/010-Optic-Nerve-Lt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Lt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/010-Optic-Nerve-Lt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Lt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/010-Optic-Nerve-Lt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Lt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/010-Optic-Nerve-Lt.json --bins 100 --output out/06-metrics/Optic-Nerve-Lt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/011-Optic-Nerve-Rt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Rt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/011-Optic-Nerve-Rt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Rt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/011-Optic-Nerve-Rt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Rt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/011-Optic-Nerve-Rt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Rt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/011-Optic-Nerve-Rt.json --dx 95,50,2 --output out/06-metrics/Optic-Nerve-Rt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/011-Optic-Nerve-Rt.json --bins 100 --output out/06-metrics/Optic-Nerve-Rt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/012-Orbit-Lt.json --dx 95,50,2 --output out/06-metrics/Orbit-Lt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/012-Orbit-Lt.json --dx 95,50,2 --output out/06-metrics/Orbit-Lt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/012-Orbit-Lt.json --dx 95,50,2 --output out/06-metrics/Orbit-Lt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/012-Orbit-Lt.json --dx 95,50,2 --output out/06-metrics/Orbit-Lt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/012-Orbit-Lt.json --dx 95,50,2 --output out/06-metrics/Orbit-Lt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/012-Orbit-Lt.json --bins 100 --output out/06-metrics/Orbit-Lt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/013-Orbit-Rt.json --dx 95,50,2 --output out/06-metrics/Orbit-Rt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/013-Orbit-Rt.json --dx 95,50,2 --output out/06-metrics/Orbit-Rt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/013-Orbit-Rt.json --dx 95,50,2 --output out/06-metrics/Orbit-Rt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/013-Orbit-Rt.json --dx 95,50,2 --output out/06-metrics/Orbit-Rt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/013-Orbit-Rt.json --dx 95,50,2 --output out/06-metrics/Orbit-Rt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/013-Orbit-Rt.json --bins 100 --output out/06-metrics/Orbit-Rt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/014-Parotid-Lt.json --dx 95,50,2 --output out/06-metrics/Parotid-Lt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/014-Parotid-Lt.json --dx 95,50,2 --output out/06-metrics/Parotid-Lt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/014-Parotid-Lt.json --dx 95,50,2 --output out/06-metrics/Parotid-Lt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/014-Parotid-Lt.json --dx 95,50,2 --output out/06-metrics/Parotid-Lt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/014-Parotid-Lt.json --dx 95,50,2 --output out/06-metrics/Parotid-Lt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/014-Parotid-Lt.json --bins 100 --output out/06-metrics/Parotid-Lt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/015-Parotid-Rt.json --dx 95,50,2 --output out/06-metrics/Parotid-Rt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/015-Parotid-Rt.json --dx 95,50,2 --output out/06-metrics/Parotid-Rt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/015-Parotid-Rt.json --dx 95,50,2 --output out/06-metrics/Parotid-Rt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/015-Parotid-Rt.json --dx 95,50,2 --output out/06-metrics/Parotid-Rt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/015-Parotid-Rt.json --dx 95,50,2 --output out/06-metrics/Parotid-Rt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/015-Parotid-Rt.json --bins 100 --output out/06-metrics/Parotid-Rt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/016-Spinal-Canal.json --dx 95,50,2 --output out/06-metrics/Spinal-Canal.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/016-Spinal-Canal.json --dx 95,50,2 --output out/06-metrics/Spinal-Canal.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/016-Spinal-Canal.json --dx 95,50,2 --output out/06-metrics/Spinal-Canal.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/016-Spinal-Canal.json --dx 95,50,2 --output out/06-metrics/Spinal-Canal.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/016-Spinal-Canal.json --dx 95,50,2 --output out/06-metrics/Spinal-Canal.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/016-Spinal-Canal.json --bins 100 --output out/06-metrics/Spinal-Canal.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/017-Spinal-Cord.json --dx 95,50,2 --output out/06-metrics/Spinal-Cord.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/017-Spinal-Cord.json --dx 95,50,2 --output out/06-metrics/Spinal-Cord.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/017-Spinal-Cord.json --dx 95,50,2 --output out/06-metrics/Spinal-Cord.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/017-Spinal-Cord.json --dx 95,50,2 --output out/06-metrics/Spinal-Cord.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/017-Spinal-Cord.json --dx 95,50,2 --output out/06-metrics/Spinal-Cord.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/017-Spinal-Cord.json --bins 100 --output out/06-metrics/Spinal-Cord.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/018-Submandibular-Lt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Lt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/018-Submandibular-Lt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Lt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/018-Submandibular-Lt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Lt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/018-Submandibular-Lt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Lt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/018-Submandibular-Lt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Lt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/018-Submandibular-Lt.json --bins 100 --output out/06-metrics/Submandibular-Lt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/019-Submandibular-Rt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Rt.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/019-Submandibular-Rt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Rt.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/019-Submandibular-Rt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Rt.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/019-Submandibular-Rt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Rt.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/019-Submandibular-Rt.json --dx 95,50,2 --output out/06-metrics/Submandibular-Rt.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/019-Submandibular-Rt.json --bins 100 --output out/06-metrics/Submandibular-Rt.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/020-BODY.json --dx 95,50,2 --output out/06-metrics/BODY.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/020-BODY.json --dx 95,50,2 --output out/06-metrics/BODY.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/020-BODY.json --dx 95,50,2 --output out/06-metrics/BODY.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/020-BODY.json --dx 95,50,2 --output out/06-metrics/BODY.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/020-BODY.json --dx 95,50,2 --output out/06-metrics/BODY.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/020-BODY.json --bins 100 --output out/06-metrics/BODY.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/021-SKIN.json --dx 95,50,2 --output out/06-metrics/SKIN.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/021-SKIN.json --dx 95,50,2 --output out/06-metrics/SKIN.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/021-SKIN.json --dx 95,50,2 --output out/06-metrics/SKIN.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/021-SKIN.json --dx 95,50,2 --output out/06-metrics/SKIN.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/021-SKIN.json --dx 95,50,2 --output out/06-metrics/SKIN.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/021-SKIN.json --bins 100 --output out/06-metrics/SKIN.physical_total.dvh.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:boron --mask out/01-import/masks/022-RESEARCH_TARGET.json --dx 95,50,2 --output out/06-metrics/RESEARCH_TARGET.component-boron.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:nitrogen --mask out/01-import/masks/022-RESEARCH_TARGET.json --dx 95,50,2 --output out/06-metrics/RESEARCH_TARGET.component-nitrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:hydrogen --mask out/01-import/masks/022-RESEARCH_TARGET.json --dx 95,50,2 --output out/06-metrics/RESEARCH_TARGET.component-hydrogen.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity component:photon --mask out/01-import/masks/022-RESEARCH_TARGET.json --dx 95,50,2 --output out/06-metrics/RESEARCH_TARGET.component-photon.metrics.json
openbnct metrics --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/022-RESEARCH_TARGET.json --dx 95,50,2 --output out/06-metrics/RESEARCH_TARGET.physical_total.metrics.json
openbnct dvh --dose out/05-boron/dose.json --quantity physical_total --mask out/01-import/masks/022-RESEARCH_TARGET.json --bins 100 --output out/06-metrics/RESEARCH_TARGET.physical_total.dvh.json
```

08-verify:

```text
# internal: write out/08-verify/source.json (the "source" member of out/03-beam/case.json) and out/08-verify/case.json (that case with this source); the port plane is moved 0.001 cm inside the grid if it sits on the boundary
# internal: write out/08-verify/execution-profile.json = inputs/openmc-execution-profile-smoke.json with the requested histories and batches
openbnct openmc data select-manifest --base-manifest inputs/openmc-endfb81-base-manifest.json --data-root /home/connoravila/nuclear-data/endfb-viii.1-hdf5 --assignment out/02-calibrate/assignment.json --manifest-id openbnct.hn-head-ct.v1.openmc-verify.endfb81 --output out/08-verify/nuclear-data-manifest.json
openbnct openmc run --case out/08-verify/case.json --component-profile inputs/openmc-component-profile-unit-mass-fraction.json --material inputs/material-air-dry.json --source out/08-verify/source.json --response-set inputs/openmc-response-set-nf-bnct-001.json --nuclear-data-manifest out/08-verify/nuclear-data-manifest.json --execution-profile out/08-verify/execution-profile.json --assignment out/02-calibrate/assignment.json --unit-source-component-profile inputs/openmc-unit-source-component-profile.json --unit-source-material inputs/openmc-unit-source-material.json --unit-source-nuclear-data-manifest inputs/openmc-endfb81-base-manifest.json --mixture-levels 20 --thermal-scattering H1=c_H_in_H2O --nuclear-data-root /home/connoravila/nuclear-data/endfb-viii.1-hdf5 --openmc /home/connoravila/micromamba/envs/openmc016/bin/openmc --env OPENMC_CROSS_SECTIONS=/home/connoravila/nuclear-data/endfb-viii.1-hdf5/cross_sections.xml --threads 3 --timeout-seconds 14400 --working-directory out/08-verify/openmc-run --dose-output out/08-verify/mc-physical-dose.json --boron-unit-dose-output out/08-verify/mc-boron-unit.json
openbnct boron dose --physical-bundle out/08-verify/mc-physical-dose.json --unit-dose out/08-verify/mc-boron-unit.json --blood-ug-g 25 --default-ratio 1 --ratio RESEARCH_TARGET=3.5 --mask RESEARCH_TARGET=out/01-import/masks/022-RESEARCH_TARGET.json --ratio SKIN=1.5 --mask SKIN=out/01-import/masks/021-SKIN.json --output out/08-verify/mc-dose.json
openbnct compare --reference out/08-verify/mc-dose.json --candidate out/05-boron/dose.json --output out/08-verify/comparison.json
openbnct gamma --reference out/08-verify/mc-dose.json --candidate out/05-boron/dose.json --dose-difference-percent 3 --distance-to-agreement-mm 3 --dose-threshold-percent 10 --gamma-volume --output out/08-verify/gamma.json
# internal: compare structure means, gamma per structure and ratio volumes -> out/08-verify/verification.json, out/08-verify/ratio-<component>.nii
```

Research software output. OpenBNCT is not a medical device and has not been clinically validated or commissioned for any treatment facility; these results are not for diagnosis, treatment planning, or any clinical decision. See docs/DISCLAIMER.md.
