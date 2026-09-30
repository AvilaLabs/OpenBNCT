// SPDX-License-Identifier: MIT

//! Post-hoc ¹⁰B dose: the unit-concentration boron dose artifact
//! (`openbnct.boron-unit-dose/0.1.0`) and the application of a ¹⁰B
//! concentration field to a physical dose bundle.
//!
//! BNCT practice treats ¹⁰B as a trace: the neutron field is transported
//! once and the boron dose is evaluated per unit ¹⁰B concentration, then
//! scaled by blood concentration × tissue:blood ratios (or a per-voxel
//! PET-derived field) at evaluation time. The mass kerma per µg/g of ¹⁰B
//! is tissue-independent, so
//!
//! ```text
//! D_boron(v) = C(v) · u(v),   u(v) = Σ_g φ_g(v) · unit_g
//! ```
//!
//! where `unit_g` is `MultigroupData::boron_unit_response_gy_cm2_per_ug_g`.
//!
//! Declared approximation (trace ¹⁰B): the applied ¹⁰B does not perturb
//! the flux φ; the neutron field is the one transported with whatever
//! boron the transport materials carried. A boron microdistribution
//! compound factor declared on a material multiplies the unit dose in
//! that material exactly as it does in the ordinary fold.
//!
//! Research software; not a clinical quantity.

use crate::model::{MaterialAssignment, TransportCase};
use crate::multigroup::{MultigroupData, MultigroupError, MultigroupFlux, fold_multigroup_dose};
use openbnct_core::{
    ContentReference, DoseComponent, DoseUnit, DoseVolume, GridGeometry, PhysicalDoseBundle,
    PhysicalTotalDoseVolume, TotalUncertaintyMethod,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Schema token for the unit-concentration boron dose artifact.
pub const BORON_UNIT_DOSE_SCHEMA: &str = "openbnct.boron-unit-dose/0.1.0";

/// The only unit an `openbnct.boron-unit-dose` value may carry.
pub const BORON_UNIT_DOSE_UNIT: &str = "gray_per_source_particle_per_ug_per_g";

/// Qualification asserted on every emitted unit dose.
pub const BORON_UNIT_DOSE_QUALIFICATION: &str =
    "research-only: trace-10B unit-concentration dose, not a clinical quantity";

/// Declared trace-boron approximation, recorded in provenance.
pub const TRACE_BORON_ASSUMPTION: &str = "trace-10B: applied 10B does not perturb the flux";

#[derive(Debug, Error)]
pub enum BoronUnitError {
    #[error("invalid boron unit dose: {0}")]
    Invalid(String),
    #[error("{0}")]
    Refused(String),
    #[error("multigroup: {0}")]
    Multigroup(#[from] MultigroupError),
    #[error("validation: {0}")]
    Core(#[from] openbnct_core::ValidationError),
}

/// Parse a boron unit dose read from `document`, resolving sidecar arrays
/// relative to it. Does not call [`BoronUnitDose::validate`].
pub fn parse_boron_unit_dose(
    bytes: &[u8],
    document: &std::path::Path,
) -> Result<BoronUnitDose, BoronUnitError> {
    openbnct_core::sidecar::from_slice_at(bytes, document)
        .map_err(|error| BoronUnitError::Invalid(error.to_string()))
}

/// Read, deserialize, resolve sidecars of, and validate a boron unit dose.
pub fn load_boron_unit_dose(path: &std::path::Path) -> Result<BoronUnitDose, BoronUnitError> {
    let unit: BoronUnitDose = openbnct_core::sidecar::load_json(path)
        .map_err(|error| BoronUnitError::Invalid(error.to_string()))?;
    unit.validate()?;
    Ok(unit)
}

/// Dose per source particle per µg/g of ¹⁰B, per voxel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoronUnitDose {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub geometry: GridGeometry,
    /// Always [`BORON_UNIT_DOSE_UNIT`].
    pub unit: String,
    /// Gy per source particle per µg/g, grid order `i + nx·j + nx·ny·k`.
    #[serde(with = "openbnct_core::sidecar::values")]
    pub values: Vec<f64>,
    /// Optional 1σ statistical uncertainty in the same unit. The
    /// deterministic S_N fold carries none.
    #[serde(default, with = "openbnct_core::sidecar::opt_uncertainty")]
    pub absolute_standard_uncertainty: Option<Vec<f64>>,
    /// The flux this dose was folded from.
    pub flux: ContentReference,
    /// The multigroup data whose unit vector was used.
    pub multigroup_data: ContentReference,
    /// Declared approximations (trace ¹⁰B).
    pub assumptions: String,
    pub qualification: String,
    pub provenance_id: String,
}

impl BoronUnitDose {
    pub fn validate(&self) -> Result<(), BoronUnitError> {
        let bad = |m: String| BoronUnitError::Invalid(m);
        if !openbnct_core::schema_matches(&self.schema_version, BORON_UNIT_DOSE_SCHEMA) {
            return Err(bad(format!("unsupported schema {:?}", self.schema_version)));
        }
        for (label, v) in [
            ("id", &self.id),
            ("case_id", &self.case_id),
            ("assumptions", &self.assumptions),
            ("qualification", &self.qualification),
            ("provenance_id", &self.provenance_id),
        ] {
            if v.trim().is_empty() {
                return Err(bad(format!("{label} must be nonempty")));
            }
        }
        if self.unit != BORON_UNIT_DOSE_UNIT {
            return Err(bad(format!(
                "unit must be {BORON_UNIT_DOSE_UNIT:?}, got {:?}",
                self.unit
            )));
        }
        let n = self.geometry.voxel_count()?;
        if self.values.len() != n {
            return Err(bad(format!(
                "values length {} does not match grid voxel count {n}",
                self.values.len()
            )));
        }
        if self.values.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(bad("values must be finite and non-negative".into()));
        }
        if let Some(sigma) = &self.absolute_standard_uncertainty {
            if sigma.len() != n {
                return Err(bad(format!(
                    "uncertainty length {} does not match grid voxel count {n}",
                    sigma.len()
                )));
            }
            if sigma.iter().any(|v| !v.is_finite() || *v < 0.0) {
                return Err(bad("uncertainty must be finite and non-negative".into()));
            }
        }
        self.flux
            .validate()
            .map_err(|e| bad(format!("flux reference: {e}")))?;
        self.multigroup_data
            .validate()
            .map_err(|e| bad(format!("multigroup_data reference: {e}")))?;
        Ok(())
    }
}

/// Fold a converged flux through the data's tissue-independent ¹⁰B unit
/// response into a [`BoronUnitDose`]. Refuses data collapsed before the
/// unit vector existed.
///
/// The fold is the ordinary [`fold_multigroup_dose`] run on a copy of the
/// data whose every material carries the unit vector as its only
/// response, so blended partial cells and boron-microdistribution
/// compound factors are applied exactly as in the dose-bundle fold.
pub fn fold_boron_unit_dose(
    case: &TransportCase,
    data: &MultigroupData,
    flux: &MultigroupFlux,
    assignment: Option<&MaterialAssignment>,
    data_ref: ContentReference,
    flux_ref: ContentReference,
) -> Result<BoronUnitDose, BoronUnitError> {
    let unit = data
        .boron_unit_response_gy_cm2_per_ug_g
        .as_ref()
        .ok_or_else(|| {
            BoronUnitError::Refused(
                "multigroup data carries no boron_unit_response_gy_cm2_per_ug_g \
                 (collapsed before the unit vector existed); re-run `sn collapse` \
                 with a B10 table to obtain it"
                    .into(),
            )
        })?;
    let mut unit_data = data.clone();
    for material in &mut unit_data.materials {
        material.dose_response_gy_cm2.clear();
        material
            .dose_response_gy_cm2
            .insert("boron".into(), unit.clone());
    }
    let placeholder = ContentReference {
        id: "boron-unit-fold".into(),
        sha256: "0".repeat(64),
    };
    let bundle = fold_multigroup_dose(
        case,
        &unit_data,
        flux,
        assignment,
        placeholder.clone(),
        placeholder,
    )?;
    let values = bundle
        .components
        .into_iter()
        .find(|c| c.component == DoseComponent::Boron)
        .ok_or_else(|| BoronUnitError::Invalid("unit fold produced no boron component".into()))?
        .values;
    let out = BoronUnitDose {
        schema_version: BORON_UNIT_DOSE_SCHEMA.into(),
        id: format!("{}.boron-unit-dose", case.case_id),
        case_id: case.case_id.clone(),
        geometry: case.geometry.clone(),
        unit: BORON_UNIT_DOSE_UNIT.into(),
        values,
        absolute_standard_uncertainty: None,
        flux: flux_ref,
        multigroup_data: data_ref,
        assumptions: TRACE_BORON_ASSUMPTION.into(),
        qualification: BORON_UNIT_DOSE_QUALIFICATION.into(),
        provenance_id: flux.provenance_id.clone(),
    };
    out.validate()?;
    Ok(out)
}

/// One tissue:blood ratio region. The first region whose mask covers a
/// voxel wins.
#[derive(Debug, Clone)]
pub struct RatioRegion {
    pub name: String,
    pub ratio: f64,
    pub mask: Vec<bool>,
}

/// Per-voxel ¹⁰B concentration (µg/g) from blood concentration ×
/// tissue:blood ratios; voxels covered by no mask get `default_ratio`.
pub fn concentration_from_ratios(
    blood_ug_g: f64,
    regions: &[RatioRegion],
    default_ratio: f64,
    voxel_count: usize,
) -> Result<Vec<f64>, BoronUnitError> {
    let bad = |m: String| BoronUnitError::Refused(m);
    if !blood_ug_g.is_finite() || blood_ug_g < 0.0 {
        return Err(bad("blood concentration must be finite and >= 0".into()));
    }
    if !default_ratio.is_finite() || default_ratio < 0.0 {
        return Err(bad("default ratio must be finite and >= 0".into()));
    }
    for region in regions {
        if !region.ratio.is_finite() || region.ratio < 0.0 {
            return Err(bad(format!(
                "ratio for {:?} must be finite and >= 0",
                region.name
            )));
        }
        if region.mask.len() != voxel_count {
            return Err(bad(format!(
                "mask {:?} has {} voxels; the grid has {voxel_count}",
                region.name,
                region.mask.len()
            )));
        }
    }
    Ok((0..voxel_count)
        .map(|v| {
            let ratio = regions
                .iter()
                .find(|r| r.mask[v])
                .map_or(default_ratio, |r| r.ratio);
            blood_ug_g * ratio
        })
        .collect())
}

/// Apply a ¹⁰B concentration field to a physical dose bundle: the boron
/// component becomes `C(v)·u(v)`, and the physical total is
/// `old_total − old_boron + new_boron` (floored at 0 against rounding).
///
/// Uncertainty: the new boron σ combines, in quadrature, the unit-dose
/// statistical σ scaled by `C` and the concentration σ scaled by `u`
/// (whichever exist). The total's σ is only defined when the input
/// bundle carries both a total σ and a boron-component σ; it is then the
/// conservative triangle bound `σ_T,old + σ_B,old + σ_B,new` (the
/// non-boron part's σ cannot exceed `σ_T,old + σ_B,old` under any
/// covariance), keeping the input's total-uncertainty method label.
/// Otherwise the total's uncertainty is `Unavailable`. Other components
/// are carried over unchanged.
pub fn apply_boron_concentration(
    physical: &PhysicalDoseBundle,
    unit: &BoronUnitDose,
    concentration_ug_g: &[f64],
    concentration_sigma_ug_g: Option<&[f64]>,
    spec_description: &str,
) -> Result<PhysicalDoseBundle, BoronUnitError> {
    let refuse = |m: String| BoronUnitError::Refused(m);
    unit.validate()?;
    physical.validate()?;
    if physical.case_id != unit.case_id {
        return Err(refuse(format!(
            "case_id mismatch: physical bundle {:?} vs unit dose {:?}",
            physical.case_id, unit.case_id
        )));
    }
    if physical.geometry != unit.geometry {
        return Err(refuse(
            "grid geometry of the physical bundle and the unit dose differ".into(),
        ));
    }
    let n = unit.values.len();
    if concentration_ug_g.len() != n || concentration_sigma_ug_g.is_some_and(|s| s.len() != n) {
        return Err(refuse(format!(
            "concentration field length does not match the grid voxel count {n}"
        )));
    }
    if concentration_ug_g
        .iter()
        .chain(concentration_sigma_ug_g.unwrap_or(&[]))
        .any(|c| !c.is_finite() || *c < 0.0)
    {
        return Err(refuse(
            "concentration and its sigma must be finite and non-negative".into(),
        ));
    }
    let old_boron = physical
        .components
        .iter()
        .find(|c| c.component == DoseComponent::Boron)
        .ok_or_else(|| refuse("physical bundle has no boron component".into()))?;

    let new_values: Vec<f64> = concentration_ug_g
        .iter()
        .zip(&unit.values)
        .map(|(c, u)| c * u)
        .collect();
    let new_sigma: Option<Vec<f64>> = if unit.absolute_standard_uncertainty.is_some()
        || concentration_sigma_ug_g.is_some()
    {
        Some(
            (0..n)
                .map(|v| {
                    let from_unit = unit
                        .absolute_standard_uncertainty
                        .as_ref()
                        .map_or(0.0, |s| concentration_ug_g[v] * s[v]);
                    let from_conc = concentration_sigma_ug_g.map_or(0.0, |s| s[v] * unit.values[v]);
                    from_unit.hypot(from_conc)
                })
                .collect(),
        )
    } else {
        None
    };

    let total_values: Vec<f64> = (0..n)
        .map(|v| (physical.physical_total.values[v] - old_boron.values[v] + new_values[v]).max(0.0))
        .collect();
    let (total_sigma, method) = match (
        &physical.physical_total.absolute_standard_uncertainty,
        &old_boron.absolute_standard_uncertainty,
    ) {
        (Some(t), Some(b)) => (
            Some(
                (0..n)
                    .map(|v| t[v] + b[v] + new_sigma.as_ref().map_or(0.0, |s| s[v]))
                    .collect::<Vec<f64>>(),
            ),
            physical.physical_total.uncertainty_method,
        ),
        _ => (None, TotalUncertaintyMethod::Unavailable),
    };

    let components = physical
        .components
        .iter()
        .map(|c| {
            if c.component == DoseComponent::Boron {
                DoseVolume {
                    component: DoseComponent::Boron,
                    unit: DoseUnit::GrayPerSourceParticle,
                    values: new_values.clone(),
                    absolute_standard_uncertainty: new_sigma.clone(),
                }
            } else {
                c.clone()
            }
        })
        .collect();
    let bundle = PhysicalDoseBundle {
        schema_version: physical.schema_version.clone(),
        case_id: physical.case_id.clone(),
        frame_of_reference_uid: physical.frame_of_reference_uid.clone(),
        geometry: physical.geometry.clone(),
        component_profile: physical.component_profile.clone(),
        response_set: physical.response_set.clone(),
        components,
        physical_total: PhysicalTotalDoseVolume {
            unit: physical.physical_total.unit,
            values: total_values,
            absolute_standard_uncertainty: total_sigma,
            uncertainty_method: method,
        },
        provenance_id: format!(
            "{}|posthoc-boron[{spec_description}; unit_dose={}; {TRACE_BORON_ASSUMPTION}]",
            physical.provenance_id, unit.provenance_id
        ),
    };
    bundle.validate()?;
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multigroup::tests::{cref, data, options, slab_case};
    use crate::multigroup::{fold_multigroup_dose, solve_multigroup};

    const UNIT: f64 = 3.0e-12;
    const CONC: f64 = 25.0;

    /// Data whose material carries `CONC` µg/g worth of boron response
    /// (`CONC·UNIT`) plus the three other required components.
    fn fixture(with_unit: bool) -> (TransportCase, MultigroupData, MultigroupFlux) {
        let case = slab_case();
        let mut mg = data(&[0.1], vec![0.0]);
        mg.materials[0].dose_response_gy_cm2 = [
            ("boron".to_string(), vec![CONC * UNIT]),
            ("nitrogen".to_string(), vec![2.0e-13]),
            ("hydrogen".to_string(), vec![5.0e-13]),
            ("photon".to_string(), vec![7.0e-13]),
        ]
        .into_iter()
        .collect();
        if with_unit {
            mg.boron_unit_response_gy_cm2_per_ug_g = Some(vec![UNIT]);
        }
        let flux = solve_multigroup(&case, &mg, &options(), cref("mg"), cref("case")).unwrap();
        (case, mg, flux)
    }

    fn unit_dose(
        case: &TransportCase,
        mg: &MultigroupData,
        flux: &MultigroupFlux,
    ) -> BoronUnitDose {
        fold_boron_unit_dose(case, mg, flux, None, cref("mg"), cref("flux")).unwrap()
    }

    fn physical(
        case: &TransportCase,
        mg: &MultigroupData,
        flux: &MultigroupFlux,
    ) -> PhysicalDoseBundle {
        let mut bundle =
            fold_multigroup_dose(case, mg, flux, None, cref("profile"), cref("rs")).unwrap();
        assert_eq!(bundle.components.len(), 4);
        bundle.provenance_id = "orig-run".into();
        bundle
    }

    fn comp(b: &PhysicalDoseBundle, c: DoseComponent) -> &DoseVolume {
        b.components.iter().find(|x| x.component == c).unwrap()
    }

    #[test]
    fn uniform_concentration_reproduces_original_bundle() {
        let (case, mg, flux) = fixture(true);
        let unit = unit_dose(&case, &mg, &flux);
        unit.validate().unwrap();
        assert_eq!(unit.unit, BORON_UNIT_DOSE_UNIT);
        let orig = physical(&case, &mg, &flux);
        let n = unit.values.len();
        let out =
            apply_boron_concentration(&orig, &unit, &vec![CONC; n], None, "uniform 25").unwrap();
        for (a, b) in out.components.iter().zip(&orig.components) {
            assert_eq!(a.component, b.component);
            for (x, y) in a.values.iter().zip(&b.values) {
                assert!((x - y).abs() <= 1e-12 * y.abs(), "{x} vs {y}");
            }
        }
        for (x, y) in out
            .physical_total
            .values
            .iter()
            .zip(&orig.physical_total.values)
        {
            assert!((x - y).abs() <= 1e-12 * y.abs());
        }
        assert!(out.provenance_id.contains("posthoc-boron"));
        assert!(out.provenance_id.contains("does not perturb the flux"));
        assert!(out.provenance_id.starts_with("orig-run|"));
    }

    #[test]
    fn missing_unit_vector_is_refused_with_recollapse_hint() {
        let (case, mg, flux) = fixture(false);
        let err = fold_boron_unit_dose(&case, &mg, &flux, None, cref("mg"), cref("flux"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("sn collapse"), "{err}");
    }

    #[test]
    fn ratio_masks_first_match_wins_and_default_applies() {
        let mask = |f: &dyn Fn(usize) -> bool| (0..8).map(f).collect::<Vec<bool>>();
        let regions = vec![
            RatioRegion {
                name: "tumor".into(),
                ratio: 3.5,
                mask: mask(&|v| v < 3),
            },
            RatioRegion {
                name: "brain".into(),
                ratio: 1.0,
                mask: mask(&|v| v < 6),
            },
        ];
        let c = concentration_from_ratios(20.0, &regions, 0.5, 8).unwrap();
        assert_eq!(c, vec![70.0, 70.0, 70.0, 20.0, 20.0, 20.0, 10.0, 10.0]);
        assert!(concentration_from_ratios(-1.0, &regions, 0.5, 8).is_err());
        assert!(concentration_from_ratios(20.0, &regions, f64::NAN, 8).is_err());
        assert!(concentration_from_ratios(20.0, &regions, 1.0, 9).is_err());
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn regional_concentration_scales_boron_only() {
        let (case, mg, flux) = fixture(true);
        let unit = unit_dose(&case, &mg, &flux);
        let orig = physical(&case, &mg, &flux);
        let n = unit.values.len();
        let conc: Vec<f64> = (0..n)
            .map(|v| if v % 2 == 0 { 10.0 } else { 40.0 })
            .collect();
        let out = apply_boron_concentration(&orig, &unit, &conc, None, "test").unwrap();
        let boron = comp(&out, DoseComponent::Boron);
        let nitrogen = comp(&out, DoseComponent::Nitrogen);
        let nitrogen0 = comp(&orig, DoseComponent::Nitrogen);
        for v in 0..n {
            let want = conc[v] * unit.values[v];
            assert!((boron.values[v] - want).abs() <= 1e-15 * want.abs());
            assert_eq!(nitrogen.values[v], nitrogen0.values[v]);
            let old_b = comp(&orig, DoseComponent::Boron).values[v];
            let want_total = orig.physical_total.values[v] - old_b + want;
            assert!((out.physical_total.values[v] - want_total.max(0.0)).abs() <= 1e-15);
        }
    }

    #[test]
    fn mismatches_are_refused() {
        let (case, mg, flux) = fixture(true);
        let unit = unit_dose(&case, &mg, &flux);
        let orig = physical(&case, &mg, &flux);
        let n = unit.values.len();
        let ok = vec![CONC; n];
        let mut other_case = unit.clone();
        other_case.case_id = "another".into();
        assert!(apply_boron_concentration(&orig, &other_case, &ok, None, "x").is_err());
        let mut other_grid = unit.clone();
        other_grid.geometry.origin_mm[0] += 0.5;
        assert!(apply_boron_concentration(&orig, &other_grid, &ok, None, "x").is_err());
        assert!(apply_boron_concentration(&orig, &unit, &ok[1..], None, "x").is_err());
        let mut negative = ok.clone();
        negative[0] = -1.0;
        assert!(apply_boron_concentration(&orig, &unit, &negative, None, "x").is_err());
    }

    #[test]
    fn sigma_combines_in_quadrature_and_total_is_a_triangle_bound() {
        let (case, mg, flux) = fixture(true);
        let mut unit = unit_dose(&case, &mg, &flux);
        let n = unit.values.len();
        unit.absolute_standard_uncertainty = Some(unit.values.iter().map(|u| 0.1 * u).collect());
        let mut orig = physical(&case, &mg, &flux);
        let conc_sigma = vec![2.0; n];
        let out = apply_boron_concentration(&orig, &unit, &vec![CONC; n], Some(&conc_sigma), "x")
            .unwrap();
        assert_eq!(
            out.physical_total.uncertainty_method,
            TotalUncertaintyMethod::Unavailable
        );
        let s = comp(&out, DoseComponent::Boron)
            .absolute_standard_uncertainty
            .as_ref()
            .unwrap();
        let v = 100;
        let want = (CONC * 0.1 * unit.values[v]).hypot(2.0 * unit.values[v]);
        assert!((s[v] - want).abs() <= 1e-12 * want);
        for c in &mut orig.components {
            if c.component == DoseComponent::Boron {
                c.absolute_standard_uncertainty = Some(vec![1.0e-15; n]);
            }
        }
        orig.physical_total.absolute_standard_uncertainty = Some(vec![3.0e-15; n]);
        orig.physical_total.uncertainty_method = TotalUncertaintyMethod::DedicatedEstimator;
        let out = apply_boron_concentration(&orig, &unit, &vec![CONC; n], Some(&conc_sigma), "x")
            .unwrap();
        let t = out
            .physical_total
            .absolute_standard_uncertainty
            .as_ref()
            .unwrap();
        assert!((t[v] - (3.0e-15 + 1.0e-15 + want)).abs() <= 1e-12 * t[v]);
        assert_eq!(
            out.physical_total.uncertainty_method,
            TotalUncertaintyMethod::DedicatedEstimator
        );
    }

    #[test]
    fn artifact_validation_rejects_bad_documents() {
        let (case, mg, flux) = fixture(true);
        let good = unit_dose(&case, &mg, &flux);
        let mut d = good.clone();
        d.schema_version = "openbnct.boron-unit-dose/9.0.0".into();
        assert!(d.validate().is_err());
        let mut d = good.clone();
        d.unit = "gray".into();
        assert!(d.validate().is_err());
        let mut d = good.clone();
        d.values.pop();
        assert!(d.validate().is_err());
        let mut d = good.clone();
        d.values[0] = f64::NAN;
        assert!(d.validate().is_err());
        let mut d = good.clone();
        d.absolute_standard_uncertainty = Some(vec![-1.0; d.values.len()]);
        assert!(d.validate().is_err());
        let mut d = good.clone();
        d.flux.sha256 = "zz".into();
        assert!(d.validate().is_err());
        let mut d = good.clone();
        d.assumptions = " ".into();
        assert!(d.validate().is_err());
        let mut json = serde_json::to_value(&good).unwrap();
        json["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<BoronUnitDose>(json).is_err());
        let back: BoronUnitDose =
            serde_json::from_str(&serde_json::to_string(&good).unwrap()).unwrap();
        assert_eq!(back, good);
    }
}
