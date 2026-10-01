# Boron uptake and timing

The boron component depends on B-10 concentration in the modeled tissue. OpenBNCT supports declared concentration fields, PET-derived estimates and time-varying uptake models. Record the assumptions behind each field.

## Concentration maps

A project can apply blood concentration multiplied by tissue:blood ratios for named regions. Region precedence matters when masks overlap. Values in example projects are illustrative placeholders.

Registered PET/SUV images can be converted using a ratio, calibrated-linear or uniform uptake model. These are model-derived B-10 estimates. Inspect image registration, calibration, assay/compound assumptions and the declared uncertainty before comparing dose.

## Post-hoc scaling

`boron dose` applies a concentration field to unit-concentration boron dose under the trace-B-10 approximation. This lets you compare uptake assumptions using an existing transport result.

That approximation assumes the changed boron does not materially change the transport field. For a materially different absorber composition, bind the appropriate materials and rerun transport. High-concentration cases require particular care with this boundary.

## Kinetics and irradiation windows

Mono- or biexponential models can be fitted to declared concentration samples. Tissue:blood ratios and washout models support time-integrated component maps and comparisons of irradiation windows under declared organ-dose limits.

Separate an assay's sample time from its availability time when using retrospective histories. Check whether a time model is interpolation, extrapolation or an assumed curve. A preferred window depends on the dose limits, uptake model and delivery assumptions you supplied.

Cell-level microdosimetry additionally models compartmental localization, cell-to-cell heterogeneity, stochastic captures and charged-particle tracks. Its cell-specific energy/survival outputs belong to that separate research model.

Commands and examples: [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md), [boron examples](https://github.com/AvilaLabs/OpenBNCT/tree/main/examples), and [cell-model study](https://github.com/AvilaLabs/OpenBNCT/tree/main/validation/cell-microdosimetry-sato2018).
