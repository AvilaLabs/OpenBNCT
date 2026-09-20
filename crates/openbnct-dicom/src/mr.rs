// SPDX-License-Identifier: Apache-2.0

//! DICOM MR series import for targeting overlays.
//!
//! MRI is the modality that defines GTVs in real BNCT workflows; the
//! workbench keeps CT as the transport-geometry source (electron
//! density drives materials) and treats MR as a co-registered display
//! and targeting volume. Import assembles the series on its own
//! declared grid — the same orientation/uniformity gates as CT — and
//! emits rescaled (unitless) intensities, never pseudo-HU.
//!
//! Resampling onto the case CT grid is a separate, explicit step
//! (`register landmarks`/`register apply`) so the registration basis is
//! recorded in an artifact rather than silently assumed; series sharing
//! a Frame of Reference with the CT are co-registered by declaration.

use std::path::{Path, PathBuf};

use dicom_dictionary_std::uids;
use dicom_object::open_file;
use openbnct_core::GridGeometry;

use crate::ct::{PixelKind, assemble_series, read_series};
use crate::{DicomError, Result};

/// A validated MR series on its own declared grid — intensities are
/// rescaled per Rescale Slope/Intercept but are unitless by nature.
#[derive(Debug, Clone, PartialEq)]
pub struct MrVolume {
    pub geometry: GridGeometry,
    pub frame_of_reference_uid: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    /// SOP Instance UIDs ordered along the positive slice-normal axis.
    pub slice_sop_instance_uids: Vec<String>,
    /// Rescaled signal intensities in grid order `i + nx·j + nx·ny·k`.
    pub intensities: Vec<f64>,
    /// Repetition Time (0018,0080) in ms when present — the sequence
    /// hint a reader wants alongside an MR overlay.
    pub repetition_time_ms: Option<f64>,
    /// Echo Time (0018,0081) in ms when present.
    pub echo_time_ms: Option<f64>,
}

/// Import an MR series onto its declared grid.
pub fn import_mr_series(paths: &[PathBuf]) -> Result<MrVolume> {
    let slices = read_series(paths, uids::MR_IMAGE_STORAGE, "MR", PixelKind::Either16)?;
    let reference_path = slices
        .first()
        .ok_or_else(|| DicomError::EmptySeries)?
        .path
        .clone();
    let assembled = assemble_series(slices)?;
    let obj = open_file(&reference_path).map_err(|source| DicomError::Read {
        path: reference_path.clone(),
        source: Box::new(source),
    })?;
    mr_from_reference(&obj, &reference_path, assembled)
}

/// In-memory variant of [`import_mr_series`] for hosts without a
/// filesystem.
pub fn import_mr_series_from_bytes(files: &[(String, Vec<u8>)]) -> Result<MrVolume> {
    let slices =
        crate::ct::read_series_bytes(files, uids::MR_IMAGE_STORAGE, "MR", PixelKind::Either16)?;
    let assembled = assemble_series(slices)?;
    let (name, bytes) = files.first().ok_or_else(|| DicomError::EmptySeries)?;
    let reference_path = PathBuf::from(name);
    let obj = dicom_object::DefaultDicomObject::from_reader(std::io::Cursor::new(bytes)).map_err(
        |source| DicomError::Read {
            path: reference_path.clone(),
            source: Box::new(source),
        },
    )?;
    mr_from_reference(&obj, &reference_path, assembled)
}

fn mr_from_reference(
    obj: &dicom_object::DefaultDicomObject,
    reference_path: &Path,
    assembled: crate::ct::AssembledSeries,
) -> Result<MrVolume> {
    // Sequence timing is optional metadata — absent tags degrade to None
    // rather than rejecting a legal series.
    let timing = |tag: dicom_core::Tag| -> Option<f64> {
        obj.element(tag)
            .ok()
            .and_then(|e| e.to_float64().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    let _ = reference_path;
    let intensities = assembled
        .stored_pixels
        .iter()
        .map(|stored| {
            f64::from(*stored).mul_add(assembled.rescale_slope, assembled.rescale_intercept)
        })
        .collect();
    Ok(MrVolume {
        geometry: assembled.geometry,
        frame_of_reference_uid: assembled.frame_of_reference_uid,
        study_instance_uid: assembled.study_instance_uid,
        series_instance_uid: assembled.series_instance_uid,
        slice_sop_instance_uids: assembled.slice_sop_instance_uids,
        intensities,
        repetition_time_ms: timing(dicom_core::Tag(0x0018, 0x0080)),
        echo_time_ms: timing(dicom_core::Tag(0x0018, 0x0081)),
    })
}
