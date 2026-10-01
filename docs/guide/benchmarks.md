# Benchmarks and code comparisons

OpenBNCT checks transport calculations against analytic solutions, published numerical problems, independent Monte Carlo and measured/digitized phantom data. Each comparison supports a particular claim under its stated inputs and gates.

## What the comparison measures

| Evidence | Question |
| --- | --- |
| Analytic oracle | Does this modeled case recover a known answer? |
| Canonical transport case | How does the solver behave under declared mesh and angular resolution? |
| Independent-code comparison | Do two methods agree on matched geometry, source, materials and responses? |
| Measured phantom comparison | Does a specific modeled observable match the declared measurement? |
| Parser/model conformance | Does a supported contract produce or reject the expected fixture? |

Convergence and artifact verification are separate from these physical comparisons. A frozen historical result does not promise every later configuration will pass.

## Deterministic transport versus OpenMC

The current-source project workflow has a documented synthetic layered-head comparison with continuous-energy OpenMC, matched thermal scattering and transported capture photons. The README records S8 and 5 million histories. Each component column below is the ratio S_N/MC:

| Region | Boron | Hydrogen | Photon |
| --- | ---: | ---: | ---: |
| Whole phantom | 0.86 | 0.98 | 0.93 |
| Target | 1.06 | 0.90 | 1.03 |

A ratio of 1 means equal mean dose. A ratio of 0.86 is 14% below the Monte Carlo result; 1.06 is 6% above. These values describe structure means, not every voxel or a gamma pass rate. That fixture has no nitrogen component to grade.

These observations are stated in the [current-source README](https://github.com/AvilaLabs/OpenBNCT/blob/main/README.md) and project report implementation. The [earlier layered-head note](https://github.com/AvilaLabs/OpenBNCT/blob/main/validation/intercomparison-layered-head/2026-09-29-accuracy-fixes.md) describes older S4/free-gas and local-photon configurations; its ratios answer a different comparison. It must not be combined with the later matched-physics result.

Source-bin weighting, thermal scattering, component response and photon policy matter. With local capture-photon deposition, the documented photon mean was roughly 3–4 times high. The project photon-transport path changes that model; one component improvement does not qualify all transport cases.

Use `project verify` on your project rather than borrowing this fixture's ratios. Its default gates are total-dose ratio within 5% and gamma pass rate at least 95% for evaluated structures, using global 3%/3 mm gamma with a 10% low-dose cutoff. Monte Carlo uncertainty can make the verdict inconclusive. A component within 15% does not automatically pass these stricter, differently defined gates.

## Public CT geometry: a separate comparison

The [head-and-neck CT benchmark](https://github.com/AvilaLabs/OpenBNCT/blob/main/validation/real-anatomy/hn-head-ct/README.md) uses a public scan cropped to 206,226 voxels at 4 mm, with an explicitly synthetic research target and illustrative boron assumptions. Its final recorded source build is `b46ffb2`, with 28 neutron groups, S4/P1, transported photons and 6 million OpenMC histories.

Structure-mean total-dose S_N/MC ratios are 0.982 for brain, 1.003 for BODY and 0.952 for the synthetic target. These are close regional means; they do not establish voxelwise agreement. The recorded overall verdict is **INCONCLUSIVE**, with eleven structures unresolved under the configured gates. Monte Carlo photon/total voxels were particularly noisy, and BODY's reported gamma result came from a single evaluated voxel.

Keep the photon policy and build identity with these results. The earlier local-deposition run used a different build and reported photon ratios of 6.03 in brain and 8.10 in the target. Comparing those runs does not isolate one code change. The final run's single-host timing was also observed with other jobs active, so it is not a controlled speed comparison.

This extends the tested geometry beyond a synthetic phantom. The scan, synthetic target, coarse small structures, illustrative uptake and unresolved statistics retain their stated research scope.

## Analytic and canonical evidence

[NF-BNCT-003](https://github.com/AvilaLabs/OpenBNCT/tree/main/benchmarks/synthetic/nf-bnct-003) tests a one-group near-pure B-10 absorber. The recorded boron-dose attenuation slope matches the analytic −0.2308 cm⁻¹ within the declared tolerance. It tests that simple model rather than heterogeneous anatomical accuracy.

[Reed](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/canonical-reed-problem) and [Azmy](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/canonical-azmy-problem) test source deposition, heterogeneity and discretization against published references. Reed records near-percent regional agreement; Azmy's quadrant errors are 0.17%, 0.67% and 3.9% under its declared setup.

[Kobayashi](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/canonical-kobayashi-p1) passes predeclared near-field probes at 5%, while documenting severe deep-field ray effects. The [OpenMC multigroup comparison](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/intercomparison-kobayashi-p1) reports off-lobe underfill and large diagonal overshoot. Increasing angular order can sharpen those lobes. Its near-field pass does not establish full-field agreement.

## Measured phantom comparisons

The historical three-group FiR 1 cylindrical-water comparison records peak-normalized depth-profile χ² = 6.7 over 12 bins. The PMMA comparison records χ² = 6.0 over 15 bins. Their sigma-weighted passes concern those normalized profiles, digitized from TECDOC-1223 figures.

Peak normalization removes overall amplitude, so a shape pass does not establish absolute dose. Later 28-group variants retain documented differences; the old three-group verdict cannot be transferred to them. Consult the [water](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/fir1-k63-cylindrical-phantom) and [PMMA](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/fir1-k63-pmma-phantom) records for the exact configurations.

The [0.3.0 rerun of the water cylinder](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/fir1-k63-cylindrical-rerun-2026-10) uses current data, the solver after the September accuracy fixes, and the 118-bin INEEL spectrum. Its absolute χ² is 55 over 12 bins (132 for the September 28-group record). Computed/measured runs from 0.58 at 1 cm through 1.04 at 3.8 cm to 1.58 at 8.75 cm. The scale deficit is gone, but the profile is flatter than measured, so neither the absolute nor the peak-normalized comparison passes.

## Other research evidence

NF-BNCT-001's 600-million-history OpenMC candidate passes its statistical gates; independent reproduction remains required for reference promotion. NF-BNCT-002's frozen deep-penetration case remains unexecuted in the evidence catalogue.

Scenario optimization, PK timing, microdosimetry, joint uncertainty and prompt-gamma studies have their own models and gates. A known-answer optimizer fixture tests that optimization problem. Prompt-gamma examples also record localization failures. See the [validation dossier](https://github.com/AvilaLabs/OpenBNCT/blob/main/validation/VALIDATION.md) and [benchmark catalogue](https://github.com/AvilaLabs/OpenBNCT/blob/main/benchmark-catalogue.json).

## What a competitive comparison can establish

OpenMC comparisons support independent transport assessment on matched research cases. MCNP/PHITS parser fixtures support interchange. The repository does not establish clinical equivalence or broad superiority over commercial treatment-planning systems. A meaningful code comparison needs matching source physics, data, geometry, dose definitions, resolution and acceptance criteria, with statistical uncertainty and runtime conditions reported.
