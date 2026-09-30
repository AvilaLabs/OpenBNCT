# OpenBNCT project report: openbnct.hn-head-ct.v1

> Research software output. OpenBNCT is not a medical device and has not been clinically validated or commissioned for any treatment facility; these results are not for diagnosis, treatment planning, or any clinical decision. See docs/DISCLAIMER.md.

> Accuracy status: on the synthetic layered-head benchmark, with the default settings (histogram beam bins uniform per eV, transported photons), deterministic (S_N) structure-mean boron, hydrogen (fast-neutron) and photon doses are within about 15% of continuous-energy OpenMC with matching S(alpha,beta) (whole phantom 0.86 / 0.98 / 0.93, target 1.06 / 0.90 / 1.03 at 5e6 histories). Projects that set photon_transport = false deposit capture-gamma energy where it is born, which over-predicts the photon dose ~3-4x. One synthetic geometry is not a validation of your study: treat absolute and biologically weighted totals as research estimates and run `openbnct project verify` for an independent Monte Carlo check. An independent continuous-energy OpenMC check was run on this study (verdict: INCONCLUSIVE; see "Independent Monte Carlo check").

| | |
|---|---|
| Project id | openbnct.hn-head-ct.v1 |
| OpenBNCT version | 0.2.2 |
| Generated (UTC) | 2026-09-30T19:18:20Z |
| Transport status | converged |
| Transport | S4 discrete ordinates, 18 outer iterations, residual 5.561e-7 |
| Beam spectrum bins | spread uniformly per eV (OpenMC/MCNP convention) within each histogram bin |
| Photon treatment | local deposition: capture-gamma energy is deposited where it is born (no photon transport; over-predicts photon dose in finite heads) |
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
| component:photon | 3.8796e-14 | 1.4380e-13 | 4.7071e-15 | 2.6022e-14 | 1.2419e-13 |
| physical_total | 5.2238e-14 | 2.5588e-13 | 6.0185e-15 | 3.2886e-14 | 2.2127e-13 |

DVH (physical total): `out/dvh/Brain.csv`

### Brainstem (357 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.6019e-15 | 1.4316e-14 | 3.3698e-15 | 6.3239e-15 | 1.1472e-14 |
| component:nitrogen | 1.0806e-16 | 7.0132e-16 | 0.0000e0 | 9.3669e-17 | 3.0302e-16 |
| component:hydrogen | 2.2915e-16 | 3.6778e-16 | 9.6229e-17 | 2.3544e-16 | 3.3285e-16 |
| component:photon | 2.6043e-14 | 5.2914e-14 | 1.3774e-14 | 2.5087e-14 | 4.4975e-14 |
| physical_total | 3.2982e-14 | 6.7619e-14 | 1.7419e-14 | 3.1760e-14 | 5.6452e-14 |

DVH (physical total): `out/dvh/Brainstem.csv`

### Lacrimal-Lt (7 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.8422e-16 | 7.0166e-16 | 4.7983e-16 | 5.7312e-16 | 6.9780e-16 |
| component:nitrogen | 5.3506e-17 | 1.0573e-16 | 1.9611e-18 | 7.7803e-17 | 1.0384e-16 |
| component:hydrogen | 5.0646e-17 | 6.3428e-17 | 4.0023e-17 | 5.2079e-17 | 6.3291e-17 |
| component:photon | 2.3429e-15 | 2.6899e-15 | 1.9416e-15 | 2.3911e-15 | 2.6815e-15 |
| physical_total | 3.0312e-15 | 3.4824e-15 | 2.5409e-15 | 3.0315e-15 | 3.4710e-15 |

DVH (physical total): `out/dvh/Lacrimal-Lt.csv`

### Lacrimal-Rt (6 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 8.5561e-15 | 9.5255e-15 | 7.0370e-15 | 9.0612e-15 | 9.5175e-15 |
| component:nitrogen | 5.1381e-16 | 1.4805e-15 | 5.5536e-18 | 2.1106e-16 | 1.4482e-15 |
| component:hydrogen | 2.0632e-16 | 2.6041e-16 | 1.1811e-16 | 2.3566e-16 | 2.6030e-16 |
| component:photon | 3.4267e-14 | 3.7659e-14 | 3.0038e-14 | 3.4533e-14 | 3.7657e-14 |
| physical_total | 4.3543e-14 | 4.8244e-14 | 3.8127e-14 | 4.4181e-14 | 4.8170e-14 |

DVH (physical total): `out/dvh/Lacrimal-Rt.csv`

### Lens-Lt (1 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 |
| component:nitrogen | 3.7819e-17 | 3.7819e-17 | 3.7819e-17 | 3.7819e-17 | 3.7819e-17 |
| component:hydrogen | 3.0341e-17 | 3.0341e-17 | 3.0341e-17 | 3.0341e-17 | 3.0341e-17 |
| component:photon | 2.8553e-15 | 2.8553e-15 | 2.8553e-15 | 2.8553e-15 | 2.8553e-15 |
| physical_total | 3.5694e-15 | 3.5694e-15 | 3.5694e-15 | 3.5694e-15 | 3.5694e-15 |

DVH (physical total): `out/dvh/Lens-Lt.csv`

### Lens-Rt (2 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 3.7289e-15 | 4.3037e-15 | 3.2115e-15 | 3.7289e-15 | 4.2807e-15 |
| component:nitrogen | 1.6802e-16 | 2.0706e-16 | 1.3289e-16 | 1.6802e-16 | 2.0549e-16 |
| component:hydrogen | 5.5091e-17 | 6.0742e-17 | 5.0005e-17 | 5.5091e-17 | 6.0516e-17 |
| component:photon | 1.6902e-14 | 1.9763e-14 | 1.4328e-14 | 1.6902e-14 | 1.9648e-14 |
| physical_total | 2.0854e-14 | 2.4334e-14 | 1.7722e-14 | 2.0854e-14 | 2.4195e-14 |

DVH (physical total): `out/dvh/Lens-Rt.csv`

### Mandible (1222 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.7312e-15 | 2.2551e-14 | 1.0413e-16 | 4.2657e-16 | 1.7777e-14 |
| component:nitrogen | 2.7874e-16 | 3.7203e-15 | 1.6429e-17 | 6.8518e-17 | 2.9053e-15 |
| component:hydrogen | 3.0613e-17 | 7.5672e-16 | 2.8847e-18 | 7.7117e-18 | 4.8781e-16 |
| component:photon | 6.3631e-15 | 9.1817e-14 | 3.4922e-16 | 1.5048e-15 | 6.5414e-14 |
| physical_total | 8.4036e-15 | 1.1880e-13 | 4.6977e-16 | 2.0052e-15 | 8.7657e-14 |

DVH (physical total): `out/dvh/Mandible.csv`

### Optic-Nerve-Lt (5 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 2.3500e-15 | 2.6168e-15 | 1.9680e-15 | 2.4149e-15 | 2.6140e-15 |
| component:nitrogen | 1.8430e-17 | 2.6446e-17 | 6.4694e-18 | 2.1963e-17 | 2.6137e-17 |
| component:hydrogen | 1.4351e-16 | 1.7052e-16 | 1.2302e-16 | 1.4373e-16 | 1.6901e-16 |
| component:photon | 9.0770e-15 | 1.0262e-14 | 7.7747e-15 | 9.2886e-15 | 1.0204e-14 |
| physical_total | 1.1589e-14 | 1.3075e-14 | 9.8879e-15 | 1.1865e-14 | 1.3012e-14 |

DVH (physical total): `out/dvh/Optic-Nerve-Lt.csv`

### Optic-Nerve-Rt (5 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 9.6101e-15 | 1.0271e-14 | 9.0452e-15 | 9.6801e-15 | 1.0232e-14 |
| component:nitrogen | 2.7012e-16 | 1.0137e-15 | 3.4530e-17 | 9.7829e-17 | 9.4506e-16 |
| component:hydrogen | 3.5662e-16 | 3.9378e-16 | 3.4091e-16 | 3.5025e-16 | 3.9054e-16 |
| component:photon | 3.7120e-14 | 3.8349e-14 | 3.5864e-14 | 3.6851e-14 | 3.8318e-14 |
| physical_total | 4.7357e-14 | 4.9044e-14 | 4.5575e-14 | 4.7944e-14 | 4.8967e-14 |

DVH (physical total): `out/dvh/Optic-Nerve-Rt.csv`

### Orbit-Lt (116 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.0121e-15 | 1.7901e-15 | 5.7236e-16 | 9.9977e-16 | 1.6154e-15 |
| component:nitrogen | 7.6296e-18 | 4.6554e-17 | 0.0000e0 | 2.8352e-18 | 3.6583e-17 |
| component:hydrogen | 5.1494e-17 | 8.8236e-17 | 3.0318e-17 | 4.9737e-17 | 8.3409e-17 |
| component:photon | 3.8431e-15 | 6.5493e-15 | 2.2646e-15 | 3.7603e-15 | 6.3287e-15 |
| physical_total | 4.9144e-15 | 8.4277e-15 | 2.9116e-15 | 4.8287e-15 | 8.0012e-15 |

DVH (physical total): `out/dvh/Orbit-Lt.csv`

### Orbit-Rt (116 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.5749e-15 | 9.4573e-15 | 3.1303e-15 | 5.4715e-15 | 9.1809e-15 |
| component:nitrogen | 5.9561e-17 | 3.4675e-16 | 0.0000e0 | 3.5284e-17 | 2.1096e-16 |
| component:hydrogen | 1.0430e-16 | 2.9939e-16 | 4.8522e-17 | 8.0291e-17 | 2.3874e-16 |
| component:photon | 2.1478e-14 | 3.7786e-14 | 1.3461e-14 | 2.0340e-14 | 3.5013e-14 |
| physical_total | 2.7217e-14 | 4.6914e-14 | 1.6721e-14 | 2.5844e-14 | 4.4404e-14 |

DVH (physical total): `out/dvh/Orbit-Rt.csv`

### Parotid-Lt (520 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 2.7279e-16 | 9.0136e-16 | 1.1663e-16 | 2.3156e-16 | 7.4767e-16 |
| component:nitrogen | 8.1257e-18 | 1.4775e-16 | 2.6035e-19 | 1.7477e-18 | 9.1319e-17 |
| component:hydrogen | 2.4627e-17 | 5.4119e-17 | 9.6061e-18 | 2.3449e-17 | 4.7545e-17 |
| component:photon | 1.0819e-15 | 3.8690e-15 | 4.4285e-16 | 8.9651e-16 | 3.3049e-15 |
| physical_total | 1.3875e-15 | 4.9313e-15 | 5.7214e-16 | 1.1501e-15 | 4.1201e-15 |

DVH (physical total): `out/dvh/Parotid-Lt.csv`

### Parotid-Rt (579 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 8.0785e-15 | 2.2413e-14 | 1.8744e-15 | 7.1703e-15 | 1.8809e-14 |
| component:nitrogen | 1.6195e-16 | 3.6463e-15 | 3.0773e-18 | 4.8965e-17 | 1.8461e-15 |
| component:hydrogen | 2.4806e-16 | 1.6518e-15 | 1.9439e-17 | 9.1911e-17 | 1.2756e-15 |
| component:photon | 3.1517e-14 | 9.5436e-14 | 7.0776e-15 | 2.7705e-14 | 7.5665e-14 |
| physical_total | 4.0006e-14 | 1.2178e-13 | 8.9780e-15 | 3.4917e-14 | 9.5421e-14 |

DVH (physical total): `out/dvh/Parotid-Rt.csv`

### Spinal-Canal (277 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.4990e-15 | 4.1452e-15 | 2.2565e-16 | 1.3769e-15 | 3.5800e-15 |
| component:nitrogen | 9.3397e-17 | 6.9710e-16 | 3.4257e-18 | 4.8279e-17 | 5.1062e-16 |
| component:hydrogen | 2.5935e-17 | 6.9466e-17 | 8.0844e-18 | 2.1430e-17 | 6.4667e-17 |
| component:photon | 6.1902e-15 | 1.8255e-14 | 9.5744e-16 | 5.6789e-15 | 1.4134e-14 |
| physical_total | 7.8085e-15 | 2.3161e-14 | 1.2252e-15 | 7.1829e-15 | 1.7981e-14 |

DVH (physical total): `out/dvh/Spinal-Canal.csv`

### Spinal-Cord (73 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.3928e-15 | 3.2493e-15 | 1.8026e-16 | 1.2929e-15 | 2.9257e-15 |
| component:nitrogen | 3.0453e-17 | 1.2514e-16 | 4.0901e-18 | 2.4680e-17 | 1.1448e-16 |
| component:hydrogen | 2.4768e-17 | 6.3007e-17 | 8.5041e-18 | 2.0407e-17 | 5.9590e-17 |
| component:photon | 5.6835e-15 | 1.3002e-14 | 8.0566e-16 | 5.3342e-15 | 1.2557e-14 |
| physical_total | 7.1315e-15 | 1.6075e-14 | 1.0008e-15 | 6.6782e-15 | 1.5647e-14 |

DVH (physical total): `out/dvh/Spinal-Cord.csv`

### Submandibular-Lt (115 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.8069e-16 | 4.3327e-16 | 6.4563e-17 | 1.7031e-16 | 3.7246e-16 |
| component:nitrogen | 3.9937e-18 | 1.9723e-17 | 1.5541e-19 | 2.4701e-18 | 1.5896e-17 |
| component:hydrogen | 6.5274e-18 | 1.2579e-17 | 4.2351e-18 | 5.9725e-18 | 1.0980e-17 |
| component:photon | 7.3674e-16 | 1.8826e-15 | 2.4092e-16 | 6.8079e-16 | 1.6422e-15 |
| physical_total | 9.2795e-16 | 2.3243e-15 | 3.1036e-16 | 8.6265e-16 | 2.0400e-15 |

DVH (physical total): `out/dvh/Submandibular-Lt.csv`

### Submandibular-Rt (95 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.4985e-16 | 1.5896e-15 | 1.4550e-16 | 4.7165e-16 | 1.3406e-15 |
| component:nitrogen | 1.7080e-17 | 8.7446e-17 | 1.2321e-18 | 1.0998e-17 | 7.4029e-17 |
| component:hydrogen | 9.0138e-18 | 1.6596e-17 | 5.9957e-18 | 8.4260e-18 | 1.4677e-17 |
| component:photon | 2.3197e-15 | 7.2256e-15 | 5.6894e-16 | 1.8848e-15 | 6.0328e-15 |
| physical_total | 2.8957e-15 | 8.9045e-15 | 7.2255e-16 | 2.4042e-15 | 7.4481e-15 |

DVH (physical total): `out/dvh/Submandibular-Rt.csv`

### BODY (66852 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.5500e-15 | 1.1035e-13 | 1.6158e-16 | 2.2319e-15 | 2.8617e-14 |
| component:nitrogen | 4.5307e-16 | 3.0656e-14 | 1.6363e-18 | 1.0276e-16 | 3.8267e-15 |
| component:hydrogen | 2.2432e-16 | 2.0797e-15 | 6.2839e-18 | 8.0005e-17 | 1.5765e-15 |
| component:photon | 2.1361e-14 | 1.4380e-13 | 5.7469e-16 | 8.5625e-15 | 1.0658e-13 |
| physical_total | 2.8588e-14 | 2.5588e-13 | 7.6110e-16 | 1.1148e-14 | 1.4433e-13 |

DVH (physical total): `out/dvh/BODY.csv`

### SKIN (10415 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 4.7625e-15 | 3.9380e-14 | 8.3186e-17 | 9.9465e-16 | 2.8190e-14 |
| component:nitrogen | 2.3441e-16 | 3.0656e-14 | 8.9065e-19 | 1.8517e-17 | 1.4913e-15 |
| component:hydrogen | 3.1581e-16 | 2.0797e-15 | 4.5710e-18 | 5.1640e-17 | 1.9226e-15 |
| component:photon | 1.3111e-14 | 1.0885e-13 | 2.3747e-16 | 2.7679e-15 | 7.5101e-14 |
| physical_total | 1.8423e-14 | 1.5197e-13 | 3.2797e-16 | 3.8276e-15 | 1.0542e-13 |

DVH (physical total): `out/dvh/SKIN.csv`

### RESEARCH_TARGET (959 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 9.2858e-14 | 1.1035e-13 | 6.8632e-14 | 9.6394e-14 | 1.0964e-13 |
| component:nitrogen | 9.3670e-16 | 5.2367e-15 | 1.8294e-16 | 6.0389e-16 | 5.0441e-15 |
| component:hydrogen | 7.0678e-16 | 1.0948e-15 | 4.5690e-16 | 6.8638e-16 | 1.0349e-15 |
| component:photon | 1.0938e-13 | 1.4380e-13 | 7.6642e-14 | 1.1230e-13 | 1.3896e-13 |
| physical_total | 2.0388e-13 | 2.5588e-13 | 1.4625e-13 | 2.0988e-13 | 2.4998e-13 |

DVH (physical total): `out/dvh/RESEARCH_TARGET.csv`

## Independent Monte Carlo check

**Verdict: INCONCLUSIVE: no structure with resolved MC statistics disagrees, but Brain, Brainstem, Lacrimal-Lt, Lacrimal-Rt, Lens-Lt, Lens-Rt, Mandible, Optic-Nerve-Lt, Optic-Nerve-Rt, Orbit-Lt, Orbit-Rt, Parotid-Lt, Parotid-Rt, Spinal-Canal, Spinal-Cord, Submandibular-Lt, Submandibular-Rt, BODY, SKIN, RESEARCH_TARGET could not be tested at +-5.0% (2 sigma of the MC structure mean exceeds the tolerance); rerun with more particles**

Definition: a structure agrees when its S_N/MC mean total-dose ratio is within +-5.0% of 1 and, where at least one of its voxels is gamma-evaluated, the total-dose gamma (3% dose difference, 3 mm distance to agreement, global, reference voxels below 10% of the MC maximum excluded) pass rate is at least 95.0%. A failing structure whose MC mean is too uncertain to resolve the tolerance (2 sigma above it) is unresolved, not a disagreement. Overall: AGREES when every evaluated structure agrees; DISAGREES when any resolved structure fails; otherwise INCONCLUSIVE. Thresholds live in `[verify]` of `project.toml`.

| | |
|---|---|
| Monte Carlo | OpenMC (commit 617d35a5063c), continuous energy, ENDF/B-VIII.1; 6000000 histories (6000000 requested) in 10 batches, 3 threads, 1687 s wall |
| Thermal scattering | S_N: bound-atom kernel for H1; MC: H1=c_H_in_H2O; consistent |
| MC statistical uncertainty | total dose, voxels above 10% of the maximum (172 voxels): median relative 1-sigma 99.8%, 95th percentile 100.0% |
| Artifacts | `out/08-verify/` (MC dose, comparison, gamma, `ratio-<component>.nii`) |

S_N/MC ratio of structure-mean dose per component (S_N is the boron-scaled deterministic dose; the MC column is the conservative, fully correlated 2-sigma relative uncertainty of the MC structure-mean total):

| structure | voxels | boron | nitrogen | hydrogen | photon | total | MC 2 sigma (total) | gamma pass | status |
|---|---|---|---|---|---|---|---|---|---|
| Brain | 19291 | 0.951 | 0.955 | 0.933 | 6.031 | 2.540 | 20.5% | n/a | unresolved |
| Brainstem | 357 | 0.981 | 0.972 | 0.961 | 4.337 | 2.523 | 32.4% | n/a | unresolved |
| Lacrimal-Lt | 7 | 1.146 | 1.157 | 1.245 | 1.268 | 1.202 | 81.2% | n/a | unresolved |
| Lacrimal-Rt | 6 | 0.953 | 0.960 | 1.081 | 5.396 | 2.670 | 23.7% | n/a | unresolved |
| Lens-Lt | 1 | 1.047 | 1.046 | 1.229 | 1.350 | 1.286 | 62.5% | n/a | unresolved |
| Lens-Rt | 2 | 1.019 | 1.018 | 1.186 | 5.777 | 3.082 | 41.4% | n/a | unresolved |
| Mandible | 1222 | 0.996 | 1.008 | 1.049 | 3.380 | 2.140 | 42.6% | n/a | unresolved |
| Optic-Nerve-Lt | 5 | 1.022 | 1.079 | 1.014 | 2.172 | 1.732 | 42.8% | n/a | unresolved |
| Optic-Nerve-Rt | 5 | 0.957 | 0.981 | 0.904 | 5.029 | 2.571 | 25.2% | n/a | unresolved |
| Orbit-Lt | 116 | 1.038 | 1.041 | 1.005 | 2.041 | 1.675 | 71.7% | n/a | unresolved |
| Orbit-Rt | 116 | 0.973 | 0.978 | 1.049 | 4.736 | 2.612 | 34.0% | n/a | unresolved |
| Parotid-Lt | 520 | 1.150 | 1.132 | 1.233 | 0.890 | 0.936 | 105.4% | n/a | unresolved |
| Parotid-Rt | 579 | 0.984 | 0.979 | 1.065 | 6.303 | 2.939 | 27.9% | n/a | unresolved |
| Spinal-Canal | 277 | 1.059 | 1.059 | 1.230 | 2.739 | 2.061 | 59.3% | n/a | unresolved |
| Spinal-Cord | 73 | 1.077 | 1.075 | 1.247 | 2.397 | 1.924 | 63.8% | n/a | unresolved |
| Submandibular-Lt | 115 | 1.077 | 1.055 | 0.837 | 0.743 | 0.793 | 116.4% | n/a | unresolved |
| Submandibular-Rt | 95 | 1.113 | 1.103 | 0.857 | 1.524 | 1.423 | 90.8% | n/a | unresolved |
| BODY | 66852 | 0.976 | 1.136 | 0.988 | 5.283 | 2.512 | 27.4% | 0.0% (1 vox) | unresolved |
| SKIN | 10415 | 1.064 | 1.558 | 1.057 | 4.812 | 2.405 | 36.5% | n/a | unresolved |
| RESEARCH_TARGET | 959 | 0.941 | 0.943 | 0.932 | 8.104 | 1.790 | 6.9% | n/a | unresolved |

- Brain: total-dose ratio 2.540 is outside 1 +- 5.0%
- Brainstem: total-dose ratio 2.523 is outside 1 +- 5.0%
- Lacrimal-Lt: total-dose ratio 1.202 is outside 1 +- 5.0%
- Lacrimal-Rt: total-dose ratio 2.670 is outside 1 +- 5.0%
- Lens-Lt: total-dose ratio 1.286 is outside 1 +- 5.0%
- Lens-Rt: total-dose ratio 3.082 is outside 1 +- 5.0%
- Mandible: total-dose ratio 2.140 is outside 1 +- 5.0%
- Optic-Nerve-Lt: total-dose ratio 1.732 is outside 1 +- 5.0%
- Optic-Nerve-Rt: total-dose ratio 2.571 is outside 1 +- 5.0%
- Orbit-Lt: total-dose ratio 1.675 is outside 1 +- 5.0%
- Orbit-Rt: total-dose ratio 2.612 is outside 1 +- 5.0%
- Parotid-Lt: total-dose ratio 0.936 is outside 1 +- 5.0%
- Parotid-Rt: total-dose ratio 2.939 is outside 1 +- 5.0%
- Spinal-Canal: total-dose ratio 2.061 is outside 1 +- 5.0%
- Spinal-Cord: total-dose ratio 1.924 is outside 1 +- 5.0%
- Submandibular-Lt: total-dose ratio 0.793 is outside 1 +- 5.0%
- Submandibular-Rt: total-dose ratio 1.423 is outside 1 +- 5.0%
- BODY: total-dose ratio 2.512 is outside 1 +- 5.0%; gamma pass rate 0.0% is below 95.0%
- SKIN: total-dose ratio 2.405 is outside 1 +- 5.0%
- RESEARCH_TARGET: total-dose ratio 1.790 is outside 1 +- 5.0%

Whole-volume agreement (S_N against MC as reference):

| quantity | gamma pass | evaluated voxels | within 2 combined sigma | max difference / MC maximum |
|---|---|---|---|---|
| component:boron | 92.5% | 11674 | n/a | 0.091 |
| component:nitrogen | 66.8% | 16114 | n/a | 0.861 |
| component:hydrogen | 41.7% | 21189 | n/a | 0.663 |
| component:photon | 0.0% | 168 | n/a | 0.999 |
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
openbnct sn solve --case out/03-beam/case.json --data inputs/multigroup-data-28g-tsl.json --assignment out/02-calibrate/assignment.json --order 4 --max-outer 128 --anderson 3 --source-weighting uniform_in_bin --dose out/04-transport/dose.json --boron-unit-output out/04-transport/boron-unit.json --output out/04-transport/flux.json
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
