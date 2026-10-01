# OpenBNCT project report: openbnct.hn-head-ct.v1

> Research software output. OpenBNCT is not a medical device and has not been clinically validated or commissioned for any treatment facility; these results are not for diagnosis, treatment planning, or any clinical decision. See docs/DISCLAIMER.md.

> Accuracy status: on the synthetic layered-head benchmark, with the default settings (histogram beam bins uniform per eV, transported photons), deterministic (S_N) structure-mean boron, hydrogen (fast-neutron) and photon doses are within about 15% of continuous-energy OpenMC with matching S(alpha,beta) (whole phantom 0.86 / 0.98 / 0.93, target 1.06 / 0.90 / 1.03 at 5e6 histories). Projects that set photon_transport = false deposit capture-gamma energy where it is born, which over-predicts the photon dose ~3-4x. One synthetic geometry is not a validation of your study: treat absolute and biologically weighted totals as research estimates and run `openbnct project verify` for an independent Monte Carlo check. An independent continuous-energy OpenMC check was run on this study (verdict: INCONCLUSIVE; see "Independent Monte Carlo check").

| | |
|---|---|
| Project id | openbnct.hn-head-ct.v1 |
| OpenBNCT version | 0.2.2 |
| Generated (UTC) | 2026-10-01T01:15:33Z |
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
| component:nitrogen | 4.3349e-16 | 5.2367e-15 | 2.9965e-17 | 1.7399e-16 | 3.2698e-15 |
| component:hydrogen | 3.3970e-16 | 1.1278e-15 | 9.7689e-17 | 2.6372e-16 | 9.4255e-16 |
| component:photon | 6.7536e-15 | 1.5292e-14 | 2.4730e-15 | 5.8729e-15 | 1.4348e-14 |
| physical_total | 2.0196e-14 | 1.2982e-13 | 3.7779e-15 | 1.2761e-14 | 1.1585e-13 |

DVH (physical total): `out/dvh/Brain.csv`

### Brainstem (357 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.6019e-15 | 1.4316e-14 | 3.3698e-15 | 6.3239e-15 | 1.1472e-14 |
| component:nitrogen | 1.0960e-16 | 7.0132e-16 | 0.0000e0 | 9.5327e-17 | 3.0354e-16 |
| component:hydrogen | 2.2914e-16 | 3.6765e-16 | 9.6224e-17 | 2.3543e-16 | 3.3283e-16 |
| component:photon | 6.5962e-15 | 1.0565e-14 | 4.4386e-15 | 6.5273e-15 | 9.2627e-15 |
| physical_total | 1.3537e-14 | 2.5268e-14 | 8.0862e-15 | 1.3229e-14 | 2.1162e-14 |

DVH (physical total): `out/dvh/Brainstem.csv`

### Lacrimal-Lt (7 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.8422e-16 | 7.0166e-16 | 4.7983e-16 | 5.7312e-16 | 6.9780e-16 |
| component:nitrogen | 5.3113e-17 | 1.0515e-16 | 1.8218e-18 | 7.7077e-17 | 1.0326e-16 |
| component:hydrogen | 4.9552e-17 | 6.3385e-17 | 3.7633e-17 | 5.0533e-17 | 6.3248e-17 |
| component:photon | 1.7934e-15 | 1.9306e-15 | 1.6711e-15 | 1.8063e-15 | 1.9273e-15 |
| physical_total | 2.4803e-15 | 2.6976e-15 | 2.2674e-15 | 2.4469e-15 | 2.6902e-15 |

DVH (physical total): `out/dvh/Lacrimal-Lt.csv`

### Lacrimal-Rt (6 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 8.5561e-15 | 9.5255e-15 | 7.0370e-15 | 9.0612e-15 | 9.5175e-15 |
| component:nitrogen | 5.1237e-16 | 1.4723e-15 | 5.7213e-18 | 2.1371e-16 | 1.4402e-15 |
| component:hydrogen | 2.0463e-16 | 2.6040e-16 | 1.1550e-16 | 2.3234e-16 | 2.6029e-16 |
| component:photon | 5.7269e-15 | 6.1333e-15 | 5.1171e-15 | 5.9025e-15 | 6.1288e-15 |
| physical_total | 1.5000e-14 | 1.6364e-14 | 1.3038e-14 | 1.5720e-14 | 1.6324e-14 |

DVH (physical total): `out/dvh/Lacrimal-Rt.csv`

### Lens-Lt (1 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 | 6.4596e-16 |
| component:nitrogen | 3.7872e-17 | 3.7872e-17 | 3.7872e-17 | 3.7872e-17 | 3.7872e-17 |
| component:hydrogen | 3.0337e-17 | 3.0337e-17 | 3.0337e-17 | 3.0337e-17 | 3.0337e-17 |
| component:photon | 1.7684e-15 | 1.7684e-15 | 1.7684e-15 | 1.7684e-15 | 1.7684e-15 |
| physical_total | 2.4826e-15 | 2.4826e-15 | 2.4826e-15 | 2.4826e-15 | 2.4826e-15 |

DVH (physical total): `out/dvh/Lens-Lt.csv`

### Lens-Rt (2 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 3.7289e-15 | 4.3037e-15 | 3.2115e-15 | 3.7289e-15 | 4.2807e-15 |
| component:nitrogen | 1.6832e-16 | 2.0706e-16 | 1.3346e-16 | 1.6832e-16 | 2.0551e-16 |
| component:hydrogen | 5.5090e-17 | 6.0742e-17 | 5.0004e-17 | 5.5090e-17 | 6.0516e-17 |
| component:photon | 3.9551e-15 | 4.2289e-15 | 3.7087e-15 | 3.9551e-15 | 4.2179e-15 |
| physical_total | 7.9074e-15 | 8.8004e-15 | 7.1037e-15 | 7.9074e-15 | 8.7647e-15 |

DVH (physical total): `out/dvh/Lens-Rt.csv`

### Mandible (1222 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.7312e-15 | 2.2551e-14 | 1.0413e-16 | 4.2657e-16 | 1.7777e-14 |
| component:nitrogen | 2.7566e-16 | 3.6856e-15 | 1.6276e-17 | 6.7902e-17 | 2.8751e-15 |
| component:hydrogen | 2.8426e-17 | 7.0298e-16 | 2.7055e-18 | 7.1877e-18 | 4.5719e-16 |
| component:photon | 1.9238e-15 | 1.0853e-14 | 7.6851e-16 | 1.2896e-15 | 9.2761e-15 |
| physical_total | 3.9591e-15 | 3.7662e-14 | 9.0186e-16 | 1.7961e-15 | 3.0670e-14 |

DVH (physical total): `out/dvh/Mandible.csv`

### Optic-Nerve-Lt (5 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 2.3500e-15 | 2.6168e-15 | 1.9680e-15 | 2.4149e-15 | 2.6140e-15 |
| component:nitrogen | 1.7439e-17 | 2.5062e-17 | 6.0376e-18 | 2.0897e-17 | 2.4768e-17 |
| component:hydrogen | 1.4331e-16 | 1.7023e-16 | 1.2280e-16 | 1.4353e-16 | 1.6874e-16 |
| component:photon | 3.2965e-15 | 3.4875e-15 | 3.0189e-15 | 3.3374e-15 | 3.4865e-15 |
| physical_total | 5.8072e-15 | 6.2873e-15 | 5.1308e-15 | 5.9122e-15 | 6.2824e-15 |

DVH (physical total): `out/dvh/Optic-Nerve-Lt.csv`

### Optic-Nerve-Rt (5 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 9.6101e-15 | 1.0271e-14 | 9.0452e-15 | 9.6801e-15 | 1.0232e-14 |
| component:nitrogen | 2.6733e-16 | 1.0155e-15 | 3.2156e-17 | 9.2706e-17 | 9.4632e-16 |
| component:hydrogen | 3.5625e-16 | 3.9352e-16 | 3.4080e-16 | 3.4963e-16 | 3.9027e-16 |
| component:photon | 7.1855e-15 | 7.5668e-15 | 6.9124e-15 | 7.0475e-15 | 7.5585e-15 |
| physical_total | 1.7419e-14 | 1.8596e-14 | 1.6492e-14 | 1.7023e-14 | 1.8569e-14 |

DVH (physical total): `out/dvh/Optic-Nerve-Rt.csv`

### Orbit-Lt (116 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.0121e-15 | 1.7901e-15 | 5.7236e-16 | 9.9977e-16 | 1.6154e-15 |
| component:nitrogen | 7.6442e-18 | 4.6843e-17 | 0.0000e0 | 2.8155e-18 | 3.6762e-17 |
| component:hydrogen | 5.1480e-17 | 8.8236e-17 | 3.0314e-17 | 4.9728e-17 | 8.3389e-17 |
| component:photon | 2.1456e-15 | 2.8311e-15 | 1.6890e-15 | 2.1264e-15 | 2.6812e-15 |
| physical_total | 3.2168e-15 | 4.7095e-15 | 2.3081e-15 | 3.2064e-15 | 4.3434e-15 |

DVH (physical total): `out/dvh/Orbit-Lt.csv`

### Orbit-Rt (116 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.5749e-15 | 9.4573e-15 | 3.1303e-15 | 5.4715e-15 | 9.1809e-15 |
| component:nitrogen | 6.0093e-17 | 3.4782e-16 | 0.0000e0 | 3.5979e-17 | 2.1282e-16 |
| component:hydrogen | 1.0428e-16 | 2.9920e-16 | 4.8520e-17 | 8.0291e-17 | 2.3874e-16 |
| component:photon | 4.7413e-15 | 6.5519e-15 | 3.6568e-15 | 4.6685e-15 | 6.0983e-15 |
| physical_total | 1.0481e-14 | 1.6334e-14 | 6.9811e-15 | 1.0260e-14 | 1.5532e-14 |

DVH (physical total): `out/dvh/Orbit-Rt.csv`

### Parotid-Lt (520 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 2.7279e-16 | 9.0136e-16 | 1.1663e-16 | 2.3156e-16 | 7.4767e-16 |
| component:nitrogen | 8.0398e-18 | 1.4775e-16 | 2.4090e-19 | 1.6613e-18 | 9.1467e-17 |
| component:hydrogen | 2.4598e-17 | 5.4118e-17 | 9.5914e-18 | 2.3415e-17 | 4.7494e-17 |
| component:photon | 1.2071e-15 | 2.1016e-15 | 8.9147e-16 | 1.1678e-15 | 1.9081e-15 |
| physical_total | 1.5125e-15 | 3.1627e-15 | 1.0294e-15 | 1.4215e-15 | 2.7384e-15 |

DVH (physical total): `out/dvh/Parotid-Lt.csv`

### Parotid-Rt (579 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 8.0785e-15 | 2.2413e-14 | 1.8744e-15 | 7.1703e-15 | 1.8809e-14 |
| component:nitrogen | 1.5983e-16 | 3.6463e-15 | 2.8473e-18 | 4.5485e-17 | 1.8451e-15 |
| component:hydrogen | 2.4776e-16 | 1.6508e-15 | 1.9422e-17 | 9.1795e-17 | 1.2735e-15 |
| component:photon | 5.1696e-15 | 1.0169e-14 | 2.3479e-15 | 5.0543e-15 | 8.9174e-15 |
| physical_total | 1.3656e-14 | 3.6200e-14 | 4.2951e-15 | 1.2305e-14 | 2.9543e-14 |

DVH (physical total): `out/dvh/Parotid-Rt.csv`

### Spinal-Canal (277 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.4990e-15 | 4.1452e-15 | 2.2565e-16 | 1.3769e-15 | 3.5800e-15 |
| component:nitrogen | 9.3465e-17 | 6.9710e-16 | 3.1697e-18 | 4.8882e-17 | 5.0789e-16 |
| component:hydrogen | 2.5850e-17 | 6.9466e-17 | 8.0676e-18 | 2.1240e-17 | 6.4638e-17 |
| component:photon | 2.6801e-15 | 4.7100e-15 | 1.4541e-15 | 2.6193e-15 | 4.4444e-15 |
| physical_total | 4.2983e-15 | 9.6162e-15 | 1.6858e-15 | 4.0882e-15 | 8.2609e-15 |

DVH (physical total): `out/dvh/Spinal-Canal.csv`

### Spinal-Cord (73 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.3928e-15 | 3.2493e-15 | 1.8026e-16 | 1.2929e-15 | 2.9257e-15 |
| component:nitrogen | 3.0784e-17 | 1.2552e-16 | 3.7971e-18 | 2.5106e-17 | 1.1479e-16 |
| component:hydrogen | 2.4767e-17 | 6.3005e-17 | 8.5039e-18 | 2.0406e-17 | 5.9578e-17 |
| component:photon | 2.6200e-15 | 4.2362e-15 | 1.4615e-15 | 2.5604e-15 | 4.0108e-15 |
| physical_total | 4.0683e-15 | 7.5505e-15 | 1.6578e-15 | 3.9053e-15 | 7.0930e-15 |

DVH (physical total): `out/dvh/Spinal-Cord.csv`

### Submandibular-Lt (115 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 1.8069e-16 | 4.3327e-16 | 6.4563e-17 | 1.7031e-16 | 3.7246e-16 |
| component:nitrogen | 4.0379e-18 | 1.9723e-17 | 1.5241e-19 | 2.5167e-18 | 1.5914e-17 |
| component:hydrogen | 6.5270e-18 | 1.2579e-17 | 4.2350e-18 | 5.9723e-18 | 1.0979e-17 |
| component:photon | 1.0136e-15 | 1.3729e-15 | 8.3350e-16 | 1.0069e-15 | 1.2945e-15 |
| physical_total | 1.2049e-15 | 1.8262e-15 | 9.3837e-16 | 1.1797e-15 | 1.6925e-15 |

DVH (physical total): `out/dvh/Submandibular-Lt.csv`

### Submandibular-Rt (95 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 5.4985e-16 | 1.5896e-15 | 1.4550e-16 | 4.7165e-16 | 1.3406e-15 |
| component:nitrogen | 1.7211e-17 | 8.7540e-17 | 1.2412e-18 | 1.1135e-17 | 7.4136e-17 |
| component:hydrogen | 9.0133e-18 | 1.6596e-17 | 5.9955e-18 | 8.4257e-18 | 1.4676e-17 |
| component:photon | 1.3169e-15 | 2.2376e-15 | 9.7189e-16 | 1.2476e-15 | 2.0053e-15 |
| physical_total | 1.8930e-15 | 3.9166e-15 | 1.1522e-15 | 1.7223e-15 | 3.4208e-15 |

DVH (physical total): `out/dvh/Submandibular-Rt.csv`

### BODY (66852 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 6.5500e-15 | 1.1035e-13 | 1.6158e-16 | 2.2319e-15 | 2.8617e-14 |
| component:nitrogen | 3.9150e-16 | 1.8211e-14 | 1.5394e-18 | 9.9768e-17 | 3.4463e-15 |
| component:hydrogen | 2.2245e-16 | 2.1369e-15 | 6.3018e-18 | 7.9071e-17 | 1.5816e-15 |
| component:photon | 4.2555e-15 | 1.5292e-14 | 9.8093e-16 | 3.0871e-15 | 1.3058e-14 |
| physical_total | 1.1419e-14 | 1.2982e-13 | 1.1777e-15 | 5.6011e-15 | 4.5172e-14 |

DVH (physical total): `out/dvh/BODY.csv`

### SKIN (10415 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 4.7625e-15 | 3.9380e-14 | 8.3186e-17 | 9.9465e-16 | 2.8190e-14 |
| component:nitrogen | 1.5597e-16 | 1.7631e-14 | 8.2771e-19 | 1.6226e-17 | 1.0152e-15 |
| component:hydrogen | 3.1879e-16 | 2.1369e-15 | 4.5957e-18 | 5.2828e-17 | 1.9572e-15 |
| component:photon | 2.8404e-15 | 1.1811e-14 | 7.3733e-16 | 1.8403e-15 | 9.4416e-15 |
| physical_total | 8.0776e-15 | 5.5946e-14 | 8.3625e-16 | 2.8992e-15 | 3.9592e-14 |

DVH (physical total): `out/dvh/SKIN.csv`

### RESEARCH_TARGET (959 voxels)

| quantity | mean | max | D95 | D50 | D2 |
|---|---|---|---|---|---|
| component:boron | 9.2858e-14 | 1.1035e-13 | 6.8632e-14 | 9.6394e-14 | 1.0964e-13 |
| component:nitrogen | 9.4345e-16 | 5.2367e-15 | 1.8757e-16 | 6.1432e-16 | 5.0289e-15 |
| component:hydrogen | 7.0594e-16 | 1.0948e-15 | 4.5689e-16 | 6.8636e-16 | 1.0285e-15 |
| component:photon | 1.3931e-14 | 1.5292e-14 | 1.2319e-14 | 1.4069e-14 | 1.5173e-14 |
| physical_total | 1.0844e-13 | 1.2982e-13 | 8.1705e-14 | 1.1193e-13 | 1.2789e-13 |

DVH (physical total): `out/dvh/RESEARCH_TARGET.csv`

## Independent Monte Carlo check

**Verdict: INCONCLUSIVE: no structure with resolved MC statistics disagrees, but Lacrimal-Rt, Lens-Lt, Lens-Rt, Optic-Nerve-Lt, Optic-Nerve-Rt, Orbit-Lt, Spinal-Canal, Spinal-Cord, Submandibular-Rt, BODY, SKIN could not be tested at +-5.0% (2 sigma of the MC structure mean exceeds the tolerance); rerun with more particles**

Definition: a structure agrees when its S_N/MC mean total-dose ratio is within +-5.0% of 1 and, where at least one of its voxels is gamma-evaluated, the total-dose gamma (3% dose difference, 3 mm distance to agreement, global, reference voxels below 10% of the MC maximum excluded) pass rate is at least 95.0%. A failing structure whose MC mean is too uncertain to resolve the tolerance (2 sigma above it) is unresolved, not a disagreement. Overall: AGREES when every evaluated structure agrees; DISAGREES when any resolved structure fails; otherwise INCONCLUSIVE. Thresholds live in `[verify]` of `project.toml`.

| | |
|---|---|
| Monte Carlo | OpenMC (commit 617d35a5063c), continuous energy, ENDF/B-VIII.1; 6000000 histories (6000000 requested) in 10 batches, 3 threads, 1268 s wall |
| Thermal scattering | S_N: bound-atom kernel for H1; MC: H1=c_H_in_H2O; consistent |
| MC statistical uncertainty | total dose, voxels above 10% of the maximum (172 voxels): median relative 1-sigma 99.8%, 95th percentile 100.0% |
| Artifacts | `out/08-verify/` (MC dose, comparison, gamma, `ratio-<component>.nii`) |

S_N/MC ratio of structure-mean dose per component (S_N is the boron-scaled deterministic dose; the MC column is the conservative, fully correlated 2-sigma relative uncertainty of the MC structure-mean total):

| structure | voxels | boron | nitrogen | hydrogen | photon | total | MC 2 sigma (total) | gamma pass | status |
|---|---|---|---|---|---|---|---|---|---|
| Brain | 19291 | 0.951 | 0.958 | 0.930 | 1.050 | 0.982 | 20.5% | n/a | agrees |
| Brainstem | 357 | 0.981 | 0.986 | 0.961 | 1.099 | 1.036 | 32.4% | n/a | agrees |
| Lacrimal-Lt | 7 | 1.146 | 1.148 | 1.218 | 0.970 | 0.983 | 81.2% | n/a | agrees |
| Lacrimal-Rt | 6 | 0.953 | 0.957 | 1.072 | 0.902 | 0.920 | 23.7% | n/a | unresolved |
| Lens-Lt | 1 | 1.047 | 1.048 | 1.228 | 0.836 | 0.895 | 62.5% | n/a | unresolved |
| Lens-Rt | 2 | 1.019 | 1.020 | 1.186 | 1.352 | 1.169 | 41.4% | n/a | unresolved |
| Mandible | 1222 | 0.996 | 0.997 | 0.974 | 1.022 | 1.008 | 42.6% | n/a | agrees |
| Optic-Nerve-Lt | 5 | 1.022 | 1.021 | 1.012 | 0.789 | 0.868 | 42.8% | n/a | unresolved |
| Optic-Nerve-Rt | 5 | 0.957 | 0.971 | 0.903 | 0.973 | 0.946 | 25.2% | n/a | unresolved |
| Orbit-Lt | 116 | 1.038 | 1.043 | 1.005 | 1.139 | 1.096 | 71.7% | n/a | unresolved |
| Orbit-Rt | 116 | 0.973 | 0.986 | 1.049 | 1.045 | 1.006 | 34.0% | n/a | agrees |
| Parotid-Lt | 520 | 1.150 | 1.120 | 1.231 | 0.993 | 1.020 | 105.4% | n/a | agrees |
| Parotid-Rt | 579 | 0.984 | 0.967 | 1.064 | 1.034 | 1.003 | 27.9% | n/a | agrees |
| Spinal-Canal | 277 | 1.059 | 1.059 | 1.226 | 1.186 | 1.134 | 59.3% | n/a | unresolved |
| Spinal-Cord | 73 | 1.077 | 1.086 | 1.246 | 1.105 | 1.097 | 63.8% | n/a | unresolved |
| Submandibular-Lt | 115 | 1.077 | 1.067 | 0.837 | 1.023 | 1.029 | 116.4% | n/a | agrees |
| Submandibular-Rt | 95 | 1.113 | 1.111 | 0.857 | 0.865 | 0.930 | 90.8% | n/a | unresolved |
| BODY | 66852 | 0.976 | 0.982 | 0.980 | 1.052 | 1.003 | 27.4% | 0.0% (1 vox) | unresolved |
| SKIN | 10415 | 1.064 | 1.037 | 1.067 | 1.042 | 1.054 | 36.5% | n/a | unresolved |
| RESEARCH_TARGET | 959 | 0.941 | 0.950 | 0.931 | 1.032 | 0.952 | 6.9% | n/a | agrees |

- Lacrimal-Rt: total-dose ratio 0.920 is outside 1 +- 5.0%
- Lens-Lt: total-dose ratio 0.895 is outside 1 +- 5.0%
- Lens-Rt: total-dose ratio 1.169 is outside 1 +- 5.0%
- Optic-Nerve-Lt: total-dose ratio 0.868 is outside 1 +- 5.0%
- Optic-Nerve-Rt: total-dose ratio 0.946 is outside 1 +- 5.0%
- Orbit-Lt: total-dose ratio 1.096 is outside 1 +- 5.0%
- Spinal-Canal: total-dose ratio 1.134 is outside 1 +- 5.0%
- Spinal-Cord: total-dose ratio 1.097 is outside 1 +- 5.0%
- Submandibular-Rt: total-dose ratio 0.930 is outside 1 +- 5.0%
- BODY: gamma pass rate 0.0% is below 95.0%
- SKIN: total-dose ratio 1.054 is outside 1 +- 5.0%

Whole-volume agreement (S_N against MC as reference):

| quantity | gamma pass | evaluated voxels | within 2 combined sigma | max difference / MC maximum |
|---|---|---|---|---|
| component:boron | 92.5% | 11674 | n/a | 0.091 |
| component:nitrogen | 66.8% | 16114 | n/a | 0.115 |
| component:hydrogen | 44.4% | 21189 | n/a | 0.269 |
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
