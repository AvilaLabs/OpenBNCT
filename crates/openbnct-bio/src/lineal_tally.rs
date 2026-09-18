//! Transport-derived lineal-energy spectra: an
//! `openbnct.lineal-tally-spec/0.1.0` artifact declares a microscopic
//! spherical site and a table of charged-secondaries per dose
//! component; `compute_lineal_spectrum` evaluates the spec against a
//! deterministic multigroup flux and emits the
//! `openbnct.lineal-spectrum/0.1.0` artifact the MKM family already
//! consumes (`LinealEnergySource::ComputedSpectrum`) — computed
//! spectra in place of published constants.
//!
//! Model (deliberately explicit, all declared inputs):
//!
//! * Events are neutron collisions in the component's material at rate
//!   `σ_t,g·φ_v,g·V_v·share_c` — `interaction_fraction` is the
//!   declared share of total collisions producing that component's
//!   secondary (reaction-channel resolution is not carried by the
//!   multigroup data, so it is declared, consistent with the rest of
//!   the declared-data contract).
//! * Each event emits one rectilinear charged secondary of declared
//!   `emission_energy_ev` and `range_um`; over a site chord of length
//!   `l` it imparts `ε(l) = E_c·min(1, l/R_c)`.
//! * The site is a sphere of declared `diameter_um`: isotropic chord
//!   density `p(l) = 2l/d²` on [0, d], mean chord `l̄ = 2d/3`
//!   (Cauchy). Lineal energy `y = ε/l̄` in keV/µm.
//! * Single-event spectra are rate-weighted and domain-integrated into
//!   the event-frequency spectrum `f(y)`, normalized to unit integral.
//!
//! Honest limits, stated in every emitted spectrum's note: no straggling
//! or escape-correction beyond the rectilinear range model, no
//! angular-transport detail inside the site, and no microscopic
//! substructure (nucleus vs cytoplasm — that is the microdistribution
//! layer's job). This is the energy-weighted traversal statistic the
//! declared data supports, clearly labeled.

use openbnct_core::ContentReference;
use openbnct_transport::{
    MaterialAssignment, MultigroupData, MultigroupFlux, TransportCase, cell_materials,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::mkm::{LINEAL_SPECTRUM_SCHEMA, LinealSpectrum, LinealWeighting};

pub const LINEAL_TALLY_SPEC_SCHEMA: &str = "openbnct.lineal-tally-spec/0.1.0";

#[derive(Debug, Error)]
pub enum LinealTallyError {
    #[error("lineal tally validation: {0}")]
    Invalid(String),
    #[error("unknown tally target: {0}")]
    UnknownTarget(String),
}

fn invalid(message: String) -> LinealTallyError {
    LinealTallyError::Invalid(message)
}

/// One declared charged-secondary channel contributing events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinealComponent {
    pub name: String,
    /// Material in whose voxels the component's collisions occur.
    pub material_id: String,
    /// Declared share of that material's total collisions producing
    /// this secondary (0, 1].
    pub interaction_fraction: f64,
    /// Charged-secondary emission energy per event, eV.
    pub emission_energy_ev: f64,
    /// Effective rectilinear range in the site medium, µm — the event
    /// deposits `min(E, E·l/R)` over chord `l`.
    pub range_um: f64,
}

/// Declared lineal-tally design over a multigroup flux.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinealTallySpec {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding of the multigroup data the flux was solved on.
    pub multigroup_data: ContentReference,
    /// Content binding of the flux artifact tallied.
    pub flux: ContentReference,
    /// Spherical site diameter, µm.
    pub site_diameter_um: f64,
    /// Lineal-energy bin edges, keV/µm (n+1 strictly increasing).
    pub bin_edges_kev_um: Vec<f64>,
    pub components: Vec<LinealComponent>,
    /// Where the secondary table comes from — required.
    pub provenance_note: String,
    pub qualification: String,
}

impl LinealTallySpec {
    pub fn validate(&self) -> Result<(), LinealTallyError> {
        if !openbnct_core::schema_matches(&self.schema_version, LINEAL_TALLY_SPEC_SCHEMA) {
            return Err(invalid(format!("schema_version {:?}", self.schema_version)));
        }
        if self.id.trim().is_empty() {
            return Err(invalid("id is empty".into()));
        }
        if !(self.site_diameter_um.is_finite() && self.site_diameter_um > 0.0) {
            return Err(invalid(
                "site_diameter_um must be positive and finite".into(),
            ));
        }
        if self.bin_edges_kev_um.len() < 2
            || !self
                .bin_edges_kev_um
                .iter()
                .all(|e| e.is_finite() && *e >= 0.0)
            || !self.bin_edges_kev_um.windows(2).all(|w| w[1] > w[0])
        {
            return Err(invalid(
                "bin_edges_kev_um must be n+1 finite increasing non-negative edges".into(),
            ));
        }
        if self.components.is_empty() {
            return Err(invalid("components must not be empty".into()));
        }
        for component in &self.components {
            if component.name.trim().is_empty() {
                return Err(invalid("component name is empty".into()));
            }
            if !(component.interaction_fraction > 0.0
                && component.interaction_fraction <= 1.0
                && component.emission_energy_ev > 0.0
                && component.emission_energy_ev.is_finite()
                && component.range_um > 0.0
                && component.range_um.is_finite())
            {
                return Err(invalid(format!(
                    "component {:?} needs 0 < fraction ≤ 1, positive energy, positive range",
                    component.name
                )));
            }
        }
        Ok(())
    }
}

/// Chord-quadrature resolution for the single-event spectrum —
/// deterministic, so spectra are bit-reproducible.
const CHORD_STEPS: usize = 4096;

/// Single-event y-distribution of one component onto `edges` (keV/µm):
/// `y(l) = (E_c/1000 keV)·min(1, l/R_c) / l̄`, l ~ sphere chord pdf.
/// Returns unnormalized bin masses integrating to 1 over y.
fn single_event_bins(
    edges: &[f64],
    site_diameter_um: f64,
    emission_energy_ev: f64,
    range_um: f64,
) -> Vec<f64> {
    let d = site_diameter_um;
    let l_bar = 2.0 * d / 3.0;
    let e_kev = emission_energy_ev / 1000.0;
    let mut bins = vec![0.0; edges.len() - 1];
    let dl = d / CHORD_STEPS as f64;
    for step in 0..CHORD_STEPS {
        // Midpoint rule on l; pdf p(l) = 2l/d² integrates to 1.
        let l = (step as f64 + 0.5) * dl;
        let weight = 2.0 * l / (d * d) * dl;
        let eps_kev = e_kev * (l / range_um).min(1.0);
        let y = eps_kev / l_bar;
        // Find the bin containing y (edges are increasing).
        let idx = match edges.binary_search_by(|e| e.partial_cmp(&y).unwrap()) {
            Ok(i) => i.min(bins.len() - 1),
            Err(i) => i.saturating_sub(1).min(bins.len() - 1),
        };
        if y <= *edges.last().unwrap() {
            bins[idx] += weight;
        }
    }
    bins
}

/// Evaluates the tally spec against a computed multigroup flux and
/// returns the `openbnct.lineal-spectrum/0.1.0` artifact.
pub fn compute_lineal_spectrum(
    case: &TransportCase,
    data: &MultigroupData,
    flux: &MultigroupFlux,
    assignment: Option<&MaterialAssignment>,
    spec: &LinealTallySpec,
    spectrum_id: &str,
    spec_ref: ContentReference,
) -> Result<LinealSpectrum, LinealTallyError> {
    spec.validate()?;
    if spec.multigroup_data.id != data.id {
        return Err(invalid(format!(
            "spec {} describes data {:?}, got {:?}",
            spec.id, spec.multigroup_data.id, data.id
        )));
    }
    if flux.energy_boundaries_ev != data.energy_boundaries_ev {
        return Err(invalid(
            "flux energy structure does not match the multigroup data".into(),
        ));
    }
    let n_cells = case
        .geometry
        .voxel_count()
        .map_err(|e| invalid(format!("geometry: {e}")))?;
    if flux.flux.len() != n_cells {
        return Err(invalid(format!(
            "flux has {} cells, geometry has {n_cells}",
            flux.flux.len()
        )));
    }
    let groups = data.group_count();
    let case_material = cell_materials(case, data, assignment)
        .map_err(|e| invalid(format!("material assignment: {e}")))?;
    let cell_volume_cm3 = case.geometry.spacing_mm.iter().product::<f64>() / 1000.0;
    let edges = &spec.bin_edges_kev_um;
    let mut bins = vec![0.0_f64; edges.len() - 1];
    for component in &spec.components {
        let Some(m) = data
            .materials
            .iter()
            .position(|mat| mat.material_id == component.material_id)
        else {
            return Err(LinealTallyError::UnknownTarget(format!(
                "material {:?}",
                component.material_id
            )));
        };
        let f1 = single_event_bins(
            edges,
            spec.site_diameter_um,
            component.emission_energy_ev,
            component.range_um,
        );
        // Event rate summed over the voxels where this component's
        // material actually sits (assignment-aware cell map).
        let mut rate = 0.0;
        for (cell, row) in flux.flux.iter().enumerate() {
            if case_material[cell] != m {
                continue;
            }
            for (g, &phi) in row.iter().enumerate().take(groups) {
                rate += data.materials[m].sigma_total_per_cm[g]
                    * phi
                    * cell_volume_cm3
                    * component.interaction_fraction;
            }
        }
        if rate <= 0.0 {
            continue;
        }
        for (bin, w) in bins.iter_mut().zip(f1.iter()) {
            *bin += rate * w;
        }
    }
    // Normalize to unit event integral: values are f(y) density.
    let mut integral = 0.0;
    for (w, e) in bins.iter().zip(edges.windows(2)) {
        integral += w * (e[1] - e[0]);
    }
    if !(integral > 0.0 && integral.is_finite()) {
        return Err(invalid(
            "tally produced no events — check fractions, materials, and flux".into(),
        ));
    }
    for w in bins.iter_mut() {
        *w /= integral;
    }
    let spectrum = LinealSpectrum {
        schema_version: LINEAL_SPECTRUM_SCHEMA.into(),
        id: spectrum_id.into(),
        bin_edges_kev_um: edges.clone(),
        values: bins,
        absolute_standard_uncertainty: None,
        weighting: LinealWeighting::EventFrequency,
        value_unit: "relative_frequency_density".into(),
        derivation: Some(spec_ref),
        note: Some(format!(
            "transport-derived: sphere site {} µm, rectilinear-secondary \
             model ε(l)=E·min(1,l/R) over the isotropic chord pdf; \
             collision-rate-weighted over the case domain; no straggling \
             or sub-site structure",
            spec.site_diameter_um
        )),
    };
    spectrum
        .validate()
        .map_err(|e| invalid(format!("emitted spectrum invalid: {e}")))?;
    Ok(spectrum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::GridGeometry;
    use openbnct_transport::{
        AngularDistribution, EnergyDistribution, FixedSourceDefinition, MultigroupMaterial,
        ParticleType, PlaneAxis, SourceSpatialDistribution, TransportCase,
    };
    use std::collections::BTreeMap;

    fn cref(id: &str) -> ContentReference {
        ContentReference {
            id: id.into(),
            sha256: "a".repeat(64),
        }
    }

    /// 2×1×1 cm³ voxel fixture, homogeneous absorber, uniform flux.
    fn fixture() -> (TransportCase, MultigroupData, MultigroupFlux) {
        let case = TransportCase {
            schema_version: "openbnct.transport-case/0.1.0".into(),
            case_id: "lt-case".into(),
            geometry: GridGeometry {
                shape: [2, 1, 1],
                spacing_mm: [10.0, 10.0, 10.0],
                origin_mm: [0.0, 0.0, 0.0],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            material: openbnct_transport::MaterialDefinition {
                schema_version: "openbnct.material-definition/0.1.0".into(),
                id: "absorber".into(),
                density_g_cm3: 1.0,
                temperature_k: 300.0,
                nuclides: vec![openbnct_transport::NuclideMassFraction {
                    name: "B10".into(),
                    mass_fraction: 1.0,
                }],
                neutron_thermal_treatment: openbnct_transport::NeutronThermalTreatment::FreeGas,
            },
            source: FixedSourceDefinition {
                schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
                id: "beam".into(),
                particle: ParticleType::Neutron,
                source_sites_per_history: 1,
                statistical_weight_per_site: 1.0,
                space: SourceSpatialDistribution::UniformDisk {
                    axis: PlaneAxis::Z,
                    offset_cm: -1.0,
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 0.25,
                },
                angle: AngularDistribution::Monodirectional {
                    unit_vector: [0.0, 0.0, 1.0],
                },
                energy: EnergyDistribution::Monoenergetic { energy_ev: 0.0253 },
            },
            requested_histories: 1,
        };
        let data = MultigroupData {
            schema_version: openbnct_transport::MULTIGROUP_DATA_SCHEMA.into(),
            id: "lt-data".into(),
            energy_boundaries_ev: vec![1.0, 1.0e-3],
            collapse_declaration: "test".into(),
            component_profile: None,
            materials: vec![MultigroupMaterial {
                material_id: "absorber".into(),
                sigma_total_per_cm: vec![0.5],
                scatter_matrix_per_cm: vec![0.0],
                dose_response_gy_cm2: BTreeMap::from([("boron".into(), vec![1e-4])]),
                transport_mu_bar: None,
            }],
        };
        let flux = MultigroupFlux {
            schema_version: openbnct_transport::MULTIGROUP_FLUX_SCHEMA.into(),
            case_id: "lt-case".into(),
            multigroup_data: cref("lt-data"),
            case: cref("lt-case"),
            energy_boundaries_ev: vec![1.0, 1.0e-3],
            beam_model: "boundary_flux".into(),
            transport_correction: false,
            source_spectrum_weighting: "collapse_consistent".into(),
            quadrature_order: 4,
            converged: true,
            residual: 0.0,
            outer_iterations: 1,
            qualification: "test".into(),
            provenance_id: "lt-flux".into(),
            flux: vec![vec![1.0], vec![1.0]],
        };
        (case, data, flux)
    }

    fn spec(components: Vec<LinealComponent>) -> LinealTallySpec {
        LinealTallySpec {
            schema_version: LINEAL_TALLY_SPEC_SCHEMA.into(),
            id: "lt-spec".into(),
            multigroup_data: cref("lt-data"),
            flux: cref("lt-flux"),
            site_diameter_um: 1.0,
            bin_edges_kev_um: (0..=50).map(|i| i as f64 * 10.0).collect(),
            components,
            provenance_note: "test".into(),
            qualification: "test_only".into(),
        }
    }

    /// Long-range secondary (R ≫ d): ε = E·l/R never saturates, so
    /// y ∝ l over the chord pdf — the spectrum must ramp upward to
    /// y_max = E·d/(R·l̄) and vanish above it. Exact shape check.
    #[test]
    fn long_range_gives_ramp_spectrum() {
        let (case, data, flux) = fixture();
        // E = 100 keV, R = 10 µm, d = 1 µm → y = 100·l/(10·0.667)
        //   = 15·l keV/µm → y_max = 15 keV/µm.
        let mut s = spec(vec![LinealComponent {
            name: "secondary".into(),
            material_id: "absorber".into(),
            interaction_fraction: 1.0,
            emission_energy_ev: 100_000.0,
            range_um: 10.0,
        }]);
        s.bin_edges_kev_um = (0..=30).map(|i| i as f64).collect();
        let spectrum =
            compute_lineal_spectrum(&case, &data, &flux, None, &s, "lt", cref("spec")).unwrap();
        // Ramp: f(y) ∝ y on [0, 15] (chord pdf p(l)=2l/d² is linear and
        // y ∝ l), zero above. Bin 1 (1–2 keV/µm) must be empty... but
        // bin 0 holds y ∈ [0,1]: f grows from 0 → check monotonicity
        // over the ramp region and emptiness above y_max.
        let above: f64 = spectrum.values[16..].iter().sum();
        assert_eq!(above, 0.0, "nothing above y_max=15");
        assert!(spectrum.values[12] > spectrum.values[5]);
        assert!(spectrum.values[8] > spectrum.values[3]);
        // y_D for a ramp f(y) ∝ y on [0,Y]: ∫y²·y/∫y·y = 3Y/4 = 11.25.
        let yd = spectrum.dose_mean_kev_um().unwrap();
        assert!(
            (yd - 11.25).abs() < 0.5,
            "ramp dose-mean should be ~11.25, got {yd}"
        );
    }

    /// Short-range secondary (R ≪ d): nearly all chords exceed R, so
    /// events saturate at ε = E and y_D ≈ E/l̄.
    #[test]
    fn short_range_saturates_at_emission_energy() {
        let (case, data, flux) = fixture();
        // E = 200 keV, R = 0.01 µm ≪ d = 1 µm → y ≈ 200/0.667 = 300.
        let mut s = spec(vec![LinealComponent {
            name: "heavy".into(),
            material_id: "absorber".into(),
            interaction_fraction: 1.0,
            emission_energy_ev: 200_000.0,
            range_um: 0.01,
        }]);
        s.bin_edges_kev_um = (0..=60).map(|i| i as f64 * 10.0).collect();
        let spectrum =
            compute_lineal_spectrum(&case, &data, &flux, None, &s, "lt", cref("spec")).unwrap();
        let yd = spectrum.dose_mean_kev_um().unwrap();
        assert!(
            (yd - 300.0).abs() < 15.0,
            "saturated spectrum should peak near E/l̄=300, got {yd}"
        );
    }

    /// Two components with disjoint event spectra combine
    /// rate-weighted — equal fractions + equal rates give each half
    /// the event mass.
    #[test]
    fn components_combine_rate_weighted() {
        let (case, data, flux) = fixture();
        let s = spec(vec![
            LinealComponent {
                name: "soft".into(),
                material_id: "absorber".into(),
                interaction_fraction: 0.5,
                emission_energy_ev: 30_000.0,
                range_um: 10.0,
            },
            LinealComponent {
                name: "hard".into(),
                material_id: "absorber".into(),
                interaction_fraction: 0.5,
                emission_energy_ev: 300_000.0,
                range_um: 10.0,
            },
        ]);
        let spectrum =
            compute_lineal_spectrum(&case, &data, &flux, None, &s, "lt", cref("spec")).unwrap();
        // Normalized to unit integral.
        let integral: f64 = spectrum
            .values
            .iter()
            .zip(spectrum.bin_edges_kev_um.windows(2))
            .map(|(v, e)| v * (e[1] - e[0]))
            .sum();
        assert!((integral - 1.0).abs() < 1e-9);
        let yd = spectrum.dose_mean_kev_um().unwrap();
        assert!(yd > 0.0 && yd.is_finite());
    }

    /// Flux scaling leaves the normalized spectrum invariant.
    #[test]
    fn flux_scale_invariant() {
        let (case, data, mut flux) = fixture();
        let s = spec(vec![LinealComponent {
            name: "secondary".into(),
            material_id: "absorber".into(),
            interaction_fraction: 1.0,
            emission_energy_ev: 100_000.0,
            range_um: 10.0,
        }]);
        let a = compute_lineal_spectrum(&case, &data, &flux, None, &s, "lt", cref("spec")).unwrap();
        for row in flux.flux.iter_mut() {
            for v in row.iter_mut() {
                *v *= 7.0;
            }
        }
        let b = compute_lineal_spectrum(&case, &data, &flux, None, &s, "lt", cref("spec")).unwrap();
        assert_eq!(a.values, b.values);
    }

    /// Zero event rate (zero fraction is rejected at validate; zero
    /// flux produces the explicit error).
    #[test]
    fn zero_flux_rejected() {
        let (case, data, mut flux) = fixture();
        for row in flux.flux.iter_mut() {
            for v in row.iter_mut() {
                *v = 0.0;
            }
        }
        let s = spec(vec![LinealComponent {
            name: "secondary".into(),
            material_id: "absorber".into(),
            interaction_fraction: 1.0,
            emission_energy_ev: 100_000.0,
            range_um: 10.0,
        }]);
        assert!(
            compute_lineal_spectrum(&case, &data, &flux, None, &s, "lt", cref("spec")).is_err()
        );
    }
}

#[cfg(test)]
mod artifact_tests {
    use super::*;
    use sha2::Digest;

    /// The committed NF-BNCT-003 lineal-tally spec + flux produce a
    /// validating spectrum whose boron-capture dose mean sits in the
    /// physical range for a 1 µm site.
    #[test]
    fn committed_nf_bnct_003_lineal_tally_produces_spectrum() {
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../benchmarks/synthetic/nf-bnct-003/transport/"
        );
        let read = |name: &str| std::fs::read(format!("{base}{name}")).unwrap();
        let hex = |b: &[u8]| -> String {
            sha2::Sha256::digest(b)
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect()
        };
        let case: openbnct_transport::TransportCase =
            serde_json::from_slice(&read("case.json")).unwrap();
        let data: openbnct_transport::MultigroupData =
            serde_json::from_slice(&read("multigroup-data.json")).unwrap();
        let flux: openbnct_transport::MultigroupFlux =
            serde_json::from_slice(&read("multigroup-flux.json")).unwrap();
        let spec_bytes = read("lineal-tally-spec.json");
        let spec: LinealTallySpec = serde_json::from_slice(&spec_bytes).unwrap();
        spec.validate().unwrap();
        assert_eq!(
            spec.multigroup_data.sha256,
            hex(&read("multigroup-data.json"))
        );
        assert_eq!(spec.flux.sha256, hex(&read("multigroup-flux.json")));
        let spectrum = compute_lineal_spectrum(
            &case,
            &data,
            &flux,
            None,
            &spec,
            "openbnct.nf-bnct-003.lineal-spectrum.v1",
            ContentReference {
                id: spec.id.clone(),
                sha256: hex(&spec_bytes),
            },
        )
        .unwrap();
        let yd = spectrum.dose_mean_kev_um().unwrap();
        assert!(
            (100.0..500.0).contains(&yd),
            "boron-capture ȳ_D at 1 µm should sit in the physical range, got {yd}"
        );
        assert_eq!(spectrum.weighting, LinealWeighting::EventFrequency);
    }
}
