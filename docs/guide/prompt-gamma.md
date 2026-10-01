# Prompt-gamma research

OpenBNCT supports research calculations connecting B-10 captures to 478 keV emission, detector response and reconstruction. The workflow separates the transport-derived emission field, detector model and inverse problem.

## Forward model

Declare source geometry, detector/aperture acceptance, transport data and calibration. Tools produce emission maps, adjoint detector responses and expected counts under those assumptions. Counts depend on normalization, efficiency and measurement geometry as well as the emission field.

## Inverse model

Regularized non-negative reconstruction estimates an emission field from declared observations and responses. Inspect the residual, regularization and spatial resolution. Fitting counts alone does not establish unique localization or a measured boron concentration map.

The [BeNEdiCTE geometry study](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/pg-benedicte-geometry) illustrates this boundary: the uncollimated inversion does not localize the sources, and the collimated example still merges its two vial sources.

## Time-dependent research

Current source also includes measurement-informed boron and retrospective delivery workflows with explicit calibration, clock, gap and information-availability declarations. Their reduced models and observation assumptions must be assessed for the intended experiment. They do not constitute a commissioned monitoring or machine-control system.

Read the [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md) for the forward/inverse chain and related model contracts. Keep synthetic observations, actual measurements and fitted reconstructions identifiable in a reported study.
