// SPDX-License-Identifier: MIT

//! PHITS input-deck export — the emit half of the PHITS adapter.
//!
//! Emits a runnable PHITS 3.x deck from a validated [`TransportCase`]:
//! the scoring grid becomes one `RPP` cell, the on-face `UniformDisk`
//! becomes an `s-type = 1` circular-plane source (`z0 = z1`), and a
//! `[t-track]` xyz-mesh tally scores the track-length fluence the
//! [`crate::interchange_from_phits`] importer can read back — closing
//! the emit→run→reimport loop for cross-code verification.
//!
//! Deliberately out of scope (refused with explicit errors, never
//! approximated):
//!
//! - `MaterialAssignment` / multi-region geometries — PHITS lattice
//!   syntax for per-voxel materials is a separate contract;
//! - non-Z-axis disks and non-plane spatial distributions — the
//!   cylindrical source is z-aligned (a `trcl` rotation for X/Y faces
//!   is a later increment);
//! - `TabulatedHistogram` spectra — `e0` monoenergetic only until the
//!   `e-type` data-block encoding is reviewed;
//! - component-dose folding — same rule as the MCNP emitter: the deck
//!   scores flux; response folding is the external pipeline's step.

use std::fmt::Write as _;

use openbnct_transport::{
    AngularDistribution, EnergyDistribution, ParticleType, SourceSpatialDistribution,
    TransportCase, TransportModelError,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PhitsDeckError {
    #[error("invalid transport case: {0}")]
    InvalidCase(#[from] TransportModelError),
    #[error(
        "unsupported feature for PHITS emission: {0} — emit subset is Z-axis disk, monoenergetic, single-material"
    )]
    UnsupportedFeature(String),
    #[error("nuclide name {0:?} cannot be mapped to a PHITS ZAAA code")]
    UnknownNuclide(String),
}

/// Operator-facing export choices, recorded in the deck header.
#[derive(Debug, Clone, Default)]
pub struct PhitsDeckOptions {
    /// `sha256:<hex>` of the source case document.
    pub case_sha256: String,
    /// Histories per batch (`maxbch` splits `requested_histories`).
    pub maxbch: u32,
}

/// Emit a complete PHITS input deck for `case`. Deterministic.
pub fn export_phits_deck(
    case: &TransportCase,
    options: &PhitsDeckOptions,
) -> Result<String, PhitsDeckError> {
    case.validate()?;
    let geometry = &case.geometry;
    let unsupported = |m: &str| PhitsDeckError::UnsupportedFeature(m.into());

    // --- source: Z-axis circular plane only ---
    let SourceSpatialDistribution::UniformDisk {
        axis,
        offset_cm,
        center_uv_cm,
        radius_cm,
    } = &case.source.space
    else {
        return Err(unsupported("source space (only uniform_disk is supported)"));
    };
    if *axis != openbnct_transport::PlaneAxis::Z {
        return Err(unsupported(
            "disk on a non-Z face (needs a trcl rotation — deferred)",
        ));
    }
    let EnergyDistribution::Monoenergetic { energy_ev } = case.source.energy else {
        return Err(unsupported(
            "source energy (only monoenergetic is supported; spectra need an e-type review)",
        ));
    };
    let (dir, phi_deg, dom_deg) = match &case.source.angle {
        AngularDistribution::Monodirectional { unit_vector } => (unit_vector[2], 0.0_f64, 0.0_f64),
        AngularDistribution::IsotropicCone {
            axis_unit_vector,
            half_angle_rad,
        } => (
            axis_unit_vector[2],
            axis_unit_vector[1].atan2(axis_unit_vector[0]).to_degrees(),
            half_angle_rad.to_degrees(),
        ),
    };
    if dir.abs() > 1.0 + 1e-9 {
        return Err(unsupported("source direction outside the unit sphere"));
    }
    let proj = match case.source.particle {
        ParticleType::Neutron => "neutron",
        ParticleType::Photon => "photon",
    };

    // --- geometry: grid RPP in cm ---
    let [nx, ny, nz] = geometry.shape;
    let lo = geometry.origin_mm.map(|v| v / 10.0);
    let hi = [
        lo[0] + nx as f64 * geometry.spacing_mm[0] / 10.0,
        lo[1] + ny as f64 * geometry.spacing_mm[1] / 10.0,
        lo[2] + nz as f64 * geometry.spacing_mm[2] / 10.0,
    ];

    let mut out = String::new();
    let _ = writeln!(out, "[ T i t l e ]");
    let _ = writeln!(out, "  openbnct case {}", case.case_id);
    if !options.case_sha256.is_empty() {
        let _ = writeln!(out, "  # case {}", options.case_sha256);
    }
    let _ = writeln!(out, "  # emitted by openbnct-phits; research use only");

    let _ = writeln!(out, "\n[ P a r a m e t e r s ]");
    let _ = writeln!(out, "  icntl = 0");
    let maxbch = options.maxbch.max(1);
    let maxcas = (case.requested_histories / maxbch as u64).max(1);
    let _ = writeln!(out, "  maxcas = {maxcas}   $ histories per batch");
    let _ = writeln!(out, "  maxbch = {maxbch}");
    let _ = writeln!(
        out,
        "  file(6) = phits.out   $ xs path via PHITS data settings"
    );

    let _ = writeln!(out, "\n[ S o u r c e ]");
    let _ = writeln!(
        out,
        "  s-type = 1        $ cylinder; z1 = z0 = circular plane"
    );
    let _ = writeln!(out, "  proj   = {proj}");
    let _ = writeln!(out, "  e0     = {:.6e}   $ MeV", energy_ev / 1.0e6);
    let _ = writeln!(out, "  x0 = {:.6}", center_uv_cm[0]);
    let _ = writeln!(out, "  y0 = {:.6}", center_uv_cm[1]);
    let _ = writeln!(out, "  z0 = {offset_cm:.6}");
    let _ = writeln!(out, "  z1 = {offset_cm:.6}");
    let _ = writeln!(out, "  r0 = {radius_cm:.6}");
    let _ = writeln!(out, "  dir = {dir:.9}");
    let _ = writeln!(out, "  phi = {phi_deg:.6}");
    if dom_deg > 0.0 {
        let _ = writeln!(out, "  dom = {dom_deg:.6}  $ cone half-angle, degrees");
    }

    let _ = writeln!(out, "\n[ M a t e r i a l ]");
    let _ = writeln!(out, "  M1");
    for nuclide in &case.material.nuclides {
        let zaaa = zaaa(&nuclide.name)
            .ok_or_else(|| PhitsDeckError::UnknownNuclide(nuclide.name.clone()))?;
        // PHITS takes positive atomic-density ratios or negative mass
        // fractions — emit negative mass fractions like the MCNP deck.
        let _ = writeln!(out, "    {zaaa}  {}", -nuclide.mass_fraction);
    }

    let _ = writeln!(out, "\n[ S u r f a c e ]");
    let _ = writeln!(
        out,
        "  1  RPP  {:.6} {:.6}  {:.6} {:.6}  {:.6} {:.6}",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );

    let _ = writeln!(out, "\n[ C e l l ]");
    let _ = writeln!(
        out,
        "  101  1  -{:.6}  -1    $ grid filled with material 1",
        case.material.density_g_cm3
    );
    let _ = writeln!(out, "  999  0          1     $ void outside");

    let _ = writeln!(out, "\n[ T - T r a c k ]");
    let _ = writeln!(out, "  mesh = xyz");
    let _ = writeln!(
        out,
        "  x-type = 2    nx = {nx}    xmin = {:.6}  xmax = {:.6}",
        lo[0], hi[0]
    );
    let _ = writeln!(
        out,
        "  y-type = 2    ny = {ny}    ymin = {:.6}  ymax = {:.6}",
        lo[1], hi[1]
    );
    let _ = writeln!(
        out,
        "  z-type = 2    nz = {nz}    zmin = {:.6}  zmax = {:.6}",
        lo[2], hi[2]
    );
    let _ = writeln!(out, "  part = {proj}");
    let _ = writeln!(out, "  axis = xy");
    let _ = writeln!(out, "  unit = 1        $ 1/cm^2/source");
    let _ = writeln!(out, "  file = track.out");

    let _ = writeln!(out, "\n[ E n d ]");
    Ok(out)
}

/// `ElA[_mN]` (e.g. `H1`, `B10`) → PHITS ZAAA integer (`Z*1000 + A`).
/// Metastable names have no PHITS ZAAA convention — refused.
fn zaaa(name: &str) -> Option<u64> {
    if name.contains("_m") {
        return None;
    }
    let bytes = name.as_bytes();
    let mut index = 1;
    if bytes.get(index).is_some_and(|b| b.is_ascii_lowercase()) {
        index += 1;
    }
    let element = &name[..index];
    let z = ELEMENTS.iter().position(|e| *e == element)? as u64 + 1;
    let mass: u64 = name[index..].parse().ok()?;
    Some(z * 1000 + mass)
}

const ELEMENTS: [&str; 118] = [
    "H", "He", "Li", "Be", "B", "C", "N", "O", "F", "Ne", "Na", "Mg", "Al", "Si", "P", "S", "Cl",
    "Ar", "K", "Ca", "Sc", "Ti", "V", "Cr", "Mn", "Fe", "Co", "Ni", "Cu", "Zn", "Ga", "Ge", "As",
    "Se", "Br", "Kr", "Rb", "Sr", "Y", "Zr", "Nb", "Mo", "Tc", "Ru", "Rh", "Pd", "Ag", "Cd", "In",
    "Sn", "Sb", "Te", "I", "Xe", "Cs", "Ba", "La", "Ce", "Pr", "Nd", "Pm", "Sm", "Eu", "Gd", "Tb",
    "Dy", "Ho", "Er", "Tm", "Yb", "Lu", "Hf", "Ta", "W", "Re", "Os", "Ir", "Pt", "Au", "Hg", "Tl",
    "Pb", "Bi", "Po", "At", "Rn", "Fr", "Ra", "Ac", "Th", "Pa", "U", "Np", "Pu", "Am", "Cm", "Bk",
    "Cf", "Es", "Fm", "Md", "No", "Lr", "Rf", "Db", "Sg", "Bh", "Hs", "Mt", "Ds", "Rg", "Cn", "Nh",
    "Fl", "Mc", "Lv", "Ts", "Og",
];

#[cfg(test)]
mod tests {
    use super::*;
    use openbnct_core::GridGeometry;
    use openbnct_transport::{FixedSourceDefinition, MaterialDefinition, NuclideMassFraction};

    fn case() -> TransportCase {
        TransportCase {
            schema_version: "openbnct.transport-case/0.1.0".into(),
            case_id: "t".into(),
            geometry: GridGeometry {
                shape: [4, 4, 4],
                spacing_mm: [5.0, 5.0, 5.0],
                origin_mm: [-10.0, -10.0, -10.0],
                direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            },
            material: MaterialDefinition {
                schema_version: "openbnct.material-definition/0.1.0".into(),
                id: "water".into(),
                density_g_cm3: 1.0,
                temperature_k: 300.0,
                nuclides: vec![
                    NuclideMassFraction {
                        name: "H1".into(),
                        mass_fraction: 0.112,
                    },
                    NuclideMassFraction {
                        name: "O16".into(),
                        mass_fraction: 0.888,
                    },
                ],
                neutron_thermal_treatment: openbnct_transport::NeutronThermalTreatment::FreeGas,
                boron_microdistribution: None,
            },
            source: FixedSourceDefinition {
                schema_version: "openbnct.fixed-source/0.1.0".into(),
                id: "beam".into(),
                particle: ParticleType::Neutron,
                statistical_weight_per_site: 1.0,
                source_sites_per_history: 1,
                space: SourceSpatialDistribution::UniformDisk {
                    axis: openbnct_transport::PlaneAxis::Z,
                    offset_cm: -1.0,
                    center_uv_cm: [0.0, 0.0],
                    radius_cm: 2.0,
                },
                angle: AngularDistribution::Monodirectional {
                    unit_vector: [0.0, 0.0, 1.0],
                },
                energy: EnergyDistribution::Monoenergetic { energy_ev: 1.0e6 },
            },
            requested_histories: 10_000,
        }
    }

    #[test]
    fn emits_runnable_skeleton() {
        let deck = export_phits_deck(
            &case(),
            &PhitsDeckOptions {
                case_sha256: "sha256:x".into(),
                maxbch: 10,
            },
        )
        .unwrap();
        for section in [
            "[ T i t l e ]",
            "[ P a r a m e t e r s ]",
            "[ S o u r c e ]",
            "[ M a t e r i a l ]",
            "[ S u r f a c e ]",
            "[ C e l l ]",
            "[ T - T r a c k ]",
            "[ E n d ]",
        ] {
            assert!(deck.contains(section), "missing {section}");
        }
        assert!(deck.contains("s-type = 1"));
        assert!(deck.contains("r0 = 2"));
        assert!(deck.contains("1001  -0.112"));
        assert!(deck.contains("8016  -0.888"));
        assert!(deck.contains("e0     = 1.000000e0"));
    }

    #[test]
    fn refuses_x_axis_disk() {
        let mut c = case();
        c.source.space = SourceSpatialDistribution::UniformDisk {
            axis: openbnct_transport::PlaneAxis::X,
            offset_cm: -1.0,
            center_uv_cm: [0.0, 0.0],
            radius_cm: 2.0,
        };
        assert!(matches!(
            export_phits_deck(&c, &PhitsDeckOptions::default()),
            Err(PhitsDeckError::UnsupportedFeature(_))
        ));
    }

    #[test]
    fn refuses_spectrum_source() {
        let mut c = case();
        c.source.energy = EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: vec![1.0, 1e6],
            bin_weights: vec![1.0],
        };
        assert!(matches!(
            export_phits_deck(&c, &PhitsDeckOptions::default()),
            Err(PhitsDeckError::UnsupportedFeature(_))
        ));
    }
}
