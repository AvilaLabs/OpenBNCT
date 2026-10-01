# Read your results

Start with the quantity, units, normalization, spatial frame and convergence status. Then read each physical component and its uncertainty before interpreting totals or weighted biological dose.

## Physical components and normalization

The main components are boron, nitrogen, hydrogen and photon dose. Their definitions belong to the declared component profile and response data. In comparisons, check what the hydrogen channel represents and how capture photons are handled.

Per-source-particle results are not delivered Gy. A project report can scale dose using both source strength per second and irradiation time. A rate, per-particle quantity and accumulated dose need distinct interpretation; retain the declared source normalization when exchanging files.

Boron dose also depends on the specified B-10 concentration or distribution. Post-hoc scaling uses a trace-boron approximation; it does not recalculate how a changed absorber distribution modifies neutron transport.

## Convergence and verification

A converged iterative solve has met its numerical stopping criterion. Mesh, energy-group, angular, source-model and nuclear-data errors can remain. An allowed unconverged result is provisional and must retain that status.

`project verify` compares the same project with continuous-energy OpenMC:

| Verdict | Meaning |
| --- | --- |
| `AGREES` | All evaluated structure-mean total-dose ratios and gamma pass rates meet the declared gates. |
| `DISAGREES` | At least one evaluated gate fails with adequate Monte Carlo precision. |
| `INCONCLUSIVE` | Monte Carlo uncertainty prevents resolving a failing structure's comparison. |

Input changes make old verification stale. A 15% component agreement observation does not satisfy a 5% total-dose gate automatically; inspect the actual evaluated quantities and thresholds.

## Dose-volume and biological results

D95 is the dose reached by at least 95% of the selected region; Vx is the volume meeting the stated dose threshold. Masks, voxel volume and normalization affect these statistics.

Biological dose applies declared effectiveness or response models. Keep its model, validity domain and weighted units distinct from physical dose. TCP/NTCP outputs describe a chosen research model and parameter set.

See [biological models](biology.md), [uncertainty](uncertainty.md) and [research scope](scope.md).
