# Research scope and qualification

OpenBNCT is experimental research software. It has not been clinically validated, commissioned for a treatment facility or authorized as a medical device. Do not use its output as the sole or primary basis for patient care, treatment delivery or regulatory submissions.

The [repository disclaimer](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/DISCLAIMER.md) records this boundary. The MIT license remains a general software license; qualification describes the state of the software and evidence.

## Interpret evidence by its scope

Software tests check implementation behavior. Analytic and canonical problems check specified equations and numerical regimes. Independent-code comparisons assess matched methods and inputs. Measured comparisons assess declared observables with their measurement and modeling limitations.

A passing synthetic demonstration cannot replace facility-specific physics, calibration, commissioning or external reproduction. Model validity, geometry transforms, boron assumptions, nuclear data and dose normalization require independent assessment for a reported study.

## Current research boundaries

The deterministic method can show strong ray effects in void/deep-field regimes. Energy condensation and source/data choices can produce substantial model differences. Biological and uptake models remain conditional on their parameter evidence.

Positioning scenarios currently shift dose fields rather than rerunning transport in moved anatomy. Joint uncertainty tools require explicit source coverage and distributions. Prompt-gamma inversions depend on detector acceptance, calibration and identifiability. Individual features can differ across CLI, Python and GUI surfaces.

The [validation dossier](https://github.com/AvilaLabs/OpenBNCT/blob/main/validation/VALIDATION.md), [qualification record](https://github.com/AvilaLabs/OpenBNCT/blob/main/qualification-record.json) and [roadmap](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/ROADMAP.md) track scoped evidence and open work. Keep the exact source version with your study because later code changes do not rewrite frozen benchmark history.
