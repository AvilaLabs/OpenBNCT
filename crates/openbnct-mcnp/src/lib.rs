// SPDX-License-Identifier: MIT

//! MCNP meshtal import adapter.
//!
//! Parses the ASCII `meshtal` file an MCNP `FMESH` tally produces (rectangular
//! `GEOM=xyz` meshes, column `OUT=col`/`cf` layouts) and lifts selected
//! tallies into a [`ComponentDoseInterchange`] document for
//! [`openbnct_core::import_component_dose`].
//!
//! Meaning is never invented: the file's tally results are per-source-particle
//! values whose dose semantics (kerma folding, response multipliers) the
//! caller declares through `unit` and `normalization`. Relative errors become
//! absolute one-sigma uncertainties; tallies written without a relative-error
//! column import with no claimed uncertainty. Cylindrical meshes, matrix
//! (`out=ij`/`ik`/`jk`) layouts, non-uniform bin widths, and meshes that
//! disagree across components are all rejected rather than resampled.
//!
//! Grid order note: meshtal rows iterate Z fastest, then Y, then X per energy
//! block; this adapter maps rows by their printed bin-center coordinates into
//! the bundle's `i + nx*j + nx*ny*k` order, so file ordering assumptions are
//! never relied on.

pub mod deck;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use openbnct_core::{
    ComponentDoseInterchange, DoseComponent, DoseUnit, DoseVolume, ExternalProducer, ExternalTotal,
    GridGeometry,
};
use thiserror::Error;

/// One parsed `meshtal` file.
#[derive(Debug, Clone)]
pub struct MeshtalFile {
    /// Version token from the `mcnp version ...` banner (e.g. `6.mpi`), when
    /// the file carried one.
    pub code_version: Option<String>,
    /// `Number of histories used for normalizing tallies`, when present.
    pub histories: Option<f64>,
    pub tallies: Vec<MeshTally>,
}

impl MeshtalFile {
    pub fn tally(&self, number: u32) -> Option<&MeshTally> {
        self.tallies.iter().find(|t| t.number == number)
    }
}

/// One `Mesh Tally Number` block: an xyz mesh plus its scored values.
#[derive(Debug, Clone)]
pub struct MeshTally {
    pub number: u32,
    /// Particle line, e.g. `neutron` or `photon` (free text from the file).
    pub particle: String,
    /// Bin boundaries per axis, in centimetres.
    pub x_edges_cm: Vec<f64>,
    pub y_edges_cm: Vec<f64>,
    pub z_edges_cm: Vec<f64>,
    /// Energy bin boundaries in MeV; a single-bin tally has two entries.
    pub energy_edges_mev: Vec<f64>,
    /// Whether data rows carry a leading `Energy` column.
    pub has_energy_column: bool,
    /// Whether data rows carry a `Rel Error` column.
    pub has_relative_error: bool,
    /// Scored rows: (energy_bin, x_index, y_index, z_index, result, rel_error).
    /// Indices are already mapped onto the bin grids above.
    pub rows: Vec<MeshTallyRow>,
}

#[derive(Debug, Clone, Copy)]
pub struct MeshTallyRow {
    pub energy_bin: usize,
    pub ix: usize,
    pub iy: usize,
    pub iz: usize,
    pub result: f64,
    pub relative_error: Option<f64>,
}

impl MeshTally {
    pub fn nx(&self) -> usize {
        self.x_edges_cm.len() - 1
    }
    pub fn ny(&self) -> usize {
        self.y_edges_cm.len() - 1
    }
    pub fn nz(&self) -> usize {
        self.z_edges_cm.len() - 1
    }
    pub fn energy_bin_count(&self) -> usize {
        self.energy_edges_mev.len().saturating_sub(1)
    }
}

/// Which tally (and energy bin) supplies one dose component.
#[derive(Debug, Clone)]
pub struct ComponentSource {
    pub component: DoseComponent,
    /// File the tally lives in.
    pub file: PathBuf,
    pub tally: u32,
    /// Required when the tally scored more than one energy bin; `None`
    /// selects the sole bin of a single-bin tally.
    pub energy_bin: Option<usize>,
}

/// Parse an ASCII meshtal file.
pub fn parse_meshtal(text: &str) -> Result<MeshtalFile, McnpError> {
    let mut code_version = None;
    let mut histories = None;
    let mut tallies = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut cursor = 0;
    while cursor < lines.len() {
        let line = lines[cursor];
        let lower = line.to_ascii_lowercase();
        if code_version.is_none()
            && lower.contains("mcnp")
            && lower.contains("version")
            && let Some(rest) = lower.split("version").nth(1)
        {
            code_version = rest.split_whitespace().next().map(str::to_owned);
        }
        if lower.contains("number of histories used for normalizing tallies") {
            histories = line
                .split('=')
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .and_then(|s| s.parse::<f64>().ok());
        }
        if lower.contains("mesh tally number") {
            let (tally, consumed) = parse_tally(&lines[cursor..])?;
            tallies.push(tally);
            cursor += consumed;
            continue;
        }
        cursor += 1;
    }
    if tallies.is_empty() {
        return Err(McnpError::NoTallies);
    }
    Ok(MeshtalFile {
        code_version,
        histories,
        tallies,
    })
}

fn parse_f64(token: &str) -> Option<f64> {
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn numeric_tokens(line: &str) -> Vec<f64> {
    line.split_whitespace().filter_map(parse_f64).collect()
}

fn is_numeric_line(line: &str) -> bool {
    !line.trim().is_empty() && line.split_whitespace().all(|t| parse_f64(t).is_some())
}

/// Collect every float on `line` after `label`, plus following lines while
/// they remain purely numeric (boundary lists wrap).
fn parse_boundary_list(lines: &[&str], start: usize, label: &str) -> (Vec<f64>, usize) {
    let mut values = Vec::new();
    let mut cursor = start;
    let first = lines[cursor];
    if let Some(pos) = first.to_ascii_lowercase().find(label) {
        values.extend(numeric_tokens(&first[pos + label.len()..]));
    }
    cursor += 1;
    while cursor < lines.len() && is_numeric_line(lines[cursor]) {
        values.extend(numeric_tokens(lines[cursor]));
        cursor += 1;
    }
    (values, cursor)
}

/// Parse one tally block starting at the `Mesh Tally Number` line; returns
/// the tally and the number of lines consumed.
fn parse_tally(lines: &[&str]) -> Result<(MeshTally, usize), McnpError> {
    let header = lines[0];
    let number = header
        .split_whitespace()
        .last()
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or(McnpError::Malformed("mesh tally number line"))?;
    let mut cursor = 1;
    // Particle descriptor: first non-empty line after the tally number.
    let mut particle = String::new();
    while cursor < lines.len() {
        let trimmed = lines[cursor].trim();
        if trimmed.is_empty() {
            cursor += 1;
            continue;
        }
        particle = trimmed.trim_end_matches('.').to_owned();
        cursor += 1;
        break;
    }
    // Bin boundary section.
    let mut x_edges = None;
    let mut y_edges = None;
    let mut z_edges = None;
    let mut e_edges = None;
    while cursor < lines.len() {
        let line = lines[cursor].to_ascii_lowercase();
        if line.contains("cylinder") || line.contains("r direction:") {
            return Err(McnpError::UnsupportedMesh);
        }
        if line.contains("x direction:") {
            let (v, next) = parse_boundary_list(lines, cursor, "x direction:");
            x_edges = Some(v);
            cursor = next;
            continue;
        }
        if line.contains("y direction:") {
            let (v, next) = parse_boundary_list(lines, cursor, "y direction:");
            y_edges = Some(v);
            cursor = next;
            continue;
        }
        if line.contains("z direction:") {
            let (v, next) = parse_boundary_list(lines, cursor, "z direction:");
            z_edges = Some(v);
            cursor = next;
            continue;
        }
        if line.contains("energy bin boundaries") || line.contains("energy boundaries") {
            let (v, next) = parse_boundary_list(lines, cursor, "boundaries:");
            e_edges = Some(v);
            cursor = next;
            continue;
        }
        // Column header names the data layout.
        let trimmed = lines[cursor].trim();
        if trimmed.starts_with("Energy") || trimmed.starts_with('X') || trimmed.contains("Result") {
            break;
        }
        if trimmed.starts_with("Mesh Tally Number") {
            return Err(McnpError::Malformed("tally ended before data"));
        }
        cursor += 1;
    }
    if cursor >= lines.len() {
        return Err(McnpError::Malformed("tally header without data"));
    }
    let columns: Vec<String> = lines[cursor]
        .split_whitespace()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if !columns.iter().any(|c| c == "result") {
        // Matrix layouts (out=ij/ik/jk) do not expose per-row Result columns.
        return Err(McnpError::UnsupportedLayout);
    }
    let has_energy_column = columns.iter().any(|c| c == "energy");
    let has_relative_error = columns.iter().any(|c| c == "rel");
    cursor += 1;

    let x_edges = x_edges.ok_or(McnpError::Malformed("missing X direction"))?;
    let y_edges = y_edges.ok_or(McnpError::Malformed("missing Y direction"))?;
    let z_edges = z_edges.ok_or(McnpError::Malformed("missing Z direction"))?;
    let e_edges = e_edges.ok_or(McnpError::Malformed("missing energy boundaries"))?;
    if x_edges.len() < 2 || y_edges.len() < 2 || z_edges.len() < 2 {
        return Err(McnpError::Malformed("boundary list shorter than two edges"));
    }

    // Data rows until a non-numeric line ends the block. Rows are grouped by
    // energy value in order of appearance: the k-th distinct energy label is
    // energy bin k.
    let mut energy_order: Vec<f64> = Vec::new();
    let mut rows = Vec::new();
    let expected = (x_edges.len() - 1) * (y_edges.len() - 1) * (z_edges.len() - 1);
    while cursor < lines.len() && is_numeric_line(lines[cursor]) {
        let tokens = numeric_tokens(lines[cursor]);
        let mut at = 0;
        let energy_bin = if has_energy_column {
            let e = tokens.first().copied().unwrap_or(f64::NAN);
            at += 1;
            match energy_order
                .iter()
                .position(|v| (*v - e).abs() <= 1e-6 * e.abs().max(1e-300))
            {
                Some(k) => k,
                None => {
                    energy_order.push(e);
                    energy_order.len() - 1
                }
            }
        } else {
            0
        };
        let need = at + 4 + usize::from(has_relative_error);
        if tokens.len() < need {
            return Err(McnpError::Malformed("short data row"));
        }
        let (x, y, z, result) = (tokens[at], tokens[at + 1], tokens[at + 2], tokens[at + 3]);
        let relative_error = if has_relative_error {
            Some(tokens[at + 4])
        } else {
            None
        };
        rows.push(MeshTallyRow {
            energy_bin,
            ix: bin_index_by_center(&x_edges, x).ok_or(McnpError::OffMesh("x"))?,
            iy: bin_index_by_center(&y_edges, y).ok_or(McnpError::OffMesh("y"))?,
            iz: bin_index_by_center(&z_edges, z).ok_or(McnpError::OffMesh("z"))?,
            result,
            relative_error,
        });
        cursor += 1;
    }
    if energy_order.len() > e_edges.len().saturating_sub(1).max(1) && e_edges.len() >= 2 {
        // More distinct energy labels than declared bins: tolerate only when
        // the file declares no energy list (treated as one bin).
        return Err(McnpError::Malformed(
            "more energy blocks than declared bins",
        ));
    }
    // Every energy bin must score the full mesh.
    let mut per_bin: BTreeMap<usize, usize> = BTreeMap::new();
    for row in &rows {
        *per_bin.entry(row.energy_bin).or_insert(0) += 1;
    }
    for (bin, count) in &per_bin {
        if *count != expected {
            return Err(McnpError::IncompleteMesh {
                energy_bin: *bin,
                expected,
                actual: *count,
            });
        }
    }
    Ok((
        MeshTally {
            number,
            particle,
            x_edges_cm: x_edges,
            y_edges_cm: y_edges,
            z_edges_cm: z_edges,
            energy_edges_mev: e_edges,
            has_energy_column,
            has_relative_error,
            rows,
        },
        cursor,
    ))
}

/// Locate the bin containing a printed center coordinate, verifying it sits
/// near the bin midpoint (meshtal prints centers, not indices).
fn bin_index_by_center(edges: &[f64], center: f64) -> Option<usize> {
    for i in 0..edges.len() - 1 {
        let (lo, hi) = (edges[i], edges[i + 1]);
        if center >= lo - f64::EPSILON && center <= hi + f64::EPSILON {
            let mid = 0.5 * (lo + hi);
            let width = hi - lo;
            if (center - mid).abs() <= 0.01 * width + 1e-12 {
                return Some(i);
            }
            return None;
        }
    }
    None
}

/// Derive a uniform-spacing [`GridGeometry`] (in mm, voxel-center origin)
/// from meshtal edge lists (cm).
pub fn tally_geometry(tally: &MeshTally) -> Result<GridGeometry, McnpError> {
    let axis = |edges: &[f64]| -> Result<(u32, f64, f64), McnpError> {
        let n = edges.len() - 1;
        let width = edges[1] - edges[0];
        if width <= 0.0 {
            return Err(McnpError::NonUniformMesh);
        }
        for pair in edges.windows(2) {
            let w = pair[1] - pair[0];
            if (w - width).abs() > 1e-6 * width {
                return Err(McnpError::NonUniformMesh);
            }
        }
        Ok((n as u32, width * 10.0, 0.5 * (edges[0] + edges[1]) * 10.0))
    };
    let (nx, dx, ox) = axis(&tally.x_edges_cm)?;
    let (ny, dy, oy) = axis(&tally.y_edges_cm)?;
    let (nz, dz, oz) = axis(&tally.z_edges_cm)?;
    Ok(GridGeometry {
        shape: [nx, ny, nz],
        spacing_mm: [dx, dy, dz],
        origin_mm: [ox, oy, oz],
        direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    })
}

/// Extract one component's dose volume from a parsed tally.
pub fn tally_dose_volume(
    tally: &MeshTally,
    energy_bin: Option<usize>,
    component: DoseComponent,
    unit: DoseUnit,
) -> Result<DoseVolume, McnpError> {
    let bins = tally.energy_bin_count().max(1);
    let ebin = match (energy_bin, bins) {
        (Some(k), n) if k < n => k,
        (Some(k), n) => return Err(McnpError::EnergyBinOutOfRange { bin: k, bins: n }),
        (None, 1) => 0,
        (None, n) => return Err(McnpError::EnergyBinRequired { bins: n }),
    };
    let (nx, ny, nz) = (tally.nx(), tally.ny(), tally.nz());
    let mut values = vec![0.0_f64; nx * ny * nz];
    let mut sigmas = tally
        .has_relative_error
        .then(|| vec![0.0_f64; nx * ny * nz]);
    let mut seen = vec![false; nx * ny * nz];
    for row in tally.rows.iter().filter(|r| r.energy_bin == ebin) {
        let index = row.ix + nx * row.iy + nx * ny * row.iz;
        values[index] = row.result;
        if let (Some(sig), Some(rel)) = (&mut sigmas, row.relative_error) {
            sig[index] = row.result.abs() * rel;
        }
        seen[index] = true;
    }
    if seen.iter().any(|s| !s) {
        return Err(McnpError::IncompleteMesh {
            energy_bin: ebin,
            expected: nx * ny * nz,
            actual: seen.iter().filter(|s| **s).count(),
        });
    }
    Ok(DoseVolume {
        component,
        unit,
        values,
        absolute_standard_uncertainty: sigmas,
    })
}

/// Build an interchange document from meshtal component selections.
///
/// `sources` must cover each of the four required components exactly once
/// (the interchange validator also enforces this). All selected tallies must
/// share one mesh. `normalization` is caller-declared dose semantics (kerma
/// factors, response folding, normalization basis); the adapter appends the
/// parsed tally map and history count. `producer_version` overrides the
/// parsed `mcnp version` token when present — a mismatch with a parsed token
/// is rejected rather than silently preferred.
pub fn interchange_from_meshtals(
    sources: &[ComponentSource],
    case_id: &str,
    unit: DoseUnit,
    normalization: &str,
    frame_of_reference_uid: Option<String>,
    producer_version: Option<String>,
) -> Result<ComponentDoseInterchange, McnpError> {
    if sources.len() != 4 {
        return Err(McnpError::ComponentCount(sources.len()));
    }
    let mut files: BTreeMap<PathBuf, MeshtalFile> = BTreeMap::new();
    let mut components = Vec::new();
    let mut geometry = None;
    let mut tally_map = Vec::new();
    for source in sources {
        if !files.contains_key(&source.file) {
            let text = fs::read_to_string(&source.file).map_err(|e| McnpError::Io {
                path: source.file.clone(),
                source: e,
            })?;
            files.insert(source.file.clone(), parse_meshtal(&text)?);
        }
        let file = &files[&source.file];
        let tally = file.tally(source.tally).ok_or(McnpError::NoSuchTally {
            tally: source.tally,
            path: source.file.clone(),
        })?;
        let tally_geometry = tally_geometry(tally)?;
        if let Some(existing) = &geometry {
            if !openbnct_core::grid_geometry_equivalent(existing, &tally_geometry) {
                return Err(McnpError::MeshDisagreement(source.component));
            }
        } else {
            geometry = Some(tally_geometry);
        }
        components.push(tally_dose_volume(
            tally,
            source.energy_bin,
            source.component,
            unit,
        )?);
        tally_map.push(format!(
            "{:?}={}:{}",
            source.component, source.tally, tally.number
        ));
    }
    let geometry = geometry.expect("four sources always produce a geometry");

    let any_file = files.values().next().expect("nonempty file map");
    let version = match (&producer_version, &any_file.code_version) {
        (Some(declared), Some(parsed)) if declared != parsed => {
            return Err(McnpError::VersionMismatch {
                declared: declared.clone(),
                parsed: parsed.clone(),
            });
        }
        (Some(declared), _) => declared.clone(),
        (None, Some(parsed)) => parsed.clone(),
        (None, None) => return Err(McnpError::VersionUnknown),
    };
    let mut norm = normalization.trim().to_owned();
    if let Some(nps) = any_file.histories {
        norm.push_str(&format!("; nps={nps:.6e}"));
    }
    norm.push_str(&format!("; meshtal tallies [{}]", tally_map.join(",")));

    Ok(ComponentDoseInterchange {
        schema_version: openbnct_core::COMPONENT_DOSE_INTERCHANGE_SCHEMA.into(),
        case_id: case_id.to_owned(),
        frame_of_reference_uid,
        geometry,
        producer: ExternalProducer {
            system: "mcnp".into(),
            version,
            normalization: norm,
        },
        components,
        component_profile: None,
        response_set: None,
        total: ExternalTotal::ComponentSum,
    })
}

#[derive(Debug, Error)]
pub enum McnpError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("file carries no mesh tally blocks")]
    NoTallies,
    #[error("malformed meshtal: {0}")]
    Malformed(&'static str),
    #[error("only rectangular (geom=xyz) meshes are supported; cylindrical/spherical rejected")]
    UnsupportedMesh,
    #[error("only column layouts (out=col/cf) with a Result column are supported")]
    UnsupportedLayout,
    #[error("row coordinate falls off the declared {0} mesh")]
    OffMesh(&'static str),
    #[error("energy bin {energy_bin} scored {actual} rows, expected {expected}")]
    IncompleteMesh {
        energy_bin: usize,
        expected: usize,
        actual: usize,
    },
    #[error("non-uniform or non-positive bin widths cannot form a GridGeometry")]
    NonUniformMesh,
    #[error("tally has {bins} energy bins; select one explicitly")]
    EnergyBinRequired { bins: usize },
    #[error("energy bin {bin} out of range (tally has {bins})")]
    EnergyBinOutOfRange { bin: usize, bins: usize },
    #[error("tally {tally} not present in {path}")]
    NoSuchTally { tally: u32, path: PathBuf },
    #[error("component {0:?} mesh disagrees with the earlier components")]
    MeshDisagreement(DoseComponent),
    #[error("exactly four component sources are required, got {0}")]
    ComponentCount(usize),
    #[error("declared producer version {declared:?} disagrees with file banner {parsed:?}")]
    VersionMismatch { declared: String, parsed: String },
    #[error("meshtal file carries no mcnp version banner; pass --producer-version")]
    VersionUnknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Layout mirrors a real MCNP6 `OUT=col` meshtal file (energy column
    // present, Z varying fastest, wrapped header fields).
    const MESHTAL: &str = "\
mcnp   version 6.2 ld=01/01/20  probid =  01/01/20 00:00:00
 synthetic test fixture
 Number of histories used for normalizing tallies =    1000000.00

 Mesh Tally Number        4
 neutron  mesh tally.

 Tally bin boundaries:
    X direction:      -1.00      1.00
    Y direction:      -1.00      0.00      1.00
    Z direction:      -1.00      0.00      1.00      2.00
    Energy bin boundaries: 0.00E+00 2.00E+01

   Energy         X         Y         Z     Result     Rel Error
  2.000E+01     0.000   -0.500    -0.500 1.00000E-03 1.00000E-02
  2.000E+01     0.000   -0.500     0.500 2.00000E-03 5.00000E-03
  2.000E+01     0.000   -0.500     1.500 3.00000E-03 2.00000E-02
  2.000E+01     0.000    0.500    -0.500 4.00000E-03 1.50000E-02
  2.000E+01     0.000    0.500     0.500 5.00000E-03 1.00000E-02
  2.000E+01     0.000    0.500     1.500 6.00000E-03 5.00000E-03

 Mesh Tally Number       14
 photon  mesh tally.

 Tally bin boundaries:
    X direction:      -1.00      1.00
    Y direction:      -1.00      0.00      1.00
    Z direction:      -1.00      0.00      1.00      2.00
    Energy bin boundaries: 0.00E+00 2.00E+01

   Energy         X         Y         Z     Result     Rel Error
  2.000E+01     0.000   -0.500    -0.500 1.00000E-04 1.00000E-01
  2.000E+01     0.000   -0.500     0.500 1.10000E-04 1.00000E-01
  2.000E+01     0.000   -0.500     1.500 1.20000E-04 1.00000E-01
  2.000E+01     0.000    0.500    -0.500 1.30000E-04 1.00000E-01
  2.000E+01     0.000    0.500     0.500 1.40000E-04 1.00000E-01
  2.000E+01     0.000    0.500     1.500 1.50000E-04 1.00000E-01
";

    // Same tally without the Energy column and without Rel Error.
    const MESHTAL_NO_E_NO_ERR: &str = "\
mcnp   version 6.2 ld=01/01/20  probid =  01/01/20 00:00:00
 synthetic test fixture
 Number of histories used for normalizing tallies =    500.00

 Mesh Tally Number        8
 neutron  mesh tally.

 Tally bin boundaries:
    X direction:      -1.00      1.00
    Y direction:      -1.00      0.00      1.00
    Z direction:      -1.00      0.00      1.00      2.00
    Energy bin boundaries: 0.00E+00 2.00E+01

            X         Y         Z     Result
        0.000   -0.500    -0.500 1.00000E-03
        0.000   -0.500     0.500 2.00000E-03
        0.000   -0.500     1.500 3.00000E-03
        0.000    0.500    -0.500 4.00000E-03
        0.000    0.500     0.500 5.00000E-03
        0.000    0.500     1.500 6.00000E-03
";

    #[test]
    fn parses_real_layout() {
        let file = parse_meshtal(MESHTAL).unwrap();
        assert_eq!(file.code_version.as_deref(), Some("6.2"));
        assert_eq!(file.histories, Some(1.0e6));
        assert_eq!(file.tallies.len(), 2);
        let tally = file.tally(4).unwrap();
        assert_eq!(tally.particle, "neutron  mesh tally");
        assert_eq!((tally.nx(), tally.ny(), tally.nz()), (1, 2, 3));
        assert!(tally.has_energy_column && tally.has_relative_error);
        assert_eq!(tally.rows.len(), 6);
        // z-fastest file order maps onto bundle indices by coordinates.
        let volume = tally_dose_volume(
            tally,
            None,
            DoseComponent::Nitrogen,
            DoseUnit::GrayPerSourceParticle,
        )
        .unwrap();
        // bundle index i + nx*j + nx*ny*k with nx=1: index = j + 2*k.
        assert_eq!(volume.values[0], 1.0e-3); // (0,0,0)
        assert_eq!(volume.values[4], 3.0e-3); // (0,0,2)
        assert_eq!(volume.values[5], 6.0e-3); // (0,1,2)
        assert_eq!(volume.values[3], 5.0e-3); // (0,1,1)
        let sigma = volume.absolute_standard_uncertainty.unwrap();
        assert!((sigma[0] - 1.0e-5).abs() < 1e-18);
    }

    #[test]
    fn geometry_from_cm_edges() {
        let file = parse_meshtal(MESHTAL).unwrap();
        let geometry = tally_geometry(file.tally(4).unwrap()).unwrap();
        assert_eq!(geometry.shape, [1, 2, 3]);
        assert_eq!(geometry.spacing_mm, [20.0, 10.0, 10.0]);
        assert_eq!(geometry.origin_mm, [0.0, -5.0, -5.0]);
    }

    #[test]
    fn single_bin_without_energy_or_error_columns() {
        let file = parse_meshtal(MESHTAL_NO_E_NO_ERR).unwrap();
        let tally = file.tally(8).unwrap();
        assert!(!tally.has_energy_column && !tally.has_relative_error);
        let volume = tally_dose_volume(
            tally,
            None,
            DoseComponent::Photon,
            DoseUnit::GrayPerSourceParticle,
        )
        .unwrap();
        assert_eq!(volume.values[1], 4.0e-3); // (0,1,0): index = j + 2*k
        assert_eq!(volume.values[3], 5.0e-3); // (0,1,1)
        assert!(volume.absolute_standard_uncertainty.is_none());
    }

    #[test]
    fn multi_energy_requires_selection() {
        // Two energy bins written as sequential blocks.
        let text = MESHTAL.replace(
            "Energy bin boundaries: 0.00E+00 2.00E+01",
            "Energy bin boundaries: 0.00E+00 1.00E-01 2.00E+01",
        );
        // Duplicate each data row under a second energy label: two energy
        // blocks written back to back.
        let mut two_bin = String::new();
        for line in text.lines() {
            two_bin.push_str(line);
            two_bin.push('\n');
            if line.starts_with("  2.000E+01") {
                let second = line.replacen("2.000E+01", "1.000E-01", 1);
                two_bin.push_str(&second);
                two_bin.push('\n');
            }
        }
        let file = parse_meshtal(&two_bin).unwrap();
        let tally = file.tally(4).unwrap();
        assert!(matches!(
            tally_dose_volume(
                tally,
                None,
                DoseComponent::Boron,
                DoseUnit::GrayPerSourceParticle
            ),
            Err(McnpError::EnergyBinRequired { bins: 2 })
        ));
        let volume = tally_dose_volume(
            tally,
            Some(0),
            DoseComponent::Boron,
            DoseUnit::GrayPerSourceParticle,
        )
        .unwrap();
        assert_eq!(volume.values[0], 1.0e-3);
    }

    #[test]
    fn rejects_bad_mesh_and_missing_tally() {
        let file = parse_meshtal(MESHTAL).unwrap();
        assert!(file.tally(99).is_none());
        // Non-uniform Z spacing (third center moves to the true midpoint).
        let text = MESHTAL
            .replace(
                "Z direction:      -1.00      0.00      1.00      2.00",
                "Z direction:      -1.00      0.00      1.00      2.50",
            )
            .replace("     1.500 ", "     1.750 ");
        let file = parse_meshtal(&text).unwrap();
        assert!(matches!(
            tally_geometry(file.tally(4).unwrap()),
            Err(McnpError::NonUniformMesh)
        ));
    }

    #[test]
    fn interchange_builder_checks_and_provenance() {
        let dir = std::env::temp_dir().join(format!("openbnct-mcnp-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("meshtal");
        std::fs::write(&path, MESHTAL).unwrap();
        let source = |component, tally| ComponentSource {
            component,
            file: path.clone(),
            tally,
            energy_bin: None,
        };
        let doc = interchange_from_meshtals(
            &[
                source(DoseComponent::Boron, 4),
                source(DoseComponent::Nitrogen, 4),
                source(DoseComponent::Hydrogen, 4),
                source(DoseComponent::Photon, 14),
            ],
            "fixture-case",
            DoseUnit::GrayPerSourceParticle,
            "per source particle; synthetic fixture",
            Some("2.25.13".into()),
            None,
        )
        .unwrap();
        assert_eq!(doc.producer.system, "mcnp");
        assert_eq!(doc.producer.version, "6.2");
        assert!(doc.producer.normalization.contains("nps=1.000000e6"));
        assert!(doc.producer.normalization.contains("Photon=14"));
        let bundle = openbnct_core::import_component_dose(&doc, &"a".repeat(64)).unwrap();
        assert_eq!(bundle.physical_total.values.len(), 6);
        // photon-only tally 14 row 0 plus neutron tally 4 row 0.
        assert!((bundle.physical_total.values[0] - (3.0 * 1.0e-3 + 1.0e-4)).abs() < 1e-18);
        // Declared version mismatching the banner is rejected.
        assert!(matches!(
            interchange_from_meshtals(
                &[
                    source(DoseComponent::Boron, 4),
                    source(DoseComponent::Nitrogen, 4),
                    source(DoseComponent::Hydrogen, 4),
                    source(DoseComponent::Photon, 14),
                ],
                "fixture-case",
                DoseUnit::GrayPerSourceParticle,
                "x",
                None,
                Some("5.0".into()),
            ),
            Err(McnpError::VersionMismatch { .. })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
