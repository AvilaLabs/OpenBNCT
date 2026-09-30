// SPDX-License-Identifier: MIT

//! NRRD and MetaImage readers plus a format-sniffing `read_volume`.
//!
//! Both formats carry a physical-space transform. The platform's grid paths
//! (analytic oracles, OpenMC/MCNP export, transport) require axis-aligned
//! grids, so these readers *reorient*: any volume whose voxel axes are a
//! signed permutation of the patient axes is resampled by index (a pure
//! flip/transpose — no interpolation) onto a canonical grid with identity
//! direction and positive spacing in patient LPS millimeters. A volume with
//! genuinely oblique axes is refused with an explicit error rather than
//! silently interpolated. (The NIfTI reader preserves whatever direction the
//! file declares and does not reorient.)
//!
//! Coordinate conventions: NRRD declares its `space`; `left-posterior-superior`,
//! `right-anterior-superior` and `left-anterior-superior` are converted to
//! LPS. MetaImage coordinates are taken as LPS (the ITK convention, where
//! `Offset` equals a DICOM Image Position Patient). MetaImage
//! `TransformMatrix` is read as three consecutive voxel-axis direction
//! vectors, as ITK does.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use openbnct_core::GridGeometry;

use crate::{NiftiError, NiftiImage};

/// Voxels above this count are refused before any allocation.
const MAX_VOXELS: usize = 1 << 31;

fn fmt_err(message: impl Into<String>) -> NiftiError {
    NiftiError::Format(message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn width(self) -> usize {
        match self {
            Scalar::I8 | Scalar::U8 => 1,
            Scalar::I16 | Scalar::U16 => 2,
            Scalar::I32 | Scalar::U32 | Scalar::F32 => 4,
            Scalar::F64 => 8,
        }
    }

    /// NIfTI-1 datatype code with the same meaning, recorded for provenance.
    fn nifti_code(self) -> i16 {
        match self {
            Scalar::U8 => 2,
            Scalar::I16 => 4,
            Scalar::I32 => 8,
            Scalar::F32 => 16,
            Scalar::F64 => 64,
            Scalar::I8 => 256,
            Scalar::U16 => 512,
            Scalar::U32 => 768,
        }
    }

    fn decode(self, bytes: &[u8], big: bool, count: usize) -> Result<Vec<f64>, NiftiError> {
        let width = self.width();
        let needed = count
            .checked_mul(width)
            .ok_or_else(|| fmt_err("voxel data size overflows"))?;
        if bytes.len() < needed {
            return Err(NiftiError::Truncated);
        }
        macro_rules! decode_as {
            ($t:ty) => {
                bytes[..needed]
                    .chunks_exact(width)
                    .map(|c| {
                        let a: [u8; std::mem::size_of::<$t>()] = c.try_into().unwrap();
                        (if big {
                            <$t>::from_be_bytes(a)
                        } else {
                            <$t>::from_le_bytes(a)
                        }) as f64
                    })
                    .collect()
            };
        }
        Ok(match self {
            Scalar::I8 => bytes[..needed].iter().map(|&b| b as i8 as f64).collect(),
            Scalar::U8 => bytes[..needed].iter().map(|&b| b as f64).collect(),
            Scalar::I16 => decode_as!(i16),
            Scalar::U16 => decode_as!(u16),
            Scalar::I32 => decode_as!(i32),
            Scalar::U32 => decode_as!(u32),
            Scalar::F32 => decode_as!(f32),
            Scalar::F64 => decode_as!(f64),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Codec {
    Raw,
    /// gzip container (NRRD).
    Gzip,
    /// zlib container (MetaImage `CompressedData`).
    Zlib,
    Ascii,
}

/// Fully-parsed description shared by both formats before reorientation.
struct Parsed {
    shape: [usize; 3],
    scalar: Scalar,
    big_endian: bool,
    codec: Codec,
    /// World step vector of each voxel axis (direction * spacing), in LPS.
    axes: [[f64; 3]; 3],
    origin: [f64; 3],
    units_declared_mm: bool,
    description: &'static str,
    transform_source: &'static str,
}

fn checked_voxels(shape: [usize; 3]) -> Result<usize, NiftiError> {
    if shape.contains(&0) {
        return Err(fmt_err("volume has a zero-length axis"));
    }
    let n = shape[0]
        .checked_mul(shape[1])
        .and_then(|v| v.checked_mul(shape[2]))
        .ok_or_else(|| fmt_err("volume size overflows"))?;
    if n > MAX_VOXELS {
        return Err(fmt_err(format!("volume of {n} voxels exceeds the size limit")));
    }
    Ok(n)
}

/// Extract exactly `count` values from `payload` (after container decode).
fn decode_payload(
    payload: &[u8],
    parsed: &Parsed,
    byte_skip: i64,
    count: usize,
) -> Result<Vec<f64>, NiftiError> {
    let width = parsed.scalar.width();
    let needed = count
        .checked_mul(width)
        .ok_or_else(|| fmt_err("voxel data size overflows"))?;
    let decoded: Vec<u8>;
    let bytes: &[u8] = match parsed.codec {
        Codec::Raw => payload,
        Codec::Gzip => {
            let mut out = Vec::new();
            let limit = (needed as u64).saturating_add(byte_skip.max(0) as u64);
            flate2::read::GzDecoder::new(payload)
                .take(limit)
                .read_to_end(&mut out)?;
            decoded = out;
            &decoded
        }
        Codec::Zlib => {
            let mut out = Vec::new();
            let limit = (needed as u64).saturating_add(byte_skip.max(0) as u64);
            flate2::read::ZlibDecoder::new(payload)
                .take(limit)
                .read_to_end(&mut out)?;
            decoded = out;
            &decoded
        }
        Codec::Ascii => {
            let text = std::str::from_utf8(payload)
                .map_err(|_| fmt_err("ascii voxel data is not valid text"))?;
            let mut values = Vec::with_capacity(count.min(1 << 20));
            for token in text.split_whitespace() {
                if values.len() == count {
                    break;
                }
                values.push(
                    token
                        .parse::<f64>()
                        .map_err(|_| fmt_err(format!("bad ascii voxel value {token:?}")))?,
                );
            }
            if values.len() < count {
                return Err(NiftiError::Truncated);
            }
            return Ok(values);
        }
    };
    let slice = if byte_skip < 0 {
        if bytes.len() < needed {
            return Err(NiftiError::Truncated);
        }
        &bytes[bytes.len() - needed..]
    } else {
        let skip = byte_skip as usize;
        if bytes.len() < skip {
            return Err(NiftiError::Truncated);
        }
        &bytes[skip..]
    };
    parsed.scalar.decode(slice, parsed.big_endian, count)
}

/// Reorient to identity direction / positive spacing, refusing oblique axes.
fn finalize(parsed: &Parsed, values: Vec<f64>) -> Result<NiftiImage, NiftiError> {
    let shape = parsed.shape;
    // For each voxel axis: which world axis it follows and with what sign.
    let mut world_axis = [0_usize; 3];
    let mut sign = [1.0_f64; 3];
    let mut spacing_of_axis = [0.0_f64; 3];
    for a in 0..3 {
        let v = parsed.axes[a];
        if v.iter().any(|c| !c.is_finite()) {
            return Err(fmt_err("non-finite space direction"));
        }
        let norm = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if norm <= 0.0 {
            return Err(fmt_err("zero-length space direction"));
        }
        let (dominant, &largest) = v
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.abs().total_cmp(&y.1.abs()))
            .unwrap();
        let off_axis = v
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != dominant)
            .map(|(_, c)| c.abs())
            .fold(0.0_f64, f64::max);
        if off_axis > 1.0e-4 * norm {
            return Err(fmt_err(format!(
                "oblique space directions are not supported: voxel axis {a} is not \
                 aligned with a patient axis (off-axis component {:.3}% of its length); \
                 resample the volume to an axis-aligned grid first",
                100.0 * off_axis / norm
            )));
        }
        world_axis[a] = dominant;
        sign[a] = if largest < 0.0 { -1.0 } else { 1.0 };
        spacing_of_axis[a] = norm;
    }
    let mut seen = [false; 3];
    for &w in &world_axis {
        if seen[w] {
            return Err(fmt_err(
                "space directions are degenerate: two voxel axes follow the same patient axis",
            ));
        }
        seen[w] = true;
    }
    // Canonical axis n follows world axis n; find the source voxel axis.
    let mut source_axis = [0_usize; 3];
    for a in 0..3 {
        source_axis[world_axis[a]] = a;
    }
    let mut new_shape = [0_usize; 3];
    let mut new_spacing = [0.0_f64; 3];
    let mut new_origin = [0.0_f64; 3];
    for n in 0..3 {
        let a = source_axis[n];
        new_shape[n] = shape[a];
        new_spacing[n] = spacing_of_axis[a];
        new_origin[n] = parsed.origin[n];
        if sign[a] < 0.0 {
            new_origin[n] -= (shape[a] - 1) as f64 * spacing_of_axis[a];
        }
    }
    let identity_orientation = (0..3).all(|a| source_axis[a] == a && sign[a] > 0.0);
    let reoriented = if identity_orientation {
        values
    } else {
        let old_stride = [1, shape[0], shape[0] * shape[1]];
        let mut out = vec![0.0_f64; values.len()];
        let mut index = 0;
        for k in 0..new_shape[2] {
            for j in 0..new_shape[1] {
                for i in 0..new_shape[0] {
                    let new_idx = [i, j, k];
                    let mut src = 0;
                    for n in 0..3 {
                        let a = source_axis[n];
                        let idx = if sign[a] > 0.0 {
                            new_idx[n]
                        } else {
                            shape[a] - 1 - new_idx[n]
                        };
                        src += idx * old_stride[a];
                    }
                    out[index] = values[src];
                    index += 1;
                }
            }
        }
        out
    };
    Ok(NiftiImage {
        geometry: GridGeometry {
            shape: [
                new_shape[0] as u32,
                new_shape[1] as u32,
                new_shape[2] as u32,
            ],
            spacing_mm: new_spacing,
            origin_mm: new_origin,
            direction: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        },
        values: reoriented,
        datatype: parsed.scalar.nifti_code(),
        transform_source: parsed.transform_source,
        description: parsed.description.to_owned(),
        intent_name: String::new(),
        units_declared_mm: parsed.units_declared_mm,
    })
}

fn safe_sibling(header_path: &Path, name: &str) -> Result<PathBuf, NiftiError> {
    let relative = Path::new(name);
    if name.is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(fmt_err(format!(
            "detached data file {name:?} must be a relative path inside the header's directory"
        )));
    }
    Ok(header_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(relative))
}

fn skip_lines(mut bytes: &[u8], lines: usize) -> Result<&[u8], NiftiError> {
    for _ in 0..lines {
        match bytes.iter().position(|&b| b == b'\n') {
            Some(p) => bytes = &bytes[p + 1..],
            None => return Err(NiftiError::Truncated),
        }
    }
    Ok(bytes)
}

fn parse_floats(text: &str, expected: usize, what: &str) -> Result<Vec<f64>, NiftiError> {
    let values: Result<Vec<f64>, _> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .map(str::parse::<f64>)
        .collect();
    let values = values.map_err(|_| fmt_err(format!("bad {what}: {text:?}")))?;
    if values.len() != expected {
        return Err(fmt_err(format!(
            "{what} needs {expected} numbers, got {}",
            values.len()
        )));
    }
    Ok(values)
}

// ---------------------------------------------------------------------------
// NRRD
// ---------------------------------------------------------------------------

fn nrrd_scalar(name: &str) -> Result<Scalar, NiftiError> {
    Ok(match name.trim().to_ascii_lowercase().as_str() {
        "signed char" | "int8" | "int8_t" => Scalar::I8,
        "uchar" | "unsigned char" | "uint8" | "uint8_t" => Scalar::U8,
        "short" | "short int" | "signed short" | "signed short int" | "int16" | "int16_t" => {
            Scalar::I16
        }
        "ushort" | "unsigned short" | "unsigned short int" | "uint16" | "uint16_t" => Scalar::U16,
        "int" | "signed int" | "int32" | "int32_t" => Scalar::I32,
        "uint" | "unsigned int" | "uint32" | "uint32_t" => Scalar::U32,
        "float" => Scalar::F32,
        "double" => Scalar::F64,
        other => {
            return Err(fmt_err(format!(
                "unsupported NRRD type {other:?}; supported: int8/16/32, uint8/16/32, float, double"
            )));
        }
    })
}

/// Parse a list of `(x,y,z)` vectors and `none` entries.
fn nrrd_vectors(text: &str) -> Result<Vec<Option<[f64; 3]>>, NiftiError> {
    let mut rest = text.trim();
    let mut out = Vec::new();
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("none") {
            out.push(None);
            rest = r.trim_start();
        } else if let Some(r) = rest.strip_prefix('(') {
            let end = r
                .find(')')
                .ok_or_else(|| fmt_err(format!("unterminated vector in {text:?}")))?;
            let v = parse_floats(&r[..end], 3, "NRRD vector")?;
            out.push(Some([v[0], v[1], v[2]]));
            rest = r[end + 1..].trim_start();
        } else {
            return Err(fmt_err(format!("bad NRRD vector list {text:?}")));
        }
    }
    Ok(out)
}

/// Read a `.nrrd` (attached) or `.nhdr` (detached) file.
pub fn read_nrrd_file(path: &Path) -> Result<NiftiImage, NiftiError> {
    let bytes = std::fs::read(path)?;
    read_nrrd_impl(&bytes, Some(path))
}

/// Parse an attached-header NRRD held in memory. Detached headers need a
/// path to find their data file; use [`read_nrrd_file`] for those.
pub fn read_nrrd(bytes: &[u8]) -> Result<NiftiImage, NiftiError> {
    read_nrrd_impl(bytes, None)
}

fn read_nrrd_impl(bytes: &[u8], path: Option<&Path>) -> Result<NiftiImage, NiftiError> {
    if !bytes.starts_with(b"NRRD000") {
        return Err(fmt_err("not a NRRD file (missing NRRD000x magic)"));
    }
    // Split header lines; the header ends at the first empty line or EOF.
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut pos = 0;
    let mut data_start = bytes.len();
    let mut first = true;
    while pos < bytes.len() {
        let end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |p| pos + p);
        let line = String::from_utf8_lossy(&bytes[pos..end]);
        let line = line.trim_end_matches('\r');
        let next = (end + 1).min(bytes.len());
        if first {
            first = false;
        } else if line.is_empty() {
            data_start = next;
            break;
        } else if !line.starts_with('#') && !line.contains(":=") {
            let colon = line
                .find(':')
                .ok_or_else(|| fmt_err(format!("bad NRRD header line {line:?}")))?;
            let key = line[..colon].trim().to_ascii_lowercase();
            if fields.iter().any(|(k, _)| *k == key) {
                return Err(fmt_err(format!("duplicate NRRD field {key:?}")));
            }
            fields.push((key, line[colon + 1..].trim().to_owned()));
        }
        pos = next;
    }
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    let dimension: usize = get("dimension")
        .ok_or_else(|| fmt_err("NRRD header has no dimension"))?
        .parse()
        .map_err(|_| fmt_err("bad NRRD dimension"))?;
    if dimension != 3 {
        return Err(NiftiError::UnsupportedDimensions(dimension as i16, 1));
    }
    let sizes = parse_floats(get("sizes").ok_or_else(|| fmt_err("NRRD has no sizes"))?, 3, "sizes")?;
    if sizes.iter().any(|s| *s < 1.0 || s.fract() != 0.0) {
        return Err(fmt_err("bad NRRD sizes"));
    }
    let shape = [sizes[0] as usize, sizes[1] as usize, sizes[2] as usize];
    let count = checked_voxels(shape)?;
    let scalar = nrrd_scalar(get("type").ok_or_else(|| fmt_err("NRRD has no type"))?)?;
    if let Some(kinds) = get("kinds") {
        if kinds
            .split_whitespace()
            .any(|k| !matches!(k, "domain" | "space" | "none"))
        {
            return Err(fmt_err(format!(
                "NRRD kinds {kinds:?} are not a scalar 3-D volume"
            )));
        }
    }
    let encoding = get("encoding")
        .ok_or_else(|| fmt_err("NRRD has no encoding"))?
        .to_ascii_lowercase();
    let codec = match encoding.as_str() {
        "raw" => Codec::Raw,
        "gzip" | "gz" => Codec::Gzip,
        "ascii" | "txt" | "text" => Codec::Ascii,
        other => {
            return Err(fmt_err(format!(
                "unsupported NRRD encoding {other:?}; supported: raw, gzip, ascii"
            )));
        }
    };
    let big_endian = match get("endian").map(str::to_ascii_lowercase).as_deref() {
        Some("big") => true,
        Some("little") => false,
        Some(other) => return Err(fmt_err(format!("bad NRRD endian {other:?}"))),
        None if scalar.width() > 1 && codec != Codec::Ascii => {
            return Err(fmt_err("NRRD with multi-byte samples must declare endian"));
        }
        None => false,
    };

    // Space and geometry.
    let space = get("space").map(str::to_ascii_lowercase);
    let (flip_x, flip_y) = match space.as_deref() {
        Some("left-posterior-superior" | "lps") => (false, false),
        Some("right-anterior-superior" | "ras") => (true, true),
        Some("left-anterior-superior" | "las") => (false, true),
        Some(other) => {
            return Err(fmt_err(format!(
                "unsupported NRRD space {other:?}; supported: LPS, RAS, LAS"
            )));
        }
        None => (false, false),
    };
    let sign = [
        if flip_x { -1.0 } else { 1.0 },
        if flip_y { -1.0 } else { 1.0 },
        1.0,
    ];
    let mut axes = [[0.0; 3]; 3];
    let mut origin = [0.0; 3];
    let transform_source;
    if let Some(dirs) = get("space directions") {
        if space.is_none() {
            return Err(fmt_err(
                "NRRD has space directions but no space; refusing to guess the orientation",
            ));
        }
        let vectors = nrrd_vectors(dirs)?;
        if vectors.len() != 3 {
            return Err(fmt_err("NRRD space directions must list 3 vectors"));
        }
        for (a, v) in vectors.iter().enumerate() {
            let v = v.ok_or_else(|| fmt_err("NRRD space directions has a 'none' axis"))?;
            for r in 0..3 {
                axes[a][r] = sign[r] * v[r];
            }
        }
        if let Some(text) = get("space origin") {
            let o = nrrd_vectors(text)?;
            let o = o
                .first()
                .copied()
                .flatten()
                .ok_or_else(|| fmt_err("bad NRRD space origin"))?;
            for r in 0..3 {
                origin[r] = sign[r] * o[r];
            }
        }
        transform_source = "nrrd-space-directions";
    } else if let Some(spacings) = get("spacings") {
        // No orientation: the axes are the array axes, in LPS by assumption.
        let s = parse_floats(spacings, 3, "spacings")?;
        for a in 0..3 {
            axes[a][a] = s[a];
        }
        transform_source = "nrrd-spacings";
    } else {
        return Err(NiftiError::NoSpatialTransform);
    }
    let units_declared_mm = match get("space units") {
        Some(units) => {
            let list: Vec<&str> = units.split('"').filter(|s| !s.trim().is_empty()).collect();
            if list.is_empty() || list.iter().any(|u| !u.trim().eq_ignore_ascii_case("mm")) {
                return Err(fmt_err(format!(
                    "non-millimeter NRRD space units {units:?} are refused"
                )));
            }
            true
        }
        None => false,
    };

    let parsed = Parsed {
        shape,
        scalar,
        big_endian,
        codec,
        axes,
        origin,
        units_declared_mm,
        description: "NRRD",
        transform_source,
    };
    let line_skip: usize = get("line skip")
        .or_else(|| get("lineskip"))
        .map_or(Ok(0), str::parse)
        .map_err(|_| fmt_err("bad NRRD line skip"))?;
    let byte_skip: i64 = get("byte skip")
        .or_else(|| get("byteskip"))
        .map_or(Ok(0), str::parse)
        .map_err(|_| fmt_err("bad NRRD byte skip"))?;
    if byte_skip < -1 || (byte_skip == -1 && codec != Codec::Raw) {
        return Err(fmt_err("byte skip -1 is only valid for raw encoding"));
    }

    let detached;
    let payload: &[u8] = match get("data file").or_else(|| get("datafile")) {
        Some(name) => {
            if name.contains('%') || name.starts_with("LIST") || name.contains(' ') {
                return Err(fmt_err(
                    "multi-file NRRD data (LIST or printf patterns) is not supported",
                ));
            }
            let header_path = path.ok_or_else(|| {
                fmt_err("detached NRRD needs a file path to locate its data file")
            })?;
            detached = std::fs::read(safe_sibling(header_path, name)?)?;
            &detached
        }
        None => &bytes[data_start..],
    };
    let payload = skip_lines(payload, line_skip)?;
    let values = decode_payload(payload, &parsed, byte_skip, count)?;
    finalize(&parsed, values)
}

// ---------------------------------------------------------------------------
// MetaImage
// ---------------------------------------------------------------------------

fn meta_scalar(name: &str) -> Result<Scalar, NiftiError> {
    Ok(match name.trim().to_ascii_uppercase().as_str() {
        "MET_CHAR" => Scalar::I8,
        "MET_UCHAR" => Scalar::U8,
        "MET_SHORT" => Scalar::I16,
        "MET_USHORT" => Scalar::U16,
        "MET_INT" | "MET_LONG" => Scalar::I32,
        "MET_UINT" | "MET_ULONG" => Scalar::U32,
        "MET_FLOAT" => Scalar::F32,
        "MET_DOUBLE" => Scalar::F64,
        other => {
            return Err(fmt_err(format!(
                "unsupported MetaImage ElementType {other:?}; supported: CHAR, UCHAR, SHORT, \
                 USHORT, INT, UINT, FLOAT, DOUBLE"
            )));
        }
    })
}

fn meta_bool(value: &str) -> Result<bool, NiftiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        other => Err(fmt_err(format!("bad MetaImage boolean {other:?}"))),
    }
}

/// Read a `.mha` (embedded data) or `.mhd` (+ raw data file) file.
pub fn read_metaimage_file(path: &Path) -> Result<NiftiImage, NiftiError> {
    let bytes = std::fs::read(path)?;
    read_metaimage_impl(&bytes, Some(path))
}

/// Parse an embedded-data (`.mha`) MetaImage held in memory.
pub fn read_metaimage(bytes: &[u8]) -> Result<NiftiImage, NiftiError> {
    read_metaimage_impl(bytes, None)
}

fn read_metaimage_impl(bytes: &[u8], path: Option<&Path>) -> Result<NiftiImage, NiftiError> {
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut pos = 0;
    let mut data_start = None;
    while pos < bytes.len() {
        let end = bytes[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |p| pos + p);
        let line = String::from_utf8_lossy(&bytes[pos..end]);
        let line = line.trim_end_matches('\r').trim().to_owned();
        pos = (end + 1).min(bytes.len());
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let eq = line
            .find('=')
            .ok_or_else(|| fmt_err(format!("bad MetaImage header line {line:?}")))?;
        let key = line[..eq].trim().to_ascii_lowercase();
        let value = line[eq + 1..].trim().to_owned();
        if fields.iter().any(|(k, _)| *k == key) {
            return Err(fmt_err(format!("duplicate MetaImage field {key:?}")));
        }
        let is_data = key == "elementdatafile";
        fields.push((key, value));
        if is_data {
            data_start = Some(pos);
            break;
        }
    }
    let data_start = data_start.ok_or_else(|| fmt_err("MetaImage has no ElementDataFile"))?;
    let get = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == &key.to_ascii_lowercase())
            .map(|(_, v)| v.as_str())
    };
    let first_of = |keys: &[&str]| keys.iter().find_map(|k| get(k));

    if let Some(kind) = get("objecttype") {
        if !kind.eq_ignore_ascii_case("image") {
            return Err(fmt_err(format!("MetaImage ObjectType {kind:?} is not Image")));
        }
    }
    let ndims: usize = get("ndims")
        .ok_or_else(|| fmt_err("MetaImage has no NDims"))?
        .parse()
        .map_err(|_| fmt_err("bad MetaImage NDims"))?;
    if ndims != 3 {
        return Err(NiftiError::UnsupportedDimensions(ndims as i16, 1));
    }
    let dims = parse_floats(
        get("dimsize").ok_or_else(|| fmt_err("MetaImage has no DimSize"))?,
        3,
        "DimSize",
    )?;
    if dims.iter().any(|d| *d < 1.0 || d.fract() != 0.0) {
        return Err(fmt_err("bad MetaImage DimSize"));
    }
    let shape = [dims[0] as usize, dims[1] as usize, dims[2] as usize];
    let count = checked_voxels(shape)?;
    let scalar = meta_scalar(get("elementtype").ok_or_else(|| fmt_err("no ElementType"))?)?;
    if let Some(ch) = get("elementnumberofchannels") {
        if ch.trim() != "1" {
            return Err(fmt_err("multi-channel MetaImage volumes are not supported"));
        }
    }
    for key in ["elementtointensityfieldslope", "elementtointensityfieldoffset"] {
        if let Some(v) = get(key) {
            let expected = if key.ends_with("slope") { 1.0 } else { 0.0 };
            let x: f64 = v.parse().map_err(|_| fmt_err("bad MetaImage intensity map"))?;
            if x != expected {
                return Err(fmt_err(format!("MetaImage {key} is not supported")));
            }
        }
    }
    let binary = get("binarydata").map_or(Ok(true), meta_bool)?;
    let compressed = get("compresseddata").map_or(Ok(false), meta_bool)?;
    let big_endian = first_of(&["elementbyteordermsb", "binarydatabyteordermsb"])
        .map_or(Ok(false), meta_bool)?;
    let codec = match (binary, compressed) {
        (false, false) => Codec::Ascii,
        (true, false) => Codec::Raw,
        (true, true) => Codec::Zlib,
        (false, true) => return Err(fmt_err("compressed ASCII MetaImage is not supported")),
    };

    let spacing = match first_of(&["elementspacing", "elementsize"]) {
        Some(s) => parse_floats(s, 3, "ElementSpacing")?,
        None => vec![1.0; 3],
    };
    let origin = match first_of(&["offset", "position", "origin"]) {
        Some(s) => parse_floats(s, 3, "Offset")?,
        None => vec![0.0; 3],
    };
    let matrix = match first_of(&["transformmatrix", "rotation", "orientation"]) {
        Some(s) => parse_floats(s, 9, "TransformMatrix")?,
        None => vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };
    let mut axes = [[0.0; 3]; 3];
    for a in 0..3 {
        for r in 0..3 {
            // File lists the direction vector of each voxel axis in turn.
            axes[a][r] = matrix[a * 3 + r];
        }
        let norm = (axes[a][0].powi(2) + axes[a][1].powi(2) + axes[a][2].powi(2)).sqrt();
        if !(norm > 0.0) {
            return Err(fmt_err("zero-length TransformMatrix axis"));
        }
        for r in 0..3 {
            axes[a][r] = axes[a][r] / norm * spacing[a];
        }
    }

    let parsed = Parsed {
        shape,
        scalar,
        big_endian,
        codec,
        axes,
        origin: [origin[0], origin[1], origin[2]],
        units_declared_mm: false,
        description: "MetaImage",
        transform_source: "metaimage",
    };
    let header_size: i64 = get("headersize")
        .map_or(Ok(0), str::parse)
        .map_err(|_| fmt_err("bad MetaImage HeaderSize"))?;
    if header_size < -1 {
        return Err(fmt_err("bad MetaImage HeaderSize"));
    }
    let data_file = get("elementdatafile").unwrap_or_default();
    let detached;
    let (payload, byte_skip): (&[u8], i64) = if data_file.eq_ignore_ascii_case("local") {
        (&bytes[data_start..], 0)
    } else {
        if data_file.eq_ignore_ascii_case("list")
            || data_file.contains('%')
            || data_file.contains(' ')
        {
            return Err(fmt_err(
                "multi-file MetaImage data (LIST or printf patterns) is not supported",
            ));
        }
        let header_path = path
            .ok_or_else(|| fmt_err("detached MetaImage needs a file path to locate its data"))?;
        detached = std::fs::read(safe_sibling(header_path, data_file)?)?;
        (&detached, header_size)
    };
    if header_size != 0 && codec != Codec::Raw && byte_skip != 0 {
        return Err(fmt_err(
            "HeaderSize is only supported for uncompressed raw MetaImage data",
        ));
    }
    let values = decode_payload(payload, &parsed, byte_skip, count)?;
    finalize(&parsed, values)
}

// ---------------------------------------------------------------------------
// Format sniffing
// ---------------------------------------------------------------------------

/// Volume file formats `read_volume` understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeFormat {
    Nifti,
    Nrrd,
    MetaImage,
}

impl VolumeFormat {
    pub fn name(self) -> &'static str {
        match self {
            VolumeFormat::Nifti => "nifti",
            VolumeFormat::Nrrd => "nrrd",
            VolumeFormat::MetaImage => "metaimage",
        }
    }
}

/// Decide the format from magic bytes first, extension second.
pub fn sniff_volume_format(path: &Path) -> Result<VolumeFormat, NiftiError> {
    let mut head = Vec::new();
    std::fs::File::open(path)?.take(2048).read_to_end(&mut head)?;
    if head.starts_with(b"NRRD000") {
        return Ok(VolumeFormat::Nrrd);
    }
    if head.starts_with(&[0x1f, 0x8b]) {
        // Only NIfTI is ever wrapped whole in gzip.
        return Ok(VolumeFormat::Nifti);
    }
    if head.len() >= 348 && (head[344..348] == *b"n+1\0" || head[344..348] == *b"ni1\0") {
        return Ok(VolumeFormat::Nifti);
    }
    let text = String::from_utf8_lossy(&head[..head.len().min(512)]).to_ascii_lowercase();
    let meta_like = text
        .lines()
        .take(3)
        .any(|l| l.starts_with("objecttype") || l.starts_with("ndims") || l.starts_with("dimsize"));
    if meta_like {
        return Ok(VolumeFormat::MetaImage);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.ends_with(".nii") || name.ends_with(".nii.gz") {
        return Ok(VolumeFormat::Nifti);
    }
    if name.ends_with(".nrrd") || name.ends_with(".nhdr") {
        return Ok(VolumeFormat::Nrrd);
    }
    if name.ends_with(".mha") || name.ends_with(".mhd") {
        return Ok(VolumeFormat::MetaImage);
    }
    Err(fmt_err(format!(
        "cannot identify {} as a volume; supported formats: NIfTI (.nii/.nii.gz), \
         NRRD (.nrrd/.nhdr), MetaImage (.mha/.mhd)",
        path.display()
    )))
}

/// Read any supported 3-D scalar volume (NIfTI, NRRD, MetaImage), sniffing
/// the format from magic bytes and extension.
pub fn read_volume(path: &Path) -> Result<NiftiImage, NiftiError> {
    match sniff_volume_format(path)? {
        VolumeFormat::Nifti => crate::read_nifti_file(path),
        VolumeFormat::Nrrd => read_nrrd_file(path),
        VolumeFormat::MetaImage => read_metaimage_file(path),
    }
}
