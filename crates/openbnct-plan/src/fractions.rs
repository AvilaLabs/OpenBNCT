//! Fraction-scaled dose pool expansion for multi-fraction planning.
//!
//! BNCT transport is fraction-independent — the physical dose fields
//! don't change between deliveries — but the *biology* does: boron
//! uptake decays over the infusion/washout window, so fraction f's
//! effective field is `Σ_c s_c^(f)·w_c·D_ic(v)` — the isoeffective fold
//! with per-fraction component scales. Scaling the component maps
//! commutes exactly through the linear fold, so an expanded pool of
//! `(beam, fraction)` delivery-time variables plugs straight into the
//! certified solvers: `d_v = Σ_f Σ_i w_i^(f)·E_i^(f)(v)` remains linear.
//!
//! The nonlinear part of fractionation (inter-fraction EQD2/LQ on the
//! IsoE total, `bio_model.fractionation`) stays out of the optimizer —
//! this expansion covers the component-weight dimension only, which is
//! the piece that is exactly linear and certifiable.
//!
//! Because expansion is field-level preprocessing, it composes with
//! every consumer that takes a beam pool: `optimize` (per-fraction
//! delivery-time allocation), `select` (which beams in which
//! fractions), and `--scenario-set` (robust per-fraction planning).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::optimize::{BeamDoseField, DoseQuantity, InversePlanObjective, OptimizeError};

/// Current schema token for fraction-scale documents.
pub const FRACTION_SCALES_SCHEMA: &str = "openbnct.fraction-scales/0.1.0";

/// One delivery fraction's component scaling — `component_scales[c]`
/// multiplies component `c`'s dose map for every beam delivered in this
/// fraction (e.g. `boron: 0.55` for uptake washout on the second
/// irradiation). Exactly the four component names, all finite and
/// non-negative.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FractionScale {
    pub name: String,
    pub component_scales: BTreeMap<String, f64>,
}

/// A versioned fraction-schedule document: the ordered list of delivery
/// fractions and their component scalings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FractionScales {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Ordered fractions — expansion is `fields × fractions`.
    pub fractions: Vec<FractionScale>,
    /// Free-text validity note — what the schedule encodes and where
    /// the per-fraction scales came from (PK curve, assay). Required.
    pub validity_domain: String,
    pub provenance_id: String,
}

impl FractionScales {
    pub fn validate(&self) -> Result<(), OptimizeError> {
        if !openbnct_core::schema_matches(&self.schema_version, FRACTION_SCALES_SCHEMA) {
            return Err(OptimizeError::InvalidObjective(format!(
                "fraction-scales schema mismatch: {}",
                self.schema_version
            )));
        }
        for (label, value) in [
            ("id", self.id.as_str()),
            ("validity_domain", self.validity_domain.as_str()),
            ("provenance_id", self.provenance_id.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(OptimizeError::InvalidObjective(format!(
                    "fraction-scales {label} is required and must not be empty"
                )));
            }
        }
        if self.fractions.is_empty() {
            return Err(OptimizeError::InvalidObjective(
                "fraction-scales requires at least one fraction".into(),
            ));
        }
        let required: BTreeSet<&str> = openbnct_core::DoseComponent::REQUIRED
            .iter()
            .map(|c| crate::optimize::component_name(*c))
            .collect();
        let mut seen = BTreeSet::new();
        for fraction in &self.fractions {
            if fraction.name.trim().is_empty() {
                return Err(OptimizeError::InvalidObjective(
                    "fraction name must not be empty".into(),
                ));
            }
            if !seen.insert(fraction.name.as_str()) {
                return Err(OptimizeError::InvalidObjective(format!(
                    "duplicate fraction name {:?}",
                    fraction.name
                )));
            }
            let present: BTreeSet<&str> = fraction
                .component_scales
                .keys()
                .map(String::as_str)
                .collect();
            if present != required {
                return Err(OptimizeError::InvalidObjective(format!(
                    "fraction {:?} component_scales must cover exactly {required:?}; observed {present:?}",
                    fraction.name
                )));
            }
            for (component, scale) in &fraction.component_scales {
                if !scale.is_finite() || *scale < 0.0 {
                    return Err(OptimizeError::InvalidObjective(format!(
                        "fraction {:?} scale for {component} must be finite and non-negative",
                        fraction.name
                    )));
                }
            }
        }
        Ok(())
    }
}

/// Expand `fields` into the `(beam, fraction)` pool: each output field
/// is one beam delivered in one fraction, with its component maps
/// scaled by that fraction's `component_scales` and `values` carrying
/// the scaled total. Expanded names are `beam@fraction`.
///
/// Requires `spec.dose_quantity == Isoeffective`: component scaling is
/// only meaningful under the component-weighted biological fold, and
/// every field must carry a complete component map set.
pub fn expand_fractions(
    fields: &[BeamDoseField],
    spec: &InversePlanObjective,
    scales: &FractionScales,
) -> Result<Vec<BeamDoseField>, OptimizeError> {
    scales.validate()?;
    if spec.dose_quantity != DoseQuantity::Isoeffective {
        return Err(OptimizeError::InvalidObjective(
            "fraction-scales requires dose_quantity isoeffective — component \
             scaling is a biological weighting, meaningless on a physical total"
                .into(),
        ));
    }
    let mut expanded = Vec::with_capacity(fields.len() * scales.fractions.len());
    for field in fields {
        let comps = field.components.as_ref().ok_or_else(|| {
            OptimizeError::InvalidObjective(format!(
                "beam field {:?} lacks component-resolved doses required by fraction scaling",
                field.name
            ))
        })?;
        for fraction in &scales.fractions {
            let mut scaled_comps = BTreeMap::new();
            let mut total = vec![0.0; field.values.len()];
            for (name, values) in comps {
                let scale = fraction.component_scales[name];
                let scaled: Vec<f64> = values.iter().map(|&d| scale * d).collect();
                for (t, &d) in total.iter_mut().zip(&scaled) {
                    *t += d;
                }
                scaled_comps.insert(name.clone(), scaled);
            }
            expanded.push(BeamDoseField {
                name: format!("{}@{}", field.name, fraction.name),
                values: total,
                components: Some(scaled_comps),
            });
        }
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lp::{self, LpMode};
    use crate::optimize::{DoseObjective, ResultProvenance};
    use openbnct_bio::{BiologicalModel, WeightSemantics};
    use openbnct_core::{ContentReference, DoseUnit, RegionMask};

    /// Photon-isoeffective model: weights are RBE/CBE factors with the
    /// photon weight pinned at 1.0.
    fn bio_model(boron_rbe: f64) -> BiologicalModel {
        BiologicalModel {
            schema_version: openbnct_bio::BIOLOGICAL_MODEL_SCHEMA.into(),
            id: "bio".into(),
            weight_semantics: WeightSemantics::PhotonIsoeffective,
            input_unit: DoseUnit::GrayPerSourceParticle,
            component_weights: [
                ("boron", boron_rbe),
                ("nitrogen", 4.0),
                ("hydrogen", 2.5),
                ("photon", 1.0),
            ]
            .iter()
            .map(|(n, w)| (n.to_string(), *w))
            .collect(),
            region_weights: Default::default(),
            region_priority: Vec::new(),
            component_weight_uncertainty: Default::default(),
            derivation: None,
            validity_domain: Some("unit test".into()),
            fractionation: None,
        }
    }

    fn spec(objectives: Vec<DoseObjective>, model: BiologicalModel) -> InversePlanObjective {
        InversePlanObjective {
            schema_version: crate::optimize::INVERSE_PLAN_OBJECTIVE_SCHEMA.into(),
            id: "obj".into(),
            case_id: "case".into(),
            dose_quantity: DoseQuantity::Isoeffective,
            objectives,
            max_iterations: 500,
            gradient_tolerance: 1e-10,
            weight_bound: None,
            weight_regularization: 1e-5,
            bio_model: Some(model),
            validity_domain: "unit test".into(),
            provenance_id: "test".into(),
        }
    }

    fn scales(fractions: &[(&str, f64)]) -> FractionScales {
        FractionScales {
            schema_version: FRACTION_SCALES_SCHEMA.into(),
            id: "sched".into(),
            fractions: fractions
                .iter()
                .map(|(name, boron)| FractionScale {
                    name: name.to_string(),
                    component_scales: [
                        ("boron", *boron),
                        ("nitrogen", 1.0),
                        ("hydrogen", 1.0),
                        ("photon", 1.0),
                    ]
                    .iter()
                    .map(|(n, s)| (n.to_string(), *s))
                    .collect(),
                })
                .collect(),
            validity_domain: "unit test".into(),
            provenance_id: "test".into(),
        }
    }

    fn field(name: &str, boron: f64, photon: f64) -> BeamDoseField {
        let comps: BTreeMap<String, Vec<f64>> = [
            ("boron", boron),
            ("nitrogen", 0.0),
            ("hydrogen", 0.0),
            ("photon", photon),
        ]
        .iter()
        .map(|(n, v)| (n.to_string(), vec![*v]))
        .collect();
        BeamDoseField {
            name: name.into(),
            values: vec![boron + photon],
            components: Some(comps),
        }
    }

    #[test]
    fn expansion_names_scales_and_validates() {
        let model = bio_model(10.0);
        let s = spec(vec![], model);
        let doc = scales(&[("f1", 1.0), ("f2", 0.5)]);
        let expanded = expand_fractions(&[field("A", 2.0, 1.0)], &s, &doc).unwrap();
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].name, "A@f1");
        assert_eq!(expanded[1].name, "A@f2");
        // Boron scaled 0.5 in f2; photon untouched.
        assert_eq!(expanded[1].components.as_ref().unwrap()["boron"], vec![1.0]);
        assert_eq!(
            expanded[1].components.as_ref().unwrap()["photon"],
            vec![1.0]
        );
    }

    #[test]
    fn fraction_solve_allocates_by_uptake() {
        // One beam, two fractions: f1 boron-rich (uptake good),
        // f2 boron-poor (washed out, rbe-weighted 10→5.5 effective).
        // A tumor floor of 40 is met cheapest by loading f1.
        let model = bio_model(10.0);
        let mut s = spec(
            vec![DoseObjective::MinDoseAtVolume {
                mask: "tumor".into(),
                volume_fraction: 1.0,
                target: 40.0,
                weight: 1.0,
            }],
            model,
        );
        s.weight_regularization = 1e-3; // min-total-fraction-time pull
        let doc = scales(&[("f1", 1.0), ("f2", 0.5)]);
        // Beam A: 2 Gy/components boron + 2 photon per unit → effective
        // f1 = 10·2 + 1·2 = 22, f2 = 10·1 + 1·2 = 12.
        let expanded = expand_fractions(&[field("A", 2.0, 2.0)], &s, &doc).unwrap();
        let masks = vec![RegionMask {
            name: "tumor".into(),
            voxels: vec![true],
        }];
        let result = lp::optimize_weights_lp(
            &expanded,
            &masks,
            &s,
            LpMode::Strict,
            ResultProvenance {
                id: "r".into(),
                provenance_id: "t".into(),
                objective: ContentReference {
                    id: "o".into(),
                    sha256: "0".repeat(64),
                },
            },
        )
        .unwrap();
        // f1 needs 40/22 ≈ 1.82; f2 alone would need 40/12 ≈ 3.33 —
        // the Σw minimizer puts everything into the high-uptake
        // fraction.
        assert!((result.weights[0].weight - 40.0 / 22.0).abs() < 1e-3);
        assert!(result.weights[1].weight < 1e-6);
    }

    #[test]
    fn expansion_rejects_physical_totals_and_bad_scales() {
        let mut s = spec(vec![], bio_model(10.0));
        s.dose_quantity = DoseQuantity::PhysicalTotal;
        let doc = scales(&[("f1", 1.0)]);
        let err = expand_fractions(&[field("A", 1.0, 1.0)], &s, &doc).unwrap_err();
        assert!(err.to_string().contains("isoeffective"));

        // Negative scale rejected.
        let s = spec(vec![], bio_model(10.0));
        let mut bad = scales(&[("f1", -1.0)]);
        let err = expand_fractions(&[field("A", 1.0, 1.0)], &s, &bad).unwrap_err();
        assert!(err.to_string().contains("non-negative"));
        // Missing component rejected.
        bad.fractions[0].component_scales.remove("photon");
        let err = expand_fractions(&[field("A", 1.0, 1.0)], &s, &bad).unwrap_err();
        assert!(err.to_string().contains("component_scales"));
        // Component-less field rejected.
        let doc = scales(&[("f1", 1.0)]);
        let bare = BeamDoseField {
            name: "bare".into(),
            values: vec![1.0],
            components: None,
        };
        let err = expand_fractions(&[bare], &s, &doc).unwrap_err();
        assert!(err.to_string().contains("component"));
    }
}
