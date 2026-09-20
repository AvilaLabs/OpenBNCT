// SPDX-License-Identifier: Apache-2.0

//! PHITS xyz-mesh tally import adapter.
//!
//! Reads PHITS tally output files (`*.out`, ANGEL format) produced by
//! `[t-deposit]`/`[t-track]`-style tallies with `mesh = xyz` and a
//! two-dimensional `axis` (`xy`, `xz`, or `yz`), and lifts them into a
//! [`ComponentDoseInterchange`] document for
//! [`openbnct_core::import_component_dose`].
//!
//! Format conventions used (PHITS 3.x manual, §4.9):
//!
//! - the file begins with an input echo repeating the tally parameters
//!   (`mesh`, `x-type`/`xmin`/`xmax`/`nx`, …, `axis`, `file`, `unit`);
//! - results follow as ANGEL pages separated by `#newpage:`; each page's `#`
//!   header comments carry the fixed-axis indices (`iz = k`, `ie = k`, …);
//! - data rows give bin lower/upper bounds per varying axis plus one value
//!   column (`x-lower x-upper y-lower y-upper <value>`);
//! - for two-dimensional axes, relative errors live in a sibling
//!   `*_err.out` file with the same page layout.
//!
//! Honesty rules: rows are mapped to voxels by their printed bin bounds
//! (never by assumed ordering); `unit` codes are cross-checked against the
//! declared [`DoseUnit`]; non-uniform (`x-type != 2`), tet/r-z/reg meshes,
//! multi-value-column pages, and disagreeing meshes are rejected rather than
//! resampled or silently substituted. Missing `*_err.out` imports with no
//! claimed uncertainty.

pub mod deck;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use openbnct_core::{
    ComponentDoseInterchange, DoseComponent, DoseUnit, DoseVolume, ExternalProducer, ExternalTotal,
    GridGeometry,
};
use thiserror::Error;

/// Two-dimensional axis selection of a PHITS output file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhitsAxis {
    Xy,
    Xz,
    Yz,
}

/// Parsed xyz mesh from the tally input echo, in centimetres.
#[derive(Debug, Clone)]
pub struct PhitsMesh {
    pub x_edges_cm: Vec<f64>,
    pub y_edges_cm: Vec<f64>,
    pub z_edges_cm: Vec<f64>,
}

impl PhitsMesh {
    fn count(edges: &[f64]) -> usize {
        edges.len() - 1
    }
    pub fn shape(&self) -> [usize; 3] {
        [
            Self::count(&self.x_edges_cm),
            Self::count(&self.y_edges_cm),
            Self::count(&self.z_edges_cm),
        ]
    }
}

/// One ANGEL page: the fixed-axis indices plus its data rows.
#[derive(Debug, Clone)]
pub struct PhitsPage {
    /// Fixed indices from `#` header comments, e.g. `iz → 3`, `ie → 1`.
    /// Keys are the axis letters (`x`, `y`, `z`, `e`, `t`, …), values are
    /// PHITS's 1-based bin indices.
    pub fixed: BTreeMap<char, usize>,
    /// `(lower_a, upper_a, lower_b, upper_b, value, rel_error)` rows in page
    /// order. `rel_error` is `Some` only when the file inlines an `r.err`
    /// column (the usual two-dimensional layout keeps it in `*_err.out`).
    pub rows: Vec<(f64, f64, f64, f64, f64, Option<f64>)>,
}

/// A parsed PHITS tally output file.
#[derive(Debug, Clone)]
pub struct PhitsTallyFile {
    /// `unit =` code from the input echo, when present.
    pub unit_code: Option<u32>,
    /// Output declared `output =` mode (`dose`, `deposit`, …), when present.
    pub output_mode: Option<String>,
    /// The `axis` whose `file =` name matches this file.
    pub axis: PhitsAxis,
    pub mesh: PhitsMesh,
    pub pages: Vec<PhitsPage>,
}

/// Which PHITS file (and energy page) supplies one dose component.
#[derive(Debug, Clone)]
pub struct ComponentSource {
    pub component: DoseComponent,
    pub file: PathBuf,
    /// Required when the tally wrote more than one energy page group
    /// (`ie > 1` appears); `None` selects the sole group.
    pub energy_bin: Option<usize>,
}

fn parse_f64(token: &str) -> Option<f64> {
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn is_numeric_line(line: &str) -> bool {
    !line.trim().is_empty() && line.split_whitespace().all(|t| parse_f64(t).is_some())
}

fn numeric_tokens(line: &str) -> Vec<f64> {
    line.split_whitespace().filter_map(parse_f64).collect()
}

/// Strip a trailing `# comment` and split `key = value` echo lines.
fn echo_entry(line: &str) -> Option<(&str, &str)> {
    let (body, _) = line.split_once('#').unwrap_or((line, ""));
    let (key, value) = body.split_once('=')?;
    let key = key.trim();
    let value = value.trim();
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

fn uniform_edges(min: f64, max: f64, n: usize) -> Result<Vec<f64>, PhitsError> {
    if n == 0 || max <= min {
        return Err(PhitsError::BadEcho("degenerate mesh extent"));
    }
    let width = (max - min) / n as f64;
    Ok((0..=n).map(|i| min + width * i as f64).collect())
}

fn echo_axis(
    edges: &mut PhitsMesh,
    echo: &BTreeMap<String, Vec<String>>,
) -> Result<(), PhitsError> {
    for (letter, field) in [
        ('x', &mut edges.x_edges_cm),
        ('y', &mut edges.y_edges_cm),
        ('z', &mut edges.z_edges_cm),
    ] {
        let ty = echo
            .get(&format!("{letter}-type"))
            .and_then(|v| v.first())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(2);
        if ty != 2 {
            return Err(PhitsError::UnsupportedMeshType { axis: letter, ty });
        }
        let scalar = |name: &str| -> Result<f64, PhitsError> {
            echo.get(&format!("{letter}{name}"))
                .and_then(|v| v.first())
                .and_then(|s| s.parse::<f64>().ok())
                .ok_or(PhitsError::BadEcho("missing mesh extent"))
        };
        let n = echo
            .get(&format!("n{letter}"))
            .and_then(|v| v.first())
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or(PhitsError::BadEcho("missing mesh count"))?;
        *field = uniform_edges(scalar("min")?, scalar("max")?, n)?;
    }
    Ok(())
}

/// Parse a PHITS tally output file (ANGEL format, `mesh = xyz`).
///
/// `file_name` is the basename used to pair the `axis`/`file` echo lists when
/// a tally wrote several output files.
pub fn parse_phits_tally(text: &str, file_name: &str) -> Result<PhitsTallyFile, PhitsError> {
    let lines: Vec<&str> = text.lines().collect();
    // ---- input echo: key = value pairs before the first page marker ----
    let mut echo: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cursor = 0;
    while cursor < lines.len() && !lines[cursor].contains("newpage") {
        if let Some((key, value)) = echo_entry(lines[cursor]) {
            echo.entry(key.to_ascii_lowercase())
                .or_default()
                .push(value.to_owned());
        }
        cursor += 1;
    }
    match echo.get("mesh").and_then(|v| v.first()).map(String::as_str) {
        Some("xyz") => {}
        Some(other) => return Err(PhitsError::UnsupportedMesh(other.to_owned())),
        None => return Err(PhitsError::BadEcho("mesh not declared in echo")),
    }
    // Pair axis[i] with file[i]; choose the entry matching this file name.
    let axes: Vec<&str> = echo
        .get("axis")
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let files: Vec<&str> = echo
        .get("file")
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let axis_token = if axes.len() == 1 {
        axes[0]
    } else {
        let base = Path::new(file_name)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name);
        let mut matched = None;
        for (axis, file) in axes.iter().zip(files.iter()) {
            if Path::new(file)
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|f| f == base)
            {
                matched = Some(*axis);
            }
        }
        matched.ok_or(PhitsError::BadEcho("no axis entry names this file"))?
    };
    let axis = match axis_token {
        "xy" => PhitsAxis::Xy,
        "xz" => PhitsAxis::Xz,
        "yz" => PhitsAxis::Yz,
        other => return Err(PhitsError::UnsupportedAxis(other.to_owned())),
    };
    let mut mesh = PhitsMesh {
        x_edges_cm: Vec::new(),
        y_edges_cm: Vec::new(),
        z_edges_cm: Vec::new(),
    };
    echo_axis(&mut mesh, &echo)?;
    let unit_code = echo
        .get("unit")
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<u32>().ok());
    let output_mode = echo.get("output").and_then(|v| v.first()).cloned();

    // ---- ANGEL pages ----
    let mut pages: Vec<PhitsPage> = Vec::new();
    while cursor < lines.len() {
        // Advance to the next page marker.
        while cursor < lines.len() && !lines[cursor].contains("newpage") {
            cursor += 1;
        }
        if cursor >= lines.len() {
            break;
        }
        cursor += 1;
        // Page header: `#` comment lines carrying fixed indices, plus
        // x:/y:/p:/h: ANGEL records, until the data block begins.
        let mut fixed: BTreeMap<char, usize> = BTreeMap::new();
        let mut column_names: Vec<String> = Vec::new();
        while cursor < lines.len() {
            let line = lines[cursor];
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                let body = trimmed.trim_start_matches('#');
                // `# no. = 3 iz = 7 ie = 1` — collect i<letter> = <int> pairs.
                for token in body.split_whitespace().collect::<Vec<_>>().windows(3) {
                    if token[1] == "="
                        && token[0].len() == 2
                        && token[0].starts_with('i')
                        && token[0]
                            .chars()
                            .nth(1)
                            .is_some_and(|c| c.is_ascii_lowercase())
                        && let Ok(index) = token[2].parse::<usize>()
                    {
                        fixed.insert(token[0].chars().nth(1).unwrap(), index);
                    }
                }
                // A `#`-comment line that names bin-bound columns is the data
                // header (e.g. `# x-lower x-upper y-lower y-upper all`).
                let names: Vec<String> = body
                    .split_whitespace()
                    .map(|s| s.to_ascii_lowercase())
                    .collect();
                if names.iter().any(|n| n.ends_with("-lower")) {
                    column_names = names;
                    cursor += 1;
                    break;
                }
                cursor += 1;
                continue;
            }
            if is_numeric_line(line) {
                break;
            }
            cursor += 1;
        }
        // Column roles: two lower/upper bound pairs for the varying axes and
        // exactly one value column.
        if column_names.is_empty() {
            // Page without data (gshow-only, or trailing markers): skip.
            while cursor < lines.len() && !lines[cursor].contains("newpage") {
                cursor += 1;
            }
            continue;
        }
        let varying: Vec<char> = match axis {
            PhitsAxis::Xy => vec!['x', 'y'],
            PhitsAxis::Xz => vec!['x', 'z'],
            PhitsAxis::Yz => vec!['y', 'z'],
        };
        let mut bounds_pos = [usize::MAX; 4]; // lo_a, hi_a, lo_b, hi_b
        let mut value_pos = None;
        let mut r_err_pos = None;
        for (pos, name) in column_names.iter().enumerate() {
            if let Some(axis_letter) = name.strip_suffix("-lower").and_then(|s| s.chars().next()) {
                if axis_letter == varying[0] {
                    bounds_pos[0] = pos;
                } else if axis_letter == varying[1] {
                    bounds_pos[2] = pos;
                }
            } else if let Some(axis_letter) =
                name.strip_suffix("-upper").and_then(|s| s.chars().next())
            {
                if axis_letter == varying[0] {
                    bounds_pos[1] = pos;
                } else if axis_letter == varying[1] {
                    bounds_pos[3] = pos;
                }
            } else if name == "r.err" {
                r_err_pos = Some(pos);
            } else {
                if value_pos.is_some() {
                    return Err(PhitsError::MultiColumnPage);
                }
                value_pos = Some(pos);
            }
        }
        // In a `*_err` sibling the sole payload column is `r.err` itself.
        let value_pos = value_pos
            .or(r_err_pos)
            .ok_or(PhitsError::BadEcho("page has no value column"))?;
        if bounds_pos.contains(&usize::MAX) {
            return Err(PhitsError::BadEcho("page missing bound columns"));
        }
        let mut rows = Vec::new();
        while cursor < lines.len() {
            let line = lines[cursor];
            if line.contains("newpage") || line.trim_start().starts_with("# gshow") {
                break;
            }
            if is_numeric_line(line) {
                let t = numeric_tokens(line);
                let positions = [
                    bounds_pos[0],
                    bounds_pos[1],
                    bounds_pos[2],
                    bounds_pos[3],
                    value_pos,
                ];
                let need = *positions.iter().chain(r_err_pos.iter()).max().unwrap() + 1;
                if t.len() < need {
                    return Err(PhitsError::Malformed("short data row"));
                }
                rows.push((
                    t[bounds_pos[0]],
                    t[bounds_pos[1]],
                    t[bounds_pos[2]],
                    t[bounds_pos[3]],
                    t[value_pos],
                    r_err_pos.map(|p| t[p]),
                ));
                cursor += 1;
                continue;
            }
            if line.trim().is_empty() {
                cursor += 1;
                continue;
            }
            break;
        }
        pages.push(PhitsPage { fixed, rows });
    }
    if pages.is_empty() {
        return Err(PhitsError::NoPages);
    }
    Ok(PhitsTallyFile {
        unit_code,
        output_mode,
        axis,
        mesh,
        pages,
    })
}

/// The sibling `*_err.out` path PHITS writes for a two-dimensional axis file.
pub fn error_file_path(file: &Path) -> PathBuf {
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let name = format!("{stem}_err");
    match file.extension().and_then(|s| s.to_str()) {
        Some(ext) => file.with_file_name(name).with_extension(ext),
        None => file.with_file_name(name),
    }
}

fn edges_index(edges: &[f64], lo: f64, hi: f64, axis: char) -> Result<usize, PhitsError> {
    for i in 0..edges.len() - 1 {
        let tol = 5e-3 * (edges[i + 1] - edges[i]).abs().max(1e-12);
        if (edges[i] - lo).abs() <= tol && (edges[i + 1] - hi).abs() <= tol {
            return Ok(i);
        }
    }
    Err(PhitsError::OffMesh(axis))
}

/// Extract one component's dose volume from a parsed PHITS file.
fn tally_dose_volume(
    tally: &PhitsTallyFile,
    errors: Option<&PhitsTallyFile>,
    energy_bin: Option<usize>,
    component: DoseComponent,
    unit: DoseUnit,
) -> Result<DoseVolume, PhitsError> {
    let [nx, ny, nz] = tally.mesh.shape();
    let fixed_letter = match tally.axis {
        PhitsAxis::Xy => 'z',
        PhitsAxis::Xz => 'y',
        PhitsAxis::Yz => 'x',
    };
    // Energy pages: distinct `ie` values across pages.
    let energy_groups: Vec<usize> = tally
        .pages
        .iter()
        .filter_map(|p| p.fixed.get(&'e').copied())
        .collect::<std::collections::BTreeSet<usize>>()
        .into_iter()
        .collect();
    let ebin = match (energy_bin, energy_groups.len()) {
        (Some(k), _) if energy_groups.is_empty() && k == 0 => None,
        (Some(k), n) if k < n => Some(energy_groups[k]),
        (Some(k), n) => return Err(PhitsError::EnergyBinOutOfRange { bin: k, bins: n }),
        (None, 0) => None,
        (None, 1) => Some(energy_groups[0]),
        (None, n) => return Err(PhitsError::EnergyBinRequired { bins: n }),
    };

    let mut values = vec![f64::NAN; nx * ny * nz];
    // Inline `r.err` columns win; the `*_err` sibling fills in only when the
    // value file carries none.
    let has_inline_err = tally
        .pages
        .iter()
        .flat_map(|p| &p.rows)
        .any(|row| row.5.is_some());
    let err_file = if has_inline_err { None } else { errors };
    let mut sigmas = (has_inline_err || err_file.is_some()).then(|| vec![f64::NAN; nx * ny * nz]);
    for page in &tally.pages {
        if let Some(want) = ebin
            && page.fixed.get(&'e').copied() != Some(want)
        {
            continue;
        }
        let fixed_index = page
            .fixed
            .get(&fixed_letter)
            .copied()
            .ok_or(PhitsError::MissingPageIndex(fixed_letter))?;
        if fixed_index == 0 {
            return Err(PhitsError::Malformed("page indices are 1-based"));
        }
        let fixed_bin = fixed_index - 1;
        if fixed_bin
            >= match fixed_letter {
                'x' => nx,
                'y' => ny,
                _ => nz,
            }
        {
            return Err(PhitsError::OffMesh(fixed_letter));
        }
        for &(lo_a, hi_a, lo_b, hi_b, value, rel_err) in &page.rows {
            let (ix, iy, iz) = voxel_index(tally, lo_a, hi_a, lo_b, hi_b, page)?;
            let index = ix + nx * iy + nx * ny * iz;
            values[index] = value;
            if let (Some(sig), Some(rel)) = (&mut sigmas, rel_err) {
                sig[index] = value.abs() * rel;
            }
        }
    }
    if values.iter().any(|v| !v.is_finite()) {
        return Err(PhitsError::IncompleteMesh);
    }
    // Relative-error sibling: same page/row layout, value column = r.err.
    if let (Some(err_file), Some(sig)) = (err_file, sigmas.as_mut()) {
        let mut matched = vec![false; nx * ny * nz];
        for (page, err_page) in tally.pages.iter().zip(&err_file.pages) {
            if let Some(want) = ebin
                && page.fixed.get(&'e').copied() != Some(want)
            {
                continue;
            }
            if err_page.fixed != page.fixed {
                return Err(PhitsError::ErrFileMismatch);
            }
            if err_page.rows.len() != page.rows.len() {
                return Err(PhitsError::ErrFileMismatch);
            }
            for (row, err_row) in page.rows.iter().zip(&err_page.rows) {
                let (ix, iy, iz) = voxel_index(tally, row.0, row.1, row.2, row.3, page)?;
                let index = ix + nx * iy + nx * ny * iz;
                sig[index] = row.4.abs() * err_row.4;
                matched[index] = true;
            }
        }
        if matched.iter().any(|m| !m) {
            return Err(PhitsError::ErrFileMismatch);
        }
    }
    if let Some(sig) = &sigmas
        && sig.iter().any(|v| !v.is_finite())
    {
        // Partial error coverage cannot honestly stand in for the whole.
        return Err(PhitsError::ErrFileMismatch);
    }
    Ok(DoseVolume {
        component,
        unit,
        values,
        absolute_standard_uncertainty: sigmas,
    })
}

fn voxel_index(
    tally: &PhitsTallyFile,
    lo_a: f64,
    hi_a: f64,
    lo_b: f64,
    hi_b: f64,
    page: &PhitsPage,
) -> Result<(usize, usize, usize), PhitsError> {
    let (varying, fixed_letter) = match tally.axis {
        PhitsAxis::Xy => (('x', 'y'), 'z'),
        PhitsAxis::Xz => (('x', 'z'), 'y'),
        PhitsAxis::Yz => (('y', 'z'), 'x'),
    };
    let edges_a = match varying.0 {
        'x' => &tally.mesh.x_edges_cm,
        'y' => &tally.mesh.y_edges_cm,
        _ => &tally.mesh.z_edges_cm,
    };
    let edges_b = match varying.1 {
        'x' => &tally.mesh.x_edges_cm,
        'y' => &tally.mesh.y_edges_cm,
        _ => &tally.mesh.z_edges_cm,
    };
    let ia = edges_index(edges_a, lo_a, hi_a, varying.0)?;
    let ib = edges_index(edges_b, lo_b, hi_b, varying.1)?;
    let fixed_bin = page
        .fixed
        .get(&fixed_letter)
        .copied()
        .ok_or(PhitsError::MissingPageIndex(fixed_letter))?
        - 1;
    Ok(match tally.axis {
        PhitsAxis::Xy => (ia, ib, fixed_bin),
        PhitsAxis::Xz => (ia, fixed_bin, ib),
        PhitsAxis::Yz => (fixed_bin, ia, ib),
    })
}

/// Derive the uniform [`GridGeometry`] (mm, voxel-center origin) from the
/// echo's cm mesh.
pub fn tally_geometry(mesh: &PhitsMesh) -> Result<GridGeometry, PhitsError> {
    let axis = |edges: &[f64]| -> Result<(u32, f64, f64), PhitsError> {
        if edges.len() < 2 {
            return Err(PhitsError::BadEcho("empty mesh axis"));
        }
        let width = edges[1] - edges[0];
        if width <= 0.0 {
            return Err(PhitsError::BadEcho("non-positive mesh width"));
        }
        for pair in edges.windows(2) {
            let w = pair[1] - pair[0];
            if (w - width).abs() > 1e-6 * width {
                return Err(PhitsError::BadEcho("non-uniform mesh"));
            }
        }
        Ok((
            (edges.len() - 1) as u32,
            width * 10.0,
            0.5 * (edges[0] + edges[1]) * 10.0,
        ))
    };
    let (nx, dx, ox) = axis(&mesh.x_edges_cm)?;
    let (ny, dy, oy) = axis(&mesh.y_edges_cm)?;
    let (nz, dz, oz) = axis(&mesh.z_edges_cm)?;
    Ok(GridGeometry {
        shape: [nx, ny, nz],
        spacing_mm: [dx, dy, dz],
        origin_mm: [ox, oy, oz],
        direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    })
}

/// PHITS `unit` codes for `[t-deposit]` dose output and the interchange unit
/// they may supply. Anything else is a mismatch the caller must resolve.
fn unit_compatible(code: u32, unit: DoseUnit) -> bool {
    // unit=0 → Dose [Gy/source]; only that maps onto a per-source-particle
    // gray unit. Other codes are MeV-based quantities, not gray.
    code == 0 && unit == DoseUnit::GrayPerSourceParticle
}

/// Build an interchange document from PHITS component selections.
///
/// Each source names one PHITS output file; the `*_err` sibling is used when
/// present. All selected tallies must share one mesh and a unit code
/// compatible with the declared `unit`. `producer_version` is required —
/// PHITS tally files do not reliably record the code version. `normalization`
/// is caller-declared dose semantics; the adapter appends the file map and
/// unit codes.
pub fn interchange_from_phits(
    sources: &[ComponentSource],
    case_id: &str,
    unit: DoseUnit,
    normalization: &str,
    frame_of_reference_uid: Option<String>,
    producer_version: &str,
) -> Result<ComponentDoseInterchange, PhitsError> {
    if producer_version.trim().is_empty() {
        return Err(PhitsError::VersionRequired);
    }
    if sources.len() != 4 {
        return Err(PhitsError::ComponentCount(sources.len()));
    }
    let mut components = Vec::new();
    let mut geometry = None;
    let mut file_map = Vec::new();
    for source in sources {
        let text = fs::read_to_string(&source.file).map_err(|e| PhitsError::Io {
            path: source.file.clone(),
            source: e,
        })?;
        let name = source
            .file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let tally = parse_phits_tally(&text, name)?;
        if let Some(code) = tally.unit_code
            && !unit_compatible(code, unit)
        {
            return Err(PhitsError::UnitMismatch {
                code,
                declared: unit,
            });
        }
        let err_path = error_file_path(&source.file);
        let errors = if err_path.exists() {
            let text = fs::read_to_string(&err_path).map_err(|e| PhitsError::Io {
                path: err_path.clone(),
                source: e,
            })?;
            Some(parse_phits_tally(&text, name)?)
        } else {
            None
        };
        let this_geometry = tally_geometry(&tally.mesh)?;
        if let Some(existing) = &geometry {
            if !openbnct_core::grid_geometry_equivalent(existing, &this_geometry) {
                return Err(PhitsError::MeshDisagreement(source.component));
            }
        } else {
            geometry = Some(this_geometry);
        }
        components.push(tally_dose_volume(
            &tally,
            errors.as_ref(),
            source.energy_bin,
            source.component,
            unit,
        )?);
        file_map.push(format!("{:?}={}", source.component, source.file.display()));
    }
    let geometry = geometry.expect("four sources always produce a geometry");
    let mut norm = normalization.trim().to_owned();
    norm.push_str(&format!("; phits .out files [{}]", file_map.join(",")));
    Ok(ComponentDoseInterchange {
        schema_version: openbnct_core::COMPONENT_DOSE_INTERCHANGE_SCHEMA.into(),
        case_id: case_id.to_owned(),
        frame_of_reference_uid,
        geometry,
        producer: ExternalProducer {
            system: "phits".into(),
            version: producer_version.trim().to_owned(),
            normalization: norm,
        },
        components,
        component_profile: None,
        response_set: None,
        total: ExternalTotal::ComponentSum,
    })
}

#[derive(Debug, Error)]
pub enum PhitsError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("malformed PHITS output: {0}")]
    Malformed(&'static str),
    #[error("incomplete input echo: {0}")]
    BadEcho(&'static str),
    #[error("only mesh=xyz is supported, found mesh={0:?}")]
    UnsupportedMesh(String),
    #[error("only linear uniform meshes ({axis}-type=2) are supported, found {axis}-type={ty}")]
    UnsupportedMeshType { axis: char, ty: u32 },
    #[error("only two-dimensional axes (xy/xz/yz) are supported, found axis={0:?}")]
    UnsupportedAxis(String),
    #[error("no ANGEL pages found")]
    NoPages,
    #[error("page carries more than one value column (multi-part output is unsupported)")]
    MultiColumnPage,
    #[error("row bounds fall off the declared {0} mesh")]
    OffMesh(char),
    #[error("page is missing the fixed {0} index")]
    MissingPageIndex(char),
    #[error("tally has {bins} energy page groups; select one with an energy index")]
    EnergyBinRequired { bins: usize },
    #[error("energy index {bin} out of range (file has {bins})")]
    EnergyBinOutOfRange { bin: usize, bins: usize },
    #[error("not every mesh voxel received a value")]
    IncompleteMesh,
    #[error("`*_err` file layout does not match the value file")]
    ErrFileMismatch,
    #[error("file unit code {code} cannot supply declared unit {declared:?}")]
    UnitMismatch { code: u32, declared: DoseUnit },
    #[error("component {0:?} mesh disagrees with the earlier components")]
    MeshDisagreement(DoseComponent),
    #[error("exactly four component sources are required, got {0}")]
    ComponentCount(usize),
    #[error("PHITS files do not record a code version; pass --producer-version")]
    VersionRequired,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Layout follows the PHITS 3.x manual (§4.9): input echo, then ANGEL
    // pages separated by `#newpage:`, each an x-y slice at fixed `iz`.
    const DEPOSIT: &str = "\
[ T - D e p o s i t ]
   title = Energy deposition in xyz mesh
    mesh =  xyz            # mesh type is xyz scoring mesh
  x-type =    2            # x-mesh is linear given by xmin, xmax and nx
    xmin =  -1.0
    xmax =   1.0
      nx =    2
  y-type =    2
    ymin =  -1.0
    ymax =   1.0
      ny =    2
  z-type =    2
    zmin =   0.0
    zmax =   2.0
      nz =    2
    unit =    0            # unit is [Gy/source]
  2D-type =    3
    axis =   xy            # axis of output
    file = deposit.out     # file name of output for the above axis
material =  all
   output =  dose          # only heat is written

#------------------------------------------------------------------------------
#newpage:
# no. = 1  iz = 1
x: x [cm]
y: y [cm]
p: xlin ylog afac(0.8) form(0.9)
h: x n n y n y(all),hh0l n
# x-lower      x-upper      y-lower      y-upper      all
 -1.0000E+00   0.0000E+00  -1.0000E+00   0.0000E+00   1.0000E-03
  0.0000E+00   1.0000E+00  -1.0000E+00   0.0000E+00   2.0000E-03
 -1.0000E+00   0.0000E+00   0.0000E+00   1.0000E+00   4.0000E-03
  0.0000E+00   1.0000E+00   0.0000E+00   1.0000E+00   5.0000E-03

#newpage:
# no. = 2  iz = 2
x: x [cm]
y: y [cm]
p: xlin ylog afac(0.8) form(0.9)
h: x n n y n y(all),hh0l n
# x-lower      x-upper      y-lower      y-upper      all
 -1.0000E+00   0.0000E+00  -1.0000E+00   0.0000E+00   7.0000E-03
  0.0000E+00   1.0000E+00  -1.0000E+00   0.0000E+00   8.0000E-03
 -1.0000E+00   0.0000E+00   0.0000E+00   1.0000E+00   9.0000E-03
  0.0000E+00   1.0000E+00   0.0000E+00   1.0000E+00   1.0000E-02
";

    // Sibling deposit_err.out: same pages, value column = r.err.
    const DEPOSIT_ERR: &str = "\
[ T - D e p o s i t ]
    mesh =  xyz
  x-type =    2
    xmin =  -1.0
    xmax =   1.0
      nx =    2
  y-type =    2
    ymin =  -1.0
    ymax =   1.0
      ny =    2
  z-type =    2
    zmin =   0.0
    zmax =   2.0
      nz =    2
    unit =    0
    axis =   xy
    file = deposit_err.out
   output =  dose

#newpage:
# no. = 1  iz = 1
x: x [cm]
y: y [cm]
h: x n n y n y(all),hh0l n
# x-lower      x-upper      y-lower      y-upper      r.err
 -1.0000E+00   0.0000E+00  -1.0000E+00   0.0000E+00   1.0000E-02
  0.0000E+00   1.0000E+00  -1.0000E+00   0.0000E+00   1.0000E-02
 -1.0000E+00   0.0000E+00   0.0000E+00   1.0000E+00   1.0000E-02
  0.0000E+00   1.0000E+00   0.0000E+00   1.0000E+00   1.0000E-02

#newpage:
# no. = 2  iz = 2
x: x [cm]
y: y [cm]
h: x n n y n y(all),hh0l n
# x-lower      x-upper      y-lower      y-upper      r.err
 -1.0000E+00   0.0000E+00  -1.0000E+00   0.0000E+00   2.0000E-02
  0.0000E+00   1.0000E+00  -1.0000E+00   0.0000E+00   2.0000E-02
 -1.0000E+00   0.0000E+00   0.0000E+00   1.0000E+00   2.0000E-02
  0.0000E+00   1.0000E+00   0.0000E+00   1.0000E+00   2.0000E-02
";

    #[test]
    fn parses_echo_and_pages() {
        let file = parse_phits_tally(DEPOSIT, "deposit.out").unwrap();
        assert_eq!(file.unit_code, Some(0));
        assert_eq!(file.axis, PhitsAxis::Xy);
        assert_eq!(file.mesh.shape(), [2, 2, 2]);
        assert_eq!(file.pages.len(), 2);
        assert_eq!(file.pages[1].fixed.get(&'z'), Some(&2));
        let geometry = tally_geometry(&file.mesh).unwrap();
        assert_eq!(geometry.shape, [2, 2, 2]);
        assert_eq!(geometry.spacing_mm, [10.0, 10.0, 10.0]);
        // origin_mm is the voxel-0 center in mm.
        assert_eq!(geometry.origin_mm, [-5.0, -5.0, 5.0]);
    }

    #[test]
    fn volume_maps_rows_by_bounds_not_order() {
        let file = parse_phits_tally(DEPOSIT, "deposit.out").unwrap();
        let volume = tally_dose_volume(
            &file,
            None,
            None,
            DoseComponent::Hydrogen,
            DoseUnit::GrayPerSourceParticle,
        )
        .unwrap();
        // index = ix + nx*iy + nx*ny*iz, nx=ny=2.
        assert_eq!(volume.values[0], 1.0e-3); // (0,0,0)
        assert_eq!(volume.values[1], 2.0e-3); // (1,0,0)
        assert_eq!(volume.values[2], 4.0e-3); // (0,1,0)
        assert_eq!(volume.values[7], 1.0e-2); // (1,1,1)
        assert!(volume.absolute_standard_uncertainty.is_none());
    }

    #[test]
    fn err_sibling_supplies_sigmas() {
        let dir = std::env::temp_dir().join(format!("openbnct-phits-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("deposit.out"), DEPOSIT).unwrap();
        std::fs::write(dir.join("deposit_err.out"), DEPOSIT_ERR).unwrap();
        let source = |component| ComponentSource {
            component,
            file: dir.join("deposit.out"),
            energy_bin: None,
        };
        let doc = interchange_from_phits(
            &[
                source(DoseComponent::Boron),
                source(DoseComponent::Nitrogen),
                source(DoseComponent::Hydrogen),
                source(DoseComponent::Photon),
            ],
            "phits-fixture",
            DoseUnit::GrayPerSourceParticle,
            "per source particle; synthetic fixture",
            None,
            "3.34",
        )
        .unwrap();
        assert_eq!(doc.producer.system, "phits");
        let hydrogen = &doc.components[2];
        let sigma = hydrogen.absolute_standard_uncertainty.as_ref().unwrap();
        assert!((sigma[0] - 1.0e-5).abs() < 1e-18); // 1e-3 * 1e-2
        assert!((sigma[7] - 2.0e-4).abs() < 1e-18); // 1e-2 * 2e-2
        let bundle = openbnct_core::import_component_dose(&doc, &"b".repeat(64)).unwrap();
        assert_eq!(bundle.physical_total.values.len(), 8);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unit_mismatch_and_missing_version_rejected() {
        let dir = std::env::temp_dir().join(format!("openbnct-phits-u-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("d.out"), DEPOSIT).unwrap();
        let source = |component| ComponentSource {
            component,
            file: dir.join("d.out"),
            energy_bin: None,
        };
        // unit=0 is Gy/source; declaring absolute gray is a mismatch.
        assert!(matches!(
            interchange_from_phits(
                &[
                    source(DoseComponent::Boron),
                    source(DoseComponent::Nitrogen),
                    source(DoseComponent::Hydrogen),
                    source(DoseComponent::Photon),
                ],
                "c",
                DoseUnit::Gray,
                "x",
                None,
                "3.34",
            ),
            Err(PhitsError::UnitMismatch { code: 0, .. })
        ));
        assert!(matches!(
            interchange_from_phits(
                &[
                    source(DoseComponent::Boron),
                    source(DoseComponent::Nitrogen),
                    source(DoseComponent::Hydrogen),
                    source(DoseComponent::Photon),
                ],
                "c",
                DoseUnit::GrayPerSourceParticle,
                "x",
                None,
                "  ",
            ),
            Err(PhitsError::VersionRequired)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
