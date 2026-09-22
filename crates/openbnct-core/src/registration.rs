// SPDX-License-Identifier: MIT

//! Rigid co-registration contracts (`openbnct.registration/0.1.0`).
//!
//! A `RigidTransform` maps patient-space LPS points from a moving image's
//! frame into the fixed (target) frame: `p_fixed = R·p_moving + t` with
//! `R` orthonormal and `det R = +1`. Because a [`GridGeometry`]'s
//! direction matrix and origin are patient-space quantities, applying the
//! transform to a volume reduces to transforming its geometry —
//! `D' = R·D`, `o' = R·o + t` — after which ordinary world-space
//! resampling (e.g. `openbnct-nifti`'s `resample_to_grid`) places the
//! moving field onto the fixed grid. No voxel warping code is needed and
//! oblique grids stay exact.
//!
//! A `Registration` document binds the transform to the method that
//! produced it — closed-form landmark least squares or an
//! operator-declared external transform — plus the landmark pairs and the
//! achieved RMS residual, so the record carries the evidence for its own
//! accuracy rather than asserting it.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ContentReference, GridGeometry, ValidationError};

/// Current schema token for registration documents.
pub const REGISTRATION_SCHEMA: &str = "openbnct.registration/0.1.0";

/// Orthonormality tolerance for a declared or fitted rotation matrix.
/// Loose enough to admit transforms transcribed from external systems
/// (which carry ~6 significant digits) while still rejecting scaling,
/// shear, and reflections outright.
const ROTATION_TOLERANCE: f64 = 1.0e-4;

/// A rigid transform mapping moving-frame LPS points to fixed-frame LPS:
/// `p_fixed = R·p_moving + t`. `rotation` is row-major.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigidTransform {
    /// Row-major 3×3 rotation (orthonormal, determinant +1).
    pub rotation: [f64; 9],
    /// Translation in millimetres, applied after rotation.
    pub translation_mm: [f64; 3],
}

impl RigidTransform {
    pub fn identity() -> Self {
        Self {
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            translation_mm: [0.0; 3],
        }
    }

    /// Orthonormal rows/columns to `ROTATION_TOLERANCE` and determinant
    /// +1 — a reflection or scaling is not a rigid transform.
    pub fn validate(&self) -> Result<(), RegistrationError> {
        let r = &self.rotation;
        if r.iter()
            .chain(self.translation_mm.iter())
            .any(|v| !v.is_finite())
        {
            return Err(RegistrationError::InvalidTransform(
                "transform contains non-finite values".into(),
            ));
        }
        let dot = |a: [f64; 3], b: [f64; 3]| a[0].mul_add(b[0], a[1].mul_add(b[1], a[2] * b[2]));
        let columns = |c: usize| [r[c], r[3 + c], r[6 + c]];
        let (c0, c1, c2) = (columns(0), columns(1), columns(2));
        if (dot(c0, c0) - 1.0).abs() > ROTATION_TOLERANCE
            || (dot(c1, c1) - 1.0).abs() > ROTATION_TOLERANCE
            || (dot(c2, c2) - 1.0).abs() > ROTATION_TOLERANCE
            || dot(c0, c1).abs() > ROTATION_TOLERANCE
            || dot(c0, c2).abs() > ROTATION_TOLERANCE
            || dot(c1, c2).abs() > ROTATION_TOLERANCE
        {
            return Err(RegistrationError::InvalidTransform(
                "rotation is not orthonormal within tolerance".into(),
            ));
        }
        let determinant = r[0] * (r[4] * r[8] - r[5] * r[7]) - r[1] * (r[3] * r[8] - r[5] * r[6])
            + r[2] * (r[3] * r[7] - r[4] * r[6]);
        if (determinant - 1.0).abs() > ROTATION_TOLERANCE {
            return Err(RegistrationError::InvalidTransform(format!(
                "rotation determinant {determinant} is not +1 (reflection or scale)"
            )));
        }
        Ok(())
    }

    /// `p_fixed = R·p + t`.
    pub fn apply(&self, point: [f64; 3]) -> [f64; 3] {
        let r = &self.rotation;
        [
            r[0].mul_add(point[0], r[1].mul_add(point[1], r[2] * point[2]))
                + self.translation_mm[0],
            r[3].mul_add(point[0], r[4].mul_add(point[1], r[5] * point[2]))
                + self.translation_mm[1],
            r[6].mul_add(point[0], r[7].mul_add(point[1], r[8] * point[2]))
                + self.translation_mm[2],
        ]
    }

    /// The rigid inverse: `Rᵀ`, `−Rᵀt`.
    pub fn inverse(&self) -> Self {
        let r = &self.rotation;
        let rt = [r[0], r[3], r[6], r[1], r[4], r[7], r[2], r[5], r[8]];
        let t = self.translation_mm;
        Self {
            rotation: rt,
            translation_mm: [
                -(rt[0] * t[0] + rt[1] * t[1] + rt[2] * t[2]),
                -(rt[3] * t[0] + rt[4] * t[1] + rt[5] * t[2]),
                -(rt[6] * t[0] + rt[7] * t[1] + rt[8] * t[2]),
            ],
        }
    }

    /// `outer ∘ inner`: apply `inner` first, then `outer`.
    pub fn compose(outer: &Self, inner: &Self) -> Self {
        let (ro, ri) = (&outer.rotation, &inner.rotation);
        let mut rotation = [0.0; 9];
        for row in 0..3 {
            for col in 0..3 {
                rotation[row * 3 + col] = ro[row * 3] * ri[col]
                    + ro[row * 3 + 1] * ri[3 + col]
                    + ro[row * 3 + 2] * ri[6 + col];
            }
        }
        Self {
            rotation,
            translation_mm: outer.apply_from_rotation(inner.translation_mm),
        }
    }

    fn apply_from_rotation(&self, point: [f64; 3]) -> [f64; 3] {
        self.apply(point)
    }

    /// Transform a voxel grid's patient-space frame: `D' = R·D` on the
    /// row-major direction and `o' = R·o + t` on the origin. The result
    /// stays a valid orthonormal `GridGeometry` because R is.
    pub fn apply_to_geometry(&self, geometry: &GridGeometry) -> GridGeometry {
        let d = &geometry.direction;
        let r = &self.rotation;
        let mut direction = [0.0; 9];
        for row in 0..3 {
            for col in 0..3 {
                direction[row * 3 + col] =
                    r[row * 3] * d[col] + r[row * 3 + 1] * d[3 + col] + r[row * 3 + 2] * d[6 + col];
            }
        }
        GridGeometry {
            shape: geometry.shape,
            spacing_mm: geometry.spacing_mm,
            origin_mm: self.apply(geometry.origin_mm),
            direction,
        }
    }
}

/// One named or anonymous correspondence: a point in the moving image's
/// patient space and its matching point in the fixed image's space, both
/// LPS millimetres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LandmarkPair {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub moving_lps_mm: [f64; 3],
    pub fixed_lps_mm: [f64; 3],
}

/// How the transform was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegistrationMethod {
    /// Closed-form least-squares fit over paired landmarks (Horn's
    /// quaternion method). Requires `landmarks` and `rms_residual_mm`.
    LandmarkLeastSquares,
    /// Operator-declared transform transcribed from an external
    /// registration (e.g. a TPS or third-party tool's matrix). The
    /// record carries no residual evidence — accuracy is the declared
    /// source's responsibility.
    Declared,
}

/// A versioned registration record: the transform, its method, the
/// landmarks that produced it, the achieved residual, and content
/// bindings of the moving/fixed images when known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    #[serde(deserialize_with = "crate::deserialize_contract_id")]
    pub schema_version: String,
    pub id: String,
    /// Content binding of the moving image artifact, when encoded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moving: Option<ContentReference>,
    /// Content binding of the fixed (target) image artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed: Option<ContentReference>,
    pub transform: RigidTransform,
    pub method: RegistrationMethod,
    /// The landmark pairs actually used by a landmark fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landmarks: Option<Vec<LandmarkPair>>,
    /// RMS point residual of the fit in millimetres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rms_residual_mm: Option<f64>,
    /// Free-text provenance note (fiducial system, external tool).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Registration {
    pub fn validate(&self) -> Result<(), RegistrationError> {
        if !crate::schema_matches(&self.schema_version, REGISTRATION_SCHEMA) {
            return Err(RegistrationError::UnsupportedSchema(
                self.schema_version.clone(),
            ));
        }
        if self.id.trim().is_empty() {
            return Err(RegistrationError::Invalid(
                "registration id is empty".into(),
            ));
        }
        self.transform.validate()?;
        for reference in [&self.moving, &self.fixed].into_iter().flatten() {
            reference
                .validate()
                .map_err(|_| RegistrationError::Invalid("image reference is invalid".into()))?;
        }
        match self.method {
            RegistrationMethod::LandmarkLeastSquares => {
                let landmarks = self.landmarks.as_ref().ok_or_else(|| {
                    RegistrationError::Invalid(
                        "landmark_least_squares records must carry the landmark pairs".into(),
                    )
                })?;
                if landmarks.len() < 3 {
                    return Err(RegistrationError::Invalid(
                        "landmark_least_squares requires at least three pairs".into(),
                    ));
                }
                for pair in landmarks {
                    if pair
                        .moving_lps_mm
                        .iter()
                        .chain(pair.fixed_lps_mm.iter())
                        .any(|v| !v.is_finite())
                    {
                        return Err(RegistrationError::Invalid(
                            "landmark coordinates must be finite".into(),
                        ));
                    }
                }
                if self
                    .rms_residual_mm
                    .is_none_or(|rms| !rms.is_finite() || rms < 0.0)
                {
                    return Err(RegistrationError::Invalid(
                        "landmark_least_squares requires a finite non-negative rms_residual_mm"
                            .into(),
                    ));
                }
            }
            RegistrationMethod::Declared => {
                if self.landmarks.is_some() || self.rms_residual_mm.is_some() {
                    return Err(RegistrationError::Invalid(
                        "declared registrations carry no landmark evidence".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Fit the rigid transform minimizing Σ|f_i − (R·m_i + t)|² over paired
/// landmarks, returning the transform and RMS residual in mm.
///
/// Horn's closed-form quaternion method: the optimum rotation is the
/// largest-eigenvalue eigenvector of the symmetric 4×4 matrix built from
/// the centered cross-dispersion `S = Σ m·fᵀ`; `t = f̄ − R·m̄`. Requires
/// at least three non-degenerate pairs — coincident or collinear point
/// sets cannot determine a unique rotation.
pub fn fit_landmark_transform(
    landmarks: &[LandmarkPair],
) -> Result<(RigidTransform, f64), RegistrationError> {
    if landmarks.len() < 3 {
        return Err(RegistrationError::DegenerateLandmarks(
            "at least three landmark pairs are required",
        ));
    }
    for (index, pair) in landmarks.iter().enumerate() {
        if pair
            .moving_lps_mm
            .iter()
            .chain(pair.fixed_lps_mm.iter())
            .any(|v| !v.is_finite())
        {
            return Err(RegistrationError::Invalid(format!(
                "landmark {index} contains non-finite coordinates"
            )));
        }
    }
    let n = landmarks.len() as f64;
    let centroid = |pick: fn(&LandmarkPair) -> [f64; 3]| -> [f64; 3] {
        let mut c = [0.0; 3];
        for pair in landmarks {
            let p = pick(pair);
            for axis in 0..3 {
                c[axis] += p[axis];
            }
        }
        [c[0] / n, c[1] / n, c[2] / n]
    };
    let moving_centroid = centroid(|p| p.moving_lps_mm);
    let fixed_centroid = centroid(|p| p.fixed_lps_mm);

    let centered: Vec<([f64; 3], [f64; 3])> = landmarks
        .iter()
        .map(|pair| {
            let m = pair.moving_lps_mm;
            let f = pair.fixed_lps_mm;
            (
                [
                    m[0] - moving_centroid[0],
                    m[1] - moving_centroid[1],
                    m[2] - moving_centroid[2],
                ],
                [
                    f[0] - fixed_centroid[0],
                    f[1] - fixed_centroid[1],
                    f[2] - fixed_centroid[2],
                ],
            )
        })
        .collect();

    // Degeneracy: the moving points must span 3D. Find the most distant
    // pair, then require a point farther than a tolerance from that line
    // (tolerance scales with the inter-point distance).
    let mut max_d2 = 0.0;
    let (mut a, mut b) = ([0.0; 3], [0.0; 3]);
    for (m0, _) in &centered {
        for (m1, _) in &centered {
            let d2 = (0..3).map(|ax| (m1[ax] - m0[ax]).powi(2)).sum::<f64>();
            if d2 > max_d2 {
                max_d2 = d2;
                a = *m0;
                b = *m1;
            }
        }
    }
    let line_len = max_d2.sqrt();
    if line_len <= f64::EPSILON {
        return Err(RegistrationError::DegenerateLandmarks(
            "moving landmarks are coincident",
        ));
    }
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let mut max_area2 = 0.0;
    for (m, _) in &centered {
        let am = [m[0] - a[0], m[1] - a[1], m[2] - a[2]];
        let cross = [
            am[1] * ab[2] - am[2] * ab[1],
            am[2] * ab[0] - am[0] * ab[2],
            am[0] * ab[1] - am[1] * ab[0],
        ];
        let area2: f64 = cross.iter().map(|v| v * v).sum();
        if area2 > max_area2 {
            max_area2 = area2;
        }
    }
    // |am × ab| = distance-to-line × |ab|; require a nonzero perpendicular
    // distance relative to the point spread.
    if max_area2.sqrt() <= line_len * line_len * 1.0e-9 {
        return Err(RegistrationError::DegenerateLandmarks(
            "moving landmarks are collinear",
        ));
    }

    // Cross-dispersion S = Σ m·fᵀ (row-major), then Horn's 4×4 symmetric
    // quaternion matrix.
    let mut s = [[0.0; 3]; 3];
    for (m, f) in &centered {
        for row in 0..3 {
            for col in 0..3 {
                s[row][col] += m[row] * f[col];
            }
        }
    }
    let trace = s[0][0] + s[1][1] + s[2][2];
    let n_matrix = [
        [
            trace,
            s[1][2] - s[2][1],
            s[2][0] - s[0][2],
            s[0][1] - s[1][0],
        ],
        [
            s[1][2] - s[2][1],
            s[0][0] - s[1][1] - s[2][2],
            s[0][1] + s[1][0],
            s[2][0] + s[0][2],
        ],
        [
            s[2][0] - s[0][2],
            s[0][1] + s[1][0],
            -s[0][0] + s[1][1] - s[2][2],
            s[1][2] + s[2][1],
        ],
        [
            s[0][1] - s[1][0],
            s[2][0] + s[0][2],
            s[1][2] + s[2][1],
            -s[0][0] - s[1][1] + s[2][2],
        ],
    ];
    let q = largest_eigenvector_symmetric_4(&n_matrix).ok_or(
        RegistrationError::DegenerateLandmarks("landmark fit did not converge"),
    )?;
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    let rotation = [
        w.mul_add(w, x * x) - y.mul_add(y, z * z),
        2.0 * x.mul_add(y, -(w * z)),
        2.0 * x.mul_add(z, w * y),
        2.0 * x.mul_add(y, w * z),
        w.mul_add(w, y * y) - x.mul_add(x, z * z),
        2.0 * y.mul_add(z, -(w * x)),
        2.0 * x.mul_add(z, -(w * y)),
        2.0 * y.mul_add(z, w * x),
        w.mul_add(w, z * z) - x.mul_add(x, y * y),
    ];
    let transform = RigidTransform {
        rotation,
        translation_mm: [0.0; 3],
    };
    let rotated_centroid = transform.apply(moving_centroid);
    let transform = RigidTransform {
        rotation,
        translation_mm: [
            fixed_centroid[0] - rotated_centroid[0],
            fixed_centroid[1] - rotated_centroid[1],
            fixed_centroid[2] - rotated_centroid[2],
        ],
    };
    transform.validate()?;

    let rms = (landmarks
        .iter()
        .map(|pair| {
            let fitted = transform.apply(pair.moving_lps_mm);
            (0..3)
                .map(|axis| (fitted[axis] - pair.fixed_lps_mm[axis]).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        / n)
        .sqrt();
    Ok((transform, rms))
}

/// Largest-eigenvalue eigenvector of a symmetric 4×4 matrix via cyclic
/// Jacobi sweeps — sufficient at this size and free of external
/// eigensolvers. Returns the normalized quaternion `[w, x, y, z]`.
fn largest_eigenvector_symmetric_4(n: &[[f64; 4]; 4]) -> Option<[f64; 4]> {
    let mut a = *n;
    let mut v = [[0.0; 4]; 4];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    let scale: f64 = a
        .iter()
        .flat_map(|row| row.iter())
        .map(|v| v.abs())
        .fold(0.0, f64::max)
        .max(1.0);
    for _sweep in 0..64 {
        // Largest off-diagonal element; stop when it is numerically
        // exhausted relative to the matrix scale.
        let mut max_off = 0.0;
        let (mut p, mut q) = (0, 1);
        for (i, row) in a.iter().enumerate() {
            for (j, value) in row.iter().enumerate().skip(i + 1) {
                if value.abs() > max_off {
                    max_off = value.abs();
                    p = i;
                    q = j;
                }
            }
        }
        if max_off <= 1.0e-14 * scale {
            break;
        }
        let app = a[p][p];
        let aqq = a[q][q];
        let apq = a[p][q];
        // Jacobi angle zeroing the (p,q) element under A ← JᵀAJ with
        // J = [[c, s], [−s, c]]: tan 2θ = 2a_pq/(a_qq − a_pp).
        let theta = 0.5 * (2.0 * apq).atan2(aqq - app);
        let (c, s) = (theta.cos(), theta.sin());
        for row in a.iter_mut() {
            let (akp, akq) = (row[p], row[q]);
            row[p] = c * akp - s * akq;
            row[q] = s * akp + c * akq;
        }
        let (before, after) = a.split_at_mut(q);
        let (row_p, row_q) = (&mut before[p], &mut after[0]);
        for (apk, aqk) in row_p.iter_mut().zip(row_q.iter_mut()) {
            let (x, y) = (*apk, *aqk);
            *apk = c * x - s * y;
            *aqk = s * x + c * y;
        }
        for row in v.iter_mut() {
            let (vkp, vkq) = (row[p], row[q]);
            row[p] = c * vkp - s * vkq;
            row[q] = s * vkp + c * vkq;
        }
    }
    let (mut best, mut best_value) = (0, f64::NEG_INFINITY);
    for (i, row) in a.iter().enumerate() {
        if row[i] > best_value {
            best_value = row[i];
            best = i;
        }
    }
    if !best_value.is_finite() {
        return None;
    }
    let column = [v[0][best], v[1][best], v[2][best], v[3][best]];
    let norm: f64 = column.iter().map(|x| x * x).sum::<f64>().sqrt();
    if !matches!(norm.partial_cmp(&0.0), Some(std::cmp::Ordering::Greater)) {
        return None;
    }
    let mut q = [
        column[0] / norm,
        column[1] / norm,
        column[2] / norm,
        column[3] / norm,
    ];
    // Sign convention: w ≥ 0 keeps the quaternion canonical.
    if q[0] < 0.0 {
        for component in &mut q {
            *component = -*component;
        }
    }
    Some(q)
}

/// Build a validated landmark-fit registration document.
pub fn landmark_registration(
    id: impl Into<String>,
    moving: Option<ContentReference>,
    fixed: Option<ContentReference>,
    landmarks: Vec<LandmarkPair>,
    note: Option<String>,
) -> Result<Registration, RegistrationError> {
    let (transform, rms) = fit_landmark_transform(&landmarks)?;
    let registration = Registration {
        schema_version: REGISTRATION_SCHEMA.into(),
        id: id.into(),
        moving,
        fixed,
        transform,
        method: RegistrationMethod::LandmarkLeastSquares,
        landmarks: Some(landmarks),
        rms_residual_mm: Some(rms),
        note,
    };
    registration.validate()?;
    Ok(registration)
}

/// Build a validated operator-declared registration document.
pub fn declared_registration(
    id: impl Into<String>,
    moving: Option<ContentReference>,
    fixed: Option<ContentReference>,
    transform: RigidTransform,
    note: Option<String>,
) -> Result<Registration, RegistrationError> {
    let registration = Registration {
        schema_version: REGISTRATION_SCHEMA.into(),
        id: id.into(),
        moving,
        fixed,
        transform,
        method: RegistrationMethod::Declared,
        landmarks: None,
        rms_residual_mm: None,
        note,
    };
    registration.validate()?;
    Ok(registration)
}

#[derive(Debug, Error)]
pub enum RegistrationError {
    #[error("unsupported registration schema {0:?}")]
    UnsupportedSchema(String),
    #[error("invalid registration: {0}")]
    Invalid(String),
    #[error("invalid rigid transform: {0}")]
    InvalidTransform(String),
    #[error("degenerate landmark set: {0}")]
    DegenerateLandmarks(&'static str),
    #[error("invalid geometry: {0}")]
    InvalidGeometry(#[from] ValidationError),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landmarks(points: &[[f64; 3]], transform: &RigidTransform) -> Vec<LandmarkPair> {
        points
            .iter()
            .map(|p| LandmarkPair {
                name: None,
                moving_lps_mm: transform.apply(*p),
                fixed_lps_mm: *p,
            })
            .collect()
    }

    fn phantom_points() -> Vec<[f64; 3]> {
        vec![
            [0.0, 0.0, 0.0],
            [80.0, 0.0, 0.0],
            [0.0, 90.0, 0.0],
            [0.0, 0.0, 70.0],
            [40.0, 30.0, 50.0],
            [-20.0, 60.0, 10.0],
        ]
    }

    /// 30° about z then a translation — a realistic table shift.
    fn known_transform() -> RigidTransform {
        let (c, s) = (30.0_f64.to_radians().cos(), 30.0_f64.to_radians().sin());
        RigidTransform {
            rotation: [c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0],
            translation_mm: [12.0, -7.5, 4.0],
        }
    }

    #[test]
    fn exact_landmarks_recover_the_transform() {
        let known = known_transform();
        // Fit maps moving→fixed; the landmarks were built with fixed→moving.
        let pairs = landmarks(&phantom_points(), &known);
        let (fit, rms) = fit_landmark_transform(&pairs).unwrap();
        let expected = known.inverse();
        for i in 0..9 {
            assert!((fit.rotation[i] - expected.rotation[i]).abs() < 1e-9);
        }
        for i in 0..3 {
            assert!((fit.translation_mm[i] - expected.translation_mm[i]).abs() < 1e-9);
        }
        assert!(rms < 1e-9, "rms {rms}");
    }

    #[test]
    fn arbitrary_rotation_and_translation_recover() {
        // Compose two rotations for a non-axial R, then translate.
        let (c1, s1) = (0.35_f64.cos(), 0.35_f64.sin());
        let (c2, s2) = (0.6_f64.cos(), 0.6_f64.sin());
        let rz = RigidTransform {
            rotation: [c1, -s1, 0.0, s1, c1, 0.0, 0.0, 0.0, 1.0],
            translation_mm: [0.0; 3],
        };
        let ry = RigidTransform {
            rotation: [c2, 0.0, s2, 0.0, 1.0, 0.0, -s2, 0.0, c2],
            translation_mm: [0.0; 3],
        };
        let known = RigidTransform {
            rotation: RigidTransform::compose(&rz, &ry).rotation,
            translation_mm: [-30.0, 15.0, 22.5],
        };
        let pairs = landmarks(&phantom_points(), &known);
        let (fit, _) = fit_landmark_transform(&pairs).unwrap();
        let probe = [11.0, -23.0, 47.0];
        let want = known.inverse().apply(probe);
        let got = fit.apply(probe);
        for axis in 0..3 {
            assert!((got[axis] - want[axis]).abs() < 1e-8);
        }
    }

    #[test]
    fn degenerate_landmark_sets_are_rejected() {
        let pairs = landmarks(&phantom_points()[..2], &known_transform());
        assert!(matches!(
            fit_landmark_transform(&pairs),
            Err(RegistrationError::DegenerateLandmarks(_))
        ));
        let coincident = vec![
            LandmarkPair {
                name: None,
                moving_lps_mm: [5.0, 5.0, 5.0],
                fixed_lps_mm: [0.0, 0.0, 0.0],
            },
            LandmarkPair {
                name: None,
                moving_lps_mm: [5.0, 5.0, 5.0],
                fixed_lps_mm: [1.0, 0.0, 0.0],
            },
            LandmarkPair {
                name: None,
                moving_lps_mm: [5.0, 5.0, 5.0],
                fixed_lps_mm: [0.0, 1.0, 0.0],
            },
        ];
        assert!(matches!(
            fit_landmark_transform(&coincident),
            Err(RegistrationError::DegenerateLandmarks(_))
        ));
        // Collinear moving points cannot determine the axial rotation.
        let collinear: Vec<LandmarkPair> = (0..4)
            .map(|i| LandmarkPair {
                name: None,
                moving_lps_mm: [i as f64 * 10.0, 0.0, 0.0],
                fixed_lps_mm: [i as f64 * 10.0, 0.0, 0.0],
            })
            .collect();
        assert!(matches!(
            fit_landmark_transform(&collinear),
            Err(RegistrationError::DegenerateLandmarks(_))
        ));
    }

    #[test]
    fn transform_inverse_and_compose_round_trip() {
        let t = known_transform();
        let p = [33.0, -14.0, 25.0];
        let round = t.inverse().apply(t.apply(p));
        for axis in 0..3 {
            assert!((round[axis] - p[axis]).abs() < 1e-9);
        }
        let composed = RigidTransform::compose(&t, &t.inverse());
        for i in 0..9 {
            let want = if i % 4 == 0 { 1.0 } else { 0.0 };
            assert!((composed.rotation[i] - want).abs() < 1e-9);
        }
        assert!(composed.translation_mm.iter().all(|v| v.abs() < 1e-9));
    }

    #[test]
    fn declared_transforms_must_be_rigid() {
        let mut t = RigidTransform::identity();
        t.rotation[0] = 2.0; // scaling, not rotation
        assert!(t.validate().is_err());
        let reflection = RigidTransform {
            rotation: [-1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            translation_mm: [0.0; 3],
        };
        assert!(reflection.validate().is_err());
    }

    #[test]
    fn geometry_transform_preserves_voxel_mapping() {
        // A point at a moving voxel center lands where the transformed
        // geometry's same-index center sits.
        let geometry = GridGeometry {
            shape: [4, 4, 4],
            spacing_mm: [2.0, 3.0, 4.0],
            origin_mm: [-10.0, -5.0, 0.0],
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        };
        let t = known_transform();
        let moved = t.apply_to_geometry(&geometry);
        moved.voxel_count().unwrap(); // still orthonormal
        let original = geometry.voxel_center_lps_mm([3, 2, 1]).unwrap();
        let transformed = moved.voxel_center_lps_mm([3, 2, 1]).unwrap();
        let want = t.apply(original);
        for axis in 0..3 {
            assert!((transformed[axis] - want[axis]).abs() < 1e-9);
        }
    }

    #[test]
    fn registration_documents_validate_method_evidence() {
        let pairs = landmarks(&phantom_points(), &known_transform());
        let registration = landmark_registration("reg.test", None, None, pairs, None).unwrap();
        registration.validate().unwrap();
        assert!(registration.rms_residual_mm.unwrap() < 1e-9);

        let mut declared = declared_registration(
            "reg.declared",
            None,
            None,
            known_transform(),
            Some("external TPS matrix".into()),
        )
        .unwrap();
        declared.validate().unwrap();
        // A declared record must not carry fabricated landmark evidence.
        declared.landmarks = Some(vec![]);
        assert!(declared.validate().is_err());
    }
}
