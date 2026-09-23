// SPDX-License-Identifier: MIT

//! Minimal `.npy` v1.0 encoder + `.npz` (zip) writer — the engine's
//! voxel-plan container. `cls` is int8, ROI masks are bool; both are
//! written little-endian, C-order, matching `np.savez` output.

use std::io::Write;

use zip::write::SimpleFileOptions;

use crate::error::AvifyError;

fn npy_bytes(descr: &str, shape_zyx: [usize; 3], data: &[u8]) -> Vec<u8> {
    let header = format!(
        "{{'descr': '{descr}', 'fortran_order': False, 'shape': ({}, {}, {}), }}",
        shape_zyx[0], shape_zyx[1], shape_zyx[2]
    );
    // v1.0: magic(6) + ver(2) + hlen(2) + header padded to 16-byte boundary, ending in \n
    let preamble = 10usize;
    let mut padded = header.clone();
    let target = (preamble + header.len() + 1).div_ceil(16) * 16;
    for _ in 0..(target - preamble - header.len() - 1) {
        padded.push(' ');
    }
    padded.push('\n');
    let mut out = Vec::with_capacity(target + data.len());
    out.extend_from_slice(b"\x93NUMPY");
    out.extend_from_slice(&[1u8, 0u8]);
    out.extend_from_slice(&(padded.len() as u16).to_le_bytes());
    out.extend_from_slice(padded.as_bytes());
    out.extend_from_slice(data);
    out
}

fn write_npy(
    w: &mut zip::ZipWriter<impl Write + std::io::Seek>,
    name: &str,
    descr: &str,
    shape_zyx: [usize; 3],
    data: &[u8],
) -> Result<(), AvifyError> {
    w.start_file(name, SimpleFileOptions::default())?;
    w.write_all(&npy_bytes(descr, shape_zyx, data))?;
    Ok(())
}

/// Write `<prefix>_arrays.npz` containing the class map and ROI masks.
pub fn write_arrays_npz(
    prefix: &std::path::Path,
    cls_zyx: &[i8],
    rois: &[(String, Vec<bool>)],
    shape_zyx: [usize; 3],
) -> Result<std::path::PathBuf, AvifyError> {
    let path = std::path::PathBuf::from(format!("{}_arrays.npz", prefix.display()));
    let file = std::fs::File::create(&path)?;
    let mut w = zip::ZipWriter::new(file);
    let cls_bytes: Vec<u8> = cls_zyx.iter().map(|&v| v as u8).collect();
    write_npy(&mut w, "cls.npy", "|i1", shape_zyx, &cls_bytes)?;
    for (name, mask) in rois {
        let bytes: Vec<u8> = mask.iter().map(|&b| b as u8).collect();
        write_npy(&mut w, &format!("roi_{name}.npy"), "|b1", shape_zyx, &bytes)?;
    }
    w.finish()?;
    Ok(path)
}

/// Parsed `<prefix>_arrays.npz` — the class map plus ROI masks, both in
/// z-y-x C order.
pub struct ArraysNpz {
    pub shape_zyx: [usize; 3],
    pub cls_zyx: Vec<i8>,
    /// `roi_<name>` → mask (name without the `roi_` prefix).
    pub rois: Vec<(String, Vec<bool>)>,
}

/// Parse one `.npy` v1.0/v2.0 blob: header dict fields + raw data.
/// Accepts only what the writer (and `np.savez`) can emit for this
/// container: `|i1`/`|b1`, little-endian, C-order.
fn parse_npy(bytes: &[u8]) -> Result<(String, Vec<usize>, Vec<u8>), AvifyError> {
    if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
        return Err(AvifyError::Invalid("not an .npy blob".into()));
    }
    let (major, header_len, mut at) = match bytes[6] {
        1 => (1, u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => (
            2,
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        ),
        v => return Err(AvifyError::Invalid(format!("npy version {v} unsupported"))),
    };
    let _ = major;
    let header = std::str::from_utf8(
        bytes
            .get(at..at + header_len)
            .ok_or_else(|| AvifyError::Invalid("truncated npy header".into()))?,
    )
    .map_err(|_| AvifyError::Invalid("npy header not utf-8".into()))?;
    at += header_len;
    let field = |key: &str| -> Result<String, AvifyError> {
        let marker = format!("'{key}':");
        let start = header
            .find(&marker)
            .map(|i| i + marker.len())
            .ok_or_else(|| AvifyError::Invalid(format!("npy header lacks {key}")))?;
        let value = header[start..].trim_start();
        let end = value
            .find([',', '}'])
            .ok_or_else(|| AvifyError::Invalid(format!("npy {key} unterminated")))?;
        Ok(value[..end].trim().to_string())
    };
    let descr = field("descr")?.trim_matches('\'').to_string();
    if field("fortran_order")? != "False" {
        return Err(AvifyError::Invalid(
            "fortran_order arrays unsupported".into(),
        ));
    }
    // `shape` is a tuple literal — its commas would defeat `field`, so
    // extract between the parens directly.
    let shape_marker = "'shape':";
    let shape_start = header
        .find(shape_marker)
        .map(|i| i + shape_marker.len())
        .ok_or_else(|| AvifyError::Invalid("npy header lacks shape".into()))?;
    let rest = &header[shape_start..];
    let open = rest
        .find('(')
        .ok_or_else(|| AvifyError::Invalid("npy shape not a tuple".into()))?;
    let close = rest[open..]
        .find(')')
        .map(|i| open + i)
        .ok_or_else(|| AvifyError::Invalid("npy shape unterminated".into()))?;
    let shape: Vec<usize> = rest[open + 1..close]
        .split(',')
        .filter_map(|t| t.trim().parse::<usize>().ok())
        .collect();
    if shape.is_empty() {
        return Err(AvifyError::Invalid("npy shape empty".into()));
    }
    let n: usize = shape.iter().product();
    let data = bytes
        .get(at..at + n)
        .ok_or_else(|| AvifyError::Invalid("truncated npy data".into()))?
        .to_vec();
    Ok((descr, shape, data))
}

/// Read `<prefix>_arrays.npz` back — the connector's own format plus
/// `np.savez` output with the same member names.
pub fn read_arrays_npz(path: &std::path::Path) -> Result<ArraysNpz, AvifyError> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let mut cls = None;
    let mut shape_zyx = None;
    let mut rois = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let name = entry.name().to_string();
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        std::io::Read::read_to_end(&mut entry, &mut bytes)?;
        let (descr, shape, data) = parse_npy(&bytes)?;
        if shape.len() != 3 {
            return Err(AvifyError::Invalid(format!(
                "{name}: expected 3-D, got {shape:?}"
            )));
        }
        match name.as_str() {
            "cls.npy" => {
                if descr != "|i1" {
                    return Err(AvifyError::Invalid(format!("cls descr {descr}")));
                }
                shape_zyx = Some([shape[0], shape[1], shape[2]]);
                cls = Some(data.iter().map(|&b| b as i8).collect::<Vec<i8>>());
            }
            _ if name.starts_with("roi_") && name.ends_with(".npy") => {
                if descr != "|b1" {
                    return Err(AvifyError::Invalid(format!("{name} descr {descr}")));
                }
                rois.push((
                    name["roi_".len()..name.len() - ".npy".len()].to_string(),
                    data.iter().map(|&b| b != 0).collect::<Vec<bool>>(),
                ));
            }
            _ => {}
        }
    }
    let shape_zyx = shape_zyx.ok_or_else(|| AvifyError::Invalid("npz lacks cls.npy".into()))?;
    let cls_zyx = cls.unwrap();
    let n: usize = shape_zyx.iter().product();
    if cls_zyx.len() != n {
        return Err(AvifyError::Invalid("cls length/shape mismatch".into()));
    }
    rois.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(ArraysNpz {
        shape_zyx,
        cls_zyx,
        rois,
    })
}
