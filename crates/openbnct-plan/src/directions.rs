// SPDX-License-Identifier: MIT

//! Beam-direction candidate enumeration and geometric pre-scoring.
//!
//! `plan fields` aims and solves one field per named direction; this
//! module generates the candidate set — an azimuth×elevation grid of
//! propagation vectors converging on the aim-mask centroid — and
//! attaches a zero-transport score: the millimetres of tissue a ray
//! traverses before reaching the centroid (depth of target under that
//! incidence). Epithermal beams lose fluence with depth, so shallower
//! incidence scores better; the ranking selects which directions are
//! worth a full transport solve. It is a heuristic pre-filter, not a
//! dosimetric model — the real ranking stays with `plan optimize` and
//! `plan robustness` on solved fields.

use openbnct_core::{ContentReference, GridGeometry, RegionMask};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current schema token for direction-candidate sweeps.
pub const DIRECTION_CANDIDATES_SCHEMA: &str = "openbnct.direction-candidates/0.1.0";

/// Qualification string carried by every direction-candidates document.
pub const DIRECTION_CANDIDATES_QUALIFICATION: &str =
    "beam_direction_sweep_research_only_not_clinical";

/// One candidate direction with its geometric score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionCandidate {
    /// Generated name (`az###-el##`).
    pub name: String,
    /// Propagation direction in LPS, unit vector, pointing from the
    /// entry surface toward the aim centroid.
    pub direction_lps: [f64; 3],
    /// Azimuth and elevation of the entry direction in degrees.
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    /// Tissue path length in millimetres traversed before reaching the
    /// aim centroid — `None` only when no body mask was supplied, in
    /// which case the grid-diagonal bound is reported instead.
    pub tissue_path_mm: Option<f64>,
    /// Adjoint importance score — the inner product of the
    /// direction's uncollided beam with the aim-region adjoint field.
    /// `None` unless the sweep ran with a multigroup-data solve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjoint_score: Option<f64>,
}

/// The angular grid the sweep enumerated, recorded verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionGridDeclaration {
    pub azimuth_steps: u32,
    pub elevation_steps: u32,
    /// Aperture radius in cm used for adjoint scoring, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aperture_radius_cm: Option<f64>,
}

/// Versioned record of a direction sweep: the ranked candidate list
/// with both scores and every input content-bound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectionCandidatesDocument {
    pub schema_version: String,
    pub id: String,
    pub case_id: String,
    /// How the list order was produced: `tissue_path_length` or
    /// `adjoint_importance`.
    pub scoring: String,
    pub grid: DirectionGridDeclaration,
    /// Ranked best-first.
    pub candidates: Vec<DirectionCandidate>,
    /// Content bindings to every input consumed.
    pub case: ContentReference,
    pub aim_mask: ContentReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_mask: Option<ContentReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multigroup_data: Option<ContentReference>,
    pub provenance_id: String,
    pub qualification: String,
}

/// Assemble the content-bound sweep document.
#[allow(clippy::too_many_arguments)]
pub fn build_direction_candidates_document(
    id: &str,
    case_id: &str,
    candidates: Vec<DirectionCandidate>,
    scoring: &str,
    azimuth_steps: u32,
    elevation_steps: u32,
    aperture_radius_cm: Option<f64>,
    case: ContentReference,
    aim_mask: ContentReference,
    body_mask: Option<ContentReference>,
    multigroup_data: Option<ContentReference>,
    provenance_id: &str,
) -> DirectionCandidatesDocument {
    DirectionCandidatesDocument {
        schema_version: DIRECTION_CANDIDATES_SCHEMA.into(),
        id: id.into(),
        case_id: case_id.into(),
        scoring: scoring.into(),
        grid: DirectionGridDeclaration {
            azimuth_steps,
            elevation_steps,
            aperture_radius_cm,
        },
        candidates,
        case,
        aim_mask,
        body_mask,
        multigroup_data,
        provenance_id: provenance_id.into(),
        qualification: DIRECTION_CANDIDATES_QUALIFICATION.into(),
    }
}

#[derive(Debug, Error)]
pub enum DirectionError {
    #[error("aim mask has no voxels")]
    EmptyAimMask,
    #[error("angular step counts must be ≥ 1")]
    DegenerateGrid,
    #[error("mask/geometry mismatch: {0}")]
    Geometry(String),
}

/// Mask centroid in millimetres, LPS — the point every direction aims
/// through.
pub fn mask_centroid_mm(geometry: &GridGeometry, mask: &RegionMask) -> [f64; 3] {
    let mut sum = [0.0_f64; 3];
    let mut count = 0.0_f64;
    let [nx, ny, _] = geometry.shape;
    for (index, present) in mask.voxels.iter().enumerate() {
        if !present {
            continue;
        }
        let i = (index % nx as usize) as f64;
        let j = ((index / nx as usize) % ny as usize) as f64;
        let k = (index / (nx as usize * ny as usize)) as f64;
        // voxel-centre position on the grid (direction ≈ identity for
        // imported cases; apply the declared direction cosines honestly).
        let local = [
            (i + 0.5) * geometry.spacing_mm[0],
            (j + 0.5) * geometry.spacing_mm[1],
            (k + 0.5) * geometry.spacing_mm[2],
        ];
        for (axis, acc) in sum.iter_mut().enumerate() {
            *acc += geometry.origin_mm[axis]
                + local[0] * geometry.direction[axis * 3]
                + local[1] * geometry.direction[axis * 3 + 1]
                + local[2] * geometry.direction[axis * 3 + 2];
        }
        count += 1.0;
    }
    if count == 0.0 {
        return geometry.origin_mm;
    }
    [sum[0] / count, sum[1] / count, sum[2] / count]
}

/// Tissue path length along a ray: march from the centroid outward
/// against `direction` until leaving `body` (or the grid); return the
/// millimetres spent inside body-mask voxels. This is the depth an
/// epithermal beam must moderate through before reaching the target.
fn tissue_path_mm(
    geometry: &GridGeometry,
    body: &RegionMask,
    centroid: [f64; 3],
    direction: [f64; 3],
) -> f64 {
    let [nx, ny, nz] = geometry.shape;
    let count = (nx as usize * ny as usize * nz as usize).min(body.voxels.len());
    let inside = |p: [f64; 3]| -> bool {
        // p in LPS mm → fractional voxel index on the grid.
        let rel = [
            p[0] - geometry.origin_mm[0],
            p[1] - geometry.origin_mm[1],
            p[2] - geometry.origin_mm[2],
        ];
        // invert the direction matrix (orthonormal rows).
        let mut idx = [0.0_f64; 3];
        for (axis, slot) in idx.iter_mut().enumerate() {
            *slot = (rel[0] * geometry.direction[axis]
                + rel[1] * geometry.direction[3 + axis]
                + rel[2] * geometry.direction[6 + axis])
                / geometry.spacing_mm[axis];
        }
        let (i, j, k) = (idx[0] as i64, idx[1] as i64, idx[2] as i64);
        if i < 0 || j < 0 || k < 0 {
            return false;
        }
        let (i, j, k) = (i as usize, j as usize, k as usize);
        if i >= nx as usize || j >= ny as usize || k >= nz as usize {
            return false;
        }
        let index = i + nx as usize * j + nx as usize * ny as usize * k;
        index < count && body.voxels[index]
    };
    // March in half-min-spacing steps — finer than a voxel, cheap.
    let step = geometry
        .spacing_mm
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min)
        * 0.5;
    let mut travelled = 0.0;
    let mut tissue = 0.0;
    let diagonal = {
        let d = [
            nx as f64 * geometry.spacing_mm[0],
            ny as f64 * geometry.spacing_mm[1],
            nz as f64 * geometry.spacing_mm[2],
        ];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    };
    // Walk *against* the propagation direction — from centroid back to
    // the entry surface.
    let mut p = centroid;
    while travelled < diagonal {
        if inside(p) {
            tissue += step;
        } else if travelled > step {
            // First voxel outside after having been inside = the skin
            // line; everything past it is air/gantry.
            break;
        }
        p = [
            p[0] - direction[0] * step,
            p[1] - direction[1] * step,
            p[2] - direction[2] * step,
        ];
        travelled += step;
    }
    tissue
}

/// Enumerate an azimuth×elevation grid of propagation directions
/// converging on `aim`'s centroid. `azimuth_steps` sweeps 0..360,
/// `elevation_steps` sweeps −60..+60 degrees (clinically meaningful
/// incidence for head/neck and brain fields). When `body` is given,
/// each candidate carries the tissue path length the beam must cross.
pub fn enumerate_directions(
    geometry: &GridGeometry,
    aim: &RegionMask,
    body: Option<&RegionMask>,
    azimuth_steps: u32,
    elevation_steps: u32,
) -> Result<Vec<DirectionCandidate>, DirectionError> {
    if azimuth_steps == 0 || elevation_steps == 0 {
        return Err(DirectionError::DegenerateGrid);
    }
    if !aim.voxels.iter().any(|v| *v) {
        return Err(DirectionError::EmptyAimMask);
    }
    let centroid = mask_centroid_mm(geometry, aim);
    let mut candidates = Vec::with_capacity((azimuth_steps * elevation_steps) as usize);
    for el in 0..elevation_steps {
        // −60..+60° elevation, inclusive of both ends when >1 step.
        let elevation_deg = if elevation_steps == 1 {
            0.0
        } else {
            -60.0 + 120.0 * el as f64 / (elevation_steps - 1) as f64
        };
        let el_rad = elevation_deg.to_radians();
        for az in 0..azimuth_steps {
            let azimuth_deg = 360.0 * az as f64 / azimuth_steps as f64;
            let az_rad = azimuth_deg.to_radians();
            // Beam points *from* the entry surface *toward* the
            // centroid: direction is the inward normal of the sphere
            // point (azimuth/elevation on the treatment circle).
            let direction = [
                -az_rad.cos() * el_rad.cos(),
                -az_rad.sin() * el_rad.cos(),
                -el_rad.sin(),
            ];
            let name = format!("az{:03}-el{:+03}", azimuth_deg as i32, elevation_deg as i32);
            let tissue_path = body.map(|mask| tissue_path_mm(geometry, mask, centroid, direction));
            candidates.push(DirectionCandidate {
                name,
                direction_lps: direction,
                azimuth_deg,
                elevation_deg,
                tissue_path_mm: tissue_path,
                adjoint_score: None,
            });
        }
    }
    // Shallowest incidence first — the transport-worthiness ranking.
    candidates.sort_by(|a, b| {
        a.tissue_path_mm
            .partial_cmp(&b.tissue_path_mm)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(candidates)
}

/// Format candidates as `plan fields --beam` argument lines —
/// `name,dx,dy,dz` — over the top `count` ranked directions.
pub fn beam_spec_lines(candidates: &[DirectionCandidate], count: usize) -> Vec<String> {
    candidates
        .iter()
        .take(count)
        .map(|c| {
            format!(
                "{},{:.6},{:.6},{:.6}",
                c.name, c.direction_lps[0], c.direction_lps[1], c.direction_lps[2]
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> GridGeometry {
        GridGeometry {
            shape: [10, 10, 10],
            spacing_mm: [1.0, 1.0, 1.0],
            origin_mm: [0.0, 0.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    fn cube_mask(lo: usize, hi: usize) -> RegionMask {
        let mut voxels = vec![false; 1000];
        for k in lo..hi {
            for j in lo..hi {
                for i in lo..hi {
                    voxels[i + 10 * j + 100 * k] = true;
                }
            }
        }
        RegionMask {
            name: "t".into(),
            voxels,
        }
    }

    #[test]
    fn enumerates_and_ranks_by_depth() {
        let aim = cube_mask(4, 6);
        let body = cube_mask(0, 10);
        let candidates = enumerate_directions(&geometry(), &aim, Some(&body), 8, 3).unwrap();
        assert_eq!(candidates.len(), 24);
        // Sorted shallowest-first; every score is Some and finite.
        let paths: Vec<f64> = candidates
            .iter()
            .map(|c| c.tissue_path_mm.unwrap())
            .collect();
        assert!(paths.windows(2).all(|w| w[0] <= w[1]));
        // A direction from the ±y face crosses ~4-6 mm less/more tissue
        // symmetric — scores must be positive and under the diagonal.
        assert!(paths.iter().all(|p| *p > 0.0 && *p < 20.0));
    }

    #[test]
    fn beam_specs_are_fields_compatible() {
        let aim = cube_mask(4, 6);
        let candidates = enumerate_directions(&geometry(), &aim, None, 4, 1).unwrap();
        let lines = beam_spec_lines(&candidates, 2);
        assert_eq!(lines.len(), 2);
        let parts: Vec<&str> = lines[0].split(',').collect();
        assert_eq!(parts.len(), 4);
        assert!(parts[1].parse::<f64>().is_ok());
    }

    /// R10-04 acceptance: the sweep reproduces a known optimum. An aim
    /// mask pressed against the +x face of a full body must rank the
    /// direction entering through that face first — azimuth 0 aims
    /// the beam −x from the +x side, crossing ~1 mm of tissue instead
    /// of ~9 mm from −x.
    #[test]
    fn sweep_recovers_known_optimum() {
        let mut aim_voxels = vec![false; 1000];
        aim_voxels[8 + 10 * 5 + 100 * 5] = true;
        let aim = RegionMask {
            name: "aim".into(),
            voxels: aim_voxels,
        };
        let body = cube_mask(0, 10);
        let candidates = enumerate_directions(&geometry(), &aim, Some(&body), 8, 1).unwrap();
        // Only azimuths sweep — elevation pinned at 0 — so the optimum
        // is exactly az000: the −x beam enters at the +x face, one
        // voxel from the aim.
        assert_eq!(candidates[0].name, "az000-el+00");
        assert!(candidates[0].tissue_path_mm.unwrap() < 3.0);
        // The diametrically opposite direction pays the full transit.
        let worst = candidates.last().unwrap();
        assert!(worst.tissue_path_mm.unwrap() > 7.0);
        let document = build_direction_candidates_document(
            "sweep",
            "case",
            candidates.clone(),
            "tissue_path_length",
            8,
            1,
            None,
            ContentReference {
                id: "case".into(),
                sha256: "0".repeat(64),
            },
            ContentReference {
                id: "aim".into(),
                sha256: "1".repeat(64),
            },
            Some(ContentReference {
                id: "body".into(),
                sha256: "2".repeat(64),
            }),
            None,
            "sweep-prov",
        );
        assert_eq!(document.scoring, "tissue_path_length");
        assert_eq!(document.candidates.len(), 8);
        assert_eq!(document.qualification, DIRECTION_CANDIDATES_QUALIFICATION);
    }

    #[test]
    fn empty_aim_is_rejected() {
        let aim = RegionMask {
            name: "empty".into(),
            voxels: vec![false; 1000],
        };
        assert!(matches!(
            enumerate_directions(&geometry(), &aim, None, 4, 1),
            Err(DirectionError::EmptyAimMask)
        ));
    }
}
