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
