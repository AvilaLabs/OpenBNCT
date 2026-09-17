// SPDX-License-Identifier: Apache-2.0

//! NIfTI-1 image import and export with explicit affine semantics.
//!
//! Scope is deliberately bounded: `.nii` single-file volumes (`n+1` magic),
//! optionally gzip-compressed (`.nii.gz`), with a 3-dimensional spatial
//! extent, scalar datatypes
//! (u8, i16, i32, f32, f64), and one declared world transform — the sform
//! is preferred over the qform when both are present, per the NIfTI-1
//! standard's precedence rule. NIfTI's world frame is RAS+; the platform's
//! `GridGeometry` is patient LPS, so import flips the world x and y axes
//! and export flips them back. Voxel indices address voxel *centers* in
//! both conventions, matching `GridGeometry::origin_mm`.
//!
//! Resampling to a case grid is offered with explicitly declared
//! interpolation (`Nearest` for masks/labels, `Trilinear` for dose) —
//! trilinear values are never silently used as masks.

#![forbid(unsafe_code)]

use std::io;
use std::path::Path;

use openbnct_core::{
    ComponentDoseInterchange, DoseComponent, DoseUnit, DoseVolume, ExternalProducer, ExternalTotal,
    GridGeometry, RegionMask, grid_geometry_equivalent,
};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use thiserror::Error;

const HEADER_LEN: usize = 348;
const MAGIC_OFFSET: usize = 344;
const DT_UINT8: i16 = 2;
const DT_INT16: i16 = 4;
const DT_INT32: i16 = 8;
const DT_FLOAT32: i16 = 16;
const DT_FLOAT64: i16 = 64;

/// How voxels are interpolated when resampling onto another grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    /// Nearest voxel center — required for masks and label images.
    Nearest,
    /// Trilinear interpolation — appropriate for dose or intensity.
    Trilinear,
}

/// A decoded NIfTI-1 volume on a `GridGeometry` expressed in patient LPS
/// millimeters (converted from the file's RAS+ world frame).
#[derive(Debug, Clone, PartialEq)]
pub struct NiftiImage {
    /// Grid geometry in patient LPS coordinates. `direction` is the
    /// normalized affine basis, so oblique volumes are preserved.
    pub geometry: GridGeometry,
    /// Scalar values after `scl_slope`/`scl_inter` application, in grid
    /// order `i + nx*j + nx*ny*k`.
    pub values: Vec<f64>,
    /// The file's NIfTI datatype code, recorded for provenance.
    pub datatype: i16,
    /// Which stored transform supplied the affine: `sform` or `qform`.
    pub transform_source: &'static str,
    /// `descrip` header field, trimmed.
    pub description: String,
    /// `intent_name` header field, trimmed.
    pub intent_name: String,
    /// Spatial unit semantics: `true` when the file declared units
    /// explicitly (`xyzt_units & 7 == 2`), `false` when units were
    /// unspecified (0) and millimeters were assumed. Declared non-mm
    /// units are refused at read time.
    pub units_declared_mm: bool,
}

/// Parse one `.nii` or `.nii.gz` file.
pub fn read_nifti_file(path: &Path) -> Result<NiftiImage, NiftiError> {
    read_nifti(&std::fs::read(path)?)
}

/// Parse NIfTI-1 bytes; a leading gzip wrapper (`.nii.gz`) is transparently
/// decompressed.
pub fn read_nifti(bytes: &[u8]) -> Result<NiftiImage, NiftiError> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        use io::Read;
        let mut decompressed = Vec::new();
        flate2::read::GzDecoder::new(bytes).read_to_end(&mut decompressed)?;
        return read_nifti(&decompressed);
    }
    if bytes.len() < HEADER_LEN + 4 {
        return Err(NiftiError::Truncated);
    }
    let mut header = Header::parse(&bytes[..HEADER_LEN])?;
    if header.magic != *b"n+1\0" {
        return Err(NiftiError::UnsupportedMagic(header.magic));
    }
    if header.dim[0] != 3 || header.dim.iter().skip(4).any(|&d| d > 1) {
        return Err(NiftiError::UnsupportedDimensions(
            header.dim[0],
            header.dim[4],
        ));
    }
    // Spatial units: 0 means unspecified in the wild — accept it but record
    // that millimeters were assumed. Declared non-mm units are refused.
    let units_declared_mm = header.xyzt_units != 0;
    if units_declared_mm && header.xyzt_units != 0b010 {
        return Err(NiftiError::NonMillimeterUnits(header.xyzt_units));
    }
    if header.dim[1..4].iter().any(|&d| d <= 0) {
        return Err(NiftiError::BadHeader);
    }
    let shape: Vec<usize> = header.dim[1..4].iter().map(|&d| d as usize).collect();
    let voxel_count = shape.iter().product::<usize>();
    let start = header.vox_offset as usize;
    let needed = start + voxel_count * header.byte_width()?;
    if bytes.len() < needed {
        return Err(NiftiError::Truncated);
    }
    header.transform_source = if header.sform_code > 0 {
        "sform"
    } else if header.qform_code > 0 {
        "qform"
    } else {
        "none"
    };
    let affine = header.affine()?;
    let mut values = Vec::with_capacity(voxel_count);
    for index in 0..voxel_count {
        let offset = start + index * header.byte_width()?;
        let raw = header.read_scalar(&bytes[offset..])?;
        values.push(header.scl_slope * raw + header.scl_inter);
    }
    Ok(NiftiImage {
        geometry: ras_affine_to_lps(&affine, [shape[0] as u32, shape[1] as u32, shape[2] as u32]),
        values,
        datatype: header.datatype,
        transform_source: header.transform_source,
        description: header.description,
        intent_name: header.intent_name,
        units_declared_mm,
    })
}

/// Serialize a volume as `.nii`, float64. Paths ending in `.nii.gz` are
/// gzip-compressed; all others are written uncompressed.
///
/// `sform_code` is written as 1 (scanner-based anatomical coordinates — a
/// template-aligned code would misrepresent patient geometry), and the
/// sform carries the LPS→RAS conversion of the supplied geometry.
pub fn write_nifti(image: &NiftiImage, path: &Path) -> io::Result<()> {
    let encoded = encode_nifti(image);
    if path.extension().is_some_and(|e| e == "gz") {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use io::Write;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&encoded)?;
        return std::fs::write(path, encoder.finish()?);
    }
    std::fs::write(path, encoded)
}

/// Serialize to bytes. Values are stored as float64 with unit slope.
pub fn encode_nifti(image: &NiftiImage) -> Vec<u8> {
    let shape = image.geometry.shape;
    let mut out = vec![0_u8; 352];
    let put_i32 = |bytes: &mut [u8], offset: usize, value: i32| {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    let put_i16 = |bytes: &mut [u8], offset: usize, value: i16| {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    };
    let put_f32 = |bytes: &mut [u8], offset: usize, value: f32| {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    };
    put_i32(&mut out, 0, HEADER_LEN as i32); // sizeof_hdr
    for (axis, &d) in shape.iter().enumerate() {
        put_i16(&mut out, 42 + 2 * axis, d as i16);
    }
    put_i16(&mut out, 40, 3); // dim[0] = 3
    put_i16(&mut out, 70, DT_FLOAT64);
    put_i16(&mut out, 72, 64); // bitpix
    put_f32(&mut out, 76, 1.0); // pixdim[0] = qfac (unused; sform only)
    for (axis, &s) in image.geometry.spacing_mm.iter().enumerate() {
        put_f32(&mut out, 80 + 4 * axis, s as f32);
    }
    put_f32(&mut out, 108, 352.0); // vox_offset
    put_f32(&mut out, 112, 1.0); // scl_slope
    put_i16(&mut out, 254, 1); // sform_code = 1 (scanner anatomy)
    let affine = lps_to_ras_affine(&image.geometry);
    for (srow, offset) in affine.iter().zip([280_usize, 296, 312]) {
        for (col, &a) in srow.iter().enumerate() {
            put_f32(&mut out, offset + 4 * col, a as f32);
        }
    }
    out[123] = 0b010; // xyzt_units: spatial mm, no time unit
    out[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(b"n+1\0");
    // Bytes 348..352 are the zero extension-flag block already in `out`;
    // voxel data begins at vox_offset 352.
    for value in &image.values {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// Sample `image` onto `target` under the declared interpolation.
///
/// Target voxels whose centers fall outside the source volume return 0.0 —
/// the caller decides whether partial coverage is acceptable; mask
/// consumers should use `to_mask` with `Interpolation::Nearest`.
pub fn resample_to_grid(
    image: &NiftiImage,
    target: &GridGeometry,
    interpolation: Interpolation,
) -> Vec<f64> {
    let [nx, ny, nz] = target.shape.map(|d| d as usize);
    let mut out = vec![0.0; nx * ny * nz];
    for k in 0..nz {
        for j in 0..ny {
            for i in 0..nx {
                let index = i + nx * j + nx * ny * k;
                let world = voxel_center(target, i, j, k);
                let Some(voxel) = world_to_voxel(&image.geometry, world) else {
                    continue;
                };
                out[index] = match interpolation {
                    Interpolation::Nearest => sample_nearest(image, voxel),
                    Interpolation::Trilinear => sample_trilinear(image, voxel),
                };
            }
        }
    }
    out
}

/// Resolve a resample target grid from a transport case
/// (`openbnct.transport-case/*` — the CT-aligned grid) or a dose bundle
/// (`openbnct.physical-dose-bundle/*`, `openbnct.biological-dose-bundle/*`).
/// Both contracts carry `geometry` as a `GridGeometry`; unknown schemas
/// are refused rather than guessed.
pub fn read_target_geometry(path: &Path) -> Result<GridGeometry, NiftiError> {
    let bytes = std::fs::read(path)?;
    let document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| NiftiError::InvalidTarget(format!("{e}")))?;
    let schema = document
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let schema = openbnct_core::normalize_contract_id(schema);
    let supported = schema.starts_with("openbnct.transport-case/")
        || schema.starts_with("openbnct.physical-dose-bundle/")
        || schema.starts_with("openbnct.biological-dose-bundle/");
    if !supported {
        return Err(NiftiError::InvalidTarget(format!(
            "unsupported target schema {schema:?}; expected a transport case or dose bundle"
        )));
    }
    serde_json::from_value(
        document
            .get("geometry")
            .cloned()
            .ok_or_else(|| NiftiError::InvalidTarget("target carries no geometry".into()))?,
    )
    .map_err(|e| NiftiError::InvalidTarget(format!("geometry: {e}")))
}

/// Convert a scalar volume into a `RegionMask`: nonzero voxels are
/// included. Voxel coordinates must align with the target grid — resample
/// with `Interpolation::Nearest` first when the geometries differ.
pub fn to_mask(image: &NiftiImage, name: impl Into<String>) -> RegionMask {
    RegionMask {
        name: name.into(),
        voxels: image.values.iter().map(|v| *v != 0.0).collect(),
    }
}

/// World-space (patient LPS mm) center of voxel (i,j,k) under a geometry.
/// `direction[r*3+c]` is world component r of voxel-axis c's direction, so
/// world = origin + direction @ local_offsets.
pub fn voxel_center(geometry: &GridGeometry, i: usize, j: usize, k: usize) -> [f64; 3] {
    let local = [
        i as f64 * geometry.spacing_mm[0],
        j as f64 * geometry.spacing_mm[1],
        k as f64 * geometry.spacing_mm[2],
    ];
    let d = &geometry.direction;
    [
        geometry.origin_mm[0] + d[0] * local[0] + d[1] * local[1] + d[2] * local[2],
        geometry.origin_mm[1] + d[3] * local[0] + d[4] * local[1] + d[5] * local[2],
        geometry.origin_mm[2] + d[6] * local[0] + d[7] * local[1] + d[8] * local[2],
    ]
}

/// Invert a geometry: world LPS point → continuous voxel coordinates.
/// Returns `None` when the point lies outside the volume.
pub fn world_to_voxel(geometry: &GridGeometry, world: [f64; 3]) -> Option<[f64; 3]> {
    let delta = [
        world[0] - geometry.origin_mm[0],
        world[1] - geometry.origin_mm[1],
        world[2] - geometry.origin_mm[2],
    ];
    let d = &geometry.direction;
    // Orthonormal axes: local_c = delta · column_c / spacing_c.
    let voxel = [
        (d[0] * delta[0] + d[3] * delta[1] + d[6] * delta[2]) / geometry.spacing_mm[0],
        (d[1] * delta[0] + d[4] * delta[1] + d[7] * delta[2]) / geometry.spacing_mm[1],
        (d[2] * delta[0] + d[5] * delta[1] + d[8] * delta[2]) / geometry.spacing_mm[2],
    ];
    let shape = geometry.shape;
    let inside = (0..3)
        .all(|axis| voxel[axis] >= -0.5 && voxel[axis] < shape[axis] as f64 - 0.5 + f64::EPSILON);
    inside.then_some(voxel)
}

fn sample_nearest(image: &NiftiImage, voxel: [f64; 3]) -> f64 {
    let [nx, ny, _] = image.geometry.shape.map(|d| d as usize);
    let i = (voxel[0] + 0.5).floor() as usize;
    let j = (voxel[1] + 0.5).floor() as usize;
    let k = (voxel[2] + 0.5).floor() as usize;
    image.values[i + nx * j + nx * ny * k]
}

fn sample_trilinear(image: &NiftiImage, voxel: [f64; 3]) -> f64 {
    let [nx, ny, nz] = image.geometry.shape.map(|d| d as usize);
    let clamp = |v: f64, max: usize| v.clamp(0.0, max.saturating_sub(1) as f64);
    let x = clamp(voxel[0], nx);
    let y = clamp(voxel[1], ny);
    let z = clamp(voxel[2], nz);
    let (x0, y0, z0) = (x.floor() as usize, y.floor() as usize, z.floor() as usize);
    let (x1, y1, z1) = (
        (x0 + 1).min(nx - 1),
        (y0 + 1).min(ny - 1),
        (z0 + 1).min(nz - 1),
    );
    let (fx, fy, fz) = (x - x0 as f64, y - y0 as f64, z - z0 as f64);
    let at = |i: usize, j: usize, k: usize| image.values[i + nx * j + nx * ny * k];
    let lerp = |a: f64, b: f64, f: f64| a + (b - a) * f;
    let c00 = lerp(at(x0, y0, z0), at(x1, y0, z0), fx);
    let c10 = lerp(at(x0, y1, z0), at(x1, y1, z0), fx);
    let c01 = lerp(at(x0, y0, z1), at(x1, y0, z1), fx);
    let c11 = lerp(at(x0, y1, z1), at(x1, y1, z1), fx);
    lerp(lerp(c00, c10, fy), lerp(c01, c11, fy), fz)
}

/// Convert a NIfTI RAS+ voxel→world affine into a patient-LPS
/// `GridGeometry`: spacing from column norms, direction from normalized
/// columns with x/y flipped, origin = affine applied to voxel index 0.
fn ras_affine_to_lps(affine: &[[f64; 4]; 3], shape: [u32; 3]) -> GridGeometry {
    let mut spacing = [0.0; 3];
    let mut direction = [0.0; 9];
    for col in 0..3 {
        let norm =
            (affine[0][col].powi(2) + affine[1][col].powi(2) + affine[2][col].powi(2)).sqrt();
        spacing[col] = norm;
        for row in 0..3 {
            // Row-major direction; RAS→LPS negates world x and y rows.
            let sign = if row == 2 { 1.0 } else { -1.0 };
            direction[row * 3 + col] = sign * affine[row][col] / norm;
        }
    }
    GridGeometry {
        shape,
        spacing_mm: spacing,
        origin_mm: [-affine[0][3], -affine[1][3], affine[2][3]],
        direction,
    }
}

/// Inverse of `ras_affine_to_lps`: build the RAS+ sform rows.
fn lps_to_ras_affine(geometry: &GridGeometry) -> [[f64; 4]; 3] {
    let mut affine = [[0.0; 4]; 3];
    for (row, out_row) in affine.iter_mut().enumerate() {
        let sign = if row == 2 { 1.0 } else { -1.0 };
        for (col, cell) in out_row.iter_mut().take(3).enumerate() {
            *cell = sign * geometry.direction[row * 3 + col] * geometry.spacing_mm[col];
        }
    }
    affine[0][3] = -geometry.origin_mm[0];
    affine[1][3] = -geometry.origin_mm[1];
    affine[2][3] = geometry.origin_mm[2];
    affine
}

struct Header {
    dim: [i16; 8],
    datatype: i16,
    vox_offset: f64,
    scl_slope: f64,
    scl_inter: f64,
    qform_code: i16,
    sform_code: i16,
    quatern: [f64; 3],
    qoffset: [f64; 3],
    pixdim: [f64; 8],
    srow: [[f64; 4]; 3],
    xyzt_units: u8,
    magic: [u8; 4],
    description: String,
    intent_name: String,
    transform_source: &'static str,
}

impl Header {
    fn parse(bytes: &[u8]) -> Result<Self, NiftiError> {
        let read_i32 =
            |offset: usize| i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let read_i16 =
            |offset: usize| i16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap());
        let read_f32 =
            |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        // sizeof_hdr must be 348; try big-endian as well.
        if read_i32(0) != HEADER_LEN as i32 {
            if i32::from_be_bytes(bytes[0..4].try_into().unwrap()) == HEADER_LEN as i32 {
                return Err(NiftiError::BigEndianUnsupported);
            }
            return Err(NiftiError::BadHeader);
        }
        let mut dim = [0_i16; 8];
        for (i, d) in dim.iter_mut().enumerate() {
            *d = read_i16(40 + 2 * i);
        }
        let mut pixdim = [0.0_f64; 8];
        for (i, p) in pixdim.iter_mut().enumerate() {
            *p = read_f32(76 + 4 * i) as f64;
        }
        let mut srow = [[0.0; 4]; 3];
        for (row, offset) in srow.iter_mut().zip([280_usize, 296, 312]) {
            for (col, cell) in row.iter_mut().enumerate() {
                *cell = read_f32(offset + 4 * col) as f64;
            }
        }
        let description = String::from_utf8_lossy(&bytes[148..228])
            .trim_end_matches('\0')
            .trim()
            .to_owned();
        let intent_name = String::from_utf8_lossy(&bytes[68..70])
            .trim_end_matches('\0')
            .to_owned();
        let mut header = Header {
            dim,
            datatype: read_i16(70),
            vox_offset: read_f32(108) as f64,
            scl_slope: read_f32(112) as f64,
            scl_inter: read_f32(116) as f64,
            qform_code: read_i16(252),
            sform_code: read_i16(254),
            quatern: [
                read_f32(256) as f64,
                read_f32(260) as f64,
                read_f32(264) as f64,
            ],
            qoffset: [
                read_f32(268) as f64,
                read_f32(272) as f64,
                read_f32(276) as f64,
            ],
            pixdim,
            srow,
            xyzt_units: bytes[123] & 0b111,
            magic: bytes[MAGIC_OFFSET..MAGIC_OFFSET + 4].try_into().unwrap(),
            description,
            intent_name,
            transform_source: "none",
        };
        if header.scl_slope == 0.0 {
            header.scl_slope = 1.0;
            header.scl_inter = 0.0;
        }
        if header.vox_offset < 352.0 {
            header.vox_offset = 352.0;
        }
        Ok(header)
    }

    fn byte_width(&self) -> Result<usize, NiftiError> {
        match self.datatype {
            DT_UINT8 => Ok(1),
            DT_INT16 => Ok(2),
            DT_INT32 | DT_FLOAT32 => Ok(4),
            DT_FLOAT64 => Ok(8),
            other => Err(NiftiError::UnsupportedDatatype(other)),
        }
    }

    fn read_scalar(&self, bytes: &[u8]) -> Result<f64, NiftiError> {
        let value = match self.datatype {
            DT_UINT8 => bytes[0] as f64,
            DT_INT16 => i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f64,
            DT_INT32 => i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            DT_FLOAT32 => f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            DT_FLOAT64 => f64::from_le_bytes(bytes[..8].try_into().unwrap()),
            other => return Err(NiftiError::UnsupportedDatatype(other)),
        };
        Ok(value)
    }

    /// Select the spatial affine: sform wins over qform per the standard's
    /// precedence rule; both absent is rejected rather than guessing.
    fn affine(&self) -> Result<[[f64; 4]; 3], NiftiError> {
        if self.sform_code > 0 {
            return Ok(self.srow);
        }
        if self.qform_code > 0 {
            return Ok(self.qform_affine());
        }
        Err(NiftiError::NoSpatialTransform)
    }

    /// Quaternion method-2 affine: rotation from quatern (b,c,d) plus
    /// pixdim scaling and qfac sign, translated by qoffset.
    fn qform_affine(&self) -> [[f64; 4]; 3] {
        let [b, c, d] = self.quatern;
        let a = (1.0 - (b * b + c * c + d * d)).max(0.0).sqrt();
        let r = [
            [
                a * a + b * b - c * c - d * d,
                2.0 * b * c - 2.0 * a * d,
                2.0 * b * d + 2.0 * a * c,
            ],
            [
                2.0 * b * c + 2.0 * a * d,
                a * a + c * c - b * b - d * d,
                2.0 * c * d - 2.0 * a * b,
            ],
            [
                2.0 * b * d - 2.0 * a * c,
                2.0 * c * d + 2.0 * a * b,
                a * a + d * d - c * c - b * b,
            ],
        ];
        let qfac = if self.pixdim[0] < 0.0 { -1.0 } else { 1.0 };
        let mut affine = [[0.0; 4]; 3];
        for row in 0..3 {
            for col in 0..3 {
                let scale = if col == 2 { qfac } else { 1.0 };
                affine[row][col] = r[row][col] * self.pixdim[col + 1] * scale;
            }
            affine[row][3] = self.qoffset[row];
        }
        affine
    }
}

#[derive(Debug, Error)]
pub enum NiftiError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("input is too short to contain a NIfTI-1 header")]
    Truncated,
    #[error("not a NIfTI-1 single file: magic {0:?}")]
    UnsupportedMagic([u8; 4]),
    #[error("big-endian NIfTI files are not supported")]
    BigEndianUnsupported,
    #[error("invalid NIfTI-1 header")]
    BadHeader,
    #[error("unsupported dimensionality dim[0]={0}, dim[4]={1}; only 3D volumes")]
    UnsupportedDimensions(i16, i16),
    #[error("unsupported datatype code {0}; supported: u8, i16, i32, f32, f64")]
    UnsupportedDatatype(i16),
    #[error("non-millimeter spatial units (xyzt_units=0x{0:02x}) are refused")]
    NonMillimeterUnits(u8),
    #[error("file carries no spatial transform (qform_code=sform_code=0)")]
    NoSpatialTransform,
    #[error("header parse failed")]
    Header,
    #[error("invalid resample target: {0}")]
    InvalidTarget(String),
    #[error("component-dose import requires exactly 4 sources, got {0}")]
    ComponentCount(usize),
    #[error("component {0:?} was supplied more than once")]
    DuplicateComponent(DoseComponent),
    #[error("component {0:?} geometry disagrees with the shared grid")]
    GeometryDisagreement(DoseComponent),
    #[error("component {0:?} contains non-finite voxels")]
    NonFinite(DoseComponent),
    #[error("component {0:?} uncertainty image geometry disagrees")]
    SigmaGeometryDisagreement(DoseComponent),
    #[error("producer system must be declared; NIfTI carries none")]
    ProducerUndeclared,
}

/// One NIfTI file's role in a component-dose import.
///
/// `sigma_file`, when present, is a companion NIfTI carrying absolute
/// one-sigma standard uncertainty in the same unit and on the same grid
/// as `file` — the convention used by pipelines that emit paired
/// value/uncertainty volumes (for example OpenPINT's
/// `get_dose_component_sigmas` products).
#[derive(Debug, Clone)]
pub struct NiftiComponentSource {
    pub component: DoseComponent,
    pub file: std::path::PathBuf,
    pub sigma_file: Option<std::path::PathBuf>,
}

/// Build a `component-dose-interchange` document from four scalar NIfTI
/// volumes, one per required dose component.
///
/// NIfTI headers carry no producer identity, so `producer_system` is a
/// required caller declaration (for example `openpint`); `producer_version`
/// is recorded verbatim or as `undeclared`. What the files did state —
/// per-file datatype and which transform supplied the affine — is appended
/// to `normalization` as the honest provenance record.
pub fn interchange_from_niftis(
    sources: &[NiftiComponentSource],
    case_id: &str,
    unit: DoseUnit,
    normalization: &str,
    producer_system: &str,
    producer_version: Option<String>,
    frame_of_reference_uid: Option<String>,
) -> Result<ComponentDoseInterchange, NiftiError> {
    if sources.len() != 4 {
        return Err(NiftiError::ComponentCount(sources.len()));
    }
    if producer_system.trim().is_empty() {
        return Err(NiftiError::ProducerUndeclared);
    }
    let mut seen = Vec::with_capacity(4);
    for source in sources {
        if seen.contains(&source.component) {
            return Err(NiftiError::DuplicateComponent(source.component));
        }
        seen.push(source.component);
    }

    let mut geometry = None;
    let mut components = Vec::new();
    let mut file_map = Vec::new();
    for source in sources {
        let image = read_nifti_file(&source.file)?;
        if let Some(existing) = &geometry {
            if !grid_geometry_equivalent(existing, &image.geometry) {
                return Err(NiftiError::GeometryDisagreement(source.component));
            }
        } else {
            geometry = Some(image.geometry.clone());
        }
        if !image.values.iter().all(|v| v.is_finite()) {
            return Err(NiftiError::NonFinite(source.component));
        }
        let absolute_standard_uncertainty = match &source.sigma_file {
            Some(sigma_path) => {
                let sigma = read_nifti_file(sigma_path)?;
                if !grid_geometry_equivalent(&image.geometry, &sigma.geometry) {
                    return Err(NiftiError::SigmaGeometryDisagreement(source.component));
                }
                if !sigma.values.iter().all(|v| v.is_finite() && *v >= 0.0) {
                    return Err(NiftiError::NonFinite(source.component));
                }
                Some(sigma.values)
            }
            None => None,
        };
        components.push(DoseVolume {
            component: source.component,
            unit,
            values: image.values,
            absolute_standard_uncertainty,
        });
        file_map.push(format!(
            "{:?}={}(dt={},{}affine)",
            source.component,
            source
                .file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            image.datatype,
            image.transform_source,
        ));
    }

    let mut norm = normalization.trim().to_owned();
    norm.push_str(&format!("; nifti [{}]", file_map.join(",")));
    Ok(ComponentDoseInterchange {
        schema_version: openbnct_core::COMPONENT_DOSE_INTERCHANGE_SCHEMA.into(),
        case_id: case_id.to_owned(),
        frame_of_reference_uid,
        geometry: geometry.expect("four sources always produce a geometry"),
        producer: ExternalProducer {
            system: producer_system.trim().to_owned(),
            version: producer_version.unwrap_or_else(|| "undeclared".into()),
            normalization: norm,
        },
        components,
        component_profile: None,
        response_set: None,
        total: ExternalTotal::ComponentSum,
    })
}

/// Versioned export-manifest schema for component NIfTI sets.
pub const COMPONENT_NIFTI_MANIFEST_SCHEMA: &str = "openbnct.component-nifti-manifest/0.1.0";

/// One written component volume and, when emitted, its sigma companion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentNiftiFile {
    /// `component:boron`, … — the serialized `DoseComponent` token.
    pub component: String,
    /// File name within the output directory.
    pub file: String,
    /// SHA-256 of the written bytes.
    pub sha256: String,
    /// Sigma companion file name, when the component carried
    /// per-voxel uncertainties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_sha256: Option<String>,
}

/// Manifest written alongside a component NIfTI export so a downstream
/// consumer maps files to components by declaration, not filename
/// convention (`openbnct.component-nifti-manifest/0.1.0`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentNiftiManifest {
    #[serde(deserialize_with = "openbnct_core::deserialize_contract_id")]
    pub schema_version: String,
    pub case_id: String,
    /// Serialized `DoseUnit` token of every component volume.
    pub unit: String,
    /// Grid provenance: shape, spacing, and origin restated for consumers
    /// that skip the NIfTI headers.
    pub geometry: GridGeometry,
    pub files: Vec<ComponentNiftiFile>,
    /// Provenance id of the exported bundle.
    pub provenance_id: String,
}

/// Export every component of a physical dose bundle as float64 NIfTI
/// volumes — the per-component plus sigma-companion convention
/// `interchange_from_niftis` consumes, so the pair round-trips.
///
/// Files are named `<case_id>.<component>.nii` (`.nii.gz` when
/// `gzip` is set) with `…<component>.sigma.nii` companions for
/// components carrying per-voxel uncertainties. The manifest records
/// each written file's SHA-256.
pub fn export_component_niftis(
    bundle: &openbnct_core::PhysicalDoseBundle,
    output_dir: &Path,
    gzip: bool,
) -> io::Result<ComponentNiftiManifest> {
    std::fs::create_dir_all(output_dir)?;
    let ext = if gzip { ".nii.gz" } else { ".nii" };
    // Hash the exact bytes on disk (post-gzip when compressed).
    let write_hashed = |image: &NiftiImage, path: &Path| -> io::Result<String> {
        let encoded = encode_nifti(image);
        let stored = if gzip {
            use std::io::Write;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            encoder.write_all(&encoded)?;
            encoder.finish()?
        } else {
            encoded
        };
        std::fs::write(path, &stored)?;
        let mut h = sha2::Sha256::new();
        h.update(&stored);
        Ok(format!("{:x}", h.finalize()))
    };
    let mut files = Vec::with_capacity(bundle.components.len());
    for component in &bundle.components {
        let name = serde_json::to_value(component.component)
            .and_then(serde_json::from_value::<String>)
            .unwrap_or_else(|_| "unknown".into());
        let file = format!("{}.{}{}", bundle.case_id, name, ext);
        let image = NiftiImage {
            geometry: bundle.geometry.clone(),
            values: component.values.clone(),
            datatype: DT_FLOAT64,
            transform_source: "sform",
            description: format!("openbnct {} component:{name}", bundle.case_id),
            intent_name: String::new(),
            units_declared_mm: true,
        };
        let mut entry = ComponentNiftiFile {
            component: format!("component:{name}"),
            sha256: write_hashed(&image, &output_dir.join(&file))?,
            file,
            sigma_file: None,
            sigma_sha256: None,
        };
        if let Some(sigma) = &component.absolute_standard_uncertainty {
            let sigma_file = format!("{}.{name}.sigma{}", bundle.case_id, ext);
            let sigma_image = NiftiImage {
                description: format!("openbnct {} component:{name} sigma", bundle.case_id),
                values: sigma.clone(),
                ..image
            };
            entry.sigma_sha256 = Some(write_hashed(&sigma_image, &output_dir.join(&sigma_file))?);
            entry.sigma_file = Some(sigma_file);
        }
        files.push(entry);
    }
    let unit = serde_json::to_value(
        bundle
            .components
            .first()
            .map(|c| c.unit)
            .unwrap_or(DoseUnit::Gray),
    )
    .and_then(serde_json::from_value::<String>)
    .unwrap_or_else(|_| "gray".into());
    Ok(ComponentNiftiManifest {
        schema_version: COMPONENT_NIFTI_MANIFEST_SCHEMA.into(),
        case_id: bundle.case_id.clone(),
        unit,
        geometry: bundle.geometry.clone(),
        files,
        provenance_id: bundle.provenance_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_direction() -> [f64; 9] {
        [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]
    }

    fn grid(shape: [u32; 3], spacing: [f64; 3], origin: [f64; 3]) -> GridGeometry {
        GridGeometry {
            shape,
            spacing_mm: spacing,
            origin_mm: origin,
            direction: identity_direction(),
        }
    }

    fn image(geometry: GridGeometry, values: Vec<f64>) -> NiftiImage {
        NiftiImage {
            geometry,
            values,
            datatype: DT_FLOAT64,
            transform_source: "sform",
            description: "test".into(),
            intent_name: String::new(),
            units_declared_mm: true,
        }
    }

    #[test]
    fn interchange_from_niftis_builds_document() {
        let dir = tempfile::tempdir().unwrap();
        let geometry = grid([2, 2, 1], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
        let components = [
            DoseComponent::Boron,
            DoseComponent::Nitrogen,
            DoseComponent::Hydrogen,
            DoseComponent::Photon,
        ];
        let mut sources = Vec::new();
        for (i, component) in components.iter().enumerate() {
            let file = dir.path().join(format!("dose_{i}.nii"));
            write_nifti(&image(geometry.clone(), vec![i as f64 + 1.0; 4]), &file).unwrap();
            let sigma_file = dir.path().join(format!("sigma_{i}.nii"));
            write_nifti(&image(geometry.clone(), vec![0.1; 4]), &sigma_file).unwrap();
            sources.push(NiftiComponentSource {
                component: *component,
                file,
                sigma_file: Some(sigma_file),
            });
        }
        let document = interchange_from_niftis(
            &sources,
            "case-1",
            DoseUnit::GrayPerSourceParticle,
            "per source neutron",
            "openpint",
            Some("test-sha".into()),
            None,
        )
        .unwrap();
        assert_eq!(document.case_id, "case-1");
        assert_eq!(document.producer.system, "openpint");
        assert_eq!(document.producer.version, "test-sha");
        assert_eq!(document.components.len(), 4);
        assert_eq!(
            document.components[0].absolute_standard_uncertainty,
            Some(vec![0.1; 4])
        );
        assert!(document.producer.normalization.contains("nifti ["));
        document.validate().unwrap();
    }

    #[test]
    fn interchange_from_niftis_rejects_mismatched_grids_and_undeclared_producer() {
        let dir = tempfile::tempdir().unwrap();
        let shared = grid([2, 2, 1], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
        let other = grid([4, 2, 1], [1.0, 1.0, 1.0], [0.0, 0.0, 0.0]);
        let make = |name: &str, g: &GridGeometry| {
            let file = dir.path().join(name);
            write_nifti(&image(g.clone(), vec![1.0; 8]), &file).unwrap();
            file
        };
        let sources = vec![
            NiftiComponentSource {
                component: DoseComponent::Boron,
                file: make("b.nii", &shared),
                sigma_file: None,
            },
            NiftiComponentSource {
                component: DoseComponent::Nitrogen,
                file: make("n.nii", &other),
                sigma_file: None,
            },
            NiftiComponentSource {
                component: DoseComponent::Hydrogen,
                file: make("h.nii", &shared),
                sigma_file: None,
            },
            NiftiComponentSource {
                component: DoseComponent::Photon,
                file: make("g.nii", &shared),
                sigma_file: None,
            },
        ];
        assert!(matches!(
            interchange_from_niftis(&sources, "c", DoseUnit::Gray, "n", "openpint", None, None),
            Err(NiftiError::GeometryDisagreement(DoseComponent::Nitrogen))
        ));
        assert!(matches!(
            interchange_from_niftis(&sources, "c", DoseUnit::Gray, "n", " ", None, None),
            Err(NiftiError::ProducerUndeclared)
        ));
    }

    #[test]
    fn round_trips_geometry_and_values() {
        let geometry = grid([4, 3, 2], [2.0, 3.0, 4.0], [-3.0, -4.5, -4.0]);
        let values: Vec<f64> = (0..24).map(|i| i as f64 * 0.5).collect();
        let encoded = encode_nifti(&image(geometry.clone(), values.clone()));
        let decoded = read_nifti(&encoded).unwrap();
        assert_eq!(decoded.geometry, geometry);
        assert_eq!(decoded.values, values);
        assert_eq!(decoded.transform_source, "sform");
        assert_eq!(decoded.datatype, DT_FLOAT64);
    }

    #[test]
    fn ras_world_frame_maps_to_lps() {
        // An image whose sform is diag(2,3,4) shifted by (10,20,30):
        // voxel 0 center sits at RAS (10,20,30) = LPS (-10,-20,30).
        let geometry = grid([2, 2, 2], [2.0, 3.0, 4.0], [-10.0, -20.0, 30.0]);
        let encoded = encode_nifti(&image(geometry.clone(), vec![0.0; 8]));
        let decoded = read_nifti(&encoded).unwrap();
        assert_eq!(decoded.geometry, geometry);
        // Confirm the stored sform is genuinely RAS: an identity LPS
        // direction writes a negative RAS x column.
        let srow_x = f32::from_le_bytes(encoded[280..284].try_into().unwrap());
        assert_eq!(srow_x, -2.0);
    }

    #[test]
    fn resample_nearest_preserves_mask_regions() {
        // Source: 4x4x4, 1mm voxels, a 2x2x2 block at voxel (1..3)^3 set.
        let source_geometry = grid([4, 4, 4], [1.0; 3], [0.5, 0.5, 0.5]);
        let mut values = vec![0.0; 64];
        for k in 1..=2 {
            for j in 1..=2 {
                for i in 1..=2 {
                    values[i + 4 * j + 16 * k] = 1.0;
                }
            }
        }
        let source = image(source_geometry, values);
        // Target: same volume sampled at 0.5mm — nearest must keep a block.
        let target = grid([8, 8, 8], [0.5; 3], [0.25, 0.25, 0.25]);
        let resampled = resample_to_grid(&source, &target, Interpolation::Nearest);
        let count = resampled.iter().filter(|v| **v != 0.0).count();
        // Each source voxel maps to a 2x2x2 target block: 8*8=64 voxels.
        assert_eq!(count, 64);
        let mask = to_mask(&image(target, resampled), "block");
        assert_eq!(mask.voxels.iter().filter(|v| **v).count(), 64);
    }

    #[test]
    fn resample_trilinear_interpolates_linearly() {
        // Ramp along x: value = voxel index i.
        let source_geometry = grid([4, 1, 1], [1.0; 3], [0.5, 0.5, 0.5]);
        let source = image(source_geometry, vec![0.0, 1.0, 2.0, 3.0]);
        // Target centers at 0.25, 0.75, 1.25, ... land on source coords
        // -0.25, 0.25, 0.75, ... — clamped at the edges.
        let target = grid([8, 1, 1], [0.5, 1.0, 1.0], [0.25, 0.5, 0.5]);
        let resampled = resample_to_grid(&source, &target, Interpolation::Trilinear);
        assert_eq!(
            resampled,
            vec![0.0, 0.25, 0.75, 1.25, 1.75, 2.25, 2.75, 3.0]
        );
    }

    #[test]
    fn rejects_oblique_transform_by_preserving_direction() {
        // A 30° rotation about z must round-trip through the direction
        // matrix — oblique cases are preserved, not snapped to axial.
        let (sin, cos) = (0.5_f64, (3.0_f64.sqrt()) / 2.0);
        let mut geometry = grid([4, 4, 4], [1.0; 3], [0.0, 0.0, 0.0]);
        geometry.direction = [cos, -sin, 0.0, sin, cos, 0.0, 0.0, 0.0, 1.0];
        let encoded = encode_nifti(&image(geometry.clone(), vec![0.0; 64]));
        let decoded = read_nifti(&encoded).unwrap();
        for axis in 0..9 {
            assert!((decoded.geometry.direction[axis] - geometry.direction[axis]).abs() < 1.0e-6);
        }
    }

    #[test]
    fn oblique_landmarks_match_reference_affine() {
        // Independently verified against nibabel: sform rows
        // [[2c,-3s,0,5],[2s,3c,0,10],[0,0,4,-20]] (30° about z, spacing
        // 2x3x4 mm) place voxel (1,1,1) at RAS (5.232,13.598,-16) = LPS
        // (-5.232,-13.598,-16).
        let (s, c) = (0.5_f64, 3.0_f64.sqrt() / 2.0);
        let mut geometry = grid([4, 3, 2], [2.0, 3.0, 4.0], [-5.0, -10.0, -20.0]);
        geometry.direction = [-c, s, 0.0, -s, -c, 0.0, 0.0, 0.0, 1.0];
        let world = voxel_center(&geometry, 1, 1, 1);
        let expected = [-5.232_050_807_568_878, -13.598076211353316, -16.0];
        for (w, e) in world.iter().zip(expected.iter()) {
            assert!((w - e).abs() < 1.0e-9);
        }
        // Round-trip: the world point must map back to voxel (1,1,1).
        let voxel = world_to_voxel(&geometry, world).unwrap();
        for v in &voxel {
            assert!((v - 1.0).abs() < 1.0e-9);
        }
    }

    #[test]
    fn rejects_unsupported_inputs_cleanly() {
        assert!(matches!(read_nifti(&[0; 100]), Err(NiftiError::Truncated)));
        let mut bytes = vec![0_u8; 400];
        bytes[0..4].copy_from_slice(&(348_i32).to_le_bytes());
        assert!(matches!(
            read_nifti(&bytes),
            Err(NiftiError::UnsupportedMagic(_))
        ));
    }

    #[test]
    fn gzip_round_trips() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let geometry = grid([2, 2, 2], [1.0; 3], [0.0; 3]);
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let plain = encode_nifti(&image(geometry, values.clone()));
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&plain).unwrap();
        let compressed = encoder.finish().unwrap();
        let decoded = read_nifti(&compressed).unwrap();
        assert_eq!(decoded.values, values);
    }

    #[test]
    fn target_geometry_accepts_cases_and_bundles_and_rejects_others() {
        let scratch = tempfile::tempdir().unwrap();
        let geometry_json = serde_json::json!({
            "shape": [4, 3, 2],
            "spacing_mm": [2.0, 3.0, 4.0],
            "origin_mm": [-3.0, -4.5, -4.0],
            "direction": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        });
        let case = scratch.path().join("case.json");
        std::fs::write(
            &case,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": "openbnct.transport-case/0.1.0",
                "geometry": geometry_json,
            }))
            .unwrap(),
        )
        .unwrap();
        let resolved = read_target_geometry(&case).unwrap();
        assert_eq!(resolved.shape, [4, 3, 2]);
        assert_eq!(resolved.spacing_mm, [2.0, 3.0, 4.0]);

        let bundle = scratch.path().join("bundle.json");
        std::fs::write(
            &bundle,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": "openbnct.physical-dose-bundle/0.2.0",
                "geometry": geometry_json,
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read_target_geometry(&bundle).unwrap(), resolved);

        let bad = scratch.path().join("bad.json");
        std::fs::write(&bad, b"{\"schema_version\": \"other/9.9\"}").unwrap();
        assert!(matches!(
            read_target_geometry(&bad),
            Err(NiftiError::InvalidTarget(_))
        ));
    }

    /// OP-01 grid-resolution convergence evidence: resampling a smooth
    /// analytic field onto a fixed target grid must show the declared
    /// second-order (quadratic) trilinear convergence — each spacing
    /// halving cuts the interior L-infinity error below the predeclared
    /// tolerance ratio of 0.35 (theory predicts ~0.25 for a C² field).
    #[test]
    fn trilinear_resample_converges_quadratically_on_a_smooth_field() {
        let analytic =
            |w: [f64; 3]| (-(w[0] * w[0] + w[1] * w[1] + w[2] * w[2]) / 200.0).exp() + 0.05 * w[0];
        // Fixed fine target: 21^3 voxels, 0.94 mm, covering [-9.63, 9.17]
        // mm — deliberately incommensurate with every source spacing so
        // target centers never coincide with source voxel centers.
        let target = grid([21, 21, 21], [0.94; 3], [-9.63; 3]);
        // Source grids halve spacing: 2.0 mm, 1.0 mm, 0.5 mm over the same
        // domain (voxel centers span [-10, 10] in each case).
        let mut errors = Vec::new();
        for n in [11u32, 21, 41] {
            let spacing = 20.0 / (n - 1) as f64;
            let source_geometry = grid([n, n, n], [spacing; 3], [-10.0; 3]);
            let values = (0..n * n * n)
                .map(|flat| {
                    let flat = flat as usize;
                    let n = n as usize;
                    let (i, j, k) = (flat % n, (flat / n) % n, flat / (n * n));
                    analytic(voxel_center(&source_geometry, i, j, k))
                })
                .collect();
            let source = image(source_geometry, values);
            let resampled = resample_to_grid(&source, &target, Interpolation::Trilinear);
            // Interior target voxels only — away from the domain edge where
            // corner clamping, not interpolation order, dominates.
            let mut max_err = 0.0_f64;
            for k in 4..17 {
                for j in 4..17 {
                    for i in 4..17 {
                        let index = i + 21 * j + 21 * 21 * k;
                        let err =
                            (resampled[index] - analytic(voxel_center(&target, i, j, k))).abs();
                        max_err = max_err.max(err);
                    }
                }
            }
            errors.push(max_err);
        }
        assert!(
            errors[0] > errors[1] && errors[1] > errors[2],
            "error must decrease with refinement: {errors:?}"
        );
        for pair in errors.windows(2) {
            let ratio = pair[1] / pair[0];
            assert!(
                ratio < 0.35,
                "trilinear error ratio {ratio} exceeds the declared 0.35 \
                 second-order bound (errors: {errors:?})"
            );
        }
    }

    #[test]
    fn component_export_round_trips_through_import() {
        use openbnct_core::{
            ContentReference, PHYSICAL_DOSE_BUNDLE_SCHEMA, PhysicalDoseBundle,
            PhysicalTotalDoseVolume, TotalUncertaintyMethod,
        };
        let geometry = grid([2, 2, 2], [2.0, 2.0, 2.0], [-1.0, -1.0, -1.0]);
        let n = 8usize;
        let component = |c: DoseComponent, base: f64| DoseVolume {
            component: c,
            unit: DoseUnit::GrayPerSourceParticle,
            values: (0..n).map(|i| base + i as f64).collect(),
            absolute_standard_uncertainty: Some(vec![0.05; n]),
        };
        let bundle = PhysicalDoseBundle {
            schema_version: PHYSICAL_DOSE_BUNDLE_SCHEMA.into(),
            case_id: "rt-case".into(),
            frame_of_reference_uid: None,
            geometry: geometry.clone(),
            component_profile: ContentReference {
                id: "profile".into(),
                sha256: "0".repeat(64),
            },
            response_set: ContentReference {
                id: "responses".into(),
                sha256: "0".repeat(64),
            },
            provenance_id: "rt-test".into(),
            components: vec![
                component(DoseComponent::Boron, 10.0),
                component(DoseComponent::Nitrogen, 20.0),
                component(DoseComponent::Hydrogen, 30.0),
                component(DoseComponent::Photon, 40.0),
            ],
            physical_total: PhysicalTotalDoseVolume {
                unit: DoseUnit::GrayPerSourceParticle,
                values: vec![100.0; n],
                absolute_standard_uncertainty: Some(vec![0.1; n]),
                uncertainty_method: TotalUncertaintyMethod::DedicatedEstimator,
            },
        };
        let dir = tempfile::tempdir().unwrap();
        let manifest = export_component_niftis(&bundle, dir.path(), false).unwrap();
        assert_eq!(manifest.files.len(), 4);
        for entry in &manifest.files {
            assert!(entry.sigma_file.is_some());
            // Manifest hashes match the bytes on disk.
            let bytes = std::fs::read(dir.path().join(&entry.file)).unwrap();
            let mut h = sha2::Sha256::new();
            h.update(&bytes);
            assert_eq!(entry.sha256, format!("{:x}", h.finalize()));
        }

        // Re-import through the component-interchange path.
        let sources: Vec<NiftiComponentSource> = [
            DoseComponent::Boron,
            DoseComponent::Nitrogen,
            DoseComponent::Hydrogen,
            DoseComponent::Photon,
        ]
        .iter()
        .map(|c| {
            let name = serde_json::to_value(*c)
                .and_then(serde_json::from_value::<String>)
                .unwrap();
            NiftiComponentSource {
                component: *c,
                file: dir.path().join(format!("rt-case.{name}.nii")),
                sigma_file: Some(dir.path().join(format!("rt-case.{name}.sigma.nii"))),
            }
        })
        .collect();
        let doc = interchange_from_niftis(
            &sources,
            "rt-case",
            DoseUnit::GrayPerSourceParticle,
            "round trip",
            "openbnct",
            None,
            None,
        )
        .unwrap();
        for (written, imported) in bundle.components.iter().zip(doc.components.iter()) {
            assert_eq!(written.component, imported.component);
            assert_eq!(written.values, imported.values);
            assert_eq!(
                written.absolute_standard_uncertainty,
                imported.absolute_standard_uncertainty
            );
        }
        assert!(grid_geometry_equivalent(&bundle.geometry, &doc.geometry));
    }
}
