// SPDX-License-Identifier: MIT

//! General-purpose DICOM study import — the research-case counterpart
//! of the frozen NF-BNCT-001 benchmark verifier.
//!
//! [`import_study_from_files`] and [`import_study_from_paths`] bucket a
//! mixed pile of Part-10 objects by SOP Class and Series UID, run the
//! same strict CT/RTSTRUCT/PET parsers every other path uses, and
//! hash-bind the accepted members so an imported study carries its own
//! content identity. Nothing is compared against frozen values — the
//! case's integrity claim is *generated* here, not verified against a
//! fixture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dicom_dictionary_std::{tags, uids};
use dicom_object::DefaultDicomObject;
use openbnct_evidence::sha256_hex;

use crate::{
    CtVolume, DicomError, PetVolume, RoiReport, StructureSet, import_ct_series_from_bytes,
    import_pet_series_from_bytes, import_rtstruct_bytes,
};

/// One imported research case: geometry, contours, optional PET, and the
/// content binding of every member that fed it.
#[derive(Debug, Clone)]
pub struct ImportedStudy {
    /// `study-<digest16>` — stable identifier derived from member bytes.
    pub case_id: String,
    /// SHA-256 over the sorted `name\nbytes` member list.
    pub sha256: String,
    /// DICOM members that entered the import.
    pub member_count: usize,
    /// Members skipped by bucketing (not Part-10, unsupported modality).
    pub ignored: Vec<String>,
    pub ct: CtVolume,
    pub structures: StructureSet,
    /// Computed per-ROI measurements (voxels, volume, centroid) — the
    /// general form of the benchmark's frozen comparisons.
    pub rois: Vec<RoiReport>,
    /// PET SUV series when the study carried one.
    pub pet: Option<PetVolume>,
}

fn is_part10(bytes: &[u8]) -> bool {
    bytes.len() > 132 && &bytes[128..132] == b"DICM"
}

fn study_error(message: impl Into<String>) -> DicomError {
    DicomError::StudyImport(message.into())
}

/// A member's SOP Class UID decides its bucket; Series Instance UID keeps
/// a multi-series study honest instead of silently merging stacks.
fn sniff_member(name: &str, bytes: &[u8]) -> Result<Option<(String, String)>, DicomError> {
    if !is_part10(bytes) {
        return Ok(None);
    }
    let path = PathBuf::from(name);
    let object =
        DefaultDicomObject::from_reader(std::io::Cursor::new(bytes)).map_err(|source| {
            DicomError::Read {
                path,
                source: Box::new(source),
            }
        })?;
    let sop_class = object
        .element(tags::SOP_CLASS_UID)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.to_string());
    let series_uid = object
        .element(tags::SERIES_INSTANCE_UID)
        .ok()
        .and_then(|e| e.to_str().ok())
        .map(|s| s.to_string());
    Ok(sop_class.map(|class| (class, series_uid.unwrap_or_default())))
}

/// Import a study from in-memory members (`name`, `bytes`) — the web
/// drop/zip path. CT slices group by Series Instance UID; exactly one CT
/// series and exactly one RTSTRUCT are required, PET is optional, and
/// everything else is listed in `ignored` rather than guessed at.
pub fn import_study_from_files(files: &[(String, Vec<u8>)]) -> Result<ImportedStudy, DicomError> {
    let mut ct_series: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    let mut pet_series: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    let mut rtstruct: Vec<(String, Vec<u8>)> = Vec::new();
    let mut ignored: Vec<String> = Vec::new();

    for (name, bytes) in files {
        match sniff_member(name, bytes)? {
            Some((sop, series)) if sop == uids::CT_IMAGE_STORAGE => {
                ct_series
                    .entry(series)
                    .or_default()
                    .push((name.clone(), bytes.clone()));
            }
            Some((sop, series)) if sop == uids::POSITRON_EMISSION_TOMOGRAPHY_IMAGE_STORAGE => {
                pet_series
                    .entry(series)
                    .or_default()
                    .push((name.clone(), bytes.clone()));
            }
            Some((sop, _)) if sop == uids::RT_STRUCTURE_SET_STORAGE => {
                rtstruct.push((name.clone(), bytes.clone()));
            }
            Some(_) => ignored.push(name.clone()),
            None => ignored.push(name.clone()),
        }
    }

    if ct_series.is_empty() {
        return Err(study_error("no CT Image Storage series found"));
    }
    if ct_series.len() > 1 {
        let detail = ct_series
            .iter()
            .map(|(uid, members)| format!("{uid} ({} slices)", members.len()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(study_error(format!(
            "multiple CT series present — import one at a time: {detail}"
        )));
    }
    if rtstruct.is_empty() {
        return Err(study_error(
            "no RT Structure Set found — a BNCT case needs contours",
        ));
    }
    if rtstruct.len() > 1 {
        return Err(study_error(format!(
            "multiple RT Structure Sets present ({}) — import one at a time",
            rtstruct.len()
        )));
    }
    if pet_series.len() > 1 {
        return Err(study_error(format!(
            "multiple PET series present — import one at a time ({})",
            pet_series.len()
        )));
    }

    let (ct_uid, ct_files) = ct_series.into_iter().next().expect("checked non-empty");
    let ct = import_ct_series_from_bytes(&ct_files)
        .map_err(|error| study_error(format!("CT series {ct_uid}: {error}")))?;
    let (_rt_name, rt_bytes) = rtstruct.into_iter().next().expect("checked non-empty");
    let structures = import_rtstruct_bytes(&rt_bytes, &ct)?;
    let pet = pet_series
        .into_iter()
        .next()
        .map(|(uid, members)| {
            import_pet_series_from_bytes(&members)
                .map_err(|error| study_error(format!("PET series {uid}: {error}")))
        })
        .transpose()?;

    let rois = structures
        .rois
        .iter()
        .map(|roi| RoiReport {
            number: roi.number,
            name: roi.name.clone(),
            voxel_count: roi.voxel_count(),
            volume_cm3: roi.volume_cm3(&ct),
            centroid_lps_mm: roi.centroid_lps_mm(&ct).unwrap_or([f64::NAN; 3]),
        })
        .collect();

    let mut digest_input = Vec::new();
    let mut member_count = 0usize;
    for (name, bytes) in files {
        if ignored.contains(name) {
            continue;
        }
        digest_input.extend_from_slice(name.as_bytes());
        digest_input.push(b'\n');
        digest_input.extend_from_slice(bytes);
        member_count += 1;
    }
    let sha256 = sha256_hex(&digest_input);

    Ok(ImportedStudy {
        case_id: format!("study-{}", &sha256[..16]),
        sha256,
        member_count,
        ignored,
        ct,
        structures,
        rois,
        pet,
    })
}

/// Filesystem variant — walks `paths` (directories recurse one level is
/// NOT implied: pass every file) and delegates to the bytes path so both
/// imports share one implementation.
pub fn import_study_from_paths(paths: &[PathBuf]) -> Result<ImportedStudy, DicomError> {
    let mut files = Vec::new();
    for path in paths {
        let bytes = std::fs::read(path).map_err(|source| DicomError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("member")
            .to_owned();
        files.push((name, bytes));
    }
    import_study_from_files(&files)
}

/// Collect candidate members under `root` (recursive) without parsing —
/// Part-10 magic is checked inside the importer itself.
pub fn collect_study_paths(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}
