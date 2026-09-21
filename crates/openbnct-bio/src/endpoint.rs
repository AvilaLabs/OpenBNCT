// SPDX-License-Identifier: Apache-2.0

//! Endpoint response models: tumor-control and normal-tissue-complication
//! probability functions over a dose distribution, plus the combined UTCP
//! endpoint.
//!
//! An `EndpointModel` is a separately versioned research artifact naming a
//! response function, its parameters, the scalar dose statistic it consumes
//! (for volume-collapsed models), and a free-text validity domain. Applying
//! it to a dose volume over a region mask produces an
//! `EndpointEvaluation` carrying the probability and full provenance.
//!
//! These are research response functions with explicitly declared
//! parameters — they are not fitted clinical models and carry the
//! `synthetic_research_only_not_clinical` qualification.

use openbnct_core::{ContentReference, RegionMask, equivalent_uniform_dose, masked_values, mean};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::BioError;

pub const ENDPOINT_MODEL_SCHEMA: &str = "openbnct.endpoint-model/0.1.0";
pub const ENDPOINT_EVALUATION_SCHEMA: &str = "openbnct.endpoint-evaluation/0.1.0";

/// Which biological endpoint a model scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    Tcp,
    Ntcp,
}

/// The response function an endpoint model applies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EndpointFunction {
    /// Voxel-level Poisson tumor control under the linear-quadratic model.
    /// Each masked voxel's per-source-particle dose becomes a per-fraction
    /// dose `d = w · source_particles_per_fraction`, a BED
    /// `n·d·(1 + d/(α/β))`, and a surviving-clonogen expectation
    /// `ρ·v_voxel·exp(-α·BED)`; `TCP = exp(-Σ_i survivors)`.
    VoxelPoissonTcp {
        /// Clonogen density per mm³ of region volume.
        clonogen_density_per_mm3: f64,
        /// Radiosensitivity α in inverse dose units.
        alpha: f64,
        /// α/β ratio in the endpoint's dose unit.
        alpha_beta: f64,
        /// Number of identical fractions.
        fraction_count: u32,
        /// Source particles delivered per fraction.
        source_particles_per_fraction: f64,
    },
    /// Logistic sigmoid `1 / (1 + (D50/D)^(4·γ50))` over a scalar dose
    /// statistic; usable for TCP or NTCP depending on `endpoint`.
    Logistic {
        /// Dose giving a 50 % response, in the dose statistic's unit.
        d50: f64,
        /// Normalized slope γ50 (dimensionless, positive).
        gamma50: f64,
    },
    /// Lyman probit `Φ((D − TD50)/(m·TD50))` over a scalar dose statistic.
    Probit {
        /// Tolerance dose for 50 % complication probability.
        td50: f64,
        /// Dimensionless slope parameter (positive).
        m: f64,
    },
}

/// The scalar reduction of a masked dose volume a response function reads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "statistic", rename_all = "snake_case")]
pub enum DoseStatistic {
    Mean,
    Min,
    Max,
    /// Niemierko generalized EUD with organ parameter `a`.
    Eud {
        a: f64,
    },
}

impl DoseStatistic {
    pub fn evaluate(&self, selected: &[f64]) -> Result<f64, BioError> {
        match *self {
            Self::Mean => Ok(mean(selected)),
            Self::Min => Ok(selected.iter().copied().fold(f64::INFINITY, f64::min)),
            Self::Max => Ok(selected.iter().copied().fold(0.0, f64::max)),
            Self::Eud { a } => equivalent_uniform_dose(selected, a)
                .map_err(|e| BioError::Invalid(format!("dose statistic: {e}"))),
        }
    }

    /// Stable wire label for the applied-statistic record.
    pub fn label(&self) -> (String, Option<f64>) {
        match *self {
            Self::Mean => ("mean".into(), None),
            Self::Min => ("min".into(), None),
            Self::Max => ("max".into(), None),
            Self::Eud { a } => ("eud".into(), Some(a)),
        }
    }
}

/// A separately versioned endpoint response-model artifact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointModel {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub endpoint: EndpointKind,
    pub function: EndpointFunction,
    /// Scalar dose reduction for volume-collapsed functions
    /// (`logistic`, `probit`); must be absent for `voxel_poisson_tcp`,
    /// which reads the per-voxel distribution directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dose_statistic: Option<DoseStatistic>,
    /// Free-text description of the parameter validity domain; recorded
    /// for provenance, not enforced.
    pub validity_domain: Option<String>,
    /// Evidence reference for where the parameter values came from.
    pub derivation: Option<ContentReference>,
}

impl EndpointModel {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, ENDPOINT_MODEL_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.id.trim().is_empty() {
            return Err(BioError::Invalid("model id is empty".into()));
        }
        let needs_statistic = matches!(
            self.function,
            EndpointFunction::Logistic { .. } | EndpointFunction::Probit { .. }
        );
        if needs_statistic != self.dose_statistic.is_some() {
            return Err(BioError::Invalid(
                "logistic and probit functions require dose_statistic; voxel_poisson_tcp forbids it"
                    .into(),
            ));
        }
        match self.function {
            EndpointFunction::VoxelPoissonTcp {
                clonogen_density_per_mm3,
                alpha,
                alpha_beta,
                fraction_count,
                source_particles_per_fraction,
            } => {
                if self.endpoint != EndpointKind::Tcp {
                    return Err(BioError::Invalid(
                        "voxel_poisson_tcp is a TCP function".into(),
                    ));
                }
                let valid = [
                    clonogen_density_per_mm3,
                    alpha,
                    alpha_beta,
                    source_particles_per_fraction,
                ]
                .iter()
                .all(|value| value.is_finite() && *value > 0.0);
                if !valid || fraction_count == 0 {
                    return Err(BioError::Invalid(
                        "voxel_poisson_tcp requires positive clonogen density, alpha, alpha/beta, fraction count, and particles per fraction".into(),
                    ));
                }
            }
            EndpointFunction::Logistic { d50, gamma50 } => {
                if ![d50, gamma50]
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0)
                {
                    return Err(BioError::Invalid(
                        "logistic response requires positive d50 and gamma50".into(),
                    ));
                }
            }
            EndpointFunction::Probit { td50, m } => {
                if ![td50, m]
                    .iter()
                    .all(|value| value.is_finite() && *value > 0.0)
                {
                    return Err(BioError::Invalid(
                        "probit response requires positive td50 and m".into(),
                    ));
                }
            }
        }
        if let Some(derivation) = &self.derivation {
            derivation
                .validate()
                .map_err(|_| BioError::Invalid("derivation reference is invalid".into()))?;
        }
        Ok(())
    }
}

/// The dose statistic a volume-collapsed function actually consumed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedDoseStatistic {
    /// `mean`, `min`, `max`, or `eud`.
    pub kind: String,
    /// The EUD organ parameter, when `kind` is `eud`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter: Option<f64>,
    /// The evaluated statistic value, in `unit`.
    pub value: f64,
    /// Unit label copied verbatim from the source dose volume.
    pub unit: String,
}

/// How a UTCP evaluation combined its TCP and NTCP inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UtcpCombination {
    /// Brahme's `P+`: `TCP · (1 − NTCP)`.
    PPlus,
    /// Plain difference `TCP − NTCP` (can be negative).
    Difference,
}

/// The TCP and NTCP ingredients of a combined UTCP evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UtcpComponents {
    pub combination: UtcpCombination,
    pub tcp: f64,
    /// Effective complication probability across the OAR inputs:
    /// `1 − Π(1 − NTCPᵢ)` under independence (equals the single NTCP
    /// when one evaluation was supplied).
    pub ntcp: f64,
    /// Region the TCP was evaluated on — the target.
    #[serde(default)]
    pub tcp_region: String,
    /// Regions the NTCP inputs were evaluated on — the organs at risk.
    /// The standard P₊ pairs a tumor TCP with *different* OAR regions.
    #[serde(default)]
    pub ntcp_regions: Vec<String>,
    /// Per-OAR NTCP inputs in declaration order.
    #[serde(default)]
    pub ntcp_terms: Vec<f64>,
    /// Content bindings of the two source evaluations.
    pub tcp_evaluation: ContentReference,
    pub ntcp_evaluation: ContentReference,
    /// Bindings of every NTCP input (multi-OAR combinations).
    #[serde(default)]
    pub ntcp_evaluations: Vec<ContentReference>,
}

/// The endpoint an evaluation scored — `utcp` marks a combination record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluatedEndpoint {
    Tcp,
    Ntcp,
    Utcp,
}

/// The scored result of applying an endpoint model to a dose volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointEvaluation {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// The endpoint this record scored.
    pub endpoint: EvaluatedEndpoint,
    /// Name of the region mask the dose distribution was taken over.
    pub region: String,
    /// Which dose volume was scored, e.g. `physical_total` or
    /// `biological_total`.
    pub quantity: String,
    /// Content binding of the dose artifact.
    pub dose_source: ContentReference,
    /// The evaluated scalar statistic for volume-collapsed functions;
    /// absent for `voxel_poisson_tcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dose_statistic: Option<AppliedDoseStatistic>,
    /// Content binding of the exact model JSON applied; absent for a UTCP
    /// combination, which carries its ingredients in `utcp` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ContentReference>,
    /// Response probability in `[0, 1]` for tcp/ntcp; may be negative for a
    /// `difference` UTCP — the combination is reported honestly.
    pub probability: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utcp: Option<UtcpComponents>,
    pub qualification: String,
}

impl EndpointEvaluation {
    pub fn validate(&self) -> Result<(), BioError> {
        if !openbnct_core::schema_matches(&self.schema_version, ENDPOINT_EVALUATION_SCHEMA) {
            return Err(BioError::UnsupportedSchema(self.schema_version.clone()));
        }
        if self.case_id.trim().is_empty() || self.region.trim().is_empty() {
            return Err(BioError::Invalid("case_id or region is empty".into()));
        }
        self.dose_source
            .validate()
            .map_err(|_| BioError::Invalid("dose source reference is invalid".into()))?;
        if let Some(model) = &self.model {
            model
                .validate()
                .map_err(|_| BioError::Invalid("model reference is invalid".into()))?;
        }
        if !self.probability.is_finite() {
            return Err(BioError::Invalid("probability is not finite".into()));
        }
        if let Some(utcp) = &self.utcp {
            if self.endpoint != EvaluatedEndpoint::Utcp {
                return Err(BioError::Invalid(
                    "a UTCP component block requires endpoint \"utcp\"".into(),
                ));
            }
            utcp.tcp_evaluation
                .validate()
                .and_then(|_| utcp.ntcp_evaluation.validate())
                .map_err(|_| BioError::Invalid("UTCP component reference is invalid".into()))?;
        }
        if self.endpoint == EvaluatedEndpoint::Utcp && self.utcp.is_none() {
            return Err(BioError::Invalid(
                "endpoint \"utcp\" requires a UTCP component block".into(),
            ));
        }
        Ok(())
    }
}

/// Standard normal CDF via the Abramowitz–Stegun 7.1.26 error-function
/// approximation (absolute error ≤ 1.5e-7).
fn normal_cdf(x: f64) -> f64 {
    const P: f64 = 0.3275911;
    const A1: f64 = 0.254829592;
    const A2: f64 = -0.284496736;
    const A3: f64 = 1.421413741;
    const A4: f64 = -1.453152027;
    const A5: f64 = 1.061405429;
    // Φ(x) = ½(1 + erf(x/√2)); the Abramowitz–Stegun approximation below
    // evaluates erf, so the argument must be scaled by 1/√2 first.
    let z = x / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + P * z.abs());
    let erf_abs = 1.0 - (((((A5 * t + A4) * t + A3) * t + A2) * t + A1) * t) * (-z * z).exp();
    let erf = if z >= 0.0 { erf_abs } else { -erf_abs };
    0.5 * (1.0 + erf)
}

/// Score `model` over the dose selection `values` restricted to `mask`.
///
/// `voxel_poisson_tcp` requires a `*_per_source_particle` unit so the
/// per-fraction dose conversion is defined; logistic/probit accept any
/// dose unit, which is copied verbatim into the statistic record.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_endpoint(
    model: &EndpointModel,
    model_bytes: &[u8],
    case_id: &str,
    mask: &RegionMask,
    quantity: &str,
    unit: &str,
    values: &[f64],
    voxel_volume_mm3: f64,
    dose_source: ContentReference,
) -> Result<EndpointEvaluation, BioError> {
    model.validate()?;
    dose_source
        .validate()
        .map_err(|_| BioError::Invalid("dose source reference is invalid".into()))?;
    if !voxel_volume_mm3.is_finite() || voxel_volume_mm3 <= 0.0 {
        return Err(BioError::Invalid("voxel volume must be positive".into()));
    }
    let selected = masked_values(&mask.name, values, &mask.voxels)
        .map_err(|e| BioError::Invalid(format!("dose selection: {e}")))?;

    let (probability, applied_statistic) = match &model.function {
        EndpointFunction::VoxelPoissonTcp {
            clonogen_density_per_mm3,
            alpha,
            alpha_beta,
            fraction_count,
            source_particles_per_fraction,
        } => {
            if !unit.ends_with("per_source_particle") {
                return Err(BioError::Invalid(format!(
                    "voxel_poisson_tcp needs a per-source-particle dose unit, observed {unit:?}"
                )));
            }
            let n = f64::from(*fraction_count);
            let p = *source_particles_per_fraction;
            let clonogens_per_voxel = clonogen_density_per_mm3 * voxel_volume_mm3;
            let survivors: f64 = selected
                .iter()
                .map(|w| {
                    let d = w * p;
                    let bed = n * d * (1.0 + d / alpha_beta);
                    clonogens_per_voxel * (-alpha * bed).exp()
                })
                .sum();
            // exp(-survivors) underflows to 0 for large survivor counts —
            // that is the correct limiting TCP.
            ((-survivors).exp(), None)
        }
        function => {
            let statistic = model
                .dose_statistic
                .expect("validated volume-collapsed functions carry a dose_statistic");
            let dose = statistic.evaluate(&selected)?;
            let probability = match *function {
                EndpointFunction::Logistic { d50, gamma50 } => {
                    if dose == 0.0 {
                        0.0
                    } else {
                        1.0 / (1.0 + (d50 / dose).powf(4.0 * gamma50))
                    }
                }
                EndpointFunction::Probit { td50, m } => normal_cdf((dose - td50) / (m * td50)),
                EndpointFunction::VoxelPoissonTcp { .. } => unreachable!(),
            };
            let (kind, parameter) = statistic.label();
            (
                probability.clamp(0.0, 1.0),
                Some(AppliedDoseStatistic {
                    kind,
                    parameter,
                    value: dose,
                    unit: unit.into(),
                }),
            )
        }
    };

    let evaluation = EndpointEvaluation {
        schema_version: ENDPOINT_EVALUATION_SCHEMA.into(),
        case_id: case_id.into(),
        endpoint: match model.endpoint {
            EndpointKind::Tcp => EvaluatedEndpoint::Tcp,
            EndpointKind::Ntcp => EvaluatedEndpoint::Ntcp,
        },
        region: mask.name.clone(),
        quantity: quantity.into(),
        dose_source,
        dose_statistic: applied_statistic,
        model: Some(ContentReference {
            id: model.id.clone(),
            sha256: format!("{:x}", Sha256::digest(model_bytes)),
        }),
        probability,
        utcp: None,
        qualification: "synthetic_research_only_not_clinical".into(),
    };
    evaluation.validate()?;
    Ok(evaluation)
}

/// Combine a TCP and an NTCP evaluation into a UTCP report.
///
/// Both evaluations must describe the same case, region, dose quantity,
/// and dose source — otherwise the combination would silently mix
/// different distributions.
/// Single-OAR convenience wrapper over [`combine_utcp_multi`].
pub fn combine_utcp(
    tcp: &EndpointEvaluation,
    tcp_bytes: &[u8],
    ntcp: &EndpointEvaluation,
    ntcp_bytes: &[u8],
    combination: UtcpCombination,
) -> Result<EndpointEvaluation, BioError> {
    combine_utcp_multi(tcp, tcp_bytes, &[(ntcp, ntcp_bytes)], combination)
}

/// Combine one TCP evaluation with one or more NTCP evaluations —
/// P₊ = TCP · Π(1 − NTCPᵢ) under independence, the standard
/// tumor-vs-organs-at-risk pairing. TCP and NTCP may be evaluated on
/// *different* regions (they normally are); the regions are recorded,
/// not required to agree. Case, quantity, and dose source must agree.
pub fn combine_utcp_multi(
    tcp: &EndpointEvaluation,
    tcp_bytes: &[u8],
    ntcps: &[(&EndpointEvaluation, &[u8])],
    combination: UtcpCombination,
) -> Result<EndpointEvaluation, BioError> {
    tcp.validate()?;
    if tcp.endpoint != EvaluatedEndpoint::Tcp {
        return Err(BioError::Invalid(
            "UTCP needs a primary tcp evaluation".into(),
        ));
    }
    if tcp.model.is_none() {
        return Err(BioError::Invalid(
            "UTCP inputs must carry their model bindings".into(),
        ));
    }
    if ntcps.is_empty() {
        return Err(BioError::Invalid(
            "UTCP needs at least one ntcp evaluation".into(),
        ));
    }
    for (ntcp, _) in ntcps {
        ntcp.validate()?;
        if ntcp.endpoint != EvaluatedEndpoint::Ntcp {
            return Err(BioError::Invalid(
                "UTCP needs a primary ntcp evaluation".into(),
            ));
        }
        if ntcp.model.is_none() {
            return Err(BioError::Invalid(
                "UTCP inputs must carry their model bindings".into(),
            ));
        }
        for (label, left, right) in [
            ("case_id", tcp.case_id.as_str(), ntcp.case_id.as_str()),
            ("quantity", tcp.quantity.as_str(), ntcp.quantity.as_str()),
            (
                "dose_source.sha256",
                tcp.dose_source.sha256.as_str(),
                ntcp.dose_source.sha256.as_str(),
            ),
        ] {
            if left != right {
                return Err(BioError::Invalid(format!(
                    "UTCP inputs disagree on {label}: {left:?} vs {right:?}"
                )));
            }
        }
    }
    // Π(1 − NTCPᵢ): independence of OAR complications.
    let oar_free: f64 = ntcps.iter().map(|(n, _)| 1.0 - n.probability).product();
    let effective_ntcp = 1.0 - oar_free;
    let probability = match combination {
        UtcpCombination::PPlus => tcp.probability * oar_free,
        UtcpCombination::Difference => tcp.probability - effective_ntcp,
    };
    let evaluation = EndpointEvaluation {
        schema_version: ENDPOINT_EVALUATION_SCHEMA.into(),
        case_id: tcp.case_id.clone(),
        endpoint: EvaluatedEndpoint::Utcp,
        region: tcp.region.clone(),
        quantity: tcp.quantity.clone(),
        dose_source: tcp.dose_source.clone(),
        dose_statistic: None,
        model: None,
        probability,
        utcp: Some(UtcpComponents {
            combination,
            tcp: tcp.probability,
            ntcp: effective_ntcp,
            tcp_region: tcp.region.clone(),
            ntcp_regions: ntcps.iter().map(|(n, _)| n.region.clone()).collect(),
            ntcp_terms: ntcps.iter().map(|(n, _)| n.probability).collect(),
            tcp_evaluation: ContentReference {
                id: format!("{}.{}", tcp.case_id, "endpoint-evaluation"),
                sha256: format!("{:x}", Sha256::digest(tcp_bytes)),
            },
            ntcp_evaluation: ContentReference {
                id: format!("{}.{}", ntcps[0].0.case_id, "endpoint-evaluation"),
                sha256: format!("{:x}", Sha256::digest(ntcps[0].1)),
            },
            ntcp_evaluations: ntcps
                .iter()
                .map(|(n, bytes)| ContentReference {
                    id: format!("{}.{}", n.case_id, "endpoint-evaluation"),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                })
                .collect(),
        }),
        qualification: "synthetic_research_only_not_clinical".into(),
    };
    evaluation.validate()?;
    Ok(evaluation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> ContentReference {
        ContentReference {
            id: "test-dose".into(),
            sha256: "c".repeat(64),
        }
    }

    fn mask(name: &str, voxels: &[bool]) -> RegionMask {
        RegionMask {
            name: name.into(),
            voxels: voxels.to_vec(),
        }
    }

    fn model(function: EndpointFunction) -> EndpointModel {
        let needs_statistic = !matches!(function, EndpointFunction::VoxelPoissonTcp { .. });
        EndpointModel {
            schema_version: ENDPOINT_MODEL_SCHEMA.into(),
            id: "openbnct.tests.endpoint.v1".into(),
            endpoint: EndpointKind::Tcp,
            function,
            dose_statistic: needs_statistic.then_some(DoseStatistic::Mean),
            validity_domain: Some("synthetic test parameters".into()),
            derivation: None,
        }
    }

    #[test]
    fn logistic_tcp_hits_fifty_percent_at_d50() {
        let model = model(EndpointFunction::Logistic {
            d50: 60.0,
            gamma50: 2.0,
        });
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let evaluation = evaluate_endpoint(
            &model,
            &bytes,
            "case",
            &mask("tumor", &[true, true]),
            "physical_total",
            "gray",
            &[60.0, 60.0],
            125.0,
            source(),
        )
        .unwrap();
        assert!((evaluation.probability - 0.5).abs() < 1e-12);
        let statistic = evaluation.dose_statistic.unwrap();
        assert_eq!(statistic.kind, "mean");
        assert_eq!(statistic.value, 60.0);
        // Zero dose gives zero control, not a 0/0 form.
        let zero = evaluate_endpoint(
            &model,
            &bytes,
            "case",
            &mask("tumor", &[true]),
            "physical_total",
            "gray",
            &[0.0],
            1.0,
            source(),
        )
        .unwrap();
        assert_eq!(zero.probability, 0.0);
    }

    #[test]
    fn probit_ntcp_hits_fifty_percent_at_td50() {
        let mut model = model(EndpointFunction::Probit { td50: 50.0, m: 0.3 });
        model.endpoint = EndpointKind::Ntcp;
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let evaluation = evaluate_endpoint(
            &model,
            &bytes,
            "case",
            &mask("organ", &[true, true]),
            "biological_total",
            "weighted_eqd2",
            &[50.0, 50.0],
            125.0,
            source(),
        )
        .unwrap();
        // Φ(0) = 0.5 within the approximation bound.
        assert!((evaluation.probability - 0.5).abs() < 2e-7);

        // One standard deviation above TD50: dose = td50 + m·td50 gives
        // z = 1, Φ(1) = 0.8413447.
        let evaluation = evaluate_endpoint(
            &model,
            &bytes,
            "case",
            &mask("organ", &[true, true]),
            "biological_total",
            "weighted_eqd2",
            &[65.0, 65.0],
            125.0,
            source(),
        )
        .unwrap();
        assert!((evaluation.probability - 0.8413447).abs() < 2e-7);
    }

    #[test]
    fn voxel_poisson_tcp_counts_surviving_clonogens() {
        // Uniform dose: two voxels at w = 2.0 gray/particle, spp = 1.0,
        // n = 1, a/b = 10, alpha = 0.3, rho = 1/mm3, voxel = 1 mm3.
        // BED = 2·(1+0.2) = 2.4; survivors per voxel = exp(-0.72);
        // TCP = exp(-2·exp(-0.72)).
        let model = model(EndpointFunction::VoxelPoissonTcp {
            clonogen_density_per_mm3: 1.0,
            alpha: 0.3,
            alpha_beta: 10.0,
            fraction_count: 1,
            source_particles_per_fraction: 1.0,
        });
        let bytes = serde_json::to_vec_pretty(&model).unwrap();
        let evaluation = evaluate_endpoint(
            &model,
            &bytes,
            "case",
            &mask("tumor", &[true, true]),
            "physical_total",
            "gray_per_source_particle",
            &[2.0, 2.0],
            1.0,
            source(),
        )
        .unwrap();
        let expected = (-2.0_f64 * (-0.72_f64).exp()).exp();
        assert!((evaluation.probability - expected).abs() < 1e-12);
        assert!(evaluation.dose_statistic.is_none());
        // Absolute dose units are rejected: per-fraction dose is undefined.
        assert!(
            evaluate_endpoint(
                &model,
                &bytes,
                "case",
                &mask("tumor", &[true]),
                "physical_total",
                "gray",
                &[2.0],
                1.0,
                source(),
            )
            .is_err()
        );
    }

    #[test]
    fn endpoint_models_validate_their_domains() {
        // Logistic without a statistic is rejected.
        let mut bad = model(EndpointFunction::Logistic {
            d50: 60.0,
            gamma50: 2.0,
        });
        bad.dose_statistic = None;
        assert!(bad.validate().is_err());
        // Statistic on a voxel function is rejected.
        let mut bad = model(EndpointFunction::VoxelPoissonTcp {
            clonogen_density_per_mm3: 1.0,
            alpha: 0.3,
            alpha_beta: 10.0,
            fraction_count: 1,
            source_particles_per_fraction: 1.0,
        });
        bad.dose_statistic = Some(DoseStatistic::Mean);
        assert!(bad.validate().is_err());
        // Voxel Poisson declared as NTCP is rejected.
        let mut bad = model(EndpointFunction::VoxelPoissonTcp {
            clonogen_density_per_mm3: 1.0,
            alpha: 0.3,
            alpha_beta: 10.0,
            fraction_count: 1,
            source_particles_per_fraction: 1.0,
        });
        bad.endpoint = EndpointKind::Ntcp;
        assert!(bad.validate().is_err());
        // Non-positive parameters are rejected.
        let bad = model(EndpointFunction::Logistic {
            d50: -1.0,
            gamma50: 2.0,
        });
        assert!(bad.validate().is_err());
    }

    #[test]
    fn utcp_combines_matching_evaluations() {
        let tcp_model = model(EndpointFunction::Logistic {
            d50: 10.0,
            gamma50: 2.0,
        });
        let tcp_bytes = serde_json::to_vec_pretty(&tcp_model).unwrap();
        let mut ntcp_model = model(EndpointFunction::Probit { td50: 1.0, m: 0.1 });
        ntcp_model.endpoint = EndpointKind::Ntcp;
        let ntcp_bytes = serde_json::to_vec_pretty(&ntcp_model).unwrap();

        let tcp = evaluate_endpoint(
            &tcp_model,
            &tcp_bytes,
            "case",
            &mask("tumor", &[true]),
            "physical_total",
            "gray",
            &[20.0],
            1.0,
            source(),
        )
        .unwrap();
        let ntcp = evaluate_endpoint(
            &ntcp_model,
            &ntcp_bytes,
            "case",
            &mask("tumor", &[true]),
            "physical_total",
            "gray",
            &[20.0],
            1.0,
            source(),
        )
        .unwrap();
        let combined =
            combine_utcp(&tcp, &tcp_bytes, &ntcp, &ntcp_bytes, UtcpCombination::PPlus).unwrap();
        let expected = tcp.probability * (1.0 - ntcp.probability);
        assert!((combined.probability - expected).abs() < 1e-12);
        let utcp = combined.utcp.unwrap();
        assert_eq!(utcp.tcp, tcp.probability);
        assert_eq!(utcp.ntcp, ntcp.probability);

        // Cross-region pairing — tumor TCP × cord NTCP — is the standard
        // P₊ use case and must succeed; both regions are recorded.
        let other_region = evaluate_endpoint(
            &ntcp_model,
            &ntcp_bytes,
            "case",
            &mask("cord", &[true]),
            "physical_total",
            "gray",
            &[20.0],
            1.0,
            source(),
        )
        .unwrap();
        let cross = combine_utcp(
            &tcp,
            &tcp_bytes,
            &other_region,
            &ntcp_bytes,
            UtcpCombination::PPlus,
        )
        .unwrap();
        let utcp = cross.utcp.unwrap();
        assert_eq!(utcp.tcp_region, "tumor");
        assert_eq!(utcp.ntcp_regions, ["cord"]);

        // Multi-OAR product: P₊ = TCP · Π(1 − NTCPᵢ).
        let cord2 = evaluate_endpoint(
            &ntcp_model,
            &ntcp_bytes,
            "case",
            &mask("cord2", &[true]),
            "physical_total",
            "gray",
            &[20.0],
            1.0,
            source(),
        )
        .unwrap();
        let multi = combine_utcp_multi(
            &tcp,
            &tcp_bytes,
            &[(&other_region, &ntcp_bytes), (&cord2, &ntcp_bytes)],
            UtcpCombination::PPlus,
        )
        .unwrap();
        let expected =
            tcp.probability * (1.0 - other_region.probability) * (1.0 - cord2.probability);
        assert!((multi.probability - expected).abs() < 1e-12);
        assert_eq!(multi.utcp.as_ref().unwrap().ntcp_regions, ["cord", "cord2"]);
    }
}
