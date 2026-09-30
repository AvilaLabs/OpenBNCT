// SPDX-License-Identifier: MIT

//! Binned phase-space beam sources (`openbnct.phase-space-source/0.1.0`).
//!
//! An IAEA phase-space file (see [`crate::phsp`]) is reduced, in one
//! streaming pass, to a table of neutron weight per source plane pixel ×
//! direction bin × energy group. The deterministic uncollided-beam
//! ray-trace consumes the table; the OpenMC deck is fed the ORIGINAL
//! accepted particles through the same [`PhaseSpaceSelection`], so both
//! codes transport the identical particle set.
//!
//! Frame. The phase-space file's +z axis is the beam axis. `--direction`
//! (`+z`, `-x`, ...) maps it onto a case axis by a proper rotation: the
//! beam travels along that direction and enters through the grid face on
//! the opposite side. The file's x = y = 0 axis is placed at
//! `center_uv_cm` on that face (default: the face centre, as `beam bind`
//! centres a port). Particles are advanced in vacuum to the source plane
//! (the face, moved inward by the `beam bind` margin).
//!
//! Direction bins are uniform in cosθ (equal solid angle) × uniform in φ,
//! about the inward plane normal, out to the file's largest polar angle;
//! each bin carries the weighted mean direction and mean 1/|cosθ| of its
//! members. Energy is histogrammed straight onto the multigroup data's
//! groups, the convention `SourceWeighting::UniformInBin` gives an analytic
//! spectrum (an actual particle energy lands in exactly one group).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use openbnct_core::GridGeometry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::model::{
    AngularDistribution, EnergyDistribution, FixedSourceDefinition, ParticleType, PlaneAxis,
    SourceSpatialDistribution,
};
use crate::multigroup::MultigroupData;
use crate::phsp::{PHSP_NEUTRON, PHSP_PHOTON, PhspError, PhspReader, PhspRecord};

pub const PHASE_SPACE_SOURCE_SCHEMA: &str = "openbnct.phase-space-source/0.1.0";

/// How far inside the grid face the source plane sits — the `beam bind`
/// convention (particles born exactly on a vacuum boundary are lost).
pub const PHASE_SPACE_PLANE_MARGIN_CM: f64 = 1.0e-6;

/// Cap on `pixels x direction bins`: the solver keeps one `u32` per cell.
const MAX_PIXEL_DIRECTION_CELLS: u64 = 1 << 25;

#[derive(Debug, Error)]
pub enum PhaseSpaceError {
    #[error(transparent)]
    Phsp(#[from] PhspError),
    #[error("phase-space source: {0}")]
    Invalid(String),
    #[error("phase-space source I/O on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

fn invalid<T>(message: impl Into<String>) -> Result<T, PhaseSpaceError> {
    Err(PhaseSpaceError::Invalid(message.into()))
}

/// Counts of neutron records not used, by reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectCounts {
    pub non_finite_or_nonpositive_weight: u64,
    /// Direction cosine along the inward normal <= 0.
    pub backward: u64,
    /// Inward cosine below the declared minimum (grazing).
    pub grazing: u64,
    /// Already beyond the source plane along its direction of travel.
    pub past_plane: u64,
    /// Plane crossing outside the grid face.
    pub outside_face: u64,
    /// Energy outside the multigroup structure.
    pub outside_energy: u64,
    /// Polar angle beyond a declared `theta_max`.
    pub outside_direction_range: u64,
}

impl RejectCounts {
    #[must_use]
    pub fn total(&self) -> u64 {
        self.non_finite_or_nonpositive_weight
            + self.backward
            + self.grazing
            + self.past_plane
            + self.outside_face
            + self.outside_energy
            + self.outside_direction_range
    }
}

/// Everything needed to decide, identically for the deterministic table
/// and the OpenMC source file, whether and where a record starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseSpaceSelection {
    /// Beam direction the file's +z maps onto: `+x|-x|+y|-y|+z|-z`.
    pub beam_direction: String,
    /// Source-plane normal axis.
    pub plane_axis: PlaneAxis,
    /// +1 when the beam travels toward +axis (enters the low face).
    pub inward_sign: i8,
    /// World coordinate of the source plane along `plane_axis`, cm.
    pub plane_offset_cm: f64,
    /// Where the file's x = y = 0 axis crosses the plane, canonical (u, v), cm.
    pub center_uv_cm: [f64; 2],
    /// File z (cm) that corresponds to the plane; particles are advanced
    /// in vacuum from their own z to it.
    pub reference_z_cm: f64,
    /// Grid-face extent in canonical (u, v), cm; crossings outside are rejected.
    pub face_u_range_cm: [f64; 2],
    pub face_v_range_cm: [f64; 2],
    /// Minimum inward cosine (grazing cut).
    pub min_cosine: f64,
    /// Accepted neutron energy window (the multigroup structure), eV.
    pub energy_range_ev: [f64; 2],
}

/// A record that passed selection, in the case frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcceptedParticle {
    /// Crossing of the source plane, canonical (u, v), cm.
    pub uv_cm: [f64; 2],
    pub direction: [f64; 3],
    /// Cosine between `direction` and the inward normal, in (0, 1].
    pub inward_cosine: f64,
    pub energy_ev: f64,
    pub weight: f64,
}

/// Why a record was not used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    NonFinite,
    Backward,
    Grazing,
    PastPlane,
    OutsideFace,
    OutsideEnergy,
}

impl RejectCounts {
    fn bump(&mut self, why: Rejection) {
        match why {
            Rejection::NonFinite => self.non_finite_or_nonpositive_weight += 1,
            Rejection::Backward => self.backward += 1,
            Rejection::Grazing => self.grazing += 1,
            Rejection::PastPlane => self.past_plane += 1,
            Rejection::OutsideFace => self.outside_face += 1,
            Rejection::OutsideEnergy => self.outside_energy += 1,
        }
    }
}

/// Parse `+x|-x|+y|-y|+z|-z` into (axis, sign).
pub fn parse_beam_direction(text: &str) -> Result<(PlaneAxis, i8), PhaseSpaceError> {
    let t = text.trim().to_ascii_lowercase();
    let mut chars = t.chars();
    let sign = match chars.next() {
        Some('+') => 1,
        Some('-') => -1,
        _ => return invalid(format!("beam direction {text:?} must look like +z or -x")),
    };
    let axis = match chars.as_str() {
        "x" => PlaneAxis::X,
        "y" => PlaneAxis::Y,
        "z" => PlaneAxis::Z,
        _ => return invalid(format!("beam direction {text:?} must look like +z or -x")),
    };
    Ok((axis, sign))
}

/// Case-axis index that file axis 0, 1, 2 (x, y, z) maps onto for a beam
/// along `axis` — a cyclic (proper) permutation.
fn axis_permutation(axis: PlaneAxis) -> [usize; 3] {
    match axis {
        PlaneAxis::Z => [0, 1, 2],
        PlaneAxis::X => [1, 2, 0],
        PlaneAxis::Y => [2, 0, 1],
    }
}

impl PhaseSpaceSelection {
    /// Rotate a file-frame vector into the case frame.
    fn to_case(&self, v: [f64; 3]) -> [f64; 3] {
        let mut p = v;
        if self.inward_sign < 0 {
            // 180 degrees about the file y axis reverses z (and x).
            p = [-v[0], v[1], -v[2]];
        }
        let perm = axis_permutation(self.plane_axis);
        let mut out = [0.0; 3];
        for k in 0..3 {
            out[perm[k]] = p[k];
        }
        out
    }

    /// Classify one record (any particle type): the crossing of the plane
    /// and the case-frame direction, or the reason it is unusable.
    pub fn classify(&self, record: &PhspRecord) -> Result<AcceptedParticle, Rejection> {
        let finite = record.position_cm.iter().all(|v| v.is_finite())
            && record.direction.iter().all(|v| v.is_finite())
            && record.energy_mev.is_finite();
        if !finite || !record.weight.is_finite() || record.weight <= 0.0 {
            return Err(Rejection::NonFinite);
        }
        let energy_ev = record.energy_mev * 1.0e6;
        if !(energy_ev > self.energy_range_ev[0] && energy_ev <= self.energy_range_ev[1]) {
            return Err(Rejection::OutsideEnergy);
        }
        let norm = record.direction.iter().map(|c| c * c).sum::<f64>().sqrt();
        if norm < 0.5 {
            return Err(Rejection::NonFinite);
        }
        let dir = self.to_case([
            record.direction[0] / norm,
            record.direction[1] / norm,
            record.direction[2] / norm,
        ]);
        let a = self.plane_axis.index();
        let mu = f64::from(self.inward_sign) * dir[a];
        if mu <= 0.0 {
            return Err(Rejection::Backward);
        }
        if mu < self.min_cosine {
            return Err(Rejection::Grazing);
        }
        let rel = self.to_case([
            record.position_cm[0],
            record.position_cm[1],
            record.position_cm[2] - self.reference_z_cm,
        ]);
        let (u, v) = self.plane_axis.in_plane_axes();
        // Advance in vacuum to the plane: rel[a] is measured from it.
        let t = -rel[a] / dir[a];
        if t < -1.0e-6 {
            return Err(Rejection::PastPlane);
        }
        let uu = self.center_uv_cm[0] + rel[u] + t * dir[u];
        let vv = self.center_uv_cm[1] + rel[v] + t * dir[v];
        if uu < self.face_u_range_cm[0]
            || uu >= self.face_u_range_cm[1]
            || vv < self.face_v_range_cm[0]
            || vv >= self.face_v_range_cm[1]
        {
            return Err(Rejection::OutsideFace);
        }
        Ok(AcceptedParticle {
            uv_cm: [uu, vv],
            direction: dir,
            inward_cosine: mu.min(1.0),
            energy_ev,
            weight: record.weight,
        })
    }

    /// Inward unit normal of the source plane.
    #[must_use]
    pub fn inward_normal(&self) -> [f64; 3] {
        let mut n = [0.0; 3];
        n[self.plane_axis.index()] = f64::from(self.inward_sign);
        n
    }
}

/// File identity and accounting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseSpaceProvenance {
    pub header_path: String,
    pub header_sha256: String,
    pub data_path: String,
    pub data_sha256: String,
    pub header_title: Option<String>,
    pub original_histories: Option<f64>,
    pub records_in_file: u64,
    pub neutron_records: u64,
    pub neutron_weight_sum: f64,
    /// Photon records are counted and reported; the deterministic
    /// neutron path does not use them.
    pub photon_records: u64,
    pub photon_weight_sum: f64,
    pub other_particle_records: u64,
    pub accepted_neutron_records: u64,
    pub accepted_weight_sum: f64,
    pub rejected: RejectCounts,
    /// `accepted_weight_sum / original_histories`, when the header
    /// declares the histories: multiply the unit-normalized source by
    /// this for accepted neutrons per original history.
    pub accepted_weight_per_original_history: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelGrid {
    pub pixel_cm: f64,
    pub u0_cm: f64,
    pub v0_cm: f64,
    pub nu: u32,
    pub nv: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionBins {
    pub rings: u32,
    pub sectors: u32,
    /// Ring edges run from cosθ = 1 down to this (the largest polar angle).
    pub cos_theta_max: f64,
    /// Weighted mean unit direction of each bin, case frame, `[ring * sectors + sector]`.
    pub mean_direction: Vec<[f64; 3]>,
    /// Weighted mean of 1 / (inward cosine) per bin.
    pub mean_inverse_cosine: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnergyStructure {
    /// Multigroup boundaries, eV, strictly descending (group 0 highest).
    pub group_boundaries_ev: Vec<f64>,
    pub multigroup_data_sha256: String,
}

/// Sparse weight table, sorted by (pixel, direction, group).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseSpaceEntries {
    /// `iu + nu * iv`.
    pub pixel: Vec<u32>,
    pub direction: Vec<u16>,
    pub group: Vec<u16>,
    /// Fraction of accepted weight; all entries sum to 1.
    pub weight: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhaseSpaceSource {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    pub provenance: PhaseSpaceProvenance,
    pub selection: PhaseSpaceSelection,
    pub pixels: PixelGrid,
    pub directions: DirectionBins,
    pub energy: EnergyStructure,
    /// Per-group total of `entries.weight` (group 0 highest energy).
    pub group_marginal: Vec<f64>,
    pub normalization: String,
    pub entries: PhaseSpaceEntries,
}

impl PhaseSpaceSource {
    pub fn direction_count(&self) -> usize {
        (self.directions.rings * self.directions.sectors) as usize
    }

    pub fn group_count(&self) -> usize {
        self.energy.group_boundaries_ev.len().saturating_sub(1)
    }

    pub fn validate(&self) -> Result<(), PhaseSpaceError> {
        if !openbnct_core::schema_matches(&self.schema_version, PHASE_SPACE_SOURCE_SCHEMA) {
            return invalid(format!("unsupported schema {:?}", self.schema_version));
        }
        let nd = self.direction_count();
        let groups = self.group_count();
        let p = &self.pixels;
        if !(p.pixel_cm.is_finite() && p.pixel_cm > 0.0) || p.nu == 0 || p.nv == 0 {
            return invalid("pixel grid is empty or non-positive");
        }
        if nd == 0 || nd > u16::MAX as usize || groups == 0 || groups > u16::MAX as usize {
            return invalid("direction or group count out of range");
        }
        if (u64::from(p.nu) * u64::from(p.nv)).saturating_mul(nd as u64) > MAX_PIXEL_DIRECTION_CELLS
        {
            return invalid("pixels x direction bins exceeds the supported table size");
        }
        if self.directions.mean_direction.len() != nd
            || self.directions.mean_inverse_cosine.len() != nd
            || self.group_marginal.len() != groups
        {
            return invalid("per-direction or per-group arrays do not match the declared bins");
        }
        if !(self.directions.cos_theta_max > 0.0 && self.directions.cos_theta_max <= 1.0) {
            return invalid("cos_theta_max must be in (0, 1]");
        }
        let e = &self.entries;
        let n = e.weight.len();
        if e.pixel.len() != n || e.direction.len() != n || e.group.len() != n || n == 0 {
            return invalid("entry arrays are empty or of unequal length");
        }
        let cells = u64::from(p.nu) * u64::from(p.nv);
        let mut last = None::<u64>;
        let mut sum = 0.0;
        for i in 0..n {
            if u64::from(e.pixel[i]) >= cells
                || usize::from(e.direction[i]) >= nd
                || usize::from(e.group[i]) >= groups
                || !(e.weight[i].is_finite() && e.weight[i] > 0.0)
            {
                return invalid(format!("entry {i} is out of range"));
            }
            let key = (u64::from(e.pixel[i]) * nd as u64 + u64::from(e.direction[i]))
                * groups as u64
                + u64::from(e.group[i]);
            if last.is_some_and(|l| key <= l) {
                return invalid("entries are not strictly sorted by (pixel, direction, group)");
            }
            last = Some(key);
            sum += e.weight[i];
        }
        if (sum - 1.0).abs() > 1e-6 {
            return invalid(format!("entry weights sum to {sum}, expected 1"));
        }
        Ok(())
    }

    /// Solver-side lookup: `start[pixel * nd + dir] .. start[.. + 1]`
    /// indexes the (group, weight) entries of that pixel/direction.
    pub fn index(&self) -> PhaseSpaceIndex {
        let nd = self.direction_count();
        let cells = (self.pixels.nu as usize) * (self.pixels.nv as usize) * nd;
        let mut start = vec![0_u32; cells + 1];
        for (pixel, dir) in self.entries.pixel.iter().zip(&self.entries.direction) {
            start[*pixel as usize * nd + usize::from(*dir) + 1] += 1;
        }
        for i in 0..cells {
            start[i + 1] += start[i];
        }
        PhaseSpaceIndex { start }
    }

    /// Energy-group marginal as source-histogram bins in ascending energy,
    /// for consumers that read `source.energy`.
    pub fn ascending_histogram(&self) -> (Vec<f64>, Vec<f64>) {
        let boundaries: Vec<f64> = self
            .energy
            .group_boundaries_ev
            .iter()
            .rev()
            .copied()
            .collect();
        let weights: Vec<f64> = self.group_marginal.iter().rev().copied().collect();
        (boundaries, weights)
    }

    /// Stream the original file and hand every accepted neutron to `f`,
    /// after verifying both file hashes against the recorded provenance.
    /// Returns the rejection counts observed (must equal the recorded ones).
    pub fn for_each_accepted(
        &self,
        mut f: impl FnMut(&AcceptedParticle),
    ) -> Result<RejectCounts, PhaseSpaceError> {
        let header_path = Path::new(&self.provenance.header_path);
        if sha256_file(header_path)? != self.provenance.header_sha256 {
            return invalid("phase-space header no longer matches its recorded sha256");
        }
        let mut reader = PhspReader::open(header_path)?;
        if sha256_file(&reader.data_path)? != self.provenance.data_sha256 {
            return invalid("phase-space data no longer matches its recorded sha256");
        }
        let mut rejected = RejectCounts::default();
        while let Some(record) = reader.next_record()? {
            if record.particle_type != PHSP_NEUTRON {
                continue;
            }
            match self.selection.classify(&record) {
                Ok(p) => {
                    if p.inward_cosine < self.directions.cos_theta_max {
                        rejected.outside_direction_range += 1;
                    } else {
                        f(&p);
                    }
                }
                Err(why) => rejected.bump(why),
            }
        }
        if rejected != self.provenance.rejected {
            return invalid("re-reading the phase-space file gave different rejection counts");
        }
        Ok(rejected)
    }
}

/// Start offsets into a [`PhaseSpaceSource`]'s entry arrays.
pub struct PhaseSpaceIndex {
    start: Vec<u32>,
}

impl PhaseSpaceIndex {
    #[inline]
    #[must_use]
    pub fn range(
        &self,
        pixel: usize,
        dir: usize,
        direction_count: usize,
    ) -> std::ops::Range<usize> {
        let i = pixel * direction_count + dir;
        self.start[i] as usize..self.start[i + 1] as usize
    }
}

/// A source table with its lookup index, shared across solver calls.
pub struct LoadedPhaseSpace {
    pub source: PhaseSpaceSource,
    pub index: PhaseSpaceIndex,
}

/// SHA-256 of a file, streaming.
pub fn sha256_file(path: &Path) -> Result<String, PhaseSpaceError> {
    let io = |source| PhaseSpaceError::Io {
        path: path.display().to_string(),
        source,
    };
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).map_err(io)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn table_cache() -> &'static Mutex<HashMap<String, Arc<LoadedPhaseSpace>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<LoadedPhaseSpace>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load a table by path, verifying its sha256 against the case's
/// declaration. Loaded tables are cached by hash.
pub fn load_phase_space_source(
    path: &str,
    sha256: &str,
) -> Result<Arc<LoadedPhaseSpace>, PhaseSpaceError> {
    if let Some(hit) = table_cache().lock().expect("cache lock").get(sha256) {
        return Ok(Arc::clone(hit));
    }
    let bytes = std::fs::read(path).map_err(|source| PhaseSpaceError::Io {
        path: path.to_string(),
        source,
    })?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != sha256 {
        return invalid(format!(
            "phase-space table {path} has sha256 {actual}, the case declares {sha256}"
        ));
    }
    let source: PhaseSpaceSource = serde_json::from_slice(&bytes)
        .map_err(|e| PhaseSpaceError::Invalid(format!("{path}: {e}")))?;
    source.validate()?;
    let index = source.index();
    let loaded = Arc::new(LoadedPhaseSpace { source, index });
    table_cache()
        .lock()
        .expect("cache lock")
        .insert(actual, Arc::clone(&loaded));
    Ok(loaded)
}

/// Binning parameters.
#[derive(Debug, Clone)]
pub struct PhaseSpaceBinOptions {
    pub id: String,
    /// `+z` etc.; see the module docs.
    pub beam_direction: String,
    /// File axis position on the plane; `None` = face centre.
    pub center_uv_cm: Option<[f64; 2]>,
    /// File z of the plane; `None` = the header's constant Z, else 0.
    pub reference_z_cm: Option<f64>,
    pub pixel_cm: f64,
    pub rings: u32,
    pub sectors: u32,
    /// Cut records beyond this polar angle; `None` = the file's maximum.
    pub theta_max_rad: Option<f64>,
    pub min_cosine: f64,
}

impl PhaseSpaceBinOptions {
    #[must_use]
    pub fn new(id: &str, beam_direction: &str) -> Self {
        Self {
            id: id.into(),
            beam_direction: beam_direction.into(),
            center_uv_cm: None,
            reference_z_cm: None,
            pixel_cm: 0.5,
            rings: 8,
            sectors: 16,
            theta_max_rad: None,
            min_cosine: 1.0e-3,
        }
    }
}

/// Reduce a phase-space file to a [`PhaseSpaceSource`] for `geometry`
/// and the group structure of `data`. `data_sha256` identifies the
/// multigroup artifact the groups came from.
pub fn bin_phase_space(
    header_path: &Path,
    geometry: &GridGeometry,
    data: &MultigroupData,
    data_sha256: &str,
    options: &PhaseSpaceBinOptions,
) -> Result<PhaseSpaceSource, PhaseSpaceError> {
    if !(options.pixel_cm.is_finite() && options.pixel_cm > 0.0) {
        return invalid("pixel size must be positive");
    }
    if options.rings == 0 || options.sectors == 0 {
        return invalid("direction bins need at least one ring and one sector");
    }
    let nd = (options.rings as usize) * (options.sectors as usize);
    if nd > u16::MAX as usize {
        return invalid("too many direction bins");
    }
    let (axis, sign) = parse_beam_direction(&options.beam_direction)?;
    let a = axis.index();
    let (u_axis, v_axis) = axis.in_plane_axes();
    let (minimum, maximum) = geometry
        .bounding_box_lps_mm()
        .map_err(|e| PhaseSpaceError::Invalid(e.to_string()))?;
    let plane_offset_cm = if sign > 0 {
        minimum[a] / 10.0 + PHASE_SPACE_PLANE_MARGIN_CM
    } else {
        maximum[a] / 10.0 - PHASE_SPACE_PLANE_MARGIN_CM
    };
    let face_u = [minimum[u_axis] / 10.0, maximum[u_axis] / 10.0];
    let face_v = [minimum[v_axis] / 10.0, maximum[v_axis] / 10.0];
    let center_uv = options
        .center_uv_cm
        .unwrap_or([0.5 * (face_u[0] + face_u[1]), 0.5 * (face_v[0] + face_v[1])]);

    let mut reader = PhspReader::open(header_path)?;
    let reference_z_cm = options.reference_z_cm.unwrap_or_else(|| {
        if reader.header.stored[2] {
            0.0
        } else {
            reader.header.constants[2]
        }
    });
    let bounds = &data.energy_boundaries_ev;
    let groups = data.group_count();
    if groups == 0 || groups > u16::MAX as usize {
        return invalid("multigroup data has no usable groups");
    }
    let selection = PhaseSpaceSelection {
        beam_direction: options.beam_direction.trim().to_ascii_lowercase(),
        plane_axis: axis,
        inward_sign: sign,
        plane_offset_cm,
        center_uv_cm: center_uv,
        reference_z_cm,
        face_u_range_cm: face_u,
        face_v_range_cm: face_v,
        min_cosine: options.min_cosine,
        energy_range_ev: [bounds[groups], bounds[0]],
    };

    // Pass 1: accounting by type, footprint and largest polar angle.
    let header_sha = sha256_file(header_path)?;
    let data_sha = sha256_file(&reader.data_path)?;
    let mut prov = PhaseSpaceProvenance {
        header_path: header_path.display().to_string(),
        header_sha256: header_sha,
        data_path: reader.data_path.display().to_string(),
        data_sha256: data_sha,
        header_title: reader.header.title.clone(),
        original_histories: reader.header.original_histories,
        records_in_file: reader.total_records,
        neutron_records: 0,
        neutron_weight_sum: 0.0,
        photon_records: 0,
        photon_weight_sum: 0.0,
        other_particle_records: 0,
        accepted_neutron_records: 0,
        accepted_weight_sum: 0.0,
        rejected: RejectCounts::default(),
        accepted_weight_per_original_history: None,
    };
    let (mut umin, mut umax) = (f64::INFINITY, f64::NEG_INFINITY);
    let (mut vmin, mut vmax) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut min_mu = f64::INFINITY;
    while let Some(record) = reader.next_record()? {
        match record.particle_type {
            PHSP_NEUTRON => {
                prov.neutron_records += 1;
                prov.neutron_weight_sum += record.weight;
                if let Ok(p) = selection.classify(&record) {
                    umin = umin.min(p.uv_cm[0]);
                    umax = umax.max(p.uv_cm[0]);
                    vmin = vmin.min(p.uv_cm[1]);
                    vmax = vmax.max(p.uv_cm[1]);
                    min_mu = min_mu.min(p.inward_cosine);
                }
            }
            PHSP_PHOTON => {
                prov.photon_records += 1;
                prov.photon_weight_sum += record.weight;
            }
            _ => prov.other_particle_records += 1,
        }
    }
    if !min_mu.is_finite() {
        return invalid(format!(
            "no neutron in the file reaches the source plane inside the grid face \
             ({} neutron records, {} photon records)",
            prov.neutron_records, prov.photon_records
        ));
    }
    let cos_theta_max = match options.theta_max_rad {
        Some(theta) => {
            if !(theta > 0.0 && theta < std::f64::consts::FRAC_PI_2) {
                return invalid("theta_max must lie in (0, pi/2)");
            }
            theta.cos().max(options.min_cosine)
        }
        // A hair below the smallest cosine so that record is inside the last ring.
        None => (min_mu * (1.0 - 1.0e-9)).min(1.0),
    };
    let pixel = options.pixel_cm;
    let u0 = (umin / pixel).floor() * pixel;
    let v0 = (vmin / pixel).floor() * pixel;
    let nu = (((umax - u0) / pixel).floor() as u64 + 1) as u32;
    let nv = (((vmax - v0) / pixel).floor() as u64 + 1) as u32;
    if (u64::from(nu) * u64::from(nv)).saturating_mul(nd as u64) > MAX_PIXEL_DIRECTION_CELLS {
        return invalid(format!(
            "{nu} x {nv} pixels x {nd} direction bins is too large; use a coarser pixel size"
        ));
    }

    // Pass 2: histogram.
    let mut reader = PhspReader::open(header_path)?;
    let mut table: HashMap<u64, f64> = HashMap::new();
    let mut dir_w = vec![0.0_f64; nd];
    let mut dir_wvec = vec![[0.0_f64; 3]; nd];
    let mut dir_winv = vec![0.0_f64; nd];
    let one_minus = 1.0 - cos_theta_max;
    let mut rejected = RejectCounts::default();
    let mut accepted_records = 0_u64;
    let mut accepted_weight = 0.0_f64;
    let (u_of, v_of) = (u_axis, v_axis);
    while let Some(record) = reader.next_record()? {
        if record.particle_type != PHSP_NEUTRON {
            continue;
        }
        let p = match selection.classify(&record) {
            Ok(p) => p,
            Err(why) => {
                rejected.bump(why);
                continue;
            }
        };
        if p.inward_cosine < cos_theta_max {
            rejected.outside_direction_range += 1;
            continue;
        }
        let ring = if one_minus < 1.0e-12 {
            0
        } else {
            (((1.0 - p.inward_cosine) / one_minus * f64::from(options.rings)) as u32)
                .min(options.rings - 1)
        };
        let phi = p.direction[v_of].atan2(p.direction[u_of]);
        let sector = (((phi + std::f64::consts::PI) / std::f64::consts::TAU
            * f64::from(options.sectors)) as u32)
            .min(options.sectors - 1);
        let dir = (ring * options.sectors + sector) as usize;
        let iu = (((p.uv_cm[0] - u0) / pixel).floor() as i64).clamp(0, i64::from(nu) - 1) as u64;
        let iv = (((p.uv_cm[1] - v0) / pixel).floor() as i64).clamp(0, i64::from(nv) - 1) as u64;
        let pix = iu + u64::from(nu) * iv;
        let Some(group) = data.group_of(p.energy_ev) else {
            rejected.outside_energy += 1;
            continue;
        };
        *table
            .entry((pix * nd as u64 + dir as u64) * groups as u64 + group as u64)
            .or_insert(0.0) += p.weight;
        dir_w[dir] += p.weight;
        for (acc, component) in dir_wvec[dir].iter_mut().zip(p.direction) {
            *acc += p.weight * component;
        }
        dir_winv[dir] += p.weight / p.inward_cosine;
        accepted_records += 1;
        accepted_weight += p.weight;
    }
    if accepted_records == 0 || accepted_weight <= 0.0 {
        return invalid("no neutron survived selection");
    }
    prov.accepted_neutron_records = accepted_records;
    prov.accepted_weight_sum = accepted_weight;
    prov.rejected = rejected;
    prov.accepted_weight_per_original_history = prov
        .original_histories
        .filter(|h| *h > 0.0)
        .map(|h| accepted_weight / h);

    let mut keys: Vec<(u64, f64)> = table.into_iter().collect();
    keys.sort_unstable_by_key(|(k, _)| *k);
    let mut entries = PhaseSpaceEntries::default();
    let mut marginal = vec![0.0_f64; groups];
    let g = groups as u64;
    for (key, w) in keys {
        let group = key % g;
        let rest = key / g;
        let dir = rest % nd as u64;
        let pix = rest / nd as u64;
        let fraction = w / accepted_weight;
        entries.pixel.push(pix as u32);
        entries.direction.push(dir as u16);
        entries.group.push(group as u16);
        entries.weight.push(fraction);
        marginal[group as usize] += fraction;
    }

    let inward = selection.inward_normal();
    let mut mean_direction = Vec::with_capacity(nd);
    let mut mean_inverse_cosine = Vec::with_capacity(nd);
    for d in 0..nd {
        if dir_w[d] > 0.0 {
            let v = dir_wvec[d];
            let n = v.iter().map(|c| c * c).sum::<f64>().sqrt();
            mean_direction.push([v[0] / n, v[1] / n, v[2] / n]);
            mean_inverse_cosine.push(dir_winv[d] / dir_w[d]);
        } else {
            mean_direction.push(inward);
            mean_inverse_cosine.push(1.0);
        }
    }

    let source = PhaseSpaceSource {
        schema_version: PHASE_SPACE_SOURCE_SCHEMA.into(),
        id: options.id.clone(),
        provenance: prov,
        selection,
        pixels: PixelGrid {
            pixel_cm: pixel,
            u0_cm: u0,
            v0_cm: v0,
            nu,
            nv,
        },
        directions: DirectionBins {
            rings: options.rings,
            sectors: options.sectors,
            cos_theta_max,
            mean_direction,
            mean_inverse_cosine,
        },
        energy: EnergyStructure {
            group_boundaries_ev: bounds.clone(),
            multigroup_data_sha256: data_sha256.into(),
        },
        group_marginal: marginal,
        normalization: "entries sum to 1: weight per accepted source neutron (accepted weight \
                        sum, not per original history; see provenance)"
            .into(),
        entries,
    };
    source.validate()?;
    Ok(source)
}

/// The `FixedSourceDefinition` a case carries for a binned phase space:
/// the `phase_space` space referencing the table by path and hash, plus a
/// cone / spectrum envelope of the table for consumers that read
/// `source.angle` / `source.energy` (the solver itself uses the table).
pub fn phase_space_fixed_source(
    table: &PhaseSpaceSource,
    table_path: &str,
    table_sha256: &str,
) -> FixedSourceDefinition {
    let sel = &table.selection;
    let half_angle = table.directions.cos_theta_max.acos();
    let normal = sel.inward_normal();
    let angle = if half_angle < 1.0e-9 {
        AngularDistribution::Monodirectional {
            unit_vector: normal,
        }
    } else {
        AngularDistribution::IsotropicCone {
            axis_unit_vector: normal,
            half_angle_rad: half_angle,
        }
    };
    let (boundaries, weights) = table.ascending_histogram();
    FixedSourceDefinition {
        schema_version: "openbnct.fixed-source-definition/0.1.0".into(),
        id: table.id.clone(),
        particle: ParticleType::Neutron,
        source_sites_per_history: 1,
        statistical_weight_per_site: 1.0,
        space: SourceSpatialDistribution::PhaseSpace {
            axis: sel.plane_axis,
            offset_cm: sel.plane_offset_cm,
            table_path: table_path.into(),
            table_sha256: table_sha256.into(),
        },
        angle,
        energy: EnergyDistribution::TabulatedHistogram {
            energy_boundaries_ev: boundaries,
            bin_weights: weights,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phsp::write_iaea_phsp;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [10, 10, 10],
            spacing_mm: [10.0; 3],
            origin_mm: [-45.0, -45.0, 5.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn rec(kind: i8, e_mev: f64, x: f64, y: f64, u: f64, v: f64, w: f64) -> PhspRecord {
        let wz = (1.0 - u * u - v * v).sqrt();
        PhspRecord {
            particle_type: kind,
            new_history: true,
            energy_mev: e_mev,
            position_cm: [x, y, 0.0],
            direction: [u, v, wz],
            weight: w,
        }
    }

    #[test]
    fn selection_frames_and_rejections() {
        let sel = PhaseSpaceSelection {
            beam_direction: "+z".into(),
            plane_axis: PlaneAxis::Z,
            inward_sign: 1,
            plane_offset_cm: 1.0e-6,
            center_uv_cm: [0.0, 0.0],
            reference_z_cm: 0.0,
            face_u_range_cm: [-5.0, 5.0],
            face_v_range_cm: [-5.0, 5.0],
            min_cosine: 1.0e-3,
            energy_range_ev: [1.0e-5, 2.0e7],
        };
        let p = sel.classify(&rec(4, 1.0, 1.0, 2.0, 0.0, 0.0, 3.0)).unwrap();
        assert_eq!(p.uv_cm, [1.0, 2.0]);
        assert!((p.energy_ev - 1.0e6).abs() < 1e-6);
        assert!((p.inward_cosine - 1.0).abs() < 1e-12);
        // 30 degrees in x: advancing to the plane from z = -1 shifts x by tan30.
        let mut r = rec(4, 1.0, 0.0, 0.0, 0.5, 0.0, 1.0);
        r.position_cm[2] = -1.0;
        let p = sel.classify(&r).unwrap();
        assert!((p.uv_cm[0] - 1.0 * 0.5 / (0.75_f64).sqrt()).abs() < 1e-9);
        // Backward, outside face, high energy, already past the plane.
        let mut back = rec(4, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        back.direction[2] = -1.0;
        assert_eq!(sel.classify(&back), Err(Rejection::Backward));
        assert_eq!(
            sel.classify(&rec(4, 1.0, 9.0, 0.0, 0.0, 0.0, 1.0)),
            Err(Rejection::OutsideFace)
        );
        assert_eq!(
            sel.classify(&rec(4, 50.0, 0.0, 0.0, 0.0, 0.0, 1.0)),
            Err(Rejection::OutsideEnergy)
        );
        let mut past = rec(4, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0);
        past.position_cm[2] = 2.0;
        assert_eq!(sel.classify(&past), Err(Rejection::PastPlane));
        // A -z beam reverses z and x but stays a proper rotation.
        let neg = PhaseSpaceSelection {
            beam_direction: "-z".into(),
            inward_sign: -1,
            plane_offset_cm: 9.0,
            ..sel.clone()
        };
        let mut r = rec(4, 1.0, 1.0, 2.0, 0.6, 0.0, 1.0);
        r.position_cm[2] = 0.0;
        let p = neg.classify(&r).unwrap();
        assert!((p.uv_cm[0] - -1.0).abs() < 1e-9 && (p.uv_cm[1] - 2.0).abs() < 1e-9);
        assert!((p.direction[0] + 0.6).abs() < 1e-12 && (p.direction[2] + 0.8).abs() < 1e-12);
    }

    fn two_group_data() -> MultigroupData {
        MultigroupData {
            schema_version: crate::MULTIGROUP_DATA_SCHEMA.into(),
            id: "t".into(),
            energy_boundaries_ev: vec![2.0e7, 1.0e5, 1.0e-5],
            collapse_declaration: String::new(),
            component_profile: None,
            boron_unit_response_gy_cm2_per_ug_g: None,
            materials: vec![],
        }
    }

    #[test]
    fn bins_a_tiny_file_and_counts_rejections() {
        let dir = std::env::temp_dir().join(format!("openbnct-ps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let header = dir.join("b.IAEAheader");
        let mut records = vec![
            rec(4, 1.0, 0.5, 0.5, 0.0, 0.0, 1.0),
            rec(4, 1.0, 0.5, 0.5, 0.0, 0.0, 1.0),
            rec(4, 0.01, -2.0, 1.0, 0.1, 0.0, 2.0),
            rec(1, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            rec(4, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0),
            rec(4, 1.0, 50.0, 0.0, 0.0, 0.0, 1.0),
        ];
        records[4].direction = [0.0, 0.0, -1.0];
        write_iaea_phsp(&header, &records, 6.0, "tiny").unwrap();
        let data = two_group_data();
        let mut options = PhaseSpaceBinOptions::new("test.phsp", "+z");
        options.pixel_cm = 1.0;
        options.rings = 2;
        options.sectors = 4;
        let source = bin_phase_space(
            &header,
            &geometry(),
            &data,
            "0".repeat(64).as_str(),
            &options,
        )
        .unwrap();
        let p = &source.provenance;
        assert_eq!(p.neutron_records, 5);
        assert_eq!(p.photon_records, 1);
        assert_eq!(p.accepted_neutron_records, 3);
        assert_eq!(p.rejected.backward, 1);
        assert_eq!(p.rejected.outside_face, 1);
        assert!((p.accepted_weight_sum - 4.0).abs() < 1e-6);
        assert_eq!(p.original_histories, Some(6.0));
        let total: f64 = source.entries.weight.iter().sum();
        assert!((total - 1.0).abs() < 1e-12);
        // 1 MeV -> group 0, 10 keV -> group 1; weights 2/4 and 2/4.
        assert!((source.group_marginal[0] - 0.5).abs() < 1e-6);
        assert!((source.group_marginal[1] - 0.5).abs() < 1e-6);
        // Round trip through JSON and the lookup index.
        let json = serde_json::to_vec(&source).unwrap();
        let back: PhaseSpaceSource = serde_json::from_slice(&json).unwrap();
        back.validate().unwrap();
        let index = back.index();
        let nd = back.direction_count();
        let covered: usize = (0..(back.pixels.nu * back.pixels.nv) as usize)
            .flat_map(|px| (0..nd).map(move |d| (px, d)))
            .map(|(px, d)| index.range(px, d, nd).len())
            .sum();
        assert_eq!(covered, back.entries.weight.len());
        // The OpenMC-side stream sees exactly the accepted particles.
        let mut seen = 0;
        let mut w = 0.0;
        back.for_each_accepted(|q| {
            seen += 1;
            w += q.weight;
        })
        .unwrap();
        assert_eq!((seen, (w - 4.0).abs() < 1e-6), (3, true));
        let fixed = phase_space_fixed_source(&back, "t.json", &"0".repeat(64));
        fixed.validate().unwrap();
    }
}
