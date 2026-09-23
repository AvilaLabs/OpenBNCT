// SPDX-License-Identifier: MIT

//! PHITS input-deck export — the emit half of the PHITS adapter.
//!
//! Emits a runnable PHITS 3.x deck from a validated [`TransportCase`] plus
//! an optional [`MaterialAssignment`]: the scoring grid becomes the
//! `[t-track]` xyz mesh and one `RPP` cell; `voxel_box` regions carve out
//! `RPP` cells of their own, and any `voxel_set`/`voxel_fractions` region
//! forces the whole grid into a `LAT=1` lattice whose `FILL` list assigns
//! a per-material universe to every voxel — the same semantics the MCNP
//! and OpenMC emitters realize.
//!
//! Source mapping:
//!
//! - `UniformDisk` on a Z face → `s-type = 1` cylindrical plane
//!   (`z1 = z0`, `r0` radius);
//! - `UniformAxisPlane`/`UniformCartesianPlane` → `s-type = 2`
//!   rectangular plane (the perpendicular axis emits `x0 = x1` etc.);
//! - `Monodirectional`/`IsotropicCone` → `dir` (cosθ vs +z), `phi`
//!   (azimuth, degrees), `dom` (cone half-angle, degrees); a π cone is
//!   `dir = all`;
//! - `Monoenergetic` → `e0` (MeV); `TabulatedHistogram` → `e-type = 1`
//!   integral distribution (`ne` bin count, `e(i) w(i)` pairs, then
//!   `e(ne+1)`).
//!
//! Deliberately out of scope (refused with explicit errors, never
//! approximated):
//!
//! - non-Z-axis disks — the cylindrical source is z-aligned; an X/Y face
//!   disk needs a `trcl` rotation, a later increment;
//! - component-dose folding — same rule as the MCNP emitter: the deck
//!   scores flux; response folding is the external pipeline's step.
//!
//! Format conventions (PHITS User's Manual 3.37): units are centimetres;
//! mass fractions emit as *negative* `[material]` ratios; metastable
//! nuclides (`Am242_m1`) map to `Z*1000 + A + 50` (`95292`); cell
//! complement is `#`; `FILL` lists universe numbers with `i` fastest,
//! matching the bundle order `i + nx·j + nx·ny·k`; the outer-void cell
//! carries material `-1`.

use std::fmt::Write as _;

use openbnct_transport::{
    AngularDistribution, EnergyDistribution, MaterialAssignment, MaterialDefinition, ParticleType,
    PlaneAxis, SourceSpatialDistribution, TransportCase, TransportModelError,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PhitsDeckError {
    #[error("invalid transport case: {0}")]
    InvalidCase(#[from] TransportModelError),
    #[error("material assignment case_id {assignment} does not match case {case}")]
    AssignmentCaseMismatch { assignment: String, case: String },
    #[error("assignment base material differs from the case material")]
    AssignmentBaseMismatch,
    #[error("unsupported feature for PHITS emission: {0}")]
    UnsupportedFeature(String),
    #[error("nuclide name {0:?} cannot be mapped to a PHITS ZAAA code")]
    UnknownNuclide(String),
}

/// Operator-facing export choices, recorded in the deck header.
#[derive(Debug, Clone, Default)]
pub struct PhitsDeckOptions {
    /// `sha256:<hex>` of the source case document.
    pub case_sha256: String,
    /// Batch count (`maxcas` is derived from `requested_histories`).
    pub maxbch: u32,
}

/// Grid-box surface number; region boxes occupy `101 + index`, the
/// lattice element-0 box is `9001`.
const GRID_SURFACE: u32 = 1;
const REGION_SURFACE_BASE: u32 = 101;
const ELEMENT_SURFACE: u32 = 9001;

/// Element universe numbers start here (cell numbers reuse the same
/// base, keeping card numbers self-describing).
const UNIVERSE_BASE: u32 = 1000;

/// Emit a complete PHITS input deck for `case` (+ optional `assignment`).
/// Deterministic: identical inputs byte-for-byte reproduce it.
pub fn export_phits_deck(
    case: &TransportCase,
    assignment: Option<&MaterialAssignment>,
    options: &PhitsDeckOptions,
) -> Result<String, PhitsDeckError> {
    case.validate()?;
    if let Some(assignment) = assignment {
        if assignment.case_id != case.case_id {
            return Err(PhitsDeckError::AssignmentCaseMismatch {
                assignment: assignment.case_id.clone(),
                case: case.case_id.clone(),
            });
        }
        if assignment.base_material != case.material {
            return Err(PhitsDeckError::AssignmentBaseMismatch);
        }
        assignment.validate(&case.geometry)?;
    }
    let unsupported = |m: &str| PhitsDeckError::UnsupportedFeature(m.into());
    let geometry = &case.geometry;

    // --- source spatial: disk (Z face) or rectangular plane ---
    let mut source = String::new();
    match &case.source.space {
        SourceSpatialDistribution::UniformDisk {
            axis,
            offset_cm,
            center_uv_cm,
            radius_cm,
        } => {
            if *axis != PlaneAxis::Z {
                return Err(unsupported(
                    "disk on a non-Z face (needs a trcl rotation — deferred)",
                ));
            }
            let _ = writeln!(
                source,
                "  s-type = 1        $ cylinder; z1 = z0 = circular plane"
            );
            let _ = writeln!(source, "  x0 = {:.6}", center_uv_cm[0]);
            let _ = writeln!(source, "  y0 = {:.6}", center_uv_cm[1]);
            let _ = writeln!(source, "  z0 = {offset_cm:.6}");
            let _ = writeln!(source, "  z1 = {offset_cm:.6}");
            let _ = writeln!(source, "  r0 = {radius_cm:.6}");
        }
        space => {
            let Some((axis, offset_cm, u_range, v_range)) = space.plane_parts() else {
                return Err(unsupported("source space (disk or rectangular plane only)"));
            };
            let (u_axis, v_axis) = axis.in_plane_axes();
            let mut bounds = [[0.0; 2]; 3];
            bounds[u_axis] = u_range;
            bounds[v_axis] = v_range;
            bounds[axis.index()] = [offset_cm, offset_cm];
            let _ = writeln!(source, "  s-type = 2        $ rectangular; a0 = a1 = plane");
            for (axis, bound) in bounds.iter().enumerate() {
                let letter = ['x', 'y', 'z'][axis];
                let _ = writeln!(source, "  {letter}0 = {:.6}", bound[0]);
                let _ = writeln!(source, "  {letter}1 = {:.6}", bound[1]);
            }
        }
    }

    // --- source direction: dir = cosθ vs +z, phi azimuth deg, dom deg ---
    let (unit, dom_deg) = match &case.source.angle {
        AngularDistribution::Monodirectional { unit_vector } => (*unit_vector, 0.0),
        AngularDistribution::IsotropicCone {
            axis_unit_vector,
            half_angle_rad,
        } => (*axis_unit_vector, half_angle_rad.to_degrees()),
    };
    let dir = unit[2];
    if dir.abs() > 1.0 + 1e-9 {
        return Err(unsupported("source direction outside the unit sphere"));
    }
    // phi matters even for a pencil: dir alone fixes only the polar angle.
    let phi_deg = unit[1].atan2(unit[0]).to_degrees();
    if dom_deg >= 180.0 - 1e-9 {
        let _ = writeln!(source, "  dir = all         $ isotropic");
    } else {
        let _ = writeln!(source, "  dir = {dir:.9}");
        let _ = writeln!(source, "  phi = {phi_deg:.6}");
        if dom_deg > 0.0 {
            let _ = writeln!(source, "  dom = {dom_deg:.6}  $ cone half-angle, degrees");
        }
    }

    // --- source energy ---
    match &case.source.energy {
        EnergyDistribution::Monoenergetic { energy_ev } => {
            let _ = writeln!(source, "  e0     = {:.6e}   $ MeV", energy_ev / 1.0e6);
        }
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev,
            bin_weights,
        } => {
            let _ = writeln!(
                source,
                "  e-type = 1        $ integral histogram: e(i) w(i), then e(ne+1)"
            );
            let _ = writeln!(source, "    ne = {}", bin_weights.len());
            for (edge_ev, weight) in energy_boundaries_ev.iter().zip(bin_weights.iter()) {
                let _ = writeln!(source, "      {:.6e}  {:.6e}", edge_ev / 1.0e6, weight);
            }
            let _ = writeln!(
                source,
                "      {:.6e}",
                energy_boundaries_ev[bin_weights.len()] / 1.0e6
            );
        }
    }
    let proj = match case.source.particle {
        ParticleType::Neutron => "neutron",
        ParticleType::Photon => "photon",
    };

    // --- geometry: grid RPP in cm (voxel-center origin → corner edges) ---
    let [nx, ny, nz] = geometry.shape;
    let mut edges = [[0.0; 2]; 3];
    for (axis, edge) in edges.iter_mut().enumerate() {
        let center0 = geometry.origin_mm[axis] / 10.0;
        let pitch = geometry.spacing_mm[axis] / 10.0;
        *edge = [
            center0 - 0.5 * pitch,
            center0 + (geometry.shape[axis] as f64 - 0.5) * pitch,
        ];
    }
    let rpp = |surface: u32, e: &[[f64; 2]; 3]| {
        format!(
            "  {surface}  RPP  {:.6} {:.6}  {:.6} {:.6}  {:.6} {:.6}",
            e[0][0], e[0][1], e[1][0], e[1][1], e[2][0], e[2][1]
        )
    };

    // Materials in emission order: mat 1 is the base; identical region
    // definitions dedupe onto one MAT card (same as the MCNP emitter).
    let mut materials: Vec<&MaterialDefinition> = vec![&case.material];
    let mut region_materials: Vec<usize> = Vec::new();
    if let Some(assignment) = assignment {
        for region in &assignment.regions {
            let index = materials
                .iter()
                .position(|m| **m == region.material)
                .unwrap_or_else(|| {
                    materials.push(&region.material);
                    materials.len() - 1
                });
            region_materials.push(index);
        }
    }
    let lattice_mode =
        assignment.is_some_and(|a| a.regions.iter().any(|r| !r.is_axis_aligned_box()));

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
    let _ = writeln!(out, "  proj   = {proj}");
    out.push_str(&source);

    let _ = writeln!(out, "\n[ M a t e r i a l ]");
    for (index, material) in materials.iter().enumerate() {
        let _ = writeln!(out, "  MAT[{}]", index + 1);
        for nuclide in &material.nuclides {
            let zaaa = zaaa(&nuclide.name)
                .ok_or_else(|| PhitsDeckError::UnknownNuclide(nuclide.name.clone()))?;
            // Negative ratio = mass fraction.
            let _ = writeln!(out, "    {zaaa}  {}", -nuclide.mass_fraction);
        }
    }

    let _ = writeln!(out, "\n[ S u r f a c e ]");
    let _ = writeln!(out, "{}", rpp(GRID_SURFACE, &edges));
    if let Some(assignment) = assignment {
        if lattice_mode {
            // Element-0 box: one pitch of the lattice frame, positioned at
            // the frame's lower corner (unit (0,0,0)'s local coordinates).
            let element = [
                [edges[0][0], edges[0][0] + geometry.spacing_mm[0] / 10.0],
                [edges[1][0], edges[1][0] + geometry.spacing_mm[1] / 10.0],
                [edges[2][0], edges[2][0] + geometry.spacing_mm[2] / 10.0],
            ];
            let _ = writeln!(out, "{}", rpp(ELEMENT_SURFACE, &element));
        } else {
            for (index, region) in assignment.regions.iter().enumerate() {
                let Some((lower_mm, upper_mm)) = region.world_bounds_mm(geometry) else {
                    continue;
                };
                let region_edges = [
                    [lower_mm[0] / 10.0, upper_mm[0] / 10.0],
                    [lower_mm[1] / 10.0, upper_mm[1] / 10.0],
                    [lower_mm[2] / 10.0, upper_mm[2] / 10.0],
                ];
                let _ = writeln!(
                    out,
                    "{}",
                    rpp(REGION_SURFACE_BASE + index as u32, &region_edges)
                );
            }
        }
    }

    let _ = writeln!(out, "\n[ C e l l ]");
    if lattice_mode {
        // World cell: grid box filled by universe 1 — the lattice.
        let _ = writeln!(out, "    1  0        -{GRID_SURFACE}  FILL=1");
        let _ = writeln!(out, "  101  0        -{GRID_SURFACE}  LAT=1  U=1");
        let _ = writeln!(
            out,
            "             FILL=0:{}  0:{}  0:{}",
            nx - 1,
            ny - 1,
            nz - 1
        );
        // Universe per voxel; i fastest, then j, then k (bundle order).
        let mut owner = vec![0_usize; (nx * ny * nz) as usize];
        if let Some(assignment) = assignment {
            for (region_index, region) in assignment.regions.iter().enumerate() {
                region.for_each_voxel(|v| {
                    owner[v[0] as usize
                        + nx as usize * v[1] as usize
                        + nx as usize * ny as usize * v[2] as usize] =
                        region_materials[region_index];
                });
            }
        }
        let mut fill = String::from("             ");
        for (index, material_index) in owner.iter().enumerate() {
            if index > 0 && index % 12 == 0 {
                fill.push_str("\n             ");
            }
            let _ = write!(fill, "{}  ", UNIVERSE_BASE + *material_index as u32);
        }
        out.push_str(fill.trim_end());
        out.push('\n');
        // One element universe per material, filling the pitch box.
        for (index, material) in materials.iter().enumerate() {
            let _ = writeln!(
                out,
                "  {:<4} {:<3} -{}  -{ELEMENT_SURFACE}  U={}",
                UNIVERSE_BASE + index as u32,
                index + 1,
                material.density_g_cm3,
                UNIVERSE_BASE + index as u32
            );
        }
        let _ = writeln!(out, "  999  -1       #1    $ outer void");
    } else {
        let mut base = format!("-{GRID_SURFACE}");
        if let Some(assignment) = assignment {
            for index in 0..assignment.regions.len() {
                let _ = write!(base, "  #{}", 2 + index);
            }
        }
        let _ = writeln!(
            out,
            "    1  1  -{}  {base}    $ grid: base material",
            case.material.density_g_cm3
        );
        if let Some(assignment) = assignment {
            for (index, region) in assignment.regions.iter().enumerate() {
                let material = materials[region_materials[index]];
                let _ = writeln!(
                    out,
                    "  {:<4} {:<3} -{}  -{}    $ {}",
                    2 + index,
                    region_materials[index] + 1,
                    material.density_g_cm3,
                    REGION_SURFACE_BASE + index as u32,
                    region.name
                );
            }
        }
        // Outer void: everything outside every defined cell (mat = -1).
        let mut complement = String::new();
        let cell_count = 1 + assignment.map_or(0, |a| a.regions.len());
        for cell in 1..=cell_count {
            let _ = write!(complement, "#{cell}  ");
        }
        let _ = writeln!(out, "  999  -1       {complement}$ outer void");
    }

    // Flux tallies on the case mesh — the importer reads these ANGEL
    // pages back. A neutron source also emits the photon tally for the
    // (n,γ) channel; a photon source emits photon only.
    for part in photon_and_neutron(case.source.particle) {
        let _ = writeln!(out, "\n[ T - T r a c k ]");
        let _ = writeln!(out, "  mesh = xyz");
        let _ = writeln!(
            out,
            "    x-type = 2    nx = {nx}    xmin = {:.6}  xmax = {:.6}",
            edges[0][0], edges[0][1]
        );
        let _ = writeln!(
            out,
            "    y-type = 2    ny = {ny}    ymin = {:.6}  ymax = {:.6}",
            edges[1][0], edges[1][1]
        );
        let _ = writeln!(
            out,
            "    z-type = 2    nz = {nz}    zmin = {:.6}  zmax = {:.6}",
            edges[2][0], edges[2][1]
        );
        let _ = writeln!(out, "  part = {part}");
        let _ = writeln!(out, "  axis = xy");
        let _ = writeln!(out, "  unit = 1        $ 1/cm^2/source");
        let _ = writeln!(out, "  file = track-{part}.out");
    }

    let _ = writeln!(out, "\n[ E n d ]");
    Ok(out)
}

/// `neutron` tallies both particles (capture gammas); `photon` tallies
/// photon only.
fn photon_and_neutron(particle: ParticleType) -> &'static [&'static str] {
    match particle {
        ParticleType::Neutron => &["neutron", "photon"],
        ParticleType::Photon => &["photon"],
    }
}

/// `ElA[_mN]` (e.g. `H1`, `B10`) → PHITS ZAAA integer: `Z*1000 + A`, plus
/// `50` on the mass for metastable states (`Am242_m1` → `95292`).
fn zaaa(name: &str) -> Option<u64> {
    let bytes = name.as_bytes();
    let mut index = 1;
    if bytes.get(index).is_some_and(|b| b.is_ascii_lowercase()) {
        index += 1;
    }
    let element = &name[..index];
    let z = ELEMENTS.iter().position(|e| *e == element)? as u64 + 1;
    let rest = &name[index..];
    let (mass_str, isomer) = match rest.split_once("_m") {
        Some((mass, state)) => (mass, state.parse::<u64>().ok()?),
        None => (rest, 0),
    };
    let mass: u64 = mass_str.parse().ok()?;
    Some(z * 1000 + mass + if isomer > 0 { 50 } else { 0 })
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
    use openbnct_transport::{
        FixedSourceDefinition, MaterialAssignment, MaterialDefinition, MaterialRegion,
        MaterialRegionShape, NuclideMassFraction,
    };

    fn material(id: &str, density: f64) -> MaterialDefinition {
        MaterialDefinition {
            schema_version: "openbnct.material-definition/0.1.0".into(),
            id: id.into(),
            density_g_cm3: density,
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
        }
    }

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
            material: material("water", 1.0),
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

    fn options() -> PhitsDeckOptions {
        PhitsDeckOptions {
            case_sha256: "sha256:x".into(),
            maxbch: 10,
        }
    }

    #[test]
    fn emits_runnable_skeleton() {
        let deck = export_phits_deck(&case(), None, &options()).unwrap();
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
        // neutron source → both tallies
        assert!(deck.contains("file = track-neutron.out"));
        assert!(deck.contains("file = track-photon.out"));
        // outer void is the -1 material cell
        assert!(deck.contains("999  -1"));
    }

    #[test]
    fn voxel_box_regions_emit_carved_cells() {
        let mut region_material = material("bone", 1.6);
        region_material.nuclides = vec![
            NuclideMassFraction {
                name: "H1".into(),
                mass_fraction: 0.1119,
            },
            NuclideMassFraction {
                name: "O16".into(),
                mass_fraction: 0.8871,
            },
            NuclideMassFraction {
                name: "Ca40".into(),
                mass_fraction: 0.001,
            },
        ];
        let assignment = MaterialAssignment {
            schema_version: "openbnct.material-assignment/0.2.0".into(),
            case_id: "t".into(),
            base_material: case().material,
            provenance_id: "p".into(),
            regions: vec![MaterialRegion {
                name: "bone".into(),
                shape: MaterialRegionShape::VoxelBox {
                    lower: [0, 0, 0],
                    upper: [1, 1, 1],
                },
                material: region_material,
            }],
        };
        let deck = export_phits_deck(&case(), Some(&assignment), &options()).unwrap();
        // region box surface + carved cell + second MAT
        assert!(deck.contains("101  RPP"));
        assert!(deck.contains("MAT[2]"));
        assert!(deck.contains("20040  -0.001"));
        assert!(deck.contains("#2"));
        assert!(!deck.contains("LAT=1"), "box regions stay CSG");
    }

    #[test]
    fn voxel_set_forces_lattice_fill() {
        let assignment = MaterialAssignment {
            schema_version: "openbnct.material-assignment/0.2.0".into(),
            case_id: "t".into(),
            base_material: case().material,
            provenance_id: "p".into(),
            regions: vec![MaterialRegion {
                name: "spot".into(),
                shape: MaterialRegionShape::VoxelSet {
                    indices: vec![[0, 0, 0], [3, 3, 3]],
                },
                material: material("dense", 2.0),
            }],
        };
        let deck = export_phits_deck(&case(), Some(&assignment), &options()).unwrap();
        assert!(deck.contains("LAT=1  U=1"));
        assert!(deck.contains("FILL=0:3  0:3  0:3"));
        assert!(deck.contains(&format!("U={}", UNIVERSE_BASE + 1)));
        // 64 fill entries; voxels [0,0,0] and [3,3,3] map to universe
        // 1001 — the fill list ends where the universe cells begin.
        let fill_start = deck.find("FILL=0:3").unwrap();
        let fill_end = deck[fill_start..].find("\n  1000 ").unwrap() + fill_start;
        let fill_block = &deck[fill_start..fill_end];
        assert_eq!(fill_block.matches("1001").count(), 2);
        assert_eq!(fill_block.matches("1000").count(), 62);
    }

    #[test]
    fn axis_plane_source_emits_rect() {
        let mut c = case();
        c.source.space = SourceSpatialDistribution::UniformAxisPlane {
            axis: openbnct_transport::PlaneAxis::X,
            u_range_cm: [-1.0, 1.0],
            v_range_cm: [-0.5, 0.5],
            offset_cm: -1.0,
            interval_convention: openbnct_transport::IntervalConvention::HalfOpen,
        };
        let deck = export_phits_deck(&c, None, &options()).unwrap();
        assert!(deck.contains("s-type = 2"));
        assert!(deck.contains("x0 = -1.000000"));
        assert!(deck.contains("x1 = -1.000000"));
        assert!(deck.contains("z0 = -0.500000"));
    }

    #[test]
    fn spectrum_source_emits_e_type_block() {
        let mut c = case();
        c.source.energy = EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: vec![1.0, 1e6, 2e7],
            bin_weights: vec![0.7, 0.3],
        };
        let deck = export_phits_deck(&c, None, &options()).unwrap();
        assert!(deck.contains("e-type = 1"));
        assert!(deck.contains("ne = 2"));
        assert!(deck.contains("1.000000e0")); // upper edge in MeV
    }

    #[test]
    fn monodirectional_off_z_sets_phi() {
        let mut c = case();
        c.source.angle = AngularDistribution::Monodirectional {
            unit_vector: [0.0, 1.0, 0.0],
        };
        let deck = export_phits_deck(&c, None, &options()).unwrap();
        assert!(deck.contains("dir = 0.000000000"));
        assert!(deck.contains("phi = 90.000000"));
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
            export_phits_deck(&c, None, &PhitsDeckOptions::default()),
            Err(PhitsDeckError::UnsupportedFeature(_))
        ));
    }

    #[test]
    fn metastable_maps_to_zaaa_plus_50() {
        assert_eq!(zaaa("B10"), Some(5010));
        assert_eq!(zaaa("Am242_m1"), Some(95292));
        assert_eq!(zaaa("O16"), Some(8016));
    }
}
