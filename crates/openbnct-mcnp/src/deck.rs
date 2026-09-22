// SPDX-License-Identifier: MIT

//! MCNP input-deck export.
//!
//! Emits a runnable MCNP deck from a validated [`TransportCase`] plus an
//! optional [`MaterialAssignment`]: the scoring grid becomes one `RPP` box,
//! `voxel_box` regions become exact `RPP` cells carved out of it, and any
//! `voxel_set` region forces the whole grid into a `LAT=1` rectilinear
//! lattice whose `FILL` array assigns a per-material universe to every
//! voxel — the same semantics the OpenMC emitter realizes.
//!
//! What the deck deliberately does *not* contain: component-dose folding.
//! The emitted `FMESH` tallies score particle flux on the case mesh; folding
//! flux into the four dose components applies the published response set,
//! which is the external pipeline's declared step before
//! [`crate::interchange_from_meshtals`] re-imports the result. NCTForge never
//! writes reaction multipliers into the deck that would silently redefine
//! the component semantics.
//!
//! Format conventions used here (documented, verified by structure only —
//! real-engine acceptance is the R4 gate):
//!
//! - units are centimetres; the case geometry's millimetre fields are
//!   converted on emission;
//! - `SDEF` samples rectangular planes as `X`/`Y`/`Z` absolute-coordinate
//!   distributions (`SI`/`SP` `1 1` = uniform on each interval) and disks
//!   as `POS`/`AXS`/`RAD` with `EXT=0` (`SP -21 1` = uniform area);
//!   `VEC`/`DIR=1` gives a monodirectional beam while a cone emits as a
//!   `DIR` cosine histogram (`SI -1 cos(θ) 1`, `SP 0 1` — uniform in
//!   solid angle); `ERG` is MeV, tabulated spectra emit as `SI H`
//!   boundaries with `SP D` bin weights;
//! - `FILL` array order is `i` fastest, then `j`, then `k`, matching the
//!   bundle convention `i + nx*j + nx*ny*k`;
//! - mass fractions emit as *negative* `M`-card entries per MCNP convention;
//! - metastable nuclides map `El_AAA_mN` to ZAID `ZZZAAA + 300 + 100*N`
//!   (e.g. `Am242_m1` → `95642`);
//! - cross-section tables come from the operator-declared `--xs-suffix`
//!   (recorded in the header) or bare ZAIDs resolved by `xsdir`.

use std::fmt::Write as _;

use openbnct_transport::{
    AngularDistribution, EnergyDistribution, MaterialAssignment, MaterialDefinition, ParticleType,
    TransportCase, TransportModelError,
};
use thiserror::Error;

/// Grid-box surface card number; region boxes occupy `101 + index`, the
/// lattice element-0 box is `9001`, and the world boundary is `9998`.
const GRID_SURFACE: u32 = 1;
const REGION_SURFACE_BASE: u32 = 101;
const ELEMENT_SURFACE: u32 = 9001;
const WORLD_SURFACE: u32 = 9998;

/// Graveyard cell number; universe cells occupy `1000 + material_index`.
const GRAVEYARD_CELL: u32 = 9999;
const UNIVERSE_CELL_BASE: u32 = 1000;

/// Neutron and photon `FMESH` tally numbers.
const NEUTRON_MESH_TALLY: u32 = 4;
const PHOTON_MESH_TALLY: u32 = 14;

/// Longest emitted card line; longer cards wrap with `&` continuations.
const CARD_WIDTH: usize = 110;

/// Operator-facing export choices that are recorded in the deck header so
/// the emitted file never hides how it was produced.
#[derive(Debug, Clone, Default)]
pub struct McnpDeckOptions {
    /// Cross-section table suffix such as `80c` (emitted as `.80c` on every
    /// ZAID). `None` emits bare ZAIDs, which MCNP resolves through `xsdir`
    /// defaults — also recorded in the header.
    pub xs_suffix: Option<String>,
    /// Optional `RAND SEED=` value; omitted cards use MCNP's default seed.
    pub seed: Option<u64>,
    /// `sha256:<hex>` of the source case document, bound into the header.
    pub case_sha256: String,
}

#[derive(Debug, Error)]
pub enum McnpDeckError {
    #[error("invalid transport case: {0}")]
    InvalidCase(#[from] TransportModelError),
    #[error("material assignment case_id {assignment} does not match case {case}")]
    AssignmentCaseMismatch { assignment: String, case: String },
    #[error("assignment base material differs from the case material")]
    AssignmentBaseMismatch,
    #[error(
        "invalid cross-section suffix {0:?}: expected digits plus an optional letter, e.g. `80c`"
    )]
    InvalidXsSuffix(String),
    #[error("nuclide name {0:?} cannot be mapped to a ZAID")]
    UnknownNuclide(String),
    #[error("source plane escapes the grid box; particles would be born in the void")]
    SourceOutsideGrid,
}

/// Emit a complete MCNP input deck for `case` (+ optional `assignment`).
///
/// The deck is deterministic: identical inputs byte-for-byte reproduce it.
pub fn export_mcnp_deck(
    case: &TransportCase,
    assignment: Option<&MaterialAssignment>,
    options: &McnpDeckOptions,
) -> Result<String, McnpDeckError> {
    case.validate()?;
    if let Some(assignment) = assignment {
        if assignment.case_id != case.case_id {
            return Err(McnpDeckError::AssignmentCaseMismatch {
                assignment: assignment.case_id.clone(),
                case: case.case_id.clone(),
            });
        }
        if assignment.base_material != case.material {
            return Err(McnpDeckError::AssignmentBaseMismatch);
        }
        assignment.validate(&case.geometry)?;
    }
    let xs_suffix = options
        .xs_suffix
        .as_deref()
        .map(normalize_xs_suffix)
        .transpose()?;
    // A source point outside the grid box is born in the void cell and
    // scores nothing — reject rather than emit a deck that reads as zero.
    let edges = grid_edges_cm(case);
    let inside = |a: usize, lo: f64, hi: f64| {
        let range = edges[a][0]..=edges[a][1];
        range.contains(&lo) && range.contains(&hi)
    };
    let contained = match &case.source.space {
        openbnct_transport::SourceSpatialDistribution::UniformDisk {
            axis,
            offset_cm,
            center_uv_cm,
            radius_cm,
        } => {
            let (u_axis, v_axis) = axis.in_plane_axes();
            inside(axis.index(), *offset_cm, *offset_cm)
                && inside(
                    u_axis,
                    center_uv_cm[0] - radius_cm,
                    center_uv_cm[0] + radius_cm,
                )
                && inside(
                    v_axis,
                    center_uv_cm[1] - radius_cm,
                    center_uv_cm[1] + radius_cm,
                )
        }
        space => {
            let Some((axis, offset_cm, u_range, v_range)) = space.plane_parts() else {
                return Err(McnpDeckError::SourceOutsideGrid);
            };
            let (u_axis, v_axis) = axis.in_plane_axes();
            inside(axis.index(), offset_cm, offset_cm)
                && inside(u_axis, u_range[0], u_range[1])
                && inside(v_axis, v_range[0], v_range[1])
        }
    };
    if !contained {
        return Err(McnpDeckError::SourceOutsideGrid);
    }
    let neutron = case.source.particle == ParticleType::Neutron;

    let mut deck = String::new();
    let _ = writeln!(deck, "openbnct case {} research deck", case.case_id);
    let _ = writeln!(
        deck,
        "c generated by openbnct export mcnp — research only, no clinical claim"
    );
    let _ = writeln!(deck, "c case {0}", options.case_sha256);
    match &xs_suffix {
        Some(suffix) => {
            let _ = writeln!(deck, "c cross sections: operator-declared suffix {suffix}");
        }
        None => {
            let _ = writeln!(
                deck,
                "c cross sections: bare ZAIDs resolved by xsdir defaults"
            );
        }
    }
    let _ = writeln!(
        deck,
        "c fmesh tallies score {0} flux on the case mesh; component dose",
        if neutron { "neutron/photon" } else { "photon" }
    );
    let _ = writeln!(
        deck,
        "c folding is the external pipeline's declared step (response set),"
    );
    let _ = writeln!(
        deck,
        "c then `openbnct import mcnp` re-ingests the meshtal."
    );

    let cell_cards = cell_cards(case, assignment);
    let surface_cards = surface_cards(case, assignment);
    let data_cards = data_cards(case, assignment, neutron, xs_suffix.as_deref(), options)?;

    deck.push_str(&cell_cards);
    deck.push('\n');
    deck.push_str(&surface_cards);
    deck.push('\n');
    deck.push_str(&data_cards);
    Ok(deck)
}

fn normalize_xs_suffix(raw: &str) -> Result<String, McnpDeckError> {
    let suffix = raw.strip_prefix('.').unwrap_or(raw);
    let digits = suffix.len().saturating_sub(usize::from(
        suffix
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphabetic()),
    ));
    let valid = !suffix.is_empty()
        && (2..=3).contains(&digits)
        && suffix[..digits].chars().all(|c| c.is_ascii_digit())
        && suffix[digits..].chars().all(|c| c.is_ascii_lowercase());
    if valid {
        Ok(format!(".{suffix}"))
    } else {
        Err(McnpDeckError::InvalidXsSuffix(raw.to_owned()))
    }
}

/// World-space (cm) edges of the grid box along each axis.
fn grid_edges_cm(case: &TransportCase) -> [[f64; 2]; 3] {
    let mut edges = [[0.0; 2]; 3];
    for (axis, edge) in edges.iter_mut().enumerate() {
        let first_center = case.geometry.origin_mm[axis];
        let spacing = case.geometry.spacing_mm[axis];
        let n = case.geometry.shape[axis] as f64;
        *edge = [
            (first_center - 0.5 * spacing) / 10.0,
            (first_center + (n - 0.5) * spacing) / 10.0,
        ];
    }
    edges
}

/// All materials in emission order: the base material is `m1`; each region
/// material gets `m(2 + distinct_index)` with identical definitions deduped,
/// matching the OpenMC emitter's `region_material_ids` semantics.
fn material_table<'a>(
    case: &'a TransportCase,
    assignment: Option<&'a MaterialAssignment>,
) -> (Vec<&'a MaterialDefinition>, Vec<usize>) {
    let mut materials: Vec<&MaterialDefinition> = vec![&case.material];
    let mut region_ids = Vec::new();
    if let Some(assignment) = assignment {
        for region in &assignment.regions {
            let index = materials
                .iter()
                .position(|existing| **existing == region.material)
                .unwrap_or_else(|| {
                    materials.push(&region.material);
                    materials.len() - 1
                });
            region_ids.push(index);
        }
    }
    (materials, region_ids)
}

fn cell_cards(case: &TransportCase, assignment: Option<&MaterialAssignment>) -> String {
    let lattice_mode = assignment.is_some_and(|assignment| {
        assignment
            .regions
            .iter()
            .any(|region| !region.is_axis_aligned_box())
    });
    let (materials, region_ids) = material_table(case, assignment);

    let mut out = String::new();
    let _ = writeln!(out, "c cell cards");
    if lattice_mode {
        // The whole grid box is the LAT=1 lattice cell; every element is
        // filled by a per-material universe.
        let [nx, ny, nz] = case.geometry.shape.map(|d| d as usize);
        let _ = writeln!(out, "1  0  -{GRID_SURFACE}  lat=1  imp:n,p=1  &");
        let _ = writeln!(out, "     fill=0:{} 0:{} 0:{}  &", nx - 1, ny - 1, nz - 1);
        // FILL order: i fastest, then j, then k (see module docs).
        let mut owner = vec![0_usize; nx * ny * nz];
        if let Some(assignment) = assignment {
            for (region_index, region) in assignment.regions.iter().enumerate() {
                region.for_each_voxel(|voxel| {
                    owner[voxel[0] as usize
                        + nx * voxel[1] as usize
                        + nx * ny * voxel[2] as usize] = region_ids[region_index];
                });
            }
        }
        let mut fill = String::from("     ");
        for material_index in &owner {
            let token = format!("{}", UNIVERSE_CELL_BASE + *material_index as u32);
            if fill.len() - fill.rfind('\n').map_or(0, |p| p + 1) + token.len() > CARD_WIDTH {
                fill.push_str("&\n     ");
            }
            fill.push_str(&token);
            fill.push(' ');
        }
        out.push_str(fill.trim_end());
        out.push('\n');
    } else {
        let mut base_spec = format!("-{GRID_SURFACE}");
        if let Some(assignment) = assignment {
            for index in 0..assignment.regions.len() {
                base_spec.push_str(&format!(" #{}", 2 + index));
            }
        }
        write_card(
            &mut out,
            &format!(
                "1  1  {}  {base_spec}  imp:n,p=1",
                fmt(-case.material.density_g_cm3)
            ),
        );
        if assignment.is_some() {
            for (index, material_index) in region_ids.iter().enumerate() {
                let material = materials[*material_index];
                write_card(
                    &mut out,
                    &format!(
                        "{}  {}  {}  -{}  imp:n,p=1",
                        2 + index,
                        material_index + 1,
                        fmt(-material.density_g_cm3),
                        REGION_SURFACE_BASE + index as u32
                    ),
                );
            }
        }
    }
    if lattice_mode {
        // One universe cell per material, each filling the element-0 box.
        for (index, material) in materials.iter().enumerate() {
            write_card(
                &mut out,
                &format!(
                    "{}  {}  {}  -{ELEMENT_SURFACE}  u={}  imp:n,p=1",
                    UNIVERSE_CELL_BASE + index as u32,
                    index + 1,
                    fmt(-material.density_g_cm3),
                    UNIVERSE_CELL_BASE + index as u32
                ),
            );
        }
    }
    write_card(
        &mut out,
        &format!("{GRAVEYARD_CELL}  0  {GRID_SURFACE} -{WORLD_SURFACE}  imp:n,p=0"),
    );
    out
}

fn surface_cards(case: &TransportCase, assignment: Option<&MaterialAssignment>) -> String {
    let edges = grid_edges_cm(case);
    let mut out = String::from("c surface cards\n");
    write_card(
        &mut out,
        &format!("{GRID_SURFACE}  rpp {}", rpp_args(&edges)),
    );
    if let Some(assignment) = assignment {
        let lattice_mode = assignment
            .regions
            .iter()
            .any(|region| !region.is_axis_aligned_box());
        if !lattice_mode {
            for (index, region) in assignment.regions.iter().enumerate() {
                if let Some((lower_mm, upper_mm)) = region.world_bounds_mm(&case.geometry) {
                    let edges = [
                        [lower_mm[0] / 10.0, upper_mm[0] / 10.0],
                        [lower_mm[1] / 10.0, upper_mm[1] / 10.0],
                        [lower_mm[2] / 10.0, upper_mm[2] / 10.0],
                    ];
                    write_card(
                        &mut out,
                        &format!(
                            "{}  rpp {}",
                            REGION_SURFACE_BASE + index as u32,
                            rpp_args(&edges)
                        ),
                    );
                }
            }
        } else {
            // Element-0 box: the lattice pitch extent starting at the grid
            // lower corner; MCNP replicates it into every element.
            let element = [
                [
                    edges[0][0],
                    edges[0][0] + case.geometry.spacing_mm[0] / 10.0,
                ],
                [
                    edges[1][0],
                    edges[1][0] + case.geometry.spacing_mm[1] / 10.0,
                ],
                [
                    edges[2][0],
                    edges[2][0] + case.geometry.spacing_mm[2] / 10.0,
                ],
            ];
            write_card(
                &mut out,
                &format!("{ELEMENT_SURFACE}  rpp {}", rpp_args(&element)),
            );
        }
    }
    // World boundary: the grid box grown by one voxel pitch per axis so the
    // void cell is a thin shell and the source plane stays inside.
    let mut world = edges;
    for (axis_edges, spacing_mm) in world.iter_mut().zip(&case.geometry.spacing_mm) {
        let margin = spacing_mm / 10.0;
        axis_edges[0] -= margin;
        axis_edges[1] += margin;
    }
    write_card(
        &mut out,
        &format!("{WORLD_SURFACE}  rpp {}", rpp_args(&world)),
    );
    out
}

fn data_cards(
    case: &TransportCase,
    assignment: Option<&MaterialAssignment>,
    neutron: bool,
    xs_suffix: Option<&str>,
    options: &McnpDeckOptions,
) -> Result<String, McnpDeckError> {
    let mut out = String::from("c data cards\n");
    let _ = writeln!(out, "mode {}", if neutron { "n p" } else { "p" });
    let (materials, _) = material_table(case, assignment);

    // M cards: mass fractions emit negative; the library suffix is the
    // operator's declaration, never invented here. Each `zaid frac` pair
    // stays on one line — continuations break only between nuclides.
    for (index, material) in materials.iter().enumerate() {
        let mut card = format!("m{}", index + 1);
        for nuclide in &material.nuclides {
            let zaid = zaid(&nuclide.name)
                .ok_or_else(|| McnpDeckError::UnknownNuclide(nuclide.name.clone()))?;
            let entry = format!(
                "{zaid}{} {}",
                xs_suffix.unwrap_or_default(),
                fmt(-nuclide.mass_fraction)
            );
            let line_len = card.len() - card.rfind('\n').map_or(0, |p| p + 1);
            if line_len + 1 + entry.len() > CARD_WIDTH {
                card.push_str(" &\n     ");
            } else {
                card.push(' ');
            }
            card.push_str(&entry);
        }
        out.push_str(&card);
        out.push('\n');
    }

    // Source card. Rectangular planes emit as per-coordinate uniform
    // distributions; a disk emits as POS/AXS/RAD with EXT=0. The cone
    // emits as a DIR cosine histogram about VEC — MCNP samples mu
    // uniformly on the tabulated bin, which is uniform in solid angle.
    let source = &case.source;
    let coord = |i: usize| ["x", "y", "z"][i];
    let (mut sdef, mut dist_index) = (String::from("sdef"), 0_u32);
    let mut si_sp = String::new();
    let dist = |si_sp: &mut String, index: u32, si: String, sp: &str| {
        write_card(si_sp, &format!("si{index}  {si}"));
        write_card(si_sp, &format!("sp{index}  {sp}"));
    };
    match &source.space {
        openbnct_transport::SourceSpatialDistribution::UniformDisk {
            axis,
            offset_cm,
            center_uv_cm,
            radius_cm,
        } => {
            let (u_axis, v_axis) = axis.in_plane_axes();
            let mut pos = [0.0; 3];
            pos[axis.index()] = *offset_cm;
            pos[u_axis] = center_uv_cm[0];
            pos[v_axis] = center_uv_cm[1];
            let mut axs = [0.0; 3];
            axs[axis.index()] = 1.0;
            dist_index += 1;
            let _ = write!(
                sdef,
                "  pos={} {} {}  axs={} {} {}  ext=0  rad=d{}",
                fmt(pos[0]),
                fmt(pos[1]),
                fmt(pos[2]),
                fmt(axs[0]),
                fmt(axs[1]),
                fmt(axs[2]),
                dist_index
            );
            dist(
                &mut si_sp,
                dist_index,
                format!("0  {}", fmt(*radius_cm)),
                "-21  1",
            );
        }
        space => {
            let Some((axis, offset_cm, u_range, v_range)) = space.plane_parts() else {
                return Err(McnpDeckError::SourceOutsideGrid);
            };
            let (u_axis, v_axis) = axis.in_plane_axes();
            dist_index += 1;
            let u_dist = dist_index;
            dist_index += 1;
            let v_dist = dist_index;
            let _ = write!(
                sdef,
                "  {}=d{}  {}=d{}  {}={}",
                coord(u_axis),
                u_dist,
                coord(v_axis),
                v_dist,
                coord(axis.index()),
                fmt(offset_cm)
            );
            dist(
                &mut si_sp,
                u_dist,
                format!("{}  {}", fmt(u_range[0]), fmt(u_range[1])),
                "1  1",
            );
            dist(
                &mut si_sp,
                v_dist,
                format!("{}  {}", fmt(v_range[0]), fmt(v_range[1])),
                "1  1",
            );
        }
    }
    match &source.angle {
        AngularDistribution::Monodirectional { unit_vector } => {
            let _ = write!(
                sdef,
                "  vec={} {} {}  dir=1",
                fmt(unit_vector[0]),
                fmt(unit_vector[1]),
                fmt(unit_vector[2])
            );
        }
        AngularDistribution::IsotropicCone {
            axis_unit_vector,
            half_angle_rad,
        } => {
            dist_index += 1;
            let _ = write!(
                sdef,
                "  vec={} {} {}  dir=d{}",
                fmt(axis_unit_vector[0]),
                fmt(axis_unit_vector[1]),
                fmt(axis_unit_vector[2]),
                dist_index
            );
            dist(
                &mut si_sp,
                dist_index,
                format!("-1  {}  1", fmt(half_angle_rad.cos())),
                "0  1",
            );
        }
    }
    match &source.energy {
        EnergyDistribution::Monoenergetic { energy_ev } => {
            let _ = write!(sdef, "  erg={}", fmt(energy_ev / 1.0e6));
        }
        EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev,
            bin_weights,
        } => {
            dist_index += 1;
            let _ = write!(sdef, "  erg=d{}", dist_index);
            let si = std::iter::once(String::from("h"))
                .chain(energy_boundaries_ev.iter().map(|e| fmt(*e / 1.0e6)))
                .collect::<Vec<_>>()
                .join("  ");
            let sp = std::iter::once(String::from("d  0"))
                .chain(bin_weights.iter().map(|w| fmt(*w)))
                .collect::<Vec<_>>()
                .join("  ");
            dist(&mut si_sp, dist_index, si, &sp);
        }
    }
    let _ = write!(
        sdef,
        "  par={}",
        match source.particle {
            ParticleType::Neutron => "n",
            ParticleType::Photon => "p",
        }
    );
    write_card(&mut out, &sdef);
    out.push_str(&si_sp);

    // Mesh flux tallies on the case grid; component folding happens outside.
    let edges = grid_edges_cm(case);
    let mut emit_mesh = |number: u32, particle: &str| {
        write_card(
            &mut out,
            &format!(
                "fmesh{number}:{particle}  geom=xyz  origin={} {} {}  \
                 imesh={}  jmesh={}  kmesh={}  \
                 iints={}  jints={}  kints={}  out=cf",
                fmt(edges[0][0]),
                fmt(edges[1][0]),
                fmt(edges[2][0]),
                fmt(edges[0][1]),
                fmt(edges[1][1]),
                fmt(edges[2][1]),
                case.geometry.shape[0],
                case.geometry.shape[1],
                case.geometry.shape[2]
            ),
        );
    };
    if neutron {
        emit_mesh(NEUTRON_MESH_TALLY, "n");
    }
    emit_mesh(PHOTON_MESH_TALLY, "p");

    let _ = writeln!(out, "nps {}", case.requested_histories);
    if let Some(seed) = options.seed {
        let _ = writeln!(out, "rand seed={seed}");
    }
    Ok(out)
}

/// `ElA[_mN]` (e.g. `H1`, `B10`, `Am242_m1`) → MCNP ZAID integer.
fn zaid(name: &str) -> Option<u64> {
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
    let mass = mass_str.parse::<u64>().ok()?;
    // MCNP isomer convention: ground = ZZZAAA, isomer N adds 300 + 100*N.
    Some(z * 1000 + mass + if isomer > 0 { 300 + 100 * isomer } else { 0 })
}

/// Shortest round-trip decimal; MCNP free format accepts plain and `e`-form.
fn fmt(value: f64) -> String {
    format_float(value)
}

fn format_float(value: f64) -> String {
    if value == 0.0 {
        "0".into()
    } else {
        value.to_string()
    }
}

fn rpp_args(edges: &[[f64; 2]; 3]) -> String {
    format!(
        "{} {}  {} {}  {} {}",
        fmt(edges[0][0]),
        fmt(edges[0][1]),
        fmt(edges[1][0]),
        fmt(edges[1][1]),
        fmt(edges[2][0]),
        fmt(edges[2][1])
    )
}

/// Emit one logical card wrapped to [`CARD_WIDTH`] with `&` continuations.
fn write_card(out: &mut String, card: &str) {
    let mut line = String::new();
    for token in card.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + token.len() > CARD_WIDTH {
            line.push_str("  &\n     ");
        } else if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(token);
    }
    out.push_str(&line);
    out.push('\n');
}

/// Periodic table indexed by atomic number minus one.
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
    use openbnct_transport::MaterialRegion;
    use serde_json::json;

    fn material() -> MaterialDefinition {
        serde_json::from_value(json!({
            "schema_version": "openbnct.material-definition/0.1.0",
            "id": "mat.water-b10",
            "density_g_cm3": 1.0,
            "temperature_k": 300.0,
            "nuclides": [
                {"name": "H1", "mass_fraction": 0.111},
                {"name": "O16", "mass_fraction": 0.889}
            ],
            "neutron_thermal_treatment": "free_gas"
        }))
        .unwrap()
    }

    fn case() -> TransportCase {
        serde_json::from_value(json!({
            "schema_version": "openbnct.transport-case/0.1.0",
            "case_id": "deck-test",
            "geometry": {
                "shape": [2, 2, 2],
                "spacing_mm": [10.0, 10.0, 10.0],
                "origin_mm": [0.0, 0.0, 0.0],
                "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
            },
            "material": material(),
            "source": {
                "schema_version": "openbnct.fixed-source-definition/0.1.0",
                "id": "src.beam",
                "particle": "neutron",
                "source_sites_per_history": 1,
                "statistical_weight_per_site": 1.0,
                "space": {
                    "kind": "uniform_cartesian_plane",
                    "x_range_cm": [-0.5, 0.5],
                    "y_range_cm": [-0.5, 0.5],
                    "z_cm": -0.4,
                    "interval_convention": "half_open"
                },
                "angle": {"kind": "monodirectional", "unit_vector": [0.0, 0.0, 1.0]},
                "energy": {"kind": "monoenergetic", "energy_ev": 1000.0}
            },
            "requested_histories": 1000
        }))
        .unwrap()
    }

    fn options() -> McnpDeckOptions {
        McnpDeckOptions {
            xs_suffix: Some("80c".into()),
            seed: Some(7),
            case_sha256: "sha256:abc".into(),
        }
    }

    #[test]
    fn homogeneous_deck_is_deterministic_and_well_formed() {
        let case = case();
        let options = options();
        let deck = export_mcnp_deck(&case, None, &options).unwrap();
        assert_eq!(deck, export_mcnp_deck(&case, None, &options).unwrap());
        // Three blocks separated by blank lines; title first.
        assert!(deck.starts_with("openbnct case deck-test research deck\n"));
        assert!(deck.contains("c case sha256:abc"));
        // Base cell: material 1 inside grid RPP, importances on.
        assert!(deck.contains("1 1 -1 -1 imp:n,p=1"));
        // Graveyard: outside grid, inside world, importance zero.
        assert!(deck.contains(&format!("{GRAVEYARD_CELL} 0 1 -{WORLD_SURFACE} imp:n,p=0")));
        // M card: ZAIDs with suffix, negative mass fractions.
        assert!(deck.contains("m1 1001.80c -0.111 8016.80c -0.889"));
        // SDEF: z fixed, x/y distributed, 1 keV = 1e-3 MeV.
        assert!(deck.contains("x=d1 y=d2 z=-0.4"));
        assert!(deck.contains("dir=1 erg=0.001 par=n"));
        assert!(deck.contains("si1 -0.5 0.5"));
        assert!(deck.contains("sp1 1 1"));
        // MODE n p (photon production), both mesh tallies, nps, seed.
        assert!(deck.contains("mode n p"));
        assert!(deck.contains("fmesh4:n geom=xyz origin=-0.5 -0.5 -0.5"));
        assert!(deck.contains("imesh=1.5 jmesh=1.5 kmesh=1.5"));
        assert!(deck.contains("iints=2 jints=2 kints=2 out=cf"));
        assert!(deck.contains("fmesh14:p"));
        assert!(deck.contains("nps 1000"));
        assert!(deck.contains("rand seed=7"));
        // No card line exceeds the MCNP free-format limit.
        assert!(deck.lines().all(|line| line.len() <= CARD_WIDTH));
    }

    #[test]
    fn box_region_carves_base_cell() {
        let region: MaterialRegion = serde_json::from_value(json!({
            "name": "tumor",
            "material": {
                "schema_version": "openbnct.material-definition/0.1.0",
                "id": "mat.b10-rich",
                "density_g_cm3": 1.1,
                "temperature_k": 300.0,
                "nuclides": [{"name": "B10", "mass_fraction": 1.0}],
                "neutron_thermal_treatment": "free_gas"
            },
            "shape": {"kind": "voxel_box", "lower": [0, 0, 0], "upper": [0, 0, 0]}
        }))
        .unwrap();
        let assignment = MaterialAssignment {
            schema_version: openbnct_transport::MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: "deck-test".into(),
            base_material: material(),
            regions: vec![region],
            provenance_id: "case:sha256:abc".into(),
        };
        let deck = export_mcnp_deck(&case(), Some(&assignment), &options()).unwrap();
        // Region cell 2 = material 2 inside RPP 101; base cell complements it.
        assert!(deck.contains("2 2 -1.1 -101 imp:n,p=1"));
        assert!(deck.contains("1 1 -1 -1 #2 imp:n,p=1"));
        assert!(deck.contains("101 rpp -0.5 0.5 -0.5 0.5 -0.5 0.5"));
        assert!(deck.contains("m2 5010.80c -1"));
        // No lattice when only box regions exist.
        assert!(!deck.contains("lat=1"));
    }

    #[test]
    fn voxel_set_forces_lattice_fill() {
        let region: MaterialRegion = serde_json::from_value(json!({
            "name": "lesion",
            "material": {
                "schema_version": "openbnct.material-definition/0.1.0",
                "id": "mat.b10-rich",
                "density_g_cm3": 1.1,
                "temperature_k": 300.0,
                "nuclides": [{"name": "B10", "mass_fraction": 1.0}],
                "neutron_thermal_treatment": "free_gas"
            },
            "shape": {"kind": "voxel_set", "indices": [[1, 0, 0], [0, 1, 0]]}
        }))
        .unwrap();
        let assignment = MaterialAssignment {
            schema_version: openbnct_transport::MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: "deck-test".into(),
            base_material: material(),
            regions: vec![region],
            provenance_id: "case:sha256:abc".into(),
        };
        let deck = export_mcnp_deck(&case(), Some(&assignment), &options()).unwrap();
        assert!(deck.contains("lat=1  imp:n,p=1"));
        assert!(deck.contains("fill=0:1 0:1 0:1"));
        // Element universes: 1000 = base (m1), 1001 = region (m2); voxels
        // (1,0,0) and (0,1,0) carry 1001 — flat order i + nx*j + nx*ny*k:
        // owners = [1000,1001, 1001,1000, 1000,1000, 1000,1000].
        assert!(deck.contains("1000 1001 1001 1000 1000 1000 1000 1000"));
        assert!(deck.contains("1000 1 -1 -9001 u=1000 imp:n,p=1"));
        assert!(deck.contains("1001 2 -1.1 -9001 u=1001 imp:n,p=1"));
    }

    #[test]
    fn zaid_mapping_and_suffix_rules() {
        assert_eq!(zaid("H1"), Some(1001));
        assert_eq!(zaid("B10"), Some(5010));
        assert_eq!(zaid("Am242_m1"), Some(95642));
        assert_eq!(zaid("U235"), Some(92235));
        assert_eq!(zaid("Xx1"), None);
        assert_eq!(normalize_xs_suffix("80c").unwrap(), ".80c");
        assert_eq!(normalize_xs_suffix(".80c").unwrap(), ".80c");
        assert!(normalize_xs_suffix("abc").is_err());
        // Bare ZAIDs when no suffix is declared.
        let options = McnpDeckOptions {
            xs_suffix: None,
            seed: None,
            case_sha256: "sha256:x".into(),
        };
        let deck = export_mcnp_deck(&case(), None, &options).unwrap();
        assert!(deck.contains("m1 1001 -0.111 8016 -0.889"));
        assert!(deck.contains("xsdir defaults"));
        assert!(!deck.contains("rand seed"));
    }

    #[test]
    fn assignment_binding_is_enforced() {
        let region: MaterialRegion = serde_json::from_value(json!({
            "name": "lesion",
            "material": {
                "schema_version": "openbnct.material-definition/0.1.0",
                "id": "mat.b10-rich",
                "density_g_cm3": 1.1,
                "temperature_k": 300.0,
                "nuclides": [{"name": "B10", "mass_fraction": 1.0}],
                "neutron_thermal_treatment": "free_gas"
            },
            "shape": {"kind": "voxel_set", "indices": [[1, 0, 0]]}
        }))
        .unwrap();
        let mut assignment = MaterialAssignment {
            schema_version: openbnct_transport::MATERIAL_ASSIGNMENT_SCHEMA.into(),
            case_id: "other-case".into(),
            base_material: material(),
            regions: vec![region],
            provenance_id: "case:sha256:abc".into(),
        };
        assert!(matches!(
            export_mcnp_deck(&case(), Some(&assignment), &options()),
            Err(McnpDeckError::AssignmentCaseMismatch { .. })
        ));
        assignment.case_id = "deck-test".into();
        let mut other = material();
        other.density_g_cm3 = 1.05;
        assignment.base_material = other;
        assert!(matches!(
            export_mcnp_deck(&case(), Some(&assignment), &options()),
            Err(McnpDeckError::AssignmentBaseMismatch)
        ));
        assignment.base_material = material();
        assert!(export_mcnp_deck(&case(), Some(&assignment), &options()).is_ok());
    }

    #[test]
    fn source_outside_the_grid_is_rejected() {
        let mut value = serde_json::to_value(case()).unwrap();
        value["source"]["space"]["z_cm"] = json!(-9.9);
        let case: TransportCase = serde_json::from_value(value).unwrap();
        assert!(matches!(
            export_mcnp_deck(&case, None, &options()),
            Err(McnpDeckError::SourceOutsideGrid)
        ));
    }

    #[test]
    fn disk_cone_and_spectrum_source_emit_distributions() {
        let mut value = serde_json::to_value(case()).unwrap();
        value["source"]["space"] = json!({
            "kind": "uniform_disk",
            "axis": "z",
            "offset_cm": -0.4,
            "center_uv_cm": [0.0, 0.0],
            "radius_cm": 0.4
        });
        value["source"]["angle"] = json!({
            "kind": "isotropic_cone",
            "axis_unit_vector": [0.0, 0.0, 1.0],
            "half_angle_rad": 0.1
        });
        value["source"]["energy"] = json!({
            "kind": "tabulated_histogram",
            "energy_boundaries_ev": [0.5, 1.0e4, 1.0e6],
            "bin_weights": [3.0, 1.0]
        });
        let case: TransportCase = serde_json::from_value(value).unwrap();
        let deck = export_mcnp_deck(&case, None, &options()).unwrap();
        // Disk: POS at the port center, AXS the port normal, RAD sampled
        // with power law n=1 (uniform area), EXT=0 keeps it planar.
        assert!(deck.contains("pos=0 0 -0.4 axs=0 0 1 ext=0 rad=d1"));
        assert!(deck.contains("si1 0 0.4"));
        assert!(deck.contains("sp1 -21 1"));
        // Cone: DIR cosine histogram on [cos 0.1, 1] about VEC.
        assert!(deck.contains("vec=0 0 1 dir=d2"));
        assert!(deck.contains("si2 -1 0.9950041652780258 1"));
        assert!(deck.contains("sp2 0 1"));
        // Spectrum: ERG histogram with H boundaries (MeV) and D weights.
        assert!(deck.contains("erg=d3 par=n"));
        assert!(deck.contains("si3 h 0.0000005 0.01 1"));
        assert!(deck.contains("sp3 d 0 3 1"));
        assert!(deck.lines().all(|line| line.len() <= CARD_WIDTH));
    }
}
