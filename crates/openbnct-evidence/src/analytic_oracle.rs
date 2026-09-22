// SPDX-License-Identifier: MIT

//! Analytic transport oracles (`openbnct.analytic-oracle/0.1.0`,
//! `openbnct.analytic-oracle-evaluation/0.1.0`).
//!
//! Where metamorphic oracles check a run against a *relation*, an analytic
//! oracle checks it against a closed-form expectation declared up front.
//! The first (and currently only) supported law is exponential attenuation
//! of a dose component along one grid axis — the pure-absorber slab
//! benchmark `NF-BNCT-003` — where the boron component follows
//! `D(z) ∝ exp(−Σ_t·z)` with Σ_t derived from the bound material and the
//! source energy.
//!
//! The oracle artifact binds the material and source it assumes by content
//! hash so the expectation cannot drift from the inputs it was derived
//! from. The evaluation performs a weighted least-squares fit of
//! `ln D(z)` over the declared window — pooling each perpendicular plane —
//! and compares the fitted slope against the declared attenuation
//! coefficient under a relative tolerance that must cover the law's stated
//! approximation bound (e.g. residual single-scatter buildup).
//!
//! A passing evaluation is *consistent-with* evidence, not proof of
//! correctness; a failing one is a genuine defect signal.

use serde::{Deserialize, Serialize};

use openbnct_core::{ContentReference, PhysicalDoseBundle};

use crate::ManifestError;

/// Versioned analytic-oracle declaration schema.
pub const ANALYTIC_ORACLE_SCHEMA: &str = "openbnct.analytic-oracle/0.1.0";
/// Versioned evaluation-record schema.
pub const ANALYTIC_EVALUATION_SCHEMA: &str = "openbnct.analytic-oracle-evaluation/0.1.0";

const QUALIFICATION: &str =
    "research-only: analytic-consistency evidence, not clinical or equivalence qualification";

/// The closed-form law the oracle declares. Only exponential attenuation
/// is defined today; further laws extend this enum, never overload it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyticLaw {
    /// `D(z) ∝ exp(−Σ_t·z)` along `axis`.
    ExponentialAttenuation,
}

/// Declared closed-form expectation bound to the inputs it assumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyticOracle {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    pub law: AnalyticLaw,
    /// Dose component the law applies to, e.g. `component:boron`.
    pub quantity: String,
    /// Grid/world axis the profile is sampled along: `x`, `y`, or `z`.
    /// Requires an axis-aligned (identity-direction) grid.
    pub axis: String,
    /// Expected attenuation coefficient Σ_t in cm⁻¹.
    pub attenuation_per_cm: f64,
    /// Allowed relative deviation of the fitted slope magnitude.
    pub relative_tolerance: f64,
    /// World-coordinate fit window in cm along `axis`.
    pub fit_window_cm: [f64; 2],
    /// Minimum bins that must fall inside the window.
    pub min_bins: u32,
    /// Artifacts the expectation assumes (material, source, nuclear-data
    /// manifest) — content-bound so the declared Σ_t stays honest.
    pub assumptions: Vec<ContentReference>,
    /// Statement of the approximation the tolerance covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Outcome of fitting the declared law to a dose bundle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyticOracleEvaluation {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// The oracle artifact evaluated, content-bound.
    pub oracle: ContentReference,
    /// The dose bundle evaluated, content-bound.
    pub inputs: Vec<ContentReference>,
    pub quantity: String,
    pub axis: String,
    pub fit_window_cm: [f64; 2],
    /// Plane-averaged bins that entered the fit.
    pub bins_evaluated: u64,
    /// Least-squares log-slope in cm⁻¹ (negative for attenuation).
    pub fitted_slope_per_cm: f64,
    /// Standard uncertainty of the fitted slope — present only when the
    /// component carries per-voxel uncertainties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fitted_slope_sigma_per_cm: Option<f64>,
    /// `−attenuation_per_cm` from the oracle.
    pub expected_slope_per_cm: f64,
    /// `|fitted − expected| / |expected|`.
    pub relative_deviation: f64,
    pub relative_tolerance: f64,
    /// Weighted RMS of `ln D` residuals about the fit — how exponential
    /// the profile actually is, independent of the slope value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_residual_rms: Option<f64>,
    pub passed: bool,
    /// Research-status qualification; no equivalence or clinical claim.
    pub qualification: String,
    pub provenance_id: String,
}

fn invalid(msg: String) -> ManifestError {
    ManifestError::Invalid(format!("analytic oracle: {msg}"))
}

impl AnalyticOracle {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if !openbnct_core::schema_matches(&self.schema_version, ANALYTIC_ORACLE_SCHEMA) {
            return Err(ManifestError::Invalid(format!(
                "analytic oracle: unsupported schema {:?}",
                self.schema_version
            )));
        }
        if self.id.trim().is_empty() || self.case_id.trim().is_empty() {
            return Err(invalid("id and case_id must be nonempty".into()));
        }
        if !matches!(self.axis.as_str(), "x" | "y" | "z") {
            return Err(invalid(format!("axis must be x|y|z, got {:?}", self.axis)));
        }
        if !self.attenuation_per_cm.is_finite() || self.attenuation_per_cm <= 0.0 {
            return Err(invalid("attenuation_per_cm must be positive".into()));
        }
        if !self.relative_tolerance.is_finite() || self.relative_tolerance <= 0.0 {
            return Err(invalid("relative_tolerance must be positive".into()));
        }
        let [lo, hi] = self.fit_window_cm;
        if !lo.is_finite() || !hi.is_finite() || lo >= hi {
            return Err(invalid("fit_window_cm must be finite and ordered".into()));
        }
        if self.min_bins < 3 {
            return Err(invalid(
                "min_bins must be at least 3 for a slope fit".into(),
            ));
        }
        if self.assumptions.is_empty() {
            return Err(invalid(
                "assumptions must bind at least the material and source".into(),
            ));
        }
        if !self.quantity.starts_with("component:") && self.quantity != "physical_total" {
            return Err(invalid(format!(
                "quantity must be component:<name> or physical_total, got {:?}",
                self.quantity
            )));
        }
        Ok(())
    }
}

fn axis_index(axis: &str) -> usize {
    match axis {
        "x" => 0,
        "y" => 1,
        _ => 2,
    }
}

/// Plane-averaged profile along `axis`: `(center_cm, mean, sigma_of_mean)`
/// per bin. Sigma is propagated under independence; absent per-voxel
/// uncertainties yield `None` and an unweighted fit.
fn axis_profile(
    bundle: &PhysicalDoseBundle,
    quantity: &str,
    axis: usize,
) -> Result<Vec<(f64, f64, Option<f64>)>, ManifestError> {
    let geometry = &bundle.geometry;
    let direction_identity = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
    if geometry
        .direction
        .iter()
        .zip(direction_identity)
        .any(|(a, b)| (a - b).abs() > 1e-9)
    {
        return Err(invalid(
            "analytic oracles require an axis-aligned (identity-direction) grid".into(),
        ));
    }

    let (values, sigmas) = if quantity == "physical_total" {
        (
            &bundle.physical_total.values,
            bundle
                .physical_total
                .absolute_standard_uncertainty
                .as_deref(),
        )
    } else {
        let name = quantity.strip_prefix("component:").unwrap_or(quantity);
        let component = bundle
            .components
            .iter()
            .find(|c| {
                serde_json::to_value(c.component)
                    .and_then(serde_json::from_value::<String>)
                    .is_ok_and(|s| s == name)
            })
            .ok_or_else(|| invalid(format!("bundle has no quantity {quantity:?}")))?;
        (
            &component.values,
            component.absolute_standard_uncertainty.as_deref(),
        )
    };

    let [nx, ny, nz] = geometry.shape.map(|d| d as usize);
    let extents = [nx, ny, nz];
    let plane = extents.iter().product::<usize>() / extents[axis];
    let spacing_cm = geometry.spacing_mm[axis] / 10.0;
    let origin_cm = geometry.origin_mm[axis] / 10.0;

    let mut profile = Vec::with_capacity(extents[axis]);
    for k in 0..extents[axis] {
        let mut sum = 0.0;
        let mut var = 0.0;
        let mut have_sigma = sigmas.is_some();
        for p in 0..plane {
            let mut c = [0usize; 3];
            c[axis] = k;
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            c[u] = p % extents[u];
            c[v] = p / extents[u];
            let idx = c[0] + nx * c[1] + nx * ny * c[2];
            sum += values[idx];
            match sigmas {
                Some(s) => var += s[idx] * s[idx],
                None => have_sigma = false,
            }
        }
        let mean = sum / plane as f64;
        let sigma = if have_sigma {
            Some(var.sqrt() / plane as f64)
        } else {
            None
        };
        profile.push((origin_cm + k as f64 * spacing_cm, mean, sigma));
    }
    Ok(profile)
}

/// Evaluate a dose bundle against a declared oracle. `oracle_ref` and
/// `dose_ref` content-bind the record to the exact artifacts consumed.
pub fn evaluate_analytic_oracle(
    id: &str,
    oracle: &AnalyticOracle,
    oracle_ref: ContentReference,
    bundle: &PhysicalDoseBundle,
    dose_ref: ContentReference,
) -> Result<AnalyticOracleEvaluation, ManifestError> {
    oracle.validate()?;
    bundle
        .validate()
        .map_err(|e| invalid(format!("dose bundle: {e}")))?;
    if oracle.case_id != bundle.case_id {
        return Err(invalid(format!(
            "case_id mismatch: {:?} vs {:?}",
            oracle.case_id, bundle.case_id
        )));
    }
    match oracle.law {
        AnalyticLaw::ExponentialAttenuation => {}
    }

    let axis = axis_index(&oracle.axis);
    let profile = axis_profile(bundle, &oracle.quantity, axis)?;
    let [lo, hi] = oracle.fit_window_cm;
    let samples: Vec<(f64, f64, Option<f64>)> = profile
        .into_iter()
        .filter(|(z, d, s)| *z >= lo && *z <= hi && *d > 0.0 && s.is_none_or(|v| v > 0.0))
        .collect();
    if samples.len() < oracle.min_bins as usize {
        return Err(invalid(format!(
            "only {} positive bins inside fit window, need {}",
            samples.len(),
            oracle.min_bins
        )));
    }

    // Weighted least squares of ln D vs z; weights 1/σ_lnD² with
    // σ_lnD = σ_D/D, unweighted when uncertainties are absent.
    let weighted = samples.iter().all(|(_, _, s)| s.is_some());
    let weight = |sigma: Option<f64>, dose: f64| -> f64 {
        match sigma {
            Some(s) if weighted => {
                let sl = s / dose;
                1.0 / (sl * sl)
            }
            _ => 1.0,
        }
    };
    let mut sw = 0.0;
    let mut swz = 0.0;
    let mut swl = 0.0;
    for (z, d, s) in &samples {
        let w = weight(*s, *d);
        sw += w;
        swz += w * z;
        swl += w * d.ln();
    }
    let zbar = swz / sw;
    let lbar = swl / sw;
    let mut szz = 0.0;
    let mut szl = 0.0;
    for (z, d, s) in &samples {
        let w = weight(*s, *d);
        let dz = z - zbar;
        szz += w * dz * dz;
        szl += w * dz * (d.ln() - lbar);
    }
    let slope = szl / szz;
    let slope_sigma = weighted.then(|| (1.0 / szz).sqrt());
    let intercept = lbar - slope * zbar;
    let mut resid_ss = 0.0;
    for (z, d, s) in &samples {
        let w = weight(*s, *d);
        let r = d.ln() - (intercept + slope * z);
        resid_ss += w * r * r;
    }
    let log_residual_rms = (resid_ss / sw).sqrt();

    let expected = -oracle.attenuation_per_cm;
    let relative_deviation = (slope - expected).abs() / expected.abs();
    let passed = relative_deviation <= oracle.relative_tolerance;

    Ok(AnalyticOracleEvaluation {
        schema_version: ANALYTIC_EVALUATION_SCHEMA.into(),
        id: id.into(),
        case_id: oracle.case_id.clone(),
        oracle: oracle_ref,
        inputs: vec![dose_ref],
        quantity: oracle.quantity.clone(),
        axis: oracle.axis.clone(),
        fit_window_cm: oracle.fit_window_cm,
        bins_evaluated: samples.len() as u64,
        fitted_slope_per_cm: slope,
        fitted_slope_sigma_per_cm: slope_sigma,
        expected_slope_per_cm: expected,
        relative_deviation,
        relative_tolerance: oracle.relative_tolerance,
        log_residual_rms: Some(log_residual_rms),
        passed,
        qualification: QUALIFICATION.into(),
        provenance_id: bundle.provenance_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::{
        DoseComponent, DoseUnit, GridGeometry, PhysicalTotalDoseVolume, TotalUncertaintyMethod,
    };

    /// Slab grid: 2×2 cm cross-section, eight 1 cm bins along z with
    /// centers at −3.5 … 3.5 cm.
    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [2, 2, 8],
            spacing_mm: [10.0, 10.0, 10.0],
            origin_mm: [-5.0, -5.0, -35.0],
            direction: [
                1.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, //
                0.0, 0.0, 1.0,
            ],
        }
    }

    fn bundle_with(values: Vec<f64>, sigma: Option<Vec<f64>>) -> PhysicalDoseBundle {
        PhysicalDoseBundle {
            schema_version: openbnct_core::PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "slab".into(),
            frame_of_reference_uid: None,
            geometry: geometry(),
            component_profile: ContentReference {
                id: "profile".into(),
                sha256: "0".repeat(64),
            },
            response_set: ContentReference {
                id: "responses".into(),
                sha256: "0".repeat(64),
            },
            provenance_id: "test".into(),
            components: [
                (DoseComponent::Boron, values.clone()),
                (DoseComponent::Nitrogen, vec![0.0; values.len()]),
                (DoseComponent::Hydrogen, vec![0.0; values.len()]),
                (DoseComponent::Photon, vec![0.0; values.len()]),
            ]
            .into_iter()
            .map(|(component, values)| openbnct_core::DoseVolume {
                component,
                unit: DoseUnit::GrayPerSourceParticle,
                values,
                absolute_standard_uncertainty: sigma.clone(),
            })
            .collect(),
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values,
                absolute_standard_uncertainty: sigma.clone(),
                uncertainty_method: if sigma.is_some() {
                    TotalUncertaintyMethod::DedicatedEstimator
                } else {
                    TotalUncertaintyMethod::Unavailable
                },
            },
        }
    }

    fn cref(name: &str) -> ContentReference {
        ContentReference {
            id: name.into(),
            sha256: "0".repeat(64),
        }
    }

    fn oracle() -> AnalyticOracle {
        AnalyticOracle {
            schema_version: ANALYTIC_ORACLE_SCHEMA.into(),
            id: "oracle".into(),
            case_id: "slab".into(),
            law: AnalyticLaw::ExponentialAttenuation,
            quantity: "component:boron".into(),
            axis: "z".into(),
            attenuation_per_cm: 0.23,
            relative_tolerance: 0.02,
            fit_window_cm: [-4.0, 4.0],
            min_bins: 3,
            assumptions: vec![cref("material"), cref("source")],
            note: None,
        }
    }

    /// Voxel order is x-fastest: index = i + 2j + 4k for shape [2,2,8].
    fn exponential_values(sigma_t: f64) -> Vec<f64> {
        let mut v = vec![0.0; 32];
        for k in 0..8usize {
            let z = -3.5 + k as f64; // cm
            let d = (-sigma_t * z).exp();
            for p in 0..4 {
                v[p + 4 * k] = d;
            }
        }
        v
    }

    #[test]
    fn exact_exponential_passes() {
        let bundle = bundle_with(exponential_values(0.23), None);
        let eval =
            evaluate_analytic_oracle("eval-1", &oracle(), cref("oracle"), &bundle, cref("dose"))
                .unwrap();
        assert!(eval.passed);
        assert_eq!(eval.bins_evaluated, 8);
        assert!((eval.fitted_slope_per_cm + 0.23).abs() < 1e-12);
        assert!(eval.relative_deviation < 1e-12);
        assert!(eval.fitted_slope_sigma_per_cm.is_none());
    }

    #[test]
    fn wrong_slope_fails() {
        // 20% steep — beyond the 2% tolerance.
        let bundle = bundle_with(exponential_values(0.276), None);
        let eval =
            evaluate_analytic_oracle("eval-2", &oracle(), cref("oracle"), &bundle, cref("dose"))
                .unwrap();
        assert!(!eval.passed);
        assert!((eval.relative_deviation - 0.2).abs() < 1e-9);
    }

    #[test]
    fn uncertainties_weight_the_fit() {
        let values = exponential_values(0.23);
        let sigma: Vec<f64> = values.iter().map(|v| v * 0.01).collect();
        let bundle = bundle_with(values, Some(sigma));
        let eval =
            evaluate_analytic_oracle("eval-3", &oracle(), cref("oracle"), &bundle, cref("dose"))
                .unwrap();
        assert!(eval.passed);
        assert!(eval.fitted_slope_sigma_per_cm.unwrap() > 0.0);
    }

    #[test]
    fn thin_window_and_case_mismatch_rejected() {
        let mut narrow = oracle();
        narrow.fit_window_cm = [-0.6, 0.6]; // covers only the two central bins
        let bundle = bundle_with(exponential_values(0.23), None);
        assert!(evaluate_analytic_oracle("e", &narrow, cref("o"), &bundle, cref("d")).is_err());

        let mut other_case = oracle();
        other_case.case_id = "other".into();
        assert!(evaluate_analytic_oracle("e", &other_case, cref("o"), &bundle, cref("d")).is_err());
    }

    #[test]
    fn oracle_validation() {
        assert!(oracle().validate().is_ok());
        for bad in [
            {
                let mut o = oracle();
                o.axis = "w".into();
                o
            },
            {
                let mut o = oracle();
                o.attenuation_per_cm = 0.0;
                o
            },
            {
                let mut o = oracle();
                o.fit_window_cm = [1.0, -1.0];
                o
            },
            {
                let mut o = oracle();
                o.assumptions.clear();
                o
            },
        ] {
            assert!(bad.validate().is_err());
        }
    }
}
